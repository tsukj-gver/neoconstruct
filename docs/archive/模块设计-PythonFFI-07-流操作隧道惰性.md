# 模块设计：PythonFFI-07 流操作 + 隧道 + 惰性 Python 包装层

> 子任务 10.8。覆盖流操作（Bitwise/Bytewise/Pointer/Peek/RawCopy/Prefixed/
> Transformed/Restreamed/Compressed/Checksum/ByteSwapped/BitsSwapped）、
> 隧道（RestreamData/ProcessXor/ProcessRotateLeft/OffsettedEnd/
> NullTerminated/NullStripped/FixedSized）、惰性（Lazy/LazyStruct/LazyArray/
> LazyBound/Rebuffered）、以及 10.6 遗留的 BitStruct。

## 模块位置

- `construct-py/src/constructs_stream.rs` — **本子任务新增**。Rust 内建流操作/隧道/惰性构造器的 PyO3 包装（Bitwise/Bytewise/Pointer/Peek/RawCopy/Prefixed/Transformed/Restreamed/Compressed/Checksum/ByteSwapped/BitsSwapped/Lazy/LazyStruct/LazyArray/LazyBound/Rebuffered/FixedSized）+ 函数式工厂（Bitwise/Bytewise/ByteSwapped/BitsSwapped/BitStruct/PrefixedArray/OffsettedEnd）。
- `construct-py/src/expr_bridge.rs` — **本子任务扩展**。新增 4 个桥接器：`py_to_transform_func` / `py_to_checksum_func` / `py_to_checksum_bytes_func` / `py_to_size_computer`。
- `construct-py/construct_rust/_stream.py` — **本子任务新增**。纯 Python 实现的构造器（NullTerminated/NullStripped/RestreamData/ProcessXor/ProcessRotateLeft/OffsettedEnd-fallback）。
- `construct-py/src/py_adapter.rs` — **本子任务扩展**。`try_extract_registered` 增补 10.8 包装类型分支。
- `construct-py/src/lib.rs` — **本子任务扩展**。注册 `constructs_stream` 模块 + 新增 pyfunction/pyclass。
- `construct-py/Cargo.toml` — **本子任务扩展**。construct 依赖启用 `compression` feature。
- `construct-py/construct_rust/__init__.py` — **本子任务扩展**。导入 `_stream` 纯 Python 模块。

## 职责

将 Rust 内核（construct-rs）中已实现的流操作、隧道、惰性构造器，通过 PyO3 `#[pyclass]` 包装暴露给 Python 用户；将依赖 Python 运行时特性（逐字节读取、context lambda 求值、itertools）或无 Rust 对应实现的构造器作为纯 Python 类提供。最终使 Python 用户能以与原版 construct 库**完全一致**的声明式语法使用这些构造器。

---

## 1. 核心设计决策

### 1.1 三层实现策略

10.8 的实现分为三层，每层对应不同的实现策略：

```
┌─────────────────────────────────────────────────────────────────────┐
│ 层 1：Rust 内建构造器的 PyO3 包装（constructs_stream.rs）            │
│   - Bitwise / Bytewise / Pointer / Peek / RawCopy / Prefixed         │
│   - Transformed / Restreamed / Compressed / Checksum                 │
│   - ByteSwapped / BitsSwapped / FixedSized                           │
│   - Lazy / LazyStruct / LazyArray / LazyBound / Rebuffered           │
│   每个 #[pyclass] 持有 Rust owned 实例，零开销调用 Rust 实现          │
└─────────────────────────────────────────────────────────────────────┘
                               ↓
┌─────────────────────────────────────────────────────────────────────┐
│ 层 2：函数式构造器（py_* 工厂函数，返回层 1 的 #[pyclass]）           │
│   - Bitwise(subcon) → PyBitwise                                     │
│   - Bytewise(subcon) → PyBytewise                                   │
│   - ByteSwapped(subcon) → PyByteSwapped                             │
│   - BitsSwapped(subcon) → PyBitsSwapped                             │
│   - BitStruct(*subcons) → PyBitwise(PyStruct)                       │
│   - PrefixedArray(countfield, subcon) → PyFocusedSeq                │
│   - OffsettedEnd(endoffset, subcon) → PyTransformed (固定大小) 或    │
│     纯 Python fallback (可变大小)                                    │
│   这些在 Python 原版中是普通函数（非类），返回其他构造器实例。        │
│   本模块用 Rust 工厂函数实现相同语义                                  │
└─────────────────────────────────────────────────────────────────────┘
                               ↓
┌─────────────────────────────────────────────────────────────────────┐
│ 层 3：纯 Python 实现（construct_rust/_stream.py）                    │
│   - NullTerminated(subcon, term, include, consume, require)          │
│   - NullStripped(subcon, pad)                                        │
│   - RestreamData(datafunc, subcon)                                   │
│   - ProcessXor(padfunc, subcon)                                      │
│   - ProcessRotateLeft(amount, group, subcon)                         │
│   这些构造器依赖 Python 运行时特性（逐字节 IO / itertools.cycle /    │
│   context lambda 求值），或其逻辑与 Rust Transformed 的无 context    │
│   闭包签名不兼容，作为纯 Python 类提供（继承 _adapter.py 基类）       │
└─────────────────────────────────────────────────────────────────────┘
```

### 1.2 实现策略分类表

每个构造器按以下决策树选择实现策略：

| 构造器 | 策略 | 理由 |
|--------|------|------|
| `Bitwise(subcon)` | **PyO3 包装** | Rust `Bitwise` 已实现（bit 层级流转换） |
| `Bytewise(subcon)` | **PyO3 包装** | Rust `Bytewise` 已实现 |
| `ByteSwapped(subcon)` | **PyO3 包装** | Rust `ByteSwapped` 已实现（字节反转） |
| `BitsSwapped(subcon)` | **PyO3 包装** | Rust `BitsSwapped` 已实现（位反转） |
| `Pointer(offset, subcon)` | **PyO3 包装** | Rust `Pointer`/`PointerExpr` 已实现 |
| `Peek(subcon)` | **PyO3 包装** | Rust `Peek` 已实现 |
| `RawCopy(subcon)` | **PyO3 包装** | Rust `RawCopy` 已实现 |
| `Prefixed(lengthfield, subcon)` | **PyO3 包装** | Rust `Prefixed` 已实现 |
| `Transformed(subcon, decoder, encodeamount, encoder, decodeamount)` | **PyO3 包装** | Rust `Transformed` 已实现，decoder/encoder 通过 TransformFunc 桥接 |
| `Restreamed(subcon, decoder, decoderunit, encoder, encoderunit, sizecomputer)` | **PyO3 包装** | Rust `Restreamed` 已实现 |
| `Compressed(subcon, encoding, level)` | **PyO3 包装** | Rust `Compressed` 已实现（feature-gated: flate2），仅 zlib/deflate |
| `CompressedLZ4(subcon)` | **不实现** | 无 lz4 依赖，测试跳过 |
| `Checksum(checksumfield, hashfunc, bytesfunc)` | **PyO3 包装** | Rust `Checksum` 已实现 |
| `Lazy(subcon)` | **PyO3 包装** | Rust `Lazy` 已实现（即时解析 + callable 包装） |
| `LazyStruct(*subcons)` | **PyO3 包装** | Rust `LazyStruct` 已实现（即时解析） |
| `LazyArray(count, subcon)` | **PyO3 包装** | Rust `LazyArray` 已实现 |
| `LazyBound(ctxfunc)` | **PyO3 包装** | Rust `LazyBound` 已实现 |
| `Rebuffered(subcon, tailcutoff)` | **PyO3 包装** | Rust `Rebuffered` 已实现 |
| `FixedSized(length, subcon)` | **PyO3 包装** | Rust `FixedSized` 已实现（computed.rs） |
| `BitStruct(*subcons)` | **函数式（PyO3）** | `= Bitwise(Struct(*subcons))` |
| `PrefixedArray(countfield, subcon)` | **函数式（PyO3）** | `= FocusedSeq("items", "count"/Rebuild(...), "items"/subcon[this.count])` |
| `OffsettedEnd(endoffset, subcon)` | **函数式（PyO3）/纯 Python** | 固定大小用 Transformed 组合；可变大小用纯 Python fallback |
| `NullTerminated(subcon, ...)` | **纯 Python** | 逐字节读取 + 多参数（term/include/consume/require），纯 Python 最清晰 |
| `NullStripped(subcon, pad)` | **纯 Python** | rstrip 逻辑 + 读取到 EOF，纯 Python 简洁 |
| `RestreamData(datafunc, subcon)` | **纯 Python** | datafunc 可能是 Construct 实例（需 Python 端解析），纯 Python 最灵活 |
| `ProcessXor(padfunc, subcon)` | **纯 Python** | padfunc 是 context lambda（需运行时求值），Rust Transformed 的无 context 闭包不兼容 |
| `ProcessRotateLeft(amount, group, subcon)` | **纯 Python** | amount/group 是 context lambda + 位旋转算法复杂，纯 Python 逐行移植 |

### 1.3 关键确认：Rust 内核完备性

| Rust 构造器 | 文件位置 | 关键方法 | 备注 |
|------------|---------|---------|------|
| `Bitwise` | `stream_ops.rs:68` | parse/build/sizeof | bytes↔bits 转换 |
| `Bytewise` | `stream_ops.rs:127` | parse/build/sizeof | bits↔bytes 转换 |
| `Pointer` | `stream_ops.rs:189` | parse/build/sizeof=0 | 支持 offset/relativeOffset |
| `PointerExpr` | `stream_ops.rs:273` | parse/build/sizeof=0 | offset 为表达式 |
| `Peek` | `stream_ops.rs:360` | parse/build=pass/sizeof=0 | flagbuildnone=true |
| `RawCopy` | `stream_ops.rs:430` | parse/build/sizeof | 返回 Container{data,value,offset1,offset2,length} |
| `Prefixed` | `stream_ops.rs:518` | parse/build/sizeof | include_length 参数 |
| `Transformed` | `stream_ops.rs:609` | parse/build/sizeof | TransformFunc = `Fn(&[u8]) -> Result<Vec<u8>>` |
| `Restreamed` | `stream_ops.rs:707` | parse/build/sizeof | chunk 级变换 + size_computer |
| `Compressed` | `stream_ops.rs:819` | parse/build/sizeof=Err | `#[cfg(feature="compression")]` |
| `Checksum` | `stream_ops.rs:971` | parse/build/sizeof | ChecksumFunc + ChecksumBytesFunc |
| `ByteSwapped` | `stream_ops.rs:1058` | parse/build/sizeof | 需固定大小 |
| `BitsSwapped` | `stream_ops.rs:1124` | parse/build/sizeof | fixed/variable 两种模式 |
| `LazyBound` | `stream_ops.rs:1211` | parse/build/sizeof | 运行时绑定 |
| `Lazy` | `lazy.rs:94` | parse/build/sizeof | **即时解析**（方案 A） |
| `LazyStruct` | `lazy.rs:177` | parse/build/sizeof | **即时解析**，委托 Struct |
| `LazyArray` | `lazy.rs:278` | parse/build/sizeof | **即时解析**，委托 Array |
| `Rebuffered` | `lazy.rs:362` | parse/build/sizeof | 全量缓冲 + 位置同步 |
| `FixedSized` | `computed.rs:585` | parse/build/sizeof | 固定大小子流 |

**结论**：19 个 Rust 类型已实现，通过 PyO3 包装可直接暴露。无需修改 construct-rs 内核。

### 1.4 Python callable 桥接需求（TransformFunc 系列）

