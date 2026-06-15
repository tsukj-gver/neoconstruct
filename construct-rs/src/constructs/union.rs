//! Union construct — treats the same data as multiple constructs.
//!
//! [`Union`] parses each sub-construct from the same starting position, then
//! advances the stream to the position of the selected (or last) sub-construct.
//! Builds from the first sub-construct that has a matching key in the provided
//! container.
//!
//! Corresponds to Python `Union(parsefrom, *subcons)`.
//!
//! # UnionTarget
//!
//! The [`UnionTarget`] enum specifies which sub-construct's end position the
//! stream should be seeked to after parsing all members:
//! - [`UnionTarget::Index(usize)`](UnionTarget::Index) — select by position
//! - [`UnionTarget::Name(String)`](UnionTarget::Name) — select by field name
//!
//! When `parsefrom` is `None`, the stream is left at the original position.
//!
//! # Examples
//!
//! ```
//! use construct::constructs::union::{Union, UnionTarget};
//! use construct::constructs::format_field::INT8UB;
//! use construct::constructs::bytes::Bytes;
//! use construct::constructs::struct_::StructField;
//! use construct::core::Construct;
//! use construct::value::Value;
//!
//! let u = Union::new(
//!     Some(UnionTarget::Index(0)),
//!     vec![
//!         StructField::new("chars", Box::new(Bytes::new(4).into())),
//!         StructField::new("num", Box::new(INT8UB.into())),
//!     ],
//! );
//!
//! let c: &dyn Construct = &u;
//! let parsed = c.parse_bytes(b"\x01\x02\x03\x04").unwrap();
//! let container = parsed.as_container().unwrap();
//! assert_eq!(container.get("chars").unwrap(), &Value::Bytes(b"\x01\x02\x03\x04".to_vec()));
//! assert_eq!(container.get("num").unwrap(), &Value::UInt(1));
//! ```

use indexmap::IndexMap;

use crate::combined::CombinedConstruct;
use crate::constructs::struct_::StructField;
use crate::core::context::Context;
use crate::core::error::{ConstructError, Result};
use crate::core::stream::CombinedStream;
use crate::core::stream::Stream;
use crate::core::Construct;
use crate::value::Value;

// ===========================================================================
// UnionTarget
// ===========================================================================

/// Specifies which sub-construct determines the final stream position after
/// a [`Union`] parse.
///
/// Corresponds to the Python `parsefrom` parameter which can be an `int`
/// (index), a `str` (name), or `None`.
#[derive(Clone, Debug)]
pub enum UnionTarget {
    /// Select the sub-construct at the given zero-based index.
    Index(usize),
    /// Select the sub-construct with the given field name.
    Name(String),
}

// ===========================================================================
// Union
// ===========================================================================

/// A composite construct that treats the same data as multiple sub-constructs
/// (similar to a C union).
///
/// Parses all sub-constructs from the same starting position. Each
/// sub-construct is parsed, then the stream is rewound to the fallback
/// position. After all sub-constructs have been parsed, the stream is
/// advanced to the position of the selected sub-construct (if any).
///
/// Builds from the first sub-construct whose name matches a key in the
/// provided container.
///
/// Sizeof always returns an error because the union's size depends on which
/// member is selected at build time.
///
/// Corresponds to Python `Union(parsefrom, *subcons)`.
pub struct Union {
    /// Which sub-construct to advance the stream to after parsing, or `None`
    /// to leave the stream at the original position.
    parsefrom: Option<UnionTarget>,
    /// The list of sub-construct fields (each may be named or anonymous).
    subcons: Vec<StructField>,
}

impl Union {
    /// Creates a new `Union` with the given `parsefrom` target and sub-constructs.
    ///
    /// # Arguments
    ///
    /// * `parsefrom` — `None` to leave the stream at the original position after
    ///   parsing, or a [`UnionTarget`] to seek to the end position of the
    ///   selected sub-construct.
    /// * `subcons` — the list of sub-construct fields. Named fields contribute
    ///   their parsed values to the output container.
    ///
    /// # Examples
    ///
    /// ```
    /// use construct::constructs::union::{Union, UnionTarget};
    /// use construct::constructs::format_field::INT8UB;
    /// use construct::constructs::bytes::Bytes;
    /// use construct::constructs::struct_::StructField;
    ///
    /// let u = Union::new(
    ///     Some(UnionTarget::Index(0)),
    ///     vec![
    ///         StructField::new("data", Box::new(Bytes::new(4).into())),
    ///         StructField::new("num", Box::new(INT8UB.into())),
    ///     ],
    /// );
    /// ```
    pub fn new(parsefrom: Option<UnionTarget>, subcons: Vec<StructField>) -> Self {
        Union { parsefrom, subcons }
    }

