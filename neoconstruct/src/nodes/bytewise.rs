//! BytewiseNode：bit→byte 适配器。
//!
//! Python 参考：`construct/construct/core.py` `Bytewise(subcon)`（L1081）。
//!
//! ## 职责
//!
//! BytewiseNode 在 bit 域（BitwiseNode 内）为内部 subcon 重建字节对齐的子流。
//! 必须在 Bitwise 域内使用。
//!
//! ## parse 流程（两条路径）
//!
//! **对齐快路径**（`stream.bit_pos() == 0`）：直接 `inner.parse(stream, ...)`——
//! inner 是字节节点（FormatField/Bytes/Struct），在同一 stream 上字节级读取，
//! 零额外开销。这是 `Bytewise(Int16ub)` 在 BitStruct 字节边界处的常见快路径。
//!
//! **未对齐慢路径**（`stream.bit_pos() != 0`）：
//! 1. `size = inner.sizeof(ctx)?`（字节节点必须有定长，否则 Err，对齐 Python
//!    Transformed 路径要求定长）。
//! 2. 从 bit 流提取 `size * 8` bit 到临时 `Vec<u8>` 缓冲（逐 bit 组装）。
//! 3. 在临时缓冲上创建 `ParseStream`，`inner.parse(&mut sub_stream, ...)`。
//! 4. 校验 sub_stream 被完全消耗（对齐 Python stream_tell 测量）。
//!
//! ## build 流程（对称）
//!
//! **对齐快路径**：直接 `inner.build(obj, stream, ...)`。
//! **未对齐慢路径**：inner build 到临时 `BuildStream` → 提取字节 → 逐 bit 写入主流。
//!
//! ## sizeof
//!
//! `inner.sizeof(ctx)? * 8`——inner 报字节数，外层报 bit 数。

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

use super::Construct;
use crate::nodes::Node;

/// bit→byte 适配器节点：在 bit 域内为内部 subcon 重建字节对齐的子流。
///
/// 详见模块级文档。
///
/// # 错误
///
/// - inner.sizeof() 返回 Err（变长 subcon）→ parse/build 返回 `BitField` 错误
/// - bit 对齐（bit_pos==0）→ 走快路径，零额外开销
/// - bit 未对齐（bit_pos!=0）→ 走慢路径，逐 bit 提取到临时缓冲
/// - 在 Bitwise 外使用 → 已知差异，Python 抛 KeyError，neoconstruct 走对齐快路径
#[derive(Debug)]
pub struct BytewiseNode {
    /// 字节级子树根（FormatField/Bytes/Struct/StructRef 等）。
    inner: Box<Node>,
}

impl BytewiseNode {
    /// 创建 `BytewiseNode`，包裹给定的字节级子树根节点。
    ///
    /// # 参数
    ///
    /// - `inner`：被包裹的字节级子树（通常是 `Node::FormatField`、`Node::Bytes` 等）。
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

impl Construct for BytewiseNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        if stream.is_byte_aligned() {
            // 对齐快路径——直接委托 inner 在同一 stream 上读取。
            // inner 是字节节点，bit_pos==0 时字节级 read 可用。
            return self.inner.parse(py, stream, ctx, path);
        }

        // 未对齐慢路径——从 bit 流提取到临时字节缓冲。
        // inner 必须有定长 sizeof。
        let size = self.inner.sizeof(ctx).map_err(|e| {
            // 把 inner 的 sizeof 错误转换为 BitField。
            ConstructError::BitField {
                message: format!(
                    "Bytewise requires a fixed-sized subcon (inner sizeof failed: {})",
                    e.full_message()
                ),
                path: path.to_string(),
            }
        })?;

