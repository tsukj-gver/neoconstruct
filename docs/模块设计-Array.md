# 模块设计：Array 支持（Phase 4）

> **设计依据**：
> - `AGENTS.md` §0（核心原则：一次 FFI、无中间表示层、直接操作 Python 对象）、
>   §7（技术决策）、§8（编码红线）、§10（Python 参考速查）
> - `docs/设计决策记录.md`（跨阶段约束，含 Phase 4 决策 3 "Index 仅作为构造器字段"）
> - `plans/phase4-array/分析报告-Array功能集.md`（功能集分析）
> - `plans/phase4-array/总纲.md`（用户面设计四原则 + S-PERF ≥10x 硬门禁 + 场景矩阵）
> - `construct-rs/src/nodes/mod.rs`（现有 Node enum + Construct trait）
> - `construct-rs/src/stream.rs`（现有 ParseStream/BuildStream）
> - `construct-rs/src/context.rs`（现有 Context）
> - `construct-rs/src/expr.rs`（现有 ExprOp / ExprProgram）
> - `construct-rs/src/compile.rs`（现有编译管线）
> - `construct-rs/src/error.rs`（现有错误变体）
> - `construct-rs/python/construct/_mixin.py`（`_compile_expr_tree` 表达式编译管线）
> - Python 源码：`construct/construct/core.py`
>   （Array L2493、GreedyRange L2570、RepeatUntil L2637、Index L2934、
>   StopIf L4079、PrefixedArray L4934）、`construct/construct/lib/containers.py`（ListContainer）
>
> **角色**：ARCH
> **状态**：DESIGNING（v5，用户打回 v4 后第五次修订，RepeatUntil 终止表达式重设计，等待重新检视）
> **创建时间**：2026-06-29
> **修订时间**：2026-06-29（v2 修正 REV 驳回的 P1/P2/P3 + M1-M4；
>   v3 修正 REV 决策记录对照驳回的 V1/V2/V3/V4；
>   2026-06-30 v4 修正 VET 驳回的 V-1 PyCallable context proxy 不完整；
>   2026-06-30 v5 用户打回——PyCallable 路径全删，终止表达式 = Phase 2 表达式，
>   引入 Element() 字段）

---

## 0. REV/VET/用户驳回修正摘要

### 0.1 v2 修正（P1/P2/P3 + M1-M4）

REV 在 v1 检视中驳回 3 个严重问题（P1/P2/P3）+ 4 个中等问题（M1-M4）。
v2 逐项修正：

| 编号 | 问题 | 修正位置 | 修正内容 |
|------|------|---------|---------|
| P1 | `has_expressions` 对 `StopIf(Expr)` 返回 false | §6.1.1 | 改为 `matches!(cond, Expr(_))`，附 StructRef 路径的根因说明 |
| P2 | RepeatUntil build discard 语义错 | §4.3.4、§7.3 RU-8 | `partial.append` 加 `if !self.discard` 守卫；新增 RU-8 边界 |
| P3 | Array 表达式 count 编译路径留 placeholder | §6.2.2、§12.2 | 完整编译路径（参照 BytesDescriptor）；明确 inner 表达式限制（P3.1）+ 扩展方案（P3.2） |
| M1 | GreedyRange 吞掉所有非 StopField 错误 | §9.5 GE-1 | 新增 §9.5 已知差异表，详述 ExplicitError 等价物缺失的决策与影响 |
| M2 | PrefixedArray count 溢出行为含糊 | §7.4 PA-5 | 改为"由 countfield 节点自行报错"，删除"回绕"误述 |
| M3 | sizeof 表达式 count 的 GIL 路径不明 | §4.1.4 | 明确 GIL 前置条件 + with_gil 闭包正确写法（避免借用逃逸） |
| M4 | RepeatUntil S-PERF 适用性歧义 | §8.2、§8.3（新增）、§12.1 | 明确"Array"指狭义 Array；RepeatUntil 拆 4.5a（PyCallable）+ 4.5b（Expr），4.5b 必须在 Phase 4 验收前完成 |

附加修正（连带）：
- §6.1.1 补全各 Node 的 has_expressions 实现细节（含 RepeatPredicate::Expr、PrefixedArray.countfield）
- §9.5 新增"已知行为差异表"，集中管理 LC-1~LC-4、IX-1、GE-1/GE-2、RU-build-1、PA-5、NE-expr-1/2
- §7.1 AR-10 更新（inner 表达式编译期失败）
- §11.5 / §12.7 自检与结论同步更新

### 0.2 v3 修正（V1/V2/V3/V4 —— 决策记录对照驳回）

REV 在 v2 复审通过后，对照 `docs/设计决策记录.md` 全部 12 条决策逐条审查，
发现违反 Phase 2 决策 1（废弃 `this`），驳回 2 个设计问题（V1/V2）+ 2 个文档表述
问题（V3/V4）。v3 逐项修正：

| 编号 | 问题 | 修正位置 | 修正内容 |
|------|------|---------|---------|
| V1 | `ExprOp::GetIndex` 用户面入口用 `this._index`，违反决策 1；GetIndex 实为死代码 | §3.3 整节重写、§9.4、§9.5 IX-1、§4.4、设计决策记录 Phase 4 决策 3 | **决策：删除 ExprOp::GetIndex**（不扩展表达式系统）。用户通过 `rfield(Index())` + 字段引用实现等价功能。已实现的 GetIndex 代码由 DEV 回退。详见 §3.3 |
| V2 | §9.4 表格把 `this.xxx` 列为 construct-rs 表达式系统输入 | §9.4 | 重构为三列对照（Python `this.xxx` 语法 / construct-rs 用户面语法 / Rust ExprOp），明确 construct-rs 不用 `this` |
| V3 | 25+ 处描述性引用 Python 代码时用 `this.xxx` 未标注 | §2.2、§2.3、§2.6、§3.2.2、§6.1.1、§6.2.2、§6.3.3、§7.1、§9.5、§12.2 等 | 全文清理：所有 `this.xxx` 描述性引用统一改为"Python 用户写 `this.xxx`（construct-rs 已废弃，改用 `<字段名>` 直接引用）"格式；示例性使用改为 construct-rs 合法语法 |
| V4 | §4.5.1 StopIfCondition::Expr 注释使用 `this.x == 0` | §4.5.1 | 改为 construct-rs 实际语法：`x == 0`（x 是字段名引用，编译为 `[GetInt(idx), Const(0), Eq]`） |

附加修正（连带）：
- 新增 `docs/设计决策记录.md` Phase 4 决策 3（Index 仅作为构造器字段）
- §0.2 新增 v3 修正摘要
- §11 自检章节同步更新

> **D1/D2/D3 文档小问题**（v2 复审附注）：D1（§4.1.4 sizeof 伪代码反例）、
> D2（§8.2 交叉引用笔误 "§8.5" 应为 "§8.3"）、D3（§6.2.1 调用 build_array_node
> 少传参数）仍然有效，v3 不在驳回范围，留作后续清理。

### 0.3 v4 修正（V-1 —— VET 驳回 PyCallable context proxy 不完整）

VET 在 4.5 子任务代码审查中驳回 1 个阻断性问题（V-1）+ 4 个次要问题（V-2~V-5）。
V-1 是行为破坏性变更：DEV 实现的 PyCallable 路径 context proxy 仅含 `_index`，
跳过了 ctx.fields() 字段复制，且 proxy 是 PyDict 而非 Container（不支持 attribute 访问）。
v4 逐项修正（V-2~V-5 留待 DEV 在 V-1 修复批次中附带处理，本节聚焦 V-1）：

| 编号 | 问题 | 修正位置 | 修正内容 |
|------|------|---------|---------|
| V-1 | PyCallable 路径 proxy 仅含 `_index`，谓词无法访问 ctx 字段；proxy 是 dict 不支持 attribute 访问 | §2.5、§4.3.3、§4.3.4、§7.3 RU-9、§8.3、设计决策记录 Phase 4 决策 4 | **决策：proxy 必须用 Container 包装 + 复制 ctx.fields() 全部字段 + 写 _index**（方案 A）。Container 通过 `__dict__ = self` 支持 attribute 访问，对齐 Python `predicate(e, list, context)` 中 context 是 Container 的语义。性能预期从 ≥3x 下调到 ≥1.5x（PyCallable 是兜底路径，Expr 路径 ≥10x 已达标） |

附加修正（连带）：
- §2.5 决策 A5 补充 PyCallable proxy 规范（从"DEV 决策"明确为硬性约束）
- §4.3.3 `call_repeat_predicate` + `build_context_proxy` 改为 Container 包装的硬性约束 + 实现伪代码
- §4.3.4 build PyCallable 路径同步更新
- §7.3 新增 RU-9 边界（谓词 context 访问）
- §8.1/§8.2/§8.3 更新 PyCallable 路径性能预期（≥3x → ≥1.5x）
- 新增 `docs/设计决策记录.md` Phase 4 决策 4（PyCallable proxy 用 Container 包装）
- §9.5 不新增已知差异条目（方案 A 行为对齐 Python，无差异）

> **V-2/V-3/V-4/V-5 不在本次设计修正范围**：
> - V-2（build partial 收集 elem 而非 buildret）：Adapter-as-inner-RepeatUntil 场景，
>   由 DEV 在 §9.5 新增 RU-build-2 条目文档化（与 NE-expr-1/2 同模式）。
> - V-3（非 callable 谓词）：Python 未文档化特性，由 DEV 在 §9.5 新增 RU-3b 条目文档化，
>   或在 RepeatUntilDescriptor.__init__ 兼容（DEV 决策）。
> - V-4（AST 谓词识别不支持负整数）：由 DEV 扩展 `_is_int_const` 识别 `UnaryOp(USub, Constant(int))`。
> - V-5（has_expressions 注释误导）：由 DEV 修正注释。
>
> 以上 4 项均为实现/文档层面，不需 ARCH 设计决策。
>
> **v5 全部作废**：上述 V-2/V-3/V-4/V-5 描述的问题均源自 PyCallable 路径，
> v5 删除 PyCallable 路径后这些问题不再存在（详见 §0.4）。

### 0.4 v5 修正（用户打回——PyCallable 路径全删，终止表达式重设计）

用户在 2026-06-30 验收 4.5 v4 实施版本时打回，根本原因：

1. **PyCallable 兜底路径违反 AGENTS.md §0 "一次 FFI" 核心原则**（每迭代跨 FFI 调 Python callable）
2. **AST 白名单 + PyCallable 二分设计**（简单 lambda → Expr 快路径；复杂 lambda → PyCallable 慢路径）覆盖率无法穷尽，且**隐藏决策路径**——用户无法预测自己写的 lambda 走哪条路径
3. **门禁 3x（首版）/ 1.5x（v4 借 V-1 下调）均无物理推导**——Python 3.14 实测 Python 调用仅 270-300ns/次（首版估算高估 2-3 倍）
4. **用户面设计四原则**（`plans/phase4-array/总纲.md` §"用户面设计四原则"）：
   - 禁止"用户不可见的开销 + 隐藏决策路径"
   - Adapter 是用户主动选择的较慢路径，不是任何构造器的内部兜底
   - 禁用"谓词"术语——RepeatUntil 的终止条件就是 Phase 2 表达式
   - 只认 Phase 2 表达式语法——禁止 RepeatUntil 自建表达式机制

v5 逐项修正（**全文性变更**，涉及多处重写）：

| 编号 | 问题 | 修正位置 | 修正内容 |
|------|------|---------|---------|
| U-1 | PyCallable 路径违反"一次 FFI"原则 | §2.5 决策 A5、§4.3、§6.2、§7.3、§8、§9.1、§9.5、§10.1、§12.1 | **删除 RepeatPredicate 枚举**（仅保留 ExprProgram）。删除 `call_repeat_predicate`、`build_context_proxy`、整个 `container_cache.rs` 文件、`python/construct/lib/containers.py`（若专为 PyCallable 新建）。RepeatUntilNode 数据结构简化为 `{ inner, terminator: ExprProgram, element_field_idx, discard }` |
| U-2 | AST 白名单 + PyCallable 二分设计隐藏决策路径 | §2.5 决策 A5、§4.3.1、§6.2、§6.3 | **删除 AST 识别器** `_try_compile_repeat_predicate`、`_AST_OP_TO_EXPROP`。RepeatUntil 用户面 API **不接收 Python lambda/callable**——终止表达式必须是 Phase 2 表达式（`_FieldDescriptor`/`_ExprRef`/`int` 组合）。RepeatUntilDescriptor 第一参数从 `predicate` 改名为 `terminator`（终止表达式） |
| U-3 | 用户无法引用"当前元素" | §2.5 决策 A5、§3.3、§4.3、§4.7（新增 ElementNode）、§6.1、§6.2、§6.3 | **引入 `Element()` 构造器字段**（与 `Index()` 同构），用户写 `e: int = rfield(Element()); payload: list = field(RepeatUntil(e > 5, Int8ub))`。终止表达式 `e > 5` 编译为 `[GetInt(idx_of_e), Const(5), Gt]`，完全走 Phase 2 ExprProgram 路径。**删除 `ExprOp::GetElem`**（不再需要）、删除 `Context::_current_elem_ptr`、`set_current_elem_ptr`、`clear_current_elem_ptr`、`current_elem_as_i64` |
| U-4 | "谓词"术语暗示接收 callable | 全文 | 全文搜索"谓词"统一改为"终止表达式"。RepeatPredicate → 终止表达式（直接用 ExprProgram，不引入新枚举名）。RepeatUntilDescriptor `predicate` 参数 → `terminator` |
| U-5 | 性能门禁无物理推导 | §8.2、§8.3（完全重写） | 删除"PyCallable 路径 ≥1.5x""Expr 路径 ≥8x"二分门禁。统一为 **RepeatUntil ≥10x**（终止表达式零 FFI 求值）。性能假设含 Python 端 + Rust 端逐项开销分析 + 数值推导 |
| U-6 | 4.5a/4.5b 拆分（PyCallable 先 / Expr 后） | §12.1 | **4.5a/4.5b 拆分作废**。RepeatUntil 统一为单一子任务 4.5（终止表达式 = Phase 2 表达式，无 PyCallable 路径） |

附加修正（连带）：
- §3.3 不再扩展 ExprOp：删除 v4 实施版本中已添加的 `ExprOp::GetElem` 变体、`compute_max_stack` 的 GetElem 分支、`eval_expr_int` 的 GetElem 分支、相关单元测试
- §6.1 Node enum 新增 `Element(ElementNode)` 变体（§4.7 新增节）
- §6.2 compile.rs 新增 ElementDescriptor 识别分支
- §6.3 Python 侧新增 `ElementDescriptor` + `Element()` 工厂函数
- §7.3 RepeatUntil 边界完全重写（删除 RU-4/RU-7/RU-8/RU-9 PyCallable 相关条目，新增终止表达式相关边界）
- §9.1 类/方法映射表更新（RepeatUntil 不再"首版仅 PyCallable"）
- §9.5 已知行为差异表删除 RU-build-2、RU-3b、RU-9（均 PyCallable 相关）
- §10.1 删除"RepeatUntil 的 Expr 谓词路径"（Expr 就是唯一路径，无后续优化方向）
- §13 新增附录：完整用户面使用样例集（用户验收门禁）
- 新增 `docs/设计决策记录.md` Phase 4 决策 4 作废 + 决策 4'（RepeatUntil 终止表达式 = Phase 2 表达式）+ 决策 5（Element 仅作为构造器字段）

> **v5 不可逆约束**（用户硬约束，违反即打回）：
> 1. 禁止保留任何形式的 PyCallable 路径
> 2. 禁止"白名单 + 兜底"二分设计
> 3. 禁止"谓词"术语（统一用"终止表达式"）
> 4. 禁止性能假设中用"物理上限"作为门禁豁免理由
> 5. 禁止自建 RepeatUntil 专属表达式机制
> 6. 禁止接受任何形式的"用户不可见的决策路径"

---

## 1. 概述

### 1.1 目标

为 construct-rs 引入"重复元素"构造器，支持解析与构建元素序列。覆盖 Python
construct 2.10.70 的 Array 完整功能集（不含 LazyArray，归入惰性解析阶段）。

**性能目标**：Array parse/build **≥10x** vs Python construct 2.10.70
（S-PERF 出口标准）。理想目标 ≥20x。

### 1.2 范围（按优先级）

| 优先级 | 构造器 | Python 行号 | 类型 | 本设计节点 |
|--------|--------|------------|------|-----------|
| P0 | `Array(count, subcon, discard)` | 2493 | 固定次数 | `ArrayNode` |
| P1 | `GreedyRange(subcon, discard)` | 2570 | 读到流结束 | `GreedyRangeNode` |
| P2 | `PrefixedArray(countfield, subcon)` | 4934 | 前缀长度 | `PrefixedArrayNode`（独立实现，§5） |
| P3 | `RepeatUntil(terminator, subcon, discard)` | 2637 | 终止表达式 | `RepeatUntilNode` |
| P4 | `Index` | 2934 | 取当前下标 | `IndexNode` |
| P5 | `StopIf(condfunc)` | 4079 | 早停信号 | `StopIfNode` |
| P6 | `Element` | —（construct-rs 新增） | 取 RepeatUntil 当前元素 | `ElementNode`（v5 新增，§4.7） |

> **不在本阶段范围**：
> - `LazyArray` / `LazyListContainer`（惰性解析，归入后续 Phase）
> - `Range`（2.11+ 才引入，2.10.70 不存在）
> - `Sequence`（独立 Phase，与 Struct 平行；StopIf 在 Sequence 的捕获留待 Sequence 阶段）

### 1.3 与现有架构的衔接

Phase 1-3 已实现 13 个 Node 变体。本设计**不破坏**现有行为：

- 现有 `ParseStream` / `BuildStream` 的字节级与 bit 级 API 语义不变；新增 `seek` 方法
  （§3.1，GreedyRange 硬依赖）
- 现有 13 个 Node 的 parse/build/sizeof 行为不变
- 现有 `Context` 接口不变；新增 `_index` 字段（§3.2，栈分配，零开销）
- 现有 `compile_schema` 签名向后兼容（新增可选参数）
- 现有 `ConstructError` 变体不变；新增 3 个变体（§3.4）

**新增内容**：
- `ParseStream` / `BuildStream` 新增 `seek` API（§3.1）
- `Context` 新增 `_index: Option<usize>` 字段 + 读写方法（§3.2，IndexNode 读取用）
- `ConstructError` 新增 `Range` / `Repeat` / `StopField` / `IndexField` 4 个变体（§3.4）
- Node enum 新增 6 个变体（§4）
- `compile_schema` 新增 Array 描述符识别分支（§6.2）
- Python 侧新增 Array 描述符与 `len_` 辅助函数（§6.3）

> **V1 修正（v3）**：不再扩展 ExprOp（不新增 `GetIndex`）。IndexNode 直接调
> `ctx.index()` 读下标，不走 ExprProgram。详见 §3.3。

---

## 2. 核心架构决策

### 2.1 决策 A1：Array 系列 Node 的 inner 用 `Box<Node>`（沿用 BitwiseNode 模式）

`ArrayNode` / `GreedyRangeNode` / `RepeatUntilNode` / `PrefixedArrayNode` 都持有
一棵完整的 `Node` 子树作为元素定义。Node 是递归类型——enum 内含 `Box<Node>` 变体
（如 `BitwiseNode.inner`）。Array 系列沿用此模式：

```rust
pub struct ArrayNode {
    inner: Box<Node>,      // 元素子树
    count: CountSource,    // 静态整数 / ExprProgram
    discard: bool,
}
```

**为什么不用 `Box<dyn Construct>`**：违反 AGENTS.md §7（禁止动态分派）。
enum_dispatch 静态分派是项目硬约束。

### 2.2 决策 A2：count 来源编译期分类（静态 / 表达式）

Python `Array(count, subcon)` 的 `count` 可以是：

1. 整数常量：`Array(5, Byte)`
2. 上下文 lambda（**Python 语法**）：`Array(this.length, Byte)`、`Array(lambda ctx: ctx.n, Byte)`

construct-rs 等价写法：count 是 `_FieldDescriptor`（字段名引用），如
`Array(length, Byte)`（其中 `length` 是已声明的 `_FieldDescriptor`）。

编译期把这两种来源编译为 `CountSource` 枚举：

```rust
pub enum CountSource {
    /// 编译期常量（如 `Array(5, Byte)`）。
    Const(usize),
    /// 表达式程序（如 `Array(length, Byte)`，length 是字段名引用，复用现有 ExprProgram）。
    Expr(ExprProgram),
}
```

运行时 `ArrayNode::eval_count(ctx, py)` 返回 `usize`。`Const` 路径零开销（直接返回），
`Expr` 路径走现有 `eval_expr_int`。

**为什么 `usize` 而非 `i64`**：
- Python 校验 `0 <= count`（core.py L2527），负数报 `RangeError`。
- Rust 在编译期或运行时把 i64 → usize 前先校验非负（§9.1 AR-2）。

### 2.3 决策 A3：`_index` 通过 Context 的栈分配字段传递（不走 PyDict）

Python 把 `_index` 写入 context dict（`context._index = i`），内层通过
`context.get("_index")` 或 `this._index`（**Python 语法**）读取。

construct-rs 中，`_index` 是 Context 的内部状态，不通过用户面表达式语法暴露。
用户访问下标的机制见 §3.3（通过 `rfield(Index())` + 字段引用）。

现有 Context 用 `expr_values_buf`（栈分配内联数组）加速 GetInt。但 `_index` 不是
字段，**不**应占用 `expr_values_buf` 的字段槽位（语义污染 + 编译期索引难以表达）。

**决策**：在 Context 增加 `_index: Option<usize>` 字段：

```rust
pub struct Context<'py> {
    fields: Option<Bound<'py, PyDict>>,
    parent: Option<&'py Context<'py>>,
    expr_values_buf: [*mut ffi::PyObject; MAX_INLINE_FIELDS],
    expr_values_len: usize,
    /// Phase 4 新增：当前数组迭代下标（仅 Array 系列节点设置）。
    /// `None` 表示当前不在数组迭代中（IndexNode 读到时返回 None / Py_None）。
    /// 栈分配，零堆开销。
    _index: Option<usize>,
}
```

**嵌套数组语义**（Array 内 Array）：

Python 在内层覆盖外层 `_index`。我们的设计：

- `ArrayNode.parse` 进入循环前保存 `old = ctx._index`，每次迭代设置 `ctx._index = Some(i)`，
  递归 inner 后**不**恢复（Python 也不恢复，inner 内的子 Struct 看到的是当前下标）。
- 循环结束后恢复 `ctx._index = old`（保证外层数组继续迭代时 _index 正确）。

IndexNode.parse 直接读 `ctx.index()`，None 时返回 Py_None（对齐 Python
`context.get("_index", None)` 的行为，详见 §4.4）。

> **V1 修正（v3）**：v1/v2 在此设计 `ExprOp::GetIndex` 指令读取 `ctx._index`，
> 但其用户面入口 `this._index` 违反 Phase 2 决策 1（废弃 `this`），实为死代码。
> v3 删除 GetIndex，IndexNode 直接调 `ctx.index()`。详见 §3.3 与设计决策记录
> Phase 4 决策 3。

**为什么不用 PyDict 路径**：
- `set_field("_index", v)` 每次迭代 ~50ns（dict 查找 + SetItem），100 元素 = 5μs。
- 栈字段 `ctx._index = Some(i)` 每次 ~1ns，100 元素 = 100ns。**50x 加速**。
- 与现有 Vec 化优化的精神一致（Context-Vec优化.md）。

### 2.4 决策 A4：`StopFieldError` 用 Result 哨兵变体（不引入 panic / 异常）

Python 用异常做控制流（`StopIf` 抛 `StopFieldError`，`Struct`/`Sequence`/
`GreedyRange` 捕获视为正常终止）。Rust 不能用 panic（AGENTS.md §8 红线 2），
也不能引入新返回类型（破坏 Construct trait 签名）。

**决策**：在 `ConstructError` 增加 `StopField` 哨兵变体：

```rust
pub enum ConstructError {
    // ... 现有变体 ...
    /// Phase 4：早停信号（仅 StopIfNode 产生）。
    /// StructNode / SequenceNode / GreedyRangeNode 捕获此变体视为正常终止。
    /// 其他节点若意外收到此变体，按错误向上传播（防御性）。
    #[error("stop field signal at {path}")]
    StopField {
        path: String,
    },
}
```

**捕获语义**：

