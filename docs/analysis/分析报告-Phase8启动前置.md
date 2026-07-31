---
id: ANALYSIS-phase8-pre
status: active
phase: "8"
depends_on:
  - PLAN-phase8
  - ADR-022
  - QUERY-rawcopy-architecture
  - QUERY-bytesinteger-bigint
  - DESIGN-phase6-adapter
last_updated: 2026-07-30
---

# Phase 8 启动前置分析报告（22 个正常常用构造器）

> **角色**：ARCH
> **任务**：`8.0 [分析报告]`
> **范围**：Adapter 核心 13 + Struct 2 + Streams 2 + Control 1 + Other 4 = 22
> **依据**：`AGENTS.md §0` / `harness/experiences.md §L-01/L-02/L-05/L-13/L-14` / ADR-022 / `质疑-能否不用RawCopy.md` / Python `core.py` + `debug.py`

---

## 0. 摘要与核心结论

### 0.1 双层分离映射（PM 决策 2 + ADR-022 落实）

| 层 | 实现位置 | 数量 | 构造器 |
|----|---------|-----|--------|
| **内置 = Rust Node** | `construct-rs/src/nodes/` 新增 Node 变体 | **18** | Const / Default / Check / OneOf / NoneOf / Checksum / Enum / FlagsEnum / Mapping / Union / Hex / HexDump / Sequence / Terminated / Probe / NamedTuple / ProcessXor / ProcessRotateLeft |
| **Python 层（macro/Python 类）** | `construct-rs/python/construct/` | **3** | Validator（抽象基类，复用 AdapterCallbackNode）/ AlignedStruct（Struct+Aligned 宏）/ Timestamp（arrow 依赖，Python 层 Adapter 子类） |
| **特殊（Error 变体 + 顶层 catch）** | `ConstructError::CancelParsing` + `schema.rs` parse 入口 | **1** | CancelParsing |

**与 ADR-022 Relations 预设的差异**：ADR-022 曾预设 Hex/HexDump/Enum/Validator/Mapping/FlagsEnum 均走 AdapterCallbackNode。本报告修正：
- **Hex/HexDump/Enum/FlagsEnum/Mapping → Rust Node**（非 AdapterCallbackNode）：因 §0.2 的 "C API 操作 ≠ FFI crossing" 框架，这些内置 Adapter 的映射/显示逻辑可用编译期物化的 `Py<PyDict>` / `Py<PyFrozenSet>` + C API 直接操作，零额外 FFI。
- **Validator → Python 层**：抽象基类，用户继承实现 `_validate`，属用户域代码（与 Adapter/SymmetricAdapter 同档）。
- **Timestamp → Python 层**：`arrow` 第三方依赖 + Arrow datetime 算术，Rust 化不现实。
- **NamedTuple → Rust Node**（非 AdapterCallbackNode）：`collections.namedtuple` 工厂是 C 级元组子类构造，Rust 端 `factory.__call__()` 是 C API 调用（非用户代码）。

### 0.2 §0 合规框架澄清（关键，解锁 Enum/Mapping/OneOf/NamedTuple 的 Rust Node 路径）

**ADR-022 §0 #1 论证核心**："FFI crossing" = 调用**用户定义的 Python 代码**（如 `_decode`/`_encode`/`_validate` 方法体）。`AdapterCallbackNode` 的 2 次 FFI 即因调用用户 `_decode`。

**反例（本报告确立的判据）**：调用 **CPython 内置类型的 C API**（`PyDict_GetItem` / `PySet_Contains` / `PyType_Call` / `PyList_New`）是 §0 #1 明文允许的"Rust 内部通过 CPython C API 直接操作 Python 对象"，**不计额外 FFI**——即使内部触发 `__hash__`/`__eq__`（对 int/str/bytes/frozenset 等内置类型，这些是 C 级实现）。

**由此推导**：内置 Adapter 持有编译期物化的 `Py<PyDict>`（Enum/Mapping/FlagsEnum）/ `Py<PyFrozenSet>`（OneOf/NoneOf）/ `Py<PyType>`（NamedTuple），运行时通过 C API 查找/构造，全程 1 次 FFI（仅 parse 入口）。这是与 `StructNode` 用 `PyDict` 填充实例 `__dict__` 完全同脉络的合规模式。

**边界**：若用户传入**自定义子类**（如 `class MySet(set): def __contains__(...)`）给 OneOf，则 `__contains__` 是用户代码，会触发额外 Python 调用——但这是用户主动引入的边缘场景，不破坏"常见场景 1 FFI"的合规论证（类比 Computed 用户写复杂表达式仍走 ExprProgram）。

### 0.3 子任务拆分总览（12 个，详见 §3）

| 子任务 | 构造器 | 类型 | 新增 Node 变体 | 难度 |
|--------|--------|------|--------------|------|
| 8.1 | Const / Default / Check | 内置 Rust Node（Subconstruct/Construct 模式） | 3 | 低 |
| 8.2 | Enum / FlagsEnum / Mapping | 内置 Rust Node（dict 物化） | 3 | 中 |
| 8.3 | OneOf / NoneOf + Validator(Python) | 内置 Rust Node + Python 层 | 2 + 1 类 | 中低 |
| 8.4 | Hex / HexDump | 内置 Rust Node（类型分派 + 显示类） | 2 | 中 |
| 8.5 | Checksum + Rust hashfunc + ParseStream::slice | 内置 Rust Node + 新依赖 + 新 API | 1 + 基础设施 | **高** |
| 8.6 | Union | 内置 Rust Node（多视角 seek） | 1 | 中高 |
| 8.7 | Sequence | 内置 Rust Node（list 输出） | 1 | 中 |
| 8.8 | AlignedStruct + Aligned 前置 | Aligned Rust Node + AlignedStruct Python 宏 | 1 + 1 宏 | 中 |
| 8.9 | Terminated / Probe | 内置 Rust Node（简单 stream） | 2 | 低 |
| 8.10 | CancelParsing | Error 变体 + 顶层 catch | 0（Error 变体） | 低中 |
| 8.11 | NamedTuple / Timestamp | NamedTuple Rust Node + Timestamp Python 层 | 1 + 1 宏 | 中 |
| 8.12 | ProcessXor / ProcessRotateLeft | 内置 Rust Node（字节变换） | 2 | 中 |

**Node enum 净增**：18 个 Rust Node 变体（当前 39 个 → 57 个）。

---

## 1. 跨构造器设计原则（Phase 8 共通）

### 1.1 编译期物化模式（Enum/FlagsEnum/Mapping/OneOf/NoneOf 共用）

**模式**：用户面描述符（Python `EnumDescriptor` 等）在 `__init__` 时构建 Python 容器（dict/frozenset），编译期 `build_node_from_descriptor` 把这些容器作为 `Py<PyAny>`（GIL-independent 引用）存入 Rust Node。运行时 parse/build 通过 C API 查询。

