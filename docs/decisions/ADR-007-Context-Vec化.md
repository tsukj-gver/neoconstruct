---
id: ADR-007
status: accepted
phase: "2.5"
decides: "Context Vec 化：GetInt 走编译期索引数组"
supersedes: []
superseded_by: []
depends_on: [ADR-001]
last_updated: 2026-07-27
---

# ADR-007: Context Vec 化

## Context

Phase 2 实现中，GetInt 操作通过 PyDict hash 查找字段值（~12.5ns/次）。性能基准显示该路径成为瓶颈。

详见 `docs/analysis/perf-p0-analysis.md`。

## Decision

Context 内部维护 `Vec<Py<PyAny>>`（编译期已知大小），GetInt 通过编译期字段索引直接数组访问（~3ns/次）。

PyDict 仍保留（用于 Python 侧属性访问），但表达式 VM 不再走 PyDict。

## Consequences

- 正面：表达式 GetInt 性能 ~4x 提升
- 正面：消除表达式路径的 PyDict hash 开销
- 中性：双存储（Vec + PyDict）轻微增加内存

## Relations

- 引用证据：`docs/design/模块设计-Context-Vec优化.md`
- 关联分析：`docs/analysis/perf-p0-analysis.md`
- 实施：Phase 2.5 E1-E3 全部 ~10-12x（见 `plans/00-项目进度.md`）
