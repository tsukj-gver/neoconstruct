//! StopIfNode：早停信号节点。
//!
//! 设计依据：`docs/模块设计-Array.md` §4.5。
//! Python 参考：`construct/construct/core.py` `StopIf`（L4079-4131）。
//!
//! ## 概述
//!
//! `StopIf(condfunc)` 检查条件，条件为真时返回 [`ConstructError::StopField`] 哨兵。
//! 哨兵由外层的 [`crate::nodes::struct_node::StructNode`] /
//! [`crate::nodes::greedy_range::GreedyRangeNode`] 捕获，正常停止后续字段/迭代。
//!
//! - sizeof 永远返回 `Err`（对齐 Python `StopIf._sizeof`，SI-7）
//! - parse 与 build 行为对称（都评估条件）
//! - 不写字节、不消耗流（哨兵错误）
//!
//! ## 条件来源
//!
//! 设计 §4.5.1：条件有三种编译期分类：
//! - [`StopIfCondition::Always`]：常量 true（永远停止，主要用于调试）
//! - [`StopIfCondition::Never`]：常量 false（永远不停止，主要用于调试）
//! - [`StopIfCondition::Expr`]：表达式（如 `x == 0`，编译为 `[GetInt(idx), Const(0), Eq]`）
//!
//! ## 在 Struct 中的捕获（设计 §4.7）
//!
//! StructNode.parse/build 在子字段返回 [`ConstructError::StopField`] 时停止后续字段，
//! 正常返回当前实例（已解析字段写入，未解析字段不写入）。这是 StructNode 在 Phase 4
//! 的小修改（新增分支，不破坏现有行为）。
//!
//! Python 等价：`Struct._parse` 用 `except StopFieldError` 捕获（core.py L2200 附近）。

use crate::context::Context;
use crate::error::ConstructError;
use crate::expr::{eval_expr_int, ExprProgram};
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

// ---------------------------------------------------------------------------
// StopIfCondition
// ---------------------------------------------------------------------------

/// StopIf 的条件来源（设计 §4.5.1）。
///
/// 编译期对条件分类：
/// - `Always` / `Never`：常量路径，零运行时求值开销
/// - `Expr`：表达式路径，运行时调 [`eval_expr_int`]（整数非零为真）
#[derive(Debug, Clone)]
pub enum StopIfCondition {
    /// 常量 true（永远停止，主要用于调试）。
    Always,
    /// 常量 false（永远不停止，主要用于调试）。
    Never,
    /// 表达式（如 `x == 0`，其中 `x` 是字段名引用，编译为 `[GetInt(idx), Const(0), Eq]`）。
    ///
    /// 求值结果非零为真（对齐 Python `evaluate(condfunc, context)` 的 truthy 语义，
    /// 但仅支持整数表达式——ExprProgram 的 VM 栈为 i64）。
    Expr(ExprProgram),
}

impl StopIfCondition {
    /// 是否为表达式条件（供 [`crate::nodes::stop_if::StopIfNode::has_expressions`] 使用）。
    pub fn is_expr(&self) -> bool {
        matches!(self, StopIfCondition::Expr(_))
    }
}

// ---------------------------------------------------------------------------
// StopIfNode
// ---------------------------------------------------------------------------

/// 早停条件节点。
///
/// 对应 Python construct `StopIf(condfunc)`（core.py L4079）。
///
/// # parse / build 行为
///
/// 对齐 Python `StopIf._parse` / `_build`（core.py L4103-4111）：
/// ```python
/// def _parse(self, stream, context, path):
///     condfunc = evaluate(self.condfunc, context)
///     if condfunc:
///         raise StopFieldError(path=path)
/// ```
///
/// - 条件为真 → 返回 [`ConstructError::StopField`]（哨兵，由外层捕获）
/// - 条件为假 → parse 返回 `Py_None`，build 返回 `Ok(())`
///
/// # sizeof 行为
///
/// 永远返回 `Err`（对齐 Python `StopIf._sizeof` L4113-4114，SI-7）。
#[derive(Debug, Clone)]
pub struct StopIfNode {
    /// 条件：常量或表达式。
    cond: StopIfCondition,
}

