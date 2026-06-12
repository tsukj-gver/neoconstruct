//! Enum, FlagsEnum, and Mapping constructs — value-to-label translation.
//!
//! This module provides three adapter-like constructs that translate between
//! integer (or arbitrary) values and human-readable labels.
//!
//! | Construct | Parse output | Build input |
//! |-----------|-------------|-------------|
//! | [`Enum`] | `Value::String` (label) or `Value::UInt`/`Value::Int` (unmapped) | `Value::String` (label) or integer |
//! | [`FlagsEnum`] | `Value::Container` of `Value::Bool` | `Value::Container` of `Value::Bool` |
//! | [`Mapping`] | arbitrary `Value` | arbitrary `Value` |
//!
//! Corresponds to Python `Enum` (line ~1920), `FlagsEnum` (line ~2018), and
//! `Mapping` (line ~2112) in `construct/construct/core.py`.

use indexmap::IndexMap;

use crate::core::context::Context;
use crate::core::error::{ConstructError, Result};
use crate::core::stream::Stream;
use crate::core::Construct;
use crate::value::Value;

// ===========================================================================
// Enum
// ===========================================================================

/// A bidirectional integer ↔ string enum mapping.
///
/// Parses an integer via `subcon`, then looks up the value in the decoding
/// map (`decmap`). If found, returns a [`Value::String`] with the label name;
/// otherwise returns the raw integer value. Builds by looking up a string
/// label in the encoding map (`mapping`) and passing the integer to `subcon`.
/// Integer values can also be built directly.
///
/// Corresponds to Python `Enum(subcon, **mapping)`.
///
/// # Examples
///
/// ```
/// use construct::constructs::enum_::Enum;
/// use construct::constructs::format_field::INT8UB;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let e = Enum::new(
///     Box::new(INT8UB),
///     [("one".to_string(), 1), ("two".to_string(), 2), ("four".to_string(), 4)].into_iter().collect(),
/// );
///
/// // Parse mapped value → string
/// assert_eq!((&e as &dyn Construct).parse_bytes(b"\x01").unwrap(), Value::String("one".to_string()));
/// // Parse unmapped value → raw integer
/// assert_eq!((&e as &dyn Construct).parse_bytes(b"\xFF").unwrap(), Value::UInt(255));
///
/// // Build from string label
/// assert_eq!((&e as &dyn Construct).build_bytes(&Value::String("one".to_string())).unwrap(), vec![1]);
/// // Build from integer (pass-through)
/// assert_eq!((&e as &dyn Construct).build_bytes(&Value::UInt(5)).unwrap(), vec![5]);
/// ```
pub struct Enum {
    /// The inner construct that reads/writes the raw integer.
    pub subcon: Box<dyn Construct>,
    /// Encoding map: label name → integer value.
    pub mapping: IndexMap<String, u64>,
    /// Decoding map: integer value → label name.
    pub decmap: IndexMap<u64, String>,
}

impl Enum {
    /// Creates a new `Enum` construct with the given sub-construct and mapping.
    ///
    /// The `mapping` parameter maps string labels to integer values. The
    /// reverse (decoding) map is built automatically.
    ///
    /// # Examples
    ///
    /// ```
    /// use construct::constructs::enum_::Enum;
    /// use construct::constructs::format_field::INT8UB;
    ///
    /// let mut mapping = indexmap::IndexMap::new();
    /// mapping.insert("yes".to_string(), 1);
    /// mapping.insert("no".to_string(), 0);
    /// let e = Enum::new(Box::new(INT8UB), mapping);
    /// ```
    pub fn new(subcon: Box<dyn Construct>, mapping: IndexMap<String, u64>) -> Self {
        let decmap: IndexMap<u64, String> = mapping
            .iter()
            .map(|(name, &value)| (value, name.clone()))
            .collect();
        Enum {
            subcon,
            mapping,
            decmap,
        }
    }
}

