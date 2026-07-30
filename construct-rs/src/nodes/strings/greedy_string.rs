//! GreedyStringNode：读到流结束并解码（Phase 6.2 §3.4）。
//!
//! Python 参考：`construct/construct/core.py` `GreedyString`（L1837-1858）。
//!
//! Python 原版是 `StringEncoded(GreedyBytes, encoding)` macro 嵌套；本设计独立实现
//! （PM 决策 1 方案 A）：直接 `read_remaining` + `Encoding::decode`。

use crate::context::Context;
use crate::error::ConstructError;
use crate::nodes::strings::encoding::Encoding;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

/// 贪婪字符串节点：读到流结束并解码。
///
/// 对应 Python construct `GreedyString(encoding)`（core.py L1837）。
///
/// # parse 行为
///
/// `stream.read_remaining()` → [`Encoding::decode`] → `PyString`。
///
/// # build 行为
///
/// 校验 `str` → [`Encoding::encode`] → 写入 stream（不校验长度）。
///
/// # sizeof
///
/// 永远 `Err`（大小未知）。
#[derive(Debug, Clone, Copy)]
pub struct GreedyStringNode {
    /// 编码（编译期从用户字符串解析为 enum）。
    encoding: Encoding,
}

impl GreedyStringNode {
    /// 创建 `GreedyStringNode`。
    pub fn new(encoding: Encoding) -> Self {
        Self { encoding }
    }

    /// 返回编码。
    pub fn encoding(&self) -> Encoding {
        self.encoding
    }
}

