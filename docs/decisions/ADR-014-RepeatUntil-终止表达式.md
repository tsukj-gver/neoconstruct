---
id: ADR-014
status: accepted
phase: "4"
decides: "RepeatUntil 终止表达式 = Phase 2 表达式（v5 用户硬约束）"
supersedes: [ADR-013]
superseded_by: []
depends_on: [ADR-001, ADR-006, ADR-012]
last_updated: 2026-07-27
---

# ADR-014: RepeatUntil 终止表达式 = Phase 2 表达式

> 本 ADR 替代 [ADR-013](ADR-013-RepeatUntil-PyCallable路径.md)。v5 用户硬约束 #1-#5。

## Context

v4 设计（ADR-013）的"PyCallable + 白名单兜底"路径：
1. 违反 AGENTS.md §0 一次 FFI 原则（每次迭代调 Python）
2. 隐藏决策路径（白名单命中走 Rust，兜底走 Python，用户无法预测）
3. 性能门禁 1.5x/3x 无物理推导

用户 2026-06-30 硬约束打回：
- #1 禁止 PyCallable 路径
- #2 禁止"白名单+兜底"二分设计
- #3 禁用"谓词"术语
- #4 只认 Phase 2 表达式语法
- #5 RepeatUntil 不自建表达式机制

## Decision

RepeatUntil 用户面 API `RepeatUntil(terminator, subcon, discard)` 中 `terminator` 必须是 **Phase 2 表达式**（`_FieldDescriptor` / `_ExprRef` / `int` 组合），编译为 ExprProgram，运行时零 FFI 求值。

**不接收 Python lambda / callable**。

### 具体删除

删除 v4 的：
- `RepeatPredicate` 枚举
- `call_repeat_predicate` 函数
- `build_context_proxy` 函数
- `container_cache.rs` 模块
- AST 识别器 `_try_compile_repeat_predicate`

### API 变更

- 用户面参数 `predicate` 改名为 `terminator`
- 禁用"谓词"术语，统一改用"终止表达式"或直接"表达式"

### 边界

无法用 Phase 2 表达式描述的终止逻辑（如 `lambda x,lst,ctx: lst[-2:] == [0, 0]`）用户须改用 **Adapter**（显式慢路径，不在 RepeatUntil 内部交汇）。

## Consequences

- 正面：消除"白名单+兜底"二分设计——RepeatUntil 内部无 PyCallable 路径
- 正面：用户面 API 表面可预测性能（无隐藏慢路径）
- 正面：与 Phase 2 表达式系统完全统一
- 负面：用户复杂终止逻辑需改用 Adapter
- 中性：性能门禁上调至 ≥10x（数值推导见 `docs/design/模块设计-Array.md` §8.3）

## Relations

- 替代 ADR-013
- 关联事件：用户 2026-06-30 Phase 4 重审硬约束（见 `plans/phase4-array/总纲.md`）
- 引用证据：`docs/design/模块设计-Array.md` §8.3（≥10x 数值推导）
- 关联教训：`experiences.md#L-01`（一次 FFI）+ `experiences.md#L-04`（决策路径隐藏）