Rust 内核的流操作构造器使用以下闭包类型别名，均**不接受 context**（纯 bytes-to-bytes 变换）：

```rust
// stream_ops.rs
pub type TransformFunc = Box<dyn Fn(&[u8]) -> Result<Vec<u8>>>;
pub type ChecksumFunc = Box<dyn Fn(&[u8]) -> Vec<u8>>;
pub type ChecksumBytesFunc = Box<dyn Fn(&Context) -> Result<Vec<u8>>>;
```

Python 用户传入的 callable 签名：

| Rust 类型 | Python callable 签名 | 示例 |
|-----------|---------------------|------|
| `TransformFunc` | `callable(data: bytes) -> bytes` | `bytes2bits`, `bits2bytes`, `swapbytes` |
| `ChecksumFunc` | `callable(data: bytes) -> bytes` | `lambda data: hashlib.sha512(data).digest()` |
| `ChecksumBytesFunc` | `callable(context) -> bytes` | `this.fields.data`（表达式求值返回 bytes） |
| `Restreamed.size_computer` | `callable(n: int) -> int` | `lambda n: n//8` |

**桥接策略**：

- `TransformFunc` / `ChecksumFunc`：通过 GIL 调用 Python callable，参数和返回值都是 `bytes`（PyO3 原生支持）。无需 context 转换。
- `ChecksumBytesFunc`：复用 10.4 的 `PyClosure`（`Evaluate` trait），将 context lambda 求值为 `Value::Bytes`。
- `size_computer`：通过 GIL 调用 Python callable，参数 `usize` → Python `int`，返回值 `int` → `usize`。

### 1.5 Lazy PyO3 包装策略（Phase 7 方案 A）

**背景**：Rust 内核的 `Lazy`/`LazyStruct`/`LazyArray` 采用**即时解析**策略（Phase 7 方案 A），parse 时立即消耗字节并返回 `Value`。Python 原版返回延迟执行的可调用对象（`Lazy` 返回 lambda）或惰性容器（`LazyContainer`/`LazyListContainer`）。

**行为差异分析**：

| 构造器 | Python 原版 parse 返回 | Rust parse 返回 | 差异影响 |
|--------|----------------------|----------------|---------|
| `Lazy(Byte)` | `lambda: 42`（callable） | `Value::UInt(42)` | 测试 `x = d.parse(data); x()` 会失败（int 不可调用） |
| `LazyStruct(...)` | `LazyContainer`（延迟字段） | `Value::Container`（即时） | 行为等价（LazyContainer 继承 dict，访问字段触发解析，但即时解析结果相同） |
| `LazyArray(n, sc)` | `LazyListContainer`（延迟元素） | `Value::List`（即时） | 行为等价（同上） |

**包装策略**：

| 构造器 | PyO3 包装策略 | 说明 |
|--------|-------------|------|
| `Lazy(subcon)` | parse 返回 Python callable | PyO3 `parse` 方法即时解析后包装为 `lambda: value`，保持 callable 语义 |
| `LazyStruct(*subcons)` | parse 返回普通 Container | 即时解析，行为与 Struct 一致 |
| `LazyArray(n, subcon)` | parse 返回普通 ListContainer | 即时解析，行为与 Array 一致 |
| `LazyBound(ctxfunc)` | PyO3 包装，parse/build 委托 Rust | ctxfunc 是无参 callable 返回 subcon |
| `Rebuffered(subcon)` | PyO3 包装，parse/build 委托 Rust | 全量缓冲 |

**Lazy callable 包装实现**：

```rust
#[pymethods]
impl PyLazy {
    fn parse_stream(&self, py, stream, kw) -> PyResult<PyObject> {
        // 即时解析得到 Value
        let value = api::py_parse_stream(&self.inner, py, stream, kw)?;
        // 包装为 Python callable，调用时返回 value
        Ok(wrap_as_callable(py, value))
    }
}

/// 将一个 Python 对象包装为零参数 callable，调用时返回原对象。
fn wrap_as_callable(py: Python<'_>, value: PyObject) -> PyObject {
    let closure = pyo3::types::PyCFunction::new_closure(py, None, None, move |_args, _kw| {
        Ok(value.clone_ref(py))  // 注意：闭包捕获 value 的引用
    }).unwrap();
    closure.into_any()
}
```

**注意**：`PyCFunction::new_closure` 创建的 callable 是一次性返回固定值。这精确匹配 Python 原版 `Lazy._parse` 中 `execute()` 闭包的行为（解析后值不变）。

### 1.6 Compressed feature gate 配置

Rust `Compressed` 在 `#[cfg(feature = "compression")]` 下。construct-py 需启用此 feature：

```toml
# construct-py/Cargo.toml
[dependencies]
construct = { path = "../construct-rs", features = ["compression"] }
```

**非可选**：Python 用户总是需要 `Compressed`（zlib/deflate），因此 construct-py 直接启用 compression feature（不用 construct-py 自己的 feature gate）。PyO3 包装类无条件编译。

**支持的编码**：

| 编码 | Rust 支持 | flate2 后端 | 测试状态 |
|------|---------|------------|---------|
| `"zlib"` | ✅ `CompressionAlgorithm::Zlib` | `flate2::read::ZlibDecoder/Encoder` | pass |
| `"deflate"` | ✅ `CompressionAlgorithm::Deflate` | `flate2::read::DeflateDecoder/Encoder` | pass |
| `"gzip"` | ❌ enum 未暴露 Gzip 变体 | flate2 支持 | **跳过**（`test_compressed_gzip`） |
| `"bzip2"` | ❌ 无 bzip2 依赖 | — | **跳过**（`test_compressed_bzip2`） |
| `"lzma"` | ❌ 无 lzma 依赖 | — | **跳过**（`test_compressed_lzma`） |
| lz4 | ❌ 无 lz4 依赖 | — | **跳过**（`test_compressedlz4`），`CompressedLZ4` 不实现 |

**encoding 参数映射**：

```rust
let algorithm = match encoding {
    "zlib" => CompressionAlgorithm::Zlib,
    "deflate" => CompressionAlgorithm::Deflate,
    other => return Err(PyValueError::new_err(format!(
        "Unsupported compression encoding '{}': only zlib/deflate are supported", other))),
};
```

### 1.7 Pointer 的 stream / relativeOffset 参数

Python `Pointer(offset, subcon, stream=None, relativeOffset=False)` 有两个额外参数：
- `stream`：context lambda，提供不同的流（默认 None = 原始流）
- `relativeOffset`：bool，offset 是否相对于当前位置（默认 False = 绝对偏移）

Rust `Pointer::new(offset: i64, subcon)` 和 `PointerExpr::new(offset_expr, subcon)` 不支持这两个参数。

**处理策略**：
- `relativeOffset=True`：在 PyO3 包装层，将 offset + 当前 tell() 传入 Rust Pointer（计算绝对偏移）。需要用 PointerExpr + 表达式（`this._.tell + offset`），或在 PyO3 包装的 parse_stream 中先求值。
- `stream` 参数：Python 原版中 `stream` 是 context lambda 返回一个 Python 流对象。如果非 None，Pointer 在该流上操作而非原始流。**Rust Stream trait 不支持流切换**（parse 签名只有 `&mut dyn Stream`）。

**已知限制**：
- `relativeOffset=True`：通过 PointerExpr + 组合表达式实现（在 PyO3 层构造 `tell + offset` 表达式）。
- `stream != None`：不支持。标记为已知限制。测试用例中此参数极少使用（grep core.py 仅在 `__init__` 中定义，测试无此用法）。

---

## 2. Rust 已有实现的 PyO3 包装

本节定义 `construct-py/src/constructs_stream.rs` 中各 `#[pyclass]` 包装类的接口。所有包装类遵循 10.5/10.6/10.7 建立的模式：
- 实现 `PyConstructWrapper` trait（提供 `as_construct()`）
- 使用 `impl_api_methods!(PyXxx)` 宏生成 7 个公共 API 方法
- 使用 `impl_construct_operators!(PyXxx)` 宏生成 6 个运算符 dunder 方法

### 2.1 Bitwise / Bytewise

Python 原版中 `Bitwise` / `Bytewise` 是**普通函数**（`def`），返回 `Transformed` 或 `Restreamed` 实例。但 Rust 有独立的 `Bitwise`/`Bytewise` 类型（更高效的 bit-level 实现）。

**包装策略**：为 `Bitwise`/`Bytewise` 创建独立的 `#[pyclass]`，包装 Rust 类型。Python 用户 `isinstance(d, Bitwise)` 检查需要独立类型。

```rust
/// PyO3 包装：位级流包装器。
#[pyclass(name = "Bitwise")]
pub struct PyBitwise {
    pub(crate) inner: construct::constructs::stream_ops::Bitwise,
}

impl PyConstructWrapper for PyBitwise {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

/// 工厂函数：Bitwise(subcon) → PyBitwise
#[pyfunction]
#[pyo3(name = "Bitwise", signature = (subcon))]
pub fn py_bitwise(py: Python<'_>, subcon: &Bound<PyAny>) -> PyResult<PyBitwise> {
    let sc = crate::py_adapter::extract_subcon(subcon)?;
    Ok(PyBitwise {
        inner: construct::constructs::stream_ops::Bitwise::new(sc),
    })
}

impl_api_methods!(PyBitwise);
impl_construct_operators!(PyBitwise);

/// PyO3 包装：字节级流恢复器（Bitwise 上下文内使用）。
#[pyclass(name = "Bytewise")]
pub struct PyBytewise {
    pub(crate) inner: construct::constructs::stream_ops::Bytewise,
}

impl PyConstructWrapper for PyBytewise {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "Bytewise", signature = (subcon))]
pub fn py_bytewise(subcon: &Bound<PyAny>) -> PyResult<PyBytewise> {
    let sc = crate::py_adapter::extract_subcon(subcon)?;
    Ok(PyBytewise {
        inner: construct::constructs::stream_ops::Bytewise::new(sc),
    })
}

impl_api_methods!(PyBytewise);
impl_construct_operators!(PyBytewise);
```

### 2.2 Pointer

Python `Pointer(offset, subcon, stream=None, relativeOffset=False)`。offset 可以是 int 或 context lambda。

```rust
/// PyO3 包装：绝对位置读写（支持 int 和表达式 offset）。
#[pyclass(name = "Pointer")]
pub struct PyPointer {
    pub(crate) inner: Box<dyn Construct>,  // Pointer 或 PointerExpr
}

impl PyConstructWrapper for PyPointer {
    fn as_construct(&self) -> &dyn Construct { &self.inner.as_ref() }
}

#[pyfunction]
#[pyo3(name = "Pointer", signature = (offset, subcon, stream=None, relativeOffset=false))]
pub fn py_pointer(
    py: Python<'_>,
    offset: &Bound<PyAny>,
    subcon: &Bound<PyAny>,
    stream: Option<&Bound<PyAny>>,
    relative_offset: bool,
) -> PyResult<PyPointer> {
    if stream.is_some() {
        return Err(PyNotImplementedError::new_err(
            "Pointer(stream=...) is not supported in construct-rust"));
    }
    let sc = crate::py_adapter::extract_subcon(subcon)?;
    let inner: Box<dyn Construct> = if offset.is_callable() {
        // 表达式 offset → PointerExpr
        let expr = crate::expr_bridge::py_param_to_evaluate(offset)?;
        if relative_offset {
            // relativeOffset=True: 实际 offset = tell() + expr
            // 用 PointerExpr 组合：此处简化，标记已知限制
            return Err(PyNotImplementedError::new_err(
                "Pointer(relativeOffset=True) with expression offset is not yet supported"));
        }
        Box::new(construct::constructs::stream_ops::PointerExpr::new(expr, sc))
    } else {
        let off: i64 = offset.extract()?;
        if relative_offset {
            // relativeOffset=True with int offset: 需要 tell + offset
            // 用 PointerExpr + 组合表达式实现
            // 此处标记已知限制
            return Err(PyNotImplementedError::new_err(
                "Pointer(relativeOffset=True) is not yet supported, use absolute offset"));
        }
        Box::new(construct::constructs::stream_ops::Pointer::new(off, sc))
    };
    Ok(PyPointer { inner })
}

impl_api_methods!(PyPointer);
impl_construct_operators!(PyPointer);
```

