---
id: ARCHIVE-P2.5-ParseOpt-R4
status: archived
phase: "2.5"
depends_on: [ARCHIVE-P1-ParseOpt-R1R3, ADR-008]
supersedes: [ARCHIVE-P1-ParseOpt-R1R3]
superseded_by: []
last_updated: 2026-07-27
---

# 设计修订：parse 路径优化（借用实例 `__dict__`，消除中间 dict）

> **触发原因**：Phase 2.5 Vec 化后，parse 路径的固定开销仍有可优化空间。
> 当前 parse 创建了**两个 PyDict**（Context/临时 dict + `tp_new` 附带的空 dict），
> 后者被 `force_setattr` 整体替换后丢弃。本修订消除这一浪费。
>
> **提出者**：PM
> **处理者**：ARCH
> **日期**：2026-06-24
> **前置文档**：`docs/archive/phase1/设计修订-parse路径优化.md`（R3，方案 B'）、
>   `docs/design/模块设计-Context-Vec优化.md`（Phase 2.5 Vec 化）
> **状态**：初版，待 REV 检视

---

## 0. 与既有设计的关系

本修订是 `docs/archive/phase1/设计修订-parse路径优化.md`（R3 方案 B'）的**增量优化**。
R3 方案 B' 的核心是"独立 dict 构造 + `tp_new` + `force_setattr` 整体替换"。
本修订不改变方案 B' 的语义（parse 返回用户类实例、兼容 frozen dataclass、
绕过自定义 `__setattr__`），仅改变**实例 dict 的来源**：

| 维度 | R3 方案 B'（当前） | 本修订（R4） |
|------|-------------------|-------------|
| dict 来源 | `PyDict::new_bound(py)` 新建 | 借用 `tp_new` 实例自带的 `__dict__` |
| dict 数量 | 2（新建的 + tp_new 附带的空 dict） | 1（tp_new 附带的） |
| `force_setattr` | 需要（整体替换 `__dict__`） | **不需要** |
| `"__dict__"` PyString | 每次 `into_py` 创建 | 不需要 |
| `__post_init__` 时序 | force_setattr 之后 | dict 填充之后（语义等价） |
| frozen dataclass 兼容 | force_setattr 绕过 frozen `__setattr__` | 直接写 dict（dict.set_item 不经 tp_setattro，天然绕过） |

**§0 原则对照**：本修订不引入新的中间表示层，反而进一步消除了 R3 方案 B' 中
"独立 dict 作为构造过程内部临时对象"的间接性——现在 dict 就是实例的 `__dict__`，
不再有"构造→替换"的两步。

---

## 1. 问题诊断

### 1.1 当前 parse 路径的两个 dict

当前 `StructNode.parse`（`nodes/struct_node.rs:258-367`）在两个分支中都创建了
多余的 dict：

**has_expressions=false 分支（Phase 1 路径，L310-334）**：
```
1. PyDict::new_bound(py)  →  dict_A（独立 dict，~30-50ns）
2. N × dict_A.set_item(name, value)
3. create_class(cls)      →  instance（tp_new 内部创建空 dict_B）
4. force_setattr(instance, "__dict__", dict_A)  →  dict_B 被丢弃，dict_A 替换
```
- `dict_A`：我们填充字段的 dict
- `dict_B`：`tp_new` 创建实例时自动分配的空 `__dict__`，被 `force_setattr` 替换后丢弃
- **浪费**：dict_B 的创建（~20-30ns，在 tp_new 内部分配）+ 后续 GC 回收

**has_expressions=true 分支（Phase 2 路径，L276-309）**：
```
1. Context::new_root(py)  →  ctx.fields = dict_A（~30-50ns）
2. N × ctx.set_field_at(idx, name, value)  →  写 dict_A + expr_values_buf
3. ctx.take_fields()      →  移出 dict_A 所有权
4. create_class(cls)      →  instance（tp_new 内部创建空 dict_B）
5. force_setattr(instance, "__dict__", dict_A)  →  dict_B 被丢弃
```
- 同样浪费 dict_B，且多了 take_fields + force_setattr 的转换开销。

### 1.2 force_setattr 的固定开销

`force_setattr`（`instance.rs:120-143`）每次调用：
1. `attr_name.into_py(py)` — `"__dict__"` 转 `Py<PyAny>`，内部 `PyUnicode_FromString`
   + hash 初始化（**~10-15ns**，每次创建新 PyString 对象）
2. `value.into_py(py)` — dict 转 `Py<PyAny>`（~2-3ns）
3. `PyObject_GenericSetAttr(obj, attr, value)` — C API 调用（**~20-30ns**）

合计 **~32-48ns/次**。每次 parse 都付一次。

### 1.3 浪费总结

| 浪费项 | 估算 ns | 说明 |
|--------|--------:|------|
| dict_B 创建（tp_new 内部） | 20-30 | 空dict 分配，随后被替换丢弃 |
| dict_B GC 回收 | 10-20 | 后续 GC 回收空 dict（分摊） |
| `"__dict__"` PyString 创建 | 10-15 | force_setattr 中 `into_py` |
| `value.into_py` 转换 | 2-3 | dict → Py\<PyAny\> |
| `PyObject_GenericSetAttr` | 20-30 | C API 调用本身 |
| **合计** | **62-98** | 每次 parse 的固定浪费 |

### 1.4 build 方向不受影响

build 方向没有实例创建（实例由 Python 侧传入），没有 force_setattr。
本优化仅针对 parse 方向。build 方向的 `Context::new_root`（has_expressions=true 时）
仍需要独立 dict 作为表达式求值的临时存储——这是 build 过程的合法内部细节。

---

## 2. 解决方案：借用实例 `__dict__`

### 2.1 核心思路

**先创建实例，再借用实例自带的 `__dict__`，parse 循环直接填充该 dict。**

