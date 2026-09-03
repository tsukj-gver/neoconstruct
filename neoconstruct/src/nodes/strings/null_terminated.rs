//! NullTerminatedNode：null 终止包装器。
//!
//! Python 参考：`construct/construct/core.py` `NullTerminated`（L5050-5115）。
//!
//! 与 CStringNode 的扫描算法共享，但持有任意 `inner: Box<Node>`，
//! 返回 inner 的解析结果（通常是 `bytes`，inner=GreedyBytes 时）。

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

/// null 终止包装器节点（持有任意 inner subcon）。
///
/// 对应 Python construct `NullTerminated(subcon, term, include, consume, require)`
/// （core.py L5050）。
///
/// # parse 行为
///
/// 1. 逐 unit 字节扫描直到 term（同 CString，但累积的 bytes 用于 inner.parse）
/// 2. 在累积的 bytes 上构造子 ParseStream，调 inner.parse
/// 3. 返回 inner 的结果（通常是 PyBytes，inner=GreedyBytes 时）
///
/// # build 行为
///
/// 1. 调 inner.build(obj) → 字节写入主 stream
/// 2. 写 term 字节串
///
/// # sizeof
///
/// 永远 `Err`（inner 长度 + term 长度未知）。
///
/// # 注意
///
/// 不派生 `Clone`：持有 `Box<Node>`，而 [`crate::nodes::Node`] enum 因含 Python
/// 引用不派生 `Clone`（与 `BitwiseNode` / `SubconstructNode` / `PrefixedArrayNode`
/// 等现有 `Box<Node>` 节点一致）。
#[derive(Debug)]
pub struct NullTerminatedNode {
    /// 内层子构造器（默认 GreedyBytes，但用户可传 Byte / Bytes(n) / Struct 等）。
    inner: Box<crate::nodes::Node>,
    /// 终止符字节串（默认 `b"\x00"`）。
    term: Vec<u8>,
    /// 是否将 term 包含在累积数据中（默认 false）。
    include: bool,
    /// 是否消费 term（true=消费；false=seek 回退 unit 字节，默认 true）。
    consume: bool,
    /// EOF 时是否报错（默认 true）。
    require: bool,
}

impl NullTerminatedNode {
    /// 创建 `NullTerminatedNode`，默认 term=`b"\x00"`、include=false、consume=true、require=true。
    pub fn new(inner: crate::nodes::Node) -> Self {
        Self {
            inner: Box::new(inner),
            term: vec![0u8],
            include: false,
            consume: true,
            require: true,
        }
    }

    /// 全参数构造（Descriptor 编译期调用）。
    pub fn with_options(
        inner: crate::nodes::Node,
        term: Vec<u8>,
        include: bool,
        consume: bool,
        require: bool,
    ) -> Self {
        Self {
            inner: Box::new(inner),
            term,
            include,
            consume,
            require,
        }
    }

    /// 返回 inner 子树引用。
    pub fn inner(&self) -> &crate::nodes::Node {
        &self.inner
    }

    /// 返回 term 字节切片。
    pub fn term(&self) -> &[u8] {
        &self.term
    }

    /// 是否包含 term。
    pub fn include(&self) -> bool {
        self.include
    }

    /// 是否消费 term。
    pub fn consume(&self) -> bool {
        self.consume
    }

    /// 是否在 EOF 时报错。
    pub fn require(&self) -> bool {
        self.require
    }

    /// has_expressions：递归检查 inner 子树（StructNode 编译期检查用）。
    pub fn has_expressions(&self) -> bool {
        self.inner.has_expressions()
    }
}

