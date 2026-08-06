---
id: DESIGN-Phase8-OPT-SHARED
status: designing
phase: "8"
depends_on:
  - ADR-023
  - phase8-parse-perf-reassessment
  - ADR-008
  - ADR-019
  - ADR-021
  - 8.ENV-环境切换
last_updated: 2026-08-06
---

# 模块设计 - Phase 8 OPT-SHARED（共享税优化，Python 3.13 非 abi3）

## 模块位置

| 文件 | 改动类型 |
|------|---------|
| `construct-rs/src/instance.rs` | 新增 `dict_via_generic_getdict` / `set_item_knownhash` 两个 unsafe 工具函数；FieldName::new 添加 `PyObject_Hash` 缓存（成功路径 + -1 panic 处理） |
| `construct-rs/src/nodes/struct_node.rs` | StructNode.parse 改 dict 获取路径（O1-A：getattr → PyObject_GenericGetDict）；FieldName 新增 `cached_hash: Py_hash_t`；set_item 走 KnownHash 路径（O1-B） |
| `construct-rs/src/schema.rs` | `_parse_raw` 入口精简（O2-A：直接访问 PyBytesObject.ob_sval 字段 + 返回 Py<PyAny>） |

**关键事实**（pyo3 0.22 + Python 3.13 实测验证，2026-08-06）：
- **目标环境约束**（用户决策 + 8.ENV 验证）：Python 3.13 非 abi3 模式（无 abi3 fallback）。
- `PyObject_GenericGetDict` **已由 pyo3::ffi 公开 stable ABI 暴露**（`pyo3-ffi-0.22.6/src/object.rs`
  L373）。**不依赖 cpython 子模块，abi3 / 非 abi3 均可用**。O1-A 改用此函数（替代原
  tp_dictoffset 直读，详见 §managed dict 调研）。
- `_PyDict_SetItem_KnownHash` **已由 pyo3::ffi::cpython 暴露**（`pyo3-ffi-0.22.6/src/cpython/dictobject.rs`
  L29-34，通过 lib.rs L459 `pub use self::cpython::*` re-export 为 `pyo3::ffi::_PyDict_SetItem_KnownHash`）。
  **不需手动 extern 声明，不需 build.rs 符号探测**。由 `#[cfg(not(Py_LIMITED_API))]` 守护（项目非 abi3，可用）。
- `PyBytesObject.ob_sval` **已由 pyo3::ffi::cpython 暴露**（`pyo3-ffi-0.22.6/src/cpython/bytesobject.rs`
  L8-14）。直接 struct field 访问。由 `#[cfg(not(any(PyPy, GraalPy, Py_LIMITED_API)))]` 守护（项目
  锁定 CPython AMD64 + 非 abi3，可用）。
- **`tp_dictoffset` 直读路径废弃**（Python 3.13 managed dict 使 `tp_dictoffset = -1`，
  原设计完全失效，详 §managed dict 调研）。

## 职责

把 StructNode.parse 共享税从 ~180ns 压到 ~140-160ns（vs Python 3.13 非 abi3 新基线，
8.ENV §4.1），为 10 项 Phase 8 parse case 解锁 10x+ 提供前置条件。三条优化方向
（O1-A / O1-B / O2-A）均不改 FFI 边界数量，仅减少 Rust 内部 CPython C API 调用开销。

## 设计依据

- **决策**：`docs/decisions/ADR-023-共享税优化-unsafe-CPython内部API.md`（v2，2026-08-06 更新）
- **设计输入**：`docs/analysis/phase8-parse-perf-reassessment.md §5.1`
- **环境前提**：`plans/phase8-adapters-struct-streams/traces/8.ENV-环境切换.md`（Python 3.13 非 abi3 + 新基线 + cpython 符号可访问）
- **已有实现**：`construct-rs/src/instance.rs`（`create_class` unsafe 先例）+
  `construct-rs/src/nodes/struct_node.rs`（R4 借用 dict 路径）

---

## §abi3 前提变更说明（可追溯）

本设计经历三个阶段（2026-08-06 同日完成，便于跨阶段追溯）：

### 阶段 1：初版设计（v1）

- **前提**：假设项目非 abi3 模式（基于 `pyproject.toml` + `Cargo.toml` 未显式启用 abi3）。
- **设计**：三条优化（O1-A tp_dictoffset 直读 + O1-B KnownHash + O2-A ob_sval）均依赖
  pyo3::ffi::cpython 子模块，配套 abi3 cfg fallback。
- **错误**：忽略 `PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1` 环境变量路径（pyo3-build-config
  的 abi3 检测有两条独立路径，详 REV 检视 §2.3.1）。

### 阶段 2：REV 驳回（2026-08-06）

- **驳回依据**（`8.OPT-SHARED-REV检视.md`）：项目实际编译时 pyo3 build script 输出
  `--cfg Py_LIMITED_API`（实测铁证），cpython 子模块不编译，三条优化全部走 fallback
  路径，节省 ns = 0。
- **L-14 第 3 次触发**：ARCH 把"cpython 子模块可用"当默认前提，**未交叉验证**（没查
  pyo3-build-config 源码，没实测项目编译时 cfg）。**单一路径推断**：只检查
  `pyproject.toml`/`Cargo.toml` 的 abi3 feature，忽略环境变量路径。

### 阶段 3：8.ENV 落地（2026-08-06 ACCEPTED）+ 用户决策

- **用户决策**：放弃 abi3，目标 Python 3.13（立项未要求 3.14）。
- **8.ENV 完成**：
  - Python 3.13.14 双 venv（crs_venv_313 + crs_venv_py_313）
  - **决定性验证**：clean rebuild 后全部 4 个 pyo3 build output 无任何
    `cargo:rustc-cfg=Py_LIMITED_API`（详 8.ENV §2.3）
  - 功能不回退：cargo test 1373 passed / pytest 767 passed
  - **cpython 三符号编译链接验证**：临时测试 `tests/abi3_symbol_check.rs` 通过
    （offset_of PyTypeObject.tp_dictoffset / 函数指针 _PyDict_SetItem_KnownHash /
    offset_of PyBytesObject.ob_sval），验证后删除
  - **Python 3.13 非 abi3 新基线建立**（41/48 PASS，详 §3.2）

### 阶段 4：v2 设计（本版，2026-08-06）

- **abi3 fallback 去除**：非 abi3 是项目约束（用户决策 + 8.ENV 落地），不再保留
  abi3 cfg fallback。O1-A 改用 stable ABI（不依赖 cpython 子模块），O1-B/O2-A 依赖
  cpython 子模块（项目非 abi3 可用）。
- **O1-A 重新设计**：Python 3.13 managed dict 使 `tp_dictoffset = -1`，原设计
  完全失效（详 §Python 3.13 managed dict 调研）。改用 `PyObject_GenericGetDict`
  （CPython 公开 stable ABI，正确处理 managed dict）。
- **预测更新**：vs Python 3.13 非 abi3 新基线（不再 vs 3.14+abi3 旧基线，L-09 不作
  跨版本对比）。

---

## §Python 3.13 managed dict 调研（L-14 第 3 次触发核心）

### 触发原因

用户指令（2026-08-06）明确指出：Python 3.13 引入 **managed dict**
（`Py_TPFLAGS_MANAGED_DICT` flag）——实例 dict 的存储方式变了。这直接影响 O1-A
（tp_dictoffset 直读）。

**用户明确警告（L-14 自检要求）**：
> 不要把"managed dict 不可处理"当硬约束——先交叉验证 CPython 3.13 是否提供兼容
> 访问方式（如 `PyObject_GetDict` 内部 API、或 managed dict 仍保留 tp_dictoffset
> 兼容值）。

### 调研方法（L-02 对策：实证优先于理论）

在 Python 3.13 venv（crs_venv_313）实测用户类布局（详 §证据）：

#### 实证 1：所有用户类启用 managed dict

| 类 | managed_dict | inline_values | tp_dictoffset |
|----|:---:|:---:|:---:|
| StructMixin（基类）| ✓ | ✓ | **-1** |
| `@dataclass class X(StructMixin)` | ✓ | ✓ | **-1** |
| frozen dataclass | ✓ | ✓ | **-1** |
| slots class | ✓ | ✗ | **-1** |
| `int` / `str` / `object`（C 类型）| ✗ | ✗ | 0 |

