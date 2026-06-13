# 模块设计：PythonFFI-03 Container / ListContainer

## 模块位置

`construct-py/construct_rust/lib/containers.py`（纯 Python 实现）

## 职责

提供与 Python 原版 construct 库行为完全一致的 `Container`（dict 子类）和 `ListContainer`（list 子类），支持属性+下标双访问、多行缩进格式化打印、正则搜索、全局打印标志控制。作为 Rust `Value::Container` / `Value::List` 在 Python 侧的落地类型。

---

## 1. 设计方案选择

### 1.1 核心技术挑战

原版 Container 继承 `dict` 并使用 `self.__dict__ = self` trick（`containers.py:110`），使得：

1. **属性-下标完全等价**：`c.a` 与 `c["a"]` 是同一份数据
2. **Method shadowing**：当 dict 中存在与方法同名的键时（如 `"items"`、`"search"`、`"update"`），属性访问返回键值而非方法。原版源码注释（line 85）明确写道："Any method can be shadowed"
3. **`__slots__ = ('__dict__', '__recursion_lock__')`**：限制实例属性仅为这两个 slot

测试 `test_method_shadowing_1` / `test_method_shadowing_2`（`test_containers_dict.py:293-323`）明确验证此行为：`c['update'] = 42` 后 `c.update == 42`。

### 1.2 方案对比

| 维度 | 方案 A：PyO3 `#[pyclass(extends=PyDict)]` | 方案 B：纯 Python（推荐） |
|------|------------------------------------------|--------------------------|
| `__dict__ = self` trick | 无法直接实现。PyO3 extends 模式下实例 `__dict__` 与 dict 数据分离 | **完美复刻**：`self.__dict__ = self` 一行搞定 |
| Method shadowing | 需覆盖 `__getattribute__`（`tp_getattro`），手动实现先查 dict 数据再 fallback 到类型方法。fallback 逻辑涉及描述符 `__get__` 绑定，实现复杂且有递归风险 | **天然支持**：`__dict__ = self` 使 dict 数据优先于类型方法 |
| `isinstance(c, dict)` | `True`（extends PyDict） | **`True`**（继承 dict） |
| pickle（`__getstate__`/`__setstate__`） | PyO3 dict 子类的 pickle 支持不确定，需额外适配 | **天然支持**：原版 `__getstate__`/`__setstate__` 直接移植 |
| copy/deepcopy | 需实现 `__copy__`/`__deepcopy__` | **天然支持**：原版 `__copy__`/`__deepcopy__` 直接移植 |
| 开发成本 | 高（`__getattribute__` + 描述符 fallback + 各种边界） | **低**（移植原版 290 行代码，几乎逐行 1:1） |
| 行为差异风险 | 中（极端边界可能不一致） | **零**（逐行移植） |
| 性能影响 | 属性访问走 `__getattribute__` 变慢；但不在解析热路径 | **无影响**：方法不在热路径（Rust 内核解析结果为 `Value`，仅在 `value_to_py` 出口处构造） |

### 1.3 推荐方案：纯 Python

**推荐方案 B（纯 Python）**，在 `construct-py/construct_rust/lib/containers.py` 中实现。理由：

1. **行为零差异**：`__dict__ = self` trick 是 Python 特有的深层实例字典机制。PyO3 无法原生模拟此 trick，需通过覆盖 `__getattribute__` 间接实现，引入不必要的复杂性和风险。
2. **测试通过率最大化**：36 个 dict 测试 + 1 个 list 测试 + 6 个 search 测试均可通过（仅 `test_eq_numpy` 因 numpy 依赖跳过）。
3. **与总纲一致**：总纲已规定 `construct_rust/lib/` 中的 `binary.py`/`py3compat.py`/`hex.py`/`bitstream.py` 为纯 Python 实现。Container 放入同一目录，架构一致性更好。
4. **性能无劣势**：Container/ListContainer 方法（`__repr__`/`__str__`/`search`）仅在用户交互（打印、搜索）时调用，不在 Rust 内核解析热路径上。`value_to_py` 出口处构造 Container 对象的成本与 PyO3 方案相同（都需要遍历键值对）。

### 1.4 对总纲决策的修正

总纲设计决策表中的决策：
> Container 模拟 | `#[pyclass]` 继承 dict | 通过 `__getattr__`/`__setattr__` 实现属性+下标双访问

**修正为**：
> Container 模拟 | 纯 Python（继承 dict） | 通过 `__dict__ = self` trick 实现属性+下标双访问（与原版完全一致）

### 1.5 对 10.2 接口的影响

10.2 设计文档中 `value_to_py` 对 `Value::Container` / `Value::List` 的处理原为"构造 `Py<PyContainer>`"。纯 Python 方案下调整为：

```rust
// value_to_py 中 Value::Container 分支
Value::Container(m) => {
    let cls = py.import("construct_rust.lib.containers")?
        .getattr("Container")?;
    let dict = PyDict::new(py);
    for (k, v) in m {
        dict.set_item(k, value_to_py(py, v)?)?;
    }
    cls.call1((dict,))?.into()
}

// value_to_py 中 Value::List 分支
Value::List(v) => {
    let cls = py.import("construct_rust.lib.containers")?
        .getattr("ListContainer")?;
    let items: Vec<PyObject> = v.iter()
        .map(|v| value_to_py(py, v))
        .collect::<PyResult<_>>()?;
    cls.call1((items,))?.into()
}
```

`py_to_value` 对 Container 的识别：`isinstance(obj, Container)` 或 `isinstance(obj, dict)` → `Value::Container`（已有逻辑兼容，无需修改）。

`context_to_py_container`：逻辑不变，构造 Container 对象（改为调用 Python Container 类）。

