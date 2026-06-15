//! Compiled execution tree — build-once representation of a construct schema.
//!
//! This module defines the runtime representation of a compiled schema:
//!
//! - [`CompiledNode`] — an enum with 71 variants that mirror
//!   [`CombinedConstruct`](crate::combined::CombinedConstruct) 1:1.
//! - [`CompiledExec`] — the runtime trait implemented by each variant.
//! - [`CompiledSchema`] — the top-level container and entry point for
//!   `parse_bytes` / `build_bytes`.
//!
//! Produced once by a compiler (Phase 12.4) from a declaration tree,
//! a `CompiledNode` tree is traversed on every `parse` / `build` call
//! with zero per-call construction overhead.
//!
//! # Phase 12.1 scope
//!
//! This module defines the **infrastructure**: the enum, trait, schema
//! container, and sink mechanism. All [`CompiledExec`] and
//! [`BuildConstruct`](build::BuildConstruct) implementations are
//! **stubs** that return `Err` — real implementations are filled in
//! across sub-tasks 12.4-12.9.

pub mod build;
pub mod expr;
pub mod sink;

pub use expr::{compile_expr, CompiledExpr};
pub use sink::{OutputSink, ValueSink};

use std::sync::Arc;

use crate::core::context::Context;
use crate::core::error::{ConstructError, Result};
use crate::core::stream::{ByteStream, CombinedStream};
use crate::value::Value;

// ===========================================================================
// CompiledExec trait
// ===========================================================================

/// Runtime trait for compiled execution-tree nodes.
///
/// Each variant of [`CompiledNode`] implements this trait. Unlike the
/// declaration-tree [`Construct`](crate::core::Construct) trait,
/// `CompiledExec` operates on already-compiled nodes — no per-call
/// parameter extraction.
///
/// The three methods mirror [`Construct`](crate::core::Construct):
/// - [`exec_parse`](CompiledExec::exec_parse) — reads from the stream and
///   writes results to the [`OutputSink`].
/// - [`exec_build`](CompiledExec::exec_build) — reads from a [`Value`]
///   tree and writes binary data to the stream.
/// - [`exec_sizeof`](CompiledExec::exec_sizeof) — computes the byte size,
///   with compile-time constant folding where possible.
#[enum_dispatch::enum_dispatch]
pub trait CompiledExec {
    /// Parses from the stream, writing the result into `sink` and updating
    /// `ctx` for downstream field references.
    ///
    /// Unlike [`Construct::parse`](crate::core::Construct::parse), this
    /// method does **not** return a [`Value`] — instead, the result is
    /// deposited into `sink` (leaf nodes via
    /// [`set_scalar`](OutputSink::set_scalar), composite nodes via
    /// [`set_field`](OutputSink::set_field) /
    /// [`push_item`](OutputSink::push_item)).
    ///
    /// # Errors
    ///
    /// Propagates any [`ConstructError`] from stream reading or sink
    /// operations.
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()>;

