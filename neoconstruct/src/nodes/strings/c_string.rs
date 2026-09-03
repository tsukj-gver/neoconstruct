//! CStringNode：C 风格 null 终止字符串。
//!
//! Python 参考：`construct/construct/core.py` `CString`（L1811-1834）。
//!
//! Python 原版是 `StringEncoded(NullTerminated(GreedyBytes, term=encodingunit(encoding)), encoding)`
//! macro 嵌套；neoconstruct 独立实现：直接扫描 term + decode。

use crate::context::Context;
use crate::error::ConstructError;
use crate::nodes::strings::encoding::Encoding;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

/// C 风格 null 终止字符串节点。
///
/// 对应 Python construct `CString(encoding)`（core.py L1811）。
///
/// # parse 行为
///
/// 1. 从流中逐 unit_size 字节扫描，直到遇到 `term` 字节串
/// 2. 消费 term（不剥离 = 默认 include=False；include=True 时保留）
/// 3. 剩余字节（不含 term）调 [`Encoding::decode`] → `PyString`
/// 4. EOF 前未遇到 term：
///    - `require=true`（默认）→ [`ConstructError::Stream`]
///    - `require=false` → 读到 EOF 的所有字节解码（无 term）
///
/// # build 行为
///
/// 1. 校验输入是 `str` → [`Encoding::encode`] → 字节序列
/// 2. 写字节序列 + 写 term
///
/// # sizeof
///
/// 永远 `Err`（字符串长度 + term 长度运行时未知）。
#[derive(Debug, Clone)]
pub struct CStringNode {
    /// 编码（编译期从用户字符串解析为 enum）。
    encoding: Encoding,
    /// 终止符字节串（默认 = encoding.default_term()，即 unit_size 个 0x00）。
    /// 用户可通过 `CString(encoding, term=b"...")` 显式传入（Python 兼容）。
    term: Vec<u8>,
    /// 是否将 term 包含在解码数据中（Python `NullTerminated(include=...)`，默认 false）。
    include: bool,
    /// 是否在 EOF 时报错（Python `NullTerminated(require=...)`，默认 true）。
    require: bool,
}

impl CStringNode {
    /// 默认构造：`CString(encoding)`，term = encoding 单元的全零字节串。
    pub fn new(encoding: Encoding) -> Self {
        Self {
            encoding,
            term: encoding.default_term(),
            include: false,
            require: true,
        }
    }

    /// 全参数构造（Descriptor 编译期调用，对应 Python
    /// `NullTerminated(..., include, consume, require)`，但 CString 不暴露 consume）。
    pub fn with_options(encoding: Encoding, term: Vec<u8>, include: bool, require: bool) -> Self {
        Self {
            encoding,
            term,
            include,
            require,
        }
    }

    /// 返回编码。
    pub fn encoding(&self) -> Encoding {
        self.encoding
    }

    /// 返回 term 字节切片。
    pub fn term(&self) -> &[u8] {
        &self.term
    }

    /// 是否包含 term 在解码数据中。
    pub fn include(&self) -> bool {
        self.include
    }

    /// 是否在 EOF 时报错。
    pub fn require(&self) -> bool {
        self.require
    }
}

