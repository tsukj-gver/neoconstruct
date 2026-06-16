//! [`PyCallback`] — a Python callable embedded in the compiled execution tree.
//!
//! This is the **sole** tree-internal FFI point (design §5, decision D6). When
//! `exec_parse` / `exec_build` reaches a [`CompiledNode::External`] node wrapping
//! a `PyCallback`, it crosses into Python (acquiring the GIL), creates a lazy
//! [`PyContextView`] snapshot, calls the Python callable, converts the result
//! back to a Rust [`Value`], and deposits it via [`ProducedOutput`].
//!
//! # Callback kinds
//!
//! | [`CallbackKind`] | Python calling convention                | Use case                       |
//! |------------------|------------------------------------------|--------------------------------|
//! | [`Construct`](CallbackKind::Construct) | `obj.parse_stream(stream, **kw)` / `obj.build_stream(val, stream, **kw)` | User-defined Construct subclass |
//! | [`Expr`](CallbackKind::Expr)           | `callable(context_view) → value`          | Expression / compute callable   |
//!
//! # Design deviation (D1)
//!
//! The design doc §5.2 specifies `ext_parse(&self, stream, ctx, sink)`. The
//! **actual** [`CompiledExtension`] trait (implemented in `compiled/mod.rs`)
//! uses `ext_parse(&self, stream, ctx) -> Result<ProducedOutput>` (no sink,
//! returns [`ProducedOutput`]). This implementation matches the **actual**
//! trait signature, which is the one the compiler enforces.

use construct::compiled::input::Input;
use construct::compiled::sink::ProducedOutput;
use construct::compiled::{CompiledExtension, ExprExtension};
use construct::core::context::Context;
use construct::core::error::{ConstructError, Result};
use construct::core::stream::{CombinedStream, Stream};
use construct::value::Value;

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};

use crate::conversions::{py_to_value, value_to_py};
use crate::py_context::PyContextView;
use crate::py_sink::py_err_to_construct;

// ===========================================================================
// CallbackKind
// ===========================================================================

/// Kind of Python callback — determines the calling convention used to invoke
/// the wrapped Python callable.
///
/// This mirrors design §5.4's `CallbackKind` (renamed from the PM draft's
/// Parse/Build/Symmetric/Check model to the design's Construct/Expr model,
/// which maps directly onto the two real-world Python callback shapes).
#[derive(Clone, Copy, Debug)]
pub enum CallbackKind {
    /// A user-defined Python `Construct` subclass. The callable is expected
    /// to expose `parse_stream(stream, **kw)` and `build_stream(obj, stream,
    /// **kw)` methods (duck-typed, same contract as `PyConstructAdapter`).
    ///
    /// Uses the **BytesIO read-all** strategy: `ext_parse` reads remaining
    /// bytes from the Rust stream, wraps them in a Python `io.BytesIO`, calls
    /// `parse_stream`, then syncs the consumed position back. This mirrors
    /// the existing `PyConstructAdapter` flow (now expressed as a compiled
    /// tree node rather than a declaration-tree `Construct` impl).
    Construct,

    /// An expression / compute callable: `callable(context_view) → value`.
    /// A [`PyContextView`] snapshot is created from the live [`Context`] and
    /// passed as the sole argument. The callable returns a value, which is
    /// converted to a Rust [`Value`].
    ///
    /// This covers both design §5.4's `Expr` and `Compute` kinds — both share
    /// the identical `callable(context) → value` contract.
    Expr,
}

// ===========================================================================
// PyCallback
// ===========================================================================

/// A Python callable embedded in the compiled execution tree.
///
/// Implements [`CompiledExtension`], so a `PyCallback` can be wrapped in
/// [`CompiledExternal`](construct::compiled::CompiledExternal) and inserted
/// into a [`CompiledNode`](construct::compiled::CompiledNode) tree. This is
/// the only construct-py → construct-rs integration point for Python
/// callables in the compiled path.
///
/// # Thread safety
///
/// `Py<PyAny>` is `Send + Sync` (PyO3 guarantee), so `PyCallback` satisfies
/// the `Send + Sync` bound of [`CompiledExtension`]. All Python access goes
/// through `Python::with_gil`, which is safe and re-entrant.
pub struct PyCallback {
    /// The wrapped Python callable (a Construct subclass object or a plain
    /// function, depending on [`CallbackKind`]).
    callable: Py<PyAny>,
    /// Determines how `callable` is invoked.
    kind: CallbackKind,
}

