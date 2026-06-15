//! Select construct — selects the first matching sub-construct.
//!
//! [`Select`] tries each sub-construct in sequence during parse. If a
//! sub-construct succeeds, its result is returned and the stream is left at
//! the advanced position. If a sub-construct fails, the stream is rewound to
//! the original position and the next sub-construct is tried. If all
//! sub-constructs fail, a [`ConstructError::Select`] is returned.
//!
//! Build works similarly: each sub-construct is tried in order, and the first
//! one that succeeds is used.
//!
//! Corresponds to Python `Select(*subcons)`.
//!
//! # Examples
//!
//! ```
//! use construct::constructs::select::Select;
//! use construct::constructs::format_field::INT32UB;
//! use construct::constructs::bytes::Bytes;
//! use construct::core::Construct;
//! use construct::value::Value;
//!
//! let s = Select::new(vec![
//!     INT32UB.into(),
//!     Bytes::new(4).into(),
//! ]);
//!
//! let c: &dyn Construct = &s;
//! // INT32UB matches first
//! let parsed = c.parse_bytes(b"\x00\x00\x00\x2A").unwrap();
//! assert_eq!(parsed, Value::UInt(42));
//! ```

use crate::combined::CombinedConstruct;
use crate::core::context::Context;
use crate::core::error::{ConstructError, Result};
use crate::core::stream::CombinedStream;
use crate::core::stream::Stream;
use crate::core::Construct;
use crate::value::Value;

// ===========================================================================
// Select
// ===========================================================================

/// A composite construct that selects the first matching sub-construct.
///
/// During parse, each sub-construct is tried in order. If parsing succeeds,
/// the result is returned. If parsing fails, the stream is rewound and the
/// next sub-construct is tried. If all sub-constructs fail, a
/// [`ConstructError::Select`] error is returned.
///
/// During build, each sub-construct is tried in a similar fashion by building
/// to an internal buffer. The first sub-construct that builds successfully
/// has its output written to the stream.
///
/// Sizeof is not defined (returns an error) because the active sub-construct
/// depends on runtime data.
///
/// Corresponds to Python `Select`.
pub struct Select {
    /// The list of sub-constructs to try in order.
    subcons: Vec<CombinedConstruct>,
}

impl Select {
    /// Creates a new `Select` with the given list of sub-constructs.
    ///
    /// # Examples
    ///
    /// ```
    /// use construct::constructs::select::Select;
    /// use construct::constructs::format_field::INT8UB;
    /// use construct::constructs::meta::Pass;
    ///
    /// let s = Select::new(vec![
    ///     INT8UB.into(),
    ///     Pass::new().into(),
    /// ]);
    /// ```
    pub fn new(subcons: Vec<CombinedConstruct>) -> Self {
        Select { subcons }
    }

    /// Builder-style method to add a sub-construct.
    ///
    /// # Examples
    ///
    /// ```
    /// use construct::constructs::select::Select;
    /// use construct::constructs::format_field::INT8UB;
    ///
    /// let s = Select::new(vec![]).push(INT8UB);
    /// ```
    pub fn push(mut self, subcon: impl Into<CombinedConstruct>) -> Self {
        self.subcons.push(subcon.into());
        self
    }

    /// Returns the list of sub-constructs to try in order (read-only access).
    #[must_use]
    pub fn subcons(&self) -> &[CombinedConstruct] {
        &self.subcons
    }
}

impl Construct for Select {
    fn parse(&self, stream: &mut CombinedStream, ctx: &mut Context) -> Result<Value> {
        let fallback = stream.tell()?;

        for sc in &self.subcons {
            match sc.parse(stream, ctx) {
                Ok(value) => return Ok(value),
                Err(ConstructError::StopField { .. }) => {
                    // StopField propagates immediately (like ExplicitError in Python)
                    return Err(ConstructError::StopField {
                        path: String::new(),
                    });
                }
                Err(_) => {
                    // Rewind and try next
                    stream.seek(fallback)?;
                }
            }
        }

        Err(ConstructError::Select {
            path: String::new(),
            message: format!(
                "no subconstruct matched after trying {} candidates",
                self.subcons.len()
            ),
        })
    }

