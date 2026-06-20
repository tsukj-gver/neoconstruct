//! Python-direct input source for the build direction (Phase 16).
//!
//! This module defines [`PyInput`] — an input abstraction that reads field
//! values directly from Python objects (`dict` / dataclass / `list`) during
//! `exec_build_py`, **without** converting the entire input tree to `Value`
//! upfront. Each field is extracted lazily, one FFI call at a time, with
//! schema-guided type knowledge (no 7× type-check chain).
//!
//! # `python` feature gate
//!
//! This entire module is compiled only under `--features python`. Without
//! it, the pure-Rust [`ValueInput`](super::ValueInput) path is used
//! unchanged.
//
// NOTE: The `#[cfg(feature = "python")]` gate lives on the `pub mod` line in
// `mod.rs`; an inner `#![cfg(...)]` would duplicate it and trigger
// `clippy::duplicated_attributes`.

use pyo3::ffi as pyffi;
use pyo3::prelude::*;
use pyo3::types::PyBool;

use crate::core::error::{ConstructError, Result};
use crate::value::Value;

// ===========================================================================
// PyInput trait
// ===========================================================================

/// Python-direct input source for build-direction traversal.
///
/// Unlike the old `PyInput` (construct-py), which converted the entire
/// input to `Value` via `py_to_value` on every access, this trait provides:
/// - Per-field `PyObject` access (no full-tree conversion).
/// - Schema-guided extraction at leaf level (type-known extract, no 7×
///   type-check chain).
/// - Optional batch extraction for homogeneous integer arrays (raw CPython
///   API, see [`extract_batch_int_raw`]).
///
/// # Implementations
///
/// - [`PyDictInput`] — wraps an arbitrary `PyObject` (dict, dataclass,
///   list, or scalar). The standard build-direction input.
/// - [`PyScalarInput`] — wraps a known `Value`. Used after batch extraction
///   to feed each pre-extracted scalar back into the per-element build loop
///   without re-crossing the FFI boundary.
pub trait PyInput: std::fmt::Debug {
    /// Extracts this input as a scalar `Value` for leaf build.
    ///
    /// Leaf nodes call this to obtain the value to encode. Internally uses
    /// short-circuit type extraction (bool, then int, float, bytes, string,
    /// none), **not** the generic 7x type-check chain.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::TypeMismatch`] if the wrapped object is not
    /// a scalar (e.g. it is a `dict` or `list`).
    fn extract_scalar_py(&self, py: Python<'_>) -> Result<Value>;

    /// Returns whether a named field is present and non-`None`.
    ///
    /// Used by composite nodes to implement `flagbuildnone` semantics.
    fn has_field_py(&self, py: Python<'_>, name: &str) -> bool;

    /// Returns a sub-input for a named field (Struct build).
    ///
    /// Tries `__getitem__` first (dict access), then falls back to `getattr`
    /// (dataclass / attribute access). This fallback chain is required for
    /// dataclass build inputs (C-IMP-1).
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::FieldMissing`] if the field is absent under
    /// both access modes.
    fn sub_field_py(&self, py: Python<'_>, name: &str) -> Result<Box<dyn PyInput>>;

    /// Returns a sub-input for a sequential index (Array build).
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::Index`] if `index` is out of bounds.
    fn sub_index_py(&self, py: Python<'_>, index: usize) -> Result<Box<dyn PyInput>>;

    /// Returns the number of items (for repetition constructs).
    fn len_py(&self, py: Python<'_>) -> usize;

    /// Schema-guided batch extraction for homogeneous integer arrays.
    ///
    /// If the input is a `list` of integers, extracts all elements into a
    /// `Vec<i64>` in one pass using raw CPython API
    /// ([`extract_batch_int_raw`]), avoiding per-element `Box<dyn PyInput>`
    /// allocation and vtable dispatch.
    ///
    /// # Errors
    ///
    /// Returns `Ok(None)` (not an error) if batch extraction is not
    /// applicable (input is not a list, or elements are not all integers).
    /// The caller falls back to per-index extraction.
    fn extract_batch_int_py(&self, py: Python<'_>) -> Result<Option<Vec<i64>>>;

    /// Extracts a [`Value`] suitable for context insertion.
    ///
    /// Unlike [`extract_scalar_py`](Self::extract_scalar_py), which fails on
    /// composite inputs (dict / list), this method handles both:
    /// - **Scalar inputs** (int, str, bytes, …): returned directly.
    /// - **Composite inputs** (dict, list): shallow-extracted via
    ///   [`shallow_py_to_value_for_ctx`] — top-level scalar fields are
    ///   extracted, nested composites become empty placeholders.
    ///
    /// Used by composite nodes (`Struct`, `Sequence`, `Array`, …) and wrapper
    /// nodes (`Adapter`, `Const`, `Enum`, …) to obtain the value for context
    /// expression evaluation (`this.field`) during build.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::TypeMismatch`] only if the object is neither
    /// a scalar nor a recognised composite type.
    fn extract_ctx_value_py(&self, py: Python<'_>) -> Result<Value>;
}

// ===========================================================================
// extract_batch_int_raw — raw CPython API batch extraction
// ===========================================================================

/// Raw CPython API batch extraction of `Vec<i64>` from a Python `list`.
///
/// Uses `PyList_GET_SIZE` + `PyList_GET_ITEM` (macros / direct `ob_item`
/// array access) + `PyLong_AsLongLong`, bypassing pyo3's
/// `PySequence_GetItem` method-resolution overhead.
///
/// # Advantage over pyo3 `extract::<Vec<i64>>`
///
/// pyo3's `Vec<T>` extraction uses `PySequence_GetItem` (~25ns/element,
/// method resolution) + `extract::<i64>` wrapper (~40ns/element). This raw
/// path uses the `PyList_GET_ITEM` macro (~2ns, direct array access) +
/// `PyLong_AsLongLong` (~30ns), saving the method-resolution cost per
/// element (~20ns × N).
///
/// # Errors
///
/// Returns `Ok(None)` (not an error) if:
/// - The input is not a `list` (or `list` subclass like `ListContainer`).
/// - Any element is not an `int` or overflows `i64`.
///
/// The caller should fall back to per-index extraction on `None`.
///
/// # Safety
///
/// This function uses raw CPython FFI calls. The caller must ensure:
/// - `py` (the GIL token) is held for the entire duration — `obj` must not
///   be garbage-collected mid-iteration. This is guaranteed by the
///   `py: Python<'_>` parameter lifetime.
/// - `obj` is not mutated during the call. Since build inputs are
///   user-provided and not concurrently modified, this holds.
/// - `PyList_GET_ITEM` returns a **borrowed** reference (no refcount inc).
///   The borrowed pointer is only used to immediately call
///   `PyLong_AsLongLong`, which copies the integer value out — the borrow
///   does not outlive the loop iteration. The list itself is kept alive by
///   the `Bound` borrow on `obj`.
pub(crate) unsafe fn extract_batch_int_raw(
    _py: Python<'_>,
    obj: &Bound<'_, PyAny>,
) -> Result<Option<Vec<i64>>> {
    let ptr = obj.as_ptr();
    // PyList_Check accepts subclasses (ListContainer is a list subclass).
    // Returns 1 for lists, 0 otherwise.
    if pyffi::PyList_Check(ptr) == 0 {
        return Ok(None);
    }
    // PyList_Size returns the length (function form of the PyList_GET_SIZE
    // macro; pyo3 ffi only exposes the function form).
    let len = pyffi::PyList_Size(ptr) as usize;
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        // PyList_GetItem returns a borrowed *mut PyObject (no refcount
        // change). Safe here because `obj` is borrowed for the function's
        // lifetime and not mutated; `i` is in range [0, len) as returned by
        // PyList_Size.
        let item = pyffi::PyList_GetItem(ptr, i as pyffi::Py_ssize_t);
        if item.is_null() {
            // Defensive: index out of range (should not happen given the
            // bounds check above).
            return Ok(None);
        }
        // Clear any stale error before probing (defensive).
        if !pyffi::PyErr_Occurred().is_null() {
            return Ok(None);
        }
        // PyLong_AsLongLong: returns -1 on error (overflow or non-int).
        // Must check PyErr_Occurred to distinguish -1-as-value from error.
        let val = pyffi::PyLong_AsLongLong(item);
        if val == -1 && !pyffi::PyErr_Occurred().is_null() {
            // Overflow or non-int element — fall back to per-index.
            pyffi::PyErr_Clear();
            return Ok(None);
        }
        out.push(val);
    }
    Ok(Some(out))
}

// ===========================================================================
// Helper: short-circuit scalar extraction
// ===========================================================================

/// Extracts a scalar `Value` from a Python object using short-circuit type
/// probing.
///
/// Unlike the old `py_to_value` (7+ `is_instance_of` checks with error
/// handling), this probes in priority order and returns as soon as a match
/// is found. Returns `None` if the object is not a scalar (e.g. it is a
/// `dict` / `list`), signalling the caller to report a type mismatch.
///
/// The probe order matters: `bool` must precede `int` (in Python `bool`
/// subclasses `int`), and `is_none` must come first.
fn extract_scalar_short_circuit(obj: &Bound<'_, PyAny>) -> Option<Value> {
    if obj.is_none() {
        return Some(Value::None);
    }
    // bool before int — bool is a subclass of int in Python.
    if let Ok(b) = obj.extract::<bool>() {
        // pyo3 extract::<bool> only succeeds for actual bool objects, so
        // there is no risk of int values being captured here.
        if obj.is_instance_of::<PyBool>() {
            return Some(Value::Bool(b));
        }
    }
    if let Ok(i) = obj.extract::<i64>() {
        return Some(Value::Int(i));
    }
    if let Ok(u) = obj.extract::<u64>() {
        return Some(Value::UInt(u));
    }
    if let Ok(f) = obj.extract::<f64>() {
        return Some(Value::Float(f));
    }
    if let Ok(b) = obj.extract::<&[u8]>() {
        return Some(Value::Bytes(b.to_vec()));
    }
    if let Ok(s) = obj.extract::<String>() {
        return Some(Value::String(s));
    }
    None
}

// ===========================================================================
// Helper: shallow dict/list → Value for context insertion
// ===========================================================================

/// Shallow extraction of a Python dict / list into a [`Value`] for context.
///
/// Top-level scalar fields are extracted via [`extract_scalar_short_circuit`];
/// nested composite values (sub-dicts / sub-lists) become [`Value::None`]
/// placeholders. This avoids the cost of a full recursive conversion while
/// supporting the majority of context expressions (`this.field`,
/// `this.field.subfield` at one level deep).
///
/// If the object is already a scalar, it is returned directly.
///
/// # Errors
///
/// Returns [`ConstructError::TypeMismatch`] if `obj` is not a scalar, dict,
/// or list.
fn shallow_py_to_value_for_ctx(obj: &Bound<'_, PyAny>) -> Result<Value> {
    use pyo3::types::{PyDict, PyList};

    // Scalar fast-path (covers leaf fields like int, str, bytes, …).
    if let Some(scalar) = extract_scalar_short_circuit(obj) {
        return Ok(scalar);
    }
    // Dict: shallow-extract top-level keys.
    if let Ok(dict) = obj.downcast::<PyDict>() {
        let mut map = indexmap::IndexMap::new();
        for (key, value) in dict {
            let key_str: String = key.extract().unwrap_or_default();
            if let Some(scalar) = extract_scalar_short_circuit(&value) {
                map.insert(key_str, scalar);
            } else {
                // Nested composite: placeholder (avoids deep conversion).
                map.insert(key_str, Value::None);
            }
        }
        return Ok(Value::Container(map));
    }
    // List: shallow-extract elements.
    if let Ok(list) = obj.downcast::<PyList>() {
        let mut items = Vec::with_capacity(list.len());
        for item in list {
            if let Some(scalar) = extract_scalar_short_circuit(&item) {
                items.push(scalar);
            } else {
                items.push(Value::None);
            }
        }
        return Ok(Value::List(items));
    }
    // Fallback: type mismatch.
    let type_name = obj
        .get_type()
        .name()
        .map(|n| n.to_string())
        .unwrap_or_else(|_| "<unknown>".to_string());
    Err(ConstructError::TypeMismatch {
        path: String::new(),
        expected: "scalar, dict, or list".to_string(),
        actual: type_name,
    })
}

// ===========================================================================
// PyDictInput — generic PyObject wrapper
// ===========================================================================

/// [`PyInput`] implementation wrapping an arbitrary `PyObject` (dict,
/// dataclass, list, or scalar).
///
/// This is the standard build-direction input. Each `sub_field_py` /
/// `sub_index_py` call crosses the FFI boundary exactly once — only the
/// requested field/index is fetched, not the entire tree.
///
/// # Dict vs attribute access (C-IMP-1)
///
/// `sub_field_py` tries dict-style `__getitem__` first, then falls back to
/// `getattr` (for dataclasses, named tuples, namespaces, etc.). This
/// fallback chain mirrors the old construct-py `PyInput` and is required
/// for dataclass build inputs.
pub struct PyDictInput {
    /// Owned Python object reference (GIL-independent).
    obj: Py<PyAny>,
}

impl PyDictInput {
    /// Creates a new `PyDictInput` wrapping an owned [`Py<PyAny>`].
    #[must_use]
    pub fn new(obj: Py<PyAny>) -> Self {
        Self { obj }
    }

    /// Creates a new `PyDictInput` from a bound reference.
    ///
    /// Clones the Python reference (increments refcount) and detaches it
    /// from the borrow, making the input `'static` and boxable into
    /// `Box<dyn PyInput>`.
    #[must_use]
    pub fn from_bound(obj: &Bound<'_, PyAny>) -> Self {
        Self {
            obj: obj.clone().unbind(),
        }
    }

    /// Internal helper: retrieves a named field as a bound Python object.
    ///
    /// Tries `__getitem__` first (dict), then `getattr` (dataclass/attr).
    /// Returns [`ConstructError::FieldMissing`] if neither succeeds.
    fn get_field_bound<'py>(
        &self,
        py: Python<'py>,
        name: &str,
    ) -> std::result::Result<Bound<'py, PyAny>, ConstructError> {
        let bound = self.obj.bind(py);
        bound
            .get_item(name)
            .or_else(|_| bound.getattr(name))
            .map_err(|_| ConstructError::FieldMissing {
                path: String::new(),
                field: name.to_string(),
            })
    }
}

impl std::fmt::Debug for PyDictInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyDictInput").finish_non_exhaustive()
    }
}

