---
id: ADR-023
title: 共享税优化（GenericGetDict + KnownHash + pyo3 FFI 入口精简，Python 3.13 非 abi3）
status: accepted
phase: "8"
decides: "StructNode.parse 共享税优化三件套（目标 Python 3.13 非 abi3）：O1-A 用 pyo3::ffi::PyObject_GenericGetDict（CPython 公开 stable ABI，处理 managed dict / inline values）替代 getattr('__dict__') 包装路径，绕过 pyo3 MRO 描述符查找 + Bound 构造；O1-B 用 pyo3::ffi::_PyDict_SetItem_KnownHash（pyo3 0.22 cpython 子模块暴露，CPython 内部 API）跳过 interned key 的 hash 计算；O2-A 精简 _parse_raw pyo3 入口包装（直接访问 PyBytesObject.ob_sval 字段绕过 PyBytes_AsStringAndSize 调用）。O1-B/O2-A 依赖 cpython 子模块（非 abi3 模式，用户决策 + 8.ENV 验证），无 abi3 fallback——非 abi3 是项目约束。"
supersedes: []
superseded_by: ""
depends_on:
  - ADR-008
  - ADR-019
  - ADR-021
  - ADR-022
  - PERF-retest
  - phase8-parse-perf-reassessment
  - 8.ENV-环境切换
last_updated: 2026-08-06
---

# ADR-023: 共享税优化（GenericGetDict + KnownHash + pyo3 FFI 入口精简）

## Context

### 触发事件（L-14 第 2 次触发，2026-08-06）

`docs/analysis/phase8-parse-perf-reassessment.md` 实测发现：14 项 Phase 8 parse
case 的 Rust 侧实测时间（170-640ns）中，**~180ns 是共享框架税**——与具体 case
核心操作无关的固定开销。共享税占普通 case 的 Rust 侧总时间 **50-75%**。即使把
子节点内部开销优化到 0，HX1 仍要 ~180ns 共享税，加速比上限约 14x——其他 case
（如 CN1/EN1/AL2）则会被这 180ns 卡在 8-9x。

这 180ns 共享税的拆解（PERF-REASSESS §2.1）：

| 来源 | 估算 ns | 当前实现 |
|------|--------:|---------|
| `_parse_raw` pyo3 入口（参数解析 + GIL 包装 + return 包装）| ~30-50 | `#[pyo3(signature = (data))]` + `Bound<PyBytes>` |
| `create_class`（tp_new 调用）| ~30-50 | 已优化（ADR-019 同脉络）|
| `getattr("__dict__")` | ~15-30 | `instance.getattr(self.dict_attr_name)`（pyo3 包装）|
| 每 field `PyDict_SetItem` | ~30-50/字段 | pyo3 `Bound<PyDict>::set_item` |
| FFI 出口 + return 包装 | ~20-30 | pyo3 自动 |

### L-14 自检：共享税不是硬约束

PERF-REASSESS §2.3 对每个组成部分做 L-14 交叉验证，结论：每项都有 ≥2 条
§0/ADR 允许的替代路径（详 §3.1）。把它打包归入"显示对象固有税"是 L-14 触发的
典型模式：单一路径推断 + 标签惰性。

本 ADR 沉淀三条替代路径的工程化落地，目标是把共享税压到 ~140-160ns（节省
20-40ns），为 10 项 parse case 达到 10x+ 提供前置条件。

### 设计输入与不可越边界

- **设计输入**：`docs/analysis/phase8-parse-perf-reassessment.md §5.1`（三条优化方向）
- **§0 不可违反**：本 ADR 必须保持"parse 1 次 FFI / 直接构造 PyObject / 无中间
  表示层"。三条优化都不改变 FFI 边界数量——仅减少边界内 C API 调用开销。
- **已有 unsafe 脉络**：`ADR-019`（错误抛出 fast-path）+ `ADR-021`（Strings
  UTF-16/32 raw FFI）已为本项目建立"正常路径 unsafe"先例，本 ADR 沿用同套
  SAFETY 论证模板。
- **目标环境约束**（用户决策 + 8.ENV 验证）：
  - 目标 Python **3.13**（pyo3 0.22 原生支持，无 3.14 要求）
  - **非 abi3 模式**（放弃 `PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1`）
  - 8.ENV 决定性验证：clean rebuild 后全部 pyo3 build output 无
    `cargo:rustc-cfg=Py_LIMITED_API`，cpython 子模块可用
  - 8.ENV 实证：cargo test 1373 passed / pytest 767 passed，功能无回退
  - 8.ENV 临时测试验证：三符号（`tp_dictoffset` / `_PyDict_SetItem_KnownHash` /
    `ob_sval`）在非 abi3 下编译链接通过

