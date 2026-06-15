//! Core module — error types, context container, stream abstraction, and
//! the central [`Construct`] trait.
//!
//! This module groups together the fundamental building blocks:
//! - [`error`] — the unified error type for all construct operations
//! - [`context`] — the context container used during parse/build
//! - [`stream`] — the stream abstraction for binary I/O
//! - [`Construct`] — the core trait that all constructors must implement
//! - [`Renamed`] — a wrapper that assigns a name to a construct
//! - [`Subconstruct`] — a base wrapper that delegates to an inner construct

pub mod context;
pub mod error;
pub mod stream;

use std::path::Path;

use crate::combined::CombinedConstruct;
use crate::core::context::Context;
use crate::core::error::Result;
use crate::core::stream::ByteStream;
use crate::core::stream::CombinedStream;
use crate::value::Value;

// ===========================================================================
// Construct trait
// ===========================================================================

/// The core trait that every constructor in the library must implement.
///
/// Provides three fundamental operations:
/// - [`parse`](Construct::parse) — reads binary data from a stream and returns
///   a [`Value`]
/// - [`build`](Construct::build) — writes binary data to a stream from a
///   [`Value`]
/// - [`sizeof`](Construct::sizeof) — computes the byte size of the construct
///
/// Corresponds to the Python `Construct` class
/// (`construct/construct/core.py` line ~321).
#[enum_dispatch::enum_dispatch]
pub trait Construct {
    /// Parses a [`Value`] from the given stream.
    ///
    /// Reads bytes from `stream` starting at the current position, using
    /// `ctx` for any contextual information needed during parsing.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError`] if the stream does not contain enough data,
    /// the data does not match the expected format, or any other parsing
    /// failure occurs.
    fn parse(&self, stream: &mut CombinedStream, ctx: &mut Context) -> Result<Value>;

    /// Builds binary data from `data` and writes it to the given stream.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError`] if `data` is not valid for this construct,
    /// the stream write fails, or any other building failure occurs.
    fn build(&self, data: &Value, stream: &mut CombinedStream, ctx: &mut Context) -> Result<()>;

    /// Computes the byte size of this construct in the given context.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::Sizeof`] if the size cannot be determined
    /// (e.g. for variable-length constructs without sufficient context).
    fn sizeof(&self, ctx: &Context) -> Result<usize>;

    /// Returns `true` if this construct can be built without a value.
    ///
    /// Constructs such as [`Pass`](crate::constructs::Pass),
    /// [`Tell`](crate::constructs::Tell), [`Const`](crate::constructs::Const),
    /// etc. do not require a build value because they produce fixed output or
    /// no output at all. [`Struct`](crate::constructs::Struct) uses this flag
    /// to decide whether to look up a field value in the container or pass
    /// `None`.
    ///
    /// The default implementation returns `false`.
    ///
    /// Corresponds to the Python `Construct.flagbuildnone` attribute.
    fn flagbuildnone(&self) -> bool {
        false
    }
}

/// Blanket forwarding impl so that `Box<dyn Construct>` can be used where
/// `impl Construct` is expected (e.g. inside [`CombinedConstruct::Dynamic`]).
impl Construct for Box<dyn Construct> {
    fn parse(&self, stream: &mut CombinedStream, ctx: &mut Context) -> Result<Value> {
        (**self).parse(stream, ctx)
    }

    fn build(&self, data: &Value, stream: &mut CombinedStream, ctx: &mut Context) -> Result<()> {
        (**self).build(data, stream, ctx)
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        (**self).sizeof(ctx)
    }

    fn flagbuildnone(&self) -> bool {
        (**self).flagbuildnone()
    }
}

// ===========================================================================
// Convenience methods on dyn Construct
// ===========================================================================

/// The initial path string used for parse operations.
const PARSE_PATH: &str = "(parsing)";

/// The initial path string used for build operations.
const BUILD_PATH: &str = "(building)";

