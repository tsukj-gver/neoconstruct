---
id: ANALYSIS-phase7-pre
status: active
phase: "7"
task: "Phase 7 启动前置分析报告（Conditional 5 + Streams 3 构造器分析 + 子任务拆分）"
last_updated: 2026-07-30
depends_on: [AGENTS.md, ADR-014, ADR-021, ADR-022, MEMORY.md, harness/experiences.md, plans/phase7-conditional-streams/总纲.md, docs/analysis/分析报告-Phase6启动前置.md, construct-rs/src/nodes/mod.rs, construct-rs/src/compile.rs, construct-rs/src/stream.rs, construct/construct/core.py]
---

# Phase 7 启动前置分析报告

> 本报告由 ARCH 在 DESIGNING 阶段产出，覆盖 Phase 7 全部 8 个构造器（Conditional 5 +
> Streams 3）的逐项分析、子任务拆分建议与 PM 决策点。
>
> **信息源**：Python 原版 `construct/construct/core.py`（Select L3830 / If L3912 /
> IfThenElse L3944 / Switch L4002 / FocusedSeq L3176 / Pointer L4384 / Seek L4594 /
> Prefixed L4862）/ `construct-rs/src/`（Phase 6 完成后 40+ Node 变体 + Descriptor 编译系统 +
> ParseStream/BuildStream）/ ADR-014（RepeatUntil 删 PyCallable）/ ADR-021（utf16/32 raw FFI）/
> ADR-022（用户面 Adapter Python 层化 + ExplicitError 推迟 PE-3）。
>
> **§0 原则对照**贯穿全文。已对照 L-01（中间表示层）/ L-04（跨阶段模式未沉淀）/ L-09（性能测量）。
>
> **路径说明**：本报告写于 `docs/analysis/`（ARCH 权限范围，见 AGENTS.md §2）。

## 摘要

| 维度 | 关键发现 | PM 决策点 |
|------|---------|----------|
| 依赖核查 | Phase 6 完成后 **无跨 Phase 依赖缺失**（Pass/Subconstruct/Rebuild/ExprProgram 均已就绪） | 0 |
| Conditional 5 | If=macro(IfThenElse+Pass)；IfThenElse/Select 直观；**Switch keyfunc 需路线决策**；FocusedSeq 复杂（context nesting） | 2（Switch keyfunc / ExplicitError） |
| Streams 3 | Pointer/Seek 暴露 **Stream 基础设施缺口**：ParseStream.seek 不支持 whence=1/2；BuildStream 完全无 seek | 1（Stream 扩展范围） |
| 子任务拆分 | 推荐 **7.1 Conditional（5）+ 7.2 Streams（3）+ Stream 基础设施扩展**，2 个子任务 | — |

---

## §A 依赖核查（Phase 6 完成后状态）

### A.1 核查方法

对照 `docs/analysis/分析报告-Phase6启动前置.md §A.2` 的 Phase 7 依赖矩阵，逐一确认
Phase 6 ACCEPTED 后各依赖项的落地状态。

### A.2 依赖矩阵（Phase 6 完成后复核）

| Phase 7 构造器 | 依赖项 | Phase 6 落地状态 | 结论 |
|---------------|--------|----------------|------|
| If / IfThenElse | Pass（else 分支）/ ExprProgram（condfunc） | ✅ PassNode（6.3）/ ✅ ExprProgram（Phase 2） | **无缺失** |
| Switch | Pass（default）/ ExprProgram 或 FieldRef（keyfunc） | ✅ PassNode / ✅ ExprProgram + ctx 字段读取 | **无缺失**（keyfunc 路线见 §C.1） |
| Select | stream_tell/seek（已有）/ ExplicitError（PE-3 推迟项） | ✅ ParseStream.tell/seek / ⚠️ ExplicitError 未引入（见 §C.2） | **1 项待决策** |
| FocusedSeq | context nesting（StructNode 机制）/ Rebuild（非聚焦字段）/ Computed | ✅ StructNode context（Phase 1）/ ✅ RebuildNode（6.3）/ ✅ ComputedNode（Phase 1） | **无缺失** |
| Pointer | Subconstruct（6.3）/ ParseStream.seek whence=0,1,2 / **BuildStream.seek** | ✅ Subconstruct / ⚠️ ParseStream 仅 whence=0 / ❌ BuildStream 无 seek | **Stream 缺口**（见 §C.3） |
| Prefixed | Subconstruct / lengthfield（任意 Node）/ 子流（ParseStream over slice） | ✅ 全部就绪（PrefixedArrayNode 已验证子流模式） | **无缺失** |
| Seek | stream_seek（ParseStream + BuildStream）/ ExprProgram（at/whence） | ⚠️ 同 Pointer 的 Stream 缺口 | **Stream 缺口**（同上） |

### A.3 结论

**无跨 Phase 依赖缺失**。Phase 7 启动不需要从 Phase 8+ 调入任何构造器。

