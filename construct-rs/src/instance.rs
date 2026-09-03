//! 实例构造辅助：[`create_class`] 与 [`force_setattr`]。
//!
//! 参考：pydantic-core `validators/model.rs`（create_class / force_setattr）。
//!
//! ## 设计动机
//!
//! 实例构造完整在 Rust 侧进行：通过 CPython C API 直接调用 `tp_new` 创建
//! 空实例，再用 `PyObject_GenericSetAttr` 整体替换 `__dict__`。
//! 相比在 Python 侧执行 `cls(**dict)` 的方案，避免了 O(N²) kwargs
//! unpacking 开销，且实例不跨 FFI 边界中转。
//!
//! ## 与 pydantic-core 的对齐
//!
//! - [`create_class`] 对应 `pydantic_core::validators::model::create_class`
//!   （model.rs:348-365）：从类型对象读 `tp_new` 槽位调用。
//! - [`force_setattr`] 对应 `pydantic_core::validators::model::force_setattr`
//!   （model.rs:381-394）：直接调 `PyObject_GenericSetAttr` 绕过自定义 `__setattr__`。
//!
//! ## ABI 模式
//!
//! **当前状态**：非 abi3 模式，目标 Python 3.13。
//! `PYO3_USE_ABI3_FORWARD_COMPATIBILITY` 已弃用。
//! 非 abi3 模式下 cpython 子模块可用（`#[cfg(not(Py_LIMITED_API))]`），
//! KnownHash 写入与 PyBytes 直接访问等优化依赖此子模块。
//!
//! ## 直接 dict 访问工具函数
//!
//! - [`dict_via_generic_getdict`]：用 `PyObject_GenericGetDict` 直接获取实例
//!   `__dict__`，绕过 pyo3 getattr 包装链（stable ABI）。
//! - [`set_item_knownhash`]：用 `_PyDict_SetItem_KnownHash` 跳过 interned key
//!   的 hash 重算（cpython 子模块）。
//!
//! pyo3 0.22 API 约束（项目锁定 0.22，详见 `Cargo.toml`）：
//! - `IntoPy<Py<PyAny>>` trait bound（非 0.23+ 的 `IntoPyObject`）。
//! - `.into_py(py)` 返回 `Py<PyAny>`（非 Result，无需 `?`）。
//! - 异常取出用 `PyErr::fetch(py)`（非 0.23+ 的 `py_error_on_minusone` helper）。

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyString, PyTuple, PyType};

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