impl crate::nodes::Construct for CStringNode {
    fn parse(
        &self,
        py: Python<'_>,
        stream: &mut ParseStream<'_>,
        _ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let unit = self.term.len();
        if unit == 0 {
            return Err(ConstructError::Padding {
                message: "CString term must be at least 1 byte".to_string(),
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
                        break; // 找到 term
                    }
                    accumulated.extend_from_slice(chunk);
                }
                Err(ConstructError::Stream { .. }) if !self.require => {
                    // EOF 且 require=false：用已累积数据
                    break;
                }
                Err(e) => return Err(e), // 其他错误（含 require=true 时 EOF）
            }
        }
        let py_str = self.encoding.decode(py, &accumulated, path)?;
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
        stream.write(&self.term);
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        Err(ConstructError::Generic {
            message: "CString size is undefined".to_string(),
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
    fn new_sets_default_term_for_utf8() {
        let node = CStringNode::new(Encoding::Utf8);
        assert_eq!(node.encoding(), Encoding::Utf8);
        assert_eq!(node.term(), b"\x00");
        assert!(!node.include());
        assert!(node.require());
    }

    #[test]
    fn new_sets_default_term_for_utf16() {
        let node = CStringNode::new(Encoding::Utf16Le);
        assert_eq!(node.term(), b"\x00\x00");
    }

    #[test]
    fn with_options_overrides_defaults() {
        let node = CStringNode::with_options(Encoding::Utf8, b"\xFF".to_vec(), true, false);
        assert_eq!(node.term(), b"\xFF");
        assert!(node.include());
        assert!(!node.require());
    }

    // ======================================================================
    // parse：utf8
    // ======================================================================

    #[test]
    fn parse_utf8_basic() {
        with_py(|py| {
            let node = CStringNode::new(Encoding::Utf8);
            let mut stream = ParseStream::new(b"hello\x00");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let s: String = result.bind(py).extract().expect("extract");
            assert_eq!(s, "hello");
            // term 被消费
            assert!(stream.is_at_end());
        });
    }

    #[test]
    fn parse_utf8_term_at_start_returns_empty() {
        with_py(|py| {
            let node = CStringNode::new(Encoding::Utf8);
            let mut stream = ParseStream::new(b"\x00hello");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let s: String = result.bind(py).extract().expect("extract");
            assert!(s.is_empty());
            // 只消费第一个 \x00
            assert_eq!(stream.tell(), 1);
        });
    }

    #[test]
    fn parse_utf8_eof_no_term_require_true_returns_stream_error() {
        with_py(|py| {
            let node = CStringNode::new(Encoding::Utf8);
            let mut stream = ParseStream::new(b"hello"); // 无 term
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Stream { .. }));
        });
    }

    #[test]
    fn parse_utf8_eof_no_term_require_false_uses_accumulated() {
        with_py(|py| {
            let node = CStringNode::with_options(Encoding::Utf8, b"\x00".to_vec(), false, false);
            let mut stream = ParseStream::new(b"hello"); // 无 term
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
    fn parse_utf8_include_true_keeps_term() {
        with_py(|py| {
            let node = CStringNode::with_options(Encoding::Utf8, b"\x00".to_vec(), true, true);
            let mut stream = ParseStream::new(b"hi\x00");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let s: String = result.bind(py).extract().expect("extract");
            // include=true：term 被包含在解码数据中，b"hi\x00" 解码为 "hi\0"
            assert_eq!(s, "hi\0");
        });
    }

    #[test]
    fn parse_utf8_multibyte_chars() {
        with_py(|py| {
            let node = CStringNode::new(Encoding::Utf8);
            // "Афон" UTF-8 + term
            let mut data = "Афон".as_bytes().to_vec();
            data.push(0x00);
            let mut stream = ParseStream::new(&data);
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
    fn parse_utf16_le_with_two_byte_term() {
        with_py(|py| {
            let node = CStringNode::new(Encoding::Utf16Le);
            // "h" UTF-16 LE = b'h\x00' + term b'\x00\x00'
            let mut stream = ParseStream::new(b"h\x00\x00\x00");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let s: String = result.bind(py).extract().expect("extract");
            assert_eq!(s, "h");
            assert!(stream.is_at_end());
        });
    }

    #[test]
    fn parse_ascii_basic() {
        with_py(|py| {
            let node = CStringNode::new(Encoding::Ascii);
            let mut stream = ParseStream::new(b"hello\x00");
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
    fn parse_empty_term_returns_padding_error() {
        with_py(|py| {
            let node = CStringNode::with_options(Encoding::Utf8, vec![], false, true);
            let mut stream = ParseStream::new(b"hello");
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
    fn build_utf8_writes_data_plus_term() {
        with_py(|py| {
            let node = CStringNode::new(Encoding::Utf8);
            let obj = py.eval_bound("'hello'", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), b"hello\x00");
        });
    }

    #[test]
    fn build_utf8_empty_string_writes_term_only() {
        with_py(|py| {
            let node = CStringNode::new(Encoding::Utf8);
            let obj = py.eval_bound("''", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), b"\x00");
        });
    }

    #[test]
    fn build_non_str_returns_string_error() {
        with_py(|py| {
            let node = CStringNode::new(Encoding::Utf8);
            let obj = py.eval_bound("123", None, None).expect("eval");
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
    // round-trip
    // ======================================================================

    #[test]
    fn round_trip_utf8_preserves_data() {
        with_py(|py| {
            let node = CStringNode::new(Encoding::Utf8);
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
    fn sizeof_returns_err() {
        with_py(|py| {
            let node = CStringNode::new(Encoding::Utf8);
            let ctx = Context::new_root(py).expect("ctx");
            assert!(node.sizeof(&ctx).is_err());
        });
    }
}
