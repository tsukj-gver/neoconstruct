---
id: eval-2026-07-27-iter8-dogfood
status: completed
phase: meta
task_id: iter8-dogfood
task_category: 执行类
last_updated: 2026-07-27
---

# Evaluate 轨迹 — iter8 dogfood

> **第 4 份 Evaluate 证据**。任务 = AGENTS.md §5 自反省下沉 + base/extension 分层架构 + 全面粒度调正。
> 经历两个阶段：阶段 1 初版 base/extension 拆分；阶段 2 用户评估后全面调正粒度。

---

## 任务定义

- **task_id**: `iter8-dogfood`
- **task_category**: 执行类
- **任务描述**: 用户提出两点评估（§5 必要性 + agent 臃肿），明确"subagent 跨工程化 + 渐进式披露 via 固定路径拓展文件"。阶段 2 用户进一步评估发现 base/extension 粒度错位 + base 项目特定泄漏，要求全面调正
- **预期产出**: §5 下沉 + base/extension 分层 + 粒度调正（base 详细化通用内容，extension 仅项目特定）
- **规范来源**: `HARNESS.md §7 反模式` + `construct-rs-ahe-practices §D2`（角色归属判定）+ opencode 框架能力调研

---

## tool_calls（含重复检测）

| 序号 | 工具 | 目的 | 备注 |
|------|------|------|------|
| 1 | `bash`/`grep` (agent 行数 + §结构) | 体量评估 + 内容分类 | 必要 |
| 2 | `read` (opencode.json) + `webfetch` (opencode docs/agents) | **opencode 框架能力调研** | 用户明确要求先调研 |
| 3 | `edit` (pm.md 加 §AGENTS.md 维护标准) + `edit` (AGENTS.md §5 简化) | §5 下沉 | 阶段 1 |
| 4 | `write` × 6 extension + `write` × 6 base | 初版 base/extension 拆分 | 阶段 1 |
| 5 | `write` (iter8 dogfood 初版 + manifest 初版) | 阶段 1 记录 | 阶段 1 |
| 6 | **用户暂停 + 评估 base/extension 粒度** | 识别粒度错位 + base 泄漏 | **阶段 2 触发** |
| 7 | `edit` (AGENTS.md §1 加子任务标识) | F 工作流集中 | 阶段 2 |
| 8 | `write` (pm base 重写) + `write` (pm extension 重写) | G PM 通用内容上移 | 阶段 2 |
| 9 | `write` (reviewer/vetter base 重写 + extension 重写) | H base 详细化 | 阶段 2 |
| 10 | `edit` × 5 (developer/architect/vetter/reviewer/pm 项目特定清理) | I 清理 base 泄漏 | 阶段 2 |
| 11 | **用户指出 PM base 仍残留 extension 内容** | 进一步清理 4 处泄漏 | 阶段 2 迭代 |
| 12 | `grep` + `bash` 最终扫描 | 验证 base 跨工程化 | 阶段 2 |

**重复检测**: 阶段 2 是阶段 1 的修正迭代——同主题但更深入。无劳动重复。

---

## decision_points（含规范来源标注）

### 决策 1: opencode 框架能力调研先行

- **依据**: 用户明确要求"先调研 opencode 框架能力"
- **规范来源**: `experiences.md §L-08`（AHE 规范解读层错误——避免凭直觉判断框架能力）
- **发现**: opencode 不支持 agent 继承/include；支持 agent 主动 read → 用"base 末尾约定 + read extension"模拟分层

### 决策 2: 固定路径约定 harness/extensions/<role>-extension.md

- **依据**: 用户明确"固定指定根目录下的某个文件为拓展提示词"
- **候选**: `.opencode/agents/<role>-extension.md`（与 base 同目录，但 opencode 可能误扫描）vs `harness/extensions/`（与项目文档统一管理）
- **决策**: 选 harness/extensions/（避免 opencode 扫描干扰）

### 决策 3: 阶段 2 全面调正粒度（用户触发）

- **依据**: 用户评估发现"base 太抽象 + extension 重复展开 + base 项目特定泄漏"
- **规范来源**: HARNESS.md §7 反模式"多个组件做同一件事"（base 概述 + extension 详细展开同维度是重复）
- **应用**: F (工作流集中) + G (PM 通用上移) + H (reviewer/vetter base 详细化) + I (base 泄漏清理) + J (去重)

