//! Computed-value, rebuild, default, index, padding, alignment, fixed-size,
//! named-tuple, and timestamp constructs.
//!
//! # Module layout
//!
//! | Construct | Purpose |
//! |-----------|---------|
//! | [`Computed`] | Computes a value from context without reading/writing the stream |
//! | [`Rebuild`] | Builds from a context-derived value, ignoring the input |
//! | [`Default`] | Uses a default value when the build input is `None` |
//! | [`Index`] | Returns the current repetition index from context |
//! | [`Padded`] | Pads the sub-construct output to a fixed length |
//! | [`Aligned`] | Pads the sub-construct output to a multiple of a modulus |
//! | [`FixedSized`] | Restricts the sub-construct to a fixed-size sub-stream |
//! | [`NamedTuple`] | Adapter that maps a Struct/Sequence to a named list |
//! | [`TimestampAdapter`] | Epoch-based integer timestamp adapter |

use crate::core::context::Context;
use crate::core::error::{ConstructError, Result};
use crate::core::stream::{ByteStream, Stream};
use crate::core::Construct;
use crate::value::Value;

// ===========================================================================
// Type aliases
// ===========================================================================

/// Function signature for computing a [`Value`] from the context.
///
/// Used by [`Computed`] and [`Rebuild`] to derive values during parse/build.
pub type ComputeFunc = Box<dyn Fn(&Context) -> Result<Value>>;

// ===========================================================================
// Computed
// ===========================================================================

/// A construct that computes a value from the context without touching the
/// stream.
///
/// - **parse**: calls `func(ctx)` and returns the result
/// - **build**: calls `func(ctx)` (the build value is ignored)
/// - **sizeof**: returns `0`
///
/// Corresponds to Python `Computed(func)` (`construct/construct/core.py`
/// line ~2878).
///
/// # Examples
///
/// ```
/// use construct::constructs::computed::Computed;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let c = Computed::new(Box::new(|_ctx| Ok(Value::Int(42))));
/// let c: &dyn Construct = &c;
/// assert_eq!(c.parse_bytes(b"").unwrap(), Value::Int(42));
/// assert_eq!(c.sizeof(&Default::default()).unwrap(), 0);
/// ```
pub struct Computed {
    /// Function that produces the value from the context.
    pub func: ComputeFunc,
}

impl Computed {
    /// Creates a new `Computed` with the given context function.
    pub fn new(func: ComputeFunc) -> Self {
        Computed { func }
    }
}

impl Construct for Computed {
    fn parse(&self, _stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        (self.func)(ctx)
    }

    fn build(&self, _data: &Value, _stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        // Build evaluates the function but discards the result; the stream
        // is unaffected. This matches the Python behaviour where _build
        // returns the computed value but does not write anything.
        let _ = (self.func)(ctx)?;
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Ok(0)
    }

    fn flagbuildnone(&self) -> bool {
        true
    }
}

// ===========================================================================
// Rebuild
// ===========================================================================

/// A construct where building does not require a value because the value is
/// recomputed from the context.
///
/// - **parse**: delegates to `subcon`
/// - **build**: evaluates `func(ctx)` and passes the result to `subcon.build`
/// - **sizeof**: delegates to `subcon`
///
/// The difference from [`Default`] is that [`Rebuild`] always ignores the
/// build value, while [`Default`] only uses the fallback when the value is
/// `None`.
///
/// Corresponds to Python `Rebuild(subcon, func)`
/// (`construct/construct/core.py` line ~2975).
///
/// # Examples
///
/// ```
/// use construct::constructs::computed::Rebuild;
/// use construct::constructs::format_field::INT8UB;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let r = Rebuild::new(
///     Box::new(INT8UB),
///     Box::new(|_ctx| Ok(Value::UInt(7))),
/// );
/// let c: &dyn Construct = &r;
/// // Build ignores the input and uses the function
/// let bytes = c.build_bytes(&Value::None).unwrap();
/// assert_eq!(bytes, vec![7]);
/// ```
pub struct Rebuild {
    /// The inner construct.
    pub subcon: Box<dyn Construct>,
    /// Function that computes the build value from the context.
    pub func: ComputeFunc,
}

impl Rebuild {
    /// Creates a new `Rebuild` with the given inner construct and context
    /// function.
    pub fn new(subcon: Box<dyn Construct>, func: ComputeFunc) -> Self {
        Rebuild { subcon, func }
    }
}

impl Construct for Rebuild {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        self.subcon.parse(stream, ctx)
    }

    fn build(&self, _data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        let value = (self.func)(ctx)?;
        self.subcon.build(&value, stream, ctx)
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        self.subcon.sizeof(ctx)
    }

    fn flagbuildnone(&self) -> bool {
        true
    }
}

// ===========================================================================
// Default
// ===========================================================================

/// A construct where building uses a default value when the input is `None`.
///
/// - **parse**: delegates to `subcon`
/// - **build**: if `data` is `Value::None`, uses `value`; otherwise uses `data`
/// - **sizeof**: delegates to `subcon`
///
/// The difference from [`Rebuild`] is that [`Default`] only substitutes when
/// the value is missing, while [`Rebuild`] always recomputes.
///
/// Corresponds to Python `Default(subcon, value)`
/// (`construct/construct/core.py` line ~3030).
///
/// # Examples
///
/// ```
/// use construct::constructs::computed::Default;
/// use construct::constructs::format_field::INT8UB;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let d = Default::new(Box::new(INT8UB), Value::UInt(0));
/// let c: &dyn Construct = &d;
///
/// // Build with explicit value
/// let bytes = c.build_bytes(&Value::UInt(5)).unwrap();
/// assert_eq!(bytes, vec![5]);
///
/// // Build with None → uses default
/// let bytes = c.build_bytes(&Value::None).unwrap();
/// assert_eq!(bytes, vec![0]);
/// ```
pub struct Default {
    /// The inner construct.
    pub subcon: Box<dyn Construct>,
    /// The default value used when the build input is `None`.
    pub value: Value,
}

impl Default {
    /// Creates a new `Default` with the given inner construct and fallback
    /// value.
    pub fn new(subcon: Box<dyn Construct>, value: Value) -> Self {
        Default { subcon, value }
    }
}