impl PyInput for PyDictInput {
    fn extract_scalar_py(&self, py: Python<'_>) -> Result<Value> {
        let bound = self.obj.bind(py);
        extract_scalar_short_circuit(bound).ok_or_else(|| {
            let type_name = bound
                .get_type()
                .name()
                .map(|n| n.to_string())
                .unwrap_or_else(|_| "<unknown>".to_string());
            ConstructError::TypeMismatch {
                path: String::new(),
                expected: "scalar (None/bool/int/float/bytes/str)".to_string(),
                actual: type_name,
            }
        })
    }

    fn has_field_py(&self, py: Python<'_>, name: &str) -> bool {
        // Try dict-style access first; fall back to attribute. A field is
        // "present" only if the value is non-None (matching flagbuildnone).
        match self.get_field_bound(py, name) {
            Ok(item) => !item.is_none(),
            Err(_) => false,
        }
    }

    fn sub_field_py(&self, py: Python<'_>, name: &str) -> Result<Box<dyn PyInput>> {
        let item = self.get_field_bound(py, name)?;
        Ok(Box::new(PyDictInput::new(item.unbind())))
    }

    fn sub_index_py(&self, py: Python<'_>, index: usize) -> Result<Box<dyn PyInput>> {
        let bound = self.obj.bind(py);
        let length = bound.len().unwrap_or(0);
        let item = bound.get_item(index).map_err(|_| ConstructError::Index {
            path: String::new(),
            index,
            length,
        })?;
        Ok(Box::new(PyDictInput::new(item.unbind())))
    }

