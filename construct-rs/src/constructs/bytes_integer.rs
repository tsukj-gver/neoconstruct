//! BytesInteger and BitsInteger constructs — arbitrary-width integer fields.
//!
//! - [`BytesInteger`] reads/writes an integer of arbitrary byte width.
//! - [`BitsInteger`] reads/writes an integer of arbitrary bit width
//!   (must be used inside a `Bitwise` context).
//!
//! Corresponds to Python `BytesInteger(length, signed, swapped)` and
//! `BitsInteger(length, signed, swapped)`.
//!
//! # Convenience constants
//!
//! | Constant | Description |
//! |----------|-------------|
//! | [`INT24UB`] | 24-bit unsigned big-endian |
//! | [`INT24UL`] | 24-bit unsigned little-endian |
//! | [`INT24SB`] | 24-bit signed big-endian |
//! | [`INT24SL`] | 24-bit signed little-endian |
//! | [`BIT`] | 1-bit unsigned |
//! | [`NIBBLE`] | 4-bit unsigned |
//! | [`OCTET`] | 8-bit unsigned |

use crate::binary;
use crate::core::context::Context;
use crate::core::error::{ConstructError, Result};
use crate::core::stream::CombinedStream;
use crate::core::stream::Stream;
use crate::core::Construct;
use crate::value::Value;

// ===========================================================================
// Helper: extract i128 from any integer Value variant
// ===========================================================================

/// Extracts an `i128` from any integer-like [`Value`] variant.
///
/// # Errors
///
/// Returns [`ConstructError::TypeMismatch`] if `value` is not an integer type.
fn value_to_i128(value: &Value) -> Result<i128> {
    match value {
        Value::Int(i) => Ok(*i as i128),
        Value::UInt(u) => Ok(*u as i128),
        Value::BigInt(i) => Ok(*i),
        other => Err(ConstructError::TypeMismatch {
            path: String::new(),
            expected: "integer".to_string(),
            actual: other.type_name().to_string(),
        }),
    }
}

/// Maps a raw `i128` integer to the most appropriate [`Value`] variant based on
/// the byte `length` and `signed` flag.
///
/// | `length` | `signed` | Result |
/// |----------|----------|--------|
/// | 1 | false | `Value::UInt(u8)` |
/// | 2 | false | `Value::UInt(u16)` |
/// | 4 | false | `Value::UInt(u32)` |
/// | 8 | false | `Value::UInt(u64)` |
/// | 1 | true | `Value::Int(i8)` |
/// | 2 | true | `Value::Int(i16)` |
/// | 4 | true | `Value::Int(i32)` |
/// | 8 | true | `Value::Int(i64)` |
/// | other | any | `Value::BigInt(i128)` |
fn map_to_value(number: i128, length: usize, signed: bool) -> Value {
    match (length, signed) {
        (1, false) => Value::UInt(number as u64),
        (2, false) => Value::UInt(number as u64),
        (4, false) => Value::UInt(number as u64),
        (8, false) => Value::UInt(number as u64),
        (1, true) => Value::Int(number as i64),
        (2, true) => Value::Int(number as i64),
        (4, true) => Value::Int(number as i64),
        (8, true) => Value::Int(number as i64),
        _ => Value::BigInt(number),
    }
}

// ===========================================================================
// BytesInteger
// ===========================================================================

/// An integer construct that operates on an arbitrary number of **bytes**.
///
/// - **parse**: reads `length` bytes → integer [`Value`]
/// - **build**: integer [`Value`] → writes `length` bytes
/// - **sizeof**: returns `length`
///
/// When `swapped` is `true`, the byte order is reversed (little-endian).
/// When `signed` is `true`, the value is interpreted as a two's complement
/// signed integer.
///
/// Corresponds to Python `BytesInteger(length, signed=False, swapped=False)`.
///
/// # Examples
///
/// ```
/// use construct::constructs::bytes_integer::BytesInteger;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let c: &dyn Construct = &BytesInteger::new(3, false, false);
/// let parsed = c.parse_bytes(b"\x01\x02\x03").unwrap();
/// assert_eq!(parsed, Value::BigInt(0x010203));
///
/// let built = c.build_bytes(&Value::BigInt(0x010203)).unwrap();
/// assert_eq!(built, vec![0x01, 0x02, 0x03]);
/// ```
#[derive(Clone, Copy, Debug)]
pub struct BytesInteger {
    /// Number of bytes to read/write.
    pub length: usize,
    /// Whether the value is signed (two's complement).
    pub signed: bool,
    /// Whether to swap byte order (little-endian when `true`).
    pub swapped: bool,
}

