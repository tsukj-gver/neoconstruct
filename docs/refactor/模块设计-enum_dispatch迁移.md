# 模块设计：enum_dispatch 迁移（CombinedConstruct / CombinedStream / CombinedExpr）

> **文档性质**：模块设计（ARCH 产出）
> **日期**：2026-06-16
> **状态**：DESIGNING（待 REV 检视）
> **覆盖子任务**：Phase 11 — 11.1 CombinedConstruct + CombinedStream 迁移 / 11.2 CombinedExpr 迁移
> **前置文档**：
> - `docs/refactor/顶层架构设计.md` — §3.1 D1（分发机制）、§3.5 D5（Stream）、§3.7 D7（表达式）
>
> **设计目标**：将三个核心 trait（`Construct` / `Stream` / `Evaluate`）从 `Box<dyn Trait>` 动态
> 分发迁移到 `enum_dispatch` 静态枚举分发，为 Phase 12（编译执行树）提供零开销基础。
> 本次迁移**纯机械、不改语义**：parse/build 仍返回 `Value`，行为与 Phase 1-10 完全一致。

---

## 1. 概述与目标

### 1.1 当前状态（旧架构）

| Trait | 动态分发形式 | 实现者数量 | 性能影响 |
|-------|-------------|-----------|---------|
| `Construct` | `Box<dyn Construct>` | 70（含 1 个 cfg-gated） | 虚函数表跳转，无法内联（顶层设计 §3.3） |
| `Stream` | `&mut dyn Stream` + 辅助函数 `&mut dyn Stream` | 2（ByteStream, WindowedStream） | 轻微开销 + 三层抽象维护成本（§3.7） |
| `Evaluate` | `Box<dyn Evaluate>` | 7 | 每节点虚分发（§3.8） |

三个 trait 的动态分发**互相耦合**：`Construct::parse/build/build_effective` 的签名包含
`stream: &mut dyn Stream` 参数。因此 Construct 和 Stream **必须同批次迁移**；Evaluate 独立，
可最后迁移（或与 Construct 同批）。

### 1.2 迁移后状态（新架构）

| Trait | 分发形式 | 枚举名 | variant 数 |
|-------|---------|--------|-----------|
| `Construct` | `#[enum_dispatch]` + `CombinedConstruct` | `CombinedConstruct` | 70 |
| `Stream` | `#[enum_dispatch]` + `CombinedStream` | `CombinedStream` | 2（预留 2） |
| `Evaluate` | `#[enum_dispatch]` + `CombinedExpr` | `CombinedExpr` | 7 |

迁移后字段类型变更：
- `Box<dyn Construct>` → `Box<CombinedConstruct>`（保留 `Box`，因构造器树是**递归类型**）
- `&mut dyn Stream` → `&mut CombinedStream`（无需 Box，流不是递归类型）
- `Box<dyn Evaluate>` → `Box<CombinedExpr>`（保留 `Box`，表达式树是**递归类型**）

### 1.3 不在本次范围内（明确排除）

| 项目 | 原因 | 处理阶段 |
|------|------|---------|
| `CompiledNode` 执行树 | 全新设计，与分发迁移正交 | Phase 12 |
| parse/build 返回类型从 `Value` 改为 direct-to-Python | 改变语义，属 FFI 层重构 | Phase 12+ |
| `Context` 惰性视图 | 独立决策（D4） | Phase 12+ |
| `PyCallback` 节点 | FFI 边界设计 | Phase 12+ |
| gallery 的 `UTIndex`（用户自定义 Construct） | 在 gallery 层，不进核心 enum | 见 §6.4 |

> **REV 问题 #1 回应（D1/D2 耦合）**：CombinedConstruct ≠ CompiledNode。CombinedConstruct 只是
> 将 `Box<dyn Construct>` 替换为 `Box<CombinedConstruct>`，分发机制从虚表改为 match，**parse/build
> 仍返回 `Value`**。CompiledNode 是 Phase 12 的全新执行树设计，会引入 direct-to-Python 产出。
> 两者职责完全不同，详见 §7。

---

## 2. CombinedConstruct enum 设计

### 2.1 完整 variant 清单（70 个）

按源码模块分组，逐一定义 enum variant。`Compressed` 为 `cfg(feature = "compression")` 门控。

#### 2.1.1 core 层（2 个）

| # | Variant 名 | 包装类型 | 源码位置 |
|---|-----------|---------|---------|
| 1 | `Subconstruct` | `core::Subconstruct` | `core/mod.rs:216` |
| 2 | `Renamed` | `core::Renamed` | `core/mod.rs:262` |

#### 2.1.2 原子构造器（13 个）

| # | Variant 名 | 包装类型 | 源码位置 |
|---|-----------|---------|---------|
| 3 | `FormatField` | `format_field::FormatField` | `format_field.rs:157` |
| 4 | `Bytes` | `bytes::Bytes` | `bytes.rs:52` |
| 5 | `GreedyBytes` | `bytes::GreedyBytes` | `bytes.rs:168` |
| 6 | `BytesExpr` | `bytes::BytesExpr` | `bytes.rs:244` |
| 7 | `BytesInteger` | `bytes_integer::BytesInteger` | `bytes_integer.rs:129` |
| 8 | `BitsInteger` | `bytes_integer::BitsInteger` | `bytes_integer.rs:230` |
| 9 | `VarInt` | `varint::VarInt` | `varint.rs:57` |
| 10 | `ZigZag` | `varint::ZigZag` | `varint.rs:199` |
| 11 | `Flag` | `flag::Flag` | `flag.rs:58` |
| 12 | `CString` | `strings::CString` | `strings.rs:366` |
| 13 | `PaddedString` | `strings::PaddedString` | `strings.rs:458` |
| 14 | `Const` | `const_::Const` | `const_.rs:87` |
| 15 | `Mapping` | `enum_::Mapping` | `enum_.rs:388` |

#### 2.1.3 适配器/校验器（5 个）