## Decision

### 决策 1：O1-A 用 `PyObject_GenericGetDict` 替代 `getattr("__dict__")` 包装

> **关键修正（2026-08-06，L-14 第 3 次触发）**：原设计计划用
> `(*type_ptr).tp_dictoffset` 直读实例 `__dict__` 偏移。但 Python 3.13 引入
> managed dict（`Py_TPFLAGS_MANAGED_DICT`），实测**所有用户类**（StructMixin
> 基类 / dataclass / frozen dataclass / slots class）的 `tp_dictoffset = -1`——
> 原设计 `*(instance_ptr + tp_dictoffset)` 将读到越界内存（负偏移）。
> 详见本 ADR §"Python 3.13 managed dict 影响" + 设计文档 §managed dict 调研。
>
> 本 ADR 改用 CPython 公开 stable ABI `PyObject_GenericGetDict`，它由 CPython
> 内部正确处理 managed dict + inline values + lazy materialize。

**核心机制**：CPython heaptype 类（含 managed dict）的 `__dict__` 描述符最终走
`tp_getattro` → MRO → `object.__dict__` descriptor → `PyObject_GenericGetDict`。
当前 R4 路径 `instance.getattr(interned "__dict__")` 内部经过：
1. `PyObject_GetAttr` → `tp_getattro`（即 `subtype_getattro`）
2. MRO 遍历查找 `"__dict__"` 描述符（attr cache miss 时走 `_PyType_Lookup`）
3. 命中 `getset_descriptor` → 触发 `dict_getter`（`PyObject_GenericGetDict` 的包装）
4. pyo3 把返回的 `*mut PyObject` 包装为 `Bound<PyAny>` + Incref
5. 用户调 `downcast_into::<PyDict>()`（运行期 type check + cast）

实测开销 ~15-30ns（其中 MRO + 描述符 dispatch ~5-10ns，pyo3 包装 ~5-10ns，
downcast ~2-5ns）。

**直接调 `PyObject_GenericGetDict(instance, NULL)`**：跳过 MRO + 描述符 dispatch，
仍触发 lazy materialize（首次访问创建 PyDictObject，CPython 不变量），但省去
getattr 包装层。返回的是 dict 对象指针（owned，需正确处理 refcount）。

**ABI 稳定性声明**：
- `PyObject_GenericGetDict(PyObject *obj, void *context)` 是 **CPython 公开
  stable ABI**（`Include/cpython/object.h` + 自 Python 2.x 起存在，签名稳定）。
- pyo3 0.22 在 `pyo3-ffi-0.22.6/src/object.rs` L373 暴露
  `pub fn PyObject_GenericGetDict(arg1: *mut PyObject, arg2: *mut c_void) -> *mut PyObject`。
- **不依赖 cpython 子模块**（在 abi3 / 非 abi3 下均可用）—— 本决策不受 abi3 切换影响。
- **正确处理 managed dict**：CPython 内部 `_PyObject_GenericGetDictWithDict`
  会检查 `Py_TPFLAGS_MANAGED_DICT` flag，从 pre-header 读 dict_or_values；
  若 dict 未 materialize（dict_or_values 为 NULL 或指向 inline values），
  会触发 materialize 创建 PyDictObject 并写入 pre-header。

**退化路径**：
- 若 `PyObject_GenericGetDict` 返回 NULL（理论不发生，heaptype 实例必有 dict）：
  返回 `ConstructError::Generic`，错误路径不优化。
- 若返回的不是 `PyDictObject`（理论不发生，除非用户类显式 override `__dict__`
  描述符）：downcast 失败，fallback 到当前 R4 getattr 路径。
- slots class（`Py_TPFLAGS_MANAGED_DICT` 仍 set，但 dict 始终 NULL）：返回错误信息
  保留 R4 兼容性（与 getattr 错误信息一致）。

**节省（量级参考，L-09）**：~10-20ns（消除 pyo3 getattr 包装：MRO + 描述符 dispatch
~5-10ns + Bound 构造 + Incref + Downcast ~5-10ns）。**不消除 materialize 开销**
（首次访问必须创建 PyDictObject，~20-30ns，但这是任何路径都需付出的）。
绝对值需 Controlled A/B Test 复测验证（详 §测试策略）。

### 决策 2：O1-B 用 `_PyDict_SetItem_KnownHash` 跳过 hash 重算

