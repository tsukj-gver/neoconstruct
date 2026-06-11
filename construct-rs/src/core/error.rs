//! Error types for all construct operations.
//!
//! This module defines the unified [`ConstructError`] enum that covers every
//! error condition a construct may raise during parse, build, or sizeof
//! operations. Every variant carries a `path` field that records the location
//! of the failure inside a nested structure (e.g. `"header > magic"`), so the
//! full chain of responsibility can be traced when something goes wrong.
//!
//! See `docs/模块设计-错误体系.md` for the full design rationale and the
//! mapping between Python exception classes and the Rust variants defined here.

use std::string::FromUtf8Error;

/// Joins a parent path prefix with an existing child path.
///
/// When the existing path is empty (for example an error freshly created from a
/// raw [`std::io::Error`] via [`ConstructError::from`]), the prefix is used as
/// is to avoid a trailing separator. Otherwise the two parts are joined with a
/// `.` separator, matching the path scheme used by [`ConstructError`].
fn join_path(prefix: &str, path: &str) -> String {
    if path.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}.{path}")
    }
}

/// The unified error type for every operation in the `construct` library.
///
/// All variants carry a `path: String` field that records where the failure
/// occurred within a nested construct. Parent constructs enrich a child error's
/// path via [`ConstructError::with_path_prefix`] during propagation.
///
/// The error corresponds to the root `ConstructError` exception and its
/// subclasses from the Python original.
#[derive(Debug, thiserror::Error)]
pub enum ConstructError {
    /// A generic, uncategorised error.
    ///
    /// Corresponds to the Python `ConstructError` root exception.
    #[error("construct error at {path}: {message}")]
    Generic {
        /// Location of the failure inside the construct tree.
        path: String,
        /// Human-readable description of the failure.
        message: String,
    },

