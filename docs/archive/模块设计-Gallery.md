# 模块设计：Gallery 格式解析器（Phase 8）

> **文档性质**：前瞻性设计文档（Phase 8 开始前编写）。
> **Python 原版**：`construct/gallery/elf.py`、`construct/gallery/pe32coff.py`、`construct/gallery/ut_index.py`
> **前置依赖**：Phase 1-7 全部完成

---

## 1. 概述

### 1.1 阶段目标

Phase 8 的目标是使用已实现的所有构造器，将 Python gallery 中的真实二进制格式解析器移植到 Rust，验证整个库的实用性、正确性和 API 完备性。

### 1.2 模块位置

```
construct-rs/src/gallery/
├── mod.rs              ← 模块入口 + 公共 re-export
├── elf.rs              ← ELF 格式解析器（对应 elf.py）
├── pe32coff.rs         ← PE/COFF 格式解析器（对应 pe32coff.py）
└── ut_index.rs         ← UTIndex 自定义构造器（对应 ut_index.py）
```

### 1.3 功能缺口概要

通过精读 Python gallery 源码，发现以下构造器/功能在 Phase 1-7 中尚未实现，需要在本阶段补全：

| # | 缺口 | 严重程度 | 影响范围 | 方案 |
|---|------|---------|---------|------|
| 1 | 表达式参数化的 Pointer/Array/Seek | **致命** | ELF, PE | 新增 `PointerExpr`/`ArrayExpr`/`SeekExpr` 类型 |
| 2 | CString（空终止字符串） | **致命** | ELF | 新增 `CString` 构造器 |
| 3 | PaddedString（定长填充字符串） | **致命** | PE | 新增 `PaddedString` 构造器 |
| 4 | If(cond, subcon) 简写 | 中等 | ELF, PE | 新增 `If` 辅助函数 |
| 5 | Padding(n) | 中等 | ELF | 新增 `Padding` 辅助函数 |
| 6 | Timestamp 构造器包装 | 低 | PE | 利用已有 `TimestampAdapter` + 辅助函数 |
| 7 | UTIndex 自定义变长整数 | 中等 | ut_index | 直接实现 `Construct` trait |

### 1.4 设计约束

1. **不破坏 Phase 1-7 的 pub API**：所有新构造器以新类型/新函数添加，不修改已有 struct 的字段类型
2. **gallery 代码隔离**：格式定义放在 `construct-rs/src/gallery/` 目录，不污染 `constructs/`
3. **利用 Phase 5 的 Evaluate trait**：表达式参数化的构造器使用 `Box<dyn Evaluate>` 而非裸闭包
4. **gallery 构造器复用核心库**：gallery 中的格式定义完全使用 `constructs/` 中已有的构造器组装，仅在 UTIndex 这种特殊情况直接实现 `Construct` trait

---

## 2. 功能缺口补全

### 2.1 缺口 1：表达式参数化的 Pointer / Array / Seek（最关键）

#### 问题分析

Python gallery 中大量使用表达式参数化：

```python
# elf.py line 358
"program_table" / Pointer(this.ph_offset, p_header[this.ph_count])
# 等价于 Pointer(this.ph_offset, Array(this.ph_count, p_header))

# pe32coff.py line 241
Seek(this.msdosheader.lfanew)

# pe32coff.py line 160
"datadirectories" / Array(this.datadirectories_count, datadirectory)
```

当前 Rust 实现：
- `Pointer { offset: i64, subcon: Box<dyn Construct> }` — 仅固定 `i64` 偏移
- `Array { count: usize, subcon: Box<dyn Construct> }` — 仅固定 `usize` 计数
- `Seek { at: i64, whence: SeekWhence }` — 仅固定 `i64` 位置

#### 设计方案：新增 `PointerExpr` / `ArrayExpr` / `SeekExpr` 类型

**不修改已有类型**，新增带表达式参数的类型。理由：
1. 不破坏 Phase 1-7 的 pub API（已有代码使用 `Pointer::new(8, ...)` 固定偏移）
2. 表达式类型需要存储 `Box<dyn Evaluate>`，与固定值类型不同
3. 两种类型在运行时行为一致，仅参数来源不同

#### 2.1.1 PointerExpr

**文件位置**：`construct-rs/src/constructs/stream_ops.rs`（扩展，非 gallery）

```rust
/// 表达式偏移的 Pointer。
///
/// 与 [`Pointer`](crate::constructs::stream_ops::Pointer) 行为一致，
/// 但 offset 由表达式在运行时求值得到。
///
/// - **parse**: 求值 `offset_expr` 得到偏移量，seek 到该位置，解析 subcon，seek 回原位
/// - **build**: 求值 `offset_expr`，seek 到该位置，build subcon，seek 回原位
/// - **sizeof**: 返回 0
///
/// 对应 Python `Pointer(this.xxx, subcon)`。
///
/// # 示例
///
/// ```ignore
/// use construct::constructs::stream_ops::PointerExpr;
/// use construct::expr::this_;
/// use construct::constructs::bytes::Bytes;
///
/// let d = PointerExpr::new(
///     this_().field("offset"),
///     Box::new(Bytes::new(2)),
/// );
/// ```
pub struct PointerExpr {
    /// 求值为偏移量的表达式（返回 Value::Int 或 Value::UInt）。
    pub offset_expr: Box<dyn Evaluate>,
    /// 在目标位置解析/构建的内层构造器。
    pub subcon: Box<dyn Construct>,
}

impl PointerExpr {
    /// 创建一个表达式偏移的 Pointer。
    pub fn new(offset_expr: Box<dyn Evaluate>, subcon: Box<dyn Construct>) -> Self;

    /// 设置为相对偏移模式（相对于当前流位置）。
    pub fn with_relative(mut self, relative: bool) -> Self;
}

impl Construct for PointerExpr {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let offset = self.offset_expr.evaluate(ctx, None)?.to_i64()?;
        // 复用与 Pointer 相同的 seek 逻辑
        let fallback = stream.tell()?;
        if offset >= 0 {
            stream.seek(offset as u64)?;
        } else {
            stream.seek_from(SeekFrom::End(offset))?;
        }
        let result = self.subcon.parse(stream, ctx);
        stream.seek(fallback)?;
        result
    }
    // build / sizeof 类似
}
```

**关键边界条件**：
- 表达式求值失败 → 返回 `ConstructError::Expr`
- 求值结果为非整数 → 返回 `ConstructError::TypeMismatch`
- 负偏移 → 从流末尾计算（与固定 Pointer 一致）
- 偏移超出流范围 → Stream seek 返回错误

#### 2.1.2 ArrayExpr

**文件位置**：`construct-rs/src/constructs/repetition.rs`（扩展）

```rust
/// 表达式计数的 Array。
///
/// 与 [`Array`](crate::constructs::repetition::Array) 行为一致，
/// 但 count 由表达式在运行时求值得到。
///
/// 对应 Python `Array(this.xxx, subcon)`。
///
/// # 示例
///
/// ```ignore
/// use construct::constructs::repetition::ArrayExpr;
/// use construct::expr::this_;
/// use construct::constructs::format_field::INT8UB;
///
/// let d = ArrayExpr::new(
///     this_().field("count"),
///     Box::new(INT8UB),
/// );
/// ```
pub struct ArrayExpr {
    /// 求值为元素数量的表达式（返回 Value::UInt）。
    pub count_expr: Box<dyn Evaluate>,
    /// 应用于每个元素的子构造器。
    pub subcon: Box<dyn Construct>,
}

impl ArrayExpr {
    pub fn new(count_expr: Box<dyn Evaluate>, subcon: Box<dyn Construct>) -> Self;
}

