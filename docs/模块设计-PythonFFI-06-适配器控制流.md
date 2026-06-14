# 模块设计：PythonFFI-06 适配器 + 控制流 Python 包装层

> 子任务 10.7。覆盖适配器（Adapter/Validator/Enum/Hex 等）、控制流
> （If/Switch/Check/StopIf 等）、函数式宏（OneOf/NoneOf/Filter/Slicing/
> Indexing/Optional）、以及纯 Python 实现的构造器（NamedTuple/
> TimestampAdapter）。

## 模块位置

- `construct-py/src/constructs_adapter.rs` — Rust 内建适配器/控制流的 PyO3 包装（**本子任务新增**）
- `construct-py/src/constructs_adapter.rs` 内的 `py_*` 工厂函数 — 函数式构造器（OneOf/NoneOf/Filter/Optional/Slicing/Indexing）
- `construct-py/src/py_adapter.rs` — PyConstructAdapter 混合架构桥接器（10.4 已实现，10.7 增强 `extract_subcon`）
- `construct-py/construct_rust/_adapter.py` — 纯 Python 基类（10.4 已实现，10.7 审查 + 补充 NamedTuple/Timestamp）
- `construct-py/construct_rust/_namedtuple.py` — NamedTuple 纯 Python 实现（**本子任务新增**）
- `construct-py/src/lib.rs` — 模块注册（新增 `constructs_adapter` 模块 + 注册新的 pyfunction）

## 职责

将 Rust 内核（construct-rs）中已实现的适配器与控制流构造器，通过 PyO3 `#[pyclass]` 包装暴露给 Python 用户；同时提供依赖 Python 标准库（`collections.namedtuple`）或第三方库（`arrow`）的构造器作为纯 Python 实现。最终使 Python 用户能以与原版 construct 库**完全一致**的声明式语法使用这些构造器，并支持用户继承 `Adapter`/`Validator` 创建自定义构造器（混合架构）。

---

## 1. 核心设计决策

### 1.1 三层架构总览

10.7 的实现分为三层，每层对应不同的实现策略：

```
┌─────────────────────────────────────────────────────────────────┐
│ 层 1：Rust 内建构造器的 PyO3 包装（constructs_adapter.rs）       │
│   - Adapter / ExprAdapter / SymmetricAdapter / ExprSymmetricAdapter │
│   - Validator / ExprValidator                                    │
│   - Enum / FlagsEnum / Mapping                                   │
│   - Hex / HexDump                                                │
│   - If / IfThenElse / Switch / Check / StopIf / Optional         │
│   每个 #[pyclass] 持有 Rust owned 实例，零开销调用 Rust 实现    │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│ 层 2：函数式构造器（py_* 工厂函数，返回层 1 的 #[pyclass]）      │
│   - OneOf(subcon, valids) → ExprValidator                        │
│   - NoneOf(subcon, invalids) → ExprValidator                     │
│   - Filter(predicate, subcon) → ExprSymmetricAdapter             │
│   - Slicing(subcon, count, start, stop, step, empty) → Adapter   │
│   - Indexing(subcon, count, index, empty) → Adapter              │
│   - Optional(subcon) → If                                        │
│   这些在 Python 原版中是普通函数（非类），返回 Adapter/Validator │
│   实例。本模块用 Rust 工厂函数 + PyClosure 实现相同语义          │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│ 层 3：纯 Python 实现（construct_rust/_namedtuple.py 等）         │
│   - NamedTuple（依赖 collections.namedtuple，Python 专属）       │
│   - TimestampAdapter / Timestamp（依赖 arrow 库，Phase 10 stub） │
│   - 用户自定义 Adapter/Validator 子类（继承 _adapter.py 基类）   │
│   这些构造器无法用 Rust 合理实现，作为纯 Python 类提供            │
└─────────────────────────────────────────────────────────────────┘
```

### 1.2 实现策略分类（PyO3 包装 vs 纯 Python）

每个构造器按以下决策树选择实现策略：

| 构造器 | 策略 | 理由 |
|--------|------|------|
| `Adapter(subcon, decoder, encoder)` | **PyO3 包装** | Rust `adapters::Adapter` 已实现，decoder/encoder 通过 PyClosure 桥接 |
| `SymmetricAdapter(subcon, func)` | **PyO3 包装** | Rust `adapters::SymmetricAdapter` 已实现 |
| `ExprAdapter(subcon, decoder, encoder)` | **PyO3 包装** | Rust `adapters::ExprAdapter` 已实现（语义别名） |
| `ExprSymmetricAdapter(subcon, func)` | **PyO3 包装** | 复用 Rust `SymmetricAdapter`（Python 原版中也是 ExprAdapter 子类） |
| `Validator(subcon, func)` | **PyO3 包装** | Rust `adapters::Validator` 已实现 |
| `ExprValidator(subcon, validator)` | **PyO3 包装** | Rust `adapters::ExprValidator` 已实现 |
| `Enum(subcon, **kw)` | **PyO3 包装** | Rust `enum_::Enum` 已实现 |
| `FlagsEnum(subcon, **kw)` | **PyO3 包装** | Rust `enum_::FlagsEnum` 已实现 |
| `Mapping(subcon, mapping)` | **PyO3 包装** | Rust `enum_::Mapping` 已实现 |
| `Hex(subcon)` | **PyO3 包装** | Rust `hex::Hex` 已实现（pass-through，显示语义在 Python 侧） |
| `HexDump(subcon)` | **PyO3 包装** | Rust `hex::HexDump` 已实现 |
| `If(condfunc, subcon)` | **PyO3 包装** | Rust `control_flow::If()` 已实现（= IfThenElse + Pass） |
| `IfThenElse(condfunc, then, else)` | **PyO3 包装** | Rust `control_flow::IfThenElse` 已实现 |
| `Switch(keyfunc, cases)` | **PyO3 包装** | Rust `control_flow::Switch` 已实现 |
| `Check(condfunc)` | **PyO3 包装** | Rust `control_flow::Check` 已实现 |
| `StopIf(condfunc)` | **PyO3 包装** | Rust `control_flow::StopIf` 已实现 |
| `Optional(subcon)` | **函数式（PyO3）** | Python 原版是函数，返回 `IfThenElse(..., subcon, Pass)` |
| `OneOf(subcon, valids)` | **函数式（PyO3）** | Python 原版是函数，返回 `ExprValidator` |
| `NoneOf(subcon, invalids)` | **函数式（PyO3）** | 同上 |
| `Filter(predicate, subcon)` | **函数式（PyO3）** | Python 原版是函数，返回 `ExprSymmetricAdapter` |
| `Slicing(subcon, ...)` | **函数式（PyO3）** | Python 原版是 Adapter 子类，用 Rust Adapter + 闭包实现 |
| `Indexing(subcon, ...)` | **函数式（PyO3）** | 同上 |
| `NamedTuple(name, fields, subcon)` | **纯 Python** | 依赖 `collections.namedtuple`（Python 标准库工厂），返回 Python namedtuple 实例 |
| `Timestamp(subcon, unit, epoch)` | **纯 Python stub** | 依赖 `arrow` 第三方库；Phase 10 不安装 arrow，导出 stub 在导入时抛 ImportError |

### 1.3 Check / StopIf 在 Struct/Sequence 中的生效路径（内核完备性确认）

**关键确认**：Rust 内核已完备实现 `StopField` 错误的捕获与处理。

| 复合构造器 | StopField 处理 | 源码位置 |
|-----------|---------------|---------|
| `Struct` | parse: `Err(StopField) => break`（停止后续字段） | `struct_.rs:225` |
| `Struct` | build: `Err(StopField) => return Ok(())` | `struct_.rs:296` |
| `Sequence` | parse: `Err(StopField) => break` | `sequence.rs:214` |
| `Sequence` | build: `Err(StopField) => return Ok(())` | `sequence.rs:272` |
| `GreedyRange` | parse: `Err(StopField) => 停止循环` | `repetition.rs:384` |
| `RepeatUntil` | （通过 GreedyRange 机制） | `repetition.rs` |
| `FocusedSeq` | parse: `Err(StopField) => break` | `focused_seq.rs:120` |

**结论**：`Check` 和 `StopIf` 通过 PyO3 包装后，直接调用 Rust 内核的 `Check::parse`/`StopIf::parse`，产生的 `ConstructError::Check`/`ConstructError::StopField` 会被外层 Struct/Sequence 正确捕获。**无需在 PyO3 层做任何特殊处理**。

### 1.4 PyClosure 双参数桥接（obj, context）

Rust 内核的 Adapter/Validator 使用 `DecodeFunc`/`EncodeFunc`/`CheckFunc` 类型别名，签名为 `Fn(&Value, &Context) -> Result<...>`（**两个参数**：值 + 上下文）。

这与 10.4 设计的 `py_to_cond_func`（单参数 `(context)`）不同。Adapter 的 decode/encode 函数需要同时访问**被解码的值**和**上下文**。

Python 原版的调用约定：
```python
# Adapter._decode(self, obj, context, path)
# ExprAdapter 的 decoder lambda 签名：decoder(obj, ctx)
d = ExprAdapter(Byte, obj_ + 1, obj_ - 1)
# decoder = lambda obj, ctx: obj + 1
```

因此，10.7 需要在 `expr_bridge.rs` 中新增**双参数 PyClosure 桥接器**：

