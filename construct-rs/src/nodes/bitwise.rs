//! BitwiseNode：bit 域包装器（核心节点）。
//!
//! Python 参考：`construct/construct/core.py` `Bitwise`（L1030）、
//! `lib/bitstream.py` `RestreamedBytesIO.close`（L50-54，缓冲清空校验）。
//!
//! ## 职责
//!
//! BitwiseNode 是 bit 域边界节点：在其内部 subcon 上建立 bit 级游标，结束时校验
//! "消耗 bit 数为 8 倍数"（即退出 bit 游标 == 入口 bit 游标）。
//!
//! 内部 `inner` 是一棵完整的 [`Node`] 子树（通常是 [`StructNode`](super::StructNode)，
//! 由 BitStructMixin 编译；或单个 BitsInteger，由 `Bitwise(BitsInteger(N))` 编译）。
//!
//! ## parse 流程
//!
//! 1. 记录入口 bit 游标 `entry_bit_pos = stream.bit_pos()`。
//!    - 顶层 Bitwise：`entry_bit_pos == 0`（字节对齐入口）。
//!    - 嵌套 Bitwise：`entry_bit_pos` 可能为 0-7（bit_pos 延续不重置）。
//! 2. 调用 `inner.parse(stream, ...)`——inner 内部的 bit 节点通过 `read_bits` 推进 bit_pos。
//! 3. 校验 `stream.bit_pos() == entry_bit_pos`（内层消耗的 bit 数是 8 倍数），
//!    否则返回 [`ConstructError::BitField`]。
//!
//! ## build 流程（对称）
//!
//! 1. 记录入口 `entry_bit_pos`。
//! 2. 调用 `inner.build(...)`——inner 内部的 bit 节点通过 `write_bits` 填充部分字节。
//! 3. 校验 `stream.bit_pos() == entry_bit_pos`。
//!
//! ## sizeof
//!
//! `inner.sizeof(ctx)? / 8`——inner 报 bit 数，外层报字节数。若 inner sizeof 非 8 倍数，
//! 返回 [`ConstructError::BitField`]。
//!
//! ## 嵌套 Bitwise 对齐语义
//!
//! 顶层 Bitwise 入口 `entry_bit_pos=0`，退化为"退出 bit_pos==0"的绝对校验。
//! 嵌套 Bitwise 入口 `entry_bit_pos` 可能为 1-7，退出检查回到入口值（相对校验）。
//! 两者统一为 `exit == entry`，无需特判。这与 Python construct 的行为一致：
//! Python 内层 Bitwise 在已膨胀的 bit 流上再膨胀，等价于继续读 bit。

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

use super::Construct;
use crate::nodes::Node;

/// bit 域包装器节点：建立 bit 级游标边界，结束时校验"消耗 bit 数为 8 倍数"。
///
/// 详见模块级文档。
///
/// # 错误
///
/// - inner 消耗 bit 数非 8 倍数 → parse/build 返回 `BitField` 错误
/// - inner.sizeof() 非 8 倍数 → sizeof 返回 `BitField` 错误
/// - inner 含表达式字段（sizeof Err）→ sizeof 向上传播
/// - 嵌套 Bitwise → 入口 bit_pos 任意，退出检查回到入口值
/// - 空 BitStruct → parse/build 不读写，sizeof=0
#[derive(Debug)]
pub struct BitwiseNode {
    /// 被包裹的子树根（Box 因 Node 递归）。
    inner: Box<Node>,
}

impl BitwiseNode {
    /// 创建 `BitwiseNode`，包裹给定的子树根节点。
    ///
    /// # 参数
    ///
    /// - `inner`：被包裹的子树（通常是 `Node::Struct` 或 `Node::BitsInteger`）。
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

impl Construct for BitwiseNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 记录入口 bit 游标（顶层为 0，嵌套 Bitwise 可能为 1-7）。
        let entry_bit_pos = stream.bit_pos();

        // 委托给 inner 子树——inner 内部的 bit 节点通过 read_bits 推进 bit_pos。
        let result = self.inner.parse(py, stream, ctx, path)?;

