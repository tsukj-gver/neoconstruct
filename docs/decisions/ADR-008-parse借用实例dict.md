---
id: ADR-008
status: accepted
phase: "2.5"
decides: "parse 借用实例 __dict__ 直接填充，无中间 dict"
supersedes: []
superseded_by: []
depends_on: [ADR-007]
last_updated: 2026-07-27
---

# ADR-008: parse 借用实例 `__dict__` 直接填充

## Context

Phase 2 实现的 parse 路径：
1. Rust 内构造中间 `PyDict`
2. `force_setattr` 把字段写入 dict
3. dict 跨 FFI 返回到 Python
4. Python 侧 `cls(**dict)` 构造实例

**这条路径违反 AGENTS.md §0（一次 FFI / 无中间表示）**——dict 跨越 FFI 边界。Phase 1 设计修订 R1/R2/R3 都未发现此违反。详见 `docs/archive/phase1/设计修订-parse路径优化.md` R3 修订记录。

关联教训：`experiences.md#L-01`（中间表示层违反）。

## Decision

parse 路径改造：
1. Rust 内先 `tp_new` 创建实例（空的 `__dict__`）
2. 借用实例自带的 `__dict__` 作为 PyDict 直接填充字段
3. 实例 PyObject 跨 FFI 返回（一次 FFI）

无独立中间 dict + force_setattr。

## Consequences

- 正面：消除中间 dict + force_setattr 开销
- 正面：消除 dict 跨 FFI 边界违反
- 正面：parse 性能 ~10-12x（Phase 2.5 实测）
- 中性：`take_fields` / `force_setattr` 函数保留但 parse 不再调用（dead path）

## Relations

- 引用证据：`docs/design/模块设计-Context-Vec优化.md` / `docs/archive/phase1/设计修订-parse路径优化-借用实例dict.md`
- 关联教训：`experiences.md#L-01`（中间表示层违反） + `experiences.md#L-05`（优化 A 路径忽略 B 路径）
- 修订历史：R1/R2/R3 → R4（详见 `docs/archive/phase1/`）
