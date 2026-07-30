---
id: ADR-022
title: 用户面 Adapter Python 层化 + 最小 Rust 钩子（AdapterCallbackNode）
status: accepted
phase: "6"
decides: "内置 Adapter（Subconstruct/Peek/RawCopy/Rebuild）= Rust Node（零 FFI）；用户面 Adapter（Adapter/SymmetricAdapter）= Python 层；AdapterCallbackNode = 嵌入 Struct 时的最小 Rust 钩子"
supersedes: []
superseded_by: ""
depends_on: [ADR-014, ADR-004, ADR-012]
last_updated: 2026-07-30
---

# ADR-022: 用户面 Adapter Python 层化 + 最小 Rust 钩子（AdapterCallbackNode）

## Context

Phase 6.3 Adapter 核心需要处理 Python construct 的 Adapter 家族（core.py L787-846）：

| 构造器 | Python 性质 | 用户用法 |
|--------|------------|---------|
| `Subconstruct(subcon)` | 抽象基类（Adapter/RawCopy/Peek/Rebuild/Tunnel 父类） | 用户极少直接用；内部包装 |
| `RawCopy(subcon)` / `Peek(subcon)` / `Rebuild(subcon, func)` | 具体子类，语义明确 | 用户用具体名（如 `Peek(Int8ub)`） |
| `Adapter(subcon)` | **用户继承基类**，实现 `_decode`/`_encode` | `class HexAdapter(Adapter): def _decode(...): ...` |
| `SymmetricAdapter(subcon)` | 对称基类（`_encode = _decode`） | 同上 |

### 问题：用户面 Adapter 的 `_decode`/`_encode` 是 Python 代码

内置 Adapter（Subconstruct/RawCopy/Peek/Rebuild）语义固定，可编译为 Rust Node（零 FFI）。
但通用 `Adapter` / `SymmetricAdapter` 的 `_decode`/`_encode` 是**用户写的 Python 方法**，
Rust 无法在编译期将其编译为 Node——这是用户域代码，不是构造器内部逻辑。

**核心设计问题**：用户面 Adapter 嵌入 Struct 字段时（`field(HexAdapter(Int8ub))`），
Rust StructNode 遍历到该字段时如何处理？两条路线：

- **路线 A（Rust Node 变体承载用户回调）**：新增 `AdapterCallbackNode { subcon, decode, encode }`，
  Rust 端执行 subcon parse，再通过 `Py<PyAny>` 引用回调用户 `_decode`
- **路线 B（完全禁止嵌入 Struct）**：用户必须在 Struct 外层包装（`adapter.parse(struct.build(data))`），
  Rust 完全不背 Adapter 包袱

### PM 决策 2：双层分离约束（用户强化，2026-07-29）

用户在 ARCH 分析报告后补充两点关键约束，PM 接受并强化：

> **约束 1（类型层面分离）**：内置 Adapter 和用户面 Adapter **从类型上就是两种不同的 Rust
> 构造器**，不是一个类型内部两条路径。各自独立的 Node 变体（或 Python 类），不共享内部实现。

> **约束 2（用户面 Adapter 在 Python 层）**：用户面 Adapter（通用 Adapter / SymmetricAdapter 基类）
> **基本在 Python 层实现**——用户继承写 parse/build Python 方法，Rust 层不专门做 Node 变体
> （最多提供最小通用钩子）。Rust 不背"通用用户回调"的包袱。

本 ADR 沉淀这两条约束的工程化落地，并选择路线 A（AdapterCallbackNode 最小钩子）。

## Decision

### 决策 1：双层分离（类型层面，约束 1 落实）

内置 Adapter 与用户面 Adapter **不共享内部实现**，各自独立的 Rust 类型 / Python 类型：

| 层 | 实现位置 | Rust Node 变体 | FFI 次数（每次 parse/build） | 性能目标 | 用户主动选择 |
|----|---------|--------------|----------------------------|---------|------------|
| **内置 Adapter**（Subconstruct/RawCopy/Peek/Rebuild/Pass） | Rust Node 变体 | `SubconstructNode` / `PeekNode` / `RawCopyNode` / `RebuildNode` / `PassNode` | **1 次**（仅 Struct 顶层 parse 入口，内部零 FFI） | ≥10x（与现有 Node 同量级） | 用户用具体名（如 `Peek(Int8ub)`） |
| **用户面 Adapter**（Adapter/SymmetricAdapter 基类） | Python 类（用户继承） | **无独立 Node 变体**（仅 `AdapterCallbackNode` 最小钩子，见决策 3） | **2 次**（parse 入口 + `_decode` 回调，见 §FFI 分析） | 用户主动接受折衷（不设硬门禁） | 用户继承 `Adapter` 写 `_decode`/`_encode` |

