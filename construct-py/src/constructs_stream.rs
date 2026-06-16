//! PyO3 wrappers for stream-operation, tunnel, and lazy constructs (10.8).
//!
//! This module provides `#[pyclass]` wrappers and `#[pyfunction]` factories
//! for the Rust-backed stream-operation/lazy constructs:
//!
//! - Bitwise / Bytewise / Pointer / Peek / RawCopy / Prefixed
//! - Transformed / Restreamed / Compressed / Checksum
//! - ByteSwapped / BitsSwapped / FixedSized
//! - Lazy / LazyStruct / LazyArray / LazyBound / Rebuffered
//!
//! Plus functional factories: BitStruct / PrefixedArray.
//!
//! # Clone strategy (PM decision — 方案 B)
//!
//! Each wrapper that holds closures (Transformed / Restreamed / Checksum /
//! LazyBound) stores the original Python callable objects (`PyObject`)
//! alongside the inner Rust construct. When [`extract_subcon`] needs an owned
//! `Box<dyn Construct>`, the wrapper's `make_owned` method re-bridges the
//! closures from the stored PyObjects.

use construct::combined::CombinedConstruct;
use construct::core::context::Context;
use construct::core::stream::CombinedStream;
use construct::core::Construct;
use construct::value::Value;

use pyo3::exceptions::{PyNotImplementedError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};

use crate::construct_macros::PyConstructWrapper;
use crate::expr_bridge::{
    py_param_to_evaluate, py_to_checksum_bytes_func, py_to_checksum_func, py_to_size_computer,
    py_to_transform_func,
};
use crate::py_adapter::extract_subcon;

/// Boxes a concrete construct into `Box<dyn Construct>`.
///
/// Rust does not auto-coerce `Box<Concrete>` to `Box<dyn Construct>` inside
/// `Ok(...)`; this helper provides the explicit coercion point.
fn boxed<C: Construct + 'static>(c: C) -> Box<dyn Construct> {
    Box::new(c)
}

/// A construct that always fails with a given error message.
///
/// Used by [`PyLazyBound`] as a fallback when the bound closure fails to
/// resolve the subcon. Since the closure return type is `Box<dyn Construct>`
/// (not `Result`), errors cannot be propagated via `?`. Instead, this
/// construct carries the original error description so that the failure is
/// diagnosable when parse/build/sizeof is eventually called.
struct ErrorWithMessage {
    message: String,
}

impl ErrorWithMessage {
    fn new(message: impl Into<String>) -> Self {
        ErrorWithMessage {
            message: message.into(),
        }
    }
}

impl Construct for ErrorWithMessage {
    fn parse(
        &self,
        _stream: &mut CombinedStream,
        _ctx: &mut Context,
    ) -> construct::core::error::Result<Value> {
        Err(construct::core::error::ConstructError::Check {
            path: String::new(),
            message: self.message.clone(),
        })
    }

    fn build(
        &self,
        _data: &Value,
        _stream: &mut CombinedStream,
        _ctx: &mut Context,
    ) -> construct::core::error::Result<()> {
        Err(construct::core::error::ConstructError::Check {
            path: String::new(),
            message: self.message.clone(),
        })
    }

    fn sizeof(&self, _ctx: &Context) -> construct::core::error::Result<usize> {
        Err(construct::core::error::ConstructError::Sizeof {
            path: String::new(),
            reason: self.message.clone(),
        })
    }
}

// ===========================================================================
// Bitwise / Bytewise
// ===========================================================================

/// PyO3 wrapper: bit-level stream wrapper.
///
/// Corresponds to Python `Bitwise(subcon)`.
#[pyclass(name = "Bitwise", unsendable)]
pub struct PyBitwise {
    /// The Rust Bitwise construct.
    pub(crate) inner: construct::constructs::stream_ops::Bitwise,
    /// Original subcon Python object (for clone/re-bridge).
    pub(crate) subcon_obj: PyObject,
}

impl PyConstructWrapper for PyBitwise {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyBitwise {
    /// Reconstructs an owned `Box<dyn Construct>` from stored Python objects.
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            Ok(boxed(construct::constructs::stream_ops::Bitwise::new(sc)))
        })
    }
}

/// Factory: `Bitwise(subcon)`.
#[pyfunction]
#[pyo3(name = "Bitwise")]
pub fn py_bitwise(_py: Python<'_>, subcon: &Bound<PyAny>) -> PyResult<PyBitwise> {
    let sc = extract_subcon(subcon)?;
    Ok(PyBitwise {
        inner: construct::constructs::stream_ops::Bitwise::new(sc),
        subcon_obj: subcon.clone().unbind(),
    })
}

crate::impl_api_methods!(PyBitwise);
crate::impl_construct_operators!(PyBitwise);

/// PyO3 wrapper: byte-level stream restorer (used inside Bitwise).
///
/// Corresponds to Python `Bytewise(subcon)`.
#[pyclass(name = "Bytewise", unsendable)]
pub struct PyBytewise {
    /// The Rust Bytewise construct.
    pub(crate) inner: construct::constructs::stream_ops::Bytewise,
    /// Original subcon Python object (for clone/re-bridge).
    pub(crate) subcon_obj: PyObject,
}

impl PyConstructWrapper for PyBytewise {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyBytewise {
    /// Reconstructs an owned `Box<dyn Construct>` from stored Python objects.
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            Ok(boxed(construct::constructs::stream_ops::Bytewise::new(sc)))
        })
    }
}

/// Factory: `Bytewise(subcon)`.
#[pyfunction]
#[pyo3(name = "Bytewise")]
pub fn py_bytewise(_py: Python<'_>, subcon: &Bound<PyAny>) -> PyResult<PyBytewise> {
    let sc = extract_subcon(subcon)?;
    Ok(PyBytewise {
        inner: construct::constructs::stream_ops::Bytewise::new(sc),
        subcon_obj: subcon.clone().unbind(),
    })
}

crate::impl_api_methods!(PyBytewise);
crate::impl_construct_operators!(PyBytewise);

// ===========================================================================
// Pointer
// ===========================================================================

/// PyO3 wrapper: absolute position read/write.
///
/// Corresponds to Python `Pointer(offset, subcon, stream=None, relativeOffset=False)`.
#[pyclass(name = "Pointer", unsendable)]
pub struct PyPointer {
    /// The inner Rust construct (Pointer or PointerExpr).
    pub(crate) inner: Box<dyn Construct>,
    /// Original offset Python object (int or callable).
    pub(crate) offset_obj: PyObject,
    /// Original subcon Python object.
    pub(crate) subcon_obj: PyObject,
}

