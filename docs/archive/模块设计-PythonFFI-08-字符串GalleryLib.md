# 模块设计：PythonFFI-08 字符串构造器 + Gallery + lib 子模块

> 子任务 10.9。覆盖字符串构造器（PaddedString / PascalString / CString /
> GreedyString）的 Python 暴露、Gallery 格式解析器（ELF / PE32COFF /
> UTIndex）的导出、`construct_rust.lib` 纯 Python 子模块（binary /
> py3compat / hex / bitstream）的逐行移植，以及 `construct_rust/debug.py`
> 调试工具（Probe / Debugger）的 stub 实现。

## 模块位置

### 本子任务新增文件

| 文件 | 类型 | 职责 |
|------|------|------|
| `construct-py/src/constructs_string.rs` | Rust (PyO3) | `PyPaddedString` / `PyCString` 包装类 + `py_padded_string` / `py_cstring` 工厂函数 |
| `construct-py/src/gallery.rs` | Rust (PyO3) | `py_elf` / `py_pe32coff` / `py_ut_index` 工厂函数，返回 PyO3 构造器对象 |
| `construct-py/construct_rust/_string.py` | 纯 Python | `StringEncoded` Adapter + `PascalString` / `GreedyString` macro 函数 |
| `construct-py/construct_rust/lib/binary.py` | 纯 Python | 整数/位/字节转换工具（逐行移植原版 169 行） |
| `construct-py/construct_rust/lib/py3compat.py` | 纯 Python | Python 2/3 兼容层（逐行移植原版 43 行） |
| `construct-py/construct_rust/lib/hex.py` | 纯 Python | hex dump 工具（逐行移植原版 94 行） |
| `construct-py/construct_rust/lib/bitstream.py` | 纯 Python | 流包装器（逐行移植原版 147 行） |
| `construct-py/construct_rust/debug.py` | 纯 Python | `Probe` / `Debugger` stub 实现 |

### 本子任务扩展文件

| 文件 | 扩展内容 |
|------|---------|
| `construct-py/src/lib.rs` | 注册 `constructs_string` + `gallery` 模块；新增 `py_padded_string` / `py_c_string` / `py_elf` / `py_pe32coff` / `py_ut_index` pyfunction |
| `construct-py/src/py_adapter.rs` | `try_extract_registered` 增补 `PyPaddedString` / `PyCString` 分支（使二者可嵌套于 Rust Struct/Sequence 中） |
| `construct-py/construct_rust/__init__.py` | 导入 `_string`（注入 PascalString/GreedyString/StringEncoded）+ `debug`（注入 Probe/Debugger） |
| `construct-py/construct_rust/lib/__init__.py` | 取消注释 `binary` / `py3compat` / `hex` / `bitstream` 的 re-export 行 |

## 职责

将 Rust 内核中已实现的字符串构造器（CString / PaddedString）通过 PyO3 暴露给
Python；将无 Rust 对应实现的字符串构造器（PascalString / GreedyString）以纯
Python macro 组合方式实现（与原版 `StringEncoded(...)` macro 策略一致）；将
Rust gallery（ELF / PE32COFF / UTIndex）导出为 Python 可用的构造器对象；逐
行移植原版 `construct.lib` 的 4 个纯 Python 工具子模块；为调试工具（Probe /
Debugger）提供 stub 实现，使 `import construct_rust` 不因缺失这些名称而失败。

---

## 1. 核心设计决策

### 1.1 四类组件的实现策略

10.9 包含的组件按以下决策树选择实现策略：

| 组件 | 策略 | 理由 |
|------|------|------|
| `PaddedString(length, encoding)` | **PyO3 包装** | Rust 内核 `constructs::strings::PaddedString` 已实现（含 8 种编码、padding/strip/sizeof 完整逻辑） |
| `CString(encoding)` | **PyO3 包装** | Rust 内核 `constructs::strings::CString` 已实现（含 null terminator 检测、多字节编码单元） |
| `PascalString(lengthfield, encoding)` | **纯 Python macro** | Rust 内核无对应实现；原版为 `StringEncoded(Prefixed(lengthfield, GreedyBytes), encoding)` 组合，Prefixed + GreedyBytes 已在 10.5/10.8 暴露 |
| `GreedyString(encoding)` | **纯 Python macro** | Rust 内核无对应实现；原版为 `StringEncoded(GreedyBytes, encoding)`，GreedyBytes 已在 10.5 暴露 |
| Gallery: ELF / PE32COFF / UTIndex | **PyO3 工厂函数** | Rust `gallery::{elf, pe32coff, ut_index}` 已实现，返回 `Box<dyn Construct>` |
| `lib/binary.py` | **纯 Python 逐行移植** | 纯算法函数，无 Rust 依赖；逐行移植保证测试零差异 |
| `lib/py3compat.py` | **纯 Python 逐行移植** | Python 运行时兼容层，Rust 无意义 |
| `lib/hex.py` | **纯 Python 逐行移植** | 纯格式化函数 |
| `lib/bitstream.py` | **纯 Python 逐行移植** | Python 流包装器，依赖 Python `io` 语义 |
| `debug.py`: Probe / Debugger | **纯 Python stub** | `test_probe` / `test_debugger` 已标记跳过（见总纲跳过表）；仅需 import 成功 |

### 1.2 字符串构造器的分层策略（关键决策）

原版 Python 的 4 个字符串构造器**全部是 macro 函数**（非类），内部用
`StringEncoded(Adapter)` 包装其他构造器：

```python
# 原版 core.py:1747-1858
PaddedString(length, encoding) = StringEncoded(FixedSized(length, NullStripped(GreedyBytes, pad=encodingunit(encoding))), encoding)
PascalString(lengthfield, encoding) = StringEncoded(Prefixed(lengthfield, GreedyBytes), encoding)
CString(encoding) = StringEncoded(NullTerminated(GreedyBytes, term=encodingunit(encoding)), encoding)
GreedyString(encoding) = StringEncoded(GreedyBytes, encoding)
```

本设计采用**混合策略**：

- **PaddedString / CString** → PyO3 包装 Rust 内核实现（性能优先，Rust 已有
  完整实现）。Python 侧表现为 `#[pyfunction]` 工厂，返回 `#[pyclass]` 包装对
  象，对外 API 与原版 macro 完全一致。
