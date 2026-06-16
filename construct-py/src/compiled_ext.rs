//! [`CompiledSchemaHolder`] — construct-py wrapper around a compiled schema,
//! providing the **direct-to-Python** parse/build entry points.
//!
//! See design §2.3 (`parse_bytes_py`) / §3.5 (`build_from_py`) / §7.3 (caching).
//!
//! # Why a wrapper?
//!
//! `CompiledSchema` is defined in construct-rs. Rust's orphan rule forbids
//! adding inherent methods (like `parse_bytes_py`) to a foreign type from
//! construct-py. `CompiledSchemaHolder` owns an `Arc<CompiledSchema>` and
//! hosts the Python-facing entry points as its own methods (design I-FEAS-1).
//!
//! # Direct-to-Python path
//!
//! - [`CompiledSchemaHolder::parse_bytes_py`] writes parse results straight
//!   into a `PyDict` / `PyList` via [`PyDictSink`] — no `Value::Container`
//!   intermediate tree is built (design S-ARCH-2).
//! - [`CompiledSchemaHolder::build_from_py`] reads build input lazily via
//!   [`PyInput`] — only fields actually consumed by `exec_build` incur FFI.

use std::sync::Arc;

use construct::combined::CombinedConstruct;
use construct::compiled::input::Input;
use construct::compiled::{CompiledExec, CompiledSchema};
use construct::compiler::SchemaCompiler;
use construct::core::context::Context;
use construct::core::stream::{ByteStream, CombinedStream};
use construct::value::Value;

use indexmap::IndexMap;
use pyo3::prelude::*;
use pyo3::types::PyBytes;

use crate::exceptions::rust_err_to_py;
use crate::py_input::PyInput;
use crate::py_sink::PyDictSink;

/// Path prefix added to errors originating from parse operations.
const PARSE_PATH: &str = "(parsing)";

/// Path prefix added to errors originating from build operations.
const BUILD_PATH: &str = "(building)";

// ===========================================================================
// CompiledSchemaHolder
// ===========================================================================

/// construct-py wrapper around [`CompiledSchema`] — holds the compiled
/// execution tree and provides Python-facing parse/build entry points.
///
/// This wrapper exists to work around the Rust orphan rule: we cannot add
/// inherent methods to `CompiledSchema` (defined in construct-rs) from
/// construct-py. Instead, `CompiledSchemaHolder` owns an `Arc<CompiledSchema>`
/// and provides `parse_bytes_py` / `build_from_py` as its own methods.
///
/// It also serves as the compilation cache carrier (design §7.3): a PyO3
/// wrapper may store a `CompiledSchemaHolder` (or `Arc<CompiledSchema>`) in a
/// `OnceLock` to avoid recompiling on every parse/build call.
pub struct CompiledSchemaHolder {
    /// The compiled execution tree, shared via `Arc` for cheap cloning.
    schema: Arc<CompiledSchema>,
}

impl CompiledSchemaHolder {
    /// Creates a new holder wrapping a compiled schema.
    #[must_use]
    pub fn new(schema: Arc<CompiledSchema>) -> Self {
        Self { schema }
    }

    /// Compiles a [`CombinedConstruct`] declaration tree into a
    /// [`CompiledSchemaHolder`].
    ///
    /// This is the convenience entry point for construct-py: it runs
    /// [`SchemaCompiler::compile`] and wraps the result.
    ///
    /// # Errors
    ///
    /// Returns `PyErr` if compilation fails (e.g. an uncompilable construct
    /// variant). The error is mapped to a construct exception subclass.
    pub fn compile(construct: &CombinedConstruct) -> PyResult<Self> {
        let compiler = SchemaCompiler::new();
        let schema = compiler
            .compile(construct)
            .map_err(|e| Python::with_gil(|py| rust_err_to_py(py, e)))?;
        // CompiledSchema is not Send + Sync (it may contain Arc<dyn Construct>
        // via the Dynamic escape-hatch). This is the same I3 design decision as
        // construct-rs's CompiledSchema::new — the Arc is used for cheap
        // cloning/caching, not cross-thread sharing.
        #[allow(clippy::arc_with_non_send_sync)]
        Ok(Self::new(Arc::new(schema)))
    }

    /// Returns a reference to the wrapped [`CompiledSchema`].
    #[must_use]
    pub fn schema(&self) -> &CompiledSchema {
        &self.schema
    }