impl PyConstructWrapper for PyPointer {
    fn as_construct(&self) -> &dyn Construct {
        self.inner.as_ref()
    }
}

impl PyPointer {
    /// Reconstructs an owned `Box<dyn Construct>` from stored Python objects.
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            let con: Box<dyn Construct> = if self.offset_obj.bind(py).is_callable() {
                let expr = py_param_to_evaluate(py, self.offset_obj.bind(py))?;
                Box::new(construct::constructs::stream_ops::PointerExpr::new(
                    expr, sc,
                ))
            } else {
                let off: i64 = self.offset_obj.extract(py)?;
                Box::new(construct::constructs::stream_ops::Pointer::new(off, sc))
            };
            Ok(con)
        })
    }
}

/// Factory: `Pointer(offset, subcon, stream=None, relativeOffset=False)`.
///
/// `stream` and `relativeOffset=True` are not supported (raise NotImplementedError).
#[pyfunction]
#[pyo3(name = "Pointer", signature = (offset, subcon, stream=None, relative_offset=false))]
#[allow(clippy::too_many_arguments)]
pub fn py_pointer(
    py: Python<'_>,
    offset: &Bound<PyAny>,
    subcon: &Bound<PyAny>,
    stream: Option<&Bound<PyAny>>,
    relative_offset: bool,
) -> PyResult<PyPointer> {
    if stream.is_some() {
        return Err(PyNotImplementedError::new_err(
            "Pointer(stream=...) is not supported in construct-rust",
        ));
    }
    if relative_offset {
        return Err(PyNotImplementedError::new_err(
            "Pointer(relativeOffset=True) is not yet supported, use absolute offset",
        ));
    }
    let sc = extract_subcon(subcon)?;
    let inner: Box<dyn Construct> = if offset.is_callable() {
        let expr = py_param_to_evaluate(py, offset)?;
        Box::new(construct::constructs::stream_ops::PointerExpr::new(
            expr, sc,
        ))
    } else {
        let off: i64 = offset.extract()?;
        Box::new(construct::constructs::stream_ops::Pointer::new(off, sc))
    };
    Ok(PyPointer {
        inner,
        offset_obj: offset.clone().unbind(),
        subcon_obj: subcon.clone().unbind(),
    })
}

crate::impl_api_methods!(PyPointer);
crate::impl_construct_operators!(PyPointer);

// ===========================================================================
// Peek
// ===========================================================================

/// PyO3 wrapper: peek without consuming stream position.
///
/// Corresponds to Python `Peek(subcon)`.
#[pyclass(name = "Peek", unsendable)]
pub struct PyPeek {
    pub(crate) inner: construct::constructs::stream_ops::Peek,
    pub(crate) subcon_obj: PyObject,
}

impl PyConstructWrapper for PyPeek {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyPeek {
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            Ok(boxed(construct::constructs::stream_ops::Peek::new(sc)))
        })
    }
}

/// Factory: `Peek(subcon)`.
#[pyfunction]
#[pyo3(name = "Peek")]
pub fn py_peek(_py: Python<'_>, subcon: &Bound<PyAny>) -> PyResult<PyPeek> {
    let sc = extract_subcon(subcon)?;
    Ok(PyPeek {
        inner: construct::constructs::stream_ops::Peek::new(sc),
        subcon_obj: subcon.clone().unbind(),
    })
}

crate::impl_api_methods!(PyPeek);
crate::impl_construct_operators!(PyPeek);

// ===========================================================================
// RawCopy
// ===========================================================================

/// PyO3 wrapper: raw byte copy capture.
///
/// Corresponds to Python `RawCopy(subcon)`.
#[pyclass(name = "RawCopy", unsendable)]
pub struct PyRawCopy {
    pub(crate) inner: construct::constructs::stream_ops::RawCopy,
    pub(crate) subcon_obj: PyObject,
}

impl PyConstructWrapper for PyRawCopy {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyRawCopy {
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            Ok(boxed(construct::constructs::stream_ops::RawCopy::new(sc)))
        })
    }
}

/// Factory: `RawCopy(subcon)`.
#[pyfunction]
#[pyo3(name = "RawCopy")]
pub fn py_raw_copy(_py: Python<'_>, subcon: &Bound<PyAny>) -> PyResult<PyRawCopy> {
    let sc = extract_subcon(subcon)?;
    Ok(PyRawCopy {
        inner: construct::constructs::stream_ops::RawCopy::new(sc),
        subcon_obj: subcon.clone().unbind(),
    })
}

crate::impl_api_methods!(PyRawCopy);
crate::impl_construct_operators!(PyRawCopy);

// ===========================================================================
// Prefixed
// ===========================================================================

/// PyO3 wrapper: length-prefixed subconstruct.
///
/// Corresponds to Python `Prefixed(lengthfield, subcon, includelength=False)`.
#[pyclass(name = "Prefixed", unsendable)]
pub struct PyPrefixed {
    pub(crate) inner: construct::constructs::stream_ops::Prefixed,
    pub(crate) lengthfield_obj: PyObject,
    pub(crate) subcon_obj: PyObject,
    pub(crate) includelength: bool,
}

impl PyConstructWrapper for PyPrefixed {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyPrefixed {
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let lf = extract_subcon(self.lengthfield_obj.bind(py))?;
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            let mut inner = construct::constructs::stream_ops::Prefixed::new(lf, sc);
            if self.includelength {
                inner = inner.with_include_length(true);
            }
            Ok(boxed(inner))
        })
    }
}

/// Factory: `Prefixed(lengthfield, subcon, includelength=False)`.
#[pyfunction]
#[pyo3(name = "Prefixed", signature = (lengthfield, subcon, includelength=false))]
pub fn py_prefixed(
    _py: Python<'_>,
    lengthfield: &Bound<PyAny>,
    subcon: &Bound<PyAny>,
    includelength: bool,
) -> PyResult<PyPrefixed> {
    let lf = extract_subcon(lengthfield)?;
    let sc = extract_subcon(subcon)?;
    let mut inner = construct::constructs::stream_ops::Prefixed::new(lf, sc);
    if includelength {
        inner = inner.with_include_length(true);
    }
    Ok(PyPrefixed {
        inner,
        lengthfield_obj: lengthfield.clone().unbind(),
        subcon_obj: subcon.clone().unbind(),
        includelength,
    })
}

crate::impl_api_methods!(PyPrefixed);
crate::impl_construct_operators!(PyPrefixed);

// ===========================================================================
// Transformed
// ===========================================================================