impl StopIfNode {
    /// 创建 StopIfNode。
    pub fn new(cond: StopIfCondition) -> Self {
        Self { cond }
    }

    /// 返回条件引用。
    pub fn cond(&self) -> &StopIfCondition {
        &self.cond
    }

    /// has_expressions 判断（设计 §6.1.1）。
    ///
    /// **关键**：`StopIf(Expr)` 中的表达式（如 `x == 0`）**确实引用 Struct 字段**
    /// （此例中的 `x`）。`has_expressions()` 必须返回 true，触发 StructNode 创建
    /// child context 并 init_expr_values（设计 §6.1.1 P1 关键说明）。
    ///
    /// `StopIf(Always)` / `StopIf(Never)` 不引用字段（编译期常量），返回 false。
    pub fn has_expressions(&self) -> bool {
        self.cond.is_expr()
    }

    /// 求值条件（设计 §4.5.2 + §16.1.3-O1 fast-path）。
    ///
    /// - `Always` → `true`
    /// - `Never` → `false`
    /// - `Expr(prog)` → 整数非零为真。先尝试 [`ExprProgram::try_eval_simple_cmp`]
    ///   fast-path（命中 3-op 单/双字段比较模式时内联求值，省 ~10ns/调用），
    ///   未命中走通用 [`eval_expr_int`]（5-op `(e&0xFF)==0` / 复合表达式等）。
    ///   fast-path 范式与 [`crate::nodes::repeat_until::RepeatUntilNode::eval_terminator`]
    ///   一致（4.5 v5.1 已验证）。
    ///
    /// Phase 7 重构：转发到 [`eval_condition`] 公共辅助函数（设计 §2.3），
    /// 与 [`crate::nodes::if_then_else::IfThenElseNode`] 共用同一份求值逻辑。
    fn eval_cond(&self, ctx: &Context<'_>, py: Python<'_>) -> Result<bool, ConstructError> {
        eval_condition(&self.cond, ctx, py)
    }
}

// ---------------------------------------------------------------------------
// 公共辅助：eval_condition（Phase 7 提取，IfThenElse / StopIf 共用）
// ---------------------------------------------------------------------------

/// 求值条件（StopIf / IfThenElse 共用，设计 §2.3）。
///
/// - [`StopIfCondition::Always`] → `true`
/// - [`StopIfCondition::Never`] → `false`
/// - [`StopIfCondition::Expr`] → fast-path `try_eval_simple_cmp` / 通用
///   [`eval_expr_int`]，非零为真（对齐 Python truthy 语义）。
///
/// # 性能（设计 §8.1）
///
/// - `Always` / `Never`：编译期常量分支，零运行时求值。
/// - `Expr`：先尝试 fast-path（3-op 比较模式内联求值，省 ~10ns/调用），
///   未命中走通用 VM（5-op 或复合表达式）。
///
/// # 错误传播
///
/// 求值失败（字段缺失、context 未初始化等）错误向上传播
/// （[`ConstructError::ExprFieldMissing`] / [`ConstructError::ExprContext`] 等）。
pub(crate) fn eval_condition(
    cond: &StopIfCondition,
    ctx: &Context<'_>,
    py: Python<'_>,
) -> Result<bool, ConstructError> {
    match cond {
        StopIfCondition::Always => Ok(true),
        StopIfCondition::Never => Ok(false),
        StopIfCondition::Expr(prog) => {
            // O1 fast-path（设计 §16.1.3-O1）：命中常见 3-op 比较模式时内联求值。
            let v = if let Some(fast) = prog.try_eval_simple_cmp(ctx, py) {
                fast?
            } else {
                eval_expr_int(prog, ctx, py)?
            };
            Ok(v != 0)
        }
    }
}

// ---------------------------------------------------------------------------
// Construct impl
// ---------------------------------------------------------------------------