**两项 Phase 7 内部的基础设施工作**（非跨 Phase 依赖，属本 Phase 范围）：
1. **Stream whence 扩展 + BuildStream.seek**（Pointer/Seek 必需，见 §C.3）
2. **ExplicitError 决策**（Select 必需，见 §C.2；ADR-022 PE-3 已预告推迟到 Phase 7）

---

## §B 构造器逐项分析

### B.1 Conditional 5 个

#### B.1.1 If(condfunc, subcon) + IfThenElse(condfunc, then, else)

**Python 原版实现摘要**（core.py L3912-3999）：

- `If` 是 **macro**：`If(condfunc, subcon) = IfThenElse(condfunc, subcon, Pass)`（L3935）。
  Python 侧无独立类，直接返回 IfThenElse 实例。
- `IfThenElse` 是 `Construct` 直接子类：
  - `_parse`：`condfunc = evaluate(self.condfunc, context)` → 选 then/else subcon → `sc._parsereport(...)`。
  - `_build`：同上，选 subcon 后 `sc._build(obj, ...)`。
  - `_sizeof`：同上，选 subcon 后 `sc._sizeof(...)`。
- `condfunc` 类型：`bool 或 context lambda（或 truthy value）`。`evaluate()` 把 bool 常量原样返回，
  把 `this.x > 0` 等 FieldRef/ExprRef 在 context 上求值。

**现有架构适配方案**：

- **IfThenElse 新增 `IfThenElseNode`**：
  ```rust
  pub struct IfThenElseNode {
      cond: Condition,              // 复用 StopIfCondition 模式：Always/Never/Expr(ExprProgram)
      then_sub: Box<Node>,
      else_sub: Box<Node>,
  }
  ```
  - `Condition` 分类与 `StopIfCondition`（compile.rs L61 `StopIfCondition`）同模式：
    - Python bool 常量 → `Condition::Always` / `Condition::Never`
    - FieldRef/ExprRef → `Condition::Expr(ExprProgram)`，从 `expr_programs[field_index]["cond"]` 取
      （与 StopIfDescriptor 的 "cond" 键、ComputedDescriptor 的 "func" 键同模式）
  - parse：求值 cond → 选 then/else → 委托 subcon.parse。`eval_expr_int` 返回 i64（0=false，非零=true）。
  - build：对称。
  - sizeof：选 subcon 后 sizeof（与 Python 一致；若 cond 无法编译期求值则返回 Err，与 Python
    `SizeofError` 一致）。

- **If 不新增 Rust Node**：Python 侧 macro 即可（`construct.If = lambda cond, sub: IfThenElse(cond, sub, Pass)`），
  与 Python 原版完全一致。Rust 侧零改动。

**§0 合规性**：
- #1 一次 FFI：✅ cond 走 ExprProgram（零 FFI），subcon 是 Box<Node>（Rust 内部）
- #2 无中间表示层：✅ subcon.parse 直接产 PyObject
- #3 无 I/O trait 抽象层：✅ 仅持 Box<Node>
- #6 enum_dispatch：✅ IfThenElseNode 加入 Node enum

**依赖关系**：
- 前置：Pass（✅ 6.3）/ ExprProgram（✅ Phase 2）/ StopIfCondition 模式（✅ Phase 4，可复用或提取公共 enum）
- 被依赖：用户协议条件字段（常见）

**边界条件**：
- cond 为编译期 bool 常量 → 走 Always/Never fast-path（不创建 ExprProgram）
- cond 表达式求值失败（字段缺失）→ 返回 `ConstructError::ExprFieldMissing`（与 Computed 同路径）
- then/else subcon 的 sizeof 不一致 → sizeof 返回 Err（动态依赖 cond，无法编译期确定）

**工作量**：低（~150 行 Rust + Python macro）。复用 StopIf 的 Condition 模式。

---

#### B.1.2 Switch(keyfunc, cases, default=Pass)

**Python 原版实现摘要**（core.py L4002-4076）：

- `Construct` 直接子类。`__init__`：`default=None` 时设为 `Pass`（L4032-4033）。
- `_parse`：`keyfunc = evaluate(self.keyfunc, context)` → `sc = self.cases.get(keyfunc, self.default)` →
  `sc._parsereport(...)`。
- `_build` / `_sizeof`：同模式。
- **keyfunc 返回值是 dict key**，可为 int / str / bytes / tuple 等任意 hashable。
- cases 是 Python dict，`cases.get(key, default)` 用 `__eq__` + `__hash__` 匹配。

**关键设计难点（PM 决策点 §C.1）**：

keyfunc 的返回值类型决定实现路线。Python `evaluate(keyfunc, context)` 可返回任意 Python 对象。
Phase 2 ExprProgram 当前 `eval_expr_int` 仅返回 i64。三条候选路线：

