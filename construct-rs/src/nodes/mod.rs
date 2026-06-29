//! Node 系统：Construct trait 定义与 Node 枚举（enum_dispatch 静态分派）。
//!
//! 设计依据：`docs/架构设计.md` §C.1（Construct trait）、§C.2（Node enum）、
//! `docs/模块设计-BitStream.md` §6。
//!
//! ## 概述
//!
//! 执行树的每个节点实现统一的 [`Construct`] trait，提供 `parse` / `build` / `sizeof`
//! 三个方法。节点类型用封闭 [`Node`] 枚举列举，通过 `enum_dispatch` 宏自动生成
//! `match` 分派代码，无 `Box<dyn>` 动态分派开销。
//!
//! ## 当前范围
//!
//! 当前含 13 个节点变体：
//! - 原子节点：[`FormatFieldNode`](format_field::FormatFieldNode)（整数读写）、
//!   [`BytesNode`](bytes::BytesNode)（固定/表达式长度字节）、
//!   [`GreedyBytesNode`](greedy_bytes::GreedyBytesNode)（剩余字节）、
//!   [`BitsIntegerNode`](bits_integer::BitsIntegerNode)（bit 级整数，Phase 3.1）
//! - 复合节点：[`StructNode`](struct_node::StructNode)（字段序列根节点）、
//!   [`StructRefNode`](struct_ref::StructRefNode)（嵌套引用其他 StructMixin 子类）、
//!   [`BitwiseNode`](bitwise::BitwiseNode)（bit 域包装器，Phase 3.2）、
//!   [`BytewiseNode`](bytewise::BytewiseNode)（bit→byte 适配器，Phase 3.3）、
//!   [`TransformNode`](transform::TransformNode)（字节级变换，Phase 3.3）
//! - RO 节点：[`TellNode`](tell::TellNode)（流位置）、
//!   [`ComputedNode`](computed::ComputedNode)（表达式计算值）
//! - 填充节点：[`BitPaddingNode`](bit_padding::BitPaddingNode)（bit 级填充，Phase 3.3）、
//!   [`PaddingNode`](padding::PaddingNode)（字节级填充，Phase 3.3）

pub mod array;
pub mod bit_padding;
pub mod bits_integer;
pub mod bitwise;
pub mod bytes;
pub mod bytewise;
pub mod computed;
pub mod format_field;
pub mod greedy_bytes;
pub mod greedy_range;
pub mod padding;
pub mod struct_node;
pub mod struct_ref;
pub mod tell;
pub mod transform;

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use array::ArrayNode;
use bit_padding::BitPaddingNode;
use bits_integer::BitsIntegerNode;
use bitwise::BitwiseNode;
use bytes::BytesNode;
use bytewise::BytewiseNode;
use computed::ComputedNode;
use enum_dispatch::enum_dispatch;
use format_field::FormatFieldNode;
use greedy_bytes::GreedyBytesNode;
use greedy_range::GreedyRangeNode;
use padding::PaddingNode;
use pyo3::prelude::*;
use struct_node::StructNode;
use struct_ref::StructRefNode;
use tell::TellNode;
use transform::TransformNode;