**关键**：不存在"一个 Rust 类型内部两条路径"。用户面 Adapter 不实现为通用 Rust Adapter Node；
内置 Adapter 也不暴露 `_decode`/`_encode` 用户钩子。两类从类型层面就是不同构造器。

### 决策 2：用户面 Adapter 基本在 Python 层（约束 2 落实）

通用 `Adapter` / `SymmetricAdapter` 基类**完全在 Python 层实现**（设计 §4.1）：

```python
class Adapter:
    def __init__(self, subcon): ...
    def parse(self, data, **kw):
        obj = self.subcon._parse(stream, ctx, path)  # 触发 Rust FFI（1 次）
        return self._decode(obj, ctx, path)           # Python 层解码
    def build(self, obj, **kw):
        obj2 = self._encode(obj, ctx, path)           # Python 层编码
        self.subcon._build(obj2, stream, ctx, path)   # 触发 Rust FFI（1 次）
        return stream.getvalue()
    def _decode(self, obj, context, path): raise NotImplementedError
    def _encode(self, obj, context, path): raise NotImplementedError

class SymmetricAdapter(Adapter):
    def _encode(self, obj, context, path): return self._decode(obj, context, path)
```

**Rust 层不背"通用用户回调"包袱**：Rust 不解析用户 `_decode`/`_encode` 的内部逻辑，不引入
"通用 callable 字段"机制（与 Phase 4 RepeatUntil v5 删 PyCallable 谓词同一脉络，见 ADR-014）。

### 决策 3：AdapterCallbackNode 最小 Rust 钩子（PM 决策 2 允许的范围）

PM 决策 2 约束 2 原文允许"最多提供最小通用钩子（如 StructMixin 的某个机制让 Adapter 包装的
子构造器仍走 Rust parse/build）"。本 ADR 选择路线 A：新增 `AdapterCallbackNode`，**仅在 Adapter
嵌入 Struct 字段时使用**。

```rust
/// Adapter 回调节点：嵌入 Struct 字段时，subcon 在 Rust 执行，adapter 的
/// _decode/_encode 通过 Py<PyAny> 引用在 Rust→Python 回调中执行。
///
/// **仅用于 Adapter/SymmetricAdapter 嵌入 Struct 场景**。用户单独使用 Adapter
/// （如 HexAdapter(Int8ub).parse(b)）不经过此节点（直接走 Python 层 Adapter.parse）。
#[derive(Debug)]
pub struct AdapterCallbackNode {
    subcon: Box<Node>,           // Rust 端执行的 subcon
    decode: Py<PyAny>,           // 用户 _decode 方法引用（bound method）
    encode: Py<PyAny>,           // 用户 _encode 方法引用（bound method）
    adapter_instance: Py<PyAny>, // self 引用（_decode/_encode 是 bound method）
}
```

**parse 路径**（嵌入 Struct 第 i 字段）：
1. Rust StructNode 遍历到 AdapterCallbackNode 字段
2. `subcon.parse(...)` → Rust 内执行（零额外 FFI）
3. `decode.call(py, (obj, ctx, path))` → **Rust→Python 回调**（1 次额外 FFI）
4. 回调返回值作为字段值

**build 路径**：对称（`encode.call` 回调 + `subcon.build`）。

**为什么这是"最小"钩子，不是"通用用户回调包袱"**：
1. **不接收任意 callable**：编译期 type check 仅识别 `Adapter` 子类实例，用户不能传 lambda 当字段
   （与 RepeatUntil v5 同硬约束，见 ADR-014）
2. **不引入新表达式机制**：`_decode`/`_encode` 是用户 Python 方法，Rust 不解析其内部逻辑，只调
   `decode.call(py, (obj, ctx, path))`
3. **不破坏 §0**：用户主动继承 Adapter = 已接受 2 次 FFI（见下文 §0 合规论证）

### §0 #1 合规论证（PM 决策点的核心）

**§0 #1 原文**："编译 / parse / build 各只有一次 Python↔Rust 边界穿越，Rust 内部通过 CPython C API
直接操作 Python 对象"。