```rust
/// 将 Python callable 桥接为 DecodeFunc（Fn(&Value, &Context) -> Result<Value>）。
///
/// Python 调用约定：callable(obj, context) — 双参数。
pub fn py_to_decode_func(callable: PyObject) -> DecodeFunc {
    Box::new(move |obj: &Value, ctx: &Context| {
        Python::with_gil(|py| {
            let py_obj = crate::conversions::value_to_py(py, obj)?;
            let py_ctx = crate::conversions::context_to_py_container(py, ctx)?;
            let result = callable
                .call1(py, (py_obj.bind(py), py_ctx.bind(py)))
                .map_err(|e| ConstructError::Generic {
                    path: String::new(),
                    message: format!("Adapter decoder error: {}", e),
                })?;
            crate::conversions::py_to_value(py, result.bind(py))
        })
    })
}
```

`EncodeFunc` 和 `CheckFunc`（Adapter 版本，非 control_flow 版本）的桥接逻辑相同。

**注意命名冲突**：`control_flow::CheckFunc` 是 `Fn(&Context) -> Result<()>`（单参数），`adapters::CheckFunc` 是 `Fn(&Value, &Context) -> Result<()>`（双参数）。两者是不同的类型别名，桥接器需要分别实现。

---

## 2. Rust 内建适配器 PyO3 包装

本节定义 `construct-py/src/constructs_adapter.rs` 中各 `#[pyclass]` 包装类的接口。所有包装类遵循 10.5/10.6 建立的模式：
- 实现 `PyConstructWrapper` trait（提供 `as_construct()`）
- 使用 `impl_api_methods!(PyXxx)` 宏生成 7 个公共 API 方法
- 使用 `impl_construct_operators!(PyXxx)` 宏生成 6 个运算符 dunder 方法（`/` `*` `+` `>>` `[]`）

### 2.1 Adapter / ExprAdapter

Python 原版 `Adapter` 是抽象基类（用户继承后实现 `_decode`/`_encode`）。但 `ExprAdapter(subcon, decoder, encoder)` 是**具体类**，接受 decoder/encoder lambda。

在 Rust 侧，`construct::constructs::adapters::Adapter` 是具体的（接受 `DecodeFunc` + `EncodeFunc` 闭包）。PyO3 包装类 `PyAdapter` 对应 Python 的 `ExprAdapter`（函数式 API）。

**注意**：Python 用户继承的抽象 `Adapter` 基类由纯 Python `_adapter.py` 提供（§5.3），不是 PyO3 包装。

```rust
/// PyO3 包装：双向值适配器（对应 Python ExprAdapter）。
///
/// 接受 decoder/encoder lambda，在 parse/build 时对值进行转换。
#[pyclass(name = "ExprAdapter")]
pub struct PyExprAdapter {
    pub(crate) inner: construct::constructs::adapters::Adapter,
}

impl PyConstructWrapper for PyExprAdapter {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "ExprAdapter", signature = (subcon, decoder, encoder))]
pub fn py_expr_adapter(
    py: Python<'_>,
    subcon: &Bound<PyAny>,
    decoder: &Bound<PyAny>,
    encoder: &Bound<PyAny>,
) -> PyResult<PyExprAdapter> {
    let sc = extract_subcon(subcon)?;
    let decode = py_to_decode_func(decoder.clone().unbind())?;
    let encode = py_to_encode_func(encoder.clone().unbind())?;
    Ok(PyExprAdapter {
        inner: construct::constructs::adapters::Adapter::new(sc, decode, encode),
    })
}

impl_api_methods!(PyExprAdapter);
impl_construct_operators!(PyExprAdapter);
```

**decoder/encoder lambda 签名**：

| 参数位置 | Python 原版 | 本模块 | 说明 |
|---------|------------|--------|------|
| decoder | `lambda obj, ctx: ...` | 同 | 接收 (raw_value, context)，返回 transformed_value |
| encoder | `lambda obj, ctx: ...` | 同 | 接收 (user_value, context)，返回 raw_value |

**obj_ 表达式在 decoder 中的行为**：Python `ExprAdapter(Byte, obj_ + 1, obj_ - 1)` 中，`obj_` 是 `Path("obj_")`，其 `__call__(obj, *args)` 返回第一个参数（即 raw value）。通过 `py_to_decode_func` 的双参数调用 `decoder(raw_value, context)`，`obj_(raw_value)` 返回 `raw_value`。✓

### 2.2 SymmetricAdapter / ExprSymmetricAdapter

```rust
/// PyO3 包装：对称适配器（decode == encode）。
#[pyclass(name = "SymmetricAdapter")]
pub struct PySymmetricAdapter {
    pub(crate) inner: construct::constructs::adapters::SymmetricAdapter,
}

impl PyConstructWrapper for PySymmetricAdapter {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "SymmetricAdapter", signature = (subcon, func))]
pub fn py_symmetric_adapter(
    py: Python<'_>,
    subcon: &Bound<PyAny>,
    func: &Bound<PyAny>,
) -> PyResult<PySymmetricAdapter> {
    let sc = extract_subcon(subcon)?;
    let symmetric = py_to_symmetric_func(func.clone().unbind())?;
    Ok(PySymmetricAdapter {
        inner: construct::constructs::adapters::SymmetricAdapter::new(sc, symmetric),
    })
}

/// PyO3 包装：ExprSymmetricAdapter（SymmetricAdapter 的语义别名）。
///
/// Python 原版中 ExprSymmetricAdapter 是 ExprAdapter 的子类，
/// 但语义上与 SymmetricAdapter 等价（decoder == encoder）。
/// 本模块直接复用 Rust SymmetricAdapter。
#[pyclass(name = "ExprSymmetricAdapter")]
pub struct PyExprSymmetricAdapter {
    pub(crate) inner: construct::constructs::adapters::SymmetricAdapter,
}

impl PyConstructWrapper for PyExprSymmetricAdapter {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "ExprSymmetricAdapter", signature = (subcon, func))]
pub fn py_expr_symmetric_adapter(
    py: Python<'_>,
    subcon: &Bound<PyAny>,
    func: &Bound<PyAny>,
) -> PyResult<PyExprSymmetricAdapter> {
    // 与 SymmetricAdapter 完全相同，仅 Python 类型名不同
    let sc = extract_subcon(subcon)?;
    let symmetric = py_to_symmetric_func(func.clone().unbind())?;
    Ok(PyExprSymmetricAdapter {
        inner: construct::constructs::adapters::SymmetricAdapter::new(sc, symmetric),
    })
}
```

**注意**：`ExprSymmetricAdapter` 和 `SymmetricAdapter` 在 Rust 侧是同一个类型，但 Python 侧需要两个不同的类型对象（`isinstance(x, ExprSymmetricAdapter)` 检查）。因此定义两个独立的 `#[pyclass]`。

### 2.3 Validator / ExprValidator

```rust
/// PyO3 包装：验证器（值通过验证后不变换）。
#[pyclass(name = "Validator")]
pub struct PyValidator {
    pub(crate) inner: construct::constructs::adapters::Validator,
}

impl PyConstructWrapper for PyValidator {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "Validator", signature = (subcon, func))]
pub fn py_validator(
    py: Python<'_>,
    subcon: &Bound<PyAny>,
    func: &Bound<PyAny>,
) -> PyResult<PyValidator> {
    let sc = extract_subcon(subcon)?;
    // func 是 validatorfunc(obj, ctx) -> bool
    // 桥接为 adapters::CheckFunc（双参数）
    let check = py_to_adapter_check_func(func.clone().unbind())?;
    Ok(PyValidator {
        inner: construct::constructs::adapters::Validator::new(sc, check),
    })
}

/// PyO3 包装：ExprValidator（Validator 的语义别名，接受 lambda）。
#[pyclass(name = "ExprValidator")]
pub struct PyExprValidator {
    pub(crate) inner: construct::constructs::adapters::ExprValidator,
}

impl PyConstructWrapper for PyExprValidator {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "ExprValidator", signature = (subcon, validator))]
pub fn py_expr_validator(
    py: Python<'_>,
    subcon: &Bound<PyAny>,
    validator: &Bound<PyAny>,
) -> PyResult<PyExprValidator> {
    let sc = extract_subcon(subcon)?;
    let check = py_to_adapter_check_func(validator.clone().unbind())?;
    Ok(PyExprValidator {
        inner: construct::constructs::adapters::ExprValidator::new(sc, check),
    })
}
```

**validator lambda 语义**：`ExprValidator(Byte, obj_ & 0b11111110 == 0)` 中，validator 返回 Python bool。`py_to_adapter_check_func` 将 truthy → `Ok(())`，falsy → `Err(Validation)`。

```rust
/// 将 Python callable 桥接为 adapters::CheckFunc（Fn(&Value, &Context) -> Result<()>）。
///
/// Python 调用约定：callable(obj, context) → truthy/falsy。
/// truthy → Ok(())，falsy → Err(ConstructError::Validation)。
pub fn py_to_adapter_check_func(callable: PyObject) -> adapters::CheckFunc {
    Box::new(move |obj: &Value, ctx: &Context| {
        Python::with_gil(|py| {
            let py_obj = crate::conversions::value_to_py(py, obj)?;
            let py_ctx = crate::conversions::context_to_py_container(py, ctx)?;
            let result = callable
                .call1(py, (py_obj.bind(py), py_ctx.bind(py)))
                .map_err(|e| ConstructError::Generic {
                    path: String::new(),
                    message: format!("Validator error: {}", e),
                })?;
            if result.is_truthy(py).unwrap_or(false) {
                Ok(())
            } else {
                Err(ConstructError::Validation {
                    path: String::new(),
                    message: format!("object failed validation: {:?}", obj),
                })
            }
        })
    })
}
```

