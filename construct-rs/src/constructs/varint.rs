//! VarInt and ZigZag constructs — variable-length integer encoding.
//!
//! - [`VarInt`] encodes unsigned integers using LEB128.
//! - [`ZigZag`] encodes signed integers using ZigZag + VarInt.
//!
//! Corresponds to Python `VarInt` and `ZigZag`.

use crate::core::context::Context;
use crate::core::error::{ConstructError, Result};
use crate::core::stream::Stream;
use crate::core::Construct;
use crate::value::Value;

/// LEB128 unsigned variable-length integer encoding.
///
/// - **parse**: reads LEB128-encoded bytes → [`Value::UInt`] or [`Value::BigInt`]
/// - **build**: encodes a non-negative integer as LEB128 and writes it
/// - **sizeof**: returns [`ConstructError::Sizeof`] (variable size)
///
/// Corresponds to Python `VarInt`.
///
/// # Examples
///
/// ```
/// use construct::constructs::varint::VarInt;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let c: &dyn Construct = &VarInt;
/// let bytes = c.build_bytes(&Value::UInt(300)).unwrap();
/// assert_eq!(bytes, vec![0xAC, 0x02]);
///
/// let parsed = c.parse_bytes(&bytes).unwrap();
/// assert_eq!(parsed, Value::UInt(300));
/// ```
pub struct VarInt;

/// LEB128 continuation bit mask.
const LEB128_CONTINUATION: u8 = 0x80;
/// LEB128 payload mask (lower 7 bits).
const LEB128_PAYLOAD: u8 = 0x7F;

impl VarInt {
    /// Creates a new `VarInt` construct.
    pub fn new() -> Self {
        VarInt
    }
}

impl Default for VarInt {
    fn default() -> Self {
        Self::new()
    }
}

impl Construct for VarInt {
    fn parse(&self, stream: &mut dyn Stream, _ctx: &mut Context) -> Result<Value> {
        let mut acc: Vec<u8> = Vec::new();
        loop {
            let byte = stream.read_bytes(1)?;
            let b = byte[0];
            acc.push(b & LEB128_PAYLOAD);
            if b & LEB128_CONTINUATION == 0 {
                break;
            }
        }

        // Combine 7-bit groups: acc[0] is least significant, acc[last] is most
        let mut num: u128 = 0;
        for &b in acc.iter().rev() {
            num = (num << 7) | (b as u128);
        }

        // Map to appropriate Value variant
        if num <= u64::MAX as u128 {
            Ok(Value::UInt(num as u64))
        } else {
            Ok(Value::BigInt(num as i128))
        }
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, _ctx: &mut Context) -> Result<()> {
        let value = match data {
            Value::Int(i) => {
                if *i < 0 {
                    return Err(ConstructError::Generic {
                        path: String::new(),
                        message: format!("VarInt cannot build from negative number {}", i),
                    });
                }
                *i as u128
            }
            Value::UInt(u) => *u as u128,
            Value::BigInt(i) => {
                if *i < 0 {
                    return Err(ConstructError::Generic {
                        path: String::new(),
                        message: format!("VarInt cannot build from negative number {}", i),
                    });
                }
                *i as u128
            }
            other => {
                return Err(ConstructError::TypeMismatch {
                    path: String::new(),
                    expected: "integer".to_string(),
                    actual: other.type_name().to_string(),
                });
            }
        };

        let mut encoded = Vec::new();
        let mut x = value;
        while x > LEB128_PAYLOAD as u128 {
            encoded.push(LEB128_CONTINUATION | (x & LEB128_PAYLOAD as u128) as u8);
            x >>= 7;
        }
        encoded.push(x as u8);
        stream.write_bytes(&encoded)
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Err(ConstructError::Sizeof {
            path: String::new(),
            reason: "VarInt has variable size".to_string(),
        })
    }
}

