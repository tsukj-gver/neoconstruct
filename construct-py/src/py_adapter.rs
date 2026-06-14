//! [`PyConstructAdapter`] — bridges arbitrary Python constructs into the Rust
//! [`Construct`] trait.
//!
//! When a Python user creates a custom Adapter/Validator subclass, the
//! resulting object is a pure-Python instance (not a PyO3 `#[pyclass]`).
//! To embed it inside a Rust Struct or Sequence, we wrap it in
//! [`PyConstructAdapter`], which implements [`Construct`] by delegating to
//! the Python object's `parse_stream` / `build_stream` / `sizeof` methods
//! through the GIL.
//!
//! # Stream strategy (Phase 10)
//!
//! The **Read-all + BytesIO** approach (design §6.5, 方案 A) is used:
//!
//! - **Parse**: Read all remaining bytes from the Rust stream → create a
//!   Python `io.BytesIO` → call `parse_stream` → sync the consumed position
//!   back to the Rust stream via `tell()`.
//! - **Build**: Create an empty `io.BytesIO` → call `build_stream` → read
//!   `getvalue()` → write bytes to the Rust stream.
//!
//! Known limitation: large data and streaming constructs may incur high
//! memory usage. This is acceptable for Phase 10; optimisation is deferred.

use construct::core::context::Context;
use construct::core::error::{ConstructError, Result};
use construct::core::stream::Stream;
use construct::core::Construct;
use construct::value::Value;

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};

use crate::conversions::{context_to_py_container, py_to_value, value_to_py};

/// Wraps an arbitrary Python construct object as a Rust [`Construct`].
///
/// The Python object must expose `parse_stream(stream, **kw)` and
/// `build_stream(obj, stream, **kw)` methods (duck typing).
pub struct PyConstructAdapter {
    /// The Python construct object.
    py_obj: PyObject,
}

impl PyConstructAdapter {
    /// Creates a new [`PyConstructAdapter`] wrapping the given Python object.
    #[must_use]
    pub fn new(py_obj: PyObject) -> Self {
        PyConstructAdapter { py_obj }
    }

    /// Converts a [`ConstructError::Generic`] from an error display.
    fn generic_err(msg: impl std::fmt::Display) -> ConstructError {
        ConstructError::Generic {
            path: String::new(),
            message: msg.to_string(),
        }
    }

    /// Imports the `io.BytesIO` class.
    fn get_bytesio(py: Python<'_>) -> Result<PyObject> {
        py.import_bound("io")
            .and_then(|m| m.getattr("BytesIO"))
            .map(|c| c.unbind())
            .map_err(|e| Self::generic_err(format!("Failed to import io.BytesIO: {e}")))
    }

    /// Converts a Rust [`Context`] into kwargs for Python method calls.
    ///
    /// Returns `None` if the context is empty (no kwargs needed).
    fn context_to_kwargs<'py>(
        py: Python<'py>,
        ctx: &Context,
    ) -> PyResult<Option<Bound<'py, PyDict>>> {
        let py_dict = PyDict::new_bound(py);
        for (key, value) in ctx.iter_fields() {
            py_dict.set_item(key, value_to_py(py, value)?)?;
        }
        if let Some(parent) = ctx.parent() {
            let parent_container = context_to_py_container(py, parent)?;
            py_dict.set_item("_", parent_container)?;
        }
        if py_dict.is_empty() {
            Ok(None)
        } else {
            Ok(Some(py_dict))
        }
    }
}

impl std::fmt::Debug for PyConstructAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyConstructAdapter")
            .field("py_obj", &"<Python object>")
            .finish()
    }
}