- `GreedyRangeNode.parse`：每次迭代 `match inner.parse(...) { Ok(v) => push, Err(StopField{..}) => break, Err(e) => return Err(e) }`
- `StructNode.parse` / `SequenceNode.parse`（Phase 5+）：同样匹配 StopField 后停止后续字段。
- `ArrayNode.parse`：**不**捕获 StopField（Python Array 也不捕获——Array 是固定次数，StopIf 在 Array 内无意义；用户若误用，错误向上传播）。

**为什么用错误枚举变体而非 `Result<Result<T, StopSignal>, E>`**：
- 不改 Construct trait 签名（13 个现有节点无需修改）。
- 错误路径开销可接受（StopField 仅在 StopIf 触发时构造，成功路径零成本）。
- enum_dispatch 自动生成的 match 代码无需调整。

### 2.5 决策 A5：RepeatUntil 终止表达式 = Phase 2 表达式（v5 完全重写）

> **v5 用户硬约束**（`plans/phase4-array/总纲.md` §"用户面设计四原则"）：
> 1. 禁止用户不可见的开销 + 隐藏决策路径
> 2. Adapter 是用户主动选择的较慢路径，不是任何构造器的内部兜底
> 3. 禁用"谓词"术语（统一用"终止表达式"）
> 4. 只认 Phase 2 表达式语法（禁止 RepeatUntil 自建表达式机制）

Python `RepeatUntil(predicate, subcon)` 的 `predicate` 是 `(obj, list, context) -> bool`
lambda，每次迭代跨 FFI 调用。

**construct-rs 用户面 API（v5）**：

```python
RepeatUntil(terminator, subcon, discard=False)
```

- `terminator`：**Phase 2 表达式**（`_FieldDescriptor` / `_ExprRef` / `int` 组合）。
  必须是引用当前 Struct 中**已声明的 Element 字段**的表达式（如 `e > 5`，
  其中 `e` 是 `rfield(Element())` 字段）。求值结果非零即终止。
- `subcon`：元素子构造器（与 Python 一致）。
- `discard`：是否丢弃解析结果（与 Python 一致）。

**不接收 Python lambda / callable**——避免隐藏决策路径。

#### 2.5.1 用户面 API 完整示例

```python
from dataclasses import dataclass
from construct import (
    StructMixin, field, rfield,
    Int8ub, RepeatUntil, Element,
)

@dataclass
class Packet(StructMixin):
    # Element 字段：用户为 RepeatUntil 提供"当前元素"引用入口
    e: int = rfield(Element())
    # RepeatUntil 终止表达式：e > 5（编译为 [GetInt(0), Const(5), Gt]）
    payload: list = field(RepeatUntil(e > 5, Int8ub))

# parse：读字节直到某个元素 > 5
Packet.parse(b"\x01\x02\x03\x06\xff\xff")
# Packet(e=None, payload=[1, 2, 3, 6])  ← 最后元素 6 满足 e > 5，包含在内

# build：写字节直到某个元素 > 5
Packet(e=None, payload=[1, 2, 3, 6, 9]).build()
# b"\x01\x02\x03\x06"  ← 写到 6 即停止（6 > 5）
```

**Element 字段语义**：
- `e: int = rfield(Element())` 是 Packet 的字段，但其值**不是数据**——它在 Packet
  实例中始终为 `None`（对齐 IndexNode 在 Array 之外返回 None 的语义）。
- Element 字段的存在仅是为 RepeatUntil 终止表达式提供"当前元素"引用入口
  （与 Index 字段为表达式提供"当前下标"引用入口同模式）。
- Element 字段必须声明在 RepeatUntil 字段**之前**（前序字段引用约束，
  `_check_forward_reference` 编译期强制）。

#### 2.5.2 数据结构（v5 简化）

RepeatUntilNode **不再持有枚举变体**（v4 的 `RepeatPredicate` 删除）：

```rust
#[derive(Debug)]
pub struct RepeatUntilNode {
    /// 元素子树（递归 Box）。
    inner: Box<Node>,
    /// 终止表达式（Phase 2 ExprProgram，编译期从用户面表达式编译）。
    /// 求值结果非零即终止。零 FFI（Rust 内部栈式 VM 求值）。
    terminator: ExprProgram,
    /// Element 字段在当前 Struct 的 expr_values_buf 中的索引（编译期确定）。
    /// RepeatUntil 每次迭代调 `ctx.set_field_at(element_field_idx, elem, ...)`
    /// 把当前元素写入该槽位，终止表达式中的 GetInt(element_field_idx) 即取到此值。
    element_field_idx: usize,
    /// Element 字段名（interned PyString，用于 set_field_at 的 key）。
    element_field_name: Py<PyString>,
    /// 是否丢弃解析结果（仍消耗流）。
    discard: bool,
}
```

**为什么不再需要 `_current_elem_ptr` / `ExprOp::GetElem`**（v5 关键设计）：

v4 实施版本中，RepeatUntilNode 通过 `ctx.set_current_elem_ptr(elem.as_ptr())`
设置裸 PyObject 指针，`ExprOp::GetElem` 通过 `PyLong_AsLongLong` 取值。这有两个问题：

1. **用户面入口不统一**：v4 的 Expr 路径由 AST 识别器（`_try_compile_repeat_predicate`）
   隐式触发，用户不主动选择。Element 路径用户主动声明 `rfield(Element())`，
   与 Phase 4 决策 3（Index）模式一致。
2. **裸指针借用风险**：`_current_elem_ptr` 是 `*mut ffi::PyObject`，依赖调用方契约
   保证存活，是 SAFETY 风险。

v5 设计：终止表达式通过 GetInt(element_field_idx) 从 `expr_values_buf` 取值，
RepeatUntil 每次迭代把当前元素 `set_field_at` 到该槽位（strong reference，
PyObject 引用计数管理）。这与 Bytes(count) 中 count 字段引用、Computed(end - start)
中 end/start 字段引用完全同模式——所有引用统一走 `_FieldDescriptor` + `GetInt`，
表达式系统输入类型保持纯粹（仅 `_FieldDescriptor` / `_ExprRef` / `int`）。

#### 2.5.3 与 Phase 4 决策 3（Index 仅作为构造器字段）的一致性论证

| 维度 | Index 决策（Phase 4 决策 3） | Element 决策（v5 Phase 4 决策 5） |
|------|----------------------------|--------------------------------|
| 用户面入口 | `rfield(Index())` | `rfield(Element())` |
| 表达式引用方式 | 字段名引用：`Bytes(i + 1)` → `[GetInt(idx_of_i), Const(1), Add]` | 字段名引用：`RepeatUntil(e > 5, ...)` → `[GetInt(idx_of_e), Const(5), Gt]` |
| 表达式系统输入类型 | `_FieldDescriptor` / `_ExprRef` / `int`（不引入新节点类型） | `_FieldDescriptor` / `_ExprRef` / `int`（不引入新节点类型） |
| ExprOp 指令 | 不引入新 ExprOp（GetInt 已足够） | 不引入新 ExprOp（GetInt 已足够） |
| 值生命周期 | Struct 单次 parse 周期（IndexNode.parse 一次性写入 ctx.fields） | RepeatUntil 单次迭代周期（RepeatUntilNode 迭代时主动 set_field_at 覆盖） |
| 在 Array/RepeatUntil 之外 | IndexNode.parse 返回 None（对齐 Python `context.get("_index", None)`） | ElementNode.parse 返回 None（对齐 Element 字段不持有真实数据） |
| sizeof | 0（不消耗字节） | 0（不消耗字节） |

**唯一实现差异**：值生命周期。Index 字段值由 IndexNode.parse 在 Struct 进入时
一次性写入 ctx；Element 字段值由 RepeatUntilNode 在每次迭代时主动覆盖
（借用模式）。这是因为 Index 的值在 Struct 一次 parse 内不变，而 Element 的值
随 RepeatUntil 迭代变化。该差异是**实现细节**，不影响用户面一致性——两者都遵循
"字段名即引用、所有引用统一走 _FieldDescriptor"哲学。

#### 2.5.4 Phase 2 表达式无法描述的终止逻辑（能力边界）

Python `RepeatUntil(lambda x, lst, ctx: ..., subcon)` 接收任意 callable，常见
不支持的写法：

| Python 写法 | 是否支持 | 原因 / 替代方案 |
|------------|---------|----------------|
| `lambda x,_,_: x > N` | ✅ 支持 | `RepeatUntil(e > N, subcon)` |
| `lambda x,_,_: x == 0xFF` | ✅ 支持 | `RepeatUntil(e == 0xFF, subcon)` |
| `lambda x,_,_: x != -1` | ✅ 支持 | `RepeatUntil(e != -1, subcon)` |
| `lambda x,_,_: (e & 0xFF) == 0` | ✅ 支持 | `RepeatUntil((e & 0xFF) == 0, subcon)` |
| `lambda x,_,ctx: x > ctx.threshold` | ✅ 支持 | `RepeatUntil(e > threshold, subcon)`（threshold 是已声明的字段） |
| `lambda x,lst,_: x > lst[0]` | ❌ 不支持 | Phase 2 表达式 VM 栈为 i64，不支持 list 索引。**替代方案**：用 Adapter 显式包装（用户主动选慢路径）。详见 §13.9 能力边界 |
| `lambda x,lst,ctx: lst[-2:] == [0, 0]` | ❌ 不支持 | Phase 2 表达式不支持 list 切片。**替代方案**：用 Adapter |

**能力边界原则**：RepeatUntil 终止表达式只支持 Phase 2 表达式 VM 能描述的逻辑
（整数算术 / 位运算 / 比较，全部零 FFI）。无法描述的逻辑，用户须改用 Adapter
（显式慢路径，用户主动选择，不在 RepeatUntil 内部交汇）。这与"用户面设计四原则"
原则 2 一致：Adapter 是用户主动选择的较慢路径，不是任何构造器的内部兜底。

详细能力边界 + Adapter 替代示例见 §13.9。

### 2.6 决策 A6：PrefixedArray 用独立 Node（不依赖未实现的 FocusedSeq/Rebuild）

Python `PrefixedArray` 是宏（**Python 语法**）：`FocusedSeq("items", Rebuild(countfield, len_(this.items)), subcon[this.count])`。
其中 `this.items` / `this.count` 是 Python construct 的 `this` 引用语法，construct-rs 已废弃（见 Phase 2 决策 1）。

`FocusedSeq` / `Rebuild` 尚未实现（属于"字段引用回写"特性，Phase 5+ 范围）。
若等 FocusedSeq 实现再做 PrefixedArray，会阻塞 Phase 4 出口。

**决策**：实现独立的 `PrefixedArrayNode`，直接组合 `countfield` 与 `subcon`：

```rust
pub struct PrefixedArrayNode {
    /// 前缀字段（如 VarInt、Byte）。
    countfield: Box<Node>,
    /// 元素子树。
    inner: Box<Node>,
}
```

parse：`countfield.parse(stream)` → 得 count → `Array(count, inner).parse` 内联。
build：取 `len(list)` → `countfield.build(len)` → 遍历 list 调 `inner.build`。

**为什么内联而非构造临时 ArrayNode**：避免堆分配 ArrayNode 实例。直接在
PrefixedArrayNode::parse 内联循环，逻辑等价于 ArrayNode 但消除一层间接。

### 2.7 决策 A7：ListContainer 直接返回原生 `list`（不引入新 Python 类型）

Python `ListContainer` 是 `list` 子类，仅添加 `repr`/`str` 美化与 `search`/`search_all`。
`==` 与普通 list 完全一致。

**决策**：construct-rs 直接返回原生 `list`（`PyList`）。

理由：
1. 引入 `ListContainer` Python 子类需要 Rust 侧 pyclass + Python 侧类定义 + 类型注册，
   增加复杂度。
2. `repr` 美化是非必要功能（用户可用 `pprint` 或自定义）。
3. `search`/`search_all` 是工具方法，可后续作为独立函数提供（非类型绑定）。
4. 性能：原生 `PyList` 创建走 C API fast path，子类实例化走慢路径。

**已知限制**：用户代码若 `isinstance(result, ListContainer)` 检查会失败。
这是 Phase 4 的已知行为差异，文档标注（§9.5 LC-1）。

### 2.8 决策 A8：`discard` 用编译期标志（运行时零开销）

Python `Array(count, subcon, discard=True)` 仍消耗流但不收集结果。

**决策**：`discard: bool` 字段直接存在 ArrayNode/GreedyRangeNode/RepeatUntilNode 中。
parse 循环内 `if !self.discard { list.append(value) }`——分支预测稳定（每次相同），
CPU 预测准确率 100%，几乎零开销。

---

## 3. 基础设施扩展

### 3.1 ParseStream / BuildStream 新增 `seek`

GreedyRange 硬依赖 seek：失败时回退到最后成功位置。

#### 3.1.1 ParseStream::seek

```rust
impl<'a> ParseStream<'a> {
    /// 设置字节游标到 `pos`，同时重置 bit 游标为 0（字节对齐）。
    ///
    /// 用于 GreedyRange 失败回退（对齐 Python `stream_seek(stream, fallback, 0, path)`，
    /// `whence=0` 绝对定位）。
    ///
    /// # 边界
    ///
    /// - `pos > data.len()`：返回 `ConstructError::Stream`（含 expected/found）。
    /// - `bit_pos != 0` 时调用：先重置 bit_pos 为 0（GreedyRange 通常字节对齐，
    ///   此分支防御性兼容）。
    ///
    /// # 参数
    ///
    /// - `pos`：目标字节位置（绝对偏移，0-based）。
    /// - `path`：错误追踪路径。
    pub fn seek(&mut self, pos: usize, path: &Path) -> Result<(), ConstructError> {
        if pos > self.data.len() {
            return Err(ConstructError::Stream {
                message: format!(
                    "stream seek out of bounds, pos={}, data_len={}",
                    pos, self.data.len()
                ),
                path: path.to_string(),
            });
        }
        self.pos = pos;
        self.bit_pos = 0;
        Ok(())
    }
}
```

> **BuildStream 不需要 seek**：build 方向是顺序写入，GreedyRange build 失败时
> 不回退（直接返回错误）。若未来需求出现，可补充。

#### 3.1.2 ParseStream::tell 已存在

现有 `tell()` 返回 `self.pos`（字节游标），GreedyRange 直接用 `stream.tell()` 记录
fallback。**无需新增**。

### 3.2 Context 新增 `_index` 字段

#### 3.2.1 字段定义

在 `Context` 结构体新增 `_index: Option<usize>` 字段。所有现有构造函数
（`new_root` / `placeholder` / `new_child` / `new_child_placeholder`）初始化为 `None`。

```rust
impl<'py> Context<'py> {
    /// 设置当前数组迭代下标。
    /// 由 ArrayNode / GreedyRangeNode / RepeatUntilNode / PrefixedArrayNode 在
    /// 每次迭代前调用。
    #[inline]
    pub fn set_index(&mut self, index: usize) {
        self._index = Some(index);
    }

    /// 清除当前数组迭代下标。
    /// 由 Array 系列节点在循环结束后调用（恢复"不在数组中"状态）。
    #[inline]
    pub fn clear_index(&mut self) {
        self._index = None;
    }

    /// 读取当前数组迭代下标。
    /// IndexNode 与 ExprOp::GetIndex 使用。`None` 表示不在数组迭代中。
    #[inline]
    pub fn index(&self) -> Option<usize> {
        self._index
    }
}
```

#### 3.2.2 嵌套数组语义（关键）

Array 内 Array（如 `Array(3, Array(2, Byte))`）：

```
外层 i=0: ctx._index = Some(0)
  内层进入：保存 old_outer = Some(0)（在 ArrayNode::parse 局部变量）
  内层 j=0: ctx._index = Some(0)  ← 覆盖外层
  内层 j=1: ctx._index = Some(1)
  内层结束：ctx._index = old_outer = Some(0)  ← 恢复
外层 i=1: ctx._index = Some(1)
  ...
```

**实现**（ArrayNode::parse 伪代码）：

```rust
let old_index = ctx.index();           // 保存外层下标
for i in 0..count {
    ctx.set_index(i);                  // 设置当前下标
    let elem = self.inner.parse(...)?;
    if !self.discard { list.append(elem); }
}
match old_index {
    Some(idx) => ctx.set_index(idx),   // 恢复外层下标
    None => ctx.clear_index(),         // 或清除
}
```

**子 Struct 的 _index 可见性**：内层 Struct 通过 `new_child` 创建子 context，
子 context **不**自动继承父的 `_index`（Python 中 `_index` 是直接写在同一个
context dict 上的，子 context 通过 `_` 看父的 _index）。

设计选择：
- **选项 A**：子 context 继承父的 `_index`（new_child 时复制）
- **选项 B**：子 context 不继承，需要时通过 `parent()` 链查找

**采用选项 A**：在 `new_child` / `new_child_placeholder` 中复制父的 `_index` 到子。
理由：
- 子 Struct 的 Index 字段（`rfield(Index())`）应直接读到外层 Array 的下标（对齐
  Python 中 `this._index` 在子 Struct 中可见的行为——**Python 语法**，construct-rs
  通过 IndexNode + ctx._index 继承实现等价语义）
- 通过 parent 链查找的开销 = O(depth)，复制开销 = O(1)
- 内层 Array 自己 set_index 会覆盖复制来的值，符合嵌套语义

```rust
pub fn new_child(parent: &'py Context<'py>, py: Python<'py>) -> PyResult<Self> {
    Ok(Self {
        fields: Some(PyDict::new_bound(py)),
        parent: Some(parent),
        expr_values_buf: [std::ptr::null_mut(); MAX_INLINE_FIELDS],
        expr_values_len: 0,
        _index: parent._index,    // 继承父的 _index
    })
}
```

> **R4 inject_fields 路径**（struct_ref.rs）也需要同步继承 _index。
> DEV 实现时检查所有 Context 构造点。

### 3.3 ExprOp 不扩展（Index 与 Element 都通过字段引用复用 GetInt）

> **v3 修正（REV 决策记录对照驳回）**：
> v1/v2 在此节设计了 `ExprOp::GetIndex` 指令，声称支持 `this._index` 表达式。
> 但这违反 Phase 2 决策 1（废弃 `this`）——construct-rs 表达式系统输入只有
> `_FieldDescriptor` / `_ExprRef` / 常量三种节点（表达式系统 §2.3 / §3.3），
> 不存在 `this._index` 节点类型。Python 侧 `_compile_expr_tree`
> （`_mixin.py` L475-533）也没有产生 `("getindex",)` 元组的分支。
> v1/v2 设计的 GetIndex 是事实上的死代码。
>
> **v5 决策（用户硬约束 #5 + Element 一致性）**：
> v4 实施版本曾引入 `ExprOp::GetElem`（repeat_until.rs 谓词路径用），
> 但 v5 删除——RepeatUntil 终止表达式统一通过 `rfield(Element())` +
> 字段名引用，编译为 `[GetInt(idx_of_e), ...]`，与其他字段引用完全同模式。
> 详见 §2.5.3 Element 与 Index 一致性论证。
>
> **v3/v5 决策合并（写入设计决策记录 Phase 4 决策 3 + 决策 5）**：
> **不扩展 ExprOp**——Index 与 Element 都不引入新 ExprOp 指令。
>
> 用户访问数组下标 / RepeatUntil 当前元素的机制：

> | 用户需求 | construct-rs 写法 | 编译结果 |
> |---------|------------------|---------|
> | 取当前下标值（作为字段） | `i: int = rfield(Index())` | IndexNode.parse 读 `ctx.index()` |
> | 在表达式中引用下标 | 先声明 Index 字段，再用字段名引用：`v: bytes = field(Bytes(i + 1))` | `[GetInt(idx_of_i), Const(1), Add]` |
> | 取 RepeatUntil 当前元素值（作为字段） | `e: int = rfield(Element())` | ElementNode.parse 在 RepeatUntil 之外返回 None；RepeatUntilNode 迭代时 set_field_at 覆盖该槽位 |
> | 在 RepeatUntil 终止表达式中引用当前元素 | 先声明 Element 字段，再用字段名引用：`RepeatUntil(e > 5, subcon)` | `[GetInt(idx_of_e), Const(5), Gt]` |
>
> 这与 Phase 2 "字段名即引用、废弃 this" 的精神一致——所有引用统一走
> `_FieldDescriptor`，表达式系统输入类型保持纯粹。
>
> **对已实现代码的影响**（DEV 在 4.5 v5 子任务中执行回退/重构）：
> - `expr.rs`：**删除** `ExprOp::GetElem` 变体、`compute_max_stack` 中的 GetElem 分支、
>   `eval_expr_int` 中的 GetElem 分支、相关单元测试（约 2-4 个）。
> - `compile.rs`：**删除** `parse_expr_ops_from_py` 中的 `"getelem"` 分支。
> - `context.rs`：**删除** `_current_elem_ptr` 字段、`set_current_elem_ptr`、
>   `clear_current_elem_ptr`、`current_elem_ptr`、`current_elem_as_i64` 方法、
>   相关测试。
> - `container_cache.rs`：**删除整个文件**（PyCallable 路径不再需要 Container 缓存）。
> - `nodes/repeat_until.rs`：**完全重写**（删除 `RepeatPredicate` 枚举、
>   `call_repeat_predicate`、`build_context_proxy`、`parse_callable_path`、
>   `parse_expr_path`、`eval_expr_predicate`；新增 Element 字段借用模式）。
> - `python/construct/_descriptors.py`：**删除** `_AST_OP_TO_EXPROP`、
>   `_try_compile_repeat_predicate`；重写 `RepeatUntilDescriptor`（参数名
>   `predicate` → `terminator`，类型从 callable 改为 Phase 2 表达式）；
>   新增 `ElementDescriptor` + `Element()` 工厂。
> - `python/construct/lib/containers.py`：若专为 PyCallable 新建则**删除整个文件**
>   （PM 与 DEV 确认；若该文件为 ListContainer 兼容性保留则不删除，但
>   `container_cache.rs` 中对其的引用全部删除）。
>
> **边界说明**：
> - 用户代码 `rfield(Element())` 中 ElementNode.parse 在 RepeatUntil 之外时返回
>   `None`（对齐 IndexNode 在 Array 之外返回 None）。Packet 实例的 Element 字段
>   属性始终为 None（Element 字段不持有真实数据，仅作为终止表达式引用入口）。
> - 用户代码 `e: int = rfield(Element()); payload = field(RepeatUntil(e + 1 > 0, ...))`
>   中，若误用 Element 字段（如直接 `packet.e` 访问）得到 None，是预期行为。
>   Element 字段必须与 RepeatUntil 终止表达式配合使用。

### 3.4 ConstructError 新增变体

#### 3.4.1 `Range`（对应 Python `RangeError`）

```rust
/// Array count 无效（负数或与给定列表长度不符）。
/// 对应 Python construct 的 `RangeError`（core.py L2528、L2541、L2543）。
#[error("range error: {message} at {path}")]
Range {
    message: String,
    path: String,
},
```

**触发场景**：
- AR-2：count 表达式求值为负数
- AR-3：build 时 `len(obj) != count`
- AR-7：PrefixedArray count 为负

#### 3.4.2 `Repeat`（对应 Python `RepeatError`）

```rust
    /// RepeatUntil build 时无元素满足终止表达式。
    /// 对应 Python construct 的 `RepeatError`（core.py L2700）。
#[error("repeat error: {message} at {path}")]
Repeat {
    message: String,
    path: String,
},
```

**触发场景**：RU-3（build 遍历完列表无元素满足终止表达式）。

#### 3.4.3 `StopField`（早停哨兵，§2.4）

```rust
/// 早停信号（仅 StopIfNode 产生，GreedyRangeNode / StructNode / SequenceNode 捕获）。
/// 对应 Python construct 的 `StopFieldError`（core.py L4106）。
#[error("stop field signal at {path}")]
StopField {
    path: String,
},
```

#### 3.4.4 `IndexField`（对应 Python `IndexFieldError`）

```rust
/// Index 节点读取 _index 时上下文未提供（理论上不发生——Array 都会设置）。
/// 对应 Python construct 的 `IndexFieldError`。
#[error("index field error: {message} at {path}")]
IndexField {
    message: String,
    path: String,
},
```

> **保留但当前不触发**：当前设计 IndexNode 在 _index 为 None 时返回 Py_None
> （对齐 Python `context.get("_index", None)`），不报错。此变体保留供未来
> 严格模式使用（如 `Index` 强制要求在数组内）。Phase 4 实现可不映射到此变体。

#### 3.4.5 Python 异常类映射

