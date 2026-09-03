//! PrefixedNode：长度前缀子流节点。
//!
//! Python 参考：`construct/construct/core.py` `Prefixed`（L4862-4918）。
//!
//! ## 行为概述
//!
//! Prefixed 用 lengthfield 给出字节数，subcon 在该子流上处理。
//! parse 读 N 字节作为子流，subcon 在子流上 parse；build 创建 temp BuildStream
//! 收集 subcon 输出，再写 lengthfield + data 到主流。
//!
//! ## 与 PrefixedArrayNode 的关系
//!
//! 两者独立 Node，不共享代码：
//! - [`crate::nodes::prefixed_array::PrefixedArrayNode`]：lengthfield 是**元素计数**，
//!   parse 循环 N 次 inner.parse
//! - [`PrefixedNode`]：lengthfield 是**字节计数**，parse 读 N 字节作为子流，
//!   subcon.parse 整个子流
//!
//! Prefixed 是 PrefixedArray 的"字节计数 + 单元素"兄弟。
//!
//! ## subcon 错误路径不加 push_path_segment
//!
//! PrefixedNode 的 subcon 在子流上 parse，错误直接传播（不附加 path segment）。
//! lengthfield 错误时仍加 "lengthfield" segment（与 PrefixedArray 同模式）。

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

use super::Construct;
use crate::nodes::Node;

/// 长度前缀子流节点：lengthfield 给出字节数，subcon 在该子流上处理。
///
/// 对应 Python construct `Prefixed(lengthfield, subcon, includelength=False)`
/// （core.py L4862）。
///
/// # 三方法行为
///
/// - parse：lengthfield.parse → 得 length（PyLong → i64）→ if includelength 减
///   lengthfield.sizeof → `stream.read(length)` 取子切片 → `ParseStream::new(slice)`
///   → subcon.parse(子流)
/// - build：创建 temp BuildStream → subcon.build(temp) → `data = temp.into_bytes()` →
///   length = data.len()（+ lengthfield.sizeof if includelength）→ lengthfield.build(length)
///   → 主 stream.write(data)
/// - sizeof：lengthfield.sizeof + subcon.sizeof（includelength 不影响 sizeof 总和）
///
/// # 与 PrefixedArrayNode 的关系
///
/// 见模块级文档。两者独立 Node，不共享代码。PrefixedArray 是元素计数 + 循环；
/// Prefixed 是字节计数 + 子流。
///
/// # subcon 错误路径
///
/// PrefixedNode 的 subcon 在子流上 parse，错误直接传播（**不附加 path segment**）。
/// lengthfield 错误时仍加 "lengthfield" segment（与 PrefixedArray 同模式）。
#[derive(Debug)]
pub struct PrefixedNode {
    /// 长度字段（任意能产生整数的 Node，如 VarInt / Byte / Int16ub）。
    lengthfield: Box<Node>,
    /// 子树（在子流上 parse/build）。
    subcon: Box<Node>,
    /// length 是否包含 lengthfield 自身大小（Python 默认 False）。
    includelength: bool,
}

impl PrefixedNode {
    /// 创建 `PrefixedNode`。
    pub fn new(lengthfield: Node, subcon: Node, includelength: bool) -> Self {
        Self {
            lengthfield: Box::new(lengthfield),
            subcon: Box::new(subcon),
            includelength,
        }
    }

    /// 返回 lengthfield 子树引用。
    pub fn lengthfield(&self) -> &Node {
        &self.lengthfield
    }

    /// 返回 subcon 子树引用。
    pub fn subcon(&self) -> &Node {
        &self.subcon
    }

    /// 返回 includelength 标志。
    pub fn includelength(&self) -> bool {
        self.includelength
    }

    /// has_expressions：lengthfield 或 subcon 含表达式。
    pub fn has_expressions(&self) -> bool {
        self.lengthfield.has_expressions() || self.subcon.has_expressions()
    }
}

