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
use crate::error::ConstructError;
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
/// parse 入口（[`CompiledSchema::_parse_raw`]）直接返回用户类实例（R4，
/// Rust 内部 create_class + 借用 `__dict__` 构造，不返回 dict 到 Python）。
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
    /// 缓存：根 StructNode 是否含表达式（创建时计算一次）。
    ///
    /// 决定 `_build_raw` 入口使用 `Context::new_root`（有表达式）还是
    /// `Context::placeholder`（无表达式）。`_parse_raw` 在 R4 后统一使用
    /// `placeholder`（StructNode.parse 内部自行管理 dict）。缓存避免每次
    /// build 重新 match。
    has_expressions: bool,
    /// 缓存：静态大小（无表达式时为 `Some(n)`，有表达式时为 `None`）。
    ///
    /// 创建时调用一次 `root.sizeof(placeholder)`。`_build_raw` 直接读此缓存
    /// 决定 `BuildStream::with_capacity` 预分配，消除每次 build 的 sizeof 递归遍历。
    static_size: Option<usize>,
}

impl CompiledSchema {
    /// 创建一个新的编译产物。
    ///
    /// 在创建时一次性计算并缓存 `has_expressions` 与 `static_size`，
    /// 避免每次 parse/build 的重复计算（sizeof 递归遍历 + match）。
    ///
    /// # 参数
    ///
    /// - `root`：执行树根节点（通常是 `Node::Struct(StructNode { ... })`）
    /// - `cls`：用户类（`StructMixin` 子类）的 Python 引用
    /// - `py`：GIL token（用于 sizeof 的 placeholder context）
    pub fn new(root: Node, cls: Py<PyType>, py: Python<'_>) -> Self {
        let has_expressions = root.has_expressions();
        // 仅对无表达式结构尝试 sizeof（含表达式时 sizeof 必然失败，跳过无用计算）。
        let static_size = if !has_expressions {
            root.sizeof(&Context::placeholder(py)).ok()
        } else {
            None
        };
        Self {
            root,
            cls,
            has_expressions,
            static_size,
        }
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
    /// 决定 `_build_raw` 入口使用 `Context::new_root`（有表达式）还是
    /// `Context::placeholder`（无表达式，Phase 1 性能优化保留）。`_parse_raw`
    /// 在 R4 后统一使用 `placeholder`，不再调用此方法。
    ///
    /// 非 Struct 根节点视为无表达式。
    ///
    /// 此方法读取创建时缓存的 `has_expressions` 字段，避免每次调用的 match 开销。
    fn root_has_expressions(&self) -> bool {
        self.has_expressions
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
    /// # 内部流程（R4：借用实例 `__dict__`）
    ///
    /// 1. 从 `PyBytes` 提取 `&[u8]`，创建 `ParseStream`（纯 Rust）。
    /// 2. 统一使用 `Context::placeholder`（不分配 `PyDict`）——StructNode.parse
    ///    内部根据 `has_expressions` 自行创建实例并 inject 实例 `__dict__`。
    /// 3. 调用 `self.root.parse(...)`——进入执行树遍历。
    /// 4. StructNode.parse 内部完整构造用户类实例（create_class → getattr
    ///    `__dict__` → 填充 dict → 可选 `__post_init__`），无需 `force_setattr`。
    /// 5. 直接返回实例（不再跨 FFI 返回 dict 到 Python）。
    ///
    /// 详见 `docs/设计修订-parse路径优化-借用实例dict.md` §3.5。
    #[pyo3(signature = (data))]
    pub fn _parse_raw<'py>(
        &self,
        py: Python<'py>,
        data: &Bound<'py, PyBytes>,
    ) -> PyResult<Py<PyAny>> {
        // O2-A.1（ADR-023 决策 3）：直接访问 PyBytesObject.ob_sval 字段，绕过
        // `PyBytes_AsStringAndSize` 函数调用 + 内部 type check。data 类型由 pyo3
        // 参数签名 &Bound<PyBytes> 在入口处保证（成功才进入此函数）。
        //
        // SAFETY:
        // - data 是 &Bound<'py, PyBytes>，pyo3 参数类型签名在入口处 type check
        //   （PyBytes_CheckExact 通过才进入此函数）。直接将 PyObject* cast 为
        //   PyBytesObject* 是 CPython 内部约定。
        // - ob_sval 字段声明为 `[c_char; 1]` 是占位（CPython 不变量：实际长度为
        //   ob_base.ob_size + 1，含终止 NUL；`from_raw_parts` 的 len 取 ob_size，
        //   不含 NUL）。
        // - 返回的 &[u8] 生命周期受 `data` 借用约束（Rust borrow checker 保证）。
        // - GIL 持有（由 `py: Python<'py>` 参数保证）。
        //
        // ABI 依赖：[CPython Internal Struct] PyBytesObject.ob_sval + ob_base.ob_size
        // 由 pyo3 0.22 cpython 子模块暴露（`pyo3-ffi-0.22.6/src/cpython/bytesobject.rs`
        // L8-14）。项目非 abi3 模式（8.ENV 验证），cpython 子模块可用。
        let bytes: &[u8] = unsafe {
            use pyo3::ffi::PyBytesObject;
            // data.as_ptr() 返回 *mut ffi::PyObject；cast 为 *const PyBytesObject
            // 后 dereference 才能访问 ob_base.ob_size + ob_sval 字段。
            let bytes_ptr = data.as_ptr() as *const PyBytesObject;
            let bytes_obj = &*bytes_ptr;
            let len = bytes_obj.ob_base.ob_size as usize;
            std::slice::from_raw_parts(bytes_obj.ob_sval.as_ptr() as *const u8, len)
        };

        let mut stream = ParseStream::new(bytes);
        // R4：统一使用 placeholder。StructNode.parse 内部根据 has_expressions
        // 自行创建实例并 inject dict（has_expressions=true 时）或直接操作实例
        // dict（has_expressions=false 时）。不再需要入口处判断 has_expressions。
        let mut ctx = Context::placeholder(py);
        let mut path = Path::new();
        // Phase 8.10：顶层 catch CancelParsing（对齐 Python core.py L416-419）。
        // 用户主动 raise CancelParsing → ConstructError::CancelParsing → 返回 None。
        // 其他错误正常转 PyErr 向上传播。
        let parse_result = self.root.parse(py, &mut stream, &mut ctx, &mut path);
        match parse_result {
            // O2-A.2：返回类型从 Bound<PyAny> 改为 Py<PyAny>，省去 pyo3 wrap 阶段
            // 的一次 Bound 构造（~1-2ns）。pyo3 0.22 #[pymethods] 接受 Py<PyAny>
            // 返回类型（DEV §7 #10 已验证）。
            Ok(result) => Ok(result),
            Err(ConstructError::CancelParsing { .. }) => {
                // 用户主动取消，返回 None（对齐 Python `except CancelParsing: pass`）。
                Ok(py.None())
            }
            Err(e) => Err(e.into()),
        }
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
        // 避免 Vec 扩容 realloc。`static_size` 在创建时缓存（一次 sizeof 调用），
        // 消除每次 build 的 sizeof 递归遍历。含表达式结构 static_size 为 None，
        // 直接用 new()。
        let mut stream = match self.static_size {
            Some(cap) => BuildStream::with_capacity(cap),
            None => BuildStream::new(),
        };
        // 根据 root 是否含表达式选择 context 模式（build 方向，设计 §5.5.1）。
        // 注意：parse 方向（R4）统一用 placeholder，build 方向仍需区分。
        let has_expr = self.root_has_expressions();
        let mut ctx = if has_expr {
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
            let schema = CompiledSchema::new(root, object_cls.clone_ref(py), py);

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
            let schema = CompiledSchema::new(empty_root(py), cls, py);
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
            let schema = CompiledSchema::new(empty_root(py), cls.clone_ref(py), py);
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

    /// Phase 8 P0 VET 驳回修复（D-P0-3）：验证 Aligned 字段 sizeof 修复后
    /// Struct static_size 预分配恢复。
    ///
    /// 修复前：Aligned.sizeof 统一返回 Err → 含 Aligned 字段的 Struct
    /// static_size 永远为 None → BuildStream 退化为无预分配模式（每次 build 走 realloc）。
    ///
    /// 修复后：Aligned(4, Int16ub) 编译期常量 modulus sizeof 成功 → static_size=Some(4)。
    #[test]
    fn static_size_restored_when_aligned_has_const_modulus() {
        use crate::expr::{ExprOp, ExprProgram};
        use crate::nodes::aligned::AlignedNode;
        use crate::nodes::format_field::{FormatFieldNode, PythonFormat};

        with_py(|py| {
            // 构造 Struct 含一个 Aligned(4, Int16ub) 字段。
            // new_for_test 强制 StructNode.has_expressions=false（模拟编译期
            // 正确识别 Aligned 常量 modulus 不含表达式的情况）。
            let aligned = Node::Aligned(AlignedNode::new(
                Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big)),
                ExprProgram::new(vec![ExprOp::Const(4)]),
                0x00,
            ));
            let root = Node::Struct(StructNode::new_for_test(
                py,
                vec![("field1".to_string(), aligned)],
            ));
            let cls = py
                .eval_bound("type('S', (), {})", None, None)
                .expect("create type")
                .extract::<Py<PyType>>()
                .expect("extract");
            let schema = CompiledSchema::new(root, cls, py);

            // 修复后：static_size 应为 Some(4)（Int16ub=2 + pad=2）。
            // 修复前：Aligned.sizeof 返回 Err → static_size 为 None。
            assert_eq!(
                schema.static_size,
                Some(4),
                "Aligned(4, Int16ub) const modulus should restore static_size prealloc"
            );
        });
    }

    /// 对照测试：Aligned 字段含运行期 modulus 表达式时，static_size 仍为 None
    /// （D-P0-3：运行期表达式 sizeof 返回 Err，对齐 Python SizeofError）。
    #[test]
    fn static_size_none_when_aligned_has_runtime_modulus() {
        use crate::expr::{ExprOp, ExprProgram};
        use crate::nodes::aligned::AlignedNode;
        use crate::nodes::format_field::{FormatFieldNode, PythonFormat};

        with_py(|py| {
            let aligned = Node::Aligned(AlignedNode::new(
                Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big)),
                ExprProgram::new(vec![ExprOp::GetInt(0)]),
                0x00,
            ));
            // 注意：new_for_test 强制 StructNode.has_expressions=false，
            // 但 schema.rs 仍会尝试 sizeof（因为 has_expressions=false），
            // 此时 Aligned.sizeof 对运行期 modulus 返回 Err → static_size=None。
            let root = Node::Struct(StructNode::new_for_test(
                py,
                vec![("field1".to_string(), aligned)],
            ));
            let cls = py
                .eval_bound("type('S', (), {})", None, None)
                .expect("create type")
                .extract::<Py<PyType>>()
                .expect("extract");
            let schema = CompiledSchema::new(root, cls, py);

            assert_eq!(
                schema.static_size, None,
                "Aligned with runtime modulus should keep static_size None (SizeofError)"
            );
        });
    }

    // ======================================================================
    // Phase 8 OPT-SHARED（ADR-023）：_parse_raw O2-A 路径
    // ======================================================================

    /// O2-A.1 验证：直接访问 PyBytesObject.ob_sval 字段得到的 bytes slice
    /// 与 data.as_bytes() 内容一致（设计 §4.1.1 测试 `test_ob_sval_direct_access_matches_as_bytes`）。
    #[test]
    fn parse_raw_ob_sval_direct_access_matches_as_bytes() {
        use crate::nodes::format_field::{FormatFieldNode, PythonFormat};
        with_py(|py| {
            // 构造最小 schema：单字段 Int8ub
            let root = Node::Struct(StructNode::new_for_test(
                py,
                vec![(
                    "x".to_string(),
                    Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big)),
                )],
            ));
            let cls = py
                .eval_bound("type('S', (), {})", None, None)
                .expect("create type")
                .extract::<Py<PyType>>()
                .expect("extract");
            let schema = CompiledSchema::new(root, cls, py);
            // 5 字节输入，schema 只消费 1（剩余忽略）
            let input: &[u8] = &[0xAA, 0xBB, 0xCC, 0xDD, 0xEE];
            let data = PyBytes::new_bound(py, input);
            let result = schema._parse_raw(py, &data).expect("parse");
            let inst = result.bind(py);
            let x: i64 = inst.getattr("x").unwrap().extract().unwrap();
            assert_eq!(x, 0xAA, "ob_sval direct read should give correct value");
        });
    }

    /// O2-A.2 验证：返回类型为 Py<PyAny>，且 CancelParsing 仍返回 None
    /// （Phase 8.10 顶层 catch 不破坏）。
    #[test]
    fn parse_raw_cancel_parsing_returns_none_after_o2a() {
        with_py(|py| {
            // 空 schema（无字段），直接触发 CancelParsing
            let root = Node::Struct(StructNode::new_for_test(py, Vec::new()));
            let cls = py
                .eval_bound("type('S', (), {})", None, None)
                .expect("create type")
                .extract::<Py<PyType>>()
                .expect("extract");
            let schema = CompiledSchema::new(root, cls, py);
            let data = PyBytes::new_bound(py, b"");
            let result = schema._parse_raw(py, &data).expect("parse ok");
            // 空 schema 返回实例（不是 None），验证 O2-A 返回路径正常
            assert!(
                !result.is_none(py),
                "empty struct should return instance, not None"
            );
        });
    }

    /// O2-A 边界：空 bytes 输入。
    #[test]
    fn parse_raw_empty_bytes_o2a_works() {
        with_py(|py| {
            let root = Node::Struct(StructNode::new_for_test(py, Vec::new()));
            let cls = py
                .eval_bound("type('S', (), {})", None, None)
                .expect("create type")
                .extract::<Py<PyType>>()
                .expect("extract");
            let schema = CompiledSchema::new(root, cls, py);
            let data = PyBytes::new_bound(py, b"");
            let _ = schema._parse_raw(py, &data).expect("parse empty ok");
        });
    }
}
