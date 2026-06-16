//! FormatField construct — fixed-width numeric fields with endianness.
//!
//! Corresponds to the Python `FormatField(endianity, format)` class
//! (`construct/construct/core.py` line ~1124).
//!
//! Supports all format characters except `?` (boolean, handled by `Flag`):
//!
//! | FormatKind | Python | Bytes | Signed |
//! |-----------|--------|-------|--------|
//! | `I8`      | `b`    | 1     | yes    |
//! | `U8`      | `B`    | 1     | no     |
//! | `I16`     | `h`    | 2     | yes    |
//! | `U16`     | `H`    | 2     | no     |
//! | `I32`     | `i`/`l`| 4     | yes    |
//! | `U32`     | `I`/`L`| 4     | no     |
//! | `I64`     | `q`    | 8     | yes    |
//! | `U64`     | `Q`    | 8     | no     |
//! | `F16`     | `e`    | 2     | —      |
//! | `F32`     | `f`    | 4     | —      |
//! | `F64`     | `d`    | 8     | —      |

use crate::core::context::Context;
use crate::core::error::{ConstructError, Result};
use crate::core::stream::CombinedStream;
use crate::core::stream::Stream;
use crate::core::Construct;
use crate::value::Value;

// ===========================================================================
// Endianness
// ===========================================================================

/// Byte order used by [`FormatField`].
///
/// Corresponds to the Python `endianity` parameter (`<`, `>`, `=`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endianness {
    /// Big-endian (Python `>`).
    Big,
    /// Little-endian (Python `<`).
    Little,
    /// Native endian (Python `=`).
    ///
    /// Resolved at runtime to [`Big`](Endianness::Big) or
    /// [`Little`](Endianness::Little) depending on `#[cfg(target_endian)]`.
    Native,
}

impl Endianness {
    /// Returns `true` if this endianness resolves to little-endian on the
    /// current target.
    fn is_little(self) -> bool {
        match self {
            Endianness::Little => true,
            Endianness::Big => false,
            Endianness::Native => cfg!(target_endian = "little"),
        }
    }
}

// ===========================================================================
// FormatKind
// ===========================================================================

/// Numeric format kind used by [`FormatField`].
///
/// Each variant maps to a Python `struct` format character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatKind {
    /// `b` — signed 8-bit integer.
    I8,
    /// `B` — unsigned 8-bit integer.
    U8,
    /// `?` — boolean (1 byte, nonzero = true).
    Bool,
    /// `h` — signed 16-bit integer.
    I16,
    /// `H` — unsigned 16-bit integer.
    U16,
    /// `i`/`l` — signed 32-bit integer.
    I32,
    /// `I`/`L` — unsigned 32-bit integer.
    U32,
    /// `q` — signed 64-bit integer.
    I64,
    /// `Q` — unsigned 64-bit integer.
    U64,
    /// `e` — IEEE 754 half-precision (16-bit) float.
    F16,
    /// `f` — IEEE 754 single-precision (32-bit) float.
    F32,
    /// `d` — IEEE 754 double-precision (64-bit) float.
    F64,
}

impl FormatKind {
    /// Returns the number of bytes this format occupies.
    const fn byte_size(self) -> usize {
        match self {
            FormatKind::I8 | FormatKind::U8 | FormatKind::Bool => 1,
            FormatKind::I16 | FormatKind::U16 | FormatKind::F16 => 2,
            FormatKind::I32 | FormatKind::U32 | FormatKind::F32 => 4,
            FormatKind::I64 | FormatKind::U64 | FormatKind::F64 => 8,
        }
    }
}

// ===========================================================================
// FormatField
// ===========================================================================

/// Fixed-width numeric field construct with configurable endianness.
///
/// Corresponds to Python `FormatField(endianity, format)`.
///
/// # Parse
///
/// Reads [`byte_size`](FormatField::byte_size) bytes from the stream,
/// interprets them according to `endian` + `kind`, and returns the
/// appropriate [`Value`] variant.
///
/// # Build
///
/// Extracts a numeric value from `data`, encodes it according to `endian` +
/// `kind`, and writes the resulting bytes to the stream.
///
/// # Sizeof
///
/// Always returns [`byte_size`](FormatField::byte_size).
///
/// # Example
///
/// ```ignore
/// use construct::constructs::format_field::INT32UB;
/// let val = INT32UB.parse_bytes(b"\x00\x00\x01\x00").unwrap();
/// assert_eq!(val, Value::UInt(256));
/// ```
#[derive(Clone, Copy, Debug)]
pub struct FormatField {
    /// Byte order.
    pub endian: Endianness,
    /// Numeric format (int width + signedness, or float width).
    pub kind: FormatKind,
}

impl FormatField {
    /// Creates a new `FormatField` with the given endianness and format kind.
    pub const fn new(endian: Endianness, kind: FormatKind) -> Self {
        FormatField { endian, kind }
    }

    /// Returns the number of bytes this field occupies.
    pub const fn byte_size(&self) -> usize {
        self.kind.byte_size()
    }
}

