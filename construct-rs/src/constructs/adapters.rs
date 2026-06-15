//! Adapter and validator constructs — value transformation and validation.
//!
//! This module provides wrapper constructs that transform or validate values
//! produced/consumed by an inner construct:
//!
//! - [`Adapter`] — general-purpose bi-directional value transformation
//! - [`SymmetricAdapter`] — symmetric transformation (same function for both
//!   decode and encode)
//! - [`ExprAdapter`] — semantic alias for [`Adapter`], indicating that the
//!   decode/encode closures are expression-based
//! - [`Validator`] — validates parsed/built values, passing them through
//!   unchanged on success
//! - [`ExprValidator`] — semantic alias for [`Validator`], indicating that
//!   the check closure is expression-based
//!
//! # Python correspondence
//!
//! | Rust | Python |
//! |------|--------|
//! | [`Adapter`] | `Adapter` (line ~813) |
//! | [`SymmetricAdapter`] | `SymmetricAdapter` (line ~837) |
//! | [`Validator`] | `Validator` (line ~849) |
//!
//! In the Python version `Adapter` is an abstract base class with virtual
//! `_decode` / `_encode` methods. In Rust we use owned closures instead,
//! making each `Adapter` instance a concrete, self-contained construct.

use crate::combined::CombinedConstruct;
use crate::core::context::Context;
use crate::core::error::Result;
use crate::core::stream::CombinedStream;
use crate::core::Construct;
use crate::value::Value;
use std::sync::Arc;

// ===========================================================================
// Type aliases for closure-based decode / encode / check functions
// ===========================================================================
//
// These aliases use `Arc<dyn Fn ... + Send + Sync>` so that adapter/validator
// instances (and compiled execution trees that embed them) are `Send + Sync`
// and can be shared across threads. Construct APIs still accept `Box<dyn Fn
// ... + Send + Sync>` and convert internally via `Arc::from`, keeping call
// sites as `Box::new(closure)`.

/// Function signature for decoding (parsing) a value.
///
/// Receives the raw [`Value`] produced by the inner construct and the current
/// [`Context`], and returns the transformed value.
///
/// This is the canonical (stored) form using [`Arc`]. Constructors accept the
/// boxed form [`DecodeFuncBox`] and convert internally.
///
/// Corresponds to the Python `Adapter._decode(obj, context, path)` method.
pub type DecodeFunc = Arc<dyn Fn(&Value, &Context) -> Result<Value> + Send + Sync>;

/// Boxed decode function accepted by constructors; converted to
/// [`DecodeFunc`] internally. Call sites pass `Box::new(closure)`.
pub type DecodeFuncBox = Box<dyn Fn(&Value, &Context) -> Result<Value> + Send + Sync>;

/// Function signature for encoding (building) a value.
///
/// Receives the user-supplied [`Value`] and the current [`Context`], and
/// returns the encoded value that the inner construct will consume.
///
/// This is the canonical (stored) form using [`Arc`]. Constructors accept the
/// boxed form [`EncodeFuncBox`] and convert internally.
///
/// Corresponds to the Python `Adapter._encode(obj, context, path)` method.
pub type EncodeFunc = Arc<dyn Fn(&Value, &Context) -> Result<Value> + Send + Sync>;

/// Boxed encode function accepted by constructors; converted to
/// [`EncodeFunc`] internally. Call sites pass `Box::new(closure)`.
pub type EncodeFuncBox = Box<dyn Fn(&Value, &Context) -> Result<Value> + Send + Sync>;

/// Function signature for a symmetric value transformation.
///
/// Used by [`SymmetricAdapter`] where the same function is applied during
/// both parse and build.
///
/// This is the canonical (stored) form using [`Arc`]. Constructors accept the
/// boxed form [`SymmetricFuncBox`] and convert internally.
pub type SymmetricFunc = Arc<dyn Fn(&Value, &Context) -> Result<Value> + Send + Sync>;

/// Boxed symmetric function accepted by constructors; converted to
/// [`SymmetricFunc`] internally. Call sites pass `Box::new(closure)`.
pub type SymmetricFuncBox = Box<dyn Fn(&Value, &Context) -> Result<Value> + Send + Sync>;

/// Function signature for validating a value.
///
/// Receives the [`Value`] and the current [`Context`]. Returns `Ok(())` if
/// the value is valid, or `Err` with a [`ConstructError::Validation`] error
/// on failure.
///
/// This is the canonical (stored) form using [`Arc`]. Constructors accept the
/// boxed form [`CheckFuncBox`] and convert internally.
///
/// Corresponds to the Python `Validator._validate(obj, context, path)` method.
pub type CheckFunc = Arc<dyn Fn(&Value, &Context) -> Result<()> + Send + Sync>;

/// Boxed check function accepted by constructors; converted to
/// [`CheckFunc`] internally. Call sites pass `Box::new(closure)`.
pub type CheckFuncBox = Box<dyn Fn(&Value, &Context) -> Result<()> + Send + Sync>;

