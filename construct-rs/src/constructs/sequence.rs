//! Sequence construct — a sequence of usually un-named sub-constructs.
//!
//! [`Sequence`] parses a series of sub-constructs into a [`Value::List`]
//! (ordered list of values) and builds binary data from such a list.
//!
//! Corresponds to Python `Sequence(*subcons)`.
//!
//! # Entry types
//!
//! Each entry in a `Sequence` is either:
//! - **Named** — has a string name; its parsed value is inserted into the
//!   context so that later entries can reference earlier ones.
//! - **Anonymous** — has no name (`None`); its parsed value is still appended
//!   to the output list.
//!
//! # Context nesting
//!
//! During parsing and building, `Sequence` creates a child context (via
//! [`Context::subcontext`]) so that later entries can reference earlier ones.
//!
//! # Examples
//!
//! ```
//! use construct::constructs::sequence::Sequence;
//! use construct::constructs::format_field::INT8UB;
//! use construct::constructs::bytes::Bytes;
//! use construct::core::Construct;
//! use construct::value::Value;
//!
//! let s = Sequence::new()
//!     .push(Box::new(INT8UB))
//!     .push(Box::new(Bytes::new(3)));
//!
//! let c: &dyn Construct = &s;
//! let parsed = c.parse_bytes(b"\x04ABC").unwrap();
//! let list = parsed.as_list().unwrap();
//! assert_eq!(list.len(), 2);
//! assert_eq!(list[0], Value::UInt(4));
//! assert_eq!(list[1], Value::Bytes(b"ABC".to_vec()));
//! ```

use crate::core::context::Context;
use crate::core::error::{ConstructError, Result};
use crate::core::stream::Stream;
use crate::core::Construct;
use crate::value::Value;

// ===========================================================================
// SeqEntry
// ===========================================================================

/// A single entry within a [`Sequence`].
///
/// An entry wraps a sub-construct and optionally associates it with a name.
/// Named entries contribute their parsed value to the context for later
/// entries to reference; anonymous entries are parsed normally but do not
/// update the context.
///
/// Corresponds to a single positional argument in Python's
/// `Sequence(*subcons)`.
pub struct SeqEntry {
    /// The entry name, or `None` for anonymous entries.
    pub name: Option<String>,
    /// The sub-construct that parses / builds this entry.
    pub subcon: Box<dyn Construct>,
}

impl SeqEntry {
    /// Creates a new named entry.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let entry = SeqEntry::new("count", Box::new(INT8UB));
    /// ```
    pub fn new(name: impl Into<String>, subcon: Box<dyn Construct>) -> Self {
        SeqEntry {
            name: Some(name.into()),
            subcon,
        }
    }

    /// Creates a new anonymous entry.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let entry = SeqEntry::anonymous(Box::new(INT8UB));
    /// ```
    pub fn anonymous(subcon: Box<dyn Construct>) -> Self {
        SeqEntry { name: None, subcon }
    }
}

// ===========================================================================
// Sequence
// ===========================================================================

/// A composite construct that parses a sequence of named/anonymous entries
/// into a [`Value::List`].
///
/// Parses into a `List` (ordered vector) where values are in the same order
/// as entries. Builds from a `List` where each entry is given the element at
/// the corresponding position. Size is the sum of all entry sizes, unless any
/// entry raises a sizeof error.
///
/// Corresponds to Python `Sequence`.
///
/// # Context nesting
///
/// During parse, a child context is created so that later entries can
/// reference earlier parsed values (only named entries are inserted into the
/// context). During build, named entries are inserted into the context before
/// each entry is built.
///
/// # StopField support
///
/// If a sub-construct returns [`ConstructError::StopField`], the `Sequence`
/// stops processing further entries and returns successfully.
pub struct Sequence {
    /// The ordered list of entries in this sequence.
    entries: Vec<SeqEntry>,
}

impl Sequence {
    /// Creates a new, empty `Sequence`.
    ///
    /// Use the builder methods [`push`](Sequence::push) to add entries.
    ///
    /// # Examples
    ///
    /// ```
    /// use construct::constructs::sequence::Sequence;
    ///
    /// let s = Sequence::new();
    /// ```
    pub fn new() -> Self {
        Sequence {
            entries: Vec::new(),
        }
    }

