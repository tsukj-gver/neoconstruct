//! Expression system Python ↔ Rust bridge.
//!
//! This module provides:
//!
//! - [`PyClosure`] — wraps any Python callable (Path, BinExpr, FuncPath,
//!   lambda) so it can be evaluated by the Rust [`Evaluate`] trait.
//! - Five function-type adapters (`py_to_cond_func`, `py_to_key_func`, etc.)
//!   that bridge Python callables to the corresponding Rust closure type
//!   aliases used by control-flow and adapter constructs.
//! - Parameter helpers (`py_param_to_evaluate`, `py_param_to_cond_func`,
//!   `py_param_to_compute_func`) that accept either a callable expression
//!   or a plain value, producing the appropriate Rust type.

use construct::core::context::Context;
use construct::core::error::{ConstructError, Result};
use construct::expr::{ConstExpr, Evaluate};
use construct::value::Value;

use pyo3::prelude::*;

use crate::conversions::{context_to_py_container, py_to_value, value_to_py};

// ===========================================================================
// PyClosure
// ===========================================================================

/// Wraps an arbitrary Python callable so it can be evaluated as a Rust
/// [`Evaluate`] expression.
///
/// Python expression objects (`Path`, `BinExpr`, `FuncPath`, lambdas) are all
/// callable via the `__call__` protocol. `PyClosure` holds a strong reference
/// to such an object and, when evaluated, acquires the GIL, converts the Rust
/// [`Context`] to a Python dict, calls the callable, and converts the return
/// value back to a Rust [`Value`].
///
/// The single-argument calling convention `callable(context)` is used for all
/// expression types except `RepeatPredicate`, which uses the three-argument
/// convention `callable(element, list, context)`.
#[derive(Debug)]
pub struct PyClosure {
    /// The Python callable object (Path, BinExpr, FuncPath, lambda, etc.).
    callable: PyObject,
}

impl PyClosure {
    /// Creates a new [`PyClosure`] wrapping the given Python callable.
    #[must_use]
    pub fn new(callable: PyObject) -> Self {
        PyClosure { callable }
    }

    /// Single-argument call: `callable(context)`.
    ///
    /// Used by `Evaluate`, `CondFunc`, `KeyFunc`, `CheckFunc`, `ComputeFunc`.
    fn call_single(&self, py: Python<'_>, py_ctx: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        self.callable.call1(py, (py_ctx,))
    }
}

impl Evaluate for PyClosure {
    fn evaluate(&self, ctx: &Context, _obj: Option<&Value>) -> Result<Value> {
        Python::with_gil(|py| {
            // 1. Context → Python dict
            let py_ctx = context_to_py_container(py, ctx).map_err(|e| ConstructError::Expr {
                path: String::new(),
                message: format!("Context conversion failed: {e}"),
            })?;

            // 2. Call Python callable (single-arg convention)
            let result =
                self.call_single(py, py_ctx.bind(py))
                    .map_err(|e| ConstructError::Expr {
                        path: String::new(),
                        message: format!("Python expression error: {e}"),
                    })?;

            // 3. Return value → Value
            py_to_value(py, result.bind(py)).map_err(|e| ConstructError::Expr {
                path: String::new(),
                message: format!("Expression result conversion failed: {e}"),
            })
        })
    }
}

// ===========================================================================
// Function-type adapters
// ===========================================================================

/// Converts a [`ConstructError`] into an `Expr` variant for use inside
/// closure bridges.
fn err_to_expr(prefix: &str, e: impl std::fmt::Display) -> ConstructError {
    ConstructError::Expr {
        path: String::new(),
        message: format!("{prefix}: {e}"),
    }
}

