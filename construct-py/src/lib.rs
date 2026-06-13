//! PyO3 module entry point for the `construct_rust` native extension.
//!
//! The compiled cdylib is loaded as `construct_rust._core` (see
//! `pyproject.toml` -> `module-name`). The pure-Python package
//! `construct_rust/__init__.py` re-exports everything via `from ._core import *`.
//!
//! Subsequent Phase 10 sub-tasks (10.3-10.9) register their `#[pyclass]` /
//! `#[pyfunction]` items inside the [`_core`] function body.

pub mod api;
pub mod conversions;
pub mod exceptions;
pub mod pystream;

use pyo3::prelude::*;

/// Initializes the Python interpreter for unit tests.
///
/// When the `extension-module` feature is not enabled (as during `cargo test`),
/// the Python interpreter must be explicitly initialized before any
/// `Python::with_gil` call. This function is safe to call multiple times.
#[cfg(test)]
pub fn ensure_python() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(pyo3::prepare_freethreaded_python);
}

/// Registers all exception classes as top-level module attributes.
///
/// Each exception is exported under its Python name so that
/// `from construct_rust import SizeofError` (etc.) works.
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

/// PyO3 module entry — registers the native extension symbols.
///
/// 10.2 registers the 28-class exception hierarchy. Downstream sub-tasks
/// (10.3+) will append `m.add_class::<X>()` calls here.
#[pymodule]
fn _core(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;

    register_exceptions(py, m)?;

    Ok(())
}
