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

> ⚠️ **范畴提示（L-08 对策）**：本节"B3"中的 AUDITOR 仅适用于 **phase 子任务验收**（项目自定义流程）。
> AHE iteration 本身的 verify 由 **PM 自我验证 + 下一轮 Evaluate 时间独立** 完成（AHE 规范要求），
> 项目 AUDITOR 不审计 AHE iteration。详见 §E。

---

## C. Evaluate 步骤（通用 AHE skill 未覆盖，项目级补充）

> **规范来源**：本节是项目级补充。通用 AHE skill（`.opencode/skills/agentic-harness-engineering/SKILL.md`）
> 提供了 Audit / Generate Manifest / Verify / Init 四个工作流，**未覆盖** HARNESS.md §演化循环中的
> Evaluate（用当前 harness 跑任务收集失败轨迹）和 Analyze（轨迹蒸馏为根因报告）。
> PROJECT_README.md 明确标注这两步为"⏳ 未实现（后续迭代）"。
> 本节把项目级 Evaluate 协议落实为可执行检查项。

### C1. 触发时机（必做）

| 时机 | 输入 | 产出 |
|------|------|------|
| **每 AHE iteration 开始** | 上一轮 pending manifest + 本轮 iteration dogfood 轨迹 | `experiments/eval-{timestamp}-{task_id}.md` |
| **每 phase 子任务 ACCEPTED 时** | 子任务过程记录 | 轨迹摘录（合并到下次 AHE iteration 的 Evaluate） |
| **AUDITOR 审计 phase 验收发现异常** | AUDITOR 报告 | 临时 Evaluate（轨迹文件同样进 experiments/） |

> 反面教材：iter1-4 全部跳过 Evaluate，凭 PM 直觉/审计发现直接 Improve。
> 这违反通用 AHE skill 的 Evidence-Driven 原则（"不要基于直觉、猜测或'最佳实践'做修改"）。
> iter5 是首个执行 Evaluate 的 iteration（dogfood = iter5 自身执行轨迹）。

### C2. 测试任务集（construct-rs 项目特定）

| 类别 | 示例任务 | 检验的 harness 能力 |
|------|---------|---------------------|
| **启动类** | 新 agent 读 AGENTS.md / MEMORY.md / experiences.md 后回答"Phase 4 当前状态及阻塞点" | LTM 索引可读性 / 跨文件指针一致性 |
| **检索类** | 找 P0-3 lazy path 模式的设计决策记录 / 找 §0 原则对照表模板 | ADR / 教训索引检索效率 |
| **执行类** | 一个完整子任务（PENDING → ... → ACCEPTED） | 工作流合规 / 角色隔离 / manifest 生成 |

> AHE 论文用 Terminal-Bench 类任务集；construct-rs 的"测试任务集"= 真实开发任务。

### C3. 轨迹文件格式

每次 Evaluate 产出一份轨迹文件（写入 `experiments/eval-{YYYY-MM-DD}-{task_id}.md`），含：

| 字段 | 说明 |
|------|------|
| `task_id` | 任务标识（如 `iter5-dogfood` / `phase4-4.1-验收`） |
| `task_category` | 启动类 / 检索类 / 执行类 |
| `tool_calls` | 工具调用序列（含次数 + 重复检测——同操作 ≥2 次记为 repeat） |
| `decision_points` | 关键决策点 + 依据（**引用规范条款必须标注来源，详见 §E2**） |
| `failures` | 失败/重试/用户驳斥 + 模式分类（与 experiences.md L-XX 对齐） |
| `outcome` | 完成 / 部分完成 / 失败 |

### C4. 失败模式分类（强制对齐 L-XX）

每个 failure 必须尝试对齐到 `experiences.md` 已有教训（L-01~L-08）：

- 若匹配 L-XX → 在轨迹中标注 `L-XX`，可立即触发 manifest 生成
- 若不匹配任何 L-XX → 标记 `候选 L-XX`，进入 Analyze 步骤评估是否新增教训

### C5. Analyze 步骤（从轨迹到 evidence）

收集 ≥1 份轨迹后，PM 执行 Analyze：

1. 跨轨迹找模式化失败（同一失败在 2+ 份轨迹中出现 → 模式化）
2. 提取根因（不是"工具报错"，而是"为什么 agent 不知道正确用法"）
3. 若现有 L-XX 对策可覆盖 → 不新增教训，触发对应对策
4. 若无覆盖 → 在 `experiences.md` 新增 L-XX，同时在 §D（本节）补充对应 Evaluate 检查项

---

## D. AHE 规范解读检查清单（L-08 对策）

> **规范来源**：本节是项目级补充，对策对应 `experiences.md §L-08`（AHE 规范解读层错误）。
> L-08 是 iter5 dogfood 首次发现的认知层错误，比 L-07（执行层 predicted_impact 漏报）更基础。

### D1. 触发条件（强制）

**每次 AHE iteration 开始前，PM 必须执行本清单。** 不允许跳过——iter5 自身就是因为跳过本清单导致误判 AUDITOR 缺位为漏洞。

### D2. 角色/流程归属判定