impl dyn Construct {
    /// Parses a [`Value`] from a byte slice.
    ///
    /// Creates an in-memory stream from `data`, a fresh [`Context`], and
    /// calls [`parse`](Construct::parse). This is the most common entry
    /// point for parsing in-memory data.
    ///
    /// Corresponds to the Python `Construct.parse(data)`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let value = my_construct.parse_bytes(b"\x01\x02\x03")?;
    /// ```
    ///
    /// # Errors
    ///
    /// Propagates any [`ConstructError`] from the underlying
    /// [`Construct::parse`] call.
    pub fn parse_bytes(&self, data: &[u8]) -> Result<Value> {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(data));
        let mut ctx = Context::new();
        self.parse(&mut stream, &mut ctx)
            .map_err(|e| e.with_path_prefix(PARSE_PATH))
    }

    /// Builds binary data from `data` and returns it as a byte vector.
    ///
    /// Creates an in-memory write stream, a fresh [`Context`], and calls
    /// [`build`](Construct::build).
    ///
    /// Corresponds to the Python `Construct.build(obj)`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let bytes = my_construct.build_bytes(&Value::Int(42))?;
    /// ```
    ///
    /// # Errors
    ///
    /// Propagates any [`ConstructError`] from the underlying
    /// [`Construct::build`] call.
    pub fn build_bytes(&self, data: &Value) -> Result<Vec<u8>> {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        self.build(data, &mut stream, &mut ctx)
            .map_err(|e| e.with_path_prefix(BUILD_PATH))?;
        match stream {
            CombinedStream::ByteStream(bs) => Ok(bs.into_bytes()),
            _ => Ok(Vec::new()),
        }
    }

    /// Parses a [`Value`] from a file.
    ///
    /// Reads the entire file into memory and then calls
    /// [`parse_bytes`](Self::parse_bytes).
    ///
    /// Corresponds to the Python `Construct.parse_file(filename)`.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::Stream`] if the file cannot be read, or
    /// propagates any [`ConstructError`] from the underlying
    /// [`Construct::parse`] call.
    pub fn parse_file(&self, path: &Path) -> Result<Value> {
        let data = std::fs::read(path)?;
        self.parse_bytes(&data)
    }

    /// Builds binary data from `data` and writes it to a file.
    ///
    /// Corresponds to the Python `Construct.build_file(obj, filename)`.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::Stream`] if the file cannot be written, or
    /// propagates any [`ConstructError`] from the underlying
    /// [`Construct::build`] call.
    pub fn build_file(&self, data: &Value, path: &Path) -> Result<()> {
        let bytes = self.build_bytes(data)?;
        std::fs::write(path, bytes)?;
        Ok(())
    }
}

// ===========================================================================
// Subconstruct
// ===========================================================================

/// A base wrapper that holds an inner construct and delegates all operations
/// to it.
///
/// This struct is used as a building block by other wrappers such as
/// [`Renamed`], `Adapter`, `Tunnel`, etc. It corresponds to the Python
/// `Subconstruct` class.
///
/// # Python correspondence
///
/// | Python | Rust |
/// |--------|------|
/// | `Subconstruct(subcon)` | `Subconstruct { subcon }` |
/// | `self.subcon._parse(...)` | `self.subcon.parse(...)` |
/// | `self.subcon._build(...)` | `self.subcon.build(...)` |
/// | `self.subcon._sizeof(...)` | `self.subcon.sizeof(...)` |
pub struct Subconstruct {
    /// The inner construct that all operations are delegated to.
    pub subcon: Box<CombinedConstruct>,
}

impl Construct for Subconstruct {
    fn parse(&self, stream: &mut CombinedStream, ctx: &mut Context) -> Result<Value> {
        self.subcon.parse(stream, ctx)
    }

    fn build(&self, data: &Value, stream: &mut CombinedStream, ctx: &mut Context) -> Result<()> {
        self.subcon.build(data, stream, ctx)
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        self.subcon.sizeof(ctx)
    }
}

// ===========================================================================
// Renamed
// ===========================================================================

/// Type alias for a parsed hook callback.
///
/// Corresponds to the Python `Renamed.parsed` attribute — a callable invoked
/// immediately after a successful parse, receiving the parsed value and the
/// current context.
pub type ParsedHook = Box<dyn Fn(&Value, &Context) + Send + Sync>;

/// A wrapper that assigns a name to an inner construct.
///
/// Used primarily by `Struct` and similar composite constructs to associate
/// field names with sub-constructs. The name is also used to build path
/// information that appears in error messages when parsing, building, or
/// sizeof fails.
///
/// Corresponds to the Python `Renamed` class and the `"field" / Construct`
/// naming operator.
///
/// # Python correspondence
///
/// | Python | Rust |
/// |--------|------|
/// | `"field" / Int32ub` | `Renamed::new(Int32ub, "field")` |
/// | `path += " -> %s" % (self.name,)` | `with_path_prefix(name)` |
pub struct Renamed {
    /// The inner construct being named.
    pub inner: Box<CombinedConstruct>,
    /// The name assigned to the construct.
    pub name: String,
    /// Optional docstring. Corresponds to the Python `Renamed.docs` attribute,
    /// set via the `subcon * "documentation"` operator.
    pub docs: Option<String>,
    /// Optional parse callback hook. Corresponds to the Python
    /// `Renamed.parsed` attribute, invoked immediately after a successful
    /// parse. Set via the `subcon * callback` operator.
    pub parsed: Option<ParsedHook>,
}