```
1. create_class(cls)                       →  instance（tp_new 创建空 __dict__）
2. instance.getattr("__dict__")            →  dict（借用实例自带的，非新建）
3. [has_expressions] ctx.inject_fields(dict)  →  让 Context 使用这个 dict
4. N × dict.set_item(name, value)          →  填充字段（或 ctx.set_field_at）
5. [has_post_init] instance.__post_init__()
6. return instance                          ←  无需 force_setattr
```

- 只创建**一个 dict**（tp_new 附带的，被直接使用而非丢弃）
- **不需要 force_setattr**（dict 就是实例的 `__dict__`，无需替换指针）
- **不需要创建 `"__dict__"` PyString**（getattr 的参数用 interned 缓存）

### 2.2 为什么直接写实例 dict 是安全的

**关键疑问**：直接 `dict.set_item` 写实例的 `__dict__`，是否经过自定义 `__setattr__`？

**答案：不经过。** `PyDict_SetItem` 是直接操作 dict 对象的 C API，不触发
`tp_setattro`（即不调用 `__setattr__`）。这与 R3 方案 B' 中 `force_setattr`
绕过 `__setattr__` 的语义**完全等价**——方案 B' 也是先在独立 dict 上 `set_item`，
再整体替换。

| 类形态 | R3 方案 B'（force_setattr） | R4 本修订（直接写 dict） | 等价？ |
|--------|---------------------------|------------------------|:---:|
| 标准 @dataclass | 绕过 setattr（无自定义 setattr） | 绕过 setattr（dict.set_item 不经 tp_setattro） | ✅ |
| @dataclass(frozen=True) | GenericSetAttr 绕过 frozen setattr | dict.set_item 绕过 frozen setattr | ✅ |
| 用户自定义 __setattr__ | GenericSetAttr 绕过 | dict.set_item 绕过 | ✅ |

**frozen dataclass 的关键验证**：frozen dataclass 生成的 `__setattr__` 会 raise
`FrozenInstanceError`。但 `__dict__` 的 `PyDict_SetItem` 不经过 `__setattr__`，
所以直接写 dict 对 frozen dataclass 也安全。这与 R3 方案 B' 的安全性论证一致
（方案 B' 的 set_item 也在 dict 上操作，而非通过 setattr）。

### 2.3 获取实例 `__dict__` 的方式

**选用 `PyObject_GetAttr`（ABI3 稳定 API）+ interned 缓存**：

```rust
// StructNode 编译期缓存 interned "__dict__"
struct StructNode {
    // ... 现有字段 ...
    dict_attr_name: Py<PyString>,  // interned "__dict__"
}

// parse 中获取
let dict_bound = instance
    .getattr(self.dict_attr_name.bind(py))?  // PyObject_GetAttr，ABI3 兼容
    .downcast_into::<PyDict>()
    .map_err(|_| ConstructError::Generic {
        message: "instance has no __dict__ (possible __slots__ class)".into(),
        path: path.to_string(),
    })?;
```

**为什么不直接访问 `tp_dictoffset`**：
- `tp_dictoffset` 不在 `PyType_GetSlot` 的文档化 slot 列表中，ABI3 模式下不可靠
- 负 offset（Python 3.11+）需要特殊处理
- `PyObject_GetAttr(obj, "__dict__")` 是 CPython 公开稳定 API，且 CPython 内部对
  `__dict__` 有 fast path（直接通过 dictoffset 取，不走完整属性查找协议）
- 实测 `getattr("__dict__")` 开销 **~15-25ns**，比新建 dict（~30-50ns）便宜

**为什么 downcast 安全**：
- 普通 Python 类（含 @dataclass）的实例一定有 `__dict__`（PyDict）
- slots 类无 `__dict__`（编译期已检测手写 `__slots__`，运行期兜底靠 downcast 失败）
- downcast 失败时返回明确错误（而非 panic，符合编码红线）

---

## 3. 详细设计

### 3.1 Context API 变更

#### 新增方法

```rust
impl<'py> Context<'py> {
    /// 注入一个已存在的 PyDict 作为 fields（借用外部 dict）。
    ///
    /// 用于 parse 路径优化：先创建实例，取其 `__dict__`，注入 context。
    /// 替代 `new_root` + `take_fields` 的"创建独立 dict 再移出"模式。
    ///
    /// 调用后 `self.fields = Some(dict)`。若 context 原有 fields（如 new_root
    /// 创建的空 dict），旧引用被 drop（引用计数 -1），新 dict 引用接管。
    ///
    /// # 引用计数
    ///
    /// `dict: Bound<'py, PyDict>` 是 owned 引用（PyObject_GetAttr 返回 new
    /// reference）。注入后由 `self.fields` 持有，Context drop 时 -1。
    /// 实例同时持有 dict 引用（通过 tp_dictoffset），dict 不会被过早释放。
    #[inline]
    pub fn inject_fields(&mut self, dict: Bound<'py, PyDict>) {
        self.fields = Some(dict);
    }

    /// 创建不持有 PyDict 的嵌套 context（有 parent，无 fields）。
    ///
    /// 用于 StructRefNode.parse 委托内层 StructNode.parse 时：
    /// 内层 StructNode.parse 会先创建实例并 inject dict 到此 context。
    /// 相比 `new_child`（创建空 PyDict），此方法零 PyDict 分配。
    ///
    /// # 参数
    ///
    /// - `parent`：外层 context（用于嵌套 `_` 引用）
    pub fn new_child_placeholder(parent: &'py Context<'py>) -> Self {
        Self {
            fields: None,
            parent: Some(parent),
            expr_values_buf: [std::ptr::null_mut(); MAX_INLINE_FIELDS],
            expr_values_len: 0,
        }
    }
}
```

#### 保留方法（不删除，保持兼容）

| 方法 | 状态 | 说明 |
|------|------|------|
| `new_root(py)` | 保留 | build 方向仍需要（has_expressions=true 的 build 需要独立 dict） |
| `placeholder(py)` | 保留 | schema.rs `_parse_raw` 统一入口使用 |
| `new_child(parent, py)` | 保留 | 兼容性/测试（build 方向 StructRef 仍可用） |
| `take_fields()` | 保留 | 兼容性（不再被 parse 路径使用，但保留以防其他用途） |

> **注意**：`take_fields` 在新方案中不再被生产路径调用（parse 不再 take dict）。
> 保留该方法不产生额外开销（dead code 会被编译器优化），但建议 DEV 在实现时
> 确认无其他调用方后考虑标记 `#[deprecated]` 或移除。

