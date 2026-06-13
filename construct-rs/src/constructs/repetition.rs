//! Repetition constructs — Array, GreedyRange, and RepeatUntil.
//!
//! These constructs parse/build homogenous sequences of elements into a
//! [`Value::List`]. They differ in how the element count is determined:
//!
//! | Construct | Count strategy |
//! |-----------|---------------|
//! | [`Array`] | Fixed count, known in advance |
//! | [`GreedyRange`] | Parse until end-of-stream or sub-construct failure |
//! | [`RepeatUntil`] | Parse until a predicate returns `true` |
//!
//! Corresponds to Python `Array`, `GreedyRange`, and `RepeatUntil`.

use crate::core::context::Context;
use crate::core::error::{ConstructError, Result};
use crate::core::stream::Stream;
use crate::core::Construct;
use crate::expr::Evaluate;
use crate::value::Value;

// ===========================================================================
// Context key for the current loop index
// ===========================================================================

/// The context key used to store the current iteration index during
/// repetition constructs' parse/build loops.
///
/// Corresponds to the Python `context._index` field.
const CONTEXT_INDEX_KEY: &str = "_index";

// ===========================================================================
// Array
// ===========================================================================

/// A fixed-count homogenous array of elements.
///
/// Parses exactly `count` elements and returns them as a [`Value::List`].
/// Builds from a list and verifies that the list length equals `count`.
/// Size is `count × subcon.sizeof()`.
///
/// During iteration, the current index is stored in the context as `_index`
/// (accessible via [`CONTEXT_INDEX_KEY`]).
///
/// Corresponds to Python `Array(count, subcon, discard=False)`.
///
/// # Examples
///
/// ```
/// use construct::constructs::repetition::Array;
/// use construct::constructs::format_field::INT8UB;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let arr = Array::new(3, Box::new(INT8UB));
/// let c: &dyn Construct = &arr;
///
/// // Parse
/// let parsed = c.parse_bytes(b"\x01\x02\x03").unwrap();
/// assert_eq!(parsed, Value::List(vec![
///     Value::UInt(1), Value::UInt(2), Value::UInt(3),
/// ]));
///
/// // Build
/// let built = c.build_bytes(&Value::List(vec![
///     Value::UInt(1), Value::UInt(2), Value::UInt(3),
/// ])).unwrap();
/// assert_eq!(built, vec![1, 2, 3]);
/// ```
pub struct Array {
    /// The exact number of elements to parse/build.
    pub count: usize,
    /// The sub-construct applied to each element.
    pub subcon: Box<dyn Construct>,
    /// If `true`, parse returns an empty list (elements are consumed but
    /// discarded). During build, elements are still written but no result
    /// list is accumulated.
    pub discard: bool,
}

impl Array {
    /// Creates a new `Array` that processes exactly `count` elements using
    /// `subcon`.
    ///
    /// # Examples
    ///
    /// ```
    /// use construct::constructs::repetition::Array;
    /// use construct::constructs::format_field::INT8UB;
    ///
    /// let arr = Array::new(5, Box::new(INT8UB));
    /// ```
    pub fn new(count: usize, subcon: Box<dyn Construct>) -> Self {
        Array {
            count,
            subcon,
            discard: false,
        }
    }

    /// Creates a new `Array` with `discard = true`.
    ///
    /// Parsed elements are consumed from the stream but not collected into
    /// the result list. The result is always an empty [`Value::List`].
    ///
    /// # Examples
    ///
    /// ```
    /// use construct::constructs::repetition::Array;
    /// use construct::constructs::format_field::INT8UB;
    /// use construct::core::Construct;
    /// use construct::value::Value;
    ///
    /// let arr = Array::new_discard(3, Box::new(INT8UB));
    /// let c: &dyn Construct = &arr;
    /// let parsed = c.parse_bytes(b"\x01\x02\x03").unwrap();
    /// assert_eq!(parsed, Value::List(vec![]));
    /// ```
    pub fn new_discard(count: usize, subcon: Box<dyn Construct>) -> Self {
        Array {
            count,
            subcon,
            discard: true,
        }
    }
}

impl Construct for Array {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let mut list = Vec::with_capacity(if self.discard { 0 } else { self.count });

        for i in 0..self.count {
            ctx.insert(CONTEXT_INDEX_KEY, Value::UInt(i as u64));
            let element = self
                .subcon
                .parse(stream, ctx)
                .map_err(|e| e.with_path_prefix(&format!("[{i}]")))?;
            if !self.discard {
                list.push(element);
            }
        }

