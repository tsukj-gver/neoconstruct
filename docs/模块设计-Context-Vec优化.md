# 模块设计：Context Vec 优化（Phase 2.5）

> **目标**：使 E1-E3 全部 parse/build 场景达到 ≥10x 加速比（vs Python construct 2.10.70）。
>
> **性质**：框架级性能优化，不改变对外 API 与功能语义。
>
> **验收基线**：tag `phase-2-clean-final`。

---

## 1. 优化方案总览

### 1.1 瓶颈定位（PM 诊断结论摘要）

GetInt 指令的 PyDict hash 查找是表达式场景的核心瓶颈：

| 操作 | Rust Δns | Python Δns | 增量比率 |
|------|---:|---:|:---:|
| Int8ub 字段 | 23.0 | 407.5 | 17.7x |
| Computed(2GI)-over-Int8ub | 54.4 | 408.2 | **7.5x** |

GetInt 精确成本 = **29.2ns**（`PyDict_GetItem` ~24ns + `PyLong_AsLongLong` ~5ns）。
Python 侧 `this.end` 也是 dict 查找——两边调用同一个 CPython C 函数，Rust 优势被压缩。

完整调用链（当前）：`GetInt(idx)` → `names[idx]` → `PyDict_GetItem`(~24ns) →
`from_borrowed_ptr`(~2ns) → `extract::<i64>`(~7ns) ≈ **36ns**。

### 1.2 优化项排序

| 优先级 | 优化项 | 预期收益/GetInt | 影响面 | 风险 |
|:---:|--------|:---:|--------|:---:|
| **P0** | Context Vec 化（核心） | ~30ns | context/expr/struct_node/bytes/computed/mod | 低 |
| **P1** | 移除 field_names 体系 | ~10ns/expr 调用 | 同上（清理代码） | 低 |
| **P2** | dict clone → take | ~3-5ns | struct_node | 低 |
| **P3** | interned "__dict__" 缓存 | ~3-5ns | instance/struct_node | 低 |
| **P4** | force_setattr 内联简化 | ~5-10ns | instance | 中 |

- **P0 + P1 为 Layer 1（必做）**：覆盖 GetInt 热路径 + 清理冗余间接层。
- **P2-P4 为 Layer 2（视实测决定）**：固定开销微优化，补足接近达标的场景。

### 1.3 设计核心思路

`Context` 新增 `expr_values: Option<Vec<*mut PyObject>>`，按**编译期字段索引**
存储 borrowed 指针（指向 fields PyDict 中的值对象）。GetInt 从 PyDict hash 查找
改为 Vec 数组索引：

- 当前：`names[idx]` → `PyDict_GetItem`(~24ns) → `PyLong_AsLongLong`(~5ns) ≈ 36ns
- 优化后：`vec[idx]` → `PyLong_AsLongLong`(~5ns) ≈ **6ns**
- 每次 GetInt 节省 ~30ns

**为什么用 raw pointer 而非 `Option<Py<PyAny>>`：**

`Option<Py<PyAny>>` 方案在 set_field 时需额外一次 `Py_INCREF`（Vec 持有引用），
Context drop 时再 `Py_DECREF`。N 字段 = 2N 次原子引用计数操作（~2ns × 2N）。
raw pointer 方案零额外引用计数——指针仅"借用" dict 中的值，dict 持有引用计数，
Vec 与 Context 同生命周期，安全性可证明（见 §2.5）。

> **替代方案对比**：i64 提前缓存（`Vec<Option<i64>>`）在 set_field 时提前 extract，
> 对不被 GetInt 引用的字段（如 bytes 值）产生无谓 extract 开销（~5ns/字段），
> 经分析净收益反而低于 raw pointer（见附录 A）。

---

## 2. Context Vec 化详细设计

### 2.1 新 Context struct 定义

```rust
use pyo3::ffi;
use pyo3::types::{PyDict, PyString};

/// 解析/构建上下文。
///
/// 字段值存入 `PyDict`（C API `PyDict_SetItem`），同时按编译期字段索引
/// 在 `expr_values` 中缓存 borrowed 指针，供 GetInt 快速访问（跳过 hash 查找）。
///
/// `expr_values` 为 `Option`：仅 `has_expressions=true` 的 Struct 通过
/// [`Context::init_expr_values`] 初始化；无表达式的 Struct（placeholder）
/// 保持 `None`，零分配开销。
pub struct Context<'py> {
    /// 当前层字段字典（`PyDict` 引用），按需创建。
    ///
    /// parse 时存入已解析字段；build 时存入从对象读取的字段值。
    /// 同时充当 instance dict（has_expressions 路径）。
    /// `None` 表示占位 context（无表达式的 Struct 入口使用）。
    fields: Option<Bound<'py, PyDict>>,

    /// 外层上下文（嵌套 Struct 时指向父 Context），对应 Python construct 的 `_`。
    parent: Option<&'py Context<'py>>,

    /// 按编译期字段索引存储的 borrowed PyObject 指针，供 GetInt 快速访问。
    ///
    /// `None` 表示未初始化（无表达式的 Struct 或 placeholder）。
    /// `Some(vec)` 中每个槽位：
    /// - 非 null：指向 `fields` PyDict 中对应字段的值对象（borrowed，不 incref）
    /// - null：WO 字段（不写入值）或未设置的字段
    ///
    /// GetInt(idx) 直接读 `vec[idx]` → `PyLong_AsLongLong`，跳过 PyDict hash 查找。
    ///
    /// # Safety（不变量）
    ///
    /// 1. 指针仅在 [`Context::set_field_at`] 中写入（与 dict 写入同步）
    /// 2. 指针仅在 [`Context::get_int_by_index`] 中读取（fields dict 存活期间）
    /// 3. `fields` 与 `expr_values` 同属 Context，生命周期一致
    /// 4. dict 中的值不会被外部替换（Context API 是唯一写入路径）
    /// 5. PyDict resize 时不移动值对象（PyObject 在堆上，dict 内部数组重分配
    ///    不影响已存入值的对象指针）
    expr_values: Option<Vec<*mut ffi::PyObject>>,
}
```

