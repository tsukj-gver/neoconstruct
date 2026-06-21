//! 解析/构建上下文。
//!
//! 设计依据：`docs/架构设计.md` §C.5。
//!
//! ## 用途
//!
//! `Context` 用于：
//! - 字段间引用（如 `Bytes(this.length)` 中 `this.length` 引用前序字段）。
//! - 嵌套层传递（Python construct 的 `_` 指向外层 context）。
//!
//! ## Phase 1 范围
//!
//! Phase 1 仅存储已解析/已读取的字段值，不支持 this 表达式求值。
//! `set_field` 由 Struct 节点在每字段处理完成后调用；
//! `get_field` 预留给 Phase 2 表达式系统使用（当前 Phase 1 不调用）。
//!
//! ## 为什么持有 PyDict 而非 Rust HashMap
//!
//! 字段值本身就是 Python 对象（`Py<PyAny>`），存入 `PyDict` 是 C API 调用
//! （`PyDict_SetItem`），无需 Rust→Python 转换。this 引用读取时直接返回
//! Python 对象，无中间类型（FFI 设计 §5）。与 Python construct 的 Container 语义一致。

use pyo3::prelude::*;
use pyo3::types::PyDict;

/// 解析/构建上下文。Rust 内部持有 Python dict 引用。
///
/// 字段值存入 `PyDict`（C API `PyDict_SetItem`），无 Rust 中间类型。
/// 嵌套 Struct 节点通过 `new_child` 创建子 context，`parent` 指向外层。
pub struct Context<'py> {
    /// 当前层字段字典（PyDict 引用）。
    ///
    /// parse 时存入已解析字段；build 时存入从对象读取的字段值。
    fields: Bound<'py, PyDict>,
    /// 外层上下文（嵌套 Struct 时指向父 Context），对应 Python construct 的 `_`。
    ///
    /// 顶层 context 的 `parent` 为 `None`。
    parent: Option<&'py Context<'py>>,
}

impl<'py> Context<'py> {
    /// 创建顶层 context（parse/build 入口处调用）。
    ///
    /// 创建一个空的 `PyDict` 作为字段存储容器，`parent` 为 `None`。
    pub fn new_root(py: Python<'py>) -> PyResult<Self> {
        Ok(Self {
            fields: PyDict::new_bound(py),
            parent: None,
        })
    }

    /// 创建嵌套 context（Struct 节点进入时调用）。
    ///
    /// 新建一个空 `PyDict`，`parent` 指向传入的父 context。
    /// 嵌套层可通过 `parent` 访问外层字段（对应 Python construct 的 `_`）。
    pub fn new_child(parent: &'py Context<'py>, py: Python<'py>) -> PyResult<Self> {
        Ok(Self {
            fields: PyDict::new_bound(py),
            parent: Some(parent),
        })
    }

    /// 存入字段（parse/build 每个字段完成后调用）。
    ///
    /// 对齐 Python construct `Struct._parse` 中的 `context[sc.name] = subobj`。
    pub fn set_field(&self, name: &str, value: &Bound<'_, PyAny>) -> PyResult<()> {
        self.fields.set_item(name, value)
    }

