//! Binary utilities — integer/byte/bit conversion helpers.
//!
//! This module provides low-level functions for converting between integers,
//! byte strings, and bit strings. These are used by the atomic construct
//! implementations ([`BytesInteger`], [`BitsInteger`], [`VarInt`], etc.).
//!
//! Corresponds to Python `construct/construct/lib/binary.py`.
//!
//! [`BytesInteger`]: crate::constructs::bytes_integer::BytesInteger
//! [`BitsInteger`]: crate::constructs::bytes_integer::BitsInteger
//! [`VarInt`]: crate::constructs::varint::VarInt

use crate::core::error::{ConstructError, Result};

/// Maximum value that can be represented in `width` unsigned bytes.
const fn unsigned_max(width: usize) -> u128 {
    if width >= 16 {
        u128::MAX
    } else {
        (1u128).wrapping_shl((width * 8) as u32) - 1
    }
}

/// Maximum value that can be represented in `width` signed bytes (positive side).
const fn signed_max(width: usize) -> i128 {
    if width >= 16 {
        i128::MAX
    } else {
        (1i128).wrapping_shl((width * 8 - 1) as u32) - 1
    }
}

/// Minimum value that can be represented in `width` signed bytes.
const fn signed_min(width: usize) -> i128 {
    if width >= 16 {
        i128::MIN
    } else {
        -(1i128).wrapping_shl((width * 8 - 1) as u32)
    }
}

/// Converts an integer into a big-endian byte string of the given width.
///
/// When `signed` is `true`, the value is encoded using two's complement.
///
/// Corresponds to Python `integer2bytes(number, width, signed=False)`.
///
/// # Errors
///
/// - `width == 0` → [`ConstructError::Generic`] "width must be positive"
/// - Value out of range for the given width/signedness → [`ConstructError::Generic`]
///
/// # Examples
///
/// ```
/// use construct::binary::integer2bytes;
/// let bytes = integer2bytes(19, 4, false).unwrap();
/// assert_eq!(bytes, vec![0x00, 0x00, 0x00, 0x13]);
/// ```
pub fn integer2bytes(value: i128, width: usize, signed: bool) -> Result<Vec<u8>> {
    if width == 0 {
        return Err(ConstructError::Generic {
            path: String::new(),
            message: "width must be positive".to_string(),
        });
    }

    // Range check
    if signed {
        let min = signed_min(width);
        let max = signed_max(width);
        if value < min || value > max {
            return Err(ConstructError::Generic {
                path: String::new(),
                message: format!(
                    "number {} does not fit width {} signed true (range [{}, {}])",
                    value, width, min, max
                ),
            });
        }
    } else {
        let max = unsigned_max(width) as i128;
        if value < 0 || value > max {
            return Err(ConstructError::Generic {
                path: String::new(),
                message: format!(
                    "number {} does not fit width {} signed false (range [0, {}])",
                    value,
                    width,
                    max.min(u64::MAX as i128)
                ),
            });
        }
    }

    // Handle negative values (two's complement)
    let mut v = if value < 0 {
        // For signed negative values, convert to two's complement
        // Use wrapping arithmetic to avoid overflow when width*8 == 128
        let bias = (1u128).wrapping_shl((width * 8) as u32);
        (value as u128).wrapping_add(bias)
    } else {
        value as u128
    };

    let mut result = vec![0u8; width];
    for i in (0..width).rev() {
        result[i] = (v & 0xFF) as u8;
        v >>= 8;
    }
    Ok(result)
}

/// Converts a big-endian byte string into an integer.
///
/// When `signed` is `true`, the result is interpreted as a two's complement
/// signed integer.
///
/// Corresponds to Python `bytes2integer(data, signed=False)`.
///
/// # Errors
///
/// - Empty input → [`ConstructError::Generic`] "byte-string cannot be empty"
///
/// # Examples
///
/// ```
/// use construct::binary::bytes2integer;
/// let val = bytes2integer(&[0x00, 0x00, 0x00, 0x13], false).unwrap();
/// assert_eq!(val, 19);
/// ```
pub fn bytes2integer(data: &[u8], signed: bool) -> Result<i128> {
    if data.is_empty() {
        return Err(ConstructError::Generic {
            path: String::new(),
            message: "byte-string cannot be empty".to_string(),
        });
    }

    let mut number: i128 = 0;
    for &b in data {
        number = (number << 8) | (b as i128);
    }

    if signed && !data.is_empty() && (data[0] & 0x80) != 0 {
        // Two's complement: subtract bias
        let bias = 1i128 << (data.len() * 8);
        number -= bias;
    }

    Ok(number)
}

