//! Control-flow constructs — [`IfThenElse`], [`Switch`], [`Check`], [`StopIf`].
//!
//! These constructs control which sub-constructs are executed based on
//! runtime conditions:
//!
//! - [`IfThenElse`] — selects between two sub-constructs based on a condition
//! - [`Switch`] — selects a sub-construct from a map of cases based on a key
//! - [`Check`] — asserts a condition, returning an error if it fails
//! - [`StopIf`] — signals early termination to the enclosing composite
//!
//! # Python correspondence
//!
//! | Rust | Python |
//! |------|--------|
//! | [`IfThenElse`] | `IfThenElse` (line ~3944) |
//! | [`Switch`] | `Switch` (line ~4002) |
//! | [`Check`] | `Check` (line ~3081) |
//! | [`StopIf`] | `StopIf` (line ~4079) |

use crate::constructs::Pass;
use crate::core::context::Context;
use crate::core::error::{ConstructError, Result};
use crate::core::stream::Stream;
use crate::core::Construct;
use crate::value::Value;

// ===========================================================================
// Type aliases for closure-based functions
// ===========================================================================

/// Function signature for the condition used by [`IfThenElse`].
///
/// Receives the current [`Context`] and returns a `bool` indicating which
/// branch to take.
pub type CondFunc = Box<dyn Fn(&Context) -> bool>;

/// Function signature for the key function used by [`Switch`].
///
/// Receives the current [`Context`] and returns a [`Value`] that is looked
/// up in the switch case map.
pub type KeyFunc = Box<dyn Fn(&Context) -> Result<Value>>;

/// Function signature for the check function used by [`Check`].
///
/// Receives the current [`Context`] and returns `Ok(())` if the check passes,
/// or an `Err` describing the failure.
pub type CheckFunc = Box<dyn Fn(&Context) -> Result<()>>;

/// Function signature for the condition used by [`StopIf`].
///
/// Receives the current [`Context`] and returns a `bool`. If `true`, the
/// enclosing construct should stop processing further fields.
pub type StopCondFunc = Box<dyn Fn(&Context) -> bool>;

// ===========================================================================
// IfThenElse
// ===========================================================================

/// A conditional construct that selects between two sub-constructs.
///
/// During parsing, building, and sizeof, the `cond` function is evaluated.
/// If it returns `true`, `then_constr` is used; otherwise `else_constr` is
/// used.
///
/// Corresponds to the Python `IfThenElse` class
/// (`construct/construct/core.py` line ~3944).
///
/// # Examples
///
/// ```
/// use construct::constructs::control_flow::IfThenElse;
/// use construct::constructs::format_field::{INT8UB, INT16UB};
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let ite = IfThenElse::new(
///     Box::new(|ctx| {
///         ctx.get("use_short")
///             .map(|v| v.as_bool().unwrap_or(false))
///             .unwrap_or(false)
///     }),
///     Box::new(INT16UB),
///     Box::new(INT8UB),
/// );
///
/// let c: &dyn Construct = &ite;
///
/// // Build with use_short=true → uses INT16UB
/// let mut stream = construct::core::stream::ByteStream::new_write();
/// let mut ctx = construct::core::context::Context::new();
/// ctx.insert("use_short", Value::Bool(true));
/// c.build(&Value::UInt(5), &mut stream, &mut ctx).unwrap();
/// assert_eq!(stream.into_bytes(), vec![0x00, 0x05]);
/// ```
pub struct IfThenElse {
    /// The condition function. Returns `true` to use `then_constr`,
    /// `false` to use `else_constr`.
    pub cond: CondFunc,
    /// The sub-construct used when the condition is `true`.
    pub then_constr: Box<dyn Construct>,
    /// The sub-construct used when the condition is `false`.
    pub else_constr: Box<dyn Construct>,
}

impl IfThenElse {
    /// Creates a new `IfThenElse` construct.
    ///
    /// # Parameters
    ///
    /// - `cond` — function that evaluates the condition from the context
    /// - `then_constr` — sub-construct used when the condition is `true`
    /// - `else_constr` — sub-construct used when the condition is `false`
    pub fn new(
        cond: CondFunc,
        then_constr: Box<dyn Construct>,
        else_constr: Box<dyn Construct>,
    ) -> Self {
        IfThenElse {
            cond,
            then_constr,
            else_constr,
        }
    }
}

impl Construct for IfThenElse {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        if (self.cond)(ctx) {
            self.then_constr.parse(stream, ctx)
        } else {
            self.else_constr.parse(stream, ctx)
        }
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        if (self.cond)(ctx) {
            self.then_constr.build(data, stream, ctx)
        } else {
            self.else_constr.build(data, stream, ctx)
        }
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        if (self.cond)(ctx) {
            self.then_constr.sizeof(ctx)
        } else {
            self.else_constr.sizeof(ctx)
        }
    }

    fn flagbuildnone(&self) -> bool {
        self.then_constr.flagbuildnone() && self.else_constr.flagbuildnone()
    }
}