impl PyCallback {
    /// Creates a new [`PyCallback`] wrapping the given Python callable.
    ///
    /// The `kind` argument selects the calling convention
    /// ([`CallbackKind::Construct`] for `parse_stream`/`build_stream`
    /// duck-typed objects, [`CallbackKind::Expr`] for `callable(ctx) → value`
    /// functions).
    #[must_use]
    pub fn new(callable: Py<PyAny>, kind: CallbackKind) -> Self {
        Self { callable, kind }
    }

    /// Returns the callback kind.
    #[must_use]
    pub fn kind(&self) -> CallbackKind {
        self.kind
    }

    /// Builds a kwargs [`PyDict`] from the local fields of a [`Context`]
    /// (for the `Construct` kind's `**kw` argument).
    ///
    /// Only the current context level's fields are included (not the parent
    /// chain) — this matches the existing `py_adapter` behaviour where
    /// `**contextkw` carries the top-level kwargs injected by the user.
    fn context_to_kwargs<'py>(py: Python<'py>, ctx: &Context) -> PyResult<Bound<'py, PyDict>> {
        let py_dict = PyDict::new_bound(py);
        for (key, value) in ctx.iter_fields() {
            py_dict.set_item(key, value_to_py(py, value)?)?;
        }
        Ok(py_dict)
    }

    /// Imports `io.BytesIO`, returning the class object.
    fn get_bytesio(py: Python<'_>) -> Result<PyObject> {
        py.import_bound("io")
            .and_then(|m| m.getattr("BytesIO"))
            .map(|c| c.unbind())
            .map_err(|e| Self::err(format!("Failed to import io.BytesIO: {e}")))
    }

    /// Shorthand for a path-less generic [`ConstructError`].
    fn err(msg: impl Into<String>) -> ConstructError {
        ConstructError::Generic {
            path: String::new(),
            message: msg.into(),
        }
    }
}

impl std::fmt::Debug for PyCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyCallback")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

// ===========================================================================
// CompiledExtension impl
// ===========================================================================

impl CompiledExtension for PyCallback {
    fn ext_parse(&self, stream: &mut CombinedStream, ctx: &mut Context) -> Result<ProducedOutput> {
        Python::with_gil(|py| -> Result<ProducedOutput> {
            match self.kind {
                CallbackKind::Expr => {
                    // callable(context_view) → value
                    let ctx_view = PyContextView::from_context(ctx);
                    let ctx_py = Py::new(py, ctx_view).map_err(py_err_to_construct)?;
                    let result = self
                        .callable
                        .call1(py, (ctx_py,))
                        .map_err(py_err_to_construct)?;
                    let value = py_to_value(py, result.bind(py)).map_err(py_err_to_construct)?;
                    Ok(ProducedOutput::Value(value))
                }
                CallbackKind::Construct => {
                    // BytesIO read-all strategy (mirrors PyConstructAdapter).
                    // 1. Record current stream position.
                    let start_pos = stream.tell()?;
                    // 2. Read remaining bytes from the Rust stream.
                    let remaining = stream.read_remaining()?;
                    // 3. Create Python BytesIO with the data.
                    let bytesio_class = Self::get_bytesio(py)?;
                    let py_stream = bytesio_class
                        .call1(py, (PyBytes::new_bound(py, &remaining),))
                        .map_err(|e| Self::err(format!("Failed to create BytesIO: {e}")))?;
                    // 4. Build kwargs from context.
                    let kwargs = Self::context_to_kwargs(py, ctx).map_err(py_err_to_construct)?;
                    // 5. Call parse_stream(stream, **kwargs).
                    let result = self
                        .callable
                        .bind(py)
                        .call_method("parse_stream", (py_stream.bind(py),), Some(&kwargs))
                        .map_err(py_err_to_construct)?;
                    // 6. Sync Rust stream position from BytesIO tell().
                    let consumed: u64 = py_stream
                        .call_method0(py, "tell")
                        .and_then(|v| v.extract(py))
                        .map_err(|e| Self::err(format!("Invalid stream position: {e}")))?;
                    stream.seek(start_pos + consumed)?;
                    // 7. Convert result → Value → ProducedOutput.
                    let value = py_to_value(py, &result).map_err(py_err_to_construct)?;
                    Ok(ProducedOutput::Value(value))
                }
            }
        })
    }