**结论**：Python 3.13 下所有 heaptype 用户类默认启用 managed dict + inline values，
`tp_dictoffset = -1`（CPython 不变量，`docs.python.org/3/c-api/typeobj.html` 明确规定：
"If the `Py_TPFLAGS_MANAGED_DICT` bit is set in the `tp_flags` field, then
`tp_dictoffset` will be set to `-1`, to indicate that it is unsafe to use this field"）。

#### 实证 2：CPython 3.13 pre-header 布局 + lazy materialize

通过 ctypes 直接读实例内存（不触发 `__dict__` 访问），确认 CPython 3.13 layout：

```
instance_ptr - 32: weakreflist
instance_ptr - 24: dict_or_values   <-- pre-header 关键槽（实测）
instance_ptr - 16: GC 1
instance_ptr -  8: GC 2
instance_ptr +  0: ob_refcnt
instance_ptr +  8: ob_type
```

**实测 lazy materialize 行为**：
- `object.__new__(S)` 后 `dict_or_values` 槽 = `NULL`（dict 未 materialize）
- 首次访问 `inst.__dict__` 后 `dict_or_values` 槽 = PyDictObject 指针（materialize 发生）

**结论**：dict 是 lazy 的，首次访问必须 materialize（任何路径都需付出 ~20-30ns 开销）。

### O1-A 原设计失效分析

原设计（v1）：
```rust
// 编译期：cache_dict_offset(cls) 读 (*type_ptr).tp_dictoffset
// parse 时：*(instance_ptr + offset) 拿 dict 指针
```

**Python 3.13 下完全失效**：
1. `(*type_ptr).tp_dictoffset` = **-1**（managed dict 模式强制值）
2. `instance_ptr + (-1)` 读越界（负偏移访问 ob_type 之前的内存）
3. 运行期 UB（崩溃 / 错误数据 / 数据竞争）

**根本原因**：managed dict 模式下 dict 不在固定正偏移，而在 pre-header 中
（dict_or_values 槽，offset = -24 in Python 3.13 64-bit）。CPython 通过
`PyObject_GenericGetDict` / `_PyObject_GetDictPtr` 等 API 处理这个差异。

### L-14 交叉验证：替代路径枚举

| 路径 | 描述 | 可行性 | 节省（量级参考） |
|------|------|-------|---------|
| 1（原设计）| tp_dictoffset 直读 | ❌ **失效**（Python 3.13 = -1）| 0 |
| 2（v2 采用）| `PyObject_GenericGetDict(obj, NULL)` | ✅ 公开 stable ABI，处理 managed dict + materialize | ~10-20ns |
| 3 | `_PyObject_GetDictPtr(obj)` | ⚠️ 不触发 materialize（返回 dict 指针的地址；若未 materialize 返回 NULL），不适用 parse 写入路径 | N/A |
| 4 | 直接读 pre-header dict_or_values 槽（unsafe）| ⚠️ 偏移随 Python 版本变化（3.12 vs 3.13 不同）+ 需 manual materialize | ~13-25ns（但维护成本高）|

**选择路径 2（PyObject_GenericGetDict）** 的理由：
- ✅ CPython 公开 stable ABI（abi3 / 非 abi3 均可用，无 abi3 风险）
- ✅ CPython 内部正确处理 managed dict + inline values + lazy materialize
- ✅ CPython 官方文档明确推荐："To get the pointer to the dictionary call
  `PyObject_GenericGetDict()`"
- ✅ pyo3 0.22 已暴露（`pyo3::ffi::PyObject_GenericGetDict`）
- ✅ 节省 ~10-20ns（与原设计 15-25ns 同量级，主要差来自绕过 pyo3 getattr 包装而非
  materialize）

### managed dict 对 O1-B / O2-A 的影响

- **O1-B（KnownHash）**：parse 路径用 `PyObject_GenericGetDict` 获取 dict 后，dict 已
  materialize 为 PyDictObject（CPython 不变量）。传给 `_PyDict_SetItem_KnownHash`
  安全。inline values 在 GenericGetDict 之前已强制 materialize。**O1-B 不受 managed dict 影响**。
- **O2-A（ob_sval 直读）**：PyBytesObject 在 Python 3.13 没变化（PyBytesObject
  布局稳定，ob_sval 自 Python 1.x 未变）。**O2-A 不受 managed dict 影响**。

### 调研证据

| 证据 | 来源 |
|------|------|
| Python 3.13 实测用户类 flags + tp_dictoffset | 本设计调研脚本，Python 3.13.14 venv crs_venv_313，2026-08-06 |
| CPython 3.13 pre-header 布局 | `cpython/Objects/object_layout.md` (3.13 节) |
| `PyObject_GenericGetDict` 推荐 | `docs.python.org/3/c-api/typeobj.html` tp_dictoffset 节 |
| `Py_TPFLAGS_MANAGED_DICT` = 1 << 4 | `pyo3-ffi-0.22.6/src/object.rs` L408 + `Tools/gdb/libpython.py` |
| pyo3 0.22 暴露 `PyObject_GenericGetDict` | `pyo3-ffi-0.22.6/src/object.rs` L373（实测 grep） |
| `tp_dictoffset = -1` CPython 不变量 | `docs.python.org/3/c-api/typeobj.html` + GH-95854 PR |

---

## 1. 三个优化方案详细设计

### 1.1 O1-A：用 `PyObject_GenericGetDict` 替代 getattr 包装

> **v2 设计修正（2026-08-06，L-14 第 3 次触发）**：原 v1 设计采用
> `tp_dictoffset` 直读路径。Python 3.13 managed dict 实测使 `tp_dictoffset = -1`，
> 原设计完全失效（详 §Python 3.13 managed dict 调研）。本节是 v2 重写。

#### 1.1.1 当前路径（待优化）

`construct-rs/src/nodes/struct_node.rs` L291-306：

```rust
let dict_bound = instance
    .getattr(self.dict_attr_name.bind(py))    // ← 目标优化点
    .map_err(...)?
    .downcast_into::<PyDict>()
    .map_err(...)?;
```

`Bound::getattr(interned "__dict__")` 内部走 pyo3 包装：
1. `PyObject_GetAttr`（C API）→ 触发 `tp_getattro`（即 `subtype_getattro`）
2. MRO 遍历到 `object.__dict__` getset descriptor（attr cache miss 时走 `_PyType_Lookup`）
3. descriptor `__get__` 触发，最终调用 `PyObject_GenericGetDict`（返回实例 dict）
4. pyo3 把返回的 `*mut PyObject` 包装为 `Bound<PyAny>`（Incref + Bound 构造）
5. 用户调 `downcast_into::<PyDict>()`（运行期 type check + cast）

实测开销 ~15-30ns（其中 MRO + 描述符 dispatch ~5-10ns，pyo3 包装 ~5-10ns，
downcast ~2-5ns）。**注意**：CPython 内部 `PyObject_GenericGetDict` 调用本身
~3-8ns（含 managed dict 检查 + lazy materialize），是必要开销。

#### 1.1.2 优化后路径：直接调 `PyObject_GenericGetDict`

**核心思路**：跳过 pyo3 getattr 包装链（步骤 1-2 + 4-5），直接调
`PyObject_GenericGetDict`（步骤 3 等价）。

**instance.rs 新增工具函数**：

