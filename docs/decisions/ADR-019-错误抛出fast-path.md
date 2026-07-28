---
id: ADR-019
title: 错误抛出 fast-path（绕过 Python __init__ 直接构造异常实例）
status: accepted
phase: cross  # 跨阶段模式（与 ADR-016/017/018 一致）
decides: "内置 Python 异常类的错误抛出走 fast-path（绕过 __init__），用户子类化异常走慢路径"
supersedes: []
superseded_by: ""
depends_on: [ADR-016]
last_updated: 2026-07-28
---

# ADR-019: 错误抛出 fast-path（绕过 Python `__init__` 直接构造异常实例）

## Context

错误抛出路径 `ConstructError → PyErr` 转换（`From<ConstructError> for PyErr` 实现）固定占用 ~1,200-1,500ns，是 Rust 侧错误路径的主要开销来源。

**数据来源**（`experiments/phase4_perf_investigation_report.md §3.3` INVEST 实测，量级参考非精确值）：

- a_err_eof（Array(100, Int8ub) + 50B stream EOF）parse：6.45x（远低于用户硬约束 #5 的 10x）
- p_err_overflow（PrefixedArray cf=300 build overflow）build：1.97x

**Rs 端开销分解**（INVEST §3.3）：

| 成分 | 估算 ns | 占比 |
|------|--------:|----:|
| 正常路径至错误点 | ~1,000-1,100 | ~45% |
| `ConstructError → PyErr` 转换 | **~1,200-1,500** | **~53%** |
| 合计 | ~2,300-2,500 | 100% |

错误抛出路径的 FFI / Python 调用来源（设计 `docs/design/模块设计-错误路径优化.md §1.2`，F1-F8 逐项分解）：

| # | FFI 来源 | 估算 ns | 备注 |
|---|---------|--------:|------|
| F1 | 字符串预格式化（`to_string()` × 2-3） | ~100-200 | 多次 Rust String 分配 |
| F5 | `Py<PyType>::clone`（Py_INCREF/DECREF 各一次） | ~10-20 | 每次错误都 incref/decref |
| F6 | args 元组构造 `cls.call1((message, p))` | ~50-100 | 每次新建 PyTuple |
| **F7** | **`tp_call → tp_new → tp_init` 执行 Python `__init__` 字节码** | **~600-900** | **核心瓶颈**（含 `format` + `super().__init__`） |
| F8 | `PyErr::from_value_bound` 包装 | ~50-100 | incref instance |

F7 的 `__init__` 字节码（`construct-rs/python/construct/_errors.py:46-53`）含 `"Error in path {}\n{}".format(path, message)`（~150ns）和 `super().__init__(full)`（~100ns）两项不必要工作——Python 侧 `__str__` 已重写为基于 `self.path` + `self.message` 格式化，`args` 内容不影响 `str(e)`。

## Decision

实施 4 级优化（O1-O4），**核心创新**为 O3 首次使用 raw CPython C API（`pyo3::ffi`）绕过 Python `__init__` 字节码。

### 优化项汇总

| 优化项 | 目标 FFI | 决策内容 | 实现位置 |
|--------|---------|---------|---------|
| O1 lazy 字符串 | F1 | 借用 `&err` 进入 `with_gil` 闭包，闭包内借用 `err.message()` / `err.path()` 返回的 `&str`，不调 `to_string()` 预格式化 | `error.rs::From::from` L859-875 |
| O2 避免 incref | F5 | `select_exception_class` 返回 `&Bound<'py, PyType>` 借用引用，不调 `clone()`（避免 `Py_INCREF`/`Py_DECREF` 各一次） | `error.rs::select_exception_class` L639-675 |
| **O3 绕过 `__init__`** | **F6+F7+F8 核心** | **`try_fast_path_alloc` 用 raw CPython C API 直接构造异常实例，跳过 `tp_init` 字节码执行** | `error.rs::try_fast_path_alloc` L740-825 |
| O4 PyErr 直接构造 | F8 | `PyErr::from_value_bound` 一步构造，无中间层 incref | `error.rs::try_fast_path_alloc` 末段 L822-823 |

