//! Input abstraction for build-direction field reading.
//!
//! During `exec_build`, compiled nodes read field values from an [`Input`]
//! rather than receiving a complete [`Value`] tree. This enables lazy reading:
//!
//! - **Phase 12 (Rust path)**: [`ValueInput`] wraps a `&Value` (zero-cost,
//!   reads on demand).
//! - **Phase 13 (Python path)**: `PyInput` (defined in construct-py) wraps a
//!   `&PyObject`, reading fields via FFI only when accessed.

use crate::core::error::{ConstructError, Result};
use crate::value::Value;

// ===========================================================================
// Input trait
// ===========================================================================

/// A field-value reader for build-direction traversal.
///
/// Each [`CompiledNode`](super::CompiledNode) variant's `exec_build` reads its
/// input via this trait. The trait provides two access modes:
///
/// - **Scalar access** ([`as_value`](Self::as_value)): for **leaf** and
///   **wrapper** nodes that consume the entire input as a single [`Value`].
///   `ValueInput` returns the wrapped `&Value` (clone).
/// - **Composite access** ([`get_field`](Self::get_field) /
///   [`get_index`](Self::get_index)): for **composite** nodes (Struct,
///   Sequence, Array) that read individual children by name or index.
///
/// # Implementations
///
/// - [`ValueInput`] — wraps `&Value`, the Phase 12 pure-Rust path.
/// - [`OwnedValueInput`] — owns a `Value`, used for sub-inputs and wrapper
///   transforms that produce temporary `Value`s.
/// - `PyInput` (construct-py, Phase 13) — wraps `&PyObject`, reads fields
///   lazily via FFI.
#[allow(clippy::len_without_is_empty)]
pub trait Input {
    /// Returns the entire input as a single [`Value`].
    ///
    /// Used by **leaf nodes** and **wrapper nodes** whose `exec_build`
    /// delegates to `inner.build(&Value, ...)`.
    ///
    /// # Errors
    ///
    /// Returns a type-mismatch error if the underlying object cannot be
    /// converted to a [`Value`].
    fn as_value(&self) -> Result<Value>;

    /// Reads a named field, returning its value.
    ///
    /// Used by **composite nodes** (Struct, Sequence named entries) to read
    /// individual child values.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::FieldMissing`] if the field is absent.
    fn get_field(&self, name: &str) -> Result<Value>;

    /// Returns whether a named field is present (and non-None).
    fn has_field(&self, name: &str) -> bool;

    /// Reads the value at a sequential index (for Array/Sequence build).
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::Index`] if the index is out of bounds.
    fn get_index(&self, index: usize) -> Result<Value>;

    /// Returns the number of items (for repetition constructs).
    fn len(&self) -> usize;

    /// Returns a sub-input for a nested field.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::FieldMissing`] if the field is absent.
    fn sub_input_field(&self, name: &str) -> Result<Box<dyn Input>>;

    /// Returns a sub-input for a sequential index.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::Index`] if the index is out of bounds.
    fn sub_input_index(&self, index: usize) -> Result<Box<dyn Input>>;
}

// ===========================================================================
// ValueInput — borrowed &Value wrapper
// ===========================================================================

/// Phase 12 [`Input`] impl — wraps a `&Value` (zero-cost lazy read).
///
/// All methods read from the wrapped `&Value` with no FFI and no allocation
/// beyond the inevitable `Value::clone` in `as_value` / `get_field` (the
/// Value must be owned to be passed to `inner.build`).
pub struct ValueInput<'a> {
    /// The borrowed Value being read.
    value: &'a Value,
}

impl<'a> ValueInput<'a> {
    /// Creates a new `ValueInput` wrapping a reference to a [`Value`].
    #[must_use]
    pub fn new(value: &'a Value) -> Self {
        Self { value }
    }
}

impl<'a> Input for ValueInput<'a> {
    fn as_value(&self) -> Result<Value> {
        Ok(self.value.clone())
    }

    fn get_field(&self, name: &str) -> Result<Value> {
        let container = self.value.as_container()?;
        container
            .get(name)
            .cloned()
            .ok_or_else(|| ConstructError::FieldMissing {
                path: String::new(),
                field: name.to_string(),
            })
    }

    fn has_field(&self, name: &str) -> bool {
        self.value
            .as_container()
            .map(|c| c.get(name).is_some())
            .unwrap_or(false)
    }

    fn get_index(&self, index: usize) -> Result<Value> {
        let list = self.value.as_list()?;
        list.get(index)
            .cloned()
            .ok_or_else(|| ConstructError::Index {
                path: String::new(),
                index,
                length: list.len(),
            })
    }

    fn len(&self) -> usize {
        match self.value {
            Value::Container(c) => c.len(),
            Value::List(l) => l.len(),
            _ => 1,
        }
    }