**移除的字段**（当前 Context 持有）：

| 字段 | 移除原因 |
|------|---------|
| `field_names_owned: Option<Vec<Py<PyString>>>` | Vec 化后 GetInt 不再需要字段名，names 参数移除 |
| `field_names_ptr: Option<*const Py<PyString>>` | 同上 |
| `field_names_len: usize` | 同上 |

### 2.2 方法签名：新增

#### `init_expr_values`（替代 `set_field_names_ref`）

```rust
impl<'py> Context<'py> {
    /// 初始化 expr_values 缓冲区（预分配 n 个 null 槽位）。
    ///
    /// 在 StructNode.parse/build 的 `has_expressions` 分支入口调用一次，
    /// 在遍历字段之前。槽位数 = 当前 StructNode 的字段数。
    ///
    /// 后续 [`Context::set_field_at`] 按 idx 填充槽位（RW/RO 字段），
    /// WO 字段的槽位保持 null（编译期保证 GetInt 不引用 WO 字段）。
    ///
    /// 对占位 context（`fields = None`）安全：仅记录 n，不分配（无表达式
    /// 的 Struct 不会调用此方法，由编译期 `has_expressions` 标志保证）。
    ///
    /// # 性能
    ///
    /// `vec![ptr::null_mut(); n]` 零次 Py_INCREF，仅一次 Vec 堆分配
    /// （n 个指针宽度 = 8n 字节，典型 n=2-4，单次 malloc ~5ns）。
    pub fn init_expr_values(&mut self, n: usize) {
        self.expr_values = Some(vec![std::ptr::null_mut(); n]);
    }
}
```

#### `set_field_at`（替代 `set_field_interned`）

```rust
impl<'py> Context<'py> {
    /// 按索引写入字段值（同时写 PyDict + expr_values）。
    ///
    /// 替代原 [`Context::set_field_interned`]，增加 `idx` 参数用于同步 expr_values。
    /// 由 StructNode.parse/build 在每个 RW/RO 字段完成后调用。
    ///
    /// # 操作
    ///
    /// 1. `fields.set_item(name, value)` — 写入 PyDict（C API `PyDict_SetItem`，
    ///    interned key 命中 fast path，~10-15ns）
    /// 2. `expr_values[idx] = value.as_ptr()` — 写入 borrowed 指针（~1ns，不 incref）
    ///
    /// # 借用
    ///
    /// 需要 `&mut self`（写 expr_values）。StructNode.parse/build 中
    /// `field.node.parse(...)` 与 `ctx.set_field_at(...)` 是**顺序调用**，
    /// 不存在同时借用 ctx 的情况（见 §2.4 借用分析）。
    ///
    /// 对占位 context（`fields = None`）为无操作（has_expressions=false 的
    /// Struct 不调用此方法）。
    ///
    /// # 错误
    ///
    /// `PyDict_SetItem` 失败时返回 `PyErr`（转为 `ConstructError::Generic`）。
    pub fn set_field_at(
        &mut self,
        idx: usize,
        name: &Py<PyString>,
        value: &Bound<'_, PyAny>,
        py: Python<'_>,
    ) -> PyResult<()> {
        match (&self.fields, &mut self.expr_values) {
            (Some(fields), Some(vals)) => {
                fields.set_item(name.bind(py), value)?;
                // 同步 borrowed 指针到 expr_values（不 incref，依赖 dict 持有引用）。
                if let Some(slot) = vals.get_mut(idx) {
                    *slot = value.as_ptr();
                }
                // idx 越界：编译期保证 idx < fields.len() == vals.len()。
                // 防御性忽略（不 panic，符合 §8 红线）。
                Ok(())
            }
            // 占位 context 或未 init_expr_values：仅写 dict（若存在），跳过 Vec。
            (Some(fields), None) => fields.set_item(name.bind(py), value),
            (None, _) => Ok(()),
        }
    }
}
```

#### `get_int_by_index`（替代 `get_int_by_name`）