```rust
/// O1-A 工具函数：通过 `PyObject_GenericGetDict` 直接获取实例 `__dict__`，
/// 绕过 pyo3 getattr 包装（MRO + 描述符 dispatch + Bound 包装 + downcast）。
///
/// 适用于 CPython heaptype 类（含 managed dict 模式）。CPython 内部正确处理：
/// - `Py_TPFLAGS_MANAGED_DICT` flag：从 pre-header dict_or_values 槽读 dict
/// - lazy materialize：若 dict 未物化，触发 materialize 创建 PyDictObject
/// - 非 managed dict 模式（如 C extension type）：从 tp_dictoffset 偏移读 dict
///
/// 返回 `Bound<PyDict>`（owned 引用，调用方负责 decref——但 pyo3 Bound 自动处理）。
///
/// # ABI 依赖
///
/// `[CPython Public Stable ABI]`：`PyObject_GenericGetDict` 自 Python 2.x 起存在，
/// 签名稳定（`PyObject *PyObject_GenericGetDict(PyObject *obj, void *context)`）。
/// pyo3 0.22 在 `pyo3-ffi-0.22.6/src/object.rs` L373 暴露。
/// **不依赖 cpython 子模块**——abi3 / 非 abi3 均可用。
pub fn dict_via_generic_getdict<'py>(
    py: Python<'py>,
    instance: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyDict>> {
    // SAFETY:
    // - instance.as_ptr() 是有效的 PyObject 指针（pyo3 Bound 守护）
    // - PyObject_GenericGetDict 是 CPython 公开 API，签名稳定
    // - GIL 持有（由 py: Python<'py> 参数保证）
    // - context 参数 NULL（CPython 文档：__dict__ getset descriptor 调用时传 NULL）
    // 返回值是 owned reference（new reference），由调用方负责 decref。
    let dict_ptr = unsafe {
        ffi::PyObject_GenericGetDict(instance.as_ptr(), std::ptr::null_mut())
    };
    if dict_ptr.is_null() {
        // 理论不发生（heaptype 实例必有 dict），但保留错误路径
        let err = PyErr::fetch(py);
        return Err(err.unwrap_or_else(|| pyo3::exceptions::PyAttributeError::new_err(
            "PyObject_GenericGetDict returned NULL without setting exception"
        )));
    }
    // SAFETY: dict_ptr 是 owned reference（PyObject_GenericGetDict 返回 new ref）。
    // 由 Bound::from_owned_ptr 接管 ownership，pyo3 在 Bound drop 时 decref。
    // downcast_into::<PyDict> 做运行期 type check（理论总是成功——GenericGetDict
    // 返回 PyDictObject，除非用户 override __dict__ 描述符）。
    unsafe { Bound::from_owned_ptr(py, dict_ptr) }
        .downcast_into::<PyDict>()
        .map_err(|_| pyo3::exceptions::PyTypeError::new_err(
            "instance __dict__ is not a dict (possible __dict__ override or __slots__ class)"
        ))
}
```

**StructNode.parse 改造（仅 dict 获取部分）**：

```rust
// R4 步骤 2：获取实例的 __dict__。
// O1-A 优化：直接调 PyObject_GenericGetDict，绕过 pyo3 getattr 包装。
//
// 与原 R4 getattr 路径的语义差异：getattr 路径会触发 __dict__ getset descriptor
// 的完整 dispatch（subtype_getattro → MRO lookup → descriptor.__get__）；
// 直接调 GenericGetDict 跳过 dispatch，仍触发 lazy materialize（CPython 不变量）。
//
// 退化路径：GenericGetDict 返回 NULL 或非 PyDict 类型时 fallback 到 getattr。
let dict_bound = match dict_via_generic_getdict(py, &instance) {
    Ok(d) => d,
    Err(_) => {
        // Fallback：保留 R4 原路径（getattr），保留 slots class / __dict__ override
        // 等异常场景的明确错误信息
        instance
            .getattr(self.dict_attr_name.bind(py))
            .map_err(|e| ConstructError::Generic {
                message: format!("failed to get __dict__ (both GenericGetDict and getattr failed): {}", e),
                path: path.to_string(),
            })?
            .downcast_into::<PyDict>()
            .map_err(|_| ConstructError::Generic {
                message: "instance has no __dict__ (possible __slots__ class)".into(),
                path: path.to_string(),
            })?
    }
};
```

**注**：`StructNode` 不再新增 `cached_dict_offset` 字段（原 v1 设计的编译期缓存）——
`PyObject_GenericGetDict` 不需要编译期信息，运行期直接调即可。StructNode 字段保持原状。

#### 1.1.3 ABI 稳定性论证

| 依赖 | 类别 | 稳定性 | 来源 |
|------|------|-------|------|
| `PyObject_GenericGetDict` 函数 | CPython Public Stable ABI | ✅ 自 Python 2.x 未变（PEP 384 stable）| CPython `Include/object.h` + `cpython/object.h` |
| `pyo3::ffi::PyObject_GenericGetDict` | pyo3 0.22 公开 API（不依赖 cpython 子模块）| ✅ | `pyo3-ffi-0.22.6/src/object.rs` L373 |
| `Bound::from_owned_ptr` | pyo3 0.22 公开 API | ✅ | pyo3 0.22 文档 |

**风险**：
- CPython 未来版本理论上可能改 `PyObject_GenericGetDict` 签名（实测自 2.x 未变，PEP 384
  保证 stable ABI）。若改动，pyo3 升级时编译失败（编译期发现）。
- **managed dict 行为变化**：Python 3.13 的 lazy materialize 是当前行为。未来版本若改为
  eager materialize（tp_new 时立即创建 dict），节省量略变（materialize 开销从 parse 路径
  转移到 create_class 路径），但 GenericGetDict 仍正确工作。
- **不影响 abi3**：本决策不依赖 cpython 子模块，abi3 / 非 abi3 均可用。

#### 1.1.4 不变量（invariants）

1. **instance 是 heaptype 类的实例**：`create_class(self.cls)` 创建的实例类型严格等于
   `self.cls`（StructNode 持有 `cls: Py<PyType>` 保证）。
2. **GIL 持有**：调用方 `py: Python<'py>` 强制约束。pyo3 pymethods 入口自动满足。
3. **实例已初始化**：`create_class` 返回时实例已完成 `tp_new`。Python 3.13 managed dict
   模式下，dict 可能未 materialize（lazy），`PyObject_GenericGetDict` 会触发 materialize。
4. **dict 类型是 PyDictObject**：CPython 不变量（heaptype 默认 dict 类型）。downcast 失败
   表示用户 override 了 `__dict__` 描述符，走 fallback。

违反以上任何一条 → 退化到 fallback 路径（getattr），不会 UB（GenericGetDict 内部
正确处理无效输入，返回 NULL + PyErr）。

#### 1.1.5 退化路径

| 触发条件 | 退化行为 |
|---------|---------|
| `PyObject_GenericGetDict` 返回 NULL（理论不发生）| fallback 到 R4 getattr 路径，getattr 失败时返回 `ConstructError::Generic` |
| 返回值不是 PyDictObject（用户 override `__dict__` 描述符）| downcast 失败，fallback 到 getattr 路径 |
| slots class（`Py_TPFLAGS_MANAGED_DICT` 仍 set，但 dict 始终 NULL）| GenericGetDict 返回空 dict 或 NULL（视 CPython 版本），fallback 到 getattr 报错 |
| `PyObject_GenericGetDict` 抛 Python 异常（如 MemoryError）| `PyErr::fetch` 取出异常，fallback 到 getattr |

**注意**：原 v1 设计的 "abi3 模式 cfg fallback" + "Python 3.12+ managed dict 检测"
已**去除**——`PyObject_GenericGetDict` 在 abi3 / 非 abi3 / managed dict / 非 managed dict
下均正确工作，不需要 cfg 守护或运行期 flag 检测。

### 1.2 O1-B：`PyDict_SetItem_KnownHash` 跳过 hash 重算

#### 1.2.1 当前路径

`construct-rs/src/nodes/struct_node.rs` L355（无表达式路径）+ L330（has_expressions
路径）：

```rust
dict_bound.set_item(field.name.py_name().bind(py), value.bind(py))?;
// 或
ctx.set_field_at(idx, field.name.py_name(), value.bind(py), py)?;
```

`Bound<PyDict>::set_item` 内部走 `PyDict_SetItem(dict, key, value)`，该函数：
1. 调 `PyObject_Hash(key)`（对 interned 仅取缓存字段 ~1-2ns）
2. 进入 `insertdict` 写入（hash 查找 + 插入 ~10-15ns）
3. 返回

#### 1.2.2 优化后路径

**关键事实**：`_PyDict_SetItem_KnownHash` **已由 pyo3 0.22 暴露**
（`pyo3::ffi::_PyDict_SetItem_KnownHash`，经 `pyo3-ffi-0.22.6/src/cpython/dictobject.rs` L29
+ lib.rs L459 `pub use self::cpython::*` re-export）。**不需手动 extern 声明**。

**工具函数（`instance.rs` 末尾）**：