        // 对齐校验：消耗的 bit 数必须是 8 倍数。
        // 对齐 Python RestreamedBytesIO.close() 缓冲清空校验（bitstream.py L50-54）。
        let exit_bit_pos = stream.bit_pos();
        if exit_bit_pos != entry_bit_pos {
            return Err(ConstructError::BitField {
                message: format!(
                    "bit stream not byte-aligned after Bitwise parse (entry bit_pos={}, exit bit_pos={}, consumed {} bits mod 8)",
                    entry_bit_pos,
                    exit_bit_pos,
                    ((exit_bit_pos as i32 - entry_bit_pos as i32 + 8) % 8)
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
        // 记录入口 bit 游标。
        let entry_bit_pos = stream.bit_pos();

        // 委托给 inner 子树——inner 内部的 bit 节点通过 write_bits 填充部分字节。
        self.inner.build(py, obj, stream, ctx, path)?;

        // 对齐校验。
        let exit_bit_pos = stream.bit_pos();
        if exit_bit_pos != entry_bit_pos {
            return Err(ConstructError::BitField {
                message: format!(
                    "bit stream not byte-aligned after Bitwise build (entry bit_pos={}, exit bit_pos={}, consumed {} bits mod 8)",
                    entry_bit_pos,
                    exit_bit_pos,
                    ((exit_bit_pos as i32 - entry_bit_pos as i32 + 8) % 8)
                ),
                path: path.to_string(),
            });
        }

        Ok(())
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // inner.sizeof() 返回 bit 数，外层报字节数。
        let inner_bits = self.inner.sizeof(ctx)?;

        // inner sizeof 非 8 倍数 → BitField 错误。
        // 注意：inner_bits % 8 != 0 时返回错误（与 parse/build 时校验一致），
        // 而非 Python 的 size//8 截断。construct-rs fail-fast。
        //
        // sizeof 签名不携带 path 参数，因此 path 字段为空字符串。
        // message 已含 "Bitwise inner sizeof N" 定位信息。
        if inner_bits % 8 != 0 {
            return Err(ConstructError::BitField {
                message: format!(
                    "Bitwise inner sizeof {} is not a multiple of 8 bits (cannot convert to bytes)",
                    inner_bits
                ),
                path: String::new(),
            });
        }

        Ok(inner_bits / 8)
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::bits_integer::BitsIntegerNode;
    use crate::nodes::struct_node::StructNode;

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
    // 构造器与访问器
    // ======================================================================

    #[test]
    fn new_stores_inner() {
        let inner = Node::BitsInteger(BitsIntegerNode::new(8, false, false));
        let node = BitwiseNode::new(inner);
        assert!(matches!(node.inner(), Node::BitsInteger(_)));
    }

    // ======================================================================
    // parse — Bitwise(BitsInteger(N)) 单字段
    // ======================================================================

    #[test]
    fn parse_single_bitsinteger_byte_aligned() {
        // Bitwise(BitsInteger(8)) parse 0x42 → 66
        with_py(|py| {
            let inner = Node::BitsInteger(BitsIntegerNode::new(8, false, false));
            let node = BitwiseNode::new(inner);

            let mut stream = ParseStream::new(&[0x42]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let i: i64 = v.bind(py).extract().unwrap();
            assert_eq!(i, 0x42);
            // 退出时 bit_pos 应回到入口 0
            assert_eq!(stream.bit_pos(), 0);
            assert_eq!(stream.tell(), 1);
        });
    }

    #[test]
    fn parse_two_bitsintegers_byte_aligned() {
        // Bitwise(Struct{a: BitsInteger(4), b: BitsInteger(4)}) parse 0xA5 → a=10, b=5
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
                        Node::BitsInteger(BitsIntegerNode::new(4, false, false)),
                    ),
                ],
            );
            let node = BitwiseNode::new(Node::Struct(inner_struct));

            let mut stream = ParseStream::new(&[0xA5]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let inst = v.bind(py);
            let a: i64 = inst.getattr("a").unwrap().extract().unwrap();
            let b: i64 = inst.getattr("b").unwrap().extract().unwrap();
            assert_eq!(a, 10);
            assert_eq!(b, 5);
            assert_eq!(stream.bit_pos(), 0);
            assert_eq!(stream.tell(), 1);
        });
    }

    #[test]
    fn parse_across_byte_boundary() {
        // Bitwise(Struct{a: BitsInteger(12), b: BitsInteger(4)}) parse [0xAB, 0xC0]
        // → a = 0xABC = 2748, b = 0
        with_py(|py| {
            let inner_struct = StructNode::new_for_test(
                py,
                vec![
                    (
                        "a".to_string(),
                        Node::BitsInteger(BitsIntegerNode::new(12, false, false)),
                    ),
                    (
                        "b".to_string(),
                        Node::BitsInteger(BitsIntegerNode::new(4, false, false)),
                    ),
                ],
            );
            let node = BitwiseNode::new(Node::Struct(inner_struct));

            let mut stream = ParseStream::new(&[0xAB, 0xC0]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let inst = v.bind(py);
            let a: i64 = inst.getattr("a").unwrap().extract().unwrap();
            let b: i64 = inst.getattr("b").unwrap().extract().unwrap();
            assert_eq!(a, 0xABC);
            assert_eq!(b, 0);
            assert_eq!(stream.bit_pos(), 0);
            assert_eq!(stream.tell(), 2);
        });
    }

    // ======================================================================
    // parse — 对齐校验
    // ======================================================================

    #[test]
    fn parse_unaligned_inner_returns_bitfield_error() {
        // Bitwise(BitsInteger(5)) inner 只消耗 5 bit，非 8 倍数
        // → parse 返回 BitField 错误
        with_py(|py| {
            let inner = Node::BitsInteger(BitsIntegerNode::new(5, false, false));
            let node = BitwiseNode::new(inner);

            let mut stream = ParseStream::new(&[0xFF]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::BitField { message, path } => {
                    assert!(
                        message.contains("not byte-aligned") || message.contains("bit_pos"),
                        "got: {}",
                        message
                    );
                    assert_eq!(path, "root");
                }
                other => panic!("expected BitField, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // parse — 空 BitStruct
    // ======================================================================

    #[test]
    fn parse_empty_bitstruct_returns_empty_instance() {
        // Bitwise(Struct()) parse b'' → {} 实例
        with_py(|py| {
            let inner_struct = StructNode::new_for_test(py, Vec::new());
            let node = BitwiseNode::new(Node::Struct(inner_struct));

            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // 应是 StructNode 的用户类实例
            let d_binding = v
                .bind(py)
                .getattr("__dict__")
                .unwrap()
                .downcast_into::<pyo3::types::PyDict>()
                .unwrap();
            assert_eq!(d_binding.len(), 0);
            assert_eq!(stream.tell(), 0);
            assert_eq!(stream.bit_pos(), 0);
        });
    }

    // ======================================================================
    // build — Bitwise(BitsInteger(N)) 单字段
    // ======================================================================

    #[test]
    fn build_single_bitsinteger_byte_aligned() {
        with_py(|py| {
            let inner = Node::BitsInteger(BitsIntegerNode::new(8, false, false));
            let node = BitwiseNode::new(inner);

            let obj = py.eval_bound("0x42", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x42]);
            assert_eq!(stream.bit_pos(), 0);
        });
    }

    #[test]
    fn build_two_bitsintegers_byte_aligned() {
        // Bitwise(Struct{a: BitsInteger(4), b: BitsInteger(4)}) build a=10, b=5 → 0xA5
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
                        Node::BitsInteger(BitsIntegerNode::new(4, false, false)),
                    ),
                ],
            );
            let node = BitwiseNode::new(Node::Struct(inner_struct));

            let obj = py
                .eval_bound("type('O', (), {'a': 10, 'b': 5})()", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0xA5]);
            assert_eq!(stream.bit_pos(), 0);
        });
    }