| 路线 | keyfunc 形态 | cases 匹配方式 | FFI | §0 合规 | 用户面兼容 |
|------|-------------|---------------|-----|---------|-----------|
| **A（ExprProgram i64）** | `this.n`（int 字段）/ int 常量 | i64 == 比较（Rust 内） | 零 | ✅ | 仅 int key |
| **B（FieldRef + PyObject 匹配）** | `this.fieldname`（任意类型字段） | 从 ctx 读 PyObject → cases 用 PyObject `__eq__`（每比较 1 FFI） | N 次（N=cases 数） | ⚠️ 边界 | int/str/bytes key |
| **C（PyCallable 慢路径）** | 任意 lambda | Rust→Python 回调 keyfunc（1 FFI）→ cases.get | 1+ 次 | ❌ 违反 ADR-014 精神 | 完全兼容 |

**ARCH 推荐：路线 A + B 混合（拒绝 C）**，理由：
1. **ADR-014/022 脉络**：RepeatUntil v5 删 PyCallable 谓词、AdapterCallbackNode 拒绝任意 callable
   ——构造器内部决策不背 PyCallable 包袱。Switch keyfunc 是构造器内部决策路径，同理拒绝 C。
2. **路线 A 覆盖 int key 主流场景**（`Switch(this.type, {1: Int8ub, 2: Int16ub})` 是最常见用法），
   ExprProgram 返回 i64 + Rust 内 i64 == 比较，零 FFI，性能最优。
3. **路线 B 扩展 str/bytes key**：当 keyfunc 是纯 FieldRef（`this.tag` 其中 tag 是 str/bytes 字段），
   编译期识别为 FieldRef，运行时从 ctx 读 PyObject，cases 预编译为 `Vec<(Py<PyAny>, Node)>`，
   匹配时调 `PyObject.__eq__`（每比较 1 FFI，N=cases 数）。cases 通常 ≤5 个，FFI 开销可接受。
4. **复杂表达式（`this.x + 1`、`this.tag.lower()`）编译期拒绝**，引导用户用 Computed 字段预计算
   （`"key" / Computed(this.x + 1)` 后 `Switch(this.key, ...)`）。与 ADR-014 强制表达式的硬约束同脉络。

**Adapter 方案（路线 A+B）**：
```rust
pub struct SwitchNode {
    key: SwitchKey,                 // Const(i64) / IntExpr(ExprProgram) / FieldRef(FieldName)
    cases: Vec<SwitchCase>,         // 编译期从 Python dict 展开
    default: Box<Node>,             // 默认 PassNode；用户传 Error 则装 ErrorNode（Phase 7+）
}
pub enum SwitchKey {
    ConstInt(i64),
    IntExpr(ExprProgram),
    FieldRef(FieldName),            // 运行时从 ctx 读 PyObject
}
pub struct SwitchCase {
    key_py: Py<PyAny>,              // Python 侧 key（int/str/bytes），用于 PyObject __eq__
    key_int: Option<i64>,           // 若 key 是 int，额外存 i64 用于零 FFI 快速匹配
    subcon: Box<Node>,
}
```
- parse：求 key → 先尝试 i64 快速匹配（key_int）→ 未命中则 PyObject `__eq__` 遍历 → 未命中走 default。
- build：对称。

**§0 合规性**：
- #1 一次 FFI：路线 A 零 FFI；路线 B 每次 PyObject `__eq__` 1 FFI（cases 数决定，用户主动选 str/bytes key）
- #2 无中间表示层：✅ subcon 直接产 PyObject
- ADR-014 合规：✅ 拒绝任意 callable

**依赖关系**：
- 前置：Pass（✅）/ ExprProgram（✅）/ ctx 字段读取（✅ Phase 2）
- 被依赖：用户协议多路分支（常见，如 TLV 解析）

**边界条件**：
- cases 为空 + default=Pass → 总走 Pass（等价 Optional 的反面）
- keyfunc 求值字段缺失 → `ExprFieldMissing`
- cases key 类型与 keyfunc 返回类型不匹配 → 全部 __eq__ 失败 → 走 default
- Python `default=Error` 构造器（core.py 未定义 Error 类，是用户传入抛错构造器）→ Phase 7 暂不支持
  Error 构造器（未实现），default 仅接 Pass 或已实现 Node；文档化为已知限制

**工作量**：中（~250 行 Rust）。路线 A+B 合一的匹配逻辑是主要复杂度。

---

#### B.1.3 Select(*subcons)

**Python 原版实现摘要**（core.py L3830-3884）：

- `Construct` 直接子类。
- `_parse`：遍历 subcons，`fallback = stream_tell`，`try: sc._parsereport` `except ExplicitError: raise`
  `except Exception: stream_seek(fallback, 0)`，成功则 return obj。全部失败 `raise SelectError`。
- `_build`：遍历 subcons，`try: data = sc.build(obj)` `except ExplicitError: raise`
  `except Exception: pass`，成功则 `stream_write(data)` 并 return。全部失败 `raise SelectError`。
- **关键**：`except ExplicitError: raise` —— ExplicitError 不被吞掉，直接向上传播。

**现有架构适配方案**：

