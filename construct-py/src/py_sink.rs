//! Direct-to-Python output sink — writes parse results straight into a
//! `PyDict` / `PyList`, avoiding intermediate `Value::Container` trees.
//!
//! This is the Phase 13 [`OutputSink`] implementation. During `exec_parse`,
//! compiled nodes write their results to a [`PyDictSink`] which directly
//! assembles Python `dict` / `list` objects — no `Value::Container`
//! intermediate tree is built.
//!
//! # Mode transitions
//!
//! A [`PyDictSink`] starts in [`PySinkMode::Empty`] and transitions based on
//! the first write operation:
//!
//! | First call          | Resulting mode        |
//! |---------------------|-----------------------|
//! | `set_scalar(v)`     | `Leaf(Some(v))`       |
//! | `set_field(name,v)` | `Struct { dict }`     |
//! | `push_item(v)`      | `Array { list }`      |
//!
//! Once a mode is set, mismatched operations return `Err`.

use construct::compiled::sink::{OutputSink, ProducedOutput};
use construct::core::error::{ConstructError, Result};
use construct::value::Value;

use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyList};

use crate::conversions::{py_to_value, value_to_py};

/// Converts a [`PyErr`] into a [`ConstructError::Generic`].
///
/// This is a lossy mapping — the Python exception type and traceback are
/// stringified into the `message` field. The `path` is left empty because
/// the sink layer does not track construct-tree position (path enrichment
/// happens at the `exec_parse` level via `with_path_prefix`).
pub(crate) fn py_err_to_construct(e: PyErr) -> ConstructError {
    ConstructError::Generic {
        path: String::new(),
        message: e.to_string(),
    }
}

/// Internal mode of a [`PyDictSink`].
///
/// Tracks whether the sink has received a leaf scalar, a named-field write
/// (Struct/dict), or a positional-item write (Array/list).
enum PySinkMode {
    /// Nothing written yet; transitions on first write.
    Empty,
    /// Leaf: holds a single scalar `Value` deposited by `set_scalar`.
    /// `into_produced` returns `ProducedOutput::Value` — the caller
    /// (parent's `finish_field`) converts to `PyObject`.
    Leaf(Option<Value>),
    /// Struct: owns a `PyDict`. `set_field` converts `Value → PyObject`
    /// immediately and calls `PyDict::set_item`. No `Value::Container`
    /// accumulation. `into_produced` returns `ProducedOutput::Object`.
    Struct {
        /// The Python dict being assembled.
        dict: Py<PyDict>,
    },
    /// Array: owns a `PyList`. `push_item` converts + appends immediately.
    /// `into_produced` returns `ProducedOutput::Object`.
    Array {
        /// The Python list being assembled.
        list: Py<PyList>,
    },
}

/// [`OutputSink`] that writes directly to Python `dict` / `list` objects.
///
/// This is the Phase 13 direct-to-Python sink. It **never** accumulates a
/// `Value::Container` tree — composite fields are assembled via
/// `PyDict::set_item` / `PyList::append`, and leaf scalars are stored as
/// `Value` until `finish_field` / `finish_item` converts them.
///
/// # GIL handling
///
/// All FFI operations acquire the GIL via `Python::with_gil`. Since
/// `PyDictSink` is only used in the Python path (`parse_bytes_py`), the GIL
/// is already held by the caller — `with_gil` is essentially a no-op in
/// that case.
pub struct PyDictSink {
    mode: PySinkMode,
}

impl PyDictSink {
    /// Creates a new sink in [`PySinkMode::Empty`].
    ///
    /// The mode transitions automatically based on the first write
    /// operation (`set_scalar`, `set_field`, or `push_item`).
    #[must_use]
    pub fn new() -> Self {
        Self {
            mode: PySinkMode::Empty,
        }
    }

    /// Creates a root sink pre-initialized in [`PySinkMode::Struct`] with a
    /// fresh empty `PyDict`.
    ///
    /// Used by `parse_bytes_py` so that even an empty struct produces an
    /// empty `dict` (rather than `None`).
    #[must_use]
    pub fn new_root(py: Python<'_>) -> Self {
        Self {
            mode: PySinkMode::Struct {
                dict: PyDict::new_bound(py).unbind(),
            },
        }
    }