impl Construct for PyConstructAdapter {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        Python::with_gil(|py| {
            // 1. Record the current stream position (before read_remaining
            //    consumes all remaining bytes).
            let start_pos = stream.tell().map_err(|e| ConstructError::Stream {
                path: String::new(),
                source: std::io::Error::new(std::io::ErrorKind::Other, e.to_string()),
            })?;

            // 2. Read remaining bytes from Rust stream.
            let remaining = stream
                .read_remaining()
                .map_err(|e| ConstructError::Stream {
                    path: String::new(),
                    source: std::io::Error::new(std::io::ErrorKind::Other, e.to_string()),
                })?;

            // 3. Create Python BytesIO with the data.
            let bytesio_class = Self::get_bytesio(py)?;
            let py_stream = bytesio_class
                .call1(py, (PyBytes::new_bound(py, &remaining),))
                .map_err(|e| Self::generic_err(format!("Failed to create BytesIO: {e}")))?;

            // 4. Prepare kwargs from context.
            let kwargs = Self::context_to_kwargs(py, ctx).map_err(Self::generic_err)?;

            // 5. Call parse_stream(stream, **kwargs).
            let result = self
                .py_obj
                .bind(py)
                .call_method(
                    "parse_stream",
                    (py_stream.bind(py),),
                    kwargs.as_ref().map(|d| d as &Bound<PyDict>),
                )
                .map_err(|e| Self::generic_err(format!("Python construct parse error: {e}")))?;

            // 6. Sync stream position: BytesIO tell() gives the number of
            //    bytes consumed within the Python BytesIO (relative to 0).
            //    The Rust stream must be set to start_pos + consumed.
            let consumed: u64 = py_stream
                .call_method0(py, "tell")
                .and_then(|v| v.extract(py))
                .map_err(|e| Self::generic_err(format!("Invalid stream position: {e}")))?;
            stream
                .seek(start_pos + consumed)
                .map_err(|e| ConstructError::Stream {
                    path: String::new(),
                    source: std::io::Error::new(std::io::ErrorKind::Other, e.to_string()),
                })?;

            // 7. Convert result → Value.
            py_to_value(py, &result).map_err(Self::generic_err)
        })
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        Python::with_gil(|py| {
            // 1. Value → Python object.
            let py_data = value_to_py(py, data).map_err(Self::generic_err)?;

            // 2. Create empty Python BytesIO for writing.
            let bytesio_class = Self::get_bytesio(py)?;
            let py_stream = bytesio_class
                .call0(py)
                .map_err(|e| Self::generic_err(format!("Failed to create BytesIO: {e}")))?;

            // 3. Prepare kwargs from context.
            let kwargs = Self::context_to_kwargs(py, ctx).map_err(Self::generic_err)?;

            // 4. Call build_stream(obj, stream, **kwargs).
            self.py_obj
                .bind(py)
                .call_method(
                    "build_stream",
                    (py_data.bind(py), py_stream.bind(py)),
                    kwargs.as_ref().map(|d| d as &Bound<PyDict>),
                )
                .map_err(|e| Self::generic_err(format!("Python construct build error: {e}")))?;

            // 5. Read bytes from BytesIO and write to Rust stream.
            let built: PyObject = py_stream
                .call_method0(py, "getvalue")
                .map_err(|e| Self::generic_err(format!("Failed to get built bytes: {e}")))?;
            let bytes_obj = built
                .bind(py)
                .downcast::<PyBytes>()
                .map_err(|_| Self::generic_err("Python build_stream did not produce bytes"))?;
            stream
                .write_bytes(bytes_obj.as_bytes())
                .map_err(|e| ConstructError::Stream {
                    path: String::new(),
                    source: std::io::Error::new(std::io::ErrorKind::Other, e.to_string()),
                })?;

            Ok(())
        })
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        Python::with_gil(|py| {
            let kwargs = Self::context_to_kwargs(py, ctx).map_err(Self::generic_err)?;

            let result = self.py_obj.bind(py).call_method(
                "sizeof",
                (),
                kwargs.as_ref().map(|d| d as &Bound<PyDict>),
            );

            match result {
                Ok(v) => {
                    let size: i64 = v
                        .extract()
                        .map_err(|e| Self::generic_err(format!("sizeof returned non-int: {e}")))?;
                    if size >= 0 {
                        Ok(size as usize)
                    } else {
                        Err(Self::generic_err(format!(
                            "sizeof returned negative: {size}"
                        )))
                    }
                }
                Err(_) => Err(ConstructError::Sizeof {
                    path: String::new(),
                    reason: "Python construct sizeof failed".to_string(),
                }),
            }
        })
    }

    fn flagbuildnone(&self) -> bool {
        Python::with_gil(|py| match self.py_obj.getattr(py, "flagbuildnone") {
            Ok(v) => v.extract::<bool>(py).unwrap_or(false),
            Err(_) => false,
        })
    }
}