        Ok(Value::List(list))
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        let list = data.as_list().map_err(|e| {
            if matches!(e, ConstructError::TypeMismatch { .. }) {
                ConstructError::TypeMismatch {
                    path: String::new(),
                    expected: "List".to_string(),
                    actual: data.type_name().to_string(),
                }
            } else {
                e
            }
        })?;

        if list.len() != self.count {
            return Err(ConstructError::Array {
                path: String::new(),
                expected: self.count,
                actual: list.len(),
            });
        }

        for (i, element) in list.iter().enumerate() {
            ctx.insert(CONTEXT_INDEX_KEY, Value::UInt(i as u64));
            self.subcon
                .build(element, stream, ctx)
                .map_err(|e| e.with_path_prefix(&format!("[{i}]")))?;
        }

        Ok(())
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        let sub_size = self.subcon.sizeof(ctx)?;
        Ok(self.count.saturating_mul(sub_size))
    }
}

// ===========================================================================
// ArrayExpr
// ===========================================================================

/// An [`Array`] whose element count is computed at runtime from an expression.
///
/// - **parse**: evaluates `count_expr` to get the element count, then parses
///   that many elements into a [`Value::List`]
/// - **build**: builds each element from a list; if the list length does not
///   match the evaluated count, returns [`ConstructError::Array`]
/// - **sizeof**: returns [`ConstructError::Sizeof`] (count is runtime-dependent)
///
/// Corresponds to Python `Array(this.xxx, subcon)`.
///
/// # Examples
///
/// ```
/// use construct::constructs::repetition::ArrayExpr;
/// use construct::constructs::struct_::Struct;
/// use construct::constructs::format_field::INT8UB;
/// use construct::core::Construct;
/// use construct::value::Value;
/// use construct::expr::this_;
///
/// let d = Struct::new()
///     .field("count", Box::new(INT8UB))
///     .field("items", Box::new(ArrayExpr::new(
///         Box::new(this_().field("count")),
///         Box::new(INT8UB),
///     )));
/// let c: &dyn Construct = &d;
/// let parsed = c.parse_bytes(b"\x03\x01\x02\x03").unwrap();
/// ```
pub struct ArrayExpr {
    /// Expression that evaluates to the element count.
    pub count_expr: Box<dyn Evaluate>,
    /// The sub-construct applied to each element.
    pub subcon: Box<dyn Construct>,
}

impl ArrayExpr {
    /// Creates a new `ArrayExpr` with the given count expression and
    /// sub-construct.
    pub fn new(count_expr: Box<dyn Evaluate>, subcon: Box<dyn Construct>) -> Self {
        ArrayExpr { count_expr, subcon }
    }

    /// Evaluates the count expression and returns it as a `usize`.
    fn resolve_count(&self, ctx: &Context) -> Result<usize> {
        let val = self.count_expr.evaluate(ctx, None)?;
        let u = val.to_u64().map_err(|e| ConstructError::Expr {
            path: String::new(),
            message: format!("ArrayExpr count must be a non-negative integer: {e}"),
        })?;
        Ok(u as usize)
    }
}

impl Construct for ArrayExpr {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let count = self.resolve_count(ctx)?;
        let mut list = Vec::with_capacity(count);
        for i in 0..count {
            ctx.insert(CONTEXT_INDEX_KEY, Value::UInt(i as u64));
            let element = self
                .subcon
                .parse(stream, ctx)
                .map_err(|e| e.with_path_prefix(&format!("[{i}]")))?;
            list.push(element);
        }
        Ok(Value::List(list))
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        let expected = self.resolve_count(ctx)?;
        let list = data.as_list().map_err(|_| ConstructError::TypeMismatch {
            path: String::new(),
            expected: "List".to_string(),
            actual: data.type_name().to_string(),
        })?;

        if list.len() != expected {
            return Err(ConstructError::Array {
                path: String::new(),
                expected,
                actual: list.len(),
            });
        }

        for (i, element) in list.iter().enumerate() {
            ctx.insert(CONTEXT_INDEX_KEY, Value::UInt(i as u64));
            self.subcon
                .build(element, stream, ctx)
                .map_err(|e| e.with_path_prefix(&format!("[{i}]")))?;
        }
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Err(ConstructError::Sizeof {
            path: String::new(),
            reason: "ArrayExpr has variable size (count is runtime-dependent)".to_string(),
        })
    }
}

// ===========================================================================
// GreedyRange
// ===========================================================================