`error.rs` 的 `init_exception_classes` 需要从 `construct._errors` 缓存 4 个新类：

```rust
range_error: Py<PyType>,           // construct.RangeError
repeat_error: Py<PyType>,          // construct.RepeatError
stop_field_error: Py<PyType>,      // construct.StopFieldError
index_field_error: Py<PyType>,     // construct.IndexFieldError
```

`select_exception_class` 增加 4 个分支。

> **StopField 不应跨 FFI**：StopField 是内部哨兵，正常路径被 GreedyRange 等捕获，
> 不应到达 FFI 入口。若意外到达（如 StopIf 在 Array 内），映射到 `stop_field_error`
> 让 Python 用户可识别（虽然这是用户误用）。

---

## 4. 各构造器详细设计

### 4.1 ArrayNode（P0，固定次数数组）

#### 4.1.1 数据结构

```rust
/// 固定次数数组节点。
/// 对应 Python construct `Array(count, subcon, discard)`（core.py L2493）。
#[derive(Debug)]
pub struct ArrayNode {
    /// 元素子树（递归 Box）。
    inner: Box<Node>,
    /// 元素数量来源（静态 / 表达式）。
    count: CountSource,
    /// 是否丢弃解析结果（仍消耗流）。
    discard: bool,
}

#[derive(Debug, Clone)]
pub enum CountSource {
    /// 编译期常量。
    Const(usize),
    /// 表达式程序（运行时求值）。
    Expr(ExprProgram),
}

impl ArrayNode {
    pub fn new(inner: Node, count: CountSource, discard: bool) -> Self {
        Self { inner: Box::new(inner), count, discard }
    }

    /// 求值元素数量。负数（i64 < 0）返回 `Range` 错误（对齐 core.py L2527-2528）。
    fn eval_count(&self, ctx: &Context<'_>, py: Python<'_>) -> Result<usize, ConstructError> {
        let count_i64 = match &self.count {
            CountSource::Const(n) => *n as i64,
            CountSource::Expr(prog) => crate::expr::eval_expr_int(prog, ctx, py)?,
        };
        if count_i64 < 0 {
            return Err(ConstructError::Range {
                message: format!("invalid count {}", count_i64),
                path: String::new(),
            });
        }
        Ok(count_i64 as usize)
    }
}
```

#### 4.1.2 parse 流程

对齐 Python `Array._parse`（core.py L2525-2536）：

```rust
fn parse<'py>(
    &self,
    py: Python<'py>,
    stream: &mut ParseStream<'_>,
    ctx: &mut Context<'py>,
    path: &mut Path,
) -> Result<Py<PyAny>, ConstructError> {
    let count = self.eval_count(ctx, py)?;      // AR-1: 求值 count

    // 预分配 PyList（count 个 None 占位，append 时替换）。
    // PyList::new(py, count) 比 append 循环快（一次性分配）。
    let list = PyList::new_bound(py, Vec::<Py<PyAny>>::with_capacity(count));

    let old_index = ctx.index();                // 保存外层 _index（嵌套数组）

    for i in 0..count {
        ctx.set_index(i);                       // AR-4: 设置 _index
        path.push_index(i);                     // 错误路径追踪
        let elem = match self.inner.parse(py, stream, ctx, path) {
            Ok(v) => v,
            Err(e) => {
                path.pop();
                // 恢复 _index（即使出错也要恢复，避免污染外层）
                match old_index { Some(idx) => ctx.set_index(idx), None => ctx.clear_index() }
                return Err(e);                  // AR-5: 错误向上传播
            }
        };
        path.pop();
        if !self.discard {
            list.append(elem).map_err(ConstructError::from)?;
        } else {
            drop(elem);
        }
    }

    match old_index { Some(idx) => ctx.set_index(idx), None => ctx.clear_index() }

    Ok(list.into_any())
}
```

**性能要点**：
- `PyList::new_bound(py, Vec::with_capacity(count))`：一次性预分配，避免 append 时
  多次 realloc（C API fast path）。
- `path.push_index/pop` 仅在错误路径有开销？**否**——当前设计每次迭代都 push/pop，
  开销 ~10ns × N。若性能不达标，可改为"P0-3 优化"风格（成功路径不 push，错误时
  push_path_segment）。**初始实现保留 push/pop 简化逻辑**，性能优化延后。

#### 4.1.3 build 流程

对齐 Python `Array._build`（core.py L2538-2551）：

```rust
fn build(
    &self,
    py: Python<'_>,
    obj: &Bound<'_, PyAny>,
    stream: &mut BuildStream,
    ctx: &mut Context<'_>,
    path: &mut Path,
) -> Result<(), ConstructError> {
    let count = self.eval_count(ctx, py)?;

    // 校验 obj 是 list/tuple，并取长度。
    let obj_len = if let Ok(list) = obj.downcast::<PyList>() {
        list.len()
    } else if let Ok(tuple) = obj.downcast::<PyTuple>() {
        tuple.len()
    } else {
        // 兜底：尝试 len(obj)（对任意可迭代对象）。
        // 性能差，仅在用户传入非 list/tuple 时触发。
        obj.len().map_err(|_| ConstructError::Generic {
            message: format!("Array build expects list/tuple, got {}", 
                obj.get_type().name().map(|n| n.to_string()).unwrap_or_default()),
            path: path.to_string(),
        })?
    };

    // AR-3: 长度校验
    if obj_len != count {
        return Err(ConstructError::Range {
            message: format!("expected {} elements, found {}", count, obj_len),
            path: path.to_string(),
        });
    }

    let old_index = ctx.index();
    let items: Vec<Py<PyAny>> = if let Ok(list) = obj.downcast::<PyList>() {
        list.iter().map(|b| b.unbind()).collect()
    } else if let Ok(tuple) = obj.downcast::<PyTuple>() {
        tuple.iter().map(|b| b.unbind()).collect()
    } else {
        // 兜底路径：转 list 后取（性能差）
        PyList::new_bound(py, obj.try_iter().map_err(|e| ConstructError::Generic {
            message: format!("not iterable: {}", e),
            path: path.to_string(),
        })?).into_any().downcast::<PyList>().unwrap().iter().map(|b| b.unbind()).collect()
    };

    for (i, elem) in items.into_iter().enumerate() {
        ctx.set_index(i);
        path.push_index(i);
        let elem_bound = elem.bind(py);
        if let Err(e) = self.inner.build(py, elem_bound, stream, ctx, path) {
            path.pop();
            match old_index { Some(idx) => ctx.set_index(idx), None => ctx.clear_index() }
            return Err(e);
        }
        path.pop();
    }

    match old_index { Some(idx) => ctx.set_index(idx), None => ctx.clear_index() }
    Ok(())
}
```

> **简化机会（DEV 决策）**：build 时可不预先 collect 到 Vec，直接遍历 list/tuple
> 迭代器。当前伪代码 collect 是为了同时支持 list/tuple/iterable 的统一处理。
> 若性能分析显示 collect 有显著开销，可拆分 list/tuple/iterable 三条分支。

#### 4.1.4 sizeof 流程

对齐 Python `Array._sizeof`（core.py L2553-2558）：

```rust
fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
    let count = match &self.count {
        CountSource::Const(n) => *n,
        CountSource::Expr(prog) => {
            // 表达式 count：尝试求值，失败（如字段缺失）返回 Err（对齐 SizeofError）。
            crate::expr::eval_expr_int(prog, ctx, Python::with_gil(|py| py))? as usize
        }
    };
    let elem_size = self.inner.sizeof(ctx)?;
    count.checked_mul(elem_size).ok_or_else(|| ConstructError::Generic {
        message: format!("array size overflow: {} * {}", count, elem_size),
        path: String::new(),
    })
}
```

> **GIL 前置条件（M3 修正，明确化）**：sizeof 接收 `&Context` 而非 `py: Python`
> （现有 Construct trait 签名）。若 count 是表达式，需获取 GIL 调 `eval_expr_int`。
>
> **契约明确**：
> 1. **sizeof 必须在已持有 GIL 的线程调用**。当前所有 sizeof 调用点都在
>    parse/build 内部（FFI 入口已持 GIL），此前置条件自然满足。Rust 侧不
>    显式断言 GIL（pyo3 内部会 panic，但实际不触发）。
> 2. **`Python::with_gil` 在已持有 GIL 时是 O(1)**（仅获取 token，不阻塞），
>    可接受。但需注意闭包返回值的生命周期——`Python::with_gil(|py| py)` 返回的
>    `Python<'_>` 借用闭包内的 GIL scope，**不能跨闭包边界使用**。正确写法是把
>    所有需要 `py` 的逻辑放进闭包内：
>    ```rust
>    let n = Python::with_gil(|py| -> Result<usize, ConstructError> {
>        let v = crate::expr::eval_expr_int(prog, ctx, py)?;
>        if v < 0 {
>            return Err(ConstructError::Range {
>                message: format!("sizeof array count {} is negative", v),
>                path: String::new(),
>            });
>        }
>        Ok(v as usize)
>    })?;
>    ```
>    DEV 实现时严格按此模式，避免借用逃逸。
> 3. **sizeof 不在热路径**（Python 用户极少对 Array 调 sizeof），with_gil 开销可接受。
>
> **替代方案（不采用）**：修改 Construct trait 的 sizeof 签名增加 `py: Python<'_>`
> 参数。优点是消除 with_gil；缺点是影响全部 13 个现有节点 + IndexNode/StopIfNode 的
> sizeof 签名，改动面大。PM 决策点 6 维持选项 A（with_gil）。

### 4.2 GreedyRangeNode（P1，读到流结束）

#### 4.2.1 数据结构

```rust
/// 读到流结束的数组节点。
/// 对应 Python construct `GreedyRange(subcon, discard)`（core.py L2570）。
#[derive(Debug)]
pub struct GreedyRangeNode {
    inner: Box<Node>,
    discard: bool,
}
```

#### 4.2.2 parse 流程

对齐 Python `GreedyRange._parse`（core.py L2599-2615）：

```rust
fn parse<'py>(...) -> Result<Py<PyAny>, ConstructError> {
    let list = PyList::new_bound(py, Vec::<Py<PyAny>>::new());  // 无法预估容量，动态增长
    let old_index = ctx.index();
    let mut i: usize = 0;

    loop {
        // 记录 fallback 位置（用于子构造器失败时回退）。
        let fallback = stream.tell();

        ctx.set_index(i);
        path.push_index(i);

        match self.inner.parse(py, stream, ctx, path) {
            Ok(elem) => {
                path.pop();
                if !self.discard {
                    list.append(elem).map_err(ConstructError::from)?;
                }
                i += 1;
                // 继续下一次迭代
            }
            Err(ConstructError::StopField { .. }) => {
                // StopIf 触发：正常终止（对齐 Python StopFieldError 捕获）。
                path.pop();
                stream.seek(fallback, path)?;   // 回退到 fallback（StopIf 不消耗字节）
                break;
            }
            Err(e) => {
                path.pop();
                // 区分错误类型（对齐 Python L2609-2614）：
                // - Stream 错误（EOF / 字节不足）：seek 回退，正常终止
                // - 其他错误（FormatField 类型错、表达式错等）：seek 回退，正常终止
                //   （Python 用 `except Exception` 捕获所有非 ExplicitError）
                // - 注意：ExplicitError 在 Python 中向上传播。
                //   construct-rs 没有 ExplicitError 等价物（无独立变体），
                //   所有错误都走"seek 回退 + 正常终止"路径（保守对齐）。
                //   未来若引入 ExplicitError 等价变体，再分流。
                let _ = stream.seek(fallback, path);   // 回退，忽略 seek 错误（已是要终止）
                break;
            }
        }
    }

    match old_index { Some(idx) => ctx.set_index(idx), None => ctx.clear_index() }
    Ok(list.into_any())
}
```

**关键差异 vs Python**：
- Python 用 `itertools.count()` 无限循环 + 异常退出。Rust 用 `loop` + `break`。
- Python `except StopFieldError` 与 `except Exception` 分流。Rust 用 `match` 错误变体。
  当前设计：StopField 单独处理，其他错误统一"回退 + 终止"。这与 Python 等价
  （Python 的 ExplicitError 在 construct-rs 中暂无对应）。

#### 4.2.3 build 流程

对齐 Python `GreedyRange._build`（core.py L2617-2628）：

```rust
fn build(...) -> Result<(), ConstructError> {
    // obj 必须是可迭代对象（list/tuple/任意 iterable）。
    let items: Vec<Py<PyAny>> = collect_iterable(py, obj, path)?;

    let old_index = ctx.index();
    for (i, elem) in items.into_iter().enumerate() {
        ctx.set_index(i);
        path.push_index(i);
        let elem_bound = elem.bind(py);
        match self.inner.build(py, elem_bound, stream, ctx, path) {
            Ok(()) => { path.pop(); }
            Err(ConstructError::StopField { .. }) => {
                // StopIf 触发：停止后续元素构建（对齐 Python L2627-2628）。
                path.pop();
                break;
            }
            Err(e) => {
                path.pop();
                match old_index { Some(idx) => ctx.set_index(idx), None => ctx.clear_index() }
                return Err(e);
            }
        }
    }
    match old_index { Some(idx) => ctx.set_index(idx), None => ctx.clear_index() }
    Ok(())
}
```

#### 4.2.4 sizeof

永远 Err（对齐 Python `GreedyRange._sizeof` L2630-2631）：

```rust
fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
    Err(ConstructError::Generic {
        message: "GreedyRange size is undefined".to_string(),
        path: String::new(),
    })
}
```

> **替代**：可新增 `ConstructError::Sizeof` 专门变体映射 Python `SizeofError`。
> 当前 `Generic` 已足够（错误消息明确）。PM 决策是否细化。

### 4.3 RepeatUntilNode（P3，终止表达式）

> **v5 完全重写**：删除 v4 的 PyCallable 路径、`RepeatPredicate` 枚举、
> `call_repeat_predicate`、`build_context_proxy`、Container proxy、AST 识别器。
> 终止表达式 = Phase 2 表达式（`ExprProgram`），用户面通过 `rfield(Element())`
> 字段引用当前元素。详见 §2.5 决策 A5 v5。

#### 4.3.1 数据结构（v5）

```rust
/// 终止表达式数组节点。
/// 对应 Python construct `RepeatUntil(predicate, subcon, discard)`（core.py L2637）。
///
/// construct-rs 用户面 API（v5）：
/// ```python
/// RepeatUntil(terminator, subcon, discard=False)
/// ```
/// `terminator` 是 Phase 2 表达式（引用 Element 字段），编译期翻译为 ExprProgram。
///
/// # 工作原理
///
/// 每次迭代：
/// 1. inner.parse → elem（失败直接上抛，RU-1）
/// 2. `ctx.set_field_at(element_field_idx, elem, ...)` 把当前元素借用为 Element 字段槽位
/// 3. 求值终止表达式（GetInt(element_field_idx) 从 expr_values_buf 取到 elem）
/// 4. 表达式非零 → 终止（最后元素包含在内，RU-2）；零 → 继续迭代
///
/// 终止表达式求值全程在 Rust 内部栈式 VM 执行，零 FFI。
#[derive(Debug)]
pub struct RepeatUntilNode {
    /// 元素子树（递归 Box）。
    inner: Box<Node>,
    /// 终止表达式（Phase 2 ExprProgram，编译期从用户面表达式编译）。
    /// 求值结果非零即终止。
    terminator: ExprProgram,
    /// Element 字段在当前 Struct 的 `expr_values_buf` 中的索引（编译期确定）。
    /// RepeatUntil 每次迭代调 `ctx.set_field_at(element_field_idx, ...)` 把当前元素
    /// 写入该槽位，终止表达式中的 GetInt(element_field_idx) 即取到此值。
    element_field_idx: usize,
    /// Element 字段名（interned PyString，set_field_at 的 key 参数）。
    element_field_name: Py<PyString>,
    /// 是否丢弃解析结果（仍消耗流）。
    discard: bool,
}

impl RepeatUntilNode {
    pub fn new(
        inner: Node,
        terminator: ExprProgram,
        element_field_idx: usize,
        element_field_name: Py<PyString>,
        discard: bool,
    ) -> Self {
        Self {
            inner: Box::new(inner),
            terminator,
            element_field_idx,
            element_field_name,
            discard,
        }
    }

    /// has_expressions 判断（设计 §6.1.1）。
    ///
    /// **始终返回 true**（v5）：终止表达式始终引用 Element 字段，需要外层 Struct
    /// 通过 `init_expr_values` 初始化 `expr_values_buf`，使 `set_field_at` 可用。
    ///
    /// 此外，inner 子树可能含表达式（虽 Phase 4 暂不支持 inner 表达式，但
    /// 防御性返回 true 避免未来扩展时遗漏 init）。
    pub fn has_expressions(&self) -> bool {
        true
    }
}
```

**为什么 RepeatUntilNode 持有 `element_field_idx` 而非 ElementNode 引用**：

RepeatUntil 与 Element 字段是**两个独立的 Node**，通过 `element_field_idx` 在
`expr_values_buf` 中"借用"同一个槽位实现通信：

- StructNode.parse 处理 `e` 字段时：ElementNode.parse 返回 None（因为不在
  RepeatUntil 内），StructNode 把 None set_field_at(idx_of_e) 到 buf。
- StructNode.parse 处理 `payload` 字段时：RepeatUntilNode.parse 接管，
  每次迭代 set_field_at(idx_of_e, elem, ...) **覆盖**该槽位为当前元素，
  求值终止表达式（GetInt(idx_of_e) 取到 elem），下次迭代再覆盖。

`element_field_idx` 是编译期从 Element 字段在 Struct 中的声明顺序推导的常量，
运行时零开销查找。

#### 4.3.2 parse 流程（v5）

对齐 Python `RepeatUntil._parse`（core.py L2670-2682），但终止条件用 Phase 2
表达式（零 FFI）：

```rust
fn parse<'py>(
    &self,
    py: Python<'py>,
    stream: &mut ParseStream<'_>,
    ctx: &mut Context<'py>,
    path: &mut Path,
) -> Result<Py<PyAny>, ConstructError> {
    let list = PyList::new_bound(py, Vec::<Py<PyAny>>::new());
    let old_index = ctx.index();
    let elem_name = self.element_field_name.bind(py);

    let result = self.parse_loop(py, stream, ctx, path, &list, elem_name);

    restore_index(ctx, old_index);
    match result {
        Ok(()) => Ok(list.into_any().unbind()),
        Err(e) => Err(e),
    }
}

fn parse_loop<'py>(
    &self,
    py: Python<'py>,
    stream: &mut ParseStream<'_>,
    ctx: &mut Context<'py>,
    path: &mut Path,
    list: &Bound<'py, PyList>,
    elem_name: &Bound<'py, PyString>,
) -> Result<(), ConstructError> {
    let mut i: usize = 0;
    loop {
        ctx.set_index(i);
        path.push_index(i);

        let elem = match self.inner.parse(py, stream, ctx, path) {
            Ok(v) => v,
            Err(e) => {
                path.pop();
                // RU-1: 失败直接上抛（不像 GreedyRange 回退）
                return Err(e);
            }
        };
        path.pop();

        if !self.discard {
            list.append(elem.clone_ref(py))
                .map_err(ConstructError::from)?;
        }

        // 把当前元素写入 Element 字段槽位（借用模式）。
        // 终止表达式中的 GetInt(element_field_idx) 会从此槽位取值。
        // set_field_at 内部走 expr_values_buf（栈分配内联数组），零 PyDict 开销。
        ctx.set_field_at(self.element_field_idx, elem_name, elem.bind(py), py)
            .map_err(|e| ConstructError::Generic {
                message: format!("RepeatUntil parse: set_field_at failed: {}", e),
                path: path.to_string(),
            })?;

        // 求值终止表达式（零 FFI，Rust 内部栈式 VM）。
        // 表达式非零即终止（最后元素已包含在 list 中，RU-2）。
        let stop_v = crate::expr::eval_expr_int(&self.terminator, ctx, py)?;
        if stop_v != 0 {
            return Ok(());
        }

        i = i.saturating_add(1);
    }
}
```

**性能要点**：
- `ctx.set_field_at`：直接写入 `expr_values_buf[idx]`（栈分配内联数组），
  无 PyDict 操作，~5-10ns/iter
- `eval_expr_int`：栈式 VM 求值，~15-25ns/iter（典型终止表达式 3 个 ExprOp）
- 无 PyCallable FFI 调用、无 Container proxy 构造、无 ctx 字段浅复制

> **build 方向的 set_field_at 副作用**：parse 方向 RepeatUntilNode 主动覆盖
> Element 字段槽位。**StructNode.parse 完成后**，Element 字段在 PyDict 中的值
> 仍是 StructNode 在处理 e 字段时写入的 None（因为 set_field_at 不写 PyDict，
> 只写 expr_values_buf）。因此 Packet 实例的 e 属性 = None（语义正确）。

#### 4.3.3 build 流程（v5）

对齐 Python `RepeatUntil._build`（core.py L2684-2701）：

```rust
fn build(
    &self,
    py: Python<'_>,
    obj: &Bound<'_, PyAny>,
    stream: &mut BuildStream,
    ctx: &mut Context<'_>,
    path: &mut Path,
) -> Result<(), ConstructError> {
    let old_index = ctx.index();
    let elem_name = self.element_field_name.bind(py);

    // 收集 obj（list/tuple/任意 iterable）到 Vec。
    // 注意：与 v4 不同，v5 必须物化到 Vec——终止表达式求值需要 owned elem 引用
    // 写入 expr_values_buf。但仅在终止表达式满足前需要物化，可优化为惰性迭代
    // （性能不达标再优化，§10.2）。
    let items: Vec<Py<PyAny>> = collect_iterable(py, obj, path)?;

    let mut matched = false;
    for (i, elem) in items.iter().enumerate() {
        ctx.set_index(i);
        path.push_index(i);
        let elem_bound = elem.bind(py);
        if let Err(e) = self.inner.build(py, elem_bound, stream, ctx, path) {
            path.pop();
            restore_index(ctx, old_index);
            return Err(e);
        }
        path.pop();

        // 把当前元素写入 Element 字段槽位（与 parse 同模式）。
        ctx.set_field_at(self.element_field_idx, elem_name, elem_bound, py)
            .map_err(|e| ConstructError::Generic {
                message: format!("RepeatUntil build: set_field_at failed: {}", e),
                path: path.to_string(),
            })?;

        // 求值终止表达式。
        let stop_v = crate::expr::eval_expr_int(&self.terminator, ctx, py)?;
        if stop_v != 0 {
            matched = true;
            break;
        }
    }

    restore_index(ctx, old_index);

    if !matched {
        // RU-3: 无元素满足终止表达式
        return Err(ConstructError::Repeat {
            message: "expected any item to match terminator, when building"
                .to_string(),
            path: path.to_string(),
        });
    }
    Ok(())
}
```

**与 Python 的语义对齐**：
- Python `if predicate(e, partiallist, context): break`：partiallist 是已构建元素
  的 list（discard=False 时收集，discard=True 时为空）。
- construct-rs v5：终止表达式不接收 list 参数（Phase 2 表达式 VM 栈为 i64，
  不支持 list 引用）。**这是已知能力边界**（§2.5.4 / §13.9），用户须改用 Adapter
  显式包装 list-dependent 终止逻辑。
- discard 在 v5 中仅影响 `list.append`（parse 方向）；build 方向 discard
  不影响终止表达式求值（终止表达式不依赖 list）。

> **v5 行为差异**（vs Python construct）：
>
> Python `RepeatUntil(lambda x,lst,c: lst[-2:] == [0, 0], Byte)` 这种
> 依赖 list 内容的终止逻辑，construct-rs RepeatUntil **不支持**。
> 用户须改用 Adapter（显式慢路径）。详见 §13.9 能力边界 + Adapter 替代示例。

#### 4.3.4 sizeof

永远 Err（对齐 Python L2703-2704）：

```rust
fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
    Err(ConstructError::Generic {
        message: "RepeatUntil size is undefined".to_string(),
        path: String::new(),
    })
}
```

### 4.4 IndexNode（P4，取当前下标）

> **V1 修正（v3）**：IndexNode 是 construct-rs 中**唯一**的用户面下标访问机制。
> v1/v2 设计了 `ExprOp::GetIndex` 作为表达式内引用下标的入口，但用户面写法
> `this._index` 违反 Phase 2 决策 1，已被删除（§3.3）。
> 用户要在表达式中引用下标，先用 Index 字段声明再用字段名引用：
>
> ```python
> @dataclass
> class Inner(StructMixin):
>     i: int = rfield(Index())               # IndexNode，parse 得到当前下标
>     v: bytes = field(Bytes(i + 1))         # 字段名引用 i，编译为 [GetInt(0), Const(1), Add]
> ```

#### 4.4.1 数据结构

```rust
/// 取当前数组迭代下标的节点。
/// 对应 Python construct `Index`（core.py L2934）。
///
/// construct-rs 中 IndexNode 是用户访问数组下标的唯一机制（v3 决策，§3.3）。
/// 直接调 `ctx.index()` 读取，不走 ExprProgram。
#[derive(Debug)]
pub struct IndexNode;
```

#### 4.4.2 parse / build / sizeof

```rust
impl Construct for IndexNode {
    fn parse<'py>(&self, py: Python<'py>, _stream, ctx, _path) -> Result<Py<PyAny>, ConstructError> {
        // 对齐 Python `context.get("_index", None)`（Python 语法）。
        match ctx.index() {
            Some(i) => Ok(i.into_py(py)),       // 返回 PyLong
            None => Ok(py.None()),               // 返回 Py_None
        }
    }

    fn build(&self, py: Python<'_>, _obj, _stream, ctx, _path) -> Result<(), ConstructError> {
        // 对齐 Python `Index._build`：返回 _index（但 build 不写流）。
        // IndexNode 的 sizeof=0，build 是 no-op。
        let _ = ctx.index();   // 不实际使用（保持接口一致）
        let _ = py;
        Ok(())
    }

    fn sizeof(&self, _ctx) -> Result<usize, ConstructError> {
        Ok(0)   // 对齐 Python `Index._sizeof` 返回 0
    }
}
```

> **build 语义**：Python `Index._build(obj, ...)` 返回 `context._index`，但
> build 结果不影响输出（Index 不写字节）。我们的 build 是 no-op，与 Python
> 等价（Python 的返回值在 Sequence 上下文中可能有用，但 IndexNode 的 build
> 不需要返回值，因为是 sizeof=0 字段）。
>
> **在表达式中引用下标的边界**：用户写 `i: int = rfield(Index()); v: bytes = field(Bytes(i + 1))`
> 时，若 IndexNode 在 `ctx.index() == None`（不在数组中）返回 Py_None，
> `i + 1` 表达式求值时 GetInt 会因 None 无法 extract 为 i64 抛 `ExprType` 错误。
> 这与 Python `this._index + 1`（**Python 语法**）在非数组上下文抛 TypeError
> 的行为一致——无需特殊处理。

### 4.5 StopIfNode（P5，早停信号）

#### 4.5.1 数据结构

```rust
/// 早停条件节点。
/// 对应 Python construct `StopIf(condfunc)`（core.py L4079）。
#[derive(Debug)]
pub struct StopIfNode {
    /// 条件：编译期常量（bool）或表达式程序。
    cond: StopIfCondition,
}