**核心机制**：field name 是 interned PyString（FieldName 缓存，instance.rs
`intern_pystring`），CPython 在 PyUnicodeObject 内部缓存了 hash 值。当前
`PyDict_SetItem` 内部仍要调 `PyObject_Hash(key)`（对 interned 仅取缓存字段，
~1-2ns）+ 进入 `insertdict` 写入。CPython 内部 API `_PyDict_SetItem_KnownHash`
（私有，但在 shared library 中导出）允许调用方直接传入已知 hash，跳过
`PyObject_Hash` 调用与一次类型检查分支。

**ABI 稳定性声明**：
- `_PyDict_SetItem_KnownHash` 是 **CPython 内部 API**（非 stable ABI、非 limited
  API），签名自 Python 3.5 起未变化（`int _PyDict_SetItem_KnownHash(PyObject *mp,
  PyObject *key, PyObject *value, Py_hash_t hash)`），3.7-3.14 实测可用。
- **pyo3 0.22 已暴露**此符号（`pyo3-ffi-0.22.6/src/cpython/dictobject.rs` L29-34
  `pub fn _PyDict_SetItem_KnownHash(...)`，通过 lib.rs `pub use self::cpython::*`
  re-export 为 `pyo3::ffi::_PyDict_SetItem_KnownHash`）。
- 该符号的 cpython 子模块由 `#[cfg(not(Py_LIMITED_API))]` 守护——仅非 abi3 模式
  下可用（项目约束，详 §Context 目标环境约束）。
- **风险**：未来 CPython 版本若改名 / 改签名，pyo3 cpython 子模块编译失败（编译期
  发现，非运行期 UB）。

**Hash 缓存的正确性保证**：
- field name 在 `StructNode::new` 时由 `intern_pystring` 创建，hash 在首次
  `PyObject_Hash` 后写入 PyUnicodeObject 的 hash 字段并 cache。
- StructNode 编译期一次性 `unsafe { ffi::PyObject_Hash(name.as_ptr()) }` 提取
  hash 存入 `FieldName`（与 `py_name` 并列）。`PyObject_Hash` 返回 -1 表示失败
  时，FieldName::new 必须立即 panic（构造期错误，不缓存 -1，避免运行期 dict 内部
  hash 冲突路径性能退化——NB-4 修复）。
- **不变量**：interned PyString 的 hash 在其生命周期内不可变（CPython 强约束：
  interned 字符串不可变 + hash 字段 once-cached）。只要 StructNode 存活，hash
  正确性保证。

**与 managed dict 的兼容性**：parse 路径用 `PyObject_GenericGetDict` 获取 dict
后，dict 已 materialize 为 PyDictObject（CPython 不变量），传给
`_PyDict_SetItem_KnownHash` 安全。inline values 在 `_PyDict_SetItem_KnownHash`
之前已被 GenericGetDict 强制 materialize。

**退化路径**：
- 运行期 dict 不为 `PyDictObject`（如用户继承 dict 的 Container）→ 走公开
  `PyDict_SetItem`（已有路径）。
- **无 abi3 fallback**（项目约束非 abi3；若未来切 abi3，编译期 cpython 子模块缺失，
  需重新评估 O1-B/O2-A 整体可行性——见 §Consequences 负面）。

**节省（量级参考，L-09）**：~5-10ns/字段（实测需验证：hash 缓存命中后 `PyDict_SetItem`
内部的 PyObject_Hash 已经接近免费，主要节省来自函数 dispatch 与一次条件分支）。
PERF-REASSESS §5.1.1 预测的 ~10-15ns 偏乐观，本 ADR 修正为 ~5-10ns。

### 决策 3：O2-A 精简 `_parse_raw` pyo3 FFI 入口包装

**核心机制**：pyo3 0.22 的 `#[pymethods]` 入口在每次调用时执行：
- Python `*args` 解包 + 类型检查（PyBytes 检查 ~3-5ns）
- `Bound<'_, PyBytes>` 智能指针构造（Incref + PyRef 绑定 ~2-3ns）
- 返回值 `Bound<PyAny>` → PyResult 包装（~2-3ns）
- 错误处理 `ConstructError::into()` 转 PyErr（成功路径 ~1-2ns）