**模块缓存**：`value_to_py` 频繁调用时，每次 `py.import` 有开销。建议在 construct-py 初始化时缓存 Container/ListContainer 类引用（`OnceCell<PyObject>` 或在 `lib.rs` 模块初始化中预存）。

---

## 2. Container 类详细设计

### 2.1 类定义

```python
class Container(dict):
    """Generic ordered dictionary that allows both key and attribute access,
    and preserves key order by insertion. Equality does NOT check item order.
    Also provides regex searching."""

    __slots__ = ('__dict__', '__recursion_lock__')
```

- 继承 `dict`，使 `isinstance(c, dict)` 为 `True`
- `__slots__` 限制实例属性为 `__dict__`（属性字典 = self）和 `__recursion_lock__`（递归锁标记）

### 2.2 构造函数

```python
def __init__(self, *args, **kwargs):
    super().__init__(*args, **kwargs)
    self.__dict__ = self
```

**接受参数形式**（与原版 `dict.__init__` 一致）：

| 调用形式 | 示例 | 行为 |
|---------|------|------|
| 无参数 | `Container()` | 空容器 |
| dict | `Container(d)` / `Container({"a":1})` | 从 dict 复制键值对 |
| Container | `Container(c)` | 从 Container 复制（保留顺序） |
| 序列 of (k,v) | `Container([("a",1),("b",2)])` | 从元组列表构造 |
| keyword args | `Container(a=1, b=2)` | 从关键字参数构造 |
| 混合 | `Container(d, x=9)` | 先复制 positional，再覆盖 keyword |

**关键**：`self.__dict__ = self` 使得后续所有属性操作（`c.a`、`del c.a`）直接作用于 dict 数据本身。

### 2.3 属性-下标双访问

由 `__dict__ = self` trick 自动实现，无需额外代码：

| 操作 | 实际执行 | 说明 |
|------|---------|------|
| `c.a`（读） | 查 `c.__dict__["a"]` → 等价于 `c["a"]` | 属性查找命中实例字典（= self） |
| `c.a = 1`（写） | `c.__dict__["a"] = 1` → 等价于 `c["a"] = 1` | 写入实例字典 |
| `del c.a`（删） | `del c.__dict__["a"]` → 等价于 `del c["a"]` | 从实例字典删除 |
| `c.a`（键不存在） | `AttributeError` | 实例字典查找失败 → 属性查找失败 |
| `c["a"]`（键不存在） | `KeyError` | dict 原生行为 |

**Method shadowing**：由于实例字典（= dict 数据）在属性查找链中优先于类型方法（非数据描述符），dict 中与方法同名的键会 shadow 方法：

```python
c = Container()
assert c.update != 42          # 此时 dict 无 "update" 键 → 返回 dict.update 方法
c['update'] = 42
assert c.update == 42          # dict 有 "update" 键 → 返回 42（shadow 了方法）
```

绕过 shadowing 的方法：通过类型显式调用 `Container.search(c, 'x')`（对应 `test_method_shadowing_2:321`）。

### 2.4 `__repr__`

```python
@recursion_lock()
def __repr__(self):
    parts = []
    for k, v in dict.items(self):
        if isinstance(k, str) and k.startswith("_"):
            continue
        parts.append(f'{k}={v!r}')
    return "Container(%s)" % ", ".join(parts)
```

行为规则：
- 格式：`Container(k1=v1, k2=v2, ...)`（Python repr 格式，单行）
- **跳过 `_` 前缀键**（私有字段不显示）
- `v!r` 使用 Python 标准 `repr()` 格式化值
- 递归保护：自引用时输出 `<recursion detected>`（见 §5.1 recursion_lock）
- 空容器：`Container()`（`eval(repr(c)) == c` 必须成立）

关键实现细节：
- `dict.items(self)` 而非 `self.items()`，因为 `self.items` 可能被 shadow（原版注释 line 85）
- 测试 `test_str_repr_recursive` 验证递归保护

### 2.5 `__str__`

```python
@recursion_lock()
def __str__(self):
    indentation = "\n    "
    text = ["Container: "]
    isflags = getattr(self, "_flagsenum", False)
    for k, v in dict.items(self):
        if isinstance(k, str) and k.startswith("_") and not globalPrintPrivateEntries:
            continue
        if isflags and not v and not globalPrintFalseFlags:
            continue
        text.extend([indentation, str(k), " = ",
                     indentation.join(value_to_string(v).split("\n"))])
    return "".join(text)
```

行为规则：
- 格式：多行缩进，`Container:` 开头
- **私有键（`_` 前缀）**：默认隐藏，`globalPrintPrivateEntries=True` 时显示
- **False flags**：当 `_flagsenum` 标记为 True 且值为 falsy 时，默认隐藏，`globalPrintFalseFlags=True` 时显示
- 值通过 `value_to_string()` 格式化（见 §5.2），支持 bytes/str 截断
- 多行值（如嵌套 Container）按缩进对齐
- 递归保护：自引用时输出 `<recursion detected>`

测试用例：
- `test_str_repr_empty`：`str(Container()) == "Container: "`
- `test_str_repr`：三键容器的精确格式
- `test_str_repr_nested`：嵌套空 Container 的格式
- `test_fullstrings`：全局打印标志对 bytes/str 截断的控制
- `test_falseflags`：FlagsEnum 解析结果的 false flag 过滤
- `test_privateentries`：私有键的显示/隐藏控制

### 2.6 `__eq__` / `__ne__`

```python
def __eq__(self, other):
    if self is other:
        return True
    if not isinstance(other, dict):
        return False
    def isequal(v1, v2):
        if v1.__class__.__name__ == "ndarray" or v2.__class__.__name__ == "ndarray":
            import numpy
            return numpy.array_equal(v1, v2)
        return v1 == v2
    for k, v in dict.items(self):
        if isinstance(k, str) and k.startswith("_"):
            continue
        if k not in other or not isequal(v, other[k]):
            return False
    for k, v in dict.items(other):
        if isinstance(k, str) and k.startswith("_"):
            continue
        if k not in self or not isequal(v, self[k]):
            return False
    return True

def __ne__(self, other):
    return not self == other
```

