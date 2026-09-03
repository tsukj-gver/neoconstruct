//! CheckNode：断言检查节点。
//!
//! Python 参考：`construct/construct/core.py` `Check`（L3081-3129）。
//!
//! ## 行为概述
//!
//! Check 是 Construct 非包装模式的 RO 字段：parse/build 求值表达式，非真抛 CheckError。
//! sizeof 恒为 0（不消费字节）。
//!
//! ## 表达式约束
//!
//! func 必须是字段表达式（FieldRef/ExprRef/int 组合），编译为 ExprProgram。
//! **不接收 Python lambda/callable**（与 RebuildNode 同约束）。

use crate::context::Context;
use crate::error::ConstructError;
use crate::expr::{eval_expr_bool, ExprProgram};
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

use super::Construct;

/// 断言检查节点：parse/build 求值表达式，非真抛 CheckError。
///
/// 对应 Python construct `Check(func)`（core.py L3081）。
///
/// **必须作为 RO 字段使用**（`rfield` 包装，与 Computed 同类）。
/// parse 返回 Py_None（无字段值）；build 求值表达式但不写字节。
///
/// # 表达式约束
///
/// func 必须是字段表达式（FieldRef/ExprRef/int 组合），编译为 ExprProgram。
/// **不接收 Python lambda/callable**。
#[derive(Debug)]
pub struct CheckNode {
    /// 待求值的断言表达式。
    func: ExprProgram,
}

impl CheckNode {
    /// 创建 `CheckNode`，绑定待求值的断言表达式。
    pub fn new(func: ExprProgram) -> Self {
        Self { func }
    }

    /// 返回断言表达式的引用。
    pub fn func(&self) -> &ExprProgram {
        &self.func
    }
}

impl Construct for CheckNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        _stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let passed = eval_expr_bool(&self.func, ctx, py)?;
        if !passed {
            return Err(ConstructError::Check {
                message: "check failed during parsing".to_string(),
                path: path.to_string(),
            });
        }
        Ok(py.None())
    }

    fn build(
        &self,
        py: Python<'_>,
        _obj: &Bound<'_, PyAny>,
        _stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        let passed = eval_expr_bool(&self.func, ctx, py)?;
        if !passed {
            return Err(ConstructError::Check {
                message: "check failed during building".to_string(),
                path: path.to_string(),
            });
        }
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        Ok(0)
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::ExprOp;
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
    fn new_stores_func() {
        let func = ExprProgram::new(vec![ExprOp::Const(1)]);
        let node = CheckNode::new(func);
        assert_eq!(node.func().ops(), &[ExprOp::Const(1)]);
    }

    // ======================================================================
    // parse — 常量真/假
    // ======================================================================

    #[test]
    fn parse_passing_returns_none() {
        // Check(1).parse(...) → None（const 1 视为真）
        with_py(|py| {
            let func = ExprProgram::new(vec![ExprOp::Const(1)]);
            let node = CheckNode::new(func);
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert!(result.is(&py.None()));
        });
    }

    #[test]
    fn parse_failing_raises_check_error() {
        // Check(0).parse(...) → CheckError（const 0 视为假）
        with_py(|py| {
            let func = ExprProgram::new(vec![ExprOp::Const(0)]);
            let node = CheckNode::new(func);
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Check { message, path: p } => {
                    assert!(message.contains("parsing"), "got: {}", message);
                    assert_eq!(p, "root");
                }
                other => panic!("expected Check error, got {:?}", other),
            }
        });
    }

    #[test]
    fn parse_with_field_expression_passes() {
        // 表达式 x > 5 where x=10 → 真
        with_py(|py| {
            // x > 5：GetInt(0) Const(5) Gt
            let func = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(5), ExprOp::Gt]);
            let node = CheckNode::new(func);
            let mut stream = ParseStream::new(b"");
            let mut ctx = make_context(py, &[("x", 10)]);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert!(result.is(&py.None()));
        });
    }

    #[test]
    fn parse_with_field_expression_fails() {
        // 表达式 x > 5 where x=3 → 假
        with_py(|py| {
            let func = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(5), ExprOp::Gt]);
            let node = CheckNode::new(func);
            let mut stream = ParseStream::new(b"");
            let mut ctx = make_context(py, &[("x", 3)]);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Check { .. }));
        });
    }

    // ======================================================================
    // build — 求值断言
    // ======================================================================

    #[test]
    fn build_failing_raises_check_error() {
        // Check(0).build(...) → CheckError
        with_py(|py| {
            let func = ExprProgram::new(vec![ExprOp::Const(0)]);
            let node = CheckNode::new(func);
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Check { message, .. } => {
                    assert!(message.contains("building"), "got: {}", message);
                }
                other => panic!("expected Check error, got {:?}", other),
            }
        });
    }

    #[test]
    fn build_passing_writes_no_bytes() {
        with_py(|py| {
            let func = ExprProgram::new(vec![ExprOp::Const(1)]);
            let node = CheckNode::new(func);
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert!(stream.as_bytes().is_empty());
        });
    }

    // ======================================================================
    // sizeof — 恒为 0
    // ======================================================================

    #[test]
    fn sizeof_returns_zero() {
        with_py(|py| {
            let func = ExprProgram::new(vec![ExprOp::Const(1)]);
            let node = CheckNode::new(func);
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).unwrap(), 0);
        });
    }

    // ======================================================================
    // Debug
    // ======================================================================

    #[test]
    fn debug_format_includes_check_node() {
        let func = ExprProgram::new(vec![ExprOp::Const(1)]);
        let node = CheckNode::new(func);
        let s = format!("{:?}", node);
        assert!(s.contains("CheckNode"), "got: {}", s);
    }
}