**已知限制**：`relativeOffset=True` 和 `stream != None` 不支持。测试用例中极少使用。

### 2.3 Peek

```rust
#[pyclass(name = "Peek")]
pub struct PyPeek {
    pub(crate) inner: construct::constructs::stream_ops::Peek,
}

impl PyConstructWrapper for PyPeek {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "Peek", signature = (subcon))]
pub fn py_peek(subcon: &Bound<PyAny>) -> PyResult<PyPeek> {
    let sc = crate::py_adapter::extract_subcon(subcon)?;
    Ok(PyPeek {
        inner: construct::constructs::stream_ops::Peek::new(sc),
    })
}

impl_api_methods!(PyPeek);
impl_construct_operators!(PyPeek);
```

### 2.4 RawCopy

```rust
#[pyclass(name = "RawCopy")]
pub struct PyRawCopy {
    pub(crate) inner: construct::constructs::stream_ops::RawCopy,
}

impl PyConstructWrapper for PyRawCopy {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "RawCopy", signature = (subcon))]
pub fn py_raw_copy(subcon: &Bound<PyAny>) -> PyResult<PyRawCopy> {
    let sc = crate::py_adapter::extract_subcon(subcon)?;
    Ok(PyRawCopy {
        inner: construct::constructs::stream_ops::RawCopy::new(sc),
    })
}

impl_api_methods!(PyRawCopy);
impl_construct_operators!(PyRawCopy);
```

### 2.5 Prefixed

Python `Prefixed(lengthfield, subcon, includelength=False)`。

```rust
#[pyclass(name = "Prefixed")]
pub struct PyPrefixed {
    pub(crate) inner: construct::constructs::stream_ops::Prefixed,
}

impl PyConstructWrapper for PyPrefixed {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "Prefixed", signature = (lengthfield, subcon, includelength=false))]
pub fn py_prefixed(
    lengthfield: &Bound<PyAny>,
    subcon: &Bound<PyAny>,
    includelength: bool,
) -> PyResult<PyPrefixed> {
    let lf = crate::py_adapter::extract_subcon(lengthfield)?;
    let sc = crate::py_adapter::extract_subcon(subcon)?;
    let mut inner = construct::constructs::stream_ops::Prefixed::new(lf, sc);
    if includelength {
        inner = inner.with_include_length(true);
    }
    Ok(PyPrefixed { inner })
}

impl_api_methods!(PyPrefixed);
impl_construct_operators!(PyPrefixed);
```

### 2.6 Transformed

Python `Transformed(subcon, decodefunc, decodeamount, encodefunc, encodeamount)`。注意参数顺序：Python 先 decodefunc 后 decodeamount。

Rust `Transformed::new(subcon, decode, decode_amount, encode, encode_amount)`。

**关键**：decodefunc/encodefunc 是 Python 的 bytes-to-bytes 函数（如 `bytes2bits`），通过 `py_to_transform_func` 桥接。decodeamount/encodeamount 可以是 int 或 None（None 表示读取到 EOF）。

```rust
#[pyclass(name = "Transformed")]
pub struct PyTransformed {
    pub(crate) inner: construct::constructs::stream_ops::Transformed,
}

impl PyConstructWrapper for PyTransformed {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "Transformed", signature = (subcon, decodefunc, decodeamount, encodefunc, encodeamount))]
pub fn py_transformed(
    py: Python<'_>,
    subcon: &Bound<PyAny>,
    decodefunc: &Bound<PyAny>,
    decodeamount: Option<usize>,
    encodefunc: &Bound<PyAny>,
    encodeamount: Option<usize>,
) -> PyResult<PyTransformed> {
    let sc = crate::py_adapter::extract_subcon(subcon)?;
    let decode = crate::expr_bridge::py_to_transform_func(decodefunc.clone().unbind())?;
    let encode = crate::expr_bridge::py_to_transform_func(encodefunc.clone().unbind())?;
    Ok(PyTransformed {
        inner: construct::constructs::stream_ops::Transformed::new(
            sc, decode, decodeamount, encode, encodeamount,
        ),
    })
}

impl_api_methods!(PyTransformed);
impl_construct_operators!(PyTransformed);
```

### 2.7 Restreamed

Python `Restreamed(subcon, decoder, decoderunit, encoder, encoderunit, sizecomputer)`。

```rust
#[pyclass(name = "Restreamed")]
pub struct PyRestreamed {
    pub(crate) inner: construct::constructs::stream_ops::Restreamed,
}

impl PyConstructWrapper for PyRestreamed {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "Restreamed", signature = (subcon, decoder, decoderunit, encoder, encoderunit, sizecomputer))]
pub fn py_restreamed(
    py: Python<'_>,
    subcon: &Bound<PyAny>,
    decoder: &Bound<PyAny>,
    decoderunit: usize,
    encoder: &Bound<PyAny>,
    encoderunit: usize,
    sizecomputer: Option<&Bound<PyAny>>,
) -> PyResult<PyRestreamed> {
    let sc = crate::py_adapter::extract_subcon(subcon)?;
    let dec = crate::expr_bridge::py_to_transform_func(decoder.clone().unbind())?;
    let enc = crate::expr_bridge::py_to_transform_func(encoder.clone().unbind())?;
    let size_comp = match sizecomputer {
        Some(f) => Some(crate::expr_bridge::py_to_size_computer(f.clone().unbind())?),
        None => None,
    };
    Ok(PyRestreamed {
        inner: construct::constructs::stream_ops::Restreamed::new(
            sc, dec, decoderunit, enc, encoderunit, size_comp,
        ),
    })
}

impl_api_methods!(PyRestreamed);
impl_construct_operators!(PyRestreamed);
```

### 2.8 Compressed（feature-gated: flate2）

Python `Compressed(subcon, encoding, level=None)`。

**Cargo.toml 配置**：construct-py 的 Cargo.toml 启用 construct 的 compression feature：
```toml
construct = { path = "../construct-rs", features = ["compression"] }
```

```rust
#[pyclass(name = "Compressed")]
pub struct PyCompressed {
    pub(crate) inner: construct::constructs::stream_ops::Compressed,
}

impl PyConstructWrapper for PyCompressed {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "Compressed", signature = (subcon, encoding, level=None))]
pub fn py_compressed(
    subcon: &Bound<PyAny>,
    encoding: &str,
    level: Option<i32>,
) -> PyResult<PyCompressed> {
    let sc = crate::py_adapter::extract_subcon(subcon)?;
    let algorithm = match encoding {
        "zlib" => construct::constructs::stream_ops::CompressionAlgorithm::Zlib,
        "deflate" => construct::constructs::stream_ops::CompressionAlgorithm::Deflate,
        other => return Err(PyValueError::new_err(format!(
            "Unsupported compression '{}': only zlib/deflate are supported (got '{}')", encoding, other))),
    };
    // level 参数：Rust Compressed 当前使用 Compression::default()，忽略 level。
    // 标记为已知限制（level 不生效，但 zlib/deflate 测试用例通常不检查 level）。
    let _ = level;  // 接受但不使用
    Ok(PyCompressed {
        inner: construct::constructs::stream_ops::Compressed::new(sc, algorithm),
    })
}

impl_api_methods!(PyCompressed);
impl_construct_operators!(PyCompressed);
```

**已知限制**：
- `level` 参数被接受但不生效（Rust 使用 `flate2::Compression::default()`）。zlib/deflate 的压缩率测试可能因 level 不同而字节不同。如果测试用例检查压缩后的字节精确匹配，可能失败。**建议**：如果 test_compressed_zlib 失败，标记为已知限制。
- `encoding="gzip"` / `"bzip2"` / `"lzma"` / `"lz4"` 不支持，抛 `ValueError`。对应测试用例跳过。

### 2.9 Checksum

Python `Checksum(checksumfield, hashfunc, bytesfunc)`。

- `checksumfield`：Construct 实例（如 `Bytes(64)`）
- `hashfunc`：`callable(data: bytes) -> bytes`（如 `lambda data: hashlib.sha512(data).digest()`）
- `bytesfunc`：context lambda 返回 bytes（如 `this.fields.data`）

```rust
#[pyclass(name = "Checksum")]
pub struct PyChecksum {
    pub(crate) inner: construct::constructs::stream_ops::Checksum,
}

impl PyConstructWrapper for PyChecksum {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "Checksum", signature = (checksumfield, hashfunc, bytesfunc))]
pub fn py_checksum(
    py: Python<'_>,
    checksumfield: &Bound<PyAny>,
    hashfunc: &Bound<PyAny>,
    bytesfunc: &Bound<PyAny>,
) -> PyResult<PyChecksum> {
    let cf = crate::py_adapter::extract_subcon(checksumfield)?;
    let hf = crate::expr_bridge::py_to_checksum_func(hashfunc.clone().unbind())?;
    let bf = crate::expr_bridge::py_to_checksum_bytes_func(bytesfunc.clone().unbind())?;
    Ok(PyChecksum {
        inner: construct::constructs::stream_ops::Checksum::new(cf, hf, bf),
    })
}

impl_api_methods!(PyChecksum);
impl_construct_operators!(PyChecksum);
```

### 2.10 ByteSwapped / BitsSwapped

Python `ByteSwapped(subcon)` 和 `BitsSwapped(subcon)` 是**普通函数**，返回 `Transformed` 实例。Rust 有独立类型。

```rust
#[pyclass(name = "ByteSwapped")]
pub struct PyByteSwapped {
    pub(crate) inner: construct::constructs::stream_ops::ByteSwapped,
}

impl PyConstructWrapper for PyByteSwapped {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "ByteSwapped", signature = (subcon))]
pub fn py_byte_swapped(subcon: &Bound<PyAny>) -> PyResult<PyByteSwapped> {
    let sc = crate::py_adapter::extract_subcon(subcon)?;
    Ok(PyByteSwapped {
        inner: construct::constructs::stream_ops::ByteSwapped::new(sc),
    })
}

impl_api_methods!(PyByteSwapped);
impl_construct_operators!(PyByteSwapped);

#[pyclass(name = "BitsSwapped")]
pub struct PyBitsSwapped {
    pub(crate) inner: construct::constructs::stream_ops::BitsSwapped,
}

impl PyConstructWrapper for PyBitsSwapped {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "BitsSwapped", signature = (subcon))]
pub fn py_bits_swapped(subcon: &Bound<PyAny>) -> PyResult<PyBitsSwapped> {
    let sc = crate::py_adapter::extract_subcon(subcon)?;
    // Rust BitsSwapped 需要知道 subcon 是否固定大小。
    // 尝试 sizeof 判断：成功 → new_fixed，失败 → new_variable
    let ctx = construct::core::context::Context::new();
    let inner = match sc.sizeof(&ctx) {
        Ok(_) => construct::constructs::stream_ops::BitsSwapped::new_fixed(sc),
        Err(_) => construct::constructs::stream_ops::BitsSwapped::new_variable(sc),
    };
    Ok(PyBitsSwapped { inner })
}

impl_api_methods!(PyBitsSwapped);
impl_construct_operators!(PyBitsSwapped);
```

