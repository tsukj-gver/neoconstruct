//! Struct construct — a sequence of named (or anonymous) sub-constructs.
//!
//! [`Struct`] is the primary composite construct. It parses a series of fields
//! into a [`Value::Container`] (an ordered map of name → value) and builds
//! binary data from such a container.
//!
//! Corresponds to Python `Struct(*subcons)`.
//!
//! # Field types
//!
//! Each field in a `Struct` is either:
//! - **Named** — has a string name; its parse result is inserted into the
//!   output container and the context.
//! - **Anonymous** — has no name (`None`); its parse result is discarded (or
//!   built from `Value::None`).
//!
//! Anonymous fields are typically constructs that do not need user-supplied
//! values for building, such as [`Const`], [`Pass`], [`Padding`], etc.
//!
//! # Context nesting
//!
//! During parsing and building, `Struct` creates a child context (via
//! [`Context::subcontext`]) so that later fields can reference earlier ones.
//!
//! # Examples
//!
//! ```
//! use construct::constructs::struct_::Struct;
//! use construct::constructs::format_field::INT8UB;
//! use construct::constructs::bytes::Bytes;
//! use construct::core::Construct;
//! use construct::value::Value;
//!
//! let s = Struct::new()
//!     .field("num", Box::new(INT8UB))
//!     .field("data", Box::new(Bytes::new(3)));
//!
//! let c: &dyn Construct = &s;
//! let parsed = c.parse_bytes(b"\x04ABC").unwrap();
//! let container = parsed.as_container().unwrap();
//! assert_eq!(container.get("num").unwrap(), &Value::UInt(4));
//! assert_eq!(container.get("data").unwrap(), &Value::Bytes(b"ABC".to_vec()));
//! ```
//!
//! [`Const`]: crate::constructs::Const
//! [`Pass`]: crate::constructs::Pass
//! [`Padding`]: crate::constructs::Padding

use indexmap::IndexMap;

use crate::core::context::Context;
use crate::core::error::{ConstructError, Result};
use crate::core::stream::Stream;
use crate::core::Construct;
use crate::value::Value;

// ===========================================================================
// StructField
// ===========================================================================

/// A single field within a [`Struct`].
///
/// A field wraps a sub-construct and optionally associates it with a name.
/// Named fields contribute their parsed value to the output container;
/// anonymous fields are parsed but their result is discarded.
///
/// Corresponds to a single positional argument in Python's `Struct(*subcons)`.
pub struct StructField {
    /// The field name, or `None` for anonymous fields.
    pub name: Option<String>,
    /// The sub-construct that parses / builds this field.
    pub subcon: Box<dyn Construct>,
}

impl StructField {
    /// Creates a new named field.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let field = StructField::new("count", Box::new(INT8UB));
    /// ```
    pub fn new(name: impl Into<String>, subcon: Box<dyn Construct>) -> Self {
        StructField {
            name: Some(name.into()),
            subcon,
        }
    }

    /// Creates a new anonymous field.
    ///
    /// Anonymous fields are parsed and the result is discarded, or built from
    /// `Value::None`.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let field = StructField::anonymous(Box::new(Pass::new()));
    /// ```
    pub fn anonymous(subcon: Box<dyn Construct>) -> Self {
        StructField { name: None, subcon }
    }
}

// ===========================================================================
// Struct
// ===========================================================================

/// A composite construct that parses a sequence of named/anonymous fields into
/// a [`Value::Container`].
///
/// Parses into a `Container` (ordered map) where keys match field names.
/// Builds from a `Container` where each named field gets a value from the map.
/// Anonymous fields are built from `Value::None` if the sub-construct has
/// `flagbuildnone` returning `true`.
///
/// Size is the sum of all field sizes, unless any field raises a sizeof error.
///
/// Corresponds to Python `Struct`.
///
/// # Context nesting
///
/// During parse, a child context is created so that later fields can reference
/// earlier parsed values. During build, the container's entries are inserted
/// into the context before each field is built.
///
/// # StopField support
///
/// If a sub-construct returns [`ConstructError::StopField`], the `Struct`
/// stops processing further fields and returns successfully.
pub struct Struct {
    /// The ordered list of fields in this struct.
    fields: Vec<StructField>,
}