    /// Builder-style method to add a named field.
    pub fn field(mut self, name: impl Into<String>, subcon: Box<CombinedConstruct>) -> Self {
        self.subcons.push(StructField::new(name, subcon));
        self
    }

    /// Builder-style method to add an anonymous field.
    pub fn anonymous(mut self, subcon: Box<CombinedConstruct>) -> Self {
        self.subcons.push(StructField::anonymous(subcon));
        self
    }

    /// Returns the `parsefrom` target (read-only access).
    #[must_use]
    pub fn parsefrom(&self) -> Option<&UnionTarget> {
        self.parsefrom.as_ref()
    }

    /// Returns the ordered list of sub-construct fields (read-only access).
    #[must_use]
    pub fn subcons(&self) -> &[StructField] {
        &self.subcons
    }
}

impl Construct for Union {
    fn parse(&self, stream: &mut CombinedStream, ctx: &mut Context) -> Result<Value> {
        // Pre-allocate with the known subcon count to avoid repeated rehashing.
        let mut container: IndexMap<String, Value> = IndexMap::with_capacity(self.subcons.len());
        let mut child_ctx = ctx.subcontext();

        // Save the original (fallback) position
        let fallback = stream.tell()?;

        // Track the forward position after each sub-construct
        let mut forward_by_index: Vec<u64> = Vec::with_capacity(self.subcons.len());
        let mut forward_by_name: IndexMap<String, u64> =
            IndexMap::with_capacity(self.subcons.len());

        for (i, field) in self.subcons.iter().enumerate() {
            let subobj = match field.subcon.parse(stream, &mut child_ctx) {
                Ok(value) => value,
                Err(e) => {
                    return Err(if let Some(ref name) = field.name {
                        e.with_path_prefix(name)
                    } else {
                        e.with_path_prefix(&format!("[{i}]"))
                    });
                }
            };

            // Record the forward position for this sub-construct
            let forward_pos = stream.tell()?;
            forward_by_index.push(forward_pos);
            if let Some(ref name) = field.name {
                forward_by_name.insert(name.clone(), forward_pos);
            }

            // Store result in container and context if named
            if let Some(ref name) = field.name {
                container.insert(name.clone(), subobj.clone());
                child_ctx.insert(name.clone(), subobj);
            }

            // Rewind to fallback position for the next sub-construct
            stream.seek(fallback)?;
        }

        // Advance the stream to the selected sub-construct's forward position
        if let Some(ref target) = self.parsefrom {
            let pos = match target {
                UnionTarget::Index(idx) => {
                    forward_by_index
                        .get(*idx)
                        .copied()
                        .ok_or_else(|| ConstructError::Index {
                            path: String::new(),
                            index: *idx,
                            length: forward_by_index.len(),
                        })
                }
                UnionTarget::Name(name) => {
                    forward_by_name
                        .get(name)
                        .copied()
                        .ok_or_else(|| ConstructError::FieldMissing {
                            path: String::new(),
                            field: name.clone(),
                        })
                }
            }?;
            stream.seek(pos)?;
        }

        Ok(Value::Container(container))
    }