impl Construct for ArrayExpr {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let count = self.count_expr.evaluate(ctx, None)?.to_u64()? as usize;
        // 复用 Array 的循环逻辑
        let mut list = Vec::with_capacity(count);
        for i in 0..count {
            ctx.insert("_index", Value::UInt(i as u64));
            let element = self.subcon.parse(stream, ctx)?;
            list.push(element);
        }
        Ok(Value::List(list))
    }
    // build / sizeof 类似
}
```

**关键边界条件**：
- count 为 0 → 返回空 List
- count 为负数（求值为 i64 负值）→ 返回 `ConstructError::Expr`
- build 时 list 长度与 count 不符 → 返回 `ConstructError::Array`

#### 2.1.3 SeekExpr

**文件位置**：`construct-rs/src/constructs/meta.rs`（扩展）

```rust
/// 表达式偏移的 Seek。
///
/// 对应 Python `Seek(this.xxx)`。
pub struct SeekExpr {
    /// 求值为目标位置的表达式。
    pub at_expr: Box<dyn Evaluate>,
    /// 起始位置。
    pub whence: SeekWhence,
}

impl SeekExpr {
    pub fn new(at_expr: Box<dyn Evaluate>) -> Self;
    pub fn with_whence(at_expr: Box<dyn Evaluate>, whence: SeekWhence) -> Self;
}
```

---

### 2.2 缺口 2：CString — 空终止字符串

#### Python 行为

```python
# CString("utf8") = StringEncoded(NullTerminated(GreedyBytes, term=b"\x00"), "utf8")
# parse: 读取直到 \x00（含），解码为 String
# build: 编码为 bytes，追加 \x00 终止符
# sizeof: 未定义（变长）
```

#### 设计方案

**文件位置**：`construct-rs/src/constructs/strings.rs`（新增模块）

```rust
/// 字符串编码类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StringEncoding {
    /// ASCII 编码（1 字节/字符）。
    Ascii,
    /// UTF-8 编码（1-4 字节/字符）。
    Utf8,
    /// UTF-16 编码（2 字节/字符，主机字节序）。
    Utf16,
    /// UTF-16 大端编码。
    Utf16Be,
    /// UTF-16 小端编码。
    Utf16Le,
    /// UTF-32 编码（4 字节/字符，主机字节序）。
    Utf32,
    /// UTF-32 大端编码。
    Utf32Be,
    /// UTF-32 小端编码。
    Utf32Le,
}

impl StringEncoding {
    /// 终止符的字节长度（UTF-8/ASCII=1, UTF-16=2, UTF-32=4）。
    pub fn term_size(&self) -> usize;

    /// 将字节切片解码为 Rust String。
    pub fn decode(&self, data: &[u8]) -> Result<String>;

    /// 将 Rust String 编码为字节 Vec。
    pub fn encode(&self, s: &str) -> Result<Vec<u8>>;
}

/// 空终止字符串。
///
/// - **parse**: 逐字节（或按编码单元）读取直到遇到终止符，解码为 `Value::String`
/// - **build**: 将 `Value::String` 编码为字节，追加终止符
/// - **sizeof**: 未定义（变长）
///
/// 对应 Python `CString(encoding)`。
///
/// # 示例
///
/// ```ignore
/// use construct::constructs::strings::{CString, StringEncoding};
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let d = CString::new(StringEncoding::Utf8);
/// let c: &dyn Construct = &d;
/// let parsed = c.parse_bytes(b"hello\x00world").unwrap();
/// assert_eq!(parsed, Value::String("hello".to_string()));
/// ```
pub struct CString {
    /// 字符串编码。
    pub encoding: StringEncoding,
}

impl CString {
    /// 创建一个使用指定编码的空终止字符串构造器。
    pub fn new(encoding: StringEncoding) -> Self;

    /// UTF-8 编码的便捷构造。
    pub fn utf8() -> Self;

    /// ASCII 编码的便捷构造。
    pub fn ascii() -> Self;
}

impl Construct for CString {
    fn parse(&self, stream: &mut dyn Stream, _ctx: &mut Context) -> Result<Value> {
        let term_size = self.encoding.term_size();
        let mut data = Vec::new();
        loop {
            let chunk = stream.read_bytes(term_size)?;
            if chunk.len() < term_size {
                // EOF 未遇到终止符
                return Err(ConstructError::Stream { /* EOF */ });
            }
            // 检查是否为终止符
            if chunk.iter().all(|&b| b == 0) {
                break;
            }
            data.extend_from_slice(&chunk);
        }
        let s = self.encoding.decode(&data)?;
        Ok(Value::String(s))
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, _ctx: &mut Context) -> Result<()> {
        let s = data.as_string()?;
        let encoded = self.encoding.encode(&s)?;
        stream.write_bytes(&encoded)?;
        // 写入终止符
        let term = vec![0u8; self.encoding.term_size()];
        stream.write_bytes(&term)
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Err(ConstructError::Sizeof {
            path: String::new(),
            reason: "CString has variable size".to_string(),
        })
    }
}
```

**关键边界条件**：
- 空字符串 `""` → parse 时立即遇到 \x00，返回 `Value::String("")`；build 时仅写 \x00
- EOF 未遇到终止符 → 返回 `ConstructError::Stream`（EOF）
- UTF-16/UTF-32 终止符为 2/4 个零字节
- build 非 String 类型 → 返回 `ConstructError::TypeMismatch`

---

### 2.3 缺口 3：PaddedString — 定长填充字符串

#### Python 行为

```python
# PaddedString(10, "utf8")
#   = StringEncoded(FixedSized(10, NullStripped(GreedyBytes, pad=b"\x00")), "utf8")
# parse: 读取 length 字节，从右侧去除填充零字节，解码
# build: 编码为字节，右侧填充零字节到 length，超过则 PaddingError
# sizeof: length
```

#### 设计方案

**文件位置**：`construct-rs/src/constructs/strings.rs`（与 CString 同模块）

```rust
/// 定长填充字符串。
///
/// - **parse**: 读取 `length` 字节，从右侧去除填充零字节，解码为 `Value::String`
/// - **build**: 编码 `Value::String` 为字节，右侧填充零字节到 `length`。
///   若编码后超过 `length`，返回 `ConstructError::Padding`
/// - **sizeof**: 返回 `length`
///
/// 对应 Python `PaddedString(length, encoding)`。
///
/// # 示例
///
/// ```ignore
/// use construct::constructs::strings::{PaddedString, StringEncoding};
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let d = PaddedString::new(10, StringEncoding::Utf8);
/// let c: &dyn Construct = &d;
/// let built = c.build_bytes(&Value::String("hello".to_string())).unwrap();
/// assert_eq!(built, b"hello\0\0\0\0\0");
/// ```
pub struct PaddedString {
    /// 固定长度（字节数）。
    pub length: usize,
    /// 字符串编码。
    pub encoding: StringEncoding,
}

impl PaddedString {
    pub fn new(length: usize, encoding: StringEncoding) -> Self;
    pub fn utf8(length: usize) -> Self;
    pub fn ascii(length: usize) -> Self;
}