- **新增 `SelectNode { subcons: Vec<Node> }`**：
  - parse：遍历 subcons，`fallback = stream.tell()`，try `sub.parse(...)`：
    - Ok(obj) → return obj
    - Err(ConstructError::Explicit) → **直接 return Err**（不吞）
    - Err(其他) → `stream.seek(fallback)` 继续
  - 全部失败 → `ConstructError::Select`（新增变体）
  - build：遍历 subcons，try 在临时 BuildStream 上 build，成功则 write 到主 stream：
    - 临时 BuildStream 模式（与 PrefixedArray build 同：subcon.build 到 temp → write bytes）

**§0 合规性**：
- #1 一次 FFI：✅ subcons 都是 Box<Node>，Rust 内部尝试
- #2 无中间表示层：✅ build 临时 BuildStream 是输出缓冲本身（非中间数据类型）
- #3 无 I/O trait 抽象层：✅

**依赖关系**：
- 前置：ParseStream.tell/seek（✅）/ **ExplicitError 变体（⚠️ 未引入，见 §C.2）**
- 被依赖：Union（Phase 8+，Select 是 Union 的基础）/ Optional（macro = Select(subcon, Pass)）

**边界条件**：
- subcons 为空 → 立即 SelectError
- 第一个 subcon 成功 → 后续不尝试（短路）
- ExplicitError 必须穿透（Python 语义）—— 需 §C.2 决策引入 `ConstructError::Explicit`
- build 方向：Python 用 `sc.build(obj)` 在新 BytesIO 尝试；construct-rs build 无返回值，
  需在临时 BuildStream 上 build 后比对成功（无异常即成功）

**工作量**：低-中（~200 行 Rust + ExplicitError 变体引入）。

---

#### B.1.4 FocusedSeq(parsebuildfrom, *subcons)

**Python 原版实现摘要**（core.py L3176-3318）：

- `Construct` 直接子类。"允许构造比 Adapter 更精细的适配器"。
- `_parse`：**context nesting**（新 Container，`_` 指向外层 context），`parsebuildfrom = evaluate(...)`，
  遍历 subcons 依次 parse，命名字段塞入新 context，记录 `sc.name == parsebuildfrom` 的返回值，
  最终返回该聚焦字段值。
- `_build`：context nesting，`context[parsebuildfrom] = obj`，遍历 subcons 依次 build
  （聚焦字段传 obj，其余传 None），命名字段用 buildret 替换 context。
- `_sizeof`：context nesting，`sum(sc._sizeof for sc in subcons)`。
- **内部用于实现 PrefixedArray**（L4956：`PrefixedArray = FocusedSeq("items", "count"/Rebuild(...), "items"/subcon[count])`）。
  construct-rs 的 PrefixedArrayNode（Phase 4）已独立实现，不依赖 FocusedSeq。

**现有架构适配方案**：

FocusedSeq 本质是"返回单字段的 Struct + context nesting"。两条路线：

- **路线 A（复用 StructNode 机制）**：`FocusedSeqNode { fields: Vec<StructField>, focus_index: usize }`，
  内部委托 StructNode 的 context nesting + 字段遍历逻辑，parse/build 结束后只返回 focus 字段值。
  - 优点：最大化复用 StructNode 的 context nesting（`_` 外层引用、字段间引用）
  - 缺点：StructNode.parse 返回的是实例（Container），FocusedSeq 要返回单字段值——需在 StructNode
    内部或 FocusedSeqNode wrapper 层提取 focus 字段
- **路线 B（独立 Node，简化 context nesting）**：`FocusedSeqNode` 自行管理 context nesting
  （push 新 scope，遍历字段，pop scope，返回 focus 值）
  - 优点：不耦合 StructNode 内部
  - 缺点：context nesting 逻辑重复

**ARCH 推荐：路线 A（复用 StructNode 机制）**，但需在模块设计阶段确认 StructNode 是否暴露
"返回单字段"的钩子。若 StructNode 紧耦合"返回实例"，则用路线 B。

**§0 合规性**：
- #1 一次 FFI：✅ context nesting 是纯 Rust（Context 结构），字段都是 Box<Node>
- #2 无中间表示层：✅ 聚焦字段值就是 subcon.parse 产出的 PyObject
- #3 无 I/O trait 抽象层：✅

**依赖关系**：
- 前置：context nesting（✅ StructNode Phase 1）/ Rebuild（✅ 6.3，非聚焦字段常用 Rebuild）/ Computed（✅）
- 被依赖：用户面"精细适配器"（少见，PrefixedArray 已独立实现）

**边界条件**：
- parsebuildfrom 不匹配任何字段名 → `UnboundLocalError`（Python）/ `ConstructError::Generic`（Rust）
- 非聚焦字段 build 时传 None → 该字段的 subcon 必须支持 build(None)（如 Rebuild/Computed/Const）
- context nesting 的 `_` 外层引用 —— 与 StructNode 嵌套语义一致

**工作量**：中-高（~300 行 Rust）。context nesting 复用是关键设计点。

**实施优先级**：**P2**（用户面频率低，PrefixedArray 已独立实现不依赖）。可在 7.1 末尾或独立 7.1b。

---

### B.2 Streams 3 个

