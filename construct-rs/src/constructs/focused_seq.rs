//! FocusedSeq construct — parses a sequence but returns only one focused field.
//!
//! [`FocusedSeq`] parses all sub-constructs in sequence (like a [`Struct`]),
//! but returns only the value of the "focused" field. During build, the focused
//! field receives the build object, while all other fields receive `Value::None`.
//!
//! Corresponds to Python `FocusedSeq(parsebuildfrom, *subcons)`.
//!
//! # Examples
//!
//! ```
//! use construct::constructs::focused_seq::FocusedSeq;
//! use construct::constructs::format_field::INT8UB;
//! use construct::constructs::const_::Const;
//! use construct::core::Construct;
//! use construct::value::Value;
//!
//! let fs = FocusedSeq::new(
//!     "num",
//!     vec![
//!         construct::constructs::struct_::StructField::anonymous(
//!             Box::new(Const::new_bytes(b"SIG".to_vec()).into()),
//!         ),
//!         construct::constructs::struct_::StructField::new("num", Box::new(INT8UB.into())),
//!     ],
//! );
//!
//! let c: &dyn Construct = &fs;
//! let parsed = c.parse_bytes(b"SIG\xFF").unwrap();
//! assert_eq!(parsed, Value::UInt(255));
//!
//! let built = c.build_bytes(&Value::UInt(255)).unwrap();
//! assert_eq!(built, b"SIG\xFF");
//! ```
//!
//! [`Struct`]: crate::constructs::Struct

use crate::combined::CombinedConstruct;
use crate::constructs::struct_::StructField;
use crate::core::context::Context;
use crate::core::error::{ConstructError, Result};
use crate::core::stream::CombinedStream;
use crate::core::Construct;
use crate::value::Value;

// ===========================================================================
// FocusedSeq
// ===========================================================================

/// A composite construct that parses a sequence of fields but returns only
/// the value of the "focused" field.
///
/// During parse, all sub-constructs are parsed in order (with context nesting),
/// but only the value of the field matching `parsebuildfrom` is returned.
///
/// During build, the focused field receives the build object directly; all
/// other fields receive `Value::None`.
///
/// Size is the sum of all sub-construct sizes, matching the Python behavior.
///
/// Corresponds to Python `FocusedSeq`.
pub struct FocusedSeq {
    /// The name of the field to focus on (return from parse, use for build).
    parsebuildfrom: String,
    /// The list of sub-construct fields.
    subcons: Vec<StructField>,
}

impl FocusedSeq {
    /// Creates a new `FocusedSeq` with the given focus field name and
    /// sub-constructs.
    ///
    /// # Arguments
    ///
    /// * `parsebuildfrom` — the name of the field whose value is returned from
    ///   parse and used as the build object.
    /// * `subcons` — the list of sub-construct fields.
    ///
    /// # Examples
    ///
    /// ```
    /// use construct::constructs::focused_seq::FocusedSeq;
    /// use construct::constructs::format_field::INT8UB;
    /// use construct::constructs::struct_::StructField;
    ///
    /// let fs = FocusedSeq::new(
    ///     "value",
    ///     vec![
    ///         StructField::new("value", Box::new(INT8UB.into())),
    ///     ],
    /// );
    /// ```
    pub fn new(parsebuildfrom: impl Into<String>, subcons: Vec<StructField>) -> Self {
        FocusedSeq {
            parsebuildfrom: parsebuildfrom.into(),
            subcons,
        }
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

    /// Returns the name of the focused field.
    #[must_use]
    pub fn parsebuildfrom(&self) -> &str {
        &self.parsebuildfrom
    }

    /// Returns the ordered list of sub-construct fields (read-only access).
    #[must_use]
    pub fn subcons(&self) -> &[StructField] {
        &self.subcons
    }
}

impl Construct for FocusedSeq {
    fn parse(&self, stream: &mut CombinedStream, ctx: &mut Context) -> Result<Value> {
        let mut child_ctx = ctx.subcontext();
        let mut focused_value: Option<Value> = None;

        for field in &self.subcons {
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

            // Named fields are inserted into context
            if let Some(ref name) = field.name {
                child_ctx.insert(name.clone(), subobj.clone());

                // Track the focused field's value
                if *name == self.parsebuildfrom {
                    focused_value = Some(subobj);
                }
            }
        }

        focused_value.ok_or_else(|| ConstructError::FieldMissing {
            path: String::new(),
            field: self.parsebuildfrom.clone(),
        })
    }