// ===========================================================================
// Switch
// ===========================================================================

/// A conditional branch construct that selects a sub-construct from a list of
/// cases based on a key function.
///
/// During parsing, building, and sizeof, the `keyfunc` is evaluated to produce
/// a key [`Value`]. This key is compared against the case keys using
/// [`PartialEq`]. If a match is found, the corresponding sub-construct is
/// used. If no case matches, `default` is used (which defaults to
/// [`Pass`](crate::constructs::Pass)).
///
/// Cases are stored as an ordered `Vec` of (key, construct) pairs. The first
/// matching case wins. This differs from the Python version which uses a
/// `dict`, but is necessary because [`Value`] cannot implement [`Hash`] + [`Eq`]
/// (the `Float` variant contains `f64`, which does not satisfy `Eq`).
///
/// Corresponds to the Python `Switch` class
/// (`construct/construct/core.py` line ~4002).
///
/// # Examples
///
/// ```
/// use construct::constructs::control_flow::Switch;
/// use construct::constructs::format_field::{INT8UB, INT16UB, INT32UB};
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let sw = Switch::new(
///     Box::new(|ctx| {
///         ctx.get("n").cloned().ok_or_else(|| {
///             construct::core::error::ConstructError::FieldMissing {
///                 path: String::new(),
///                 field: "n".to_string(),
///             }
///         })
///     }),
///     vec![
///         (Value::UInt(1), Box::new(INT8UB) as Box<dyn Construct>),
///         (Value::UInt(2), Box::new(INT16UB) as Box<dyn Construct>),
///         (Value::UInt(4), Box::new(INT32UB) as Box<dyn Construct>),
///     ],
///     None,
/// );
///
/// let c: &dyn Construct = &sw;
///
/// // Build with n=1 → INT8UB
/// let mut stream = construct::core::stream::ByteStream::new_write();
/// let mut ctx = construct::core::context::Context::new();
/// ctx.insert("n", Value::UInt(1));
/// c.build(&Value::UInt(5), &mut stream, &mut ctx).unwrap();
/// assert_eq!(stream.into_bytes(), vec![0x05]);
///
/// // Build with n=4 → INT32UB
/// let mut stream2 = construct::core::stream::ByteStream::new_write();
/// let mut ctx2 = construct::core::context::Context::new();
/// ctx2.insert("n", Value::UInt(4));
/// c.build(&Value::UInt(5), &mut stream2, &mut ctx2).unwrap();
/// assert_eq!(stream2.into_bytes(), vec![0x00, 0x00, 0x00, 0x05]);
/// ```
pub struct Switch {
    /// The key function. Evaluated against the context to produce the key
    /// used for case lookup.
    pub keyfunc: KeyFunc,
    /// Ordered list of (key, sub-construct) pairs. The first matching key
    /// (by [`PartialEq`]) wins.
    pub cases: Vec<(Value, Box<dyn Construct>)>,
    /// Optional default sub-construct used when no case matches.
    /// If `None`, [`Pass`](crate::constructs::Pass) is used as default.
    pub default: Option<Box<dyn Construct>>,
    /// Internal Pass instance used as the implicit default when `default`
    /// is `None`.
    pass_default: Pass,
}

impl Switch {
    /// Creates a new `Switch` construct.
    ///
    /// # Parameters
    ///
    /// - `keyfunc` — function that evaluates the key from the context
    /// - `cases` — ordered list of (key value, sub-construct) pairs; the
    ///   first matching key wins
    /// - `default` — optional default sub-construct; if `None`,
    ///   [`Pass`](crate::constructs::Pass) is used
    pub fn new(
        keyfunc: KeyFunc,
        cases: Vec<(Value, Box<dyn Construct>)>,
        default: Option<Box<dyn Construct>>,
    ) -> Self {
        Switch {
            keyfunc,
            cases,
            default,
            pass_default: Pass::new(),
        }
    }

    /// Resolves the sub-construct for the given key.
    ///
    /// Searches `cases` linearly for the first key that matches. If not
    /// found, returns `default` or the internal [`Pass`] instance.
    fn resolve(&self, key: &Value) -> &dyn Construct {
        match self.cases.iter().find(|(k, _)| k == key) {
            Some((_, sc)) => sc.as_ref(),
            None => match &self.default {
                Some(sc) => sc.as_ref(),
                None => &self.pass_default,
            },
        }
    }
}

impl Construct for Switch {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let key = (self.keyfunc)(ctx)?;
        let sc = self.resolve(&key);
        sc.parse(stream, ctx)
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        let key = (self.keyfunc)(ctx)?;
        let sc = self.resolve(&key);
        sc.build(data, stream, ctx)
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        let key = (self.keyfunc)(ctx)?;
        let sc = self.resolve(&key);
        sc.sizeof(ctx)
    }

    fn flagbuildnone(&self) -> bool {
        let all_flag = self.cases.iter().all(|(_, sc)| sc.flagbuildnone());
        let default_flag = match &self.default {
            Some(sc) => sc.flagbuildnone(),
            None => true, // Pass has flagbuildnone = true
        };
        all_flag && default_flag
    }
}