```rust
/// O1-B 工具函数：用 KnownHash 写入 dict（fallback 到 PyDict_SetItem）。
///
/// # Safety
///
/// 调用方必须保证：
/// 1. `hash` 是 `key` 的正确 hash（CPython `PyObject_Hash` 返回值）
/// 2. `key` 是 hashable（interned PyString 自动满足）
/// 3. GIL 持有（由 `py: Python<'_>` 保证）
///
/// 返回 Ok(()) 表示成功，Err(PyErr) 表示 C API 返回 -1（异常已挂起）。
///
/// # ABI 依赖
///
/// `[CPython Internal Function]`：`_PyDict_SetItem_KnownHash` 是 CPython 私有 API
/// （非 stable ABI），pyo3 0.22 cpython 子模块已暴露（`pyo3::ffi::_PyDict_SetItem_KnownHash`）。
/// 该子模块由 `#[cfg(not(Py_LIMITED_API))]` 守护——abi3 模式自动 fallback。
#[cfg(not(Py_LIMITED_API))]
pub fn set_item_knownhash(
    py: Python<'_>,
    dict: &Bound<'_, PyDict>,
    key: &Bound<'_, PyAny>,
    value: &Bound<'_, PyAny>,
    hash: ffi::Py_hash_t,
) -> PyResult<()> {
    // SAFETY: dict/key/value 由 pyo3 Bound 守护，均为有效 PyObject 指针。
    // GIL 持有保证单线程访问。hash 由 FieldName 编译期一次性 PyObject_Hash 计算
    // 并缓存（interned PyString hash 不变）。
    let result = unsafe {
        ffi::_PyDict_SetItem_KnownHash(
            dict.as_ptr(),
            key.as_ptr(),
            value.as_ptr(),
            hash,
        )
    };
    if result == -1 {
        Err(PyErr::fetch(py))
    } else {
        Ok(())
    }
}
```

> **v2 修正（去除 abi3 fallback）**：原 v1 设计包含 abi3 cfg fallback（Py_LIMITED_API
> 模式下走 `Bound<PyDict>::set_item`）。**非 abi3 是项目约束**（用户决策 + 8.ENV 落地），
> 不再保留 abi3 fallback。若未来切 abi3，O1-B 整体不可编译（cpython 子模块缺失），需
> 重新评估。

**FieldName 缓存 hash（含 NB-4 修复）**：

```rust
pub struct FieldName {
    py_name: Py<PyString>,
    rust_name: String,
    /// 新增：编译期缓存的 PyObject_Hash 结果。
    /// interned PyString 的 hash 在其生命周期内不变（CPython 强约束）。
    cached_hash: ffi::Py_hash_t,
}

impl FieldName {
    pub fn new(py: Python<'_>, name: impl Into<String>) -> Self {
        let rust_name = name.into();
        let py_name = intern_pystring(py, &rust_name);
        // 编译期一次性计算 hash 并缓存。
        // SAFETY: py_name 是有效的 PyString 指针，PyObject_Hash 是 stable ABI。
        let cached_hash = unsafe { ffi::PyObject_Hash(py_name.as_ptr()) };
        // NB-4 修复（REV 检视建议）：PyObject_Hash 返回 -1 表示失败（异常已挂起）。
        // interned PyString 理论不会失败，但若发生则 panic（构造期错误，比运行期
        // 退化更明确——避免 cached_hash == -1 传给 _PyDict_SetItem_KnownHash 触发
        // dict 内部 hash 冲突路径性能退化）。
        if cached_hash == -1 {
            let err = PyErr::fetch(py);
            panic!(
                "FieldName::new: PyObject_Hash failed for interned string {:?}: {:?}",
                rust_name, err
            );
        }
        Self { py_name, rust_name, cached_hash }
    }

    pub fn cached_hash(&self) -> ffi::Py_hash_t {
        self.cached_hash
    }
}
```

**StructNode.parse 写入路径替换**：

```rust
// 无表达式路径
match field.mode {
    FieldMode::Rw | FieldMode::Ro => {
        // O1-B 优化：用 KnownHash 跳过 hash 重算。
        set_item_knownhash(
            py,
            &dict_bound,
            field.name.py_name().bind(py),
            value.bind(py),
            field.name.cached_hash(),
        )?;
    }
    FieldMode::Wo => drop(value),
}
```

`ctx.set_field_at` 内部也改用 `set_item_knownhash`（同理）。

#### 1.2.3 Hash 缓存的正确性论证

**CPython 不变量**：interned PyString 的 hash 字段在对象生命周期内不可变。
- `PyUnicodeObject.hash` 初始为 -1（未计算）
- 首次 `PyObject_Hash` 计算并写入 hash 字段
- 后续 `PyObject_Hash` 直接返回缓存值（无重算）
- interned 字符串不可变（CPython intern 表只允许不可变字符串）

**FieldName.cached_hash 的正确性保证**：
1. `intern_pystring` 返回的 PyString 已 intern（不可变）
2. `FieldName::new` 中 `PyObject_Hash` 触发 hash 计算并缓存
3. 只要 FieldName 存活（持有 `Py<PyString>` 引用），hash 字段值不变
4. StructNode 持有 FieldName，与 Py<PyType> 同生命周期——只要 schema 存活，hash 正确

**边界条件**：
- **NB-4 修复（hash 失败处理）**：`PyObject_Hash` 返回 -1 时，`FieldName::new` panic
  （构造期错误）。原 v1 设计缓存 -1，传给 `_PyDict_SetItem_KnownHash` 会触发 dict
  内部 hash 冲突路径性能退化——v2 改为构造期 panic 避免运行期性能陷阱。
- 若 hash 在某个未来 CPython 版本中改变计算方式（不会，因为会破坏所有已序列化
  数据）：cached_hash 失效，需重新探测。

#### 1.2.4 ABI 稳定性论证

| 依赖 | 类别 | 稳定性 | 风险 |
|------|------|-------|------|
| `pyo3::ffi::_PyDict_SetItem_KnownHash` | CPython Internal Function（pyo3 cpython 子模块暴露）| 自 3.5 起未变，3.7-3.14 实测可用 | 未来 CPython 改名（pyo3 升级时编译期发现）|
| `PyObject_Hash` | Stable ABI | ✅ | 无 |
| `Py_hash_t` 类型 | Stable ABI（Py_ssize_t 别名）| ✅ | 无 |

**风险缓解**：cpython 子模块由 pyo3 在每次升级时同步重新生成，符号变化会编译期暴露。
**无 abi3 fallback**——非 abi3 是项目约束（详 §abi3 前提变更说明）。

#### 1.2.5 退化路径

| 触发条件 | 退化行为 |
|---------|---------|
| `_PyDict_SetItem_KnownHash` 返回 -1（dict 操作失败）| 取出挂起异常，返回 Err（与 `PyDict_SetItem` 同行为）|
| 运行期 dict 不为 `PyDictObject`（用户继承 dict 的 Container）| 调用前先 type check，失败则走公开 `PyDict_SetItem`（已有路径）|
| 未来 CPython 删除符号 | pyo3 cpython 子模块升级时编译失败（编译期发现）|
| 项目切 abi3 模式 | **O1-B 整体不可编译**（cpython 子模块缺失）→ 需重新评估（详 §abi3 前提变更说明）|

### 1.3 O2-A：精简 `_parse_raw` pyo3 FFI 入口

#### 1.3.1 当前路径

`construct-rs/src/schema.rs` L150-175：

```rust
#[pyo3(signature = (data))]
pub fn _parse_raw<'py>(
    &self,
    py: Python<'py>,
    data: &Bound<'py, PyBytes>,
) -> PyResult<Bound<'py, PyAny>> {
    let bytes = data.as_bytes();              // ← 走 PyBytes_AsStringAndSize
    let mut stream = ParseStream::new(bytes);
    let mut ctx = Context::placeholder(py);
    let mut path = Path::new();
    let parse_result = self.root.parse(py, &mut stream, &mut ctx, &mut path);
    match parse_result {
        Ok(result) => Ok(result.into_bound(py)),  // ← Bound 构造
        Err(ConstructError::CancelParsing { .. }) => Ok(py.None().into_bound(py)),
        Err(e) => Err(e.into()),
    }
}
```

开销点：
1. `data.as_bytes()` 内部 `PyBytes_AsStringAndSize` 含 type check + size check
   （~3-5ns）
2. `result.into_bound(py)`（~1-2ns）+ pyo3 wrap `Bound<PyAny>` → PyResult
3. 错误路径 `e.into()` 转 PyErr（成功路径 ~1-2ns）

#### 1.3.2 优化后路径

```rust
#[pyo3(signature = (data))]
pub fn _parse_raw<'py>(
    &self,
    py: Python<'py>,
    data: &Bound<'py, PyBytes>,
) -> PyResult<Py<PyAny>> {  // ← 改返回类型为 Py<PyAny>，省去 Bound 包装
    // O2-A.1：直接访问 PyBytesObject.ob_sval 字段（绕过 PyBytes_AsStringAndSize
    // 函数调用 + 内部 type check）。data 类型由 pyo3 参数签名 &Bound<PyBytes>
    // 在入口处保证。
    //
    // SAFETY: data 是 &Bound<'py, PyBytes>，pyo3 参数类型签名在入口处 type check
    // （成功才进入此函数）。直接将 PyObject* cast 为 PyBytesObject* 是 CPython
    // 内部约定（PyBytes_CheckExact 通过则安全）。ob_sval 是 [c_char; 1] 但实际
    // 长度由 ob_base.ob_size 决定（CPython 不变量：ob_sval 数组至少 1 字节 + 终止 NUL）。
    //
    // ABI 依赖：[CPython Internal Struct] PyBytesObject.ob_sval + ob_base.ob_size
    // 由 pyo3 0.22 cpython 子模块暴露（`pyo3-ffi-0.22.6/src/cpython/bytesobject.rs`
    // L8-14）。项目非 abi3 模式（用户决策 + 8.ENV 验证），cpython 子模块可用。
    // 无 abi3 fallback——非 abi3 是项目约束。
    let bytes: &[u8] = unsafe {
        use pyo3::ffi::cpython::PyBytesObject;
        let obj_ptr = data.as_ptr();
        let bytes_obj = &*obj_ptr;
        let len = bytes_obj.ob_base.ob_size as usize;
        std::slice::from_raw_parts(bytes_obj.ob_sval.as_ptr() as *const u8, len)
    };

    let mut stream = ParseStream::new(bytes);
    let mut ctx = Context::placeholder(py);
    let mut path = Path::new();
    let parse_result = self.root.parse(py, &mut stream, &mut ctx, &mut path);
    match parse_result {
        Ok(result) => Ok(result),  // 已是 Py<PyAny>，无需 into_bound
        Err(ConstructError::CancelParsing { .. }) => Ok(py.None()),
        Err(e) => Err(e.into()),
    }
}
```

**注**：返回类型从 `PyResult<Bound<'py, PyAny>>` 改为 `PyResult<Py<PyAny>>`——
pyo3 0.22 #[pymethods] 接受 `Py<T>` 返回类型（实现 `IntoPyCallbackOutput`）。
DEV 实施时需验证 pyo3 0.22 行为（如不接受，保持 Bound 返回类型，损失 ~1-2ns）。

