//! TransformNode：字节级变换节点（BitsSwapped / ByteSwapped）。
//!
//! 设计依据：`docs/模块设计-BitStream.md` §4.5、§5.2、§9.5、§13.5。
//! Python 参考：`construct/construct/core.py` `ByteSwapped`（L4817）、
//! `BitsSwapped`（L4836）、`Transformed`（L5233）；
//! `construct/construct/lib/binary.py` `swapbytes`（L123）、
//! `swapbitsinbytes`（L149，对应 `SWAPBITSINBYTES_CACHE`）。
//!
//! ## 职责
//!
//! TransformNode 在字节域对 `inner.sizeof()` 字节做整体或字节内 bit 反序，
//! 再喂给 inner 解析（或反向用于 build）。覆盖 Python `ByteSwapped`/`BitsSwapped`
//! 的定长 subcon 路径。
//!
//! ## 已知限制（Phase 3.1，§13.5）
//!
//! Python `BitsSwapped`/`ByteSwapped` 对变长 subcon 走 `Restreamed` 回退
//! （encoderunit=1 逐字节变换）。construct-rs 的 TransformNode 统一要求定长 subcon
//! （step 1 调 `inner.sizeof()`），因此 `BitsSwapped(GreedyBytes)` 等变长用例
//! 在本阶段返回错误。常见用例（`BitsSwapped(Bitwise(...))`、`ByteSwapped(Bytes(N))`）
//! subcon 均为定长，Phase 3 覆盖主要场景。

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

use super::Construct;
use crate::nodes::Node;

// ---------------------------------------------------------------------------
// BIT_REVERSE_TABLE（§5.2）
// ---------------------------------------------------------------------------

/// 预计算的 bit 反序表：`BIT_REVERSE_TABLE[b] = b` 的 bit 序反序（按 8 位组）。
///
/// 对应 Python `SWAPBITSINBYTES_CACHE`（`lib/binary.py` L149）。
/// 用 `const` 表 + 索引替代 Python 的 dict 查表，零运行时初始化开销。
///
/// 例：`BIT_REVERSE_TABLE[0xF0] == 0x0F`，`BIT_REVERSE_TABLE[0x01] == 0x80`。
const BIT_REVERSE_TABLE: [u8; 256] = {
    let mut table = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let mut v = i as u8;
        let mut r = 0u8;
        let mut j = 0;
        while j < 8 {
            r = (r << 1) | (v & 1);
            v >>= 1;
            j += 1;
        }
        table[i] = r;
        i += 1;
    }
    table
};

// ---------------------------------------------------------------------------
// ByteTransform
// ---------------------------------------------------------------------------

/// 字节级变换种类（设计 §4.5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteTransform {
    /// 整体字节反序（`swapbytes`，binary.py L123）。用于 `ByteSwapped`。
    ///
    /// 例：`[0x01, 0x02, 0x03] → [0x03, 0x02, 0x01]`
    ByteSwap,
    /// 每字节内 bit 反序（`swapbitsinbytes`，binary.py L149）。用于 `BitsSwapped`。
    ///
    /// 例：`[0xF0, 0x00] → [0x0F, 0x00]`
    BitSwap,
}