    /// Adds an anonymous entry to this sequence, returning `self` for chaining.
    ///
    /// # Examples
    ///
    /// ```
    /// use construct::constructs::sequence::Sequence;
    /// use construct::constructs::format_field::INT8UB;
    ///
    /// let s = Sequence::new().push(Box::new(INT8UB));
    /// ```
    pub fn push(mut self, subcon: Box<dyn Construct>) -> Self {
        self.entries.push(SeqEntry::anonymous(subcon));
        self
    }

    /// Adds a named entry to this sequence, returning `self` for chaining.
    ///
    /// Named entries have their parsed values inserted into the context so
    /// that later entries can reference them.
    ///
    /// # Examples
    ///
    /// ```
    /// use construct::constructs::sequence::Sequence;
    /// use construct::constructs::format_field::INT8UB;
    ///
    /// let s = Sequence::new().named("count", Box::new(INT8UB));
    /// ```
    pub fn named(mut self, name: impl Into<String>, subcon: Box<dyn Construct>) -> Self {
        self.entries.push(SeqEntry::new(name, subcon));
        self
    }

    /// Appends all entries from another `Sequence` to this one, consuming the
    /// other sequence.
    ///
    /// This allows composing sequences from reusable building blocks.
    ///
    /// # Examples
    ///
    /// ```
    /// use construct::constructs::sequence::Sequence;
    /// use construct::constructs::format_field::INT8UB;
    ///
    /// let header = Sequence::new().push(Box::new(INT8UB));
    /// let full = Sequence::new().push(Box::new(INT8UB)).extend(header);
    /// ```
    pub fn extend(mut self, other: Sequence) -> Self {
        let other_entries = other.entries;
        for entry in other_entries {
            self.entries.push(entry);
        }
        self
    }
}

impl Default for Sequence {
    fn default() -> Self {
        Self::new()
    }
}

impl Construct for Sequence {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        // Pre-allocate with the known entry count to avoid reallocations.
        let mut list: Vec<Value> = Vec::with_capacity(self.entries.len());
        let mut child_ctx = ctx.subcontext();

        for entry in &self.entries {
            let subobj = match entry.subcon.parse(stream, &mut child_ctx) {
                Ok(value) => value,
                Err(ConstructError::StopField { .. }) => break,
                Err(e) => {
                    return Err(if let Some(ref name) = entry.name {
                        e.with_path_prefix(name)
                    } else {
                        let idx = list.len();
                        e.with_path_prefix(&format!("[{idx}]"))
                    });
                }
            };

            // Always append to the list (unlike Struct which skips anonymous)
            list.push(subobj.clone());

            // Named entries also update the context
            if let Some(ref name) = entry.name {
                child_ctx.insert(name.clone(), subobj);
            }
        }

        Ok(Value::List(list))
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        // If data is None, treat as list of Nones matching entry count
        let list = match data {
            Value::None => vec![Value::None; self.entries.len()],
            Value::List(items) => items.clone(),
            other => {
                return Err(ConstructError::TypeMismatch {
                    path: String::new(),
                    expected: "List".to_string(),
                    actual: other.type_name().to_string(),
                });
            }
        };

        // Check that list has enough elements
        if list.len() < self.entries.len() {
            return Err(ConstructError::Array {
                path: String::new(),
                expected: self.entries.len(),
                actual: list.len(),
            });
        }

        let mut child_ctx = ctx.subcontext();

        for (i, entry) in self.entries.iter().enumerate() {
            let build_value = list[i].clone();

            // Named entries are inserted into context before building
            if let Some(ref name) = entry.name {
                child_ctx.insert(name.clone(), build_value.clone());
            }

            let result = match entry.subcon.build(&build_value, stream, &mut child_ctx) {
                Ok(()) => Ok(()),
                Err(ConstructError::StopField { .. }) => return Ok(()),
                Err(e) => Err(if let Some(ref name) = entry.name {
                    e.with_path_prefix(name)
                } else {
                    e.with_path_prefix(&format!("[{i}]"))
                }),
            };

            result?;
        }