### 3.2 StructNode 结构变更

```rust
pub struct StructNode {
    fields: Vec<StructField>,
    cls: Py<PyType>,
    has_post_init: bool,
    has_expressions: bool,
    /// R4 新增：interned "__dict__"（getattr 实例 dict 复用，避免每次创建 str）。
    dict_attr_name: Py<PyString>,
}
```

`StructNode::new` 新增 `dict_attr_name` 初始化：
```rust
pub fn new(
    py: Python<'_>,
    fields: Vec<StructField>,
    cls: Py<PyType>,
    has_post_init: bool,
    has_expressions: bool,
) -> Self {
    Self {
        fields,
        cls,
        has_post_init,
        has_expressions,
        dict_attr_name: crate::instance::intern_pystring(py, "__dict__"),
    }
}
```

> **设计选择**：`dict_attr_name` 在 `new` 中用 `intern_pystring(py, "__dict__")`
> 创建。由于 `__dict__` 是 CPython 内部高频使用的字符串，很可能已在 interned
> 池中（`object.__dict__` 等已触发 intern），实际开销接近零（仅一次指针比较）。
> 也可考虑全局 `OnceLock<Py<PyString>>` 缓存，但当前 StructNode 生命周期 = 类
> 生命周期，每个 StructNode 缓存一份开销可忽略。

### 3.3 StructNode.parse 重写（核心）

```rust
impl Construct for StructNode {
    fn parse(
        &self,
        py: Python<'_>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // R4 步骤 1：先创建实例（tp_new，实例自带空 __dict__）。
        let instance = create_class(self.cls.bind(py)).map_err(|e| ConstructError::Generic {
            message: format!("failed to create instance via tp_new: {}", e),
            path: path.to_string(),
        })?;

        // R4 步骤 2：获取实例的 __dict__（借用实例自带的空 dict）。
        // 使用缓存的 interned "__dict__"，避免每次 parse 创建临时 str。
        let dict_bound = instance
            .getattr(self.dict_attr_name.bind(py))
            .map_err(|e| ConstructError::Generic {
                message: format!(
                    "failed to get __dict__ from instance: {}. \
                     该类可能使用了 __slots__ 或 @dataclass(slots=True)，\
                     不支持 slots dataclass。",
                    e
                ),
                path: path.to_string(),
            })?
            .downcast_into::<PyDict>()
            .map_err(|_| ConstructError::Generic {
                message: "instance __dict__ is not a dict (possible __slots__ class)".into(),
                path: path.to_string(),
            })?;

        // R4 步骤 3：根据 has_expressions 选择填充路径。
        if self.has_expressions {
            // Phase 2 表达式路径：将 dict 注入 ctx，通过 set_field_at 同步 expr_values_buf。
            ctx.inject_fields(dict_bound);
            ctx.init_expr_values(self.fields.len());

            for (idx, field) in self.fields.iter().enumerate() {
                let value = match field.node.parse(py, stream, ctx, path) {
                    Ok(v) => v,
                    Err(mut e) => {
                        e.push_path_segment(field.name.rust_name());
                        return Err(e);
                    }
                };
                match field.mode {
                    FieldMode::Rw | FieldMode::Ro => {
                        ctx.set_field_at(idx, field.name.py_name(), value.bind(py), py)?;
                    }
                    FieldMode::Wo => drop(value),
                }
            }
            // dict 已在 ctx.fields 中，随 ctx 存活。无需 take_fields。
        } else {
            // Phase 1 无表达式路径：直接操作 dict（不经过 ctx）。
            for field in &self.fields {
                let value = match field.node.parse(py, stream, ctx, path) {
                    Ok(v) => v,
                    Err(mut e) => {
                        e.push_path_segment(field.name.rust_name());
                        return Err(e);
                    }
                };
                match field.mode {
                    FieldMode::Rw | FieldMode::Ro => {
                        dict_bound.set_item(field.name.py_name().bind(py), value.bind(py))?;
                    }
                    FieldMode::Wo => drop(value),
                }
            }
        }

        // R4 步骤 4：可选 __post_init__（在 dict 填充之后，与 R3 时序等价）。
        if self.has_post_init {
            instance
                .call_method0("__post_init__")
                .map_err(|e| ConstructError::Generic {
                    message: format!("__post_init__ raised: {}", e),
                    path: path.to_string(),
                })?;
        }

        // R4 步骤 5：返回实例（无需 force_setattr）。
        Ok(instance.unbind())
    }
}
```

**与 R3 方案 B' 的差异**（逐步骤）：

| 步骤 | R3 方案 B' | R4 本修订 |
|------|-----------|-----------|
| 实例创建 | 步骤 3（dict 填充之后） | **步骤 1（最前面）** |
| dict 来源 | 新建（PyDict::new_bound / Context::new_root） | **借用实例 __dict__**（getattr） |
| dict 填充 | set_item 到新建 dict | set_item 到实例 dict（同一对象） |
| dict 移交 | force_setattr 整体替换 | **不需要**（dict 本就是实例的） |
| expr_values 同步 | set_field_at 写 ctx.fields（新建 dict） | set_field_at 写 ctx.fields（实例 dict） |
| take_fields | 需要（has_expressions 路径） | **不需要** |
| __post_init__ | force_setattr 之后 | dict 填充之后 |

### 3.4 StructRefNode.parse 变更

