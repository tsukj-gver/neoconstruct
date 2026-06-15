# 模块设计：PythonFFI-05 构造器包装层（原子 + 复合 + 运算符重载）

> 子任务 10.5（原子构造器）+ 10.6（复合构造器 + 运算符重载）合并设计文档。

## 模块位置

- `construct-py/src/constructs_atomic.rs` — 原子构造器 PyO3 包装（FormatField / Bytes / VarInt / Flag / Const / meta / computed）
- `construct-py/src/constructs_composite.rs` — 复合构造器 PyO3 包装（Struct / Sequence / Array / Union / Select / FocusedSeq / GreedyRange / RepeatUntil）
- `construct-py/src/py_renamed.rs` — Renamed PyO3 包装类 + 运算符重载核心逻辑（`/` `*` `+` `>>` `[]`）
- `construct-py/src/construct_macros.rs` — Rust 辅助宏（批量注册常量、批量实现 API 方法）

## 职责

将 Rust 内核（construct-rs）中已实现的原子与复合构造器，通过 PyO3 `#[pyclass]` 包装暴露给 Python 用户，使其能以与原版 construct 库**完全一致**的声明式语法（`Struct("name" / Byte, "data" / Bytes(4))`）定义解析器，并提供 7 个公共 API 方法（parse / parse_stream / parse_file / build / build_stream / build_file / sizeof）。

---

## 1. 核心设计决策

### 1.1 包装类架构：每个 Rust 构造器一个 `#[pyclass]`

**决策**：为每种构造器类型创建独立的 `#[pyclass]` 结构体，内部持有 `Box<dyn Construct>`（或具体类型的 owned 实例）。

**不使用**单一 `PyConstruct` 泛型包装类（`PyConstruct<T: Construct>`），原因：
1. PyO3 `#[pyclass]` 不支持泛型参数（每个 `#[pyclass]` 对应一个 Python 类型对象）
2. Python 用户需要 `isinstance(d, Struct)` 类型检查，每个构造器必须是独立的 Python 类型
3. 运算符重载的返回类型不同（`/` 返回 Renamed，`+` 返回 Struct），需要精确的类型

**包装类清单**（本子任务定义的 23 个 `#[pyclass]`）：

| 包装类 | 持有的 Rust 类型 | 类别 |
|--------|-----------------|------|
| `PyFormatField` | `FormatField`（owned） | 原子 |
| `PyBytes` | `Box<dyn Construct>`（Bytes 或 BytesExpr） | 原子 |
| `PyGreedyBytes` | `GreedyBytes`（owned） | 原子 |
| `PyBytesInteger` | `BytesInteger`（owned） | 原子 |
| `PyBitsInteger` | `BitsInteger`（owned） | 原子 |
| `PyVarInt` | `VarInt`（owned） | 原子 |
| `PyZigZag` | `ZigZag`（owned） | 原子 |
| `PyFlag` | `Flag`（owned） | 原子 |
| `PyConst` | `Const`（owned） | 原子 |
| `PyPass` | `Pass`（owned） | 原子 |
| `PyTerminated` | `Terminated`（owned） | 原子 |
| `PyTell` | `Tell`（owned） | 原子 |
| `PySeek` | `Box<dyn Construct>`（Seek 或 SeekExpr） | 原子 |
| `PyError` | `Error`（owned） | 原子 |
| `PyComputed` | `Computed`（owned） | 原子 |
| `PyRebuild` | `Rebuild`（owned） | 原子 |
| `PyDefault` | `Default`（owned） | 原子 |
| `PyIndex` | `Index`（owned） | 原子 |
| `PyRenamed` | `Renamed`（owned，扩展后） | 10.6 核心 |
| `PyStruct` | `Struct`（owned） | 复合 |
| `PySequence` | `Sequence`（owned） | 复合 |
| `PyArray` | `Box<dyn Construct>`（Array 或 ArrayExpr） | 复合 |
| `PyGreedyRange` | `GreedyRange`（owned） | 复合 |
| `PyRepeatUntil` | `RepeatUntil`（owned） | 复合 |
| `PyUnion` | `Union`（owned） | 复合 |
| `PySelect` | `Select`（owned） | 复合 |
| `PyFocusedSeq` | `FocusedSeq`（owned） | 复合 |
| `PyPadded` | `Padded`（owned） | 复合 |
| `PyAligned` | `Aligned`（owned） | 复合 |

### 1.2 公共 API 方法：委托 10.2 的 helper

每个包装类的 7 个方法（parse / parse_stream / parse_file / build / build_stream / build_file / sizeof）**完全委托**给 `api.rs` 的 7 个 helper 函数（10.2 已实现）。

为避免在 28 个类中重复编写相同的 7 个方法（196 个方法签名），设计一个 **Rust 辅助宏**：

```rust
/// 为 PyO3 包装类批量实现 7 个公共 API 方法。
///
/// 使用示例：
/// ```
/// impl_api_methods!(PyFormatField, |s| &s.inner as &dyn Construct);
/// ```
macro_rules! impl_api_methods {
    ($pyclass:ty, $accessor:expr) => {
        #[pymethods]
        impl $pyclass {
            fn parse(&self, py: Python<'_>, data: &PyAny, #[pyo3(keyword_args)] kw: &Bound<PyDict>) -> PyResult<PyObject> {
                let bytes = data.extract::<Cow<[u8]>>()?;
                let ctx_map = pykw_to_indexmap(py, kw)?;
                api::py_parse(py, $accessor(self), &bytes, ctx_map)
            }
            fn parse_stream(&self, py: Python<'_>, stream: PyObject, #[pyo3(keyword_args)] kw: &Bound<PyDict>) -> PyResult<PyObject> {
                let ctx_map = pykw_to_indexmap(py, kw)?;
                api::py_parse_stream(py, $accessor(self), stream, ctx_map)
            }
            // ... parse_file / build / build_stream / build_file / sizeof 同理
            fn sizeof(&self, py: Python<'_>, #[pyo3(keyword_args)] kw: &Bound<PyDict>) -> PyResult<usize> {
                let ctx_map = pykw_to_indexmap(py, kw)?;
                api::py_sizeof(py, $accessor(self), ctx_map)
            }
        }
    };
}
```

**注意**：宏内部的 `#[pymethods]` 是合法的（PyO3 支持 macro 展开的 `#[pymethods]`）。

### 1.3 常量导出策略：注册为模块属性

Rust 内核有 40 个 FormatField 常量 + 7 个 BytesInteger/BitsInteger 常量 + Python 别名（Byte/Short/Int/Long/Half/Single/Double）。在 `#[pymodule]` 中注册为模块级属性。

**方案**：手动注册每个常量（不使用宏批量注册），因为：
1. 每个 `pub const` 的 Rust 名称与 Python 导出名相同（如 `INT8UB`），一一映射
2. PyO3 的 `m.add("NAME", wrapped_value)` 需要逐个调用
3. 常量值是 `FormatField`（Rust struct），需包装为 `PyFormatField` 实例

```rust
fn register_constants(m: &Bound<PyModule>) -> PyResult<()> {
    // FormatField 常量（40 个）
    m.add("Int8ub",  PyFormatField { inner: construct::INT8UB  })?;
    m.add("Int8ul",  PyFormatField { inner: construct::INT8UL  })?;
    // ... 38 个 ...
    m.add("Float64b", PyFormatField { inner: construct::FLOAT64B })?;
    // 别名
    m.add("Byte",    m.getattr("Int8ub")?)?;  // 引用同一对象
    m.add("Short",   m.getattr("Int16ub")?)?;
    m.add("Int",     m.getattr("Int32ub")?)?;
    m.add("Long",    m.getattr("Int64ub")?)?;
    m.add("Half",    m.getattr("Float16b")?)?;
    m.add("Single",  m.getattr("Float32b")?)?;
    m.add("Double",  m.getattr("Float64b")?)?;
    // BytesInteger/BitsInteger 常量
    m.add("Int24ub", PyBytesInteger { inner: construct::INT24UB })?;
    // ... 3 个 ...
    m.add("Bit",     PyBitsInteger { inner: construct::BIT })?;
    m.add("Nibble",  PyBitsInteger { inner: construct::NIBBLE })?;
    m.add("Octet",   PyBitsInteger { inner: construct::OCTET })?;
    Ok(())
}
```

**Python 命名约定**：Python 原版使用**驼峰首字母小写**（`Int8ub`），而 Rust 使用**全大写**（`INT8UB`）。注册时使用 Python 命名。别名（`Byte`/`Short`/`Half` 等）通过 `getattr` 引用同一对象（非 clone），保持 `Int8ub is Byte` 为 True。

### 1.4 运算符重载归属：PyRenamed + 各包装类的 dunder 方法

Python 原版的 5 种运算符（`/` `*` `+` `>>` `[]`）定义在 `Construct` 基类（`core.py:732-784`），对所有构造器生效。

在 PyO3 中，运算符通过 `#[pymethods]` 的 dunder 方法实现。由于 PyO3 不支持"基类方法继承到 #[pyclass]"，每个包装类需要各自实现这些 dunder。

**方案**：用辅助宏批量实现。5 个运算符的语义：

