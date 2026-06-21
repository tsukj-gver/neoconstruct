//! 实例构造辅助：[`create_class`] 与 [`force_setattr`]。
//!
//! 设计依据：`docs/设计修订-parse路径优化.md` §3.2（方案 B'）。
//! 参考：`refs/pydantic/pydantic-core/src/validators/model.rs`（line 348-394）。
//!
//! ## 设计动机
//!
//! Phase 1 子任务 1.8 揭示 parse 路径存在两个 §0 违反（详见设计修订 §1）：
//!
//! 1. `_parse_raw` 返回 dict 到 Python，Python 侧 `cls(**dict)` 引入 O(N²) kwargs
//!    unpacking 开销（B4 仅 0.94x）。
//! 2. dict 跨 FFI 边界返回，是明确的"中间表示层"违反。
//!
//! 方案 B' 将实例构造完整移入 Rust：通过 CPython C API 直接调用 `tp_new` 创建
//! 空实例，再用 `PyObject_GenericSetAttr` 整体替换 `__dict__`。
//!
//! ## 与 pydantic-core 的对齐
//!
//! - [`create_class`] 对应 `pydantic_core::validators::model::create_class`
//!   （model.rs:348-365）：从类型对象读 `tp_new` 槽位调用。
//! - [`force_setattr`] 对应 `pydantic_core::validators::model::force_setattr`
//!   （model.rs:381-394）：直接调 `PyObject_GenericSetAttr` 绕过自定义 `__setattr__`。
//!
//! ## ABI3 / 有限 API 兼容性
//!
//! 本项目通过 `PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1` 编译（详见 conftest.py），
//! 启用 `Py_LIMITED_API`。此模式下：
//!
//! - `ffi::PyTypeObject` 为 opaque（不可直接访问 `tp_new` 字段），必须用
//!   `PyType_GetSlot(type, Py_tp_new)` 间接获取槽位（ABI3 稳定 API）。
//! - `_PyDict_NewPresized` 不可用（私有 API，仅非 limited API 暴露）。
//!   改用 `PyDict::new_bound(py)` 创建 dict。
//!
//! pyo3 0.22 API 约束（项目锁定 0.22，详见 `Cargo.toml`）：
//! - `IntoPy<Py<PyAny>>` trait bound（非 0.23+ 的 `IntoPyObject`）。
//! - `.into_py(py)` 返回 `Py<PyAny>`（非 Result，无需 `?`）。
//! - 异常取出用 `PyErr::fetch(py)`（非 0.23+ 的 `py_error_on_minusone` helper）。

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyString, PyTuple, PyType};

/// 创建用户类的空实例（等价 `cls.__new__(cls)`，绕过 Python 层方法查找）。
///
/// 直接通过 `PyType_GetSlot(cls, Py_tp_new)` 获取 `tp_new` 函数指针并调用，
/// 与 pydantic-core `create_class`（model.rs:348-365）语义一致。
///
/// # 参数
///
/// - `cls`：用户类对象（`@dataclass` 装饰的 `StructMixin` 子类）。
///
/// # 返回
///
/// 成功返回 `Bound<PyAny>`（用户类的空实例，`__dict__` 为空 dict），
/// 失败返回 `PyErr`。
///
/// # ABI3 兼容性
///
/// `Py_LIMITED_API` 下 `PyTypeObject` 为 opaque，不能直接读 `tp_new` 字段。
/// 改用 `PyType_GetSlot`（自 Python 3.4 起的稳定 ABI）获取函数指针：
///
/// ```text
/// let slot = PyType_GetSlot(raw_type, Py_tp_new);   // *mut c_void
/// let new_func: newfunc = transmute(slot);          // 转为函数指针
/// ```
///
/// # 安全性
///
/// - `cls.as_type_ptr()` 返回有效的 `PyTypeObject` 指针（由 pyo3 守护）。
/// - `PyType_GetSlot` 是 CPython 标准 C API，对 `Py_tp_new` 返回 `newfunc` 函数指针。
/// - `new_func` 调用要求首参为类型对象指针（`raw_type`），布局兼容 `*mut PyObject`。
pub fn create_class<'py>(cls: &Bound<'py, PyType>) -> PyResult<Bound<'py, PyAny>> {
    let py = cls.py();
    let args = PyTuple::empty_bound(py);
    let raw_type = cls.as_type_ptr();
    // SAFETY: raw_type 来自 Bound<PyType>::as_type_ptr()，保证是有效的 PyTypeObject
    // 指针。PyType_GetSlot 是标准 CPython C API（ABI3 稳定），返回槽位值。
    let slot_ptr = unsafe { ffi::PyType_GetSlot(raw_type, ffi::Py_tp_new) };
    if slot_ptr.is_null() {
        // 类型未定义 tp_new（理论不会发生——所有 Python 类型都继承自 object）。
        return Err(pyo3::exceptions::PyTypeError::new_err(
            "base type without tp_new",
        ));
    }
    // SAFETY: PyType_GetSlot 对 Py_tp_new 返回的是 newfunc 函数指针，
    // 其布局与 *mut c_void 相同（函数指针宽度等于 usize）。
    // transmute 将 *mut c_void 转换为 newfunc 类型的函数指针。
    let new_func: ffi::newfunc = unsafe { std::mem::transmute(slot_ptr) };
    // SAFETY: 调用 new_func 是标准 CPython C API 用法。首参 raw_type 为类型对象
    // 指针，newfunc 的签名首参为 *mut PyTypeObject（布局兼容 *mut PyObject）。
    // 第二参为 args tuple（已通过 PyTuple::empty_bound 构造），第三参为 kwargs（NULL）。
    let instance_ptr = unsafe { new_func(raw_type, args.as_ptr(), std::ptr::null_mut()) };
    // SAFETY: instance_ptr 来自 tp_new 调用。若 tp_new 失败，instance_ptr 为 null，
    // 且 Python 异常已挂起——from_owned_ptr_or_err 会检测 null 并通过 PyErr::fetch
    // 取出异常转为 PyErr。成功时 instance_ptr 为新分配的 Python 对象，所有权移交给 Bound。
    unsafe { Bound::from_owned_ptr_or_err(py, instance_ptr) }
}

