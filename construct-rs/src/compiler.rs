//! Schema compiler — converts a declaration tree into a compiled execution tree.
//!
//! [`SchemaCompiler`] is the entry point of the compilation pipeline. It takes
//! a [`CombinedConstruct`] declaration tree and produces a
//! [`CompiledSchema`], which is the runtime entry point for parse/build.
//!
//! This is the construct equivalent of pydantic-core's `build_validator`
//! (`validators/mod.rs:520`). Compilation is a **pure, stateless** function:
//! the same input always yields the same output. `SchemaCompiler` therefore
//! holds no state and can be freely constructed (or used via its associated
//! function).
//!
//! # Compilation flow
//!
//! 1. Traverse the declaration tree (depth-first, driven by each node's
//!    [`BuildConstruct::compile`] implementation).
//! 2. For each node, extract compile-time parameters and produce the matching
//!    [`CompiledNode`] variant.
//! 3. Wrap the root [`CompiledNode`] in a [`CompiledSchema`].
//!
//! # Phase 12.3 scope
//!
//! In Phase 12.3, only the `Dynamic` escape-hatch compiles to a *functional*
//! node ([`CompiledDynamic`](crate::compiled::CompiledDynamic)). All other
//! variants' [`BuildConstruct::compile`] implementations are stubs that return
//! `Err`. Real implementations are filled in across sub-tasks 12.4-12.9.
//!
//! # Caching
//!
//! [`SchemaCompiler`] itself does **not** cache. Compilation results may be
//! cached externally (e.g. on a PyO3 wrapper object or via a `OnceLock`) —
//! see design doc §3.3 for the `CompiledCache` pattern.

use crate::combined::CombinedConstruct;
use crate::compiled::{
    compile_expr as free_compile_expr, BuildConstruct, CompiledExpr, CompiledSchema,
};
use crate::core::error::Result;
use crate::expr::CombinedExpr;

// ===========================================================================
// SchemaCompiler
// ===========================================================================

/// Compiles a [`CombinedConstruct`] declaration tree into a [`CompiledSchema`].
///
/// `SchemaCompiler` is a unit struct: compilation is stateless, so a compiler
/// carries no per-instance data. Construct one with [`SchemaCompiler::new`]
/// (or [`SchemaCompiler::default`]) and call [`SchemaCompiler::compile`].
///
/// # Example
///
/// ```
/// # use construct::combined::dynamic;
/// # use construct::compiler::SchemaCompiler;
/// # use construct::constructs::meta::Pass;
/// // A Dynamic-wrapped Pass compiles to a functional escape-hatch node.
/// let schema = dynamic(Pass::new());
/// let compiler = SchemaCompiler::new();
/// let compiled = compiler.compile(&schema).unwrap();
/// ```
///
/// # Errors
///
/// [`SchemaCompiler::compile`] returns [`construct::core::error::ConstructError`]
/// when a declaration node cannot be compiled. In Phase 12.3 this is the case
/// for every variant except `Dynamic` (whose compile impls are still stubs);
/// real implementations arrive in sub-tasks 12.4-12.9.
pub struct SchemaCompiler;

impl SchemaCompiler {
    /// Creates a new compiler.
    ///
    /// Compilation is stateless, so the returned value carries no data — it
    /// exists primarily for API symmetry and forward compatibility (future
    /// phases may attach configuration such as a maximum recursion depth).
    #[must_use]
    pub const fn new() -> Self {
        SchemaCompiler
    }

    /// Compiles a declaration tree into a [`CompiledSchema`].
    ///
    /// This delegates to [`BuildConstruct::compile`](crate::compiled::BuildConstruct)
    /// on the root node (dispatched through the `CombinedConstruct` match),
    /// then wraps the resulting [`crate::compiled::CompiledNode`] in a
    /// [`CompiledSchema`].
    ///
    /// # Errors
    ///
    /// Propagates any error from the underlying `compile` call. In Phase
    /// 12.3, only the `Dynamic` escape-hatch variant compiles successfully;
    /// all other variants return a stub error.
    pub fn compile(&self, construct: &CombinedConstruct) -> Result<CompiledSchema> {
        let tree = construct.compile()?;
        Ok(CompiledSchema::new(tree))
    }