/// ZigZag encoding for signed integers, built on top of [`VarInt`].
///
/// ZigZag maps signed integers to unsigned integers so that small-magnitude
/// values (both positive and negative) produce small encoded outputs:
///
/// - encode: `n >= 0 → 2n`, `n < 0 → 2|n| - 1`
/// - decode: `x & 1 == 0 → x/2`, `x & 1 == 1 → -(x/2 + 1)`
///
/// - **parse**: reads VarInt → ZigZag decode → [`Value::Int`] or [`Value::BigInt`]
/// - **build**: ZigZag encode → VarInt write
/// - **sizeof**: returns [`ConstructError::Sizeof`] (variable size)
///
/// Corresponds to Python `ZigZag`.
///
/// # Examples
///
/// ```
/// use construct::constructs::varint::ZigZag;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let c: &dyn Construct = &ZigZag;
/// // -3 → ZigZag(5) → VarInt(0x05)
/// let bytes = c.build_bytes(&Value::Int(-3)).unwrap();
/// assert_eq!(bytes, vec![0x05]);
///
/// let parsed = c.parse_bytes(&bytes).unwrap();
/// assert_eq!(parsed, Value::Int(-3));
/// ```
pub struct ZigZag;

impl ZigZag {
    /// Creates a new `ZigZag` construct.
    pub fn new() -> Self {
        ZigZag
    }
}

impl Default for ZigZag {
    fn default() -> Self {
        Self::new()
    }
}

/// ZigZag-encodes a signed integer to an unsigned integer.
///
/// - `n >= 0 → 2n`
/// - `n < 0 → 2|n| - 1`
fn zigzag_encode(n: i128) -> u128 {
    if n >= 0 {
        (n as u128) * 2
    } else {
        (-(n + 1) as u128) * 2 + 1
    }
}

/// ZigZag-decodes an unsigned integer to a signed integer.
///
/// - `x & 1 == 0 → x/2`
/// - `x & 1 == 1 → -(x/2 + 1)`
fn zigzag_decode(x: u128) -> i128 {
    if x & 1 == 0 {
        (x / 2) as i128
    } else {
        -(((x / 2) + 1) as i128)
    }
}

impl Construct for ZigZag {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let varint_val = VarInt.parse(stream, ctx)?;

        let unsigned = match varint_val {
            Value::UInt(u) => u as u128,
            Value::BigInt(i) => {
                if i < 0 {
                    return Err(ConstructError::Generic {
                        path: String::new(),
                        message: "ZigZag decoded negative intermediate value".to_string(),
                    });
                }
                i as u128
            }
            other => {
                return Err(ConstructError::TypeMismatch {
                    path: String::new(),
                    expected: "unsigned integer".to_string(),
                    actual: other.type_name().to_string(),
                });
            }
        };

        let signed = zigzag_decode(unsigned);

        if signed >= i64::MIN as i128 && signed <= i64::MAX as i128 {
            Ok(Value::Int(signed as i64))
        } else {
            Ok(Value::BigInt(signed))
        }
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        let signed = match data {
            Value::Int(i) => *i as i128,
            Value::UInt(u) => *u as i128,
            Value::BigInt(i) => *i,
            other => {
                return Err(ConstructError::TypeMismatch {
                    path: String::new(),
                    expected: "integer".to_string(),
                    actual: other.type_name().to_string(),
                });
            }
        };

        let encoded = zigzag_encode(signed);

        // Convert to Value for VarInt
        let varint_value = if encoded <= u64::MAX as u128 {
            Value::UInt(encoded as u64)
        } else {
            Value::BigInt(encoded as i128)
        };

