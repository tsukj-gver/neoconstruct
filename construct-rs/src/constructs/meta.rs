//! Meta constructs — Pass, Terminated, Tell, Seek, and Error.
//!
//! These are utility constructs that manipulate stream state or control flow
//! rather than reading/writing structured data.
//!
//! | Construct | Purpose |
//! |-----------|---------|
//! | [`Pass`] | No-op, returns `None` |
//! | [`Terminated`] | Asserts the stream is at EOF |
//! | [`Tell`] | Returns the current stream position |
//! | [`Seek`] | Seeks the stream to a given offset |
//! | [`Error`] | Unconditionally returns an error |

use std::io::SeekFrom;

use crate::core::context::Context;
use crate::core::error::{ConstructError, Result};
use crate::core::stream::Stream;
use crate::core::Construct;
use crate::expr::Evaluate;
use crate::value::Value;

// ===========================================================================
// Pass
// ===========================================================================

/// A no-op construct.
///
/// - **parse**: returns [`Value::None`] without reading anything
/// - **build**: does nothing
/// - **sizeof**: returns `0`
///
/// Useful as a default case for `Switch` and `Enum`.
///
/// Corresponds to Python `Pass`.
///
/// # Examples
///
/// ```
/// use construct::constructs::meta::Pass;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let p: &dyn Construct = &Pass::new();
/// assert_eq!(p.parse_bytes(b"").unwrap(), Value::None);
/// let built = p.build_bytes(&Value::None).unwrap();
/// assert!(built.is_empty());
/// assert_eq!(p.sizeof(&Default::default()).unwrap(), 0);
/// ```
pub struct Pass;

impl Pass {
    /// Creates a new `Pass` construct.
    pub fn new() -> Self {
        Pass
    }
}

impl Default for Pass {
    fn default() -> Self {
        Self::new()
    }
}

impl Construct for Pass {
    fn parse(&self, _stream: &mut dyn Stream, _ctx: &mut Context) -> Result<Value> {
        Ok(Value::None)
    }

    fn build(&self, _data: &Value, _stream: &mut dyn Stream, _ctx: &mut Context) -> Result<()> {
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Ok(0)
    }

    fn flagbuildnone(&self) -> bool {
        true
    }
}

// ===========================================================================
// Terminated
// ===========================================================================

/// Asserts that the stream has reached end-of-file.
///
/// - **parse**: succeeds if the stream is at EOF, returns [`Value::None`];
///   fails with [`ConstructError::Terminated`] otherwise
/// - **build**: does nothing
/// - **sizeof**: returns [`ConstructError::Sizeof`]
///
/// Corresponds to Python `Terminated`.
///
/// # Examples
///
/// ```
/// use construct::constructs::meta::Terminated;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let t: &dyn Construct = &Terminated::new();
/// assert_eq!(t.parse_bytes(b"").unwrap(), Value::None);
/// ```
pub struct Terminated;

impl Terminated {
    /// Creates a new `Terminated` construct.
    pub fn new() -> Self {
        Terminated
    }
}

impl Default for Terminated {
    fn default() -> Self {
        Self::new()
    }
}

impl Construct for Terminated {
    fn parse(&self, stream: &mut dyn Stream, _ctx: &mut Context) -> Result<Value> {
        if stream.is_eof()? {
            Ok(Value::None)
        } else {
            let pos = stream.tell()?;
            let total = stream.size()?;
            let remaining = total.saturating_sub(pos) as usize;
            Err(ConstructError::Terminated {
                path: String::new(),
                remaining,
            })
        }
    }

    fn build(&self, _data: &Value, _stream: &mut dyn Stream, _ctx: &mut Context) -> Result<()> {
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Err(ConstructError::Sizeof {
            path: String::new(),
            reason: "Terminated has undefined size".to_string(),
        })
    }

    fn flagbuildnone(&self) -> bool {
        true
    }
}

// ===========================================================================
// Tell
// ===========================================================================

