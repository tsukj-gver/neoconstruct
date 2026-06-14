//! Helper macros for PyO3 construct wrappers.
//!
//! - [`impl_api_methods!`] generates the 7 public API methods (parse,
//!   parse_stream, parse_file, build, build_stream, build_file, sizeof) for a
//!   wrapper type.
//! - [`impl_construct_operators!`] generates the 6 operator dunder methods
//!   (`__rtruediv__`, `__mul__`, `__rmul__`, `__add__`, `__rshift__`,
//!   `__getitem__`).

use construct::core::Construct;

/// Unified interface for PyO3 wrapper classes that hold a Rust [`Construct`].
///
/// Each `#[pyclass]` wrapper implements this trait so that the helper macros
/// and `extract_subcon` can uniformly access the inner Rust construct.
pub trait PyConstructWrapper {
    /// Returns a reference to the inner Rust [`Construct`].
    fn as_construct(&self) -> &dyn Construct;
}

/// Generates the 7 public API methods for a PyO3 wrapper type.
///
/// The type must implement [`PyConstructWrapper`].
///
/// ```ignore
/// impl_api_methods!(PyFormatField);
/// ```
#[macro_export]
macro_rules! impl_api_methods {
    ($pyclass:ty) => {
        #[pymethods]
        impl $pyclass {
            /// Parse a bytes object into a Python value.
            #[pyo3(signature = (data, **kw))]
            fn parse(
                &self,
                py: Python<'_>,
                data: &Bound<PyAny>,
                kw: Option<&Bound<pyo3::types::PyDict>>,
            ) -> PyResult<PyObject> {
                use $crate::construct_macros::PyConstructWrapper;
                let bytes: std::borrow::Cow<[u8]> = data.extract()?;
                let ctx = $crate::api::opt_kw_to_indexmap(py, kw)?;
                $crate::api::py_parse(
                    <Self as PyConstructWrapper>::as_construct(self),
                    py,
                    &bytes,
                    ctx,
                )
            }

            /// Parse from a file-like stream object.
            #[pyo3(signature = (stream, **kw))]
            fn parse_stream(
                &self,
                py: Python<'_>,
                stream: PyObject,
                kw: Option<&Bound<pyo3::types::PyDict>>,
            ) -> PyResult<PyObject> {
                use $crate::construct_macros::PyConstructWrapper;
                let ctx = $crate::api::opt_kw_to_indexmap(py, kw)?;
                $crate::api::py_parse_stream(
                    <Self as PyConstructWrapper>::as_construct(self),
                    py,
                    stream,
                    ctx,
                )
            }

            /// Parse from a file path.
            #[pyo3(signature = (filename, **kw))]
            fn parse_file(
                &self,
                py: Python<'_>,
                filename: &str,
                kw: Option<&Bound<pyo3::types::PyDict>>,
            ) -> PyResult<PyObject> {
                use $crate::construct_macros::PyConstructWrapper;
                let ctx = $crate::api::opt_kw_to_indexmap(py, kw)?;
                $crate::api::py_parse_file(
                    <Self as PyConstructWrapper>::as_construct(self),
                    py,
                    filename,
                    ctx,
                )
            }

            /// Build a Python value into bytes.
            #[pyo3(signature = (obj, **kw))]
            fn build(
                &self,
                py: Python<'_>,
                obj: &Bound<PyAny>,
                kw: Option<&Bound<pyo3::types::PyDict>>,
            ) -> PyResult<PyObject> {
                use $crate::construct_macros::PyConstructWrapper;
                let ctx = $crate::api::opt_kw_to_indexmap(py, kw)?;
                $crate::api::py_build(
                    <Self as PyConstructWrapper>::as_construct(self),
                    py,
                    obj,
                    ctx,
                )
            }

            /// Build a Python value and write to a file-like stream.
            #[pyo3(signature = (obj, stream, **kw))]
            fn build_stream(
                &self,
                py: Python<'_>,
                obj: &Bound<PyAny>,
                stream: PyObject,
                kw: Option<&Bound<pyo3::types::PyDict>>,
            ) -> PyResult<()> {
                use $crate::construct_macros::PyConstructWrapper;
                let ctx = $crate::api::opt_kw_to_indexmap(py, kw)?;
                $crate::api::py_build_stream(
                    <Self as PyConstructWrapper>::as_construct(self),
                    py,
                    obj,
                    stream,
                    ctx,
                )
            }

            /// Build a Python value and write to a file.
            #[pyo3(signature = (obj, filename, **kw))]
            fn build_file(
                &self,
                py: Python<'_>,
                obj: &Bound<PyAny>,
                filename: &str,
                kw: Option<&Bound<pyo3::types::PyDict>>,
            ) -> PyResult<()> {
                use $crate::construct_macros::PyConstructWrapper;
                let ctx = $crate::api::opt_kw_to_indexmap(py, kw)?;
                $crate::api::py_build_file(
                    <Self as PyConstructWrapper>::as_construct(self),
                    py,
                    obj,
                    filename,
                    ctx,
                )
            }

            /// Compute the static byte size.
            #[pyo3(signature = (**kw))]
            fn sizeof(
                &self,
                py: Python<'_>,
                kw: Option<&Bound<pyo3::types::PyDict>>,
            ) -> PyResult<usize> {
                use $crate::construct_macros::PyConstructWrapper;
                let ctx = $crate::api::opt_kw_to_indexmap(py, kw)?;
                $crate::api::py_sizeof(<Self as PyConstructWrapper>::as_construct(self), py, ctx)
            }
        }
    };
}