        Ok(())
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        let child_ctx = ctx.subcontext();
        let mut total: usize = 0;
        for entry in &self.entries {
            let size = match entry.subcon.sizeof(&child_ctx) {
                Ok(s) => s,
                Err(e) => {
                    return Err(if let Some(ref name) = entry.name {
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
        self.entries.iter().all(|e| e.subcon.flagbuildnone())
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

    // Helper: convert Sequence to &dyn Construct for convenience methods
    macro_rules! as_dyn {
        ($s:expr) => {
            &$s as &dyn Construct
        };
    }

    // ======================================================================
    // Basic parse tests
    // ======================================================================

    #[test]
    fn parse_empty_sequence_returns_empty_list() {
        let s = Sequence::new();
        let result = as_dyn!(s).parse_bytes(b"").unwrap();
        let list = result.as_list().unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn parse_single_anonymous_entry() {
        let s = Sequence::new().push(Box::new(INT8UB));
        let result = as_dyn!(s).parse_bytes(b"\x2A").unwrap();
        let list = result.as_list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0], Value::UInt(42));
    }

    #[test]
    fn parse_two_anonymous_entries() {
        let s = Sequence::new()
            .push(Box::new(INT8UB))
            .push(Box::new(INT8UB));
        let result = as_dyn!(s).parse_bytes(b"\x01\x02").unwrap();
        let list = result.as_list().unwrap();
        assert_eq!(list[0], Value::UInt(1));
        assert_eq!(list[1], Value::UInt(2));
    }

    #[test]
    fn parse_named_entry_appends_to_list() {
        let s = Sequence::new().named("num", Box::new(INT8UB));
        let result = as_dyn!(s).parse_bytes(b"\x2A").unwrap();
        let list = result.as_list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0], Value::UInt(42));
    }

    #[test]
    fn parse_mixed_named_and_anonymous() {
        let s = Sequence::new()
            .named("a", Box::new(INT8UB))
            .push(Box::new(INT8UB))
            .named("b", Box::new(INT8UB));
        let result = as_dyn!(s).parse_bytes(b"\x01\x02\x03").unwrap();
        let list = result.as_list().unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0], Value::UInt(1));
        assert_eq!(list[1], Value::UInt(2));
        assert_eq!(list[2], Value::UInt(3));
    }

    #[test]
    fn parse_context_visible_to_later_entries() {
        // Named entry "length" is in context, later entry can use it
        let s = Sequence::new()
            .named("length", Box::new(INT8UB))
            .push(Box::new(Bytes::new(3)));
        let result = as_dyn!(s).parse_bytes(b"\x03ABC").unwrap();
        let list = result.as_list().unwrap();
        assert_eq!(list[0], Value::UInt(3));
        assert_eq!(list[1], Value::Bytes(b"ABC".to_vec()));
    }

    // ======================================================================
    // Basic build tests
    // ======================================================================

    #[test]
    fn build_empty_sequence_returns_empty_bytes() {
        let s = Sequence::new();
        let built = as_dyn!(s).build_bytes(&Value::List(Vec::new())).unwrap();
        assert!(built.is_empty());
    }

    #[test]
    fn build_single_entry() {
        let s = Sequence::new().push(Box::new(INT8UB));
        let built = as_dyn!(s)
            .build_bytes(&Value::List(vec![Value::UInt(42)]))
            .unwrap();
        assert_eq!(built, vec![42]);
    }

    #[test]
    fn build_two_entries() {
        let s = Sequence::new()
            .push(Box::new(INT8UB))
            .push(Box::new(INT8UB));
        let built = as_dyn!(s)
            .build_bytes(&Value::List(vec![Value::UInt(1), Value::UInt(2)]))
            .unwrap();
        assert_eq!(built, vec![1, 2]);
    }

    #[test]
    fn build_mixed_types() {
        let s = Sequence::new()
            .push(Box::new(INT8UB))
            .push(Box::new(INT16UB));
        let built = as_dyn!(s)
            .build_bytes(&Value::List(vec![Value::UInt(0x12), Value::UInt(0x3456)]))
            .unwrap();
        assert_eq!(built, vec![0x12, 0x34, 0x56]);
    }

    #[test]
    fn build_from_none_creates_list_of_nones() {
        // When all subcons have flagbuildnone=true, Sequence can build from None
        let s = Sequence::new()
            .push(Box::new(Const::new_bytes(b"HI".to_vec())))
            .push(Box::new(Pass::new()));
        let built = as_dyn!(s).build_bytes(&Value::None).unwrap();
        assert_eq!(built, b"HI");
    }