/// PyO3 wrapper: byte-transformation tunnel.
///
/// Corresponds to Python
/// `Transformed(subcon, decodefunc, decodeamount, encodefunc, encodeamount)`.
#[pyclass(name = "Transformed", unsendable)]
pub struct PyTransformed {
    pub(crate) inner: construct::constructs::stream_ops::Transformed,
    pub(crate) subcon_obj: PyObject,
    pub(crate) decode_obj: PyObject,
    pub(crate) decode_amount: Option<usize>,
    pub(crate) encode_obj: PyObject,
    pub(crate) encode_amount: Option<usize>,
}

impl PyConstructWrapper for PyTransformed {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyTransformed {
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            let decode = py_to_transform_func(self.decode_obj.clone_ref(py));
            let encode = py_to_transform_func(self.encode_obj.clone_ref(py));
            Ok(boxed(construct::constructs::stream_ops::Transformed::new(
                sc,
                decode,
                self.decode_amount,
                encode,
                self.encode_amount,
            )))
        })
    }
}

/// Factory: `Transformed(subcon, decodefunc, decodeamount, encodefunc, encodeamount)`.
#[pyfunction]
#[pyo3(
    name = "Transformed",
    signature = (subcon, decodefunc, decodeamount, encodefunc, encodeamount)
)]
pub fn py_transformed(
    _py: Python<'_>,
    subcon: &Bound<PyAny>,
    decodefunc: &Bound<PyAny>,
    decodeamount: Option<usize>,
    encodefunc: &Bound<PyAny>,
    encodeamount: Option<usize>,
) -> PyResult<PyTransformed> {
    let sc = extract_subcon(subcon)?;
    let decode = py_to_transform_func(decodefunc.clone().unbind());
    let encode = py_to_transform_func(encodefunc.clone().unbind());
    Ok(PyTransformed {
        inner: construct::constructs::stream_ops::Transformed::new(
            sc,
            decode,
            decodeamount,
            encode,
            encodeamount,
        ),
        subcon_obj: subcon.clone().unbind(),
        decode_obj: decodefunc.clone().unbind(),
        decode_amount: decodeamount,
        encode_obj: encodefunc.clone().unbind(),
        encode_amount: encodeamount,
    })
}

crate::impl_api_methods!(PyTransformed);
crate::impl_construct_operators!(PyTransformed);

// ===========================================================================
// Restreamed
// ===========================================================================

/// PyO3 wrapper: chunk-wise restreaming tunnel.
///
/// Corresponds to Python
/// `Restreamed(subcon, decoder, decoderunit, encoder, encoderunit, sizecomputer)`.
#[pyclass(name = "Restreamed", unsendable)]
pub struct PyRestreamed {
    pub(crate) inner: construct::constructs::stream_ops::Restreamed,
    pub(crate) subcon_obj: PyObject,
    pub(crate) decoder_obj: PyObject,
    pub(crate) decoder_unit: usize,
    pub(crate) encoder_obj: PyObject,
    pub(crate) encoder_unit: usize,
    pub(crate) sizecomputer_obj: Option<PyObject>,
}

impl PyConstructWrapper for PyRestreamed {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyRestreamed {
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            let dec = py_to_transform_func(self.decoder_obj.clone_ref(py));
            let enc = py_to_transform_func(self.encoder_obj.clone_ref(py));
            let size_comp = match &self.sizecomputer_obj {
                Some(f) => Some(py_to_size_computer(f.clone_ref(py))?),
                None => None,
            };
            Ok(boxed(construct::constructs::stream_ops::Restreamed::new(
                sc,
                dec,
                self.decoder_unit,
                enc,
                self.encoder_unit,
                size_comp,
            )))
        })
    }
}

/// Factory: `Restreamed(subcon, decoder, decoderunit, encoder, encoderunit, sizecomputer)`.
#[pyfunction]
#[pyo3(
    name = "Restreamed",
    signature = (subcon, decoder, decoderunit, encoder, encoderunit, sizecomputer)
)]
#[allow(clippy::too_many_arguments)]
pub fn py_restreamed(
    _py: Python<'_>,
    subcon: &Bound<PyAny>,
    decoder: &Bound<PyAny>,
    decoderunit: usize,
    encoder: &Bound<PyAny>,
    encoderunit: usize,
    sizecomputer: Option<&Bound<PyAny>>,
) -> PyResult<PyRestreamed> {
    let sc = extract_subcon(subcon)?;
    let dec = py_to_transform_func(decoder.clone().unbind());
    let enc = py_to_transform_func(encoder.clone().unbind());
    let size_comp = match sizecomputer {
        Some(f) => Some(py_to_size_computer(f.clone().unbind())?),
        None => None,
    };
    Ok(PyRestreamed {
        inner: construct::constructs::stream_ops::Restreamed::new(
            sc,
            dec,
            decoderunit,
            enc,
            encoderunit,
            size_comp,
        ),
        subcon_obj: subcon.clone().unbind(),
        decoder_obj: decoder.clone().unbind(),
        decoder_unit: decoderunit,
        encoder_obj: encoder.clone().unbind(),
        encoder_unit: encoderunit,
        sizecomputer_obj: sizecomputer.map(|f| f.clone().unbind()),
    })
}

crate::impl_api_methods!(PyRestreamed);
crate::impl_construct_operators!(PyRestreamed);

// ===========================================================================
// Compressed (feature-gated: flate2)
// ===========================================================================

/// PyO3 wrapper: compressed-data tunnel (zlib/deflate).
///
/// Corresponds to Python `Compressed(subcon, encoding, level=None)`.
///
/// Known limitation: `level` is accepted but not forwarded to the Rust
/// backend (which uses `flate2::Compression::default()`).
#[pyclass(name = "Compressed", unsendable)]
pub struct PyCompressed {
    pub(crate) inner: construct::constructs::stream_ops::Compressed,
    pub(crate) subcon_obj: PyObject,
    pub(crate) encoding: String,
}

impl PyConstructWrapper for PyCompressed {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyCompressed {
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            let algorithm = parse_compression_encoding(&self.encoding)?;
            Ok(boxed(construct::constructs::stream_ops::Compressed::new(
                sc, algorithm,
            )))
        })
    }
}

/// Maps a string encoding name to a `CompressionAlgorithm`.
fn parse_compression_encoding(
    encoding: &str,
) -> PyResult<construct::constructs::stream_ops::CompressionAlgorithm> {
    match encoding {
        "zlib" => Ok(construct::constructs::stream_ops::CompressionAlgorithm::Zlib),
        "deflate" => Ok(construct::constructs::stream_ops::CompressionAlgorithm::Deflate),
        other => Err(PyValueError::new_err(format!(
            "Unsupported compression encoding '{other}': only zlib/deflate are supported"
        ))),
    }
}