| 运算符 | Python dunder | 触发条件 | 返回类型 |
|--------|-------------|---------|---------|
| `name / subcon` | `__rtruediv__` | 左操作数是 str | `PyRenamed` |
| `subcon * x` | `__mul__` | x 是 str 或 callable | `PyRenamed` |
| `x * subcon` | `__rmul__` | x 是 str 或 callable | `PyRenamed` |
| `a + b` | `__add__` | 两个构造器 | `PyStruct` |
| `a >> b` | `__rshift__` | 两个构造器 | `PySequence` |
| `subcon[n]` | `__getitem__` | n 是 int 或 callable | `PyArray` |

辅助宏：

```rust
macro_rules! impl_construct_operators {
    ($pyclass:ty, $accessor:expr) => {
        #[pymethods]
        impl $pyclass {
            /// `"name" / self` → Renamed
            fn __rtruediv__(&self, name: &str) -> PyResult<PyRenamed> {
                let inner = $accessor(self).clone_box();
                Ok(PyRenamed::from_rust(construct::Renamed::new(inner, name)))
            }

            /// `self * other` → Renamed(newdocs) 或 Renamed(newparsed)
            fn __mul__(&self, other: &PyAny) -> PyResult<PyObject> {
                py_mul_operator(other, $accessor(self))
            }

            /// `other * self` → Renamed(newdocs) 或 Renamed(newparsed)
            fn __rmul__(&self, other: &PyAny) -> PyResult<PyObject> {
                py_mul_operator(other, $accessor(self))
            }

            /// `self + other` → Struct
            fn __add__(&self, other: &PyAny) -> PyResult<PyStruct> {
                py_add_operator($accessor(self), other)
            }

            /// `self >> other` → Sequence
            fn __rshift__(&self, other: &PyAny) -> PyResult<PySequence> {
                py_rshift_operator($accessor(self), other)
            }

            /// `self[n]` → Array
            fn __getitem__(&self, count: &PyAny) -> PyResult<PyArray> {
                py_getitem_operator($accessor(self), count)
            }
        }
    };
}
```

**关键**：运算符辅助函数（`py_mul_operator` 等）处理混合分发——检查 other 是 PyO3 包装类还是纯 Python 构造器（通过 `extract_subcon`）。

### 1.5 混合分发：subcon 提取

当 Python 用户创建 `Struct("x" / Byte, "y" / MyAdapter(Byte))` 时，每个 subcon 可能是：
- **PyO3 包装类**（`PyFormatField`、`PyBytes` 等）→ 直接提取内部 `Box<dyn Construct>`
- **纯 Python 构造器**（用户自定义 `Adapter` 子类）→ 包装为 `PyConstructAdapter`

`extract_subcon` 函数已在 10.4 设计（`py_adapter.rs`）。10.5 需补充各 PyO3 包装类的提取分支。

**统一提取方案**：所有包装类实现一个 trait `PyConstructWrapper`：

```rust
/// PyO3 包装类的统一接口：提供对内部 Rust Construct 的访问。
pub trait PyConstructWrapper {
    fn as_construct(&self) -> &dyn Construct;
    fn clone_box_construct(&self) -> Box<dyn Construct>;
}
```

`extract_subcon` 改为：

```rust
pub fn extract_subcon(py: Python<'_>, obj: &PyAny) -> PyResult<Box<dyn Construct>> {
    // 尝试 1：遍历所有已注册的 PyO3 包装类型
    if let Ok(w) = obj.extract::<PyRef<PyFormatField>>() { return Ok(w.clone_box_construct()); }
    if let Ok(w) = obj.extract::<PyRef<PyBytes>>()       { return Ok(w.clone_box_construct()); }
    if let Ok(w) = obj.extract::<PyRef<PyStruct>>()      { return Ok(w.clone_box_construct()); }
    // ... 所有 28 个包装类 ...

    // 尝试 2：纯 Python 对象 → PyConstructAdapter（10.4 已实现）
    if is_construct_like(obj) {
        return Ok(Box::new(PyConstructAdapter::new(obj.into())));
    }

    Err(PyTypeError::new_err(format!(
        "Expected a Construct instance, got {}", obj.getattr("__class__")?
    )))
}
```

**性能优化**（后续）：用 `obj.get_type()` 的指针比较替代逐个 `extract` 尝试。Phase 10 优先正确性。

### 1.6 Context keyword 参数

已在 10.2 设计（`api.rs` 的 `pykw_to_indexmap`）：所有 7 个 API 方法的 `#[pyo3(signature = (..., **kw))]` 捕获 keyword 参数为 `Bound<PyDict>`，转为 `IndexMap<String, Value>` 注入为 Context 顶层条目。

本子任务的辅助宏 `impl_api_methods!` 在每个方法中调用 `pykw_to_indexmap(py, kw)` 并传给 helper。

---

## 2. 原子构造器包装设计（10.5）

### 2.1 FormatField + 40 常量

```rust
/// PyO3 包装：固定宽度数值字段。
#[pyclass(name = "FormatField")]
pub struct PyFormatField {
    pub(crate) inner: construct::FormatField,
}

impl PyConstructWrapper for PyFormatField {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
    fn clone_box_construct(&self) -> Box<dyn Construct> { Box::new(self.inner) }
}

// 工厂函数（Python 端 FormatField(endian, format)）
#[pyfunction]
#[pyo3(name = "FormatField")]
pub fn py_format_field(endian: &str, format: &str) -> PyResult<PyFormatField> {
    let endianness = match endian {
        "<" => Endianness::Little,
        ">" => Endianness::Big,
        "=" => Endianness::Native,
        _ => return Err(PyValueError::new_err("endian must be '<', '>', or '='")),
    };
    let kind = match format {
        "b" => FormatKind::I8, "B" => FormatKind::U8,
        "h" => FormatKind::I16, "H" => FormatKind::U16,
        "i" | "l" => FormatKind::I32, "I" | "L" => FormatKind::U32,
        "q" => FormatKind::I64, "Q" => FormatKind::U64,
        "e" => FormatKind::F16, "f" => FormatKind::F32, "d" => FormatKind::F64,
        _ => return Err(PyValueError::new_err(
            "format must be one of b/B/h/H/i/I/l/L/q/Q/e/f/d")),
    };
    Ok(PyFormatField { inner: construct::FormatField::new(endianness, kind) })
}

impl_api_methods!(PyFormatField, |s: &PyFormatField| &s.inner as &dyn Construct);
impl_construct_operators!(PyFormatField, |s: &PyFormatField| &s.inner as &dyn Construct);
```

**40 个常量**通过 §1.3 的 `register_constants` 注册。Python 用户 `from construct import Int8ub` 获得预构造的 `PyFormatField` 实例。

### 2.2 Bytes / GreedyBytes

```rust
/// PyO3 包装：固定或动态长度字节字段。
/// 持有 Bytes（固定）或 BytesExpr（动态），统一为 Box<dyn Construct>。
#[pyclass(name = "Bytes")]
pub struct PyBytes {
    pub(crate) inner: Box<dyn Construct>,
}

// 工厂函数
#[pyfunction]
#[pyo3(name = "Bytes", signature = (length))]
pub fn py_bytes(length: &PyAny) -> PyResult<PyBytes> {
    let py = length.py();
    // 双模式：int → Bytes，callable → BytesExpr
    if length.is_callable() {
        let expr = py_param_to_evaluate(py, length)?;
        Ok(PyBytes { inner: Box::new(construct::BytesExpr::new(expr)) })
    } else {
        let n = length.extract::<usize>()?;
        Ok(PyBytes { inner: Box::new(construct::Bytes::new(n)) })
    }
}

// GreedyBytes（单例）
#[pyclass(name = "GreedyBytes")]
pub struct PyGreedyBytes { pub(crate) inner: construct::GreedyBytes }

#[pyfunction]
#[pyo3(name = "GreedyBytes")]
pub fn py_greedy_bytes() -> PyGreedyBytes {
    PyGreedyBytes { inner: construct::GreedyBytes::new() }
}
```

### 2.3 BytesInteger / BitsInteger

```rust
#[pyclass(name = "BytesInteger")]
pub struct PyBytesInteger { pub(crate) inner: construct::BytesInteger }

#[pyfunction]
#[pyo3(name = "BytesInteger", signature = (length, signed=false, swapped=false))]
pub fn py_bytes_integer(length: usize, signed: bool, swapped: bool) -> PyBytesInteger {
    PyBytesInteger { inner: construct::BytesInteger::new(length, signed, swapped) }
}

#[pyclass(name = "BitsInteger")]
pub struct PyBitsInteger { pub(crate) inner: construct::BitsInteger }

#[pyfunction]
#[pyo3(name = "BitsInteger", signature = (length, signed=false, swapped=false))]
pub fn py_bits_integer(length: usize, signed: bool, swapped: bool) -> PyBitsInteger {
    PyBitsInteger { inner: construct::BitsInteger::new(length, signed, swapped) }
}
```

**常量**：`Int24ub/ul/sb/sl`（4 个）、`Bit`、`Nibble`、`Octet`（3 个）通过 register_constants 注册。

### 2.4 VarInt / ZigZag