**§0 对照**：
- #1 一次 FFI：✅ 仅 parse 入口 1 次，C API 查询不计额外 FFI（§0.2）
- #2 无中间表示层：✅ `Py<PyDict>` 是 Python 对象本身的引用，非 Rust 中间类型
- #3 输入输出无 trait 抽象：✅ 仅 `Box<Node>`（与现有模式同）

**性能**：C API dict 查找（int key）~50-100ns；frozenset `__contains__`（int）~30-50ns。远优于 Python 层 AdapterCallbackNode 的 ~200-300ns 回调 + Python dict 操作。

### 1.2 Subconstruct 包装模式（Const/Default/Hex/HexDump/ProcessXor/ProcessRotateLeft 共用）

**模式**：持有 `inner: Box<Node>` + 额外字段（value/valids/mapping）。parse 转发或包装 inner 结果。已由 Phase 6.3 SubconstructNode/PeekNode/RawCopyNode/RebuildNode 验证。

### 1.3 Construct 非包装模式（Check/Checksum/Terminated/Probe/Union/Sequence 共用）

**模式**：不持有 inner（或持有多 inner 列表）。parse/build 逻辑独立。参考 Phase 6.3 PassNode（no-op）、Phase 4 ArrayNode（循环）、Phase 7 FocusedSeqNode（多字段聚焦）。

### 1.4 L-14 对策落实：Checksum 零拷贝路径

`harness/experiences.md §L-14`（设计硬约束认知需交叉验证）已触发：`质疑-能否不用RawCopy.md §7-§10` 确认 Rust 内置 hashfunc（sha2/md-5/crc32fast/adler）+ StreamRange + `ParseStream::slice` = 全程零拷贝。本报告 §4 正式确认此方案，作为 8.5 的设计基线。

---

## 2. 22 构造器逐一分析

> 行号引用 Python `construct/construct/core.py`（除 Probe 在 `debug.py`）。

### 2.1 Adapter 核心（13）

#### 2.1.1 Const（core.py L2808-2876）
- **Python 摘要**：`Const(value, subcon=None)`。parse：subcon.parse → 检查 `obj == value`，不等抛 ConstError。build：obj ∈ {None, value} 时 subcon.build(value)，否则 ConstError。sizeof 转发。`flagbuildnone=True`。
- **适配方案**：**Rust Node（内置）**。`ConstNode { inner: Box<Node>, value: Py<PyAny> }`。parse：inner.parse → `obj.compare(value)?`（pyo3 rich compare C API）→ 不等 ConstError。build：obj is None or == value → inner.build(value)，否则 ConstError。
- **PM 决策 2 双层**：内置（Subconstruct 派生，固定语义）。
- **§0**：✅ 1 FFI，pyo3 compare 是 C API。
- **边界**：value 为 bytes 时 subcon 默认 Bytes(len)；为 int 时 subcon 必须显式给出。编译期 descriptor 处理此分支。
- **依赖**：无新依赖。参考 RebuildNode 模式。

#### 2.1.2 Default（core.py L3030-3078）
- **Python 摘要**：`Default(subcon, value)`。parse 转发 subcon（继承 Subconstruct）。build：obj is None → `evaluate(value)`，否则用 obj；subcon.build。`flagbuildnone=True`。
- **适配方案**：**Rust Node（内置）**。`DefaultNode { inner: Box<Node>, value: ExprProgram }`。value 必须是 Phase 2 表达式或常量（与 Rebuild 同脉络，ADR-006 + ADR-014：不接 callable）。
- **PM 决策 2 双层**：内置。
- **§0**：✅。
- **边界**：value 为常量（非表达式）时，编译期包成空 ExprProgram + 常量 PyObject（与 Computed 常量模式同）。
- **依赖**：ExprProgram（已有）。

#### 2.1.3 Check（core.py L3081-3129）
- **Python 摘要**：`Check(func)`。parse/build：`evaluate(func, context)`，非真抛 CheckError。sizeof=0。`flagbuildnone=True`。parse 返回 None。
- **适配方案**：**Rust Node（内置）**。`CheckNode { func: ExprProgram }`。必须作为 RO 字段（`_field_kind="ro"`，与 Computed 同）。parse：`eval_expr_bool(func, ctx)?` → 非真 CheckError，返回 Py_None。build：同检查（no-op 写流）。
- **PM 决策 2 双层**：内置。
- **§0**：✅。与 ComputedNode 同量级（ExprProgram eval ~10ns）。
- **边界**：func 必须是 Phase 2 表达式（ADR-006：仅 i64，无 lambda）。Python 用户写 `Check(lambda ctx: ...)` 不支持——parity 已知差异（与 Rebuild RB-5 同硬约束）。
- **依赖**：ExprProgram + compute_ro_value 集成（参考 RebuildNode §1.4.3）。

#### 2.1.4 OneOf（core.py L6320-6339）
- **Python 摘要**：`OneOf(subcon, valids)` = `ExprValidator(subcon, lambda obj,ctx: obj in valids)`。Validator 子类，`_validate` 返回 bool。
- **适配方案**：**Rust Node（内置）**。`OneOfNode { inner: Box<Node>, valids: Py<PyFrozenSet> }`（编译期把 list/set 转 frozenset 物化）。parse：inner.parse → `valids.contains(obj)?`（C API `PySet_Contains`）→ False 抛 ValidationError，否则返回 obj。
- **PM 决策 2 双层**：内置（OneOf 是核心库函数，非用户继承）。
- **§0**：✅（§0.2 框架：frozenset `__contains__` 对 int/str 是 C 级）。
- **边界**：valids 为空 → 总是 ValidationError（对齐 Python）。valids 含非 hashable 元素 → 编译期 TypeError。
- **依赖**：无新依赖。

#### 2.1.5 NoneOf（core.py L6342-6354）
- **Python 摘要**：`NoneOf(subcon, invalids)` = `ExprValidator(subcon, lambda obj,ctx: obj not in invalids)`。
- **适配方案**：**Rust Node（内置）**。`NoneOfNode`（与 OneOfNode 结构相同，检查取反）。可合并为 `MembershipCheckNode { inner, set: Py<PyFrozenSet>, mode: OneOf|NoneOf }`，或分两个 Node 变体（推荐分开，语义清晰）。
- **PM 决策 2 双层**：内置。
- **§0**：✅。
- **依赖**：同 OneOf。

#### 2.1.6 Validator（core.py L849-863）
- **Python 摘要**：`Validator(SymmetricAdapter)`。抽象基类，`_decode` 调 `_validate` 返回 bool，False 抛 ValidationError。用户继承实现 `_validate`。
- **适配方案**：**Python 层（用户面）**。用户继承 `Validator`（Python 类，复用 ADR-022 的 `SymmetricAdapter` 机制）写 `_validate`。嵌入 Struct 时走 `AdapterCallbackNode`（已实现，Phase 6.3）。
- **PM 决策 2 双层**：用户面（抽象基类，`_validate` 是用户代码 = ADR-022 §0 论证的用户域后处理）。
- **§0**：⚠️ 嵌入 Struct 时 2 FFI（PM 决策 2 已接受，不设硬门禁）。
- **边界**：`ExprValidator`（Python 函数式快捷构造）也走 Python 层（用户传 lambda validator）——但 construct-rs 表达式系统不接 lambda，`ExprValidator` 仅支持 Phase 2 表达式编译（若实现）或降为 AdapterCallbackNode。
- **依赖**：AdapterCallbackNode（已有）+ `Validator` Python 基类（`_adapters.py` 新增）。

