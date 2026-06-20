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
//! - [`CompiledSchemaHolder::parse_bytes_raw`] writes parse results straight
//!   into a `PyDict` / `PyList` via [`PyDictSink`] — no `Value::Container`
//!   intermediate tree is built (design S-ARCH-2). The Python-facing wrapper
//!   [`CompiledSchemaHolder::parse_bytes_py`] (a `#[pymethod]`) converts
//!   `**kwargs` into an [`IndexMap`] and delegates here.
//! - [`CompiledSchemaHolder::build_from_raw`] reads build input lazily via
//!   [`PyDictInput`] — only fields actually consumed by `exec_build_py` incur FFI.
//!   The Python-facing wrapper [`CompiledSchemaHolder::build_from_py`] (a
//!   `#[pymethod]`) delegates here.

use std::sync::Arc;

use construct::combined::CombinedConstruct;
use construct::compiled::py_exec::{exec_build_py_dispatch, exec_parse_py_dispatch};
use construct::compiled::py_input::{PyDictInput, PyInput};
use construct::compiled::py_sink::{PyDictSink2, PySink};
use construct::compiled::CompiledSchema;
use construct::compiler::SchemaCompiler;
use construct::core::context::Context;
use construct::core::stream::{ByteStream, CombinedStream};
use construct::value::Value;

use indexmap::IndexMap;
use pyo3::prelude::*;
use pyo3::types::PyBytes;

use crate::exceptions::rust_err_to_py;

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
///
/// # PyO3 exposure (Phase 14)
///
/// Registered as `#[pyclass(name = "CompiledSchemaHolder", unsendable)]`.
/// The `unsendable` marker is required because `CompiledSchema` is not
/// `Send + Sync` (it may contain `Arc<dyn Construct>` via the `Dynamic`
/// escape-hatch). Python code holds instances behind the GIL only.
#[pyclass(name = "CompiledSchemaHolder", unsendable)]
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
    /// This is the Rust-facing entry point. The Python-facing wrapper
    /// [`parse_bytes_py`](Self::parse_bytes_py) (a `#[pymethod]`) converts
    /// `**kwargs` into an [`IndexMap`] and delegates here.
    ///
    /// # Errors
    ///
    /// Returns `PyErr` (a construct exception subclass) on parse failure.
    /// On error, the partially-built `PyDict` is **dropped** (discarded) —
    /// the Python caller receives only the exception (design §8, REV #9
    /// scheme A).
    pub fn parse_bytes_raw(
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
        // Phase 16.3: direct-to-Python parse path via PyDictSink2.
        // The compiled tree is traversed in Rust while holding the GIL,
        // writing parse results straight into PyDict / PyList — no
        // Value::Container intermediate tree is built (design S-ARCH-2).
        let mut sink = PyDictSink2::new_root_struct(py);
        match exec_parse_py_dispatch(self.schema.tree(), py, &mut stream, &mut ctx, &mut sink) {
            Ok(()) => Box::new(sink)
                .into_root_py(py)
                .map_err(|e| rust_err_to_py(py, e.with_path_prefix(PARSE_PATH))),
            Err(e) => Err(rust_err_to_py(py, e.with_path_prefix(PARSE_PATH))),
        }
        // Err path: `sink` (PyDictSink2 owning a Py<PyDict>) is dropped here.
        // PyO3 decrements the refcount; Python GC reclaims the partial dict.
        // Python only sees the exception.
    }

    /// Builds binary data from a Python object, returning it as `bytes`.
    ///
    /// **Python-direct path** (Phase 16.4): the entire compiled tree is
    /// traversed in Rust while holding the GIL. Field values are read lazily
    /// from the Python input object via [`PyDictInput`] — only fields
    /// actually consumed by `exec_build_py` incur FFI, not the entire object.
    ///
    /// `kw` provides top-level context fields (matching `**contextkw`).
    ///
    /// This is the Rust-facing entry point. The Python-facing wrapper
    /// [`build_from_py`](Self::build_from_py) (a `#[pymethod]`) converts
    /// `**kwargs` into an [`IndexMap`] and delegates here.
    ///
    /// # Errors
    ///
    /// Returns `PyErr` on build failure.
    pub fn build_from_raw(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        kw: IndexMap<String, Value>,
    ) -> PyResult<PyObject> {
        let input = PyDictInput::from_bound(obj);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        ctx.insert("_parsing", Value::Bool(false));
        ctx.insert("_building", Value::Bool(true));
        ctx.insert("_sizing", Value::Bool(false));
        // Embed the input as a shallow Value in context (context._ = obj),
        // matching the Python original so condition-based constructs (Optional,
        // IfThenElse) can inspect the value being built via `ctx.get("_")`.
        // Uses a shallow conversion (top-level scalars only; nested composites
        // become empty placeholders) to avoid full-tree conversion overhead.
        if let Ok(embed) = input.extract_ctx_value_py(py) {
            ctx.insert("_", embed);
        }
        for (key, value) in kw {
            ctx.insert(key, value);
        }
        match exec_build_py_dispatch(self.schema.tree(), py, &input, &mut stream, &mut ctx) {
            Ok(()) => {
                let bytes = stream.into_bytes();
                Ok(PyBytes::new_bound(py, &bytes).into_any().unbind())
            }
            Err(e) => Err(rust_err_to_py(py, e.with_path_prefix(BUILD_PATH))),
        }
    }
}