    #[test]
    fn build_wrong_type_returns_error() {
        let s = Sequence::new().push(Box::new(INT8UB));
        let err = as_dyn!(s).build_bytes(&Value::Int(42)).unwrap_err();
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
    }

    #[test]
    fn build_too_few_elements_returns_error() {
        let s = Sequence::new()
            .push(Box::new(INT8UB))
            .push(Box::new(INT8UB));
        let err = as_dyn!(s)
            .build_bytes(&Value::List(vec![Value::UInt(1)]))
            .unwrap_err();
        assert!(matches!(err, ConstructError::Array { .. }));
    }

    #[test]
    fn build_named_entry_uses_context() {
        // Named entries should be available in context for later entries
        let s = Sequence::new()
            .named("a", Box::new(INT8UB))
            .push(Box::new(INT8UB));
        let built = as_dyn!(s)
            .build_bytes(&Value::List(vec![Value::UInt(0x10), Value::UInt(0x20)]))
            .unwrap();
        assert_eq!(built, vec![0x10, 0x20]);
    }

    // ======================================================================
    // Sizeof tests
    // ======================================================================

    #[test]
    fn sizeof_empty_sequence_is_0() {
        let s = Sequence::new();
        assert_eq!(s.sizeof(&Context::new()).unwrap(), 0);
    }

    #[test]
    fn sizeof_single_entry() {
        let s = Sequence::new().push(Box::new(INT8UB));
        assert_eq!(s.sizeof(&Context::new()).unwrap(), 1);
    }

    #[test]
    fn sizeof_multiple_entries() {
        let s = Sequence::new()
            .push(Box::new(INT8UB))
            .push(Box::new(INT16UB))
            .push(Box::new(INT32UB));
        assert_eq!(s.sizeof(&Context::new()).unwrap(), 7);
    }

    #[test]
    fn sizeof_with_named_entries() {
        let s = Sequence::new()
            .named("a", Box::new(INT8UB))
            .push(Box::new(Bytes::new(3)));
        assert_eq!(s.sizeof(&Context::new()).unwrap(), 4);
    }

    // ======================================================================
    // flagbuildnone tests
    // ======================================================================

    #[test]
    fn flagbuildnone_true_when_all_entries_are_build_none() {
        let s = Sequence::new()
            .push(Box::new(Pass::new()))
            .push(Box::new(Const::new_bytes(b"X".to_vec())));
        assert!(s.flagbuildnone());
    }

    #[test]
    fn flagbuildnone_false_when_any_entry_requires_value() {
        let s = Sequence::new()
            .push(Box::new(Pass::new()))
            .push(Box::new(INT8UB));
        assert!(!s.flagbuildnone());
    }

    #[test]
    fn flagbuildnone_true_for_empty_sequence() {
        let s = Sequence::new();
        assert!(s.flagbuildnone());
    }

    // ======================================================================
    // Roundtrip tests
    // ======================================================================