**注意**：`BitsSwapped` 的构造需要判断 subcon 是否固定大小。在工厂函数中尝试 `sizeof()`，成功用 `new_fixed`（Transformed 语义），失败用 `new_variable`（Restreamed 语义）。这匹配 Python 原版的 try/except SizeofError 逻辑。

### 2.11 FixedSized

Python `FixedSized(length, subcon)`。length 可以是 int 或 context lambda。

Rust `FixedSized` 在 `computed.rs` 中，仅接受 `usize`（固定 int），不支持表达式。

```rust
#[pyclass(name = "FixedSized")]
pub struct PyFixedSized {
    pub(crate) inner: construct::constructs::computed::FixedSized,
}

impl PyConstructWrapper for PyFixedSized {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "FixedSized", signature = (length, subcon))]
pub fn py_fixed_sized(
    length: &Bound<PyAny>,
    subcon: &Bound<PyAny>,
) -> PyResult<PyFixedSized> {
    let sc = crate::py_adapter::extract_subcon(subcon)?;
    // length: int only (Rust FixedSized 不支持表达式 length)
    if length.is_callable() {
        return Err(PyNotImplementedError::new_err(
            "FixedSized with context lambda length is not yet supported"));
    }
    let len: usize = length.extract()?;
    Ok(PyFixedSized {
        inner: construct::constructs::computed::FixedSized::new(len, sc),
    })
}

impl_api_methods!(PyFixedSized);
impl_construct_operators!(PyFixedSized);
```

**已知限制**：`length` 为 context lambda 时不支持（Rust FixedSized 仅接受 usize）。测试用例中 FixedSized 通常用固定 int。

### 2.12 Lazy / LazyStruct / LazyArray

Python `Lazy(subcon)` / `LazyStruct(*subcons)` / `LazyArray(count, subcon)`。

**Lazy 包装**（parse 返回 callable）：

```rust
#[pyclass(name = "Lazy")]
pub struct PyLazy {
    pub(crate) inner: construct::constructs::lazy::Lazy,
}

impl PyConstructWrapper for PyLazy {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pymethods]
impl PyLazy {
    /// Parse from bytes — returns a zero-arg callable (matching Python Lazy semantics).
    #[pyo3(signature = (data, **kw))]
    fn parse(
        &self,
        py: Python<'_>,
        data: &Bound<PyAny>,
        kw: Option<&Bound<PyDict>>,
    ) -> PyResult<PyObject> {
        let bytes: std::borrow::Cow<[u8]> = data.extract()?;
        let ctx = crate::api::opt_kw_to_indexmap(py, kw)?;
        // 即时解析得到 Value
        let value = crate::api::py_parse(&self.inner, py, &bytes, ctx)?;
        // 包装为 callable，调用时返回 value
        Ok(wrap_as_zero_arg_callable(py, value))
    }

    /// Parse from stream — returns a zero-arg callable.
    #[pyo3(signature = (stream, **kw))]
    fn parse_stream(
        &self,
        py: Python<'_>,
        stream: PyObject,
        kw: Option<&Bound<PyDict>>,
    ) -> PyResult<PyObject> {
        let ctx = crate::api::opt_kw_to_indexmap(py, kw)?;
        let value = crate::api::py_parse_stream(&self.inner, py, stream, ctx)?;
        Ok(wrap_as_zero_arg_callable(py, value))
    }

    // parse_file / build / build_stream / build_file / sizeof
    // 直接委托 impl_api_methods! 宏（Lazy.build 接受 callable 或 value）
}

/// 将一个 PyObject 包装为零参数 callable，调用时返回该对象。
///
/// 用于 Lazy.parse() 的返回值：Python 原版 Lazy 返回 execute() 闭包，
/// 调用时才解析。Rust 即时解析后包装为 callable 保持接口一致。
fn wrap_as_zero_arg_callable(py: Python<'_>, value: PyObject) -> PyObject {
    use pyo3::types::PyCFunction;
    PyCFunction::new_closure(py, None, None, move |_, _| {
        Ok(value.clone_ref(unsafe { Python::assume_gil_acquired() }))
    })
    .map(|f| f.into_any())
    .unwrap_or(value)  // fallback: 如果闭包创建失败，直接返回值
}

impl_construct_operators!(PyLazy);
```

**Lazy 的 build 行为**：Python `Lazy._build` 检查 `callable(obj)`，如果是 callable 则先调用 `obj()` 求值。Rust `Lazy::build` 直接委托 subcon.build。在 PyO3 包装层需要覆盖 build 方法：

```rust
// 在 #[pymethods] impl PyLazy 中
#[pyo3(signature = (obj, **kw))]
fn build(
    &self,
    py: Python<'_>,
    obj: &Bound<PyAny>,
    kw: Option<&Bound<PyDict>>,
) -> PyResult<PyObject> {
    // 如果 obj 是 callable，先调用 obj() 求值
    let actual_obj = if obj.is_callable() {
        obj.call0()?
    } else {
        obj.clone()
    };
    let ctx = crate::api::opt_kw_to_indexmap(py, kw)?;
    crate::api::py_build(&self.inner, py, &actual_obj, ctx)
}
```

**LazyStruct 包装**：

```rust
#[pyclass(name = "LazyStruct")]
pub struct PyLazyStruct {
    pub(crate) inner: construct::constructs::lazy::LazyStruct,
    pub(crate) py_subcons: Vec<PyObject>,  // 用于 __getattr__ 成员暴露
}

impl PyConstructWrapper for PyLazyStruct {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "LazyStruct", signature = (*subcons, **subconskw))]
pub fn py_lazy_struct(
    py: Python<'_>,
    subcons: &Bound<PyTuple>,
    subconskw: &Bound<PyDict>,
) -> PyResult<PyLazyStruct> {
    let mut builder = construct::constructs::lazy::LazyStruct::new();
    let mut py_subcons = Vec::new();
    // 收集 subcons（同 Struct 的逻辑）
    for item in subcons.iter() {
        let inner = crate::py_adapter::extract_subcon(item)?;
        let name = crate::constructs_composite::get_subcon_name(item)?;
        match name {
            Some(n) => { builder = builder.field(n, inner); }
            None    => { builder = builder.anonymous(inner); }
        }
        py_subcons.push(item.into_py(py));
    }
    // **subconskw 处理（同 Struct）
    Ok(PyLazyStruct { inner: builder, py_subcons })
}

impl_api_methods!(PyLazyStruct);
impl_construct_operators!(PyLazyStruct);
```

**LazyArray 包装**：

```rust
#[pyclass(name = "LazyArray")]
pub struct PyLazyArray {
    pub(crate) inner: construct::constructs::lazy::LazyArray,
}

impl PyConstructWrapper for PyLazyArray {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "LazyArray", signature = (count, subcon))]
pub fn py_lazy_array(
    count: &Bound<PyAny>,
    subcon: &Bound<PyAny>,
) -> PyResult<PyLazyArray> {
    let sc = crate::py_adapter::extract_subcon(subcon)?;
    // count 仅支持 int（Rust LazyArray 仅接受 usize）
    if count.is_callable() {
        return Err(PyNotImplementedError::new_err(
            "LazyArray with expression count is not supported"));
    }
    let n: usize = count.extract()?;
    Ok(PyLazyArray {
        inner: construct::constructs::lazy::LazyArray::new(n, sc),
    })
}

impl_api_methods!(PyLazyArray);
impl_construct_operators!(PyLazyArray);
```

**已知限制**：`LazyArray` 的 count 仅支持 int（不支持表达式）。

### 2.13 LazyBound

Python `LazyBound(ctxfunc)` — ctxfunc 是**无参 callable**返回 subcon。用于递归结构。

Rust `LazyBound::new(subcon_func: Box<dyn Fn() -> Box<dyn Construct>>)`。

**桥接**：Python 的无参 callable 返回一个构造器对象（PyO3 包装或纯 Python），需要通过 `extract_subcon` 提取。

```rust
#[pyclass(name = "LazyBound")]
pub struct PyLazyBound {
    pub(crate) inner: construct::constructs::stream_ops::LazyBound,
}

impl PyConstructWrapper for PyLazyBound {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "LazyBound", signature = (ctxfunc))]
pub fn py_lazy_bound(
    py: Python<'_>,
    ctxfunc: &Bound<PyAny>,
) -> PyResult<PyLazyBound> {
    let func_obj: PyObject = ctxfunc.clone().unbind();
    let subcon_func: Box<dyn Fn() -> Box<dyn Construct>> = Box::new(move || {
        Python::with_gil(|py| {
            let py_constr = func_obj.call0(py).map_err(|e| {
                ConstructError::Generic {
                    path: String::new(),
                    message: format!("LazyBound ctxfunc error: {e}"),
                }
            }).unwrap();  // 注意：此处 panic 风险，见下方说明
            crate::py_adapter::extract_subcon(py_constr.bind(py))
                .unwrap_or_else(|_| Box::new(construct::constructs::meta::Error::new()))
        })
    });
    Ok(PyLazyBound {
        inner: construct::constructs::stream_ops::LazyBound::new(subcon_func),
    })
}

impl_api_methods!(PyLazyBound);
impl_construct_operators!(PyLazyBound);
```

**panic 风险**：`LazyBound` 的 `subcon_func` 闭包在 Rust parse/build/sizeof 路径中调用，返回 `Box<dyn Construct>`（非 Result）。如果 Python `ctxfunc()` 抛异常或 `extract_subcon` 失败，闭包内部 unwrap 会 panic。

**缓解方案**：闭包返回 `Error` 构造器（parse/build 永远返回 Err）作为 fallback，而非 panic。这是已有模式（见 stream_ops.rs LazyBound 文档）。

### 2.14 Rebuffered

Python `Rebuffered(subcon, tailcutoff=None)`。

```rust
#[pyclass(name = "Rebuffered")]
pub struct PyRebuffered {
    pub(crate) inner: construct::constructs::lazy::Rebuffered,
}

impl PyConstructWrapper for PyRebuffered {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

#[pyfunction]
#[pyo3(name = "Rebuffered", signature = (subcon, tailcutoff=None))]
pub fn py_rebuffered(
    subcon: &Bound<PyAny>,
    tailcutoff: Option<usize>,
) -> PyResult<PyRebuffered> {
    let sc = crate::py_adapter::extract_subcon(subcon)?;
    let mut inner = construct::constructs::lazy::Rebuffered::new(sc);
    if let Some(tc) = tailcutoff {
        inner = inner.with_tailcutoff(tc);
    }
    Ok(PyRebuffered { inner })
}

impl_api_methods!(PyRebuffered);
impl_construct_operators!(PyRebuffered);
```

---

## 3. 函数式构造器（层 2）

### 3.1 BitStruct（10.6 遗留）

Python `BitStruct(*subcons, **subconskw)` = `Bitwise(Struct(*subcons, **subconskw))`。

```rust
/// `BitStruct(*subcons)` → Bitwise(Struct(*subcons))
#[pyfunction]
#[pyo3(name = "BitStruct", signature = (*subcons, **subconskw))]
pub fn py_bit_struct(
    py: Python<'_>,
    subcons: &Bound<PyTuple>,
    subconskw: &Bound<PyDict>,
) -> PyResult<PyBitwise> {
    // 先构造 PyStruct（复用 10.6 的 py_struct 逻辑）
    let py_struct = crate::constructs_composite::py_struct(py, subcons, subconskw)?;
    // 提取 inner Struct，用 Bitwise 包装
    let sc: Box<dyn Construct> = Box::new(py_struct.inner);
    Ok(PyBitwise {
        inner: construct::constructs::stream_ops::Bitwise::new(sc),
    })
}
```