/// Factory: `Compressed(subcon, encoding, level=None)`.
///
/// Only `"zlib"` and `"deflate"` are supported. `level` is accepted but not
/// used (Rust uses `flate2::Compression::default()`).
#[pyfunction]
#[pyo3(name = "Compressed", signature = (subcon, encoding, level=None))]
pub fn py_compressed(
    _py: Python<'_>,
    subcon: &Bound<PyAny>,
    encoding: &str,
    level: Option<i32>,
) -> PyResult<PyCompressed> {
    let _ = level; // accepted but not forwarded (PM decision)
    let sc = extract_subcon(subcon)?;
    let algorithm = parse_compression_encoding(encoding)?;
    Ok(PyCompressed {
        inner: construct::constructs::stream_ops::Compressed::new(sc, algorithm),
        subcon_obj: subcon.clone().unbind(),
        encoding: encoding.to_string(),
    })
}

crate::impl_api_methods!(PyCompressed);
crate::impl_construct_operators!(PyCompressed);

// ===========================================================================
// Checksum
// ===========================================================================

/// PyO3 wrapper: checksum field that verifies a hash.
///
/// Corresponds to Python `Checksum(checksumfield, hashfunc, bytesfunc)`.
#[pyclass(name = "Checksum", unsendable)]
pub struct PyChecksum {
    pub(crate) inner: construct::constructs::stream_ops::Checksum,
    pub(crate) checksumfield_obj: PyObject,
    pub(crate) hashfunc_obj: PyObject,
    pub(crate) bytesfunc_obj: PyObject,
}

impl PyConstructWrapper for PyChecksum {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyChecksum {
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let cf = extract_subcon(self.checksumfield_obj.bind(py))?;
            let hf = py_to_checksum_func(self.hashfunc_obj.clone_ref(py));
            let bf = py_to_checksum_bytes_func(self.bytesfunc_obj.clone_ref(py));
            Ok(boxed(construct::constructs::stream_ops::Checksum::new(
                cf, hf, bf,
            )))
        })
    }
}

/// Factory: `Checksum(checksumfield, hashfunc, bytesfunc)`.
/// Always uses the Python-side implementation for full flexibility
/// (bytesfunc can return non-bytes values).
#[pyfunction]
#[pyo3(name = "Checksum", signature = (checksumfield, hashfunc, bytesfunc))]
pub fn py_checksum(
    py: Python<'_>,
    checksumfield: &Bound<PyAny>,
    hashfunc: &Bound<PyAny>,
    bytesfunc: &Bound<PyAny>,
) -> PyResult<PyObject> {
    // Use Python-side Checksum for full flexibility
    let module = py.import_bound("construct_rust._checksum")?;
    let cls = module.getattr("_Checksum")?;
    let result = cls.call1((checksumfield.clone(), hashfunc.clone(), bytesfunc.clone()))?;
    Ok(result.unbind())
}

crate::impl_api_methods!(PyChecksum);
crate::impl_construct_operators!(PyChecksum);

// ===========================================================================
// ByteSwapped / BitsSwapped
// ===========================================================================

/// PyO3 wrapper: byte-order reversal (fixed-size subcon).
///
/// Corresponds to Python `ByteSwapped(subcon)`.
#[pyclass(name = "ByteSwapped", unsendable)]
pub struct PyByteSwapped {
    pub(crate) inner: construct::constructs::stream_ops::ByteSwapped,
    pub(crate) subcon_obj: PyObject,
}

impl PyConstructWrapper for PyByteSwapped {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyByteSwapped {
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            Ok(boxed(construct::constructs::stream_ops::ByteSwapped::new(
                sc,
            )))
        })
    }
}

/// Factory: `ByteSwapped(subcon)`.
#[pyfunction]
#[pyo3(name = "ByteSwapped")]
pub fn py_byte_swapped(_py: Python<'_>, subcon: &Bound<PyAny>) -> PyResult<PyByteSwapped> {
    let sc = extract_subcon(subcon)?;
    Ok(PyByteSwapped {
        inner: construct::constructs::stream_ops::ByteSwapped::new(sc),
        subcon_obj: subcon.clone().unbind(),
    })
}

crate::impl_api_methods!(PyByteSwapped);
crate::impl_construct_operators!(PyByteSwapped);

/// PyO3 wrapper: bit-order reversal within each byte.
///
/// Corresponds to Python `BitsSwapped(subcon)`.
#[pyclass(name = "BitsSwapped", unsendable)]
pub struct PyBitsSwapped {
    pub(crate) inner: construct::constructs::stream_ops::BitsSwapped,
    pub(crate) subcon_obj: PyObject,
}

impl PyConstructWrapper for PyBitsSwapped {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyBitsSwapped {
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            let ctx = Context::new();
            let con: Box<dyn Construct> = match sc.sizeof(&ctx) {
                Ok(_) => Box::new(construct::constructs::stream_ops::BitsSwapped::new_fixed(
                    sc,
                )),
                Err(_) => {
                    Box::new(construct::constructs::stream_ops::BitsSwapped::new_variable(sc))
                }
            };
            Ok(con)
        })
    }
}

/// Factory: `BitsSwapped(subcon)`.
///
/// Chooses fixed vs variable variant by attempting `sizeof`.
#[pyfunction]
#[pyo3(name = "BitsSwapped")]
pub fn py_bits_swapped(_py: Python<'_>, subcon: &Bound<PyAny>) -> PyResult<PyBitsSwapped> {
    let sc = extract_subcon(subcon)?;
    let ctx = Context::new();
    let inner = match sc.sizeof(&ctx) {
        Ok(_) => construct::constructs::stream_ops::BitsSwapped::new_fixed(sc),
        Err(_) => construct::constructs::stream_ops::BitsSwapped::new_variable(sc),
    };
    Ok(PyBitsSwapped {
        inner,
        subcon_obj: subcon.clone().unbind(),
    })
}

crate::impl_api_methods!(PyBitsSwapped);
crate::impl_construct_operators!(PyBitsSwapped);

// ===========================================================================
// FixedSized
// ===========================================================================

/// PyO3 wrapper: fixed-size sub-stream.
///
/// Corresponds to Python `FixedSized(length, subcon)`.
#[pyclass(name = "FixedSized", unsendable)]
pub struct PyFixedSized {
    pub(crate) inner: construct::constructs::FixedSized,
    pub(crate) subcon_obj: PyObject,
    pub(crate) length: usize,
}