/// 所有构造器节点实现的统一接口。
///
/// parse 直接产出 Python 对象（`Py<PyAny>`），build 直接读取 Python 对象
/// （`&Bound<PyAny>`）——不经过任何 Rust 中间类型（FFI 设计 §5）。
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
    /// 注入到 ctx 中（R4 优化）。
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
/// 新增节点类型需在此枚举添加变体（Phase 2+ 扩展时修改）。
///
/// # 当前变体
///
/// - 4 个原子节点：`FormatField`、`Bytes`、`GreedyBytes`、`BitsInteger`（Phase 3.1）
/// - 5 个复合节点：`Struct`（字段序列）、`StructRef`（嵌套引用）、
///   `Bitwise`（bit 域包装器，Phase 3.2，首个递归 `Box<Node>` 变体）、
///   `Bytewise`（bit→byte 适配器，Phase 3.3，递归 `Box<Node>`）、
///   `Transform`（字节级变换，Phase 3.3，递归 `Box<Node>`）
/// - 2 个 RO 节点：`Tell`（流位置）、`Computed`（表达式计算值）
/// - 2 个填充节点：`BitPadding`（bit 级，Phase 3.3）、`Padding`（字节级，Phase 3.3）
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
    /// （对应 Python construct `BitsInteger`，必须在 `Bitwise` 域内使用，Phase 3.1）
    BitsInteger(BitsIntegerNode),
    /// bit 域包装器节点：建立 bit 级游标边界，结束时校验对齐
    /// （对应 Python construct `Bitwise` / `BitStruct`，Phase 3.2）。
    ///
    /// 首个递归 Node 变体——`inner: Box<Node>` 打破 enum 的无限大小。
    /// enum_dispatch 仍正常工作（dispatch 到 BitwiseNode::parse，内部解引用 Box）。
    Bitwise(BitwiseNode),
    /// bit→byte 适配器节点：在 bit 域内为内部 subcon 重建字节对齐的子流
    /// （对应 Python construct `Bytewise`，Phase 3.3，递归 `Box<Node>`）。
    Bytewise(BytewiseNode),
    /// 字节级变换节点：`BitsSwapped` / `ByteSwapped`
    /// （对应 Python construct `Transformed` 的两种变换，Phase 3.3，递归 `Box<Node>`）。
    Transform(TransformNode),
    /// bit 级填充节点：`Padding` 在 Bitwise 域内的行为
    /// （Phase 3.3，pattern 严格校验 0x00/0x01）。
    BitPadding(BitPaddingNode),
    /// 字节级填充节点：`Padding` 在普通 Struct 域内的行为
    /// （对应 Python construct `Padding` 字节域用法，Phase 3.3 补全 Phase 1 遗留）。
    Padding(PaddingNode),
    /// 固定次数数组节点（对应 Python construct `Array(count, subcon, discard)`）。
    /// Phase 4 新增。inner 用 `Box<Node>`，count 可为常量或 ExprProgram。
    Array(ArrayNode),
    /// 读到流结束的数组节点（对应 Python construct `GreedyRange(subcon, discard)`）。
    /// Phase 4 新增。inner 用 `Box<Node>`，parse 直到流末尾或子构造器失败。
    GreedyRange(GreedyRangeNode),
}

impl Node {
    /// 判断此节点（或其子树）是否含有表达式字段。
    ///
    /// 当前仅 [`Node::Struct`] 携带 `has_expressions` 标志（编译期计算），
    /// [`Node::Bitwise`] / [`Node::Bytewise`] / [`Node::Transform`] 递归检查内部子树
    /// （Phase 3.2 / 3.3）。
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
            _ => false,
        }
    }

    /// 为 RO 字段在 build 方向计算值。
    ///
    /// RO 字段不从实例取值，而是通过节点自身的逻辑计算。
    /// 设计依据：`docs/模块设计-表达式系统.md` §5.4。
    ///
    /// 当前支持的 RO 节点：
    /// - [`Node::Tell`]：返回当前 `BuildStream` 的写入位置（`usize → PyLong`）。
    /// - [`Node::Computed`]：通过 [`crate::expr::eval_expr_int`] 求值表达式。
    ///
    /// 其他节点（如 `Const`、`ContextParam`）将在 Phase 3 扩展时添加分支。
    ///
    /// NH-1 重构：从 struct_node.rs 的自由函数移至 `impl Node` 方法，
    /// 使其成为 Node 的公共 API，未来添加 Const/ContextParam 时只需在此方法中增加分支。
    ///
    /// # 参数
    ///
    /// - `py`：GIL token。
    /// - `stream`：当前 `BuildStream`（用于 Tell 取位置）。
    /// - `ctx`：当前上下文（用于 Computed 求值表达式，不可变借用）。
    /// - `path`：错误追踪路径栈。
    ///
    /// # 错误
    ///
    /// - [`ConstructError::Generic`]：节点类型不支持作为 RO（如 `FormatField`、`Bytes`）。
    ///   编译期校验（§5.6）应保证此分支不被触发，此处为运行时兜底。
    /// - 其他错误由 `Computed` 求值向上传播（`ExprContext` / `ExprFieldMissing` 等）。
    pub fn compute_ro_value(
        &self,
        py: Python<'_>,
        stream: &BuildStream,
        ctx: &Context<'_>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        match self {
            // Tell：返回当前写入位置（usize → PyLong）
            Node::Tell(_) => Ok(stream.tell().into_py(py)),
            // Computed：求值表达式（i64 → PyLong）
            Node::Computed(c) => {
                let v = crate::expr::eval_expr_int(c.expr(), ctx, py)?;
                Ok(v.into_py(py))
            }
            // 其他节点暂不支持 RO 语义（Phase 3 将扩展 Const/ContextParam）
            _ => Err(ConstructError::Generic {
                message: format!(
                    "compute_ro_value: node type {:?} is not a valid RO node. \
                     RO fields must be Tell, Computed (Const/ContextParam in Phase 3).",
                    self
                ),
                path: path.to_string(),
            }),
        }
    }
}
