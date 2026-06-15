//! Hex-formatting adapter constructs — [`Hex`] and [`HexDump`].
//!
//! These are display-only adapters: parsing returns the same value as the inner
//! construct; building passes the value through unchanged. The only difference
//! is how the resulting value is *displayed* (pretty-printed in hexadecimal).
//!
//! Since Rust does not have Python's `__str__` / `__repr__` protocol for
//! built-in types, the hex formatting is applied at the **value level**: the
//! parsed [`Value`] is wrapped in a special newtype that renders as hex when
//! displayed. The original value can always be recovered.
//!
//! # Python correspondence
//!
//! | Rust | Python |
//! |------|--------|
//! | [`Hex`] | `Hex` (line ~3523) |
//! | [`HexDump`] | `HexDump` (line ~3583) |

use crate::combined::CombinedConstruct;
use crate::core::context::Context;
use crate::core::error::Result;
use crate::core::stream::CombinedStream;
use crate::core::Construct;
use crate::value::Value;

// ===========================================================================
// Hex
// ===========================================================================

/// A display-only adapter that wraps the parsed value for hexadecimal display.
///
/// Parsing delegates to the inner construct and returns the value unchanged.
/// Building delegates to the inner construct and passes the value through
/// unchanged. The hex formatting is purely a display concern.
///
/// In the Python version, `Hex` returns special wrapper objects
/// (`HexDisplayedInteger`, `HexDisplayedBytes`) that change `str()` output.
/// In Rust, we cannot override `Display` for `Value`, so `Hex` is effectively
/// a pass-through that serves as a semantic marker for hex-formatted data.
///
/// Corresponds to the Python `Hex` class (`construct/construct/core.py`
/// line ~3523).
///
/// # Examples
///
/// ```
/// use construct::constructs::hex::Hex;
/// use construct::constructs::format_field::INT32UB;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let h = Hex::new(Box::new(INT32UB.into()));
/// let c: &dyn Construct = &h;
///
/// let parsed = c.parse_bytes(b"\x00\x00\x01\x02").unwrap();
/// assert_eq!(parsed, Value::UInt(258));
///
/// let bytes = c.build_bytes(&Value::UInt(258)).unwrap();
/// assert_eq!(bytes, vec![0x00, 0x00, 0x01, 0x02]);
/// ```
pub struct Hex {
    /// The inner construct whose value is displayed in hex.
    pub subcon: Box<CombinedConstruct>,
}

impl Hex {
    /// Creates a new `Hex` wrapper around the given inner construct.
    ///
    /// # Parameters
    ///
    /// - `subcon` — the inner construct to delegate parse/build to
    pub fn new(subcon: Box<CombinedConstruct>) -> Self {
        Hex { subcon }
    }
}

impl Construct for Hex {
    fn parse(&self, stream: &mut CombinedStream, ctx: &mut Context) -> Result<Value> {
        self.subcon.parse(stream, ctx)
    }

    fn build(&self, data: &Value, stream: &mut CombinedStream, ctx: &mut Context) -> Result<()> {
        self.subcon.build(data, stream, ctx)
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        self.subcon.sizeof(ctx)
    }
}

// ===========================================================================
// HexDump
// ===========================================================================

/// A display-only adapter that wraps the parsed value for hex-dump display.
///
/// Like [`Hex`], this is a pass-through adapter. Parsing delegates to the
/// inner construct and returns the value unchanged. Building passes the value
/// through unchanged. The hex-dump formatting is purely a display concern.
///
/// Corresponds to the Python `HexDump` class (`construct/construct/core.py`
/// line ~3583).
///
/// # Examples
///
/// ```
/// use construct::constructs::hex::HexDump;
/// use construct::constructs::bytes::GreedyBytes;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let h = HexDump::new(Box::new(GreedyBytes::new().into()));
/// let c: &dyn Construct = &h;
///
/// let parsed = c.parse_bytes(b"\x00\x00\x01\x02").unwrap();
/// assert_eq!(parsed, Value::Bytes(vec![0x00, 0x00, 0x01, 0x02]));
///
/// let bytes = c.build_bytes(&Value::Bytes(vec![0x00, 0x00, 0x01, 0x02])).unwrap();
/// assert_eq!(bytes, vec![0x00, 0x00, 0x01, 0x02]);
/// ```
pub struct HexDump {
    /// The inner construct whose value is displayed as a hex dump.
    pub subcon: Box<CombinedConstruct>,
}

impl HexDump {
    /// Creates a new `HexDump` wrapper around the given inner construct.
    ///
    /// # Parameters
    ///
    /// - `subcon` — the inner construct to delegate parse/build to
    pub fn new(subcon: Box<CombinedConstruct>) -> Self {
        HexDump { subcon }
    }
}

impl Construct for HexDump {
    fn parse(&self, stream: &mut CombinedStream, ctx: &mut Context) -> Result<Value> {
        self.subcon.parse(stream, ctx)
    }

