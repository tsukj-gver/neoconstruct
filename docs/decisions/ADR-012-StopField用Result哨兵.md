---
id: ADR-012
status: accepted
phase: "4"
decides: "StopField 用 Result 哨兵变体（不用异常做控制流）"
supersedes: []
superseded_by: []
depends_on: []
last_updated: 2026-07-27
---

# ADR-012: StopField 用 Result 哨兵变体

## Context

GreedyRange / RepeatUntil 需要在迭代中"提前停止"。Python construct 用 `StopFieldError` 异常做控制流。

Rust 不适合用 panic/异常做常规控制流（unwinding 开销大，且破坏 `Result` 范式）。

## Decision

引入 `ConstructError::StopField` 变体作为哨兵。StructNode / GreedyRangeNode 捕获此变体视为正常终止（不传播）。

## Consequences

- 正面：符合 Rust `Result` 范式
- 正面：无 unwinding 开销
- 正面：错误传播路径统一（所有 ConstructError 走 `?`）

## Relations

- 引用证据：`docs/design/模块设计/模块设计-Array.md`
- Python 参考：`construct/construct/core.py` StopIf