impl ByteTransform {
    /// 对 data 应用变换，返回变换后的 `Vec<u8>`。
    ///
    /// - [`ByteSwap`](Self::ByteSwap)：整体反序。
    /// - [`BitSwap`](Self::BitSwap)：每字节用 `BIT_REVERSE_TABLE` 查表。
    pub fn apply(&self, data: &[u8]) -> Vec<u8> {
        match self {
            ByteTransform::ByteSwap => {
                // 整体反序：iter().rev().copied() 零开销。
                let mut result = data.to_vec();
                result.reverse();
                result
            }
            ByteTransform::BitSwap => {
                // 每字节查表反序 bit。
                data.iter()
                    .map(|&b| BIT_REVERSE_TABLE[b as usize])
                    .collect()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TransformNode
// ---------------------------------------------------------------------------

/// 字节级变换节点：读取 `inner.sizeof()` 字节，按变换函数处理后喂给 inner。
///
/// 对应 Python `Transformed(subcon, func, size, func, size)`。
/// 本阶段仅支持两种变换（见 [`ByteTransform`]），覆盖 `BitsSwapped`/`ByteSwapped`。
///
/// # parse 流程（设计 §4.5）
///
/// 1. `size = inner.sizeof(ctx)?`（定长要求，否则 Err，对齐 ByteSwapped L4832）。
/// 2. `data = stream.read(size, path)?`（字节级，要求 `bit_pos == 0`）。
/// 3. `data = transform.apply(data)`（原地或新分配）。
/// 4. 在 data 上创建临时 ParseStream，`inner.parse(&mut sub_stream, ...)`。
///
/// # build 流程
///
/// 1. inner build 到临时 BuildStream。
/// 2. `data = transform.apply(&sub_stream.into_bytes())`。
/// 3. `stream.write(&data)`。
///
/// # sizeof
///
/// `inner.sizeof(ctx)?`（变换不改变长度）。
///
/// # 错误（§9.5 TR-1~TR-4）
///
/// - TR-1：inner.sizeof() 返回 Err（变长 subcon）→ 返回错误（Phase 3.1 已知限制）
/// - TR-2：BitSwap 应用——每字节 bit 反序（查表）
/// - TR-3：ByteSwap 应用——整体字节反序
/// - TR-4：inner 是 BitwiseNode → 先变换字节再进入 bit 域
#[derive(Debug)]
pub struct TransformNode {
    /// 被包裹的子树根（必须定长）。
    inner: Box<Node>,
    /// 字节级变换种类。
    transform: ByteTransform,
}

impl TransformNode {
    /// 创建 `TransformNode`，包裹给定的子树根节点与变换。
    ///
    /// # 参数
    ///
    /// - `inner`：被包裹的子树（必须支持 sizeof，即定长）。
    /// - `transform`：字节级变换种类（ByteSwap 或 BitSwap）。
    pub fn new(inner: Node, transform: ByteTransform) -> Self {
        Self {
            inner: Box::new(inner),
            transform,
        }
    }

    /// 返回内部子树根节点的引用。
    pub fn inner(&self) -> &Node {
        &self.inner
    }

    /// 返回变换种类。
    pub fn transform(&self) -> ByteTransform {
        self.transform
    }
}

impl Construct for TransformNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // TR-1: inner 必须有定长 sizeof。
        let size = self
            .inner
            .sizeof(ctx)
            .map_err(|e| ConstructError::Generic {
                message: format!(
                    "Transform (BitsSwapped/ByteSwapped) requires a fixed-sized subcon \
                     (Phase 3.1 limitation, inner sizeof failed: {})",
                    e.full_message()
                ),
                path: path.to_string(),
            })?;

        // 从主流读取 size 字节（要求 bit_pos==0）。
        let data = stream.read(size, path)?;

        // 应用变换。
        let transformed = self.transform.apply(data);

        // 在临时缓冲上创建 ParseStream，inner 解析。
        let mut sub_stream = ParseStream::new(&transformed);
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
        // 先在临时 BuildStream 上 build inner。
        // 预估容量：若 inner sizeof 可算则用，否则用 0（Vec 自动扩容）。
        let capacity = self.inner.sizeof(ctx).unwrap_or(0);
        let mut sub_stream = BuildStream::with_capacity(capacity);
        self.inner.build(py, obj, &mut sub_stream, ctx, path)?;
        let data = sub_stream.into_bytes();

        // 应用变换后写入主流。
        let transformed = self.transform.apply(&data);
        stream.write(&transformed);
        Ok(())
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // 变换不改变长度。
        self.inner.sizeof(ctx)
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::bitwise::BitwiseNode;
    use crate::nodes::bytes::BytesNode;
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
    // BIT_REVERSE_TABLE
    // ======================================================================

    #[test]
    fn bit_reverse_table_correct_values() {
        // 验证若干典型值
        assert_eq!(BIT_REVERSE_TABLE[0x00], 0x00);
        assert_eq!(BIT_REVERSE_TABLE[0xFF], 0xFF);
        assert_eq!(BIT_REVERSE_TABLE[0xF0], 0x0F);
        assert_eq!(BIT_REVERSE_TABLE[0x0F], 0xF0);
        assert_eq!(BIT_REVERSE_TABLE[0x01], 0x80);
        assert_eq!(BIT_REVERSE_TABLE[0x80], 0x01);
        assert_eq!(BIT_REVERSE_TABLE[0xAA], 0x55);
        assert_eq!(BIT_REVERSE_TABLE[0x55], 0xAA);
    }

    #[test]
    fn bit_reverse_table_involution() {
        // 反序两次回到原值：BIT_REVERSE_TABLE[BIT_REVERSE_TABLE[x]] == x
        for i in 0..256u32 {
            let once = BIT_REVERSE_TABLE[i as usize];
            let twice = BIT_REVERSE_TABLE[once as usize];
            assert_eq!(twice, i as u8, "involution failed at {}", i);
        }
    }

    // ======================================================================
    // ByteTransform::apply
    // ======================================================================

    #[test]
    fn byte_swap_reverses_byte_order() {
        // TR-3: 整体反序
        let result = ByteTransform::ByteSwap.apply(&[0x01, 0x02, 0x03, 0x04]);
        assert_eq!(result, vec![0x04, 0x03, 0x02, 0x01]);
    }

    #[test]
    fn byte_swap_empty_input() {
        let result = ByteTransform::ByteSwap.apply(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn byte_swap_single_byte_no_change() {
        let result = ByteTransform::ByteSwap.apply(&[0xAB]);
        assert_eq!(result, vec![0xAB]);
    }

    #[test]
    fn bit_swap_reverses_each_byte() {
        // TR-2: 每字节 bit 反序
        let result = ByteTransform::BitSwap.apply(&[0xF0, 0x00, 0x01]);
        assert_eq!(result, vec![0x0F, 0x00, 0x80]);
    }

    #[test]
    fn bit_swap_empty_input() {
        let result = ByteTransform::BitSwap.apply(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn bit_swap_preserves_byte_count() {
        let data = vec![0xAB, 0xCD, 0xEF];
        let result = ByteTransform::BitSwap.apply(&data);
        assert_eq!(result.len(), data.len());
    }

    // ======================================================================
    // 构造器
    // ======================================================================

    #[test]
    fn new_stores_params() {
        let inner = Node::Bytes(BytesNode::new_const(4));
        let node = TransformNode::new(inner, ByteTransform::ByteSwap);
        assert!(matches!(node.inner(), Node::Bytes(_)));
        assert_eq!(node.transform(), ByteTransform::ByteSwap);
    }

    // ======================================================================
    // parse — ByteSwap
    // ======================================================================

    #[test]
    fn parse_byte_swap_int16() {
        // ByteSwapped(Int16ub) 解析 [0x34, 0x12] → 0x1234
        // Transformed: read 2 bytes, swapbytes → [0x12, 0x34], Int16ub parse → 0x1234
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
            let node = TransformNode::new(inner, ByteTransform::ByteSwap);

            let mut stream = ParseStream::new(&[0x34, 0x12]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().unwrap();
            assert_eq!(v, 0x1234);
            assert_eq!(stream.tell(), 2);
        });
    }

    #[test]
    fn parse_byte_swap_int32() {
        // ByteSwapped(Int32ub) 解析 [0x78, 0x56, 0x34, 0x12] → 0x12345678
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt32Big));
            let node = TransformNode::new(inner, ByteTransform::ByteSwap);

            let mut stream = ParseStream::new(&[0x78, 0x56, 0x34, 0x12]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().unwrap();
            assert_eq!(v, 0x12345678);
        });
    }

    #[test]
    fn parse_byte_swap_bytes() {
        // ByteSwapped(Bytes(3)) 解析 [0x03, 0x02, 0x01] → b"\x03\x02\x01" 反序 → b"\x01\x02\x03"
        with_py(|py| {
            let inner = Node::Bytes(BytesNode::new_const(3));
            let node = TransformNode::new(inner, ByteTransform::ByteSwap);

            let mut stream = ParseStream::new(&[0x03, 0x02, 0x01]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: &[u8] = result.bind(py).extract().unwrap();
            assert_eq!(v, &[0x01, 0x02, 0x03]);
        });
    }

    // ======================================================================
    // parse — BitSwap
    // ======================================================================

    #[test]
    fn parse_bit_swap_bytes() {
        // BitsSwapped(Bytes(2)) 解析 [0xF0, 0x0F] → bit 反序 → [0x0F, 0xF0]
        with_py(|py| {
            let inner = Node::Bytes(BytesNode::new_const(2));
            let node = TransformNode::new(inner, ByteTransform::BitSwap);

            let mut stream = ParseStream::new(&[0xF0, 0x0F]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: &[u8] = result.bind(py).extract().unwrap();
            assert_eq!(v, &[0x0F, 0xF0]);
        });
    }

    #[test]
    fn parse_bit_swap_int8() {
        // BitsSwapped(Int8ub) 解析 [0xF0] → bit 反序 → [0x0F] → Int8ub = 15
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let node = TransformNode::new(inner, ByteTransform::BitSwap);

            let mut stream = ParseStream::new(&[0xF0]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().unwrap();
            assert_eq!(v, 0x0F);
        });
    }

    // ======================================================================
    // build — ByteSwap
    // ======================================================================

    #[test]
    fn build_byte_swap_int16() {
        // ByteSwapped(Int16ub) build 0x1234 → 输出 [0x34, 0x12]
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
            let node = TransformNode::new(inner, ByteTransform::ByteSwap);

            let obj = py.eval_bound("0x1234", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x34, 0x12]);
        });
    }

    // ======================================================================
    // build — BitSwap
    // ======================================================================

    #[test]
    fn build_bit_swap_bytes() {
        // BitsSwapped(Bytes(2)) build b"\x0F\xF0" → bit 反序 → [0xF0, 0x0F]
        with_py(|py| {
            let inner = Node::Bytes(BytesNode::new_const(2));
            let node = TransformNode::new(inner, ByteTransform::BitSwap);

            let obj = py.eval_bound("b'\\x0F\\xF0'", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0xF0, 0x0F]);
        });
    }

