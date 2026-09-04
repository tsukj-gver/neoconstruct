---
id: EXT-reviewer
project: neoconstruct
phase: meta
last_updated: 2026-07-27
---

# REV 项目特定拓展（neoconstruct）

> 配合 `.opencode/agents/reviewer.md`（跨工程 base）使用。
> 工作流状态机与子任务标识格式见 `AGENTS.md §1`。
> REV 在 **DESIGN_REVIEW** 阶段执行。**驳回目标**：`DESIGNING`。

## §0 原则对照（硬要求，L-01 对策）

涉及 parse/build 数据流的设计**必须**包含 §0 原则对照表（`AGENTS.md §0` 核心原则全条），逐条说明设计如何满足。**未提供对照表直接驳回**。

## 性能检查项目特定补充

补充 base §性能 的通用检查项，加入 neoconstruct 特有：

- [ ] FFI 边界（Python 互操作）的穿越次数有量化分析
- [ ] 数据结构选择合理（IndexMap vs Vec vs HashMap 等 Rust 特定容器）

## 可行性检查项目特定补充

- [ ] 设计在 **Rust + PyO3** 技术栈下可实现
- [ ] 无已知的技术障碍（如 PyO3 0.22 的 API 限制）

## 完备性检查项目特定补充

- [ ] 没有遗漏的 **Python 行为**（参考实现：`construct/`，源码位置速查见 `.opencode/agents/architect.md §Python 参考速查`）

## 跨阶段决策检查

设计不可违反 `docs/decisions/`（ADR-001~ADR-NNN，索引 `docs/decisions/README.md`）。每个版本批次设计时先 read ADR 与 `experiences.md`，避免重新决策（L-04 对策）。

## 可写文件

- `plans/phaseN/过程记录.md`（检视结果，按 `harness/metadata-convention.md` 格式）
- `experiments/**`
