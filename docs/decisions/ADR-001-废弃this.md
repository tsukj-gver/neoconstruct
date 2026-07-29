---
id: ADR-001
status: accepted
phase: "2"
decides: "废弃 this，用编译期字段引用"
supersedes: []
superseded_by: []
depends_on: []
last_updated: 2026-07-27
---

# ADR-001: 废弃 `this`，用编译期字段引用

## Context

Python construct 用 `this.xxx` 表达字段引用，运行时通过 `Path` 对象在 context 中回溯求值。这套机制：

1. 运行时开销大（每次求值走 `__getattr__` 链）
2. 跨 FFI 表达困难（`this._._` 链无法在 Rust 栈式 VM 中高效求值）
3. 用户写 `Bytes(this.count)` 时，`this` 实际指 Container 上下文——语义模糊

## Decision

用户写 `Bytes(count)` 而非 `Bytes(this.count)`。字段名本身就是引用对象，编译期通过 `id()` 绑定到字段索引。

**禁止**在 construct-rs 设计和文档中使用 `this.xxx` 语法。

## Consequences

- 正面：消除运行时 `this` 求值开销；编译期绑定后表达式 VM 只需索引查找
- 正面：用户 API 更简洁
- 负面：迁移自 Python construct 的代码需要改写（不可机械转换）
- 中性：跨层引用见 ADR-003

## Relations

- 引用证据：`docs/design/模块设计/模块设计-表达式系统.md`
- 关联 Python 源码：`construct/construct/expr.py`（Path 类）