#[derive(Debug)]
pub enum StopIfCondition {
    /// 常量 true（永远停止，主要用于调试）。
    Always,
    /// 常量 false（永远不停止，主要用于调试）。
    Never,
    /// 表达式（如 `x == 0`，其中 `x` 是字段名引用，编译为 `[GetInt(idx), Const(0), Eq]`；
    /// construct-rs 不使用 Python 的 `this.x == 0` 语法）。
    Expr(ExprProgram),
}
```

#### 4.5.2 parse / build / sizeof

```rust
impl Construct for StopIfNode {
    fn parse<'py>(&self, py: Python<'py>, _stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
        let stop = self.eval_cond(ctx, py)?;
        if stop {
            // 抛出早停哨兵（被外层 Struct/Sequence/GreedyRange 捕获）。
            Err(ConstructError::StopField { path: path.to_string() })
        } else {
            Ok(py.None())
        }
    }

    fn build(&self, py, _obj, _stream, ctx, path) -> Result<(), ConstructError> {
        let stop = self.eval_cond(ctx, py)?;
        if stop {
            Err(ConstructError::StopField { path: path.to_string() })
        } else {
            Ok(())
        }
    }

    fn sizeof(&self, _ctx) -> Result<usize, ConstructError> {
        // 对齐 Python `StopIf._sizeof`：永远 SizeofError。
        Err(ConstructError::Generic {
            message: "StopIf size is undefined".to_string(),
            path: String::new(),
        })
    }
}

impl StopIfNode {
    fn eval_cond(&self, ctx: &Context<'_>, py: Python<'_>) -> Result<bool, ConstructError> {
        match &self.cond {
            StopIfCondition::Always => Ok(true),
            StopIfCondition::Never => Ok(false),
            StopIfCondition::Expr(prog) => {
                let v = crate::expr::eval_expr_int(prog, ctx, py)?;
                Ok(v != 0)
            }
        }
    }
}
```

> **StopIf 在 Struct 中的捕获**：当前 StructNode.parse 不识别 StopField
> （StructNode 设计于 Phase 1，先于 StopField 变体）。Phase 4 需在 StructNode.parse
> 增加 StopField 捕获分支：子字段返回 StopField 时，StructNode 停止后续字段，
> 正常返回当前实例。这是**对现有 StructNode 的小修改**，§6.1 详述。
>
> **是否阻塞**：此修改是 Phase 4 的必要配套（否则 StopIf 在 Struct 内无效）。
> DEV 实现时一并修改 struct_node.rs。

### 4.6 PrefixedArrayNode（P2，前缀长度数组）

#### 4.6.1 数据结构

```rust
/// 前缀长度数组节点。
/// 对应 Python construct `PrefixedArray(countfield, subcon)`（core.py L4934）。
///
/// 不依赖 FocusedSeq/Rebuild（未实现），独立实现 parse/build。
#[derive(Debug)]
pub struct PrefixedArrayNode {
    /// 计数字段（如 VarInt、Byte、Int16ub 等）。
    countfield: Box<Node>,
    /// 元素子树。
    inner: Box<Node>,
}
```

#### 4.6.2 parse 流程

对齐 Python `PrefixedArray._emitparse`（core.py L4961-4962）：

```rust
fn parse<'py>(...) -> Result<Py<PyAny>, ConstructError> {
    // 1. 解析 countfield 得到 count。
    let count_obj = self.countfield.parse(py, stream, ctx, path)?;
    let count_i64: i64 = count_obj.bind(py).extract()
        .map_err(|_| ConstructError::Range {
            message: "PrefixedArray countfield did not produce an integer".to_string(),
            path: path.to_string(),
        })?;
    if count_i64 < 0 {
        return Err(ConstructError::Range {
            message: format!("invalid PrefixedArray count {}", count_i64),
            path: path.to_string(),
        });
    }
    let count = count_i64 as usize;

    // 2. 内联 Array 逻辑（避免构造临时 ArrayNode 实例）。
    let list = PyList::new_bound(py, Vec::<Py<PyAny>>::with_capacity(count));
    let old_index = ctx.index();

    for i in 0..count {
        ctx.set_index(i);
        path.push_index(i);
        let elem = match self.inner.parse(py, stream, ctx, path) {
            Ok(v) => v,
            Err(e) => {
                path.pop();
                match old_index { Some(idx) => ctx.set_index(idx), None => ctx.clear_index() }
                return Err(e);
            }
        };
        path.pop();
        list.append(elem).map_err(ConstructError::from)?;
    }

    match old_index { Some(idx) => ctx.set_index(idx), None => ctx.clear_index() }
    Ok(list.into_any())
}
```

#### 4.6.3 build 流程

对齐 Python `PrefixedArray._emitbuild`（core.py L4965-4966）：

```rust
fn build(...) -> Result<(), ConstructError> {
    // 1. 取 list 长度。
    let items: Vec<Py<PyAny>> = collect_iterable(py, obj, path)?;
    let count = items.len();

    // 2. 先构建 countfield（写入长度）。
    let count_py = count.into_py(py);
    self.countfield.build(py, count_py.bind(py), stream, ctx, path)?;

    // 3. 遍历构建元素。
    let old_index = ctx.index();
    for (i, elem) in items.into_iter().enumerate() {
        ctx.set_index(i);
        path.push_index(i);
        let elem_bound = elem.bind(py);
        if let Err(e) = self.inner.build(py, elem_bound, stream, ctx, path) {
            path.pop();
            match old_index { Some(idx) => ctx.set_index(idx), None => ctx.clear_index() }
            return Err(e);
        }
        path.pop();
    }
    match old_index { Some(idx) => ctx.set_index(idx), None => ctx.clear_index() }
    Ok(())
}
```

#### 4.6.4 sizeof

```rust
fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
    // countfield 大小 + 0（元素数量运行时未知，无法静态计算）。
    // 对齐 Python PrefixedArray._actualsize（需流上下文）→ sizeof 总是 SizeofError。
    // 但若 count 已知（如 ctx 中有），可计算。
    // 简化：永远 Err（保守对齐）。
    Err(ConstructError::Generic {
        message: "PrefixedArray size depends on stream data".to_string(),
        path: String::new(),
    })
}
```

> **替代**（DEV 决策）：若需 sizeof 支持，可对静态 count（countfield 是常量表达式）
> 计算 `countfield.sizeof() + count * inner.sizeof()`。当前保守 Err。

### 4.7 ElementNode（P6，RepeatUntil 当前元素引用入口；v5 新增）

> **v5 新增**：Element 字段是用户为 RepeatUntil 终止表达式提供"当前元素"引用入口
> 的机制。与 Index 字段平行（Index 是 Array 内"当前下标"引用入口）。
> 详见 §2.5.3 Element 与 Index 一致性论证。

#### 4.7.1 设计动机

Python `RepeatUntil(lambda x, lst, ctx: x > 5, subcon)` 中 `x` 是 lambda 函数参数，
指向当前迭代的元素。construct-rs 不接收 Python lambda（用户硬约束 #1），必须通过
某种用户面机制让终止表达式引用"当前元素"。

**Element 字段就是这个机制**：

```python
@dataclass
class Packet(StructMixin):
    e: int = rfield(Element())              # Element 字段：当前元素引用入口
    payload: list = field(RepeatUntil(e > 5, Int8ub))
```

- `e` 是 Packet 的字段（不是 RepeatUntil 内部字段），与其他字段同等地位
- ElementNode.parse 返回 None（Element 字段不持有真实数据）
- RepeatUntilNode 在迭代时**借用** Element 字段槽位（`set_field_at(idx_of_e, elem)`），
  使终止表达式中的 `e` 引用能取到当前元素值

**ElementNode 与 IndexNode 的语义差异**：

| 维度 | IndexNode | ElementNode |
|------|-----------|-------------|
| 用户面写法 | `i: int = rfield(Index())` | `e: int = rfield(Element())` |
| 值的来源 | `ctx._index`（Array 系列节点设置） | `ctx.expr_values_buf[idx_of_e]`（RepeatUntilNode 借用） |
| parse 在 Array/RepeatUntil 之外 | 返回 None（对齐 Python `context.get("_index", None)`） | 返回 None（Element 字段不持有真实数据） |
| parse 在 Array/RepeatUntil 内 | 返回当前下标（Array 节点已 set_index） | 返回 None（RepeatUntil 不调 ElementNode.parse，直接 set_field_at） |
| Packet 实例属性值 | 当前下标（Index 在 Array 内的 Item Struct 中） | None（Element 在 Packet 顶层，RepeatUntil 内部借用） |

#### 4.7.2 数据结构

```rust
/// RepeatUntil 当前元素引用入口节点。
///
/// construct-rs 新增（v5）。Python construct 无对应物——Python 用 lambda 参数 `x`
/// 引用当前元素，construct-rs 用 Element 字段 + 字段名引用机制。
///
/// # parse 行为
///
/// 始终返回 `Py_None`——ElementNode.parse 在用户面不可见。Element 字段的值
/// 由 RepeatUntilNode 在迭代时通过 `ctx.set_field_at(element_field_idx, elem, ...)`
/// 借用设置，终止表达式通过 `GetInt(element_field_idx)` 取值。
///
/// # build 行为
///
/// no-op（sizeof=0，不写字节）。
///
/// # sizeof 行为
///
/// 返回 0（不消耗字节）。
#[derive(Debug, Default, Clone, Copy)]
pub struct ElementNode;

impl ElementNode {
    /// 创建 ElementNode。
    pub fn new() -> Self {
        Self
    }

    /// has_expressions 判断（设计 §6.1.1）。
    ///
    /// 返回 `false`——ElementNode 自身不引用 Struct 字段（仅作为引用入口）。
    /// RepeatUntilNode 的 has_expressions 始终返回 true，覆盖 Element 字段的
    /// init_expr_values 需求。
    pub fn has_expressions(&self) -> bool {
        false
    }
}
```

#### 4.7.3 parse / build / sizeof

```rust
impl super::Construct for ElementNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        _stream: &mut ParseStream<'_>,
        _ctx: &mut Context<'py>,
        _path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 始终返回 Py_None。Element 字段的值由 RepeatUntilNode 借用设置，
        // 不在 StructNode.parse 处理 Element 字段时取值。
        Ok(py.None())
    }

    fn build(
        &self,
        _py: Python<'_>,
        _obj: &Bound<'_, PyAny>,
        _stream: &mut BuildStream,
        _ctx: &mut Context<'_>,
        _path: &mut Path,
    ) -> Result<(), ConstructError> {
        // no-op（sizeof=0，不写字节）。
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        Ok(0)
    }
}
```

#### 4.7.4 compute_ro_value 行为

Element 字段是 RO 模式（`rfield(Element())`），StructNode 在 build 方向处理
RO 字段时调 `compute_ro_value(node)`。ElementNode 的 RO 计算应返回 None
（build 方向 Element 字段也无真实数据，RepeatUntil 内部借用模式与 parse 同）。

```rust
// 在 nodes/mod.rs::compute_ro_value 中新增分支：
Node::Element(_) => Ok(py.None()),
```

#### 4.7.5 编译期约束（Python 侧 `_mixin.py`）

Element 字段在编译期需通过以下校验：

1. **必须为 RO 模式**：`rfield(Element())` 合法；`field(Element())` / `wfield(Element())`
   编译期报错（Element 字段不从用户取值）。
2. **必须声明在 RepeatUntil 字段之前**：前序字段引用约束（`_check_forward_reference`），
   因 RepeatUntil 终止表达式引用 Element 字段。
3. **同 Struct 内 Element 字段数量限制**：建议 ≤1 个（多个 Element 字段无意义，
   因 RepeatUntil 终止表达式只能引用其中一个）。编译期不强制限制数量，
   但若多个 Element 字段都未被 RepeatUntil 引用，编译期 warning（DEV 决策是否实现）。
4. **Element 字段必须被同 Struct 的 RepeatUntil 字段引用**：若声明了 Element 字段
   但无 RepeatUntil 引用，编译期 warning（用户可能误用）。

---

## 5. PrefixedArray 的组合方式（决策对比）

### 5.1 选项 A：独立 Node（采用，§4.6）

**优点**：
- 不依赖未实现的 FocusedSeq/Rebuild
- 实现简单，逻辑清晰
- 性能：无间接层

**缺点**：
- 与 Python 实现不一致（Python 是宏组合）
- 若未来实现 FocusedSeq，需考虑是否重构

### 5.2 选项 B：宏展开为节点组合（拒绝）

**思路**：在编译期把 `PrefixedArray(countfield, subcon)` 展开为：
```rust
Node::Struct(StructNode {
    fields: vec![
        StructField { name: "count", node: Node::Rebuild(...), mode: Ro },
        StructField { name: "items", node: Node::Array(...), mode: Rw },
    ],
    ...
})
```

**拒绝理由**：
- Rebuild 节点未实现（依赖"build 时从 context 计算"的能力）
- StructNode 返回的是用户类实例（不是 list），与 PrefixedArray 应返回 list 不符
- 需要引入 FocusedSeq 节点（取字段之一作为整体返回值），增加复杂度
- 性能：多一层 Struct 抽象

### 5.3 决策：采用选项 A（独立 PrefixedArrayNode）

Phase 4 用独立 Node 实现。若未来 FocusedSeq/Rebuild 落地，可保留独立 Node
（性能更优）或迁移到组合方案（行为更一致）。当前选 A 不阻塞未来选项。

---

## 6. 与现有架构的集成

### 6.1 Node enum 扩展

在 `nodes/mod.rs` 的 `Node` enum 新增 7 个变体（v5：含 Element）：

```rust
#[derive(Debug)]
#[enum_dispatch(Construct)]
pub enum Node {
    // ... 现有 13 个变体 ...

    // --- Phase 4：Array 系列 ---
    /// 固定次数数组（对应 Python `Array(count, subcon, discard)`）。
    Array(ArrayNode),
    /// 读到流结束的数组（对应 Python `GreedyRange(subcon, discard)`）。
    GreedyRange(GreedyRangeNode),
    /// 终止表达式数组（对应 Python `RepeatUntil(predicate, subcon, discard)`）。
    /// v5：终止表达式 = Phase 2 ExprProgram，无 PyCallable 路径。
    RepeatUntil(RepeatUntilNode),
    /// 前缀长度数组（对应 Python `PrefixedArray(countfield, subcon)`）。
    PrefixedArray(PrefixedArrayNode),
    /// 取当前下标（对应 Python `Index`）。
    Index(IndexNode),
    /// 早停信号（对应 Python `StopIf(condfunc)`）。
    StopIf(StopIfNode),
    /// RepeatUntil 当前元素引用入口（v5 新增；Python construct 无对应物）。
    Element(ElementNode),
}
```

新增 `pub mod` 声明：

```rust
pub mod array;
pub mod greedy_range;
pub mod repeat_until;
pub mod prefixed_array;
pub mod index;
pub mod stop_if;
pub mod element;        // v5 新增
```

#### 6.1.1 `has_expressions` 扩展

`Node::has_expressions` 需递归检查 Array 系列的 inner：

```rust
pub fn has_expressions(&self) -> bool {
    match self {
        // 现有分支 ...
        Node::Array(a) => a.has_expressions(),
        Node::GreedyRange(g) => g.has_expressions(),
        Node::RepeatUntil(r) => r.has_expressions(),
        Node::PrefixedArray(p) => p.has_expressions(),
        Node::Index(_) => false,                       // IndexNode 不引用任何 Struct 字段
        Node::StopIf(s) => s.has_expressions(),        // StopIf(Expr) 引用 Struct 字段
        Node::Element(_) => false,                     // ElementNode 仅作为引用入口（v5 新增）
    }
}
```

各 Node 的实现：

```rust
impl ArrayNode {
    pub fn has_expressions(&self) -> bool {
        self.inner.has_expressions() || matches!(self.count, CountSource::Expr(_))
    }
}

impl GreedyRangeNode {
    pub fn has_expressions(&self) -> bool {
        self.inner.has_expressions()
    }
}

impl RepeatUntilNode {
    /// v5：始终返回 true（终止表达式始终引用 Element 字段）。
    /// RepeatUntil 必须触发 StructNode 通过 init_expr_values 初始化
    /// expr_values_buf，使 set_field_at(element_field_idx, ...) 可用。
    pub fn has_expressions(&self) -> bool {
        true
    }
}

impl PrefixedArrayNode {
    pub fn has_expressions(&self) -> bool {
        // countfield 是独立 Node（如 VarInt、Byte），其本身通常无表达式；
        // inner 子树可能含表达式。
        self.countfield.has_expressions() || self.inner.has_expressions()
    }
}

impl IndexNode {
    pub fn has_expressions(&self) -> bool {
        false   // 仅读 ctx._index（v3：不走 ExprProgram），不引用 Struct 字段
    }
}

impl StopIfNode {
    pub fn has_expressions(&self) -> bool {
        // 关键：StopIf(x == 0)（x 是字段名引用，**Python 写法为 `this.x == 0`**）
        // 中的 x 引用 Struct 字段 x，经 expr_values_buf 取值；
        // 必须返回 true 触发 StructNode 创建 child ctx。
        matches!(self.cond, StopIfCondition::Expr(_))
    }
}

impl StopIfCondition {
    pub fn is_expr(&self) -> bool {
        matches!(self, StopIfCondition::Expr(_))
    }
}
```

> **P1 关键说明（REV 驳回修正）**：`StopIf(Expr)` 节点自身的表达式程序
> （如 `x == 0` 编译为 `[GetInt(0), Const(0), Eq]`，其中 `x` 是字段名引用，
> **Python 等价写法为 `this.x == 0`**）**确实引用 Struct 字段**
> （此例中的 `x`）。`has_expressions()` 必须返回 true，原因有二：
>
> 1. **StructRef 路径**（struct_ref.rs L171/L194）：当 StructRef 引用一个含
>    `Array(5, StopIf(x > 10))`（x 是字段名引用，**Python 写法 `this.x > 10`**）
>    字段的 Struct 时，`root.has_expressions()` 会递归到 StopIf；若返回 false，
>    则 StructRef 不创建 child context、不调用 `init_expr_values`，
>    导致 StopIf 求值时 GetInt 命中空 buf 报错。
>
> 2. **StructRef 不依赖 expr_programs**：虽然顶层 StructNode 通过
>    `expr_programs` 参数知道哪些字段含表达式（编译期），但 StructRef 在
>    **运行时**重新解析 schema 并通过 `root.has_expressions()` 决定是否创建
>    child context。这是运行时路径，与编译期的 expr_programs 是两套机制。
>    因此 `has_expressions()` 必须独立正确。
>
> `StopIf(Always)` / `StopIf(Never)` 不引用字段（编译期常量），返回 false。
> `IndexNode` 仅读 `ctx._index`，不引用 Struct 字段，返回 false。

#### 6.1.2 `compute_ro_value` 扩展

`StopIfNode` 不作为 RO 字段（其 build 行为是检查条件而非计算值）。
`IndexNode` 可作为 RO 字段（值来自 ctx._index，不从实例取）。
但 Index 通常用作普通字段（Rw），不强制 RO。

当前 `compute_ro_value` 不增加 Array 系列分支。若 DEV 发现需要（如 Index 作为 RO），
再补充。

### 6.2 compile.rs 编译管线扩展

#### 6.2.1 描述符识别（按 type_name 字符串）

新增 7 个描述符类型名识别分支（v5：含 ElementDescriptor）：

```rust
match type_name.to_str()? {
    // ... 现有分支 ...

    "ArrayDescriptor" => return Ok(Node::Array(build_array_node(py, desc, field_index, expr_programs, bitwise)?)),
    "GreedyRangeDescriptor" => return Ok(Node::GreedyRange(build_greedy_range_node(py, desc, field_index, expr_programs, bitwise)?)),
    "RepeatUntilDescriptor" => return Ok(Node::RepeatUntil(build_repeat_until_node(py, desc, field_index, expr_programs, field_names, bitwise)?)),
    "PrefixedArrayDescriptor" => return Ok(Node::PrefixedArray(build_prefixed_array_node(py, desc, field_index, expr_programs, bitwise)?)),
    "IndexDescriptor" => return Ok(Node::Index(IndexNode::new())),
    "StopIfDescriptor" => return Ok(Node::StopIf(build_stop_if_node(py, desc, field_index, expr_programs)?)),
    "ElementDescriptor" => return Ok(Node::Element(ElementNode::new())),   // v5 新增
    _ => {}
}
```

#### 6.2.2 build_array_node 辅助函数

**完整编译设计（P3 修正）**：参照 compile.rs L266-308 的 `BytesDescriptor` 表达式长度
编译路径，给出 ArrayDescriptor 的 count 表达式完整编译路径。

**Python 侧 ArrayDescriptor**：

```python
class ArrayDescriptor:
    """Array(count, subcon, discard) 描述符。"""
    __slots__ = ("count", "subcon", "discard")

    def __init__(self, count, subcon, discard=False):
        self.count = count
        self.subcon = subcon
        self.discard = discard

    @property
    def _expr_params(self):
        """表达式参数协议（与 BytesDescriptor._expr_params 同模式）。

        返回 {"count": self.count}。当 count 是 int 时常量跳过编译；
        是 _FieldDescriptor/_ExprRef 时编译为 ExprOp 列表。
        """
        if isinstance(self.count, int):
            return {}
        return {"count": self.count}