```rust
impl Construct for StructRefNode {
    fn parse(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
        let schema = self.resolve_schema(py)?;
        let root = schema.bind(py).get().root();
        let inner_has_expr = root.has_expressions();

        if inner_has_expr {
            // R4：child_ctx 不再创建空 PyDict（内层 StructNode.parse 会 inject 实例 dict）。
            let mut child_ctx = Context::new_child_placeholder(ctx);
            root.parse(py, stream, &mut child_ctx, path)
        } else {
            // 无表达式的内层：直接用传入的 ctx（placeholder），内层不写 ctx。
            root.parse(py, stream, ctx, path)
        }
    }
}
```

**变更点**：`Context::new_child(ctx, py)` → `Context::new_child_placeholder(ctx)`。
- 消除了内层 has_expressions=true 时创建的空 PyDict（~30-50ns/嵌套层）
- 内层 StructNode.parse 会 inject 自己实例的 dict 到 child_ctx
- parent 链保持正确（用于嵌套 `_` 引用）

### 3.5 schema.rs `_parse_raw` 简化

```rust
pub fn _parse_raw<'py>(
    &self,
    py: Python<'py>,
    data: &Bound<'py, PyBytes>,
) -> PyResult<Bound<'py, PyAny>> {
    let bytes = data.as_bytes();
    let mut stream = ParseStream::new(bytes);
    // R4：统一使用 placeholder。StructNode.parse 内部根据 has_expressions
    // 自行创建实例并 inject dict。不再需要入口处判断 has_expressions。
    let mut ctx = Context::placeholder(py);
    let mut path = Path::new();
    let result = self.root.parse(py, &mut stream, &mut ctx, &mut path)?;
    Ok(result.into_bound(py))
}
```

**变更点**：移除 `if self.root_has_expressions()` 分支，统一用 `placeholder`。
- `_parse_raw` 不再需要 `has_expressions` 判断（StructNode.parse 自己处理）
- `CompiledSchema.has_expressions` 字段仍保留（sizeof / build 方向仍需要）

`_build_raw` **不变**（build 方向仍需要 new_root 或 placeholder）。

### 3.6 Node trait parse 签名：不变

`Node::parse` 签名保持 `fn parse(&self, py, stream, ctx, path) -> Result<Py<PyAny>>`。
所有变更封装在 StructNode.parse 内部，不影响 trait 接口和其他节点。

### 3.7 新 parse 路径流程图

```
┌─────────────────────────────────────────────────────────────────┐
│  schema.rs _parse_raw                                           │
│                                                                 │
│  ctx = Context::placeholder(py)     ← 统一入口，不创建 PyDict    │
│  root.parse(py, stream, &mut ctx, &mut path)                    │
│         │                                                       │
│         ▼                                                       │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │  StructNode.parse                                        │  │
│  │                                                          │  │
│  │  ① instance = create_class(cls)   ← tp_new（含空__dict__）│  │
│  │                                                          │  │
│  │  ② dict = instance.getattr("__dict__")                   │  │
│  │       .downcast_into::<PyDict>()?   ← 借用实例 dict       │  │
│  │                                                          │  │
│  │  ┌── has_expressions=true ──────────────────────────┐   │  │
│  │  │  ③ ctx.inject_fields(dict)                       │   │  │
│  │  │  ④ ctx.init_expr_values(N)                       │   │  │
│  │  │  ⑤ for each field:                               │   │  │
│  │  │       value = field.node.parse(...)              │   │  │
│  │  │       ctx.set_field_at(idx, name, value)         │   │  │
│  │  │         ↑ 写入 ctx.fields（即实例 dict）+ buf    │   │  │
│  │  └──────────────────────────────────────────────────┘   │  │
│  │                                                          │  │
│  │  ┌── has_expressions=false ────────────────────────┐   │  │
│  │  │  ③ for each field:                               │   │  │
│  │  │       value = field.node.parse(...)              │   │  │
│  │  │       dict.set_item(name, value)                 │   │  │
│  │  │         ↑ 直接写实例 dict（不经 ctx）            │   │  │
│  │  └──────────────────────────────────────────────────┘   │  │
│  │                                                          │  │
│  │  ⑥ if has_post_init: instance.__post_init__()           │  │
│  │                                                          │  │
│  │  ⑦ return instance  ← 无 force_setattr                  │  │
│  └──────────────────────────────────────────────────────────┘  │
│                                                                 │
│  Ok(result.into_bound(py))                                      │
└─────────────────────────────────────────────────────────────────┘
```

**与 R3 流程对比**（R3 = 先填 dict → 创建实例 → force_setattr 替换）：

```
R3:  new_dict → fill dict → tp_new(instance+空dict) → force_setattr(替换) → post_init
R4:  tp_new(instance+空dict) → getattr(__dict__) → fill dict → post_init
     [省去: new_dict / force_setattr / "__dict__" str / 空dict丢弃]
```

### 3.8 嵌套 StructRef 流程

```
StructRefNode.parse (inner_has_expr=true):
  │
  ├─ child_ctx = Context::new_child_placeholder(ctx)  ← 有 parent，无 dict
  │
  └─ root.parse(py, stream, &mut child_ctx, path)
         │
         ▼
     ┌─ 内层 StructNode.parse ─────────────────────────────────┐
     │  ① instance_inner = create_class(inner_cls)             │
     │  ② dict_inner = instance_inner.getattr("__dict__")...   │
     │  ③ child_ctx.inject_fields(dict_inner)                  │
     │  ④ for each field: child_ctx.set_field_at(...)          │
     │  ⑤ return instance_inner                                 │
     └─────────────────────────────────────────────────────────┘
         │
         ▼
  外层 StructNode.parse 继续：ctx.set_field_at(outer_idx, "inner", instance_inner)
```

- 内层实例的 dict 被 inject 到 child_ctx
- child_ctx 的 parent 指向外层 ctx（支持嵌套 `_` 引用）
- 内层 parse 返回后，child_ctx drop（dict 引用 -1，但 inner_instance 持有）

---

## 4. 引用计数与借用安全性

### 4.1 dict 引用计数追踪

以 has_expressions=true 为例（最复杂路径）：

