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
pub mod input;
pub mod sink;

#[cfg(feature = "python")]
pub mod py_input;
#[cfg(feature = "python")]
pub mod py_sink;

pub use build::BuildConstruct;
pub use expr::{compile_expr, CompiledExpr, ExprExtension};
pub use input::{Input, OwnedValueInput, ValueInput};
pub use sink::{OutputSink, ProducedOutput, ValueSink};

use std::sync::Arc;

use indexmap::IndexMap;

use crate::constructs::adapters::{CheckFunc, DecodeFunc, EncodeFunc, SymmetricFunc};
use crate::constructs::bytes::{Bytes, BytesExpr, GreedyBytes};
use crate::constructs::bytes_integer::{BitsInteger, BytesInteger};
use crate::constructs::computed::ComputeFunc;
use crate::constructs::computed::{TimestampEpoch, TimestampUnit};
use crate::constructs::control_flow::{
    CheckFunc as ControlCheckFunc, CondFunc, KeyFunc, StopCondFunc,
};
use crate::constructs::flag::Flag;
use crate::constructs::format_field::FormatField;
use crate::constructs::meta::{Error, Pass, Seek, SeekExpr, Tell, Terminated};
use crate::constructs::stream_ops::{ChecksumBytesFunc, ChecksumFunc, TransformFunc};
use crate::constructs::strings::{CString, PaddedString};
use crate::constructs::varint::{VarInt, ZigZag};
use crate::core::context::Context;
use crate::core::error::{ConstructError, Result};
use crate::core::stream::{ByteStream, CombinedStream, Stream};
use crate::core::Construct;
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

    /// Builds binary data, reading field values from `input` (an
    /// [`Input`] abstraction), writing bytes to `stream`, and updating `ctx`.
    ///
    /// # Errors
    ///
    /// Propagates any [`ConstructError`] from stream writing or value
    /// extraction.
    fn exec_build(
        &self,
        input: &dyn Input,
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

// -- Leaf constructors (embed concrete construct, Phase 12.4) --
// Each compiled leaf embeds the declaration-tree construct by value. At
// runtime, exec_parse / exec_build / exec_sizeof delegate directly to the
// inner construct's `Construct` impl (the old path). This is the
// embed-and-delegate pattern: zero per-call parameter extraction, but the
// same parse/build logic as the declaration tree.
/// Compiled node for `FormatField`.
#[derive(Debug)]
pub struct CompiledFormatField {
    /// The declaration-tree construct, embedded for direct delegation.
    pub inner: FormatField,
}
/// Compiled node for `VarInt`.
#[derive(Debug)]
pub struct CompiledVarInt {
    /// The declaration-tree construct, embedded for direct delegation.
    pub inner: VarInt,
}
/// Compiled node for `ZigZag`.
#[derive(Debug)]
pub struct CompiledZigZag {
    /// The declaration-tree construct, embedded for direct delegation.
    pub inner: ZigZag,
}
/// Compiled node for `Flag`.
#[derive(Debug)]
pub struct CompiledFlag {
    /// The declaration-tree construct, embedded for direct delegation.
    pub inner: Flag,
}
/// Compiled node for `Bytes`.
#[derive(Debug)]
pub struct CompiledBytes {
    /// The declaration-tree construct, embedded for direct delegation.
    pub inner: Bytes,
}
/// Compiled node for `GreedyBytes`.
#[derive(Debug)]
pub struct CompiledGreedyBytes {
    /// The declaration-tree construct, embedded for direct delegation.
    pub inner: GreedyBytes,
}
/// Compiled node for `BytesExpr`.
#[derive(Debug)]
pub struct CompiledBytesExpr {
    /// The declaration-tree construct, embedded for direct delegation.
    pub inner: BytesExpr,
}
/// Compiled node for `BytesInteger`.
#[derive(Debug)]
pub struct CompiledBytesInteger {
    /// The declaration-tree construct, embedded for direct delegation.
    pub inner: BytesInteger,
}
/// Compiled node for `BitsInteger`.
#[derive(Debug)]
pub struct CompiledBitsInteger {
    /// The declaration-tree construct, embedded for direct delegation.
    pub inner: BitsInteger,
}
/// Compiled node for `CString`.
#[derive(Debug)]
pub struct CompiledCString {
    /// The declaration-tree construct, embedded for direct delegation.
    pub inner: CString,
}
/// Compiled node for `PaddedString`.
#[derive(Debug)]
pub struct CompiledPaddedString {
    /// The declaration-tree construct, embedded for direct delegation.
    pub inner: PaddedString,
}

// -- Wrappers (hold compiled sub-node + wrapper data, Phase 12.5) --
// Each compiled wrapper recursively compiles its inner subcon into a
// Box<CompiledNode>, then stores wrapper-specific data (closures via Arc
// clone, mapping tables via clone). At runtime, exec delegates to the
// compiled inner node and applies the wrapper's transform/check/mapping logic.

/// Compiled node for `Const`.
#[derive(Debug)]
pub struct CompiledConst {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
    /// The expected constant value.
    pub value: Value,
}
/// Compiled node for `Renamed`.
#[derive(Debug)]
pub struct CompiledRenamed {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
}
/// Compiled node for `Subconstruct`.
#[derive(Debug)]
pub struct CompiledSubconstruct {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
}
/// Compiled node for `Adapter`.
pub struct CompiledAdapter {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
    /// Decode function applied after parsing the inner node.
    pub decode: DecodeFunc,
    /// Encode function applied before building the inner node.
    pub encode: EncodeFunc,
}
impl std::fmt::Debug for CompiledAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledAdapter")
            .field("inner", &self.inner)
            .field("decode", &"<closure>")
            .field("encode", &"<closure>")
            .finish()
    }
}
/// Compiled node for `SymmetricAdapter`.
pub struct CompiledSymmetricAdapter {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
    /// The symmetric function applied during both parse and build.
    pub func: SymmetricFunc,
}
impl std::fmt::Debug for CompiledSymmetricAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledSymmetricAdapter")
            .field("inner", &self.inner)
            .field("func", &"<closure>")
            .finish()
    }
}
/// Compiled node for `ExprAdapter`.
pub struct CompiledExprAdapter {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
    /// Decode function applied after parsing the inner node.
    pub decode: DecodeFunc,
    /// Encode function applied before building the inner node.
    pub encode: EncodeFunc,
}
impl std::fmt::Debug for CompiledExprAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledExprAdapter")
            .field("inner", &self.inner)
            .field("decode", &"<closure>")
            .field("encode", &"<closure>")
            .finish()
    }
}
/// Compiled node for `Validator`.
pub struct CompiledValidator {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
    /// The validation function. Returns `Ok(())` on success.
    pub check: CheckFunc,
}
impl std::fmt::Debug for CompiledValidator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledValidator")
            .field("inner", &self.inner)
            .field("check", &"<closure>")
            .finish()
    }
}
/// Compiled node for `ExprValidator`.
pub struct CompiledExprValidator {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
    /// The validation function. Returns `Ok(())` on success.
    pub check: CheckFunc,
}
impl std::fmt::Debug for CompiledExprValidator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledExprValidator")
            .field("inner", &self.inner)
            .field("check", &"<closure>")
            .finish()
    }
}
/// Compiled node for `Hex`.
#[derive(Debug)]
pub struct CompiledHex {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
}
/// Compiled node for `HexDump`.
#[derive(Debug)]
pub struct CompiledHexDump {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
}

// -- Meta constructors (leaf: embed concrete construct, Phase 12.4) --
/// Compiled node for `Pass`.
#[derive(Debug)]
pub struct CompiledPass {
    /// The declaration-tree construct, embedded for direct delegation.
    pub inner: Pass,
}
/// Compiled node for `Terminated`.
#[derive(Debug)]
pub struct CompiledTerminated {
    /// The declaration-tree construct, embedded for direct delegation.
    pub inner: Terminated,
}
/// Compiled node for `Tell`.
#[derive(Debug)]
pub struct CompiledTell {
    /// The declaration-tree construct, embedded for direct delegation.
    pub inner: Tell,
}
/// Compiled node for `Seek`.
#[derive(Debug)]
pub struct CompiledSeek {
    /// The declaration-tree construct, embedded for direct delegation.
    pub inner: Seek,
}
/// Compiled node for `SeekExpr`.
#[derive(Debug)]
pub struct CompiledSeekExpr {
    /// The declaration-tree construct, embedded for direct delegation.
    pub inner: SeekExpr,
}
/// Compiled node for `Error`.
#[derive(Debug)]
pub struct CompiledError {
    /// The declaration-tree construct, embedded for direct delegation.
    pub inner: Error,
}

// -- Composite constructors (Phase 12.6: real fields with sub-sink recursion) --
//
// Composite compiled nodes hold a `Vec<CompiledField>` (or equivalent) plus
// an optional focus target. At runtime, `exec_parse` creates a sub-sink for
// each field, delegates to the child's `exec_parse`, then integrates the
// child's result via `set_field` (for named fields) or discards it (for
// anonymous fields).

/// A single compiled field within a composite node (Struct / Union /
/// FocusedSeq).
///
/// Each field holds a recursively-compiled subcon and the field's name (if
/// any). `flagbuildnone` is pre-computed at compile time from the
/// declaration-tree subcon, avoiding a runtime virtual dispatch.
#[derive(Debug)]
pub struct CompiledField {
    /// The field name, or `None` for anonymous fields.
    pub name: Option<String>,
    /// The recursively compiled subcon.
    pub subcon: Box<CompiledNode>,
    /// Pre-computed `flagbuildnone` from the declaration-tree subcon.
    pub flagbuildnone: bool,
}

/// A single compiled entry within a `CompiledSequence`.
///
/// Entries are similar to [`CompiledField`] but do not track `flagbuildnone`
/// (Sequence's build always consumes list elements positionally).
#[derive(Debug)]
pub struct CompiledEntry {
    /// The entry name, or `None` for anonymous entries.
    pub name: Option<String>,
    /// The recursively compiled subcon.
    pub subcon: Box<CompiledNode>,
}

/// Compiled node for `Struct`.
#[derive(Debug)]
pub struct CompiledStruct {
    /// The ordered list of compiled fields.
    pub fields: Vec<CompiledField>,
}
/// Compiled node for `Sequence`.
#[derive(Debug)]
pub struct CompiledSequence {
    /// The ordered list of compiled entries.
    pub entries: Vec<CompiledEntry>,
}
/// Compiled node for `Union`.
#[derive(Debug)]
pub struct CompiledUnion {
    /// Which sub-construct determines the final stream position after parsing.
    pub parsefrom: Option<crate::constructs::union::UnionTarget>,
    /// The ordered list of compiled fields.
    pub fields: Vec<CompiledField>,
}
/// Compiled node for `Select`.
#[derive(Debug)]
pub struct CompiledSelect {
    /// The list of compiled subcons to try in order.
    pub subcons: Vec<CompiledNode>,
}
/// Compiled node for `FocusedSeq`.
#[derive(Debug)]
pub struct CompiledFocusedSeq {
    /// The name of the field to focus on (return from parse, use for build).
    pub parsebuildfrom: String,
    /// The ordered list of compiled fields.
    pub fields: Vec<CompiledField>,
}

// -- Enum / computed constructors (mapping tables in 12.8) --
// Enum, FlagsEnum, and Mapping are implemented in Phase 12.5; the rest
// remain stubs until Phase 12.8.
/// Compiled node for `Enum`.
#[derive(Debug)]
pub struct CompiledEnum {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
    /// Encoding map: label name → integer value.
    pub mapping: IndexMap<String, u64>,
    /// Decoding map: integer value → label name.
    pub decmap: IndexMap<u64, String>,
}
/// Compiled node for `FlagsEnum`.
#[derive(Debug)]
pub struct CompiledFlagsEnum {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
    /// Mapping of flag names to their integer bit values.
    pub flags: IndexMap<String, u64>,
}
/// Compiled node for `Mapping`.
#[derive(Debug)]
pub struct CompiledMapping {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
    /// Forward (encoding) pairs: (build key, build value passed to inner).
    pub mapping: Vec<(Value, Value)>,
    /// Reverse (decoding) pairs: (parsed value from inner, return value).
    pub decmapping: Vec<(Value, Value)>,
}
/// Compiled node for `Computed`.
pub struct CompiledComputed {
    /// The compute function: produces a value from context.
    pub func: ComputeFunc,
}
impl std::fmt::Debug for CompiledComputed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledComputed")
            .field("func", &"<closure>")
            .finish()
    }
}
/// Compiled node for `Rebuild`.
pub struct CompiledRebuild {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
    /// The compute function: produces the build value from context.
    pub func: ComputeFunc,
}
impl std::fmt::Debug for CompiledRebuild {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledRebuild")
            .field("inner", &self.inner)
            .field("func", &"<closure>")
            .finish()
    }
}
/// Compiled node for `Default`.
#[derive(Debug)]
pub struct CompiledDefault {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
    /// The default value used when the build input is `None`.
    pub value: Value,
}
/// Compiled node for `Index`.
#[derive(Debug)]
pub struct CompiledIndex;
/// Compiled node for `Padded`.
#[derive(Debug)]
pub struct CompiledPadded {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
    /// Total length in bytes (data + padding).
    pub length: usize,
    /// Padding pattern byte.
    pub pattern: u8,
    /// If `true`, padding bytes are validated during parse.
    pub strict: bool,
}
/// Compiled node for `Aligned`.
#[derive(Debug)]
pub struct CompiledAligned {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
    /// Alignment modulus.
    pub modulus: usize,
    /// Padding pattern byte.
    pub pattern: u8,
}
/// Compiled node for `FixedSized`.
#[derive(Debug)]
pub struct CompiledFixedSized {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
    /// Total size in bytes.
    pub length: usize,
}
/// Compiled node for `NamedTuple`.
#[derive(Debug)]
pub struct CompiledNamedTuple {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
    /// The field names.
    pub field_names: Vec<String>,
    /// The tuple type name (for display).
    pub tuple_name: String,
}
/// Compiled node for `TimestampAdapter`.
#[derive(Debug)]
pub struct CompiledTimestampAdapter {
    /// The recursively compiled inner integer subcon.
    pub inner: Box<CompiledNode>,
    /// The time unit of the integer value.
    pub unit: TimestampUnit,
    /// The epoch reference.
    pub epoch: TimestampEpoch,
}

// -- Repetition constructors (Phase 12.6) --
/// Compiled node for `Array`.
#[derive(Debug)]
pub struct CompiledArray {
    /// The exact number of elements to parse / build.
    pub count: usize,
    /// The recursively compiled subcon for each element.
    pub subcon: Box<CompiledNode>,
    /// If `true`, parse returns an empty list (elements are consumed but
    /// discarded).
    pub discard: bool,
}
/// Compiled node for `ArrayExpr`.
#[derive(Debug)]
pub struct CompiledArrayExpr {
    /// Pre-compiled count expression.
    pub compiled_count: CompiledExpr,
    /// The recursively compiled subcon for each element.
    pub subcon: Box<CompiledNode>,
}
/// Compiled node for `GreedyRange`.
#[derive(Debug)]
pub struct CompiledGreedyRange {
    /// The recursively compiled subcon for each element.
    pub subcon: Box<CompiledNode>,
    /// If `true`, parse returns an empty list.
    pub discard: bool,
}
/// Compiled node for `RepeatUntil`.
///
/// This remains a unit struct because `RepeatUntil` compiles to
/// [`CompiledDynamic`](super::CompiledDynamic) — the predicate closure
/// (`RepeatPredicate`) is a `Box<dyn Fn>` that cannot be cloned from `&self`
/// during compilation. After the `RepeatPredicate` Arc migration (B4-style),
/// this struct now holds the compiled predicate and recursively compiled
/// subcon.
pub struct CompiledRepeatUntil {
    /// The predicate that determines when to stop iterating.
    pub predicate: crate::constructs::repetition::RepeatPredicate,
    /// The recursively compiled subcon for each element.
    pub subcon: Box<CompiledNode>,
    /// If `true`, parse returns an empty list (elements consumed but discarded).
    pub discard: bool,
}
impl std::fmt::Debug for CompiledRepeatUntil {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledRepeatUntil")
            .field("predicate", &"<closure>")
            .field("subcon", &self.subcon)
            .field("discard", &self.discard)
            .finish()
    }
}

// -- Lazy constructors (Phase 12.8: eager delegation) --
/// Compiled node for `Lazy`.
#[derive(Debug)]
pub struct CompiledLazy {
    /// The recursively compiled inner subcon (eager delegation).
    pub inner: Box<CompiledNode>,
}
/// Compiled node for `LazyStruct`.
#[derive(Debug)]
pub struct CompiledLazyStruct {
    /// The recursively compiled inner (compiled as Struct).
    pub inner: Box<CompiledNode>,
}
/// Compiled node for `LazyArray`.
#[derive(Debug)]
pub struct CompiledLazyArray {
    /// The exact number of elements.
    pub count: usize,
    /// The recursively compiled subcon for each element.
    pub subcon: Box<CompiledNode>,
}
/// Compiled node for `Rebuffered`.
#[derive(Debug)]
pub struct CompiledRebuffered {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
}