#### 2.1.7 Enum（core.py L1920-2005）
- **Python 摘要**：`Enum(subcon, *merge, **mapping)`。`decmapping={v:label}`, `encmapping={label:v}`。parse：subcon.parse → `decmapping.get(obj, EnumInteger(obj))`（无映射返回 int，不报错）。build：obj is int → 用 obj；否则 `encmapping[obj]`（KeyError → MappingError）。
- **适配方案**：**Rust Node（内置）**。`EnumNode { inner: Box<Node>, decmapping: Py<PyDict>, encmapping: Py<PyDict> }`（编译期物化）。parse：inner.parse → `decmapping.get(obj)`（C API）→ Some(label) 返回 label，None 返回 `EnumInteger(obj)`（int 子类，需 Rust 端构造或调 Python factory）。build：int → inner.build(obj)；否则 encmapping.get(obj) → Some(v) inner.build(v)，None → MappingError。
- **PM 决策 2 双层**：内置。
- **§0**：✅（§0.2：dict lookup C 级）。
- **关于"是否需要表达式系统扩展 keyfunc"**：**不需要**。Python Enum 不用 keyfunc（用 `**mapping` dict）。Enum 仅需编译期 mapping 物化。表达式系统无需扩展。
- **边界**：(1) `EnumIntegerString.new(intvalue, stringvalue)` 是 int 可转换的 str 子类——Rust 端需持 Python factory（`Py<PyAny>`）调 `EnumIntegerString.new(v, label)`（C API 调用，非用户代码）。(2) `__getattr__`（`d.one` → label）是 Python 层描述符职责，Rust Node 不涉及。
- **依赖**：无新依赖。

#### 2.1.8 FlagsEnum（core.py L2018-2109）
- **Python 摘要**：`FlagsEnum(subcon, *merge, **flags)`。parse：subcon.parse（int）→ `Container({name: (obj & value == value)})`（dict，每 flag 一 bool）。build：int → 用 obj；str → split("|") OR；dict → OR where value True。
- **适配方案**：**Rust Node（内置）**。`FlagsEnumNode { inner: Box<Node>, flags: Py<PyList> }`（编译期物化为 `[(name: Py<PyString>, value: i64)]`，或 Rust `Vec<(Py<PyString>, i64)>`）。parse：inner.parse → 提取 int → 遍历 flags 构造 PyDict（每项 set_item bool）。build：根据 obj 类型分支（int/str/dict）。
- **PM 决策 2 双层**：内置。
- **§0**：✅。
- **边界**：parse 返回 dict（`_flagsenum=True` 标志）。build 的 str split("|") 需 Rust 端调 `PyString.split`（C API）。dict 输入需遍历 items。
- **依赖**：无新依赖。

#### 2.1.9 Mapping（core.py L2112-2156）
- **Python 摘要**：`Mapping(subcon, mapping)`。`decmapping={v:k}`, `encmapping=mapping`（注意 k 可为任意对象）。parse：`decmapping[obj]`（KeyError/TypeError → MappingError）。build：`encmapping[obj]`。
- **适配方案**：**Rust Node（内置）**。`MappingNode { inner: Box<Node>, decmapping: Py<PyDict>, encmapping: Py<PyDict> }`。与 EnumNode 结构同，但 key/value 可为任意 hashable 对象（不像 Enum 限定 int/str）。
- **PM 决策 2 双层**：内置（Mapping 是核心库 Adapter 子类，非用户继承基类）。
- **§0**：✅（§0.2：dict lookup；若 key 是自定义对象，`__hash__`/`__eq__` 是用户代码——边缘场景，不破坏常见场景合规）。
- **边界**：key 不可 hash → MappingError（TypeError 转换）。
- **依赖**：无新依赖。可与 EnumNode 共享 dict-lookup 辅助函数。

#### 2.1.10 Checksum（core.py L5532-5600）⭐ 重点
- **Python 摘要**：`Checksum(checksumfield, hashfunc, bytesfunc)`。parse：`hash1 = checksumfield.parse`；`hash2 = hashfunc(bytesfunc(context))`；不等抛 ChecksumError。build：`hash2 = hashfunc(bytesfunc(context))`；`checksumfield.build(hash2)`。
- **适配方案**：**Rust Node（内置）+ Rust hashfunc + StreamRange + ParseStream::slice**（`质疑-能否不用RawCopy.md §7` 确认方案）。详见 §4。
- **PM 决策 2 双层**：内置。
- **§0**：✅（Rust 内置 hashfunc 路径零拷贝；Python callable 路径兼容）。
- **边界**：详见 §4。
- **依赖**：新 crate（sha2/sha1/md-5/crc32fast/adler）+ ParseStream::slice（新 API）。

#### 2.1.11 Union（core.py L3641-3800）⭐ 重点
- **Python 摘要**：`Union(parsefrom, *subcons)`。parse：记录 fallback=tell；遍历 subcons 各自 parse（每次后 seek 回 fallback）；记录每 subcon 的 forward tell；最后按 parsefrom（None/int/str/lambda）seek 到指定 forward。返回 Container（所有命名 subcon 结果）。build：找首个 name in obj 的 subcon，build 之，返回。
- **适配方案**：**Rust Node（内置）**。`UnionNode { subcons: Vec<(Option<Py<PyString>>, Node)>, parsefrom: ParseFrom }`。`ParseFrom = None | Index(usize) | Name(Py<PyString>) | Expr(ExprProgram)`（Python callable 不支持，parity 已知差异）。
- **ParseStream 能力需求**：已有 `tell()` / `seek(pos, path)`（Phase 4/7）。Union 用：tell fallback → 循环 { subcon.parse; forward_i = tell; seek(fallback) } → 按 parsefrom seek(forward_selected)。**无需新 API**。
- **PM 决策 2 双层**：内置。
- **§0**：✅（seek 是纯 Rust 内部，多 subcon parse 在 Rust 内）。
- **边界**：(1) build 仅 build 首个匹配 subcon（Python 语义）。(2) parsefrom 为 lambda 不支持（ADR-006/014 同脉络，仅接表达式/常量）。(3) sizeof 永远 Err（Python 同）。
- **依赖**：ParseStream seek/tell（已有）。