对本 iteration 涉及的每个角色或流程，回答：

| 问题 | 答"是" | 答"否" |
|------|--------|--------|
| (a) 该角色/流程在 HARNESS.md 中定义？ | → 通用规范 | → 继续判断 |
| (b) 该角色/流程在通用 AHE skill 中定义？ | → 通用规范 | → 继续判断 |
| (c) 该角色/流程仅在项目 AGENTS.md / `.opencode/agents/*.md` / 项目级 skill 中定义？ | → 项目自定义 | → 标记为"边界模糊，需用户澄清" |

**规则**：项目自定义角色/流程**不适用于** AHE iteration 本身（AHE iteration 由 PM 执行，
verify 由 PM 自我验证 + 下一轮 Evaluate 时间独立）。

**iter5 反面教材**：
- 涉及角色：AUDITOR
- (a) HARNESS.md 是否定义 AUDITOR？**否**（HARNESS.md §演化循环：Evaluate → Analyze → Improve → Verify，无 AUDITOR）
- (b) 通用 AHE skill 是否定义 AUDITOR？**否**（4 个工作流：Audit / Generate / Verify / Init，"Audit"是结构审计不是角色）
- (c) 仅在项目 `auditor.md` 定义？**是** → 项目自定义 → 不适用于 AHE iteration verify

### D3. 引用规范条款必须标注来源

manifest / 轨迹文件 / 过程记录中所有 `failure_evidence` / `root_cause` / `targeted_fix` / `rationale` 引用规范条款时，必须显式标注来源前缀：

| 来源前缀 | 含义 | 示例 |
|---------|------|------|
| `HARNESS.md §X` | HARNESS.md v1.0 硬要求 | `HARNESS.md §演化循环` |
| `AHE skill 工作流 Y` | 通用 AHE skill 工作流 | `AHE skill 工作流 3` |
| `AGENTS.md §Z` | 项目 System Rules | `AGENTS.md §3 工作流` |
| `construct-rs-ahe-practices §W` | 项目级 AHE 补充（本 skill） | `construct-rs-ahe-practices §D2` |
| `auditor.md` / `pm.md` 等 | 项目级 agent 角色定义 | `auditor.md §审计清单` |
| `experiences.md §L-XX` | 跨阶段教训 | `experiences.md §L-07` |

### D4. "X 角色未执行 Y 流程" 判断的强制前置检查

PM 发现"某角色未执行某流程"时，**禁止**直接判定为漏洞。必须先：

1. 查 `HARNESS.md` / `AHE skill` 确认 Y 是否 AHE 规范要求
2. 若 Y 仅在项目文件中出现（`AGENTS.md` / `auditor.md` / 项目级 skill） → 标注"项目自定义，不适用于 AHE iteration"
3. 仅当 Y 是 AHE 规范要求时，才可作为 manifest 的 `failure_evidence`

> iter5 教训：PM 跳过本检查直接判定"AUDITOR 未审计 iteration = 漏洞"，浪费 1 轮交互。
> 若非用户驳斥，会浪费 auditor agent 调用 + 引入范畴混乱。

---

## 与通用 AHE skill 的差异总结

| 维度 | 通用 AHE skill | 本 skill（项目补充） |
|------|---------------|--------------------|
| 工作流 2 步骤数 | 6 步 | +3 步（A1/A2/A3） |
| 工作流 3 触发 | scheduled_at 到时 | **每次 commit 前**（B1） |
| `at_risk_regressions` | 开放列表，agent 凭直觉 | 必须含路径回归 + 元数据回归类别 |
| Verify 漏报处理 | agent 自决 | **强制记入 false_predictions**（B2） |
| Evaluate 步骤 | ⏳ 未实现（仅 Audit 结构审计） | **项目级补充**（§C：测试任务集 + 轨迹格式 + Analyze 流程） |
| AHE 规范解读 | 无（默认 agent 知道边界） | **强制检查清单**（§D：D1 触发 / D2 角色归属 / D3 来源标注 / D4 漏洞前置检查） |
| 规范条款引用 | 无约束 | **必须标注来源前缀**（D3：HARNESS.md / AHE skill / AGENTS.md / 项目级 skill / agent.md） |

---

## 关联文档

- 通用 AHE skill：`.opencode/skills/agentic-harness-engineering/SKILL.md`
- 教训 L-07：`experiences.md#L-07`（执行层：predicted_impact 漏报）
- 教训 L-08：`experiences.md#L-08`（认知层：AHE 规范解读错误）
- 文档元数据规范：`docs/文档元数据规范.md`
- 历史失败证据：`manifests/change_2026-07-27-docs-restructure.json` verification.regressions_observed
- iter5 dogfood 轨迹：`experiments/eval-2026-07-27-iter5-dogfood.md`（本次 iteration 自身轨迹，作为首份 Evaluate 证据）

---

## 维护

- 本 skill 由 PM 维护
- 通用 AHE skill 升级时（从上游同步），本 skill 不受影响（项目特定）
- 新发现的 AHE 实践盲区应同时沉淀到 `experiences.md#L-XX` 和本 skill 的对应检查项
