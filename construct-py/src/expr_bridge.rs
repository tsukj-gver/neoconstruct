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
pub fn py_to_cond_func(callable: PyObject) -> Box<dyn Fn(&Context) -> bool + Send + Sync> {
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
pub fn py_to_key_func(callable: PyObject) -> Box<dyn Fn(&Context) -> Result<Value> + Send + Sync> {
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
pub fn py_to_check_func(callable: PyObject) -> Box<dyn Fn(&Context) -> Result<()> + Send + Sync> {
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
pub fn py_to_compute_func(
    callable: PyObject,
) -> Box<dyn Fn(&Context) -> Result<Value> + Send + Sync> {
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
) -> Box<dyn Fn(&Value, &[Value], &Context) -> bool + Send + Sync> {
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
) -> PyResult<Box<dyn Fn(&Context) -> bool + Send + Sync>> {
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
) -> PyResult<Box<dyn Fn(&Context) -> Result<Value> + Send + Sync>> {
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
}