// ===========================================================================
// Check
// ===========================================================================

/// A construct that asserts a condition on the context during both parse
/// and build.
///
/// The condition is a closure that receives the current [`Context`] and
/// returns `Ok(())` if the check passes, or an `Err` if it fails.
/// `Check` does not read from or write to the stream; it only validates
/// the context state.
///
/// - **parse**: calls `check(ctx)` and returns [`Value::None`] on success
/// - **build**: calls `check(ctx)` and does nothing on success
/// - **sizeof**: returns `0`
///
/// Corresponds to the Python `Check` class
/// (`construct/construct/core.py` line ~3081).
///
/// # Examples
///
/// ```
/// use construct::constructs::control_flow::Check;
/// use construct::core::Construct;
/// use construct::core::error::ConstructError;
/// use construct::value::Value;
///
/// let chk = Check::new(Box::new(|ctx| {
///     let value = ctx.get("x").cloned().unwrap_or(Value::None);
///     if value == Value::UInt(42) {
///         Ok(())
///     } else {
///         Err(ConstructError::Check {
///             path: String::new(),
///             message: "x must be 42".to_string(),
///         })
///     }
/// }));
///
/// let c: &dyn Construct = &chk;
///
/// // Pass: x == 42
/// let mut ctx = construct::core::context::Context::new();
/// ctx.insert("x", Value::UInt(42));
/// let mut stream = construct::core::stream::ByteStream::new_read(b"");
/// let parsed = c.parse(&mut stream, &mut ctx).unwrap();
/// assert_eq!(parsed, Value::None);
///
/// // Fail: x != 42
/// let mut ctx2 = construct::core::context::Context::new();
/// ctx2.insert("x", Value::UInt(99));
/// let mut stream2 = construct::core::stream::ByteStream::new_read(b"");
/// let err = c.parse(&mut stream2, &mut ctx2).unwrap_err();
/// assert!(matches!(err, ConstructError::Check { .. }));
/// ```
pub struct Check {
    /// The check function. Returns `Ok(())` if the condition passes.
    pub check: CheckFunc,
}

impl Check {
    /// Creates a new `Check` construct with the given check function.
    ///
    /// # Parameters
    ///
    /// - `check` — function that validates the context; returns `Ok(())` on
    ///   success or `Err(ConstructError::Check)` on failure
    pub fn new(check: CheckFunc) -> Self {
        Check { check }
    }
}

impl Construct for Check {
    fn parse(&self, _stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        (self.check)(ctx)?;
        Ok(Value::None)
    }

    fn build(&self, _data: &Value, _stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        (self.check)(ctx)?;
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Ok(0)
    }

    fn flagbuildnone(&self) -> bool {
        true
    }
}

// ===========================================================================
// StopIf
// ===========================================================================

/// Reason returned when `sizeof` is called on a `StopIf` construct.
const STOPIF_SIZEOF_REASON: &str =
    "StopIf cannot determine size because it depends on actual context";

