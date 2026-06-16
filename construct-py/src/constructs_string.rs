//! PyO3 wrappers for string constructs (PaddedString, CString).
//!
//! - [`PyPaddedString`] wraps the Rust [`PaddedString`] construct.
//! - [`PyCString`] wraps the Rust [`CString`] construct.
//!
//! Both expose the standard 7 API methods (via [`impl_api_methods!`]) and
//! the 6 operator dunders (via [`impl_construct_operators!`]).

use construct::constructs::strings::{CString, PaddedString, StringEncoding};
use construct::core::Construct;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use crate::construct_macros::PyConstructWrapper;
use crate::exceptions::StringError;

// ===========================================================================
// Encoding name mapping
// ===========================================================================

/// Maps a Python encoding name string to a Rust [`StringEncoding`] enum value.
///
/// Normalisation: lowercase + replace `-` with `_`.
///
/// # Errors
///
/// Returns [`StringError`] if the encoding name is not recognised.
fn encoding_name_to_enum(encoding: &str) -> PyResult<StringEncoding> {
    let normalized = encoding.to_lowercase().replace('-', "_");
    match normalized.as_str() {
        "ascii" => Ok(StringEncoding::Ascii),
        "utf8" | "utf_8" | "u8" => Ok(StringEncoding::Utf8),
        "utf16" | "utf_16" | "u16" => Ok(StringEncoding::Utf16),
        "utf_16_be" => Ok(StringEncoding::Utf16Be),
        "utf_16_le" => Ok(StringEncoding::Utf16Le),
        "utf32" | "utf_32" | "u32" => Ok(StringEncoding::Utf32),
        "utf_32_be" => Ok(StringEncoding::Utf32Be),
        "utf_32_le" => Ok(StringEncoding::Utf32Le),
        other => Err(StringError::new_err(format!(
            "encoding {:?} not found among supported encodings",
            other
        ))),
    }
}

// ===========================================================================
// PyPaddedString
// ===========================================================================

/// PyO3 wrapper: fixed-length null-padded string.
///
/// Corresponds to Python `PaddedString(length, encoding)`.
#[pyclass(name = "PaddedString", unsendable)]
pub struct PyPaddedString {
    /// The Rust PaddedString construct.
    pub(crate) inner: PaddedString,
}

impl PyConstructWrapper for PyPaddedString {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyPaddedString {
    /// Reconstructs an owned `Box<dyn Construct>` from this wrapper.
    ///
    /// Since [`PaddedString`] has only `Copy` fields, this simply creates
    /// a new instance with the same parameters.
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Ok(Box::new(PaddedString::new(
            self.inner.length,
            self.inner.encoding,
        )))
    }
}

/// Factory: `PaddedString(length, encoding)`.
///
/// - If `length` is an int, creates a Rust-backed [`PyPaddedString`].
/// - If `length` is callable (context lambda), falls back to the pure-Python
///   macro `_padded_string_fallback` (PM decision: 方案 A).
#[pyfunction]
#[pyo3(name = "PaddedString")]
pub fn py_padded_string(
    py: Python<'_>,
    length: &Bound<PyAny>,
    encoding: &str,
) -> PyResult<PyObject> {
    if length.is_callable() {
        // PM decision (方案 A): fallback to pure-Python macro.
        let string_module = py.import_bound("construct_rust._string")?;
        let fallback = string_module.getattr("_padded_string_fallback")?;
        let result = fallback.call1((length, encoding))?;
        return Ok(result.unbind());
    }

    let len: usize = length.extract().map_err(|_| {
        PyValueError::new_err(format!(
            "PaddedString length must be an int or callable, got {}",
            length
                .get_type()
                .name()
                .map(|n| n.to_string())
                .unwrap_or_default()
        ))
    })?;
    let enc = encoding_name_to_enum(encoding)?;
    let wrapper = PyPaddedString {
        inner: PaddedString::new(len, enc),
    };
    Ok(Py::new(py, wrapper)?.into_any())
}

crate::impl_api_methods!(PyPaddedString);
crate::impl_construct_operators!(PyPaddedString);

// ===========================================================================
// PyCString
// ===========================================================================

/// PyO3 wrapper: null-terminated string.
///
/// Corresponds to Python `CString(encoding)`.
#[pyclass(name = "CString", unsendable)]
pub struct PyCString {
    /// The Rust CString construct.
    pub(crate) inner: CString,
}

impl PyConstructWrapper for PyCString {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyCString {
    /// Reconstructs an owned `Box<dyn Construct>` from this wrapper.
    ///
    /// Since [`CString`] has only `Copy` fields, this simply creates
    /// a new instance with the same encoding.
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Ok(Box::new(CString::new(self.inner.encoding)))
    }
}

/// Factory: `CString(encoding)`.
#[pyfunction]
#[pyo3(name = "CString")]
pub fn py_c_string(encoding: &str) -> PyResult<PyCString> {
    let enc = encoding_name_to_enum(encoding)?;
    Ok(PyCString {
        inner: CString::new(enc),
    })
}

crate::impl_api_methods!(PyCString);
crate::impl_construct_operators!(PyCString);

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_name_ascii() {
        assert_eq!(
            encoding_name_to_enum("ascii").unwrap(),
            StringEncoding::Ascii
        );
    }

    #[test]
    fn encoding_name_utf8_aliases() {
        assert_eq!(encoding_name_to_enum("utf8").unwrap(), StringEncoding::Utf8);
        assert_eq!(
            encoding_name_to_enum("utf_8").unwrap(),
            StringEncoding::Utf8
        );
        assert_eq!(encoding_name_to_enum("u8").unwrap(), StringEncoding::Utf8);
        assert_eq!(
            encoding_name_to_enum("UTF-8").unwrap(),
            StringEncoding::Utf8
        );
    }

    #[test]
    fn encoding_name_utf16_be_le() {
        assert_eq!(
            encoding_name_to_enum("utf_16_be").unwrap(),
            StringEncoding::Utf16Be
        );
        assert_eq!(
            encoding_name_to_enum("utf-16-le").unwrap(),
            StringEncoding::Utf16Le
        );
    }

    #[test]
    fn encoding_name_unknown_raises() {
        assert!(encoding_name_to_enum("unknown").is_err());
    }

    #[test]
    fn py_cstring_make_owned() {
        let wrapper = PyCString {
            inner: CString::new(StringEncoding::Utf8),
        };
        let owned = wrapper.make_owned().unwrap();
        // Verify the owned construct works independently.
        let c: &dyn Construct = owned.as_ref();
        let mut stream = construct::core::stream::CombinedStream::ByteStream(
            construct::core::stream::ByteStream::new_read(b"hi\x00"),
        );
        let mut ctx = construct::core::context::Context::new();
        let result = c.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, construct::value::Value::String("hi".to_string()));
    }

    #[test]
    fn py_padded_string_make_owned() {
        let wrapper = PyPaddedString {
            inner: PaddedString::new(10, StringEncoding::Utf8),
        };
        let owned = wrapper.make_owned().unwrap();
        let ctx = construct::core::context::Context::new();
        assert_eq!(owned.sizeof(&ctx).unwrap(), 10);
    }
}