/// Generates the 6 operator dunder methods for a PyO3 wrapper type.
///
/// The type must implement [`PyConstructWrapper`] and be a `#[pyclass]`.
#[macro_export]
macro_rules! impl_construct_operators {
    ($pyclass:ty) => {
        #[pymethods]
        impl $pyclass {
            /// `"name" / self` → Renamed
            fn __rtruediv__(
                slf: PyRef<Self>,
                py: Python<'_>,
                name: &Bound<PyAny>,
            ) -> PyResult<PyObject> {
                if let Ok(s) = name.extract::<String>() {
                    let self_obj: PyObject = slf.into_py(py);
                    return $crate::py_renamed::make_renamed(py, self_obj, &s, None, None);
                }
                Ok(py.NotImplemented())
            }

            /// `self * other` → Renamed with docs or parsed hook
            fn __mul__(
                slf: PyRef<Self>,
                py: Python<'_>,
                other: &Bound<PyAny>,
            ) -> PyResult<PyObject> {
                let self_obj: PyObject = slf.into_py(py);
                $crate::py_renamed::py_mul_operator(py, self_obj, other)
            }

            /// `other * self` → Renamed with docs or parsed hook
            fn __rmul__(
                slf: PyRef<Self>,
                py: Python<'_>,
                other: &Bound<PyAny>,
            ) -> PyResult<PyObject> {
                let self_obj: PyObject = slf.into_py(py);
                $crate::py_renamed::py_mul_operator(py, self_obj, other)
            }

            /// `self + other` → Struct
            fn __add__(
                slf: PyRef<Self>,
                py: Python<'_>,
                other: &Bound<PyAny>,
            ) -> PyResult<PyObject> {
                let self_obj: PyObject = slf.into_py(py);
                $crate::constructs_composite::make_struct_from_add(py, self_obj, other)
            }

            /// `self >> other` → Sequence
            fn __rshift__(
                slf: PyRef<Self>,
                py: Python<'_>,
                other: &Bound<PyAny>,
            ) -> PyResult<PyObject> {
                let self_obj: PyObject = slf.into_py(py);
                $crate::constructs_composite::make_sequence_from_rshift(py, self_obj, other)
            }

            /// `self[n]` → Array
            fn __getitem__(
                slf: PyRef<Self>,
                py: Python<'_>,
                count: &Bound<PyAny>,
            ) -> PyResult<PyObject> {
                let self_obj: PyObject = slf.into_py(py);
                $crate::constructs_composite::make_array_from_getitem(py, self_obj, count)
            }
        }
    };
}