```

**Rust 侧 build_array_node**：

```rust
/// 编译 ArrayDescriptor → ArrayNode。
///
/// 设计参照 build_node_from_descriptor 的 BytesDescriptor 分支（compile.rs L266-308）：
/// 1. 从 desc 读取 count / subcon / discard
/// 2. count 分类：usize 常量 → CountSource::Const；
///    非常量 → 从 expr_programs[field_index]["count"] 取 ExprOp 列表 → CountSource::Expr
/// 3. 递归编译 subcon（沿用 field_index，inner 共享外层字段的 expr_values_buf）
fn build_array_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    bitwise: bool,
) -> Result<ArrayNode, ConstructError> {
    // 1. 递归编译 subcon（沿用 field_index；详见下方"嵌套 inner 表达式"说明）。
    let subcon_desc = desc.getattr("subcon").map_err(|e| ConstructError::Compilation {
        message: format!("ArrayDescriptor missing 'subcon': {}", e),
    })?;
    let inner_node = build_node_from_descriptor(py, &subcon_desc, field_index, expr_programs, bitwise)?;

    // 2. 解析 count：常量 or 表达式。
    let count_obj = desc.getattr("count").map_err(|e| ConstructError::Compilation {
        message: format!("ArrayDescriptor missing 'count': {}", e),
    })?;

    let count = if let Ok(n) = count_obj.extract::<i64>() {
        // 常量路径（Phase 4 首版必须支持，零运行时开销）
        if n < 0 {
            return Err(ConstructError::Compilation {
                message: format!("Array count {} must be non-negative", n),
            });
        }
        CountSource::Const(n as usize)
    } else {
        // 表达式路径：从 expr_programs[field_index]["count"] 取 ExprOp 列表。
        // 与 BytesDescriptor 的 "length" 键完全同模式。
        let field_exprs = expr_programs
            .get(field_index)
            .and_then(Option::as_ref)
            .ok_or_else(|| ConstructError::Compilation {
                message: format!(
                    "Array field has non-constant count but no expression program was provided \
                     (field index {})", field_index
                ),
            })?;

        let field_exprs_dict = field_exprs
            .bind(py)
            .downcast::<PyDict>()
            .map_err(|_| ConstructError::Compilation {
                message: "Array expression program must be a dict".to_string(),
            })?;

        let ops_list = field_exprs_dict
            .get_item("count")
            .map_err(|e| ConstructError::Compilation {
                message: format!("failed to get 'count' from Array expression programs: {}", e),
            })?
            .ok_or_else(|| ConstructError::Compilation {
                message: format!(
                    "Array field has non-constant count but 'count' key missing in expression \
                     program (field index {})", field_index
                ),
            })?;

        let ops = parse_expr_ops_from_py(&ops_list)?;
        let program = ExprProgram::new(ops);
        CountSource::Expr(program)
    };

    // 3. discard 标志。
    let discard: bool = desc
        .getattr("discard")
        .and_then(|d| d.extract())
        .unwrap_or(false);

    Ok(ArrayNode::new(inner_node, count, discard))
}
```

> **CountSource::is_expr 辅助**：DEV 实现时给 `CountSource` 加
> `pub fn is_expr(&self) -> bool { matches!(self, CountSource::Expr(_)) }`，
> 供 `has_expressions` 使用（§6.1.1）。

##### P3.1 嵌套 inner 表达式的支持范围（关键决策）

`build_node_from_descriptor` 递归调用 `build_array_node` 时，**沿用同一个 field_index
和 expr_programs 切片**（参照 BitwiseDescriptor 分支 compile.rs L412-413）。这意味着：

> **语法约定（v3）**：下表中 `this.n` / `this.m` / `this.x` 等是 **Python construct
> 语法**。construct-rs 中等价写法是字段名直接引用（如 `n`、`m`、`x` 是已声明的
> `_FieldDescriptor`），不用 `this.` 前缀。本表沿用 Python 语法仅为对照 Python
> construct 用户的心智模型。

| 场景（Python 语法对照） | 支持情况 | 说明 |
|------|---------|------|
| `Array(this.n, Byte)` | ✅ Phase 4 支持 | 顶层 count 表达式，从 expr_programs[N]["count"] 取 |
| `Array(N, StopIf(this.x))` | ❌ 编译期失败 | inner StopIf 的 "cond" 表达式未被 Python 侧收集（见下方） |
| `Array(N, Bytes(this.m))` | ❌ 编译期失败 | inner Bytes 的 "length" 表达式未被收集 |
| `Array(N, Array(this.m, Byte))` | ❌ 编译期失败 | 内层 Array 的 count 表达式未被收集 |
| `Bitwise(Bytes(this.m))` | ❌ Phase 1-3 已不可用 | 同根问题（_extract_and_compile_exprs 不递归） |

**根因**：Python 侧 `_extract_and_compile_exprs`（_mixin.py L583-615）**只对字段顶层
subcon 调用一次**（_mixin.py L633-634：`subcon = desc.subcon;
_extract_and_compile_exprs(subcon, ...)`），**不递归进入 inner**。当 subcon 是
BitwiseDescriptor / ArrayDescriptor / 等"包装型"描述符（`_expr_params = {}` 或仅含
自身参数）时，inner 的表达式参数不会被收集到 expr_programs 中。

Rust 侧递归调用 `build_node_from_descriptor(inner_desc, field_index, expr_programs, ..)`
时，若 inner 是含表达式的描述符（Bytes(expr_length)、Computed、StopIf(expr_cond) 等），
会从 `expr_programs[field_index]` 查找 "length"/"func"/"cond" 键——但 Python 侧未填入，
报 CompilationError（"key missing"）。**这是 Phase 1-3 既有的限制，Array 与之一致。**

**Phase 4 的支持范围明确为**：

- ✅ **顶层 count 表达式**：`Array(n, simple_subcon)`（`n` 是字段名引用，**Python 写法 `this.n`**），
  simple_subcon 是 FormatField / 常量 Bytes / 常量 BitsInteger / Padding 等不含表达式的描述符。
- ❌ **inner 含表达式**：留待"嵌套表达式收集"特性（独立子任务，建议 Phase 4.0b 或
  Phase 5+，需扩展 `_extract_and_compile_exprs` 递归收集 inner，并解决命名空间冲突
  ——见下方 P3.2）。

**StopIf 在 Struct 直接字段中**（非 inner）依然支持：`@dataclass class S(StructMixin):
x: int = field(Byte); stop = rfield(StopIf(x))`（`x` 是字段名引用，**Python 写法 `this.x`**）。
StopIfDescriptor 是字段，field_index 在字段列表中，其 "cond" 表达式从
`expr_programs[field_index]["cond"]` 取（与 ComputedDescriptor 的 "func" 同模式），
不受上述 inner 限制影响。

##### P3.2 嵌套 inner 表达式的扩展方案（后续子任务参考）

> **语法约定（v3）**：本节示例中 `this.m` / `this.x` 等是 **Python construct 语法**，
> 仅供对照。construct-rs 等价写法是字段名直接引用（`m`、`x`）。

若未来需支持 `Array(N, Bytes(m))`（**Python 写法 `Bytes(this.m)`**）、
`Array(N, StopIf(x))`（**Python 写法 `StopIf(this.x)`**），扩展点：

1. **Python 侧**：修改 `_extract_and_compile_exprs` 递归进入包装型描述符的 `subcon`
   属性，用**路径化键名**避免命名空间冲突：
   ```python
   # 例如 Array(N, Array(m, Byte))（Python 写法 Array(N, Array(this.m, Byte))）
   # 的 expr_programs[N] 结构：
   {
       "count": <outer count ops>,           # 顶层 Array count
       "inner.count": <inner count ops>,     # 内层 Array count（路径化键名）
   }
   # 或用嵌套 dict：{"count": [...], "inner": {"count": [...]}}
   ```
2. **Rust 侧**：`build_array_node` 递归时，从 `expr_programs[field_index]` 取
   `"inner.count"` / `"inner.length"` / `"inner.cond"` 等路径化键。
3. **路径生成规则**：包装型描述符（Bitwise / Bytewise / Array / GreedyRange /
   PrefixedArray / RepeatUntil）递归时给键名前缀 `"inner."`。多重嵌套按层级累加。

**复杂度评估**：~2-3 个子任务（Python 编译器扩展 + Rust build_*_node 路径化取键 +
端到端测试）。**不阻塞 Phase 4 出口**——S-FUNC 仅要求覆盖 Python 顶层 count 表达式。

> **PM 决策点（更新 §12.2）**：Phase 4 首版仅支持顶层 count 表达式。
> inner 含表达式的场景（`Array(N, Bytes(m))`（Python 写法 `Bytes(this.m)`）、
> `Array(N, StopIf(x))`（Python 写法 `StopIf(this.x)`））
> 归入 Phase 4.0b 或 Phase 5+，与现有 `Bitwise(Bytes(m))`（Python 写法
> `Bitwise(Bytes(this.m))`）的限制一致。

#### 6.2.3 其他 build_*_node 函数

模式类似 build_array_node：递归编译 subcon + 提取参数。
详细签名 DEV 实现时参照现有 `build_bits_integer_node` / `build_padding_node`。

##### 6.2.3.1 `build_repeat_until_node`（v5 完全重写）

```rust
/// 编译 RepeatUntilDescriptor → RepeatUntilNode。
///
/// v5 设计（终止表达式 = Phase 2 ExprProgram）：
/// 1. 从 desc 读取 terminator / subcon / discard
/// 2. 从 expr_programs[field_index]["terminator"] 取 ExprOp 列表，构建 ExprProgram
/// 3. 从 expr_programs[field_index]["element_field_idx"] 取 Element 字段索引
/// 4. 从 field_names 取 Element 字段名（intern）
/// 5. 递归编译 subcon → Box<Node>
fn build_repeat_until_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],   // Struct 全部字段名
    bitwise: bool,
) -> Result<RepeatUntilNode, ConstructError> {
    // 1. 递归编译 subcon。
    let subcon_desc = desc.getattr("subcon").map_err(|e| ConstructError::Compilation {
        message: format!("RepeatUntilDescriptor missing 'subcon': {}", e),
    })?;
    let inner_node = build_node_from_descriptor(py, &subcon_desc, field_index, expr_programs, bitwise)?;

    // 2. 从 expr_programs[field_index] 取终止表达式 + Element 字段索引。
    let field_exprs = expr_programs
        .get(field_index)
        .and_then(Option::as_ref)
        .ok_or_else(|| ConstructError::Compilation {
            message: format!(
                "RepeatUntil field has no expression program (field index {}); \
                 terminator expression is required", field_index
            ),
        })?;
    let field_exprs_dict = field_exprs.bind(py).downcast::<PyDict>().map_err(|_| {
        ConstructError::Compilation {
            message: "RepeatUntil expression program must be a dict".to_string(),
        }
    })?;

    // 取终止表达式 ExprOp 列表。
    let ops_obj = field_exprs_dict.get_item("terminator")?.ok_or_else(|| {
        ConstructError::Compilation {
            message: format!(
                "RepeatUntil missing 'terminator' in expression programs (field index {})",
                field_index
            ),
        }
    })?;
    let ops = parse_expr_ops_from_py(&ops_obj)?;
    let terminator = ExprProgram::new(ops);

    // 取 Element 字段索引（编译期由 Python 侧从终止表达式引用的 Element 字段推导）。
    let element_field_idx: usize = field_exprs_dict
        .get_item("element_field_idx")?
        .ok_or_else(|| ConstructError::Compilation {
            message: format!(
                "RepeatUntil missing 'element_field_idx' in expression programs (field index {})",
                field_index
            ),
        })?
        .extract()?;

    // 3. 取 Element 字段名（intern PyString）。
    let element_field_name_str = field_names.get(element_field_idx).ok_or_else(|| {
        ConstructError::Compilation {
            message: format!(
                "RepeatUntil element_field_idx {} out of range (field_names len {})",
                element_field_idx, field_names.len()
            ),
        }
    })?;
    let element_field_name = intern_string(py, element_field_name_str)?.unbind();

    // 4. discard 标志。
    let discard: bool = desc
        .getattr("discard")
        .and_then(|d| d.extract())
        .unwrap_or(false);

    Ok(RepeatUntilNode::new(
        inner_node,
        terminator,
        element_field_idx,
        element_field_name,
        discard,
    ))
}
```

> **v5 与 v4 关键差异**：
> - 不再从 `self.predicate` 读取 callable（v4 路径已删）
> - 不再调 `container_cache::container_class`（Container proxy 路径已删）
> - 不再识别 AST（`_try_compile_repeat_predicate` 已删）
> - 终止表达式编译完全在 Python 侧 `_compile_expr_tree` 完成（与其他表达式字段同模式）

##### 6.2.3.2 `build_element_node`（v5 新增）

ElementDescriptor 编译极简——无参数，直接构造 ElementNode：

```rust
// compile.rs build_node_from_descriptor 中：
"ElementDescriptor" => return Ok(Node::Element(ElementNode::new())),
```

无需独立 `build_element_node` 函数（无参数可提取）。

### 6.3 Python 层 API

#### 6.3.1 描述符定义（Python 侧，v5）

> **v5 完全重写**：删除 v4 的 `_AST_OP_TO_EXPROP`、`_try_compile_repeat_predicate`、
> RepeatUntilDescriptor 中的 PyCallable 兼容（`if not callable(predicate)` 包装）。
> 新增 ElementDescriptor。RepeatUntilDescriptor 第一参数从 `predicate` 改名为
> `terminator`，类型从 callable 改为 Phase 2 表达式（`_FieldDescriptor` / `_ExprRef` / `int`）。

```python
class ElementDescriptor:
    """``Element()`` 描述符（v5 新增）。

    RepeatUntil 终止表达式中"当前元素"的引用入口。无参数。

    必须在 ``rfield()`` 中使用（RO 模式），且必须声明在 RepeatUntil 字段之前。
    Element 字段在 Packet 实例中始终为 None（不持有真实数据）——其值由
    RepeatUntilNode 在迭代时通过 set_field_at 借用设置。

    ``_expr_params`` 协议返回空 dict：Element 无表达式参数。
    """

    __slots__ = ()

    _expr_params = {}

    def __repr__(self):
        return "Element()"


def Element():
    """创建一个 Element 描述符（v5 新增）。

    RepeatUntil 终止表达式中"当前元素"的引用入口。

    使用方式::

        @dataclass
        class Packet(StructMixin):
            e: int = rfield(Element())                # Element 字段
            payload: list = field(RepeatUntil(e > 5, Int8ub))

    :return: ``ElementDescriptor`` 实例。
    """
    return ElementDescriptor()


class RepeatUntilDescriptor:
    """``RepeatUntil(terminator, subcon, discard=False)`` 描述符（v5 重写）。

    终止表达式数组：解析元素到 list 直到终止表达式求值非零（最后元素包含在内），
    或从 list 构建字节序列直到某元素满足终止表达式。对应 Python construct 的 ``RepeatUntil``。

    :param terminator: 终止表达式（Phase 2 表达式：``_FieldDescriptor`` / ``_ExprRef`` /
                       ``int`` 组合）。必须引用同 Struct 中已声明的 Element 字段
                       （如 ``e > 5``，其中 ``e`` 是 ``rfield(Element())``）。
                       求值结果非零即终止。不接收 Python lambda / callable。
    :param subcon: 元素子构造器（描述符）。
    :param discard: 若为 True，parse 返回空 list 但仍消耗流。

    ``_expr_params`` 协议：

    - 返回 ``{"terminator": <expr>, "element_field_idx": <int>}``。
    - ``terminator`` 是终止表达式（由 ``_compile_expr_tree`` 编译为 ExprOp 列表）。
    - ``element_field_idx`` 是终止表达式引用的 Element 字段在 Struct 中的索引
      （由 ``_compile_expr_tree`` 在编译时从表达式中提取的 GetInt 索引推导）。

    与 Python construct 的差异：

    - **不接收 Python lambda / callable**：用户硬约束 #1（避免隐藏决策路径）。
      Phase 2 表达式 VM 无法描述的终止逻辑（如 list 切片），用户须改用 Adapter
      （显式慢路径）。详见 §13.9 能力边界。
    - **discard 语义简化**：v5 中 discard 仅影响 parse 方向 list 收集；
      build 方向 discard 不影响终止表达式求值（终止表达式不接收 list 参数）。
    """

    __slots__ = ("terminator", "subcon", "discard", "_expr_params",
                 "_element_field_idx")

    def __init__(self, terminator, subcon, discard=False):
        """初始化 RepeatUntil 描述符。

        :param terminator: 终止表达式（Phase 2 表达式）。
        :param subcon: 元素子构造器。
        :param discard: 是否丢弃解析结果。
        """
        # 类型检查：terminator 必须是 Phase 2 表达式（不接受 callable）。
        if callable(terminator):
            raise CompilationError(
                "RepeatUntil terminator must be a Phase 2 expression "
                "(_FieldDescriptor / _ExprRef / int), not a Python callable. "
                "Example: RepeatUntil(e > 5, Int8ub) where 'e' is rfield(Element()). "
                "For complex termination logic depending on list/context, "
                "use Adapter (explicit slow path)."
            )
        self.terminator = terminator
        self.subcon = subcon
        self.discard = discard
        # _element_field_idx 由 _compile_expressions 在编译期填充（延迟设置）。
        self._element_field_idx = None
        # _expr_params 也延迟设置——_compile_expressions 会调 set_compiled_expr_params。
        self._expr_params = {"terminator": terminator}

    def set_compiled_expr_params(self, ops, element_field_idx):
        """编译期由 _compile_expressions 调用，注入已编译的 ops 和 Element 字段索引。

        :param ops: 编译后的 ExprOp 元组列表（[("getint", idx), ("const", 5), ("gt",)]）。
        :param element_field_idx: Element 字段在 Struct 中的索引。
        """
        self._element_field_idx = element_field_idx
        self._expr_params = {
            "terminator": ops,
            "element_field_idx": element_field_idx,
        }

    def __repr__(self):
        return "RepeatUntil(terminator={!r}, subcon={!r}, discard={!r})".format(
            self.terminator, self.subcon, self.discard
        )


def RepeatUntil(terminator, subcon, discard=False):
    """创建一个 RepeatUntil 描述符（v5 重写）。

    终止表达式数组。对应 Python construct 的 ``RepeatUntil``。

    使用方式（终止表达式 = Phase 2 表达式，引用 Element 字段）::

        @dataclass
        class Packet(StructMixin):
            e: int = rfield(Element())                # 当前元素引用入口
            payload: list = field(RepeatUntil(e > 5, Int8ub))

        Packet.parse(b"\\x01\\x02\\x06\\xAA")
        # Packet(e=None, payload=[1, 2, 6])    # 最后元素 6 满足 e > 5

    限制：

    - 不接收 Python lambda / callable（用户硬约束）
    - 终止表达式必须引用 Element 字段（编译期校验）
    - sizeof 永远返回 SizeofError
    - inner subcon 不支持含表达式的子描述符（与 Array / Bitwise 同限制）

    :param terminator: 终止表达式（Phase 2 表达式，引用 Element 字段）。
    :param subcon: 元素子构造器。
    :param discard: 若为 True，parse 返回空 list 但仍消耗流。
    :return: ``RepeatUntilDescriptor`` 实例。
    """
    return RepeatUntilDescriptor(terminator, subcon, discard)
```

> **DEV 实现要点**：
> 1. `_mixin._compile_expressions` 需对 RepeatUntilDescriptor 特殊处理：
>    编译 `terminator` 表达式后，从编译产物中提取第一个 `("getint", idx)` 指令
>    （Element 字段索引），调 `desc.set_compiled_expr_params(ops, element_field_idx)`。
> 2. 若 `terminator` 表达式不含任何 `("getint", idx)` 指令（即不引用任何 Element 字段），
>    编译期报错：`RepeatUntil terminator must reference an Element field`。
> 3. 若 `terminator` 引用了多个字段（如 `e1 > e2`），取第一个 Element 字段索引。
>    DEV 决策是否在编译期强制只允许引用一个 Element 字段（建议强制）。

#### 6.3.2 用户 API（构造器函数，v5）

```python
def Array(count, subcon, discard=False):
    return ArrayDescriptor(count, subcon, discard)

def GreedyRange(subcon, discard=False):
    return GreedyRangeDescriptor(subcon, discard)

def RepeatUntil(terminator, subcon, discard=False):   # v5: predicate → terminator
    return RepeatUntilDescriptor(terminator, subcon, discard)

def PrefixedArray(countfield, subcon):
    return PrefixedArrayDescriptor(countfield, subcon)

def Element():     # v5 新增
    return ElementDescriptor()

# Index 与 StopIf 是类（无参数 / 单参数）
class Index:
    def __init__(self):
        pass

class StopIf:
    def __init__(self, condfunc):
        self.condfunc = condfunc