impl BytesInteger {
    /// Creates a new `BytesInteger` with the given byte width, signedness,
    /// and byte order.
    pub fn new(length: usize, signed: bool, swapped: bool) -> Self {
        BytesInteger {
            length,
            signed,
            swapped,
        }
    }
}

impl Construct for BytesInteger {
    fn parse(&self, stream: &mut CombinedStream, _ctx: &mut Context) -> Result<Value> {
        if self.length == 0 {
            return Err(ConstructError::Generic {
                path: String::new(),
                message: "length must be positive".to_string(),
            });
        }

        let data = stream.read_bytes(self.length)?;

        let data = if self.swapped {
            binary::swapbytes(&data)
        } else {
            data
        };

        let number = binary::bytes2integer(&data, self.signed)?;
        Ok(map_to_value(number, self.length, self.signed))
    }

    fn build(&self, data: &Value, stream: &mut CombinedStream, _ctx: &mut Context) -> Result<()> {
        let number = value_to_i128(data)?;

        if self.length == 0 {
            return Err(ConstructError::Generic {
                path: String::new(),
                message: "length must be positive".to_string(),
            });
        }

        let bytes = binary::integer2bytes(number, self.length, self.signed)?;

        let bytes = if self.swapped {
            binary::swapbytes(&bytes)
        } else {
            bytes
        };

        stream.write_bytes(&bytes)
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Ok(self.length)
    }
}

// ===========================================================================
// BitsInteger
// ===========================================================================

/// An integer construct that operates on an arbitrary number of **bits**.
///
/// Must be used inside a `Bitwise` context (where each "byte" in the stream
/// represents a single bit: `0x00` or `0x01`).
///
/// - **parse**: reads `length` bits → integer [`Value`]
/// - **build**: integer [`Value`] → writes `length` bits
/// - **sizeof**: returns `length`
///
/// When `swapped` is `true`, byte groups within the bit string are reordered
/// (only valid when `length` is a multiple of 8).
///
/// Corresponds to Python `BitsInteger(length, signed=False, swapped=False)`.
///
/// # Examples
///
/// ```
/// use construct::constructs::bytes_integer::BitsInteger;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// // In a bitstream, each byte is 0x00 or 0x01
/// let c: &dyn Construct = &BitsInteger::new(4, false, false);
/// let parsed = c.parse_bytes(&[0, 1, 0, 1]).unwrap();
/// assert_eq!(parsed, Value::UInt(5));
///
/// let built = c.build_bytes(&Value::UInt(5)).unwrap();
/// assert_eq!(built, vec![0, 1, 0, 1]);
/// ```
#[derive(Clone, Copy, Debug)]
pub struct BitsInteger {
    /// Number of bits to read/write.
    pub length: usize,
    /// Whether the value is signed (two's complement).
    pub signed: bool,
    /// Whether to swap byte groups in the bit string.
    pub swapped: bool,
}

impl BitsInteger {
    /// Creates a new `BitsInteger` with the given bit width, signedness,
    /// and byte-swap setting.
    pub fn new(length: usize, signed: bool, swapped: bool) -> Self {
        BitsInteger {
            length,
            signed,
            swapped,
        }
    }
}

impl Construct for BitsInteger {
    fn parse(&self, stream: &mut CombinedStream, _ctx: &mut Context) -> Result<Value> {
        if self.length == 0 {
            return Err(ConstructError::Generic {
                path: String::new(),
                message: "length must be positive".to_string(),
            });
        }

        let data = stream.read_bytes(self.length)?;

        let data = if self.swapped {
            binary::swapbytesinbits(&data)?
        } else {
            data
        };

        let number = binary::bits2integer(&data, self.signed)?;

        // Map bits length to equivalent byte length for Value selection
        let byte_len = (self.length + 7) / 8;
        // For bit widths that map to standard byte widths, use the corresponding Value variant
        let effective_byte_len = if self.length == 8 {
            1
        } else if self.length == 16 {
            2
        } else if self.length == 32 {
            4
        } else if self.length == 64 {
            8
        } else {
            byte_len
        };
        Ok(map_to_value(number, effective_byte_len, self.signed))
    }