impl Renamed {
    /// Creates a new `Renamed` wrapper around `inner` with the given `name`.
    ///
    /// The `docs` and `parsed` fields default to `None`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let field = Renamed::new(my_construct, "my_field");
    /// ```
    pub fn new<C: Into<CombinedConstruct>>(inner: C, name: impl Into<String>) -> Self {
        Renamed {
            inner: Box::new(inner.into()),
            name: name.into(),
            docs: None,
            parsed: None,
        }
    }

    /// Builder: sets the docstring.
    ///
    /// Corresponds to the Python `subcon * "docs"` operator.
    #[must_use]
    pub fn with_docs(mut self, docs: impl Into<String>) -> Self {
        self.docs = Some(docs.into());
        self
    }

    /// Builder: sets the parsed hook.
    ///
    /// Corresponds to the Python `subcon * callback` operator. The hook is
    /// invoked immediately after a successful [`Construct::parse`] call.
    #[must_use]
    pub fn with_parsed(mut self, hook: ParsedHook) -> Self {
        self.parsed = Some(hook);
        self
    }
}

impl Construct for Renamed {
    fn parse(&self, stream: &mut CombinedStream, ctx: &mut Context) -> Result<Value> {
        let obj = self
            .inner
            .parse(stream, ctx)
            .map_err(|e| e.with_path_prefix(&self.name))?;
        if let Some(hook) = &self.parsed {
            hook(&obj, ctx);
        }
        Ok(obj)
    }