impl Construct for Enum {
    /// Parses a value from `stream` using the sub-construct, then translates
    /// it through the decoding map.
    ///
    /// If the parsed integer has a mapping, returns [`Value::String`].
    /// If not, returns the raw integer value unchanged (default mapping).
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let obj = self.subcon.parse(stream, ctx)?;
        let int_val = obj.to_u64().map_err(|e| e.with_path_prefix("Enum"))?;
        match self.decmap.get(&int_val) {
            Some(label) => Ok(Value::String(label.clone())),
            None => Ok(obj),
        }
    }

    /// Builds binary data from a string label or integer value.
    ///
    /// If `data` is a [`Value::String`], looks it up in the encoding map.
    /// If `data` is an integer, passes it through directly.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::Mapping`] if a string label is not found
    /// in the encoding map.
    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        let build_val = match data {
            Value::String(label) => match self.mapping.get(label) {
                Some(&v) => Value::UInt(v),
                None => {
                    return Err(ConstructError::Mapping {
                        path: String::new(),
                        key: format!("{:?}", label),
                    }
                    .with_path_prefix("Enum"))
                }
            },
            Value::Int(i) => Value::Int(*i),
            other => {
                let v = other.to_u64().map_err(|e| e.with_path_prefix("Enum"))?;
                Value::UInt(v)
            }
        };
        self.subcon
            .build(&build_val, stream, ctx)
            .map_err(|e| e.with_path_prefix("Enum"))
    }

    /// Returns the byte size of the sub-construct.
    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        self.subcon
            .sizeof(ctx)
            .map_err(|e| e.with_path_prefix("Enum"))
    }
}

// ===========================================================================
// FlagsEnum
// ===========================================================================

/// A bit-flag enum that maps integer bits to named boolean flags.
///
/// Parses an integer via `subcon`, then creates a [`Value::Container`] where
/// each flag name maps to a [`Value::Bool`] indicating whether that flag's bits
/// are set. Builds by OR-ing together the values of all flags that are `true`.
///
/// Corresponds to Python `FlagsEnum(subcon, **flags)`.
///
/// # Examples
///
/// ```
/// use construct::constructs::enum_::FlagsEnum;
/// use construct::constructs::format_field::INT8UB;
/// use construct::core::Construct;
/// use construct::value::Value;
/// use indexmap::IndexMap;
///
/// let mut flags = IndexMap::new();
/// flags.insert("read".to_string(), 1);
/// flags.insert("write".to_string(), 2);
/// flags.insert("exec".to_string(), 4);
/// let fe = FlagsEnum::new(Box::new(INT8UB), flags);
///
/// // Parse 0b101 → read=true, write=false, exec=true
/// let parsed = (&fe as &dyn Construct).parse_bytes(b"\x05").unwrap();
/// let container = parsed.as_container().unwrap();
/// assert_eq!(container.get("read").unwrap(), &Value::Bool(true));
/// assert_eq!(container.get("write").unwrap(), &Value::Bool(false));
/// assert_eq!(container.get("exec").unwrap(), &Value::Bool(true));
/// ```
pub struct FlagsEnum {
    /// The inner construct that reads/writes the raw integer.
    pub subcon: Box<dyn Construct>,
    /// Mapping of flag names to their integer bit values.
    pub flags: IndexMap<String, u64>,
}

impl FlagsEnum {
    /// Creates a new `FlagsEnum` with the given sub-construct and flag mapping.
    ///
    /// # Examples
    ///
    /// ```
    /// use construct::constructs::enum_::FlagsEnum;
    /// use construct::constructs::format_field::INT8UB;
    /// use indexmap::IndexMap;
    ///
    /// let mut flags = IndexMap::new();
    /// flags.insert("a".to_string(), 1);
    /// flags.insert("b".to_string(), 2);
    /// let fe = FlagsEnum::new(Box::new(INT8UB), flags);
    /// ```
    pub fn new(subcon: Box<dyn Construct>, flags: IndexMap<String, u64>) -> Self {
        FlagsEnum { subcon, flags }
    }
}

impl Construct for FlagsEnum {
    /// Parses an integer and expands it into a container of boolean flags.
    ///
    /// For each flag in `flags`, the corresponding entry in the returned
    /// container is `true` if `(value & flag_value) == flag_value`, and
    /// `false` otherwise.
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let obj = self.subcon.parse(stream, ctx)?;
        let int_val = obj.to_u64().map_err(|e| e.with_path_prefix("FlagsEnum"))?;

        let mut container = IndexMap::new();
        for (name, &flag_value) in &self.flags {
            let set = (int_val & flag_value) == flag_value;
            container.insert(name.clone(), Value::Bool(set));
        }
        Ok(Value::Container(container))
    }

    /// Builds an integer from a container of boolean flags.
    ///
    /// Reads each flag name from the container. If the flag value is truthy
    /// (a [`Value::Bool`] set to `true`), its bit value is OR-ed into the
    /// result.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::Mapping`] if a flag name in the container is
    /// not found in the flags mapping.
    /// Returns [`ConstructError::TypeMismatch`] if `data` is not a
    /// [`Value::Container`].
    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        let container = data
            .as_container()
            .map_err(|e| e.with_path_prefix("FlagsEnum"))?;

        let mut flags_val: u64 = 0;
        for (name, value) in container {
            // Skip internal keys starting with '_' (mirrors Python behavior)
            if name.starts_with('_') {
                continue;
            }
            match self.flags.get(name) {
                Some(&flag_bits) => {
                    let is_set = value
                        .as_bool()
                        .map_err(|e| e.with_path_prefix("FlagsEnum"))?;
                    if is_set {
                        flags_val |= flag_bits;
                    }
                }
                None => {
                    return Err(ConstructError::Mapping {
                        path: String::new(),
                        key: format!("{:?}", name),
                    }
                    .with_path_prefix("FlagsEnum"));
                }
            }
        }

        self.subcon
            .build(&Value::UInt(flags_val), stream, ctx)
            .map_err(|e| e.with_path_prefix("FlagsEnum"))
    }

    /// Returns the byte size of the sub-construct.
    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        self.subcon
            .sizeof(ctx)
            .map_err(|e| e.with_path_prefix("FlagsEnum"))
    }
}