剩余可挤的开销（pyo3 0.22 提供的优化已用尽）：
1. **`data.as_bytes()` 内部仍有指针范围验证**——pyo3 `Bound<PyBytes>::as_bytes`
   内部走 `PyBytes_AsStringAndSize`（含一次 type check + size check，~3-5ns）。
   可改用直接访问 `pyo3::ffi::cpython::PyBytesObject.ob_sval` 字段（pyo3 0.22
   `pyo3-ffi-0.22.6/src/cpython/bytesobject.rs` L8-14，pyo3 已暴露此 struct，
   由 `#[cfg(not(any(PyPy, GraalPy, Py_LIMITED_API)))]` 守护）。直接字段访问
   无 type check / 函数调用 dispatch，~1-2ns。节省 ~2-3ns。
2. **返回 `Bound<PyAny>`** 改为返回 `Py<PyAny>`（unbind 提早）—— 避免 pyo3
   在 wrap 阶段的一次 Bound 构造，节省 ~1-2ns。**DEV 实施前必须验证 pyo3 0.22
   #[pymethods] 是否接受 `Py<PyAny>` 返回类型**（设计文档 §7 #3）。

**ABI 稳定性声明**：
- `PyBytesObject` struct + `ob_sval` 字段属 **CPython internal struct**（在
  `cpython/bytesobject.h` 中定义）。**不在 stable ABI / limited API 范围**。
- pyo3 0.22 在 cpython 子模块（`#[cfg(not(Py_LIMITED_API))]`）暴露此 struct。
- **当前项目为非 abi3 模式**（用户决策 + 8.ENV 验证），cpython 子模块可用。

**节省（量级参考，L-09）**：~5-10ns（两处微优化叠加）。PERF-REASSESS §5.1.2
预测的 ~10-20ns 偏乐观，本 ADR 修正为 ~5-10ns。

### 决策 4：unsafe 边界（统一规范）

所有 unsafe 操作必须满足以下规范（REV 严格审查项）：

1. **SAFETY 注释模板**：每个 unsafe 块附 SAFETY 注释，逐条说明：
   - 指针来源（从哪个 pyo3 守护的方法获取 / GIL 是否持有）
   - 生命周期保证（borrowed / owned / Py<'py> 边界）
   - 别名规则（Python refcount + GIL 单线程，无 &mut 别名）
2. **CPython ABI 依赖声明**：注释中标注依赖的 CPython 版本/API 类别：
   - `[CPython Public Stable ABI]`（如 `PyObject_GenericGetDict`）
   - `[CPython Internal Struct]`（如 `PyBytesObject.ob_sval`）
   - `[CPython Internal Function]`（如 `_PyDict_SetItem_KnownHash`）
3. **不变量（invariants）**：列出操作有效的条件 + 违反时 UB 风险。
4. **退化路径**：运行期 feature detection / fallback to safe path。

**abi3 处理策略**（重要变更）：
- **本 ADR 不提供 abi3 fallback**——非 abi3 是项目约束（用户决策 + 8.ENV 落地）。
- 若未来项目切 abi3：
  - O1-A 仍可用（`PyObject_GenericGetDict` 是 stable ABI）
  - O1-B / O2-A 编译失败（cpython 子模块缺失）→ 需重新评估整体方案
- 设计文档 §abi3 前提变更说明记录此变更历程。

详细 SAFETY 论证见设计文档 `docs/design/模块设计/模块设计-Phase8-OPT-SHARED.md §1`。

## Python 3.13 managed dict 影响（L-14 第 3 次触发核心调研）

### 实证结论（2026-08-06）

通过 Python 3.13 venv（crs_venv_313）实测用户类布局（详设计文档 §managed dict 调研）：

| 类 | `Py_TPFLAGS_MANAGED_DICT` | `Py_TPFLAGS_INLINE_VALUES` | `tp_dictoffset` |
|----|:---:|:---:|:---:|
| StructMixin（基类）| ✓ | ✓ | **-1** |
| `@dataclass class X(StructMixin)` | ✓ | ✓ | **-1** |
| frozen dataclass | ✓ | ✓ | **-1** |
| slots class | ✓ | ✗ | **-1** |
| `int` / `str` / `object`（C 类型）| ✗ | ✗ | 0 |

**关键事实**：
1. **Python 3.13 下所有用户类（heaptype）默认启用 managed dict + inline values**，
   `tp_dictoffset = -1`（CPython 不变量：managed dict 模式下强制 -1，详见
   `docs.python.org/3/c-api/typeobj.html` `tp_dictoffset` 节）。
2. **原 O1-A 设计（`*(instance_ptr + tp_dictoffset)` 直读）完全失效**：负偏移
   读越界，运行期 UB（崩溃或读到错误内存）。
