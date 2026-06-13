//! Unreal Tournament 1999 Index — variable-length signed integer format.
//!
//! Ported from `construct/gallery/ut_index.py`.
//!
//! # Format layout
//!
//! ```text
//! +------------------------------------+-------------------------+--------------+
//! | Byte 0                             | Bytes 1-3               | Byte 4       |
//! +----------+----------+--------------+----------+--------------+--------------+
//! | Sign Bit | More Bit | Data Bits[6] | More Bit | Data Bits[7] | Data Bits[8] |
//! +----------+----------+--------------+----------+--------------+--------------+
//! ```
//!
//! - Byte 0: 1 sign bit + 1 continuation bit + 6 data bits
//! - Bytes 1-3: 1 continuation bit + 7 data bits
//! - Byte 4: 8 data bits (no continuation bit; max 5 bytes total)
//!
//! Maximum value: 2^(6+7+7+7+8) − 1 = 2^35 − 1 = 34_359_738_367.

use crate::core::context::Context;
use crate::core::error::{ConstructError, Result};
use crate::core::stream::Stream;
use crate::core::Construct;
use crate::value::Value;

/// Data-bit lengths for each of the 5 possible bytes.
const LENGTHS: [usize; 5] = [6, 7, 7, 7, 8];

/// Sign bit mask in byte 0.
const NEGATIVE_BIT: u8 = 0x80;

/// Number of possible bytes in an Index.
const MAX_BYTES: usize = 5;

/// Returns the data-bit mask for a given data-bit length.
fn get_data_mask(length: usize) -> u8 {
    match length {
        6 => 0x3F,
        7 => 0x7F,
        8 => 0xFF,
        _ => 0x00,
    }
}

/// Returns the continuation ("more") bit for a given data-bit length.
fn get_more_bit(length: usize) -> u8 {
    if length >= 8 {
        0
    } else {
        1 << length
    }
}

/// Unreal Tournament 1999 Index variable-length signed integer.
///
/// - **parse**: reads bytes one at a time, accumulating data bits until the
///   continuation bit is 0
/// - **build**: splits the integer into data-bit segments, setting the sign
///   and continuation bits appropriately
/// - **sizeof**: always returns [`ConstructError::Sizeof`] (variable length)
///
/// Corresponds to the Python `UTIndex` class.
///
/// # Examples
///
/// ```
/// use construct::gallery::ut_index::UTIndex;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let c: &dyn Construct = &UTIndex::new();
///
/// // Value 0 → single byte 0x00
/// let built = c.build_bytes(&Value::Int(0)).unwrap();
/// assert_eq!(built, vec![0x00]);
///
/// // Value 63 → single byte 0x3F (6 data bits all set, no continuation)
/// let built = c.build_bytes(&Value::Int(63)).unwrap();
/// assert_eq!(built, vec![0x3F]);
///
/// // Value 64 → two bytes: 0x40 (data=0, more) + 0x01 (data=1)
/// let built = c.build_bytes(&Value::Int(64)).unwrap();
/// assert_eq!(built, vec![0x40, 0x01]);
/// ```
pub struct UTIndex;

impl UTIndex {
    /// Creates a new `UTIndex` construct.
    pub fn new() -> Self {
        UTIndex
    }
}