```
操作                                    dict 引用计数    持有者
──────────────────────────────────────  ──────────────  ──────────────
tp_new 创建实例（内部分配空 dict）          1            instance（tp_dictoffset）
instance.getattr("__dict__") 返回          2            instance + dict_bound
ctx.inject_fields(dict_bound)（move）      2            instance + ctx.fields
parse 循环 set_field_at（写 dict）          2            （不变）
函数返回 instance.unbind()                  2            instance(Py<PyAny>) + ctx.fields
_parse_raw 返回后 ctx drop                 1            instance(Py<PyAny>) ← 最终持有
Python 侧使用 instance                      1            （安全）
```

**结论**：dict 引用计数始终 ≥1，无 use-after-free 风险。

### 4.2 instance 与 dict 的 Rust 借用关系

```rust
let instance: Bound<'py, PyAny> = create_class(cls)?;       // 持有实例
let dict_bound: Bound<'py, PyDict> = instance               // 从实例取 dict
    .getattr(self.dict_attr_name.bind(py))?
    .downcast_into::<PyDict>()?;
```

- `instance` 和 `dict_bound` 都是 `Bound<'py, T>`（GIL-bound 引用，生命周期 `'py`）
- `getattr` 返回 new reference（PyObject_GetAttr incref +1），包装为 owned `Bound`
- `downcast_into` 不改变引用计数，仅做类型检查
- 两者在 Rust 侧是**独立的 handle**，不存在 Rust 借用冲突（都是 `'py` 生命周期的 immutable 借用）
- 对 dict 的修改通过 C API（`PyDict_SetItem`）进行，不涉及 Rust 可变借用

**与 R3 的对比**：R3 中 `dict` 和 `instance` 也是独立的 handle（dict 是新建的）。
R4 中 dict 来自 instance 的 getattr，但 Rust 侧的借用关系相同——无新增借用风险。

### 4.3 `dict_bound` move 到 ctx 后的生命周期

```rust
ctx.inject_fields(dict_bound);  // dict_bound move into ctx.fields
// dict_bound 此后不可访问（已 move）
// ctx.fields: Option<Bound<'py, PyDict>> 持有 dict
```

- `inject_fields` 接收 `Bound<'py, PyDict>`（owned），move 到 `self.fields`
- 生命周期 `'py` 与 Context 一致（Context<'py>）
- instance 仍通过 `Bound<'py, PyAny>` 独立持有，不受 dict move 影响

**`inject_fields` 是否需要 clone_ref？** 不需要。
- `getattr` 返回的 `Bound<PyDict>` 是 owned（引用计数已 +1）
- move 到 ctx.fields 后，ctx 持有这个 +1
- instance 的引用是独立的（tp_dictoffset 内部引用，不经 Rust handle）
- 无需额外 incref

### 4.4 has_expressions=false 的 dict 生命周期

```rust
let dict_bound: Bound<'py, PyDict> = ...;  // owned，引用计数 2
// parse 循环直接用 dict_bound.set_item(...)
// 函数末尾 dict_bound drop → 引用计数 1（instance 持有）
// instance.unbind() → 返回给调用方
```

- `dict_bound` 存活到函数末尾，parse 循环中可安全使用
- 函数返回前 `dict_bound` drop（-1），instance.unbind() 保持实例存活
- 安全

---

## 5. ABI3 兼容性

### 5.1 使用的 C API

| 操作 | C API | ABI3 状态 |
|------|-------|-----------|
| 创建实例 | `PyType_GetSlot(cls, Py_tp_new)` + `newfunc` 调用 | ✅ 稳定（Python 3.4+） |
| 获取 `__dict__` | `PyObject_GetAttr(obj, name)` | ✅ 稳定（公开 API） |
| intern 字符串 | `PyUnicode_InternInPlace` | ✅ 稳定 |
| dict set_item | `PyDict_SetItem` | ✅ 稳定 |
| 调用 `__post_init__` | `PyObject_CallMethod0` | ✅ 稳定 |

### 5.2 不使用的 C API（R3 用但 R4 不再需要）

| 操作 | C API | 说明 |
|------|-------|------|
| 整体替换 `__dict__` | `PyObject_GenericSetAttr` | R4 不再需要（无 force_setattr） |

> **注意**：`force_setattr` 函数本身**不删除**（instance.rs 保留），
> 仅 parse 路径不再调用。未来其他场景（如 slots 支持的逐字段 setattr）可能复用。

### 5.3 关于 `tp_dictoffset` 直接访问

**不采用**。理由：
1. `Py_tp_dictoffset` 不在 `PyType_GetSlot` 的文档化 slot 列表中
2. Python 3.11+ 中 `tp_dictoffset` 可能为负值（表示 dict 在负偏移处），处理复杂
3. `PyObject_GetAttr(obj, "__dict__")` 是稳定 API，CPython 内部有 fast path
4. 收益（~5-10ns vs getattr）不足以承担 ABI3 风险

### 5.4 `getattr("__dict__")` 返回值类型保证

| 类形态 | `getattr("__dict__")` 返回 | downcast::<PyDict> |
|--------|---------------------------|-------------------|
| 普通 Python 类（含 @dataclass） | `dict` 实例 | ✅ 成功 |
| `@dataclass(frozen=True)` | `dict` 实例 | ✅ 成功 |
| `@dataclass(slots=True)` | **无 `__dict__` 属性** | ❌ getattr 失败 |
| 手写 `__slots__` 的类 | **无 `__dict__` 属性** | ❌ getattr 失败 |

- 标准 @dataclass 总有 `__dict__`（non-slots），getattr 成功
- slots 类无 `__dict__`，getattr 失败 → 错误信息提示 slots（运行期兜底）
- 与 R3 的 force_setattr 失败兜底语义一致

---

## 6. 影响面分析（逐文件）

### 6.1 `context.rs`（新增方法）

