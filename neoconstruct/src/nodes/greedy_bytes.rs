//! GreedyBytesNode：剩余字节读写。
//!
//! Python 参考：`construct/construct/core.py` `GreedyBytes`（L993-1018）。
//!
//! ## 行为
//!
//! - parse：读取流中剩余所有字节，创建 `PyBytes` 返回。
//! - build：从 `bytes` 对象提取数据，写入流（不校验长度）。
//! - sizeof：返回 `Err`（大小未知，对应 Python construct 的 `SizeofError`）。
//!
//! ## build 输入约束
//!
//! 与 `BytesNode` 一致，build 时接受 bytes-like 输入（`bytes` / `bytearray` /
//! `memoryview`，语义等同 `bytes`）。

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;
use pyo3::types::PyBytes;

// ---------------------------------------------------------------------------
// GreedyBytesNode
// ---------------------------------------------------------------------------

/// 剩余字节字段节点：读取/写入流中所有剩余字节。
///
/// 对应 Python construct 的 `GreedyBytes`（单元结构体，无配置参数）。
///
/// # parse 行为
///
/// 读取 `ParseStream::read_remaining()`，创建 `PyBytes` 返回。
/// 游标推进到末尾，之后 `is_at_end()` 返回 `true`。
///
/// # build 行为
///
/// 从 bytes-like 对象（`bytes` / `bytearray` / `memoryview`）提取数据，
/// 直接写入流。不校验长度（长度始终匹配）。
///
/// # sizeof
///
/// 返回 `Err`（大小未知），对应 Python construct 的 `SizeofError`。
#[derive(Debug, Clone, Copy, Default)]
pub struct GreedyBytesNode;

impl GreedyBytesNode {
    /// 创建一个 `GreedyBytesNode`（单元结构体，无参数）。
    pub fn new() -> Self {
        Self
    }
}

// ---------------------------------------------------------------------------
// Construct impl
// ---------------------------------------------------------------------------

impl super::Construct for GreedyBytesNode {
    fn parse(
        &self,
        py: Python<'_>,
        stream: &mut ParseStream<'_>,
        _ctx: &mut Context<'_>,
        _path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let data = stream.read_remaining();
        Ok(PyBytes::new_bound(py, data).into_any().unbind())
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        _ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        // 接受 bytes / bytearray / memoryview（语义等同 bytes）
        let data = super::common::extract_bytes_like(py, obj).map_err(|type_name| {
            ConstructError::Generic {
                message: format!(
                    "expected bytes object for GreedyBytes field, received non-bytes value of type {}",
                    type_name
                ),
                path: path.to_string(),
            }
        })?;
        stream.write(&data);
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // 对齐 Python construct：GreedyBytes 的 sizeof 抛出 SizeofError。
        // 我们的错误枚举无 SizeofError 变体，使用 Generic 表示大小未知。
        Err(ConstructError::Generic {
            message: "GreedyBytes size is undefined".to_string(),
            path: String::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::Construct;

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
    // parse
    // ======================================================================

    #[test]
    fn parse_reads_all_remaining_bytes() {
        with_py(|py| {
            let node = GreedyBytesNode::new();
            let mut stream = ParseStream::new(b"hello world");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(bytes, b"hello world");
            assert!(stream.is_at_end());
        });
    }

    #[test]
    fn parse_after_partial_read_gets_rest() {
        with_py(|py| {
            let node = GreedyBytesNode::new();
            let mut stream = ParseStream::new(b"hello world");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            // 先读 6 字节
            let _ = stream.read(6, &path).expect("first 6");

            // GreedyBytes 读剩余
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(bytes, b"world");
            assert!(stream.is_at_end());
        });
    }

    #[test]
    fn parse_empty_stream_returns_empty_bytes() {
        with_py(|py| {
            let node = GreedyBytesNode::new();
            let mut stream = ParseStream::new(b"");
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
    fn parse_at_end_returns_empty_bytes() {
        with_py(|py| {
            let node = GreedyBytesNode::new();
            let mut stream = ParseStream::new(b"ab");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let _ = stream.read(2, &path).expect("all");
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert!(bytes.is_empty());
        });
    }

    // ======================================================================
    // build
    // ======================================================================

    #[test]
    fn build_writes_all_bytes() {
        with_py(|py| {
            let node = GreedyBytesNode::new();
            let obj = py.eval_bound("b'hello'", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), b"hello");
        });
    }

    #[test]
    fn build_accepts_bytearray_input() {
        with_py(|py| {
            let node = GreedyBytesNode::new();
            let obj = py
                .eval_bound("bytearray(b'hello')", None, None)
                .expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), b"hello");
        });
    }

    #[test]
    fn build_accepts_memoryview_input() {
        with_py(|py| {
            let node = GreedyBytesNode::new();
            let obj = py
                .eval_bound("memoryview(b'hello')", None, None)
                .expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), b"hello");
        });
    }

    #[test]
    fn build_empty_bytes() {
        with_py(|py| {
            let node = GreedyBytesNode::new();
            let obj = py.eval_bound("b''", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert!(stream.as_bytes().is_empty());
        });
    }

    #[test]
    fn build_long_bytes() {
        with_py(|py| {
            let node = GreedyBytesNode::new();
            let obj = py.eval_bound("b'x' * 1000", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.tell(), 1000);
            assert_eq!(&stream.as_bytes()[..10], b"xxxxxxxxxx");
        });
    }

    #[test]
    fn build_rejects_non_bytes() {
        with_py(|py| {
            let node = GreedyBytesNode::new();
            let obj = py.eval_bound("42", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Generic { .. }));
        });
    }

    #[test]
    fn build_rejects_str() {
        with_py(|py| {
            let node = GreedyBytesNode::new();
            let obj = py.eval_bound("'hello'", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Generic { .. }));
        });
    }

    // ======================================================================
    // parse ↔ build 往返
    // ======================================================================

    #[test]
    fn round_trip_preserves_data() {
        with_py(|py| {
            let node = GreedyBytesNode::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            let obj = py
                .eval_bound("b'\\x00\\x01\\x02\\x03\\xFF'", None, None)
                .expect("eval");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();

            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let parsed: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(parsed, &[0x00, 0x01, 0x02, 0x03, 0xFF]);
        });
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_returns_err() {
        with_py(|py| {
            let node = GreedyBytesNode::new();
            let ctx = Context::new_root(py).expect("ctx");
            let err = node.sizeof(&ctx).expect_err("should fail");
            match err {
                ConstructError::Generic { message, .. } => {
                    assert!(message.contains("undefined"), "got: {}", message);
                }
                other => panic!("expected Generic error, got {:?}", other),
            }
        });
    }

    #[test]
    fn default_equals_new() {
        let n1 = GreedyBytesNode::new();
        let n2 = GreedyBytesNode;
        // 单元结构体，始终相等
        assert_eq!(format!("{:?}", n1), format!("{:?}", n2));
    }
}