### O3 fast-path 决策细节

**fast-path 流程**（设计 §1.3.1，对应 U1-U4 unsafe API）：

1. **U0 入口检查**：`is_builtin_class` 用裸指针相等比较（`*mut ffi::PyObject` 直接 `contains`，**非** `PyType_IsSubtype`）判定 `cls` 是否确切为 13 个内置异常类之一。用户子类化 → 返回 `None`，走慢路径。
2. **U1 分配实例**：`PyType_GenericAlloc(cls.as_type_ptr(), 0)`（CPython 固有 ~200ns，不可压缩）。
3. **U2 设置 `message` 属性**：`PyObject_SetAttrString(instance, "message", msg)`。
4. **U3 设置 `path` 属性**：`PyObject_SetAttrString(instance, "path", path_or_none)`（None 分支借用 `Py_None`，SetAttr 内部 incref）。
5. **U4 设置 `args` 字段**：`PyObject_SetAttrString(instance, "args", (message,))`（依赖 CPython `BaseException_setattro` 对 `args` 的特殊处理）。
6. **O4 包装**：`Bound::from_owned_ptr_or_opt` 消费 instance 指针（不 incref），所有权转移给 `PyErr`。

**子类化兼容性策略**（按 4.x-REV OBS-2 修订）：

- 仅对 `ExceptionClasses` 缓存的 13 个内置异常类（`StreamError` / `FormatFieldError` / `FieldLengthError` / `CompilationError` / `UnresolvedReferenceError` / `GenericConstructError` / `ConstructError` / `IntegerError` / `PaddingError` / `RangeError` / `RepeatError` / `StopFieldError` / `IndexFieldError`）启用 fast-path。
- **检测方法**：指针相等比较 `cls.as_ptr() == builtin_cls.as_ptr()`，确保是确切类型而非子类。
  - **不用** `PyType_IsSubtype(A, B)`：后者检查"A 是否 B 的子类"，任何子类都返回 true，不能用于"是否等于内置类"判定。
- 用户子类化的异常（如 `class MyError(StreamError): def __init__(...): ...`）自动降级走原有 `call1` 慢路径，保证用户的 `__init__` 重写被调用。

**BC6 三层降级**（`error.rs::From::from` L857-895）：

1. fast-path 失败（4 个 unsafe API 任一返回 `-1`/`NULL`，如极端的 monkey patch）→ `try_fast_path_alloc` 返回 `None`。
2. `build_exception_instance` 慢路径（`cls.call1((message, p))`，调 Python `__init__`）。
3. `PyValueError::new_err` 最终兜底（BC1，如异常实例构造失败）。

**行为差异**（设计 §3.1.3 决策 4 显式批准）：

- 慢路径：`args = (full,)`，其中 `full = "Error in path X\nY"` 或 `Y`。
- fast-path：`args = (message,)`（不含 path 前缀）。
- `str(e)` 行为一致：Python 侧 `__str__` 已重写为基于 `self.path` + `self.message` 格式化，不依赖 `args`。
- `repr(e)` / `pickle` 行为差异由本 ADR 显式批准（`pickle.dumps(e)` / `pickle.loads(...)` 往返不破坏，依赖 `args` + `message` + `path`）。

## 适用范围

所有 `ConstructError → PyErr` 转换路径（`From<ConstructError> for PyErr` 实现内部优化，对外接口不变）。

- 所有 Rust Node 的错误抛出路径透明受益（接口不变）。
- 错误路径不触发 §0 八条原则任何一条（设计 §1.5 对照表逐条核对）。
- O3 使用 `pyo3::ffi`（CPython C API）是 pyo3 公开稳定 API（§0 第 4 条 pyo3 是核心依赖，不违反）。

## 禁止行为