    fn build(&self, data: &Value, stream: &mut CombinedStream, ctx: &mut Context) -> Result<()> {
        self.inner
            .build(data, stream, ctx)
            .map_err(|e| e.with_path_prefix(&self.name))
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        self.inner
            .sizeof(ctx)
            .map_err(|e| e.with_path_prefix(&self.name))
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constructs::format_field::INT32UB;
    use crate::core::error::ConstructError;
    use crate::core::stream::ByteStream;
    use crate::core::stream::Stream;
    use indexmap::IndexMap;

    // -- Helper: a minimal Construct implementation for testing -----------

    /// A trivial construct that reads/writes exactly 4 bytes as a big-endian
    /// u32. Used solely in tests to verify that `Construct`, `Subconstruct`,
    /// and `Renamed` work correctly.
    struct U32Big;

    impl Construct for U32Big {
        fn parse(&self, stream: &mut CombinedStream, _ctx: &mut Context) -> Result<Value> {
            let bytes = stream.read_bytes(4)?;
            let val = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            Ok(Value::UInt(val as u64))
        }

        fn build(
            &self,
            data: &Value,
            stream: &mut CombinedStream,
            _ctx: &mut Context,
        ) -> Result<()> {
            let val = data.to_u64()?;
            if val > u32::MAX as u64 {
                return Err(ConstructError::Generic {
                    path: String::new(),
                    message: format!("value {} exceeds u32 range", val),
                });
            }
            let bytes = (val as u32).to_be_bytes();
            stream.write_bytes(&bytes)
        }

        fn sizeof(&self, _ctx: &Context) -> Result<usize> {
            Ok(4)
        }
    }

    /// A construct that always fails to parse, for error-propagation testing.
    struct FailingConstruct;

    impl Construct for FailingConstruct {
        fn parse(&self, _stream: &mut CombinedStream, _ctx: &mut Context) -> Result<Value> {
            Err(ConstructError::Generic {
                path: String::new(),
                message: "always fails".to_string(),
            })
        }

        fn build(
            &self,
            _data: &Value,
            _stream: &mut CombinedStream,
            _ctx: &mut Context,
        ) -> Result<()> {
            Err(ConstructError::Generic {
                path: String::new(),
                message: "always fails".to_string(),
            })
        }

        fn sizeof(&self, _ctx: &Context) -> Result<usize> {
            Err(ConstructError::Sizeof {
                path: String::new(),
                reason: "size unknown".to_string(),
            })
        }
    }

    /// A variable-size construct that reads/writes N bytes where N comes from
    /// context field "length".
    struct VarBytes;

    impl Construct for VarBytes {
        fn parse(&self, stream: &mut CombinedStream, ctx: &mut Context) -> Result<Value> {
            let length = ctx
                .get_recursive("length")
                .ok_or_else(|| ConstructError::FieldMissing {
                    path: String::new(),
                    field: "length".to_string(),
                })?
                .to_u64()? as usize;
            let bytes = stream.read_bytes(length)?;
            Ok(Value::Bytes(bytes))
        }

        fn build(
            &self,
            data: &Value,
            stream: &mut CombinedStream,
            _ctx: &mut Context,
        ) -> Result<()> {
            let bytes = data.as_bytes()?;
            stream.write_bytes(bytes)
        }

        fn sizeof(&self, ctx: &Context) -> Result<usize> {
            ctx.get_recursive("length")
                .ok_or_else(|| ConstructError::Sizeof {
                    path: String::new(),
                    reason: "length not in context".to_string(),
                })?
                .to_u64()
                .map(|v| v as usize)
                .map_err(|e: ConstructError| e.with_path_prefix("length"))
        }
    }

    // ======================================================================
    // Construct trait tests
    // ======================================================================

    // -- U32Big basic parse/build -------------------------------------------

    #[test]
    fn u32big_parse_reads_4_bytes() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\x00\x01\x02\x03"));
        let mut ctx = Context::new();
        let result = U32Big.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::UInt(0x00010203));
    }

    #[test]
    fn u32big_build_writes_4_bytes() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        U32Big
            .build(&Value::UInt(42), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), vec![0, 0, 0, 42]);
    }

    #[test]
    fn u32big_sizeof_returns_4() {
        let ctx = Context::new();
        assert_eq!(U32Big.sizeof(&ctx).unwrap(), 4);
    }

    #[test]
    fn u32big_parse_insufficient_data_returns_error() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\x00\x01"));
        let mut ctx = Context::new();
        let err = U32Big.parse(&mut stream, &mut ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Stream { .. }));
    }

    #[test]
    fn u32big_build_wrong_type_returns_error() {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        let err = U32Big
            .build(
                &Value::String("not a number".to_string()),
                &mut stream,
                &mut ctx,
            )
            .unwrap_err();
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
    }

    // -- parse/build symmetry -----------------------------------------------

    #[test]
    fn u32big_build_then_parse_roundtrip() {
        let original = Value::UInt(0xDEADBEEF);
        let mut build_stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut build_ctx = Context::new();
        U32Big
            .build(&original, &mut build_stream, &mut build_ctx)
            .unwrap();
        let bytes = build_stream.into_bytes();

        let mut parse_stream = CombinedStream::ByteStream(ByteStream::new_read(&bytes));
        let mut parse_ctx = Context::new();
        let parsed = U32Big.parse(&mut parse_stream, &mut parse_ctx).unwrap();
        assert_eq!(parsed, original);
    }

    // ======================================================================
    // Convenience methods (dyn Construct)
    // ======================================================================

    #[test]
    fn parse_bytes_convenience_method() {
        let c: &dyn Construct = &U32Big;
        let result = c.parse_bytes(b"\x00\x00\x00\x2A").unwrap();
        assert_eq!(result, Value::UInt(42));
    }

    #[test]
    fn parse_bytes_error_has_path() {
        let c: &dyn Construct = &U32Big;
        let err = c.parse_bytes(b"\x00").unwrap_err();
        assert_eq!(err.path(), "(parsing)");
    }

    #[test]
    fn build_bytes_convenience_method() {
        let c: &dyn Construct = &U32Big;
        let bytes = c.build_bytes(&Value::UInt(256)).unwrap();
        assert_eq!(bytes, vec![0, 0, 1, 0]);
    }

    #[test]
    fn build_bytes_error_has_path() {
        let c: &dyn Construct = &U32Big;
        let err = c.build_bytes(&Value::None).unwrap_err();
        // Path should be "(building)" since the inner construct has no name
        assert_eq!(err.path(), "(building)");
    }

    #[test]
    fn build_bytes_then_parse_bytes_roundtrip() {
        let c: &dyn Construct = &U32Big;
        let original = Value::UInt(12345);
        let bytes = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn parse_file_convenience_method() {
        // Create a temp file with test data
        let dir = std::env::temp_dir().join("construct_test_parse_file");
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("test_u32.bin");
        std::fs::write(&file_path, b"\x00\x00\x00\xFF").unwrap();

        let c: &dyn Construct = &U32Big;
        let result = c.parse_file(&file_path).unwrap();
        assert_eq!(result, Value::UInt(255));

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_file_nonexistent_returns_error() {
        let c: &dyn Construct = &U32Big;
        let err = c
            .parse_file(Path::new("/nonexistent/path/to/file.bin"))
            .unwrap_err();
        assert!(matches!(err, ConstructError::Stream { .. }));
    }

    #[test]
    fn build_file_convenience_method() {
        let dir = std::env::temp_dir().join("construct_test_build_file");
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("test_u32_out.bin");

        let c: &dyn Construct = &U32Big;
        c.build_file(&Value::UInt(0xABCDEF01), &file_path).unwrap();

        let contents = std::fs::read(&file_path).unwrap();
        assert_eq!(contents, vec![0xAB, 0xCD, 0xEF, 0x01]);

        // Verify round-trip
        let parsed = c.parse_file(&file_path).unwrap();
        assert_eq!(parsed, Value::UInt(0xABCDEF01));

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ======================================================================
    // Subconstruct tests
    // ======================================================================

    #[test]
    fn subconstruct_delegates_parse() {
        let sub = Subconstruct {
            subcon: Box::new(INT32UB.into()),
        };
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\x00\x00\x01\x00"));
        let mut ctx = Context::new();
        let result = sub.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::UInt(256));
    }

    #[test]
    fn subconstruct_delegates_build() {
        let sub = Subconstruct {
            subcon: Box::new(INT32UB.into()),
        };
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        sub.build(&Value::UInt(1), &mut stream, &mut ctx).unwrap();
        assert_eq!(stream.into_bytes(), vec![0, 0, 0, 1]);
    }

    #[test]
    fn subconstruct_delegates_sizeof() {
        let sub = Subconstruct {
            subcon: Box::new(INT32UB.into()),
        };
        let ctx = Context::new();
        assert_eq!(sub.sizeof(&ctx).unwrap(), 4);
    }

    #[test]
    fn subconstruct_propagates_parse_error() {
        let sub = Subconstruct {
            subcon: Box::new(crate::combined::dynamic(FailingConstruct)),
        };
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\x00\x01\x02\x03"));
        let mut ctx = Context::new();
        let err = sub.parse(&mut stream, &mut ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    #[test]
    fn subconstruct_propagates_build_error() {
        let sub = Subconstruct {
            subcon: Box::new(crate::combined::dynamic(FailingConstruct)),
        };
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        let err = sub.build(&Value::None, &mut stream, &mut ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    #[test]
    fn subconstruct_propagates_sizeof_error() {
        let sub = Subconstruct {
            subcon: Box::new(crate::combined::dynamic(FailingConstruct)),
        };
        let ctx = Context::new();
        let err = sub.sizeof(&ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Sizeof { .. }));
    }

    #[test]
    fn subconstruct_build_then_parse_roundtrip() {
        let sub = Subconstruct {
            subcon: Box::new(INT32UB.into()),
        };
        let original = Value::UInt(999);
        let mut build_stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut build_ctx = Context::new();
        sub.build(&original, &mut build_stream, &mut build_ctx)
            .unwrap();
        let bytes = build_stream.into_bytes();

        let mut parse_stream = CombinedStream::ByteStream(ByteStream::new_read(&bytes));
        let mut parse_ctx = Context::new();
        let parsed = sub.parse(&mut parse_stream, &mut parse_ctx).unwrap();
        assert_eq!(parsed, original);
    }

    // ======================================================================
    // Renamed tests
    // ======================================================================

    #[test]
    fn renamed_new_creates_wrapper() {
        let renamed = Renamed::new(INT32UB, "my_field");
        assert_eq!(renamed.name, "my_field");
    }

    #[test]
    fn renamed_new_accepts_string() {
        let renamed = Renamed::new(INT32UB, String::from("field"));
        assert_eq!(renamed.name, "field");
    }

    #[test]
    fn renamed_parse_delegates_and_enriches_path_on_error() {
        let renamed = Renamed::new(crate::combined::dynamic(FailingConstruct), "failing_field");
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\x00\x01\x02\x03"));
        let mut ctx = Context::new();
        let err = renamed.parse(&mut stream, &mut ctx).unwrap_err();
        match err {
            ConstructError::Generic { path, message } => {
                assert_eq!(path, "failing_field");
                assert_eq!(message, "always fails");
            }
            other => panic!("expected Generic error, got {:?}", other),
        }
    }

    #[test]
    fn renamed_build_delegates_and_enriches_path_on_error() {
        let renamed = Renamed::new(crate::combined::dynamic(FailingConstruct), "build_field");
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        let err = renamed
            .build(&Value::None, &mut stream, &mut ctx)
            .unwrap_err();
        match err {
            ConstructError::Generic { path, message } => {
                assert_eq!(path, "build_field");
                assert_eq!(message, "always fails");
            }
            other => panic!("expected Generic error, got {:?}", other),
        }
    }

    #[test]
    fn renamed_sizeof_delegates_and_enriches_path_on_error() {
        let renamed = Renamed::new(crate::combined::dynamic(FailingConstruct), "sizeof_field");
        let ctx = Context::new();
        let err = renamed.sizeof(&ctx).unwrap_err();
        match err {
            ConstructError::Sizeof { path, reason } => {
                assert_eq!(path, "sizeof_field");
                assert_eq!(reason, "size unknown");
            }
            other => panic!("expected Sizeof error, got {:?}", other),
        }
    }

    #[test]
    fn renamed_parse_succeeds_without_error() {
        let renamed = Renamed::new(INT32UB, "counter");
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\x00\x00\x00\x2A"));
        let mut ctx = Context::new();
        let result = renamed.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::UInt(42));
    }

    #[test]
    fn renamed_build_succeeds_without_error() {
        let renamed = Renamed::new(INT32UB, "counter");
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        renamed
            .build(&Value::UInt(42), &mut stream, &mut ctx)
            .unwrap();
        assert_eq!(stream.into_bytes(), vec![0, 0, 0, 42]);
    }

    #[test]
    fn renamed_sizeof_succeeds() {
        let renamed = Renamed::new(INT32UB, "counter");
        let ctx = Context::new();
        assert_eq!(renamed.sizeof(&ctx).unwrap(), 4);
    }

    #[test]
    fn renamed_build_then_parse_roundtrip() {
        let renamed = Renamed::new(INT32UB, "value");
        let original = Value::UInt(0x12345678);
        let mut build_stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut build_ctx = Context::new();
        renamed
            .build(&original, &mut build_stream, &mut build_ctx)
            .unwrap();
        let bytes = build_stream.into_bytes();

        let mut parse_stream = CombinedStream::ByteStream(ByteStream::new_read(&bytes));
        let mut parse_ctx = Context::new();
        let parsed = renamed.parse(&mut parse_stream, &mut parse_ctx).unwrap();
        assert_eq!(parsed, original);
    }

    // -- Nested Renamed (path chaining) -------------------------------------

    #[test]
    fn nested_renamed_chains_error_path() {
        // outer_name > inner_name
        let inner = Renamed::new(crate::combined::dynamic(FailingConstruct), "inner_field");
        let outer = Renamed::new(inner, "outer_field");

        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\x00"));
        let mut ctx = Context::new();
        let err = outer.parse(&mut stream, &mut ctx).unwrap_err();

        // The error propagates: inner adds "inner_field", then outer adds "outer_field"
        match err {
            ConstructError::Generic { path, message } => {
                assert_eq!(path, "outer_field.inner_field");
                assert_eq!(message, "always fails");
            }
            other => panic!("expected Generic error, got {:?}", other),
        }
    }

    // -- Convenience methods with Renamed -----------------------------------

    #[test]
    fn renamed_parse_bytes_error_path() {
        let renamed = Renamed::new(crate::combined::dynamic(FailingConstruct), "field");
        let c: &dyn Construct = &renamed;
        let err = c.parse_bytes(b"\x00").unwrap_err();
        // path: "(parsing)" from parse_bytes + "field" from Renamed
        assert_eq!(err.path(), "(parsing).field");
    }

    #[test]
    fn renamed_build_bytes_error_path() {
        let renamed = Renamed::new(crate::combined::dynamic(FailingConstruct), "field");
        let c: &dyn Construct = &renamed;
        let err = c.build_bytes(&Value::None).unwrap_err();
        // path: "(building)" from build_bytes + "field" from Renamed
        assert_eq!(err.path(), "(building).field");
    }

    // ======================================================================
    // Context integration tests
    // ======================================================================

    #[test]
    fn var_bytes_parse_with_context_length() {
        let mut ctx = Context::new();
        ctx.insert("length", Value::UInt(3));
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\x01\x02\x03\x04"));
        let result = VarBytes.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::Bytes(vec![1, 2, 3]));
    }

    #[test]
    fn var_bytes_parse_missing_context_length_returns_error() {
        let mut ctx = Context::new();
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\x01\x02\x03"));
        let err = VarBytes.parse(&mut stream, &mut ctx).unwrap_err();
        assert!(matches!(err, ConstructError::FieldMissing { .. }));
    }

    #[test]
    fn var_bytes_sizeof_with_context() {
        let mut ctx = Context::new();
        ctx.insert("length", Value::UInt(10));
        assert_eq!(VarBytes.sizeof(&ctx).unwrap(), 10);
    }

    #[test]
    fn var_bytes_sizeof_missing_context_returns_error() {
        let ctx = Context::new();
        let err = VarBytes.sizeof(&ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Sizeof { .. }));
    }

    // -- Context subcontext integration with construct ----------------------

    #[test]
    fn construct_can_use_subcontext() {
        let mut outer_ctx = Context::new();
        outer_ctx.insert("length", Value::UInt(2));

        let result = outer_ctx.with_subcontext(|child_ctx| {
            child_ctx.insert("extra", Value::String("hello".to_string()));
            let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"\xAA\xBB\xCC\xDD"));
            VarBytes.parse(&mut stream, child_ctx)
        });

        // VarBytes reads 2 bytes (length from parent context via get_recursive)
        assert_eq!(result.unwrap(), Value::Bytes(vec![0xAA, 0xBB]));
    }

    // -- Custom construct using IndexMap Container --------------------------

    #[test]
    fn custom_construct_returns_container() {
        /// A minimal struct-like construct that parses two U32 fields and
        /// returns a Container value.
        struct TwoFields;

        impl Construct for TwoFields {
            fn parse(&self, stream: &mut CombinedStream, _ctx: &mut Context) -> Result<Value> {
                let a_bytes = stream.read_bytes(4)?;
                let a = u32::from_be_bytes([a_bytes[0], a_bytes[1], a_bytes[2], a_bytes[3]]);
                let b_bytes = stream.read_bytes(4)?;
                let b = u32::from_be_bytes([b_bytes[0], b_bytes[1], b_bytes[2], b_bytes[3]]);

                let mut map = IndexMap::new();
                map.insert("a".to_string(), Value::UInt(a as u64));
                map.insert("b".to_string(), Value::UInt(b as u64));
                Ok(Value::Container(map))
            }

            fn build(
                &self,
                data: &Value,
                stream: &mut CombinedStream,
                _ctx: &mut Context,
            ) -> Result<()> {
                let container = data.as_container()?;
                let a = container
                    .get("a")
                    .ok_or_else(|| ConstructError::FieldMissing {
                        path: String::new(),
                        field: "a".to_string(),
                    })?
                    .to_u64()? as u32;
                let b = container
                    .get("b")
                    .ok_or_else(|| ConstructError::FieldMissing {
                        path: String::new(),
                        field: "b".to_string(),
                    })?
                    .to_u64()? as u32;
                stream.write_bytes(&a.to_be_bytes())?;
                stream.write_bytes(&b.to_be_bytes())
            }

            fn sizeof(&self, _ctx: &Context) -> Result<usize> {
                Ok(8)
            }
        }

        // Parse
        let mut stream =
            CombinedStream::ByteStream(ByteStream::new_read(b"\x00\x00\x00\x01\x00\x00\x00\x02"));
        let mut ctx = Context::new();
        let result = TwoFields.parse(&mut stream, &mut ctx).unwrap();

        let container = result.as_container().unwrap();
        assert_eq!(container.get("a").unwrap(), &Value::UInt(1));
        assert_eq!(container.get("b").unwrap(), &Value::UInt(2));

        // Build
        let mut build_stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut build_ctx = Context::new();
        TwoFields
            .build(&result, &mut build_stream, &mut build_ctx)
            .unwrap();
        assert_eq!(build_stream.into_bytes(), vec![0, 0, 0, 1, 0, 0, 0, 2]);

        // Sizeof
        assert_eq!(TwoFields.sizeof(&Context::new()).unwrap(), 8);
    }
}