行为规则：
- **顺序无关**：不检查键的插入顺序，只比较键值对集合
- **跳过 `_` 前缀键**：私有字段不参与比较（`_io`、`_flagsenum` 等）
- **双向遍历**：两个方向各遍历一次，确保 A⊆B 且 B⊆A
- **numpy 特殊处理**：如果值是 ndarray，用 `numpy.array_equal` 比较
- **与普通 dict 可比较**：`Container(a=1) == {"a": 1}` 为 `True`
- `isinstance(other, dict)` 检查使 Container 与任何 dict 子类可比较

关键实现细节：
- `dict.items(self)` 而非 `self.items()`（防 shadowing）
- `k not in other` 使用 `__contains__`（不受 shadowing 影响，原版注释 line 85）
- `other[k]` 使用 `__getitem__`（同上）

测试用例：`test_eq_issue_818`、`test_eq_numpy`（跳过）、`test_ne_issue_818`

### 2.7 dict 协议方法

Container 继承 `dict`，以下方法**直接从 dict 继承**，无需重新实现：

| 方法 | 继承来源 | 行为 |
|------|---------|------|
| `__getitem__` / `__setitem__` / `__delitem__` | dict | 下标访问，KeyError |
| `__contains__` | dict | `in` 运算符 |
| `__iter__` | dict | 迭代键 |
| `__len__` | dict | 键数量 |
| `__bool__` | dict | 非空判断 |
| `keys()` / `values()` / `items()` | dict | 保持插入顺序的视图 |
| `get(key, default)` | dict | 安全获取 |
| `pop(key)` | dict | 弹出并返回 |
| `popitem()` | dict | 弹出末尾项 |
| `update(*args, **kwargs)` | dict | 批量更新 |
| `clear()` | dict | 清空 |
| `setdefault(key, default)` | dict | 设置默认值 |

**注意**：这些方法可被 shadow（见 §2.3），但 dict 的 C 层操作（如 `k in self`、`self[k]`）不受影响。

测试用例：`test_keys`、`test_values`、`test_items`、`test_iter`、`test_clear`、`test_pop`、`test_popitem`、`test_update_dict`、`test_update_seqoftuples`、`test_len_bool`、`test_in`

### 2.8 copy / pickle

```python
def copy(self):
    return self.__class__(self)

def __copy__(self):
    return self.__class__.copy(self)

def __deepcopy__(self, _):
    return self.__class__.copy(self)

def __getstate__(self):
    return dict(self)

def __setstate__(self, state):
    dict.clear(self)
    dict.update(self, state)

def __dir__(self):
    return list(dict.keys(self)) + list(type(self).__dict__) + dir(dict)
```

| 方法 | 行为 | 测试 |
|------|------|------|
| `copy()` | 浅拷贝（构造新 Container） | `test_copy_method` |
| `__copy__()` | `copy.copy(c)` 的协议方法 | `test_copy` |
| `__deepcopy__()` | `copy.deepcopy(c)` 的协议方法，**浅拷贝语义**（原版行为，注释 line 118-121） | `test_deepcopy` |
| `__getstate__()` | pickle 序列化 → 普通 dict | `test_pickling` |
| `__setstate__()` | pickle 反序列化 ← 普通 dict | `test_pickling` |
| `__dir__()` | 自动补全（返回 dict 键 + 类型属性 + dict 方法） | — |

**关键**：`__deepcopy__` 返回浅拷贝而非深拷贝。原版注释解释："otherwise copy.deepcopy() will copy self and self.__dict__ separately for some reason"。`test_deepcopy` 验证 `d.a = 2` 后 `c.a != 2`（因为 c 和 d 是不同对象，但内部值是浅拷贝，所以简单值如 int 不受影响——Python int 不可变，赋值创建新对象）。

### 2.9 search / search_all

```python
def _search(self, compiled_pattern, search_all):
    items = []
    for key, value in dict.items(self):
        try:
            if isinstance(value, (Container, ListContainer)):
                ret = value.__class__._search(value, compiled_pattern, search_all)
                if ret is not None:
                    if search_all:
                        items.extend(ret)
                    else:
                        return ret
            elif compiled_pattern.match(key):
                if search_all:
                    items.append(value)
                else:
                    return value
        except Exception:
            pass
    if search_all:
        return items
    else:
        return None

def search(self, pattern):
    compiled_pattern = re.compile(pattern)
    return self.__class__._search(self, compiled_pattern, False)

def search_all(self, pattern):
    compiled_pattern = re.compile(pattern)
    return self.__class__._search(self, compiled_pattern, True)
```

**搜索算法**（`_search`）：

| 步骤 | 行为 |
|------|------|
| 1. 遍历 | 按插入顺序遍历容器的键值对 |
| 2. 值是 Container/ListContainer | **递归搜索**子容器 |
| 2a. 递归结果非 None | `search`：立即返回（首个匹配）；`search_all`：extend 到结果列表 |
| 3. 值非容器 | 用 `compiled_pattern.match(key)` 匹配**键名** |
| 3a. 匹配成功 | `search`：返回值；`search_all`：append 到结果列表 |
| 4. 异常处理 | `except Exception: pass`（静默忽略，继续下一个） |
| 5. 返回 | `search`：None（无匹配）；`search_all`：列表（可能为空） |

**关键细节**：
- `value.__class__._search(value, ...)` 通过类型调用而非实例调用（防 shadowing）
- `compiled_pattern.match(key)` 使用 `re.match`（匹配键的**开头**，非 `search` 的任意位置）
- `search` 返回**第一个匹配**（深度优先），`search_all` 返回**所有匹配**