**注意**：返回 `PyBitwise`（而非 `PyStruct`），因为 `isinstance(d, BitStruct)` 在 Python 原版中实际检查的是 `Bitwise`（BitStruct 是函数）。如果测试检查 `type(d).__name__ == "BitStruct"`，需额外处理。但 Python 原版 `BitStruct` 是函数，`type(Bitwise(Struct(...)))` 是 `Transformed` 或 `Restreamed`，不检查类型名。因此返回 `PyBitwise` 是安全的。

### 3.2 PrefixedArray

Python `PrefixedArray(countfield, subcon)` = `FocusedSeq("items", "count"/Rebuild(countfield, len_(this.items)), "items"/subcon[this.count])`。

```rust
/// `PrefixedArray(countfield, subcon)` → FocusedSeq
#[pyfunction]
#[pyo3(name = "PrefixedArray", signature = (countfield, subcon))]
pub fn py_prefixed_array(
    py: Python<'_>,
    countfield: &Bound<PyAny>,
    subcon: &Bound<PyAny>,
) -> PyResult<crate::constructs_composite::PyFocusedSeq> {
    // 构造 count field: Rebuild(countfield, len_(this.items))
    let cf = crate::py_adapter::extract_subcon(countfield)?;
    // len_(this.items) 表达式：需要 ExprBridge 支持 len_ + this 组合
    // 此处复用 10.4 的表达式桥接
    let count_expr = crate::expr_bridge::make_len_this_items_expr();
    let count_rebuild = construct::constructs::computed::Rebuild::new(cf, count_expr);

    // items field: subcon[this.count]
    let sc = crate::py_adapter::extract_subcon(subcon)?;
    let count_ref = crate::expr_bridge::make_this_count_expr();
    let items_array = construct::constructs::repetition::ArrayExpr::new(count_ref, sc);

    // FocusedSeq("items", count_field, items_field)
    let focused = construct::constructs::focused_seq::FocusedSeq::new(
        "items",
        vec![
            construct::constructs::struct_::StructField::new("count", Box::new(count_rebuild)),
            construct::constructs::struct_::StructField::new("items", Box::new(items_array)),
        ],
    );
    Ok(crate::constructs_composite::PyFocusedSeq { inner: focused })
}
```

**表达式构造辅助函数**（在 `expr_bridge.rs` 新增）：

```rust
/// 构造 `len_(this.items)` 表达式。
pub fn make_len_this_items_expr() -> Box<dyn Evaluate> { /* ... */ }

/// 构造 `this.count` 表达式。
pub fn make_this_count_expr() -> Box<dyn Evaluate> { /* ... */ }
```

**替代方案**（如果表达式组合复杂）：PrefixedArray 可以用纯 Python 实现，继承 FocusedSeq 或直接组合。但这会引入 Python 回调开销。推荐优先用 Rust 组合。

### 3.3 OffsettedEnd

Python `OffsettedEnd(endoffset, subcon)` — endoffset 为负数或零（从 EOF 向前的偏移）。

**实现策略**：
- 如果 subcon 有固定大小：可以用 Transformed 组合（读取 `sizeof(subcon) + endoffset` 字节 → 透传 → subcon 解析）。但 endoffset 通常为负，且读取的是"EOF 前 endoffset 字节"，这与 Transformed 的"读取固定字节数"语义不同（OffsettedEnd 是从当前位置读到 EOF-endoffset）。
- **最佳方案**：纯 Python 实现（见 §4.6），因为 OffsettedEnd 需要 seek 到 EOF 计算总长度，这在 PyO3 包装层用自定义 Construct 实现不如纯 Python 清晰。

**PyO3 工厂函数**（委托纯 Python 实现）：

```rust
/// OffsettedEnd 在纯 Python 中实现（_stream.py）。
/// PyO3 不导出 OffsettedEnd，由 __init__.py 从 _stream 导入。
```

OffsettedEnd 在 `__init__.py` 中从 `_stream` 导入：
```python
from ._stream import OffsettedEnd, NullTerminated, NullStripped, RestreamData, ProcessXor, ProcessRotateLeft
```

---

## 4. 纯 Python 实现的构造器（层 3）

### 4.1 设计依据

以下构造器选择纯 Python 实现的统一理由：

| 构造器 | 不用 Rust 实现的理由 |
|--------|-------------------|
| `NullTerminated` | 逐字节（或逐 unit 字节）读取直到 term，支持 4 个布尔参数（include/consume/require），逻辑复杂且涉及 Python 流的逐字节 IO |
| `NullStripped` | 读取全部后 rstrip，涉及 Python bytes 的 rstrip 语义（多字节 pad 的尾部对齐处理） |
| `RestreamData` | datafunc 可能是 bytes / BytesIO / Construct 实例，三种分支处理是 Python 动态类型特性 |
| `ProcessXor` | padfunc 是 context lambda（需 Python 端 evaluate），Rust Transformed 的 `Fn(&[u8])` 无 context 不兼容 |
| `ProcessRotateLeft` | amount/group 是 context lambda，且位旋转算法使用 Python 的预计算表 |
| `OffsettedEnd` | 需要 seek 到 EOF 计算总长度，涉及流的 seek/tell 操作，纯 Python 最清晰 |

**共同模式**：所有纯 Python 构造器继承 `_adapter.py` 的 `Subconstruct` 基类（10.4/10.7 已实现），通过 dual-path 机制（`_parsereport` vs `parse_stream`）在嵌套场景中正确委托内嵌的 Rust subcon。

### 4.2 NullTerminated

**文件**：`construct-py/construct_rust/_stream.py`

```python
"""Stream-processing constructs implemented in pure Python.

These constructs depend on Python runtime features (byte-at-a-time IO,
context lambda evaluation, itertools) or have no Rust equivalent.
They inherit from the Subconstruct base class (10.4/10.7) and use the
dual-path mechanism for correct delegation to nested Rust subcons.
"""

from io import BytesIO

from ._adapter import Subconstruct, Construct
from .lib.containers import Container, ListContainer


class NullTerminated(Subconstruct):
    r"""Restricts parsing to bytes preceding a null byte.

    See construct.core.NullTerminated for full documentation.
    """

    def __init__(self, subcon, term=b"\x00", include=False, consume=True, require=True):
        super().__init__(subcon)
        self.term = term
        self.include = include
        self.consume = consume
        self.require = require

    def _parse(self, stream, context, path):
        term = self.term
        unit = len(term)
        if unit < 1:
            from . import PaddingError
            raise PaddingError("NullTerminated term must be at least 1 byte", path=path)
        data = b""
        while True:
            try:
                b = stream.read(unit)
                if len(b) < unit:
                    raise EOFError()
            except Exception:
                if self.require:
                    from . import StreamError
                    raise StreamError(
                        "stream read less than specified amount", path=path)
                break
            if b == term:
                if self.include:
                    data += b
                if not self.consume:
                    stream.seek(-unit, 1)
                break
            data += b
        return self.subcon._parsereport(BytesIO(data), context, path)

    def _build(self, obj, stream, context, path):
        buildret = self.subcon._build(obj, stream, context, path)
        stream.write(self.term)
        return buildret

    def _sizeof(self, context, path):
        from . import SizeofError
        raise SizeofError(path=path)
```

### 4.3 NullStripped

```python
class NullStripped(Subconstruct):
    r"""Restricts parsing to bytes except padding left of EOF."""

    def __init__(self, subcon, pad=b"\x00"):
        super().__init__(subcon)
        self.pad = pad

    def _parse(self, stream, context, path):
        pad = self.pad
        unit = len(pad)
        if unit < 1:
            from . import PaddingError
            raise PaddingError("NullStripped pad must be at least 1 byte", path=path)
        data = stream.read()
        if unit == 1:
            data = data.rstrip(pad)
        else:
            tailunit = len(data) % unit
            end = len(data)
            if tailunit and data[-tailunit:] == pad[:tailunit]:
                end -= tailunit
            while end - unit >= 0 and data[end - unit:end] == pad:
                end -= unit
            data = data[:end]
        return self.subcon._parsereport(BytesIO(data), context, path)

    def _build(self, obj, stream, context, path):
        return self.subcon._build(obj, stream, context, path)

    def _sizeof(self, context, path):
        from . import SizeofError
        raise SizeofError(path=path)
```

### 4.4 RestreamData

```python
class RestreamData(Subconstruct):
    r"""Parses a field on external data (does not build)."""

    def __init__(self, datafunc, subcon):
        super().__init__(subcon)
        self.datafunc = datafunc
        self.flagbuildnone = True

    def _parse(self, stream, context, path):
        from .expr import evaluate
        data = evaluate(self.datafunc, context)
        if isinstance(data, bytes):
            stream2 = BytesIO(data)
        elif isinstance(data, BytesIO):
            stream2 = data
        elif isinstance(data, Construct):
            stream2 = BytesIO(data._parsereport(stream, context, path))
        else:
            from . import StreamError
            raise StreamError("RestreamData datafunc returned unsupported type", path=path)
        return self.subcon._parsereport(stream2, context, path)

    def _build(self, obj, stream, context, path):
        return obj

    def _sizeof(self, context, path):
        return 0
```

### 4.5 ProcessXor

```python
import itertools


class ProcessXor(Subconstruct):
    r"""XOR-transforms bytes between the underlying stream and subcon."""

    def __init__(self, padfunc, subcon):
        super().__init__(subcon)
        self.padfunc = padfunc

    def _parse(self, stream, context, path):
        from .expr import evaluate
        from .lib.py3compat import byte2int
        pad = evaluate(self.padfunc, context)
        if not isinstance(pad, (int, bytes)):
            from . import StringError
            raise StringError("ProcessXor needs integer or bytes pad", path=path)
        if isinstance(pad, bytes) and len(pad) == 1:
            pad = byte2int(pad)
        data = stream.read()
        if isinstance(pad, int):
            if pad != 0:
                data = bytes(b ^ pad for b in data)
        elif isinstance(pad, bytes):
            if not (len(pad) <= 64 and pad == bytes(len(pad))):
                data = bytes(b ^ p for b, p in zip(data, itertools.cycle(pad)))
        return self.subcon._parsereport(BytesIO(data), context, path)

    def _build(self, obj, stream, context, path):
        from .expr import evaluate
        from .lib.py3compat import byte2int
        pad = evaluate(self.padfunc, context)
        if not isinstance(pad, (int, bytes)):
            from . import StringError
            raise StringError("ProcessXor needs integer or bytes pad", path=path)
        if isinstance(pad, bytes) and len(pad) == 1:
            pad = byte2int(pad)
        stream2 = BytesIO()
        buildret = self.subcon._build(obj, stream2, context, path)
        data = stream2.getvalue()
        if isinstance(pad, int):
            if pad != 0:
                data = bytes(b ^ pad for b in data)
        elif isinstance(pad, bytes):
            if not (len(pad) <= 64 and pad == bytes(len(pad))):
                data = bytes(b ^ p for b, p in zip(data, itertools.cycle(pad)))
        stream.write(data)
        return buildret

    def _sizeof(self, context, path):
        return self.subcon._sizeof(context, path)
```

### 4.6 ProcessRotateLeft