    /// A low-level stream read/write/seek failure.
    ///
    /// Corresponds to the Python `StreamError`. Wraps the underlying
    /// [`std::io::Error`] as its source.
    #[error("stream error at {path}: {source}")]
    Stream {
        /// Location of the failure inside the construct tree.
        path: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// A format field (e.g. a `struct`-style primitive) parse/build failure.
    ///
    /// Corresponds to the Python `FormatFieldError`. Typically raised when the
    /// number of bytes read/written does not match the expected width.
    #[error("format field error at {path}: expected {expected} bytes but got {actual}")]
    FormatField {
        /// Location of the failure inside the construct tree.
        path: String,
        /// Expected number of bytes.
        expected: usize,
        /// Actual number of bytes observed.
        actual: usize,
    },

    /// A failure to compute the static size of a construct.
    ///
    /// Corresponds to the Python `SizeofError`. Raised when a construct's size
    /// cannot be determined a priori (for example a variable-length field).
    #[error("cannot compute sizeof at {path}: {reason}")]
    Sizeof {
        /// Location of the failure inside the construct tree.
        path: String,
        /// Why the size could not be computed.
        reason: String,
    },

    /// A constant value mismatch.
    ///
    /// Corresponds to the Python `ConstError`. `expected` and `actual` hold the
    /// debug representations of the two values for a readable comparison.
    #[error("const error at {path}: expected {expected:?} but got {actual:?}")]
    Const {
        /// Location of the failure inside the construct tree.
        path: String,
        /// Debug representation of the expected value.
        expected: String,
        /// Debug representation of the actual value.
        actual: String,
    },

    /// A value validation failure.
    ///
    /// Corresponds to the Python `ValidationError`. Raised by validator
    /// constructs such as `OneOf` / `NoneOf`.
    #[error("validation failed at {path}: {message}")]
    Validation {
        /// Location of the failure inside the construct tree.
        path: String,
        /// Human-readable description of why the value is invalid.
        message: String,
    },

    /// An array/repeat construct length mismatch.
    ///
    /// Corresponds to the Python `RangeError` / `RepeatError` family. Raised
    /// when the number of elements supplied or parsed does not match the count
    /// required by the construct.
    #[error("array error at {path}: expected {expected} elements but got {actual}")]
    Array {
        /// Location of the failure inside the construct tree.
        path: String,
        /// Expected number of elements.
        expected: usize,
        /// Actual number of elements.
        actual: usize,
    },

    /// An index out of range.
    ///
    /// Corresponds to the Python `IndexFieldError` and general index lookups.
    #[error("index out of range at {path}: index {index} in list of length {length}")]
    Index {
        /// Location of the failure inside the construct tree.
        path: String,
        /// The offending index.
        index: usize,
        /// The length of the collection being indexed.
        length: usize,
    },

    /// A padding/alignment failure.
    ///
    /// Corresponds to the Python `PaddingError`. Raised by `Padding`,
    /// `Padded`, `Aligned`, `NullTerminated`, etc.
    #[error("padding error at {path}: {message}")]
    Padding {
        /// Location of the failure inside the construct tree.
        path: String,
        /// Human-readable description of the padding failure.
        message: String,
    },

    /// A check/assertion failure.
    ///
    /// Corresponds to the Python `CheckError` and `ExplicitError`.
    #[error("check failed at {path}: {message}")]
    Check {
        /// Location of the failure inside the construct tree.
        path: String,
        /// Human-readable description of the failed check.
        message: String,
    },

    /// A stream that was expected to be at end-of-file but was not.
    ///
    /// Corresponds to the Python `TerminatedError`.
    #[error("stream not terminated at {path}: {remaining} bytes remaining")]
    Terminated {
        /// Location of the failure inside the construct tree.
        path: String,
        /// Number of bytes left in the stream past the expected terminator.
        remaining: usize,
    },

    /// A string encoding/decoding failure.
    ///
    /// Corresponds to the Python `StringError`. Wraps the underlying
    /// [`FromUtf8Error`] as its source.
    #[error("string encoding error at {path}: {source}")]
    StringEncoding {
        /// Location of the failure inside the construct tree.
        path: String,
        /// The underlying UTF-8 conversion error.
        #[source]
        source: FromUtf8Error,
    },

    /// A mapping lookup failure.
    ///
    /// Corresponds to the Python `MappingError`. Raised by `Enum`,
    /// `FlagsEnum`, etc. when a build value has no mapping.
    #[error("mapping error at {path}: key {key:?} not found in mapping")]
    Mapping {
        /// Location of the failure inside the construct tree.
        path: String,
        /// Debug representation of the unmapped key.
        key: String,
    },

    /// An expression evaluation failure.
    ///
    /// Corresponds to errors raised by the expression system (`this`/`obj_`
    /// references, field lookups, arithmetic, etc.).
    #[error("expression error at {path}: {message}")]
    Expr {
        /// Location of the failure inside the construct tree.
        path: String,
        /// Human-readable description of the expression failure.
        message: String,
    },

    /// A `Union` construct failure.
    ///
    /// Corresponds to the Python `UnionError`. Raised when none of the union
    /// members was successfully selected or no value was supplied for building.
    #[error("union error at {path}: {message}")]
    Union {
        /// Location of the failure inside the construct tree.
        path: String,
        /// Human-readable description of the union failure.
        message: String,
    },

    /// A `Switch` construct failure.
    ///
    /// Corresponds to the Python `SwitchError`. Raised when no branch matches
    /// the selector key.
    #[error("switch error at {path}: no branch for key {key:?}")]
    Switch {
        /// Location of the failure inside the construct tree.
        path: String,
        /// Debug representation of the unmatched selector key.
        key: String,
    },

    /// A required field missing from a container during build.
    ///
    /// Corresponds to the Python `FieldError`.
    #[error("field missing at {path}: field {field:?} not found in container")]
    FieldMissing {
        /// Location of the failure inside the construct tree.
        path: String,
        /// The missing field name.
        field: String,
    },

    /// A value whose runtime type did not match the construct's expectation.
    #[error("type mismatch at {path}: expected {expected} but got {actual}")]
    TypeMismatch {
        /// Location of the failure inside the construct tree.
        path: String,
        /// Description of the expected type.
        expected: String,
        /// Description of the actual type.
        actual: String,
    },
}

impl ConstructError {
    /// Returns the path where the error occurred.
    ///
    /// Every variant carries a `path`; this is a convenience accessor that does
    /// not require matching on the variant.
    #[must_use]
    pub fn path(&self) -> &str {
        match self {
            ConstructError::Generic { path, .. }
            | ConstructError::Stream { path, .. }
            | ConstructError::FormatField { path, .. }
            | ConstructError::Sizeof { path, .. }
            | ConstructError::Const { path, .. }
            | ConstructError::Validation { path, .. }
            | ConstructError::Array { path, .. }
            | ConstructError::Index { path, .. }
            | ConstructError::Padding { path, .. }
            | ConstructError::Check { path, .. }
            | ConstructError::Terminated { path, .. }
            | ConstructError::StringEncoding { path, .. }
            | ConstructError::Mapping { path, .. }
            | ConstructError::Expr { path, .. }
            | ConstructError::Union { path, .. }
            | ConstructError::Switch { path, .. }
            | ConstructError::FieldMissing { path, .. }
            | ConstructError::TypeMismatch { path, .. } => path,
        }
    }