| 改动类型 | 内容 |
|---------|------|
| **新增方法** | `inject_fields(&mut self, dict: Bound<'py, PyDict>)` |
| **新增方法** | `new_child_placeholder(parent: &'py Context<'py>) -> Self` |
| **保留** | `new_root`, `placeholder`, `new_child`, `take_fields`（兼容性） |
| **测试** | 新增 `inject_fields_*` 和 `new_child_placeholder_*` 测试 |

改动量：~40 行（含测试）。

### 6.2 `nodes/struct_node.rs`（parse 重写）

| 改动类型 | 内容 |
|---------|------|
| **新增字段** | `dict_attr_name: Py<PyString>`（interned "__dict__"） |
| **修改构造** | `StructNode::new` 中初始化 `dict_attr_name` |
| **重写 parse** | 先 create_class → getattr dict → inject/直接写 → post_init → 返回 |
| **移除调用** | `force_setattr`（parse 中不再调用）、`take_fields`（不再调用） |
| **移除调用** | `PyDict::new_bound`（has_expressions=false 分支不再新建 dict） |
| **import** | 移除 `force_setattr`（如不再使用）；保留 `create_class`、`intern_pystring` |

改动量：~50 行（parse 方法约 +20 -30 净变化，字段/构造 +5）。

### 6.3 `nodes/struct_ref.rs`（parse 简化）

| 改动类型 | 内容 |
|---------|------|
| **修改 parse** | `Context::new_child(ctx, py)` → `Context::new_child_placeholder(ctx)` |
| **注释更新** | 说明内层 StructNode.parse 会 inject 实例 dict |

改动量：~5 行。

### 6.4 `schema.rs`（_parse_raw 简化）

| 改动类型 | 内容 |
|---------|------|
| **简化 _parse_raw** | 移除 `if self.root_has_expressions()` 分支，统一 `placeholder` |
| **保留** | `_build_raw` 不变（仍需 new_root/placeholder） |
| **保留** | `CompiledSchema.has_expressions` 字段（build/sizeof 仍需要） |

改动量：~5 行（-8 +3 净变化）。

### 6.5 `instance.rs`（无改动）

| 改动类型 | 内容 |
|---------|------|
| **保留** | `create_class`（parse 仍需要） |
| **保留** | `force_setattr`（parse 不再调用，但保留函数以防其他用途） |
| **保留** | `intern_pystring`（StructNode 初始化 dict_attr_name 用） |

改动量：0 行。

### 6.6 其他文件（无改动）

| 文件 | 说明 |
|------|------|
| `nodes/mod.rs` | Node trait 不变 |
| `nodes/bytes.rs` | 不变（eval_expr_int 调用不变） |
| `nodes/computed.rs` | 不变 |
| `nodes/tell.rs` | 不变 |
| `expr.rs` | 不变（get_int_at / eval_expr_int 不变） |
| `compile.rs` | StructNode::new 调用方式不变（dict_attr_name 在 new 内部创建） |
| `error.rs` | 不变 |

### 6.7 改动量汇总

| 文件 | 改动行数（估） | 风险 |
|------|:---:|:---:|
| context.rs | ~40 | 低 |
| struct_node.rs | ~50 | **中**（parse 核心重写） |
| struct_ref.rs | ~5 | 低 |
| schema.rs | ~5 | 低 |
| 测试（各文件） | ~80 | 低 |
| **合计** | **~180** | — |

---

## 7. 性能预估

### 7.1 消除的操作

| 操作 | R3 存在 | R4 消除 | 估算 ns |
|------|:---:|:---:|--------:|
| `PyDict::new_bound` / `Context::new_root`（新建 dict） | ✅ | ✅ | 30-50 |
| 实例空 dict 创建（tp_new 内部，被替换丢弃） | ✅ | —（不再丢弃，被复用） | 20-30（省去 GC） |
| `force_setattr`（`PyObject_GenericSetAttr`） | ✅ | ✅ | 20-30 |
| `"__dict__".into_py` PyString 创建 | ✅ | ✅ | 10-15 |
| `value.into_py`（dict → Py\<PyAny\>） | ✅ | ✅ | 2-3 |
| `ctx.take_fields()` | ✅ | ✅ | ~1 |
| **合计消除** | | | **83-129** |

### 7.2 新增的操作

| 操作 | R3 无 | R4 新增 | 估算 ns |
|------|:---:|:---:|--------:|
| `instance.getattr(interned_"__dict__")` | — | ✅ | 15-25 |
| `downcast_into::<PyDict>()` | — | ✅ | ~1 |
| `ctx.inject_fields(dict)`（has_expressions 路径） | — | ✅ | ~1 |
| **合计新增** | | | **17-27** |

### 7.3 净节省

**净节省 ≈ (83-129) - (17-27) = 66-102 ns/parse**

保守取 **~70ns**，乐观取 **~100ns**。

### 7.4 嵌套 StructRef 额外节省

R3 中 StructRef 创建 `new_child`（带空 PyDict ~30-50ns）。
R4 改用 `new_child_placeholder`（零 PyDict），额外节省 **~30-50ns/嵌套层**。

### 7.5 各场景预估（基于 Phase 2.5 Vec 化后实测中位数）

> 基线：Phase 2.5 Vec 化修复后的实测中位数（过程记录 §性能退化排查）。
> 本优化仅影响 parse 方向（build 不变）。

| 场景 | 方向 | Vec化后 ns | R4 预估 ns (-70~100) | py ns | R4 预估比率 | ≥10x? |
|------|------|---:|---:|---:|---:|:---:|
| E1 | parse | 350 | **250-280** | 2724 | **9.7-10.9x** | ⚠️→✅ |
| E1 | build | 296 | 296（不变） | 2655 | 9.0x | ❌（不受影响） |
| E2 | parse | 410 | **310-340** | 3602 | **10.6-11.6x** | ✅ |
| E2 | build | 318 | 318（不变） | 3449 | 10.9x | ✅ |
| E3 | parse | 650 | **550-580** | 4682 | **8.1-8.5x** | ❌ |
| E3 | build | 444 | 444（不变） | 4271 | 9.6x | ❌ |

