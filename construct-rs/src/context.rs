//! 解析/构建上下文。
//!
//! 设计依据：`docs/架构设计.md` §C.5、`docs/模块设计-Context-Vec优化.md`。
//!
//! ## 用途
//!
//! `Context` 用于：
//! - 字段间引用（如 `Bytes(this.length)` 中 `this.length` 引用前序字段）。
//! - 嵌套层传递（Python construct 的 `_` 指向外层 context）。
//!
//! ## Vec 化优化（Phase 2.5）
//!
//! `expr_values` 按编译期字段索引存储 borrowed PyObject 指针，供 GetInt 快速访问
//! （跳过 PyDict hash 查找）。GetInt 从 ~36ns 降至 ~6ns。
//!
//! `expr_values` 为 `Option`：仅 `has_expressions=true` 的 Struct 通过
//! [`Context::init_expr_values`] 初始化；无表达式的 Struct 保持 `None`，零分配开销。

use crate::error::ConstructError;
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString};

/// 解析/构建上下文。Rust 内部持有 Python dict 引用（按需创建）。
///
/// 字段值存入 `PyDict`（C API `PyDict_SetItem`），同时按编译期字段索引
/// 在 `expr_values` 中缓存 borrowed 指针，供 GetInt 快速访问（跳过 hash 查找）。
///
/// `expr_values` 为 `Option`：仅 `has_expressions=true` 的 Struct 通过
/// [`Context::init_expr_values`] 初始化；无表达式的 Struct（placeholder）
/// 保持 `None`，零分配开销。
///
/// `fields` 为 `Option`：Phase 1 的 parse/build 入口使用 [`Context::placeholder`]
/// 创建不持有 `PyDict` 的占位 context（零 Python 对象创建）；Phase 2 恢复
/// context 使用时入口改用 [`Context::new_root`]。
pub struct Context<'py> {
    /// 当前层字段字典（`PyDict` 引用），按需创建。
    ///
    /// parse 时存入已解析字段；build 时存入从对象读取的字段值。
    /// 同时充当 instance dict（has_expressions 路径）。
    /// `None` 表示占位 context（Phase 1 入口使用，不分配 `PyDict`）。
    fields: Option<Bound<'py, PyDict>>,

    /// 外层上下文（嵌套 Struct 时指向父 Context），对应 Python construct 的 `_`。
    ///
    /// 顶层 context 的 `parent` 为 `None`。
    parent: Option<&'py Context<'py>>,

    /// 按编译期字段索引存储的 borrowed PyObject 指针，供 GetInt 快速访问。
    ///
    /// `None` 表示未初始化（无表达式的 Struct 或 placeholder）。
    /// `Some(vec)` 中每个槽位：
    /// - 非 null：指向 `fields` PyDict 中对应字段的值对象（borrowed，不 incref）
    /// - null：WO 字段（不写入值）或未设置的字段
    ///
    /// GetInt(idx) 直接读 `vec[idx]` → `PyLong_AsLongLong`，跳过 PyDict hash 查找。
    ///
    /// # Safety（不变量）
    ///
    /// 1. 指针仅在 [`Context::set_field_at`] 中写入（与 dict 写入同步）
    /// 2. 指针仅在 [`Context::get_int_at`] 中读取（fields dict 存活期间）
    /// 3. `fields` 与 `expr_values` 同属 Context，生命周期一致
    /// 4. dict 中的值不会被外部替换（Context API 是唯一写入路径）
    /// 5. PyDict resize 时不移动值对象（PyObject 在堆上，dict 内部数组重分配
    ///    不影响已存入值的对象指针）
    expr_values: Option<Vec<*mut ffi::PyObject>>,
}

