---
id: ADR-010
status: accepted
phase: "3"
decides: "剩余位不自动 padding"
supersedes: []
superseded_by: []
depends_on: [ADR-009]
last_updated: 2026-07-27
---

# ADR-010: 剩余位不自动 padding

## Context

Bitwise 解析若干 BitsField 后可能剩余不足 8 位。Python construct 的行为是报错（要求用户显式 Padding）。

## Decision

不足 8 的倍数时报错。用户必须用 Padding 补齐。

## Consequences

- 正面：与 Python construct 行为一致
- 正面：避免静默 padding 导致的歧义
- 负面：用户需手动补齐

## Relations

- 引用证据：`docs/design/模块设计-BitStream.md`
- Python 参考：`construct/construct/core.py` Bitwise