3. **CPython pre-header layout（实测）**：`instance_ptr - 24` 是 dict_or_values
   槽（64-bit 系统）；`object.__new__` 后该槽为 NULL（lazy），首次访问
   `__dict__` 触发 materialize 创建 PyDictObject。
4. **`PyObject_GenericGetDict` 是 CPython 公开 stable ABI**，内部正确处理
   managed dict + inline values + lazy materialize——是 O1-A 的正确替代路径。

### L-14 自检（用户指令"不要把 managed dict 不可处理当硬约束"）

- ✅ 交叉验证 1（替代路径）：枚举 4 条路径——tp_dictoffset 直读（失效）/
  PyObject_GenericGetDict（可用）/ _PyObject_GetDictPtr（不 materialize，不适用）/
  pre-header 直读（unsafe 复杂，需处理 tag bit + materialize）。选
  PyObject_GenericGetDict（最简洁 + stable ABI）。
- ✅ 交叉验证 2（CPython 文档）：typeobj.md 明确推荐
  "To get the pointer to the dictionary call PyObject_GenericGetDict()"。
- ✅ 交叉验证 3（pyo3 暴露）：`pyo3::ffi::PyObject_GenericGetDict` 0.22.6 暴露。
- ✅ 交叉验证 4（实测）：Python 3.13 venv 跑用户类，确认 managed dict 行为。

不把"managed dict 不可处理"当硬约束——已找到 CPython 提供的兼容访问方式
（`PyObject_GenericGetDict`）。

## Consequences

### 正面

- **共享税压缩 ~20-40ns**（O1-A 10-20ns + O1-B 5-10ns/字段 × 1-2 字段 + O2-A
  5-10ns），把 14 项 parse case 的共享税从 ~180ns 压到 ~140-160ns。
- **10 项 parse case 解锁 10x+**（CN1/CN2/AL2/TM1/EN1/EN2/MP1/OO1/NO1/FE1-parse，
   详 PERF-REASSESS §4.2 + 本 ADR §预测）。
- **沿用 ADR-019/ADR-021 unsafe 脉络**：本项目已有"正常路径 unsafe"先例
  （错误抛出 fast-path、UTF-16/32 raw FFI），本 ADR 扩展到 dict/PyBytes 操作。
- **O1-A 不依赖 cpython 子模块**（PyObject_GenericGetDict 是 stable ABI）——
  即使未来切 abi3，O1-A 仍可用（仅 O1-B/O2-A 受影响）。
- **机制可复用**：GenericGetDict + KnownHash 模式可推广到 SequenceNode /
  ArrayNode 的同类共享路径（Phase 9+）。

### 负面

- **CPython 内部 API 依赖（O1-B / O2-A）**：`_PyDict_SetItem_KnownHash` /
  `PyBytesObject.ob_sval` 是私有 API/internal struct，未来 CPython 版本可能改名/
  改签名。pyo3 cpython 子模块已暴露此符号，若 CPython 删除则 pyo3 升级时会编译
  失败（编译期发现）。
- **非 abi3 模式锁定（O1-B / O2-A）**：两条优化依赖 `pyo3::ffi::cpython` 子模块
  （`#[cfg(not(Py_LIMITED_API))]`）。若项目未来切 abi3（如要求支持 Python 3.14+），
  O1-B/O2-A 编译失败需重新评估。**O1-A 仍可用**（PyObject_GenericGetDict 是
  stable ABI，不依赖 cpython 子模块）。
- **维护成本**：unsafe 代码增加 ~100-150 行（StructNode + instance.rs + 字段
  缓存逻辑），需在每次 CPython / pyo3 升级时复核 SAFETY 论证。
- **量级参考风险（L-09）**：所有节省估算均为 ns 量级，受测量噪声影响。需 VET
  Controlled A/B Test 独立复测验证（不可只信 ARCH 理论推算）。
- **Python 3.13 managed dict 隐性约束**：O1-A 不消除 materialize 开销（任何路径
  都需 ~20-30ns 首次 materialize）。节省上限受此约束。

### 中性

- **O1-A 退化路径简化**：不再有 abi3 cfg 分支（PyObject_GenericGetDict 在 abi3 /
  非 abi3 均可用），代码复杂度降低。
- **managed dict materialize 行为依赖 CPython 不变量**：未来 CPython 若改变
  lazy materialize 策略（如立即 materialize / 不 materialize），需重新评估 O1-A
  节省量。

## §0 合规确认

