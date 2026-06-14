//! Public API helpers for PyConstruct wrapper types.
//!
//! Each `#[pyclass]` construct wrapper delegates its `parse` / `build` /
//! `sizeof` methods to one of these seven helper functions. The helpers
//! handle `**contextkw` injection, Value &harr; PyObject conversion, and
//! error mapping uniformly.

use std::path::Path;

use construct::core::context::Context;
use construct::core::error::ConstructError;
use construct::core::stream::{ByteStream, Stream};
use construct::core::Construct;
use construct::value::Value;

use indexmap::IndexMap;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};

use crate::conversions::{py_to_value, value_to_py};
use crate::exceptions::rust_err_to_py;
use crate::pystream::PyStream;

/// The path prefix added to errors originating from `parse` operations,
/// mirroring the Python original.
const PARSE_PATH: &str = "(parsing)";

/// The path prefix added to errors originating from `build` operations.
const BUILD_PATH: &str = "(building)";

/// The path prefix added to errors originating from `sizeof` operations.
const SIZEOF_PATH: &str = "(sizeof)";

/// Converts a Python `**kwargs` dict into an [`IndexMap<String, Value>`].
///
/// Keys must be Python `str`; non-string keys produce a `TypeError`.
/// Values are recursively converted via [`py_to_value`].
///
/// # Errors
///
/// Returns `PyErr` on type errors or conversion failures.
pub fn pykw_to_indexmap(
    py: Python<'_>,
    kw: &Bound<'_, PyDict>,
) -> PyResult<IndexMap<String, Value>> {
    let mut map = IndexMap::new();
    for (key, value) in kw.iter() {
        let key_str: String = key.extract().map_err(|_| {
            pyo3::exceptions::PyTypeError::new_err("contextkw keys must be strings")
        })?;
        let val = py_to_value(py, &value)?;
        map.insert(key_str, val);
    }
    Ok(map)
}

/// Converts an optional Python `**kwargs` dict into an [`IndexMap`].
///
/// Returns an empty map when `kw` is `None`.
pub fn opt_kw_to_indexmap(
    py: Python<'_>,
    kw: Option<&Bound<'_, PyDict>>,
) -> PyResult<IndexMap<String, Value>> {
    match kw {
        Some(d) => pykw_to_indexmap(py, d),
        None => Ok(IndexMap::new()),
    }
}

// ===========================================================================
// Parse helpers
// ===========================================================================

/// Parses binary data from a byte slice.
///
/// Creates an in-memory [`ByteStream`], injects `kw` as top-level context
/// fields, calls `constr.parse`, and converts the result to a Python object.
///
/// # Errors
///
/// Returns `PyErr` (a construct exception subclass) on parse failure.
pub fn py_parse(
    constr: &dyn Construct,
    py: Python<'_>,
    data: &[u8],
    kw: IndexMap<String, Value>,
) -> PyResult<PyObject> {
    let mut stream = ByteStream::new_read(data);
    let mut ctx = Context::new();
    for (key, value) in kw {
        ctx.insert(key, value);
    }
    match constr.parse(&mut stream, &mut ctx) {
        Ok(value) => value_to_py(py, &value),
        Err(e) => Err(rust_err_to_py(py, e.with_path_prefix(PARSE_PATH))),
    }
}

/// Parses binary data from a Python file-like object.
///
/// Wraps the object in a [`PyStream`] and delegates to [`Construct::parse`].
///
/// # Errors
///
/// Returns `PyErr` on parse or stream failure.
pub fn py_parse_stream(
    constr: &dyn Construct,
    py: Python<'_>,
    stream_obj: Py<PyAny>,
    kw: IndexMap<String, Value>,
) -> PyResult<PyObject> {
    let mut stream = PyStream::new(stream_obj);
    let mut ctx = Context::new();
    for (key, value) in kw {
        ctx.insert(key, value);
    }
    match constr.parse(&mut stream, &mut ctx) {
        Ok(value) => value_to_py(py, &value),
        Err(e) => Err(rust_err_to_py(py, e.with_path_prefix(PARSE_PATH))),
    }
}

