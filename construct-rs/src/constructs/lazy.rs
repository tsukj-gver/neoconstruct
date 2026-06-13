//! Lazy parsing constructs — deferred/on-demand parsing of fields and elements.
//!
//! This module provides lazy equivalents of the standard composite constructs.
//! In the Python original, these constructs return special container objects
//! (`LazyContainer`, `LazyListContainer`) that hold a reference to the stream
//! and only parse individual fields/elements when they are first accessed.
//!
//! # Rust simplification
//!
//! Rust's ownership model makes it impractical to return a [`Value`] that
//! internally borrows the `&mut dyn Stream` (the stream is borrowed mutably
//! for the duration of `parse` and cannot be stored inside the returned
//! value). Additionally, [`Value`] is an enum without a "thunk"/"lazy"
//! variant.
//!
//! For these reasons, the current implementations use a **simplified eager
//! strategy**: `parse` consumes the bytes immediately (exactly like their
//! non-lazy counterparts) but exposes the lazy *type* and *interface*. True
//! on-demand deferral is deferred to a future optimization phase, where a
//! dedicated `Value::Lazy` variant or a `LazyValue` wrapper holding an owned
//! byte snapshot + sub-construct reference can be introduced.
//!
//! Despite the eager evaluation, the round-trip (`build` → `parse`) symmetry
//! and overall semantics match the non-lazy equivalents exactly, so these
//! constructs are drop-in compatible with their eager counterparts.
//!
//! # Constructs
//!
//! | Construct | Eager counterpart | Purpose |
//! |-----------|-------------------|---------|
//! | [`Lazy`] | (none) | Wraps a single sub-construct with lazy semantics |
//! | [`LazyStruct`] | [`Struct`] | Struct whose fields are (notionally) parsed on demand |
//! | [`LazyArray`] | [`Array`] | Array whose elements are (notionally) parsed on demand |
//! | [`LazyBound`] | (none) | Binds to a sub-construct at runtime (recursive structures) |
//! | [`Rebuffered`] | (none) | Buffers a stream so it becomes seekable |
//!
//! [`Struct`]: crate::constructs::struct_::Struct
//! [`Array`]: crate::constructs::repetition::Array

use crate::constructs::repetition::Array;
use crate::constructs::struct_::Struct;
use crate::core::context::Context;
use crate::core::error::Result;
use crate::core::stream::{ByteStream, Stream};
use crate::core::Construct;
use crate::value::Value;

// `LazyBound` was already implemented in `stream_ops` (Phase 6). Re-export it
// from this module so all lazy constructs live under a single path.
pub use crate::constructs::stream_ops::LazyBound;

// ===========================================================================
// Lazy
// ===========================================================================

/// Lazily wraps a single sub-construct.
///
/// In the Python original, `Lazy(subcon)` records the stream offset and
/// returns a zero-argument callable; the sub-construct is only parsed when
/// that callable is invoked. This allows individual fields inside a regular
/// `Struct` to be deferred.
///
/// # Rust simplification
///
/// Because [`Value`] cannot hold a borrow of the `&mut dyn Stream`, this
/// implementation parses the sub-construct **eagerly** during `parse`. The
/// offset is still recorded (for diagnostic parity) but the value is consumed
/// immediately. The `Lazy` type is preserved so that API contracts match the
/// Python version and so that a true lazy implementation can be swapped in
/// later without changing call sites.
///
/// - **parse**: records the offset, parses `subcon` immediately, returns its
///   [`Value`].
/// - **build**: delegates to `subcon`. If `data` is callable in Python it
///   would be evaluated first; in Rust values are always materialized so this
///   is a plain delegation.
/// - **sizeof**: delegates to `subcon`.
///
/// Corresponds to Python `Lazy(subcon)`.
///
/// # Example
///
/// ```
/// use construct::constructs::lazy::Lazy;
/// use construct::constructs::format_field::INT8UB;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let d = Lazy::new(Box::new(INT8UB));
/// let c: &dyn Construct = &d;
/// let parsed = c.parse_bytes(b"\x2a").unwrap();
/// assert_eq!(parsed, Value::UInt(42));
/// ```
pub struct Lazy {
    /// The inner construct that is (notionally) parsed on demand.
    pub subcon: Box<dyn Construct>,
}