impl super::Construct for StopIfNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        _stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let stop = self.eval_cond(ctx, py)?;
        if stop {
            // 抛出早停哨兵（被外层 Struct / GreedyRange 捕获，设计 §2.4 决策 A4）。
            Err(ConstructError::StopField {
                path: path.to_string(),
            })
        } else {
            // 条件为假：返回 Py_None（对齐 Python `_parse` 不显式 return 时返回 None）。
            Ok(py.None())
        }
    }

    fn build(
        &self,
        py: Python<'_>,
        _obj: &Bound<'_, PyAny>,
        _stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        let stop = self.eval_cond(ctx, py)?;
        if stop {
            Err(ConstructError::StopField {
                path: path.to_string(),
            })
        } else {
            Ok(())
        }
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // 对齐 Python `StopIf._sizeof` L4113-4114：永远 SizeofError（SI-7）。
        Err(ConstructError::Generic {
            message: "StopIf size is undefined".to_string(),
            path: String::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{ExprOp, ExprProgram};
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

    /// 构造一个简单的 `GetInt(idx) == const` 表达式程序。
    fn eq_expr(idx: usize, value: i64) -> ExprProgram {
        ExprProgram::new(vec![ExprOp::GetInt(idx), ExprOp::Const(value), ExprOp::Eq])
    }

    /// 构造 `GetInt(idx) > const` 表达式程序。
    fn gt_expr(idx: usize, value: i64) -> ExprProgram {
        ExprProgram::new(vec![ExprOp::GetInt(idx), ExprOp::Const(value), ExprOp::Gt])
    }

    // ======================================================================
    // 构造器与访问器
    // ======================================================================

    #[test]
    fn new_always_creates_node() {
        let node = StopIfNode::new(StopIfCondition::Always);
        assert!(matches!(node.cond(), StopIfCondition::Always));
    }

    #[test]
    fn new_never_creates_node() {
        let node = StopIfNode::new(StopIfCondition::Never);
        assert!(matches!(node.cond(), StopIfCondition::Never));
    }

    #[test]
    fn new_expr_creates_node() {
        let prog = eq_expr(0, 0);
        let node = StopIfNode::new(StopIfCondition::Expr(prog));
        assert!(matches!(node.cond(), StopIfCondition::Expr(_)));
    }

    #[test]
    fn is_expr_returns_correct_value() {
        assert!(!StopIfCondition::Always.is_expr());
        assert!(!StopIfCondition::Never.is_expr());
        assert!(StopIfCondition::Expr(eq_expr(0, 0)).is_expr());
    }

    #[test]
    fn has_expressions_depends_on_cond() {
        // Always / Never → false
        assert!(!StopIfNode::new(StopIfCondition::Always).has_expressions());
        assert!(!StopIfNode::new(StopIfCondition::Never).has_expressions());
        // Expr → true（关键：触发 StructNode init_expr_values）
        assert!(StopIfNode::new(StopIfCondition::Expr(eq_expr(0, 0))).has_expressions());
    }

    // ======================================================================
    // SI-6：常量 false（Never）不停止
    // ======================================================================

    #[test]
    fn parse_never_returns_none() {
        // SI-6: 条件为 false → 不停止，返回 Py_None
        with_py(|py| {
            let node = StopIfNode::new(StopIfCondition::Never);
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse should succeed");
            assert!(result.bind(py).is_none());
        });
    }

    #[test]
    fn build_never_succeeds() {
        with_py(|py| {
            let node = StopIfNode::new(StopIfCondition::Never);
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build should succeed");
            assert!(stream.as_bytes().is_empty());
        });
    }

    // ======================================================================
    // SI-1/SI-2：常量 true（Always）抛出 StopField
    // ======================================================================

    #[test]
    fn parse_always_returns_stop_field() {
        // SI-1 直接形式：Always → StopField 哨兵
        with_py(|py| {
            let node = StopIfNode::new(StopIfCondition::Always);
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should return StopField");
            match err {
                ConstructError::StopField { .. } => {}
                other => panic!("expected StopField, got {:?}", other),
            }
        });
    }

    #[test]
    fn build_always_returns_stop_field() {
        with_py(|py| {
            let node = StopIfNode::new(StopIfCondition::Always);
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should return StopField");
            match err {
                ConstructError::StopField { .. } => {}
                other => panic!("expected StopField, got {:?}", other),
            }
        });
    }

    #[test]
    fn parse_always_does_not_consume_stream() {
        // StopIf 不消耗字节（哨兵错误提前返回）
        with_py(|py| {
            let node = StopIfNode::new(StopIfCondition::Always);
            let mut stream = ParseStream::new(&[0x01, 0x02, 0x03]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let _ = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("StopField");
            assert_eq!(stream.tell(), 0, "StopIf should not consume stream");
        });
    }

    #[test]
    fn parse_always_does_not_write_stream() {
        with_py(|py| {
            let node = StopIfNode::new(StopIfCondition::Always);
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let _ = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("StopField");
            assert!(stream.as_bytes().is_empty());
        });
    }

    // ======================================================================
    // 表达式条件：Eq / Gt 等
    // ======================================================================

    #[test]
    fn parse_expr_cond_true_returns_stop_field() {
        // 表达式 x == 0，x = 0 → 条件为真 → StopField
        with_py(|py| {
            use pyo3::types::PyString;
            let node = StopIfNode::new(StopIfCondition::Expr(eq_expr(0, 0)));
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::new_root(py).expect("ctx");
            ctx.init_expr_values(1);
            let key = PyString::new_bound(py, "x").unbind();
            let val = 0i64.into_py(py);
            ctx.set_field_at(0, &key, val.bind(py), py).expect("set");
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should stop");
            match err {
                ConstructError::StopField { .. } => {}
                other => panic!("expected StopField, got {:?}", other),
            }
        });
    }

    #[test]
    fn parse_expr_cond_false_returns_none() {
        // 表达式 x == 0，x = 5 → 条件为假 → Py_None
        with_py(|py| {
            use pyo3::types::PyString;
            let node = StopIfNode::new(StopIfCondition::Expr(eq_expr(0, 0)));
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::new_root(py).expect("ctx");
            ctx.init_expr_values(1);
            let key = PyString::new_bound(py, "x").unbind();
            let val = 5i64.into_py(py);
            ctx.set_field_at(0, &key, val.bind(py), py).expect("set");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse ok");
            assert!(result.bind(py).is_none());
        });
    }

    #[test]
    fn build_expr_cond_true_returns_stop_field() {
        // build 方向同样评估条件
        with_py(|py| {
            use pyo3::types::PyString;
            let node = StopIfNode::new(StopIfCondition::Expr(eq_expr(0, 0)));
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            ctx.init_expr_values(1);
            let key = PyString::new_bound(py, "x").unbind();
            let val = 0i64.into_py(py);
            ctx.set_field_at(0, &key, val.bind(py), py).expect("set");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should stop");
            assert!(matches!(err, ConstructError::StopField { .. }));
        });
    }

    #[test]
    fn build_expr_cond_false_succeeds() {
        with_py(|py| {
            use pyo3::types::PyString;
            let node = StopIfNode::new(StopIfCondition::Expr(eq_expr(0, 0)));
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            ctx.init_expr_values(1);
            let key = PyString::new_bound(py, "x").unbind();
            let val = 99i64.into_py(py);
            ctx.set_field_at(0, &key, val.bind(py), py).expect("set");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build ok");
        });
    }

    #[test]
    fn parse_expr_gt_cond_works() {
        // x > 10：x=5 不停；x=20 停
        with_py(|py| {
            use pyo3::types::PyString;

            // x = 5：不停
            let node = StopIfNode::new(StopIfCondition::Expr(gt_expr(0, 10)));
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::new_root(py).expect("ctx");
            ctx.init_expr_values(1);
            let key = PyString::new_bound(py, "x").unbind();
            let val = 5i64.into_py(py);
            ctx.set_field_at(0, &key, val.bind(py), py).expect("set");
            let mut path = Path::new();
            let _ = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("5 > 10 is false, no stop");

            // x = 20：停
            let val2 = 20i64.into_py(py);
            ctx.set_field_at(0, &key, val2.bind(py), py).expect("set");
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("20 > 10 is true, should stop");
            assert!(matches!(err, ConstructError::StopField { .. }));
        });
    }

    // ======================================================================
    // 表达式求值错误传播
    // ======================================================================

    #[test]
    fn parse_expr_with_missing_field_returns_error() {
        // GetInt(0) 引用的槽位为 null → ExprFieldMissing（非 StopField）
        with_py(|py| {
            let node = StopIfNode::new(StopIfCondition::Expr(eq_expr(0, 0)));
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::new_root(py).expect("ctx");
            ctx.init_expr_values(1);
            // 不调用 set_field_at，槽位保持 null
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail with ExprFieldMissing");
            match err {
                ConstructError::ExprFieldMissing { .. } => {}
                other => panic!("expected ExprFieldMissing, got {:?}", other),
            }
        });
    }

    #[test]
    fn parse_expr_on_placeholder_returns_expr_context() {
        // placeholder ctx：expr_values 未初始化
        with_py(|py| {
            let node = StopIfNode::new(StopIfCondition::Expr(eq_expr(0, 0)));
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::ExprContext { .. } => {}
                other => panic!("expected ExprContext, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // SI-7：sizeof 永远 Err
    // ======================================================================

    #[test]
    fn sizeof_always_returns_error() {
        // SI-7: sizeof 永远 Err
        with_py(|py| {
            let ctx = Context::new_root(py).expect("ctx");
            let always = StopIfNode::new(StopIfCondition::Always);
            let never = StopIfNode::new(StopIfCondition::Never);
            let expr = StopIfNode::new(StopIfCondition::Expr(eq_expr(0, 0)));
            for node in [&always, &never, &expr] {
                let err = node.sizeof(&ctx).expect_err("sizeof should fail");
                match err {
                    ConstructError::Generic { message, .. } => {
                        assert!(message.contains("StopIf"), "got: {}", message);
                    }
                    other => panic!("expected Generic, got {:?}", other),
                }
            }
        });
    }

    #[test]
    fn sizeof_returns_error_on_placeholder() {
        with_py(|py| {
            let ctx = Context::placeholder(py);
            let node = StopIfNode::new(StopIfCondition::Always);
            assert!(node.sizeof(&ctx).is_err());
        });
    }

    // ======================================================================
    // StopField 携带 path（错误追踪）
    // ======================================================================

    #[test]
    fn stop_field_carries_path_string() {
        // StopField 错误携带 path 字段，便于追踪
        with_py(|py| {
            let node = StopIfNode::new(StopIfCondition::Always);
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            path.push_field("stop_field");
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("StopField");
            match err {
                ConstructError::StopField { path: p } => {
                    assert!(
                        p.contains("stop_field"),
                        "path should contain field name: {}",
                        p
                    );
                }
                other => panic!("expected StopField, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // parse/build 对称性
    // ======================================================================

    #[test]
    fn parse_and_build_symmetry_for_always() {
        // 两个方向都抛 StopField
        with_py(|py| {
            let node = StopIfNode::new(StopIfCondition::Always);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            let mut stream = ParseStream::new(b"");
            let perr = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("parse StopField");
            assert!(matches!(perr, ConstructError::StopField { .. }));

            let obj = py.eval_bound("None", None, None).expect("None");
            let mut bstream = BuildStream::new();
            let berr = node
                .build(py, &obj, &mut bstream, &mut ctx, &mut path)
                .expect_err("build StopField");
            assert!(matches!(berr, ConstructError::StopField { .. }));
        });
    }

    #[test]
    fn parse_and_build_symmetry_for_never() {
        // 两个方向都成功
        with_py(|py| {
            let node = StopIfNode::new(StopIfCondition::Never);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            let mut stream = ParseStream::new(b"");
            let _ = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse ok");

            let obj = py.eval_bound("None", None, None).expect("None");
            let mut bstream = BuildStream::new();
            node.build(py, &obj, &mut bstream, &mut ctx, &mut path)
                .expect("build ok");
        });
    }
}