#### 2.1.12 Hex（core.py L3523-3580）
- **Python 摘要**：`Hex(subcon)` Adapter。`_decode`：obj is int → `HexDisplayedInteger.new(obj, fmtstr)`；bytes → `HexDisplayedBytes(obj)`；dict → `HexDisplayedDict(obj)`；else 原 obj。`_encode`：return obj（透传）。
- **适配方案**：**Rust Node（内置，非 AdapterCallbackNode）**（`质疑-能否不用RawCopy.md §2.2` 决策）。`HexNode { inner: Box<Node>, display_classes: HexDisplayClasses }`。parse：inner.parse → Rust 内 `is_instance_of::<PyLong/PyBytes/PyDict>` 分派 → 调对应 Python 显示类 factory（`Py<PyAny>`，C API 调用）→ 返回显示对象。
- **PM 决策 2 双层**：内置（`质疑-能否不用RawCopy.md §2.2` 已论证）。
- **§0**：✅（类型分派 Rust 内联 + Python 显示类构造是 C API 调用，非用户代码）。
- **边界**：`fmtstr = "0%sX" % (2 * subcon.sizeof())`——sizeof 可能 Err（变长 subcon），此时 fallback 用通用格式。`lib/hex.py` 的 3 个显示类需在 Python 侧保留（Rust Node 调用它们）。
- **依赖**：Python `lib/hex.py` 显示类（已有，construct-rs 需 port）。

#### 2.1.13 HexDump（core.py L3583-3635）
- **Python 摘要**：`HexDump(subcon)` Adapter。`_decode`：bytes → `HexDumpDisplayedBytes(obj)`；dict → `HexDumpDisplayedDict(obj)`；else 透传。`_encode` 透传。
- **适配方案**：**Rust Node（内置）**。与 HexNode 同模式（`质疑-能否不用RawCopy.md §2.2`）。`HexDumpNode { inner, display_classes }`。
- **PM 决策 2 双层**：内置。
- **§0**：✅。
- **依赖**：同 Hex。可与 Hex 合并实现（共享类型分派 + 显示类加载逻辑）。

### 2.2 Struct（2）

#### 2.2.1 Sequence（core.py L2329-2487）⭐ 重点
- **Python 摘要**：`Sequence(*subcons)`。parse：`ListContainer()`；context nesting（`_=context`）；遍历 subcons，每 subcon.parse → append 到 list，命名 subcon 的值也写入 context。StopFieldError 中断。build：从 list 按序取元素 build。
- **适配方案**：**Rust Node（内置）**。`SequenceNode { fields: Vec<SequenceField> }`，`SequenceField { name: Option<Py<PyString>>, node: Node }`。
- **是否复用 StructNode 逻辑**：**不复用代码，但共享模式**。StructNode 把字段值写入实例 `__dict__`（PyDict sink）；SequenceNode 把字段值 append 到 `PyList`（list sink）。输出 sink 不同 → 独立 Node。但共享：context nesting 设置、`has_expressions` 递归、`StopField` 哨兵处理、`compute_ro_value`（命名 RO 字段）、`restore_index`。
- **PM 决策 2 双层**：内置（核心复合构造器）。
- **§0**：✅（PyList append 是 C API；1 FFI = parse 入口）。
- **边界**：(1) Sequence 字段可命名（写入 context）或匿名（仅 append）。construct-rs dataclass 语法不直接表达 Sequence（Sequence 是位置序，非命名序）→ Sequence 作为"构造器工厂"使用，非 StructMixin 子类。用户面 API：`seq = Sequence(Int8ub, Int16ub); seq.parse(b"...")`。(2) `>>` 操作符（Python 语法糖）不实现（construct-rs 不重载操作符）。
- **依赖**：StructNode 的 context-nesting 模式（参考，非代码依赖）+ StopField 哨兵（已有）。

#### 2.2.2 AlignedStruct（core.py L4334-4351）⭐ 重点
- **Python 摘要**：`AlignedStruct(modulus, *subcons)` = 宏，展开为 `Struct(*[sc.name / Aligned(modulus, sc) for sc in subcons])`。即每字段用 `Aligned` 包装（对齐到 modulus）。
- **适配方案**：**Python 层宏 + Aligned Rust Node 前置**。AlignedStruct 本身是 Python 函数（生成 Struct 描述符列表），但依赖 `Aligned` 节点。
  - **Aligned 前置**（不在 22 清单但是必要依赖）：`Aligned(modulus, subcon)` Subconstruct。parse：subcon.parse → 跳过 padding（`modulus - (tell % modulus) % modulus` 字节）。build：subcon.build → 写 padding。sizeof：subcon.sizeof + padding。
  - **Aligned 实现**：Rust Node（内置），`AlignedNode { inner: Box<Node>, modulus: ExprProgram }`。参考 PaddingNode 模式（Phase 3.3）。
- **PM 决策 2 双层**：AlignedStruct = Python 层宏（trivial，展开为 Struct+Aligned）；Aligned = Rust Node（内置）。
- **§0**：✅。
- **边界**：modulus 可为表达式（context lambda）→ ExprProgram。padding 字节恒为 `\x00`（Python 默认）。
- **依赖**：Aligned Rust Node（本子任务实现）+ StructNode（已有）+ Padding 写流（已有）。
- **PM 决策点 D-2**：Aligned 不在原 22 清单。是否将 Aligned 作为 8.8 子任务的隐含范围？（ARCH 推荐：是，Aligned 是 AlignedStruct 的必要前置，一并实现成本最低。）

### 2.3 Streams（2）

#### 2.3.1 Terminated（core.py L4727-4755）
- **Python 摘要**：`Terminated`。parse：`if stream.read(1): raise TerminatedError`（读到 EOF 则通过）。build：return obj（no-op）。sizeof：SizeofError。`flagbuildnone=True`。
- **适配方案**：**Rust Node（内置）**。`TerminatedNode`（零字段单元结构体，类似 PassNode）。parse：`if stream.remaining() > 0 { TerminatedError }`（比 Python read(1) 更直接，不消费字节——Python 的 read(1) 在 EOF 返回 b"" 等价 remaining==0）。build：no-op。sizeof：Err。
- **PM 决策 2 双层**：内置。
- **§0**：✅。
- **边界**：与 PassNode 同量级（< 100 行）。
- **依赖**：ParseStream::remaining（已有或 trivial 新增）。

#### 2.3.2 Probe（debug.py L6-95）
- **Python 摘要**：`Probe(into=None, lookahead=None)`。parse/build/sizeof 都调 `printout`：打印分隔线 + path + （可选）stream peek（tell/read(lookahead)/seek 回）+ （可选）context 子项或全 context。`flagbuildnone=True`。
- **适配方案**：**Rust Node（内置）**。`ProbeNode { into: Option<ExprProgram>, lookahead: Option<usize> }`。parse/build：调 Rust 内 `printout`（tell + 可选 read+seek + 可选 context dump）→ 返回 Py_None。sizeof：printout + 返回 0。
- **PM 决策 2 双层**：内置。
- **§0**：✅（打印是 stdout 副作用，非数据流 FFI；context 遍历用 C API）。
- **边界**：(1) `into` 是 context lambda → 仅支持 Phase 2 表达式（ADR-006/014），不支持任意 callable。(2) stdout 输出在 Rust 端用 `eprintln!` 或调 Python `print`（推荐后者，保持输出缓冲一致）。(3) context dump 需遍历 PyDict 并 repr 每项（C API）。
- **依赖**：ParseStream tell/read/seek（已有）+ Context 访问（已有）。

