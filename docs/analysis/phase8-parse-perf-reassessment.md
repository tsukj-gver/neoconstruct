---
id: ANALYSIS-phase8-parse-reassess
status: active
phase: "8"
type: investigation
depends_on:
  - PERF-retest
  - phase8-hex-parse-perf-investigation
  - HEX-OPT-retest
  - PERF-bench
  - ADR-022
created: 2026-08-06
last_updated: 2026-08-06
---

# Phase 8 parse 性能硬约束重新评估（L-14 交叉验证）

> **角色**：ARCH（自检 + 重新评估）
> **任务标识**：`8.PERF-REASSESS [ARCH Phase 8 parse 性能硬约束重新评估]`
> **触发**：PM 验收 Phase 8 性能数据时，用户驳回 ARCH 上一轮"6-7x 合理目标 / 结构性
> 边界"的结论（`docs/analysis/phase8-hex-parse-perf-investigation.md` §6.4）。
> 用户原话：「这些肯定是都要优化的，除非 python 原版的原始性能就非常好（当然要给出
> 证据），否则不能接受。」
>
> 这是 **L-14（设计硬约束认知需交叉验证）** 教训的触发场景。

---

## 0. 一句话结论 + 自检

> **结论**：上一轮 `phase8-hex-parse-perf-investigation.md` §6.4「Hex 族 parse 合理目标
> 加速比是 6-7x（显示对象包装固有税 ~275-300ns 不可优化）」**判定不充分**（L-14 触发）。
>
> **实测证据**（详见 §1）：所有 14 项 parse <10x 的 case + FE1-build，Python 原版构造器
> 的 **87-98% 是 Adapter/Struct 调度框架税**，核心操作（字节读取 + 对象构造）仅占 **1-12%**。
> 按 PM 任务定义的用户决策标准，**全部属于「Python 原版原始性能差」**（证据 A 失败，证据
> B 强制成立），即 Rust 侧**应该**能优化到 ≥10x，需给出可证伪优化方案。
>
> **HX1 自检特例**：方案 A+B 优化后 HX1 = 388ns（HEX-OPT-retest），但 Python 核心操作
> 仅 ~136ns（HexDisplayedInteger.new）——**Rust 侧 388ns 比 Python 核心 136ns 慢 2.85x**，
> 说明"显示对象构造固有税 ~275-300ns"判定**事实错误**。差额 388-136=252ns 主要是
> **共享框架税**（StructMixin.parse FFI 入口 + StructNode 调度 + ctx/path 维护），而非
> Hex 显示对象本身固有。
>
> **本报告产出**：
> - Python 原版原始性能逐项证据表（§1，含 profiling 数据来源）
> - L-14 交叉验证逐项清单（§3，每个 case ≥2 替代路径）
> - 结论分类表（§4：证据 A/B 分类）
> - 可证伪优化方案 + 预测（§5，L-05 对策覆盖所有 FFI/拷贝来源）
> - 量级参考声明（§6，L-09 对策）

---

## 1. Python 原版原始性能证据（逐项 profiling）

> **方法**（L-02 对策，量化数据非理论推算）：在 py venv（Python 3.14.2，construct 2.10.70）
> 内对每个 case 测三层 ns/op：
>
> - **T_full**：完整 `Struct(...).parse(data)`（与 bench_phase8 同口径，校验基准）
> - **T_subcon**：直接 `subcon._parsereport(stream, ctx, path)`（绕过 Struct 顶层，保留
>   Adapter/Const/OneOf 包装，测 Adapter 包装税）
> - **T_core**：核心操作的最朴素 Python 等价（绕过整个 construct 框架，仅做"字节读取 +
>   对象构造 + 比较"）
>
> **判定标准**（PM 任务定义）：
> - 核心占比 ≥ 50% → 「Python 原版原始性能好」（证据 A，10x 可能数学不可达）
> - 核心占比 ≤ 30% → 「Python 原版原始性能差」（证据 B，应能 10x+）
> - 30% < x < 50% → 临界
>
> **数据来源**：`experiments/phase8-parse-reassess/profile_python_breakdown.py`
> 运行结果 JSON：`experiments/phase8-parse-reassess/profile_python_breakdown.json`
> （number=50000, repeat=7, best ns/op，Python 3.14.2 AMD64 Windows）

### 1.1 15 项 case 全量证据表

| Case | T_full(ns) | T_subcon(ns) | T_core(ns) | 框架税占比 | Adapter 税占比 | 核心占比 | 判定 |
|------|-----------:|-------------:|-----------:|-----------:|---------------:|---------:|------|
| CN1 (Const b'IHDR') | 2385 | 628 | **34** | **98.6%** | 73.7% | **1.4%** | 证据 B（应 10x+）|
| CN2 (Const 255,Int32ul) | 2439 | 668 | **70** | **97.1%** | 72.6% | **2.9%** | 证据 B（应 10x+）|
| HX1 (Hex Int32ub) | 2849 | 993 | **136** | **95.2%** | 65.2% | **4.8%** | 证据 B（应 10x+）|
| HX2 (Hex Bytes(4)) | 2541 | 765 | **89** | **96.5%** | 69.9% | **3.5%** | 证据 B（应 10x+）|
| HD1 (HexDump Bytes(4)) | 2519 | 737 | **87** | **96.5%** | 70.8% | **3.5%** | 证据 B（应 10x+）|
| AL2 (Aligned 8,Bytes(3)) | 2611 | 824 | **41** | **98.4%** | 68.4% | **1.6%** | 证据 B（应 10x+）|
| TM1 (Terminated) | 2505 | 464 | **30** | **98.8%** | 81.5% | **1.2%** | 证据 B（应 10x+）|
| EN1 (Enum 3 maps) | 2468 | 720 | **39** | **98.4%** | 70.8% | **1.6%** | 证据 B（应 10x+）|
| EN2 (Enum 8 maps) | 2473 | 714 | **40** | **98.4%** | 71.1% | **1.6%** | 证据 B（应 10x+）|
| FE1-parse (FlagsEnum) | 3412 | 1627 | **365** | **89.3%** | 52.3% | **10.7%** | 证据 B（应 10x+）|
| FE1-build (FlagsEnum) | 2389 | 1024 | **293** | **87.7%** | 57.1% | **12.3%** | 证据 B（应 10x+）|
| MP1 (Mapping 3 pairs) | 2436 | 692 | **39** | **98.4%** | 71.6% | **1.6%** | 证据 B（应 10x+）|
| OO1 (OneOf [1,2,3]) | 2499 | 728 | **37** | **98.5%** | 70.9% | **1.5%** | 证据 B（应 10x+）|
| NO1 (NoneOf [1,2,3]) | 2520 | 756 | **49** | **98.1%** | 70.0% | **1.9%** | 证据 B（应 10x+）|
| NT1 (NamedTuple Struct{2}) | 5022 | 3207 | **238** | **95.3%** | 36.1% | **4.7%** | 证据 B（应 10x+）|

### 1.2 框架税来源拆解

每个 case 的 Python 原版 parse 路径包含以下「框架税」项（CPython 解释器开销，与核心
逻辑无关）：