/// Returns the current stream position.
///
/// - **parse**: returns the current cursor position as [`Value::UInt`]
/// - **build**: returns the current cursor position as [`Value::UInt`]
/// - **sizeof**: returns `0`
///
/// Does not consume or produce any bytes in the stream.
///
/// Corresponds to Python `Tell`.
///
/// # Examples
///
/// ```
/// use construct::constructs::meta::Tell;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let t: &dyn Construct = &Tell::new();
/// assert_eq!(t.parse_bytes(b"").unwrap(), Value::UInt(0));
/// ```
pub struct Tell;

impl Tell {
    /// Creates a new `Tell` construct.
    pub fn new() -> Self {
        Tell
    }
}

impl Default for Tell {
    fn default() -> Self {
        Self::new()
    }
}

impl Construct for Tell {
    fn parse(&self, stream: &mut dyn Stream, _ctx: &mut Context) -> Result<Value> {
        let pos = stream.tell()?;
        Ok(Value::UInt(pos))
    }

    fn build(&self, _data: &Value, _stream: &mut dyn Stream, _ctx: &mut Context) -> Result<()> {
        // Tell does not write anything; it only reports the position.
        // In Python, _build returns stream.tell(), but the Rust build
        // signature writes nothing and returns ().
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Ok(0)
    }

    fn flagbuildnone(&self) -> bool {
        true
    }
}

// ===========================================================================
// SeekWhence
// ===========================================================================

/// Offset origin for [`Seek`].
///
/// Corresponds to the `whence` parameter of Python's `stream.seek(offset, whence)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeekWhence {
    /// Offset from the start of the stream (Python `whence=0`).
    Start,
    /// Offset from the current position (Python `whence=1`).
    Current,
    /// Offset from the end of the stream (Python `whence=2`).
    End,
}

impl SeekWhence {
    /// Converts this enum into the equivalent [`SeekFrom`] for use with
    /// [`Stream::seek_from`].
    fn to_seek_from(self, offset: i64) -> SeekFrom {
        match self {
            SeekWhence::Start => SeekFrom::Start(offset as u64),
            SeekWhence::Current => SeekFrom::Current(offset),
            SeekWhence::End => SeekFrom::End(offset),
        }
    }
}

// ===========================================================================
// Seek
// ===========================================================================

/// Seeks the stream to a specified position.
///
/// - **parse**: seeks and returns the new position as [`Value::UInt`]
/// - **build**: seeks and returns the new position as [`Value::UInt`]
/// - **sizeof**: returns [`ConstructError::Sizeof`]
///
/// Corresponds to Python `Seek(at, whence)`.
///
/// # Examples
///
/// ```
/// use construct::constructs::meta::{Seek, SeekWhence};
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let s: &dyn Construct = &Seek::new(5);
/// let parsed = s.parse_bytes(b"01234x").unwrap();
/// assert_eq!(parsed, Value::UInt(5));
/// ```
pub struct Seek {
    /// The offset to seek to.
    pub at: i64,
    /// The origin from which the offset is measured.
    pub whence: SeekWhence,
}

impl Seek {
    /// Creates a new `Seek` construct that seeks to an absolute position from
    /// the start of the stream.
    pub fn new(at: i64) -> Self {
        Seek {
            at,
            whence: SeekWhence::Start,
        }
    }

    /// Creates a new `Seek` construct with a custom `whence` origin.
    pub fn with_whence(at: i64, whence: SeekWhence) -> Self {
        Seek { at, whence }
    }
}

impl Construct for Seek {
    fn parse(&self, stream: &mut dyn Stream, _ctx: &mut Context) -> Result<Value> {
        let new_pos = stream.seek_from(self.whence.to_seek_from(self.at))?;
        Ok(Value::UInt(new_pos))
    }