- **PascalString / GreedyString** → 纯 Python macro（一致性优先）。这两个构
  造器在 Rust 内核不存在，若新增 Rust 实现需修改 `construct-rs/src/`（超出
  ARCH 权限且非本阶段目标）。采用与原版**完全相同**的 macro 组合方式，复用
  已暴露的 `Prefixed` / `GreedyBytes` + 纯 Python `StringEncoded` Adapter。

**为何不为 PascalString/GreedyString 新增 Rust 实现？**

1. Rust 内核 `CString`/`PaddedString` 是 Phase 2 已验收的独立 struct，新增
   `PascalString`/`GreedyString` 需要回归测试，扩大本阶段改动面。
2. 原版这两个构造器本身就是组合 macro（非独立类），纯 Python 复刻能保证
   **零行为差异**——组合的原子构造器（Prefixed/GreedyBytes）已由 Rust 高速
   执行，仅最外层的 bytes↔str 转换在 Python 中完成，性能损失可忽略。
3. `StringEncoded._decode`/`_encode` 依赖 Python `bytes.decode`/`str.encode`
   的完整编码别名表（含 `utf_8`/`u8`/`utf_16_be` 等非标准别名），Rust
   `StringEncoding` 枚举仅支持 8 个标准名，纯 Python 能直接复用 Python 编解码
   器的全部别名。

### 1.3 编码名映射（Rust ↔ Python）

Rust `StringEncoding` 枚举仅支持 8 个标准名（Ascii/Utf8/Utf16/Utf16Be/
Utf16Le/Utf32/Utf32Be/Utf32Le），而 Python 原版 `possiblestringencodings`
支持 13 个别名（含 `utf_8`/`u8`/`utf_16_be`/`utf_16_le`/`utf_32_be`/
`utf_32_le` 等）。

**PyO3 包装层（PaddedString/CString）**：需在 Rust 侧实现 `encoding_name_to_enum()`
函数，将 Python 字符串名映射到 `StringEncoding` 枚举：

| Python 编码名（小写，`-`→`_`） | Rust StringEncoding |
|-------------------------------|---------------------|
| `ascii` | `Ascii` |
| `utf8` / `utf_8` / `u8` | `Utf8` |
| `utf16` / `utf_16` / `u16` | `Utf16` |
| `utf_16_be` | `Utf16Be` |
| `utf_16_le` | `Utf16Le` |
| `utf32` / `utf_32` / `u32` | `Utf32` |
| `utf_32_be` | `Utf32Be` |
| `utf_32_le` | `Utf32Le` |

不在表中的编码名 → 抛 `StringError`（对应原版 `encodingunit()` 的
`StringError("encoding %r not found among %r")`）。

**纯 Python 层（PascalString/GreedyString）**：直接透传编码名字符串给
Python `bytes.decode(encoding)`，自动支持全部 Python 编码别名。

---

## 2. 字符串构造器详细设计

### 2.1 PyPaddedString（PyO3 包装）

对应 Rust 内核：`construct::constructs::strings::PaddedString`

```rust
// constructs_string.rs

/// PyO3 wrapper: fixed-length null-padded string.
#[pyclass(name = "PaddedString", unsendable)]
pub struct PyPaddedString {
    pub(crate) inner: construct::constructs::strings::PaddedString,
}

impl PyConstructWrapper for PyPaddedString {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

/// Factory: `PaddedString(length, encoding)`.
/// length: int or context-lambda; encoding: str ("utf8"/"ascii"/"utf16"/...)
#[pyfunction]
#[pyo3(name = "PaddedString")]
pub fn py_padded_string(
    length: PyObject,       // int 或 callable（context lambda）
    encoding: &str,
) -> PyResult<PyPaddedString> {
    // 1. 若 length 是 int，直接构造
    // 2. 若 length 是 callable，暂不支持（PaddedString Rust 内核 length 是 usize，非表达式）
    //    → 抛 ValueError 提示"context lambda length 需使用纯 Python 组合"
    // 3. encoding_name_to_enum(encoding)? → StringEncoding
    let len = extract_length(&length)?;   // PyObject → usize（仅接受 int）
    let enc = encoding_name_to_enum(encoding)?;
    Ok(PyPaddedString {
        inner: construct::constructs::strings::PaddedString::new(len, enc),
    })
}

crate::impl_api_methods!(PyPaddedString);
crate::impl_construct_operators!(PyPaddedString);
```

**`encoding_name_to_enum` 辅助函数**（本文件内私有）：

```rust
fn encoding_name_to_enum(encoding: &str) -> PyResult<construct::constructs::strings::StringEncoding> {
    use construct::constructs::strings::StringEncoding;
    // 规范化：小写 + '-' → '_'
    let normalized = encoding.to_lowercase().replace('-', "_");
    match normalized.as_str() {
        "ascii" => Ok(StringEncoding::Ascii),
        "utf8" | "utf_8" | "u8" => Ok(StringEncoding::Utf8),
        "utf16" | "utf_16" | "u16" => Ok(StringEncoding::Utf16),
        "utf_16_be" => Ok(StringEncoding::Utf16Be),
        "utf_16_le" => Ok(StringEncoding::Utf16Le),
        "utf32" | "utf_32" | "u32" => Ok(StringEncoding::Utf32),
        "utf_32_be" => Ok(StringEncoding::Utf32Be),
        "utf_32_le" => Ok(StringEncoding::Utf32Le),
        other => Err(PyErr::new::<exceptions::StringError, _>(format!(
            "encoding {:?} not found among supported encodings", other
        ))),
    }
}
```

**边界条件（PaddedString）**：

| 场景 | 行为 |
|------|------|
| `length` 为 callable（context lambda） | 抛 `ValueError`（Rust 内核 length 为编译期 usize，不支持表达式；原版支持但需纯 Python 组合实现，见下文"限制说明"） |
| 编码名不在支持表 | 抛 `StringError`（对应原版 `encodingunit()` 的错误） |
| build 时编码后字节数 > length | 抛 `PaddingError`（Rust 内核已实现） |
| sizeof 时 length 非 encoding unit 整数倍 | 抛 `SizeofError`（Rust 内核已实现，如 `PaddedString(3, "utf16")`） |

**限制说明（context lambda length）**：原版 `PaddedString(this.count, "utf8")`
支持 context lambda 作为 length。Rust 内核 `PaddedString.length: usize` 是固
定值，不支持表达式。若测试用例使用 context lambda length，需 fallback 到纯
Python 组合（`StringEncoded(FixedSized(length, NullStripped(GreedyBytes, pad=...)), encoding)`）。
**预判**：`test_core.py` 中 PaddedString 测试均使用整数字面量 length，此限制
不影响验收。若 REF 验证发现个别用例使用 lambda length，标记为已知限制并补
充纯 Python fallback（见第 10 节"需 PM 裁定"）。