    /// Builds binary data, reading field values from the `input` [`Value`]
    /// tree, writing bytes to `stream`, and updating `ctx`.
    ///
    /// # Errors
    ///
    /// Propagates any [`ConstructError`] from stream writing or value
    /// extraction.
    fn exec_build(
        &self,
        input: &Value,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()>;

    /// Computes the byte size, with compile-time constant folding where
    /// possible.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::Sizeof`] if the size is not statically
    /// determinable.
    fn exec_sizeof(&self, ctx: &Context) -> Result<usize>;
}

// ===========================================================================
// Compiled node struct definitions (Phase 12.1: unit structs)
// ===========================================================================
//
// All 71 compiled node types are defined as **unit structs** in Phase 12.1.
// Real field definitions (embedding inner constructs, pre-compiled
// expressions, fixed mapping tables, etc.) are added in sub-tasks
// 12.4-12.9. The enum and trait infrastructure is fully functional —
// only the per-variant exec/compile implementations are stubs.

// -- Leaf constructors (embed concrete construct in 12.5) --
/// Compiled node for `FormatField`.
#[derive(Debug)]
pub struct CompiledFormatField;
/// Compiled node for `VarInt`.
#[derive(Debug)]
pub struct CompiledVarInt;
/// Compiled node for `ZigZag`.
#[derive(Debug)]
pub struct CompiledZigZag;
/// Compiled node for `Flag`.
#[derive(Debug)]
pub struct CompiledFlag;
/// Compiled node for `Bytes`.
#[derive(Debug)]
pub struct CompiledBytes;
/// Compiled node for `GreedyBytes`.
#[derive(Debug)]
pub struct CompiledGreedyBytes;
/// Compiled node for `BytesExpr`.
#[derive(Debug)]
pub struct CompiledBytesExpr;
/// Compiled node for `BytesInteger`.
#[derive(Debug)]
pub struct CompiledBytesInteger;
/// Compiled node for `BitsInteger`.
#[derive(Debug)]
pub struct CompiledBitsInteger;
/// Compiled node for `CString`.
#[derive(Debug)]
pub struct CompiledCString;
/// Compiled node for `PaddedString`.
#[derive(Debug)]
pub struct CompiledPaddedString;

// -- Wrappers (hold compiled sub-node in 12.6) --
/// Compiled node for `Const`.
#[derive(Debug)]
pub struct CompiledConst;
/// Compiled node for `Renamed`.
#[derive(Debug)]
pub struct CompiledRenamed;
/// Compiled node for `Subconstruct`.
#[derive(Debug)]
pub struct CompiledSubconstruct;
/// Compiled node for `Adapter`.
#[derive(Debug)]
pub struct CompiledAdapter;
/// Compiled node for `SymmetricAdapter`.
#[derive(Debug)]
pub struct CompiledSymmetricAdapter;
/// Compiled node for `ExprAdapter`.
#[derive(Debug)]
pub struct CompiledExprAdapter;
/// Compiled node for `Validator`.
#[derive(Debug)]
pub struct CompiledValidator;
/// Compiled node for `ExprValidator`.
#[derive(Debug)]
pub struct CompiledExprValidator;
/// Compiled node for `Hex`.
#[derive(Debug)]
pub struct CompiledHex;
/// Compiled node for `HexDump`.
#[derive(Debug)]
pub struct CompiledHexDump;

// -- Meta constructors --
/// Compiled node for `Pass`.
#[derive(Debug)]
pub struct CompiledPass;
/// Compiled node for `Terminated`.
#[derive(Debug)]
pub struct CompiledTerminated;
/// Compiled node for `Tell`.
#[derive(Debug)]
pub struct CompiledTell;
/// Compiled node for `Seek`.
#[derive(Debug)]
pub struct CompiledSeek;
/// Compiled node for `SeekExpr`.
#[derive(Debug)]
pub struct CompiledSeekExpr;
/// Compiled node for `Error`.
#[derive(Debug)]
pub struct CompiledError;

// -- Composite constructors (hold Vec<CompiledField> in 12.7) --
/// Compiled node for `Struct`.
#[derive(Debug)]
pub struct CompiledStruct;
/// Compiled node for `Sequence`.
#[derive(Debug)]
pub struct CompiledSequence;
/// Compiled node for `Union`.
#[derive(Debug)]
pub struct CompiledUnion;
/// Compiled node for `Select`.
#[derive(Debug)]
pub struct CompiledSelect;
/// Compiled node for `FocusedSeq`.
#[derive(Debug)]
pub struct CompiledFocusedSeq;

// -- Enum / computed constructors (mapping tables in 12.8) --
/// Compiled node for `Enum`.
#[derive(Debug)]
pub struct CompiledEnum;
/// Compiled node for `FlagsEnum`.
#[derive(Debug)]
pub struct CompiledFlagsEnum;
/// Compiled node for `Mapping`.
#[derive(Debug)]
pub struct CompiledMapping;
/// Compiled node for `Computed`.
#[derive(Debug)]
pub struct CompiledComputed;
/// Compiled node for `Rebuild`.
#[derive(Debug)]
pub struct CompiledRebuild;
/// Compiled node for `Default`.
#[derive(Debug)]
pub struct CompiledDefault;
/// Compiled node for `Index`.
#[derive(Debug)]
pub struct CompiledIndex;
/// Compiled node for `Padded`.
#[derive(Debug)]
pub struct CompiledPadded;
/// Compiled node for `Aligned`.
#[derive(Debug)]
pub struct CompiledAligned;
/// Compiled node for `FixedSized`.
#[derive(Debug)]
pub struct CompiledFixedSized;
/// Compiled node for `NamedTuple`.
#[derive(Debug)]
pub struct CompiledNamedTuple;
/// Compiled node for `TimestampAdapter`.
#[derive(Debug)]
pub struct CompiledTimestampAdapter;

// -- Repetition constructors --
/// Compiled node for `Array`.
#[derive(Debug)]
pub struct CompiledArray;
/// Compiled node for `ArrayExpr`.
#[derive(Debug)]
pub struct CompiledArrayExpr;
/// Compiled node for `GreedyRange`.
#[derive(Debug)]
pub struct CompiledGreedyRange;
/// Compiled node for `RepeatUntil`.
#[derive(Debug)]
pub struct CompiledRepeatUntil;

// -- Lazy constructors --
/// Compiled node for `Lazy`.
#[derive(Debug)]
pub struct CompiledLazy;
/// Compiled node for `LazyStruct`.
#[derive(Debug)]
pub struct CompiledLazyStruct;
/// Compiled node for `LazyArray`.
#[derive(Debug)]
pub struct CompiledLazyArray;
/// Compiled node for `Rebuffered`.
#[derive(Debug)]
pub struct CompiledRebuffered;

// -- Stream ops / tunneling constructors --
/// Compiled node for `Bitwise`.
#[derive(Debug)]
pub struct CompiledBitwise;
/// Compiled node for `Bytewise`.
#[derive(Debug)]
pub struct CompiledBytewise;
/// Compiled node for `Pointer`.
#[derive(Debug)]
pub struct CompiledPointer;
/// Compiled node for `PointerExpr`.
#[derive(Debug)]
pub struct CompiledPointerExpr;
/// Compiled node for `Peek`.
#[derive(Debug)]
pub struct CompiledPeek;
/// Compiled node for `RawCopy`.
#[derive(Debug)]
pub struct CompiledRawCopy;
/// Compiled node for `Prefixed`.
#[derive(Debug)]
pub struct CompiledPrefixed;
/// Compiled node for `Transformed`.
#[derive(Debug)]
pub struct CompiledTransformed;
/// Compiled node for `Restreamed`.
#[derive(Debug)]
pub struct CompiledRestreamed;
/// Compiled node for `Compressed` (feature-gated).
#[cfg(feature = "compression")]
#[derive(Debug)]
pub struct CompiledCompressed;
/// Compiled node for `Checksum`.
#[derive(Debug)]
pub struct CompiledChecksum;
/// Compiled node for `ByteSwapped`.
#[derive(Debug)]
pub struct CompiledByteSwapped;
/// Compiled node for `BitsSwapped`.
#[derive(Debug)]
pub struct CompiledBitsSwapped;
/// Compiled node for `LazyBound`.
#[derive(Debug)]
pub struct CompiledLazyBound;

// -- Control flow constructors --
/// Compiled node for `IfThenElse`.
#[derive(Debug)]
pub struct CompiledIfThenElse;
/// Compiled node for `Switch`.
#[derive(Debug)]
pub struct CompiledSwitch;
/// Compiled node for `Check`.
#[derive(Debug)]
pub struct CompiledCheck;
/// Compiled node for `StopIf`.
#[derive(Debug)]
pub struct CompiledStopIf;

// -- Escape-hatch --
/// Compiled node for `Dynamic` — holds an `Arc<dyn Construct>` that falls
/// back to the declaration-tree old path at runtime.
///
/// This is the escape-hatch for constructs that cannot be compiled (e.g.
/// user-defined constructs, Python callables in Phase 13). The inner
/// `Arc<dyn Construct>` is called directly at runtime, bypassing the
/// compiled tree.
#[derive(Debug)]
pub struct CompiledDynamic;

// ===========================================================================
// CompiledNode enum
// ===========================================================================

/// The compiled execution-tree node — runtime representation of a schema.
///
/// Variant list **mirrors [`CombinedConstruct`] exactly** (70 built-in +
/// `Dynamic` = 71 variants with `compression` feature; 70 without).
/// Phase 12 does **not** define a `PyCallback` variant (B1 correction);
/// Python callable sources compile to `Dynamic` (old-path fallback).
/// Phase 13 will extend with `PyCallback` in the `construct-py` crate.
///
/// Each variant holds a compiled node struct (currently a unit struct in
/// Phase 12.1; real fields are populated in sub-tasks 12.4-12.9).
///
/// [`CombinedConstruct`]: crate::combined::CombinedConstruct
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
#[enum_dispatch::enum_dispatch(CompiledExec)]
pub enum CompiledNode {
    // -- core --
    /// Wraps [`CompiledSubconstruct`].
    Subconstruct(CompiledSubconstruct),
    /// Wraps [`CompiledRenamed`].
    Renamed(CompiledRenamed),
    // -- atomic --
    /// Wraps [`CompiledFormatField`].
    FormatField(CompiledFormatField),
    /// Wraps [`CompiledBytes`].
    Bytes(CompiledBytes),
    /// Wraps [`CompiledGreedyBytes`].
    GreedyBytes(CompiledGreedyBytes),
    /// Wraps [`CompiledBytesExpr`].
    BytesExpr(CompiledBytesExpr),
    /// Wraps [`CompiledBytesInteger`].
    BytesInteger(CompiledBytesInteger),
    /// Wraps [`CompiledBitsInteger`].
    BitsInteger(CompiledBitsInteger),
    /// Wraps [`CompiledVarInt`].
    VarInt(CompiledVarInt),
    /// Wraps [`CompiledZigZag`].
    ZigZag(CompiledZigZag),
    /// Wraps [`CompiledFlag`].
    Flag(CompiledFlag),
    /// Wraps [`CompiledCString`].
    CString(CompiledCString),
    /// Wraps [`CompiledPaddedString`].
    PaddedString(CompiledPaddedString),
    // -- const / mapping --
    /// Wraps [`CompiledConst`].
    Const(CompiledConst),
    /// Wraps [`CompiledMapping`].
    Mapping(CompiledMapping),
    // -- adapters --
    /// Wraps [`CompiledAdapter`].
    Adapter(CompiledAdapter),
    /// Wraps [`CompiledSymmetricAdapter`].
    SymmetricAdapter(CompiledSymmetricAdapter),
    /// Wraps [`CompiledExprAdapter`].
    ExprAdapter(CompiledExprAdapter),
    /// Wraps [`CompiledValidator`].
    Validator(CompiledValidator),
    /// Wraps [`CompiledExprValidator`].
    ExprValidator(CompiledExprValidator),
    // -- meta --
    /// Wraps [`CompiledPass`].
    Pass(CompiledPass),
    /// Wraps [`CompiledTerminated`].
    Terminated(CompiledTerminated),
    /// Wraps [`CompiledTell`].
    Tell(CompiledTell),
    /// Wraps [`CompiledSeek`].
    Seek(CompiledSeek),
    /// Wraps [`CompiledSeekExpr`].
    SeekExpr(CompiledSeekExpr),
    /// Wraps [`CompiledError`].
    Error(CompiledError),
    // -- composite --
    /// Wraps [`CompiledStruct`].
    Struct(CompiledStruct),
    /// Wraps [`CompiledSequence`].
    Sequence(CompiledSequence),
    /// Wraps [`CompiledUnion`].
    Union(CompiledUnion),
    /// Wraps [`CompiledSelect`].
    Select(CompiledSelect),
    /// Wraps [`CompiledFocusedSeq`].
    FocusedSeq(CompiledFocusedSeq),
    // -- enum / computed --
    /// Wraps [`CompiledEnum`].
    Enum(CompiledEnum),
    /// Wraps [`CompiledFlagsEnum`].
    FlagsEnum(CompiledFlagsEnum),
    /// Wraps [`CompiledComputed`].
    Computed(CompiledComputed),
    /// Wraps [`CompiledRebuild`].
    Rebuild(CompiledRebuild),
    /// Wraps [`CompiledDefault`].
    Default(CompiledDefault),
    /// Wraps [`CompiledIndex`].
    Index(CompiledIndex),
    /// Wraps [`CompiledPadded`].
    Padded(CompiledPadded),
    /// Wraps [`CompiledAligned`].
    Aligned(CompiledAligned),
    /// Wraps [`CompiledFixedSized`].
    FixedSized(CompiledFixedSized),
    /// Wraps [`CompiledNamedTuple`].
    NamedTuple(CompiledNamedTuple),
    /// Wraps [`CompiledTimestampAdapter`].
    TimestampAdapter(CompiledTimestampAdapter),
    // -- repetition --
    /// Wraps [`CompiledArray`].
    Array(CompiledArray),
    /// Wraps [`CompiledArrayExpr`].
    ArrayExpr(CompiledArrayExpr),
    /// Wraps [`CompiledGreedyRange`].
    GreedyRange(CompiledGreedyRange),
    /// Wraps [`CompiledRepeatUntil`].
    RepeatUntil(CompiledRepeatUntil),
    // -- lazy --
    /// Wraps [`CompiledLazy`].
    Lazy(CompiledLazy),
    /// Wraps [`CompiledLazyStruct`].
    LazyStruct(CompiledLazyStruct),
    /// Wraps [`CompiledLazyArray`].
    LazyArray(CompiledLazyArray),
    /// Wraps [`CompiledRebuffered`].
    Rebuffered(CompiledRebuffered),
    // -- stream ops / tunneling --
    /// Wraps [`CompiledBitwise`].
    Bitwise(CompiledBitwise),
    /// Wraps [`CompiledBytewise`].
    Bytewise(CompiledBytewise),
    /// Wraps [`CompiledPointer`].
    Pointer(CompiledPointer),
    /// Wraps [`CompiledPointerExpr`].
    PointerExpr(CompiledPointerExpr),
    /// Wraps [`CompiledPeek`].
    Peek(CompiledPeek),
    /// Wraps [`CompiledRawCopy`].
    RawCopy(CompiledRawCopy),
    /// Wraps [`CompiledPrefixed`].
    Prefixed(CompiledPrefixed),
    /// Wraps [`CompiledTransformed`].
    Transformed(CompiledTransformed),
    /// Wraps [`CompiledRestreamed`].
    Restreamed(CompiledRestreamed),
    /// Wraps [`CompiledCompressed`] (feature-gated).
    #[cfg(feature = "compression")]
    Compressed(CompiledCompressed),
    /// Wraps [`CompiledChecksum`].
    Checksum(CompiledChecksum),
    /// Wraps [`CompiledByteSwapped`].
    ByteSwapped(CompiledByteSwapped),
    /// Wraps [`CompiledBitsSwapped`].
    BitsSwapped(CompiledBitsSwapped),
    /// Wraps [`CompiledLazyBound`].
    LazyBound(CompiledLazyBound),
    // -- control flow --
    /// Wraps [`CompiledIfThenElse`].
    IfThenElse(CompiledIfThenElse),
    /// Wraps [`CompiledSwitch`].
    Switch(CompiledSwitch),
    /// Wraps [`CompiledCheck`].
    Check(CompiledCheck),
    /// Wraps [`CompiledStopIf`].
    StopIf(CompiledStopIf),
    // -- formatting wrappers --
    /// Wraps [`CompiledHex`].
    Hex(CompiledHex),
    /// Wraps [`CompiledHexDump`].
    HexDump(CompiledHexDump),
    // -- escape-hatch --
    /// Wraps [`CompiledDynamic`].
    Dynamic(CompiledDynamic),
}

// ===========================================================================
// Stub CompiledExec implementations
// ===========================================================================
//
// All compiled node types implement CompiledExec with stub implementations
// that return a descriptive error. Real implementations are added in
// sub-tasks 12.4-12.9.

/// Error message for stub CompiledExec implementations.
const STUB_EXEC_MESSAGE: &str =
    "CompiledExec not yet implemented (Phase 12.1 stub — real impl in 12.4-12.9)";

/// Builds a stub error for CompiledExec methods.
fn stub_exec_error() -> ConstructError {
    ConstructError::Generic {
        path: String::new(),
        message: STUB_EXEC_MESSAGE.to_string(),
    }
}

/// Macro to generate stub CompiledExec impls for all compiled node types.
macro_rules! impl_compiled_exec_stub {
    ($($ty:ty),* $(,)?) => {
        $(
            impl CompiledExec for $ty {
                fn exec_parse(
                    &self,
                    _stream: &mut CombinedStream,
                    _ctx: &mut Context,
                    _sink: &mut dyn OutputSink,
                ) -> Result<()> {
                    Err(stub_exec_error())
                }

                fn exec_build(
                    &self,
                    _input: &Value,
                    _stream: &mut CombinedStream,
                    _ctx: &mut Context,
                ) -> Result<()> {
                    Err(stub_exec_error())
                }

                fn exec_sizeof(&self, _ctx: &Context) -> Result<usize> {
                    Err(stub_exec_error())
                }
            }
        )*
    };
}

impl_compiled_exec_stub! {
    // core
    CompiledSubconstruct, CompiledRenamed,
    // atomic
    CompiledFormatField, CompiledBytes, CompiledGreedyBytes, CompiledBytesExpr,
    CompiledBytesInteger, CompiledBitsInteger, CompiledVarInt, CompiledZigZag,
    CompiledFlag, CompiledCString, CompiledPaddedString,
    // const / mapping
    CompiledConst, CompiledMapping,
    // adapters
    CompiledAdapter, CompiledSymmetricAdapter, CompiledExprAdapter,
    CompiledValidator, CompiledExprValidator,
    // meta
    CompiledPass, CompiledTerminated, CompiledTell, CompiledSeek,
    CompiledSeekExpr, CompiledError,
    // composite
    CompiledStruct, CompiledSequence, CompiledUnion, CompiledSelect,
    CompiledFocusedSeq,
    // enum / computed
    CompiledEnum, CompiledFlagsEnum, CompiledComputed, CompiledRebuild,
    CompiledDefault, CompiledIndex, CompiledPadded, CompiledAligned,
    CompiledFixedSized, CompiledNamedTuple, CompiledTimestampAdapter,
    // repetition
    CompiledArray, CompiledArrayExpr, CompiledGreedyRange, CompiledRepeatUntil,
    // lazy
    CompiledLazy, CompiledLazyStruct, CompiledLazyArray, CompiledRebuffered,
    // stream ops / tunneling
    CompiledBitwise, CompiledBytewise, CompiledPointer, CompiledPointerExpr,
    CompiledPeek, CompiledRawCopy, CompiledPrefixed, CompiledTransformed,
    CompiledRestreamed, CompiledChecksum, CompiledByteSwapped, CompiledBitsSwapped,
    CompiledLazyBound,
    // control flow
    CompiledIfThenElse, CompiledSwitch, CompiledCheck, CompiledStopIf,
    // formatting
    CompiledHex, CompiledHexDump,
    // escape-hatch
    CompiledDynamic,
}

// Feature-gated stub for CompiledCompressed.
#[cfg(feature = "compression")]
impl_compiled_exec_stub!(CompiledCompressed,);

// ===========================================================================
// CompiledSchema
// ===========================================================================

/// The initial path string used for parse operations.
const PARSE_PATH: &str = "(parsing)";

/// The initial path string used for build operations.
const BUILD_PATH: &str = "(building)";

/// The initial path string used for sizeof operations.
const SIZEOF_PATH: &str = "(sizeof)";

/// A compiled construct schema — the runtime entry point for parse/build.
///
/// Produced once by a compiler from a
/// [`CombinedConstruct`](crate::combined::CombinedConstruct) declaration
/// tree. The compiled tree is stored behind an [`Arc`] for cheap cloning
/// and caching.
///
/// # Pure Rust path (Phase 12)
///
/// [`parse_bytes`](Self::parse_bytes) returns a [`Value`] tree and
/// [`build_bytes`](Self::build_bytes) accepts a [`Value`] tree. No Python
/// objects are involved — this is the pure Rust path.
///
/// # Python correspondence
///
/// This is the construct equivalent of pydantic-core's `SchemaValidator`,
/// which holds `Arc<CombinedValidator>`.
#[derive(Debug)]
pub struct CompiledSchema {
    /// The compiled execution tree (root node).
    tree: Arc<CompiledNode>,
    /// Optional schema name (for error messages / debugging).
    name: Option<String>,
    /// Compile-time computed static size, if determinable.
    /// `None` if the schema has variable-length components.
    static_size: Option<usize>,
}

impl CompiledSchema {
    /// Creates a new `CompiledSchema` from a compiled tree node.
    ///
    /// This is called by the compiler after compilation. Users typically do
    /// not call this directly.
    #[must_use]
    pub fn new(tree: CompiledNode) -> Self {
        let static_size = try_fold_static_size(&tree);
        CompiledSchema {
            tree: Arc::new(tree),
            name: None,
            static_size,
        }
    }