        // 从 bit 流读取 size*8 bit 到字节缓冲。
        // 读到 u64 后转字节（每次最多 8 字节，避免 u64 溢出）。
        let mut buffer: Vec<u8> = Vec::with_capacity(size);
        let mut remaining_bits = size.saturating_mul(8);
        while remaining_bits >= 64 {
            let chunk = stream.read_bits(64, path)?;
            buffer.extend_from_slice(&chunk.to_be_bytes());
            remaining_bits -= 64;
        }
        if remaining_bits >= 8 {
            let full_bytes = remaining_bits / 8;
            let bits_to_read = full_bytes * 8;
            let chunk = stream.read_bits(bits_to_read, path)?;
            let bytes = chunk.to_be_bytes();
            // chunk 的有效位在高位（MSB-first），取前 full_bytes 字节
            let skip = 8 - full_bytes;
            buffer.extend_from_slice(&bytes[skip..]);
            remaining_bits -= bits_to_read;
        }
        if remaining_bits > 0 {
            // 剩余不足 8 bit：读到 u64 后取高 remaining_bits 位组成一字节
            let partial = stream.read_bits(remaining_bits, path)?;
            // partial 的有效位在低 remaining_bits 位，需要左对齐到字节高位
            let byte = if remaining_bits < 8 {
                ((partial << (8 - remaining_bits)) & 0xFF) as u8
            } else {
                partial as u8
            };
            buffer.push(byte);
        }

        // 在临时缓冲上创建 ParseStream，inner 解析。
        let mut sub_stream = ParseStream::new(&buffer);
        let result = self.inner.parse(py, &mut sub_stream, ctx, path)?;

        // 校验 sub_stream 完全消耗（对齐 Python stream_tell 测量）。
        // sub_stream 不在 bit 域，bit_pos 应保持 0；tell 应等于 buffer.len()。
        if sub_stream.tell() != buffer.len() || sub_stream.bit_pos() != 0 {
            return Err(ConstructError::BitField {
                message: format!(
                    "Bytewise inner did not consume the entire extracted buffer ({} of {} bytes, bit_pos={})",
                    sub_stream.tell(),
                    buffer.len(),
                    sub_stream.bit_pos()
                ),
                path: path.to_string(),
            });
        }

        Ok(result)
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        if stream.is_byte_aligned() {
            // 对齐快路径——直接委托 inner 在同一 stream 上写入。
            return self.inner.build(py, obj, stream, ctx, path);
        }

        // 未对齐慢路径——inner build 到临时 BuildStream，再逐 bit 写回主流。
        let size = self
            .inner
            .sizeof(ctx)
            .map_err(|e| ConstructError::BitField {
                message: format!(
                    "Bytewise requires a fixed-sized subcon (inner sizeof failed: {})",
                    e.full_message()
                ),
                path: path.to_string(),
            })?;

        // inner build 到独立 stream（从 bit_pos==0 开始）。
        let mut sub_stream = BuildStream::with_capacity(size);
        self.inner.build(py, obj, &mut sub_stream, ctx, path)?;
        let bytes = sub_stream.into_bytes();

        // 校验 inner 写入了正确字节数（防止 inner 行为异常破坏 bit 流）。
        if bytes.len() != size {
            return Err(ConstructError::BitField {
                message: format!(
                    "Bytewise inner build produced {} bytes but sizeof is {}",
                    bytes.len(),
                    size
                ),
                path: path.to_string(),
            });
        }

        // 逐字节写入主流（每字节 8 bit）。stream.write_bits 处理未对齐情况。
        for &b in &bytes {
            stream.write_bits(b as u64, 8);
        }

        Ok(())
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // inner.sizeof() 返回字节数，外层报 bit 数。
        // inner 可能返回 Err（变长 subcon），此时 Bytewise 的 sizeof 也 Err。
        let inner_bytes = self.inner.sizeof(ctx)?;
        Ok(inner_bytes * 8)
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::bits_integer::BitsIntegerNode;
    use crate::nodes::format_field::{FormatFieldNode, PythonFormat};
    use crate::nodes::struct_node::StructNode;
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
    // 构造器
    // ======================================================================

    #[test]
    fn new_stores_inner() {
        let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
        let node = BytewiseNode::new(inner);
        assert!(matches!(node.inner(), Node::FormatField(_)));
    }

    // ======================================================================
    // parse — 对齐快路径
    // ======================================================================