#### B.2.1 Seek(at, whence=0)

**Python 原版实现摘要**（core.py L4594-4643）：

- `Construct` 直接子类。`flagbuildnone=True`。
- `_parse`：`at = evaluate(self.at, context)` / `whence = evaluate(self.whence, context)` →
  `return stream_seek(stream, at, whence, path)`。
- `_build`：同 parse（seek 是副作用，build 也执行）。
- `_sizeof`：`raise SizeofError`（size 无意义）。
- whence 语义：0=begin / 1=relative / 2=from EOF。

**现有架构适配方案**：

- **新增 `SeekNode { at: SeekTarget, whence: Whence }`**：
  ```rust
  pub enum SeekTarget {
      Const(i64),
      Expr(ExprProgram),            // this.offset 等
  }
  pub enum Whence { Set, Cur, End } // 对应 0/1/2
  ```
  - parse：`stream.seek(at, whence)`（需 ParseStream 扩展 whence 支持，见 §C.3）
  - build：`stream.seek(at, whence)`（需 BuildStream 新增 seek 方法，见 §C.3）
  - sizeof：总是 Err
  - 返回值：Python 返回 `stream_seek` 返回值（新位置）；construct-rs parse 返回 PyLong(新位置)

**§0 合规性**：
- #1/#2/#3：✅ 全 Rust 内部 stream 操作
- #8 Stream 抽象纯 Rust 内部：✅（本构造器正是 Stream 操作的体现）

**依赖关系**：
- 前置：**ParseStream.seek whence 扩展 + BuildStream.seek**（⚠️ 见 §C.3）
- 被依赖：Pointer（共用 seek 基础设施）/ 用户面流定位（少见，通常在 Sequence 内）

**边界条件**：
- at 为负 + whence=0 → Python `stream_seek` 行为依实现（BytesIO 视为 0）；Rust 应返回 Stream Err
- at 超出流范围 → ParseStream 已有 bounds 检查（L148-158）；BuildStream 需定义扩展行为（零填充）
- whence=2（EOF）+ at 负 → 从末尾向前定位

**工作量**：低（~100 行 Rust），但**依赖 Stream 基础设施扩展**（§C.3，~150 行）。

---

#### B.2.2 Pointer(offset, subcon, stream=None, relativeOffset=False)

**Python 原版实现摘要**（core.py L4384-4483）：

- `Subconstruct` 子类。
- `_pointer_seek`：`offset = evaluate(self.offset, context)` / `stream = evaluate(self.stream, context) or stream` /
  `fallback = stream_tell` → 若 relativeOffset: `stream_seek(stream, offset, 1)`（相对）
  否则 `stream_seek(stream, offset, 2 if offset < 0 else 0)`（负 offset 从 EOF，正 offset 从 begin）。
- `_parse`：`fallback = _pointer_seek` → `obj = subcon._parsereport` → `stream_seek(fallback, 0)` 回退。
- `_build`：对称（seek → subcon.build → seek 回 fallback）。
- `_sizeof`：`return 0`（Pointer 不占主流位置）。
- `stream` 参数允许换流（context lambda 提供不同流）—— **construct-rs 单一流模型不支持换流**，
  文档化为已知限制（与 Python 主流用法不冲突，换流极少用）。

**现有架构适配方案**：

- **新增 `PointerNode { offset: PointerOffset, subcon: Box<Node> }`**：
  ```rust
  pub enum PointerOffset {
      Const(i64),                   // 编译期常量
      Expr(ExprProgram),            // this.offset 等
  }
  // relativeOffset 编译期 bool 标志
  ```
  - parse：求 offset → 记 fallback → seek(offset, whence) → subcon.parse → seek(fallback, Set)
  - build：求 offset → 记 fallback → seek(offset, whence) → subcon.build → seek(fallback, Set)
  - sizeof：返回 0

**§0 合规性**：
- #1/#2/#3：✅ 全 Rust 内部
- #8：✅ Stream 抽象纯 Rust

**依赖关系**：
- 前置：Subconstruct（✅ 6.3）/ **ParseStream.seek whence=1/2 + BuildStream.seek**（⚠️ §C.3）
- 被依赖：用户面绝对偏移字段（常见，如文件头指针）

**边界条件**：
- offset 为负 → whence=2（从 EOF）—— **需 §C.3 ParseStream.seek 支持 whence=2**
- relativeOffset=True → whence=1（相对当前）—— **需 §C.3 ParseStream.seek 支持 whence=1**
- build 方向 seek 到超出当前末尾 → BuildStream 需零填充扩展（§C.3）
- subcon.parse/build 失败 → 仍需 seek 回 fallback（Python 语义，用 finally；Rust 需显式处理 Err 路径）

**工作量**：中（~200 行 Rust），依赖 Stream 基础设施扩展（§C.3）。

---

#### B.2.3 Prefixed(lengthfield, subcon, includelength=False)

**Python 原版实现摘要**（core.py L4862-4931）：

