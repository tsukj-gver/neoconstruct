//! PaddedStringNode：固定长度填充字符串。
//!
//! Python 参考：`construct/construct/core.py` `PaddedString`（L1747-1775）。
//!
//! Python 原版是 `StringEncoded(FixedSized(length, NullStripped(GreedyBytes, pad=...)), encoding)`
//! 三层 macro 嵌套；neoconstruct 独立实现：
//! 内联"固定长度读取 + rstrip pad + decode"，不依赖 FixedSized / NullStripped。

use crate::context::Context;
use crate::error::ConstructError;
use crate::expr::eval_expr_int;
use crate::nodes::bytes::BytesLength;
use crate::nodes::strings::encoding::Encoding;
use crate::nodes::strings::rstrip_pad;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

/// 固定长度填充字符串节点（独立实现，不依赖 FixedSized / NullStripped）。
///
/// 对应 Python construct `PaddedString(length, encoding)`（core.py L1747）。
///
/// # parse 行为
///
/// 1. 求值 length（常量 / 表达式）
/// 2. 读 length 字节
/// 3. 右剥离 pad（[`rstrip_pad`]，pad = encoding 单元的全零字节串）
/// 4. [`Encoding::decode`] → `PyString`
///
/// # build 行为
///
/// 1. 校验 `str` → [`Encoding::encode`] → 字节序列
/// 2. 若 encoded.len() > length：[`ConstructError::Padding`]（对齐 Python FixedSized "negative padding"）
/// 3. 写 encoded + 写 (length - encoded.len()) 个 pad 字节（按 unit 循环填充）
///
/// # sizeof
///
/// - [`BytesLength::Const`] → `Ok(length)`
/// - [`BytesLength::Expr`] → `Err`（表达式长度 sizeof 编译期未知，对齐 BytesNode 行为）
#[derive(Debug, Clone)]
pub struct PaddedStringNode {
    /// 总长度（含数据 + pad），可为常量或表达式。
    length: BytesLength,
    /// 编码（编译期从用户字符串解析）。
    encoding: Encoding,
    /// pad 字节串（默认 = encoding.default_term()，即 unit_size 个 0x00）。
    /// 对齐 Python `encodingunit(encoding)` 返回的 bytes。
    pad: Vec<u8>,
}

impl PaddedStringNode {
    /// 创建 `PaddedStringNode`，pad 默认 = encoding 单元的全零字节串。
    pub fn new(length: BytesLength, encoding: Encoding) -> Self {
        Self {
            pad: encoding.default_term(),
            length,
            encoding,
        }
    }

    /// 显式 pad 构造（Python 兼容，但 PaddedString 原版不暴露 pad 参数）。
    pub fn with_pad(length: BytesLength, encoding: Encoding, pad: Vec<u8>) -> Self {
        Self {
            length,
            encoding,
            pad,
        }
    }

    /// 返回长度来源引用。
    pub fn length(&self) -> &BytesLength {
        &self.length
    }

    /// 返回编码。
    pub fn encoding(&self) -> Encoding {
        self.encoding
    }

    /// 返回 pad 字节切片。
    pub fn pad(&self) -> &[u8] {
        &self.pad
    }

    /// has_expressions：Expr 长度时返回 true（StructNode 编译期检查用）。
    pub fn has_expressions(&self) -> bool {
        matches!(self.length, BytesLength::Expr(_))
    }

    /// 求值 length（parse/build 共用）。负数表达式返回 FieldLength 错误。
    fn eval_length(
        &self,
        ctx: &mut Context<'_>,
        py: Python<'_>,
        path: &Path,
    ) -> Result<usize, ConstructError> {
        match &self.length {
            BytesLength::Const(n) => Ok(*n),
            BytesLength::Expr(prog) => {
                let n = eval_expr_int(prog, ctx, py).map_err(|e| ConstructError::Generic {
                    message: format!(
                        "PaddedString length expression evaluation failed: {}",
                        e.full_message()
                    ),
                    path: path.to_string(),
                })?;
                if n < 0 {
                    return Err(ConstructError::FieldLength {
                        message: format!(
                            "PaddedString length expression evaluated to negative: {}",
                            n
                        ),
                        path: path.to_string(),
                    });
                }
                Ok(n as usize)
            }
        }
    }
}