    fn len_py(&self, py: Python<'_>) -> usize {
        self.obj.bind(py).len().unwrap_or(1)
    }

    fn extract_batch_int_py(&self, py: Python<'_>) -> Result<Option<Vec<i64>>> {
        let bound = self.obj.bind(py);
        // SAFETY: `py` (GIL token) is held, guaranteeing `bound` is not
        // garbage-collected during iteration. The build input is not
        // concurrently mutated. See `extract_batch_int_raw` safety docs.
        unsafe { extract_batch_int_raw(py, bound) }
    }

    fn extract_ctx_value_py(&self, py: Python<'_>) -> Result<Value> {
        let bound = self.obj.bind(py);
        shallow_py_to_value_for_ctx(bound)
    }
}

// ===========================================================================
// PyScalarInput — known Value wrapper (post batch-extract)
// ===========================================================================

/// [`PyInput`] implementation wrapping a known [`Value`].
///
/// Used after [`extract_batch_int_raw`] extracts a `Vec<i64>`: each element
/// is wrapped in a `PyScalarInput(Value::Int(i))` and fed back into the
/// per-element build loop, **without** re-crossing the FFI boundary. This
/// eliminates per-element `PyInput` allocation + FFI extraction for
/// homogeneous integer arrays.
pub struct PyScalarInput {
    /// The owned `Value` being read.
    value: Value,
}