### 决策 4: PM base 反复清理（用户二次纠偏）

- **现象**: 阶段 2 完成后用户指出"PM base 仍存在 extension 内容"
- **根因**: 阶段 2 初版清理不彻底——4 处隐蔽泄漏（AGENTS.md §1 引用 × 2 / Python construct 2.10.70 基线 / pm-performance-validation skill 名）
- **修复**: 全部改为通用描述（"项目工作流定义"/"项目验证机制"/"extension"）
- **模式分类**: 候选 L-11（base/extension 分层时项目特定泄漏易反复——单次事件未沉淀）

### 决策 5: permission frontmatter 中的项目特定路径保留

- **现象**: pm.md permission `construct-rs/src/**`、`construct/**` 是项目特定
- **根因**: opencode 框架要求 permission 在 frontmatter（不能放 extension）
- **决策**: 视为"项目占位符"——其他项目复制 base 时需替换。不算内容泄漏（框架约束）

---

## failures

### Failure #1: PM base 残留 extension 内容（用户二次纠偏）

- **现象**: 阶段 2 初版清理后，用户指出 PM base 仍有 4 处项目特定泄漏
- **根因**: 清理时只关注明显项目名词（construct-rs / Python），忽略了"AGENTS.md §1"这类结构性引用（任何项目都有 AGENTS.md 但 §号含义不同）
- **修复**: 全部改为通用描述
- **模式分类**: 候选 L-11（base/extension 分层时项目特定泄漏易反复）—— 单次事件未沉淀

### Failure #2: 阶段 1 base/extension 粒度错位

- **现象**: 阶段 1 把通用内容放 extension，项目特定内容放 base，导致两边都臃肿
- **根因**: 未充分区分"通用详细"vs"项目特定"——把"详细"等同于"项目特定"
- **修复**: 阶段 2 全面调正
- **模式分类**: 候选 L-12（分层粒度判断错误）—— 单次事件未沉淀

---

## outcome

**完成**：
- AGENTS.md §5 准入标准下沉到 pm-extension（关闭自反讽）
- 6 个 base agent 重写为跨工程通用（含详细通用清单）
- 6 个 extension 瘦身为仅项目特定
- AGENTS.md §1 加子任务标识格式统一约定
- AGENTS.md §4 启动入口更新（base + extension 双层）

**最终行数**：
- AGENTS.md: 81 行
- base 合计: 742 行（6 个文件）
- extension 合计: 463 行（6 个文件）
- 总量: 1286 行（vs iter7 前 1391 行）

**每角色启动加载量**（AGENTS + base + ext）：
| 角色 | 行数 |
|------|------|
| PM | 333 |
| ARCHITECT | 292 |
| DEVELOPER | 288 |
| REVIEWER | 248 |
| VETTER | 258 |
| AUDITOR | 272 |

平均 282 行（vs iter7 前 557-657，**减半目标达成**）。

**L-09 候选达沉淀阈值**：iter6+iter7+iter8 三次 AHE iteration 修改 System Rules（AGENTS.md / agent 文件），都涉及"meta 层不走 §3 管道"的矛盾。建议 iter9 沉淀 L-09。

---

## Analyze 摘要（跨轨迹模式分析）

**当前轨迹数**: 4（iter5/6/7/8 dogfood）

**候选教训（未沉淀）**：
- L-09（AHE iteration 修改 System Rules 审查机制缺失）—— iter6+7+8 共 3 次，**达沉淀阈值**
- L-10（PM 大段 edit 跨 §边界）—— iter7 单次
- L-11（base/extension 分层时项目特定泄漏易反复）—— iter8 单次
- L-12（分层粒度判断错误）—— iter8 单次

**关键 insight**: iter8 经历"初版 → 用户评估 → 全面调正 → 用户二次纠偏"两轮迭代。证明 base/extension 分层的复杂性——单次拆分难以避免泄漏，需要 dogfood 验证 + 用户审查。

---

## 关联

- iter8 manifest: `harness/manifests/change_2026-07-27-iter8-base-extension-split.json`
- iter7 manifest verification 关闭: `harness/manifests/change_2026-07-27-iter7-agents-slim.json`
- §AGENTS.md 维护标准: `pm-extension.md §AGENTS.md 维护标准——内容下沉位置`
- opencode 框架能力调研: 本轨迹决策 1