| # | Variant 名 | 包装类型 | 源码位置 |
|---|-----------|---------|---------|
| 16 | `Adapter` | `adapters::Adapter` | `adapters.rs:138` |
| 17 | `SymmetricAdapter` | `adapters::SymmetricAdapter` | `adapters.rs:214` |
| 18 | `ExprAdapter` | `adapters::ExprAdapter` | `adapters.rs:270` |
| 19 | `Validator` | `adapters::Validator` | `adapters.rs:355` |
| 20 | `ExprValidator` | `adapters::ExprValidator` | `adapters.rs:401` |

#### 2.1.4 元构造器（6 个）

| # | Variant 名 | 包装类型 | 源码位置 |
|---|-----------|---------|---------|
| 21 | `Pass` | `meta::Pass` | `meta.rs:65` |
| 22 | `Terminated` | `meta::Terminated` | `meta.rs:121` |
| 23 | `Tell` | `meta::Tell` | `meta.rs:191` |
| 24 | `Seek` | `meta::Seek` | `meta.rs:288` |
| 25 | `SeekExpr` | `meta::SeekExpr` | `meta.rs:442` |
| 26 | `Error` | `meta::Error` | `meta.rs:357` |

#### 2.1.5 复合构造器（5 个）

| # | Variant 名 | 包装类型 | 源码位置 |
|---|-----------|---------|---------|
| 27 | `Struct` | `struct_::Struct` | `struct_.rs:216` |
| 28 | `Sequence` | `sequence::Sequence` | `sequence.rs:205` |
| 29 | `Union` | `union::Union` | `union.rs:141` |
| 30 | `Select` | `select::Select` | `select.rs:99` |
| 31 | `FocusedSeq` | `focused_seq::FocusedSeq` | `focused_seq.rs:112` |

#### 2.1.6 枚举/计算构造器（11 个）

| # | Variant 名 | 包装类型 | 源码位置 |
|---|-----------|---------|---------|
| 32 | `Enum` | `enum_::Enum` | `enum_.rs:99` |
| 33 | `FlagsEnum` | `enum_::FlagsEnum` | `enum_.rs:215` |
| 34 | `Computed` | `computed::Computed` | `computed.rs:72` |
| 35 | `Rebuild` | `computed::Rebuild` | `computed.rs:155` |
| 36 | `Default` | `computed::Default` | `computed.rs:235` |
| 37 | `Index` | `computed::Index` | `computed.rs:315` |
| 38 | `Padded` | `computed::Padded` | `computed.rs:403` |
| 39 | `Aligned` | `computed::Aligned` | `computed.rs:554` |
| 40 | `FixedSized` | `computed::FixedSized` | `computed.rs:640` |
| 41 | `NamedTuple` | `computed::NamedTuple` | `computed.rs:740` |
| 42 | `TimestampAdapter` | `computed::TimestampAdapter` | `computed.rs:951` |

#### 2.1.7 重复构造器（4 个）

| # | Variant 名 | 包装类型 | 源码位置 |
|---|-----------|---------|---------|
| 43 | `Array` | `repetition::Array` | `repetition.rs:127` |
| 44 | `ArrayExpr` | `repetition::ArrayExpr` | `repetition.rs:240` |
| 45 | `GreedyRange` | `repetition::GreedyRange` | `repetition.rs:372` |
| 46 | `RepeatUntil` | `repetition::RepeatUntil` | `repetition.rs:541` |

#### 2.1.8 惰性构造器（4 个）

| # | Variant 名 | 包装类型 | 源码位置 |
|---|-----------|---------|---------|
| 47 | `Lazy` | `lazy::Lazy` | `lazy.rs:115` |
| 48 | `LazyStruct` | `lazy::LazyStruct` | `lazy.rs:219` |
| 49 | `LazyArray` | `lazy::LazyArray` | `lazy.rs:300` |
| 50 | `Rebuffered` | `lazy::Rebuffered` | `lazy.rs:391` |

#### 2.1.9 流操作/隧道构造器（14 个）

| # | Variant 名 | 包装类型 | 源码位置 | 备注 |
|---|-----------|---------|---------|------|
| 51 | `Bitwise` | `stream_ops::Bitwise` | `stream_ops.rs:80` | |
| 52 | `Bytewise` | `stream_ops::Bytewise` | `stream_ops.rs:160` | |
| 53 | `Pointer` | `stream_ops::Pointer` | `stream_ops.rs:265` | |
| 54 | `PointerExpr` | `stream_ops::PointerExpr` | `stream_ops.rs:361` | |
| 55 | `Peek` | `stream_ops::Peek` | `stream_ops.rs:433` | |
| 56 | `RawCopy` | `stream_ops::RawCopy` | `stream_ops.rs:503` | |
| 57 | `Prefixed` | `stream_ops::Prefixed` | `stream_ops.rs:678` | |
| 58 | `Transformed` | `stream_ops::Transformed` | `stream_ops.rs:802` | |
| 59 | `Restreamed` | `stream_ops::Restreamed` | `stream_ops.rs:904` | |
| 60 | `Compressed` | `stream_ops::Compressed` | `stream_ops.rs:1064` | `#[cfg(feature="compression")]` |
| 61 | `Checksum` | `stream_ops::Checksum` | `stream_ops.rs:1156` | |
| 62 | `ByteSwapped` | `stream_ops::ByteSwapped` | `stream_ops.rs:1233` | |
| 63 | `BitsSwapped` | `stream_ops::BitsSwapped` | `stream_ops.rs:1331` | |
| 64 | `LazyBound` | `stream_ops::LazyBound` | `stream_ops.rs:1384` | |

#### 2.1.10 控制流构造器（4 个）

| # | Variant 名 | 包装类型 | 源码位置 |
|---|-----------|---------|---------|
| 65 | `IfThenElse` | `control_flow::IfThenElse` | `control_flow.rs:131` |
| 66 | `Switch` | `control_flow::Switch` | `control_flow.rs:285` |
| 67 | `Check` | `control_flow::Check` | `control_flow.rs:408` |
| 68 | `StopIf` | `control_flow::StopIf` | `control_flow.rs:500` |