    fn build(&self, _data: &Value, stream: &mut dyn Stream, _ctx: &mut Context) -> Result<()> {
        stream.seek_from(self.whence.to_seek_from(self.at))?;
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Err(ConstructError::Sizeof {
            path: String::new(),
            reason: "Seek only moves the stream, size is not meaningful".to_string(),
        })
    }

    fn flagbuildnone(&self) -> bool {
        true
    }
}

// ===========================================================================
// Error
// ===========================================================================

/// Unconditionally returns an error.
///
/// - **parse**: always returns [`ConstructError::Check`]
/// - **build**: always returns [`ConstructError::Check`]
/// - **sizeof**: always returns [`ConstructError::Sizeof`]
///
/// Corresponds to Python `Error`. The Python version raises `ExplicitError`;
/// Rust uses [`ConstructError::Check`] as the closest semantic equivalent.
///
/// # Examples
///
/// ```
/// use construct::constructs::meta::Error as ErrorConstruct;
/// use construct::core::Construct;
/// use construct::core::error::ConstructError;
///
/// let e: &dyn Construct = &ErrorConstruct::new();
/// assert!(matches!(e.parse_bytes(b"").unwrap_err(), ConstructError::Check { .. }));
/// ```
pub struct Error;

impl Error {
    /// Creates a new `Error` construct.
    pub fn new() -> Self {
        Error
    }
}

impl Default for Error {
    fn default() -> Self {
        Self::new()
    }
}

/// Error message returned when the Error construct is triggered during parsing.
const ERROR_PARSE_MESSAGE: &str = "Error field was activated during parsing";
/// Error message returned when the Error construct is triggered during building.
const ERROR_BUILD_MESSAGE: &str = "Error field was activated during building";
/// Reason returned when sizeof is called on the Error construct.
const ERROR_SIZEOF_REASON: &str =
    "Error does not have size, because it interrupts parsing and building";

impl Construct for Error {
    fn parse(&self, _stream: &mut dyn Stream, _ctx: &mut Context) -> Result<Value> {
        Err(ConstructError::Check {
            path: String::new(),
            message: ERROR_PARSE_MESSAGE.to_string(),
        })
    }

    fn build(&self, _data: &Value, _stream: &mut dyn Stream, _ctx: &mut Context) -> Result<()> {
        Err(ConstructError::Check {
            path: String::new(),
            message: ERROR_BUILD_MESSAGE.to_string(),
        })
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Err(ConstructError::Sizeof {
            path: String::new(),
            reason: ERROR_SIZEOF_REASON.to_string(),
        })
    }
}

// ===========================================================================
// SeekExpr
// ===========================================================================

/// A [`Seek`] whose target position is computed at runtime from an expression.
///
/// - **parse**: evaluates `at_expr`, seeks to the resolved position, returns
///   the new position as [`Value::UInt`]
/// - **build**: evaluates `at_expr`, seeks to the resolved position
/// - **sizeof**: returns [`ConstructError::Sizeof`]
///
/// Corresponds to Python `Seek(this.xxx)`.
///
/// # Examples
///
/// ```
/// use construct::constructs::meta::SeekExpr;
/// use construct::core::Construct;
/// use construct::value::Value;
/// use construct::expr::this_;
///
/// let d = SeekExpr::new(Box::new(this_().field("pos")));
/// let c: &dyn Construct = &d;
///
/// let mut stream = construct::core::stream::ByteStream::new_read(b"0123456789");
/// let mut ctx = construct::core::context::Context::new();
/// ctx.insert("pos", Value::UInt(3));
/// let parsed = c.parse(&mut stream, &mut ctx).unwrap();
/// assert_eq!(parsed, Value::UInt(3));
/// ```
pub struct SeekExpr {
    /// Expression that evaluates to the seek offset.
    pub at_expr: Box<dyn Evaluate>,
    /// Origin from which the offset is measured.
    pub whence: SeekWhence,
}

impl SeekExpr {
    /// Creates a new `SeekExpr` that seeks to an absolute position from the
    /// start of the stream.
    pub fn new(at_expr: Box<dyn Evaluate>) -> Self {
        SeekExpr {
            at_expr,
            whence: SeekWhence::Start,
        }
    }

