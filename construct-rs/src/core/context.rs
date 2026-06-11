//! Context container for passing state between nested construct operations.
//!
//! This module defines the [`Context`] type used during `parse` and `build`
//! operations. It supports nested scopes via parent references (the Python
//! `context._` pattern), field insertion and lookup (both local and recursive),
//! and a closure-based scope management API ([`with_subcontext`]).
//!
//! See `docs/模块设计-Context容器.md` for the full design rationale and the
//! mapping between Python patterns and the Rust API.

use indexmap::IndexMap;

use crate::core::error::ConstructError;
use crate::value::Value;

/// A key-value container used to pass context during `parse` and `build`.
///
/// `Context` supports **nested scopes**: calling [`subcontext`](Context::subcontext)
/// creates a child context that holds an owned copy of its parent. Lookup methods
/// such as [`get_recursive`] search the current level first, then walk up the
/// parent chain.
///
/// # Python correspondence
///
/// | Python | Rust |
/// |--------|------|
/// | `context = Container()` | `Context::new()` |
/// | `context.field = value` | `ctx.insert("field", value)` |
/// | `context.field` | `ctx.get("field")` |
/// | `context._` | `ctx.parent()` |
/// | `this.field.subfield` | `ctx.get_path(&["field", "subfield"])` |
///
/// [`get_recursive`]: Context::get_recursive
#[derive(Clone, Debug)]
pub struct Context {
    /// The key-value pairs stored at this nesting level.
    fields: IndexMap<String, Value>,
    /// The enclosing (parent) context, if any. Corresponds to `_` in the
    /// Python version.
    parent: Option<Box<Context>>,
}

