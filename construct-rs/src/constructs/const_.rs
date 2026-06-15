//! Const construct — enforces a constant value during parse and build.
//!
//! [`Const`] wraps an inner construct and validates that parsed data matches
//! the expected constant value. On build, it ignores the supplied value and
//! writes the constant instead.
//!
//! Corresponds to Python `Const(value, subcon)`.

use crate::combined::CombinedConstruct;
use crate::constructs::bytes::Bytes;
use crate::core::context::Context;
use crate::core::error::{ConstructError, Result};
use crate::core::stream::CombinedStream;
use crate::core::Construct;
use crate::value::Value;

/// A constant-value enforcement construct.
///
/// - **parse**: delegates to `subcon`, then checks the result equals `value`
/// - **build**: builds `value` via `subcon` (ignoring the supplied data,
///   unless it is `Value::None` or matches `value`)
/// - **sizeof**: delegates to `subcon`
///
/// Corresponds to Python `Const(value, subcon)`.
///
/// # Examples
///
/// ```
/// use construct::constructs::const_::Const;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let c: &dyn Construct = &Const::new_bytes(b"IHDR".to_vec());
/// let parsed = c.parse_bytes(b"IHDR").unwrap();
/// assert_eq!(parsed, Value::Bytes(b"IHDR".to_vec()));
///
/// let built = c.build_bytes(&Value::None).unwrap();
/// assert_eq!(built, b"IHDR");
/// ```
pub struct Const {
    /// The expected constant value.
    pub value: Value,
    /// The inner construct used for parsing and building.
    pub subcon: Box<CombinedConstruct>,
}

impl Const {
    /// Creates a byte-constant construct.
    ///
    /// Automatically creates a [`Bytes`] subcon of the appropriate length.
    ///
    /// # Examples
    ///
    /// ```
    /// use construct::constructs::const_::Const;
    /// use construct::core::Construct;
    /// use construct::value::Value;
    ///
    /// let c = Const::new_bytes(b"IHDR".to_vec());
    /// assert_eq!(c.value, Value::Bytes(b"IHDR".to_vec()));
    /// ```
    pub fn new_bytes(value: Vec<u8>) -> Self {
        let length = value.len();
        Const {
            value: Value::Bytes(value),
            subcon: Box::new(Bytes::new(length).into()),
        }
    }

    /// Creates a value-constant construct with a custom subcon.
    ///
    /// # Examples
    ///
    /// ```
    /// use construct::constructs::const_::Const;
    /// use construct::constructs::format_field::INT8UB;
    /// use construct::core::Construct;
    /// use construct::value::Value;
    ///
    /// let c = Const::new_value(Value::UInt(255), Box::new(INT8UB.into()));
    /// assert_eq!(c.value, Value::UInt(255));
    /// ```
    pub fn new_value(value: Value, subcon: Box<CombinedConstruct>) -> Self {
        Const { value, subcon }
    }
}

impl Construct for Const {
    fn parse(&self, stream: &mut CombinedStream, ctx: &mut Context) -> Result<Value> {
        let obj = self.subcon.parse(stream, ctx)?;
        if obj != self.value {
            return Err(ConstructError::Const {
                path: String::new(),
                expected: format!("{:?}", self.value),
                actual: format!("{:?}", obj),
            });
        }
        Ok(obj)
    }

    fn build(&self, data: &Value, stream: &mut CombinedStream, ctx: &mut Context) -> Result<()> {
        // Python: if obj not in (None, self.value): raise ConstError
        if !data.is_none() && *data != self.value {
            return Err(ConstructError::Const {
                path: String::new(),
                expected: format!("None or {:?}", self.value),
                actual: format!("{:?}", data),
            });
        }
        self.subcon.build(&self.value, stream, ctx)
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        self.subcon.sizeof(ctx)
    }

