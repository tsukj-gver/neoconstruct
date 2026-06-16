//! [`PyContextView`] — lazy Python view over a Rust [`Context`] snapshot.
//!
//! Implements Python's `Mapping` protocol (`__getitem__`, `__len__`,
//! `__contains__`, `keys`, `values`, `items`) over an **owned** `Context`
//! clone. Each field access converts a single `Value → PyObject` on demand —
//! never recursively builds the entire Container upfront.
//!
//! This replaces the old `context_to_py_container` full-recursion approach:
//! for a 100-field context, accessing one field now costs one conversion
//! instead of 100.
//!
//! # Memory safety (I-COMP-1)
//!
//! The view holds an owned `Context` snapshot (deep clone), not a raw pointer.
//! This eliminates use-after-free risk: even if Python code retains the view
//! after the parse/build frame that created it, the snapshot stays valid.
//!
//! # Special keys
//!
//! Mirroring Python construct's context model:
//!
//! | Key       | Meaning                                                 |
//! |-----------|---------------------------------------------------------|
//! | `_`       | Parent context view (or `KeyError` if none)            |
//! | `_root`   | Root struct context view (or `KeyError` if none)       |
//! | `_params` | Topmost (entry/params) context view (always available) |
//!
//! Each special key returns a **new** `PyContextView` holding a derived
//! snapshot, not a pre-built Container — preserving laziness.

use std::cell::RefCell;

use construct::core::context::Context;

use indexmap::IndexMap;
use pyo3::exceptions::{PyAttributeError, PyKeyError};
use pyo3::prelude::*;
use pyo3::types::PyList;

use crate::conversions::value_to_py;

// ---------------------------------------------------------------------------
// Helper functions (mirrors conversions.rs private functions)
// ---------------------------------------------------------------------------

/// Walks the parent chain to find the topmost context (the one with no parent).
///
/// This corresponds to the entry/params context created by `parse`/`build`.
fn find_topmost(ctx: &Context) -> &Context {
    let mut current = ctx;
    while let Some(parent) = current.parent() {
        current = parent;
    }
    current
}

/// Finds the root struct context: the context whose parent is the entry
/// (params) context (i.e., the parent has no grandparent).
///
/// Returns `None` if `ctx` is the entry context itself or the chain is too
/// shallow to have a distinct root struct context.
fn find_root(ctx: &Context) -> Option<&Context> {
    let mut current = ctx;
    while let Some(parent) = current.parent() {
        if parent.parent().is_none() {
            return Some(current);
        }
        current = parent;
    }
    None
}

// ---------------------------------------------------------------------------
// PyContextView
// ---------------------------------------------------------------------------

/// Lazy Python view over a Rust [`Context`] snapshot.
///
/// Implements the `Mapping` protocol (`__getitem__` / `__len__` /
/// `__contains__` / `keys` / `values` / `items`). Each field access converts a
/// **single** `Value → PyObject` on demand — never recursively builds the
/// entire Container.
///
/// # I-COMP-1: owned snapshot
///
/// The view holds an owned `Context` clone taken at creation time. This is
/// safe even if the original `Context` (on the call stack of a `parse` /
/// `build` frame) is later dropped, because the snapshot is self-contained.
#[pyclass(name = "ContextView", unsendable)]
pub struct PyContextView {
    /// Owned snapshot of the Rust Context at creation time.
    ctx: Context,
    /// Field-level conversion cache (key → PyObject). Repeat accesses to the
    /// same field skip the `Value → PyObject` conversion after the first hit.
    cache: RefCell<IndexMap<String, PyObject>>,
}

impl PyContextView {
    /// Creates a [`PyContextView`] from a live [`Context`] (during PyCallback
    /// or `ExprExtension` evaluation).
    ///
    /// Takes a snapshot (clone) of the Context — safe even if the original
    /// Context is later dropped.
    #[must_use]
    pub fn from_context(ctx: &Context) -> Self {
        Self {
            ctx: ctx.clone(),
            cache: RefCell::new(IndexMap::new()),
        }
    }

    /// Returns the parent context view for the `_` special key.
    fn parent_view(&self, py: Python<'_>) -> PyResult<PyObject> {
        match self.ctx.parent() {
            Some(parent) => {
                let view = Self::from_context(parent);
                Ok(Py::new(py, view)?.into_any())
            }
            None => Err(PyKeyError::new_err("_")),
        }
    }