impl Lazy {
    /// Creates a new `Lazy` wrapper around `subcon`.
    ///
    /// # Example
    ///
    /// ```
    /// use construct::constructs::lazy::Lazy;
    /// use construct::constructs::format_field::INT8UB;
    ///
    /// let d = Lazy::new(Box::new(INT8UB));
    /// ```
    pub fn new(subcon: Box<dyn Construct>) -> Self {
        Lazy { subcon }
    }
}

impl Construct for Lazy {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        // Record the offset for diagnostic parity with the Python version.
        // The actual parse happens eagerly because `Value` cannot hold a
        // borrow of the stream.
        let _offset = stream.tell()?;
        self.subcon.parse(stream, ctx)
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        self.subcon.build(data, stream, ctx)
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        self.subcon.sizeof(ctx)
    }
}

// ===========================================================================
// LazyStruct
// ===========================================================================

/// A struct whose fields are (notionally) parsed on demand.
///
/// `LazyStruct` is the lazy counterpart of [`Struct`]. In the Python original
/// it returns a `LazyContainer` that only parses a field the first time it is
/// accessed, skipping fixed-size fields entirely until needed.
///
/// # Rust simplification
///
/// Because [`Value::Container`] cannot hold deferred thunks, this
/// implementation parses **all** fields eagerly — exactly like [`Struct`] —
/// and returns a regular [`Value::Container`]. The type exists so that lazy
/// call sites compile and so a true lazy implementation can be introduced
/// later without changing the public API.
///
/// - **parse**: parses every field into a [`Value::Container`] (eager).
/// - **build**: identical to [`Struct`].
/// - **sizeof**: identical to [`Struct`].
///
/// Corresponds to Python `LazyStruct(*subcons, **subconskw)`.
///
/// # Example
///
/// ```
/// use construct::constructs::lazy::LazyStruct;
/// use construct::constructs::format_field::INT8UB;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let s = LazyStruct::new()
///     .field("a", Box::new(INT8UB))
///     .field("b", Box::new(INT8UB));
///
/// let c: &dyn Construct = &s;
/// let parsed = c.parse_bytes(b"\x01\x02").unwrap();
/// let container = parsed.as_container().unwrap();
/// assert_eq!(container.get("a").unwrap(), &Value::UInt(1));
/// assert_eq!(container.get("b").unwrap(), &Value::UInt(2));
/// ```
///
/// [`Struct`]: crate::constructs::struct_::Struct
pub struct LazyStruct {
    /// The inner eager `Struct` that provides the field storage and
    /// parse/build/sizeof logic.
    inner: Struct,
}

impl LazyStruct {
    /// Creates a new, empty `LazyStruct`.
    ///
    /// Use [`field`](LazyStruct::field) and [`anonymous`](LazyStruct::anonymous)
    /// to add fields.
    pub fn new() -> Self {
        LazyStruct {
            inner: Struct::new(),
        }
    }

    /// Adds a named field, returning `self` for chaining.
    ///
    /// See [`Struct::field`](crate::constructs::struct_::Struct::field) for
    /// details on named-field semantics.
    pub fn field(mut self, name: impl Into<String>, subcon: Box<dyn Construct>) -> Self {
        self.inner = self.inner.field(name, subcon);
        self
    }

    /// Adds an anonymous field, returning `self` for chaining.
    ///
    /// See [`Struct::anonymous`](crate::constructs::struct_::Struct::anonymous)
    /// for details on anonymous-field semantics.
    pub fn anonymous(mut self, subcon: Box<dyn Construct>) -> Self {
        self.inner = self.inner.anonymous(subcon);
        self
    }
}

impl Default for LazyStruct {
    fn default() -> Self {
        Self::new()
    }
}

impl Construct for LazyStruct {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        // Simplified eager parse — delegates entirely to the inner Struct.
        self.inner.parse(stream, ctx)
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        self.inner.build(data, stream, ctx)
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        self.inner.sizeof(ctx)
    }

    fn flagbuildnone(&self) -> bool {
        self.inner.flagbuildnone()
    }
}

// ===========================================================================
// LazyArray
// ===========================================================================