    fn ext_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        Python::with_gil(|py| -> Result<()> {
            match self.kind {
                CallbackKind::Expr => {
                    // callable(context_view) → value (then write value as bytes
                    // via value_to_py? No — Expr kind has no build semantics
                    // in the design. We extract the value and write it as a
                    // best-effort scalar; the common case is that Expr
                    // callbacks are parse-only (Computed/Rebuild use
                    // ExprExtension, not CompiledExtension). Return Ok.
                    // Build for an Expr-kind PyCallback is a no-op: the
                    // callable produces a value from context, not bytes.
                    let _ = input.as_value()?; // validate input is readable
                    let _ = (py, ctx);
                    Ok(())
                }
                CallbackKind::Construct => {
                    // BytesIO write strategy (mirrors PyConstructAdapter).
                    // 1. Value → Python object.
                    let value = input.as_value()?;
                    let py_data = value_to_py(py, &value).map_err(py_err_to_construct)?;
                    // 2. Create empty Python BytesIO for writing.
                    let bytesio_class = Self::get_bytesio(py)?;
                    let py_stream = bytesio_class
                        .call0(py)
                        .map_err(|e| Self::err(format!("Failed to create BytesIO: {e}")))?;
                    // 3. Build kwargs from context.
                    let kwargs = Self::context_to_kwargs(py, ctx).map_err(py_err_to_construct)?;
                    // 4. Call build_stream(obj, stream, **kwargs).
                    self.callable
                        .bind(py)
                        .call_method(
                            "build_stream",
                            (py_data.bind(py), py_stream.bind(py)),
                            Some(&kwargs),
                        )
                        .map_err(py_err_to_construct)?;
                    // 5. Read bytes from BytesIO getvalue() and write to Rust stream.
                    let built: PyObject = py_stream
                        .call_method0(py, "getvalue")
                        .map_err(|e| Self::err(format!("Failed to get built bytes: {e}")))?;
                    let bytes_obj = built
                        .bind(py)
                        .downcast::<PyBytes>()
                        .map_err(|_| Self::err("Python build_stream did not produce bytes"))?;
                    let written = bytes_obj.as_bytes();
                    if !written.is_empty() {
                        stream.write_bytes(written)?;
                    }
                    Ok(())
                }
            }
        })
    }

    fn ext_sizeof(&self) -> Result<usize> {
        // Python callables generally do not expose a statically knowable size.
        // We attempt to call a `sizeof`-equivalent if the callable supports it
        // (Construct kind only); otherwise report Sizeof.
        match self.kind {
            CallbackKind::Expr => Err(ConstructError::Sizeof {
                path: String::new(),
                reason: "PyCallback (Expr kind) has no static size".to_string(),
            }),
            CallbackKind::Construct => {
                Python::with_gil(|py| -> Result<usize> {
                    // Try calling the callable's `sizeof`-equivalent. Most
                    // Python constructs expose `sizeof` via the Construct
                    // protocol, but it requires a context. Since ext_sizeof
                    // receives none (the trait does not pass one), we report
                    // Sizeof for dynamic Python constructs.
                    let _ = (py,);
                    Err(ConstructError::Sizeof {
                        path: String::new(),
                        reason: "PyCallback (Construct kind) size is dynamic".to_string(),
                    })
                })
            }
        }
    }
}

// ===========================================================================
// PyExprCallback
// ===========================================================================

/// A Python callable wrapped as an [`ExprExtension`] for
/// [`CompiledExpr::External`](construct::compiled::CompiledExpr::External).
///
/// Unlike [`PyCallback`] (which is a tree node implementing
/// [`CompiledExtension`]), `PyExprCallback` is an **expression** — it
/// evaluates against a [`Context`] and returns a [`Value`], used inside
/// constructs like `Switch`, `ArrayExpr`, `IfThenElse` whose count/key/cond
/// expressions may be backed by Python callables.
///
/// At evaluation time, a [`PyContextView`] snapshot is created from the live
/// [`Context`] and passed to the Python callable. This is the degraded path
/// (design §4.5): the common `this.field` expressions compile to native Rust
/// closures with **zero** FFI; only lambdas / dynamic callables reach this
/// path (~5% of expressions).
#[derive(Debug)]
pub struct PyExprCallback {
    /// The wrapped Python callable: `callable(context_view) → value`.
    callable: Py<PyAny>,
}

impl PyExprCallback {
    /// Creates a new [`PyExprCallback`] wrapping the given Python callable.
    ///
    /// The callable must accept a single argument (a [`PyContextView`]) and
    /// return a value convertible to a Rust [`Value`].
    #[must_use]
    pub fn new(callable: Py<PyAny>) -> Self {
        Self { callable }
    }
}

