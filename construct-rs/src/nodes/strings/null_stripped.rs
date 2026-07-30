//! NullStrippedNode：null 剥离包装器（Phase 6.2 §3.3）。
//!
//! Python 参考：`construct/construct/core.py` `NullStripped`（L5123-5178）。
//!
//! "读全部 + 右剥离 pad + inner parse"。build 仅转发 inner（不补 pad）——
//! 补 pad 由外层 PaddedStringNode 内联实现。

use crate::context::Context;
use crate::error::ConstructError;
use crate::nodes::strings::rstrip_pad;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

/// null 剥离包装器节点（持有任意 inner subcon）。
///
/// 对应 Python construct `NullStripped(subcon, pad)`（core.py L5123）。
///
/// # parse 行为
///
/// 1. 读流中剩余所有字节（`stream.read_remaining()`）
/// 2. 右剥离 pad 字节串（[`rstrip_pad`]）：
///    - unit=1（默认）：单字节 pad，反复剥离末尾匹配字节
///    - unit>1：尾部不完整单元匹配 pad 前缀则剥离 + 整 unit 剥离（对齐 Python 算法）
/// 3. 用剥离后字节构造子流，调 inner.parse
///
/// # build 行为
///
/// 直接调 `inner.build(obj)`，不追加 pad（与 Python NullStripped._build 一致；
/// pad 补全由外层 PaddedStringNode 内联实现，设计 §3.5）。
///
/// # sizeof
///
/// 永远 `Err`（数据 + pad 长度未知）。
///
/// # 注意
///
/// 不派生 `Clone`：持有 `Box<Node>`，与 `NullTerminatedNode` / `BitwiseNode` 等
/// 现有 `Box<Node>` 节点一致。
#[derive(Debug)]
pub struct NullStrippedNode {
    /// 内层子构造器。
    inner: Box<crate::nodes::Node>,
    /// pad 字节串（默认 `b"\x00"`）。
    pad: Vec<u8>,
}

impl NullStrippedNode {
    /// 创建 `NullStrippedNode`，默认 pad=`b"\x00"`。
    pub fn new(inner: crate::nodes::Node) -> Self {
        Self {
            inner: Box::new(inner),
            pad: vec![0u8],
        }
    }

    /// 指定 pad 字节串构造。
    pub fn with_pad(inner: crate::nodes::Node, pad: Vec<u8>) -> Self {
        Self {
            inner: Box::new(inner),
            pad,
        }
    }

    /// 返回 inner 子树引用。
    pub fn inner(&self) -> &crate::nodes::Node {
        &self.inner
    }

    /// 返回 pad 字节切片。
    pub fn pad(&self) -> &[u8] {
        &self.pad
    }

    /// has_expressions：递归检查 inner 子树。
    pub fn has_expressions(&self) -> bool {
        self.inner.has_expressions()
    }
}

