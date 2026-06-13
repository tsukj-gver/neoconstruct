//! String constructs — CString, PaddedString, and StringEncoding.
//!
//! This module provides string-oriented constructs that encode/decode between
//! Rust [`String`] and binary byte sequences.
//!
//! | Construct | Purpose |
//! |-----------|---------|
//! | [`CString`] | Null-terminated string (reads until `\0`) |
//! | [`PaddedString`] | Fixed-length string padded with null bytes |
//!
//! Corresponds to Python `CString(encoding)` and `PaddedString(length, encoding)`.

use crate::core::context::Context;
use crate::core::error::{ConstructError, Result};
use crate::core::stream::Stream;
use crate::core::Construct;
use crate::value::Value;

// ===========================================================================
// StringEncoding
// ===========================================================================

/// The character encoding used by [`CString`] and [`PaddedString`].
///
/// Corresponds to the Python encoding name string (`"ascii"`, `"utf8"`,
/// `"utf16"`, etc.).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StringEncoding {
    /// ASCII encoding (1 byte per character; non-ASCII bytes cause errors).
    Ascii,
    /// UTF-8 encoding (1–4 bytes per character).
    Utf8,
    /// UTF-16 encoding, native host byte order.
    Utf16,
    /// UTF-16 big-endian encoding.
    Utf16Be,
    /// UTF-16 little-endian encoding.
    Utf16Le,
    /// UTF-32 encoding, native host byte order.
    Utf32,
    /// UTF-32 big-endian encoding.
    Utf32Be,
    /// UTF-32 little-endian encoding.
    Utf32Le,
}

impl StringEncoding {
    /// Returns the byte size of a single terminator (null) unit for this
    /// encoding.
    ///
    /// - ASCII/UTF-8: 1 byte (`0x00`)
    /// - UTF-16: 2 bytes (`0x00 0x00`)
    /// - UTF-32: 4 bytes (`0x00 0x00 0x00 0x00`)
    #[must_use]
    pub const fn term_size(&self) -> usize {
        match self {
            StringEncoding::Ascii | StringEncoding::Utf8 => 1,
            StringEncoding::Utf16 | StringEncoding::Utf16Be | StringEncoding::Utf16Le => 2,
            StringEncoding::Utf32 | StringEncoding::Utf32Be | StringEncoding::Utf32Le => 4,
        }
    }

    /// Decodes a byte slice into a Rust [`String`] using this encoding.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::StringEncoding`] for UTF-8/ASCII decode
    /// failures, or [`ConstructError::Generic`] for UTF-16/UTF-32 decode
    /// failures (length not a multiple of the code unit).
    pub fn decode(&self, data: &[u8]) -> Result<String> {
        match self {
            StringEncoding::Ascii => {
                // ASCII: each byte must be <= 0x7F
                for &b in data {
                    if b > 0x7F {
                        return Err(ConstructError::Generic {
                            path: String::new(),
                            message: format!(
                                "ASCII decode failed: byte 0x{:02X} is not valid ASCII",
                                b
                            ),
                        });
                    }
                }
                Ok(String::from_utf8_lossy(data).into_owned())
            }
            StringEncoding::Utf8 => {
                String::from_utf8(data.to_vec()).map_err(|e| ConstructError::StringEncoding {
                    path: String::new(),
                    source: e,
                })
            }
            StringEncoding::Utf16 => {
                if data.len() % 2 != 0 {
                    return Err(ConstructError::Generic {
                        path: String::new(),
                        message: format!(
                            "UTF-16 decode failed: data length {} is not a multiple of 2",
                            data.len()
                        ),
                    });
                }
                decode_utf16(data, cfg!(target_endian = "little"))
            }
            StringEncoding::Utf16Be => {
                if data.len() % 2 != 0 {
                    return Err(ConstructError::Generic {
                        path: String::new(),
                        message: format!(
                            "UTF-16BE decode failed: data length {} is not a multiple of 2",
                            data.len()
                        ),
                    });
                }
                decode_utf16(data, false)
            }
            StringEncoding::Utf16Le => {
                if data.len() % 2 != 0 {
                    return Err(ConstructError::Generic {
                        path: String::new(),
                        message: format!(
                            "UTF-16LE decode failed: data length {} is not a multiple of 2",
                            data.len()
                        ),
                    });
                }
                decode_utf16(data, true)
            }
            StringEncoding::Utf32 => {
                if data.len() % 4 != 0 {
                    return Err(ConstructError::Generic {
                        path: String::new(),
                        message: format!(
                            "UTF-32 decode failed: data length {} is not a multiple of 4",
                            data.len()
                        ),
                    });
                }
                decode_utf32(data, cfg!(target_endian = "little"))
            }
            StringEncoding::Utf32Be => {
                if data.len() % 4 != 0 {
                    return Err(ConstructError::Generic {
                        path: String::new(),
                        message: format!(
                            "UTF-32BE decode failed: data length {} is not a multiple of 4",
                            data.len()
                        ),
                    });
                }
                decode_utf32(data, false)
            }
            StringEncoding::Utf32Le => {
                if data.len() % 4 != 0 {
                    return Err(ConstructError::Generic {
                        path: String::new(),
                        message: format!(
                            "UTF-32LE decode failed: data length {} is not a multiple of 4",
                            data.len()
                        ),
                    });
                }
                decode_utf32(data, true)
            }
        }
    }