        VarInt.build(&varint_value, stream, ctx)
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Err(ConstructError::Sizeof {
            path: String::new(),
            reason: "ZigZag has variable size".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stream::ByteStream;

    // =========================================================================
    // VarInt tests
    // =========================================================================

    #[test]
    fn varint_build_zero() {
        let c: &dyn Construct = &VarInt;
        let bytes = c.build_bytes(&Value::UInt(0)).unwrap();
        assert_eq!(bytes, vec![0x00]);
    }

    #[test]
    fn varint_build_one() {
        let c: &dyn Construct = &VarInt;
        let bytes = c.build_bytes(&Value::UInt(1)).unwrap();
        assert_eq!(bytes, vec![0x01]);
    }

    #[test]
    fn varint_build_127() {
        let c: &dyn Construct = &VarInt;
        let bytes = c.build_bytes(&Value::UInt(127)).unwrap();
        assert_eq!(bytes, vec![0x7F]);
    }

    #[test]
    fn varint_build_128() {
        let c: &dyn Construct = &VarInt;
        let bytes = c.build_bytes(&Value::UInt(128)).unwrap();
        assert_eq!(bytes, vec![0x80, 0x01]);
    }

    #[test]
    fn varint_build_300() {
        // 300 = 0x12C = 0b100101100
        // LEB128: 0b10101100 0b00000010 = 0xAC 0x02
        let c: &dyn Construct = &VarInt;
        let bytes = c.build_bytes(&Value::UInt(300)).unwrap();
        assert_eq!(bytes, vec![0xAC, 0x02]);
    }

    #[test]
    fn varint_parse_zero() {
        let c: &dyn Construct = &VarInt;
        let result = c.parse_bytes(&[0x00]).unwrap();
        assert_eq!(result, Value::UInt(0));
    }

    #[test]
    fn varint_parse_one() {
        let c: &dyn Construct = &VarInt;
        let result = c.parse_bytes(&[0x01]).unwrap();
        assert_eq!(result, Value::UInt(1));
    }

    #[test]
    fn varint_parse_300() {
        let c: &dyn Construct = &VarInt;
        let result = c.parse_bytes(&[0xAC, 0x02]).unwrap();
        assert_eq!(result, Value::UInt(300));
    }

    #[test]
    fn varint_roundtrip_small() {
        let c: &dyn Construct = &VarInt;
        for val in [0, 1, 127, 128, 255, 256, 300, 16383, 16384] {
            let bytes = c.build_bytes(&Value::UInt(val)).unwrap();
            let parsed = c.parse_bytes(&bytes).unwrap();
            assert_eq!(parsed, Value::UInt(val), "roundtrip failed for {val}");
        }
    }

    #[test]
    fn varint_roundtrip_large() {
        let c: &dyn Construct = &VarInt;
        let val = Value::UInt(u64::MAX);
        let bytes = c.build_bytes(&val).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, val);
    }

    #[test]
    fn varint_build_negative() {
        let c: &dyn Construct = &VarInt;
        let err = c.build_bytes(&Value::Int(-1)).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
        assert!(err.to_string().contains("negative"));
    }

    #[test]
    fn varint_build_negative_bigint() {
        let c: &dyn Construct = &VarInt;
        let err = c.build_bytes(&Value::BigInt(-42)).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    #[test]
    fn varint_build_wrong_type() {
        let c: &dyn Construct = &VarInt;
        let err = c.build_bytes(&Value::String("hi".to_string())).unwrap_err();
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
    }

    #[test]
    fn varint_sizeof_error() {
        let ctx = Context::new();
        let err = VarInt.sizeof(&ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Sizeof { .. }));
    }

    #[test]
    fn varint_parse_stream_ends_prematurely() {
        // Continuation bit set but no more data
        let mut stream = ByteStream::new_read(&[0x80]);
        let mut ctx = Context::new();
        let err = VarInt.parse(&mut stream, &mut ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Stream { .. }));
    }

