//! PyO3 module entry point for the construct_rust native extension.

#![allow(clippy::useless_conversion)]
#![allow(clippy::type_complexity)]

pub mod api;
pub mod construct_macros;
pub mod constructs_adapter;
pub mod constructs_atomic;
pub mod constructs_composite;
pub mod constructs_stream;
pub mod constructs_string;
pub mod conversions;
pub mod exceptions;
pub mod expr_bridge;
pub mod gallery;
pub mod py_adapter;
pub mod py_context;
pub mod py_input;
pub mod py_renamed;
pub mod py_sink;
pub mod pystream;

use pyo3::prelude::*;
use pyo3::types::PyModule;

/// Initializes the Python interpreter for unit tests.
#[cfg(test)]
pub fn ensure_python() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(pyo3::prepare_freethreaded_python);
}

/// Registers all exception classes as top-level module attributes.
fn register_exceptions(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add(
        "ConstructError",
        py.get_type_bound::<exceptions::ConstructError>(),
    )?;
    m.add(
        "SizeofError",
        py.get_type_bound::<exceptions::SizeofError>(),
    )?;
    m.add(
        "AdaptationError",
        py.get_type_bound::<exceptions::AdaptationError>(),
    )?;
    m.add(
        "ValidationError",
        py.get_type_bound::<exceptions::ValidationError>(),
    )?;
    m.add(
        "StreamError",
        py.get_type_bound::<exceptions::StreamError>(),
    )?;
    m.add(
        "FormatFieldError",
        py.get_type_bound::<exceptions::FormatFieldError>(),
    )?;
    m.add(
        "IntegerError",
        py.get_type_bound::<exceptions::IntegerError>(),
    )?;
    m.add(
        "StringError",
        py.get_type_bound::<exceptions::StringError>(),
    )?;
    m.add(
        "MappingError",
        py.get_type_bound::<exceptions::MappingError>(),
    )?;
    m.add("RangeError", py.get_type_bound::<exceptions::RangeError>())?;
    m.add(
        "RepeatError",
        py.get_type_bound::<exceptions::RepeatError>(),
    )?;
    m.add("ConstError", py.get_type_bound::<exceptions::ConstError>())?;
    m.add(
        "IndexFieldError",
        py.get_type_bound::<exceptions::IndexFieldError>(),
    )?;
    m.add("CheckError", py.get_type_bound::<exceptions::CheckError>())?;
    m.add(
        "ExplicitError",
        py.get_type_bound::<exceptions::ExplicitError>(),
    )?;
    m.add(
        "NamedTupleError",
        py.get_type_bound::<exceptions::NamedTupleError>(),
    )?;
    m.add(
        "TimestampError",
        py.get_type_bound::<exceptions::TimestampError>(),
    )?;
    m.add("UnionError", py.get_type_bound::<exceptions::UnionError>())?;
    m.add(
        "SelectError",
        py.get_type_bound::<exceptions::SelectError>(),
    )?;
    m.add(
        "SwitchError",
        py.get_type_bound::<exceptions::SwitchError>(),
    )?;
    m.add(
        "StopFieldError",
        py.get_type_bound::<exceptions::StopFieldError>(),
    )?;
    m.add(
        "PaddingError",
        py.get_type_bound::<exceptions::PaddingError>(),
    )?;
    m.add(
        "TerminatedError",
        py.get_type_bound::<exceptions::TerminatedError>(),
    )?;
    m.add(
        "RawCopyError",
        py.get_type_bound::<exceptions::RawCopyError>(),
    )?;
    m.add(
        "RotationError",
        py.get_type_bound::<exceptions::RotationError>(),
    )?;
    m.add(
        "ChecksumError",
        py.get_type_bound::<exceptions::ChecksumError>(),
    )?;
    m.add(
        "CancelParsing",
        py.get_type_bound::<exceptions::CancelParsing>(),
    )?;
    m.add(
        "CipherError",
        py.get_type_bound::<exceptions::CipherError>(),
    )?;
    Ok(())
}