impl Construct for PrefixedNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 1. 解析 lengthfield 得到 length（对齐 PrefixedArray 错误处理）。
        let length_obj = self
            .lengthfield
            .parse(py, stream, ctx, path)
            .map_err(|mut e| {
                e.push_path_segment("lengthfield");
                e
            })?;
        let mut length_i64: i64 =
            length_obj
                .bind(py)
                .extract()
                .map_err(|_| ConstructError::Range {
                    message: "Prefixed lengthfield did not produce an integer".to_string(),
                    path: path.to_string(),
                })?;
        if length_i64 < 0 {
            return Err(ConstructError::Range {
                message: format!("invalid Prefixed length {}", length_i64),
                path: path.to_string(),
            });
        }
        // 2. if includelength，减去 lengthfield.sizeof。
        if self.includelength {
            let lf_size = self.lengthfield.sizeof(ctx).map_err(|mut e| {
                e.push_path_segment("lengthfield");
                e
            })? as i64;
            length_i64 -= lf_size;
            if length_i64 < 0 {
                return Err(ConstructError::Range {
                    message: format!(
                        "Prefixed length {} shorter than lengthfield size {}",
                        length_i64 + lf_size,
                        lf_size
                    ),
                    path: path.to_string(),
                });
            }
        }
        let length = length_i64 as usize;
        // 3. 读 length 字节作为子流（对齐 Python BytesIOWithOffsets.from_reading）。
        let slice = stream.read(length, path)?;
        let mut sub_stream = ParseStream::new(slice);
        // 4. subcon 在子流上 parse（path 不变，子流错误直接传播——不加 segment）。
        //    注意：subcon 未消费完的字节被忽略（对齐 Python "忽略剩余"语义）。
        self.subcon.parse(py, &mut sub_stream, ctx, path)
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        // 1. 创建 temp BuildStream，subcon.build 到 temp（对齐 PrefixedArray build 模式）。
        let mut temp = BuildStream::new();
        // subcon 错误不加 push_path_segment（与 parse 对称）
        self.subcon.build(py, obj, &mut temp, ctx, path)?;
        let data = temp.into_bytes();
        // 2. 计算 length（+ lengthfield.sizeof if includelength）。
        let mut length = data.len() as i64;
        if self.includelength {
            length += self.lengthfield.sizeof(ctx).map_err(|mut e| {
                e.push_path_segment("lengthfield");
                e
            })? as i64;
        }
        // 3. 先 build lengthfield（写入长度，对齐 PrefixedArray：溢出由 lengthfield 自报）。
        let length_py = length.into_py(py);
        if let Err(mut e) = self
            .lengthfield
            .build(py, length_py.bind(py), stream, ctx, path)
        {
            e.push_path_segment("lengthfield");
            return Err(e);
        }
        // 4. 写入 data 到主流。
        stream.write(&data);
        Ok(())
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // 对齐 Python _sizeof：lengthfield.sizeof + subcon.sizeof（includelength 不影响总和，
        // 因为 includelength 仅调整 length 值，不改变两字段总字节数）。
        let lf = self.lengthfield.sizeof(ctx).map_err(|mut e| {
            e.push_path_segment("lengthfield");
            e
        })?;
        let sub = self.subcon.sizeof(ctx)?;
        Ok(lf + sub)
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
    use crate::nodes::greedy_bytes::GreedyBytesNode;
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

    /// Byte（Int8ub）节点。
    fn byte_node() -> Node {
        Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big))
    }

    /// Int16ub 节点（2 字节大端无符号）。
    fn int16ub_node() -> Node {
        Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big))
    }

    /// GreedyBytes 节点。
    fn greedy_bytes_node() -> Node {
        Node::GreedyBytes(GreedyBytesNode::new())
    }

    /// Bytes(N) 节点。
    fn bytes_node(n: usize) -> Node {
        Node::Bytes(BytesNode::new_const(n))
    }

    // ======================================================================
    // 构造器 & 访问器
    // ======================================================================

    #[test]
    fn new_stores_lengthfield_subcon_includelength() {
        let node = PrefixedNode::new(byte_node(), bytes_node(3), false);
        assert!(matches!(node.lengthfield(), Node::FormatField(_)));
        assert!(matches!(node.subcon(), Node::Bytes(_)));
        assert!(!node.includelength());
    }

    #[test]
    fn new_with_includelength_true() {
        let node = PrefixedNode::new(int16ub_node(), bytes_node(3), true);
        assert!(node.includelength());
    }

    #[test]
    fn has_expressions_no_expr_returns_false() {
        let node = PrefixedNode::new(byte_node(), bytes_node(3), false);
        assert!(!node.has_expressions());
    }

    // ======================================================================
    // parse — PF1: 基本 parse
    // ======================================================================

    #[test]
    fn parse_pf1_basic() {
        // Prefixed(Byte, Bytes(3)) on b"\x03abc" → lengthfield=3，读 "abc"
        with_py(|py| {
            let node = PrefixedNode::new(byte_node(), bytes_node(3), false);
            let mut stream = ParseStream::new(b"\x03abc");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(bytes, b"abc");
            // 流已消费 1+3=4 字节
            assert_eq!(stream.tell(), 4);
        });
    }

    // ======================================================================
    // parse — PF2: subcon 消费少于 length（忽略剩余）
    // ======================================================================

    #[test]
    fn parse_pf2_subcon_consumes_less_than_length() {
        // Prefixed(Byte, Bytes(1)) on b"\x05abcde" → subcon 读 1 字节，剩 4 字节忽略
        with_py(|py| {
            let node = PrefixedNode::new(byte_node(), bytes_node(1), false);
            let mut stream = ParseStream::new(b"\x05abcde");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(bytes, b"a");
            // 主流 pos 推进 1+5=6（lengthfield + 子流全部字节）
            assert_eq!(stream.tell(), 6);
        });
    }

    // ======================================================================
    // parse — PF3: subcon 消费多于 length（子流 EOF）
    // ======================================================================

    #[test]
    fn parse_pf3_subcon_consumes_more_than_length() {
        // Prefixed(Byte, Bytes(10)) on b"\x03abc" → subcon 在 3 字节子流上 EOF
        with_py(|py| {
            let node = PrefixedNode::new(byte_node(), bytes_node(10), false);
            let mut stream = ParseStream::new(b"\x03abc");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Stream { message, .. } => {
                    assert!(message.contains("expected 10"), "got: {}", message);
                }
                other => panic!("expected Stream, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // parse — PF4: includelength=True
    // ======================================================================

    #[test]
    fn parse_pf4_includelength_true() {
        // Prefixed(Int16ub, Bytes(N), includelength=True)
        // lengthfield=5（3 字节 + 2 字节 lengthfield），读 3 字节
        with_py(|py| {
            let node = PrefixedNode::new(int16ub_node(), greedy_bytes_node(), true);
            // lengthfield Int16ub 解出 5，减 sizeof(Int16ub)=2 → 子流 3 字节
            let mut stream = ParseStream::new(b"\x00\x05abc");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(bytes, b"abc");
            assert_eq!(stream.tell(), 5);
        });
    }

    // ======================================================================
    // parse — PF5: lengthfield 负数
    // ======================================================================

    #[test]
    fn parse_pf5_lengthfield_negative() {
        // Prefixed(Int8sb, Bytes(3)) on b"\xff..." → length=-1 → Range Err
        with_py(|py| {
            let signed_byte = Node::FormatField(FormatFieldNode::new(PythonFormat::SignedInt8Big));
            let node = PrefixedNode::new(signed_byte, bytes_node(3), false);
            let mut stream = ParseStream::new(b"\xff\x01\x02\x03");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Range { message, .. } => {
                    assert!(message.contains("-1"), "got: {}", message);
                }
                other => panic!("expected Range, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // parse — PF6: lengthfield 非整数
    // ======================================================================

    #[test]
    fn parse_pf6_lengthfield_non_integer() {
        // Prefixed(Bytes(2), Bytes(3)) → lengthfield 返回 bytes 而非 int → Range Err
        with_py(|py| {
            let node = PrefixedNode::new(bytes_node(2), bytes_node(3), false);
            let mut stream = ParseStream::new(b"\xaa\xbb\x01\x02");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Range { message, .. } => {
                    assert!(
                        message.contains("did not produce an integer"),
                        "got: {}",
                        message
                    );
                }
                other => panic!("expected Range, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // build — PF7: 基本路径
    // ======================================================================

    #[test]
    fn build_pf7_basic() {
        // Prefixed(Byte, Bytes(3)).build(b"abc") → b"\x03abc"
        with_py(|py| {
            let node = PrefixedNode::new(byte_node(), bytes_node(3), false);
            let obj = py.eval_bound("b'abc'", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();
            assert_eq!(bytes, b"\x03abc");
        });
    }

    // ======================================================================
    // build — PF8: lengthfield 溢出由 lengthfield 自报
    // ======================================================================

    #[test]
    fn build_pf8_lengthfield_overflow() {
        // Prefixed(Byte, Bytes(N)).build N=300 → lengthfield Byte 抛错（300 > 255）
        with_py(|py| {
            let node = PrefixedNode::new(byte_node(), bytes_node(300), false);
            // Bytes(300) 需要 300 字节输入
            let long_bytes = py.eval_bound("b'X' * 300", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .build(py, &long_bytes, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            let p = err.path().unwrap_or("");
            assert!(
                p.contains("lengthfield"),
                "expected 'lengthfield' in path, got: {}",
                p
            );
        });
    }

    // ======================================================================
    // sizeof — PF9
    // ======================================================================

    #[test]
    fn sizeof_pf9_sum_of_lengthfield_and_subcon() {
        // Prefixed(Int16ub, Bytes(3)).sizeof() = 2 + 3 = 5
        with_py(|py| {
            let node = PrefixedNode::new(int16ub_node(), bytes_node(3), false);
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).expect("sizeof"), 5);
        });
    }

    // ======================================================================
    // sizeof — PF10: subcon 不可知
    // ======================================================================

    #[test]
    fn sizeof_pf10_subcon_unknown_returns_err() {
        // Prefixed(Byte, GreedyBytes).sizeof() → subcon.sizeof 失败
        with_py(|py| {
            let node = PrefixedNode::new(byte_node(), greedy_bytes_node(), false);
            let ctx = Context::placeholder(py);
            let err = node.sizeof(&ctx).expect_err("should fail");
            // GreedyBytes sizeof 失败 → 错误传播
            assert!(matches!(err, ConstructError::Generic { .. }));
        });
    }

    // ======================================================================
    // build — includelength=True
    // ======================================================================

    #[test]
    fn build_includelength_true() {
        // Prefixed(Int16ub, Bytes(3), includelength=True).build(b"abc")
        // length = 3 + 2 = 5（Int16ub sizeof）→ b"\x00\x05abc"
        with_py(|py| {
            let node = PrefixedNode::new(int16ub_node(), bytes_node(3), true);
            let obj = py.eval_bound("b'abc'", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();
            assert_eq!(bytes, b"\x00\x05abc");
        });
    }

    // ======================================================================
    // round trip
    // ======================================================================

    #[test]
    fn round_trip_basic_preserves_data() {
        with_py(|py| {
            let node = PrefixedNode::new(byte_node(), bytes_node(3), false);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            // build
            let obj = py.eval_bound("b'xyz'", None, None).expect("obj");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();

            // parse 回来
            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let parsed: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(parsed, b"xyz");
        });
    }

    // ======================================================================
    // Debug
    // ======================================================================

    #[test]
    fn debug_format_includes_prefixed_node() {
        let node = PrefixedNode::new(byte_node(), bytes_node(3), false);
        let s = format!("{:?}", node);
        assert!(s.contains("PrefixedNode"), "got: {}", s);
    }
}
