---
id: ADR-002
status: accepted
phase: "2"
decides: "不提供 len_，用 Tell() + Computed() 替代"
supersedes: []
superseded_by: []
depends_on: []
last_updated: 2026-07-27
---

# ADR-002: 不提供 `len_`

## Context

Python construct 提供 `len_(this.xxx)` 表达式获取字段长度。在 construct-rs 中：

1. `len_` 的语义模糊——bytes 长度？list 元素数？
2. 用 `Tell()`（当前位置）+ `Computed()`（表达式）能更清晰地表达"长度"概念

## Decision

不提供 `len_`。用户用 `Tell()` + `Computed()` 显式表达长度需求。

## Consequences

- 正面：语义更清晰——用户必须显式说明"长度从哪里来"
- 负面：迁移自 Python construct 的代码需要改写

## Relations

- 引用证据：`docs/design/模块设计/模块设计-表达式系统.md`