#### 2.1.11 格式化包装器（2 个）

| # | Variant 名 | 包装类型 | 源码位置 |
|---|-----------|---------|---------|
| 69 | `Hex` | `hex::Hex` | `hex.rs:76` |
| 70 | `HexDump` | `hex::HexDump` | `hex.rs:136` |

> **合计：70 个 variant**（其中 `Compressed` 受 `cfg(feature = "compression")` 门控，未启用该
> feature 时为 69 个 variant）。

### 2.2 enum 定义

定义在新模块 `construct-rs/src/combined.rs`（可引用所有 `constructs::*` 子模块类型，避免
`core/mod.rs` 反向依赖 `constructs`）：

```rust
//! Combined dispatch enums for Construct / Stream / Evaluate traits.
//! Generated dispatch via `enum_dispatch`, eliminating `Box<dyn Trait>` overhead.

use crate::core::stream::Stream;
use crate::core::{Construct, Renamed, Subconstruct};
use crate::constructs::*;
use crate::expr::Evaluate;

/// enum_dispatch 包装枚举，替代 `Box<dyn Construct>`。
///
/// 所有 70 个内置构造器均为 variant。`Compressed` 受 feature gate 控制。
#[derive(Debug)]
#[allow(clippy::large_enum_variant)] // 见 §6.1 enum 大小评估
pub enum CombinedConstruct {
    // core
    Subconstruct(Subconstruct),
    Renamed(Renamed),
    // 原子
    FormatField(constructs::format_field::FormatField),
    Bytes(constructs::bytes::Bytes),
    GreedyBytes(constructs::bytes::GreedyBytes),
    BytesExpr(constructs::bytes::BytesExpr),
    BytesInteger(constructs::bytes_integer::BytesInteger),
    BitsInteger(constructs::bytes_integer::BitsInteger),
    VarInt(constructs::varint::VarInt),
    ZigZag(constructs::varint::ZigZag),
    Flag(constructs::flag::Flag),
    CString(constructs::strings::CString),
    PaddedString(constructs::strings::PaddedString),
    Const(constructs::const_::Const),
    Mapping(constructs::enum_::Mapping),
    // 适配器
    Adapter(constructs::adapters::Adapter),
    SymmetricAdapter(constructs::adapters::SymmetricAdapter),
    ExprAdapter(constructs::adapters::ExprAdapter),
    Validator(constructs::adapters::Validator),
    ExprValidator(constructs::adapters::ExprValidator),
    // 元
    Pass(constructs::meta::Pass),
    Terminated(constructs::meta::Terminated),
    Tell(constructs::meta::Tell),
    Seek(constructs::meta::Seek),
    SeekExpr(constructs::meta::SeekExpr),
    Error(constructs::meta::Error),
    // 复合
    Struct(constructs::struct_::Struct),
    Sequence(constructs::sequence::Sequence),
    Union(constructs::union::Union),
    Select(constructs::select::Select),
    FocusedSeq(constructs::focused_seq::FocusedSeq),
    // 枚举/计算
    Enum(constructs::enum_::Enum),
    FlagsEnum(constructs::enum_::FlagsEnum),
    Computed(constructs::computed::Computed),
    Rebuild(constructs::computed::Rebuild),
    Default(constructs::computed::Default),
    Index(constructs::computed::Index),
    Padded(constructs::computed::Padded),
    Aligned(constructs::computed::Aligned),
    FixedSized(constructs::computed::FixedSized),
    NamedTuple(constructs::computed::NamedTuple),
    TimestampAdapter(constructs::computed::TimestampAdapter),
    // 重复
    Array(constructs::repetition::Array),
    ArrayExpr(constructs::repetition::ArrayExpr),
    GreedyRange(constructs::repetition::GreedyRange),
    RepeatUntil(constructs::repetition::RepeatUntil),
    // 惰性
    Lazy(constructs::lazy::Lazy),
    LazyStruct(constructs::lazy::LazyStruct),
    LazyArray(constructs::lazy::LazyArray),
    Rebuffered(constructs::lazy::Rebuffered),
    // 流操作
    Bitwise(constructs::stream_ops::Bitwise),
    Bytewise(constructs::stream_ops::Bytewise),
    Pointer(constructs::stream_ops::Pointer),
    PointerExpr(constructs::stream_ops::PointerExpr),
    Peek(constructs::stream_ops::Peek),
    RawCopy(constructs::stream_ops::RawCopy),
    Prefixed(constructs::stream_ops::Prefixed),
    Transformed(constructs::stream_ops::Transformed),
    Restreamed(constructs::stream_ops::Restreamed),
    #[cfg(feature = "compression")]
    Compressed(constructs::stream_ops::Compressed),
    Checksum(constructs::stream_ops::Checksum),
    ByteSwapped(constructs::stream_ops::ByteSwapped),
    BitsSwapped(constructs::stream_ops::BitsSwapped),
    LazyBound(constructs::stream_ops::LazyBound),
    // 控制流
    IfThenElse(constructs::control_flow::IfThenElse),
    Switch(constructs::control_flow::Switch),
    Check(constructs::control_flow::Check),
    StopIf(constructs::control_flow::StopIf),
    // 格式化包装器
    Hex(constructs::hex::Hex),
    HexDump(constructs::hex::HexDump),
}

#[enum_dispatch::enum_dispatch(Construct)]
impl Construct for CombinedConstruct {}
```

> **enum_dispatch 工作原理**：宏读取 `impl Construct for CombinedConstruct {}`，自动为
> `CombinedConstruct` 生成 `parse/build/sizeof/flagbuildnone/build_effective` 方法，方法体是
> 一个 `match self { Self::Subconstruct(inner) => inner.parse(...), ... }`，将调用转发给内部
> 具体类型。编译器可对这个 match 做分支内联优化。**无需手写 match**。