impl super::super::Construct for GreedyStringNode {
    fn parse(
        &self,
        py: Python<'_>,
        stream: &mut ParseStream<'_>,
        _ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let data = stream.read_remaining();
        let py_str = self.encoding.decode(py, data, path)?;
        Ok(py_str.into_any().unbind())
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        _ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        let data = self.encoding.encode(py, obj, path)?;
        stream.write(&data);
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        Err(ConstructError::Generic {
            message: "GreedyString size is undefined".to_string(),
            path: String::new(),
        })
    }
}

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
    // 构造器 & 访问器
    // ======================================================================

    #[test]
    fn new_stores_encoding() {
        let node = GreedyStringNode::new(Encoding::Utf8);
        assert_eq!(node.encoding(), Encoding::Utf8);
    }

    #[test]
    fn clone_preserves_encoding() {
        let node = GreedyStringNode::new(Encoding::Ascii);
        let cloned = node;
        assert_eq!(cloned.encoding(), Encoding::Ascii);
    }

    // ======================================================================
    // parse：utf8
    // ======================================================================

    #[test]
    fn parse_utf8_ascii_text() {
        with_py(|py| {
            let node = GreedyStringNode::new(Encoding::Utf8);
            let mut stream = ParseStream::new(b"hello");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let s: String = result.bind(py).extract().expect("extract");
            assert_eq!(s, "hello");
            assert!(stream.is_at_end());
        });
    }

    #[test]
    fn parse_utf8_multibyte_text() {
        with_py(|py| {
            let node = GreedyStringNode::new(Encoding::Utf8);
            let mut stream = ParseStream::new("Афон".as_bytes());
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let s: String = result.bind(py).extract().expect("extract");
            assert_eq!(s, "Афон");
        });
    }

    #[test]
    fn parse_utf8_empty_stream_returns_empty_string() {
        with_py(|py| {
            let node = GreedyStringNode::new(Encoding::Utf8);
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let s: String = result.bind(py).extract().expect("extract");
            assert!(s.is_empty());
        });
    }

    #[test]
    fn parse_utf8_invalid_bytes_returns_string_error() {
        with_py(|py| {
            let node = GreedyStringNode::new(Encoding::Utf8);
            // 0xFF 不是合法 UTF-8 起始字节
            let mut stream = ParseStream::new(&[0xFF, 0xFE]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::String { .. }));
        });
    }

    // ======================================================================
    // parse：ascii
    // ======================================================================

    #[test]
    fn parse_ascii_text() {
        with_py(|py| {
            let node = GreedyStringNode::new(Encoding::Ascii);
            let mut stream = ParseStream::new(b"hello");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let s: String = result.bind(py).extract().expect("extract");
            assert_eq!(s, "hello");
        });
    }

    #[test]
    fn parse_ascii_high_byte_returns_string_error() {
        with_py(|py| {
            let node = GreedyStringNode::new(Encoding::Ascii);
            let mut stream = ParseStream::new(&[0xFF]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::String { .. }));
        });
    }

    // ======================================================================
    // parse：utf16-le / utf16-be / utf32-le（raw FFI）
    // ======================================================================

    #[test]
    fn parse_utf16_le_text() {
        with_py(|py| {
            let node = GreedyStringNode::new(Encoding::Utf16Le);
            // "h" in UTF-16 LE = b'h\x00'
            let mut stream = ParseStream::new(b"h\x00");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let s: String = result.bind(py).extract().expect("extract");
            assert_eq!(s, "h");
        });
    }

    #[test]
    fn parse_utf16_be_text() {
        with_py(|py| {
            let node = GreedyStringNode::new(Encoding::Utf16Be);
            // "h" in UTF-16 BE = b'\x00h'
            let mut stream = ParseStream::new(b"\x00h");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let s: String = result.bind(py).extract().expect("extract");
            assert_eq!(s, "h");
        });
    }

    #[test]
    fn parse_utf16_le_does_not_consume_bom() {
        // P2a 验证：byteorder=-1 不扫描 BOM。
        // 数据流开头恰好是 \xff\xfe（LE BOM 字节序列），应保留为数据。
        // "h" + LE BOM 字节 = b'\xff\xfeh\x00'，
        // UTF-16 LE 解码 b'\xff\xfe' = U+FEFF (BOM 字符) + b'h\x00' = U+0068 (h)
        with_py(|py| {
            let node = GreedyStringNode::new(Encoding::Utf16Le);
            let mut stream = ParseStream::new(b"\xff\xfeh\x00");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let s: String = result.bind(py).extract().expect("extract");
            // 第一个字符是 U+FEFF（BOM 字符作为数据保留），第二个是 h
            let chars: Vec<char> = s.chars().collect();
            assert_eq!(chars.len(), 2);
            assert_eq!(chars[0], '\u{FEFF}');
            assert_eq!(chars[1], 'h');
        });
    }

    #[test]
    fn parse_utf32_le_text() {
        with_py(|py| {
            let node = GreedyStringNode::new(Encoding::Utf32Le);
            // "X" in UTF-32 LE = b'X\x00\x00\x00'
            let mut stream = ParseStream::new(b"X\x00\x00\x00");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let s: String = result.bind(py).extract().expect("extract");
            assert_eq!(s, "X");
        });
    }

    // ======================================================================
    // build
    // ======================================================================

    #[test]
    fn build_utf8_writes_string_bytes() {
        with_py(|py| {
            let node = GreedyStringNode::new(Encoding::Utf8);
            let obj = py.eval_bound("'hello'", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), b"hello");
        });
    }

    #[test]
    fn build_utf8_empty_string() {
        with_py(|py| {
            let node = GreedyStringNode::new(Encoding::Utf8);
            let obj = py.eval_bound("''", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert!(stream.as_bytes().is_empty());
        });
    }

    #[test]
    fn build_utf16_le_writes_no_bom() {
        // 验证 encode_utf16_32_raw 剥离 BOM（PyUnicode_AsUTF16String 输出含本机序 BOM）
        with_py(|py| {
            let node = GreedyStringNode::new(Encoding::Utf16Le);
            let obj = py.eval_bound("'h'", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            // "h" UTF-16 LE = b'h\x00'（无 BOM）
            assert_eq!(stream.as_bytes(), b"h\x00");
        });
    }

    #[test]
    fn build_utf16_be_writes_no_bom() {
        with_py(|py| {
            let node = GreedyStringNode::new(Encoding::Utf16Be);
            let obj = py.eval_bound("'h'", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            // "h" UTF-16 BE = b'\x00h'（无 BOM）
            assert_eq!(stream.as_bytes(), b"\x00h");
        });
    }

    #[test]
    fn build_non_str_returns_string_error() {
        with_py(|py| {
            let node = GreedyStringNode::new(Encoding::Utf8);
            let obj = py.eval_bound("b'bytes'", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::String { .. }));
        });
    }

    // ======================================================================
    // parse ↔ build 往返
    // ======================================================================

    #[test]
    fn round_trip_utf8_preserves_data() {
        with_py(|py| {
            let node = GreedyStringNode::new(Encoding::Utf8);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            let obj = py.eval_bound("'Афон'", None, None).expect("eval");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();

            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let s: String = result.bind(py).extract().expect("extract");
            assert_eq!(s, "Афон");
        });
    }

    #[test]
    fn round_trip_utf16_be_preserves_data() {
        with_py(|py| {
            let node = GreedyStringNode::new(Encoding::Utf16Be);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            let obj = py.eval_bound("'hello world'", None, None).expect("eval");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();

            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let s: String = result.bind(py).extract().expect("extract");
            assert_eq!(s, "hello world");
        });
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_returns_err() {
        with_py(|py| {
            let node = GreedyStringNode::new(Encoding::Utf8);
            let ctx = Context::new_root(py).expect("ctx");
            let err = node.sizeof(&ctx).expect_err("should fail");
            assert!(matches!(err, ConstructError::Generic { .. }));
        });
    }
}