impl ExprExtension for PyExprCallback {
    fn ext_eval(&self, ctx: &Context) -> Result<Value> {
        Python::with_gil(|py| -> Result<Value> {
            let ctx_view = PyContextView::from_context(ctx);
            let ctx_py = Py::new(py, ctx_view).map_err(py_err_to_construct)?;
            // KeyError → FieldMissing special handling (design §8.3):
            // a callable raising KeyError signals a missing context field.
            let result = self.callable.call1(py, (ctx_py,)).map_err(|pyerr| {
                if pyerr
                    .get_type_bound(py)
                    .is(&py.get_type_bound::<pyo3::exceptions::PyKeyError>())
                {
                    ConstructError::FieldMissing {
                        path: String::new(),
                        field: "PyExprCallback KeyError".to_string(),
                    }
                } else {
                    py_err_to_construct(pyerr)
                }
            })?;
            py_to_value(py, result.bind(py)).map_err(py_err_to_construct)
        })
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use construct::compiled::sink::{OutputSink, ValueSink};
    use construct::core::stream::{ByteStream, CombinedStream};
    use construct::value::Value;

    // -- PyCallback (Expr kind): ext_parse ----------------------------------

    #[test]
    fn py_callback_expr_ext_parse_returns_callable_value() {
        crate::ensure_python();
        // A Python callable that ignores its context arg and returns 42.
        let callable = Python::with_gil(|py| {
            py.eval_bound("lambda ctx: 42", None, None)
                .unwrap()
                .unbind()
        });
        let cb = PyCallback::new(callable, CallbackKind::Expr);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[]));
        let mut ctx = Context::new();
        let produced = cb.ext_parse(&mut stream, &mut ctx).unwrap();
        match produced {
            ProducedOutput::Value(Value::Int(42)) => {}
            ProducedOutput::Value(v) => panic!("expected Value(Int(42)), got Value({v:?})"),
            ProducedOutput::Object(_) => panic!("expected Value(Int(42)), got Object"),
        }
    }

    #[test]
    fn py_callback_expr_ext_parse_reads_context_field() {
        crate::ensure_python();
        // callable reads ctx["count"] and returns it doubled.
        let callable = Python::with_gil(|py| {
            py.eval_bound("lambda ctx: ctx['count'] * 2", None, None)
                .unwrap()
                .unbind()
        });
        let cb = PyCallback::new(callable, CallbackKind::Expr);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[]));
        let mut ctx = Context::new();
        ctx.insert("count", Value::Int(21));
        let produced = cb.ext_parse(&mut stream, &mut ctx).unwrap();
        match produced {
            ProducedOutput::Value(Value::Int(42)) => {}
            ProducedOutput::Value(v) => panic!("expected Value(Int(42)), got Value({v:?})"),
            ProducedOutput::Object(_) => panic!("expected Value(Int(42)), got Object"),
        }
    }

    // -- PyCallback (Expr kind): ext_build ----------------------------------

    #[test]
    fn py_callback_expr_ext_build_is_noop() {
        crate::ensure_python();
        let callable = Python::with_gil(|py| py.None());
        let cb = PyCallback::new(callable, CallbackKind::Expr);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        let input = construct::compiled::input::ValueInput::new(&Value::Int(1));
        // Expr-kind build is a no-op; succeeds without writing bytes.
        cb.ext_build(&input, &mut stream, &mut ctx).unwrap();
        assert_eq!(stream.into_bytes(), Vec::<u8>::new());
    }

    // -- PyCallback: ext_sizeof --------------------------------------------

    #[test]
    fn py_callback_ext_sizeof_returns_sizeof_error() {
        crate::ensure_python();
        let callable = Python::with_gil(|py| py.None());
        let cb = PyCallback::new(callable, CallbackKind::Expr);
        let err = cb.ext_sizeof().unwrap_err();
        assert!(matches!(err, ConstructError::Sizeof { .. }));
    }

    // -- PyCallback: CompiledExternal integration (parse via tree) ---------

    #[test]
    fn py_callback_embedded_in_compiled_external_parse_deposits_value() {
        crate::ensure_python();
        use construct::compiled::{CompiledExec, CompiledExternal, CompiledNode};
        let callable = Python::with_gil(|py| {
            py.eval_bound("lambda ctx: 99", None, None)
                .unwrap()
                .unbind()
        });
        let cb = PyCallback::new(callable, CallbackKind::Expr);
        let node = CompiledNode::External(CompiledExternal {
            inner: Box::new(cb),
        });
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[]));
        let mut ctx = Context::new();
        let mut sink = ValueSink::new();
        node.exec_parse(&mut stream, &mut ctx, &mut sink).unwrap();
        let val = Box::new(sink).into_value().unwrap();
        assert_eq!(val, Value::Int(99));
    }

    // -- PyCallback: Construct kind (parse_stream via BytesIO) -------------

    #[test]
    fn py_callback_construct_ext_parse_calls_parse_stream() {
        crate::ensure_python();
        // A minimal Python Construct-like object with parse_stream that reads
        // 1 byte and returns it as an int. Built via type() (expression, since
        // eval_bound cannot execute statements).
        let code = "type('FC', (), {'parse_stream': lambda self, s, **kw: s.read(1)[0], 'build_stream': lambda self, o, s, **kw: s.write(bytes([o]))})()";
        let callable = Python::with_gil(|py| py.eval_bound(code, None, None).unwrap().unbind());
        let cb = PyCallback::new(callable, CallbackKind::Construct);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[0x41, 0x42]));
        let mut ctx = Context::new();
        let produced = cb.ext_parse(&mut stream, &mut ctx).unwrap();
        match produced {
            ProducedOutput::Value(Value::Int(0x41)) => {}
            ProducedOutput::Value(v) => panic!("expected Value(Int(0x41)), got Value({v:?})"),
            ProducedOutput::Object(_) => panic!("expected Value(Int(0x41)), got Object"),
        }
        // The BytesIO strategy should have consumed exactly 1 byte.
        let pos = stream.tell().unwrap();
        assert_eq!(pos, 1);
    }

    #[test]
    fn py_callback_construct_ext_build_calls_build_stream() {
        crate::ensure_python();
        let code = "type('FC', (), {'parse_stream': lambda self, s, **kw: s.read(1)[0], 'build_stream': lambda self, o, s, **kw: s.write(bytes([o]))})()";
        let callable = Python::with_gil(|py| py.eval_bound(code, None, None).unwrap().unbind());
        let cb = PyCallback::new(callable, CallbackKind::Construct);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        let input = construct::compiled::input::ValueInput::new(&Value::Int(0x77));
        cb.ext_build(&input, &mut stream, &mut ctx).unwrap();
        assert_eq!(stream.into_bytes(), vec![0x77]);
    }

    // -- PyExprCallback: ext_eval ------------------------------------------

    #[test]
    fn py_expr_callback_ext_eval_returns_value() {
        crate::ensure_python();
        let callable = Python::with_gil(|py| {
            py.eval_bound("lambda ctx: ctx['x'] + 10", None, None)
                .unwrap()
                .unbind()
        });
        let ext = PyExprCallback::new(callable);
        let mut ctx = Context::new();
        ctx.insert("x", Value::Int(5));
        let result = ext.ext_eval(&ctx).unwrap();
        assert_eq!(result, Value::Int(15));
    }

    #[test]
    fn py_expr_callback_ext_eval_keyerror_maps_to_fieldmissing() {
        crate::ensure_python();
        // callable accesses a missing key → raises KeyError → FieldMissing.
        let callable = Python::with_gil(|py| {
            py.eval_bound("lambda ctx: ctx['missing']", None, None)
                .unwrap()
                .unbind()
        });
        let ext = PyExprCallback::new(callable);
        let ctx = Context::new();
        let err = ext.ext_eval(&ctx).unwrap_err();
        assert!(matches!(err, ConstructError::FieldMissing { .. }));
    }

    // -- PyExprCallback: CompiledExpr::External integration ----------------

    #[test]
    fn py_expr_callback_in_compiled_expr_external_evaluates() {
        crate::ensure_python();
        use construct::compiled::expr::CompiledExpr;
        use std::sync::Arc;
        let callable = Python::with_gil(|py| {
            py.eval_bound("lambda ctx: ctx['n'] * 3", None, None)
                .unwrap()
                .unbind()
        });
        let ext: Arc<dyn ExprExtension> = Arc::new(PyExprCallback::new(callable));
        let compiled = CompiledExpr::External(ext);
        let mut ctx = Context::new();
        ctx.insert("n", Value::Int(14));
        let result = compiled.eval(&ctx, None).unwrap();
        assert_eq!(result, Value::Int(42));
    }

    // -- Debug impls --------------------------------------------------------

    #[test]
    fn py_callback_debug_does_not_panic() {
        crate::ensure_python();
        let callable = Python::with_gil(|py| py.None());
        let cb = PyCallback::new(callable, CallbackKind::Expr);
        let _ = format!("{cb:?}");
    }

    #[test]
    fn py_expr_callback_debug_does_not_panic() {
        crate::ensure_python();
        let callable = Python::with_gil(|py| py.None());
        let cb = PyExprCallback::new(callable);
        let _ = format!("{cb:?}");
    }

    // -- kind() accessor ----------------------------------------------------

    #[test]
    fn py_callback_kind_accessor() {
        crate::ensure_python();
        let callable = Python::with_gil(|py| py.None());
        let cb = PyCallback::new(callable, CallbackKind::Construct);
        assert!(matches!(cb.kind(), CallbackKind::Construct));
    }
}