### 2.4 函数式适配器：OneOf / NoneOf / Filter / Slicing / Indexing

这些在 Python 原版中是**普通函数**（非类），返回 Adapter/Validator 实例。本模块用 Rust 工厂函数实现，返回 §2.1-2.3 的 `#[pyclass]` 包装。

#### OneOf / NoneOf

```python
# Python 原版
def OneOf(subcon, valids):
    return ExprValidator(subcon, lambda obj, ctx: obj in valids)

def NoneOf(subcon, invalids):
    return ExprValidator(subcon, lambda obj, ctx: obj not in invalids)
```

**实现方案**：`valids`/`invalids` 是 Python 集合（list/set/frozenset），无法直接转换为 Rust 集合（因为 Value 无 Hash）。将 `valids` 作为 `PyObject` 持有在闭包中，在验证时通过 GIL 调用 Python 的 `__contains__`。

```rust
/// `OneOf(subcon, valids)` → ExprValidator
///
/// valids 是任意 Python 容器（list/set/frozenset），通过 GIL 调用 `in` 检查。
#[pyfunction]
#[pyo3(name = "OneOf", signature = (subcon, valids))]
pub fn py_one_of(
    py: Python<'_>,
    subcon: &Bound<PyAny>,
    valids: &Bound<PyAny>,
) -> PyResult<PyExprValidator> {
    let sc = extract_subcon(subcon)?;
    let valids_obj: PyObject = valids.clone().unbind();
    let check: adapters::CheckFunc = Box::new(move |obj: &Value, _ctx: &Context| {
        Python::with_gil(|py| {
            let py_obj = crate::conversions::value_to_py(py, obj)?;
            let contains = valids_obj
                .bind(py)
                .call_method1("__contains__", (py_obj.bind(py),))
                .map_err(|e| ConstructError::Generic {
                    path: String::new(),
                    message: format!("OneOf __contains__ error: {}", e),
                })?;
            if contains.is_truthy(py).unwrap_or(false) {
                Ok(())
            } else {
                Err(ConstructError::Validation {
                    path: String::new(),
                    message: format!("object failed validation: {:?}", obj),
                })
            }
        })
    });
    Ok(PyExprValidator {
        inner: construct::constructs::adapters::ExprValidator::new(sc, check),
    })
}

/// `NoneOf(subcon, invalids)` → ExprValidator（取反逻辑）
#[pyfunction]
#[pyo3(name = "NoneOf", signature = (subcon, invalids))]
pub fn py_none_of(
    py: Python<'_>,
    subcon: &Bound<PyAny>,
    invalids: &Bound<PyAny>,
) -> PyResult<PyExprValidator> {
    // 逻辑同 OneOf，但 __contains__ 结果取反
    // ...（省略，与 py_one_of 镜像，contains 为 false 时 Ok，true 时 Err）
}
```

#### Filter

```python
# Python 原版
def Filter(predicate, subcon):
    return ExprSymmetricAdapter(subcon, lambda obj, ctx: [x for x in obj if predicate(x, ctx)])
```

**语义**：Filter 包装一个返回列表的 subcon（Array/GreedyRange/Sequence），parse 后过滤列表元素，build 时也过滤（对称）。predicate 签名是 `predicate(obj, ctx) -> bool`（单元素验证）。

```rust
/// `Filter(predicate, subcon)` → ExprSymmetricAdapter
///
/// predicate 签名：predicate(element, context) -> bool
/// symmetric func 对整个列表应用列表推导式。
#[pyfunction]
#[pyo3(name = "Filter", signature = (predicate, subcon))]
pub fn py_filter(
    py: Python<'_>,
    predicate: &Bound<PyAny>,
    subcon: &Bound<PyAny>,
) -> PyResult<PyExprSymmetricAdapter> {
    let sc = extract_subcon(subcon)?;
    let pred_obj: PyObject = predicate.clone().unbind();
    let func: adapters::SymmetricFunc = Box::new(move |obj: &Value, ctx: &Context| {
        Python::with_gil(|py| {
            // obj 应为 Value::List
            let list = match obj {
                Value::List(items) => items,
                _ => return Err(ConstructError::TypeMismatch {
                    path: String::new(),
                    expected: "List".to_string(),
                    actual: obj.type_name(),
                }),
            };
            let py_ctx = crate::conversions::context_to_py_container(py, ctx)?;
            let mut filtered = Vec::new();
            for item in list {
                let py_item = crate::conversions::value_to_py(py, item)?;
                let result = pred_obj
                    .bind(py)
                    .call1((py_item.bind(py), py_ctx.bind(py)))
                    .map_err(|e| ConstructError::Generic {
                        path: String::new(),
                        message: format!("Filter predicate error: {}", e),
                    })?;
                if result.is_truthy(py).unwrap_or(false) {
                    filtered.push(item.clone());
                }
            }
            Ok(Value::List(filtered))
        })
    });
    Ok(PyExprSymmetricAdapter {
        inner: construct::constructs::adapters::SymmetricAdapter::new(sc, func),
    })
}
```

#### Slicing / Indexing

这两个在 Python 原版是 `Adapter` 子类（有 `count`/`start`/`stop`/`step`/`empty` 配置）。本模块用 Rust `Adapter` + 闭包实现，配置参数捕获到闭包中。

```rust
/// `Slicing(subcon, count, start, stop, step=1, empty=None)` → Adapter
#[pyfunction]
#[pyo3(name = "Slicing", signature = (subcon, count, start, stop, step=1, empty=None))]
pub fn py_slicing(
    py: Python<'_>,
    subcon: &Bound<PyAny>,
    count: usize,
    start: Option<usize>,
    stop: Option<usize>,
    step: usize,
    empty: Option<&Bound<PyAny>>,
) -> PyResult<PyAdapter> {
    let sc = extract_subcon(subcon)?;
    let empty_val = match empty {
        Some(e) => crate::conversions::py_to_value(py, e)?,
        None => Value::None,
    };
    let decode = move |obj: &Value, _ctx: &Context| {
        let list = match obj {
            Value::List(items) => items.clone(),
            _ => return Err(/* TypeMismatch */),
        };
        // Python slice 语义：list[start:stop:step]
        let sliced = slice_list(&list, start, stop, step);
        Ok(Value::List(sliced))
    };
    let encode = move |obj: &Value, _ctx: &Context| {
        let input = match obj {
            Value::List(items) => items.clone(),
            _ => return Err(/* TypeMismatch */),
        };
        // 构建长度为 count 的列表，用 empty 填充，将 input 放入 [start:stop:step] 位置
        let mut output = vec![empty_val.clone(); count];
        place_into_slice(&mut output, &input, start, stop, step);
        Ok(Value::List(output))
    };
    Ok(PyAdapter {
        inner: construct::constructs::adapters::Adapter::new(
            sc,
            Box::new(decode),
            Box::new(encode),
        ),
    })
}

/// `Indexing(subcon, count, index, empty=None)` → Adapter
#[pyfunction]
#[pyo3(name = "Indexing", signature = (subcon, count, index, empty=None))]
pub fn py_indexing(
    py: Python<'_>,
    subcon: &Bound<PyAny>,
    count: usize,
    index: usize,
    empty: Option<&Bound<PyAny>>,
) -> PyResult<PyAdapter> {
    // decode: list → list[index]
    // encode: value → list[count] with value at index, empty elsewhere
    // ...
}
```

**slice_list / place_into_slice 辅助函数**：实现 Python 切片语义（处理 None、负数索引、step）。这些是纯 Rust 辅助函数，不穿越 FFI。

### 2.5 Optional

```python
# Python 原版
def Optional(subcon):
    return IfThenElse(this._embedding or False, subcon, Pass)
```

**注意**：Python 原版的 `Optional` 使用 `this._embedding` 上下文字段。但在大多数测试用例中，`Optional` 的行为是"解析时尝试，失败则返回 None"。Rust 内核的 `stream_ops.rs:375` 已有类似 `Peek` 机制（捕获 Check/StopField 错误返回 None）。

**实现方案**：用 Rust `IfThenElse` + 一个常量 CondFunc（基于 `_embedding` 上下文字段）。

```rust
/// `Optional(subcon)` → IfThenElse
///
/// 条件：context 中 `_embedding` 为 truthy 时使用 subcon，否则使用 Pass。
#[pyfunction]
#[pyo3(name = "Optional", signature = (subcon))]
pub fn py_optional(
    py: Python<'_>,
    subcon: &Bound<PyAny>,
) -> PyResult<PyIfThenElse> {
    let sc = extract_subcon(subcon)?;
    let cond: CondFunc = Box::new(|ctx: &Context| {
        // 检查 context 中 _embedding 字段
        ctx.get("_embedding")
            .map(|v| v.as_bool().unwrap_or(false))
            .unwrap_or(false)
    });
    Ok(PyIfThenElse {
        inner: construct::constructs::control_flow::IfThenElse::new(
            cond,
            sc,
            Box::new(construct::constructs::meta::Pass::new()),
        ),
    })
}
```

**已知限制**：Python 原版的 `Optional` 在解析失败时（如数据不足）也返回 None（通过捕获 StreamError）。Rust 端的 `IfThenElse` 不捕获 StreamError。如果测试用例依赖"解析失败返回 None"的行为，需要用 `Select`（已实现，捕获错误尝试下一个）或自定义包装。Phase 10 先实现基本语义（基于 `_embedding`），解析失败行为标记为已知限制。

