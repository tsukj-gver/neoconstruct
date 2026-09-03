//! RebuildNode：build 时基于表达式重算字段的节点。
//!
//! Python 参考：`construct/construct/core.py` `Rebuild`（L2975-3028）。
//!
//! ## 行为概述
//!
//! Rebuild 的本质是"build 值不来自用户输入，而来自表达式求值"——与 Computed 同类。
//! parse 转发 inner.parse；build 求值表达式得到 i64，转 PyLong 后交给 inner.build。
//!
//! ## construct-rs 集成：作为 RO 字段
//!
//! Rebuild 必须作为 `FieldMode::Ro` 字段使用（用户写
//! `count: int = rfield(Rebuild(Byte, items.length))`）。
//! [`crate::nodes::Node::compute_ro_value`] 对 Rebuild 分支调表达式求值，
//! 返回 PyLong 供 StructNode 写入 context（供后续表达式引用）。
//!
//! ## 表达式系统
//!
//! `func` 必须是字段表达式（FieldRef/ExprRef/int 组合），编译为 ExprProgram，
//! 运行时零 FFI 求值。**不接收 Python lambda/callable**。

use crate::context::Context;
use crate::error::ConstructError;
use crate::expr::{eval_expr_int, ExprProgram};
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

use super::Construct;
use crate::nodes::Node;

/// build 时重算字段节点。
///
/// 对应 Python construct `Rebuild(subcon, func)`（core.py L2975）。
///
/// # 三方法行为
///
/// - parse：转发 `inner.parse(...)`（继承自 Subconstruct）
/// - build：忽略传入 obj，求值 func 表达式得到 i64 → 转 PyLong → `inner.build(value)`
/// - sizeof：`inner.sizeof(ctx)`
///
/// # RO 字段集成
///
/// Rebuild 必须作为 `FieldMode::Ro` 字段使用（与 Computed/Tell 同类）。
/// [`crate::nodes::Node::compute_ro_value`] 对 Rebuild 分支调 `eval_expr_int`，
/// 返回 PyLong 供 StructNode 写入 context（供后续表达式引用）。
///
/// 字段包装约定：RO 用 `rfield(...)`（parse 后有值）；v0.1.1 起 `field(...)`
/// （RW）也可用——隐式 default=None，build 时实例传入值被忽略（用表达式值）。
///
/// # 表达式约束
///
/// `func` 必须是字段表达式（FieldRef/ExprRef/int 组合），编译为 ExprProgram，
/// 运行时零 FFI 求值。**不接收 Python lambda/callable**。
#[derive(Debug)]
pub struct RebuildNode {
    /// 被包装的子树根。
    inner: Box<Node>,
    /// build 时求值的表达式（编译为 ExprProgram）。
    func: ExprProgram,
}

impl RebuildNode {
    /// 创建 `RebuildNode`，包裹给定的子树根节点与 build 表达式。
    ///
    /// # 参数
    ///
    /// - `inner`：被包装的子树（用于 parse 和 sizeof，以及 build 的最终写入）。
    /// - `func`：build 时求值的表达式程序（i64 表达式 VM）。
    pub fn new(inner: Node, func: ExprProgram) -> Self {
        Self {
            inner: Box::new(inner),
            func,
        }
    }

    /// 返回内部子树根节点的引用。
    pub fn inner(&self) -> &Node {
        &self.inner
    }

    /// 返回 build 表达式的只读引用（供 `compute_ro_value` 在 build 方向求值）。
    pub fn func(&self) -> &ExprProgram {
        &self.func
    }
}

