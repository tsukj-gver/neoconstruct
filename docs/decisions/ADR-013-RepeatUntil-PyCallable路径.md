---
id: ADR-013
status: superseded
phase: "4"
decides: "RepeatUntil PyCallable 谓词 context proxy 用 Container 包装（v4）"
supersedes: []
superseded_by: [ADR-014]
depends_on: [ADR-012]
last_updated: 2026-07-27
---

# ADR-013: RepeatUntil PyCallable 谓词 Container 包装（v4，已作废）

> ⚠️ **本决策已被 [ADR-014](ADR-014-RepeatUntil-终止表达式.md) 替代**。保留此文档仅供演化追溯。

## Context（v4 当时的判断）

v4 设计中，RepeatUntil 接收 Python callable 作为谓词，通过 AST 识别"简单表达式"走 Rust 快路径，复杂 lambda 走 PyCallable 慢路径。谓词的 context proxy 包装为 `construct.lib.containers.Container` 实例。

## Decision（已被作废）

PyCallable 路径传给谓词的 context proxy **必须**是 `construct.lib.containers.Container` 实例。

## Consequences（已观察到的负面后果）

- 负面：**违反"一次 FFI"核心原则**（PyCallable 路径每次迭代调 Python）
- 负面：**隐藏决策路径**——"白名单 + 兜底"二分设计让用户无法预测性能
- 负面：v4 性能门禁 1.5x/3x 无物理推导（Python 3.14 实测 Python 调用 270-300ns，v4 估算高估 2-3 倍）

## Relations

- 被 ADR-014 替代（v5 用户硬约束 #1-#4）
- 关联事件：用户 2026-06-30 打回 Phase 4 验收
- 关联教训：`harness/experiences.md#L-04`（跨阶段模式未沉淀——v4 决策路径隐藏）+ `harness/experiences.md#L-02`（理论估算替代实证数据——v4 性能估算无物理推导）