/// Attempts to extract a Rust [`Construct`] from a Python object.
///
/// - **PyO3 `#[pyclass]` objects** (Rust-backed constructs): directly use
///   their inner `Box<dyn Construct>`. *(Not yet implemented — requires the
///   PyO3 wrapper types from 10.5+.)*
/// - **Pure-Python objects** (duck-typed with `parse_stream` + `build_stream`):
///   wrapped in [`PyConstructAdapter`].
///
/// This function enables the **mixed architecture** where Rust Structs,
/// Sequences, and other composites can embed user-defined Python constructs.
pub fn extract_subcon(obj: &Bound<'_, PyAny>) -> PyResult<Box<dyn Construct>> {
    // Check for duck typing: must have parse_stream and build_stream methods.
    let has_parse = obj.hasattr("parse_stream")?;
    let has_build = obj.hasattr("build_stream")?;
    if has_parse && has_build {
        let adapter = PyConstructAdapter::new(obj.clone().unbind());
        return Ok(Box::new(adapter));
    }

    let class_name = obj
        .get_type()
        .name()
        .map(|n| n.to_string())
        .unwrap_or_else(|_| "<unknown>".to_string());
    Err(pyo3::exceptions::PyTypeError::new_err(format!(
        "Expected a Construct instance (must have parse_stream and build_stream methods), got {class_name}"
    )))
}

