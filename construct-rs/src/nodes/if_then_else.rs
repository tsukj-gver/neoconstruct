//! IfThenElseNode：双分支条件节点。
//!
//! 设计依据：`docs/design/模块设计/模块设计-Conditional.md` §2。
//! Python 参考：`construct/construct/core.py` `IfThenElse`（L3944-3987）。
//!
//! ## 行为概述
//!
//! `IfThenElse(condfunc, thensubcon, elsesubcon)` 求值 condfunc，根据结果选择
//! then/else 子树委托 parse/build/sizeof。
//!
//! - parse：求值 cond → 选 then/else → 委托 subcon.parse
//! - build：求值 cond → 选 then/else → 委托 subcon.build
//! - sizeof：仅常量条件返回确定值；Expr 条件返回 Err（无法在 sizeof 接口求值）
//!
//! ## Condition 模式（复用 StopIfCondition，设计 §2.1）
//!
//! ARCH 决策：复用 Phase 4 的 [`StopIfCondition`]（不新建 `IfThenElseCondition`）。
//! 理由：同构类型 + L-04 对策（跨阶段模式未沉淀）。详见设计 §2.1。
//!
//! ## If macro（Python 用户面）
//!
//! `If(cond, sub) = IfThenElse(cond, sub, Pass)`（Python L3935）。
//! Rust 不新增 IfNode——Python 用户面 macro 完成等价转换（设计 §2.5）。

use crate::context::Context;
use crate::error::ConstructError;
use crate::nodes::stop_if::{eval_condition, StopIfCondition};
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

use super::{Construct, Node};

/// 双分支条件节点（对应 Python construct `IfThenElse`，core.py L3944）。
///
/// # 三方法行为
///
/// 对齐 Python `IfThenElse._parse` / `_build` / `_sizeof`（L3974-3987）：
/// - parse：求值 cond → 选 then/else → 委托 subcon.parse。
/// - build：求值 cond → 选 then/else → 委托 subcon.build。
/// - sizeof：求值 cond → 选 then/else → 委托 subcon.sizeof。
///
/// 求值失败（字段缺失等）错误向上传播（与 Computed/StopIf 同路径）。
///
/// # Condition 模式（复用 StopIfCondition，设计 §2.1）
///
/// - [`StopIfCondition::Always`] → 走 then 分支
/// - [`StopIfCondition::Never`] → 走 else 分支
/// - [`StopIfCondition::Expr`] → 求值非零走 then，为零走 else（对齐 Python truthy 语义）
///
/// # If macro
///
/// Python `If(cond, sub) = IfThenElse(cond, sub, Pass)`（L3935）。
/// Rust 不新增 IfNode——Python 用户面 macro 完成等价转换。
///
/// [设计质疑]：设计文档 §2.2 标注 `#[derive(Debug, Clone)]`，但 `Node` enum
/// 整体未实现 `Clone`（含 `Py<PyType>` / `FieldName` 等非 Clone 字段）。删除
/// `Clone` derive，与 `BitwiseNode` / `TransformNode` 等持有 `Box<Node>` 的
/// 节点保持一致。如未来需 Clone 能力，应统一为 Node 实现 Clone。
#[derive(Debug)]
pub struct IfThenElseNode {
    /// 条件：常量或表达式（复用 StopIfCondition）。
    cond: StopIfCondition,
    /// 条件为真时委托的子树。
    then_sub: Box<Node>,
    /// 条件为假时委托的子树（通常为 PassNode）。
    else_sub: Box<Node>,
}

impl IfThenElseNode {
    /// 创建 `IfThenElseNode`，包含给定的条件与 then/else 子树。
    pub fn new(cond: StopIfCondition, then_sub: Node, else_sub: Node) -> Self {
        Self {
            cond,
            then_sub: Box::new(then_sub),
            else_sub: Box::new(else_sub),
        }
    }

    /// 返回条件引用。
    pub fn cond(&self) -> &StopIfCondition {
        &self.cond
    }