    /// Prepends a parent path to this error's path and returns the enriched
    /// error.
    ///
    /// Parent constructs call this when propagating a child error so that the
    /// final error records the full chain of responsibility. The error is
    /// consumed (taken by value) so that non-`Clone` sources such as
    /// [`std::io::Error`] and [`FromUtf8Error`] are preserved verbatim.
    ///
    /// # Example
    ///
    /// ```
    /// # use construct::core::error::ConstructError;
    /// let inner = ConstructError::Const {
    ///     path: "magic".to_string(),
    ///     expected: "b\"PNG\"".to_string(),
    ///     actual: "b\"JPG\"".to_string(),
    /// };
    /// let outer = inner.with_path_prefix("header");
    /// assert_eq!(outer.path(), "header.magic");
    /// ```
    #[must_use]
    pub fn with_path_prefix(self, prefix: &str) -> Self {
        match self {
            ConstructError::Generic { path, message } => ConstructError::Generic {
                path: join_path(prefix, &path),
                message,
            },
            ConstructError::Stream { path, source } => ConstructError::Stream {
                path: join_path(prefix, &path),
                source,
            },
            ConstructError::FormatField {
                path,
                expected,
                actual,
            } => ConstructError::FormatField {
                path: join_path(prefix, &path),
                expected,
                actual,
            },
            ConstructError::Sizeof { path, reason } => ConstructError::Sizeof {
                path: join_path(prefix, &path),
                reason,
            },
            ConstructError::Const {
                path,
                expected,
                actual,
            } => ConstructError::Const {
                path: join_path(prefix, &path),
                expected,
                actual,
            },
            ConstructError::Validation { path, message } => ConstructError::Validation {
                path: join_path(prefix, &path),
                message,
            },
            ConstructError::Array {
                path,
                expected,
                actual,
            } => ConstructError::Array {
                path: join_path(prefix, &path),
                expected,
                actual,
            },
            ConstructError::Index {
                path,
                index,
                length,
            } => ConstructError::Index {
                path: join_path(prefix, &path),
                index,
                length,
            },
            ConstructError::Padding { path, message } => ConstructError::Padding {
                path: join_path(prefix, &path),
                message,
            },
            ConstructError::Check { path, message } => ConstructError::Check {
                path: join_path(prefix, &path),
                message,
            },
            ConstructError::Terminated { path, remaining } => ConstructError::Terminated {
                path: join_path(prefix, &path),
                remaining,
            },
            ConstructError::StringEncoding { path, source } => ConstructError::StringEncoding {
                path: join_path(prefix, &path),
                source,
            },
            ConstructError::Mapping { path, key } => ConstructError::Mapping {
                path: join_path(prefix, &path),
                key,
            },
            ConstructError::Expr { path, message } => ConstructError::Expr {
                path: join_path(prefix, &path),
                message,
            },
            ConstructError::Union { path, message } => ConstructError::Union {
                path: join_path(prefix, &path),
                message,
            },
            ConstructError::Switch { path, key } => ConstructError::Switch {
                path: join_path(prefix, &path),
                key,
            },
            ConstructError::FieldMissing { path, field } => ConstructError::FieldMissing {
                path: join_path(prefix, &path),
                field,
            },
            ConstructError::TypeMismatch {
                path,
                expected,
                actual,
            } => ConstructError::TypeMismatch {
                path: join_path(prefix, &path),
                expected,
                actual,
            },
        }
    }
}

impl From<std::io::Error> for ConstructError {
    /// Wraps a raw [`std::io::Error`] into a [`ConstructError::Stream`] with an
    /// empty path. The path is filled in later as the error propagates up the
    /// construct tree.
    fn from(err: std::io::Error) -> Self {
        ConstructError::Stream {
            path: String::new(),
            source: err,
        }
    }
}

/// Convenience alias for `Result` values produced by construct operations.
pub type Result<T> = std::result::Result<T, ConstructError>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    // -- Construction + Display for every variant ---------------------------