impl Construct for RebuildNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // parse 转发 inner
        self.inner.parse(py, stream, ctx, path)
    }

    fn build(
        &self,
        py: Python<'_>,
        _obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        // build：求值表达式得到 i64，转 PyLong，再交给 inner.build。
        // 注：Rebuild 作为 RO 字段时，obj 由 StructNode.compute_ro_value 提供
        // （compute_ro_value 调本节点的 compute_ro_value 分支，详见 mod.rs）。
        // 此处 build 的 obj 是 compute_ro_value 的返回值（PyLong）。
        // 但 Python 语义是"忽略 obj，重算"——为对齐 Python，build 内部仍调
        // 表达式求值（即使 obj 已是 compute_ro_value 计算的值）。
        // 双重求值的开销：仅一次 ExprProgram::eval（~10ns），可忽略。
        let value_i64 = eval_expr_int(&self.func, ctx, py)?;
        let value_py = value_i64.into_py(py);
        self.inner.build(py, value_py.bind(py), stream, ctx, path)
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        self.inner.sizeof(ctx)
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::ExprOp;
    use crate::nodes::format_field::{FormatFieldNode, PythonFormat};
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

    /// 构造一个含若干整数字段的 `Context`，用于表达式求值测试。
    fn make_context<'py>(py: Python<'py>, entries: &[(&str, i64)]) -> Context<'py> {
        let mut ctx = Context::new_root(py).expect("new_root");
        ctx.init_expr_values(entries.len());
        for (idx, (name, value)) in entries.iter().enumerate() {
            let key = PyString::new_bound(py, name).unbind();
            let val = (*value).into_py(py);
            ctx.set_field_at(idx, &key, val.bind(py), py)
                .expect("set_field_at");
        }
        ctx
    }

    // ======================================================================
    // 构造器
    // ======================================================================

    #[test]
    fn new_stores_inner_and_func() {
        let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
        let func = ExprProgram::new(vec![ExprOp::Const(42)]);
        let node = RebuildNode::new(inner, func);
        assert!(matches!(node.inner(), Node::FormatField(_)));
        assert_eq!(node.func().ops(), &[ExprOp::Const(42)]);
    }

    // ======================================================================
    // parse — 转发 inner
    // ======================================================================

    #[test]
    fn parse_forwards_to_inner() {
        // Rebuild(Byte, items.length).parse(b"\x03") → 3
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let func = ExprProgram::new(vec![ExprOp::Const(0)]);
            let node = RebuildNode::new(inner, func);

            let mut stream = ParseStream::new(&[0x03]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().unwrap();
            assert_eq!(v, 3);
        });
    }

    // ======================================================================
    // build — 求值表达式 + inner.build
    // ======================================================================

    #[test]
    fn build_uses_expression_value_constant() {
        // 表达式为常量 → inner.build(const)
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let func = ExprProgram::new(vec![ExprOp::Const(42)]);
            let node = RebuildNode::new(inner, func);

            // obj 被 Rebuild 忽略（Python 语义）
            let obj = py.eval_bound("None", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[42]);
        });
    }

    #[test]
    fn build_uses_expression_value_from_context() {
        // 表达式引用 context 字段
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            // func: items_length（GetInt(0)）
            let func = ExprProgram::new(vec![ExprOp::GetInt(0)]);
            let node = RebuildNode::new(inner, func);

            let obj = py.eval_bound("None", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = make_context(py, &[("items_length", 5)]);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[5]);
        });
    }

    #[test]
    fn build_uses_arithmetic_expression() {
        // 表达式：items_length * 2 → 写入 Int8ub
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let func = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(2), ExprOp::Mul]);
            let node = RebuildNode::new(inner, func);

            let obj = py.eval_bound("None", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = make_context(py, &[("n", 6)]);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[12]);
        });
    }

    #[test]
    fn build_ignores_obj_value() {
        // 即使 obj 是有效值，Rebuild 仍用表达式重算（对齐 Python）
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let func = ExprProgram::new(vec![ExprOp::Const(99)]);
            let node = RebuildNode::new(inner, func);

            let obj = py.eval_bound("0", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            // 应写 99（来自表达式），不是 0（来自 obj）
            assert_eq!(stream.as_bytes(), &[99]);
        });
    }

    #[test]
    fn build_propagates_expression_error() {
        // 表达式求值失败（引用不存在的字段）→ 错误向上传播
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            // GetInt(0) 引用 idx 0，但 placeholder ctx 未 init_expr_values
            let func = ExprProgram::new(vec![ExprOp::GetInt(0)]);
            let node = RebuildNode::new(inner, func);

            let obj = py.eval_bound("None", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            // ExprContext 错误（placeholder ctx 未 init）
            assert!(
                matches!(err, ConstructError::ExprContext { .. }),
                "got: {:?}",
                err
            );
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
            let func = ExprProgram::new(vec![ExprOp::Const(0)]);
            let node = RebuildNode::new(inner, func);
            assert_eq!(node.sizeof(&ctx).unwrap(), 2);
        });
    }

    // ======================================================================
    // parse ↔ build 往返（结合 context）
    // ======================================================================

    #[test]
    fn build_then_parse_round_trip() {
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let func = ExprProgram::new(vec![ExprOp::Const(7)]);
            let node = RebuildNode::new(inner, func);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            let obj = py.eval_bound("None", None, None).expect("obj");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();

            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().unwrap();
            assert_eq!(v, 7);
        });
    }

    // ======================================================================
    // Debug
    // ======================================================================

    #[test]
    fn debug_format_includes_rebuild_node() {
        let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
        let func = ExprProgram::new(vec![ExprOp::Const(0)]);
        let node = RebuildNode::new(inner, func);
        let s = format!("{:?}", node);
        assert!(s.contains("RebuildNode"), "got: {}", s);
    }
}
