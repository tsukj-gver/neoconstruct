# 模块设计：PythonFFI-02 Value ↔ Python 映射 + 错误映射

## 模块位置
`construct-py/src/conversions.rs`（Value 转换）、`construct-py/src/exceptions.rs`（异常类）、`construct-py/src/pystream.rs`（PyStream）、`construct-py/src/api.rs`（公共 helper）

## 职责
Rust `Value`/`ConstructError`/`Context` 与 Python 对象/异常/Container 的双向转换，以及 PyConstruct 公共 API 的共享逻辑。

---

## 1. Value → PyObject 映射

函数签名：`pub fn value_to_py(py: Python<'_>, v: &Value) -> PyResult<PyObject>`

| Value 变体 | Python 类型 | 实现方式 |
|-----------|------------|---------|
| `None` | `None` | `py.None()` |
| `Bool(b)` | `bool` | `b.into_py(py)` |
| `Int(i)` | `int` | `i.into_py(py)` |
| `UInt(u)` | `int` | `u.into_py(py)` |
| `BigInt(i)` | `int` | 见下方注 1 |
| `Float(f)` | `float` | `f.into_py(py)` |
| `Bytes(b)` | `bytes` | `PyBytes::new(py, &b).into()` |
| `String(s)` | `str` | `PyString::new(py, s).into()` |
| `List(v)` | `ListContainer` (10.3) | 递归转换各元素，构造 `Py<PyListContainer>` |
| `Container(m)` | `Container` (10.3) | 递归转换各值，构造 `Py<PyContainer>` |

**注 1（BigInt/i128 → Python int）**：PyO3 0.22 的 `IntoPy` 不支持 `i128`/`u128`。两种实现策略：
- **推荐**：引入 `num-bigint`（仅 construct-py 依赖），`BigInt::from(i).into_py(py)`（pyo3 支持 `num_bigint::BigInt` 的 `IntoPy`）。
- **备选**：范围检查 — 若 `i` 在 `i64` 范围内用 `i as i64`，否则用 `py.eval(&i.to_string(), ...)` 动态求值。

---

## 2. PyObject → Value 反向转换

函数签名：`pub fn py_to_value(py: Python<'_>, obj: &PyAny) -> PyResult<Value>`

**检查顺序（关键，因 Python bool 是 int 子类）**：

```
1. obj.is_none()           → Value::None
2. obj.is_instance_of::<PyBool>() → Value::Bool(提取 bool)
3. obj.is_instance_of::<PyInt>()  → 整数处理（见下）
4. obj.is_instance_of::<PyFloat>() → Value::Float
5. obj.is_instance_of::<PyBytes>() → Value::Bytes
6. obj.is_instance_of::<PyString>() → Value::String
7. obj.is_instance_of::<PyList>() 或 PyListContainer → Value::List (递归)
8. obj.is_instance_of::<PyDict>() 或 PyContainer → Value::Container (递归)
9. 以上都不是 → Err(PyTypeError)
```

**Python int → Value 整数处理**（按范围降级）：

| Python int 值范围 | 映射到 | 说明 |
|------------------|-------|------|
| `i64::MIN..=i64::MAX` | `Value::Int` | 常见情况 |
| `i64::MAX < v <= u64::MAX` | `Value::UInt` | 非负大数 |
| `i128::MIN..=i128::MAX` 之外 | `Value::UInt` 或 `Err` | 见注 2 |
| 超出 `i128` 范围 | `Err(OverflowError)` | 见注 2 |

**注 2（大整数限制）**：Rust `Value::BigInt` 为 `i128`，无法表示超 i128 的 Python int。转换时：
- 先尝试 `i128::try_from(bigint)`，失败则返回 `PyErr`（`OverflowError` 消息标注 "integer exceeds i128 range"）。
- `num-bigint` 的 `BigInt::try_into::<i128>()` 用于范围检查。

**dict → Container**：遍历 `.items()`，键必须为 `str`（非 str 键 → `PyTypeError`），值递归转换。用 `IndexMap` 保持插入顺序。

---

## 3. ConstructError → Python 异常映射

### 3.1 Python 异常类层次（28 个，全部定义）

用 PyO3 `create_exception!` 宏批量创建，继承关系对齐 Python 原版（`core.py:13-154`）：

```
PyException
└── ConstructError (基类)
    ├── SizeofError ★          ├── ConstError          ├── PaddingError
    ├── AdaptationError         ├── IndexFieldError     ├── TerminatedError
    ├── ValidationError         ├── CheckError          ├── RawCopyError
    ├── StreamError             ├── ExplicitError       ├── RotationError
    ├── FormatFieldError        ├── NamedTupleError     ├── ChecksumError
    ├── IntegerError            ├── TimestampError      ├── CancelParsing ★
    ├── StringError             ├── UnionError          └── CipherError
    ├── MappingError            ├── SelectError
    ├── RangeError              ├── SwitchError
    ├── RepeatError             └── StopFieldError
```

