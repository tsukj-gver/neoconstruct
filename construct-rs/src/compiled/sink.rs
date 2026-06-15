//! Output sink abstraction for parse result collection.
//!
//! During `exec_parse`, compiled nodes write their results to an
//! [`OutputSink`] rather than returning a [`Value`]. This abstraction
//! enables:
//!
//! - **Phase 12 (Rust path)**: [`ValueSink`] collects into a [`Value`] tree.
//! - **Phase 13 (Python path)**: `PyDictSink` will write directly to a Python
//!   `dict`, avoiding intermediate `Value` construction.
//!
//! # Protocol (I1 unified sub-sink protocol)
//!
//! The protocol is **uniform** across leaf and composite nodes:
//!
//! 1. **Parent node** creates a sub-sink for each child via
//!    [`sub_sink_for_field`](OutputSink::sub_sink_for_field) or
//!    [`sub_sink_for_item`](OutputSink::sub_sink_for_item).
//! 2. **Leaf nodes** deposit their scalar result via
//!    [`set_scalar`](OutputSink::set_scalar).
//! 3. **Composite nodes** recursively create sub-sinks and call
//!    [`set_field`](OutputSink::set_field) /
//!    [`push_item`](OutputSink::push_item).
//! 4. **Parent node** integrates the child's result via
//!    [`finish_subsink`](OutputSink::finish_subsink) or directly calls
//!    [`into_value`](OutputSink::into_value) on the sub-sink.

use crate::core::error::Result;
use crate::value::Value;

// ===========================================================================
// OutputSink trait
// ===========================================================================

/// Trait for collecting parse results during `exec_parse`.
///
/// Each [`CompiledNode`](super::CompiledNode) variant's `exec_parse`
/// implementation writes its output to the provided sink via this trait.
/// The protocol is **uniform** across leaf and composite nodes (I1
/// correction):
///
/// - **Leaf nodes** (e.g. `CompiledFormatField`) call
///   [`set_scalar`](Self::set_scalar) to deposit their parsed value.
/// - **Composite nodes** (e.g. `CompiledStruct`) create sub-sinks for
///   children, delegate `exec_parse` to the child with the sub-sink, then
///   call [`set_field`](Self::set_field) /
///   [`push_item`](Self::push_item) to integrate the child's result.
///
/// # Implementations
///
/// - [`ValueSink`] — the Phase 12 pure-Rust path, collects into a
///   [`Value`] tree.
/// - `PyDictSink` (Phase 13) — writes directly to a Python `dict`.
pub trait OutputSink {
    /// Deposits a scalar value as this sink's result.
    ///
    /// Used by **leaf nodes** after parsing. The value is retrieved later
    /// by [`into_value`](Self::into_value).
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError`](crate::core::error::ConstructError) on
    /// type mismatch (typed sinks in future phases).
    fn set_scalar(&mut self, value: Value) -> Result<()>;

    /// Sets a named field to a value.
    ///
    /// Used by **composite nodes** for each named field after integrating
    /// the child sub-sink's result.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError`](crate::core::error::ConstructError) if
    /// the sink cannot accept the value.
    fn set_field(&mut self, name: &str, value: Value) -> Result<()>;

    /// Pushes an unnamed / sequential item (used by Array / Sequence).
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError`](crate::core::error::ConstructError) if
    /// the sink cannot accept more items.
    fn push_item(&mut self, value: Value) -> Result<()>;

    /// Creates a sub-sink for a named field.
    ///
    /// The returned sub-sink receives the child node's output (scalar via
    /// `set_scalar`, or composite via recursive `set_field`). When the
    /// child finishes, the parent integrates the result via
    /// [`finish_subsink`](Self::finish_subsink) or by calling
    /// [`into_value`](Self::into_value) directly.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError`](crate::core::error::ConstructError) on
    /// allocation failure.
    fn sub_sink_for_field(&mut self, name: &str) -> Result<Box<dyn OutputSink>>;

    /// Creates a sub-sink for a sequential item.
    ///
    /// Analogous to [`sub_sink_for_field`](Self::sub_sink_for_field) but
    /// for array elements.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError`](crate::core::error::ConstructError) on
    /// allocation failure.
    fn sub_sink_for_item(&mut self) -> Result<Box<dyn OutputSink>>;

    /// Finishes a sub-sink and integrates its result.
    ///
    /// Called after a child finishes parsing. The implementation calls
    /// [`into_value`](Self::into_value) on the sub-sink and integrates the
    /// result (as a field using the name remembered from
    /// `sub_sink_for_field`, or as a pushed item).
    ///
    /// # Errors
    ///
    /// Propagates any [`ConstructError`](crate::core::error::ConstructError)
    /// from the sub-sink's `into_value`.
    fn finish_subsink(&mut self, sub: Box<dyn OutputSink>) -> Result<()>;

    /// Consumes the sink and returns the final [`Value`].
    ///
    /// For [`ValueSink`]:
    /// - If `set_scalar` was called: returns the scalar value.
    /// - If `set_field` was called: returns `Value::Container`.
    /// - If `push_item` was called: returns `Value::List`.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError`](crate::core::error::ConstructError) if
    /// the sink contents cannot be converted.
    fn into_value(self: Box<Self>) -> Result<Value>;
}