impl Construct for PaddedString {
    fn parse(&self, stream: &mut dyn Stream, _ctx: &mut Context) -> Result<Value> {
        let data = stream.read_bytes(self.length)?;
        // 从右侧去除填充零字节（按编码单元对齐）
        let unit = self.encoding.term_size();
        let mut end = data.len();
        while end >= unit && data[end - unit..end].iter().all(|&b| b == 0) {
            end -= unit;
        }
        let stripped = &data[..end];
        let s = self.encoding.decode(stripped)?;
        Ok(Value::String(s))
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, _ctx: &mut Context) -> Result<()> {
        let s = data.as_string()?;
        let encoded = self.encoding.encode(&s)?;
        if encoded.len() > self.length {
            return Err(ConstructError::Padding {
                path: String::new(),
                message: format!(
                    "encoded string is {} bytes but PaddedString length is {}",
                    encoded.len(), self.length
                ),
            });
        }
        stream.write_bytes(&encoded)?;
        let pad = self.length - encoded.len();
        if pad > 0 {
            stream.write_bytes(&vec![0u8; pad])?;
        }
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Ok(self.length)
    }
}
```

**关键边界条件**：
- 编码后长度恰好等于 length → 无填充
- 编码后长度超过 length → `ConstructError::Padding`
- 空字符串 → 全部为填充零字节
- UTF-16/UTF-32 的填充需按 2/4 字节单元对齐

---

### 2.4 缺口 4：If(cond, subcon) — IfThenElse 的简写

#### Python 行为

```python
# If(condfunc, subcon) = IfThenElse(condfunc, subcon, Pass)
```

#### 设计方案

**文件位置**：`construct-rs/src/constructs/control_flow.rs`（扩展）

不新增 struct，仅添加辅助构造函数：

```rust
/// 创建一个条件构造器：当 `cond` 为 true 时执行 `then_constr`，否则跳过。
///
/// 等价于 `IfThenElse::new(cond, then_constr, Box::new(Pass::new()))`。
///
/// 对应 Python `If(condfunc, subcon)`。
///
/// # 示例
///
/// ```ignore
/// use construct::constructs::control_flow::If;
/// use construct::constructs::format_field::INT8UB;
/// use construct::expr::this_;
///
/// let d = If::new(
///     Box::new(|ctx| ctx.get("flag").map(|v| v.as_bool().unwrap_or(false)).unwrap_or(false)),
///     Box::new(INT8UB),
/// );
/// ```
pub fn If(cond: CondFunc, then_constr: Box<dyn Construct>) -> IfThenElse {
    IfThenElse::new(cond, then_constr, Box::new(Pass::new()))
}
```

**关键边界条件**：
- cond 为 false → parse 返回 `Value::None`，build 不写入字节
- flagbuildnone 为 true（因为 else 分支是 Pass）

---

### 2.5 缺口 5：Padding(n) — 读取并丢弃 n 字节

#### Python 行为

```python
# Padding(length, pattern=b"\x00") = Padded(length, Pass, pattern=pattern)
# parse: 读取 length 字节并丢弃
# build: 写入 length 个 pattern 字节
# sizeof: length
```

#### 设计方案

**文件位置**：`construct-rs/src/constructs/computed.rs`（与 Padded 同模块）

不新增 struct，仅添加辅助构造函数：

```rust
/// 创建一个纯填充构造器：读取/写入指定长度的填充字节。
///
/// 等价于 `Padded::new(length, Box::new(Pass::new()), pattern, false)`。
///
/// 对应 Python `Padding(length, pattern)`。
///
/// # 示例
///
/// ```ignore
/// use construct::constructs::computed::Padding;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let d = Padding::new(4);
/// let c: &dyn Construct = &d;
/// let built = c.build_bytes(&Value::None).unwrap();
/// assert_eq!(built, vec![0, 0, 0, 0]);
/// ```
pub fn Padding(length: usize) -> Padded {
    Padded::new(length, Box::new(Pass::new()), 0x00, false)
}

/// 创建一个带自定义填充模式的 Padding。
pub fn Padding_with_pattern(length: usize, pattern: u8) -> Padded {
    Padded::new(length, Box::new(Pass::new()), pattern, false)
}
```

> **命名注意**：Rust 不支持函数重载，因此带 pattern 的版本使用 `_with_pattern` 后缀。
> `Padding` 作为函数名与 Python 保持一致。

---

### 2.6 缺口 6：Timestamp — 时间戳构造器

#### 现状

Phase 6 已实现 `TimestampAdapter`，其签名为：

```rust
pub struct TimestampAdapter {
    pub subcon: Box<dyn Construct>,
    pub unit: TimestampUnit,
    pub epoch: TimestampEpoch,
}
```

PE/COFF 中使用方式：`Timestamp(Int32ul, 1., 1970)` 对应：
- subcon = Int32ul
- unit = 1.0 秒 → `TimestampUnit::Seconds`
- epoch = 1970 → `TimestampEpoch::Unix`（1970 即 Unix epoch）

#### 设计方案

无需新增类型。gallery 中直接使用 `TimestampAdapter::new`：

```rust
// PE/COFF 中的 Timestamp(Int32ul, 1., 1970)
TimestampAdapter::new(
    Box::new(INT32UL),
    TimestampUnit::Seconds,
    TimestampEpoch::Unix,
)
```

---

### 2.7 缺口 7：ut_index — 自定义变长整数

详见 [第 5 节](#5-ut_index-自定义构造器设计)。

---

## 3. ELF 格式解析器设计

### 3.1 模块位置

`construct-rs/src/gallery/elf.rs`

### 3.2 Python 源码结构分析

elf.py 定义了以下函数，返回 Struct 构造器：

| 函数 | 参数 | 返回 | 作用 |
|------|------|------|------|
| `(identifier)` | 无 | Struct | ELF 标识头（signature + class + encoding + version + osabi + padding） |
| `program_header(ELFInt32, ELFInt64, is64bit)` | 整数类型, 是否64位 | Struct | 程序头表项 |
| `section_header(ELFInt32, ELFInt64, is64bit)` | 整数类型, 是否64位 | Struct | 段头表项 |
| `body(ELFInt16, ELFInt32, ELFInt64, is64bit)` | 所有整数类型, 是否64位 | Struct | ELF 主体（不含标识头） |
| `elf` | 无 | Struct | 完整 ELF 文件（identifier + body） |

### 3.3 Rust 设计

```rust
//! ELF 格式解析器。
//!
//! 移植自 `construct/gallery/elf.py`。
//! 支持解析 32/64 位、大/小端 ELF 文件。

use crate::constructs::const_::Const;
use crate::constructs::control_flow::{If, IfThenElse};
use crate::constructs::enum_::{Enum, FlagsEnum};
use crate::constructs::format_field::*;
use crate::constructs::meta::Pass;
use crate::constructs::repetition::{Array, ArrayExpr};
use crate::constructs::stream_ops::{Pointer, PointerExpr};
use crate::constructs::struct_::{Struct, StructField};
use crate::constructs::bytes::Bytes;
use crate::constructs::computed::Padding;
use crate::constructs::strings::CString;
use crate::core::Construct;
use crate::expr::{this_, Evaluate};
use crate::value::Value;

/// ELF 标识头。
///
/// 对应 Python `identifier` Struct。
pub fn identifier() -> Struct {
    Struct::new()
        .field("signature", Box::new(Const::new_bytes(b"\x7fELF".to_vec())))
        .field("elfclass", Box::new(Enum::new_byte(
            Box::new(FormatField::new(Endianness::Big, FormatKind::U8)),
            vec![
                ("ELFCLASSNONE", Value::UInt(0)),
                ("ELFCLASS32", Value::UInt(1)),
                ("ELFCLASS64", Value::UInt(2)),
            ],
        )))
        // ... encoding, version, osabi, abiversion
        .field_anonymous(Box::new(Padding::new(7)))
}

/// 程序头表项。
///
/// 对应 Python `program_header(ELFInt32, ELFInt64, is64bit)`。
///
/// # 参数
/// - `int32`: 32 位整数构造器（如 Int32ul 或 Int32ub）
/// - `int64`: 64 位整数构造器
/// - `is64bit`: 是否为 64 位
pub fn program_header(
    int32: Box<dyn Construct>,
    int64: Box<dyn Construct>,
    is64bit: bool,
) -> Struct {
    let addr: Box<dyn Construct> = if is64bit { int64 } else { int32 };
    Struct::new()
        .field("p_type", Box::new(Enum::new(int32.clone(), /* ... */)))
        .field("flags_64", Box::new(If::new(
            Box::new(move |_ctx| is64bit),
            Box::new(Enum::new(int32.clone(), /* flags ... */)),
        )))
        .field("offset", addr.clone())
        .field("virtual_address", addr.clone())
        .field("physical_address", addr.clone())
        .field("size_file", addr.clone())
        .field("size_mem", addr.clone())
        .field("flags_32", Box::new(If::new(
            Box::new(move |_ctx| !is64bit),
            Box::new(Enum::new(int32, /* flags ... */)),
        )))
        .field("alignment", addr)
}