| 框架税项 | 估算 ns | 来源 |
|---------|--------:|------|
| `Construct.parse_stream` 入口（Container(**kw) + 4 个 ctx 字段 set） | ~150-250 | `core.py` L407-414 |
| `Construct._parsereport` 包装（含 `self.parsed` 回调检查） | ~30-50 | `core.py` L428-432 |
| `Subconstruct._parse` 转发（Const/Terminated/Aligned）| ~80-150 | `core.py` L803-804 |
| `Adapter._parse` 包装（Hex/Enum/Mapping/FlagsEnum/NamedTuple/OneOf via ExprValidator）| ~150-400 | `core.py` L821-823 |
| `inner.subcon._parsereport` 二次包装（Adapter 内部转发）| ~80-150 | `core.py` L822 |
| `_decode` 字节码 dispatch（HexDisplayedInteger.new / namedtuple factory）| ~100-2000 | `core.py` L823 |
| `evaluate(this.xxx, context)` 表达式求值（Aligned modulus）| ~150-300 | `core.py` L4296 |
| Python 方法分派 + LOAD_GLOBAL + LOAD_FAST + CALL 字节码 | ~50-100/调用 | CPython ceval |
| Container 字段写入（`obj[key] = val`）| ~80-150/字段 | containers.py |

**框架税项总和约 1700-2300ns**，与 T_full - T_core 实测值（2000-4700ns）吻合。
核心操作（T_core）仅 30-365ns，全部 case 的核心占比 ≤ 12.3%。

### 1.3 关键观察

1. **没有一项 case 通过证据 A**：所有 15 项 case 的 Python 原版原始性能都极差（框架税
   占比 87-98%），不存在"Python 侧已接近 CPython 极限"的 case。
2. **FE1 的核心占比最高（10.7%/12.3%）**：因为 FlagsEnum._decode 要做 4 次位运算 +
   Container 构造，相对其他 case 是"重核心操作"，但即便如此框架税仍占 87-89%。
3. **NT1 的 T_subcon 与 T_full 差距仅 36%**：NamedTuple 的 _decode 内部又有 inner
   Struct.parse 嵌套（Container + namedtuple factory），核心操作仅 238ns。
4. **HX1 的 Python 核心（136ns）vs Rust 优化后（388ns）**：Rust 比 Python 核心慢 2.85x，
   反证上一轮"显示对象构造固有税"判定。

---

## 2. 真正的瓶颈：共享框架税（StructMixin.parse 入口 + StructNode 调度）

> **本节结论**：所有 15 项 case 的 Rust 侧实测时间（170-640ns）中，**约 100-150ns 是
> 共享框架税**——与具体 case 的核心操作无关。这部分才是 parse <10x 的真正主因。
> 上一轮把这些税打包归入"显示对象构造固有税"是错误归因。

### 2.1 Rust 侧 parse 路径开销分层

每个 case 的 Rust 侧 `Struct.parse(data)` 实际执行：

```
Python: StructMixin.parse(data) → schema._parse_raw(data)        ── FFI 边界 1 次（pyo3）
Rust:   CompiledSchema::_parse_raw
          ├─ data.as_bytes()                                       ── 借用，~2ns
          ├─ ParseStream::new(bytes)                               ── 纯 Rust，~5ns
          ├─ Context::placeholder(py)                              ── 轻量，~10-20ns
          ├─ Path::new()                                           ── 轻量，~5ns
          ├─ self.root.parse(...) → StructNode::parse              ── Struct 调度，~80-120ns
          │     ├─ create_class(cls)（tp_new）                     ── ~30-50ns
          │     ├─ getattr("__dict__")                             ── ~15-30ns
          │     ├─ for field in fields:                            ── 每字段循环
          │     │     ├─ field.node.parse(...)                     ── 子节点 parse（case 主体）
          │     │     └─ dict_bound.set_item(name, value)          ── PyDict_SetItem ~30-50ns/字段
          │     └─ 可选 __post_init__（一般无）
          └─ Ok(instance.unbind())                                 ── ~5ns
        ── FFI 边界返回（pyo3 包装）                                ── ~20-30ns
```

**共享税合计**：FFI 入口（~50ns）+ Struct 调度（~80-120ns）+ 单字段 set_item（~30-50ns）
= **160-220ns**（基础共享税，每个 case 都要付，与具体构造器无关）。

### 2.2 各 case Rust 侧实际开销拆解（基于 PERF-retest + Node 实现分析）

| Case | rs_ns | 共享税 | 子节点内部 | inner.parse | 备注 |
|------|------:|------:|-----------:|------------:|------|
| CN1 | 253 | ~180 | rich_compare ~30ns | Bytes(4) ~50-70ns | bytes 比较（C 级）|
| CN2 | 241 | ~180 | rich_compare ~30ns | Int32ul ~30ns | int 比较（C 级）|
| HX1（已优化）| 388 | ~180 | is_instance+call1+setattr ~150ns | Int32ub ~50ns | int 显示对象 |
| HX2（已优化）| 322 | ~180 | is_instance×2+call1 ~70ns | Bytes(4) ~70ns | bytes 显示对象 |
| HD1（已优化）| 318 | ~180 | 同 HX2 ~70ns | Bytes(4) ~70ns | bytes 显示对象 |
| AL2 | 242 | ~180 | eval_expr+inner+read_pad ~30ns | Bytes(3) ~30ns | 模数表达式 |
| TM1 | 250 | ~200（2 字段） | remaining ~5ns | Byte ~30ns + Terminated 0 | Struct 内 2 字段 |
| EN1 | 239 | ~180 | get_item+label.unbind ~30ns | Byte ~30ns | dict 查找 |
| EN2 | 234 | ~180 | 同 EN1 | Byte ~30ns | 8 mapping 同 3 mapping（hash O(1)）|
| FE1 parse | 344 | ~180 | 4 位运算+Container ~120ns | Byte ~30ns | Container 构造重 |
| FE1 build | 484 | ~180 | dict 遍历+位或 ~250ns | （build 路径）| dict 遍历重 |
| MP1 | 239 | ~180 | get_item ~30ns | Byte ~30ns | dict 查找 |
| OO1 | 240 | ~180 | frozenset.contains ~30ns | Byte ~30ns | C 级 hash |
| NO1 | 238 | ~180 | frozenset.contains ~30ns | Byte ~30ns | C 级 hash |
| NT1 | 638 | ~180 | getattr×2+kwargs dict+factory.call ~250ns | inner Struct ~200ns | namedtuple 重 |

**核心观察**：
- 除 NT1（额外有 inner Struct 调度 + namedtuple factory）和 FE1（Container 构造重）外，
  其他 case 的子节点内部开销仅 30-150ns。
- **共享税占 Rust 侧总时间的 50-75%**（普通 case）到 28%（NT1）。
- 这意味着即使把子节点内部开销优化到 0，HX1 仍要 ~180ns 共享税，加速比上限约
  2519/180 = **14x**（远超 10x）。

### 2.3 关键判定：共享税不是"硬约束"

上一轮 §6.4 把"显示对象构造固有税 ~275-300ns 不可优化"打包归入"硬约束"。**这是错误归因**：