### 2.2 PyCString（PyO3 包装）

对应 Rust 内核：`construct::constructs::strings::CString`

```rust
/// PyO3 wrapper: null-terminated string.
#[pyclass(name = "CString", unsendable)]
pub struct PyCString {
    pub(crate) inner: construct::constructs::strings::CString,
}

impl PyConstructWrapper for PyCString {
    fn as_construct(&self) -> &dyn Construct { &self.inner }
}

/// Factory: `CString(encoding)`.
#[pyfunction]
#[pyo3(name = "CString")]
pub fn py_c_string(encoding: &str) -> PyResult<PyCString> {
    let enc = encoding_name_to_enum(encoding)?;
    Ok(PyCString {
        inner: construct::constructs::strings::CString::new(enc),
    })
}

crate::impl_api_methods!(PyCString);
crate::impl_construct_operators!(PyCString);
```

**边界条件（CString）**：

| 场景 | 行为 |
|------|------|
| 编码名不在支持表 | 抛 `StringError` |
| parse 时 EOF 前未遇到 null terminator | 抛 `ConstructError`（Rust 内核 `CSTRING_EOF_MESSAGE`） |
| sizeof | 抛 `SizeofError`（变长，Rust 内核已实现） |

### 2.3 StringEncoded Adapter（纯 Python，PascalString/GreedyString 共用）

逐行移植原版 `core.py:1711-1744` 的 `StringEncoded` 类。继承 `_adapter.Adapter`：

```python
# construct_rust/_string.py

from ._adapter import Adapter
from .exceptions import StringError   # PyO3 异常（10.2 已暴露）


class StringEncoded(Adapter):
    """Used internally. Ported from construct.core.StringEncoded."""

    def __init__(self, subcon, encoding):
        super().__init__(subcon)
        if not encoding:
            raise StringError("String* classes require explicit encoding")
        self.encoding = encoding

    def _decode(self, obj, context, path):
        try:
            return obj.decode(self.encoding)
        except:
            raise StringError(f"cannot use encoding {self.encoding!r} to decode {obj!r}")

    def _encode(self, obj, context, path):
        if not isinstance(obj, str):
            raise StringError("string encoding failed, expected unicode string", path=path)
        if obj == u"":
            return b""
        try:
            return obj.encode(self.encoding)
        except:
            raise StringError(f"cannot use encoding {self.encoding!r} to encode {obj!r}")
```

**关键点**：
- `obj` 来自 subcon（Prefixed/GreedyBytes）的 parse 结果，类型为 `bytes`
- `_decode` 将 `bytes` → `str`，`_encode` 将 `str` → `bytes`
- 空 unicode 字符串 `u""` → 空字节串 `b""`（原版特殊处理，line 1729-1730）
- 异常一律转为 `StringError`（与原版 `except:` 裸捕获一致）

### 2.4 PascalString（纯 Python macro）

```python
# construct_rust/_string.py（续）

def PascalString(lengthfield, encoding):
    """Length-prefixed string. Ported from construct.core.PascalString."""
    return StringEncoded(Prefixed(lengthfield, GreedyBytes), encoding)
```

`Prefixed`（10.8 已暴露）和 `GreedyBytes`（10.5 已暴露）均为 PyO3 包装对象。
`StringEncoded` 作为纯 Python Adapter，通过 `_adapter.py` 的双路径机制委托到
Rust subcon（见 10.7 设计文档 §1.2 混合架构）。

**行为对应表**：

| 操作 | Python 原版 | 本实现 |
|------|------------|--------|
| parse | `Prefixed(lengthfield, GreedyBytes)._parse()` → `StringEncoded._decode()` | 相同（Prefixed/GreedyBytes 走 Rust，_decode 走 Python） |
| build | `StringEncoded._encode()` → `Prefixed._build()` | 相同 |
| sizeof | `Prefixed` sizeof 依赖 lengthfield（变长 → SizeofError） | 相同 |

### 2.5 GreedyString（纯 Python macro）

```python
# construct_rust/_string.py（续）

def GreedyString(encoding):
    """String that reads entire stream until EOF. Ported from construct.core.GreedyString."""
    return StringEncoded(GreedyBytes, encoding)
```

`GreedyBytes`（10.5 已暴露）读取到 EOF，`StringEncoded._decode()` 将结果
bytes → str。

---

## 3. Gallery 暴露

### 3.1 Rust gallery 现状

Rust 内核 `construct::gallery` 模块已实现 3 个格式解析器（Phase 8 验收）：

| 模块 | 工厂函数 | 返回类型 | Python 对应 |
|------|---------|---------|-------------|
| `gallery::elf` | `elf()` | `Struct` | `gallery/elf.py` 的顶层 `elf` Struct |
| `gallery::pe32coff` | `pe32file()` | `Struct` | `gallery/pe32coff.py` 的 `pe32file` |
| `gallery::ut_index` | `UTIndex` (struct) | `impl Construct` | `gallery/ut_index.py` 的 `UTIndex` |

**Python 原版 gallery 导出**（`gallery/__init__.py`）：

```python
from .pe32coff import pe32file
from .ut_index import UTIndex
# elf 在 test_gallery.py 中直接 from construct.gallery.elf import elf
```

### 3.2 PyO3 工厂函数（gallery.rs）

由于 Rust gallery 工厂函数返回的是具体的 `Struct` / `UTIndex`（非
`Box<dyn Construct>`），需要一个适配层将其装箱为 `Box<dyn Construct>` 并包装
进通用 PyO3 构造器包装类。

**方案**：复用 10.6 已有的通用包装机制（若存在 `PyBoxedConstruct` 或类似通
用包装类）。若无，则定义一个轻量包装：