    /// 读取当前层字段（Phase 2 的 this.xxx 引用使用）。
    ///
    /// Phase 1 不调用此方法（无 this 表达式）。Phase 2 表达式系统会通过此方法
    /// 读取前序字段的值。
    ///
    /// 返回 `None` 表示当前层无此字段。注意：此方法**不**递归查找 parent。
    /// 读取父层字段应通过 `parent()` 显式获取父 context 后调用。
    pub fn get_field(&self, name: &str) -> PyResult<Option<Bound<'_, PyAny>>> {
        self.fields.get_item(name)
    }

    /// 返回父 context 的引用（若有）。
    ///
    /// 对应 Python construct 中通过 `_` 访问外层 context 的能力。
    /// Phase 1 不使用，预留给 Phase 2 表达式系统。
    pub fn parent(&self) -> Option<&'py Context<'py>> {
        self.parent
    }

    /// 借用当前层的字段字典（用于测试与调试）。
    pub fn fields(&self) -> &Bound<'py, PyDict> {
        &self.fields
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 在测试运行前手动初始化 Python 解释器。
    ///
    /// pyo3 在 `extension-module` feature 关闭时仍然不会自动初始化解释器
    /// （`auto-initialize` feature 未启用）。`prepare_freethreaded_python` 是幂等的，
    /// 多次调用安全。使用 `Once` 保证只在第一次 `Python::with_gil` 前调用。
    ///
    /// 不启用 `pyo3/auto-initialize` 是因为：
    /// 1. 避免在 maturin 打包时（通过 feature unification）误启用嵌入模式的初始化逻辑。
    /// 2. `prepare_freethreaded_python` 在测试 helper 中显式调用更直观、更可控。
    fn ensure_python_initialized() {
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            pyo3::prepare_freethreaded_python();
        });
    }

    /// 在已初始化的 Python 解释器上执行闭包。
    fn with_python<F, R>(f: F) -> R
    where
        F: for<'py> FnOnce(Python<'py>) -> R,
    {
        ensure_python_initialized();
        Python::with_gil(f)
    }

    #[test]
    fn new_root_creates_empty_dict_with_no_parent() {
        with_python(|py| {
            let ctx = Context::new_root(py).expect("new_root should succeed");
            assert!(ctx.parent().is_none());
            assert_eq!(ctx.fields().len(), 0);
        });
    }

    #[test]
    fn new_child_links_to_parent() {
        with_python(|py| {
            let root = Context::new_root(py).expect("root");
            let child = Context::new_child(&root, py).expect("child");
            assert!(child.parent().is_some());
            let parent_ref = child.parent().expect("parent should exist");
            assert_eq!(parent_ref.fields().len(), root.fields().len());
        });
    }

    #[test]
    fn set_field_stores_value() {
        with_python(|py| {
            let ctx = Context::new_root(py).expect("root");
            let value = py.eval_bound("42", None, None).expect("eval 42");
            ctx.set_field("count", &value).expect("set_field");
            assert_eq!(ctx.fields().len(), 1);
        });
    }

    #[test]
    fn get_field_returns_stored_value() {
        with_python(|py| {
            let ctx = Context::new_root(py).expect("root");
            let value = py.eval_bound("7", None, None).expect("eval 7");
            ctx.set_field("answer", &value).expect("set");
            let retrieved = ctx.get_field("answer").expect("get");
            let retrieved = retrieved.expect("field should exist");
            let n: i64 = retrieved.extract().expect("extract i64");
            assert_eq!(n, 7);
        });
    }

    #[test]
    fn get_field_returns_none_for_missing() {
        with_python(|py| {
            let ctx = Context::new_root(py).expect("root");
            let retrieved = ctx.get_field("nonexistent").expect("get");
            assert!(retrieved.is_none());
        });
    }

    #[test]
    fn set_field_overrides_existing() {
        with_python(|py| {
            let ctx = Context::new_root(py).expect("root");
            let v1 = py.eval_bound("1", None, None).expect("eval 1");
            ctx.set_field("x", &v1).expect("set v1");
            let v2 = py.eval_bound("2", None, None).expect("eval 2");
            ctx.set_field("x", &v2).expect("set v2");
            assert_eq!(ctx.fields().len(), 1);
            let retrieved = ctx.get_field("x").expect("get").expect("exist");
            let n: i64 = retrieved.extract().expect("extract");
            assert_eq!(n, 2);
        });
    }

    #[test]
    fn multiple_fields_stored_independently() {
        with_python(|py| {
            let ctx = Context::new_root(py).expect("root");
            let a = py.eval_bound("10", None, None).expect("eval");
            let b = py.eval_bound("20", None, None).expect("eval");
            let c = py.eval_bound("30", None, None).expect("eval");
            ctx.set_field("a", &a).expect("set a");
            ctx.set_field("b", &b).expect("set b");
            ctx.set_field("c", &c).expect("set c");
            assert_eq!(ctx.fields().len(), 3);
            let got_a: i64 = ctx
                .get_field("a")
                .expect("get")
                .expect("exist")
                .extract()
                .expect("extract");
            let got_c: i64 = ctx
                .get_field("c")
                .expect("get")
                .expect("exist")
                .extract()
                .expect("extract");
            assert_eq!(got_a, 10);
            assert_eq!(got_c, 30);
        });
    }

    #[test]
    fn child_context_is_independent_from_parent() {
        with_python(|py| {
            let root = Context::new_root(py).expect("root");
            let v_root = py.eval_bound("100", None, None).expect("eval");
            root.set_field("parent_only", &v_root).expect("set");

            let child = Context::new_child(&root, py).expect("child");
            let missing = child.get_field("parent_only").expect("get");
            assert!(
                missing.is_none(),
                "child should not see parent fields directly"
            );
            assert_eq!(child.fields().len(), 0);
            assert_eq!(root.fields().len(), 1);
        });
    }

    #[test]
    fn parent_chain_depth_two() {
        with_python(|py| {
            let root = Context::new_root(py).expect("root");
            let mid = Context::new_child(&root, py).expect("mid");
            let leaf = Context::new_child(&mid, py).expect("leaf");
            assert!(leaf.parent().is_some());
            let mid_ref = leaf.parent().expect("mid");
            assert!(mid_ref.parent().is_some());
            let root_ref = mid_ref.parent().expect("root");
            assert!(root_ref.parent().is_none());
        });
    }

    #[test]
    fn set_field_with_unicode_name() {
        with_python(|py| {
            let ctx = Context::new_root(py).expect("root");
            let value = py.eval_bound("1", None, None).expect("eval");
            ctx.set_field("字段", &value).expect("set unicode");
            assert_eq!(ctx.fields().len(), 1);
            let retrieved = ctx.get_field("字段").expect("get").expect("exist");
            let n: i64 = retrieved.extract().expect("extract");
            assert_eq!(n, 1);
        });
    }

    #[test]
    fn set_field_with_empty_name() {
        with_python(|py| {
            let ctx = Context::new_root(py).expect("root");
            let value = py.eval_bound("0", None, None).expect("eval");
            ctx.set_field("", &value).expect("set empty");
            assert!(ctx.get_field("").expect("get").is_some());
        });
    }
}