/// A construct that signals early termination to the enclosing composite.
///
/// When the condition evaluates to `true`, `StopIf` raises a
/// [`ConstructError::StopField`] error. This error is caught by enclosing
/// composite constructs such as `Struct`, `Sequence`, and `GreedyRange`,
/// which stop processing further fields/elements.
///
/// - **parse**: if condition is `true`, raises [`ConstructError::StopField`];
///   otherwise returns [`Value::None`]
/// - **build**: if condition is `true`, raises [`ConstructError::StopField`];
///   otherwise does nothing
/// - **sizeof**: returns [`ConstructError::Sizeof`] (size depends on runtime
///   context)
///
/// Corresponds to the Python `StopIf` class
/// (`construct/construct/core.py` line ~4079).
///
/// # Examples
///
/// ```
/// use construct::constructs::control_flow::StopIf;
/// use construct::core::Construct;
/// use construct::core::error::ConstructError;
/// use construct::value::Value;
///
/// let stop = StopIf::new(Box::new(|ctx| {
///     ctx.get("should_stop")
///         .map(|v| v.as_bool().unwrap_or(false))
///         .unwrap_or(false)
/// }));
///
/// let c: &dyn Construct = &stop;
///
/// // Condition true → StopField error
/// let mut ctx = construct::core::context::Context::new();
/// ctx.insert("should_stop", Value::Bool(true));
/// let mut stream = construct::core::stream::ByteStream::new_read(b"");
/// let err = c.parse(&mut stream, &mut ctx).unwrap_err();
/// assert!(matches!(err, ConstructError::StopField { .. }));
///
/// // Condition false → returns None
/// let mut ctx2 = construct::core::context::Context::new();
/// ctx2.insert("should_stop", Value::Bool(false));
/// let mut stream2 = construct::core::stream::ByteStream::new_read(b"");
/// let parsed = c.parse(&mut stream2, &mut ctx2).unwrap();
/// assert_eq!(parsed, Value::None);
/// ```
pub struct StopIf {
    /// The condition function. If it returns `true`, the enclosing construct
    /// should stop processing.
    pub cond: StopCondFunc,
}

impl StopIf {
    /// Creates a new `StopIf` construct with the given condition function.
    ///
    /// # Parameters
    ///
    /// - `cond` — function that evaluates the stop condition from the context
    pub fn new(cond: StopCondFunc) -> Self {
        StopIf { cond }
    }
}

impl Construct for StopIf {
    fn parse(&self, _stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        if (self.cond)(ctx) {
            Err(ConstructError::StopField {
                path: String::new(),
            })
        } else {
            Ok(Value::None)
        }
    }

    fn build(&self, _data: &Value, _stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        if (self.cond)(ctx) {
            Err(ConstructError::StopField {
                path: String::new(),
            })
        } else {
            Ok(())
        }
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Err(ConstructError::Sizeof {
            path: String::new(),
            reason: STOPIF_SIZEOF_REASON.to_string(),
        })
    }

