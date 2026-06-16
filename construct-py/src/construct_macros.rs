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

            /// No-op compile for API compatibility with Python construct.
            ///
            /// The Rust kernel is already compiled, so this returns ``self``.
            fn compile(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
                slf
            }

            /// Whether this construct can build from ``None`` (no value needed).
            #[getter]
            fn flagbuildnone(&self) -> bool {
                use $crate::construct_macros::PyConstructWrapper;
                <Self as PyConstructWrapper>::as_construct(self).flagbuildnone()
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
                // `None / self` → anonymous (return self unwrapped)
                if name.is_none() {
                    let self_obj: PyObject = slf.into_py(py);
                    return Ok(self_obj);
                }
                if let Ok(s) = name.extract::<String>() {
                    let self_obj: PyObject = slf.into_py(py);
                    return $crate::py_renamed::make_renamed(py, self_obj, &s, None, None);
                }
                if let Ok(b) = name.extract::<Vec<u8>>() {
                    let s = String::from_utf8_lossy(&b).into_owned();
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

/// Generates a default `__repr__` method returning `<ClassName>`.
///
/// Use this for simple constructs whose Python repr is just the class name
/// in angle brackets (e.g. `Byte` → `<FormatField>`). Types with custom
/// repr needs (Struct, Sequence, Renamed) should define `__repr__` manually.
#[macro_export]
macro_rules! impl_default_repr {
    ($pyclass:ty, $class_name:literal) => {
        #[pymethods]
        impl $pyclass {
            /// Default repr: `<ClassName>` (matching Python construct format).
            fn __repr__(&self) -> String {
                use $crate::construct_macros::PyConstructWrapper;
                let flag = <Self as PyConstructWrapper>::as_construct(self).flagbuildnone();
                let nb = if flag { " +nonbuild" } else { "" };
                format!("<{}{}>", $class_name, nb)
            }
        }
    };
}

/// Generates a `__repr__` method that includes a subcon's repr.
///
/// For Adapter/Subconstruct-style classes: `<ClassName +nonbuild <SubconRepr>>`.
#[macro_export]
macro_rules! impl_subcon_repr {
    ($pyclass:ty, $class_name:literal) => {
        #[pymethods]
        impl $pyclass {
            /// Repr including nested subcon: `<ClassName <SubconRepr>>`.
            fn __repr__(&self, py: Python<'_>) -> String {
                use $crate::construct_macros::PyConstructWrapper;
                let flag = <Self as PyConstructWrapper>::as_construct(self).flagbuildnone();
                let nb = if flag { " +nonbuild" } else { "" };
                let sub_repr = self
                    .subcon_obj
                    .call_method0(py, "__repr__")
                    .and_then(|r| r.extract::<String>(py))
                    .unwrap_or_else(|_| format!("<{}>", $class_name));
                format!("<{}{} {}>", $class_name, nb, sub_repr)
            }
        }
    };
}
