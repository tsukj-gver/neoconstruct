//! ComputedNode：从表达式计算值（不消费字节）。
//!
//! Python 参考：`construct/construct/core.py` `Computed`（L3358-3400）。
//!
//! ## 行为概述
//!
//! Computed 是 RO（只读）节点：parse 通过 [`crate::expr::eval_expr_int`]
//! 求值表达式并返回 `PyLong`。sizeof 恒为 0。build 方向是 no-op——值计算
//! 由 [`crate::nodes::struct_node`] 的 `compute_ro_value` 在 RO 字段处理时完成。
//!
//! ## 典型用法
//!
//! ```python
//! @dataclass
//! class Packet(StructMixin):
//!     start: int = rfield(Tell())
//!     count: int = field(Int8ub)
//!     end: int = rfield(Tell())
//!     size: int = rfield(Computed(end - start))
//! ```

use crate::context::Context;
use crate::error::ConstructError;
use crate::expr::{eval_expr_int, ExprProgram};
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

use super::Construct;

/// 从表达式计算值。对应 Python construct 的 `Computed`。
///
/// parse 通过 [`eval_expr_int`] 求值表达式，返回 `PyLong`。
/// sizeof 返回 0。
///
/// build 方向是 no-op：值计算由 [`crate::nodes::struct_node`] 的
/// `compute_ro_value` 在 RO 字段处理时完成。
#[derive(Debug, Clone)]
pub struct ComputedNode {
    /// 计算表达式（编译后的 ExprProgram）。
    expr: ExprProgram,
}

impl ComputedNode {
    /// 创建一个 `ComputedNode`，携带给定的表达式程序。
    pub fn new(expr: ExprProgram) -> Self {
        Self { expr }
    }

    /// 返回表达式的只读引用（供 `compute_ro_value` 在 build 方向求值）。
    pub fn expr(&self) -> &ExprProgram {
        &self.expr
    }
}

impl Construct for ComputedNode {
    fn parse(
        &self,
        py: Python<'_>,
        _stream: &mut ParseStream<'_>,
        ctx: &mut Context<'_>,
        _path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let value = eval_expr_int(&self.expr, ctx, py)?;
        Ok(value.into_py(py)) // i64 → PyLong
    }