// ===========================================================================
// PyO3 exposure (Phase 14): Python-facing methods on CompiledSchemaHolder
// ===========================================================================
//
// These `#[pymethod]` wrappers accept Python `**kwargs` and convert them to
// `IndexMap<String, Value>` via [`crate::api::opt_kw_to_indexmap`], then
// delegate to the Rust-facing `*_raw` methods above.
//
// This separation is needed because:
// 1. The `*_raw` methods are called from Rust code (`api::py_parse_compiled`
//    etc.) with a pre-built `IndexMap` — no Python dict conversion needed.
// 2. The Python-facing methods must accept `**kwargs` (Python convention).
// 3. Rust disallows two methods with the same name in different `impl` blocks.

#[pymethods]
impl CompiledSchemaHolder {
    /// Parses binary data into a Python dict (direct-to-Python path).
    ///
    /// Python signature: ``parse_bytes_py(data, **kw)``.
    ///
    /// `data` must be a `bytes`-like object. `**kw` provides top-level
    /// context fields (matching the ``**contextkw`` convention).
    ///
    /// # Errors
    ///
    /// Returns a construct exception subclass on parse failure.
    #[pyo3(signature = (data, **kw))]
    pub fn parse_bytes_py(
        &self,
        py: Python<'_>,
        data: &Bound<'_, PyAny>,
        kw: Option<&Bound<'_, pyo3::types::PyDict>>,
    ) -> PyResult<PyObject> {
        let bytes: std::borrow::Cow<[u8]> = data.extract()?;
        let ctx = crate::api::opt_kw_to_indexmap(py, kw)?;
        self.parse_bytes_raw(py, &bytes, ctx)
    }

    /// Builds binary data from a Python object, returning ``bytes``.
    ///
    /// Python signature: ``build_from_py(obj, **kw)``.
    ///
    /// `obj` is typically a dict or a dataclass instance; [`PyDictInput`] reads
    /// its attributes lazily. `**kw` provides top-level context fields.
    ///
    /// # Errors
    ///
    /// Returns a construct exception subclass on build failure.
    #[pyo3(signature = (obj, **kw))]
    pub fn build_from_py(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        kw: Option<&Bound<'_, pyo3::types::PyDict>>,
    ) -> PyResult<PyObject> {
        let ctx = crate::api::opt_kw_to_indexmap(py, kw)?;
        self.build_from_raw(py, obj, ctx)
    }

    /// Returns the static byte size of the compiled schema, or ``None`` if
    /// the size is not statically known.
    ///
    /// Python signature: ``sizeof(**kw)``.
    ///
    /// Note: ``**kw`` is accepted for API consistency but currently ignored —
    /// the compiled schema's static size is computed at compile time. Future
    /// extensions may use ``kw`` for context-parameterized sizes.
    #[pyo3(signature = (**kw))]
    #[allow(unused_variables)]
    pub fn sizeof(
        &self,
        py: Python<'_>,
        kw: Option<&Bound<'_, pyo3::types::PyDict>>,
    ) -> PyResult<Option<usize>> {
        // kw is accepted for forward compatibility but not currently used:
        // CompiledSchema::static_size is a compile-time-folded constant.
        Ok(self.schema.static_size())
    }
}

// ===========================================================================
// compile_schema pyfunction (Phase 14)
// ===========================================================================