```rust
// gallery.rs

use construct::core::Construct;
use pyo3::prelude::*;
use crate::constructs_composite::PyConstructWrapper;  // 复用 trait

/// 通用 Box<dyn Construct> 包装，用于 gallery 工厂返回值。
#[pyclass(name = "GalleryParser", unsendable)]
pub struct PyGalleryParser {
    pub(crate) inner: Box<dyn Construct>,
}

impl PyConstructWrapper for PyGalleryParser {
    fn as_construct(&self) -> &dyn Construct { self.inner.as_ref() }
}

crate::impl_api_methods!(PyGalleryParser);
crate::impl_construct_operators!(PyGalleryParser);

/// Factory: ELF parser.
#[pyfunction]
#[pyo3(name = "elf")]
pub fn py_elf() -> PyGalleryParser {
    PyGalleryParser { inner: Box::new(construct::gallery::elf::elf()) }
}

/// Factory: PE32/COFF parser.
#[pyfunction]
#[pyo3(name = "pe32file")]
pub fn py_pe32coff() -> PyGalleryParser {
    PyGalleryParser { inner: Box::new(construct::gallery::pe32coff::pe32file()) }
}

/// Factory: Unreal Tournament Index parser.
#[pyfunction]
#[pyo3(name = "UTIndex")]
pub fn py_ut_index() -> PyGalleryParser {
    PyGalleryParser { inner: Box::new(construct::gallery::ut_index::UTIndex::new()) }
}
```

**注意**：`elf()` 返回 `Struct`，`pe32file()` 返回 `Struct`，
`UTIndex` 是独立 struct（`impl Construct`）。三者均可 `Box::new()` 装箱为
`Box<dyn Construct>`。需确认 Rust gallery 的公开 API 签名（DEV 阶段核对）。

### 3.3 Python 侧 gallery 子模块

在 `construct_rust/` 包下创建 `gallery/` 子包，对齐原版导入路径：

```
construct-py/construct_rust/gallery/
├── __init__.py          ← from .elf import elf; from .pe32coff import pe32file; from .ut_index import UTIndex
├── elf.py               ← from .._core import elf  (re-export PyO3 工厂)
├── pe32coff.py          ← from .._core import pe32file
└── ut_index.py          ← from .._core import UTIndex
```

**`gallery/elf.py`**：
```python
from construct_rust._core import elf  # PyO3 工厂函数
```

**`gallery/__init__.py`**：
```python
from .elf import elf
from .pe32coff import pe32file
from .ut_index import UTIndex
```

这样 `from construct_rust.gallery import elf` 和
`from construct_rust.gallery.elf import elf` 均可工作（对齐原版
`from construct.gallery.elf import elf`）。

### 3.4 Gallery 边界条件

| 场景 | 行为 |
|------|------|
| ELF 无效签名（非 `\x7fELF`） | parse 抛 `ConstError`（Rust Const 构造器） |
| PE32COFF 无效签名 | parse 抛对应错误 |
| UTIndex 超过 5 字节 | parse 读取完整 5 字节（MAX_BYTES=5），不报错 |
| Gallery parser 嵌套于 Struct | 需 `py_adapter::try_extract_registered` 支持 `PyGalleryParser` 分支 |

**嵌套支持**：若测试用例将 gallery parser 用作 Struct 字段（如
`"header" / elf`），需在 `py_adapter.rs::try_extract_registered` 中增加
`PyGalleryParser` 分支。但 `PyGalleryParser` 实现了
`impl_construct_operators!`（`/` 运算符 → Renamed），故 `"name" / elf()` 产
生 `Renamed(PyGalleryParser)`，Struct 构造器收到的已是包装对象。
`try_extract_registered` 需识别 `PyGalleryParser`（建议：因其也实现
`PyConstructWrapper`，可归入通用 `PyConstructWrapper` 提取分支，无需单独分支）。

---

## 4. lib 子模块（纯 Python 逐行移植）

### 4.1 移植原则

4 个 lib 子模块**全部纯 Python 逐行移植**，保证测试零差异：

1. **函数签名、参数名、默认值**与原版完全一致
2. **错误消息文本**与原版完全一致（测试可能断言错误消息）
3. **模块级常量和缓存**（如 `BYTES2BITS_CACHE`）保留
4. **循环依赖处理**：原版 `binary.py` 开头 `from construct import *` +
   `from construct.lib import *` 会引入循环。移植时改为**显式导入**所需符号
   （`int2byte`/`byte2int` 等），避免循环依赖。

### 4.2 lib/binary.py（169 行 → 逐行移植）

原版定义 11 个函数 + 3 个模块级缓存：

| 函数 | 签名 | 用途 |
|------|------|------|
| `integer2bits(number, width, signed=False)` | int → bytes（每 bit 一个字节） | 整数 → 位串 |
| `integer2bytes(number, width, signed=False)` | int → bytes | 整数 → 字节串（用 `int.to_bytes`） |
| `bits2integer(data, signed=False)` | bytes → int | 位串 → 整数 |
| `bytes2integer(data, signed=False)` | bytes → int | 字节串 → 整数（用 `int.from_bytes`） |
| `bytes2bits(data)` | bytes → bytes | 字节串 → 位串（查表） |
| `bits2bytes(data)` | bytes → bytes | 位串 → 字节串（查表，长度需 8 的倍数） |
| `swapbytes(data)` | bytes → bytes | 字节序反转 |
| `swapbytesinbits(data)` | bytes → bytes | 位串中按字节反转 |
| `swapbitsinbytes(data)` | bytes → bytes | 每字节内位反转（查表） |
| `hexlify(data)` | bytes → bytes | `binascii.hexlify` 封装 |
| `unhexlify(data)` | bytes → bytes | `binascii.unhexlify` 封装 |

**模块级缓存**：
- `BYTES2BITS_CACHE = {i: integer2bits(i,8) for i in range(256)}`
- `BITS2BYTES_CACHE = {bytes2bits(int2byte(i)): i for i in range(256)}`
- `SWAPBITSINBYTES_CACHE = {i: ... for i in range(256)}`

**移植要点**：
- `integer2bytes` 依赖 `int.to_bytes(number, width, 'big', signed=signed)`，Python 3 原生支持
- `bytes2bits`/`bits2bytes`/`swapbitsinbytes` 依赖 `int2byte`/`byte2int`（来自 `py3compat`）
- **导入调整**：删除 `from construct import *` 和 `from construct.lib import *`，改为
  `from construct_rust.lib.py3compat import int2byte, byte2int`（显式导入）
- 保留 `import binascii`

### 4.3 lib/py3compat.py（43 行 → 逐行移植）

| 符号 | 类型 | 说明 |
|------|------|------|
| `PY` | `sys.version_info[:2]` | Python 版本元组 |
| `PYPY` | bool | 是否 PyPy |
| `ONWINDOWS` | bool | `platform.system() == "Windows"` |
| `INT2BYTE_CACHE` | dict | `{i: bytes([i]) for i in range(256)}` |
| `int2byte(character: int)` | → bytes | int → 1 字节 bytes |
| `byte2int(character: bytes)` | → int | 1 字节 bytes → int |
| `str2bytes(string: str)` | → bytes | utf8 编码 |
| `bytes2str(string: bytes)` | → str | utf8 解码 |
| `PY2` / `PY3` | bool | 固定 `False` / `True`（Python 3） |
| `stringtypes` / `integertypes` / ... | 类型常量 | 弃用兼容 |