    /// Parses binary data and returns the result as a Python object.
    ///
    /// **Direct-to-Python path** (Phase 13): the entire compiled tree is
    /// traversed in Rust while holding the GIL. Results are written directly
    /// into a `PyDict` / `PyList` via [`PyDictSink`] — no `Value::Container`
    /// intermediate tree is built.
    ///
    /// `kw` provides top-level context fields (matching the `**contextkw`
    /// convention of the Python original).
    ///
    /// # Errors
    ///
    /// Returns `PyErr` (a construct exception subclass) on parse failure.
    /// On error, the partially-built `PyDict` is **dropped** (discarded) —
    /// the Python caller receives only the exception (design §8, REV #9
    /// scheme A).
    pub fn parse_bytes_py(
        &self,
        py: Python<'_>,
        data: &[u8],
        kw: IndexMap<String, Value>,
    ) -> PyResult<PyObject> {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(data));
        let mut ctx = Context::new();
        ctx.insert("_parsing", Value::Bool(true));
        ctx.insert("_building", Value::Bool(false));
        ctx.insert("_sizing", Value::Bool(false));
        for (key, value) in kw {
            ctx.insert(key, value);
        }
        let mut sink = PyDictSink::new_root(py);
        match self
            .schema
            .tree()
            .exec_parse(&mut stream, &mut ctx, &mut sink)
        {
            Ok(()) => sink.into_py_object(),
            Err(e) => Err(rust_err_to_py(py, e.with_path_prefix(PARSE_PATH))),
        }
        // Err path: `sink` (PyDictSink owning a Py<PyDict>) is dropped here.
        // PyO3 decrements the refcount; Python GC reclaims the partial dict.
        // Python only sees the exception.
    }

    /// Builds binary data from a Python object, returning it as `bytes`.
    ///
    /// Uses [`PyInput`] for lazy field reading — only fields actually
    /// consumed by `exec_build` incur FFI, not the entire object.
    ///
    /// `kw` provides top-level context fields (matching `**contextkw`).
    ///
    /// # Errors
    ///
    /// Returns `PyErr` on build failure.
    pub fn build_from_py(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        kw: IndexMap<String, Value>,
    ) -> PyResult<PyObject> {
        let input = PyInput::from_bound(obj);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        ctx.insert("_parsing", Value::Bool(false));
        ctx.insert("_building", Value::Bool(true));
        ctx.insert("_sizing", Value::Bool(false));
        // Set the embedding value in context (context._ = obj), matching the
        // Python original so condition-based constructs (Optional, IfThenElse)
        // can inspect the value being built via `ctx.get("_")`.
        if let Ok(embed) = input.as_value() {
            ctx.insert("_", embed);
        }
        for (key, value) in kw {
            ctx.insert(key, value);
        }
        match self.schema.tree().exec_build(&input, &mut stream, &mut ctx) {
            Ok(()) => {
                let bytes = stream.into_bytes();
                Ok(PyBytes::new_bound(py, &bytes).into_any().unbind())
            }
            Err(e) => Err(rust_err_to_py(py, e.with_path_prefix(BUILD_PATH))),
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use construct::combined::CombinedConstruct;
    use construct::constructs::format_field::INT8UB;
    use construct::constructs::Struct;
    use construct::value::Value;
    use pyo3::types::PyDict;

    // -- End-to-end: compile + parse_bytes_py (simple struct) ---------------

    #[test]
    fn holder_parse_bytes_py_simple_struct() {
        crate::ensure_python();
        // Struct("magic" / Int8ub, "count" / Int8ub) — 2 bytes.
        let decl: CombinedConstruct = Struct::new()
            .field("magic", Box::new(INT8UB.into()))
            .field("count", Box::new(INT8UB.into()))
            .into();
        let holder = CompiledSchemaHolder::compile(&decl).unwrap();

        Python::with_gil(|py| {
            let result = holder
                .parse_bytes_py(py, &[0xAB, 0x05], IndexMap::new())
                .unwrap();
            let dict = result.bind(py).downcast::<PyDict>().unwrap();
            let magic: i64 = dict.as_any().get_item("magic").unwrap().extract().unwrap();
            let count: i64 = dict.as_any().get_item("count").unwrap().extract().unwrap();
            assert_eq!(magic, 0xAB);
            assert_eq!(count, 5);
        });
    }

    // -- End-to-end: compile + build_from_py (simple struct) ---------------

    #[test]
    fn holder_build_from_py_simple_struct() {
        crate::ensure_python();
        let decl: CombinedConstruct = Struct::new()
            .field("magic", Box::new(INT8UB.into()))
            .field("count", Box::new(INT8UB.into()))
            .into();
        let holder = CompiledSchemaHolder::compile(&decl).unwrap();

        Python::with_gil(|py| {
            let dict = PyDict::new_bound(py);
            dict.set_item("magic", 0xCDi64).unwrap();
            dict.set_item("count", 9i64).unwrap();
            let result = holder
                .build_from_py(py, dict.as_any(), IndexMap::new())
                .unwrap();
            let bytes: Vec<u8> = result.extract(py).unwrap();
            assert_eq!(bytes, vec![0xCD, 0x09]);
        });
    }

    // -- Round-trip: build then parse ---------------------------------------

    #[test]
    fn holder_build_then_parse_roundtrip() {
        crate::ensure_python();
        let decl: CombinedConstruct = Struct::new()
            .field("magic", Box::new(INT8UB.into()))
            .field("count", Box::new(INT8UB.into()))
            .into();
        let holder = CompiledSchemaHolder::compile(&decl).unwrap();

        Python::with_gil(|py| {
            // Build.
            let dict = PyDict::new_bound(py);
            dict.set_item("magic", 0x12i64).unwrap();
            dict.set_item("count", 0x34i64).unwrap();
            let built = holder
                .build_from_py(py, dict.as_any(), IndexMap::new())
                .unwrap();
            let bytes: Vec<u8> = built.extract(py).unwrap();
            assert_eq!(bytes, vec![0x12, 0x34]);

            // Parse back.
            let parsed = holder.parse_bytes_py(py, &bytes, IndexMap::new()).unwrap();
            let pdict = parsed.bind(py).downcast::<PyDict>().unwrap();
            let magic: i64 = pdict.as_any().get_item("magic").unwrap().extract().unwrap();
            let count: i64 = pdict.as_any().get_item("count").unwrap().extract().unwrap();
            assert_eq!(magic, 0x12);
            assert_eq!(count, 0x34);
        });
    }

    // -- Parse error propagates as exception --------------------------------

    #[test]
    fn holder_parse_error_raises_exception() {
        crate::ensure_python();
        // Struct with one Int8ub field; truncated input (0 bytes, needs 1).
        let decl: CombinedConstruct = Struct::new().field("magic", Box::new(INT8UB.into())).into();
        let holder = CompiledSchemaHolder::compile(&decl).unwrap();

        Python::with_gil(|py| {
            let err = holder.parse_bytes_py(py, &[], IndexMap::new()).unwrap_err();
            use crate::exceptions::StreamError;
            assert!(err.is_instance_of::<StreamError>(py));
        });
    }

    // -- Build error propagates as exception --------------------------------

    #[test]
    fn holder_build_error_raises_exception() {
        crate::ensure_python();
        // Struct("magic" / Int8ub) — pass a dict with a string value to
        // trigger a build error.
        let decl: CombinedConstruct = Struct::new().field("magic", Box::new(INT8UB.into())).into();
        let holder = CompiledSchemaHolder::compile(&decl).unwrap();

        Python::with_gil(|py| {
            let dict = PyDict::new_bound(py);
            dict.set_item("magic", "not-an-int").unwrap();
            let err = holder
                .build_from_py(py, dict.as_any(), IndexMap::new())
                .unwrap_err();
            use crate::exceptions::ConstructError;
            assert!(err.is_instance_of::<ConstructError>(py));
        });
    }

    // -- Context kw injection -----------------------------------------------

    #[test]
    fn holder_parse_with_contextkw() {
        crate::ensure_python();
        // Use a Struct (the normal top-level construct shape) with one field.
        // kw fields are stored in context; we verify the call succeeds with
        // non-empty kw.
        let decl: CombinedConstruct = Struct::new().field("magic", Box::new(INT8UB.into())).into();
        let holder = CompiledSchemaHolder::compile(&decl).unwrap();

        Python::with_gil(|py| {
            let mut kw = IndexMap::new();
            kw.insert("length".to_string(), Value::UInt(3));
            let result = holder.parse_bytes_py(py, &[0x07], kw).unwrap();
            let dict = result.bind(py).downcast::<PyDict>().unwrap();
            let magic: i64 = dict.as_any().get_item("magic").unwrap().extract().unwrap();
            assert_eq!(magic, 7);
        });
    }

    // -- schema() accessor --------------------------------------------------

    #[test]
    fn holder_schema_accessor_returns_compiled_schema() {
        let decl: CombinedConstruct = Struct::new().field("magic", Box::new(INT8UB.into())).into();
        let holder = CompiledSchemaHolder::compile(&decl).unwrap();
        assert_eq!(holder.schema().static_size(), Some(1));
    }
}