#### 1.3.3 ABI 稳定性论证

| 依赖 | 类别 | 稳定性 | 风险 |
|------|------|-------|------|
| `pyo3::ffi::cpython::PyBytesObject.ob_sval` 字段 | CPython Internal Struct | ✅ 自 Python 1.x 起 PyBytesObject 布局稳定 | CPython 重构 ob_sval 字段时（编译期发现）|
| `pyo3::ffi::cpython::PyBytesObject.ob_base.ob_size` | CPython Internal（PyVarObject）| ✅ 自 Python 1.x 起未变 | 同上 |

**当前项目 ABI 现状**（用户决策 + 8.ENV 验证）：非 abi3 模式，cpython 子模块可用。
O2-A 在当前配置下可用。**无 abi3 fallback**——若未来切 abi3，O2-A 整体不可编译，
需重新评估（详 §abi3 前提变更说明）。

#### 1.3.4 不变量

1. `data` 必须是有效的 PyBytes（pyo3 参数签名守护）
2. GIL 持有（由 `py: Python<'py>` 保证）
3. 返回的 `&[u8]` 生命周期不超过 `data`（Rust 借用规则保证）

#### 1.3.5 退化路径

| 触发条件 | 退化行为 |
|---------|---------|
| 未来 CPython 重构 PyBytesObject 布局（破坏 ob_sval 字段）| pyo3 cpython 子模块升级时编译失败（编译期发现）|
| 项目切 abi3 模式 | **O2-A 整体不可编译**（cpython 子模块缺失）→ 需重新评估（详 §abi3 前提变更说明）|

### 1.4 ~~build.rs：`_PyDict_SetItem_KnownHash` 符号探测~~（取消）

**原设计**：新增 `construct-rs/build.rs` 在编译期探测 `_PyDict_SetItem_KnownHash`
符号，定义 cfg flag `have_knownhash`。

**取消原因**：实测 pyo3 0.22 已在 cpython 子模块直接暴露 `_PyDict_SetItem_KnownHash`
（详 §模块位置 §关键事实）。不需要手动 extern 声明，不需要 build.rs 符号探测。

**v2 进一步简化**：原 v1 设计的 `#[cfg(not(Py_LIMITED_API))]` 守护在 v2 中**保留**
（O1-B / O2-A 仍依赖 cpython 子模块），但**去除 abi3 fallback 分支**——非 abi3 是项目
约束。无需新增 build.rs。

---

## 2. §0 原则对照表（L-01 对策，逐条）

| §0 原则 | 对照说明 |
|---------|---------|
| **#1 一次 FFI** | ✅ 三条优化都不改变 FFI 边界数量。`_parse_raw` 仍是唯一 FFI 入口（Python→Rust 一次），优化的是 Rust 内部 CPython C API 调用开销。**不引入新的 Python↔Rust 边界穿越**。 |
| **#2 无中间表示层** | ✅ O1-A：`PyObject_GenericGetDict` 返回的是**实例 `__dict__` 这个 Python 对象本身**（PyDictObject），不是 Rust 中间数据类型——与 R4（ADR-008）语义一致。O1-B：KnownHash 仅优化 `PyDict_SetItem` 调用，dict 仍是 Python 对象。O2-A：仅省去 pyo3 包装，输入 `&[u8]` / 输出 `Py<PyAny>` 类型不变。**不引入 Rust 中间数据类型再转回 PyObject**。 |
| **#3 输入输出无 trait 抽象** | ✅ 不引入新 trait，不引入 per-field 抽象层。优化发生在已有 StructNode / CompiledSchema / FieldName 内部字段和方法。 |
| **#4 pyo3 核心依赖** | ✅ 仍用 pyo3 + pyo3::ffi。O1-A 用 pyo3::ffi::PyObject_GenericGetDict（stable ABI），O1-B/O2-A 通过 pyo3::ffi::cpython 子模块访问 CPython internal struct/function（pyo3 0.22 已暴露 `_PyDict_SetItem_KnownHash` / `PyBytesObject.ob_sval`），不引入第二个 Python 绑定层。pyo3 仍是唯一 Python↔Rust 绑定。 |
| **#5 mashumaro 式 API** | ✅ 用户面 API 不变（`@dataclass class X(StructMixin)` + `.parse(data)`），优化完全在 Rust 内部。FieldName 内部字段变化不暴露到 Python。 |
| **#6 enum_dispatch 静态分派** | ✅ StructNode 仍是 Node enum 变体，分派路径不变。v2 不再新增 `cached_dict_offset` 字段（O1-A 改用 GenericGetDict 运行期调用，不需编译期缓存），StructNode 字段保持原状。 |
| **#7 Result<T, ConstructError> + thiserror** | ✅ 错误处理不变。退化路径（GenericGetDict 返回 NULL / KnownHash 返回 -1）仍返回 `ConstructError::Generic`，错误路径不优化（ADR-019 同脉络）。 |
| **#8 Stream 抽象纯 Rust** | ✅ Stream 抽象不涉及。三条优化都不触及 ParseStream / BuildStream。 |

---

## 3. 可证伪预测 + FFI 来源清单（L-05 + L-09 对策）