```rust
impl<'py> Context<'py> {
    /// 按编译期字段索引取整数值（VM `GetInt` 指令使用）。
    ///
    /// 从 `expr_values[idx]` 取 borrowed PyObject 指针，直接调用
    /// `PyLong_AsLongLong`（~5ns），跳过 `PyDict_GetItem` hash 查找（~24ns）。
    ///
    /// 完整路径：`vec[idx]`(~1ns) → `PyLong_AsLongLong`(~5ns) = **~6ns**
    /// （vs 原 `get_int_by_name` ~36ns，节省 ~30ns）。
    ///
    /// # 错误
    ///
    /// - [`ConstructError::ExprContext`]：expr_values 未初始化（占位 context，
    ///   理论不发生——有表达式的 Struct 调用 init_expr_values）。
    /// - [`ConstructError::ExprFieldMissing`]：槽位为 null（WO 字段或未设置）。
    /// - [`ConstructError::ExprType`]：值无法 `PyLong_AsLongLong`（非整数类型）。
    pub fn get_int_by_index(
        &self,
        idx: usize,
        py: Python<'_>,
    ) -> Result<i64, ConstructError> {
        let vals = self.expr_values.as_ref().ok_or(ConstructError::ExprContext {
            message: "get_int_by_index: expr_values not initialized (placeholder context)"
                .to_string(),
            path: String::new(),
        })?;
        let ptr = vals.get(idx).copied().unwrap_or(std::ptr::null_mut());
        if ptr.is_null() {
            return Err(ConstructError::ExprFieldMissing {
                field: format!("<index {}>", idx),
                path: String::new(),
            });
        }
        // SAFETY: ptr 来自 set_field_at 写入的 value.as_ptr()，指向 fields dict
        // 中的值对象。dict 由 self.fields (Bound<PyDict>) 保证存活，值对象因此存活。
        // PyLong_AsLongLong 对非 PyLong 对象返回 -1 并设置 OverflowError/TypeError，
        // 我们检查返回值区分错误类型。
        let v = unsafe { ffi::PyLong_AsLongLong(ptr) };
        if v == -1 {
            // 区分"值为 -1"与"转换失败"：检查 PyErr_Occurred
            // SAFETY: 持有 GIL，PyErr_Occurred 仅检查不取异常。
            let err_occurred = unsafe { ffi::PyErr_Occurred() };
            if !err_occurred.is_null() {
                unsafe { ffi::PyErr_Clear() };
                return Err(ConstructError::ExprType {
                    field: format!("<index {}>", idx),
                    expected: "integer (i64)".to_string(),
                    path: String::new(),
                });
            }
        }
        Ok(v)
    }
}
```

### 2.3 方法签名：移除 / 保留

**移除的方法**（Vec 化后不再需要）：

| 方法 | 移除原因 |
|------|---------|
| `set_field_names(&mut self, names: Vec<Py<PyString>>)` | GetInt 不再通过字段名查找 |
| `set_field_names_ref(&mut self, names: &[Py<PyString>])` | 同上，被 `init_expr_values(n)` 替代 |
| `field_names(&self) -> Option<&[Py<PyString>]>` | names 参数从 eval_expr_int 移除 |
| `get_int_by_name(&self, name, py) -> Result<i64>` | 被 `get_int_by_index(idx, py)` 替代 |
| `get_obj_by_name(&self, name) -> Result<Option<Bound>>` | 当前无生产调用方（Phase 3 Switch 实现时重新设计） |

**保留不变的方法**：

| 方法 | 说明 |
|------|------|
| `new_root(py)` | 创建顶层 context；`expr_values` 初始化为 `None` |
| `placeholder(py)` | 占位 context；`fields = None, expr_values = None` |
| `new_child(parent, py)` | 嵌套 context；`expr_values = None`（子层自行 init） |
| `set_field(&self, name, value)` | `&self`，仅写 dict（测试用，不写 Vec） |
| `get_field(&self, name)` | `&self`，仅读 dict |
| `parent(&self)` | 返回父 context |
| `fields(&self)` | 返回 dict 引用（测试/调试） |

**修改的构造函数**：

`new_root` / `placeholder` / `new_child` 的初始化列表移除 `field_names_*` 字段，
新增 `expr_values: None`。

### 2.4 eval_expr_int 新签名

```rust
/// 在 Rust 内部栈式求值表达式。
///
/// # 参数变更
///
/// - 移除 `names: &[Py<PyString>]`：GetInt 直接用 idx 访问 `ctx.expr_values`，
///   不再需要字段名表
/// - `ctx` 参数不变（`&Context`，不可变借用——get_int_by_index 是 `&self`）
pub fn eval_expr_int(
    program: &ExprProgram,
    ctx: &Context<'_>,
    py: Python<'_>,
) -> Result<i64, ConstructError>
```

GetInt 分支改写：

```rust
ExprOp::GetInt(idx) => {
    // 移除：let name = names.get(*idx)?; let val = ctx.get_int_by_name(name.bind(py), py)?;
    let val = ctx.get_int_by_index(*idx, py)?;
    stack_buf[stack_len] = val;
    stack_len += 1;
}
```

