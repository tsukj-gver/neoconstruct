//! Stream abstraction for binary I/O.
//!
//! This module defines the [`Stream`] trait and the default [`ByteStream`]
//! implementation used for all binary read/write operations in the construct
//! library.
//!
//! # Stream trait
//!
//! The [`Stream`] trait provides a unified interface for byte-level read,
//! write, and seek operations. All construct operations interact with binary
//! data through this trait, with errors uniformly wrapped in
//! [`ConstructError::Stream`](super::error::ConstructError::Stream).
//!
//! # ByteStream
//!
//! [`ByteStream`] is the default implementation, backed by an in-memory
//! [`std::io::Cursor<Vec<u8>>`]. It supports both parse (read) and build
//! (write) scenarios.
//!
//! # Helper functions
//!
//! Module-level functions [`stream_read`], [`stream_write`], [`stream_seek`],
//! [`stream_tell`], [`stream_size`], and [`stream_iseof`] wrap the trait
//! methods and enrich errors with a `path` parameter, matching the Python
//! original's `stream_read(stream, count, path)` pattern.

use std::io::{Cursor, Read, Write};

use super::error::{ConstructError, Result};

// ===========================================================================
// Stream trait
// ===========================================================================

/// The core stream abstraction for binary I/O.
///
/// All construct operations interact with binary data through this trait.
/// It provides byte-level read, write, and positioning operations with
/// unified error handling via [`ConstructError`].
///
/// Corresponds to Python's `io.BytesIO` interface used in construct, but
/// with a unified error type instead of raw I/O exceptions.
pub trait Stream {
    /// Reads exactly `count` bytes from the current position.
    ///
    /// Returns an error if fewer than `count` bytes are available.
    fn read_bytes(&mut self, count: usize) -> Result<Vec<u8>>;

    /// Reads exactly `buf.len()` bytes into the provided buffer.
    ///
    /// Returns an error if fewer than `buf.len()` bytes are available.
    fn read_exact(&mut self, buf: &mut [u8]) -> Result<()>;

    /// Writes all bytes from `data` to the stream at the current position.
    fn write_bytes(&mut self, data: &[u8]) -> Result<()>;

    /// Seeks to the given absolute position (in bytes from the start).
    fn seek(&mut self, pos: u64) -> Result<()>;

    /// Returns the current cursor position.
    fn tell(&mut self) -> Result<u64>;

    /// Returns the total size of the stream's backing data.
    fn size(&mut self) -> Result<u64>;

    /// Returns `true` if the cursor is at or past the end of the data.
    fn is_eof(&mut self) -> Result<bool>;
}

// ===========================================================================
// ByteStream
// ===========================================================================

/// An in-memory byte stream backed by a [`Cursor<Vec<u8>>`].
///
/// Used for all binary I/O in construct operations. Supports both
/// reading (parse) and writing (build) scenarios.
///
/// # Parse usage
///
/// ```
/// # use construct::core::stream::{ByteStream, Stream};
/// let data = b"\x01\x02\x03";
/// let mut stream = ByteStream::new_read(data);
/// let bytes = stream.read_bytes(3).unwrap();
/// assert_eq!(bytes, vec![1, 2, 3]);
/// ```
///
/// # Build usage
///
/// ```
/// # use construct::core::stream::{ByteStream, Stream};
/// let mut stream = ByteStream::new_write();
/// stream.write_bytes(b"\x04\x05\x06").unwrap();
/// let output = stream.into_bytes();
/// assert_eq!(output, vec![4, 5, 6]);
/// ```
pub struct ByteStream {
    cursor: Cursor<Vec<u8>>,
}

impl ByteStream {
    /// Creates a new stream for reading, initialized with a copy of `data`.
    ///
    /// Use this for parse operations where the input data is consumed.
    pub fn new_read(data: &[u8]) -> Self {
        Self {
            cursor: Cursor::new(data.to_vec()),
        }
    }

    /// Creates a new writable stream with an empty internal buffer.
    ///
    /// Use this for build operations where output data is produced.
    pub fn new_write() -> Self {
        Self {
            cursor: Cursor::new(Vec::new()),
        }
    }

    /// Consumes the stream and returns the internal byte buffer.
    ///
    /// Use this to extract all written data after a build operation.
    pub fn into_bytes(self) -> Vec<u8> {
        self.cursor.into_inner()
    }

