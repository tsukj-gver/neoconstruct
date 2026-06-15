//! PyO3 wrappers for atomic constructors (FormatField, Bytes, VarInt, etc.).

use construct::constructs::format_field::{Endianness, FormatKind};
use construct::core::Construct;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
// PyO3 types imported where needed

use crate::construct_macros::PyConstructWrapper;
use crate::conversions::py_to_value;
use crate::expr_bridge::{py_param_to_compute_func, py_param_to_evaluate};
use crate::py_adapter::extract_subcon;

// ===========================================================================
// FormatField
// ===========================================================================

/// PyO3 wrapper: fixed-width numeric field.
#[pyclass(name = "FormatField", unsendable)]
pub struct PyFormatField {
    pub(crate) inner: construct::constructs::FormatField,
}

impl PyConstructWrapper for PyFormatField {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

/// Factory: `FormatField(endian, format)`.
#[pyfunction]
#[pyo3(name = "FormatField")]
pub fn py_format_field(endian: &str, format: &str) -> PyResult<PyFormatField> {
    let endianness = match endian {
        "<" => Endianness::Little,
        ">" => Endianness::Big,
        "=" => Endianness::Native,
        _ => {
            return Err(PyValueError::new_err("endian must be '<', '>', or '='"));
        }
    };
    let kind = match format {
        "b" => FormatKind::I8,
        "B" => FormatKind::U8,
        "?" => FormatKind::Bool,
        "h" => FormatKind::I16,
        "H" => FormatKind::U16,
        "i" | "l" => FormatKind::I32,
        "I" | "L" => FormatKind::U32,
        "q" => FormatKind::I64,
        "Q" => FormatKind::U64,
        "e" => FormatKind::F16,
        "f" => FormatKind::F32,
        "d" => FormatKind::F64,
        _ => {
            return Err(PyValueError::new_err(
                "format must be one of: b B h H i I l L q Q e f d",
            ));
        }
    };
    Ok(PyFormatField {
        inner: construct::constructs::FormatField::new(endianness, kind),
    })
}

crate::impl_api_methods!(PyFormatField);
crate::impl_construct_operators!(PyFormatField);
crate::impl_default_repr!(PyFormatField, "FormatField");

// ===========================================================================
// Bytes / GreedyBytes
// ===========================================================================

/// PyO3 wrapper: fixed or dynamic length bytes field.
#[pyclass(name = "Bytes", unsendable)]
pub struct PyBytes {
    pub(crate) inner: Box<dyn Construct>,
}

impl PyConstructWrapper for PyBytes {
    fn as_construct(&self) -> &dyn Construct {
        &*self.inner
    }
}

impl PyBytes {
    /// Reconstructs an owned `Box<dyn Construct>`.
    /// Returns `None` for dynamic-length Bytes (BytesExpr), causing the
    /// caller to fall through to PyConstructAdapter.
    pub(crate) fn make_owned(&self) -> PyResult<Option<Box<dyn Construct>>> {
        match self.inner.sizeof(&construct::core::context::Context::new()) {
            Ok(len) => Ok(Some(Box::new(construct::constructs::Bytes::new(len)))),
            Err(_) => Ok(None), // Dynamic length — fall through to PyConstructAdapter
        }
    }
}

/// Factory: `Bytes(length)`. Int 鈫?fixed, callable 鈫?dynamic.
#[pyfunction]
#[pyo3(name = "Bytes")]
pub fn py_bytes(length: &Bound<PyAny>) -> PyResult<PyBytes> {
    let py = length.py();
    if length.is_callable() {
        let expr = py_param_to_evaluate(py, length)?;
        Ok(PyBytes {
            inner: Box::new(construct::constructs::BytesExpr::new(expr)),
        })
    } else {
        let n = length.extract::<usize>()?;
        Ok(PyBytes {
            inner: Box::new(construct::constructs::Bytes::new(n)),
        })
    }
}

crate::impl_api_methods!(PyBytes);
crate::impl_construct_operators!(PyBytes);
crate::impl_default_repr!(PyBytes, "Bytes");

/// PyO3 wrapper: greedy bytes (reads to EOF).
#[pyclass(name = "GreedyBytes", unsendable)]
pub struct PyGreedyBytes {
    pub(crate) inner: construct::constructs::GreedyBytes,
}

impl PyConstructWrapper for PyGreedyBytes {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

/// Factory: `GreedyBytes()`.
#[pyfunction]
#[pyo3(name = "GreedyBytes")]
pub fn py_greedy_bytes() -> PyGreedyBytes {
    PyGreedyBytes {
        inner: construct::constructs::GreedyBytes::new(),
    }
}

crate::impl_api_methods!(PyGreedyBytes);
crate::impl_construct_operators!(PyGreedyBytes);
crate::impl_default_repr!(PyGreedyBytes, "GreedyBytes");

// ===========================================================================
// BytesInteger / BitsInteger
// ===========================================================================

/// PyO3 wrapper: integer represented as N bytes.
#[pyclass(name = "BytesInteger", unsendable)]
pub struct PyBytesInteger {
    pub(crate) inner: construct::constructs::BytesInteger,
}

impl PyConstructWrapper for PyBytesInteger {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

/// Factory: `BytesInteger(length, signed=False, swapped=False)`.
/// Returns a Python fallback when `swapped` is not a plain bool.
#[pyfunction]
#[pyo3(name = "BytesInteger", signature = (length, signed=false, swapped=None))]
pub fn py_bytes_integer(
    py: Python<'_>,
    length: &Bound<PyAny>,
    signed: bool,
    swapped: Option<&Bound<PyAny>>,
) -> PyResult<PyObject> {
    let swapped_val: Bound<PyAny> = match swapped {
        Some(s) => s.clone(),
        None => false.to_object(py).into_bound(py),
    };
    // If both length and swapped are plain types, use Rust-backed.
    let is_length_int = length.is_instance_of::<pyo3::types::PyInt>();
    let is_swapped_bool = swapped_val.is_instance_of::<pyo3::types::PyBool>();

    if is_length_int && is_swapped_bool {
        let len_val: i64 = length.extract()?;
        let swp: bool = swapped_val.extract()?;
        if len_val <= 0 {
            // Negative/zero length → always route to Python fallback which raises IntegerError
        } else {
            return Ok(Py::new(
                py,
                PyBytesInteger {
                    inner: construct::constructs::BytesInteger::new(len_val as usize, signed, swp),
                },
            )?
            .into_any());
        }
    }
    // Fallback to Python implementation
    let module = py.import_bound("construct_rust._integer")?;
    let fallback = module.getattr("_bytes_integer_fallback")?;
    let result = fallback.call1((length.clone(), signed, swapped_val.clone()))?;
    Ok(result.unbind())
}

crate::impl_api_methods!(PyBytesInteger);
crate::impl_construct_operators!(PyBytesInteger);
crate::impl_default_repr!(PyBytesInteger, "BytesInteger");

/// PyO3 wrapper: integer represented as N bits.
#[pyclass(name = "BitsInteger", unsendable)]
pub struct PyBitsInteger {
    pub(crate) inner: construct::constructs::BitsInteger,
}

impl PyConstructWrapper for PyBitsInteger {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

/// Factory: `BitsInteger(length, signed=False, swapped=False)`.
/// Returns a Python fallback when `swapped` or `length` is dynamic.
#[pyfunction]
#[pyo3(name = "BitsInteger", signature = (length, signed=false, swapped=None))]
pub fn py_bits_integer(
    py: Python<'_>,
    length: &Bound<PyAny>,
    signed: bool,
    swapped: Option<&Bound<PyAny>>,
) -> PyResult<PyObject> {
    let swapped_val: Bound<PyAny> = match swapped {
        Some(s) => s.clone(),
        None => false.to_object(py).into_bound(py),
    };
    let is_length_int = length.is_instance_of::<pyo3::types::PyInt>();
    let is_swapped_bool = swapped_val.is_instance_of::<pyo3::types::PyBool>();

    if is_length_int && is_swapped_bool {
        let len_val: i64 = length.extract()?;
        let swp: bool = swapped_val.extract()?;
        if len_val <= 0 {
            // Negative/zero length → always route to Python fallback which raises IntegerError
        } else {
            return Ok(Py::new(
                py,
                PyBitsInteger {
                    inner: construct::constructs::BitsInteger::new(len_val as usize, signed, swp),
                },
            )?
            .into_any());
        }
    }
    // Fallback to Python implementation
    let module = py.import_bound("construct_rust._integer")?;
    let fallback = module.getattr("_bits_integer_fallback")?;
    let result = fallback.call1((length.clone(), signed, swapped_val.clone()))?;
    Ok(result.unbind())
}

crate::impl_api_methods!(PyBitsInteger);
crate::impl_construct_operators!(PyBitsInteger);
crate::impl_default_repr!(PyBitsInteger, "BitsInteger");

// ===========================================================================
// VarInt / ZigZag
// ===========================================================================

/// PyO3 wrapper: variable-length unsigned integer.
#[pyclass(name = "VarInt", unsendable)]
pub struct PyVarInt {
    pub(crate) inner: construct::constructs::VarInt,
}

impl PyConstructWrapper for PyVarInt {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

/// Factory: `VarInt()`.
#[pyfunction]
#[pyo3(name = "VarInt")]
pub fn py_varint() -> PyVarInt {
    PyVarInt {
        inner: construct::constructs::VarInt::new(),
    }
}

crate::impl_api_methods!(PyVarInt);
crate::impl_construct_operators!(PyVarInt);
crate::impl_default_repr!(PyVarInt, "VarInt");

/// PyO3 wrapper: zigzag-encoded signed integer.
#[pyclass(name = "ZigZag", unsendable)]
pub struct PyZigZag {
    pub(crate) inner: construct::constructs::ZigZag,
}

impl PyConstructWrapper for PyZigZag {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

/// Factory: `ZigZag()`.
#[pyfunction]
#[pyo3(name = "ZigZag")]
pub fn py_zigzag() -> PyZigZag {
    PyZigZag {
        inner: construct::constructs::ZigZag::new(),
    }
}

crate::impl_api_methods!(PyZigZag);
crate::impl_construct_operators!(PyZigZag);
crate::impl_default_repr!(PyZigZag, "ZigZag");

// ===========================================================================
// Flag
// ===========================================================================

/// PyO3 wrapper: boolean flag (1 byte, nonzero = True).
#[pyclass(name = "Flag", unsendable)]
pub struct PyFlag {
    pub(crate) inner: construct::constructs::Flag,
}

impl PyConstructWrapper for PyFlag {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

/// Factory: `Flag()`.
#[pyfunction]
#[pyo3(name = "Flag")]
pub fn py_flag() -> PyFlag {
    PyFlag {
        inner: construct::constructs::Flag::new(),
    }
}

crate::impl_api_methods!(PyFlag);
crate::impl_construct_operators!(PyFlag);
crate::impl_default_repr!(PyFlag, "Flag");
// ===========================================================================
// Const
// ===========================================================================

/// PyO3 wrapper: constant value field.
#[pyclass(name = "Const", unsendable)]
pub struct PyConst {
    pub(crate) inner: construct::constructs::Const,
}

impl PyConstructWrapper for PyConst {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

/// Factory: `Const(data, subcon=None)`.
#[pyfunction]
#[pyo3(name = "Const", signature = (data, subcon=None))]
pub fn py_const(
    py: Python<'_>,
    data: &Bound<PyAny>,
    subcon: Option<&Bound<PyAny>>,
) -> PyResult<PyConst> {
    let value = py_to_value(py, data)?;
    if let Some(sc) = subcon {
        let inner = extract_subcon(sc)?;
        Ok(PyConst {
            inner: construct::constructs::Const::new_value(value, inner),
        })
    } else {
        match &value {
            construct::value::Value::Bytes(b) => Ok(PyConst {
                inner: construct::constructs::Const::new_bytes(b.clone()),
            }),
            _ => Err(crate::exceptions::StringError::new_err(
                "given non-bytes value, perhaps unicode?",
            )),
        }
    }
}

crate::impl_api_methods!(PyConst);
crate::impl_construct_operators!(PyConst);
crate::impl_default_repr!(PyConst, "Const");

// ===========================================================================
// Pass / Terminated / Tell / Error
// ===========================================================================

macro_rules! impl_unit_wrapper {
    ($pyclass:ident, $inner_ty:ty, $name_:literal, $factory:ident, $new_expr:expr) => {
        #[pyclass(name = $name_)]
        pub struct $pyclass {
            pub(crate) inner: $inner_ty,
        }

        impl PyConstructWrapper for $pyclass {
            fn as_construct(&self) -> &dyn Construct {
                &self.inner
            }
        }

        #[pyfunction]
        #[pyo3(name = $name_)]
        pub fn $factory() -> $pyclass {
            $pyclass { inner: $new_expr }
        }

        crate::impl_api_methods!($pyclass);
        crate::impl_construct_operators!($pyclass);
        crate::impl_default_repr!($pyclass, $name_);
    };
}

impl_unit_wrapper!(
    PyPass,
    construct::constructs::Pass,
    "Pass",
    py_pass,
    construct::constructs::Pass::new()
);
impl_unit_wrapper!(
    PyTerminated,
    construct::constructs::Terminated,
    "Terminated",
    py_terminated,
    construct::constructs::Terminated::new()
);
impl_unit_wrapper!(
    PyTell,
    construct::constructs::Tell,
    "Tell",
    py_tell,
    construct::constructs::Tell::new()
);
impl_unit_wrapper!(
    PyError,
    construct::constructs::Error,
    "Error",
    py_error,
    construct::constructs::Error::new()
);

// ===========================================================================
// Seek
// ===========================================================================

/// PyO3 wrapper: seek to a position in the stream.
#[pyclass(name = "Seek", unsendable)]
pub struct PySeek {
    pub(crate) inner: Box<dyn Construct>,
    /// Original `at` parameter (for reconstruction in extract_subcon).
    pub(crate) at: i64,
    /// Original `whence` parameter.
    pub(crate) whence: construct::constructs::SeekWhence,
    /// Whether the seek uses a dynamic (callable) expression.
    pub(crate) is_dynamic: bool,
    /// Original Python callable (if dynamic), for reconstruction.
    pub(crate) at_obj: Option<PyObject>,
}

impl PyConstructWrapper for PySeek {
    fn as_construct(&self) -> &dyn Construct {
        &*self.inner
    }
}

impl PySeek {
    /// Reconstructs an owned `Box<dyn Construct>` from stored parameters.
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        if self.is_dynamic {
            if let Some(ref obj) = self.at_obj {
                Python::with_gil(|py| {
                    let expr = crate::expr_bridge::py_param_to_evaluate(py, obj.bind(py))?;
                    Ok(Box::new(construct::constructs::SeekExpr::with_whence(
                        expr,
                        self.whence,
                    )) as Box<dyn Construct>)
                })
            } else {
                Ok(Box::new(construct::constructs::Seek::with_whence(
                    self.at,
                    self.whence,
                )) as Box<dyn Construct>)
            }
        } else {
            Ok(Box::new(construct::constructs::Seek::with_whence(
                self.at,
                self.whence,
            )) as Box<dyn Construct>)
        }
    }
}

fn parse_whence(whence: i32) -> PyResult<construct::constructs::SeekWhence> {
    match whence {
        0 => Ok(construct::constructs::SeekWhence::Start),
        1 => Ok(construct::constructs::SeekWhence::Current),
        2 => Ok(construct::constructs::SeekWhence::End),
        _ => Err(PyValueError::new_err(
            "whence must be 0 (Start), 1 (Current), or 2 (End)",
        )),
    }
}

/// Factory: `Seek(at, whence=0)`. Int 鈫?fixed, callable 鈫?dynamic.
#[pyfunction]
#[pyo3(name = "Seek", signature = (at, whence=0))]
pub fn py_seek(at: &Bound<PyAny>, whence: i32) -> PyResult<PySeek> {
    let py = at.py();
    let sw = parse_whence(whence)?;
    if at.is_callable() {
        let expr = py_param_to_evaluate(py, at)?;
        Ok(PySeek {
            inner: Box::new(construct::constructs::SeekExpr::with_whence(expr, sw)),
            at: 0,
            whence: sw,
            is_dynamic: true,
            at_obj: Some(at.clone().unbind()),
        })
    } else {
        let pos = at.extract::<i64>()?;
        Ok(PySeek {
            inner: Box::new(construct::constructs::Seek::with_whence(pos, sw)),
            at: pos,
            whence: sw,
            is_dynamic: false,
            at_obj: None,
        })
    }
}

crate::impl_api_methods!(PySeek);
crate::impl_construct_operators!(PySeek);
crate::impl_default_repr!(PySeek, "Seek");

// ===========================================================================
// Computed / Rebuild / Default / Index
// ===========================================================================

/// PyO3 wrapper: computed field (no bytes consumed).
#[pyclass(name = "Computed", unsendable)]
pub struct PyComputed {
    pub(crate) inner: construct::constructs::Computed,
    pub(crate) func_obj: PyObject,
}

impl PyConstructWrapper for PyComputed {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyComputed {
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let f = py_param_to_compute_func(py, self.func_obj.bind(py))?;
            let con: Box<dyn Construct> = Box::new(construct::constructs::Computed::new(f));
            Ok(con)
        })
    }
}

/// Factory: `Computed(func)`.
#[pyfunction]
#[pyo3(name = "Computed")]
pub fn py_computed(py: Python<'_>, func: &Bound<PyAny>) -> PyResult<PyComputed> {
    let f = py_param_to_compute_func(py, func)?;
    Ok(PyComputed {
        inner: construct::constructs::Computed::new(f),
        func_obj: func.clone().unbind(),
    })
}

crate::impl_api_methods!(PyComputed);
crate::impl_construct_operators!(PyComputed);
crate::impl_default_repr!(PyComputed, "Computed");

/// PyO3 wrapper: rebuild field (compute on build, parse on parse).
#[pyclass(name = "Rebuild", unsendable)]
pub struct PyRebuild {
    pub(crate) inner: construct::constructs::Rebuild,
    pub(crate) subcon_obj: PyObject,
    pub(crate) func_obj: PyObject,
}

impl PyConstructWrapper for PyRebuild {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyRebuild {
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            let f = py_param_to_compute_func(py, self.func_obj.bind(py))?;
            let con: Box<dyn Construct> = Box::new(construct::constructs::Rebuild::new(sc, f));
            Ok(con)
        })
    }
}

/// Factory: `Rebuild(subcon, func)`.
#[pyfunction]
#[pyo3(name = "Rebuild")]
pub fn py_rebuild(
    py: Python<'_>,
    subcon: &Bound<PyAny>,
    func: &Bound<PyAny>,
) -> PyResult<PyRebuild> {
    let sc = extract_subcon(subcon)?;
    let f = py_param_to_compute_func(py, func)?;
    Ok(PyRebuild {
        inner: construct::constructs::Rebuild::new(sc, f),
        subcon_obj: subcon.clone().unbind(),
        func_obj: func.clone().unbind(),
    })
}

crate::impl_api_methods!(PyRebuild);
crate::impl_construct_operators!(PyRebuild);
crate::impl_default_repr!(PyRebuild, "Rebuild");

/// PyO3 wrapper: default value field.
#[pyclass(name = "Default", unsendable)]
pub struct PyDefault {
    pub(crate) inner: construct::constructs::Default,
    pub(crate) subcon_obj: PyObject,
}

impl PyConstructWrapper for PyDefault {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

/// Factory: `Default(subcon, value)`.
#[pyfunction]
#[pyo3(name = "Default")]
pub fn py_default(
    py: Python<'_>,
    subcon: &Bound<PyAny>,
    value: &Bound<PyAny>,
) -> PyResult<PyDefault> {
    let sc = extract_subcon(subcon)?;
    let v = py_to_value(py, value)?;
    Ok(PyDefault {
        inner: construct::constructs::Default::new(sc, v),
        subcon_obj: subcon.clone().unbind(),
    })
}

crate::impl_api_methods!(PyDefault);
crate::impl_construct_operators!(PyDefault);
crate::impl_subcon_repr!(PyDefault, "Default");

/// PyO3 wrapper: current array index.
#[pyclass(name = "Index", unsendable)]
pub struct PyIndex {
    pub(crate) inner: construct::constructs::Index,
}

impl PyConstructWrapper for PyIndex {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

/// Factory: `Index()`.
#[pyfunction]
#[pyo3(name = "Index")]
pub fn py_index() -> PyIndex {
    PyIndex {
        inner: construct::constructs::Index::new(),
    }
}

crate::impl_api_methods!(PyIndex);
crate::impl_construct_operators!(PyIndex);
crate::impl_default_repr!(PyIndex, "Index");