**用户面 Adapter 嵌入 Struct 有 2 次 FFI**（parse 入口 + `_decode` 回调），是否违反 §0 #1？

**ARCH 论证：不违反，4 点理由**（设计 §4.4）：

1. **§0 #1 的精确语义**针对的是**编译期生成的执行树**——执行树一旦编译完成，运行时不应每次字段
   都跨越 FFI。这条原则禁止的是"中间表示层"（Rust 端临时数据类型再转换）。
2. **用户面 Adapter 不在执行树内**：Adapter 的 `_decode`/`_encode` 是**用户 Python 代码**，无法被
   Rust 编译为 Node。它的本质是"用户在 Python 层包了一层"。Rust 不能"穿透"到用户的 Python 类里
   去执行用户写的代码——这不是 §0 #1 要禁止的"中间表示层"，而是用户主动添加的"用户域后处理"。
3. **PM 决策 2 已明确**：用户主动选 Adapter = 显式接受性能折衷（与 Phase 4 RepeatUntil v5 删
   PyCallable 兜底同一脉络，见 ADR-014）。用户面 Adapter 是用户的**显式选择**，不是构造器的
   "内部决策路径"。
4. **类比**：Python 用户也可以自己写 `parsed = MyStruct.parse(data); parsed.value = hex(parsed.value)`
   ——这是 100% Python 后处理。Adapter 只是把这个后处理"包装"成了构造器形式，**不改变 FFI 次数本质**
   （仍是 Rust parse 1 次 + Python 后处理 N 次）。

**§0 #2（无中间表示层）合规**：`_decode`/`_encode` 的输入输出是 Python 对象——但这些 Python 对象
**就是最终用户看到的对象**，不是"中间表示"（中间表示指 Rust 端的临时数据类型再转换）。用户面
Adapter 的"中间态"在用户域，不在 Rust 域。

**§0 #3（输入输出无 trait 抽象层）合规**：AdapterCallbackNode 持有 `Box<Node> + Py<PyAny>`，
不引入新 trait（与 Bitwise/Bytewise/Transform 的 `Box<Node>` 模式同脉络）。

### FFI 边界分析（设计 §4.3）

| 场景 | Rust FFI 次数 | Python 调用 | 总边界穿越 |
|------|-------------|-----------|----------|
| `HexAdapter(Int8ub).parse(b)` 单独使用 | 1（subcon._parse） | 1（_decode） | 2 |
| `HexAdapter(Int8ub)` 嵌入 Struct 第 i 字段 | 1（Struct 顶层 parse，subcon 在 Rust 内执行） | 1（_decode，Python 层后处理） | **2** |
| 普通 `Int8ub` 嵌入 Struct 第 i 字段 | 1（Struct 顶层 parse） | 0 | 1 |

用户面 Adapter 嵌入 Struct 比普通字段多 1 次 Rust→Python 回调（`_decode`/`_encode`）。

### 与已有 ADR 的脉络一致性

| ADR | 决策 | 与本 ADR 的关系 |
|-----|------|---------------|
| **ADR-014**（RepeatUntil 终止表达式） | v5 删 PyCallable 谓词兜底，强制用 Phase 2 表达式 | 同脉络：用户面是"用户主动选择"时允许慢路径；构造器内部决策不背 PyCallable 包袱。Rebuild 的 `func` 也只接 Phase 2 表达式（设计 §1.4.1 RB-5），不接 callable |
| **ADR-004**（三种 field 函数 RW/RO/WO） | Rebuild 必须作为 RO 字段（`_field_kind = "ro"`） | 本 ADR 内置 RebuildNode 集成 FieldMode::Ro（设计 §1.4.1） |
| **ADR-012**（StopField 用 Result 哨兵） | Peek 错误吞掉用 Result 模式 | 本 ADR PeekNode 复用 Result 哨兵模式判断错误分类（设计 §1.2.2） |

## 适用范围

- **内置 Adapter**（Subconstruct/RawCopy/Peek/Rebuild/Pass）：Rust Node 变体，加入 Node enum
  （设计 §1-§2，新增 5 个变体），编译期 `build_node_from_descriptor` 识别（设计 §6.3）
- **用户面 Adapter**（Adapter/SymmetricAdapter）：Python 层基类（`_adapters.py`，设计 §4.1）
- **AdapterCallbackNode**：最小 Rust 钩子，仅嵌入 Struct 时编译期生成（设计 §4.5）