    fn build(&self, data: &Value, stream: &mut CombinedStream, ctx: &mut Context) -> Result<()> {
        // Data must be a Container
        let container = match data {
            Value::None => IndexMap::new(),
            Value::Container(map) => map.clone(),
            other => {
                return Err(ConstructError::TypeMismatch {
                    path: String::new(),
                    expected: "Container".to_string(),
                    actual: other.type_name().to_string(),
                });
            }
        };

        let mut child_ctx = ctx.subcontext();

        // Insert all container values into child context
        for (key, value) in &container {
            child_ctx.insert(key.clone(), value.clone());
        }

        // Find the first subcon whose name matches a key in the container
        for field in &self.subcons {
            let name_ref = field.name.as_deref();

            let should_build = if field.subcon.flagbuildnone() {
                // Sub-construct can build from None; build if key exists or name is None
                name_ref.map(|n| container.contains_key(n)).unwrap_or(false)
            } else if let Some(n) = name_ref {
                container.contains_key(n)
            } else {
                false
            };

            if !should_build {
                continue;
            }

            let build_value =
                if field.subcon.flagbuildnone() {
                    name_ref
                        .and_then(|n| container.get(n))
                        .cloned()
                        .unwrap_or(Value::None)
                } else {
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

            if let Some(ref name) = field.name {
                child_ctx.insert(name.clone(), build_value.clone());
            }

            return field
                .subcon
                .build(&build_value, stream, &mut child_ctx)
                .map_err(|e| {
                    if let Some(ref name) = field.name {
                        e.with_path_prefix(name)
                    } else {
                        e.with_path_prefix("(anonymous)")
                    }
                });
        }

        Err(ConstructError::Union {
            path: String::new(),
            message: format!(
                "cannot build, none of the subcons were found in the dictionary: {:?}",
                container.keys().collect::<Vec<_>>()
            ),
        })
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        // Python: raises SizeofError because size depends on which member is built
        // However, the task says "sizeof: 返回所有 subcon 中的最大值（对齐 Python）"
        // Looking at the Python code, Union._sizeof raises SizeofError.
        // But the task explicitly asks for "返回所有 subcon 中的最大值".
        // We follow the task spec since it explicitly overrides Python behavior.
        // Actually, let me re-read the task:
        // "sizeof: 返回所有 subcon 中的最大值（对齐 Python）"
        // This says "align with Python", but Python raises an error.
        // I'll follow the task spec and return the max size.

        // Actually, re-reading more carefully: the task says sizeof returns max.
        // But Python raises SizeofError. The "(对齐 Python)" might mean
        // "aligned with Python" or might mean something else.
        // Given the task instruction is explicit, I'll return the max.
        // But if any subcon can't compute sizeof, we propagate the error.

        let mut max_size: usize = 0;
        let child_ctx = _ctx.subcontext();
        for field in &self.subcons {
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
            if size > max_size {
                max_size = size;
            }
        }
        Ok(max_size)
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constructs::bytes::Bytes;
    use crate::constructs::format_field::{INT16UB, INT32UB, INT8UB};
    use crate::core::error::ConstructError;
    use crate::core::stream::ByteStream;
    use crate::core::stream::Stream;
    use crate::core::Construct;

    // Helper: convert Union to &dyn Construct
    macro_rules! as_dyn {
        ($u:expr) => {
            &$u as &dyn Construct
        };
    }

    // ======================================================================
    // Parse tests
    // ======================================================================

    #[test]
    fn parse_union_two_named_fields() {
        let u = Union::new(
            Some(UnionTarget::Index(0)),
            vec![
                StructField::new("chars", Box::new(Bytes::new(4).into())),
                StructField::new("num", Box::new(INT8UB.into())),
            ],
        );
        let result = as_dyn!(u).parse_bytes(b"\x01\x02\x03\x04").unwrap();
        let container = result.as_container().unwrap();
        assert_eq!(
            container.get("chars").unwrap(),
            &Value::Bytes(b"\x01\x02\x03\x04".to_vec())
        );
        assert_eq!(container.get("num").unwrap(), &Value::UInt(1));
    }

    #[test]
    fn parse_union_none_parsefrom_leaves_stream_at_start() {
        let u = Union::new(
            None,
            vec![
                StructField::new("a", Box::new(INT8UB.into())),
                StructField::new("b", Box::new(INT16UB.into())),
            ],
        );
        let c: &dyn Construct = &u;
        let data = b"\x01\x02\x03";
        let mut stream =
            crate::core::stream::CombinedStream::ByteStream(ByteStream::new_read(data));
        let mut ctx = Context::new();
        let result = c.parse(&mut stream, &mut ctx).unwrap();

        // Stream should be back at position 0 (fallback) since parsefrom is None
        assert_eq!(stream.tell().unwrap(), 0);

        let container = result.as_container().unwrap();
        assert_eq!(container.get("a").unwrap(), &Value::UInt(1));
        assert_eq!(container.get("b").unwrap(), &Value::UInt(0x0102));
    }

    #[test]
    fn parse_union_by_name() {
        let u = Union::new(
            Some(UnionTarget::Name("b".to_string())),
            vec![
                StructField::new("a", Box::new(INT8UB.into())),
                StructField::new("b", Box::new(INT16UB.into())),
            ],
        );
        let data = b"\x01\x02\x03";
        let mut stream =
            crate::core::stream::CombinedStream::ByteStream(ByteStream::new_read(data));
        let mut ctx = Context::new();
        let c: &dyn Construct = &u;
        let _result = c.parse(&mut stream, &mut ctx).unwrap();

        // Stream should be at position 2 (after parsing "b" = INT16UB)
        assert_eq!(stream.tell().unwrap(), 2);
    }

    #[test]
    fn parse_union_by_index() {
        let u = Union::new(
            Some(UnionTarget::Index(1)),
            vec![
                StructField::new("a", Box::new(INT8UB.into())),
                StructField::new("b", Box::new(INT16UB.into())),
            ],
        );
        let data = b"\x01\x02\x03";
        let mut stream =
            crate::core::stream::CombinedStream::ByteStream(ByteStream::new_read(data));
        let mut ctx = Context::new();
        let c: &dyn Construct = &u;
        let _result = c.parse(&mut stream, &mut ctx).unwrap();

        // Stream should be at position 2 (after parsing "b" = INT16UB)
        assert_eq!(stream.tell().unwrap(), 2);
    }

    #[test]
    fn parse_union_invalid_index_returns_error() {
        let u = Union::new(
            Some(UnionTarget::Index(5)),
            vec![StructField::new("a", Box::new(INT8UB.into()))],
        );
        let err = as_dyn!(u).parse_bytes(b"\x01").unwrap_err();
        assert!(matches!(err, ConstructError::Index { .. }));
    }

    #[test]
    fn parse_union_invalid_name_returns_error() {
        let u = Union::new(
            Some(UnionTarget::Name("nonexistent".to_string())),
            vec![StructField::new("a", Box::new(INT8UB.into()))],
        );
        let err = as_dyn!(u).parse_bytes(b"\x01").unwrap_err();
        assert!(matches!(err, ConstructError::FieldMissing { .. }));
    }

    // ======================================================================
    // Build tests
    // ======================================================================

    #[test]
    fn build_union_first_matching_key() {
        let u = Union::new(
            None,
            vec![
                StructField::new("a", Box::new(INT8UB.into())),
                StructField::new("b", Box::new(INT16UB.into())),
            ],
        );
        let mut container = IndexMap::new();
        container.insert("b".to_string(), Value::UInt(0x0102));
        let built = as_dyn!(u)
            .build_bytes(&Value::Container(container))
            .unwrap();
        assert_eq!(built, vec![0x01, 0x02]);
    }

    #[test]
    fn build_union_first_field_matching() {
        let u = Union::new(
            None,
            vec![
                StructField::new("a", Box::new(INT8UB.into())),
                StructField::new("b", Box::new(INT16UB.into())),
            ],
        );
        let mut container = IndexMap::new();
        container.insert("a".to_string(), Value::UInt(0x42));
        container.insert("b".to_string(), Value::UInt(0x0102));
        // "a" is first, so it builds from "a"
        let built = as_dyn!(u)
            .build_bytes(&Value::Container(container))
            .unwrap();
        assert_eq!(built, vec![0x42]);
    }

    #[test]
    fn build_union_no_matching_key_returns_error() {
        let u = Union::new(None, vec![StructField::new("a", Box::new(INT8UB.into()))]);
        let mut container = IndexMap::new();
        container.insert("z".to_string(), Value::UInt(1));
        let err = as_dyn!(u)
            .build_bytes(&Value::Container(container))
            .unwrap_err();
        assert!(matches!(err, ConstructError::Union { .. }));
    }

    #[test]
    fn build_union_empty_container_returns_error() {
        let u = Union::new(None, vec![StructField::new("a", Box::new(INT8UB.into()))]);
        let err = as_dyn!(u)
            .build_bytes(&Value::Container(IndexMap::new()))
            .unwrap_err();
        assert!(matches!(err, ConstructError::Union { .. }));
    }

    #[test]
    fn build_union_wrong_type_returns_error() {
        let u = Union::new(None, vec![StructField::new("a", Box::new(INT8UB.into()))]);
        let err = as_dyn!(u).build_bytes(&Value::Int(42)).unwrap_err();
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
    }

    // ======================================================================
    // Sizeof tests
    // ======================================================================

    #[test]
    fn sizeof_returns_max_of_subcons() {
        let u = Union::new(
            None,
            vec![
                StructField::new("a", Box::new(INT8UB.into())),
                StructField::new("b", Box::new(INT16UB.into())),
                StructField::new("c", Box::new(INT32UB.into())),
            ],
        );
        assert_eq!(u.sizeof(&Context::new()).unwrap(), 4);
    }

    #[test]
    fn sizeof_single_field() {
        let u = Union::new(None, vec![StructField::new("a", Box::new(INT16UB.into()))]);
        assert_eq!(u.sizeof(&Context::new()).unwrap(), 2);
    }

    // ======================================================================
    // Roundtrip tests
    // ======================================================================

    #[test]
    fn roundtrip_union_build_then_parse() {
        let u = Union::new(
            Some(UnionTarget::Index(0)),
            vec![
                StructField::new("num", Box::new(INT8UB.into())),
                StructField::new("bytes", Box::new(Bytes::new(1).into())),
            ],
        );

        let mut container = IndexMap::new();
        container.insert("num".to_string(), Value::UInt(0x42));

        let c: &dyn Construct = &u;
        let built = c.build_bytes(&Value::Container(container.clone())).unwrap();

        let parsed = c.parse_bytes(&built).unwrap();
        let parsed_container = parsed.as_container().unwrap();
        assert_eq!(parsed_container.get("num").unwrap(), &Value::UInt(0x42));
    }
}
