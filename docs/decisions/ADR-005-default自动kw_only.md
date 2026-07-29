---
id: ADR-005
status: accepted
phase: "2"
decides: "default 自动 kw_only"
supersedes: []
superseded_by: []
depends_on: []
last_updated: 2026-07-27
---

# ADR-005: `default` 自动 `kw_only`

## Context

dataclass 的 `field(default=value)` 与位置参数顺序冲突——有默认值的字段必须在无默认值字段之后。但二进制协议声明顺序由字节布局决定，不能任意调整。

## Decision

`field(subcon, default=value)` 自动标记为 `kw_only=True`，解决字段顺序冲突。

## Consequences

- 正面：字节声明顺序与 init 顺序解耦
- 正面：用户写 `field(subcon, default=0)` 无需担心顺序

## Relations

- 引用证据：`docs/design/模块设计/模块设计-表达式系统.md`
- 关联：dataclass mixin 设计见 `docs/design/基础设施/Mixin设计.md`