    #[test]
    fn build_unaligned_inner_returns_bitfield_error() {
        // build: Bitwise(BitsInteger(5)) 内层 5 bit 未填满字节
        with_py(|py| {
            let inner = Node::BitsInteger(BitsIntegerNode::new(5, false, false));
            let node = BitwiseNode::new(inner);

            let obj = py.eval_bound("1", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::BitField { message, .. } => {
                    assert!(
                        message.contains("not byte-aligned") || message.contains("bit_pos"),
                        "got: {}",
                        message
                    );
                }
                other => panic!("expected BitField, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // parse ↔ build 往返
    // ======================================================================

    #[test]
    fn round_trip_bitwise_struct_two_fields() {
        // Bitwise(Struct{a: BitsInteger(4), b: BitsInteger(4)}) round-trip
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
                        Node::BitsInteger(BitsIntegerNode::new(4, false, false)),
                    ),
                ],
            );
            let node = BitwiseNode::new(Node::Struct(inner_struct));

            // build
            let obj = py
                .eval_bound("type('O', (), {'a': 0xA, 'b': 0x5})()", None, None)
                .expect("obj");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();
            assert_eq!(bytes, &[0xA5]);

            // parse back
            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let inst = result.bind(py);
            let a: i64 = inst.getattr("a").unwrap().extract().unwrap();
            let b: i64 = inst.getattr("b").unwrap().extract().unwrap();
            assert_eq!(a, 0xA);
            assert_eq!(b, 0x5);
        });
    }