    /// Creates a new `SeekExpr` with a custom `whence` origin.
    pub fn with_whence(at_expr: Box<dyn Evaluate>, whence: SeekWhence) -> Self {
        SeekExpr { at_expr, whence }
    }

    /// Resolves the offset from the expression and returns it as `i64`.
    fn resolve_offset(&self, ctx: &Context) -> Result<i64> {
        let val = self.at_expr.evaluate(ctx, None)?;
        val.to_i64().map_err(|e| ConstructError::Expr {
            path: String::new(),
            message: format!("SeekExpr offset must be an integer: {e}"),
        })
    }
}

impl Construct for SeekExpr {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let offset = self.resolve_offset(ctx)?;
        let new_pos = stream.seek_from(self.whence.to_seek_from(offset))?;
        Ok(Value::UInt(new_pos))
    }

    fn build(&self, _data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        let offset = self.resolve_offset(ctx)?;
        stream.seek_from(self.whence.to_seek_from(offset))?;
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Err(ConstructError::Sizeof {
            path: String::new(),
            reason: "SeekExpr only moves the stream, size is not meaningful".to_string(),
        })
    }

    fn flagbuildnone(&self) -> bool {
        true
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::error::ConstructError;
    use crate::core::stream::ByteStream;

    // ======================================================================
    // Pass tests
    // ======================================================================

    #[test]
    fn pass_parse_returns_none() {
        let mut stream = ByteStream::new_read(b"");
        let mut ctx = Context::new();
        let result = Pass.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::None);
    }

    #[test]
    fn pass_parse_does_not_consume_data() {
        let mut stream = ByteStream::new_read(b"\x01\x02");
        let mut ctx = Context::new();
        let result = Pass.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::None);
        assert_eq!(stream.tell().unwrap(), 0);
    }

    #[test]
    fn pass_build_does_nothing() {
        let mut stream = ByteStream::new_write();
        let mut ctx = Context::new();
        Pass.build(&Value::None, &mut stream, &mut ctx).unwrap();
        assert!(stream.into_bytes().is_empty());
    }

    #[test]
    fn pass_sizeof_returns_0() {
        let ctx = Context::new();
        assert_eq!(Pass.sizeof(&ctx).unwrap(), 0);
    }

    #[test]
    fn pass_roundtrip() {
        let c: &dyn Construct = &Pass;
        let built = c.build_bytes(&Value::None).unwrap();
        assert!(built.is_empty());
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, Value::None);
    }

    // ======================================================================
    // Terminated tests
    // ======================================================================

    #[test]
    fn terminated_parse_at_eof_returns_none() {
        let mut stream = ByteStream::new_read(b"");
        let mut ctx = Context::new();
        let result = Terminated.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::None);
    }

    #[test]
    fn terminated_parse_after_reading_all_returns_none() {
        let mut stream = ByteStream::new_read(b"\x01");
        let mut ctx = Context::new();
        let _ = stream.read_bytes(1).unwrap();
        let result = Terminated.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::None);
    }

    #[test]
    fn terminated_parse_not_at_eof_returns_terminated_error() {
        let mut stream = ByteStream::new_read(b"\x01\x02\x03");
        let mut ctx = Context::new();
        let err = Terminated.parse(&mut stream, &mut ctx).unwrap_err();
        match err {
            ConstructError::Terminated { remaining, .. } => {
                assert_eq!(remaining, 3);
            }
            other => panic!("expected Terminated error, got {:?}", other),
        }
    }

    #[test]
    fn terminated_parse_partial_read_shows_correct_remaining() {
        let mut stream = ByteStream::new_read(b"\x01\x02\x03\x04\x05");
        let mut ctx = Context::new();
        let _ = stream.read_bytes(2).unwrap();
        let err = Terminated.parse(&mut stream, &mut ctx).unwrap_err();
        match err {
            ConstructError::Terminated { remaining, .. } => {
                assert_eq!(remaining, 3);
            }
            other => panic!("expected Terminated error, got {:?}", other),
        }
    }

    #[test]
    fn terminated_build_does_nothing() {
        let mut stream = ByteStream::new_write();
        let mut ctx = Context::new();
        Terminated
            .build(&Value::None, &mut stream, &mut ctx)
            .unwrap();
        assert!(stream.into_bytes().is_empty());
    }

    #[test]
    fn terminated_sizeof_returns_error() {
        let ctx = Context::new();
        let err = Terminated.sizeof(&ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Sizeof { .. }));
    }

    // ======================================================================
    // Tell tests
    // ======================================================================

    #[test]
    fn tell_parse_at_start_returns_0() {
        let mut stream = ByteStream::new_read(b"\x01\x02");
        let mut ctx = Context::new();
        let result = Tell.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::UInt(0));
    }

    #[test]
    fn tell_parse_after_seek_returns_position() {
        let mut stream = ByteStream::new_read(b"\x01\x02\x03\x04");
        stream.seek(3).unwrap();
        let mut ctx = Context::new();
        let result = Tell.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::UInt(3));
    }

    #[test]
    fn tell_parse_on_empty_stream_returns_0() {
        let mut stream = ByteStream::new_read(b"");
        let mut ctx = Context::new();
        let result = Tell.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::UInt(0));
    }

    #[test]
    fn tell_parse_does_not_advance_position() {
        let mut stream = ByteStream::new_read(b"\x01\x02\x03");
        let mut ctx = Context::new();
        let _ = Tell.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(stream.tell().unwrap(), 0);
    }

    #[test]
    fn tell_build_does_not_write() {
        let mut stream = ByteStream::new_write();
        let mut ctx = Context::new();
        Tell.build(&Value::None, &mut stream, &mut ctx).unwrap();
        assert!(stream.into_bytes().is_empty());
    }

    #[test]
    fn tell_sizeof_returns_0() {
        let ctx = Context::new();
        assert_eq!(Tell.sizeof(&ctx).unwrap(), 0);
    }

    // ======================================================================
    // SeekWhence tests
    // ======================================================================

    #[test]
    fn seek_whence_start_converts_correctly() {
        let sf = SeekWhence::Start.to_seek_from(10);
        assert_eq!(sf, SeekFrom::Start(10));
    }

    #[test]
    fn seek_whence_current_converts_correctly() {
        let sf = SeekWhence::Current.to_seek_from(-3);
        assert_eq!(sf, SeekFrom::Current(-3));
    }

    #[test]
    fn seek_whence_end_converts_correctly() {
        let sf = SeekWhence::End.to_seek_from(-2);
        assert_eq!(sf, SeekFrom::End(-2));
    }

    // ======================================================================
    // Seek tests
    // ======================================================================

    #[test]
    fn seek_parse_from_start() {
        let s = Seek::new(5);
        let mut stream = ByteStream::new_read(b"01234x");
        let mut ctx = Context::new();
        let result = s.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::UInt(5));
        assert_eq!(stream.tell().unwrap(), 5);
    }

    #[test]
    fn seek_parse_from_current() {
        let s = Seek::with_whence(2, SeekWhence::Current);
        let mut stream = ByteStream::new_read(b"012345");
        stream.seek(1).unwrap();
        let mut ctx = Context::new();
        let result = s.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::UInt(3));
    }

    #[test]
    fn seek_parse_from_end() {
        let s = Seek::with_whence(-1, SeekWhence::End);
        let mut stream = ByteStream::new_read(b"01234");
        let mut ctx = Context::new();
        let result = s.parse(&mut stream, &mut ctx).unwrap();
        // size=5, seek from end -1 → position 4
        assert_eq!(result, Value::UInt(4));
    }

    #[test]
    fn seek_build_from_start() {
        let s = Seek::new(5);
        let mut stream = ByteStream::new_write();
        stream.write_bytes(b"0123456789").unwrap();
        let mut ctx = Context::new();
        s.build(&Value::None, &mut stream, &mut ctx).unwrap();
        assert_eq!(stream.tell().unwrap(), 5);
    }

    #[test]
    fn seek_sizeof_returns_error() {
        let s = Seek::new(0);
        let ctx = Context::new();
        let err = s.sizeof(&ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Sizeof { .. }));
    }

    #[test]
    fn seek_new_default_whence_is_start() {
        let s = Seek::new(10);
        assert_eq!(s.whence, SeekWhence::Start);
        assert_eq!(s.at, 10);
    }

    #[test]
    fn seek_with_whence_custom_whence() {
        let s = Seek::with_whence(-2, SeekWhence::End);
        assert_eq!(s.whence, SeekWhence::End);
        assert_eq!(s.at, -2);
    }

    // ======================================================================
    // Error tests
    // ======================================================================

    #[test]
    fn error_parse_returns_check_error() {
        let mut stream = ByteStream::new_read(b"");
        let mut ctx = Context::new();
        let err = Error.parse(&mut stream, &mut ctx).unwrap_err();
        match err {
            ConstructError::Check { message, .. } => {
                assert_eq!(message, ERROR_PARSE_MESSAGE);
            }
            other => panic!("expected Check error, got {:?}", other),
        }
    }

    #[test]
    fn error_build_returns_check_error() {
        let mut stream = ByteStream::new_write();
        let mut ctx = Context::new();
        let err = Error
            .build(&Value::None, &mut stream, &mut ctx)
            .unwrap_err();
        match err {
            ConstructError::Check { message, .. } => {
                assert_eq!(message, ERROR_BUILD_MESSAGE);
            }
            other => panic!("expected Check error, got {:?}", other),
        }
    }

    #[test]
    fn error_sizeof_returns_sizeof_error() {
        let ctx = Context::new();
        let err = Error.sizeof(&ctx).unwrap_err();
        match err {
            ConstructError::Sizeof { reason, .. } => {
                assert_eq!(reason, ERROR_SIZEOF_REASON);
            }
            other => panic!("expected Sizeof error, got {:?}", other),
        }
    }

    // ======================================================================
    // Convenience method roundtrips
    // ======================================================================

    #[test]
    fn pass_convenience_roundtrip() {
        let c: &dyn Construct = &Pass;
        let bytes = c.build_bytes(&Value::None).unwrap();
        assert!(bytes.is_empty());
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, Value::None);
    }

    #[test]
    fn terminated_convenience_at_eof() {
        let c: &dyn Construct = &Terminated;
        let parsed = c.parse_bytes(b"").unwrap();
        assert_eq!(parsed, Value::None);
    }

    #[test]
    fn terminated_convenience_not_at_eof() {
        let c: &dyn Construct = &Terminated;
        let err = c.parse_bytes(b"extra").unwrap_err();
        assert!(matches!(err, ConstructError::Terminated { .. }));
    }

    #[test]
    fn tell_convenience_returns_position() {
        let c: &dyn Construct = &Tell;
        let parsed = c.parse_bytes(b"").unwrap();
        assert_eq!(parsed, Value::UInt(0));
    }

    #[test]
    fn seek_convenience_returns_position() {
        let c: &dyn Construct = &Seek::new(3);
        let parsed = c.parse_bytes(b"012345").unwrap();
        assert_eq!(parsed, Value::UInt(3));
    }

    #[test]
    fn error_convenience_parse_fails() {
        let c: &dyn Construct = &Error;
        let err = c.parse_bytes(b"").unwrap_err();
        assert!(matches!(err, ConstructError::Check { .. }));
    }

    #[test]
    fn error_convenience_build_fails() {
        let c: &dyn Construct = &Error;
        let err = c.build_bytes(&Value::None).unwrap_err();
        assert!(matches!(err, ConstructError::Check { .. }));
    }
}