    fn sub_input_field(&self, name: &str) -> Result<Box<dyn Input>> {
        // ValueInput borrows &'a Value, but Box<dyn Input> must be independent
        // of the parent's borrow. Clone the sub-value and wrap in
        // OwnedValueInput.
        let value = self.get_field(name)?;
        Ok(Box::new(OwnedValueInput::new(value)))
    }

    fn sub_input_index(&self, index: usize) -> Result<Box<dyn Input>> {
        let value = self.get_index(index)?;
        Ok(Box::new(OwnedValueInput::new(value)))
    }
}

// ===========================================================================
// OwnedValueInput — owned Value wrapper (for sub-inputs / transforms)
// ===========================================================================

/// Owned variant of [`ValueInput`] — holds an owned [`Value`].
///
/// Used for sub-inputs that must outlive the borrow of their parent (e.g.
/// `sub_input_field` / `sub_input_index` return `Box<dyn Input>` which cannot
/// borrow from the parent), and for wrapper transforms that produce temporary
/// `Value`s (e.g. Adapter's `encode` output).
pub struct OwnedValueInput {
    /// The owned Value being read.
    value: Value,
}

impl OwnedValueInput {
    /// Creates a new `OwnedValueInput` from an owned [`Value`].
    #[must_use]
    pub fn new(value: Value) -> Self {
        Self { value }
    }
}

impl Input for OwnedValueInput {
    fn as_value(&self) -> Result<Value> {
        Ok(self.value.clone())
    }

    fn get_field(&self, name: &str) -> Result<Value> {
        let container = self.value.as_container()?;
        container
            .get(name)
            .cloned()
            .ok_or_else(|| ConstructError::FieldMissing {
                path: String::new(),
                field: name.to_string(),
            })
    }

    fn has_field(&self, name: &str) -> bool {
        self.value
            .as_container()
            .map(|c| c.get(name).is_some())
            .unwrap_or(false)
    }

    fn get_index(&self, index: usize) -> Result<Value> {
        let list = self.value.as_list()?;
        list.get(index)
            .cloned()
            .ok_or_else(|| ConstructError::Index {
                path: String::new(),
                index,
                length: list.len(),
            })
    }

    fn len(&self) -> usize {
        match &self.value {
            Value::Container(c) => c.len(),
            Value::List(l) => l.len(),
            _ => 1,
        }
    }

    fn sub_input_field(&self, name: &str) -> Result<Box<dyn Input>> {
        let value = self.get_field(name)?;
        Ok(Box::new(OwnedValueInput::new(value)))
    }