impl crate::nodes::Construct for PaddedStringNode {
    fn parse(
        &self,
        py: Python<'_>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let length = self.eval_length(ctx, py, path)?;
        let data = stream.read(length, path)?;
        let stripped = rstrip_pad(data, &self.pad);
        let py_str = self.encoding.decode(py, stripped, path)?;
        Ok(py_str.into_any().unbind())
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        let length = self.eval_length(ctx, py, path)?;
        let encoded = self.encoding.encode(py, obj, path)?;
        if encoded.len() > length {
            // 对齐 Python FixedSized._build "subcon build %d bytes but was allowed only %d"。
            return Err(ConstructError::Padding {
                message: format!(
                    "PaddedString build {} bytes but was allowed only {}",
                    encoded.len(),
                    length
                ),
                path: path.to_string(),
            });
        }
        stream.write(&encoded);
        // 补 pad 到 length（对齐 FixedSized._build: stream_write(stream, bytes(pad), pad, path)）。
        let pad_len = length - encoded.len();
        if pad_len > 0 {
            let unit = self.pad.len();
            let mut padding = Vec::with_capacity(pad_len);
            for i in 0..pad_len {
                // 多字节 pad 按 unit 循环填充（对齐 Python bytes(pad * (pad_len // unit) + pad[:pad_len % unit])）
                padding.push(self.pad[i % unit]);
            }
            stream.write(&padding);
        }
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        match &self.length {
            BytesLength::Const(n) => Ok(*n),
            BytesLength::Expr(_) => Err(ConstructError::Generic {
                message: "PaddedString with expression length has no static size".to_string(),
                path: String::new(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{ExprOp, ExprProgram};
    use crate::nodes::Construct;
    use pyo3::types::PyString;

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

    /// 在 ctx 中设置字段值 + expr_values，模拟 StructNode parse 环境。
    fn setup_ctx_for_expr<'py>(py: Python<'py>, field_values: &[(&str, i64)]) -> Context<'py> {
        let mut ctx = Context::new_root(py).expect("ctx");
        ctx.init_expr_values(field_values.len());
        for (idx, (name, value)) in field_values.iter().enumerate() {
            let key = PyString::new_bound(py, name).unbind();
            let val = (*value).into_py(py);
            ctx.set_field_at(idx, &key, val.bind(py), py)
                .expect("set_field_at");
        }
        ctx
    }

    // ======================================================================
    // 构造器 & 访问器
    // ======================================================================

    #[test]
    fn new_const_sets_default_pad_for_utf8() {
        let node = PaddedStringNode::new(BytesLength::Const(10), Encoding::Utf8);
        assert_eq!(node.encoding(), Encoding::Utf8);
        assert_eq!(node.pad(), b"\x00");
        assert!(!node.has_expressions());
    }

    #[test]
    fn new_const_sets_default_pad_for_utf16() {
        let node = PaddedStringNode::new(BytesLength::Const(10), Encoding::Utf16Le);
        assert_eq!(node.pad(), b"\x00\x00");
    }

    #[test]
    fn new_expr_has_expressions() {
        let prog = ExprProgram::new(vec![ExprOp::Const(5)]);
        let node = PaddedStringNode::new(BytesLength::Expr(prog), Encoding::Utf8);
        assert!(node.has_expressions());
    }

    // ======================================================================
    // parse：Const
    // ======================================================================

    #[test]
    fn parse_const_basic_with_trailing_pad() {
        with_py(|py| {
            let node = PaddedStringNode::new(BytesLength::Const(10), Encoding::Utf8);
            // "hello" + 5 个 pad
            let mut stream = ParseStream::new(b"hello\x00\x00\x00\x00\x00");
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
    fn parse_const_exact_length_no_pad() {
        with_py(|py| {
            let node = PaddedStringNode::new(BytesLength::Const(5), Encoding::Utf8);
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
    fn parse_const_all_pad_returns_empty() {
        with_py(|py| {
            let node = PaddedStringNode::new(BytesLength::Const(3), Encoding::Utf8);
            let mut stream = ParseStream::new(b"\x00\x00\x00");
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
    fn parse_const_zero_length_returns_empty() {
        with_py(|py| {
            let node = PaddedStringNode::new(BytesLength::Const(0), Encoding::Utf8);
            let mut stream = ParseStream::new(b"abc");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let s: String = result.bind(py).extract().expect("extract");
            assert!(s.is_empty());
            assert_eq!(stream.tell(), 0);
        });
    }

    #[test]
    fn parse_const_multibyte_utf8() {
        with_py(|py| {
            let node = PaddedStringNode::new(BytesLength::Const(10), Encoding::Utf8);
            // "Афон" UTF-8 = 8 字节 + 2 个 pad
            let mut data = "Афон".as_bytes().to_vec();
            data.extend_from_slice(b"\x00\x00");
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
    fn parse_const_insufficient_bytes_returns_stream_error() {
        with_py(|py| {
            let node = PaddedStringNode::new(BytesLength::Const(5), Encoding::Utf8);
            let mut stream = ParseStream::new(b"abc");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Stream { .. }));
        });
    }

    // ======================================================================
    // parse：Expr
    // ======================================================================

    #[test]
    fn parse_expr_length_resolves_at_runtime() {
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0)]);
            let node = PaddedStringNode::new(BytesLength::Expr(prog), Encoding::Utf8);
            let mut stream = ParseStream::new(b"hi\x00\x00\x00");
            let mut ctx = setup_ctx_for_expr(py, &[("n", 5)]);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let s: String = result.bind(py).extract().expect("extract");
            assert_eq!(s, "hi");
        });
    }

    #[test]
    fn parse_expr_negative_returns_field_length_error() {
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::Const(-1)]);
            let node = PaddedStringNode::new(BytesLength::Expr(prog), Encoding::Utf8);
            let mut stream = ParseStream::new(b"abc");
            let mut ctx = setup_ctx_for_expr(py, &[]);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::FieldLength { .. }));
        });
    }

    // ======================================================================
    // build
    // ======================================================================

    #[test]
    fn build_const_pads_to_length() {
        with_py(|py| {
            let node = PaddedStringNode::new(BytesLength::Const(10), Encoding::Utf8);
            let obj = py.eval_bound("'hi'", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), b"hi\x00\x00\x00\x00\x00\x00\x00\x00");
            assert_eq!(stream.as_bytes().len(), 10);
        });
    }

    #[test]
    fn build_const_exact_length_no_pad() {
        with_py(|py| {
            let node = PaddedStringNode::new(BytesLength::Const(5), Encoding::Utf8);
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
    fn build_const_zero_length_accepts_empty_string() {
        with_py(|py| {
            let node = PaddedStringNode::new(BytesLength::Const(0), Encoding::Utf8);
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
    fn build_const_encoded_exceeds_length_returns_padding_error() {
        with_py(|py| {
            let node = PaddedStringNode::new(BytesLength::Const(3), Encoding::Utf8);
            let obj = py.eval_bound("'hello'", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Padding { .. }));
        });
    }

    #[test]
    fn build_non_str_returns_string_error() {
        with_py(|py| {
            let node = PaddedStringNode::new(BytesLength::Const(5), Encoding::Utf8);
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
    // round-trip
    // ======================================================================

    #[test]
    fn round_trip_const_preserves_data() {
        with_py(|py| {
            let node = PaddedStringNode::new(BytesLength::Const(10), Encoding::Utf8);
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

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_const_returns_length() {
        with_py(|py| {
            let node = PaddedStringNode::new(BytesLength::Const(7), Encoding::Utf8);
            let ctx = Context::new_root(py).expect("ctx");
            assert_eq!(node.sizeof(&ctx).unwrap(), 7);
        });
    }

    #[test]
    fn sizeof_expr_returns_error() {
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::Const(5)]);
            let node = PaddedStringNode::new(BytesLength::Expr(prog), Encoding::Utf8);
            let ctx = Context::new_root(py).expect("ctx");
            assert!(node.sizeof(&ctx).is_err());
        });
    }
}