    /// Returns a copy of the remaining bytes from the current position to
    /// the end of the stream, without moving the cursor.
    ///
    /// Corresponds to Python's `stream_read_entire` but non-destructive.
    pub fn remaining_bytes(&mut self) -> Result<Vec<u8>> {
        let pos = self.cursor.position() as usize;
        let data = self.cursor.get_ref();
        Ok(data.get(pos..).unwrap_or(&[]).to_vec())
    }
}

impl Default for ByteStream {
    /// Returns an empty writable stream, equivalent to [`ByteStream::new_write`].
    fn default() -> Self {
        Self::new_write()
    }
}

impl Stream for ByteStream {
    fn read_bytes(&mut self, count: usize) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; count];
        self.cursor.read_exact(&mut buf)?;
        Ok(buf)
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        self.cursor.read_exact(buf)?;
        Ok(())
    }

    fn write_bytes(&mut self, data: &[u8]) -> Result<()> {
        self.cursor.write_all(data)?;
        Ok(())
    }

    fn seek(&mut self, pos: u64) -> Result<()> {
        self.cursor.set_position(pos);
        Ok(())
    }

    fn tell(&mut self) -> Result<u64> {
        Ok(self.cursor.position())
    }

    fn size(&mut self) -> Result<u64> {
        Ok(self.cursor.get_ref().len() as u64)
    }

    fn is_eof(&mut self) -> Result<bool> {
        let pos = self.cursor.position() as usize;
        let len = self.cursor.get_ref().len();
        Ok(pos >= len)
    }
}

// ===========================================================================
// Helper functions
// ===========================================================================

/// Reads exactly `count` bytes from the stream.
///
/// Corresponds to Python's `stream_read(stream, length, path)`.
/// On error, the returned [`ConstructError`] carries the provided `path`.
pub fn stream_read(stream: &mut dyn Stream, count: usize, path: &str) -> Result<Vec<u8>> {
    stream
        .read_bytes(count)
        .map_err(|e| e.with_path_prefix(path))
}

/// Writes all bytes in `data` to the stream.
///
/// Corresponds to Python's `stream_write(stream, data, length, path)`.
/// On error, the returned [`ConstructError`] carries the provided `path`.
pub fn stream_write(stream: &mut dyn Stream, data: &[u8], path: &str) -> Result<()> {
    stream
        .write_bytes(data)
        .map_err(|e| e.with_path_prefix(path))
}

/// Seeks the stream to the given absolute position.
///
/// Corresponds to Python's `stream_seek(stream, offset, path)` (with
/// `whence=SEEK_SET` semantics). Negative offsets are not supported and
/// produce an error.
///
/// On error, the returned [`ConstructError`] carries the provided `path`.
pub fn stream_seek(stream: &mut dyn Stream, offset: i64, path: &str) -> Result<()> {
    if offset < 0 {
        let io_err = std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "negative seek offset ({}) is not supported for absolute seeking",
                offset
            ),
        );
        return Err(ConstructError::Stream {
            path: path.to_string(),
            source: io_err,
        });
    }
    stream
        .seek(offset as u64)
        .map_err(|e| e.with_path_prefix(path))
}

/// Returns the current stream position.
///
/// Corresponds to Python's `stream_tell(stream, path)`.
/// On error, the returned [`ConstructError`] carries the provided `path`.
pub fn stream_tell(stream: &mut dyn Stream, path: &str) -> Result<u64> {
    stream.tell().map_err(|e| e.with_path_prefix(path))
}

/// Returns the total size of the stream's backing data.
///
/// Corresponds to Python's `stream_size(stream)`. Unlike the Python
/// version (which takes no `path`), this Rust wrapper enriches errors
/// with the provided `path`.
pub fn stream_size(stream: &mut dyn Stream, path: &str) -> Result<u64> {
    stream.size().map_err(|e| e.with_path_prefix(path))
}