    fn flagbuildnone(&self) -> bool {
        true
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constructs::format_field::{Endianness, FormatField, FormatKind};
    use crate::constructs::meta::Pass;
    use crate::core::stream::ByteStream;

    /// Helper: creates a U8 big-endian construct for tests.
    fn u8be() -> Box<dyn Construct> {
        Box::new(FormatField::new(Endianness::Big, FormatKind::U8))
    }

    /// Helper: creates a U16 big-endian construct for tests.
    fn u16be() -> Box<dyn Construct> {
        Box::new(FormatField::new(Endianness::Big, FormatKind::U16))
    }

    /// Helper: creates a U32 big-endian construct for tests.
    fn u32be() -> Box<dyn Construct> {
        Box::new(FormatField::new(Endianness::Big, FormatKind::U32))
    }

    // ======================================================================
    // IfThenElse tests
    // ======================================================================

    #[test]
    fn ifthenelse_parse_true_branch() {
        let ite = IfThenElse::new(Box::new(|_ctx| true), u16be(), u8be());

        let c: &dyn Construct = &ite;
        let parsed = c.parse_bytes(b"\x00\x05").unwrap();
        assert_eq!(parsed, Value::UInt(5));
    }

    #[test]
    fn ifthenelse_parse_false_branch() {
        let ite = IfThenElse::new(Box::new(|_ctx| false), u16be(), u8be());

        let c: &dyn Construct = &ite;
        let parsed = c.parse_bytes(b"\x05").unwrap();
        assert_eq!(parsed, Value::UInt(5));
    }

    #[test]
    fn ifthenelse_build_true_branch() {
        let ite = IfThenElse::new(Box::new(|_ctx| true), u16be(), u8be());

        let c: &dyn Construct = &ite;
        let bytes = c.build_bytes(&Value::UInt(5)).unwrap();
        assert_eq!(bytes, vec![0x00, 0x05]);
    }

    #[test]
    fn ifthenelse_build_false_branch() {
        let ite = IfThenElse::new(Box::new(|_ctx| false), u16be(), u8be());

        let c: &dyn Construct = &ite;
        let bytes = c.build_bytes(&Value::UInt(5)).unwrap();
        assert_eq!(bytes, vec![5]);
    }

    #[test]
    fn ifthenelse_roundtrip_true() {
        let ite = IfThenElse::new(Box::new(|_ctx| true), u16be(), u8be());

        let c: &dyn Construct = &ite;
        let original = Value::UInt(1000);
        let bytes = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn ifthenelse_roundtrip_false() {
        let ite = IfThenElse::new(Box::new(|_ctx| false), u16be(), u8be());

        let c: &dyn Construct = &ite;
        let original = Value::UInt(42);
        let bytes = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn ifthenelse_sizeof_true_branch() {
        let ite = IfThenElse::new(Box::new(|_ctx| true), u16be(), u8be());
        let ctx = Context::new();
        assert_eq!(ite.sizeof(&ctx).unwrap(), 2);
    }

    #[test]
    fn ifthenelse_sizeof_false_branch() {
        let ite = IfThenElse::new(Box::new(|_ctx| false), u16be(), u8be());
        let ctx = Context::new();
        assert_eq!(ite.sizeof(&ctx).unwrap(), 1);
    }

    #[test]
    fn ifthenelse_uses_context() {
        let ite = IfThenElse::new(
            Box::new(|ctx| {
                ctx.get("use_short")
                    .map(|v| v.as_bool().unwrap_or(false))
                    .unwrap_or(false)
            }),
            u16be(),
            u8be(),
        );

        // use_short = true → INT16UB
        let mut stream = ByteStream::new_read(b"\x00\x05\xAA");
        let mut ctx = Context::new();
        ctx.insert("use_short", Value::Bool(true));
        let parsed = ite.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(parsed, Value::UInt(5));

        // use_short = false → INT8UB
        let mut stream2 = ByteStream::new_read(b"\x07");
        let mut ctx2 = Context::new();
        ctx2.insert("use_short", Value::Bool(false));
        let parsed2 = ite.parse(&mut stream2, &mut ctx2).unwrap();
        assert_eq!(parsed2, Value::UInt(7));
    }

    #[test]
    fn ifthenelse_flagbuildnone_both_pass() {
        let ite = IfThenElse::new(
            Box::new(|_ctx| true),
            Box::new(Pass::new()),
            Box::new(Pass::new()),
        );
        assert!(ite.flagbuildnone());
    }

    #[test]
    fn ifthenelse_flagbuildnone_one_fails() {
        let ite = IfThenElse::new(Box::new(|_ctx| true), Box::new(Pass::new()), u8be());
        assert!(!ite.flagbuildnone());
    }

    #[test]
    fn ifthenelse_parse_error_propagates() {
        let ite = IfThenElse::new(Box::new(|_ctx| true), u16be(), u8be());

        let c: &dyn Construct = &ite;
        let err = c.parse_bytes(b"\x05").unwrap_err();
        assert!(matches!(err, ConstructError::Stream { .. }));
    }

    // ======================================================================
    // Switch tests
    // ======================================================================

    #[test]
    fn switch_parse_matching_case() {
        let sw = Switch::new(
            Box::new(|ctx| {
                ctx.get("n")
                    .cloned()
                    .ok_or_else(|| ConstructError::FieldMissing {
                        path: String::new(),
                        field: "n".to_string(),
                    })
            }),
            vec![(Value::UInt(1), u8be()), (Value::UInt(2), u16be())],
            None,
        );

        // n=1 → parse as U8
        let mut stream = ByteStream::new_read(b"\x05\xAA");
        let mut ctx = Context::new();
        ctx.insert("n", Value::UInt(1));
        let parsed = sw.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(parsed, Value::UInt(5));

        // n=2 → parse as U16
        let mut stream2 = ByteStream::new_read(b"\x00\x0A");
        let mut ctx2 = Context::new();
        ctx2.insert("n", Value::UInt(2));
        let parsed2 = sw.parse(&mut stream2, &mut ctx2).unwrap();
        assert_eq!(parsed2, Value::UInt(10));
    }

    #[test]
    fn switch_parse_default_pass() {
        let sw = Switch::new(
            Box::new(|ctx| {
                ctx.get("n")
                    .cloned()
                    .ok_or_else(|| ConstructError::FieldMissing {
                        path: String::new(),
                        field: "n".to_string(),
                    })
            }),
            vec![(Value::UInt(1), u8be())],
            None,
        );

        // n=99 → no case matches → default is Pass → returns None
        let mut stream = ByteStream::new_read(b"\x05");
        let mut ctx = Context::new();
        ctx.insert("n", Value::UInt(99));
        let parsed = sw.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(parsed, Value::None);
        // Pass does not consume bytes
        assert_eq!(stream.tell().unwrap(), 0);
    }

    #[test]
    fn switch_build_matching_case() {
        let sw = Switch::new(
            Box::new(|ctx| {
                ctx.get("n")
                    .cloned()
                    .ok_or_else(|| ConstructError::FieldMissing {
                        path: String::new(),
                        field: "n".to_string(),
                    })
            }),
            vec![(Value::UInt(1), u8be()), (Value::UInt(2), u16be())],
            None,
        );

        // n=1 → build as U8
        let mut stream = ByteStream::new_write();
        let mut ctx = Context::new();
        ctx.insert("n", Value::UInt(1));
        sw.build(&Value::UInt(5), &mut stream, &mut ctx).unwrap();
        assert_eq!(stream.into_bytes(), vec![5]);

        // n=2 → build as U16
        let mut stream2 = ByteStream::new_write();
        let mut ctx2 = Context::new();
        ctx2.insert("n", Value::UInt(2));
        sw.build(&Value::UInt(5), &mut stream2, &mut ctx2).unwrap();
        assert_eq!(stream2.into_bytes(), vec![0x00, 0x05]);
    }

    #[test]
    fn switch_roundtrip() {
        let sw = Switch::new(
            Box::new(|ctx| {
                ctx.get("n")
                    .cloned()
                    .ok_or_else(|| ConstructError::FieldMissing {
                        path: String::new(),
                        field: "n".to_string(),
                    })
            }),
            vec![(Value::UInt(4), u32be())],
            None,
        );

        let c: &dyn Construct = &sw;

        // n=4 → INT32UB
        let original = Value::UInt(0xDEADBEEF);
        let mut stream = ByteStream::new_write();
        let mut ctx = Context::new();
        ctx.insert("n", Value::UInt(4));
        c.build(&original, &mut stream, &mut ctx).unwrap();
        let bytes = stream.into_bytes();

        let mut parse_stream = ByteStream::new_read(&bytes);
        let mut parse_ctx = Context::new();
        parse_ctx.insert("n", Value::UInt(4));
        let parsed = sw.parse(&mut parse_stream, &mut parse_ctx).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn switch_sizeof_matching_case() {
        let sw = Switch::new(
            Box::new(|ctx| {
                ctx.get("n")
                    .cloned()
                    .ok_or_else(|| ConstructError::FieldMissing {
                        path: String::new(),
                        field: "n".to_string(),
                    })
            }),
            vec![
                (Value::UInt(1), u8be()),
                (Value::UInt(2), u16be()),
                (Value::UInt(4), u32be()),
            ],
            None,
        );

        let mut ctx1 = Context::new();
        ctx1.insert("n", Value::UInt(1));
        assert_eq!(sw.sizeof(&ctx1).unwrap(), 1);

        let mut ctx2 = Context::new();
        ctx2.insert("n", Value::UInt(2));
        assert_eq!(sw.sizeof(&ctx2).unwrap(), 2);

        let mut ctx4 = Context::new();
        ctx4.insert("n", Value::UInt(4));
        assert_eq!(sw.sizeof(&ctx4).unwrap(), 4);
    }

    #[test]
    fn switch_sizeof_default_pass() {
        let sw = Switch::new(
            Box::new(|ctx| {
                ctx.get("n")
                    .cloned()
                    .ok_or_else(|| ConstructError::FieldMissing {
                        path: String::new(),
                        field: "n".to_string(),
                    })
            }),
            vec![(Value::UInt(1), u8be())],
            None,
        );

        // No matching case → default Pass → sizeof = 0
        let mut ctx = Context::new();
        ctx.insert("n", Value::UInt(99));
        assert_eq!(sw.sizeof(&ctx).unwrap(), 0);
    }

    #[test]
    fn switch_with_explicit_default() {
        let sw = Switch::new(
            Box::new(|ctx| {
                ctx.get("n")
                    .cloned()
                    .ok_or_else(|| ConstructError::FieldMissing {
                        path: String::new(),
                        field: "n".to_string(),
                    })
            }),
            vec![(Value::UInt(1), u8be())],
            Some(u16be()),
        );

        // n=99 → no case → default = u16be
        let c: &dyn Construct = &sw;
        let mut stream = ByteStream::new_write();
        let mut ctx = Context::new();
        ctx.insert("n", Value::UInt(99));
        c.build(&Value::UInt(5), &mut stream, &mut ctx).unwrap();
        assert_eq!(stream.into_bytes(), vec![0x00, 0x05]);

        let mut ctx2 = Context::new();
        ctx2.insert("n", Value::UInt(99));
        assert_eq!(sw.sizeof(&ctx2).unwrap(), 2);
    }

    #[test]
    fn switch_keyfunc_error_propagates() {
        let sw = Switch::new(
            Box::new(|_ctx| {
                Err(ConstructError::Generic {
                    path: String::new(),
                    message: "keyfunc error".to_string(),
                })
            }),
            vec![],
            None,
        );

        let mut stream = ByteStream::new_read(b"\x05");
        let mut ctx = Context::new();
        let err = sw.parse(&mut stream, &mut ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    #[test]
    fn switch_flagbuildnone() {
        let sw = Switch::new(
            Box::new(|_ctx| Ok(Value::UInt(1))),
            vec![(Value::UInt(1), Box::new(Pass::new()) as Box<dyn Construct>)],
            Some(Box::new(Pass::new())),
        );
        assert!(sw.flagbuildnone());
    }

    #[test]
    fn switch_flagbuildnone_false_when_case_needs_value() {
        let sw = Switch::new(
            Box::new(|_ctx| Ok(Value::UInt(1))),
            vec![(Value::UInt(1), u8be())],
            None,
        );
        assert!(!sw.flagbuildnone());
    }

    // ======================================================================
    // Check tests
    // ======================================================================

    #[test]
    fn check_parse_passes() {
        let chk = Check::new(Box::new(|_ctx| Ok(())));

        let c: &dyn Construct = &chk;
        let mut stream = ByteStream::new_read(b"");
        let mut ctx = Context::new();
        let parsed = c.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(parsed, Value::None);
    }

    #[test]
    fn check_parse_fails() {
        let chk = Check::new(Box::new(|_ctx| {
            Err(ConstructError::Check {
                path: String::new(),
                message: "check failed during parsing".to_string(),
            })
        }));

        let c: &dyn Construct = &chk;
        let mut stream = ByteStream::new_read(b"");
        let mut ctx = Context::new();
        let err = c.parse(&mut stream, &mut ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Check { .. }));
    }

    #[test]
    fn check_build_passes() {
        let chk = Check::new(Box::new(|_ctx| Ok(())));

        let c: &dyn Construct = &chk;
        let mut stream = ByteStream::new_write();
        let mut ctx = Context::new();
        c.build(&Value::None, &mut stream, &mut ctx).unwrap();
        assert!(stream.into_bytes().is_empty());
    }

    #[test]
    fn check_build_fails() {
        let chk = Check::new(Box::new(|_ctx| {
            Err(ConstructError::Check {
                path: String::new(),
                message: "check failed during building".to_string(),
            })
        }));

        let c: &dyn Construct = &chk;
        let mut stream = ByteStream::new_write();
        let mut ctx = Context::new();
        let err = c.build(&Value::None, &mut stream, &mut ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Check { .. }));
    }

    #[test]
    fn check_sizeof_returns_zero() {
        let chk = Check::new(Box::new(|_ctx| Ok(())));
        let ctx = Context::new();
        assert_eq!(chk.sizeof(&ctx).unwrap(), 0);
    }

    #[test]
    fn check_uses_context() {
        let chk = Check::new(Box::new(|ctx| {
            let value = ctx.get("x").cloned().unwrap_or(Value::None);
            if value == Value::UInt(42) {
                Ok(())
            } else {
                Err(ConstructError::Check {
                    path: String::new(),
                    message: "x must be 42".to_string(),
                })
            }
        }));

        // Pass: x == 42
        let mut stream = ByteStream::new_read(b"");
        let mut ctx = Context::new();
        ctx.insert("x", Value::UInt(42));
        let parsed = chk.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(parsed, Value::None);

        // Fail: x != 42
        let mut stream2 = ByteStream::new_read(b"");
        let mut ctx2 = Context::new();
        ctx2.insert("x", Value::UInt(99));
        let err = chk.parse(&mut stream2, &mut ctx2).unwrap_err();
        assert!(matches!(err, ConstructError::Check { .. }));
    }

    #[test]
    fn check_flagbuildnone() {
        let chk = Check::new(Box::new(|_ctx| Ok(())));
        assert!(chk.flagbuildnone());
    }

    #[test]
    fn check_does_not_consume_stream() {
        let chk = Check::new(Box::new(|_ctx| Ok(())));
        let mut stream = ByteStream::new_read(b"\x01\x02\x03");
        let mut ctx = Context::new();
        chk.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(stream.tell().unwrap(), 0);
    }

    // ======================================================================
    // StopIf tests
    // ======================================================================

    #[test]
    fn stopif_parse_returns_none_when_false() {
        let stop = StopIf::new(Box::new(|_ctx| false));

        let mut stream = ByteStream::new_read(b"");
        let mut ctx = Context::new();
        let parsed = stop.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(parsed, Value::None);
    }

    #[test]
    fn stopif_parse_raises_stop_field_when_true() {
        let stop = StopIf::new(Box::new(|_ctx| true));

        let mut stream = ByteStream::new_read(b"");
        let mut ctx = Context::new();
        let err = stop.parse(&mut stream, &mut ctx).unwrap_err();
        assert!(matches!(err, ConstructError::StopField { .. }));
    }

    #[test]
    fn stopif_build_does_nothing_when_false() {
        let stop = StopIf::new(Box::new(|_ctx| false));

        let mut stream = ByteStream::new_write();
        let mut ctx = Context::new();
        stop.build(&Value::None, &mut stream, &mut ctx).unwrap();
        assert!(stream.into_bytes().is_empty());
    }

    #[test]
    fn stopif_build_raises_stop_field_when_true() {
        let stop = StopIf::new(Box::new(|_ctx| true));

        let mut stream = ByteStream::new_write();
        let mut ctx = Context::new();
        let err = stop.build(&Value::None, &mut stream, &mut ctx).unwrap_err();
        assert!(matches!(err, ConstructError::StopField { .. }));
    }

    #[test]
    fn stopif_sizeof_returns_error() {
        let stop = StopIf::new(Box::new(|_ctx| false));
        let ctx = Context::new();
        let err = stop.sizeof(&ctx).unwrap_err();
        assert!(matches!(err, ConstructError::Sizeof { .. }));
    }

    #[test]
    fn stopif_uses_context() {
        let stop = StopIf::new(Box::new(|ctx| {
            ctx.get("should_stop")
                .map(|v| v.as_bool().unwrap_or(false))
                .unwrap_or(false)
        }));

        // should_stop = true → StopField
        let mut stream = ByteStream::new_read(b"");
        let mut ctx = Context::new();
        ctx.insert("should_stop", Value::Bool(true));
        let err = stop.parse(&mut stream, &mut ctx).unwrap_err();
        assert!(matches!(err, ConstructError::StopField { .. }));

        // should_stop = false → None
        let mut stream2 = ByteStream::new_read(b"");
        let mut ctx2 = Context::new();
        ctx2.insert("should_stop", Value::Bool(false));
        let parsed = stop.parse(&mut stream2, &mut ctx2).unwrap();
        assert_eq!(parsed, Value::None);
    }

    #[test]
    fn stopif_flagbuildnone() {
        let stop = StopIf::new(Box::new(|_ctx| false));
        assert!(stop.flagbuildnone());
    }

    #[test]
    fn stopif_does_not_consume_stream() {
        let stop = StopIf::new(Box::new(|_ctx| false));
        let mut stream = ByteStream::new_read(b"\x01\x02\x03");
        let mut ctx = Context::new();
        stop.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(stream.tell().unwrap(), 0);
    }

    // ======================================================================
    // Cross-cutting integration tests
    // ======================================================================

    #[test]
    fn ifthenelse_with_switch_combined() {
        // Use IfThenElse to decide whether to use a Switch
        let inner_switch = Switch::new(
            Box::new(|ctx| {
                ctx.get("type")
                    .cloned()
                    .ok_or_else(|| ConstructError::FieldMissing {
                        path: String::new(),
                        field: "type".to_string(),
                    })
            }),
            vec![(Value::UInt(1), u8be()), (Value::UInt(2), u16be())],
            None,
        );

        let ite = IfThenElse::new(
            Box::new(|ctx| {
                ctx.get("enabled")
                    .map(|v| v.as_bool().unwrap_or(false))
                    .unwrap_or(false)
            }),
            Box::new(inner_switch),
            Box::new(Pass::new()),
        );

        // enabled=true, type=2 → parse U16
        let mut stream = ByteStream::new_read(b"\x00\x0A");
        let mut ctx = Context::new();
        ctx.insert("enabled", Value::Bool(true));
        ctx.insert("type", Value::UInt(2));
        let parsed = ite.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(parsed, Value::UInt(10));

        // enabled=false → Pass
        let mut stream2 = ByteStream::new_read(b"\x00\x0A");
        let mut ctx2 = Context::new();
        ctx2.insert("enabled", Value::Bool(false));
        ctx2.insert("type", Value::UInt(2));
        let parsed2 = ite.parse(&mut stream2, &mut ctx2).unwrap();
        assert_eq!(parsed2, Value::None);
    }

    #[test]
    fn check_combined_with_ifthenelse() {
        // IfThenElse where the true branch includes a condition check
        let ite = IfThenElse::new(
            Box::new(|ctx| {
                ctx.get("validate")
                    .map(|v| v.as_bool().unwrap_or(false))
                    .unwrap_or(false)
            }),
            u8be(),
            u8be(),
        );

        // Parse with validate=false → no special behavior
        let c: &dyn Construct = &ite;
        let mut stream = ByteStream::new_read(b"\x05");
        let mut ctx = Context::new();
        ctx.insert("validate", Value::Bool(false));
        let parsed = c.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(parsed, Value::UInt(5));
    }
}