测试用例（`test_search.py`，6 个全部通过）：
- `test_search_sanity`：基本搜索，无匹配返回 None
- `test_search_functionality`：Switch 分支中不同路径的搜索
- `test_search_regexp`：正则模式 `abcb[1-4]a`
- `test_search_all_sanity`：search_all 基本行为
- `test_search_all_functionality`：GreedyRange 多元素全部收集
- `test_search_all_regexp`：`ab.*` 匹配多个键

---

## 3. ListContainer 类详细设计

### 3.1 类定义

```python
class ListContainer(list):
    """Generic container like list. Provides pretty-printing.
    Also provides regex searching."""
```

- 继承 `list`，使 `isinstance(lc, list)` 为 `True`
- 无 `__slots__`（原版未定义）

### 3.2 构造函数

继承 `list.__init__`，接受任意可迭代对象：

| 调用形式 | 示例 |
|---------|------|
| 无参数 | `ListContainer()` |
| 可迭代对象 | `ListContainer([1, 2, 3])`、`ListContainer(range(5))` |

### 3.3 `__repr__`

```python
@recursion_lock()
def __repr__(self):
    return "ListContainer(%s)" % (list.__repr__(self),)
```

- 格式：`ListContainer([elem1, elem2, ...])`（Python repr 格式，单行）
- 内部用 `list.__repr__(self)` 而非 `repr(list(self))`，保留 Python 原生 list repr 格式
- 递归保护：自引用时，内部元素显示为 `<recursion detected>`

### 3.4 `__str__`

```python
@recursion_lock()
def __str__(self):
    indentation = "\n    "
    text = ["ListContainer: "]
    for k in self:
        text.append(indentation)
        lines = value_to_string(k).split("\n")
        text.append(indentation.join(lines))
    return "".join(text)
```

- 格式：多行缩进，`ListContainer:` 开头
- 每个元素一行（缩进 4 空格）
- 值通过 `value_to_string()` 格式化
- 多行值按缩进对齐
- 递归保护：自引用时输出 `<recursion detected>`

测试用例（`test_containers_list.py`，1 个测试文件含 2 个断言组）：
- `ListContainer(range(5))` → `"ListContainer: \n    0\n    1\n    2\n    3\n    4"`
- `repr(ListContainer(range(5)))` → `"ListContainer([0, 1, 2, 3, 4])"`
- 自引用（`lc.append(lc)`）→ 最后一个元素显示 `<recursion detected>`

### 3.5 list 协议方法

ListContainer 继承 `list`，以下方法**直接从 list 继承**：

| 方法 | 行为 |
|------|------|
| `__getitem__` / `__setitem__` / `__delitem__` | 下标访问 |
| `__iter__` | 迭代元素 |
| `__len__` | 元素数量 |
| `__contains__` | `in` 运算符 |
| `append(x)` / `extend(it)` / `insert(i, x)` | 添加元素 |
| `pop(i)` / `remove(x)` | 删除元素 |
| `sort()` / `reverse()` | 排序/反转 |
| `__add__` / `__mul__` / `__iadd__` | 运算符 |
| `index(x)` / `count(x)` | 查找 |
| `clear()` | 清空 |

### 3.6 search / search_all

```python
def _search(self, compiled_pattern, search_all):
    items = []
    for item in self:
        try:
            ret = item.__class__._search(item, compiled_pattern, search_all)
        except Exception:
            continue
        if ret is not None:
            if search_all:
                items.extend(ret)
            else:
                return ret
    if search_all:
        return items
    else:
        return None

def search(self, pattern):
    compiled_pattern = re.compile(pattern)
    return self._search(compiled_pattern, False)

def search_all(self, pattern):
    compiled_pattern = re.compile(pattern)
    return self._search(compiled_pattern, True)
```

**搜索算法**（与 Container 的区别）：
- 遍历的是**元素**而非键值对
- 只递归搜索 Container/ListContainer 类型的元素（非容器元素调用 `._search` 会抛 `AttributeError`，被 `except` 静默跳过）
- 不会在非容器元素上做键名匹配（list 没有键）

| 步骤 | 行为 |
|------|------|
| 1. 遍历 | 按顺序遍历 list 元素 |
| 2. 元素是 Container/ListContainer | 递归搜索 |
| 2a. 递归结果非 None | `search`：返回；`search_all`：extend |
| 3. 元素非容器 | `item._search(...)` 抛 `AttributeError` → `except: continue` |
| 4. 异常处理 | `except Exception: continue`（静默跳过） |
| 5. 返回 | `search`：None；`search_all`：列表 |

---

## 4. 全局打印标志

### 4.1 模块级全局变量

```python
globalPrintFullStrings = False
globalPrintFalseFlags = False
globalPrintPrivateEntries = False
```

| 标志 | 默认值 | 作用域 | 影响 |
|------|--------|-------|------|
| `globalPrintFullStrings` | `False` | `__str__` 中 `value_to_string` | True 时 bytes/str 不截断，完整显示 |
| `globalPrintFalseFlags` | `False` | `__str__` 中 flagsenum 过滤 | True 时显示值为 False 的 flag 键 |
| `globalPrintPrivateEntries` | `False` | `__str__` 中私有键过滤 | True 时显示 `_` 前缀键 |

**注意**：这三个标志**只影响 `__str__`**，**不影响 `__repr__`**。`__repr__` 始终跳过私有键、不截断、不过滤 flags。

### 4.2 setter 函数