impl Context {
    /// Creates a new, empty context with no parent.
    ///
    /// # Example
    ///
    /// ```
    /// # use construct::core::context::Context;
    /// let ctx = Context::new();
    /// assert!(ctx.get("absent").is_none());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Context {
            fields: IndexMap::new(),
            parent: None,
        }
    }

    /// Creates a child context that references the current context as its
    /// parent.
    ///
    /// The child starts with an empty `fields` map. It can look up values in
    /// the parent via [`get_recursive`](Context::get_recursive).
    ///
    /// # Example
    ///
    /// ```
    /// # use construct::core::context::Context;
    /// # use construct::value::Value;
    /// let mut parent = Context::new();
    /// parent.insert("x", Value::Int(1));
    /// let child = parent.subcontext();
    /// assert!(child.get("x").is_none());        // local lookup only
    /// assert_eq!(child.get_recursive("x").unwrap(), &Value::Int(1)); // recursive
    /// ```
    #[must_use]
    pub fn subcontext(&self) -> Context {
        Context {
            fields: IndexMap::new(),
            parent: Some(Box::new(self.clone())),
        }
    }

    /// Inserts a key-value pair into the context, overwriting any existing
    /// entry with the same key.
    ///
    /// # Example
    ///
    /// ```
    /// # use construct::core::context::Context;
    /// # use construct::value::Value;
    /// let mut ctx = Context::new();
    /// ctx.insert("count", Value::Int(42));
    /// assert_eq!(ctx.get("count").unwrap(), &Value::Int(42));
    /// ```
    pub fn insert(&mut self, key: impl Into<String>, value: Value) {
        self.fields.insert(key.into(), value);
    }

    /// Returns a reference to the value for `key` at the **current level only**.
    ///
    /// Returns `None` if the key does not exist in this context's own fields.
    /// Use [`get_recursive`](Context::get_recursive) to search parent scopes.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.fields.get(key)
    }

    /// Returns a mutable reference to the value for `key` at the **current
    /// level only**.
    ///
    /// Returns `None` if the key does not exist in this context's own fields.
    #[must_use]
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.fields.get_mut(key)
    }

    /// Returns a reference to the value for `key`, searching the current level
    /// first and then walking up the parent chain.
    ///
    /// Returns `None` if the key is not found at any level.
    #[must_use]
    pub fn get_recursive(&self, key: &str) -> Option<&Value> {
        if let Some(value) = self.fields.get(key) {
            return Some(value);
        }
        self.parent.as_ref().and_then(|p| p.get_recursive(key))
    }

    /// Returns a reference to the value for `key`, returning an error if the
    /// key is not found at the **current level**.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::FieldMissing`] if `key` is not present.
    pub fn get_or_error(&self, key: &str) -> Result<&Value, ConstructError> {
        self.fields
            .get(key)
            .ok_or_else(|| ConstructError::FieldMissing {
                path: String::new(),
                field: key.to_string(),
            })
    }

    /// Resolves a `this`-style path such as `["header", "length"]`.
    ///
    /// Each segment is looked up in turn:
    /// 1. The first segment is looked up in the context (recursively, via
    ///    [`get_recursive`](Context::get_recursive)).
    /// 2. Subsequent segments are looked up inside the resulting
    ///    [`Value::Container`] using [`Value::get`].
    ///
    /// If any segment fails, a [`ConstructError`] is returned with the path
    /// prefix enriched.
    ///
    /// # Example
    ///
    /// ```
    /// # use construct::core::context::Context;
    /// # use construct::value::Value;
    /// # use indexmap::IndexMap;
    /// let mut inner = IndexMap::new();
    /// inner.insert("length".to_string(), Value::Int(42));
    /// let mut outer = IndexMap::new();
    /// outer.insert("header".to_string(), Value::Container(inner));
    ///
    /// let mut ctx = Context::new();
    /// ctx.insert("header", Value::Container(outer));
    ///
    /// let path: Vec<String> = vec!["header".into(), "header".into(), "length".into()];
    /// let result = ctx.get_path(&path).unwrap();
    /// assert_eq!(result, &Value::Int(42));
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::FieldMissing`] if the first segment is not
    /// found in the context, or [`ConstructError`] from [`Value::get`] if a
    /// subsequent segment is not found in a nested container.
    pub fn get_path(&self, path: &[String]) -> Result<&Value, ConstructError> {
        if path.is_empty() {
            return Err(ConstructError::Generic {
                path: String::new(),
                message: "empty path".to_string(),
            });
        }

        let first_key = &path[0];
        let mut current =
            self.get_recursive(first_key)
                .ok_or_else(|| ConstructError::FieldMissing {
                    path: String::new(),
                    field: first_key.clone(),
                })?;

        let mut traversed = first_key.clone();

        for key in &path[1..] {
            current = match current.get(key) {
                Ok(value) => value,
                Err(err) => {
                    return Err(err.with_path_prefix(&traversed));
                }
            };
            traversed.push('.');
            traversed.push_str(key);
        }

        Ok(current)
    }

    /// Returns a reference to the parent context, or `None` if this is the
    /// root context.
    ///
    /// Corresponds to `context._` in the Python version.
    #[must_use]
    pub fn parent(&self) -> Option<&Context> {
        self.parent.as_ref().map(|boxed| boxed.as_ref())
    }

    /// Executes a closure in a child scope.
    ///
    /// A subcontext is created from `self` and passed to `f`. When `f`
    /// returns, the subcontext is discarded. Any values inserted by `f`
    /// into the subcontext are **not** visible in `self`.
    ///
    /// # Example
    ///
    /// ```
    /// # use construct::core::context::Context;
    /// # use construct::value::Value;
    /// let mut ctx = Context::new();
    /// ctx.insert("outer", Value::Int(1));
    ///
    /// let result = ctx.with_subcontext(|child| {
    ///     child.insert("inner", Value::Int(2));
    ///     assert_eq!(child.get_recursive("outer").unwrap(), &Value::Int(1));
    ///     child.get_recursive("inner").unwrap().clone()
    /// });
    ///
    /// assert_eq!(result, Value::Int(2));
    /// assert!(ctx.get("inner").is_none()); // inner was in subcontext only
    /// ```
    pub fn with_subcontext<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut Context) -> R,
    {
        let mut child = self.subcontext();
        f(&mut child)
    }

    /// Merges all fields from `other` into `self`, overwriting existing keys.
    ///
    /// Only the top-level fields of `other` are merged; its parent chain is
    /// not walked. This is used by `Union` and similar constructs.
    ///
    /// # Example
    ///
    /// ```
    /// # use construct::core::context::Context;
    /// # use construct::value::Value;
    /// let mut ctx1 = Context::new();
    /// ctx1.insert("a", Value::Int(1));
    ///
    /// let mut ctx2 = Context::new();
    /// ctx2.insert("b", Value::Int(2));
    /// ctx2.insert("a", Value::Int(10));
    ///
    /// ctx1.merge(ctx2);
    /// assert_eq!(ctx1.get("a").unwrap(), &Value::Int(10)); // overwritten
    /// assert_eq!(ctx1.get("b").unwrap(), &Value::Int(2));
    /// ```
    pub fn merge(&mut self, other: Context) {
        for (key, value) in other.fields {
            self.fields.insert(key, value);
        }
    }
}

impl Default for Context {
    /// Returns an empty [`Context::new`].
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    // -- Construction -------------------------------------------------------

    #[test]
    fn new_creates_empty_context() {
        let ctx = Context::new();
        assert!(ctx.get("anything").is_none());
        assert!(ctx.parent().is_none());
    }