- ❌ 用户子类化的异常走 fast-path（必须用指针比较排除，走 `call1` 慢路径）
- ❌ fast-path 失败不回退到慢路径（必须实现 BC6 三层降级）
- ❌ 任何 `unsafe` 块无 SAFETY 注释（必须列出前置条件 + 失败模式 + BC6 回退路径）
- ❌ fast-path 失败分支不调用 `Py_DecRef(instance)` / `PyErr_Clear()`（避免引用泄漏 + Python 错误状态污染）
- ❌ 用 `PyType_IsSubtype` 替代指针相等比较做"确切类型"判定（语义不符，任何子类都返回 true）

## Consequences

### 性能（VET Controlled A/B Test 实测，非估算）

数据源：`plans/phase4-array/过程记录.md §4.x-VET`（VET 独立复测，2026-07-28 同会话 3 轮交替测量均值）。

| 场景 | 优化前 | 优化后（B 状态） | Δ (B-A) | 达标判据 |
|------|------:|----------------:|--------:|---------|
| a_err_eof parse | 6.45x（INVEST 基线） | **11.345 ± 0.472x** | **+1.53x**（A=9.815x，welch_t=4.34, p<0.05 显著） | ✅ ≥10x 用户硬约束 #5 |
| p_err_overflow build | 1.97x（INVEST 基线） | 2.716 ± 0.054x | +0.22x（speedup 在波动内，但 rs_ns 减少 308ns 真实） | ⚠️ 结构性边界，合流 Phase 5 |
| i02_idx_n100 parse（阴性对照） | — | 17.768 ± 0.424x | -0.29x（<0.3x 波动阈值） | ✅ 环境漂移幅度可接受 |

**关键结论**：

- **O1-O4 真实生效**：a_err_eof Δ=+1.53x >0.5x 显著阈值（Checkpoint 4 判据），DLL hash A/B 不同（A=45A51F16/517120B vs B=E616D98E/518656B，cargo 真重编译）。
- **错误抛出开销**：~1,200-1,500ns → ~480-580ns（节省 ~62%，设计 §1.3 优化方案汇总）。
- **实测落入设计预测区间**：设计 §1.4.3 预测保守 9.5x / 中位 10.5x / 上限 12x，实测 B=11.34x 落入"中位 - 上限"区间。
- **p_err_overflow 结构性边界**：Python baseline 仅 ~5.6µs（FocusedSeq+Rebuild 偷懒）+ Rs 正常路径 ~200ns（§0 一次 FFI 约束），即便错误抛出压到 0ns 也仅 ~28x 理论上限；O1-O4 后 ~2.7x 已达工程上限，进一步改善需 Phase 5（Struct + FFI 入口优化）。

### 首次 unsafe raw FFI 使用（SAFETY 论证规范）

O3 是项目首次在错误路径使用 `unsafe` raw CPython C API（`pyo3::ffi`）。建立以下规范（设计 §1.3.1，按 4.x-REV OBS-3 修订）：

- **每个 unsafe 块必须有完整 SAFETY 注释**：列出前置条件 + 失败模式 + BC6 回退路径。
- **U1-U4 四个 unsafe API 的安全契约**：

| # | unsafe API | 安全前置条件 | 失败模式 | BC6 回退 |
|---|-----------|-------------|---------|---------|
| U1 | `PyType_GenericAlloc` | GIL 持有 + cls 有效 + cls 确切内置类（U0 指针比较验证） | NULL + `PyErr_MemoryError`（仅 OOM） | `PyErr_Clear()` + 返回 None |
| U2 | `PyObject_SetAttrString "message"` | GIL 持有 + instance 是 U1 非空返回 + value 是有效 PyString | -1 + `PyErr_AttributeError`/`TypeError`（如 monkey patch `__setattr__`） | `PyErr_Clear()` + `Py_DecRef(instance)` + 返回 None |
| U3 | `PyObject_SetAttrString "path"` | 同 U2；value 为 PyString 或借用 Py_None | 同 U2 | 同 U2 |
| U4 | `PyObject_SetAttrString "args"` | GIL 持有 + instance 是 BaseException 子类 + tuple 非空 | -1（罕见） | 同 U2 |

