//! Node 系统：Construct trait 定义与 Node 枚举（enum_dispatch 静态分派）。
//!
//! ## 概述
//!
//! 执行树的每个节点实现统一的 [`Construct`] trait，提供 `parse` / `build` / `sizeof`
//! 三个方法。节点类型用封闭 [`Node`] 枚举列举，通过 `enum_dispatch` 宏自动生成
//! `match` 分派代码，无 `Box<dyn>` 动态分派开销。
//!
//! ## 节点分类
//!
//! - 原子节点：[`FormatFieldNode`](format_field::FormatFieldNode)（整数读写）、
//!   [`BytesNode`](bytes::BytesNode)（固定/表达式长度字节）、
//!   [`GreedyBytesNode`](greedy_bytes::GreedyBytesNode)（剩余字节）、
//!   [`BitsIntegerNode`](bits_integer::BitsIntegerNode)（bit 级整数）
//! - 复合节点：[`StructNode`](struct_node::StructNode)（字段序列根节点）、
//!   [`StructRefNode`](struct_ref::StructRefNode)（嵌套引用其他 StructMixin 子类）、
//!   [`BitwiseNode`](bitwise::BitwiseNode)（bit 域包装器）、
//!   [`BytewiseNode`](bytewise::BytewiseNode)（bit→byte 适配器）、
//!   [`TransformNode`](transform::TransformNode)（字节级变换）
//! - RO 节点：[`TellNode`](tell::TellNode)（流位置）、
//!   [`ComputedNode`](computed::ComputedNode)（表达式计算值）
//! - 填充节点：[`BitPaddingNode`](bit_padding::BitPaddingNode)（bit 级填充）、
//!   [`PaddingNode`](padding::PaddingNode)（字节级填充）

pub mod adapter_callback;
pub mod aligned;
pub mod array;
pub mod bit_padding;
pub mod bits_integer;
pub mod bitwise;
pub mod bytes;
pub mod bytes_integer;
pub mod bytewise;
pub mod check_node;
pub mod checksum;
pub mod common;
pub mod computed;
pub mod const_node;
pub mod default_node;
pub mod element;
pub mod enum_node;
pub mod flags_enum;
pub mod focused_seq;
pub mod format_field;
pub mod greedy_bytes;
pub mod greedy_range;
pub mod hex;
pub mod hex_dump;
pub mod if_then_else;
pub mod index;
pub mod mapping;
pub mod named_tuple;
pub mod one_of;
pub mod padding;
pub mod pass;
pub mod peek;
pub mod pointer;
pub mod prefixed;
pub mod prefixed_array;
pub mod probe;
pub mod process_rotate_left;
pub mod process_xor;
pub mod raw_copy;
pub mod rebuild;
pub mod repeat_until;
pub mod seek;
pub mod select;
pub mod sequence;
pub mod stop_if;
pub mod strings;
pub mod struct_node;
pub mod struct_ref;
pub mod subconstruct;
pub mod switch;
pub mod tell;
pub mod terminated;
pub mod transform;
pub mod union;
pub mod varint;
pub mod zigzag;

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use adapter_callback::AdapterCallbackNode;
use aligned::AlignedNode;
use array::ArrayNode;
use bit_padding::BitPaddingNode;
use bits_integer::BitsIntegerNode;
use bitwise::BitwiseNode;
use bytes::BytesNode;
pub use bytes_integer::BytesIntegerNode;
use bytewise::BytewiseNode;
use check_node::CheckNode;
use checksum::ChecksumNode;
use computed::ComputedNode;
use const_node::ConstNode;
use default_node::DefaultNode;
use element::ElementNode;
use enum_dispatch::enum_dispatch;
use enum_node::EnumNode;
use flags_enum::FlagsEnumNode;
use focused_seq::FocusedSeqNode;
use format_field::FormatFieldNode;
use greedy_bytes::GreedyBytesNode;
use greedy_range::GreedyRangeNode;
use hex::HexNode;
use hex_dump::HexDumpNode;
use if_then_else::IfThenElseNode;
use index::IndexNode;
use mapping::MappingNode;
use named_tuple::NamedTupleNode;
use one_of::{NoneOfNode, OneOfNode};
use padding::PaddingNode;
use pass::PassNode;
use peek::PeekNode;
use pointer::PointerNode;
use prefixed::PrefixedNode;
use prefixed_array::PrefixedArrayNode;
use probe::ProbeNode;
use process_rotate_left::ProcessRotateLeftNode;
use process_xor::{ProcessXorNode, XorPad};
use pyo3::prelude::*;
use raw_copy::RawCopyNode;
use rebuild::RebuildNode;
use repeat_until::RepeatUntilNode;
use seek::SeekNode;
use select::SelectNode;
use sequence::SequenceNode;
use stop_if::StopIfNode;
use struct_node::StructNode;
use struct_ref::StructRefNode;
use subconstruct::SubconstructNode;
use switch::SwitchNode;
use tell::TellNode;
use terminated::TerminatedNode;
use transform::TransformNode;
use union::UnionNode;
pub use varint::VarIntNode;
pub use zigzag::ZigZagNode;