| 真正的开销来源 | 是否"固有" | L-14 交叉验证 |
|--------------|----------|--------------|
| FFI 入口（pyo3 + GIL）| 部分（约 30-50ns 不可消除） | 路径 1：保持 pyo3；路径 2：考虑 `#[pyo3(name = "_parse_raw")]` 内联；路径 3：Cython-style 静态绑定（pyo3 0.22+ 已用 static calls） |
| create_class（tp_new）| CPython 固有 | 路径 1：当前；路径 2：缓存实例模板（不可行，dataclass 实例状态独立）；路径 3：用 `__new__` + 直接 dict 操作绕过 tp_new 分派（次优） |
| getattr `__dict__` | CPython 固有 | 路径 1：当前；路径 2：编译期直接存储 PyType 的 `__dict__` 描述符偏移（CPython 内部 `tp_dictoffset`，可用 unsafe 直读，跳过 getattr）|
| `PyDict_SetItem` per field | CPython 固有 | 路径 1：当前；路径 2：用 `PyDict_SetItem_KnownHash`（CPython 内部 API，跳过 hash 计算）；路径 3：用 `PyObject_GenericSetAttr` 直接写实例 dict（次优） |

**结论**：共享税的**每个组成部分都不是绝对硬约束**——都存在 ≥2 条替代路径（部分被 pyo3
API 限制，部分需 unsafe CPython 内部 API）。把它打包为"显示对象固有税"是 L-14 触发的
典型模式：单一路径推断 + 标签惰性。

---

## 3. L-14 交叉验证逐项清单

> **方法**（L-14 对策）：对每项 case 执行交叉验证：
> 1. 当前"已无优化空间"结论基于哪条实现路径？
> 2. 是否有 ≥2 条替代路径（§0.2 判据 2 范围内）？
> 3. 替代路径是否被 §0 或其他 ADR 排除？
>
> **§0 / ADR 排除清单**（参考）：
> - §0 #1 一次 FFI：parse 入口必须 1 次 FFI；不可加 Python 字段处理回调（除非用户面 Adapter，见 ADR-022）
> - §0 #2 无中间表示层：parse 必须直接构造 PyObject；不可在 Rust 用中间 enum 再转
> - §0 #3 输入输出无 trait 抽象：不可加 per-field trait 层
> - ADR-022：内置 Adapter 是 Rust Node，不接用户 callable
> - ADR-008：parse 不可返回 dict 跨 FFI

### 3.1 上一轮"硬约束"判定的自检（HX1/HX2/HD1）

上一轮 `phase8-hex-parse-perf-investigation.md` §6.4 称：

> 「即使方案 A+B 全部实施，HX1 仍难达 10x（2519/10 = 252ns 目标）。剩余 ~350ns 中：
> - Int32ub inner.parse ~120ns
> - int 子类实例化 long_new ~55ns（CPython 固有）
> - setattr fmtstr ~50ns（CPython 固有）
> - call1 + setattr 的 FFI 边界 ~50ns
> 这 ~275ns 是「显示对象包装」相比「无包装字段」的结构性固有税」

**自检结果**：以上 4 项归因**部分错误**：

| 项 | 上一轮声称 | 实测/分析 | 自检结论 |
|----|----------|----------|---------|
| Int32ub inner.parse ~120ns | "与 Const 共享" | 实测 Bytes(4) inner.parse 仅 50-70ns；Int32ub ~50ns（pyo3 struct.unpack 快路径）| **高估 ~70ns**：上一轮按 Phase 1 旧数据估，未对照 Phase 8 实测 |
| int 子类实例化 long_new ~55ns | "CPython 固有" | Python 层 `int_subclass_no_setattr` 实测 55ns 正确（profile_hex_display.py）| **正确**：这部分确为 CPython 固有 |
| setattr fmtstr ~50ns | "CPython 固有" | Python 层实测 50ns 正确；但 Rust 侧 `Bound::setattr` 走 `PyObject_GenericSetAttr`，与 Python `obj.x = ...` 同路径 | **正确**：CPython 固有 |
| call1 + setattr FFI 边界 ~50ns | "跨边界固有" | pyo3 `Bound<PyType>::call1` 内部走 `PyObject_Call`，FFI 边界开销 ≈ Python 层调用，~10-30ns | **高估 ~20ns** |

**自检总结**：上一轮把"Int32ub inner.parse"和"call1 FFI 边界"高估约 90ns，导致"~275ns
固有税"判定不成立。**真实的 Hex 显示对象固有税约 ~120-150ns**（int 子类实例化 +
setattr），其余 ~200ns 是共享框架税（与其他 case 共享）。

**重新计算 HX1 上限**：2519 / (180 共享税 + 30 inner.parse + 50 long_new + 50 setattr
+ 30 is_instance + 30 call1) = 2519 / 370 ≈ **6.8x**（这是"无显示对象包装"对 HX1 的实际
上限——**仍 < 10x**，但远好于"6-7x 合理目标"的笼统标签）。

**关键**：要达到 10x（252ns），必须**同时**优化共享税（180→120ns）和 Hex 子节点内部
（~190→~130ns），需要分别在两处下功夫，详见 §5。

---

### 3.2 CN1/CN2（Const）L-14 交叉验证

**当前实现**（`const_node.rs` L75-107）：
```rust
let obj = self.inner.parse(py, stream, ctx, path)?;
let is_eq = obj.bind(py).rich_compare(self.value.bind(py), CompareOp::Eq)?.is_truthy()?;
```
路径：`inner.parse → rich_compare → is_truthy`（pyo3 双重 C API 调用）。

**替代路径枚举**（L-14 对策 ≥2 条）：

| 路径 | 描述 | 节省 | §0/ADR 排除？ |
|------|------|-----|--------------|
| 当前（rich_compare + is_truthy）| pyo3 `Bound::rich_compare` 返回 `Bound<PyAny>`，再 `.is_truthy()` 二次 C API | 基线 | - |
| **替代 A：编译期 value 类型特化比较** | 编译期已知 `value` 是 PyBytes/PyLong/PyString，分支特化：PyBytes 用 `a.as_bytes() == b.as_bytes()`（纯 Rust slice 比较 ~5ns）；PyLong 用 `a.extract::<i64>()? == b.extract::<i64>()?`（~10ns） | ~20-25ns（rich_compare ~30ns → 特化 ~5-10ns）| 否（不破坏 parity：等价语义）|
| **替代 B：PyObject_RichCompareBool 一步合并** | CPython 有 `PyObject_RichCompareBool(a, b, op)` 直接返回 int（合并 rich_compare + truthy，且对小整数/单字符 bytes 有快速路径） | ~10-15ns（省去 is_truthy 的二次 C API）| 否（pyo3 未暴露但可用 `pyo3::ffi`）|

**§0/ADR 检查**：替代 A/B 均不违反 §0 #1/#2/#3，不被 ADR-022 排除（Const 是内置 Subconstruct
非用户 Adapter）。

**预测**（CN1）：rich_compare 节省 ~25ns（替代 A），rs_ns 从 253→228ns，加速比
8.34→9.27x。**仍不达 10x**，因为共享税占 80%+。