/// A variable-length homogenous array that parses until end-of-stream.
///
/// Parses elements until the sub-construct fails (typically due to EOF),
/// then seeks back to the position after the last successful parse and
/// returns all collected elements as a [`Value::List`].
///
/// Builds from a list by writing each element.
///
/// Size is undefined (returns [`ConstructError::Sizeof`]).
///
/// Corresponds to Python `GreedyRange(subcon, discard=False)`.
///
/// # Examples
///
/// ```
/// use construct::constructs::repetition::GreedyRange;
/// use construct::constructs::format_field::INT8UB;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let gr = GreedyRange::new(Box::new(INT8UB));
/// let c: &dyn Construct = &gr;
///
/// let parsed = c.parse_bytes(b"\x01\x02\x03").unwrap();
/// assert_eq!(parsed, Value::List(vec![
///     Value::UInt(1), Value::UInt(2), Value::UInt(3),
/// ]));
/// ```
pub struct GreedyRange {
    /// The sub-construct applied to each element.
    pub subcon: Box<dyn Construct>,
    /// If `true`, parse returns an empty list (elements are consumed but
    /// discarded).
    pub discard: bool,
}

impl GreedyRange {
    /// Creates a new `GreedyRange` with the given sub-construct.
    ///
    /// # Examples
    ///
    /// ```
    /// use construct::constructs::repetition::GreedyRange;
    /// use construct::constructs::format_field::INT8UB;
    ///
    /// let gr = GreedyRange::new(Box::new(INT8UB));
    /// ```
    pub fn new(subcon: Box<dyn Construct>) -> Self {
        GreedyRange {
            subcon,
            discard: false,
        }
    }

    /// Creates a new `GreedyRange` with `discard = true`.
    ///
    /// Parsed elements are consumed from the stream but not collected.
    ///
    /// # Examples
    ///
    /// ```
    /// use construct::constructs::repetition::GreedyRange;
    /// use construct::constructs::format_field::INT8UB;
    ///
    /// let gr = GreedyRange::new_discard(Box::new(INT8UB));
    /// ```
    pub fn new_discard(subcon: Box<dyn Construct>) -> Self {
        GreedyRange {
            subcon,
            discard: true,
        }
    }
}

impl Construct for GreedyRange {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let mut list = Vec::new();
        let mut i: u64 = 0;

        loop {
            ctx.insert(CONTEXT_INDEX_KEY, Value::UInt(i));

            // Save position before attempting to parse
            let fallback = stream.tell()?;

            match self.subcon.parse(stream, ctx) {
                Ok(element) => {
                    if !self.discard {
                        list.push(element);
                    }
                    i = i.saturating_add(1);
                }
                Err(ConstructError::StopField { .. }) => {
                    // StopField signals graceful termination
                    break;
                }
                Err(_) => {
                    // Any other error: seek back to after last successful parse
                    stream.seek(fallback)?;
                    break;
                }
            }
        }

        Ok(Value::List(list))
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        let list = data.as_list().map_err(|e| {
            if matches!(e, ConstructError::TypeMismatch { .. }) {
                ConstructError::TypeMismatch {
                    path: String::new(),
                    expected: "List".to_string(),
                    actual: data.type_name().to_string(),
                }
            } else {
                e
            }
        })?;

        for (i, element) in list.iter().enumerate() {
            ctx.insert(CONTEXT_INDEX_KEY, Value::UInt(i as u64));
            self.subcon
                .build(element, stream, ctx)
                .map_err(|e| e.with_path_prefix(&format!("[{i}]")))?;
        }

        Ok(())
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Err(ConstructError::Sizeof {
            path: String::new(),
            reason: "GreedyRange has undefined size".to_string(),
        })
    }
}

// ===========================================================================
// RepeatPredicate
// ===========================================================================

/// A predicate function used by [`RepeatUntil`] to decide when to stop.
///
/// The predicate receives:
/// - `&Value` — the most recently parsed/built element
/// - `&[Value]` — the list of all elements collected so far
/// - `&Context` — the current context
///
/// Returns `true` to stop iterating (the current element **is** included in
/// the result).
///
/// Corresponds to the Python `predicate` lambda `(obj, list, context) -> bool`.
pub type RepeatPredicate = Box<dyn Fn(&Value, &[Value], &Context) -> bool>;

// ===========================================================================
// RepeatUntil
// ===========================================================================