impl Construct for FormatField {
    fn parse(&self, stream: &mut CombinedStream, _ctx: &mut Context) -> Result<Value> {
        let n = self.byte_size();
        let data = stream.read_bytes(n)?;

        // Determine if we need to swap bytes for native endian.
        let bytes = if self.endian.is_little() {
            let mut v = data;
            v.reverse();
            v
        } else {
            data
        };

        // `bytes` is now big-endian; decode.
        let value = match self.kind {
            FormatKind::I8 => Value::Int(i8::from_be_bytes([bytes[0]]) as i64),
            FormatKind::U8 => Value::UInt(u8::from_be_bytes([bytes[0]]) as u64),
            FormatKind::Bool => Value::Bool(bytes[0] != 0),
            FormatKind::I16 => Value::Int(i16::from_be_bytes([bytes[0], bytes[1]]) as i64),
            FormatKind::U16 => Value::UInt(u16::from_be_bytes([bytes[0], bytes[1]]) as u64),
            FormatKind::I32 => {
                Value::Int(i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as i64)
            }
            FormatKind::U32 => {
                Value::UInt(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as u64)
            }
            FormatKind::I64 => Value::Int(i64::from_be_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ])),
            FormatKind::U64 => Value::UInt(u64::from_be_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ])),
            FormatKind::F16 => {
                let f = f16_to_f32(u16::from_be_bytes([bytes[0], bytes[1]]));
                Value::Float(f64::from(f))
            }
            FormatKind::F32 => {
                let f = f32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                Value::Float(f64::from(f))
            }
            FormatKind::F64 => {
                let f = f64::from_be_bytes([
                    bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
                ]);
                Value::Float(f)
            }
        };

        Ok(value)
    }

    fn build(&self, data: &Value, stream: &mut CombinedStream, _ctx: &mut Context) -> Result<()> {
        let bytes = match self.kind {
            FormatKind::I8 => {
                let v = data.to_i64()?;
                if v < (i8::MIN as i64) || v > (i8::MAX as i64) {
                    return Err(ConstructError::FormatField {
                        path: String::new(),
                        expected: self.byte_size(),
                        actual: 0, // Using actual=0 to signal value out of range
                    });
                }
                (v as i8).to_be_bytes().to_vec()
            }
            FormatKind::U8 => {
                let v = data.to_u64()?;
                if v > (u8::MAX as u64) {
                    return Err(ConstructError::FormatField {
                        path: String::new(),
                        expected: self.byte_size(),
                        actual: 0,
                    });
                }
                (v as u8).to_be_bytes().to_vec()
            }
            FormatKind::Bool => {
                vec![if data.as_bool().unwrap_or(false) {
                    1
                } else {
                    0
                }]
            }
            FormatKind::I16 => {
                let v = data.to_i64()?;
                if v < (i16::MIN as i64) || v > (i16::MAX as i64) {
                    return Err(ConstructError::FormatField {
                        path: String::new(),
                        expected: self.byte_size(),
                        actual: 0,
                    });
                }
                (v as i16).to_be_bytes().to_vec()
            }
            FormatKind::U16 => {
                let v = data.to_u64()?;
                if v > (u16::MAX as u64) {
                    return Err(ConstructError::FormatField {
                        path: String::new(),
                        expected: self.byte_size(),
                        actual: 0,
                    });
                }
                (v as u16).to_be_bytes().to_vec()
            }
            FormatKind::I32 => {
                let v = data.to_i64()?;
                if v < (i32::MIN as i64) || v > (i32::MAX as i64) {
                    return Err(ConstructError::FormatField {
                        path: String::new(),
                        expected: self.byte_size(),
                        actual: 0,
                    });
                }
                (v as i32).to_be_bytes().to_vec()
            }
            FormatKind::U32 => {
                let v = data.to_u64()?;
                if v > (u32::MAX as u64) {
                    return Err(ConstructError::FormatField {
                        path: String::new(),
                        expected: self.byte_size(),
                        actual: 0,
                    });
                }
                (v as u32).to_be_bytes().to_vec()
            }
            FormatKind::I64 => {
                let v = data.to_i64()?;
                v.to_be_bytes().to_vec()
            }
            FormatKind::U64 => {
                let v = data.to_u64()?;
                v.to_be_bytes().to_vec()
            }
            FormatKind::F16 => {
                let v = data.to_f64()?;
                let bits = f32_to_f16(v as f32);
                bits.to_be_bytes().to_vec()
            }
            FormatKind::F32 => {
                let v = data.to_f64()?;
                (v as f32).to_be_bytes().to_vec()
            }
            FormatKind::F64 => {
                let v = data.to_f64()?;
                v.to_be_bytes().to_vec()
            }
        };

        // Reverse bytes for little-endian.
        let bytes = if self.endian.is_little() {
            let mut v = bytes;
            v.reverse();
            v
        } else {
            bytes
        };

        stream.write_bytes(&bytes)?;
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Ok(self.byte_size())
    }
}