/// An array whose elements are (notionally) parsed on demand.
///
/// `LazyArray` is the lazy counterpart of [`Array`]. In the Python original
/// it returns a `LazyListContainer` that records per-element offsets and only
/// parses an element the first time it is indexed.
///
/// # Rust simplification
///
/// Because [`Value::List`] cannot hold deferred thunks, this implementation
/// parses **all** elements eagerly — exactly like [`Array`] — and returns a
/// regular [`Value::List`]. The type is preserved for API compatibility and
/// future lazy optimization.
///
/// - **parse**: parses exactly `count` elements into a [`Value::List`].
/// - **build**: identical to [`Array`], including the count check.
/// - **sizeof**: `count × subcon.sizeof()`.
///
/// Corresponds to Python `LazyArray(count, subcon)`.
///
/// # Example
///
/// ```
/// use construct::constructs::lazy::LazyArray;
/// use construct::constructs::format_field::INT8UB;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let d = LazyArray::new(3, Box::new(INT8UB));
/// let c: &dyn Construct = &d;
/// let parsed = c.parse_bytes(b"\x01\x02\x03").unwrap();
/// assert_eq!(parsed, Value::List(vec![
///     Value::UInt(1), Value::UInt(2), Value::UInt(3),
/// ]));
/// ```
///
/// [`Array`]: crate::constructs::repetition::Array
pub struct LazyArray {
    /// The inner eager `Array` that provides element storage and
    /// parse/build/sizeof logic.
    inner: Array,
}

impl LazyArray {
    /// Creates a new `LazyArray` that processes exactly `count` elements
    /// using `subcon`.
    pub fn new(count: usize, subcon: Box<dyn Construct>) -> Self {
        LazyArray {
            inner: Array::new(count, subcon),
        }
    }

    /// Returns the fixed element count.
    #[must_use]
    pub fn count(&self) -> usize {
        self.inner.count
    }
}

impl Construct for LazyArray {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        // Simplified eager parse — delegates entirely to the inner Array.
        self.inner.parse(stream, ctx)
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        self.inner.build(data, stream, ctx)
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        self.inner.sizeof(ctx)
    }
}

// ===========================================================================
// Rebuffered
// ===========================================================================

/// Buffers an underlying stream so that the sub-construct sees a seekable,
/// fully in-memory stream.
///
/// In the Python original, `Rebuffered` wraps a non-seekable stream (e.g. a
/// pipe or socket) in a `RebufferedBytesIO` that caches bytes as they are
/// read, making `seek`/`tell` work over the already-buffered region. This is
/// useful when a sub-construct needs random access but the data source only
/// supports sequential reading.
///
/// # Rust implementation
///
/// The Rust [`Stream`](crate::core::stream::Stream) trait already requires
/// `seek`/`tell` support, so buffering is mainly relevant when a sub-construct
/// needs random access over data that should be snapshotted up front.
///
/// This implementation buffers **all remaining bytes** of the stream into an
/// in-memory [`ByteStream`] (matching the Python default where `tailcutoff` is
/// `None`, meaning "buffer everything"), parses the sub-construct from that
/// buffered stream, and then repositions the original stream to immediately
/// after the bytes the sub-construct consumed. The `tailcutoff` parameter is
/// accepted for API parity but, in this simplified version, does not limit the
/// buffer (a streaming, chunked buffer is reserved for a future optimization).
///
/// - **parse**: snapshots the remaining stream into a buffer, parses `subcon`
///   from it, then advances the original stream past the consumed bytes.
/// - **build**: delegates to `subcon` (sequential writes need no buffering).
/// - **sizeof**: delegates to `subcon`.
///
/// Corresponds to Python `Rebuffered(subcon, tailcutoff=None)`.
///
/// # Example
///
/// ```
/// use construct::constructs::lazy::Rebuffered;
/// use construct::constructs::format_field::INT8UB;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let d = Rebuffered::new(Box::new(INT8UB));
/// let c: &dyn Construct = &d;
/// let parsed = c.parse_bytes(b"\x05garbage").unwrap();
/// assert_eq!(parsed, Value::UInt(5));
/// ```
pub struct Rebuffered {
    /// The inner construct operating on the buffered stream.
    pub subcon: Box<dyn Construct>,
    /// Optional maximum number of bytes to retain in the buffer. In this
    /// simplified implementation the value is accepted but the buffer is not
    /// truncated (all remaining bytes are buffered), matching the Python
    /// default of "buffer everything".
    pub tailcutoff: Option<usize>,
}

impl Rebuffered {
    /// Creates a new `Rebuffered` with no tail cutoff (buffers everything).
    pub fn new(subcon: Box<dyn Construct>) -> Self {
        Rebuffered {
            subcon,
            tailcutoff: None,
        }
    }