/// 段头表项。
///
/// 对应 Python `section_header(ELFInt32, ELFInt64, is64bit)`。
pub fn section_header(
    int32: Box<dyn Construct>,
    int64: Box<dyn Construct>,
    is64bit: bool,
) -> Struct {
    let addr: Box<dyn Construct> = if is64bit { int64 } else { int32 };
    Struct::new()
        .field("sh_name_offset", int32.clone())
        // sh_name: Pointer(this._.strtab_data_offset + this.sh_name_offset, CString("utf-8"))
        // 需要表达式加法 + 父上下文访问
        .field("sh_name", Box::new(PointerExpr::new(
            // this._.strtab_data_offset + this.sh_name_offset
            Box::new(
                this_().field("_").field("strtab_data_offset")
                    .add(this_().field("sh_name_offset"))
            ),
            Box::new(CString::utf8()),
        )))
        .field("sh_type", Box::new(Enum::new(int32.clone(), /* ... */)))
        .field("sh_flags", Box::new(Enum::new(addr.clone(), /* ... */)))
        .field("sh_addr", addr.clone())
        .field("sh_offset", addr.clone())
        .field("sh_size", addr.clone())
        .field("sh_link", int32.clone())
        .field("sh_info", int32)
        .field("sh_addralign", addr.clone())
        .field("sh_entsize", addr)
}

/// ELF 主体（不含标识头）。
///
/// 对应 Python `body(ELFInt16, ELFInt32, ELFInt64, is64bit)`。
pub fn body(
    int16: Box<dyn Construct>,
    int32: Box<dyn Construct>,
    int64: Box<dyn Construct>,
    is64bit: bool,
) -> Struct {
    let ph = program_header(int32.clone(), int64.clone(), is64bit);
    let sh = section_header(int32.clone(), int64.clone(), is64bit);
    let addr: Box<dyn Construct> = if is64bit { int64.clone() } else { int32.clone() };

    Struct::new()
        .field("type", Box::new(Enum::new(int16.clone(), /* ET_* ... */)))
        .field("machine", Box::new(Enum::new(int16.clone(), /* EM_* ... */)))
        .field("version", Box::new(Enum::new(int32.clone(), /* EV_* ... */)))
        .field("entry", addr.clone())
        .field("ph_offset", addr.clone())
        .field("sh_offset", addr.clone())
        .field("flags", int32.clone())
        .field("header_size", int16.clone())
        .field("ph_entry_size", int16.clone())
        .field("ph_count", int16.clone())
        .field("sh_entry_size", int16.clone())
        .field("sh_count", int16.clone())
        .field("strtab_section_index", int16)
        // strtab_data_offset: Pointer(计算偏移, ELFInt32)
        .field("strtab_data_offset", Box::new(PointerExpr::new(
            // this.sh_offset + this.strtab_section_index * this.sh_entry_size + (24 if is64bit else 16)
            Box::new(
                this_().field("sh_offset")
                    .add(
                        this_().field("strtab_section_index")
                            .mul(this_().field("sh_entry_size"))
                    )
                    .add(ConstExpr::new(Value::Int(if is64bit { 24 } else { 16 })))
            ),
            int32.clone(),
        )))
        // program_table: Pointer(this.ph_offset, Array(this.ph_count, p_header))
        .field("program_table", Box::new(PointerExpr::new(
            Box::new(this_().field("ph_offset")),
            Box::new(ArrayExpr::new(
                Box::new(this_().field("ph_count")),
                Box::new(ph),
            )),
        )))
        // sections: Pointer(this.sh_offset, Array(this.sh_count, s_header))
        .field("sections", Box::new(PointerExpr::new(
            Box::new(this_().field("sh_offset")),
            Box::new(ArrayExpr::new(
                Box::new(this_().field("sh_count")),
                Box::new(sh),
            )),
        )))
}

/// 完整的 ELF 文件构造器。
///
/// 对应 Python `elf` Struct。
///
/// 根据 identifier 中的 encoding 和 elfclass 字段自动选择
/// 字节序和位数。
pub fn elf() -> Box<dyn Construct> {
    // 顶层需要嵌套 IfThenElse：
    // IfThenElse(this.identifier.encoding == "LSB",
    //   IfThenElse(this.identifier.elfclass == "ELFCLASS64",
    //     body(Int16ul, Int32ul, Int64ul, true),
    //     body(Int16ul, Int32ul, Int64ul, false)),
    //   IfThenElse(this.identifier.elfclass == "ELFCLASS64",
    //     body(Int16ub, Int32ub, Int64ub, true),
    //     body(Int16ub, Int32ub, Int64ub, false)))

    // 注意：由于 is64bit 在编译时固定，而实际需要根据解析结果动态选择，
    // 需要预构建 4 种 body 变体，用 IfThenElse 在运行时选择。
    Box::new(Struct::new()
        .field("identifier", Box::new(identifier()))
        .field("body", Box::new(
            IfThenElse::new(
                // this.identifier.encoding == "LSB"
                Box::new(|ctx| {
                    // 从上下文中获取 encoding 字段并比较
                    check_enum_field(ctx, &["identifier", "encoding"], "LSB")
                }),
                // LSB 分支
                Box::new(IfThenElse::new(
                    Box::new(|ctx| {
                        check_enum_field(ctx, &["identifier", "elfclass"], "ELFCLASS64")
                    }),
                    Box::new(body(
                        Box::new(INT16UL), Box::new(INT32UL), Box::new(INT64UL), true
                    )),
                    Box::new(body(
                        Box::new(INT16UL), Box::new(INT32UL), Box::new(INT64UL), false
                    )),
                )),
                // MSB 分支
                Box::new(IfThenElse::new(
                    Box::new(|ctx| {
                        check_enum_field(ctx, &["identifier", "elfclass"], "ELFCLASS64")
                    }),
                    Box::new(body(
                        Box::new(INT16UB), Box::new(INT32UB), Box::new(INT64UB), true
                    )),
                    Box::new(body(
                        Box::new(INT16UB), Box::new(INT32UB), Box::new(INT64UB), false
                    )),
                )),
            )
        ))
    )
}

/// 辅助函数：从嵌套上下文中读取 Enum 字段值并比较。
fn check_enum_field(ctx: &Context, path: &[&str], expected: &str) -> bool {
    ctx.get_path(path)
        .and_then(|v| v.as_string().ok())
        .map(|s| s == expected)
        .unwrap_or(false)
}
```

### 3.4 与 Python 版本对应

| Python | Rust | 备注 |
|--------|------|------|
| `Struct("name" / subcon)` | `Struct::new().field("name", Box::new(subcon))` | 构建器模式 |
| `Enum(Byte, NAME=value, ...)` | `Enum::new_byte(Box::new(FormatField::U8), vec![...])` | 需要确认 Enum API |
| `IfThenElse(is64bit, A, B)` | `IfThenElse::new(Box::new(\|_\| is64bit), A, B)` | 编译期 bool → 运行时 CondFunc |
| `If(is64bit, subcon)` | `If::new(Box::new(\|_\| is64bit), subcon)` | 新增辅助函数 |
| `Pointer(this.xxx, subcon)` | `PointerExpr::new(Box::new(this_().field("xxx")), subcon)` | 新增类型 |
| `subcon[this.count]` (Array语法糖) | `ArrayExpr::new(Box::new(this_().field("count")), subcon)` | 新增类型 |
| `Padding(7)` | `Padding::new(7)` | 新增辅助函数 |
| `CString("utf-8")` | `CString::utf8()` | 新增构造器 |
| `Const(b"\x7fELF")` | `Const::new_bytes(b"\x7fELF".to_vec())` | 已有 |
| `this._.strtab_data_offset` | `this_().field("_").field("strtab_data_offset")` | `_` 导航到父上下文 |

### 3.5 边界条件清单

- **32 位 vs 64 位**：通过 `IfThenElse` 在运行时根据 `elfclass` 选择
- **大端 vs 小端**：通过 `IfThenElse` 在运行时根据 `encoding` 选择
- **空程序头表**（ph_count=0）：`ArrayExpr` 求值为 0，返回空 List
- **空段头表**（sh_count=0）：同上
- **strtab_section_index 越界**：Pointer 寻址可能超出流范围，返回 Stream 错误
- **非标准 ELF 签名**：`Const` 构造器验证失败，返回 `ConstError`

---

## 4. PE/COFF 格式解析器设计

### 4.1 模块位置

`construct-rs/src/gallery/pe32coff.rs`

### 4.2 Python 源码结构分析

pe32coff.py 定义了以下组件：

| 组件 | 类型 | 作用 |
|------|------|------|
| `msdosheader` | Struct | MZ 头 + lfanew 指针 |
| `coffheader` | Struct | COFF 头（machine, sections_count, timestamp, characteristics） |
| `optionalheader` | Struct | PE 可选头（大量字段 + data directories） |
| `datadirectory` | Struct | 数据目录条目（含 Computed name） |
| `section` | Struct | 段表项（含 Pointer 引用的 rawdata/relocations/linenumbers） |
| `pe32file` | Struct | 完整 PE 文件 |

### 4.3 Rust 设计

```rust
//! PE/COFF 格式解析器。
//!
//! 移植自 `construct/gallery/pe32coff.py`。
//! 支持 PE32 和 PE32+ 格式。