// ===========================================================================
// f16 ↔ f32 conversion (no external crate)
// ===========================================================================

/// Number of bits in the exponent of an IEEE 754 half-precision float.
const F16_EXP_BITS: u32 = 5;
/// Number of bits in the mantissa of an IEEE 754 half-precision float.
const F16_MANT_BITS: u32 = 10;
/// Bias of the exponent in an IEEE 754 half-precision float.
const F16_BIAS: i32 = 15;
/// Bias of the exponent in an IEEE 754 single-precision float.
const F32_BIAS: i32 = 127;
/// Number of mantissa bits in an IEEE 754 single-precision float.
const F32_MANT_BITS: u32 = 23;

/// Converts an IEEE 754 half-precision `u16` bit pattern to an `f32`.
///
/// Handles zero, denormals, infinities, NaNs, and normal numbers.
fn f16_to_f32(bits: u16) -> f32 {
    let sign = (bits >> 15) & 1;
    let exp = ((bits >> F16_MANT_BITS) & ((1 << F16_EXP_BITS) - 1)) as i32;
    let mant = (bits & ((1 << F16_MANT_BITS) - 1)) as u32;

    let f32_sign = (sign as u32) << 31;

    if exp == 0 {
        // Zero or denormal
        if mant == 0 {
            // ±zero
            return f32::from_bits(f32_sign);
        }
        // Denormal: normalize it
        // f16 denormal = (-1)^s * 2^(1-bias) * (mant / 2^10)
        // f32 normal   = (-1)^s * 2^(e-127) * (1 + mant_f/2^23)
        // We need to find the leading 1 in mant and adjust.
        let mut m = mant;
        let mut shift = 0;
        while (m & (1 << F16_MANT_BITS)) == 0 {
            m <<= 1;
            shift += 1;
        }
        // Remove the implicit leading 1
        m &= (1 << F16_MANT_BITS) - 1;
        let f32_exp = (F32_BIAS - F16_BIAS + 1 - shift) as u32;
        let f32_mant = m << (F32_MANT_BITS - F16_MANT_BITS);
        return f32::from_bits(f32_sign | (f32_exp << F32_MANT_BITS) | f32_mant);
    }

    if exp == (1 << F16_EXP_BITS) - 1 {
        // Infinity or NaN
        let f32_mant = if mant != 0 {
            // NaN: set the MSB of f32 mantissa to ensure it's a quiet NaN
            (mant << (F32_MANT_BITS - F16_MANT_BITS)) | (1 << (F32_MANT_BITS - 1))
        } else {
            0 // Infinity
        };
        let f32_exp = 0xFF_u32;
        return f32::from_bits(f32_sign | (f32_exp << F32_MANT_BITS) | f32_mant);
    }

    // Normal number
    let f32_exp = (exp - F16_BIAS + F32_BIAS) as u32;
    let f32_mant = mant << (F32_MANT_BITS - F16_MANT_BITS);
    f32::from_bits(f32_sign | (f32_exp << F32_MANT_BITS) | f32_mant)
}

/// Converts an `f32` to an IEEE 754 half-precision `u16` bit pattern.
///
/// Uses round-to-zero semantics (truncation). Overflow values become infinity.
fn f32_to_f16(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = (bits >> 31) & 1;
    let exp = ((bits >> F32_MANT_BITS) & 0xFF) as i32;
    let mant = bits & ((1 << F32_MANT_BITS) - 1);

    let u16_sign = (sign as u16) << 15;

    if exp == 0 {
        // ±zero
        return u16_sign;
    }

    if exp == 0xFF {
        // Infinity or NaN
        let u16_mant = if mant != 0 {
            // Preserve NaN-ness: map non-zero mant to non-zero f16 mant
            let shifted = mant >> (F32_MANT_BITS - F16_MANT_BITS);
            if shifted == 0 {
                1
            } else {
                shifted as u16
            }
        } else {
            0 // Infinity
        };
        return u16_sign | (((1 << F16_EXP_BITS) - 1) << F16_MANT_BITS) | u16_mant;
    }

    let f16_exp = exp - F32_BIAS + F16_BIAS;

    if f16_exp <= 0 {
        // Underflow to zero (we could produce denormals, but for simplicity
        // and matching Python struct behavior, produce zero)
        if f16_exp < -(F16_MANT_BITS as i32) {
            // Way too small → zero
            return u16_sign;
        }
        // Produce a denormal
        let shift = (1 - f16_exp) as u32;
        // The implicit 1 bit becomes part of the mantissa
        let full_mant = (1 << F32_MANT_BITS) | mant;
        let shifted_mant = full_mant >> shift;
        let u16_mant = (shifted_mant >> (F32_MANT_BITS - F16_MANT_BITS)) as u16;
        return u16_sign | u16_mant;
    }

    if f16_exp >= (1 << F16_EXP_BITS) - 1 {
        // Overflow → infinity
        return u16_sign | (((1 << F16_EXP_BITS) - 1) << F16_MANT_BITS);
    }

    // Normal number: drop excess mantissa bits (round-to-zero)
    let u16_mant = (mant >> (F32_MANT_BITS - F16_MANT_BITS)) as u16;
    u16_sign | ((f16_exp as u16) << F16_MANT_BITS) | u16_mant
}