**关键结论**：
- **E1 parse**：保守预估 9.7x，乐观 10.9x。是本优化的主要受益场景（固定开销占比最大）
- **E2 parse**：预估达标（10.6x+）
- **E3 parse**：仍不达标（8.1-8.5x），因 E3 字段/表达式多，固定开销占比小
- **build 方向**：完全不受影响（需另寻优化方向）

### 7.6 局限性说明

本优化**不能单独让所有 parse 场景达标 ≥10x**。E3 parse 因字段多（6 字段 + 3 GetInt），
固定开销占比相对小（~650ns 中固定开销 ~130ns，其余为字段解析 + 表达式求值）。
E1 parse 的固定开销占比最大（~350ns 中固定 ~130ns），所以受益最明显。

如需 E3 parse 达标，可能需要叠加其他优化（如 Tell/Computed 节点的零开销化、
BuildStream/ParseStream 的进一步优化等）。本优化是固定开销优化的**最后一项**
（force_setattr + dict 创建已全部消除）。

### 7.7 性能假设与验证方法

**性能假设**（性能门禁 §1）：
- **瓶颈识别**：R3 parse 路径创建 2 个 dict（一个浪费）+ force_setattr 固定开销
  ~32-48ns/次，有量化来源（§1.3 拆解）
- **可证伪预测**：净节省 66-102ns/parse，E1 parse 预估从 7.8x 提升至 9.7-10.9x
- **验证方法**：DEV 实现后 PM 执行 benchmark vs Python construct 绝对基线，
  对比 Vec 化后中位数，确认 parse 方向提升 ≥60ns

**预测为假的应对**：若实测净节省 <40ns，说明 getattr("__dict__") 开销超预期
（>40ns），需 micro-benchmark 诊断 getattr 路径。若净节省 >100ns，说明空 dict
GC 回收开销比预估更高（乐观情况）。

---

## 8. 边界条件清单

### 8.1 slots 类（无 `__dict__`）

- **场景**：用户类使用 `__slots__` 或 `@dataclass(slots=True)`，实例无 `__dict__`
- **预期**：`instance.getattr("__dict__")` 失败（AttributeError）或返回非 dict
- **行为**：返回 `ConstructError::Generic`，消息提示 slots 不支持
- **与 R3 对比**：R3 在 force_setattr 时失败（slots 类 GenericSetAttr 报错），
  R4 在 getattr 时失败（更早，错误更明确）
- **测试**：构造 slots 类，验证 parse 返回 slots 相关错误信息

### 8.2 frozen dataclass

- **场景**：`@dataclass(frozen=True)` 的类，`__setattr__` 会 raise FrozenInstanceError
- **预期**：直接 `dict.set_item` 不经过 `__setattr__`，写入成功
- **行为**：parse 正常返回实例，字段已写入 `__dict__`
- **与 R3 对比**：R3 用 force_setattr 绕过 frozen setattr；R4 用 dict.set_item 绕过
  （机制不同，结果一致）
- **测试**：复用现有 `parse_frozen_dataclass_does_not_raise` 测试

### 8.3 `__post_init__`

- **场景**：用户类定义了 `__post_init__`
- **预期**：在 dict 填充完毕后调用（字段已可读）
- **行为**：与 R3 一致（R3 是 force_setattr 后调用，R4 是 dict 填充后调用，
  两种情况下字段都已在 `__dict__` 中）
- **测试**：复用现有 `parse_invokes_post_init_when_flag_set` 测试

### 8.4 嵌套 StructRef + 内层 has_expressions

- **场景**：外层 Struct 含 StructRef 字段，内层 has_expressions=true
- **预期**：内层 StructNode.parse 创建内层实例，inject dict 到 child_ctx，
  表达式正常求值
- **行为**：child_ctx 有 parent（外层 ctx），内层表达式可通过 parent 访问外层字段
- **测试**：构造嵌套结构（外层含表达式 + 内层含表达式），验证双向 `_` 引用正确

### 8.5 空结构体（0 字段）

- **场景**：StructNode 无字段
- **预期**：create_class 创建实例，getattr dict 成功（空 dict），循环不执行，
  返回空实例
- **行为**：与 R3 一致
- **测试**：复用现有 `empty_struct_parse_empty_bytes_returns_empty_instance`

### 8.6 WO 字段（只写）

- **场景**：Struct 含 WO 字段（parse 时消费字节但不存入 dict）
- **预期**：WO 字段的值被 drop，不 set_item 到 dict
- **行为**：与 R3 一致（match field.mode 分支处理）
- **测试**：复用现有 `wo_field_parse_consumes_bytes_but_not_stored_in_instance`

### 8.7 dict 引用为 None（理论不发生）

- **场景**：tp_new 返回的实例 `__dict__` 为 None（极罕见的自定义 `__new__`）
- **预期**：getattr("__dict__") 返回 None，downcast_into::<PyDict> 失败
- **行为**：返回 `ConstructError::Generic`，提示 `__dict__` 不是 dict
- **测试**：难以构造（需自定义 `__new__` 返回无 dict 实例），列为已知限制

### 8.8 多次 inject_fields（防御性）

- **场景**：对同一个 ctx 调用两次 inject_fields
- **预期**：第二次的 dict 替换第一次的，旧 dict 引用 drop（-1）
- **行为**：生产路径不触发（StructNode.parse 只 inject 一次）。防御性安全
- **测试**：context.rs 单元测试覆盖

### 8.9 >16 字段 Struct（MAX_INLINE_FIELDS 截断）

- **场景**：Struct 字段数超过 MAX_INLINE_FIELDS(16)，且 has_expressions=true
- **预期**：init_expr_values 截断到 16，get_int_at(idx>=16) 返回 ExprFieldMissing
- **行为**：与 Phase 2.5 一致（VET SHOULD-FIX SF-1，不因本修订改变）
- **测试**：现有边界测试覆盖

---

## 9. 风险与缓解

### 9.1 getattr("__dict__") 开销超预期（中风险）