/// 强制设置属性，绕过自定义 `__setattr__`（直接调 `PyObject_GenericSetAttr`）。
///
/// 用于整体替换实例的 `__dict__`，兼容 frozen dataclass（其 `__setattr__` 会 raise
/// `FrozenInstanceError`）。与 pydantic-core `force_setattr`
/// （model.rs:381-394）语义一致。
///
/// # 参数
///
/// - `py`：GIL token。
/// - `obj`：待设置属性的 Python 对象。
/// - `attr_name`：属性名（实现 `IntoPy<Py<PyAny>>`，如 `&str`、`Py<PyString>`）。
/// - `value`：属性值（实现 `IntoPy<Py<PyAny>>`）。
///
/// # 返回
///
/// 成功返回 `Ok(())`，失败返回 `PyErr`（C API 返回 -1 时取出挂起的异常）。
///
/// # 安全性
///
/// `PyObject_GenericSetAttr` 是 CPython 公开 C API（limited API 包含），等价
/// `object.__setattr__`。调用前必须持有 GIL（由 `py: Python<'_>` 参数保证）。
pub fn force_setattr<N, V>(
    py: Python<'_>,
    obj: &Bound<'_, PyAny>,
    attr_name: N,
    value: V,
) -> PyResult<()>
where
    N: pyo3::conversion::IntoPy<Py<PyAny>>,
    V: pyo3::conversion::IntoPy<Py<PyAny>>,
{
    // into_py 返回 Py<PyAny>（拥有所有权的 Python 对象指针）。
    let attr_name = attr_name.into_py(py);
    let value = value.into_py(py);
    // SAFETY: 持有 GIL（由 py 参数保证），obj.as_ptr / attr_name.as_ptr / value.as_ptr
    // 均为有效的 Python 对象指针。PyObject_GenericSetAttr 是 CPython 标准 C API。
    let result =
        unsafe { ffi::PyObject_GenericSetAttr(obj.as_ptr(), attr_name.as_ptr(), value.as_ptr()) };
    if result == -1 {
        // C API 返回 -1 表示出错，Python 异常已挂起，fetch 取出转为 PyErr。
        Err(PyErr::fetch(py))
    } else {
        Ok(())
    }
}