/// Adapts a Python callable into a `CondFunc` (`Fn(&Context) -> bool`).
///
/// The callable is invoked as `callable(context)`. The return value is
/// interpreted using Python truthiness. On conversion errors, `false` is
/// returned (fail-safe).
///
/// Used by `IfThenElse` and `StopIf`.
#[allow(clippy::type_complexity)]
pub fn py_to_cond_func(callable: PyObject) -> Box<dyn Fn(&Context) -> bool> {
    Box::new(move |ctx| {
        Python::with_gil(|py| {
            let py_ctx = match context_to_py_container(py, ctx) {
                Ok(c) => c,
                Err(_) => return false,
            };
            match callable.call1(py, (py_ctx.bind(py),)) {
                Ok(result) => result.is_truthy(py).unwrap_or(false),
                Err(_) => false,
            }
        })
    })
}

/// Adapts a Python callable into a `KeyFunc` (`Fn(&Context) -> Result<Value>`).
///
/// The callable is invoked as `callable(context)`. The return value is
/// converted to a Rust [`Value`].
///
/// Used by `Switch`.
#[allow(clippy::type_complexity)]
pub fn py_to_key_func(callable: PyObject) -> Box<dyn Fn(&Context) -> Result<Value>> {
    Box::new(move |ctx| {
        Python::with_gil(|py| {
            let py_ctx = context_to_py_container(py, ctx)
                .map_err(|e| err_to_expr("Context conversion", e))?;
            let result = callable
                .call1(py, (py_ctx.bind(py),))
                .map_err(|e| err_to_expr("Switch keyfunc error", e))?;
            py_to_value(py, result.bind(py)).map_err(|e| err_to_expr("Key conversion", e))
        })
    })
}

/// Adapts a Python callable into a `CheckFunc` (`Fn(&Context) -> Result<()>`).
///
/// The callable is invoked as `callable(context)`. A truthy return value means
/// the check passed (`Ok(())`); a falsy value means the check failed
/// (`Err(CheckError)`).
///
/// Used by `Check`.
#[allow(clippy::type_complexity)]
pub fn py_to_check_func(callable: PyObject) -> Box<dyn Fn(&Context) -> Result<()>> {
    Box::new(move |ctx| {
        Python::with_gil(|py| {
            let py_ctx = context_to_py_container(py, ctx)
                .map_err(|e| err_to_expr("Context conversion", e))?;
            let result = callable
                .call1(py, (py_ctx.bind(py),))
                .map_err(|e| err_to_expr("Check func error", e))?;
            if result.is_truthy(py).unwrap_or(false) {
                Ok(())
            } else {
                Err(ConstructError::Check {
                    path: String::new(),
                    message: "Check failed".to_string(),
                })
            }
        })
    })
}

/// Adapts a Python callable into a `ComputeFunc`
/// (`Fn(&Context) -> Result<Value>`).
///
/// Functionally identical to [`py_to_key_func`]; provided as a separate
/// function for semantic clarity.
///
/// Used by `Computed` and `Rebuild`.
#[allow(clippy::type_complexity)]
pub fn py_to_compute_func(callable: PyObject) -> Box<dyn Fn(&Context) -> Result<Value>> {
    py_to_key_func(callable)
}

/// Adapts a Python callable into a `RepeatPredicate`
/// (`Fn(&Value, &[Value], &Context) -> bool`).
///
/// Uses the three-argument calling convention:
/// `callable(element, list, context)`. On conversion errors, `false` is
/// returned (fail-safe, causing the loop to continue).
///
/// Used by `RepeatUntil`.
#[allow(clippy::type_complexity)]
pub fn py_to_repeat_predicate(
    callable: PyObject,
) -> Box<dyn Fn(&Value, &[Value], &Context) -> bool> {
    Box::new(move |element, list, ctx| {
        Python::with_gil(|py| {
            let py_element = match value_to_py(py, element) {
                Ok(v) => v,
                Err(_) => return false,
            };
            let py_list = match value_to_py(py, &Value::List(list.to_vec())) {
                Ok(v) => v,
                Err(_) => return false,
            };
            let py_ctx = match context_to_py_container(py, ctx) {
                Ok(c) => c,
                Err(_) => return false,
            };
            let result = match callable.call1(py, (py_element, py_list, py_ctx)) {
                Ok(r) => r,
                Err(_) => return false,
            };
            result.is_truthy(py).unwrap_or(false)
        })
    })
}