```python
def setGlobalPrintFullStrings(enabled=False):
    global globalPrintFullStrings
    globalPrintFullStrings = enabled

def setGlobalPrintFalseFlags(enabled=False):
    global globalPrintFalseFlags
    globalPrintFalseFlags = enabled

def setGlobalPrintPrivateEntries(enabled=False):
    global globalPrintPrivateEntries
    globalPrintPrivateEntries = enabled
```

- 每个函数接受一个 `enabled` 参数（默认 `False`），修改对应全局变量
- 测试中 `setGlobalPrintFullStrings()` 无参数调用 → 重置为 `False`（默认值）

### 4.3 使用约定

这三个全局变量是**进程级**的。测试中典型的使用模式：

```python
setGlobalPrintFullStrings(True)      # 开启完整字符串显示
assert str(c) == "..."               # 验证输出
setGlobalPrintFullStrings()          # 无参数 → 重置为 False
```

对应测试：`test_fullstrings`、`test_falseflags`、`test_privateentries`

---

## 5. 辅助函数

### 5.1 recursion_lock 装饰器

```python
def recursion_lock(retval="<recursion detected>", lock_name="__recursion_lock__"):
    """Used internally."""
    def decorator(func):
        def wrapper(self, *args, **kw):
            if getattr(self, lock_name, False):
                return retval
            setattr(self, lock_name, True)
            try:
                return func(self, *args, **kw)
            finally:
                delattr(self, lock_name)
        wrapper.__name__ = func.__name__
        return wrapper
    return decorator
```

**机制**：
1. 进入被装饰方法前，检查 `self.__recursion_lock__`（通过 `getattr` 安全获取，默认 False）
2. 若已锁定 → 返回 `retval`（默认 `"<recursion detected>"`）
3. 否则设置 `self.__recursion_lock__ = True`，执行方法
4. `finally` 中删除 `self.__recursion_lock__`（`delattr`，确保不残留）

**应用于**：
- `Container.__repr__`
- `Container.__str__`
- `ListContainer.__repr__`
- `ListContainer.__str__`

**重要细节**：
- `__recursion_lock__` 通过 `setattr`/`getattr`/`delattr` 操作。在 Container 中，由于 `__dict__ = self`，这些操作作用于 dict 数据本身——`__recursion_lock__` 会作为 dict 的键临时存在
- `__repr__` 和 `__str__` 遍历时跳过 `_` 前缀键，所以 `__recursion_lock__` 不会出现在输出中
- `finally` 中的 `delattr` 确保 `__recursion_lock__` 不残留（测试 `test_regression_recursionlock` 验证此行为）
- `wrapper.__name__ = func.__name__` 保持原函数名（用于 `__repr__` 检查）

**自引用测试**（`test_str_repr_recursive`）：
```python
c = Container(a=1, b=2)
c.c = c  # 自引用
assert str(c) == "Container: \n    a = 1\n    b = 2\n    c = <recursion detected>"
```

### 5.2 value_to_string

```python
def value_to_string(value):
    if value.__class__.__name__ == "EnumInteger":
        return "(enum) (unknown) %s" % (value,)

    if value.__class__.__name__ == "EnumIntegerString":
        return "(enum) %s %s" % (value, value.intvalue,)

    if value.__class__.__name__ in ["HexDisplayedBytes", "HexDumpDisplayedBytes"]:
        return str(value)

    if isinstance(value, bytes):
        printingcap = 16
        if len(value) <= printingcap or globalPrintFullStrings:
            return "%s (total %d)" % (repr(value), len(value))
        return "%s... (truncated, total %d)" % (repr(value[:printingcap]), len(value))

    if isinstance(value, str):
        printingcap = 32
        if len(value) <= printingcap or globalPrintFullStrings:
            return "%s (total %d)" % (repr(value), len(value))
        return "%s... (truncated, total %d)" % (repr(value[:printingcap]), len(value))

    return str(value)
```

**格式化规则**：

| 值类型 | 类名检查方式 | 输出格式 | 截断阈值 |
|--------|-------------|---------|---------|
| EnumInteger | `__class__.__name__` | `(enum) (unknown) <value>` | 不截断 |
| EnumIntegerString | `__class__.__name__` | `(enum) <string> <intvalue>` | 不截断 |
| HexDisplayedBytes | `__class__.__name__` | `str(value)` | 不截断 |
| HexDumpDisplayedBytes | `__class__.__name__` | `str(value)` | 不截断 |
| bytes | `isinstance` | `b'...' (total N)` 或 `b'...'... (truncated, total N)` | **16 字节** |
| str | `isinstance` | `'...' (total N)` 或 `'...'... (truncated, total N)` | **32 字符** |
| 其他（int/float/bool/Container/ListContainer/...） | fallback | `str(value)` | 不截断 |

**截断行为**（受 `globalPrintFullStrings` 控制）：
- `globalPrintFullStrings = False`（默认）：超过阈值时截断，加 `... (truncated, total N)` 后缀
- `globalPrintFullStrings = True`：完整显示，仅加 ` (total N)` 后缀
- 等于或短于阈值：完整显示 + ` (total N)` 后缀

**类名检查 vs isinstance**：
- EnumInteger/EnumIntegerString/HexDisplayedBytes 使用 `__class__.__name__` 检查，因为这些是 PyO3 包装的类型（10.7/10.5 实现），Python 侧无法 import 进行 isinstance 检查
- bytes/str 使用 `isinstance`，因为是 Python 内置类型

**嵌套处理**：Container/ListContainer 类型的值走最后的 `return str(value)` 分支，触发它们自身的 `__str__`（递归，受 recursion_lock 保护）。

---

## 6. Python API 映射表

### 6.1 Container 类