/// A variable-length homogenous array that repeats until a predicate is met.
///
/// Parses elements until the predicate returns `true`. The element that
/// satisfied the predicate **is** included in the result list.
///
/// Builds from a list: iterates over elements, calling the predicate on each.
/// If no element satisfies the predicate, returns
/// [`ConstructError::Array`] (corresponding to Python `RepeatError`).
///
/// Size is undefined (returns [`ConstructError::Sizeof`]).
///
/// Corresponds to Python `RepeatUntil(predicate, subcon, discard=False)`.
///
/// # Examples
///
/// ```
/// use construct::constructs::repetition::RepeatUntil;
/// use construct::constructs::format_field::INT8UB;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// // Stop when we encounter a value greater than 7
/// let ru = RepeatUntil::new(
///     Box::new(|obj, _list, _ctx| {
///         obj.to_u64().unwrap_or(0) > 7
///     }),
///     Box::new(INT8UB),
/// );
/// let c: &dyn Construct = &ru;
///
/// let parsed = c.parse_bytes(b"\x01\x02\x08").unwrap();
/// assert_eq!(parsed, Value::List(vec![
///     Value::UInt(1), Value::UInt(2), Value::UInt(8),
/// ]));
/// ```
pub struct RepeatUntil {
    /// The predicate that determines when to stop iterating.
    /// The last element for which the predicate returns `true` is included.
    pub predicate: RepeatPredicate,
    /// The sub-construct applied to each element.
    pub subcon: Box<dyn Construct>,
    /// If `true`, parse returns an empty list (elements are consumed but
    /// discarded).
    pub discard: bool,
}

impl RepeatUntil {
    /// Creates a new `RepeatUntil` with the given predicate and sub-construct.
    ///
    /// # Examples
    ///
    /// ```
    /// use construct::constructs::repetition::RepeatUntil;
    /// use construct::constructs::format_field::INT8UB;
    ///
    /// let ru = RepeatUntil::new(
    ///     Box::new(|obj, _list, _ctx| obj.to_u64().unwrap_or(0) > 5),
    ///     Box::new(INT8UB),
    /// );
    /// ```
    pub fn new(predicate: RepeatPredicate, subcon: Box<dyn Construct>) -> Self {
        RepeatUntil {
            predicate,
            subcon,
            discard: false,
        }
    }

    /// Creates a new `RepeatUntil` with `discard = true`.
    ///
    /// Parsed elements are consumed from the stream but not collected.
    pub fn new_discard(predicate: RepeatPredicate, subcon: Box<dyn Construct>) -> Self {
        RepeatUntil {
            predicate,
            subcon,
            discard: true,
        }
    }
}

impl Construct for RepeatUntil {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let mut list = Vec::new();
        let mut i: u64 = 0;

        loop {
            ctx.insert(CONTEXT_INDEX_KEY, Value::UInt(i));

            let element = self
                .subcon
                .parse(stream, ctx)
                .map_err(|e| e.with_path_prefix(&format!("[{i}]")))?;

            if !self.discard {
                list.push(element.clone());
            }

            if (self.predicate)(&element, &list, ctx) {
                return Ok(Value::List(list));
            }

            i = i.saturating_add(1);
        }
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        let list = data.as_list().map_err(|e| {
            if matches!(e, ConstructError::TypeMismatch { .. }) {
                ConstructError::TypeMismatch {
                    path: String::new(),
                    expected: "List".to_string(),
                    actual: data.type_name().to_string(),
                }
            } else {
                e
            }
        })?;

        // Track elements that have been built, for the predicate
        let mut built: Vec<Value> = Vec::new();

        for (i, element) in list.iter().enumerate() {
            ctx.insert(CONTEXT_INDEX_KEY, Value::UInt(i as u64));
            self.subcon
                .build(element, stream, ctx)
                .map_err(|e| e.with_path_prefix(&format!("[{i}]")))?;

            if !self.discard {
                built.push(element.clone());
            }

            if (self.predicate)(element, &built, ctx) {
                return Ok(());
            }
        }

        // No element matched the predicate
        Err(ConstructError::Array {
            path: String::new(),
            expected: 0,
            actual: 0,
        })
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Err(ConstructError::Sizeof {
            path: String::new(),
            reason: "RepeatUntil has undefined size, amount depends on actual data".to_string(),
        })
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constructs::format_field::INT8UB;
    use crate::core::error::ConstructError;

    /// Helper macro to treat a construct as `&dyn Construct`.
    macro_rules! as_dyn {
        ($c:expr) => {
            &$c as &dyn Construct
        };
    }

    // ======================================================================
    // Array — parse tests
    // ======================================================================

    #[test]
    fn array_parse_three_bytes() {
        let arr = Array::new(3, Box::new(INT8UB));
        let result = as_dyn!(arr).parse_bytes(b"\x01\x02\x03").unwrap();
        assert_eq!(
            result,
            Value::List(vec![Value::UInt(1), Value::UInt(2), Value::UInt(3)])
        );
    }

    #[test]
    fn array_parse_zero_count_returns_empty_list() {
        let arr = Array::new(0, Box::new(INT8UB));
        let result = as_dyn!(arr).parse_bytes(b"").unwrap();
        assert_eq!(result, Value::List(vec![]));
    }