/// Returns `true` if the cursor is at or past the end of the data.
///
/// Corresponds to Python's `stream_iseof(stream)`. Unlike the Python
/// version (which takes no `path`), this Rust wrapper enriches errors
/// with the provided `path`.
pub fn stream_iseof(stream: &mut dyn Stream, path: &str) -> Result<bool> {
    stream.is_eof().map_err(|e| e.with_path_prefix(path))
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -- ByteStream construction ---------------------------------------------

    #[test]
    fn new_read_preserves_data() {
        let data = b"\x01\x02\x03";
        let stream = ByteStream::new_read(data);
        assert_eq!(stream.into_bytes(), vec![1, 2, 3]);
    }

    #[test]
    fn new_write_starts_empty() {
        let stream = ByteStream::new_write();
        assert!(stream.into_bytes().is_empty());
    }

    #[test]
    fn default_is_new_write() {
        let stream = ByteStream::default();
        assert!(stream.into_bytes().is_empty());
    }

    // -- Read operations -----------------------------------------------------

    #[test]
    fn read_bytes_returns_correct_data() {
        let mut stream = ByteStream::new_read(b"\x01\x02\x03\x04\x05");
        let bytes = stream.read_bytes(3).unwrap();
        assert_eq!(bytes, vec![1, 2, 3]);
    }

    #[test]
    fn read_bytes_advances_position() {
        let mut stream = ByteStream::new_read(b"\x01\x02\x03\x04\x05");
        let _ = stream.read_bytes(2).unwrap();
        assert_eq!(stream.tell().unwrap(), 2);
    }

    #[test]
    fn read_exact_fills_buffer() {
        let mut stream = ByteStream::new_read(b"\x01\x02\x03\x04");
        let mut buf = [0u8; 2];
        stream.read_exact(&mut buf).unwrap();
        assert_eq!(buf, [1, 2]);
    }

    #[test]
    fn read_exact_advances_position() {
        let mut stream = ByteStream::new_read(b"\x01\x02\x03");
        let mut buf = [0u8; 1];
        stream.read_exact(&mut buf).unwrap();
        stream.read_exact(&mut buf).unwrap();
        assert_eq!(stream.tell().unwrap(), 2);
    }

    #[test]
    fn read_zero_bytes_returns_empty() {
        let mut stream = ByteStream::new_read(b"\x01\x02");
        let bytes = stream.read_bytes(0).unwrap();
        assert!(bytes.is_empty());
        assert_eq!(stream.tell().unwrap(), 0);
    }

    #[test]
    fn read_past_end_returns_stream_error() {
        let mut stream = ByteStream::new_read(b"\x01\x02");
        let result = stream.read_bytes(5);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ConstructError::Stream { .. }));
        assert_eq!(
            err.to_string(),
            "stream error at : failed to fill whole buffer"
        );
    }

    #[test]
    fn read_on_empty_stream_returns_error() {
        let mut stream = ByteStream::new_write();
        let result = stream.read_bytes(1);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ConstructError::Stream { .. }));
    }

    // -- Write operations ----------------------------------------------------

    #[test]
    fn write_bytes_and_into_bytes() {
        let mut stream = ByteStream::new_write();
        stream.write_bytes(b"\x01\x02\x03").unwrap();
        assert_eq!(stream.into_bytes(), vec![1, 2, 3]);
    }

    #[test]
    fn write_zero_bytes_is_noop() {
        let mut stream = ByteStream::new_write();
        stream.write_bytes(b"").unwrap();
        assert!(stream.into_bytes().is_empty());
    }

    #[test]
    fn write_appends_at_position() {
        let mut stream = ByteStream::new_write();
        stream.write_bytes(b"\x01\x02").unwrap();
        stream.write_bytes(b"\x03\x04").unwrap();
        assert_eq!(stream.into_bytes(), vec![1, 2, 3, 4]);
    }

    // -- Seek / Tell ---------------------------------------------------------

    #[test]
    fn tell_initially_zero() {
        let mut stream = ByteStream::new_read(b"\x01\x02");
        assert_eq!(stream.tell().unwrap(), 0);
    }

    #[test]
    fn seek_and_tell() {
        let mut stream = ByteStream::new_read(b"\x01\x02\x03\x04\x05");
        stream.seek(3).unwrap();
        assert_eq!(stream.tell().unwrap(), 3);
    }

    #[test]
    fn seek_then_read() {
        let mut stream = ByteStream::new_read(b"\x01\x02\x03\x04\x05");
        stream.seek(3).unwrap();
        let bytes = stream.read_bytes(2).unwrap();
        assert_eq!(bytes, vec![4, 5]);
    }

    #[test]
    fn seek_to_zero_resets_position() {
        let mut stream = ByteStream::new_read(b"\x01\x02\x03");
        stream.seek(2).unwrap();
        stream.seek(0).unwrap();
        assert_eq!(stream.tell().unwrap(), 0);
        let bytes = stream.read_bytes(1).unwrap();
        assert_eq!(bytes, vec![1]);
    }

    #[test]
    fn seek_past_end_sets_position_without_extending() {
        let mut stream = ByteStream::new_read(b"\x01\x02");
        stream.seek(100).unwrap();
        assert_eq!(stream.tell().unwrap(), 100);
        // Size is still 2, not extended
        assert_eq!(stream.size().unwrap(), 2);
    }

    // -- Size ----------------------------------------------------------------

    #[test]
    fn size_of_read_stream() {
        let mut stream = ByteStream::new_read(b"\x01\x02\x03");
        assert_eq!(stream.size().unwrap(), 3);
    }

    #[test]
    fn size_of_empty_stream() {
        let mut stream = ByteStream::new_write();
        assert_eq!(stream.size().unwrap(), 0);
    }

    #[test]
    fn size_after_writing() {
        let mut stream = ByteStream::new_write();
        stream.write_bytes(b"\x01\x02\x03").unwrap();
        assert_eq!(stream.size().unwrap(), 3);
    }

    #[test]
    fn size_does_not_change_position() {
        let mut stream = ByteStream::new_read(b"\x01\x02\x03\x04");
        stream.seek(2).unwrap();
        let _ = stream.size().unwrap();
        assert_eq!(stream.tell().unwrap(), 2);
    }

    // -- is_eof --------------------------------------------------------------

    #[test]
    fn is_eof_false_for_nonempty_at_start() {
        let mut stream = ByteStream::new_read(b"\x01\x02");
        assert!(!stream.is_eof().unwrap());
    }

    #[test]
    fn is_eof_true_after_reading_all() {
        let mut stream = ByteStream::new_read(b"\x01\x02");
        let _ = stream.read_bytes(2).unwrap();
        assert!(stream.is_eof().unwrap());
    }

    #[test]
    fn is_eof_true_for_empty_stream() {
        let mut stream = ByteStream::new_write();
        assert!(stream.is_eof().unwrap());
    }

    #[test]
    fn is_eof_false_in_middle() {
        let mut stream = ByteStream::new_read(b"\x01\x02\x03\x04");
        stream.seek(1).unwrap();
        assert!(!stream.is_eof().unwrap());
    }

    #[test]
    fn is_eof_true_past_end() {
        let mut stream = ByteStream::new_read(b"\x01\x02");
        stream.seek(10).unwrap();
        assert!(stream.is_eof().unwrap());
    }

    // -- remaining_bytes -----------------------------------------------------

    #[test]
    fn remaining_bytes_from_start() {
        let mut stream = ByteStream::new_read(b"\x01\x02\x03");
        assert_eq!(stream.remaining_bytes().unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn remaining_bytes_from_middle() {
        let mut stream = ByteStream::new_read(b"\x01\x02\x03\x04\x05");
        stream.seek(2).unwrap();
        assert_eq!(stream.remaining_bytes().unwrap(), vec![3, 4, 5]);
    }

    #[test]
    fn remaining_bytes_does_not_move_cursor() {
        let mut stream = ByteStream::new_read(b"\x01\x02\x03");
        stream.seek(1).unwrap();
        let _ = stream.remaining_bytes().unwrap();
        assert_eq!(stream.tell().unwrap(), 1);
    }

    #[test]
    fn remaining_bytes_at_end_is_empty() {
        let mut stream = ByteStream::new_read(b"\x01\x02");
        stream.seek(2).unwrap();
        assert!(stream.remaining_bytes().unwrap().is_empty());
    }

    #[test]
    fn remaining_bytes_on_empty_stream() {
        let mut stream = ByteStream::new_write();
        assert!(stream.remaining_bytes().unwrap().is_empty());
    }

    // -- Helper function: stream_read ----------------------------------------

    #[test]
    fn stream_read_success() {
        let mut stream = ByteStream::new_read(b"\x01\x02\x03");
        let bytes = stream_read(&mut stream, 2, "test").unwrap();
        assert_eq!(bytes, vec![1, 2]);
    }

    #[test]
    fn stream_read_error_has_path() {
        let mut stream = ByteStream::new_read(b"\x01");
        let err = stream_read(&mut stream, 5, "myfield").unwrap_err();
        assert_eq!(err.path(), "myfield");
        assert!(matches!(err, ConstructError::Stream { .. }));
    }

    // -- Helper function: stream_write ---------------------------------------

    #[test]
    fn stream_write_success() {
        let mut stream = ByteStream::new_write();
        stream_write(&mut stream, b"\x01\x02", "test").unwrap();
        assert_eq!(stream.into_bytes(), vec![1, 2]);
    }

    // -- Helper function: stream_seek ----------------------------------------

    #[test]
    fn stream_seek_success() {
        let mut stream = ByteStream::new_read(b"\x01\x02\x03\x04");
        stream_seek(&mut stream, 2, "test").unwrap();
        assert_eq!(stream.tell().unwrap(), 2);
    }

    #[test]
    fn stream_seek_negative_offset_error_has_path() {
        let mut stream = ByteStream::new_read(b"\x01\x02");
        let err = stream_seek(&mut stream, -1, "seekfield").unwrap_err();
        assert_eq!(err.path(), "seekfield");
        match err {
            ConstructError::Stream { source, .. } => {
                assert_eq!(source.kind(), std::io::ErrorKind::InvalidInput);
            }
            other => panic!("expected Stream error, got {:?}", other),
        }
    }

    // -- Helper function: stream_tell ----------------------------------------

    #[test]
    fn stream_tell_success() {
        let mut stream = ByteStream::new_read(b"\x01\x02");
        let pos = stream_tell(&mut stream, "test").unwrap();
        assert_eq!(pos, 0);
    }

    // -- Helper function: stream_size ----------------------------------------

    #[test]
    fn stream_size_success() {
        let mut stream = ByteStream::new_read(b"\x01\x02\x03");
        let sz = stream_size(&mut stream, "test").unwrap();
        assert_eq!(sz, 3);
    }

    // -- Helper function: stream_iseof ---------------------------------------

    #[test]
    fn stream_iseof_true_after_read() {
        let mut stream = ByteStream::new_read(b"\x01");
        let _ = stream.read_bytes(1).unwrap();
        assert!(stream_iseof(&mut stream, "test").unwrap());
    }

    #[test]
    fn stream_iseof_false_before_read() {
        let mut stream = ByteStream::new_read(b"\x01\x02");
        assert!(!stream_iseof(&mut stream, "test").unwrap());
    }

    // -- Parse/Build symmetry ------------------------------------------------

    #[test]
    fn write_then_read_roundtrip() {
        let mut writer = ByteStream::new_write();
        writer.write_bytes(b"\x0A\x0B\x0C").unwrap();
        let data = writer.into_bytes();

        let mut reader = ByteStream::new_read(&data);
        let bytes = reader.read_bytes(3).unwrap();
        assert_eq!(bytes, vec![0x0A, 0x0B, 0x0C]);
        assert!(reader.is_eof().unwrap());
    }

    #[test]
    fn seek_write_seek_read_roundtrip() {
        let mut writer = ByteStream::new_write();
        writer.write_bytes(b"\x01\x02\x03").unwrap();
        writer.seek(1).unwrap();
        writer.write_bytes(b"\xFF").unwrap();
        let data = writer.into_bytes();

        let mut reader = ByteStream::new_read(&data);
        assert_eq!(reader.read_bytes(3).unwrap(), vec![1, 0xFF, 3]);
    }

    // -- Sequential reads ----------------------------------------------------

    #[test]
    fn sequential_reads_consume_data() {
        let mut stream = ByteStream::new_read(b"\x01\x02\x03\x04\x05");
        assert_eq!(stream.read_bytes(2).unwrap(), vec![1, 2]);
        assert_eq!(stream.read_bytes(2).unwrap(), vec![3, 4]);
        assert_eq!(stream.read_bytes(1).unwrap(), vec![5]);
        assert!(stream.is_eof().unwrap());
    }

    // -- Large data ----------------------------------------------------------

    #[test]
    fn large_data_roundtrip() {
        let data: Vec<u8> = (0..=255).cycle().take(1024).collect();
        let mut stream = ByteStream::new_read(&data);
        let read = stream.read_bytes(1024).unwrap();
        assert_eq!(read, data);
        assert!(stream.is_eof().unwrap());
    }
}
