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
//! ## Phase 1.4-1.6 范围
//!
//! - **1.4**：定义结构体与 Rust 内部访问方法（[`CompiledSchema::new`]、
//!   [`CompiledSchema::root`]、[`CompiledSchema::cls`]）。
//! - **1.5-1.6**：添加 Python 可见的 parse/build 方法（`_parse_raw` / `_build_raw`）
//!   作为 `#[pymethods]` 块，提供 parse/build 的 FFI 执行入口。

// pyo3 0.22 的 #[pymethods] 宏在展开返回 PyResult<T> 的包装代码时，
// 会生成 `PyErr.into()` 形式的冗余转换，触发 clippy::useless_conversion。
// 这是宏的已知行为（不是用户代码问题），在此模块级别抑制该 lint。
// 后续升级 pyo3 版本后若修复，可移除此属性。
#![allow(clippy::useless_conversion)]

use crate::context::Context;
use crate::nodes::{Construct, Node};
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyBytes, PyType};

/// 编译产物：一棵不可变的执行树，关联到用户类。
///
/// `#[pyclass(frozen)]` 使 Python 侧无法修改其属性。Rust 侧 `root`（[`Node`]）
/// 与 `cls` 不可变，运行时不允许增删节点或修改参数。
///
/// parse 入口（[`CompiledSchema::_parse_raw`]）直接返回用户类实例（方案 B'，
/// Rust 内部 create_class + force_setattr 构造，不返回 dict 到 Python）。
/// 嵌套字段的实例化由 [`crate::nodes::struct_node::StructNode`] 与
/// [`crate::nodes::struct_ref::StructRefNode`] 在 Rust 内部递归完成，
/// 全程无 Python 回调。
///
/// # 线程安全
///
/// `CompiledSchema` 是 frozen pyclass，内部 [`Node`] 不可变，可被多线程安全共享
/// （在 GIL 约束下）。parse/build 调用无副作用，可并发。
#[pyclass(frozen, name = "CompiledSchema")]
#[derive(Debug)]
pub struct CompiledSchema {
    /// 执行树根节点（通常是 [`Node::Struct`]）。
    root: Node,
    /// 用户类的 Python 引用（`StructMixin` 子类）。
    ///
    /// 用于：调试与错误信息中报告类名。Rust 内部由 StructNode 持有独立的
    /// `cls` 引用（详见设计修订 §3.2 根节点关系说明），此字段为元数据。
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

    /// 判断根节点（StructNode）是否含有表达式字段。
    ///
    /// 决定 `_parse_raw` / `_build_raw` 入口使用 `Context::new_root`（有表达式）
    /// 还是 `Context::placeholder`（无表达式，Phase 1 性能优化保留）。
    ///
    /// 非 Struct 根节点视为无表达式。
    fn root_has_expressions(&self) -> bool {
        match &self.root {
            Node::Struct(s) => s.has_expressions(),
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Python 可见方法（parse / build FFI 入口）
// ---------------------------------------------------------------------------

#[pymethods]
impl CompiledSchema {
    /// 从字节解析为用户类实例（parse FFI 入口，恰好一次 FFI 穿越）。
    ///
    /// Python 可见签名：
    /// ```python
    /// _parse_raw(self, data: bytes) -> Any
    /// ```
    ///
    /// # 内部流程（方案 B'）
    ///
    /// 1. 从 `PyBytes` 提取 `&[u8]`，创建 `ParseStream`（纯 Rust）。
    /// 2. 根据 root 是否含表达式选择 Context：
    ///    - 有表达式 → `Context::new_root`（创建 PyDict，供表达式求值）
    ///    - 无表达式 → `Context::placeholder`（零开销，Phase 1 优化保留）
    /// 3. 调用 `self.root.parse(...)`——进入执行树遍历。
    /// 4. StructNode.parse 内部完整构造用户类实例（create_class + force_setattr）。
    /// 5. 直接返回实例（不再跨 FFI 返回 dict 到 Python）。
    ///
    /// 详见 `docs/设计修订-parse路径优化.md` §3.1、`docs/模块设计-表达式系统.md` §5.5。
    #[pyo3(signature = (data))]
    pub fn _parse_raw<'py>(
        &self,
        py: Python<'py>,
        data: &Bound<'py, PyBytes>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let bytes = data.as_bytes();
        let mut stream = ParseStream::new(bytes);
        // 根据 root 是否含表达式选择 context 模式（设计 §5.5.1）。
        let mut ctx = if self.root_has_expressions() {
            Context::new_root(py)?
        } else {
            // P0-2：无表达式时使用占位 context（不分配 PyDict）。
            Context::placeholder(py)
        };
        let mut path = Path::new();
        // root.parse 返回用户类实例（StructNode 内部 create_class + force_setattr）
        let result = self.root.parse(py, &mut stream, &mut ctx, &mut path)?;
        Ok(result.into_bound(py))
    }

    /// 从 Python 对象构建字节（build FFI 入口，恰好一次 FFI 穿越）。
    ///
    /// Python 可见签名：
    /// ```python
    /// _build_raw(self, obj: Any) -> bytes
    /// ```
    ///
    /// # 内部流程
    ///
    /// 1. 创建 `BuildStream`（纯 Rust 输出缓冲）。
    /// 2. 根据 root 是否含表达式选择 Context（同 `_parse_raw`）。
    /// 3. 调用 `self.root.build(obj, ...)`——遍历执行树，通过 C API 读取属性。
    /// 4. 将 `BuildStream` 的字节缓冲转为 `PyBytes` 返回。
    pub fn _build_raw<'py>(
        &self,
        py: Python<'py>,
        obj: &Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyBytes>> {
        // PERF-1 优化：对无表达式的结构（sizeof 可计算），预分配 BuildStream 容量
        // 避免 Vec 扩容 realloc。含表达式的结构 sizeof 必然失败（Bytes(Expr) 无法静态求值），
        // 直接用 new() 跳过无用的 sizeof 调用 + String 堆分配。
        let mut stream = if !self.root_has_expressions() {
            match self.root.sizeof(&Context::placeholder(py)) {
                Ok(cap) => BuildStream::with_capacity(cap),
                Err(_) => BuildStream::new(),
            }
        } else {
            BuildStream::new()
        };
        // 根据 root 是否含表达式选择 context 模式（设计 §5.5.1）。
        let mut ctx = if self.root_has_expressions() {
            Context::new_root(py)?
        } else {
            Context::placeholder(py)
        };
        let mut path = Path::new();
        self.root.build(py, obj, &mut stream, &mut ctx, &mut path)?;
        Ok(PyBytes::new_bound(py, &stream.into_bytes()))
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
    fn empty_root(py: Python<'_>) -> Node {
        // 用 object 作为 cls（无字段的 mock 类），has_post_init=false。
        let cls = py
            .import_bound("builtins")
            .expect("import builtins")
            .getattr("object")
            .expect("get object")
            .extract::<Py<PyType>>()
            .expect("extract type");
        Node::Struct(StructNode::new(py, Vec::new(), cls, false, false))
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

            let root = empty_root(py);
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
            let schema = CompiledSchema::new(empty_root(py), cls);
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
            let schema = CompiledSchema::new(empty_root(py), cls.clone_ref(py));
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