每个类用一行宏创建：`create_exception!(construct_rust, ClassName, ParentClass)`。所有 28 个类在 `#[pymodule]` 中用 `m.add("ClassName", ...)` 注册为顶层可导入名。`SizeofError` 为强制导出（declarativeunittest.py 依赖）。

### 3.2 Rust 错误 → Python 异常映射表

| Rust `ConstructError` 变体 | 抛出的 Python 异常 | 转换说明 |
|---------------------------|-------------------|---------|
| `Generic` | `ConstructError` | 基类，message 作为参数 |
| `Stream` | `StreamError` | io::Error 的描述 |
| `FormatField` | `FormatFieldError` | "expected X got Y" |
| `Sizeof` | `SizeofError` | reason 作为消息 |
| `Const` | `ConstError` | expected/actual 对比 |
| `Validation` | `ValidationError` | message |
| `Array` | `RangeError` | 用 expected/actual（非 RepeatError） |
| `Index` | `IndexFieldError` | "index X in list of length Y" |
| `Padding` | `PaddingError` | message |
| `Check` | `CheckError` | message |
| `Terminated` | `TerminatedError` | "N bytes remaining" |
| `StringEncoding` | `StringError` | UTF-8 错误描述 |
| `Mapping` | `MappingError` | "key K not found" |
| `Expr` | `ConstructError` | 无专用类，用基类 |
| `Union` | `UnionError` | message |
| `Switch` | `SwitchError` | "no branch for key K" |
| `FieldMissing` | `ConstructError` | 无专用 FieldError，用基类+消息 |
| `TypeMismatch` | `ConstructError` | 无专用类，用基类+消息 |
| `Select` | `SelectError` | message |
| `StopField` | (不传播) | 内部信号，不应逃逸到 Python |

**转换函数**：`pub fn rust_err_to_py(py: Python<'_>, e: ConstructError) -> PyErr`

### 3.3 CancelParsing 特殊处理

`CancelParsing` 是控制流信号，非普通错误：
- **Rust → Python**：Rust 内核不会主动产生 `CancelParsing`（无对应变体）。
- **Python → Rust（10.7 桥接层）**：当 Python 回调（Check func、Adapter decode 等）抛出 `CancelParsing` 时，PyConstructAdapter 捕获该 `PyErr`，转为优雅退出（对应 `core.py:416-419`）。10.2 仅负责定义类并导出。

---

## 4. PyConstruct 公共 API 契约

### 4.1 设计方案：共享 helper + 各构造器独立 #[pyclass]

不使用单一 `PyConstruct` 包装类（会丢失具体类型信息）。改为提供 7 个共享 helper 函数（`api.rs`）：`py_parse`、`py_parse_stream`、`py_parse_file`、`py_build`、`py_build_stream`、`py_build_file`、`py_sizeof`。每个 helper 首参为 `&dyn Construct`，内部处理 contextkw 注入、Value↔PyObject 转换、错误映射。

各 `#[pyclass]` 构造器的 `#[pyo3]` 方法委托 helper，签名用 `#[pyo3(signature = (data, **kw))]` 捕获 keyword args：

```rust
impl PyBytes {
    fn parse(&self, py: Python<'_>, data: &PyBytes, #[pyo3(keyword_args)] kw: PyObject) -> PyResult<PyObject> {
        api::py_parse(&self.inner, py, data.as_bytes(), pykw_to_indexmap(py, &kw)?)
    }
    // build/sizeof 同理
}
```

### 4.2 `**contextkw` 注入机制

`#[pyo3(signature = (data, **kw))]` 捕获 keyword args 为 `Bound<PyDict>`。helper 内：
1. 将 kw dict 转为 `IndexMap<String, Value>`（键必须 str，值用 `py_to_value` 转换）
2. 创建 `Context::new()`，逐条 `ctx.insert(k, v)` 注入为**顶层条目**
3. 传给 `Construct::parse`/`build`/`sizeof`

### 4.3 build_file 模式

`build_file` 以 `'w+b'`（读写）模式打开文件，因为 `RawCopy` 需要回读刚写入的数据（issue #888）。Rust 端 `OpenOptions::new().write(true).create(true).truncate(true).read(true).open()`。

---

## 5. PyStream 桥接

文件：`pystream.rs`

```rust
pub struct PyStream {
    obj: PyObject,  // Python file-like 对象 (BytesIO 等)，持有 Py<PyAny> 引用
}

impl Stream for PyStream { ... }
```