    fn build(&self, data: &Value, stream: &mut CombinedStream, ctx: &mut Context) -> Result<()> {
        let mut child_ctx = ctx.subcontext();

        // Insert the focused value into context under its name
        child_ctx.insert(self.parsebuildfrom.clone(), data.clone());

        for field in &self.subcons {
            let name_ref = field.name.as_deref();

            // The focused field gets the actual data; others get None
            let build_value = if name_ref == Some(self.parsebuildfrom.as_str()) {
                data.clone()
            } else {
                Value::None
            };

            // Insert into context before building
            if let Some(ref name) = field.name {
                child_ctx.insert(name.clone(), build_value.clone());
            }

            match field.subcon.build(&build_value, stream, &mut child_ctx) {
                Ok(()) => {}
                Err(ConstructError::StopField { .. }) => return Ok(()),
                Err(e) => {
                    return Err(if let Some(ref name) = field.name {
                        e.with_path_prefix(name)
                    } else {
                        e.with_path_prefix("(anonymous)")
                    });
                }
            }
        }

        Ok(())
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        let child_ctx = ctx.subcontext();
        let mut total: usize = 0;
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
            total = total.saturating_add(size);
        }
        Ok(total)
    }

    fn flagbuildnone(&self) -> bool {
        // FocusedSeq can only build without a value if ALL subcons (including
        // the focused one) have flagbuildnone=true
        self.subcons.iter().all(|f| f.subcon.flagbuildnone())
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
    use crate::constructs::format_field::{INT16UB, INT8UB};
    use crate::constructs::meta::Pass;
    use crate::core::error::ConstructError;
    use crate::core::Construct;

    macro_rules! as_dyn {
        ($s:expr) => {
            &$s as &dyn Construct
        };
    }

    // ======================================================================
    // Parse tests
    // ======================================================================

    #[test]
    fn parse_returns_focused_field_value() {
        let fs = FocusedSeq::new(
            "num",
            vec![
                StructField::anonymous(Box::new(Const::new_bytes(b"SIG".to_vec()).into())),
                StructField::new("num", Box::new(INT8UB.into())),
            ],
        );
        let result = as_dyn!(fs).parse_bytes(b"SIG\xFF").unwrap();
        assert_eq!(result, Value::UInt(255));
    }

    #[test]
    fn parse_single_focused_field() {
        let fs = FocusedSeq::new(
            "value",
            vec![StructField::new("value", Box::new(INT8UB.into()))],
        );
        let result = as_dyn!(fs).parse_bytes(b"\x42").unwrap();
        assert_eq!(result, Value::UInt(0x42));
    }

    #[test]
    fn parse_missing_focus_field_returns_error() {
        let fs = FocusedSeq::new(
            "nonexistent",
            vec![StructField::new("value", Box::new(INT8UB.into()))],
        );
        let err = as_dyn!(fs).parse_bytes(b"\x42").unwrap_err();
        assert!(matches!(err, ConstructError::FieldMissing { .. }));
    }

    #[test]
    fn parse_with_multiple_fields() {
        let fs = FocusedSeq::new(
            "b",
            vec![
                StructField::new("a", Box::new(INT8UB.into())),
                StructField::new("b", Box::new(INT16UB.into())),
                StructField::new("c", Box::new(INT8UB.into())),
            ],
        );
        let result = as_dyn!(fs).parse_bytes(b"\x01\x02\x03\x04").unwrap();
        // Returns only "b" field value
        assert_eq!(result, Value::UInt(0x0203));
    }

    #[test]
    fn parse_context_available_to_later_fields() {
        // "length" is parsed first, "data" can reference it via context
        let fs = FocusedSeq::new(
            "data",
            vec![
                StructField::new("length", Box::new(INT8UB.into())),
                StructField::new("data", Box::new(Bytes::new(3).into())),
            ],
        );
        let result = as_dyn!(fs).parse_bytes(b"\x03ABC").unwrap();
        assert_eq!(result, Value::Bytes(b"ABC".to_vec()));
    }

    // ======================================================================
    // Build tests
    // ======================================================================

    #[test]
    fn build_focused_field_gets_object() {
        let fs = FocusedSeq::new(
            "num",
            vec![
                StructField::anonymous(Box::new(Const::new_bytes(b"SIG".to_vec()).into())),
                StructField::new("num", Box::new(INT8UB.into())),
            ],
        );
        let built = as_dyn!(fs).build_bytes(&Value::UInt(255)).unwrap();
        assert_eq!(built, b"SIG\xFF");
    }

    #[test]
    fn build_single_field() {
        let fs = FocusedSeq::new(
            "value",
            vec![StructField::new("value", Box::new(INT8UB.into()))],
        );
        let built = as_dyn!(fs).build_bytes(&Value::UInt(0x42)).unwrap();
        assert_eq!(built, vec![0x42]);
    }

    #[test]
    fn build_non_focused_fields_get_none() {
        // Pass field gets None, focused field gets the value
        let fs = FocusedSeq::new(
            "num",
            vec![
                StructField::anonymous(Box::new(Pass::new().into())),
                StructField::new("num", Box::new(INT8UB.into())),
            ],
        );
        let built = as_dyn!(fs).build_bytes(&Value::UInt(0x42)).unwrap();
        assert_eq!(built, vec![0x42]);
    }

    // ======================================================================
    // Sizeof tests
    // ======================================================================

    #[test]
    fn sizeof_returns_sum_of_all_fields() {
        let fs = FocusedSeq::new(
            "b",
            vec![
                StructField::new("a", Box::new(INT8UB.into())),
                StructField::new("b", Box::new(INT16UB.into())),
            ],
        );
        assert_eq!(fs.sizeof(&Context::new()).unwrap(), 3);
    }

    #[test]
    fn sizeof_single_field() {
        let fs = FocusedSeq::new(
            "value",
            vec![StructField::new("value", Box::new(INT8UB.into()))],
        );
        assert_eq!(fs.sizeof(&Context::new()).unwrap(), 1);
    }

    // ======================================================================
    // flagbuildnone tests
    // ======================================================================

    #[test]
    fn flagbuildnone_false_when_focused_requires_value() {
        let fs = FocusedSeq::new(
            "value",
            vec![StructField::new("value", Box::new(INT8UB.into()))],
        );
        assert!(!fs.flagbuildnone());
    }

    #[test]
    fn flagbuildnone_true_when_all_build_none() {
        let fs = FocusedSeq::new(
            "value",
            vec![
                StructField::anonymous(Box::new(Pass::new().into())),
                StructField::new("value", Box::new(Pass::new().into())),
            ],
        );
        assert!(fs.flagbuildnone());
    }

    // ======================================================================
    // Roundtrip tests
    // ======================================================================

    #[test]
    fn roundtrip_with_const_prefix() {
        let fs = FocusedSeq::new(
            "num",
            vec![
                StructField::anonymous(Box::new(Const::new_bytes(b"SIG".to_vec()).into())),
                StructField::new("num", Box::new(INT8UB.into())),
            ],
        );

        let c: &dyn Construct = &fs;
        let built = c.build_bytes(&Value::UInt(255)).unwrap();
        assert_eq!(built, b"SIG\xFF");

        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, Value::UInt(255));
    }