/// Registers all pyclass types.
fn register_classes(m: &Bound<'_, PyModule>) -> PyResult<()> {
    use constructs_adapter::*;
    use constructs_atomic::*;
    use constructs_composite::*;
    use constructs_stream::*;
    use constructs_string::*;
    use gallery::PyGalleryParser;
    use py_context::PyContextView;
    use py_renamed::PyRenamed;

    // Atomic
    m.add_class::<PyFormatField>()?;
    m.add_class::<PyBytes>()?;
    m.add_class::<PyGreedyBytes>()?;
    m.add_class::<PyBytesInteger>()?;
    m.add_class::<PyBitsInteger>()?;
    m.add_class::<PyVarInt>()?;
    m.add_class::<PyZigZag>()?;
    m.add_class::<PyFlag>()?;
    m.add_class::<PyConst>()?;
    m.add_class::<PyPass>()?;
    m.add_class::<PyTerminated>()?;
    m.add_class::<PyTell>()?;
    m.add_class::<PySeek>()?;
    m.add_class::<PyError>()?;
    m.add_class::<PyComputed>()?;
    m.add_class::<PyRebuild>()?;
    m.add_class::<PyDefault>()?;
    m.add_class::<PyIndex>()?;
    // Composite
    m.add_class::<PyStruct>()?;
    m.add_class::<PySequence>()?;
    m.add_class::<PyArray>()?;
    m.add_class::<PyGreedyRange>()?;
    m.add_class::<PyRepeatUntil>()?;
    m.add_class::<PyUnion>()?;
    m.add_class::<PySelect>()?;
    m.add_class::<PyFocusedSeq>()?;
    m.add_class::<PyPadded>()?;
    m.add_class::<PyAligned>()?;
    // Adapter / Control flow (10.7)
    m.add_class::<PyIfThenElse>()?;
    m.add_class::<PySwitch>()?;
    m.add_class::<PyCheck>()?;
    m.add_class::<PyStopIf>()?;
    m.add_class::<PyEnum>()?;
    m.add_class::<PyFlagsEnum>()?;
    m.add_class::<PyMapping>()?;
    m.add_class::<PyHex>()?;
    m.add_class::<PyHexDump>()?;
    m.add_class::<PyExprAdapter>()?;
    m.add_class::<PySymmetricAdapter>()?;
    m.add_class::<PyExprSymmetricAdapter>()?;
    m.add_class::<PyValidator>()?;
    m.add_class::<PyExprValidator>()?;
    m.add_class::<PySlicing>()?;
    m.add_class::<PyIndexing>()?;
    // Stream operations / tunnel / lazy (10.8)
    m.add_class::<PyBitwise>()?;
    m.add_class::<PyBytewise>()?;
    m.add_class::<PyPointer>()?;
    m.add_class::<PyPeek>()?;
    m.add_class::<PyRawCopy>()?;
    m.add_class::<PyPrefixed>()?;
    m.add_class::<PyTransformed>()?;
    m.add_class::<PyRestreamed>()?;
    m.add_class::<PyCompressed>()?;
    m.add_class::<PyChecksum>()?;
    m.add_class::<PyByteSwapped>()?;
    m.add_class::<PyBitsSwapped>()?;
    m.add_class::<PyFixedSized>()?;
    m.add_class::<PyLazy>()?;
    m.add_class::<PyLazyStruct>()?;
    m.add_class::<PyLazyArray>()?;
    m.add_class::<PyLazyBound>()?;
    m.add_class::<PyRebuffered>()?;
    // String constructs (10.9)
    m.add_class::<PyPaddedString>()?;
    m.add_class::<PyCString>()?;
    // Gallery (10.9)
    m.add_class::<PyGalleryParser>()?;
    // Renamed
    m.add_class::<PyRenamed>()?;
    // ContextView (13.5)
    m.add_class::<PyContextView>()?;
    Ok(())
}
/// Registers all pyfunction factories.
fn register_functions(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    use constructs_adapter::*;
    use constructs_atomic::*;
    use constructs_composite::*;
    use constructs_stream::*;
    use constructs_string::*;
    use gallery::{py_elf, py_pe32file, py_ut_index};
    // Atomic
    m.add_function(wrap_pyfunction!(py_format_field, m)?)?;
    m.add_function(wrap_pyfunction!(py_bytes, m)?)?;
    m.add_function(wrap_pyfunction!(py_bytes_integer, m)?)?;
    m.add_function(wrap_pyfunction!(py_bits_integer, m)?)?;
    m.add_function(wrap_pyfunction!(py_const, m)?)?;
    m.add_function(wrap_pyfunction!(py_seek, m)?)?;
    // Error is a singleton instance (not a factory), matching Python's SingletonError
    {
        let error_inst = constructs_atomic::py_error();
        m.add("Error", Py::new(py, error_inst)?)?;
    }
    m.add_function(wrap_pyfunction!(py_computed, m)?)?;
    m.add_function(wrap_pyfunction!(py_rebuild, m)?)?;
    m.add_function(wrap_pyfunction!(py_default, m)?)?;
    m.add_function(wrap_pyfunction!(py_index, m)?)?;
    // Composite
    m.add_function(wrap_pyfunction!(py_struct, m)?)?;
    m.add_function(wrap_pyfunction!(py_sequence, m)?)?;
    m.add_function(wrap_pyfunction!(py_array, m)?)?;
    m.add_function(wrap_pyfunction!(py_greedy_range, m)?)?;
    m.add_function(wrap_pyfunction!(py_repeat_until, m)?)?;
    m.add_function(wrap_pyfunction!(py_union, m)?)?;
    m.add_function(wrap_pyfunction!(py_select, m)?)?;
    m.add_function(wrap_pyfunction!(py_focused_seq, m)?)?;
    m.add_function(wrap_pyfunction!(py_padded, m)?)?;
    m.add_function(wrap_pyfunction!(py_aligned, m)?)?;
    m.add_function(wrap_pyfunction!(py_padding, m)?)?;
    m.add_function(wrap_pyfunction!(py_aligned_struct, m)?)?;
    // Adapter / Control flow / Enum / Hex (10.7)
    m.add_function(wrap_pyfunction!(py_if_then_else, m)?)?;
    m.add_function(wrap_pyfunction!(py_if, m)?)?;
    m.add_function(wrap_pyfunction!(py_optional, m)?)?;
    m.add_function(wrap_pyfunction!(py_switch, m)?)?;
    m.add_function(wrap_pyfunction!(py_check, m)?)?;
    m.add_function(wrap_pyfunction!(py_stop_if, m)?)?;
    m.add_function(wrap_pyfunction!(py_enum, m)?)?;
    m.add_function(wrap_pyfunction!(py_flags_enum, m)?)?;
    m.add_function(wrap_pyfunction!(py_mapping, m)?)?;
    m.add_function(wrap_pyfunction!(py_hex, m)?)?;
    m.add_function(wrap_pyfunction!(py_hex_dump, m)?)?;
    m.add_function(wrap_pyfunction!(py_expr_adapter, m)?)?;
    m.add_function(wrap_pyfunction!(py_symmetric_adapter, m)?)?;
    m.add_function(wrap_pyfunction!(py_expr_symmetric_adapter, m)?)?;
    m.add_function(wrap_pyfunction!(py_validator, m)?)?;
    m.add_function(wrap_pyfunction!(py_expr_validator, m)?)?;
    m.add_function(wrap_pyfunction!(py_one_of, m)?)?;
    m.add_function(wrap_pyfunction!(py_none_of, m)?)?;
    m.add_function(wrap_pyfunction!(py_filter, m)?)?;
    m.add_function(wrap_pyfunction!(py_slicing, m)?)?;
    m.add_function(wrap_pyfunction!(py_indexing, m)?)?;
    // Stream operations / tunnel / lazy (10.8)
    m.add_function(wrap_pyfunction!(py_bitwise, m)?)?;
    m.add_function(wrap_pyfunction!(py_bytewise, m)?)?;
    m.add_function(wrap_pyfunction!(py_pointer, m)?)?;
    m.add_function(wrap_pyfunction!(py_peek, m)?)?;
    m.add_function(wrap_pyfunction!(py_raw_copy, m)?)?;
    m.add_function(wrap_pyfunction!(py_prefixed, m)?)?;
    m.add_function(wrap_pyfunction!(py_transformed, m)?)?;
    m.add_function(wrap_pyfunction!(py_restreamed, m)?)?;
    m.add_function(wrap_pyfunction!(py_compressed, m)?)?;
    m.add_function(wrap_pyfunction!(py_checksum, m)?)?;
    m.add_function(wrap_pyfunction!(py_byte_swapped, m)?)?;
    m.add_function(wrap_pyfunction!(py_bits_swapped, m)?)?;
    m.add_function(wrap_pyfunction!(py_fixed_sized, m)?)?;
    m.add_function(wrap_pyfunction!(py_lazy, m)?)?;
    m.add_function(wrap_pyfunction!(py_lazy_struct, m)?)?;
    m.add_function(wrap_pyfunction!(py_lazy_array, m)?)?;
    m.add_function(wrap_pyfunction!(py_lazy_bound, m)?)?;
    m.add_function(wrap_pyfunction!(py_rebuffered, m)?)?;
    m.add_function(wrap_pyfunction!(py_bit_struct, m)?)?;
    m.add_function(wrap_pyfunction!(py_prefixed_array, m)?)?;
    // String constructs (10.9)
    m.add_function(wrap_pyfunction!(py_padded_string, m)?)?;
    m.add_function(wrap_pyfunction!(py_c_string, m)?)?;
    // Gallery (10.9)
    m.add_function(wrap_pyfunction!(py_elf, m)?)?;
    m.add_function(wrap_pyfunction!(py_pe32file, m)?)?;
    m.add_function(wrap_pyfunction!(py_ut_index, m)?)?;
    // No-argument types registered as pre-created instances
    m.add("GreedyBytes", Py::new(py, py_greedy_bytes())?)?;
    m.add("VarInt", Py::new(py, py_varint())?)?;
    m.add("ZigZag", Py::new(py, py_zigzag())?)?;
    m.add("Flag", Py::new(py, py_flag())?)?;
    m.add("Pass", Py::new(py, py_pass())?)?;
    m.add("Terminated", Py::new(py, py_terminated())?)?;
    m.add("Tell", Py::new(py, py_tell())?)?;
    Ok(())
}