impl crate::nodes::Construct for NullStrippedNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        if self.pad.is_empty() {
            return Err(ConstructError::Padding {
                message: "NullStripped pad must be at least 1 byte".to_string(),
                path: path.to_string(),
            });
        }
        let data = stream.read_remaining();
        let stripped = rstrip_pad(data, &self.pad);
        let mut sub_stream = ParseStream::new(stripped);
        self.inner.parse(py, &mut sub_stream, ctx, path)
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        // 直接转发 inner（不补 pad——pad 由外层 PaddedStringNode 内联处理）。
        self.inner.build(py, obj, stream, ctx, path)
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        Err(ConstructError::Generic {
            message: "NullStripped size is undefined".to_string(),
            path: String::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::greedy_bytes::GreedyBytesNode;
    use crate::nodes::Construct;
    use pyo3::types::PyBytes;

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

    fn greedy_bytes_node() -> crate::nodes::Node {
        crate::nodes::Node::GreedyBytes(GreedyBytesNode::new())
    }

    // ======================================================================
    // 构造器 & 访问器
    // ======================================================================

    #[test]
    fn new_sets_default_pad() {
        let node = NullStrippedNode::new(greedy_bytes_node());
        assert_eq!(node.pad(), b"\x00");
        assert!(!node.has_expressions());
    }

    #[test]
    fn with_pad_overrides_default() {
        let node = NullStrippedNode::with_pad(greedy_bytes_node(), b"\xFF".to_vec());
        assert_eq!(node.pad(), b"\xFF");
    }

    // ======================================================================
    // parse：single byte pad
    // ======================================================================

    #[test]
    fn parse_single_byte_pad_strips_trailing_zeros() {
        with_py(|py| {
            let node = NullStrippedNode::new(greedy_bytes_node());
            let mut stream = ParseStream::new(b"abc\x00\x00");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(bytes, b"abc");
        });
    }

    #[test]
    fn parse_single_byte_pad_no_trailing() {
        with_py(|py| {
            let node = NullStrippedNode::new(greedy_bytes_node());
            let mut stream = ParseStream::new(b"abc");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(bytes, b"abc");
        });
    }

    #[test]
    fn parse_single_byte_pad_all_zeros_returns_empty() {
        with_py(|py| {
            let node = NullStrippedNode::new(greedy_bytes_node());
            let mut stream = ParseStream::new(b"\x00\x00");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert!(bytes.is_empty());
        });
    }

    #[test]
    fn parse_single_byte_custom_pad() {
        with_py(|py| {
            let node = NullStrippedNode::with_pad(greedy_bytes_node(), b"\xAA".to_vec());
            let mut stream = ParseStream::new(b"abc\xAA\xAA");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(bytes, b"abc");
        });
    }

    // ======================================================================
    // parse：multi-byte pad（utf16）
    // ======================================================================

    #[test]
    fn parse_multibyte_pad_strips_full_units() {
        with_py(|py| {
            let node = NullStrippedNode::with_pad(greedy_bytes_node(), b"\x00\x00".to_vec());
            // data = b"ab\x00\x00\x00\x00"（6 字节），pad unit=2
            // tailunit=0，剥离 2 个完整 unit → "ab"
            let mut stream = ParseStream::new(b"ab\x00\x00\x00\x00");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(bytes, b"ab");
        });
    }

    #[test]
    fn parse_multibyte_pad_partial_tail_matching_prefix() {
        with_py(|py| {
            let node = NullStrippedNode::with_pad(greedy_bytes_node(), b"XY".to_vec());
            // data = b"abX"（3 字节），pad=b"XY"，tailunit=1
            // data[-1:]=b"X" == pad[:1]=b"X" → 剥离前缀 → "ab"
            let mut stream = ParseStream::new(b"abX");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(bytes, b"ab");
        });
    }

    #[test]
    fn parse_empty_pad_returns_padding_error() {
        with_py(|py| {
            let node = NullStrippedNode::with_pad(greedy_bytes_node(), vec![]);
            let mut stream = ParseStream::new(b"abc");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Padding { .. }));
        });
    }

    // ======================================================================
    // build：仅转发 inner（不补 pad）
    // ======================================================================

    #[test]
    fn build_forwards_inner_no_pad() {
        with_py(|py| {
            let node = NullStrippedNode::new(greedy_bytes_node());
            let obj = py.eval_bound("b'abc'", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), b"abc");
        });
    }

    #[test]
    fn build_empty_bytes_forwards_as_empty() {
        with_py(|py| {
            let node = NullStrippedNode::new(greedy_bytes_node());
            let obj = py.eval_bound("b''", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert!(stream.as_bytes().is_empty());
        });
    }

    // ======================================================================
    // round-trip（注意：build 不补 pad，故 round-trip 仅当无 pad 时保真）
    // ======================================================================

    #[test]
    fn round_trip_no_trailing_pad_preserves_data() {
        with_py(|py| {
            let node = NullStrippedNode::new(greedy_bytes_node());
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            let obj = py.eval_bound("b'abc'", None, None).expect("eval");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();

            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let parsed: &[u8] = result
                .bind(py)
                .downcast::<PyBytes>()
                .expect("PyBytes")
                .as_bytes();
            assert_eq!(parsed, b"abc");
        });
    }

    #[test]
    fn sizeof_returns_err() {
        with_py(|py| {
            let node = NullStrippedNode::new(greedy_bytes_node());
            let ctx = Context::new_root(py).expect("ctx");
            assert!(node.sizeof(&ctx).is_err());
        });
    }
}