// -- Stream ops / tunneling constructors (Phase 12.7) --
/// Compiled node for `Bitwise`.
#[derive(Debug)]
pub struct CompiledBitwise {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
}
/// Compiled node for `Bytewise`.
#[derive(Debug)]
pub struct CompiledBytewise {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
}
/// Compiled node for `Pointer`.
#[derive(Debug)]
pub struct CompiledPointer {
    /// The absolute position to seek to.
    pub offset: i64,
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
}
/// Compiled node for `PointerExpr`.
#[derive(Debug)]
pub struct CompiledPointerExpr {
    /// Pre-compiled offset expression.
    pub compiled_offset: CompiledExpr,
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
}
/// Compiled node for `Peek`.
#[derive(Debug)]
pub struct CompiledPeek {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
}
/// Compiled node for `RawCopy`.
#[derive(Debug)]
pub struct CompiledRawCopy {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
}
/// Compiled node for `Prefixed`.
#[derive(Debug)]
pub struct CompiledPrefixed {
    /// The recursively compiled length-field subcon.
    pub length_field: Box<CompiledNode>,
    /// The recursively compiled data subcon.
    pub subcon: Box<CompiledNode>,
    /// Whether the length field includes its own size.
    pub include_length: bool,
}
/// Compiled node for `Transformed`.
pub struct CompiledTransformed {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
    /// Decoding function applied before parsing subcon.
    pub decode: TransformFunc,
    /// Number of bytes to read before decoding, or `None` to read all.
    pub decode_amount: Option<usize>,
    /// Encoding function applied after building subcon.
    pub encode: TransformFunc,
    /// Expected number of bytes after encoding, or `None` to skip validation.
    pub encode_amount: Option<usize>,
}
impl std::fmt::Debug for CompiledTransformed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledTransformed")
            .field("inner", &self.inner)
            .field("decode", &"<closure>")
            .field("decode_amount", &self.decode_amount)
            .field("encode", &"<closure>")
            .field("encode_amount", &self.encode_amount)
            .finish()
    }
}
/// Compiled node for `Restreamed`.
pub struct CompiledRestreamed {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
    /// Chunk decoder function.
    pub decoder: TransformFunc,
    /// Size of chunks fed to the decoder.
    pub decoder_unit: usize,
    /// Chunk encoder function.
    pub encoder: TransformFunc,
    /// Size of chunks fed to the encoder.
    pub encoder_unit: usize,
    /// Function to compute the outer size from the inner size.
    pub size_computer: Option<Arc<dyn Fn(usize) -> usize + Send + Sync>>,
}
impl std::fmt::Debug for CompiledRestreamed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledRestreamed")
            .field("inner", &self.inner)
            .field("decoder", &"<closure>")
            .field("decoder_unit", &self.decoder_unit)
            .field("encoder", &"<closure>")
            .field("encoder_unit", &self.encoder_unit)
            .field("size_computer", &self.size_computer.is_some())
            .finish()
    }
}
/// Compiled node for `Compressed` (feature-gated).
#[cfg(feature = "compression")]
#[derive(Debug)]
pub struct CompiledCompressed {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
    /// The compression algorithm to use.
    pub algorithm: crate::constructs::stream_ops::CompressionAlgorithm,
}
/// Compiled node for `Checksum`.
pub struct CompiledChecksum {
    /// The recursively compiled checksum-field subcon.
    pub checksum_field: Box<CompiledNode>,
    /// Function that computes the checksum from raw bytes.
    pub hash_func: ChecksumFunc,
    /// Function that extracts the bytes to be checksummed from the context.
    pub bytes_func: ChecksumBytesFunc,
}
impl std::fmt::Debug for CompiledChecksum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledChecksum")
            .field("checksum_field", &self.checksum_field)
            .field("hash_func", &"<closure>")
            .field("bytes_func", &"<closure>")
            .finish()
    }
}
/// Compiled node for `ByteSwapped`.
#[derive(Debug)]
pub struct CompiledByteSwapped {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
}
/// Compiled node for `BitsSwapped`.
#[derive(Debug)]
pub struct CompiledBitsSwapped {
    /// The recursively compiled inner subcon.
    pub inner: Box<CompiledNode>,
    /// Whether the subcon has a fixed known size.
    pub fixed_size: bool,
}
/// Compiled node for `LazyBound` (I4: holds `Arc<dyn Construct>` for
/// runtime degradation to the old path).
pub struct CompiledLazyBound {
    /// The declaration-tree `LazyBound` wrapped behind an `Arc<dyn Construct>`.
    /// `exec_parse` / `exec_build` / `exec_sizeof` delegate directly to it.
    pub inner: Arc<dyn crate::core::Construct>,
}
impl std::fmt::Debug for CompiledLazyBound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledLazyBound")
            .field("inner", &"<LazyBound>")
            .finish()
    }
}

// -- Control flow constructors (Phase 12.7) --
/// Compiled node for `IfThenElse`.
pub struct CompiledIfThenElse {
    /// The condition function (shared via [`Arc`]).
    pub cond: CondFunc,
    /// The recursively compiled true-branch subcon.
    pub then_constr: Box<CompiledNode>,
    /// The recursively compiled false-branch subcon.
    pub else_constr: Box<CompiledNode>,
}
impl std::fmt::Debug for CompiledIfThenElse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledIfThenElse")
            .field("cond", &"<closure>")
            .field("then_constr", &self.then_constr)
            .field("else_constr", &self.else_constr)
            .finish()
    }
}
/// Compiled node for `Switch`.
pub struct CompiledSwitch {
    /// The key function (shared via [`Arc`]).
    pub keyfunc: KeyFunc,
    /// Ordered list of (key value, compiled subcon) pairs.
    pub cases: Vec<(Value, Box<CompiledNode>)>,
    /// Optional default compiled subcon.
    pub default: Option<Box<CompiledNode>>,
}
impl std::fmt::Debug for CompiledSwitch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledSwitch")
            .field("keyfunc", &"<closure>")
            .field("cases", &self.cases.len())
            .field("default", &self.default.is_some())
            .finish()
    }
}
/// Compiled node for `Check`.
pub struct CompiledCheck {
    /// The check function (shared via [`Arc`]).
    pub check: ControlCheckFunc,
}
impl std::fmt::Debug for CompiledCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledCheck")
            .field("check", &"<closure>")
            .finish()
    }
}
/// Compiled node for `StopIf`.
pub struct CompiledStopIf {
    /// The condition function (shared via [`Arc`]).
    pub cond: StopCondFunc,
}
impl std::fmt::Debug for CompiledStopIf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledStopIf")
            .field("cond", &"<closure>")
            .finish()
    }
}

// -- Escape-hatch --
/// Compiled node for `Dynamic` — holds an `Arc<dyn Construct>` that falls
/// back to the declaration-tree old path at runtime.
///
/// This is the escape-hatch for constructs that cannot be compiled (e.g.
/// user-defined constructs, Python callables in Phase 13). The inner
/// `Arc<dyn Construct>` is called directly at runtime, bypassing the
/// compiled tree.
///
/// # I3 correction
///
/// The inner field is `Arc<dyn Construct>` (not `Box`), mirroring the
/// [`CombinedConstruct::Dynamic`](crate::combined::CombinedConstruct::Dynamic)
/// variant after the I3 Box→Arc migration. `BuildConstruct::compile` takes
/// `&self`, so it uses `Arc::clone` to hand the inner construct to this
/// node without moving.
pub struct CompiledDynamic {
    /// The declaration-tree construct wrapped behind an `Arc<dyn Construct>`.
    /// `exec_parse` / `exec_build` / `exec_sizeof` delegate directly to it
    /// (old-path fallback).
    pub inner: Arc<dyn crate::core::Construct>,
}

impl std::fmt::Debug for CompiledDynamic {
    // `dyn Construct` is not `Debug`, so a manual impl is required. We report
    // the type name opaquely — the inner construct's internals are not
    // introspectable behind the trait object.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledDynamic")
            .field("inner", &"<dyn Construct>")
            .finish()
    }
}

// ===========================================================================
// CompiledExtension trait + CompiledExternal (Phase 13)
// ===========================================================================

/// Trait for external (non-built-in) compiled constructs.
///
/// This is the extension point for Phase 13's FFI layer: the
/// [`CompiledNode::External`] variant wraps a `Box<dyn CompiledExtension>`,
/// allowing construct-py to inject Python-callback-backed constructs into the
/// compiled execution tree **without** construct-rs depending on pyo3.
///
/// The trait mirrors [`CompiledExec`] but operates through trait-object
/// dispatch rather than `enum_dispatch`. The `ext_parse` method returns a
/// [`ProducedOutput`] (allowing direct-to-Python output), while `ext_build`
/// accepts an [`Input`] (allowing lazy Python-side reading).
pub trait CompiledExtension: Send + Sync {
    /// Parses from the stream and returns the produced output.
    ///
    /// # Errors
    ///
    /// Propagates any [`ConstructError`] from stream reading or parsing.
    fn ext_parse(&self, stream: &mut CombinedStream, ctx: &mut Context) -> Result<ProducedOutput>;

    /// Builds binary data from the input, writing to the stream.
    ///
    /// # Errors
    ///
    /// Propagates any [`ConstructError`] from value extraction or stream
    /// writing.
    fn ext_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()>;

    /// Computes the byte size.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::Sizeof`] if the size is not statically
    /// determinable.
    fn ext_sizeof(&self) -> Result<usize>;
}

/// Compiled node wrapping an external [`CompiledExtension`] trait object.
///
/// This is the Phase 13 extension point: it allows construct-py to insert
/// Python-callback-backed constructs into the compiled tree. The
/// [`CompiledExec`] implementation delegates to the inner
/// [`CompiledExtension`] trait object.
///
/// # `ext_parse` → sink integration
///
/// `exec_parse` calls `ext_parse` which returns a [`ProducedOutput`]. For
/// [`ProducedOutput::Value`], the value is deposited into the sink via
/// `set_scalar`. For [`ProducedOutput::Object`], the opaque boxed object is
/// stored internally and surfaced when the parent sink calls
/// [`OutputSink::into_produced`] (Phase 13 PyDictSink path).
pub struct CompiledExternal {
    /// The external extension trait object.
    pub inner: Box<dyn CompiledExtension>,
}

impl std::fmt::Debug for CompiledExternal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledExternal")
            .field("inner", &"<dyn CompiledExtension>")
            .finish()
    }
}

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
    // -- Phase 13 extension --
    /// Wraps [`CompiledExternal`] — an external (e.g. Python-callback-backed)
    /// compiled construct injected via the [`CompiledExtension`] trait.
    External(CompiledExternal),
}

// ===========================================================================
// Stub CompiledExec implementations
// ===========================================================================
//
// All compiled node types implement CompiledExec with stub implementations
// that return a descriptive error. Real implementations are added in
// sub-tasks 12.4-12.9.

/// Error message for stub CompiledExec implementations.
#[allow(dead_code)]
const STUB_EXEC_MESSAGE: &str =
    "CompiledExec not yet implemented (Phase 12.1 stub — real impl in 12.4-12.9)";

/// Builds a stub error for CompiledExec methods.
#[allow(dead_code)]
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
                    _input: &dyn Input,
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
    // All types now have real implementations (RepeatUntil migrated in 12.9).
}

// ===========================================================================
// Functional CompiledExec for leaf nodes (Phase 12.4)
// ===========================================================================
//
// Leaf compiled nodes embed their declaration-tree construct by value (the
// embed-and-delegate pattern). At runtime, every operation delegates directly
// to the inner construct's `Construct` impl — the same parse/build/sizeof
// logic as the declaration tree (the "old path"), but with zero per-call
// parameter extraction. exec_parse deposits the parsed scalar via
// `set_scalar` (uniform leaf protocol, I1).

/// Macro to generate functional [`CompiledExec`] impls for leaf compiled
/// nodes that embed a concrete `Construct` in their `inner` field.
macro_rules! impl_leaf_exec {
    ($($ty:ty),* $(,)?) => {
        $(
            impl CompiledExec for $ty {
                fn exec_parse(
                    &self,
                    stream: &mut CombinedStream,
                    ctx: &mut Context,
                    sink: &mut dyn OutputSink,
                ) -> Result<()> {
                    let value = self.inner.parse(stream, ctx)?;
                    sink.set_scalar(value)
                }

                fn exec_build(
                    &self,
                    input: &dyn Input,
                    stream: &mut CombinedStream,
                    ctx: &mut Context,
                ) -> Result<()> {
                    let value = input.as_value()?;
                    self.inner.build(&value, stream, ctx)
                }

                fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
                    self.inner.sizeof(ctx)
                }
            }
        )*
    };
}

impl_leaf_exec! {
    // atomic leaves
    CompiledFormatField, CompiledBytes, CompiledGreedyBytes, CompiledBytesExpr,
    CompiledBytesInteger, CompiledBitsInteger, CompiledVarInt, CompiledZigZag,
    CompiledFlag, CompiledCString, CompiledPaddedString,
    // meta leaves
    CompiledPass, CompiledTerminated, CompiledTell, CompiledSeek,
    CompiledSeekExpr, CompiledError,
}

// ===========================================================================
// Functional CompiledExec for wrapper nodes (Phase 12.5)
// ===========================================================================
//
// Wrapper compiled nodes hold a recursively-compiled inner `Box<CompiledNode>`
// plus wrapper-specific data (Arc closures, cloned mapping tables, or a cloned
// Value). At runtime, exec delegates to the compiled inner node and applies
// the wrapper's transform / check / mapping logic.

/// Helper: executes `exec_parse` on the inner compiled node and returns the
/// resulting [`Value`] via a temporary [`ValueSink`].
///
/// Used by wrapper nodes that need to inspect the inner parse result before
/// applying their own transform (e.g. Adapter's decode, Validator's check).
fn exec_parse_inner_value(
    inner: &CompiledNode,
    stream: &mut CombinedStream,
    ctx: &mut Context,
) -> Result<Value> {
    let mut sub = ValueSink::new();
    inner.exec_parse(stream, ctx, &mut sub)?;
    Box::new(sub).into_value()
}

/// Macro to generate functional [`CompiledExec`] impls for **pure-forward**
/// wrapper nodes that delegate every operation directly to their inner
/// compiled node with no transform.
macro_rules! impl_forward_wrapper_exec {
    ($($ty:ty),* $(,)?) => {
        $(
            impl CompiledExec for $ty {
                fn exec_parse(
                    &self,
                    stream: &mut CombinedStream,
                    ctx: &mut Context,
                    sink: &mut dyn OutputSink,
                ) -> Result<()> {
                    let value = exec_parse_inner_value(&self.inner, stream, ctx)?;
                    sink.set_scalar(value)
                }

                fn exec_build(
                    &self,
                    input: &dyn Input,
                    stream: &mut CombinedStream,
                    ctx: &mut Context,
                ) -> Result<()> {
                    self.inner.exec_build(input, stream, ctx)
                }

                fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
                    self.inner.exec_sizeof(ctx)
                }
            }
        )*
    };
}

impl_forward_wrapper_exec! {
    CompiledSubconstruct, CompiledRenamed, CompiledHex, CompiledHexDump,
}

// -- Const ----------------------------------------------------------------

impl CompiledExec for CompiledConst {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let obj = exec_parse_inner_value(&self.inner, stream, ctx)?;
        if obj != self.value {
            return Err(ConstructError::Const {
                path: String::new(),
                expected: format!("{:?}", self.value),
                actual: format!("{:?}", obj),
            });
        }
        sink.set_scalar(obj)
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let value = input.as_value()?;
        if !value.is_none() && value != self.value {
            return Err(ConstructError::Const {
                path: String::new(),
                expected: format!("None or {:?}", self.value),
                actual: format!("{:?}", value),
            });
        }
        let owned = OwnedValueInput::new(self.value.clone());
        self.inner.exec_build(&owned, stream, ctx)
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        self.inner.exec_sizeof(ctx)
    }
}

// -- Adapter / ExprAdapter (decode then deposit / encode then delegate) ---

/// Macro to generate [`CompiledExec`] for adapter-style wrappers that hold
/// separate `decode` and `encode` closures.
macro_rules! impl_adapter_exec {
    ($($ty:ty),* $(,)?) => {
        $(
            impl CompiledExec for $ty {
                fn exec_parse(
                    &self,
                    stream: &mut CombinedStream,
                    ctx: &mut Context,
                    sink: &mut dyn OutputSink,
                ) -> Result<()> {
                    let raw = exec_parse_inner_value(&self.inner, stream, ctx)?;
                    let decoded = (self.decode)(&raw, ctx)?;
                    sink.set_scalar(decoded)
                }

                fn exec_build(
                    &self,
                    input: &dyn Input,
                    stream: &mut CombinedStream,
                    ctx: &mut Context,
                ) -> Result<()> {
                    let value = input.as_value()?;
                    let encoded = (self.encode)(&value, ctx)?;
                    let owned = OwnedValueInput::new(encoded);
                    self.inner.exec_build(&owned, stream, ctx)
                }

                fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
                    self.inner.exec_sizeof(ctx)
                }
            }
        )*
    };
}

impl_adapter_exec!(CompiledAdapter, CompiledExprAdapter,);

// -- SymmetricAdapter (single func for both directions) -------------------

impl CompiledExec for CompiledSymmetricAdapter {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let raw = exec_parse_inner_value(&self.inner, stream, ctx)?;
        let decoded = (self.func)(&raw, ctx)?;
        sink.set_scalar(decoded)
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let value = input.as_value()?;
        let encoded = (self.func)(&value, ctx)?;
        let owned = OwnedValueInput::new(encoded);
        self.inner.exec_build(&owned, stream, ctx)
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        self.inner.exec_sizeof(ctx)
    }
}

// -- Validator / ExprValidator (check then pass-through) ------------------

/// Macro to generate [`CompiledExec`] for validator-style wrappers that hold
/// a `check` closure. The value passes through unchanged when validation
/// succeeds.
macro_rules! impl_validator_exec {
    ($($ty:ty),* $(,)?) => {
        $(
            impl CompiledExec for $ty {
                fn exec_parse(
                    &self,
                    stream: &mut CombinedStream,
                    ctx: &mut Context,
                    sink: &mut dyn OutputSink,
                ) -> Result<()> {
                    let raw = exec_parse_inner_value(&self.inner, stream, ctx)?;
                    (self.check)(&raw, ctx)?;
                    sink.set_scalar(raw)
                }

                fn exec_build(
                    &self,
                    input: &dyn Input,
                    stream: &mut CombinedStream,
                    ctx: &mut Context,
                ) -> Result<()> {
                    let value = input.as_value()?;
                    (self.check)(&value, ctx)?;
                    self.inner.exec_build(input, stream, ctx)
                }

                fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
                    self.inner.exec_sizeof(ctx)
                }
            }
        )*
    };
}

impl_validator_exec!(CompiledValidator, CompiledExprValidator,);

// -- Enum (integer ↔ string mapping) --------------------------------------

