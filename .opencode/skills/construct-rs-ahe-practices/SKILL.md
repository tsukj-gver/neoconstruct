---
description: construct-rs 项目对 AHE（Agentic Harness Engineering）通用工作流的项目特定补充规则。当 PM 执行 AHE iteration（生成 manifest / verify / harness 重组）时加载，作为 agentic-harness-engineering skill 的项目级附加约束。
---

# construct-rs AHE Practices — 项目级 AHE 补充

> **关系**：本 skill 是 [`.opencode/skills/agentic-harness-engineering/SKILL.md`](../agentic-harness-engineering/SKILL.md)（AHE 通用工具，上游副本）的**项目级补充**。
>
> - 通用 AHE skill 提供 4 个工作流（Audit / Generate Manifest / Verify / Init）
> - 本 skill 在通用工作流之上**追加**项目特定检查项与触发条件
> - 两者冲突时，**本 skill 优先**（项目实践覆盖通用默认）
>
> **加载时机**：与通用 AHE skill 同时加载（任何 AHE iteration 任务）。

---

## 触发本 skill 的额外检查项

### A. Generate Manifest 时（通用工作流 2 补充）

通用工作流 2 的步骤 5 是"预测影响"——抽象描述。本 skill 强制：

#### A1. Cross-Reference Migration Check（涉及文件移动/重命名/重组时强制）

**触发条件**：本次修改包含任意 `git mv`、文件路径变更、目录重组。

**强制动作**：

1. 用 `grep -r "docs/<旧路径>" --include="*.md" .`（或 plans/ 等）列出所有引用旧路径的位置
2. 把每个引用作为 manifest `at_risk_regressions` 字段的**具体条目**（格式："文件 X 第 Y 行引用 Z 会 broken"）
3. 在 `targeted_fix` 字段明确"批量替换 N 处引用为新路径"
4. **漏报后果**：iteration 3 实测 42 处 broken refs 完全未预测（详见 `experiences.md#L-07`），导致 commit 后需要 verify 修复

#### A2. Frontmatter 新增清单（涉及 frontmatter 规范时强制）

**触发条件**：本次修改涉及要求 frontmatter 的目录（`docs/` 下所有文件、`plans/phaseN/过程记录-*.md`、`plans/phaseN/总纲.md`、`plans/phaseN/分析报告-*.md`、`plans/phaseN/索引.md`）。

**强制动作**：在 `targeted_fix` 字段列出所有**新建/已存在但缺 frontmatter** 的文件清单。

#### A3. 不接受"功能性 at_risk_regressions"作为唯一预测

manifest 的 `at_risk_regressions` 必须包含**路径回归**和**元数据回归**类别，不能只有"功能可能破坏"。

### B. Verify 时（通用工作流 3 补充）

通用工作流 3 原触发条件"到了 scheduled_at 时间"过严。本 skill 强制：

#### B1. Commit 前必须自验证（不论 scheduled_at）

**强制动作**：每次 AHE iteration commit 前，必须执行以下自动扫描：

| 扫描项 | 命令 | 失败处理 |
|--------|------|---------|
| Broken refs | grep 所有 `.md` 中 `docs/<path>.md` / `plans/<path>.md` 引用，验证目标存在 | 批量替换或回滚 |
| Frontmatter 完整性 | 所有应加 frontmatter 的文件必须含 `id` / `status` / `phase` / `last_updated` | 补加 |
| YAML 解析 | 首行必须是 `---`，且第二行 `---` 之前能正确闭合 | 修复格式 |

#### B2. 漏报必填条款

若 commit 后验证发现 manifest `predicted_impact` 未覆盖的回归，**必须**在 manifest 的 `verification.result.false_predictions` 字段如实记录。**禁止静默修复**——漏报本身就是 AHE 工具改进的输入。

#### B3. AUDITOR 审计 AHE iteration 时检查 at_risk_regressions 覆盖度

AUDITOR 审计 manifest 时，若修改涉及文件重组但 `at_risk_regressions` 为空或仅含功能性条目 → 驳回。

---

## 与通用 AHE skill 的差异总结

| 维度 | 通用 AHE skill | 本 skill（项目补充） |
|------|---------------|--------------------|
| 工作流 2 步骤数 | 6 步 | +3 步（A1/A2/A3） |
| 工作流 3 触发 | scheduled_at 到时 | **每次 commit 前**（B1） |
| `at_risk_regressions` | 开放列表，agent 凭直觉 | 必须含路径回归 + 元数据回归类别 |
| Verify 漏报处理 | agent 自决 | **强制记入 false_predictions**（B2） |

---

## 关联文档

- 通用 AHE skill：`.opencode/skills/agentic-harness-engineering/SKILL.md`
- 教训 L-07：`experiences.md#L-07`
- 文档元数据规范：`docs/文档元数据规范.md`
- 历史失败证据：`manifests/change_2026-07-27-docs-restructure.json` verification.regressions_observed

---

## 维护

- 本 skill 由 PM 维护
- 通用 AHE skill 升级时（从上游同步），本 skill 不受影响（项目特定）
- 新发现的 AHE 实践盲区应同时沉淀到 `experiences.md#L-XX` 和本 skill 的对应检查项
