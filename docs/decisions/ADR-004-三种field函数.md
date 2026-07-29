---
id: ADR-004
status: accepted
phase: "2"
decides: "三种 field 函数 RW/RO/WO"
supersedes: []
superseded_by: []
depends_on: [ADR-001]
last_updated: 2026-07-27
---

# ADR-004: 三种 field 函数 RW/RO/WO

## Context

字段在 parse 和 build 两个方向的角色不同：
- RW（read-write）：parse 时读，build 时写
- RO（read-only）：parse 时读，build 时忽略（表达式推导）
- WO（write-only）：parse 时忽略，build 时写

数学完备分类为 2^2 = 4 种，但"既不读也不写"无意义，所以 3 种。

## Decision

- `field(subcon)` = RW
- `rfield(subcon)` = RO（read-only，parse 写入实例，build 时从表达式推导）
- `wfield(subcon)` = WO（write-only，parse 时跳过，build 时用户提供）

**WO 字段编译期禁止被表达式引用**（因为 parse 路径不会写入它，表达式求值时无值）。

## Consequences

- 正面：用户一眼可知字段角色
- 正面：编译期类型检查（WO 引用报编译错误）
- 负面：迁移自 Python construct 的代码需要标注

## Relations

- 引用证据：`docs/design/模块设计/模块设计-表达式系统.md`