```rust
#[pyclass(name = "VarInt")] pub struct PyVarInt { pub(crate) inner: construct::VarInt }
#[pyclass(name = "ZigZag")] pub struct PyZigZag { pub(crate) inner: construct::ZigZag }

#[pyfunction(name = "VarInt")] pub fn py_varint() -> PyVarInt { /* ... */ }
#[pyfunction(name = "ZigZag")] pub fn py_zigzag() -> PyZigZag { /* ... */ }
```

### 2.5 Flag

```rust
#[pyclass(name = "Flag")] pub struct PyFlag { pub(crate) inner: construct::Flag }
#[pyfunction(name = "Flag")] pub fn py_flag() -> PyFlag { /* ... */ }
```

### 2.6 Const

Python `Const(data)` 或 `Const(value, subcon)`。当只传一个参数时：
- `bytes` → `Const::new_bytes(data)` + 自动推导长度
- `int`/`str`/`float` → 需要 subcon（报错，Python 原版也要求 bytes 时可省略 subcon）

```rust
#[pyclass(name = "Const")]
pub struct PyConst { pub(crate) inner: construct::Const }

#[pyfunction]
#[pyo3(name = "Const", signature = (data, subcon=None))]
pub fn py_const(data: &PyAny, subcon: Option<&PyAny>) -> PyResult<PyConst> {
    let py = data.py();
    let value = crate::conversions::py_to_value(py, data)?;
    if let Some(sc) = subcon {
        let inner = extract_subcon(py, sc)?;
        Ok(PyConst { inner: construct::Const::new_value(value, inner) })
    } else {
        // 无 subcon：仅支持 bytes / str
        match &value {
            construct::Value::Bytes(b) => {
                Ok(PyConst { inner: construct::Const::new_bytes(b.clone()) })
            }
            _ => Err(PyTypeError::new_err(
                "Const requires a subcon when data is not bytes")),
        }
    }
}
```

### 2.7 Pass / Terminated / Tell / Seek / Error

```rust
#[pyclass(name = "Pass")]        pub struct PyPass { inner: construct::Pass }
#[pyclass(name = "Terminated")]  pub struct PyTerminated { inner: construct::Terminated }
#[pyclass(name = "Tell")]        pub struct PyTell { inner: construct::Tell }
#[pyclass(name = "Error")]       pub struct PyError { inner: construct::Error }

#[pyclass(name = "Seek")]
pub struct PySeek { inner: Box<dyn Construct> }  // Seek 或 SeekExpr

#[pyfunction]
#[pyo3(name = "Seek", signature = (at, whence=0))]
pub fn py_seek(at: &PyAny, whence: i32) -> PyResult<PySeek> {
    let py = at.py();
    if at.is_callable() {
        let expr = py_param_to_evaluate(py, at)?;
        let sw = match whence { 0=>Start, 1=>Current, 2=>End, _=>Err(...) };
        Ok(PySeek { inner: Box::new(construct::SeekExpr::with_whence(expr, sw)) })
    } else {
        let pos = at.extract::<i64>()?;
        let sw = match whence { 0=>Start, 1=>Current, 2=>End, _=>Err(...) };
        Ok(PySeek { inner: Box::new(construct::Seek::with_whence(pos, sw)) })
    }
}
```

### 2.8 Computed / Rebuild / Default / Index

```rust
#[pyclass(name = "Computed")]
pub struct PyComputed { inner: construct::Computed }

#[pyfunction]
#[pyo3(name = "Computed", signature = (func))]
pub fn py_computed(func: &PyAny) -> PyResult<PyComputed> {
    let py = func.py();
    let f = py_param_to_compute_func(py, func)?;
    Ok(PyComputed { inner: construct::Computed::new(f) })
}

#[pyclass(name = "Rebuild")]
pub struct PyRebuild { inner: construct::Rebuild }

#[pyfunction]
#[pyo3(name = "Rebuild", signature = (subcon, func))]
pub fn py_rebuild(subcon: &PyAny, func: &PyAny) -> PyResult<PyRebuild> {
    let py = func.py();
    let sc = extract_subcon(py, subcon)?;
    let f = py_param_to_compute_func(py, func)?;
    Ok(PyRebuild { inner: construct::Rebuild::new(sc, f) })
}

#[pyclass(name = "Default")]
pub struct PyDefault { inner: construct::Default }

#[pyfunction]
#[pyo3(name = "Default", signature = (subcon, value))]
pub fn py_default(subcon: &PyAny, value: &PyAny) -> PyResult<PyDefault> {
    let py = value.py();
    let sc = extract_subcon(py, subcon)?;
    let v = crate::conversions::py_to_value(py, value)?;
    Ok(PyDefault { inner: construct::Default::new(sc, v) })
}

#[pyclass(name = "Index")] pub struct PyIndex { inner: construct::Index }
#[pyfunction(name = "Index")] pub fn py_index() -> PyIndex { /* ... */ }
```

---

## 3. 复合构造器包装设计（10.6）

### 3.1 Struct

Python `Struct(*subcons, **subconskw)`。每个 subcon 经过 `/` 运算符后是 `Renamed`（具名）或裸构造器（匿名）。`**subconskw` 提供 `name=subcon` 键值对形式。

```rust
#[pyclass(name = "Struct")]
pub struct PyStruct {
    pub(crate) inner: construct::Struct,
    /// Python 端访问的 subcons 列表（用于 __add__ 合并和成员暴露）。
    pub(crate) py_subcons: Vec<PyObject>,
}

#[pyfunction]
#[pyo3(name = "Struct", signature = (*subcons, **subconskw))]
pub fn py_struct(
    py: Python<'_>,
    subcons: &Bound<PyTuple>,
    subconskw: &Bound<PyDict>,
) -> PyResult<PyStruct> {
    let mut builder = construct::Struct::new();
    let mut py_subcons = Vec::new();

    // 处理位置参数 *subcons
    for item in subcons.iter() {
        // 可能是单个构造器，也可能是列表/元组（list of subcons 拆包）
        if let Ok(list) = item.extract::<Vec<PyObject>>() {
            for sub in list {
                add_struct_subcon(py, &mut builder, &mut py_subcons, &sub.bind(py))?;
            }
        } else {
            add_struct_subcon(py, &mut builder, &mut py_subcons, item)?;
        }
    }

    // 处理 keyword 参数 **subconskw（name=subcon）
    for (key, val) in subconskw.iter() {
        let name: String = key.extract()?;
        let renamed = apply_rename(py, val, &name)?;
        let inner = extract_subcon(py, &renamed)?;
        builder = builder.field(name.clone(), inner);
        py_subcons.push(renamed);
    }

    Ok(PyStruct { inner: builder, py_subcons })
}

/// 将一个 subcon 添加到 Struct builder。
/// 如果 subcon 是 PyRenamed，提取其 name 和 inner。
fn add_struct_subcon(
    py: Python<'_>,
    builder: &mut construct::Struct,
    py_subcons: &mut Vec<PyObject>,
    obj: &Bound<PyAny>,
) -> PyResult<()> {
    let inner = extract_subcon(py, obj)?;
    // 检查是否有名称（PyRenamed 或纯 Python Renamed）
    let name = get_subcon_name(obj)?;
    match name {
        Some(n) => { *builder = builder.field(n, inner); }
        None    => { *builder = builder.anonymous(inner); }
    }
    py_subcons.push(obj.into_py(py));
    Ok(())
}
```

**成员属性暴露**（`d.animal.giraffe`）：

Python 原版 `Struct.__getattr__` 查找 `self._subcons` 字典（具名子构造器）。PyO3 实现：

```rust
#[pymethods]
impl PyStruct {
    fn __getattr__(&self, py: Python<'_>, name: &str) -> PyResult<PyObject> {
        // 查找 py_subcons 中名称匹配的子构造器
        for sub in &self.py_subcons {
            if let Some(n) = get_subcon_name(sub.bind(py)).ok().flatten() {
                if n == name {
                    return Ok(sub.clone_ref(py));
                }
            }
        }
        Err(PyAttributeError::new_err(format!(
            "Struct has no field '{}'", name)))
    }
}
```

### 3.2 Sequence

Python `Sequence(*subcons, **subconskw)`。与 Struct 类似，但解析为 `Value::List`。

```rust
#[pyclass(name = "Sequence")]
pub struct PySequence {
    pub(crate) inner: construct::Sequence,
    pub(crate) py_subcons: Vec<PyObject>,
}

#[pyfunction]
#[pyo3(name = "Sequence", signature = (*subcons, **subconskw))]
pub fn py_sequence(py: Python<'_>, subcons: &Bound<PyTuple>, subconskw: &Bound<PyDict>) -> PyResult<PySequence> {
    let mut builder = construct::Sequence::new();
    let mut py_subcons = Vec::new();
    for item in subcons.iter() {
        let inner = extract_subcon(py, item)?;
        let name = get_subcon_name(item)?;
        match name {
            Some(n) => builder = builder.named(n, inner),  // Sequence 有 named 方法
            None    => builder = builder.push(inner),
        }
        py_subcons.push(item.into_py(py));
    }
    // **subconskw 处理同 Struct
    Ok(PySequence { inner: builder, py_subcons })
}
```

### 3.3 Array

