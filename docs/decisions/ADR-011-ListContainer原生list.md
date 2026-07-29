---
id: ADR-011
status: accepted
phase: "4"
decides: "ListContainer 返回原生 list"
supersedes: []
superseded_by: []
depends_on: []
last_updated: 2026-07-27
---

# ADR-011: ListContainer 返回原生 list

## Context

Python construct 的 Array 系列返回 `ListContainer`（`list` 子类，支持嵌套打印和 `search` 方法）。

在 construct-rs 中：
1. 引入 Python 子类意味着跨 FFI 调用 Python 类型构造器（违反 §0 一次 FFI）
2. `search` 等扩展方法用户面使用率低

## Decision

parse 直接返回 Python `list`，**不包装为 ListContainer 子类**。

## Consequences

- 正面：性能优先（无 Python 子类构造开销）
- 正面：`==` 行为与原生 list 一致
- 负面：用户依赖 ListContainer 的 `search` / 嵌套打印时需自行处理

## Relations

- 引用证据：`docs/design/模块设计/模块设计-Array.md`
- Python 参考：`construct/construct/lib/containers.py` ListContainer