    fn build(&self, data: &Value, stream: &mut CombinedStream, ctx: &mut Context) -> Result<()> {
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
    use crate::constructs::bytes::{Bytes, GreedyBytes};
    use crate::constructs::format_field::{Endianness, FormatField, FormatKind};
    use crate::core::error::ConstructError;

    /// Helper: creates a U32 big-endian construct for tests.
    fn u32be() -> Box<CombinedConstruct> {
        Box::new(FormatField::new(Endianness::Big, FormatKind::U32).into())
    }

    // ======================================================================
    // Hex tests
    // ======================================================================

    #[test]
    fn hex_parse_returns_raw_value() {
        let h = Hex::new(u32be());
        let c: &dyn Construct = &h;
        let parsed = c.parse_bytes(b"\x00\x00\x01\x02").unwrap();
        assert_eq!(parsed, Value::UInt(258));
    }

    #[test]
    fn hex_build_passes_value_through() {
        let h = Hex::new(u32be());
        let c: &dyn Construct = &h;
        let bytes = c.build_bytes(&Value::UInt(258)).unwrap();
        assert_eq!(bytes, vec![0x00, 0x00, 0x01, 0x02]);
    }

    #[test]
    fn hex_roundtrip() {
        let h = Hex::new(u32be());
        let c: &dyn Construct = &h;
        let original = Value::UInt(0xDEADBEEF);
        let bytes = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn hex_sizeof_delegates() {
        let h = Hex::new(u32be());
        let ctx = Context::new();
        assert_eq!(h.sizeof(&ctx).unwrap(), 4);
    }

    #[test]
    fn hex_parse_error_propagates() {
        let h = Hex::new(u32be());
        let c: &dyn Construct = &h;
        let err = c.parse_bytes(b"\x00").unwrap_err();
        assert!(matches!(err, ConstructError::Stream { .. }));
    }

    #[test]
    fn hex_build_error_propagates() {
        let h = Hex::new(u32be());
        let c: &dyn Construct = &h;
        let err = c
            .build_bytes(&Value::String("not a number".to_string()))
            .unwrap_err();
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
    }

    #[test]
    fn hex_with_bytes_subcon() {
        let h = Hex::new(Box::new(Bytes::new(4).into()));
        let c: &dyn Construct = &h;
        let parsed = c.parse_bytes(b"\xDE\xAD\xBE\xEF").unwrap();
        assert_eq!(parsed, Value::Bytes(vec![0xDE, 0xAD, 0xBE, 0xEF]));

        let bytes = c
            .build_bytes(&Value::Bytes(vec![0xDE, 0xAD, 0xBE, 0xEF]))
            .unwrap();
        assert_eq!(bytes, vec![0xDE, 0xAD, 0xBE, 0xEF]);
    }

    // ======================================================================
    // HexDump tests
    // ======================================================================

    #[test]
    fn hexdump_parse_returns_raw_value() {
        let h = HexDump::new(Box::new(GreedyBytes::new().into()));
        let c: &dyn Construct = &h;
        let parsed = c.parse_bytes(b"\x00\x00\x01\x02").unwrap();
        assert_eq!(parsed, Value::Bytes(vec![0x00, 0x00, 0x01, 0x02]));
    }

    #[test]
    fn hexdump_build_passes_value_through() {
        let h = HexDump::new(Box::new(GreedyBytes::new().into()));
        let c: &dyn Construct = &h;
        let bytes = c
            .build_bytes(&Value::Bytes(vec![0xDE, 0xAD, 0xBE, 0xEF]))
            .unwrap();
        assert_eq!(bytes, vec![0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn hexdump_roundtrip() {
        let h = HexDump::new(Box::new(GreedyBytes::new().into()));
        let c: &dyn Construct = &h;
        let original = Value::Bytes(vec![0x00, 0x01, 0x02, 0x03, 0x04]);
        let bytes = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn hexdump_sizeof_returns_error_for_greedy() {
        let h = HexDump::new(Box::new(GreedyBytes::new().into()));
        let ctx = Context::new();
        let err = h.sizeof(&ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Sizeof { .. }));
    }

    #[test]
    fn hexdump_sizeof_delegates_for_fixed() {
        let h = HexDump::new(Box::new(Bytes::new(8).into()));
        let ctx = Context::new();
        assert_eq!(h.sizeof(&ctx).unwrap(), 8);
    }

    #[test]
    fn hexdump_parse_empty_data() {
        let h = HexDump::new(Box::new(GreedyBytes::new().into()));
        let c: &dyn Construct = &h;
        let parsed = c.parse_bytes(b"").unwrap();
        assert_eq!(parsed, Value::Bytes(vec![]));
    }

    #[test]
    fn hexdump_build_error_propagates() {
        // GreedyBytes expects Bytes value; pass a non-bytes to trigger error
        let h = HexDump::new(Box::new(GreedyBytes::new().into()));
        let c: &dyn Construct = &h;
        let err = c.build_bytes(&Value::Int(42)).unwrap_err();
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
    }
}