// ===========================================================================
// Dual-parameter function-type adapters (obj, context)
// ===========================================================================

/// Converts a [`ConstructError`] into a `Generic` variant for use inside
/// dual-parameter closure bridges.
fn err_to_generic(prefix: &str, e: impl std::fmt::Display) -> ConstructError {
    ConstructError::Generic {
        path: String::new(),
        message: format!("{prefix}: {e}"),
    }
}

/// Adapts a Python callable into a decode function
/// (`Fn(&Value, &Context) -> Result<Value>`).
///
/// Uses the two-argument calling convention: `callable(obj, context)`.
/// Used by `ExprAdapter` and `Adapter`.
#[allow(clippy::type_complexity)]
pub fn py_to_decode_func(callable: PyObject) -> Box<dyn Fn(&Value, &Context) -> Result<Value>> {
    Box::new(move |obj, ctx| {
        Python::with_gil(|py| {
            let py_obj = value_to_py(py, obj).map_err(|e| err_to_generic("Decoder value", e))?;
            let py_ctx = context_to_py_container(py, ctx)
                .map_err(|e| err_to_generic("Decoder context", e))?;
            let result = callable
                .call1(py, (py_obj.bind(py), py_ctx.bind(py)))
                .map_err(|e| err_to_generic("Adapter decoder error", e))?;
            py_to_value(py, result.bind(py))
                .map_err(|e| err_to_generic("Decoder result conversion", e))
        })
    })
}

/// Adapts a Python callable into an encode function
/// (`Fn(&Value, &Context) -> Result<Value>`).
///
/// Uses the two-argument calling convention: `callable(obj, context)`.
/// Used by `ExprAdapter` and `Adapter`.
#[allow(clippy::type_complexity)]
pub fn py_to_encode_func(callable: PyObject) -> Box<dyn Fn(&Value, &Context) -> Result<Value>> {
    Box::new(move |obj, ctx| {
        Python::with_gil(|py| {
            let py_obj = value_to_py(py, obj).map_err(|e| err_to_generic("Encoder value", e))?;
            let py_ctx = context_to_py_container(py, ctx)
                .map_err(|e| err_to_generic("Encoder context", e))?;
            let result = callable
                .call1(py, (py_obj.bind(py), py_ctx.bind(py)))
                .map_err(|e| err_to_generic("Adapter encoder error", e))?;
            py_to_value(py, result.bind(py))
                .map_err(|e| err_to_generic("Encoder result conversion", e))
        })
    })
}

/// Adapts a Python callable into a symmetric function
/// (`Fn(&Value, &Context) -> Result<Value>`).
///
/// Uses the two-argument calling convention: `callable(obj, context)`.
/// Used by `SymmetricAdapter` and `ExprSymmetricAdapter`.
#[allow(clippy::type_complexity)]
pub fn py_to_symmetric_func(callable: PyObject) -> Box<dyn Fn(&Value, &Context) -> Result<Value>> {
    Box::new(move |obj, ctx| {
        Python::with_gil(|py| {
            let py_obj = value_to_py(py, obj).map_err(|e| err_to_generic("Symmetric value", e))?;
            let py_ctx = context_to_py_container(py, ctx)
                .map_err(|e| err_to_generic("Symmetric context", e))?;
            let result = callable
                .call1(py, (py_obj.bind(py), py_ctx.bind(py)))
                .map_err(|e| err_to_generic("Symmetric func error", e))?;
            py_to_value(py, result.bind(py))
                .map_err(|e| err_to_generic("Symmetric result conversion", e))
        })
    })
}