### 2.3 字段类型变更映射表（`Box<dyn Construct>` → `Box<CombinedConstruct>`）

下表列出所有持有 `Box<dyn Construct>` 字段的结构体及其变更。**所有字段统一改为
`Box<CombinedConstruct>`**（保留 `Box`，因构造器树是递归类型，无 Box 会无限大）。

| 结构体 | 字段 | 旧类型 | 新类型 | 源码位置 |
|--------|------|--------|--------|---------|
| `Subconstruct` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `core/mod.rs:218` |
| `Renamed` | `inner` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `core/mod.rs:264` |
| `Const` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `const_.rs:43` |
| `Adapter` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `adapters.rs:113` |
| `SymmetricAdapter` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `adapters.rs:196` |
| `ExprAdapter` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `adapters.rs:245` |
| `Validator` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `adapters.rs:336` |
| `ExprValidator` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `adapters.rs:382` |
| `Enum` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `enum_.rs:62` |
| `FlagsEnum` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `enum_.rs:190` |
| `Mapping` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `enum_.rs:324` |
| `StructField` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `struct_.rs:72` |
| `SeqEntry` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `sequence.rs:65` |
| `Rebuild` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `computed.rs:142` |
| `Default` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `computed.rs:222` |
| `Padded` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `computed.rs:375` |
| `Aligned` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `computed.rs:512` |
| `FixedSized` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `computed.rs:628` |
| `NamedTuple` | `inner` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `computed.rs:717` |
| `TimestampAdapter` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `computed.rs:907` |
| `Array` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `repetition.rs:73` |
| `ArrayExpr` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `repetition.rs:219` |
| `GreedyRange` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `repetition.rs:328` |
| `RepeatUntil` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `repetition.rs:501` |
| `Lazy` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `lazy.rs:96` |
| `LazyArray` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `lazy.rs`(field) |
| `Rebuffered` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `lazy.rs:364` |
| `Bitwise` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `stream_ops.rs` |
| `Bytewise` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `stream_ops.rs` |
| `Pointer` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `stream_ops.rs` |
| `PointerExpr` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `stream_ops.rs` |
| `Peek` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `stream_ops.rs` |
| `RawCopy` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `stream_ops.rs` |
| `Prefixed` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `stream_ops.rs` |
| `Transformed` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `stream_ops.rs` |
| `Restreamed` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `stream_ops.rs` |
| `Compressed` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `stream_ops.rs`(cfg) |
| `Checksum` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `stream_ops.rs` |
| `ByteSwapped` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `stream_ops.rs` |
| `BitsSwapped` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `stream_ops.rs` |
| `LazyBound` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `stream_ops.rs` |
| `IfThenElse` | `then_constr` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `control_flow.rs:105` |
| `IfThenElse` | `else_constr` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `control_flow.rs:107` |
| `Switch` | `cases` | `Vec<(Value, Box<dyn Construct>)>` | `Vec<(Value, Box<CombinedConstruct>)>` | `control_flow.rs:238` |
| `Switch` | `default` | `Option<Box<dyn Construct>>` | `Option<Box<CombinedConstruct>>` | `control_flow.rs:241` |
| `Select` | `subcons` | `Vec<Box<dyn Construct>>` | `Vec<Box<CombinedConstruct>>` | `select.rs:61` |
| `Hex` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `hex.rs:62` |
| `HexDump` | `subcon` | `Box<dyn Construct>` | `Box<CombinedConstruct>` | `hex.rs:122` |
| `FocusedSeq` | `fields` | `Vec<StructField>`（内含 subcon） | 同（字段类型已变） | `focused_seq.rs` |
| `LazyStruct` | `fields` | `Vec<StructField>`（内含 subcon） | 同（字段类型已变） | `lazy.rs` |
| `Sequence` | `subcons` | `Vec<SeqEntry>`（内含 subcon） | 同（字段类型已变） | `sequence.rs` |
| `Struct` | `fields` | `Vec<StructField>`（内含 subcon） | 同（字段类型已变） | `struct_.rs` |

> **`LazyStruct`/`LazyArray` 的 builder 方法**（`field`/`anonymous`/`new`）参数类型从
> `Box<dyn Construct>` 改为 `Box<CombinedConstruct>`，同 `StructField::new` / `SeqEntry::new`。

### 2.4 构造方式变更

所有创建 boxed construct 的调用点需同步迁移。两类变更：

**变更 A：`Renamed::new` 签名**
```rust
// 旧
pub fn new(inner: Box<dyn Construct>, name: impl Into<String>) -> Self
// 新
pub fn new(inner: Box<CombinedConstruct>, name: impl Into<String>) -> Self
```
同理适用于所有接受 `Box<dyn Construct>` 参数的 `new`/`field`/`push`/`anonymous` 方法
（约 40 处，见 §2.3 表中 builder 方法）。

**变更 B：构造调用点**

所有形如 `Box::new(XxxConstruct) as Box<dyn Construct>` 或 `Box::new(XxxConstruct)` 的
构造，改为 `Box::new(CombinedConstruct::XxxConstruct(...))`。

示例（`Renamed` 包装）：
```rust
// 旧
Renamed::new(Box::new(INT8UB), "len")
// 新（INT8UB 等常量需改为返回 CombinedConstruct，见下方常量迁移）
Renamed::new(Box::new(INT8UB), "len")  // INT8UB 类型已变为 CombinedConstruct
```

**`format_field` 常量迁移**：`INT8UB` / `INT16UB` / `INT32UB` 等预定义常量当前类型为
`FormatField`。需确认：是改为 `CombinedConstruct` 类型，还是保持 `FormatField`（依赖
`From<FormatField> for CombinedConstruct`）？