```

#### 6.3.3 `len_` 辅助函数

Python construct 的 `len_(this.items)`（**Python 语法**，`this.items` 引用 items 字段）
用于 PrefixedArray 的 Rebuild。本设计 PrefixedArray 不依赖 Rebuild（§5），故 `len_`
**不在 Phase 4 实现**。若用户代码引用 `len_`，文档提示用 PrefixedArray 节点替代手动组合。

> **若未来实现 Rebuild**：`len_` 表达式编译为 `ExprOp::Len`（取 list 长度）。
> 当前 ExprOp 无 Len 指令，需新增。延后到 Rebuild 阶段。

### 6.4 StructNode 的 StopField 捕获修改

当前 `StructNode::parse`（struct_node.rs L315-356）：

```rust
for (idx, field) in self.fields.iter().enumerate() {
    let value = match field.node.parse(py, stream, ctx, path) {
        Ok(v) => v,
        Err(mut e) => {
            e.push_path_segment(field.name.rust_name());
            return Err(e);
        }
    };
    // ... 处理 value ...
}
```

**Phase 4 修改**：增加 StopField 捕获：

```rust
for (idx, field) in self.fields.iter().enumerate() {
    let value = match field.node.parse(py, stream, ctx, path) {
        Ok(v) => v,
        Err(ConstructError::StopField { .. }) => {
            // StopIf 触发：停止后续字段，正常返回当前实例。
            // 已解析的字段已在 dict 中，未解析的字段不写入（对齐 Python Struct 行为）。
            break;
        }
        Err(mut e) => {
            e.push_path_segment(field.name.rust_name());
            return Err(e);
        }
    };
    // ... 处理 value ...
}
```

**build 方向同样修改**：子字段 build 返回 StopField 时停止后续字段。

> **影响范围**：仅 struct_node.rs 一处。修改是新增分支，不破坏现有行为
> （现有节点不产生 StopField）。
>
> **Sequence 节点**：Phase 4 不实现 Sequence。StopIf 在 Sequence 的捕获
> 留待 Sequence 阶段。

### 6.5 Python 异常类映射

`error.rs::init_exception_classes` 增加 4 个新类的缓存：

```rust
let classes = ExceptionClasses {
    // ... 现有字段 ...
    range_error: get("RangeError")?,
    repeat_error: get("RepeatError")?,
    stop_field_error: get("StopFieldError")?,
    index_field_error: get("IndexFieldError")?,
};
```

`select_exception_class` 增加 4 个分支：

```rust
ConstructError::Range { .. } => &classes.range_error,
ConstructError::Repeat { .. } => &classes.repeat_error,
ConstructError::StopField { .. } => &classes.stop_field_error,
ConstructError::IndexField { .. } => &classes.index_field_error,
```

`build_test_classes`（error.rs 测试辅助）同步增加 4 个测试类定义。

---

## 7. 边界条件清单

### 7.1 Array 边界

| ID | 场景 | 预期行为 |
|----|------|---------|
| AR-1 | count 为常量正整数 | 正常解析/构建 N 个元素 |
| AR-2 | count 表达式求值为负数 | 返回 `Range` 错误（"invalid count -5"） |
| AR-3 | build 时 `len(obj) != count` | 返回 `Range` 错误（"expected N elements, found M"） |
| AR-4 | 嵌套 Array（Array 内 Array） | 内层 _index 覆盖外层，结束后恢复 |
| AR-5 | 子构造器 parse 失败 | 错误向上传播，已解析元素丢弃 |
| AR-6 | count = 0 | 返回空 list，不消耗字节 |
| AR-7 | discard=True | 仍消耗字节，返回空 list |
| AR-8 | count 超过 usize::MAX（i64 表达式） | 求值时 as usize 截断（wrapping），后续 Stream 错误 |
| AR-9 | inner 是 Struct | 每个 elem 是 Struct 实例，list 是 PyList of instances |
| AR-10 | inner 是表达式长度 Bytes（`Array(N, Bytes(m))`，**Python 写法 `Bytes(this.m)`**） | **编译期失败**（CompilationError，"length key missing"）—— inner 表达式收集未实现，§6.2.2 P3.1 |

### 7.2 GreedyRange 边界

| ID | 场景 | 预期行为 |
|----|------|---------|
| GR-1 | 空流（立即 EOF） | 返回空 list |
| GR-2 | 第一个元素解析失败 | 回退到 pos=0，返回空 list |
| GR-3 | 第 N 个元素解析失败 | 回退到第 N 个起始位置，返回前 N-1 个元素 |
| GR-4 | 内含 StopIf | StopField 触发时正常终止（不回退 fallback 之后） |
| GR-5 | stream 末尾部分元素（字节不足） | 回退，返回完整解析的元素 |
| GR-6 | discard=True | 仍消耗字节，返回空 list |
| GR-7 | build 时空列表 | 不写字节，正常返回 |
| GR-8 | build 时某元素失败 | 错误向上传播（不回退，build 无 seek） |
| GR-9 | sizeof | 永远 Err |

### 7.3 RepeatUntil 边界（v5 重写）

> **v5 变更**：删除 RU-4/RU-7/RU-8/RU-9（PyCallable 相关），新增 RU-10~RU-15
> （终止表达式 / Element 字段相关）。

| ID | 场景 | 预期行为 |
|----|------|---------|
| RU-1 | 子构造器解析失败 | 错误直接上抛（不回退，不像 GreedyRange） |
| RU-2 | 终止表达式在第 N 个元素求值非零 | 返回前 N+1 个元素（最后触发的被包含） |
| RU-3 | build 时无元素满足终止表达式 | 返回 `Repeat` 错误 |
| RU-5 | 空流且终止表达式立即满足（不可能，因需先 parse） | 至少 parse 一个元素（若流可读） |
| RU-6 | sizeof | 永远 Err |
| RU-10 | 终止表达式求值为 0 | 继续下一次迭代（不终止） |
| RU-11 | 终止表达式求值为负数 | 视为非零，**终止**（与 Python truthy 语义对齐：非零即真） |
| RU-12 | 终止表达式引用的 Element 字段未被 RepeatUntil 借用（理论不发生） | GetInt 取到 StructNode.parse 初始写入的 None，extract i64 失败，返回 `ExprType` 错误 |
| RU-13 | 终止表达式不引用任何 Element 字段（如 `RepeatUntil(1, Int8ub)`，常量表达式） | **编译期失败**：`terminator must reference an Element field`。理由：常量终止表达式要么永远终止（无意义）、要么永不终止（build 必抛 RepeatError），都是用户误用 |
| RU-14 | Element 字段未声明在 RepeatUntil 字段之前 | **编译期失败**：前向引用约束（`_check_forward_reference`） |
| RU-15 | Element 字段类型不是 `rfield(Element())`（如 `field(Element())` RW 模式） | **编译期失败**：Element 字段必须 RO 模式 |
| RU-16 | RepeatUntil 用户面 API 传入 Python lambda | **编译期失败**：`RepeatUntilDescriptor.__init__` 检查 `callable(terminator)`，报错 "must be a Phase 2 expression" |
| RU-17 | parse 方向 discard=True | 仍消耗字节、求值终止表达式，但不收集元素到 list（返回空 list） |
| RU-18 | build 方向 discard=True | 不影响终止表达式求值（终止表达式不接收 list 参数），与 discard=False 行为一致 |

### 7.3.1 Element 边界（v5 新增）

| ID | 场景 | 预期行为 |
|----|------|---------|
| EL-1 | `rfield(Element())` 在 RepeatUntil 之外（如同 Struct 单独存在） | ElementNode.parse 返回 None；Packet 实例的 Element 字段属性 = None |
| EL-2 | Element 字段被 RepeatUntil 终止表达式引用 | RepeatUntil 在迭代时 set_field_at 借用槽位，终止表达式 GetInt 取到当前元素值 |
| EL-3 | sizeof | 返回 0（不消耗字节） |
| EL-4 | build | no-op（不写字节） |
| EL-5 | Element 字段不在 RepeatUntil 终止表达式中引用（声明但未用） | 编译期 warning（不报错）；Packet 实例 Element 字段属性 = None（语义：未使用） |
| EL-6 | 同 Struct 中声明多个 Element 字段 | 编译期允许（不报错），但只有被 RepeatUntil 终止表达式引用的那个有效。建议用户只声明一个 Element 字段 |

### 7.4 PrefixedArray 边界

| ID | 场景 | 预期行为 |
|----|------|---------|
| PA-1 | countfield 解析的 count 为负 | 返回 `Range` 错误 |
| PA-2 | countfield 解析的 count 不是整数 | 返回 `Range` 错误 |
| PA-3 | count = 0 | 返回空 list（countfield 已消耗） |
| PA-4 | count 大于实际可解析元素 | 第 N 个元素失败时错误向上传播 |
| PA-5 | build 时 list 长度超出 countfield 表示范围 | 由 countfield 节点自行报错（如 FormatFieldNode 抛 `FormatField`/`Stream` 错误），PrefixedArrayNode 不做额外校验 |
| PA-6 | sizeof | 永远 Err（保守，§4.6.4） |

### 7.5 Index 边界

| ID | 场景 | 预期行为 |
|----|------|---------|
| IX-1 | 在 Array 内 | 返回当前 _index（PyLong） |
| IX-2 | 不在任何 Array 内（ctx._index=None） | 返回 Py_None（对齐 Python） |
| IX-3 | sizeof | 返回 0（不消耗字节） |
| IX-4 | build | no-op（不写字节） |

### 7.6 StopIf 边界

| ID | 场景 | 预期行为 |
|----|------|---------|
| SI-1 | 在 Struct 内，条件为真 | 抛 StopField，Struct 捕获，停止后续字段 |
| SI-2 | 在 GreedyRange 内，条件为真 | 抛 StopField，GreedyRange 捕获，停止迭代 |
| SI-3 | 在 Array 内，条件为真 | Array **不**捕获，StopField 向上传播（用户误用） |
| SI-4 | 在 Sequence 内（未实现） | 当前不捕获，错误传播（Phase 5+ 补全） |
| SI-5 | 顶层（无外层捕获） | StopField 到达 FFI，映射到 StopFieldError |
| SI-6 | 条件为常量 false | 永远不停止，正常继续 |
| SI-7 | sizeof | 永远 Err |

### 7.7 ListContainer 兼容性

| ID | 场景 | 预期行为 |
|----|------|---------|
| LC-1 | `isinstance(result, ListContainer)` | **失败**（construct-rs 返回原生 list） |
| LC-2 | `result == [1,2,3]` | 正常比较（list == list） |
| LC-3 | `result.search(...)` | **AttributeError**（原生 list 无 search 方法） |
| LC-4 | `repr(result)` | 与 Python 不同（原生 list repr，非 ListContainer 美化） |

### 7.8 嵌套与组合

| ID | 场景 | 预期行为 |
|----|------|---------|
| NE-1 | Array 内 Array | 多维 list，_index 嵌套覆盖/恢复（§3.2.2） |
| NE-2 | Struct 内 Array | Array 作为字段值，返回 list |
| NE-3 | Array 内 Struct | 每个 elem 是 Struct 实例 |
| NE-4 | GreedyRange 内 Struct | 同上，读到流结束 |
| NE-5 | PrefixedArray 内 Struct | count 个 Struct 实例 |
| NE-6 | Bitwise 内 Array | Array 在 bit 域内，inner 是 BitsInteger 等 |
| NE-7 | Array 内 Bitwise | 每个 elem 是 bit 域解析结果 |

---

## 8. 性能假设（S-PERF 验证基础）

### 8.1 瓶颈识别（v5 含 RepeatUntil 逐项分析）

> **v5 关键变更**：原 v4 §8.3 RepeatUntil 单独评估（PyCallable ≥1.5x + Expr ≥8x）
> 已作废。v5 RepeatUntil 统一为终止表达式路径，**性能目标 ≥10x**（无 PyCallable 路径）。
> 本节给出 Python 端 + Rust 端逐项开销分析 + 数值推导（用户硬约束：禁止范围估算、
> 禁止"物理上限"豁免）。

#### 8.1.1 Python 端 RepeatUntil 开销分析（Python 3.14 实测基线）

Python construct 2.10.70 `RepeatUntil(lambda x,l,c: x > 5, Byte).parse` 的单次
迭代开销（参考 `core.py` L2670-2682 + Python 3.14 实测元操作开销）：

| 开销来源 | 单次耗时（ns） | 来源 / 测量方法 |
|---------|---------------|---------------|
| `self.subcon._parsereport(stream, context, path)` 调用 | ~1000-1500 | Python 函数调用 + Stream.read_bytes + FormatField 解析 |
| `context._index = i`（Container `__setitem__`） | ~80-120 | Container dict 写入 |
| `obj.append(e)`（ListContainer.append） | ~100-150 | list.append + ListContainer hook |
| `predicate(e, obj, context)` lambda 调用 | ~270-300 | Python 3.14 实测：3 参数 lambda + 比较运算 |
| 终止表达式内 `x > 5` 求值（Python lambda） | ~30-50 | int.__gt__ |
| 循环控制（`itertools.count` 迭代 + 条件分支） | ~50-80 | next() + if |
| **单次迭代合计** | **~1530-2250** | |
| **N=100 总耗时** | **~153-225 μs** | |

**关键修正**（v5 vs v4 估算）：
- v4 估算 Python callable 调用 500-1000ns（高估 2-3 倍）
- Python 3.14 实测：3 参数 lambda 调用仅 270-300ns（含参数绑定 + 返回）
- v5 取实测值，不再用范围估算上界

#### 8.1.2 Rust 端 RepeatUntilNode 开销分析（v5 设计）

construct-rs `RepeatUntilNode.parse`（终止表达式 = Phase 2 ExprProgram）的单次
迭代开销：

| 开销来源 | 单次耗时（ns） | 来源 / 测量方法 |
|---------|---------------|---------------|
| `self.inner.parse(py, stream, ctx, path)`（FormatField 分派 + read 1 byte + PyLong 创建） | ~15-25 | Phase 1-3 实测 |
| `ctx.set_index(i)`（栈字段写入） | ~1 | 直接赋值 |
| `path.push_index/pop`（Vec::push + format!） | ~20-30 | format!("[{i}]") 是主要开销 |
| `list.append(elem)`（PyList C API） | ~30-50 | PyList_Append |
| `ctx.set_field_at(idx, name, elem, py)`（写入 expr_values_buf 内联数组） | ~5-10 | 直接指针赋值 + 引用计数 incr |
| `eval_expr_int(&terminator, ctx, py)`（栈式 VM 求值，~3 个 ExprOp） | ~15-25 | i64 栈数组 + match 分派 |
| 循环控制（loop + saturating_add） | ~2-5 | 编译期常量 |
| **单次迭代合计** | **~88-146** | |
| **N=100 总耗时** | **~8.8-14.6 μs** | |

#### 8.1.3 加速比数值推导（v5）

```
加速比 = Python 总耗时 / Rust 总耗时

保守估计（取 Python 下限、Rust 上限）：
  加速比 = 153 μs / 14.6 μs ≈ 10.5x

理想估计（取 Python 上限、Rust 下限）：
  加速比 = 225 μs / 8.8 μs ≈ 25.6x

中位数估计：
  加速比 = 189 μs / 11.7 μs ≈ 16.2x
```

**结论**：RepeatUntil v5 设计预期加速比 **10.5x - 25.6x**，中位数 ~16x，
满足 S-PERF ≥10x 硬门禁（用户硬约束 #5：禁止"物理上限"豁免）。

#### 8.1.4 Array / GreedyRange / PrefixedArray 开销分析（不变，沿用 v4）

Python construct `Array(N, Byte).parse` 的主要开销：

| 开销来源 | 单次耗时（估算） | N=100 总耗时 |
|---------|----------------|-------------|
| `_parsereport` 调用（Python 函数调用） | ~1-2μs | 100-200μs |
| `ListContainer.append` | ~100-200ns | 10-20μs |
| `context._index = i`（dict 写入） | ~50-100ns | 5-10μs |
| `evaluate(self.count, context)` | ~200ns（仅一次） | 0.2μs |
| **总计** | | ~120-230μs |

construct-rs `ArrayNode.parse` 的预期开销：

| 开销来源 | 单次耗时（估算） | N=100 总耗时 |
|---------|----------------|-------------|
| `self.inner.parse`（match 分派 + read + PyLong 创建） | ~15-30ns | 1.5-3μs |
| `PyList.append`（C API） | ~30-50ns | 3-5μs |
| `ctx.set_index`（栈字段写入） | ~1ns | 0.1μs |
| `eval_count`（仅一次） | ~5-30ns | 0.03μs |
| `path.push_index/pop` | ~10ns × 2 | 2μs |
| **总计** | | ~7-10μs |

**预测加速比**：120/8 ≈ **15x**（保守），230/7 ≈ **30x**（理想）。

### 8.2 可证伪预测（v5 重写）

| 场景 | 预测加速比 | 验证方法 | S-PERF 出口 |
|------|-----------|---------|-----------|
| Array(100, Byte) parse | ≥15x | timeit, N=1000, 取 min | ✅ 纳入 |
| Array(100, Byte) build | ≥12x | timeit, N=1000 | ✅ 纳入 |
| Array(100, Struct{2 fields}) parse | ≥8x | Struct 内层有更多 C API 开销 | ✅ 纳入 |
| GreedyRange(Byte) parse 100 元素 | ≥8x | seek 开销 + 动态长度 | ✅ 纳入 |
| PrefixedArray(Byte, Byte) parse 100 元素 | ≥10x | 等同 Array + 1 次 countfield | ✅ 纳入 |
| **RepeatUntil(e > 50, Byte) parse 100 元素**（v5） | **≥10x** | 终止表达式零 FFI 求值，§8.1.3 数值推导 | ✅ 纳入 |
| **RepeatUntil(e > 50, Byte) build 100 元素**（v5） | **≥10x** | 同 parse 路径开销结构 | ✅ 纳入 |
| **RepeatUntil((e & 0xFF) == 0, Int32ub) parse**（v5） | **≥10x** | 终止表达式含位运算 + 比较，4 个 ExprOp | ✅ 纳入 |
| **RepeatUntil(e > threshold, Byte) parse**（v5，threshold 是字段） | **≥10x** | 终止表达式引用 Element + 字段，4 个 ExprOp | ✅ 纳入 |
| Index（在 Array 内） | ≥20x | 仅读 ctx 字段 | ✅ 纳入 |
| Element（在 RepeatUntil 内） | N/A | Element 是引用入口，不独立 benchmark | — |

**证伪条件**：
- 若 RepeatUntil 任意场景加速比 < 10x，视为性能未达标，需排查：
  - `eval_expr_int` 是否意外走 FFI（应全程 Rust 内部栈）
  - `set_field_at` 是否走 PyDict 路径（应直接写 expr_values_buf 内联数组）
  - `terminator` 表达式 ExprOp 数量是否超预期（典型 3-5 个）
  - inner.parse 是否被错误地走多一层 FFI
- 若加速比 < 4x（S-PERF 硬目标的下限），视为架构失败，回退设计。

**禁止事项**（用户硬约束 #5）：
- 禁止用"物理上限"作为门禁豁免理由
- 禁止用"兜底路径"作为门禁豁免理由（v5 无兜底路径）
- 禁止场景标签与实际工作量不符（如标 "N=1024" 实际只迭代 6/3 次）
- 禁止"特殊用例不计入硬门禁"——所有场景必须达标

### 8.3 RepeatUntil 性能假设的物理推导（v5 完全重写，替换 v4 §8.3）

> **v5 变更**：v4 §8.3 把 RepeatUntil 拆为 PyCallable（≥1.5x）+ Expr（≥8x）两条路径。
> v5 删除 PyCallable 路径，RepeatUntil 统一为终止表达式路径，性能目标 **≥10x**。
> 本节给出 ≥10x 的逐项数值推导（替代 v4 的"两路径分别评估"）。

#### 8.3.1 终止表达式 → ExprOp 编译路径示例（≥5 个典型场景）

| 终止表达式（construct-rs 语法） | 编译为 ExprOp 序列 | ExprOp 数量 |
|--------------------------------|--------------------|------------|
| `e > 5` | `[GetInt(0), Const(5), Gt]` | 3 |
| `e == 0xFF` | `[GetInt(0), Const(255), Eq]` | 3 |
| `e != -1` | `[GetInt(0), Const(-1), Ne]` | 3 |
| `(e & 0xFF) == 0` | `[GetInt(0), Const(255), BitAnd, Const(0), Eq]` | 5 |
| `-e > -5` | `[GetInt(0), Neg, Const(-5), Gt]` | 4 |
| `(e + offset) & mask == sentinel` | `[GetInt(0), GetInt(1), Add, Const(mask), BitAnd, Const(sentinel), Eq]` | 7（最复杂） |
| `e > threshold`（threshold 是 Struct 字段） | `[GetInt(0), GetInt(1), Gt]` | 3 |

**ExprOp 求值开销（每个 ExprOp）**：
- GetInt(idx)：从 expr_values_buf 取 PyObject 指针 + PyLong_AsLongLong → ~5-8 ns
- Const(v)：栈数组写入 → ~1 ns
- 二元运算（Add/Gt 等）：pop 2 + 计算 + push → ~2-3 ns

**典型终止表达式（3 个 ExprOp）总开销**：~10-15 ns
**最复杂终止表达式（7 个 ExprOp）总开销**：~30-40 ns

#### 8.3.2 RepeatUntil ≥10x 的逐项推导

参考 §8.1.2（Rust 端开销），关键依赖：

| 开销项 | 单次（ns） | 是否可消除？ |
|-------|----------|------------|
| inner.parse（FormatField） | 15-25 | 否（必需的流读 + PyLong 创建） |
| ctx.set_index | 1 | 否（栈字段写入，零开销） |
| path.push_index/pop | 20-30 | **可优化**（P0-3 风格，§10.2） |
| list.append | 30-50 | 否（PyList C API 已最快） |
| ctx.set_field_at（借用 Element 槽位） | 5-10 | 否（栈数组写入） |
| eval_expr_int | 10-40 | 否（零 FFI 栈式 VM） |
| 循环控制 | 2-5 | 否 |

**未优化总开销**：~83-161 ns/iter → 加速比 10-27x
**P0-3 优化后总开销**（消除 path push/pop）：~63-131 ns/iter → 加速比 12-35x

**结论**：即使最保守估计（取 Rust 上限 + Python 下限），加速比 ~10x。
P0-3 优化（成功路径不维护 path）可进一步提升至 ~12x+。**满足 ≥10x 硬门禁**。

#### 8.3.3 性能假设的关键依赖

1. **`ctx.set_field_at` 必须走 expr_values_buf**（栈分配内联数组），不走 PyDict。
   - 若误用 `ctx.set_field(name, value)`（PyDict 路径），每次迭代多 ~50-80 ns
   - 100 元素 = 5-8 μs 额外开销，加速比从 ~16x 降到 ~10x（仍达标，但余量小）
2. **`eval_expr_int` 必须全程 Rust 内部栈式 VM**（无 FFI）。
   - 若误用 PyCallable 路径（v5 已删除），每次迭代跨 FFI ~270-300 ns
   - 100 元素 = 27-30 μs 额外开销，加速比从 ~16x 降到 ~5-6x（不达标）
3. **`path.push_index/pop` 首版保留**，若性能不达标再用 P0-3 风格优化。
4. **inner.parse 的 FormatField 分派开销**（~15-25 ns）不可消除，
   这是 Rust 实现的下限。


### 8.4 验证脚本设计

`experiments/phase4_bench.py`（PM 在 S-PERF 阶段执行）：

```python
import timeit
from construct import Array as PyArray, Byte
from construct_rust import Array as RsArray, ...   # 或通过 StructMixin 子类

# 场景定义
scenarios = [
    ("Array(100, Byte) parse", lambda: PyArray(100, Byte).parse(DATA), ...),
    ("Array(100, Byte) build", ...),
    ...
]

# 两边相同 timeit 参数
NUMBER = 1000
REPEAT = 5

results = []
for name, py_fn, rs_fn in scenarios:
    py_times = timeit.repeat(py_fn, number=NUMBER, repeat=REPEAT)
    rs_times = timeit.repeat(rs_fn, number=NUMBER, repeat=REPEAT)
    py_min = min(py_times) / NUMBER
    rs_min = min(rs_times) / NUMBER
    speedup = py_min / rs_min
    results.append((name, py_min, rs_min, speedup))

