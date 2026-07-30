//! PascalStringNode：长度前缀字符串（Phase 6.2 §3.6）。
//!
//! Python 参考：`construct/construct/core.py` `PascalString`（L1778-1808）。
//!
//! Python 原版是 `StringEncoded(Prefixed(lengthfield, GreedyBytes), encoding)` macro 嵌套；
//! 本设计独立实现（PM 决策 1 方案 A）：内联"lengthfield 解析 + 读 N 字节 + decode"，
//! 不依赖 Prefixed（Phase 7 Streams）。

use crate::context::Context;
use crate::error::ConstructError;
use crate::nodes::strings::encoding::Encoding;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

/// 长度前缀字符串节点（独立实现，不依赖 Prefixed）。
///
/// 对应 Python construct `PascalString(lengthfield, encoding)`（core.py L1778）。
///
/// # parse 行为
///
/// 1. `lengthfield.parse(stream)` → 得到 Python 整数对象
/// 2. extract i64；非整数 → [`ConstructError::Range`]（PA-2 对齐）；负数 → Range（PA-1 对齐）
/// 3. 读 N 字节
/// 4. [`Encoding::decode`] → `PyString`
///
/// # build 行为
///
/// 1. 校验 `str` → [`Encoding::encode`] → 字节序列
/// 2. 取 encoded.len() → build lengthfield（写入长度）
/// 3. 写 encoded 字节
///
/// 对齐 Python：build 时 lengthfield 自动从数据长度计算，不需用户传长度。
///
/// # sizeof
///
/// 永远 `Err`（lengthfield 大小 + 数据长度，后者未知）。
///
/// # 注意
///
/// 不派生 `Clone`：持有 `Box<Node>`（lengthfield），与 `NullTerminatedNode` /
/// `PrefixedArrayNode` 等现有 `Box<Node>` 节点一致。
#[derive(Debug)]
pub struct PascalStringNode {
    /// 长度字段（任意能产生整数的 Node：VarInt / Int16ub / Byte 等）。
    lengthfield: Box<crate::nodes::Node>,
    /// 编码。
    encoding: Encoding,
}

impl PascalStringNode {
    /// 创建 `PascalStringNode`。
    pub fn new(lengthfield: crate::nodes::Node, encoding: Encoding) -> Self {
        Self {
            lengthfield: Box::new(lengthfield),
            encoding,
        }
    }

    /// 返回 lengthfield 子树引用。
    pub fn lengthfield(&self) -> &crate::nodes::Node {
        &self.lengthfield
    }

    /// 返回编码。
    pub fn encoding(&self) -> Encoding {
        self.encoding
    }

    /// has_expressions：递归检查 lengthfield 子树。
    pub fn has_expressions(&self) -> bool {
        self.lengthfield.has_expressions()
    }
}