- `Subconstruct` 子类。
- `_parse`：`length = lengthfield._parsereport` → 若 includelength: `length -= lengthfield._sizeof` →
  `substream = BytesIOWithOffsets.from_reading(stream, length)` → `return subcon._parsereport(substream, ...)`。
- `_build`：`stream2 = io.BytesIO()` → `subcon._build(obj, stream2)` → `data = stream2.getvalue()` →
  `length = len(data)`（若 includelength 加 lengthfield sizeof）→ `lengthfield._build(length, stream)` →
  `stream_write(stream, data)`。
- `_sizeof`：`lengthfield._sizeof + subcon._sizeof`。

**现有架构适配方案**：

- **新增 `PrefixedNode { lengthfield: Box<Node>, subcon: Box<Node>, includelength: bool }`**：
  - parse：lengthfield.parse → 得 length（PyLong → i64）→ 若 includelength 减 lengthfield.sizeof →
    `stream.read(length)` 取子切片 → `ParseStream::new(slice)` → subcon.parse(子流)
  - build：创建 temp BuildStream → subcon.build(temp) → `data = temp.into_bytes()` →
    lengthfield.build(len) → 主 stream.write(data)
  - sizeof：lengthfield.sizeof + subcon.sizeof

**关键**：**子流模式已在 PrefixedArrayNode（Phase 4）验证**（PrefixedArray build 也是先 build 元素到
临时再写 count + data）。Prefixed 是其"单元素"版本，模式更简单。

**§0 合规性**：
- #1/#2/#3：✅ 子流是 ParseStream over slice（纯 Rust），temp BuildStream 是输出缓冲本身
- #8：✅ Stream 抽象纯 Rust

**依赖关系**：
- 前置：Subconstruct（✅）/ 任意 lengthfield Node（✅）/ 任意 subcon Node（✅）/ 子流模式（✅ PrefixedArray 验证）
- 被依赖：用户面长度前缀字段（常见，如 TLS 记录）

**边界条件**：
- length 为负 → Stream Err（与 read 一致）
- length 超过剩余字节 → Stream Err（read 已有 bounds 检查）
- includelength=True 且 lengthfield.sizeof 为 Err → sizeof 返回 Err
- subcon 在子流上 parse 未消费全部字节 → Python 行为是"忽略剩余"；Rust 应一致（不报错）

**工作量**：中（~200 行 Rust）。复用 PrefixedArray 的子流 + temp build 模式。

---

## §C 跨构造器决策点

### C.1 Switch keyfunc 路线（PM 决策点 1）

详见 §B.1.2。三路线对比：

| 路线 | ARCH 推荐 | 理由 |
|------|----------|------|
| A（ExprProgram i64）+ B（FieldRef PyObject）混合 | ✅ **推荐** | 覆盖 int/str/bytes key 主流；零 FFI（int）/ 少量 FFI（str/bytes，cases 数决定）；合规 ADR-014 |
| C（PyCallable 慢路径） | ❌ 不推荐 | 违反 ADR-014/022 脉络（构造器内部决策不背 PyCallable）；RepeatUntil v5 已证明 PyCallable 性能不达标 |

**ARCH 推荐 A+B 混合，拒绝 C**。复杂表达式（非纯 FieldRef）编译期拒绝，引导用户用 Computed 预计算。

**PM 决策**：接受 A+B / 接受 C（全兼容但性能折衷）/ 其他。

### C.2 ExplicitError 引入（PM 决策点 2）

**背景**：ADR-022 PE-3 把 ExplicitError 推迟到 Phase 7 Select 评估。Python `Select._parse`/
`_build` 明确 `except ExplicitError: raise`（不吞）。

**当前状态**：`ConstructError`（error.rs）无 Explicit 变体。Peek（6.3）的 `is_explicit_error`
占位返回 false（所有错误都被吞）。

**选项**：
- **选项 A（引入 `ConstructError::Explicit` 变体）**：Select 与 Python 完全 parity；Peek 也可受益
  （显式错误不被吞）。需新增变体 + 修改 Peek 的错误分类。
- **选项 B（Phase 7 Select 不区分 ExplicitError，所有错误都吞）**：偏离 Python 语义，文档化为
  已知差异。用户用 ExplicitError 的场景极少（Python 文档显示它是高级用法）。

**ARCH 推荐：选项 A（引入 Explicit 变体）**。理由：
1. ADR-022 已预告 Phase 7 引入，设计连贯
2. Python parity 是本项目核心目标（交付 Python 包）
3. 实现成本低（新增 enum 变体 + 错误分类逻辑微调）

**PM 决策**：接受 A / 接受 B（简化）/ 其他。

### C.3 Stream 基础设施扩展（PM 决策点 3）

**背景**：Pointer/Seek 暴露两项 Stream 缺口：
1. **ParseStream.seek 仅支持 whence=0**（L148），不支持 whence=1（相对）/ whence=2（从 EOF）。
   Pointer 的负 offset（从 EOF）和 relativeOffset=True（相对）需要 whence=1/2。