- **引用计数追踪**：`msg_bound` refcount 在 4 个 SetAttr 调用中追踪（最终 instance 独占持有 message/path/args，无泄漏）。
- **VET 验证**：`error.rs::try_fast_path_alloc` L740-825 引用计数链路完整 ✅；BC6 三层降级覆盖 BC1/BC3/BC6 ✅。

### 子类化兼容性（指针比较 + 慢路径回退）

**T6 子类化兼容性独立验证**（VET 编写 `experiments/phase4_4x_t6_verify.py`，5 个场景 × 4 个断言 = 20 项 check，**20/20 PASS**）：

| 场景 | 关键断言 | 结果 |
|------|---------|------|
| V1 用户子类化 StreamError（重写 `__init__` 设 `self.extra`） | 抛错是内置 StreamError + message/path 非空 + **不是** MyStreamError 实例（指针对比） + **无** extra 属性（Rust 不调用户 `__init__`） | 5/5 PASS |
| V2 用户子类化 RangeError（Array(5) build 6 元素） | 抛错是 RangeError + 不是 MyRangeError + message/path 设置 | 4/4 PASS |
| V3 内置 StreamError fast-path 属性 | 是 StreamError + message 是 str + path 是 str + **args 是 1 元组**（fast-path: `args=(message,)`） | 4/4 PASS |
| V4 `str(e)` 对齐 Python construct | 捕获 + 以 "Error in path " 开头 + 含换行 | 3/3 PASS |
| V5 pickle 兼容 | dumps 成功 + loads 成功 + 类型一致 + message 一致 | 4/4 PASS |

**关键证据**：

- V1.4：抛错不是 MyStreamError 实例（Rust 用指针相等比较，**不**把内置类替换为用户子类）。
- V1.5：`err` 无 `extra` 属性（Rust 不会调用用户重写的 `__init__` 来"伪装"内置类）。
- V3.4：`args` 是 1 元组 `(message,)`（fast-path 行为差异，本 ADR 决策 4 显式批准）。

**BC3 真正语义**："Rust 抛内置类 + 用户 `except` 内置类"，而非"Rust 把用户子类注入到抛错链路"。OBS-2 修订的指针比较实现正确。

## Alternatives Considered

### 替代方案 1：仅缓存异常类 PyObject 引用（INVEST §5.3 第 1 项建议）

**描述**：模块初始化时缓存 13 个 Python 异常类的 `Py<PyType>` 引用，避免每次错误抛出都 `import` + `getattr`。

**为何不采用**：**优化空间为零**——`ExceptionClasses` struct（`error.rs:488`）已在模块初始化时（`init_exception_classes`）通过 `GILOnceCell` 缓存全部 13 个 Python 异常类的 `Py<PyType>` 引用。设计 §0.3 关键澄清：INVEST §5.3 第 1 项建议"实际上已经实施"，§16.2.3 表格预测"缓存类引用可节省 ~600ns"的部分优化空间为零。本 ADR 必须寻找更深层优化路径（O3 绕过 `__init__`）。

### 替代方案 2：仅实施 O1（lazy 字符串预格式化）

**描述**：仅优化 F1（字符串预格式化），避免 `to_string()` × 2-3 次的 Rust String 分配。

**为何不采用（单独）**：节省有限（~100ns），无法触及核心瓶颈 F7（`tp_init` 字节码 ~600-900ns）。a_err_eof 即便实施 O1 也仅从 6.45x 提升至 ~7x，远低于 10x 用户硬约束 #5。本 ADR 将 O1 纳入 4 级优化组合（O1+O2+O3+O4），但 O1 单独不足以达标。

### 替代方案 3：仅实施 O4（PyErr 直接构造）

**描述**：仅优化 F8（`PyErr::from_value_bound` 包装），用 `PyErr::from_value` 一步构造，避免中间层 incref。

