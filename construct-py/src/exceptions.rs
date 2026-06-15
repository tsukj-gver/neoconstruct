//! Python exception hierarchy for construct_rust.
//!
//! Defines 28 Python exception classes mirroring the Python original's
//! `construct.core` exception hierarchy (core.py lines 13-154), plus the
//! `rust_err_to_py` function that maps Rust [`ConstructError`] variants to
//! the appropriate Python exception.

// The create_exception! macro expands with internal cfg(gil-refs) checks
// that are expected by the pyo3 crate but not registered in our crate's
// cfg surface. Suppress the resulting unexpected_cfgs warnings.
#![allow(unexpected_cfgs)]

use construct::core::error::ConstructError as RustConstructError;
use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyOverflowError, PyTypeError};
use pyo3::prelude::*;

// ---------------------------------------------------------------------------
// Exception class hierarchy (28 classes)
// ---------------------------------------------------------------------------

create_exception!(construct_rust, ConstructError, PyException);
create_exception!(construct_rust, SizeofError, ConstructError);
create_exception!(construct_rust, AdaptationError, ConstructError);
create_exception!(construct_rust, ValidationError, ConstructError);
create_exception!(construct_rust, StreamError, ConstructError);
create_exception!(construct_rust, FormatFieldError, ConstructError);
create_exception!(construct_rust, IntegerError, ConstructError);
create_exception!(construct_rust, StringError, ConstructError);
create_exception!(construct_rust, MappingError, ConstructError);
create_exception!(construct_rust, RangeError, ConstructError);
create_exception!(construct_rust, RepeatError, ConstructError);
create_exception!(construct_rust, ConstError, ConstructError);
create_exception!(construct_rust, IndexFieldError, ConstructError);
create_exception!(construct_rust, CheckError, ConstructError);
create_exception!(construct_rust, ExplicitError, CheckError);
create_exception!(construct_rust, NamedTupleError, ConstructError);
create_exception!(construct_rust, TimestampError, ConstructError);
create_exception!(construct_rust, UnionError, ConstructError);
create_exception!(construct_rust, SelectError, ConstructError);
create_exception!(construct_rust, SwitchError, ConstructError);
create_exception!(construct_rust, StopFieldError, ConstructError);
create_exception!(construct_rust, PaddingError, ConstructError);
create_exception!(construct_rust, TerminatedError, ConstructError);
create_exception!(construct_rust, RawCopyError, ConstructError);
create_exception!(construct_rust, RotationError, ConstructError);
create_exception!(construct_rust, ChecksumError, ConstructError);
create_exception!(construct_rust, CancelParsing, ConstructError);
create_exception!(construct_rust, CipherError, ConstructError);

/// Formats an error message with an optional path prefix, mirroring the
/// Python original's `ConstructError.__init__` behaviour:
/// `"Error in path {path}\n{message}"` when path is non-empty.
fn format_with_path(path: &str, message: &str) -> String {
    if path.is_empty() {
        message.to_string()
    } else {
        format!("Error in path {path}\n{message}")
    }
}

