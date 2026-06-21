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
//! ## Phase 1.3 范围
//!
//! 当前仅含 3 个原子节点变体：
//! - [`FormatFieldNode`](format_field::FormatFieldNode)：整数读写（Int8ub 等）
//! - [`BytesNode`](bytes::BytesNode)：固定长度字节读写
//! - [`GreedyBytesNode`](greedy_bytes::GreedyBytesNode)：剩余字节读写
//!
//! Phase 1.4 将添加 `Struct` / `StructRef` 复合节点变体。

pub mod bytes;
pub mod format_field;
pub mod greedy_bytes;

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use bytes::BytesNode;
use enum_dispatch::enum_dispatch;
use format_field::FormatFieldNode;
use greedy_bytes::GreedyBytesNode;
use pyo3::prelude::*;

/// 所有构造器节点实现的统一接口。
///
/// parse 直接产出 Python 对象（`Py<PyAny>`），build 直接读取 Python 对象
/// （`&Bound<PyAny>`）——不经过任何 Rust 中间类型（FFI 设计 §5）。
///
/// 每个具体节点类型实现此 trait，然后通过 `enum_dispatch` 在 [`Node`] 枚举上
/// 自动生成静态分派的 `match` 代码。
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
/// # Phase 1.3 变体
///
/// 仅含 3 个原子节点。Phase 1.4 将添加 `Struct(StructNode)` 和
/// `StructRef(StructRefNode)` 复合节点。
#[enum_dispatch(Construct)]
pub enum Node {
    /// 整数读写节点：`Int8ub` / `Int16ul` / ... （对应 Python construct `FormatField`）
    FormatField(FormatFieldNode),
    /// 固定长度字节读写节点：`Bytes(n)` （对应 Python construct `Bytes`）
    Bytes(BytesNode),
    /// 剩余字节读写节点：`GreedyBytes` （对应 Python construct `GreedyBytes`）
    GreedyBytes(GreedyBytesNode),
    // Phase 1.4 将添加：
    // Struct(StructNode),
    // StructRef(StructRefNode),
}
