//! Bidirectional conversion between Rust [`Value`] and Python objects.
//!
//! - [`value_to_py`] converts a [`Value`] reference into a Python `PyObject`.
//! - [`py_to_value`] converts a Python object reference into a [`Value`].
//! - [`context_to_py_container`] converts a [`Context`] into a Python dict
//!   (Container in 10.3+).
//!
//! See `docs/模块设计-PythonFFI-02-Value映射.md` for the full mapping table.

use indexmap::IndexMap;
use num_bigint::BigInt;
use num_traits::ToPrimitive;
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyByteArray, PyBytes, PyDict, PyFloat, PyList, PyString};

use construct::core::context::Context;
use construct::value::Value;

use crate::exceptions::{py_overflow_error, py_type_error};

/// Converts a Rust [`Value`] into a Python object.
///
/// # Mapping
///
/// | Value variant       | Python type |
/// |---------------------|-------------|
/// | `None`              | `None`      |
/// | `Bool(b)`           | `bool`      |
/// | `Int(i)`            | `int`       |
/// | `UInt(u)`           | `int`       |
/// | `BigInt(i)`         | `int`       |
/// | `Float(f)`          | `float`     |
/// | `Bytes(b)`          | `bytes`     |
/// | `String(s)`         | `str`       |
/// | `List(v)`           | `list`      |
/// | `Container(m)`      | `dict`      |
///
/// # Errors
///
/// Returns `PyErr` if the Python runtime rejects an operation (e.g. memory
/// allocation failure).
pub fn value_to_py(py: Python<'_>, v: &Value) -> PyResult<PyObject> {
    match v {
        Value::None => Ok(py.None()),
        Value::Bool(b) => Ok(b.into_py(py)),
        Value::Int(i) => Ok(i.into_py(py)),
        Value::UInt(u) => Ok(u.into_py(py)),
        Value::BigInt(i) => {
            // i128 is not natively supported by PyO3 IntoPy; go through
            // num_bigint::BigInt which has an IntoPy<PyObject> impl.
            let bigint = BigInt::from(*i);
            Ok(bigint.into_py(py))
        }
        Value::Float(f) => Ok(f.into_py(py)),
        Value::Bytes(b) => Ok(PyBytes::new_bound(py, b).into_any().unbind()),
        Value::String(s) => Ok(PyString::new_bound(py, s).into_any().unbind()),
        Value::List(list) => {
            // Use ListContainer from construct_rust.lib.containers for repr/eq
            // compatibility with the Python original. Falls back to plain list
            // if the pure-Python package is not importable (e.g. cargo test).
            let listcontainer = py
                .import_bound("construct_rust.lib.containers")
                .and_then(|m| m.getattr("ListContainer"));

            let py_obj: PyObject = match listcontainer {
                Ok(cls) => cls.call0()?.unbind(),
                Err(_) => PyList::empty_bound(py).into_any().unbind(),
            };
            let bound = py_obj.bind(py);
            for item in list {
                bound.call_method1("append", (value_to_py(py, item)?,))?;
            }
            Ok(py_obj)
        }
        Value::Container(map) => {
            // Use Container from construct_rust.lib.containers for attribute
            // access (.field) and search/search_all compatibility with the
            // Python original. Falls back to plain dict if the pure-Python
            // package is not importable (e.g. cargo test).
            let container = py
                .import_bound("construct_rust.lib.containers")
                .and_then(|m| m.getattr("Container"))
                .and_then(|cls| cls.call0());

            let py_obj: PyObject = match container {
                Ok(c) => c.unbind(),
                Err(_) => PyDict::new_bound(py).into_any().unbind(),
            };
            let bound = py_obj.bind(py);
            for (key, value) in map {
                bound.set_item(key, value_to_py(py, value)?)?;
            }
            Ok(py_obj)
        }
    }
}