| §0 原则 | 本 ADR 合规论证 |
|---------|---------------|
| **#1 一次 FFI** | ✅ 三条优化都不改变 FFI 边界数量。`_parse_raw` 仍是唯一 FFI 入口（Python→Rust 一次）。优化的是 Rust 内部 CPython C API 调用（`PyObject_GenericGetDict` / `_PyDict_SetItem_KnownHash` / `PyBytesObject.ob_sval` 字段访问），不增加 Python↔Rust 穿越次数。 |
| **#2 无中间表示层** | ✅ parse 仍直接构造 PyObject（用户类实例）。`PyObject_GenericGetDict` 返回的是**实例 `__dict__` 这个 Python 对象本身**（PyDictObject），不是 Rust 中间数据类型——与 R4 设计（ADR-008）语义一致。KnownHash 优化的是 `PyDict_SetItem` 调用，dict 仍是 Python 对象。 |
| **#3 输入输出无 trait 抽象** | ✅ 不引入新 trait，不引入 per-field 抽象层。优化发生在已有 StructNode / CompiledSchema 内部。 |
| **#4 pyo3 核心依赖** | ✅ 仍用 pyo3 + pyo3::ffi。O1-A 用 pyo3::ffi::PyObject_GenericGetDict（stable ABI），O1-B/O2-A 通过 pyo3::ffi::cpython 子模块访问 CPython internal struct/function（pyo3 0.22 已暴露），不引入第二个 Python 绑定层。 |
| **#5 mashumaro 式 API** | ✅ 用户面 API 不变（`@dataclass class X(StructMixin)`），优化完全在 Rust 内部。 |
| **#6 enum_dispatch 静态分派** | ✅ StructNode 仍是 Node enum 变体，分派路径不变。新增内部字段不影响 enum_dispatch。 |
| **#7 Result<T, ConstructError> + thiserror** | ✅ 错误处理不变。退化路径（GenericGetDict 返回 NULL）仍返回 `ConstructError::Generic`，错误路径不优化。 |
| **#8 Stream 抽象纯 Rust** | ✅ Stream 抽象不涉及。 |

### ADR 兼容性

| ADR | 关系 |
|-----|------|
| ADR-008（parse 借用实例 dict）| **本 ADR 是 ADR-008 的性能精化**：R4 借用 dict 的方式从 `getattr` 升级为 `PyObject_GenericGetDict`，语义不变（仍是借用实例 `__dict__`）。 |
| ADR-019（错误抛出 fast-path，unsafe raw FFI）| **同脉络**：本项目"正常路径 unsafe"先例。本 ADR 沿用同一 SAFETY 论证模板 + ABI 依赖声明规范。 |
| ADR-021（Strings UTF-16/32 raw FFI）| **同脉络**：扩展 unsafe raw FFI 到 dict / PyBytes 操作。 |
| ADR-022（用户面 Adapter Python 层化）| **正交**：本 ADR 优化内置 Node 共享路径，用户面 Adapter 仍是 Python 层（不受影响）。 |

## 适用范围

- **适用**：所有内置 Node 的 StructNode.parse 共享路径（14 项 Phase 8 parse case
  + 未来 Phase 9 Sequence/Array 共享路径）。
- **不适用**：
  - 用户面 Adapter（ADR-022，仍是 Python 层 + 2 次 FFI）
  - 非 dict 类型的实例（理论不会发生，heaptype 必有 managed dict；slots class
    会返回错误）—— 走退化路径
  - build 路径（本 ADR 仅覆盖 parse；build 已用 `static_size` 预分配优化）

## 禁止行为

- ❌ 跳过 SAFETY 注释模板直接写 unsafe 块（违反 ADR-019/ADR-021 SAFETY 规范）
- ❌ 假设 `tp_dictoffset > 0` 可作为有效偏移直接读实例 dict（Python 3.13 下
  managed dict 模式 `tp_dictoffset == -1`，直读越界——L-14 第 3 次触发教训）
- ❌ 缓存 `PyObject_Hash` 返回 -1 的值（构造期必须 panic，避免运行期 dict 内部
  hash 冲突路径退化——NB-4 修复）
- ❌ 假设 `Py_tp_dictoffset` 是 PyType_GetSlot 标准插槽（pyo3 0.22 typeslots.rs
  不暴露此常量；`tp_dictoffset` 是字段不是插槽）
- ❌ 在 parse 路径引入新的 Python→Rust FFI 边界（违反 §0 #1）
- ❌ 引入 Rust 中间数据类型再转回 PyObject（违反 §0 #2，L-01 触发条件）

## Alternatives Considered