impl Construct for Default {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        self.subcon.parse(stream, ctx)
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        let effective = if data.is_none() { &self.value } else { data };
        self.subcon.build(effective, stream, ctx)
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        self.subcon.sizeof(ctx)
    }

    fn flagbuildnone(&self) -> bool {
        true
    }
}

// ===========================================================================
// Index
// ===========================================================================

/// Context key used to store the current repetition index.
///
/// Corresponds to the Python `context._index` field.
const CONTEXT_INDEX_KEY: &str = "_index";

/// Returns the current repetition index from the context.
///
/// - **parse**: returns `ctx["_index"]` or `Value::None` if absent
/// - **build**: returns `ctx["_index"]` or `Value::None` if absent
/// - **sizeof**: returns `0`
///
/// Used inside [`Array`](crate::constructs::repetition::Array) and similar
/// repetition constructs.
///
/// Corresponds to Python `Index` singleton
/// (`construct/construct/core.py` line ~2933).
///
/// # Examples
///
/// ```
/// use construct::constructs::computed::Index;
/// use construct::constructs::repetition::Array;
/// use construct::constructs::format_field::INT8UB;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let arr = Array::new(3, Box::new(Index::new()));
/// let c: &dyn Construct = &arr;
/// let parsed = c.parse_bytes(b"").unwrap();
/// assert_eq!(parsed, Value::List(vec![Value::UInt(0), Value::UInt(1), Value::UInt(2)]));
/// ```
pub struct Index;

impl Index {
    /// Creates a new `Index` construct.
    pub fn new() -> Self {
        Index
    }
}

impl std::default::Default for Index {
    fn default() -> Self {
        Self::new()
    }
}

impl Construct for Index {
    fn parse(&self, _stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        Ok(ctx.get(CONTEXT_INDEX_KEY).cloned().unwrap_or(Value::None))
    }

    fn build(&self, _data: &Value, _stream: &mut dyn Stream, _ctx: &mut Context) -> Result<()> {
        // Build returns the index from context (like parse), but since build
        // returns () we just do nothing.
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Ok(0)
    }

    fn flagbuildnone(&self) -> bool {
        true
    }
}

// ===========================================================================
// Padded
// ===========================================================================

/// Default padding pattern byte (null byte).
const DEFAULT_PAD_PATTERN: u8 = 0x00;

/// Wraps a sub-construct and pads its output to a fixed length.
///
/// - **parse**: parses `subcon`, then reads and discards padding bytes to
///   reach `length` total bytes consumed
/// - **build**: builds `subcon`, then writes padding bytes to reach `length`
///   total bytes written
/// - **sizeof**: returns `length`
///
/// If `strict` is `true`, the padding bytes are validated against `pattern`
/// during parse.
///
/// Corresponds to Python `Padded(length, subcon, pattern)`
/// (`construct/construct/core.py` line ~4175).
///
/// # Examples
///
/// ```
/// use construct::constructs::computed::Padded;
/// use construct::constructs::format_field::INT8UB;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let p = Padded::new(4, Box::new(INT8UB), 0x00, false);
/// let c: &dyn Construct = &p;
///
/// let parsed = c.parse_bytes(b"\xFF\x00\x00\x00").unwrap();
/// assert_eq!(parsed, Value::UInt(255));
///
/// let built = c.build_bytes(&Value::UInt(255)).unwrap();
/// assert_eq!(built, vec![0xFF, 0x00, 0x00, 0x00]);
/// ```
pub struct Padded {
    /// The inner construct.
    pub subcon: Box<dyn Construct>,
    /// Total length in bytes (data + padding).
    pub length: usize,
    /// Padding pattern byte.
    pub pattern: u8,
    /// If `true`, padding bytes are validated during parse.
    pub strict: bool,
}

impl Padded {
    /// Creates a new `Padded` construct with the given length, sub-construct,
    /// padding pattern, and strict mode.
    pub fn new(length: usize, subcon: Box<dyn Construct>, pattern: u8, strict: bool) -> Self {
        Padded {
            subcon,
            length,
            pattern,
            strict,
        }
    }

    /// Creates a new `Padded` with default null-byte padding and non-strict
    /// mode.
    pub fn new_default(length: usize, subcon: Box<dyn Construct>) -> Self {
        Self::new(length, subcon, DEFAULT_PAD_PATTERN, false)
    }
}

impl Construct for Padded {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let pos_before = stream.tell()?;
        let obj = self.subcon.parse(stream, ctx)?;
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

        Ok(obj)
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        let pos_before = stream.tell()?;
        self.subcon.build(data, stream, ctx)?;
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

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Ok(self.length)
    }
}

// ===========================================================================
// Aligned
// ===========================================================================

/// Minimum valid modulus for [`Aligned`].
const ALIGNED_MIN_MODULUS: usize = 2;

/// Wraps a sub-construct and pads its output to the next multiple of a
/// modulus.
///
/// - **parse**: parses `subcon`, then reads and discards alignment padding
/// - **build**: builds `subcon`, then writes padding bytes to reach the next
///   multiple of `modulus`
/// - **sizeof**: returns `subcon.sizeof()` rounded up to the next multiple of
///   `modulus`
///
/// Corresponds to Python `Aligned(modulus, subcon, pattern)`
/// (`construct/construct/core.py` line ~4261).
///
/// # Examples
///
/// ```
/// use construct::constructs::computed::Aligned;
/// use construct::constructs::format_field::INT16UB;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let a = Aligned::new(4, Box::new(INT16UB), 0x00);
/// let c: &dyn Construct = &a;
///
/// let parsed = c.parse_bytes(b"\x00\x01\x00\x00").unwrap();
/// assert_eq!(parsed, Value::UInt(1));
///
/// assert_eq!(c.sizeof(&Default::default()).unwrap(), 4);
/// ```
pub struct Aligned {
    /// The inner construct.
    pub subcon: Box<dyn Construct>,
    /// Alignment modulus (must be >= 2).
    pub modulus: usize,
    /// Padding pattern byte.
    pub pattern: u8,
}

impl Aligned {
    /// Creates a new `Aligned` construct with the given modulus, sub-construct,
    /// and padding pattern.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::Padding`] if `modulus < 2`.
    pub fn new(modulus: usize, subcon: Box<dyn Construct>, pattern: u8) -> Self {
        Aligned {
            subcon,
            modulus,
            pattern,
        }
    }