/// Converts an integer into a bit string (each byte is 0x00 or 0x01, big-endian bit order).
///
/// `width` is the number of bits to generate. The most significant bit is first.
///
/// Corresponds to Python `integer2bits(number, width, signed=False)`.
///
/// # Errors
///
/// - `width == 0` → [`ConstructError::Generic`] "width must be positive"
/// - Value out of range → [`ConstructError::Generic`]
///
/// # Examples
///
/// ```
/// use construct::binary::integer2bits;
/// let bits = integer2bits(19, 8, false).unwrap();
/// assert_eq!(bits, vec![0, 0, 0, 1, 0, 0, 1, 1]);
/// ```
pub fn integer2bits(value: i128, width: usize, signed: bool) -> Result<Vec<u8>> {
    if width == 0 {
        return Err(ConstructError::Generic {
            path: String::new(),
            message: "width must be positive".to_string(),
        });
    }

    // Range check
    if signed {
        let min = -(1i128 << (width - 1));
        let max = (1i128 << (width - 1)) - 1;
        if value < min || value > max {
            return Err(ConstructError::Generic {
                path: String::new(),
                message: format!("number {} out of range (min={}, max={})", value, min, max),
            });
        }
    } else {
        let max = (1i128 << width) - 1;
        if value < 0 || value > max {
            return Err(ConstructError::Generic {
                path: String::new(),
                message: format!("number {} out of range (min=0, max={})", value, max),
            });
        }
    }

    let mut number = value;

    // Handle negative: convert to unsigned representation
    if number < 0 {
        number += 1i128 << width;
    }

    let mut bits = vec![0u8; width];
    let mut i = width;
    while number > 0 && i > 0 {
        i -= 1;
        bits[i] = (number & 1) as u8;
        number >>= 1;
    }
    Ok(bits)
}

/// Converts a bit string (each byte is 0x00 or 0x01) into an integer.
///
/// When `signed` is `true`, the result is interpreted as a two's complement
/// signed integer.
///
/// Corresponds to Python `bits2integer(data, signed=False)`.
///
/// # Errors
///
/// - Empty input → [`ConstructError::Generic`] "bit-string cannot be empty"
///
/// # Examples
///
/// ```
/// use construct::binary::bits2integer;
/// let val = bits2integer(&[0, 0, 0, 1, 0, 0, 1, 1], false).unwrap();
/// assert_eq!(val, 19);
/// ```
pub fn bits2integer(data: &[u8], signed: bool) -> Result<i128> {
    if data.is_empty() {
        return Err(ConstructError::Generic {
            path: String::new(),
            message: "bit-string cannot be empty".to_string(),
        });
    }

    let mut number: i128 = 0;
    for &b in data {
        number = (number << 1) | (b as i128);
    }

    if signed && data[0] != 0 {
        let bias = 1i128 << data.len();
        number -= bias;
    }

    Ok(number)
}

/// Reverses the byte order (endianness swap).
///
/// Corresponds to Python `swapbytes(data)`.
///
/// # Examples
///
/// ```
/// use construct::binary::swapbytes;
/// assert_eq!(swapbytes(&[0x01, 0x02, 0x03, 0x04]), vec![0x04, 0x03, 0x02, 0x01]);
/// ```
pub fn swapbytes(data: &[u8]) -> Vec<u8> {
    data.iter().rev().copied().collect()
}

/// Swaps byte groups within a bit string.
///
/// The length of `data` must be a multiple of 8. Each 8-bit group is reordered
/// in reverse order (last group becomes first).
///
/// Corresponds to Python `swapbytesinbits(data)`.
///
/// # Errors
///
/// - Length not a multiple of 8 → [`ConstructError::Generic`]
pub fn swapbytesinbits(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() % 8 != 0 {
        return Err(ConstructError::Generic {
            path: String::new(),
            message: format!(
                "little-endianness only defined if data length {} is multiple of 8",
                data.len()
            ),
        });
    }
    let mut result = Vec::with_capacity(data.len());
    for i in (0..data.len()).step_by(8).rev() {
        result.extend_from_slice(&data[i..i + 8]);
    }
    Ok(result)
}

