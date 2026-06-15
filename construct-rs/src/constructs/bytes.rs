//! Bytes and GreedyBytes constructs — fixed-length and variable-length byte fields.
//!
//! - [`Bytes`] reads/writes a fixed number of bytes.
//! - [`GreedyBytes`] reads all remaining bytes from the stream.
//!
//! Corresponds to Python `Bytes(length)` and `GreedyBytes`.

use std::io::{self, ErrorKind};

use crate::binary;
use crate::core::context::Context;
use crate::core::error::{ConstructError, Result};
use crate::core::stream::CombinedStream;
use crate::core::stream::Stream;
use crate::core::Construct;
use crate::expr::CombinedExpr;
use crate::expr::Evaluate;
use crate::value::Value;

/// A construct that reads/writes a fixed number of bytes.
///
/// - **parse**: reads exactly `length` bytes from the stream → [`Value::Bytes`]
/// - **build**: writes [`Value::Bytes`] or expands an integer into bytes
/// - **sizeof**: returns `length`
///
/// Corresponds to Python `Bytes(length)`.
///
/// # Examples
///
/// ```
/// use construct::constructs::bytes::Bytes;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let b: &dyn Construct = &Bytes::new(4);
/// let parsed = b.parse_bytes(b"beef").unwrap();
/// assert_eq!(parsed, Value::Bytes(b"beef".to_vec()));
///
/// let built = b.build_bytes(&Value::Bytes(b"beef".to_vec())).unwrap();
/// assert_eq!(built, b"beef");
/// ```
pub struct Bytes {
    /// The number of bytes to read/write.
    pub length: usize,
}

impl Bytes {
    /// Creates a new `Bytes` construct that reads/writes exactly `length` bytes.
    pub fn new(length: usize) -> Self {
        Bytes { length }
    }
}

impl Construct for Bytes {
    fn parse(&self, stream: &mut CombinedStream, _ctx: &mut Context) -> Result<Value> {
        let data = stream.read_bytes(self.length)?;
        Ok(Value::Bytes(data))
    }

    fn build(&self, data: &Value, stream: &mut CombinedStream, _ctx: &mut Context) -> Result<()> {
        match data {
            Value::Bytes(bytes) => {
                if bytes.len() != self.length {
                    return Err(ConstructError::Stream {
                        path: String::new(),
                        source: io::Error::new(
                            ErrorKind::InvalidData,
                            format!("expected {} bytes but got {}", self.length, bytes.len()),
                        ),
                    });
                }
                stream.write_bytes(bytes)
            }
            Value::Int(i) => {
                let bytes = binary::integer2bytes(*i as i128, self.length, false)?;
                stream.write_bytes(&bytes)
            }
            Value::UInt(u) => {
                let bytes = binary::integer2bytes(*u as i128, self.length, false)?;
                stream.write_bytes(&bytes)
            }
            Value::BigInt(i) => {
                let bytes = binary::integer2bytes(*i, self.length, false)?;
                stream.write_bytes(&bytes)
            }
            other => Err(ConstructError::TypeMismatch {
                path: String::new(),
                expected: "Bytes or integer".to_string(),
                actual: other.type_name().to_string(),
            }),
        }
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Ok(self.length)
    }
}

/// A construct that reads all remaining bytes from the stream.
///
/// - **parse**: reads from the current position to the end → [`Value::Bytes`]
/// - **build**: writes [`Value::Bytes`]
/// - **sizeof**: returns [`ConstructError::Sizeof`] (size is undefined)
///
/// Corresponds to Python `GreedyBytes`.
///
/// # Examples
///
/// ```
/// use construct::constructs::bytes::GreedyBytes;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let c: &dyn Construct = &GreedyBytes;
/// let parsed = c.parse_bytes(b"asislight").unwrap();
/// assert_eq!(parsed, Value::Bytes(b"asislight".to_vec()));
/// ```
pub struct GreedyBytes;

impl GreedyBytes {
    /// Creates a new `GreedyBytes` construct.
    pub fn new() -> Self {
        GreedyBytes
    }
}

impl Default for GreedyBytes {
    fn default() -> Self {
        Self::new()
    }
}

impl Construct for GreedyBytes {
    fn parse(&self, stream: &mut CombinedStream, _ctx: &mut Context) -> Result<Value> {
        let data = stream.read_remaining()?;
        Ok(Value::Bytes(data))
    }