// ===========================================================================
// Mapping
// ===========================================================================

/// A general-purpose bidirectional value mapping.
///
/// Unlike [`Enum`] (which is specialized for integer ↔ string), `Mapping`
/// can translate between arbitrary [`Value`] types. The `mapping` field stores
/// (build-key, build-value) pairs used during build. The `decmapping` field
/// stores (parse-value, parse-key) pairs used during parse. Both are stored
/// as `Vec`s because [`Value`] cannot implement `Hash`/`Eq` (it contains
/// `f64`).
///
/// Corresponds to Python `Mapping(subcon, mapping)`.
///
/// # Examples
///
/// ```
/// use construct::constructs::enum_::Mapping;
/// use construct::constructs::format_field::INT8UB;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let mapping = vec![
///     (Value::String("A".to_string()), Value::UInt(0)),
///     (Value::String("B".to_string()), Value::UInt(1)),
/// ];
/// let m = Mapping::new(Box::new(INT8UB), mapping);
///
/// // Parse: 0 → "A"
/// assert_eq!((&m as &dyn Construct).parse_bytes(b"\x00").unwrap(), Value::String("A".to_string()));
/// // Build: "B" → 1
/// assert_eq!((&m as &dyn Construct).build_bytes(&Value::String("B".to_string())).unwrap(), vec![1]);
/// ```
pub struct Mapping {
    /// The inner construct used for reading/writing raw values.
    pub subcon: Box<dyn Construct>,
    /// Forward (encoding) pairs: (build key, build value passed to subcon).
    pub mapping: Vec<(Value, Value)>,
    /// Reverse (decoding) pairs: (parsed value from subcon, return value).
    pub decmapping: Vec<(Value, Value)>,
}

impl Mapping {
    /// Creates a new `Mapping` with the given sub-construct and forward pairs.
    ///
    /// The `mapping` parameter contains (build-key, build-value) pairs.
    /// The reverse (decoding) pairs are built automatically by swapping
    /// each pair.
    ///
    /// # Examples
    ///
    /// ```
    /// use construct::constructs::enum_::Mapping;
    /// use construct::constructs::format_field::INT8UB;
    /// use construct::value::Value;
    ///
    /// let mapping = vec![
    ///     (Value::String("on".to_string()), Value::UInt(1)),
    ///     (Value::String("off".to_string()), Value::UInt(0)),
    /// ];
    /// let m = Mapping::new(Box::new(INT8UB), mapping);
    /// ```
    pub fn new(subcon: Box<dyn Construct>, mapping: Vec<(Value, Value)>) -> Self {
        let decmapping: Vec<(Value, Value)> = mapping
            .iter()
            .map(|(k, v)| (v.clone(), k.clone()))
            .collect();
        Mapping {
            subcon,
            mapping,
            decmapping,
        }
    }

    /// Looks up a value in the encoding map by linear scan.
    ///
    /// Returns the corresponding build value if a key matches, or `None`.
    fn find_encoded(&self, key: &Value) -> Option<&Value> {
        for (k, v) in &self.mapping {
            if k == key {
                return Some(v);
            }
        }
        None
    }

    /// Looks up a value in the decoding map by linear scan.
    ///
    /// Returns the corresponding parse value if a key matches, or `None`.
    fn find_decoded(&self, key: &Value) -> Option<&Value> {
        for (k, v) in &self.decmapping {
            if k == key {
                return Some(v);
            }
        }
        None
    }
}