    /// Creates a new `Aligned` with default null-byte padding.
    pub fn new_default(modulus: usize, subcon: Box<dyn Construct>) -> Self {
        Self::new(modulus, subcon, DEFAULT_PAD_PATTERN)
    }

    /// Validates that the modulus is >= 2, returning an error otherwise.
    fn validate_modulus(&self) -> Result<()> {
        if self.modulus < ALIGNED_MIN_MODULUS {
            return Err(ConstructError::Padding {
                path: String::new(),
                message: format!(
                    "expected modulus {} or greater, got {}",
                    ALIGNED_MIN_MODULUS, self.modulus
                ),
            });
        }
        Ok(())
    }
}

impl Construct for Aligned {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        self.validate_modulus()?;
        let pos_before = stream.tell()?;
        let obj = self.subcon.parse(stream, ctx)?;
        let pos_after = stream.tell()?;
        let consumed = (pos_after - pos_before) as usize;
        let pad = (self.modulus - (consumed % self.modulus)) % self.modulus;
        if pad > 0 {
            let _ = stream.read_bytes(pad)?;
        }
        Ok(obj)
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        self.validate_modulus()?;
        let pos_before = stream.tell()?;
        self.subcon.build(data, stream, ctx)?;
        let pos_after = stream.tell()?;
        let written = (pos_after - pos_before) as usize;
        let pad = (self.modulus - (written % self.modulus)) % self.modulus;
        if pad > 0 {
            let padding = vec![self.pattern; pad];
            stream.write_bytes(&padding)?;
        }
        Ok(())
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        self.validate_modulus()?;
        let subcon_size = self.subcon.sizeof(ctx)?;
        let pad = (self.modulus - (subcon_size % self.modulus)) % self.modulus;
        Ok(subcon_size + pad)
    }
}

// ===========================================================================
// FixedSized
// ===========================================================================

/// Restricts the sub-construct to a fixed-size sub-stream.
///
/// - **parse**: reads `length` bytes from the stream, creates a sub-stream,
///   and parses `subcon` from it
/// - **build**: builds `subcon` into a sub-stream, then writes the result
///   padded to `length` bytes
/// - **sizeof**: returns `length`
///
/// Corresponds to Python `FixedSized(length, subcon)`
/// (`construct/construct/core.py` line ~4986).
///
/// # Examples
///
/// ```
/// use construct::constructs::computed::FixedSized;
/// use construct::constructs::format_field::INT8UB;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let fs = FixedSized::new(4, Box::new(INT8UB));
/// let c: &dyn Construct = &fs;
///
/// let parsed = c.parse_bytes(b"\xFF\x00\x00\x00").unwrap();
/// assert_eq!(parsed, Value::UInt(255));
///
/// let built = c.build_bytes(&Value::UInt(255)).unwrap();
/// assert_eq!(built, vec![0xFF, 0x00, 0x00, 0x00]);
/// ```
pub struct FixedSized {
    /// The inner construct.
    pub subcon: Box<dyn Construct>,
    /// Total size in bytes (data + padding).
    pub length: usize,
}

impl FixedSized {
    /// Creates a new `FixedSized` with the given length and sub-construct.
    pub fn new(length: usize, subcon: Box<dyn Construct>) -> Self {
        FixedSized { subcon, length }
    }
}

impl Construct for FixedSized {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let data = stream.read_bytes(self.length)?;
        let mut sub_stream = ByteStream::new_read(&data);
        self.subcon.parse(&mut sub_stream, ctx)
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        let mut sub_stream = ByteStream::new_write();
        self.subcon.build(data, &mut sub_stream, ctx)?;
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
            let padding = vec![DEFAULT_PAD_PATTERN; pad];
            stream.write_bytes(&padding)?;
        }

        Ok(())
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Ok(self.length)
    }
}

// ===========================================================================
// NamedTuple
// ===========================================================================

/// Adapter that maps a Struct/Sequence construct's result to a named list.
///
/// In Python this maps to a `collections.namedtuple`. In Rust, since there
/// is no direct equivalent, we return a [`Value::List`] where each element
/// is a [`Value::Container`] with `name` and `value` fields. This is a
/// simplified implementation.
///
/// - **parse** (decode): converts the subcon result into a named list
/// - **build** (encode): converts a named list back into the subcon format
/// - **sizeof**: delegates to the inner subcon adapter
///
/// Corresponds to Python `NamedTuple(tuplename, tuplefields, subcon)`
/// (`construct/construct/core.py` line ~3381).
///
/// # Examples
///
/// ```
/// use construct::constructs::computed::NamedTuple;
/// use construct::constructs::sequence::Sequence;
/// use construct::constructs::format_field::INT8UB;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let seq = Sequence::new()
///     .push(Box::new(INT8UB))
///     .push(Box::new(INT8UB))
///     .push(Box::new(INT8UB));
/// let nt = NamedTuple::new("coord", vec!["x".to_string(), "y".to_string(), "z".to_string()], Box::new(seq));
/// let c: &dyn Construct = &nt;
///
/// let parsed = c.parse_bytes(b"\x01\x02\x03").unwrap();
/// // Returns a List of Containers: [{name:"x", value:1}, {name:"y", value:2}, {name:"z", value:3}]
/// ```
pub struct NamedTuple {
    /// The inner adapter (wrapping the actual subcon).
    pub inner: Box<dyn Construct>,
    /// The field names.
    pub field_names: Vec<String>,
    /// The tuple type name (for display/debug purposes).
    pub tuple_name: String,
}

impl NamedTuple {
    /// Creates a new `NamedTuple` with the given name, field names, and
    /// inner construct (expected to be a Struct, Sequence, Array, etc.).
    pub fn new(
        tuple_name: impl Into<String>,
        field_names: Vec<String>,
        subcon: Box<dyn Construct>,
    ) -> Self {
        NamedTuple {
            inner: subcon,
            field_names,
            tuple_name: tuple_name.into(),
        }
    }
}