/// Registers 35 FormatField constants + 4 BytesInteger + 3 BitsInteger + 7 aliases.
fn register_constants(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    use construct::constructs::bytes_integer as bi;
    use construct::constructs::format_field as ff;
    use constructs_atomic::{PyBitsInteger, PyBytesInteger, PyFormatField};

    // FormatField constants — Python naming (Int8ub, not INT8UB)
    m.add("Int8ub", Py::new(py, PyFormatField { inner: ff::INT8UB })?)?;
    m.add(
        "Int16ub",
        Py::new(py, PyFormatField { inner: ff::INT16UB })?,
    )?;
    m.add(
        "Int32ub",
        Py::new(py, PyFormatField { inner: ff::INT32UB })?,
    )?;
    m.add(
        "Int64ub",
        Py::new(py, PyFormatField { inner: ff::INT64UB })?,
    )?;
    m.add("Int8sb", Py::new(py, PyFormatField { inner: ff::INT8SB })?)?;
    m.add(
        "Int16sb",
        Py::new(py, PyFormatField { inner: ff::INT16SB })?,
    )?;
    m.add(
        "Int32sb",
        Py::new(py, PyFormatField { inner: ff::INT32SB })?,
    )?;
    m.add(
        "Int64sb",
        Py::new(py, PyFormatField { inner: ff::INT64SB })?,
    )?;
    m.add("Int8ul", Py::new(py, PyFormatField { inner: ff::INT8UL })?)?;
    m.add(
        "Int16ul",
        Py::new(py, PyFormatField { inner: ff::INT16UL })?,
    )?;
    m.add(
        "Int32ul",
        Py::new(py, PyFormatField { inner: ff::INT32UL })?,
    )?;
    m.add(
        "Int64ul",
        Py::new(py, PyFormatField { inner: ff::INT64UL })?,
    )?;
    m.add("Int8sl", Py::new(py, PyFormatField { inner: ff::INT8SL })?)?;
    m.add(
        "Int16sl",
        Py::new(py, PyFormatField { inner: ff::INT16SL })?,
    )?;
    m.add(
        "Int32sl",
        Py::new(py, PyFormatField { inner: ff::INT32SL })?,
    )?;
    m.add(
        "Int64sl",
        Py::new(py, PyFormatField { inner: ff::INT64SL })?,
    )?;
    m.add("Int8un", Py::new(py, PyFormatField { inner: ff::INT8UN })?)?;
    m.add(
        "Int16un",
        Py::new(py, PyFormatField { inner: ff::INT16UN })?,
    )?;
    m.add(
        "Int32un",
        Py::new(py, PyFormatField { inner: ff::INT32UN })?,
    )?;
    m.add(
        "Int64un",
        Py::new(py, PyFormatField { inner: ff::INT64UN })?,
    )?;
    m.add("Int8sn", Py::new(py, PyFormatField { inner: ff::INT8SN })?)?;
    m.add(
        "Int16sn",
        Py::new(py, PyFormatField { inner: ff::INT16SN })?,
    )?;
    m.add(
        "Int32sn",
        Py::new(py, PyFormatField { inner: ff::INT32SN })?,
    )?;
    m.add(
        "Int64sn",
        Py::new(py, PyFormatField { inner: ff::INT64SN })?,
    )?;
    m.add(
        "Float16b",
        Py::new(
            py,
            PyFormatField {
                inner: ff::FLOAT16B,
            },
        )?,
    )?;
    m.add(
        "Float32b",
        Py::new(
            py,
            PyFormatField {
                inner: ff::FLOAT32B,
            },
        )?,
    )?;
    m.add(
        "Float64b",
        Py::new(
            py,
            PyFormatField {
                inner: ff::FLOAT64B,
            },
        )?,
    )?;
    m.add(
        "Float16l",
        Py::new(
            py,
            PyFormatField {
                inner: ff::FLOAT16L,
            },
        )?,
    )?;
    m.add(
        "Float32l",
        Py::new(
            py,
            PyFormatField {
                inner: ff::FLOAT32L,
            },
        )?,
    )?;
    m.add(
        "Float64l",
        Py::new(
            py,
            PyFormatField {
                inner: ff::FLOAT64L,
            },
        )?,
    )?;
    m.add(
        "Float16n",
        Py::new(
            py,
            PyFormatField {
                inner: ff::FLOAT16N,
            },
        )?,
    )?;
    m.add(
        "Float32n",
        Py::new(
            py,
            PyFormatField {
                inner: ff::FLOAT32N,
            },
        )?,
    )?;
    m.add(
        "Float64n",
        Py::new(
            py,
            PyFormatField {
                inner: ff::FLOAT64N,
            },
        )?,
    )?;

    // Aliases — reference the same objects
    m.add("Byte", m.getattr("Int8ub")?)?;
    m.add("Short", m.getattr("Int16ub")?)?;
    m.add("Int", m.getattr("Int32ub")?)?;
    m.add("Long", m.getattr("Int64ub")?)?;
    m.add("Half", m.getattr("Float16b")?)?;
    m.add("Single", m.getattr("Float32b")?)?;
    m.add("Double", m.getattr("Float64b")?)?;

    // BytesInteger constants
    m.add(
        "Int24ub",
        Py::new(py, PyBytesInteger { inner: bi::INT24UB })?,
    )?;
    m.add(
        "Int24ul",
        Py::new(py, PyBytesInteger { inner: bi::INT24UL })?,
    )?;
    m.add(
        "Int24sb",
        Py::new(py, PyBytesInteger { inner: bi::INT24SB })?,
    )?;
    m.add(
        "Int24sl",
        Py::new(py, PyBytesInteger { inner: bi::INT24SL })?,
    )?;

    // BitsInteger constants
    m.add("Bit", Py::new(py, PyBitsInteger { inner: bi::BIT })?)?;
    m.add("Nibble", Py::new(py, PyBitsInteger { inner: bi::NIBBLE })?)?;
    m.add("Octet", Py::new(py, PyBitsInteger { inner: bi::OCTET })?)?;
    Ok(())
}

/// PyO3 module entry.
#[pymodule]
fn _core(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    register_exceptions(py, m)?;
    register_classes(m)?;
    register_functions(py, m)?;
    register_constants(py, m)?;
    Ok(())
}