use crate::constructs::*;
use crate::core::Construct;
use crate::expr::{this_, Evaluate};
use crate::value::Value;

/// MZ DOS 头。
///
/// 对应 Python `msdosheader`。
pub fn msdosheader() -> Struct {
    Struct::new()
        .field("signature", Box::new(Const::new_bytes(b"MZ".to_vec())))
        .field("lfanew", Box::new(Pointer::new(
            0x3c, // 固定偏移
            Box::new(INT16UL),
        )))
}

/// COFF 头。
///
/// 对应 Python `coffheader`。
pub fn coffheader() -> Struct {
    Struct::new()
        .field("signature", Box::new(Const::new_bytes(b"PE\x00\x00".to_vec())))
        .field("machine", Box::new(Enum::new(
            Box::new(INT16UL),
            vec![
                ("UNKNOWN", Value::UInt(0x0)),
                ("AMD64", Value::UInt(0x8664)),
                ("I386", Value::UInt(0x14c)),
                ("ARM64", Value::UInt(0xaa64)),
                // ... 完整列表见 pe32coff.py
            ],
        )))
        .field("sections_count", Box::new(INT16UL))
        .field("created", Box::new(TimestampAdapter::new(
            Box::new(INT32UL),
            TimestampUnit::Seconds,
            TimestampEpoch::Unix,
        )))
        .field("symbol_pointer", Box::new(INT32UL))
        .field("symbol_count", Box::new(INT32UL))
        .field("optionalheader_size", Box::new(INT16UL))
        .field("characteristics", Box::new(FlagsEnum::new(
            Box::new(INT16UL),
            vec![
                ("RELOCS_STRIPPED", Value::UInt(0x0001)),
                ("EXECUTABLE_IMAGE", Value::UInt(0x0002)),
                // ... 完整列表
            ],
        )))
}

/// 数据目录条目。
///
/// 对应 Python `datadirectory`。
/// name 字段使用 Computed 从 entriesnames 表中查找。
pub fn datadirectory() -> Struct {
    // entriesnames 映射表
    static ENTRIES_NAMES: [(u64, &str); 16] = [
        (0, "export_table"),
        (1, "import_table"),
        // ... 完整 16 项
        (15, "reserved"),
    ];
    Struct::new()
        .field("name", Box::new(Computed::new(Box::new(|ctx| {
            let index = ctx.get("_index")
                .and_then(|v| v.to_u64().ok())
                .ok_or_else(|| ConstructError::FieldMissing { /* ... */ })?;
            let name = ENTRIES_NAMES.iter()
                .find(|(i, _)| *i == index)
                .map(|(_, n)| *n)
                .unwrap_or("unknown");
            Ok(Value::String(name.to_string()))
        }))))
        .field("virtualaddress", Box::new(INT32UL))
        .field("size", Box::new(INT32UL))
}

/// PE 可选头。
///
/// 对应 Python `optionalheader`。
pub fn optionalheader() -> Struct {
    // plusfield: IfThenElse(this.signature == "PE32plus", Int64ul, Int32ul)
    // 需要运行时根据 signature 选择

    Struct::new()
        .field("signature", Box::new(Enum::new(Box::new(INT16UL), vec![
            ("PE32", Value::UInt(0x10b)),
            ("PE32plus", Value::UInt(0x20b)),
            ("ROMIMAGE", Value::UInt(0x107)),
        ])))
        .field("linker_version", Box::new(Array::new(2, Box::new(INT8UL))))
        .field("size_code", Box::new(INT32UL))
        .field("size_initialized_data", Box::new(INT32UL))
        .field("size_uninitialized_data", Box::new(INT32UL))
        .field("entrypoint", Box::new(INT32UL))
        .field("base_code", Box::new(INT32UL))
        // base_data: If(this.signature == "PE32", Int32ul)
        .field("base_data", Box::new(If::new(
            Box::new(|ctx| check_enum_field(ctx, &["signature"], "PE32")),
            Box::new(INT32UL),
        )))
        // image_base: plusfield — 运行时选择 32/64 位
        .field("image_base", Box::new(IfThenElse::new(
            Box::new(|ctx| check_enum_field(ctx, &["signature"], "PE32plus")),
            Box::new(INT64UL),
            Box::new(INT32UL),
        )))
        .field("section_alignment", Box::new(INT32UL))
        .field("file_alignment", Box::new(INT32UL))
        .field("os_version", Box::new(Array::new(2, Box::new(INT16UL))))
        .field("image_version", Box::new(Array::new(2, Box::new(INT16UL))))
        .field("subsystem_version", Box::new(Array::new(2, Box::new(INT16UL))))
        .field("win32versionvalue", Box::new(INT32UL))
        .field("image_size", Box::new(INT32UL))
        .field("headers_size", Box::new(INT32UL))
        .field("checksum", Box::new(INT32UL))
        .field("subsystem", Box::new(Enum::new(Box::new(INT16UL), vec![/* ... */])))
        .field("dll_characteristics", Box::new(FlagsEnum::new(Box::new(INT16UL), vec![/* ... */])))
        // stack/heap reserve/commit: plusfield
        .field("stack_reserve", Box::new(IfThenElse::new(
            Box::new(|ctx| check_enum_field(ctx, &["signature"], "PE32plus")),
            Box::new(INT64UL), Box::new(INT32UL),
        )))
        .field("stack_commit", Box::new(IfThenElse::new(
            Box::new(|ctx| check_enum_field(ctx, &["signature"], "PE32plus")),
            Box::new(INT64UL), Box::new(INT32UL),
        )))
        .field("heap_reserve", Box::new(IfThenElse::new(
            Box::new(|ctx| check_enum_field(ctx, &["signature"], "PE32plus")),
            Box::new(INT64UL), Box::new(INT32UL),
        )))
        .field("heap_commit", Box::new(IfThenElse::new(
            Box::new(|ctx| check_enum_field(ctx, &["signature"], "PE32plus")),
            Box::new(INT64UL), Box::new(INT32UL),
        )))
        .field("loader_flags", Box::new(INT32UL))
        .field("datadirectories_count", Box::new(INT32UL))
        // datadirectories: Array(this.datadirectories_count, datadirectory)
        .field("datadirectories", Box::new(ArrayExpr::new(
            Box::new(this_().field("datadirectories_count")),
            Box::new(datadirectory()),
        )))
}

