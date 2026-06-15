//! Compiled expression — pre-processed expression for runtime evaluation.
//!
//! This module defines [`CompiledExpr`], a compiled representation of a
//! [`CombinedExpr`](crate::expr::CombinedExpr) that is produced once during
//! schema compilation and reused on every `parse` / `build` call.
//!
//! # Phase 12 scope
//!
//! In Phase 12 (pure Rust path), all expressions are native Rust expression
//! trees. [`compile_expr`] applies **constant folding** to
//! [`ConstExpr`](crate::expr::ConstExpr) and otherwise wraps the cloned
//! expression tree. There is **no** `PyCallback` variant (B1 correction):
//! Python callables are not reachable in the pure Rust path and will be
//! handled by Phase 13 inside the `construct-py` crate.
//!
//! # Python correspondence
//!
//! This mirrors pydantic-core's distinction between the schema-declaration
//! representation and the compiled validator representation — but for
//! expressions. The Phase 11 `CombinedExpr` already eliminates virtual
//! dispatch via `enum_dispatch`, so the primary benefit of `CompiledExpr`
//! in Phase 12 is **embedding**: a [`CompiledNode`](crate::compiled::CompiledNode)
//! variant (e.g. `CompiledArrayExpr`, `CompiledSwitch`) holds a
//! `CompiledExpr` field directly, avoiding re-evaluation-tree construction
//! on every parse.

use crate::core::context::Context;
use crate::core::error::Result;
use crate::expr::{CombinedExpr, Evaluate};
use crate::value::Value;

// ===========================================================================
// CompiledExpr
// ===========================================================================

/// A compiled expression that can be evaluated at runtime.
///
/// Produced once by [`compile_expr`] from a declaration-tree
/// [`CombinedExpr`](crate::expr::CombinedExpr). Variants:
///
/// - [`CompiledExpr::Native`] — wraps a cloned `CombinedExpr`. Evaluation
///   delegates to [`Evaluate::evaluate`] via `enum_dispatch`. This is the
///   common case in Phase 12: paths, binary ops, unary ops, function calls,
///   ternaries, and list-indexing all compile to `Native`.
/// - [`CompiledExpr::Constant`] — a compile-time constant folded out of
///   [`ConstExpr`](crate::expr::ConstExpr). Avoids tree traversal for
///   literal expressions.
///
/// # Cloning
///
/// `CompiledExpr` derives `Clone`. This is required because
/// `CombinedConstruct::compile` (Phase 12.4+) takes `&self`, so any
/// `CompiledNode` variant holding a `CompiledExpr` (e.g. `CompiledSwitch`)
/// must clone the expression rather than move it. `CombinedExpr` is `Clone`
/// thanks to the Phase 12.2 prerequisite change (§6.4 of the design doc),
/// and [`Value`] is already `Clone`.
///
/// # Example
///
/// ```
/// # use construct::compiled::expr::{CompiledExpr, compile_expr};
/// # use construct::expr::{this_, ConstExpr, BinExpr, BinOp, Evaluate};
/// # use construct::core::context::Context;
/// # use construct::value::Value;
/// // Build a declaration expression: this.count + 1
/// let decl = BinExpr::new(
///     BinOp::Add,
///     this_().field("count"),
///     ConstExpr::new(Value::Int(1)),
/// );
///
/// // Compile it once.
/// let combined = construct::expr::CombinedExpr::BinExpr(decl);
/// let compiled = compile_expr(&combined).unwrap();
///
/// // Evaluate it many times against different contexts.
/// let mut ctx = Context::new();
/// ctx.insert("count", Value::Int(41));
/// assert_eq!(compiled.eval(&ctx, None).unwrap(), Value::Int(42));
/// ```
#[derive(Clone, Debug)]
pub enum CompiledExpr {
    /// A native Rust expression tree (Phase 11's [`CombinedExpr`], cloned
    /// into the compiled node). Evaluation delegates to
    /// [`Evaluate::evaluate`].
    Native(CombinedExpr),

    /// A compile-time constant folded from
    /// [`ConstExpr`](crate::expr::ConstExpr). Avoids tree traversal for
    /// literal expressions.
    Constant(Value),
}