    /// Sets the `tailcutoff` hint.
    ///
    /// In this simplified implementation the cutoff is recorded for API parity
    /// but the buffer still holds all remaining bytes.
    pub fn with_tailcutoff(mut self, tailcutoff: usize) -> Self {
        self.tailcutoff = Some(tailcutoff);
        self
    }
}

impl Construct for Rebuffered {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        // Record where we started in the real stream.
        let start = stream.tell()?;

        // Buffer all remaining bytes (matches Python default: buffer everything).
        let data = stream.read_remaining()?;

        // Parse the sub-construct from the fully-buffered, seekable stream.
        let mut buffered = ByteStream::new_read(&data);
        let result = self.subcon.parse(&mut buffered, ctx)?;

        // Reposition the original stream to just past the bytes the sub-construct
        // actually consumed, so subsequent constructs see the correct cursor.
        let consumed = buffered.tell()?;
        stream.seek(start.saturating_add(consumed))?;

        Ok(result)
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        // Sequential writes do not benefit from buffering, so delegate directly.
        self.subcon.build(data, stream, ctx)
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        self.subcon.sizeof(ctx)
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constructs::bytes::Bytes;
    use crate::constructs::format_field::{INT16UB, INT8UB};
    use crate::constructs::meta::Pass;
    use crate::core::error::ConstructError;
    use indexmap::IndexMap;

    // ======================================================================
    // Lazy tests
    // ======================================================================

    #[test]
    fn lazy_parse_returns_value() {
        let d = Lazy::new(Box::new(INT8UB));
        let c: &dyn Construct = &d;
        let parsed = c.parse_bytes(b"\x2a").unwrap();
        assert_eq!(parsed, Value::UInt(42));
    }

    #[test]
    fn lazy_parse_advances_stream() {
        let d = Lazy::new(Box::new(INT8UB));
        let mut stream = ByteStream::new_read(b"\x01\x02");
        let mut ctx = Context::new();
        let parsed = d.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(parsed, Value::UInt(1));
        // The byte should be consumed even though the parse is "lazy".
        assert_eq!(stream.tell().unwrap(), 1);
    }

    #[test]
    fn lazy_build_delegates() {
        let d = Lazy::new(Box::new(INT8UB));
        let c: &dyn Construct = &d;
        let built = c.build_bytes(&Value::UInt(7)).unwrap();
        assert_eq!(built, vec![7]);
    }

    #[test]
    fn lazy_sizeof_delegates() {
        let d = Lazy::new(Box::new(INT8UB));
        assert_eq!(d.sizeof(&Context::new()).unwrap(), 1);
    }

    #[test]
    fn lazy_roundtrip() {
        let d = Lazy::new(Box::new(INT16UB));
        let c: &dyn Construct = &d;
        let original = Value::UInt(0x0102);
        let built = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn lazy_propagates_parse_error() {
        let d = Lazy::new(Box::new(Bytes::new(5)));
        let c: &dyn Construct = &d;
        let err = c.parse_bytes(b"\x01").unwrap_err();
        assert!(matches!(err, ConstructError::Stream { .. }));
    }

    // ======================================================================
    // LazyStruct tests
    // ======================================================================

    #[test]
    fn lazystruct_parse_returns_container() {
        let s = LazyStruct::new()
            .field("a", Box::new(INT8UB))
            .field("b", Box::new(INT8UB));
        let c: &dyn Construct = &s;
        let parsed = c.parse_bytes(b"\x01\x02").unwrap();
        let container = parsed.as_container().unwrap();
        assert_eq!(container.get("a").unwrap(), &Value::UInt(1));
        assert_eq!(container.get("b").unwrap(), &Value::UInt(2));
    }

    #[test]
    fn lazystruct_parse_preserves_order() {
        let s = LazyStruct::new()
            .field("first", Box::new(INT8UB))
            .field("second", Box::new(INT8UB))
            .field("third", Box::new(INT8UB));
        let c: &dyn Construct = &s;
        let parsed = c.parse_bytes(b"\x01\x02\x03").unwrap();
        let container = parsed.as_container().unwrap();
        let keys: Vec<&str> = container.keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["first", "second", "third"]);
    }

    #[test]
    fn lazystruct_build() {
        let s = LazyStruct::new()
            .field("a", Box::new(INT8UB))
            .field("b", Box::new(INT8UB));
        let mut container = IndexMap::new();
        container.insert("a".to_string(), Value::UInt(1));
        container.insert("b".to_string(), Value::UInt(2));
        let c: &dyn Construct = &s;
        let built = c.build_bytes(&Value::Container(container)).unwrap();
        assert_eq!(built, vec![1, 2]);
    }