**不触发 §0 八条原则任何一条**（设计 §7 §0 对照表逐条核对）：
- #1 一次 FFI：内置 Adapter 严格 1 次；用户面 Adapter 2 次（合规论证见上，不违反）
- #2 无中间表示层：内置透传 PyObject；用户面操作的是用户域 Python 对象
- #6 enum_dispatch：5 个内置 + 1 个 AdapterCallbackNode 加入 Node enum 静态分派

## 禁止行为

- ❌ 用户面 Adapter 实现为通用 Rust Adapter Node（违反约束 2"Rust 不背通用用户回调包袱"）
- ❌ 内置与用户面 Adapter 共享一个 Rust 类型内部两条路径（违反约束 1 类型层面分离）
- ❌ AdapterCallbackNode 接收任意 callable（必须编译期 type check 仅识别 Adapter 子类，RB-5 同硬约束）
- ❌ 用户面 Adapter 嵌入 Struct 设硬性能门禁（PM 决策 2：用户主动选 = 显式接受折衷，仅功能 parity）
- ❌ RawCopy.build 返回 Container.data（construct-rs build 路径无返回值是 §0 #1 体现，设计 §5.3 RC-build-1）

## Consequences

### 正面

- **用户面 API 兼容**：`field(HexAdapter(Int8ub))` 保持 Python construct 习惯，用户迁移成本低
- **Rust 不背用户回调包袱**：通用 Adapter 在 Python 层，Rust 仅 `AdapterCallbackNode` 最小钩子
  （~250 行，仅嵌入 Struct 时启用）
- **内置 Adapter 零额外 FFI**：Subconstruct/RawCopy/Peek/Rebuild/Pass 是 Rust Node 变体，与现有
  Node 同量级（≥10x，设计 §9.2 预测）
- **类型层面分离**：约束 1 落实——内置和用户面从类型上就是不同构造器，无"一个类型内部两条路径"
  的歧义
- **未来复用**：Phase 6+ 用户面 Adapter 子类（Hex/HexDump/Enum/Validator/Mapping/FlagsEnum）均
  复用 AdapterCallbackNode 机制（设计 §4.5）

### 负面

- **用户面 Adapter 嵌入 Struct 有 2 次 FFI**：比普通字段多 1 次 Rust→Python 回调（`_decode`/`_encode`）。
  性能预测 1.5-3x（subcon Rust 部分 ~10x，但 `_decode` Python 回调拉低整体，设计 §9.2）。**不设硬门禁**
  （PM 决策 2）
- **AdapterCallbackNode 是 Rust→Python 回调**：~200-300ns（Python 函数调用 Phase 4 v5 实测），
  若实测 <1x（比 Python 还慢），说明回调开销过大，需重新评估设计（设计 §9.2 可证伪预测）
- **已知行为差异**（设计 §5.3）：
  - AC-FFI：用户面 Adapter 嵌入 Struct 2 次 FFI（文档化性能折衷，不设硬门禁）
  - RC-build-1：RawCopy.build 不返回 Container.data（construct-rs build 无返回值）
  - PE-3：Peek 不区分 ExplicitError（Phase 6 不引入，Phase 7 Select 再评估）

### 中性

- **ExplicitError 暂不引入**（设计 §5.1 PE-3 + 决策 D-2）：Phase 6 范围内无用户场景需要，`is_explicit_error`
  占位返回 false（所有错误都被 Peek 吞掉）。Phase 7 Select 实现时再评估引入 `ConstructError::Explicit` 变体

## Alternatives Considered

### 替代方案 1：完全禁止 Adapter 嵌入 Struct（路线 B）

**描述**：用户面 Adapter 仅能单独使用（`HexAdapter(Int8ub).parse(b)`），嵌入 Struct 字段时
编译期拒绝，强制用户在 Struct 外层包装（`adapter.parse(struct.build(data))` 模式）。

**为何不采用**：
1. **用户面 API 严重破坏**：Python construct 习惯 `field(HexAdapter(Int8ub))`，禁止嵌入迫使
   用户重构所有使用 Adapter 字段的 Struct
2. **实际 FFI 次数未减少**：用户必然在 Python 层做 bytes→str 转换，总边界穿越不变（仍是
   Rust parse 1 次 + Python 后处理 N 次）
