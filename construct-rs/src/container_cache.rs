//! Container 类缓存：RepeatUntil PyCallable 路径的 context proxy 类型。
//!
//! 设计依据：
//! - `docs/模块设计-Array.md` §2.5 决策 A5 v4 + §4.3.3 `build_context_proxy`
//! - `docs/设计决策记录.md` Phase 4 决策 4
//!
//! ## 概述
//!
//! Python construct 的 `RepeatUntil._parse/_build`（core.py L2681/L2697）将
//! Container 实例作为 context 传给谓词，Container 通过 `self.__dict__ = self`
//! （containers.py L110）支持 attribute 访问（如 `ctx.threshold`）。
//!
//! construct-rs 在 RepeatUntil PyCallable 路径上对齐此语义：每次迭代构造
//! Container proxy（含 `ctx.fields()` 全部字段 + `_index`）。
//!
//! 为避免每次构造 proxy 都跨 FFI 查找 `Container` 类，本模块在
//! [`crate::_construct_rust`] 模块初始化时一次性 import `Container` 类并缓存
//! 为 [`Py<PyType>`]，运行时通过 [`container_class`] 取用（~1ns）。
//!
//! ## 缓存策略
//!
//! 参照 [`crate::error::init_exception_classes`] 模式：
//! - [`GILOnceCell`]：写入需要 GIL（模块初始化时一次），读取需要 GIL（运行时）
//! - 一旦写入，永不变更（`Container` 类是模块级单例）
//! - 缓存未初始化时（如单元测试）[`container_class`] 返回 `Err`，调用方应处理
//!
//! ## 性能开销
//!
//! | 操作 | 单次耗时 |
//! |------|---------|
//! | `container_class(py)`（已缓存） | ~5-10ns（GILOnceCell::get + bind） |
//! | `Container.__init__`（每次迭代） | ~200-500ns |
//! | 字段复制（PyDict 浅拷贝 N 字段） | ~200-400ns |
//!
//! 详见 `docs/模块设计-Array.md` §8.5 关键依赖 4。

use pyo3::prelude::*;
use pyo3::sync::GILOnceCell;
use pyo3::types::PyType;

/// 全局缓存的 `construct.lib.containers.Container` 类引用。
///
/// 在 [`init_container_class`] 时一次性填充。运行时通过 [`container_class`] 取用。
static CONTAINER_CLASS: GILOnceCell<Py<PyType>> = GILOnceCell::new();

/// 在模块初始化时调用，从 `construct.lib.containers` 缓存 `Container` 类引用。
///
/// 必须在 [`crate::_construct_rust`] 模块初始化函数中调用一次。多次调用幂等
/// （后续调用因已初始化而立即返回 `Ok(())`）。
///
/// 调用时机：`construct.lib.containers` 已被 Python 侧 `construct/__init__.py`
/// 间接导入（或在首次 RepeatUntil PyCallable 调用时按需导入，参见下方错误处理）。
///
/// # 错误
///
/// - `construct.lib.containers` 模块不存在（环境异常）
/// - 模块中缺少 `Container` 类（开发期错误）
///
/// # 失败不致命
///
/// 与 [`crate::error::init_exception_classes`] 同模式：缓存失败不阻塞模块加载，
/// 仅记录到 stderr。后续 [`container_class`] 调用会返回 `Err`，RepeatUntil
/// PyCallable 谓词路径上的 build_context_proxy 会将错误包装为
/// [`ConstructError::Generic`](crate::error::ConstructError::Generic)。
pub fn init_container_class(py: Python<'_>) -> PyResult<()> {
    if CONTAINER_CLASS.get(py).is_some() {
        return Ok(());
    }

    let containers_module = py.import_bound("construct.lib.containers")?;
    let container_cls: Py<PyType> = containers_module
        .getattr("Container")
        .and_then(|attr| attr.extract::<Py<PyType>>())?;

    // GILOnceCell::set 在已初始化时返回 Err(value)。由于前面已检查，这里应成功；
    // 即使并发竞争（不会发生，因为模块初始化持有 GIL），也安全降级。
    let _ = CONTAINER_CLASS.set(py, container_cls);
    Ok(())
}

/// 返回缓存的 `Container` 类引用（GIL-bound）。
///
/// 设计依据：`docs/模块设计-Array.md` §4.3.3 / §4.3.4。
///
/// # 性能
///
/// 已缓存路径：~5-10ns（GILOnceCell::get + bind）。
///
/// # 错误
///
/// 返回 `PyErr`（非 `ConstructError`）以便调用方用 `?` 直接在 pyo3 上下文中传播。
/// 未初始化时（如单元测试中未调用 [`init_container_class`]）返回 `ImportError`。
///
/// 调用方（如 `nodes::repeat_until::build_context_proxy`）应将 `PyErr` 包装为
/// `ConstructError::Generic` 并附带 path 字段（错误追踪）。
pub fn container_class<'py>(py: Python<'py>) -> PyResult<Bound<'py, PyType>> {
    match CONTAINER_CLASS.get(py) {
        Some(cls) => Ok(cls.bind(py).clone()),
        None => Err(pyo3::exceptions::PyImportError::new_err(
            "construct-rs: Container class cache not initialized. \
             This indicates _construct_rust module init failed to import \
             construct.lib.containers.Container.",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// 初始化后 container_class 应返回 Container 类。
    #[test]
    fn init_then_container_class_returns_class() {
        with_py(|py| {
            // 幂等：多次调用应成功。
            init_container_class(py).expect("init once");
            init_container_class(py).expect("init twice (idempotent)");

            let cls = container_class(py).expect("container_class");
            let name = cls.name().expect("name");
            assert_eq!(name.to_string(), "Container");
        });
    }

    /// 缓存的类应能构造 Container 实例，且支持 attribute 访问。
    #[test]
    fn cached_class_produces_attribute_accessible_instance() {
        with_py(|py| {
            init_container_class(py).expect("init");
            let cls = container_class(py).expect("container_class");

            // Container(threshold=42)
            let kwargs = pyo3::types::PyDict::new_bound(py);
            kwargs.set_item("threshold", 42).unwrap();
            let instance = cls.call((), Some(&kwargs)).expect("call Container");
            // attribute 访问
            let v: i64 = instance.getattr("threshold").unwrap().extract().unwrap();
            assert_eq!(v, 42);
            // item 访问
            let v2: i64 = instance.get_item("threshold").unwrap().extract().unwrap();
            assert_eq!(v2, 42);
            // isinstance(instance, dict) → True（Container 继承 dict）
            let dict_cls = py.get_type_bound::<pyo3::types::PyDict>();
            assert!(instance.is_instance(&dict_cls).unwrap());
        });
    }

    /// 通过 dict 位置参数构造 Container（与 build_context_proxy 一致）。
    #[test]
    fn container_class_accepts_dict_positional_arg() {
        with_py(|py| {
            init_container_class(py).expect("init");
            let cls = container_class(py).expect("container_class");

            let dict = pyo3::types::PyDict::new_bound(py);
            dict.set_item("threshold", 100).unwrap();
            dict.set_item("_index", 5).unwrap();

            let instance = cls.call1((dict,)).expect("Container(dict)");
            // 通过 __dict__ = self，attribute 应可访问
            let t: i64 = instance.getattr("threshold").unwrap().extract().unwrap();
            let i: i64 = instance.getattr("_index").unwrap().extract().unwrap();
            assert_eq!(t, 100);
            assert_eq!(i, 5);
        });
    }
}
