---
id: ADR-016
status: accepted
phase: cross
decides: "P0-3 lazy path 错误传播模式"
supersedes: []
superseded_by: []
depends_on: []
last_updated: 2026-07-27
---

# ADR-016: P0-3 lazy path 错误传播模式

## Context

错误路径定位（path 字段）的传统做法是成功路径每次进入/退出子节点时 `path.push_field(name)` / `path.pop()`。每次 push 涉及 String 分配 + Vec 操作，在性能敏感路径上是显著开销。

Phase 1 实测：成功路径占比 >99%，错误路径 <1%。

## Decision

**成功路径不维护 Path 栈**（零 String 分配，零 Vec 操作）。

子节点返回 `Err` 时，父节点通过 `ConstructError::push_path_segment(name)` 或 `push_path_index(i)` 重建路径段。

Path 类型为 enum：
```rust
enum Path {
    Root,                      // 成功路径走此分支（零分配）
    Segments(Vec<PathSegment>), // 错误路径走此分支
}
```

## 适用范围

所有含递归子节点的节点：
- Struct / Array / GreedyRange / PrefixedArray / RepeatUntil
- Bitwise / Bytewise / Transform / StructRef

## 禁止行为

- ❌ 成功路径调用 `path.push_field` / `path.push_index` / `path.pop`
- ❌ "首版保留 push/pop，后续优化"的重新决策

## Consequences

- 正面：成功路径零分配，错误路径开销可接受
- 负面：错误时需重建路径（但错误是罕见事件）

## Relations

- 首次验证：Phase 1（StructNode）
- 推广：Phase 4.7（Array 系列）
- 关联教训：`experiences.md#L-04`（Phase 4 未采用此模式导致性能塌方后补迁移）
- 引用证据：`docs/reviews/架构审查-重复代码与抽象质量.md` §1.2
- 强制规范：每个新 Node 设计文档必须含"模式采用声明"段（见 `docs/decisions/README.md`）