    #[test]
    fn default_is_same_as_new() {
        let ctx = Context::default();
        assert!(ctx.get("anything").is_none());
    }

    // -- Insert and get (local) ---------------------------------------------

    #[test]
    fn insert_and_get() {
        let mut ctx = Context::new();
        ctx.insert("name", Value::String("hello".to_string()));
        ctx.insert("count", Value::Int(42));

        assert_eq!(
            ctx.get("name").unwrap(),
            &Value::String("hello".to_string())
        );
        assert_eq!(ctx.get("count").unwrap(), &Value::Int(42));
        assert!(ctx.get("absent").is_none());
    }

    #[test]
    fn insert_overwrites_existing_key() {
        let mut ctx = Context::new();
        ctx.insert("key", Value::Int(1));
        ctx.insert("key", Value::Int(2));
        assert_eq!(ctx.get("key").unwrap(), &Value::Int(2));
    }

    #[test]
    fn insert_accepts_impl_into_string() {
        let mut ctx = Context::new();
        ctx.insert(String::from("owned"), Value::None);
        ctx.insert("borrowed", Value::None);
        assert!(ctx.get("owned").is_some());
        assert!(ctx.get("borrowed").is_some());
    }

    // -- get_mut ------------------------------------------------------------

    #[test]
    fn get_mut_returns_mutable_reference() {
        let mut ctx = Context::new();
        ctx.insert("val", Value::Int(10));
        {
            let val = ctx.get_mut("val").unwrap();
            *val = Value::Int(99);
        }
        assert_eq!(ctx.get("val").unwrap(), &Value::Int(99));
    }

    #[test]
    fn get_mut_returns_none_for_missing_key() {
        let mut ctx = Context::new();
        assert!(ctx.get_mut("nope").is_none());
    }

    // -- Subcontext and parent ----------------------------------------------

    #[test]
    fn subcontext_has_parent_reference() {
        let parent = Context::new();
        let child = parent.subcontext();
        assert!(child.parent().is_some());
        assert!(parent.parent().is_none());
    }

    #[test]
    fn subcontext_starts_empty() {
        let mut parent = Context::new();
        parent.insert("x", Value::Int(1));
        let child = parent.subcontext();
        assert!(child.get("x").is_none()); // local only
    }

    // -- get_recursive ------------------------------------------------------

    #[test]
    fn get_recursive_finds_local_key() {
        let mut ctx = Context::new();
        ctx.insert("x", Value::Int(1));
        assert_eq!(ctx.get_recursive("x").unwrap(), &Value::Int(1));
    }

    #[test]
    fn get_recursive_falls_through_to_parent() {
        let mut parent = Context::new();
        parent.insert("x", Value::Int(1));
        let child = parent.subcontext();
        assert_eq!(child.get_recursive("x").unwrap(), &Value::Int(1));
    }

    #[test]
    fn get_recursive_prefers_local_over_parent() {
        let mut parent = Context::new();
        parent.insert("x", Value::Int(1));
        let mut child = parent.subcontext();
        child.insert("x", Value::Int(2));
        assert_eq!(child.get_recursive("x").unwrap(), &Value::Int(2));
    }

    #[test]
    fn get_recursive_walks_deep_chain() {
        let mut level0 = Context::new();
        level0.insert("deep", Value::String("found".to_string()));

        let level1 = level0.subcontext();
        let level2 = level1.subcontext();
        let level3 = level2.subcontext();

        assert_eq!(
            level3.get_recursive("deep").unwrap(),
            &Value::String("found".to_string())
        );
    }

    #[test]
    fn get_recursive_returns_none_when_absent() {
        let ctx = Context::new();
        assert!(ctx.get_recursive("nope").is_none());
    }

    // -- get_or_error -------------------------------------------------------

    #[test]
    fn get_or_error_returns_value_when_present() {
        let mut ctx = Context::new();
        ctx.insert("key", Value::UInt(7));
        let result = ctx.get_or_error("key").unwrap();
        assert_eq!(result, &Value::UInt(7));
    }

    #[test]
    fn get_or_error_returns_field_missing_when_absent() {
        let ctx = Context::new();
        let err = ctx.get_or_error("missing").unwrap_err();
        match err {
            ConstructError::FieldMissing { field, path } => {
                assert_eq!(field, "missing");
                assert_eq!(path, "");
            }
            other => panic!("expected FieldMissing, got {other:?}"),
        }
    }