    #[test]
    fn parse_aligned_fast_path_reads_directly() {
        // Bitwise 内对齐位置直接读 2 字节
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
            let node = BytewiseNode::new(inner);

            let mut stream = ParseStream::new(&[0x12, 0x34]);
            // 初始 bit_pos==0（对齐）
            assert!(stream.is_byte_aligned());
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().unwrap();
            assert_eq!(v, 0x1234);
            assert_eq!(stream.tell(), 2);
            assert_eq!(stream.bit_pos(), 0);
        });
    }

    #[test]
    fn parse_aligned_struct_subcon() {
        // Bytewise(Struct{a: Int8ub, b: Int8ub}) 对齐快路径
        with_py(|py| {
            let inner_struct = StructNode::new_for_test(
                py,
                vec![
                    (
                        "a".to_string(),
                        Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big)),
                    ),
                    (
                        "b".to_string(),
                        Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big)),
                    ),
                ],
            );
            let node = BytewiseNode::new(Node::Struct(inner_struct));

            let mut stream = ParseStream::new(&[0xAA, 0xBB]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let inst = result.bind(py);
            let a: i64 = inst.getattr("a").unwrap().extract().unwrap();
            let b: i64 = inst.getattr("b").unwrap().extract().unwrap();
            assert_eq!(a, 0xAA);
            assert_eq!(b, 0xBB);
        });
    }

    // ======================================================================
    // parse — 未对齐慢路径
    // ======================================================================

    #[test]
    fn parse_unaligned_slow_path_extracts_bits() {
        // 先读 4 bit Nibble，再 Bytewise(Int16ub)——bit_pos=4 时慢路径
        // 数据：0xAB 0xCD 0xEF → Nibble=0xA，Bytewise 读 16 bit 跨字节边界
        // bit 4-19 = 0b1011_1100_1101_1110_1111 = 0xBCDE
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
            let node = BytewiseNode::new(inner);

            let mut stream = ParseStream::new(&[0xAB, 0xCD, 0xEF]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            // 先读 4 bit Nibble 使 bit_pos=4
            let _ = stream.read_bits(4, &path).expect("nibble");
            assert_eq!(stream.bit_pos(), 4);

            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().unwrap();
            // bit 4-19 of [0xAB, 0xCD, 0xEF] (MSB-first):
            // 0xAB = 1010_1011 → bit 4-7 = 1011
            // 0xCD = 1100_1101 → bit 8-15 = 1100_1101
            // 0xEF = 1110_1111 → bit 16-19 = 1110
            // 合计：1011_1100_1101_1110 = 0xBCDE
            assert_eq!(v, 0xBCDE);
            assert_eq!(stream.tell(), 2);
            assert_eq!(stream.bit_pos(), 4);
        });
    }

    #[test]
    fn parse_unaligned_single_byte() {
        // 先 Nibble，再 Bytewise(Int8ub)——bit_pos=4 读 8 bit
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let node = BytewiseNode::new(inner);

            let mut stream = ParseStream::new(&[0xA5]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let _ = stream.read_bits(4, &path).expect("nibble");
            // bit_pos=4，读 8 bit：bit 4-7 of 0xA5 + bit 0-3 of next byte (none)
            // 但只有 4 bit 剩余，应失败
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Stream { .. }));
        });
    }

    #[test]
    fn parse_unaligned_multi_byte_struct() {
        // Bytewise(Struct{a: Int8ub, b: Int8ub}) 在 bit_pos=4 慢路径
        // 数据：0x_A 0xBC 0xDE → Nibble(0xA), Bytewise(2 字节)
        // bit 4-11 = 1011_1100 (0xBC), bit 12-19 = 1101_1110 (0xDE)
        with_py(|py| {
            let inner_struct = StructNode::new_for_test(
                py,
                vec![
                    (
                        "a".to_string(),
                        Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big)),
                    ),
                    (
                        "b".to_string(),
                        Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big)),
                    ),
                ],
            );
            let node = BytewiseNode::new(Node::Struct(inner_struct));

            let mut stream = ParseStream::new(&[0xAB, 0xCD, 0xEF]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let _ = stream.read_bits(4, &path).expect("nibble");
            assert_eq!(stream.bit_pos(), 4);

            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let inst = result.bind(py);
            let a: i64 = inst.getattr("a").unwrap().extract().unwrap();
            let b: i64 = inst.getattr("b").unwrap().extract().unwrap();
            assert_eq!(a, 0xBC);
            assert_eq!(b, 0xDE);
        });
    }

    // ======================================================================
    // parse — 变长 subcon 错误
    // ======================================================================

    #[test]
    fn parse_unaligned_variable_size_inner_returns_bitfield_error() {
        // GreedyBytes 无定长 sizeof，在未对齐慢路径报 BitField
        with_py(|py| {
            let inner = Node::GreedyBytes(crate::nodes::greedy_bytes::GreedyBytesNode::new());
            let node = BytewiseNode::new(inner);

            let mut stream = ParseStream::new(&[0xAB, 0xCD, 0xEF]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let _ = stream.read_bits(4, &path).expect("nibble");
            // bit_pos=4，inner sizeof 失败
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::BitField { message, .. } => {
                    assert!(
                        message.contains("fixed-sized") || message.contains("Bytewise"),
                        "got: {}",
                        message
                    );
                }
                other => panic!("expected BitField, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // build — 对齐快路径
    // ======================================================================

    #[test]
    fn build_aligned_fast_path_writes_directly() {
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
            let node = BytewiseNode::new(inner);

            let obj = py.eval_bound("0x1234", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x12, 0x34]);
        });
    }

    // ======================================================================
    // build — 未对齐慢路径
    // ======================================================================

    #[test]
    fn build_unaligned_slow_path_writes_bit_by_bit() {
        // 先写 4 bit Nibble，再 Bytewise(Int16ub=0x1234)
        // 结果：bit 0-3=Nibble(0xA), bit 4-19=0x1234
        // 总 20 bit，5 字节 boundary：实际 24 bit（最后 4 bit 用 Nibble 填充对齐）
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
            let node = BytewiseNode::new(inner);

            let mut stream = BuildStream::new();
            // 先写 4 bit Nibble
            stream.write_bits(0xA, 4);
            assert_eq!(stream.bit_pos(), 4);

            let obj = py.eval_bound("0x1234", None, None).expect("obj");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            // stream 现有 4 + 16 = 20 bit，未对齐
            assert_eq!(stream.bit_pos(), 4);
            assert_eq!(stream.tell(), 2); // 完整字节

            // 验证：再写 4 bit 凑齐 3 字节
            stream.write_bits(0xB, 4);
            assert_eq!(stream.bit_pos(), 0);
            let bytes = stream.as_bytes();
            assert_eq!(bytes.len(), 3);
            // bit 0-3: 0xA = 1010
            // bit 4-11: 0x12 = 0001_0010
            // bit 12-19: 0x34 = 0011_0100
            // bit 20-23: 0xB = 1011
            // 合计 24 bit = 3 字节：1010_0001 0010_0011 0100_1011
            //                = 0xA1 0x23 0x4B
            assert_eq!(bytes, &[0xA1, 0x23, 0x4B]);
        });
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_returns_inner_bytes_times_8() {
        with_py(|py| {
            let ctx = Context::placeholder(py);
            // Int16ub → sizeof = 2 字节 = 16 bit
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
            let node = BytewiseNode::new(inner);
            assert_eq!(node.sizeof(&ctx).unwrap(), 16);

            // Int32ub → sizeof = 4 字节 = 32 bit
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt32Big));
            let node = BytewiseNode::new(inner);
            assert_eq!(node.sizeof(&ctx).unwrap(), 32);
        });
    }

    #[test]
    fn sizeof_propagates_inner_error() {
        // GreedyBytes 无定长 → Bytewise sizeof 也失败
        with_py(|py| {
            let ctx = Context::placeholder(py);
            let inner = Node::GreedyBytes(crate::nodes::greedy_bytes::GreedyBytesNode::new());
            let node = BytewiseNode::new(inner);
            assert!(node.sizeof(&ctx).is_err());
        });
    }

    // ======================================================================
    // parse ↔ build 往返（对齐快路径）
    // ======================================================================

    #[test]
    fn round_trip_aligned_bytewise_int16() {
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
            let node = BytewiseNode::new(inner);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            let obj = py.eval_bound("0xBEEF", None, None).expect("obj");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();
            assert_eq!(bytes, vec![0xBE, 0xEF]);

            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().unwrap();
            assert_eq!(v, 0xBEEF);
        });
    }

    // ======================================================================
    // 集成测试：Bytewise 嵌入 Bitwise 域
    // ======================================================================

    #[test]
    fn bytewise_inside_bitwise_aligned() {
        // Bitwise(Struct{a: Nibble, b: Bytewise(Int8ub), c: Nibble})
        // 总 bit = 4 + 8 + 4 = 16 bit = 2 字节
        // 数据 0xA5 0xF0 → a=0xA, b=0x5F (bit 4-11), c=0x0 (bit 12-15)
        with_py(|py| {
            let inner_struct = StructNode::new_for_test(
                py,
                vec![
                    (
                        "a".to_string(),
                        Node::BitsInteger(BitsIntegerNode::new(4, false, false)),
                    ),
                    (
                        "b".to_string(),
                        Node::Bytewise(BytewiseNode::new(Node::FormatField(FormatFieldNode::new(
                            PythonFormat::UnsignedInt8Big,
                        )))),
                    ),
                    (
                        "c".to_string(),
                        Node::BitsInteger(BitsIntegerNode::new(4, false, false)),
                    ),
                ],
            );
            let bitwise = crate::nodes::bitwise::BitwiseNode::new(Node::Struct(inner_struct));

            // parse 0xA5 0xF0
            // bit 0-3: 1010 = 0xA
            // bit 4-11: 0101_1111 = 0x5F
            // bit 12-15: 0000 = 0x0
            let mut stream = ParseStream::new(&[0xA5, 0xF0]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = bitwise
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let inst = result.bind(py);
            let a: i64 = inst.getattr("a").unwrap().extract().unwrap();
            let b: i64 = inst.getattr("b").unwrap().extract().unwrap();
            let c: i64 = inst.getattr("c").unwrap().extract().unwrap();
            assert_eq!(a, 0xA);
            assert_eq!(b, 0x5F);
            assert_eq!(c, 0x0);
        });
    }

    #[test]
    fn bytewise_inside_bitwise_unaligned_slow_path() {
        // Bitwise(Struct{a: Nibble, b: Bytewise(Int16ub), c: Nibble})
        // 总 bit = 4 + 16 + 4 = 24 bit = 3 字节
        // b 在 Nibble 后未对齐（bit_pos=4），走慢路径
        with_py(|py| {
            let inner_struct = StructNode::new_for_test(
                py,
                vec![
                    (
                        "a".to_string(),
                        Node::BitsInteger(BitsIntegerNode::new(4, false, false)),
                    ),
                    (
                        "b".to_string(),
                        Node::Bytewise(BytewiseNode::new(Node::FormatField(FormatFieldNode::new(
                            PythonFormat::UnsignedInt16Big,
                        )))),
                    ),
                    (
                        "c".to_string(),
                        Node::BitsInteger(BitsIntegerNode::new(4, false, false)),
                    ),
                ],
            );
            let bitwise = crate::nodes::bitwise::BitwiseNode::new(Node::Struct(inner_struct));

            // build: a=0xA, b=0x1234, c=0xB
            // bit 0-3: 0xA = 1010
            // bit 4-19: 0x1234 = 0001_0010_0011_0100
            // bit 20-23: 0xB = 1011
            // 合计 24 bit：1010_0001 0010_0011 0100_1011 = 0xA1 0x23 0x4B
            let obj = py
                .eval_bound(
                    "type('O', (), {'a': 0xA, 'b': 0x1234, 'c': 0xB})()",
                    None,
                    None,
                )
                .expect("obj");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let mut stream = BuildStream::new();
            bitwise
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();
            assert_eq!(bytes, vec![0xA1, 0x23, 0x4B]);

            // parse back
            let mut pstream = ParseStream::new(&bytes);
            let result = bitwise
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let inst = result.bind(py);
            let a: i64 = inst.getattr("a").unwrap().extract().unwrap();
            let b: i64 = inst.getattr("b").unwrap().extract().unwrap();
            let c: i64 = inst.getattr("c").unwrap().extract().unwrap();
            assert_eq!(a, 0xA);
            assert_eq!(b, 0x1234);
            assert_eq!(c, 0xB);
        });
    }
}
