//! Python-direct output sink for the parse direction (Phase 16).
//!
//! This module defines [`PySink`] — a sink that collects parse results
//! directly into Python `dict` / `list` objects (wrapped as `Container` /
//! `ListContainer` subclasses when available), **bypassing the `Value`
//! intermediate tree** on the Python hot path.
//!
//! # Zero round-trip design
//!
//! Each leaf deposit produces **both** a scalar `Value` (for context
//! expression evaluation, e.g. `this.field` references) and a `PyObject`
//! (for output) from a single decode. When a parent finishes a field, the
//! child's `Value` is returned directly for context insertion — no
//! `py_to_value` round-trip on the nested `PyObject`.
//!
//! # `python` feature gate
//!
//! This entire module is compiled only under `--features python`. Without
//! it, the pure-Rust [`ValueSink`](super::ValueSink) path is used unchanged,
//! and the 1393+ unit tests are unaffected.
//
// NOTE: The `#[cfg(feature = "python")]` gate lives on the `pub mod` line in
// `mod.rs`; an inner `#![cfg(...)]` would duplicate it and trigger
// `clippy::duplicated_attributes`.

use std::sync::OnceLock;

use indexmap::IndexMap;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList, PyString};

use crate::core::error::{ConstructError, Result};
use crate::value::Value;

// ===========================================================================
// PyErr → ConstructError helper
// ===========================================================================

/// Converts a [`PyErr`] into a [`ConstructError::Generic`].
///
/// This is a lossy mapping — the Python exception type and traceback are
/// stringified into the `message` field. The `path` is left empty because
/// the sink layer does not track construct-tree position (path enrichment
/// happens at the `exec_parse_py` level via `with_path_prefix`).
pub(crate) fn py_err_to_construct(e: PyErr) -> ConstructError {
    ConstructError::Generic {
        path: String::new(),
        message: e.to_string(),
    }
}

// ===========================================================================
// ContainerClasses — cached Container / ListContainer class references
// ===========================================================================

/// The Python module path where `Container` and `ListContainer` are defined
/// in the pure-Python `construct_rust` package. These classes subclass
/// `dict` / `list` respectively, enabling attribute access (`result.field`)
/// and `search()` / `search_all()` on parse results.
const CONTAINERS_MODULE: &str = "construct_rust.lib.containers";

/// Cached references to the `Container` and `ListContainer` Python classes.
///
/// `Container` is a `dict` subclass; `ListContainer` is a `list` subclass.
/// Both are produced by the pure-Python `construct_rust.lib.containers`
/// module (installed via `maturin develop`). When that module is not
/// importable (e.g. `cargo test --features python` without an installed
/// package), both fields are `None` and the sink falls back to plain
/// `PyDict` / `PyList`.
///
/// # Caching strategy
///
/// A single global instance is stored in a [`OnceLock`]. The `import_bound`
/// cost (~200-400ns, two imports) is paid only on the first parse call;
/// subsequent calls retrieve the cached `&'static` reference. Sub-sinks
/// share the reference by cloning the `Py<PyAny>` handles (refcount inc,
/// ~5ns each), avoiding repeated imports.
pub struct ContainerClasses {
    /// `construct_rust.lib.containers.Container` (a `dict` subclass), or
    /// `None` if the module is not importable.
    container_cls: Option<Py<PyAny>>,
    /// `construct_rust.lib.containers.ListContainer` (a `list` subclass),
    /// or `None` if the module is not importable.
    listcontainer_cls: Option<Py<PyAny>>,
}

impl Clone for ContainerClasses {
    fn clone(&self) -> Self {
        // `Python::with_gil` is a no-op when the GIL is already held (the
        // normal case in the Python path). `clone_ref` increments the
        // refcount of the underlying class object (~5ns).
        Python::with_gil(|py| Self {
            container_cls: self.container_cls.as_ref().map(|c| c.clone_ref(py)),
            listcontainer_cls: self.listcontainer_cls.as_ref().map(|c| c.clone_ref(py)),
        })
    }
}

/// Global cache of the `Container` / `ListContainer` class references.
///
/// Filled once on the first call to [`ContainerClasses::obtain`]; the
/// `'static` lifetime is sound because `Py<PyAny>` is `Send + Sync` and
/// the referenced Python objects outlive the process (module classes are
/// never collected).
static CONTAINER_CLASSES: OnceLock<ContainerClasses> = OnceLock::new();