### 替代 1：完全不做 unsafe，仅做 pyo3 优化

**描述**：只用 pyo3 0.22 公开 API（不调 `_PyDict_SetItem_KnownHash`、不读
`PyBytesObject.ob_sval` 字段），仅依赖 stable ABI 函数。O1-A 用
`PyObject_GenericGetDict`（stable ABI）。

**为何不采用**：
- O1-A 单独节省 ~10-20ns，无法把共享税压到 140-160ns 目标。
- 无法解锁 10 项 parse case 的 10x+（PERF-REASSESS §4.2 + 本 ADR §预测）。
- 不利用本项目已建立的 unsafe 基础设施（ADR-019/ADR-021）。

### 替代 2：完全用 Rust HashMap 替代 PyDict

**描述**：parse 时构造 Rust `HashMap<String, Py<PyAny>>`，最后一次性
`PyObject_GenericSetAttr` 写入实例。

**为何不采用**：
- 违反 §0 #2（引入中间表示层 Rust HashMap + 字段值在 Rust/Python 间倒腾）。
- L-01 触发条件：每字段跨抽象层，性能反退化（参考 Phase 15 历史 0.08x 教训）。
- ADR-008（R4 借用实例 dict）就是从这条路改回去的。

### 替代 3：用 `_PyDict_NewPresized` 预分配 dict

**描述**：CPython 内部 API `_PyDict_NewPresized(n)` 预分配 n 个 slot 的 dict，
避免 set_item 时的 resize。

**为何不采用**：
- 当前 R4（ADR-008）借用**实例自带的 dict**（由 tp_new + managed dict materialize
  创建），不新建 dict。`_PyDict_NewPresized` 仅在新建场景有意义。
- 改回新建 dict 会重新引入 R3 → R4 修复的 O(N²) 开销。
- `_PyDict_NewPresized` 也是私有 API，ABI 风险等同 O1-B 但收益更小（dict 不 resize
  时节省 ~0ns）。

### 替代 4：直接读 pre-header dict_or_values 槽（绕过 GenericGetDict）

**描述**：unsafe 读 `instance_ptr - 24`（dict_or_values 槽），处理 tag bit
（low bit 0=dict / 1=values）+ NULL 情况 + manual materialize。

**为何不采用**：
- pre-header 偏移依赖具体 Python 版本（3.12 vs 3.13 不同），ABI 不稳定。
- materialize 逻辑（inline values → PyDictObject）复杂，需重新实现 CPython 内部
  `_PyObject_MaterializeDict` 逻辑，维护成本高。
- `PyObject_GenericGetDict` 已封装这些细节（stable ABI），节省 ~2-5ns 不值得
  增加复杂度。
- 违反"不引入 Rust 中间数据类型"精神（虽不严格违反 §0，但徒增 unsafe 边界）。

## 预测（vs Python 3.13 非 abi3 新基线，8.ENV §4.1）

> **基线来源**：`plans/phase8-adapters-struct-streams/traces/8.ENV-环境切换.md §4.1`
> （Python 3.13.14 非 abi3，crs_venv_313 vs crs_venv_py_313，number=5000, iter=10）。
> 旧 3.14+abi3 基线已 deprecated（L-09 不作跨版本对比）。

| Case | 3.13 基线 rs_ns | 3.13 基线 sp | OPT-SHARED 预测 rs_ns | 预测 sp | 状态 |
|------|---------------:|------------:|---------------------:|--------:|------|
| CN1 | 214 | 9.71 | ~175-190 | ~10.9-11.9x | LOW→OK |
| CN2 | 206 | 10.35 | ~165-180 | ~11.9-12.9x | OK |
| HX1 | 370 | 7.19 | ~325-345 | ~7.7-8.2x | LOW（需 OPT-CASE-B）|
| HX2 | 332 | 8.08 | ~285-305 | ~8.8-9.4x | LOW（需 OPT-CASE-B）|
| HD1 | 327 | 7.72 | ~280-300 | ~8.4-9.0x | LOW（需 OPT-CASE-B）|
| AL2 | 249 | 10.10 | ~205-220 | ~11.4-12.2x | OK |
| TM1 | 237 | 10.28 | ~165-180 | ~13.5-14.8x | OK |
| EN1 | 222 | 10.40 | ~180-195 | ~11.8-12.8x | OK |
| EN2 | 226 | 10.09 | ~180-195 | ~11.7-12.7x | OK |
| FE1-parse | 339 | 9.99 | ~295-310 | ~10.9-11.5x | LOW→OK |
| FE1-build | 439 | 5.94 | ~395-415 | ~6.3-6.6x | 仍 LOW（PM 决策）|
| MP1 | 231 | 10.88 | ~190-205 | ~12.2-13.2x | OK |
| OO1 | 224 | 11.29 | ~180-195 | ~12.9-14.0x | OK |
| NO1 | 229 | 10.29 | ~185-200 | ~11.8-12.7x | OK |
| NT1 | 602 | 7.69 | ~560-580 | ~8.0-8.3x | LOW（需 OPT-CASE-B）|