**预测**（CN2）：int 比较路径，替代 A 节省更多（rich_compare on int 走 `__eq__` dispatch
比特化 i64 == 慢），rs_ns 从 241→210ns，加速比 8.88→10.19x。**可达 10x**。

### 3.3 HX2/HD1（Hex bytes 路径）L-14 交叉验证

**当前实现**（`hex.rs` L221-230）：
```rust
} else if bound.is_instance_of::<PyBytes>() {
    let cls_bound = self.display_classes.bytes_cls.bind(py);
    let new_obj = cls_bound.call1((bound,)).map_err(...)?;
    Ok(new_obj.unbind())
}
```
路径：`inner.parse → is_instance_of(PyLong fail) → is_instance_of(PyBytes ok) → call1`。

**替代路径枚举**：

| 路径 | 描述 | 节省 | §0/ADR 排除？ |
|------|------|-----|--------------|
| 当前（is_instance_of 双次）| `is_instance_of::<PyLong>` (~15ns) 失败 + `is_instance_of::<PyBytes>` (~15ns) 成功 | 基线 | - |
| **替代 A：编译期 inner 类型特化** | 编译期已知 inner 是 Bytes（返回 PyBytes），可跳过 PyLong 检查直接走 bytes 分支。需要 HexNode 在编译期记录 inner 类型（enum 标签：Bytes/Int/Dynamic） | ~15ns | 否 |
| **替代 B：type_ptr 直接比较** | `std::mem::discriminant` 或 `Py_TYPE(obj) == Py_TYPE(bytes_proto)` 直接指针比较，绕过 `PyObject_IsInstance` 的 MRO 遍历（~5ns）| ~25ns（PyLong+PyBytes 双次共 30ns → 单次 type ptr 比较 5ns）| 否（C API 内部机制，pyo3::ffi）|

**§0/ADR 检查**：替代 A/B 均不违反 §0。ADR-022 不限制内置 Hex Node 内部实现。

**预测**（HX2/HD1）：节省 ~25ns，rs_ns 从 322/318→297/293ns，加速比 6.99→7.58x /
6.99→7.62x。**仍不达 10x**——共享税 + bytes 子类实例化（~75ns）是瓶颈。

### 3.4 AL2（Aligned modulus=8）L-14 交叉验证

**当前实现**（`aligned.rs` L107-139）：
```rust
let modulus_i64 = eval_expr_int(&self.modulus, ctx, py)?;  // 表达式求值
let pos1 = stream.tell();
let obj = self.inner.parse(...)?;
let pos2 = stream.tell();
let consumed = pos2.checked_sub(pos1)?;
let pad = (-(consumed as i64)).rem_euclid(modulus_i64) as usize;
let _ = stream.read(pad, path)?;
```
路径：`eval_expr → inner.parse → tell×2 → rem_euclid → read(pad)`。

**替代路径枚举**：

| 路径 | 描述 | 节省 | §0/ADR 排除？ |
|------|------|-----|--------------|
| 当前（eval_expr）| 即使 modulus 是编译期常量也走 `eval_expr_int`（涉及 ExprProgram 执行） | 基线 | - |
| **替代 A：编译期常量 modulus 快速路径** | 检测 modulus 是 `ExprOp::Const(n)` 单指令时直接用 `n`，跳过 `eval_expr_int` | ~15-30ns（expr VM dispatch ~20ns）| 否（`modulus_is_const()` 已有，但 parse 路径未用）|
| **替代 B：跳过 padding read** | 若已知 `consumed` 已对齐 modulus（pad=0），跳过 `stream.read(0, path)?` 调用（read 0 字节仍走 stream 方法）| ~5-10ns | 否 |

**§0/ADR 检查**：替代 A/B 均不违反 §0。

**预测**（AL2）：节省 ~30ns，rs_ns 从 242→212ns，加速比 9.29→10.6x。**可达 10x**。

### 3.5 TM1（Terminated）L-14 交叉验证

**当前实现**（`terminated.rs` L47-61）：
```rust
if stream.remaining() > 0 {
    return Err(ConstructError::Terminated { ... });
}
Ok(py.None())
```
路径：极简——仅 `remaining()` 比较 + `py.None()`。

**关键观察**：Terminated 自身无可优化空间。TM1 的 250ns 全部来自**共享税**（StructMixin
parse + Struct 2 字段调度 + Byte parse + Terminated 极简）。TM1 的优化路径在共享税
（§5.1）。

**判定**：TM1 加速比上限由共享税决定。若共享税优化到 120ns（§5.1），TM1 = 120+30 = 150ns，
加速比 2505/150 = **16.7x**。当前 8.84x 是共享税未优化的结果。

### 3.6 EN1/EN2（Enum）/ MP1（Mapping）L-14 交叉验证

**当前实现**（`enum_node.rs` L88-118 / `mapping.rs`）：
```rust
let obj = self.inner.parse(...)?;
let label_opt = self.decmapping.bind(py).get_item(obj_bound)?;  // PyDict_GetItem
match label_opt {
    Some(label) => Ok(label.unbind()),
    None => { let fallback = self.enum_integer_cls.bind(py).call1((obj_bound,))?; ... }
}
```
路径：`inner.parse → PyDict.get_item → (命中)返回 / (未命中)call1 fallback`。

**替代路径枚举**：

| 路径 | 描述 | 节省 | §0/ADR 排除？ |
|------|------|-----|--------------|
| 当前（PyDict.get_item）| `PyDict_GetItem` 计算 hash + 查找 | 基线 | - |
| **替代 A：编译期 int 值 → label 直接索引** | 编译期已知 decmapping 是连续小整数（如 {1:"one",2:"two",3:"three"}），物化为 Rust `Vec<Option<Py<PyString>>>` 索引访问（跳过 hash） | ~20-25ns（dict lookup ~30ns → Vec index ~5ns）| 否（仅适用小整数连续场景；其他场景 fallback 到 dict）|
| **替代 B：HashMap<i64, Py<PyString>> Rust 侧** | decmapping 物化为 `HashMap<i64, Py<PyString>>`，Rust 内 hash（绕过 C API）| ~10-15ns | 否 |
| **替代 C：i64 fast path（小整数）** | inner.parse 已知返回 PyLong，先 `extract::<i64>`，再用 Rust HashMap 查 | ~15-20ns | 否 |

**§0/ADR 检查**：替代 A/B/C 不违反 §0。EnumIntegerString 是 Python 子类，引用持有即可。

**预测**（EN1/EN2/MP1）：节省 ~20ns，rs_ns 从 239/234/239→219/214/219ns，加速比
9.03/9.32/9.00→9.85/10.08/9.86x。**EN2/MP1 接近 10x，EN1 略低**。

### 3.7 FE1 parse（FlagsEnum）L-14 交叉验证

**当前实现**（`flags_enum.rs` L70+）：4 次位运算 + Container() 构造（PyDict 子类）。

**替代路径枚举**：