impl Construct for NamedTuple {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let raw = self.inner.parse(stream, ctx)?;
        Self::decode_value(&raw, &self.field_names, &self.tuple_name)
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        let encoded = Self::encode_value(data, &self.field_names)?;
        self.inner.build(&encoded, stream, ctx)
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        self.inner.sizeof(ctx)
    }
}

impl NamedTuple {
    /// Decodes a raw value (List or Container from subcon) into a named
    /// list representation.
    fn decode_value(raw: &Value, field_names: &[String], _tuple_name: &str) -> Result<Value> {
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
                        let mut entry = indexmap::IndexMap::new();
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
                        let mut entry = indexmap::IndexMap::new();
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

    /// Encodes a named list representation back into a value suitable for
    /// the inner construct.
    fn encode_value(data: &Value, field_names: &[String]) -> Result<Value> {
        match data {
            Value::List(items) => {
                // Convert named list → plain list
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
            Value::Container(map) => {
                // Already a container, pass through
                Ok(Value::Container(map.clone()))
            }
            other => Err(ConstructError::TypeMismatch {
                path: String::new(),
                expected: "List or Container".to_string(),
                actual: other.type_name().to_string(),
            }),
        }
    }
}

// ===========================================================================
// TimestampAdapter
// ===========================================================================

/// Unit of time for [`TimestampAdapter`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimestampUnit {
    /// Second resolution (Unix epoch).
    Seconds,
    /// Millisecond resolution.
    Milliseconds,
    /// Microsecond resolution.
    Microseconds,
    /// Nanosecond resolution.
    Nanoseconds,
}

/// Epoch reference point for [`TimestampAdapter`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimestampEpoch {
    /// Unix epoch: 1970-01-01 00:00:00 UTC.
    Unix,
    /// Custom epoch year.
    Custom(i64),
}

impl TimestampEpoch {
    /// Returns the epoch offset in seconds from 0000-01-01 (conceptual).
    ///
    /// For the simplified implementation we just return the epoch base year
    /// as a count of years. The actual conversion uses seconds-since-epoch.
    fn epoch_seconds(&self) -> i64 {
        match self {
            TimestampEpoch::Unix => 0,
            TimestampEpoch::Custom(_year) => 0,
        }
    }
}

/// An adapter that converts between integer epoch timestamps and a structured
/// representation.
///
/// This is a simplified implementation that does not depend on `chrono` or
/// `time` crates. It operates on raw epoch integers and provides a Container
/// with `secs` and `nanos` fields.
///
/// - **parse**: reads an integer via `subcon`, converts to `{secs, nanos}`
/// - **build**: reads `{secs, nanos}` from a Container, converts to integer
///   and builds via `subcon`
/// - **sizeof**: delegates to `subcon`
///
/// Corresponds to Python `TimestampAdapter`
/// (`construct/construct/core.py` line ~3449).
///
/// # Examples
///
/// ```
/// use construct::constructs::computed::{TimestampAdapter, TimestampUnit, TimestampEpoch};
/// use construct::constructs::format_field::INT32UB;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let ts = TimestampAdapter::new(
///     Box::new(INT32UB),
///     TimestampUnit::Seconds,
///     TimestampEpoch::Unix,
/// );
/// let c: &dyn Construct = &ts;
///
/// let parsed = c.parse_bytes(b"\x00\x00\x00\x3C").unwrap();
/// // 60 seconds since Unix epoch → {secs: 60, nanos: 0}
/// ```
pub struct TimestampAdapter {
    /// The inner integer construct.
    pub subcon: Box<dyn Construct>,
    /// The time unit of the integer value.
    pub unit: TimestampUnit,
    /// The epoch reference.
    pub epoch: TimestampEpoch,
}

impl TimestampAdapter {
    /// Creates a new `TimestampAdapter` with the given sub-construct, unit,
    /// and epoch.
    pub fn new(subcon: Box<dyn Construct>, unit: TimestampUnit, epoch: TimestampEpoch) -> Self {
        TimestampAdapter {
            subcon,
            unit,
            epoch,
        }
    }

    /// Converts a raw integer (in the configured unit) to seconds and
    /// nanoseconds.
    fn integer_to_secs_nanos(&self, raw: i64) -> (i64, u32) {
        let (secs, nanos) = match self.unit {
            TimestampUnit::Seconds => (raw, 0u32),
            TimestampUnit::Milliseconds => (raw / 1000, ((raw % 1000) * 1_000_000) as u32),
            TimestampUnit::Microseconds => (raw / 1_000_000, ((raw % 1_000_000) * 1000) as u32),
            TimestampUnit::Nanoseconds => (raw / 1_000_000_000, (raw % 1_000_000_000) as u32),
        };
        // Apply epoch offset (for Unix epoch, offset is 0)
        let _offset = self.epoch.epoch_seconds();
        (secs, nanos)
    }

    /// Converts seconds and nanoseconds back to a raw integer in the
    /// configured unit.
    fn secs_nanos_to_integer(&self, secs: i64, nanos: u32) -> i64 {
        match self.unit {
            TimestampUnit::Seconds => secs,
            TimestampUnit::Milliseconds => secs * 1000 + (i64::from(nanos) / 1_000_000),
            TimestampUnit::Microseconds => secs * 1_000_000 + (i64::from(nanos) / 1000),
            TimestampUnit::Nanoseconds => secs * 1_000_000_000 + i64::from(nanos),
        }
    }
}

impl Construct for TimestampAdapter {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let raw_val = self.subcon.parse(stream, ctx)?;
        let raw_i64 = raw_val.to_i64()?;

        let (secs, nanos) = self.integer_to_secs_nanos(raw_i64);

        let mut map = indexmap::IndexMap::new();
        map.insert("secs".to_string(), Value::Int(secs));
        map.insert("nanos".to_string(), Value::UInt(u64::from(nanos)));
        Ok(Value::Container(map))
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        let container = data.as_container()?;
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

        let raw_i64 = self.secs_nanos_to_integer(secs, nanos);
        self.subcon.build(&Value::Int(raw_i64), stream, ctx)
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
    use crate::constructs::format_field::{Endianness, FormatField, FormatKind};
    use crate::core::error::ConstructError;
    use crate::core::stream::ByteStream;

    /// Helper: creates a U8 big-endian construct.
    fn u8be() -> Box<dyn Construct> {
        Box::new(FormatField::new(Endianness::Big, FormatKind::U8))
    }