impl CompiledExec for CompiledEnum {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let obj = exec_parse_inner_value(&self.inner, stream, ctx)?;
        let int_val = obj.to_u64().map_err(|e| e.with_path_prefix("Enum"))?;
        match self.decmap.get(&int_val) {
            Some(label) => sink.set_scalar(Value::String(label.clone())),
            None => sink.set_scalar(obj),
        }
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let raw = input.as_value()?;
        let build_val = match &raw {
            Value::String(label) => match self.mapping.get(label) {
                Some(&v) => Value::UInt(v),
                None => {
                    return Err(ConstructError::Mapping {
                        path: String::new(),
                        key: format!("{:?}", label),
                    }
                    .with_path_prefix("Enum"))
                }
            },
            Value::Int(i) => Value::Int(*i),
            other => {
                let v = other.to_u64().map_err(|e| e.with_path_prefix("Enum"))?;
                Value::UInt(v)
            }
        };
        let owned = OwnedValueInput::new(build_val);
        self.inner
            .exec_build(&owned, stream, ctx)
            .map_err(|e| e.with_path_prefix("Enum"))
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        self.inner
            .exec_sizeof(ctx)
            .map_err(|e| e.with_path_prefix("Enum"))
    }
}

// -- FlagsEnum (bit-flag expansion) ---------------------------------------

impl CompiledExec for CompiledFlagsEnum {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let obj = exec_parse_inner_value(&self.inner, stream, ctx)?;
        let int_val = obj.to_u64().map_err(|e| e.with_path_prefix("FlagsEnum"))?;
        let mut container = IndexMap::new();
        for (name, &flag_value) in &self.flags {
            let set = (int_val & flag_value) == flag_value;
            container.insert(name.clone(), Value::Bool(set));
        }
        sink.set_scalar(Value::Container(container))
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let raw = input.as_value()?;
        let container = raw
            .as_container()
            .map_err(|e| e.with_path_prefix("FlagsEnum"))?;
        let mut flags_val: u64 = 0;
        for (name, value) in container {
            if name.starts_with('_') {
                continue;
            }
            match self.flags.get(name) {
                Some(&flag_bits) => {
                    let is_set = value
                        .as_bool()
                        .map_err(|e| e.with_path_prefix("FlagsEnum"))?;
                    if is_set {
                        flags_val |= flag_bits;
                    }
                }
                None => {
                    return Err(ConstructError::Mapping {
                        path: String::new(),
                        key: format!("{:?}", name),
                    }
                    .with_path_prefix("FlagsEnum"))
                }
            }
        }
        let owned = OwnedValueInput::new(Value::UInt(flags_val));
        self.inner
            .exec_build(&owned, stream, ctx)
            .map_err(|e| e.with_path_prefix("FlagsEnum"))
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        self.inner
            .exec_sizeof(ctx)
            .map_err(|e| e.with_path_prefix("FlagsEnum"))
    }
}

// -- Mapping (arbitrary Value ↔ Value mapping) ----------------------------

impl CompiledExec for CompiledMapping {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let obj = exec_parse_inner_value(&self.inner, stream, ctx)?;
        for (k, v) in &self.decmapping {
            if k == &obj {
                return sink.set_scalar(v.clone());
            }
        }
        Err(ConstructError::Mapping {
            path: String::new(),
            key: format!("{:?}", obj),
        }
        .with_path_prefix("Mapping"))
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let raw = input.as_value()?;
        for (k, v) in &self.mapping {
            if k == &raw {
                let owned = OwnedValueInput::new(v.clone());
                return self
                    .inner
                    .exec_build(&owned, stream, ctx)
                    .map_err(|e| e.with_path_prefix("Mapping"));
            }
        }
        Err(ConstructError::Mapping {
            path: String::new(),
            key: format!("{:?}", raw),
        }
        .with_path_prefix("Mapping"))
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        self.inner
            .exec_sizeof(ctx)
            .map_err(|e| e.with_path_prefix("Mapping"))
    }
}

// ===========================================================================
// Functional CompiledExec for composite constructors (Phase 12.6)
// ===========================================================================
//
// Composite compiled nodes (Struct, Sequence, Union, Select, FocusedSeq,
// Array, ArrayExpr, GreedyRange) hold a `Vec` of compiled children. At
// runtime, `exec_parse` coordinates the `OutputSink` sub-sink protocol:
// a sub-sink is created for each child, the child's `exec_parse` writes
// into it, and the parent integrates the result (`set_field` for named
// fields, `push_item` for list entries, discard for anonymous). `exec_build`
// reads from the input `Value` tree and delegates to each child's
// `exec_build`.
//
// RepeatUntil compiles to `CompiledDynamic` (its `RepeatPredicate` closure
// cannot be cloned from `&self`); see `build.rs`.

/// Context key for the current loop index during repetition constructs.
///
/// Mirrors `constructs::repetition::CONTEXT_INDEX_KEY`.
const REPETITION_INDEX_KEY: &str = "_index";

/// Helper: parses a single child node into a named sub-sink and integrates
/// the result into the parent sink via `finish_field`, returning the child's
/// parsed [`Value`] for context insertion.
///
/// Creates a sub-sink via `parent_sink.sub_sink_for_field(name)`, delegates
/// `exec_parse` to `node` with the sub-sink, then calls
/// [`OutputSink::finish_field`] which integrates the result (via `set_field`
/// for ValueSink) and returns the [`Value`].
///
/// **Used by**: `CompiledStruct`, `CompiledUnion` — named-keyed container
/// constructs. **Not** used by `CompiledFocusedSeq` (which uses
/// [`exec_parse_field_value`] instead, since it only needs the Value for
/// context, not sink integration).
fn exec_parse_named_child(
    node: &CompiledNode,
    stream: &mut CombinedStream,
    ctx: &mut Context,
    parent_sink: &mut dyn OutputSink,
    name: &str,
) -> Result<Value> {
    let mut sub_sink = parent_sink.sub_sink_for_field(name)?;
    node.exec_parse(stream, ctx, sub_sink.as_mut())?;
    parent_sink.finish_field(name, sub_sink)
}

/// Helper: parses a single child node into a named sub-sink and extracts the
/// result as a [`Value`] **without** integrating it into the parent sink.
///
/// This is the Phase 12 behavior of `exec_parse_named_child` (before the
/// Phase 13 finish_field migration). Used by `CompiledFocusedSeq` which only
/// needs the Value for context insertion — the focused value is deposited via
/// `set_scalar` at the end, not via per-field sink integration.
fn exec_parse_field_value(
    node: &CompiledNode,
    stream: &mut CombinedStream,
    ctx: &mut Context,
    parent_sink: &mut dyn OutputSink,
    name: &str,
) -> Result<Value> {
    let mut sub_sink = parent_sink.sub_sink_for_field(name)?;
    node.exec_parse(stream, ctx, sub_sink.as_mut())?;
    sub_sink.into_value()
}

// -- CompiledStruct -------------------------------------------------------

impl CompiledExec for CompiledStruct {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let mut child_ctx = ctx.subcontext();

        for field in &self.fields {
            match &field.name {
                Some(name) => {
                    let value = match exec_parse_named_child(
                        &field.subcon,
                        stream,
                        &mut child_ctx,
                        sink,
                        name,
                    ) {
                        Ok(v) => v,
                        Err(ConstructError::StopField { .. }) => break,
                        Err(e) => return Err(e.with_path_prefix(name)),
                    };
                    // sink already integrated by finish_field inside helper.
                    child_ctx.insert(name.clone(), value);
                }
                None => {
                    // Anonymous field: parse into a sub-sink, finish_item.
                    let mut sub_sink = sink.sub_sink_for_item()?;
                    match field
                        .subcon
                        .exec_parse(stream, &mut child_ctx, sub_sink.as_mut())
                    {
                        Ok(()) => {
                            sink.finish_item(sub_sink)?;
                        }
                        Err(ConstructError::StopField { .. }) => break,
                        Err(e) => return Err(e.with_path_prefix("(anonymous)")),
                    }
                }
            }
        }
        Ok(())
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        // Extract the container from input for field lookups.
        let raw = input.as_value()?;
        let container = match &raw {
            Value::None => IndexMap::new(),
            Value::Container(map) => map.clone(),
            other => {
                return Err(ConstructError::TypeMismatch {
                    path: String::new(),
                    expected: "Container".to_string(),
                    actual: other.type_name().to_string(),
                });
            }
        };

        let mut child_ctx = ctx.subcontext();
        for (key, value) in &container {
            child_ctx.insert(key.clone(), value.clone());
        }

        for field in &self.fields {
            let name_ref = field.name.as_deref();

            let build_value =
                if field.flagbuildnone {
                    name_ref
                        .and_then(|n| container.get(n))
                        .cloned()
                        .unwrap_or(Value::None)
                } else {
                    match name_ref {
                        Some(n) => container.get(n).cloned().ok_or_else(|| {
                            ConstructError::FieldMissing {
                                path: String::new(),
                                field: n.to_string(),
                            }
                        })?,
                        None => Value::None,
                    }
                };

            if let Some(ref name) = field.name {
                child_ctx.insert(name.clone(), build_value.clone());
            }

            let field_input = OwnedValueInput::new(build_value);
            match field
                .subcon
                .exec_build(&field_input, stream, &mut child_ctx)
            {
                Ok(()) => {}
                Err(ConstructError::StopField { .. }) => return Ok(()),
                Err(e) => {
                    return Err(if let Some(ref name) = field.name {
                        e.with_path_prefix(name)
                    } else {
                        e.with_path_prefix("(anonymous)")
                    });
                }
            }
        }
        Ok(())
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        let child_ctx = ctx.subcontext();
        let mut total: usize = 0;
        for field in &self.fields {
            let size = match field.subcon.exec_sizeof(&child_ctx) {
                Ok(s) => s,
                Err(e) => {
                    return Err(if let Some(ref name) = field.name {
                        e.with_path_prefix(name)
                    } else {
                        e.with_path_prefix("(anonymous)")
                    });
                }
            };
            total = total.saturating_add(size);
        }
        Ok(total)
    }
}

// -- CompiledSequence -----------------------------------------------------

impl CompiledExec for CompiledSequence {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let mut child_ctx = ctx.subcontext();

        for (idx, entry) in self.entries.iter().enumerate() {
            match &entry.name {
                Some(name) => {
                    // Named entry: positionally appended via finish_named_item.
                    let mut sub_sink = sink.sub_sink_for_item()?;
                    match entry
                        .subcon
                        .exec_parse(stream, &mut child_ctx, sub_sink.as_mut())
                    {
                        Ok(()) => {}
                        Err(ConstructError::StopField { .. }) => break,
                        Err(e) => return Err(e.with_path_prefix(name)),
                    }
                    let value = sink.finish_named_item(name, sub_sink)?;
                    child_ctx.insert(name.clone(), value);
                }
                None => {
                    let mut sub_sink = sink.sub_sink_for_item()?;
                    match entry
                        .subcon
                        .exec_parse(stream, &mut child_ctx, sub_sink.as_mut())
                    {
                        Ok(()) => {
                            sink.finish_item(sub_sink)?;
                        }
                        Err(ConstructError::StopField { .. }) => break,
                        Err(e) => {
                            return Err(e.with_path_prefix(&format!("[{idx}]")));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let raw = input.as_value()?;
        let list = match &raw {
            Value::None => vec![Value::None; self.entries.len()],
            Value::List(items) => items.clone(),
            other => {
                return Err(ConstructError::TypeMismatch {
                    path: String::new(),
                    expected: "List".to_string(),
                    actual: other.type_name().to_string(),
                });
            }
        };

        if list.len() < self.entries.len() {
            return Err(ConstructError::Array {
                path: String::new(),
                expected: self.entries.len(),
                actual: list.len(),
            });
        }

        let mut child_ctx = ctx.subcontext();

        for (i, entry) in self.entries.iter().enumerate() {
            let build_value = list[i].clone();

            if let Some(ref name) = entry.name {
                child_ctx.insert(name.clone(), build_value.clone());
            }

            let entry_input = OwnedValueInput::new(build_value);
            match entry
                .subcon
                .exec_build(&entry_input, stream, &mut child_ctx)
            {
                Ok(()) => {}
                Err(ConstructError::StopField { .. }) => return Ok(()),
                Err(e) => {
                    return Err(if let Some(ref name) = entry.name {
                        e.with_path_prefix(name)
                    } else {
                        e.with_path_prefix(&format!("[{i}]"))
                    });
                }
            }
        }
        Ok(())
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        let child_ctx = ctx.subcontext();
        let mut total: usize = 0;
        for entry in &self.entries {
            let size = match entry.subcon.exec_sizeof(&child_ctx) {
                Ok(s) => s,
                Err(e) => {
                    return Err(if let Some(ref name) = entry.name {
                        e.with_path_prefix(name)
                    } else {
                        e.with_path_prefix("(anonymous)")
                    });
                }
            };
            total = total.saturating_add(size);
        }
        Ok(total)
    }
}

// -- CompiledArray --------------------------------------------------------

impl CompiledExec for CompiledArray {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        for i in 0..self.count {
            ctx.insert(REPETITION_INDEX_KEY, Value::UInt(i as u64));
            let mut sub_sink = sink.sub_sink_for_item()?;
            match self.subcon.exec_parse(stream, ctx, sub_sink.as_mut()) {
                Ok(()) => {
                    if !self.discard {
                        sink.finish_item(sub_sink)?;
                    }
                }
                Err(e) => return Err(e.with_path_prefix(&format!("[{i}]"))),
            }
        }
        Ok(())
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let raw = input.as_value()?;
        let list = match &raw {
            Value::List(items) => items.clone(),
            other => {
                return Err(ConstructError::TypeMismatch {
                    path: String::new(),
                    expected: "List".to_string(),
                    actual: other.type_name().to_string(),
                });
            }
        };

        if list.len() != self.count {
            return Err(ConstructError::Array {
                path: String::new(),
                expected: self.count,
                actual: list.len(),
            });
        }

        for (i, element) in list.iter().enumerate() {
            ctx.insert(REPETITION_INDEX_KEY, Value::UInt(i as u64));
            let elem_input = OwnedValueInput::new(element.clone());
            if let Err(e) = self.subcon.exec_build(&elem_input, stream, ctx) {
                return Err(e.with_path_prefix(&format!("[{i}]")));
            }
        }
        Ok(())
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        let sub_size = self.subcon.exec_sizeof(ctx)?;
        Ok(self.count.saturating_mul(sub_size))
    }
}

// -- CompiledArrayExpr ----------------------------------------------------

impl CompiledExec for CompiledArrayExpr {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let count_val = self
            .compiled_count
            .eval(ctx, None)
            .map_err(|e| e.with_path_prefix("ArrayExpr"))?;
        let count = count_val.to_u64().map_err(|e| {
            ConstructError::Expr {
                path: String::new(),
                message: format!("ArrayExpr count must be a non-negative integer: {e}"),
            }
            .with_path_prefix("ArrayExpr")
        })? as usize;

        for i in 0..count {
            ctx.insert(REPETITION_INDEX_KEY, Value::UInt(i as u64));
            let mut sub_sink = sink.sub_sink_for_item()?;
            match self.subcon.exec_parse(stream, ctx, sub_sink.as_mut()) {
                Ok(()) => {
                    sink.finish_item(sub_sink)?;
                }
                Err(e) => return Err(e.with_path_prefix(&format!("[{i}]"))),
            }
        }
        Ok(())
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let expected_val = self
            .compiled_count
            .eval(ctx, None)
            .map_err(|e| e.with_path_prefix("ArrayExpr"))?;
        let expected = expected_val.to_u64().map_err(|e| {
            ConstructError::Expr {
                path: String::new(),
                message: format!("ArrayExpr count must be a non-negative integer: {e}"),
            }
            .with_path_prefix("ArrayExpr")
        })? as usize;

        let raw = input.as_value()?;
        let list = match &raw {
            Value::List(items) => items.clone(),
            other => {
                return Err(ConstructError::TypeMismatch {
                    path: String::new(),
                    expected: "List".to_string(),
                    actual: other.type_name().to_string(),
                });
            }
        };

        if list.len() != expected {
            return Err(ConstructError::Array {
                path: String::new(),
                expected,
                actual: list.len(),
            });
        }

        for (i, element) in list.iter().enumerate() {
            ctx.insert(REPETITION_INDEX_KEY, Value::UInt(i as u64));
            let elem_input = OwnedValueInput::new(element.clone());
            if let Err(e) = self.subcon.exec_build(&elem_input, stream, ctx) {
                return Err(e.with_path_prefix(&format!("[{i}]")));
            }
        }
        Ok(())
    }

    fn exec_sizeof(&self, _ctx: &Context) -> Result<usize> {
        Err(ConstructError::Sizeof {
            path: String::new(),
            reason: "ArrayExpr has variable size (count is runtime-dependent)".to_string(),
        })
    }
}

// -- CompiledGreedyRange --------------------------------------------------

impl CompiledExec for CompiledGreedyRange {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let mut i: u64 = 0;
        loop {
            ctx.insert(REPETITION_INDEX_KEY, Value::UInt(i));
            let fallback = stream.tell()?;
            let mut sub_sink = sink.sub_sink_for_item()?;
            match self.subcon.exec_parse(stream, ctx, sub_sink.as_mut()) {
                Ok(()) => {
                    if !self.discard {
                        sink.finish_item(sub_sink)?;
                    }
                    i = i.saturating_add(1);
                }
                Err(ConstructError::StopField { .. }) => break,
                Err(_) => {
                    // Any other error: seek back and stop.
                    stream.seek(fallback)?;
                    break;
                }
            }
        }
        Ok(())
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let raw = input.as_value()?;
        let list = match &raw {
            Value::List(items) => items.clone(),
            other => {
                return Err(ConstructError::TypeMismatch {
                    path: String::new(),
                    expected: "List".to_string(),
                    actual: other.type_name().to_string(),
                });
            }
        };

        for (i, element) in list.iter().enumerate() {
            ctx.insert(REPETITION_INDEX_KEY, Value::UInt(i as u64));
            let elem_input = OwnedValueInput::new(element.clone());
            if let Err(e) = self.subcon.exec_build(&elem_input, stream, ctx) {
                return Err(e.with_path_prefix(&format!("[{i}]")));
            }
        }
        Ok(())
    }

    fn exec_sizeof(&self, _ctx: &Context) -> Result<usize> {
        Err(ConstructError::Sizeof {
            path: String::new(),
            reason: "GreedyRange has undefined size".to_string(),
        })
    }
}

// -- CompiledRepeatUntil (Phase 12.9: migrated from stub) --------------------

impl CompiledExec for CompiledRepeatUntil {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let mut i: u64 = 0;
        // Local list for predicate evaluation (mirrors declaration-tree behavior).
        let mut local_list: Vec<Value> = Vec::new();
        loop {
            ctx.insert(REPETITION_INDEX_KEY, Value::UInt(i));
            let mut sub_sink = sink.sub_sink_for_item()?;
            self.subcon
                .exec_parse(stream, ctx, sub_sink.as_mut())
                .map_err(|e| e.with_path_prefix(&format!("[{i}]")))?;
            let element = sub_sink.into_value()?;

            if !self.discard {
                local_list.push(element.clone());
                sink.push_item(element.clone())?;
            }

            if (self.predicate)(&element, &local_list, ctx) {
                return Ok(());
            }
            i = i.saturating_add(1);
        }
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let raw = input.as_value()?;
        let list = match &raw {
            Value::List(items) => items.clone(),
            other => {
                return Err(ConstructError::TypeMismatch {
                    path: String::new(),
                    expected: "List".to_string(),
                    actual: other.type_name().to_string(),
                });
            }
        };