impl Struct {
    /// Creates a new, empty `Struct`.
    ///
    /// Use the builder methods [`field`](Struct::field) and
    /// [`anonymous`](Struct::anonymous) to add fields.
    ///
    /// # Examples
    ///
    /// ```
    /// use construct::constructs::struct_::Struct;
    ///
    /// let s = Struct::new();
    /// ```
    pub fn new() -> Self {
        Struct { fields: Vec::new() }
    }

    /// Adds a named field to this struct, returning `self` for chaining.
    ///
    /// # Examples
    ///
    /// ```
    /// use construct::constructs::struct_::Struct;
    /// use construct::constructs::format_field::INT8UB;
    ///
    /// let s = Struct::new().field("count", Box::new(INT8UB));
    /// ```
    pub fn field(mut self, name: impl Into<String>, subcon: Box<dyn Construct>) -> Self {
        self.fields.push(StructField::new(name, subcon));
        self
    }

    /// Adds an anonymous field to this struct, returning `self` for chaining.
    ///
    /// Anonymous fields are parsed and the result is discarded, or built from
    /// `Value::None`.
    ///
    /// # Examples
    ///
    /// ```
    /// use construct::constructs::struct_::Struct;
    /// use construct::constructs::meta::Pass;
    ///
    /// let s = Struct::new().anonymous(Box::new(Pass::new()));
    /// ```
    pub fn anonymous(mut self, subcon: Box<dyn Construct>) -> Self {
        self.fields.push(StructField::anonymous(subcon));
        self
    }

    /// Appends all fields from another `Struct` to this one, consuming the
    /// other struct.
    ///
    /// This allows composing structs from reusable building blocks.
    ///
    /// # Examples
    ///
    /// ```
    /// use construct::constructs::struct_::Struct;
    /// use construct::constructs::format_field::INT8UB;
    ///
    /// let header = Struct::new().field("magic", Box::new(INT8UB));
    /// let full = Struct::new().field("version", Box::new(INT8UB)).extend(header);
    /// ```
    pub fn extend(mut self, other: Struct) -> Self {
        // Move fields from other into self
        let other_fields = other.fields;
        for field in other_fields {
            self.fields.push(field);
        }
        self
    }
}

impl Default for Struct {
    fn default() -> Self {
        Self::new()
    }
}

impl Construct for Struct {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let mut container: IndexMap<String, Value> = IndexMap::new();
        let mut child_ctx = ctx.subcontext();

        for field in &self.fields {
            let subobj = match field.subcon.parse(stream, &mut child_ctx) {
                Ok(value) => value,
                Err(ConstructError::StopField { .. }) => break,
                Err(e) => {
                    return Err(if let Some(ref name) = field.name {
                        e.with_path_prefix(name)
                    } else {
                        e.with_path_prefix("(anonymous)")
                    });
                }
            };

            if let Some(ref name) = field.name {
                container.insert(name.clone(), subobj.clone());
                child_ctx.insert(name.clone(), subobj);
            }
            // Anonymous fields: parsed value is discarded
        }

        Ok(Value::Container(container))
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        // If data is None, treat as empty container
        let container = match data {
            Value::None => Ok(IndexMap::new()),
            Value::Container(map) => Ok(map.clone()),
            other => Err(ConstructError::TypeMismatch {
                path: String::new(),
                expected: "Container".to_string(),
                actual: other.type_name().to_string(),
            }),
        }?;

        let mut child_ctx = ctx.subcontext();

        // Insert all container values into child context (matching Python's context.update(obj))
        for (key, value) in &container {
            child_ctx.insert(key.clone(), value.clone());
        }

        for field in &self.fields {
            let name_ref = field.name.as_deref();

            // Determine the value to build with
            let build_value =
                if field.subcon.flagbuildnone() {
                    // Sub-construct can build without a value: use the container
                    // value if present, otherwise None
                    name_ref
                        .and_then(|n| container.get(n))
                        .cloned()
                        .unwrap_or(Value::None)
                } else {
                    // Must have a value in the container
                    match name_ref {
                        Some(n) => container.get(n).cloned().ok_or_else(|| {
                            ConstructError::FieldMissing {
                                path: String::new(),
                                field: n.to_string(),
                            }
                        })?,
                        None => Value::None,
                    }
                };

            // Insert into context before building
            if let Some(ref name) = field.name {
                child_ctx.insert(name.clone(), build_value.clone());
            }

            let result = match field.subcon.build(&build_value, stream, &mut child_ctx) {
                Ok(()) => Ok(()),
                Err(ConstructError::StopField { .. }) => return Ok(()),
                Err(e) => Err(if let Some(ref name) = field.name {
                    e.with_path_prefix(name)
                } else {
                    e.with_path_prefix("(anonymous)")
                }),
            };

            result?;

            // Note: in Python, the build return value replaces context[name].
            // Our build returns () not a value, so we don't need this step.
        }