    #[test]
    fn lazystruct_build_missing_field_errors() {
        let s = LazyStruct::new().field("a", Box::new(INT8UB));
        let c: &dyn Construct = &s;
        let err = c
            .build_bytes(&Value::Container(IndexMap::new()))
            .unwrap_err();
        assert!(matches!(err, ConstructError::FieldMissing { .. }));
    }

    #[test]
    fn lazystruct_build_wrong_type_errors() {
        let s = LazyStruct::new().field("a", Box::new(INT8UB));
        let c: &dyn Construct = &s;
        let err = c.build_bytes(&Value::Int(5)).unwrap_err();
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
    }

    #[test]
    fn lazystruct_sizeof() {
        let s = LazyStruct::new()
            .field("a", Box::new(INT8UB))
            .field("b", Box::new(INT16UB));
        assert_eq!(s.sizeof(&Context::new()).unwrap(), 3);
    }

    #[test]
    fn lazystruct_flagbuildnone_all_pass() {
        let s = LazyStruct::new().anonymous(Box::new(Pass::new()));
        assert!(s.flagbuildnone());
    }

    #[test]
    fn lazystruct_flagbuildnone_false_with_value_field() {
        let s = LazyStruct::new().field("a", Box::new(INT8UB));
        assert!(!s.flagbuildnone());
    }

