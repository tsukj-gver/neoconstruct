//! [`PyRenamed`] — PyO3 wrapper for the Rust [`construct::core::Renamed`] type.
//!
//! Also contains operator helper functions used by [`impl_construct_operators!`]:
//! - [`make_renamed`] — creates a `PyRenamed` from a subcon Python object.
//! - [`py_mul_operator`] — handles the `*` operator (docs or parsed hook).
//! - [`get_subcon_name`] — extracts the name from a Python construct object.
//! - [`apply_rename`] — applies `name / subcon` for `**subconskw` processing.

use construct::core::context::Context;
use construct::core::Construct;
use construct::value::Value;

use pyo3::exceptions::PyAttributeError;
use pyo3::prelude::*;

use crate::conversions::{context_to_py_container, value_to_py};
use crate::exceptions::ConstructError;
use crate::py_adapter::extract_subcon;

/// PyO3 wrapper for [`construct::core::Renamed`].
///
/// Produced by the `/` and `*` operators. Holds a reference to the original
/// Python subcon so that [`extract_subcon`] can re-extract an owned
/// `Box<dyn Construct>` when needed.
#[pyclass(name = "Renamed", unsendable)]
pub struct PyRenamed {
    /// The Rust Renamed construct.
    pub(crate) inner: construct::core::Renamed,
    /// The original Python subcon object (for re-extraction and __getattr__).
    pub(crate) py_subcon: PyObject,
}