    fn build(
        &self,
        _py: Python<'_>,
        _obj: &Bound<'_, PyAny>,
        _stream: &mut BuildStream,
        _ctx: &mut Context<'_>,
        _path: &mut Path,
    ) -> Result<(), ConstructError> {
        // Computed build 是 no-op。值计算由 StructNode 的 compute_ro_value 处理。
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
    ///
    /// 使用 Vec 化 API：`init_expr_values` + `set_field_at`。
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
    // 构造器与访问器
    // ======================================================================

    #[test]
    fn new_stores_expr_program() {
        let prog = ExprProgram::new(vec![ExprOp::Const(42)]);
        let node = ComputedNode::new(prog);
        assert_eq!(node.expr().ops(), &[ExprOp::Const(42)]);
        assert!(!node.expr().is_empty());
    }

    #[test]
    fn expr_returns_reference_to_program() {
        let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(2), ExprOp::Mul]);
        let node = ComputedNode::new(prog);
        let expr_ref = node.expr();
        assert_eq!(expr_ref.ops().len(), 3);
        assert_eq!(expr_ref.max_stack(), 2);
    }

    #[test]
    fn clone_preserves_expr() {
        let prog = ExprProgram::new(vec![ExprOp::Const(7)]);
        let node = ComputedNode::new(prog);
        let cloned = node.clone();
        assert_eq!(cloned.expr().ops(), node.expr().ops());
    }

    // ======================================================================
    // parse：简单字段引用
    // ======================================================================

    #[test]
    fn parse_simple_field_reference() {
        // Computed(count) → 引用 count 字段
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0)]);
            let node = ComputedNode::new(prog);
            let mut ctx = make_context(py, &[("count", 42)]);
            let mut stream = ParseStream::new(b"");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let n: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(n, 42);
        });
    }

    #[test]
    fn parse_returns_pylong_type() {
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::Const(100)]);
            let node = ComputedNode::new(prog);
            let mut ctx = make_context(py, &[]);
            let mut stream = ParseStream::new(b"");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let type_name = result
                .bind(py)
                .get_type()
                .name()
                .expect("type name")
                .to_string();
            assert_eq!(type_name, "int");
        });
    }

    // ======================================================================
    // parse：算术表达式
    // ======================================================================

    #[test]
    fn parse_end_minus_start_expression() {
        // Computed(end - start) → [GetInt(end), GetInt(start), Sub]
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::GetInt(1), ExprOp::GetInt(0), ExprOp::Sub]);
            let node = ComputedNode::new(prog);
            let mut ctx = make_context(py, &[("start", 0), ("end", 4)]);
            let mut stream = ParseStream::new(b"");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let n: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(n, 4);
        });
    }

    #[test]
    fn parse_count_times_two_expression() {
        // Computed(count * 2)
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(2), ExprOp::Mul]);
            let node = ComputedNode::new(prog);
            let mut ctx = make_context(py, &[("count", 5)]);
            let mut stream = ParseStream::new(b"");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let n: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(n, 10);
        });
    }

    #[test]
    fn parse_constant_expression() {
        // Computed(42) → [Const(42)]
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::Const(42)]);
            let node = ComputedNode::new(prog);
            let mut ctx = make_context(py, &[]);
            let mut stream = ParseStream::new(b"");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let n: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(n, 42);
        });
    }

    #[test]
    fn parse_negative_result() {
        // Computed(0 - count) → [Const(0), GetInt(0), Sub] → -count
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::Const(0), ExprOp::GetInt(0), ExprOp::Sub]);
            let node = ComputedNode::new(prog);
            let mut ctx = make_context(py, &[("count", 17)]);
            let mut stream = ParseStream::new(b"");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let n: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(n, -17);
        });
    }

    // ======================================================================
    // parse：嵌套表达式（引用前序 RO 字段）
    // ======================================================================

    #[test]
    fn parse_chained_reference_expression() {
        // Computed(size * 2 + 1)，size 是前序 RO 字段
        // [GetInt(0), Const(2), Mul, Const(1), Add]
        with_py(|py| {
            let prog = ExprProgram::new(vec![
                ExprOp::GetInt(0),
                ExprOp::Const(2),
                ExprOp::Mul,
                ExprOp::Const(1),
                ExprOp::Add,
            ]);
            let node = ComputedNode::new(prog);
            let mut ctx = make_context(py, &[("size", 5)]);
            let mut stream = ParseStream::new(b"");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let n: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(n, 11);
        });
    }

    // ======================================================================
    // parse：错误路径
    // ======================================================================

    #[test]
    fn parse_on_placeholder_context_returns_expr_context_error() {
        // GetInt(0) 需要 expr_values，但 placeholder 未调用 init_expr_values → ExprContext。
        // （Const-only 表达式不需要 context，在 placeholder 上也能求值——这是正确行为。）
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0)]);
            let node = ComputedNode::new(prog);
            let mut ctx = Context::placeholder(py);
            let mut stream = ParseStream::new(b"");
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::ExprContext { message, .. } => {
                    assert!(message.contains("not initialized"), "got: {}", message);
                }
                other => panic!("expected ExprContext, got {:?}", other),
            }
        });
    }

    #[test]
    fn parse_missing_field_returns_expr_field_missing_error() {
        // 引用 idx 1（null 槽位，模拟 WO 字段未写入）→ ExprFieldMissing。
        with_py(|py| {
            // 手动构造 ctx：init_expr_values(2)，但只 set_field_at(0, "a")。
            let mut ctx = Context::new_root(py).expect("new_root");
            ctx.init_expr_values(2);
            let key = PyString::new_bound(py, "a").unbind();
            let val = 1i64.into_py(py);
            ctx.set_field_at(0, &key, val.bind(py), py).expect("set a");
            // idx 1 保持 null（模拟 WO 或未设置的字段）

            let prog = ExprProgram::new(vec![ExprOp::GetInt(1)]); // 引用 null 槽位
            let node = ComputedNode::new(prog);
            let mut stream = ParseStream::new(b"");
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::ExprFieldMissing { field, .. } => {
                    assert!(
                        field.contains("1"),
                        "field should contain index 1: {}",
                        field
                    );
                }
                other => panic!("expected ExprFieldMissing, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // parse：不消费字节
    // ======================================================================

    #[test]
    fn parse_does_not_consume_bytes() {
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::Const(42)]);
            let node = ComputedNode::new(prog);
            let mut ctx = make_context(py, &[]);
            let mut stream = ParseStream::new(b"abc");
            let mut path = Path::new();
            let _ = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert_eq!(stream.tell(), 0, "Computed should not advance stream");
        });
    }

    // ======================================================================
    // build (no-op)
    // ======================================================================

    #[test]
    fn build_is_noop_returns_ok() {
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::Const(42)]);
            let node = ComputedNode::new(prog);
            let obj = py.eval_bound("0", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert!(
                stream.as_bytes().is_empty(),
                "Computed build should not write"
            );
        });
    }

    #[test]
    fn build_does_not_advance_stream() {
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::Const(0)]);
            let node = ComputedNode::new(prog);
            let obj = py.eval_bound("None", None, None).expect("obj");
            let mut stream = BuildStream::new();
            stream.write(b"abc");
            let pos_before = stream.tell();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.tell(), pos_before);
        });
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_returns_zero() {
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::Const(42)]);
            let node = ComputedNode::new(prog);
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).expect("sizeof"), 0);
        });
    }

    // ======================================================================
    // Debug
    // ======================================================================

    #[test]
    fn debug_format_includes_computed_node() {
        let prog = ExprProgram::new(vec![ExprOp::Const(42)]);
        let node = ComputedNode::new(prog);
        let s = format!("{:?}", node);
        assert!(s.contains("ComputedNode"), "got: {}", s);
    }
}
