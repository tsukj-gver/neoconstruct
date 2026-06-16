//! [`Stream`] implementation backed by a Python file-like object.
//!
//! [`PyStream`] wraps a `Py<PyAny>` reference to a Python object that
//! supports `read`, `write`, `seek`, and `tell` methods (e.g.
//! `io.BytesIO`, file objects). Each [`Stream`] trait method acquires the
//! GIL via [`Python::with_gil`] and dispatches to the corresponding Python
//! method.

use std::io::SeekFrom;

use construct::core::error::{ConstructError, Result};
use construct::core::stream::Stream;

use pyo3::prelude::*;
use pyo3::types::PyBytes;

/// A [`Stream`] adapter that delegates to a Python file-like object.
///
/// The wrapped `Py<PyAny>` holds a strong reference to the Python object,
/// keeping it alive for the lifetime of the [`PyStream`].
pub struct PyStream {
    /// The Python file-like object (BytesIO, file handle, etc.).
    obj: Py<PyAny>,
}

impl PyStream {
    /// Creates a new [`PyStream`] wrapping the given Python file-like object.
    #[must_use]
    pub fn new(obj: Py<PyAny>) -> Self {
        PyStream { obj }
    }

    /// Reads all remaining bytes from the Python stream by calling `read()`
    /// with no arguments.
    ///
    /// This is used by [`py_parse_stream`](crate::api::py_parse_stream) to
    /// buffer the entire stream into memory before parsing (since `PyStream`
    /// cannot be a `CombinedStream` variant).
    pub fn read_all(&mut self) -> Result<Vec<u8>> {
        Python::with_gil(|py| {
            let obj = self.obj.bind(py);
            let result = obj.call_method0("read").map_err(pyerr_to_stream)?;
            let bytes_obj = result
                .downcast::<PyBytes>()
                .map_err(|_| stream_error("Python stream read() did not return bytes"))?;
            Ok(bytes_obj.as_bytes().to_vec())
        })
    }
}

/// Converts a [`PyErr`] into a [`ConstructError::Stream`] with an empty path.
/// The path is filled in by parent constructs via `with_path_prefix`.
fn pyerr_to_stream(e: PyErr) -> ConstructError {
    ConstructError::Stream {
        path: String::new(),
        source: std::io::Error::new(std::io::ErrorKind::Other, e.to_string()),
    }
}

/// Creates a [`ConstructError::Stream`] with a custom message.
fn stream_error(msg: impl Into<String>) -> ConstructError {
    ConstructError::Stream {
        path: String::new(),
        source: std::io::Error::new(std::io::ErrorKind::Other, msg.into()),
    }
}

impl Stream for PyStream {
    fn read_bytes(&mut self, count: usize) -> Result<Vec<u8>> {
        Python::with_gil(|py| {
            let obj = self.obj.bind(py);
            let result = obj
                .call_method1("read", (count,))
                .map_err(pyerr_to_stream)?;
            let bytes_obj = result
                .downcast::<PyBytes>()
                .map_err(|_| stream_error("Python stream read() did not return bytes"))?;
            let data = bytes_obj.as_bytes();
            if data.len() < count {
                return Err(ConstructError::Stream {
                    path: String::new(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        format!(
                            "Python stream read {} bytes, expected {}",
                            data.len(),
                            count
                        ),
                    ),
                });
            }
            Ok(data.to_vec())
        })
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        let expected = buf.len();
        let data = self.read_bytes(expected)?;
        // read_bytes already verified the length, but double-check for safety.
        if data.len() != expected {
            return Err(stream_error(format!(
                "read_exact: expected {expected} bytes, got {}",
                data.len()
            )));
        }
        buf.copy_from_slice(&data);
        Ok(())
    }

    fn write_bytes(&mut self, data: &[u8]) -> Result<()> {
        Python::with_gil(|py| {
            let obj = self.obj.bind(py);
            let py_bytes = PyBytes::new_bound(py, data);
            obj.call_method1("write", (py_bytes,))
                .map_err(pyerr_to_stream)?;
            Ok(())
        })
    }

    fn seek(&mut self, pos: u64) -> Result<()> {
        Python::with_gil(|py| {
            let obj = self.obj.bind(py);
            obj.call_method1("seek", (pos,)).map_err(pyerr_to_stream)?;
            Ok(())
        })
    }

    fn tell(&mut self) -> Result<u64> {
        Python::with_gil(|py| {
            let obj = self.obj.bind(py);
            let result = obj.call_method0("tell").map_err(pyerr_to_stream)?;
            let pos: u64 = result.extract().map_err(pyerr_to_stream)?;
            Ok(pos)
        })
    }

    fn size(&mut self) -> Result<u64> {
        Python::with_gil(|py| {
            let obj = self.obj.bind(py);
            // Save current position.
            let current: u64 = obj
                .call_method0("tell")
                .map_err(pyerr_to_stream)?
                .extract()
                .map_err(pyerr_to_stream)?;
            // Seek to end (whence=2).
            let end: u64 = obj
                .call_method1("seek", (0u64, 2i32))
                .map_err(pyerr_to_stream)?
                .extract()
                .map_err(pyerr_to_stream)?;
            // Restore position.
            obj.call_method1("seek", (current,))
                .map_err(pyerr_to_stream)?;
            Ok(end)
        })
    }

