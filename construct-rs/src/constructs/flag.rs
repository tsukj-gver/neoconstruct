//! Flag construct — a single-byte boolean value.
//!
//! [`Flag`] reads one byte and returns `true` for any non-zero value,
//! `false` for zero. On build, it writes `0x01` for `true` and `0x00` for
//! `false`.
//!
//! Corresponds to Python `Flag`.

use crate::core::context::Context;
use crate::core::error::Result;
use crate::core::stream::CombinedStream;
use crate::core::stream::Stream;
use crate::core::Construct;
use crate::value::Value;

/// Truthy byte written when building `true`.
const FLAG_TRUE: u8 = 0x01;
/// Falsy byte written when building `false`.
const FLAG_FALSE: u8 = 0x00;

/// A single-byte boolean construct.
///
/// - **parse**: reads 1 byte → `true` if non-zero, `false` if `0x00`
/// - **build**: writes `0x01` for `true`, `0x00` for `false`
/// - **sizeof**: always returns `1`
///
/// Corresponds to Python `Flag`.
///
/// # Examples
///
/// ```
/// use construct::constructs::flag::Flag;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let f: &dyn Construct = &Flag::new();
/// assert_eq!(f.parse_bytes(b"\x01").unwrap(), Value::Bool(true));
/// assert_eq!(f.parse_bytes(b"\x00").unwrap(), Value::Bool(false));
/// assert_eq!(f.parse_bytes(b"\xFF").unwrap(), Value::Bool(true));
///
/// let built = f.build_bytes(&Value::Bool(true)).unwrap();
/// assert_eq!(built, vec![0x01]);
/// ```
pub struct Flag;

impl Flag {
    /// Creates a new `Flag` construct.
    pub fn new() -> Self {
        Flag
    }
}

impl Default for Flag {
    fn default() -> Self {
        Self::new()
    }
}

impl Construct for Flag {
    fn parse(&self, stream: &mut CombinedStream, _ctx: &mut Context) -> Result<Value> {
        let data = stream.read_bytes(1)?;
        Ok(Value::Bool(data[0] != FLAG_FALSE))
    }

    fn build(&self, data: &Value, stream: &mut CombinedStream, _ctx: &mut Context) -> Result<()> {
        let flag = data.as_bool().map_err(|e| e.with_path_prefix("Flag"))?;
        let byte = if flag { FLAG_TRUE } else { FLAG_FALSE };
        stream.write_bytes(&[byte])?;
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Ok(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::error::ConstructError;
    use crate::core::stream::ByteStream;

    // -- Parse tests ---------------------------------------------------------

    #[test]
    fn parse_zero_returns_false() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\x00"));
        let mut ctx = Context::new();
        let result = Flag.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn parse_one_returns_true() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\x01"));
        let mut ctx = Context::new();
        let result = Flag.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn parse_arbitrary_nonzero_returns_true() {
        for byte in [0x02, 0x7F, 0x80, 0xFF] {
            let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[byte]));
            let mut ctx = Context::new();
            let result = Flag.parse(&mut stream, &mut ctx).unwrap();
            assert_eq!(result, Value::Bool(true), "byte {byte:#x} should be true");
        }
    }

    #[test]
    fn parse_insufficient_data_returns_stream_error() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b""));
        let mut ctx = Context::new();
        let err = Flag.parse(&mut stream, &mut ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Stream { .. }));
    }

    // -- Build tests ---------------------------------------------------------

    #[test]
    fn build_true_writes_0x01() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        Flag.build(&Value::Bool(true), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), vec![FLAG_TRUE]);
    }

    #[test]
    fn build_false_writes_0x00() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        Flag.build(&Value::Bool(false), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), vec![FLAG_FALSE]);
    }

    #[test]
    fn build_non_bool_returns_type_mismatch() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        let err = Flag
            .build(&Value::Int(1), &mut stream, &mut ctx)
            .unwrap_err();
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
    }

    // -- Sizeof tests --------------------------------------------------------

    #[test]
    fn sizeof_returns_1() {
        let ctx = Context::new();
        assert_eq!(Flag.sizeof(&ctx).unwrap(), 1);
    }

    // -- Roundtrip tests -----------------------------------------------------

    #[test]
    fn roundtrip_true() {
        let c: &dyn Construct = &Flag;
        let bytes = c.build_bytes(&Value::Bool(true)).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, Value::Bool(true));
    }

    #[test]
    fn roundtrip_false() {
        let c: &dyn Construct = &Flag;
        let bytes = c.build_bytes(&Value::Bool(false)).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, Value::Bool(false));
    }
}