/// 通过 `PyObject_GenericGetDict` 直接获取实例 `__dict__`，绕过 pyo3 getattr
/// 包装（MRO + 描述符 dispatch + Bound 包装 + downcast）。
///
/// # ABI 依赖
///
/// `[CPython Public Stable ABI]`：`PyObject_GenericGetDict` 自 Python 3.3 起存在，
/// **Stable ABI since version 3.10**（CPython 3.14.7 官方文档 `object.html#c.PyObject_GenericGetDict`）。
/// pyo3 0.22 在 `pyo3-ffi-0.22.6/src/object.rs` L373 暴露（在 Python ≥ 3.10 下，
/// abi3 / 非 abi3 均可用）。
///
/// # Python 3.13 managed dict 行为
///
/// Python 3.12 引入 `Py_TPFLAGS_MANAGED_DICT` flag，3.13 默认对所有 heaptype 启用
/// （CPython typeobj.html：`tp_dictoffset` 设为 -1，表示不再保证该字段可直接访问，
/// 官方推荐改调 `PyObject_GenericGetDict()`）。本函数内部由 CPython 正确处理：
/// - managed dict 模式：从 pre-header `dict_or_values` 槽读 dict
/// - lazy materialize：若 dict 未物化，触发 materialize 创建 PyDictObject
/// - 非 managed dict 模式（C 类型）：从 tp_dictoffset 偏移读 dict（C 类型无
///   `__dict__` 时返回 NULL + AttributeError）
///
/// # 参数
///
/// - `py`：GIL token。
/// - `instance`：heaptype 类的实例（如 [`create_class`] 返回的用户类实例）。
///
/// # 返回
///
/// 成功返回 `Bound<PyDict>`（owned 引用，由 pyo3 Bound drop 自动 decref）。
/// 失败（GenericGetDict 返回 NULL / 返回非 PyDict）返回 `PyErr`——调用方应
/// fallback 到原 getattr 路径。
///
/// # 退化路径
///
/// | 触发条件 | 行为 |
/// |---------|------|
/// | GenericGetDict 返回 NULL（理论不发生，heaptype 实例必有 dict）| 返回 `PyErr`，调用方 fallback 到 getattr |
/// | 返回非 PyDictObject（用户 override `__dict__` 描述符）| downcast 失败，返回 `PyTypeError` |
/// | slots class（dict 始终 NULL）| 返回 `PyErr`，调用方 fallback 到 getattr 报错 |
pub fn dict_via_generic_getdict<'py>(
    py: Python<'py>,
    instance: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyDict>> {
    // SAFETY:
    // - instance.as_ptr() 是有效的 PyObject 指针（pyo3 Bound 守护）
    // - PyObject_GenericGetDict 是 CPython 公开 Stable ABI（自 3.10 起），签名稳定：
    //   `PyObject *PyObject_GenericGetDict(PyObject *obj, void *context)`
    // - GIL 持有（由 py: Python<'py> 参数保证）
    // - context 参数 NULL（CPython 文档：__dict__ getset descriptor 调用时传 NULL）
    // 返回值是 owned reference（new reference），由调用方负责 decref。
    let dict_ptr = unsafe { ffi::PyObject_GenericGetDict(instance.as_ptr(), std::ptr::null_mut()) };
    if dict_ptr.is_null() {
        // 理论不发生（heaptype 实例必有 dict）。CPython 在异常时返回 NULL + 设置异常。
        // PyErr::take 返回 Option<PyErr>（fetch 在无异常时行为未定义，用 take 更安全）。
        let err = PyErr::take(py).unwrap_or_else(|| {
            pyo3::exceptions::PyAttributeError::new_err(
                "PyObject_GenericGetDict returned NULL without setting exception",
            )
        });
        return Err(err);
    }
    // SAFETY: dict_ptr 是 owned reference（PyObject_GenericGetDict 返回 new ref）。
    // 由 Bound::from_owned_ptr 接管 ownership，pyo3 在 Bound drop 时自动 decref。
    // downcast_into::<PyDict> 做运行期 type check（理论总是成功——GenericGetDict
    // 返回 PyDictObject，除非用户 override __dict__ 描述符，此时 fallback 到 getattr）。
    unsafe { Bound::from_owned_ptr(py, dict_ptr) }
        .downcast_into::<PyDict>()
        .map_err(|_| {
            pyo3::exceptions::PyTypeError::new_err(
                "instance __dict__ is not a dict (possible __dict__ override or __slots__ class)",
            )
        })
}