Python `Array(count, subcon)`。count 可以是 int 或表达式。

```rust
#[pyclass(name = "Array")]
pub struct PyArray { pub(crate) inner: Box<dyn Construct> }

#[pyfunction]
#[pyo3(name = "Array", signature = (count, subcon))]
pub fn py_array(count: &PyAny, subcon: &PyAny) -> PyResult<PyArray> {
    let py = count.py();
    let sc = extract_subcon(py, subcon)?;
    if count.is_callable() {
        let expr = py_param_to_evaluate(py, count)?;
        Ok(PyArray { inner: Box::new(construct::ArrayExpr::new(expr, sc)) })
    } else {
        let n = count.extract::<usize>()?;
        Ok(PyArray { inner: Box::new(construct::Array::new(n, sc)) })
    }
}
```

**`subcon[n]` 语法**（`__getitem__`）：`Byte[5]` → `Array(5, Byte)`，在运算符宏中实现（§4.4）。

### 3.4 GreedyRange / RepeatUntil

```rust
#[pyclass(name = "GreedyRange")]
pub struct PyGreedyRange { pub(crate) inner: construct::GreedyRange }

#[pyfunction]
#[pyo3(name = "GreedyRange", signature = (subcon, discard=false))]
pub fn py_greedy_range(subcon: &PyAny, discard: bool) -> PyResult<PyGreedyRange> {
    let py = subcon.py();
    let sc = extract_subcon(py, subcon)?;
    let inner = if discard {
        construct::GreedyRange::new_discard(sc)
    } else {
        construct::GreedyRange::new(sc)
    };
    Ok(PyGreedyRange { inner })
}

#[pyclass(name = "RepeatUntil")]
pub struct PyRepeatUntil { pub(crate) inner: construct::RepeatUntil }

#[pyfunction]
#[pyo3(name = "RepeatUntil", signature = (func, subcon, discard=false))]
pub fn py_repeat_until(func: &PyAny, subcon: &PyAny, discard: bool) -> PyResult<PyRepeatUntil> {
    let py = func.py();
    let predicate = py_to_repeat_predicate(func.into_py(py))?;  // 10.4 已设计
    let sc = extract_subcon(py, subcon)?;
    let inner = if discard {
        construct::RepeatUntil::new_discard(predicate, sc)
    } else {
        construct::RepeatUntil::new(predicate, sc)
    };
    Ok(PyRepeatUntil { inner })
}
```

### 3.5 Union / Select / FocusedSeq

```rust
#[pyclass(name = "Union")]
pub struct PyUnion { pub(crate) inner: construct::Union }

#[pyfunction]
#[pyo3(name = "Union", signature = (parsefrom, *subcons, **subconskw))]
pub fn py_union(parsefrom: &PyAny, subcons: &Bound<PyTuple>, subconskw: &Bound<PyDict>) -> PyResult<PyUnion> {
    let py = parsefrom.py();
    // parsefrom: int → Index, str → Name, None
    let target = if parsefrom.is_none() {
        None
    } else if let Ok(i) = parsefrom.extract::<usize>() {
        Some(construct::UnionTarget::Index(i))
    } else if let Ok(s) = parsefrom.extract::<String>() {
        Some(construct::UnionTarget::Name(s))
    } else {
        return Err(PyTypeError::new_err("parsefrom must be int, str, or None"));
    };
    // 收集 subcons 为 Vec<StructField>
    let mut fields = Vec::new();
    for item in subcons.iter() {
        let inner = extract_subcon(py, item)?;
        let name = get_subcon_name(item)?;
        match name {
            Some(n) => fields.push(construct::StructField::new(n, inner)),
            None    => fields.push(construct::StructField::anonymous(inner)),
        }
    }
    Ok(PyUnion { inner: construct::Union::new(target, fields) })
}

#[pyclass(name = "Select")]
pub struct PySelect { pub(crate) inner: construct::Select }

#[pyfunction]
#[pyo3(name = "Select", signature = (*subcons, **subconskw))]
pub fn py_select(subcons: &Bound<PyTuple>, subconskw: &Bound<PyDict>) -> PyResult<PySelect> {
    let py = subcons.py();
    let mut subs: Vec<Box<dyn Construct>> = Vec::new();
    for item in subcons.iter() {
        subs.push(extract_subcon(py, item)?);
    }
    Ok(PySelect { inner: construct::Select::new(subs) })
}

#[pyclass(name = "FocusedSeq")]
pub struct PyFocusedSeq { pub(crate) inner: construct::FocusedSeq }

#[pyfunction]
#[pyo3(name = "FocusedSeq", signature = (parsebuildfrom, *subcons, **subconskw))]
pub fn py_focused_seq(parsebuildfrom: &str, subcons: &Bound<PyTuple>, subconskw: &Bound<PyDict>) -> PyResult<PyFocusedSeq> {
    // 类似 Struct 的 subcons 收集 + focus 字段名
    // ...
}
```

### 3.6 Padding / Padded / Aligned / AlignedStruct / BitStruct

**Padding（函数式宏）**：Python `Padding(length)` 返回 `Padded(length, Pass)`。在 PyO3 层实现为：

```rust
#[pyfunction]
#[pyo3(name = "Padding", signature = (length, pattern=b"\x00"))]
pub fn py_padding(length: usize, pattern: &[u8]) -> PyResult<PyPadded> {
    let pad = pattern.get(0).copied().unwrap_or(0x00);
    Ok(PyPadded {
        inner: construct::Padded::new(length, Box::new(construct::Pass::new()), pad, false)
    })
}
```

**Padded / Aligned**：

```rust
#[pyclass(name = "Padded")]
pub struct PyPadded { pub(crate) inner: construct::Padded }

#[pyfunction]
#[pyo3(name = "Padded", signature = (length, subcon, pattern=b"\x00"))]
pub fn py_padded(length: usize, subcon: &PyAny, pattern: &[u8]) -> PyResult<PyPadded> {
    let sc = extract_subcon(subcon.py(), subcon)?;
    let pad = pattern.get(0).copied().unwrap_or(0x00);
    Ok(PyPadded { inner: construct::Padded::new(length, sc, pad, false) })
}

#[pyclass(name = "Aligned")]
pub struct PyAligned { pub(crate) inner: construct::Aligned }

#[pyfunction]
#[pyo3(name = "Aligned", signature = (modulus, subcon, pattern=b"\x00"))]
pub fn py_aligned(modulus: usize, subcon: &PyAny, pattern: &[u8]) -> PyResult<PyAligned> {
    let sc = extract_subcon(subcon.py(), subcon)?;
    let pad = pattern.get(0).copied().unwrap_or(0x00);
    Ok(PyAligned { inner: construct::Aligned::new(modulus, sc, pad) })
}
```

**AlignedStruct / BitStruct（Python 函数）**：

这两个在 Python 原版是**普通函数**（非类）。在 PyO3 中作为 `#[pyfunction]` 实现：

```rust
/// `AlignedStruct(modulus, *subcons)` → Struct(每个字段用 Aligned 包装)
#[pyfunction]
#[pyo3(name = "AlignedStruct", signature = (modulus, *subcons, **subconskw))]
pub fn py_aligned_struct(
    py: Python<'_>,
    modulus: usize,
    subcons: &Bound<PyTuple>,
    subconskw: &Bound<PyDict>,
) -> PyResult<PyStruct> {
    // 对每个 subcon：提取 name + inner → Aligned(modulus, inner) → Renamed(aligned, name)
    let mut builder = construct::Struct::new();
    let mut py_subcons = Vec::new();
    for item in subcons.iter() {
        let inner = extract_subcon(py, item)?;
        let name = get_subcon_name(item)?.ok_or_else(|| {
            PyValueError::new_err("AlignedStruct requires named subcons")
        })?;
        let aligned = construct::Aligned::new(modulus, inner, 0x00);
        builder = builder.field(name.clone(), Box::new(aligned));
        py_subcons.push(item.into_py(py));
    }
    Ok(PyStruct { inner: builder, py_subcons })
}

/// `BitStruct(*subcons)` → Bitwise(Struct(*subcons))
/// Bitwise 在 10.8 实现。此处先委托到 10.8 的包装。
#[pyfunction]
#[pyo3(name = "BitStruct", signature = (*subcons, **subconskw))]
pub fn py_bit_struct(subcons: &Bound<PyTuple>, subconskw: &Bound<PyDict>) -> PyResult<PyObject> {
    // 先构造 PyStruct，再用 Bitwise 包装
    // Bitwise 的 PyO3 包装在 10.8 定义，此处依赖前向声明或运行时查找
    // 10.6 阶段：返回 PyStruct，BitStruct 的完整功能在 10.8 完成后生效
    todo_in_10_8(subcons, subconskw)
}
```

**注意**：`BitStruct` 依赖 `Bitwise`（10.8）。设计建议 10.6 先实现 `AlignedStruct`（无外部依赖），`BitStruct` 标记为 **10.6 依赖 10.8 的前向引用**，在 10.8 Bitwise 包装完成后集成。如果 PM 希望在 10.6 独立完成，可用纯 Python fallback 实现 BitStruct（调用 Bitwise）。

---