    /// Consumes the sink and returns the root Python object as `PyObject`.
    ///
    /// This is an inherent method (not part of the [`OutputSink`] trait)
    /// used by `parse_bytes_py` to extract the final result without going
    /// through `Box<dyn Any>` boxing/unboxing.
    ///
    /// # Errors
    ///
    /// Returns `PyErr` if a `Value → PyObject` conversion fails.
    pub fn into_py_object(self) -> PyResult<PyObject> {
        match self.mode {
            PySinkMode::Empty => Python::with_gil(|py| Ok(py.None())),
            PySinkMode::Leaf(v) => {
                let val = v.unwrap_or(Value::None);
                Python::with_gil(|py| value_to_py(py, &val))
            }
            PySinkMode::Struct { dict } => Ok(dict.into_any()),
            PySinkMode::Array { list } => Ok(list.into_any()),
        }
    }
}

impl Default for PyDictSink {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// OutputSink trait implementation
// ===========================================================================

impl OutputSink for PyDictSink {
    fn set_scalar(&mut self, value: Value) -> Result<()> {
        if matches!(self.mode, PySinkMode::Empty | PySinkMode::Leaf(_)) {
            self.mode = PySinkMode::Leaf(Some(value));
            Ok(())
        } else {
            Err(ConstructError::Generic {
                path: String::new(),
                message: "PyDictSink::set_scalar on composite (Struct/Array) sink".into(),
            })
        }
    }

    fn set_field(&mut self, name: &str, value: Value) -> Result<()> {
        // Compute potential new mode without holding a mutable borrow.
        let new_mode: Option<PySinkMode> = match &self.mode {
            PySinkMode::Empty => {
                let dict = Python::with_gil(|py| -> Result<Py<PyDict>> {
                    let dict = PyDict::new_bound(py);
                    let py_obj = value_to_py(py, &value).map_err(py_err_to_construct)?;
                    dict.set_item(name, py_obj).map_err(py_err_to_construct)?;
                    Ok(dict.unbind())
                })?;
                Some(PySinkMode::Struct { dict })
            }
            PySinkMode::Struct { dict } => {
                Python::with_gil(|py| -> Result<()> {
                    let py_obj = value_to_py(py, &value).map_err(py_err_to_construct)?;
                    dict.bind(py)
                        .set_item(name, py_obj)
                        .map_err(py_err_to_construct)?;
                    Ok(())
                })?;
                None // No mode change.
            }
            _ => {
                return Err(ConstructError::Generic {
                    path: String::new(),
                    message: format!("PyDictSink::set_field({name}) on non-Struct sink"),
                });
            }
        };
        if let Some(mode) = new_mode {
            self.mode = mode;
        }
        Ok(())
    }

    fn push_item(&mut self, value: Value) -> Result<()> {
        let new_mode: Option<PySinkMode> = match &self.mode {
            PySinkMode::Empty => {
                let list = Python::with_gil(|py| -> Result<Py<PyList>> {
                    let list = PyList::empty_bound(py);
                    let py_obj = value_to_py(py, &value).map_err(py_err_to_construct)?;
                    list.append(py_obj).map_err(py_err_to_construct)?;
                    Ok(list.unbind())
                })?;
                Some(PySinkMode::Array { list })
            }
            PySinkMode::Array { list } => {
                Python::with_gil(|py| -> Result<()> {
                    let py_obj = value_to_py(py, &value).map_err(py_err_to_construct)?;
                    list.bind(py).append(py_obj).map_err(py_err_to_construct)?;
                    Ok(())
                })?;
                None
            }
            _ => {
                return Err(ConstructError::Generic {
                    path: String::new(),
                    message: "PyDictSink::push_item on non-Array sink".into(),
                });
            }
        };
        if let Some(mode) = new_mode {
            self.mode = mode;
        }
        Ok(())
    }

    fn sub_sink_for_field(&mut self, _name: &str) -> Result<Box<dyn OutputSink>> {
        Ok(Box::new(PyDictSink::new()))
    }

    fn sub_sink_for_item(&mut self) -> Result<Box<dyn OutputSink>> {
        Ok(Box::new(PyDictSink::new()))
    }