/// Converts a Python object into a Rust [`Value`].
///
/// The type-dispatch order is critical because `bool` is a subclass of `int`
/// in Python. `bool` must be checked before `int`.
///
/// # Errors
///
/// - `TypeError` if the object cannot be converted (unsupported type or
///   non-string dict key).
/// - `OverflowError` if an integer exceeds the `i128` range.
#[allow(clippy::only_used_in_recursion)]
pub fn py_to_value(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<Value> {
    // 1. None
    if obj.is_none() {
        return Ok(Value::None);
    }

    // 2. Bool (must check before int — bool is subclass of int)
    if obj.is_instance_of::<PyBool>() {
        let b: bool = obj.extract()?;
        return Ok(Value::Bool(b));
    }

    // 3. Int (not bool — already returned above).  Try extracting as BigInt
    //    which accepts any Python int regardless of magnitude.
    if let Ok(bigint) = obj.extract::<BigInt>() {
        return bigint_to_value(&bigint);
    }

    // 4. Float
    if obj.is_instance_of::<PyFloat>() {
        let f: f64 = obj.extract()?;
        return Ok(Value::Float(f));
    }

    // 5. Bytes or bytearray
    if obj.is_instance_of::<PyBytes>() {
        let bytes_obj = obj.downcast::<PyBytes>()?;
        return Ok(Value::Bytes(bytes_obj.as_bytes().to_vec()));
    }
    if obj.is_instance_of::<PyByteArray>() {
        let bytes_obj = obj.downcast::<PyByteArray>()?;
        return Ok(Value::Bytes(bytes_obj.to_vec()));
    }

    // 6. String
    if obj.is_instance_of::<PyString>() {
        let s: &str = obj.downcast::<PyString>()?.to_str()?;
        return Ok(Value::String(s.to_string()));
    }

    // 7. List (or ListContainer from 10.3, which subclasses list)
    if obj.is_instance_of::<PyList>() {
        let list = obj.downcast::<PyList>()?;
        let mut result = Vec::with_capacity(list.len());
        for item in list.iter() {
            result.push(py_to_value(py, &item)?);
        }
        return Ok(Value::List(result));
    }

    // 8. Dict (or Container from 10.3, which subclasses dict)
    if obj.is_instance_of::<PyDict>() {
        let dict = obj.downcast::<PyDict>()?;
        let mut map = IndexMap::new();
        for (key, value) in dict.iter() {
            let key_str: String = key.extract().map_err(|_| {
                PyTypeError::new_err("dict key must be a string for Value::Container conversion")
            })?;
            let val = py_to_value(py, &value)?;
            map.insert(key_str, val);
        }
        return Ok(Value::Container(map));
    }

    // 9. Unknown type
    let type_name = obj
        .get_type()
        .name()
        .map(|n| n.to_string())
        .unwrap_or_else(|_| "<unknown>".to_string());
    Err(py_type_error(&format!(
        "cannot convert Python object of type {type_name} to Value"
    )))
}

/// Maps a `BigInt` to the narrowest integer [`Value`] variant, returning
/// `OverflowError` if the value exceeds `i128`.
fn bigint_to_value(bigint: &BigInt) -> PyResult<Value> {
    if let Some(v) = bigint.to_i64() {
        Ok(Value::Int(v))
    } else if let Some(v) = bigint.to_u64() {
        Ok(Value::UInt(v))
    } else if let Some(v) = bigint.to_i128() {
        Ok(Value::BigInt(v))
    } else {
        Err(py_overflow_error(
            "integer exceeds i128 range and cannot be represented in Value",
        ))
    }
}

/// Converts a [`Context`] into a Python dict (or `Container` if the pure-Python
/// package is importable).
///
/// Each field is recursively converted via [`value_to_py`]. If the context
/// has a parent, it is recursively converted and stored under the `"_"` key,
/// mirroring the Python `context._` convention.
///
/// In addition, two special keys are injected at the topmost call level (not
/// recursively) to match the Python construct context model:
///
/// - `"_root"` — a snapshot Container of the root struct's context (the context
///   whose parent is the entry/params context). Mirrors Python's
///   `context._root = context._.get("_root", context)`.
/// - `"_params"` — a snapshot Container of the entry (params) context (the
///   topmost context with no parent). Mirrors Python's `context._params`.
///
/// These are only injected at the entry point of conversion to avoid infinite
/// recursion (`_root._root` would otherwise point to itself).
///
/// # Container vs dict
///
/// The function tries to import `construct_rust.lib.containers.Container`
/// first. If the import succeeds (production environment after `maturin
/// develop`), a `Container` instance is returned — providing attribute access
/// (`ctx.field`) that pure-Python Adapter/Validator classes expect. If the
/// import fails (e.g., during `cargo test` where the pure-Python package is
/// not in `sys.path`), a plain `PyDict` is used instead. Dict indexing
/// (`ctx["field"]`) works identically in both cases, so the expression system
/// (`this.field` → `Path.__call__`) functions correctly with either type.
///
/// # Errors
///
/// Returns `PyErr` if any value conversion fails.
pub fn context_to_py_container(py: Python<'_>, ctx: &Context) -> PyResult<PyObject> {
    context_to_py_container_impl(py, ctx, true)
}

/// Internal implementation that accepts an `inject_special` flag.
///
/// When `true`, `_root` and `_params` are injected. Recursive calls for parent
/// levels pass `false` to avoid infinite recursion.
fn context_to_py_container_impl(
    py: Python<'_>,
    ctx: &Context,
    inject_special: bool,
) -> PyResult<PyObject> {
    // Attempt to use Container for attribute access compatibility.
    let container = py
        .import_bound("construct_rust.lib.containers")
        .and_then(|m| m.getattr("Container"))
        .and_then(|cls| cls.call0());

    let py_obj: PyObject = match container {
        Ok(c) => c.unbind(),
        Err(_) => PyDict::new_bound(py).into_any().unbind(),
    };

    let bound = py_obj.bind(py);
    for (key, value) in ctx.iter_fields() {
        bound.set_item(key, value_to_py(py, value)?)?;
    }

    // Recursively convert parent context under the "_" key.
    if let Some(parent) = ctx.parent() {
        let parent_container = context_to_py_container_impl(py, parent, false)?;
        bound.set_item("_", parent_container)?;
    }

    // Inject `_root` and `_params` only at the entry point of conversion.
    // This matches Python's Struct._parse which sets:
    //   context._root = context._.get("_root", context)
    //   context._params = context._params   (inherited from entry)
    if inject_special {
        // `_params` points to the topmost context (entry/params context).
        let params_ctx = find_topmost(ctx);
        let params_container = context_to_py_container_impl(py, params_ctx, false)?;
        bound.set_item("_params", params_container)?;

        // `_root` points to the root struct's context: the context whose
        // parent is the entry (params) context. If no struct wraps the data
        // (ctx is the entry context itself), skip `_root`.
        if let Some(root_ctx) = find_root(ctx) {
            let root_container = context_to_py_container_impl(py, root_ctx, false)?;
            bound.set_item("_root", root_container)?;
        }
    }

    Ok(py_obj)
}

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
            // `current`'s parent is the entry context — `current` is the root.
            return Some(current);
        }
        current = parent;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    // -- value_to_py: scalars ------------------------------------------------

    #[test]
    fn value_to_py_none() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let result = value_to_py(py, &Value::None).unwrap();
            assert!(result.is_none(py));
        });
    }

    #[test]
    fn value_to_py_bool() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let result = value_to_py(py, &Value::Bool(true)).unwrap();
            assert!(result.bind(py).is_instance_of::<PyBool>());
            let b: bool = result.extract(py).unwrap();
            assert!(b);
        });
    }

    #[test]
    fn value_to_py_int() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let result = value_to_py(py, &Value::Int(-42)).unwrap();
            let i: i64 = result.extract(py).unwrap();
            assert_eq!(i, -42);
        });
    }

    #[test]
    fn value_to_py_uint() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let result = value_to_py(py, &Value::UInt(42)).unwrap();
            let u: u64 = result.extract(py).unwrap();
            assert_eq!(u, 42);
        });
    }

    #[test]
    fn value_to_py_bigint() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let val = i128::MAX;
            let result = value_to_py(py, &Value::BigInt(val)).unwrap();
            let extracted: BigInt = result.extract(py).unwrap();
            assert_eq!(extracted, BigInt::from(val));
        });
    }

    #[test]
    fn value_to_py_bigint_negative() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let val = i128::MIN;
            let result = value_to_py(py, &Value::BigInt(val)).unwrap();
            let extracted: BigInt = result.extract(py).unwrap();
            assert_eq!(extracted, BigInt::from(val));
        });
    }

    #[test]
    fn value_to_py_float() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let result = value_to_py(py, &Value::Float(1.5)).unwrap();
            let f: f64 = result.extract(py).unwrap();
            assert!((f - 1.5).abs() < 1e-10);
        });
    }

    #[test]
    fn value_to_py_bytes() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let result = value_to_py(py, &Value::Bytes(vec![1, 2, 3])).unwrap();
            let b: Vec<u8> = result.extract(py).unwrap();
            assert_eq!(b, vec![1, 2, 3]);
        });
    }

    #[test]
    fn value_to_py_empty_bytes() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let result = value_to_py(py, &Value::Bytes(vec![])).unwrap();
            let b: Vec<u8> = result.extract(py).unwrap();
            assert!(b.is_empty());
        });
    }

    #[test]
    fn value_to_py_string() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let result = value_to_py(py, &Value::String("hello".to_string())).unwrap();
            let s: String = result.extract(py).unwrap();
            assert_eq!(s, "hello");
        });
    }

    #[test]
    fn value_to_py_empty_string() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let result = value_to_py(py, &Value::String(String::new())).unwrap();
            let s: String = result.extract(py).unwrap();
            assert!(s.is_empty());
        });
    }

    // -- value_to_py: containers --------------------------------------------

    #[test]
    fn value_to_py_list() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let list = Value::List(vec![Value::Int(1), Value::String("a".to_string())]);
            let result = value_to_py(py, &list).unwrap();
            assert!(result.bind(py).is_instance_of::<PyList>());
        });
    }

    #[test]
    fn value_to_py_container_preserves_order() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let mut map = IndexMap::new();
            map.insert("first".to_string(), Value::Int(1));
            map.insert("second".to_string(), Value::Int(2));
            map.insert("third".to_string(), Value::Int(3));
            let result = value_to_py(py, &Value::Container(map)).unwrap();
            let dict = result.downcast_bound::<PyDict>(py).unwrap();
            let keys: Vec<String> = dict
                .keys()
                .iter()
                .map(|k| k.extract::<String>().unwrap())
                .collect();
            assert_eq!(keys, vec!["first", "second", "third"]);
        });
    }

    // -- py_to_value: scalars ------------------------------------------------

    #[test]
    fn py_to_value_none() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let obj = py.None();
            let bound = obj.into_bound(py);
            let result = py_to_value(py, &bound).unwrap();
            assert_eq!(result, Value::None);
        });
    }

    #[test]
    fn py_to_value_bool_true() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let obj = true.to_object(py);
            let bound = obj.into_bound(py);
            let result = py_to_value(py, &bound).unwrap();
            assert_eq!(result, Value::Bool(true));
        });
    }

    #[test]
    fn py_to_value_bool_false() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let obj = false.to_object(py);
            let bound = obj.into_bound(py);
            let result = py_to_value(py, &bound).unwrap();
            assert_eq!(result, Value::Bool(false));
        });
    }

    #[test]
    fn py_to_value_int_positive() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let obj = 42.to_object(py);
            let bound = obj.into_bound(py);
            let result = py_to_value(py, &bound).unwrap();
            assert_eq!(result, Value::Int(42));
        });
    }

    #[test]
    fn py_to_value_int_negative() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let obj = (-99i64).to_object(py);
            let bound = obj.into_bound(py);
            let result = py_to_value(py, &bound).unwrap();
            assert_eq!(result, Value::Int(-99));
        });
    }

    #[test]
    fn py_to_value_int_large_u64() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let val = u64::MAX;
            let obj = val.to_object(py);
            let bound = obj.into_bound(py);
            let result = py_to_value(py, &bound).unwrap();
            assert_eq!(result, Value::UInt(val));
        });
    }

    #[test]
    fn py_to_value_int_bigint() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let val = i128::MAX;
            let bigint = BigInt::from(val);
            let obj = bigint.into_py(py);
            let bound = obj.into_bound(py);
            let result = py_to_value(py, &bound).unwrap();
            assert_eq!(result, Value::BigInt(val));
        });
    }

    #[test]
    fn py_to_value_int_negative_bigint() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let val = i128::MIN;
            let bigint = BigInt::from(val);
            let obj = bigint.into_py(py);
            let bound = obj.into_bound(py);
            let result = py_to_value(py, &bound).unwrap();
            assert_eq!(result, Value::BigInt(val));
        });
    }

    #[test]
    fn py_to_value_int_overflow() {
        crate::ensure_python();
        Python::with_gil(|py| {
            // 2^130 — exceeds i128 range
            let huge: BigInt = BigInt::from(1) << 130;
            let obj = huge.into_py(py);
            let bound = obj.into_bound(py);
            let err = py_to_value(py, &bound).unwrap_err();
            // Should be an OverflowError
            assert!(err.is_instance_of::<pyo3::exceptions::PyOverflowError>(py));
        });
    }

    #[test]
    fn py_to_value_float() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let obj = 1.5f64.to_object(py);
            let bound = obj.into_bound(py);
            let result = py_to_value(py, &bound).unwrap();
            match result {
                Value::Float(f) => assert!((f - 1.5).abs() < 1e-10),
                other => panic!("expected Float, got {other:?}"),
            }
        });
    }

    #[test]
    fn py_to_value_bytes() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let obj = PyBytes::new_bound(py, &[1, 2, 3]).into_any().unbind();
            let bound = obj.into_bound(py);
            let result = py_to_value(py, &bound).unwrap();
            assert_eq!(result, Value::Bytes(vec![1, 2, 3]));
        });
    }

    #[test]
    fn py_to_value_string() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let obj = "hello".to_object(py);
            let bound = obj.into_bound(py);
            let result = py_to_value(py, &bound).unwrap();
            assert_eq!(result, Value::String("hello".to_string()));
        });
    }

    // -- py_to_value: bool checked before int --------------------------------

    #[test]
    fn py_to_value_bool_not_int() {
        crate::ensure_python();
        Python::with_gil(|py| {
            // True must be Value::Bool, not Value::Int(1)
            let obj = true.to_object(py);
            let bound = obj.into_bound(py);
            let result = py_to_value(py, &bound).unwrap();
            assert_eq!(result, Value::Bool(true));
            assert_ne!(result, Value::Int(1));
        });
    }

    // -- py_to_value: containers ---------------------------------------------

    #[test]
    fn py_to_value_list() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let list = PyList::new_bound(py, [1i32, 2, 3]);
            let result = py_to_value(py, list.as_any()).unwrap();
            match result {
                Value::List(items) => {
                    assert_eq!(items.len(), 3);
                    assert_eq!(items[0], Value::Int(1));
                    assert_eq!(items[1], Value::Int(2));
                    assert_eq!(items[2], Value::Int(3));
                }
                other => panic!("expected List, got {other:?}"),
            }
        });
    }

    #[test]
    fn py_to_value_dict() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let dict = PyDict::new_bound(py);
            dict.set_item("a", 1i32).unwrap();
            dict.set_item("b", "hello").unwrap();
            let result = py_to_value(py, dict.as_any()).unwrap();
            match result {
                Value::Container(map) => {
                    assert_eq!(map.len(), 2);
                    assert_eq!(map.get("a").unwrap(), &Value::Int(1));
                    assert_eq!(map.get("b").unwrap(), &Value::String("hello".to_string()));
                }
                other => panic!("expected Container, got {other:?}"),
            }
        });
    }

    #[test]
    fn py_to_value_dict_non_str_key_errors() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let dict = PyDict::new_bound(py);
            dict.set_item(42i32, "value").unwrap();
            let err = py_to_value(py, dict.as_any()).unwrap_err();
            assert!(err.is_instance_of::<PyTypeError>(py));
        });
    }

    // -- py_to_value: unsupported type ---------------------------------------

    #[test]
    fn py_to_value_unsupported_type() {
        crate::ensure_python();
        Python::with_gil(|py| {
            // tuple is not supported
            let tuple_obj: PyObject = (1i32, 2i32).into_py(py);
            let bound = tuple_obj.bind(py);
            let err = py_to_value(py, bound).unwrap_err();
            assert!(err.is_instance_of::<PyTypeError>(py));
        });
    }

    // -- round-trip ----------------------------------------------------------

    #[test]
    fn roundtrip_int() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let original = Value::Int(-12345);
            let py_obj = value_to_py(py, &original).unwrap();
            let bound = py_obj.into_bound(py);
            let back = py_to_value(py, &bound).unwrap();
            assert_eq!(back, original);
        });
    }

    #[test]
    fn roundtrip_uint() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let original = Value::UInt(u64::MAX);
            let py_obj = value_to_py(py, &original).unwrap();
            let bound = py_obj.into_bound(py);
            let back = py_to_value(py, &bound).unwrap();
            assert_eq!(back, original);
        });
    }

    #[test]
    fn roundtrip_bigint() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let original = Value::BigInt(i128::MAX);
            let py_obj = value_to_py(py, &original).unwrap();
            let bound = py_obj.into_bound(py);
            let back = py_to_value(py, &bound).unwrap();
            assert_eq!(back, original);
        });
    }

    #[test]
    fn roundtrip_bytes() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let original = Value::Bytes(vec![0xDE, 0xAD, 0xBE, 0xEF]);
            let py_obj = value_to_py(py, &original).unwrap();
            let bound = py_obj.into_bound(py);
            let back = py_to_value(py, &bound).unwrap();
            assert_eq!(back, original);
        });
    }

    #[test]
    fn roundtrip_string() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let original = Value::String("hello world".to_string());
            let py_obj = value_to_py(py, &original).unwrap();
            let bound = py_obj.into_bound(py);
            let back = py_to_value(py, &bound).unwrap();
            assert_eq!(back, original);
        });
    }

    #[test]
    fn roundtrip_list_nested() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let original = Value::List(vec![
                Value::Int(1),
                Value::String("a".to_string()),
                Value::Bytes(vec![0xFF]),
            ]);
            let py_obj = value_to_py(py, &original).unwrap();
            let bound = py_obj.into_bound(py);
            let back = py_to_value(py, &bound).unwrap();
            assert_eq!(back, original);
        });
    }

    #[test]
    fn roundtrip_container_nested() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let mut inner = IndexMap::new();
            inner.insert("x".to_string(), Value::Int(42));
            let mut outer = IndexMap::new();
            outer.insert("inner".to_string(), Value::Container(inner));
            outer.insert("flag".to_string(), Value::Bool(true));
            let original = Value::Container(outer);

            let py_obj = value_to_py(py, &original).unwrap();
            let bound = py_obj.into_bound(py);
            let back = py_to_value(py, &bound).unwrap();
            assert_eq!(back, original);
        });
    }
}