**移植要点**：直接逐行复制，仅需将模块开头的隐式依赖保留（`import sys`,
`import platform`）。无循环依赖。

### 4.4 lib/hex.py（94 行 → 逐行移植）

| 符号 | 类型 | 说明 |
|------|------|------|
| `HexDisplayedInteger(int)` | 类 | hex 显示的 int 子类 |
| `HexDisplayedBytes(bytes)` | 类 | hex 显示的 bytes 子类 |
| `HexDisplayedDict(dict)` | 类 | hex 显示的 dict 子类 |
| `HexDumpDisplayedBytes(bytes)` | 类 | hexdump 显示的 bytes 子类 |
| `HexDumpDisplayedDict(dict)` | 类 | hexdump 显示的 dict 子类 |
| `PRINTABLE` | list[256] | 可打印字符映射表 |
| `HEXPRINT` | list[256] | hex 格式映射表 |
| `hexdump(data, linesize)` | → str | bytes → hexdump 格式字符串 |
| `hexundump(data, linesize)` | → bytes | hexdump → bytes（逆操作） |

**移植要点**：
- `HexDisplayedInteger.new(intvalue, fmtstr)` 是静态工厂方法（设置 `obj.fmtstr` 属性）
- `PRINTABLE`/`HEXPRINT` 依赖 `bytes2str`/`int2byte`（来自 `py3compat`）
- **导入调整**：`from construct.lib.py3compat import *` → `from construct_rust.lib.py3compat import *`

### 4.5 lib/bitstream.py（147 行 → 逐行移植）

| 类 | 说明 |
|----|------|
| `RestreamedBytesIO` | 重流包装器：通过 decoder/encoder 函数在 substream 和上层之间转换数据。用于 `Restreamed` 构造器（10.8） |
| `RebufferedBytesIO` | 缓冲流包装器：支持 seek/tell，带 tailcutoff 裁剪。用于 `Rebuffered` 构造器（10.8） |

**RestreamedBytesIO 方法清单**：

| 方法 | 签名 | 行为 |
|------|------|------|
| `__init__(substream, decoder, decoderunit, encoder, encoderunit)` | — | 初始化读写缓冲区 |
| `read(count=None)` | → bytes | count=None 读全部；count<0 抛 ValueError；不足返回 b'' |
| `write(data)` | → int | 缓冲写，按 encoderunit 分块 encoder 后写 substream |
| `close()` | — | 检查残留缓冲区，非空抛 ValueError |
| `seek(at, whence=0)` | — | 仅支持 tell 后的 noop seek，否则抛 IOError |
| `seekable()` | → False | 不可 seek |
| `tell()` | → int | 返回 sincereadwritten |
| `tellable()` | → True | 可 tell |

**RebufferedBytesIO 方法清单**：

| 方法 | 签名 | 行为 |
|------|------|------|
| `__init__(substream, tailcutoff=None)` | — | 初始化 rwbuffer |
| `read(count)` | → bytes | count=None 抛 ValueError；按需从 substream 读 128KB 块 |
| `write(data)` | → int | 覆盖写 rwbuffer |
| `seek(at, whence=0)` | → int | whence 0/1 支持，2 抛 ValueError |
| `seekable()` | → True | 可 seek |
| `tell()` | → int | 返回 offset |
| `tellable()` | → True | 可 tell |
| `cachedfrom()` | → int | 返回 moved（缓冲区起始绝对偏移） |
| `cachedto()` | → int | 返回 moved + len(rwbuffer) |

**移植要点**：
- 依赖 `io.BlockingIOError`、`time.sleep`、`sys.maxsize`（Python 标准库，原样保留）
- 无 construct 内部依赖（纯流工具类）
- 逐行复制，零修改

### 4.6 lib/__init__.py 更新

取消注释 4 个 re-export 行（10.1 创建时已预留注释占位）：

```python
# construct_rust/lib/__init__.py（更新后）
from .containers import (...)
from .binary import *          # 取消注释（10.9）
from .bitstream import *       # 取消注释（10.9）
from .hex import *             # 取消注释（10.9）
from .py3compat import *       # 取消注释（10.9）
```

**__all__ 扩展**：追加 4 个子模块的全部公开符号。注意 `binary.py` 依赖
`py3compat.int2byte`，导入顺序需保证 `py3compat` 在 `binary` 之前（或
`binary.py` 内部显式 import，见 §4.2）。

**循环依赖规避**：`binary.py` 内部 `from construct_rust.lib.py3compat import
int2byte, byte2int`（显式子模块导入，不经过 `lib/__init__.py`），避免
`lib/__init__.py` 导入 `binary` 时 `py3compat` 尚未初始化的问题。

---

## 5. debug.py（stub 实现）

### 5.1 设计目标

`test_probe` 和 `test_debugger` 在总纲跳过表中已标记跳过（Probe 输出到 stdout，
Debugger 依赖 `pdb.set_trace`）。因此 debug.py 只需满足：
1. `from construct_rust import Probe, Debugger` **不报错**
2. `Probe()` / `Debugger(subcon)` 可实例化（构造器签名与原版一致）
3. 实例具备 `parse`/`build`/`sizeof` 方法（可被 Renamed 包装、嵌入 Struct）
4. 功能可简化（Probe 的 printout 可保留原样或简化，Debugger 的 pdb 调用可保留）

### 5.2 Probe（stub）

```python
# construct_rust/debug.py

from ._adapter import Construct


class Probe(Construct):
    """Probe stub. Dumps context/stream to stdout for debugging.
    Ported from construct.debug.Probe (test skipped in Phase 10)."""

    def __init__(self, into=None, lookahead=None):
        super().__init__()
        self.flagbuildnone = True
        self.into = into
        self.lookahead = lookahead

    def _parse(self, stream, context, path):
        self.printout(stream, context, path)

    def _build(self, obj, stream, context, path):
        self.printout(stream, context, path)

    def _sizeof(self, context, path):
        self.printout(None, context, path)
        return 0

    def printout(self, stream, context, path):
        # 逐行移植原版 debug.py:73-95（print 输出）
        ...
```