/// 创建一个 interned `Py<PyString>`（供 [`crate::nodes::struct_node::FieldName`] 缓存使用）。
///
/// Interning 使相同字符串字面量共享同一个 `PyUnicodeObject`，在 `PyDict_SetItem`
/// 中作为 key 时可走指针快速比较路径（跳过 hash 计算与逐字符比较）。
///
/// # 参数
///
/// - `py`：GIL token。
/// - `name`：待 intern 的字符串（运行时值，非 `&'static str`）。
///
/// # 返回
///
/// 拥有所有权的 `Py<PyString>`（interned，全局共享）。
///
/// # 实现细节
///
/// 使用 pyo3 0.22 的 `PyString::intern_bound(py, &str)`，内部调用
/// `PyUnicode_FromStringAndSize` + `PyUnicode_InternInPlace`。
pub fn intern_pystring(py: Python<'_>, name: &str) -> Py<PyString> {
    PyString::intern_bound(py, name).into()
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::types::PyDict;

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

    /// 定义一个最小 Python 类用于测试 create_class。
    fn make_simple_class<'py>(py: Python<'py>, _name: &str) -> Bound<'py, PyType> {
        let code = format!("type('{}', (), {{}})", _name);
        py.eval_bound(&code, None, None)
            .expect("create type")
            .downcast_into::<PyType>()
            .expect("is PyType")
    }

    // ======================================================================
    // create_class
    // ======================================================================

    #[test]
    fn create_class_produces_instance_of_input_class() {
        with_py(|py| {
            let cls = make_simple_class(py, "Foo");
            let instance = create_class(&cls).expect("create_class");
            assert!(
                instance.is_instance(&cls).expect("is_instance"),
                "result should be a Foo instance"
            );
        });
    }

    #[test]
    fn create_class_produces_empty_dict() {
        // tp_new 创建的实例 __dict__ 应为空（无属性）。
        with_py(|py| {
            let cls = make_simple_class(py, "Bar");
            let instance = create_class(&cls).expect("create_class");
            let d = instance
                .getattr("__dict__")
                .expect("getattr __dict__")
                .downcast_into::<PyDict>()
                .expect("is dict");
            assert_eq!(d.len(), 0, "fresh instance __dict__ should be empty");
        });
    }

    #[test]
    fn create_class_works_with_dataclass() {
        // 标准 @dataclass 无自定义 __new__，cls.tp_new == object.tp_new。
        with_py(|py| {
            let code = concat!(
                "from dataclasses import dataclass\n",
                "@dataclass\n",
                "class Point:\n",
                "    x: int = 0\n",
                "    y: int = 0\n",
            );
            let globals = PyDict::new_bound(py);
            py.run_bound(code, Some(&globals), None)
                .expect("define Point");
            let cls = globals
                .get_item("Point")
                .expect("get_item ok")
                .expect("Point exists")
                .extract::<Py<PyType>>()
                .expect("extract Py<PyType>");

            let instance = create_class(cls.bind(py)).expect("create_class");
            assert!(
                instance.is_instance(cls.bind(py)).expect("is_instance"),
                "should be Point instance"
            );
        });
    }

    // ======================================================================
    // force_setattr
    // ======================================================================

    #[test]
    fn force_setattr_writes_attribute() {
        with_py(|py| {
            let cls = make_simple_class(py, "Box");
            let instance = create_class(&cls).expect("create_class");
            let value = 42i64.into_py(py);
            force_setattr(py, &instance, "x", value).expect("force_setattr");
            let x: i64 = instance
                .getattr("x")
                .expect("getattr")
                .extract()
                .expect("extract");
            assert_eq!(x, 42);
        });
    }

    #[test]
    fn force_setattr_replaces_dict() {
        // 方案 B' 核心场景：整体替换 __dict__。
        with_py(|py| {
            let cls = make_simple_class(py, "Replaced");
            let instance = create_class(&cls).expect("create_class");

            // 构造独立 dict 并 set_item
            let dict = PyDict::new_bound(py);
            dict.set_item("a", 1i64).expect("set a");
            dict.set_item("b", 2i64).expect("set b");

            // 整体替换 __dict__
            force_setattr(py, &instance, "__dict__", dict.into_any().unbind())
                .expect("force_setattr __dict__");

            // 验证属性可读
            let a: i64 = instance
                .getattr("a")
                .expect("getattr a")
                .extract()
                .expect("extract a");
            let b: i64 = instance
                .getattr("b")
                .expect("getattr b")
                .extract()
                .expect("extract b");
            assert_eq!(a, 1);
            assert_eq!(b, 2);
        });
    }

    #[test]
    fn force_setattr_bypasses_frozen_dataclass_setattr() {
        // frozen dataclass 生成的 __setattr__ 会 raise FrozenInstanceError。
        // force_setattr 走 PyObject_GenericSetAttr，应绕过拦截。
        with_py(|py| {
            let code = concat!(
                "from dataclasses import dataclass\n",
                "@dataclass(frozen=True)\n",
                "class Frozen:\n",
                "    x: int = 0\n",
            );
            let globals = PyDict::new_bound(py);
            py.run_bound(code, Some(&globals), None)
                .expect("define Frozen");
            let cls = globals
                .get_item("Frozen")
                .expect("get_item ok")
                .expect("Frozen exists")
                .extract::<Py<PyType>>()
                .expect("extract");

            let instance = create_class(cls.bind(py)).expect("create_class");

            // 普通 setattr 应失败（frozen）
            let normal_result = instance.setattr("x", 100i64);
            assert!(
                normal_result.is_err(),
                "normal setattr should fail on frozen dataclass"
            );

            // force_setattr 应成功
            force_setattr(py, &instance, "x", 100i64.into_py(py)).expect("force_setattr on frozen");
            let x: i64 = instance
                .getattr("x")
                .expect("getattr")
                .extract()
                .expect("extract");
            assert_eq!(x, 100);
        });
    }

    // ======================================================================
    // intern_pystring
    // ======================================================================

    #[test]
    fn intern_pystring_returns_interned_str() {
        with_py(|py| {
            let s1 = intern_pystring(py, "hello");
            let s2 = intern_pystring(py, "hello");
            // interned 字符串应共享同一对象（identity 比较）
            assert!(s1.is(&s2), "interned strings should be identical objects");
        });
    }

    #[test]
    fn intern_pystring_distinct_for_distinct_names() {
        with_py(|py| {
            let a = intern_pystring(py, "alpha");
            let b = intern_pystring(py, "beta");
            assert!(!a.is(&b), "distinct names should produce distinct objects");
        });
    }
}