/// 所有构造器节点实现的统一接口。
///
/// parse 直接产出 Python 对象（`Py<PyAny>`），build 直接读取 Python 对象
/// （`&Bound<PyAny>`）——不经过任何 Rust 中间类型。
///
/// 每个具体节点类型实现此 trait，然后通过 `enum_dispatch` 在 [`Node`] 枚举上
/// 自动生成静态分派的 `match` 代码。
///
/// # enum_dispatch 用法说明
///
/// `#[enum_dispatch]` 必须同时标注在 trait 和 enum 上：trait 上的标注让宏缓存
/// trait 定义，enum 上的 `#[enum_dispatch(Construct)]` 引用缓存生成 match 分派。
/// 若 trait 上缺少标注，enum 端的宏会静默不生成 impl（无编译错误，但运行时分派失败）。
#[enum_dispatch]
pub trait Construct {
    /// 从流中解析，直接构造并返回 Python 对象。
    ///
    /// # 参数
    ///
    /// - `py`：pyo3 GIL token，持有 GIL 才能调用 C API
    /// - `stream`：字节流游标（纯 Rust，可读写位置）
    /// - `ctx`：当前上下文（持有 Python dict 引用，供字段间引用）
    /// - `path`：错误追踪路径栈（如 `"root.header.flags"`），Struct 节点进入字段时 push
    ///
    /// # 生命周期
    ///
    /// `py` 与 `ctx` 共享同一个生命周期 `'py`——两者均绑定到调用方的 GIL scope。
    /// 这允许 StructNode.parse 将实例 `__dict__`（来自 `py` 的 GIL scope）
    /// 注入到 ctx 中。
    ///
    /// # 返回
    ///
    /// 成功返回 `Py<PyAny>`（Python 对象引用），失败返回 [`ConstructError`]。
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError>;

    /// 从 Python 对象构建，将字节写入流。
    ///
    /// # 参数
    ///
    /// - `py`：pyo3 GIL token
    /// - `obj`：待构建的 Python 对象（用户实例或标量值）
    /// - `stream`：输出缓冲
    /// - `ctx`：当前上下文
    /// - `path`：错误追踪路径栈
    ///
    /// # 返回
    ///
    /// 成功返回 `Ok(())`，失败返回 [`ConstructError`]。
    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError>;

    /// 静态大小（字节）。
    ///
    /// 无法预知大小时返回 `Err`（对应 Python construct 的 `SizeofError`）。
    ///
    /// # 参数
    ///
    /// - `ctx`：当前上下文（某些节点的大小可能依赖上下文字段）
    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError>;
}