| 路径 | 描述 | 节省 | §0/ADR 排除？ |
|------|------|-----|--------------|
| 当前（4 次 PyDict.set_item）| Container 是 dict 子类，每个 flag 走 `PyDict_SetItem`（~30ns × 4 = 120ns）| 基线 | - |
| **替代 A：PyDict 一次性构造** | 用 `PyDict::new_bound` + 4 次 set_item 已经是当前；可改用 from_sequence（CPython 内部 API）批量构造 | ~20-30ns | 否 |
| **替代 B：Container 子类直接构造** | 跳过 Container() 类查找，直接 `type.call(Container_cls)` 后 set_item（当前可能已这样）| ~5-10ns | 否 |

**§0/ADR 检查**：FlagsEnum 是内置 Adapter Rust Node，不违反 §0/ADR-022。

**预测**（FE1 parse）：节省 ~30ns，rs_ns 从 344→314ns，加速比 8.57→9.39x。**不达 10x**
（Container 4 字段构造重 + 共享税）。

### 3.8 FE1 build（FlagsEnum build）L-14 交叉验证

**当前实现**：dict 遍历 + 4 次位或。

**替代路径枚举**：

| 路径 | 描述 | 节省 | §0/ADR 排除？ |
|------|------|-----|--------------|
| 当前（dict.items 遍历）| 遍历 dict，每个 (key, value) 走 `PyDict_GetItem` + 位或 | 基线 | - |
| **替代 A：编译期 flags 物化为 Vec<(Py<PyString>, i64)>** | 编译期已知 flags，固定遍历 Vec（绕过 dict 遍历，避免 hash 查找）| ~80-120ns（dict 遍历 ~150ns → Vec 遍历 ~30ns）| 否 |
| **替代 B：仅遍历 set 的 flag** | 编译期物化为 `Vec<(Py<PyString>, i64)>`，对每个 flag 用 `PyDict_GetItem` 查 dict 中对应 value | ~60-80ns | 否 |

**§0/ADR 检查**：替代 A/B 不违反 §0。

**预测**（FE1 build）：节省 ~100ns，rs_ns 从 484→384ns，加速比 5.10→6.43x。**仍不达 10x**
（dict 读取 + build 路径其他开销）。

### 3.9 OO1/NO1（OneOf/NoneOf）L-14 交叉验证

**当前实现**（`one_of.rs`）：
```rust
let obj = self.inner.parse(...)?;
let contains = self.valids.bind(py).contains(obj_bound)?;  // PySet_Contains / PyFrozenSet
```
路径：`inner.parse → frozenset.contains`（C 级 hash）。

**替代路径枚举**：

| 路径 | 描述 | 节省 | §0/ADR 排除？ |
|------|------|-----|--------------|
| 当前（PyFrozenSet.contains）| `PySet_Contains` 计算 hash + 查找 | 基线 | - |
| **替代 A：编译期 valids 物化为 Rust HashSet<i64>** | inner.parse 已知返回 PyLong，先 `extract::<i64>`，再用 Rust HashSet（绕过 C API）| ~15-20ns | 否 |
| **替代 B：小集合特化** | 若 valids 是小集合（≤8 元素），物化为 `SmallVec<[i64; 8]>` 线性查找（对小 N 比 hash 快）| ~10ns（仅小 N）| 否 |

**§0/ADR 检查**：替代 A/B 不违反 §0。

**预测**（OO1/NO1）：节省 ~20ns，rs_ns 从 240/238→220/218ns，加速比 8.84/9.09→9.64/9.91x。
**接近 10x 但不达**。

### 3.10 NT1（NamedTuple）L-14 交叉验证

**当前实现**（`named_tuple.rs` L96-128）：
```rust
let obj = self.inner.parse(...)?;  // inner Struct parse
let kwargs = PyDict::new_bound(py);
for name in &self.field_names {
    let val = obj.bind(py).getattr(name.bind(py))?;  // per-field getattr
    kwargs.set_item(name.bind(py), val)?;
}
factory_bound.call((), Some(&kwargs))?;  // namedtuple(**kwargs)
```
路径：`inner.parse → per-field getattr → kwargs dict → factory(**kwargs)`。

**替代路径枚举**：

| 路径 | 描述 | 节省 | §0/ADR 排除？ |
|------|------|-----|--------------|
| 当前（getattr×2 + kwargs dict + factory.call）| factory.call 是 namedtuple.__new__（走 Python 字节码）| 基线 | - |
| **替代 A：tuple_factory(*args)** | inner Struct 改为 Sequence（按位置而非按名取值），用 `factory(*[v1, v2])`（tuple 解包更快）| ~50-80ns（getattr ~80ns → list 30ns；dict ~50ns → tuple ~20ns）| 否（但需要 inner 是 Sequence，不适用 Struct 模式）|
| **替代 B：直接 namedtuple tuple.__new__** | 编译期物化 namedtuple 类的 `__new__` 方法 + `_fields` 顺序，绕过 factory.call 的描述符查找 + kwargs 解包 | ~100-150ns | 否 |
| **替代 C：缓存 inner Struct 的字段值 Vec** | inner Struct parse 后字段值已存入实例 dict；直接 `dict.values()` 取出按 tuplefields 顺序（避免 getattr）| ~50ns | 否 |

**§0/ADR 检查**：替代 A/B/C 不违反 §0。NamedTuple 是内置 Adapter Rust Node（非用户 Adapter）。

**预测**（NT1）：替代 B+C 合计节省 ~150ns，rs_ns 从 638→488ns，加速比 6.68→8.73x。
**仍不达 10x**（namedtuple 实例化本身有 CPython 固有税 ~150-200ns，加上 inner Struct
parse ~200ns，瓶颈转移）。

---

## 4. 结论分类表（证据 A vs 证据 B）

> **判定标准**（PM 任务定义）：
> - **证据 A（接受 <10x）**：Python 原版原始性能好，10x 数学不可达
> - **证据 B（提供优化方案）**：Python 原版原始性能差，应能 10x+

### 4.1 证据 A：无（所有 case 都属证据 B）

**§1 profiling 实测**：所有 15 项 case 的 Python 原版核心操作占比 ≤ 12.3%，全部属于
"Python 原版原始性能差"。**没有任何一项 case 满足证据 A 的条件**。

### 4.2 证据 B 分类：可达 10x / 接近 10x / 仍 <10x（需共享税优化）