**附加收益**：移除 names 参数连带移除调用方的 `ctx.field_names()` 调用
（bytes.rs / computed.rs / mod.rs 各省 ~8-10ns 的 field_names() 开销 +
错误路径 ok_or_else 闭包）。

### 2.5 借用安全性分析

**核心问题**：`set_field_at` 需要 `&mut self`，是否与 parse/build 中的其他
ctx 借用冲突？

#### StructNode.parse（has_expressions 分支）

```rust
ctx.init_expr_values(self.fields.len());              // &mut ctx（一次性）
for (idx, field) in self.fields.iter().enumerate() {
    let value = field.node.parse(py, stream, ctx, path)?;  // &mut ctx（子节点）
    // ↑ parse 返回后，ctx 的 &mut 借用释放
    ctx.set_field_at(idx, field.name.py_name(), value.bind(py), py)?;  // &mut ctx
}
```

`field.node.parse(...)` 与 `ctx.set_field_at(...)` 是**顺序语句**，非嵌套借用。
`field.node.parse` 返回 `Py<PyAny>`（owned，与 ctx 无关）后，`&mut ctx` 借用结束，
随后的 `ctx.set_field_at` 重新获取 `&mut ctx`。**无冲突**。

#### StructNode.build（has_expressions 分支）

```rust
ctx.init_expr_values(self.fields.len());
for (idx, field) in self.fields.iter().enumerate() {
    // RW 路径
    let value = obj.getattr(...)?;                    // 不借用 ctx
    ctx.set_field_at(idx, ..., &value, py)?;          // &mut ctx
    field.node.build(py, &value, stream, ctx, path)?; // &mut ctx（顺序）
    // RO 路径
    let value = field.node.compute_ro_value(py, stream, ctx, path)?; // &ctx（不可变！）
    // ↑ compute_ro_value 接收 &Context（不可变），用于 Computed 求值
    ctx.set_field_at(idx, ..., value.bind(py), py)?;  // &mut ctx（compute_ro_value 已返回）
}
```

**关键点**：`compute_ro_value` 签名是 `&Context`（不可变），它在内部调用
`eval_expr_int`（读 ctx.expr_values，`&self`）。compute_ro_value 返回后，
不可变借用释放，随后 `set_field_at` 获取 `&mut ctx`。**无冲突**（顺序借用）。

#### 嵌套 StructRef（child_ctx）

StructRefNode 创建独立的 `child_ctx`（`Context::new_child`），child_ctx 有自己的
`fields` 和 `expr_values`。child_ctx 的借用与父 ctx 完全隔离。**无冲突**。

#### 结论

所有路径中 `&mut ctx`（set_field_at / init_expr_values）与 `&ctx`（compute_ro_value /
get_int_by_index / eval_expr_int）均为**顺序借用**，不存在同时持有可变与不可变
借用的情况。借用安全。

---

## 3. Layer 2：固定开销优化（视实测决定）

> 仅当 Layer 1 实测后 E1 parse / E3 build 仍 <10x 时启用。

### 3.1 固定开销拆解（B7 空 Struct parse = 314ns）

| 组件 | 估算 ns | 来源 | 可优化 |
|------|--------:|------|:---:|
| `_parse_raw` pyo3 `#[pymethods]` wrapper | 30-50 | GIL 验证 + 参数解析 + 返回转换 | 否 |
| `Context::new_root`（`PyDict::new_bound`） | 30-50 | `PyDict_New` 分配 dict 对象 | 否 |
| `ParseStream::new` | 2 | 切片 + usize | 否 |
| `StructNode.parse` 空循环 | 5 | 迭代开销 | 否 |
| `create_class`（`tp_new`） | 40-60 | `PyType_GetSlot` + `newfunc` 调用 | 否 |
| `force_setattr`（`GenericSetAttr`） | 20-30 | `PyObject_GenericSetAttr` + attr_name 创建 | **是** |
| dict `clone_ref` → instance | 3-5 | `Bound::clone`（Py_INCREF）+ 转换 | **是** |
| `Path::new` | 1 | 空字符串 | 否 |

### 3.2 可优化项详细设计

#### P2：dict clone → take（~3-5ns）

**当前**（struct_node.rs:324-332）：
```rust
ctx.fields()?
    .clone()        // Py_INCREF dict
    .into_any()     // 零开销类型转换
    .unbind()       // Py<PyAny>，无额外 incref
```

**优化后**：Context 新增 `take_fields` 方法，移出 dict 所有权（避免 clone 的 incref）：
```rust
/// 取出 fields dict 的所有权（parse 末尾将 dict 移交给实例）。
///
/// 调用后 `self.fields = None`。仅在 parse/build 结束、ctx 不再使用时调用。
pub fn take_fields(&mut self) -> Option<Bound<'py, PyDict>> {
    self.fields.take()
}
```