// SAFETY note: Context 包含 raw pointer (`expr_values` 中的 `*mut ffi::PyObject`)，
// 这不会影响线程安全——指针指向 fields PyDict 中的值对象，在 GIL 保护下单线程访问。
// Context 本身因持有 `Bound<PyDict>` 而不可跨线程发送（pyo3 保证）。
impl<'py> Context<'py> {
    /// 创建顶层 context（parse/build 入口处调用）。
    ///
    /// 创建一个空的 `PyDict` 作为字段存储容器，`parent` 为 `None`。
    /// `expr_values` 初始化为 `None`——有表达式的 Struct 在 parse/build 入口
    /// 通过 [`Context::init_expr_values`] 按需初始化。
    ///
    /// Phase 2 恢复 context 使用后，入口改用此方法。Phase 1 入口使用
    /// [`Context::placeholder`] 以避免白白创建空 dict。
    pub fn new_root(py: Python<'py>) -> PyResult<Self> {
        Ok(Self {
            fields: Some(PyDict::new_bound(py)),
            parent: None,
            expr_values: None,
        })
    }

    /// 创建不持有 `PyDict` 的占位 context（Phase 1 parse/build 入口使用）。
    ///
    /// Phase 1 不读取也不写入 context，每次 parse 创建空 `PyDict` 是纯浪费
    /// （80-150 ns 固定开销，P0-2 优化）。此方法返回 `fields = None` 的占位
    /// context，[`Context::set_field`] / [`Context::get_field`] 在占位 context 上
    /// 为无操作 / 返回 `None`。
    ///
    /// Phase 2 恢复 context 使用后，入口改回 [`Context::new_root`]。
    pub fn placeholder(_py: Python<'py>) -> Self {
        Self {
            fields: None,
            parent: None,
            expr_values: None,
        }
    }

    /// 创建嵌套 context（Struct 节点进入时调用）。
    ///
    /// 新建一个空 `PyDict`，`parent` 指向传入的父 context。
    /// `expr_values` 初始化为 `None`——内层 Struct 自行调用
    /// [`Context::init_expr_values`] 按需初始化。
    /// 嵌套层可通过 `parent` 访问外层字段（对应 Python construct 的 `_`）。
    pub fn new_child(parent: &'py Context<'py>, py: Python<'py>) -> PyResult<Self> {
        Ok(Self {
            fields: Some(PyDict::new_bound(py)),
            parent: Some(parent),
            expr_values: None,
        })
    }

    /// 初始化 expr_values 缓冲区（预分配 n 个 null 槽位）。
    ///
    /// 在 StructNode.parse/build 的 `has_expressions` 分支入口调用一次，
    /// 在遍历字段之前。槽位数 = 当前 StructNode 的字段数。
    ///
    /// 后续 [`Context::set_field_at`] 按 idx 填充槽位（RW/RO 字段），
    /// WO 字段的槽位保持 null（编译期保证 GetInt 不引用 WO 字段）。
    ///
    /// 对占位 context（`fields = None`）安全：仅记录 n，不分配（无表达式
    /// 的 Struct 不会调用此方法，由编译期 `has_expressions` 标志保证）。
    ///
    /// # 性能
    ///
    /// `vec![ptr::null_mut(); n]` 零次 Py_INCREF，仅一次 Vec 堆分配
    /// （n 个指针宽度 = 8n 字节，典型 n=2-4，单次 malloc ~5ns）。
    pub fn init_expr_values(&mut self, n: usize) {
        self.expr_values = Some(vec![std::ptr::null_mut(); n]);
    }