**决策**：保持常量为 `FormatField` 类型（避免改动常量定义和文档示例），但在需要 boxed 时通过
`Box::new(CombinedConstruct::FormatField(INT8UB))` 包装。为减少样板代码，提供便捷转换：
```rust
impl From<FormatField> for CombinedConstruct {
    fn from(f: FormatField) -> Self { CombinedConstruct::FormatField(f) }
}
// 可为每个 variant 批量生成 From，或用宏
```
> DEV 可选择为所有 70 个类型实现 `From<T> for CombinedConstruct`（推荐用 `macro_rules!`
> 批量生成），使调用点可写 `Box::new(INT8UB.into())`。

### 2.5 `dyn Construct` 便利方法的保留

`core/mod.rs:113` 的 `impl dyn Construct` 提供 `parse_bytes`/`build_bytes`/`parse_file`/
`build_file` 便利方法。迁移后这些方法应转移到 `impl CombinedConstruct`（或泛型 `impl<T:
Construct>`）。**推荐**：定义为 `impl CombinedConstruct`，因为这是用户的主要入口类型。

---

## 3. CombinedStream enum 设计

### 3.1 当前 Stream 实现者（2 个）

| # | 类型 | 源码位置 | 用途 |
|---|------|---------|------|
| 1 | `ByteStream` | `core/stream.rs:126` | 内存读写流（Cursor 包装），parse/build 默认实现 |
| 2 | `WindowedStream` | `core/stream.rs:257` | 带绝对偏移的只读窗口流，Prefixed/RawCopy 使用 |

> Phase 13 会新增 `PyStream`（Python io 对象包装）和 `BitStream`（位级流），故 enum 预留扩展。

### 3.2 enum 定义

定义在 `core/stream.rs`（Stream trait 所在模块，ByteStream/WindowedStream 均在此）：

```rust
/// enum_dispatch 包装枚举，替代 `&mut dyn Stream`。
///
/// 当前 2 个 variant；Phase 13 将新增 PyStream / BitStream。
#[derive(Debug)]
pub enum CombinedStream {
    ByteStream(ByteStream),
    WindowedStream(WindowedStream),
    // 预留（Phase 13）：
    // PyStream(PyStream),
    // BitStream(BitStream),
}

#[enum_dispatch::enum_dispatch(Stream)]
impl Stream for CombinedStream {}
```

### 3.3 签名变更

#### 3.3.1 Construct trait 签名（5 个方法）

```rust
// 旧（core/mod.rs:39-100）
pub trait Construct {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value>;
    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()>;
    fn sizeof(&self, ctx: &Context) -> Result<usize>;
    fn flagbuildnone(&self) -> bool { false }
    fn build_effective(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> { ... }
}

// 新
pub trait Construct {
    fn parse(&self, stream: &mut CombinedStream, ctx: &mut Context) -> Result<Value>;
    fn build(&self, data: &Value, stream: &mut CombinedStream, ctx: &mut Context) -> Result<()>;
    fn sizeof(&self, ctx: &Context) -> Result<usize>;
    fn flagbuildnone(&self) -> bool { false }
    fn build_effective(&self, data: &Value, stream: &mut CombinedStream, ctx: &mut Context) -> Result<Value> { ... }
}
```

> **影响面**：`&mut dyn Stream` 在源码中出现 ~100 处（每个 Construct 实现者的 parse/build 方法
> 签名）。这是机械替换，DEV 可用全局搜索替换 `&mut dyn Stream` → `&mut CombinedStream`。

#### 3.3.2 辅助函数签名（6 个）

```rust
// 旧（core/stream.rs:422-487）
pub fn stream_read(stream: &mut dyn Stream, count: usize, path: &str) -> Result<Vec<u8>>
pub fn stream_write(stream: &mut dyn Stream, data: &[u8], path: &str) -> Result<()>
pub fn stream_seek(stream: &mut dyn Stream, offset: i64, path: &str) -> Result<()>
pub fn stream_tell(stream: &mut dyn Stream, path: &str) -> Result<u64>
pub fn stream_size(stream: &mut dyn Stream, path: &str) -> Result<u64>
pub fn stream_iseof(stream: &mut dyn Stream, path: &str) -> Result<bool>

// 新：参数类型改为 &mut CombinedStream
pub fn stream_read(stream: &mut CombinedStream, count: usize, path: &str) -> Result<Vec<u8>>
// ... 其余同理
```

#### 3.3.3 `dyn Construct` 便利方法中的流构造

`core/mod.rs:132-161` 中 `parse_bytes`/`build_bytes` 内部创建 `ByteStream`：

```rust
// 新
pub fn parse_bytes(&self, data: &[u8]) -> Result<Value> {
    let mut stream = CombinedStream::ByteStream(ByteStream::new_read(data));
    let mut ctx = Context::new();
    self.parse(&mut stream, &mut ctx).map_err(...)
}
```

### 3.4 Stream 的 Box 需求分析

`CombinedStream` **无需 Box**：
- `ByteStream` 和 `WindowedStream` 都不持有 `Stream` 字段（`WindowedStream` 持有 `Vec<u8>` 副本，
  非流引用），因此 `CombinedStream` 不是递归类型。
- `&mut CombinedStream` 直接传递即可，无堆分配开销。

> **注意**：当前 `Prefixed`/`RawCopy`/`Restreamed` 等构造器在内部创建临时 `ByteStream`/
> `WindowedStream` 并调用 `subcon.parse(&mut sub_stream, ...)`。迁移后这些临时流需包装为
> `CombinedStream::ByteStream(...)` / `CombinedStream::WindowedStream(...)`。

---

## 4. CombinedExpr enum 设计

### 4.1 当前 Evaluate 实现者（7 个）

| # | Variant 名 | 包装类型 | 源码位置 | 含 `Box<dyn Evaluate>` 字段 |
|---|-----------|---------|---------|---------------------------|
| 1 | `Path` | `expr::Path` | `expr.rs:149` | 否（segments: Vec<String>） |
| 2 | `ConstExpr` | `expr::ConstExpr` | `expr.rs:214` | 否（value: Value） |
| 3 | `BinExpr` | `expr::BinExpr` | `expr.rs:348` | 是（left, right） |
| 4 | `UniExpr` | `expr::UniExpr` | `expr.rs:695` | 是（inner） |
| 5 | `FuncPath` | `expr::FuncPath` | `expr.rs:782` | 是（inner） |
| 6 | `CondExpr` | `expr::CondExpr` | `expr.rs:937` | 是（cond, then_val, else_val） |
| 7 | `ListPath` | `expr::ListPath` | `expr.rs:1032` | 是（inner: Option） |