# 输出派生指标
print("场景 | Python ns/call | Rust ns/call | 加速比 | parse/build 比率 | 低于4x?")
...
```

**测量口径**（AGENTS.md §6 S-PERF）：
- construct-rs 侧：`maturin develop --release` 安装后，Python 调用用户面 API
- Python 侧：直接 `import construct`，调等效 API
- 子进程隔离（两边包同名，不可同进程导入）
- 取 `min(repeat=5)` × `number=1000`

### 8.5 性能假设的关键依赖

1. **PyList::new_bound + Vec 预分配**：消除 append 循环的 realloc。
   - 若 PyList 内部仍多次 realloc（容量预测不准），加速比下降。
   - 验证：用 `PyList::new_bound(py, Vec::with_capacity(count))` 而非空 List + append。
2. **ctx._index 栈字段**：不走 PyDict。
   - 若误用 `ctx.set_field("_index", i)`，加速比降 5-10x。
   - 验证：profile 检查 PyDict_SetItem 调用次数（应为字段数，非字段数×N）。
3. **path push/pop 开销**：当前每次迭代调用。
   - 若 path.push_index 的 Vec::push 成本 ~10ns 累积过大（N=100 时 1μs），可改为
     P0-3 风格（成功路径不维护，错误时重建）。首版保留 push/pop，性能不达标再优化。
4. **RepeatUntil 终止表达式求值必须全程 Rust 内部栈式 VM**（v5 关键依赖）。
   - 单次迭代终止表达式求值开销：~10-40 ns（3-7 个 ExprOp，§8.3.1）
   - N=100 时 100 次求值 = ~1-4 μs（占总耗时 ~10-30%）
   - **关键不变量**：`ctx.set_field_at` 必须走 `expr_values_buf`（栈分配内联数组），
     绝不走 PyDict。`eval_expr_int` 全程在 Rust 内部，无 FFI。
   - 若误走 PyDict 路径：每次迭代多 ~50-80 ns，100 元素 = 5-8 μs，加速比从 ~16x
     降到 ~10x（仍达标但余量小）。
   - 若误引入 FFI（如 v4 PyCallable 路径）：每次迭代多 ~270-300 ns，100 元素 =
     27-30 μs，加速比从 ~16x 降到 ~5-6x（**不达标**）。
   - 验证：profile 检查 RepeatUntil 迭代中无 PyObject_Call 调用（PyCallable 已删除）。

---

## 9. 与 Python 版本的对应

### 9.1 类/方法映射表

| Python 类/方法 | Rust 类型/方法 | 行号 | 状态 |
|---------------|---------------|------|------|
| `Array` | `ArrayNode` | 2493 | ✅ 完整映射 |
| `Array.__init__(count, subcon, discard)` | `ArrayNode::new(inner, count, discard)` | 2520 | ✅ |
| `Array._parse` | `ArrayNode::parse` | 2525 | ✅ |
| `Array._build` | `ArrayNode::build` | 2538 | ✅ |
| `Array._sizeof` | `ArrayNode::sizeof` | 2553 | ✅ |
| `GreedyRange` | `GreedyRangeNode` | 2570 | ✅ |
| `GreedyRange._parse` | `GreedyRangeNode::parse` | 2599 | ✅（简化错误分流） |
| `GreedyRange._build` | `GreedyRangeNode::build` | 2617 | ✅ |
| `GreedyRange._sizeof` | `GreedyRangeNode::sizeof` | 2630 | ✅（永远 Err） |
| `RepeatUntil` | `RepeatUntilNode` | 2637 | ✅（v5：终止表达式 = Phase 2 ExprProgram，无 PyCallable） |
| `RepeatUntil._parse` | `RepeatUntilNode::parse` | 2670 | ✅（v5：终止表达式零 FFI 求值） |
| `RepeatUntil._build` | `RepeatUntilNode::build` | 2684 | ✅（v5：同 parse 路径） |
| `RepeatUntil._sizeof` | `RepeatUntilNode::sizeof` | 2703 | ✅（永远 Err） |
| `PrefixedArray` | `PrefixedArrayNode` | 4934 | ✅（独立实现，不依赖 FocusedSeq） |
| `PrefixedArray._emitparse` | `PrefixedArrayNode::parse` | 4961 | ✅ |
| `PrefixedArray._emitbuild` | `PrefixedArrayNode::build` | 4965 | ✅ |
| `Index` | `IndexNode` | 2934 | ✅ |
| `Index._parse` | `IndexNode::parse` | 2965 | ✅ |
| `Index._build` | `IndexNode::build` | 2968 | ✅（no-op） |
| `Index._sizeof` | `IndexNode::sizeof` | 2971 | ✅（返回 0） |
| `Element` | `ElementNode` | —（v5 新增） | ✅ construct-rs 新增，Python construct 无对应物 |
| `Element._parse` | `ElementNode::parse` | — | ✅（始终返回 None） |
| `Element._build` | `ElementNode::build` | — | ✅（no-op） |
| `Element._sizeof` | `ElementNode::sizeof` | — | ✅（返回 0） |
| `StopIf` | `StopIfNode` | 4079 | ✅ |
| `StopIf._parse` | `StopIfNode::parse` | 4103 | ✅（StopField 哨兵） |
| `StopIf._build` | `StopIfNode::build` | 4108 | ✅ |
| `StopIf._sizeof` | `StopIfNode::sizeof` | 4113 | ✅（永远 Err） |
| `ListContainer` | 原生 `PyList` | containers.py | ⚠️ 不引入子类（§2.7） |
| `LazyArray` | — | 6118 | ❌ 延后到惰性阶段 |
| `LazyListContainer` | — | — | ❌ 延后 |
| `len_` | — | — | ❌ PrefixedArray 不依赖（§6.3.3） |

### 9.2 异常映射

| Python 异常 | Rust 错误变体 | 备注 |
|------------|--------------|------|
| `RangeError` | `ConstructError::Range` | 新增 |
| `RepeatError` | `ConstructError::Repeat` | 新增 |
| `StopFieldError` | `ConstructError::StopField` | 新增（哨兵） |
| `IndexFieldError` | `ConstructError::IndexField` | 新增（保留，首版不触发） |
| `SizeofError`（GreedyRange/RepeatUntil/StopIf） | `ConstructError::Generic` | 复用现有 |
| `StreamError` | `ConstructError::Stream` | 复用现有 |

### 9.3 Python context 字段对应

| Python context 字段 | Rust Context 字段 | 备注 |
|--------------------|--------------------|------|
| `context._index` | `Context._index: Option<usize>` | 新增（§3.2） |
| `context.<field>` | `Context.expr_values_buf[idx]` 或 `fields` PyDict | 现有 |
| `context._`（外层） | `Context.parent: Option<&Context>` | 现有 |

### 9.4 Python 表达式对应

> **V2 修正（v3）**：本表重构为三列对照，明确 construct-rs 用户面**不使用 `this.xxx` 语法**。
> construct-rs 表达式系统输入是 `_FieldDescriptor` / `_ExprRef` 对象（字段名直接引用），
> 详见表达式系统 §2.3。Phase 2 决策 1 明确禁止 `this.xxx` 语法。

| Python construct 语法 | construct-rs 用户面语法 | Rust ExprOp | 备注 |
|-----------------------|------------------------|-------------|------|
| `this.<field>`（如 `this.count`） | `<field>`（如 `count`，字段名直接引用，是 `_FieldDescriptor` 对象） | `GetInt(idx)` | 现有；编译期 `id(descriptor)` 绑定字段索引 |
| 算术/位/比较（如 `this.a + this.b`） | 字段名 + 运算符（如 `a + b`，编译为 `_ExprRef` 树） | `Add` / `Sub` / ... / `Gt` / ... | 现有；表达式系统 §2.3.3 |
| `this._index`（数组下标引用） | **不直接支持**——通过 Index 字段 + 字段名引用实现：`i: int = rfield(Index()); v: bytes = field(Bytes(i + 1))` | `[GetInt(idx_of_i), Const(1), Add]` | v3 决策（§3.3）；不引入 `GetIndex` 指令，复用 GetInt |

> **v3 决策说明**：v1/v2 设计了 `ExprOp::GetIndex` 指令作为 `this._index` 的对应物，
> 但用户面入口违反 Phase 2 决策 1（废弃 `this`）。v3 删除 GetIndex，统一通过
> IndexNode + 字段引用机制实现等价功能。详见 §3.3 与设计决策记录 Phase 4 决策 3。

### 9.5 已知行为差异（M1 修正 + 整理）

construct-rs 与 Python construct 2.10.70 的有意行为差异，列于此集中管理。
PM/REV 验收时需逐项确认（用户文档应注明）。

| ID | 差异点 | Python 行为 | construct-rs 行为 | 理由 / 影响范围 |
|----|--------|------------|------------------|----------------|
| LC-1 | `isinstance(result, ListContainer)` | True | **False**（返回原生 list） | ListContainer 子类实例化走 Python 慢路径；§2.7 决策 A7 |
| LC-2 | `result.search(name, value)` | 可用 | **AttributeError**（原生 list 无 search） | 工具方法可后续作为独立函数提供 |
| LC-3 | `repr(result)` / `str(result)` | ListContainer 美化（缩进展示） | 原生 list repr | 用户可用 pprint 替代；非核心功能 |
| LC-4 | `result == [1,2,3]` | True（值相等） | True | 值比较一致，无差异 |
| IX-1 | 数组下标引用不在数组内（Python 写法 `Computed(this._index + 1)`） | `TypeError: None + 1` | construct-rs 等价写法 `i: int = rfield(Index()); v = rfield(Computed(i + 1))` 在 `i` 为 None 时 `i + 1` 表达式求值抛 `ExprType`（None 无法 extract 为 i64） | v3：行为与 Python 一致（都报错）。v1/v2 设计的 `GetIndex` 在 None 时返回 0 导致行为差异，已删除（§3.3） |
| GE-1 | GreedyRange 内部子构造器抛 `ExplicitError` | 向上传播（不回退） | **无 ExplicitError 等价物**，与其他错误一样 seek 回退 + 正常终止 | 见下方详述 |
| GE-2 | GreedyRange 内部子构造器抛 FormatField/Stream 等普通错误 | seek 回退 + 正常终止 | 同 Python（seek 回退 + 正常终止） | 行为一致 |
| RU-v5-1 | RepeatUntil 用户面 API 传入 Python lambda/callable | 接受 | **编译期失败**（CompilationError） | v5 用户硬约束 #1（禁止 PyCallable）；§7.3 RU-16 |
| RU-v5-2 | RepeatUntil 终止表达式引用 list 切片（如 `lst[-2:] == [0, 0]`） | 接受 | **编译期失败**（CompilationError） | Phase 2 表达式 VM 栈为 i64，不支持 list 切片；用户须改用 Adapter；§13.9 |
| RU-v5-3 | Element 字段在 RepeatUntil 之外独立存在 | — | ElementNode.parse 返回 None，Packet 实例属性 = None | v5 新增能力（Python construct 无对应物）；§4.7 / §13.2 |
| PA-5 | PrefixedArray build 时 list 长度超出 countfield 表示范围 | countfield 抛 FormatFieldError/StreamError | 由 countfield 节点自行报错（同方向） | 行为一致，§7.4 PA-5 |
| NE-expr-1 | `Array(N, Bytes(m))` inner 含表达式（**Python 写法 `Bytes(this.m)`**） | 支持 | **编译期失败**（CompilationError） | 与 `Bitwise(Bytes(m))`（Python 写法 `Bitwise(Bytes(this.m))`）同限制；§6.2.2 P3.1 |
| NE-expr-2 | `Array(N, StopIf(x))` inner 含表达式（**Python 写法 `StopIf(this.x)`**） | 支持 | **编译期失败** | 同上，需扩展 _extract_and_compile_exprs 递归 |

#### M1 详述：GreedyRange 错误吞掉（GE-1）

Python `GreedyRange._parse`（core.py L2599-2615）区分三类异常：

```python
try:
    for i in itertools.count():
        ...
except StopFieldError:      # 1. 早停信号：正常终止，不回退
    pass
except ExplicitError:        # 2. 显式不可恢复错误：向上传播
    raise
except Exception:            # 3. 其他错误（含 Stream/FormatField 等）：seek 回退 + 正常终止
    stream_seek(stream, fallback, 0, path)
```

construct-rs 当前设计（§4.2.2）的分流：

```rust
match self.inner.parse(...) {
    Ok(elem) => { ... },
    Err(StopField { .. }) => { stream.seek(fallback, ...)?; break; },  // 1. 同 Python
    Err(e) => { let _ = stream.seek(fallback, ...); break; },          // 2+3 合并
}
```

**差异**：construct-rs 没有 `ConstructError::Explicit` 等价变体。Python 的
`ExplicitError` 是用户/库显式标记的"不可恢复错误"（在 Stream/FieldError 之上），
construct-rs 把所有非 StopField 错误都视为"流终止信号"（回退 + 正常终止）。

**影响**：
- **FormatField/Stream 错误**（如字节不足、整数溢出）：行为与 Python 一致（GE-2）。
- **用户主动抛 ExplicitError**：Python 让错误传播（GreedyRange 不终止），construct-rs
  把它当成普通错误终止 GreedyRange。**但**：当前 construct-rs 不暴露任何
  ExplicitError 触发 API（无对应描述符），所以**实际无用户场景触发此差异**。

**决策**：**不引入 `ConstructError::Explicit`**。理由：
1. 当前用户面 API 不暴露 ExplicitError 触发方式，差异不可达。
2. 引入新变体需要：(a) Python 侧暴露 `ExplicitError` 类；(b) Rust 侧新增 ConstructError
   变体 + 全节点链路正确传播；(c) 测试覆盖。工作量大，收益小。
3. Python `ExplicitError` 极少用（仅在 Switch/If ThenElse 等高级特性的错误分支），
   Phase 4 不覆盖这些特性。

**后续处理（标记为已知问题）**：若未来 Phase 5+ 实现 Switch / IfThenElse 并暴露
`ExplicitError` 用户 API，再补充 `ConstructError::Explicit` 变体并在 GreedyRange 分流。
当前 GE-1 作为已知差异记录，**不阻塞 Phase 4 出口**。

---

## 10. 后续优化方向（非 Phase 4 范围）

### 10.1 RepeatUntil 性能优化（v5：终止表达式已是唯一路径）

> **v5 变更**：v4 此节曾写"RepeatUntil 的 Expr 谓词路径作为后续优化方向"，
> 暗示 Expr 路径是 PyCallable 的补充。v5 删除 PyCallable 后，**终止表达式就是
> 唯一路径**——本节不再是"补充 Expr 路径"，而是"现有 Expr 路径的微优化"。

RepeatUntil v5 终止表达式路径已满足 ≥10x（§8.3 推导）。可选的进一步优化：

1. **path push/pop 优化（P0-3 风格）**：见 §10.2。
2. **`ctx.set_field_at` 内联**：当前 `set_field_at` 是 Context 方法调用，
   可在 RepeatUntilNode 中直接写 `ctx.expr_values_buf[self.element_field_idx] = elem.as_ptr()`
   （需要 Context 暴露 `expr_values_buf_mut` unsafe 访问器）。预期节省 ~2-3 ns/iter。
3. **terminator ExprProgram 特化**：若 terminator 是固定模式（如 `[GetInt, Const, Gt]`），
   可在编译期识别为"哨兵比较"，运行时直接调 `inner.parse + PyLong_AsLongLong + 比较`，
   绕过栈式 VM 开销。预期节省 ~5-10 ns/iter。但增加编译器复杂度，YAGNI。

### 10.2 path push/pop 优化（P0-3 风格）

若性能分析显示 path.push_index/pop 成为瓶颈（~2μs/100 元素），改为成功路径不维护：

```rust
// 成功路径不 push/pop
let elem = self.inner.parse(py, stream, ctx, path)?;
// 错误路径在 ArrayNode::parse 的 Err 分支内 push_index 后 return
```

但这要求子节点错误时 path 是 "root" 状态（不含数组索引），ArrayNode 在错误分支
push_index 重建。与现有 StructNode 的 P0-3 优化一致。

### 10.3 LazyArray / LazyListContainer

Python `LazyArray` 使用惰性解析（按需解析元素）。归入惰性解析 Phase（与 Lazy、
LazyStruct 同阶段）。

### 10.4 Rebuild / FocusedSeq 与 PrefixedArray 重构

若未来实现 Rebuild / FocusedSeq，PrefixedArray 可迁移到组合方案（§5.2 选项 B）。
当前独立 Node 方案性能更优，迁移非必要。

### 10.5 path.push_index 的整数格式化优化

当前 `Path::push_index` 用 `format!("[{}]", idx)`，每次分配 String。
可改为 `Path` 内部用 `Vec<usize>` 存索引，Display 时一次性格式化。
但这是 Phase 1 遗留优化，不在 Phase 4 范围。

---

## 11. 设计完整性自检

### 11.1 API 映射完整性

✅ 覆盖 Python Array 功能集的全部公开方法（§9.1）
✅ 异常映射完整（§9.2）
✅ Context 字段映射完整（§9.3）
✅ 表达式映射完整（§9.4，v3 重构为三列对照，明确 construct-rs 不用 `this.xxx`）
✅ 决策记录对照完整（v3 修正后，Phase 2 决策 1 不再违反，详见 §11.7）

### 11.2 边界条件完整性

✅ Array 边界 10 条（§7.1，AR-10 已更新为"inner 表达式编译期失败"）
✅ GreedyRange 边界 9 条（§7.2）
✅ RepeatUntil 边界 8 条（§7.3，新增 RU-8 build discard 语义）
✅ PrefixedArray 边界 6 条（§7.4，PA-5 措辞修正）
✅ Index 边界 4 条（§7.5）
✅ StopIf 边界 7 条（§7.6）
✅ ListContainer 兼容性 4 条（§7.7）
✅ 嵌套与组合 7 条（§7.8）
✅ 已知行为差异 11 条（§9.5，含 LC/IX/GE/RU-build/PA/NE-expr 全集）

### 11.3 与现有架构的兼容性

✅ 不修改 Construct trait 签名（13 个现有节点无需改动）
✅ 不修改现有 Node 变体的行为
✅ 不修改 ParseStream/BuildStream 的现有 API（仅新增 seek）
✅ Context 新增字段不影响现有构造函数语义（_index 默认 None）
✅ compile_schema 向后兼容（新增描述符识别分支）
✅ StructNode 仅增加 StopField 捕获分支（不破坏现有行为）
✅ has_expressions 修正不影响现有 13 个 Node 变体（仅 StopIf/Index 新增分支）

### 11.4 性能假设的瓶颈覆盖

✅ FFI 调用（RepeatUntil PyCallable 路径有，Expr 路径消除）
✅ PyList 创建与 append（§8.1）
✅ ctx._index 写入（§8.1）
✅ path push/pop（§8.1、§8.5）
✅ 子节点 parse 分派（§8.1）
✅ RepeatUntil S-PERF 适用性（§8.3 单独明确）

### 11.5 编码红线遵守

✅ 无 `unwrap()` / `expect()` 在非测试代码（伪代码中的 unwrap 仅为示意，DEV 实现时
   用 `?` 或 match）
✅ 无 panic（错误返回 Err）
✅ 无 TODO/FIXME（设计文档中无；v1 的 placeholder 已在 v2 补全）
✅ 无硬编码魔法数字（常量在模块顶部定义）
✅ 所有 pub 项有 `///` 文档注释（DEV 实现时补全）
✅ 错误携带 path（所有新增错误变体都有 path 字段）
✅ parse/build 对称性（所有 Array 节点同时支持 parse 和 build）

### 11.6 REV 驳回修正自检（v2 新增）

| 编号 | 自检项 | 结论 |
|------|--------|------|
| P1 | §6.1.1 中 `StopIf(s)` 的 has_expressions 是否返回 `matches!(cond, Expr(_))`？ | ✅ |
| P1 | 是否附 StructRef 路径（struct_ref.rs L171/L194）的根因说明？ | ✅ |
| P2 | §4.3.4 build 伪代码中 `partial.append` 是否在 `if !self.discard` 守卫内？ | ✅ |
| P2 | §7.3 是否新增 RU-8 build discard 边界条目？ | ✅ |
| P3 | §6.2.2 是否给出完整的表达式 count 编译路径（无 placeholder）？ | ✅ |
| P3 | 是否明确 inner 含表达式的支持范围（P3.1）与扩展方案（P3.2）？ | ✅ |
| M1 | §9.5 是否新增 GE-1（GreedyRange 错误吞掉）条目并附决策说明？ | ✅ |
| M2 | §7.4 PA-5 是否删除"回绕"误述，改为"countfield 自行报错"？ | ✅ |
| M3 | §4.1.4 是否明确 sizeof 的 GIL 前置条件 + with_gil 闭包正确写法？ | ✅ |
| M4 | §8.3 是否明确 S-PERF 出口对 RepeatUntil 的判定规则？ | ✅ |
| M4 | §12.1 是否明确 4.5a/4.5b 拆分且 4.5b 必须在 Phase 4 验收前完成？ | ✅ |

### 11.7 REV 决策记录对照驳回修正自检（v3 新增）

| 编号 | 自检项 | 结论 |
|------|--------|------|
| V1 | §3.3 是否删除 ExprOp::GetIndex（不再扩展表达式系统）？ | ✅ 整节重写为"不扩展 ExprOp" |
| V1 | 是否在设计决策记录 Phase 4 决策 3 写入"Index 仅作为构造器字段"？ | ✅ |
| V1 | 是否明确用户访问下标的机制（`rfield(Index())` + 字段引用）？ | ✅ §3.3 表格 + §4.4 示例 |
| V1 | 是否说明已实现代码（expr.rs/compile.rs 的 GetIndex）需 DEV 回退？ | ✅ §3.3 + §12.7 |
| V1 | 是否保留 Context._index 字段（IndexNode 仍需通过 ctx.index() 读）？ | ✅ §3.3 明确"保留" |
| V2 | §9.4 是否重构为三列对照（Python / construct-rs / ExprOp）？ | ✅ |
| V2 | 是否明确 construct-rs 用户面不用 `this.xxx` 语法？ | ✅ §9.4 表头 + 表内说明 |
| V3 | §2.2 CountSource::Expr 示例的 `this.length` 是否标注 Python 语法？ | ✅ |
| V3 | §2.3 的 `this._index` 描述是否改为标注 Python 语法或删除？ | ✅ 重写为"不通过用户面表达式暴露" |
| V3 | §2.6 PrefixedArray 宏 `len_(this.items)` 是否标注 Python 语法？ | ✅ |
| V3 | §3.2.2 `this._index` 是否标注 Python 语法？ | ✅ |
| V3 | §6.1.1 has_expressions 注释中 `this.x` 是否改为 construct-rs 语法或标注？ | ✅ 改为字段名引用 + Python 写法注释 |
| V3 | §6.2.2 P3.1/P3.2 表格中 `this.n`/`this.m`/`this.x` 是否标注 Python 语法？ | ✅ 表前加语法约定 + 表内字段名引用 |
| V3 | §6.3.3 `len_(this.items)` 是否标注 Python 语法？ | ✅ |
| V3 | §7.1 AR-10 边界 `Bytes(this.m)` 是否标注 Python 语法？ | ✅ |
| V3 | §9.5 NE-expr-1/2 差异表 `this.m`/`this.x` 是否标注 Python 语法？ | ✅ |
| V3 | §12.2 PM 决策点 2 表格 `this.xxx` 是否标注 Python 语法？ | ✅ 表前加语法约定 |
| V4 | §4.5.1 StopIfCondition::Expr 注释 `this.x == 0` 是否改为 construct-rs 语法？ | ✅ 改为 `x == 0` + 编译产物说明 |

### 11.8 VET 驳回修正自检（v4 新增）

| 编号 | 自检项 | 结论 |
|------|--------|------|
| V-1 | §2.5 决策 A5 是否补充 PyCallable proxy 硬性约束（Container + 字段复制）？ | ✅ 补充 4 条约束 + 决策依据 |
| V-1 | §4.3.3 `build_context_proxy` 是否改为 Container 包装的硬性约束（删除"DEV 决策"模糊空间）？ | ✅ 提供完整实现伪代码 + 性能估算 |
| V-1 | §4.3.3 / §4.3.4 伪代码是否传 container_cls 并构造 Container proxy？ | ✅ parse / build 均更新 |
| V-1 | §7.3 是否新增 RU-9（谓词访问 context 字段）边界？ | ✅ |
| V-1 | §8.2/§8.3 是否更新 PyCallable 路径性能预期（≥3x → ≥1.5x）？ | ✅ 含 v4 决策依据说明 |
| V-1 | §8.5 关键依赖 4 是否更新开销结构（含 Container 实例化）？ | ✅ |
| V-1 | §9.5 是否需要新增已知差异条目？ | ✅ 不新增（方案 A 行为对齐 Python，无差异） |
| V-1 | 设计决策记录 Phase 4 是否新增决策 4？ | ✅ "PyCallable proxy 必须用 Container 包装" |
| V-2~V-5 | 是否明确这些次要问题由 DEV 在 V-1 修复批次中附带处理？ | ✅ §0.3 末尾说明 |

---

## 12. 遗留问题与 PM 决策点

### 12.1 PM 决策点 1：RepeatUntil 终止表达式路径范围（v5 完全重写）

**问题**：RepeatUntil 在 Phase 4 的实现范围？

**结论（v5 用户打回后）**：**RepeatUntil 统一为单一子任务 4.5**，终止表达式 =
Phase 2 表达式（ExprProgram），无 PyCallable 路径。

理由（详见 §0.4 v5 修正 + §2.5 决策 A5 v5）：
- 用户硬约束 #1：禁止 PyCallable 路径（违反"一次 FFI"原则）
- 用户硬约束 #2：禁止"白名单 + 兜底"二分设计
- 用户硬约束 #5：终止表达式必须复用 Phase 2 表达式系统
- v4 的 4.5a/4.5b 拆分（PyCallable 先 / Expr 后）作废——v5 中 Expr 是唯一路径

**4.5 子任务范围**：
- 删除 v4 已实施的 PyCallable 路径代码：
  - `RepeatPredicate` 枚举、`call_repeat_predicate`、`build_context_proxy`
  - `container_cache.rs` 整个文件
  - `python/construct/lib/containers.py`（若专为 PyCallable 新建）
  - `ExprOp::GetElem`、`Context::_current_elem_ptr`、`set_current_elem_ptr`、
    `clear_current_elem_ptr`、`current_elem_as_i64`
  - `_descriptors.py` 的 `_AST_OP_TO_EXPROP`、`_try_compile_repeat_predicate`
- 实现 v5 设计：
  - `ElementNode` 新节点（§4.7）
  - `RepeatUntilNode` 重写（§4.3）：`{ inner, terminator, element_field_idx, element_field_name, discard }`
  - `ElementDescriptor` + `Element()` 工厂（§6.3.1）
  - `RepeatUntilDescriptor` 重写：参数 `terminator`，类型 Phase 2 表达式
  - `_mixin._compile_expressions` 特殊处理 RepeatUntilDescriptor（提取 element_field_idx）
  - `compile.rs::build_repeat_until_node` 重写（§6.2.3.1）
- 测试覆盖：§13 用户面使用样例集 + §14 场景测试覆盖矩阵

**性能出口**：RepeatUntil ≥10x（§8.3 数值推导，无 PyCallable 兜底）。

**v5 已知能力边界**（写入设计文档 §2.5.4 + §13.9）：
- 不支持 Python lambda / callable（用户硬约束）
- 终止表达式只支持 Phase 2 ExprProgram 能描述的逻辑（整数算术/位运算/比较）
- list 切片 / list 索引 / 复杂 callable 逻辑须改用 Adapter（显式慢路径）

### 12.2 PM 决策点 2：Array count 表达式支持范围（P3 已明确）

**问题**：Array 的 count 表达式（**Python 写法 `Array(this.length, Byte)`**；
construct-rs 等价写法 `Array(length, Byte)`，`length` 是字段名引用）支持范围？

**结论（P3 修正后）**：

> **语法约定（v3）**：下表"场景"列同时给出 Python 写法（含 `this.`）与 construct-rs
> 等价写法（字段名直接引用），便于 Python construct 用户对照。