    fn is_eof(&mut self) -> Result<bool> {
        let pos = self.tell()?;
        let size = self.size()?;
        Ok(pos >= size)
    }

    fn seek_from(&mut self, pos: SeekFrom) -> Result<u64> {
        Python::with_gil(|py| {
            let obj = self.obj.bind(py);
            let (offset, whence) = match pos {
                SeekFrom::Start(n) => (n as i64, 0i32),
                SeekFrom::End(n) => (n, 2i32),
                SeekFrom::Current(n) => (n, 1i32),
            };
            let result = obj
                .call_method1("seek", (offset, whence))
                .map_err(pyerr_to_stream)?;
            let new_pos: u64 = result.extract().map_err(pyerr_to_stream)?;
            Ok(new_pos)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates a Python BytesIO object for testing.
    fn make_bytesio(initial_data: &[u8]) -> Py<PyAny> {
        crate::ensure_python();
        Python::with_gil(|py| {
            let bytesio_class = py.import_bound("io").unwrap().getattr("BytesIO").unwrap();
            let obj = bytesio_class
                .call1((PyBytes::new_bound(py, initial_data),))
                .unwrap();
            obj.unbind()
        })
    }

    #[test]
    fn read_bytes_from_bytesio() {
        let bio = make_bytesio(&[1, 2, 3, 4, 5]);
        let mut stream = PyStream::new(bio);
        let data = stream.read_bytes(3).unwrap();
        assert_eq!(data, vec![1, 2, 3]);
    }

    #[test]
    fn read_exact_fills_buffer() {
        let bio = make_bytesio(&[10, 20, 30]);
        let mut stream = PyStream::new(bio);
        let mut buf = [0u8; 3];
        stream.read_exact(&mut buf).unwrap();
        assert_eq!(buf, [10, 20, 30]);
    }

    #[test]
    fn read_past_eof_errors() {
        let bio = make_bytesio(&[1, 2]);
        let mut stream = PyStream::new(bio);
        let err = stream.read_bytes(5).unwrap_err();
        assert!(matches!(err, ConstructError::Stream { .. }));
    }

    #[test]
    fn write_then_read_roundtrip() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let bytesio_class = py.import_bound("io").unwrap().getattr("BytesIO").unwrap();
            let obj = bytesio_class.call0().unwrap();
            let bio = obj.unbind();

            let mut stream = PyStream::new(bio);
            stream.write_bytes(&[0xDE, 0xAD, 0xBE, 0xEF]).unwrap();
            stream.seek(0).unwrap();
            let data = stream.read_bytes(4).unwrap();
            assert_eq!(data, vec![0xDE, 0xAD, 0xBE, 0xEF]);
        });
    }

    #[test]
    fn tell_returns_position() {
        let bio = make_bytesio(&[1, 2, 3, 4, 5]);
        let mut stream = PyStream::new(bio);
        assert_eq!(stream.tell().unwrap(), 0);
        stream.read_bytes(2).unwrap();
        assert_eq!(stream.tell().unwrap(), 2);
    }

    #[test]
    fn seek_to_position() {
        let bio = make_bytesio(&[1, 2, 3, 4, 5]);
        let mut stream = PyStream::new(bio);
        stream.seek(3).unwrap();
        assert_eq!(stream.tell().unwrap(), 3);
        let data = stream.read_bytes(2).unwrap();
        assert_eq!(data, vec![4, 5]);
    }

    #[test]
    fn size_returns_total_length() {
        let bio = make_bytesio(&[1, 2, 3, 4, 5]);
        let mut stream = PyStream::new(bio);
        assert_eq!(stream.size().unwrap(), 5);
        // size() should not move the cursor.
        assert_eq!(stream.tell().unwrap(), 0);
    }

    #[test]
    fn is_eof_checks_position() {
        let bio = make_bytesio(&[1, 2]);
        let mut stream = PyStream::new(bio);
        assert!(!stream.is_eof().unwrap());
        stream.read_bytes(2).unwrap();
        assert!(stream.is_eof().unwrap());
    }

    #[test]
    fn seek_from_current() {
        let bio = make_bytesio(&[1, 2, 3, 4, 5]);
        let mut stream = PyStream::new(bio);
        stream.read_bytes(1).unwrap();
        let pos = stream.seek_from(SeekFrom::Current(2)).unwrap();
        assert_eq!(pos, 3);
    }

    #[test]
    fn seek_from_end() {
        let bio = make_bytesio(&[1, 2, 3, 4, 5]);
        let mut stream = PyStream::new(bio);
        let pos = stream.seek_from(SeekFrom::End(-2)).unwrap();
        assert_eq!(pos, 3);
    }

    #[test]
    fn read_zero_bytes() {
        let bio = make_bytesio(&[1, 2, 3]);
        let mut stream = PyStream::new(bio);
        let data = stream.read_bytes(0).unwrap();
        assert!(data.is_empty());
        assert_eq!(stream.tell().unwrap(), 0);
    }
}