// ===========================================================================
// Adapter
// ===========================================================================

/// A bi-directional adapter that transforms values between the inner construct
/// and the outer world.
///
/// - **parse**: inner construct parses → `decode` function transforms the result
/// - **build**: `encode` function transforms the value → inner construct builds
/// - **sizeof**: delegated to the inner construct unchanged
///
/// Corresponds to the Python `Adapter` class (`construct/construct/core.py`
/// line ~813).
///
/// # Examples
///
/// ```
/// use construct::constructs::adapters::Adapter;
/// use construct::constructs::format_field::INT8UB;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// // Double the value: parse reads N, returns 2*N; build receives 2*N, writes N
/// let adapter = Adapter::new(
///     Box::new(INT8UB.into()),
///     Box::new(|v, _ctx| {
///         let n = v.to_u64()?;
///         Ok(Value::UInt(n * 2))
///     }.into()),
///     Box::new(|v, _ctx| {
///         let n = v.to_u64()?;
///         Ok(Value::UInt(n / 2))
///     }.into()),
/// );
///
/// let c: &dyn Construct = &adapter;
/// let parsed = c.parse_bytes(b"\x05").unwrap();
/// assert_eq!(parsed, Value::UInt(10));
///
/// let bytes = c.build_bytes(&Value::UInt(10)).unwrap();
/// assert_eq!(bytes, vec![5]);
/// ```
pub struct Adapter {
    /// The inner construct whose raw value is being adapted.
    pub subcon: Box<CombinedConstruct>,
    /// Decode function applied after parsing the inner construct.
    pub decode: DecodeFunc,
    /// Encode function applied before building the inner construct.
    pub encode: EncodeFunc,
}

impl Adapter {
    /// Creates a new `Adapter` with the given inner construct, decode, and
    /// encode functions.
    ///
    /// # Parameters
    ///
    /// - `subcon` — the inner construct
    /// - `decode` — function applied to the value produced by `subcon.parse()`
    /// - `encode` — function applied to the user value before `subcon.build()`
    ///
    /// The closures must be `Send + Sync`. They are stored internally as
    /// [`DecodeFunc`] / [`EncodeFunc`] (`Arc<dyn Fn + Send + Sync>`). Call
    /// sites typically pass `Box::new(closure)`, which is accepted as
    /// [`DecodeFuncBox`] / [`EncodeFuncBox`] and converted via [`Arc::from`].
    pub fn new(
        subcon: Box<CombinedConstruct>,
        decode: DecodeFuncBox,
        encode: EncodeFuncBox,
    ) -> Self {
        Adapter {
            subcon,
            decode: Arc::from(decode),
            encode: Arc::from(encode),
        }
    }
}

impl Construct for Adapter {
    fn parse(&self, stream: &mut CombinedStream, ctx: &mut Context) -> Result<Value> {
        let raw = self.subcon.parse(stream, ctx)?;
        (self.decode)(&raw, ctx)
    }

    fn build(&self, data: &Value, stream: &mut CombinedStream, ctx: &mut Context) -> Result<()> {
        let encoded = (self.encode)(data, ctx)?;
        self.subcon.build(&encoded, stream, ctx)
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        self.subcon.sizeof(ctx)
    }
}

// ===========================================================================
// SymmetricAdapter
// ===========================================================================

/// A symmetric adapter that uses a single function for both decoding and
/// encoding.
///
/// This is useful when the forward and inverse transformations are identical
/// (e.g. byte-swapping, checksumming).
///
/// - **parse**: inner construct parses → `func` transforms the result
/// - **build**: `func` transforms the value → inner construct builds
///
/// Corresponds to the Python `SymmetricAdapter` class
/// (`construct/construct/core.py` line ~837).
///
/// # Examples
///
/// ```
/// use construct::constructs::adapters::SymmetricAdapter;
/// use construct::constructs::format_field::INT8UB;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// // Negate: parse reads N, returns 255-N; build receives 255-N, writes N
/// let adapter = SymmetricAdapter::new(
///     Box::new(INT8UB.into()),
///     Box::new(|v, _ctx| {
///         let n = v.to_u64()?;
///         Ok(Value::UInt(255 - n))
///     }.into()),
/// );
///
/// let c: &dyn Construct = &adapter;
/// let parsed = c.parse_bytes(b"\x0A").unwrap();
/// assert_eq!(parsed, Value::UInt(245));
///
/// let bytes = c.build_bytes(&Value::UInt(245)).unwrap();
/// assert_eq!(bytes, vec![0x0A]);
/// ```
pub struct SymmetricAdapter {
    /// The inner construct whose raw value is being adapted.
    pub subcon: Box<CombinedConstruct>,
    /// The symmetric function applied during both parse and build.
    pub func: SymmetricFunc,
}