### 4.2 enum 定义

定义在 `expr.rs`（Evaluate trait 所在模块）：

```rust
/// enum_dispatch 包装枚举，替代 `Box<dyn Evaluate>`。
#[derive(Debug)]
pub enum CombinedExpr {
    Path(Path),
    ConstExpr(ConstExpr),
    BinExpr(BinExpr),
    UniExpr(UniExpr),
    FuncPath(FuncPath),
    CondExpr(CondExpr),
    ListPath(ListPath),
}

#[enum_dispatch::enum_dispatch(Evaluate)]
impl Evaluate for CombinedExpr {}
```

### 4.3 字段类型变更（`Box<dyn Evaluate>` → `Box<CombinedExpr>`）

| 结构体 | 字段 | 旧类型 | 新类型 | 源码位置 |
|--------|------|--------|--------|---------|
| `BinExpr` | `left` | `Box<dyn Evaluate>` | `Box<CombinedExpr>` | `expr.rs:320` |
| `BinExpr` | `right` | `Box<dyn Evaluate>` | `Box<CombinedExpr>` | `expr.rs:322` |
| `UniExpr` | `inner` | `Box<dyn Evaluate>` | `Box<CombinedExpr>` | `expr.rs:682` |
| `FuncPath` | `inner` | `Box<dyn Evaluate>` | `Box<CombinedExpr>` | `expr.rs:764` |
| `CondExpr` | `cond` | `Box<dyn Evaluate>` | `Box<CombinedExpr>` | `expr.rs:915` |
| `CondExpr` | `then_val` | `Box<dyn Evaluate>` | `Box<CombinedExpr>` | `expr.rs:917` |
| `CondExpr` | `else_val` | `Box<dyn Evaluate>` | `Box<CombinedExpr>` | `expr.rs:919` |
| `ListPath` | `inner` | `Option<Box<dyn Evaluate>>` | `Option<Box<CombinedExpr>>` | `expr.rs:988` |

> **保留 Box**：表达式树是递归类型（BinExpr 含 left/right 子表达式），必须 Box。

### 4.4 构造器中的表达式字段（`Box<dyn Evaluate>` → `Box<CombinedExpr>`）

| 结构体 | 字段 | 旧类型 | 新类型 | 源码位置 |
|--------|------|--------|--------|---------|
| `BytesExpr` | `length_expr` | `Box<dyn Evaluate>` | `Box<CombinedExpr>` | `bytes.rs:224` |
| `ArrayExpr` | `count_expr` | `Box<dyn Evaluate>` | `Box<CombinedExpr>` | `repetition.rs:217` |
| `SeekExpr` | `at_expr` | `Box<dyn Evaluate>` | `Box<CombinedExpr>` | `meta.rs:412` |
| `PointerExpr` | `offset_expr` | `Box<dyn Evaluate>` | `Box<CombinedExpr>` | `stream_ops.rs:336` |

### 4.5 IntoExpr trait 变更

```rust
// 旧（expr.rs:1119-1122）
pub trait IntoExpr: 'static {
    fn into_expr(self) -> Box<dyn Evaluate>;
}

// 新
pub trait IntoExpr: 'static {
    fn into_expr(self) -> Box<CombinedExpr>;
}
```

所有 `IntoExpr` 实现块（`usize`, `i64`, `u64`, `Path`, `ListPath`, `Box<dyn Evaluate>`）需更新
返回类型。最后一个实现 `impl IntoExpr for Box<dyn Evaluate>` 改为
`impl IntoExpr for Box<CombinedExpr>`。

> **builder 方法**：`BinExpr::new<L, R>` / `UniExpr::new<E>` / `FuncPath::new<E>` /
> `CondExpr::new<C,T,E>` 的泛型约束 `L: Evaluate + 'static` 保持不变（接受任何具体实现者），
> 但内部 `Box::new(left)` 改为 `Box::new(CombinedExpr::from(left))`（需 `From` 实现，见 §2.4）。
>
> **Path 的运算符方法**（`add`/`sub`/...，`expr.rs:1230+`）返回 `BinExpr`，其字段已是
> `Box<CombinedExpr>`，无需改动返回类型——但 `BinExpr::new` 内部构造方式需更新。

### 4.6 便利构造函数

`this_()` / `obj_()` / `const_val()` 返回具体类型（`Path` / `ConstExpr`），**类型不变**，
调用方按需 `.into()` 转为 `CombinedExpr`。

---

## 5. 迁移策略

### 5.1 Cargo.toml 变更

新增 `enum_dispatch` 依赖（编译期宏，无运行时开销）：

```toml
[dependencies]
thiserror = "2"
indexmap = "2"
enum_dispatch = "0.3"
```

> `enum_dispatch` 是 `proc-macro` crate，仅在编译期展开，不增加二进制体积。

### 5.2 迁移顺序（三步，一次性提交）

由于 `&mut dyn Stream` 与 `Construct` 签名耦合，且中间状态无法编译，**必须按以下顺序在一次
CODING 阶段内全部完成**，分步如下（每步内部可拆为多个文件提交，但整体必须在一个 CODING
子任务内收敛到可编译）：

