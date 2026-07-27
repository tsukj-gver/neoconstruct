---
id: ADR-018
status: accepted
phase: cross
decides: "_index save/restore 配对模式"
supersedes: []
superseded_by: []
depends_on: []
last_updated: 2026-07-27
---

# ADR-018: `_index` save/restore 配对

## Context

Array 系列节点通过 `ctx.set_index(i)` 设置当前下标。嵌套数组（Array of Array）的语义：内层覆盖外层，循环结束后恢复。

错误实现：只 set 不 restore 会导致外层迭代看到错误的 _index 值。

## Decision

数组迭代前：
```rust
let old = ctx.index();
ctx.set_index(i);
// ... 迭代体 ...
```

**所有退出路径**（成功 return / 错误 return / break）必须 `ctx.restore_index(old)`。

嵌套数组通过覆盖+恢复支持（push 旧值，循环结束 pop）。

Phase 4.5 v5 将此提升为 `Context::restore_index` 方法（封装通用模式）。

## 适用范围

所有使用 `ctx.set_index` 的 Node（Array 系列）。

## 禁止行为

- ❌ 只有 `set_index` 无 `restore_index`
- ❌ 错误路径漏掉 restore

## Consequences

- 正面：嵌套数组语义正确
- 正面：错误路径不污染 context

## Relations

- 首次验证：Phase 4（ArrayNode）
- 整合：Phase 4.5 v5（`Context::restore_index` 方法）
- 引用证据：`docs/reviews/架构审查-重复代码与抽象质量.md` §4 整改清单（P0-1）