    /// Encodes a Rust [`&str`] into bytes using this encoding.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::Generic`] if a character cannot be represented
    /// in the target encoding (e.g. non-ASCII in ASCII mode).
    pub fn encode(&self, s: &str) -> Result<Vec<u8>> {
        match self {
            StringEncoding::Ascii => {
                let mut bytes = Vec::with_capacity(s.len());
                for c in s.chars() {
                    let b = c as u32;
                    if b > 0x7F {
                        return Err(ConstructError::Generic {
                            path: String::new(),
                            message: format!(
                                "ASCII encode failed: character '{}' (U+{:04X}) is not ASCII",
                                c, b
                            ),
                        });
                    }
                    bytes.push(b as u8);
                }
                Ok(bytes)
            }
            StringEncoding::Utf8 => Ok(s.as_bytes().to_vec()),
            StringEncoding::Utf16 => Ok(encode_utf16(s, cfg!(target_endian = "little"))),
            StringEncoding::Utf16Be => Ok(encode_utf16(s, false)),
            StringEncoding::Utf16Le => Ok(encode_utf16(s, true)),
            StringEncoding::Utf32 => Ok(encode_utf32(s, cfg!(target_endian = "little"))),
            StringEncoding::Utf32Be => Ok(encode_utf32(s, false)),
            StringEncoding::Utf32Le => Ok(encode_utf32(s, true)),
        }
    }
}

/// Decodes a UTF-16 byte sequence into a `String`.
///
/// `little_endian` selects the byte order of each 16-bit code unit.
fn decode_utf16(data: &[u8], little_endian: bool) -> Result<String> {
    let mut code_units: Vec<u16> = Vec::with_capacity(data.len() / 2);
    for chunk in data.chunks_exact(2) {
        let unit = if little_endian {
            u16::from_le_bytes([chunk[0], chunk[1]])
        } else {
            u16::from_be_bytes([chunk[0], chunk[1]])
        };
        code_units.push(unit);
    }
    String::from_utf16(&code_units).map_err(|e| ConstructError::Generic {
        path: String::new(),
        message: format!("UTF-16 decode failed: {e}"),
    })
}

/// Encodes a string into UTF-16 bytes.
fn encode_utf16(s: &str, little_endian: bool) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(s.len() * 2);
    for unit in s.encode_utf16() {
        if little_endian {
            bytes.extend_from_slice(&unit.to_le_bytes());
        } else {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
    }
    bytes
}