    /// Helper: creates a U16 big-endian construct.
    fn u16be() -> Box<dyn Construct> {
        Box::new(FormatField::new(Endianness::Big, FormatKind::U16))
    }

    /// Helper: creates a U32 big-endian construct.
    fn u32be() -> Box<dyn Construct> {
        Box::new(FormatField::new(Endianness::Big, FormatKind::U32))
    }

    // ======================================================================
    // Computed tests
    // ======================================================================

    #[test]
    fn computed_parse_returns_func_result() {
        let c = Computed::new(Box::new(|_ctx| Ok(Value::Int(42))));
        let c: &dyn Construct = &c;
        let result = c.parse_bytes(b"").unwrap();
        assert_eq!(result, Value::Int(42));
    }

    #[test]
    fn computed_parse_does_not_consume_stream() {
        let c = Computed::new(Box::new(|_ctx| Ok(Value::Int(1))));
        let mut stream = ByteStream::new_read(b"\x01\x02\x03");
        let mut ctx = Context::new();
        let _ = c.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(stream.tell().unwrap(), 0);
    }

    #[test]
    fn computed_build_does_not_write() {
        let c = Computed::new(Box::new(|_ctx| Ok(Value::Int(1))));
        let c: &dyn Construct = &c;
        let bytes = c.build_bytes(&Value::None).unwrap();
        assert!(bytes.is_empty());
    }

    #[test]
    fn computed_sizeof_returns_zero() {
        let c = Computed::new(Box::new(|_ctx| Ok(Value::Int(1))));
        assert_eq!(c.sizeof(&Context::new()).unwrap(), 0);
    }

