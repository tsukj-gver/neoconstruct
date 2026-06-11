//! Value type system — the dynamically-typed data representation.
//!
//! This module defines the [`Value`] enum that replaces Python's dynamic
//! typing for all construct operations. Every construct's `parse` returns a
//! `Value` and every `build` consumes one.
//!
//! See `docs/模块设计-值类型系统.md` for the full design rationale and the
//! mapping between Python types and [`Value`] variants.

use std::fmt;

use indexmap::IndexMap;

use crate::core::error::ConstructError;

/// The number of leading bytes shown before a `Bytes` value is truncated in
/// its [`Display`](fmt::Display) output. Matches the Python original's
/// `printingcap` for `bytes`.
const BYTES_DISPLAY_CAP: usize = 16;

/// The number of leading characters shown before a `String` value is
/// truncated in its [`Display`](fmt::Display) output. Matches the Python
/// original's `printingcap` for `str`.
const STRING_DISPLAY_CAP: usize = 32;

/// The indentation step (in spaces) used when pretty-printing nested
/// [`Value::List`] and [`Value::Container`] values. Matches the Python
/// original's 4-space `indentation`.
const DISPLAY_INDENT_STEP: usize = 4;

/// The dynamically-typed value returned by `parse` and consumed by `build`.
///
/// Each variant corresponds to a Python type as documented in the module
/// design. The three integer variants ([`Int`](Value::Int),
/// [`UInt`](Value::UInt), [`BigInt`](Value::BigInt)) together cover Python's
/// single `int` type; cross-integer conversions are available via
/// [`to_u64`](Value::to_u64), [`to_i64`](Value::to_i64) and
/// [`to_f64`](Value::to_f64).
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// The absence of a value. Corresponds to Python `None`.
    None,
    /// A boolean. Corresponds to Python `bool`.
    Bool(bool),
    /// A signed 64-bit integer. Corresponds to Python `int` for values that
    /// fit in `i64`.
    Int(i64),
    /// An unsigned 64-bit integer. Corresponds to Python `int` for
    /// non-negative values up to `u64::MAX`.
    UInt(u64),
    /// A signed 128-bit integer. Corresponds to Python `int` for large
    /// values that exceed the `i64` range.
    BigInt(i128),
    /// A 64-bit floating point number. Corresponds to Python `float`.
    Float(f64),
    /// A byte string. Corresponds to Python `bytes`.
    Bytes(Vec<u8>),
    /// A UTF-8 string. Corresponds to Python `str`.
    String(String),
    /// An ordered list of values. Corresponds to Python `list` /
    /// `ListContainer`.
    List(Vec<Value>),
    /// An insertion-ordered map of named values. Corresponds to Python
    /// `dict` / `Container`. Uses [`IndexMap`] so that iteration order matches
    /// insertion order while retaining `O(1)` key lookup.
    Container(IndexMap<String, Value>),
}