impl PyConstructWrapper for PyFixedSized {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyFixedSized {
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            Ok(boxed(construct::constructs::FixedSized::new(
                self.length,
                sc,
            )))
        })
    }
}

/// Factory: `FixedSized(length, subcon)`.
///
/// `length` must be a plain int (context-lambda length is not supported).
#[pyfunction]
#[pyo3(name = "FixedSized", signature = (length, subcon))]
pub fn py_fixed_sized(
    _py: Python<'_>,
    length: &Bound<PyAny>,
    subcon: &Bound<PyAny>,
) -> PyResult<PyObject> {
    if length.is_callable() {
        return Err(PyNotImplementedError::new_err(
            "FixedSized with context lambda length is not supported",
        ));
    }
    let len: i64 = length.extract()?;
    if len < 0 {
        let adapter_mod = _py.import_bound("construct_rust._adapter")?;
        let cls = adapter_mod.getattr("_NegativeFixedSized")?;
        let result = cls.call1((len,))?;
        return Ok(result.unbind());
    }
    let len = len as usize;
    let sc = extract_subcon(subcon)?;
    let result = PyFixedSized {
        inner: construct::constructs::FixedSized::new(len, sc),
        subcon_obj: subcon.clone().unbind(),
        length: len,
    };
    Ok(Bound::new(_py, result)?.into_any().unbind())
}

crate::impl_api_methods!(PyFixedSized);
crate::impl_construct_operators!(PyFixedSized);

// ===========================================================================
// Lazy — parse returns a zero-arg callable
// ===========================================================================

/// Wraps a parsed `PyObject` value as a zero-argument Python callable.
///
/// This matches the Python original `Lazy._parse`, which returns an
/// `execute()` closure. Rust eagerly parses the value; this wrapper preserves
/// the callable interface so `x = Lazy(Byte).parse(data); x()` works.
///
/// Uses a Python lambda that captures the value via a **globals** dict.
/// Python lambda bodies look up free variables in the enclosing scope then
/// the module globals, but NOT in `eval` locals. Placing `_v` in globals
/// ensures it is resolved at call time.
fn wrap_as_zero_arg_callable(py: Python<'_>, value: PyObject) -> PyObject {
    let globals = pyo3::types::PyDict::new_bound(py);
    let fallback = value.clone_ref(py);
    if globals.set_item("_v", value).is_err() {
        return fallback;
    }
    match py.eval_bound("lambda *a, **k: _v", Some(&globals), None) {
        Ok(callable) => callable.unbind(),
        Err(_) => fallback,
    }
}

/// PyO3 wrapper: lazy single-construct (parse returns a callable).
///
/// Corresponds to Python `Lazy(subcon)`.
#[pyclass(name = "Lazy", unsendable)]
pub struct PyLazy {
    pub(crate) inner: construct::constructs::Lazy,
    pub(crate) subcon_obj: PyObject,
}

impl PyConstructWrapper for PyLazy {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

#[pymethods]
impl PyLazy {
    /// Parse bytes — returns a zero-arg callable (Python Lazy semantics).
    #[pyo3(signature = (data, **kw))]
    fn parse(
        &self,
        py: Python<'_>,
        data: &Bound<PyAny>,
        kw: Option<&Bound<PyDict>>,
    ) -> PyResult<PyObject> {
        let bytes: std::borrow::Cow<[u8]> = data.extract()?;
        let ctx = crate::api::opt_kw_to_indexmap(py, kw)?;
        let value = crate::api::py_parse(&self.inner, py, &bytes, ctx)?;
        Ok(wrap_as_zero_arg_callable(py, value))
    }

    /// Parse from stream — returns a zero-arg callable.
    #[pyo3(signature = (stream, **kw))]
    fn parse_stream(
        &self,
        py: Python<'_>,
        stream: PyObject,
        kw: Option<&Bound<PyDict>>,
    ) -> PyResult<PyObject> {
        let ctx = crate::api::opt_kw_to_indexmap(py, kw)?;
        let value = crate::api::py_parse_stream(&self.inner, py, stream, ctx)?;
        Ok(wrap_as_zero_arg_callable(py, value))
    }

    /// Parse from file — returns a zero-arg callable.
    #[pyo3(signature = (filename, **kw))]
    fn parse_file(
        &self,
        py: Python<'_>,
        filename: &str,
        kw: Option<&Bound<PyDict>>,
    ) -> PyResult<PyObject> {
        let ctx = crate::api::opt_kw_to_indexmap(py, kw)?;
        let value = crate::api::py_parse_file(&self.inner, py, filename, ctx)?;
        Ok(wrap_as_zero_arg_callable(py, value))
    }

    /// Build — if `obj` is callable, invoke `obj()` first (Python Lazy._build).
    #[pyo3(signature = (obj, **kw))]
    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<PyAny>,
        kw: Option<&Bound<PyDict>>,
    ) -> PyResult<PyObject> {
        let actual_obj = if obj.is_callable() {
            obj.call0()?
        } else {
            obj.clone().into_any()
        };
        let ctx = crate::api::opt_kw_to_indexmap(py, kw)?;
        crate::api::py_build(&self.inner, py, &actual_obj, ctx)
    }

    /// Build to stream — if `obj` is callable, invoke `obj()` first.
    #[pyo3(signature = (obj, stream, **kw))]
    fn build_stream(
        &self,
        py: Python<'_>,
        obj: &Bound<PyAny>,
        stream: PyObject,
        kw: Option<&Bound<PyDict>>,
    ) -> PyResult<()> {
        let actual_obj = if obj.is_callable() {
            obj.call0()?
        } else {
            obj.clone().into_any()
        };
        let ctx = crate::api::opt_kw_to_indexmap(py, kw)?;
        crate::api::py_build_stream(&self.inner, py, &actual_obj, stream, ctx)
    }

    /// Build to file — if `obj` is callable, invoke `obj()` first.
    #[pyo3(signature = (obj, filename, **kw))]
    fn build_file(
        &self,
        py: Python<'_>,
        obj: &Bound<PyAny>,
        filename: &str,
        kw: Option<&Bound<PyDict>>,
    ) -> PyResult<()> {
        let actual_obj = if obj.is_callable() {
            obj.call0()?
        } else {
            obj.clone().into_any()
        };
        let ctx = crate::api::opt_kw_to_indexmap(py, kw)?;
        crate::api::py_build_file(&self.inner, py, &actual_obj, filename, ctx)
    }

    /// Compute static byte size.
    #[pyo3(signature = (**kw))]
    fn sizeof(&self, py: Python<'_>, kw: Option<&Bound<PyDict>>) -> PyResult<usize> {
        let ctx = crate::api::opt_kw_to_indexmap(py, kw)?;
        crate::api::py_sizeof(&self.inner, py, ctx)
    }
}