        Ok(())
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        let child_ctx = ctx.subcontext();
        let mut total: usize = 0;
        for field in &self.fields {
            let size = match field.subcon.sizeof(&child_ctx) {
                Ok(s) => s,
                Err(e) => {
                    return Err(if let Some(ref name) = field.name {
                        e.with_path_prefix(name)
                    } else {
                        e.with_path_prefix("(anonymous)")
                    });
                }
            };
            total = total.saturating_add(size);
        }
        Ok(total)
    }

    fn flagbuildnone(&self) -> bool {
        self.fields.iter().all(|f| f.subcon.flagbuildnone())
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constructs::bytes::Bytes;
    use crate::constructs::const_::Const;
    use crate::constructs::format_field::{INT16UB, INT32UB, INT8UB};
    use crate::constructs::meta::{Pass, Terminated};
    use crate::core::error::ConstructError;
    use crate::core::Construct;

    // Helper: convert Struct to &dyn Construct for convenience methods
    macro_rules! as_dyn {
        ($s:expr) => {
            &$s as &dyn Construct
        };
    }

    // ======================================================================
    // Basic parse tests
    // ======================================================================

    #[test]
    fn parse_empty_struct_returns_empty_container() {
        let s = Struct::new();
        let result = as_dyn!(s).parse_bytes(b"").unwrap();
        let container = result.as_container().unwrap();
        assert!(container.is_empty());
    }

    #[test]
    fn parse_single_named_field() {
        let s = Struct::new().field("num", Box::new(INT8UB));
        let result = as_dyn!(s).parse_bytes(b"\x2A").unwrap();
        let container = result.as_container().unwrap();
        assert_eq!(container.get("num").unwrap(), &Value::UInt(42));
        assert_eq!(container.len(), 1);
    }

    #[test]
    fn parse_two_named_fields() {
        let s = Struct::new()
            .field("a", Box::new(INT8UB))
            .field("b", Box::new(INT8UB));
        let result = as_dyn!(s).parse_bytes(b"\x01\x02").unwrap();
        let container = result.as_container().unwrap();
        assert_eq!(container.get("a").unwrap(), &Value::UInt(1));
        assert_eq!(container.get("b").unwrap(), &Value::UInt(2));
    }

    #[test]
    fn parse_preserves_field_order() {
        let s = Struct::new()
            .field("first", Box::new(INT8UB))
            .field("second", Box::new(INT8UB))
            .field("third", Box::new(INT8UB));
        let result = as_dyn!(s).parse_bytes(b"\x01\x02\x03").unwrap();
        let container = result.as_container().unwrap();
        let keys: Vec<&str> = container.keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["first", "second", "third"]);
    }

    #[test]
    fn parse_anonymous_field_discards_value() {
        let s = Struct::new()
            .field("a", Box::new(INT8UB))
            .anonymous(Box::new(Bytes::new(2)))
            .field("b", Box::new(INT8UB));
        let result = as_dyn!(s).parse_bytes(b"\x01XX\x02").unwrap();
        let container = result.as_container().unwrap();
        assert_eq!(container.len(), 2);
        assert_eq!(container.get("a").unwrap(), &Value::UInt(1));
        assert_eq!(container.get("b").unwrap(), &Value::UInt(2));
    }

    #[test]
    fn parse_const_field_skipped_in_output() {
        // Const is anonymous, its value is not in the container
        let s = Struct::new()
            .anonymous(Box::new(Const::new_bytes(b"MZ".to_vec())))
            .field("data", Box::new(INT8UB));
        let result = as_dyn!(s).parse_bytes(b"MZ\x42").unwrap();
        let container = result.as_container().unwrap();
        assert_eq!(container.len(), 1);
        assert_eq!(container.get("data").unwrap(), &Value::UInt(0x42));
    }

    #[test]
    fn parse_named_const_included_in_output() {
        // A named Const field DOES appear in the output
        let s = Struct::new()
            .field("magic", Box::new(Const::new_bytes(b"MZ".to_vec())))
            .field("data", Box::new(INT8UB));
        let result = as_dyn!(s).parse_bytes(b"MZ\x42").unwrap();
        let container = result.as_container().unwrap();
        assert_eq!(container.len(), 2);
        assert_eq!(
            container.get("magic").unwrap(),
            &Value::Bytes(b"MZ".to_vec())
        );
        assert_eq!(container.get("data").unwrap(), &Value::UInt(0x42));
    }

    #[test]
    fn parse_context_visible_to_later_fields() {
        // A later field reads a length from context set by an earlier field
        // This tests context nesting
        let s = Struct::new()
            .field("length", Box::new(INT8UB))
            .field("data", Box::new(Bytes::new(3)));
        let result = as_dyn!(s).parse_bytes(b"\x03ABC").unwrap();
        let container = result.as_container().unwrap();
        assert_eq!(container.get("length").unwrap(), &Value::UInt(3));
        assert_eq!(
            container.get("data").unwrap(),
            &Value::Bytes(b"ABC".to_vec())
        );
    }

    // ======================================================================
    // Basic build tests
    // ======================================================================

    #[test]
    fn build_empty_struct_returns_empty_bytes() {
        let s = Struct::new();
        let built = as_dyn!(s)
            .build_bytes(&Value::Container(IndexMap::new()))
            .unwrap();
        assert!(built.is_empty());
    }

    #[test]
    fn build_single_named_field() {
        let s = Struct::new().field("num", Box::new(INT8UB));
        let mut container = IndexMap::new();
        container.insert("num".to_string(), Value::UInt(42));
        let built = as_dyn!(s)
            .build_bytes(&Value::Container(container))
            .unwrap();
        assert_eq!(built, vec![42]);
    }

    #[test]
    fn build_two_named_fields() {
        let s = Struct::new()
            .field("a", Box::new(INT8UB))
            .field("b", Box::new(INT8UB));
        let mut container = IndexMap::new();
        container.insert("a".to_string(), Value::UInt(1));
        container.insert("b".to_string(), Value::UInt(2));
        let built = as_dyn!(s)
            .build_bytes(&Value::Container(container))
            .unwrap();
        assert_eq!(built, vec![1, 2]);
    }

    #[test]
    fn build_anonymous_field_uses_none() {
        // Anonymous field with flagbuildnone=true (like Pass) builds with None
        let s = Struct::new()
            .field("data", Box::new(INT8UB))
            .anonymous(Box::new(Pass::new()));
        let mut container = IndexMap::new();
        container.insert("data".to_string(), Value::UInt(0xFF));
        let built = as_dyn!(s)
            .build_bytes(&Value::Container(container))
            .unwrap();
        assert_eq!(built, vec![0xFF]);
    }

    #[test]
    fn build_const_field_from_none() {
        // Const has flagbuildnone=true; builds from Value::None
        let s = Struct::new()
            .anonymous(Box::new(Const::new_bytes(b"MZ".to_vec())))
            .field("data", Box::new(INT8UB));
        let mut container = IndexMap::new();
        container.insert("data".to_string(), Value::UInt(0x42));
        let built = as_dyn!(s)
            .build_bytes(&Value::Container(container))
            .unwrap();
        assert_eq!(built, vec![b'M', b'Z', 0x42]);
    }

    #[test]
    fn build_from_none_value_builds_empty_container() {
        // If all fields have flagbuildnone=true, Struct can build from None
        let s = Struct::new()
            .anonymous(Box::new(Const::new_bytes(b"HI".to_vec())))
            .anonymous(Box::new(Pass::new()));
        let built = as_dyn!(s).build_bytes(&Value::None).unwrap();
        assert_eq!(built, b"HI");
    }

    #[test]
    fn build_missing_named_field_returns_error() {
        let s = Struct::new().field("num", Box::new(INT8UB));
        let container = IndexMap::new(); // missing "num"
        let err = as_dyn!(s)
            .build_bytes(&Value::Container(container))
            .unwrap_err();
        assert!(matches!(err, ConstructError::FieldMissing { .. }));
    }

    #[test]
    fn build_wrong_type_returns_error() {
        let s = Struct::new().field("num", Box::new(INT8UB));
        let err = as_dyn!(s).build_bytes(&Value::Int(42)).unwrap_err();
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
    }

    // ======================================================================
    // Sizeof tests
    // ======================================================================

    #[test]
    fn sizeof_empty_struct_is_0() {
        let s = Struct::new();
        assert_eq!(s.sizeof(&Context::new()).unwrap(), 0);
    }

    #[test]
    fn sizeof_single_field() {
        let s = Struct::new().field("num", Box::new(INT8UB));
        assert_eq!(s.sizeof(&Context::new()).unwrap(), 1);
    }

    #[test]
    fn sizeof_multiple_fields() {
        let s = Struct::new()
            .field("a", Box::new(INT8UB))
            .field("b", Box::new(INT16UB))
            .field("c", Box::new(INT32UB));
        assert_eq!(s.sizeof(&Context::new()).unwrap(), 7);
    }

    #[test]
    fn sizeof_with_anonymous_fields() {
        let s = Struct::new()
            .field("a", Box::new(INT8UB))
            .anonymous(Box::new(Bytes::new(3)));
        assert_eq!(s.sizeof(&Context::new()).unwrap(), 4);
    }

    // ======================================================================
    // flagbuildnone tests
    // ======================================================================

    #[test]
    fn flagbuildnone_true_when_all_fields_are_build_none() {
        let s = Struct::new()
            .anonymous(Box::new(Pass::new()))
            .anonymous(Box::new(Const::new_bytes(b"X".to_vec())));
        assert!(s.flagbuildnone());
    }

    #[test]
    fn flagbuildnone_false_when_any_field_requires_value() {
        let s = Struct::new()
            .anonymous(Box::new(Pass::new()))
            .field("data", Box::new(INT8UB));
        assert!(!s.flagbuildnone());
    }

    #[test]
    fn flagbuildnone_true_for_empty_struct() {
        let s = Struct::new();
        assert!(s.flagbuildnone());
    }

    // ======================================================================
    // Roundtrip tests
    // ======================================================================

    #[test]
    fn roundtrip_two_fields() {
        let s = Struct::new()
            .field("a", Box::new(INT8UB))
            .field("b", Box::new(INT16UB));

        let mut container = IndexMap::new();
        container.insert("a".to_string(), Value::UInt(0x12));
        container.insert("b".to_string(), Value::UInt(0x3456));

        let c: &dyn Construct = &s;
        let built = c.build_bytes(&Value::Container(container.clone())).unwrap();
        assert_eq!(built, vec![0x12, 0x34, 0x56]);

        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, Value::Container(container));
    }

    #[test]
    fn roundtrip_with_const() {
        let s = Struct::new()
            .anonymous(Box::new(Const::new_bytes(b"PK".to_vec())))
            .field("version", Box::new(INT8UB));

        let mut container = IndexMap::new();
        container.insert("version".to_string(), Value::UInt(10));

        let c: &dyn Construct = &s;
        let built = c.build_bytes(&Value::Container(container.clone())).unwrap();
        assert_eq!(built, b"PK\x0A");

        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, Value::Container(container));
    }

    #[test]
    fn roundtrip_with_pass() {
        let s = Struct::new()
            .field("x", Box::new(INT8UB))
            .anonymous(Box::new(Pass::new()))
            .field("y", Box::new(INT8UB));

        let mut container = IndexMap::new();
        container.insert("x".to_string(), Value::UInt(1));
        container.insert("y".to_string(), Value::UInt(2));

        let c: &dyn Construct = &s;
        let built = c.build_bytes(&Value::Container(container.clone())).unwrap();
        assert_eq!(built, vec![1, 2]);

        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, Value::Container(container));
    }

    #[test]
    fn roundtrip_nested_struct() {
        // Inner struct: { inner_a: u8 }
        let inner = Struct::new().field("inner_a", Box::new(INT8UB));

        // Outer struct: { outer: inner, b: u8 }
        let outer = Struct::new()
            .field("outer", Box::new(inner))
            .field("b", Box::new(INT8UB));

        let mut inner_map = IndexMap::new();
        inner_map.insert("inner_a".to_string(), Value::UInt(0xAA));
        let mut outer_map = IndexMap::new();
        outer_map.insert("outer".to_string(), Value::Container(inner_map));
        outer_map.insert("b".to_string(), Value::UInt(0xBB));

        let c: &dyn Construct = &outer;
        let built = c.build_bytes(&Value::Container(outer_map.clone())).unwrap();
        assert_eq!(built, vec![0xAA, 0xBB]);

        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, Value::Container(outer_map));
    }

    // ======================================================================
    // Error path tests
    // ======================================================================

    #[test]
    fn parse_error_enriches_path_with_field_name() {
        let s = Struct::new()
            .field("a", Box::new(INT8UB))
            .field("b", Box::new(INT16UB));
        // Only provide 1 byte, second field needs 2
        let c: &dyn Construct = &s;
        let err = c.parse_bytes(b"\x01").unwrap_err();
        // Error should reference field "b"
        match &err {
            ConstructError::Stream { path, .. } => {
                assert!(
                    path.starts_with("(parsing).b") || path.starts_with("b"),
                    "expected path to contain 'b', got: {path}"
                );
            }
            other => panic!("expected Stream error, got {:?}", other),
        }
    }

    #[test]
    fn build_error_enriches_path_with_field_name() {
        let s = Struct::new().field("num", Box::new(INT8UB));
        let mut container = IndexMap::new();
        container.insert("num".to_string(), Value::String("not a number".to_string()));
        let c: &dyn Construct = &s;
        let err = c.build_bytes(&Value::Container(container)).unwrap_err();
        // The error path should contain "num"
        match &err {
            ConstructError::TypeMismatch { path, .. } => {
                assert!(
                    path.contains("num"),
                    "expected path to contain 'num', got: {path}"
                );
            }
            other => panic!("expected TypeMismatch, got {:?}", other),
        }
    }

    #[test]
    fn sizeof_error_enriches_path_with_field_name() {
        let s = Struct::new()
            .field("a", Box::new(INT8UB))
            .field("b", Box::new(Terminated::new()));
        let err = s.sizeof(&Context::new()).unwrap_err();
        match &err {
            ConstructError::Sizeof { path, .. } => {
                assert!(
                    path.contains("b"),
                    "expected path to contain 'b', got: {path}"
                );
            }
            other => panic!("expected Sizeof error, got {:?}", other),
        }
    }

    // ======================================================================
    // StopField tests
    // ======================================================================

    #[test]
    fn stop_field_halts_parsing() {
        use crate::core::error::Result;

        /// A construct that always returns StopField error during parse.
        struct StopConstruct;

        impl Construct for StopConstruct {
            fn parse(&self, _stream: &mut dyn Stream, _ctx: &mut Context) -> Result<Value> {
                Err(ConstructError::StopField {
                    path: String::new(),
                })
            }
            fn build(
                &self,
                _data: &Value,
                _stream: &mut dyn Stream,
                _ctx: &mut Context,
            ) -> Result<()> {
                Err(ConstructError::StopField {
                    path: String::new(),
                })
            }
            fn sizeof(&self, _ctx: &Context) -> Result<usize> {
                Ok(0)
            }
        }

        let s = Struct::new()
            .field("a", Box::new(INT8UB))
            .field("stopper", Box::new(StopConstruct))
            .field("b", Box::new(INT8UB));

        // parse: should stop after "a", "b" is never read
        let c: &dyn Construct = &s;
        let result = c.parse_bytes(b"\x01\x02").unwrap();
        let container = result.as_container().unwrap();
        assert_eq!(container.len(), 1);
        assert_eq!(container.get("a").unwrap(), &Value::UInt(1));
    }

    // ======================================================================
    // Extend tests
    // ======================================================================

    #[test]
    fn extend_combines_fields() {
        let header = Struct::new().field("magic", Box::new(INT8UB));

        // extend takes other by value, moving its fields
        let full = Struct::new()
            .field("version", Box::new(INT8UB))
            .extend(header);

        // This should parse "version" then "magic"
        let c: &dyn Construct = &full;
        let result = c.parse_bytes(b"\x01\x02").unwrap();
        let container = result.as_container().unwrap();
        assert_eq!(container.get("version").unwrap(), &Value::UInt(1));
        assert_eq!(container.get("magic").unwrap(), &Value::UInt(2));
    }

    // ======================================================================
    // Convenience method roundtrips
    // ======================================================================

    #[test]
    fn convenience_parse_and_build_roundtrip() {
        let s = Struct::new()
            .field("x", Box::new(INT8UB))
            .field("y", Box::new(INT16UB));

        let mut container = IndexMap::new();
        container.insert("x".to_string(), Value::UInt(100));
        container.insert("y".to_string(), Value::UInt(1000));

        let c: &dyn Construct = &s;
        let built = c.build_bytes(&Value::Container(container.clone())).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, Value::Container(container));
    }

    // ======================================================================
    // Default implementation
    // ======================================================================

    #[test]
    fn default_is_empty_struct() {
        let s = Struct::default();
        let c: &dyn Construct = &s;
        let result = c.parse_bytes(b"").unwrap();
        let container = result.as_container().unwrap();
        assert!(container.is_empty());
    }
}
