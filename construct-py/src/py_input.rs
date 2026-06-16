//! Python input source for build-direction lazy field reading.
//!
//! [`PyInput`] implements the [`Input`] trait by wrapping an owned
//! `Py<PyAny>` reference. Each `get_field` / `get_index` call is a single
//! FFI crossing — only the requested field is converted to a Rust [`Value`],
//! not the entire object tree.
//!
//! # GIL handling
//!
//! Each method acquires the GIL via `Python::with_gil`. Since `PyInput` is
//! only used in the Python path (`build_from_py`), the GIL is already held
//! by the caller — `with_gil` is essentially a no-op in that case.

use construct::compiled::input::Input;
use construct::core::error::{ConstructError, Result};
use construct::value::Value;

use indexmap::IndexMap;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::conversions::py_to_value;
use crate::py_sink::py_err_to_construct;

/// [`Input`] implementation that lazily reads fields from a Python `dict` /
/// dataclass / `list`.
///
/// Each `get_field` call is one FFI crossing (Python → Rust [`Value`]), but
/// only for the requested field — no pre-conversion of the entire tree.
///
/// # Dict vs attribute access
///
/// `get_field` tries dict-style `__getitem__` first, then falls back to
/// `getattr` (for dataclasses, named tuples, namespaces, etc.).
pub struct PyInput {
    /// Owned Python object reference (GIL-independent).
    obj: Py<PyAny>,
}

impl PyInput {
    /// Creates a new `PyInput` wrapping an owned [`Py<PyAny>`].
    #[must_use]
    pub fn new(obj: Py<PyAny>) -> Self {
        Self { obj }
    }

    /// Creates a new `PyInput` from a bound reference.
    ///
    /// Clones the Python reference (increments refcount) and detaches it
    /// from the borrow, making the `PyInput` `'static` and boxable into
    /// `Box<dyn Input>`.
    #[must_use]
    pub fn from_bound(obj: &Bound<'_, PyAny>) -> Self {
        Self {
            obj: obj.clone().unbind(),
        }
    }

    /// Internal helper: retrieves a named field as a bound Python object.
    ///
    /// Tries `__getitem__` first (dict), then `getattr` (dataclass/attr).
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

impl Input for PyInput {
    fn as_value(&self) -> Result<Value> {
        Python::with_gil(|py| {
            let bound = self.obj.bind(py);

            // Standard conversion handles: None, bool, int, float, bytes,
            // string, list, and dict (including Container subclasses).
            match py_to_value(py, bound) {
                Ok(v) => Ok(v),
                Err(_) => {
                    // Fallback: the object is not a primitive or dict/list.
                    // Try converting it as a struct-like object by reading
                    // its __dict__ (dataclass instances, plain objects, etc.).
                    if let Ok(dict_attr) = bound.getattr("__dict__") {
                        if let Ok(dict) = dict_attr.downcast::<PyDict>() {
                            let mut map = IndexMap::new();
                            for (key, value) in dict.iter() {
                                if let Ok(key_str) = key.extract::<String>() {
                                    if let Ok(val) = py_to_value(py, &value) {
                                        map.insert(key_str, val);
                                    }
                                }
                            }
                            return Ok(Value::Container(map));
                        }
                    }
                    // Cannot convert — return a type-mismatch error.
                    let type_name = bound
                        .get_type()
                        .name()
                        .map(|n| n.to_string())
                        .unwrap_or_else(|_| "<unknown>".to_string());
                    Err(ConstructError::TypeMismatch {
                        path: String::new(),
                        expected: "convertible Value or object with __dict__".to_string(),
                        actual: type_name,
                    })
                }
            }
        })
    }

    fn get_field(&self, name: &str) -> Result<Value> {
        Python::with_gil(|py| {
            let item = self.get_field_bound(py, name)?;
            py_to_value(py, &item).map_err(py_err_to_construct)
        })
    }

    fn has_field(&self, name: &str) -> bool {
        Python::with_gil(|py| {
            // Try dict-style access first; fall back to attribute.
            if let Ok(item) = self.get_field_bound(py, name) {
                return !item.is_none();
            }
            false
        })
    }