impl SymmetricAdapter {
    /// Creates a new `SymmetricAdapter` with the given inner construct and
    /// symmetric function.
    ///
    /// # Parameters
    ///
    /// - `subcon` — the inner construct
    /// - `func` — the function applied to values in both directions
    ///
    /// The closure must be `Send + Sync`. It is stored internally as
    /// [`SymmetricFunc`] (`Arc<dyn Fn + Send + Sync>`). Call sites typically
    /// pass `Box::new(closure)`, which is accepted as [`SymmetricFuncBox`]
    /// and converted via [`Arc::from`].
    pub fn new(subcon: Box<CombinedConstruct>, func: SymmetricFuncBox) -> Self {
        SymmetricAdapter {
            subcon,
            func: Arc::from(func),
        }
    }
}

impl Construct for SymmetricAdapter {
    fn parse(&self, stream: &mut CombinedStream, ctx: &mut Context) -> Result<Value> {
        let raw = self.subcon.parse(stream, ctx)?;
        (self.func)(&raw, ctx)
    }

    fn build(&self, data: &Value, stream: &mut CombinedStream, ctx: &mut Context) -> Result<()> {
        let encoded = (self.func)(data, ctx)?;
        self.subcon.build(&encoded, stream, ctx)
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        self.subcon.sizeof(ctx)
    }
}

// ===========================================================================
// ExprAdapter
// ===========================================================================

/// A semantic alias for [`Adapter`] that signals the decode/encode closures
/// are derived from an expression system.
///
/// In the Python original there is no separate `ExprAdapter` class; it is a
/// naming convention used in the Rust port to distinguish adapter instances
/// whose closures come from the expression subsystem (Phase 5) versus
/// hand-written closures.
///
/// Functionally identical to [`Adapter`].
pub struct ExprAdapter {
    /// The inner construct whose raw value is being adapted.
    pub subcon: Box<CombinedConstruct>,
    /// Decode function applied after parsing the inner construct.
    pub decode: DecodeFunc,
    /// Encode function applied before building the inner construct.
    pub encode: EncodeFunc,
}

impl ExprAdapter {
    /// Creates a new `ExprAdapter` with the given inner construct, decode,
    /// and encode functions.
    ///
    /// # Parameters
    ///
    /// - `subcon` — the inner construct
    /// - `decode` — function applied to the value produced by `subcon.parse()`
    /// - `encode` — function applied to the user value before `subcon.build()`
    ///
    /// The closures must be `Send + Sync`. They are stored internally as
    /// [`DecodeFunc`] / [`EncodeFunc`] (`Arc<dyn Fn + Send + Sync>`). Call
    /// sites typically pass `Box::new(closure)`, which is accepted as
    /// [`DecodeFuncBox`] / [`EncodeFuncBox`] and converted via [`Arc::from`].
    pub fn new(
        subcon: Box<CombinedConstruct>,
        decode: DecodeFuncBox,
        encode: EncodeFuncBox,
    ) -> Self {
        ExprAdapter {
            subcon,
            decode: Arc::from(decode),
            encode: Arc::from(encode),
        }
    }
}

impl Construct for ExprAdapter {
    fn parse(&self, stream: &mut CombinedStream, ctx: &mut Context) -> Result<Value> {
        let raw = self.subcon.parse(stream, ctx)?;
        (self.decode)(&raw, ctx)
    }

    fn build(&self, data: &Value, stream: &mut CombinedStream, ctx: &mut Context) -> Result<()> {
        let encoded = (self.encode)(data, ctx)?;
        self.subcon.build(&encoded, stream, ctx)
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        self.subcon.sizeof(ctx)
    }
}

// ===========================================================================
// Validator
// ===========================================================================

/// A validator construct that checks a condition on the parsed/built value.
///
/// The value passes through unchanged when validation succeeds. When
/// validation fails, a [`ConstructError::Validation`] error is returned.
///
/// This is implemented on top of [`SymmetricAdapter`]: the "adaptation" is
/// the identity function (value passes through), but a check is applied
/// first.
///
/// Corresponds to the Python `Validator` class
/// (`construct/construct/core.py` line ~849).
///
/// # Examples
///
/// ```
/// use construct::constructs::adapters::Validator;
/// use construct::constructs::format_field::INT8UB;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let validator = Validator::new(
///     Box::new(INT8UB.into()),
///     Box::new(|v, _ctx| {
///         let n = v.to_u64()?;
///         if n <= 10 {
///             Ok(())
///         } else {
///             Err(construct::core::error::ConstructError::Validation {
///                 path: String::new(),
///                 message: format!("value {} exceeds maximum 10", n),
///             })
///         }
///     }.into()),
/// );
///
/// let c: &dyn Construct = &validator;
/// // Valid value
/// let parsed = c.parse_bytes(b"\x05").unwrap();
/// assert_eq!(parsed, Value::UInt(5));
///
/// // Invalid value
/// let err = c.parse_bytes(b"\x20").unwrap_err();
/// assert!(matches!(err, construct::core::error::ConstructError::Validation { .. }));
/// ```
pub struct Validator {
    /// The inner construct whose value is being validated.
    pub subcon: Box<CombinedConstruct>,
    /// The validation function. Returns `Ok(())` on success.
    pub check: CheckFunc,
}