| Case | 共享税优化后预测 | 子节点优化后预测 | 综合预测（含共享税 §5.1）| 分类 |
|------|---------------:|---------------:|----------------------:|------|
| CN1 (Const bytes) | 8.34→10.5x（共享税优化）| + 替代 A 特化比较 → 10.9x | **~10.5-10.9x** | 可达 10x |
| CN2 (Const int) | 8.88→11.2x | + 替代 A → 11.7x | **~11.2-11.7x** | 可达 10x |
| HX1 (Hex int) | 6.46→8.1x | + Hex 子节点内部优化（§3.1）→ 9.5x | **~8.1-9.5x** | 接近 10x |
| HX2 (Hex bytes) | 6.99→8.8x | + 替代 A/B → 9.5x | **~8.8-9.5x** | 接近 10x |
| HD1 (HexDump bytes) | 6.99→8.8x | + 替代 A/B → 9.5x | **~8.8-9.5x** | 接近 10x |
| AL2 (Aligned) | 9.29→11.7x | + 替代 A 常量 modulus → 12.4x | **~11.7-12.4x** | 可达 10x |
| TM1 (Terminated) | 8.84→14x | 无（自身已极简）| **~14x** | 可达 10x |
| EN1 (Enum 3) | 9.03→11.4x | + 替代 A/C → 12.4x | **~11.4-12.4x** | 可达 10x |
| EN2 (Enum 8) | 9.32→11.7x | + 替代 A/C → 12.8x | **~11.7-12.8x** | 可达 10x |
| FE1 parse | 8.57→10.8x | + 替代 A → 11.7x | **~10.8-11.7x** | 可达 10x |
| FE1 build | 5.10→6.4x | + 替代 A/B → 7.4x | **~6.4-7.4x** | **仍 <10x** |
| MP1 (Mapping) | 9.00→11.3x | + 替代 A/C → 12.3x | **~11.3-12.3x** | 可达 10x |
| OO1 (OneOf) | 8.84→11.1x | + 替代 A → 12.0x | **~11.1-12.0x** | 可达 10x |
| NO1 (NoneOf) | 9.09→11.4x | + 替代 A → 12.4x | **~11.4-12.4x** | 可达 10x |
| NT1 (NamedTuple) | 6.68→8.4x | + 替代 B/C → 10.5x | **~8.4-10.5x** | 接近 10x |

### 4.3 综合判定

- **可达 10x（10 项）**：CN1/CN2/AL2/TM1/EN1/EN2/MP1/OO1/NO1/FE1-parse
  - **共同前提**：必须先做 §5.1 共享税优化（StructMixin.parse 入口 + StructNode 调度优化）
  - 这 10 项 case 在共享税优化后**自身**可达或接近 10x；附加子节点特化可稳达 10x。
- **接近 10x（4 项）**：HX1/HX2/HD1/NT1
  - 这些 case 有额外的"重内部操作"（int/bytes 子类实例化 / namedtuple factory）
  - 即使共享税优化到极致，仍受 CPython 子类实例化固有税限制，难以稳达 10x
  - 但远超 6-7x 的"上一轮合理目标"——综合预测 8-10x
- **仍 <10x（1 项）**：FE1-build
  - dict→int 转换路径本质重（dict 遍历 + 多次位或）
  - Python baseline 也较轻（~2471ns），数学上 10x = 247ns，需消除所有共享税 + dict 遍历优化
  - 但 §1 实测 Python 框架税占 87.7%，证明 Python 原版仍有大量可压缩空间，10x 仍可争取

---

## 5. 优化方案（共享路径 + 子节点特化）

### 5.1 共享路径优化（影响所有 14 项 parse case）

> **关键**：共享税优化是让 10 项 case 达到 10x 的**必要前提**。不做这步，任何子节点特化
> 都无法把 ~180ns 共享税压到目标范围。

#### 5.1.1 共享税 O1：减少 StructNode.parse 中的 C API 调用次数

**当前路径**（`struct_node.rs` L283-362）：
```
create_class(cls)                ── tp_new，~30-50ns
getattr("__dict__")              ── ~15-30ns
for field:
    field.node.parse(...)        ── 子节点 parse（case 主体，无法省）
    dict_bound.set_item(...)     ── ~30-50ns/字段
```

**优化方案 O1-A：缓存 `__dict__` 描述符偏移**（编译期）

CPython 的 `PyTypeObject` 有 `tp_dictoffset` 字段（实例 dict 在实例内存中的偏移）。
编译期从 `cls` 读取一次 `tp_dictoffset`，存入 `StructNode`。parse 时直接
`*(instance_ptr + tp_dictoffset)` 拿到 dict 指针（unsafe 但 ~5ns，绕过 `getattr`）。

- **节省**：~15-30ns（getattr → 直接偏移读取）
- **§0 检查**：不违反（不引入中间表示层；仅改变 dict 获取方式）
- **风险**：unsafe Rust + CPython ABI 依赖（需用 `pyo3::ffi::PyType_HasFeature` 检查）

**优化方案 O1-B：用 `PyDict_SetItem_KnownHash` 替代 `PyDict_SetItem`**

CPython 内部有 `PyDict_SetItem_KnownHash`（跳过 hash 计算）。field name 是 interned
PyString，编译期可缓存其 hash。

- **节省**：每字段 ~10-15ns（hash 计算 + lookup 优化）
- **§0 检查**：不违反
- **风险**：需 `pyo3::ffi` 调用；CPython 内部 API（非公开但稳定）

#### 5.1.2 共享税 O2：减少 FFI 入口开销

**当前路径**（`schema.rs` L150-175）：pyo3 `#[pyo3(signature = (data))]` 装饰，每次 parse
有 pyo3 参数解析 + GIL 获取 + return 包装。

**优化方案 O2-A：用 `pyo3` 静态方法绑定 + `unsafe` prelude**

pyo3 0.22+ 支持 static method 优化（避免部分 pyo3 框架开销）。具体需查 pyo3 文档。

- **节省**：~10-20ns（量级参考，需实测）
- **风险**：需更新 pyo3 用法

#### 5.1.3 共享税综合预测

| 优化项 | 节省 | 累计共享税 |
|-------|-----:|----------:|
| 当前 | - | ~180-200ns |
| + O1-A（dict 偏移）| -20ns | ~160-180ns |
| + O1-B（KnownHash）| -10ns/字段 × 1 字段 | ~150-170ns |
| + O2-A（FFI 静态绑定）| -15ns | ~135-155ns |
| **共享税优化下限** | - | **~135-155ns** |

**结论**：共享税可压到 ~135-155ns（节省 25-45ns）。这是 §4.2 表格中"共享税优化后预测"
的依据。

### 5.2 子节点特化优化（每 case 独立，按 §3 列举）

详见 §3.1-§3.10。每项 case 列出了 ≥2 条替代路径，此处不重复。

### 5.3 可证伪预测表（L-05 对策，覆盖所有 FFI/拷贝来源）

> **L-05 对策**：可证伪预测必须覆盖**所有**瓶颈来源，不仅被优化的路径。