    /// Compiles a declaration-tree expression into a [`CompiledExpr`].
    ///
    /// This is a thin delegation to the free function
    /// [`compile_expr`](crate::compiled::compile_expr) (Phase 12.2). It is
    /// provided on [`SchemaCompiler`] for API discoverability: callers that
    /// already hold a compiler can compile both nodes and expressions through
    /// the same object.
    ///
    /// # Errors
    ///
    /// In Phase 12 this never returns `Err`; the `Result` is preserved for
    /// Phase 13 forward compatibility (callable detection).
    pub fn compile_expr(&self, expr: &CombinedExpr) -> Result<CompiledExpr> {
        free_compile_expr(expr)
    }
}

impl Default for SchemaCompiler {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combined::dynamic;
    use crate::compiled::CompiledNode;
    use crate::constructs::format_field::INT8UB;
    use crate::constructs::meta::Pass;
    use crate::constructs::struct_::Struct;
    use crate::core::context::Context;
    use crate::core::error::ConstructError;
    use crate::core::Construct;
    use crate::expr::{this_, ConstExpr};
    use crate::value::Value;

    // -- Construction / Default --------------------------------------------

    #[test]
    fn schema_compiler_new_is_const_constructible() {
        const _C: SchemaCompiler = SchemaCompiler::new();
    }

    #[test]
    fn schema_compiler_default_matches_new() {
        let _ = SchemaCompiler::default();
    }

    // -- compile: leaf variants succeed (Phase 12.4) ------------------------

    #[test]
    fn compile_pass_produces_compiled_schema() {
        // Pass is a leaf: compile succeeds and yields a CompiledSchema.
        let compiler = SchemaCompiler::new();
        let cc: CombinedConstruct = Pass::new().into();
        let schema = compiler.compile(&cc).expect("Pass should compile");
        assert!(matches!(schema.tree(), CompiledNode::Pass(_)));
        // Pass has a static size of 0.
        assert_eq!(schema.static_size(), Some(0));
    }

    #[test]
    fn compile_struct_returns_stub_error() {
        // Struct (composite) is still a stub in Phase 12.4.
        let compiler = SchemaCompiler::new();
        let cc: CombinedConstruct = Struct::new().into();
        assert!(compiler.compile(&cc).is_err());
    }

    #[test]
    fn compile_format_field_produces_compiled_schema() {
        let compiler = SchemaCompiler::new();
        let cc: CombinedConstruct = INT8UB.into();
        let schema = compiler.compile(&cc).expect("FormatField should compile");
        assert!(matches!(schema.tree(), CompiledNode::FormatField(_)));
        // INT8UB has a static size of 1 byte.
        assert_eq!(schema.static_size(), Some(1));
    }

    #[test]
    fn compile_stub_error_message_mentions_not_yet_implemented() {
        // Struct (composite) is still a stub in Phase 12.4.
        let compiler = SchemaCompiler::new();
        let cc: CombinedConstruct = Struct::new().into();
        let err = compiler.compile(&cc).unwrap_err();
        match err {
            ConstructError::Generic { message, .. } => {
                assert!(message.contains("not yet implemented"));
            }
            _ => unreachable!(),
        }
    }

    // -- compile: Dynamic variant succeeds (I3 escape-hatch) ----------------

    #[test]
    fn compile_dynamic_produces_compiled_schema() {
        // The headline test for Phase 12.3: compiling a Dynamic-wrapped
        // construct yields a usable CompiledSchema (not an error).
        let compiler = SchemaCompiler::new();
        let cc = dynamic(Pass::new());
        let schema = compiler.compile(&cc).expect("Dynamic should compile");
        assert!(schema.static_size().is_none());
        assert!(schema.name().is_none());
    }

    #[test]
    fn compile_dynamic_tree_is_compiled_dynamic_node() {
        let compiler = SchemaCompiler::new();
        let cc = dynamic(Pass::new());
        let schema = compiler.compile(&cc).unwrap();
        assert!(matches!(schema.tree(), CompiledNode::Dynamic(_)));
    }