impl Validator {
    /// Creates a new `Validator` with the given inner construct and check
    /// function.
    ///
    /// # Parameters
    ///
    /// - `subcon` — the inner construct
    /// - `check` — function that validates the value; returns `Ok(())` on
    ///   success or `Err(ConstructError::Validation)` on failure
    ///
    /// The closure must be `Send + Sync`. It is stored internally as
    /// [`CheckFunc`] (`Arc<dyn Fn + Send + Sync>`). Call sites typically
    /// pass `Box::new(closure)`, which is accepted as [`CheckFuncBox`] and
    /// converted via [`Arc::from`].
    pub fn new(subcon: Box<CombinedConstruct>, check: CheckFuncBox) -> Self {
        Validator {
            subcon,
            check: Arc::from(check),
        }
    }
}

impl Construct for Validator {
    fn parse(&self, stream: &mut CombinedStream, ctx: &mut Context) -> Result<Value> {
        let raw = self.subcon.parse(stream, ctx)?;
        (self.check)(&raw, ctx)?;
        Ok(raw)
    }

    fn build(&self, data: &Value, stream: &mut CombinedStream, ctx: &mut Context) -> Result<()> {
        (self.check)(data, ctx)?;
        self.subcon.build(data, stream, ctx)
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        self.subcon.sizeof(ctx)
    }
}

// ===========================================================================
// ExprValidator
// ===========================================================================

/// A semantic alias for [`Validator`] that signals the check closure is
/// derived from an expression system.
///
/// Functionally identical to [`Validator`].
pub struct ExprValidator {
    /// The inner construct whose value is being validated.
    pub subcon: Box<CombinedConstruct>,
    /// The validation function. Returns `Ok(())` on success.
    pub check: CheckFunc,
}

impl ExprValidator {
    /// Creates a new `ExprValidator` with the given inner construct and check
    /// function.
    ///
    /// # Parameters
    ///
    /// - `subcon` — the inner construct
    /// - `check` — function that validates the value; returns `Ok(())` on
    ///   success or `Err(ConstructError::Validation)` on failure
    ///
    /// The closure must be `Send + Sync`. It is stored internally as
    /// [`CheckFunc`] (`Arc<dyn Fn + Send + Sync>`). Call sites typically
    /// pass `Box::new(closure)`, which is accepted as [`CheckFuncBox`] and
    /// converted via [`Arc::from`].
    pub fn new(subcon: Box<CombinedConstruct>, check: CheckFuncBox) -> Self {
        ExprValidator {
            subcon,
            check: Arc::from(check),
        }
    }
}

impl Construct for ExprValidator {
    fn parse(&self, stream: &mut CombinedStream, ctx: &mut Context) -> Result<Value> {
        let raw = self.subcon.parse(stream, ctx)?;
        (self.check)(&raw, ctx)?;
        Ok(raw)
    }

