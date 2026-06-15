//! BuildConstruct trait — declaration-to-compiled-node compilation.
//!
//! This module defines the [`BuildConstruct`] trait that each
//! [`CombinedConstruct`](crate::combined::CombinedConstruct) variant
//! implements to provide "declaration parameters → compiled node"
//! conversion.
//!
//! # Phase 12.1 scope
//!
//! The trait and enum_dispatch framework are defined here. All
//! implementations are **stubs** that return `Err` — real implementations
//! are filled in across sub-tasks 12.4-12.9.

use crate::combined::CombinedConstruct;
use crate::compiled::{
    CompiledBitsInteger, CompiledBytes, CompiledBytesExpr, CompiledBytesInteger, CompiledCString,
    CompiledError, CompiledFlag, CompiledFormatField, CompiledGreedyBytes, CompiledNode,
    CompiledPaddedString, CompiledPass, CompiledSeek, CompiledSeekExpr, CompiledTell,
    CompiledTerminated, CompiledVarInt, CompiledZigZag,
};
use crate::constructs::{
    adapters::{Adapter, ExprAdapter, ExprValidator, SymmetricAdapter, Validator},
    bytes::{Bytes, BytesExpr, GreedyBytes},
    bytes_integer::{BitsInteger, BytesInteger},
    computed::{
        Aligned, Computed, Default, FixedSized, Index, NamedTuple, Padded, Rebuild,
        TimestampAdapter,
    },
    const_::Const,
    control_flow::{Check, IfThenElse, StopIf, Switch},
    enum_::{Enum, FlagsEnum, Mapping},
    flag::Flag,
    focused_seq::FocusedSeq,
    format_field::FormatField,
    hex::{Hex, HexDump},
    lazy::{Lazy, LazyArray, LazyStruct, Rebuffered},
    meta::{Error, Pass, Seek, SeekExpr, Tell, Terminated},
    repetition::{Array, ArrayExpr, GreedyRange, RepeatUntil},
    select::Select,
    sequence::Sequence,
    stream_ops::{
        BitsSwapped, Bitwise, ByteSwapped, Bytewise, Checksum, LazyBound, Peek, Pointer,
        PointerExpr, Prefixed, RawCopy, Restreamed, Transformed,
    },
    strings::{CString, PaddedString},
    struct_::Struct,
    union::Union,
    varint::{VarInt, ZigZag},
};
use crate::core::error::{ConstructError, Result};
use crate::core::{Renamed, Subconstruct};

#[cfg(feature = "compression")]
use crate::constructs::stream_ops::Compressed;

// ===========================================================================
// BuildConstruct trait
// ===========================================================================