    /// Returns the root struct context view for the `_root` special key.
    fn root_view(&self, py: Python<'_>) -> PyResult<PyObject> {
        match find_root(&self.ctx) {
            Some(root) => {
                let view = Self::from_context(root);
                Ok(Py::new(py, view)?.into_any())
            }
            None => Err(PyKeyError::new_err("_root")),
        }
    }

    /// Returns the topmost (params) context view for the `_params` key.
    ///
    /// The params context always exists — at worst it is `self` when there is
    /// no parent chain.
    fn params_view(&self, py: Python<'_>) -> PyResult<PyObject> {
        let params = find_topmost(&self.ctx);
        let view = Self::from_context(params);
        Ok(Py::new(py, view)?.into_any())
    }
}

// ---------------------------------------------------------------------------
// Python Mapping protocol
// ---------------------------------------------------------------------------

#[pymethods]
impl PyContextView {
    /// Get a field value by name.
    ///
    /// Handles special keys (`_`, `_root`, `_params`) by returning lazy
    /// sub-views. Normal keys are looked up at the **current context level**
    /// (matching Python `Container` dict semantics) and cached after the first
    /// conversion.
    ///
    /// # Errors
    ///
    /// Returns `KeyError` if the key is not found.
    fn __getitem__(&self, py: Python<'_>, key: &str) -> PyResult<PyObject> {
        // 1. Check conversion cache (fast path for repeat accesses).
        {
            let cache = self.cache.borrow();
            if let Some(cached) = cache.get(key) {
                return Ok(cached.clone_ref(py));
            }
        }
        // Immutable borrow released here — safe to borrow_mut below.

        // 2. Resolve the value (special keys → sub-views, normal → local field).
        let py_obj = match key {
            "_" => self.parent_view(py)?,
            "_root" => self.root_view(py)?,
            "_params" => self.params_view(py)?,
            _ => match self.ctx.get(key) {
                Some(value) => value_to_py(py, value)?,
                None => return Err(PyKeyError::new_err(format!("Context has no key {key}"))),
            },
        };

        // 3. Cache and return.
        self.cache
            .borrow_mut()
            .insert(key.to_string(), py_obj.clone_ref(py));
        Ok(py_obj)
    }

    /// Attribute access delegates to item access (`ctx.field` → `ctx["field"]`).
    ///
    /// Dunder names (`__xxx__`) are excluded so they fall through to Python's
    /// normal `AttributeError`, preventing interference with internal lookups.
    fn __getattr__(&self, py: Python<'_>, name: &str) -> PyResult<PyObject> {
        if name.starts_with("__") && name.ends_with("__") {
            return Err(PyAttributeError::new_err(name.to_string()));
        }
        self.__getitem__(py, name)
    }

    /// Number of local fields in this context level (excludes special keys).
    fn __len__(&self) -> usize {
        self.ctx.iter_fields().count()
    }

    /// Check if a key exists (including special keys when applicable).
    fn __contains__(&self, key: &str) -> bool {
        match key {
            "_" => self.ctx.parent().is_some(),
            "_root" => find_root(&self.ctx).is_some(),
            "_params" => true,
            _ => self.ctx.get(key).is_some(),
        }
    }

    /// Returns a string representation for debugging.
    fn __repr__(&self) -> String {
        let names: Vec<&str> = self.ctx.iter_fields().map(|(k, _)| k.as_str()).collect();
        format!("ContextView({{{}}})", names.join(", "))
    }

    /// Return keys as a Python list (including available special keys).
    fn keys(&self, py: Python<'_>) -> PyResult<PyObject> {
        let mut keys: Vec<PyObject> = self
            .ctx
            .iter_fields()
            .map(|(k, _)| k.to_object(py))
            .collect();
        if self.ctx.parent().is_some() {
            keys.push("_".to_object(py));
        }
        if find_root(&self.ctx).is_some() {
            keys.push("_root".to_object(py));
        }
        keys.push("_params".to_object(py));
        Ok(PyList::new_bound(py, keys).into_any().unbind())
    }

    /// Return values as a Python list (normal fields only).
    fn values(&self, py: Python<'_>) -> PyResult<Vec<PyObject>> {
        let names: Vec<String> = self.ctx.iter_fields().map(|(k, _)| k.clone()).collect();
        let mut values = Vec::with_capacity(names.len());
        for name in names {
            values.push(self.__getitem__(py, &name)?);
        }
        Ok(values)
    }