    fn build(&self, data: &Value, stream: &mut CombinedStream, _ctx: &mut Context) -> Result<()> {
        let number = value_to_i128(data)?;

        if self.length == 0 {
            return Err(ConstructError::Generic {
                path: String::new(),
                message: "length must be positive".to_string(),
            });
        }

        let bits = binary::integer2bits(number, self.length, self.signed)?;

        let bits = if self.swapped {
            binary::swapbytesinbits(&bits)?
        } else {
            bits
        };

        stream.write_bytes(&bits)
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Ok(self.length)
    }
}

// ===========================================================================
// Convenience constants — 24-bit integers
// ===========================================================================

/// 24-bit unsigned big-endian integer.
pub const INT24UB: BytesInteger = BytesInteger {
    length: 3,
    signed: false,
    swapped: false,
};

/// 24-bit unsigned little-endian integer.
pub const INT24UL: BytesInteger = BytesInteger {
    length: 3,
    signed: false,
    swapped: true,
};

/// 24-bit signed big-endian integer.
pub const INT24SB: BytesInteger = BytesInteger {
    length: 3,
    signed: true,
    swapped: false,
};

/// 24-bit signed little-endian integer.
pub const INT24SL: BytesInteger = BytesInteger {
    length: 3,
    signed: true,
    swapped: true,
};

/// Returns a 24-bit unsigned native-endian `BytesInteger`.
///
/// Detects the target endianness at compile time.
#[must_use]
pub fn int24un() -> BytesInteger {
    BytesInteger::new(3, false, cfg!(target_endian = "little"))
}

/// Returns a 24-bit signed native-endian `BytesInteger`.
///
/// Detects the target endianness at compile time.
#[must_use]
pub fn int24sn() -> BytesInteger {
    BytesInteger::new(3, true, cfg!(target_endian = "little"))
}

// ===========================================================================
// Convenience constants — bit-level integers
// ===========================================================================

/// 1-bit unsigned integer (for use in `Bitwise` context).
pub const BIT: BitsInteger = BitsInteger {
    length: 1,
    signed: false,
    swapped: false,
};

/// 4-bit unsigned integer (for use in `Bitwise` context).
pub const NIBBLE: BitsInteger = BitsInteger {
    length: 4,
    signed: false,
    swapped: false,
};