    fn finish_subsink(&mut self, sub: Box<dyn OutputSink>) -> Result<()> {
        let produced = sub.into_produced()?;
        match &self.mode {
            PySinkMode::Array { list } => Python::with_gil(|py| {
                let py_obj: Py<PyAny> = match produced {
                    ProducedOutput::Value(v) => value_to_py(py, &v).map_err(py_err_to_construct)?,
                    ProducedOutput::Object(obj) => obj
                        .downcast_ref::<Py<PyAny>>()
                        .ok_or_else(|| ConstructError::Generic {
                            path: String::new(),
                            message: "PyDictSink: failed to downcast Object to Py<PyAny>".into(),
                        })?
                        .clone_ref(py),
                };
                list.bind(py).append(py_obj).map_err(py_err_to_construct)?;
                Ok(())
            }),
            _ => Err(ConstructError::Generic {
                path: String::new(),
                message: "PyDictSink::finish_subsink on non-Array sink".into(),
            }),
        }
    }

    fn into_value(self: Box<Self>) -> Result<Value> {
        let sink = *self;
        match sink.mode {
            PySinkMode::Empty => Ok(Value::None),
            PySinkMode::Leaf(v) => Ok(v.unwrap_or(Value::None)),
            PySinkMode::Struct { dict } => Python::with_gil(|py| {
                let bound = dict.bind(py);
                py_to_value(py, bound.as_any()).map_err(py_err_to_construct)
            }),
            PySinkMode::Array { list } => Python::with_gil(|py| {
                let bound = list.bind(py);
                py_to_value(py, bound.as_any()).map_err(py_err_to_construct)
            }),
        }
    }

    fn into_produced(self: Box<Self>) -> Result<ProducedOutput> {
        let sink = *self;
        match sink.mode {
            PySinkMode::Empty => Ok(ProducedOutput::Value(Value::None)),
            PySinkMode::Leaf(v) => Ok(ProducedOutput::Value(v.unwrap_or(Value::None))),
            PySinkMode::Struct { dict } => Ok(ProducedOutput::Object(Box::new(dict.into_any()))),
            PySinkMode::Array { list } => Ok(ProducedOutput::Object(Box::new(list.into_any()))),
        }
    }

    // ── Phase 13 override methods ──

    fn finish_field(&mut self, name: &str, sub: Box<dyn OutputSink>) -> Result<Value> {
        let produced = sub.into_produced()?;
        match &self.mode {
            PySinkMode::Struct { dict } => Python::with_gil(|py| -> Result<Value> {
                let (value_for_ctx, py_obj): (Value, Py<PyAny>) = match produced {
                    ProducedOutput::Value(v) => {
                        let py_obj = value_to_py(py, &v).map_err(py_err_to_construct)?;
                        (v, py_obj)
                    }
                    ProducedOutput::Object(obj) => {
                        let py_ref: &Py<PyAny> = obj
                            .downcast_ref::<Py<PyAny>>()
                            .ok_or_else(|| ConstructError::Generic {
                                path: String::new(),
                                message:
                                    "PyDictSink::finish_field: failed to downcast Object to Py<PyAny>"
                                        .into(),
                            })?;
                        let py_any: Py<PyAny> = py_ref.clone_ref(py);
                        let v = py_to_value(py, py_any.bind(py)).map_err(py_err_to_construct)?;
                        (v, py_any)
                    }
                };
                dict.bind(py)
                    .set_item(name, py_obj)
                    .map_err(py_err_to_construct)?;
                Ok(value_for_ctx)
            }),
            _ => Err(ConstructError::Generic {
                path: String::new(),
                message: format!("PyDictSink::finish_field({name}) on non-Struct sink"),
            }),
        }
    }