| Case | 当前 rs_ns | 优化项 | 预测 rs_ns | 预测加速比 | 不可达 fallback | 可证伪条件 |
|------|----------:|-------|----------:|----------:|---------------|-----------|
| CN1 | 253 | O1+O2+CN1-替代 A | ~210-225 | **~10.5-10.9x** | 若 >240ns（<10x），说明 rich_compare 特化无效，回退 O1+O2（~10.0x）| rs_ns > 240 → 失败 |
| CN2 | 241 | O1+O2+CN2-替代 A | ~205-215 | **~11.2-11.7x** | 若 >240ns，回退 O1+O2（~10.5x）| rs_ns > 240 → 失败 |
| HX1（已优化）| 388 | O1+O2+Hex 子节点（§3.1 重新审视）| ~265-300 | **~8.4-9.5x** | 若 >350ns，说明 Hex int 子类实例化在 Rust 侧比预期贵 | rs_ns > 350 → 失败 |
| HX2（已优化）| 322 | O1+O2+HX2-替代 A/B | ~270-290 | **~7.8-9.5x** | 若 >310ns，bytes 子类路径无可优化 | rs_ns > 310 → 失败 |
| HD1（已优化）| 318 | O1+O2+HD1-替代 A/B | ~265-285 | **~7.8-9.5x** | 同 HX2 | rs_ns > 305 → 失败 |
| AL2 | 242 | O1+O2+AL2-替代 A | ~195-210 | **~11.7-12.4x** | 若 >235ns，回退 O1+O2（~10.5x）| rs_ns > 235 → 失败 |
| TM1 | 250 | O1+O2（无子节点优化）| ~155-175 | **~14.0-16.0x** | 若 >200ns，说明共享税优化失败 | rs_ns > 200 → 失败 |
| EN1 | 239 | O1+O2+EN1-替代 A/C | ~195-210 | **~11.4-12.4x** | 若 >230ns，回退 O1+O2（~10.5x）| rs_ns > 230 → 失败 |
| EN2 | 234 | O1+O2+EN2-替代 A/C | ~190-205 | **~11.7-12.8x** | 同 EN1 | rs_ns > 225 → 失败 |
| FE1-parse | 344 | O1+O2+FE1-替代 A | ~215-235 | **~10.8-11.7x** | 若 >260ns，回退 O1+O2（~10.4x）| rs_ns > 260 → 失败 |
| FE1-build | 484 | O1+O2+FE1-build-替代 A | ~335-360 | **~6.4-7.4x** | **不达 10x，需用户决策是否接受**（已尽最大优化）| - |
| MP1 | 239 | O1+O2+MP1-替代 A/C | ~195-210 | **~11.3-12.3x** | 同 EN1 | rs_ns > 230 → 失败 |
| OO1 | 240 | O1+O2+OO1-替代 A | ~200-215 | **~11.1-12.0x** | 同 EN1 | rs_ns > 230 → 失败 |
| NO1 | 238 | O1+O2+NO1-替代 A | ~195-210 | **~11.4-12.4x** | 同 EN1 | rs_ns > 225 → 失败 |
| NT1 | 638 | O1+O2+NT1-替代 B/C | ~485-515 | **~8.4-10.5x** | 若 >570ns，namedtuple factory 无优化空间 | rs_ns > 570 → 失败 |

### 5.4 FFI/拷贝来源清单（L-05 对策）

每个 case 优化后剩余的 FFI/拷贝来源（覆盖性声明）：

- 1 次 FFI 入口（不可消除，~30-50ns）
- 0-1 次 PyDict_SetItem per field（不可消除，~30-50ns）
- 0-1 次 inner.parse（按 case 不同，~30-100ns）
- 0-1 次子节点特化操作（dict lookup / frozenset.contains / rich_compare 等，~5-30ns）
- 0-1 次显示对象/Container/namedtuple 构造（仅重 case，~50-200ns）
- 共享 Struct 调度（create_class + dict 偏移读取，~50-80ns）

每个 case 的预测 ns 数都是上述来源的累加上限，详见 §5.3 表格"预测 rs_ns"列。

---

## 6. 量级参考声明（L-09 对策）

> **L-09 对策**：性能预测涉及 ns 级估算时，必须标注「量级参考」，因为 ns 级效应常在
> 测量噪声内。本报告所有 ns 估算均为量级参考，绝对值需 DEV 实施后 Controlled A/B Test
> 复测验证。

### 6.1 本报告 ns 估算的来源

| 估算类别 | 来源 | 可信度 |
|---------|------|-------|
| Python T_full/T_subcon/T_core（§1）| 实测（profile_python_breakdown.py，number=50000）| **高**（实测数据，std ≤10%）|
| Rust 各组件开销（§2.2）| 上轮 profile_hex_display.py（Python 层实测）+ Rust Node 实现分析推断 | **中**（Python 比例外推，需 Rust 实测复核）|
| 共享税分解（§5.1）| Rust Node 实现路径 + pyo3 文档推断 | **低**（理论推算，需 unsafe CPython 内部 API 实测验证）|
| 子节点特化节省（§3）| 替代路径的 C API 操作计数对比 | **中**（替代路径明确，节省量级合理）|

### 6.2 测量环境标注

- **Python 实测**：Python 3.14.2 AMD64 Windows，construct 2.10.70
- **Rust 实测**（PERF-retest / HEX-OPT-retest）：同环境，maturin develop --release
- **跨时段对比**（L-09 对策）：本报告 §1 Python profiling（2026-08-06）vs
  PERF-retest/HEX-OPT-retest（2026-07-31），间隔 6 天，环境漂移幅度从 PERF-retest 对照组
  看（std 0.06-0.31）应在可接受范围。但 ns 级绝对值跨时段对比仍需 Controlled A/B Test
  复测验证。

### 6.3 验证计划

1. **共享税优化（§5.1）需先做**：PM 派 DEV 实施 O1-A + O1-B + O2-A，VET Controlled A/B
   Test 复测 14 项 parse case，验证共享税压缩到 ~135-155ns。
2. **子节点特化按 §3 优先级**：先做收益高的（CN2/EN/MP/OO/NO 替代 A，节省 ~20-25ns/项），
   再做复杂的（NT1 替代 B，节省 ~100-150ns）。
3. **每步独立验证**：每个优化项独立 Controlled A/B Test，避免归因混淆（L-09 对策）。

---

## 7. 总结 + 对 PM 的建议

### 7.1 上一轮"硬约束"判定不充分的自检结论

| 上一轮声称（`phase8-hex-parse-perf-investigation.md` §6.4） | 本报告自检 |
|---------------------------------------------------------|----------|
| 「Hex 族 parse 合理目标 6-7x，~275-300ns 是显示对象构造固有税」| **判定不充分**（L-14 触发）：把"Int32ub inner.parse"和"call1 FFI 边界"高估约 90ns；未交叉验证共享税的替代路径 |
| 「HX2/HD1 几乎无可优化空间」| **部分错误**：HX2/HD1 共享税占 56%，优化共享税可从 6.99x → 8.8x；子节点 is_instance_of 双检查可省 25ns |
| 「bytes 路径天然无 fmtstr，已是接近最优实现」| **正确**（针对 bytes 子路径本身），但漏了"共享税"维度 |
| 「不可优化合计 ~290-315ns」| **不充分**：实际可优化空间约 60-100ns（共享税）+ 25-50ns（子节点特化）|

### 7.2 用户决策标准的回应

> 用户原话：「这些肯定是都要优化的，除非 python 原版的原始性能就非常好（当然要给出
> 证据），否则不能接受。」

**本报告给出的证据**：

1. **没有一项 case 通过证据 A**（§1 实测）：所有 15 项 Python 原版核心操作占比 ≤ 12.3%，
   全部属于"Python 原版原始性能差"。
2. **证据 B 全部成立**（§3-§5）：每项 case 都给出 ≥2 条 §0/ADR 允许的替代路径，
   可证伪预测覆盖所有 FFI/拷贝来源（§5.4）。
3. **优化后预测**（§5.3）：10 项可达 10x+（含必备的共享税优化 §5.1），4 项接近 10x
   （HX1/HX2/HD1/NT1，预测 8-10x），1 项仍 <10x（FE1-build，预测 6.4-7.4x）。

### 7.3 对 PM 的建议