/// Adapts a Python callable into an adapter check function
/// (`Fn(&Value, &Context) -> Result<()>`).
///
/// Uses the two-argument calling convention: `callable(obj, context)`.
/// A truthy return means validation passed (`Ok(())`); falsy means failure
/// (`Err(ConstructError::Validation)`).
///
/// Used by `Validator` and `ExprValidator`.
#[allow(clippy::type_complexity)]
pub fn py_to_adapter_check_func(callable: PyObject) -> Box<dyn Fn(&Value, &Context) -> Result<()>> {
    Box::new(move |obj, ctx| {
        Python::with_gil(|py| {
            let py_obj = value_to_py(py, obj).map_err(|e| err_to_generic("Validator value", e))?;
            let py_ctx = context_to_py_container(py, ctx)
                .map_err(|e| err_to_generic("Validator context", e))?;
            let result = callable
                .call1(py, (py_obj.bind(py), py_ctx.bind(py)))
                .map_err(|e| err_to_generic("Validator error", e))?;
            if result.is_truthy(py).unwrap_or(false) {
                Ok(())
            } else {
                Err(ConstructError::Validation {
                    path: String::new(),
                    message: format!("object failed validation: {obj:?}"),
                })
            }
        })
    })
}

/// Adapts a Python callable into a control-flow check function
/// (`Fn(&Context) -> Result<()>`).
///
/// Functionally identical to [`py_to_check_func`]; provided as a separate
/// function for semantic clarity (used by the `Check` control-flow construct).
#[allow(clippy::type_complexity)]
pub fn py_to_control_check_func(callable: PyObject) -> Box<dyn Fn(&Context) -> Result<()>> {
    py_to_check_func(callable)
}

// ===========================================================================
// Stream-operation function bridges (10.8)
// ===========================================================================

use pyo3::types::PyBytes;

/// Adapts a Python callable into a stream_ops `TransformFunc`
/// (`Fn(&[u8]) -> Result<Vec<u8>>`).
///
/// Python calling convention: `callable(data: bytes) -> bytes`.
///
/// Used by `Transformed` (decode/encode) and `Restreamed` (decoder/encoder).
pub fn py_to_transform_func(
    callable: PyObject,
) -> construct::constructs::stream_ops::TransformFunc {
    Box::new(move |data: &[u8]| {
        Python::with_gil(|py| {
            let py_input = PyBytes::new_bound(py, data);
            let result = callable
                .call1(py, (py_input,))
                .map_err(|e| err_to_generic("Transform function error", e))?;
            let py_bytes = result.bind(py).downcast::<PyBytes>().map_err(|_| {
                err_to_generic("Transform function return type", "must return bytes")
            })?;
            Ok(py_bytes.as_bytes().to_vec())
        })
    })
}

/// Adapts a Python callable into a stream_ops `ChecksumFunc`
/// (`Fn(&[u8]) -> Vec<u8>`).
///
/// Python calling convention: `callable(data: bytes) -> bytes`.
/// On error, returns an empty checksum (fail-safe; the parsed value will not
/// match and `Checksum` raises a `ChecksumError`).
///
/// Used by `Checksum` (hashfunc).
pub fn py_to_checksum_func(callable: PyObject) -> construct::constructs::stream_ops::ChecksumFunc {
    Box::new(move |data: &[u8]| {
        Python::with_gil(|py| {
            let py_input = PyBytes::new_bound(py, data);
            match callable.call1(py, (py_input,)) {
                Ok(result) => {
                    if let Ok(py_bytes) = result.bind(py).downcast::<PyBytes>() {
                        py_bytes.as_bytes().to_vec()
                    } else {
                        Vec::new()
                    }
                }
                Err(_) => Vec::new(),
            }
        })
    })
}