// ===========================================================================
// Convenience constants
// ===========================================================================

/// Unsigned 8-bit big-endian (Python `Int8ub`).
pub const INT8UB: FormatField = FormatField::new(Endianness::Big, FormatKind::U8);
/// Unsigned 16-bit big-endian (Python `Int16ub`).
pub const INT16UB: FormatField = FormatField::new(Endianness::Big, FormatKind::U16);
/// Unsigned 32-bit big-endian (Python `Int32ub`).
pub const INT32UB: FormatField = FormatField::new(Endianness::Big, FormatKind::U32);
/// Unsigned 64-bit big-endian (Python `Int64ub`).
pub const INT64UB: FormatField = FormatField::new(Endianness::Big, FormatKind::U64);

/// Signed 8-bit big-endian (Python `Int8sb`).
pub const INT8SB: FormatField = FormatField::new(Endianness::Big, FormatKind::I8);
/// Signed 16-bit big-endian (Python `Int16sb`).
pub const INT16SB: FormatField = FormatField::new(Endianness::Big, FormatKind::I16);
/// Signed 32-bit big-endian (Python `Int32sb`).
pub const INT32SB: FormatField = FormatField::new(Endianness::Big, FormatKind::I32);
/// Signed 64-bit big-endian (Python `Int64sb`).
pub const INT64SB: FormatField = FormatField::new(Endianness::Big, FormatKind::I64);

/// Unsigned 8-bit little-endian (Python `Int8ul`).
pub const INT8UL: FormatField = FormatField::new(Endianness::Little, FormatKind::U8);
/// Unsigned 16-bit little-endian (Python `Int16ul`).
pub const INT16UL: FormatField = FormatField::new(Endianness::Little, FormatKind::U16);
/// Unsigned 32-bit little-endian (Python `Int32ul`).
pub const INT32UL: FormatField = FormatField::new(Endianness::Little, FormatKind::U32);
/// Unsigned 64-bit little-endian (Python `Int64ul`).
pub const INT64UL: FormatField = FormatField::new(Endianness::Little, FormatKind::U64);

/// Signed 8-bit little-endian (Python `Int8sl`).
pub const INT8SL: FormatField = FormatField::new(Endianness::Little, FormatKind::I8);
/// Signed 16-bit little-endian (Python `Int16sl`).
pub const INT16SL: FormatField = FormatField::new(Endianness::Little, FormatKind::I16);
/// Signed 32-bit little-endian (Python `Int32sl`).
pub const INT32SL: FormatField = FormatField::new(Endianness::Little, FormatKind::I32);
/// Signed 64-bit little-endian (Python `Int64sl`).
pub const INT64SL: FormatField = FormatField::new(Endianness::Little, FormatKind::I64);

/// Unsigned 8-bit native-endian (Python `Int8un`).
pub const INT8UN: FormatField = FormatField::new(Endianness::Native, FormatKind::U8);
/// Unsigned 16-bit native-endian (Python `Int16un`).
pub const INT16UN: FormatField = FormatField::new(Endianness::Native, FormatKind::U16);
/// Unsigned 32-bit native-endian (Python `Int32un`).
pub const INT32UN: FormatField = FormatField::new(Endianness::Native, FormatKind::U32);
/// Unsigned 64-bit native-endian (Python `Int64un`).
pub const INT64UN: FormatField = FormatField::new(Endianness::Native, FormatKind::U64);

/// Signed 8-bit native-endian (Python `Int8sn`).
pub const INT8SN: FormatField = FormatField::new(Endianness::Native, FormatKind::I8);
/// Signed 16-bit native-endian (Python `Int16sn`).
pub const INT16SN: FormatField = FormatField::new(Endianness::Native, FormatKind::I16);
/// Signed 32-bit native-endian (Python `Int32sn`).
pub const INT32SN: FormatField = FormatField::new(Endianness::Native, FormatKind::I32);
/// Signed 64-bit native-endian (Python `Int64sn`).
pub const INT64SN: FormatField = FormatField::new(Endianness::Native, FormatKind::I64);

/// Half-precision (16-bit) float, big-endian (Python `Float16b`).
pub const FLOAT16B: FormatField = FormatField::new(Endianness::Big, FormatKind::F16);
/// Single-precision (32-bit) float, big-endian (Python `Float32b`).
pub const FLOAT32B: FormatField = FormatField::new(Endianness::Big, FormatKind::F32);
/// Double-precision (64-bit) float, big-endian (Python `Float64b`).
pub const FLOAT64B: FormatField = FormatField::new(Endianness::Big, FormatKind::F64);