### 2.4 Control（1）

#### 2.4.1 CancelParsing（core.py L149-148 + L416-419）⭐ 重点
- **Python 摘要**：`CancelParsing` 是 **Exception 子类**（非 Construct 类！）。用户代码（如 Computed lambda、Adapter `_decode`）`raise CancelParsing` → 顶层 `parse_stream` 捕获（L416-419 `except CancelParsing: pass`）→ parse 提前终止，返回 None。
- **与 ExplicitError 的关系**：**完全不同机制**。
  - `CancelParsing`：**顶层 catch**（parse_stream 入口），中止整个 parse，返回 None。用户主动抛。
  - `ExplicitError`（Phase 7 Select 处理）：**字段级穿透**，不被 Peek/Select 吞掉，但正常向上传播到 parse 入口（转 ConstructError）。
- **适配方案**：**ConstructError 变体 + schema.rs 顶层 catch**。
  - 新增 `ConstructError::CancelParsing { path: String }`。
  - `schema.rs` parse 入口（`PyBytes::as_bytes` 后的 parse 调用）：捕获 `ConstructError::CancelParsing` → 返回 `Ok(Py_None)`（对齐 Python `except CancelParsing: pass`）。
  - Python 侧导出 `CancelParsing` 异常类（用户 `raise CancelParsing()` 跨 FFI 转为 `ConstructError::CancelParsing`）。
- **PM 决策 2 双层**：不适用（非 Adapter，是控制流原语）。
- **§0**：✅（Error 变体传播是 Rust 内部，顶层 catch 在 schema.rs）。
- **边界**：(1) construct-rs 表达式系统不接 lambda（ADR-006），用户无法在 Computed 表达式中 raise CancelParsing。用户仅能从 Python 层 Adapter `_decode`（AdapterCallbackNode）或自定义 Python 代码 raise。→ **CancelParsing 实际触发场景有限**（仅 Python 层用户代码）。(2) build 路径不捕获 CancelParsing（Python build_stream 无 try/except）。
- **依赖**：`ConstructError` 扩展 + `schema.rs` parse 入口修改（DEV 注意：parse 入口改动需重跑全量 parity 确保不回归）。

### 2.5 Other 常用（4）

#### 2.5.1 NamedTuple（core.py L3381-3446）
- **Python 摘要**：`NamedTuple(tuplename, tuplefields, subcon)` Adapter。subcon 须为 Struct/Sequence/Array/GreedyRange。`_decode`：Struct → `factory(**obj)`；Sequence/Array/GreedyRange → `factory(*obj)`。`_encode`：反向。`factory = collections.namedtuple(tuplename, tuplefields)`。
- **适配方案**：**Rust Node（内置）**。`NamedTupleNode { inner: Box<Node>, factory: Py<PyType>, mode: NamedTupleMode }`，`NamedTupleMode = Struct | Sequence`。parse：inner.parse → 按 mode 提取字段（Struct：按名取 dict 值；Sequence：list 解包）→ `factory.__call__(args)`（C API 构造 namedtuple 实例）。build：从 namedtuple 实例按 mode 提取 → inner.build。
- **PM 决策 2 双层**：内置（namedtuple factory 是 C 级元组子类构造，非用户代码；§0.2 框架）。
- **§0**：✅（`PyType.__call__` 对标准 namedtuple 是 C 级）。
- **边界**：(1) subcon 类型校验（Struct/Sequence/Array/GreedyRange）编译期做。(2) tuplefields 可为 str（空格分隔）或 list——编译期统一转 list 传给 `collections.namedtuple`。
- **依赖**：`collections.namedtuple`（Python 标准库，编译期 factory 物化为 `Py<PyType>`）。

#### 2.5.2 Timestamp（core.py L3449-3520）⭐ 重点
- **Python 摘要**：`Timestamp(subcon, unit, epoch)` 是**函数**（非类），返回 `TimestampAdapter`（Adapter 子类）实例。依赖 `arrow` 第三方包。两模式：(1) msdos（BitStruct + 自定义 decode/encode，2 秒分辨率，1980 epoch）；(2) epoch（`epoch.shift(seconds=obj*unit)` / `int((obj-epoch).total_seconds()/unit)`）。
- **适配方案**：**Python 层（macro 函数 + Adapter 子类）**。理由：
  1. `arrow` 是第三方 Python 包（非 Rust），Arrow datetime 算术（`.shift()`/`.total_seconds()`）无法 Rust 化；
  2. `_decode`/`_encode` 涉及 Arrow 对象构造与运算，是 100% Python 代码；
  3. msdos 模式内部用 BitStruct（已有 Rust Node）+ Python decode 逻辑。
- **PM 决策 2 双层**：用户面（arrow 依赖 + Python 算术 = 用户域后处理；与 AdapterCallbackNode §0 论证同脉络）。
- **§0**：⚠️ 嵌入 Struct 时 2 FFI（PM 决策 2 已接受）。Timestamp 是"Python 重依赖内置 Adapter"的典型——Rust 化不现实。
- **边界**：(1) `arrow` 是用户依赖（construct-rs 不打包 arrow，用户 `pip install arrow`）。(2) 编译期 `import arrow` 失败 → ImportError（对齐 Python L3478）。(3) msdos 模式的 BitStruct 在 Rust 内 parse，decode 在 Python 层。
- **依赖**：AdapterCallbackNode（已有）+ `arrow`（用户依赖）+ BitStruct（已有）。
- **PM 决策点 D-3**：Timestamp 是否纳入 Phase 8？（ARCH 推荐：是，但标注"性能不设硬门禁，Python 层 arrow 路径"；或标 `wont_implement` 若 arrow 依赖不可接受。）

#### 2.5.3 ProcessRotateLeft（core.py L5424-5529）
- **Python 摘要**：`ProcessRotateLeft(amount, group, subcon)` Subconstruct。parse：read entire stream → 按 amount/group 做位旋转左移（3 分支：amount==0 / group==1 查表 / amount%8==0 字节序 / 通用）→ `io.BytesIO(data)` 子流 → subcon.parse。build：反向（amount 取负）。
- **适配方案**：**Rust Node（内置）**。`ProcessRotateLeftNode { inner: Box<Node>, amount: ExprProgram, group: ExprProgram, precomputed: ... }`。parse：按 inner.sizeof 读字节（sizeof 未知则读至 EOF）→ Rust 内位旋转（3 分支，含预计算查表 `precomputed_single_rotations`）→ 构造子 ParseStream → inner.parse(子流)。build：build 到子缓冲 → 反向旋转 → 写主流。
- **PM 决策 2 双层**：内置（位旋转是 Rust 强项，性能优）。
- **§0**：✅（子流构造与 BitwiseNode/TransformNode 同模式；旋转在 Rust 内）。
- **边界**：(1) `group < 1` → RotationError。(2) `len(data) % group != 0` → RotationError。(3) amount 可为表达式 → ExprProgram。(4) `precomputed_single_rotations` 是类级缓存，Rust 端用 `lazy_static` 或编译期 `const`。
- **依赖**：无新依赖。参考 TransformNode 子流模式。