        let mut built: Vec<Value> = Vec::new();
        for (i, element) in list.iter().enumerate() {
            ctx.insert(REPETITION_INDEX_KEY, Value::UInt(i as u64));
            let elem_input = OwnedValueInput::new(element.clone());
            self.subcon
                .exec_build(&elem_input, stream, ctx)
                .map_err(|e| e.with_path_prefix(&format!("[{i}]")))?;

            if !self.discard {
                built.push(element.clone());
            }

            if (self.predicate)(element, &built, ctx) {
                return Ok(());
            }
        }

        // No element matched the predicate.
        Err(ConstructError::Array {
            path: String::new(),
            expected: 0,
            actual: 0,
        })
    }

    fn exec_sizeof(&self, _ctx: &Context) -> Result<usize> {
        Err(ConstructError::Sizeof {
            path: String::new(),
            reason: "RepeatUntil has undefined size, amount depends on actual data".to_string(),
        })
    }
}

// -- CompiledSelect -------------------------------------------------------

impl CompiledExec for CompiledSelect {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let fallback = stream.tell()?;
        for sc in &self.subcons {
            let mut sub_sink = sink.sub_sink_for_item()?;
            match sc.exec_parse(stream, ctx, sub_sink.as_mut()) {
                Ok(()) => {
                    let value = sub_sink.into_value()?;
                    sink.set_scalar(value)?;
                    return Ok(());
                }
                Err(ConstructError::StopField { .. }) => {
                    return Err(ConstructError::StopField {
                        path: String::new(),
                    });
                }
                Err(_) => {
                    stream.seek(fallback)?;
                }
            }
        }
        Err(ConstructError::Select {
            path: String::new(),
            message: format!(
                "no subconstruct matched after trying {} candidates",
                self.subcons.len()
            ),
        })
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        for sc in &self.subcons {
            let mut build_stream = CombinedStream::ByteStream(ByteStream::new_write());
            match sc.exec_build(input, &mut build_stream, ctx) {
                Ok(()) => {
                    let bytes = build_stream.into_bytes();
                    stream.write_bytes(&bytes)?;
                    return Ok(());
                }
                Err(ConstructError::StopField { .. }) => {
                    return Err(ConstructError::StopField {
                        path: String::new(),
                    });
                }
                Err(_) => {
                    // Try next subconstruct.
                }
            }
        }
        let val = input.as_value().unwrap_or(Value::None);
        Err(ConstructError::Select {
            path: String::new(),
            message: format!("no subconstruct matched for building: {val:?}"),
        })
    }

    fn exec_sizeof(&self, _ctx: &Context) -> Result<usize> {
        Err(ConstructError::Sizeof {
            path: String::new(),
            reason: "Select size depends on runtime data".to_string(),
        })
    }
}

// -- CompiledUnion --------------------------------------------------------

impl CompiledExec for CompiledUnion {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let mut child_ctx = ctx.subcontext();
        let fallback = stream.tell()?;

        let mut forward_by_index: Vec<u64> = Vec::with_capacity(self.fields.len());
        let mut forward_by_name: IndexMap<String, u64> = IndexMap::with_capacity(self.fields.len());

        for (i, field) in self.fields.iter().enumerate() {
            match &field.name {
                Some(name) => {
                    let value = match exec_parse_named_child(
                        &field.subcon,
                        stream,
                        &mut child_ctx,
                        sink,
                        name,
                    ) {
                        Ok(v) => v,
                        Err(e) => return Err(e.with_path_prefix(name)),
                    };
                    let forward_pos = stream.tell()?;
                    forward_by_index.push(forward_pos);
                    forward_by_name.insert(name.clone(), forward_pos);
                    // sink already integrated by finish_field inside helper.
                    child_ctx.insert(name.clone(), value);
                }
                None => {
                    let mut sub_sink = sink.sub_sink_for_item()?;
                    if let Err(e) =
                        field
                            .subcon
                            .exec_parse(stream, &mut child_ctx, sub_sink.as_mut())
                    {
                        return Err(e.with_path_prefix(&format!("[{i}]")));
                    }
                    let forward_pos = stream.tell()?;
                    forward_by_index.push(forward_pos);
                }
            }
            stream.seek(fallback)?;
        }

        // Advance the stream to the selected sub-construct's forward position.
        if let Some(ref target) = self.parsefrom {
            let pos = match target {
                crate::constructs::union::UnionTarget::Index(idx) => forward_by_index
                    .get(*idx)
                    .copied()
                    .ok_or_else(|| ConstructError::Index {
                        path: String::new(),
                        index: *idx,
                        length: forward_by_index.len(),
                    }),
                crate::constructs::union::UnionTarget::Name(name) => forward_by_name
                    .get(name)
                    .copied()
                    .ok_or_else(|| ConstructError::FieldMissing {
                        path: String::new(),
                        field: name.clone(),
                    }),
            }?;
            stream.seek(pos)?;
        }
        Ok(())
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let raw = input.as_value()?;
        let container = match &raw {
            Value::None => IndexMap::new(),
            Value::Container(map) => map.clone(),
            other => {
                return Err(ConstructError::TypeMismatch {
                    path: String::new(),
                    expected: "Container".to_string(),
                    actual: other.type_name().to_string(),
                });
            }
        };

        let mut child_ctx = ctx.subcontext();
        for (key, value) in &container {
            child_ctx.insert(key.clone(), value.clone());
        }

        // Find the first subcon whose name matches a key in the container.
        for field in &self.fields {
            let name_ref = field.name.as_deref();

            let should_build = if field.flagbuildnone {
                name_ref.map(|n| container.contains_key(n)).unwrap_or(false)
            } else if let Some(n) = name_ref {
                container.contains_key(n)
            } else {
                false
            };

            if !should_build {
                continue;
            }

            let build_value =
                if field.flagbuildnone {
                    name_ref
                        .and_then(|n| container.get(n))
                        .cloned()
                        .unwrap_or(Value::None)
                } else {
                    match name_ref {
                        Some(n) => container.get(n).cloned().ok_or_else(|| {
                            ConstructError::FieldMissing {
                                path: String::new(),
                                field: n.to_string(),
                            }
                        })?,
                        None => Value::None,
                    }
                };

            if let Some(ref name) = field.name {
                child_ctx.insert(name.clone(), build_value.clone());
            }

            let field_input = OwnedValueInput::new(build_value);
            return field
                .subcon
                .exec_build(&field_input, stream, &mut child_ctx)
                .map_err(|e| {
                    if let Some(ref name) = field.name {
                        e.with_path_prefix(name)
                    } else {
                        e.with_path_prefix("(anonymous)")
                    }
                });
        }

        Err(ConstructError::Union {
            path: String::new(),
            message: format!(
                "cannot build, none of the subcons were found in the dictionary: {:?}",
                container.keys().collect::<Vec<_>>()
            ),
        })
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        // Union sizeof returns the max of all subcons (matches declaration
        // tree behavior).
        let child_ctx = ctx.subcontext();
        let mut max_size: usize = 0;
        for field in &self.fields {
            let size = match field.subcon.exec_sizeof(&child_ctx) {
                Ok(s) => s,
                Err(e) => {
                    return Err(if let Some(ref name) = field.name {
                        e.with_path_prefix(name)
                    } else {
                        e.with_path_prefix("(anonymous)")
                    });
                }
            };
            if size > max_size {
                max_size = size;
            }
        }
        Ok(max_size)
    }
}

// -- CompiledFocusedSeq ---------------------------------------------------

impl CompiledExec for CompiledFocusedSeq {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let mut child_ctx = ctx.subcontext();
        let mut focused_value: Option<Value> = None;

        for field in &self.fields {
            match &field.name {
                Some(name) => {
                    // FocusedSeq uses exec_parse_field_value (extracts Value
                    // for context WITHOUT sink integration) — NOT the
                    // finish_field-based exec_parse_named_child. The focused
                    // value is deposited via set_scalar at the end.
                    let value = match exec_parse_field_value(
                        &field.subcon,
                        stream,
                        &mut child_ctx,
                        sink,
                        name,
                    ) {
                        Ok(v) => v,
                        Err(ConstructError::StopField { .. }) => break,
                        Err(e) => return Err(e.with_path_prefix(name)),
                    };
                    child_ctx.insert(name.clone(), value.clone());

                    if name == &self.parsebuildfrom {
                        focused_value = Some(value);
                    }
                }
                None => {
                    let mut sub_sink = sink.sub_sink_for_item()?;
                    match field
                        .subcon
                        .exec_parse(stream, &mut child_ctx, sub_sink.as_mut())
                    {
                        Ok(()) => {}
                        Err(ConstructError::StopField { .. }) => break,
                        Err(e) => return Err(e.with_path_prefix("(anonymous)")),
                    }
                }
            }
        }

        match focused_value {
            Some(v) => sink.set_scalar(v),
            None => Err(ConstructError::FieldMissing {
                path: String::new(),
                field: self.parsebuildfrom.clone(),
            }),
        }
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let input_val = input.as_value()?;
        let mut child_ctx = ctx.subcontext();
        child_ctx.insert(self.parsebuildfrom.clone(), input_val.clone());

        for field in &self.fields {
            let name_ref = field.name.as_deref();

            // The focused field gets the actual data; others get None.
            let build_value = if name_ref == Some(self.parsebuildfrom.as_str()) {
                input_val.clone()
            } else {
                Value::None
            };

            if let Some(ref name) = field.name {
                child_ctx.insert(name.clone(), build_value.clone());
            }

            let field_input = OwnedValueInput::new(build_value);
            match field
                .subcon
                .exec_build(&field_input, stream, &mut child_ctx)
            {
                Ok(()) => {}
                Err(ConstructError::StopField { .. }) => return Ok(()),
                Err(e) => {
                    return Err(if let Some(ref name) = field.name {
                        e.with_path_prefix(name)
                    } else {
                        e.with_path_prefix("(anonymous)")
                    });
                }
            }
        }
        Ok(())
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        let child_ctx = ctx.subcontext();
        let mut total: usize = 0;
        for field in &self.fields {
            let size = match field.subcon.exec_sizeof(&child_ctx) {
                Ok(s) => s,
                Err(e) => {
                    return Err(if let Some(ref name) = field.name {
                        e.with_path_prefix(name)
                    } else {
                        e.with_path_prefix("(anonymous)")
                    });
                }
            };
            total = total.saturating_add(size);
        }
        Ok(total)
    }
}

// ===========================================================================
// Functional CompiledExec for control flow (Phase 12.7)
// ===========================================================================

impl CompiledExec for CompiledIfThenElse {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        if (self.cond)(ctx) {
            self.then_constr.exec_parse(stream, ctx, sink)
        } else {
            self.else_constr.exec_parse(stream, ctx, sink)
        }
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        if (self.cond)(ctx) {
            self.then_constr.exec_build(input, stream, ctx)
        } else {
            self.else_constr.exec_build(input, stream, ctx)
        }
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        if (self.cond)(ctx) {
            self.then_constr.exec_sizeof(ctx)
        } else {
            self.else_constr.exec_sizeof(ctx)
        }
    }
}

impl CompiledExec for CompiledSwitch {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let key = (self.keyfunc)(ctx)?;
        let node = self
            .cases
            .iter()
            .find(|(k, _)| k == &key)
            .map(|(_, n)| n.as_ref())
            .or(self.default.as_deref());
        // If no case and no default: use Pass semantics (no-op, None).
        match node {
            Some(n) => n.exec_parse(stream, ctx, sink),
            None => {
                // No default — deposit None.
                sink.set_scalar(Value::None)
            }
        }
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let key = (self.keyfunc)(ctx)?;
        let node = self
            .cases
            .iter()
            .find(|(k, _)| k == &key)
            .map(|(_, n)| n.as_ref())
            .or(self.default.as_deref());
        match node {
            Some(n) => n.exec_build(input, stream, ctx),
            None => Ok(()), // Pass: no-op
        }
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        let key = (self.keyfunc)(ctx)?;
        let node = self
            .cases
            .iter()
            .find(|(k, _)| k == &key)
            .map(|(_, n)| n.as_ref())
            .or(self.default.as_deref());
        match node {
            Some(n) => n.exec_sizeof(ctx),
            None => Ok(0), // Pass: size 0
        }
    }
}

impl CompiledExec for CompiledCheck {
    fn exec_parse(
        &self,
        _stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        (self.check)(ctx)?;
        sink.set_scalar(Value::None)
    }

    fn exec_build(
        &self,
        _input: &dyn Input,
        _stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        (self.check)(ctx)
    }

    fn exec_sizeof(&self, _ctx: &Context) -> Result<usize> {
        Ok(0)
    }
}

impl CompiledExec for CompiledStopIf {
    fn exec_parse(
        &self,
        _stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        if (self.cond)(ctx) {
            Err(ConstructError::StopField {
                path: String::new(),
            })
        } else {
            sink.set_scalar(Value::None)
        }
    }

    fn exec_build(
        &self,
        _input: &dyn Input,
        _stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        if (self.cond)(ctx) {
            Err(ConstructError::StopField {
                path: String::new(),
            })
        } else {
            Ok(())
        }
    }

    fn exec_sizeof(&self, _ctx: &Context) -> Result<usize> {
        Err(ConstructError::Sizeof {
            path: String::new(),
            reason: "StopIf size depends on runtime context".to_string(),
        })
    }
}

// ===========================================================================
// Functional CompiledExec for computed/meta (Phase 12.8)
// ===========================================================================

impl CompiledExec for CompiledComputed {
    fn exec_parse(
        &self,
        _stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let value = (self.func)(ctx)?;
        sink.set_scalar(value)
    }

    fn exec_build(
        &self,
        _input: &dyn Input,
        _stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        // Evaluate and discard; the stream is unaffected.
        let _ = (self.func)(ctx)?;
        Ok(())
    }

    fn exec_sizeof(&self, _ctx: &Context) -> Result<usize> {
        Ok(0)
    }
}

impl CompiledExec for CompiledRebuild {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        self.inner.exec_parse(stream, ctx, sink)
    }

    fn exec_build(
        &self,
        _input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let value = (self.func)(ctx)?;
        let owned = OwnedValueInput::new(value);
        self.inner.exec_build(&owned, stream, ctx)
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        self.inner.exec_sizeof(ctx)
    }
}

impl CompiledExec for CompiledDefault {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        self.inner.exec_parse(stream, ctx, sink)
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let raw = input.as_value()?;
        let effective = if raw.is_none() {
            self.value.clone()
        } else {
            raw
        };
        let owned = OwnedValueInput::new(effective);
        self.inner.exec_build(&owned, stream, ctx)
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        self.inner.exec_sizeof(ctx)
    }
}

impl CompiledExec for CompiledIndex {
    fn exec_parse(
        &self,
        _stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let value = ctx
            .get(REPETITION_INDEX_KEY)
            .cloned()
            .unwrap_or(Value::None);
        sink.set_scalar(value)
    }

    fn exec_build(
        &self,
        _input: &dyn Input,
        _stream: &mut CombinedStream,
        _ctx: &mut Context,
    ) -> Result<()> {
        Ok(())
    }

    fn exec_sizeof(&self, _ctx: &Context) -> Result<usize> {
        Ok(0)
    }
}

/// Minimum valid modulus for Aligned.
const ALIGNED_MIN_MODULUS: usize = 2;

impl CompiledExec for CompiledPadded {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let pos_before = stream.tell()?;
        let value = exec_parse_inner_value(&self.inner, stream, ctx)?;
        let pos_after = stream.tell()?;
        let consumed = (pos_after - pos_before) as usize;

        if consumed > self.length {
            return Err(ConstructError::Padding {
                path: String::new(),
                message: format!(
                    "subcon parsed {} bytes but was allowed only {}",
                    consumed, self.length
                ),
            });
        }

        let pad = self.length - consumed;
        if pad > 0 {
            let padding_bytes = stream.read_bytes(pad)?;
            if self.strict {
                for &b in &padding_bytes {
                    if b != self.pattern {
                        return Err(ConstructError::Padding {
                            path: String::new(),
                            message: format!(
                                "expected padding byte 0x{:02X} but got 0x{:02X}",
                                self.pattern, b
                            ),
                        });
                    }
                }
            }
        }
        sink.set_scalar(value)
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let pos_before = stream.tell()?;
        self.inner.exec_build(input, stream, ctx)?;
        let pos_after = stream.tell()?;
        let written = (pos_after - pos_before) as usize;

        if written > self.length {
            return Err(ConstructError::Padding {
                path: String::new(),
                message: format!(
                    "subcon build {} bytes but was allowed only {}",
                    written, self.length
                ),
            });
        }

        let pad = self.length - written;
        if pad > 0 {
            let padding = vec![self.pattern; pad];
            stream.write_bytes(&padding)?;
        }
        Ok(())
    }

    fn exec_sizeof(&self, _ctx: &Context) -> Result<usize> {
        Ok(self.length)
    }
}