---

## 3. Rust 内建控制流 PyO3 包装

### 3.1 If / IfThenElse

```rust
/// PyO3 包装：条件构造器（二选一）。
#[pyclass(name = "IfThenElse")]
pub struct PyIfThenElse {
    pub(crate) inner: construct::constructs::control_flow::IfThenElse,
}

impl PyConstructWrapper for PyIfThenElse {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "IfThenElse", signature = (condfunc, then_subcon, else_subcon))]
pub fn py_if_then_else(
    py: Python<'_>,
    condfunc: &Bound<PyAny>,
    then_subcon: &Bound<PyAny>,
    else_subcon: &Bound<PyAny>,
) -> PyResult<PyIfThenElse> {
    let cond = py_param_to_cond_func(py, condfunc)?;  // 10.4 已实现
    let then_sc = extract_subcon(then_subcon)?;
    let else_sc = extract_subcon(else_subcon)?;
    Ok(PyIfThenElse {
        inner: construct::constructs::control_flow::IfThenElse::new(cond, then_sc, else_sc),
    })
}

/// `If(condfunc, subcon)` → IfThenElse(condfunc, subcon, Pass)
#[pyfunction]
#[pyo3(name = "If", signature = (condfunc, subcon))]
pub fn py_if(
    py: Python<'_>,
    condfunc: &Bound<PyAny>,
    subcon: &Bound<PyAny>,
) -> PyResult<PyIfThenElse> {
    let cond = py_param_to_cond_func(py, condfunc)?;
    let sc = extract_subcon(subcon)?;
    Ok(PyIfThenElse {
        inner: construct::constructs::control_flow::IfThenElse::new(
            cond,
            sc,
            Box::new(construct::constructs::meta::Pass::new()),
        ),
    })
}
```

**condfunc 桥接**：复用 10.4 的 `py_param_to_cond_func`（单参数 `(context) → bool`）。支持 Path/BinExpr/lambda。

### 3.2 Switch

```rust
/// PyO3 包装：多分支条件构造器。
#[pyclass(name = "Switch")]
pub struct PySwitch {
    pub(crate) inner: construct::constructs::control_flow::Switch,
}

impl PyConstructWrapper for PySwitch {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "Switch", signature = (keyfunc, cases, default=None))]
pub fn py_switch(
    py: Python<'_>,
    keyfunc: &Bound<PyAny>,
    cases: &Bound<PyDict>,
    default: Option<&Bound<PyAny>>,
) -> PyResult<PySwitch> {
    let keyf = py_to_key_func(keyfunc.clone().unbind())?;  // 10.4 已实现
    // 将 Python dict 转为 Vec<(Value, Box<dyn Construct>)>
    let mut cases_vec = Vec::new();
    for (key, val) in cases.iter() {
        let key_val = crate::conversions::py_to_value(py, &key)?;
        let sc = extract_subcon(&val)?;
        cases_vec.push((key_val, sc));
    }
    let default_sc = match default {
        Some(d) => Some(extract_subcon(d)?),
        None => None,
    };
    Ok(PySwitch {
        inner: construct::constructs::control_flow::Switch::new(keyf, cases_vec, default_sc),
    })
}
```

**Python 调用约定**：`Switch(this.type, {1: "a"/Byte, 2: "b"/Short})`。cases 是 Python dict，key 是表达式求值结果（int/str），value 是构造器。

**Switch keyfunc 的 NODEFAULT 标记**：Python 原版 Switch 有 `default=Pass`（默认）和 `default=NODEFAULT`（无匹配时抛 SwitchError）。Rust 端 `default: None` 对应 Python 的 `default=Pass`。如需支持 `NODEFAULT`，需额外参数。

### 3.3 Check / StopIf

```rust
/// PyO3 包装：上下文条件检查。
#[pyclass(name = "Check")]
pub struct PyCheck {
    pub(crate) inner: construct::constructs::control_flow::Check,
}

impl PyConstructWrapper for PyCheck {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "Check", signature = (condfunc))]
pub fn py_check(
    py: Python<'_>,
    condfunc: &Bound<PyAny>,
) -> PyResult<PyCheck> {
    // condfunc 是单参数 (context) → truthy/falsy 的表达式
    let check = py_to_control_check_func(condfunc.clone().unbind())?;
    Ok(PyCheck {
        inner: construct::constructs::control_flow::Check::new(check),
    })
}

/// PyO3 包装：停止字段处理（信号构造器）。
#[pyclass(name = "StopIf")]
pub struct PyStopIf {
    pub(crate) inner: construct::constructs::control_flow::StopIf,
}

impl PyConstructWrapper for PyStopIf {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "StopIf", signature = (condfunc))]
pub fn py_stop_if(
    py: Python<'_>,
    condfunc: &Bound<PyAny>,
) -> PyResult<PyStopIf> {
    let cond = py_param_to_cond_func(py, condfunc)?;
    Ok(PyStopIf {
        inner: construct::constructs::control_flow::StopIf::new(cond),
    })
}
```

**Check 的 condfunc 桥接**：Check 的 Rust 签名是 `CheckFunc = Fn(&Context) -> Result<()>`（单参数，返回 Result）。与 Adapter 的 CheckFunc（双参数）不同。

```rust
/// 将 Python callable 桥接为 control_flow::CheckFunc（Fn(&Context) -> Result<()>）。
///
/// Python 调用约定：callable(context) → truthy/falsy。
pub fn py_to_control_check_func(callable: PyObject) -> control_flow::CheckFunc {
    Box::new(move |ctx: &Context| {
        Python::with_gil(|py| {
            let py_ctx = crate::conversions::context_to_py_container(py, ctx)?;
            let result = callable
                .call1(py, (py_ctx.bind(py),))
                .map_err(|e| ConstructError::Generic {
                    path: String::new(),
                    message: format!("Check error: {}", e),
                })?;
            if result.is_truthy(py).unwrap_or(false) {
                Ok(())
            } else {
                Err(ConstructError::Check {
                    path: String::new(),
                    message: "Check failed".to_string(),
                })
            }
        })
    })
}
```

---

## 4. Rust 内建映射/显示 PyO3 包装

### 4.1 Enum / FlagsEnum / Mapping

#### Enum

```rust
/// PyO3 包装：整数 ↔ 字符串枚举映射。
#[pyclass(name = "Enum")]
pub struct PyEnum {
    pub(crate) inner: construct::constructs::enum_::Enum,
}

impl PyConstructWrapper for PyEnum {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "Enum", signature = (*args, **kw))]
pub fn py_enum(
    py: Python<'_>,
    args: &Bound<PyTuple>,
    kw: &Bound<PyDict>,
) -> PyResult<PyEnum> {
    // Python: Enum(subcon, **mapping)
    // args[0] = subcon, **kw = label → integer 映射
    let args_vec: Vec<&Bound<PyAny>> = args.iter().collect();
    if args_vec.len() != 1 {
        return Err(PyTypeError::new_err("Enum requires exactly 1 positional arg (subcon)"));
    }
    let sc = extract_subcon(args_vec[0])?;
    let mut mapping = IndexMap::new();
    for (key, val) in kw.iter() {
        let label: String = key.extract()?;
        let int_val: u64 = val.extract()?;
        mapping.insert(label, int_val);
    }
    Ok(PyEnum {
        inner: construct::constructs::enum_::Enum::new(sc, mapping),
    })
}
```

**Python 调用约定**：`Enum(Byte, one=1, two=2, four=4)`。subcon 是位置参数，映射是 keyword 参数。

**parse 语义**（Rust 已实现）：
- 映射值 → 返回 `Value::String`（标签名）
- 未映射值 → 返回原始整数

**build 语义**：
- `Value::String`（标签）→ 查 encoding map → 整数
- 整数 → 直接传递（passthrough）

#### FlagsEnum

```rust
/// PyO3 包装：位标志枚举。
#[pyclass(name = "FlagsEnum")]
pub struct PyFlagsEnum {
    pub(crate) inner: construct::constructs::enum_::FlagsEnum,
}

impl PyConstructWrapper for PyFlagsEnum {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "FlagsEnum", signature = (*args, **kw))]
pub fn py_flags_enum(
    py: Python<'_>,
    args: &Bound<PyTuple>,
    kw: &Bound<PyDict>,
) -> PyResult<PyFlagsEnum> {
    let args_vec: Vec<&Bound<PyAny>> = args.iter().collect();
    if args_vec.len() != 1 {
        return Err(PyTypeError::new_err("FlagsEnum requires exactly 1 positional arg"));
    }
    let sc = extract_subcon(args_vec[0])?;
    let mut flags = IndexMap::new();
    for (key, val) in kw.iter() {
        let name: String = key.extract()?;
        let bit_val: u64 = val.extract()?;
        flags.insert(name, bit_val);
    }
    Ok(PyFlagsEnum {
        inner: construct::constructs::enum_::FlagsEnum::new(sc, flags),
    })
}
```

**Python 调用约定**：`FlagsEnum(Byte, read=1, write=2, exec=4)`。parse 返回 `Value::Container`（每个 flag → bool）。build 接受 Container，OR 运算后写入。

#### Mapping

