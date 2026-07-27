---
id: eval-2026-07-27-iter7-dogfood
status: completed
phase: meta
task_id: iter7-dogfood
task_category: 执行类
last_updated: 2026-07-27
---

# Evaluate 轨迹 — iter7 dogfood

> **第 3 份 Evaluate 证据**。任务 = AGENTS.md 瘦身重构（457 行 → 66 行）+ 内容准入标准建立。
> 验证 iter6 §3.1 AHE 融入是否在 iter7 中被主动维护（dogfood 轨迹产出）。

---

## 任务定义

- **task_id**: `iter7-dogfood`
- **task_category**: 执行类
- **任务描述**: 用户评估 AGENTS.md 冗余/越位/污染三个问题后，要求全量重构且 <100 行
- **预期产出**: AGENTS.md <100 行 + 内容迁移接收方就位 + cross-reference 更新 + manifest
- **规范来源**: `HARNESS.md §7 反模式`（规则和实现混杂 / 长期记忆塞进 System Rules / 多组件做同一件事）+ `experiences.md §L-07`（cross-reference migration check）

---

## tool_calls（含重复检测）

| 序号 | 工具 | 目的 | 备注 |
|------|------|------|------|
| 1 | `bash` (python 各 §行数统计) | 体量测量 | 必要 |
| 2 | `grep` (AGENTS.md §N 引用扫描) | cross-reference 影响图 | 必要 |
| 3 | `read` (4 个 agent 文件 + docs/文档元数据规范.md + AGENTS.md §6/§10) | 接收方结构确认 | 必要 |
| 4 | `edit` (pm.md 接收 §6 git + §12 角色分派 + §13 提交规范) | 内容迁移 | **触发 failure #1** |
| 5 | `edit` (pm.md 修复结构破坏) | 立即修复 | 必要 |
| 6 | `edit` (architect.md / reviewer.md / vetter.md / developer.md) | Python 速查迁移 | 必要 |
| 7 | `write` (AGENTS.md 重写 66 行版本) | 核心 System Rules 重构 | 必要 |
| 8 | `grep` (broken refs 精确扫描) | §A1 cross-reference 检查 | 必要 |
| 9 | `edit` × 6 (agent/MEMORY/experiences/Cargo.toml) | 活跃文件 broken refs 修复 | 必要 |
| 10 | `bash` (python 批量替换 tests/*.py + experiments/*.py + docs/design/*.md) | 批量 broken refs 修复 | 必要（29+7 处） |
| 11 | `bash` (python 最终扫描) | 活跃文件 broken refs 验证 | 必要 |

**重复检测**: 无同操作 ≥2 次。批量替换避免逐文件 edit 的重复。

---

## decision_points（含规范来源标注）

### 决策 1: 四维度分类（D 全局必需 / A 共同 / B 独立 / C 渐进式）

- **依据**: `HARNESS.md §7` 反模式（规则和实现混杂）+ 用户体验"渐进式披露"原则
- **规范来源**: `HARNESS.md §7` + `construct-rs-ahe-practices §D2` 角色归属判定
- **应用**: 16 个 §按维度分类，识别 §3.1/§5/§6/§7/§8/§10/§12/§13 为 B 独立或 C 渐进式

### 决策 2: 接收方选择（architect.md 接收 §10 完整表）

- **依据**: ARCH 是 §10 Python 参考速查的主消费者（设计时定位 Python 源码）
- **规范来源**: `construct-rs-ahe-practices §D2` —— 不凭直觉判断角色归属
- **替代方案考虑**: 创建独立 `docs/python-reference.md`？否决——避免新增文件，architect.md 已是 ARCH 启动入口

### 决策 3: AGENTS.md §5 直接删除（不迁移）

- **依据**: 读 `docs/文档元数据规范.md` 发现已有完整 §5 过程记录模板（L66-89）
- **规范来源**: `experiences.md §L-07`（cross-reference check）—— 发现重复内容应消除单一源
- **决策**: 删 AGENTS.md §5，留 1 句索引指向 docs/文档元数据规范.md

### 决策 4: 历史/归档文件 broken refs 保留

- **依据**: `experiences.md §L-07` 对策——活跃文件 broken refs 必须修复；archive 文件可保留
- **规范来源**: `experiences.md §L-07` + `construct-rs-ahe-practices §B1`
- **应用**: 18 处剩余 broken refs 全部在 docs/archive / dogfood / plans/phase0-4 过程记录（历史快照），保留不修

### 决策 5: 候选 L-09 识别（AHE iteration 修改 System Rules 审查机制）

- **现象**: iter7 修改 AGENTS.md（System Rules 核心），按 §3 工作流应走 DESIGNING→DESIGN_REVIEW，但 AHE iteration 是 meta 层不走 §3 管道
- **历史 evidence**: iter6 决策 4 已识别（manifest process_lesson）
- **决策**: 单独事件不沉淀，iter7 是第 2 次出现——若 iter8+ 再现则升级为 L-09

---

## failures（含模式分类）

### Failure #1: pm.md edit 破坏结构（PM 不做的事 标题被误替换）

- **现象**: edit pm.md §提交规范 时，oldString 含 "## PM 不做的事"，newString 误用 "## 流程恢复" 替换
- **根因**: oldString 跨多个 §（§提交规范 + §PM 不做的事），newString 末尾标题写错
- **识别**: 立即 read pm.md 验证，发现 L193 "## 流程恢复" 标题与内容（"不编写业务代码..."）不匹配
- **修复**: edit 改回 "## PM 不做的事"
- **模式分类**: **候选 L-10**（PM 在大段 edit 时易跨 §边界，标题与内容失配）—— 单次事件，未沉淀
- **lesson 候选**: 未来大段 edit 应使用更小的 oldString（单 §边界），避免跨 §

### Failure #2: PowerShell 中文匹配问题

- **现象**: PowerShell 5.1 输出乱码 / regex 匹配中文失败
- **根因**: Windows console 默认 GBK 编码与 UTF-8 文件交互问题
- **模式分类**: 工具环境问题，非 harness 缺陷
- **对策**: 改用 Python（已在 iter5/iter6 dogfood 记录，已是常规实践）

---

## outcome

**完成**：
- AGENTS.md 重写：457 行 / 23.3 KB → **66 行 / 5.9 KB**（缩减 85.6%，超额完成 <100 行目标）
- 内容迁移：6 个 agent 文件接收 + 5 个 docs/ 文件更新
- cross-reference 修复：38 个文件改动，~50 处 broken refs 修复（活跃文件 0 broken refs）
- 内容准入标准（新 §5）：防止 AGENTS.md 再次膨胀
- iter7 manifest + dogfood 轨迹

**iter6 pending 预测验证**：
- (1) PM 是否按 AGENTS.md §1.1 主动产出 Evaluate 摘录 → 本次 iter7 dogfood 即是 ✓
- (2) AUDITOR 是否按第 6 类审计项检查 → 未触发场景（iter7 不涉及 phase 子任务验收）
- (3) pm.md §指标验收 '无 Evaluate 摘录的 ACCEPTED 状态无效' 条款 → 未触发场景
- (4) auditor.md §范畴边界 是否实际阻止 AUDITOR 越界 → 未触发场景

iter6 应改为 partial（1/4 预测触发验证场景，3 个待 phase 子任务场景）。

**新教训候选**：
- L-09 候选（AHE iteration 修改 System Rules 审查机制缺失）—— iter6+iter7 共 2 次，仍不足 3 次沉淀
- L-10 候选（PM 大段 edit 跨 §边界）—— 单次，未沉淀

---

## Analyze 摘要（跨轨迹模式分析）

**当前轨迹数**: 3（iter5/6/7 dogfood）

**3 份轨迹已识别模式**:
- L-08（AHE 规范解读层错误）—— iter5 触发，iter6/iter7 §D 协议有效拦截
- 候选 L-09（AHE iteration 修改 System Rules 审查）—— iter6+iter7 共 2 次
- 候选 L-10（PM 大段 edit 跨 §边界）—— iter7 单次

**AGENTS.md 膨胀根因总结**:
- iter3-6 持续加法无减法（每次 AHE iteration 都是"+内容"）
- 缺准入标准（iter7 §5 已建立）
- 项目早期 AGENTS.md 是唯一规则文件，所有内容堆进来

**iter7 是首次"减法 iteration"**：证明 AHE 演化循环不仅支持"+内容"，也支持"-内容"。这是 Evidence-Driven 改进的完整闭环。

---

## 关联

- iter7 manifest: `manifests/change_2026-07-27-iter7-agents-slim.md.json`
- iter6 manifest verification 关闭: `manifests/change_2026-07-27-iter6-workflow-integration.json`
- 内容准入标准: `AGENTS.md §5`
- 各 agent 文件: `.opencode/agents/*.md`