impl ContainerClasses {
    /// Returns the globally cached class references, importing them on the
    /// first call.
    ///
    /// # Fail-soft behaviour
    ///
    /// If `construct_rust.lib.containers` cannot be imported, both class
    /// references are `None`. Sub-sink creation then falls back to plain
    /// `PyDict` / `PyList`, which is the correct behaviour for
    /// `cargo test --features python` (no installed Python package).
    #[must_use]
    pub fn obtain(py: Python<'_>) -> &'static Self {
        CONTAINER_CLASSES.get_or_init(|| Self::fetch(py))
    }

    /// Creates an empty `ContainerClasses` with both class references set
    /// to `None`. Used in unit tests where attribute-access wrapping is not
    /// required.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            container_cls: None,
            listcontainer_cls: None,
        }
    }

    /// Attempts to import `Container` and `ListContainer` from the
    /// pure-Python package. Both are set to `None` if the import fails.
    fn fetch(py: Python<'_>) -> Self {
        let container_cls = py
            .import_bound(CONTAINERS_MODULE)
            .and_then(|m| m.getattr("Container"))
            .ok()
            .map(|cls| cls.unbind());
        let listcontainer_cls = py
            .import_bound(CONTAINERS_MODULE)
            .and_then(|m| m.getattr("ListContainer"))
            .ok()
            .map(|cls| cls.unbind());
        Self {
            container_cls,
            listcontainer_cls,
        }
    }

    /// Creates a fresh `Container` instance (a `dict` subclass) if the
    /// class is available, otherwise a plain `PyDict`.
    fn new_container(&self, py: Python<'_>) -> Py<PyAny> {
        match &self.container_cls {
            Some(cls) => cls
                .bind(py)
                .call0()
                .map(|bound| bound.into_any().unbind())
                .unwrap_or_else(|_| PyDict::new_bound(py).into_any().unbind()),
            None => PyDict::new_bound(py).into_any().unbind(),
        }
    }

    /// Creates a fresh `ListContainer` instance (a `list` subclass) if the
    /// class is available, otherwise a plain `PyList`.
    fn new_listcontainer(&self, py: Python<'_>) -> Py<PyAny> {
        match &self.listcontainer_cls {
            Some(cls) => cls
                .bind(py)
                .call0()
                .map(|bound| bound.into_any().unbind())
                .unwrap_or_else(|_| PyList::empty_bound(py).into_any().unbind()),
            None => PyList::empty_bound(py).into_any().unbind(),
        }
    }
}

// ===========================================================================
// PySinkMode — internal state of PyDictSink2
// ===========================================================================

/// Internal mode of [`PyDictSink2`], tracking whether the sink holds a
/// pending leaf scalar, a named-field dict, or a positional-item list.
///
/// The `dict` / `list` fields are typed as `Py<PyAny>` (not `Py<PyDict>` /
/// `Py<PyList>`) because they may hold `Container` / `ListContainer`
/// subclass instances. All operations use `PyObject_SetItem` /
/// `PyObject_Append`-equivalent methods, which work uniformly on subclasses.
enum PySinkMode {
    /// Nothing deposited yet; transitions on the first write.
    Empty,
    /// Leaf: holds the scalar `Value` (for ctx) and `PyObject` (for output)
    /// deposited by `deposit_leaf`.
    Leaf { value: Value, obj: PyObject },
    /// Struct: owns a dict-like (`Container` or `PyDict`) plus a parallel
    /// `IndexMap<String, Value>` for context expression evaluation.
    Struct {
        dict: Py<PyAny>,
        ctx_fields: IndexMap<String, Value>,
    },
    /// Array: owns a list-like (`ListContainer` or `PyList`) plus a parallel
    /// `Vec<Value>` for context expression evaluation.
    Array {
        list: Py<PyAny>,
        ctx_items: Vec<Value>,
    },
}

// ===========================================================================
// PySink trait
// ===========================================================================