## 4. Renamed 包装类与运算符重载

### 4.1 Renamed 的 docs/parsed 支持（需 PM 裁定）

**问题**：Python `Renamed(subcon, newname, newdocs, newparsed)` 有 4 个参数。Rust 内核 `Renamed`（core/mod.rs:237）只有 `inner` + `name`，不支持 `docs`（docstring）和 `parsed`（parse 回调 hook）。

**影响**：
- `docs`：仅用于 `__repr__` 和 KSY 导出。Python 用户 `Byte * "documentation"` 设置 docstring。影响 `__repr__` 输出。
- `parsed`：在 `_parsereport` 中调用 `self.parsed(obj, context)`。用于调试/监控解析结果。

**方案对比**：

| 方案 | 描述 | 优点 | 缺点 |
|------|------|------|------|
| **A: 扩展 Rust Renamed** | 向 `construct::Renamed` 添加 `docs: Option<String>` 和 `parsed: Option<Box<dyn Fn(&Value, &Context) + Send + Sync>>` | 行为完全一致；parsed hook 在 Rust parse 路径中触发 | 修改 Phase 1 已验收代码（core/mod.rs） |
| B: PyO3 层处理 | PyRenamed 持有额外字段，在 PyRenamed::parse 的 API 层调用 parsed hook | 不修改 Rust 内核 | parsed hook 仅在 PyO3 入口触发，Rust 内部调用 Renamed::parse 时不触发（行为差异） |
| C: 忽略 docs/parsed | 不实现 `*` 运算符的 docs/parsed 功能 | 最简单 | 测试用例可能失败（`test_operator_doc`） |

**建议方案 A**（扩展 Rust Renamed），理由：
1. parsed hook 必须在 parse 成功后立即触发，无论调用来自 PyO3 还是 Rust 内核（如 Struct 内部字段）
2. 修改面小：Renamed 加 2 个 `Option` 字段，默认 None，不影响现有行为
3. Construct trait 不变，已验收的 1306 测试不受影响（Renamed 现有测试不涉及 docs/parsed）

**扩展后的 Rust Renamed**：

```rust
// construct-rs/src/core/mod.rs（需 PM 授权修改）
pub struct Renamed {
    pub inner: Box<dyn Construct>,
    pub name: String,
    /// Docstring（可选）。对应 Python Renamed.docs。
    pub docs: Option<String>,
    /// Parsed hook（可选）。对应 Python Renamed.parsed。
    /// 在 parse 成功后调用。
    pub parsed: Option<Box<dyn Fn(&Value, &Context) + Send + Sync>>,
}

impl Renamed {
    pub fn new(inner: Box<dyn Construct>, name: impl Into<String>) -> Self {
        Self { inner, name: name.into(), docs: None, parsed: None }
    }

    /// Builder: 设置 docstring。
    pub fn with_docs(mut self, docs: impl Into<String>) -> Self {
        self.docs = Some(docs.into()); self
    }

    /// Builder: 设置 parsed hook。
    pub fn with_parsed(mut self, hook: Box<dyn Fn(&Value, &Context) + Send + Sync>) -> Self {
        self.parsed = Some(hook); self
    }
}

impl Construct for Renamed {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let obj = self.inner.parse(stream, ctx)
            .map_err(|e| e.with_path_prefix(&self.name))?;
        if let Some(hook) = &self.parsed {
            hook(&obj, ctx);
        }
        Ok(obj)
    }
    // build / sizeof 不变（加 with_path_prefix）
}
```

**需 PM 裁定**：是否授权修改 `construct-rs/src/core/mod.rs` 的 `Renamed` 结构体。此修改不影响 Phase 1 已验收行为（新字段默认 None），但需重新运行 construct-rs 全部测试确认无回归。

### 4.2 PyRenamed 包装类

```rust
/// PyO3 包装：具名构造器（/ 和 * 运算符的产物）。
#[pyclass(name = "Renamed")]
pub struct PyRenamed {
    pub(crate) inner: construct::Renamed,
    /// Python 端持有的 subcon 引用（用于 __getattr__ 委托）。
    pub(crate) py_subcon: PyObject,
}

impl PyConstructWrapper for PyRenamed {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
    fn clone_box_construct(&self) -> Box<dyn Construct> { Box::new(self.inner.clone_for_box()) }
}
```

**`__getattr__` 委托**：Python 原版 `Renamed.__getattr__` 返回 `getattr(self.subcon, name)`。PyO3 实现：

```rust
#[pymethods]
impl PyRenamed {
    fn __getattr__(&self, py: Python<'_>, name: &str) -> PyResult<PyObject> {
        // 委托到内部 subcon 的属性
        self.py_subcon.getattr(py, name)
            .map_err(|_| PyAttributeError::new_err(format!(
                "Renamed subcon has no attribute '{}'", name)))
    }

    /// `.subcon` 属性暴露（Python 原版 Renamed 继承 Subconstruct，有 .subcon）
    #[getter]
    fn subcon(&self, py: Python<'_>) -> PyObject {
        self.py_subcon.clone_ref(py)
    }

    /// `.name` 属性暴露
    #[getter]
    fn name(&self) -> &str { &self.inner.name }

    /// `.docs` 属性暴露
    #[getter]
    fn docs(&self) -> Option<&str> { self.inner.docs.as_deref() }
}
```

**构造方式**：

```rust
impl PyRenamed {
    /// 从 Rust Renamed 创建（运算符重载路径）。
    pub fn from_rust(py: Python<'_>, renamed: construct::Renamed, py_subcon: PyObject) -> Self {
        PyRenamed { inner: renamed, py_subcon }
    }

    /// Python 构造器 Renamed(subcon, newname, newdocs, newparsed)。
    #[new]
    #[pyo3(signature = (subcon, newname=None, newdocs=None, newparsed=None))]
    fn new(py: Python<'_>, subcon: &PyAny, newname: Option<&str>,
           newdocs: Option<&str>, newparsed: Option<&PyAny>) -> PyResult<Self> {
        let inner = extract_subcon(py, subcon)?;
        let name = newname.unwrap_or("").to_string();
        let mut renamed = construct::Renamed::new(inner, name);
        if let Some(d) = newdocs { renamed = renamed.with_docs(d); }
        if let Some(h) = newparsed {
            let hook = py_to_parsed_hook(h.into_py(py))?;
            renamed = renamed.with_parsed(hook);
        }
        Ok(PyRenamed { inner: renamed, py_subcon: subcon.into_py(py) })
    }
}
```

### 4.3 `/` 运算符（`__rtruediv__`）

Python `"name" / Byte` 的求值顺序：
1. Python 先调用 `str.__truediv__("name", Byte)` → 返回 `NotImplemented`（str 不支持）
2. Python 回退到 `Byte.__rtruediv__("name")` → 返回 `Renamed(Byte, "name")`

PyO3 实现（在运算符宏 `impl_construct_operators!` 中）：

```rust
fn __rtruediv__(&self, name: &str) -> PyResult<PyRenamed> {
    let inner = self.as_construct().clone_box();
    let renamed = construct::Renamed::new(inner, name);
    Ok(PyRenamed::from_rust(Python::with_gil(|py| py), renamed,
        self_as_pyobj /* self 的 PyObject 引用 */))
}
```

**注意**：`__rtruediv__` 需要 self 的 Python 引用（`PyRef<PyFormatField>`）来构造 `py_subcon`。PyO3 的 `__rtruediv__` 方法签名可接受 `&PyRef<Self>` 或使用 `slf: Py<Self>`。

### 4.4 `*` 运算符（`__mul__` / `__rmul__`）

```rust
/// `subcon * other` 或 `other * subcon` → Renamed(newdocs) 或 Renamed(newparsed)
fn py_mul_operator(other: &PyAny, construct_obj: &dyn Construct) -> PyResult<PyObject> {
    let py = other.py();
    if let Ok(s) = other.extract::<String>() {
        // str → newdocs
        let inner = construct_obj.clone_box();
        let renamed = construct::Renamed::new(inner, "").with_docs(s);
        // 包装为 PyRenamed 并返回为 PyObject
        Ok(Py::new(py, PyRenamed::from_rust(py, renamed, other.into_py(py)))?.into_any())
    } else if other.is_callable() {
        // callable → newparsed
        let hook = py_to_parsed_hook(other.into_py(py))?;
        let inner = construct_obj.clone_box();
        let renamed = construct::Renamed::new(inner, "").with_parsed(hook);
        Ok(Py::new(py, PyRenamed::from_rust(py, renamed, other.into_py(py)))?.into_any())
    } else {
        Err(construct_error_to_py(py, "operator * can only be used with string or lambda"))
    }
}

/// 将 Python callable 转换为 parsed hook。
fn py_to_parsed_hook(callable: PyObject) -> PyResult<Box<dyn Fn(&Value, &Context) + Send + Sync>> {
    Ok(Box::new(move |obj: &Value, ctx: &Context| {
        let _ = Python::with_gil(|py| {
            let py_obj = crate::conversions::value_to_py(py, obj).ok()?;
            let py_ctx = crate::conversions::context_to_py_container(py, ctx).ok()?;
            callable.call1(py, (py_obj, py_ctx)).ok()
        });
    }))
}
```