impl CompiledExec for CompiledAligned {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        if self.modulus < ALIGNED_MIN_MODULUS {
            return Err(ConstructError::Padding {
                path: String::new(),
                message: format!(
                    "expected modulus {} or greater, got {}",
                    ALIGNED_MIN_MODULUS, self.modulus
                ),
            });
        }
        let pos_before = stream.tell()?;
        let value = exec_parse_inner_value(&self.inner, stream, ctx)?;
        let pos_after = stream.tell()?;
        let consumed = (pos_after - pos_before) as usize;
        let pad = (self.modulus - (consumed % self.modulus)) % self.modulus;
        if pad > 0 {
            let _ = stream.read_bytes(pad)?;
        }
        sink.set_scalar(value)
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        if self.modulus < ALIGNED_MIN_MODULUS {
            return Err(ConstructError::Padding {
                path: String::new(),
                message: format!(
                    "expected modulus {} or greater, got {}",
                    ALIGNED_MIN_MODULUS, self.modulus
                ),
            });
        }
        let pos_before = stream.tell()?;
        self.inner.exec_build(input, stream, ctx)?;
        let pos_after = stream.tell()?;
        let written = (pos_after - pos_before) as usize;
        let pad = (self.modulus - (written % self.modulus)) % self.modulus;
        if pad > 0 {
            let padding = vec![self.pattern; pad];
            stream.write_bytes(&padding)?;
        }
        Ok(())
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        if self.modulus < ALIGNED_MIN_MODULUS {
            return Err(ConstructError::Padding {
                path: String::new(),
                message: format!(
                    "expected modulus {} or greater, got {}",
                    ALIGNED_MIN_MODULUS, self.modulus
                ),
            });
        }
        let inner_size = self.inner.exec_sizeof(ctx)?;
        let pad = (self.modulus - (inner_size % self.modulus)) % self.modulus;
        Ok(inner_size + pad)
    }
}

/// Default null-byte padding.
const DEFAULT_PAD_BYTE: u8 = 0x00;

impl CompiledExec for CompiledFixedSized {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let data = stream.read_bytes(self.length)?;
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_read(&data));
        let value = exec_parse_inner_value(&self.inner, &mut sub_stream, ctx)?;
        sink.set_scalar(value)
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_write());
        self.inner.exec_build(input, &mut sub_stream, ctx)?;
        let built = sub_stream.into_bytes();

        if built.len() > self.length {
            return Err(ConstructError::Padding {
                path: String::new(),
                message: format!(
                    "subcon build {} bytes but was allowed only {}",
                    built.len(),
                    self.length
                ),
            });
        }

        stream.write_bytes(&built)?;
        let pad = self.length - built.len();
        if pad > 0 {
            let padding = vec![DEFAULT_PAD_BYTE; pad];
            stream.write_bytes(&padding)?;
        }
        Ok(())
    }

    fn exec_sizeof(&self, _ctx: &Context) -> Result<usize> {
        Ok(self.length)
    }
}

impl CompiledExec for CompiledNamedTuple {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let raw = exec_parse_inner_value(&self.inner, stream, ctx)?;
        let named = Self::decode_value(&raw, &self.field_names)?;
        sink.set_scalar(named)
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let raw = input.as_value()?;
        let encoded = Self::encode_value(&raw, &self.field_names)?;
        let owned = OwnedValueInput::new(encoded);
        self.inner.exec_build(&owned, stream, ctx)
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        self.inner.exec_sizeof(ctx)
    }
}

impl CompiledNamedTuple {
    /// Decodes a raw value (List or Container from subcon) into a named
    /// list representation.
    fn decode_value(raw: &Value, field_names: &[String]) -> Result<Value> {
        match raw {
            Value::List(items) => {
                if items.len() != field_names.len() {
                    return Err(ConstructError::Array {
                        path: String::new(),
                        expected: field_names.len(),
                        actual: items.len(),
                    });
                }
                let named: Vec<Value> = items
                    .iter()
                    .zip(field_names.iter())
                    .map(|(val, name)| {
                        let mut entry = IndexMap::new();
                        entry.insert("name".to_string(), Value::String(name.clone()));
                        entry.insert("value".to_string(), val.clone());
                        Value::Container(entry)
                    })
                    .collect();
                Ok(Value::List(named))
            }
            Value::Container(map) => {
                let named: Vec<Value> = field_names
                    .iter()
                    .map(|name| {
                        let val = map.get(name).cloned().unwrap_or(Value::None);
                        let mut entry = IndexMap::new();
                        entry.insert("name".to_string(), Value::String(name.clone()));
                        entry.insert("value".to_string(), val);
                        Value::Container(entry)
                    })
                    .collect();
                Ok(Value::List(named))
            }
            other => Err(ConstructError::TypeMismatch {
                path: String::new(),
                expected: "List or Container".to_string(),
                actual: other.type_name().to_string(),
            }),
        }
    }

    /// Encodes a named list back into a value suitable for the inner construct.
    fn encode_value(data: &Value, field_names: &[String]) -> Result<Value> {
        match data {
            Value::List(items) => {
                let plain: Vec<Value> = items
                    .iter()
                    .zip(field_names.iter())
                    .map(|(item, _name)| match item {
                        Value::Container(entry) => {
                            entry.get("value").cloned().unwrap_or(Value::None)
                        }
                        other => other.clone(),
                    })
                    .collect();
                Ok(Value::List(plain))
            }
            Value::Container(map) => Ok(Value::Container(map.clone())),
            other => Err(ConstructError::TypeMismatch {
                path: String::new(),
                expected: "List or Container".to_string(),
                actual: other.type_name().to_string(),
            }),
        }
    }
}

impl CompiledExec for CompiledTimestampAdapter {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let raw_val = exec_parse_inner_value(&self.inner, stream, ctx)?;
        let raw_i64 = raw_val.to_i64()?;
        let (secs, nanos) = timestamp_to_secs_nanos(raw_i64, self.unit);
        let mut map = IndexMap::new();
        map.insert("secs".to_string(), Value::Int(secs));
        map.insert("nanos".to_string(), Value::UInt(u64::from(nanos)));
        sink.set_scalar(Value::Container(map))
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let raw = input.as_value()?;
        let container = raw.as_container()?;
        let secs = container
            .get("secs")
            .ok_or_else(|| ConstructError::FieldMissing {
                path: String::new(),
                field: "secs".to_string(),
            })?
            .to_i64()?;
        let nanos = container
            .get("nanos")
            .ok_or_else(|| ConstructError::FieldMissing {
                path: String::new(),
                field: "nanos".to_string(),
            })?
            .to_u64()? as u32;
        let raw_i64 = secs_nanos_to_timestamp(secs, nanos, self.unit);
        let owned = OwnedValueInput::new(Value::Int(raw_i64));
        self.inner.exec_build(&owned, stream, ctx)
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        self.inner.exec_sizeof(ctx)
    }
}

/// Converts a raw integer (in the given unit) to seconds and nanoseconds.
fn timestamp_to_secs_nanos(raw: i64, unit: TimestampUnit) -> (i64, u32) {
    match unit {
        TimestampUnit::Seconds => (raw, 0u32),
        TimestampUnit::Milliseconds => (raw / 1000, ((raw % 1000) * 1_000_000) as u32),
        TimestampUnit::Microseconds => (raw / 1_000_000, ((raw % 1_000_000) * 1000) as u32),
        TimestampUnit::Nanoseconds => (raw / 1_000_000_000, (raw % 1_000_000_000) as u32),
    }
}

/// Converts seconds and nanoseconds back to a raw integer in the given unit.
fn secs_nanos_to_timestamp(secs: i64, nanos: u32, unit: TimestampUnit) -> i64 {
    match unit {
        TimestampUnit::Seconds => secs,
        TimestampUnit::Milliseconds => secs * 1000 + (i64::from(nanos) / 1_000_000),
        TimestampUnit::Microseconds => secs * 1_000_000 + (i64::from(nanos) / 1000),
        TimestampUnit::Nanoseconds => secs * 1_000_000_000 + i64::from(nanos),
    }
}

// ===========================================================================
// Functional CompiledExec for lazy (Phase 12.8: eager delegation)
// ===========================================================================

impl CompiledExec for CompiledLazy {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let _ = stream.tell()?;
        self.inner.exec_parse(stream, ctx, sink)
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        self.inner.exec_build(input, stream, ctx)
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        self.inner.exec_sizeof(ctx)
    }
}

impl CompiledExec for CompiledLazyStruct {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        self.inner.exec_parse(stream, ctx, sink)
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        self.inner.exec_build(input, stream, ctx)
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        self.inner.exec_sizeof(ctx)
    }
}

impl CompiledExec for CompiledLazyArray {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        for i in 0..self.count {
            ctx.insert(REPETITION_INDEX_KEY, Value::UInt(i as u64));
            let mut sub_sink = sink.sub_sink_for_item()?;
            let value = match self.subcon.exec_parse(stream, ctx, sub_sink.as_mut()) {
                Ok(()) => sub_sink.into_value()?,
                Err(e) => return Err(e.with_path_prefix(&format!("[{i}]"))),
            };
            sink.push_item(value)?;
        }
        Ok(())
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let raw = input.as_value()?;
        let list = match &raw {
            Value::List(items) => items.clone(),
            other => {
                return Err(ConstructError::TypeMismatch {
                    path: String::new(),
                    expected: "List".to_string(),
                    actual: other.type_name().to_string(),
                });
            }
        };
        if list.len() != self.count {
            return Err(ConstructError::Array {
                path: String::new(),
                expected: self.count,
                actual: list.len(),
            });
        }
        for (i, element) in list.iter().enumerate() {
            ctx.insert(REPETITION_INDEX_KEY, Value::UInt(i as u64));
            let elem_input = OwnedValueInput::new(element.clone());
            if let Err(e) = self.subcon.exec_build(&elem_input, stream, ctx) {
                return Err(e.with_path_prefix(&format!("[{i}]")));
            }
        }
        Ok(())
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        let sub_size = self.subcon.exec_sizeof(ctx)?;
        Ok(self.count.saturating_mul(sub_size))
    }
}

impl CompiledExec for CompiledRebuffered {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let start = stream.tell()?;
        let data = stream.read_remaining()?;
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_read(&data));
        let value = exec_parse_inner_value(&self.inner, &mut sub_stream, ctx)?;
        let consumed = sub_stream.tell()?;
        stream.seek(start + consumed)?;
        sink.set_scalar(value)
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        self.inner.exec_build(input, stream, ctx)
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        self.inner.exec_sizeof(ctx)
    }
}

// ===========================================================================
// Functional CompiledExec for stream ops (Phase 12.7)
// ===========================================================================

impl CompiledExec for CompiledBitwise {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let data = stream.read_remaining()?;
        let bits = crate::binary::bytes2bits(&data);
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_read(&bits));
        let value = exec_parse_inner_value(&self.inner, &mut sub_stream, ctx)?;
        sink.set_scalar(value)
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_write());
        self.inner.exec_build(input, &mut sub_stream, ctx)?;
        let bits = sub_stream.into_bytes();
        let bytes = crate::binary::bits2bytes(&bits)?;
        stream.write_bytes(&bytes)
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        let bits_size = self.inner.exec_sizeof(ctx)?;
        Ok((bits_size + 7) / 8)
    }
}

impl CompiledExec for CompiledBytewise {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let bits = stream.read_remaining()?;
        let bytes = crate::binary::bits2bytes(&bits)?;
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_read(&bytes));
        let value = exec_parse_inner_value(&self.inner, &mut sub_stream, ctx)?;
        sink.set_scalar(value)
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_write());
        self.inner.exec_build(input, &mut sub_stream, ctx)?;
        let bytes = sub_stream.into_bytes();
        let bits = crate::binary::bytes2bits(&bytes);
        stream.write_bytes(&bits)
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        let byte_size = self.inner.exec_sizeof(ctx)?;
        Ok(byte_size * 8)
    }
}

impl CompiledExec for CompiledPointer {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let fallback = stream.tell()?;
        seek_to_offset(stream, self.offset)?;
        let result = self.inner.exec_parse(stream, ctx, sink);
        let _ = stream.seek(fallback);
        result
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let fallback = stream.tell()?;
        seek_to_offset(stream, self.offset)?;
        let result = self.inner.exec_build(input, stream, ctx);
        let _ = stream.seek(fallback);
        result
    }

    fn exec_sizeof(&self, _ctx: &Context) -> Result<usize> {
        Ok(0)
    }
}

impl CompiledExec for CompiledPointerExpr {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let offset_val = self
            .compiled_offset
            .eval(ctx, None)
            .map_err(|e| e.with_path_prefix("PointerExpr"))?;
        let offset = offset_val.to_i64().map_err(|e| ConstructError::Expr {
            path: String::new(),
            message: format!("PointerExpr offset must be an integer: {e}"),
        })?;
        let fallback = stream.tell()?;
        seek_to_offset(stream, offset)?;
        let result = self.inner.exec_parse(stream, ctx, sink);
        let _ = stream.seek(fallback);
        result
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let offset_val = self
            .compiled_offset
            .eval(ctx, None)
            .map_err(|e| e.with_path_prefix("PointerExpr"))?;
        let offset = offset_val.to_i64().map_err(|e| ConstructError::Expr {
            path: String::new(),
            message: format!("PointerExpr offset must be an integer: {e}"),
        })?;
        let fallback = stream.tell()?;
        seek_to_offset(stream, offset)?;
        let result = self.inner.exec_build(input, stream, ctx);
        let _ = stream.seek(fallback);
        result
    }

    fn exec_sizeof(&self, _ctx: &Context) -> Result<usize> {
        Ok(0)
    }
}

/// Seeks the stream to the given offset (positive = absolute, negative =
/// from end).
fn seek_to_offset(stream: &mut CombinedStream, offset: i64) -> Result<()> {
    use std::io::SeekFrom;
    if offset >= 0 {
        stream.seek(offset as u64)?;
        Ok(())
    } else {
        stream.seek_from(SeekFrom::End(offset))?;
        Ok(())
    }
}

impl CompiledExec for CompiledPeek {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let fallback = stream.tell()?;
        let result = self.inner.exec_parse(stream, ctx, sink);
        let _ = stream.seek(fallback);
        match result {
            Ok(()) => Ok(()),
            Err(e @ ConstructError::Check { .. }) | Err(e @ ConstructError::StopField { .. }) => {
                Err(e)
            }
            Err(_) => sink.set_scalar(Value::None),
        }
    }

    fn exec_build(
        &self,
        _input: &dyn Input,
        _stream: &mut CombinedStream,
        _ctx: &mut Context,
    ) -> Result<()> {
        Ok(())
    }

    fn exec_sizeof(&self, _ctx: &Context) -> Result<usize> {
        Ok(0)
    }
}

impl CompiledExec for CompiledRawCopy {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let offset1 = stream.tell()?;
        let value = exec_parse_inner_value(&self.inner, stream, ctx)?;
        let offset2 = stream.tell()?;
        let length = offset2.saturating_sub(offset1) as usize;
        stream.seek(offset1)?;
        let data = stream.read_bytes(length)?;
        let mut map = IndexMap::new();
        map.insert("data".to_string(), Value::Bytes(data));
        map.insert("value".to_string(), value);
        map.insert("offset1".to_string(), Value::UInt(offset1));
        map.insert("offset2".to_string(), Value::UInt(offset2));
        map.insert("length".to_string(), Value::UInt(length as u64));
        sink.set_scalar(Value::Container(map))
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let raw = input.as_value()?;
        let container = raw.as_container()?;
        if let Some(Value::Bytes(raw_data)) = container.get("data") {
            stream.write_bytes(raw_data)?;
            return Ok(());
        }
        if let Some(value) = container.get("value") {
            let owned = OwnedValueInput::new(value.clone());
            self.inner.exec_build(&owned, stream, ctx)?;
            return Ok(());
        }
        Err(ConstructError::Generic {
            path: String::new(),
            message: "RawCopy cannot build: both data and value keys are missing".to_string(),
        })
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        self.inner.exec_sizeof(ctx)
    }
}

impl CompiledExec for CompiledPrefixed {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let length_val = exec_parse_inner_value(&self.length_field, stream, ctx)?;
        let mut length = length_val.to_u64()? as usize;
        if self.include_length {
            let lf_size = self.length_field.exec_sizeof(ctx)?;
            length = length.saturating_sub(lf_size);
        }
        let data = stream.read_bytes(length)?;
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_read(&data));
        let value = exec_parse_inner_value(&self.subcon, &mut sub_stream, ctx)?;
        sink.set_scalar(value)
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_write());
        self.subcon.exec_build(input, &mut sub_stream, ctx)?;
        let built_data = sub_stream.into_bytes();

        let mut length = built_data.len() as u64;
        if self.include_length {
            let lf_size = self.length_field.exec_sizeof(ctx)?;
            length += lf_size as u64;
        }
        self.length_field
            .exec_build(&OwnedValueInput::new(Value::UInt(length)), stream, ctx)?;
        stream.write_bytes(&built_data)
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        let length_size = self.length_field.exec_sizeof(ctx)?;
        let subcon_size = self.subcon.exec_sizeof(ctx)?;
        Ok(length_size + subcon_size)
    }
}

impl CompiledExec for CompiledTransformed {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let data = match self.decode_amount {
            Some(amount) => stream.read_bytes(amount)?,
            None => stream.read_remaining()?,
        };
        let decoded = (self.decode)(&data)?;
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_read(&decoded));
        let value = exec_parse_inner_value(&self.inner, &mut sub_stream, ctx)?;
        sink.set_scalar(value)
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_write());
        self.inner.exec_build(input, &mut sub_stream, ctx)?;
        let built = sub_stream.into_bytes();
        let encoded = (self.encode)(&built)?;
        if let Some(expected) = self.encode_amount {
            if encoded.len() != expected {
                return Err(ConstructError::Generic {
                    path: String::new(),
                    message: format!(
                        "encoding transformation produced {} bytes instead of expected {}",
                        encoded.len(),
                        expected
                    ),
                });
            }
        }
        stream.write_bytes(&encoded)
    }

    fn exec_sizeof(&self, _ctx: &Context) -> Result<usize> {
        match (self.decode_amount, self.encode_amount) {
            (Some(da), Some(ea)) if da == ea => Ok(ea),
            _ => Err(ConstructError::Sizeof {
                path: String::new(),
                reason: "Transformed size undefined".to_string(),
            }),
        }
    }
}