/// Adapts a Python callable/expression into a stream_ops `ChecksumBytesFunc`
/// (`Fn(&Context) -> Result<Vec<u8>>`).
///
/// Python calling convention: `callable(context) -> bytes`.
/// Supports Path/BinExpr/lambda via [`PyClosure`].
///
/// Used by `Checksum` (bytesfunc).
pub fn py_to_checksum_bytes_func(
    callable: PyObject,
) -> construct::constructs::stream_ops::ChecksumBytesFunc {
    let closure = PyClosure::new(callable);
    Box::new(move |ctx: &Context| {
        let val = closure.evaluate(ctx, None)?;
        match val {
            Value::Bytes(b) => Ok(b),
            other => Err(ConstructError::Generic {
                path: String::new(),
                message: format!(
                    "Checksum bytesfunc must return bytes, got {}",
                    other.type_name()
                ),
            }),
        }
    })
}

/// Adapts a Python callable into a Restreamed `size_computer`
/// (`Fn(usize) -> usize`).
///
/// Python calling convention: `callable(n: int) -> int`.
/// On error, returns 0 (fail-safe; sizeof may be inaccurate but no panic).
pub fn py_to_size_computer(callable: PyObject) -> PyResult<Box<dyn Fn(usize) -> usize>> {
    Ok(Box::new(move |n: usize| {
        Python::with_gil(|py| match callable.call1(py, (n,)) {
            Ok(r) => r.extract::<usize>(py).unwrap_or(0),
            Err(_) => 0,
        })
    }))
}

// ===========================================================================
// Parameter helpers (dual-mode: callable vs plain value)
// ===========================================================================

/// Converts a Python parameter into a `Box<dyn Evaluate>`.
///
/// - **Callable** (Path, BinExpr, FuncPath, lambda) → wrapped in [`PyClosure`].
/// - **Non-callable** (int, bytes, float, bool) → wrapped in [`ConstExpr`].
///
/// This enables constructs like `Bytes(this.length)` (expression) and
/// `Bytes(5)` (constant) to work with the same API.
pub fn py_param_to_evaluate(
    py: Python<'_>,
    param: &Bound<'_, PyAny>,
) -> PyResult<Box<dyn Evaluate>> {
    if param.is_callable() {
        let callable: PyObject = param.into_py(py);
        Ok(Box::new(PyClosure::new(callable)))
    } else {
        let value = py_to_value(py, param)?;
        Ok(Box::new(ConstExpr::new(value)))
    }
}

/// Converts a Python parameter into a `CondFunc`.
///
/// - **Callable** → [`py_to_cond_func`].
/// - **Non-callable truthy** → constant function always returning `true`.
/// - **Non-callable falsy** → constant function always returning `false`.
#[allow(clippy::type_complexity)]
pub fn py_param_to_cond_func(
    py: Python<'_>,
    param: &Bound<'_, PyAny>,
) -> PyResult<Box<dyn Fn(&Context) -> bool>> {
    if param.is_callable() {
        let callable: PyObject = param.into_py(py);
        Ok(py_to_cond_func(callable))
    } else {
        let truthy = param.is_truthy()?;
        Ok(Box::new(move |_| truthy))
    }
}