    #[test]
    fn get_or_error_does_not_search_parent() {
        let mut parent = Context::new();
        parent.insert("x", Value::Int(1));
        let child = parent.subcontext();
        // get_or_error only checks local level
        let err = child.get_or_error("x").unwrap_err();
        assert!(matches!(err, ConstructError::FieldMissing { .. }));
    }

    // -- get_path -----------------------------------------------------------

    #[test]
    fn get_path_single_key() {
        let mut ctx = Context::new();
        ctx.insert("val", Value::Int(42));
        let path: Vec<String> = vec!["val".to_string()];
        assert_eq!(ctx.get_path(&path).unwrap(), &Value::Int(42));
    }

    #[test]
    fn get_path_nested_container() {
        // ctx = { header: Container { length: 42 } }
        let mut inner = IndexMap::new();
        inner.insert("length".to_string(), Value::Int(42));
        let mut outer = IndexMap::new();
        outer.insert("header".to_string(), Value::Container(inner));

        let mut ctx = Context::new();
        ctx.insert("data", Value::Container(outer));

        let path: Vec<String> = vec![
            "data".to_string(),
            "header".to_string(),
            "length".to_string(),
        ];
        assert_eq!(ctx.get_path(&path).unwrap(), &Value::Int(42));
    }

    #[test]
    fn get_path_uses_recursive_lookup_for_first_key() {
        // parent has "shared", child resolves it via get_recursive
        let mut parent = Context::new();
        parent.insert("shared", Value::Int(99));
        let child = parent.subcontext();

        let path: Vec<String> = vec!["shared".to_string()];
        assert_eq!(child.get_path(&path).unwrap(), &Value::Int(99));
    }

    #[test]
    fn get_path_empty_path_returns_error() {
        let ctx = Context::new();
        let err = ctx.get_path(&[]).unwrap_err();
        assert!(matches!(err, ConstructError::Generic { .. }));
    }

    #[test]
    fn get_path_first_key_missing_returns_field_missing() {
        let ctx = Context::new();
        let path: Vec<String> = vec!["nonexistent".to_string()];
        let err = ctx.get_path(&path).unwrap_err();
        match err {
            ConstructError::FieldMissing { field, .. } => {
                assert_eq!(field, "nonexistent");
            }
            other => panic!("expected FieldMissing, got {other:?}"),
        }
    }

    #[test]
    fn get_path_intermediate_missing_enriches_path() {
        let mut ctx = Context::new();
        let mut outer = IndexMap::new();
        outer.insert("header".to_string(), Value::Container(IndexMap::new()));
        ctx.insert("data", Value::Container(outer));

        let path: Vec<String> = vec![
            "data".to_string(),
            "header".to_string(),
            "missing".to_string(),
        ];
        let err = ctx.get_path(&path).unwrap_err();
        match err {
            ConstructError::FieldMissing { path, field } => {
                assert_eq!(path, "data.header");
                assert_eq!(field, "missing");
            }
            other => panic!("expected FieldMissing, got {other:?}"),
        }
    }

    #[test]
    fn get_path_intermediate_non_container_enriches_path() {
        let mut ctx = Context::new();
        ctx.insert("data", Value::Int(1));

        let path: Vec<String> = vec!["data".to_string(), "sub".to_string()];
        let err = ctx.get_path(&path).unwrap_err();
        // Value::Int.get("sub") → TypeMismatch, enriched with path "data"
        assert!(matches!(err, ConstructError::TypeMismatch { .. }));
        assert_eq!(err.path(), "data");
    }

    // -- Parent isolation ---------------------------------------------------

    #[test]
    fn child_inserts_do_not_affect_parent() {
        let mut parent = Context::new();
        parent.insert("a", Value::Int(1));
        let mut child = parent.subcontext();
        child.insert("b", Value::Int(2));
        // child has b
        assert_eq!(child.get("b").unwrap(), &Value::Int(2));
        // parent does not
        assert!(parent.get("b").is_none());
    }

    #[test]
    fn child_overwrite_does_not_affect_parent() {
        let mut parent = Context::new();
        parent.insert("x", Value::Int(1));
        let mut child = parent.subcontext();
        child.insert("x", Value::Int(99));
        // child sees its own value
        assert_eq!(child.get("x").unwrap(), &Value::Int(99));
        // parent's value is unchanged
        assert_eq!(parent.get("x").unwrap(), &Value::Int(1));
    }

    // -- with_subcontext ----------------------------------------------------