StructNode.parse 末尾：
```rust
let dict_for_instance = ctx.take_fields()
    .ok_or_else(|| ConstructError::ExprContext { ... })?
    .into_any().unbind();
```

**安全性**：`_parse_raw` 中 `ctx` 在 `root.parse(...)` 返回后不再使用，
`take` 不会导致后续访问悬垂。需在 StructRef 的 child_ctx 路径验证 child_ctx
也在使用后 take（当前 child_ctx 在 StructRefNode.parse 内创建并随函数返回 drop，
take 安全）。

#### P3：interned "__dict__" 缓存（~3-5ns）

**当前**（instance.rs:131）：`force_setattr` 内部 `"__dict__".into_py(py)` 每次
创建新 PyString（~5ns，含 PyUnicode_FromString + hash 初始化）。

**优化后**：StructNode 编译期预计算 interned `"__dict__"` PyString，存为字段：
```rust
pub struct StructNode {
    // ... 现有字段 ...
    /// interned "__dict__"（force_setattr 复用，避免每次 parse 创建临时 str）。
    dict_attr_name: Py<PyString>,
}
```

force_setattr 改为接收 `&Py<PyString>` 而非 `impl IntoPy`：
```rust
pub fn force_setattr_ptr(
    py: Python<'_>,
    obj: &Bound<'_, PyAny>,
    attr_name: &Py<PyString>,
    value: &Bound<'_, PyAny>,
) -> PyResult<()>
```

#### P4：force_setattr 内联简化（~5-10ns）

当前 force_setattr 的 `into_py` 转换有 pyo3 wrapper 开销。直接用 ffi：
```rust
pub fn force_setattr_ptr(...) -> PyResult<()> {
    let result = unsafe {
        ffi::PyObject_GenericSetAttr(
            obj.as_ptr(),
            attr_name.as_ptr(),   // 直接用缓存指针，跳过 into_py
            value.as_ptr(),
        )
    };
    if result == -1 { Err(PyErr::fetch(py)) } else { Ok(()) }
}
```

省去两次 `into_py`（attr_name + value 的 Py<PyAny> 构造）。

---

## 4. 影响面分析（逐文件）

### 4.1 `context.rs`（核心改动）

| 改动类型 | 内容 |
|---------|------|
| **移除字段** | `field_names_owned`, `field_names_ptr`, `field_names_len` |
| **新增字段** | `expr_values: Option<Vec<*mut ffi::PyObject>>` |
| **修改构造** | `new_root`/`placeholder`/`new_child` 初始化 `expr_values: None`，移除 field_names_* 初始化 |
| **移除方法** | `set_field_names`, `set_field_names_ref`, `field_names`, `get_int_by_name`, `get_obj_by_name` |
| **新增方法** | `init_expr_values(&mut self, n)`, `set_field_at(&mut self, idx, name, value, py)`, `get_int_by_index(&self, idx, py)` |
| **移除方法** | `set_field_interned`（被 `set_field_at` 替代） |
| **修改方法** | `take_fields(&mut self)`（Layer 2，可选） |
| **保留方法** | `set_field`(`&self`), `get_field`, `parent`, `fields` |
| **测试** | 移除 field_names 相关测试；新增 expr_values/set_field_at/get_int_by_index 测试；set_field_interned 测试改为 set_field_at |

### 4.2 `expr.rs`

| 改动类型 | 内容 |
|---------|------|
| **修改签名** | `eval_expr_int(program, ctx, py)` — 移除 `names` 参数 |
| **修改 GetInt** | `ctx.get_int_by_index(*idx, py)?` 替代 `names.get` + `get_int_by_name` |
| **文档** | 更新 GetInt 文档注释（移除 names[idx] 描述，改为 ctx.expr_values[idx]） |
| **测试** | 所有 `eval_expr_int(&prog, &names, &ctx, py)` 调用改为 `eval_expr_int(&prog, &ctx, py)`；make_context 辅助改用 `init_expr_values` + `set_field_at`；移除 `context_get_int_by_name_*` 测试，新增 `context_get_int_by_index_*` 测试 |

### 4.3 `nodes/struct_node.rs`

| 改动类型 | 内容 |
|---------|------|
| **parse** | `ctx.set_field_names_ref(...)` → `ctx.init_expr_values(self.fields.len())`；`for field` → `for (idx, field) in enumerate()`；`ctx.set_field_interned(name, value, py)` → `ctx.set_field_at(idx, name, value, py)` |
| **build** | 同 parse 的 init + set_field_at 改动；RO 路径 `compute_ro_value` 后 `set_field_at(idx, ...)` |
| **dict移交** | `ctx.fields()?.clone()...` → `ctx.take_fields()?...`（Layer 2） |
| **移除字段** | `field_names_cache: Vec<Py<PyString>>`（Vec 化后无引用方）；`StructNode::new` 移除预计算逻辑 |
| **测试** | setup_ctx 辅助改用新 API；context 相关断言适配 |

### 4.4 `nodes/bytes.rs`