**为何不采用（单独）**：节省有限（~30-50ns），同样无法触及核心瓶颈 F7。本 ADR 将 O4 纳入 4 级优化组合（与 O3 合并——fast-path 末段直接构造 PyErr），但 O4 单独不足以达标。

### 反例 R1：fast-path 完全跳过 `tp_alloc`

**描述**：用预分配的异常实例池（每次抛出复用并 set 新属性），完全跳过 `PyType_GenericAlloc` 的 ~200ns。

**为何未实施**：复杂度高（需评估线程安全 + 实例复用的引用计数管理）。O3 已保留 `tp_alloc`（CPython 固有 ~200ns，不可压缩），实测 a_err_eof 11.34x 达标，无需进入 R1 流程。仅在反例触发（实施 O1-O4 后加速比 <10x）时才考虑。

## Relations

- **关联教训**：`experiences.md §L-09`（跨时段性能对比消除法归因失效）
  - VET 用 Controlled A/B Test 验证 O1-O4 真实生效（Δ=+1.53x >0.5x 显著），而非依赖跨时段单次对比。
  - DEV OBS-DEV-2 假设 i02_idx_n100 改善（INVEST 10x → 4.x 后 17.3x）归因 crate LTO 副作用，被 Controlled A/B Test 证伪（A 状态 18.06x vs B 状态 17.77x，Δ=-0.29x <0.3x 波动），实际是 17 天跨时段测量环境漂移。L-09 对策有效。
- **关联 ADR**：`ADR-016 P0-3 lazy path 错误传播`（互补关系，见下表）
- **设计文档**：`docs/design/模块设计-错误路径优化.md`（§1 a_err_eof 优化 + §1.3 O1-O4 + §1.3.1 unsafe 安全前置 + §3 ADR-019 候选评估）
- **实测数据**：`plans/phase4-array/过程记录.md §4.x-VET`（VET Controlled A/B Test 完整数据 + DLL hash 验证 + T6 子类化兼容性 20/20 PASS）
- **实现位置**：`construct-rs/src/error.rs::try_fast_path_alloc`（L740-825）+ `is_builtin_class`（L560-579）+ `From<ConstructError> for PyErr`（L857-895）
- **测试位置**：T6 子类化兼容性测试（`experiments/phase4_4x_t6_verify.py`，VET 独立验证 20/20 PASS）+ `error.rs` 测试模块 8 个新单元测试（L1606-1901）
- **首次验证**：Phase 4.x（错误路径 D 类优化，2026-07-28 ACCEPTED）
- **未来复用**：Phase 5+（Struct + FFI 入口优化）及未来 Phase（Sequence / Union / Lazy）的错误抛出路径可直接引用此 ADR

### 与 ADR-016 的互补关系

ADR-019 与 ADR-016 形成**错误路径优化的两个互补维度**：

| 维度 | ADR-016（lazy path 错误传播） | ADR-019（错误抛出 fast-path） |
|------|------------------------------|------------------------------|
| 优化阶段 | 错误抛出**之前**（Rust 内部 path 重建） | 错误抛出**之时**（Rust → Python PyErr 构造） |
| 优化对象 | `ConstructError::push_path_segment` / `push_path_index` | `From<ConstructError> for PyErr` |
| 核心机制 | 成功路径不维护 Path 栈（零分配），错误时重建 | 内置类绕过 `__init__` 直接构造实例，用户子类走慢路径 |
| 收益场景 | 嵌套 Struct + Array 错误路径（Phase 1 验证，Phase 4.7 推广） | 所有错误抛出场景（Phase 4.x 验证） |
| 组合效果 | 单独使用：节省 path 维护开销 | 与 ADR-016 叠加：错误路径从 path 重建 + PyErr 构造双维度优化 |

**协同验证**：4.x VET 性能验证同时测量 a_err_eof 在 ADR-016（Phase 4.7 已实施）+ ADR-019（4.x 实施）双重优化下的加速比（11.34x），验证两者无相互冲突。