    #[test]
    fn lazystruct_roundtrip() {
        let s = LazyStruct::new()
            .field("a", Box::new(INT8UB))
            .field("b", Box::new(INT16UB));
        let mut container = IndexMap::new();
        container.insert("a".to_string(), Value::UInt(0x12));
        container.insert("b".to_string(), Value::UInt(0x3456));
        let c: &dyn Construct = &s;
        let built = c.build_bytes(&Value::Container(container.clone())).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, Value::Container(container));
    }

    #[test]
    fn lazystruct_default_is_empty() {
        let s = LazyStruct::default();
        let c: &dyn Construct = &s;
        let parsed = c.parse_bytes(b"").unwrap();
        assert!(parsed.as_container().unwrap().is_empty());
    }

    // ======================================================================
    // LazyArray tests
    // ======================================================================

    #[test]
    fn lazyarray_parse_returns_list() {
        let d = LazyArray::new(3, Box::new(INT8UB));
        let c: &dyn Construct = &d;
        let parsed = c.parse_bytes(b"\x01\x02\x03").unwrap();
        assert_eq!(
            parsed,
            Value::List(vec![Value::UInt(1), Value::UInt(2), Value::UInt(3)])
        );
    }

    #[test]
    fn lazyarray_count_accessor() {
        let d = LazyArray::new(5, Box::new(INT8UB));
        assert_eq!(d.count(), 5);
    }

    #[test]
    fn lazyarray_build() {
        let d = LazyArray::new(3, Box::new(INT8UB));
        let c: &dyn Construct = &d;
        let built = c
            .build_bytes(&Value::List(vec![
                Value::UInt(1),
                Value::UInt(2),
                Value::UInt(3),
            ]))
            .unwrap();
        assert_eq!(built, vec![1, 2, 3]);
    }

    #[test]
    fn lazyarray_build_wrong_count_errors() {
        let d = LazyArray::new(3, Box::new(INT8UB));
        let c: &dyn Construct = &d;
        let err = c
            .build_bytes(&Value::List(vec![Value::UInt(1), Value::UInt(2)]))
            .unwrap_err();
        assert!(matches!(err, ConstructError::Array { .. }));
    }

    #[test]
    fn lazyarray_build_wrong_type_errors() {
        let d = LazyArray::new(2, Box::new(INT8UB));
        let c: &dyn Construct = &d;
        let err = c.build_bytes(&Value::Int(5)).unwrap_err();
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
    }

    #[test]
    fn lazyarray_sizeof() {
        let d = LazyArray::new(4, Box::new(INT16UB));
        assert_eq!(d.sizeof(&Context::new()).unwrap(), 8);
    }

    #[test]
    fn lazyarray_roundtrip() {
        let d = LazyArray::new(3, Box::new(INT8UB));
        let c: &dyn Construct = &d;
        let original = Value::List(vec![Value::UInt(10), Value::UInt(20), Value::UInt(30)]);
        let built = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    // ======================================================================
    // LazyBound re-export test
    // ======================================================================

    #[test]
    fn lazybound_reexported_from_lazy_module() {
        // LazyBound is implemented in stream_ops; this test verifies it is
        // accessible via the lazy module path.
        let d = LazyBound::new(Box::new(|| Box::new(INT8UB)));
        let c: &dyn Construct = &d;
        let parsed = c.parse_bytes(b"\x2a").unwrap();
        assert_eq!(parsed, Value::UInt(42));
    }

    // ======================================================================
    // Rebuffered tests
    // ======================================================================

    #[test]
    fn rebuffered_parse_basic() {
        let d = Rebuffered::new(Box::new(INT8UB));
        let c: &dyn Construct = &d;
        let parsed = c.parse_bytes(b"\x05").unwrap();
        assert_eq!(parsed, Value::UInt(5));
    }

    #[test]
    fn rebuffered_parse_advances_stream_past_consumed() {
        // After Rebuffered parses a sub-construct that consumes N bytes,
        // the original stream cursor must be positioned exactly N bytes
        // forward — even though Rebuffered internally buffers everything.
        let d = Rebuffered::new(Box::new(Bytes::new(2)));
        let mut stream = ByteStream::new_read(b"\x01\x02\x03\x04\x05");
        let mut ctx = Context::new();

        let parsed = d.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(parsed, Value::Bytes(vec![1, 2]));

        // Only 2 bytes were consumed by the sub-construct.
        assert_eq!(stream.tell().unwrap(), 2);

        // The remaining bytes must still be readable from the original stream.
        let rest = stream.read_remaining().unwrap();
        assert_eq!(rest, vec![3, 4, 5]);
    }

    #[test]
    fn rebuffered_parse_consumes_everything_when_subcon_is_greedy() {
        use crate::constructs::bytes::GreedyBytes;
        let d = Rebuffered::new(Box::new(GreedyBytes));
        let mut stream = ByteStream::new_read(b"\x01\x02\x03");
        let mut ctx = Context::new();

        let parsed = d.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(parsed, Value::Bytes(vec![1, 2, 3]));
        // GreedyBytes consumed everything → cursor at end.
        assert_eq!(stream.tell().unwrap(), 3);
        assert!(stream.is_eof().unwrap());
    }

    #[test]
    fn rebuffered_provides_seekable_substream() {
        // A sub-construct that relies on seeking (Pointer) should work through
        // Rebuffered because the buffered stream is fully seekable.
        use crate::constructs::stream_ops::Pointer;

        // Pointer reads 1 byte at absolute offset 2 within the buffered stream.
        let d = Rebuffered::new(Box::new(Pointer::new(2, Box::new(Bytes::new(1)))));
        let c: &dyn Construct = &d;
        let parsed = c.parse_bytes(b"\xAA\xBB\xCC").unwrap();
        assert_eq!(parsed, Value::Bytes(vec![0xCC]));
    }

    #[test]
    fn rebuffered_build_delegates() {
        let d = Rebuffered::new(Box::new(INT8UB));
        let c: &dyn Construct = &d;
        let built = c.build_bytes(&Value::UInt(9)).unwrap();
        assert_eq!(built, vec![9]);
    }

    #[test]
    fn rebuffered_sizeof_delegates() {
        let d = Rebuffered::new(Box::new(INT16UB));
        assert_eq!(d.sizeof(&Context::new()).unwrap(), 2);
    }

    #[test]
    fn rebuffered_roundtrip() {
        let d = Rebuffered::new(Box::new(INT16UB));
        let c: &dyn Construct = &d;
        let original = Value::UInt(0x1234);
        let built = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn rebuffered_with_tailcutoff_still_parses() {
        // tailcutoff is accepted but does not limit buffering in this
        // simplified implementation; parsing must still succeed.
        let d = Rebuffered::new(Box::new(Bytes::new(3))).with_tailcutoff(1);
        let c: &dyn Construct = &d;
        let parsed = c.parse_bytes(b"\x01\x02\x03").unwrap();
        assert_eq!(parsed, Value::Bytes(vec![1, 2, 3]));
    }

    #[test]
    fn rebuffered_parse_error_propagates() {
        // Reading 5 bytes when only 1 is available must surface a Stream error.
        let d = Rebuffered::new(Box::new(Bytes::new(5)));
        let c: &dyn Construct = &d;
        let err = c.parse_bytes(b"\x01").unwrap_err();
        assert!(matches!(err, ConstructError::Stream { .. }));
    }
}