/// Decodes a UTF-32 byte sequence into a `String`.
fn decode_utf32(data: &[u8], little_endian: bool) -> Result<String> {
    let mut chars = String::with_capacity(data.len() / 4);
    for chunk in data.chunks_exact(4) {
        let cp = if little_endian {
            u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
        } else {
            u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
        };
        let c = char::from_u32(cp).ok_or_else(|| ConstructError::Generic {
            path: String::new(),
            message: format!("UTF-32 decode failed: invalid code point U+{cp:04X}"),
        })?;
        chars.push(c);
    }
    Ok(chars)
}

/// Encodes a string into UTF-32 bytes.
fn encode_utf32(s: &str, little_endian: bool) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(s.len() * 4);
    for c in s.chars() {
        let cp = c as u32;
        if little_endian {
            bytes.extend_from_slice(&cp.to_le_bytes());
        } else {
            bytes.extend_from_slice(&cp.to_be_bytes());
        }
    }
    bytes
}

// ===========================================================================
// CString
// ===========================================================================

/// Error message used when EOF is reached before a null terminator.
const CSTRING_EOF_MESSAGE: &str = "CString: stream ended before null terminator";

/// Sizeof error reason for CString (variable length).
const CSTRING_SIZEOF_REASON: &str = "CString has variable size";

/// A null-terminated string construct.
///
/// - **parse**: reads encoding-sized units until a null terminator is found,
///   decodes the accumulated bytes into a [`Value::String`]
/// - **build**: encodes the [`Value::String`] into bytes, appends a null
///   terminator of the appropriate size
/// - **sizeof**: returns [`ConstructError::Sizeof`] (variable length)
///
/// Corresponds to Python `CString(encoding)`.
///
/// # Examples
///
/// ```
/// use construct::constructs::strings::{CString, StringEncoding};
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let d = CString::utf8();
/// let c: &dyn Construct = &d;
/// let parsed = c.parse_bytes(b"hello\x00world").unwrap();
/// assert_eq!(parsed, Value::String("hello".to_string()));
/// ```
pub struct CString {
    /// The character encoding.
    pub encoding: StringEncoding,
}

impl CString {
    /// Creates a new `CString` with the given encoding.
    #[must_use]
    pub const fn new(encoding: StringEncoding) -> Self {
        CString { encoding }
    }

    /// Creates a UTF-8 `CString`.
    #[must_use]
    pub const fn utf8() -> Self {
        Self::new(StringEncoding::Utf8)
    }

    /// Creates an ASCII `CString`.
    #[must_use]
    pub const fn ascii() -> Self {
        Self::new(StringEncoding::Ascii)
    }
}

impl Construct for CString {
    fn parse(&self, stream: &mut dyn Stream, _ctx: &mut Context) -> Result<Value> {
        let term_size = self.encoding.term_size();
        let mut data = Vec::new();
        loop {
            let chunk = stream.read_bytes(term_size).map_err(|e| match e {
                ConstructError::Stream { source, .. } => ConstructError::Generic {
                    path: String::new(),
                    message: format!("{}: {}", CSTRING_EOF_MESSAGE, source),
                },
                other => other,
            })?;
            // Check if this chunk is a null terminator (all zero bytes)
            if chunk.iter().all(|&b| b == 0) {
                break;
            }
            data.extend_from_slice(&chunk);
        }
        let s = self.encoding.decode(&data)?;
        Ok(Value::String(s))
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, _ctx: &mut Context) -> Result<()> {
        let s = data.as_string()?;
        let encoded = self.encoding.encode(s)?;
        stream.write_bytes(&encoded)?;
        let term = vec![0u8; self.encoding.term_size()];
        stream.write_bytes(&term)
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Err(ConstructError::Sizeof {
            path: String::new(),
            reason: CSTRING_SIZEOF_REASON.to_string(),
        })
    }
}