    fn flagbuildnone(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constructs::format_field::INT8UB;
    use crate::core::error::ConstructError;
    use crate::core::Construct;

    // -- new_bytes construction ----------------------------------------------

    #[test]
    fn new_bytes_creates_bytes_const() {
        let c = Const::new_bytes(b"IHDR".to_vec());
        assert_eq!(c.value, Value::Bytes(b"IHDR".to_vec()));
    }

    #[test]
    fn new_bytes_empty_vec_is_valid() {
        let c = Const::new_bytes(vec![]);
        assert_eq!(c.value, Value::Bytes(vec![]));
        let size = c.sizeof(&Context::new()).unwrap();
        assert_eq!(size, 0);
    }

    // -- new_value construction ----------------------------------------------

    #[test]
    fn new_value_creates_custom_const() {
        let c = Const::new_value(Value::UInt(255), Box::new(INT8UB.into()));
        assert_eq!(c.value, Value::UInt(255));
    }

    // -- Parse tests ---------------------------------------------------------

    #[test]
    fn parse_matching_bytes_returns_value() {
        let c: &dyn Construct = &Const::new_bytes(b"IHDR".to_vec());
        let result = c.parse_bytes(b"IHDR").unwrap();
        assert_eq!(result, Value::Bytes(b"IHDR".to_vec()));
    }

    #[test]
    fn parse_matching_uint_returns_value() {
        let c: &dyn Construct = &Const::new_value(Value::UInt(42), Box::new(INT8UB.into()));
        let result = c.parse_bytes(b"\x2A").unwrap();
        assert_eq!(result, Value::UInt(42));
    }

    #[test]
    fn parse_mismatching_bytes_returns_const_error() {
        let c: &dyn Construct = &Const::new_bytes(b"IHDR".to_vec());
        let err = c.parse_bytes(b"JPEG").unwrap_err();
        assert!(matches!(err, ConstructError::Const { .. }));
        // Debug format for Value::Bytes shows numeric bytes, e.g. Bytes([73, 72, 68, 82])
        match err {
            ConstructError::Const {
                expected, actual, ..
            } => {
                assert!(
                    expected.contains("73, 72, 68, 82"),
                    "expected should contain IHDR bytes, got: {expected}"
                );
                assert!(
                    actual.contains("74, 80, 69, 71"),
                    "actual should contain JPEG bytes, got: {actual}"
                );
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn parse_mismatching_uint_returns_const_error() {
        let c: &dyn Construct = &Const::new_value(Value::UInt(42), Box::new(INT8UB.into()));
        let err = c.parse_bytes(b"\x2B").unwrap_err();
        assert!(matches!(err, ConstructError::Const { .. }));
    }

    #[test]
    fn parse_insufficient_data_returns_stream_error() {
        let c: &dyn Construct = &Const::new_bytes(b"IHDR".to_vec());
        let err = c.parse_bytes(b"IH").unwrap_err();
        assert!(matches!(err, ConstructError::Stream { .. }));
    }

    // -- Build tests ---------------------------------------------------------

    #[test]
    fn build_none_writes_constant() {
        let c: &dyn Construct = &Const::new_bytes(b"IHDR".to_vec());
        let built = c.build_bytes(&Value::None).unwrap();
        assert_eq!(built, b"IHDR");
    }

    #[test]
    fn build_matching_value_writes_constant() {
        let c: &dyn Construct = &Const::new_bytes(b"IHDR".to_vec());
        let built = c.build_bytes(&Value::Bytes(b"IHDR".to_vec())).unwrap();
        assert_eq!(built, b"IHDR");
    }

    #[test]
    fn build_wrong_value_returns_const_error() {
        let c: &dyn Construct = &Const::new_bytes(b"IHDR".to_vec());
        let err = c.build_bytes(&Value::Bytes(b"JPEG".to_vec())).unwrap_err();
        assert!(matches!(err, ConstructError::Const { .. }));
    }

    #[test]
    fn build_arbitrary_type_returns_const_error() {
        let c: &dyn Construct = &Const::new_bytes(b"IHDR".to_vec());
        let err = c.build_bytes(&Value::Int(0)).unwrap_err();
        assert!(matches!(err, ConstructError::Const { .. }));
    }

    #[test]
    fn build_uint_const_with_none() {
        let c: &dyn Construct = &Const::new_value(Value::UInt(42), Box::new(INT8UB.into()));
        let built = c.build_bytes(&Value::None).unwrap();
        assert_eq!(built, vec![42]);
    }

    #[test]
    fn build_uint_const_with_matching_value() {
        let c: &dyn Construct = &Const::new_value(Value::UInt(42), Box::new(INT8UB.into()));
        let built = c.build_bytes(&Value::UInt(42)).unwrap();
        assert_eq!(built, vec![42]);
    }

    #[test]
    fn build_uint_const_with_wrong_value_returns_error() {
        let c: &dyn Construct = &Const::new_value(Value::UInt(42), Box::new(INT8UB.into()));
        let err = c.build_bytes(&Value::UInt(99)).unwrap_err();
        assert!(matches!(err, ConstructError::Const { .. }));
    }

    // -- Sizeof tests --------------------------------------------------------

    #[test]
    fn sizeof_delegates_to_subcon_bytes() {
        let c = Const::new_bytes(b"IHDR".to_vec());
        assert_eq!(c.sizeof(&Context::new()).unwrap(), 4);
    }

    #[test]
    fn sizeof_delegates_to_subcon_uint() {
        let c = Const::new_value(Value::UInt(42), Box::new(INT8UB.into()));
        assert_eq!(c.sizeof(&Context::new()).unwrap(), 1);
    }

    // -- Roundtrip tests -----------------------------------------------------

    #[test]
    fn roundtrip_bytes_const() {
        let c: &dyn Construct = &Const::new_bytes(b"IHDR".to_vec());
        let built = c.build_bytes(&Value::None).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, Value::Bytes(b"IHDR".to_vec()));
    }

    #[test]
    fn roundtrip_uint_const() {
        let c: &dyn Construct = &Const::new_value(Value::UInt(42), Box::new(INT8UB.into()));
        let built = c.build_bytes(&Value::None).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, Value::UInt(42));
    }
}
