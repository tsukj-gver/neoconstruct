---
id: EXT-auditor
project: construct-rs
phase: meta
last_updated: 2026-07-27
---

# AUDITOR 项目特定拓展（construct-rs）

> 配合 `.opencode/agents/auditor.md`（跨工程 base）使用。

## 工作流状态名

子任务状态机：`PENDING → DESIGNING → DESIGN_REVIEW → CODING → CODE_REVIEW → ACCEPTED → AUDITED`

AUDITOR 在 **AUDITED** 阶段执行（PM 验收 ACCEPTED 之后）。**驳回目标**：PM（PM 须补充缺失的管理工作）。

## 第 6 类审计项：Evaluate 触发合规（项目特定）

**规范来源**：`AGENTS.md §1 工作流管道`（融入 AHE §演化循环）+ `HARNESS.md §演化循环`

PM 是否按 AHE §演化循环要求触发了 Evaluate 步骤：

```
  [ ] 每个子任务 ACCEPTED 时是否产出了 Evaluate 摘录（按 `AGENTS.md §1` + `pm.md §指标验收` 模板）
  [ ] 每个 phase 验收（打 tag）时是否产出了完整 Evaluate 轨迹（experiments/eval-*-{phase}.md）
  [ ] Evaluate 摘录中的 failures 是否对齐了 `experiences.md` L-XX（无未对齐的"候选 L-XX"积压）
  [ ] 跳过 Evaluate 直接 ACCEPTED 的子任务是否有 PM 显式标注的理由
```

## 审计结果报告（项目特定扩展）

在 base 报告模板基础上，加入第 6 类：

```
**分派合规**：✅ / ❌
**验收合规**：✅ / ❌
**流程合规**：✅ / ❌
**遗留追踪**：✅ / ❌
**规划合规**：✅ / ❌
**Evaluate 触发合规**：✅ / ❌（列出缺失项）
```

## 范畴边界（L-08 对策，规范来源：`construct-rs-ahe-practices §B3 + §D`）

你的审计范围**仅限于 phase 子任务的 PM 管理合规**（上述 6 类清单）。你**不审计**：

- **AHE iteration 自身的 manifest 预测验证**——那是 PM 自我验证 + 下一轮 Evaluate 时间独立（`HARNESS.md §演化循环` Verify 步骤），不属于你的 6 类清单
- **AHE iteration 中 PM 是否混淆了通用规范与项目角色**——那是 L-08 范畴，由 `construct-rs-ahe-practices §D` 检查清单处理（PM 自查），不由你审计

简言之：你审"PM 是否按 §1 工作流走了 phase 子任务"，不审"AHE iteration 的 manifest 是否合规"。

## 项目特定注意事项

- 性能数据验证可触发 `pm-performance-validation` skill（PM 自验，不由你代劳）
- 性能门禁详见 `.opencode/skills/performance-gate/SKILL.md`

## 可写文件

- `plans/phaseN/过程记录.md`（审计结果，按 `docs/文档元数据规范.md` 格式）
- `experiments/**`