impl CompiledExec for CompiledRestreamed {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        if self.decoder_unit == 0 {
            return Err(ConstructError::Generic {
                path: String::new(),
                message: "Restreamed decoder_unit must be positive".to_string(),
            });
        }
        let raw = stream.read_remaining()?;
        let mut decoded = Vec::new();
        for chunk in raw.chunks(self.decoder_unit) {
            let decoded_chunk = (self.decoder)(chunk)?;
            decoded.extend_from_slice(&decoded_chunk);
        }
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_read(&decoded));
        let value = exec_parse_inner_value(&self.inner, &mut sub_stream, ctx)?;
        sink.set_scalar(value)
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        if self.encoder_unit == 0 {
            return Err(ConstructError::Generic {
                path: String::new(),
                message: "Restreamed encoder_unit must be positive".to_string(),
            });
        }
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_write());
        self.inner.exec_build(input, &mut sub_stream, ctx)?;
        let built = sub_stream.into_bytes();
        for chunk in built.chunks(self.encoder_unit) {
            let encoded_chunk = (self.encoder)(chunk)?;
            stream.write_bytes(&encoded_chunk)?;
        }
        Ok(())
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        match &self.size_computer {
            Some(computer) => {
                let inner_size = self.inner.exec_sizeof(ctx)?;
                Ok(computer(inner_size))
            }
            None => Err(ConstructError::Sizeof {
                path: String::new(),
                reason: "Restreamed cannot calculate size without size_computer".to_string(),
            }),
        }
    }
}

#[cfg(feature = "compression")]
impl CompiledExec for CompiledCompressed {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        use crate::constructs::stream_ops::CompressionAlgorithm;
        let data = stream.read_remaining()?;
        let decoded = match self.algorithm {
            CompressionAlgorithm::Zlib => {
                use std::io::Read;
                let mut decoder = flate2::read::ZlibDecoder::new(&data[..]);
                let mut out = Vec::new();
                decoder
                    .read_to_end(&mut out)
                    .map_err(|e| ConstructError::Generic {
                        path: String::new(),
                        message: format!("zlib decompression failed: {e}"),
                    })?;
                out
            }
            CompressionAlgorithm::Deflate => {
                use std::io::Read;
                let mut decoder = flate2::read::DeflateDecoder::new(&data[..]);
                let mut out = Vec::new();
                decoder
                    .read_to_end(&mut out)
                    .map_err(|e| ConstructError::Generic {
                        path: String::new(),
                        message: format!("deflate decompression failed: {e}"),
                    })?;
                out
            }
        };
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_read(&decoded));
        let value = exec_parse_inner_value(&self.inner, &mut sub_stream, ctx)?;
        sink.set_scalar(value)
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        use crate::constructs::stream_ops::CompressionAlgorithm;
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_write());
        self.inner.exec_build(input, &mut sub_stream, ctx)?;
        let built = sub_stream.into_bytes();
        let encoded = match self.algorithm {
            CompressionAlgorithm::Zlib => {
                use std::io::Read;
                let mut encoder =
                    flate2::read::ZlibEncoder::new(&built[..], flate2::Compression::default());
                let mut out = Vec::new();
                encoder
                    .read_to_end(&mut out)
                    .map_err(|e| ConstructError::Generic {
                        path: String::new(),
                        message: format!("zlib compression failed: {e}"),
                    })?;
                out
            }
            CompressionAlgorithm::Deflate => {
                use std::io::Read;
                let mut encoder =
                    flate2::read::DeflateEncoder::new(&built[..], flate2::Compression::default());
                let mut out = Vec::new();
                encoder
                    .read_to_end(&mut out)
                    .map_err(|e| ConstructError::Generic {
                        path: String::new(),
                        message: format!("deflate compression failed: {e}"),
                    })?;
                out
            }
        };
        stream.write_bytes(&encoded)
    }

    fn exec_sizeof(&self, _ctx: &Context) -> Result<usize> {
        Err(ConstructError::Sizeof {
            path: String::new(),
            reason: "Compressed has undefined size".to_string(),
        })
    }
}

impl CompiledExec for CompiledChecksum {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let parsed = exec_parse_inner_value(&self.checksum_field, stream, ctx)?;
        let raw_bytes = (self.bytes_func)(ctx)?;
        let expected = (self.hash_func)(&raw_bytes);
        let parsed_bytes = parsed.as_bytes().unwrap_or(&[]).to_vec();
        if parsed_bytes != expected {
            return Err(ConstructError::Check {
                path: String::new(),
                message: format!(
                    "wrong checksum: read {:02x?}, computed {:02x?}",
                    parsed_bytes, expected
                ),
            });
        }
        sink.set_scalar(parsed)
    }

    fn exec_build(
        &self,
        _input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let raw_bytes = (self.bytes_func)(ctx)?;
        let checksum = (self.hash_func)(&raw_bytes);
        self.checksum_field
            .exec_build(&OwnedValueInput::new(Value::Bytes(checksum)), stream, ctx)
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        self.checksum_field.exec_sizeof(ctx)
    }
}

impl CompiledExec for CompiledByteSwapped {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let size = self.inner.exec_sizeof(ctx)?;
        let data = stream.read_bytes(size)?;
        let swapped = crate::binary::swapbytes(&data);
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_read(&swapped));
        let value = exec_parse_inner_value(&self.inner, &mut sub_stream, ctx)?;
        sink.set_scalar(value)
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_write());
        self.inner.exec_build(input, &mut sub_stream, ctx)?;
        let built = sub_stream.into_bytes();
        let swapped = crate::binary::swapbytes(&built);
        stream.write_bytes(&swapped)
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        self.inner.exec_sizeof(ctx)
    }
}

/// Reverses the bit order within each byte of `data`.
fn swap_bits_in_bytes(data: &[u8]) -> Vec<u8> {
    data.iter()
        .map(|&b| {
            let mut result = 0u8;
            for i in 0..8 {
                if b & (1 << i) != 0 {
                    result |= 1 << (7 - i);
                }
            }
            result
        })
        .collect()
}

impl CompiledExec for CompiledBitsSwapped {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let data = if self.fixed_size {
            let size = self.inner.exec_sizeof(ctx)?;
            stream.read_bytes(size)?
        } else {
            stream.read_remaining()?
        };
        let swapped = swap_bits_in_bytes(&data);
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_read(&swapped));
        let value = exec_parse_inner_value(&self.inner, &mut sub_stream, ctx)?;
        sink.set_scalar(value)
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_write());
        self.inner.exec_build(input, &mut sub_stream, ctx)?;
        let built = sub_stream.into_bytes();
        let swapped = swap_bits_in_bytes(&built);
        stream.write_bytes(&swapped)
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        self.inner.exec_sizeof(ctx)
    }
}

impl CompiledExec for CompiledLazyBound {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let value = self.inner.parse(stream, ctx)?;
        sink.set_scalar(value)
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let value = input.as_value()?;
        self.inner.build(&value, stream, ctx)
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        self.inner.sizeof(ctx)
    }
}

// ===========================================================================
// Functional CompiledExec implementation for CompiledDynamic (escape-hatch)
// ===========================================================================
//
// CompiledDynamic is the escape-hatch: it wraps a declaration-tree
// `Arc<dyn Construct>` and delegates every operation to it (old-path
// fallback). Unlike the stub impls above, this is fully functional — it is
// the only compiled node with a real runtime implementation in Phase 12.3.
// This makes the Dynamic escape-hatch end-to-end usable: compile a Dynamic
// node, then parse/build/sizeof through CompiledSchema.

impl CompiledExec for CompiledDynamic {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        // Delegate to the wrapped construct's old-path parse, then deposit
        // the resulting scalar into the sink (uniform leaf protocol, I1).
        let value = self.inner.parse(stream, ctx)?;
        sink.set_scalar(value)
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        // Delegate to the wrapped construct's old-path build.
        let value = input.as_value()?;
        self.inner.build(&value, stream, ctx)
    }

    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        // Delegate to the wrapped construct's old-path sizeof.
        self.inner.sizeof(ctx)
    }
}

// ===========================================================================
// Functional CompiledExec implementation for CompiledExternal (Phase 13)
// ===========================================================================

impl CompiledExec for CompiledExternal {
    fn exec_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()> {
        let produced = self.inner.ext_parse(stream, ctx)?;
        match produced {
            ProducedOutput::Value(v) => sink.set_scalar(v),
            // For Object output: the opaque boxed object cannot be deposited
            // into a ValueSink (pure Rust path). In the PyDictSink path
            // (construct-py), the parent's finish_field/finish_named_item
            // calls into_produced which handles the Object variant. For the
            // pure Rust path, we deposit Value::None as a placeholder.
            ProducedOutput::Object(_) => sink.set_scalar(Value::None),
        }
    }