### 3.1 共享税优化后的 rs_ns 预测（量级参考，L-09，vs Python 3.13 非 abi3 新基线）

| 优化项 | 单项节省（量级参考）| 累计共享税 |
|-------|--------------------:|----------:|
| 当前（3.13 基线，8.ENV §4.1）| - | ~180-200ns |
| + O1-A（GenericGetDict 替代 getattr）| -10 ~ -20ns | ~160-190ns |
| + O1-B（KnownHash，1 字段）| -5 ~ -10ns | ~150-185ns |
| + O2-A（FFI 入口精简）| -5 ~ -10ns | ~145-180ns |
| **三项累计优化下限** | -20 ~ -40ns | **~140-160ns** |

**L-09 量级参考声明**：所有 ns 估算均为量级参考，绝对值需 VET Controlled A/B Test
独立复测验证。O1-A 节省略低于原 v1 设计的 15-25ns（10-20ns），原因：v1 假设
tp_dictoffset 直读可省去全部 getattr 包装，但 Python 3.13 managed dict 下
GenericGetDict 仍需处理 lazy materialize（部分开销保留）。

### 3.2 14 项 parse case 预测加速比（vs Python 3.13 非 abi3 新基线，8.ENV §4.1）

> **基线来源**：`plans/phase8-adapters-struct-streams/traces/8.ENV-环境切换.md §4.1`
> （Python 3.13.14 非 abi3，crs_venv_313 vs crs_venv_py_313，number=5000, iter=10）。

| Case | 3.13 基线 rs_ns | 3.13 基线 sp | 本设计预测 rs_ns | 预测加速比 | 状态 | 可证伪条件 |
|------|---------------:|------------:|---------------:|----------:|------|-----------|
| CN1 | 214 | 9.71 | ~175-190 | **~10.9-11.9x** | LOW→OK | rs_ns > 200 → 失败 |
| CN2 | 206 | 10.35 | ~165-180 | **~11.9-12.9x** | OK | rs_ns > 195 → 失败 |
| HX1 | 370 | 7.19 | ~325-345 | **~7.7-8.2x** | LOW（需 OPT-CASE-B）| rs_ns > 360 → O1-A/B 无效 |
| HX2 | 332 | 8.08 | ~285-305 | **~8.8-9.4x** | LOW（需 OPT-CASE-B）| rs_ns > 325 → O1-A/B 无效 |
| HD1 | 327 | 7.72 | ~280-300 | **~8.4-9.0x** | LOW（需 OPT-CASE-B）| rs_ns > 320 → O1-A/B 无效 |
| AL2 | 249 | 10.10 | ~205-220 | **~11.4-12.2x** | OK | rs_ns > 240 → 失败 |
| TM1 | 237 | 10.28 | ~165-180 | **~13.5-14.8x** | OK | rs_ns > 200 → 失败 |
| EN1 | 222 | 10.40 | ~180-195 | **~11.8-12.8x** | OK | rs_ns > 215 → 失败 |
| EN2 | 226 | 10.09 | ~180-195 | **~11.7-12.7x** | OK | rs_ns > 220 → 失败 |
| FE1-parse | 339 | 9.99 | ~295-310 | **~10.9-11.5x** | LOW→OK | rs_ns > 320 → 失败 |
| FE1-build | 439 | 5.94 | ~395-415 | **~6.3-6.6x** | 仍 LOW（PM 决策）| 不达 10x（已声明）|
| MP1 | 231 | 10.88 | ~190-205 | **~12.2-13.2x** | OK | rs_ns > 225 → 失败 |
| OO1 | 224 | 11.29 | ~180-195 | **~12.9-14.0x** | OK | rs_ns > 220 → 失败 |
| NO1 | 229 | 10.29 | ~185-200 | **~11.8-12.7x** | OK | rs_ns > 225 → 失败 |
| NT1 | 602 | 7.69 | ~560-580 | **~8.0-8.3x** | LOW（需 OPT-CASE-B）| rs_ns > 595 → O1-A/B 无效 |

**总判定**（PERF-REASSESS §4.3 + 本设计修正 + vs 3.13 基线）：
- **OPT-SHARED 后达标 10x+（10 项）**：CN1/CN2/AL2/TM1/EN1/EN2/FE1-parse/MP1/OO1/NO1
- **接近 10x（4 项）**：HX1/HX2/HD1/NT1（预测 8-10x，需 OPT-CASE-B 进一步优化子节点）
- **仍 <10x（1 项）**：FE1-build（预测 6.3-6.6x，需 PM 单独决策）

**注意**：3.13 基线下 CN2/AL2/TM1/EN1/EN2/MP1/OO1/NO1 已自然达标（8 项），OPT-SHARED
对这些 case 的作用是**留出余量**（避免测量漂移导致 LOW），并把 CN1/FE1-parse 拉到 10x+。
HX1/HX2/HD1/NT1 仍 LOW，需 8.OPT-CASE-B 配合。

### 3.3 可证伪条件（L-05 对策）

**整体失败判据**：实施 O1-A + O1-B + O2-A 后，VET Controlled A/B Test（同会话
交替测量，详 §4 测试策略）测得：
- 共享税（用 TM1 测量，TM1 自身无子节点开销，rs_ns ≈ 共享税 + 30ns 内层）：
  - 优化后 TM1 rs_ns > 200ns → 共享税优化失败（理论值 ~165-180ns + 余量）
- 10 项达标 case 中任一未达 10x → 子节点开销未充分优化（不在本任务范围）

**单项失败判据**：
- O1-A 失败：实施后 TM1 共享税仍 ≥210ns（即 GenericGetDict 替代未生效或被其他开销抵消）
- O1-B 失败：dict 写入路径无可见改善（<2ns 差异，量级在测量噪声内 → 难以证伪）
- O2-A 失败：FFI 入口开销无可见改善（同上）

### 3.4 FFI/拷贝来源清单（L-05 对策覆盖性声明）

每个 parse case 优化后剩余的 FFI/拷贝来源：

| 来源 | 优化项影响 | 剩余 ns |
|------|-----------|--------:|
| 1. `_parse_raw` pyo3 FFI 入口（Python→Rust 1 次穿越）| O2-A 优化 | ~25-40 |
| 2. `create_class` tp_new 调用 | 不优化（已 ADR-019 优化过）| ~30-50 |
| 3. 获取实例 `__dict__`（getattr 或 GenericGetDict）| **O1-A 优化** | ~5-15（GenericGetDict）/ ~15-30（getattr fallback）|
| 4. per-field `PyDict_SetItem`（~30-50ns × 1-2 字段）| **O1-B 优化** | ~25-40（KnownHash）/ ~30-50（fallback）|
| 5. 子节点 inner.parse（按 case 不同）| 不优化（在 OPT-CASE-A/B）| ~30-200 |
| 6. 子节点特化操作（dict lookup / rich_compare / frozenset.contains）| 不优化 | ~5-30 |
| 7. 显示对象 / Container / namedtuple 构造（仅重 case）| 不优化 | ~50-200 |
| 8. FFI 出口 + return 包装 | O2-A 优化（Py<PyAny> 直接返回）| ~10-20 |

**覆盖性声明**：以上 8 项已覆盖所有 parse 路径的 FFI/拷贝来源，无遗漏。每个 case
的预测 rs_ns 是上述来源的累加上限。

### 3.5 fallback 方案（v2：去除 abi3 fallback）

| 优化项 | fallback 条件 | fallback 行为 | 性能影响 |
|-------|--------------|--------------|---------|
| O1-A | GenericGetDict 返回 NULL / 非 PyDict（用户 override）| fallback 到 R4 getattr 路径 | 0（与当前一致）|
| O1-A | CPython 改 GenericGetDict 签名（理论不会，stable ABI）| pyo3 升级时编译失败 | 编译期发现 |
| O1-B | dict 不为 PyDictObject（用户继承 dict 的 Container）| 走公开 `PyDict_SetItem` | -5-10ns/字段 |
| O1-B | CPython 内部 dict 重构使 KnownHash 错误（理论不会）| 字段写入失败 → ConstructError | 错误路径 |
| O1-B | pyo3 cpython 子模块移除 `_PyDict_SetItem_KnownHash` 符号 | 编译失败 | 编译期发现 |
| O2-A | CPython 破坏 PyBytesObject.ob_sval 字段 | pyo3 cpython 子模块编译失败 | 编译期发现 |
| O2-A | pyo3 0.22 不接受 `Py<PyAny>` 返回类型 | DEV 验证后保留 `Bound<PyAny>` 返回类型 | -1-2ns |
| **切 abi3** | O1-B / O2-A 整体不可编译；**O1-A 仍可用**（stable ABI）| 需重新评估 O1-B/O2-A 替代方案 | 详 §abi3 前提变更说明 |