    #[test]
    fn round_trip_three_fields_16_bits() {
        // Bitwise(Struct{flag: BitsInteger(1), value: BitsInteger(10), reserved: BitsInteger(5)})
        // 总计 16 bit = 2 字节
        // 对应 Python BitStruct('flag'/Bit, 'value'/BitsInteger(10), 'reserved'/Padding(5))
        with_py(|py| {
            let inner_struct = StructNode::new_for_test(
                py,
                vec![
                    (
                        "flag".to_string(),
                        Node::BitsInteger(BitsIntegerNode::new(1, false, false)),
                    ),
                    (
                        "value".to_string(),
                        Node::BitsInteger(BitsIntegerNode::new(10, false, false)),
                    ),
                    (
                        "reserved".to_string(),
                        Node::BitsInteger(BitsIntegerNode::new(5, false, false)),
                    ),
                ],
            );
            let node = BitwiseNode::new(Node::Struct(inner_struct));

            // build: flag=1, value=0x2A5, reserved=0 → 0b1_1010_0101_0_0000 = 0xB2A8?
            // 1 << 15 | 0x2A5 << 5 | 0 = 0x8000 | 0x54A0 = 0xD4A0
            // 0x2A5 = 677 = 0b10_1010_0101 (10 bit)
            // flag(1) | value(10) | reserved(5) = 0b1_10_1010_0101_00000 = 0xD4A0
            // high byte: 0b1101_0100 = 0xD4
            // low byte: 0b1010_0000 = 0xA0
            let obj = py
                .eval_bound(
                    "type('O', (), {'flag': 1, 'value': 0x2A5, 'reserved': 0})()",
                    None,
                    None,
                )
                .expect("obj");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();
            assert_eq!(bytes.len(), 2);
            assert_eq!(bytes, &[0xD4, 0xA0]);

            // parse back
            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let inst = result.bind(py);
            let flag: i64 = inst.getattr("flag").unwrap().extract().unwrap();
            let value: i64 = inst.getattr("value").unwrap().extract().unwrap();
            let reserved: i64 = inst.getattr("reserved").unwrap().extract().unwrap();
            assert_eq!(flag, 1);
            assert_eq!(value, 0x2A5);
            assert_eq!(reserved, 0);
        });
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_returns_inner_bits_div_8() {
        with_py(|py| {
            let ctx = Context::placeholder(py);

            // Bitwise(BitsInteger(8)) → sizeof = 1
            let inner = Node::BitsInteger(BitsIntegerNode::new(8, false, false));
            let node = BitwiseNode::new(inner);
            assert_eq!(node.sizeof(&ctx).unwrap(), 1);

            // Bitwise(BitsInteger(16)) → sizeof = 2
            let inner = Node::BitsInteger(BitsIntegerNode::new(16, false, false));
            let node = BitwiseNode::new(inner);
            assert_eq!(node.sizeof(&ctx).unwrap(), 2);
        });
    }

    #[test]
    fn sizeof_returns_byte_count_for_struct() {
        // Bitwise(Struct{a: BitsInteger(4), b: BitsInteger(12)}) → 16 bit = 2 字节
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
                        Node::BitsInteger(BitsIntegerNode::new(12, false, false)),
                    ),
                ],
            );
            let node = BitwiseNode::new(Node::Struct(inner_struct));
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).unwrap(), 2);
        });
    }

    #[test]
    fn sizeof_inner_not_multiple_of_8_returns_bitfield_error() {
        // Bitwise(BitsInteger(5)) → 5 bit 非 8 倍数
        with_py(|py| {
            let inner = Node::BitsInteger(BitsIntegerNode::new(5, false, false));
            let node = BitwiseNode::new(inner);
            let ctx = Context::placeholder(py);

            let err = node.sizeof(&ctx).expect_err("should fail");
            match err {
                ConstructError::BitField { message, .. } => {
                    assert!(
                        message.contains("5") && message.contains("8"),
                        "got: {}",
                        message
                    );
                }
                other => panic!("expected BitField, got {:?}", other),
            }
        });
    }

    #[test]
    fn sizeof_empty_struct_is_zero() {
        // Bitwise(Struct()) → sizeof = 0
        with_py(|py| {
            let inner_struct = StructNode::new_for_test(py, Vec::new());
            let node = BitwiseNode::new(Node::Struct(inner_struct));
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).unwrap(), 0);
        });
    }

    // ======================================================================
    // 嵌套 Bitwise
    // ======================================================================

    #[test]
    fn nested_bitwise_aligned_consumption() {
        // 外层 Bitwise(Struct{
        //   inner1: Bitwise(Struct{a: BitsInteger(8)}),  // 1 字节 = 8 bit
        //   b: BitsInteger(8),                           // 1 字节 = 8 bit
        // })
        // 总计 16 bit = 2 字节
        with_py(|py| {
            let inner_inner_struct = StructNode::new_for_test(
                py,
                vec![(
                    "a".to_string(),
                    Node::BitsInteger(BitsIntegerNode::new(8, false, false)),
                )],
            );
            let inner_bitwise = Node::Bitwise(BitwiseNode::new(Node::Struct(inner_inner_struct)));

            let outer_struct = StructNode::new_for_test(
                py,
                vec![
                    ("inner1".to_string(), inner_bitwise),
                    (
                        "b".to_string(),
                        Node::BitsInteger(BitsIntegerNode::new(8, false, false)),
                    ),
                ],
            );
            let node = BitwiseNode::new(Node::Struct(outer_struct));

            // build: inner1.a = 0xAA, b = 0xBB → bytes = [0xAA, 0xBB]
            let obj = py
                .eval_bound(
                    "type('O', (), {'inner1': type('I',(),{'a':0xAA})(), 'b': 0xBB})()",
                    None,
                    None,
                )
                .expect("obj");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();
            assert_eq!(bytes, &[0xAA, 0xBB]);

            // parse back
            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let inst = result.bind(py);
            let inner1 = inst.getattr("inner1").unwrap();
            let a: i64 = inner1.getattr("a").unwrap().extract().unwrap();
            let b: i64 = inst.getattr("b").unwrap().extract().unwrap();
            assert_eq!(a, 0xAA);
            assert_eq!(b, 0xBB);
        });
    }

    #[test]
    fn nested_bitwise_inner_unaligned_returns_bitfield_error() {
        // 嵌套 Bitwise(Bitwise(BitsInteger(5)))
        // 内层 Bitwise 消耗 5 bit，非 8 倍数 → BitField 错误
        with_py(|py| {
            let innermost = Node::BitsInteger(BitsIntegerNode::new(5, false, false));
            let inner_bitwise = Node::Bitwise(BitwiseNode::new(innermost));
            let node = BitwiseNode::new(inner_bitwise);

            let mut stream = ParseStream::new(&[0xFF]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            // 内层 Bitwise 应先返回 BitField，外层 Bitwise 直接传播
            assert!(matches!(err, ConstructError::BitField { .. }));
        });
    }

    // ======================================================================
    // 错误路径传播（push_path_segment 一致性）
    // ======================================================================

    #[test]
    fn error_propagates_with_inner_path_segment() {
        // Bitwise(Struct{a: BitsInteger(4), b: BitsInteger(4)})
        // 内层 b 读时字节不足 → 错误路径应含 .b 段
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
                        Node::BitsInteger(BitsIntegerNode::new(4, false, false)),
                    ),
                ],
            );
            let node = BitwiseNode::new(Node::Struct(inner_struct));

            // 只提供 0 字节，b 解析时会失败（虽然 a 也会失败）
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            // 应是 Stream 错误（bit 不足），路径含 .a
            match err {
                ConstructError::Stream { path, .. } => {
                    assert!(path.contains(".a"), "path should contain .a: {}", path);
                }
                other => panic!("expected Stream error, got {:?}", other),
            }
        });
    }
}