```rust
/// PyO3 包装：通用双向值映射。
#[pyclass(name = "Mapping")]
pub struct PyMapping {
    pub(crate) inner: construct::constructs::enum_::Mapping,
}

impl PyConstructWrapper for PyMapping {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "Mapping", signature = (subcon, mapping))]
pub fn py_mapping(
    py: Python<'_>,
    subcon: &Bound<PyAny>,
    mapping: &Bound<PyDict>,
) -> PyResult<PyMapping> {
    let sc = extract_subcon(subcon)?;
    // mapping: Python dict {build_key: build_value}
    let mut pairs = Vec::new();
    for (key, val) in mapping.iter() {
        let k = crate::conversions::py_to_value(py, &key)?;
        let v = crate::conversions::py_to_value(py, &val)?;
        pairs.push((k, v));
    }
    Ok(PyMapping {
        inner: construct::constructs::enum_::Mapping::new(sc, pairs),
    })
}
```

**与 Enum 的区别**：Mapping 是通用的（key/value 可以是任意 Value 类型），Enum 是特化的（string ↔ u64）。Mapping 在未匹配时抛 MappingError，Enum 在未匹配时返回原始整数。

### 4.2 Hex / HexDump

```rust
/// PyO3 包装：十六进制显示适配器（pass-through）。
#[pyclass(name = "Hex")]
pub struct PyHex {
    pub(crate) inner: construct::constructs::hex::Hex,
}

impl PyConstructWrapper for PyHex {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "Hex", signature = (subcon))]
pub fn py_hex(subcon: &Bound<PyAny>) -> PyResult<PyHex> {
    let sc = extract_subcon(subcon)?;
    Ok(PyHex {
        inner: construct::constructs::hex::Hex::new(sc),
    })
}

/// PyO3 包装：十六进制转储显示适配器（pass-through）。
#[pyclass(name = "HexDump")]
pub struct PyHexDump {
    pub(crate) inner: construct::constructs::hex::HexDump,
}

impl PyConstructWrapper for PyHexDump {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "HexDump", signature = (subcon))]
pub fn py_hex_dump(subcon: &Bound<PyAny>) -> PyResult<PyHexDump> {
    let sc = extract_subcon(subcon)?;
    Ok(PyHexDump {
        inner: construct::constructs::hex::HexDump::new(sc),
    })
}
```

**Hex/HexDump 的显示语义**：Rust 端 Hex/HexDump 是 pass-through（parse/build 不变换值）。Python 原版中 Hex 返回 `HexDisplayedInteger`/`HexDisplayedBytes` 包装对象，改变 `str()`/`repr()` 输出。

**Phase 10 策略**：由于测试用例（`test_core.py` 的 `commonhex`/`commondump` helper）主要检查 parse/build 的值正确性（而非 `str()` 输出），Rust pass-through 实现可覆盖这些测试。如果个别测试检查 `str()` 输出，标记为已知限制（显示语义差异）。

---

## 5. 纯 Python 实现的构造器

### 5.1 NamedTuple

**文件**：`construct-py/construct_rust/_namedtuple.py`（新建）

**依赖**：Python 标准库 `collections.namedtuple`（无法用 Rust 合理实现，因为是 Python 动态类工厂）。

```python
"""NamedTuple construct — maps parsed values to collections.namedtuple instances.

This is a pure-Python implementation because it depends on the Python
standard library's `collections.namedtuple` factory, which dynamically
generates Python classes. This cannot be reasonably replicated in Rust.
"""

import collections

from ._adapter import Adapter, Construct
from .lib.containers import Container


class NamedTuple(Adapter):
    """Both arrays, structs, and sequences can be mapped to a namedtuple.

    :param tuplename: string
    :param tuplefields: string or list of strings
    :param subcon: Construct instance (Struct, Sequence, Array, GreedyRange)
    """

    def __init__(self, tuplename, tuplefields, subcon):
        if not isinstance(subcon, Construct) and not _is_composite(subcon):
            from . import NamedTupleError
            raise NamedTupleError("subcon is neither Struct Sequence Array GreedyRange")
        if isinstance(subcon, Adapter):
            # 用户可能传入已包装的 Adapter，检查其内部 subcon
            pass
        super().__init__(subcon)
        self.tuplename = tuplename
        self.tuplefields = tuplefields
        self.factory = collections.namedtuple(tuplename, tuplefields)

    def _decode(self, obj, context, path):
        # obj 可能是 Container（来自 Struct）或 list（来自 Sequence/Array/GreedyRange）
        if isinstance(obj, (list, tuple)):
            return self.factory(*obj)
        if isinstance(obj, dict):
            obj = {k: v for k, v in obj.items() if not k.startswith("_")}
            return self.factory(**obj)
        from . import NamedTupleError
        raise NamedTupleError(
            "subcon is neither Struct Sequence Array GreedyRange", path=path)

    def _encode(self, obj, context, path):
        if isinstance(obj, tuple) and hasattr(obj, "_fields"):
            # namedtuple 实例
            if self.tuplefields and isinstance(self.subcon, _StructLike):
                return Container({f: getattr(obj, f) for f in obj._fields})
            return list(obj)
        if isinstance(obj, (list, dict)):
            return obj
        from . import NamedTupleError
        raise NamedTupleError("cannot encode", path=path)


def _is_composite(obj):
    """Check if obj is a Struct/Sequence/Array/GreedyRange (Rust or Python)."""
    # 检查类型名（Rust 包装类名是 "Struct"/"Sequence"/"Array"/"GreedyRange"）
    class_name = type(obj).__name__
    return class_name in ("Struct", "Sequence", "Array", "GreedyRange")


class _StructLike:
    """Marker for isinstance checks (never instantiated)."""
    pass
```

**关键设计**：
1. `NamedTuple` 继承纯 Python `Adapter` 基类（10.4 已实现）
2. `_decode`/`_encode` 用 Python 的 `isinstance` 检查值类型（Container/list）
3. 混合分发：`self.subcon` 可能是 Rust PyO3 包装（Struct/Sequence/Array/GreedyRange），通过 `_adapter.py` 的 dual-path 机制（`_parsereport` vs `parse_stream`）调用

### 5.2 TimestampAdapter / Timestamp（Phase 10 不支持）

**依赖**：Python 第三方库 `arrow`（datetime 库）。Phase 10 不安装 arrow。

**策略**：在 `__init__.py` 中尝试导入，失败则导出 stub：

```python
# construct_rust/__init__.py 片段
try:
    import arrow  # noqa: F401
    from ._timestamp import Timestamp, TimestampAdapter, TimestampError
except ImportError:
    # arrow 未安装，导出 stub（导入时不报错，调用时报错）
    class TimestampError(Exception):
        pass

    def Timestamp(*args, **kwargs):
        raise ImportError(
            "Timestamp requires the 'arrow' library. "
            "Install it with: pip install arrow"
        )

    TimestampAdapter = Timestamp  # alias
```

**对应测试用例**：`test_core.py` 中的 `test_timestamp*` 系列用例（如有）将因 `arrow` 未安装而跳过。需在验收报告中记录。

### 5.3 _adapter.py 现有基类审查

10.4 已实现 `_adapter.py`（228 行），包含：
- `Construct`（基类，7 个公共 API + `_parsereport` hook）
- `Subconstruct`（委托 subcon 的基类，dual-path：`_parsereport` vs `parse_stream`）
- `Adapter`（`_decode`/`_encode` 抽象方法）
- `SymmetricAdapter`（`_encode = _decode`）
- `Validator`（`_validate` → bool）

**10.7 审查结论**：现有基类**无需扩展**，已满足以下需求：

| 需求 | 现有支持 | 说明 |
|------|---------|------|
| 用户继承 Adapter 创建自定义适配器 | ✓ `class IpAddressAdapter(Adapter)` | `_decode`/`_encode` 抽象方法 |
| 用户继承 Validator 创建验证器 | ✓ `class MyValidator(Validator)` | `_validate` 抽象方法 |
| 混合嵌套（Python Adapter 内嵌 Rust subcon） | ✓ dual-path `_parsereport` vs `parse_stream` | Subconstruct._parse 自动选择路径 |
| Context 传递（`_` parent 链） | ✓ `_filter_context` 过滤内部键 | 非 `_` 前缀键作为 kwargs 传递 |
| `flagbuildnone` 属性传播 | ✓ `getattr(subcon, "flagbuildnone", False)` | Subconstruct.__init__ |

**唯一需要补充**：`NamedTuple` 和 `TimestampAdapter` 作为 `_adapter.py` 的扩展模块（`_namedtuple.py`、`_timestamp.py`），而非修改 `_adapter.py` 本身。

---

## 6. PyConstructAdapter 增强

### 6.1 extract_subcon 的 PyO3 类型分支补全

**现状**：10.4 实现的 `extract_subcon`（`py_adapter.rs:247`）仅使用 duck typing（检查 `parse_stream` + `build_stream` 方法）。10.5/10.6 已在 `constructs_composite.rs` 中扩展了 PyO3 类型分支（PyFormatField/PyBytes/PyStruct 等）。

**10.7 需补充**：将本子任务新增的适配器/控制流 `#[pyclass]` 类型加入 `extract_subcon` 的类型检查分支。

**推荐方案**：在 `construct_macros.rs` 中定义统一的注册机制，避免在每个新包装类出现时手动修改 `extract_subcon`。