impl PyScalarInput {
    /// Creates a new `PyScalarInput` from an owned [`Value`].
    #[must_use]
    pub fn new(value: Value) -> Self {
        Self { value }
    }
}

impl std::fmt::Debug for PyScalarInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyScalarInput")
            .field("value", &self.value)
            .finish()
    }
}

impl PyInput for PyScalarInput {
    fn extract_scalar_py(&self, _py: Python<'_>) -> Result<Value> {
        Ok(self.value.clone())
    }

    fn has_field_py(&self, _py: Python<'_>, name: &str) -> bool {
        match &self.value {
            Value::Container(map) => map.get(name).is_some_and(|v| !v.is_none()),
            _ => false,
        }
    }

    fn sub_field_py(&self, _py: Python<'_>, name: &str) -> Result<Box<dyn PyInput>> {
        let val = match &self.value {
            Value::Container(map) => {
                map.get(name)
                    .cloned()
                    .ok_or_else(|| ConstructError::FieldMissing {
                        path: String::new(),
                        field: name.to_string(),
                    })?
            }
            other => {
                return Err(ConstructError::TypeMismatch {
                    path: String::new(),
                    expected: "Container".to_string(),
                    actual: other.type_name().to_string(),
                });
            }
        };
        Ok(Box::new(PyScalarInput::new(val)))
    }