    #[test]
    fn generic_display_and_path() {
        let err = ConstructError::Generic {
            path: "root".to_string(),
            message: "something went wrong".to_string(),
        };
        assert_eq!(err.path(), "root");
        assert_eq!(
            err.to_string(),
            "construct error at root: something went wrong"
        );
    }

    #[test]
    fn stream_display_and_path() {
        let io_err = std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "eof");
        let err = ConstructError::Stream {
            path: "f".to_string(),
            source: io_err,
        };
        assert_eq!(err.path(), "f");
        assert!(err.to_string().starts_with("stream error at f: "));
    }

    #[test]
    fn format_field_display_and_path() {
        let err = ConstructError::FormatField {
            path: "u32".to_string(),
            expected: 4,
            actual: 2,
        };
        assert_eq!(err.path(), "u32");
        assert_eq!(
            err.to_string(),
            "format field error at u32: expected 4 bytes but got 2"
        );
    }

    #[test]
    fn sizeof_display_and_path() {
        let err = ConstructError::Sizeof {
            path: "p".to_string(),
            reason: "variable length".to_string(),
        };
        assert_eq!(err.path(), "p");
        assert_eq!(
            err.to_string(),
            "cannot compute sizeof at p: variable length"
        );
    }

    #[test]
    fn const_display_and_path() {
        let err = ConstructError::Const {
            path: "magic".to_string(),
            expected: "b\"PNG\"".to_string(),
            actual: "b\"JPG\"".to_string(),
        };
        assert_eq!(err.path(), "magic");
        assert_eq!(
            err.to_string(),
            r#"const error at magic: expected "b\"PNG\"" but got "b\"JPG\"""#
        );
    }

    #[test]
    fn validation_display_and_path() {
        let err = ConstructError::Validation {
            path: "v".to_string(),
            message: "not in allowed set".to_string(),
        };
        assert_eq!(err.path(), "v");
        assert_eq!(
            err.to_string(),
            "validation failed at v: not in allowed set"
        );
    }

    #[test]
    fn array_display_and_path() {
        let err = ConstructError::Array {
            path: "arr".to_string(),
            expected: 3,
            actual: 1,
        };
        assert_eq!(err.path(), "arr");
        assert_eq!(
            err.to_string(),
            "array error at arr: expected 3 elements but got 1"
        );
    }

    #[test]
    fn index_display_and_path() {
        let err = ConstructError::Index {
            path: "idx".to_string(),
            index: 5,
            length: 3,
        };
        assert_eq!(err.path(), "idx");
        assert_eq!(
            err.to_string(),
            "index out of range at idx: index 5 in list of length 3"
        );
    }

    #[test]
    fn padding_display_and_path() {
        let err = ConstructError::Padding {
            path: "pad".to_string(),
            message: "data too long".to_string(),
        };
        assert_eq!(err.path(), "pad");
        assert_eq!(err.to_string(), "padding error at pad: data too long");
    }

    #[test]
    fn check_display_and_path() {
        let err = ConstructError::Check {
            path: "chk".to_string(),
            message: "assertion failed".to_string(),
        };
        assert_eq!(err.path(), "chk");
        assert_eq!(err.to_string(), "check failed at chk: assertion failed");
    }

    #[test]
    fn terminated_display_and_path() {
        let err = ConstructError::Terminated {
            path: "t".to_string(),
            remaining: 7,
        };
        assert_eq!(err.path(), "t");
        assert_eq!(
            err.to_string(),
            "stream not terminated at t: 7 bytes remaining"
        );
    }

    #[test]
    fn string_encoding_display_and_path() {
        let bad = vec![0xFF, 0xFE, 0xFD];
        let utf8_err = String::from_utf8(bad).unwrap_err();
        let err = ConstructError::StringEncoding {
            path: "s".to_string(),
            source: utf8_err,
        };
        assert_eq!(err.path(), "s");
        assert!(err.to_string().starts_with("string encoding error at s: "));
    }

    #[test]
    fn mapping_display_and_path() {
        let err = ConstructError::Mapping {
            path: "enum".to_string(),
            key: "42".to_string(),
        };
        assert_eq!(err.path(), "enum");
        assert_eq!(
            err.to_string(),
            r#"mapping error at enum: key "42" not found in mapping"#
        );
    }

    #[test]
    fn expr_display_and_path() {
        let err = ConstructError::Expr {
            path: "e".to_string(),
            message: "division by zero".to_string(),
        };
        assert_eq!(err.path(), "e");
        assert_eq!(err.to_string(), "expression error at e: division by zero");
    }

    #[test]
    fn union_display_and_path() {
        let err = ConstructError::Union {
            path: "u".to_string(),
            message: "no parse".to_string(),
        };
        assert_eq!(err.path(), "u");
        assert_eq!(err.to_string(), "union error at u: no parse");
    }

    #[test]
    fn switch_display_and_path() {
        let err = ConstructError::Switch {
            path: "sw".to_string(),
            key: "3".to_string(),
        };
        assert_eq!(err.path(), "sw");
        assert_eq!(
            err.to_string(),
            r#"switch error at sw: no branch for key "3""#
        );
    }

    #[test]
    fn field_missing_display_and_path() {
        let err = ConstructError::FieldMissing {
            path: "st".to_string(),
            field: "version".to_string(),
        };
        assert_eq!(err.path(), "st");
        assert_eq!(
            err.to_string(),
            r#"field missing at st: field "version" not found in container"#
        );
    }

    #[test]
    fn type_mismatch_display_and_path() {
        let err = ConstructError::TypeMismatch {
            path: "tm".to_string(),
            expected: "Int".to_string(),
            actual: "Bytes".to_string(),
        };
        assert_eq!(err.path(), "tm");
        assert_eq!(
            err.to_string(),
            "type mismatch at tm: expected Int but got Bytes"
        );
    }

    // -- path() over every variant -----------------------------------------

    #[test]
    fn path_accessor_covers_all_variants() {
        let cases: Vec<(ConstructError, &str)> = vec![
            (
                ConstructError::Generic {
                    path: "g".into(),
                    message: String::new(),
                },
                "g",
            ),
            (
                ConstructError::Stream {
                    path: "s".into(),
                    source: std::io::Error::from(std::io::ErrorKind::Other),
                },
                "s",
            ),
            (
                ConstructError::FormatField {
                    path: "ff".into(),
                    expected: 0,
                    actual: 0,
                },
                "ff",
            ),
            (
                ConstructError::Sizeof {
                    path: "so".into(),
                    reason: String::new(),
                },
                "so",
            ),
            (
                ConstructError::Const {
                    path: "c".into(),
                    expected: String::new(),
                    actual: String::new(),
                },
                "c",
            ),
            (
                ConstructError::Validation {
                    path: "v".into(),
                    message: String::new(),
                },
                "v",
            ),
            (
                ConstructError::Array {
                    path: "a".into(),
                    expected: 0,
                    actual: 0,
                },
                "a",
            ),
            (
                ConstructError::Index {
                    path: "i".into(),
                    index: 0,
                    length: 0,
                },
                "i",
            ),
            (
                ConstructError::Padding {
                    path: "p".into(),
                    message: String::new(),
                },
                "p",
            ),
            (
                ConstructError::Check {
                    path: "ch".into(),
                    message: String::new(),
                },
                "ch",
            ),
            (
                ConstructError::Terminated {
                    path: "t".into(),
                    remaining: 0,
                },
                "t",
            ),
            (
                ConstructError::Mapping {
                    path: "m".into(),
                    key: String::new(),
                },
                "m",
            ),
            (
                ConstructError::Expr {
                    path: "e".into(),
                    message: String::new(),
                },
                "e",
            ),
            (
                ConstructError::Union {
                    path: "u".into(),
                    message: String::new(),
                },
                "u",
            ),
            (
                ConstructError::Switch {
                    path: "sw".into(),
                    key: String::new(),
                },
                "sw",
            ),
            (
                ConstructError::FieldMissing {
                    path: "fm".into(),
                    field: String::new(),
                },
                "fm",
            ),
            (
                ConstructError::TypeMismatch {
                    path: "tm".into(),
                    expected: String::new(),
                    actual: String::new(),
                },
                "tm",
            ),
        ];
        for (err, expected_path) in cases {
            assert_eq!(err.path(), expected_path);
        }
    }

    // -- with_path_prefix --------------------------------------------------

    #[test]
    fn with_path_prefix_joins_paths() {
        let err = ConstructError::Const {
            path: "magic".to_string(),
            expected: "b\"PNG\"".to_string(),
            actual: "b\"JPG\"".to_string(),
        };
        let outer = err.with_path_prefix("header");
        assert_eq!(outer.path(), "header.magic");
        // other fields preserved
        match outer {
            ConstructError::Const {
                expected, actual, ..
            } => {
                assert_eq!(expected, "b\"PNG\"");
                assert_eq!(actual, "b\"JPG\"");
            }
            _ => panic!("expected Const variant"),
        }
    }

    #[test]
    fn with_path_prefix_on_empty_path_uses_prefix_only() {
        // An io::Error converted via From has an empty path.
        let io_err = std::io::Error::from(std::io::ErrorKind::UnexpectedEof);
        let err: ConstructError = io_err.into();
        assert_eq!(err.path(), "");
        let outer = err.with_path_prefix("root");
        assert_eq!(outer.path(), "root");
    }

    #[test]
    fn with_path_prefix_preserves_io_source() {
        let io_err = std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "eof");
        let err = ConstructError::Stream {
            path: "child".to_string(),
            source: io_err,
        };
        let outer = err.with_path_prefix("parent");
        assert_eq!(outer.path(), "parent.child");
        match outer {
            ConstructError::Stream { source, .. } => {
                assert_eq!(source.kind(), std::io::ErrorKind::UnexpectedEof);
            }
            _ => panic!("expected Stream variant"),
        }
    }

    #[test]
    fn with_path_prefix_preserves_utf8_source() {
        let utf8_err = String::from_utf8(vec![0xC3, 0x28]).unwrap_err();
        let err = ConstructError::StringEncoding {
            path: "name".to_string(),
            source: utf8_err,
        };
        let outer = err.with_path_prefix("record");
        assert_eq!(outer.path(), "record.name");
        assert!(outer.source().is_some());
    }

    // -- From<io::Error> ---------------------------------------------------

    #[test]
    fn from_io_error_produces_stream_with_empty_path() {
        let io_err = std::io::Error::new(std::io::ErrorKind::InvalidData, "bad");
        let err: ConstructError = io_err.into();
        match err {
            ConstructError::Stream { path, source } => {
                assert!(path.is_empty());
                assert_eq!(source.kind(), std::io::ErrorKind::InvalidData);
            }
            _ => panic!("expected Stream variant"),
        }
    }

    // -- std::error::Error::source() ---------------------------------------

    #[test]
    fn stream_source_is_available() {
        let err = ConstructError::Stream {
            path: "x".to_string(),
            source: std::io::Error::new(std::io::ErrorKind::Other, "boom"),
        };
        let src = err.source();
        assert!(src.is_some());
        assert!(src.unwrap().to_string().contains("boom"));
    }

    #[test]
    fn string_encoding_source_is_available() {
        let utf8_err = String::from_utf8(vec![0xFF]).unwrap_err();
        let err = ConstructError::StringEncoding {
            path: "y".to_string(),
            source: utf8_err,
        };
        assert!(err.source().is_some());
    }

    #[test]
    fn generic_has_no_source() {
        let err = ConstructError::Generic {
            path: "z".to_string(),
            message: "none".to_string(),
        };
        assert!(err.source().is_none());
    }

    // -- Result alias + ? propagation --------------------------------------

    #[test]
    fn result_alias_propagates_io_error() {
        fn fallible() -> Result<()> {
            // Trigger an io error inside a Result-returning function.
            std::io::Cursor::new(&b""[..]).read_exact(&mut [0u8; 4])?;
            Ok(())
        }
        let err = fallible().unwrap_err();
        assert!(matches!(err, ConstructError::Stream { .. }));
        assert_eq!(err.path(), "");
    }

    // -- Debug + Send + Sync (object-safety / cross-thread) -----------------

    #[test]
    fn error_implements_debug_send_sync() {
        fn assert_send_sync<T: std::fmt::Debug + Send + Sync + 'static>() {}
        assert_send_sync::<ConstructError>();

        let err = ConstructError::Generic {
            path: "d".to_string(),
            message: "debug".to_string(),
        };
        // Debug formats without panic.
        let _ = format!("{err:?}");
    }

    // bring read_exact into scope for the propagation test
    use std::io::Read;
}