    /// Sets the schema name (for debugging / error messages).
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Returns the root compiled node.
    #[must_use]
    pub fn tree(&self) -> &CompiledNode {
        &self.tree
    }

    /// Returns the optional schema name.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Returns the pre-computed static byte size, if known at compile time.
    #[must_use]
    pub fn static_size(&self) -> Option<usize> {
        self.static_size
    }

    /// Parses binary data and returns the result as a [`Value`].
    ///
    /// **Pure Rust path** (Phase 12): the entire tree is traversed in Rust,
    /// producing a [`Value`]. No Python objects are involved.
    ///
    /// # Errors
    ///
    /// Propagates any [`ConstructError`] from the underlying
    /// [`CompiledExec::exec_parse`] call.
    pub fn parse_bytes(&self, data: &[u8]) -> Result<Value> {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(data));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        self.tree
            .exec_parse(&mut stream, &mut ctx, &mut sink)
            .map_err(|e| e.with_path_prefix(PARSE_PATH))?;
        Box::new(sink).into_value()
    }

    /// Builds binary data from a [`Value`] and returns it as bytes.
    ///
    /// **Pure Rust path** (Phase 12): reads field values from the `Value`
    /// tree, traverses the compiled tree, writes to an in-memory stream.
    ///
    /// # Errors
    ///
    /// Propagates any [`ConstructError`] from the underlying
    /// [`CompiledExec::exec_build`] call.
    pub fn build_bytes(&self, data: &Value) -> Result<Vec<u8>> {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        self.tree
            .exec_build(data, &mut stream, &mut ctx)
            .map_err(|e| e.with_path_prefix(BUILD_PATH))?;
        match stream {
            CombinedStream::ByteStream(bs) => Ok(bs.into_bytes()),
            _ => Ok(Vec::new()),
        }
    }