`printout` 方法**逐行移植**原版（输出格式需精确，因 Probe 可能在其他测试
的捕获 stdout 中出现）。`into` 参数若为 context lambda（`this.count`），通过
`expr.evaluate()` 求值（10.4 已实现）。

### 5.3 Debugger（stub）

```python
class Debugger(Subconstruct):
    """PDB-based debugger stub. Ported from construct.debug.Debugger (test skipped)."""

    def _parse(self, stream, context, path):
        try:
            return self.subcon._parsereport(stream, context, path)
        except Exception:
            self.retval = NotImplemented
            self.handle_exc(path, msg="(you can set self.retval, ...)")
            if self.retval is NotImplemented:
                raise
            return self.retval

    def _build(self, obj, stream, context, path):
        try:
            return self.subcon._build(obj, stream, context, path)
        except Exception:
            self.handle_exc(path)

    def _sizeof(self, context, path):
        try:
            return self.subcon._sizeof(context, path)
        except Exception:
            self.handle_exc(path)

    def handle_exc(self, path, msg=None):
        # 逐行移植原版（含 pdb.post_mortem 调用）
        ...
```

**注意**：`Debugger` 继承 `Subconstruct`（10.7 `_adapter.py` 已定义）。
`pdb.post_mortem` 调用保留（测试跳过，不会触发）。`import pdb, sys, traceback`
保留原版导入。

### 5.4 __init__.py 导入

```python
# construct_rust/__init__.py（扩展）
from ._core import *          # PyO3 原生扩展（已有）
from . import lib             # 已有
from . import expr            # 已有（10.4）
from ._adapter import *       # 已有（10.7）
from ._stream import *        # 已有（10.8）
from ._string import *        # 新增（10.9）：PascalString, GreedyString, StringEncoded
from .debug import *          # 新增（10.9）：Probe, Debugger
```

---

## 6. 边界条件清单

### 6.1 字符串构造器边界条件

| # | 构造器 | 场景 | 预期行为 |
|---|--------|------|---------|
| 1 | PaddedString | length=0 | parse 返回空字符串；build 写 0 字节；sizeof 返回 0 |
| 2 | PaddedString | 编码后字节数 == length | 无 padding，精确填充 |
| 3 | PaddedString | 编码后字节数 > length | build 抛 PaddingError |
| 4 | PaddedString | 全 null 输入 | parse 返回空字符串（strip 所有 null unit） |
| 5 | PaddedString | sizeof 时 length 非 unit 整数倍（如 utf16, length=3） | 抛 SizeofError |
| 6 | PaddedString | 编码名 "utf_8"（下划线别名） | 正常工作（encoding_name_to_enum 归一化） |
| 7 | PaddedString | 编码名 "unknown" | 抛 StringError |
| 8 | CString | 空字符串（首字节即 null） | parse 返回 "" |
| 9 | CString | EOF 前无 null terminator | parse 抛错误（含 CSTRING_EOF_MESSAGE） |
| 10 | CString | UTF-16BE（2 字节 terminator） | 正确检测 00 00 终止符 |
| 11 | CString | sizeof | 抛 SizeofError（变长） |
| 12 | PascalString | 空字符串 | lengthfield=0，无 payload |
| 13 | PascalString | VarInt 作 lengthfield | 兼容（Prefixed 已支持 VarInt） |
| 14 | GreedyString | 空流 | parse 返回 "" |
| 15 | GreedyString | build 非 str（如 int） | _encode 抛 StringError |
| 16 | StringEncoded | encoding 为空字符串/None | __init__ 抛 StringError |

### 6.2 Gallery 边界条件

| # | 解析器 | 场景 | 预期行为 |
|---|--------|------|---------|
| 17 | elf | 有效 32-bit LSB ELF | 正确解析 identifier + body |
| 18 | elf | 无效签名 | parse 抛 ConstError |
| 19 | pe32file | 有效 PE 文件 | 正确解析 |
| 20 | UTIndex | 正数 Index | 正确解析 |
| 21 | UTIndex | 负数 Index（sign bit 置位） | 正确解析为负整数 |

### 6.3 lib 子模块边界条件

| # | 函数 | 场景 | 预期行为 |
|---|------|------|---------|
| 22 | integer2bits | width < 1 | 抛 ValueError("width N must be positive") |
| 23 | integer2bits | signed number 超范围 | 抛 ValueError("number N is out of range") |
| 24 | integer2bytes | number 不 fit width | 抛 ValueError（含 OverflowError 捕获） |
| 25 | bits2integer | data == b"" | 抛 ValueError("bit-string cannot be empty") |
| 26 | bits2integer | signed=True, 首位为 1 | 返回负数（2's complement） |
| 27 | bytes2integer | data == b"" | 抛 ValueError("byte-string cannot be empty") |
| 28 | bits2bytes | len(data) % 8 != 0 | 抛 ValueError("data length N must be a multiple of 8") |
| 29 | swapbytesinbits | len(data) % 8 != 0 | 抛 ValueError |
| 30 | hexdump | len(data) >= 16**8 | 抛 ValueError("hexdump cannot process more than 16**8...") |
| 31 | RestreamedBytesIO.read | count < 0 | 抛 ValueError("count cannot be negative") |
| 32 | RestreamedBytesIO.close | rbuffer 非空 | 抛 ValueError（含 unread bytes 数量） |
| 33 | RestreamedBytesIO.seek | whence!=0 或 at!=tell | 抛 IOError |
| 34 | RebufferedBytesIO.read | count=None | 抛 ValueError("count must be integer") |
| 35 | RebufferedBytesIO.read | startsat < moved | 抛 IOError("tail was cut off") |
| 36 | RebufferedBytesIO.seek | whence=2 | 抛 ValueError("seeks only with whence: 0 and 1") |

---

## 7. 与 Python 版本对应

### 7.1 字符串构造器

| Python 类/函数 | Rust/Python 实现 | 实现位置 |
|---------------|-----------------|---------|
| `PaddedString(length, encoding)` | `PyPaddedString` (PyO3 包装 Rust `PaddedString`) | `constructs_string.rs` |
| `CString(encoding)` | `PyCString` (PyO3 包装 Rust `CString`) | `constructs_string.rs` |
| `PascalString(lengthfield, encoding)` | 纯 Python macro (`StringEncoded(Prefixed(...), enc)`) | `_string.py` |
| `GreedyString(encoding)` | 纯 Python macro (`StringEncoded(GreedyBytes, enc)`) | `_string.py` |
| `StringEncoded(subcon, encoding)` | 纯 Python Adapter（逐行移植 `core.py:1711`） | `_string.py` |
| `possiblestringencodings` (dict) | Rust `encoding_name_to_enum` 映射表 | `constructs_string.rs` |
| `encodingunit(encoding)` (函数) | Rust `StringEncoding::term_size()` | `constructs_string.rs`（隐式） |

