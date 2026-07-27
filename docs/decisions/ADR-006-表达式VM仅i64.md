---
id: ADR-006
status: accepted
phase: "2"
decides: "表达式 VM 仅 i64，无浮点/lambda/字符串"
supersedes: []
superseded_by: []
depends_on: [ADR-001]
last_updated: 2026-07-27
---

# ADR-006: 表达式 VM 仅 i64

## Context

表达式系统覆盖场景：长度计算、下标引用、计数、位运算。这些都用整数。浮点/字符串/lambda 表达式在二进制协议描述中罕见，且引入 FFI 开销。

## Decision

栈式 VM，操作数类型仅 `i64`。不支持：
- 浮点运算
- Python lambda
- 字符串表达式

## Consequences

- 正面：覆盖长度/计数场景，FFI 零开销
- 正面：VM 实现简单（无需类型分派）
- 负面：用户若需浮点，必须改用 Adapter（显式慢路径）

## Relations

- 引用证据：`docs/design/模块设计-表达式系统.md`
- 关联教训：`experiences.md#L-01`（中间表示层违反）— 表达式 VM 内的 i64 操作不跨 FFI