| Python 原版 API | 本模块实现 | 说明 |
|----------------|----------|------|
| `class Container(dict)` | 同 | 继承 dict |
| `__slots__ = ('__dict__', '__recursion_lock__')` | 同 | 限制实例属性 |
| `__init__(*args, **kwargs)` | 同 | `super().__init__` + `self.__dict__ = self` |
| `copy()` | 同 | `self.__class__(self)` |
| `__copy__()` | 同 | 委托 `copy()` |
| `__deepcopy__(_, /)` | 同 | 委托 `copy()`（浅拷贝语义） |
| `__dir__()` | 同 | dict 键 + 类型属性 + dict 方法 |
| `__eq__(other)` | 同 | 顺序无关，跳过 `_` 前缀，numpy 特殊处理 |
| `__ne__(other)` | 同 | `not self == other` |
| `__repr__()` | 同 | `@recursion_lock`，跳过私有键，单行格式 |
| `__str__()` | 同 | `@recursion_lock`，多行格式，受全局标志控制 |
| `_search(compiled_pattern, search_all)` | 同 | 内部搜索逻辑 |
| `search(pattern)` | 同 | 正则搜索，返回首个匹配 |
| `search_all(pattern)` | 同 | 正则搜索，返回所有匹配 |
| `__getstate__()` | 同 | pickle → dict |
| `__setstate__(state)` | 同 | pickle ← dict |
| 以下继承自 dict，不重新实现 | | |
| `__getitem__` / `__setitem__` / `__delitem__` | dict | 下标访问 |
| `__contains__` | dict | `in` |
| `__iter__` / `__len__` / `__bool__` | dict | 迭代/长度/布尔 |
| `keys()` / `values()` / `items()` | dict | 保持插入顺序 |
| `get(key, default)` | dict | 安全获取 |
| `pop(key)` / `popitem()` | dict | 弹出 |
| `update(*args, **kwargs)` | dict | 批量更新 |
| `clear()` / `setdefault(key, default)` | dict | 清空/默认值 |

### 6.2 ListContainer 类

| Python 原版 API | 本模块实现 | 说明 |
|----------------|----------|------|
| `class ListContainer(list)` | 同 | 继承 list |
| `__repr__()` | 同 | `@recursion_lock`，`ListContainer([...])` |
| `__str__()` | 同 | `@recursion_lock`，多行格式 |
| `_search(compiled_pattern, search_all)` | 同 | 内部搜索逻辑 |
| `search(pattern)` | 同 | 正则搜索 |
| `search_all(pattern)` | 同 | 正则搜索 |
| 以下继承自 list，不重新实现 | | |
| `__getitem__` / `__setitem__` / `__delitem__` | list | 下标访问 |
| `__iter__` / `__len__` / `__contains__` | list | 迭代/长度/包含 |
| `append(x)` / `extend(it)` / `insert(i, x)` | list | 添加 |
| `pop(i)` / `remove(x)` | list | 删除 |
| `sort()` / `reverse()` | list | 排序 |
| `index(x)` / `count(x)` | list | 查找 |
| `clear()` | list | 清空 |

### 6.3 模块级函数和变量

| Python 原版 API | 本模块实现 | 说明 |
|----------------|----------|------|
| `globalPrintFullStrings` | 同 | 全局变量，默认 False |
| `globalPrintFalseFlags` | 同 | 全局变量，默认 False |
| `globalPrintPrivateEntries` | 同 | 全局变量，默认 False |
| `setGlobalPrintFullStrings(enabled=False)` | 同 | setter |
| `setGlobalPrintFalseFlags(enabled=False)` | 同 | setter |
| `setGlobalPrintPrivateEntries(enabled=False)` | 同 | setter |
| `recursion_lock(retval, lock_name)` | 同 | 装饰器工厂 |
| `value_to_string(value)` | 同 | 值格式化函数 |

### 6.4 导出接口

在 `construct_rust/lib/__init__.py` 中 re-export：

```python
from .containers import (
    Container,
    ListContainer,
    globalPrintFullStrings,
    globalPrintFalseFlags,
    globalPrintPrivateEntries,
    setGlobalPrintFullStrings,
    setGlobalPrintFalseFlags,
    setGlobalPrintPrivateEntries,
    recursion_lock,
    value_to_string,
)
```

在 `construct_rust/lib/__init__.py` 的 `__all__` 中包含以上名称，使 `from construct.lib import *` 能正确导入。

---

## 7. 边界条件清单

### 7.1 构造边界

| # | 场景 | 输入 | 预期行为 | 测试 |
|---|------|------|---------|------|
| 1 | 空构造 | `Container()` | len=0, `== Container()`, `== Container({})`, `== Container([])` | `test_ctor_empty` |
| 2 | 从 Container 复制 | `Container(c)` | 深度复制键值对，保持顺序 | `test_ctor_chained` |
| 3 | 从 dict 构造 | `Container({"a":1})` | 保持插入顺序 | `test_ctor_dict` |
| 4 | 从元组列表构造 | `Container([("a",1)])` | 保持列表顺序 | `test_ctor_seqoftuples` |
| 5 | keyword args | `Container(a=1, b=2)` | 保持参数顺序 | `test_ctor_orderedkw` |
| 6 | ListContainer 空构造 | `ListContainer()` | len=0 | — |
| 7 | ListContainer 从可迭代 | `ListContainer(range(5))` | 保持迭代顺序 | `test_str` |

### 7.2 访问边界

| # | 场景 | 输入 | 预期行为 | 测试 |
|---|------|------|---------|------|
| 8 | 读不存在的属性 | `c.unknownkey` | `AttributeError` | `test_getitem` |
| 9 | 读不存在的键 | `c["unknownkey"]` | `KeyError` | `test_getitem` |
| 10 | 删除后访问属性 | `del c.a; c.a` | `AttributeError` | `test_delitem` |
| 11 | 删除后访问键 | `del c["a"]; c["a"]` | `KeyError` | `test_delitem` |
| 12 | method shadowing | `c['update'] = 42; c.update` | 返回 42（非 dict.update 方法） | `test_method_shadowing_1` |
| 13 | 多方法 shadowing | items/keys/search/update/copy 同时 shadow | copy/deepcopy 正常工作；显式 `Container.search(c, ...)` 绕过 | `test_method_shadowing_2` |