| 改动类型 | 内容 |
|---------|------|
| **parse** | 移除 `let names = ctx.field_names().ok_or_else(...)?`；`eval_expr_int(prog, names, ctx, py)` → `eval_expr_int(prog, ctx, py)` |
| **build** | 同 parse 的 eval_expr_int 调用改动 |
| **测试** | `setup_ctx_for_expr` 辅助改用 `init_expr_values` + `set_field_at` |

### 4.5 `nodes/computed.rs`

| 改动类型 | 内容 |
|---------|------|
| **parse** | 移除 `ctx.field_names()`；`eval_expr_int(&self.expr, names, ctx, py)` → `eval_expr_int(&self.expr, ctx, py)` |
| **测试** | `make_context` 辅助改用新 API |

### 4.6 `nodes/mod.rs`

| 改动类型 | 内容 |
|---------|------|
| **compute_ro_value** | 移除 `let names = ctx.field_names().ok_or(...)?`；`eval_expr_int(c.expr(), names, ctx, py)` → `eval_expr_int(c.expr(), ctx, py)` |

### 4.7 `nodes/struct_ref.rs`

| 改动类型 | 内容 |
|---------|------|
| **无逻辑改动** | child_ctx 由 `new_child` 创建，StructNode 内部自行 `init_expr_values`。StructRef 仅传递 ctx，不直接操作 expr_values |
| **验证** | 确认 child_ctx 的 expr_values 隔离正确（各层独立 init） |

### 4.8 `schema.rs`

| 改动类型 | 内容 |
|---------|------|
| **无改动** | `_parse_raw`/`_build_raw` 创建 Context（new_root/placeholder），不涉及 expr_values API。Context 内部变化对外透明 |

### 4.9 `instance.rs`（Layer 2）

| 改动类型 | 内容 |
|---------|------|
| **新增** | `force_setattr_ptr`（接收 `&Py<PyString>` 而非 `impl IntoPy`） |
| **保留** | `force_setattr`（兼容现有调用，或标记 deprecated） |

### 4.10 `compile.rs` / `lib.rs`

| 改动类型 | 内容 |
|---------|------|
| **lib.rs** | `pub use expr::{eval_expr_int, ...}` 签名变化对外透明（eval_expr_int 非 pub API 暴露给 Python） |
| **compile.rs** | 无改动（编译期不涉及 Context 运行时 API） |

### 4.11 改动量汇总

| 文件 | 改动行数（估） | 风险 |
|------|:---:|:---:|
| context.rs | ~150（核心重写） | 中 |
| expr.rs | ~30（签名 + GetInt） | 低 |
| struct_node.rs | ~40（parse/build + 移除 cache） | 中 |
| bytes.rs | ~10 | 低 |
| computed.rs | ~5 | 低 |
| mod.rs | ~5 | 低 |
| instance.rs | ~20（Layer 2） | 低 |
| 测试（各文件） | ~200（API 适配） | 低 |

---

## 5. Vec 后性能预估

### 5.1 Layer 1（Vec 化 + field_names 清理）

**节省模型**：
- 每个 GetInt 节省 ~30ns（PyDict_GetItem 24 + from_borrowed_ptr 2 + extract wrapper 7 - Vec索引 1 - PyLong_AsLongLong 5 ≈ 27-30ns）
- 每个 eval_expr_int 调用节省 ~10ns（field_names() 调用 8 + names 参数传递 2）

| 场景 | 方向 | GetInt数 | eval调用数 | 当前ns | Vec节省 | field_names节省 | **预估ns** | py ns | **预估比率** | ≥10x? |
|------|------|:---:|:---:|---:|---:|---:|---:|---:|:---:|:---:|
| E1 | parse | 1 | 1 | 353 | 30 | 10 | **313** | 2724 | **8.7x** | ⚠️ |
| E1 | build | 1 | 1 | 271 | 30 | 10 | **231** | 2655 | **11.5x** | ✅ |
| E2 | parse | 2 | 2 | 391 | 60 | 20 | **311** | 3602 | **11.6x** | ✅ |
| E2 | build | 2 | 2 | 317 | 60 | 20 | **237** | 3449 | **14.6x** | ✅ |
| E3 | parse | 3 | 3 | 572 | 90 | 30 | **452** | 4682 | **10.4x** | ✅ |
| E3 | build | 3 | 3 | 541 | 90 | 30 | **421** | 4271 | **10.1x** | ✅ |

> ⚠️ **E1 parse 预估 8.7x，可能不达标**。这是 Layer 1 的主要风险点。
> 其余 5 项预估达标。

### 5.2 Layer 1 + Layer 2（叠加固定开销优化）

Layer 2 预估节省：~15-25ns（dict take 5 + interned "__dict__" 5 + force_setattr 内联 10）。

| 场景 | 方向 | Layer1预估ns | Layer2节省 | **总预估ns** | **总预估比率** | ≥10x? |
|------|------|---:|---:|---:|:---:|:---:|
| E1 | parse | 313 | 20 | **293** | **9.3x** | ⚠️ |
| E1 | build | 231 | 20 | **211** | **12.6x** | ✅ |
| E2 | parse | 311 | 20 | **291** | **12.4x** | ✅ |
| E2 | build | 237 | 20 | **217** | **15.9x** | ✅ |
| E3 | parse | 452 | 20 | **432** | **10.8x** | ✅ |
| E3 | build | 421 | 20 | **401** | **10.7x** | ✅ |