```python
class ProcessRotateLeft(Subconstruct):
    r"""Rotates (shifts) bytes left by amount in bits, within group-sized chunks."""

    _precomputed_single_rotations = {
        amount: [(i << amount) & 0xff | (i >> (8 - amount)) for i in range(256)]
        for amount in range(1, 8)
    }

    def __init__(self, amount, group, subcon):
        super().__init__(subcon)
        self.amount = amount
        self.group = group

    def _rotate(self, data, amount, group, path):
        from .expr import evaluate
        if group < 1:
            from . import RotationError
            raise RotationError("group size must be at least 1 to be valid", path=path)
        amount = amount % (group * 8)
        amount_bytes = amount // 8
        if len(data) % group != 0:
            from . import RotationError
            raise RotationError("data length must be a multiple of group size", path=path)
        if amount == 0:
            return data
        elif group == 1:
            translate = self._precomputed_single_rotations[amount]
            return bytes(translate[a] for a in data)
        elif amount % 8 == 0:
            indices = [(i + amount_bytes) % group for i in range(group)]
            return bytes(data[i + k] for i in range(0, len(data), group) for k in indices)
        else:
            amount1 = amount % 8
            amount2 = 8 - amount1
            indices_pairs = [
                ((i + amount_bytes) % group, (i + 1 + amount_bytes) % group)
                for i in range(group)
            ]
            return bytes(
                (data[i + k1] << amount1) & 0xff | (data[i + k2] >> amount2)
                for i in range(0, len(data), group)
                for k1, k2 in indices_pairs
            )

    def _parse(self, stream, context, path):
        from .expr import evaluate
        amount = evaluate(self.amount, context)
        group = evaluate(self.group, context)
        data = stream.read()
        data = self._rotate(data, amount, group, path)
        return self.subcon._parsereport(BytesIO(data), context, path)

    def _build(self, obj, stream, context, path):
        from .expr import evaluate
        amount = evaluate(self.amount, context)
        group = evaluate(self.group, context)
        stream2 = BytesIO()
        buildret = self.subcon._build(obj, stream2, context, path)
        data = stream2.getvalue()
        # build 时反向旋转（取负）
        data = self._rotate(data, -amount % (group * 8), group, path)
        stream.write(data)
        return buildret

    def _sizeof(self, context, path):
        return self.subcon._sizeof(context, path)
```

### 4.7 OffsettedEnd

```python
class OffsettedEnd(Subconstruct):
    r"""Parses all bytes till EOF plus a negative endoffset is reached."""

    def __init__(self, endoffset, subcon):
        super().__init__(subcon)
        self.endoffset = endoffset

    def _parse(self, stream, context, path):
        from .expr import evaluate
        endoffset = evaluate(self.endoffset, context)
        curpos = stream.tell()
        stream.seek(0, 2)  # seek to end
        endpos = stream.tell()
        stream.seek(curpos, 0)  # seek back
        length = endpos + endoffset - curpos
        data = stream.read(length)
        return self.subcon._parsereport(BytesIO(data), context, path)

    def _build(self, obj, stream, context, path):
        return self.subcon._build(obj, stream, context, path)

    def _sizeof(self, context, path):
        from . import SizeofError
        raise SizeofError(path=path)
```

---

## 5. 桥接器设计

本节定义 `expr_bridge.rs` 中新增的 4 个桥接函数。

### 5.1 py_to_transform_func

将 Python callable 桥接为 `TransformFunc`（`Fn(&[u8]) -> Result<Vec<u8>>`）。

用于 `Transformed` 和 `Restreamed` 的 decoder/encoder。

```rust
/// 将 Python callable 桥接为 stream_ops::TransformFunc。
///
/// Python 调用约定：callable(data: bytes) -> bytes
/// 通过 GIL 调用，参数和返回值均为 bytes。
pub fn py_to_transform_func(callable: PyObject) -> construct::constructs::stream_ops::TransformFunc {
    Box::new(move |data: &[u8]| {
        Python::with_gil(|py| {
            let py_input = PyBytes::new_bound(py, data);
            let result = callable
                .call1(py, (py_input,))
                .map_err(|e| ConstructError::Generic {
                    path: String::new(),
                    message: format!("Transform function error: {e}"),
                })?;
            let py_bytes = result.bind(py).downcast::<PyBytes>()
                .map_err(|_| ConstructError::Generic {
                    path: String::new(),
                    message: "Transform function must return bytes".to_string(),
                })?;
            Ok(py_bytes.as_bytes().to_vec())
        })
    })
}
```

### 5.2 py_to_checksum_func

将 Python callable 桥接为 `ChecksumFunc`（`Fn(&[u8]) -> Vec<u8>>`）。

```rust
/// 将 Python callable 桥接为 stream_ops::ChecksumFunc。
///
/// Python 调用约定：callable(data: bytes) -> bytes
/// 用于 hashlib 类哈希函数。
pub fn py_to_checksum_func(callable: PyObject) -> construct::constructs::stream_ops::ChecksumFunc {
    Box::new(move |data: &[u8]| {
        Python::with_gil(|py| {
            let py_input = PyBytes::new_bound(py, data);
            match callable.call1(py, (py_input,)) {
                Ok(result) => {
                    if let Ok(py_bytes) = result.bind(py).downcast::<PyBytes>() {
                        py_bytes.as_bytes().to_vec()
                    } else {
                        Vec::new()  // fallback: 空校验和
                    }
                }
                Err(_) => Vec::new(),  // fallback
            }
        })
    })
}
```

### 5.3 py_to_checksum_bytes_func

将 Python callable 桥接为 `ChecksumBytesFunc`（`Fn(&Context) -> Result<Vec<u8>>`）。

Python 用户通常传入 context 表达式（如 `this.fields.data`），通过 PyClosure 求值。

```rust
/// 将 Python callable/表达式 桥接为 stream_ops::ChecksumBytesFunc。
///
/// Python 调用约定：callable(context) -> bytes
/// 支持 Path/BinExpr/lambda（通过 PyClosure）。
pub fn py_to_checksum_bytes_func(
    callable: PyObject,
) -> construct::constructs::stream_ops::ChecksumBytesFunc {
    let closure = PyClosure::new(callable);
    Box::new(move |ctx: &Context| {
        let val = closure.evaluate(ctx, None)?;
        match val {
            Value::Bytes(b) => Ok(b),
            other => Err(ConstructError::Generic {
                path: String::new(),
                message: format!(
                    "Checksum bytesfunc must return bytes, got {:?}",
                    other.type_name()
                ),
            }),
        }
    })
}
```

### 5.4 py_to_size_computer

将 Python callable 桥接为 `Option<Box<dyn Fn(usize) -> usize>>`。

```rust
/// 将 Python callable 桥接为 Restreamed 的 size_computer。
///
/// Python 调用约定：callable(n: int) -> int
pub fn py_to_size_computer(
    callable: PyObject,
) -> PyResult<Box<dyn Fn(usize) -> usize>> {
    Box::new(move |n: usize| {
        Python::with_gil(|py| {
            let result = callable.call1(py, (n,));
            match result {
                Ok(r) => r.extract::<usize>(py).unwrap_or(0),
                Err(_) => 0,
            }
        })
    });
    // 注意：Box<dyn Fn> 不返回 Result，需要特殊处理
}
```

**实现说明**：`py_to_size_computer` 返回 `PyResult<Box<dyn Fn(usize) -> usize>>`。由于 `Fn` 不返回 Result，错误处理只能是 fallback（返回 0 或 panic）。实际实现中应确保 callable 正常工作。如果 GIL 获取失败或 callable 抛异常，fallback 返回 0（导致 sizeof 返回 0，可能不准确但不 panic）。

**替代方案**：如果需要更健壮的错误处理，可以将 `size_computer` 的类型改为 `Option<Box<dyn Fn(usize) -> Result<usize>>>`。但这需要修改 Rust 内核 `Restreamed` 的类型定义（超出 ARCH 权限）。当前方案保持 fallback 行为。

---

## 6. Python API 映射表

### 6.1 PyO3 包装类（层 1）

| Python 原版 | Rust 内核类型 | PyO3 包装类 | 映射说明 |
|------------|-------------|------------|---------|
| `Bitwise(subcon)` | `stream_ops::Bitwise` | `PyBitwise` | 工厂函数（Python def → Rust struct） |
| `Bytewise(subcon)` | `stream_ops::Bytewise` | `PyBytewise` | 工厂函数 |
| `Pointer(offset, subcon)` | `Pointer` / `PointerExpr` | `PyPointer` | int→Pointer, callable→PointerExpr |
| `Peek(subcon)` | `stream_ops::Peek` | `PyPeek` | |
| `RawCopy(subcon)` | `stream_ops::RawCopy` | `PyRawCopy` | |
| `Prefixed(lengthfield, subcon, includelength)` | `stream_ops::Prefixed` | `PyPrefixed` | includelength→with_include_length |
| `Transformed(subcon, decodefunc, decodeamount, encodefunc, encodeamount)` | `stream_ops::Transformed` | `PyTransformed` | decodefunc/encodefunc→TransformFunc 桥接 |
| `Restreamed(subcon, decoder, decoderunit, encoder, encoderunit, sizecomputer)` | `stream_ops::Restreamed` | `PyRestreamed` | 同上 + sizecomputer 桥接 |
| `Compressed(subcon, encoding, level)` | `stream_ops::Compressed` | `PyCompressed` | 仅 zlib/deflate；level 接受但不使用 |
| `Checksum(checksumfield, hashfunc, bytesfunc)` | `stream_ops::Checksum` | `PyChecksum` | hashfunc→ChecksumFunc, bytesfunc→ChecksumBytesFunc |
| `ByteSwapped(subcon)` | `stream_ops::ByteSwapped` | `PyByteSwapped` | 需固定大小 |
| `BitsSwapped(subcon)` | `stream_ops::BitsSwapped` | `PyBitsSwapped` | sizeof 判断 fixed/variable |
| `FixedSized(length, subcon)` | `computed::FixedSized` | `PyFixedSized` | length 仅 int |
| `Lazy(subcon)` | `lazy::Lazy` | `PyLazy` | parse 返回 callable 包装 |
| `LazyStruct(*subcons)` | `lazy::LazyStruct` | `PyLazyStruct` | 即时解析 |
| `LazyArray(count, subcon)` | `lazy::LazyArray` | `PyLazyArray` | count 仅 int |
| `LazyBound(ctxfunc)` | `stream_ops::LazyBound` | `PyLazyBound` | ctxfunc→Fn()→Box<dyn Construct> |
| `Rebuffered(subcon, tailcutoff)` | `lazy::Rebuffered` | `PyRebuffered` | tailcutoff 接受但不限制缓冲 |

### 6.2 函数式构造器（层 2）

| Python 原版 | 返回类型 | 实现方式 |
|------------|---------|---------|
| `BitStruct(*subcons)` | `PyBitwise` | `= Bitwise(Struct(*subcons))` |
| `PrefixedArray(countfield, subcon)` | `PyFocusedSeq` | `= FocusedSeq("items", Rebuild+ArrayExpr)` |

### 6.3 纯 Python 实现的构造器（层 3）

| Python 原版 | Python 文件 | 继承基类 |
|------------|-----------|---------|
| `OffsettedEnd(endoffset, subcon)` | `_stream.py` | `Subconstruct` |
| `NullTerminated(subcon, term, include, consume, require)` | `_stream.py` | `Subconstruct` |
| `NullStripped(subcon, pad)` | `_stream.py` | `Subconstruct` |
| `RestreamData(datafunc, subcon)` | `_stream.py` | `Subconstruct` |
| `ProcessXor(padfunc, subcon)` | `_stream.py` | `Subconstruct` |
| `ProcessRotateLeft(amount, group, subcon)` | `_stream.py` | `Subconstruct` |

### 6.4 不实现的构造器

| Python 原版 | 理由 | 测试处理 |
|------------|------|---------|
| `CompressedLZ4(subcon)` | 无 lz4 依赖 | `test_compressedlz4` 跳过 |

---

## 7. 边界条件清单

