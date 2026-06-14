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

/// Factory: `Bytes(length)`. Int → fixed, callable → dynamic.
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
#[pyfunction]
#[pyo3(name = "BytesInteger", signature = (length, signed=false, swapped=false))]
pub fn py_bytes_integer(length: usize, signed: bool, swapped: bool) -> PyBytesInteger {
    PyBytesInteger {
        inner: construct::constructs::BytesInteger::new(length, signed, swapped),
    }
}

crate::impl_api_methods!(PyBytesInteger);
crate::impl_construct_operators!(PyBytesInteger);

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
#[pyfunction]
#[pyo3(name = "BitsInteger", signature = (length, signed=false, swapped=false))]
pub fn py_bits_integer(length: usize, signed: bool, swapped: bool) -> PyBitsInteger {
    PyBitsInteger {
        inner: construct::constructs::BitsInteger::new(length, signed, swapped),
    }
}

crate::impl_api_methods!(PyBitsInteger);
crate::impl_construct_operators!(PyBitsInteger);

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
            _ => Err(pyo3::exceptions::PyTypeError::new_err(
                "Const requires a subcon when data is not bytes",
            )),
        }
    }
}

crate::impl_api_methods!(PyConst);
crate::impl_construct_operators!(PyConst);

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
}

impl PyConstructWrapper for PySeek {
    fn as_construct(&self) -> &dyn Construct {
        &*self.inner
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

/// Factory: `Seek(at, whence=0)`. Int → fixed, callable → dynamic.
#[pyfunction]
#[pyo3(name = "Seek", signature = (at, whence=0))]
pub fn py_seek(at: &Bound<PyAny>, whence: i32) -> PyResult<PySeek> {
    let py = at.py();
    let sw = parse_whence(whence)?;
    if at.is_callable() {
        let expr = py_param_to_evaluate(py, at)?;
        Ok(PySeek {
            inner: Box::new(construct::constructs::SeekExpr::with_whence(expr, sw)),
        })
    } else {
        let pos = at.extract::<i64>()?;
        Ok(PySeek {
            inner: Box::new(construct::constructs::Seek::with_whence(pos, sw)),
        })
    }
}

crate::impl_api_methods!(PySeek);
crate::impl_construct_operators!(PySeek);

// ===========================================================================
// Computed / Rebuild / Default / Index
// ===========================================================================

/// PyO3 wrapper: computed field (no bytes consumed).
#[pyclass(name = "Computed", unsendable)]
pub struct PyComputed {
    pub(crate) inner: construct::constructs::Computed,
}

impl PyConstructWrapper for PyComputed {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

/// Factory: `Computed(func)`.
#[pyfunction]
#[pyo3(name = "Computed")]
pub fn py_computed(py: Python<'_>, func: &Bound<PyAny>) -> PyResult<PyComputed> {
    let f = py_param_to_compute_func(py, func)?;
    Ok(PyComputed {
        inner: construct::constructs::Computed::new(f),
    })
}

crate::impl_api_methods!(PyComputed);
crate::impl_construct_operators!(PyComputed);

/// PyO3 wrapper: rebuild field (compute on build, parse on parse).
#[pyclass(name = "Rebuild", unsendable)]
pub struct PyRebuild {
    pub(crate) inner: construct::constructs::Rebuild,
}

impl PyConstructWrapper for PyRebuild {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
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
    })
}

crate::impl_api_methods!(PyRebuild);
crate::impl_construct_operators!(PyRebuild);

/// PyO3 wrapper: default value field.
#[pyclass(name = "Default", unsendable)]
pub struct PyDefault {
    pub(crate) inner: construct::constructs::Default,
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
    })
}

crate::impl_api_methods!(PyDefault);
crate::impl_construct_operators!(PyDefault);

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