```
步骤 1：Stream（基础设施，被其他步骤依赖）
  ├─ Cargo.toml 加 enum_dispatch
  ├─ core/stream.rs: 定义 CombinedStream enum + #[enum_dispatch]
  ├─ 全局替换 &mut dyn Stream → &mut CombinedStream（Construct trait + 所有 impl + 辅助函数）
  └─ 此刻项目应可编译（Stream 只有 2 个实现者，变更面小）

步骤 2：Evaluate（独立于 Construct，先做可降低 Construct 步骤的复杂度）
  ├─ expr.rs: 定义 CombinedExpr enum + #[enum_dispatch]
  ├─ expr.rs: BinExpr/UniExpr/FuncPath/CondExpr/ListPath 字段 + IntoExpr 返回类型
  ├─ constructs: BytesExpr/ArrayExpr/SeekExpr/PointerExpr 的 *_expr 字段
  └─ 此刻项目应可编译

步骤 3：Construct（影响面最大，最后做）
  ├─ combined.rs: 定义 CombinedConstruct enum + #[enum_dispatch]
  ├─ core/mod.rs: Subconstruct/Renamed 字段 + Renamed::new 签名
  ├─ constructs/*: 所有含 subcon/inner 字段的结构体（见 §2.3 表）
  ├─ 所有 builder 方法签名（new/field/push/anonymous）
  ├─ 所有构造调用点（Box::new(Xxx) → Box::new(CombinedConstruct::Xxx(...))）
  └─ 便利方法 impl 转移到 impl CombinedConstruct
```

> **为什么不能分多次 CODING 验收**：`Box<dyn Construct>` 与 `Box<CombinedConstruct>` 类型不兼容，
> 中间状态会大面积编译失败。三个 enum 必须在单次 CODING 任务内全部落地，由 DEV 一次性提交。

### 5.3 编译策略

迁移期间项目**无法保持逐文件可编译**——这是"大爆炸"式重构。但可通过以下手段控制风险：

1. **先写 enum 定义，再做字段替换**：先落地三个 enum（§2.2/§3.2/§4.2），此时项目有重复实现
   （既有 `impl Construct for Xxx` 又有 `impl Construct for CombinedConstruct`），编译会有"未使用"
   警告，但不报错。
2. **字段替换用全局搜索替换**：`Box<dyn Construct>` → `Box<CombinedConstruct>` 是机械操作。
3. **构造调用点最后改**：把所有 `Box::new(Xxx)` → `Box::new(CombinedConstruct::Xxx(...))`。
4. **`cargo check` 频繁验证**：每完成一个模块的替换就 `cargo check`，定位错误。

### 5.4 测试代码迁移

测试中的 mock 构造器和类型标注需同步迁移：

| 测试模式 | 旧 | 新 |
|---------|-----|-----|
| `let c: &dyn Construct = &U32Big` | `&dyn Construct` | `&CombinedConstruct`（或保留 `&dyn Construct`，因 CombinedConstruct impl 了 Construct） |
| `Subconstruct { subcon: Box::new(U32Big) }` | `Box::new(U32Big)` | `Box::new(CombinedConstruct::from(U32Big))` 或将 U32Big 等测试 mock 也纳入 enum |
| `as Box<dyn Construct>` 显式标注 | `as Box<dyn Construct>` | 删除标注，或 `as Box<CombinedConstruct>` |

> **测试 mock 处理决策**：测试中的私有 mock 构造器（如 `U32Big`/`FailingConstruct`/`VarBytes`）
> 不纳入 `CombinedConstruct`（它们是测试局部类型）。这些测试需调整：要么 mock 直接作为
> `CombinedConstruct` 的测试专用 variant（不推荐，污染核心 enum），要么测试改为直接调用
> trait 方法（不经过 `Box<CombinedConstruct>` 字段）。
>
> **推荐**：测试中的 mock 保留为实现 `Construct` 的独立类型，测试直接调用其 `parse`/`build`
> 方法（不装箱）。需要装箱的测试改用内置构造器（如 `INT8UB`）替代 mock。`core/mod.rs` 的
> `Subconstruct`/`Renamed` 单元测试改用 `INT8UB` 等真实构造器。

---

## 6. 边界情况与风险

### 6.1 enum 大小评估（关键风险）

`CombinedConstruct` 有 70 个 variant，其大小 = 最大 variant 的大小（Rust enum 内存模型）。

- **最大 variant 候选**：含较多字段的结构体，如 `Switch`（cases: Vec + default + keyfunc）、
  `Transformed`/`Restreamed`（含多个闭包）、`Struct`（fields: Vec<StructField>）。
- **字段类型**：闭包（`Box<dyn Fn>`，指针大小）、`String`、`Vec`、`Box` 均为指针大小（8 字节）。
  含 ~5-8 个字段的结构体约 40-64 字节。
- **`Box<CombinedConstruct>` 的间接**：所有字段是 `Box<CombinedConstruct>`（指针，8 字节），
  不直接内联子构造器，因此 enum 大小不会因嵌套递归膨胀。enum 本体约 64 字节 + 1 字节 tag（对齐到 72-80 字节）。
- **结论**：enum 大小可接受（~80 字节），`Box<CombinedConstruct>` 是 8 字节指针。**无需对内部
  variant 再加 Box**（子 variant 内部已是 Box 指针）。
- **若 DEV 实测发现 enum 过大**（如 >128 字节）：对最大的 1-2 个 variant 内部字段加 Box。
  但根据字段分析，预期不需要。

### 6.2 Compressed 的 feature gate 处理

`Compressed` variant 受 `#[cfg(feature = "compression")]` 门控：

```rust
#[cfg(feature = "compression")]
Compressed(constructs::stream_ops::Compressed),
```

`enum_dispatch` 宏**支持 cfg-gated variant**。当未启用 `compression` feature 时，enum 为 69 个
variant，生成的 match 不含 Compressed 分支。需确保：
- `cargo build`（默认）通过：69 variant
- `cargo build --features compression` 通过：70 variant
- `cargo test --features compression` 通过

> DEV 验收时必须运行两种 feature 组合的 `cargo build` + `cargo test`。

### 6.3 gallery 构造器迁移

`gallery/elf.rs` 和 `gallery/pe32coff.rs` 的工厂函数（`elf()`、`pe32file()`、`int_field()`）
当前返回 `Box<dyn Construct>`：