    #[test]
    fn with_subcontext_runs_in_child_scope() {
        let mut ctx = Context::new();
        ctx.insert("outer", Value::Int(1));

        let result = ctx.with_subcontext(|child| {
            child.insert("inner", Value::Int(2));
            // child can see parent's key
            assert_eq!(child.get_recursive("outer").unwrap(), &Value::Int(1));
            // child can see its own key
            assert_eq!(child.get("inner").unwrap(), &Value::Int(2));
            "returned"
        });

        assert_eq!(result, "returned");
        // inner was in subcontext only
        assert!(ctx.get("inner").is_none());
    }

    #[test]
    fn with_subcontext_discards_child_after_closure() {
        let mut ctx = Context::new();
        ctx.with_subcontext(|child| {
            child.insert("temp", Value::None);
        });
        assert!(ctx.get("temp").is_none());
    }

    #[test]
    fn with_subcontext_nested() {
        let mut ctx = Context::new();
        ctx.insert("level", Value::Int(0));

        ctx.with_subcontext(|child1| {
            child1.insert("level", Value::Int(1));
            assert_eq!(child1.get_recursive("level").unwrap(), &Value::Int(1));

            child1.with_subcontext(|child2| {
                assert_eq!(child2.get_recursive("level").unwrap(), &Value::Int(1));
                child2.insert("level", Value::Int(2));
                assert_eq!(child2.get_recursive("level").unwrap(), &Value::Int(2));
            });

            // child2's changes are discarded
            assert_eq!(child1.get_recursive("level").unwrap(), &Value::Int(1));
        });
    }

    // -- merge --------------------------------------------------------------

    #[test]
    fn merge_adds_keys_from_other() {
        let mut ctx1 = Context::new();
        ctx1.insert("a", Value::Int(1));

        let mut ctx2 = Context::new();
        ctx2.insert("b", Value::Int(2));

        ctx1.merge(ctx2);
        assert_eq!(ctx1.get("a").unwrap(), &Value::Int(1));
        assert_eq!(ctx1.get("b").unwrap(), &Value::Int(2));
    }

    #[test]
    fn merge_overwrites_existing_keys() {
        let mut ctx1 = Context::new();
        ctx1.insert("key", Value::Int(1));

        let mut ctx2 = Context::new();
        ctx2.insert("key", Value::Int(99));

        ctx1.merge(ctx2);
        assert_eq!(ctx1.get("key").unwrap(), &Value::Int(99));
    }

    #[test]
    fn merge_does_not_pull_parent_fields() {
        let mut parent = Context::new();
        parent.insert("from_parent", Value::Int(1));

        let mut child = parent.subcontext();
        child.insert("from_child", Value::Int(2));

        let mut target = Context::new();
        target.merge(child);
        // Only child's own fields are merged
        assert_eq!(target.get("from_child").unwrap(), &Value::Int(2));
        assert!(target.get("from_parent").is_none());
    }

    #[test]
    fn merge_empty_context_is_noop() {
        let mut ctx = Context::new();
        ctx.insert("a", Value::Int(1));
        ctx.merge(Context::new());
        assert_eq!(ctx.get("a").unwrap(), &Value::Int(1));
    }

    // -- Clone and Debug ----------------------------------------------------

    #[test]
    fn clone_is_equal() {
        let mut ctx = Context::new();
        ctx.insert("x", Value::Int(1));
        let cloned = ctx.clone();
        assert_eq!(cloned.get("x").unwrap(), &Value::Int(1));
        assert!(cloned.parent().is_none());
    }

    #[test]
    fn clone_with_parent() {
        let mut parent = Context::new();
        parent.insert("p", Value::Int(1));
        let child = parent.subcontext();
        let cloned = child.clone();
        assert_eq!(cloned.parent().unwrap().get("p").unwrap(), &Value::Int(1));
    }

    #[test]
    fn debug_formats_without_panic() {
        let mut ctx = Context::new();
        ctx.insert("a", Value::Int(1));
        let _ = format!("{ctx:?}");

        let child = ctx.subcontext();
        let _ = format!("{child:?}");
    }

    // -- Edge cases ---------------------------------------------------------

    #[test]
    fn empty_context_get_recursive_is_none() {
        let ctx = Context::new();
        assert!(ctx.get_recursive("").is_none());
        assert!(ctx.get_recursive("anything").is_none());
    }

    #[test]
    fn deeply_nested_subcontexts() {
        let mut root = Context::new();
        root.insert("root_key", Value::Int(0));

        let mut current = root.subcontext();
        for i in 1..=10 {
            current = current.subcontext();
            current.insert("level", Value::Int(i));
        }

        // The deepest context can still see root_key
        assert_eq!(current.get_recursive("root_key").unwrap(), &Value::Int(0));
        // And its own level
        assert_eq!(current.get_recursive("level").unwrap(), &Value::Int(10));
    }
}