// ===========================================================================
// ValueSink — Phase 12 pure-Rust sink
// ===========================================================================

/// Sink that collects parse results into a [`Value`] tree.
///
/// This is the Phase 12 (pure Rust path) implementation of [`OutputSink`].
/// It mirrors the current `CombinedConstruct::parse` behavior: builds a
/// `Value::Container` (for Struct) or `Value::List` (for Array), or holds
/// a single scalar (for leaf nodes).
///
/// # Priority of `into_value`
///
/// When [`into_value`](OutputSink::into_value) is called, the priority is:
/// 1. `pending` scalar (if `set_scalar` was called) — leaf node scenario.
/// 2. `fields` map (if `set_field` was called) — Struct scenario.
/// 3. `items` list (otherwise) — Array scenario.
#[derive(Debug, Default)]
pub struct ValueSink {
    /// Scalar result deposited by a leaf node via `set_scalar`.
    /// `Some` when this sink was used for a leaf; `None` for composites.
    pending: Option<Value>,
    /// Accumulated fields (for Container output, used by Struct / Sequence).
    fields: indexmap::IndexMap<String, Value>,
    /// Accumulated items (for List output, used by Array / Sequence).
    items: Vec<Value>,
    /// Field name remembered from `sub_sink_for_field` (for
    /// `finish_subsink` integration).
    pending_field_name: Option<String>,
}

impl ValueSink {
    /// Creates a new empty sink.
    ///
    /// The mode (scalar / container / list) is determined by which methods
    /// are called on the sink.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if a scalar value has been deposited via `set_scalar`.
    #[must_use]
    pub fn has_scalar(&self) -> bool {
        self.pending.is_some()
    }

    /// Returns the number of accumulated named fields.
    #[must_use]
    pub fn field_count(&self) -> usize {
        self.fields.len()
    }

    /// Returns the number of accumulated list items.
    #[must_use]
    pub fn item_count(&self) -> usize {
        self.items.len()
    }
}

impl OutputSink for ValueSink {
    fn set_scalar(&mut self, value: Value) -> Result<()> {
        self.pending = Some(value);
        Ok(())
    }

    fn set_field(&mut self, name: &str, value: Value) -> Result<()> {
        self.fields.insert(name.to_string(), value);
        Ok(())
    }

    fn push_item(&mut self, value: Value) -> Result<()> {
        self.items.push(value);
        Ok(())
    }

    fn sub_sink_for_field(&mut self, name: &str) -> Result<Box<dyn OutputSink>> {
        // Remember the field name so finish_subsink can integrate correctly.
        let mut sub = ValueSink::new();
        sub.pending_field_name = Some(name.to_string());
        Ok(Box::new(sub))
    }

    fn sub_sink_for_item(&mut self) -> Result<Box<dyn OutputSink>> {
        Ok(Box::new(ValueSink::new()))
    }