    fn finish_named_item(&mut self, _name: &str, sub: Box<dyn OutputSink>) -> Result<Value> {
        let produced = sub.into_produced()?;
        match &self.mode {
            PySinkMode::Array { list } => Python::with_gil(|py| -> Result<Value> {
                let (value_for_ctx, py_obj): (Value, Py<PyAny>) = match produced {
                    ProducedOutput::Value(v) => {
                        let py_obj = value_to_py(py, &v).map_err(py_err_to_construct)?;
                        (v, py_obj)
                    }
                    ProducedOutput::Object(obj) => {
                        let py_ref: &Py<PyAny> =
                            obj.downcast_ref::<Py<PyAny>>().ok_or_else(|| {
                                ConstructError::Generic {
                                    path: String::new(),
                                    message:
                                        "PyDictSink::finish_named_item: failed to downcast Object"
                                            .into(),
                                }
                            })?;
                        let py_any: Py<PyAny> = py_ref.clone_ref(py);
                        let v = py_to_value(py, py_any.bind(py)).map_err(py_err_to_construct)?;
                        (v, py_any)
                    }
                };
                list.bind(py).append(py_obj).map_err(py_err_to_construct)?;
                Ok(value_for_ctx)
            }),
            _ => Err(ConstructError::Generic {
                path: String::new(),
                message: "PyDictSink::finish_named_item on non-Array sink".into(),
            }),
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use construct::compiled::sink::{OutputSink, ProducedOutput, ValueSink};

    // -- set_scalar / Leaf mode --------------------------------------------

    #[test]
    fn dict_sink_set_scalar_on_empty_transitions_to_leaf() {
        crate::ensure_python();
        let mut sink = PyDictSink::new();
        sink.set_scalar(Value::Int(42)).unwrap();
        let boxed: Box<dyn OutputSink> = Box::new(sink);
        let produced = boxed.into_produced().unwrap();
        assert!(matches!(produced, ProducedOutput::Value(Value::Int(42))));
    }

    #[test]
    fn dict_sink_set_scalar_overwrites_previous() {
        crate::ensure_python();
        let mut sink = PyDictSink::new();
        sink.set_scalar(Value::Int(1)).unwrap();
        sink.set_scalar(Value::Int(2)).unwrap();
        let boxed: Box<dyn OutputSink> = Box::new(sink);
        let produced = boxed.into_produced().unwrap();
        assert!(matches!(produced, ProducedOutput::Value(Value::Int(2))));
    }

    #[test]
    fn dict_sink_set_scalar_on_struct_returns_err() {
        crate::ensure_python();
        let mut sink = Python::with_gil(PyDictSink::new_root);
        let err = sink.set_scalar(Value::Int(42)).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    // -- set_field / Struct mode --------------------------------------------

    #[test]
    fn dict_sink_set_field_on_empty_transitions_to_struct() {
        crate::ensure_python();
        let mut sink = PyDictSink::new();
        sink.set_field("a", Value::Int(1)).unwrap();
        sink.set_field("b", Value::String("hello".into())).unwrap();
        let boxed: Box<dyn OutputSink> = Box::new(sink);
        let produced = boxed.into_produced().unwrap();
        match produced {
            ProducedOutput::Object(obj) => {
                Python::with_gil(|py| {
                    let py_any = obj.downcast_ref::<Py<PyAny>>().unwrap();
                    let value = crate::conversions::py_to_value(py, py_any.bind(py)).unwrap();
                    let container = value.as_container().unwrap();
                    assert_eq!(container.get("a").unwrap(), &Value::Int(1));
                    assert_eq!(container.get("b").unwrap(), &Value::String("hello".into()));
                });
            }
            _ => panic!("expected Object"),
        }
    }

    #[test]
    fn dict_sink_set_field_on_leaf_returns_err() {
        crate::ensure_python();
        let mut sink = PyDictSink::new();
        sink.set_scalar(Value::Int(1)).unwrap();
        let err = sink.set_field("x", Value::Int(2)).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    // -- push_item / Array mode ---------------------------------------------

    #[test]
    fn dict_sink_push_item_on_empty_transitions_to_array() {
        crate::ensure_python();
        let mut sink = PyDictSink::new();
        sink.push_item(Value::Int(10)).unwrap();
        sink.push_item(Value::Int(20)).unwrap();
        let boxed: Box<dyn OutputSink> = Box::new(sink);
        let produced = boxed.into_produced().unwrap();
        match produced {
            ProducedOutput::Object(obj) => {
                Python::with_gil(|py| {
                    let py_any = obj.downcast_ref::<Py<PyAny>>().unwrap();
                    let value = crate::conversions::py_to_value(py, py_any.bind(py)).unwrap();
                    let list = value.as_list().unwrap();
                    assert_eq!(list, &vec![Value::Int(10), Value::Int(20)]);
                });
            }
            _ => panic!("expected Object"),
        }
    }

    #[test]
    fn dict_sink_push_item_on_struct_returns_err() {
        crate::ensure_python();
        let mut sink = Python::with_gil(PyDictSink::new_root);
        let err = sink.push_item(Value::Int(1)).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    // -- into_produced / into_value -----------------------------------------

    #[test]
    fn dict_sink_into_produced_empty_returns_none() {
        crate::ensure_python();
        let boxed: Box<dyn OutputSink> = Box::new(PyDictSink::new());
        let produced = boxed.into_produced().unwrap();
        assert!(matches!(produced, ProducedOutput::Value(Value::None)));
    }

    #[test]
    fn dict_sink_into_value_struct_returns_container() {
        crate::ensure_python();
        let mut sink = Python::with_gil(PyDictSink::new_root);
        sink.set_field("x", Value::Int(42)).unwrap();
        let boxed: Box<dyn OutputSink> = Box::new(sink);
        let value = boxed.into_value().unwrap();
        let container = value.as_container().unwrap();
        assert_eq!(container.get("x").unwrap(), &Value::Int(42));
    }

    #[test]
    fn dict_sink_into_value_array_returns_list() {
        crate::ensure_python();
        let mut sink = PyDictSink::new();
        sink.push_item(Value::Int(1)).unwrap();
        sink.push_item(Value::Int(2)).unwrap();
        let boxed: Box<dyn OutputSink> = Box::new(sink);
        let value = boxed.into_value().unwrap();
        let list = value.as_list().unwrap();
        assert_eq!(list, &vec![Value::Int(1), Value::Int(2)]);
    }

    // -- finish_field -------------------------------------------------------

    #[test]
    fn dict_sink_finish_field_with_leaf_sub_writes_dict() {
        crate::ensure_python();
        let mut sink = Python::with_gil(PyDictSink::new_root);
        // Create a sub-sink (leaf), set_scalar, then finish_field.
        let mut sub = sink.sub_sink_for_field("magic").unwrap();
        sub.set_scalar(Value::UInt(0xDEAD)).unwrap();
        let value = sink.finish_field("magic", sub).unwrap();
        assert_eq!(value, Value::UInt(0xDEAD));
        // Verify the field was written to the dict.
        // Note: into_value round-trips through Python (PyObject → Value),
        // which narrows positive ints to Value::Int.
        let boxed: Box<dyn OutputSink> = Box::new(sink);
        let result = boxed.into_value().unwrap();
        let container = result.as_container().unwrap();
        assert_eq!(container.get("magic").unwrap(), &Value::Int(0xDEAD_i64));
    }

    #[test]
    fn dict_sink_finish_field_with_struct_sub_writes_nested_dict() {
        crate::ensure_python();
        let mut sink = Python::with_gil(PyDictSink::new_root);
        // Outer field "header" → sub-sink (Struct mode) with field "magic".
        let mut header_sub = sink.sub_sink_for_field("header").unwrap();
        // Force the sub-sink into Struct mode by calling set_field.
        header_sub.set_field("magic", Value::UInt(42)).unwrap();
        let header_val = sink.finish_field("header", header_sub).unwrap();
        assert!(header_val.is_container());
        // Verify nested structure.
        // Note: into_value round-trips through Python, which narrows
        // positive ints to Value::Int.
        let boxed: Box<dyn OutputSink> = Box::new(sink);
        let result = boxed.into_value().unwrap();
        let outer = result.as_container().unwrap();
        let header = outer.get("header").unwrap().as_container().unwrap();
        assert_eq!(header.get("magic").unwrap(), &Value::Int(42));
    }

    #[test]
    fn dict_sink_finish_field_on_array_returns_err() {
        crate::ensure_python();
        let mut sink = PyDictSink::new();
        sink.push_item(Value::Int(1)).unwrap();
        let mut sub = ValueSink::new();
        sub.set_scalar(Value::Int(2)).unwrap();
        let err = sink.finish_field("x", Box::new(sub)).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    // -- finish_named_item --------------------------------------------------

    #[test]
    fn dict_sink_finish_named_item_with_leaf_sub_appends_list() {
        crate::ensure_python();
        let mut sink = PyDictSink::new();
        sink.push_item(Value::Int(0)).unwrap(); // Force Array mode.
        let mut sub = sink.sub_sink_for_item().unwrap();
        sub.set_scalar(Value::Int(99)).unwrap();
        let value = sink.finish_named_item("entry", sub).unwrap();
        assert_eq!(value, Value::Int(99));
        let boxed: Box<dyn OutputSink> = Box::new(sink);
        let result = boxed.into_value().unwrap();
        let list = result.as_list().unwrap();
        assert_eq!(list, &vec![Value::Int(0), Value::Int(99)]);
    }

    // -- finish_subsink / finish_item ---------------------------------------

    #[test]
    fn dict_sink_finish_subsink_appends_anonymous_item() {
        crate::ensure_python();
        let mut sink = PyDictSink::new();
        sink.push_item(Value::Int(1)).unwrap();
        let mut sub = ValueSink::new();
        sub.set_scalar(Value::Int(2)).unwrap();
        sink.finish_subsink(Box::new(sub)).unwrap();
        let boxed: Box<dyn OutputSink> = Box::new(sink);
        let result = boxed.into_value().unwrap();
        let list = result.as_list().unwrap();
        assert_eq!(list, &vec![Value::Int(1), Value::Int(2)]);
    }

    #[test]
    fn dict_sink_finish_item_delegates_to_finish_subsink() {
        crate::ensure_python();
        let mut sink = PyDictSink::new();
        // Force Array mode.
        sink.push_item(Value::Int(10)).unwrap();
        let mut sub = ValueSink::new();
        sub.set_scalar(Value::Int(20)).unwrap();
        sink.finish_item(Box::new(sub)).unwrap();
        let boxed: Box<dyn OutputSink> = Box::new(sink);
        let result = boxed.into_value().unwrap();
        let list = result.as_list().unwrap();
        assert_eq!(list, &vec![Value::Int(10), Value::Int(20)]);
    }

    // -- into_py_object (inherent method) -----------------------------------

    #[test]
    fn dict_sink_into_py_object_struct_returns_dict() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let mut sink = PyDictSink::new_root(py);
            sink.set_field("key", Value::String("value".into()))
                .unwrap();
            let obj = sink.into_py_object().unwrap();
            assert!(obj.bind(py).is_instance_of::<PyDict>());
            // Verify via py_to_value.
            let value = crate::conversions::py_to_value(py, obj.bind(py)).unwrap();
            let container = value.as_container().unwrap();
            assert_eq!(
                container.get("key").unwrap(),
                &Value::String("value".into())
            );
        });
    }

    #[test]
    fn dict_sink_into_py_object_array_returns_list() {
        crate::ensure_python();
        let mut sink = PyDictSink::new();
        sink.push_item(Value::Int(1)).unwrap();
        sink.push_item(Value::Int(2)).unwrap();
        let obj = sink.into_py_object().unwrap();
        Python::with_gil(|py| {
            assert!(obj.bind(py).is_instance_of::<PyList>());
            let value = crate::conversions::py_to_value(py, obj.bind(py)).unwrap();
            let list = value.as_list().unwrap();
            assert_eq!(list, &vec![Value::Int(1), Value::Int(2)]);
        });
    }

    #[test]
    fn dict_sink_into_py_object_empty_returns_none() {
        crate::ensure_python();
        let sink = PyDictSink::new();
        let obj = sink.into_py_object().unwrap();
        Python::with_gil(|py| {
            assert!(obj.is_none(py));
        });
    }

    // -- new_root -----------------------------------------------------------

    #[test]
    fn dict_sink_new_root_creates_empty_struct_dict() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let sink = PyDictSink::new_root(py);
            let obj = sink.into_py_object().unwrap();
            assert!(obj.bind(py).is_instance_of::<PyDict>());
            let value = crate::conversions::py_to_value(py, obj.bind(py)).unwrap();
            assert!(value.is_container());
            assert_eq!(value.as_container().unwrap().len(), 0);
        });
    }

    // -- Default ------------------------------------------------------------

    #[test]
    fn dict_sink_default_is_empty() {
        let sink = PyDictSink::default();
        let boxed: Box<dyn OutputSink> = Box::new(sink);
        let produced = boxed.into_produced().unwrap();
        assert!(matches!(produced, ProducedOutput::Value(Value::None)));
    }
}