    #[test]
    fn array_parse_one_element() {
        let arr = Array::new(1, Box::new(INT8UB));
        let result = as_dyn!(arr).parse_bytes(b"\x42").unwrap();
        assert_eq!(result, Value::List(vec![Value::UInt(0x42)]));
    }

    #[test]
    fn array_parse_insufficient_data_returns_error() {
        let arr = Array::new(3, Box::new(INT8UB));
        let err = as_dyn!(arr).parse_bytes(b"\x01").unwrap_err();
        assert!(matches!(err, ConstructError::Stream { .. }));
    }

    #[test]
    fn array_parse_discard_returns_empty_list() {
        let arr = Array::new_discard(3, Box::new(INT8UB));
        let result = as_dyn!(arr).parse_bytes(b"\x01\x02\x03").unwrap();
        assert_eq!(result, Value::List(vec![]));
        // Verify the data was actually consumed (stream position advanced)
    }

    #[test]
    fn array_parse_sets_index_in_context() {
        /// A test construct that captures `_index` from context during parse.
        struct IndexCapture;
        impl Construct for IndexCapture {
            fn parse(&self, _stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
                let idx = ctx
                    .get_recursive(CONTEXT_INDEX_KEY)
                    .cloned()
                    .unwrap_or(Value::UInt(9999));
                Ok(idx)
            }
            fn build(
                &self,
                _data: &Value,
                _stream: &mut dyn Stream,
                _ctx: &mut Context,
            ) -> Result<()> {
                Ok(())
            }
            fn sizeof(&self, _ctx: &Context) -> Result<usize> {
                Ok(0)
            }
        }

        let arr = Array::new(3, Box::new(IndexCapture));
        let result = as_dyn!(arr).parse_bytes(b"").unwrap();
        assert_eq!(
            result,
            Value::List(vec![Value::UInt(0), Value::UInt(1), Value::UInt(2)])
        );
    }

    // ======================================================================
    // Array — build tests
    // ======================================================================

    #[test]
    fn array_build_three_bytes() {
        let arr = Array::new(3, Box::new(INT8UB));
        let built = as_dyn!(arr)
            .build_bytes(&Value::List(vec![
                Value::UInt(1),
                Value::UInt(2),
                Value::UInt(3),
            ]))
            .unwrap();
        assert_eq!(built, vec![1, 2, 3]);
    }

    #[test]
    fn array_build_zero_count_with_empty_list() {
        let arr = Array::new(0, Box::new(INT8UB));
        let built = as_dyn!(arr).build_bytes(&Value::List(vec![])).unwrap();
        assert!(built.is_empty());
    }

    #[test]
    fn array_build_wrong_count_returns_error() {
        let arr = Array::new(3, Box::new(INT8UB));
        let err = as_dyn!(arr)
            .build_bytes(&Value::List(vec![Value::UInt(1), Value::UInt(2)]))
            .unwrap_err();
        match err {
            ConstructError::Array {
                expected, actual, ..
            } => {
                assert_eq!(expected, 3);
                assert_eq!(actual, 2);
            }
            other => panic!("expected Array error, got {:?}", other),
        }
    }

    #[test]
    fn array_build_non_list_returns_type_mismatch() {
        let arr = Array::new(3, Box::new(INT8UB));
        let err = as_dyn!(arr).build_bytes(&Value::Int(42)).unwrap_err();
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
    }

    // ======================================================================
    // Array — sizeof tests
    // ======================================================================

    #[test]
    fn array_sizeof_is_count_times_subcon_size() {
        let arr = Array::new(5, Box::new(INT8UB));
        assert_eq!(arr.sizeof(&Context::new()).unwrap(), 5);
    }

    #[test]
    fn array_sizeof_zero_count() {
        let arr = Array::new(0, Box::new(INT8UB));
        assert_eq!(arr.sizeof(&Context::new()).unwrap(), 0);
    }

    // ======================================================================
    // Array — roundtrip tests
    // ======================================================================