### 7.2 Gallery

| Python 模块/名称 | Rust 实现 | 暴露方式 |
|-----------------|----------|---------|
| `gallery.elf.elf` | `construct::gallery::elf::elf()` | `py_elf()` → `PyGalleryParser` |
| `gallery.pe32coff.pe32file` | `construct::gallery::pe32coff::pe32file()` | `py_pe32coff()` → `PyGalleryParser` |
| `gallery.ut_index.UTIndex` | `construct::gallery::ut_index::UTIndex` | `py_ut_index()` → `PyGalleryParser` |

### 7.3 lib 子模块

| Python 模块 | 移植目标 | 公开符号数 |
|------------|---------|-----------|
| `construct.lib.binary` | `construct_rust.lib.binary` | 11 函数 + 3 缓存 |
| `construct.lib.py3compat` | `construct_rust.lib.py3compat` | 3 常量 + 1 缓存 + 5 函数 + 8 弃用常量 |
| `construct.lib.hex` | `construct_rust.lib.hex` | 5 类 + 2 常量 + 2 函数 |
| `construct.lib.bitstream` | `construct_rust.lib.bitstream` | 2 类 |

### 7.4 debug

| Python 类 | 移植目标 | 说明 |
|-----------|---------|------|
| `Probe(into, lookahead)` | `construct_rust.debug.Probe` | stub，printout 逐行移植 |
| `Debugger(subcon)` | `construct_rust.debug.Debugger` | stub，handle_exc 逐行移植 |

---

## 8. 与其他模块的交互

### 8.1 依赖（本子任务使用的已设计模块）

| 依赖模块 | 提供方 | 用途 |
|---------|--------|------|
| `construct::constructs::strings` (Rust 内核) | Phase 2 | `CString` / `PaddedString` / `StringEncoding` |
| `construct::gallery` (Rust 内核) | Phase 8 | `elf` / `pe32file` / `UTIndex` |
| `Prefixed` (10.8 PyO3 包装) | Phase 10.8 | PascalString 的 subcon 组合 |
| `GreedyBytes` (10.5 PyO3 包装) | Phase 10.5 | PascalString/GreedyString 的 subcon |
| `_adapter.Adapter` (10.7 纯 Python) | Phase 10.7 | `StringEncoded` 基类 |
| `_adapter.Subconstruct` (10.7 纯 Python) | Phase 10.7 | `Debugger` 基类 |
| `expr.evaluate` (10.4) | Phase 10.4 | Probe 的 `into` lambda 求值 |
| `exceptions.StringError` (10.2) | Phase 10.2 | 字符串编码错误 |
| `lib.containers` (10.3) | Phase 10.3 | 已有，lib/__init__ re-export |
| `PyConstructWrapper` trait (10.5) | Phase 10.5 | PyO3 包装统一接口 |
| `impl_api_methods!` / `impl_construct_operators!` 宏 (10.5) | Phase 10.5 | PyO3 包装公共 API 注入 |

### 8.2 被依赖（后续子任务使用本子任务产出）

| 被依赖方 | 用途 |
|---------|------|
| 10.10 测试套件 | `test_binary.py` / `test_hex.py` / `test_bitstream.py` / `test_py3compat.py` / `test_gallery.py` 依赖本子任务的 lib 子模块和 gallery 暴露；`test_core.py` 中字符串构造器用例依赖 PaddedString/CString/PascalString/GreedyString |
| 10.10 `declarativeunittest.py` | `common()` helper 中字符串测试用例依赖 4 个字符串构造器 |

### 8.3 py_adapter 交互（嵌套支持）

`PyPaddedString` / `PyCString` / `PyGalleryParser` 均实现 `PyConstructWrapper`
trait。当它们被嵌入 Rust `Struct`/`Sequence` 时，`py_adapter::try_extract_registered`
需能识别这些类型并提取 `Box<dyn Construct>`。

**实现方式**：由于三者均实现 `PyConstructWrapper`，建议在 `try_extract_registered`
中增加**通用 `PyConstructWrapper` 提取分支**（若 10.5-10.8 已有此机制，则无需
额外代码；若按类型逐一匹配，则需新增 3 个分支）。

DEV 阶段需检查 `py_adapter.rs` 现有实现：
- 若已有 `PyConstructWrapper` trait object 的通用提取（如通过 `__pyclass__`
  类型注册表），则无需修改
- 若按具体类型 `match`，则新增 `PyPaddedString` / `PyCString` / `PyGalleryParser`
  三个 arm

---

## 9. 验证标准

### 9.1 构建验证

```bash
# 在 construct-py/ 下
cargo build          # constructs_string.rs + gallery.rs 编译通过
cargo clippy         # 零 warning
cargo fmt --check    # 格式通过
maturin develop       # Python 扩展安装成功
```

### 9.2 导入验证

```python
import construct_rust
# 字符串构造器
construct_rust.PaddedString(10, "utf8")
construct_rust.CString("utf8")
construct_rust.PascalString(construct_rust.VarInt, "utf8")
construct_rust.GreedyString("utf8")
# Gallery
from construct_rust.gallery import elf, pe32file, UTIndex
from construct_rust.gallery.elf import elf
# lib 子模块
from construct_rust.lib import binary, hex, bitstream, py3compat
from construct_rust.lib.binary import integer2bits, bits2integer, bytes2integer
from construct_rust.lib.hex import hexdump, hexundump
from construct_rust.lib.bitstream import RestreamedBytesIO, RebufferedBytesIO
from construct_rust.lib.py3compat import int2byte, byte2int, ONWINDOWS, PY
# debug
from construct_rust import Probe, Debugger
```

### 9.3 功能验证（单元测试，DEV 阶段编写）