/// Half-precision (16-bit) float, little-endian (Python `Float16l`).
pub const FLOAT16L: FormatField = FormatField::new(Endianness::Little, FormatKind::F16);
/// Single-precision (32-bit) float, little-endian (Python `Float32l`).
pub const FLOAT32L: FormatField = FormatField::new(Endianness::Little, FormatKind::F32);
/// Double-precision (64-bit) float, little-endian (Python `Float64l`).
pub const FLOAT64L: FormatField = FormatField::new(Endianness::Little, FormatKind::F64);

/// Half-precision (16-bit) float, native-endian (Python `Float16n`).
pub const FLOAT16N: FormatField = FormatField::new(Endianness::Native, FormatKind::F16);
/// Single-precision (32-bit) float, native-endian (Python `Float32n`).
pub const FLOAT32N: FormatField = FormatField::new(Endianness::Native, FormatKind::F32);
/// Double-precision (64-bit) float, native-endian (Python `Float64n`).
pub const FLOAT64N: FormatField = FormatField::new(Endianness::Native, FormatKind::F64);

// -- Python aliases ---------------------------------------------------------

/// Alias for `Int8ub` (Python `Byte`).
pub const BYTE: FormatField = INT8UB;
/// Alias for `Int16ub` (Python `Short`).
pub const SHORT: FormatField = INT16UB;
/// Alias for `Int32ub` (Python `Int`).
pub const INT: FormatField = INT32UB;
/// Alias for `Int64ub` (Python `Long`).
pub const LONG: FormatField = INT64UB;
/// Alias for `Float16b` (Python `Half`).
pub const HALF: FormatField = FLOAT16B;
/// Alias for `Float32b` (Python `Single`).
pub const SINGLE: FormatField = FLOAT32B;
/// Alias for `Float64b` (Python `Double`).
pub const DOUBLE: FormatField = FLOAT64B;

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stream::ByteStream;

    // ======================================================================
    // Integer parse tests — big-endian
    // ======================================================================

    #[test]
    fn parse_int8ub_zero() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\x00"));
        let mut ctx = Context::new();
        assert_eq!(INT8UB.parse(&mut stream, &mut ctx).unwrap(), Value::UInt(0));
    }

    #[test]
    fn parse_int8ub_max() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\xFF"));
        let mut ctx = Context::new();
        assert_eq!(
            INT8UB.parse(&mut stream, &mut ctx).unwrap(),
            Value::UInt(255)
        );
    }

    #[test]
    fn parse_int8sb_negative() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\xFF"));
        let mut ctx = Context::new();
        assert_eq!(INT8SB.parse(&mut stream, &mut ctx).unwrap(), Value::Int(-1));
    }

    #[test]
    fn parse_int8sb_min() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\x80"));
        let mut ctx = Context::new();
        assert_eq!(
            INT8SB.parse(&mut stream, &mut ctx).unwrap(),
            Value::Int(-128)
        );
    }

    #[test]
    fn parse_int16ub() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\x01\x00"));
        let mut ctx = Context::new();
        assert_eq!(
            INT16UB.parse(&mut stream, &mut ctx).unwrap(),
            Value::UInt(256)
        );
    }

    #[test]
    fn parse_int16sb_negative() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\xFF\xFF"));
        let mut ctx = Context::new();
        assert_eq!(
            INT16SB.parse(&mut stream, &mut ctx).unwrap(),
            Value::Int(-1)
        );
    }

    #[test]
    fn parse_int32ub() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\x00\x00\x01\x00"));
        let mut ctx = Context::new();
        assert_eq!(
            INT32UB.parse(&mut stream, &mut ctx).unwrap(),
            Value::UInt(256)
        );
    }

    #[test]
    fn parse_int32sb_negative() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\xFF\xFF\xFF\xFF"));
        let mut ctx = Context::new();
        assert_eq!(
            INT32SB.parse(&mut stream, &mut ctx).unwrap(),
            Value::Int(-1)
        );
    }

    #[test]
    fn parse_int64ub() {
        let mut stream =
            CombinedStream::ByteStream(ByteStream::new_read(b"\x00\x00\x00\x00\x00\x00\x01\x00"));
        let mut ctx = Context::new();
        assert_eq!(
            INT64UB.parse(&mut stream, &mut ctx).unwrap(),
            Value::UInt(256)
        );
    }

    #[test]
    fn parse_int64sb_negative() {
        let mut stream =
            CombinedStream::ByteStream(ByteStream::new_read(b"\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF"));
        let mut ctx = Context::new();
        assert_eq!(
            INT64SB.parse(&mut stream, &mut ctx).unwrap(),
            Value::Int(-1)
        );
    }

    // ======================================================================
    // Integer parse tests — little-endian
    // ======================================================================

    #[test]
    fn parse_int16ul() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\x00\x01"));
        let mut ctx = Context::new();
        assert_eq!(
            INT16UL.parse(&mut stream, &mut ctx).unwrap(),
            Value::UInt(256)
        );
    }

    #[test]
    fn parse_int32ul() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\x00\x01\x00\x00"));
        let mut ctx = Context::new();
        assert_eq!(
            INT32UL.parse(&mut stream, &mut ctx).unwrap(),
            Value::UInt(256)
        );
    }

    #[test]
    fn parse_int64ul() {
        let mut stream =
            CombinedStream::ByteStream(ByteStream::new_read(b"\x00\x01\x00\x00\x00\x00\x00\x00"));
        let mut ctx = Context::new();
        assert_eq!(
            INT64UL.parse(&mut stream, &mut ctx).unwrap(),
            Value::UInt(256)
        );
    }

    #[test]
    fn parse_int8sl_negative() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\xFE"));
        let mut ctx = Context::new();
        assert_eq!(INT8SL.parse(&mut stream, &mut ctx).unwrap(), Value::Int(-2));
    }

    #[test]
    fn parse_int16sl_negative() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\xFE\xFF"));
        let mut ctx = Context::new();
        assert_eq!(
            INT16SL.parse(&mut stream, &mut ctx).unwrap(),
            Value::Int(-2)
        );
    }

    // ======================================================================
    // Integer build tests — big-endian
    // ======================================================================

    #[test]
    fn build_int8ub() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        INT8UB
            .build(&Value::UInt(42), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), vec![42]);
    }

    #[test]
    fn build_int8sb_negative() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        INT8SB
            .build(&Value::Int(-1), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), vec![0xFF]);
    }

    #[test]
    fn build_int16ub() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        INT16UB
            .build(&Value::UInt(256), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), vec![0x01, 0x00]);
    }

    #[test]
    fn build_int32ub() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        INT32UB
            .build(&Value::UInt(0xDEADBEEF), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), vec![0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn build_int64ub() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        INT64UB
            .build(&Value::UInt(0x0102030405060708), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(
            stream.into_bytes(),
            vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]
        );
    }

    // ======================================================================
    // Integer build tests — little-endian
    // ======================================================================

    #[test]
    fn build_int16ul() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        INT16UL
            .build(&Value::UInt(256), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), vec![0x00, 0x01]);
    }

    #[test]
    fn build_int32ul() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        INT32UL
            .build(&Value::UInt(0xDEADBEEF), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), vec![0xEF, 0xBE, 0xAD, 0xDE]);
    }

    #[test]
    fn build_int64ul() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        INT64UL
            .build(&Value::UInt(0x0102030405060708), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(
            stream.into_bytes(),
            vec![0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01]
        );
    }

    // ======================================================================
    // Float parse/build tests
    // ======================================================================

    #[test]
    fn parse_float32b() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\x3F\x80\x00\x00"));
        let mut ctx = Context::new();
        let result = FLOAT32B.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::Float(1.0));
    }

    #[test]
    fn parse_float64b() {
        let mut stream =
            CombinedStream::ByteStream(ByteStream::new_read(b"\x40\x09\x21\xFB\x54\x44\x2D\x18"));
        let mut ctx = Context::new();
        let result = FLOAT64B.parse(&mut stream, &mut ctx).unwrap();
        assert!((result.as_float().unwrap() - std::f64::consts::PI).abs() < 1e-10);
    }

    #[test]
    fn build_float32b() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        FLOAT32B
            .build(&Value::Float(1.0), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), vec![0x3F, 0x80, 0x00, 0x00]);
    }

    #[test]
    fn build_float64b() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        FLOAT64B
            .build(&Value::Float(std::f64::consts::PI), &mut stream, &mut ctx)
            .unwrap();
        let bytes = stream.into_bytes();
        assert_eq!(bytes, vec![0x40, 0x09, 0x21, 0xFB, 0x54, 0x44, 0x2D, 0x18]);
    }

    #[test]
    fn parse_float32l() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\x00\x00\x80\x3F"));
        let mut ctx = Context::new();
        let result = FLOAT32L.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::Float(1.0));
    }

    #[test]
    fn build_float32l() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        FLOAT32L
            .build(&Value::Float(1.0), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), vec![0x00, 0x00, 0x80, 0x3F]);
    }

    // ======================================================================
    // f16 conversion tests
    // ======================================================================

    #[test]
    fn f16_to_f32_zero() {
        assert_eq!(f16_to_f32(0x0000u16), 0.0f32);
        assert_eq!(f16_to_f32(0x8000u16), -0.0f32);
    }

    #[test]
    fn f16_to_f32_one() {
        // f16(1.0) = 0x3C00
        assert_eq!(f16_to_f32(0x3C00), 1.0f32);
        // f16(-1.0) = 0xBC00
        assert_eq!(f16_to_f32(0xBC00), -1.0f32);
    }

    #[test]
    fn f16_to_f32_half() {
        // f16(0.5) = 0x3800
        assert_eq!(f16_to_f32(0x3800), 0.5f32);
    }

    #[test]
    fn f16_to_f32_infinity() {
        // f16(+inf) = 0x7C00
        assert!(f16_to_f32(0x7C00).is_infinite() && f16_to_f32(0x7C00).is_sign_positive());
        // f16(-inf) = 0xFC00
        assert!(f16_to_f32(0xFC00).is_infinite() && f16_to_f32(0xFC00).is_sign_negative());
    }

    #[test]
    fn f16_to_f32_nan() {
        // f16(NaN) = 0x7E00
        assert!(f16_to_f32(0x7E00).is_nan());
    }

    #[test]
    fn f16_to_f32_denormal() {
        // Smallest f16 denormal = 2^(-24) = 0x0001
        let val = f16_to_f32(0x0001);
        assert!((val - 5.9604645e-8).abs() < 1e-14);
    }

    #[test]
    fn f32_to_f16_one() {
        assert_eq!(f32_to_f16(1.0f32), 0x3C00u16);
        assert_eq!(f32_to_f16(-1.0f32), 0xBC00u16);
    }

    #[test]
    fn f32_to_f16_zero() {
        assert_eq!(f32_to_f16(0.0f32), 0x0000u16);
        assert_eq!(f32_to_f16(-0.0f32), 0x8000u16);
    }

    #[test]
    fn f32_to_f16_infinity() {
        assert_eq!(f32_to_f16(f32::INFINITY), 0x7C00u16);
        assert_eq!(f32_to_f16(f32::NEG_INFINITY), 0xFC00u16);
    }

    #[test]
    fn f32_to_f16_nan() {
        let result = f32_to_f16(f32::NAN);
        // NaN has exponent all 1s and non-zero mantissa
        let exp = (result >> F16_MANT_BITS) & ((1 << F16_EXP_BITS) - 1);
        let mant = result & ((1 << F16_MANT_BITS) - 1);
        assert_eq!(exp as u32, (1 << F16_EXP_BITS) - 1);
        assert_ne!(mant, 0);
    }

    // ======================================================================
    // Float16 parse/build tests
    // ======================================================================

    #[test]
    fn parse_float16b_one() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\x3C\x00"));
        let mut ctx = Context::new();
        let result = FLOAT16B.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::Float(1.0));
    }

    #[test]
    fn build_float16b_one() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        FLOAT16B
            .build(&Value::Float(1.0), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), vec![0x3C, 0x00]);
    }

    #[test]
    fn parse_float16l_one() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\x00\x3C"));
        let mut ctx = Context::new();
        let result = FLOAT16L.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::Float(1.0));
    }

    #[test]
    fn build_float16l_one() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        FLOAT16L
            .build(&Value::Float(1.0), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), vec![0x00, 0x3C]);
    }

    // ======================================================================
    // Roundtrip (build → parse) tests
    // ======================================================================

    #[test]
    fn roundtrip_int8ub() {
        let c: &dyn Construct = &INT8UB;
        let original = Value::UInt(42);
        let bytes = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn roundtrip_int8sb_negative() {
        let c: &dyn Construct = &INT8SB;
        let original = Value::Int(-100);
        let bytes = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn roundtrip_int16ub() {
        let c: &dyn Construct = &INT16UB;
        let original = Value::UInt(12345);
        let bytes = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn roundtrip_int32ul() {
        let c: &dyn Construct = &INT32UL;
        let original = Value::UInt(0xDEADBEEF);
        let bytes = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn roundtrip_int64sb() {
        let c: &dyn Construct = &INT64SB;
        let original = Value::Int(-123456789);
        let bytes = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn roundtrip_float32b() {
        // Use a value that is exactly representable in f32 to avoid
        // f64→f32→f64 precision differences.
        let c: &dyn Construct = &FLOAT32B;
        let original = Value::Float(1.0);
        let bytes = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn roundtrip_float64l() {
        let c: &dyn Construct = &FLOAT64L;
        let original = Value::Float(std::f64::consts::E);
        let bytes = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn roundtrip_float16b() {
        let c: &dyn Construct = &FLOAT16B;
        let original = Value::Float(1.0);
        let bytes = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn roundtrip_float16b_half() {
        let c: &dyn Construct = &FLOAT16B;
        let original = Value::Float(0.5);
        let bytes = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    // ======================================================================
    // sizeof tests
    // ======================================================================

    #[test]
    fn sizeof_int8ub() {
        assert_eq!(INT8UB.sizeof(&Context::new()).unwrap(), 1);
    }

    #[test]
    fn sizeof_int16ub() {
        assert_eq!(INT16UB.sizeof(&Context::new()).unwrap(), 2);
    }

    #[test]
    fn sizeof_int32ub() {
        assert_eq!(INT32UB.sizeof(&Context::new()).unwrap(), 4);
    }

    #[test]
    fn sizeof_int64ub() {
        assert_eq!(INT64UB.sizeof(&Context::new()).unwrap(), 8);
    }

    #[test]
    fn sizeof_float16b() {
        assert_eq!(FLOAT16B.sizeof(&Context::new()).unwrap(), 2);
    }

    #[test]
    fn sizeof_float32b() {
        assert_eq!(FLOAT32B.sizeof(&Context::new()).unwrap(), 4);
    }

    #[test]
    fn sizeof_float64b() {
        assert_eq!(FLOAT64B.sizeof(&Context::new()).unwrap(), 8);
    }

    // ======================================================================
    // Error path tests
    // ======================================================================

    #[test]
    fn parse_insufficient_data_returns_stream_error() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\x00"));
        let mut ctx = Context::new();
        let err = INT32UB.parse(&mut stream, &mut ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Stream { .. }));
    }

    #[test]
    fn build_wrong_type_returns_type_mismatch() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        let err = INT32UB
            .build(
                &Value::String("not a number".to_string()),
                &mut stream,
                &mut ctx,
            )
            .unwrap_err();
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
    }

    #[test]
    fn build_value_out_of_range_returns_error() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        let err = INT8UB
            .build(&Value::UInt(256), &mut stream, &mut ctx)
            .unwrap_err();
        assert!(matches!(err, ConstructError::FormatField { .. }));
    }

    #[test]
    fn build_signed_out_of_range_returns_error() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        let err = INT8SB
            .build(&Value::Int(128), &mut stream, &mut ctx)
            .unwrap_err();
        assert!(matches!(err, ConstructError::FormatField { .. }));
    }

    #[test]
    fn build_negative_for_unsigned_returns_error() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        let err = INT8UB
            .build(&Value::Int(-1), &mut stream, &mut ctx)
            .unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    #[test]
    fn build_float_for_int_field_works() {
        // Python struct.pack('>B', 42.0) works — our implementation
        // uses to_i64/to_u64 which reject floats.
        // This is intentional: Python's struct auto-truncates, but we
        // require explicit types for safety.
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        let err = INT8UB
            .build(&Value::Float(42.0), &mut stream, &mut ctx)
            .unwrap_err();
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
    }

    // ======================================================================
    // Alias tests
    // ======================================================================

    #[test]
    fn byte_alias_is_int8ub() {
        let val = Value::UInt(42);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        BYTE.build(&val, &mut stream, &mut ctx).unwrap();
        let bytes = stream.into_bytes();
        let mut stream2 = CombinedStream::ByteStream(ByteStream::new_read(&bytes));
        let mut ctx2 = Context::new();
        assert_eq!(INT8UB.parse(&mut stream2, &mut ctx2).unwrap(), val);
    }

    // ======================================================================
    // Native endian tests (compile-time determined)
    // ======================================================================

    #[test]
    fn roundtrip_int32un() {
        let c: &dyn Construct = &INT32UN;
        let original = Value::UInt(0x12345678);
        let bytes = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    // ======================================================================
    // byte_size() method tests
    // ======================================================================

    #[test]
    fn byte_size_matches_kind() {
        assert_eq!(INT8UB.byte_size(), 1);
        assert_eq!(INT16UB.byte_size(), 2);
        assert_eq!(INT32UB.byte_size(), 4);
        assert_eq!(INT64UB.byte_size(), 8);
        assert_eq!(FLOAT16B.byte_size(), 2);
        assert_eq!(FLOAT32B.byte_size(), 4);
        assert_eq!(FLOAT64B.byte_size(), 8);
    }

    // ======================================================================
    // Convenience method tests (dyn Construct)
    // ======================================================================

    #[test]
    fn parse_bytes_convenience() {
        let c: &dyn Construct = &INT32UB;
        let val = c.parse_bytes(b"\x00\x00\x01\x00").unwrap();
        assert_eq!(val, Value::UInt(256));
    }

    #[test]
    fn build_bytes_convenience() {
        let c: &dyn Construct = &INT32UB;
        let bytes = c.build_bytes(&Value::UInt(256)).unwrap();
        assert_eq!(bytes, vec![0x00, 0x00, 0x01, 0x00]);
    }

    #[test]
    fn roundtrip_via_convenience_methods() {
        let c: &dyn Construct = &INT64SL;
        let original = Value::Int(-999999);
        let bytes = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    // ======================================================================
    // Cross-type integer build tests
    // ======================================================================

    #[test]
    fn build_int32_from_uint_value() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        INT32UB
            .build(&Value::UInt(42), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), vec![0x00, 0x00, 0x00, 0x2A]);
    }

    #[test]
    fn build_int32_from_int_value() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        INT32SB
            .build(&Value::Int(-1), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), vec![0xFF, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn build_int8_from_bigint_value() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        INT8UB
            .build(&Value::BigInt(100), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), vec![100]);
    }

    // ======================================================================
    // Float build from integer value
    // ======================================================================

    #[test]
    fn build_float_from_int_value() {
        // to_f64() accepts all numeric types
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        FLOAT32B
            .build(&Value::Int(1), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), vec![0x3F, 0x80, 0x00, 0x00]);
    }

    #[test]
    fn build_float_from_uint_value() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        FLOAT32B
            .build(&Value::UInt(1), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), vec![0x3F, 0x80, 0x00, 0x00]);
    }
}