```rust
/// 尝试从 Python 对象提取已注册的 PyO3 包装类型。
///
/// 遍历所有已注册的 #[pyclass] 类型，返回第一个匹配的。
/// 使用 Python 类型对象的指针比较（O(1) per type）优化性能。
///
/// 10.5/10.6/10.7 的所有包装类型在此注册。
pub fn try_extract_registered(obj: &Bound<PyAny>) -> PyResult<Option<Box<dyn Construct>>> {
    use constructs_atomic::*;
    use constructs_composite::*;
    use constructs_adapter::*;  // 10.7 新增

    // 原子（10.5）
    if let Ok(w) = obj.extract::<PyRef<PyFormatField>>() { return Ok(Some(Box::new(w.inner))); }
    if let Ok(w) = obj.extract::<PyRef<PyBytes>>() { return Ok(Some(w.inner.clone_box_construct())); }
    // ... 其他原子类型 ...

    // 复合（10.6）
    if let Ok(w) = obj.extract::<PyRef<PyStruct>>() { return Ok(Some(Box::new(w.inner.clone()))); }
    // ... 其他复合类型 ...

    // 适配器/控制流（10.7）
    if let Ok(w) = obj.extract::<PyRef<PyExprAdapter>>() { return Ok(Some(Box::new(w.inner.clone()))); }
    if let Ok(w) = obj.extract::<PyRef<PySymmetricAdapter>>() { return Ok(Some(Box::new(w.inner.clone()))); }
    if let Ok(w) = obj.extract::<PyRef<PyValidator>>() { return Ok(Some(Box::new(w.inner.clone()))); }
    if let Ok(w) = obj.extract::<PyRef<PyEnum>>() { return Ok(Some(Box::new(w.inner.clone()))); }
    if let Ok(w) = obj.extract::<PyRef<PyHex>>() { return Ok(Some(Box::new(w.inner.clone()))); }
    if let Ok(w) = obj.extract::<PyRef<PyIfThenElse>>() { return Ok(Some(Box::new(w.inner.clone()))); }
    if let Ok(w) = obj.extract::<PyRef<PySwitch>>() { return Ok(Some(Box::new(w.inner.clone()))); }
    if let Ok(w) = obj.extract::<PyRef<PyCheck>>() { return Ok(Some(Box::new(w.inner.clone()))); }
    if let Ok(w) = obj.extract::<PyRef<PyStopIf>>() { return Ok(Some(Box::new(w.inner.clone()))); }
    // ... 所有 10.7 包装类型 ...

    Ok(None)
}
```

**Adapter 的 clone_box 问题**：Rust `Adapter` 持有 `Box<dyn Construct>` 和 `Box<dyn Fn(...)>` 闭包，不实现 `Clone`。但 `extract_subcon` 需要 owned `Box<dyn Construct>`。

**解决方案**：为每个适配器包装类实现 `clone_box_construct()` 方法，通过**重新构造**而非 clone：

```rust
impl PyExprAdapter {
    /// 克隆内部 Adapter（重新构造，闭包无法 Clone 但可重新创建）。
    ///
    /// 注意：此方法要求原始 Python callable 仍然有效。
    /// 由于 PyExprAdapter 持有 Python 对象的引用（通过 GIL），
    /// 重新构造时闭包捕获新的 PyObject 引用。
    pub fn clone_box_construct(&self) -> Box<dyn Construct> {
        // 问题：Adapter 内部的闭包无法 clone
        // 方案：PyExprAdapter 额外持有原始 PyObject（decoder/encoder），
        //       clone_box_construct 时重新调用 py_to_decode_func
        todo!("需要 PM 确认 Clone 策略")
    }
}
```

**PM 需裁定**：适配器包装类的 Clone 策略。两个备选方案：

| 方案 | 描述 | 优点 | 缺点 |
|------|------|------|------|
| **A: 持有原始 PyObject** | PyExprAdapter 额外存储 `decoder_obj: PyObject` + `encoder_obj: PyObject`，clone 时重新桥接 | Clone 可行 | 内存增加（重复存储 callable 引用） |
| **B: 禁止 Clone** | 适配器包装类不支持 clone_box，extract_subcon 对适配器返回借用 | 无额外内存 | 无法在需要 owned 的场景使用（如 Struct 字段） |
| **C: Rc 共享** | 用 `Rc<Adapter>` 共享，clone 增加 refcount | Clone 廉价 | `Rc` 非 `Send + Sync`，违反 Construct trait 约束 |

**建议方案 A**，因为 Struct 字段需要 owned `Box<dyn Construct>`，禁止 Clone 会导致 `Struct("x" / ExprAdapter(...))` 失败。

### 6.2 PyConstructAdapter 流位置同步（已知限制）

10.4 实现的 `PyConstructAdapter::parse` 使用 **Read-all + BytesIO** 方案：
1. 读取 Rust Stream 所有剩余字节 → Python BytesIO
2. 调用 Python `parse_stream`
3. 通过 `BytesIO.tell()` 同步消耗的位置回 Rust Stream

**已知限制**（10.4 §6.5 已记录）：
- 大数据内存占用高（全部读入内存）
- 不支持流式构造器（如 Compressed、Restreamed）

**10.7 确认**：适配器测试用例（`test_ipaddress_adapter_issue_95`、`test_from_issue_244`）不涉及大数据或流式构造，方案 A 可覆盖。

### 6.3 混合嵌套调用链

混合架构下的典型调用链：

```
Python: d = Struct("ip" / IpAddressAdapter(Byte[4]))
                     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
                     纯 Python Adapter 子类

Rust Struct.parse(stream, ctx):
  → 遍历字段 "ip"
  → extract_subcon(IpAddressAdapter(Byte[4]))
    → IpAddressAdapter 是纯 Python 对象（无 PyO3 类型匹配）
    → duck typing: hasattr("parse_stream") + hasattr("build_stream") → True
    → 包装为 PyConstructAdapter(IpAddressAdapter 实例)
  → PyConstructAdapter.parse(stream, ctx):
    → Python::with_gil:
      → 读取剩余字节 → BytesIO
      → IpAddressAdapter.parse_stream(BytesIO, **filtered_ctx)
        → 纯 Python Adapter._parse(stream, context, path):
          → self.subcon._parsereport(stream, context, path)
            → self.subcon 是 Byte[4]（Rust PyArray 包装）
            → hasattr(subcon, "_parsereport")? No（Rust 包装只有 parse_stream）
            → 走 dual-path: subcon.parse_stream(stream, **filtered_ctx)
              → PyArray.parse_stream → Rust Array.parse → 4 × FormatField.parse
          → obj = [192, 168, 1, 1]
          → self._decode([192,168,1,1], context, path)
            → return "192.168.1.1"
      → BytesIO.tell() = 4
    → 同步 Rust Stream: seek(start_pos + 4)
    → 返回 Value::String("192.168.1.1")
  → ctx["ip"] = "192.168.1.1"
```

**性能特征**：
- Rust 内建字段（Byte[4]）：Rust 原生速度
- Python Adapter 回调：1 次 FFI 往返 + Context 转换 + GIL 获取
- 内嵌 Rust subcon 回调：通过 `parse_stream` 再入 Rust（dual-path）

---

## 7. 运算符重载

所有 10.7 的 `#[pyclass]` 包装类（PyExprAdapter/PyValidator/PyEnum/PyHex/PyIfThenElse 等）通过 `impl_construct_operators!` 宏获得 6 个运算符 dunder 方法。

**语义**（与 10.5/10.6 一致）：

| 运算符 | 示例 | 结果 |
|--------|------|------|
| `"name" / adapter` | `"x" / Hex(Byte)` | `PyRenamed`（命名） |
| `adapter * "docs"` | `Hex(Byte) * "docstring"` | `PyRenamed`（带 docs） |
| `adapter + other` | `Hex(Byte) + "y"/Short` | `PyStruct`（合并） |
| `adapter >> other` | `Hex(Byte) >> Short` | `PySequence`（合并） |
| `adapter[n]` | `Hex(Byte)[4]` | `PyArray`（重复） |

**Check/StopIf 的运算符**：Check 和 StopIf 是信号构造器，通常不参与运算符组合。但仍通过宏提供运算符（Python 原版中它们继承 Construct 基类，也有运算符）。使用时如 `"check" / Check(this.x == 0)`。

---

## 8. Python API 映射表

### 8.1 适配器（Rust PyO3 包装）

| Python 原版 | Rust 内核类型 | PyO3 包装类 | 映射说明 |
|------------|-------------|------------|---------|
| `ExprAdapter(subcon, decoder, encoder)` | `adapters::Adapter` | `PyExprAdapter` | decoder/encoder → PyClosure 双参数 |
| `SymmetricAdapter(subcon, func)` | `adapters::SymmetricAdapter` | `PySymmetricAdapter` | func → PyClosure 双参数 |
| `ExprSymmetricAdapter(subcon, func)` | `adapters::SymmetricAdapter` | `PyExprSymmetricAdapter` | 语义别名，独立 pyclass |
| `Validator(subcon, func)` | `adapters::Validator` | `PyValidator` | func → adapters::CheckFunc 双参数 |
| `ExprValidator(subcon, validator)` | `adapters::ExprValidator` | `PyExprValidator` | 语义别名，独立 pyclass |
| `Enum(subcon, **kw)` | `enum_::Enum` | `PyEnum` | **kw → IndexMap<String, u64> |
| `FlagsEnum(subcon, **kw)` | `enum_::FlagsEnum` | `PyFlagsEnum` | **kw → IndexMap<String, u64> |
| `Mapping(subcon, mapping)` | `enum_::Mapping` | `PyMapping` | mapping dict → Vec<(Value, Value)> |
| `Hex(subcon)` | `hex::Hex` | `PyHex` | pass-through |
| `HexDump(subcon)` | `hex::HexDump` | `PyHexDump` | pass-through |

### 8.2 控制流（Rust PyO3 包装）