/// 执行树节点：封闭枚举，编译期类型集合固定。
///
/// 使用 `enum_dispatch` 宏自动为 `Node` 生成 [`Construct`] trait 的实现，
/// 通过 `match` 静态分派到具体节点类型，无 `Box<dyn>` 动态分派开销。
///
/// 新增节点类型需在此枚举添加变体。
///
/// # 变体分组
///
/// - 原子节点：`FormatField`、`Bytes`、`GreedyBytes`、`BitsInteger`
/// - 复合节点：`Struct`（字段序列）、`StructRef`（嵌套引用）、
///   `Bitwise`（bit 域包装器，首个递归 `Box<Node>` 变体）、
///   `Bytewise`（bit→byte 适配器，递归 `Box<Node>`）、
///   `Transform`（字节级变换，递归 `Box<Node>`）
/// - RO 节点：`Tell`（流位置）、`Computed`（表达式计算值）
/// - 填充节点：`BitPadding`（bit 级）、`Padding`（字节级）
#[derive(Debug)]
#[enum_dispatch(Construct)]
pub enum Node {
    /// 整数读写节点：`Int8ub` / `Int16ul` / ... （对应 Python construct `FormatField`）
    FormatField(FormatFieldNode),
    /// 固定/表达式长度字节读写节点：`Bytes(n)` / `Bytes(count)` （对应 Python construct `Bytes`）
    Bytes(BytesNode),
    /// 剩余字节读写节点：`GreedyBytes` （对应 Python construct `GreedyBytes`）
    GreedyBytes(GreedyBytesNode),
    /// 字段序列根节点：`StructMixin` 子类的根（对应 Python construct `Struct`）
    Struct(StructNode),
    /// 嵌套引用节点：引用另一个 `StructMixin` 子类的执行树
    StructRef(StructRefNode),
    /// RO 节点：记录当前流位置（对应 Python construct `Tell`）
    Tell(TellNode),
    /// RO 节点：从表达式计算值（对应 Python construct `Computed`）
    Computed(ComputedNode),
    /// bit 级整数节点：`BitsInteger` / `Bit` / `Nibble` / `Octet`
    /// （对应 Python construct `BitsInteger`，必须在 `Bitwise` 域内使用）
    BitsInteger(BitsIntegerNode),
    /// bit 域包装器节点：建立 bit 级游标边界，结束时校验对齐
    /// （对应 Python construct `Bitwise` / `BitStruct`）。
    ///
    /// 首个递归 Node 变体——`inner: Box<Node>` 打破 enum 的无限大小。
    /// enum_dispatch 仍正常工作（dispatch 到 BitwiseNode::parse，内部解引用 Box）。
    Bitwise(BitwiseNode),
    /// bit→byte 适配器节点：在 bit 域内为内部 subcon 重建字节对齐的子流
    /// （对应 Python construct `Bytewise`，递归 `Box<Node>`）。
    Bytewise(BytewiseNode),
    /// 字节级变换节点：`BitsSwapped` / `ByteSwapped`
    /// （对应 Python construct `Transformed` 的两种变换，递归 `Box<Node>`）。
    Transform(TransformNode),
    /// bit 级填充节点：`Padding` 在 Bitwise 域内的行为
    /// （pattern 严格校验 0x00/0x01）。
    BitPadding(BitPaddingNode),
    /// 字节级填充节点：`Padding` 在普通 Struct 域内的行为
    /// （对应 Python construct `Padding` 字节域用法）。
    Padding(PaddingNode),
    /// 固定次数数组节点（对应 Python construct `Array(count, subcon, discard)`）。
    /// inner 用 `Box<Node>`，count 可为常量或 ExprProgram。
    Array(ArrayNode),
    /// 读到流结束的数组节点（对应 Python construct `GreedyRange(subcon, discard)`）。
    /// inner 用 `Box<Node>`，parse 直到流末尾或子构造器失败。
    GreedyRange(GreedyRangeNode),
    /// 前缀长度数组节点（对应 Python construct `PrefixedArray(countfield, subcon)`）。
    /// countfield 与 inner 都用 `Box<Node>`；parse 先解析 countfield
    /// 得到 count，再循环 count 次 inner.parse；build 先取 list 长度 build countfield，
    /// 再遍历 list build inner。
    PrefixedArray(PrefixedArrayNode),
    /// 终止表达式数组节点（对应 Python construct `RepeatUntil(predicate, subcon, discard)`）。
    /// inner 用 `Box<Node>`，终止表达式为 ExprProgram（编译期从
    /// 用户面表达式编译，运行时零 FFI）。parse/build 循环直到终止表达式求值非零
    /// （最后元素包含在内）；build 无元素满足则 `Repeat` 错误。sizeof 永远 Err。
    RepeatUntil(RepeatUntilNode),
    /// 取当前数组迭代下标的节点（对应 Python construct `Index`）。
    /// 直接调 `ctx.index()` 读取，不走 ExprProgram。
    /// parse 返回 PyLong 或 Py_None；build 是 no-op；sizeof=0。
    Index(IndexNode),
    /// 早停信号节点（对应 Python construct `StopIf(condfunc)`）。
    /// 条件为真时返回 `ConstructError::StopField` 哨兵，
    /// 被外层 StructNode / GreedyRangeNode 捕获，停止后续字段/迭代。
    /// 条件可为编译期常量（Always / Never）或表达式（Expr）。
    StopIf(StopIfNode),
    /// RepeatUntil 当前元素引用入口节点（Python construct 无对应物）。
    /// parse 始终返回 Py_None，值由
    /// RepeatUntilNode 在迭代时 set_expr_value_raw/set_expr_value_py 借用设置；build 是 no-op；
    /// sizeof 返回 0；has_expressions 返回 false。
    Element(ElementNode),
    /// 单子构造器包装节点（对应 Python Subconstruct）。
    /// 持有 `Box<Node>`，parse/build/sizeof 全部转发给 inner。
    Subconstruct(SubconstructNode),
    /// 预读不消费流节点（对应 Python Peek）。
    /// parse 后 seek 回入口位置；build 是 no-op；sizeof 返回 0。
    Peek(PeekNode),
    /// 原始字节捕获节点（对应 Python RawCopy）。
    /// parse 返回 dict(data,value,offset1,offset2,length)。
    RawCopy(RawCopyNode),
    /// build 时基于表达式重算字段节点（对应 Python Rebuild）。
    /// 必须作为 RO 字段使用（与 Computed 同类）。build 求值表达式后调 inner.build。
    Rebuild(RebuildNode),
    /// No-op 节点（对应 Python Pass）。
    /// parse 返回 Py_None；build 不写字节；sizeof 返回 0。
    Pass(PassNode),
    /// 用户面 Adapter 嵌入 Struct 字段的钩子节点。
    /// subcon 在 Rust 内执行，_decode/_encode 通过 Rust→Python 回调。
    AdapterCallback(AdapterCallbackNode),
    /// LEB128 无符号变长整数（对应 Python construct `VarInt`）。
    /// fast-path u64 范围；slow-path（>2^64）调缓存 Python 函数。
    VarInt(VarIntNode),
    /// 有符号变长整数（对应 Python construct `ZigZag`）。
    /// ZigZag 数值变换后复用 VarIntNode 字节编解码。
    ZigZag(ZigZagNode),
    /// 任意字节长度整数（对应 Python construct `BytesInteger`）。
    /// fast-path（≤8 字节）Rust 原生 u64/i64；slow-path（>8 字节）调 Python int.from_bytes。
    /// Int24ub/ul/sb/sl 是 length=3 的 Python 层别名。
    BytesInteger(BytesIntegerNode),
    // === Strings ===
    /// C 风格 null 终止字符串。
    CString(crate::nodes::strings::CStringNode),
    /// 贪婪字符串：读到 EOF + decode。
    GreedyString(crate::nodes::strings::GreedyStringNode),
    /// 固定长度填充字符串。
    PaddedString(crate::nodes::strings::PaddedStringNode),
    /// 长度前缀字符串。
    PascalString(crate::nodes::strings::PascalStringNode),
    /// null 终止包装器（持有任意 inner）。
    NullTerminated(crate::nodes::strings::NullTerminatedNode),
    /// null 剥离包装器（持有任意 inner）。
    NullStripped(crate::nodes::strings::NullStrippedNode),
    // === Conditional ===
    /// 双分支条件节点（对应 Python construct `IfThenElse`）。
    /// 条件可为常量或 ExprProgram；then/else 都是 `Box<Node>`。
    IfThenElse(IfThenElseNode),
    /// 多分支条件节点（对应 Python construct `Switch`）。
    /// keyfunc 走 ExprProgram(i64) + FieldRef(PyObject) 混合。
    Switch(SwitchNode),
    /// 多分支尝试节点（对应 Python construct `Select`）。
    /// 遍历 subcons，首个成功者胜出；ExplicitError 穿透。
    Select(SelectNode),
    /// 聚焦字段序列节点（对应 Python construct `FocusedSeq`）。
    /// context nesting + 返回单聚焦字段值。
    FocusedSeq(FocusedSeqNode),
    // === Streams ===
    /// 流定位节点（对应 Python construct `Seek`）。
    /// parse/build 执行 stream.seek；sizeof 永远 Err。
    Seek(SeekNode),
    /// 绝对偏移读写节点（对应 Python construct `Pointer`）。
    /// seek 到 offset 处理 subcon 再 seek 回；sizeof 返回 0。
    Pointer(PointerNode),
    /// 长度前缀子流节点（对应 Python construct `Prefixed`）。
    /// lengthfield 给出字节数，subcon 在子流上处理。
    Prefixed(PrefixedNode),
    /// 常量字段节点（对应 Python Const）。parse 校验 == value；build 用 value。
    Const(ConstNode),
    /// 默认值字段节点（对应 Python Default）。build obj=None 时用 value 表达式。
    Default(DefaultNode),
    /// 断言检查节点（对应 Python Check）。必须作为 RO 字段使用。
    Check(CheckNode),
    /// 对齐包装节点（对应 Python Aligned）。填充字节到 modulus 的整数倍。
    Aligned(AlignedNode),
    /// Hex 显示包装节点（对应 Python Hex，Rust Node 非 AdapterCallback）。
    Hex(HexNode),
    /// HexDump 显示包装节点（对应 Python HexDump）。
    HexDump(HexDumpNode),
    /// 校验和节点（对应 Python Checksum，双轨 hashfunc + StreamRange）。
    Checksum(ChecksumNode),
    /// EOF 断言节点（对应 Python Terminated）。
    Terminated(TerminatedNode),
    /// 调试探针节点（对应 Python Probe）。
    /// into 字段用 FieldName 而非 ExprProgram（详见 probe.rs 模块级注释）。
    Probe(ProbeNode),
    /// 枚举映射节点（对应 Python Enum，Rust Node 非 AdapterCallback）。
    /// decmapping/encmapping 在编译期物化为 Py<PyDict>。
    Enum(EnumNode),
    /// 标志位枚举节点（对应 Python FlagsEnum）。
    /// flags 物化为 Vec<(Py<PyString>, i64)>，运行时遍历构造 dict。
    FlagsEnum(FlagsEnumNode),
    /// 通用对象映射节点（对应 Python Mapping，无映射时报错）。
    /// key/value 任意 hashable，与 EnumNode 结构同但语义不同。
    Mapping(MappingNode),
    /// 单值校验节点（对应 Python OneOf）。
    /// valids 编译期物化为 Py<PyFrozenSet>，C API 查询不计 FFI。
    OneOf(OneOfNode),
    /// 排除值校验节点（对应 Python NoneOf）。
    /// 与 OneOf 结构同，校验取反。
    NoneOf(NoneOfNode),
    /// 联合体节点（对应 Python Union，多视角 parse）。
    /// 含 ParseFrom 策略（None/Index/Name/Expr）+ context nesting。
    Union(UnionNode),
    /// 位置序字段序列节点（对应 Python Sequence）。
    /// parse 产出 PyList；build 接收 list（RO 字段不从 list 取值）。
    Sequence(SequenceNode),
    /// XOR 字节变换节点（对应 Python ProcessXor）。
    /// XorPad enum（Int/Bytes/Expr）含 fast-path（pad==0/全零不变换）。
    ProcessXor(ProcessXorNode),
    /// 位旋转左移节点（对应 Python ProcessRotateLeft）。
    /// ROTATION_TABLES const 表 + 4 分支位运算。
    ProcessRotateLeft(ProcessRotateLeftNode),
    /// NamedTuple 包装节点（对应 Python NamedTuple）。
    /// factory 编译期物化为 Py<PyType>（collections.namedtuple）。
    NamedTuple(NamedTupleNode),
}