    #[test]
    fn compile_dynamic_then_sizeof_works() {
        // End-to-end: compile a Dynamic-wrapped Pass, then sizeof via the
        // compiled schema. Pass has size 0.
        let compiler = SchemaCompiler::new();
        let cc = dynamic(Pass::new());
        let schema = compiler.compile(&cc).unwrap();
        let ctx = Context::new();
        assert_eq!(schema.sizeof(&ctx).unwrap(), 0);
    }

    #[test]
    fn compile_dynamic_then_parse_bytes_works() {
        // End-to-end parse: Dynamic escape-hatch delegates to Pass, which
        // parses nothing and yields Value::None.
        let compiler = SchemaCompiler::new();
        let cc = dynamic(Pass::new());
        let schema = compiler.compile(&cc).unwrap();
        let value = schema.parse_bytes(b"").unwrap();
        assert_eq!(value, Value::None);
    }

    #[test]
    fn compile_dynamic_then_build_bytes_works() {
        // End-to-end build: Dynamic escape-hatch delegates to Pass, which
        // writes nothing.
        let compiler = SchemaCompiler::new();
        let cc = dynamic(Pass::new());
        let schema = compiler.compile(&cc).unwrap();
        let bytes = schema.build_bytes(&Value::None).unwrap();
        assert!(bytes.is_empty());
    }

    #[test]
    fn compile_dynamic_roundtrip_preserves_semantics() {
        // The escape-hatch should behave identically to the declaration-tree
        // old path. Compare sizeof through CompiledSchema vs direct Construct.
        let compiler = SchemaCompiler::new();
        let cc = dynamic(Pass::new());
        let schema = compiler.compile(&cc).unwrap();

        let ctx = Context::new();
        let compiled_size = schema.sizeof(&ctx).unwrap();

        // Old path: call sizeof on the CombinedConstruct directly.
        let old_size = cc.sizeof(&ctx).unwrap();
        assert_eq!(compiled_size, old_size);
    }

    // -- compile_expr delegation --------------------------------------------

    #[test]
    fn compile_expr_folds_constant() {
        let compiler = SchemaCompiler::new();
        let expr = CombinedExpr::ConstExpr(ConstExpr::new(Value::Int(42)));
        let compiled = compiler.compile_expr(&expr).unwrap();
        assert!(compiled.is_constant());
        assert_eq!(compiled.as_constant(), Some(&Value::Int(42)));
    }

    #[test]
    fn compile_expr_wraps_native() {
        let compiler = SchemaCompiler::new();
        let expr = CombinedExpr::Path(this_().field("count"));
        let compiled = compiler.compile_expr(&expr).unwrap();
        assert!(matches!(compiled, CompiledExpr::Native(_)));
    }

    // -- Idempotency --------------------------------------------------------

    #[test]
    fn compile_dynamic_twice_yields_independent_schemas() {
        // Compilation is pure; calling it twice on equivalent inputs produces
        // equivalent (but distinct Arc-backed) schemas.
        let compiler = SchemaCompiler::new();
        let cc = dynamic(Pass::new());
        let schema_a = compiler.compile(&cc).unwrap();
        let schema_b = compiler.compile(&cc).unwrap();

        let ctx = Context::new();
        assert_eq!(
            schema_a.sizeof(&ctx).unwrap(),
            schema_b.sizeof(&ctx).unwrap()
        );
        // Distinct Arc allocations (not pointer-identical).
        assert!(!std::ptr::eq(
            schema_a.tree() as *const _,
            schema_b.tree() as *const _,
        ));
    }

    // -- with_name on compiled schema --------------------------------------

    #[test]
    fn compile_result_can_be_named_via_builder() {
        let compiler = SchemaCompiler::new();
        let cc = dynamic(Pass::new());
        let schema = compiler.compile(&cc).unwrap().with_name("my_schema");
        assert_eq!(schema.name(), Some("my_schema"));
    }
}