// ===========================================================================
// PaddedString
// ===========================================================================

/// Sizeof error reason for PaddedString when length is not a multiple of the
/// encoding unit.
fn padded_sizeof_message(length: usize, unit: usize) -> String {
    format!(
        "PaddedString length {} is not a multiple of encoding unit size {}",
        length, unit
    )
}

/// A fixed-length string padded with null bytes.
///
/// - **parse**: reads exactly `length` bytes, strips trailing null bytes
///   (aligned to the encoding unit), decodes into a [`Value::String`]
/// - **build**: encodes the [`Value::String`], pads with null bytes to
///   `length`. If the encoded length exceeds `length`, returns
///   [`ConstructError::Padding`].
/// - **sizeof**: returns `length`
///
/// Corresponds to Python `PaddedString(length, encoding)`.
///
/// # Examples
///
/// ```
/// use construct::constructs::strings::{PaddedString, StringEncoding};
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let d = PaddedString::utf8(10);
/// let c: &dyn Construct = &d;
/// let built = c.build_bytes(&Value::String("hello".to_string())).unwrap();
/// assert_eq!(built, b"hello\0\0\0\0\0");
/// ```
pub struct PaddedString {
    /// The fixed total length in bytes.
    pub length: usize,
    /// The character encoding.
    pub encoding: StringEncoding,
}

impl PaddedString {
    /// Creates a new `PaddedString` with the given length and encoding.
    #[must_use]
    pub const fn new(length: usize, encoding: StringEncoding) -> Self {
        PaddedString { length, encoding }
    }

    /// Creates a UTF-8 `PaddedString` of the given length.
    #[must_use]
    pub const fn utf8(length: usize) -> Self {
        Self::new(length, StringEncoding::Utf8)
    }

    /// Creates an ASCII `PaddedString` of the given length.
    #[must_use]
    pub const fn ascii(length: usize) -> Self {
        Self::new(length, StringEncoding::Ascii)
    }
}