/// Parses binary data from a file.
///
/// Reads the entire file into memory, then delegates to [`py_parse`].
///
/// # Errors
///
/// Returns `PyErr` on file I/O or parse failure.
pub fn py_parse_file(
    constr: &dyn Construct,
    py: Python<'_>,
    filename: &str,
    kw: IndexMap<String, Value>,
) -> PyResult<PyObject> {
    let data = std::fs::read(Path::new(filename)).map_err(|e| {
        rust_err_to_py(
            py,
            ConstructError::Stream {
                path: String::new(),
                source: e,
            },
        )
    })?;
    py_parse(constr, py, &data, kw)
}

// ===========================================================================
// Build helpers
// ===========================================================================

/// Builds binary data from a Python object and returns it as `bytes`.
///
/// Converts `data` to [`Value`], creates a write [`ByteStream`], injects
/// `kw`, calls `constr.build`, and returns the written bytes.
///
/// # Errors
///
/// Returns `PyErr` on conversion or build failure.
pub fn py_build(
    constr: &dyn Construct,
    py: Python<'_>,
    data: &Bound<'_, PyAny>,
    kw: IndexMap<String, Value>,
) -> PyResult<PyObject> {
    let value = py_to_value(py, data)?;
    let mut stream = ByteStream::new_write();
    let mut ctx = Context::new();
    for (key, val) in kw {
        ctx.insert(key, val);
    }
    match constr.build(&value, &mut stream, &mut ctx) {
        Ok(()) => {
            let bytes = stream.into_bytes();
            Ok(PyBytes::new_bound(py, &bytes).into_any().unbind())
        }
        Err(e) => Err(rust_err_to_py(py, e.with_path_prefix(BUILD_PATH))),
    }
}

/// Builds binary data and writes it to a Python file-like object.
///
/// Builds to an in-memory buffer first, then writes the bytes to the stream.
///
/// # Errors
///
/// Returns `PyErr` on conversion, build, or stream-write failure.
pub fn py_build_stream(
    constr: &dyn Construct,
    py: Python<'_>,
    data: &Bound<'_, PyAny>,
    stream_obj: Py<PyAny>,
    kw: IndexMap<String, Value>,
) -> PyResult<()> {
    let value = py_to_value(py, data)?;
    let mut byte_stream = ByteStream::new_write();
    let mut ctx = Context::new();
    for (key, val) in kw {
        ctx.insert(key, val);
    }
    constr
        .build(&value, &mut byte_stream, &mut ctx)
        .map_err(|e| rust_err_to_py(py, e.with_path_prefix(BUILD_PATH)))?;
    let bytes = byte_stream.into_bytes();
    let mut py_stream = PyStream::new(stream_obj);
    py_stream
        .write_bytes(&bytes)
        .map_err(|e| rust_err_to_py(py, e))?;
    Ok(())
}

/// Builds binary data and writes it to a file.
///
/// Uses `OpenOptions` with write+create+truncate+read modes to match the
/// Python original's `'w+b'` behaviour (needed by `RawCopy` for readback).
///
/// # Errors
///
/// Returns `PyErr` on conversion, build, or file-write failure.
pub fn py_build_file(
    constr: &dyn Construct,
    py: Python<'_>,
    data: &Bound<'_, PyAny>,
    filename: &str,
    kw: IndexMap<String, Value>,
) -> PyResult<()> {
    let value = py_to_value(py, data)?;
    let mut stream = ByteStream::new_write();
    let mut ctx = Context::new();
    for (key, val) in kw {
        ctx.insert(key, val);
    }
    constr
        .build(&value, &mut stream, &mut ctx)
        .map_err(|e| rust_err_to_py(py, e.with_path_prefix(BUILD_PATH)))?;
    let bytes = stream.into_bytes();
    std::fs::write(Path::new(filename), bytes).map_err(|e| {
        rust_err_to_py(
            py,
            ConstructError::Stream {
                path: String::new(),
                source: e,
            },
        )
    })?;
    Ok(())
}