    fn build(&self, data: &Value, stream: &mut CombinedStream, _ctx: &mut Context) -> Result<()> {
        let bytes = data.as_bytes()?;
        stream.write_bytes(bytes)
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Err(ConstructError::Sizeof {
            path: String::new(),
            reason: "GreedyBytes has undefined size".to_string(),
        })
    }
}

// ===========================================================================
// BytesExpr — expression-length Bytes
// ===========================================================================

/// A [`Bytes`] whose length is computed at runtime from an expression.
///
/// - **parse**: evaluates `length_expr` to get the byte count, then reads
///   exactly that many bytes → [`Value::Bytes`]
/// - **build**: writes [`Value::Bytes`] whose length must match the evaluated
///   count; if the evaluated count is `0`, writes nothing
/// - **sizeof**: returns [`ConstructError::Sizeof`] (length is runtime-dependent)
///
/// Corresponds to Python `Bytes(this.xxx)` or `Bytes(lambda this: ...)`.
///
/// # Examples
///
/// ```
/// use construct::constructs::bytes::BytesExpr;
/// use construct::constructs::struct_::Struct;
/// use construct::constructs::format_field::INT8UB;
/// use construct::core::Construct;
/// use construct::value::Value;
/// use construct::expr::this_;
///
/// let d = Struct::new()
///     .field("len", Box::new(INT8UB.into()))
///     .field("data", Box::new(BytesExpr::new(
///         Box::new(this_().field("len").into()),
///     ).into()));
/// let c: &dyn Construct = &d;
/// let parsed = c.parse_bytes(b"\x03ABC").unwrap();
/// let container = parsed.as_container().unwrap();
/// assert_eq!(container.get("len").unwrap(), &Value::UInt(3));
/// assert_eq!(container.get("data").unwrap(), &Value::Bytes(b"ABC".to_vec()));
/// ```
pub struct BytesExpr {
    /// Expression that evaluates to the number of bytes to read/write.
    pub length_expr: Box<CombinedExpr>,
}

impl BytesExpr {
    /// Creates a new `BytesExpr` with the given length expression.
    pub fn new(length_expr: Box<CombinedExpr>) -> Self {
        BytesExpr { length_expr }
    }

    /// Evaluates the length expression and returns it as a `usize`.
    fn resolve_length(&self, ctx: &Context) -> Result<usize> {
        let val = self.length_expr.evaluate(ctx, None)?;
        let u = val.to_u64().map_err(|e| ConstructError::Expr {
            path: String::new(),
            message: format!("BytesExpr length must be a non-negative integer: {e}"),
        })?;
        Ok(u as usize)
    }
}

impl Construct for BytesExpr {
    fn parse(&self, stream: &mut CombinedStream, ctx: &mut Context) -> Result<Value> {
        let length = self.resolve_length(ctx)?;
        let data = stream.read_bytes(length)?;
        Ok(Value::Bytes(data))
    }

    fn build(&self, data: &Value, stream: &mut CombinedStream, ctx: &mut Context) -> Result<()> {
        let expected = self.resolve_length(ctx)?;
        let bytes = data.as_bytes().map_err(|_| ConstructError::TypeMismatch {
            path: String::new(),
            expected: "Bytes".to_string(),
            actual: data.type_name().to_string(),
        })?;

        if bytes.len() != expected {
            return Err(ConstructError::Stream {
                path: String::new(),
                source: io::Error::new(
                    ErrorKind::InvalidData,
                    format!("expected {expected} bytes but got {}", bytes.len()),
                ),
            });
        }

        if expected > 0 {
            stream.write_bytes(bytes)?;
        }
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Err(ConstructError::Sizeof {
            path: String::new(),
            reason: "BytesExpr has variable size (length is runtime-dependent)".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stream::ByteStream;

    // =========================================================================
    // Bytes tests
    // =========================================================================

    #[test]
    fn bytes_parse_basic() {
        let b = Bytes::new(4);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"beef"));
        let mut ctx = Context::new();
        let result = b.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::Bytes(b"beef".to_vec()));
    }