    fn sub_input_index(&self, index: usize) -> Result<Box<dyn Input>> {
        let value = self.get_index(index)?;
        Ok(Box::new(OwnedValueInput::new(value)))
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    // -- ValueInput: as_value -----------------------------------------------

    #[test]
    fn value_input_as_value_returns_clone() {
        let value = Value::Int(42);
        let input = ValueInput::new(&value);
        assert_eq!(input.as_value().unwrap(), Value::Int(42));
    }

    #[test]
    fn value_input_as_value_preserves_type() {
        let value = Value::String("hello".to_string());
        let input = ValueInput::new(&value);
        assert_eq!(
            input.as_value().unwrap(),
            Value::String("hello".to_string())
        );
    }

    // -- ValueInput: get_field ----------------------------------------------

    #[test]
    fn value_input_get_field_returns_value() {
        let mut map = IndexMap::new();
        map.insert("x".to_string(), Value::Int(1));
        map.insert("y".to_string(), Value::Int(2));
        let value = Value::Container(map);
        let input = ValueInput::new(&value);
        assert_eq!(input.get_field("x").unwrap(), Value::Int(1));
        assert_eq!(input.get_field("y").unwrap(), Value::Int(2));
    }

    #[test]
    fn value_input_get_field_missing_returns_error() {
        let map = IndexMap::new();
        let value = Value::Container(map);
        let input = ValueInput::new(&value);
        let err = input.get_field("missing").unwrap_err();
        assert!(matches!(err, ConstructError::FieldMissing { .. }));
    }

    #[test]
    fn value_input_get_field_on_non_container_returns_error() {
        let value = Value::Int(42);
        let input = ValueInput::new(&value);
        let err = input.get_field("x").unwrap_err();
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
    }

    // -- ValueInput: has_field ----------------------------------------------

    #[test]
    fn value_input_has_field_true_for_existing() {
        let mut map = IndexMap::new();
        map.insert("x".to_string(), Value::Int(1));
        let value = Value::Container(map);
        let input = ValueInput::new(&value);
        assert!(input.has_field("x"));
    }

    #[test]
    fn value_input_has_field_false_for_missing() {
        let map = IndexMap::new();
        let value = Value::Container(map);
        let input = ValueInput::new(&value);
        assert!(!input.has_field("x"));
    }

    #[test]
    fn value_input_has_field_false_for_non_container() {
        let value = Value::Int(42);
        let input = ValueInput::new(&value);
        assert!(!input.has_field("x"));
    }

    // -- ValueInput: get_index ----------------------------------------------

    #[test]
    fn value_input_get_index_returns_value() {
        let value = Value::List(vec![Value::Int(10), Value::Int(20)]);
        let input = ValueInput::new(&value);
        assert_eq!(input.get_index(0).unwrap(), Value::Int(10));
        assert_eq!(input.get_index(1).unwrap(), Value::Int(20));
    }

    #[test]
    fn value_input_get_index_out_of_bounds_returns_error() {
        let value = Value::List(vec![Value::Int(10)]);
        let input = ValueInput::new(&value);
        let err = input.get_index(5).unwrap_err();
        assert!(matches!(err, ConstructError::Index { .. }));
    }

    // -- ValueInput: len ----------------------------------------------------

    #[test]
    fn value_input_len_for_list() {
        let value = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        let input = ValueInput::new(&value);
        assert_eq!(input.len(), 3);
    }

    #[test]
    fn value_input_len_for_container() {
        let mut map = IndexMap::new();
        map.insert("a".to_string(), Value::Int(1));
        map.insert("b".to_string(), Value::Int(2));
        let value = Value::Container(map);
        let input = ValueInput::new(&value);
        assert_eq!(input.len(), 2);
    }

    #[test]
    fn value_input_len_for_scalar_is_one() {
        let value = Value::Int(42);
        let input = ValueInput::new(&value);
        assert_eq!(input.len(), 1);
    }

    // -- ValueInput: sub_input_field ----------------------------------------

    #[test]
    fn value_input_sub_input_field_returns_owned_input() {
        let mut map = IndexMap::new();
        map.insert(
            "inner".to_string(),
            Value::Container({
                let mut m = IndexMap::new();
                m.insert("deep".to_string(), Value::Int(99));
                m
            }),
        );
        let value = Value::Container(map);
        let input = ValueInput::new(&value);
        let sub = input.sub_input_field("inner").unwrap();
        assert_eq!(sub.get_field("deep").unwrap(), Value::Int(99));
    }

    // -- ValueInput: sub_input_index ----------------------------------------

    #[test]
    fn value_input_sub_input_index_returns_owned_input() {
        let value = Value::List(vec![Value::Int(100), Value::Int(200)]);
        let input = ValueInput::new(&value);
        let sub = input.sub_input_index(1).unwrap();
        assert_eq!(sub.as_value().unwrap(), Value::Int(200));
    }

    // -- OwnedValueInput ----------------------------------------------------

    #[test]
    fn owned_value_input_as_value_returns_clone() {
        let input = OwnedValueInput::new(Value::UInt(7));
        assert_eq!(input.as_value().unwrap(), Value::UInt(7));
    }

    #[test]
    fn owned_value_input_get_field_works() {
        let mut map = IndexMap::new();
        map.insert("key".to_string(), Value::String("val".to_string()));
        let input = OwnedValueInput::new(Value::Container(map));
        assert_eq!(
            input.get_field("key").unwrap(),
            Value::String("val".to_string())
        );
    }

    #[test]
    fn owned_value_input_has_field_works() {
        let mut map = IndexMap::new();
        map.insert("present".to_string(), Value::Int(1));
        let input = OwnedValueInput::new(Value::Container(map));
        assert!(input.has_field("present"));
        assert!(!input.has_field("absent"));
    }

    #[test]
    fn owned_value_input_get_index_works() {
        let input = OwnedValueInput::new(Value::List(vec![Value::Int(5)]));
        assert_eq!(input.get_index(0).unwrap(), Value::Int(5));
    }

    #[test]
    fn owned_value_input_len_works() {
        let input = OwnedValueInput::new(Value::List(vec![Value::Int(1), Value::Int(2)]));
        assert_eq!(input.len(), 2);
    }

    #[test]
    fn owned_value_input_sub_input_field_works() {
        let mut inner = IndexMap::new();
        inner.insert("deep".to_string(), Value::Bool(true));
        let mut outer = IndexMap::new();
        outer.insert("inner".to_string(), Value::Container(inner));
        let input = OwnedValueInput::new(Value::Container(outer));
        let sub = input.sub_input_field("inner").unwrap();
        assert_eq!(sub.get_field("deep").unwrap(), Value::Bool(true));
    }

    #[test]
    fn owned_value_input_sub_input_index_works() {
        let input = OwnedValueInput::new(Value::List(vec![Value::Int(42)]));
        let sub = input.sub_input_index(0).unwrap();
        assert_eq!(sub.as_value().unwrap(), Value::Int(42));
    }

    // -- Input as trait object ----------------------------------------------

    #[test]
    fn input_trait_object_dispatches_correctly() {
        let value = Value::Int(99);
        let input: Box<dyn Input> = Box::new(ValueInput::new(&value));
        assert_eq!(input.as_value().unwrap(), Value::Int(99));
    }

    #[test]
    fn owned_input_trait_object_dispatches_correctly() {
        let input: Box<dyn Input> = Box::new(OwnedValueInput::new(Value::Int(99)));
        assert_eq!(input.as_value().unwrap(), Value::Int(99));
    }
}