| Stream 方法 | Python 调用 | 说明 |
|------------|------------|------|
| `read_bytes(n)` | `obj.read(n)` → PyBytes | 返回 Vec |
| `read_exact(buf)` | `obj.readinto(buf)` 或循环 read | EOF → StreamError |
| `write_bytes(data)` | `obj.write(data)` | 忽略返回值 |
| `seek(pos)` | `obj.seek(pos)` (whence=0) | 绝对定位 |
| `tell()` | `obj.tell()` | 当前位置 |
| `size()` | `obj.seek(0,2); tell; seek back` | 或从 `len()` 获取 |
| `is_eof()` | `tell() >= size()` | |

**GIL 管理**：`Py<PyAny>` 不持有 GIL token。每个 Stream 方法内部用 `Python::with_gil(|py| { obj.bind(py).call_method1("read", (n,))? ... })` 获取 GIL。

---

## 6. Context → Python Container 转换

函数：`pub fn context_to_py_container(py: Python<'_>, ctx: &Context) -> PyResult<Py<PyContainer>>`

转换规则（递归）：创建空 PyContainer → 遍历 `ctx.fields`，各值用 `value_to_py` 转换并写入 → 若 `ctx.parent()` 存在，递归转换 parent 写入 `container["_"]`。

**用途**：当 Rust 内核调用 Python 回调（Check func、Adapter decode、Computed func 等，10.4/10.7 实现）时，将当前 Context 转为 Python Container 传给回调。Container 类型由 10.3 定义。

---

## 7. 边界条件清单

1. **Python bool 优先于 int**：`isinstance(True, int)` 为 True，转换必须先检查 bool
2. **Python int 超出 i128**：返回 `OverflowError`（Rust Value 无法表示）
3. **dict 非 str 键**：返回 `TypeError`（Value::Container 键必须 String）
4. **Python None**：映射为 `Value::None`，不映射为 `Option::None`
5. **嵌套深度**：递归转换无深度限制（与 Python 原版行为对齐）
6. **CancelParsing 仅在 parse 路径捕获**：build/sizeof 路径视作普通错误
7. **StopField 不逃逸**：若意外到达 FFI 层转为 `ConstructError`(Generic) 抛出
8. **空 bytes/空 string**：正常映射（`b""` → `Value::Bytes(vec![])`）
9. **PyStream GIL 重入**：`Python::with_gil` 在已持有 GIL 时安全
10. **contextkw `_` 键冲突**：允许覆盖 parent 引用（用户责任，按 Python 原版语义）

---

## 8. 与其他模块的交互

### 依赖（本模块需要）
- **construct-rs**：`Value`、`ConstructError`、`Context`、`Construct` trait、`Stream` trait、`ByteStream`
- **PyO3 0.22**：`IntoPy`、`Python`、`PyResult`、`create_exception!`
- **num-bigint**（construct-py 依赖）：i128 ↔ Python int 大整数转换
- **indexmap**：与 construct-rs 共用

### 被依赖（下游模块需要本模块）
- **10.3 Container/ListContainer**：提供 `PyContainer`/`PyListContainer` 类型，本模块的 `value_to_py` 构造它们
- **10.4 表达式系统**：`context_to_py_container` 供 `PyClosure` 求值；`py_to_value` 转换 lambda 返回值
- **10.5-10.8 各构造器包装**：所有 `#[pyclass]` 的 7 个方法委托本模块的 `api::py_*` helper
- **10.10 测试**：`SizeofError` 导出是 declarativeunittest.py 导入的前提

### 接口契约（对下游的承诺）
- `value_to_py` / `py_to_value` 是**值不丢失**的双射（i128 范围内的整数往返一致）
- `rust_err_to_py` 保证 Rust 任何 `ConstructError` 都映射到可捕获的 Python 异常子类
- 28 个异常类名 + `SizeofError` 在模块顶层可导入

---

## 9. 需 PM 确认的事项

1. **num-bigint 依赖**：建议 construct-py/Cargo.toml 添加 `num-bigint = "0.4"` 和 `pyo3` 的 `abi3` 兼容性确认。若 PM 不希望引入 num-bigint，则 BigInt→Python int 用 `py.eval` fallback（性能略差，但无新依赖）。
2. **FieldMissing/TypeMismatch 无专用 Python 类**：Python 原版无 `FieldError`/`TypeError` 子类（Struct 内部用 `ConstructError` 基类抛出）。本设计遵循原版，映射到 `ConstructError`。若 PM 希望细化，需新增 Python 异常类（但会偏离原版 API）。
3. **i128 上限**：Python 任意精度 int 超 i128 时报错而非截断。这是 Rust 内核架构限制，无法绕过（除非 Rust 端改用 num-bigint，影响 Phase 1-9 已验收代码）。确认可接受。
