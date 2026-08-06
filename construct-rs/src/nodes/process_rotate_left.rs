//! ProcessRotateLeftNode：位旋转左移节点。
//!
//! 设计依据：`docs/design/模块设计/模块设计-Phase8-P1P2.md` §5.5-5.6。
//! Python 参考：`construct/construct/core.py` `ProcessRotateLeft`（L5424-5529）。
//!
//! ## 行为概述
//!
//! `ProcessRotateLeft(amount, group, subcon)`：parse 读至 EOF → 按 amount/group
//! 位旋转 → 子流 → inner.parse。build 对称（amount 取负）。
//!
//! ## 4 分支位运算
//!
//! - amount % (group*8) == 0 → 分支 1：不旋转
//! - group == 1 → 分支 2：查 ROTATION_TABLES[amount]（amount < 8，单字节旋转）
//! - amount % 8 == 0 → 分支 3：字节序重排（纯字节移位）
//! - 通用 → 分支 4：bit rotate（字节重排 + 字节内 bit 旋转）
//!
//! ## build 取负（对齐 Python L5499）
//!
//! build 时 `amount = (-amount).rem_euclid(group*8)`（Rust rem_euclid 对齐 Python 模运算）。

use crate::context::Context;
use crate::error::ConstructError;
use crate::expr::{eval_expr_int, ExprProgram};
use crate::nodes::Node;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

use super::Construct;

/// 预计算单字节旋转表（对应 Python L5454 precomputed_single_rotations）。
/// 8 个表（amount 0..7），每表 256 值。const 表，零运行时初始化。
//
// clippy::manual_rotate：使用 `(v << amount) | (v >> (8 - amount))` 而非
// `v.rotate_left(amount)` 是为了与 Python L5454 表达式保持直观对应关系
// （便于审查），且 const 上下文中 `rotate_left` 在早期 Rust 版本可用性不一致。
#[allow(clippy::manual_rotate)]
const ROTATION_TABLES: [[u8; 256]; 8] = {
    let mut tables = [[0u8; 256]; 8];
    let mut amount = 1;
    while amount < 8 {
        let mut i = 0;
        while i < 256 {
            let v = i as u8;
            tables[amount][i] = (v << amount) | (v >> (8 - amount));
            i += 1;
        }
        amount += 1;
    }
    tables
};

/// 位旋转左移节点：parse 读至 EOF → 按 amount/group 位旋转 → 子流 → inner.parse。
///
/// 对应 Python construct `ProcessRotateLeft(amount, group, subcon)`（core.py L5424）。
/// 详见模块级注释。
#[derive(Debug)]
pub struct ProcessRotateLeftNode {
    /// 被包装的子树根。
    inner: Box<Node>,
    /// amount 表达式（常量编译期包装为单 op 程序）。
    amount: ExprProgram,
    /// group 表达式（同 amount）。
    group: ExprProgram,
}

impl ProcessRotateLeftNode {
    /// 创建 `ProcessRotateLeftNode`。
    pub fn new(inner: Node, amount: ExprProgram, group: ExprProgram) -> Self {
        Self {
            inner: Box::new(inner),
            amount,
            group,
        }
    }

    /// 返回内部子树根节点的引用。
    pub fn inner(&self) -> &Node {
        &self.inner
    }

    /// 返回 amount 表达式引用。
    pub fn amount(&self) -> &ExprProgram {
        &self.amount
    }

    /// 返回 group 表达式引用。
    pub fn group(&self) -> &ExprProgram {
        &self.group
    }
}

impl Construct for ProcessRotateLeftNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let amount = eval_expr_int(&self.amount, ctx, py)?;
        let group = eval_expr_int(&self.group, ctx, py)?;
        let offset = stream.tell();
        let data = &stream.data()[offset..];
        let transformed = rotate_left(data, amount as usize, group as usize, path)?;
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
        let amount = eval_expr_int(&self.amount, ctx, py)?;
        let group = eval_expr_int(&self.group, ctx, py)?;
        // build 取负（对齐 Python L5499：amount = -amount % (group*8)）
        let neg_amount = (-(amount)).rem_euclid(group * 8) as usize;
        let capacity = self.inner.sizeof(ctx).unwrap_or(0);
        let mut sub_stream = BuildStream::with_capacity(capacity);
        self.inner.build(py, obj, &mut sub_stream, ctx, path)?;
        let data = sub_stream.into_bytes();
        let transformed = rotate_left(&data, neg_amount, group as usize, path)?;
        stream.write(&transformed);
        Ok(())
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        self.inner.sizeof(ctx)
    }
}