### 7.1 Bitwise / Bytewise

| # | 场景 | 输入 | 预期行为 |
|---|------|------|---------|
| 1 | Bitwise 固定大小 | `Bitwise(Bytes(8)).sizeof()` | 1（8 bits / 8） |
| 2 | Bitwise 向上取整 | `Bitwise(Bytes(12)).sizeof()` | 2（ceil(12/8)） |
| 3 | Bitwise parse | `Bitwise(Bytes(8)).parse(b"\xff")` | `b"\x01" * 8`（8 个 bit） |
| 4 | Bytewise 在 Bitwise 内 | `Bitwise(Struct("b"/Bytewise(Byte)))` | 字节级访问恢复 |
| 5 | Bytewise sizeof | `Bytewise(Bytes(2)).sizeof()` | 16（2 bytes * 8 bits） |

### 7.2 Pointer / Peek

| # | 场景 | 输入 | 预期行为 |
|---|------|------|---------|
| 6 | Pointer 正偏移 | `Pointer(8, Bytes(1)).parse(b"abcdefghijkl")` | `b"i"` |
| 7 | Pointer 负偏移 | `Pointer(-2, Bytes(1)).parse(b"abcdefghijkl")` | `b"k"` |
| 8 | Pointer sizeof | `Pointer(5, Bytes(2)).sizeof()` | 0 |
| 9 | Pointer 表达式偏移 | `Pointer(this.off, Bytes(1))` | PointerExpr |
| 10 | Pointer relativeOffset | `Pointer(3, sc, relativeOffset=True)` | **不支持**（NotImplementedError） |
| 11 | Pointer stream 参数 | `Pointer(3, sc, stream=lambda)` | **不支持**（NotImplementedError） |
| 12 | Peek 不前进 | `Peek(Byte).parse(b"\x2a\xff")` → 位置仍为 0 | |
| 13 | Peek 解析失败 | `Peek(Bytes(100)).parse(b"\x01")` | 返回 None（非 Check/StopField 错误） |
| 14 | Peek sizeof | `Peek(Byte).sizeof()` | 0 |
| 15 | Peek flagbuildnone | `Peek(Byte).build(None)` | Ok（不写任何字节） |

### 7.3 RawCopy / Prefixed

| # | 场景 | 输入 | 预期行为 |
|---|------|------|---------|
| 16 | RawCopy parse | `RawCopy(Byte).parse(b"\xff")` | `Container(data=b"\xff", value=255, offset1=0, offset2=1, length=1)` |
| 17 | RawCopy build 有 data | `build(dict(data=b"\xff"))` | 写入 `b"\xff"` |
| 18 | RawCopy build 有 value | `build(dict(value=255))` | 构建子构造器 |
| 19 | RawCopy build 无 data/value | `build({})` | 错误 |
| 20 | Prefixed 基本解析 | `Prefixed(Byte, GreedyBytes).parse(b"\x03ABC")` | `b"ABC"` |
| 21 | Prefixed includelength | `Prefixed(Byte, sc, includelength=True)` | length 减去 lengthfield 大小 |
| 22 | Prefixed sizeof | `Prefixed(Byte, Bytes(4)).sizeof()` | 5（1 + 4） |

### 7.4 Transformed / Restreamed

| # | 场景 | 输入 | 预期行为 |
|---|------|------|---------|
| 23 | Transformed 固定大小 | `Transformed(Bytes(16), bytes2bits, 2, bits2bytes, 2)` | decodeamount=2 |
| 24 | Transformed None amount | `Transformed(GreedyBytes, bytes2bits, None, bits2bytes, None)` | 读取到 EOF |
| 25 | Transformed sizeof 相等 | decodeamount == encodeamount | 返回该值 |
| 26 | Transformed sizeof 不等 | decodeamount != encodeamount | SizeofError |
| 27 | Restreamed chunk 解码 | `Restreamed(sc, decoder, 1, encoder, 8, lambda n: n//8)` | 逐块变换 |
| 28 | Restreamed 无 sizecomputer | sizecomputer=None | sizeof → SizeofError |

### 7.5 Compressed

| # | 场景 | 输入 | 预期行为 |
|---|------|------|---------|
| 29 | Compressed zlib | `Compressed(GreedyBytes, "zlib")` | flate2 ZlibDecoder/Encoder |
| 30 | Compressed deflate | `Compressed(GreedyBytes, "deflate")` | flate2 DeflateDecoder/Encoder |
| 31 | Compressed gzip | `Compressed(GreedyBytes, "gzip")` | **ValueError**（不支持） |
| 32 | Compressed bzip2 | `Compressed(GreedyBytes, "bzip2")` | **ValueError** |
| 33 | Compressed sizeof | `Compressed(sc, "zlib").sizeof()` | SizeofError（大小不定） |
| 34 | Compressed level | `Compressed(sc, "zlib", level=9)` | level 接受但使用 default（已知限制） |

### 7.6 Checksum

| # | 场景 | 输入 | 预期行为 |
|---|------|------|---------|
| 35 | Checksum parse 匹配 | parsed == computed | 返回 parsed 值 |
| 36 | Checksum parse 不匹配 | parsed != computed | ChecksumError |
| 37 | Checksum build | `build(None)` | 计算 hashfunc(bytesfunc(ctx)) 并写入 |
| 38 | Checksum flagbuildnone | `Checksum(...).build(None)` | Ok（总是可构建） |

### 7.7 Lazy 系列

| # | 场景 | 输入 | 预期行为 |
|---|------|------|---------|
| 39 | Lazy parse 返回 callable | `x = Lazy(Byte).parse(b"\x2a"); x()` | callable，调用返回 42 |
| 40 | Lazy build callable | `Lazy(Byte).build(lambda: 42)` | 先调用 `obj()` 求值，再构建 |
| 41 | Lazy build value | `Lazy(Byte).build(42)` | 直接构建 |
| 42 | LazyStruct parse | `LazyStruct("a"/Byte).parse(b"\x01")` | Container(a=1)（即时解析） |
| 43 | LazyArray parse | `LazyArray(3, Byte).parse(b"\x01\x02\x03")` | [1,2,3]（即时解析） |
| 44 | LazyBound 递归 | `LazyBound(lambda: Struct("next"/Byte))` | 每次调用 ctxfunc() 获取 subcon |
| 45 | Rebuffered 基本 | `Rebuffered(Byte).parse(b"\x05")` | 5（全量缓冲后解析） |
| 46 | Rebuffered 消耗同步 | `Rebuffered(Bytes(2)).parse(b"\x01\x02\x03\x04")` | 解析后流位置在 2（非 4） |

### 7.8 纯 Python 构造器

| # | 场景 | 输入 | 预期行为 |
|---|------|------|---------|
| 47 | NullTerminated 基本 | `NullTerminated(Byte).parse(b"\xff\x00")` | 255 |
| 48 | NullTerminated include | `NullTerminated(GreedyBytes, include=True).parse(b"\x01\x00")` | `b"\x01\x00"`（含 term） |
| 49 | NullTerminated consume=False | `NullTerminated(GreedyBytes, consume=False)` | 解析后 term 留在流中 |
| 50 | NullTerminated require=False | `NullTerminated(GreedyBytes, require=False)` | EOF 时不报错 |
| 51 | NullTerminated build | `NullTerminated(Byte).build(255)` | `b"\xff\x00"` |
| 52 | NullStripped 基本 | `NullStripped(Byte).parse(b"\xff\x00\x00")` | 255（rstrip 后） |
| 53 | NullStripped build | `NullStripped(Byte).build(255)` | `b"\xff"`（不加 pad） |
| 54 | RestreamData bytes | `RestreamData(b"\x01", Byte).parse(b"")` | 1 |
| 55 | RestreamData build | `RestreamData(b"\x01", Byte).build(0)` | `b""`（不写任何字节） |
| 56 | RestreamData sizeof | `RestreamData(...).sizeof()` | 0 |
| 57 | ProcessXor int pad | `ProcessXor(0xf0, Int16ub).parse(b"\x00\xff")` | 0xf00f |
| 58 | ProcessXor bytes pad | `ProcessXor(b"\xf0\xf0", sc)` | 循环 XOR |
| 59 | ProcessRotateLeft group=1 | `ProcessRotateLeft(4, 1, Int16ub).parse(b"\x0f\xf0")` | 0xf00f |
| 60 | ProcessRotateLeft group=2 | `ProcessRotateLeft(4, 2, Int16ub).parse(b"\x0f\xf0")` | 0xff00 |
| 61 | OffsettedEnd 基本 | `OffsettedEnd(-2, GreedyBytes).parse(b"...")` | 读取到 EOF-2 |

---

## 8. 与其他模块的交互

### 8.1 依赖的已设计模块

| 模块 | 依赖内容 |
|------|---------|
| **10.2 Value/Context/错误映射** | `py_to_value` / `value_to_py` / `context_to_py_container` / 错误映射 |
| **10.4 表达式桥接** | `PyClosure`（Evaluate trait）/ `py_param_to_evaluate` / `py_to_cond_func` |
| **10.5/10.6 构造器包装** | `extract_subcon` / `PyConstructWrapper` trait / `impl_api_methods!` / `impl_construct_operators!` 宏 |
| **10.6 复合构造器** | `PyStruct` / `PyFocusedSeq` / `get_subcon_name`（BitStruct/PrefixedArray 组合） |
| **10.7 适配器** | `py_to_decode_func` 等双参数桥接器（设计参考）/ `_adapter.py` Subconstruct 基类（纯 Python 构造器继承） |
| **Phase 1-7 Rust 内核** | `stream_ops::*` / `lazy::*` / `computed::FixedSized` / `binary`（bytes2bits/bits2bytes/swapbytes） |

### 8.2 被依赖的后续模块

| 模块 | 被依赖内容 |
|------|-----------|
| **10.9 字符串构造器** | `NullTerminated` / `NullStripped`（CString 等字符串构造器内部使用） |
| **10.10 测试套件** | 所有 10.8 构造器导出名 |

### 8.3 extract_subcon 类型分支补全

`try_extract_registered`（`constructs_adapter.rs:1273`）需要增补以下 10.8 包装类型分支：

```rust
pub fn try_extract_registered(obj: &Bound<PyAny>) -> PyResult<Option<Box<dyn Construct>>> {
    use constructs_stream::*;
    // 流操作/隧道/惰性（10.8）
    if let Ok(w) = obj.extract::<PyRef<PyBitwise>>()    { return Ok(Some(Box::new(w.inner.clone()))); }  // 需要 Clone
    if let Ok(w) = obj.extract::<PyRef<PyBytewise>>()   { return Ok(Some(Box::new(w.inner.clone()))); }
    if let Ok(w) = obj.extract::<PyRef<PyPointer>>()    { return Ok(Some(w.inner.clone_box())); }
    if let Ok(w) = obj.extract::<PyRef<PyPeek>>()       { return Ok(Some(Box::new(w.inner.clone()))); }
    if let Ok(w) = obj.extract::<PyRef<PyRawCopy>>()    { return Ok(Some(Box::new(w.inner.clone()))); }
    if let Ok(w) = obj.extract::<PyRef<PyPrefixed>>()   { return Ok(Some(Box::new(w.inner.clone()))); }
    if let Ok(w) = obj.extract::<PyRef<PyTransformed>>() { return Ok(Some(/* clone 逻辑 */)); }
    if let Ok(w) = obj.extract::<PyRef<PyRestreamed>>() { return Ok(Some(/* clone */)); }
    if let Ok(w) = obj.extract::<PyRef<PyCompressed>>() { return Ok(Some(Box::new(w.inner.clone()))); }
    if let Ok(w) = obj.extract::<PyRef<PyChecksum>>()   { return Ok(Some(/* clone */)); }
    if let Ok(w) = obj.extract::<PyRef<PyByteSwapped>>() { return Ok(Some(Box::new(w.inner.clone()))); }
    if let Ok(w) = obj.extract::<PyRef<PyBitsSwapped>>() { return Ok(Some(Box::new(w.inner.clone()))); }
    if let Ok(w) = obj.extract::<PyRef<PyFixedSized>>() { return Ok(Some(Box::new(w.inner.clone()))); }
    if let Ok(w) = obj.extract::<PyRef<PyLazy>>()       { return Ok(Some(Box::new(w.inner.clone()))); }
    if let Ok(w) = obj.extract::<PyRef<PyLazyStruct>>() { return Ok(Some(Box::new(w.inner.clone()))); }
    if let Ok(w) = obj.extract::<PyRef<PyLazyArray>>()  { return Ok(Some(Box::new(w.inner.clone()))); }
    if let Ok(w) = obj.extract::<PyRef<PyLazyBound>>()  { return Ok(Some(/* clone */)); }
    if let Ok(w) = obj.extract::<PyRef<PyRebuffered>>() { return Ok(Some(Box::new(w.inner.clone()))); }
    Ok(None)
}
```