    fn get_index(&self, index: usize) -> Result<Value> {
        Python::with_gil(|py| {
            let bound = self.obj.bind(py);
            let length = bound.len().unwrap_or(0);
            let item = bound.get_item(index).map_err(|_| ConstructError::Index {
                path: String::new(),
                index,
                length,
            })?;
            py_to_value(py, &item).map_err(py_err_to_construct)
        })
    }

    fn len(&self) -> usize {
        Python::with_gil(|py| self.obj.bind(py).len().unwrap_or(1))
    }

    fn sub_input_field(&self, name: &str) -> Result<Box<dyn Input>> {
        Python::with_gil(|py| -> Result<Box<dyn Input>> {
            let item = self.get_field_bound(py, name)?;
            Ok(Box::new(PyInput::new(item.unbind())))
        })
    }

    fn sub_input_index(&self, index: usize) -> Result<Box<dyn Input>> {
        Python::with_gil(|py| -> Result<Box<dyn Input>> {
            let bound = self.obj.bind(py);
            let length = bound.len().unwrap_or(0);
            let item = bound.get_item(index).map_err(|_| ConstructError::Index {
                path: String::new(),
                index,
                length,
            })?;
            Ok(Box::new(PyInput::new(item.unbind())))
        })
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use construct::compiled::input::Input;
    use pyo3::types::{PyDict, PyList};

    // -- as_value ------------------------------------------------------------

    #[test]
    fn py_input_as_value_int() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let obj = 42i64.to_object(py);
            let input = PyInput::new(obj);
            assert_eq!(input.as_value().unwrap(), Value::Int(42));
        });
    }

    #[test]
    fn py_input_as_value_string() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let obj = "hello".to_object(py);
            let input = PyInput::new(obj);
            assert_eq!(input.as_value().unwrap(), Value::String("hello".into()));
        });
    }

    #[test]
    fn py_input_as_value_none() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let input = PyInput::new(py.None());
            assert_eq!(input.as_value().unwrap(), Value::None);
        });
    }

    #[test]
    fn py_input_as_value_object_with_dict() {
        // Non-primitive objects with __dict__ (e.g., dataclass instances,
        // plain Python objects) should be converted to Value::Container
        // by reading their __dict__ attributes.
        crate::ensure_python();
        Python::with_gil(|py| {
            // Create a simple Python object with attributes.
            py.run_bound("class _TestObj:\n    pass\n", None, None)
                .unwrap();
            let obj = py.eval_bound("_TestObj()", None, None).unwrap();
            obj.setattr("x", 42i64).unwrap();
            obj.setattr("y", "hello").unwrap();
            let input = PyInput::from_bound(&obj);
            let val = input.as_value().unwrap();
            match val {
                Value::Container(map) => {
                    assert_eq!(map.get("x"), Some(&Value::Int(42)));
                    assert_eq!(map.get("y"), Some(&Value::String("hello".into())));
                }
                other => panic!("expected Container, got {:?}", other),
            }
        });
    }

    #[test]
    fn py_input_as_value_unconvertible_returns_error() {
        // Objects without __dict__ and not convertible to a standard Value
        // should return a TypeMismatch error.
        crate::ensure_python();
        Python::with_gil(|py| {
            // Create a class with __slots__ (no __dict__).
            py.run_bound("class _NoDict:\n    __slots__ = ()\n", None, None)
                .unwrap();
            let obj = py.eval_bound("_NoDict()", None, None).unwrap();
            let input = PyInput::from_bound(&obj);
            let err = input.as_value().unwrap_err();
            assert!(matches!(err, ConstructError::TypeMismatch { .. }));
        });
    }

    // -- get_field (dict) ----------------------------------------------------

    #[test]
    fn py_input_get_field_from_dict() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let dict = PyDict::new_bound(py);
            dict.set_item("a", 1i64).unwrap();
            dict.set_item("b", "text").unwrap();
            let input = PyInput::from_bound(dict.as_any());
            assert_eq!(input.get_field("a").unwrap(), Value::Int(1));
            assert_eq!(input.get_field("b").unwrap(), Value::String("text".into()));
        });
    }

    #[test]
    fn py_input_get_field_missing_returns_error() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let dict = PyDict::new_bound(py);
            let input = PyInput::from_bound(dict.as_any());
            let err = input.get_field("missing").unwrap_err();
            assert!(matches!(err, ConstructError::FieldMissing { .. }));
        });
    }

    // -- has_field -----------------------------------------------------------

    #[test]
    fn py_input_has_field_existing_true() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let dict = PyDict::new_bound(py);
            dict.set_item("x", 42i64).unwrap();
            let input = PyInput::from_bound(dict.as_any());
            assert!(input.has_field("x"));
        });
    }

    #[test]
    fn py_input_has_field_missing_false() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let dict = PyDict::new_bound(py);
            let input = PyInput::from_bound(dict.as_any());
            assert!(!input.has_field("missing"));
        });
    }

    #[test]
    fn py_input_has_field_none_value_false() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let dict = PyDict::new_bound(py);
            dict.set_item("x", py.None()).unwrap();
            let input = PyInput::from_bound(dict.as_any());
            // None values are treated as "not present" (matching flagbuildnone).
            assert!(!input.has_field("x"));
        });
    }

    // -- get_index -----------------------------------------------------------

    #[test]
    fn py_input_get_index_from_list() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let list = PyList::new_bound(py, [10i64, 20, 30]);
            let input = PyInput::from_bound(list.as_any());
            assert_eq!(input.get_index(0).unwrap(), Value::Int(10));
            assert_eq!(input.get_index(1).unwrap(), Value::Int(20));
            assert_eq!(input.get_index(2).unwrap(), Value::Int(30));
        });
    }

    #[test]
    fn py_input_get_index_out_of_bounds_returns_error() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let list = PyList::new_bound(py, [1i64]);
            let input = PyInput::from_bound(list.as_any());
            let err = input.get_index(5).unwrap_err();
            assert!(matches!(err, ConstructError::Index { .. }));
        });
    }

    // -- len -----------------------------------------------------------------

    #[test]
    fn py_input_len_list() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let list = PyList::new_bound(py, [1i64, 2, 3]);
            let input = PyInput::from_bound(list.as_any());
            assert_eq!(input.len(), 3);
        });
    }

    #[test]
    fn py_input_len_dict() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let dict = PyDict::new_bound(py);
            dict.set_item("a", 1i64).unwrap();
            dict.set_item("b", 2i64).unwrap();
            let input = PyInput::from_bound(dict.as_any());
            assert_eq!(input.len(), 2);
        });
    }

    #[test]
    fn py_input_len_scalar_is_one() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let input = PyInput::new(42i64.to_object(py));
            assert_eq!(input.len(), 1);
        });
    }

    // -- sub_input_field -----------------------------------------------------

    #[test]
    fn py_input_sub_input_field_returns_nested_input() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let inner = PyDict::new_bound(py);
            inner.set_item("deep", 99i64).unwrap();
            let outer = PyDict::new_bound(py);
            outer.set_item("inner", inner).unwrap();
            let input = PyInput::from_bound(outer.as_any());
            let sub = input.sub_input_field("inner").unwrap();
            assert_eq!(sub.get_field("deep").unwrap(), Value::Int(99));
        });
    }

    // -- sub_input_index -----------------------------------------------------

    #[test]
    fn py_input_sub_input_index_returns_item_input() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let list = PyList::new_bound(py, [100i64, 200]);
            let input = PyInput::from_bound(list.as_any());
            let sub = input.sub_input_index(1).unwrap();
            assert_eq!(sub.as_value().unwrap(), Value::Int(200));
        });
    }

    // -- attr access (dataclass fallback) ------------------------------------

    #[test]
    fn py_input_get_field_via_attribute() {
        crate::ensure_python();
        Python::with_gil(|py| {
            // Create a simple namespace-like object via type().
            let code = "type('O', (), {'x': 42})()";
            let obj = py.eval_bound(code, None, None).unwrap();
            let input = PyInput::from_bound(&obj);
            assert_eq!(input.get_field("x").unwrap(), Value::Int(42));
        });
    }

    // -- from_bound ----------------------------------------------------------

    #[test]
    fn py_input_from_bound_works() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let obj = 42i64.to_object(py);
            let bound = obj.bind(py);
            let input = PyInput::from_bound(bound);
            assert_eq!(input.as_value().unwrap(), Value::Int(42));
        });
    }
}