    fn finish_subsink(&mut self, sub: Box<dyn OutputSink>) -> Result<()> {
        let value = sub.into_value()?;
        // Integrate based on whether the sub-sink remembered a field name.
        // Note: the field name is on the sub-sink (already consumed by
        // into_value). For Phase 12, the parent typically calls
        // set_field/push_item directly after into_value (see design §7.5
        // Struct example). This method is a convenience for the
        // anonymous / array path where the result is pushed as an item.
        self.items.push(value);
        Ok(())
    }

    fn into_value(self: Box<Self>) -> Result<Value> {
        // Unbox to access fields.
        let sink = *self;
        // Priority: scalar (leaf) > fields (Container) > items (List).
        if let Some(v) = sink.pending {
            Ok(v)
        } else if !sink.fields.is_empty() {
            Ok(Value::Container(sink.fields))
        } else {
            Ok(Value::List(sink.items))
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    // -- ValueSink: set_scalar / into_value (leaf scenario) ----------------

    #[test]
    fn valuesink_set_scalar_then_into_value_returns_scalar() {
        let mut sink = ValueSink::new();
        sink.set_scalar(Value::UInt(42)).unwrap();
        let result = Box::new(sink).into_value().unwrap();
        assert_eq!(result, Value::UInt(42));
    }

    #[test]
    fn valuesink_set_scalar_overwrites_previous() {
        let mut sink = ValueSink::new();
        sink.set_scalar(Value::UInt(1)).unwrap();
        sink.set_scalar(Value::UInt(2)).unwrap();
        let result = Box::new(sink).into_value().unwrap();
        assert_eq!(result, Value::UInt(2));
    }

    #[test]
    fn valuesink_has_scalar_flag() {
        let mut sink = ValueSink::new();
        assert!(!sink.has_scalar());
        sink.set_scalar(Value::None).unwrap();
        assert!(sink.has_scalar());
    }

    // -- ValueSink: set_field / into_value (Container scenario) ------------

    #[test]
    fn valuesink_set_field_then_into_value_returns_container() {
        let mut sink = ValueSink::new();
        sink.set_field("a", Value::Int(1)).unwrap();
        sink.set_field("b", Value::Int(2)).unwrap();
        let result = Box::new(sink).into_value().unwrap();
        let container = result.as_container().unwrap();
        assert_eq!(container.get("a").unwrap(), &Value::Int(1));
        assert_eq!(container.get("b").unwrap(), &Value::Int(2));
    }

    #[test]
    fn valuesink_set_field_preserves_insertion_order() {
        let mut sink = ValueSink::new();
        sink.set_field("first", Value::Int(1)).unwrap();
        sink.set_field("second", Value::Int(2)).unwrap();
        sink.set_field("third", Value::Int(3)).unwrap();
        let result = Box::new(sink).into_value().unwrap();
        let container = result.as_container().unwrap();
        let keys: Vec<&str> = container.keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["first", "second", "third"]);
    }

    #[test]
    fn valuesink_set_field_overwrites_same_name() {
        let mut sink = ValueSink::new();
        sink.set_field("x", Value::Int(1)).unwrap();
        sink.set_field("x", Value::Int(99)).unwrap();
        let result = Box::new(sink).into_value().unwrap();
        let container = result.as_container().unwrap();
        assert_eq!(container.get("x").unwrap(), &Value::Int(99));
        assert_eq!(container.len(), 1);
    }

    #[test]
    fn valuesink_field_count() {
        let mut sink = ValueSink::new();
        assert_eq!(sink.field_count(), 0);
        sink.set_field("a", Value::Int(1)).unwrap();
        assert_eq!(sink.field_count(), 1);
        sink.set_field("b", Value::Int(2)).unwrap();
        assert_eq!(sink.field_count(), 2);
    }

    // -- ValueSink: push_item / into_value (List scenario) -----------------

    #[test]
    fn valuesink_push_item_then_into_value_returns_list() {
        let mut sink = ValueSink::new();
        sink.push_item(Value::Int(1)).unwrap();
        sink.push_item(Value::Int(2)).unwrap();
        let result = Box::new(sink).into_value().unwrap();
        let list = result.as_list().unwrap();
        assert_eq!(list, &vec![Value::Int(1), Value::Int(2)]);
    }

    #[test]
    fn valuesink_item_count() {
        let mut sink = ValueSink::new();
        assert_eq!(sink.item_count(), 0);
        sink.push_item(Value::Int(1)).unwrap();
        assert_eq!(sink.item_count(), 1);
    }

    // -- ValueSink: into_value priority ------------------------------------

    #[test]
    fn valuesink_scalar_takes_priority_over_fields_and_items() {
        let mut sink = ValueSink::new();
        sink.set_scalar(Value::Bool(true)).unwrap();
        sink.set_field("x", Value::Int(1)).unwrap();
        sink.push_item(Value::Int(2)).unwrap();
        let result = Box::new(sink).into_value().unwrap();
        // Scalar wins.
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn valuesink_fields_take_priority_over_items() {
        let mut sink = ValueSink::new();
        sink.set_field("x", Value::Int(1)).unwrap();
        sink.push_item(Value::Int(2)).unwrap();
        let result = Box::new(sink).into_value().unwrap();
        // Fields (Container) wins over items (List).
        assert!(result.is_container());
    }

    #[test]
    fn valuesink_empty_returns_empty_list() {
        let sink = ValueSink::new();
        let result = Box::new(sink).into_value().unwrap();
        assert_eq!(result, Value::List(vec![]));
    }

    // -- ValueSink: sub-sink creation --------------------------------------

    #[test]
    fn valuesink_sub_sink_for_field_creates_independent_sink() {
        let mut parent = ValueSink::new();
        let mut sub = parent.sub_sink_for_field("child").unwrap();
        sub.set_scalar(Value::UInt(10)).unwrap();
        // Parent is not affected by sub writes.
        assert!(!parent.has_scalar());
        // sub is Box<dyn OutputSink>, call into_value directly.
        let sub_value = sub.into_value().unwrap();
        assert_eq!(sub_value, Value::UInt(10));
    }

    #[test]
    fn valuesink_sub_sink_for_item_creates_independent_sink() {
        let mut parent = ValueSink::new();
        let mut sub = parent.sub_sink_for_item().unwrap();
        sub.set_scalar(Value::String("hello".to_string())).unwrap();
        // Parent not affected.
        assert_eq!(parent.item_count(), 0);
    }

    // -- ValueSink: finish_subsink -----------------------------------------

    #[test]
    fn valuesink_finish_subsink_pushes_value_as_item() {
        let mut parent = ValueSink::new();
        let mut sub = parent.sub_sink_for_item().unwrap();
        sub.set_scalar(Value::Int(42)).unwrap();
        parent.finish_subsink(sub).unwrap();
        assert_eq!(parent.item_count(), 1);
        let result = Box::new(parent).into_value().unwrap();
        assert_eq!(result, Value::List(vec![Value::Int(42)]));
    }

    #[test]
    fn valuesink_finish_multiple_subsinks() {
        let mut parent = ValueSink::new();
        for i in 0..3_i64 {
            let mut sub = parent.sub_sink_for_item().unwrap();
            sub.set_scalar(Value::Int(i)).unwrap();
            parent.finish_subsink(sub).unwrap();
        }
        let result = Box::new(parent).into_value().unwrap();
        assert_eq!(
            result,
            Value::List(vec![Value::Int(0), Value::Int(1), Value::Int(2)])
        );
    }

    // -- ValueSink: default and debug --------------------------------------

    #[test]
    fn valuesink_default_is_empty() {
        let sink = ValueSink::default();
        assert!(!sink.has_scalar());
        assert_eq!(sink.field_count(), 0);
        assert_eq!(sink.item_count(), 0);
    }

    #[test]
    fn valuesink_debug_formats() {
        let sink = ValueSink::new();
        let _ = format!("{sink:?}");
    }

    // -- OutputSink as trait object ----------------------------------------

    #[test]
    fn dyn_output_sink_trait_object_works() {
        let sink: Box<dyn OutputSink> = Box::new(ValueSink::new());
        // Can call into_value on the trait object.
        let result = sink.into_value().unwrap();
        assert_eq!(result, Value::List(vec![]));
    }

    #[test]
    fn output_sink_set_field_via_trait_object() {
        let mut sink: Box<dyn OutputSink> = Box::new(ValueSink::new());
        sink.set_field("key", Value::Int(1)).unwrap();
        let result = sink.into_value().unwrap();
        assert!(result.is_container());
    }

    // -- ValueSink: manual integration (Struct-like pattern) ----------------

    #[test]
    fn valuesink_struct_like_integration_pattern() {
        // Simulates CompiledStruct::exec_parse behavior:
        // 1. Create sub_sink for field
        // 2. "Parse" into sub_sink (set_scalar)
        // 3. Extract value via into_value
        // 4. set_field on parent
        let mut parent = ValueSink::new();

        // Field "len"
        let mut sub = parent.sub_sink_for_field("len").unwrap();
        sub.set_scalar(Value::UInt(3)).unwrap();
        let len_value = sub.into_value().unwrap();
        parent.set_field("len", len_value.clone()).unwrap();

        // Field "data"
        let mut sub2 = parent.sub_sink_for_field("data").unwrap();
        sub2.set_scalar(Value::Bytes(vec![0x41, 0x42, 0x43]))
            .unwrap();
        let data_value = sub2.into_value().unwrap();
        parent.set_field("data", data_value.clone()).unwrap();

        let result = Box::new(parent).into_value().unwrap();
        let container = result.as_container().unwrap();
        assert_eq!(container.get("len").unwrap(), &Value::UInt(3));
        assert_eq!(
            container.get("data").unwrap(),
            &Value::Bytes(vec![0x41, 0x42, 0x43])
        );
    }

    // -- ValueSink: nested container scenario -------------------------------

    #[test]
    fn valuesink_nested_container_via_manual_integration() {
        // Simulate: Struct { header: Struct { magic: Int } }
        let mut root = ValueSink::new();

        // Create sub-sink for "header" (which is itself a container)
        let mut header_sink = root.sub_sink_for_field("header").unwrap();
        // "Parse" header struct: create sub-sink for "magic"
        let mut magic_sink = header_sink.sub_sink_for_field("magic").unwrap();
        magic_sink.set_scalar(Value::UInt(42)).unwrap();
        let magic_val = magic_sink.into_value().unwrap();
        header_sink.set_field("magic", magic_val).unwrap();
        // Header is done — extract and put into root
        let header_val = header_sink.into_value().unwrap();
        root.set_field("header", header_val).unwrap();

        let result = Box::new(root).into_value().unwrap();
        let outer = result.as_container().unwrap();
        let header = outer.get("header").unwrap().as_container().unwrap();
        assert_eq!(header.get("magic").unwrap(), &Value::UInt(42));
    }

    // -- ValueSink: mixed-type field values ---------------------------------

    #[test]
    fn valuesink_holds_various_value_types() {
        let mut sink = ValueSink::new();
        sink.set_field("int_val", Value::Int(-5)).unwrap();
        sink.set_field("uint_val", Value::UInt(10)).unwrap();
        sink.set_field("str_val", Value::String("hello".to_string()))
            .unwrap();
        sink.set_field("bytes_val", Value::Bytes(vec![1, 2, 3]))
            .unwrap();
        sink.set_field("list_val", Value::List(vec![Value::Int(1)]))
            .unwrap();

        let mut expected = IndexMap::new();
        expected.insert("int_val".to_string(), Value::Int(-5));
        expected.insert("uint_val".to_string(), Value::UInt(10));
        expected.insert("str_val".to_string(), Value::String("hello".to_string()));
        expected.insert("bytes_val".to_string(), Value::Bytes(vec![1, 2, 3]));
        expected.insert("list_val".to_string(), Value::List(vec![Value::Int(1)]));

        let result = Box::new(sink).into_value().unwrap();
        assert_eq!(result, Value::Container(expected));
    }
}