**Clone 策略**（同 10.7 §6.1 的 PM 裁定方案 A）：

Transformed/Restreamed/Checksum/LazyBound 持有闭包（`Box<dyn Fn>`），无法直接 Clone。这些包装类需要额外持有原始 Python callable 的 `PyObject` 引用，clone 时重新桥接闭包。

**包装类字段扩展**（以 PyTransformed 为例）：

```rust
#[pyclass(name = "Transformed")]
pub struct PyTransformed {
    pub(crate) inner: construct::constructs::stream_ops::Transformed,
    /// 原始 Python callable 引用（用于 clone_box_construct）。
    decode_obj: PyObject,
    encode_obj: PyObject,
}

impl PyTransformed {
    pub fn clone_inner(&self, py: Python<'_>) -> Box<dyn Construct> {
        let decode = crate::expr_bridge::py_to_transform_func(self.decode_obj.clone_ref(py));
        let encode = crate::expr_bridge::py_to_transform_func(self.encode_obj.clone_ref(py));
        // 需要访问 inner.subcon，但 subcon 是 Box<dyn Construct>（无法 clone）
        // 方案：从 py_subcon 重新 extract（同 PyRenamed 的策略）
        todo!("需要 inner.subcon 的 clone 方案")
    }
}
```

**替代方案**（推荐，避免 Clone 复杂性）：为 `Construct` trait 添加 `fn clone_box(&self) -> Box<dyn Construct>` 方法。此方案已在 10.6 §5.4 提出，需 PM 裁定。如果 PM 批准修改 Construct trait，所有包装类的 clone 问题一次性解决。

---

## 9. 需 PM 裁定的问题

### 问题 1：Construct trait 的 clone_box 方法

**背景**：`Transformed`/`Restreamed`/`Checksum`/`LazyBound` 持有 `Box<dyn Construct>` + `Box<dyn Fn>` 闭包。PyO3 包装类的 `extract_subcon` 需要 owned `Box<dyn Construct>`，但 `dyn Construct` 不要求 Clone。

**两个方案**：

| 方案 | 描述 | 优点 | 缺点 |
|------|------|------|------|
| **A: 修改 Construct trait** | 添加 `fn clone_box(&self) -> Box<dyn Construct>` 默认方法（用 `Any` + type-tag 或要求 `Clone` bound） | 所有包装类的 clone 问题一次性解决；代码简洁 | 修改 Phase 1 已验收 trait（影响面大） |
| **B: 每个包装类持有原始 PyObject** | 包装类额外存储 Python callable 引用 + subcon 的 PyObject，clone 时重新桥接 | 不修改内核 | clone 开销大（重新穿越 FFI）；代码冗余 |

**建议方案 A**。理由：
1. 10.6 §5.4 和 10.7 §6.1 都已提出此需求，三个子任务（10.6/10.7/10.8）的 clone 问题需要统一解决
2. 修改面可控：仅向 Construct trait 添加一个方法 + 为所有已实现类型实现该方法
3. 已验收的 1306 测试不受影响（clone_box 是新方法，不改变现有行为）

### 问题 2：Compressed level 参数

**背景**：Rust `Compressed` 使用 `flate2::Compression::default()`，不接受 level 参数。Python 原版 `Compressed(subcon, encoding, level=None)` 支持指定压缩级别。

**影响**：如果测试用例检查压缩后的字节精确匹配，不同 level 会导致字节不同。

**建议**：接受 level 参数但不使用（标记为已知限制）。如果 `test_compressed_zlib` / `test_compressed_deflate` 因 level 失败，在验收报告中记录。Rust 可以在 `Compressed::new` 添加 `with_level` builder 方法（需修改内核），但这超出 ARCH 权限。

### 问题 3：Lazy parse 返回 callable 的 PyCFunction 安全性

**背景**：`wrap_as_zero_arg_callable` 使用 `PyCFunction::new_closure` 创建闭包。闭包捕获 `value: PyObject`，在调用时 `clone_ref`。

**风险**：如果闭包在 Python 侧被多次调用，每次返回同一 value 的 clone_ref。这匹配 Python 原版行为（Lazy 返回的 execute() 闭包多次调用返回同一解析结果）。

**需要确认**：`PyCFunction::new_closure` 在 PyO3 0.22 中是否稳定可用。如果 API 不稳定，替代方案是用纯 Python 的 `functools.partial` 或自定义 callable 类。

---

## 10. 验证策略

### 10.1 单元测试（Rust 端）

| 测试类别 | 测试数 | 覆盖内容 |
|---------|--------|---------|
| PyO3 包装工厂函数 | ~30 | 每个工厂函数的正确构造 + 错误分支 |
| TransformFunc/ChecksumFunc 桥接 | ~10 | bytes-to-bytes callable 正确调用 |
| Lazy callable 包装 | ~5 | parse 返回 callable + build callable 求值 |
| Compressed encoding 映射 | ~4 | zlib/deflate/不支持编码错误 |
| 纯 Python 构造器集成 | ~10 | Python 测试（pytest）验证 NullTerminated 等 |

### 10.2 Python 端测试

| 测试文件 | 相关用例 | 预期 |
|---------|---------|------|
| `test_core.py` | `test_bitwise*` / `test_bytewise*` / `test_pointer*` / `test_peek*` / `test_rawcopy*` | pass |
| `test_core.py` | `test_prefixed*` / `test_prefixedarray*` / `test_fixedsized*` | pass |
| `test_core.py` | `test_transformed*` / `test_restreamed*` | pass |
| `test_core.py` | `test_compressed_zlib` / `test_compressed_deflate` | pass（或 level 限制跳过） |
| `test_core.py` | `test_compressed_gzip` / `_bzip2` / `_lzma` | skip（不支持） |
| `test_core.py` | `test_compressedlz4` | skip |
| `test_core.py` | `test_checksum*` | pass |
| `test_core.py` | `test_processxor*` / `test_processrotateleft*` | pass（纯 Python） |
| `test_core.py` | `test_lazy*` / `test_lazystruct*` / `test_lazyarray*` | pass（callable 包装） |
| `test_core.py` | `test_rebuffered*` | pass |
| `test_core.py` | `test_bitstruct*` | pass |

---

## 11. lib.rs 注册清单

### 11.1 新增 pyclass 注册（register_classes）

```rust
// 流操作 / 隧道 / 惰性（10.8）
m.add_class::<constructs_stream::PyBitwise>()?;
m.add_class::<constructs_stream::PyBytewise>()?;
m.add_class::<constructs_stream::PyPointer>()?;
m.add_class::<constructs_stream::PyPeek>()?;
m.add_class::<constructs_stream::PyRawCopy>()?;
m.add_class::<constructs_stream::PyPrefixed>()?;
m.add_class::<constructs_stream::PyTransformed>()?;
m.add_class::<constructs_stream::PyRestreamed>()?;
m.add_class::<constructs_stream::PyCompressed>()?;
m.add_class::<constructs_stream::PyChecksum>()?;
m.add_class::<constructs_stream::PyByteSwapped>()?;
m.add_class::<constructs_stream::PyBitsSwapped>()?;
m.add_class::<constructs_stream::PyFixedSized>()?;
m.add_class::<constructs_stream::PyLazy>()?;
m.add_class::<constructs_stream::PyLazyStruct>()?;
m.add_class::<constructs_stream::PyLazyArray>()?;
m.add_class::<constructs_stream::PyLazyBound>()?;
m.add_class::<constructs_stream::PyRebuffered>()?;
```

### 11.2 新增 pyfunction 注册（register_functions）

```rust
// 流操作 / 隧道 / 惰性（10.8）
m.add_function(wrap_pyfunction!(constructs_stream::py_bitwise, m)?)?;
m.add_function(wrap_pyfunction!(constructs_stream::py_bytewise, m)?)?;
m.add_function(wrap_pyfunction!(constructs_stream::py_pointer, m)?)?;
m.add_function(wrap_pyfunction!(constructs_stream::py_peek, m)?)?;
m.add_function(wrap_pyfunction!(constructs_stream::py_raw_copy, m)?)?;
m.add_function(wrap_pyfunction!(constructs_stream::py_prefixed, m)?)?;
m.add_function(wrap_pyfunction!(constructs_stream::py_transformed, m)?)?;
m.add_function(wrap_pyfunction!(constructs_stream::py_restreamed, m)?)?;
m.add_function(wrap_pyfunction!(constructs_stream::py_compressed, m)?)?;
m.add_function(wrap_pyfunction!(constructs_stream::py_checksum, m)?)?;
m.add_function(wrap_pyfunction!(constructs_stream::py_byte_swapped, m)?)?;
m.add_function(wrap_pyfunction!(constructs_stream::py_bits_swapped, m)?)?;
m.add_function(wrap_pyfunction!(constructs_stream::py_fixed_sized, m)?)?;
m.add_function(wrap_pyfunction!(constructs_stream::py_lazy, m)?)?;
m.add_function(wrap_pyfunction!(constructs_stream::py_lazy_struct, m)?)?;
m.add_function(wrap_pyfunction!(constructs_stream::py_lazy_array, m)?)?;
m.add_function(wrap_pyfunction!(constructs_stream::py_lazy_bound, m)?)?;
m.add_function(wrap_pyfunction!(constructs_stream::py_rebuffered, m)?)?;
m.add_function(wrap_pyfunction!(constructs_stream::py_bit_struct, m)?)?;
m.add_function(wrap_pyfunction!(constructs_stream::py_prefixed_array, m)?)?;
```

### 11.3 纯 Python 导入（__init__.py）

```python
# construct_rust/__init__.py 片段
from ._core import *  # PyO3 包装类和工厂函数
# 纯 Python 流操作构造器（层 3）
from ._stream import (
    OffsettedEnd,
    NullTerminated,
    NullStripped,
    RestreamData,
    ProcessXor,
    ProcessRotateLeft,
)
```

**注意导入顺序**：`from ._core import *` 必须先于 `from ._stream import *`，因为 `_stream.py` 中的纯 Python 构造器继承 `_adapter.py` 的 `Subconstruct` 基类，而 `Subconstruct` 需要访问 `_core` 中的 Rust 构造器（通过 duck typing `parse_stream`/`build_stream`）。

### 11.4 Cargo.toml 修改

```toml
# construct-py/Cargo.toml
[dependencies]
construct = { path = "../construct-rs", features = ["compression"] }  # 启用 compression feature
```