    /// 按索引写入字段值（同时写 PyDict + expr_values）。
    ///
    /// 替代原 `set_field_interned`，增加 `idx` 参数用于同步 expr_values。
    /// 由 StructNode.parse/build 在每个 RW/RO 字段完成后调用。
    ///
    /// # 操作
    ///
    /// 1. `fields.set_item(name, value)` — 写入 PyDict（C API `PyDict_SetItem`，
    ///    interned key 命中 fast path，~10-15ns）
    /// 2. `expr_values[idx] = value.as_ptr()` — 写入 borrowed 指针（~1ns，不 incref）
    ///
    /// # 借用
    ///
    /// 需要 `&mut self`（写 expr_values）。StructNode.parse/build 中
    /// `field.node.parse(...)` 与 `ctx.set_field_at(...)` 是**顺序调用**，
    /// 不存在同时借用 ctx 的情况。
    ///
    /// 对占位 context（`fields = None`）为无操作（has_expressions=false 的
    /// Struct 不调用此方法）。
    ///
    /// # 错误
    ///
    /// `PyDict_SetItem` 失败时返回 `PyErr`（转为 `ConstructError::Generic`）。
    pub fn set_field_at(
        &mut self,
        idx: usize,
        name: &Py<PyString>,
        value: &Bound<'_, PyAny>,
        py: Python<'_>,
    ) -> PyResult<()> {
        match (&self.fields, &mut self.expr_values) {
            (Some(fields), Some(vals)) => {
                fields.set_item(name.bind(py), value)?;
                // 同步 borrowed 指针到 expr_values（不 incref，依赖 dict 持有引用）。
                if let Some(slot) = vals.get_mut(idx) {
                    *slot = value.as_ptr();
                }
                // idx 越界：编译期保证 idx < fields.len() == vals.len()。
                // 防御性忽略（不 panic，符合编码红线）。
                Ok(())
            }
            // 占位 context 或未 init_expr_values：仅写 dict（若存在），跳过 Vec。
            (Some(fields), None) => fields.set_item(name.bind(py), value),
            (None, _) => Ok(()),
        }
    }

    /// 存入字段（parse/build 每个字段完成后调用）。
    ///
    /// 对齐 Python construct `Struct._parse` 中的 `context[sc.name] = subobj`。
    ///
    /// **注意**：此方法仅写 PyDict，**不**同步 `expr_values`。仅用于测试或
    /// has_expressions=false 的路径。若在已调用 [`Context::init_expr_values`]
    /// 的 context 上用此方法覆盖将被 GetInt 引用的字段，会导致 expr_values
    /// 指针与 dict 不同步（GetInt 读取到旧值）。生产路径（has_expressions=true）
    /// 必须使用 [`Context::set_field_at`]。
    ///
    /// 占位 context（`fields = None`）上为无操作（Phase 1 不写入）。
    pub fn set_field(&self, name: &str, value: &Bound<'_, PyAny>) -> PyResult<()> {
        match &self.fields {
            Some(fields) => fields.set_item(name, value),
            None => Ok(()),
        }
    }