### 4.5 `+` 运算符（`__add__`）

```rust
/// `a + b` → Struct(a, b)
/// 如果 a 或 b 已经是 Struct，合并其 subcons。
fn py_add_operator(lhs: &dyn Construct, other: &PyAny) -> PyResult<PyStruct> {
    let py = other.py();
    // 收集左侧 subcons
    let mut lhs_subs: Vec<(Option<String>, Box<dyn Construct>)> = Vec::new();
    let mut lhs_py_subs: Vec<PyObject> = Vec::new();

    // 检查 lhs 是否是 PyStruct（合并而非嵌套）
    // 注：lhs 来自 self.as_construct()，无法直接判断是否是 Struct
    // 需要从 Python 侧传入 self 的 PyObject 判断

    collect_subcons_for_merge(py, lhs_obj, &mut lhs_subs, &mut lhs_py_subs)?;
    collect_subcons_for_merge(py, other, &mut rhs_subs, &mut rhs_py_subs)?;

    let mut builder = construct::Struct::new();
    for (name, sc) in lhs_subs.into_iter().chain(rhs_subs.into_iter()) {
        match name {
            Some(n) => builder = builder.field(n, sc),
            None => builder = builder.anonymous(sc),
        }
    }
    Ok(PyStruct { inner: builder, py_subcons: lhs_py_subs.into_iter().chain(rhs_py_subs).collect() })
}
```

**注意**：`__add__` 需要访问 self 的 Python 对象以判断是否是 `PyStruct`（合并 subcons）。PyO3 的 `__add__` 签名为 `fn __add__(slf: PyRef<Self>, other: &PyAny) -> PyResult<PyStruct>`，`slf` 提供了 Python 引用。

### 4.6 `>>` 运算符（`__rshift__`）

与 `__add__` 逻辑相同，但构建 `Sequence` 而非 `Struct`：

```rust
fn __rshift__(slf: PyRef<Self>, other: &PyAny) -> PyResult<PySequence> {
    // 收集两侧 subcons → construct::Sequence::new().push(...)
    // 如果一侧已是 Sequence，合并其 entries
}
```

### 4.7 `[]` 运算符（`__getitem__`）

```rust
/// `subcon[n]` → Array(n, subcon)
fn py_getitem_operator(construct_obj: &dyn Construct, count: &PyAny) -> PyResult<PyArray> {
    let py = count.py();
    let sc = construct_obj.clone_box();
    if count.is_callable() {
        let expr = py_param_to_evaluate(py, count)?;
        Ok(PyArray { inner: Box::new(construct::ArrayExpr::new(expr, sc)) })
    } else {
        let n = count.extract::<usize>()?;
        Ok(PyArray { inner: Box::new(construct::Array::new(n, sc)) })
    }
}
```

**slice 检查**：Python 原版 `__getitem__` 检查 `isinstance(count, slice)` 并抛 `ConstructError`（提示用 GreedyRange）。PyO3 实现：

```rust
fn __getitem__(&self, count: &PyAny) -> PyResult<PyArray> {
    // 检查 slice
    if count.is_instance_of::<PySlice>()? {
        return Err(crate::exceptions::ConstructError::new_err(
            "subcon[N] syntax can only be used for Arrays, use GreedyRange(subcon) instead?"
        ));
    }
    py_getitem_operator(self.as_construct(), count)
}
```

---

## 5. 辅助函数

### 5.1 get_subcon_name

从 Python 对象提取构造器的名称（如果有）。

```rust
/// 从 Python 构造器对象提取名称。
///
/// - PyRenamed → Some(self.inner.name)（可能为空字符串 ""，视为匿名）
/// - 纯 Python Renamed → getattr(obj, "name", None)，非空非"_" 返回 Some
/// - 其他构造器 → None
fn get_subcon_name(obj: &Bound<PyAny>) -> PyResult<Option<String>> {
    // 尝试 PyRenamed
    if let Ok(r) = obj.extract::<PyRef<PyRenamed>>() {
        let name = &r.inner.name;
        return Ok(if name.is_empty() || name == "_" { None } else { Some(name.clone()) });
    }
    // 尝试纯 Python 对象的 .name 属性
    if let Ok(name_attr) = obj.getattr("name") {
        if name_attr.is_none() { return Ok(None); }
        if let Ok(s) = name_attr.extract::<String>() {
            return Ok(if s.is_empty() || s == "_" { None } else { Some(s) });
        }
    }
    Ok(None)
}
```

### 5.2 apply_rename

对 Python 对象应用 `/` 运算符的命名效果（用于 `**subconskw` 处理）。

```rust
/// 等价于 `name / subcon`，返回 PyRenamed。
fn apply_rename(py: Python<'_>, subcon: &Bound<PyAny>, name: &str) -> PyResult<PyObject> {
    // 如果 subcon 是 PyO3 包装类，调用其 __rtruediv__
    // 直接构造 PyRenamed 更高效
    let inner = extract_subcon(py, subcon)?;
    let renamed = construct::Renamed::new(inner, name);
    let py_r = PyRenamed { inner: renamed, py_subcon: subcon.into_py(py) };
    Ok(Py::new(py, py_r)?.into_any())
}
```

### 5.3 collect_subcons_for_merge

为 `__add__` / `__rshift__` 收集 subcons（支持 Struct/Sequence 合并）。

```rust
/// 收集构造器及其 subcons 列表（用于 + / >> 合并）。
///
/// 如果 obj 是 PyStruct/PySequence，展开其 py_subcons。
/// 否则，obj 作为单个 subcon。
fn collect_subcons_for_merge(
    py: Python<'_>,
    obj: &Bound<PyAny>,
    out_subs: &mut Vec<(Option<String>, Box<dyn Construct>)>,
    out_py: &mut Vec<PyObject>,
) -> PyResult<()> {
    // PyStruct → 展开 py_subcons
    if let Ok(s) = obj.extract::<PyRef<PyStruct>>() {
        for sub in &s.py_subcons {
            let inner = extract_subcon(py, sub.bind(py))?;
            let name = get_subcon_name(sub.bind(py))?;
            out_subs.push((name, inner));
            out_py.push(sub.clone_ref(py));
        }
        return Ok(());
    }
    // PySequence → 同理
    if let Ok(s) = obj.extract::<PyRef<PySequence>>() {
        for sub in &s.py_subcons {
            let inner = extract_subcon(py, sub.bind(py))?;
            let name = get_subcon_name(sub.bind(py))?;
            out_subs.push((name, inner));
            out_py.push(sub.clone_ref(py));
        }
        return Ok(());
    }
    // 单个构造器
    let inner = extract_subcon(py, obj)?;
    let name = get_subcon_name(obj)?;
    out_subs.push((name, inner));
    out_py.push(obj.into_py(py));
    Ok(())
}
```

### 5.4 Renamed 的 clone_box 问题

`Renamed` 持有 `Box<dyn Construct>`，而 `dyn Construct` 不要求 `Clone`。`extract_subcon` 需要从 PyRenamed 克隆出 `Box<dyn Construct>`。

**问题**：`Renamed` 内部的 `inner: Box<dyn Construct>` 无法 clone（Construct 无 Clone 约束）。

**方案**：PyRenamed 的 `clone_box_construct` 不 clone `Renamed.inner`，而是重新从 `py_subcon` 提取：

```rust
impl PyConstructWrapper for PyRenamed {
    fn clone_box_construct(&self) -> Box<dyn Construct> {
        // 方案 1：如果 inner 无 parsed hook，直接 clone（需要 Renamed 实现 Clone）
        // 方案 2：从 py_subcon 重新 extract
        // 推荐：Renamed 实现基于 inner 的 clone（见下）
    }
}
```

**推荐**：为 `construct::Renamed` 的 `inner` 提供一个 `clone_box` 方法（通过 `Construct` trait 的扩展）。但这需要修改 Construct trait。

**备选方案**（不修改 Construct trait）：`PyRenamed` 的 `as_construct()` 返回 `&self.inner`（借用，不 clone）。仅在 `extract_subcon` 需要 owned `Box<dyn Construct>` 时才 clone。由于 PyRenamed 持有 `py_subcon`（Python 引用），可以从 py_subcon 重新 extract。但性能差（每次都穿越 FFI）。

**PM 需裁定**：是否为 `Construct` trait 添加 `fn clone_box(&self) -> Box<dyn Construct>` 方法（修改 Phase 1 已验收代码），还是接受 PyRenamed 每次 clone 时从 Python 重新 extract。

---

## 6. Python API 映射表

### 6.1 原子构造器（10.5）