2. **BuildStream 完全无 seek 方法**。Pointer build 需要 seek 到 offset 写 subcon 再 seek 回；
   Seek build 直接执行流定位。BuildStream 是 Vec<u8>，seek 需定义：
   - 向后 seek（pos < len）→ 覆盖写
   - 向前 seek 超出末尾（pos > len）→ 零填充扩展
   - whence=1/2 → 相对/从末尾计算

**扩展范围**：
- `ParseStream::seek_whence(&mut self, at: i64, whence: Whence, path) -> Result<()>`
  （保留现有 `seek(pos, path)` 兼容 whence=0；新增支持 whence=1/2 的方法）
- `BuildStream::seek(&mut self, at: i64, whence: Whence) -> Result<(), ConstructError>`
  （新增，含零填充扩展逻辑）

**§0 合规**：✅ Stream 是纯 Rust 内部抽象（§0 #8），扩展不跨 FFI。

**ARCH 推荐：在 7.2 Streams 子任务内先行扩展 Stream 基础设施，再实现 Seek/Pointer/Prefixed**。
扩展属 7.2 范围，不单列子任务（~150 行 Rust，与 Seek 实现紧密耦合）。

**PM 决策**：确认 Stream 扩展纳入 7.2 范围 / 拆为独立 7.0b 基础设施子任务 / 其他。

---

## §D 子任务拆分建议

**ARCH 推荐 2 个子任务**（与 PM 任务书建议一致）：

| 子任务 | 范围 | 估计工作量 | 风险 | 依赖决策 |
|-------|------|----------|------|---------|
| **7.1 Conditional**（5 构造器） | IfThenElse（含 If macro）/ Switch / Select / FocusedSeq + ExplicitError 变体引入 | 4-6 天 | 中（Switch keyfunc + FocusedSeq context nesting） | §C.1（Switch）/ §C.2（ExplicitError） |
| **7.2 Streams**（3 构造器 + Stream 扩展） | Stream whence 扩展 + BuildStream.seek / Seek / Pointer / Prefixed | 3-5 天 | 中（BuildStream seek 零填充 + Pointer whence 语义） | §C.3（Stream 扩展范围） |

**子任务依赖图**：
```
7.1 Conditional ──┐
                  │  （7.1 与 7.2 可并行，无相互依赖）
7.2 Streams ──────┘
```

**为何 7.1/7.2 可并行**：
- Conditional 不依赖 Streams（IfThenElse/Switch/Select/FocusedSeq 不操作流位置，Select 用 tell/seek
  是 whence=0 已有能力）
- Streams 不依赖 Conditional（Seek/Pointer/Prefixed 无条件分支）

**7.1 内部建议顺序**（DEV 实施参考）：
1. ExplicitError 变体引入（基础设施，Select 依赖）
2. IfThenElse（最简，复用 StopIf 的 Condition 模式）+ If macro
3. Switch（路线 A+B，依赖 §C.1 决策）
4. Select（依赖 ExplicitError）
5. FocusedSeq（最复杂，context nesting，可推迟到 7.1b）

**7.2 内部建议顺序**：
1. Stream 基础设施扩展（ParseStream whence + BuildStream.seek）
2. Seek（最简，直接用扩展后的 seek）
3. Pointer（offset + whence + subcon）
4. Prefixed（复用 PrefixedArray 子流模式）

---

## §E PM 决策点汇总

| # | 决策 | ARCH 推荐 | 影响范围 | 优先级 |
|---|------|----------|---------|--------|
| **1** | **Switch keyfunc 路线**：A+B 混合（ExprProgram i64 + FieldRef PyObject，拒绝 PyCallable）vs C（全 PyCallable 兼容） | A+B 混合 | 7.1 Switch 实现 + 用户面兼容性 | **P0**（7.1 Switch 启动前） |
| **2** | **ExplicitError 引入**：选项 A（引入 ConstructError::Explicit 变体，Select/Peek parity）vs B（不引入，Select 吞所有错误） | 选项 A | 7.1 Select + error.rs + Peek 行为 | **P0**（7.1 Select 启动前） |
| **3** | **Stream 基础设施扩展范围**：纳入 7.2 内（~150 行）vs 拆独立 7.0b 子任务 | 纳入 7.2 | 7.2 工作量估算 + 子任务数 | **P1**（7.2 启动前） |

> **额外提示**（不需 PM 决策但需知会）：
>
> - **Pointer `stream` 参数（换流）不支持**：Python 允许 `stream=context lambda` 提供不同流，
>   construct-rs 单一流模型不支持。文档化为已知限制（换流极少用，主流用法是默认流）。
> - **Switch `default=Error` 不支持**：Python 的 `Error` 是抛错构造器（core.py 未定义类），
>   Phase 7 default 仅接 Pass 或已实现 Node。文档化已知限制。
> - **FocusedSeq 实施优先级 P2**：用户面频率低（PrefixedArray 已独立实现），可在 7.1 末尾或
>   独立 7.1b。PM 可决定是否纳入 Phase 7 范围或推迟到 Phase 8+。
> - **If 是 Python macro**：Rust 侧不新增 IfNode，Python 侧 `construct.If = lambda cond, sub: IfThenElse(cond, sub, Pass)`。
>   与 Python 原版完全一致（L3935）。

