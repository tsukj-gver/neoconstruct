//! 编译产物 [`CompiledSchema`]：不可变的执行树，关联到用户类。
//!
//! 设计依据：`docs/架构设计.md` §B.6。
//!
//! ## 职责
//!
//! [`CompiledSchema`] 是编译入口（`compile_schema`，子任务 1.5）的输出，
//! parse/build 入口（`_parse_raw` / `_build_raw`，子任务 1.5）的载体。
//! 它在 `__init_subclass__` 中一次性创建，存为类属性 `cls._construct_compiled`，
//! 之后每次 parse/build 通过类属性查找零开销地访问。
//!
//! ## Phase 1.4 范围
//!
//! 本子任务仅定义结构体与 Rust 内部访问方法（[`CompiledSchema::new`]、
//! [`CompiledSchema::root`]、[`CompiledSchema::cls`]）。Python 可见的
//! parse/build 方法（`_parse_raw` / `_build_raw`）在子任务 1.5 的
//! `parse.rs` / `build.rs` 中作为独立 `#[pymethods]` 块实现。

use crate::nodes::Node;
use pyo3::prelude::*;
use pyo3::types::PyType;

/// 编译产物：一棵不可变的执行树，关联到用户类。
///
/// `#[pyclass(frozen)]` 使 Python 侧无法修改其属性。Rust 侧 `root`（[`Node`]）
/// 与 `cls` 不可变，运行时不允许增删节点或修改参数。
///
/// parse 入口返回字段 dict（Rust 通过 C API 构造的 `PyDict`），Python 侧用
/// `cls(**dict)` 构造实例。嵌套字段的实例化由 [`crate::nodes::struct_ref::StructRefNode`]
/// 在 parse 内部通过 Rust→Python 回调完成。
///
/// # 线程安全
///
/// `CompiledSchema` 是 frozen pyclass，内部 [`Node`] 不可变，可被多线程安全共享
/// （在 GIL 约束下）。parse/build 调用无副作用，可并发。
#[pyclass(frozen, name = "CompiledSchema")]
pub struct CompiledSchema {
    /// 执行树根节点（通常是 [`Node::Struct`]）。
    root: Node,
    /// 用户类的 Python 引用（`StructMixin` 子类）。
    ///
    /// 用于：
    /// - 调试与错误信息中报告类名；
    /// - parse 时 Python 侧的 `cls(**dict)`（Python 侧已持有 cls，此处作为元数据）。
    cls: Py<PyType>,
}

impl CompiledSchema {
    /// 创建一个新的编译产物。
    ///
    /// # 参数
    ///
    /// - `root`：执行树根节点（通常是 `Node::Struct(StructNode { ... })`）
    /// - `cls`：用户类（`StructMixin` 子类）的 Python 引用
    pub fn new(root: Node, cls: Py<PyType>) -> Self {
        Self { root, cls }
    }

    /// 获取根节点引用（供 [`crate::nodes::struct_ref::StructRefNode`] 递归调用）。
    ///
    /// StructRefNode 在 parse/build 时通过此方法获取对方的执行树根，
    /// 递归遍历以处理嵌套结构。
    pub fn root(&self) -> &Node {
        &self.root
    }

    /// 获取用户类引用。
    pub fn cls(&self) -> &Py<PyType> {
        &self.cls
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::struct_node::StructNode;
    use crate::nodes::Node;

    /// 初始化 Python 解释器（幂等）。
    fn ensure_python() {
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(pyo3::prepare_freethreaded_python);
    }

    fn with_py<F, R>(f: F) -> R
    where
        F: for<'py> FnOnce(Python<'py>) -> R,
    {
        ensure_python();
        Python::with_gil(f)
    }

    /// 构造一个最小的 Struct 根节点（无字段），用于测试。
    fn empty_root() -> Node {
        Node::Struct(StructNode::new(Vec::new()))
    }

    #[test]
    fn new_stores_root_and_cls() {
        with_py(|py| {
            let builtins = py.import_bound("builtins").expect("import builtins");
            let object_cls = builtins
                .getattr("object")
                .expect("get object")
                .extract::<Py<PyType>>()
                .expect("extract type");

            let root = empty_root();
            let schema = CompiledSchema::new(root, object_cls.clone_ref(py));

            // root 返回的 Node 是 Struct 变体
            assert!(matches!(schema.root(), Node::Struct(s) if s.is_empty()));
            // cls 与传入一致（指针相等）
            assert!(schema.cls().is(&object_cls));
        });
    }

    #[test]
    fn root_returns_struct_variant_for_empty_schema() {
        with_py(|py| {
            let cls = py
                .eval_bound("type('Empty', (), {})", None, None)
                .expect("create type")
                .extract::<Py<PyType>>()
                .expect("extract");
            let schema = CompiledSchema::new(empty_root(), cls);
            match schema.root() {
                Node::Struct(s) => assert_eq!(s.len(), 0),
                _ => panic!("expected Node::Struct"),
            }
        });
    }

    #[test]
    fn pyclass_can_be_instantiated_in_python_heap() {
        // 验证 CompiledSchema 能通过 Py::new 创建到 Python 堆，
        // 且之后能 setattr 到另一个 Python 类上（StructRef 测试的基础）。
        with_py(|py| {
            let cls = py
                .eval_bound("type('Dummy', (), {})", None, None)
                .expect("create type")
                .extract::<Py<PyType>>()
                .expect("extract");
            let schema = CompiledSchema::new(empty_root(), cls.clone_ref(py));
            let schema_py = Py::new(py, schema).expect("Py::new");

            // setattr 到 cls
            cls.bind(py)
                .setattr("_construct_compiled", schema_py.clone_ref(py))
                .expect("setattr");

            // 读取回来验证类型
            let retrieved = cls
                .bind(py)
                .getattr("_construct_compiled")
                .expect("getattr");
            assert!(
                retrieved.extract::<Py<CompiledSchema>>().is_ok(),
                "retrieved attribute should be a CompiledSchema"
            );
        });
    }
}