/// Factory: `Lazy(subcon)`.
#[pyfunction]
#[pyo3(name = "Lazy")]
pub fn py_lazy(_py: Python<'_>, subcon: &Bound<PyAny>) -> PyResult<PyLazy> {
    let sc = extract_subcon(subcon)?;
    Ok(PyLazy {
        inner: construct::constructs::Lazy::new(sc),
        subcon_obj: subcon.clone().unbind(),
    })
}

impl PyLazy {
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            Ok(boxed(construct::constructs::Lazy::new(sc)))
        })
    }
}

crate::impl_construct_operators!(PyLazy);

// ===========================================================================
// LazyStruct
// ===========================================================================

/// PyO3 wrapper: lazy struct (eager parse in Rust; type preserved for API parity).
///
/// Corresponds to Python `LazyStruct(*subcons, **subconskw)`.
#[pyclass(name = "LazyStruct", unsendable)]
pub struct PyLazyStruct {
    pub(crate) inner: construct::constructs::LazyStruct,
    pub(crate) py_subcons: Vec<PyObject>,
}

impl PyConstructWrapper for PyLazyStruct {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyLazyStruct {
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let mut builder = construct::constructs::LazyStruct::new();
            for sub in &self.py_subcons {
                let inner = extract_subcon(sub.bind(py))?;
                let name = crate::py_renamed::get_subcon_name(sub.bind(py))?;
                match name {
                    Some(n) => builder = builder.field(n, inner),
                    None => builder = builder.anonymous(inner),
                }
            }
            Ok(boxed(builder))
        })
    }
}

/// Factory: `LazyStruct(*subcons, **subconskw)`.
#[pyfunction]
#[pyo3(name = "LazyStruct", signature = (*subcons, **subconskw))]
pub fn py_lazy_struct(
    py: Python<'_>,
    subcons: &Bound<PyTuple>,
    subconskw: Option<&Bound<PyDict>>,
) -> PyResult<PyLazyStruct> {
    let mut builder = construct::constructs::LazyStruct::new();
    let mut py_subcons = Vec::new();
    for item in subcons.iter() {
        let inner = extract_subcon(&item)?;
        let name = crate::py_renamed::get_subcon_name(&item)?;
        match name {
            Some(n) => builder = builder.field(n, inner),
            None => builder = builder.anonymous(inner),
        }
        py_subcons.push(item.clone().unbind());
    }
    if let Some(kw) = subconskw {
        for (key, val) in kw.iter() {
            let name: String = key.extract()?;
            let renamed = crate::py_renamed::apply_rename(py, &val, &name)?;
            let inner = extract_subcon(renamed.bind(py))?;
            builder = builder.field(name, inner);
            py_subcons.push(renamed);
        }
    }
    Ok(PyLazyStruct {
        inner: builder,
        py_subcons,
    })
}

crate::impl_api_methods!(PyLazyStruct);
crate::impl_construct_operators!(PyLazyStruct);

// ===========================================================================
// LazyArray
// ===========================================================================

/// PyO3 wrapper: lazy array (eager parse in Rust; type preserved for API parity).
///
/// Corresponds to Python `LazyArray(count, subcon)`.
#[pyclass(name = "LazyArray", unsendable)]
pub struct PyLazyArray {
    pub(crate) inner: construct::constructs::LazyArray,
    pub(crate) count: usize,
    pub(crate) subcon_obj: PyObject,
}

impl PyConstructWrapper for PyLazyArray {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyLazyArray {
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            Ok(boxed(construct::constructs::LazyArray::new(self.count, sc)))
        })
    }
}

/// Factory: `LazyArray(count, subcon)`.
///
/// `count` must be a plain int (expression count is not supported).
#[pyfunction]
#[pyo3(name = "LazyArray", signature = (count, subcon))]
pub fn py_lazy_array(
    _py: Python<'_>,
    count: &Bound<PyAny>,
    subcon: &Bound<PyAny>,
) -> PyResult<PyLazyArray> {
    if count.is_callable() {
        return Err(PyNotImplementedError::new_err(
            "LazyArray with expression count is not supported",
        ));
    }
    let n: usize = count.extract()?;
    let sc = extract_subcon(subcon)?;
    Ok(PyLazyArray {
        inner: construct::constructs::LazyArray::new(n, sc),
        count: n,
        subcon_obj: subcon.clone().unbind(),
    })
}

crate::impl_api_methods!(PyLazyArray);
crate::impl_construct_operators!(PyLazyArray);

// ===========================================================================
// LazyBound
// ===========================================================================

/// PyO3 wrapper: runtime-bound recursive construct.
///
/// Corresponds to Python `LazyBound(ctxfunc)` where `ctxfunc` is a no-arg
/// callable returning a subcon.
#[pyclass(name = "LazyBound", unsendable)]
pub struct PyLazyBound {
    pub(crate) inner: construct::constructs::stream_ops::LazyBound,
    pub(crate) ctxfunc_obj: PyObject,
}

impl PyConstructWrapper for PyLazyBound {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyLazyBound {
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        let ctxfunc_obj = Python::with_gil(|py| self.ctxfunc_obj.clone_ref(py));
        let subcon_func: Box<dyn Fn() -> Box<CombinedConstruct> + Send + Sync> =
            Box::new(move || {
                Python::with_gil(|py| {
                    let result: PyResult<Box<CombinedConstruct>> = (|| {
                        let py_constr = ctxfunc_obj.call0(py)?;
                        let sc = extract_subcon(py_constr.bind(py))?;
                        Ok(sc)
                    })();
                    result.unwrap_or_else(|e| {
                        Box::new(construct::combined::dynamic(ErrorWithMessage::new(
                            format!("LazyBound failed to resolve subcon: {e}"),
                        )))
                    })
                })
            });
        Ok(boxed(construct::constructs::stream_ops::LazyBound::new(
            subcon_func,
        )))
    }
}