/// 应用旋转（parse 路径，amount 为正）。
//
// clippy::manual_is_multiple_of：`x % y == 0` 与 `x.is_multiple_of(y)` 等价，
// 但 `is_multiple_of` 在 Rust 1.87 才稳定。本项目 MSRV 不保证 ≥1.87，保守用 %。
#[allow(clippy::manual_is_multiple_of)]
fn rotate_left(
    data: &[u8],
    amount: usize,
    group: usize,
    path: &mut Path,
) -> Result<Vec<u8>, ConstructError> {
    if group < 1 {
        return Err(ConstructError::Rotation {
            message: "group size must be at least 1 to be valid".to_string(),
            path: path.to_string(),
        });
    }
    if data.len() % group != 0 {
        return Err(ConstructError::Rotation {
            message: "data length must be a multiple of group size".to_string(),
            path: path.to_string(),
        });
    }
    let amount = amount % (group * 8);
    let amount_bytes = amount / 8;

    if amount == 0 {
        // 分支 1：不旋转
        return Ok(data.to_vec());
    }
    if group == 1 {
        // 分支 2：查表（amount 1..7，对齐 Python L5478）。
        // group==1 时 amount%8 = amount%（1*8=8） ∈ [1,7]
        let table = &ROTATION_TABLES[amount];
        return Ok(data.iter().map(|&b| table[b as usize]).collect());
    }
    if amount % 8 == 0 {
        // 分支 3：字节序重排（纯字节移位，对齐 Python L5482）
        let indices: Vec<usize> = (0..group).map(|i| (i + amount_bytes) % group).collect();
        let mut result = Vec::with_capacity(data.len());
        for chunk_start in (0..data.len()).step_by(group) {
            for &k in &indices {
                result.push(data[chunk_start + k]);
            }
        }
        return Ok(result);
    }
    // 分支 4：通用 bit rotate（对齐 Python L5488）
    let amount1 = amount % 8;
    let amount2 = 8 - amount1;
    let indices_pairs: Vec<(usize, usize)> = (0..group)
        .map(|i| ((i + amount_bytes) % group, (i + 1 + amount_bytes) % group))
        .collect();
    let mut result = Vec::with_capacity(data.len());
    for chunk_start in (0..data.len()).step_by(group) {
        for &(k1, k2) in &indices_pairs {
            // u8 << amount1 结果是 i32（整数提升），需 & 0xff 截回 u8（与 Python 一致）。
            // clippy::identity_op：& 0xff 在 u8 上下文看似冗余，但 << 提升为 i32
            // 后必须截断，否则或运算结果会溢出 u8 范围。
            #[allow(clippy::identity_op)]
            let rotated: u8 =
                ((data[chunk_start + k1] << amount1) & 0xff) | (data[chunk_start + k2] >> amount2);
            result.push(rotated);
        }
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{ExprOp, ExprProgram};
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

    fn make_rotate_node(amount: i64, group: i64) -> ProcessRotateLeftNode {
        let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
        let amount_prog = ExprProgram::new(vec![ExprOp::Const(amount)]);
        let group_prog = ExprProgram::new(vec![ExprOp::Const(group)]);
        ProcessRotateLeftNode::new(inner, amount_prog, group_prog)
    }

    #[test]
    fn rotation_table_correct_values() {
        // amount=4：0x0f → 0xf0, 0xf0 → 0x0f
        assert_eq!(ROTATION_TABLES[4][0x0f], 0xf0);
        assert_eq!(ROTATION_TABLES[4][0xf0], 0x0f);
        // amount=1：0x01 → 0x02
        assert_eq!(ROTATION_TABLES[1][0x01], 0x02);
    }

    #[test]
    fn parse_group1_uses_table() {
        // PR-1: ProcessRotateLeft(4, 1, Int16ub).parse(b'\x0f\xf0') → 查表 → b'\xf0\x0f' → 0xf00f
        with_py(|py| {
            let node = make_rotate_node(4, 1);
            let mut stream = ParseStream::new(&[0x0f, 0xf0]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let n: i64 = v.bind(py).extract().unwrap();
            assert_eq!(n, 0xf00f);
        });
    }

    #[test]
    fn parse_group2_uses_branch4() {
        // PR-2: ProcessRotateLeft(4, 2, Int16ub).parse(b'\x0f\xf0') → 分支 4 → 0xff00
        // amount=4, group=2: amount%16=4 ≠ 0; group≠1; amount%8=4 ≠ 0 → 分支 4
        with_py(|py| {
            let node = make_rotate_node(4, 2);
            let mut stream = ParseStream::new(&[0x0f, 0xf0]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let n: i64 = v.bind(py).extract().unwrap();
            assert_eq!(n, 0xff00);
        });
    }

    #[test]
    fn parse_zero_amount_fastpath() {
        // PR-3: ProcessRotateLeft(0, 1, Int16ub).parse(b'\x0f\xf0') → 不变 → 0x0ff0
        with_py(|py| {
            let node = make_rotate_node(0, 1);
            let mut stream = ParseStream::new(&[0x0f, 0xf0]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let n: i64 = v.bind(py).extract().unwrap();
            assert_eq!(n, 0x0ff0);
        });
    }

    #[test]
    fn parse_group_zero_raises() {
        // PR-5: ProcessRotateLeft(4, 0, ...) → RotationError
        with_py(|py| {
            let node = make_rotate_node(4, 0);
            let mut stream = ParseStream::new(&[0x0f, 0xf0]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Rotation { .. }));
        });
    }

    #[test]
    fn build_negates_amount() {
        // PR-7: ProcessRotateLeft(4, 1, Int16ub).build(0xf00f) → b'\x0f\xf0'
        with_py(|py| {
            let node = make_rotate_node(4, 1);
            let obj = py.eval_bound("0xf00f", None, None).expect("int");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x0f, 0xf0]);
        });
    }

    #[test]
    fn round_trip_group1() {
        // parse → build 可还原
        with_py(|py| {
            let node = make_rotate_node(4, 1);
            let mut stream = ParseStream::new(&[0x0f, 0xf0]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // build back
            let mut build_stream = BuildStream::new();
            node.build(py, v.bind(py), &mut build_stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(build_stream.as_bytes(), &[0x0f, 0xf0]);
        });
    }

    #[test]
    fn sizeof_forwards_to_inner() {
        with_py(|py| {
            let node = make_rotate_node(4, 1);
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).unwrap(), 2);
        });
    }

    #[test]
    fn debug_format_includes_rotate_node() {
        with_py(|_py| {
            let node = make_rotate_node(4, 1);
            let s = format!("{:?}", node);
            assert!(s.contains("ProcessRotateLeftNode"), "got: {}", s);
        });
    }
}