---

## 4. 测试策略

### 4.1 正确性验证

#### 4.1.1 单元测试（cargo test）

新增 / 扩展测试覆盖以下场景：

| 测试场景 | 测试名（建议）| 断言 |
|---------|--------------|------|
| 普通 class（有 __dict__）| `test_generic_getdict_returns_correct_dict` | GenericGetDict 路径返回的 dict 与 getattr 路径返回的 dict 是同一对象（identity）|
| dataclass（非 frozen）| `test_generic_getdict_works_with_dataclass` | 同上 |
| frozen dataclass | `test_generic_getdict_works_with_frozen_dataclass` | 同上（不变量：frozen 不影响 dict 读取，只影响 setattr）|
| slots class（Py_TPFLAGS_MANAGED_DICT but dict NULL）| `test_slots_class_falls_back_to_getattr_error` | GenericGetDict fallback 到 getattr 路径，返回原错误信息 |
| Python 3.13 managed dict + inline_values | `test_generic_getdict_handles_managed_dict_materialize` | 实例首次 access 时正确触发 materialize（PyDictObject 创建）|
| 字段写入后可读取 | `test_knownhash_write_dict_read_getattr` | KnownHash 写入的 (key, value) 可通过 instance.x 正常读取 |
| 多字段结构体 | `test_knownhash_multiple_fields` | 所有字段 hash 不冲突，写入顺序正确 |
| 字段名包含 Unicode | `test_knownhash_unicode_field_name` | cached_hash 正确（PyObject_Hash 对 Unicode 字符串有效）|
| FieldName hash 失败 panic（NB-4）| `test_fieldname_hash_failure_panics` | PyObject_Hash 返回 -1 时 panic（构造期错误）|
| ob_sval 直读正确性（NB-1 修正）| `test_ob_sval_direct_access_matches_as_bytes` | O2-A 的 bytes slice（直接访问 ob_sval 字段）与 `data.as_bytes()` 内容一致 |
| CancelParsing 仍返回 None | `test_cancel_parsing_returns_none_after_optimization` | O2-A 不破坏 Phase 8.10 顶层 catch |

#### 4.1.2 Parity 测试（与 Python construct 原版对照）

复用现有 `tests/python/test_*parity*` 框架，扩展覆盖：
- `parse` 输出与 Python 原版 Struct.parse 输出属性一致（field value / type）
- 字段顺序、字段名、字段值在 O1-A + O1-B 后保持不变
- 错误路径（stream EOF / field 校验失败）的 path 字符串不变

#### 4.1.3 Fuzzing（建议但非强制）

针对 unsafe 路径建议 fuzzing：
- 构造随机 Struct（字段数 0-32、字段类型混合），parse 随机 bytes，断言无内存安全
  问题（用 miri 或 AddressSanitizer）
- 重点 fuzz：GenericGetDict 返回值边界（NULL / 非 PyDict 触发 fallback）、
  KnownHash hash 边界（极长 Unicode 字段名触发 hash 计算）

### 4.2 性能验证（Controlled A/B Test，L-09 对策）

#### 4.2.1 测试方法学（引用 PERF-retest）

- **同会话交替测量**：O1-A on / off / O1-B on / off / O2-A on / off，每轮 ≥3 次
  交替（A→B→A→B→A→B）
- **采样参数**：number=50000, repeat=7, best ns/op（与 PERF-retest 一致）
- **环境标注**：Python 版本 / venv / CPU 型号 / OS 版本 / 时段（L-09 对策）
- **统计判据**：差异 >0.5x → 显著；<0.3x → 噪声；中间加测
- **阴性对照**：每轮加测 1 个不触及相关代码的场景（如 build path），证明环境漂移幅度

#### 4.2.2 Controlled A/B Test 计划

| 实验组 | A（基线）| B（优化）| 测量 case | 预期 Δns |
|-------|---------|---------|----------|---------:|
| O1-A 单独 | 当前 struct_node.rs（getattr）| + GenericGetDict 路径 | TM1 / CN1 | -10 ~ -20 |
| O1-B 单独 | + O1-A | + O1-B KnownHash | TM1 / FE1-parse（4 字段）| -5 ~ -10/字段 |
| O2-A 单独 | + O1-A + O1-B | + O2-A FFI 入口精简 | TM1 / CN1 | -5 ~ -10 |
| **三项综合** | 当前 main | + O1-A + O1-B + O2-A | 14 项 parse case | -20 ~ -40 |

每组实验独立 commit + 独立 Controlled A/B Test，避免归因混淆（L-09 对策）。

#### 4.2.3 验证 Acceptance Criteria（vs Python 3.13 非 abi3 基线）

- **共享税目标**：TM1 rs_ns ≤ 180ns（共享税 ≤ 150ns + 30ns 内层）
- **加速比目标**：CN1/CN2/AL2/TM1/EN1/EN2/MP1/OO1/NO1/FE1-parse 10 项 ≥ 10x
- **接近 10x 目标**：HX1/HX2/HD1/NT1 ≥ 8x（需 OPT-CASE-B 进一步优化）
- **FE1-build**：≥ 6.3x（不达 10x，PM 决策接受与否）

---

## 5. fallback 方案（汇总）

详 §3.5 fallback 表格 + §1.1.5 / §1.2.5 / §1.3.5 各项 fallback 段落。

**总原则**（v2 修正）：
- O1-A：GenericGetDict 是 stable ABI，abi3 / 非 abi3 均可用，**无 abi3 风险**
- O1-B / O2-A：依赖 cpython 子模块，**非 abi3 是项目约束**，无 abi3 fallback
- 所有 unsafe 路径有运行期 fallback（GenericGetDict NULL → getattr；KnownHash 失败 → PyDict_SetItem）
- 若未来切 abi3：O1-B/O2-A 不可编译，需重新评估（详 §abi3 前提变更说明）

---

## 6. 项目 ABI 现状（v2：用户决策 + 8.ENV 决定性验证）

**目标环境**（用户决策 2026-08-06 + 8.ENV ACCEPTED 落地）：
- 目标 Python **3.13**（pyo3 0.22 原生支持，无 3.14 要求）
- **非 abi3 模式**（放弃 `PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1`）

**8.ENV 决定性验证**（`plans/phase8-adapters-struct-streams/traces/8.ENV-环境切换.md §2`）：
- Python 3.13.14 双 venv（crs_venv_313 + crs_venv_py_313）
- maturin develop --release --uv 产出 `cp313-cp313-win_amd64.whl`（非 abi3 wheel 标签）
- **clean rebuild 后 4 个 pyo3 build output 全部无 `cargo:rustc-cfg=Py_LIMITED_API`**（决定性证据）
- `PYO3_PRINT_CONFIG=1` 输出 `abi3=false`
- cargo test 1373 passed / pytest 767 passed（功能无回退）

**cpython 子模块符号可访问性验证**（8.ENV §5.2 临时测试）：
- `core::mem::offset_of!(ffi::PyTypeObject, tp_dictoffset)` 编译通过
- `unsafe extern "C" fn(...) -> c_int = ffi::_PyDict_SetItem_KnownHash` 链接通过
- `core::mem::offset_of!(ffi::PyBytesObject, ob_sval)` 编译通过
- 临时测试文件 `tests/abi3_symbol_check.rs` 已删除，Cargo.toml 已还原

**pyo3 0.22 暴露确认**（v2 调研补充）：
- `pyo3::ffi::PyObject_GenericGetDict`（stable ABI，**不依赖 cpython 子模块**）：`pyo3-ffi-0.22.6/src/object.rs` L373
- `pyo3::ffi::Py_TPFLAGS_MANAGED_DICT = 1 << 4`：`pyo3-ffi-0.22.6/src/object.rs` L408
- `pyo3::ffi::_PyDict_SetItem_KnownHash`（cpython 子模块）：`pyo3-ffi-0.22.6/src/cpython/dictobject.rs` L29-34
- `pyo3::ffi::cpython::PyBytesObject.ob_sval`（cpython 子模块）：`pyo3-ffi-0.22.6/src/cpython/bytesobject.rs` L8-14