/// Converts a Rust [`RustConstructError`] into the corresponding Python
/// [`PyErr`], selecting the appropriate exception subclass.
///
/// The mapping follows the design document §3.2. Variants without a dedicated
/// Python subclass (FieldMissing, TypeMismatch, Expr) fall back to the
/// `ConstructError` base class. `StopField` is an internal signal that should
/// not escape to Python -- if it does, it is converted to a generic
/// `ConstructError`.
#[allow(clippy::too_many_lines)]
pub fn rust_err_to_py(_py: Python<'_>, e: RustConstructError) -> PyErr {
    match e {
        RustConstructError::Generic { path, message } => {
            ConstructError::new_err(format_with_path(&path, &message))
        }
        RustConstructError::Stream { path, source } => {
            StreamError::new_err(format_with_path(&path, &source.to_string()))
        }
        RustConstructError::FormatField {
            path,
            expected,
            actual,
        } => FormatFieldError::new_err(format_with_path(
            &path,
            &format!("expected {expected} bytes but got {actual}"),
        )),
        RustConstructError::Integer { path, message } => {
            IntegerError::new_err(format_with_path(&path, &message))
        }
        RustConstructError::Sizeof { path, reason } => {
            SizeofError::new_err(format_with_path(&path, &reason))
        }
        RustConstructError::Const {
            path,
            expected,
            actual,
        } => ConstError::new_err(format_with_path(
            &path,
            &format!("expected {expected:?} but got {actual:?}"),
        )),
        RustConstructError::Validation { path, message } => {
            ValidationError::new_err(format_with_path(&path, &message))
        }
        RustConstructError::Array {
            path,
            expected,
            actual,
        } => {
            // RepeatUntil uses expected=0, actual=0 as sentinel for "no match".
            if expected == 0 && actual == 0 {
                crate::exceptions::RepeatError::new_err(format_with_path(
                    &path,
                    "no element satisfied the repeat predicate",
                ))
            } else {
                RangeError::new_err(format_with_path(
                    &path,
                    &format!("expected {expected} elements but got {actual}"),
                ))
            }
        }
        RustConstructError::Index {
            path,
            index,
            length,
        } => IndexFieldError::new_err(format_with_path(
            &path,
            &format!("index {index} in list of length {length}"),
        )),
        RustConstructError::Padding { path, message } => {
            PaddingError::new_err(format_with_path(&path, &message))
        }
        RustConstructError::Check { path, message } => {
            // Error construct raises ExplicitError (subclass of CheckError).
            if message.contains("Error field was activated") {
                ExplicitError::new_err(format_with_path(&path, &message))
            } else if message.contains("checksum") || message.contains("Checksum") {
                ChecksumError::new_err(format_with_path(&path, &message))
            } else {
                CheckError::new_err(format_with_path(&path, &message))
            }
        }
        RustConstructError::Terminated { path, remaining } => TerminatedError::new_err(
            format_with_path(&path, &format!("{remaining} bytes remaining")),
        ),
        RustConstructError::StringEncoding { path, source } => {
            StringError::new_err(format_with_path(&path, &source.to_string()))
        }
        RustConstructError::Mapping { path, key } => {
            MappingError::new_err(format_with_path(&path, &format!("key {key:?} not found")))
        }
        RustConstructError::Expr { path, message } => {
            // No dedicated Python subclass; use the base class.
            ConstructError::new_err(format_with_path(&path, &message))
        }
        RustConstructError::Union { path, message } => {
            UnionError::new_err(format_with_path(&path, &message))
        }
        RustConstructError::Switch { path, key } => SwitchError::new_err(format_with_path(
            &path,
            &format!("no branch for key {key:?}"),
        )),
        RustConstructError::FieldMissing { path, field } => {
            // Python's construct raises KeyError for missing dict/container keys.
            pyo3::exceptions::PyKeyError::new_err(format_with_path(&path, &format!("{field:?}")))
        }
        RustConstructError::TypeMismatch {
            path,
            expected,
            actual,
        } => {
            // No dedicated Python subclass; use the base class.
            ConstructError::new_err(format_with_path(
                &path,
                &format!("expected {expected} but got {actual}"),
            ))
        }
        RustConstructError::Select { path, message } => {
            SelectError::new_err(format_with_path(&path, &message))
        }
        RustConstructError::StopField { path } => {
            // Internal signal that should not escape to Python.
            ConstructError::new_err(format_with_path(
                &path,
                "internal stop field signal escaped to FFI boundary",
            ))
        }
    }
}

/// Creates a Python `TypeError` with the given message.
///
/// Convenience wrapper used by conversion functions.
pub fn py_type_error(msg: &str) -> PyErr {
    PyTypeError::new_err(msg.to_string())
}

/// Creates a Python `OverflowError` with the given message.
///
/// Convenience wrapper used by conversion functions.
pub fn py_overflow_error(msg: &str) -> PyErr {
    PyOverflowError::new_err(msg.to_string())
}