### 7.3 相等性边界

| # | 场景 | 输入 | 预期行为 | 测试 |
|---|------|------|---------|------|
| 14 | 自等 | `c == c` | `True` | `test_eq_issue_818` |
| 15 | 键数不同 | 2 键 vs 3 键 | `False`（双向） | `test_eq_issue_818` |
| 16 | Container vs dict | `Container(a=1) == {"a":1}` | `True`（双向） | `test_eq_issue_818` |
| 17 | `_` 前缀键不参与比较 | 含 `_io` 的解析结果 vs 无 `_io` 的 dict | `True` | `test_eq_issue_818` |
| 18 | numpy 数组比较 | `Container(arr=ndarray)` | `numpy.array_equal`（跳过） | `test_eq_numpy`（跳过） |
| 19 | 不等 | `c != d` | `not (c == d)` | `test_ne_issue_818` |

### 7.4 打印格式边界

| # | 场景 | 输入 | 预期行为 | 测试 |
|---|------|------|---------|------|
| 20 | 空容器 str | `str(Container())` | `"Container: "` | `test_str_repr_empty` |
| 21 | 空容器 repr | `repr(Container())` | `"Container()"` | `test_str_repr_empty` |
| 22 | repr eval 往返 | `eval(repr(c)) == c` | `True` | `test_str_repr_empty/repr/nested` |
| 23 | 嵌套空 Container | `Container(c=Container())` | str 中显示 `c = Container: ` | `test_str_repr_nested` |
| 24 | 自引用 | `c.c = c` | str/repr 中显示 `<recursion detected>` | `test_str_repr_recursive` |
| 25 | bytes 短（≤16） | `b"1234567890"`（10 字节） | `b'1234567890' (total 10)` | `test_fullstrings` |
| 26 | bytes 长（>16） | 40 字节 | `b'1234567890123456'... (truncated, total 40)` | `test_fullstrings` |
| 27 | str 短（≤32） | 10 字符 | `'1234567890' (total 10)` | `test_fullstrings` |
| 28 | str 长（>32） | 40 字符 | `'12345678901234567890123456789012'... (truncated, total 40)` | `test_fullstrings` |
| 29 | 全字符串模式 | `setGlobalPrintFullStrings(True)` | 不截断 | `test_fullstrings` |
| 30 | false flags | FlagsEnum 解析，`globalPrintFalseFlags=True` | 显示值为 False 的键 | `test_falseflags` |
| 31 | false flags 关闭 | `globalPrintFalseFlags=False` | 隐藏值为 False 的键 | `test_falseflags` |
| 32 | 私有键显示 | `globalPrintPrivateEntries=True` | str 显示 `_` 前缀键 | `test_privateentries` |
| 33 | 私有键隐藏 | `globalPrintPrivateEntries=False` | str 不显示 `_` 前缀键（repr 始终不显示） | `test_privateentries` |
| 34 | recursion_lock 不残留 | `str(c); repr(c); bool(c)` | `False`（lock 已清理） | `test_regression_recursionlock` |
| 35 | ListContainer 自引用 | `lc.append(lc)` | 末尾元素显示 `<recursion detected>` | `test_str`（list） |

### 7.5 搜索边界

| # | 场景 | 输入 | 预期行为 | 测试 |
|---|------|------|---------|------|
| 36 | 无匹配 search | `obj.search("bb")` | `None` | `test_search_sanity` |
| 37 | 嵌套匹配 search | `obj.search("abcb")` | 首个匹配值 | `test_search_sanity` |
| 38 | 正则匹配 search | `obj.search('abcb[1-4]a')` | 首个匹配值 | `test_search_regexp` |
| 39 | 无匹配 search_all | `obj.search_all("bb")` | `[]`（空列表） | `test_search_all_sanity` |
| 40 | 多匹配 search_all | GreedyRange 多元素 | 所有匹配值的列表 | `test_search_all_functionality` |
| 41 | 正则多匹配 | `obj.search_all("ab.*")` | 所有匹配（含嵌套） | `test_search_all_regexp` |
| 42 | 搜索异常静默 | 元素非容器，无 `_search` 方法 | `except: pass`，不影响其他元素 | — |

### 7.6 copy/pickle 边界

| # | 场景 | 输入 | 预期行为 | 测试 |
|---|------|------|---------|------|
| 43 | copy 浅拷贝 | `d = c.copy()` | `c == d`, `c is not d` | `test_copy_method` |
| 44 | copy.copy | `copy.copy(c)` | 同上 | `test_copy` |
| 45 | copy.deepcopy | `deepcopy(c); d.a = 2` | `c != d`（int 不可变，新对象） | `test_deepcopy` |
| 46 | pickle 空容器 | `pickle.loads(pickle.dumps(Container()))` | `== Container()` | `test_pickling` |
| 47 | pickle 嵌套 | 嵌套 Container | 往返一致 | `test_pickling` |

### 7.7 全局标志边界

| # | 场景 | 输入 | 预期行为 | 测试 |
|---|------|------|---------|------|
| 48 | 无参数 setter | `setGlobalPrintFullStrings()` | 重置为默认值 `False` | `test_fullstrings` |
| 49 | 标志只影响 str | 任何标志 | `__repr__` 不受影响 | `test_falseflags/privateentries` |

---

## 8. 与其他模块的交互

### 8.1 依赖（本模块需要）