impl CompiledExpr {
    /// Evaluates the compiled expression against a context and an optional
    /// build object.
    ///
    /// - For [`CompiledExpr::Native`], this delegates to
    ///   [`CombinedExpr::evaluate`] via `enum_dispatch` dispatch.
    /// - For [`CompiledExpr::Constant`], this returns a clone of the stored
    ///   constant without touching `ctx` or `obj`.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError`](crate::core::error::ConstructError) on
    /// evaluation failure (path resolution, type mismatch, unknown
    /// function, etc.). The error carries the `path` of the expression
    /// evaluation site, prefixed by the caller as appropriate.
    ///
    /// [`CombinedExpr::evaluate`]: crate::expr::CombinedExpr::evaluate
    //
    // Note: no `#[must_use]` here — `Result<Value>` is already `#[must_use]`,
    // so an explicit attribute would trip `clippy::double_must_use`.
    pub fn eval(&self, ctx: &Context, obj: Option<&Value>) -> Result<Value> {
        match self {
            CompiledExpr::Native(e) => e.evaluate(ctx, obj),
            CompiledExpr::Constant(v) => Ok(v.clone()),
        }
    }

    /// Returns `true` if this compiled expression is a compile-time constant.
    ///
    /// Useful for callers that can short-circuit evaluation when the
    /// expression is known at compile time (e.g. a `Switch` whose key
    /// expression folds to a constant).
    #[must_use]
    pub fn is_constant(&self) -> bool {
        matches!(self, CompiledExpr::Constant(_))
    }

    /// Returns a reference to the constant [`Value`] if this expression is
    /// a [`CompiledExpr::Constant`]; otherwise `None`.
    #[must_use]
    pub fn as_constant(&self) -> Option<&Value> {
        match self {
            CompiledExpr::Constant(v) => Some(v),
            _ => None,
        }
    }
}

// ===========================================================================
// compile_expr
// ===========================================================================