    /// 读取当前层字段（Phase 2 的 this.xxx 引用使用）。
    ///
    /// 返回 `None` 表示当前层无此字段。注意：此方法**不**递归查找 parent。
    /// 读取父层字段应通过 `parent()` 显式获取父 context 后调用。
    ///
    /// 占位 context（`fields = None`）始终返回 `Ok(None)`。
    pub fn get_field(&self, name: &str) -> PyResult<Option<Bound<'_, PyAny>>> {
        match &self.fields {
            Some(fields) => fields.get_item(name),
            None => Ok(None),
        }
    }

    /// 返回父 context 的引用（若有）。
    ///
    /// 对应 Python construct 中通过 `_` 访问外层 context 的能力。
    pub fn parent(&self) -> Option<&'py Context<'py>> {
        self.parent
    }

    /// 按编译期字段索引取整数值（VM `GetInt` 指令使用）。
    ///
    /// 从 `expr_values[idx]` 取 borrowed PyObject 指针，直接调用
    /// `PyLong_AsLongLong`（~5ns），跳过 `PyDict_GetItem` hash 查找（~24ns）。
    ///
    /// 完整路径：`vec[idx]`(~1ns) → `PyLong_AsLongLong`(~5ns) = **~6ns**
    /// （vs 原 `get_int_by_name` ~36ns，节省 ~30ns）。
    ///
    /// # 错误
    ///
    /// - [`ConstructError::ExprContext`]：expr_values 未初始化（占位 context，
    ///   理论不发生——有表达式的 Struct 调用 init_expr_values）。
    /// - [`ConstructError::ExprFieldMissing`]：槽位为 null（WO 字段或未设置）。
    /// - [`ConstructError::ExprType`]：值无法 `PyLong_AsLongLong`（非整数类型）。
    pub fn get_int_at(&self, idx: usize, _py: Python<'_>) -> Result<i64, ConstructError> {
        let vals = self
            .expr_values
            .as_ref()
            .ok_or(ConstructError::ExprContext {
                message: "get_int_at: expr_values not initialized (placeholder context)"
                    .to_string(),
                path: String::new(),
            })?;
        let ptr = vals.get(idx).copied().unwrap_or(std::ptr::null_mut());
        if ptr.is_null() {
            return Err(ConstructError::ExprFieldMissing {
                field: format!("<index {}>", idx),
                path: String::new(),
            });
        }
        // SAFETY: ptr 来自 set_field_at 写入的 value.as_ptr()，指向 fields dict
        // 中的值对象。dict 由 self.fields (Bound<PyDict>) 保证存活，值对象因此存活。
        // PyLong_AsLongLong 对非 PyLong 对象返回 -1 并设置 OverflowError/TypeError，
        // 我们检查返回值区分错误类型。
        let v = unsafe { ffi::PyLong_AsLongLong(ptr) };
        if v == -1 {
            // 区分"值为 -1"与"转换失败"：检查 PyErr_Occurred
            // SAFETY: 持有 GIL，PyErr_Occurred 仅检查不取异常。
            let err_occurred = unsafe { ffi::PyErr_Occurred() };
            if !err_occurred.is_null() {
                unsafe { ffi::PyErr_Clear() };
                return Err(ConstructError::ExprType {
                    field: format!("<index {}>", idx),
                    expected: "integer (i64)".to_string(),
                    path: String::new(),
                });
            }
        }
        Ok(v)
    }

    /// 取出 fields dict 的所有权（parse 末尾将 dict 移交给实例）。
    ///
    /// 调用后 `self.fields = None`，后续访问 `fields()` 返回 `None`。
    /// 仅在 parse/build 结束、ctx 不再使用时调用。
    ///
    /// 用于 has_expressions=true 的 parse 路径：将 ctx 的 dict 直接移交给
    /// 实例（避免 clone 的 Py_INCREF 开销）。
    pub fn take_fields(&mut self) -> Option<Bound<'py, PyDict>> {
        self.fields.take()
    }

    /// 借用当前层的字段字典（用于测试与调试）。
    ///
    /// 占位 context（`fields = None`）或已 [`take_fields`] 的 context 返回 `None`。
    pub fn fields(&self) -> Option<&Bound<'py, PyDict>> {
        self.fields.as_ref()
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
            assert_eq!(ctx.fields().expect("fields").len(), 0);
        });
    }

    #[test]
    fn placeholder_has_no_dict_and_no_parent() {
        with_python(|py| {
            let ctx = Context::placeholder(py);
            assert!(ctx.parent().is_none());
            assert!(
                ctx.fields().is_none(),
                "placeholder should not allocate PyDict"
            );
        });
    }

    #[test]
    fn placeholder_set_field_is_noop() {
        with_python(|py| {
            let ctx = Context::placeholder(py);
            let value = py.eval_bound("42", None, None).expect("eval 42");
            // set_field 在占位 context 上为无操作，不报错。
            ctx.set_field("count", &value).expect("set_field noop");
            assert!(ctx.get_field("count").expect("get").is_none());
        });
    }

    #[test]
    fn new_child_links_to_parent() {
        with_python(|py| {
            let root = Context::new_root(py).expect("root");
            let child = Context::new_child(&root, py).expect("child");
            assert!(child.parent().is_some());
            let parent_ref = child.parent().expect("parent should exist");
            assert_eq!(
                parent_ref.fields().expect("fields").len(),
                root.fields().expect("fields").len()
            );
        });
    }

    #[test]
    fn set_field_stores_value() {
        with_python(|py| {
            let ctx = Context::new_root(py).expect("root");
            let value = py.eval_bound("42", None, None).expect("eval 42");
            ctx.set_field("count", &value).expect("set_field");
            assert_eq!(ctx.fields().expect("fields").len(), 1);
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
            assert_eq!(ctx.fields().expect("fields").len(), 1);
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
            assert_eq!(ctx.fields().expect("fields").len(), 3);
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
            assert_eq!(child.fields().expect("fields").len(), 0);
            assert_eq!(root.fields().expect("fields").len(), 1);
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
            assert_eq!(ctx.fields().expect("fields").len(), 1);
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

    // ======================================================================
    // init_expr_values / set_field_at / get_int_at（Phase 2.5 Vec 化）
    // ======================================================================

    #[test]
    fn init_expr_values_creates_null_slots() {
        with_python(|py| {
            let mut ctx = Context::new_root(py).expect("root");
            ctx.init_expr_values(3);
            // 验证 expr_values 已初始化（通过 get_int_at 返回 ExprFieldMissing
            // 而非 ExprContext 来间接验证）。
            let result = ctx.get_int_at(0, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprFieldMissing { .. }) => {}
                other => panic!("expected ExprFieldMissing for null slot, got {:?}", other),
            }
        });
    }

    #[test]
    fn set_field_at_stores_value_in_dict_and_vec() {
        with_python(|py| {
            let mut ctx = Context::new_root(py).expect("root");
            ctx.init_expr_values(1);
            let key = PyString::new_bound(py, "count").unbind();
            let value = py.eval_bound("42", None, None).expect("eval 42");
            ctx.set_field_at(0, &key, &value, py).expect("set_field_at");
            // 验证 dict 写入
            assert_eq!(ctx.fields().expect("fields").len(), 1);
            // 验证 Vec 写入（通过 get_int_at）
            assert_eq!(ctx.get_int_at(0, py).expect("get_int_at"), 42);
        });
    }

    #[test]
    fn set_field_at_on_placeholder_is_noop() {
        with_python(|py| {
            let mut ctx = Context::placeholder(py);
            ctx.init_expr_values(1);
            let key = PyString::new_bound(py, "x").unbind();
            let value = py.eval_bound("1", None, None).expect("eval 1");
            // 占位 context 上 set_field_at 为无操作，不报错。
            ctx.set_field_at(0, &key, &value, py)
                .expect("noop on placeholder");
            assert!(ctx.fields().is_none());
            // placeholder 的 fields=None，get_int_at 会因 expr_values 初始化但指针 null
            // 返回 ExprFieldMissing（因为 set_field_at 在 (None, _) 分支不写 Vec）。
            let result = ctx.get_int_at(0, py);
            assert!(result.is_err());
        });
    }

    #[test]
    fn set_field_at_without_init_writes_dict_only() {
        with_python(|py| {
            let mut ctx = Context::new_root(py).expect("root");
            // 未调用 init_expr_values → expr_values = None
            let key = PyString::new_bound(py, "x").unbind();
            let value = py.eval_bound("5", None, None).expect("eval 5");
            ctx.set_field_at(0, &key, &value, py).expect("set_field_at");
            // dict 已写入
            assert_eq!(ctx.fields().expect("fields").len(), 1);
            // 但 get_int_at 返回 ExprContext（expr_values 未初始化）
            let result = ctx.get_int_at(0, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprContext { .. }) => {}
                other => panic!("expected ExprContext, got {:?}", other),
            }
        });
    }

    #[test]
    fn set_field_at_overrides_existing() {
        with_python(|py| {
            let mut ctx = Context::new_root(py).expect("root");
            ctx.init_expr_values(1);
            let key = PyString::new_bound(py, "x").unbind();
            let v1 = py.eval_bound("1", None, None).expect("eval 1");
            ctx.set_field_at(0, &key, &v1, py).expect("set v1");
            let v2 = py.eval_bound("2", None, None).expect("eval 2");
            ctx.set_field_at(0, &key, &v2, py).expect("set v2");
            assert_eq!(ctx.fields().expect("fields").len(), 1);
            assert_eq!(ctx.get_int_at(0, py).expect("get_int_at"), 2);
        });
    }

    #[test]
    fn get_int_at_returns_correct_value() {
        with_python(|py| {
            let mut ctx = Context::new_root(py).expect("root");
            ctx.init_expr_values(3);
            let values = [10i64, 20, 30];
            let names = ["a", "b", "c"];
            for (i, (&n, &v)) in names.iter().zip(values.iter()).enumerate() {
                let key = PyString::new_bound(py, n).unbind();
                let val = v.into_py(py);
                ctx.set_field_at(i, &key, val.bind(py), py).expect("set");
            }
            assert_eq!(ctx.get_int_at(0, py).expect("idx 0"), 10);
            assert_eq!(ctx.get_int_at(1, py).expect("idx 1"), 20);
            assert_eq!(ctx.get_int_at(2, py).expect("idx 2"), 30);
        });
    }

    #[test]
    fn get_int_at_negative_value() {
        with_python(|py| {
            let mut ctx = Context::new_root(py).expect("root");
            ctx.init_expr_values(1);
            let key = PyString::new_bound(py, "offset").unbind();
            let val = (-100i64).into_py(py);
            ctx.set_field_at(0, &key, val.bind(py), py).expect("set");
            // 值为 -1 边界场景的特殊验证（PyLong_AsLongLong 歧义）
            assert_eq!(ctx.get_int_at(0, py).expect("get negative"), -100);
        });
    }

    #[test]
    fn get_int_at_minus_one_value() {
        with_python(|py| {
            let mut ctx = Context::new_root(py).expect("root");
            ctx.init_expr_values(1);
            let key = PyString::new_bound(py, "x").unbind();
            let val = (-1i64).into_py(py);
            ctx.set_field_at(0, &key, val.bind(py), py).expect("set");
            // -1 是 PyLong_AsLongLong 的歧义值，必须正确区分"-1 合法值"与"转换失败"
            assert_eq!(ctx.get_int_at(0, py).expect("get -1"), -1);
        });
    }

    #[test]
    fn get_int_at_i64_max() {
        with_python(|py| {
            let mut ctx = Context::new_root(py).expect("root");
            ctx.init_expr_values(1);
            let key = PyString::new_bound(py, "max").unbind();
            let val = i64::MAX.into_py(py);
            ctx.set_field_at(0, &key, val.bind(py), py).expect("set");
            assert_eq!(ctx.get_int_at(0, py).expect("get max"), i64::MAX);
        });
    }

    #[test]
    fn get_int_at_null_slot_returns_field_missing() {
        // WO 字段的槽位保持 null → ExprFieldMissing
        with_python(|py| {
            let mut ctx = Context::new_root(py).expect("root");
            ctx.init_expr_values(3);
            // 只写入 idx 0 和 2（跳过 idx 1，模拟 WO 字段）
            let key_a = PyString::new_bound(py, "a").unbind();
            let val_a = 1i64.into_py(py);
            ctx.set_field_at(0, &key_a, val_a.bind(py), py)
                .expect("set a");
            let key_c = PyString::new_bound(py, "c").unbind();
            let val_c = 3i64.into_py(py);
            ctx.set_field_at(2, &key_c, val_c.bind(py), py)
                .expect("set c");
            // idx 1 为 null
            let result = ctx.get_int_at(1, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprFieldMissing { field, .. }) => {
                    assert!(
                        field.contains("1"),
                        "field should contain index 1: {}",
                        field
                    );
                }
                other => panic!("expected ExprFieldMissing, got {:?}", other),
            }
        });
    }

    #[test]
    fn get_int_at_out_of_bounds_returns_field_missing() {
        // idx 越界 → vals.get(idx) 返回 None → null → ExprFieldMissing
        with_python(|py| {
            let mut ctx = Context::new_root(py).expect("root");
            ctx.init_expr_values(1);
            let key = PyString::new_bound(py, "a").unbind();
            let val = 1i64.into_py(py);
            ctx.set_field_at(0, &key, val.bind(py), py).expect("set a");
            // idx 5 越界（vec 只有 1 个槽位）
            let result = ctx.get_int_at(5, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprFieldMissing { field, .. }) => {
                    assert!(
                        field.contains("5"),
                        "field should contain index 5: {}",
                        field
                    );
                }
                other => panic!("expected ExprFieldMissing, got {:?}", other),
            }
        });
    }

    #[test]
    fn get_int_at_on_placeholder_returns_expr_context() {
        with_python(|py| {
            let ctx = Context::placeholder(py);
            let result = ctx.get_int_at(0, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprContext { message, .. }) => {
                    assert!(message.contains("not initialized"));
                }
                other => panic!("expected ExprContext, got {:?}", other),
            }
        });
    }

    #[test]
    fn get_int_at_uninitialized_returns_expr_context() {
        with_python(|py| {
            let ctx = Context::new_root(py).expect("root");
            // 未调用 init_expr_values
            let result = ctx.get_int_at(0, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprContext { message, .. }) => {
                    assert!(message.contains("not initialized"));
                }
                other => panic!("expected ExprContext, got {:?}", other),
            }
        });
    }

    #[test]
    fn get_int_at_wrong_type_returns_expr_type() {
        // 存入 str 值而非 int → PyLong_AsLongLong 失败 → ExprType
        with_python(|py| {
            let mut ctx = Context::new_root(py).expect("root");
            ctx.init_expr_values(1);
            let key = PyString::new_bound(py, "name").unbind();
            let str_val = PyString::new_bound(py, "hello");
            ctx.set_field_at(0, &key, &str_val, py).expect("set str");
            let result = ctx.get_int_at(0, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprType { expected, .. }) => {
                    assert!(expected.contains("integer"));
                }
                other => panic!("expected ExprType, got {:?}", other),
            }
        });
    }

    #[test]
    fn get_int_at_overflow_returns_expr_type() {
        // 存入超大整数（超出 i64 范围）→ PyLong_AsLongLong 返回 -1 + OverflowError
        with_python(|py| {
            let mut ctx = Context::new_root(py).expect("root");
            ctx.init_expr_values(1);
            let key = PyString::new_bound(py, "big").unbind();
            // 2**63 + 1 = 9223372036854775809，超出 i64::MAX
            let val = py
                .eval_bound("2**63 + 1", None, None)
                .expect("eval big int");
            ctx.set_field_at(0, &key, &val, py).expect("set big");
            let result = ctx.get_int_at(0, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprType { .. }) => {}
                other => panic!("expected ExprType for overflow, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // take_fields
    // ======================================================================

    #[test]
    fn take_fields_moves_dict_ownership() {
        with_python(|py| {
            let mut ctx = Context::new_root(py).expect("root");
            let value = py.eval_bound("42", None, None).expect("eval");
            ctx.set_field("x", &value).expect("set");
            let dict = ctx.take_fields().expect("should have dict");
            assert_eq!(dict.len(), 1);
            // take 后 fields() 返回 None
            assert!(ctx.fields().is_none());
        });
    }

    #[test]
    fn take_fields_on_placeholder_returns_none() {
        with_python(|py| {
            let mut ctx = Context::placeholder(py);
            assert!(ctx.take_fields().is_none());
        });
    }

    #[test]
    fn take_fields_twice_second_returns_none() {
        with_python(|py| {
            let mut ctx = Context::new_root(py).expect("root");
            let _first = ctx.take_fields().expect("first take");
            assert!(ctx.take_fields().is_none(), "second take should be None");
        });
    }
}