| Python 原版 | Rust 内核类型 | PyO3 包装类 | 映射说明 |
|------------|-------------|------------|---------|
| `If(condfunc, subcon)` | `control_flow::IfThenElse`（+Pass） | `PyIfThenElse` | 函数式 |
| `IfThenElse(condfunc, then, else)` | `control_flow::IfThenElse` | `PyIfThenElse` | condfunc → CondFunc 单参数 |
| `Switch(keyfunc, cases, default)` | `control_flow::Switch` | `PySwitch` | keyfunc → KeyFunc，cases dict → Vec |
| `Check(condfunc)` | `control_flow::Check` | `PyCheck` | condfunc → control_flow::CheckFunc 单参数 |
| `StopIf(condfunc)` | `control_flow::StopIf` | `PyStopIf` | condfunc → CondFunc |

### 8.3 函数式构造器（Rust 工厂函数）

| Python 原版 | 返回类型 | 实现方式 |
|------------|---------|---------|
| `OneOf(subcon, valids)` | `PyExprValidator` | CheckFunc 通过 GIL 调用 `valids.__contains__` |
| `NoneOf(subcon, invalids)` | `PyExprValidator` | 取反的 OneOf |
| `Filter(predicate, subcon)` | `PyExprSymmetricAdapter` | SymmetricFunc 列表推导式 |
| `Slicing(subcon, count, start, stop, step, empty)` | `PyExprAdapter`（用 Rust Adapter） | decode: slice; encode: fill + place |
| `Indexing(subcon, count, index, empty)` | `PyExprAdapter`（用 Rust Adapter） | decode: index; encode: fill + place |
| `Optional(subcon)` | `PyIfThenElse` | IfThenElse(cond=_embedding, subcon, Pass) |

### 8.4 纯 Python 实现

| Python 原版 | 实现文件 | 说明 |
|------------|---------|------|
| `Adapter`（抽象基类） | `_adapter.py`（10.4 已实现） | 用户继承创建自定义适配器 |
| `SymmetricAdapter`（抽象基类） | `_adapter.py`（10.4 已实现） | |
| `Validator`（抽象基类） | `_adapter.py`（10.4 已实现） | |
| `Subconstruct`（抽象基类） | `_adapter.py`（10.4 已实现） | |
| `NamedTuple(name, fields, subcon)` | `_namedtuple.py`（10.7 新增） | 依赖 collections.namedtuple |
| `TimestampAdapter` | stub（arrow 未安装） | ImportError |
| `Timestamp(subcon, unit, epoch)` | stub（arrow 未安装） | ImportError |

---

## 9. 边界条件清单

### 9.1 适配器边界

| # | 场景 | 输入 | 预期行为 |
|---|------|------|---------|
| 1 | ExprAdapter 基本 | `ExprAdapter(Byte, obj_+1, obj_-1)` parse `\x04` | 返回 5 |
| 2 | ExprAdapter build | `ExprAdapter(...)` build 5 | 返回 `b'\x04'` |
| 3 | ExprAdapter decoder 抛异常 | decoder lambda 抛 ValueError | `ConstructError::Generic` |
| 4 | SymmetricAdapter | `SymmetricAdapter(Byte, obj_ & 0xf)` parse `\xff` | 返回 15 |
| 5 | ExprSymmetricAdapter | 同上 | 返回 15（类型名不同） |
| 6 | Validator 通过 | `Validator(Byte, lambda o,c: o <= 10)` parse `\x05` | 返回 5 |
| 7 | Validator 失败 | 同上 parse `\x20` | `ValidationError` |
| 8 | ExprValidator | `ExprValidator(Byte, obj_ != 0)` build 0 | `ValidationError` |
| 9 | ExprAdapter 嵌套 | `ExprAdapter(ExprAdapter(Byte, ...), ...)` | 两层 decode 依次应用 |

### 9.2 枚举/映射边界

| # | 场景 | 输入 | 预期行为 |
|---|------|------|---------|
| 10 | Enum 映射值 | `Enum(Byte, one=1)` parse `\x01` | 返回 "one" |
| 11 | Enum 未映射值 | 同上 parse `\xff` | 返回 255（原始整数） |
| 12 | Enum build 标签 | build "one" | 返回 `b'\x01'` |
| 13 | Enum build 整数 | build 5 | 返回 `b'\x05'`（passthrough） |
| 14 | Enum 未知标签 | build "unknown" | `MappingError` |
| 15 | FlagsEnum 全设置 | `FlagsEnum(Byte, a=1,b=2)` parse `\x03` | Container(a=True, b=True) |
| 16 | FlagsEnum 部分设置 | 同上 parse `\x01` | Container(a=True, b=False) |
| 17 | FlagsEnum build | build Container(a=True, b=False) | 返回 `b'\x01'` |
| 18 | FlagsEnum 跳过 _ 键 | build Container(a=True, _flagsenum=True) | 忽略 _flagsenum |
| 19 | Mapping 基本 | `Mapping(Byte, {"A":0})` parse `\x00` | 返回 "A" |
| 20 | Mapping 未匹配 | 同上 parse `\xff` | `MappingError` |

### 9.3 控制流边界

| # | 场景 | 输入 | 预期行为 |
|---|------|------|---------|
| 21 | If true 分支 | `If(this.x > 0, Byte)` x=5 | 解析 Byte |
| 22 | If false 分支 | 同上 x=0 | 返回 None（Pass） |
| 23 | IfThenElse | `IfThenElse(cond, Int16ub, Int8ub)` | 按 cond 选择 |
| 24 | Switch 匹配 | `Switch(this.t, {1: Byte})` t=1 | 解析 Byte |
| 25 | Switch 无匹配 | 同上 t=99 | 返回 None（默认 Pass） |
| 26 | Switch 显式 default | `Switch(..., default=Byte)` | 无匹配时用 default |
| 27 | Check 通过 | `Check(this.x == 42)` x=42 | 返回 None |
| 28 | Check 失败 | 同上 x=99 | `CheckError` |
| 29 | StopIf 在 Struct 中 | `Struct("a"/Byte, StopIf(this.a==0), "b"/Byte)` | a==0 时停止，不解析 b |
| 30 | StopIf 条件 false | 同上 a==1 | 继续解析 b |
| 31 | Check 不消耗流 | `Check(this.x)` | stream.tell() 不变 |
| 32 | Optional 嵌入模式 | `Optional(Byte)` _embedding=True | 解析 Byte |
| 33 | Optional 非嵌入 | 同上 _embedding=False | 返回 None |

### 9.4 函数式构造器边界

| # | 场景 | 输入 | 预期行为 |
|---|------|------|---------|
| 34 | OneOf 通过 | `OneOf(Byte, [1,2,3])` parse `\x01` | 返回 1 |
| 35 | OneOf 失败 | 同上 parse `\xff` | `ValidationError` |
| 36 | NoneOf 通过 | `NoneOf(Byte, [1,2])` parse `\x03` | 返回 3 |
| 37 | NoneOf 失败 | 同上 parse `\x01` | `ValidationError` |
| 38 | Filter | `Filter(obj_!=0, Byte[:])` parse `\x00\x02\x00` | 返回 [2] |
| 39 | Slicing | `Slicing(Array(4,Byte), 4, 1, 3)` parse `\x01\x02\x03\x04` | 返回 [2,3] |
| 40 | Slicing build | 同上 build [2,3] | 返回 `b'\x00\x02\x03\x00'` |
| 41 | Indexing | `Indexing(Array(4,Byte), 4, 2)` parse `\x01\x02\x03\x04` | 返回 3 |
| 42 | Indexing build | 同上 build 3 | 返回 `b'\x00\x00\x03\x00'` |

### 9.5 Hex/HexDump 边界

| # | 场景 | 输入 | 预期行为 |
|---|------|------|---------|
| 43 | Hex parse | `Hex(Int32ub)` parse 4 字节 | 返回原始整数（pass-through） |
| 44 | Hex build | 同上 build 258 | 返回 4 字节 |
| 45 | HexDump parse | `HexDump(Bytes(4))` parse 4 字节 | 返回原始 bytes |
| 46 | Hex sizeof | `Hex(Int32ub).sizeof()` | 返回 4 |

### 9.6 混合架构边界

| # | 场景 | 输入 | 预期行为 |
|---|------|------|---------|
| 47 | IpAddressAdapter | `Struct("ip"/IpAddressAdapter(Byte[4]))` | 混合分发正确 |
| 48 | Adapter 内嵌 Rust subcon | `class My(Adapter): _decode uses ctx` | dual-path 调用 |
| 49 | 非构造器对象 | `Struct("x"/42)` | `TypeError`（42 无 parse_stream） |
| 50 | PyConstructAdapter 流同步 | Adapter 消耗 4 字节 | Rust Stream seek 到正确位置 |
| 51 | Python 异常传播 | Adapter._decode 抛 ValueError | 映射为 ConstructError::Generic |
| 52 | Check 在 Struct 中失败 | `Struct("x"/Byte, Check(this.x==0))` x=5 | Struct.parse 返回 CheckError |
| 53 | StopIf 停止后续字段 | `Struct(StopIf(True), "x"/Byte)` | 返回空 Container，不解析 x |

### 9.7 NamedTuple 边界

