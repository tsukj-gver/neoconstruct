---
id: ADR-003
status: accepted
phase: "2"
decides: "嵌套跨层引用用显式 context="
supersedes: []
superseded_by: []
depends_on: [ADR-001]
last_updated: 2026-07-27
---

# ADR-003: 嵌套跨层引用用显式 `context=`

## Context

Python construct 用 `this._._.xxx` 表达跨层引用（向上回溯祖先 context）。这种隐式魔法：

1. 难以静态分析（编译期不知道引用哪一层）
2. 性能差（多层回溯）
3. 用户难以预测行为

## Decision

嵌套跨层引用用显式 `context=` 参数传递。不靠 `this._._` 魔法。

**显式优于隐式。**

## Consequences

- 正面：编译期可静态分析
- 正面：用户一眼看出引用来源
- 负面：用户需手动传递 context 参数

## Relations

- 引用证据：`docs/design/模块设计/模块设计-表达式系统.md`