impl Default for UTIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl Construct for UTIndex {
    fn parse(&self, stream: &mut dyn Stream, _ctx: &mut Context) -> Result<Value> {
        let mut result: i64 = 0;
        let mut sign: i64 = 1;
        let mut i: usize = 0;
        let mut depth: u32 = 0;

        loop {
            let length = LENGTHS[i];
            let raw = stream.read_bytes(1)?;
            let byte = raw[0];
            let mask = get_data_mask(length);
            let data = (byte & mask) as i64;
            let more = get_more_bit(length) & byte;

            if i == 0 && (NEGATIVE_BIT & byte) != 0 {
                sign = -1;
            }

            result |= data << depth;

            if more == 0 {
                break;
            }

            i += 1;
            depth += length as u32;

            if i >= MAX_BYTES {
                return Err(ConstructError::Generic {
                    path: String::new(),
                    message: "UTIndex: continuation bit set on byte 4 which has no more bit"
                        .to_string(),
                });
            }
        }

        Ok(Value::Int(sign * result))
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, _ctx: &mut Context) -> Result<()> {
        let obj = data.to_i64().map_err(|e| e.with_path_prefix("UTIndex"))?;

        let mut to_write: i64 = obj;
        let negative = obj < 0;
        if negative {
            to_write = -to_write;
        }

        for (i, &length) in LENGTHS.iter().enumerate() {
            let mask = get_data_mask(length);
            let mut byte: u8 = 0;

            if i == 0 && negative {
                byte |= NEGATIVE_BIT;
            }

            byte |= (to_write as u8) & mask;
            to_write >>= length;

            // More bit: only set if there's remaining data AND this is not
            // the last byte (byte 4 uses all 8 bits for data, no continuation).
            let more_bit = if to_write > 0 && i < MAX_BYTES - 1 {
                get_more_bit(length)
            } else {
                0
            };
            byte |= more_bit;

            stream.write_bytes(&[byte])?;

            if more_bit == 0 {
                break;
            }
        }

        // If we stopped but still have data, the value is too large.
        if to_write > 0 {
            return Err(ConstructError::Generic {
                path: String::new(),
                message: format!(
                    "UTIndex: value {obj} exceeds maximum representable value (2^35 - 1)"
                ),
            });
        }

        Ok(())
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Err(ConstructError::Sizeof {
            path: String::new(),
            reason: "UTIndex has variable size".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ======================================================================
    // Parse tests
    // ======================================================================

    #[test]
    fn parse_zero() {
        let c: &dyn Construct = &UTIndex::new();
        assert_eq!(c.parse_bytes(&[0x00]).unwrap(), Value::Int(0));
    }

    #[test]
    fn parse_single_byte_max() {
        // 6 data bits all set = 63, no continuation
        let c: &dyn Construct = &UTIndex::new();
        assert_eq!(c.parse_bytes(&[0x3F]).unwrap(), Value::Int(63));
    }

    #[test]
    fn parse_two_bytes() {
        // 0x40 = continuation bit set, data=0
        // 0x01 = data=1, no continuation
        // value = 0 + (1 << 6) = 64
        let c: &dyn Construct = &UTIndex::new();
        assert_eq!(c.parse_bytes(&[0x40, 0x01]).unwrap(), Value::Int(64));
    }

    #[test]
    fn parse_three_bytes() {
        // value = 64 + 127*64 = 8192
        // 0x7F = data=63, continuation → depth=6
        // 0xFF = data=127, continuation → depth=13
        // 0x01 = data=1 → value = 63 + (127<<6) + (1<<13) = 63 + 8128 + 8192 = 16383
        let c: &dyn Construct = &UTIndex::new();
        assert_eq!(
            c.parse_bytes(&[0x7F, 0xFF, 0x01]).unwrap(),
            Value::Int(16383)
        );
    }

    #[test]
    fn parse_negative_one() {
        // 0x80 = sign bit + data=0, no continuation → -1 * 1 = -1
        let c: &dyn Construct = &UTIndex::new();
        assert_eq!(c.parse_bytes(&[0x81]).unwrap(), Value::Int(-1));
    }

    #[test]
    fn parse_negative_63() {
        // 0xBF = sign bit + data=63, no continuation → -63
        let c: &dyn Construct = &UTIndex::new();
        assert_eq!(c.parse_bytes(&[0xBF]).unwrap(), Value::Int(-63));
    }

    #[test]
    fn parse_five_bytes() {
        // Maximum 5-byte value (all data bits set)
        // byte0: 0x7F (data=63, continuation bit set)
        // byte1: 0xFF (data=127, continuation set)
        // byte2: 0xFF (data=127, continuation set)
        // byte3: 0xFF (data=127, continuation set)
        // byte4: 0xFF (8 bits data, no continuation possible)
        let c: &dyn Construct = &UTIndex::new();
        let result = c.parse_bytes(&[0x7F, 0xFF, 0xFF, 0xFF, 0xFF]).unwrap();
        // 63 + (127<<6) + (127<<13) + (127<<20) + (255<<27)
        // = 2^35 - 1 = 34359738367
        assert_eq!(result, Value::Int(34_359_738_367));
    }

    #[test]
    fn parse_eof_returns_error() {
        let c: &dyn Construct = &UTIndex::new();
        let result = c.parse_bytes(b"");
        assert!(result.is_err());
    }

    // ======================================================================
    // Build tests
    // ======================================================================

    #[test]
    fn build_zero() {
        let c: &dyn Construct = &UTIndex::new();
        let built = c.build_bytes(&Value::Int(0)).unwrap();
        assert_eq!(built, vec![0x00]);
    }

    #[test]
    fn build_small_positive() {
        let c: &dyn Construct = &UTIndex::new();
        let built = c.build_bytes(&Value::Int(42)).unwrap();
        assert_eq!(built, vec![42]);
    }

    #[test]
    fn build_single_byte_max() {
        let c: &dyn Construct = &UTIndex::new();
        let built = c.build_bytes(&Value::Int(63)).unwrap();
        assert_eq!(built, vec![0x3F]);
    }

    #[test]
    fn build_two_bytes() {
        let c: &dyn Construct = &UTIndex::new();
        let built = c.build_bytes(&Value::Int(64)).unwrap();
        assert_eq!(built, vec![0x40, 0x01]);
    }

    #[test]
    fn build_negative_one() {
        let c: &dyn Construct = &UTIndex::new();
        let built = c.build_bytes(&Value::Int(-1)).unwrap();
        // sign bit (0x80) + data=1 = 0x81
        assert_eq!(built, vec![0x81]);
    }

    #[test]
    fn build_negative_value() {
        let c: &dyn Construct = &UTIndex::new();
        let built = c.build_bytes(&Value::Int(-64)).unwrap();
        // sign bit (0x80) + continuation (0x40) + data=0
        // then 0x01 (data=1, no continuation)
        // value = -(0 + (1 << 6)) = -64
        assert_eq!(built, vec![0xC0, 0x01]);
    }

    #[test]
    fn build_uint_value() {
        let c: &dyn Construct = &UTIndex::new();
        let built = c.build_bytes(&Value::UInt(128)).unwrap();
        // 128 = 0 + (2 << 6)
        // byte0: continuation (0x40) + data=0 → 0x40
        // byte1: data=2, no continuation → 0x02
        assert_eq!(built, vec![0x40, 0x02]);
    }

    #[test]
    fn build_non_integer_returns_error() {
        let c: &dyn Construct = &UTIndex::new();
        let result = c.build_bytes(&Value::String("abc".to_string()));
        assert!(result.is_err());
    }

    // ======================================================================
    // Roundtrip tests
    // ======================================================================

    #[test]
    fn roundtrip_zero() {
        let c: &dyn Construct = &UTIndex::new();
        let built = c.build_bytes(&Value::Int(0)).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, Value::Int(0));
    }

    #[test]
    fn roundtrip_positive_values() {
        let c: &dyn Construct = &UTIndex::new();
        for &v in &[
            1,
            10,
            63,
            64,
            127,
            128,
            8191,
            8192,
            1_000_000,
            34_359_738_367,
        ] {
            let built = c.build_bytes(&Value::Int(v)).unwrap();
            let parsed = c.parse_bytes(&built).unwrap();
            assert_eq!(parsed, Value::Int(v), "roundtrip failed for value {v}");
        }
    }

    #[test]
    fn roundtrip_negative_values() {
        let c: &dyn Construct = &UTIndex::new();
        for &v in &[-1, -10, -63, -64, -127, -128, -8191, -8192, -1_000_000] {
            let built = c.build_bytes(&Value::Int(v)).unwrap();
            let parsed = c.parse_bytes(&built).unwrap();
            assert_eq!(parsed, Value::Int(v), "roundtrip failed for value {v}");
        }
    }

    // ======================================================================
    // Boundary tests
    // ======================================================================

    #[test]
    fn build_value_too_large_returns_error() {
        let c: &dyn Construct = &UTIndex::new();
        // 2^35 is too large (max is 2^35 - 1)
        let err = c.build_bytes(&Value::Int(1i64 << 35)).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    #[test]
    fn sizeof_returns_error() {
        let c: &dyn Construct = &UTIndex::new();
        let err = c.sizeof(&Context::new()).unwrap_err();
        assert!(matches!(err, ConstructError::Sizeof { .. }));
    }

    #[test]
    fn new_and_default() {
        let _ = UTIndex::new();
        let _ = UTIndex;
    }
}