| Python 原版 | Rust 内核类型 | PyO3 包装类 | 映射说明 |
|------------|-------------|------------|---------|
| `FormatField(endian, format)` | `FormatField` | `PyFormatField` | endian `<`/`>`/`=` → Endianness enum |
| `Int8ub` … `Float64n` (40) | `INT8UB` … `FLOAT64N` | `PyFormatField` 实例 | register_constants 注册 |
| `Byte`/`Short`/`Int`/`Long`/`Half`/`Single`/`Double` | 别名 | 同上对象 | getattr 引用 |
| `Bytes(integer)` | `Bytes` 或 `BytesExpr` | `PyBytes` | int→Bytes, callable→BytesExpr |
| `GreedyBytes` | `GreedyBytes` | `PyGreedyBytes` | 单例 |
| `BytesInteger(n, signed, swapped)` | `BytesInteger` | `PyBytesInteger` | |
| `BitsInteger(n, signed, swapped)` | `BitsInteger` | `PyBitsInteger` | |
| `Int24ub/ul/sb/sl` (4) | `INT24UB` 等 | `PyBytesInteger` 实例 | 常量注册 |
| `Bit`/`Nibble`/`Octet` | `BIT`/`NIBBLE`/`OCTET` | `PyBitsInteger` 实例 | 常量注册 |
| `VarInt` | `VarInt` | `PyVarInt` | 单例 |
| `ZigZag` | `ZigZag` | `PyZigZag` | 单例 |
| `Flag` | `Flag` | `PyFlag` | 单例 |
| `Const(data, subcon)` | `Const` | `PyConst` | bytes 自动推导 subcon |
| `Pass` | `Pass` | `PyPass` | 单例 |
| `Terminated` | `Terminated` | `PyTerminated` | 单例 |
| `Tell` | `Tell` | `PyTell` | 单例 |
| `Seek(at, whence)` | `Seek` / `SeekExpr` | `PySeek` | int→Seek, callable→SeekExpr |
| `Error` | `Error` | `PyError` | 单例 |
| `Computed(func)` | `Computed` | `PyComputed` | func→ComputeFunc |
| `Rebuild(subcon, func)` | `Rebuild` | `PyRebuild` | |
| `Default(subcon, value)` | `Default` | `PyDefault` | |
| `Index` | `Index` | `PyIndex` | 单例 |

### 6.2 复合构造器 + 运算符（10.6）

| Python 原版 | Rust 内核类型 | PyO3 包装类 | 映射说明 |
|------------|-------------|------------|---------|
| `Renamed(subcon, newname, newdocs, newparsed)` | `Renamed`（扩展后） | `PyRenamed` | 需扩展 Rust Renamed（§4.1） |
| `Struct(*subcons, **subconskw)` | `Struct` + `StructField` | `PyStruct` | builder 模式收集 |
| `Sequence(*subcons, **subconskw)` | `Sequence` + `SeqEntry` | `PySequence` | builder 模式收集 |
| `Array(count, subcon)` | `Array` / `ArrayExpr` | `PyArray` | int→Array, callable→ArrayExpr |
| `GreedyRange(subcon, discard)` | `GreedyRange` | `PyGreedyRange` | |
| `RepeatUntil(func, subcon, discard)` | `RepeatUntil` | `PyRepeatUntil` | func→RepeatPredicate |
| `Union(parsefrom, *subcons)` | `Union` + `UnionTarget` | `PyUnion` | parsefrom int/str/None |
| `Select(*subcons)` | `Select` | `PySelect` | |
| `FocusedSeq(parsebuildfrom, *subcons)` | `FocusedSeq` | `PyFocusedSeq` | |
| `Padding(length, pattern)` | `Padded(length, Pass)` | `PyPadded` | 函数式 |
| `Padded(length, subcon, pattern)` | `Padded` | `PyPadded` | |
| `Aligned(modulus, subcon, pattern)` | `Aligned` | `PyAligned` | |
| `AlignedStruct(modulus, *subcons)` | `Struct`（组合） | `PyStruct` | 函数式：每字段 Aligned 包装 |
| `BitStruct(*subcons)` | `Bitwise(Struct(...))` | 依赖 10.8 | 前向引用 |
| `"name" / subcon` | `Renamed::new(sc, "name")` | `__rtruediv__` | 运算符 |
| `subcon * "docs"` | `Renamed::with_docs` | `__mul__` | 运算符 |
| `subcon * callback` | `Renamed::with_parsed` | `__mul__` | 运算符 |
| `"docs" * subcon` | 同上 | `__rmul__` | 运算符 |
| `a + b` | `Struct(a, b)` | `__add__` | 运算符 |
| `a >> b` | `Sequence(a, b)` | `__rshift__` | 运算符 |
| `subcon[n]` | `Array(n, sc)` | `__getitem__` | 运算符 |

---

## 7. 边界条件清单

### 7.1 原子构造器边界

| # | 场景 | 输入 | 预期行为 |
|---|------|------|---------|
| 1 | FormatField 无效 endian | `FormatField("x", "b")` | `ValueError` |
| 2 | FormatField 无效 format | `FormatField(">", "x")` | `ValueError` |
| 3 | Bytes 负数长度 | `Bytes(-1)` | `ValueError`（usize 提取失败） |
| 4 | Bytes 表达式长度 | `Bytes(this.len)` | `BytesExpr` 动态长度 |
| 5 | Bytes 长度 0 | `Bytes(0)` | 空 bytes，sizeof=0 |
| 6 | Const 无 subcon 非 bytes | `Const(42)` | `TypeError` |
| 7 | Const bytes 自动 subcon | `Const(b"MZ")` | `Const::new_bytes` |
| 8 | Seek whence 无效 | `Seek(0, 5)` | `ValueError` |
| 9 | Seek 表达式 | `Seek(this.pos)` | `SeekExpr` |
| 10 | Computed 非 callable | `Computed(42)` | `py_param_to_compute_func` → 常量值 |
| 11 | Computed lambda | `Computed(lambda ctx: 42)` | `ComputeFunc` 桥接 |
| 12 | Default value 为 None | `Default(Byte, None)` | build 时使用 value（None→None） |

### 7.2 复合构造器边界

| # | 场景 | 输入 | 预期行为 |
|---|------|------|---------|
| 13 | Struct 空参数 | `Struct()` | 空 Struct，sizeof=0，parse→空 Container |
| 14 | Struct **subconskw | `Struct(a=Byte, b=Short)` | 等价 `Struct("a"/Byte, "b"/Short)` |
| 15 | Struct subcons 为列表 | `Struct([a, b, c])` | 拆包列表元素 |
| 16 | Struct 混合 subcon | `Struct("x"/Byte, MyAdapter(Byte))` | PyO3 + PyConstructAdapter 混合 |
| 17 | Struct 成员暴露 | `d.animal.giraffe` | `__getattr__` 查找 py_subcons |
| 18 | Struct 成员不存在 | `d.nonexistent` | `AttributeError` |
| 19 | Sequence 嵌套 | `Seq(Byte, Seq(Short, Int))` | 扁平化？否，Sequence 不合并嵌套 |
| 20 | Array 表达式 count | `Array(this.n, Byte)` | `ArrayExpr` 动态 count |
| 21 | GreedyRange EOF | `GreedyRange(Byte).parse(b"")` | 空 list |
| 22 | RepeatUntil 无终止 | `RepeatUntil(lambda x: x>9, Byte).parse(b"\x01")` | `RepeatError`（流耗尽） |
| 23 | Union parsefrom None | `Union(None, "a"/Byte)` | stream 不前进 |
| 24 | Union parsefrom 超界 | `Union(5, "a"/Byte)` | `IndexError`（构造时检查） |
| 25 | Select 全部失败 | `Select(Byte, Short).parse(b"")` | `SelectError` |
| 26 | FocusedSeq 字段不存在 | `FocusedSeq("missing", "a"/Byte)` | 构造时 `ValueError` |
| 27 | Padding 负长度 | `Padding(-1)` | `ValueError` |
| 28 | Padded subcon 超长 | `Padded(2, Byte).parse(b"\x01\x02")` | `PaddingError`（subcon 解析 2 字节 > 允许 2） |
| 29 | Aligned modulus < 2 | `Aligned(1, Byte)` | `PaddingError`（运行时检查） |

### 7.3 运算符边界

| # | 场景 | 输入 | 预期行为 |
|---|------|------|---------|
| 30 | `/` 运算符 | `"name" / Byte` | `Renamed(Byte, "name")` |
| 31 | `/` 空 name | `"" / Byte` | `Renamed(Byte, "")` → 匿名（name 视为 None） |
| 32 | `/` 下划线 name | `"_" / Byte` | Renamed，但 Struct 视为匿名（保留 `_` 语义） |
| 33 | `*` str | `Byte * "docs"` | `Renamed(Byte, docs="docs")` |
| 34 | `*` callable | `Byte * hook` | `Renamed(Byte, parsed=hook)` |
| 35 | `*` 其他类型 | `Byte * 42` | `ConstructError` |
| 36 | `rmul` str | `"docs" * Byte` | 同 `Byte * "docs"` |
| 37 | `+` 两个单构造器 | `Byte + Short` | `Struct(Byte, Short)` |
| 38 | `+` Struct 合并 | `(Byte + Short) + Int` | `Struct(Byte, Short, Int)`（非嵌套） |
| 39 | `+` Struct + 单 | `Struct(Byte) + Short` | `Struct(Byte, Short)` |
| 40 | `>>` 两个单构造器 | `Byte >> Short` | `Sequence(Byte, Short)` |
| 41 | `>>` Sequence 合并 | `(Byte >> Short) >> Int` | `Sequence(Byte, Short, Int)` |
| 42 | `[]` int | `Byte[5]` | `Array(5, Byte)` |
| 43 | `[]` callable | `Byte[this.n]` | `ArrayExpr(expr, Byte)` |
| 44 | `[]` slice | `Byte[1:3]` | `ConstructError`（提示用 GreedyRange） |
| 45 | `[]` 其他 | `Byte["x"]` | `ConstructError` |
| 46 | 链式运算符 | `"n" / Byte * "doc"` | `Renamed(Renamed(Byte,"n"), "", docs="doc")` — 嵌套 Renamed |