#### 2.5.4 ProcessXor（core.py L5357-5421）
- **Python 摘要**：`ProcessXor(padfunc, subcon)` Subconstruct。parse：evaluate pad（int/bytes）→ read entire stream → XOR（int：每字节 `b ^ pad`；bytes：`zip(cycle(pad))`）→ 子流 → subcon.parse。build：反向。
- **适配方案**：**Rust Node（内置）**。`ProcessXorNode { inner: Box<Node>, pad: XorPad }`，`XorPad = Int(u8) | Bytes(Py<PyBytes>)`（编译期物化；padfunc 为表达式时运行期求值）。parse：按 sizeof 读字节 → Rust 内 XOR（`iter().map(|b| b ^ pad)`）→ 子流 → inner.parse。build：反向。
- **PM 决策 2 双层**：内置。
- **§0**：✅。
- **边界**：(1) pad 为 bytes 且 len==1 → 转 int（对齐 Python L5389）。(2) pad 为全零 bytes/0 → 不变换（Python 快速路径 L5394/L5397）。(3) padfunc 为表达式 → ExprProgram 求值。
- **依赖**：无新依赖。可与 ProcessRotateLeft 合并子任务（共享子流模式）。

---

## 3. 子任务拆分建议（12 个）

> 每子任务含：构造器 / 新增 Node 变体数 / 预估 Rust 行数 / 依赖 / 建议优先级。

### 3.1 拆分表

| 子任务 | 构造器 | 新 Node | Rust 行估 | Python 行估 | 依赖 | 优先级 |
|--------|--------|--------|----------|------------|------|--------|
| **8.1** | Const / Default / Check | 3 | ~600 | ~120（descriptor） | ExprProgram（已有） | P0（简单，先跑通模式） |
| **8.2** | Enum / FlagsEnum / Mapping | 3 | ~900 | ~200 | dict C API（已有） | P1 |
| **8.3** | OneOf / NoneOf + Validator | 2 + 1 类 | ~400 | ~150 | AdapterCallbackNode（已有） | P1 |
| **8.4** | Hex / HexDump | 2 | ~500 | ~150（lib/hex.py port） | lib/hex 显示类 | P1 |
| **8.5** ⭐ | Checksum + Rust hashfunc + ParseStream::slice | 1 + 基础设施 | ~700 | ~150（HashAlgo enum） | **新 crate**（sha2/sha1/md-5/crc32fast/adler） | P2（依赖 PM 决策 D-1） |
| **8.6** | Union | 1 | ~500 | ~120 | ParseStream seek（已有） | P1 |
| **8.7** | Sequence | 1 | ~600 | ~100 | StopField（已有） | P1 |
| **8.8** | AlignedStruct + Aligned | 1 + 1 宏 | ~400（Aligned）+ ~80（宏） | ~150 | StructNode（已有） | P2（依赖 PM 决策 D-2） |
| **8.9** | Terminated / Probe | 2 | ~300 | ~80 | ParseStream（已有） | P0 |
| **8.10** | CancelParsing | 0（Error 变体） | ~100 | ~80 | ConstructError + schema.rs | P2（触碰 parse 入口，需全量回归） |
| **8.11** | NamedTuple / Timestamp | 1 + 1 宏 | ~400（NamedTuple） | ~200（Timestamp macro） | namedtuple/arrow | P2（依赖 PM 决策 D-3） |
| **8.12** | ProcessXor / ProcessRotateLeft | 2 | ~700 | ~100 | TransformNode 模式（参考） | P1 |

### 3.2 建议实施顺序（基于依赖 + 难度）

```
批次 A（P0，跑通模式，低风险）：
  8.1 (Const/Default/Check) → 8.9 (Terminated/Probe)
  ↓
批次 B（P1，核心 Adapter，中等难度，可并行）：
  8.2 (Enum/FlagsEnum/Mapping) | 8.3 (OneOf/NoneOf/Validator) | 8.4 (Hex/HexDump)
  8.6 (Union) | 8.7 (Sequence) | 8.12 (ProcessXor/ProcessRotateLeft)
  ↓
批次 C（P2，需 PM 决策或高风险，串行）：
  8.5 (Checksum，待 D-1) → 8.8 (AlignedStruct，待 D-2)
  8.10 (CancelParsing，触碰 parse 入口)
  8.11 (NamedTuple/Timestamp，待 D-3)
```

### 3.3 可合并建议（PM 裁量）

- **8.2 + 8.3**：均为"映射/验证 Adapter"，共享 dict/set C API 模式。合并为"8.2 [Enum/FlagsEnum/Mapping/OneOf/NoneOf + Validator]"（5 Node + 1 类），但 DEV 工作量较大（~1300 Rust 行）。
- **8.4 + 8.11 NamedTuple**：Hex/HexDump 与 NamedTuple 共享"类型分派 + Python factory 调用"模式。但 NamedTuple 另涉 Timestamp（Python 层），不建议合并。
- **8.7 + 8.6**：Sequence 与 Union 都是"多字段复合 Node"，但输出 sink 不同（list vs dict）+ Union 多视角 seek，不建议合并。

---

## 4. Checksum Rust hashfunc 方案确认（§4 详述）

### 4.1 方案确认（基于 `质疑-能否不用RawCopy.md §7`）

**ARCH 正式确认**：Checksum 采用 **StreamRange + Rust 内置 hashfunc + ParseStream::slice** 零拷贝方案。L-14 教训已触发，§7 修正了原"拷贝不可避免"的盲点。

### 4.2 ChecksumNode 设计骨架（待 8.5 子任务正式设计时细化）

```rust
enum HashFunc {
    /// Rust 内置哈希（零拷贝，操作 &[u8]）
    BuiltIn(BuiltinHash),
    /// Python callable（兼容模式，跨 FFI）
    PythonCallable(PyObject),
}

enum BuiltinHash {
    Md5, Sha1, Sha256, Sha512,  // sha2 + sha1 + md-5 crate
    Crc32, Adler32,              // crc32fast + adler crate
}

enum BytesSource {
    /// 原版模式：从 context 求值 bytesfunc（需 RawCopy 把 bytes 塞进 context）
    ContextBytes(ExprProgram),
    /// 扩展模式：从 stream 范围 [start, end) 直接切片（construct-rs 扩展）
    StreamRange { start: ExprProgram, end: ExprProgram },
}

struct ChecksumNode {
    checksumfield: Box<Node>,
    hashfunc: HashFunc,
    bytes_source: BytesSource,
}
```

### 4.3 零拷贝路径（Rust 内置 hashfunc + StreamRange）