```rust
// 旧
pub fn elf() -> Box<dyn Construct> { ... }
// 新
pub fn elf() -> Box<CombinedConstruct> { ... }   // 或 CombinedConstruct（不 Box）
```

这些函数内部用内置构造器组合（Struct/Bytes/Array...），组合后产生的 `Box<CombinedConstruct>`
可直接返回。**gallery 的 `UTIndex`（`gallery/ut_index.rs:101`）是自定义 Construct 实现者**：

- **决策**：UTIndex **不纳入** `CombinedConstruct`（它在 gallery 层，纳入会让核心 enum 依赖
  gallery 模块，造成循环依赖）。
- **处理**：UTIndex 作为"用户自定义 Construct"的示例，保持 `impl Construct for UTIndex`。
  如果它需要被装箱进 `Box<CombinedConstruct>` 字段，则 gallery 需通过其他方式（如外层 Adapter
  包装为 `Box<dyn Construct>`，或后续阶段提供"用户自定义" escape hatch variant）。
- **Phase 11 范围**：gallery 工厂函数的返回类型迁移；UTIndex 本身的 impl 保持不变（它不被
  装箱进任何 `Box<CombinedConstruct>` 字段，仅作为顶层 schema 使用）。

> 若 DEV 发现 UTIndex 必须进入 CombinedConstruct 字段，提出 Argue，ARCH 评估是否新增
> `Custom(Box<dyn Construct>)` escape-hatch variant（见 §6.6）。

### 6.4 不受影响的类型（确认排除）

| 类型 | 原因 | 处理 |
|------|------|------|
| `RepeatPredicate`（repetition.rs） | 非 Construct/Evaluate 实现者，是枚举/闭包类型 | 不受影响 |
| `CondFunc`/`KeyFunc`/`CheckFunc`/`DecodeFunc` 等闭包类型别名 | `Box<dyn Fn>`，非 trait object 分发目标 | 不受影响 |
| `ParsedHook`（`Box<dyn Fn(&Value, &Context)>`） | 闭包类型别名 | 不受影响 |
| `ComputeFunc`/`CompressionAlgorithm`/`TimestampUnit` 等 | 配置类型 | 不受影响 |
| `StructField`/`SeqEntry` | 不 impl Construct（仅持有 subcon） | 字段类型变更（§2.3），但不作为 variant |

### 6.5 `Send + Sync` 约束

`Evaluate: Send + Sync`（`expr.rs:59`）。`CombinedExpr` 的所有 variant（Path/ConstExpr/...）
需满足 `Send + Sync`。当前实现者均满足（字段为 Value/String/Box，均 Send+Sync）。
`CombinedConstruct` 无显式 Send+Sync 约束（Construct trait 未要求），但若 Phase 12 需要跨线程
缓存 CompiledSchema，需验证。**本次迁移不引入新约束**。

### 6.6 escape-hatch variant（备选方案）

若未来（Phase 12+）需要支持用户自定义 Construct 进入执行树，可新增 escape-hatch variant：

```rust
// 备选，本次不实现
Custom(Box<dyn Construct>),  // 退化为 dyn 分发，但兼容任意用户实现
```

这会重新引入一处虚分发，但仅限用户自定义节点（性能影响可忽略）。**Phase 11 不实现**，记录为
后续阶段的可选扩展。

---

## 7. REV 问题回应

### 7.1 问题 #1：D1（分发机制）与 D2（Value 保留）的耦合

**质疑**：CombinedConstruct 是否等同于 CompiledNode？迁移是否会改变 parse/build 的返回类型？

**回应**：**确认设计正确，两者完全不同。**

| 维度 | CombinedConstruct（Phase 11） | CompiledNode（Phase 12） |
|------|------------------------------|--------------------------|
| 本质 | 分发机制替换（dyn → enum） | 全新执行树模型 |
| 返回类型 | `Value`（不变） | direct-to-target（PyDict/dataclass） |
| Context | 原 Context（不变） | 惰性 PyContextView + Rust 闭包 |
| 表达式 | 原 `Box<dyn Evaluate>`（迁移为 CombinedExpr） | 编译为 CompiledExpr（含 PyCallback） |
| 语义变更 | **零**（纯机械迁移） | 重大（执行模型重构） |

Phase 11 的唯一目标：把 `Box<dyn Construct>` 的虚函数表跳转换成 `CombinedConstruct` 的 match
分发，让编译器能内联。parse 仍构建 `Value`，build 仍消费 `Value`。这是 Phase 12 编译执行树的
**零开销基础**——Phase 12 会在 CombinedConstruct 之上构建 CompiledNode，而非替换它。

**文档已更新**：§1.3 明确排除 CompiledNode，§1.3 末尾的"REV 问题 #1 回应"框已说明此点。

---

## 8. 验收检查清单（DEV 自检 / VET 审查用）

- [ ] `Cargo.toml` 新增 `enum_dispatch = "0.3"`
- [ ] `combined.rs` 定义 `CombinedConstruct`（70 variant，Compressed cfg-gated）
- [ ] `core/stream.rs` 定义 `CombinedStream`（2 variant）
- [ ] `expr.rs` 定义 `CombinedExpr`（7 variant）
- [ ] 全局无残留 `Box<dyn Construct>`（核心 src，除 escape-hatch 外）
- [ ] 全局无残留 `&mut dyn Stream`（核心 src）
- [ ] 全局无残留 `Box<dyn Evaluate>`（核心 src）
- [ ] `cargo build` 通过（默认 feature）
- [ ] `cargo build --features compression` 通过
- [ ] `cargo build --all-features` 通过
- [ ] `cargo clippy` 零 warning（含 `--all-features`）
- [ ] `cargo fmt --check` 通过
- [ ] `cargo test` 全部 PASS（默认 feature）
- [ ] `cargo test --features compression` 全部 PASS
- [ ] `cargo test --all-features` 全部 PASS
- [ ] gallery 工厂函数返回类型已迁移
- [ ] 测试中 mock 构造器已调整（不装箱或改用内置构造器）