    #[test]
    fn array_roundtrip() {
        let arr = Array::new(4, Box::new(INT8UB));
        let original = Value::List(vec![
            Value::UInt(10),
            Value::UInt(20),
            Value::UInt(30),
            Value::UInt(40),
        ]);

        let c: &dyn Construct = &arr;
        let built = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    // ======================================================================
    // GreedyRange — parse tests
    // ======================================================================

    #[test]
    fn greedy_range_parse_all_bytes() {
        let gr = GreedyRange::new(Box::new(INT8UB));
        let result = as_dyn!(gr).parse_bytes(b"\x01\x02\x03").unwrap();
        assert_eq!(
            result,
            Value::List(vec![Value::UInt(1), Value::UInt(2), Value::UInt(3)])
        );
    }

    #[test]
    fn greedy_range_parse_empty_stream() {
        let gr = GreedyRange::new(Box::new(INT8UB));
        let result = as_dyn!(gr).parse_bytes(b"").unwrap();
        assert_eq!(result, Value::List(vec![]));
    }

    #[test]
    fn greedy_range_parse_discard_returns_empty() {
        let gr = GreedyRange::new_discard(Box::new(INT8UB));
        let result = as_dyn!(gr).parse_bytes(b"\x01\x02\x03").unwrap();
        assert_eq!(result, Value::List(vec![]));
    }

    #[test]
    fn greedy_range_parse_partial_failure_seeks_back() {
        use crate::constructs::bytes::Bytes;

        // GreedyRange with a 2-byte subcon. Data has 5 bytes, so:
        // - Parse 2 bytes (succeed)
        // - Parse 2 bytes (succeed)
        // - Parse 2 bytes (fail, only 1 byte left)
        // - Seek back to position 4
        // - Result: 2 elements (4 bytes consumed)
        let gr = GreedyRange::new(Box::new(Bytes::new(2)));
        let result = as_dyn!(gr).parse_bytes(b"\x01\x02\x03\x04\x05").unwrap();
        let list = result.as_list().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0], Value::Bytes(vec![1, 2]));
        assert_eq!(list[1], Value::Bytes(vec![3, 4]));
    }

    #[test]
    fn greedy_range_parse_stops_on_stop_field() {
        use crate::core::error::Result;

        /// A construct that returns StopField after parsing `limit` elements.
        struct StopAfter {
            limit: usize,
        }

        impl Construct for StopAfter {
            fn parse(&self, stream: &mut dyn Stream, _ctx: &mut Context) -> Result<Value> {
                let pos = stream.tell()? as usize;
                if pos >= self.limit {
                    return Err(ConstructError::StopField {
                        path: String::new(),
                    });
                }
                let byte = stream.read_bytes(1)?;
                Ok(Value::UInt(byte[0] as u64))
            }
            fn build(
                &self,
                _data: &Value,
                _stream: &mut dyn Stream,
                _ctx: &mut Context,
            ) -> Result<()> {
                Ok(())
            }
            fn sizeof(&self, _ctx: &Context) -> Result<usize> {
                Ok(1)
            }
        }

        let gr = GreedyRange::new(Box::new(StopAfter { limit: 3 }));
        let result = as_dyn!(gr).parse_bytes(b"\x0A\x0B\x0C\x0D").unwrap();
        // Should parse 3 elements (0, 1, 2), then StopField at position 3
        assert_eq!(
            result,
            Value::List(vec![Value::UInt(10), Value::UInt(11), Value::UInt(12)])
        );
    }

    #[test]
    fn greedy_range_parse_sets_index_in_context() {
        struct IndexCapture;
        impl Construct for IndexCapture {
            fn parse(&self, _stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
                let idx = ctx
                    .get_recursive(CONTEXT_INDEX_KEY)
                    .cloned()
                    .unwrap_or(Value::UInt(9999));
                // Stop after 3 elements to avoid infinite loop
                if idx == Value::UInt(3) {
                    return Err(ConstructError::Stream {
                        path: String::new(),
                        source: std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "stop"),
                    });
                }
                Ok(idx)
            }
            fn build(
                &self,
                _data: &Value,
                _stream: &mut dyn Stream,
                _ctx: &mut Context,
            ) -> Result<()> {
                Ok(())
            }
            fn sizeof(&self, _ctx: &Context) -> Result<usize> {
                Ok(0)
            }
        }

        let gr = GreedyRange::new(Box::new(IndexCapture));
        let result = as_dyn!(gr).parse_bytes(b"").unwrap();
        assert_eq!(
            result,
            Value::List(vec![Value::UInt(0), Value::UInt(1), Value::UInt(2)])
        );
    }

    // ======================================================================
    // GreedyRange — build tests
    // ======================================================================

    #[test]
    fn greedy_range_build_three_bytes() {
        let gr = GreedyRange::new(Box::new(INT8UB));
        let built = as_dyn!(gr)
            .build_bytes(&Value::List(vec![
                Value::UInt(1),
                Value::UInt(2),
                Value::UInt(3),
            ]))
            .unwrap();
        assert_eq!(built, vec![1, 2, 3]);
    }

    #[test]
    fn greedy_range_build_empty_list() {
        let gr = GreedyRange::new(Box::new(INT8UB));
        let built = as_dyn!(gr).build_bytes(&Value::List(vec![])).unwrap();
        assert!(built.is_empty());
    }

    #[test]
    fn greedy_range_build_non_list_returns_type_mismatch() {
        let gr = GreedyRange::new(Box::new(INT8UB));
        let err = as_dyn!(gr).build_bytes(&Value::None).unwrap_err();
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
    }

    // ======================================================================
    // GreedyRange — sizeof tests
    // ======================================================================

    #[test]
    fn greedy_range_sizeof_returns_error() {
        let gr = GreedyRange::new(Box::new(INT8UB));
        let err = gr.sizeof(&Context::new()).unwrap_err();
        assert!(matches!(err, ConstructError::Sizeof { .. }));
    }

    // ======================================================================
    // GreedyRange — roundtrip tests
    // ======================================================================

    #[test]
    fn greedy_range_roundtrip() {
        let gr = GreedyRange::new(Box::new(INT8UB));
        let original = Value::List(vec![Value::UInt(10), Value::UInt(20), Value::UInt(30)]);

        let c: &dyn Construct = &gr;
        let built = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    // ======================================================================
    // RepeatUntil — parse tests
    // ======================================================================

    #[test]
    fn repeat_until_parse_stops_on_predicate() {
        // Stop when value > 7
        let ru = RepeatUntil::new(
            Box::new(|obj, _list, _ctx| obj.to_u64().unwrap_or(0) > 7),
            Box::new(INT8UB),
        );
        let result = as_dyn!(ru).parse_bytes(b"\x01\x02\x08").unwrap();
        assert_eq!(
            result,
            Value::List(vec![Value::UInt(1), Value::UInt(2), Value::UInt(8)])
        );
    }

    #[test]
    fn repeat_until_parse_first_element_matches() {
        let ru = RepeatUntil::new(Box::new(|_obj, _list, _ctx| true), Box::new(INT8UB));
        let result = as_dyn!(ru).parse_bytes(b"\x42").unwrap();
        assert_eq!(result, Value::List(vec![Value::UInt(0x42)]));
    }

    #[test]
    fn repeat_until_parse_discard_returns_empty_list() {
        let ru = RepeatUntil::new_discard(
            Box::new(|obj, _list, _ctx| obj.to_u64().unwrap_or(0) > 7),
            Box::new(INT8UB),
        );
        let result = as_dyn!(ru).parse_bytes(b"\x01\x08").unwrap();
        // Elements are consumed but not collected
        assert_eq!(result, Value::List(vec![]));
    }

    #[test]
    fn repeat_until_parse_predicate_can_inspect_list() {
        // Stop when the last two elements are [0, 0]
        let ru = RepeatUntil::new(
            Box::new(|_obj, list, _ctx| {
                let len = list.len();
                len >= 2 && list[len - 2] == Value::UInt(0) && list[len - 1] == Value::UInt(0)
            }),
            Box::new(INT8UB),
        );
        let result = as_dyn!(ru).parse_bytes(b"\x01\x00\x00\xFF").unwrap();
        // Should stop after [1, 0, 0] because last two are [0, 0]
        assert_eq!(
            result,
            Value::List(vec![Value::UInt(1), Value::UInt(0), Value::UInt(0)])
        );
    }

    #[test]
    fn repeat_until_parse_sets_index_in_context() {
        struct IndexCapture;
        impl Construct for IndexCapture {
            fn parse(&self, _stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
                let idx = ctx
                    .get_recursive(CONTEXT_INDEX_KEY)
                    .cloned()
                    .unwrap_or(Value::UInt(9999));
                Ok(idx)
            }
            fn build(
                &self,
                _data: &Value,
                _stream: &mut dyn Stream,
                _ctx: &mut Context,
            ) -> Result<()> {
                Ok(())
            }
            fn sizeof(&self, _ctx: &Context) -> Result<usize> {
                Ok(0)
            }
        }

        let ru = RepeatUntil::new(
            Box::new(|obj, _list, _ctx| obj == &Value::UInt(2)),
            Box::new(IndexCapture),
        );
        let result = as_dyn!(ru).parse_bytes(b"").unwrap();
        assert_eq!(
            result,
            Value::List(vec![Value::UInt(0), Value::UInt(1), Value::UInt(2)])
        );
    }

    #[test]
    fn repeat_until_parse_insufficient_data_returns_error() {
        let ru = RepeatUntil::new(
            Box::new(|_obj, _list, _ctx| false), // never satisfied
            Box::new(INT8UB),
        );
        let err = as_dyn!(ru).parse_bytes(b"\x01").unwrap_err();
        assert!(matches!(err, ConstructError::Stream { .. }));
    }

    // ======================================================================
    // RepeatUntil — build tests
    // ======================================================================

    #[test]
    fn repeat_until_build_with_matching_element() {
        let ru = RepeatUntil::new(
            Box::new(|obj, _list, _ctx| obj.to_u64().unwrap_or(0) > 7),
            Box::new(INT8UB),
        );
        let built = as_dyn!(ru)
            .build_bytes(&Value::List(vec![
                Value::UInt(1),
                Value::UInt(2),
                Value::UInt(8),
            ]))
            .unwrap();
        assert_eq!(built, vec![1, 2, 8]);
    }

    #[test]
    fn repeat_until_build_no_match_returns_error() {
        let ru = RepeatUntil::new(
            Box::new(|_obj, _list, _ctx| false), // never satisfied
            Box::new(INT8UB),
        );
        let err = as_dyn!(ru)
            .build_bytes(&Value::List(vec![Value::UInt(1), Value::UInt(2)]))
            .unwrap_err();
        assert!(matches!(err, ConstructError::Array { .. }));
    }

    #[test]
    fn repeat_until_build_non_list_returns_type_mismatch() {
        let ru = RepeatUntil::new(Box::new(|_obj, _list, _ctx| true), Box::new(INT8UB));
        let err = as_dyn!(ru)
            .build_bytes(&Value::String("nope".to_string()))
            .unwrap_err();
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
    }

    // ======================================================================
    // RepeatUntil — sizeof tests
    // ======================================================================

    #[test]
    fn repeat_until_sizeof_returns_error() {
        let ru = RepeatUntil::new(Box::new(|_obj, _list, _ctx| true), Box::new(INT8UB));
        let err = ru.sizeof(&Context::new()).unwrap_err();
        assert!(matches!(err, ConstructError::Sizeof { .. }));
    }

    // ======================================================================
    // RepeatUntil — roundtrip tests
    // ======================================================================

    #[test]
    fn repeat_until_roundtrip() {
        let ru = RepeatUntil::new(
            Box::new(|obj, _list, _ctx| obj.to_u64().unwrap_or(0) >= 5),
            Box::new(INT8UB),
        );
        let original = Value::List(vec![Value::UInt(1), Value::UInt(3), Value::UInt(5)]);

        let c: &dyn Construct = &ru;
        let built = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    // ======================================================================
    // Error path enrichment tests
    // ======================================================================

    #[test]
    fn array_parse_error_has_index_in_path() {
        let arr = Array::new(3, Box::new(INT8UB));
        // Only 1 byte available; parsing element [1] should fail
        let err = as_dyn!(arr).parse_bytes(b"\x01").unwrap_err();
        // Path should include "[1]"
        assert!(
            err.path().contains("[1]"),
            "expected path to contain '[1]', got: {}",
            err.path()
        );
    }

    #[test]
    fn array_build_error_has_index_in_path() {
        // Array of 2 INT8UB elements. Second element is a String (wrong type).
        let arr = Array::new(2, Box::new(INT8UB));
        let err = as_dyn!(arr)
            .build_bytes(&Value::List(vec![
                Value::UInt(1),
                Value::String("not a number".to_string()), // wrong type
            ]))
            .unwrap_err();
        assert!(
            err.path().contains("[1]"),
            "expected path to contain '[1]', got: {}",
            err.path()
        );
    }

    #[test]
    fn greedy_range_build_error_has_index_in_path() {
        // GreedyRange with INT8UB. Second element is a String (wrong type).
        let gr = GreedyRange::new(Box::new(INT8UB));
        let err = as_dyn!(gr)
            .build_bytes(&Value::List(vec![
                Value::UInt(1),
                Value::String("not a number".to_string()), // wrong type
            ]))
            .unwrap_err();
        assert!(
            err.path().contains("[1]"),
            "expected path to contain '[1]', got: {}",
            err.path()
        );
    }

    #[test]
    fn repeat_until_parse_error_has_index_in_path() {
        use crate::constructs::bytes::Bytes;

        // RepeatUntil with Bytes(2), stop when bytes start with 0xFF
        let ru = RepeatUntil::new(
            Box::new(|obj, _list, _ctx| {
                obj.as_bytes()
                    .map(|b| !b.is_empty() && b[0] == 0xFF)
                    .unwrap_or(false)
            }),
            Box::new(Bytes::new(2)),
        );
        // Only 1 byte, so Bytes(2) will fail on first attempt
        let err = as_dyn!(ru).parse_bytes(b"\x01").unwrap_err();
        assert!(
            err.path().contains("[0]"),
            "expected path to contain '[0]', got: {}",
            err.path()
        );
    }
}