    #[test]
    fn varint_build_from_bigint() {
        let c: &dyn Construct = &VarInt;
        let val = Value::BigInt(300);
        let bytes = c.build_bytes(&val).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, Value::UInt(300));
    }

    #[test]
    fn varint_new_and_default() {
        let _ = VarInt::new();
        let _ = VarInt;
    }

    // =========================================================================
    // ZigZag tests
    // =========================================================================

    #[test]
    fn zigzag_encode_decode_roundtrip() {
        for n in [-10, -3, -1, 0, 1, 3, 10, 100, -100] {
            let encoded = zigzag_encode(n as i128);
            let decoded = zigzag_decode(encoded);
            assert_eq!(decoded as i64, n, "zigzag roundtrip failed for {n}");
        }
    }

    #[test]
    fn zigzag_build_zero() {
        let c: &dyn Construct = &ZigZag;
        let bytes = c.build_bytes(&Value::Int(0)).unwrap();
        assert_eq!(bytes, vec![0x00]);
    }

    #[test]
    fn zigzag_build_neg_one() {
        // -1 → zigzag(1) → VarInt(0x01)
        let c: &dyn Construct = &ZigZag;
        let bytes = c.build_bytes(&Value::Int(-1)).unwrap();
        assert_eq!(bytes, vec![0x01]);
    }

    #[test]
    fn zigzag_build_neg_three() {
        // -3 → zigzag(5) → VarInt(0x05)
        let c: &dyn Construct = &ZigZag;
        let bytes = c.build_bytes(&Value::Int(-3)).unwrap();
        assert_eq!(bytes, vec![0x05]);
    }

    #[test]
    fn zigzag_build_three() {
        // 3 → zigzag(6) → VarInt(0x06)
        let c: &dyn Construct = &ZigZag;
        let bytes = c.build_bytes(&Value::Int(3)).unwrap();
        assert_eq!(bytes, vec![0x06]);
    }

    #[test]
    fn zigzag_parse_zero() {
        let c: &dyn Construct = &ZigZag;
        let result = c.parse_bytes(&[0x00]).unwrap();
        assert_eq!(result, Value::Int(0));
    }

    #[test]
    fn zigzag_parse_neg_one() {
        let c: &dyn Construct = &ZigZag;
        let result = c.parse_bytes(&[0x01]).unwrap();
        assert_eq!(result, Value::Int(-1));
    }

    #[test]
    fn zigzag_parse_neg_three() {
        let c: &dyn Construct = &ZigZag;
        let result = c.parse_bytes(&[0x05]).unwrap();
        assert_eq!(result, Value::Int(-3));
    }

    #[test]
    fn zigzag_parse_three() {
        let c: &dyn Construct = &ZigZag;
        let result = c.parse_bytes(&[0x06]).unwrap();
        assert_eq!(result, Value::Int(3));
    }

    #[test]
    fn zigzag_roundtrip() {
        let c: &dyn Construct = &ZigZag;
        for n in [-100, -3, -1, 0, 1, 3, 100] {
            let bytes = c.build_bytes(&Value::Int(n)).unwrap();
            let parsed = c.parse_bytes(&bytes).unwrap();
            assert_eq!(parsed, Value::Int(n), "zigzag roundtrip failed for {n}");
        }
    }

    #[test]
    fn zigzag_build_wrong_type() {
        let c: &dyn Construct = &ZigZag;
        let err = c.build_bytes(&Value::Bool(true)).unwrap_err();
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
    }

    #[test]
    fn zigzag_sizeof_error() {
        let ctx = Context::new();
        let err = ZigZag.sizeof(&ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Sizeof { .. }));
    }

    #[test]
    fn zigzag_build_from_uint() {
        // UInt(5) → signed 5 → zigzag(10) → VarInt(0x0A)
        let c: &dyn Construct = &ZigZag;
        let bytes = c.build_bytes(&Value::UInt(5)).unwrap();
        assert_eq!(bytes, vec![0x0A]);
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, Value::Int(5));
    }

    #[test]
    fn zigzag_new_and_default() {
        let _ = ZigZag::new();
        let _ = ZigZag;
    }
}