/// 8-bit unsigned integer (for use in `Bitwise` context).
pub const OCTET: BitsInteger = BitsInteger {
    length: 8,
    signed: false,
    swapped: false,
};

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stream::ByteStream;

    // =========================================================================
    // BytesInteger tests
    // =========================================================================

    // -- parse: basic --------------------------------------------------------

    #[test]
    fn bytes_integer_parse_1byte_unsigned() {
        let c = BytesInteger::new(1, false, false);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[0xFF]));
        let mut ctx = Context::new();
        let result = c.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::UInt(255));
    }

    #[test]
    fn bytes_integer_parse_1byte_signed_negative() {
        let c = BytesInteger::new(1, true, false);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[0x80]));
        let mut ctx = Context::new();
        let result = c.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::Int(-128));
    }

    #[test]
    fn bytes_integer_parse_2byte_unsigned() {
        let c = BytesInteger::new(2, false, false);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[0x01, 0x02]));
        let mut ctx = Context::new();
        let result = c.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::UInt(0x0102));
    }

    #[test]
    fn bytes_integer_parse_4byte_unsigned() {
        let c = BytesInteger::new(4, false, false);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\x00\x01\x02\x03"));
        let mut ctx = Context::new();
        let result = c.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::UInt(0x00010203));
    }

    #[test]
    fn bytes_integer_parse_8byte_unsigned() {
        let c = BytesInteger::new(8, false, false);
        let data = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2A];
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&data));
        let mut ctx = Context::new();
        let result = c.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::UInt(42));
    }

    #[test]
    fn bytes_integer_parse_3byte_unsigned() {
        let c = BytesInteger::new(3, false, false);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[0x01, 0x02, 0x03]));
        let mut ctx = Context::new();
        let result = c.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::BigInt(0x010203));
    }

    #[test]
    fn bytes_integer_parse_3byte_signed_negative() {
        let c = BytesInteger::new(3, true, false);
        // 0xFFFFFF = -1 in 24-bit two's complement
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[0xFF, 0xFF, 0xFF]));
        let mut ctx = Context::new();
        let result = c.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::BigInt(-1));
    }

    // -- parse: swapped (little-endian) --------------------------------------

    #[test]
    fn bytes_integer_parse_2byte_swapped() {
        let c = BytesInteger::new(2, false, true);
        // 0x02 0x01 in stream → reversed to 0x01 0x02 → 258
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[0x02, 0x01]));
        let mut ctx = Context::new();
        let result = c.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::UInt(0x0102));
    }

    #[test]
    fn bytes_integer_parse_4byte_swapped() {
        let c = BytesInteger::new(4, false, true);
        // 0x03 0x02 0x01 0x00 in stream → reversed to 0x00 0x01 0x02 0x03
        let mut stream =
            CombinedStream::ByteStream(ByteStream::new_read(&[0x03, 0x02, 0x01, 0x00]));
        let mut ctx = Context::new();
        let result = c.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::UInt(0x00010203));
    }

    // -- parse: errors -------------------------------------------------------

    #[test]
    fn bytes_integer_parse_length_zero() {
        let c = BytesInteger::new(0, false, false);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[0x01]));
        let mut ctx = Context::new();
        let err = c.parse(&mut stream, &mut ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
        assert!(err.to_string().contains("length must be positive"));
    }

    #[test]
    fn bytes_integer_parse_insufficient_data() {
        let c = BytesInteger::new(4, false, false);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[0x01, 0x02]));
        let mut ctx = Context::new();
        let err = c.parse(&mut stream, &mut ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Stream { .. }));
    }

    // -- build: basic --------------------------------------------------------

    #[test]
    fn bytes_integer_build_1byte_unsigned() {
        let c = BytesInteger::new(1, false, false);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        c.build(&Value::UInt(255), &mut stream, &mut ctx).unwrap();
        assert_eq!(stream.into_bytes(), vec![0xFF]);
    }

    #[test]
    fn bytes_integer_build_1byte_signed_negative() {
        let c = BytesInteger::new(1, true, false);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        c.build(&Value::Int(-1), &mut stream, &mut ctx).unwrap();
        assert_eq!(stream.into_bytes(), vec![0xFF]);
    }

    #[test]
    fn bytes_integer_build_4byte_unsigned() {
        let c = BytesInteger::new(4, false, false);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        c.build(&Value::UInt(0x01020304), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), vec![0x01, 0x02, 0x03, 0x04]);
    }

    #[test]
    fn bytes_integer_build_3byte_unsigned() {
        let c = BytesInteger::new(3, false, false);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        c.build(&Value::BigInt(0x010203), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), vec![0x01, 0x02, 0x03]);
    }

    // -- build: swapped (little-endian) --------------------------------------

    #[test]
    fn bytes_integer_build_4byte_swapped() {
        let c = BytesInteger::new(4, false, true);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        c.build(&Value::UInt(0x01020304), &mut stream, &mut ctx)
            .unwrap();
        // Swapped: bytes are reversed
        assert_eq!(stream.into_bytes(), vec![0x04, 0x03, 0x02, 0x01]);
    }

    // -- build: errors -------------------------------------------------------

    #[test]
    fn bytes_integer_build_length_zero() {
        let c = BytesInteger::new(0, false, false);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        let err = c.build(&Value::UInt(1), &mut stream, &mut ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
        assert!(err.to_string().contains("length must be positive"));
    }

    #[test]
    fn bytes_integer_build_wrong_type() {
        let c = BytesInteger::new(4, false, false);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        let err = c
            .build(&Value::String("not int".to_string()), &mut stream, &mut ctx)
            .unwrap_err();
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
    }

    #[test]
    fn bytes_integer_build_overflow() {
        let c = BytesInteger::new(1, false, false);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        let err = c
            .build(&Value::UInt(256), &mut stream, &mut ctx)
            .unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    #[test]
    fn bytes_integer_build_negative_unsigned() {
        let c = BytesInteger::new(1, false, false);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        let err = c.build(&Value::Int(-1), &mut stream, &mut ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    // -- sizeof --------------------------------------------------------------

    #[test]
    fn bytes_integer_sizeof() {
        let c = BytesInteger::new(3, false, false);
        assert_eq!(c.sizeof(&Context::new()).unwrap(), 3);
    }

    #[test]
    fn bytes_integer_sizeof_8() {
        let c = BytesInteger::new(8, true, true);
        assert_eq!(c.sizeof(&Context::new()).unwrap(), 8);
    }

    // -- roundtrip -----------------------------------------------------------

    #[test]
    fn bytes_integer_roundtrip_1byte_unsigned() {
        let c: &dyn Construct = &BytesInteger::new(1, false, false);
        let original = Value::UInt(200);
        let built = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn bytes_integer_roundtrip_2byte_signed() {
        let c: &dyn Construct = &BytesInteger::new(2, true, false);
        let original = Value::Int(-1000);
        let built = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn bytes_integer_roundtrip_4byte_unsigned_swapped() {
        let c: &dyn Construct = &BytesInteger::new(4, false, true);
        let original = Value::UInt(0xDEADBEEF);
        let built = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn bytes_integer_roundtrip_8byte_signed() {
        let c: &dyn Construct = &BytesInteger::new(8, true, false);
        let original = Value::Int(-123456789);
        let built = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn bytes_integer_roundtrip_3byte_bigint() {
        let c: &dyn Construct = &BytesInteger::new(3, false, false);
        let original = Value::BigInt(0x808080);
        let built = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    // -- convenience methods (dyn Construct) ---------------------------------

    #[test]
    fn bytes_integer_parse_bytes_convenience() {
        let c: &dyn Construct = &BytesInteger::new(4, false, false);
        let result = c.parse_bytes(b"\x00\x00\x01\x00").unwrap();
        assert_eq!(result, Value::UInt(256));
    }

    #[test]
    fn bytes_integer_build_bytes_convenience() {
        let c: &dyn Construct = &BytesInteger::new(4, false, false);
        let bytes = c.build_bytes(&Value::UInt(256)).unwrap();
        assert_eq!(bytes, vec![0x00, 0x00, 0x01, 0x00]);
    }

    // -- build from different integer Value types ----------------------------

    #[test]
    fn bytes_integer_build_from_int() {
        let c: &dyn Construct = &BytesInteger::new(4, true, false);
        let bytes = c.build_bytes(&Value::Int(-1)).unwrap();
        assert_eq!(bytes, vec![0xFF, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn bytes_integer_build_from_bigint() {
        let c: &dyn Construct = &BytesInteger::new(3, false, false);
        let bytes = c.build_bytes(&Value::BigInt(0xABCDEF)).unwrap();
        assert_eq!(bytes, vec![0xAB, 0xCD, 0xEF]);
    }

    // =========================================================================
    // BitsInteger tests
    // =========================================================================

    // -- parse: basic --------------------------------------------------------

    #[test]
    fn bits_integer_parse_1bit() {
        let c = BitsInteger::new(1, false, false);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[0x01]));
        let mut ctx = Context::new();
        let result = c.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::UInt(1));
    }

    #[test]
    fn bits_integer_parse_4bit() {
        let c = BitsInteger::new(4, false, false);
        // 0b0101 = 5
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[0, 1, 0, 1]));
        let mut ctx = Context::new();
        let result = c.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::UInt(5));
    }

    #[test]
    fn bits_integer_parse_8bit() {
        let c = BitsInteger::new(8, false, false);
        // 0b00010011 = 19
        let mut stream =
            CombinedStream::ByteStream(ByteStream::new_read(&[0, 0, 0, 1, 0, 0, 1, 1]));
        let mut ctx = Context::new();
        let result = c.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::UInt(19));
    }

    #[test]
    fn bits_integer_parse_signed_negative() {
        let c = BitsInteger::new(4, true, false);
        // 0b1111 = -1 (4-bit two's complement)
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[1, 1, 1, 1]));
        let mut ctx = Context::new();
        let result = c.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::Int(-1));
    }

    #[test]
    fn bits_integer_parse_signed_min() {
        let c = BitsInteger::new(4, true, false);
        // 0b1000 = -8 (4-bit two's complement minimum)
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[1, 0, 0, 0]));
        let mut ctx = Context::new();
        let result = c.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::Int(-8));
    }

    // -- parse: swapped ------------------------------------------------------

    #[test]
    fn bits_integer_parse_16bit_swapped() {
        // 16-bit swapped: swapbytesinbits reverses the two byte groups
        // Group1: bits 0-7, Group2: bits 8-15
        // After swap: Group2 first, Group1 second
        let c = BitsInteger::new(16, false, true);
        // Before swap: 00000000 11111111 → after swap: 11111111 00000000
        let input: Vec<u8> = vec![0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1];
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&input));
        let mut ctx = Context::new();
        let result = c.parse(&mut stream, &mut ctx).unwrap();
        // After swap: 11111111 00000000 = 0xFF00 = 65280
        assert_eq!(result, Value::UInt(0xFF00));
    }

    // -- parse: errors -------------------------------------------------------

    #[test]
    fn bits_integer_parse_length_zero() {
        let c = BitsInteger::new(0, false, false);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[0x01]));
        let mut ctx = Context::new();
        let err = c.parse(&mut stream, &mut ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
        assert!(err.to_string().contains("length must be positive"));
    }

    #[test]
    fn bits_integer_parse_swapped_non_multiple_of_8() {
        // BitsInteger with swapped and length not a multiple of 8
        let c = BitsInteger::new(4, false, true);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[0, 1, 0, 1]));
        let mut ctx = Context::new();
        let err = c.parse(&mut stream, &mut ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
        assert!(err.to_string().contains("multiple of 8"));
    }

    // -- build: basic --------------------------------------------------------

    #[test]
    fn bits_integer_build_1bit() {
        let c = BitsInteger::new(1, false, false);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        c.build(&Value::UInt(1), &mut stream, &mut ctx).unwrap();
        assert_eq!(stream.into_bytes(), vec![1]);
    }

    #[test]
    fn bits_integer_build_4bit() {
        let c = BitsInteger::new(4, false, false);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        c.build(&Value::UInt(5), &mut stream, &mut ctx).unwrap();
        assert_eq!(stream.into_bytes(), vec![0, 1, 0, 1]);
    }

    #[test]
    fn bits_integer_build_8bit() {
        let c = BitsInteger::new(8, false, false);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        c.build(&Value::UInt(19), &mut stream, &mut ctx).unwrap();
        assert_eq!(stream.into_bytes(), vec![0, 0, 0, 1, 0, 0, 1, 1]);
    }

    #[test]
    fn bits_integer_build_signed_negative() {
        let c = BitsInteger::new(4, true, false);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        c.build(&Value::Int(-1), &mut stream, &mut ctx).unwrap();
        assert_eq!(stream.into_bytes(), vec![1, 1, 1, 1]);
    }

    // -- build: swapped ------------------------------------------------------

    #[test]
    fn bits_integer_build_16bit_swapped() {
        let c = BitsInteger::new(16, false, true);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        c.build(&Value::UInt(0xFF00), &mut stream, &mut ctx)
            .unwrap();
        // 0xFF00 in bits: 11111111 00000000
        // After swapbytesinbits: 00000000 11111111
        let expected: Vec<u8> = vec![0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1];
        assert_eq!(stream.into_bytes(), expected);
    }

    // -- build: errors -------------------------------------------------------

    #[test]
    fn bits_integer_build_length_zero() {
        let c = BitsInteger::new(0, false, false);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        let err = c.build(&Value::UInt(1), &mut stream, &mut ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    #[test]
    fn bits_integer_build_wrong_type() {
        let c = BitsInteger::new(4, false, false);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        let err = c
            .build(&Value::Bool(true), &mut stream, &mut ctx)
            .unwrap_err();
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
    }

    #[test]
    fn bits_integer_build_overflow() {
        let c = BitsInteger::new(4, false, false);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        // Max 4-bit unsigned is 15, 16 overflows
        let err = c
            .build(&Value::UInt(16), &mut stream, &mut ctx)
            .unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    #[test]
    fn bits_integer_build_swapped_non_multiple_of_8() {
        let c = BitsInteger::new(4, false, true);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        let err = c.build(&Value::UInt(5), &mut stream, &mut ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    // -- sizeof --------------------------------------------------------------

    #[test]
    fn bits_integer_sizeof() {
        let c = BitsInteger::new(4, false, false);
        assert_eq!(c.sizeof(&Context::new()).unwrap(), 4);
    }

    #[test]
    fn bits_integer_sizeof_16() {
        let c = BitsInteger::new(16, false, true);
        assert_eq!(c.sizeof(&Context::new()).unwrap(), 16);
    }

    // -- roundtrip -----------------------------------------------------------

    #[test]
    fn bits_integer_roundtrip_4bit() {
        let c: &dyn Construct = &BitsInteger::new(4, false, false);
        let original = Value::UInt(13);
        let built = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn bits_integer_roundtrip_8bit() {
        let c: &dyn Construct = &BitsInteger::new(8, false, false);
        let original = Value::UInt(19);
        let built = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn bits_integer_roundtrip_signed() {
        let c: &dyn Construct = &BitsInteger::new(4, true, false);
        let original = Value::Int(-5);
        let built = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn bits_integer_roundtrip_16bit_swapped() {
        let c: &dyn Construct = &BitsInteger::new(16, false, true);
        let original = Value::UInt(0xABCD);
        let built = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    // =========================================================================
    // Convenience constants tests
    // =========================================================================

    #[test]
    fn int24ub_parse() {
        let c: &dyn Construct = &INT24UB;
        let result = c.parse_bytes(&[0x01, 0x02, 0x03]).unwrap();
        assert_eq!(result, Value::BigInt(0x010203));
    }

    #[test]
    fn int24ub_roundtrip() {
        let c: &dyn Construct = &INT24UB;
        let original = Value::BigInt(0xABCDEF);
        let built = c.build_bytes(&original).unwrap();
        assert_eq!(built, vec![0xAB, 0xCD, 0xEF]);
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn int24ul_parse() {
        let c: &dyn Construct = &INT24UL;
        // Little-endian: 0x03 0x02 0x01 → reversed → 0x01 0x02 0x03 = 66051
        let result = c.parse_bytes(&[0x03, 0x02, 0x01]).unwrap();
        assert_eq!(result, Value::BigInt(0x010203));
    }

    #[test]
    fn int24sb_parse_negative() {
        let c: &dyn Construct = &INT24SB;
        let result = c.parse_bytes(&[0xFF, 0xFF, 0xFF]).unwrap();
        assert_eq!(result, Value::BigInt(-1));
    }

    #[test]
    fn int24sl_parse() {
        let c: &dyn Construct = &INT24SL;
        let result = c.parse_bytes(&[0xFF, 0xFF, 0xFF]).unwrap();
        assert_eq!(result, Value::BigInt(-1));
    }

    #[test]
    fn int24un_returns_native_endian() {
        let c = int24un();
        assert_eq!(c.length, 3);
        assert!(!c.signed);
    }

    #[test]
    fn int24sn_returns_native_endian() {
        let c = int24sn();
        assert_eq!(c.length, 3);
        assert!(c.signed);
    }

    // -- BIT, NIBBLE, OCTET --------------------------------------------------

    #[test]
    fn bit_constant() {
        let c: &dyn Construct = &BIT;
        let parsed = c.parse_bytes(&[1]).unwrap();
        assert_eq!(parsed, Value::UInt(1));

        let parsed = c.parse_bytes(&[0]).unwrap();
        assert_eq!(parsed, Value::UInt(0));
    }

    #[test]
    fn nibble_constant() {
        let c: &dyn Construct = &NIBBLE;
        let parsed = c.parse_bytes(&[0, 1, 1, 1]).unwrap();
        assert_eq!(parsed, Value::UInt(7));
    }

    #[test]
    fn octet_constant() {
        let c: &dyn Construct = &OCTET;
        let bits: Vec<u8> = vec![1, 0, 1, 0, 1, 0, 1, 0];
        let parsed = c.parse_bytes(&bits).unwrap();
        assert_eq!(parsed, Value::UInt(0b10101010));
    }

    #[test]
    fn bit_roundtrip() {
        let c: &dyn Construct = &BIT;
        let original = Value::UInt(1);
        let built = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn nibble_roundtrip() {
        let c: &dyn Construct = &NIBBLE;
        let original = Value::UInt(15);
        let built = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn octet_roundtrip() {
        let c: &dyn Construct = &OCTET;
        let original = Value::UInt(255);
        let built = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }
}