3. **§0 合规论证成立**（见上文 4 点）：用户主动选 Adapter 不违反 §0 #1，无需禁止

本 ADR 选择路线 A（AdapterCallbackNode 最小钩子），保留用户面兼容。

### 替代方案 2：用户面 Adapter 也实现为通用 Rust Adapter Node

**描述**：新增通用 `UserAdapterNode { subcon, decode_fn, encode_fn }`，接收任意 callable，
Rust 端在编译期/运行期调用用户 Python 函数。

**为何不采用**：
1. **违反约束 2**："Rust 不背通用用户回调包袱"——接收任意 callable 正是 RepeatUntil v5 删除的
   反模式（ADR-014）
2. **§0 #1 合规论证风险**：通用 callable Node 会模糊"执行树内部"与"用户域后处理"的边界，
   打开"每字段都跨 FFI"的口子（L-01 中间表示层违反的变种）

本 ADR 把用户面 Adapter 限制为 Python 层 + 最小钩子（仅识别 Adapter 子类，不接任意 callable）。

## Relations

- **关联 ADR**：`ADR-014 RepeatUntil 终止表达式`（v5 删 PyCallable 谓词兜底）
  - 同脉络：用户面"用户主动选择"时允许慢路径；构造器内部决策不背 PyCallable 包袱
  - 本 ADR 的 RebuildNode（`func` 只接 Phase 2 表达式，不接 callable，设计 §1.4.1 RB-5）与
    AdapterCallbackNode（仅识别 Adapter 子类，不接任意 callable）共同落实 ADR-014 的硬约束
- **关联 ADR**：`ADR-004 三种 field 函数`（Rebuild 必须为 RO 字段）
- **关联 ADR**：`ADR-012 StopField 用 Result 哨兵`（PeekNode 错误吞掉复用 Result 模式）
- **关联教训**：`harness/experiences.md §L-01`（中间表示层违反）
  - 本 ADR 核心合规论证：用户面 Adapter 的 `_decode`/`_encode` 是**用户域后处理**，不是执行树
    内部的"中间表示层"（设计 §4.4 第 2 点）。L-01 禁止的是"Rust 端临时数据类型再转换"，
    不禁止"用户主动添加的 Python 后处理"
  - L-01 对策落实：设计 §7 §0 对照表逐条核对（#1/#2/#3 三条对用户面 Adapter 的合规性专门论证）
- **设计文档**：`docs/design/模块设计/模块设计-Adapter核心.md`（§0 双层分离 + §1 内置 Adapter Rust Node +
  §4 用户面 Adapter Python 层 + §4.3 FFI 边界分析 + §4.4 §0 合规论证 + §4.5 AdapterCallbackNode +
  §7 §0 对照表 + §8 D-1 决策记录）
- **PM 决策**：`plans/phase6-primitives-strings-adapter/总纲.md §PM 决策 2`（双层分离约束，用户强化）+
  `§PM 决策 6.3-D1`（接受 AdapterCallbackNode 选项 A，建议 ADR-022 沉淀）
- **VET 验证**：用户面 Adapter 性能验证**不设硬门禁**（PM 决策 2），仅功能 parity（设计 §11 parity 模板
  `test_user_adapter_in_struct`）。若实测 <1x（比 Python 还慢）需重新评估（设计 §9.2 可证伪）
- **实现位置**：`construct-rs/src/nodes/adapter_callback.rs`（AdapterCallbackNode，设计 §10.1）+
  `construct-rs/python/construct/_adapters.py`（Adapter/SymmetricAdapter 基类，设计 §4.1）
- **首次验证**：Phase 6.3（Adapter 核心，2026-07-30 设计 ACCEPTED，DEV/VET 已完成）
- **未来复用**：Phase 6+ 用户面 Adapter 子类（Hex/HexDump/Enum/Validator/Mapping/FlagsEnum）均复用
  AdapterCallbackNode 机制 + Python 层 Adapter 基类

### ADR 编号说明

本 ADR 占用编号 **022**。Phase 6.3 Adapter 设计（§12）与 Phase 6.2 Strings 设计（v2 §10.1 R3.5）
曾都预留 ADR-021。PM 决策采用**方案 A**：6.2 Strings（先验收）用 ADR-021（unsafe raw FFI），
6.3 Adapter 用 ADR-022（本 ADR，用户面 Adapter Python 层化）。