/// Converts a byte string into a bit string.
///
/// Each byte is expanded into 8 bits (0x00 or 0x01), most significant bit first.
///
/// Corresponds to Python `bytes2bits(data)`.
///
/// # Examples
///
/// ```
/// use construct::binary::bytes2bits;
/// let bits = bytes2bits(&[0xFF]);
/// assert_eq!(bits, vec![1, 1, 1, 1, 1, 1, 1, 1]);
/// ```
pub fn bytes2bits(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(data.len() * 8);
    for &byte in data {
        for shift in (0..8).rev() {
            result.push((byte >> shift) & 1);
        }
    }
    result
}

/// Converts a bit string into a byte string.
///
/// Every 8 bits are combined into one byte. The length of `data` must be a
/// multiple of 8.
///
/// Corresponds to Python `bits2bytes(data)`.
///
/// # Errors
///
/// - Length not a multiple of 8 → [`ConstructError::Generic`]
///
/// # Examples
///
/// ```
/// use construct::binary::bits2bytes;
/// let bytes = bits2bytes(&[1, 1, 1, 1, 1, 1, 1, 1]).unwrap();
/// assert_eq!(bytes, vec![0xFF]);
/// ```
pub fn bits2bytes(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() % 8 != 0 {
        return Err(ConstructError::Generic {
            path: String::new(),
            message: format!("data length {} must be a multiple of 8", data.len()),
        });
    }
    let mut result = Vec::with_capacity(data.len() / 8);
    for chunk in data.chunks(8) {
        let mut byte = 0u8;
        for &bit in chunk {
            byte = (byte << 1) | (bit & 1);
        }
        result.push(byte);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // integer2bytes tests
    // =========================================================================

    #[test]
    fn integer2bytes_zero_width() {
        let err = integer2bytes(0, 0, false).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
        assert!(err.to_string().contains("width must be positive"));
    }

    #[test]
    fn integer2bytes_basic() {
        assert_eq!(
            integer2bytes(19, 4, false).unwrap(),
            vec![0x00, 0x00, 0x00, 0x13]
        );
    }

    #[test]
    fn integer2bytes_single_byte() {
        assert_eq!(integer2bytes(0, 1, false).unwrap(), vec![0x00]);
        assert_eq!(integer2bytes(255, 1, false).unwrap(), vec![0xFF]);
        assert_eq!(integer2bytes(127, 1, true).unwrap(), vec![0x7F]);
        assert_eq!(integer2bytes(-128, 1, true).unwrap(), vec![0x80]);
    }

    #[test]
    fn integer2bytes_signed_negative() {
        assert_eq!(
            integer2bytes(-1, 4, true).unwrap(),
            vec![0xFF, 0xFF, 0xFF, 0xFF]
        );
    }

    #[test]
    fn integer2bytes_overflow_unsigned() {
        let err = integer2bytes(256, 1, false).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    #[test]
    fn integer2bytes_negative_unsigned() {
        let err = integer2bytes(-1, 1, false).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    #[test]
    fn integer2bytes_overflow_signed() {
        let err = integer2bytes(128, 1, true).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
        let err = integer2bytes(-129, 1, true).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    #[test]
    fn integer2bytes_two_bytes() {
        assert_eq!(integer2bytes(0x0102, 2, false).unwrap(), vec![0x01, 0x02]);
        assert_eq!(integer2bytes(-1, 2, true).unwrap(), vec![0xFF, 0xFF]); // -1
    }

    // =========================================================================
    // bytes2integer tests
    // =========================================================================

    #[test]
    fn bytes2integer_empty() {
        let err = bytes2integer(&[], false).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn bytes2integer_basic() {
        assert_eq!(bytes2integer(&[0x00, 0x00, 0x00, 0x13], false).unwrap(), 19);
    }

    #[test]
    fn bytes2integer_unsigned_max() {
        assert_eq!(bytes2integer(&[0xFF], false).unwrap(), 255);
        assert_eq!(bytes2integer(&[0xFF, 0xFF], false).unwrap(), 65535);
    }

    #[test]
    fn bytes2integer_signed_negative() {
        assert_eq!(bytes2integer(&[0xFF], true).unwrap(), -1);
        assert_eq!(bytes2integer(&[0x80], true).unwrap(), -128);
        assert_eq!(bytes2integer(&[0xFF, 0xFF], true).unwrap(), -1);
    }

    #[test]
    fn bytes2integer_signed_positive() {
        assert_eq!(bytes2integer(&[0x7F], true).unwrap(), 127);
        assert_eq!(bytes2integer(&[0x00, 0x80], true).unwrap(), 128);
    }

    // =========================================================================
    // integer2bits tests
    // =========================================================================

    #[test]
    fn integer2bits_zero_width() {
        let err = integer2bits(0, 0, false).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    #[test]
    fn integer2bits_basic() {
        // 19 = 0b00010011
        assert_eq!(
            integer2bits(19, 8, false).unwrap(),
            vec![0, 0, 0, 1, 0, 0, 1, 1]
        );
    }

    #[test]
    fn integer2bits_zero() {
        assert_eq!(integer2bits(0, 4, false).unwrap(), vec![0, 0, 0, 0]);
    }

    #[test]
    fn integer2bits_signed_negative() {
        // -1 in 4 bits: 1111 (two's complement)
        assert_eq!(integer2bits(-1, 4, true).unwrap(), vec![1, 1, 1, 1]);
    }

    #[test]
    fn integer2bits_out_of_range_unsigned() {
        let err = integer2bits(16, 4, false).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    #[test]
    fn integer2bits_out_of_range_signed() {
        let err = integer2bits(8, 4, true).unwrap_err(); // max is 7
        assert!(matches!(err, ConstructError::Generic { .. }));
        let err = integer2bits(-9, 4, true).unwrap_err(); // min is -8
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    #[test]
    fn integer2bits_negative_unsigned() {
        let err = integer2bits(-1, 8, false).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    // =========================================================================
    // bits2integer tests
    // =========================================================================

    #[test]
    fn bits2integer_empty() {
        let err = bits2integer(&[], false).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    #[test]
    fn bits2integer_basic() {
        assert_eq!(bits2integer(&[0, 0, 0, 1, 0, 0, 1, 1], false).unwrap(), 19);
    }

    #[test]
    fn bits2integer_all_ones_unsigned() {
        assert_eq!(bits2integer(&[1, 1, 1, 1], false).unwrap(), 15);
    }

    #[test]
    fn bits2integer_signed_negative() {
        // 1111 signed = -1 (4-bit two's complement)
        assert_eq!(bits2integer(&[1, 1, 1, 1], true).unwrap(), -1);
    }

    #[test]
    fn bits2integer_signed_positive() {
        // 0111 signed = 7
        assert_eq!(bits2integer(&[0, 1, 1, 1], true).unwrap(), 7);
    }

    #[test]
    fn bits2integer_signed_min() {
        // 1000 signed = -8 (4-bit two's complement minimum)
        assert_eq!(bits2integer(&[1, 0, 0, 0], true).unwrap(), -8);
    }

    // =========================================================================
    // swapbytes tests
    // =========================================================================

    #[test]
    fn swapbytes_basic() {
        assert_eq!(
            swapbytes(&[0x01, 0x02, 0x03, 0x04]),
            vec![0x04, 0x03, 0x02, 0x01]
        );
    }

    #[test]
    fn swapbytes_empty() {
        assert_eq!(swapbytes(&[]), Vec::<u8>::new());
    }

    #[test]
    fn swapbytes_single() {
        assert_eq!(swapbytes(&[0xAB]), vec![0xAB]);
    }

    // =========================================================================
    // swapbytesinbits tests
    // =========================================================================

    #[test]
    fn swapbytesinbits_basic() {
        // 00000000 11111111 -> 11111111 00000000
        let input: Vec<u8> = vec![0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1];
        let expected: Vec<u8> = vec![1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(swapbytesinbits(&input).unwrap(), expected);
    }

    #[test]
    fn swapbytesinbits_not_multiple_of_8() {
        let err = swapbytesinbits(&[0, 0, 0]).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    #[test]
    fn swapbytesinbits_empty() {
        assert_eq!(swapbytesinbits(&[]).unwrap(), Vec::<u8>::new());
    }

    // =========================================================================
    // bytes2bits / bits2bytes tests
    // =========================================================================

    #[test]
    fn bytes2bits_basic() {
        // 0xAB = 10101011
        assert_eq!(bytes2bits(&[0xAB]), vec![1, 0, 1, 0, 1, 0, 1, 1]);
    }

    #[test]
    fn bytes2bits_zero() {
        assert_eq!(bytes2bits(&[0x00]), vec![0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn bytes2bits_empty() {
        assert!(bytes2bits(&[]).is_empty());
    }

    #[test]
    fn bytes2bits_multi_byte() {
        // 'a' = 0x61 = 01100001, 'b' = 0x62 = 01100010
        let bits = bytes2bits(b"ab");
        assert_eq!(
            bits,
            vec![
                0, 1, 1, 0, 0, 0, 0, 1, // 'a'
                0, 1, 1, 0, 0, 0, 1, 0, // 'b'
            ]
        );
    }

    #[test]
    fn bits2bytes_basic() {
        assert_eq!(bits2bytes(&[1, 1, 1, 1, 1, 1, 1, 1]).unwrap(), vec![0xFF]);
    }

    #[test]
    fn bits2bytes_roundtrip() {
        let original = b"hello";
        let bits = bytes2bits(original);
        let bytes = bits2bytes(&bits).unwrap();
        assert_eq!(bytes, original);
    }

    #[test]
    fn bits2bytes_not_multiple_of_8() {
        let err = bits2bytes(&[1, 0, 1]).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    #[test]
    fn bits2bytes_empty() {
        assert_eq!(bits2bytes(&[]).unwrap(), Vec::<u8>::new());
    }

    // =========================================================================
    // Roundtrip tests: integer2bytes ↔ bytes2integer
    // =========================================================================

    #[test]
    fn integer_bytes_roundtrip_unsigned() {
        for width in 1..=8 {
            let max: u64 = if width >= 8 {
                u64::MAX
            } else {
                (1u64 << (width * 8)) - 1
            };
            let test_vals: Vec<u64> = {
                let mut v = vec![0, 1];
                if max > 2 {
                    v.push(max / 2);
                }
                v.push(max);
                v.sort();
                v.dedup();
                v
            };
            for &val in &test_vals {
                let bytes = integer2bytes(val as i128, width, false).unwrap();
                let back = bytes2integer(&bytes, false).unwrap();
                assert_eq!(
                    back, val as i128,
                    "roundtrip failed for val={val}, width={width}"
                );
            }
        }
    }

    #[test]
    fn integer_bytes_roundtrip_signed() {
        for width in 1..=8 {
            let min = signed_min(width);
            let max = signed_max(width);
            let test_vals: Vec<i128> = {
                let mut v = vec![min, -1, 0];
                if max > 0 {
                    v.push(1);
                    v.push(max - 1);
                }
                v.push(max);
                v.sort();
                v.dedup();
                v
            };
            for &val in &test_vals {
                let bytes = integer2bytes(val, width, true).unwrap();
                let back = bytes2integer(&bytes, true).unwrap();
                assert_eq!(back, val, "roundtrip failed for val={val}, width={width}");
            }
        }
    }

    // =========================================================================
    // Roundtrip tests: integer2bits ↔ bits2integer
    // =========================================================================

    #[test]
    fn integer_bits_roundtrip_unsigned() {
        for width in 1..=8 {
            let max = (1i128 << width) - 1;
            for &val in &[0, 1, max / 2, max] {
                let bits = integer2bits(val, width, false).unwrap();
                let back = bits2integer(&bits, false).unwrap();
                assert_eq!(back, val, "roundtrip failed for val={val}, width={width}");
            }
        }
    }

    #[test]
    fn integer_bits_roundtrip_signed() {
        for width in 1..=8 {
            let min = -(1i128 << (width - 1));
            let max = (1i128 << (width - 1)) - 1;
            let test_vals: Vec<i128> = {
                let mut v = vec![min, -1, 0];
                if max > 0 {
                    v.push(1);
                    v.push(max - 1);
                }
                v.push(max);
                v.sort();
                v.dedup();
                v
            };
            for &val in &test_vals {
                let bits = integer2bits(val, width, true).unwrap();
                let back = bits2integer(&bits, true).unwrap();
                assert_eq!(back, val, "roundtrip failed for val={val}, width={width}");
            }
        }
    }
}