/// Compiles a construct declaration into a [`CompiledSchemaHolder`].
///
/// Python signature: ``compile_schema(struct_decl)``.
///
/// `struct_decl` is typically a `Struct` instance (from `Struct(**kwargs)`),
/// but any construct-like Python object is accepted. The declaration tree is
/// traversed and compiled into a [`CompiledSchema`] (Phase 12/13), then
/// wrapped in a [`CompiledSchemaHolder`] for direct-to-Python parse/build.
///
/// # PyStruct handling
///
/// When `struct_decl` is a [`PyStruct`](crate::constructs_composite::PyStruct),
/// the inner `Struct` is reconstructed from `py_subcons` to produce an owned
/// `CombinedConstruct::Struct` (preserving static dispatch in the compiler).
/// This avoids the need for `Struct` to be `Clone`.
///
/// # Errors
///
/// Returns a construct exception subclass if compilation fails (e.g. an
/// uncompilable construct variant, or `struct_decl` is not a construct).
#[pyfunction]
#[pyo3(name = "compile_schema")]
pub fn py_compile_schema(
    py: Python<'_>,
    struct_decl: &Bound<'_, PyAny>,
) -> PyResult<Py<CompiledSchemaHolder>> {
    // Common case: ConstructMixin._compile passes a PyStruct built via
    // Struct(**kwargs). Reconstruct from py_subcons to get an owned
    // CombinedConstruct::Struct (preserving the Struct variant for static
    // dispatch in SchemaCompiler).
    if let Ok(s) = struct_decl.extract::<pyo3::PyRef<crate::constructs_composite::PyStruct>>() {
        let combined = s.make_combined(py)?;
        let holder = CompiledSchemaHolder::compile(&combined)?;
        return Py::new(py, holder);
    }

    // Fallback: any construct-like Python object via extract_subcon. This
    // produces a CombinedConstruct::Dynamic wrapper (still compilable, but
    // with dynamic dispatch at the root).
    let combined = crate::py_adapter::extract_subcon(struct_decl)?;
    let holder = CompiledSchemaHolder::compile(&combined)?;
    Py::new(py, holder)
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

    // -- End-to-end: compile + parse_bytes_raw (simple struct) ---------------

    #[test]
    fn holder_parse_bytes_raw_simple_struct() {
        crate::ensure_python();
        // Struct("magic" / Int8ub, "count" / Int8ub) — 2 bytes.
        let decl: CombinedConstruct = Struct::new()
            .field("magic", Box::new(INT8UB.into()))
            .field("count", Box::new(INT8UB.into()))
            .into();
        let holder = CompiledSchemaHolder::compile(&decl).unwrap();

        Python::with_gil(|py| {
            let result = holder
                .parse_bytes_raw(py, &[0xAB, 0x05], IndexMap::new())
                .unwrap();
            let dict = result.bind(py).downcast::<PyDict>().unwrap();
            let magic: i64 = dict.as_any().get_item("magic").unwrap().extract().unwrap();
            let count: i64 = dict.as_any().get_item("count").unwrap().extract().unwrap();
            assert_eq!(magic, 0xAB);
            assert_eq!(count, 5);
        });
    }

    // -- End-to-end: compile + build_from_raw (simple struct) ---------------

    #[test]
    fn holder_build_from_raw_simple_struct() {
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
                .build_from_raw(py, dict.as_any(), IndexMap::new())
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
                .build_from_raw(py, dict.as_any(), IndexMap::new())
                .unwrap();
            let bytes: Vec<u8> = built.extract(py).unwrap();
            assert_eq!(bytes, vec![0x12, 0x34]);

            // Parse back.
            let parsed = holder.parse_bytes_raw(py, &bytes, IndexMap::new()).unwrap();
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
            let err = holder
                .parse_bytes_raw(py, &[], IndexMap::new())
                .unwrap_err();
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
                .build_from_raw(py, dict.as_any(), IndexMap::new())
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
            let result = holder.parse_bytes_raw(py, &[0x07], kw).unwrap();
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

    // =======================================================================
    // Phase 14.1: Rust exposure layer tests
    // =======================================================================
    //
    // These tests verify the `#[pyclass]` / `#[pymethods]` / `#[pyfunction]`
    // wrappers directly from Rust, without importing the Python package.
    // End-to-end ConstructMixin tests live in `tests/test_dataclass_api.py`
    // (run via pytest), which avoids the unsendable-pyclass cross-thread
    // limitation of `cargo test`.

    // -- compile_schema pyfunction: PyStruct → CompiledSchemaHolder ---------

    #[test]
    fn pyfunction_compile_schema_from_pystruct() {
        crate::ensure_python();
        Python::with_gil(|py| {
            use crate::constructs_composite::{py_struct, PyStruct};
            use pyo3::types::PyDict;

            // Build Struct(a=Int8ub) via the py_struct factory.
            let kwargs = PyDict::new_bound(py);
            // Create Int8ub as a PyFormatField pyclass instance.
            let int8ub = crate::constructs_atomic::py_format_field(">", "B").unwrap();
            let int8ub_obj = Py::new(py, int8ub).unwrap();
            kwargs.set_item("a", int8ub_obj.bind(py)).unwrap();
            let py_struct: PyStruct =
                py_struct(py, &pyo3::types::PyTuple::empty_bound(py), Some(&kwargs)).unwrap();

            // Convert PyStruct → CombinedConstruct via make_combined.
            let combined = py_struct.make_combined(py).unwrap();
            let holder = CompiledSchemaHolder::compile(&combined).unwrap();

            // Verify the holder can parse.
            let result = holder
                .parse_bytes_raw(py, &[0x42], IndexMap::new())
                .unwrap();
            let dict = result.bind(py).downcast::<PyDict>().unwrap();
            let a: i64 = dict.as_any().get_item("a").unwrap().extract().unwrap();
            assert_eq!(a, 0x42);
        });
    }

    // -- compile_schema pyfunction: fallback via extract_subcon ------------

    #[test]
    fn pyfunction_compile_schema_fallback_extract_subcon() {
        crate::ensure_python();
        Python::with_gil(|py| {
            use crate::constructs_atomic::py_format_field;
            use pyo3::types::PyModule;

            // Register _core module so compile_schema pyfunction is accessible.
            let m = PyModule::new_bound(py, "test_fallback").unwrap();
            crate::_core(py, &m).unwrap();

            // Get compile_schema function.
            let compile_fn = m.getattr("compile_schema").unwrap();

            // Create Int8ub as a Python object.
            let int8ub = py_format_field(">", "B").unwrap();
            let int8ub_obj = Py::new(py, int8ub).unwrap();

            // Call compile_schema(int8ub) — should use extract_subcon fallback.
            // This produces a Dynamic root (still compilable).
            let holder = compile_fn.call1((int8ub_obj.bind(py),)).unwrap();
            assert!(!holder.is_none());
        });
    }

    // -- sizeof pymethod: returns Option<usize> ----------------------------

    #[test]
    fn pymethod_sizeof_returns_static_size() {
        crate::ensure_python();
        // Direct CombinedConstruct (not via extract_subcon) → preserves
        // FormatField variant → static_size is computable.
        let decl: CombinedConstruct = Struct::new()
            .field("a", Box::new(INT8UB.into()))
            .field("b", Box::new(INT8UB.into()))
            .into();
        let holder = CompiledSchemaHolder::compile(&decl).unwrap();

        Python::with_gil(|py| {
            // Call sizeof via the pymethod signature (bypassing Python import).
            // sizeof accepts Option<&Bound<PyDict>> for **kw.
            let size = holder.sizeof(py, None).unwrap();
            assert_eq!(size, Some(2));
        });
    }

    // -- sizeof pymethod: returns None for Dynamic -------------------------

    #[test]
    fn pymethod_sizeof_none_for_dynamic_schema() {
        crate::ensure_python();
        Python::with_gil(|py| {
            use crate::constructs_composite::{py_struct, PyStruct};
            use pyo3::types::PyDict;

            // Build Struct(a=Int8ub) via factory — extract_subcon wraps as Dynamic.
            let kwargs = PyDict::new_bound(py);
            let int8ub = crate::constructs_atomic::py_format_field(">", "B").unwrap();
            let int8ub_obj = Py::new(py, int8ub).unwrap();
            kwargs.set_item("a", int8ub_obj.bind(py)).unwrap();
            let py_struct: PyStruct =
                py_struct(py, &pyo3::types::PyTuple::empty_bound(py), Some(&kwargs)).unwrap();

            let combined = py_struct.make_combined(py).unwrap();
            let holder = CompiledSchemaHolder::compile(&combined).unwrap();

            // Dynamic fields → static_size returns None.
            let size = holder.sizeof(py, None).unwrap();
            assert_eq!(size, None);
        });
    }

    // -- make_combined preserves Struct variant -----------------------------

    #[test]
    fn pystruct_make_combined_produces_struct_variant() {
        crate::ensure_python();
        Python::with_gil(|py| {
            use crate::constructs_composite::{py_struct, PyStruct};
            use pyo3::types::PyDict;

            let kwargs = PyDict::new_bound(py);
            let int8ub = crate::constructs_atomic::py_format_field(">", "B").unwrap();
            let int8ub_obj = Py::new(py, int8ub).unwrap();
            kwargs.set_item("a", int8ub_obj.bind(py)).unwrap();
            let py_struct: PyStruct =
                py_struct(py, &pyo3::types::PyTuple::empty_bound(py), Some(&kwargs)).unwrap();

            let combined = py_struct.make_combined(py).unwrap();
            // Verify it's a Struct variant (not Dynamic at the root).
            assert!(
                matches!(*combined, CombinedConstruct::Struct(_)),
                "make_combined should produce CombinedConstruct::Struct"
            );
        });
    }
}