/// Factory: `LazyBound(ctxfunc)`.
#[pyfunction]
#[pyo3(name = "LazyBound")]
pub fn py_lazy_bound(py: Python<'_>, ctxfunc: &Bound<PyAny>) -> PyResult<PyLazyBound> {
    let ctxfunc_obj: PyObject = ctxfunc.clone().unbind();
    let captured = ctxfunc_obj.clone_ref(py);
    let subcon_func: Box<dyn Fn() -> Box<CombinedConstruct> + Send + Sync> = Box::new(move || {
        Python::with_gil(|py| {
            let result: PyResult<Box<CombinedConstruct>> = (|| {
                let py_constr = captured.call0(py)?;
                let sc = extract_subcon(py_constr.bind(py))?;
                Ok(sc)
            })();
            result.unwrap_or_else(|e| {
                Box::new(construct::combined::dynamic(ErrorWithMessage::new(
                    format!("LazyBound failed to resolve subcon: {e}"),
                )))
            })
        })
    });
    Ok(PyLazyBound {
        inner: construct::constructs::stream_ops::LazyBound::new(subcon_func),
        ctxfunc_obj,
    })
}

crate::impl_api_methods!(PyLazyBound);
crate::impl_construct_operators!(PyLazyBound);

// ===========================================================================
// Rebuffered
// ===========================================================================

/// PyO3 wrapper: fully-buffered sub-stream.
///
/// Corresponds to Python `Rebuffered(subcon, tailcutoff=None)`.
#[pyclass(name = "Rebuffered", unsendable)]
pub struct PyRebuffered {
    pub(crate) inner: construct::constructs::Rebuffered,
    pub(crate) subcon_obj: PyObject,
    pub(crate) tailcutoff: Option<usize>,
}

impl PyConstructWrapper for PyRebuffered {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyRebuffered {
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            let mut inner = construct::constructs::Rebuffered::new(sc);
            if let Some(tc) = self.tailcutoff {
                inner = inner.with_tailcutoff(tc);
            }
            Ok(boxed(inner))
        })
    }
}

/// Factory: `Rebuffered(subcon, tailcutoff=None)`.
#[pyfunction]
#[pyo3(name = "Rebuffered", signature = (subcon, tailcutoff=None))]
pub fn py_rebuffered(
    _py: Python<'_>,
    subcon: &Bound<PyAny>,
    tailcutoff: Option<usize>,
) -> PyResult<PyRebuffered> {
    let sc = extract_subcon(subcon)?;
    let mut inner = construct::constructs::Rebuffered::new(sc);
    if let Some(tc) = tailcutoff {
        inner = inner.with_tailcutoff(tc);
    }
    Ok(PyRebuffered {
        inner,
        subcon_obj: subcon.clone().unbind(),
        tailcutoff,
    })
}

crate::impl_api_methods!(PyRebuffered);
crate::impl_construct_operators!(PyRebuffered);

// ===========================================================================
// Functional factories (Layer 2)
// ===========================================================================

/// Factory: `BitStruct(*subcons, **subconskw)` → `Bitwise(Struct(*subcons))`.
///
/// This is a functional factory (Python `BitStruct` is a `def`, not a class).
/// It returns a [`PyBitwise`] wrapping a `Struct`.
///
/// # Implementation note
///
/// The Rust `Struct` is built twice from the same subcons: once for the
/// inner `Bitwise` (so bit-level fields like Bit/Nibble work correctly with
/// the Rust bit-stream), and once wrapped in a [`PyStruct`] Python object
/// stored as `subcon_obj` (so [`PyBitwise::make_owned`] can re-extract a
/// construct when this `Bitwise` is nested inside another composite). This
/// is necessary because `construct::constructs::Struct` does not implement
/// `Clone`.
#[pyfunction]
#[pyo3(name = "BitStruct", signature = (*subcons, **subconskw))]
pub fn py_bit_struct(
    py: Python<'_>,
    subcons: &Bound<PyTuple>,
    subconskw: Option<&Bound<PyDict>>,
) -> PyResult<PyBitwise> {
    // Build the inner Rust Struct for the Bitwise wrapper. This keeps
    // bit-level fields (Bit, Nibble) working correctly because they operate
    // directly on the Rust Bitwise sub-stream.
    let py_struct_for_inner = crate::constructs_composite::py_struct(py, subcons, subconskw)?;
    let sc: Box<CombinedConstruct> =
        Box::new(construct::combined::dynamic(py_struct_for_inner.inner));

    // Build a separate PyStruct instance to store as subcon_obj. This is used
    // by make_owned() when the Bitwise is nested inside another composite.
    // The previous code incorrectly stored the type OBJECT (not an instance),
    // causing TypeError when nested.
    let py_struct_for_obj = crate::constructs_composite::py_struct(py, subcons, subconskw)?;
    let subcon_obj = Py::new(py, py_struct_for_obj)?.into_any();

    Ok(PyBitwise {
        inner: construct::constructs::stream_ops::Bitwise::new(sc),
        subcon_obj,
    })
}

/// Factory: `PrefixedArray(countfield, subcon)` → `FocusedSeq`.
///
/// Equivalent to Python:
/// `FocusedSeq("items", "count"/Rebuild(countfield, len_(this.items)), "items"/subcon[this.count])`.
#[pyfunction]
#[pyo3(name = "PrefixedArray", signature = (countfield, subcon))]
pub fn py_prefixed_array(
    _py: Python<'_>,
    countfield: &Bound<PyAny>,
    subcon: &Bound<PyAny>,
) -> PyResult<crate::constructs_composite::PyFocusedSeq> {
    use construct::constructs::struct_::StructField;

    let cf = extract_subcon(countfield)?;
    let sc = extract_subcon(subcon)?;

    // count field: Rebuild(countfield, len_(this.items))
    // ComputeFunc = Fn(&Context) -> Result<Value>; adapt from Evaluate.
    let len_expr = LenThisItemsExpr;
    let count_compute: Box<
        dyn Fn(&Context) -> construct::core::error::Result<Value> + Send + Sync,
    > = Box::new(move |ctx| construct::expr::Evaluate::evaluate(&len_expr, ctx, None));
    let count_rebuild = construct::constructs::Rebuild::new(cf, count_compute);

    // items field: subcon[this.count]  →  ArrayExpr(this.count, subcon)
    let count_ref: Box<construct::expr::CombinedExpr> =
        Box::new(construct::expr::dynamic_expr(ThisCountExpr));
    let items_array = construct::constructs::ArrayExpr::new(count_ref, sc);

    let focused = construct::constructs::FocusedSeq::new(
        "items",
        vec![
            StructField::new(
                "count",
                Box::new(construct::combined::dynamic(count_rebuild)),
            ),
            StructField::new("items", Box::new(construct::combined::dynamic(items_array))),
        ],
    );
    Ok(crate::constructs_composite::PyFocusedSeq { inner: focused })
}

/// Expression: `len_(this.items)` — returns the length of `items` in context.
#[derive(Debug)]
struct LenThisItemsExpr;