    #[test]
    fn roundtrip_two_entries() {
        let s = Sequence::new()
            .push(Box::new(INT8UB))
            .push(Box::new(INT16UB));

        let data = Value::List(vec![Value::UInt(0x12), Value::UInt(0x3456)]);

        let c: &dyn Construct = &s;
        let built = c.build_bytes(&data).unwrap();
        assert_eq!(built, vec![0x12, 0x34, 0x56]);

        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, data);
    }

    #[test]
    fn roundtrip_with_const() {
        let s = Sequence::new()
            .push(Box::new(Const::new_bytes(b"PK".to_vec())))
            .push(Box::new(INT8UB));

        let data = Value::List(vec![Value::Bytes(b"PK".to_vec()), Value::UInt(10)]);

        let c: &dyn Construct = &s;
        let built = c.build_bytes(&data).unwrap();
        assert_eq!(built, b"PK\x0A");

        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, data);
    }

    #[test]
    fn roundtrip_with_pass() {
        let s = Sequence::new()
            .push(Box::new(INT8UB))
            .push(Box::new(Pass::new()))
            .push(Box::new(INT8UB));

        let data = Value::List(vec![Value::UInt(1), Value::None, Value::UInt(2)]);

        let c: &dyn Construct = &s;
        let built = c.build_bytes(&data).unwrap();
        assert_eq!(built, vec![1, 2]);

        let parsed = c.parse_bytes(&built).unwrap();
        // Pass returns None on parse, so list is [1, None, 2]
        assert_eq!(parsed, data);
    }

    #[test]
    fn roundtrip_with_named_entries() {
        let s = Sequence::new()
            .named("x", Box::new(INT8UB))
            .named("y", Box::new(INT8UB));

        let data = Value::List(vec![Value::UInt(42), Value::UInt(99)]);

        let c: &dyn Construct = &s;
        let built = c.build_bytes(&data).unwrap();
        assert_eq!(built, vec![42, 99]);

        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, data);
    }

    // ======================================================================
    // Error path tests
    // ======================================================================

    #[test]
    fn parse_error_enriches_path_with_entry_name() {
        let s = Sequence::new()
            .named("a", Box::new(INT8UB))
            .named("b", Box::new(INT16UB));
        // Only provide 1 byte, second entry needs 2
        let c: &dyn Construct = &s;
        let err = c.parse_bytes(b"\x01").unwrap_err();
        // Error should reference entry "b"
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
    fn parse_error_anonymous_entry_uses_index() {
        let s = Sequence::new()
            .push(Box::new(INT8UB))
            .push(Box::new(INT16UB));
        // Only provide 1 byte, second entry needs 2
        let c: &dyn Construct = &s;
        let err = c.parse_bytes(b"\x01").unwrap_err();
        // Error should reference index [1]
        match &err {
            ConstructError::Stream { path, .. } => {
                assert!(
                    path.contains("[1]"),
                    "expected path to contain '[1]', got: {path}"
                );
            }
            other => panic!("expected Stream error, got {:?}", other),
        }
    }

    #[test]
    fn build_error_enriches_path_with_entry_name() {
        let s = Sequence::new().named("num", Box::new(INT8UB));
        let data = Value::List(vec![Value::String("not a number".to_string())]);
        let c: &dyn Construct = &s;
        let err = c.build_bytes(&data).unwrap_err();
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
    fn sizeof_error_enriches_path_with_entry_name() {
        let s = Sequence::new()
            .named("a", Box::new(INT8UB))
            .named("b", Box::new(Terminated::new()));
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

        let s = Sequence::new()
            .push(Box::new(INT8UB))
            .push(Box::new(StopConstruct))
            .push(Box::new(INT8UB));

        // parse: should stop after first entry, third is never read
        let c: &dyn Construct = &s;
        let result = c.parse_bytes(b"\x01\x02").unwrap();
        let list = result.as_list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0], Value::UInt(1));
    }

    // ======================================================================
    // Extend tests
    // ======================================================================

    #[test]
    fn extend_combines_entries() {
        let header = Sequence::new().push(Box::new(INT8UB));

        let full = Sequence::new().push(Box::new(INT8UB)).extend(header);

        // This should parse first entry then second entry
        let c: &dyn Construct = &full;
        let result = c.parse_bytes(b"\x01\x02").unwrap();
        let list = result.as_list().unwrap();
        assert_eq!(list[0], Value::UInt(1));
        assert_eq!(list[1], Value::UInt(2));
    }

    // ======================================================================
    // Convenience method roundtrips
    // ======================================================================

    #[test]
    fn convenience_parse_and_build_roundtrip() {
        let s = Sequence::new()
            .push(Box::new(INT8UB))
            .push(Box::new(INT16UB));

        let data = Value::List(vec![Value::UInt(100), Value::UInt(1000)]);

        let c: &dyn Construct = &s;
        let built = c.build_bytes(&data).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, data);
    }

    // ======================================================================
    // Default implementation
    // ======================================================================

    #[test]
    fn default_is_empty_sequence() {
        let s = Sequence::default();
        let c: &dyn Construct = &s;
        let result = c.parse_bytes(b"").unwrap();
        let list = result.as_list().unwrap();
        assert!(list.is_empty());
    }

    // ======================================================================
    // Extra elements in list are ignored during build
    // ======================================================================

    #[test]
    fn build_ignores_extra_elements_in_list() {
        let s = Sequence::new().push(Box::new(INT8UB));
        // Provide 2 elements when only 1 is needed
        let built = as_dyn!(s)
            .build_bytes(&Value::List(vec![Value::UInt(42), Value::UInt(99)]))
            .unwrap();
        assert_eq!(built, vec![42]);
    }
}