    /// 返回 then 分支子树引用。
    pub fn then_sub(&self) -> &Node {
        &self.then_sub
    }

    /// 返回 else 分支子树引用。
    pub fn else_sub(&self) -> &Node {
        &self.else_sub
    }

    /// has_expressions：仅 Expr 条件返回 true（触发 StructNode init_expr_values）。
    /// 与 [`crate::nodes::stop_if::StopIfNode::has_expressions`] 同模式。
    pub fn has_expressions(&self) -> bool {
        self.cond.is_expr()
    }
}

impl Construct for IfThenElseNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 求值 cond（复用 eval_condition 公共辅助，设计 §2.3）。
        let take_then = eval_condition(&self.cond, ctx, py)?;
        let sub = if take_then {
            &self.then_sub
        } else {
            &self.else_sub
        };
        sub.parse(py, stream, ctx, path)
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        let take_then = eval_condition(&self.cond, ctx, py)?;
        let sub = if take_then {
            &self.then_sub
        } else {
            &self.else_sub
        };
        sub.build(py, obj, stream, ctx, path)
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // 对齐 Python L3984-3987：求值 cond 选 subcon sizeof。
        // sizeof 接口无 py token——但 Expr 求值需要 py（GetInt 调 PyDict）。
        // 处理方式（与 ComputedNode::sizeof 同模式）：sizeof 仅对常量条件返回；
        // Expr 条件下抛 Generic（SizeofError 等价）——因为无法在 sizeof 接口内求值。
        match &self.cond {
            StopIfCondition::Always => self.then_sub.sizeof(ctx),
            StopIfCondition::Never => self.else_sub.sizeof(ctx),
            StopIfCondition::Expr(_) => Err(ConstructError::Generic {
                message: "IfThenElse size is undefined when condition is an expression".to_string(),
                path: String::new(),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{ExprOp, ExprProgram};
    use crate::nodes::format_field::{FormatFieldNode, PythonFormat};
    use crate::nodes::pass::PassNode;
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

    /// 构造 `GetInt(idx) > const` 表达式程序（IF-3 / IF-4）。
    fn gt_expr(idx: usize, value: i64) -> ExprProgram {
        ExprProgram::new(vec![ExprOp::GetInt(idx), ExprOp::Const(value), ExprOp::Gt])
    }

    /// 构造 `GetInt(idx) == const` 表达式程序。
    fn eq_expr(idx: usize, value: i64) -> ExprProgram {
        ExprProgram::new(vec![ExprOp::GetInt(idx), ExprOp::Const(value), ExprOp::Eq])
    }

    fn fmt_node(fmt: PythonFormat) -> Node {
        Node::FormatField(FormatFieldNode::new(fmt))
    }

    // ======================================================================
    // 构造器
    // ======================================================================

    #[test]
    fn new_always_stores_then_branch() {
        let then = fmt_node(PythonFormat::UnsignedInt8Big);
        let else_ = fmt_node(PythonFormat::UnsignedInt16Big);
        let node = IfThenElseNode::new(StopIfCondition::Always, then, else_);
        assert!(matches!(node.cond(), StopIfCondition::Always));
        assert!(matches!(node.then_sub(), Node::FormatField(_)));
        assert!(matches!(node.else_sub(), Node::FormatField(_)));
    }

    #[test]
    fn has_expressions_only_for_expr_cond() {
        // Always / Never → false
        assert!(!IfThenElseNode::new(
            StopIfCondition::Always,
            fmt_node(PythonFormat::UnsignedInt8Big),
            fmt_node(PythonFormat::UnsignedInt16Big)
        )
        .has_expressions());
        assert!(!IfThenElseNode::new(
            StopIfCondition::Never,
            fmt_node(PythonFormat::UnsignedInt8Big),
            fmt_node(PythonFormat::UnsignedInt16Big)
        )
        .has_expressions());
        // Expr → true
        assert!(IfThenElseNode::new(
            StopIfCondition::Expr(eq_expr(0, 0)),
            fmt_node(PythonFormat::UnsignedInt8Big),
            fmt_node(PythonFormat::UnsignedInt16Big)
        )
        .has_expressions());
    }

    // ======================================================================
    // IF-1/IF-2：常量条件
    // ======================================================================

    #[test]
    fn parse_always_takes_then_branch() {
        // IF-1: cond=True → Always → 走 then
        with_py(|py| {
            let node = IfThenElseNode::new(
                StopIfCondition::Always,
                fmt_node(PythonFormat::UnsignedInt8Big),
                fmt_node(PythonFormat::UnsignedInt16Big),
            );
            let mut stream = ParseStream::new(&[0xFF]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().expect("i64");
            assert_eq!(v, 0xFF);
            assert_eq!(stream.tell(), 1, "then branch consumed 1 byte");
        });
    }

    #[test]
    fn parse_never_takes_else_branch() {
        // IF-2: cond=False → Never → 走 else
        with_py(|py| {
            let node = IfThenElseNode::new(
                StopIfCondition::Never,
                fmt_node(PythonFormat::UnsignedInt8Big),
                fmt_node(PythonFormat::UnsignedInt16Big),
            );
            let mut stream = ParseStream::new(&[0xAA, 0xBB]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().expect("i64");
            assert_eq!(v, 0xAABB, "else branch parses Int16ub big-endian");
            assert_eq!(stream.tell(), 2);
        });
    }

    // ======================================================================
    // IF-3/IF-4：Expr 条件
    // ======================================================================

    #[test]
    fn parse_expr_cond_true_takes_then() {
        // IF-3: cond = this.x > 0, x=5 → 真 → then (Int8ub)
        with_py(|py| {
            use pyo3::types::PyString;
            let node = IfThenElseNode::new(
                StopIfCondition::Expr(gt_expr(0, 0)),
                fmt_node(PythonFormat::UnsignedInt8Big),
                fmt_node(PythonFormat::UnsignedInt16Big),
            );
            let mut stream = ParseStream::new(&[0xFF]);
            let mut ctx = Context::new_root(py).expect("ctx");
            ctx.init_expr_values(1);
            let key = PyString::new_bound(py, "x").unbind();
            let val = 5i64.into_py(py);
            ctx.set_field_at(0, &key, val.bind(py), py).expect("set");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().expect("i64");
            assert_eq!(v, 0xFF);
        });
    }

    #[test]
    fn parse_expr_cond_false_takes_else() {
        // IF-4: cond = this.x > 0, x=0 → 假 → else (Int16ub)
        with_py(|py| {
            use pyo3::types::PyString;
            let node = IfThenElseNode::new(
                StopIfCondition::Expr(gt_expr(0, 0)),
                fmt_node(PythonFormat::UnsignedInt8Big),
                fmt_node(PythonFormat::UnsignedInt16Big),
            );
            let mut stream = ParseStream::new(&[0xAA, 0xBB]);
            let mut ctx = Context::new_root(py).expect("ctx");
            ctx.init_expr_values(1);
            let key = PyString::new_bound(py, "x").unbind();
            let val = 0i64.into_py(py);
            ctx.set_field_at(0, &key, val.bind(py), py).expect("set");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().expect("i64");
            assert_eq!(v, 0xAABB);
        });
    }

    // ======================================================================
    // IF-5：Expr 字段缺失 → ExprFieldMissing
    // ======================================================================

    #[test]
    fn parse_expr_with_missing_field_returns_error() {
        with_py(|py| {
            let node = IfThenElseNode::new(
                StopIfCondition::Expr(eq_expr(0, 0)),
                fmt_node(PythonFormat::UnsignedInt8Big),
                fmt_node(PythonFormat::UnsignedInt16Big),
            );
            let mut stream = ParseStream::new(&[]);
            let mut ctx = Context::new_root(py).expect("ctx");
            ctx.init_expr_values(1);
            // 不设置字段 → 槽位为 null → ExprFieldMissing
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("ExprFieldMissing");
            assert!(matches!(err, ConstructError::ExprFieldMissing { .. }));
        });
    }

    // ======================================================================
    // build 对称性
    // ======================================================================

    #[test]
    fn build_always_takes_then_branch() {
        with_py(|py| {
            let node = IfThenElseNode::new(
                StopIfCondition::Always,
                fmt_node(PythonFormat::UnsignedInt8Big),
                fmt_node(PythonFormat::UnsignedInt16Big),
            );
            let obj = py.eval_bound("0x42", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x42]);
        });
    }

    #[test]
    fn build_never_takes_else_branch() {
        with_py(|py| {
            let node = IfThenElseNode::new(
                StopIfCondition::Never,
                fmt_node(PythonFormat::UnsignedInt8Big),
                fmt_node(PythonFormat::UnsignedInt16Big),
            );
            let obj = py.eval_bound("0x42", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x00, 0x42]);
        });
    }

    // ======================================================================
    // IF-6/IF-9：sizeof + If macro 等价（Pass else 分支）
    // ======================================================================

    #[test]
    fn sizeof_always_returns_then_size() {
        with_py(|py| {
            let node = IfThenElseNode::new(
                StopIfCondition::Always,
                fmt_node(PythonFormat::UnsignedInt32Big),
                fmt_node(PythonFormat::UnsignedInt8Big),
            );
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).expect("sizeof"), 4);
        });
    }

    #[test]
    fn sizeof_never_returns_else_size() {
        with_py(|py| {
            let node = IfThenElseNode::new(
                StopIfCondition::Never,
                fmt_node(PythonFormat::UnsignedInt32Big),
                fmt_node(PythonFormat::UnsignedInt8Big),
            );
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).expect("sizeof"), 1);
        });
    }

    #[test]
    fn sizeof_expr_cond_returns_error() {
        // IF-6: Expr 条件下 sizeof 抛 Generic（SizeofError 等价）
        with_py(|py| {
            let node = IfThenElseNode::new(
                StopIfCondition::Expr(eq_expr(0, 0)),
                fmt_node(PythonFormat::UnsignedInt8Big),
                fmt_node(PythonFormat::UnsignedInt16Big),
            );
            let ctx = Context::placeholder(py);
            let err = node.sizeof(&ctx).expect_err("sizeof");
            match err {
                ConstructError::Generic { message, .. } => {
                    assert!(message.contains("IfThenElse"), "got: {}", message);
                }
                other => panic!("expected Generic, got {:?}", other),
            }
        });
    }

    #[test]
    fn if_macro_equivalent_with_pass_else() {
        // IF-9: If(True, Byte) ≡ IfThenElse(True, Byte, Pass)
        // Pass.parse 返回 None，Pass.build 不写字节
        with_py(|py| {
            let node = IfThenElseNode::new(
                StopIfCondition::Always,
                fmt_node(PythonFormat::UnsignedInt8Big),
                Node::Pass(PassNode::new()),
            );
            // Always → then (Byte)
            let mut stream = ParseStream::new(&[0x42]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let r = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = r.bind(py).extract().expect("i64");
            assert_eq!(v, 0x42);

            // Never → else (Pass) → 返回 None
            let node2 = IfThenElseNode::new(
                StopIfCondition::Never,
                fmt_node(PythonFormat::UnsignedInt8Big),
                Node::Pass(PassNode::new()),
            );
            let mut stream2 = ParseStream::new(&[]);
            let r2 = node2
                .parse(py, &mut stream2, &mut ctx, &mut path)
                .expect("parse Pass");
            assert!(r2.bind(py).is_none(), "Pass.parse returns None");
        });
    }

    // ======================================================================
    // Debug
    // ======================================================================

    #[test]
    fn debug_format_includes_if_then_else_node() {
        let node = IfThenElseNode::new(
            StopIfCondition::Always,
            fmt_node(PythonFormat::UnsignedInt8Big),
            fmt_node(PythonFormat::UnsignedInt16Big),
        );
        let s = format!("{:?}", node);
        assert!(s.contains("IfThenElseNode"), "got: {}", s);
    }
}
