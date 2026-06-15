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
use crate::compiled::CompiledNode;
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
    // atomic
    FormatField, Bytes, GreedyBytes, BytesExpr, BytesInteger, BitsInteger,
    VarInt, ZigZag, Flag, CString, PaddedString,
    // const / mapping
    Const, Mapping,
    // adapters
    Adapter, SymmetricAdapter, ExprAdapter, Validator, ExprValidator,
    // meta
    Pass, Terminated, Tell, Seek, SeekExpr, Error,
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
// BuildConstruct for Box<dyn Construct> (Dynamic variant support)
// ===========================================================================
//
// CombinedConstruct::Dynamic holds a Box<dyn Construct>. For enum_dispatch
// to generate a match arm for the Dynamic variant, Box<dyn Construct>
// must implement BuildConstruct. This stub returns Err — real impl
// (producing CompiledDynamic with Arc<dyn Construct>) is added in
// Phase 12.4 after the Box→Arc migration (design doc §5.5, I3 correction).

impl BuildConstruct for Box<dyn crate::core::Construct> {
    fn compile(&self) -> Result<CompiledNode> {
        Err(ConstructError::Generic {
            path: String::new(),
            message: "Dynamic construct compilation not yet implemented \
                      (Phase 12.1 stub — real impl in 12.4 after Box→Arc migration)"
                .to_string(),
        })
    }
}

// ===========================================================================
// enum_dispatch: extend CombinedConstruct with BuildConstruct
// ===========================================================================
//
// NOTE: enum_dispatch cannot generate the dispatch for CombinedConstruct
// because the Dynamic variant holds `Box<dyn Construct>` (a trait object),
// which causes the enum_dispatch macro to panic. Instead, we hand-write
// the match dispatch here. Each arm forwards to the inner type's
// BuildConstruct::compile impl.
//
// In Phase 12.4, after the Box→Arc migration (design §5.5, I3 correction),
// this can potentially be switched to enum_dispatch. For now, the hand-
// written match is correct and functionally equivalent.

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

    // -- Stub implementations return errors ---------------------------------

    #[test]
    fn stub_compile_returns_error_for_format_field() {
        let ff: FormatField = INT8UB;
        let result = ff.compile();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    #[test]
    fn stub_compile_returns_error_for_struct() {
        let s = Struct::new();
        let result = s.compile();
        assert!(result.is_err());
    }

    #[test]
    fn stub_compile_returns_error_for_pass() {
        let p = Pass::new();
        let result = p.compile();
        assert!(result.is_err());
    }

    #[test]
    fn stub_compile_message_contains_not_yet() {
        let p = Pass::new();
        let err = p.compile().unwrap_err();
        match err {
            ConstructError::Generic { message, .. } => {
                assert!(message.contains("not yet implemented"));
            }
            _ => unreachable!(),
        }
    }

    // -- enum_dispatch on CombinedConstruct ---------------------------------

    #[test]
    fn combined_construct_dispatches_compile() {
        // Create a CombinedConstruct and call compile via dispatch.
        let cc: CombinedConstruct = Pass::new().into();
        let result = cc.compile();
        assert!(result.is_err());
    }

    #[test]
    fn combined_construct_compile_on_struct() {
        let cc: CombinedConstruct = Struct::new().into();
        let result = cc.compile();
        assert!(result.is_err());
    }

    #[test]
    fn combined_construct_compile_on_renamed() {
        let cc: CombinedConstruct = Renamed::new(INT8UB, "test_field").into();
        let result = cc.compile();
        assert!(result.is_err());
    }

    // -- Dynamic variant dispatch -------------------------------------------

    #[test]
    fn dynamic_variant_compile_returns_error() {
        // Dynamic variant holds Box<dyn Construct>. enum_dispatch should
        // dispatch to Box<dyn Construct>::compile (our stub).
        let cc = crate::combined::dynamic(Pass::new());
        let result = cc.compile();
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            ConstructError::Generic { message, .. } => {
                assert!(message.contains("Dynamic"));
            }
            _ => unreachable!(),
        }
    }

    // -- All major construct categories dispatch ----------------------------

    #[test]
    fn all_construct_categories_dispatch_compile() {
        let constructs: Vec<CombinedConstruct> = vec![
            Pass::new().into(),
            Struct::new().into(),
            INT8UB.into(),
            crate::combined::dynamic(Pass::new()),
        ];
        for cc in &constructs {
            assert!(cc.compile().is_err(), "expected compile to return Err");
        }
    }

    // -- BuildConstruct trait object ----------------------------------------

    #[test]
    fn build_construct_can_be_called_on_trait_object() {
        let pass: &dyn BuildConstruct = &Pass::new();
        let result = pass.compile();
        assert!(result.is_err());
    }
}