    #[test]
    fn bytes_build_bytes() {
        let b = Bytes::new(4);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        b.build(&Value::Bytes(b"beef".to_vec()), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), b"beef");
    }

    #[test]
    fn bytes_build_from_integer() {
        let b: &dyn Construct = &Bytes::new(4);
        let bytes = b.build_bytes(&Value::UInt(0)).unwrap();
        assert_eq!(bytes, vec![0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn bytes_build_from_int() {
        let b: &dyn Construct = &Bytes::new(4);
        let bytes = b.build_bytes(&Value::Int(1)).unwrap();
        assert_eq!(bytes, vec![0x00, 0x00, 0x00, 0x01]);
    }

    #[test]
    fn bytes_build_wrong_length() {
        let b = Bytes::new(4);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        let err = b
            .build(&Value::Bytes(b"hi".to_vec()), &mut stream, &mut ctx)
            .unwrap_err();
        assert!(matches!(err, ConstructError::Stream { .. }));
    }

    #[test]
    fn bytes_build_wrong_type() {
        let b = Bytes::new(4);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        let err = b
            .build(&Value::String("hi".to_string()), &mut stream, &mut ctx)
            .unwrap_err();
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
    }

    #[test]
    fn bytes_parse_insufficient_data() {
        let b = Bytes::new(4);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"ab"));
        let mut ctx = Context::new();
        let err = b.parse(&mut stream, &mut ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Stream { .. }));
    }

    #[test]
    fn bytes_sizeof() {
        let b = Bytes::new(4);
        let ctx = Context::new();
        assert_eq!(b.sizeof(&ctx).unwrap(), 4);
    }

    #[test]
    fn bytes_length_zero() {
        let b: &dyn Construct = &Bytes::new(0);
        let parsed = b.parse_bytes(b"anything").unwrap();
        assert_eq!(parsed, Value::Bytes(vec![]));

        let built = b.build_bytes(&Value::Bytes(vec![])).unwrap();
        assert!(built.is_empty());
    }

    #[test]
    fn bytes_roundtrip() {
        let b: &dyn Construct = &Bytes::new(5);
        let original = Value::Bytes(b"hello".to_vec());
        let bytes = b.build_bytes(&original).unwrap();
        let parsed = b.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn bytes_parse_bytes_convenience() {
        let b: &dyn Construct = &Bytes::new(3);
        let result = b.parse_bytes(b"\x01\x02\x03").unwrap();
        assert_eq!(result, Value::Bytes(vec![1, 2, 3]));
    }

    #[test]
    fn bytes_build_bytes_convenience() {
        let b: &dyn Construct = &Bytes::new(3);
        let bytes = b.build_bytes(&Value::Bytes(vec![1, 2, 3])).unwrap();
        assert_eq!(bytes, vec![1, 2, 3]);
    }

    // =========================================================================
    // GreedyBytes tests
    // =========================================================================

    #[test]
    fn greedy_bytes_parse_basic() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"asislight"));
        let mut ctx = Context::new();
        let result = GreedyBytes.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::Bytes(b"asislight".to_vec()));
    }

    #[test]
    fn greedy_bytes_parse_empty() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write()); // empty stream
        let mut ctx = Context::new();
        let result = GreedyBytes.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::Bytes(vec![]));
    }

    #[test]
    fn greedy_bytes_build_basic() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        GreedyBytes
            .build(&Value::Bytes(b"asislight".to_vec()), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), b"asislight");
    }

    #[test]
    fn greedy_bytes_build_wrong_type() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        let err = GreedyBytes
            .build(&Value::Int(42), &mut stream, &mut ctx)
            .unwrap_err();
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
    }

    #[test]
    fn greedy_bytes_sizeof_error() {
        let ctx = Context::new();
        let err = GreedyBytes.sizeof(&ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Sizeof { .. }));
    }

    #[test]
    fn greedy_bytes_roundtrip() {
        let c: &dyn Construct = &GreedyBytes;
        let original = Value::Bytes(b"test data".to_vec());
        let bytes = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn greedy_bytes_parse_partial() {
        // GreedyBytes with data remaining in a larger stream
        let data = b"\x01\x02\x03\x04\x05";
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(data));
        // Read 2 bytes first, then GreedyBytes should get the rest
        let _first = stream.read_bytes(2).unwrap();
        let mut ctx = Context::new();
        let result = GreedyBytes.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::Bytes(vec![3, 4, 5]));
    }

    #[test]
    fn greedy_bytes_new_and_default() {
        let _ = GreedyBytes::new();
        let _ = GreedyBytes;
    }
}