/// Trait implemented by declaration-tree construct types that can be
/// compiled into [`CompiledNode`] execution-tree nodes.
///
/// Each construct type knows how to extract its compile-time parameters
/// and produce an optimized [`CompiledNode`]. In Phase 12.1, all
/// implementations are stubs; real logic is added in sub-tasks 12.4-12.9.
///
/// # Future signature change (Phase 12.4)
///
/// In Phase 12.4, this trait's `compile` method will gain a `compiler`
/// parameter to support recursive compilation of child nodes:
///
/// ```ignore
/// fn compile(&self, compiler: &SchemaCompiler) -> Result<CompiledNode>;
/// ```
///
/// Phase 12.1 uses the simpler `&self`-only signature since all
/// implementations are stubs.
pub trait BuildConstruct {
    /// Compiles this declaration node into a [`CompiledNode`].
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError`] on compilation failure (e.g.
    /// unsupported construct type in current phase, cycle detection,
    /// invalid parameters).
    fn compile(&self) -> Result<CompiledNode>;
}

// ===========================================================================
// Stub implementations
// ===========================================================================
//
// All construct types implement BuildConstruct with stub implementations
// that return a descriptive error. Real implementations are added in
// sub-tasks 12.4-12.9.

/// Error message for stub BuildConstruct implementations.
const STUB_COMPILE_MESSAGE: &str =
    "compile not yet implemented (Phase 12.1 stub — real impl in 12.4-12.9)";

/// Builds a stub error for BuildConstruct::compile.
fn stub_compile_error() -> ConstructError {
    ConstructError::Generic {
        path: String::new(),
        message: STUB_COMPILE_MESSAGE.to_string(),
    }
}

/// Macro to generate stub BuildConstruct impls for all construct types.
macro_rules! impl_build_construct_stub {
    ($($ty:ty),* $(,)?) => {
        $(
            impl BuildConstruct for $ty {
                fn compile(&self) -> Result<CompiledNode> {
                    Err(stub_compile_error())
                }
            }
        )*
    };
}

impl_build_construct_stub! {
    // core
    Subconstruct, Renamed,
    // const / mapping
    Const, Mapping,
    // adapters
    Adapter, SymmetricAdapter, ExprAdapter, Validator, ExprValidator,
    // composite
    Struct, Sequence, Union, Select, FocusedSeq,
    // enum / computed
    Enum, FlagsEnum, Computed, Rebuild, Default, Index, Padded, Aligned,
    FixedSized, NamedTuple, TimestampAdapter,
    // repetition
    Array, ArrayExpr, GreedyRange, RepeatUntil,
    // lazy
    Lazy, LazyStruct, LazyArray, Rebuffered,
    // stream ops / tunneling
    Bitwise, Bytewise, Pointer, PointerExpr, Peek, RawCopy, Prefixed,
    Transformed, Restreamed, Checksum, ByteSwapped, BitsSwapped, LazyBound,
    // control flow
    IfThenElse, Switch, Check, StopIf,
    // formatting
    Hex, HexDump,
}

// Feature-gated stub for Compressed.
#[cfg(feature = "compression")]
impl_build_construct_stub!(Compressed,);

// ===========================================================================
// Leaf BuildConstruct implementations (Phase 12.4)
// ===========================================================================
//
// Leaf constructs have no children, so compilation simply clones the construct
// into its compiled node (embed-and-delegate). The resulting CompiledXxx node
// embeds the declaration-tree construct by value and delegates parse/build/
// sizeof to it at runtime.

/// Macro to generate [`BuildConstruct`] impls for leaf constructs: `compile`
/// clones `self` into the corresponding `CompiledXxx { inner }` node.
///
/// `$compiled` must be a single identifier (the compiled struct name); the
/// `:ident` matcher is used so it can be followed directly by `{` in a struct
/// literal (unlike `:path`/`:ty`). `self` resolves to the impl's receiver.
macro_rules! impl_leaf_build_construct {
    ($($ty:ty => $variant:ident($compiled:ident)),* $(,)?) => {
        $(
            impl BuildConstruct for $ty {
                fn compile(&self) -> Result<CompiledNode> {
                    Ok(CompiledNode::$variant($compiled { inner: self.clone() }))
                }
            }
        )*
    };
}

impl_leaf_build_construct! {
    FormatField => FormatField(CompiledFormatField),
    Bytes => Bytes(CompiledBytes),
    GreedyBytes => GreedyBytes(CompiledGreedyBytes),
    BytesExpr => BytesExpr(CompiledBytesExpr),
    BytesInteger => BytesInteger(CompiledBytesInteger),
    BitsInteger => BitsInteger(CompiledBitsInteger),
    VarInt => VarInt(CompiledVarInt),
    ZigZag => ZigZag(CompiledZigZag),
    Flag => Flag(CompiledFlag),
    CString => CString(CompiledCString),
    PaddedString => PaddedString(CompiledPaddedString),
    Pass => Pass(CompiledPass),
    Terminated => Terminated(CompiledTerminated),
    Tell => Tell(CompiledTell),
    Seek => Seek(CompiledSeek),
    SeekExpr => SeekExpr(CompiledSeekExpr),
    Error => Error(CompiledError),
}

// ===========================================================================
// BuildConstruct for Arc<dyn Construct> (Dynamic variant support, I3 correction)
// ===========================================================================
//
// After the I3 correction (Phase 12.3), `CombinedConstruct::Dynamic` holds an
// `Arc<dyn Construct>` instead of a `Box<dyn Construct>`. The hand-written
// match dispatch below forwards the Dynamic arm to `inner.compile()`, so
// `Arc<dyn Construct>` must implement `BuildConstruct`.
//
// The Dynamic variant is the escape-hatch: compilation does not recurse into
// the wrapped construct (it is opaque behind a trait object). Instead, we
// hand the `Arc` clone directly to `CompiledDynamic`, which falls back to
// the declaration-tree old path at runtime.

impl BuildConstruct for std::sync::Arc<dyn crate::core::Construct> {
    fn compile(&self) -> Result<CompiledNode> {
        // Arc::clone is a cheap atomic refcount bump; the inner construct is
        // preserved verbatim for runtime old-path execution.
        Ok(CompiledNode::Dynamic(crate::compiled::CompiledDynamic {
            inner: std::sync::Arc::clone(self),
        }))
    }
}

// ===========================================================================
// BuildConstruct dispatch on CombinedConstruct
// ===========================================================================
//
// NOTE: enum_dispatch cannot generate the dispatch for CombinedConstruct
// because the Dynamic variant holds `Arc<dyn Construct>` (a trait object),
// which causes the enum_dispatch macro to panic ("does not support unsized
// types"). Instead, we hand-write the match dispatch here. Each arm forwards
// to the inner type's BuildConstruct::compile impl. This is functionally
// equivalent to enum_dispatch's generated match.

impl BuildConstruct for CombinedConstruct {
    fn compile(&self) -> Result<CompiledNode> {
        match self {
            // core
            CombinedConstruct::Subconstruct(inner) => inner.compile(),
            CombinedConstruct::Renamed(inner) => inner.compile(),
            // atomic
            CombinedConstruct::FormatField(inner) => inner.compile(),
            CombinedConstruct::Bytes(inner) => inner.compile(),
            CombinedConstruct::GreedyBytes(inner) => inner.compile(),
            CombinedConstruct::BytesExpr(inner) => inner.compile(),
            CombinedConstruct::BytesInteger(inner) => inner.compile(),
            CombinedConstruct::BitsInteger(inner) => inner.compile(),
            CombinedConstruct::VarInt(inner) => inner.compile(),
            CombinedConstruct::ZigZag(inner) => inner.compile(),
            CombinedConstruct::Flag(inner) => inner.compile(),
            CombinedConstruct::CString(inner) => inner.compile(),
            CombinedConstruct::PaddedString(inner) => inner.compile(),
            // const / mapping
            CombinedConstruct::Const(inner) => inner.compile(),
            CombinedConstruct::Mapping(inner) => inner.compile(),
            // adapters
            CombinedConstruct::Adapter(inner) => inner.compile(),
            CombinedConstruct::SymmetricAdapter(inner) => inner.compile(),
            CombinedConstruct::ExprAdapter(inner) => inner.compile(),
            CombinedConstruct::Validator(inner) => inner.compile(),
            CombinedConstruct::ExprValidator(inner) => inner.compile(),
            // meta
            CombinedConstruct::Pass(inner) => inner.compile(),
            CombinedConstruct::Terminated(inner) => inner.compile(),
            CombinedConstruct::Tell(inner) => inner.compile(),
            CombinedConstruct::Seek(inner) => inner.compile(),
            CombinedConstruct::SeekExpr(inner) => inner.compile(),
            CombinedConstruct::Error(inner) => inner.compile(),
            // composite
            CombinedConstruct::Struct(inner) => inner.compile(),
            CombinedConstruct::Sequence(inner) => inner.compile(),
            CombinedConstruct::Union(inner) => inner.compile(),
            CombinedConstruct::Select(inner) => inner.compile(),
            CombinedConstruct::FocusedSeq(inner) => inner.compile(),
            // enum / computed
            CombinedConstruct::Enum(inner) => inner.compile(),
            CombinedConstruct::FlagsEnum(inner) => inner.compile(),
            CombinedConstruct::Computed(inner) => inner.compile(),
            CombinedConstruct::Rebuild(inner) => inner.compile(),
            CombinedConstruct::Default(inner) => inner.compile(),
            CombinedConstruct::Index(inner) => inner.compile(),
            CombinedConstruct::Padded(inner) => inner.compile(),
            CombinedConstruct::Aligned(inner) => inner.compile(),
            CombinedConstruct::FixedSized(inner) => inner.compile(),
            CombinedConstruct::NamedTuple(inner) => inner.compile(),
            CombinedConstruct::TimestampAdapter(inner) => inner.compile(),
            // repetition
            CombinedConstruct::Array(inner) => inner.compile(),
            CombinedConstruct::ArrayExpr(inner) => inner.compile(),
            CombinedConstruct::GreedyRange(inner) => inner.compile(),
            CombinedConstruct::RepeatUntil(inner) => inner.compile(),
            // lazy
            CombinedConstruct::Lazy(inner) => inner.compile(),
            CombinedConstruct::LazyStruct(inner) => inner.compile(),
            CombinedConstruct::LazyArray(inner) => inner.compile(),
            CombinedConstruct::Rebuffered(inner) => inner.compile(),
            // stream ops / tunneling
            CombinedConstruct::Bitwise(inner) => inner.compile(),
            CombinedConstruct::Bytewise(inner) => inner.compile(),
            CombinedConstruct::Pointer(inner) => inner.compile(),
            CombinedConstruct::PointerExpr(inner) => inner.compile(),
            CombinedConstruct::Peek(inner) => inner.compile(),
            CombinedConstruct::RawCopy(inner) => inner.compile(),
            CombinedConstruct::Prefixed(inner) => inner.compile(),
            CombinedConstruct::Transformed(inner) => inner.compile(),
            CombinedConstruct::Restreamed(inner) => inner.compile(),
            #[cfg(feature = "compression")]
            CombinedConstruct::Compressed(inner) => inner.compile(),
            CombinedConstruct::Checksum(inner) => inner.compile(),
            CombinedConstruct::ByteSwapped(inner) => inner.compile(),
            CombinedConstruct::BitsSwapped(inner) => inner.compile(),
            CombinedConstruct::LazyBound(inner) => inner.compile(),
            // control flow
            CombinedConstruct::IfThenElse(inner) => inner.compile(),
            CombinedConstruct::Switch(inner) => inner.compile(),
            CombinedConstruct::Check(inner) => inner.compile(),
            CombinedConstruct::StopIf(inner) => inner.compile(),
            // formatting wrappers
            CombinedConstruct::Hex(inner) => inner.compile(),
            CombinedConstruct::HexDump(inner) => inner.compile(),
            // escape-hatch
            CombinedConstruct::Dynamic(inner) => inner.compile(),
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constructs::format_field::INT8UB;
    use crate::constructs::meta::Pass;

    // -- Leaf compile succeeds (Phase 12.4) ---------------------------------

    #[test]
    fn leaf_compile_pass_produces_compiled_pass() {
        let node = Pass::new().compile().unwrap();
        match node {
            CompiledNode::Pass(_) => { /* expected */ }
            other => panic!("expected CompiledPass, got {other:?}"),
        }
    }

    #[test]
    fn leaf_compile_format_field_produces_compiled_format_field() {
        let ff: FormatField = INT8UB;
        let node = ff.compile().unwrap();
        match node {
            CompiledNode::FormatField(_) => { /* expected */ }
            other => panic!("expected CompiledFormatField, got {other:?}"),
        }
    }

    #[test]
    fn leaf_compile_embeds_construct_value() {
        // The compiled node embeds a clone of the original construct's params.
        let node = crate::constructs::bytes::Bytes::new(9).compile().unwrap();
        match node {
            CompiledNode::Bytes(b) => assert_eq!(b.inner.length, 9),
            other => panic!("expected CompiledBytes, got {other:?}"),
        }
    }

    // -- Stub compile for not-yet-implemented nodes -------------------------

    #[test]
    fn stub_compile_returns_error_for_struct() {
        let s = Struct::new();
        let result = s.compile();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConstructError::Generic { .. }
        ));
    }

    #[test]
    fn stub_compile_message_contains_not_yet() {
        // Struct (composite) is still a stub.
        let s = Struct::new();
        let err = s.compile().unwrap_err();
        match err {
            ConstructError::Generic { message, .. } => {
                assert!(message.contains("not yet implemented"));
            }
            _ => unreachable!(),
        }
    }

    // -- enum_dispatch on CombinedConstruct ---------------------------------

    #[test]
    fn combined_construct_compile_pass_succeeds() {
        // Pass is a leaf: compile now succeeds.
        let cc: CombinedConstruct = Pass::new().into();
        assert!(cc.compile().is_ok());
    }

    #[test]
    fn combined_construct_compile_on_struct_is_error() {
        let cc: CombinedConstruct = Struct::new().into();
        assert!(cc.compile().is_err());
    }

    #[test]
    fn combined_construct_compile_on_renamed_is_error() {
        let cc: CombinedConstruct = Renamed::new(INT8UB, "test_field").into();
        assert!(cc.compile().is_err());
    }

    // -- Dynamic variant dispatch (I3 correction: Box → Arc) ----------------

    #[test]
    fn dynamic_variant_compile_produces_compiled_dynamic() {
        // After the I3 correction, Dynamic holds Arc<dyn Construct> and its
        // compile produces a functional CompiledDynamic node (escape-hatch),
        // NOT an error.
        let cc = crate::combined::dynamic(Pass::new());
        let result = cc.compile();
        assert!(result.is_ok(), "Dynamic compile should succeed");
        match result.unwrap() {
            CompiledNode::Dynamic(_) => { /* expected */ }
            other => panic!("expected CompiledDynamic, got {other:?}"),
        }
    }

    #[test]
    fn dynamic_variant_compile_preserves_inner_via_arc_clone() {
        // The inner Arc<dyn Construct> is shared (Arc::clone), so both the
        // declaration tree and the compiled node reference the same construct.
        let cc = crate::combined::dynamic(Pass::new());
        let node = cc.compile().unwrap();
        let inner = match node {
            CompiledNode::Dynamic(d) => d.inner,
            _ => unreachable!(),
        };
        // The inner construct is still callable (sizeof == 0 for Pass).
        let ctx = crate::core::context::Context::new();
        assert_eq!(inner.sizeof(&ctx).unwrap(), 0);
    }

    // -- Mixed leaf / stub dispatch ----------------------------------------

    #[test]
    fn all_construct_categories_dispatch_compile() {
        // Leaves compile OK; Struct (composite) is still a stub.
        assert!(Pass::new().compile().is_ok());
        assert!(Struct::new().compile().is_err());
        let cc: CombinedConstruct = INT8UB.into();
        assert!(cc.compile().is_ok());
    }

    // -- BuildConstruct trait object ----------------------------------------

    #[test]
    fn build_construct_can_be_called_on_trait_object() {
        // A leaf construct's compile succeeds through a trait object too.
        let pass: &dyn BuildConstruct = &Pass::new();
        assert!(pass.compile().is_ok());
    }
}