| # | 场景 | 输入 | 预期行为 |
|---|------|------|---------|
| 54 | NamedTuple with Array | `NamedTuple("coord","x y z", Byte[3])` parse `b"123"` | coord(x=49,y=50,z=51) |
| 55 | NamedTuple with Sequence | 同上用 `Byte >> Byte >> Byte` | 同上 |
| 56 | NamedTuple with Struct | 同上用 `"x"/Byte + "y"/Byte + "z"/Byte` | 同上 |
| 57 | NamedTuple build from namedtuple | build coord(x=1,y=2,z=3) | 正确编码 |
| 58 | NamedTuple build from list | build [1,2,3] | 正确编码 |
| 59 | NamedTuple 无效 subcon | `NamedTuple("x","y", Byte)` | `NamedTupleError` |

---

## 10. 与其他模块的交互

### 10.1 依赖（本模块需要）

| 模块 | 使用方式 |
|------|---------|
| **10.2 Value 映射** | `value_to_py`/`py_to_value`/`context_to_py_container` — 所有 PyClosure 桥接的基础 |
| **10.4 表达式桥接** | `py_param_to_cond_func`（单参数 CondFunc）、`py_to_key_func`（KeyFunc）、`PyClosure`、`PyConstructAdapter`、`extract_subcon`、`_adapter.py` 基类 |
| **10.5 原子构造器** | `PyFormatField`/`PyBytes` 等 — 适配器的 subcon 通常是原子构造器 |
| **10.6 复合构造器** | `PyStruct`/`PySequence`/`PyArray` — 适配器/控制流的 subcon 和上下文 |
| **construct-rs adapters.rs** | `Adapter`/`SymmetricAdapter`/`ExprAdapter`/`Validator`/`ExprValidator` + 类型别名 |
| **construct-rs control_flow.rs** | `IfThenElse`/`Switch`/`Check`/`StopIf`/`If` + 类型别名 |
| **construct-rs enum_.rs** | `Enum`/`FlagsEnum`/`Mapping` |
| **construct-rs hex.rs** | `Hex`/`HexDump` |
| **PyO3 0.22** | `#[pyclass]`/`#[pyfunction]`/`Python::with_gil` |

### 10.2 被依赖（下游模块需要本模块）

| 模块 | 使用方式 |
|------|---------|
| **10.8 流操作** | `Prefixed`/`Checksum` 等可能使用 Adapter/Validator 包装 |
| **10.9 字符串/Gallery** | Gallery 格式（ELF/PE）大量使用 Enum/FlagsEnum/Adapter |
| **10.10 测试** | `test_core.py` 中适配器/控制流/枚举相关用例（约 30-40 个） |

### 10.3 接口契约（对下游的承诺）

1. **PyO3 包装透明性**：所有适配器/控制流包装类通过 `impl_api_methods!` 提供 7 个公共 API，行为与 Python 原版一致
2. **运算符支持**：所有包装类通过 `impl_construct_operators!` 支持 `/`/`*`/`+`/`>>`/`[]` 运算符
3. **混合分发**：`extract_subcon` 能识别所有 10.7 包装类，并正确提取 `Box<dyn Construct>`
4. **PyClosure 双参数**：`py_to_decode_func`/`py_to_encode_func`/`py_to_adapter_check_func` 正确传递 `(obj, context)` 给 Python callable
5. **Check/StopIf 生效**：通过 Rust 内核的 StopField 机制，在 Struct/Sequence/GreedyRange 中正确停止字段处理
6. **纯 Python 基类兼容**：用户继承 `_adapter.py` 的 Adapter/Validator 创建的自定义构造器，可通过 PyConstructAdapter 嵌套在 Rust Struct 中

---

## 11. 需 PM 确认的事项

### 11.1 适配器包装类的 Clone 策略

§6.1 提出三种方案（A: 持有原始 PyObject / B: 禁止 Clone / C: Rc 共享）。

**建议方案 A**：PyExprAdapter 等包装类额外存储 `decoder_obj: PyObject` + `encoder_obj: PyObject`，`clone_box_construct()` 时重新调用 `py_to_decode_func` 重新桥接。

**影响**：
- 每个 Adapter 包装类增加 2 个 PyObject 字段（decoder + encoder）
- clone 时重新创建闭包（~微秒级开销）
- 使 `Struct("x" / ExprAdapter(Byte, ..., ...))` 可行

**需 PM 授权**：确认方案 A，或选择其他方案。

### 11.2 Optional 的解析失败行为

§2.5 指出 Python 原版 `Optional` 在解析失败时（StreamError）也返回 None。Rust `IfThenElse` 不捕获 StreamError。

**选项**：
- **A**：Phase 10 实现基本语义（基于 `_embedding`），解析失败行为标记为已知限制
- **B**：实现自定义 `Optional` 包装（类似 Select），捕获 StreamError 返回 None

**建议方案 A**。需检查 `test_core.py` 中 `test_optional` 用例是否依赖解析失败返回 None 的行为。如依赖，升级为方案 B。

### 11.3 Timestamp / arrow 依赖

§5.2 建议 Timestamp stub（arrow 未安装时 ImportError）。

**需 PM 确认**：Phase 10 是否安装 arrow 依赖？如不安装，`test_timestamp*` 用例跳过并在验收报告中记录。

**建议**：不安装 arrow（保持零可选依赖理念），Timestamp 作为 stub。

### 11.4 Hex/HexDump 显示语义

§4.2 指出 Rust Hex/HexDump 是 pass-through，不改变 `str()` 输出。

**需检查**：`test_core.py` 中 `commonhex`/`commondump` helper 是否检查 `str()` 输出。如检查，需要额外的 Python 包装层（返回 `HexDisplayedInteger` 等）。

**建议**：先按 pass-through 实现，测试时如发现 `str()` 检查失败，再补充 Python 显示包装。

### 11.5 ExprAdapter/ExprValidator 的 Python 类型名

Python 原版中：
- `ExprAdapter` 是**类**（用户可 `isinstance(x, ExprAdapter)`）
- `ExprValidator` 是**类**
- `Adapter` 是**抽象基类**（用户继承）
- `Validator` 是**抽象基类**（用户继承）

本模块设计中：
- `PyExprAdapter`（pyclass name="ExprAdapter"）— 对应 Python 的 ExprAdapter 类
- `PyAdapter`（pyclass name="Adapter"）— **未定义**（Python 的 Adapter 是纯 Python 抽象类）

**问题**：`isinstance(ExprAdapter(...), Adapter)` 应为 True（ExprAdapter 继承 Adapter）。但 PyExprAdapter 是 Rust pyclass，PyAdapter 是纯 Python 类，无继承关系。

**解决方案**：不在 PyO3 层建立继承关系。Python `isinstance` 检查通过 duck typing 或 `hasattr(x, 'parse_stream')` 替代。测试用例如检查 `isinstance`，标记为已知限制。

**需 PM 确认**：是否需要建立 PyO3 pyclass 与纯 Python 基类的继承关系？（复杂度高，收益低）

---

## 12. test_core.py 相关用例预期

### 12.1 适配器/控制流相关用例

| 测试用例 | 预期 | 说明 |
|---------|------|------|
| `test_ipaddress_adapter_issue_95` | **pass** | IpAddressAdapter 混合架构 |
| `test_from_issue_244` | **pass** | 自定义 Adapter 子类 |
| `test_adapter` | **pass** | ExprAdapter 基本 |
| `test_validator` | **pass** | ExprValidator |
| `test_enum` | **pass** | Enum 映射 |
| `test_flagsenum` | **pass** | FlagsEnum 位标志 |
| `test_mapping` | **pass** | Mapping 通用映射 |
| `test_hex` | **pass**（可能 str 差异） | Hex pass-through |
| `test_hexdump` | **pass**（可能 str 差异） | HexDump pass-through |
| `test_ifthenelse` | **pass** | IfThenElse |
| `test_if` | **pass** | If（= IfThenElse + Pass） |
| `test_switch` | **pass** | Switch |
| `test_check` | **pass** | Check |
| `test_stopif` | **pass** | StopIf 在 Struct 中 |
| `test_optional` | **pass**（如不依赖解析失败） | Optional |
| `test_oneof` | **pass** | OneOf |
| `test_noneof` | **pass** | NoneOf |
| `test_filter` | **pass** | Filter |
| `test_slicing` | **pass** | Slicing |
| `test_indexing` | **pass** | Indexing |
| `test_namedtuple` | **pass** | NamedTuple（纯 Python） |
| `test_timestamp*` | **skip** | arrow 未安装 |

### 12.2 预期通过率

10.7 覆盖的测试用例约 20-25 个，预期：
- **pass**：~18-22 个
- **skip**：~2-4 个（timestamp、可能的 hex str 差异）
- **xfail**：0 个（原版无 xfail 标记的适配器用例）

---

## 13. 实现优先级建议

建议 DEV 按以下顺序实现（每步可独立验证）：

1. **expr_bridge.rs 双参数桥接器**：`py_to_decode_func`/`py_to_encode_func`/`py_to_symmetric_func`/`py_to_adapter_check_func`/`py_to_control_check_func`
2. **控制流包装**（依赖少）：PyIfThenElse/PySwitch/PyCheck/PyStopIf + py_if/py_optional
3. **枚举包装**：PyEnum/PyFlagsEnum/PyMapping
4. **Hex 包装**：PyHex/PyHexDump
5. **适配器包装**（依赖双参数桥接）：PyExprAdapter/PySymmetricAdapter/PyValidator/PyExprValidator
6. **函数式构造器**：OneOf/NoneOf/Filter/Slicing/Indexing
7. **extract_subcon 扩展**：注册所有新包装类型
8. **纯 Python NamedTuple**：_namedtuple.py
9. **lib.rs 注册**：register_classes + register_functions
10. **单元测试 + 集成测试**