impl Value {
    /// Returns the variant name of this value, e.g. `"Int"` or `"Container"`.
    ///
    /// This is used internally to populate the `actual` field of
    /// [`ConstructError::TypeMismatch`] errors.
    #[must_use]
    fn type_name(&self) -> &'static str {
        match self {
            Value::None => "None",
            Value::Bool(_) => "Bool",
            Value::Int(_) => "Int",
            Value::UInt(_) => "UInt",
            Value::BigInt(_) => "BigInt",
            Value::Float(_) => "Float",
            Value::Bytes(_) => "Bytes",
            Value::String(_) => "String",
            Value::List(_) => "List",
            Value::Container(_) => "Container",
        }
    }

    // -- Type query methods -----------------------------------------------

    /// Returns `true` if this value is [`Value::None`].
    #[must_use]
    pub fn is_none(&self) -> bool {
        matches!(self, Value::None)
    }

    /// Returns `true` if this value is [`Value::Bool`].
    #[must_use]
    pub fn is_bool(&self) -> bool {
        matches!(self, Value::Bool(_))
    }

    /// Returns `true` if this value is [`Value::Int`].
    #[must_use]
    pub fn is_int(&self) -> bool {
        matches!(self, Value::Int(_))
    }

    /// Returns `true` if this value is [`Value::UInt`].
    #[must_use]
    pub fn is_uint(&self) -> bool {
        matches!(self, Value::UInt(_))
    }

    /// Returns `true` if this value is [`Value::Float`].
    #[must_use]
    pub fn is_float(&self) -> bool {
        matches!(self, Value::Float(_))
    }

    /// Returns `true` if this value is [`Value::Bytes`].
    #[must_use]
    pub fn is_bytes(&self) -> bool {
        matches!(self, Value::Bytes(_))
    }

    /// Returns `true` if this value is [`Value::String`].
    #[must_use]
    pub fn is_string(&self) -> bool {
        matches!(self, Value::String(_))
    }

    /// Returns `true` if this value is [`Value::List`].
    #[must_use]
    pub fn is_list(&self) -> bool {
        matches!(self, Value::List(_))
    }

    /// Returns `true` if this value is [`Value::Container`].
    #[must_use]
    pub fn is_container(&self) -> bool {
        matches!(self, Value::Container(_))
    }

    // -- Type conversion methods ------------------------------------------

    /// Returns the contained `bool` if this is [`Value::Bool`].
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::TypeMismatch`] if this value is not a
    /// [`Value::Bool`].
    pub fn as_bool(&self) -> Result<bool, ConstructError> {
        match self {
            Value::Bool(b) => Ok(*b),
            other => Err(type_mismatch(other, "Bool")),
        }
    }

    /// Returns the contained `i64` if this is [`Value::Int`].
    ///
    /// For cross-integer-type conversions use [`to_i64`](Value::to_i64).
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::TypeMismatch`] if this value is not a
    /// [`Value::Int`].
    pub fn as_int(&self) -> Result<i64, ConstructError> {
        match self {
            Value::Int(i) => Ok(*i),
            other => Err(type_mismatch(other, "Int")),
        }
    }

    /// Returns the contained `u64` if this is [`Value::UInt`].
    ///
    /// For cross-integer-type conversions use [`to_u64`](Value::to_u64).
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::TypeMismatch`] if this value is not a
    /// [`Value::UInt`].
    pub fn as_uint(&self) -> Result<u64, ConstructError> {
        match self {
            Value::UInt(u) => Ok(*u),
            other => Err(type_mismatch(other, "UInt")),
        }
    }

    /// Returns the contained `f64` if this is [`Value::Float`].
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::TypeMismatch`] if this value is not a
    /// [`Value::Float`].
    pub fn as_float(&self) -> Result<f64, ConstructError> {
        match self {
            Value::Float(f) => Ok(*f),
            other => Err(type_mismatch(other, "Float")),
        }
    }

    /// Returns the contained bytes as a slice if this is [`Value::Bytes`].
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::TypeMismatch`] if this value is not a
    /// [`Value::Bytes`].
    pub fn as_bytes(&self) -> Result<&[u8], ConstructError> {
        match self {
            Value::Bytes(b) => Ok(b),
            other => Err(type_mismatch(other, "Bytes")),
        }
    }

    /// Returns a mutable reference to the contained `Vec<u8>` if this is
    /// [`Value::Bytes`].
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::TypeMismatch`] if this value is not a
    /// [`Value::Bytes`].
    pub fn as_bytes_mut(&mut self) -> Result<&mut Vec<u8>, ConstructError> {
        match self {
            Value::Bytes(b) => Ok(b),
            other => Err(type_mismatch(other, "Bytes")),
        }
    }

    /// Returns the contained string as a `&str` if this is [`Value::String`].
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::TypeMismatch`] if this value is not a
    /// [`Value::String`].
    pub fn as_string(&self) -> Result<&str, ConstructError> {
        match self {
            Value::String(s) => Ok(s),
            other => Err(type_mismatch(other, "String")),
        }
    }

    /// Returns a reference to the contained list if this is [`Value::List`].
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::TypeMismatch`] if this value is not a
    /// [`Value::List`].
    pub fn as_list(&self) -> Result<&Vec<Value>, ConstructError> {
        match self {
            Value::List(list) => Ok(list),
            other => Err(type_mismatch(other, "List")),
        }
    }

    /// Returns a mutable reference to the contained list if this is
    /// [`Value::List`].
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::TypeMismatch`] if this value is not a
    /// [`Value::List`].
    pub fn as_list_mut(&mut self) -> Result<&mut Vec<Value>, ConstructError> {
        match self {
            Value::List(list) => Ok(list),
            other => Err(type_mismatch(other, "List")),
        }
    }

    /// Returns a reference to the contained map if this is
    /// [`Value::Container`].
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::TypeMismatch`] if this value is not a
    /// [`Value::Container`].
    pub fn as_container(&self) -> Result<&IndexMap<String, Value>, ConstructError> {
        match self {
            Value::Container(map) => Ok(map),
            other => Err(type_mismatch(other, "Container")),
        }
    }

    /// Returns a mutable reference to the contained map if this is
    /// [`Value::Container`].
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::TypeMismatch`] if this value is not a
    /// [`Value::Container`].
    pub fn as_container_mut(&mut self) -> Result<&mut IndexMap<String, Value>, ConstructError> {
        match self {
            Value::Container(map) => Ok(map),
            other => Err(type_mismatch(other, "Container")),
        }
    }

    // -- Integer / numeric cross-type conversions -------------------------

    /// Returns the `u64` representation of any integer variant
    /// ([`Int`](Value::Int), [`UInt`](Value::UInt),
    /// [`BigInt`](Value::BigInt)).
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::TypeMismatch`] if this value is not an
    /// integer, or [`ConstructError::Generic`] if the integer is negative or
    /// exceeds `u64::MAX`.
    pub fn to_u64(&self) -> Result<u64, ConstructError> {
        match self {
            Value::UInt(u) => Ok(*u),
            Value::Int(i) => {
                if *i >= 0 {
                    Ok(*i as u64)
                } else {
                    Err(out_of_range(*i as i128, "u64"))
                }
            }
            Value::BigInt(i) => {
                if (0..=u64::MAX as i128).contains(i) {
                    Ok(*i as u64)
                } else {
                    Err(out_of_range(*i, "u64"))
                }
            }
            other => Err(type_mismatch(other, "integer")),
        }
    }

    /// Returns the `i64` representation of any integer variant
    /// ([`Int`](Value::Int), [`UInt`](Value::UInt),
    /// [`BigInt`](Value::BigInt)).
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::TypeMismatch`] if this value is not an
    /// integer, or [`ConstructError::Generic`] if the integer falls outside
    /// the `i64` range.
    pub fn to_i64(&self) -> Result<i64, ConstructError> {
        match self {
            Value::Int(i) => Ok(*i),
            Value::UInt(u) => {
                if *u <= i64::MAX as u64 {
                    Ok(*u as i64)
                } else {
                    Err(out_of_range(*u as i128, "i64"))
                }
            }
            Value::BigInt(i) => {
                if (i64::MIN as i128..=i64::MAX as i128).contains(i) {
                    Ok(*i as i64)
                } else {
                    Err(out_of_range(*i, "i64"))
                }
            }
            other => Err(type_mismatch(other, "integer")),
        }
    }

    /// Returns the `f64` representation of any numeric variant
    /// ([`Int`](Value::Int), [`UInt`](Value::UInt),
    /// [`BigInt`](Value::BigInt), [`Float`](Value::Float)).
    ///
    /// Note that converting a large integer to `f64` may lose precision; this
    /// is inherent to floating-point representation and matches the behaviour
    /// of the Python original where `float(big_int)` is also lossy.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::TypeMismatch`] if this value is not numeric.
    pub fn to_f64(&self) -> Result<f64, ConstructError> {
        match self {
            Value::Float(f) => Ok(*f),
            Value::Int(i) => Ok(*i as f64),
            Value::UInt(u) => Ok(*u as f64),
            Value::BigInt(i) => Ok(*i as f64),
            other => Err(type_mismatch(other, "numeric")),
        }
    }

    // -- Container convenience access -------------------------------------

    /// Looks up `key` in a [`Value::Container`].
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::TypeMismatch`] if this value is not a
    /// [`Value::Container`], or [`ConstructError::FieldMissing`] if `key` is
    /// not present.
    pub fn get(&self, key: &str) -> Result<&Value, ConstructError> {
        match self {
            Value::Container(map) => map.get(key).ok_or_else(|| ConstructError::FieldMissing {
                path: String::new(),
                field: key.to_string(),
            }),
            other => Err(type_mismatch(other, "Container")),
        }
    }

    /// Looks up a dotted `path` (e.g. `"header.magic"`) inside nested
    /// [`Value::Container`] values.
    ///
    /// Each segment is looked up with the same semantics as [`get`](Self::get).
    /// If a lookup fails at an intermediate level, the error's path is
    /// enriched with the segments traversed so far.
    ///
    /// # Errors
    ///
    /// Propagates any error from [`get`](Self::get), with the traversed prefix
    /// prepended to the error path.
    pub fn get_path(&self, path: &str) -> Result<&Value, ConstructError> {
        let mut current = self;
        let mut traversed = String::new();
        for key in path.split('.') {
            current = match current.get(key) {
                Ok(value) => value,
                Err(err) => {
                    return Err(if traversed.is_empty() {
                        err
                    } else {
                        err.with_path_prefix(&traversed)
                    });
                }
            };
            if !traversed.is_empty() {
                traversed.push('.');
            }
            traversed.push_str(key);
        }
        Ok(current)
    }
}

/// Builds a [`ConstructError::TypeMismatch`] describing that `other` was found
/// where `expected` was required. The path is left empty; callers further up
/// the construct tree enrich it via [`ConstructError::with_path_prefix`].
fn type_mismatch(other: &Value, expected: &str) -> ConstructError {
    ConstructError::TypeMismatch {
        path: String::new(),
        expected: expected.to_string(),
        actual: other.type_name().to_string(),
    }
}

/// Builds a [`ConstructError::Generic`] describing that `value` does not fit
/// in the integer type named `target`.
fn out_of_range(value: i128, target: &str) -> ConstructError {
    ConstructError::Generic {
        path: String::new(),
        message: format!("value {value} out of range for {target}"),
    }
}

// -- From implementations -------------------------------------------------

impl From<bool> for Value {
    /// Converts a `bool` into a [`Value::Bool`].
    fn from(value: bool) -> Self {
        Value::Bool(value)
    }
}

impl From<i8> for Value {
    /// Converts a signed 8-bit integer into a [`Value::Int`].
    fn from(value: i8) -> Self {
        Value::Int(i64::from(value))
    }
}

impl From<i16> for Value {
    /// Converts a signed 16-bit integer into a [`Value::Int`].
    fn from(value: i16) -> Self {
        Value::Int(i64::from(value))
    }
}

impl From<i32> for Value {
    /// Converts a signed 32-bit integer into a [`Value::Int`].
    fn from(value: i32) -> Self {
        Value::Int(i64::from(value))
    }
}

impl From<i64> for Value {
    /// Converts a signed 64-bit integer into a [`Value::Int`].
    fn from(value: i64) -> Self {
        Value::Int(value)
    }
}

impl From<u8> for Value {
    /// Converts an unsigned 8-bit integer into a [`Value::UInt`].
    fn from(value: u8) -> Self {
        Value::UInt(u64::from(value))
    }
}

impl From<u16> for Value {
    /// Converts an unsigned 16-bit integer into a [`Value::UInt`].
    fn from(value: u16) -> Self {
        Value::UInt(u64::from(value))
    }
}

impl From<u32> for Value {
    /// Converts an unsigned 32-bit integer into a [`Value::UInt`].
    fn from(value: u32) -> Self {
        Value::UInt(u64::from(value))
    }
}

impl From<u64> for Value {
    /// Converts an unsigned 64-bit integer into a [`Value::UInt`].
    fn from(value: u64) -> Self {
        Value::UInt(value)
    }
}

impl From<f32> for Value {
    /// Converts a 32-bit float into a [`Value::Float`].
    fn from(value: f32) -> Self {
        Value::Float(f64::from(value))
    }
}

impl From<f64> for Value {
    /// Converts a 64-bit float into a [`Value::Float`].
    fn from(value: f64) -> Self {
        Value::Float(value)
    }
}

impl From<Vec<u8>> for Value {
    /// Converts a `Vec<u8>` into a [`Value::Bytes`].
    fn from(value: Vec<u8>) -> Self {
        Value::Bytes(value)
    }
}

impl From<String> for Value {
    /// Converts a [`String`] into a [`Value::String`].
    fn from(value: String) -> Self {
        Value::String(value)
    }
}

impl From<&str> for Value {
    /// Converts a `&str` into a [`Value::String`].
    fn from(value: &str) -> Self {
        Value::String(value.to_string())
    }
}

// -- Display implementation -----------------------------------------------

impl fmt::Display for Value {
    /// Pretty-prints the value recursively, mirroring the Python original's
    /// `Container.__str__` / `ListContainer.__str__` multi-line layout.
    ///
    /// Scalars use a compact representation; [`Value::List`] and
    /// [`Value::Container`] expand to one entry per line with 4-space
    /// indentation per nesting level. Bytes and strings are truncated past
    /// [`BYTES_DISPLAY_CAP`] / [`STRING_DISPLAY_CAP`] characters respectively.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", value_to_display(self, 0))
    }
}

/// Recursively builds the display string for `value` at the given indentation
/// level (counted in [`DISPLAY_INDENT_STEP`] groups).
fn value_to_display(value: &Value, indent: usize) -> String {
    match value {
        Value::None => "None".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::UInt(u) => u.to_string(),
        Value::BigInt(i) => i.to_string(),
        Value::Float(number) => number.to_string(),
        Value::Bytes(bytes) => format_bytes(bytes),
        Value::String(string) => format_string(string),
        Value::List(list) => format_list(list, indent),
        Value::Container(map) => format_container(map, indent),
    }
}

/// Formats a [`Value::Container`] with one `key = value` entry per line.
fn format_container(map: &IndexMap<String, Value>, indent: usize) -> String {
    let inner = indent + 1;
    let pad = pad(inner);
    let mut out = String::from("Container:");
    for (key, val) in map {
        let rendered = value_to_display(val, 0);
        out.push('\n');
        out.push_str(&pad);
        out.push_str(key);
        out.push_str(" = ");
        out.push_str(&indent_lines(&rendered, &pad));
    }
    out
}

/// Formats a [`Value::List`] with one element per line.
fn format_list(list: &[Value], indent: usize) -> String {
    let inner = indent + 1;
    let pad = pad(inner);
    let mut out = String::from("List:");
    for val in list {
        let rendered = value_to_display(val, 0);
        out.push('\n');
        out.push_str(&pad);
        out.push_str(&indent_lines(&rendered, &pad));
    }
    out
}

/// Produces the leading-space padding string for the given indentation level.
fn indent(level: usize) -> String {
    " ".repeat(level * DISPLAY_INDENT_STEP)
}

/// Alias for [`indent`] used at call sites for clarity.
fn pad(level: usize) -> String {
    indent(level)
}

/// Indents every line of `s` after the first by `pad`, leaving the first line
/// untouched. This mirrors the Python original's
/// `indentation.join(value_to_string(v).split("\n"))` behaviour.
fn indent_lines(s: &str, pad: &str) -> String {
    let mut result = String::new();
    for (i, line) in s.lines().enumerate() {
        if i > 0 {
            result.push('\n');
            result.push_str(pad);
        }
        result.push_str(line);
    }
    result
}

/// Formats a byte string in a Python-like `b'...'` literal, truncating it to
/// [`BYTES_DISPLAY_CAP`] bytes and appending the total length.
fn format_bytes(bytes: &[u8]) -> String {
    let total = bytes.len();
    let (shown, truncated) = if total <= BYTES_DISPLAY_CAP {
        (bytes, false)
    } else {
        (&bytes[..BYTES_DISPLAY_CAP], true)
    };
    let mut out = String::from("b'");
    for &byte in shown {
        match byte {
            b'\\' => out.push_str("\\\\"),
            b'\'' => out.push_str("\\'"),
            byte if byte.is_ascii_graphic() || byte == b' ' => out.push(byte as char),
            byte => out.push_str(&format!("\\x{byte:02x}")),
        }
    }
    out.push('\'');
    if truncated {
        format!("{out}... (truncated, total {total})")
    } else {
        format!("{out} (total {total})")
    }
}

/// Formats a string in a Python-like `'...'` literal, truncating it to
/// [`STRING_DISPLAY_CAP`] characters and appending the total length.
fn format_string(string: &str) -> String {
    let total = string.chars().count();
    let (shown, truncated) = if total <= STRING_DISPLAY_CAP {
        (string.to_string(), false)
    } else {
        let truncated_text: String = string.chars().take(STRING_DISPLAY_CAP).collect();
        (truncated_text, true)
    };
    let out = format!("'{shown}'");
    if truncated {
        format!("{out}... (truncated, total {total})")
    } else {
        format!("{out} (total {total})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    // -- From implementations ---------------------------------------------

    #[test]
    fn from_bool() {
        assert_eq!(Value::from(true), Value::Bool(true));
        assert_eq!(Value::from(false), Value::Bool(false));
    }

    #[test]
    fn from_signed_integers_produce_int() {
        assert_eq!(Value::from(-1_i8), Value::Int(-1));
        assert_eq!(Value::from(-1_i16), Value::Int(-1));
        assert_eq!(Value::from(-1_i32), Value::Int(-1));
        assert_eq!(Value::from(-1_i64), Value::Int(-1));
        assert_eq!(Value::from(0_i32), Value::Int(0));
        assert_eq!(Value::from(i64::MAX), Value::Int(i64::MAX));
    }

    #[test]
    fn from_unsigned_integers_produce_uint() {
        assert_eq!(Value::from(0_u8), Value::UInt(0));
        assert_eq!(Value::from(1_u16), Value::UInt(1));
        assert_eq!(Value::from(1_u32), Value::UInt(1));
        assert_eq!(Value::from(u64::MAX), Value::UInt(u64::MAX));
    }

    #[test]
    fn from_floats_produce_float() {
        assert_eq!(Value::from(1.5_f32), Value::Float(1.5));
        assert_eq!(Value::from(1.5_f64), Value::Float(1.5));
    }

    #[test]
    fn from_bytes_string_str() {
        assert_eq!(Value::from(vec![1u8, 2, 3]), Value::Bytes(vec![1, 2, 3]));
        assert_eq!(
            Value::from(String::from("hi")),
            Value::String(String::from("hi"))
        );
        assert_eq!(Value::from("hi"), Value::String(String::from("hi")));
        assert_eq!(Value::from(""), Value::String(String::new()));
    }

    // -- Type query methods -----------------------------------------------

    #[test]
    fn is_query_methods() {
        assert!(Value::None.is_none());
        assert!(Value::Bool(true).is_bool());
        assert!(Value::Int(0).is_int());
        assert!(Value::UInt(0).is_uint());
        assert!(Value::Float(0.0).is_float());
        assert!(Value::Bytes(vec![]).is_bytes());
        assert!(Value::String(String::new()).is_string());
        assert!(Value::List(vec![]).is_list());

        let map = IndexMap::new();
        assert!(Value::Container(map).is_container());

        // cross checks: only one returns true
        assert!(!Value::Int(0).is_bool());
        assert!(!Value::UInt(0).is_int());
        assert!(!Value::BigInt(0).is_int());
    }

    #[test]
    fn big_int_has_no_dedicated_query_but_works() {
        // BigInt is part of the integer family for numeric conversions.
        let v = Value::BigInt(123);
        assert!(!v.is_int());
        assert!(!v.is_uint());
        assert_eq!(v.to_i64().unwrap(), 123);
    }

    // -- as_* conversions: success path -----------------------------------

    #[test]
    fn as_conversions_ok() {
        assert!(Value::Bool(true).as_bool().unwrap());
        assert_eq!(Value::Int(-5).as_int().unwrap(), -5);
        assert_eq!(Value::UInt(7).as_uint().unwrap(), 7);
        assert_eq!(Value::Float(2.5).as_float().unwrap(), 2.5);
        assert_eq!(Value::Bytes(vec![1, 2]).as_bytes().unwrap(), &[1, 2]);
        assert_eq!(Value::String(String::from("x")).as_string().unwrap(), "x");
    }

    #[test]
    fn as_list_and_container_ok() {
        let list = vec![Value::Int(1), Value::Int(2)];
        assert_eq!(Value::List(list.clone()).as_list().unwrap(), &list);

        let mut map = IndexMap::new();
        map.insert("a".to_string(), Value::Int(1));
        let v = Value::Container(map.clone());
        assert_eq!(v.as_container().unwrap(), &map);
    }

    #[test]
    fn as_mut_methods() {
        let mut v = Value::Bytes(vec![1, 2]);
        v.as_bytes_mut().unwrap().push(3);
        assert_eq!(v.as_bytes().unwrap(), &[1, 2, 3]);

        let mut v = Value::List(vec![Value::Int(1)]);
        v.as_list_mut().unwrap().push(Value::Int(2));
        assert_eq!(v.as_list().unwrap().len(), 2);

        let mut v = Value::Container(IndexMap::new());
        v.as_container_mut()
            .unwrap()
            .insert("k".to_string(), Value::Int(9));
        assert!(v.get("k").is_ok());
    }

    // -- as_* conversions: error path -------------------------------------

    #[test]
    fn as_conversions_type_mismatch() {
        let err = Value::Int(5).as_bool().unwrap_err();
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
        match err {
            ConstructError::TypeMismatch {
                expected, actual, ..
            } => {
                assert_eq!(expected, "Bool");
                assert_eq!(actual, "Int");
            }
            _ => unreachable!(),
        }

        assert!(Value::UInt(1).as_int().is_err());
        assert!(Value::Int(1).as_uint().is_err());
        assert!(Value::Int(1).as_float().is_err());
        assert!(Value::String(String::new()).as_bytes().is_err());
        assert!(Value::Bytes(vec![]).as_string().is_err());
        assert!(Value::List(vec![]).as_container().is_err());
        assert!(Value::Container(IndexMap::new()).as_list().is_err());
    }

    #[test]
    fn as_errors_carry_empty_path() {
        let err = Value::None.as_int().unwrap_err();
        assert_eq!(err.path(), "");
    }

    // -- to_u64 / to_i64 / to_f64 -----------------------------------------

    #[test]
    fn to_u64_from_all_integer_variants() {
        assert_eq!(Value::UInt(10).to_u64().unwrap(), 10);
        assert_eq!(Value::Int(10).to_u64().unwrap(), 10);
        assert_eq!(Value::BigInt(10).to_u64().unwrap(), 10);
        assert_eq!(Value::BigInt(u64::MAX as i128).to_u64().unwrap(), u64::MAX);
    }

    #[test]
    fn to_u64_rejects_negative_and_overflow() {
        assert!(Value::Int(-1).to_u64().is_err());
        assert!(Value::BigInt(-1).to_u64().is_err());
        let err = Value::BigInt((u64::MAX as i128) + 1).to_u64().unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    #[test]
    fn to_i64_from_all_integer_variants() {
        assert_eq!(Value::Int(-10).to_i64().unwrap(), -10);
        assert_eq!(Value::UInt(10).to_i64().unwrap(), 10);
        assert_eq!(Value::BigInt(-10).to_i64().unwrap(), -10);
        assert_eq!(Value::BigInt(i64::MAX as i128).to_i64().unwrap(), i64::MAX);
        assert_eq!(Value::BigInt(i64::MIN as i128).to_i64().unwrap(), i64::MIN);
    }

    #[test]
    fn to_i64_rejects_overflow() {
        assert!(Value::UInt((i64::MAX as u64) + 1).to_i64().is_err());
        let err = Value::BigInt((i64::MAX as i128) + 1).to_i64().unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
        assert!(Value::BigInt((i64::MIN as i128) - 1).to_i64().is_err());
    }

    #[test]
    fn to_f64_from_all_numeric_variants() {
        assert_eq!(Value::Float(1.5).to_f64().unwrap(), 1.5);
        assert_eq!(Value::Int(3).to_f64().unwrap(), 3.0);
        assert_eq!(Value::UInt(3).to_f64().unwrap(), 3.0);
        assert_eq!(Value::BigInt(3).to_f64().unwrap(), 3.0);
    }

    #[test]
    fn to_integer_rejects_non_integer() {
        assert!(Value::Float(1.0).to_u64().is_err());
        assert!(Value::Bool(true).to_i64().is_err());
        assert!(Value::String(String::new()).to_f64().is_err());
    }

    // -- get / get_path ----------------------------------------------------

    #[test]
    fn get_existing_key() {
        let mut map = IndexMap::new();
        map.insert("name".to_string(), Value::String(String::from("abc")));
        map.insert("age".to_string(), Value::Int(21));
        let v = Value::Container(map);

        assert_eq!(v.get("name").unwrap().as_string().unwrap(), "abc");
        assert_eq!(v.get("age").unwrap().as_int().unwrap(), 21);
    }

    #[test]
    fn get_missing_key_returns_field_missing() {
        let v = Value::Container(IndexMap::new());
        let err = v.get("nope").unwrap_err();
        match err {
            ConstructError::FieldMissing { field, .. } => assert_eq!(field, "nope"),
            _ => unreachable!(),
        }
    }

    #[test]
    fn get_on_non_container_returns_type_mismatch() {
        let err = Value::Int(0).get("x").unwrap_err();
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
    }

    #[test]
    fn get_path_traverses_nested_containers() {
        // Container { outer = Container { inner = 42 } }
        let mut inner = IndexMap::new();
        inner.insert("inner".to_string(), Value::Int(42));
        let mut outer = IndexMap::new();
        outer.insert("outer".to_string(), Value::Container(inner));
        let v = Value::Container(outer);

        assert_eq!(v.get_path("outer.inner").unwrap().as_int().unwrap(), 42);
    }

    #[test]
    fn get_path_single_key() {
        let mut map = IndexMap::new();
        map.insert("k".to_string(), Value::UInt(1));
        let v = Value::Container(map);
        assert_eq!(v.get_path("k").unwrap().as_uint().unwrap(), 1);
    }

    #[test]
    fn get_path_missing_enriches_path() {
        let mut inner = IndexMap::new();
        inner.insert("inner".to_string(), Value::Int(1));
        let mut outer = IndexMap::new();
        outer.insert("outer".to_string(), Value::Container(inner));
        let v = Value::Container(outer);

        let err = v.get_path("outer.missing").unwrap_err();
        match err {
            ConstructError::FieldMissing { path, field } => {
                assert_eq!(path, "outer");
                assert_eq!(field, "missing");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn get_path_through_non_container_reports_context() {
        let mut map = IndexMap::new();
        map.insert("a".to_string(), Value::Int(1));
        let v = Value::Container(map);

        let err = v.get_path("a.b").unwrap_err();
        // 'a' is an Int, not a Container.
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
        assert_eq!(err.path(), "a");
    }

    // -- PartialEq / Clone / Debug ----------------------------------------

    #[test]
    fn partial_eq_for_scalars_and_containers() {
        assert_eq!(Value::Int(1), Value::Int(1));
        assert_ne!(Value::Int(1), Value::UInt(1));
        assert_eq!(Value::None, Value::None);

        let mut m1 = IndexMap::new();
        m1.insert("a".to_string(), Value::Int(1));
        let mut m2 = IndexMap::new();
        m2.insert("a".to_string(), Value::Int(1));
        assert_eq!(Value::Container(m1.clone()), Value::Container(m2.clone()));

        // different value -> not equal
        let mut m3 = IndexMap::new();
        m3.insert("a".to_string(), Value::Int(2));
        assert_ne!(Value::Container(m1), Value::Container(m3));

        assert_eq!(
            Value::List(vec![Value::Int(1), Value::Int(2)]),
            Value::List(vec![Value::Int(1), Value::Int(2)])
        );
    }

    #[test]
    fn clone_is_equal() {
        let original = Value::List(vec![Value::Int(1), Value::Bytes(vec![1, 2])]);
        let cloned = original.clone();
        assert_eq!(original, cloned);
    }

    #[test]
    fn debug_formats_without_panic() {
        let mut map = IndexMap::new();
        map.insert("a".to_string(), Value::Int(1));
        let v = Value::Container(map);
        let debug = format!("{v:?}");
        assert!(debug.contains("Container"));
        assert!(debug.contains("Int(1)"));
    }

    // -- Display -----------------------------------------------------------

    #[test]
    fn display_scalars() {
        assert_eq!(Value::None.to_string(), "None");
        assert_eq!(Value::Bool(true).to_string(), "true");
        assert_eq!(Value::Bool(false).to_string(), "false");
        assert_eq!(Value::Int(-42).to_string(), "-42");
        assert_eq!(Value::UInt(42).to_string(), "42");
        assert_eq!(Value::BigInt(99).to_string(), "99");
        assert_eq!(Value::Float(3.5).to_string(), "3.5");
    }

    #[test]
    fn display_bytes_short_and_long() {
        let short = Value::Bytes(vec![b'A', b'B']);
        assert_eq!(short.to_string(), "b'AB' (total 2)");

        // non-printable byte uses \xNN
        let bin = Value::Bytes(vec![0x00, 0xFF]);
        let s = bin.to_string();
        assert!(s.contains("\\x00"));
        assert!(s.contains("\\xff"));

        // truncation past 16 bytes
        let long = Value::Bytes(vec![b'x'; 20]);
        let s = long.to_string();
        assert!(s.contains("truncated"));
        assert!(s.contains("total 20"));
    }

    #[test]
    fn display_string_short_and_long() {
        let short = Value::String(String::from("hello"));
        assert_eq!(short.to_string(), "'hello' (total 5)");

        let long_text = "a".repeat(40);
        let long = Value::String(long_text);
        let s = long.to_string();
        assert!(s.contains("truncated"));
        assert!(s.contains("total 40"));
    }

    #[test]
    fn display_container_multiline() {
        let mut map = IndexMap::new();
        map.insert("a".to_string(), Value::Int(1));
        map.insert("b".to_string(), Value::Int(2));
        let v = Value::Container(map);
        let s = v.to_string();
        assert!(s.starts_with("Container:"));
        assert!(s.contains("a = 1"));
        assert!(s.contains("b = 2"));
    }

    #[test]
    fn display_list_multiline() {
        let v = Value::List(vec![Value::Int(1), Value::Int(2)]);
        let s = v.to_string();
        assert!(s.starts_with("List:"));
        assert!(s.contains("    1"));
        assert!(s.contains("    2"));
    }

    #[test]
    fn display_nested_container_indents() {
        let mut inner = IndexMap::new();
        inner.insert("c".to_string(), Value::Int(3));
        let mut outer = IndexMap::new();
        outer.insert("b".to_string(), Value::Container(inner));
        let v = Value::Container(outer);
        let s = v.to_string();
        let lines: Vec<&str> = s.lines().collect();
        assert!(
            lines.contains(&"    b = Container:"),
            "Expected line '    b = Container:', got:\n{s}"
        );
        assert!(
            lines.contains(&"        c = 3"),
            "Expected line with exactly 8 spaces for 'c = 3', got:\n{s}"
        );
    }

    #[test]
    fn display_container_nested_list_indents() {
        // Container({a: List([1, 2])})
        let list_val = Value::List(vec![Value::Int(10), Value::Int(20)]);
        let mut map = IndexMap::new();
        map.insert("a".to_string(), list_val);
        let v = Value::Container(map);
        let s = v.to_string();
        let lines: Vec<&str> = s.lines().collect();
        assert!(
            lines.contains(&"    a = List:"),
            "Expected line '    a = List:', got:\n{s}"
        );
        assert!(
            lines.contains(&"        10"),
            "Expected line '        10', got:\n{s}"
        );
        assert!(
            lines.contains(&"        20"),
            "Expected line '        20', got:\n{s}"
        );
    }

    #[test]
    fn display_list_nested_container_indents() {
        // List([Container({x: 1})])
        let mut inner = IndexMap::new();
        inner.insert("x".to_string(), Value::Int(1));
        let v = Value::List(vec![Value::Container(inner)]);
        let s = v.to_string();
        let lines: Vec<&str> = s.lines().collect();
        assert!(
            lines.contains(&"    Container:"),
            "Expected line '    Container:', got:\n{s}"
        );
        assert!(
            lines.contains(&"        x = 1"),
            "Expected line '        x = 1', got:\n{s}"
        );
    }

    #[test]
    fn display_deeply_nested_triple_indent() {
        // Container({a: Container({b: Container({c: 1})})})
        let mut third = IndexMap::new();
        third.insert("c".to_string(), Value::Int(1));
        let mut second = IndexMap::new();
        second.insert("b".to_string(), Value::Container(third));
        let mut first = IndexMap::new();
        first.insert("a".to_string(), Value::Container(second));
        let v = Value::Container(first);
        let s = v.to_string();
        let lines: Vec<&str> = s.lines().collect();
        // level 0: Container:
        // level 1 (4 spaces): a = Container:
        // level 2 (8 spaces): b = Container:
        // level 3 (12 spaces): c = 1
        assert!(
            lines.contains(&"    a = Container:"),
            "Expected '    a = Container:', got:\n{s}"
        );
        assert!(
            lines.contains(&"        b = Container:"),
            "Expected '        b = Container:', got:\n{s}"
        );
        assert!(
            lines.contains(&"            c = 1"),
            "Expected '            c = 1', got:\n{s}"
        );
    }

    // -- round-trip construction helpers ----------------------------------

    #[test]
    fn container_preserves_insertion_order() {
        let mut map = IndexMap::new();
        map.insert("first".to_string(), Value::Int(1));
        map.insert("second".to_string(), Value::Int(2));
        map.insert("third".to_string(), Value::Int(3));
        let v = Value::Container(map);

        let keys: Vec<&str> = v
            .as_container()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, vec!["first", "second", "third"]);
    }
}