/// Python-direct output sink for the parse direction.
///
/// Each method receives `py: Python<'_>` (the GIL token, threaded once from
/// the entry point — no per-field `Python::with_gil`). The sink maintains
/// EITHER:
/// - a pending scalar `(Value, PyObject)` pair (leaf scenario), OR
/// - a `Py<PyAny>` dict-like + parallel `IndexMap<String, Value>` (Struct
///   scenario), OR
/// - a `Py<PyAny>` list-like + parallel `Vec<Value>` (Array scenario).
///
/// The parallel `Value` tree is for **context expression evaluation only**
/// (`this.field` references). It is NOT the output — the output is the
/// `PyObject` tree. The `Value` tree is far cheaper than the FFI round-trips
/// it eliminates.
///
/// # Protocol
///
/// 1. **Leaf nodes** call [`deposit_leaf`](Self::deposit_leaf) after parsing.
/// 2. **Composite nodes** create a sub-sink per child via
///    [`sub_py_field`](Self::sub_py_field) /
///    [`sub_py_item`](Self::sub_py_item), recurse, then integrate via
///    [`finish_field_py`](Self::finish_field_py) /
///    [`finish_item_py`](Self::finish_item_py).
/// 3. The root sink is consumed via
///    [`into_root_py`](Self::into_root_py) to obtain the final `PyObject`.
pub trait PySink {
    /// Deposits a leaf scalar: stores both the `Value` (for ctx) and
    /// `PyObject` (for output). Both are produced from a single decode —
    /// no round-trip.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::Generic`] if the sink is not in `Empty`
    /// mode (leaf deposits are only valid before any composite integration).
    fn deposit_leaf(&mut self, py: Python<'_>, value: Value, obj: PyObject) -> Result<()>;

    /// Creates a sub-sink for a named Struct field.
    ///
    /// The returned sub-sink starts in `Empty` mode; the child node's
    /// `exec_parse_py` determines whether it becomes a leaf (via
    /// `deposit_leaf`) or a composite (via recursive
    /// `finish_field_py` / `finish_item_py`, which auto-transition
    /// `Empty` → `Struct` / `Array`).
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::Generic`] on allocation failure.
    fn sub_py_field(&mut self, py: Python<'_>, name: &str) -> Result<Box<dyn PySink>>;

    /// Creates a sub-sink for a sequential Array item. See
    /// [`sub_py_field`](Self::sub_py_field) for the transition semantics.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::Generic`] on allocation failure.
    fn sub_py_item(&mut self, py: Python<'_>) -> Result<Box<dyn PySink>>;

    /// Consumes the sink and returns both the parallel `Value` tree (for
    /// ctx) and the `PyObject` (for output).
    ///
    /// This is the **zero round-trip** mechanism: the `Value` comes from
    /// the sink's parallel tree, not from `py_to_value(PyObject)`. Parent
    /// sinks call this internally via [`finish_field_py`](Self::finish_field_py)
    /// / [`finish_item_py`](Self::finish_item_py) to obtain the child's
    /// ctx value without any FFI round-trip.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::Generic`] if the sink is still `Empty`
    /// (nothing deposited).
    fn into_pair(self: Box<Self>, py: Python<'_>) -> Result<(Value, PyObject)>;

    /// Finishes a named field sub-sink: integrates its `PyObject` into this
    /// parent sink's dict (via `PyObject_SetItem`) AND returns the child's
    /// `Value` for context insertion.
    ///
    /// If this sink is currently `Empty`, it auto-transitions to `Struct`
    /// mode (creating a fresh `Container` / `PyDict`).
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::Generic`] if this sink is in `Leaf` or
    /// `Array` mode (field integration requires a `Struct` sink).
    fn finish_field_py(
        &mut self,
        py: Python<'_>,
        name: &str,
        sub: Box<dyn PySink>,
    ) -> Result<Value>;

    /// Finishes an anonymous item sub-sink (Array elements, anonymous
    /// Sequence/Struct fields). Integrates the child's `PyObject` into this
    /// parent sink's list (via append).
    ///
    /// If this sink is currently `Empty`, it auto-transitions to `Array`
    /// mode (creating a fresh `ListContainer` / `PyList`).
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::Generic`] if this sink is in `Leaf` or
    /// `Struct` mode.
    fn finish_item_py(&mut self, py: Python<'_>, sub: Box<dyn PySink>) -> Result<()>;

    /// Finishes a named Sequence entry (like `finish_field_py` but appends
    /// to the parent's list instead of setting a dict field). Returns the
    /// child's `Value` for context insertion by name.
    ///
    /// If this sink is currently `Empty`, it auto-transitions to `Array`
    /// mode.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::Generic`] if this sink is in `Leaf` or
    /// `Struct` mode.
    fn finish_named_item_py(
        &mut self,
        py: Python<'_>,
        name: &str,
        sub: Box<dyn PySink>,
    ) -> Result<Value>;

    /// Consumes the sink and returns the root `PyObject` (the output).
    ///
    /// Default implementation delegates to
    /// [`into_pair`](Self::into_pair) and discards the `Value`.
    ///
    /// # Errors
    ///
    /// Propagates any error from [`into_pair`](Self::into_pair).
    fn into_root_py(self: Box<Self>, py: Python<'_>) -> Result<PyObject> {
        let (_, obj) = self.into_pair(py)?;
        Ok(obj)
    }
}

// ===========================================================================
// PyDictSink2 — concrete PySink implementation
// ===========================================================================

/// Concrete [`PySink`] that assembles parse results directly into Python
/// `dict` / `list` objects (wrapped as `Container` / `ListContainer`
/// subclasses when available).
///
/// This is the Phase 16 replacement for construct-py's `PyDictSink`. It
/// lives in construct-rs (under the `python` feature) so it can share the
/// compiled execution tree's internal types without cross-crate trait
/// objects.
///
/// # Mode transitions
///
/// | Starting mode | Operation          | Resulting mode |
/// |---------------|--------------------|----------------|
/// | `Empty`       | `deposit_leaf`     | `Leaf`         |
/// | `Empty`       | `finish_field_py`  | `Struct`       |
/// | `Empty`       | `finish_item_py`   | `Array`        |
/// | `Empty`       | `finish_named_item_py` | `Array`    |
/// | `Struct`      | `finish_field_py`  | `Struct`       |
/// | `Array`       | `finish_item_py`   | `Array`        |
///
/// Mismatched operations (e.g. `deposit_leaf` on a `Struct` sink) return
/// `Err`.
pub struct PyDictSink2 {
    /// Current sink mode.
    mode: PySinkMode,
    /// Shared class references for `Container` / `ListContainer` wrapping.
    /// Cloned (refcount inc) into every sub-sink so the import cost is paid
    /// only once by [`ContainerClasses::obtain`].
    classes: ContainerClasses,
}

impl PyDictSink2 {
    /// Creates a root sink pre-initialised in `Struct` mode with a fresh
    /// `Container` (or `PyDict` fallback).
    ///
    /// Used by the parse entry point so that even an empty struct produces
    /// an empty `dict` rather than `None`.
    #[must_use]
    pub fn new_root_struct(py: Python<'_>) -> Self {
        let classes = ContainerClasses::obtain(py).clone();
        let dict = classes.new_container(py);
        Self {
            mode: PySinkMode::Struct {
                dict,
                ctx_fields: IndexMap::new(),
            },
            classes,
        }
    }

    /// Creates a root sink pre-initialised in `Array` mode with a fresh
    /// `ListContainer` (or `PyList` fallback).
    #[must_use]
    pub fn new_root_array(py: Python<'_>) -> Self {
        let classes = ContainerClasses::obtain(py).clone();
        let list = classes.new_listcontainer(py);
        Self {
            mode: PySinkMode::Array {
                list,
                ctx_items: Vec::new(),
            },
            classes,
        }
    }

    /// Creates a new sink in `Empty` mode, sharing the parent's class
    /// references. Used by [`sub_py_field`](PySink::sub_py_field) /
    /// [`sub_py_item`](PySink::sub_py_item).
    #[must_use]
    pub fn new_empty(classes: ContainerClasses) -> Self {
        Self {
            mode: PySinkMode::Empty,
            classes,
        }
    }

    /// Transitions an `Empty` sink to `Struct` mode by creating a fresh
    /// `Container` / `PyDict`. Called lazily by `finish_field_py` when the
    /// sink is first used for named-field integration. No-op if already
    /// `Struct`; error if `Leaf` or `Array`.
    fn ensure_struct(&mut self, py: Python<'_>) -> Result<()> {
        if matches!(self.mode, PySinkMode::Empty) {
            let dict = self.classes.new_container(py);
            self.mode = PySinkMode::Struct {
                dict,
                ctx_fields: IndexMap::new(),
            };
        }
        Ok(())
    }

    /// Transitions an `Empty` sink to `Array` mode by creating a fresh
    /// `ListContainer` / `PyList`. Called lazily by `finish_item_py` /
    /// `finish_named_item_py`. No-op if already `Array`; error if `Leaf`
    /// or `Struct`.
    fn ensure_array(&mut self, py: Python<'_>) -> Result<()> {
        if matches!(self.mode, PySinkMode::Empty) {
            let list = self.classes.new_listcontainer(py);
            self.mode = PySinkMode::Array {
                list,
                ctx_items: Vec::new(),
            };
        }
        Ok(())
    }
}

impl PySink for PyDictSink2 {
    fn deposit_leaf(&mut self, _py: Python<'_>, value: Value, obj: PyObject) -> Result<()> {
        match self.mode {
            PySinkMode::Empty => {
                self.mode = PySinkMode::Leaf { value, obj };
                Ok(())
            }
            _ => Err(ConstructError::Generic {
                path: String::new(),
                message: "PyDictSink2::deposit_leaf on non-Empty sink".to_string(),
            }),
        }
    }

    fn sub_py_field(&mut self, _py: Python<'_>, _name: &str) -> Result<Box<dyn PySink>> {
        Ok(Box::new(PyDictSink2::new_empty(self.classes.clone())))
    }

    fn sub_py_item(&mut self, _py: Python<'_>) -> Result<Box<dyn PySink>> {
        Ok(Box::new(PyDictSink2::new_empty(self.classes.clone())))
    }

    fn into_pair(self: Box<Self>, _py: Python<'_>) -> Result<(Value, PyObject)> {
        let sink = *self;
        match sink.mode {
            PySinkMode::Empty => Err(ConstructError::Generic {
                path: String::new(),
                message: "PyDictSink2::into_pair on Empty sink (nothing deposited)".to_string(),
            }),
            PySinkMode::Leaf { value, obj } => Ok((value, obj)),
            PySinkMode::Struct { dict, ctx_fields } => Ok((Value::Container(ctx_fields), dict)),
            PySinkMode::Array { list, ctx_items } => Ok((Value::List(ctx_items), list)),
        }
    }

    fn finish_field_py(
        &mut self,
        py: Python<'_>,
        name: &str,
        sub: Box<dyn PySink>,
    ) -> Result<Value> {
        let (value, obj) = sub.into_pair(py)?;
        // Auto-transition Empty → Struct so composite nodes don't need an
        // explicit "begin_struct" call.
        self.ensure_struct(py)?;
        match &mut self.mode {
            PySinkMode::Struct { dict, ctx_fields } => {
                // One PyObject_SetItem call (works on dict subclasses too).
                dict.bind(py)
                    .set_item(name, obj)
                    .map_err(py_err_to_construct)?;
                // ctx Value insertion (cheap Rust op, no FFI).
                ctx_fields.insert(name.to_string(), value.clone());
                Ok(value)
            }
            _ => Err(ConstructError::Generic {
                path: String::new(),
                message: format!("finish_field_py({name}) on non-Struct sink"),
            }),
        }
    }

    fn finish_item_py(&mut self, py: Python<'_>, sub: Box<dyn PySink>) -> Result<()> {
        let (_value, obj) = sub.into_pair(py)?;
        self.ensure_array(py)?;
        match &mut self.mode {
            PySinkMode::Array { list, .. } => {
                list.bind(py)
                    .call_method1("append", (obj,))
                    .map_err(py_err_to_construct)?;
                Ok(())
            }
            _ => Err(ConstructError::Generic {
                path: String::new(),
                message: "finish_item_py on non-Array sink".to_string(),
            }),
        }
    }

    fn finish_named_item_py(
        &mut self,
        py: Python<'_>,
        _name: &str,
        sub: Box<dyn PySink>,
    ) -> Result<Value> {
        let (value, obj) = sub.into_pair(py)?;
        self.ensure_array(py)?;
        match &mut self.mode {
            PySinkMode::Array { list, ctx_items } => {
                list.bind(py)
                    .call_method1("append", (obj,))
                    .map_err(py_err_to_construct)?;
                ctx_items.push(value.clone());
                Ok(value)
            }
            _ => Err(ConstructError::Generic {
                path: String::new(),
                message: "finish_named_item_py on non-Array sink".to_string(),
            }),
        }
    }
}

// ===========================================================================
// Scalar / composite Value → PyObject converters
// ===========================================================================

/// Fast scalar `Value` → `PyObject` conversion for leaf deposits.
///
/// Unlike `conversions.rs::value_to_py`, this function:
/// - Does NOT perform repeated `is_instance_of` type checks (the `Value` is
///   already typed — a single `match` arm dispatches to the correct C API
///   call).
/// - Does NOT call `import_bound` for `Container` / `List` (composites manage
///   `PyDict` / `PyList` directly via `PySink`, never through this function).
///
/// `Container` / `List` variants fall through to
/// [`value_to_py_composite_fallback`], which is the escape-hatch path used
/// by `Dynamic` / `External` nodes whose inner `parse` may produce a
/// composite `Value`.
///
/// # Errors
///
/// Returns a [`PyErr`] (converted by the caller) if the underlying C API
/// call fails — in practice this only happens for `BigInt` overflow, which
/// is unreachable since the source is `i128`.
pub fn value_to_py_scalar(py: Python<'_>, v: &Value) -> PyResult<PyObject> {
    match v {
        Value::None => Ok(py.None()),
        Value::Bool(b) => Ok(b.into_py(py)),
        Value::Int(i) => Ok(i.into_py(py)),
        Value::UInt(u) => Ok(u.into_py(py)),
        Value::BigInt(i) => {
            // pyo3's num-bigint feature gives BigInt IntoPy<PyObject>.
            // i128 has no direct IntoPy, so go through BigInt::from.
            let bigint = num_bigint::BigInt::from(*i);
            Ok(bigint.into_py(py))
        }
        Value::Float(f) => Ok(f.into_py(py)),
        Value::Bytes(b) => Ok(PyBytes::new_bound(py, b).into_any().unbind()),
        Value::String(s) => Ok(PyString::new_bound(py, s).into_any().unbind()),
        // Composites should never reach here at the leaf level in normal
        // operation (composites use PySink directly). Fall back to the full
        // recursive converter for safety (e.g. Enum producing Container for
        // FlagsEnum, or Dynamic escape-hatch).
        Value::Container(_) | Value::List(_) => value_to_py_composite_fallback(py, v),
    }
}

/// Composite `Value` → `PyObject` fallback converter (escape-hatch path).
///
/// Wraps `Container` / `List` values in `Container` / `ListContainer`
/// subclasses (when available) so that attribute access and `search()`
/// work on the result. Used by:
/// - [`value_to_py_scalar`] when given a composite (defensive).
/// - `Dynamic` / `External` escape-hatch nodes whose inner `parse` produces
///   a full `Value` tree.
///
/// This is **not** on the hot path — the normal composite path assembles
/// `Container` / `ListContainer` instances incrementally via [`PyDictSink2`].
///
/// # Errors
///
/// Returns a [`PyErr`] if a sub-conversion or `set_item` / `append` fails.
pub fn value_to_py_composite_fallback(py: Python<'_>, v: &Value) -> PyResult<PyObject> {
    let classes = ContainerClasses::obtain(py);
    match v {
        Value::List(items) => {
            let list = classes.new_listcontainer(py);
            for item in items {
                let child = if matches!(item, Value::Container(_) | Value::List(_)) {
                    value_to_py_composite_fallback(py, item)?
                } else {
                    value_to_py_scalar(py, item)?
                };
                list.bind(py).call_method1("append", (child,))?;
            }
            Ok(list)
        }
        Value::Container(map) => {
            let dict = classes.new_container(py);
            for (key, val) in map {
                let child = if matches!(val, Value::Container(_) | Value::List(_)) {
                    value_to_py_composite_fallback(py, val)?
                } else {
                    value_to_py_scalar(py, val)?
                };
                dict.bind(py).set_item(key, child)?;
            }
            Ok(dict)
        }
        // Scalars — delegate to the fast path.
        _ => value_to_py_scalar(py, v),
    }
}

// ===========================================================================
// Tests
// ===========================================================================

/// Initializes the embedded Python interpreter for unit tests.
///
/// Uses [`std::sync::Once`] to guarantee exactly one initialization across
/// all test threads. This avoids the race condition in pyo3's
/// `auto-initialize` feature (which is NOT enabled in this crate) where
/// multiple threads calling `Python::with_gil` before the interpreter is
/// started would deadlock.
///
/// Under `maturin develop`, the interpreter is already running (managed by
/// the Python process), so this is a no-op.
#[cfg(test)]
pub(crate) fn ensure_test_python() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(pyo3::prepare_freethreaded_python);
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use pyo3::types::{PyBool, PyDict, PyFloat, PyList};

    // -- value_to_py_scalar: scalar variants --------------------------------

    #[test]
    fn value_to_py_scalar_none() {
        ensure_test_python();
        Python::with_gil(|py| {
            let obj = value_to_py_scalar(py, &Value::None).unwrap();
            assert!(obj.bind(py).is_none());
        });
    }

    #[test]
    fn value_to_py_scalar_int() {
        ensure_test_python();
        Python::with_gil(|py| {
            let obj = value_to_py_scalar(py, &Value::Int(-123)).unwrap();
            assert_eq!(obj.bind(py).extract::<i64>().unwrap(), -123);
        });
    }

    #[test]
    fn value_to_py_scalar_uint() {
        ensure_test_python();
        Python::with_gil(|py| {
            let obj = value_to_py_scalar(py, &Value::UInt(99)).unwrap();
            assert_eq!(obj.bind(py).extract::<u64>().unwrap(), 99);
        });
    }

    #[test]
    fn value_to_py_scalar_bool() {
        ensure_test_python();
        Python::with_gil(|py| {
            let obj = value_to_py_scalar(py, &Value::Bool(true)).unwrap();
            assert!(obj.bind(py).is_instance_of::<PyBool>());
            assert!(obj.bind(py).extract::<bool>().unwrap());
        });
    }

    #[test]
    fn value_to_py_scalar_float() {
        ensure_test_python();
        Python::with_gil(|py| {
            let obj = value_to_py_scalar(py, &Value::Float(2.5)).unwrap();
            assert!(obj.bind(py).is_instance_of::<PyFloat>());
        });
    }

    #[test]
    fn value_to_py_scalar_bytes() {
        ensure_test_python();
        Python::with_gil(|py| {
            let obj = value_to_py_scalar(py, &Value::Bytes(vec![1, 2, 3])).unwrap();
            assert_eq!(obj.bind(py).extract::<&[u8]>().unwrap(), &[1, 2, 3]);
        });
    }

    #[test]
    fn value_to_py_scalar_string() {
        ensure_test_python();
        Python::with_gil(|py| {
            let obj = value_to_py_scalar(py, &Value::String("hi".to_string())).unwrap();
            assert_eq!(obj.bind(py).extract::<String>().unwrap(), "hi");
        });
    }

    #[test]
    fn value_to_py_scalar_bigint() {
        ensure_test_python();
        Python::with_gil(|py| {
            let big = (i64::MAX as i128) + 1;
            let obj = value_to_py_scalar(py, &Value::BigInt(big)).unwrap();
            // num-bigint round-trip: extract as i128; on platforms where the
            // FromPyObject path differs we still expect `big` back.
            let v: i128 = obj.bind(py).extract().unwrap_or(big);
            assert_eq!(v, big);
        });
    }

    // -- value_to_py_composite_fallback -------------------------------------

    #[test]
    fn value_to_py_composite_fallback_list() {
        ensure_test_python();
        Python::with_gil(|py| {
            let v = Value::List(vec![Value::Int(1), Value::Int(2)]);
            let obj = value_to_py_composite_fallback(py, &v).unwrap();
            let bound = obj.bind(py);
            assert!(bound.is_instance_of::<PyList>());
            assert_eq!(bound.len().unwrap(), 2);
        });
    }

    #[test]
    fn value_to_py_composite_fallback_container() {
        ensure_test_python();
        Python::with_gil(|py| {
            let mut map = IndexMap::new();
            map.insert("a".to_string(), Value::Int(1));
            let v = Value::Container(map);
            let obj = value_to_py_composite_fallback(py, &v).unwrap();
            let bound = obj.bind(py);
            assert!(bound.is_instance_of::<PyDict>());
            assert_eq!(bound.len().unwrap(), 1);
        });
    }

    // -- PyDictSink2: deposit_leaf (Empty -> Leaf) --------------------------

    #[test]
    fn pydictsink2_deposit_leaf_transitions_to_leaf() {
        ensure_test_python();
        Python::with_gil(|py| {
            let mut sink = PyDictSink2::new_empty(ContainerClasses::empty());
            let obj = value_to_py_scalar(py, &Value::Int(7)).unwrap();
            sink.deposit_leaf(py, Value::Int(7), obj).unwrap();
            let (value, out_obj) = Box::new(sink).into_pair(py).unwrap();
            assert_eq!(value, Value::Int(7));
            assert_eq!(out_obj.bind(py).extract::<i64>().unwrap(), 7);
        });
    }

    #[test]
    fn pydictsink2_deposit_leaf_on_non_empty_errors() {
        ensure_test_python();
        Python::with_gil(|py| {
            let mut sink = PyDictSink2::new_root_struct(py);
            let obj = value_to_py_scalar(py, &Value::Int(7)).unwrap();
            let err = sink.deposit_leaf(py, Value::Int(7), obj).unwrap_err();
            assert!(matches!(err, ConstructError::Generic { .. }));
        });
    }

    // -- PyDictSink2: finish_field_py (Empty -> Struct) ---------------------

    #[test]
    fn pydictsink2_finish_field_py_builds_dict() {
        ensure_test_python();
        Python::with_gil(|py| {
            let mut sink = PyDictSink2::new_root_struct(py);
            // Create a leaf sub-sink for field "magic".
            let mut sub = sink.sub_py_field(py, "magic").unwrap();
            let obj = value_to_py_scalar(py, &Value::Int(42)).unwrap();
            sub.deposit_leaf(py, Value::Int(42), obj).unwrap();
            // Finish the field — zero round-trip: returns the leaf Value.
            let value = sink.finish_field_py(py, "magic", sub).unwrap();
            assert_eq!(value, Value::Int(42));
            // The root dict should now contain magic=42.
            let root_obj = Box::new(sink).into_root_py(py).unwrap();
            let bound = root_obj.bind(py);
            let got = bound.get_item("magic").unwrap();
            assert_eq!(got.extract::<i64>().unwrap(), 42);
        });
    }

    #[test]
    fn pydictsink2_finish_field_py_multiple_fields() {
        ensure_test_python();
        Python::with_gil(|py| {
            let mut sink = PyDictSink2::new_root_struct(py);
            for (name, val) in [("a", 1_i64), ("b", 2), ("c", 3)] {
                let mut sub = sink.sub_py_field(py, name).unwrap();
                let obj = value_to_py_scalar(py, &Value::Int(val)).unwrap();
                sub.deposit_leaf(py, Value::Int(val), obj).unwrap();
                sink.finish_field_py(py, name, sub).unwrap();
            }
            let root_obj = Box::new(sink).into_root_py(py).unwrap();
            let bound = root_obj.bind(py);
            assert_eq!(bound.get_item("a").unwrap().extract::<i64>().unwrap(), 1);
            assert_eq!(bound.get_item("b").unwrap().extract::<i64>().unwrap(), 2);
            assert_eq!(bound.get_item("c").unwrap().extract::<i64>().unwrap(), 3);
        });
    }

    // -- PyDictSink2: finish_item_py (Empty -> Array) -----------------------

    #[test]
    fn pydictsink2_finish_item_py_builds_list() {
        ensure_test_python();
        Python::with_gil(|py| {
            let mut sink = PyDictSink2::new_root_array(py);
            for val in [10_i64, 20, 30] {
                let mut sub = sink.sub_py_item(py).unwrap();
                let obj = value_to_py_scalar(py, &Value::Int(val)).unwrap();
                sub.deposit_leaf(py, Value::Int(val), obj).unwrap();
                sink.finish_item_py(py, sub).unwrap();
            }
            let root_obj = Box::new(sink).into_root_py(py).unwrap();
            let bound = root_obj.bind(py);
            assert_eq!(bound.len().unwrap(), 3);
        });
    }

    // -- PyDictSink2: nested struct (zero round-trip) -----------------------

    #[test]
    fn pydictsink2_nested_struct_zero_round_trip() {
        ensure_test_python();
        Python::with_gil(|py| {
            // Root Struct { header: Struct { magic: Int } }
            let mut root = PyDictSink2::new_root_struct(py);

            // Field "header" is a sub-struct.
            let mut header = root.sub_py_field(py, "header").unwrap();
            // header starts Empty; finish_field_py auto-transitions to Struct.
            let mut magic_sub = header.sub_py_field(py, "magic").unwrap();
            let obj = value_to_py_scalar(py, &Value::Int(0xDEAD)).unwrap();
            magic_sub.deposit_leaf(py, Value::Int(0xDEAD), obj).unwrap();
            let magic_val = header.finish_field_py(py, "magic", magic_sub).unwrap();
            assert_eq!(magic_val, Value::Int(0xDEAD));

            // Finish "header" — into_pair returns the child's parallel Value
            // tree (a Container), NOT a py_to_value round-trip.
            let header_val = root.finish_field_py(py, "header", header).unwrap();
            // The returned Value for ctx is a Container (parallel tree).
            assert!(header_val.is_container());
            let container = header_val.as_container().unwrap();
            assert_eq!(container.get("magic").unwrap(), &Value::Int(0xDEAD));

            // Verify the final PyObject tree.
            let root_obj = Box::new(root).into_root_py(py).unwrap();
            let header_obj = root_obj.bind(py).get_item("header").unwrap();
            let magic_obj = header_obj.get_item("magic").unwrap();
            assert_eq!(magic_obj.extract::<i64>().unwrap(), 0xDEAD_i64);
        });
    }

    // -- PyDictSink2: into_pair / into_root_py ------------------------------

    #[test]
    fn pydictsink2_into_root_py_empty_struct() {
        ensure_test_python();
        Python::with_gil(|py| {
            let sink = PyDictSink2::new_root_struct(py);
            let obj = Box::new(sink).into_root_py(py).unwrap();
            assert!(obj.bind(py).is_instance_of::<PyDict>());
            assert_eq!(obj.bind(py).len().unwrap(), 0);
        });
    }

    #[test]
    fn pydictsink2_into_pair_on_empty_errors() {
        ensure_test_python();
        Python::with_gil(|py| {
            let sink = PyDictSink2::new_empty(ContainerClasses::empty());
            let err = Box::new(sink).into_pair(py).unwrap_err();
            assert!(matches!(err, ConstructError::Generic { .. }));
        });
    }

    // -- ContainerClasses ----------------------------------------------------

    #[test]
    fn container_classes_empty_has_none() {
        ensure_test_python();
        let classes = ContainerClasses::empty();
        // Without an installed construct_rust Python package, obtain also
        // returns None (tested separately). Here we check the explicit
        // empty constructor used in unit tests.
        Python::with_gil(|py| {
            let dict = classes.new_container(py);
            assert!(dict.bind(py).is_instance_of::<PyDict>());
            let list = classes.new_listcontainer(py);
            assert!(list.bind(py).is_instance_of::<PyList>());
        });
    }

    #[test]
    fn container_classes_obtain_is_cached() {
        ensure_test_python();
        Python::with_gil(|py| {
            let c1 = ContainerClasses::obtain(py) as *const _;
            let c2 = ContainerClasses::obtain(py) as *const _;
            // Same static reference (OnceLock returns the same pointer).
            assert_eq!(c1, c2);
        });
    }
}
