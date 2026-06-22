//! Node 系统：Construct trait 定义与 Node 枚举（enum_dispatch 静态分派）。
//!
//! 设计依据：`docs/架构设计.md` §C.1（Construct trait）、§C.2（Node enum）。
//!
//! ## 概述
//!
//! 执行树的每个节点实现统一的 [`Construct`] trait，提供 `parse` / `build` / `sizeof`
//! 三个方法。节点类型用封闭 [`Node`] 枚举列举，通过 `enum_dispatch` 宏自动生成
//! `match` 分派代码，无 `Box<dyn>` 动态分派开销。
//!
//! ## 当前范围
//!
//! 当前含 7 个节点变体：
//! - 原子节点：[`FormatFieldNode`](format_field::FormatFieldNode)（整数读写）、
//!   [`BytesNode`](bytes::BytesNode)（固定/表达式长度字节）、
//!   [`GreedyBytesNode`](greedy_bytes::GreedyBytesNode)（剩余字节）
//! - 复合节点：[`StructNode`](struct_node::StructNode)（字段序列根节点）、
//!   [`StructRefNode`](struct_ref::StructRefNode)（嵌套引用其他 StructMixin 子类）
//! - RO 节点：[`TellNode`](tell::TellNode)（流位置）、
//!   [`ComputedNode`](computed::ComputedNode)（表达式计算值）

pub mod bytes;
pub mod computed;
pub mod format_field;
pub mod greedy_bytes;
pub mod struct_node;
pub mod struct_ref;
pub mod tell;

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use bytes::BytesNode;
use computed::ComputedNode;
use enum_dispatch::enum_dispatch;
use format_field::FormatFieldNode;
use greedy_bytes::GreedyBytesNode;
use pyo3::prelude::*;
use struct_node::StructNode;
use struct_ref::StructRefNode;
use tell::TellNode;

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
    /// # 返回
    ///
    /// 成功返回 `Py<PyAny>`（Python 对象引用），失败返回 [`ConstructError`]。
    fn parse(
        &self,
        py: Python<'_>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'_>,
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
/// - 3 个原子节点：`FormatField`、`Bytes`、`GreedyBytes`
/// - 2 个复合节点：`Struct`（字段序列）、`StructRef`（嵌套引用）
/// - 2 个 RO 节点：`Tell`（流位置）、`Computed`（表达式计算值）
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
}