impl Construct for PaddedString {
    fn parse(&self, stream: &mut dyn Stream, _ctx: &mut Context) -> Result<Value> {
        let data = stream.read_bytes(self.length)?;
        // Strip trailing null bytes, aligned to the encoding unit size.
        let unit = self.encoding.term_size();
        let mut end = data.len();
        while end >= unit && data[end - unit..end].iter().all(|&b| b == 0) {
            end -= unit;
        }
        let stripped = &data[..end];
        let s = self.encoding.decode(stripped)?;
        Ok(Value::String(s))
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, _ctx: &mut Context) -> Result<()> {
        let s = data.as_string()?;
        let encoded = self.encoding.encode(s)?;
        if encoded.len() > self.length {
            return Err(ConstructError::Padding {
                path: String::new(),
                message: format!(
                    "encoded string is {} bytes but PaddedString length is {}",
                    encoded.len(),
                    self.length
                ),
            });
        }
        stream.write_bytes(&encoded)?;
        let pad = self.length - encoded.len();
        if pad > 0 {
            stream.write_bytes(&vec![0u8; pad])?;
        }
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        let unit = self.encoding.term_size();
        if self.length % unit != 0 {
            return Err(ConstructError::Sizeof {
                path: String::new(),
                reason: padded_sizeof_message(self.length, unit),
            });
        }
        Ok(self.length)
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stream::ByteStream;

    // ======================================================================
    // StringEncoding tests
    // ======================================================================

    #[test]
    fn encoding_term_size() {
        assert_eq!(StringEncoding::Ascii.term_size(), 1);
        assert_eq!(StringEncoding::Utf8.term_size(), 1);
        assert_eq!(StringEncoding::Utf16.term_size(), 2);
        assert_eq!(StringEncoding::Utf16Be.term_size(), 2);
        assert_eq!(StringEncoding::Utf32.term_size(), 4);
        assert_eq!(StringEncoding::Utf32Le.term_size(), 4);
    }

    #[test]
    fn encoding_utf8_roundtrip() {
        let enc = StringEncoding::Utf8;
        let original = "héllo世界";
        let bytes = enc.encode(original).unwrap();
        let decoded = enc.decode(&bytes).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn encoding_ascii_encode_rejects_non_ascii() {
        let enc = StringEncoding::Ascii;
        let result = enc.encode("café");
        assert!(result.is_err());
    }

    #[test]
    fn encoding_ascii_decode_rejects_non_ascii() {
        let enc = StringEncoding::Ascii;
        let result = enc.decode(&[0xFF]);
        assert!(result.is_err());
    }

    #[test]
    fn encoding_utf16be_roundtrip() {
        let enc = StringEncoding::Utf16Be;
        let original = "ABC";
        let bytes = enc.encode(original).unwrap();
        assert_eq!(bytes, vec![0x00, b'A', 0x00, b'B', 0x00, b'C']);
        let decoded = enc.decode(&bytes).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn encoding_utf16le_roundtrip() {
        let enc = StringEncoding::Utf16Le;
        let original = "ABC";
        let bytes = enc.encode(original).unwrap();
        assert_eq!(bytes, vec![b'A', 0x00, b'B', 0x00, b'C', 0x00]);
        let decoded = enc.decode(&bytes).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn encoding_utf32be_roundtrip() {
        let enc = StringEncoding::Utf32Be;
        let original = "AB";
        let bytes = enc.encode(original).unwrap();
        assert_eq!(bytes, vec![0, 0, 0, b'A', 0, 0, 0, b'B']);
        let decoded = enc.decode(&bytes).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn encoding_utf16_decode_odd_length_error() {
        let enc = StringEncoding::Utf16Be;
        let result = enc.decode(&[0x00]);
        assert!(result.is_err());
    }

    #[test]
    fn encoding_utf32_decode_bad_length_error() {
        let enc = StringEncoding::Utf32Be;
        let result = enc.decode(&[0, 0, 0]);
        assert!(result.is_err());
    }

    // ======================================================================
    // CString tests
    // ======================================================================

    #[test]
    fn cstring_parse_basic_utf8() {
        let d = CString::utf8();
        let mut stream = ByteStream::new_read(b"hello\x00world");
        let mut ctx = Context::new();
        let result = d.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::String("hello".to_string()));
    }

    #[test]
    fn cstring_parse_empty_string() {
        let d = CString::utf8();
        let mut stream = ByteStream::new_read(b"\x00data");
        let mut ctx = Context::new();
        let result = d.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::String("".to_string()));
    }

    #[test]
    fn cstring_parse_eof_before_terminator_returns_error() {
        let d = CString::utf8();
        let mut stream = ByteStream::new_read(b"hello");
        let mut ctx = Context::new();
        let err = d.parse(&mut stream, &mut ctx).unwrap_err();
        assert!(err.to_string().contains("ended before null terminator"));
    }

    #[test]
    fn cstring_build_basic_utf8() {
        let d = CString::utf8();
        let mut stream = ByteStream::new_write();
        let mut ctx = Context::new();
        d.build(&Value::String("hello".to_string()), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), b"hello\x00");
    }

    #[test]
    fn cstring_build_empty_string() {
        let d = CString::utf8();
        let mut stream = ByteStream::new_write();
        let mut ctx = Context::new();
        d.build(&Value::String("".to_string()), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), b"\x00");
    }

    #[test]
    fn cstring_build_non_string_returns_error() {
        let d = CString::utf8();
        let mut stream = ByteStream::new_write();
        let mut ctx = Context::new();
        let err = d
            .build(&Value::UInt(42), &mut stream, &mut ctx)
            .unwrap_err();
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
    }

    #[test]
    fn cstring_sizeof_returns_error() {
        let d = CString::utf8();
        let err = d.sizeof(&Context::new()).unwrap_err();
        assert!(matches!(err, ConstructError::Sizeof { .. }));
    }

    #[test]
    fn cstring_roundtrip_utf8() {
        let d = CString::utf8();
        let c: &dyn Construct = &d;
        let original = Value::String("héllo".to_string());
        let bytes = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn cstring_parse_utf16be() {
        let d = CString::new(StringEncoding::Utf16Be);
        // "AB\0" in UTF-16 BE: 00 41 00 42 00 00
        let mut stream = ByteStream::new_read(&[0x00, 0x41, 0x00, 0x42, 0x00, 0x00]);
        let mut ctx = Context::new();
        let result = d.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::String("AB".to_string()));
    }

    // ======================================================================
    // PaddedString tests
    // ======================================================================

    #[test]
    fn paddedstring_parse_strips_trailing_zeros() {
        let d = PaddedString::utf8(10);
        let mut stream = ByteStream::new_read(b"hello\0\0\0\0\0");
        let mut ctx = Context::new();
        let result = d.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::String("hello".to_string()));
    }