**风险**：PyObject_GetAttr 对 `__dict__` 的开销可能 >25ns（取决于 CPython 版本/
属性查找路径），侵蚀净节省。

**缓解**：
1. 使用 interned "__dict__" 缓存（命中 CPython interned 池 fast path）
2. 若实测 >40ns，考虑直接通过 `tp_dictoffset` 访问（需评估 ABI3 兼容性）
3. Micro-benchmark 单独测量 getattr 路径

### 9.2 downcast 失败的边缘情况（低风险）

**风险**：某些特殊类（如 metaclass 定制 `__dict__` 为 dictproxy）导致 downcast 失败。

**缓解**：
1. 标准 @dataclass 不会出现此问题（`__dict__` 总是普通 dict）
2. downcast 失败时返回明确错误（非 panic）
3. 文档标注不支持 slots/metaclass 定制 `__dict__` 的类

### 9.3 parse 路径借用复杂度增加（低风险）

**风险**：StructNode.parse 现在同时持有 instance 和 dict，代码复杂度略增。

**缓解**：
1. instance 和 dict 是独立 GIL-bound handle，无 Rust 借用冲突
2. 文档注释完整说明引用计数追踪（§4.1）
3. 单元测试覆盖引用计数正确性（间接验证：round-trip 测试通过即证明 dict 存活）

### 9.4 build 方向不受益（已知限制）

**风险**：build 方向仍有固定开销（Context::new_root 的 PyDict 创建），不受益于本优化。

**缓解**：
1. 本修订明确标注"仅优化 parse 方向"
2. build 方向如需优化，另立设计（如 build 路径也用 inject dict——但 build 无实例，
   需要创建临时 dict 作为表达式存储，无法消除）
3. PM 验收时分别评估 parse/build 达标情况

---

## 10. 与 Python 版本对应

本修订不改变任何对外 API 或 Python 行为。Python construct 的 `Struct._parse`
（core.py:2232-2245）创建 `Container()` 并逐字段填充——R4 的 Rust 实现只是将
"Container 创建"对应为"tp_new 创建实例 + 借用 __dict__"，语义不变。

| Python construct | R3 方案 B'（Rust） | R4 本修订（Rust） |
|-----------------|-------------------|-------------------|
| `obj = Container()` | `PyDict::new_bound` 新建 dict | 借用实例 `__dict__`（tp_new 附带） |
| `obj[sc.name] = subobj` | `dict.set_item` 或 `ctx.set_field_at` | 同（dict 来源不同） |
| 返回 `obj` | force_setattr 替换 instance.__dict__ → 返回 instance | 直接返回 instance（dict 已是 __dict__） |

---

## 11. 实施建议

### 11.1 实施顺序

```
Step 1: context.rs 新增 inject_fields + new_child_placeholder（含测试）
Step 2: struct_node.rs 重写 parse + 新增 dict_attr_name 字段
Step 3: struct_ref.rs 改用 new_child_placeholder
Step 4: schema.rs 简化 _parse_raw
Step 5: cargo test 全部通过
Step 6: maturin develop --release + benchmark 验证
```

### 11.2 测试策略

| 测试类别 | 策略 |
|---------|------|
| **现有测试** | 全部保持通过（不改测试逻辑，除非测试直接断言 force_setattr 调用） |
| **新增 context 测试** | `inject_fields_*`（注入后 fields 可用）、`new_child_placeholder_*`（有 parent 无 dict） |
| **round-trip** | 现有 parse/build round-trip 测试自动覆盖 dict 存活正确性 |
| **frozen dataclass** | 复用现有 `parse_frozen_dataclass_does_not_raise` |
| **slots 兜底** | 新增：slots 类 parse 返回明确错误 |
| **interned key** | 复用现有 `parse_dict_keys_are_interned_identity`（验证 dict key 仍是 interned） |

### 11.3 需要注意的测试适配

- 现有测试 `parse_dict_keys_match_field_names` 和 `parse_dict_keys_are_interned_identity`
  通过 `instance.getattr("__dict__")` 检查 dict 内容——R4 中 dict 来源变了但内容不变，
  这些测试**无需修改**（它们检查的是 dict 的内容，不是 dict 的来源）
- 若有测试直接 mock `force_setattr` 或检查其调用次数，需适配（grep 确认无此类测试）

---

## 设计完成状态

- [x] 问题诊断（两个 dict 的浪费 + force_setattr 开销拆解）
- [x] 解决方案（借用实例 __dict__ + 消除 force_setattr）
- [x] 详细设计（Context API + StructNode.parse 重写 + StructRef + schema）
- [x] 流程图（R3 vs R4 对比）
- [x] 引用计数与借用安全性分析
- [x] ABI3 兼容性
- [x] 影响面分析（6 个文件）
- [x] 性能预估（净节省 66-102ns + 各场景预估）
- [x] 边界条件清单（9 个场景）
- [x] 风险与缓解

**需要 PM 决策的点**：

1. **E3 parse 仍不达标**：本优化后 E3 parse 预估 8.1-8.5x，不达 ≥10x。
   是否接受"E3 parse ≥8x 视为阶段性达标"，还是要求追加优化（Tell/Computed 零开销化等）？
   本优化是 parse 固定开销的**最后一项**（force_setattr + dict 创建已全部消除），
   进一步优化需转向字段级（每字段开销）或表达式求值。

2. **build 方向不受益**：本修订仅优化 parse。build 方向 E1/E3 仍未达标。
   是否需要为 build 方向另立优化设计？（build 无实例创建，优化空间有限）

3. **take_fields 去留**：R4 后 parse 路径不再调用 `take_fields`。是否标记 deprecated
   或直接移除？（grep 确认仅 struct_node.rs parse 调用，移除安全）

---

*本设计修订（**R4，2026-06-24**）经 REV 检视 + PM 确认后，由 DEV 实施。R4 在 R3 方案 B'
基础上消除中间 dict（借用实例 __dict__）+ 消除 force_setattr 固定开销，是 parse 路径
固定开销优化的最终项。*