// ===========================================================================
// Sizeof helper
// ===========================================================================

/// Computes the static byte size of a construct.
///
/// Creates a [`Context`], injects `kw`, and calls `constr.sizeof`.
///
/// # Errors
///
/// Returns `PyErr` (a `SizeofError`) on failure.
pub fn py_sizeof(
    constr: &dyn Construct,
    py: Python<'_>,
    kw: IndexMap<String, Value>,
) -> PyResult<usize> {
    let mut ctx = Context::new();
    for (key, val) in kw {
        ctx.insert(key, val);
    }
    constr
        .sizeof(&ctx)
        .map_err(|e| rust_err_to_py(py, e.with_path_prefix(SIZEOF_PATH)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use construct::core::error::Result as ConstructResult;
    use construct::core::stream::Stream as ConstructStream;

    /// A minimal construct that reads/writes exactly 4 bytes as a big-endian
    /// u32, used solely for testing the API helpers.
    struct U32Big;

    impl Construct for U32Big {
        fn parse(
            &self,
            stream: &mut dyn ConstructStream,
            _ctx: &mut Context,
        ) -> ConstructResult<Value> {
            let bytes = stream.read_bytes(4)?;
            let val = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            Ok(Value::UInt(val as u64))
        }

        fn build(
            &self,
            data: &Value,
            stream: &mut dyn ConstructStream,
            ctx: &mut Context,
        ) -> ConstructResult<()> {
            let val = data.to_u64()?;
            let bytes = (val as u32).to_be_bytes();
            stream.write_bytes(&bytes)?;
            // Verify contextkw injection works.
            let _ = ctx;
            Ok(())
        }

        fn sizeof(&self, _ctx: &Context) -> ConstructResult<usize> {
            Ok(4)
        }
    }

    /// A construct that reads a length specified by the "length" context key.
    struct VarBytes;

    impl Construct for VarBytes {
        fn parse(
            &self,
            stream: &mut dyn ConstructStream,
            ctx: &mut Context,
        ) -> ConstructResult<Value> {
            let length = ctx
                .get_recursive("length")
                .ok_or_else(|| ConstructError::FieldMissing {
                    path: String::new(),
                    field: "length".to_string(),
                })?
                .to_u64()? as usize;
            let bytes = stream.read_bytes(length)?;
            Ok(Value::Bytes(bytes))
        }

        fn build(
            &self,
            data: &Value,
            stream: &mut dyn ConstructStream,
            _ctx: &mut Context,
        ) -> ConstructResult<()> {
            stream.write_bytes(data.as_bytes()?)
        }

        fn sizeof(&self, ctx: &Context) -> ConstructResult<usize> {
            ctx.get_recursive("length")
                .ok_or_else(|| ConstructError::Sizeof {
                    path: String::new(),
                    reason: "length not in context".to_string(),
                })?
                .to_u64()
                .map(|v| v as usize)
                .map_err(|e: ConstructError| e.with_path_prefix("length"))
        }
    }

    // -- py_parse ------------------------------------------------------------

    #[test]
    fn py_parse_u32() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let result = py_parse(&U32Big, py, &[0, 0, 0, 42], IndexMap::new()).unwrap();
            let val: u64 = result.extract(py).unwrap();
            assert_eq!(val, 42);
        });
    }

    #[test]
    fn py_parse_with_contextkw() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let mut kw = IndexMap::new();
            kw.insert("length".to_string(), Value::UInt(3));
            let result = py_parse(&VarBytes, py, &[0xAA, 0xBB, 0xCC, 0xDD], kw).unwrap();
            let bytes: Vec<u8> = result.extract(py).unwrap();
            assert_eq!(bytes, vec![0xAA, 0xBB, 0xCC]);
        });
    }

    #[test]
    fn py_parse_error_maps_to_exception() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let err = py_parse(&U32Big, py, &[0, 0], IndexMap::new()).unwrap_err();
            // Should be a StreamError (not a raw Python exception)
            use crate::exceptions::StreamError;
            assert!(err.is_instance_of::<StreamError>(py));
        });
    }

    // -- py_build ------------------------------------------------------------

    #[test]
    fn py_build_u32() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let val = 0xDEADBEEFu64.into_py(py);
            let bound = val.bind(py);
            let result = py_build(&U32Big, py, bound, IndexMap::new()).unwrap();
            let bytes: Vec<u8> = result.extract(py).unwrap();
            assert_eq!(bytes, vec![0xDE, 0xAD, 0xBE, 0xEF]);
        });
    }

    #[test]
    fn py_build_error_maps_to_exception() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let s = "not an int".to_string().into_py(py);
            let bound = s.bind(py);
            let err = py_build(&U32Big, py, bound, IndexMap::new()).unwrap_err();
            use crate::exceptions::ConstructError;
            assert!(err.is_instance_of::<ConstructError>(py));
        });
    }

    // -- py_sizeof -----------------------------------------------------------

    #[test]
    fn py_sizeof_fixed() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let size = py_sizeof(&U32Big, py, IndexMap::new()).unwrap();
            assert_eq!(size, 4);
        });
    }

    #[test]
    fn py_sizeof_with_contextkw() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let mut kw = IndexMap::new();
            kw.insert("length".to_string(), Value::UInt(10));
            let size = py_sizeof(&VarBytes, py, kw).unwrap();
            assert_eq!(size, 10);
        });
    }

    #[test]
    fn py_sizeof_error_maps_to_sizeof_error() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let err = py_sizeof(&VarBytes, py, IndexMap::new()).unwrap_err();
            use crate::exceptions::SizeofError;
            assert!(err.is_instance_of::<SizeofError>(py));
        });
    }

    // -- py_parse_stream / py_build_stream -----------------------------------

    #[test]
    fn py_parse_stream_from_bytesio() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let bytesio = py.import_bound("io").unwrap().getattr("BytesIO").unwrap();
            let stream_obj = bytesio
                .call1((PyBytes::new_bound(py, &[0, 0, 0, 99]),))
                .unwrap()
                .unbind();
            let result = py_parse_stream(&U32Big, py, stream_obj, IndexMap::new()).unwrap();
            let val: u64 = result.extract(py).unwrap();
            assert_eq!(val, 99);
        });
    }

    #[test]
    fn py_build_stream_to_bytesio() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let bytesio = py.import_bound("io").unwrap().getattr("BytesIO").unwrap();
            let stream_obj = bytesio.call0().unwrap().unbind();

            let val = 0x12345678u64.into_py(py);
            py_build_stream(
                &U32Big,
                py,
                val.bind(py),
                stream_obj.clone_ref(py),
                IndexMap::new(),
            )
            .unwrap();

            // Read back
            let stream = PyStream::new(stream_obj);
            let mut s = stream;
            s.seek(0).unwrap();
            let data = s.read_bytes(4).unwrap();
            assert_eq!(data, vec![0x12, 0x34, 0x56, 0x78]);
        });
    }

    // -- pykw_to_indexmap ----------------------------------------------------

    #[test]
    fn pykw_to_indexmap_basic() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let dict = PyDict::new_bound(py);
            dict.set_item("a", 1i32).unwrap();
            dict.set_item("b", "hello").unwrap();
            let map = pykw_to_indexmap(py, &dict).unwrap();
            assert_eq!(map.len(), 2);
            assert_eq!(map.get("a").unwrap(), &Value::Int(1));
        });
    }

    #[test]
    fn pykw_to_indexmap_non_str_key() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let dict = PyDict::new_bound(py);
            dict.set_item(42i32, "value").unwrap();
            let err = pykw_to_indexmap(py, &dict).unwrap_err();
            assert!(err.is_instance_of::<pyo3::exceptions::PyTypeError>(py));
        });
    }
}