impl crate::construct_macros::PyConstructWrapper for PyRenamed {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyRenamed {
    /// Creates a [`PyRenamed`] from the given components.
    ///
    /// The `subcon_obj` is the original Python subcon. The inner Rust
    /// [`construct::core::Renamed`] wraps a [`PyConstructAdapter`] that delegates
    /// back to `subcon_obj` for parse/build.
    #[allow(clippy::needless_pass_by_value)]
    pub fn create(
        py: Python<'_>,
        subcon_obj: PyObject,
        renamed: construct::core::Renamed,
    ) -> PyResult<Py<Self>> {
        Py::new(
            py,
            PyRenamed {
                inner: renamed,
                py_subcon: subcon_obj,
            },
        )
    }
}

// ===========================================================================
// Operator helper functions
// ===========================================================================

/// Creates a `PyRenamed` from a subcon Python object and a name.
///
/// This is the core of the `__rtruediv__` operator: `"name" / subcon`.
///
/// # Parameters
///
/// - `subcon_obj`: the Python subcon object (e.g. a `PyFormatField`).
/// - `name`: the field name.
/// - `docs`: optional docstring (from the `*` operator).
/// - `parsed`: optional parsed hook callable (from the `*` operator).
#[allow(clippy::needless_pass_by_value)]
pub fn make_renamed(
    py: Python<'_>,
    subcon_obj: PyObject,
    name: &str,
    docs: Option<String>,
    parsed: Option<PyObject>,
) -> PyResult<PyObject> {
    let inner = extract_subcon(subcon_obj.bind(py))?;
    let mut renamed = construct::core::Renamed::new(inner, name);
    if let Some(d) = docs {
        renamed = renamed.with_docs(d);
    }
    if let Some(hook_obj) = parsed {
        let hook = py_to_parsed_hook(hook_obj)?;
        renamed = renamed.with_parsed(hook);
    }
    let py_renamed = PyRenamed::create(py, subcon_obj, renamed)?;
    Ok(py_renamed.into_any())
}

/// Handles the `*` operator: `subcon * other` or `other * subcon`.
///
/// - If `other` is a string, sets `docs` on the Renamed.
/// - If `other` is a callable, sets a `parsed` hook on the Renamed.
/// - Otherwise, raises a `ConstructError`.
pub fn py_mul_operator(
    py: Python<'_>,
    subcon_obj: PyObject,
    other: &Bound<PyAny>,
) -> PyResult<PyObject> {
    if let Ok(s) = other.extract::<String>() {
        // str → docs
        let inner = extract_subcon(subcon_obj.bind(py))?;
        let renamed = construct::core::Renamed::new(inner, "").with_docs(s);
        let py_r = PyRenamed::create(py, subcon_obj, renamed)?;
        return Ok(py_r.into_any());
    }
    if other.is_callable() {
        // callable → parsed hook
        let hook_obj: PyObject = other.clone().unbind();
        let hook = py_to_parsed_hook(hook_obj.clone_ref(py))?;
        let inner = extract_subcon(subcon_obj.bind(py))?;
        let renamed = construct::core::Renamed::new(inner, "").with_parsed(hook);
        let py_r = PyRenamed::create(py, subcon_obj, renamed)?;
        return Ok(py_r.into_any());
    }
    Err(ConstructError::new_err(
        "operator * can only be used with a string (docstring) or a callable (parsed hook)",
    ))
}

/// Converts a Python callable into a parsed hook
/// (`Fn(&Value, &Context) + Send + Sync`).
pub fn py_to_parsed_hook(
    callable: PyObject,
) -> PyResult<Box<dyn Fn(&Value, &Context) + Send + Sync>> {
    Ok(Box::new(move |obj: &Value, ctx: &Context| {
        let _ = Python::with_gil(|py| {
            let py_obj = value_to_py(py, obj).ok()?;
            let py_ctx = context_to_py_container(py, ctx).ok()?;
            callable.call1(py, (py_obj, py_ctx)).ok()
        });
    }))
}

/// Extracts the name from a Python construct object.
///
/// Returns `Some(name)` for named constructs (non-empty, non-`"_"`), or
/// `None` for anonymous constructs.
pub fn get_subcon_name(obj: &Bound<PyAny>) -> PyResult<Option<String>> {
    // Try the .name attribute (works for PyRenamed and pure-Python Renamed).
    if let Ok(name_attr) = obj.getattr("name") {
        if name_attr.is_none() {
            return Ok(None);
        }
        if let Ok(s) = name_attr.extract::<String>() {
            if s.is_empty() || s == "_" {
                return Ok(None);
            }
            return Ok(Some(s));
        }
    }
    Ok(None)
}

/// Applies `name / subcon` and returns the resulting PyRenamed as a PyObject.
///
/// Used for processing `**subconskw` keyword arguments in Struct/Sequence.
#[allow(clippy::needless_pass_by_value)]
pub fn apply_rename(py: Python<'_>, subcon: &Bound<PyAny>, name: &str) -> PyResult<PyObject> {
    let subcon_obj: PyObject = subcon.clone().unbind();
    make_renamed(py, subcon_obj, name, None, None)
}

// ===========================================================================
// PyRenamed pymethods
// ===========================================================================

#[pymethods]
impl PyRenamed {
    /// Python constructor: `Renamed(subcon, newname, newdocs, newparsed)`.
    #[new]
    #[pyo3(signature = (subcon, newname=None, newdocs=None, newparsed=None))]
    fn new(
        _py: Python<'_>,
        subcon: &Bound<PyAny>,
        newname: Option<&str>,
        newdocs: Option<&str>,
        newparsed: Option<&Bound<PyAny>>,
    ) -> PyResult<Self> {
        let subcon_obj: PyObject = subcon.clone().unbind();
        let name = newname.unwrap_or("").to_string();
        let inner = extract_subcon(subcon)?;
        let mut renamed = construct::core::Renamed::new(inner, name);
        if let Some(d) = newdocs {
            renamed = renamed.with_docs(d);
        }
        if let Some(h) = newparsed {
            let hook_obj: PyObject = h.clone().unbind();
            let hook = py_to_parsed_hook(hook_obj)?;
            renamed = renamed.with_parsed(hook);
        }
        Ok(PyRenamed {
            inner: renamed,
            py_subcon: subcon_obj,
        })
    }

    /// `.name` attribute — the field name.
    #[getter]
    fn name(&self) -> &str {
        &self.inner.name
    }

    /// `.docs` attribute — the docstring (if set).
    #[getter]
    fn docs(&self) -> Option<&str> {
        self.inner.docs.as_deref()
    }

    /// `.subcon` attribute — the original subcon.
    #[getter]
    fn subcon(&self, py: Python<'_>) -> PyObject {
        self.py_subcon.clone_ref(py)
    }

    /// Delegates attribute access to the inner subcon.
    fn __getattr__(&self, py: Python<'_>, name: &str) -> PyResult<PyObject> {
        self.py_subcon
            .getattr(py, name)
            .map_err(|_| PyAttributeError::new_err(format!("Renamed has no attribute '{name}'")))
    }

    fn __repr__(&self, py: Python<'_>) -> String {
        let class = self
            .py_subcon
            .getattr(py, "__class__")
            .and_then(|c| c.getattr(py, "__name__"))
            .and_then(|n| n.extract::<String>(py))
            .unwrap_or_else(|_| "Unknown".to_string());
        format!("Renamed({}, {:?})", class, self.inner.name)
    }
}

// Apply API methods and operators to PyRenamed.
crate::impl_api_methods!(PyRenamed);
crate::impl_construct_operators!(PyRenamed);