impl construct::expr::Evaluate for LenThisItemsExpr {
    fn evaluate(
        &self,
        ctx: &Context,
        _obj: Option<&Value>,
    ) -> construct::core::error::Result<Value> {
        match ctx.get("items") {
            Some(Value::List(l)) => Ok(Value::UInt(l.len() as u64)),
            Some(other) => Err(construct::core::error::ConstructError::Expr {
                path: String::new(),
                message: format!("len_(this.items) expected list, got {}", other.type_name()),
            }),
            None => Err(construct::core::error::ConstructError::Expr {
                path: String::new(),
                message: "len_(this.items): 'items' not in context".to_string(),
            }),
        }
    }
}

/// Expression: `this.count` — returns the `count` field from context.
#[derive(Debug)]
struct ThisCountExpr;

impl construct::expr::Evaluate for ThisCountExpr {
    fn evaluate(
        &self,
        ctx: &Context,
        _obj: Option<&Value>,
    ) -> construct::core::error::Result<Value> {
        match ctx.get("count") {
            Some(v @ Value::UInt(_)) | Some(v @ Value::Int(_)) => Ok(v.clone()),
            Some(other) => Err(construct::core::error::ConstructError::Expr {
                path: String::new(),
                message: format!("this.count expected integer, got {}", other.type_name()),
            }),
            None => Err(construct::core::error::ConstructError::Expr {
                path: String::new(),
                message: "this.count: 'count' not in context".to_string(),
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

    /// Helper: create a Python Byte (Int8ub) pyclass instance for tests.
    fn get_byte(py: Python<'_>) -> Py<PyAny> {
        let ff = crate::constructs_atomic::py_format_field(">", "B").unwrap();
        Py::new(py, ff).unwrap().into_any()
    }

    #[test]
    fn bitwise_factory_basic() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let byte = get_byte(py);
            let bw = py_bitwise(py, byte.bind(py)).unwrap();
            // Bitwise(Bytes(8)).sizeof() == 1 (8 bits / 8)
            let ctx = Context::new();
            assert_eq!(bw.inner.sizeof(&ctx).unwrap(), 1);
        });
    }

    #[test]
    fn bytewise_factory_basic() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let byte = get_byte(py);
            let bw = py_bytewise(py, byte.bind(py)).unwrap();
            let ctx = Context::new();
            // Bytewise(Bytes(1)).sizeof() == 8 (1 byte * 8 bits, in bits)
            assert_eq!(bw.inner.sizeof(&ctx).unwrap(), 8);
        });
    }

    #[test]
    fn pointer_int_offset() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let byte = get_byte(py);
            let p = py_pointer(py, 8i64.into_py(py).bind(py), byte.bind(py), None, false).unwrap();
            // Pointer sizeof is 0
            let ctx = Context::new();
            assert_eq!(p.inner.sizeof(&ctx).unwrap(), 0);
        });
    }

    #[test]
    fn pointer_stream_unsupported() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let byte = get_byte(py);
            let stream = py.eval_bound("lambda: None", None, None).unwrap();
            let result = py_pointer(
                py,
                8i64.into_py(py).bind(py),
                byte.bind(py),
                Some(&stream),
                false,
            );
            assert!(result.is_err());
        });
    }

    #[test]
    fn compressed_unsupported_encoding() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let byte = get_byte(py);
            let result = py_compressed(py, byte.bind(py), "gzip", None);
            assert!(result.is_err());
            let err = result.err().unwrap();
            assert!(err.is_instance_of::<PyValueError>(py));
        });
    }

    #[test]
    fn compressed_zlib_ok() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let gb = crate::constructs_atomic::py_greedy_bytes();
            let c = py_compressed(
                py,
                Py::new(py, gb).unwrap().into_any().bind(py),
                "zlib",
                None,
            );
            assert!(c.is_ok());
        });
    }

    #[test]
    fn fixed_sized_callable_length_unsupported() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let byte = get_byte(py);
            let lam = py.eval_bound("lambda ctx: 4", None, None).unwrap();
            let result = py_fixed_sized(py, &lam, byte.bind(py));
            assert!(result.is_err());
            assert!(result
                .err()
                .unwrap()
                .is_instance_of::<PyNotImplementedError>(py));
        });
    }

    #[test]
    fn lazy_array_callable_count_unsupported() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let byte = get_byte(py);
            let lam = py.eval_bound("lambda ctx: 3", None, None).unwrap();
            let result = py_lazy_array(py, &lam, byte.bind(py));
            assert!(result.is_err());
            assert!(result
                .err()
                .unwrap()
                .is_instance_of::<PyNotImplementedError>(py));
        });
    }

    #[test]
    fn lazy_factory_builds() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let byte = get_byte(py);
            let lz = py_lazy(py, byte.bind(py)).unwrap();
            // sizeof delegates to subcon
            let ctx = Context::new();
            assert_eq!(lz.inner.sizeof(&ctx).unwrap(), 1);
        });
    }

    #[test]
    fn lazy_struct_factory() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let byte = get_byte(py);
            let tup = PyTuple::new_bound(py, [byte.bind(py)]);
            let ls = py_lazy_struct(py, &tup, None).unwrap();
            let ctx = Context::new();
            assert_eq!(ls.inner.sizeof(&ctx).unwrap(), 1);
        });
    }

    #[test]
    fn lazy_bound_factory() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let _byte = get_byte(py);
            let lam = py
                .eval_bound("lambda: __import__('construct_rust').Byte", None, None)
                .unwrap();
            // Just ensure it constructs without error
            let result = py_lazy_bound(py, &lam);
            assert!(result.is_ok());
        });
    }

    #[test]
    fn rebuffered_factory() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let byte = get_byte(py);
            let rb = py_rebuffered(py, byte.bind(py), None).unwrap();
            let ctx = Context::new();
            assert_eq!(rb.inner.sizeof(&ctx).unwrap(), 1);
        });
    }

    #[test]
    fn prefixed_array_factory() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let byte = get_byte(py);
            let byte_obj = byte.bind(py);
            let pa = py_prefixed_array(py, byte_obj, byte_obj).unwrap();
            // Just ensure the factory succeeds
            let _ = pa.inner;
        });
    }

    #[test]
    fn bit_struct_factory() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let byte = get_byte(py);
            let tup = PyTuple::new_bound(py, [byte.bind(py)]);
            let bs = py_bit_struct(py, &tup, None).unwrap();
            // BitStruct with 1 byte → Bitwise wraps a Struct of 1 byte
            let ctx = Context::new();
            // Bitwise sizeof = ceil(subcon_bits / 8)
            let size = bs.inner.sizeof(&ctx).unwrap();
            assert!(size >= 1);
        });
    }
}