    #[test]
    fn paddedstring_parse_exact_length() {
        let d = PaddedString::utf8(5);
        let mut stream = ByteStream::new_read(b"hello");
        let mut ctx = Context::new();
        let result = d.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::String("hello".to_string()));
    }

    #[test]
    fn paddedstring_parse_all_padding() {
        let d = PaddedString::utf8(4);
        let mut stream = ByteStream::new_read(b"\0\0\0\0");
        let mut ctx = Context::new();
        let result = d.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::String("".to_string()));
    }

    #[test]
    fn paddedstring_build_pads_to_length() {
        let d = PaddedString::utf8(10);
        let mut stream = ByteStream::new_write();
        let mut ctx = Context::new();
        d.build(&Value::String("hello".to_string()), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), b"hello\0\0\0\0\0");
    }

    #[test]
    fn paddedstring_build_exact_length() {
        let d = PaddedString::utf8(5);
        let mut stream = ByteStream::new_write();
        let mut ctx = Context::new();
        d.build(&Value::String("hello".to_string()), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), b"hello");
    }

    #[test]
    fn paddedstring_build_too_long_returns_error() {
        let d = PaddedString::utf8(3);
        let mut stream = ByteStream::new_write();
        let mut ctx = Context::new();
        let err = d
            .build(&Value::String("hello".to_string()), &mut stream, &mut ctx)
            .unwrap_err();
        assert!(matches!(err, ConstructError::Padding { .. }));
    }

    #[test]
    fn paddedstring_build_non_string_returns_error() {
        let d = PaddedString::utf8(4);
        let mut stream = ByteStream::new_write();
        let mut ctx = Context::new();
        let err = d.build(&Value::Int(42), &mut stream, &mut ctx).unwrap_err();
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
    }

    #[test]
    fn paddedstring_sizeof_returns_length() {
        let d = PaddedString::utf8(10);
        assert_eq!(d.sizeof(&Context::new()).unwrap(), 10);
    }

    #[test]
    fn paddedstring_sizeof_misaligned_returns_error() {
        let d = PaddedString::new(3, StringEncoding::Utf16Be);
        let err = d.sizeof(&Context::new()).unwrap_err();
        assert!(matches!(err, ConstructError::Sizeof { .. }));
    }

    #[test]
    fn paddedstring_roundtrip_utf8() {
        let d = PaddedString::utf8(10);
        let c: &dyn Construct = &d;
        let original = Value::String("hi".to_string());
        let bytes = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn paddedstring_roundtrip_utf16be() {
        let d = PaddedString::new(8, StringEncoding::Utf16Be);
        let c: &dyn Construct = &d;
        let original = Value::String("AB".to_string());
        let bytes = c.build_bytes(&original).unwrap();
        assert_eq!(bytes.len(), 8);
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
    }
}
