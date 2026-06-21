//! BytesNode：固定长度字节读写。
//!
//! 设计依据：`docs/架构设计.md` §C.3.2。
//! Python 参考：`construct/construct/core.py` `Bytes`（L924-980）。
//!
//! ## Phase 1 范围
//!
//! `length` 为编译期常量整数。`length` 为上下文 lambda（如 `Bytes(this.length)`）
//! 的能力推迟到 Phase 2（需表达式系统支持）。
//!
//! ## REV 约束
//!
//! build 时**仅接受 `bytes` 输入**。int/bytearray 转换推迟到 Phase 2。
//! 这与 Python construct 不同（Python 原版接受 int 和 bytearray），但是
//! Phase 1 简化的安全选择。

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;
use pyo3::types::PyBytes;

// ---------------------------------------------------------------------------
// BytesNode
// ---------------------------------------------------------------------------

/// 固定长度字节字段节点：读取/写入恰好 `length` 字节的原始数据。
///
/// 对应 Python construct 的 `Bytes(length)`。
///
/// # parse 行为
///
/// 从流中读取 `length` 字节，创建 `PyBytes` 对象返回。
/// 不足时返回 `StreamError`（对齐 Python construct `stream_read`）。
///
/// # build 行为
///
/// 1. 校验输入对象为 `bytes` 类型（REV 约束：仅接受 bytes）。
/// 2. 校验长度匹配，不匹配 → `FieldLengthError`。
/// 3. 写入流。
///
/// # sizeof
///
/// 返回 `length`（编译期已知）。
#[derive(Debug, Clone, Copy)]
pub struct BytesNode {
    /// 固定字节长度。Phase 1 仅支持编译期常量。
    length: usize,
}

impl BytesNode {
    /// 创建一个读取/写入 `length` 字节的 `BytesNode`。
    pub fn new(length: usize) -> Self {
        Self { length }
    }

    /// 返回固定字节长度。
    pub fn length(&self) -> usize {
        self.length
    }
}

// ---------------------------------------------------------------------------
// Construct impl
// ---------------------------------------------------------------------------

impl super::Construct for BytesNode {
    fn parse(
        &self,
        py: Python<'_>,
        stream: &mut ParseStream<'_>,
        _ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let data = stream.read(self.length, path)?;
        // PyBytes::new_bound 拷贝 data 到 Python 堆。零拷贝优化推迟到 Phase 2
        // （需安全地绑定生命周期到 PyBytes 对象，避免数据竞争）。
        Ok(PyBytes::new_bound(py, data).into_any().unbind())
    }

    fn build(
        &self,
        _py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        _ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        // REV 约束：仅接受 bytes（bytearray/int 转换推迟到 Phase 2）
        let py_bytes = obj
            .downcast::<PyBytes>()
            .map_err(|_| ConstructError::Generic {
                message: format!(
                    "expected bytes object for Bytes({}) field, received non-bytes value",
                    self.length
                ),
                path: path.to_string(),
            })?;
        let data = py_bytes.as_bytes();
        if data.len() != self.length {
            return Err(ConstructError::FieldLength {
                message: format!(
                    "bytes object of wrong length, expected {}, found {}",
                    self.length,
                    data.len()
                ),
                path: path.to_string(),
            });
        }
        stream.write(data);
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        Ok(self.length)
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
    fn parse_reads_exact_length() {
        with_py(|py| {
            let node = BytesNode::new(4);
            let mut stream = ParseStream::new(b"hello world");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // 结果应为 4 字节 bytes
            let bytes: &[u8] = result.bind(py).extract().expect("extract bytes");
            assert_eq!(bytes, b"hell");
            assert_eq!(stream.tell(), 4);
        });
    }

    #[test]
    fn parse_zero_length_returns_empty_bytes() {
        with_py(|py| {
            let node = BytesNode::new(0);
            let mut stream = ParseStream::new(b"abc");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert!(bytes.is_empty());
            assert_eq!(stream.tell(), 0);
        });
    }

    #[test]
    fn parse_insufficient_bytes_returns_stream_error() {
        with_py(|py| {
            let node = BytesNode::new(5);
            let mut stream = ParseStream::new(b"abc");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Stream { message, .. } => {
                    assert!(message.contains("expected 5"), "got: {}", message);
                    assert!(message.contains("found 3"), "got: {}", message);
                }
                other => panic!("expected Stream error, got {:?}", other),
            }
        });
    }

    #[test]
    fn parse_full_length_consumes_stream() {
        with_py(|py| {
            let node = BytesNode::new(5);
            let mut stream = ParseStream::new(b"hello");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(bytes, b"hello");
            assert!(stream.is_at_end());
        });
    }

    // ======================================================================
    // build
    // ======================================================================

    #[test]
    fn build_writes_matching_bytes() {
        with_py(|py| {
            let node = BytesNode::new(4);
            let obj = py.eval_bound("b'beef'", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), b"beef");
        });
    }

    #[test]
    fn build_zero_length_accepts_empty_bytes() {
        with_py(|py| {
            let node = BytesNode::new(0);
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
    fn build_wrong_length_returns_field_length_error() {
        with_py(|py| {
            let node = BytesNode::new(4);
            let obj = py.eval_bound("b'ab'", None, None).expect("eval"); // 2 != 4
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::FieldLength { message, .. } => {
                    assert!(message.contains("expected 4"), "got: {}", message);
                    assert!(message.contains("found 2"), "got: {}", message);
                }
                other => panic!("expected FieldLength error, got {:?}", other),
            }
        });
    }

    #[test]
    fn build_too_long_returns_field_length_error() {
        with_py(|py| {
            let node = BytesNode::new(2);
            let obj = py.eval_bound("b'hello'", None, None).expect("eval"); // 5 != 2
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::FieldLength { .. }));
        });
    }

    #[test]
    fn build_rejects_int_input() {
        // REV 约束：int 转换推迟到 Phase 2
        with_py(|py| {
            let node = BytesNode::new(4);
            let obj = py.eval_bound("0", None, None).expect("eval");
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
    fn build_rejects_str_input() {
        with_py(|py| {
            let node = BytesNode::new(5);
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

    #[test]
    fn build_rejects_bytearray_input() {
        // REV 约束：bytearray 转换推迟到 Phase 2
        with_py(|py| {
            let node = BytesNode::new(5);
            let obj = py
                .eval_bound("bytearray(b'hello')", None, None)
                .expect("eval");
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
    fn round_trip_bytes_preserves_data() {
        with_py(|py| {
            let node = BytesNode::new(6);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            let obj = py.eval_bound("b'ABCDEF'", None, None).expect("eval");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();

            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let parsed: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(parsed, b"ABCDEF");
        });
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_returns_length() {
        with_py(|py| {
            let ctx = Context::new_root(py).expect("ctx");
            assert_eq!(BytesNode::new(0).sizeof(&ctx).unwrap(), 0);
            assert_eq!(BytesNode::new(1).sizeof(&ctx).unwrap(), 1);
            assert_eq!(BytesNode::new(100).sizeof(&ctx).unwrap(), 100);
        });
    }
}