impl Node {
    /// 判断此节点（或其子树）是否含有表达式字段。
    ///
    /// 当前仅 [`Node::Struct`] 携带 `has_expressions` 标志（编译期计算），
    /// [`Node::Bitwise`] / [`Node::Bytewise`] / [`Node::Transform`] 递归检查内部子树。
    /// 其他节点变体（原子节点、`StructRef`、`Tell`、`Computed`、填充节点）返回 `false`。
    ///
    /// `struct_ref.rs` 与 `schema.rs` 通过此方法判断内层/根节点是否含表达式，
    /// 决定是否创建带 `PyDict` 的 context（避免重复 `matches!(root, Node::Struct(s) if ...)`）。
    pub fn has_expressions(&self) -> bool {
        match self {
            Node::Struct(s) => s.has_expressions(),
            Node::Bitwise(b) => b.inner().has_expressions(),
            Node::Bytewise(b) => b.inner().has_expressions(),
            Node::Transform(t) => t.inner().has_expressions(),
            Node::Array(a) => a.has_expressions(),
            Node::GreedyRange(g) => g.has_expressions(),
            Node::PrefixedArray(p) => p.has_expressions(),
            Node::RepeatUntil(r) => r.has_expressions(),
            Node::Index(i) => i.has_expressions(),
            Node::StopIf(s) => s.has_expressions(),
            Node::Element(e) => e.has_expressions(),
            // Subconstruct / Peek / RawCopy 递归检查 inner（与 Bitwise/Transform 同模式）。
            Node::Subconstruct(s) => s.inner().has_expressions(),
            Node::Peek(p) => p.inner().has_expressions(),
            Node::RawCopy(r) => r.inner().has_expressions(),
            // Rebuild 含 func 表达式，总返回 true。
            Node::Rebuild(_) => true,
            // Pass / AdapterCallback 无表达式字段（AdapterCallback 的 _decode/_encode
            // 是 Python 回调，不走 ExprProgram）。
            // Strings：
            // PaddedString 的 BytesLength::Expr 时返回 true；
            // PascalString / NullTerminated / NullStripped 递归检查 inner 子树；
            // CString / GreedyString 无表达式（encoding 是编译期 enum）。
            Node::PaddedString(p) => p.has_expressions(),
            Node::PascalString(p) => p.has_expressions(),
            Node::NullTerminated(n) => n.has_expressions(),
            Node::NullStripped(n) => n.has_expressions(),
            // Conditional 系列。
            // IfThenElse：仅 Expr 条件返回 true（与 StopIf 同模式）。
            // Switch：IntExpr / FieldRef 引用字段返回 true（ConstInt 返回 false）。
            // Select / FocusedSeq：递归检查子树（与 Bitwise / Transform 同模式）。
            Node::IfThenElse(i) => i.has_expressions(),
            Node::Switch(s) => s.has_expressions(),
            Node::Select(s) => s.has_expressions(),
            Node::FocusedSeq(f) => f.has_expressions(),
            // Streams 系列。
            // Seek：仅 Expr at 返回 true（与 StopIf 同模式）。
            // Pointer：Expr offset 或 subcon 含表达式。
            // Prefixed：lengthfield 或 subcon 含表达式（与 PrefixedArray 同模式）。
            Node::Seek(s) => s.has_expressions(),
            Node::Pointer(p) => p.has_expressions(),
            Node::Prefixed(p) => p.has_expressions(),
            // Const 递归检查 inner（与 Subconstruct 同模式）。
            Node::Const(c) => c.inner().has_expressions(),
            // Default 含 value 表达式，总返回 true（与 Rebuild 同模式）。
            Node::Default(_) => true,
            // Check 含 func 表达式，总返回 true（与 Computed 同模式）。
            Node::Check(_) => true,
            // Aligned 含 modulus 表达式。
            // - modulus 是编译期常量（Const）且 inner 无表达式 → false（恢复 static_size 预分配）
            // - modulus 是运行期表达式 或 inner 含表达式 → true
            Node::Aligned(a) => a.has_expressions(),
            // Hex/HexDump 递归检查 inner（与 Subconstruct 同模式）。
            Node::Hex(h) => h.inner().has_expressions(),
            Node::HexDump(h) => h.inner().has_expressions(),
            // Checksum 递归 checksumfield + 含 start/end 表达式（StreamRange），
            // 总返回 true（保守处理，与 Rebuild 同模式）。
            Node::Checksum(_) => true,
            // Terminated 是单元结构体，无表达式。
            Node::Terminated(_) => false,
            // Probe 含 into FieldName，引用 Struct 字段时返回 true（与 Switch FieldRef 同模式）。
            Node::Probe(p) => p.into_field().is_some(),
            // Enum/FlagsEnum/Mapping 递归检查 inner（与 Subconstruct 同模式）。
            Node::Enum(e) => e.inner().has_expressions(),
            Node::FlagsEnum(f) => f.inner().has_expressions(),
            Node::Mapping(m) => m.inner().has_expressions(),
            // OneOf/NoneOf 递归检查 inner。
            Node::OneOf(o) => o.inner().has_expressions(),
            Node::NoneOf(n) => n.inner().has_expressions(),
            // Union 编译期预算（与 FocusedSeq 同模式）。
            Node::Union(u) => u.has_expressions(),
            // Sequence 编译期预算（与 FocusedSeq 同模式）。
            Node::Sequence(s) => s.has_expressions(),
            // ProcessXor inner 子树 + 可能含 Expr pad。
            Node::ProcessXor(p) => {
                p.inner().has_expressions() || matches!(p.pad(), XorPad::Expr(_))
            }
            // ProcessRotateLeft 恒含 amount/group 表达式。
            Node::ProcessRotateLeft(_) => true,
            // NamedTuple 递归检查 inner（与 Subconstruct 同模式）。
            Node::NamedTuple(n) => n.inner().has_expressions(),
            _ => false,
        }
    }
}