    /// Computes the byte size. Uses compile-time folding where possible.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::Sizeof`] if the size is not statically
    /// determinable.
    pub fn sizeof(&self, ctx: &Context) -> Result<usize> {
        // Fast path: if static_size was computed at compile time, return it.
        if let Some(size) = self.static_size {
            return Ok(size);
        }
        self.tree
            .exec_sizeof(ctx)
            .map_err(|e| e.with_path_prefix(SIZEOF_PATH))
    }
}

// ===========================================================================
// Static size folding (Phase 12.1: always returns None)
// ===========================================================================

/// Attempts to compute the static byte size of a compiled tree.
///
/// Returns `Some(size)` if all nodes are fixed-length, `None` otherwise.
/// This enables [`CompiledSchema::sizeof`] to return a constant without
/// runtime traversal for fully-static schemas.
///
/// In Phase 12.1, this always returns `None` (conservative default). Real
/// folding logic is implemented in sub-tasks 12.4-12.9 as compiled node
/// structs gain their real fields.
fn try_fold_static_size(_node: &CompiledNode) -> Option<usize> {
    None
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -- CompiledNode construction (unit struct variants) -------------------

    #[test]
    fn compiled_node_pass_variant_constructible() {
        let node = CompiledNode::Pass(CompiledPass);
        assert!(matches!(node, CompiledNode::Pass(_)));
    }

    #[test]
    fn compiled_node_dynamic_variant_constructible() {
        let node = CompiledNode::Dynamic(CompiledDynamic);
        assert!(matches!(node, CompiledNode::Dynamic(_)));
    }

    #[test]
    fn compiled_node_struct_variant_constructible() {
        let node = CompiledNode::Struct(CompiledStruct);
        assert!(matches!(node, CompiledNode::Struct(_)));
    }

    // -- CompiledExec stubs return errors -----------------------------------

    #[test]
    fn stub_exec_parse_returns_error() {
        let node = CompiledNode::Pass(CompiledPass);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[]));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        let err = node
            .exec_parse(&mut stream, &mut ctx, &mut sink)
            .unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
        let msg = match err {
            ConstructError::Generic { message, .. } => message,
            _ => unreachable!(),
        };
        assert!(msg.contains("not yet implemented"));
    }

    #[test]
    fn stub_exec_build_returns_error() {
        let node = CompiledNode::FormatField(CompiledFormatField);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        let err = node
            .exec_build(&Value::None, &mut stream, &mut ctx)
            .unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    #[test]
    fn stub_exec_sizeof_returns_error() {
        let node = CompiledNode::Bytes(CompiledBytes);
        let ctx = Context::new();
        let err = node.exec_sizeof(&ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    // -- CompiledExec dispatches through enum_dispatch ----------------------

    #[test]
    fn exec_dispatches_to_correct_variant() {
        // Each variant should return the stub error — verifying dispatch works.
        let variants: Vec<CompiledNode> = vec![
            CompiledNode::Pass(CompiledPass),
            CompiledNode::Struct(CompiledStruct),
            CompiledNode::Dynamic(CompiledDynamic),
            CompiledNode::Tell(CompiledTell),
            CompiledNode::FormatField(CompiledFormatField),
        ];
        let ctx = Context::new();
        for node in &variants {
            let err = node.exec_sizeof(&ctx).unwrap_err();
            assert!(matches!(err, ConstructError::Generic { .. }));
        }
    }

    // -- CompiledSchema -----------------------------------------------------

    #[test]
    fn compiled_schema_new_creates_schema() {
        let schema = CompiledSchema::new(CompiledNode::Pass(CompiledPass));
        assert!(schema.static_size().is_none());
        assert!(schema.name().is_none());
    }

    #[test]
    fn compiled_schema_with_name_sets_name() {
        let schema = CompiledSchema::new(CompiledNode::Pass(CompiledPass)).with_name("my_schema");
        assert_eq!(schema.name(), Some("my_schema"));
    }

    #[test]
    fn compiled_schema_tree_returns_root() {
        let schema = CompiledSchema::new(CompiledNode::Pass(CompiledPass));
        assert!(matches!(schema.tree(), CompiledNode::Pass(_)));
    }

    #[test]
    fn compiled_schema_parse_bytes_returns_stub_error() {
        // Since all exec impls are stubs, parse_bytes should return Err.
        let schema = CompiledSchema::new(CompiledNode::Pass(CompiledPass));
        let result = schema.parse_bytes(b"\x01\x02");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
        // Path should be set to PARSE_PATH.
        assert_eq!(err.path(), PARSE_PATH);
    }

    #[test]
    fn compiled_schema_build_bytes_returns_stub_error() {
        let schema = CompiledSchema::new(CompiledNode::Pass(CompiledPass));
        let result = schema.build_bytes(&Value::None);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
        assert_eq!(err.path(), BUILD_PATH);
    }

    #[test]
    fn compiled_schema_sizeof_returns_stub_error() {
        let schema = CompiledSchema::new(CompiledNode::Pass(CompiledPass));
        let ctx = Context::new();
        let result = schema.sizeof(&ctx);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
        assert_eq!(err.path(), SIZEOF_PATH);
    }

    #[test]
    fn compiled_schema_debug_formats() {
        let schema = CompiledSchema::new(CompiledNode::Pass(CompiledPass));
        let _ = format!("{schema:?}");
    }

    // -- Static size folding (stub) -----------------------------------------

    #[test]
    fn try_fold_static_size_returns_none_for_all_variants() {
        let variants: Vec<CompiledNode> = vec![
            CompiledNode::Pass(CompiledPass),
            CompiledNode::Struct(CompiledStruct),
            CompiledNode::Bytes(CompiledBytes),
        ];
        for node in &variants {
            assert_eq!(try_fold_static_size(node), None);
        }
    }

    // -- Variant count sanity check -----------------------------------------

    #[test]
    fn all_major_variants_are_constructible() {
        // Smoke test: construct one of each variant category to ensure
        // the enum compiles and enum_dispatch generates valid dispatch.
        let _ = CompiledNode::Subconstruct(CompiledSubconstruct);
        let _ = CompiledNode::FormatField(CompiledFormatField);
        let _ = CompiledNode::Adapter(CompiledAdapter);
        let _ = CompiledNode::Pass(CompiledPass);
        let _ = CompiledNode::Struct(CompiledStruct);
        let _ = CompiledNode::Enum(CompiledEnum);
        let _ = CompiledNode::Array(CompiledArray);
        let _ = CompiledNode::Lazy(CompiledLazy);
        let _ = CompiledNode::Bitwise(CompiledBitwise);
        let _ = CompiledNode::IfThenElse(CompiledIfThenElse);
        let _ = CompiledNode::Hex(CompiledHex);
        let _ = CompiledNode::Dynamic(CompiledDynamic);
    }
}