impl Construct for Mapping {
    /// Parses a value from `stream` using the sub-construct, then translates
    /// it through the decoding map.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::Mapping`] if the parsed value is not found
    /// in the decoding map.
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let obj = self.subcon.parse(stream, ctx)?;
        match self.find_decoded(&obj) {
            Some(decoded) => Ok(decoded.clone()),
            None => Err(ConstructError::Mapping {
                path: String::new(),
                key: format!("{:?}", obj),
            }
            .with_path_prefix("Mapping")),
        }
    }

    /// Builds binary data by translating `data` through the encoding map,
    /// then passing the result to the sub-construct.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::Mapping`] if `data` is not found in the
    /// encoding map.
    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        match self.find_encoded(data) {
            Some(encoded) => self
                .subcon
                .build(encoded, stream, ctx)
                .map_err(|e| e.with_path_prefix("Mapping")),
            None => Err(ConstructError::Mapping {
                path: String::new(),
                key: format!("{:?}", data),
            }
            .with_path_prefix("Mapping")),
        }
    }

    /// Returns the byte size of the sub-construct.
    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        self.subcon
            .sizeof(ctx)
            .map_err(|e| e.with_path_prefix("Mapping"))
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constructs::format_field::{INT16UB, INT8SB, INT8UB};
    use crate::core::error::ConstructError;
    use crate::core::Construct;

    macro_rules! as_dyn {
        ($s:expr) => {
            &$s as &dyn Construct
        };
    }

    // -- Helper: build an IndexMap from a slice of (name, value) pairs -------
    fn make_mapping(pairs: &[(&str, u64)]) -> IndexMap<String, u64> {
        pairs
            .iter()
            .map(|&(name, val)| (name.to_string(), val))
            .collect()
    }

    // ======================================================================
    // Enum tests
    // ======================================================================

    // -- Construction ---------------------------------------------------------

    #[test]
    fn enum_new_builds_decmap() {
        let mapping = make_mapping(&[("one", 1), ("two", 2)]);
        let e = Enum::new(Box::new(INT8UB), mapping);
        assert_eq!(e.decmap.get(&1), Some(&"one".to_string()));
        assert_eq!(e.decmap.get(&2), Some(&"two".to_string()));
        assert_eq!(e.decmap.get(&99), None);
    }

    // -- Parse: mapped values → string --------------------------------------

    #[test]
    fn enum_parse_mapped_returns_string() {
        let e = Enum::new(Box::new(INT8UB), make_mapping(&[("one", 1), ("two", 2)]));
        let result = as_dyn!(e).parse_bytes(b"\x01").unwrap();
        assert_eq!(result, Value::String("one".to_string()));
    }

    #[test]
    fn enum_parse_mapped_returns_correct_label() {
        let e = Enum::new(
            Box::new(INT8UB),
            make_mapping(&[("alpha", 10), ("beta", 20)]),
        );
        let result = as_dyn!(e).parse_bytes(b"\x14").unwrap();
        assert_eq!(result, Value::String("beta".to_string()));
    }

    // -- Parse: unmapped values → raw integer (default mapping) --------------

    #[test]
    fn enum_parse_unmapped_returns_raw_integer() {
        let e = Enum::new(Box::new(INT8UB), make_mapping(&[("one", 1), ("two", 2)]));
        let result = as_dyn!(e).parse_bytes(b"\xFF").unwrap();
        assert_eq!(result, Value::UInt(255));
    }

    #[test]
    fn enum_parse_zero_unmapped_returns_zero() {
        let e = Enum::new(Box::new(INT8UB), make_mapping(&[("one", 1)]));
        let result = as_dyn!(e).parse_bytes(b"\x00").unwrap();
        assert_eq!(result, Value::UInt(0));
    }

    // -- Parse: with wider integer types -------------------------------------

    #[test]
    fn enum_parse_with_int16() {
        let e = Enum::new(Box::new(INT16UB), make_mapping(&[("big", 1000)]));
        let result = as_dyn!(e).parse_bytes(b"\x03\xe8").unwrap();
        assert_eq!(result, Value::String("big".to_string()));
    }

    // -- Parse: error propagation from subcon --------------------------------

    #[test]
    fn enum_parse_insufficient_data_returns_stream_error() {
        let e = Enum::new(Box::new(INT8UB), make_mapping(&[("one", 1)]));
        let err = as_dyn!(e).parse_bytes(b"").unwrap_err();
        assert!(matches!(err, ConstructError::Stream { .. }));
    }

    // -- Build: from string label -------------------------------------------

    #[test]
    fn enum_build_from_string_label() {
        let e = Enum::new(Box::new(INT8UB), make_mapping(&[("one", 1), ("two", 2)]));
        let built = as_dyn!(e)
            .build_bytes(&Value::String("one".to_string()))
            .unwrap();
        assert_eq!(built, vec![1]);
    }

    #[test]
    fn enum_build_from_string_label_two() {
        let e = Enum::new(Box::new(INT8UB), make_mapping(&[("one", 1), ("two", 2)]));
        let built = as_dyn!(e)
            .build_bytes(&Value::String("two".to_string()))
            .unwrap();
        assert_eq!(built, vec![2]);
    }

    // -- Build: from integer (pass-through) ---------------------------------

    #[test]
    fn enum_build_from_uint_passthrough() {
        let e = Enum::new(Box::new(INT8UB), make_mapping(&[("one", 1), ("two", 2)]));
        let built = as_dyn!(e).build_bytes(&Value::UInt(5)).unwrap();
        assert_eq!(built, vec![5]);
    }

    #[test]
    fn enum_build_from_int_passthrough() {
        let e = Enum::new(Box::new(INT8SB), make_mapping(&[("neg", 200)]));
        // Build with a signed integer value that's not in the mapping
        let built = as_dyn!(e).build_bytes(&Value::Int(-10)).unwrap();
        assert_eq!(built, vec![0xF6]); // -10 as u8 = 246 = 0xF6
    }

    // -- Build: unknown string → MappingError -------------------------------

    #[test]
    fn enum_build_unknown_string_returns_mapping_error() {
        let e = Enum::new(Box::new(INT8UB), make_mapping(&[("one", 1)]));
        let err = as_dyn!(e)
            .build_bytes(&Value::String("unknown".to_string()))
            .unwrap_err();
        assert!(matches!(err, ConstructError::Mapping { .. }));
        assert_eq!(err.path(), "(building).Enum");
    }

    // -- Build: error path enrichment ----------------------------------------

    #[test]
    fn enum_build_error_has_enum_path() {
        let e = Enum::new(Box::new(INT8UB), make_mapping(&[("one", 1)]));
        let err = as_dyn!(e)
            .build_bytes(&Value::String("nope".to_string()))
            .unwrap_err();
        assert_eq!(err.path(), "(building).Enum");
    }

    // -- Sizeof --------------------------------------------------------------

    #[test]
    fn enum_sizeof_delegates_to_subcon() {
        let e = Enum::new(Box::new(INT8UB), make_mapping(&[("one", 1)]));
        assert_eq!(e.sizeof(&Context::new()).unwrap(), 1);
    }

    #[test]
    fn enum_sizeof_int16() {
        let e = Enum::new(Box::new(INT16UB), make_mapping(&[("one", 1)]));
        assert_eq!(e.sizeof(&Context::new()).unwrap(), 2);
    }

    // -- Roundtrip: build then parse -----------------------------------------

    #[test]
    fn enum_roundtrip_mapped() {
        let e = Enum::new(Box::new(INT8UB), make_mapping(&[("one", 1), ("two", 2)]));
        let built = as_dyn!(e)
            .build_bytes(&Value::String("one".to_string()))
            .unwrap();
        let parsed = as_dyn!(e).parse_bytes(&built).unwrap();
        assert_eq!(parsed, Value::String("one".to_string()));
    }

    #[test]
    fn enum_roundtrip_unmapped_integer() {
        let e = Enum::new(Box::new(INT8UB), make_mapping(&[("one", 1)]));
        let built = as_dyn!(e).build_bytes(&Value::UInt(42)).unwrap();
        let parsed = as_dyn!(e).parse_bytes(&built).unwrap();
        assert_eq!(parsed, Value::UInt(42));
    }

    // ======================================================================
    // FlagsEnum tests
    // ======================================================================

    // -- Construction ---------------------------------------------------------

    #[test]
    fn flags_enum_new_stores_flags() {
        let flags = make_mapping(&[("read", 1), ("write", 2)]);
        let fe = FlagsEnum::new(Box::new(INT8UB), flags);
        assert_eq!(fe.flags.len(), 2);
        assert_eq!(fe.flags.get("read"), Some(&1));
        assert_eq!(fe.flags.get("write"), Some(&2));
    }

    // -- Parse: basic flag expansion -----------------------------------------

    #[test]
    fn flags_enum_parse_all_set() {
        let fe = FlagsEnum::new(
            Box::new(INT8UB),
            make_mapping(&[("one", 1), ("two", 2), ("four", 4)]),
        );
        let parsed = as_dyn!(fe).parse_bytes(b"\x07").unwrap();
        let container = parsed.as_container().unwrap();
        assert_eq!(container.get("one").unwrap(), &Value::Bool(true));
        assert_eq!(container.get("two").unwrap(), &Value::Bool(true));
        assert_eq!(container.get("four").unwrap(), &Value::Bool(true));
    }

    #[test]
    fn flags_enum_parse_none_set() {
        let fe = FlagsEnum::new(
            Box::new(INT8UB),
            make_mapping(&[("one", 1), ("two", 2), ("four", 4)]),
        );
        let parsed = as_dyn!(fe).parse_bytes(b"\x00").unwrap();
        let container = parsed.as_container().unwrap();
        assert_eq!(container.get("one").unwrap(), &Value::Bool(false));
        assert_eq!(container.get("two").unwrap(), &Value::Bool(false));
        assert_eq!(container.get("four").unwrap(), &Value::Bool(false));
    }

    #[test]
    fn flags_enum_parse_partial() {
        let fe = FlagsEnum::new(
            Box::new(INT8UB),
            make_mapping(&[("one", 1), ("two", 2), ("four", 4)]),
        );
        let parsed = as_dyn!(fe).parse_bytes(b"\x05").unwrap(); // 0b101 = one + four
        let container = parsed.as_container().unwrap();
        assert_eq!(container.get("one").unwrap(), &Value::Bool(true));
        assert_eq!(container.get("two").unwrap(), &Value::Bool(false));
        assert_eq!(container.get("four").unwrap(), &Value::Bool(true));
    }

    // -- Parse: preserves insertion order of flags ----------------------------

    #[test]
    fn flags_enum_parse_preserves_order() {
        let fe = FlagsEnum::new(
            Box::new(INT8UB),
            make_mapping(&[("alpha", 1), ("beta", 2), ("gamma", 4)]),
        );
        let parsed = as_dyn!(fe).parse_bytes(b"\x03").unwrap();
        let container = parsed.as_container().unwrap();
        let keys: Vec<&String> = container.keys().collect();
        assert_eq!(keys[0], "alpha");
        assert_eq!(keys[1], "beta");
        assert_eq!(keys[2], "gamma");
    }

    // -- Parse: extra bits beyond defined flags are ignored -------------------

    #[test]
    fn flags_enum_parse_extra_bits_ignored() {
        let fe = FlagsEnum::new(Box::new(INT8UB), make_mapping(&[("one", 1), ("two", 2)]));
        // 0xFF has many bits set but only flags 1 and 2 are defined
        let parsed = as_dyn!(fe).parse_bytes(b"\xFF").unwrap();
        let container = parsed.as_container().unwrap();
        assert_eq!(container.get("one").unwrap(), &Value::Bool(true));
        assert_eq!(container.get("two").unwrap(), &Value::Bool(true));
    }

    // -- Build: from container of bools -------------------------------------

    #[test]
    fn flags_enum_build_basic() {
        let fe = FlagsEnum::new(
            Box::new(INT8UB),
            make_mapping(&[("one", 1), ("two", 2), ("four", 4)]),
        );
        let mut container = IndexMap::new();
        container.insert("one".to_string(), Value::Bool(true));
        container.insert("two".to_string(), Value::Bool(true));
        container.insert("four".to_string(), Value::Bool(false));
        let built = as_dyn!(fe)
            .build_bytes(&Value::Container(container))
            .unwrap();
        assert_eq!(built, vec![3]); // 1 | 2 = 3
    }

    #[test]
    fn flags_enum_build_all_false() {
        let fe = FlagsEnum::new(Box::new(INT8UB), make_mapping(&[("one", 1), ("two", 2)]));
        let mut container = IndexMap::new();
        container.insert("one".to_string(), Value::Bool(false));
        container.insert("two".to_string(), Value::Bool(false));
        let built = as_dyn!(fe)
            .build_bytes(&Value::Container(container))
            .unwrap();
        assert_eq!(built, vec![0]);
    }

    #[test]
    fn flags_enum_build_all_true() {
        let fe = FlagsEnum::new(
            Box::new(INT8UB),
            make_mapping(&[("one", 1), ("two", 2), ("four", 4)]),
        );
        let mut container = IndexMap::new();
        container.insert("one".to_string(), Value::Bool(true));
        container.insert("two".to_string(), Value::Bool(true));
        container.insert("four".to_string(), Value::Bool(true));
        let built = as_dyn!(fe)
            .build_bytes(&Value::Container(container))
            .unwrap();
        assert_eq!(built, vec![7]); // 1 | 2 | 4 = 7
    }

    // -- Build: skips underscore-prefixed keys --------------------------------

    #[test]
    fn flags_enum_build_skips_underscore_keys() {
        let fe = FlagsEnum::new(Box::new(INT8UB), make_mapping(&[("one", 1), ("two", 2)]));
        let mut container = IndexMap::new();
        container.insert("one".to_string(), Value::Bool(true));
        container.insert("_flagsenum".to_string(), Value::Bool(true));
        container.insert("two".to_string(), Value::Bool(false));
        let built = as_dyn!(fe)
            .build_bytes(&Value::Container(container))
            .unwrap();
        assert_eq!(built, vec![1]); // only "one" counted, _flagsenum skipped
    }

    // -- Build: unknown flag name → MappingError ----------------------------

    #[test]
    fn flags_enum_build_unknown_flag_returns_mapping_error() {
        let fe = FlagsEnum::new(Box::new(INT8UB), make_mapping(&[("one", 1)]));
        let mut container = IndexMap::new();
        container.insert("one".to_string(), Value::Bool(true));
        container.insert("unknown".to_string(), Value::Bool(true));
        let err = as_dyn!(fe)
            .build_bytes(&Value::Container(container))
            .unwrap_err();
        assert!(matches!(err, ConstructError::Mapping { .. }));
        assert_eq!(err.path(), "(building).FlagsEnum");
    }

    // -- Build: non-container input → TypeMismatch --------------------------

    #[test]
    fn flags_enum_build_non_container_returns_type_mismatch() {
        let fe = FlagsEnum::new(Box::new(INT8UB), make_mapping(&[("one", 1)]));
        let err = as_dyn!(fe).build_bytes(&Value::UInt(3)).unwrap_err();
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
        assert_eq!(err.path(), "(building).FlagsEnum");
    }

    // -- Sizeof --------------------------------------------------------------

    #[test]
    fn flags_enum_sizeof_delegates_to_subcon() {
        let fe = FlagsEnum::new(Box::new(INT8UB), make_mapping(&[("one", 1)]));
        assert_eq!(fe.sizeof(&Context::new()).unwrap(), 1);
    }

    // -- Roundtrip -----------------------------------------------------------

    #[test]
    fn flags_enum_roundtrip() {
        let fe = FlagsEnum::new(
            Box::new(INT8UB),
            make_mapping(&[("one", 1), ("two", 2), ("four", 4)]),
        );

        let mut container = IndexMap::new();
        container.insert("one".to_string(), Value::Bool(true));
        container.insert("two".to_string(), Value::Bool(false));
        container.insert("four".to_string(), Value::Bool(true));

        let built = as_dyn!(fe)
            .build_bytes(&Value::Container(container))
            .unwrap();
        let parsed = as_dyn!(fe).parse_bytes(&built).unwrap();
        let parsed_container = parsed.as_container().unwrap();

        assert_eq!(parsed_container.get("one").unwrap(), &Value::Bool(true));
        assert_eq!(parsed_container.get("two").unwrap(), &Value::Bool(false));
        assert_eq!(parsed_container.get("four").unwrap(), &Value::Bool(true));
    }

    // ======================================================================
    // Mapping tests
    // ======================================================================

    // -- Construction ---------------------------------------------------------

    #[test]
    fn mapping_new_builds_decmap() {
        let mapping = vec![
            (Value::String("A".to_string()), Value::UInt(0)),
            (Value::String("B".to_string()), Value::UInt(1)),
        ];
        let m = Mapping::new(Box::new(INT8UB), mapping);

        // decmapping: (UInt(0), String("A")), (UInt(1), String("B"))
        assert_eq!(
            m.find_decoded(&Value::UInt(0)),
            Some(&Value::String("A".to_string()))
        );
        assert_eq!(
            m.find_decoded(&Value::UInt(1)),
            Some(&Value::String("B".to_string()))
        );
    }

    // -- Parse: mapped values ------------------------------------------------

    #[test]
    fn mapping_parse_returns_decoded_value() {
        let mapping = vec![
            (Value::String("A".to_string()), Value::UInt(0)),
            (Value::String("B".to_string()), Value::UInt(1)),
        ];
        let m = Mapping::new(Box::new(INT8UB), mapping);

        let result = as_dyn!(m).parse_bytes(b"\x00").unwrap();
        assert_eq!(result, Value::String("A".to_string()));
    }

    #[test]
    fn mapping_parse_second_entry() {
        let mapping = vec![
            (Value::String("A".to_string()), Value::UInt(0)),
            (Value::String("B".to_string()), Value::UInt(1)),
        ];
        let m = Mapping::new(Box::new(INT8UB), mapping);

        let result = as_dyn!(m).parse_bytes(b"\x01").unwrap();
        assert_eq!(result, Value::String("B".to_string()));
    }

    // -- Parse: unmapped → MappingError -------------------------------------

    #[test]
    fn mapping_parse_unmapped_returns_mapping_error() {
        let mapping = vec![(Value::String("A".to_string()), Value::UInt(0))];
        let m = Mapping::new(Box::new(INT8UB), mapping);

        let err = as_dyn!(m).parse_bytes(b"\xFF").unwrap_err();
        assert!(matches!(err, ConstructError::Mapping { .. }));
        assert_eq!(err.path(), "(parsing).Mapping");
    }

    // -- Build: from mapped key ----------------------------------------------

    #[test]
    fn mapping_build_from_key() {
        let mapping = vec![
            (Value::String("A".to_string()), Value::UInt(0)),
            (Value::String("B".to_string()), Value::UInt(1)),
        ];
        let m = Mapping::new(Box::new(INT8UB), mapping);

        let built = as_dyn!(m)
            .build_bytes(&Value::String("A".to_string()))
            .unwrap();
        assert_eq!(built, vec![0]);
    }

    #[test]
    fn mapping_build_from_second_key() {
        let mapping = vec![
            (Value::String("A".to_string()), Value::UInt(0)),
            (Value::String("B".to_string()), Value::UInt(1)),
        ];
        let m = Mapping::new(Box::new(INT8UB), mapping);

        let built = as_dyn!(m)
            .build_bytes(&Value::String("B".to_string()))
            .unwrap();
        assert_eq!(built, vec![1]);
    }

    // -- Build: unmapped key → MappingError ---------------------------------

    #[test]
    fn mapping_build_unmapped_returns_mapping_error() {
        let mapping = vec![(Value::String("A".to_string()), Value::UInt(0))];
        let m = Mapping::new(Box::new(INT8UB), mapping);

        let err = as_dyn!(m)
            .build_bytes(&Value::String("Z".to_string()))
            .unwrap_err();
        assert!(matches!(err, ConstructError::Mapping { .. }));
        assert_eq!(err.path(), "(building).Mapping");
    }

    // -- Build: subcon error propagation ------------------------------------

    #[test]
    fn mapping_build_subcon_error_propagates() {
        let mapping = vec![(Value::String("A".to_string()), Value::UInt(256))];
        let m = Mapping::new(Box::new(INT8UB), mapping);

        // 256 doesn't fit in u8 → FormatField error
        let err = as_dyn!(m)
            .build_bytes(&Value::String("A".to_string()))
            .unwrap_err();
        assert!(matches!(err, ConstructError::FormatField { .. }));
        assert_eq!(err.path(), "(building).Mapping");
    }

    // -- Sizeof --------------------------------------------------------------

    #[test]
    fn mapping_sizeof_delegates_to_subcon() {
        let mapping = vec![(Value::String("A".to_string()), Value::UInt(0))];
        let m = Mapping::new(Box::new(INT8UB), mapping);
        assert_eq!(m.sizeof(&Context::new()).unwrap(), 1);
    }

    #[test]
    fn mapping_sizeof_int16() {
        let mapping = vec![(Value::String("A".to_string()), Value::UInt(0))];
        let m = Mapping::new(Box::new(INT16UB), mapping);
        assert_eq!(m.sizeof(&Context::new()).unwrap(), 2);
    }

    // -- Roundtrip -----------------------------------------------------------

    #[test]
    fn mapping_roundtrip() {
        let mapping = vec![
            (Value::String("A".to_string()), Value::UInt(0)),
            (Value::String("B".to_string()), Value::UInt(1)),
        ];
        let m = Mapping::new(Box::new(INT8UB), mapping);

        let built = as_dyn!(m)
            .build_bytes(&Value::String("B".to_string()))
            .unwrap();
        let parsed = as_dyn!(m).parse_bytes(&built).unwrap();
        assert_eq!(parsed, Value::String("B".to_string()));
    }

    // -- Non-string key/value mapping ----------------------------------------

    #[test]
    fn mapping_with_integer_keys() {
        let mapping = vec![
            (Value::Int(10), Value::UInt(100)),
            (Value::Int(20), Value::UInt(200)),
        ];
        let m = Mapping::new(Box::new(INT8UB), mapping);

        // Parse: UInt(100) → Int(10)
        let parsed = as_dyn!(m).parse_bytes(b"\x64").unwrap();
        assert_eq!(parsed, Value::Int(10));

        // Build: Int(20) → UInt(200)
        let built = as_dyn!(m).build_bytes(&Value::Int(20)).unwrap();
        assert_eq!(built, vec![200]);
    }

    // -- Empty mapping (always fails) ----------------------------------------

    #[test]
    fn mapping_empty_parse_fails() {
        let m = Mapping::new(Box::new(INT8UB), vec![]);
        let err = as_dyn!(m).parse_bytes(b"\x00").unwrap_err();
        assert!(matches!(err, ConstructError::Mapping { .. }));
    }

    #[test]
    fn mapping_empty_build_fails() {
        let m = Mapping::new(Box::new(INT8UB), vec![]);
        let err = as_dyn!(m).build_bytes(&Value::UInt(0)).unwrap_err();
        assert!(matches!(err, ConstructError::Mapping { .. }));
    }
}