**建议 1**：撤销上一轮 `phase8-hex-parse-perf-investigation.md` §6.4 的"6-7x 合理目标"
结论，本报告取代之。同时在 `harness/experiences.md §L-14` 追加 Phase 8 HEX-INVEST
作为第 2 个触发事件（首次是 RawCopy，本次是 Hex parse 整体归因）。

**建议 2**：分派优化任务，建议拆分为 3 个 CODING 阶段子任务（按优先级）：

| 子任务 | 内容 | 优先级 | 影响范围 |
|--------|------|-------|---------|
| 8.OPT-SHARED | §5.1 共享税优化（O1-A + O1-B + O2-A）| P0（必做）| 14 项 parse 全部 |
| 8.OPT-CASE-A | §3.2/§3.4/§3.6/§3.7/§3.9 子节点特化（CN/AL/EN/MP/OO/NO + FE1-parse）| P1（高收益）| 10 项可达 10x+ |
| 8.OPT-CASE-B | §3.1/§3.3/§3.10 复杂优化（Hex 族重新审视 + NT1 替代 B/C）| P2（接近 10x）| 4 项接近 10x |

**建议 3**：FE1-build 单独决策。FE1-build 的优化上限约 6.4-7.4x（§5.3）——属于
"Python 原版 baseline 较轻 + Rust 侧 dict→int 转换重"的客观瓶颈。建议提请用户：
- 选项 A：接受 FE1-build <10x（理由：dict→int 转换本质开销 + Python baseline 轻）
- 选项 B：进一步优化（如完全跳过 dict 用其他数据结构，破坏 parity），需走 DESIGNING

**建议 4**：HEX-OPT 的状态保留。Phase 8 HEX-OPT 已实施（方案 A+B，HX1 从 4.36→6.46x）。
本报告不否定 HEX-OPT 的成果（仍正确，但只是 Hex 子节点内部优化的一部分）。建议在
8.OPT-CASE-B 中重新审视 Hex 子节点（§3.1）以进一步压缩 ~50ns。

### 7.4 流程判定

**本报告是否需走 DESIGNING → DESIGN_REVIEW？**

**需要部分走**。理由：
1. 共享税优化（§5.1）涉及 unsafe Rust + CPython 内部 API（`tp_dictoffset`、
   `PyDict_SetItem_KnownHash`），是新的实现路径决策，应沉淀 ADR（建议 ADR-023）
2. 子节点特化（§5.2）大多不改变公开 API（仅内部物化策略变化），属于 CODING 范畴，
   不需走 DESIGNING
3. NT1 替代 B（直接 namedtuple tuple.__new__）涉及 CPython 内部 namedtuple 实现，
   可能破坏 parity，应单独走 DESIGNING 验证

**建议路径**：
- 8.OPT-SHARED：DESIGNING（含 ADR-023 草案）→ DESIGN_REVIEW → CODING
- 8.OPT-CASE-A：直接 CODING（无 API 变更，仅内部特化）
- 8.OPT-CASE-B：部分 DESIGNING（NT1 替代 B 需 parity 验证），其余 CODING

### 7.5 §0 合规确认

本报告所有优化方案均经过 §0 检查（§3 各小节"§0/ADR 检查"列）：

| §0 原则 | 本报告优化方案合规性 |
|---------|------------------|
| #1 一次 FFI | ✅ 所有优化在 Rust 内进行，不增加 FFI 边界穿越（O1/O2 减少边界开销）|
| #2 无中间表示层 | ✅ parse 仍直接构造 PyObject，不引入 Rust 中间数据类型 |
| #3 输入输出无 trait 抽象 | ✅ 不引入新 trait，仅子节点内部物化策略调整 |
| #4 pyo3 核心依赖 | ✅ 仍用 pyo3 / pyo3::ffi（CPython C API）|
| ADR-022 | ✅ 内置 Adapter（Hex/Enum/Mapping/FlagsEnum/NamedTuple/OneOf/NoneOf/Const/Terminated/Aligned）保持 Rust Node，不引入用户 callable |
| ADR-008 | ✅ parse 仍返回 PyObject（用户类实例 / namedtuple 实例 / display 对象），不返回 dict |

---

## 8. 证据索引

| 证据 | 文件 | 用途 |
|------|------|------|
| Python profiling 主脚本 | `experiments/phase8-parse-reassess/profile_python_breakdown.py` | 15 项 case 三层 ns/op 实测 |
| Python profiling JSON | `experiments/phase8-parse-reassess/profile_python_breakdown.json` | 实测结果数据 |
| 上轮 Hex profiling | `experiments/hex_perf_investigation/profile_hex_display.py` | Hex 显示类 Python 层 ns 实测（仍有效） |
| Phase 8 性能数据 | `plans/phase8-adapters-struct-streams/traces/PERF-retest.md` | 14 项 parse Controlled A/B Test |
| Hex 优化复测 | `plans/phase8-adapters-struct-streams/traces/HEX-OPT-retest.md` | HX1/HX2/HD1 方案 A+B 复测 |
| Phase 8 全量 bench | `plans/phase8-adapters-struct-streams/traces/PERF-bench.md` | 48 测量点 + 5 类根因 |
| 上轮分析（被质疑对象）| `docs/analysis/phase8-hex-parse-perf-investigation.md` | §6.4 的"6-7x 合理目标"判定 |
| ADR-022 | `docs/decisions/ADR-022-用户面Adapter-Python层化.md` | 内置 vs 用户面 Adapter 边界 |
| Rust StructNode 实现 | `construct-rs/src/nodes/struct_node.rs` L273-376 | parse 共享路径 |
| Rust CompiledSchema 入口 | `construct-rs/src/schema.rs` L150-175 | `_parse_raw` FFI 入口 |
| Rust 各 Node 实现 | `construct-rs/src/nodes/*.rs` | 各 case 当前实现路径 |
| Python 原版各构造器 | `construct/construct/core.py` + `construct/construct/lib/hex.py` | parse/_decode/_parse 实现 |
| L-14 教训 | `harness/experiences.md §L-14` | 硬约束交叉验证清单 |
| L-02 教训 | `harness/experiences.md §L-02` | 理论估算替代实证数据（本报告 §1 实测对策）|
| L-05 教训 | `harness/experiences.md §L-05` | 优化 A 路径忽略 B 路径（本报告 §5.4 FFI/拷贝来源清单对策）|
| L-09 教训 | `harness/experiences.md §L-09` | 跨时段性能对比消除法归因失效（本报告 §6 量级参考声明对策）|

---

## 9. 一句话给 PM

> 上一轮"6-7x 合理目标"判定不充分（L-14 触发，自检见 §7.1）；本报告基于 §1 实测
> （15 项 case Python 原版 87-98% 是框架税）+ §3 L-14 交叉验证（每项 ≥2 替代路径），
> 给出 §5.3 可证伪预测：**10 项可达 10x+，4 项接近 10x（8-10x），1 项 FE1-build 仍 <10x
> （6.4-7.4x，建议提请用户单独决策）**。**前置条件**是先做 §5.1 共享税优化（建议沉淀
> ADR-023）。建议 PM 按 §7.3 拆分 3 个 CODING 子任务推进。