/// Compiles a declaration-tree expression into a [`CompiledExpr`].
///
/// This is the entry point of the Phase 12 expression compilation pipeline.
/// It applies **constant folding** — when `expr` is a
/// [`ConstExpr`](crate::expr::ConstExpr), the literal [`Value`] is extracted
/// directly into [`CompiledExpr::Constant`], avoiding runtime tree
/// traversal. All other expression variants are wrapped in
/// [`CompiledExpr::Native`] as a cloned [`CombinedExpr`].
///
/// # Phase 12 simplification
///
/// Because the pure Rust path never encounters Python callables,
/// `compile_expr` performs no `PyCallback` classification. The two branches
/// are exhaustive for Phase 12:
///
/// | Input (`CombinedExpr` variant) | Output |
/// |-------------------------------|--------|
/// | `ConstExpr(c)`                | `CompiledExpr::Constant(c.value.clone())` |
/// | `Path` / `BinExpr` / `UniExpr` / `FuncPath` / `CondExpr` / `ListPath` | `CompiledExpr::Native(expr.clone())` |
///
/// Phase 13 will extend this function (in the `construct-py` crate) to detect
/// Python callables and emit a Phase 13 `PyCallback` variant, or fall back
/// to `CompiledDynamic` (Phase 12 path) for unknown callable sources.
///
/// # Errors
///
/// In Phase 12, this function never returns `Err` — both branches are
/// infallible. The `Result` return type is preserved for Phase 13 forward
/// compatibility (callable detection may fail on invalid Python objects).
///
/// # Example
///
/// ```
/// # use construct::compiled::expr::{compile_expr, CompiledExpr};
/// # use construct::expr::ConstExpr;
/// # use construct::value::Value;
/// let decl = ConstExpr::new(Value::Int(42));
/// let combined = construct::expr::CombinedExpr::ConstExpr(decl);
/// let compiled = compile_expr(&combined).unwrap();
/// assert!(matches!(compiled, CompiledExpr::Constant(Value::Int(42))));
/// ```
pub fn compile_expr(expr: &CombinedExpr) -> Result<CompiledExpr> {
    // Constant folding: ConstExpr → Constant.
    if let CombinedExpr::ConstExpr(c) = expr {
        return Ok(CompiledExpr::Constant(c.value.clone()));
    }

    // Phase 12: all remaining variants are Native (no Python callables in
    // the pure Rust path). Phase 13: detect PyClosure inside CombinedExpr →
    // compile to CompiledDynamic fallback (Phase 12) or PyCallback (Phase 13,
    // defined in construct-py).
    //
    // B2 correction: `clone_ref()` does not exist; `expr.clone()` works
    // because CombinedExpr + all 5 inner variant types now derive Clone
    // (Phase 12.2 prerequisite change — see §6.4 of the design doc).
    Ok(CompiledExpr::Native(expr.clone()))
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{
        this_, BinExpr, BinOp, CondExpr, ConstExpr, FuncPath, ListPath, UniExpr, UniOp,
    };

    // -- compile_expr: constant folding -------------------------------------

    #[test]
    fn compile_const_expr_folds_to_constant() {
        let expr = CombinedExpr::ConstExpr(ConstExpr::new(Value::Int(42)));
        let compiled = compile_expr(&expr).expect("compile should succeed");
        assert!(compiled.is_constant());
        assert_eq!(compiled.as_constant(), Some(&Value::Int(42)));
    }

    #[test]
    fn compile_const_expr_preserves_value_type() {
        // Verify that constant folding preserves the exact Value variant.
        let cases = vec![
            Value::Int(-7),
            Value::UInt(7),
            Value::Bool(true),
            Value::Float(2.5),
            Value::String("hello".to_string()),
            Value::Bytes(b"\x01\x02".to_vec()),
            Value::None,
        ];
        for v in cases {
            let expr = CombinedExpr::ConstExpr(ConstExpr::new(v.clone()));
            let compiled = compile_expr(&expr).expect("compile should succeed");
            assert_eq!(compiled.as_constant(), Some(&v));
        }
    }

    // -- compile_expr: native wrapping --------------------------------------

    #[test]
    fn compile_path_expr_wraps_as_native() {
        let expr = CombinedExpr::Path(this_().field("count"));
        let compiled = compile_expr(&expr).expect("compile should succeed");
        assert!(!compiled.is_constant());
        assert!(matches!(compiled, CompiledExpr::Native(_)));
    }

    #[test]
    fn compile_bin_expr_wraps_as_native() {
        let expr = CombinedExpr::BinExpr(BinExpr::new(
            BinOp::Add,
            this_().field("count"),
            ConstExpr::new(Value::Int(1)),
        ));
        let compiled = compile_expr(&expr).expect("compile should succeed");
        assert!(matches!(compiled, CompiledExpr::Native(_)));
    }

    #[test]
    fn compile_uni_expr_wraps_as_native() {
        let expr = CombinedExpr::UniExpr(UniExpr::new(UniOp::Neg, ConstExpr::new(Value::Int(7))));
        let compiled = compile_expr(&expr).expect("compile should succeed");
        assert!(matches!(compiled, CompiledExpr::Native(_)));
    }

    #[test]
    fn compile_func_path_wraps_as_native() {
        let expr = CombinedExpr::FuncPath(FuncPath::new("abs", ConstExpr::new(Value::Int(-3))));
        let compiled = compile_expr(&expr).expect("compile should succeed");
        assert!(matches!(compiled, CompiledExpr::Native(_)));
    }

    #[test]
    fn compile_cond_expr_wraps_as_native() {
        let expr = CombinedExpr::CondExpr(CondExpr::new(
            ConstExpr::new(Value::Bool(true)),
            ConstExpr::new(Value::Int(1)),
            ConstExpr::new(Value::Int(0)),
        ));
        let compiled = compile_expr(&expr).expect("compile should succeed");
        assert!(matches!(compiled, CompiledExpr::Native(_)));
    }

    #[test]
    fn compile_list_path_wraps_as_native() {
        let inner = Value::List(vec![Value::Int(10), Value::Int(20), Value::Int(30)]);
        let expr = CombinedExpr::ListPath(ListPath::new_indexed(-1, ConstExpr::new(inner)));
        let compiled = compile_expr(&expr).expect("compile should succeed");
        assert!(matches!(compiled, CompiledExpr::Native(_)));
    }

    // -- eval: correctness --------------------------------------------------

    #[test]
    fn eval_constant_returns_stored_value() {
        let compiled = CompiledExpr::Constant(Value::String("hi".to_string()));
        let ctx = Context::new();
        let result = compiled.eval(&ctx, None).expect("eval should succeed");
        assert_eq!(result, Value::String("hi".to_string()));
    }

    #[test]
    fn eval_native_path_reads_context() {
        let mut ctx = Context::new();
        ctx.insert("count", Value::Int(41));
        let compiled = compile_expr(&CombinedExpr::Path(this_().field("count"))).unwrap();
        let result = compiled.eval(&ctx, None).expect("eval should succeed");
        assert_eq!(result, Value::Int(41));
    }

    #[test]
    fn eval_native_bin_expr_computes() {
        let mut ctx = Context::new();
        ctx.insert("count", Value::Int(41));
        let decl = BinExpr::new(
            BinOp::Add,
            this_().field("count"),
            ConstExpr::new(Value::Int(1)),
        );
        let compiled = compile_expr(&CombinedExpr::BinExpr(decl)).unwrap();
        let result = compiled.eval(&ctx, None).expect("eval should succeed");
        assert_eq!(result, Value::Int(42));
    }

    #[test]
    fn eval_constant_ignores_context() {
        let mut ctx = Context::new();
        ctx.insert("x", Value::Int(999));
        let compiled = CompiledExpr::Constant(Value::Int(1));
        let result = compiled.eval(&ctx, None).expect("eval should succeed");
        assert_eq!(result, Value::Int(1));
    }

    // -- Clone behavior -----------------------------------------------------

    #[test]
    fn compiled_expr_native_is_cloneable() {
        // Regression test for B2 correction: CompiledExpr must be Clone
        // because CompiledNode variants (CompiledSwitch etc.) hold it as a
        // field. Cloning must not panic or fail.
        let decl = BinExpr::new(
            BinOp::Mul,
            this_().field("n"),
            ConstExpr::new(Value::Int(2)),
        );
        let compiled = compile_expr(&CombinedExpr::BinExpr(decl)).unwrap();
        let cloned = compiled.clone();
        let mut ctx = Context::new();
        ctx.insert("n", Value::Int(21));
        assert_eq!(cloned.eval(&ctx, None).unwrap(), Value::Int(42));
    }

    #[test]
    fn compiled_expr_constant_is_cloneable() {
        let compiled = CompiledExpr::Constant(Value::Int(42));
        let cloned = compiled.clone();
        assert_eq!(cloned.as_constant(), Some(&Value::Int(42)));
    }

    #[test]
    fn deep_nested_expr_clone_preserves_semantics() {
        // (this.a + this.b) * 2 - regression for the recursive Clone path
        // (Box<CombinedExpr> inside BinExpr).
        let lhs = BinExpr::new(BinOp::Add, this_().field("a"), this_().field("b"));
        let outer = BinExpr::new(BinOp::Mul, lhs, ConstExpr::new(Value::Int(2)));
        let compiled = compile_expr(&CombinedExpr::BinExpr(outer)).unwrap();
        let cloned = compiled.clone();

        let mut ctx = Context::new();
        ctx.insert("a", Value::Int(3));
        ctx.insert("b", Value::Int(4));
        assert_eq!(cloned.eval(&ctx, None).unwrap(), Value::Int(14));
    }

    // -- CombinedExpr Clone regression (B2 prerequisite) --------------------

    #[test]
    fn combined_expr_all_variants_are_clone() {
        // Smoke test: every CombinedExpr variant must be Clone-able after
        // the B2 prerequisite change. If any of these fail to compile, the
        // corresponding struct is missing #[derive(Clone)].
        let cases: Vec<CombinedExpr> = vec![
            CombinedExpr::Path(this_().field("x")),
            CombinedExpr::ConstExpr(ConstExpr::new(Value::Int(1))),
            CombinedExpr::BinExpr(BinExpr::new(
                BinOp::Add,
                this_().field("a"),
                ConstExpr::new(Value::Int(2)),
            )),
            CombinedExpr::UniExpr(UniExpr::new(UniOp::Neg, ConstExpr::new(Value::Int(3)))),
            CombinedExpr::FuncPath(FuncPath::new("abs", ConstExpr::new(Value::Int(4)))),
            CombinedExpr::CondExpr(CondExpr::new(
                ConstExpr::new(Value::Bool(true)),
                ConstExpr::new(Value::Int(5)),
                ConstExpr::new(Value::Int(6)),
            )),
            CombinedExpr::ListPath(ListPath::new_indexed(
                -1,
                ConstExpr::new(Value::List(vec![Value::Int(7)])),
            )),
        ];

        for expr in &cases {
            let _cloned = expr.clone();
        }
    }

    // -- compile_expr never errors in Phase 12 ------------------------------

    #[test]
    fn compile_expr_never_errors_for_native_expressions() {
        // Phase 12 invariant: compile_expr returns Ok for every CombinedExpr
        // reachable in the pure Rust path.
        let cases: Vec<CombinedExpr> = vec![
            CombinedExpr::Path(this_().field("x")),
            CombinedExpr::BinExpr(BinExpr::new(
                BinOp::Sub,
                this_().field("a"),
                this_().field("b"),
            )),
            CombinedExpr::UniExpr(UniExpr::new(UniOp::Not, ConstExpr::new(Value::Bool(false)))),
            CombinedExpr::CondExpr(CondExpr::new(
                ConstExpr::new(Value::Bool(false)),
                ConstExpr::new(Value::Int(1)),
                ConstExpr::new(Value::Int(0)),
            )),
        ];

        for expr in &cases {
            assert!(compile_expr(expr).is_ok(), "compile_expr returned Err");
        }
    }
}