1. `hash1 = checksumfield.parse(stream, ctx, path)` → PyBytes（必要返回值）
2. `start = eval(start_expr, ctx)` / `end = eval(end_expr, ctx)` → usize
3. `data_slice: &[u8] = stream.slice(start, end)?` → **借用切片，零拷贝**（新增 `ParseStream::slice`）
4. `digest = match hashfunc { BuiltIn(Sha256) => Sha256::digest(data_slice), ... }` → **Rust 内计算**
5. `if hash1.as_bytes() != digest.as_slice() { ChecksumError }`
6. 返回 hash1

**全程零拷贝**：FFI 入口零拷贝（`PyBytes::as_bytes` 借用，§7.1 证据）+ StreamRange 借用切片 + Rust 内哈希。

### 4.4 parity 影响

- **双轨 API**：
  - Python callable + ContextBytes（原版 `Checksum(Bytes(64), lambda d: hashlib.sha512(d).digest(), this.fields.data)`）→ 1 次拷贝（CPython 硬约束，仅此路径）
  - Rust 内置 HashAlgo + StreamRange（construct-rs 扩展）→ **0 次拷贝**
- 现有用户代码无需改写（默认 Python callable 路径保持 parity）。

### 4.5 性能预测（L-02 对策：待 8.5 实测）

- Rust 内置 sha256 vs Python hashlib.sha256（1KB 数据）：节省 ~30-40%（消除 N 字节 PyBytes 拷贝 + FFI 回调 + RawCopy 5 键 dict）
- 预测 Checksum parse ≥1.3x vs Python callable 路径（8.5 bench 实测验证）

### 4.6 §0 合规

| §0 原则 | 合规性 |
|---------|--------|
| #1 一次 FFI | ✅ Rust 内置路径哈希全在 Rust 内，不跨 FFI |
| #2 无中间表示层 | ✅ 无 Rust→Python 中间数据结构（`&[u8]` 是借用，非中间类型） |
| #3 输入输出无 trait 抽象 | ✅ |
| #4 pyo3 核心依赖 | ✅ sha2/crc32 是 Rust 内部计算库，与 `half` crate 同先例（§7.6） |

---

## 5. 新依赖清单

### 5.1 Rust crate（8.5 Checksum 用）

| crate | 版本建议 | 用途 | 成熟度 | 许可证 | §0 #4 合规 |
|-------|---------|------|-------|--------|-----------|
| `sha2` | 0.10 | SHA-224/256/384/512 | RustCrypto 维护，广泛使用 | MIT/Apache-2.0 | ✅（与 `half` 同性质，纯 Rust 计算） |
| `sha1` | 0.10 | SHA-1 | RustCrypto 维护 | MIT/Apache-2.0 | ✅ |
| `md-5` | 0.10 | MD5 | RustCrypto 维护 | MIT/Apache-2.0 | ✅ |
| `crc32fast` | 1.4 | CRC32（zlib.crc32 等价） | 成熟，广泛使用 | MIT/Apache-2.0 OR Zlib | ✅ |
| `adler` | 1.0 | Adler32（zlib.adler32 等价） | RustCrypto 维护 | MIT/Apache-2.0 | ✅ |

**均为纯 Rust 计算库**，不跨 FFI，与 §0 #4（pyo3 核心依赖）不冲突——`Cargo.toml` 已有 `half = "2.4"` 先例（Phase 6.1 Float16）。

### 5.2 新 API（8.5 用）

| API | 位置 | 用途 |
|-----|------|------|
| `ParseStream::slice(start, end) -> Option<&[u8]>` | `stream.rs` | 借用底层缓冲切片（零拷贝，§7.2 推荐） |

实现：`self.data.get(start..end)`（trivial，~10 行 + 边界校验 + doc）。

### 5.3 Python 依赖（8.11 Timestamp 用，用户侧）

| 包 | 用途 | 性质 |
|----|------|------|
| `arrow` | Timestamp 的 datetime 表示 | **用户依赖**（construct-rs 不打包，用户 `pip install arrow`）；Python construct 原版同样依赖 |

### 5.4 不需要的依赖（澄清）

- **无 num-bigint**（`设计质疑-BytesInteger-num-bigint.md §2.6` 已拒绝）
- **无新表达式系统扩展**（Enum 不用 keyfunc，Timestamp/Validator 走 Python 层）
- **无新 trait 抽象**（所有内置 Adapter Node 用 `Box<Node>` + 编译期物化 `Py<PyAny>` 容器）

---

## 6. PM 决策点

### D-1：Checksum Rust hashfunc 方案 + 新 crate 依赖（8.5 启动门槛）

**问题**：是否接受 §4 方案（Rust 内置 hashfunc + StreamRange + ParseStream::slice）+ §5.1 的 5 个新 crate？

**ARCH 推荐**：**接受**。理由：
1. L-14 教训已确认零拷贝路径成立（`质疑-能否不用RawCopy.md §7`）；
2. 5 crate 均纯 Rust、成熟、与 `half` 同性质（§0 #4 合规）；
3. parity 双轨（Python callable 兼容 + Rust 内置扩展），不破坏现有 API；
4. 性能预期 ≥1.3x（待实测）。

**若不接受**：Checksum 降为"仅 Python callable + ContextBytes"路径（无 StreamRange），性能 ~1-2x（RawCopy dict 包装未消除），且 RawCopy 仍是 Checksum 场景的强制依赖。

### D-2：Aligned 是否纳入 8.8 范围（AlignedStruct 前置）

**问题**：Aligned（不在原 22 清单）是 AlignedStruct 的必要前置。是否将其作为 8.8 子任务的隐含范围？

**ARCH 推荐**：**是**。Aligned 是简单 Subconstruct（padding 模式，参考 PaddingNode），与 AlignedStruct 一并实现成本最低（共享 padding 逻辑）。单独实现 AlignedStruct 而无 Aligned 需在 AlignedStructNode 内联 padding，重复代码。

**若不接受**：8.8 仅实现 AlignedStructNode（内联 padding），Aligned 标 wont_implement（用户无法单独用 `Aligned(modulus, subcon)`）。

### D-3：Timestamp 是否纳入 Phase 8（arrow 依赖）

**问题**：Timestamp 依赖第三方 `arrow` 包，且只能 Python 层实现（性能不设硬门禁）。是否纳入 Phase 8，或标 wont_implement？

**ARCH 推荐**：**纳入 Phase 8，但标注"Python 层，性能不设硬门禁"**。理由：
1. Timestamp 是协议常用（时间戳字段），用户迁移需求高；
2. arrow 是 Python construct 原版依赖，迁移用户已装；
3. Python 层实现成本低（macro + Adapter 子类，~200 Python 行）。

**若不接受**：Timestamp 标 wont_implement，用户自行用 Computed + Python datetime 库替代。

### D-4：Validator 是否提供 ExprValidator 等价（表达式版快捷构造）

**问题**：Python `ExprValidator(subcon, validator_lambda)` 接受 lambda。construct-rs 表达式系统不接 lambda（ADR-006）。是否提供"表达式版 Validator 快捷构造"（如 `Validator(subcon, expr)` 走 ExprProgram 求值 bool）？