    fn build(&self, data: &Value, stream: &mut CombinedStream, ctx: &mut Context) -> Result<()> {
        (self.check)(data, ctx)?;
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
    use crate::constructs::format_field::{Endianness, FormatField, FormatKind};
    use crate::core::error::ConstructError;
    use crate::core::stream::ByteStream;

    /// Helper: creates a U8 big-endian construct for tests.
    fn u8be() -> Box<CombinedConstruct> {
        Box::new(FormatField::new(Endianness::Big, FormatKind::U8).into())
    }

    /// Helper: creates a U16 big-endian construct for tests.
    fn u16be() -> Box<CombinedConstruct> {
        Box::new(FormatField::new(Endianness::Big, FormatKind::U16).into())
    }

    // ======================================================================
    // Adapter tests
    // ======================================================================

    #[test]
    fn adapter_parse_applies_decode() {
        // decode: multiply by 2
        let adapter = Adapter::new(
            u8be(),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                Ok(Value::UInt(n * 2))
            }),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                Ok(Value::UInt(n / 2))
            }),
        );

        let c: &dyn Construct = &adapter;
        let parsed = c.parse_bytes(b"\x05").unwrap();
        assert_eq!(parsed, Value::UInt(10));
    }

    #[test]
    fn adapter_build_applies_encode() {
        // decode: multiply by 2; encode: divide by 2
        let adapter = Adapter::new(
            u8be(),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                Ok(Value::UInt(n * 2))
            }),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                Ok(Value::UInt(n / 2))
            }),
        );

        let c: &dyn Construct = &adapter;
        let bytes = c.build_bytes(&Value::UInt(20)).unwrap();
        assert_eq!(bytes, vec![10]);
    }

    #[test]
    fn adapter_roundtrip_preserves_value() {
        // decode: multiply by 3; encode: divide by 3
        let adapter = Adapter::new(
            u8be(),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                Ok(Value::UInt(n * 3))
            }),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                Ok(Value::UInt(n / 3))
            }),
        );

        let c: &dyn Construct = &adapter;
        let original = Value::UInt(21);
        let bytes = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn adapter_sizeof_delegates_to_subcon() {
        let adapter = Adapter::new(
            u16be(),
            Box::new(|v, _ctx| Ok(v.clone())),
            Box::new(|v, _ctx| Ok(v.clone())),
        );
        let ctx = Context::new();
        assert_eq!(adapter.sizeof(&ctx).unwrap(), 2);
    }

    #[test]
    fn adapter_decode_error_propagates() {
        let adapter = Adapter::new(
            u8be(),
            Box::new(|_v, _ctx| {
                {
                    Err(ConstructError::Validation {
                        path: String::new(),
                        message: "decode failed".to_string(),
                    })
                }
            }),
            Box::new(|v, _ctx| Ok(v.clone())),
        );

        let c: &dyn Construct = &adapter;
        let err = c.parse_bytes(b"\x05").unwrap_err();
        assert!(matches!(err, ConstructError::Validation { .. }));
    }

    #[test]
    fn adapter_encode_error_propagates() {
        let adapter = Adapter::new(
            u8be(),
            Box::new(|v, _ctx| Ok(v.clone())),
            Box::new(|_v, _ctx| {
                {
                    Err(ConstructError::Validation {
                        path: String::new(),
                        message: "encode failed".to_string(),
                    })
                }
            }),
        );

        let c: &dyn Construct = &adapter;
        let err = c.build_bytes(&Value::UInt(5)).unwrap_err();
        assert!(matches!(err, ConstructError::Validation { .. }));
    }

    #[test]
    fn adapter_subcon_parse_error_propagates() {
        // Build adapter on u8, then feed empty data
        let adapter = Adapter::new(
            u8be(),
            Box::new(|v, _ctx| Ok(v.clone())),
            Box::new(|v, _ctx| Ok(v.clone())),
        );

        let c: &dyn Construct = &adapter;
        let err = c.parse_bytes(b"").unwrap_err();
        assert!(matches!(err, ConstructError::Stream { .. }));
    }

    #[test]
    fn adapter_decode_uses_context() {
        // decode adds context value to parsed value
        let adapter = Adapter::new(
            u8be(),
            Box::new(|v, ctx| {
                let offset = ctx
                    .get("offset")
                    .map(|val| val.to_u64().unwrap_or(0))
                    .unwrap_or(0);
                let n = v.to_u64()?;
                Ok(Value::UInt(n + offset))
            }),
            Box::new(|v, _ctx| Ok(v.clone())),
        );

        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\x0A"));
        let mut ctx = Context::new();
        ctx.insert("offset", Value::UInt(100));
        let parsed = adapter.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(parsed, Value::UInt(110));
    }

    // ======================================================================
    // SymmetricAdapter tests
    // ======================================================================

    #[test]
    fn symmetric_adapter_parse_applies_func() {
        // func: negate byte value (255 - n)
        let adapter = SymmetricAdapter::new(
            u8be(),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                Ok(Value::UInt(255 - n))
            }),
        );

        let c: &dyn Construct = &adapter;
        let parsed = c.parse_bytes(b"\x0A").unwrap();
        assert_eq!(parsed, Value::UInt(245));
    }

    #[test]
    fn symmetric_adapter_build_applies_same_func() {
        let adapter = SymmetricAdapter::new(
            u8be(),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                Ok(Value::UInt(255 - n))
            }),
        );

        let c: &dyn Construct = &adapter;
        let bytes = c.build_bytes(&Value::UInt(245)).unwrap();
        assert_eq!(bytes, vec![0x0A]);
    }

    #[test]
    fn symmetric_adapter_roundtrip() {
        let adapter = SymmetricAdapter::new(
            u8be(),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                Ok(Value::UInt(255 - n))
            }),
        );

        let c: &dyn Construct = &adapter;
        let original = Value::UInt(100);
        let bytes = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn symmetric_adapter_sizeof_delegates_to_subcon() {
        let adapter = SymmetricAdapter::new(u16be(), Box::new(|v, _ctx| Ok(v.clone())));
        let ctx = Context::new();
        assert_eq!(adapter.sizeof(&ctx).unwrap(), 2);
    }

    #[test]
    fn symmetric_adapter_func_error_propagates() {
        let adapter = SymmetricAdapter::new(
            u8be(),
            Box::new(|_v, _ctx| {
                {
                    Err(ConstructError::Validation {
                        path: String::new(),
                        message: "symmetric fail".to_string(),
                    })
                }
            }),
        );

        let c: &dyn Construct = &adapter;
        // parse error
        let err = c.parse_bytes(b"\x05").unwrap_err();
        assert!(matches!(err, ConstructError::Validation { .. }));

        // build error
        let err = c.build_bytes(&Value::UInt(5)).unwrap_err();
        assert!(matches!(err, ConstructError::Validation { .. }));
    }

    #[test]
    fn symmetric_adapter_identity_roundtrip() {
        // Identity function: no transformation
        let adapter = SymmetricAdapter::new(u8be(), Box::new(|v, _ctx| Ok(v.clone())));

        let c: &dyn Construct = &adapter;
        let original = Value::UInt(42);
        let bytes = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
        assert_eq!(bytes, vec![42]);
    }

    // ======================================================================
    // ExprAdapter tests
    // ======================================================================

    #[test]
    fn expr_adapter_parse_applies_decode() {
        let adapter = ExprAdapter::new(
            u8be(),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                Ok(Value::UInt(n + 1000))
            }),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                Ok(Value::UInt(n.saturating_sub(1000)))
            }),
        );

        let c: &dyn Construct = &adapter;
        let parsed = c.parse_bytes(b"\x05").unwrap();
        assert_eq!(parsed, Value::UInt(1005));
    }

    #[test]
    fn expr_adapter_build_applies_encode() {
        let adapter = ExprAdapter::new(
            u8be(),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                Ok(Value::UInt(n + 1000))
            }),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                Ok(Value::UInt(n.saturating_sub(1000)))
            }),
        );

        let c: &dyn Construct = &adapter;
        let bytes = c.build_bytes(&Value::UInt(1005)).unwrap();
        assert_eq!(bytes, vec![5]);
    }

    #[test]
    fn expr_adapter_roundtrip() {
        let adapter = ExprAdapter::new(
            u8be(),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                Ok(Value::UInt(n + 1000))
            }),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                Ok(Value::UInt(n.saturating_sub(1000)))
            }),
        );

        let c: &dyn Construct = &adapter;
        let original = Value::UInt(1042);
        let bytes = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn expr_adapter_sizeof_delegates_to_subcon() {
        let adapter = ExprAdapter::new(
            u16be(),
            Box::new(|v, _ctx| Ok(v.clone())),
            Box::new(|v, _ctx| Ok(v.clone())),
        );
        let ctx = Context::new();
        assert_eq!(adapter.sizeof(&ctx).unwrap(), 2);
    }

    // ======================================================================
    // Validator tests
    // ======================================================================

    #[test]
    fn validator_parse_passes_valid_value() {
        let validator = Validator::new(
            u8be(),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                if n <= 10 {
                    Ok(())
                } else {
                    Err(ConstructError::Validation {
                        path: String::new(),
                        message: format!("value {} exceeds maximum 10", n),
                    })
                }
            }),
        );

        let c: &dyn Construct = &validator;
        let parsed = c.parse_bytes(b"\x05").unwrap();
        assert_eq!(parsed, Value::UInt(5));
    }

    #[test]
    fn validator_parse_rejects_invalid_value() {
        let validator = Validator::new(
            u8be(),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                if n <= 10 {
                    Ok(())
                } else {
                    Err(ConstructError::Validation {
                        path: String::new(),
                        message: format!("value {} exceeds maximum 10", n),
                    })
                }
            }),
        );

        let c: &dyn Construct = &validator;
        let err = c.parse_bytes(b"\x20").unwrap_err();
        assert!(matches!(err, ConstructError::Validation { .. }));
    }

    #[test]
    fn validator_build_passes_valid_value() {
        let validator = Validator::new(
            u8be(),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                if n <= 10 {
                    Ok(())
                } else {
                    Err(ConstructError::Validation {
                        path: String::new(),
                        message: format!("value {} exceeds maximum 10", n),
                    })
                }
            }),
        );

        let c: &dyn Construct = &validator;
        let bytes = c.build_bytes(&Value::UInt(5)).unwrap();
        assert_eq!(bytes, vec![5]);
    }

    #[test]
    fn validator_build_rejects_invalid_value() {
        let validator = Validator::new(
            u8be(),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                if n <= 10 {
                    Ok(())
                } else {
                    Err(ConstructError::Validation {
                        path: String::new(),
                        message: format!("value {} exceeds maximum 10", n),
                    })
                }
            }),
        );

        let c: &dyn Construct = &validator;
        let err = c.build_bytes(&Value::UInt(50)).unwrap_err();
        assert!(matches!(err, ConstructError::Validation { .. }));
    }

    #[test]
    fn validator_roundtrip_with_valid_value() {
        let validator = Validator::new(
            u8be(),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                if n <= 200 {
                    Ok(())
                } else {
                    Err(ConstructError::Validation {
                        path: String::new(),
                        message: "out of range".to_string(),
                    })
                }
            }),
        );

        let c: &dyn Construct = &validator;
        let original = Value::UInt(42);
        let bytes = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn validator_sizeof_delegates_to_subcon() {
        let validator = Validator::new(u16be(), Box::new(|_v, _ctx| Ok(())));
        let ctx = Context::new();
        assert_eq!(validator.sizeof(&ctx).unwrap(), 2);
    }

    #[test]
    fn validator_value_passes_through_unchanged() {
        // Validator must return the exact same value from the subcon,
        // not a modified copy.
        let validator = Validator::new(u8be(), Box::new(|_v, _ctx| Ok(())));

        let c: &dyn Construct = &validator;
        let parsed = c.parse_bytes(b"\xAB").unwrap();
        assert_eq!(parsed, Value::UInt(0xAB));
    }

    #[test]
    fn validator_subcon_error_propagates() {
        let validator = Validator::new(u8be(), Box::new(|_v, _ctx| Ok(())));

        let c: &dyn Construct = &validator;
        let err = c.parse_bytes(b"").unwrap_err();
        assert!(matches!(err, ConstructError::Stream { .. }));
    }

    #[test]
    fn validator_check_uses_context() {
        // Validate against a dynamic maximum from context
        let validator = Validator::new(
            u8be(),
            Box::new(|v, ctx| {
                let max = ctx
                    .get("max_value")
                    .map(|val| val.to_u64().unwrap_or(0))
                    .unwrap_or(0);
                let n = v.to_u64()?;
                if n <= max {
                    Ok(())
                } else {
                    Err(ConstructError::Validation {
                        path: String::new(),
                        message: format!("value {} exceeds context max {}", n, max),
                    })
                }
            }),
        );

        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\x05"));
        let mut ctx = Context::new();
        ctx.insert("max_value", Value::UInt(10));
        let parsed = validator.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(parsed, Value::UInt(5));

        // Now with a too-low max
        let mut stream2 = CombinedStream::ByteStream(ByteStream::new_read(b"\x05"));
        let mut ctx2 = Context::new();
        ctx2.insert("max_value", Value::UInt(3));
        let err = validator.parse(&mut stream2, &mut ctx2).unwrap_err();
        assert!(matches!(err, ConstructError::Validation { .. }));
    }

    // ======================================================================
    // ExprValidator tests
    // ======================================================================

    #[test]
    fn expr_validator_parse_passes_valid_value() {
        let validator = ExprValidator::new(
            u8be(),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                if n > 0 {
                    Ok(())
                } else {
                    Err(ConstructError::Validation {
                        path: String::new(),
                        message: "value must be positive".to_string(),
                    })
                }
            }),
        );

        let c: &dyn Construct = &validator;
        let parsed = c.parse_bytes(b"\x01").unwrap();
        assert_eq!(parsed, Value::UInt(1));
    }

    #[test]
    fn expr_validator_parse_rejects_invalid_value() {
        let validator = ExprValidator::new(
            u8be(),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                if n > 0 {
                    Ok(())
                } else {
                    Err(ConstructError::Validation {
                        path: String::new(),
                        message: "value must be positive".to_string(),
                    })
                }
            }),
        );

        let c: &dyn Construct = &validator;
        let err = c.parse_bytes(b"\x00").unwrap_err();
        assert!(matches!(err, ConstructError::Validation { .. }));
    }

    #[test]
    fn expr_validator_build_validates() {
        let validator = ExprValidator::new(
            u8be(),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                if n > 0 {
                    Ok(())
                } else {
                    Err(ConstructError::Validation {
                        path: String::new(),
                        message: "value must be positive".to_string(),
                    })
                }
            }),
        );

        let c: &dyn Construct = &validator;
        // Valid build
        let bytes = c.build_bytes(&Value::UInt(7)).unwrap();
        assert_eq!(bytes, vec![7]);

        // Invalid build
        let err = c.build_bytes(&Value::UInt(0)).unwrap_err();
        assert!(matches!(err, ConstructError::Validation { .. }));
    }

    #[test]
    fn expr_validator_roundtrip() {
        let validator = ExprValidator::new(
            u8be(),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                if (1..=100).contains(&n) {
                    Ok(())
                } else {
                    Err(ConstructError::Validation {
                        path: String::new(),
                        message: "out of range".to_string(),
                    })
                }
            }),
        );

        let c: &dyn Construct = &validator;
        let original = Value::UInt(50);
        let bytes = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn expr_validator_sizeof_delegates_to_subcon() {
        let validator = ExprValidator::new(u16be(), Box::new(|_v, _ctx| Ok(())));
        let ctx = Context::new();
        assert_eq!(validator.sizeof(&ctx).unwrap(), 2);
    }

    // ======================================================================
    // Cross-cutting: build_bytes / parse_bytes integration
    // ======================================================================

    #[test]
    fn adapter_parse_bytes_convenience() {
        let adapter = Adapter::new(
            u8be(),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                Ok(Value::Int(-(n as i64)))
            }),
            Box::new(|v, _ctx| {
                let n = v.to_i64()?;
                Ok(Value::UInt((-n) as u64))
            }),
        );

        let c: &dyn Construct = &adapter;
        let parsed = c.parse_bytes(b"\xFF").unwrap();
        assert_eq!(parsed, Value::Int(-255));
    }

    #[test]
    fn adapter_build_bytes_convenience() {
        let adapter = Adapter::new(
            u8be(),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                Ok(Value::Int(-(n as i64)))
            }),
            Box::new(|v, _ctx| {
                let n = v.to_i64()?;
                Ok(Value::UInt((-n) as u64))
            }),
        );

        let c: &dyn Construct = &adapter;
        let bytes = c.build_bytes(&Value::Int(-42)).unwrap();
        assert_eq!(bytes, vec![42]);
    }

    #[test]
    fn validator_build_bytes_rejects_before_write() {
        // The validator should reject the value BEFORE any bytes are written
        // to the inner construct's stream.
        let validator = Validator::new(
            u8be(),
            Box::new(|_v, _ctx| {
                {
                    Err(ConstructError::Validation {
                        path: String::new(),
                        message: "always reject".to_string(),
                    })
                }
            }),
        );

        let c: &dyn Construct = &validator;
        let err = c.build_bytes(&Value::UInt(0)).unwrap_err();
        assert!(matches!(err, ConstructError::Validation { .. }));
    }

    // ======================================================================
    // Adapter with subcon that returns complex values
    // ======================================================================

    #[test]
    fn adapter_with_bytes_subcon() {
        use crate::constructs::bytes::Bytes;

        // Decode: convert bytes to a hex string
        // Encode: convert hex string back to bytes
        let adapter = Adapter::new(
            Box::new(Bytes::new(4).into()),
            Box::new(|v, _ctx| {
                let bytes = v.as_bytes()?;
                let hex: String = bytes.iter().map(|b| format!("{:02X}", b)).collect();
                Ok(Value::String(hex))
            }),
            Box::new(|v, _ctx| {
                let s = v.as_string()?;
                let mut bytes = Vec::new();
                let chars: Vec<char> = s.chars().collect();
                let mut i = 0;
                while i + 1 < chars.len() {
                    let byte_val =
                        u8::from_str_radix(&chars[i..i + 2].iter().collect::<String>(), 16);
                    match byte_val {
                        Ok(b) => bytes.push(b),
                        Err(_) => {
                            return Err(ConstructError::Generic {
                                path: String::new(),
                                message: format!("invalid hex string: {}", s),
                            });
                        }
                    }
                    i += 2;
                }
                Ok(Value::Bytes(bytes))
            }),
        );

        let c: &dyn Construct = &adapter;
        let parsed = c.parse_bytes(b"\xDE\xAD\xBE\xEF").unwrap();
        assert_eq!(parsed, Value::String("DEADBEEF".to_string()));

        let bytes = c
            .build_bytes(&Value::String("DEADBEEF".to_string()))
            .unwrap();
        assert_eq!(bytes, vec![0xDE, 0xAD, 0xBE, 0xEF]);
    }

    // ======================================================================
    // Nested adapters
    // ======================================================================

    #[test]
    fn nested_adapters_compose() {
        // Inner: +10; Outer: *2
        // parse: raw → +10 → *2
        // build: *2 → +10 → raw (but reversed: encode outer first, then inner)
        // Actually: build outer.encode(val) → inner.encode(result) → subcon.build
        let inner = Adapter::new(
            u8be(),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                Ok(Value::UInt(n + 10))
            }),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                Ok(Value::UInt(n.saturating_sub(10)))
            }),
        );

        let outer = Adapter::new(
            Box::new(inner.into()),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                Ok(Value::UInt(n * 2))
            }),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                Ok(Value::UInt(n / 2))
            }),
        );

        let c: &dyn Construct = &outer;

        // Parse: raw byte = 5 → inner decode: 5+10=15 → outer decode: 15*2=30
        let parsed = c.parse_bytes(b"\x05").unwrap();
        assert_eq!(parsed, Value::UInt(30));

        // Build: outer encode: 30/2=15 → inner encode: 15-10=5 → byte = 5
        let bytes = c.build_bytes(&Value::UInt(30)).unwrap();
        assert_eq!(bytes, vec![5]);
    }

    // ======================================================================
    // Validator with SymmetricAdapter-style behaviour
    // ======================================================================

    #[test]
    fn validator_allows_zero() {
        let validator = Validator::new(
            u8be(),
            Box::new(|v, _ctx| {
                let n = v.to_u64()?;
                if n == 0 {
                    Ok(())
                } else {
                    Err(ConstructError::Validation {
                        path: String::new(),
                        message: format!("expected 0, got {}", n),
                    })
                }
            }),
        );

        let c: &dyn Construct = &validator;
        let parsed = c.parse_bytes(b"\x00").unwrap();
        assert_eq!(parsed, Value::UInt(0));

        let bytes = c.build_bytes(&Value::UInt(0)).unwrap();
        assert_eq!(bytes, vec![0]);
    }

    #[test]
    fn validator_rejects_all() {
        let validator = Validator::new(
            u8be(),
            Box::new(|_v, _ctx| {
                {
                    Err(ConstructError::Validation {
                        path: String::new(),
                        message: "nothing is valid".to_string(),
                    })
                }
            }),
        );

        let c: &dyn Construct = &validator;
        let err = c.parse_bytes(b"\x00").unwrap_err();
        assert!(matches!(err, ConstructError::Validation { .. }));

        let err = c.build_bytes(&Value::UInt(0)).unwrap_err();
        assert!(matches!(err, ConstructError::Validation { .. }));
    }
}