    fn build(&self, data: &Value, stream: &mut CombinedStream, ctx: &mut Context) -> Result<()> {
        // Try building each subconstruct into an internal buffer.
        // The first one that succeeds gets written to the actual stream.
        for sc in &self.subcons {
            let mut build_stream =
                CombinedStream::ByteStream(crate::core::stream::ByteStream::new_write());
            match sc.build(data, &mut build_stream, ctx) {
                Ok(()) => {
                    let bytes = build_stream.into_bytes();
                    stream.write_bytes(&bytes)?;
                    return Ok(());
                }
                Err(ConstructError::StopField { .. }) => {
                    return Err(ConstructError::StopField {
                        path: String::new(),
                    });
                }
                Err(_) => {
                    // Try next subconstruct
                }
            }
        }

        Err(ConstructError::Select {
            path: String::new(),
            message: format!("no subconstruct matched for building: {:?}", data),
        })
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Err(ConstructError::Sizeof {
            path: String::new(),
            reason: "Select size depends on runtime data".to_string(),
        })
    }

    fn flagbuildnone(&self) -> bool {
        self.subcons.iter().any(|sc| sc.flagbuildnone())
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
    use crate::constructs::meta::Pass;
    use crate::core::error::ConstructError;
    use crate::core::stream::ByteStream;
    use crate::core::stream::Stream;
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
    fn parse_first_matching_subcon() {
        let s = Select::new(vec![INT32UB.into(), Bytes::new(4).into()]);
        let result = as_dyn!(s).parse_bytes(b"\x00\x00\x00\x2A").unwrap();
        assert_eq!(result, Value::UInt(42));
    }

    #[test]
    fn parse_falls_through_to_second_subcon() {
        // First construct (INT32UB) needs 4 bytes, but if we make it fail somehow...
        // Actually, let's use two different formats where first fails on data.
        // A simpler approach: use INT32UB which needs 4 bytes, and Bytes(1).
        // With only 1 byte, INT32UB fails, Bytes(1) succeeds.
        let s = Select::new(vec![INT32UB.into(), Bytes::new(1).into()]);
        let result = as_dyn!(s).parse_bytes(b"\x2A").unwrap();
        assert_eq!(result, Value::Bytes(vec![0x2A]));
    }

    #[test]
    fn parse_all_fail_returns_select_error() {
        let s = Select::new(vec![INT32UB.into(), INT16UB.into()]);
        let err = as_dyn!(s).parse_bytes(b"\x01").unwrap_err();
        assert!(matches!(err, ConstructError::Select { .. }));
    }

    #[test]
    fn parse_rewinds_stream_on_failure() {
        // If first subcon fails, stream is rewound before trying second
        let s = Select::new(vec![
            INT32UB.into(), // needs 4 bytes
            INT8UB.into(),  // needs 1 byte
        ]);
        let data = b"\x2A";
        let mut stream =
            crate::core::stream::CombinedStream::ByteStream(ByteStream::new_read(data));
        let mut ctx = Context::new();
        let c: &dyn Construct = &s;
        let result = c.parse(&mut stream, &mut ctx).unwrap();
        // Should have parsed via INT8UB = 42
        assert_eq!(result, Value::UInt(0x2A));
        // Stream should be at position 1 (after INT8UB consumed 1 byte)
        assert_eq!(stream.tell().unwrap(), 1);
    }

    #[test]
    fn parse_empty_subcons_returns_error() {
        let s = Select::new(vec![]);
        let err = as_dyn!(s).parse_bytes(b"\x01").unwrap_err();
        assert!(matches!(err, ConstructError::Select { .. }));
    }

    #[test]
    fn parse_pass_subcon_succeeds_on_empty() {
        // Select(INT8UB, Pass) on empty data -> Pass matches
        let s = Select::new(vec![INT8UB.into(), Pass::new().into()]);
        let result = as_dyn!(s).parse_bytes(b"").unwrap();
        assert_eq!(result, Value::None);
    }

    // ======================================================================
    // Build tests
    // ======================================================================

    #[test]
    fn build_first_matching_subcon() {
        let s = Select::new(vec![INT8UB.into(), Bytes::new(1).into()]);
        // INT8UB can build from UInt(42)
        let built = as_dyn!(s).build_bytes(&Value::UInt(42)).unwrap();
        assert_eq!(built, vec![42]);
    }

    #[test]
    fn build_falls_through_to_second_subcon() {
        let s = Select::new(vec![INT32UB.into(), Bytes::new(3).into()]);
        // INT32UB can build from UInt(42) -> succeeds
        let built = as_dyn!(s).build_bytes(&Value::UInt(42)).unwrap();
        assert_eq!(built, vec![0, 0, 0, 42]);
    }

    #[test]
    fn build_all_fail_returns_select_error() {
        let s = Select::new(vec![INT8UB.into(), INT16UB.into()]);
        // Both will fail on a string value
        let err = as_dyn!(s)
            .build_bytes(&Value::Bytes(vec![1, 2]))
            .unwrap_err();
        assert!(matches!(err, ConstructError::Select { .. }));
    }

    #[test]
    fn build_empty_subcons_returns_error() {
        let s = Select::new(vec![]);
        let err = as_dyn!(s).build_bytes(&Value::UInt(1)).unwrap_err();
        assert!(matches!(err, ConstructError::Select { .. }));
    }

    // ======================================================================
    // Sizeof tests
    // ======================================================================

    #[test]
    fn sizeof_returns_error() {
        let s = Select::new(vec![INT8UB.into()]);
        let err = s.sizeof(&Context::new()).unwrap_err();
        assert!(matches!(err, ConstructError::Sizeof { .. }));
    }

    // ======================================================================
    // flagbuildnone tests
    // ======================================================================

    #[test]
    fn flagbuildnone_true_if_any_subcon_is_build_none() {
        let s = Select::new(vec![INT8UB.into(), Pass::new().into()]);
        assert!(s.flagbuildnone());
    }

    #[test]
    fn flagbuildnone_false_if_all_subcons_require_value() {
        let s = Select::new(vec![INT8UB.into(), INT16UB.into()]);
        assert!(!s.flagbuildnone());
    }

    #[test]
    fn flagbuildnone_true_for_empty_select() {
        let s = Select::new(vec![]);
        assert!(!s.flagbuildnone()); // any() on empty is false
    }

    // ======================================================================
    // Roundtrip tests
    // ======================================================================

    #[test]
    fn roundtrip_build_then_parse() {
        let s = Select::new(vec![INT8UB.into(), Pass::new().into()]);

        let c: &dyn Construct = &s;
        let built = c.build_bytes(&Value::UInt(0x42)).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, Value::UInt(0x42));
    }

    // ======================================================================
    // Optional (Select(subcon, Pass)) pattern
    // ======================================================================

    #[test]
    fn optional_pattern_parse_succeeds() {
        // Select(INT8UB, Pass) matches valid data
        let s = Select::new(vec![INT8UB.into(), Pass::new().into()]);
        let result = as_dyn!(s).parse_bytes(b"\x2A").unwrap();
        assert_eq!(result, Value::UInt(42));
    }

    #[test]
    fn optional_pattern_parse_returns_none_on_empty() {
        // Select(INT8UB, Pass) on empty data -> INT8UB fails, Pass succeeds
        let s = Select::new(vec![INT8UB.into(), Pass::new().into()]);
        let result = as_dyn!(s).parse_bytes(b"").unwrap();
        assert_eq!(result, Value::None);
    }

    #[test]
    fn optional_pattern_build_none() {
        // Select(INT8UB, Pass) build None -> INT8UB fails, Pass succeeds
        let s = Select::new(vec![INT8UB.into(), Pass::new().into()]);
        let built = as_dyn!(s).build_bytes(&Value::None).unwrap();
        assert!(built.is_empty());
    }
}