**ARCH 推荐**：**Phase 8 不提供**。理由：
1. 用户继承 Validator 类是主路径（Python 层）；
2. 表达式版 Validator 需要"表达式求值结果作 bool 校验"——与 Check 重叠（Check 就是表达式版断言）；
3. 复杂度增量不值（用户要表达式校验用 Check，要 Python 逻辑校验继承 Validator）。

**若提供**：新增 `ExprValidatorDescriptor` 走 Rust Node（类似 CheckNode 但返回 obj 而非 None），~150 Rust 行。

### D-5：Union 的 parsefrom 是否支持表达式（非 lambda）

**问题**：Python Union parsefrom 可为 None/int/str/**context lambda**。construct-rs 不接 lambda。是否支持"表达式版 parsefrom"（ExprProgram 求值得 int index）？

**ARCH 推荐**：**支持 ExprProgram（int 求值）**，不支持 lambda。parsefrom ∈ {None | usize(常量) | Py<PyString>(常量名) | ExprProgram(求值 → usize index)}。覆盖绝大多数场景（lambda parsefrom 极罕见，parity 已知差异）。

### D-6：Probe 的 `into` 是否支持表达式

**问题**：Python Probe `into` 是 context lambda（打印 context 子项）。construct-rs 是否支持表达式版？

**ARCH 推荐**：**支持 ExprProgram（求值后 repr 打印）**，不支持 lambda。`into: Option<ExprProgram>`。lambda 版 probe 罕见（调试用），表达式版覆盖主场景。

### D-7：CancelParsing 在表达式系统中的可用性

**问题**：construct-rs 表达式不接 lambda（ADR-006），用户无法在 Computed 表达式中 raise CancelParsing。CancelParsing 实际触发场景仅限 Python 层用户代码（Adapter `_decode` 等）。是否仍实现？

**ARCH 推荐**：**实现（parity 完整性）**，但标注"仅 Python 层用户代码可触发"。ConstructError::CancelParsing 变体 + schema.rs 顶层 catch，~100 Rust 行。用户从 Python `raise CancelParsing()` 跨 FFI 触发。

---

## 7. 特别关注项回应（对应 PM 任务要求）

### 7.1 Checksum（已有设计方向确认）

✅ 见 §4。Rust hashfunc 零拷贝方案正式确认，新依赖 5 crate（§5.1），ParseStream::slice 新 API（§5.2）。PM 决策点 D-1。

### 7.2 Enum/FlagsEnum（是否需要表达式系统扩展 keyfunc）

**不需要**。Python Enum/FlagsEnum 用 `**mapping` dict（非 keyfunc）。construct-rs 仅需编译期 mapping 物化为 `Py<PyDict>`。表达式系统无需扩展。详见 §2.1.7 / §2.1.8。

### 7.3 Hex/HexDump（Rust Node 确认）

✅ 确认为 **Rust Node（非 AdapterCallbackNode）**。`质疑-能否不用RawCopy.md §2.2` 已论证。详见 §2.1.12 / §2.1.13。

### 7.4 Union（是否需要 ParseStream tell/seek）

**已有，无需新 API**。ParseStream::tell/seek 在 Phase 4/7 已实现。Union 用 tell（记录 fallback/forward）+ seek（回退/前进）。详见 §2.1.11。

### 7.5 Sequence（是否复用 StructNode 逻辑）

**不复用代码，共享模式**。输出 sink 不同（PyList vs 实例 `__dict__`）。独立 SequenceNode，但共享 context-nesting / StopField / compute_ro_value 模式。详见 §2.2.1。

### 7.6 CancelParsing（与 ExplicitError 的关系）

**完全不同机制**。CancelParsing = 顶层 catch（parse_stream 入口，中止整个 parse 返回 None）；ExplicitError = 字段级穿透（不被 Peek/Select 吞，但正常传播）。详见 §2.4.1。

---

## 8. ARCH 总结回答（对应 PM 任务要求的 4 项）

| PM 要求 | ARCH 回答 |
|---------|----------|
| **(1) 22 构造器的子任务拆分建议** | 12 个子任务（§3.1），按 P0/P1/P2 三批次实施（§3.2）。Node enum 净增 18 变体（39→57）。 |
| **(2) Checksum Rust hashfunc 方案确认** | **确认**（§4）。StreamRange + Rust 内置 hashfunc + ParseStream::slice = 全程零拷贝（L-14 教训触发）。PM 决策点 D-1。 |
| **(3) 新依赖清单** | 5 个 Rust crate（sha2/sha1/md-5/crc32fast/adler，§5.1）+ 1 新 API（ParseStream::slice，§5.2）+ 1 用户侧 Python 包（arrow，§5.3）。无 num-bigint / 无表达式系统扩展 / 无新 trait。 |
| **(4) PM 决策点** | 7 个：D-1（Checksum 方案+依赖）/ D-2（Aligned 纳入 8.8）/ D-3（Timestamp 纳入）/ D-4（ExprValidator）/ D-5（Union parsefrom 表达式）/ D-6（Probe into 表达式）/ D-7（CancelParsing 实现）。详见 §6。 |

---

## 附录 A：参考文件索引

| 文件 | 用途 |
|------|------|
| `construct/construct/core.py` | 22 构造器 Python 源码（行号见各节） |
| `construct/construct/debug.py:6-95` | Probe 源码 |
| `construct/construct/lib/hex.py` | Hex/HexDump 显示类（8.4 需 port） |
| `docs/design/queries/质疑-能否不用RawCopy.md §7-§10` | Checksum Rust hashfunc 零拷贝方案（§4 基线） |
| `docs/design/queries/设计质疑-BytesInteger-num-bigint.md` | u128 fast-path 先例（§0.2 C API 框架参考） |
| `docs/design/模块设计/模块设计-Adapter核心.md` | Phase 6.3 Adapter 双层分离（§1.2 模式参照） |
| `docs/decisions/ADR-022-用户面Adapter-Python层化.md` | 用户面 Adapter Python 层化决策（双层分离依据） |
| `docs/decisions/ADR-006-表达式VM仅i64.md` | 表达式不接 lambda（Enum/Check/Rebuild 同脉络） |
| `docs/decisions/ADR-014-RepeatUntil-终止表达式.md` | 不接 callable 硬约束（Validator/Union parsefrom 同脉络） |
| `harness/experiences.md §L-14` | 设计硬约束认知需交叉验证（Checksum 零拷贝触发） |
| `construct-rs/src/nodes/mod.rs` | Node enum（当前 39 变体，Phase 8 后 57） |
| `construct-rs/src/stream.rs` | ParseStream tell/seek/read（Union/Probe/Checksum 用） |

---

> **报告完成时间**：2026-07-30
> **下一步**：PM 决策 D-1 ~ D-7 → 子任务分派（8.1 起，按 §3.2 批次顺序）→ 各子任务 DESIGNING（ARCH 出模块设计文档）→ DESIGN_REVIEW（REV）→ CODING（DEV）