impl crate::nodes::Construct for NullTerminatedNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let unit = self.term.len();
        if unit == 0 {
            return Err(ConstructError::Padding {
                message: "NullTerminated term must be at least 1 byte".to_string(),
                path: path.to_string(),
            });
        }
        let mut accumulated: Vec<u8> = Vec::new();
        loop {
            match stream.read(unit, path) {
                Ok(chunk) => {
                    if chunk == self.term.as_slice() {
                        if self.include {
                            accumulated.extend_from_slice(chunk);
                        }
                        if !self.consume {
                            // term 不被消费：stream 回退 unit 字节
                            let new_pos = stream.tell().saturating_sub(unit);
                            stream.seek(new_pos, path)?;
                        }
                        break;
                    }
                    accumulated.extend_from_slice(chunk);
                }
                Err(ConstructError::Stream { .. }) if !self.require => break,
                Err(e) => return Err(e),
            }
        }
        // 用 accumulated 构造子流，调 inner.parse
        let mut sub_stream = ParseStream::new(&accumulated);
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
        self.inner.build(py, obj, stream, ctx, path)?;
        stream.write(&self.term);
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        Err(ConstructError::Generic {
            message: "NullTerminated size is undefined".to_string(),
            path: String::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::bytes::BytesNode;
    use crate::nodes::format_field::{FormatFieldNode, PythonFormat};
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

    fn byte_node() -> crate::nodes::Node {
        crate::nodes::Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big))
    }

    // ======================================================================
    // 构造器 & 访问器
    // ======================================================================

    #[test]
    fn new_sets_defaults() {
        let node = NullTerminatedNode::new(greedy_bytes_node());
        assert_eq!(node.term(), b"\x00");
        assert!(!node.include());
        assert!(node.consume());
        assert!(node.require());
        assert!(!node.has_expressions());
    }

    #[test]
    fn with_options_overrides_defaults() {
        let node = NullTerminatedNode::with_options(
            greedy_bytes_node(),
            b"\xFF".to_vec(),
            true,
            false,
            false,
        );
        assert_eq!(node.term(), b"\xFF");
        assert!(node.include());
        assert!(!node.consume());
        assert!(!node.require());
    }

    // ======================================================================
    // parse：inner=GreedyBytes
    // ======================================================================

    #[test]
    fn parse_greedy_bytes_default_term() {
        with_py(|py| {
            let node = NullTerminatedNode::new(greedy_bytes_node());
            let mut stream = ParseStream::new(b"hello\x00world");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(bytes, b"hello");
            // term 消费，cursor 在 world 之前
            assert_eq!(stream.tell(), 6);
        });
    }

    #[test]
    fn parse_include_true_keeps_term_in_data() {
        with_py(|py| {
            let node = NullTerminatedNode::with_options(
                greedy_bytes_node(),
                b"\x00".to_vec(),
                true,
                true,
                true,
            );
            let mut stream = ParseStream::new(b"hi\x00");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(bytes, b"hi\x00");
        });
    }

    #[test]
    fn parse_consume_false_leaves_term_in_stream() {
        with_py(|py| {
            let node = NullTerminatedNode::with_options(
                greedy_bytes_node(),
                b"\x00".to_vec(),
                false,
                false,
                true,
            );
            let mut stream = ParseStream::new(b"hi\x00extra");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(bytes, b"hi");
            // consume=false：term 留在 stream，cursor 在 term 之前
            assert_eq!(stream.tell(), 2);
        });
    }

    #[test]
    fn parse_require_false_eof_uses_accumulated() {
        with_py(|py| {
            let node = NullTerminatedNode::with_options(
                greedy_bytes_node(),
                b"\x00".to_vec(),
                false,
                true,
                false,
            );
            let mut stream = ParseStream::new(b"hello"); // 无 term
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(bytes, b"hello");
        });
    }

    #[test]
    fn parse_require_true_eof_returns_stream_error() {
        with_py(|py| {
            let node = NullTerminatedNode::new(greedy_bytes_node());
            let mut stream = ParseStream::new(b"hello"); // 无 term
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Stream { .. }));
        });
    }

    // ======================================================================
    // parse：inner=Byte（非 GreedyBytes）
    // ======================================================================

    #[test]
    fn parse_inner_byte_takes_first_byte() {
        with_py(|py| {
            let node = NullTerminatedNode::new(byte_node());
            // 累积 b"\xff\x42\x00" → Byte 取首字节 0xFF = 255
            let mut stream = ParseStream::new(b"\xff\x42\x00");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let n: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(n, 255);
        });
    }

    #[test]
    fn parse_inner_bytes_n_reads_n_bytes() {
        with_py(|py| {
            // inner = Bytes(2)
            let inner = crate::nodes::Node::Bytes(BytesNode::new_const(2));
            let node = NullTerminatedNode::new(inner);
            // 累积 b"\xaa\xbb\x00" → Bytes(2) 取前 2 字节
            let mut stream = ParseStream::new(b"\xaa\xbb\x00");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result
                .bind(py)
                .downcast::<PyBytes>()
                .expect("PyBytes")
                .as_bytes();
            assert_eq!(bytes, b"\xaa\xbb");
        });
    }

    #[test]
    fn parse_empty_term_returns_padding_error() {
        with_py(|py| {
            let node =
                NullTerminatedNode::with_options(greedy_bytes_node(), vec![], false, true, true);
            let mut stream = ParseStream::new(b"hi");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Padding { .. }));
        });
    }

    // ======================================================================
    // build
    // ======================================================================

    #[test]
    fn build_greedy_bytes_appends_term() {
        with_py(|py| {
            let node = NullTerminatedNode::new(greedy_bytes_node());
            let obj = py.eval_bound("b'hello'", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), b"hello\x00");
        });
    }

    #[test]
    fn build_inner_byte_writes_one_byte_plus_term() {
        with_py(|py| {
            let node = NullTerminatedNode::new(byte_node());
            let obj = py.eval_bound("255", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), b"\xff\x00");
        });
    }

    // ======================================================================
    // round-trip
    // ======================================================================

    #[test]
    fn round_trip_greedy_bytes_preserves_data() {
        with_py(|py| {
            let node = NullTerminatedNode::new(greedy_bytes_node());
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
            let parsed: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(parsed, b"abc");
        });
    }

    #[test]
    fn sizeof_returns_err() {
        with_py(|py| {
            let node = NullTerminatedNode::new(greedy_bytes_node());
            let ctx = Context::new_root(py).expect("ctx");
            assert!(node.sizeof(&ctx).is_err());
        });
    }
}