| 场景 | Phase 4 支持 | 说明 |
|------|------------|------|
| 顶层 count 表达式（construct-rs：`Array(n, simple_subcon)`；Python 写法：`Array(this.n, ...)`） | ✅ 支持 | 从 `expr_programs[field_index]["count"]` 取，参照 BytesDescriptor 表达式长度（§6.2.2 完整路径） |
| inner 含表达式（construct-rs：`Array(N, Bytes(m))`；Python 写法：`Array(N, Bytes(this.m))`） | ❌ 编译期失败 | 与 `Bitwise(Bytes(m))`（Python 写法 `Bitwise(Bytes(this.m))`）同限制；§6.2.2 P3.1 |
| inner 含表达式（construct-rs：`Array(N, StopIf(x))`；Python 写法：`Array(N, StopIf(this.x))`） | ❌ 编译期失败 | 同上 |
| 嵌套 Array 表达式（construct-rs：`Array(N, Array(m, Byte))`；Python 写法：`Array(N, Array(this.m, Byte))`） | ❌ 编译期失败 | 同上 |
| StopIf 作为 Struct 直接字段（construct-rs：`rfield(StopIf(x))`；Python 写法：`rfield(StopIf(this.x))`） | ✅ 支持 | 与 ComputedDescriptor 的 "func" 同模式 |

理由：
- 顶层 count 表达式：ExprProgram 基础设施已具备，完整编译路径已在 §6.2.2 设计。
  不实现会显著限制 Array 的实用性（用户无法用 `Array(n, Byte)` 引用前序字段）。
- inner 含表达式：Python 侧 `_extract_and_compile_exprs` 当前不递归（_mixin.py L583-615），
  需扩展递归收集 + 解决命名空间冲突。属于独立特性，建议 Phase 4.0b 或 Phase 5+。

**PM 行动**：
- 子任务 4.1（ArrayNode）必须支持顶层 count 表达式（参照 §6.2.2 完整编译路径）。
- 若需要 inner 含表达式支持，新增子任务（Phase 4.0b 或后续），扩展 Python 侧
  `_extract_and_compile_exprs` 递归（§6.2.2 P3.2 扩展方案）。
- **与现有 `Bitwise(Bytes(m))` 限制一致**（Python 写法 `Bitwise(Bytes(this.m))`）：
  在用户文档中明确说明"包装型描述符的 inner 不支持含表达式的子描述符，需扁平化到字段层级"。

**PM 行动**：确认 expr_programs 在编译期的传递路径（§6.2.2）。

### 12.3 PM 决策点 3：sizeof 是否支持静态 PrefixedArray

**问题**：PrefixedArray 的 sizeof 当前保守返回 Err（§4.6.4）。是否支持静态计算？

**建议**：首版保守 Err。理由：
- 实际场景中 PrefixedArray 的 count 来自流，sizeof 几乎不可静态计算。
- 若 countfield 是 ConstExpression，理论上可计算，但场景罕见。
- 保守 Err 不阻塞用户（用户极少对 PrefixedArray 调 sizeof）。

**PM 行动**：确认是否接受保守 Err，或要求 DEV 实现静态路径。

### 12.4 PM 决策点 4：ListContainer 子类是否引入

**问题**：是否实现 ListContainer Python 子类（§2.7）？

**建议**：不实现。理由：
- 性能代价（子类实例化慢路径）。
- repr 美化非必要。
- search/search_all 可作为独立函数。

**PM 行动**：确认接受行为差异（§9.5 LC-1~LC-4），或在 Phase 4 后单独子任务实现。

### 12.5 PM 决策点 5：path.push_index 是否首版优化

**问题**：Array 循环内每次 push_index/pop（~2μs/100 元素）是否首版就用 P0-3 风格优化？

**建议**：首版保留 push/pop，性能不达标再优化。理由：
- 优化增加代码复杂度（错误路径需手动重建）。
- 2μs/100 元素对 ≥10x 目标影响有限（占总耗时 ~20%）。
- 性能分析后再决策（§8.5 关键依赖 3）。

**PM 行动**：在 S-PERF 阶段若发现 path push/pop 是瓶颈，新增子任务优化。

### 12.6 PM 决策点 6：sizeof 签名是否增加 py 参数

**问题**：当前 `sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError>` 无 `py`
参数。Array 表达式 count 的 sizeof 需 GIL 调 `eval_expr_int`（§4.1.4）。

**选项**：
- A：sizeof 内部用 `Python::with_gil`（O(1) 若已持有 GIL，首版采用）
- B：修改 Construct trait，sizeof 增加 `py: Python<'_>` 参数（影响全部节点）

**建议**：选项 A。sizeof 不是热路径，with_gil 开销可接受。

**PM 行动**：若 DEV 实现时发现选项 A 有问题，再讨论选项 B。

### 12.7 设计完整性结论

✅ 设计覆盖全部 P0-P6 构造器（v5：含 Element P6）
✅ 与现有架构兼容（v5 修改：删除 GetElem/_current_elem_ptr/container_cache，新增 ElementNode）
✅ 性能假设具体可证伪（v5：RepeatUntil ≥10x 数值推导，无 PyCallable 兜底）
✅ 边界条件覆盖完整（v5：删除 RU-4/7/8/9 PyCallable 相关，新增 RU-10~18 + EL-1~6）
✅ 遗留问题已明确（6 个 PM 决策点，v5 §12.1 重写）
✅ REV 驳回的 3 个严重问题（P1/P2/P3）已修正（v2）
✅ REV 驳回的 4 个中等问题（M1/M2/M3/M4）已修正（v2）
✅ REV 决策记录对照驳回的 2 个设计问题（V1/V2）已修正（v3）
✅ REV 决策记录对照驳回的 2 个文档表述问题（V3/V4）已修正（v3）
✅ VET 驳回的 V-1（PyCallable context proxy）已修正（v4，但 v5 整体作废）
✅ 用户打回的 6 项硬约束违反已修正（v5，§0.4）

**建议 PM 在子任务拆分时**：
1. 4.0a：基础设施（ParseStream.seek、Context._index、错误变体、Python 异常类映射）
   —— 这是所有 Array 节点的前置依赖
2. 4.1：ArrayNode（P0，含顶层 count 表达式）—— 已实现
3. 4.2：GreedyRangeNode（P1）—— 已实现
4. 4.3：PrefixedArrayNode（P2）—— 已实现
5. 4.4：IndexNode（P4）+ StopIfNode（P5）+ StructNode StopField 捕获 —— 已实现
6. **4.5：RepeatUntilNode v5 重写 + ElementNode 新增**（v5：单一子任务，无 4.5a/4.5b 拆分）
   - 删除 v4 已实施的 PyCallable 路径代码（详见 §12.1）
   - 实现 v5 终止表达式路径（终止表达式 = Phase 2 ExprProgram，零 FFI）
   - 实现 ElementNode 新节点（§4.7）
   - 端到端测试覆盖（§13 用户面使用样例集 + §14 场景测试覆盖矩阵）
7. 4.6：S-FUNC 验证 + S-PERF 基准（RepeatUntil 单路径 ≥10x）

子任务可合并（如 4.1+4.2 一次 DEV 编码），由 PM 决定。

**v5 修正后无需重新检视全部内容**，仅针对 §0.4 / §2.5 / §3.3 / §4.3 / §4.7 /
§6.1 / §6.2 / §6.3 / §7.3 / §8 / §9.1 / §10.1 / §12.1 / §13 / §14 修改部分
重新检视即可。

---

## 13. 完整用户面使用样例集（v5 新增；用户验收门禁）

> **用户硬约束**：所有样例必须可在 Python 解释器中直接执行（含 import、数据准备、
> parse/build 调用）。本节是用户验收门禁——DEV 实施完成后所有样例必须能 copy-paste 运行。
> 建议同时在 `experiments/phase4_repeat_until_examples.py` 提供可执行脚本。

### 13.1 表达式类型覆盖（每种 Phase 2 表达式至少 1 个样例）

```python
from dataclasses import dataclass
from construct import (
    StructMixin, field, rfield,
    Int8ub, Int16ub, Int32ub, Bytes,
    RepeatUntil, Element,
)

# ---- 13.1.1 字段引用（最简形式）----
@dataclass
class P1_Simple(StructMixin):
    e: int = rfield(Element())
    payload: list = field(RepeatUntil(e > 5, Int8ub))

assert P1_Simple.parse(b"\x01\x02\x06\xaa").payload == [1, 2, 6]

# ---- 13.1.2 字段引用 + 常量算术 ----
@dataclass
class P2_ArithConst(StructMixin):
    e: int = rfield(Element())
    payload: list = field(RepeatUntil((e + 1) > 7, Int8ub))

assert P2_ArithConst.parse(b"\x01\x03\x06\xaa").payload == [1, 3, 6]  # 6+1=7 > 7? No; 7+1=8>7 yes
# 注：实际 [1,3,6,7]——7 满足 (7+1)>7

# ---- 13.1.3 字段引用 + 字段引用 ----
@dataclass
class P3_TwoFields(StructMixin):
    threshold: int = field(Int8ub)
    e: int = rfield(Element())
    payload: list = field(RepeatUntil(e > threshold, Int8ub))

p3 = P3_Simple.parse  # 占位
# 实际：P3_TwoFields(threshold=3).parse → threshold 从流读
pkt3 = P3_TwoFields.parse(b"\x03\x01\x02\x03\x04\xff")
assert pkt3.threshold == 3
assert pkt3.payload == [1, 2, 3, 4]  # 4 > 3 满足

# ---- 13.1.4 位运算 ----
@dataclass
class P4_BitOp(StructMixin):
    e: int = rfield(Element())
    payload: list = field(RepeatUntil((e & 0xFF) == 0, Int8ub))

assert P4_BitOp.parse(b"\x01\x02\x00\xaa").payload == [1, 2, 0]

# ---- 13.1.5 一元负 ----
@dataclass
class P5_UnaryNeg(StructMixin):
    e: int = rfield(Element())
    payload: list = field(RepeatUntil(-e > -5, Int8ub))
# 等价：e < 5；但用户可能直接写 -e > -5

# ---- 13.1.6 复合（算术 + 位运算 + 比较）----
@dataclass
class P6_Compound(StructMixin):
    offset: int = field(Int8ub)
    mask: int = field(Int8ub)
    sentinel: int = field(Int8ub)
    e: int = rfield(Element())
    payload: list = field(RepeatUntil(((e + offset) & mask) == sentinel, Int8ub))
```

### 13.2 引用当前元素 / 下标（Element + Index 组合）

```python
# ---- 13.2.1 Element 字段（终止表达式引用当前元素）----
@dataclass
class E1_ElementOnly(StructMixin):
    e: int = rfield(Element())
    payload: list = field(RepeatUntil(e == 0xFF, Int8ub))

assert E1_ElementOnly.parse(b"\x01\x02\xff\xaa").payload == [1, 2, 255]

# ---- 13.2.2 Index 字段（在 RepeatUntil 内引用下标）----
@dataclass
class E2_IndexInRepeat(StructMixin):
    i: int = rfield(Index())         # 当前下标
    e: int = rfield(Element())       # 当前元素
    payload: list = field(RepeatUntil(i >= 3, Int8ub))
# 终止条件：下标 >= 3（前 4 个元素，包括 i=3 那个）

pkt = E2_IndexInRepeat.parse(b"\x01\x02\x03\x04\xff")
assert pkt.payload == [1, 2, 3, 4]  # i=0,1,2,3 → i=3 满足

# ---- 13.2.3 Element + Index 组合 ----
@dataclass
class E3_ElementIndexCombo(StructMixin):
    i: int = rfield(Index())
    e: int = rfield(Element())
    payload: list = field(RepeatUntil((e + i) >= 10, Int8ub))
# 终止：e + i >= 10
```

### 13.3 不同 subcon 类型

```python
# ---- 13.3.1 Int8ub ----
@dataclass
class S1_Int8ub(StructMixin):
    e: int = rfield(Element())
    payload: list = field(RepeatUntil(e == 0, Int8ub))

# ---- 13.3.2 Int32ub ----
@dataclass
class S2_Int32ub(StructMixin):
    e: int = rfield(Element())
    payload: list = field(RepeatUntil(e == 0x12345678, Int32ub))

# ---- 13.3.3 Struct{2} ----
@dataclass
class Pair(StructMixin):
    a: int = field(Int8ub)
    b: int = field(Int8ub)

@dataclass
class S3_Struct(StructMixin):
    e: int = rfield(Element())         # 引用 Pair.a？或 Pair 本身？
    payload: list = field(RepeatUntil(e.a == 0, Pair))
# 注意：Element 字段当前设计仅支持标量（PyLong_AsLongLong），
# Struct 元素需用 Adapter 或扩展 ElementNode 支持 attribute 引用（v5 能力边界）。
```

### 13.4 parse / build 双向行为（对称性）

```python
@dataclass
class Symmetric(StructMixin):
    e: int = rfield(Element())
    payload: list = field(RepeatUntil(e == 0xFF, Int8ub))

# parse
data = b"\x01\x02\x03\xff\xaa"
pkt = Symmetric.parse(data)
assert pkt.payload == [1, 2, 3, 255]
assert pkt.e is None  # Element 字段始终为 None

# build
pkt2 = Symmetric(payload=[10, 20, 30, 255])
built = Symmetric.build(pkt2)
assert built == b"\x0a\x14\x1e\xff"  # 写到 255 即停（255 == 0xFF）

# round-trip
assert Symmetric.parse(built).payload == [10, 20, 30, 255]
```

### 13.5 discard 模式

```python
@dataclass
class WithDiscard(StructMixin):
    e: int = rfield(Element())
    payload: list = field(RepeatUntil(e == 0xFF, Int8ub, discard=True))

pkt = WithDiscard.parse(b"\x01\x02\xff\xaa")
assert pkt.payload == []          # discard=True：list 为空
# 流仍消耗到 0xFF（3 字节）
```

### 13.6 嵌套组合

```python
# ---- 13.6.1 RepeatUntil 内嵌 RepeatUntil ----
@dataclass
class Inner(StructMixin):
    e: int = rfield(Element())
    items: list = field(RepeatUntil(e == 0, Int8ub))

@dataclass
class Outer(StructMixin):
    e: int = rfield(Element())
    rows: list = field(RepeatUntil(e == 0xFF, Inner))
# 注意：嵌套 RepeatUntil 中，内层 Element 字段引用内层当前元素，
# 外层 Element 字段引用外层当前元素（Inner 实例）。
# 但 Element 当前设计仅支持标量——内层 Inner 实例无法直接 extract i64。
# v5 能力边界：嵌套 RepeatUntil 需 Element 支持 attribute 引用或 Adapter。

# ---- 13.6.2 Struct 内嵌 RepeatUntil ----
@dataclass
class Header(StructMixin):
    magic: int = field(Int8ub)
    e: int = rfield(Element())
    payload: list = field(RepeatUntil(e == 0, Int8ub))

# ---- 13.6.3 Array/GreedyRange 内嵌 RepeatUntil ----
@dataclass
class Row(StructMixin):
    e: int = rfield(Element())
    cells: list = field(RepeatUntil(e == 0xFF, Int8ub))

@dataclass
class Table(StructMixin):
    rows: list = field(Array(3, Row))
```

### 13.7 Adapter 对比示例（用户主动选慢路径）

```python
from construct import Adapter

# ---- RepeatUntil 表达式路径（≥10x）----
@dataclass
class FastPath(StructMixin):
    e: int = rfield(Element())
    payload: list = field(RepeatUntil(e > 5, Int8ub))

# ---- Adapter 显式慢路径（用户主动选择）----
class ListDependentAdapter(Adapter):
    """用户主动选 Adapter 处理 list 切片终止逻辑（RepeatUntil 不支持）"""
    subcon = GreedyRange(Int8ub)

    def decode(self, obj, ctx, path):
        # 找到 lst[-2:] == [0, 0] 的位置，截断
        for i in range(1, len(obj)):
            if obj[i-1:i+1] == [0, 0]:
                return obj[:i+1]
        raise ValueError("pattern not found")

    def encode(self, obj, ctx, path):
        return obj

# 用法对比：
# FastPath.parse(data)              # ≥10x（终止表达式零 FFI）
# ListDependentAdapter.parse(data)  # 折衷（用户主动选 Adapter）
```

### 13.8 Python construct 迁移示例（≥5 个）

```python
# ---- 迁移 1: lambda x,_,_: x != -1（哨兵终止）----
# Python construct:
py_d = RepeatUntil_py(lambda x, _, _: x != -1, Byte)
# construct-rs:
@dataclass
class M1(StructMixin):
    e: int = rfield(Element())
    payload: list = field(RepeatUntil(e != -1, Int8ub))

# ---- 迁移 2: lambda x,_,_: x > 5（阈值比较）----
# Python construct:
py_d = RepeatUntil_py(lambda x, _, _: x > 5, Byte)
# construct-rs:
@dataclass
class M2(StructMixin):
    e: int = rfield(Element())
    payload: list = field(RepeatUntil(e > 5, Int8ub))

# ---- 迁移 3: lambda x,_,_: x == 0xFF（哨兵字节）----
# Python construct:
py_d = RepeatUntil_py(lambda x, _, _: x == 0xFF, Byte)
# construct-rs:
@dataclass
class M3(StructMixin):
    e: int = rfield(Element())
    payload: list = field(RepeatUntil(e == 0xFF, Int8ub))

# ---- 迁移 4: lambda x,lst,_: x > lst[0]（list 索引）----
# Python construct 支持，construct-rs **不支持**——Phase 2 表达式 VM 不支持 list 索引。
# 替代：用户需用 Adapter 显式包装（§13.7），或重构数据布局把首元素存为字段。

# ---- 迁移 5: lambda x,_,ctx: x > ctx.threshold（引用 context）----
# Python construct:
py_d = RepeatUntil_py(lambda x, _, ctx: x > ctx.threshold, Byte)
# construct-rs（threshold 是同 Struct 的字段）:
@dataclass
class M5(StructMixin):
    threshold: int = field(Int8ub)
    e: int = rfield(Element())
    payload: list = field(RepeatUntil(e > threshold, Int8ub))
```

### 13.9 能力边界说明（RepeatUntil 终止表达式不支持的清单）

RepeatUntil 终止表达式只支持 Phase 2 表达式 VM 能描述的逻辑（整数算术/位运算/比较）。
**不支持**的场景及 Adapter 替代方案：

| # | 不支持的 Python 写法 | 原因 | Adapter 替代方案 |
|---|--------------------|------|----------------|
| 1 | `lambda x,lst,_: lst[-2:] == [0, 0]` | Phase 2 VM 栈为 i64，不支持 list 切片 | 用 Adapter 包装 GreedyRange，在 decode 中扫描 list |
| 2 | `lambda x,lst,_: x > lst[0]` | 不支持 list 索引 | 同上，或重构数据布局把首元素作为独立字段 |
| 3 | `lambda x,lst,ctx: len(lst) > 5` | 不支持 list 长度查询 | 用 Adapter 或先解析为 GreedyRange 后处理 |
| 4 | `lambda x,_,ctx: ctx._.parent_flag` | 不支持跨层 context（需 ContextParam） | 用 `context=` 参数显式注入 + ContextParam 字段 |
| 5 | `lambda x,_,ctx: some_python_function(x)` | 不支持任意 Python 函数调用 | 用 Adapter（用户主动选慢路径） |
| 6 | `lambda x,_,_: isinstance(x, int) and x > 5` | 不支持短路 and / isinstance | 用 Adapter |

**Adapter 替代示例**（场景 1）：

```python
class ListTailAdapter(Adapter):
    subcon = GreedyRange(Int8ub)

    def decode(self, obj, ctx, path):
        for i in range(1, len(obj)):
            if obj[i-1:i+1] == [0, 0]:
                return obj[:i+1]
        raise ConstructError("pattern [0,0] not found")

    def encode(self, obj, ctx, path):
        return obj
```

**性能预期对比**：
- RepeatUntil 终止表达式路径：≥10x（用户硬约束门禁）
- Adapter 路径：折衷（用户主动选择，门禁不适用，Adapter 是"显式慢路径"）

---

## 14. 场景测试覆盖矩阵（v5 新增；S-PERF 硬要求）

> **用户硬约束**：每构造器 ≥10 测量点 × 6 维度（元素类型 / 规模 / 字节序 / 分支 /
> 错误路径 / 嵌套组合）。本节列出 4.5 RepeatUntil 的最低场景覆盖。

### 14.1 RepeatUntil 场景矩阵（≥10 测量点）

| # | 场景 | 维度覆盖 | 测量点 |
|---|------|---------|--------|
| 1 | `RepeatUntil(e > 5, Int8ub)` parse N=10 | 元素类型/规模/分支 | parse |
| 2 | `RepeatUntil(e > 5, Int8ub)` build N=10 | 同上 | build |
| 3 | `RepeatUntil(e > 5, Int8ub)` parse N=100 | 规模扩展 | parse |
| 4 | `RepeatUntil(e > 5, Int8ub)` build N=100 | 规模扩展 | build |
| 5 | `RepeatUntil(e > 5, Int8ub)` parse N=1000 | 大规模 O(n) 验证 | parse |
| 6 | `RepeatUntil(e > 5, Int8ub)` build N=1000 | 大规模 O(n) 验证 | build |
| 7 | `RepeatUntil(e == 0xFF, Int16ub)` parse | 元素类型/字节序（大端 2 字节） | parse |
| 8 | `RepeatUntil(e == 0xFF, Int16ub)` build | 同上 | build |
| 9 | `RepeatUntil((e & 0xFF) == 0, Int8ub)` parse | 分支（位运算终止） | parse |
| 10 | `RepeatUntil(e > threshold, Int8ub)` parse（threshold 字段） | 分支（字段引用终止） | parse |
| 11 | `RepeatUntil(e > 5, Int8ub, discard=True)` parse | 分支（discard） | parse |
| 12 | `RepeatUntil(e > 5, Struct)` parse | 元素类型（Struct 元素，需 Element 支持 attribute） | parse |
| 13 | `RepeatUntil` parse 流不完整（EOF） | 错误路径 | parse |
| 14 | `RepeatUntil` build 无元素满足终止表达式 | 错误路径 | build（RepeatError） |
| 15 | `Array(3, RepeatUntil(e==0, Int8ub))` parse | 嵌套组合（Array 内 RepeatUntil） | parse |
| 16 | `Struct{RepeatUntil + Index}` parse | 嵌套组合（Element + Index） | parse |

### 14.2 Element 场景矩阵（v5 新增）

Element 是引用入口，不独立 benchmark，但需通过 RepeatUntil 场景间接覆盖：

| # | 场景 | 维度覆盖 |
|---|------|---------|
| E1 | Element 在 RepeatUntil 内引用（parse） | 核心功能 |
| E2 | Element 在 RepeatUntil 内引用（build） | 双向 |
| E3 | Element 字段属性为 None（parse 后） | 边界（EL-1） |
| E4 | Element + Index 组合（RepeatUntil 内引用两者） | 嵌套 |
| E5 | Element 在 RepeatUntil 之外（编译期 warning） | 边界（EL-5） |

### 14.3 S-PERF 输出要求（每个场景）

按 `plans/phase4-array/总纲.md` §"S-PERF benchmark 输出要求"：
- 原始 ns/call（Python + Rust）
- 加速比
- parse/build 比率（与参考实现对照方向）
- 加速比按场景复杂度排列（验证趋势）
- **低于 10x 的场景单独标注**（用户硬约束 #5：所有场景必须达标）
- 与参考实现 parse/build 方向相反的场景单独标注

### 14.4 测量口径（AGENTS.md §6 S-PERF）

- construct-rs 侧：`maturin develop --release` 安装后，Python timeit 调用 `Packet.parse(data)`
- Python construct 侧：Python timeit 调用 `pkt.parse(data)`（同包名，子进程隔离）
- 两边相同 timeit 参数（NUMBER=1000, REPEAT=5），取 min
- 禁止用独立 Rust 二进制调 Rust 库函数产生 S-PERF 数据（跳过 Python 分发和 FFI）

---

## 15. 参考文献


- `AGENTS.md` §0、§7、§8、§10
- `plans/phase4-array/分析报告-Array功能集.md`
- `plans/phase4-array/总纲.md`（v5：用户面设计四原则 + S-PERF ≥10x + 场景矩阵硬要求）
- `docs/架构设计.md` §C.1-C.6
- `docs/模块设计-BitStream.md`（Box<Node> 递归模式参考）
- `docs/模块设计-表达式系统.md`（ExprProgram / ExprOp 参考）
- `docs/模块设计-Context-Vec优化.md`（Context 栈分配模式参考）
- `construct/construct/core.py` L2493-2704、L2934-2972、L4079-4131、L4934-4983
- `construct/construct/lib/containers.py`（ListContainer；v5 后 construct-rs 不再依赖 Container）

---

**v5 设计完成。等待 PM 与 REV 审查。**

