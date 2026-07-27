---
id: ADR-009
status: accepted
phase: "3"
decides: "bit 顺序默认 MSB-first"
supersedes: []
superseded_by: []
depends_on: []
last_updated: 2026-07-27
---

# ADR-009: bit 顺序默认 MSB-first

## Context

Python construct 的 Bitwise 默认 MSB-first（big-endian bit order），通过 BitsSwapped 包装切换为 LSB-first。

## Decision

bit 顺序默认 MSB-first。LSB-first 通过 BitsSwapped 包装实现，**不是参数**。

## Consequences

- 正面：与 Python construct 行为一致
- 中性：LSB 场景需用 BitsSwapped 包装

## Relations

- 引用证据：`docs/design/模块设计-BitStream.md`
- Python 参考：`construct/construct/core.py` Bitwise / BitsSwapped