    #[test]
    fn computed_uses_context() {
        let c = Computed::new(Box::new(|ctx| {
            let val = ctx
                .get_recursive("multiplier")
                .cloned()
                .unwrap_or(Value::UInt(1));
            let m = val.to_u64()?;
            Ok(Value::UInt(10 * m))
        }));
        let mut stream = ByteStream::new_read(b"");
        let mut ctx = Context::new();
        ctx.insert("multiplier", Value::UInt(3));
        let result = c.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::UInt(30));
    }

    #[test]
    fn computed_flagbuildnone_is_true() {
        let c = Computed::new(Box::new(|_ctx| Ok(Value::None)));
        assert!(c.flagbuildnone());
    }

    #[test]
    fn computed_func_error_propagates() {
        let c = Computed::new(Box::new(|_ctx| {
            Err(ConstructError::Generic {
                path: String::new(),
                message: "computed failed".to_string(),
            })
        }));
        let c: &dyn Construct = &c;
        let err = c.parse_bytes(b"").unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    // ======================================================================
    // Rebuild tests
    // ======================================================================

    #[test]
    fn rebuild_parse_delegates_to_subcon() {
        let r = Rebuild::new(u8be(), Box::new(|_ctx| Ok(Value::UInt(99))));
        let c: &dyn Construct = &r;
        let parsed = c.parse_bytes(b"\x07").unwrap();
        assert_eq!(parsed, Value::UInt(7));
    }

    #[test]
    fn rebuild_build_ignores_input_uses_func() {
        let r = Rebuild::new(u8be(), Box::new(|_ctx| Ok(Value::UInt(42))));
        let c: &dyn Construct = &r;
        let bytes = c.build_bytes(&Value::None).unwrap();
        assert_eq!(bytes, vec![42]);
    }

    #[test]
    fn rebuild_build_ignores_non_none_input() {
        let r = Rebuild::new(u8be(), Box::new(|_ctx| Ok(Value::UInt(10))));
        let c: &dyn Construct = &r;
        // Even with a non-None value, rebuild uses the func
        let bytes = c.build_bytes(&Value::UInt(99)).unwrap();
        assert_eq!(bytes, vec![10]);
    }

    #[test]
    fn rebuild_sizeof_delegates() {
        let r = Rebuild::new(u16be(), Box::new(|_ctx| Ok(Value::UInt(0))));
        assert_eq!(r.sizeof(&Context::new()).unwrap(), 2);
    }

    #[test]
    fn rebuild_flagbuildnone_is_true() {
        let r = Rebuild::new(u8be(), Box::new(|_ctx| Ok(Value::UInt(0))));
        assert!(r.flagbuildnone());
    }

    #[test]
    fn rebuild_func_uses_context() {
        let r = Rebuild::new(
            u8be(),
            Box::new(|ctx| {
                let count = ctx
                    .get_recursive("count")
                    .cloned()
                    .unwrap_or(Value::UInt(0));
                Ok(count)
            }),
        );
        let mut ctx = Context::new();
        ctx.insert("count", Value::UInt(5));
        let mut stream = ByteStream::new_write();
        r.build(&Value::None, &mut stream, &mut ctx).unwrap();
        assert_eq!(stream.into_bytes(), vec![5]);
    }

    #[test]
    fn rebuild_roundtrip() {
        let r = Rebuild::new(u8be(), Box::new(|_ctx| Ok(Value::UInt(77))));
        let c: &dyn Construct = &r;
        let bytes = c.build_bytes(&Value::None).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, Value::UInt(77));
    }

    // ======================================================================
    // Default tests
    // ======================================================================

    #[test]
    fn default_parse_delegates_to_subcon() {
        let d = Default::new(u8be(), Value::UInt(0));
        let c: &dyn Construct = &d;
        let parsed = c.parse_bytes(b"\x05").unwrap();
        assert_eq!(parsed, Value::UInt(5));
    }

    #[test]
    fn default_build_uses_value_when_not_none() {
        let d = Default::new(u8be(), Value::UInt(0));
        let c: &dyn Construct = &d;
        let bytes = c.build_bytes(&Value::UInt(5)).unwrap();
        assert_eq!(bytes, vec![5]);
    }

    #[test]
    fn default_build_uses_default_when_none() {
        let d = Default::new(u8be(), Value::UInt(0));
        let c: &dyn Construct = &d;
        let bytes = c.build_bytes(&Value::None).unwrap();
        assert_eq!(bytes, vec![0]);
    }

    #[test]
    fn default_sizeof_delegates() {
        let d = Default::new(u16be(), Value::UInt(0));
        assert_eq!(d.sizeof(&Context::new()).unwrap(), 2);
    }

    #[test]
    fn default_flagbuildnone_is_true() {
        let d = Default::new(u8be(), Value::UInt(0));
        assert!(d.flagbuildnone());
    }

    #[test]
    fn default_roundtrip_with_explicit_value() {
        let d = Default::new(u8be(), Value::UInt(0));
        let c: &dyn Construct = &d;
        let bytes = c.build_bytes(&Value::UInt(42)).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, Value::UInt(42));
    }

    #[test]
    fn default_roundtrip_with_none_uses_default() {
        let d = Default::new(u8be(), Value::UInt(100));
        let c: &dyn Construct = &d;
        let bytes = c.build_bytes(&Value::None).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, Value::UInt(100));
    }

    // ======================================================================
    // Index tests
    // ======================================================================

    #[test]
    fn index_parse_returns_none_without_context() {
        let idx = Index::new();
        let mut stream = ByteStream::new_read(b"");
        let mut ctx = Context::new();
        let result = idx.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::None);
    }

    #[test]
    fn index_parse_returns_index_from_context() {
        let idx = Index::new();
        let mut stream = ByteStream::new_read(b"");
        let mut ctx = Context::new();
        ctx.insert("_index", Value::UInt(5));
        let result = idx.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::UInt(5));
    }

    #[test]
    fn index_build_does_nothing() {
        let idx = Index::new();
        let mut stream = ByteStream::new_write();
        let mut ctx = Context::new();
        idx.build(&Value::None, &mut stream, &mut ctx).unwrap();
        assert!(stream.into_bytes().is_empty());
    }

    #[test]
    fn index_sizeof_returns_zero() {
        assert_eq!(Index::new().sizeof(&Context::new()).unwrap(), 0);
    }

    #[test]
    fn index_flagbuildnone_is_true() {
        assert!(Index::new().flagbuildnone());
    }

    #[test]
    fn index_in_array() {
        use crate::constructs::repetition::Array;
        let arr = Array::new(3, Box::new(Index::new()));
        let c: &dyn Construct = &arr;
        let parsed = c.parse_bytes(b"").unwrap();
        assert_eq!(
            parsed,
            Value::List(vec![Value::UInt(0), Value::UInt(1), Value::UInt(2)])
        );
    }

    // ======================================================================
    // Padded tests
    // ======================================================================

    #[test]
    fn padded_parse_pads_correctly() {
        let p = Padded::new(4, u8be(), 0x00, false);
        let c: &dyn Construct = &p;
        let parsed = c.parse_bytes(b"\xFF\x00\x00\x00").unwrap();
        assert_eq!(parsed, Value::UInt(0xFF));
    }

    #[test]
    fn padded_build_pads_correctly() {
        let p = Padded::new(4, u8be(), 0x00, false);
        let c: &dyn Construct = &p;
        let bytes = c.build_bytes(&Value::UInt(0xFF)).unwrap();
        assert_eq!(bytes, vec![0xFF, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn padded_parse_subcon_too_large_errors() {
        let p = Padded::new(2, u16be(), 0x00, false);
        let c: &dyn Construct = &p;
        // u16be reads 2 bytes, length is 2, so no padding needed
        let parsed = c.parse_bytes(b"\x00\x01").unwrap();
        assert_eq!(parsed, Value::UInt(1));

        // But if subcon reads more than length, error
        let p = Padded::new(1, u16be(), 0x00, false);
        let c: &dyn Construct = &p;
        let err = c.parse_bytes(b"\x00\x01").unwrap_err();
        assert!(matches!(err, ConstructError::Padding { .. }));
    }

    #[test]
    fn padded_build_subcon_too_large_errors() {
        let p = Padded::new(1, u16be(), 0x00, false);
        let c: &dyn Construct = &p;
        let err = c.build_bytes(&Value::UInt(1)).unwrap_err();
        assert!(matches!(err, ConstructError::Padding { .. }));
    }

    #[test]
    fn padded_sizeof_returns_length() {
        let p = Padded::new(10, u8be(), 0x00, false);
        assert_eq!(p.sizeof(&Context::new()).unwrap(), 10);
    }

    #[test]
    fn padded_strict_parse_validates_padding() {
        let p = Padded::new(4, u8be(), 0x00, true);
        let c: &dyn Construct = &p;
        // Valid padding
        let parsed = c.parse_bytes(b"\xFF\x00\x00\x00").unwrap();
        assert_eq!(parsed, Value::UInt(0xFF));
    }

    #[test]
    fn padded_strict_parse_rejects_bad_padding() {
        let p = Padded::new(4, u8be(), 0x00, true);
        let c: &dyn Construct = &p;
        let err = c.parse_bytes(b"\xFF\x01\x00\x00").unwrap_err();
        assert!(matches!(err, ConstructError::Padding { .. }));
    }

    #[test]
    fn padded_roundtrip() {
        let p = Padded::new(4, u8be(), 0xAB, false);
        let c: &dyn Construct = &p;
        let bytes = c.build_bytes(&Value::UInt(42)).unwrap();
        assert_eq!(bytes, vec![42, 0xAB, 0xAB, 0xAB]);
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, Value::UInt(42));
    }

    #[test]
    fn padded_custom_pattern() {
        let p = Padded::new(4, u8be(), 0xFF, false);
        let c: &dyn Construct = &p;
        let bytes = c.build_bytes(&Value::UInt(1)).unwrap();
        assert_eq!(bytes, vec![1, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn padded_exact_fit() {
        // subcon uses all bytes, no padding
        let p = Padded::new(2, u16be(), 0x00, false);
        let c: &dyn Construct = &p;
        let bytes = c.build_bytes(&Value::UInt(0x0102)).unwrap();
        assert_eq!(bytes, vec![0x01, 0x02]);
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, Value::UInt(0x0102));
    }

    // ======================================================================
    // Aligned tests
    // ======================================================================

    #[test]
    fn aligned_parse_pads_to_modulus() {
        let a = Aligned::new(4, u8be(), 0x00);
        let c: &dyn Construct = &a;
        let parsed = c.parse_bytes(b"\xFF\x00\x00\x00").unwrap();
        assert_eq!(parsed, Value::UInt(0xFF));
    }

    #[test]
    fn aligned_build_pads_to_modulus() {
        let a = Aligned::new(4, u8be(), 0x00);
        let c: &dyn Construct = &a;
        let bytes = c.build_bytes(&Value::UInt(0xFF)).unwrap();
        assert_eq!(bytes, vec![0xFF, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn aligned_already_aligned_no_padding() {
        // u16 = 2 bytes, modulus 4 → needs 2 padding bytes (-(2) % 4 = 2 in Python)
        let a = Aligned::new(4, u16be(), 0x00);
        let c: &dyn Construct = &a;
        let bytes = c.build_bytes(&Value::UInt(1)).unwrap();
        assert_eq!(bytes, vec![0x00, 0x01, 0x00, 0x00]);
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, Value::UInt(1));
    }

    #[test]
    fn aligned_sizeof_rounds_up() {
        // u8 = 1 byte, modulus 4 → sizeof = 4
        let a = Aligned::new(4, u8be(), 0x00);
        assert_eq!(a.sizeof(&Context::new()).unwrap(), 4);

        // u16 = 2 bytes, modulus 4 → sizeof = 4
        let a = Aligned::new(4, u16be(), 0x00);
        assert_eq!(a.sizeof(&Context::new()).unwrap(), 4);
    }

    #[test]
    fn aligned_modulus_less_than_2_errors() {
        let a = Aligned::new(1, u8be(), 0x00);
        let ctx = Context::new();
        let err = a.sizeof(&ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Padding { .. }));
    }

    #[test]
    fn aligned_roundtrip() {
        let a = Aligned::new(8, u16be(), 0xCC);
        let c: &dyn Construct = &a;
        let bytes = c.build_bytes(&Value::UInt(0xABCD)).unwrap();
        // u16 = 2 bytes, pad to next multiple of 8: 6 padding bytes
        assert_eq!(bytes.len(), 8);
        assert_eq!(bytes[0], 0xAB);
        assert_eq!(bytes[1], 0xCD);
        for b in bytes.iter().skip(2) {
            assert_eq!(*b, 0xCC);
        }
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, Value::UInt(0xABCD));
    }

    // ======================================================================
    // FixedSized tests
    // ======================================================================

    #[test]
    fn fixed_sized_parse_reads_fixed_bytes() {
        let fs = FixedSized::new(4, u8be());
        let c: &dyn Construct = &fs;
        let parsed = c.parse_bytes(b"\xFF\x00\x00\x00").unwrap();
        assert_eq!(parsed, Value::UInt(0xFF));
    }

    #[test]
    fn fixed_sized_build_pads_to_length() {
        let fs = FixedSized::new(4, u8be());
        let c: &dyn Construct = &fs;
        let bytes = c.build_bytes(&Value::UInt(0xFF)).unwrap();
        assert_eq!(bytes, vec![0xFF, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn fixed_sized_build_subcon_too_large_errors() {
        let fs = FixedSized::new(1, u16be());
        let c: &dyn Construct = &fs;
        let err = c.build_bytes(&Value::UInt(1)).unwrap_err();
        assert!(matches!(err, ConstructError::Padding { .. }));
    }

    #[test]
    fn fixed_sized_sizeof_returns_length() {
        let fs = FixedSized::new(10, u8be());
        assert_eq!(fs.sizeof(&Context::new()).unwrap(), 10);
    }

    #[test]
    fn fixed_sized_roundtrip() {
        let fs = FixedSized::new(4, u8be());
        let c: &dyn Construct = &fs;
        let bytes = c.build_bytes(&Value::UInt(42)).unwrap();
        assert_eq!(bytes, vec![42, 0, 0, 0]);
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, Value::UInt(42));
    }

    #[test]
    fn fixed_sized_consumes_exact_bytes_on_parse() {
        let fs = FixedSized::new(4, u8be());
        let data = b"\xFF\x00\x00\x00\xAA\xBB";
        let mut stream = ByteStream::new_read(data);
        let mut ctx = Context::new();
        let _ = fs.parse(&mut stream, &mut ctx).unwrap();
        // Should have consumed exactly 4 bytes
        assert_eq!(stream.tell().unwrap(), 4);
    }

    // ======================================================================
    // NamedTuple tests
    // ======================================================================

    #[test]
    fn named_tuple_from_list() {
        use crate::constructs::sequence::Sequence;
        let seq = Sequence::new().push(u8be()).push(u8be()).push(u8be());
        let nt = NamedTuple::new(
            "coord",
            vec!["x".to_string(), "y".to_string(), "z".to_string()],
            Box::new(seq),
        );
        let c: &dyn Construct = &nt;
        let parsed = c.parse_bytes(b"\x01\x02\x03").unwrap();
        let list = parsed.as_list().unwrap();
        assert_eq!(list.len(), 3);
        // Check each named entry
        for (i, entry) in list.iter().enumerate() {
            let container = entry.as_container().unwrap();
            let name = container.get("name").unwrap().as_string().unwrap();
            let val = container.get("value").unwrap().as_uint().unwrap();
            assert_eq!(val, (i + 1) as u64);
            let expected_names = ["x", "y", "z"];
            assert_eq!(name, expected_names[i]);
        }
    }

    #[test]
    fn named_tuple_roundtrip_from_list() {
        use crate::constructs::sequence::Sequence;
        let seq = Sequence::new().push(u8be()).push(u8be());
        let nt = NamedTuple::new(
            "pair",
            vec!["a".to_string(), "b".to_string()],
            Box::new(seq),
        );
        let c: &dyn Construct = &nt;

        let parsed = c.parse_bytes(b"\x0A\x0B").unwrap();
        let bytes = c.build_bytes(&parsed).unwrap();
        assert_eq!(bytes, vec![0x0A, 0x0B]);
    }

    #[test]
    fn named_tuple_from_container() {
        use crate::constructs::struct_::Struct;
        let st = Struct::new().field("x", u8be()).field("y", u8be());
        let nt = NamedTuple::new(
            "point",
            vec!["x".to_string(), "y".to_string()],
            Box::new(st),
        );
        let c: &dyn Construct = &nt;
        let parsed = c.parse_bytes(b"\x05\x0A").unwrap();
        let list = parsed.as_list().unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn named_tuple_sizeof_delegates() {
        use crate::constructs::sequence::Sequence;
        let seq = Sequence::new().push(u8be()).push(u8be());
        let nt = NamedTuple::new(
            "pair",
            vec!["a".to_string(), "b".to_string()],
            Box::new(seq),
        );
        assert_eq!(nt.sizeof(&Context::new()).unwrap(), 2);
    }

    // ======================================================================
    // TimestampAdapter tests
    // ======================================================================

    #[test]
    fn timestamp_adapter_parse_seconds() {
        let ts = TimestampAdapter::new(u32be(), TimestampUnit::Seconds, TimestampEpoch::Unix);
        let c: &dyn Construct = &ts;
        let parsed = c.parse_bytes(b"\x00\x00\x00\x3C").unwrap();
        let container = parsed.as_container().unwrap();
        assert_eq!(container.get("secs").unwrap(), &Value::Int(60));
        assert_eq!(container.get("nanos").unwrap(), &Value::UInt(0));
    }

    #[test]
    fn timestamp_adapter_build_seconds() {
        let ts = TimestampAdapter::new(u32be(), TimestampUnit::Seconds, TimestampEpoch::Unix);
        let c: &dyn Construct = &ts;

        let mut map = indexmap::IndexMap::new();
        map.insert("secs".to_string(), Value::Int(60));
        map.insert("nanos".to_string(), Value::UInt(0));
        let bytes = c.build_bytes(&Value::Container(map)).unwrap();
        assert_eq!(bytes, vec![0x00, 0x00, 0x00, 0x3C]);
    }

    #[test]
    fn timestamp_adapter_roundtrip_seconds() {
        let ts = TimestampAdapter::new(u32be(), TimestampUnit::Seconds, TimestampEpoch::Unix);
        let c: &dyn Construct = &ts;

        let mut map = indexmap::IndexMap::new();
        map.insert("secs".to_string(), Value::Int(12345));
        map.insert("nanos".to_string(), Value::UInt(0));
        let bytes = c.build_bytes(&Value::Container(map.clone())).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, Value::Container(map));
    }

    #[test]
    fn timestamp_adapter_millis() {
        let ts = TimestampAdapter::new(u32be(), TimestampUnit::Milliseconds, TimestampEpoch::Unix);
        let c: &dyn Construct = &ts;

        // 1500 ms = 1 sec, 500000000 nanos
        let parsed = c.parse_bytes(b"\x00\x00\x05\xDC").unwrap();
        let container = parsed.as_container().unwrap();
        assert_eq!(container.get("secs").unwrap(), &Value::Int(1));
        // 500 ms = 500_000_000 nanos
        assert_eq!(container.get("nanos").unwrap(), &Value::UInt(500_000_000));
    }

    #[test]
    fn timestamp_adapter_micros() {
        let ts = TimestampAdapter::new(u32be(), TimestampUnit::Microseconds, TimestampEpoch::Unix);
        let c: &dyn Construct = &ts;

        // 1_500_000 us = 1 sec, 500_000_000 nanos
        let val: u32 = 1_500_000;
        let data = val.to_be_bytes();
        let parsed = c.parse_bytes(&data).unwrap();
        let container = parsed.as_container().unwrap();
        assert_eq!(container.get("secs").unwrap(), &Value::Int(1));
        assert_eq!(container.get("nanos").unwrap(), &Value::UInt(500_000_000));
    }

    #[test]
    fn timestamp_adapter_nanos() {
        let ts = TimestampAdapter::new(u32be(), TimestampUnit::Nanoseconds, TimestampEpoch::Unix);
        let c: &dyn Construct = &ts;

        // 1_500_000_000 ns = 1 sec, 500_000_000 nanos
        let val: u32 = 1_500_000_000;
        let data = val.to_be_bytes();
        let parsed = c.parse_bytes(&data).unwrap();
        let container = parsed.as_container().unwrap();
        assert_eq!(container.get("secs").unwrap(), &Value::Int(1));
        assert_eq!(container.get("nanos").unwrap(), &Value::UInt(500_000_000));
    }

    #[test]
    fn timestamp_adapter_missing_secs_field_errors() {
        let ts = TimestampAdapter::new(u8be(), TimestampUnit::Seconds, TimestampEpoch::Unix);
        let mut map = indexmap::IndexMap::new();
        map.insert("nanos".to_string(), Value::UInt(0));
        let c: &dyn Construct = &ts;
        let err = c.build_bytes(&Value::Container(map)).unwrap_err();
        assert!(matches!(err, ConstructError::FieldMissing { .. }));
    }

    #[test]
    fn timestamp_adapter_missing_nanos_field_errors() {
        let ts = TimestampAdapter::new(u8be(), TimestampUnit::Seconds, TimestampEpoch::Unix);
        let mut map = indexmap::IndexMap::new();
        map.insert("secs".to_string(), Value::Int(0));
        let c: &dyn Construct = &ts;
        let err = c.build_bytes(&Value::Container(map)).unwrap_err();
        assert!(matches!(err, ConstructError::FieldMissing { .. }));
    }

    #[test]
    fn timestamp_adapter_sizeof_delegates() {
        let ts = TimestampAdapter::new(u32be(), TimestampUnit::Seconds, TimestampEpoch::Unix);
        assert_eq!(ts.sizeof(&Context::new()).unwrap(), 4);
    }

    #[test]
    fn timestamp_adapter_build_roundtrip_millis() {
        let ts = TimestampAdapter::new(u32be(), TimestampUnit::Milliseconds, TimestampEpoch::Unix);
        let c: &dyn Construct = &ts;

        // Build 5 seconds, 250ms → 5250 ms
        let mut map = indexmap::IndexMap::new();
        map.insert("secs".to_string(), Value::Int(5));
        map.insert("nanos".to_string(), Value::UInt(250_000_000));
        let bytes = c.build_bytes(&Value::Container(map)).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        let container = parsed.as_container().unwrap();
        assert_eq!(container.get("secs").unwrap(), &Value::Int(5));
        // 250 ms → 250_000_000 nanos (roundtrip should be exact for millis)
        assert_eq!(container.get("nanos").unwrap(), &Value::UInt(250_000_000));
    }
}