| 测试类别 | 最少用例数 | 覆盖范围 |
|---------|-----------|---------|
| PaddedString PyO3 | 6 | parse/build/sizeof/编码别名/超长/全null |
| CString PyO3 | 5 | parse/build/sizeof/UTF16/EOF |
| PascalString 纯 Python | 4 | VarInt+utf8/空字符串/build/嵌套 |
| GreedyString 纯 Python | 3 | utf8/空流/build非str |
| Gallery | 3 | elf/pe32/ut_index 工厂返回 + parse smoke |
| lib/binary | 11 | 每个函数 1 用例 |
| lib/hex | 2 | hexdump/hexundump 往返 |
| lib/bitstream | 2 | RestreamedBytesIO/RebufferedBytesIO 基本读写 |
| lib/py3compat | 2 | int2byte/byte2int 往返 |
| debug stub | 2 | Probe/Debugger 可实例化 + parse/build 不崩 |
| **合计** | **≥40** | |

### 9.4 原版测试套件验证（10.10 执行，本子任务预判）

| 测试文件 | 用例数 | 预期 pass | 预期 skip | 本子任务负责模块 |
|---------|--------|----------|----------|-----------------|
| `test_binary.py` | 10 | 10 | 0 | lib/binary.py |
| `test_hex.py` | 2 | 2 | 0 | lib/hex.py |
| `test_bitstream.py` | 2 | 2 | 0 | lib/bitstream.py |
| `test_py3compat.py` | 2 | 2 | 0 | lib/py3compat.py |
| `test_gallery.py` | 2 | 2 | 0 | gallery 暴露 |
| `test_core.py` 中字符串用例 | ~15 | ~15 | 0 | 4 个字符串构造器 |

---

## 10. 需 PM 裁定的问题

### 问题 1：PaddedString 的 context lambda length 支持

**现状**：Rust 内核 `PaddedString.length: usize` 是固定值，不支持 context
lambda（如 `PaddedString(this.count, "utf8")`）。PyO3 包装 `py_padded_string`
的 `length` 参数仅接受 int。

**影响**：若 `test_core.py` 中有用例使用 `PaddedString(this.count, ...)`，
该用例会失败。

**预判**：基于原版 docstring 示例（均使用整数字面量），预计无影响。但需
REF 在 10.10 验证阶段确认。

**备选方案**（若确实有 lambda length 用例）：
- 方案 A（推荐）：`py_padded_string` 检测 length 是否 callable，若是则
  fallback 到纯 Python macro
  `StringEncoded(FixedSized(length, NullStripped(GreedyBytes, ...)), encoding)`
  （需 FixedSized/NullStripped 已暴露——10.8 已暴露 FixedSized，NullStripped
  在 `_stream.py` 已实现）
- 方案 B：标记为已知限制，该用例 skip

**请 PM 裁定**：是否现在就实现 fallback（方案 A），还是等 REF 验证后再定？
ARCH 建议先按方案 A 实现（检测 callable → fallback），成本极低且消除风险。

### 问题 2：PyGalleryParser 包装类命名

**现状**：新增的 `PyGalleryParser` 是一个通用 `Box<dyn Construct>` 包装。

**问题**：`type(elf()).__name__` 将返回 `"GalleryParser"`，而非原版的
`"Struct"`。若测试用例断言类型名（`assert isinstance(elf(), Struct)`），
会失败。

**预判**：`test_gallery.py`（2 用例）预计只测试 parse/build 行为，不断言类型。

**备选方案**：若 gallery parser 需暴露为 `Struct` 类型，可直接复用 10.6 的
`PyStruct` 包装（`elf()` 返回 `Struct`，包装为 `PyStruct`）。但这要求
`pe32file()` 和 `UTIndex` 也兼容 `PyStruct`（UTIndex 不是 Struct）。

**请 PM 裁定**：`PyGalleryParser` 通用包装是否可接受？还是要求按返回类型
分别包装（elf/pe32→PyStruct，UTIndex→PyGalleryParser）？
ARCH 建议通用包装（简单且测试不断言类型）。

---

## 11. 实现顺序建议（供 DEV 参考）

1. **lib 子模块**（纯 Python，无编译依赖，可先完成）
   - `py3compat.py`（无依赖）→ `binary.py`（依赖 py3compat）→ `hex.py`
     （依赖 py3compat）→ `bitstream.py`（无依赖）
   - 更新 `lib/__init__.py` 取消注释
2. **debug.py**（纯 Python stub，依赖 `_adapter`）
3. **_string.py**（纯 Python，依赖 `Prefixed`/`GreedyBytes`/`_adapter.Adapter`）
4. **constructs_string.rs**（PyO3，依赖 Rust 内核 strings）
5. **gallery.rs**（PyO3，依赖 Rust 内核 gallery）
6. **lib.rs 注册** + **py_adapter.rs 扩展** + **__init__.py 导入**
7. **单元测试**（≥40 用例）
8. `cargo build` + `clippy` + `fmt --check` + `maturin develop` 全绿

---

## 12. 文件清单汇总

### 新增文件（8 个）

| # | 文件 | 行数估算 | 语言 |
|---|------|---------|------|
| 1 | `construct-py/src/constructs_string.rs` | ~120 | Rust |
| 2 | `construct-py/src/gallery.rs` | ~60 | Rust |
| 3 | `construct-py/construct_rust/_string.py` | ~70 | Python |
| 4 | `construct-py/construct_rust/lib/binary.py` | ~170 | Python |
| 5 | `construct-py/construct_rust/lib/py3compat.py` | ~45 | Python |
| 6 | `construct-py/construct_rust/lib/hex.py` | ~95 | Python |
| 7 | `construct-py/construct_rust/lib/bitstream.py` | ~150 | Python |
| 8 | `construct-py/construct_rust/debug.py` | ~110 | Python |
| 9 | `construct-py/construct_rust/gallery/__init__.py` | ~5 | Python |
| 10 | `construct-py/construct_rust/gallery/elf.py` | ~3 | Python |
| 11 | `construct-py/construct_rust/gallery/pe32coff.py` | ~3 | Python |
| 12 | `construct-py/construct_rust/gallery/ut_index.py` | ~3 | Python |

### 扩展文件（4 个）

| # | 文件 | 扩展内容 |
|---|------|---------|
| 1 | `construct-py/src/lib.rs` | +2 模块声明 + 5 pyfunction 注册 |
| 2 | `construct-py/src/py_adapter.rs` | +3 类型分支（或确认通用分支已覆盖） |
| 3 | `construct-py/construct_rust/__init__.py` | +2 import 行（_string, debug） |
| 4 | `construct-py/construct_rust/lib/__init__.py` | 取消 4 行注释 + __all__ 扩展 |

**总行数估算**：~850 行新增 + ~30 行扩展 ≈ 880 行。