### 7.4 混合分发边界

| # | 场景 | 输入 | 预期行为 |
|---|------|------|---------|
| 47 | PyO3 构造器 subcon | `Struct("x" / Byte)` | extract_subcon → PyFormatField.inner |
| 48 | 纯 Python Adapter subcon | `Struct("x" / MyAdapter(Byte))` | extract_subcon → PyConstructAdapter |
| 49 | 非 Construct 对象 | `Struct(42)` | `TypeError` |
| 50 | 混合嵌套 | Rust Struct → Py Adapter → Rust Bytes | 3 层调用链 |

---

## 8. 与其他模块的交互

### 8.1 依赖（本模块需要）

| 模块 | 使用方式 |
|------|---------|
| **10.2 Value 映射** | `api::py_parse` 等 7 helper、`pykw_to_indexmap`、`value_to_py`/`py_to_value`、`context_to_py_container` |
| **10.3 Container** | Container/ListContainer 类（value_to_py 的 Container/List 分支） |
| **10.4 表达式桥接** | `py_param_to_evaluate`（Bytes/Array/Seek 表达式参数）、`py_param_to_compute_func`（Computed/Rebuild）、`py_param_to_cond_func`、`py_to_repeat_predicate`、`PyConstructAdapter`、`extract_subcon`、`is_construct_like` |
| **construct-rs constructs** | 全部原子/复合构造器类型（FormatField/Bytes/Struct/Sequence/Array/Union/Select/FocusedSeq/Renamed/Const/Computed 等）|
| **construct-rs expr** | `Evaluate` trait、`ConstExpr`、`this_()` |
| **PyO3 0.22** | `#[pyclass]`、`#[pyfunction]`、`#[pymethods]`、`#[pyo3(signature)]`、`PyRef`、`PyObject`、dunder 方法 |

### 8.2 被依赖（下游模块需要本模块）

| 模块 | 使用方式 |
|------|---------|
| **10.7 适配器 + 控制流** | 本模块的包装类作为 IfThenElse/Switch 的 subcon 参数；Renamed 运算符重载覆盖适配器包装类；extract_subcon 支持适配器 PyO3 包装 |
| **10.8 流操作** | BitStruct 依赖 Bitwise；Prefixed/Pointer 的 subcon 是本模块包装的构造器 |
| **10.9 字符串 + Gallery** | PaddedString 等使用本模块的 Renamed 运算符；Gallery 格式引用大量构造器常量 |
| **10.10 测试** | test_core.py 中 ~100 个用例直接测试本模块的构造器 |

### 8.3 接口契约（对下游的承诺）

1. **运算符重载全覆盖**：所有 PyO3 包装类（含 10.7-10.9 后续添加的）都实现 `/` `*` `+` `>>` `[]` 5 种运算符，行为与 Python 原版 Construct 基类一致
2. **常量导出**：40 个 FormatField 常量 + 7 个 BytesInteger/BitsInteger 常量 + 7 个别名，全部在模块顶层可导入
3. **混合分发**：任何 PyO3 包装类或纯 Python 构造器都可通过 extract_subcon 提取为 `Box<dyn Construct>`
4. **7 个 API 方法**：每个包装类都有 parse/parse_stream/parse_file/build/build_stream/build_file/sizeof，行为一致
5. **成员暴露**：PyStruct 的具名子构造器可通过 `d.fieldname` 属性访问

---

## 9. 需 PM 确认的事项

### 9.1 Renamed 扩展（需修改已验收代码）

**问题**：Rust 内核 `Renamed`（core/mod.rs:237）只有 `inner` + `name`，缺少 `docs` 和 `parsed`。

**建议**：扩展 `construct::Renamed` 添加 `docs: Option<String>` 和 `parsed: Option<Box<dyn Fn(&Value, &Context) + Send + Sync>>`（§4.1 方案 A）。

**需 PM 授权**：修改 `construct-rs/src/core/mod.rs` 的 `Renamed` 结构体。此修改：
- 新增 2 个 `Option` 字段，默认 None
- `Construct::parse` 在成功后调用 parsed hook（如有）
- 不影响现有 1306 个测试（Renamed 现有测试不涉及 docs/parsed）
- 需重新运行 construct-rs 全部测试确认无回归

**替代方案 B（PyO3 层）**：不修改 Rust 内核，parsed hook 仅在 PyO3 API 入口触发。但 Struct 内部字段（如 `"name" / Byte * hook`）的 parsed hook 不会在 Rust Struct.parse 内部触发（因 Renamed::parse 不知道 hook）。**行为差异**：Python 原版中 Struct 内部字段的 parsed hook 会触发。

**PM 裁定建议**：方案 A（扩展 Rust Renamed）。parsed hook 是 Python 原版的核心行为，方案 B 的行为差异可能导致 test_core.py 中相关用例失败。

### 9.2 Construct::clone_box（可选）

**问题**：`extract_subcon` 需要从 PyRenamed 克隆出 `Box<dyn Construct>`，但 `Construct` trait 无 `clone_box`。

**方案 A**：为 `Construct` trait 添加 `fn clone_box(&self) -> Box<dyn Construct>`（修改 Phase 1 已验收 trait）。
**方案 B**：PyRenamed 的 clone 路径从 `py_subcon` 重新 extract（性能差，每次穿越 FFI）。
**方案 C**：Renamed 包装时不 clone inner，而是共享引用（`Rc<dyn Construct>` 或 `Arc<dyn Construct>`）。但 Construct trait 用 `Box`，改 Arc 影响面大。

**建议**：方案 B（PyO3 层重新 extract）。Renamed 作为 Struct 子字段时，extract_subcon 只在构造时调用一次，不在 parse/build 热路径中。性能可接受。

### 9.3 BitStruct 依赖 10.8

**问题**：`BitStruct(*subcons)` = `Bitwise(Struct(*subcons))`，但 Bitwise 的 PyO3 包装在 10.8。

**方案 A**：10.6 先实现 AlignedStruct（无外部依赖），BitStruct 标记为 10.8 完成后集成。
**方案 B**：10.6 用纯 Python fallback 实现 BitStruct（调用 maturin develop 后的 Bitwise）。

**建议**：方案 A。BitStruct 依赖 Bitwise 是明确的阶段依赖关系，不应提前。

### 9.4 Send + Sync 返回类型偏离（10.4 遗留）

10.4 REV 审查发现的 `expr_bridge.rs` 返回类型 `+ Send + Sync` 与 Rust 内核类型别名不匹配问题。10.5 集成时需验证：
- `py_param_to_evaluate` 返回 `Box<dyn Evaluate>`（已正确）
- `py_param_to_compute_func` 返回 `ComputeFunc`（= `Box<dyn Fn(&Context) -> Result<Value>>`，需去掉 `+ Send + Sync`）

**修复**：去掉 `expr_bridge.rs` 中 `py_to_*_func` / `py_param_to_*_func` 返回类型的 `+ Send + Sync`。这是 10.4 的遗留修复，在 10.5 编码时一并处理。

### 9.5 常量 Python 命名

Python 原版使用**驼峰首字母小写**（`Int8ub`、`Float32b`），Rust 使用**全大写**（`INT8UB`、`FLOAT32B`）。register_constants 注册时使用 Python 命名。

**确认**：用户代码 `from construct import Int8ub` 应获得 PyFormatField 实例（非函数）。PyO3 `m.add("Int8ub", instance)` 注册的是实例对象，符合预期。

---

## 10. 实现优先级与分批计划

建议 DEV 按以下顺序实现（每批可独立编译验证）：

1. **批次 1**：辅助宏（`impl_api_methods!`、`impl_construct_operators!`）+ `PyConstructWrapper` trait + `extract_subcon` 扩展（补充 PyO3 类型分支）
2. **批次 2**：FormatField + 40 常量注册 + Byte/Short 等别名（最基础，test_core.py 大量使用）
3. **批次 3**：Bytes/GreedyBytes/BytesInteger/BitsInteger/VarInt/ZigZag/Flag
4. **批次 4**：Const/Pass/Terminated/Tell/Seek/Error
5. **批次 5**：Computed/Rebuild/Default/Index
6. **批次 6**：Renamed 扩展（需 PM 授权）+ PyRenamed + `/` `*` 运算符
7. **批次 7**：Struct + Sequence + `+` `>>` 运算符 + 成员暴露
8. **批次 8**：Array/GreedyRange/RepeatUntil + `[]` 运算符
9. **批次 9**：Union/Select/FocusedSeq
10. **批次 10**：Padding/Padded/Aligned/AlignedStruct（BitStruct 标记 10.8）

每批完成后运行 `maturin develop` + Python 交互验证（`from construct import Byte; Byte.parse(b"\x01")`）。