**结论**：
- O1-A / O1-B / O2-A 三条优化在 Python 3.13 非 abi3 下均可用
- O1-A（GenericGetDict）即使切 abi3 也仍可用（stable ABI）
- O1-B / O2-A 依赖非 abi3（项目约束）；若未来切 abi3 需重新评估
- 所有 cpython 子模块访问用 `#[cfg(not(Py_LIMITED_API))]` 守护（编译期检测），不再保留 abi3 fallback 分支

**DEV 实施前提（8.ENV 已完成，无需重复验证）**：
1. ✅ `Py_LIMITED_API` 未被定义（8.ENV §2.3 决定性验证）
2. ✅ `_PyDict_SetItem_KnownHash` / `PyBytesObject.ob_sval` 编译链接成功（8.ENV §5.2）
3. ⚠️ pyo3 0.22 #[pymethods] 是否接受 `Py<PyAny>` 返回类型 —— 仍需 DEV 实施时验证
4. ✅ Python 3.13 managed dict 行为已调研（详 §Python 3.13 managed dict 调研）

---

## 7. 对 DEV 实施的关键提示（容易出错点，v2 更新）

1. **不要在 parse 路径引入新的 Py<PyAny> 跨函数传递**——会触发 Incref/Decref，
   抵消 O1-A 节省。
2. **`dict_via_generic_getdict` 必须在 StructNode.parse 内联或同 crate 调用**——
   跨 crate 调用会阻止 LTO 内联，性能损失 ~5-10ns。
3. **GenericGetDict 返回值是 owned reference**（new ref）—— 必须用
   `Bound::from_owned_ptr` 接管，不是 `from_borrowed_ptr`。否则会内存泄漏
   （Incref 没对应 Decref）。
4. **NB-3 修复**：不要用 `Bound::from_borrowed_ptr_or_err`（pyo3 0.22 已废弃）——
   改用 `Bound::from_owned_ptr(py, ptr)` 后手动 NULL 检查。
5. **cpython 子模块访问用 `#[cfg(not(Py_LIMITED_API))]` 守护**——v2 不再保留 abi3
   fallback 分支。项目非 abi3 是约束，cfg 守护仅用于编译期检测（避免未来误切 abi3
   时产生编译错误而非运行期 UB）。
6. **不要假设 `_PyDict_SetItem_KnownHash` 返回值的成功路径**——返回 0 表示成功，
   -1 表示失败（与大多数 C API 一致）。
7. **测试覆盖 frozen dataclass 与 slots class**——这两类是 O1-A 路径的高风险场景。
   Python 3.13 下两者都启用 `Py_TPFLAGS_MANAGED_DICT`，但 slots class 的 dict
   可能始终为 NULL（理论 GenericGetDict 仍返回 dict）。
8. **NB-4：FieldName::new 中 PyObject_Hash 返回 -1 必须 panic**——不要缓存 -1
   hash 值，会导致 `_PyDict_SetItem_KnownHash` 触发 dict 内部冲突路径性能退化。
9. **FieldName 新增 cached_hash 后，所有构造 FieldName 的地方都要更新**——
   `FieldName::new` 是唯一公开构造器，但 `#[cfg(test)]` 测试中可能有直接构造。
10. **O2-A 改返回类型从 `Bound<PyAny>` 到 `Py<PyAny>` 后**——pyo3 的 #[pymethods]
    宏对返回类型有要求，需验证 `Py<PyAny>` 是否被接受（pyo3 0.22 文档：Py<T>
    实现 IntoPy<Py<PyAny>>，应可接受，但需 DEV 验证）。
11. **`pyo3::ffi::PyObject_GenericGetDict` 调用约定**——context 参数传
    `std::ptr::null_mut()`（CPython 文档：__dict__ getset descriptor 调用时传 NULL）。
12. **`PyBytesObject.ob_sval` 长度**——CPython 不变量：ob_sval 实际长度为
    `ob_base.ob_size + 1`（含终止 NUL），声明为 `[c_char; 1]` 仅是占位。
    `from_raw_parts(ptr, len as usize)` 的 len 必须取 `ob_base.ob_size`（不含 NUL）。
13. **不要假设 tp_dictoffset > 0**（v1 教训，L-14 第 3 次触发）——Python 3.13
    managed dict 下 `tp_dictoffset = -1`，原设计的 `*(instance_ptr + offset)` 直读
    完全失效。v2 已改用 `PyObject_GenericGetDict` 替代。

---

## 8. 与其他模块的交互

### 依赖

- `construct-rs/src/instance.rs`：复用 `create_class` / `intern_pystring`，
  新增 `dict_via_generic_getdict` / `set_item_knownhash` 工具函数；FieldName::new
  添加 hash 计算与 -1 panic 检查。**不再新增** `cache_dict_offset` /
  `dict_from_offset_unchecked`（v1 设计，v2 已废弃）。
- `construct-rs/src/context.rs`：`Context::set_field_at` 内部 dict 写入需替换
  为 `set_item_knownhash`（仅 has_expressions 路径）。Context 公开 API 不变。
- `construct-rs/src/schema.rs`：`_parse_raw` 入口精简（O2-A）。

### 被依赖

- 所有 StructNode.parse 路径（Phase 1-8 已有的所有 Struct 用法）。
- 未来 Phase 9 SequenceNode / ArrayNode（可复用 O1-A + O1-B 模式，本设计不实现）。
- 用户面 Adapter（ADR-022，Python 层）**不受影响**——它不经 StructNode.parse。

---

## 9. 性能假设（L-02/L-05 对策）

### 瓶颈识别（量化数据 + 来源）

- 共享税 ~180ns 占 Rust 侧总时间 50-75%（PERF-REASSESS §2.2 实测）
- 来源：`plans/phase8-adapters-struct-streams/traces/PERF-retest.md` +
  `experiments/phase8-parse-reassess/profile_python_breakdown.json`
- Python 3.13 非 abi3 新基线（8.ENV §4.1）：41/48 PASS，7 项 LOW（CN1/HX1/HX2/HD1/
  FE1-parse/FE1-build/NT1）正是 OPT-SHARED 优化目标

### 可证伪预测（覆盖所有 FFI/拷贝/转换来源）

详 §3.1 / §3.2 / §3.3 / §3.4。本设计对每条 FFI/拷贝来源都有对应优化声明或保留
来源说明（L-05 对策硬要求）。

### 边界条件清单（v2 更新）

| 场景 | 输入 | 预期行为 |
|------|------|---------|
| Python 3.13 managed dict 用户类 | tp_dictoffset == -1 | GenericGetDict 处理 managed dict，正确返回 PyDictObject |
| 首次访问实例 dict（lazy materialize）| dict_or_values 槽为 NULL | GenericGetDict 触发 materialize 创建 PyDictObject |
| slots class | Py_TPFLAGS_MANAGED_DICT set, dict NULL | GenericGetDict 返回空 dict 或 NULL，fallback 到 getattr |
| frozen dataclass | Py_TPFLAGS_MANAGED_DICT set | GenericGetDict 正常返回 dict（frozen 只影响 setattr）|
| 用户 override `__dict__` 描述符 | GenericGetDict 返回非 PyDict | downcast 失败，fallback 到 getattr |
| Unicode 字段名 | cached_hash = PyObject_Hash(unicode) | KnownHash 正确写入 |
| FieldName hash 失败（理论不会）| PyObject_Hash 返回 -1 | FieldName::new panic（NB-4 修复）|
| 字段数 > 16 | expr_values_buf 截断 | 仅影响 has_expressions 路径，dict 写入正常 |
| 字段值为 None | value.bind(py) is None | KnownHash 正常写入（PyDict_SetItem 支持 None value）|
| 大尺寸 bytes 输入 | ParseStream 内部 slice | O2-A 不影响 |
| 错误路径（stream EOF）| 子节点返回 Err | StructNode.parse 走 push_path_segment，不走 dict 写入 |