    // ======================================================================
    // parse ↔ build 往返
    // ======================================================================

    #[test]
    fn round_trip_byte_swap_int32() {
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt32Big));
            let node = TransformNode::new(inner, ByteTransform::ByteSwap);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            let obj = py.eval_bound("0xDEADBEEF", None, None).expect("obj");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();
            // 反序：0xDE 0xAD 0xBE 0xEF → 0xEF 0xBE 0xAD 0xDE
            assert_eq!(bytes, vec![0xEF, 0xBE, 0xAD, 0xDE]);

            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().unwrap();
            assert_eq!(v, 0xDEADBEEF);
        });
    }

    #[test]
    fn round_trip_bit_swap_bytes() {
        with_py(|py| {
            let inner = Node::Bytes(BytesNode::new_const(3));
            let node = TransformNode::new(inner, ByteTransform::BitSwap);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            let obj = py
                .eval_bound("b'\\xF0\\x0A\\x55'", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();
            // 每字节 bit 反序：0xF0→0x0F, 0x0A→0x50, 0x55→0xAA
            assert_eq!(bytes, vec![0x0F, 0x50, 0xAA]);

            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let v: &[u8] = result.bind(py).extract().unwrap();
            // 再次反序 → 回到原值
            assert_eq!(v, &[0xF0, 0x0A, 0x55]);
        });
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_returns_inner_size() {
        with_py(|py| {
            let ctx = Context::placeholder(py);
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
            let node = TransformNode::new(inner, ByteTransform::ByteSwap);
            assert_eq!(node.sizeof(&ctx).unwrap(), 2);

            let inner = Node::Bytes(BytesNode::new_const(4));
            let node = TransformNode::new(inner, ByteTransform::BitSwap);
            assert_eq!(node.sizeof(&ctx).unwrap(), 4);
        });
    }

    // ======================================================================
    // TR-4: Transform 包装 BitwiseNode（先变换字节再进入 bit 域）
    // ======================================================================

    #[test]
    fn transform_wrapping_bitwise_node() {
        // BitsSwapped(Bitwise(Bytewise(Bytes(1))))
        // = Transform(Bitwise(Bytewise(Bytes(1))), BitSwap)
        //
        // Bitwise(Bytewise(Bytes(1))) sizeof:
        //   - Bytewise(Bytes(1)).sizeof = Bytes(1).sizeof * 8 = 8 bit
        //   - Bitwise.sizeof = 8/8 = 1 byte
        //
        // parse [0xF0]:
        //   1. 读 1 字节 [0xF0]
        //   2. BitSwap → [0x0F]
        //   3. Bitwise 在 bit 域解析 [0x0F]（8 bit）
        //   4. Bytewise(Bytes(1)) 把 8 bit 重组为 1 字节 0x0F，Bytes(1) 读 → b"\x0F"
        with_py(|py| {
            let bitwise_inner = Node::Bytewise(crate::nodes::bytewise::BytewiseNode::new(
                Node::Bytes(BytesNode::new_const(1)),
            ));
            let bitwise = Node::Bitwise(BitwiseNode::new(bitwise_inner));
            let node = TransformNode::new(bitwise, ByteTransform::BitSwap);

            let mut stream = ParseStream::new(&[0xF0]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: &[u8] = result.bind(py).extract().unwrap();
            assert_eq!(v, &[0x0F]);
            assert_eq!(stream.tell(), 1);
        });
    }

    // ======================================================================
    // TR-1: 变长 subcon 错误
    // ======================================================================

    #[test]
    fn parse_variable_size_inner_returns_error() {
        // GreedyBytes 无定长 sizeof
        with_py(|py| {
            let inner = Node::GreedyBytes(crate::nodes::greedy_bytes::GreedyBytesNode::new());
            let node = TransformNode::new(inner, ByteTransform::ByteSwap);

            let mut stream = ParseStream::new(&[0x01, 0x02]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Generic { message, .. } => {
                    assert!(
                        message.contains("fixed-sized") || message.contains("Transform"),
                        "got: {}",
                        message
                    );
                }
                other => panic!("expected Generic, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // 集成：ByteSwapped 包 Struct
    // ======================================================================

    #[test]
    fn byte_swapped_struct_round_trip() {
        // ByteSwapped(Struct{a: Int8ub, b: Int16ub})
        // build {a=0xAA, b=0xBCDE}:
        //   先 Struct build → [0xAA, 0xBC, 0xDE]
        //   ByteSwap → [0xDE, 0xBC, 0xAA]
        // parse [0xDE, 0xBC, 0xAA]:
        //   ByteSwap → [0xAA, 0xBC, 0xDE]
        //   Struct parse → {a=0xAA, b=0xBCDE}
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
                        Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big)),
                    ),
                ],
            );
            let node = TransformNode::new(Node::Struct(inner_struct), ByteTransform::ByteSwap);

            // build
            let obj = py
                .eval_bound("type('O', (), {'a': 0xAA, 'b': 0xBCDE})()", None, None)
                .expect("obj");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();
            assert_eq!(bytes, vec![0xDE, 0xBC, 0xAA]);

            // parse back
            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let inst = result.bind(py);
            let a: i64 = inst.getattr("a").unwrap().extract().unwrap();
            let b: i64 = inst.getattr("b").unwrap().extract().unwrap();
            assert_eq!(a, 0xAA);
            assert_eq!(b, 0xBCDE);
        });
    }
}