/// 段表项。
///
/// 对应 Python `section`。
pub fn section() -> Struct {
    Struct::new()
        .field("name", Box::new(PaddedString::utf8(8)))
        .field("virtual_size", Box::new(INT32UL))
        .field("virtual_address", Box::new(INT32UL))
        .field("rawdata_size", Box::new(INT32UL))
        .field("rawdata_pointer", Box::new(INT32UL))
        .field("relocations_pointer", Box::new(INT32UL))
        .field("linenumbers_pointer", Box::new(INT32UL))
        .field("relocations_count", Box::new(INT16UL))
        .field("linenumbers_count", Box::new(INT16UL))
        .field("characteristics", Box::new(FlagsEnum::new(Box::new(INT32UL), vec![/* ... */])))
        // rawdata: Pointer(this.rawdata_pointer, Bytes(this.rawdata_size if pointer else 0))
        .field("rawdata", Box::new(PointerExpr::new(
            Box::new(this_().field("rawdata_pointer")),
            // 条件字节数 — 需要表达式返回 Bytes 的大小
            // 这里使用 Computed + Bytes 组合，或设计一个支持表达式长度的 BytesExpr
            // 简化方案：使用 Rebuild + Bytes 表达式
            Box::new(BytesExpr::new(
                Box::new(CondExpr::new(
                    this_().field("rawdata_pointer").gt(ConstExpr::new(Value::UInt(0))),
                    this_().field("rawdata_size"),
                    ConstExpr::new(Value::UInt(0)),
                )),
            )),
        )))
        // relocations: Pointer(this.relocations_pointer, Array(this.relocations_count, ...))
        .field("relocations", Box::new(PointerExpr::new(
            Box::new(this_().field("relocations_pointer")),
            Box::new(ArrayExpr::new(
                Box::new(this_().field("relocations_count")),
                Box::new(Struct::new()
                    .field("virtualaddress", Box::new(INT32UL))
                    .field("symboltable_index", Box::new(INT32UL))
                    .field("type", Box::new(INT16UL))
                ),
            )),
        )))
        // linenumbers: Pointer(this.linenumbers_pointer, Array(this.linenumbers_count, ...))
        .field("linenumbers", Box::new(PointerExpr::new(
            Box::new(this_().field("linenumbers_pointer")),
            Box::new(ArrayExpr::new(
                Box::new(this_().field("linenumbers_count")),
                Box::new(/* linenumber struct */),
            )),
        )))
}

/// 完整 PE32 文件构造器。
///
/// 对应 Python `pe32file`。
pub fn pe32file() -> Box<dyn Construct> {
    Box::new(Struct::new()
        .field("msdosheader", Box::new(msdosheader()))
        // Seek(this.msdosheader.lfanew) — 需要表达式 Seek
        .field_anonymous(Box::new(SeekExpr::new(
            Box::new(this_().field("msdosheader").field("lfanew")),
        )))
        .field("coffheader", Box::new(coffheader()))
        // optionalheader: If(this.coffheader.optionalheader_size > 0, optionalheader)
        .field("optionalheader", Box::new(If::new(
            Box::new(|ctx| {
                ctx.get_path(&["coffheader", "optionalheader_size"])
                    .and_then(|v| v.to_u64().ok())
                    .map(|v| v > 0)
                    .unwrap_or(false)
            }),
            Box::new(optionalheader()),
        )))
        .field("sections_count", Box::new(Computed::new(Box::new(|ctx| {
            ctx.get_path(&["coffheader", "sections_count"])
                .cloned()
                .ok_or_else(|| ConstructError::FieldMissing { /* ... */ })
        }))))
        .field("sections", Box::new(ArrayExpr::new(
            Box::new(this_().field("sections_count")),
            Box::new(section()),
        )))
    )
}
```

### 4.4 额外缺口：BytesExpr

PE `section.rawdata` 使用 `Bytes(lambda this: this.rawdata_size if this.rawdata_pointer else 0)`，需要表达式长度的 Bytes。当前 `Bytes` 仅支持固定 `length: usize`。

**方案**：新增 `BytesExpr` 构造器。

```rust
/// 表达式长度的 Bytes。
///
/// 对应 Python `Bytes(this.xxx)` 或 `Bytes(lambda this: ...)`。
pub struct BytesExpr {
    /// 求值为字节数的表达式。
    pub length_expr: Box<dyn Evaluate>,
}

impl BytesExpr {
    pub fn new(length_expr: Box<dyn Evaluate>) -> Self;
}
```

> **追加缺口说明**：此构造器应在 2.1 节的表达式参数化方案中一并实现。

### 4.5 边界条件清单

- **PE32 vs PE32+**：通过 `IfThenElse` 根据 signature 选择 32/64 位字段
- **无可选头**（optionalheader_size=0）：`If` 条件为 false，optionalheader 字段为 None
- **rawdata_pointer=0**：rawdata 大小为 0，不读取数据
- **空段表**（sections_count=0）：返回空 List
- **MZ 签名不符**：`Const` 验证失败
- **PE 签名不符**：`Const` 验证失败

---

## 5. ut_index 自定义构造器设计

### 5.1 模块位置

`construct-rs/src/gallery/ut_index.rs`

### 5.2 Python 源码分析

UTIndex 是 Unreal Tournament 1999 包中使用的变长有符号整数格式：

```
+------------------------------------+-------------------------+--------------+
| Byte 0                             | Bytes 1-3               | Byte 4       |
+----------+----------+--------------+----------+--------------+--------------+
| Sign Bit | More Bit | Data Bits[6] | More Bit | Data Bits[7] | Data Bits[8] |
+----------+----------+--------------+----------+--------------+--------------+
```

- Byte 0: 1 bit 符号 + 1 bit 继续 + 6 bit 数据
- Byte 1-3: 1 bit 继续 + 7 bit 数据
- Byte 4: 8 bit 数据（无继续位，最多 5 字节）

`lengths = {0: 6, 1: 7, 2: 7, 3: 7, 4: 8}`

### 5.3 Rust 设计

直接实现 `Construct` trait（不用任何已有构造器组合，因为是自定义位级格式）。

```rust
//! Unreal Tournament 1999 Index 变长整数格式。
//!
//! 移植自 `construct/gallery/ut_index.py`。

use crate::core::context::Context;
use crate::core::error::{ConstructError, Result};
use crate::core::stream::Stream;
use crate::core::Construct;
use crate::value::Value;

/// 每个字节中数据位的长度。
const LENGTHS: [usize; 5] = [6, 7, 7, 7, 8];

/// Byte 0 的符号位掩码。
const NEGATIVE_BIT: u8 = 0x80;

/// 返回指定数据位长度对应的数据掩码。
fn get_data_mask(length: usize) -> u8 {
    (0xFF ^ (0xFF << length)) & 0xFF
}

/// 返回指定数据位长度对应的"继续"位。
fn get_more_bit(length: usize) -> u8 {
    1 << length
}

/// Unreal Tournament 1999 Index 变长有符号整数。
///
/// 格式结构：
/// - Byte 0: 1 bit 符号 + 1 bit 继续 + 6 bit 数据
/// - Byte 1-3: 1 bit 继续 + 7 bit 数据
/// - Byte 4: 8 bit 数据（最多 5 字节）
///
/// - **parse**: 逐字节读取，累积数据位，直到"继续"位为 0
/// - **build**: 将整数拆分为数据位字节序列，设置符号位和继续位
/// - **sizeof**: 未定义（变长）
///
/// 对应 Python `UTIndex` 类。
///
/// # 示例
///
/// ```ignore
/// use construct::gallery::ut_index::UTIndex;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let d = UTIndex;
/// let c: &dyn Construct = &d;
/// // 值 0 → 单字节 0x00
/// let built = c.build_bytes(&Value::Int(0)).unwrap();
/// assert_eq!(built, vec![0x00]);
/// // 值 63 → 单字节 0x3F (6 bits all set)
/// let built = c.build_bytes(&Value::Int(63)).unwrap();
/// assert_eq!(built, vec![0x3F]);
/// // 值 64 → 两字节 0x40 0x01 (6 bits + 1 bit continuation)
/// let built = c.build_bytes(&Value::Int(64)).unwrap();
/// assert_eq!(built, vec![0x40, 0x01]);
/// ```
pub struct UTIndex;

impl UTIndex {
    /// 创建一个新的 UTIndex 构造器。
    pub fn new() -> Self {
        UTIndex
    }
}