---

## 附录 A：Python 原版源码定位

| Phase 7 构造器 | core.py 行号 | 基类 | 备注 |
|---------------|-------------|------|------|
| If | 3912-3941 | （macro） | `IfThenElse(condfunc, subcon, Pass)` |
| IfThenElse | 3944-3999 | Construct | evaluate(condfunc) 选 then/else |
| Switch | 4002-4076 | Construct | evaluate(keyfunc) → cases.get(key, default=Pass) |
| Select | 3830-3884 | Construct | 遍历尝试，失败回退流，ExplicitError 穿透 |
| FocusedSeq | 3176-3318 | Construct | context nesting，返回聚焦字段；PrefixedArray 内部用 |
| Pointer | 4384-4483 | Subconstruct | seek 到 offset 处理 subcon 再 seek 回；size=0 |
| Prefixed | 4862-4931 | Subconstruct | lengthfield + 子流；build 用 temp BytesIO |
| Seek | 4594-4643 | Construct | 纯 stream_seek；size=SizeofError |

## 附录 B：现有 Node 系统清单（Phase 6 完成后）

**已实现 40 个 Node 变体**（`construct-rs/src/nodes/mod.rs`，Phase 6 后）：

- 原子节点（4）：FormatField / Bytes / GreedyBytes / BitsInteger
- 复合节点（5）：Struct / StructRef / Bitwise / Bytewise / Transform
- RO 节点（2）：Tell / Computed
- 填充节点（2）：BitPadding / Padding
- Array 系列（5）：Array / GreedyRange / PrefixedArray / RepeatUntil / Index
- 控制流（2）：StopIf / Element
- Phase 6.1 Primitives（3）：VarInt / ZigZag / BytesInteger
- Phase 6.2 Strings（6）：CString / GreedyString / PaddedString / PascalString / NullTerminated / NullStripped
- Phase 6.3 Adapter（6）：Subconstruct / Peek / RawCopy / Rebuild / Pass / AdapterCallback

**Phase 7 预计新增 Node**：

| 子任务 | 新增 Node | 数量 |
|-------|----------|------|
| 7.1 | IfThenElse / Switch / Select / FocusedSeq | 4 |
| 7.2 | Seek / Pointer / Prefixed | 3 |
| **合计** | | **7 个新 Node**（40 → 47） |

**Stream 基础设施扩展**（非新 Node，是 ParseStream/BuildStream 方法扩展）：
- `ParseStream::seek_whence(at: i64, whence: Whence, path)` —— 支持 whence=1/2
- `BuildStream::seek(at: i64, whence: Whence)` —— 新增（含零填充扩展）

完成后构造器清单 `docs/constructors-inventory.csv` 总进度预计：
- 当前 ~70/134（~53%，MEMORY.md Phase 6 完成后口径）
- Phase 7 完成：~70 + 7 + If macro（Python 层，不计 Rust Node） = ~77 个构造器有 Rust 实现
- 进度：~58%

## 附录 C：§0 原则对照总览（L-01 对策）

| §0 原则 | 7.1 Conditional | 7.2 Streams |
|---------|----------------|-------------|
| #1 一次 FFI | ✅ IfThenElse/Switch(路线A)/Select/FocusedSeq 走 ExprProgram 或 Box<Node>；Switch 路线 B 的 PyObject `__eq__` 是用户主动选 str/bytes key 的少量 FFI | ✅ Pointer/Seek/Prefixed 全 Rust 内部 stream 操作 |
| #2 无中间表示层 | ✅ subcon 直接产 PyObject；Select temp BuildStream 是输出缓冲本身 | ✅ Prefixed 子流是 ParseStream over slice；temp BuildStream 同 |
| #3 输入输出侧无 trait 抽象层 | ✅ 全部持 Box<Node>，不引入新 trait | ✅ 同左 |
| #4 pyo3 核心依赖 | ✅ | ✅ |
| #5 mashumaro 式 API | N/A（不涉及用户面 dataclass） | N/A |
| #6 enum_dispatch 静态分派 | ✅ 7 个新 Node 加入 Node enum | ✅ 同左 |
| #7 Result<T, ConstructError> | ✅ 新增 Select 变体 + Explicit 变体（§C.2） | ✅ Stream Err 复用现有变体 |
| #8 Stream 抽象纯 Rust 内部 | N/A | ✅ 本 Phase 正是 Stream 操作；扩展不跨 FFI |

**结论**：Phase 7 设计方案整体符合 §0 原则。唯一需 PM 拍板的是 Switch keyfunc 的 FFI 取舍
（路线 A+B 零/少 FFI vs 路线 C 全 PyCallable）与 ExplicitError 引入范围。

---

> **报告完成时间**：2026-07-30
> **下一步**：PM 决策 3 项（§E）→ 7.1 / 7.2 并行启动 ARCH 模块设计