- **Python 标准库**：`re`（正则搜索）、`sys`（原版 import，本模块不直接需要但保持兼容）
- **construct_rust.lib.py3compat**：原版 `containers.py:1` 有 `from construct.lib.py3compat import *`。本模块需确认 py3compat 导出的内容是否被使用（检查后确认：containers.py 未实际使用 py3compat 的任何导出符号，仅为兼容性 import。本模块可省略此 import 或保留以保持一致性）

### 8.2 被依赖（下游模块需要本模块）

| 模块 | 使用方式 |
|------|---------|
| **10.2 Value 映射** | `value_to_py`：`Value::Container` → 构造纯 Python Container；`Value::List` → 构造纯 Python ListContainer。`py_to_value`：检测 `isinstance(obj, dict)` → `Value::Container`（Container 是 dict 子类，自动覆盖）。`context_to_py_container`：构造 Container 作为 Python 表达式求值的上下文 |
| **10.4 表达式系统** | `context_to_py_container` 的输出作为 `this.a.b.c` 表达式路径访问的目标对象（Container 的属性访问能力是表达式求值的基础） |
| **10.5-10.8 各构造器** | parse 结果返回 Container/ListContainer；Struct 的 `subcons` 解析结果是 Container；Array/GreedyRange 的结果是 ListContainer |
| **10.7 适配器** | FlagsEnum 解析结果设置 `_flagsenum` 标记（`__str__` 中检查此标记进行 false flag 过滤）；Enum 解析结果可能包含 EnumInteger（`value_to_string` 中通过类名识别） |
| **10.10 测试** | `test_containers_dict.py`（36）、`test_containers_list.py`（1）、`test_search.py`（6）直接测试本模块。所有 `test_core.py` 中涉及解析结果的断言间接依赖本模块 |

### 8.3 接口契约（对下游的承诺）

1. **`isinstance(container, dict)` → `True`**：Container 是 dict 的直接子类，可被任何期望 dict 的代码接受
2. **`isinstance(listcontainer, list)` → `True`**：ListContainer 是 list 的直接子类
3. **属性 = 下标**：`c.a` 与 `c["a"]` 访问同一份数据，包括写入和删除
4. **插入顺序保持**：`keys()`/`values()`/`items()`/`__iter__` 按插入顺序返回（Python 3.7+ dict 保证）
5. **`str()`/`repr()` 格式**：与原版完全一致的输出格式（测试逐字节验证）
6. **`search`/`search_all` 行为**：与原版完全一致的搜索语义（深度优先、正则匹配键名）
7. **`__eq__` 行为**：顺序无关、跳过私有键、与普通 dict 可互比

### 8.4 类引用缓存策略（供 10.2 实现）

`value_to_py` 在解析热路径上频繁构造 Container/ListContainer。建议缓存类引用避免重复 import：

```rust
// construct-py/src/value.rs 或 lib.rs
use std::sync::OnceLock;

static CONTAINER_CLS: OnceLock<PyObject> = OnceLock::new();
static LISTCONTAINER_CLS: OnceLock<PyObject> = OnceLock::new();

fn get_container_cls(py: Python<'_>) -> PyResult<&PyObject> {
    CONTAINER_CLS.get_or_try_init(|| {
        py.import("construct_rust.lib.containers")?
            .getattr("Container")?
            .into()
    })
}
```

**注意**：`OnceLock` 持有的 `PyObject` 需要在解释器关闭时释放。PyO3 0.22 中 `PyObject`（即 `Py<PyAny>`）在解释器关闭时自动处理。如果使用多解释器（子解释器），需改用 thread-local 或每解释器缓存。Phase 10 单解释器场景下 `OnceLock` 足够。

---

## 9. 需 PM 确认的事项

### 9.1 设计方案偏离：纯 Python 替代 PyO3（关键决策）

**背景**：总纲设计决策表规定 Container 用 "`#[pyclass]` 继承 dict"。经详细技术分析（见 §1），原版 Container 的核心特性——`__dict__ = self` trick 导致的 method shadowing——在 PyO3 中难以可靠实现。需覆盖 `__getattribute__`（`tp_getattro`）并手动实现属性查找链的 fallback 逻辑，复杂度高且有递归风险。

**推荐**：改用纯 Python 实现（放入 `construct_rust/lib/containers.py`），与已定为纯 Python 的 `binary.py`/`py3compat.py`/`hex.py`/`bitstream.py` 保持一致。

**影响**：
- 10.2 的 `value_to_py` 实现需从"构造 `Py<PyContainer>`"改为"import Python Container 类并构造"（已在 §1.5 提供适配代码）
- 10.2 的 `py_to_value` 无需修改（Container 是 dict 子类，已有 `isinstance(obj, dict)` 逻辑覆盖）
- 无其他下游影响

**请 PM 裁定**：是否接受纯 Python 方案？

### 9.2 测试预期跳过确认

总纲预期 `test_containers_dict.py` 36 用例中跳过 ~2 个。经逐用例分析：

| 跳过用例 | 理由 |
|---------|------|
| `test_eq_numpy` | 依赖 `numpy`，需 `import numpy` |

**建议**：仅跳过 `test_eq_numpy`（1 个），其余 35 个用例全部通过。`test_pickling` 不跳过（纯 Python Container 天然支持 pickle）。`test_method_shadowing_1/2` 不跳过（纯 Python 方案完美支持 method shadowing）。

**请 PM 确认**：跳过列表是否准确？

### 9.3 py3compat import 兼容性

原版 `containers.py:1` 有 `from construct.lib.py3compat import *`，但实际未使用 py3compat 导出的任何符号（仅为历史兼容性 import）。本模块设计为省略此 import（减少不必要的依赖链）。

**请 PM 确认**：是否保留 `from construct.lib.py3compat import *` 以保持与原版文件的一致性？（保留不影响功能，但不必要的 import 可能在 py3compat 尚未实现时导致 ImportError）