/// Returns whether a Python object looks like a Construct (duck typing).
///
/// Checks for the presence of `parse_stream` and `build_stream` attributes.
pub fn is_construct_like(obj: &Bound<'_, PyAny>) -> bool {
    obj.hasattr("parse_stream").unwrap_or(false) && obj.hasattr("build_stream").unwrap_or(false)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use construct::core::stream::ByteStream;

    /// A simple Python construct that reads/writes 4 bytes as a u32.
    const PY_U32_CLASS: &str = r#"
class PyU32:
    flagbuildnone = False
    def parse_stream(self, stream, **kw):
        import struct
        return struct.unpack('>I', stream.read(4))[0]
    def build_stream(self, obj, stream, **kw):
        import struct
        stream.write(struct.pack('>I', obj))
    def sizeof(self, **kw):
        return 4
"#;

    #[test]
    fn py_adapter_parse_4_bytes() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let locals = pyo3::types::PyDict::new_bound(py);
            py.run_bound(PY_U32_CLASS, None, Some(&locals)).unwrap();
            let class = locals.get_item("PyU32").unwrap().unwrap();
            let instance = class.call0().unwrap();
            let adapter = PyConstructAdapter::new(instance.unbind());

            let mut stream = ByteStream::new_read(&[0x00, 0x00, 0x00, 0x2A]);
            let mut ctx = Context::new();
            let result = adapter.parse(&mut stream, &mut ctx).unwrap();
            assert_eq!(result, Value::Int(42));
        });
    }

    #[test]
    fn py_adapter_build_4_bytes() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let locals = pyo3::types::PyDict::new_bound(py);
            py.run_bound(PY_U32_CLASS, None, Some(&locals)).unwrap();
            let class = locals.get_item("PyU32").unwrap().unwrap();
            let instance = class.call0().unwrap();
            let adapter = PyConstructAdapter::new(instance.unbind());

            let mut stream = ByteStream::new_write();
            let mut ctx = Context::new();
            adapter
                .build(&Value::UInt(0xDEADBEEF), &mut stream, &mut ctx)
                .unwrap();
            assert_eq!(stream.into_bytes(), vec![0xDE, 0xAD, 0xBE, 0xEF]);
        });
    }

    #[test]
    fn py_adapter_sizeof() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let locals = pyo3::types::PyDict::new_bound(py);
            py.run_bound(PY_U32_CLASS, None, Some(&locals)).unwrap();
            let class = locals.get_item("PyU32").unwrap().unwrap();
            let instance = class.call0().unwrap();
            let adapter = PyConstructAdapter::new(instance.unbind());

            let ctx = Context::new();
            assert_eq!(adapter.sizeof(&ctx).unwrap(), 4);
        });
    }

    #[test]
    fn py_adapter_parse_stream_sync() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let locals = pyo3::types::PyDict::new_bound(py);
            py.run_bound(PY_U32_CLASS, None, Some(&locals)).unwrap();
            let class = locals.get_item("PyU32").unwrap().unwrap();
            let instance = class.call0().unwrap();
            let adapter = PyConstructAdapter::new(instance.unbind());

            // 8 bytes: parse 4, leave 4 remaining.
            let mut stream =
                ByteStream::new_read(&[0x00, 0x01, 0x02, 0x03, 0xFF, 0xFF, 0xFF, 0xFF]);
            let mut ctx = Context::new();
            let result = adapter.parse(&mut stream, &mut ctx).unwrap();
            assert_eq!(result, Value::Int(0x00010203));
            // Stream should be at position 4.
            assert_eq!(stream.tell().unwrap(), 4);
        });
    }

    #[test]
    fn py_adapter_build_then_parse_roundtrip() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let locals = pyo3::types::PyDict::new_bound(py);
            py.run_bound(PY_U32_CLASS, None, Some(&locals)).unwrap();
            let class = locals.get_item("PyU32").unwrap().unwrap();
            let instance = class.call0().unwrap();
            let adapter = PyConstructAdapter::new(instance.unbind());

            // Build
            let mut build_stream = ByteStream::new_write();
            let mut build_ctx = Context::new();
            adapter
                .build(&Value::UInt(999), &mut build_stream, &mut build_ctx)
                .unwrap();
            let bytes = build_stream.into_bytes();

            // Parse back
            let mut parse_stream = ByteStream::new_read(&bytes);
            let mut parse_ctx = Context::new();
            let parsed = adapter.parse(&mut parse_stream, &mut parse_ctx).unwrap();
            assert_eq!(parsed, Value::Int(999));
        });
    }

    #[test]
    fn extract_subcon_valid() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let locals = pyo3::types::PyDict::new_bound(py);
            py.run_bound(PY_U32_CLASS, None, Some(&locals)).unwrap();
            let class = locals.get_item("PyU32").unwrap().unwrap();
            let instance = class.call0().unwrap();

            let con = extract_subcon(&instance).unwrap();
            let mut stream = ByteStream::new_read(&[0x00, 0x00, 0x00, 0x05]);
            let mut ctx = Context::new();
            let result = con.parse(&mut stream, &mut ctx).unwrap();
            assert_eq!(result, Value::Int(5));
        });
    }

    #[test]
    fn extract_subcon_invalid() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let val = 42i64.into_py(py);
            let result = extract_subcon(val.bind(py));
            assert!(result.is_err());
            let err = result.err().unwrap();
            assert!(err.is_instance_of::<pyo3::exceptions::PyTypeError>(py));
        });
    }

    #[test]
    fn is_construct_like_true() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let locals = pyo3::types::PyDict::new_bound(py);
            py.run_bound(PY_U32_CLASS, None, Some(&locals)).unwrap();
            let class = locals.get_item("PyU32").unwrap().unwrap();
            let instance = class.call0().unwrap();
            assert!(is_construct_like(&instance));
        });
    }

    #[test]
    fn is_construct_like_false() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let val = 42i64.into_py(py);
            assert!(!is_construct_like(val.bind(py)));
        });
    }
}