/// Converts a Python parameter into a `ComputeFunc`.
///
/// - **Callable** → [`py_to_compute_func`].
/// - **Non-callable** → constant function returning the value.
#[allow(clippy::type_complexity)]
pub fn py_param_to_compute_func(
    py: Python<'_>,
    param: &Bound<'_, PyAny>,
) -> PyResult<Box<dyn Fn(&Context) -> Result<Value>>> {
    if param.is_callable() {
        let callable: PyObject = param.into_py(py);
        Ok(py_to_compute_func(callable))
    } else {
        let value = py_to_value(py, param)?;
        Ok(Box::new(move |_| Ok(value.clone())))
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: evaluate a Python expression against a context with one field.
    fn eval_expr(expr_py: &str, key: &str, val: i64) -> Value {
        crate::ensure_python();
        Python::with_gil(|py| {
            let closure = PyClosure::new(py.eval_bound(expr_py, None, None).unwrap().unbind());
            let mut ctx = Context::new();
            ctx.insert(key, Value::Int(val));
            closure.evaluate(&ctx, None).unwrap()
        })
    }

    #[test]
    fn pyclosure_evaluates_this_field() {
        let result = eval_expr("lambda ctx: ctx[\"x\"] + 1", "x", 41);
        assert_eq!(result, Value::Int(42));
    }

    #[test]
    fn pyclosure_evaluates_lambda_bool() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let closure = PyClosure::new(
                py.eval_bound("lambda ctx: ctx[\"x\"] > 0", None, None)
                    .unwrap()
                    .unbind(),
            );
            let mut ctx = Context::new();
            ctx.insert("x", Value::Int(5));
            let result = closure.evaluate(&ctx, None).unwrap();
            assert_eq!(result, Value::Bool(true));
        });
    }

    #[test]
    fn pyclosure_returns_none() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let closure = PyClosure::new(
                py.eval_bound("lambda ctx: None", None, None)
                    .unwrap()
                    .unbind(),
            );
            let ctx = Context::new();
            let result = closure.evaluate(&ctx, None).unwrap();
            assert_eq!(result, Value::None);
        });
    }

    #[test]
    fn py_to_cond_func_truthy() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let callable = py
                .eval_bound("lambda ctx: ctx[\"flag\"]", None, None)
                .unwrap()
                .unbind();
            let cond = py_to_cond_func(callable);
            let mut ctx = Context::new();
            ctx.insert("flag", Value::Bool(true));
            assert!(cond(&ctx));
        });
    }

    #[test]
    fn py_to_cond_func_falsy() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let callable = py
                .eval_bound("lambda ctx: ctx[\"flag\"]", None, None)
                .unwrap()
                .unbind();
            let cond = py_to_cond_func(callable);
            let mut ctx = Context::new();
            ctx.insert("flag", Value::Bool(false));
            assert!(!cond(&ctx));
        });
    }

    #[test]
    fn py_to_key_func_returns_value() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let callable = py
                .eval_bound("lambda ctx: ctx[\"k\"]", None, None)
                .unwrap()
                .unbind();
            let keyfunc = py_to_key_func(callable);
            let mut ctx = Context::new();
            ctx.insert("k", Value::Int(7));
            let result = keyfunc(&ctx).unwrap();
            assert_eq!(result, Value::Int(7));
        });
    }

    #[test]
    fn py_to_check_func_pass() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let callable = py
                .eval_bound("lambda ctx: True", None, None)
                .unwrap()
                .unbind();
            let check = py_to_check_func(callable);
            assert!(check(&Context::new()).is_ok());
        });
    }

    #[test]
    fn py_to_check_func_fail() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let callable = py
                .eval_bound("lambda ctx: False", None, None)
                .unwrap()
                .unbind();
            let check = py_to_check_func(callable);
            assert!(check(&Context::new()).is_err());
        });
    }

    #[test]
    fn py_to_repeat_predicate_true() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let callable = py
                .eval_bound("lambda e, lst, ctx: e == 5", None, None)
                .unwrap()
                .unbind();
            let pred = py_to_repeat_predicate(callable);
            assert!(pred(
                &Value::Int(5),
                &[Value::Int(3), Value::Int(5)],
                &Context::new()
            ));
        });
    }

    #[test]
    fn py_to_repeat_predicate_false() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let callable = py
                .eval_bound("lambda e, lst, ctx: e == 255", None, None)
                .unwrap()
                .unbind();
            let pred = py_to_repeat_predicate(callable);
            assert!(!pred(
                &Value::Int(7),
                &[Value::Int(3), Value::Int(7)],
                &Context::new()
            ));
        });
    }

    #[test]
    fn py_param_to_evaluate_callable() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let lambda = py.eval_bound("lambda ctx: ctx[\"x\"]", None, None).unwrap();
            let expr = py_param_to_evaluate(py, &lambda).unwrap();
            let mut ctx = Context::new();
            ctx.insert("x", Value::Int(99));
            assert_eq!(expr.evaluate(&ctx, None).unwrap(), Value::Int(99));
        });
    }

    #[test]
    fn py_param_to_evaluate_constant() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let val = 42i64.into_py(py);
            let expr = py_param_to_evaluate(py, val.bind(py)).unwrap();
            let ctx = Context::new();
            assert_eq!(expr.evaluate(&ctx, None).unwrap(), Value::Int(42));
        });
    }

    #[test]
    fn py_param_to_cond_func_constant() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let true_obj = true.into_py(py);
            let cond = py_param_to_cond_func(py, true_obj.bind(py)).unwrap();
            assert!(cond(&Context::new()));

            let false_obj = false.into_py(py);
            let cond2 = py_param_to_cond_func(py, false_obj.bind(py)).unwrap();
            assert!(!cond2(&Context::new()));
        });
    }

    #[test]
    fn pyclosure_error_on_missing_field() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let closure = PyClosure::new(
                py.eval_bound("lambda ctx: ctx[\"missing\"]", None, None)
                    .unwrap()
                    .unbind(),
            );
            let ctx = Context::new();
            let err = closure.evaluate(&ctx, None).unwrap_err();
            assert!(matches!(err, ConstructError::Expr { .. }));
        });
    }

    #[test]
    fn pyclosure_returns_bytes() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let closure = PyClosure::new(
                py.eval_bound("lambda ctx: b'hello'", None, None)
                    .unwrap()
                    .unbind(),
            );
            let ctx = Context::new();
            let result = closure.evaluate(&ctx, None).unwrap();
            assert_eq!(result, Value::Bytes(b"hello".to_vec()));
        });
    }

    #[test]
    fn pyclosure_returns_list() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let closure = PyClosure::new(
                py.eval_bound("lambda ctx: [1, 2, 3]", None, None)
                    .unwrap()
                    .unbind(),
            );
            let ctx = Context::new();
            let result = closure.evaluate(&ctx, None).unwrap();
            assert_eq!(
                result,
                Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
            );
        });
    }

    #[test]
    fn py_to_decode_func_transforms_value() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let callable = py
                .eval_bound("lambda obj, ctx: obj + 1", None, None)
                .unwrap()
                .unbind();
            let decode = py_to_decode_func(callable);
            let ctx = Context::new();
            let result = decode(&Value::Int(4), &ctx).unwrap();
            assert_eq!(result, Value::Int(5));
        });
    }

    #[test]
    fn py_to_encode_func_transforms_value() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let callable = py
                .eval_bound("lambda obj, ctx: obj - 1", None, None)
                .unwrap()
                .unbind();
            let encode = py_to_encode_func(callable);
            let ctx = Context::new();
            let result = encode(&Value::Int(5), &ctx).unwrap();
            assert_eq!(result, Value::Int(4));
        });
    }

    #[test]
    fn py_to_symmetric_func_applies_same_both_directions() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let callable = py
                .eval_bound("lambda obj, ctx: obj & 0xf", None, None)
                .unwrap()
                .unbind();
            let func = py_to_symmetric_func(callable);
            let ctx = Context::new();
            assert_eq!(func(&Value::UInt(0xFF), &ctx).unwrap(), Value::Int(15));
            assert_eq!(func(&Value::UInt(0x1F), &ctx).unwrap(), Value::Int(15));
        });
    }

    #[test]
    fn py_to_adapter_check_func_passes() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let callable = py
                .eval_bound("lambda obj, ctx: obj <= 10", None, None)
                .unwrap()
                .unbind();
            let check = py_to_adapter_check_func(callable);
            let ctx = Context::new();
            assert!(check(&Value::Int(5), &ctx).is_ok());
        });
    }

    #[test]
    fn py_to_adapter_check_func_fails() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let callable = py
                .eval_bound("lambda obj, ctx: obj <= 10", None, None)
                .unwrap()
                .unbind();
            let check = py_to_adapter_check_func(callable);
            let ctx = Context::new();
            let err = check(&Value::Int(20), &ctx).unwrap_err();
            assert!(matches!(err, ConstructError::Validation { .. }));
        });
    }
}
