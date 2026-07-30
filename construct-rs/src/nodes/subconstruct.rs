//! SubconstructNode：单子构造器包装节点（纯转发）。
//!
//! 设计依据：`docs/design/模块设计/模块设计-Adapter核心.md` §1.1。
//! Python 参考：`construct/construct/core.py` `Subconstruct`（L787-810）。
//!
//! ## 行为概述
//!
//! SubstructNode 持有 `Box<Node>`，parse/build/sizeof 全部转发给 inner。
//! Python 中 Substruct 是抽象基类（Adapter/RawCopy/Peek/Rebuild/Tunnel 的父类），
//! construct-rs 把它实现为**具体节点**，用于：
//! - 用户显式包装（极少用，主要供未来 Pointer/Prefixed 复用）
//! - 作为 §2 其他内置 Adapter 的实现基础（共享"持有 `Box<Node>` + 转发"模式）

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

use super::Construct;
use crate::nodes::Node;

/// 单子构造器包装节点：parse/build/sizeof 全部转发给 inner。
///
/// 对应 Python construct `Subconstruct`（core.py L787）。
///
/// # 三方法行为
///
/// - parse：`inner.parse(...)`，结果原样返回（不解码）
/// - build：`inner.build(obj, ...)`，obj 原样传入（不编码）
/// - sizeof：`inner.sizeof(ctx)`
#[derive(Debug)]
pub struct SubconstructNode {
    /// 被包装的子树根。
    inner: Box<Node>,
}

impl SubconstructNode {
    /// 创建 `SubstructNode`，包裹给定的子树根节点。
    ///
    /// # 参数
    ///
    /// - `inner`：被包裹的子树（任意 Node 变体）。
    pub fn new(inner: Node) -> Self {
        Self {
            inner: Box::new(inner),
        }
    }

    /// 返回内部子树根节点的引用。
    pub fn inner(&self) -> &Node {
        &self.inner
    }
}

impl Construct for SubconstructNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 纯转发：直接调用 inner.parse。
        self.inner.parse(py, stream, ctx, path)
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        self.inner.build(py, obj, stream, ctx, path)
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        self.inner.sizeof(ctx)
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::bytes::BytesNode;
    use crate::nodes::format_field::{FormatFieldNode, PythonFormat};

    fn ensure_python() {
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(pyo3::prepare_freethreaded_python);
    }

    fn with_py<F, R>(f: F) -> R
    where
        F: for<'py> FnOnce(Python<'py>) -> R,
    {
        ensure_python();
        Python::with_gil(f)
    }

    // ======================================================================
    // 构造器
    // ======================================================================

    #[test]
    fn new_stores_inner() {
        let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
        let node = SubconstructNode::new(inner);
        assert!(matches!(node.inner(), Node::FormatField(_)));
    }

    // ======================================================================
    // parse — 纯转发
    // ======================================================================

    #[test]
    fn parse_format_field_returns_inner_value() {
        // SC-1: Subconstruct(FormatField) parse 应与直接 FormatField parse 一致
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let node = SubconstructNode::new(inner);

            let mut stream = ParseStream::new(&[0x42]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().unwrap();
            assert_eq!(v, 0x42);
            assert_eq!(stream.tell(), 1);
        });
    }

    #[test]
    fn parse_bytes_returns_inner_bytes() {
        with_py(|py| {
            let inner = Node::Bytes(BytesNode::new_const(3));
            let node = SubconstructNode::new(inner);

            let mut stream = ParseStream::new(&[0x01, 0x02, 0x03]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: &[u8] = result.bind(py).extract().unwrap();
            assert_eq!(v, &[0x01, 0x02, 0x03]);
        });
    }

    // ======================================================================
    // build — 纯转发
    // ======================================================================

    #[test]
    fn build_forwards_to_inner() {
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let node = SubconstructNode::new(inner);

            let obj = py.eval_bound("0x42", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x42]);
        });
    }

    // ======================================================================
    // sizeof — 纯转发
    // ======================================================================

    #[test]
    fn sizeof_returns_inner_size() {
        with_py(|py| {
            let ctx = Context::placeholder(py);
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
            let node = SubconstructNode::new(inner);
            assert_eq!(node.sizeof(&ctx).unwrap(), 2);

            let inner = Node::Bytes(BytesNode::new_const(4));
            let node = SubconstructNode::new(inner);
            assert_eq!(node.sizeof(&ctx).unwrap(), 4);
        });
    }

    // ======================================================================
    // parse ↔ build 往返
    // ======================================================================

    #[test]
    fn round_trip_format_field() {
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt32Big));
            let node = SubconstructNode::new(inner);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            let obj = py.eval_bound("0xDEADBEEF", None, None).expect("obj");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();
            assert_eq!(bytes, vec![0xDE, 0xAD, 0xBE, 0xEF]);

            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().unwrap();
            assert_eq!(v, 0xDEADBEEF);
        });
    }

    // ======================================================================
    // Debug
    // ======================================================================

    #[test]
    fn debug_format_includes_subconstruct_node() {
        let inner = Node::Bytes(BytesNode::new_const(2));
        let node = SubconstructNode::new(inner);
        let s = format!("{:?}", node);
        assert!(s.contains("SubconstructNode"), "got: {}", s);
    }
}