    fn exec_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        self.inner.ext_build(input, stream, ctx)
    }

    fn exec_sizeof(&self, _ctx: &Context) -> Result<usize> {
        self.inner.ext_sizeof()
    }
}

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
    ///
    /// # Why `Arc` (not `Rc`)
    ///
    /// The tree is stored behind an `Arc` rather than an `Rc` even though
    /// `CompiledNode` is not currently `Send + Sync` (the `Dynamic`
    /// escape-hatch holds an `Arc<dyn Construct>` without `Send + Sync`
    /// bounds, per the I3 design decision). `Arc` is used deliberately for
    /// forward compatibility: future phases that make `dyn Construct` thread-
    /// safe will not require changing this storage, and `Arc::clone` is cheap
    /// enough for the parse/build hot path. See design doc §5.5 (I3).
    #[must_use]
    #[allow(clippy::arc_with_non_send_sync)]
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
        let input = ValueInput::new(data);
        self.tree
            .exec_build(&input, &mut stream, &mut ctx)
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
/// Returns `Some(size)` if the node is a fixed-length leaf, `None` otherwise.
/// This enables [`CompiledSchema::sizeof`] to return a constant without
/// runtime traversal for fully-static leaf schemas.
///
/// Composite/repetition/wrapper folding (summing child sizes, etc.) is added
/// in later sub-tasks (12.5-12.9) as those node structs gain real fields.
fn try_fold_static_size(node: &CompiledNode) -> Option<usize> {
    match node {
        // Fixed-size leaves: size is a direct field of the embedded construct.
        CompiledNode::FormatField(f) => Some(f.inner.byte_size()),
        CompiledNode::Bytes(b) => Some(b.inner.length),
        CompiledNode::BytesInteger(b) => Some(b.inner.length),
        CompiledNode::PaddedString(p) => Some(p.inner.length),
        // Flag is always exactly one byte.
        CompiledNode::Flag(_) => Some(1),
        // Zero-size leaves.
        CompiledNode::Pass(_) | CompiledNode::Tell(_) | CompiledNode::Terminated(_) => Some(0),
        // Variable / runtime-length / stream-position leaves: unknown statically.
        CompiledNode::VarInt(_)
        | CompiledNode::ZigZag(_)
        | CompiledNode::GreedyBytes(_)
        | CompiledNode::BytesExpr(_)
        | CompiledNode::CString(_)
        | CompiledNode::BitsInteger(_)
        | CompiledNode::Seek(_)
        | CompiledNode::SeekExpr(_)
        | CompiledNode::Error(_) => None,
        // Wrappers (Phase 12.5): static size folds to the inner node's size,
        // since wrappers add no bytes of their own.
        CompiledNode::Subconstruct(w) => try_fold_static_size(&w.inner),
        CompiledNode::Renamed(w) => try_fold_static_size(&w.inner),
        CompiledNode::Const(w) => try_fold_static_size(&w.inner),
        CompiledNode::Adapter(w) => try_fold_static_size(&w.inner),
        CompiledNode::SymmetricAdapter(w) => try_fold_static_size(&w.inner),
        CompiledNode::ExprAdapter(w) => try_fold_static_size(&w.inner),
        CompiledNode::Validator(w) => try_fold_static_size(&w.inner),
        CompiledNode::ExprValidator(w) => try_fold_static_size(&w.inner),
        CompiledNode::Hex(w) => try_fold_static_size(&w.inner),
        CompiledNode::HexDump(w) => try_fold_static_size(&w.inner),
        CompiledNode::Enum(w) => try_fold_static_size(&w.inner),
        CompiledNode::FlagsEnum(w) => try_fold_static_size(&w.inner),
        CompiledNode::Mapping(w) => try_fold_static_size(&w.inner),
        // Composites (Phase 12.6): sum of all child sizes.
        CompiledNode::Struct(s) => s
            .fields
            .iter()
            .map(|f| try_fold_static_size(&f.subcon))
            .sum(),
        CompiledNode::Sequence(s) => s
            .entries
            .iter()
            .map(|e| try_fold_static_size(&e.subcon))
            .sum(),
        CompiledNode::FocusedSeq(s) => s
            .fields
            .iter()
            .map(|f| try_fold_static_size(&f.subcon))
            .sum(),
        // Array: count × element size (if element is fixed-length).
        CompiledNode::Array(a) => try_fold_static_size(&a.subcon).map(|elem| elem * a.count),
        // GreedyRange, ArrayExpr, RepeatUntil: variable count → unknown.
        CompiledNode::GreedyRange(_)
        | CompiledNode::ArrayExpr(_)
        | CompiledNode::RepeatUntil(_) => None,
        // Select, Union: size depends on runtime data → unknown.
        CompiledNode::Select(_) | CompiledNode::Union(_) => None,
        // RepeatUntil: compiles to Dynamic (never a CompiledRepeatUntil node).
        // Dynamic escape-hatch and all not-yet-implemented nodes: unknown.
        // Computed/meta/padding (Phase 12.8):
        CompiledNode::Computed(_) | CompiledNode::Index(_) | CompiledNode::Check(_) => Some(0),
        CompiledNode::Padded(p) => Some(p.length),
        CompiledNode::FixedSized(f) => Some(f.length),
        CompiledNode::Default(w) => try_fold_static_size(&w.inner),
        CompiledNode::Rebuild(w) => try_fold_static_size(&w.inner),
        CompiledNode::Aligned(a) => try_fold_static_size(&a.inner).map(|s| {
            let pad = (a.modulus - (s % a.modulus)) % a.modulus;
            s + pad
        }),
        CompiledNode::NamedTuple(w) => try_fold_static_size(&w.inner),
        CompiledNode::TimestampAdapter(w) => try_fold_static_size(&w.inner),
        // Lazy (Phase 12.8): folds to inner.
        CompiledNode::Lazy(w) => try_fold_static_size(&w.inner),
        CompiledNode::LazyStruct(w) => try_fold_static_size(&w.inner),
        CompiledNode::LazyArray(a) => try_fold_static_size(&a.subcon).map(|e| e * a.count),
        CompiledNode::Rebuffered(w) => try_fold_static_size(&w.inner),
        // Stream ops (Phase 12.7):
        CompiledNode::Pointer(_) | CompiledNode::PointerExpr(_) | CompiledNode::Peek(_) => Some(0),
        CompiledNode::RawCopy(w) => try_fold_static_size(&w.inner),
        CompiledNode::Prefixed(p) => {
            let lf = try_fold_static_size(&p.length_field);
            let sc = try_fold_static_size(&p.subcon);
            match (lf, sc) {
                (Some(a), Some(b)) => Some(a + b),
                _ => None,
            }
        }
        CompiledNode::ByteSwapped(w) => try_fold_static_size(&w.inner),
        CompiledNode::BitsSwapped(w) => try_fold_static_size(&w.inner),
        // Bitwise: (bits_size + 7) / 8.
        CompiledNode::Bitwise(b) => try_fold_static_size(&b.inner).map(|s| (s + 7) / 8),
        // Bytewise: byte_size * 8.
        CompiledNode::Bytewise(b) => try_fold_static_size(&b.inner).map(|s| s * 8),
        // Transformed: known only if decode_amount == encode_amount.
        CompiledNode::Transformed(t) => match (t.decode_amount, t.encode_amount) {
            (Some(da), Some(ea)) if da == ea => Some(ea),
            _ => None,
        },
        // Control flow: depends on runtime → unknown, except Check/StopIf.
        CompiledNode::IfThenElse(_) | CompiledNode::Switch(_) => None,
        CompiledNode::StopIf(_) => None,
        // Checksum, Restreamed, LazyBound, Compressed: variable → unknown.
        CompiledNode::Checksum(_) | CompiledNode::Restreamed(_) | CompiledNode::LazyBound(_) => {
            None
        }
        #[cfg(feature = "compression")]
        CompiledNode::Compressed(_) => None,
        // External (Phase 13): size depends on the extension → unknown.
        CompiledNode::External(_) => None,
        // Dynamic escape-hatch: unknown.
        _ => None,
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combined::CombinedConstruct;
    use crate::constructs::bytes::Bytes;
    use crate::constructs::flag::Flag;
    use crate::constructs::focused_seq::FocusedSeq;
    use crate::constructs::format_field::{Endianness, FormatField, FormatKind, INT16UB, INT8UB};
    use crate::constructs::meta::Pass;
    use crate::constructs::repetition::{Array, GreedyRange};
    use crate::constructs::select::Select;
    use crate::constructs::sequence::Sequence;
    use crate::constructs::struct_::{Struct, StructField};
    use crate::constructs::union::Union;
    use crate::constructs::varint::VarInt;

    /// Helper: builds a `CompiledNode::Pass` embedding a default `Pass`.
    fn compiled_pass() -> CompiledNode {
        CompiledNode::Pass(CompiledPass { inner: Pass::new() })
    }

    /// Helper: builds a `CompiledDynamic` wrapping a `Pass` construct, used
    /// wherever a `CompiledDynamic` value is needed in tests.
    fn dynamic_pass() -> CompiledDynamic {
        CompiledDynamic {
            inner: Arc::new(Pass::new()),
        }
    }

    // -- CompiledNode construction ------------------------------------------

    #[test]
    fn compiled_node_pass_variant_constructible() {
        let node = compiled_pass();
        assert!(matches!(node, CompiledNode::Pass(_)));
    }

    #[test]
    fn compiled_node_dynamic_variant_constructible() {
        let node = CompiledNode::Dynamic(dynamic_pass());
        assert!(matches!(node, CompiledNode::Dynamic(_)));
    }

    #[test]
    fn compiled_node_struct_variant_constructible() {
        // Struct now holds compiled fields (Phase 12.6).
        let node = CompiledNode::Struct(CompiledStruct { fields: vec![] });
        assert!(matches!(node, CompiledNode::Struct(_)));
    }

    // -- Leaf CompiledExec delegation (Phase 12.4) --------------------------
    //
    // Leaf compiled nodes embed their declaration-tree construct and delegate
    // every operation to it (the old path). exec_parse deposits the parsed
    // scalar into the sink via set_scalar.

    #[test]
    fn leaf_pass_exec_parse_delegates_to_inner() {
        // Pass parses nothing from empty input, deposits Value::None.
        let node = compiled_pass();
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[]));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        node.exec_parse(&mut stream, &mut ctx, &mut sink).unwrap();
        let value = Box::new(sink).into_value().unwrap();
        assert_eq!(value, Value::None);
    }

    #[test]
    fn leaf_pass_exec_sizeof_is_zero() {
        let node = compiled_pass();
        let ctx = Context::new();
        assert_eq!(node.exec_sizeof(&ctx).unwrap(), 0);
    }

    #[test]
    fn leaf_format_field_exec_parse_reads_bytes() {
        let node = CompiledNode::FormatField(CompiledFormatField {
            inner: FormatField::new(Endianness::Big, FormatKind::U8),
        });
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[0x05]));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        node.exec_parse(&mut stream, &mut ctx, &mut sink).unwrap();
        let value = Box::new(sink).into_value().unwrap();
        assert_eq!(value, Value::UInt(5));
    }

    #[test]
    fn leaf_format_field_exec_build_writes_bytes() {
        let node = CompiledNode::FormatField(CompiledFormatField {
            inner: FormatField::new(Endianness::Big, FormatKind::U8),
        });
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        node.exec_build(&ValueInput::new(&Value::UInt(5)), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), vec![0x05]);
    }

    #[test]
    fn leaf_format_field_exec_sizeof_returns_byte_size() {
        let node = CompiledNode::FormatField(CompiledFormatField {
            inner: FormatField::new(Endianness::Big, FormatKind::U32),
        });
        let ctx = Context::new();
        assert_eq!(node.exec_sizeof(&ctx).unwrap(), 4);
    }

    #[test]
    fn leaf_bytes_exec_sizeof_returns_length() {
        let node = CompiledNode::Bytes(CompiledBytes {
            inner: Bytes::new(7),
        });
        let ctx = Context::new();
        assert_eq!(node.exec_sizeof(&ctx).unwrap(), 7);
    }

    #[test]
    fn leaf_varint_exec_sizeof_is_sizeof_error() {
        // VarInt has a variable size → exec_sizeof returns a Sizeof error.
        let node = CompiledNode::VarInt(CompiledVarInt {
            inner: VarInt::new(),
        });
        let ctx = Context::new();
        let err = node.exec_sizeof(&ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Sizeof { .. }));
    }

    // -- Lazy CompiledExec delegation (Phase 12.8) -------------------------

    #[test]
    fn lazy_exec_delegates_to_inner() {
        // Lazy (Phase 12.8): delegates to compiled inner.
        let node = CompiledNode::Lazy(crate::compiled::CompiledLazy {
            inner: Box::new(CompiledNode::FormatField(CompiledFormatField {
                inner: FormatField::new(Endianness::Big, FormatKind::U8),
            })),
        });
        let ctx = Context::new();
        assert_eq!(node.exec_sizeof(&ctx).unwrap(), 1);
    }

    #[test]
    fn wrapper_adapter_exec_sizeof_delegates_to_inner() {
        // Adapter (Phase 12.5): exec_sizeof delegates to the compiled inner
        // node. We build a CompiledAdapter wrapping a compiled FormatField
        // (static size 1) with identity closures.
        use crate::constructs::adapters::{DecodeFuncBox, EncodeFuncBox};
        let inner = Box::new(CompiledNode::FormatField(CompiledFormatField {
            inner: FormatField::new(Endianness::Big, FormatKind::U8),
        }));
        let decode: DecodeFuncBox = Box::new(|v, _ctx| Ok(v.clone()));
        let encode: EncodeFuncBox = Box::new(|v, _ctx| Ok(v.clone()));
        let adapter = CompiledAdapter {
            inner,
            decode: Arc::from(decode),
            encode: Arc::from(encode),
        };
        let node = CompiledNode::Adapter(adapter);
        let ctx = Context::new();
        assert_eq!(node.exec_sizeof(&ctx).unwrap(), 1);
    }

    // -- Wrapper CompiledExec delegation (Phase 12.5) -----------------------
    //
    // Wrapper compiled nodes hold a recursively-compiled inner
    // Box<CompiledNode> and apply their transform/check/mapping logic on top.

    /// Helper: builds a compiled inner FormatField U8 big-endian node.
    fn compiled_u8() -> CompiledNode {
        CompiledNode::FormatField(CompiledFormatField {
            inner: FormatField::new(Endianness::Big, FormatKind::U8),
        })
    }

    #[test]
    fn wrapper_hex_exec_forwards_parse_and_build() {
        // Hex is pure-forward: parse and build delegate to inner.
        let node = CompiledNode::Hex(CompiledHex {
            inner: Box::new(compiled_u8()),
        });
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[42]));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        node.exec_parse(&mut stream, &mut ctx, &mut sink).unwrap();
        assert_eq!(Box::new(sink).into_value().unwrap(), Value::UInt(42));

        let mut stream2 = CombinedStream::ByteStream(ByteStream::new_write());
        node.exec_build(
            &ValueInput::new(&Value::UInt(42)),
            &mut stream2,
            &mut Context::new(),
        )
        .unwrap();
        assert_eq!(stream2.into_bytes(), vec![42]);
    }

    #[test]
    fn wrapper_const_exec_parse_checks_value() {
        let node = CompiledNode::Const(CompiledConst {
            inner: Box::new(compiled_u8()),
            value: Value::UInt(7),
        });
        // Matching value: succeeds.
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[7]));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        node.exec_parse(&mut stream, &mut ctx, &mut sink).unwrap();
        assert_eq!(Box::new(sink).into_value().unwrap(), Value::UInt(7));
        // Non-matching value: ConstError.
        let mut stream2 = CombinedStream::ByteStream(ByteStream::new_read(&[9]));
        let err = node
            .exec_parse(&mut stream2, &mut ctx, &mut ValueSink::new())
            .unwrap_err();
        assert!(matches!(err, ConstructError::Const { .. }));
    }

    #[test]
    fn wrapper_const_exec_build_ignores_input() {
        let node = CompiledNode::Const(CompiledConst {
            inner: Box::new(compiled_u8()),
            value: Value::UInt(42),
        });
        // build with None: writes the constant value.
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        node.exec_build(
            &ValueInput::new(&Value::None),
            &mut stream,
            &mut Context::new(),
        )
        .unwrap();
        assert_eq!(stream.into_bytes(), vec![42]);
        // build with wrong value: error.
        let mut stream2 = CombinedStream::ByteStream(ByteStream::new_write());
        let err = node
            .exec_build(
                &ValueInput::new(&Value::UInt(99)),
                &mut stream2,
                &mut Context::new(),
            )
            .unwrap_err();
        assert!(matches!(err, ConstructError::Const { .. }));
    }

    #[test]
    fn wrapper_adapter_exec_parse_applies_decode() {
        use crate::constructs::adapters::{DecodeFuncBox, EncodeFuncBox};
        // decode: multiply by 2; encode: divide by 2.
        let decode: DecodeFuncBox = Box::new(|v, _ctx| {
            let n = v.to_u64()?;
            Ok(Value::UInt(n * 2))
        });
        let encode: EncodeFuncBox = Box::new(|v, _ctx| {
            let n = v.to_u64()?;
            Ok(Value::UInt(n / 2))
        });
        let node = CompiledNode::Adapter(CompiledAdapter {
            inner: Box::new(compiled_u8()),
            decode: Arc::from(decode),
            encode: Arc::from(encode),
        });
        // Parse raw 5 → decode → 10.
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[5]));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        node.exec_parse(&mut stream, &mut ctx, &mut sink).unwrap();
        assert_eq!(Box::new(sink).into_value().unwrap(), Value::UInt(10));
        // Build 10 → encode → 5.
        let mut stream2 = CombinedStream::ByteStream(ByteStream::new_write());
        node.exec_build(
            &ValueInput::new(&Value::UInt(10)),
            &mut stream2,
            &mut Context::new(),
        )
        .unwrap();
        assert_eq!(stream2.into_bytes(), vec![5]);
    }

    #[test]
    fn wrapper_validator_exec_parse_checks_value() {
        use crate::constructs::adapters::CheckFuncBox;
        // check: value must be > 0.
        let check: CheckFuncBox = Box::new(|v, _ctx| {
            let n = v.to_u64()?;
            if n > 0 {
                Ok(())
            } else {
                Err(ConstructError::Generic {
                    path: String::new(),
                    message: "must be positive".to_string(),
                })
            }
        });
        let node = CompiledNode::Validator(CompiledValidator {
            inner: Box::new(compiled_u8()),
            check: Arc::from(check),
        });
        // Parse 5: passes validation.
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[5]));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        node.exec_parse(&mut stream, &mut ctx, &mut sink).unwrap();
        assert_eq!(Box::new(sink).into_value().unwrap(), Value::UInt(5));
        // Parse 0: fails validation.
        let mut stream2 = CombinedStream::ByteStream(ByteStream::new_read(&[0]));
        let err = node
            .exec_parse(&mut stream2, &mut ctx, &mut ValueSink::new())
            .unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    #[test]
    fn wrapper_enum_exec_parse_maps_int_to_string() {
        let mut decmap = IndexMap::new();
        decmap.insert(1u64, "one".to_string());
        decmap.insert(2u64, "two".to_string());
        let node = CompiledNode::Enum(CompiledEnum {
            inner: Box::new(compiled_u8()),
            mapping: IndexMap::new(),
            decmap,
        });
        // Parse 1 → "one".
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[1]));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        node.exec_parse(&mut stream, &mut ctx, &mut sink).unwrap();
        assert_eq!(
            Box::new(sink).into_value().unwrap(),
            Value::String("one".to_string())
        );
        // Parse unmapped 255 → raw integer.
        let mut stream2 = CombinedStream::ByteStream(ByteStream::new_read(&[255]));
        let mut sink2 = ValueSink::new();
        node.exec_parse(&mut stream2, &mut ctx, &mut sink2).unwrap();
        assert_eq!(Box::new(sink2).into_value().unwrap(), Value::UInt(255));
    }

    #[test]
    fn wrapper_flagsenum_exec_parse_expands_bits() {
        let mut flags = IndexMap::new();
        flags.insert("read".to_string(), 1u64);
        flags.insert("write".to_string(), 2u64);
        flags.insert("exec".to_string(), 4u64);
        let node = CompiledNode::FlagsEnum(CompiledFlagsEnum {
            inner: Box::new(compiled_u8()),
            flags,
        });
        // Parse 0b101 → read=true, write=false, exec=true.
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[0b101]));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        node.exec_parse(&mut stream, &mut ctx, &mut sink).unwrap();
        let container = Box::new(sink)
            .into_value()
            .unwrap()
            .as_container()
            .unwrap()
            .clone();
        assert_eq!(container.get("read").unwrap(), &Value::Bool(true));
        assert_eq!(container.get("write").unwrap(), &Value::Bool(false));
        assert_eq!(container.get("exec").unwrap(), &Value::Bool(true));
    }

    #[test]
    fn wrapper_mapping_exec_parse_and_build() {
        let node = CompiledNode::Mapping(CompiledMapping {
            inner: Box::new(compiled_u8()),
            mapping: vec![
                (Value::String("A".to_string()), Value::UInt(0)),
                (Value::String("B".to_string()), Value::UInt(1)),
            ],
            decmapping: vec![
                (Value::UInt(0), Value::String("A".to_string())),
                (Value::UInt(1), Value::String("B".to_string())),
            ],
        });
        // Parse 0 → "A".
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[0]));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        node.exec_parse(&mut stream, &mut ctx, &mut sink).unwrap();
        assert_eq!(
            Box::new(sink).into_value().unwrap(),
            Value::String("A".to_string())
        );
        // Build "B" → writes 1.
        let mut stream2 = CombinedStream::ByteStream(ByteStream::new_write());
        node.exec_build(
            &OwnedValueInput::new(Value::String("B".to_string())),
            &mut stream2,
            &mut Context::new(),
        )
        .unwrap();
        assert_eq!(stream2.into_bytes(), vec![1]);
    }

    #[test]
    fn wrapper_static_size_folds_to_inner() {
        // A wrapper around a fixed-size leaf folds the static size.
        let node = CompiledNode::Hex(CompiledHex {
            inner: Box::new(CompiledNode::FormatField(CompiledFormatField {
                inner: FormatField::new(Endianness::Big, FormatKind::U32),
            })),
        });
        let schema = CompiledSchema::new(node);
        assert_eq!(schema.static_size(), Some(4));
    }

    #[test]
    fn exec_dispatches_to_correct_variant() {
        // Leaves are functional; non-leaf stubs return the Generic error.
        // Verifies dispatch routes to the right variant.
        let ctx = Context::new();
        assert_eq!(compiled_pass().exec_sizeof(&ctx).unwrap(), 0);
        // Struct is now functional (Phase 12.6): empty Struct has size 0.
        assert_eq!(
            CompiledNode::Struct(CompiledStruct { fields: vec![] })
                .exec_sizeof(&ctx)
                .unwrap(),
            0
        );
        // Lazy is now functional (Phase 12.8): delegates to inner.
        let lazy_node = CompiledNode::Lazy(crate::compiled::CompiledLazy {
            inner: Box::new(compiled_pass()),
        });
        assert_eq!(lazy_node.exec_sizeof(&ctx).unwrap(), 0);
    }

    #[test]
    fn exec_dynamic_delegates_to_inner() {
        // CompiledDynamic is the functional escape-hatch (Phase 12.3):
        // exec_sizeof delegates to the wrapped construct. Pass has size 0.
        let node = CompiledNode::Dynamic(dynamic_pass());
        let ctx = Context::new();
        assert_eq!(node.exec_sizeof(&ctx).unwrap(), 0);
    }

    #[test]
    fn exec_dynamic_parse_delegates_to_inner() {
        // Wrapped Pass parses nothing (empty input), deposits None scalar.
        let node = CompiledNode::Dynamic(dynamic_pass());
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[]));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        node.exec_parse(&mut stream, &mut ctx, &mut sink).unwrap();
        let value = Box::new(sink).into_value().unwrap();
        assert_eq!(value, Value::None);
    }

    #[test]
    fn exec_dynamic_build_delegates_to_inner() {
        // Wrapped Pass builds nothing (empty output).
        let node = CompiledNode::Dynamic(dynamic_pass());
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        node.exec_build(&ValueInput::new(&Value::None), &mut stream, &mut ctx)
            .unwrap();
        assert!(stream.into_bytes().is_empty());
    }

    // -- CompiledSchema -----------------------------------------------------

    #[test]
    fn compiled_schema_new_with_pass_has_static_size_zero() {
        let schema = CompiledSchema::new(compiled_pass());
        assert_eq!(schema.static_size(), Some(0));
        assert!(schema.name().is_none());
    }

    #[test]
    fn compiled_schema_with_name_sets_name() {
        let schema = CompiledSchema::new(compiled_pass()).with_name("my_schema");
        assert_eq!(schema.name(), Some("my_schema"));
    }

    #[test]
    fn compiled_schema_tree_returns_root() {
        let schema = CompiledSchema::new(compiled_pass());
        assert!(matches!(schema.tree(), CompiledNode::Pass(_)));
    }

    #[test]
    fn compiled_schema_parse_bytes_pass_returns_none() {
        // Pass is now functional: parse_bytes returns Value::None (no error).
        let schema = CompiledSchema::new(compiled_pass());
        let value = schema.parse_bytes(b"\x01\x02").unwrap();
        assert_eq!(value, Value::None);
    }

    #[test]
    fn compiled_schema_build_bytes_pass_returns_empty() {
        let schema = CompiledSchema::new(compiled_pass());
        let bytes = schema.build_bytes(&Value::None).unwrap();
        assert!(bytes.is_empty());
    }

    #[test]
    fn compiled_schema_sizeof_pass_is_zero() {
        let schema = CompiledSchema::new(compiled_pass());
        let ctx = Context::new();
        assert_eq!(schema.sizeof(&ctx).unwrap(), 0);
    }

    #[test]
    fn compiled_schema_sizeof_format_field_uses_static_fold() {
        // Static size is folded at compile time → sizeof returns it directly.
        let schema = CompiledSchema::new(CompiledNode::FormatField(CompiledFormatField {
            inner: FormatField::new(Endianness::Big, FormatKind::U16),
        }));
        let ctx = Context::new();
        assert_eq!(schema.sizeof(&ctx).unwrap(), 2);
    }

    #[test]
    fn compiled_schema_sizeof_struct_folds_fields() {
        // Struct is now functional (Phase 12.6): empty Struct has static size 0.
        let schema = CompiledSchema::new(CompiledNode::Struct(CompiledStruct { fields: vec![] }));
        assert_eq!(schema.static_size(), Some(0));
        let ctx = Context::new();
        assert_eq!(schema.sizeof(&ctx).unwrap(), 0);
    }

    #[test]
    fn compiled_schema_debug_formats() {
        let schema = CompiledSchema::new(compiled_pass());
        let _ = format!("{schema:?}");
    }

    // -- Static size folding ------------------------------------------------

    #[test]
    fn try_fold_static_size_fixed_leaves() {
        assert_eq!(try_fold_static_size(&compiled_pass()), Some(0));
        assert_eq!(
            try_fold_static_size(&CompiledNode::Bytes(CompiledBytes {
                inner: Bytes::new(5),
            })),
            Some(5)
        );
        assert_eq!(
            try_fold_static_size(&CompiledNode::FormatField(CompiledFormatField {
                inner: FormatField::new(Endianness::Big, FormatKind::U32),
            })),
            Some(4)
        );
        assert_eq!(
            try_fold_static_size(&CompiledNode::Flag(CompiledFlag { inner: Flag::new() })),
            Some(1)
        );
    }

    #[test]
    fn try_fold_static_size_variable_leaves_and_stubs_return_none() {
        assert_eq!(
            try_fold_static_size(&CompiledNode::VarInt(CompiledVarInt {
                inner: VarInt::new(),
            })),
            None
        );
        // Lazy folds to inner size (Phase 12.8). Wrapping Pass (size 0).
        assert_eq!(
            try_fold_static_size(&CompiledNode::Lazy(crate::compiled::CompiledLazy {
                inner: Box::new(compiled_pass()),
            })),
            Some(0)
        );
        // Empty Struct folds to 0 (Phase 12.6).
        assert_eq!(
            try_fold_static_size(&CompiledNode::Struct(CompiledStruct { fields: vec![] })),
            Some(0)
        );
    }

    // -- Variant count sanity check -----------------------------------------

    #[test]
    fn all_major_variants_are_constructible() {
        // Smoke test: construct one of each variant category to ensure
        // the enum compiles and enum_dispatch generates valid dispatch.
        // Wrappers need a compiled inner node (Phase 12.5).
        let inner_leaf = Box::new(compiled_pass());
        let _ = CompiledNode::Subconstruct(CompiledSubconstruct { inner: inner_leaf });
        let _ = CompiledNode::FormatField(CompiledFormatField {
            inner: FormatField::new(Endianness::Big, FormatKind::U8),
        });
        let decode: crate::constructs::adapters::DecodeFuncBox = Box::new(|v, _ctx| Ok(v.clone()));
        let encode: crate::constructs::adapters::EncodeFuncBox = Box::new(|v, _ctx| Ok(v.clone()));
        let _ = CompiledNode::Adapter(CompiledAdapter {
            inner: Box::new(compiled_pass()),
            decode: Arc::from(decode),
            encode: Arc::from(encode),
        });
        let _ = compiled_pass();
        let _ = CompiledNode::Struct(CompiledStruct { fields: vec![] });
        let _ = CompiledNode::Enum(CompiledEnum {
            inner: Box::new(compiled_pass()),
            mapping: IndexMap::new(),
            decmap: IndexMap::new(),
        });
        let _ = CompiledNode::Array(CompiledArray {
            count: 0,
            subcon: Box::new(compiled_pass()),
            discard: false,
        });
        let _ = CompiledNode::Lazy(crate::compiled::CompiledLazy {
            inner: Box::new(compiled_pass()),
        });
        let _ = CompiledNode::Bitwise(CompiledBitwise {
            inner: Box::new(compiled_pass()),
        });
        let _ = CompiledNode::IfThenElse(CompiledIfThenElse {
            cond: Arc::new(|_ctx| false),
            then_constr: Box::new(compiled_pass()),
            else_constr: Box::new(compiled_pass()),
        });
        let _ = CompiledNode::Hex(CompiledHex {
            inner: Box::new(compiled_pass()),
        });
        let _ = CompiledNode::Dynamic(dynamic_pass());
    }

    // -- Composite exec tests (Phase 12.6) ---------------------------------

    /// Helper: compile a declaration construct into a CompiledNode.
    fn compile_cc(cc: CombinedConstruct) -> CompiledNode {
        cc.compile().expect("compile should succeed")
    }

    #[test]
    fn composite_struct_exec_parse_and_build_roundtrip() {
        // Struct { a: u8, b: u16<BE> }
        let st = Struct::new()
            .field("a", Box::new(INT8UB.into()))
            .field("b", Box::new(INT16UB.into()));
        let node = compile_cc(st.into());

        // Parse 0x01 0x02 0x03 → { a: 1, b: 515 }
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[0x01, 0x02, 0x03]));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        node.exec_parse(&mut stream, &mut ctx, &mut sink).unwrap();
        let val = Box::new(sink).into_value().unwrap();
        let container = match &val {
            Value::Container(c) => c,
            other => panic!("expected Container, got {other:?}"),
        };
        assert_eq!(container.get("a").unwrap(), &Value::UInt(1));
        assert_eq!(container.get("b").unwrap(), &Value::UInt(0x0203));

        // Build back: { a: 1, b: 515 } → 0x01 0x02 0x03
        let mut stream2 = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx2 = Context::new();
        node.exec_build(&ValueInput::new(&val), &mut stream2, &mut ctx2)
            .unwrap();
        assert_eq!(stream2.into_bytes(), vec![0x01, 0x02, 0x03]);
    }

    #[test]
    fn composite_struct_exec_sizeof_sums_fields() {
        let st = Struct::new()
            .field("a", Box::new(INT8UB.into()))
            .field("b", Box::new(INT16UB.into()));
        let node = compile_cc(st.into());
        let ctx = Context::new();
        assert_eq!(node.exec_sizeof(&ctx).unwrap(), 3);
    }

    #[test]
    fn composite_struct_anonymous_field_exec_parse() {
        // Anonymous field (name=None) is parsed but not stored.
        let st = Struct::new()
            .anonymous(Box::new(INT8UB.into()))
            .field("x", Box::new(INT8UB.into()));
        let node = compile_cc(st.into());

        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[0xFF, 0x42]));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        node.exec_parse(&mut stream, &mut ctx, &mut sink).unwrap();
        let val = Box::new(sink).into_value().unwrap();
        let container = match val {
            Value::Container(c) => c,
            other => panic!("expected Container, got {other:?}"),
        };
        assert_eq!(container.len(), 1); // only "x"
        assert_eq!(container.get("x").unwrap(), &Value::UInt(0x42));
    }

    #[test]
    fn composite_sequence_exec_parse_and_build_roundtrip() {
        // Sequence of (u8, u8, u8)
        let seq = Sequence::new()
            .push(Box::new(INT8UB.into()))
            .push(Box::new(INT8UB.into()))
            .push(Box::new(INT8UB.into()));
        let node = compile_cc(seq.into());

        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[10, 20, 30]));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        node.exec_parse(&mut stream, &mut ctx, &mut sink).unwrap();
        let val = Box::new(sink).into_value().unwrap();
        let list = match &val {
            Value::List(l) => l,
            other => panic!("expected List, got {other:?}"),
        };
        assert_eq!(list.len(), 3);
        assert_eq!(list[0], Value::UInt(10));
        assert_eq!(list[1], Value::UInt(20));
        assert_eq!(list[2], Value::UInt(30));

        // Build back
        let mut stream2 = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx2 = Context::new();
        node.exec_build(&ValueInput::new(&val), &mut stream2, &mut ctx2)
            .unwrap();
        assert_eq!(stream2.into_bytes(), vec![10, 20, 30]);
    }

    #[test]
    fn composite_sequence_exec_sizeof_sums_entries() {
        let seq = Sequence::new()
            .push(Box::new(INT8UB.into()))
            .push(Box::new(INT16UB.into()));
        let node = compile_cc(seq.into());
        let ctx = Context::new();
        assert_eq!(node.exec_sizeof(&ctx).unwrap(), 3);
    }

    #[test]
    fn composite_array_exec_parse_and_build_roundtrip() {
        // Array of 3 × u8
        let arr = Array::new(3, Box::new(INT8UB.into()));
        let node = compile_cc(arr.into());

        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[1, 2, 3]));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        node.exec_parse(&mut stream, &mut ctx, &mut sink).unwrap();
        let val = Box::new(sink).into_value().unwrap();
        let list = match &val {
            Value::List(l) => l,
            other => panic!("expected List, got {other:?}"),
        };
        assert_eq!(*list, vec![Value::UInt(1), Value::UInt(2), Value::UInt(3)]);

        // Build back
        let mut stream2 = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx2 = Context::new();
        node.exec_build(&ValueInput::new(&val), &mut stream2, &mut ctx2)
            .unwrap();
        assert_eq!(stream2.into_bytes(), vec![1, 2, 3]);
    }

    #[test]
    fn composite_array_exec_sizeof() {
        let arr = Array::new(4, Box::new(INT8UB.into()));
        let node = compile_cc(arr.into());
        let ctx = Context::new();
        assert_eq!(node.exec_sizeof(&ctx).unwrap(), 4);
    }

    #[test]
    fn composite_array_discard_exec_parse_returns_empty_list() {
        // Discard array: parsed items are thrown away; result is empty list.
        let arr = Array::new_discard(2, Box::new(INT8UB.into()));
        let node = compile_cc(arr.into());

        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[0xAA, 0xBB]));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        node.exec_parse(&mut stream, &mut ctx, &mut sink).unwrap();
        let val = Box::new(sink).into_value().unwrap();
        assert_eq!(val, Value::List(vec![]));
    }

    #[test]
    fn composite_greedy_range_exec_parse_reads_until_eof() {
        let gr = GreedyRange::new(Box::new(INT8UB.into()));
        let node = compile_cc(gr.into());

        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[1, 2, 3, 4]));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        node.exec_parse(&mut stream, &mut ctx, &mut sink).unwrap();
        let val = Box::new(sink).into_value().unwrap();
        let list = match val {
            Value::List(l) => l,
            other => panic!("expected List, got {other:?}"),
        };
        assert_eq!(list.len(), 4);
        assert_eq!(list[0], Value::UInt(1));
        assert_eq!(list[3], Value::UInt(4));
    }

    #[test]
    fn composite_greedy_range_exec_build_writes_all_items() {
        let gr = GreedyRange::new(Box::new(INT8UB.into()));
        let node = compile_cc(gr.into());

        let input = Value::List(vec![Value::UInt(10), Value::UInt(20), Value::UInt(30)]);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        node.exec_build(&ValueInput::new(&input), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), vec![10, 20, 30]);
    }

    #[test]
    fn composite_select_exec_parse_matches_first_subcon() {
        // Select with two subcons; INT8UB matches first.
        let sel = Select::new(vec![INT8UB.into(), INT16UB.into()]);
        let node = compile_cc(sel.into());

        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[0x07]));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        node.exec_parse(&mut stream, &mut ctx, &mut sink).unwrap();
        let val = Box::new(sink).into_value().unwrap();
        assert_eq!(val, Value::UInt(7));
    }

    #[test]
    fn composite_select_exec_parse_falls_through_to_second() {
        // First subcon fails (not enough bytes), second succeeds.
        let sel = Select::new(vec![INT16UB.into(), INT8UB.into()]);
        let node = compile_cc(sel.into());

        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[0x07]));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        node.exec_parse(&mut stream, &mut ctx, &mut sink).unwrap();
        let val = Box::new(sink).into_value().unwrap();
        assert_eq!(val, Value::UInt(7));
    }

    #[test]
    fn composite_select_exec_parse_all_fail_returns_error() {
        let sel = Select::new(vec![INT16UB.into()]);
        let node = compile_cc(sel.into());

        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[]));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        let err = node
            .exec_parse(&mut stream, &mut ctx, &mut sink)
            .unwrap_err();
        assert!(matches!(err, ConstructError::Select { .. }));
    }

    #[test]
    fn composite_union_exec_parse_reads_selected_field() {
        // Union with parsefrom = index 1; input has 2 bytes for both fields.
        // Field 0 (INT16UB, big-endian) reads [0xAA, 0xBB] = 0xAABB.
        // Field 1 (INT8UB) reads [0xAA] = 0xAA = 170.
        let uni = Union::new(
            Some(crate::constructs::union::UnionTarget::Index(1)),
            vec![],
        )
        .field("a", Box::new(INT16UB.into()))
        .field("b", Box::new(INT8UB.into()));
        let node = compile_cc(uni.into());

        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[0xAA, 0xBB]));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        node.exec_parse(&mut stream, &mut ctx, &mut sink).unwrap();
        let val = Box::new(sink).into_value().unwrap();
        let container = match &val {
            Value::Container(c) => c,
            other => panic!("expected Container, got {other:?}"),
        };
        // Both fields are parsed and stored; stream advances to field 1's position.
        assert_eq!(container.get("a").unwrap(), &Value::UInt(0xAABB));
        assert_eq!(container.get("b").unwrap(), &Value::UInt(0xAA));
        // Stream should be at position 1 (field 1's forward position).
        assert_eq!(stream.tell().unwrap(), 1);
    }

    #[test]
    fn composite_focused_seq_exec_parse_returns_focused_value() {
        // FocusedSeq on field "x": Struct { dummy: u8, x: u8 }
        let fs = FocusedSeq::new(
            "x",
            vec![
                StructField::new("dummy", Box::new(INT8UB.into())),
                StructField::new("x", Box::new(INT8UB.into())),
            ],
        );
        let node = compile_cc(fs.into());

        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[0xFF, 0x42]));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        node.exec_parse(&mut stream, &mut ctx, &mut sink).unwrap();
        let val = Box::new(sink).into_value().unwrap();
        assert_eq!(val, Value::UInt(0x42));
    }

    #[test]
    fn composite_nested_struct_exec_parse_roundtrip() {
        // Outer { a: u8, inner: Struct { b: u8 } }
        let inner = Struct::new().field("b", Box::new(INT8UB.into()));
        let outer = Struct::new()
            .field("a", Box::new(INT8UB.into()))
            .field("inner", Box::new(inner.into()));
        let node = compile_cc(outer.into());

        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[1, 2]));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        node.exec_parse(&mut stream, &mut ctx, &mut sink).unwrap();
        let val = Box::new(sink).into_value().unwrap();
        let container = match &val {
            Value::Container(c) => c,
            other => panic!("expected Container, got {other:?}"),
        };
        assert_eq!(container.get("a").unwrap(), &Value::UInt(1));
        let inner_val = container.get("inner").unwrap();
        let inner_container = match inner_val {
            Value::Container(c) => c,
            other => panic!("expected inner Container, got {other:?}"),
        };
        assert_eq!(inner_container.get("b").unwrap(), &Value::UInt(2));

        // Build back
        let mut stream2 = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx2 = Context::new();
        node.exec_build(&ValueInput::new(&val), &mut stream2, &mut ctx2)
            .unwrap();
        assert_eq!(stream2.into_bytes(), vec![1, 2]);
    }

    #[test]
    fn composite_array_of_structs_exec_parse_roundtrip() {
        // Array(2, Struct { x: u8 })
        let elem = Struct::new().field("x", Box::new(INT8UB.into()));
        let arr = Array::new(2, Box::new(elem.into()));
        let node = compile_cc(arr.into());

        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[10, 20]));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        node.exec_parse(&mut stream, &mut ctx, &mut sink).unwrap();
        let val = Box::new(sink).into_value().unwrap();
        let list = match &val {
            Value::List(l) => l,
            other => panic!("expected List, got {other:?}"),
        };
        assert_eq!(list.len(), 2);

        // Build back
        let mut stream2 = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx2 = Context::new();
        node.exec_build(&ValueInput::new(&val), &mut stream2, &mut ctx2)
            .unwrap();
        assert_eq!(stream2.into_bytes(), vec![10, 20]);
    }

    #[test]
    fn composite_struct_with_buildnone_field_exec_parse() {
        // Struct { a: u8, "skip" / Pass } — Pass has flagbuildnone=true
        let st = Struct::new()
            .field("a", Box::new(INT8UB.into()))
            .field("skip", Box::new(Pass::new().into()));
        let node = compile_cc(st.into());

        // The compiled Struct should have 2 fields, second with flagbuildnone=true
        match &node {
            CompiledNode::Struct(cs) => {
                assert_eq!(cs.fields.len(), 2);
                assert!(cs.fields[1].flagbuildnone);
            }
            other => panic!("expected CompiledStruct, got {other:?}"),
        }

        // Parse reads only "a"; "skip" gets Value::None
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[0x05]));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        node.exec_parse(&mut stream, &mut ctx, &mut sink).unwrap();
        let val = Box::new(sink).into_value().unwrap();
        let container = match val {
            Value::Container(c) => c,
            other => panic!("expected Container, got {other:?}"),
        };
        assert_eq!(container.get("a").unwrap(), &Value::UInt(5));
    }

    #[test]
    fn composite_struct_exec_build_missing_field_returns_error() {
        // Struct with a required field; building with missing field → error.
        let st = Struct::new().field("a", Box::new(INT8UB.into()));
        let node = compile_cc(st.into());

        // Build with empty container → should error (FieldMissing)
        let input = Value::Container(IndexMap::new());
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        let err = node
            .exec_build(&ValueInput::new(&input), &mut stream, &mut ctx)
            .unwrap_err();
        assert!(matches!(err, ConstructError::FieldMissing { .. }));
    }

    // -- Phase 13: CompiledExternal + CompiledExtension ----------------------

    /// Test CompiledExtension implementation that always produces a fixed
    /// Value on parse, writes a fixed byte on build, and has size 1.
    #[derive(Debug)]
    struct TestExtension {
        parse_value: Value,
        build_byte: u8,
        size: usize,
    }

    impl CompiledExtension for TestExtension {
        fn ext_parse(
            &self,
            _stream: &mut CombinedStream,
            _ctx: &mut Context,
        ) -> Result<ProducedOutput> {
            Ok(ProducedOutput::Value(self.parse_value.clone()))
        }

        fn ext_build(
            &self,
            _input: &dyn Input,
            stream: &mut CombinedStream,
            _ctx: &mut Context,
        ) -> Result<()> {
            stream.write_bytes(&[self.build_byte])
        }

        fn ext_sizeof(&self) -> Result<usize> {
            Ok(self.size)
        }
    }

    #[test]
    fn external_variant_exec_parse_delegates_to_extension() {
        let ext = TestExtension {
            parse_value: Value::UInt(42),
            build_byte: 0,
            size: 1,
        };
        let node = CompiledNode::External(CompiledExternal {
            inner: Box::new(ext),
        });
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[]));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        node.exec_parse(&mut stream, &mut ctx, &mut sink).unwrap();
        let val = Box::new(sink).into_value().unwrap();
        assert_eq!(val, Value::UInt(42));
    }

    #[test]
    fn external_variant_exec_build_delegates_to_extension() {
        let ext = TestExtension {
            parse_value: Value::None,
            build_byte: 0xAB,
            size: 1,
        };
        let node = CompiledNode::External(CompiledExternal {
            inner: Box::new(ext),
        });
        let input = ValueInput::new(&Value::None);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        node.exec_build(&input, &mut stream, &mut ctx).unwrap();
        assert_eq!(stream.into_bytes(), vec![0xAB]);
    }

    #[test]
    fn external_variant_exec_sizeof_delegates_to_extension() {
        let ext = TestExtension {
            parse_value: Value::None,
            build_byte: 0,
            size: 7,
        };
        let node = CompiledNode::External(CompiledExternal {
            inner: Box::new(ext),
        });
        let ctx = Context::new();
        assert_eq!(node.exec_sizeof(&ctx).unwrap(), 7);
    }

    #[test]
    fn external_variant_debug_formats() {
        let ext = TestExtension {
            parse_value: Value::None,
            build_byte: 0,
            size: 0,
        };
        let node = CompiledNode::External(CompiledExternal {
            inner: Box::new(ext),
        });
        let _ = format!("{node:?}");
    }

    #[test]
    fn external_variant_is_constructible_in_enum() {
        let ext = TestExtension {
            parse_value: Value::None,
            build_byte: 0,
            size: 0,
        };
        let node = CompiledNode::External(CompiledExternal {
            inner: Box::new(ext),
        });
        assert!(matches!(node, CompiledNode::External(_)));
    }
}