> ⚠️ **即使叠加 Layer 2，E1 parse 预估 9.3x，仍可能不达标**。
> E1 parse 是字段数最少（2字段 + 1 GetInt）的场景，固定开销占比最大，
> 而 Python 侧绝对值也最低（2724ns），导致 10x 门槛对应的 Rust 绝对值极小（272ns）。

### 5.3 E1 parse 达标分析

E1 parse 达标 10x 需要 Rust ≤ 272ns。当前 353ns，需减 **81ns**。

| 优化项 | 节省 | 累计 | 剩余缺口 |
|--------|---:|---:|---:|
| Layer 1 Vec + field_names | 40 | 40 | 41 |
| Layer 2 固定开销 | 20 | 60 | 21 |
| **缺口** | | | **~21ns** |

这 21ns 可能的来源：
1. **预估保守**：实际 GetInt 节省可能 >30ns（PM 测得 29.2ns 是 PyDict_GetItem+AsLongLong，
   未计入 names.get/bind/from_borrowed_ptr/extract wrapper 的附加开销 ~7ns）。
   若实际节省 37ns/GetInt，则 Layer 1 即可覆盖。
2. **基准测试噪声**：±5% 波动。272ns 的 5% = 14ns。若 E1 parse 实测落在 258-286ns，
   多次测量中位数可能 <272ns。
3. **编译优化**：Vec 化后代码路径变短，指令缓存命中更好，可能有超线性收益。

### 5.4 实施策略建议

```
Step 1: 实现 Layer 1（P0 Vec 化 + P1 field_names 清理）
  ↓
Step 2: 实测 E1-E3 全部场景
  ↓
Step 3: 评估
  ├─ 全部 ≥10x → 完成
  ├─ E1 parse 9-10x，其余达标 → 实现 Layer 2，重新实测
  └─ 多项 <10x → 回到设计（诊断新瓶颈）
```

**不建议**在未实测前一次性实现 Layer 1 + Layer 2——Layer 2 的收益不确定，
先验证 Layer 1 的实际效果再决定。

---

## 6. 边界条件清单

### 6.1 Vec 初始化与 WO 字段

- **场景**：Struct 有 WO 字段（如 magic header），WO 字段的 Vec[idx] 保持 null。
- **预期**：编译期保证 GetInt 不引用 WO 字段。运行时 get_int_by_index 访问 null
  返回 `ExprFieldMissing`（防御性，理论不触发）。
- **测试**：构造含 WO 字段的 Struct，验证 GetInt 仅访问 RW/RO 字段。

### 6.2 占位 context（placeholder）

- **场景**：无表达式的 Struct，ctx 为 placeholder（fields=None, expr_values=None）。
- **预期**：不调用 init_expr_values / set_field_at / get_int_by_index。
  eval_expr_int 不被调用（无表达式）。
- **测试**：placeholder 上调用 get_int_by_index 返回 ExprContext 错误。

### 6.3 嵌套 StructRef

- **场景**：外层 Struct 含 StructRef 字段，内层有表达式。
- **预期**：StructRefNode 创建 child_ctx（new_child），内层 StructNode.parse 在
  child_ctx 上 init_expr_values。child_ctx 的 expr_values 独立于外层。
- **测试**：嵌套结构 parse/build，验证内层表达式正确求值。

### 6.4 idx 越界

- **场景**：GetInt(5) 但 expr_values 只有 3 个槽位。
- **预期**：编译期保证 idx < fields.len() == expr_values.len()。
  运行时 `vals.get(idx)` 返回 None → `unwrap_or(null)` → ExprFieldMissing。
- **测试**：构造越界 GetInt，验证返回错误而非 panic。

### 6.5 PyLong_AsLongLong 溢出

- **场景**：字段值为超大整数（超出 i64 范围）。
- **预期**：PyLong_AsLongLong 返回 -1 并设置 OverflowError。
  get_int_by_index 检测 PyErr_Occurred，返回 ExprType 错误。
- **测试**：存入 `2**63 + 1`，验证 get_int_by_index 返回 ExprType。

### 6.6 值为 -1（AsLongLong 歧义）

- **场景**：字段值恰好为 -1，PyLong_AsLongLong 返回 -1。
- **预期**：检查 PyErr_Occurred 为 null（无错误），确认 -1 是合法值。
- **测试**：存入 -1，验证 get_int_by_index 返回 Ok(-1)。

### 6.7 非整数字段值

- **场景**：字段值为 bytes/str（非 PyLong）。
- **预期**：PyLong_AsLongLong 失败（返回 -1 + TypeError），get_int_by_index
  返回 ExprType 错误。
- **测试**：存入 str 值，验证 ExprType 错误。