    #[test]
    fn roundtrip_focused_with_const_non_focused() {
        let fs = FocusedSeq::new(
            "num",
            vec![
                StructField::anonymous(Box::new(Const::new_bytes(b"\x00".to_vec()).into())),
                StructField::new("num", Box::new(INT8UB.into())),
            ],
        );

        let c: &dyn Construct = &fs;
        let built = c.build_bytes(&Value::UInt(0x42)).unwrap();
        assert_eq!(built, vec![0x00, 0x42]);

        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, Value::UInt(0x42));
    }

    // ======================================================================
    // Error path tests
    // ======================================================================

    #[test]
    fn parse_error_enriches_path() {
        let fs = FocusedSeq::new(
            "value",
            vec![StructField::new("value", Box::new(INT16UB.into()))],
        );
        let err = as_dyn!(fs).parse_bytes(b"\x01").unwrap_err();
        match &err {
            ConstructError::Stream { path, .. } => {
                assert!(
                    path.contains("value"),
                    "expected path to contain 'value', got: {path}"
                );
            }
            other => panic!("expected Stream error, got {:?}", other),
        }
    }

    #[test]
    fn build_error_enriches_path() {
        let fs = FocusedSeq::new(
            "value",
            vec![StructField::new("value", Box::new(INT8UB.into()))],
        );
        let err = as_dyn!(fs)
            .build_bytes(&Value::String("not a number".to_string()))
            .unwrap_err();
        match &err {
            ConstructError::TypeMismatch { path, .. } => {
                assert!(
                    path.contains("value"),
                    "expected path to contain 'value', got: {path}"
                );
            }
            other => panic!("expected TypeMismatch, got {:?}", other),
        }
    }

    // ======================================================================
    // StopField tests
    // ======================================================================

    #[test]
    fn stop_field_halts_parsing() {
        use crate::core::error::Result;

        struct StopConstruct;

        impl Construct for StopConstruct {
            fn parse(&self, _stream: &mut CombinedStream, _ctx: &mut Context) -> Result<Value> {
                Err(ConstructError::StopField {
                    path: String::new(),
                })
            }
            fn build(
                &self,
                _data: &Value,
                _stream: &mut CombinedStream,
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

        let fs = FocusedSeq::new(
            "value",
            vec![
                StructField::new("value", Box::new(INT8UB.into())),
                StructField::anonymous(Box::new(crate::combined::dynamic(StopConstruct))),
                StructField::new("other", Box::new(INT8UB.into())),
            ],
        );

        // parse: stops after "value", focus field was parsed successfully
        let c: &dyn Construct = &fs;
        let result = c.parse_bytes(b"\x01\x02").unwrap();
        assert_eq!(result, Value::UInt(1));
    }
}