impl Default for UTIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl Construct for UTIndex {
    fn parse(&self, stream: &mut dyn Stream, _ctx: &mut Context) -> Result<Value> {
        let mut result: i64 = 0;
        let mut sign: i64 = 1;
        let mut i: usize = 0;
        let mut depth: u32 = 0;

        loop {
            let length = LENGTHS[i];
            let byte = stream.read_bytes(1)?[0];
            let mask = get_data_mask(length);
            let data = (byte & mask) as i64;
            let more = get_more_bit(length) & byte;

            if i == 0 && (NEGATIVE_BIT & byte) != 0 {
                sign = -1;
            }

            result |= data << depth;

            if more == 0 {
                break;
            }

            i += 1;
            depth += length as u32;

            if i >= LENGTHS.len() {
                // 超过 5 字节，数据损坏
                return Err(ConstructError::Generic {
                    path: String::new(),
                    message: "UTIndex: more bit set on last byte (byte 4 has no more bit)"
                        .to_string(),
                });
            }
        }

        Ok(Value::Int(sign * result))
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, _ctx: &mut Context) -> Result<()> {
        let obj = data.to_i64()?;

        let mut to_write = obj;
        let negative = obj < 0;
        if negative {
            to_write = -to_write;
        }

        for i in 0..5usize {
            let length = LENGTHS[i];
            let mask = get_data_mask(length);
            let mut byte: u8 = 0;

            if i == 0 && negative {
                byte |= NEGATIVE_BIT;
            }

            byte |= (to_write as u8) & mask;
            to_write >>= length;

            // 如果还有数据要写，设置 more bit
            let more_bit = if to_write > 0 {
                get_more_bit(length)
            } else {
                0
            };
            byte |= more_bit;

            stream.write_bytes(&[byte])?;

            if more_bit == 0 {
                break;
            }
        }

        // 如果循环 5 次后 to_write 仍 > 0，值超出范围
        if to_write > 0 {
            return Err(ConstructError::Generic {
                path: String::new(),
                message: format!(
                    "UTIndex: value {} exceeds maximum representable value",
                    obj
                ),
            });
        }

        Ok(())
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Err(ConstructError::Sizeof {
            path: String::new(),
            reason: "UTIndex has variable size".to_string(),
        })
    }
}
```

### 5.4 边界条件清单

| 场景 | 输入 | 预期行为 |
|------|------|---------|
| 值 0 | `Value::Int(0)` | build: `[0x00]`；parse: 返回 0 |
| 最大正数（6+7+7+7+8=35 bits） | `Value::Int(2^34-1)` | 5 字节 |
| 负数 | `Value::Int(-1)` | build: `[0x80 \| 0x01]` = `[0x81]` |
| 负零 | `Value::Int(-0)` | 等同于 0 |
| 值 63 | `Value::Int(63)` | build: `[0x3F]`（6 bits 全满，无继续位） |
| 值 64 | `Value::Int(64)` | build: `[0x40, 0x01]`（继续位 + 第二字节 1 bit） |
| 超过 5 字节（more bit 在 byte 4 设置） | 损坏数据 | parse: 返回 `Generic` 错误 |
| 值超出 35 bit 范围 | `Value::Int(2^35)` | build: 返回 `Generic` 错误 |
| 非整数输入 | `Value::String("abc")` | build: 返回 `TypeMismatch` 错误 |
| EOF | 空流 | parse: 返回 `Stream` 错误 |

---

## 6. 测试策略

### 6.1 测试文件组织

```
construct-rs/tests/
├── gallery_elf.rs          ← ELF 集成测试
├── gallery_pe32coff.rs     ← PE/COFF 集成测试
├── gallery_ut_index.rs     ← UTIndex 单元+集成测试
└── test_data/              ← 测试用二进制文件（嵌入式或生成式）
```

### 6.2 UTIndex 测试矩阵

| 测试类别 | 具体测试 |
|---------|---------|
| 基础解析 | parse `[0x00]` → 0；parse `[0x3F]` → 63 |
| 多字节解析 | parse `[0x40, 0x01]` → 64；parse 3/4/5 字节序列 |
| 负数解析 | parse `[0x81]` → -1；parse `[0xBF]` → -63 |
| 基础构建 | build 0 → `[0x00]`；build 63 → `[0x3F]` |
| 多字节构建 | build 64 → `[0x40, 0x01]`；build 大值 → 正确字节序列 |
| 负数构建 | build -1 → `[0x81]` |
| 往返测试 | 一系列值 → build → parse → 验证一致性 |
| 边界值 | 最大正数、最小负数、溢出 |
| 错误处理 | 非整数输入、损坏数据、EOF |

### 6.3 ELF 测试策略

1. **合成测试数据**：手工构造最小化的 ELF 文件二进制数据
2. **真实文件测试**：使用 `/bin/ls` 或其他系统 ELF 文件（嵌入 bytes! 宏或测试数据文件）
3. **跨语言对比**：对同一数据分别运行 Python 和 Rust 解析，比较结果

### 6.4 PE/COFF 测试策略

1. **合成测试数据**：手工构造最小化 PE 文件
2. **真实文件测试**：使用 Windows 系统 DLL/EXE（嵌入测试数据）
3. **重点验证**：PE32 vs PE32+ 切换、Pointer 表寻址、段数据读取

### 6.5 跨语言对比测试框架

```rust
/// 对比测试辅助：运行 Python 解析并比较结果。
///
/// 要求环境中安装了 Python + construct 库。
/// 通过 std::process::Command 调用 Python 脚本。
#[cfg(test)]
fn compare_with_python(format_name: &str, test_data: &[u8]) {
    // 调用 Python 脚本解析同一数据
    // 将 Python 输出（JSON）与 Rust 解析结果比较
    // 仅在 CI 环境中启用（feature gate）
}
```

---

## 7. 与 Python 原版对照

### 7.1 构造器 API 映射总表

| Python 构造器/语法 | Rust 对应 | 状态 |
|-------------------|----------|------|
| `Struct("name"/subcon, ...)` | `Struct::new().field("name", subcon)` | ✅ 已有 |
| `Sequence(subcon, ...)` | `Sequence::new().push(subcon)` | ✅ 已有 |
| `Const(b"...")` | `Const::new_bytes(b"...".to_vec())` | ✅ 已有 |
| `Enum(Byte, NAME=val,...)` | `Enum::new(Box::new(U8), vec![("NAME", val)])` | ✅ 已有 |
| `FlagsEnum(Int16ul, FLAG=val,...)` | `FlagsEnum::new(Box::new(INT16UL), vec![...])` | ✅ 已有 |
| `Bytes(n)` | `Bytes::new(n)` | ✅ 已有 |
| `Bytes(this.xxx)` | `BytesExpr::new(this_().field("xxx"))` | ⬜ **新增** |
| `GreedyBytes` | `GreedyBytes` | ✅ 已有 |
| `Int8ul/Int16ul/Int32ul/Int64ul` | `INT8UL/INT16UL/INT32UL/INT64UL` | ✅ 已有 |
| `Int8ub/Int16ub/Int32ub/Int64ub` | `INT8UB/INT16UB/INT32UB/INT64UB` | ✅ 已有 |
| `IfThenElse(cond, then, else)` | `IfThenElse::new(cond, then, else)` | ✅ 已有 |
| `If(cond, subcon)` | `If::new(cond, subcon)` | ⬜ **新增** |
| `Pointer(offset, subcon)` | `Pointer::new(offset, subcon)` | ✅ 已有 |
| `Pointer(this.xxx, subcon)` | `PointerExpr::new(this_().field("xxx"), subcon)` | ⬜ **新增** |
| `Array(count, subcon)` | `Array::new(count, subcon)` | ✅ 已有 |
| `Array(this.xxx, subcon)` | `ArrayExpr::new(this_().field("xxx"), subcon)` | ⬜ **新增** |
| `Seek(at)` | `Seek::new(at)` | ✅ 已有 |
| `Seek(this.xxx)` | `SeekExpr::new(this_().field("xxx"))` | ⬜ **新增** |
| `Padding(n)` | `Padding::new(n)` | ⬜ **新增** |
| `PaddedString(length, enc)` | `PaddedString::new(length, StringEncoding::Utf8)` | ⬜ **新增** |
| `CString(enc)` | `CString::utf8()` | ⬜ **新增** |
| `Timestamp(intfield, 1., 1970)` | `TimestampAdapter::new(intfield, Seconds, Unix)` | ✅ 已有 |
| `Computed(lambda)` | `Computed::new(Box::new(closure))` | ✅ 已有 |
| `this.field` | `this_().field("field")` | ✅ 已有 |
| `this._.field` | `this_().field("_").field("field")` | ✅ 已有 |
| `this.a + this.b` | `this_().field("a").add(this_().field("b"))` | ✅ 已有 |
| `this.a * this.b` | `this_().field("a").mul(this_().field("b"))` | ✅ 已有 |
| `cond if x else y` | `CondExpr::new(cond, x, y)` | ✅ 已有 |
| 自定义 Construct 类 | 直接 `impl Construct` | ✅ trait 可实现 |

### 7.2 未覆盖的 Python 特性

| Python 特性 | 出现位置 | 处理方案 |
|------------|---------|---------|
| `subcon * "docstring"` (Renamed) | pe32coff.py line 223 | Rust 不需要（文档注释替代），跳过 |
| `docs * Struct(...)` (文档前缀) | pe32coff.py line 239 | Rust 不需要，跳过 |
| Python 字典字面量 `entriesnames` | pe32coff.py line 77 | Rust 使用静态数组 + Computed |
| `this._._index` (祖父上下文索引) | pe32coff.py line 97 | `this_().field("_").field("_index")` |

---

## 8. 设计决策记录

### 决策 8.1：表达式参数化使用新类型而非修改已有类型

**决策**：新增 `PointerExpr`、`ArrayExpr`、`SeekExpr`、`BytesExpr`，而非修改 `Pointer`、`Array`、`Seek`、`Bytes` 的字段类型。

**理由**：
1. **API 兼容**：Phase 1-7 的代码已使用 `Pointer::new(8, ...)` 等固定值 API，修改字段类型会破坏二进制兼容
2. **类型安全**：固定值和表达式值是不同的使用场景，分开类型更清晰
3. **编译期区分**：调用者明确知道自己使用的是固定值还是表达式，减少运行时分支
4. **与任务要求一致**：任务明确要求"设计 `PointerExpr`/`ArrayExpr` 等新类型不破坏已有 API"

**替代方案（已否决）**：使用 `enum Offset { Fixed(i64), Expr(Box<dyn Evaluate>) }` 作为字段类型。
否决理由：每次 parse/build 都需要 match 分支，增加运行时开销；且改变了已有 API 的构造函数签名。

### 决策 8.2：字符串编码使用枚举而非 &str

**决策**：`StringEncoding` 枚举而非接受 `&str` 编码名。

**理由**：
1. Python 使用字符串编码名（"utf8"、"utf16"），但 Rust 应该类型安全
2. 枚举避免运行时字符串匹配错误
3. 编码行为在编译期确定

### 决策 8.3：gallery 函数返回具体类型而非 Box<dyn Construct>

**决策**：gallery 的格式构造函数返回 `Struct`（如 `pub fn identifier() -> Struct`），顶层 `elf()` 返回 `Box<dyn Construct>`。

**理由**：
1. 返回具体类型允许调用者进一步组合
2. 顶层需要 `Box<dyn Construct>` 是因为 `IfThenElse` 返回不同类型分支
3. 与 Phase 1-7 的构造器风格一致

### 决策 8.4：UTIndex 直接实现 Construct trait

**决策**：UTIndex 不使用已有构造器组合，直接实现 `Construct` trait。

**理由**：
1. UTIndex 是自定义位级格式，无法用已有构造器表达（每个字节的位布局不同）
2. 直接实现性能更好，避免多层嵌套
3. 与 Python 原版一致（Python 中也是直接继承 Construct 类）

### 决策 8.5：ELF/PE 的 is64bit 参数在编译期固定

**决策**：`body()`、`program_header()` 等函数接受 `is64bit: bool` 参数，在编译期确定。顶层 `elf()` 预构建 4 种变体（32/64 × LE/BE），通过 `IfThenElse` 在运行时选择。

**理由**：
1. 与 Python 原版设计一致（Python 也是参数化函数）
2. 4 种变体在编译期生成，无运行时动态分发开销
3. `IfThenElse` 选择逻辑简单可靠

### 决策 8.6：新增 strings 模块

**决策**：CString 和 PaddedString 放在新的 `construct-rs/src/constructs/strings.rs` 模块中，而非 `bytes.rs`。

**理由**：
1. 字符串构造器有独立的编码逻辑（StringEncoding 枚举），逻辑独立
2. 与 Python 源码中 `StringEncoded` adapter 的独立地位对应
3. 未来可能扩展更多字符串类型（PascalString、GreedyString）

### 决策 8.7：If/Padding 使用辅助函数而非新 struct

**决策**：`If` 和 `Padding` 作为辅助函数返回已有的 `IfThenElse` 和 `Padded` 类型，而非新的 struct。

**理由**：
1. Python 原版中 `If` 和 `Padding` 本身就是返回已有构造器的工厂函数
2. 避免不必要的类型膨胀
3. 运行时行为完全等价于展开后的 `IfThenElse`/`Padded`

---

## 9. 实现优先级与子任务拆分建议

### 9.1 前置子任务（gallery 依赖的基础设施）

建议在开始 gallery 格式移植前，先完成以下基础设施子任务：

| 优先级 | 子任务 | 依赖 | 估时 |
|--------|--------|------|------|
| P0 | 新增 `strings.rs` 模块（CString + PaddedString + StringEncoding） | 无 | 0.5 天 |
| P0 | 新增 `PointerExpr` / `ArrayExpr` / `SeekExpr` / `BytesExpr` | Phase 5 表达式系统 | 0.5 天 |
| P1 | 新增 `If` / `Padding` 辅助函数 | 已有 IfThenElse / Padded | 0.5 天 |

### 9.2 Gallery 子任务

| 优先级 | 子任务 | 依赖 | 估时 |
|--------|--------|------|------|
| P0 | ut_index 移植 + 测试 | 无 | 0.5 天 |
| P1 | ELF 移植 + 测试 | 全部 P0 基础设施 | 1.5 天 |
| P1 | PE/COFF 移植 + 测试 | 全部 P0 基础设施 | 1.5 天 |
| P2 | 跨语言对比测试 | ELF + PE 完成 | 1 天 |

### 9.3 风险评估

| 风险 | 概率 | 影响 | 缓解措施 |
|------|------|------|---------|
| 表达式参数化设计有遗漏 | 中 | 高 | 本文档已枚举所有 gallery 中的表达式用法 |
| 真实 ELF/PE 文件暴露构造器 bug | 中 | 中 | 预留 bugfix 时间，优先修复 |
| Python-Rust 结果对比有细微差异 | 高 | 低 | 差异通常在 Enum 的 default 行为或 Container 排序上，可接受 |
| CString/PaddedString 编码边界 case | 低 | 中 | 充分的单元测试覆盖 |

---

## 附录 A：Enum/FlagsEnum 构造器 API 确认

> DEV 在实现 gallery 时需确认以下 API 签名与实际代码一致。

```rust
// Enum 构造
Enum::new(subcon: Box<dyn Construct>, mappings: Vec<(&str, Value)>) -> Enum;
// 或使用构建器
Enum::new(Box::new(INT16UL))
    .map("AMD64", Value::UInt(0x8664))
    .map("I386", Value::UInt(0x14c))
    // ...

// FlagsEnum 构造
FlagsEnum::new(subcon: Box<dyn Construct>, flags: Vec<(&str, Value)>) -> FlagsEnum;
```

具体 API 以 `construct-rs/src/constructs/enum_.rs` 实际实现为准。如 gallery 实现时发现 API 不匹配，由 DEV 向 PM 提出 Argue，ARCH 回应。