### 6.8 take_fields 后访问（Layer 2）

- **场景**：take_fields 后再次访问 ctx.fields()。
- **预期**：返回 None（fields 已 take）。生产路径中 parse 返回后不再访问 ctx，
  理论不触发。防御性返回 None 而非 panic。
- **测试**：take_fields 后 fields() 返回 None。

---

## 7. 与 Python 版本对应

本优化为纯 Rust 内部实现优化，**不改变任何对外 API 或 Python 行为**。
Python 侧的 `this.xxx` 表达式语义、Container/字段访问行为完全不变。

| Python construct | 优化前 Rust | 优化后 Rust |
|-----------------|------------|------------|
| `context[sc.name] = subobj` | `ctx.set_field_interned(name, value)` | `ctx.set_field_at(idx, name, value)` |
| `this.xxx`（表达式求值取值） | `PyDict_GetItem` hash 查找 | `Vec[idx]` 数组索引 |
| `_`（父 context 访问） | `ctx.parent()` | 不变 |

---

## 8. 风险与缓解

### 8.1 E1 parse 可能不达标（主要风险）

**风险**：Layer 1 + Layer 2 后 E1 parse 仍 9-9.5x。

**缓解**：
1. 先实现 Layer 1 实测，获取精确数据。
2. 若差 5-10ns，检查是否有未识别的微优化（如 eval_expr_int 的栈数组初始化、
   函数内联标记、分支预测提示）。
3. 若差 >15ns，需重新诊断固定开销组成（可能 pyo3 wrapper 占比超预期），
   考虑架构层面调整（超出本设计范围，需 PM 协调）。

### 8.2 raw pointer 的 unsafe

**风险**：expr_values 存 borrowed 指针，误用可能导致 use-after-free。

**缓解**：
1. 指针仅在 `set_field_at`（写）和 `get_int_by_index`（读）中访问，封装在 Context 内。
2. SAFETY 注释完整论证 5 条不变量（§2.1）。
3. Context 不跨线程发送（pyo3 保证），单线程 GIL 下访问。
4. 单元测试覆盖 null 槽位、越界、非整数等边界。

### 8.3 测试改动量大

**风险**：~200 行测试需适配新 API（eval_expr_int 签名、set_field_at、移除 field_names）。

**缓解**：
1. 提供测试辅助函数迁移指南（make_context → init_expr_values + set_field_at）。
2. DEV 实现时先改测试辅助，再批量替换调用点。

---

## 附录 A：i64 提前缓存方案对比

**方案**：`expr_values: Vec<Option<i64>>`，set_field 时提前 extract i64 缓存。

| 维度 | raw pointer | i64 提前缓存 |
|------|------------|-------------|
| GetInt 开销 | ~6ns（Vec索引 + AsLongLong） | ~1ns（Vec索引） |
| set_field 额外开销 | 0 | +5ns/字段（extract） |
| 非整数字段处理 | 自然支持（AsLongLong 报错） | extract 失败需特殊处理 |
| E1 净收益（2字段,1GI） | 30ns | 30 - 5×1(未被引用的字段) = 25ns |
| E3 净收益（4字段,3GI） | 90ns | 99 - 5×1 = 94ns |

**结论**：raw pointer 在所有场景净收益 ≥ i64 缓存（因 set_field 的 extract
对不被 GetInt 引用的字段是纯浪费）。采用 raw pointer。

## 附录 B：get_obj_by_name 移除说明

`get_obj_by_name` 当前定义于 Context 但**无生产调用方**（Phase 2 未实现 Switch 节点）。
本优化移除该方法。Phase 3 实现 Switch 时，selector 表达式求值需要取 Python 对象
（非整数），届时重新设计——可基于 expr_values 的 raw pointer 取对象（`Py::<PyAny>::from_borrowed_ptr`），
或引入独立的 `get_obj_by_index` 方法。

移除不影响当前任何功能。

---

## 设计完成状态

- [x] 优化方案总览（含收益排序）
- [x] Context Vec 化详细设计（struct + 方法签名 + eval_expr_int 新签名 + 借用分析）
- [x] 固定开销分析（拆解 + 可优化项 + Layer 2 设计）
- [x] 影响面分析（逐文件 11 个文件）
- [x] Vec 后性能预估（Layer 1 / Layer 1+2 两版）
- [x] 边界条件清单（8 个场景）
- [x] 风险与缓解

**需要 PM 决策的点**：
1. **E1 parse 达标风险**：预估 Layer 1+2 后 9.3x，可能不达 10x。是否接受"≥9.5x 视为达标"，
   还是要求必须 ≥10x（若后者，需预留 Phase 2.5+ 进一步优化的空间）？
2. **Layer 2 实施时机**：建议 Layer 1 实测后再决定是否启用 Layer 2。PM 确认此分阶段策略。
3. **field_names_cache 移除**：确认移除 StructNode.field_names_cache 不影响后续 Phase
   （Phase 3+ 的 Prefixed/Pointer 等节点是否依赖字段名列表？经核查当前代码无依赖）。