    /// Return `(key, value)` pairs as a Python list (normal fields only).
    fn items(&self, py: Python<'_>) -> PyResult<Vec<(PyObject, PyObject)>> {
        let names: Vec<String> = self.ctx.iter_fields().map(|(k, _)| k.clone()).collect();
        let mut pairs = Vec::with_capacity(names.len());
        for name in names {
            let key = name.to_object(py);
            let val = self.__getitem__(py, &name)?;
            pairs.push((key, val));
        }
        Ok(pairs)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use construct::value::Value;
    use pyo3::exceptions::PyKeyError;
    use pyo3::types::{PyBool, PyList as PyListType};

    // -- Helper: build a 3-level context (params → root → child) ------------

    /// Builds:
    /// ```text
    /// params: { param1: 100 }
    ///   └─ root: { field1: 1, field2: "hello" }
    ///        └─ child: { child_field: 42 }
    /// ```
    fn build_sample_context() -> Context {
        let mut params = Context::new();
        params.insert("param1", Value::Int(100));

        let mut root = params.subcontext();
        root.insert("field1", Value::Int(1));
        root.insert("field2", Value::String("hello".to_string()));

        let mut child = root.subcontext();
        child.insert("child_field", Value::Int(42));
        child
    }

    // -- Basic field access -------------------------------------------------

    #[test]
    fn getitem_returns_local_field() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let ctx = build_sample_context();
            let view = PyContextView::from_context(&ctx);
            let val = view.__getitem__(py, "child_field").unwrap();
            let i: i64 = val.extract(py).unwrap();
            assert_eq!(i, 42);
        });
    }

    #[test]
    fn getitem_missing_key_raises_keyerror() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let ctx = build_sample_context();
            let view = PyContextView::from_context(&ctx);
            let err = view.__getitem__(py, "nonexistent").unwrap_err();
            assert!(err.is_instance_of::<PyKeyError>(py));
        });
    }

    #[test]
    fn getitem_local_only_does_not_find_parent_fields() {
        crate::ensure_python();
        Python::with_gil(|py| {
            // child does not have "field1" locally (it's in root/parent)
            let ctx = build_sample_context();
            let view = PyContextView::from_context(&ctx);
            let err = view.__getitem__(py, "field1").unwrap_err();
            assert!(err.is_instance_of::<PyKeyError>(py));
        });
    }

    // -- Caching ------------------------------------------------------------

    #[test]
    fn cache_returns_same_value_on_repeat_access() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let ctx = build_sample_context();
            let view = PyContextView::from_context(&ctx);

            // First access: converts Value → PyObject
            let val1 = view.__getitem__(py, "child_field").unwrap();
            // Second access: should return cached PyObject (same identity)
            let val2 = view.__getitem__(py, "child_field").unwrap();
            assert!(val1.bind(py).is(val2.bind(py)));
        });
    }

    #[test]
    fn cache_independent_for_different_keys() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let mut ctx = Context::new();
            ctx.insert("a", Value::Int(1));
            ctx.insert("b", Value::Int(2));
            let view = PyContextView::from_context(&ctx);

            let a = view.__getitem__(py, "a").unwrap();
            let b = view.__getitem__(py, "b").unwrap();
            let i_a: i64 = a.extract(py).unwrap();
            let i_b: i64 = b.extract(py).unwrap();
            assert_eq!(i_a, 1);
            assert_eq!(i_b, 2);
        });
    }

    // -- Special keys: `_` (parent) ----------------------------------------

    #[test]
    fn underscore_returns_parent_view() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let ctx = build_sample_context();
            let view = PyContextView::from_context(&ctx);

            let parent_obj = view.__getitem__(py, "_").unwrap();
            let parent_view = parent_obj.bind(py);
            // Verify it's a ContextView by checking its type name
            let type_name: String = parent_view
                .get_type()
                .name()
                .map(|n| n.to_string())
                .unwrap_or_default();
            assert_eq!(type_name, "ContextView");
            // Parent (root) has field1
            let val = parent_view.get_item("field1").unwrap();
            let i: i64 = val.extract().unwrap();
            assert_eq!(i, 1);
        });
    }

    #[test]
    fn underscore_keyerror_when_no_parent() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let ctx = Context::new();
            let view = PyContextView::from_context(&ctx);
            let err = view.__getitem__(py, "_").unwrap_err();
            assert!(err.is_instance_of::<PyKeyError>(py));
        });
    }

    // -- Special keys: `_root` ----------------------------------------------

    #[test]
    fn root_key_returns_root_context_view() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let ctx = build_sample_context();
            let view = PyContextView::from_context(&ctx);

            let root_obj = view.__getitem__(py, "_root").unwrap();
            let root_view = root_obj.bind(py);
            // Root context has field1 = 1
            let val = root_view.get_item("field1").unwrap();
            let i: i64 = val.extract().unwrap();
            assert_eq!(i, 1);
        });
    }

    #[test]
    fn root_keyerror_when_no_distinct_root() {
        crate::ensure_python();
        Python::with_gil(|py| {
            // Context with no parent → no root
            let mut ctx = Context::new();
            ctx.insert("x", Value::Int(1));
            let view = PyContextView::from_context(&ctx);
            let err = view.__getitem__(py, "_root").unwrap_err();
            assert!(err.is_instance_of::<PyKeyError>(py));
        });
    }

    // -- Special keys: `_params` --------------------------------------------

    #[test]
    fn params_key_returns_topmost_context() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let ctx = build_sample_context();
            let view = PyContextView::from_context(&ctx);

            let params_obj = view.__getitem__(py, "_params").unwrap();
            let params_view = params_obj.bind(py);
            // Params context has param1 = 100
            let val = params_view.get_item("param1").unwrap();
            let i: i64 = val.extract().unwrap();
            assert_eq!(i, 100);
        });
    }

    #[test]
    fn params_key_works_without_parent() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let mut ctx = Context::new();
            ctx.insert("only", Value::Int(5));
            let view = PyContextView::from_context(&ctx);

            // _params on root-less context returns self
            let params_obj = view.__getitem__(py, "_params").unwrap();
            let params_view = params_obj.bind(py);
            let val = params_view.get_item("only").unwrap();
            let i: i64 = val.extract().unwrap();
            assert_eq!(i, 5);
        });
    }

    // -- __len__ / __contains__ --------------------------------------------

    #[test]
    fn len_returns_local_field_count() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let ctx = build_sample_context();
            let view = PyContextView::from_context(&ctx);
            // child level has 1 field
            assert_eq!(view.__len__(), 1);

            // root level has 2 fields
            let root_view = view.__getitem__(py, "_").unwrap();
            let root = root_view.bind(py);
            let len: usize = root
                .getattr("__len__")
                .unwrap()
                .call0()
                .unwrap()
                .extract()
                .unwrap();
            assert_eq!(len, 2);
        });
    }

    #[test]
    fn contains_local_field() {
        crate::ensure_python();
        let ctx = build_sample_context();
        let view = PyContextView::from_context(&ctx);

        assert!(view.__contains__("child_field"));
        assert!(!view.__contains__("field1")); // parent's field
        assert!(view.__contains__("_"));
        assert!(view.__contains__("_root"));
        assert!(view.__contains__("_params"));
    }

    #[test]
    fn contains_no_underscore_without_parent() {
        let ctx = Context::new();
        let view = PyContextView::from_context(&ctx);
        assert!(!view.__contains__("_"));
        assert!(!view.__contains__("_root"));
        assert!(view.__contains__("_params"));
    }

    // -- keys / values / items ---------------------------------------------

    #[test]
    fn keys_returns_field_names_plus_special() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let ctx = build_sample_context();
            let view = PyContextView::from_context(&ctx);

            let keys_obj = view.keys(py).unwrap();
            let keys_list = keys_obj.bind(py).downcast::<PyListType>().unwrap();
            let keys: Vec<String> = keys_list
                .iter()
                .map(|k| k.extract::<String>().unwrap())
                .collect();
            assert!(keys.contains(&"child_field".to_string()));
            assert!(keys.contains(&"_".to_string()));
            assert!(keys.contains(&"_root".to_string()));
            assert!(keys.contains(&"_params".to_string()));
        });
    }

    #[test]
    fn values_returns_converted_field_values() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let mut ctx = Context::new();
            ctx.insert("a", Value::Int(1));
            ctx.insert("b", Value::Bool(true));
            let view = PyContextView::from_context(&ctx);

            let values = view.values(py).unwrap();
            assert_eq!(values.len(), 2);
            // Values are PyObjects — extract and check
            let v0: i64 = values[0].extract(py).unwrap();
            let v1: bool = values[1].extract(py).unwrap();
            assert_eq!(v0, 1);
            assert!(v1);
        });
    }

    #[test]
    fn items_returns_key_value_pairs() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let mut ctx = Context::new();
            ctx.insert("x", Value::Int(42));
            let view = PyContextView::from_context(&ctx);

            let items = view.items(py).unwrap();
            assert_eq!(items.len(), 1);
            let key: String = items[0].0.extract(py).unwrap();
            let val: i64 = items[0].1.extract(py).unwrap();
            assert_eq!(key, "x");
            assert_eq!(val, 42);
        });
    }

    // -- Attribute access (__getattr__) -------------------------------------

    #[test]
    fn getattr_delegates_to_getitem() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let mut ctx = Context::new();
            ctx.insert("my_field", Value::Int(99));
            let view = PyContextView::from_context(&ctx);

            // ctx.my_field should work via __getattr__
            let val = view.__getattr__(py, "my_field").unwrap();
            let i: i64 = val.extract(py).unwrap();
            assert_eq!(i, 99);
        });
    }

    #[test]
    fn getattr_dunder_returns_attribute_error() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let ctx = Context::new();
            let view = PyContextView::from_context(&ctx);

            let err = view.__getattr__(py, "__deepcopy__").unwrap_err();
            assert!(err.is_instance_of::<PyAttributeError>(py));
        });
    }

    // -- Value type coverage ------------------------------------------------

    #[test]
    fn getitem_container_value() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let mut inner = indexmap::IndexMap::new();
            inner.insert("nested".to_string(), Value::Int(7));
            let mut ctx = Context::new();
            ctx.insert("data", Value::Container(inner));
            let view = PyContextView::from_context(&ctx);

            let val = view.__getitem__(py, "data").unwrap();
            // Container converts to dict (or Container subclass)
            let converted = crate::conversions::py_to_value(py, val.bind(py)).unwrap();
            let container = converted.as_container().unwrap();
            assert_eq!(container.get("nested").unwrap(), &Value::Int(7));
        });
    }

    #[test]
    fn getitem_list_value() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let mut ctx = Context::new();
            ctx.insert(
                "items",
                Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
            );
            let view = PyContextView::from_context(&ctx);

            let val = view.__getitem__(py, "items").unwrap();
            let list = val.bind(py).downcast::<PyListType>().unwrap();
            assert_eq!(list.len(), 3);
        });
    }

    #[test]
    fn getitem_bool_value() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let mut ctx = Context::new();
            ctx.insert("flag", Value::Bool(true));
            let view = PyContextView::from_context(&ctx);

            let val = view.__getitem__(py, "flag").unwrap();
            assert!(val.bind(py).is_instance_of::<PyBool>());
        });
    }

    #[test]
    fn getitem_none_value() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let mut ctx = Context::new();
            ctx.insert("nothing", Value::None);
            let view = PyContextView::from_context(&ctx);

            let val = view.__getitem__(py, "nothing").unwrap();
            assert!(val.is_none(py));
        });
    }

    // -- Deep traversal via `_` chain ---------------------------------------

    #[test]
    fn double_underscore_traverses_two_levels() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let ctx = build_sample_context();
            let view = PyContextView::from_context(&ctx);

            // child["_"]["_"] → root's parent → params
            let parent = view.__getitem__(py, "_").unwrap();
            let parent_view = parent.bind(py);
            let grandparent = parent_view.get_item("_").unwrap();
            let val = grandparent.get_item("param1").unwrap();
            let i: i64 = val.extract().unwrap();
            assert_eq!(i, 100);
        });
    }

    // -- Repr ---------------------------------------------------------------

    #[test]
    fn repr_includes_field_names() {
        let mut ctx = Context::new();
        ctx.insert("alpha", Value::Int(1));
        ctx.insert("beta", Value::Int(2));
        let view = PyContextView::from_context(&ctx);
        let repr = view.__repr__();
        assert!(repr.contains("alpha"));
        assert!(repr.contains("beta"));
        assert!(repr.starts_with("ContextView("));
    }

    #[test]
    fn repr_empty_context() {
        let ctx = Context::new();
        let view = PyContextView::from_context(&ctx);
        let repr = view.__repr__();
        assert_eq!(repr, "ContextView({})");
    }
}