    fn sub_index_py(&self, _py: Python<'_>, index: usize) -> Result<Box<dyn PyInput>> {
        let val = match &self.value {
            Value::List(list) => list
                .get(index)
                .cloned()
                .ok_or_else(|| ConstructError::Index {
                    path: String::new(),
                    index,
                    length: list.len(),
                })?,
            other => {
                return Err(ConstructError::TypeMismatch {
                    path: String::new(),
                    expected: "List".to_string(),
                    actual: other.type_name().to_string(),
                });
            }
        };
        Ok(Box::new(PyScalarInput::new(val)))
    }

    fn len_py(&self, _py: Python<'_>) -> usize {
        match &self.value {
            Value::Container(c) => c.len(),
            Value::List(l) => l.len(),
            _ => 1,
        }
    }

    fn extract_batch_int_py(&self, _py: Python<'_>) -> Result<Option<Vec<i64>>> {
        // PyScalarInput wraps a single Value, so batch extraction of a list
        // is possible only when the inner Value is a List of all-ints.
        if let Value::List(items) = &self.value {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item.to_i64() {
                    Ok(i) => out.push(i),
                    Err(_) => return Ok(None),
                }
            }
            Ok(Some(out))
        } else {
            Ok(None)
        }
    }

    fn extract_ctx_value_py(&self, _py: Python<'_>) -> Result<Value> {
        Ok(self.value.clone())
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiled::py_sink::ensure_test_python;
    use indexmap::IndexMap;
    use pyo3::types::{PyDict, PyList};

    // -- PyDictInput: extract_scalar_py -------------------------------------

    #[test]
    fn pydictinput_extract_scalar_int() {
        ensure_test_python();
        Python::with_gil(|py| {
            let obj = 42i64.to_object(py);
            let input = PyDictInput::new(obj);
            assert_eq!(input.extract_scalar_py(py).unwrap(), Value::Int(42));
        });
    }

    #[test]
    fn pydictinput_extract_scalar_string() {
        ensure_test_python();
        Python::with_gil(|py| {
            let obj = "hello".to_object(py);
            let input = PyDictInput::new(obj);
            assert_eq!(
                input.extract_scalar_py(py).unwrap(),
                Value::String("hello".to_string())
            );
        });
    }

    #[test]
    fn pydictinput_extract_scalar_none() {
        ensure_test_python();
        Python::with_gil(|py| {
            let input = PyDictInput::new(py.None());
            assert_eq!(input.extract_scalar_py(py).unwrap(), Value::None);
        });
    }

    #[test]
    fn pydictinput_extract_scalar_bool() {
        ensure_test_python();
        Python::with_gil(|py| {
            let obj = true.to_object(py);
            let input = PyDictInput::new(obj);
            assert_eq!(input.extract_scalar_py(py).unwrap(), Value::Bool(true));
        });
    }

    #[test]
    fn pydictinput_extract_scalar_dict_returns_type_mismatch() {
        ensure_test_python();
        Python::with_gil(|py| {
            let dict = PyDict::new_bound(py);
            let input = PyDictInput::from_bound(dict.as_any());
            let err = input.extract_scalar_py(py).unwrap_err();
            assert!(matches!(err, ConstructError::TypeMismatch { .. }));
        });
    }

    // -- PyDictInput: has_field_py ------------------------------------------

    #[test]
    fn pydictinput_has_field_existing() {
        ensure_test_python();
        Python::with_gil(|py| {
            let dict = PyDict::new_bound(py);
            dict.set_item("x", 42i64).unwrap();
            let input = PyDictInput::from_bound(dict.as_any());
            assert!(input.has_field_py(py, "x"));
        });
    }

    #[test]
    fn pydictinput_has_field_missing() {
        ensure_test_python();
        Python::with_gil(|py| {
            let dict = PyDict::new_bound(py);
            let input = PyDictInput::from_bound(dict.as_any());
            assert!(!input.has_field_py(py, "missing"));
        });
    }

    #[test]
    fn pydictinput_has_field_none_is_false() {
        ensure_test_python();
        Python::with_gil(|py| {
            let dict = PyDict::new_bound(py);
            dict.set_item("x", py.None()).unwrap();
            let input = PyDictInput::from_bound(dict.as_any());
            // None values are treated as "not present" (flagbuildnone).
            assert!(!input.has_field_py(py, "x"));
        });
    }

    // -- PyDictInput: sub_field_py (dict + getattr fallback) ----------------

    #[test]
    fn pydictinput_sub_field_via_dict() {
        ensure_test_python();
        Python::with_gil(|py| {
            let dict = PyDict::new_bound(py);
            dict.set_item("a", 1i64).unwrap();
            let input = PyDictInput::from_bound(dict.as_any());
            let sub = input.sub_field_py(py, "a").unwrap();
            assert_eq!(sub.extract_scalar_py(py).unwrap(), Value::Int(1));
        });
    }

    #[test]
    fn pydictinput_sub_field_via_getattr_fallback() {
        // C-IMP-1: dataclass / attribute access must fall back to getattr.
        ensure_test_python();
        Python::with_gil(|py| {
            // Create a simple namespace-like object via type().
            let code = "type('O', (), {'x': 42})()";
            let obj = py.eval_bound(code, None, None).unwrap();
            let input = PyDictInput::from_bound(&obj);
            let sub = input.sub_field_py(py, "x").unwrap();
            assert_eq!(sub.extract_scalar_py(py).unwrap(), Value::Int(42));
        });
    }

    #[test]
    fn pydictinput_sub_field_missing_returns_error() {
        ensure_test_python();
        Python::with_gil(|py| {
            let dict = PyDict::new_bound(py);
            let input = PyDictInput::from_bound(dict.as_any());
            let err = input.sub_field_py(py, "missing").unwrap_err();
            assert!(matches!(err, ConstructError::FieldMissing { .. }));
        });
    }

    // -- PyDictInput: sub_index_py + len_py ---------------------------------

    #[test]
    fn pydictinput_sub_index_and_len() {
        ensure_test_python();
        Python::with_gil(|py| {
            let list = PyList::new_bound(py, [10i64, 20, 30]);
            let input = PyDictInput::from_bound(list.as_any());
            assert_eq!(input.len_py(py), 3);
            let sub = input.sub_index_py(py, 1).unwrap();
            assert_eq!(sub.extract_scalar_py(py).unwrap(), Value::Int(20));
        });
    }

    #[test]
    fn pydictinput_sub_index_out_of_bounds() {
        ensure_test_python();
        Python::with_gil(|py| {
            let list = PyList::new_bound(py, [1i64]);
            let input = PyDictInput::from_bound(list.as_any());
            let err = input.sub_index_py(py, 5).unwrap_err();
            assert!(matches!(err, ConstructError::Index { .. }));
        });
    }

    // -- PyDictInput: extract_batch_int_py (raw CPython API) ----------------

    #[test]
    fn pydictinput_extract_batch_int_py_homogeneous() {
        ensure_test_python();
        Python::with_gil(|py| {
            let list = PyList::new_bound(py, [1i64, 2, 3, 4, 5]);
            let input = PyDictInput::from_bound(list.as_any());
            let result = input.extract_batch_int_py(py).unwrap();
            assert_eq!(result, Some(vec![1, 2, 3, 4, 5]));
        });
    }

    #[test]
    fn pydictinput_extract_batch_int_py_non_list_returns_none() {
        ensure_test_python();
        Python::with_gil(|py| {
            let obj = 42i64.to_object(py);
            let input = PyDictInput::new(obj);
            let result = input.extract_batch_int_py(py).unwrap();
            assert_eq!(result, None);
        });
    }

    #[test]
    fn pydictinput_extract_batch_int_py_non_int_elements_returns_none() {
        ensure_test_python();
        Python::with_gil(|py| {
            // Mixed list (int + string) — should return None for fallback.
            let list = PyList::new_bound(py, [1i64, 2]);
            list.append("three").unwrap();
            let input = PyDictInput::from_bound(list.as_any());
            let result = input.extract_batch_int_py(py).unwrap();
            assert_eq!(result, None);
        });
    }

    // -- PyScalarInput ------------------------------------------------------

    #[test]
    fn pyscalarinput_extract_scalar() {
        let input = PyScalarInput::new(Value::Int(99));
        ensure_test_python();
        Python::with_gil(|py| {
            assert_eq!(input.extract_scalar_py(py).unwrap(), Value::Int(99));
        });
    }

    #[test]
    fn pyscalarinput_sub_field_from_container() {
        let mut map = IndexMap::new();
        map.insert("x".to_string(), Value::Int(1));
        let input = PyScalarInput::new(Value::Container(map));
        ensure_test_python();
        Python::with_gil(|py| {
            let sub = input.sub_field_py(py, "x").unwrap();
            assert_eq!(sub.extract_scalar_py(py).unwrap(), Value::Int(1));
        });
    }

    #[test]
    fn pyscalarinput_len_list() {
        let input = PyScalarInput::new(Value::List(vec![Value::Int(1), Value::Int(2)]));
        ensure_test_python();
        Python::with_gil(|py| {
            assert_eq!(input.len_py(py), 2);
        });
    }

    #[test]
    fn pyscalarinput_extract_batch_int_py() {
        let input = PyScalarInput::new(Value::List(vec![
            Value::Int(10),
            Value::Int(20),
            Value::Int(30),
        ]));
        ensure_test_python();
        Python::with_gil(|py| {
            let result = input.extract_batch_int_py(py).unwrap();
            assert_eq!(result, Some(vec![10, 20, 30]));
        });
    }
}