**总判定**：
- **OPT-SHARED 后达标 10x+（10 项）**：CN1/CN2/AL2/TM1/EN1/EN2/FE1-parse/MP1/OO1/NO1
- **接近 10x（4 项）**：HX1/HX2/HD1/NT1（需 8.OPT-CASE-B 进一步优化子节点）
- **仍 <10x（1 项）**：FE1-build（PM 单独决策）

**可证伪判据**：TM1 共享税优化后 rs_ns > 200ns → O1-A/O1-B/O2-A 综合失败。

## Relations

- **关联 ADR**：ADR-008（parse 借用实例 dict，本 ADR 是其性能精化）+ ADR-019
  （unsafe raw FFI 规范）+ ADR-021（unsafe 正常路径扩展）
- **关联教训**：`harness/experiences.md §L-01`（中间表示层，本 ADR §0 #2 合规）
  + `§L-05`（优化 A 路径忽略 B 路径，本 ADR §测试 FFI 来源清单对策）+ `§L-09`
  （ns 量级参考，本 ADR 所有节省估算标注量级参考）+ `§L-14`（硬约束交叉验证，
  本 ADR 触发第 3 次：abi3 单一路径推断 + managed dict 直读失效）
- **设计文档**：`docs/design/模块设计/模块设计-Phase8-OPT-SHARED.md`（详细设计 +
  SAFETY 论证 + 测试策略 + managed dict 调研）
- **设计输入**：`docs/analysis/phase8-parse-perf-reassessment.md §5.1`（三条优化方向）
- **环境前提**：`plans/phase8-adapters-struct-streams/traces/8.ENV-环境切换.md`
  （非 abi3 验证 + 3.13 新基线 + cpython 符号可访问性验证）
- **REV 驳回 + 回应**：`plans/phase8-adapters-struct-streams/traces/8.OPT-SHARED-REV检视.md`
  （abi3 问题已由 8.ENV 解决，本 ADR 是更新版）
- **实现位置**：
  - `construct-rs/src/instance.rs`（新增 `dict_via_generic_getdict` /
    `set_item_knownhash` 工具函数；FieldName::new 添加 hash 计算）
  - `construct-rs/src/nodes/struct_node.rs`（StructNode.parse 改 dict 获取路径；
    FieldName 新增 `cached_hash: Py_hash_t` 字段）
  - `construct-rs/src/schema.rs`（`_parse_raw` 入口精简，O2-A）
  - 注：`_PyDict_SetItem_KnownHash` / `PyBytesObject.ob_sval` 由 pyo3::ffi::cpython
    子模块暴露，不需手动 extern 声明；`PyObject_GenericGetDict` 由 pyo3::ffi 公开
    API 暴露
- **首次验证**：Phase 8 OPT-SHARED（待 PM 分派 DEV 实施）
- **未来复用**：SequenceNode / ArrayNode 的同类 dict 操作可复用 O1-A + O1-B 模式

### ADR 编号说明

本 ADR 占用编号 **023**。Phase 8 上一 ADR 为 ADR-022（用户面 Adapter Python 层化）。
本 ADR 与 ADR-022 正交（不同优化目标，不冲突）。

### 变更历史

| 日期 | 版本 | 变更 |
|------|------|------|
| 2026-08-06（初版）| v1 | 原设计：tp_dictoffset 直读 + KnownHash + ob_sval，含 abi3 fallback |
| 2026-08-06（REV 驳回）| - | REV 发现项目实际编译 abi3，三符号不可达，节省 = 0 |
| 2026-08-06（8.ENV 落地）| - | 用户决策放弃 abi3、目标 3.13；8.ENV 决定性验证非 abi3 + 三符号可编译 |
| 2026-08-06（v2 当前）| v2 | 去除 abi3 fallback；O1-A 改用 PyObject_GenericGetDict（Python 3.13 managed dict 使 tp_dictoffset = -1，原设计失效，L-14 第 3 次触发）；O1-B/O2-A 不变；预测 vs 3.13 新基线 |