impl crate::nodes::Construct for PascalStringNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 1. 解析 lengthfield 得到 Python 整数（复用 PrefixedArray 的模式）。
        //    lengthfield 错误时 push_path_segment("lengthfield")（对齐 PrefixedArray）。
        let length_obj = match self.lengthfield.parse(py, stream, ctx, path) {
            Ok(v) => v,
            Err(mut e) => {
                e.push_path_segment("lengthfield");
                return Err(e);
            }
        };
        // 2. extract 为 i64。非整数返回 Range 错误（PA-2 对齐）。
        let length_i64: i64 = length_obj.bind(py).extract().map_err(|_| {
            let type_name = length_obj
                .bind(py)
                .get_type()
                .name()
                .map(|n| n.to_string())
                .unwrap_or_else(|_| "<unknown>".to_string());
            ConstructError::Range {
                message: format!(
                    "PascalString lengthfield returned non-integer value of type {}",
                    type_name
                ),
                path: path.to_string(),
            }
        })?;
        // 3. 负数 count 返回 Range 错误（PA-1 对齐）。
        if length_i64 < 0 {
            return Err(ConstructError::Range {
                message: format!("PascalString length is negative: {}", length_i64),
                path: path.to_string(),
            });
        }
        let length = length_i64 as usize;
        // 4. 读 length 字节。
        let data = stream.read(length, path)?;
        // 5. decode。
        let py_str = self.encoding.decode(py, data, path)?;
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
        // 1. encode 数据。
        let encoded = self.encoding.encode(py, obj, path)?;
        // 2. build lengthfield（传入 encoded.len() 作为 Python int）。
        //    lengthfield 自行检查长度范围（如 Byte 超过 255 报 Range，对齐 PrefixedArray PA-5）。
        let len_obj = (encoded.len() as i64).into_py(py);
        let len_bound = len_obj.bind(py);
        if let Err(mut e) = self.lengthfield.build(py, len_bound, stream, ctx, path) {
            e.push_path_segment("lengthfield");
            return Err(e);
        }
        // 3. 写数据。
        stream.write(&encoded);
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        Err(ConstructError::Generic {
            message: "PascalString size depends on stream data".to_string(),
            path: String::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::format_field::{FormatFieldNode, PythonFormat};
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

    /// 构造 Byte（Int8ub）节点，用作 lengthfield。
    fn byte_node() -> crate::nodes::Node {
        crate::nodes::Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big))
    }

    /// 构造 Int16ub 节点。
    fn int16ub_node() -> crate::nodes::Node {
        crate::nodes::Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big))
    }

    // ======================================================================
    // 构造器 & 访问器
    // ======================================================================

    #[test]
    fn new_stores_lengthfield_and_encoding() {
        let node = PascalStringNode::new(byte_node(), Encoding::Utf8);
        assert_eq!(node.encoding(), Encoding::Utf8);
        assert!(matches!(
            node.lengthfield(),
            crate::nodes::Node::FormatField(_)
        ));
        assert!(!node.has_expressions());
    }

    // ======================================================================
    // parse：utf8 + Byte lengthfield
    // ======================================================================

    #[test]
    fn parse_byte_lengthfield_utf8_basic() {
        with_py(|py| {
            let node = PascalStringNode::new(byte_node(), Encoding::Utf8);
            // length=5 + "hello"
            let mut stream = ParseStream::new(b"\x05hello");
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
    fn parse_byte_lengthfield_zero_returns_empty() {
        with_py(|py| {
            let node = PascalStringNode::new(byte_node(), Encoding::Utf8);
            let mut stream = ParseStream::new(b"\x00");
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
    fn parse_int16ub_lengthfield_utf8() {
        with_py(|py| {
            let node = PascalStringNode::new(int16ub_node(), Encoding::Utf8);
            // length=0x0005 (Int16ub) + "hello"
            let mut stream = ParseStream::new(b"\x00\x05hello");
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
    fn parse_multibyte_utf8() {
        with_py(|py| {
            let node = PascalStringNode::new(byte_node(), Encoding::Utf8);
            // "Афон" UTF-8 = 8 字节
            let mut data = vec![8u8];
            data.extend_from_slice("Афон".as_bytes());
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
    fn parse_insufficient_bytes_returns_stream_error() {
        with_py(|py| {
            let node = PascalStringNode::new(byte_node(), Encoding::Utf8);
            // length=5 但只有 "hi"
            let mut stream = ParseStream::new(b"\x05hi");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Stream { .. }));
        });
    }

    // ======================================================================
    // build
    // ======================================================================

    #[test]
    fn build_byte_lengthfield_utf8() {
        with_py(|py| {
            let node = PascalStringNode::new(byte_node(), Encoding::Utf8);
            let obj = py.eval_bound("'hello'", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), b"\x05hello");
        });
    }

    #[test]
    fn build_empty_string_writes_zero_length() {
        with_py(|py| {
            let node = PascalStringNode::new(byte_node(), Encoding::Utf8);
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
    fn build_byte_lengthfield_overflow_returns_error() {
        // PA-5 对齐：Byte 超过 255 时 lengthfield.build 报错（FormatField 范围检查）
        with_py(|py| {
            let node = PascalStringNode::new(byte_node(), Encoding::Utf8);
            // 300 字节的字符串，Byte lengthfield 装不下
            let long_str = "x".repeat(300);
            let obj = py
                .eval_bound(&format!("'{}'", long_str), None, None)
                .expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            // Byte build 失败会抛 FormatField 或 FieldLength 错误，path 含 lengthfield
            let path_str = err.path().unwrap_or("");
            assert!(path_str.contains("lengthfield"), "got path: {}", path_str);
        });
    }

    #[test]
    fn build_non_str_returns_string_error() {
        with_py(|py| {
            let node = PascalStringNode::new(byte_node(), Encoding::Utf8);
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
    fn round_trip_byte_lengthfield_preserves_data() {
        with_py(|py| {
            let node = PascalStringNode::new(byte_node(), Encoding::Utf8);
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

    #[test]
    fn sizeof_returns_err() {
        with_py(|py| {
            let node = PascalStringNode::new(byte_node(), Encoding::Utf8);
            let ctx = Context::new_root(py).expect("ctx");
            assert!(node.sizeof(&ctx).is_err());
        });
    }
}
