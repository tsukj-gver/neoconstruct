---
id: ANALYSIS-Construct-Expr
status: active
phase: meta
depends_on: []
supersedes: []
superseded_by: []
last_updated: 2026-07-27
---

# Python construct 2.10.70 表达式系统分析

> 本文档是对 Python 库 [construct](https://github.com/construct/construct) 2.10.70 版本表达式系统的独立分析报告。
> 面向希望理解 construct 表达式系统（`this` / `obj_` / `len_` 等）设计与实现的读者。
> 所有源码引用基于 `construct/construct/` 目录下的实际代码，标注 `文件名:行号`。

---

## 1. 概述

construct 是一个声明式、对称的二进制数据解析与构建库。一个核心需求是：**字段之间可以互相引用**。例如，先读取一个 `length` 字段，然后用它决定后续 `data` 字段读取多少字节：

```python
Struct(
    "length" / Int8ub,
    "data" / Bytes(this.length),     # data 的长度取决于 length 字段的值
)
```

这里的 `this.length` 就是一个**表达式**。它不是在定义时立即求值的，而是在 parse 或 build 执行到该字段时，根据当前的上下文（context）动态求值。

construct 的表达式系统由 `construct/construct/expr.py`（256 行）实现，核心设计可以概括为一句话：

> **用 Python 运算符重载构建一棵延迟求值的表达式树，在 parse/build 时用 context 字典对这棵树求值。**

整个系统由以下几个角色组成：

| 角色 | 定义位置 | 作用 |
|------|---------|------|
| `this` | `expr.py:248` | 根表达式对象，代表"当前 context" |
| `Path` | `expr.py:165` | 属性/索引访问链节点 |
| `BinExpr` | `expr.py:146` | 二元运算表达式节点 |
| `UniExpr` | `expr.py:129` | 一元运算表达式节点 |
| `FuncPath` | `expr.py:223` | 函数包装表达式（`len_` / `sum_` 等） |
| `obj_` / `list_` | `expr.py:249-250` | 特殊上下文符号（RepeatUntil 中使用） |
| `len_` / `sum_` / `min_` / `max_` / `abs_` | `expr.py:252-256` | 内置函数表达式 |
| `evaluate()` | `core.py:314` | **统一的求值入口** |

---

## 2. this 对象：延迟求值的属性链

### 2.1 Path 类

`this` 是一个 `Path` 实例：

```python
# expr.py:248
this = Path("this")
```

`Path` 类的定义（`expr.py:165-197`）：

```python
class Path(ExprMixin):

    def __init__(self, name, field=None, parent=None):
        self.__name = name
        self.__field = field
        self.__parent = parent
```

每个 `Path` 对象持有三个私有属性：
- `__name`：仅用于 `__repr__` 显示，对求值无影响
- `__field`：当前节点要访问的字段名（`None` 表示这是根节点）
- `__parent`：父级 `Path`（`None` 表示这是根节点）

`this` 本身是一个**根节点**：`Path("this")`，其 `field=None`、`parent=None`。

### 2.2 属性访问与索引访问

`Path` 重载了 `__getattr__` 和 `__getitem__`，使每次访问都返回一个**新的 Path 节点**，而非立即取值：

```python
# expr.py:193-197
def __getattr__(self, name):
    return Path(self.__name, name, self)

def __getitem__(self, name):
    return Path(self.__name, name, self)
```

因此：

```python
this              # Path("this")            — 根节点，field=None, parent=None
this.a            # Path("this","a",this)   — field="a", parent=this
this.a.b          # Path("this","b",this.a) — field="b", parent=this.a
this["key"]       # Path("this","key",this) — 与属性访问等价
this.a[0]         # Path("this",0,this.a)   — 混合使用
```

这就构成了一条**链表**，每个节点指向其父节点。`this.a.b` 的结构是：

```
Path("this","b")  ──parent──>  Path("this","a")  ──parent──>  Path("this")  ──parent──> None
```

关键点：**`__getattr__` 和 `__getitem__` 是完全等价的**，两者都创建 `Path(self.__name, name, self)`。这意味着 `this.a` 和 `this["a"]` 产生完全相同的表达式对象。

> ⚠️ 注意：由于 `__getattr__` 拦截所有属性访问，`Path` 内部属性使用了**双下划线名称改写**（`self.__name` → `self._Path__name`），以避免与用户字段名冲突。这是 Python 名称改写（name mangling）机制在此处的关键应用。

### 2.3 运算符重载（BinExpr）

`Path` 继承自 `ExprMixin`（`expr.py:32`），该 mixin 重载了几乎所有 Python 运算符。每个运算符都返回一个 `BinExpr` 或 `UniExpr` 节点，而非立即计算：

**二元运算符**（`expr.py:34-84`）：

```python
class ExprMixin(object):
    def __add__(self, other):
        return BinExpr(operator.add, self, other)
    def __sub__(self, other):
        return BinExpr(operator.sub, self, other)
    def __mul__(self, other):
        return BinExpr(operator.mul, self, other)
    # ... floordiv, truediv, mod, pow, xor, lshift, rshift, and, or ...
```

**反向运算符**（`expr.py:60-84`）：当左操作数不是表达式（如 `2 * this.x`）时，Python 调用右操作数的 `__rmul__`：

```python
    def __rmul__(self, other):
        return BinExpr(operator.mul, other, self)
```

**比较运算符**（`expr.py:96-107`）：

```python
    def __gt__(self, other):
        return BinExpr(operator.gt, self, other)
    def __eq__(self, other):
        return BinExpr(operator.eq, self, other)
    # ... ge, lt, le, ne ...
```

> ⚠️ **重要副作用**：由于重载了 `__eq__`，`Path` / `BinExpr` 对象**不能用作普通 dict 的键**或参与 `==` 的常规比较。例如 `this.x == this.x` 返回一个 `BinExpr` 对象而非 `True`。construct 通过 `evaluate()` 显式求值来规避这个问题。

**一元运算符**（`expr.py:86-92`）：

```python
    def __neg__(self):
        return UniExpr(operator.neg, self)
    def __pos__(self):
        return UniExpr(operator.pos, self)
    def __invert__(self):
        return UniExpr(operator.not_, self)
```

`BinExpr` 本身也继承 `ExprMixin`，因此**表达式可以无限嵌套**：

```python
(this.a + this.b) * 2
```

展开过程：
1. `this.a + this.b` → `BinExpr(operator.add, this.a, this.b)`
2. `BinExpr(...) * 2` → `BinExpr(operator.mul, BinExpr(add, this.a, this.b), 2)`

这是一棵**二叉表达式树**：

```
            BinExpr(*)
           /         \
      BinExpr(+)      2
      /      \
   this.a   this.b
```

`BinExpr` 的定义（`expr.py:146-162`）：

```python
class BinExpr(ExprMixin):
    def __init__(self, op, lhs, rhs):
        self.op = op
        self.lhs = lhs
        self.rhs = rhs

    def __call__(self, obj, *args):
        lhs = self.lhs(obj) if callable(self.lhs) else self.lhs
        rhs = self.rhs(obj) if callable(self.rhs) else self.rhs
        return self.op(lhs, rhs)
```

注意 `__call__` 中的 `if callable(self.lhs)` 检查：运算符的某一侧可能是普通常量（如 `* 2` 中的 `2`），`2` 不是 callable，所以直接使用；如果某一侧是 `Path` / `BinExpr`（callable），则先递归求值。这使得常量和表达式可以混合使用。

### 2.4 求值机制

#### 核心函数：evaluate()

所有表达式的求值都经过同一个入口函数（`core.py:314-315`）：

```python
def evaluate(param, context):
    return param(context) if callable(param) else param
```

这行代码是整个表达式系统的**枢纽**。逻辑极其简单：

- 如果 `param` 是 callable（`Path` / `BinExpr` / `UniExpr` / `FuncPath` / lambda），就用 `context` 调用它
- 如果 `param` 不是 callable（int / str / bytes 等常量），直接返回它

这意味着**表达式对象和 lambda 的处理方式完全统一**——它们都是 callable。

#### Path 的求值

`Path.__call__` 的实现（`expr.py:184-188`）：

```python
def __call__(self, obj, *args):
    if self.__parent is None:
        return obj
    else:
        return self.__parent(obj)[self.__field]
```

求值是**递归的**：
- 如果当前节点是根（`parent is None`），返回传入的 `obj`（即 context 本身）
- 否则，先递归求值父节点 `self.__parent(obj)`，然后在结果上用 `[self.__field]` 取字段

以 `this.a.b` 为例，调用 `(this.a.b)(context)`：
1. `this.a.b` 的 parent 是 `this.a`，field 是 `"b"`
2. 先求 `this.a(context)`：parent 是 `this`，field 是 `"a"`
3. 先求 `this(context)`：parent 是 None，返回 `context`
4. 回到步骤 2：`context["a"]`
5. 回到步骤 1：`context["a"]["b"]`

最终等价于 Python 的 `context["a"]["b"]`。

由于 construct 的 `context` 是 `Container`（一个同时支持属性访问和键访问的 dict 子类，见 `containers.py:84`），`context["a"]` 和 `context.a` 是等价的。`Path` 统一使用 `[]` 访问。

#### 求值时机

表达式**不在定义时求值**，而是在 parse / build / sizeof 执行到使用该表达式的构造器时才求值。以 `Array` 为例（`core.py:2525-2536`）：

```python
def _parse(self, stream, context, path):
    count = evaluate(self.count, context)   # ← 此刻才求值
    if not 0 <= count:
        raise RangeError(...)
    obj = ListContainer()
    for i in range(count):
        context._index = i
        e = self.subcon._parsereport(stream, context, path)
        if not discard:
            obj.append(e)
    return obj
```

`self.count` 可能是 `5`（int）、`this.n`（Path）或 `lambda ctx: ctx["n"] + 1`（lambda）。无论哪种，`evaluate()` 都能正确处理。

#### Container 的角色

context 使用的是 `Container` 类（`containers.py:84-222`）。它的关键设计：

```python
class Container(dict):
    __slots__ = ('__dict__', '__recursion_lock__')

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.__dict__ = self    # ← 让属性访问等价于键访问
```

`self.__dict__ = self` 这一行让 `Container` 同时支持 `ctx.field` 和 `ctx["field"]` 两种访问方式。这是 `this.field` 表达式能在 context 上工作的基础——因为 `Path.__call__` 使用 `[]` 访问，而 Container 是 dict 所以支持 `[]`。

---

## 3. obj_ / len_ / 其他上下文符号

construct 在 `expr.py` 底部定义了多个全局符号（`expr.py:248-256`）：

```python
this  = Path("this")
obj_  = Path("obj_")
list_ = Path2("list_")

len_ = FuncPath(len)
sum_ = FuncPath(sum)
min_ = FuncPath(min)
max_ = FuncPath(max)
abs_ = FuncPath(abs)
```

### 3.1 this

`this` 是最主要的上下文符号，代表**当前 context 字典**。`this.field` 访问当前作用域中已解析/已构建的字段值。

### 3.2 obj_

`obj_` 与 `this` 是**同类型**对象（都是 `Path`），但语义不同：

- `this` 在常规表达式中使用，代表当前 context
- `obj_` 专用于 **RepeatUntil 的编译路径**（`core.py:2706-2734`），代表"当前正在处理的元素"

在 `RepeatUntil._emitparse` 生成的代码中（`core.py:2706-2719`）：

```python
def _emitparse(self, code):
    fname = f"parse_repeatuntil_{code.allocateId()}"
    block = f"""
        def {fname}(io, this):
            list_ = ListContainer()
            while True:
                obj_ = {self.subcon._compileparse(code)}
                if not ({self.discard}):
                    list_.append(obj_)
                if ({self.predicate}):       # ← predicate 中可用 obj_ 和 list_
                    return list_
    """
    code.append(block)
    return f"{fname}(io, this)"
```

在编译后的代码中，`obj_` 和 `list_` 是局部变量名。predicate 表达式中的 `obj_` 引用会通过 `repr()` 序列化后直接嵌入生成的 Python 代码。

注意：在**解释执行路径**（`RepeatUntil._parse`，`core.py:2670-2682`）中，predicate 是一个接收三个参数的 lambda，而非使用 `obj_` 表达式：

```python
def _parse(self, stream, context, path):
    predicate = self.predicate
    if not callable(predicate):
        predicate = lambda _1,_2,_3: predicate
    obj = ListContainer()
    for i in itertools.count():
        context._index = i
        e = self.subcon._parsereport(stream, context, path)
        if not discard:
            obj.append(e)
        if predicate(e, obj, context):   # ← 三参数 lambda: (元素, 列表, context)
            return obj
```

### 3.3 list_

`list_` 是 `Path2` 实例（`expr.py:200-220`），用于 RepeatUntil 的编译路径中代表"到目前为止的列表"。

`Path2` 与 `Path` 的区别在于 `__call__` 的签名（`expr.py:213-217`）：

```python
class Path2(ExprMixin):
    def __call__(self, *args):
        if self.__index is None:
            return args[1]          # ← 返回第二个位置参数（build 方向的 obj）
        else:
            return self.__parent(*args)[self.__index]
```

`Path2.__call__` 接收 `*args`（不定长参数），根节点返回 `args[1]`。这是因为在 build 方向，编译函数签名为 `buildall(obj, io, this)`，`obj` 是第二个位置参数（索引 1）。

### 3.4 len_ / sum_ / min_ / max_ / abs_

这些是 `FuncPath` 实例（`expr.py:223-245`），包装了 Python 内置函数：

```python
class FuncPath(ExprMixin):
    def __init__(self, func, operand=None):
        self.__func = func
        self.__operand = operand

    def __call__(self, operand, *args):
        if self.__operand is None:
            # 尚未绑定操作数：如果传入的是 callable，返回绑定了操作数的新 FuncPath
            return FuncPath(self.__func, operand) if callable(operand) else operand
        else:
            # 已绑定操作数：对操作数求值后应用函数
            return self.__func(self.__operand(operand) if callable(self.__operand) else self.__operand)
```

使用方式：

```python
len_(this.payload.data)        # 先返回 FuncPath(len, this.payload.data)
                               # 求值时：len(this.payload.data(context))
```

实际应用（来自 `Check` 的文档，`core.py:3095-3096`）：

```python
Check(lambda ctx: len(ctx.payload.data) == ctx.payload_len)
Check(len_(this.payload.data) == this.payload_len)   # 等价的 this 写法
```

`len_(this.payload.data)` 的求值过程：
1. `len_` = `FuncPath(len)`，operand=None
2. `len_(this.payload.data)` = `FuncPath.__call__(this.payload.data)` → 因 operand 为 None 且 `this.payload.data` 是 callable，返回 `FuncPath(len, this.payload.data)`
3. `FuncPath(len, this.payload.data).__call__(context)` → `len(this.payload.data(context))`

然后 `== this.payload_len` 触发 `ExprMixin.__eq__`，生成 `BinExpr(operator.eq, FuncPath(len, ...), this.payload_len)`。

---

## 4. 表达式在构造器中的使用模式

### 4.1 parse 方向

在 parse 方向，context 是**逐步构建**的。以 `Struct._parse`（`core.py:2232-2245`）为例：

```python
def _parse(self, stream, context, path):
    obj = Container()
    obj._io = stream
    # 创建新的嵌套 context，旧 context 保存在 "_" 键
    context = Container(_ = context, _params = context._params, _root = None,
                        _parsing = context._parsing, _building = context._building,
                        _sizing = context._sizing, _subcons = self._subcons,
                        _io = stream, _index = context.get("_index", None))
    context._root = context._.get("_root", context)
    for sc in self.subcons:
        try:
            subobj = sc._parsereport(stream, context, path)
            if sc.name:
                obj[sc.name] = subobj
                context[sc.name] = subobj    # ← 每解析一个字段就写入 context
        except StopFieldError:
            break
    return obj
```

关键机制：
1. **context nesting**：进入 Struct 时，创建新 context，旧 context 保存在 `_` 键下
2. **逐步更新**：每解析完一个命名字段，立即 `context[sc.name] = subobj`
3. **后续字段可见前面的字段**：因为 context 是同一个对象，前面的字段值在后续字段的 `evaluate()` 时已经存在

示例：
```python
Struct(
    "width" / Byte,
    "height" / Byte,
    "total" / Computed(this.width * this.height),  # 此时 context 已有 width 和 height
)
```

解析 `"total"` 字段时，`context["width"]` 和 `context["height"]` 已经存在，`this.width * this.height` 能正确求值。

### 4.2 build 方向

在 build 方向，context 从**用户传入的对象**构建。以 `Struct._build`（`core.py:2247-2268`）为例：

```python
def _build(self, obj, stream, context, path):
    if obj is None:
        obj = Container()
    context = Container(_ = context, ...)
    context._root = context._.get("_root", context)
    context.update(obj)          # ← 将用户传入的 dict 内容灌入 context
    for sc in self.subcons:
        try:
            if sc.flagbuildnone:
                subobj = obj.get(sc.name, None)
            else:
                subobj = obj[sc.name]   # raises KeyError

            if sc.name:
                context[sc.name] = subobj    # ← 写入 context

            buildret = sc._build(subobj, stream, context, path)
            if sc.name:
                context[sc.name] = buildret  # ← 用 build 返回值更新 context
        except StopFieldError:
            break
    return context
```

关键区别：
1. 用户传入的 `obj`（一个 dict）在循环前通过 `context.update(obj)` 灌入 context
2. 每个字段 build 后，用 build 返回值（`buildret`）更新 context——因为某些构造器的 build 返回值可能与输入不同（如 `Const` 会返回常量值，`Default` 可能返回默认值）
3. 后续字段的 `this.xxx` 取的是**最新更新后的值**

**parse 和 build 的统一性**：无论哪个方向，`this.field` 求值时执行的都是 `context["field"]`。区别仅在于 context 里存的是什么：
- parse 时：存的是**已解析的值**
- build 时：存的是**用户传入的值**（或 build 返回值）

这就是 `this` 表达式能在两个方向上工作的原因——它只是"从 context 取值"，不关心值的来源。

### 4.3 典型构造器分析

#### Array（`core.py:2493-2567`）

`Array(count, subcon)` — `count` 可以是 int、`this.n` 或 lambda。

```python
def _parse(self, stream, context, path):
    count = evaluate(self.count, context)    # 求值 count
    if not 0 <= count:
        raise RangeError("invalid count %s" % (count,), path=path)
    obj = ListContainer()
    for i in range(count):
        context._index = i                   # 设置当前索引
        e = self.subcon._parsereport(stream, context, path)
        if not discard:
            obj.append(e)
    return obj
```

- parse：先求 count，再循环 count 次解析元素。每次循环设置 `context._index`，子构造器可通过 `this._index` 访问。
- build：先求 count，校验 `len(obj) == count`，再循环 build。

#### Bytes（`core.py:924-989`）

`Bytes(length)` — `length` 可以是 int 或表达式。

```python
def _parse(self, stream, context, path):
    length = self.length(context) if callable(self.length) else self.length
    return stream_read(stream, length, path)
```

注意：`Bytes` 没有使用 `evaluate()` 函数，而是**内联了相同的逻辑**（`if callable(...) else ...`）。这在构造器中是常见模式，`evaluate()` 只是一个便利函数。

#### Switch（`core.py:4002-4058`）

`Switch(keyfunc, cases, default)` — 根据 `keyfunc` 的值从 `cases` 字典中选择子构造器。

```python
def _parse(self, stream, context, path):
    keyfunc = evaluate(self.keyfunc, context)    # 求值 keyfunc（如 this.type）
    sc = self.cases.get(keyfunc, self.default)   # 从 cases 字典中选择
    return sc._parsereport(stream, context, path)
```

典型用法：
```python
Switch(this.n, { 1: Int8ub, 2: Int16ub, 4: Int32ub })
```

#### IfThenElse（`core.py:3944-3999`）

`IfThenElse(condfunc, thensubcon, elsesubcon)` — 条件分支。

```python
def _parse(self, stream, context, path):
    condfunc = evaluate(self.condfunc, context)   # 求值条件
    sc = self.thensubcon if condfunc else self.elsesubcon
    return sc._parsereport(stream, context, path)
```

典型用法：
```python
IfThenElse(this.x > 0, VarInt, Byte)
```

#### Computed（`core.py:2878-2930`）

`Computed(func)` — 从 context 计算值，不读写流。

```python
def _parse(self, stream, context, path):
    return self.func(context) if callable(self.func) else self.func

def _build(self, obj, stream, context, path):
    return self.func(context) if callable(self.func) else self.func
```

parse 和 build 完全对称——都返回 `func(context)` 的求值结果。典型用法：

```python
"total" / Computed(this.width * this.height)
```

#### Prefixed（`core.py:4862-4913`）

`Prefixed(lengthfield, subcon)` — 这里的 `lengthfield` 是一个**构造器**（不是表达式），用于读取长度前缀。

```python
def _parse(self, stream, context, path):
    length = self.lengthfield._parsereport(stream, context, path)  # 用构造器读长度
    if self.includelength:
        length -= self.lengthfield._sizeof(context, path)
    substream = BytesIOWithOffsets.from_reading(stream, length, path)
    return self.subcon._parsereport(substream, context, path)
```

注意：`Prefixed` 不使用 `evaluate()`，因为 `lengthfield` 是构造器而非表达式。这与 `Bytes(this.length)` 不同——后者 `length` 是表达式。

#### Pointer（`core.py:4400-4478`）

`Pointer(offset, subcon)` — `offset` 可以是 int 或表达式。

```python
def _pointer_seek(self, stream, context, path):
    offset = evaluate(self.offset, context)    # 求值偏移量
    stream = evaluate(self.stream, context) or stream
    fallback = stream_tell(stream, path)
    if self.relativeOffset:
        stream_seek(stream, offset, 1, path)
    else:
        stream_seek(stream, offset, 2 if offset < 0 else 0, path)
    return fallback
```

ELF 格式中大量使用：
```python
"sh_name" / Pointer(this._.strtab_data_offset + this.sh_name_offset, CString("utf-8"))
```

#### Check / Assert（`core.py:3081-3115`）

`Check(func)` — 在 parse 和 build 时检查条件。

```python
def _parse(self, stream, context, path):
    passed = evaluate(self.func, context)
    if not passed:
        raise CheckError("check failed during parsing", path=path)
```

两种等价写法：
```python
Check(lambda ctx: len(ctx.payload.data) == ctx.payload_len)
Check(len_(this.payload.data) == this.payload_len)
```

---

## 5. lambda 替代方案

construct 允许在所有接受表达式的地方使用 Python lambda 作为替代：

```python
# 以下两种写法完全等价
Array(this.count, Int32ub)
Array(lambda ctx: ctx["count"], Int32ub)
```

### 5.1 统一处理机制

`evaluate()` 函数是统一的入口（`core.py:314-315`）：

```python
def evaluate(param, context):
    return param(context) if callable(param) else param
```

`Path` / `BinExpr` / `FuncPath` 对象是 callable（实现了 `__call__`），lambda 也是 callable，`int` / `str` 不是 callable。因此 `evaluate()` 对三者一视同仁。

### 5.2 两者的区别

| 方面 | this 表达式 | lambda |
|------|------------|--------|
| **可序列化** | ✅ `repr()` 产生可执行代码 | ❌ `repr()` 产生 `<function ...>` |
| **运算符组合** | ✅ `this.a + this.b` 返回表达式树 | ❌ 必须写 `lambda ctx: ctx["a"] + ctx["b"]` |
| **任意逻辑** | ❌ 仅限运算符重载和函数包装 | ✅ 可写任意 Python 逻辑 |
| **访问 _subcons** | ❌ 不支持 | ✅ `lambda ctx: ctx._subcons.field.sizeof()` |
| **类型安全** | 较弱（`this.x` 不报错即使字段不存在） | 较弱（同样运行时才发现） |

**可序列化性**是关键区别。construct 的 `compile()` 方法（`core.py:511-575`）会将构造器编译成 Python 源代码。在这个过程中，表达式对象通过 `repr()` 转成代码字符串：

- `this.length` 的 `repr()` 是 `this['length']`（见 `Path.__repr__`，`expr.py:172-176`）
- `this.a + this.b` 的 `repr()` 是 `(this['a'] + this['b'])`（见 `BinExpr.__repr__`，`expr.py:153-154`）
- lambda 的 `repr()` 是 `<function <lambda> at 0x...>`（不可执行）

因此，**编译路径（compile）不支持 lambda**，只支持 this 表达式或常量。construct 的文档中多次强调这一点（如 `Struct` 的 docstring，`core.py:2176`）：

> Note that you need to use a lambda (`this` expression is not supported). Also note that compiler does not support this feature.

这句话有两层含义：
1. 某些功能（如通过 context 访问 `_subcons`）**只能用 lambda**，this 表达式不支持
2. 但这些 lambda 功能**不能被编译**（compile 不支持）

### 5.3 Bytes 中的内联模式

部分构造器（如 `Bytes`）没有使用 `evaluate()` 函数，而是直接内联了相同的逻辑：

```python
# Bytes._parse (core.py:965-967)
def _parse(self, stream, context, path):
    length = self.length(context) if callable(self.length) else self.length
    return stream_read(stream, length, path)
```

这与 `evaluate(self.length, context)` 完全等价，只是省去了函数调用开销。

---

## 6. 嵌套 context 与作用域

### 6.1 context nesting 机制

construct 的 `Struct` / `Sequence` / `Union` / `LazyStruct` 都会进行 **context nesting**——在进入时创建新的 context，旧 context 保存在 `_` 键下。

以 `Struct._parse`（`core.py:2235-2236`）为例：

```python
context = Container(_ = context, _params = context._params, _root = None,
                    _parsing = context._parsing, _building = context._building,
                    _sizing = context._sizing, _subcons = self._subcons,
                    _io = stream, _index = context.get("_index", None))
context._root = context._.get("_root", context)
```

新 context 包含以下特殊键：

| 键 | 含义 |
|----|------|
| `_` | 父级 context（外层作用域） |
| `_params` | 最顶层用户传入的参数（`parse(**contextkw)` 中的 kw） |
| `_root` | 最外层 context（根作用域） |
| `_parsing` / `_building` / `_sizing` | 当前操作模式标志 |
| `_subcons` | 当前 Struct 的子构造器字典 |
| `_io` | 当前流对象 |
| `_index` | 当前在 Array / GreedyRange 中的索引 |

### 6.2 this._ — 访问父级 context

`this._` 通过 `Path.__getattr__("_")` 创建 `Path("this", "_", this)`。求值时：

```
this._(context) = context["_"] = 父级 context
```

因此 `this._.field` 取的是**父级 context 中的字段**。

ELF 格式中的实际用法（`gallery/elf.py:86`）：

```python
"sh_name" / Pointer(
    this._.strtab_data_offset + this.sh_name_offset,
    CString("utf-8"),
)
```

这里 `this._.strtab_data_offset` 访问的是外层 Struct 的 `strtab_data_offset` 字段，而 `this.sh_name_offset` 访问的是当前 Struct 的 `sh_name_offset` 字段。

### 6.3 多层嵌套

可以连续使用 `this._._.field` 访问祖父级 context：

```
this._       → 父级 context
this._._     → 祖父级 context
this._._._   → 曾祖父级 context
```

### 6.4 _root — 访问根 context

`this._root` 指向最外层的 context（根作用域）。其设置逻辑（`core.py:2236`）：

```python
context._root = context._.get("_root", context)
```

- 如果父级 context 有 `_root`，则继承父级的 `_root`（保持指向同一个根）
- 如果父级没有 `_root`（即当前就是根），则 `_root` 指向自己

这使得无论嵌套多深，`this._root` 始终指向最外层 context。

### 6.5 Array 循环中的 this._

在 `Array` / `GreedyRange` / `RepeatUntil` 中，循环体内设置 `context._index`：

```python
# Array._parse (core.py:2531-2535)
for i in range(count):
    context._index = i
    e = self.subcon._parsereport(stream, context, path)
```

子构造器可通过 `this._index` 访问当前循环索引。construct 还提供了专门的 `Index` 构造器（`core.py:2934-2971`）：

```python
>>> d = Array(3, Index)
>>> d.parse(b"")
[0, 1, 2]

>>> d = Array(3, Computed(this._index+1))
>>> d.parse(b"")
[1, 2, 3]
```

如果子构造器本身是一个 `Struct`（会进行 context nesting），则需要通过 `this._._index` 访问外层的索引：

```python
>>> d = Array(3, Struct("i" / Computed(this._._index+1)))
>>> d.parse(b"")
[Container(i=1), Container(i=2), Container(i=3)]
```

因为进入 `Struct` 后，新的 context 的 `_` 指向 Array 的 context，`this._index` 在新 context 中不存在（只有 `this._._index` 才能取到）。

### 6.6 _subcons — 访问内联构造器

context 中的 `_subcons` 键保存当前 Struct 的所有命名子构造器。这使得 lambda 可以访问内联定义的构造器（`this` 表达式不支持此功能）：

```python
d = Struct(
    "count" / Byte,
    "data" / Bytes(lambda this: this.count - this._subcons.count.sizeof()),
)
```

这里 `this._subcons.count` 取到的是 `count` 字段的**构造器对象**（`Byte`），然后调用 `.sizeof()` 计算其大小。这在定义"扣除长度字段自身大小后剩余的数据长度"时很有用。

### 6.7 context 继承链总结

```
根 context (parse_stream 创建)
  └─ _params = 自己
  └─ _root = 自己
  └─ Struct._parse → 新 context
       └─ _ = 根 context
       └─ _params = 根 context._params
       └─ _root = 根 context._root (即根 context)
       └─ field1, field2, ... (逐步添加)
       └─ Array._parse → 共享当前 context，设置 _index
            └─ Struct._parse → 新 context
                 └─ _ = Array 的 context
                 └─ _root = 根 context
                 └─ ...
```

这是一条**链式作用域链**，通过 `_` 键实现向上查找，通过 `_root` 实现直达根节点。

---

## 7. 设计哲学与特点

### 7.1 为什么选择"延迟求值的属性链"而非"字符串表达式"

许多模板引擎（如 Jinja2 的 `"{{ field }}"`、Django 模板的 `{% %}`）选择用**字符串**来表示动态表达式。construct 没有这么做，而是选择了 Python 原生的运算符重载。原因分析：

**1. 避免 DSL（领域特定语言）的解析开销**

字符串表达式需要一个词法分析器 + 语法分析器 + 求值器。construct 的 `Path` / `BinExpr` 直接利用 Python 的 AST 和运算符分派，不需要任何字符串解析。

**2. IDE 友好性**

`this.field.subfield` 在 IDE 中有自动补全（虽然是假的，但至少不报语法错误）、有类型提示、可以重构。字符串 `"{{ field.subfield }}"` 没有任何 IDE 支持。

**3. 错误更早暴露**

字符串表达式中的拼写错误（如 `"{{ feild }}"`）在运行时求值时才暴露。而 `this.feild` 虽然 construct 本身不做静态检查，但 Python 的属性访问机制至少保证了语法正确性。

**4. 可序列化性**

`Path.__repr__` 产生 `this['field']`，`BinExpr.__repr__` 产生 `(this['a'] + this['b'])`。这些 repr 字符串本身就是合法的 Python 代码，使得 construct 的 `compile()` 可以直接将表达式序列化为源代码。字符串表达式无法做到这一点（除非额外实现转译器）。

**代价**：重载 `__eq__` 导致表达式对象无法参与正常比较和哈希；类型安全性完全缺失（`this.x` 返回的是 `Path` 对象，不是 `x` 的类型）。

### 7.2 与 pydantic / dataclass 的 field 表达式对比

Python 生态中其他库处理"字段间引用"的方式：

| 库 | 机制 | 求值时机 | 示例 |
|----|------|---------|------|
| **construct** | 运算符重载表达式树 | parse/build 时 | `Bytes(this.length)` |
| **pydantic** | `Field(default_factory=...)` 或 validator | 验证时 | `Field(default_factory=lambda: uuid4())` |
| **dataclasses** | `field(default=...)` / `__post_init__` | 初始化后 | `field(init=False)` |
| **attrs** | `attrib(factory=...)` | 初始化时 | `attrib(factory=list)` |

关键区别：

- **construct 的表达式引用的是"同一结构内其他字段的值"**，而不是外部值。`this.length` 的语义是"当前正在解析的 Struct 中的 length 字段"。
- **pydantic / dataclass 的 factory 是独立的**，不直接引用同级的其他字段（pydantic V2 有 `model_validator` 可以做，但不是一等公民）。
- **construct 的表达式在两个方向（parse/build）上自动工作**，因为它们只是"从 context 取值"，而 pydantic 等只在实例化方向工作。

### 7.3 类型安全性问题

construct 的表达式系统在类型上**完全动态**：

- `this.length` 的类型是 `Path`，不是 `int`
- `this.length * 2` 的类型是 `BinExpr`，不是 `int`
- 求值前无法知道 `context["length"]` 的实际类型

这意味着：
```python
# 以下代码在定义时不会报错，但运行时可能出错
Bytes(this.length)       # 如果 length 解析出来是字符串呢？
Array(this.count, Byte)  # 如果 count 是负数呢？
```

construct 的处理方式是**延迟失败**——运行时 `evaluate()` 返回值类型不匹配时，由具体构造器做检查（如 `Array` 检查 `0 <= count`）。这与 Python 的动态类型哲学一致。

对于静态类型系统语言（如 Rust、Haskell），这种设计不可直接移植。需要选择：
1. 用宏在编译期生成类型安全的表达式
2. 用 `enum Value` 的动态类型模拟（牺牲性能和类型安全）
3. 限制表达式能力，只支持已知的模式（如 length field 引用）

### 7.4 对称性设计

construct 表达式系统的一个优雅之处是**parse/build 对称**。同一个 `this.length` 表达式：

- **parse 时**：从 context 取已解析的 `length` 值（刚从字节流读取）
- **build 时**：从 context 取用户传入的 `length` 值（从用户 dict 灌入）

表达式本身**不区分方向**，它只是 `context["length"]`。方向的区分由 context 的内容自然决定。这避免了维护两套表达式系统（一套用于 parse、一套用于 build）的复杂性。

### 7.5 编译路径的表达式处理

construct 的 `compile()` 方法（`core.py:511-575`）将解释执行的构造器树编译成 Python 源代码。表达式的处理方式：

编译后的函数签名（`core.py:548-551`）：
```python
def parseall(io, this):
    return {self._compileparse(code)}
def buildall(obj, io, this):
    return {self._compilebuild(code)}
```

注意参数名就叫 `this`。在编译后的代码中，`this` 是一个普通的 Python 变量（context dict），不是 `Path` 对象。

当构造器的 `_emitparse` 方法生成代码时，表达式通过 `repr()` 嵌入：

```python
# Array._emitparse (core.py:2560-2561)
def _emitparse(self, code):
    return f"ListContainer(({self.subcon._compileparse(code)}) for i in range({self.count}))"
```

如果 `self.count` 是 `this.n`，则 `repr(this.n)` = `"this['n']"`，生成的代码是：
```python
ListContainer((...) for i in range(this['n']))
```

在编译后的代码中，`this` 是 context dict，`this['n']` 直接取键值——比解释执行时创建 `Path` 对象再递归求值要快得多。这就是编译路径的性能优势来源。

### 7.6 性能特征

表达式系统的性能特点：

1. **解释执行路径**：每次 `evaluate()` 都会创建临时对象（如果表达式是内联的）或调用 `Path.__call__` 递归。对于深层嵌套的 `this.a.b.c.d`，需要 4 次递归调用。

2. **编译路径**：表达式被序列化为直接的 Python 代码 `this['a']['b']['c']['d']`，没有递归开销。

3. **lambda 的开销**：与 `Path` 对象的 `__call__` 调用开销相当，都是一次 Python 函数调用。但 lambda 无法被编译，所以编译后的路径不支持 lambda。

---

## 8. 完整使用示例（from gallery/）

### 8.1 ELF 格式解析器（`gallery/elf.py`）

ELF 格式是表达式系统的**集大成者**。以下是关键片段的分析：

#### 示例 1：条件选择字节序和位宽（`gallery/elf.py:367-379`）

```python
elf = Struct(
    "identifier" / identifier,
    "body" / IfThenElse(
        this.identifier.encoding == "LSB",       # ① 比较表达式
        IfThenElse(
            this.identifier.elfclass == "ELFCLASS64",  # ② 嵌套比较
            body(Int16ul, Int32ul, Int64ul, is64bit=True),
            body(Int16ul, Int32ul, Int64ul, is64bit=False),
        ),
        IfThenElse(
            this.identifier.elfclass == "ELFCLASS64",
            body(Int16ub, Int32ub, Int64ub, is64bit=True),
            body(Int16ub, Int32ub, Int64ub, is64bit=False),
        ),
    ),
)
```

分析：
- `this.identifier.encoding` — 两级属性访问：先取 `context["identifier"]`（一个 Container），再取 `["encoding"]`
- `== "LSB"` — 触发 `ExprMixin.__eq__`，生成 `BinExpr(operator.eq, this.identifier.encoding, "LSB")`
- 求值时：`context["identifier"]["encoding"] == "LSB"` → `True` 或 `False`
- 嵌套 `IfThenElse` 实现两路条件（字节序 × 位宽 = 4 种组合）

#### 示例 2：跨层级引用（`gallery/elf.py:86`）

```python
def section_header(ELFInt32, ELFInt64, is64bit=True):
    return Struct(
        "sh_name_offset" / ELFInt32,
        "sh_name" / Pointer(
            this._.strtab_data_offset + this.sh_name_offset,  # ③ 跨层级 + 算术
            CString("utf-8"),
        ),
        ...
    )
```

分析：
- `this._.strtab_data_offset` — 通过 `this._` 访问**父级 context**（外层 Struct）的 `strtab_data_offset` 字段
- `this.sh_name_offset` — 当前 Struct 的字段
- 两者相加（`BinExpr(operator.add, ...)`），结果作为 `Pointer` 的偏移量
- 求值时：`context["_"]["strtab_data_offset"] + context["sh_name_offset"]`

#### 示例 3：数组数量引用（`gallery/elf.py:358-359`）

```python
"program_table" / Pointer(this.ph_offset, p_header[this.ph_count]),
"sections" / Pointer(this.sh_offset, s_header[this.sh_count]),
```

分析：
- `p_header[this.ph_count]` — `p_header` 是 `Renamed(Array(...))` 构造器，`[this.ph_count]` 是 `/` 运算符的语法糖等价物（`Byte[5]` 等价于 `Array(5, Byte)`）
- `this.ph_count` 是 `Path` 表达式，在 parse 时从 context 取已解析的 `ph_count` 值作为数组长度

#### 示例 4：复杂偏移计算（`gallery/elf.py:351-357`）

```python
"strtab_data_offset" / Pointer(
    this.sh_offset
    + this.strtab_section_index * this.sh_entry_size
    + (24 if is64bit else 16),
    ELFInt32,
),
```

分析：
- `this.sh_offset + this.strtab_section_index * this.sh_entry_size + 24`
- 运算符优先级由 Python 自动处理：先 `*`（`BinExpr(mul, ...)`），再 `+`（`BinExpr(add, BinExpr(add, ...), 24)`）
- 求值时：`context["sh_offset"] + context["strtab_section_index"] * context["sh_entry_size"] + 24`

### 8.2 PE/COFF 格式解析器（`gallery/pe32coff.py`）

#### 示例 5：条件字段（`gallery/pe32coff.py:75`）

```python
plusfield = IfThenElse(this.signature == "PE32plus", Int64ul, Int32ul)
```

这是顶层表达式（不在 Struct 内），在 parse 时 `this.signature` 从 context 取值。

### 8.3 Computed + len_ 组合

```python
from construct import *

d = Struct(
    "width" / Byte,
    "height" / Byte,
    "total" / Computed(this.width * this.height),         # 算术表达式
    "pixels" / Array(this.width * this.height, Byte),     # 表达式作为数组长度
    "checksum" / Computed(sum_(this.pixels)),             # sum_ 函数表达式
)

# parse
result = d.parse(b"\x02\x03" + bytes(range(6)))
# Container(width=2, height=3, total=6, pixels=ListContainer([0,1,2,3,4,5]), checksum=15)
```

分析：
- `this.width * this.height` — `BinExpr(operator.mul, this.width, this.height)`
- `sum_(this.pixels)` — `FuncPath(sum, this.pixels)`，求值时 `sum(context["pixels"])`
- 两个字段引用同一个表达式 `this.width * this.height`，构造了两个独立的 `BinExpr` 对象

### 8.4 完整求值追踪

以以下定义为例，追踪一次 parse 的完整求值过程：

```python
d = Struct(
    "length" / Byte,
    "data" / Bytes(this.length),
    "doubled" / Computed(this.length * 2),
)

d.parse(b"\x05hello")
```

**步骤 1：`Struct._parse` 创建 context**

```python
context = Container(_ = outer_context, _params = outer_context, ...)
```

**步骤 2：解析 `"length"` 字段**

```python
# Byte._parse 读取 1 字节 → 5
subobj = 5
context["length"] = 5    # context 现在有 length=5
```

**步骤 3：解析 `"data"` 字段**

```python
# Bytes._parse 被调用
# Bytes._parse (core.py:965-967):
length = self.length(context) if callable(self.length) else self.length
# self.length = this.length = Path("this","length",this)
# this.length(context):
#   → this.length 的 parent 是 this，field 是 "length"
#   → this(context) → context (根节点返回 obj 本身)
#   → context["length"] → 5
length = 5
return stream_read(stream, 5, path)    # 读取 5 字节 → b"hello"

context["data"] = b"hello"
```

**步骤 4：解析 `"doubled"` 字段**

```python
# Computed._parse (core.py:2917-2918):
return self.func(context) if callable(self.func) else self.func
# self.func = this.length * 2 = BinExpr(operator.mul, this.length, 2)
# BinExpr.__call__(context):
#   lhs = this.length(context) = context["length"] = 5
#   rhs = 2 (not callable, 直接使用)
#   return operator.mul(5, 2) = 10

context["doubled"] = 10
```

**最终结果**：
```python
Container(length=5, data=b"hello", doubled=10)
```

---

## 附录：表达式系统类继承关系

```
ExprMixin (expr.py:32)           ← 所有运算符重载的 mixin
├── UniExpr (expr.py:129)        ← 一元运算表达式
├── BinExpr (expr.py:146)        ← 二元运算表达式
├── Path (expr.py:165)           ← 属性/索引访问链
├── Path2 (expr.py:200)          ← RepeatUntil 专用（obj_/list_）
└── FuncPath (expr.py:223)       ← 函数包装表达式（len_/sum_/...）

全局实例 (expr.py:248-256):
  this  = Path("this")
  obj_  = Path("obj_")
  list_ = Path2("list_")
  len_  = FuncPath(len)
  sum_  = FuncPath(sum)
  min_  = FuncPath(min)
  max_  = FuncPath(max)
  abs_  = FuncPath(abs)
```

## 附录：使用 evaluate() 的构造器清单

以下构造器在 `_parse` / `_build` / `_sizeof` 中调用 `evaluate()`（`core.py:314`）来求值表达式参数：

| 构造器 | 求值的字段 | 典型表达式 |
|--------|-----------|-----------|
| `Array` | `count` | `this.n` |
| `Bytes` | `length` (内联) | `this.length` |
| `BytesInteger` | `length` (内联) | `this.length` |
| `Computed` | `func` (内联) | `this.x * 2` |
| `Check` / `Assert` | `func` | `len_(this.data) == this.n` |
| `Const` | `value` (内联) | `this.value` |
| `IfThenElse` | `condfunc` | `this.flag` |
| `Switch` | `keyfunc` | `this.type` |
| `Pointer` | `offset`, `stream` | `this.base + this.off` |
| `Padded` | `length` | `this.size` |
| `Aligned` | `modulus` | `this.align` |
| `FixedSized` | `length` | `this.size` |
| `Rebuffered` | `amount`, `group` | `this.chunk` |
| `PrefixedArray` | 等 | — |
| `Union` | `parsefrom` | `this.which` |
| `RestreamData` | `datafunc` | `this.raw` |
| `Padding` | `padfunc` (内联) | `this.padlen` |
| `Lazy` | 等 | — |
| 加密相关 | `cipher`, `nonce`, `associated_data` | — |