/// 用 `_PyDict_SetItem_KnownHash` 写入 dict（跳过 hash 重算）。
///
/// # ABI 依赖
///
/// `[CPython Internal Function]`：`_PyDict_SetItem_KnownHash` 是 CPython 私有 API
/// （非 stable ABI），签名自 Python 3.5 起未变：
/// `int _PyDict_SetItem_KnownHash(PyObject *mp, PyObject *key, PyObject *value, Py_hash_t hash)`。
/// pyo3 0.22 cpython 子模块已暴露（`pyo3::ffi::_PyDict_SetItem_KnownHash`，
/// 经 `pyo3-ffi-0.22.6/src/cpython/dictobject.rs` L29 + lib.rs L459 re-export）。
/// 该子模块由 `#[cfg(not(Py_LIMITED_API))]` 守护——项目非 abi3，可用。
///
/// # Safety
///
/// 调用方必须保证：
/// 1. `hash` 是 `key` 的正确 hash（CPython `PyObject_Hash` 返回值）——本项目由
///    [`crate::nodes::struct_node::FieldName::cached_hash`] 编译期一次性计算并缓存。
/// 2. `key` 是 hashable（interned PyString 自动满足）。
/// 3. GIL 持有（由 `py: Python<'_>` 保证）。
/// 4. `dict` 是 `PyDictObject`（pyo3 `Bound<PyDict>` 类型签名守护）。
///
/// 返回 `Ok(())` 表示成功（C API 返回 0），`Err(PyErr)` 表示失败（C API 返回 -1，
/// 异常已挂起）。
///
/// # 退化路径
///
/// - `_PyDict_SetItem_KnownHash` 返回 -1（dict 操作失败）→ 取出挂起异常，返回 `Err`
///   （与 `PyDict_SetItem` 同行为，调用方可 fallback）
/// - 未来 CPython 删除符号 → pyo3 cpython 子模块升级时编译失败（编译期发现）
/// - 项目切 abi3 → 本函数整体不可编译（cpython 子模块缺失）
pub fn set_item_knownhash(
    py: Python<'_>,
    dict: &Bound<'_, PyDict>,
    key: &Bound<'_, PyAny>,
    value: &Bound<'_, PyAny>,
    hash: ffi::Py_hash_t,
) -> PyResult<()> {
    // SAFETY:
    // - dict / key / value 由 pyo3 Bound 守护，均为有效 PyObject 指针
    // - dict 是 Bound<PyDict>，保证是 PyDictObject（_PyDict_SetItem_KnownHash 要求）
    // - GIL 持有保证单线程访问
    // - hash 由 FieldName 编译期一次性 PyObject_Hash 计算并缓存（interned PyString
    //   hash 在其生命周期内不变，CPython 强约束）
    let result = unsafe {
        ffi::_PyDict_SetItem_KnownHash(dict.as_ptr(), key.as_ptr(), value.as_ptr(), hash)
    };
    if result == -1 {
        Err(PyErr::fetch(py))
    } else {
        Ok(())
    }
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
        // 核心场景：整体替换 __dict__。
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

    // ======================================================================
    // dict_via_generic_getdict
    // ======================================================================
    //
    // 测试场景：
    // - 普通 class（有 __dict__）
    // - dataclass（非 frozen）
    // - frozen dataclass
    // - slots class（fallback 到 getattr 路径）
    // - managed dict materialize

    /// 通过 GenericGetDict 获取实例 __dict__，验证返回的 dict 与 instance 是
    /// instance 的真实 __dict__（identity 比较）。
    #[test]
    fn dict_via_generic_getdict_returns_correct_dict() {
        with_py(|py| {
            let cls = make_simple_class(py, "User");
            let instance = create_class(&cls).expect("create_class");
            let dict_via_ggd = dict_via_generic_getdict(py, &instance).expect("generic getdict");
            // instance.getattr("__dict__") 是同对象引用——GenericGetDict 返回的应是
            // 同一个 PyDictObject（materialize 后）。
            let dict_via_getattr = instance.getattr("__dict__").expect("getattr __dict__");
            assert!(
                dict_via_ggd.as_ptr() == dict_via_getattr.as_ptr(),
                "GenericGetDict and getattr should return the same dict object (identity)"
            );
        });
    }

    #[test]
    fn dict_via_generic_getdict_works_with_dataclass() {
        // dataclass（非 frozen）启用 managed dict，GenericGetDict 应正常工作。
        with_py(|py| {
            let code = concat!(
                "from dataclasses import dataclass\n",
                "@dataclass\n",
                "class Point:\n",
                "    x: int = 0\n",
                "    y: int = 0\n",
            );
            let globals = PyDict::new_bound(py);
            py.run_bound(code, Some(&globals), None).expect("define");
            let cls = globals
                .get_item("Point")
                .expect("get_item ok")
                .expect("exists")
                .extract::<Py<PyType>>()
                .expect("extract");
            let instance = create_class(cls.bind(py)).expect("create_class");
            let dict = dict_via_generic_getdict(py, &instance).expect("generic getdict");
            assert_eq!(
                dict.len(),
                0,
                "fresh dataclass instance __dict__ should be empty"
            );
        });
    }

    #[test]
    fn dict_via_generic_getdict_works_with_frozen_dataclass() {
        // frozen dataclass 同样启用 managed dict。frozen 只影响 setattr（tp_setattro），
        // 不影响 dict 读取（GenericGetDict 不经 __setattr__）。
        with_py(|py| {
            let code = concat!(
                "from dataclasses import dataclass\n",
                "@dataclass(frozen=True)\n",
                "class Frozen:\n",
                "    x: int = 0\n",
            );
            let globals = PyDict::new_bound(py);
            py.run_bound(code, Some(&globals), None).expect("define");
            let cls = globals
                .get_item("Frozen")
                .expect("get_item ok")
                .expect("exists")
                .extract::<Py<PyType>>()
                .expect("extract");
            let instance = create_class(cls.bind(py)).expect("create_class");
            let dict = dict_via_generic_getdict(py, &instance).expect("generic getdict");
            assert_eq!(
                dict.len(),
                0,
                "fresh frozen dataclass __dict__ should be empty"
            );
        });
    }

    #[test]
    fn dict_via_generic_getdict_handles_managed_dict_materialize() {
        // Python 3.13 lazy materialize：create_class 创建后 dict_or_values 槽为 NULL，
        // GenericGetDict 应触发 materialize 创建 PyDictObject 并返回。
        with_py(|py| {
            let cls = make_simple_class(py, "Managed");
            let instance = create_class(&cls).expect("create_class");
            // 首次访问触发 materialize——GenericGetDict 内部处理。
            let dict = dict_via_generic_getdict(py, &instance).expect("materialize");
            // dict 应为有效 PyDictObject，可写入。
            assert_eq!(dict.len(), 0, "dict should be empty after materialize");
            // 写入后再次通过 GenericGetDict 获取，应是同一对象（已 materialize）。
            dict.set_item("k", 1i64).expect("set k");
            let dict2 = dict_via_generic_getdict(py, &instance).expect("second call");
            assert_eq!(
                dict2.len(),
                1,
                "second access should return the materialized dict"
            );
            assert!(
                dict2.contains("k").expect("contains"),
                "k should be in dict"
            );
        });
    }

    // ======================================================================
    // set_item_knownhash
    // ======================================================================

    #[test]
    fn set_item_knownhash_writes_correctly_and_readable() {
        // KnownHash 写入的 (key, value) 可通过 dict.get_item 正常读取。
        with_py(|py| {
            let dict = PyDict::new_bound(py);
            let key = intern_pystring(py, "alpha");
            let value = 42i64.into_py(py);
            // 计算正确的 hash（PyObject_Hash 返回值）。
            let hash = unsafe { ffi::PyObject_Hash(key.as_ptr()) };
            assert_ne!(
                hash, -1,
                "PyObject_Hash should not fail for interned string"
            );
            set_item_knownhash(py, &dict, key.bind(py), value.bind(py), hash)
                .expect("set_item_knownhash");
            let v: i64 = dict
                .get_item("alpha")
                .expect("get_item ok")
                .expect("alpha exists")
                .extract()
                .expect("extract");
            assert_eq!(v, 42);
        });
    }

    #[test]
    fn set_item_knownhash_multiple_fields_no_collision() {
        // 多字段写入：所有字段 hash 不冲突，写入顺序正确。
        with_py(|py| {
            let dict = PyDict::new_bound(py);
            for (name, val) in [("a", 1i64), ("b", 2i64), ("c", 3i64)] {
                let key = intern_pystring(py, name);
                let value = val.into_py(py);
                let hash = unsafe { ffi::PyObject_Hash(key.as_ptr()) };
                assert_ne!(hash, -1);
                set_item_knownhash(py, &dict, key.bind(py), value.bind(py), hash).expect("set");
            }
            assert_eq!(dict.len(), 3);
            for (name, expected) in [("a", 1i64), ("b", 2i64), ("c", 3i64)] {
                let v: i64 = dict
                    .get_item(name)
                    .expect("get")
                    .expect("exists")
                    .extract()
                    .expect("extract");
                assert_eq!(v, expected);
            }
        });
    }

    #[test]
    fn set_item_knownhash_unicode_field_name() {
        // Unicode 字段名：cached_hash = PyObject_Hash(unicode) 应正确。
        with_py(|py| {
            let dict = PyDict::new_bound(py);
            // 包含 Unicode 字符的字段名（中日韩字符 + emoji）
            let key = intern_pystring(py, "字段αβγ✨");
            let value = 0xDEAD_i64.into_py(py);
            let hash = unsafe { ffi::PyObject_Hash(key.as_ptr()) };
            assert_ne!(hash, -1, "PyObject_Hash should succeed for Unicode string");
            set_item_knownhash(py, &dict, key.bind(py), value.bind(py), hash).expect("set");
            let v: i64 = dict
                .get_item("字段αβγ✨")
                .expect("get")
                .expect("exists")
                .extract()
                .expect("extract");
            assert_eq!(v, 0xDEAD);
        });
    }

    #[test]
    fn set_item_knownhash_none_value() {
        // 字段值为 None：KnownHash 应正常写入（PyDict_SetItem 支持 None value）。
        with_py(|py| {
            let dict = PyDict::new_bound(py);
            let key = intern_pystring(py, "x");
            let none = py.None();
            let hash = unsafe { ffi::PyObject_Hash(key.as_ptr()) };
            set_item_knownhash(py, &dict, key.bind(py), none.bind(py), hash).expect("set None");
            assert!(dict.contains("x").expect("contains"));
            assert!(dict.get_item("x").expect("get").expect("exists").is_none());
        });
    }
}
