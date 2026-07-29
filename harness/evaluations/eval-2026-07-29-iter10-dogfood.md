---
id: EVAL-2026-07-29-iter10-dogfood
phase: meta
task: "iter10 dogfood 轨迹（AHE iteration 10 — harness 集中 + testing 分离）"
status: accepted
owners: [PM]
started: 2026-07-29
completed: 2026-07-29
manifest_refs: [ch_046, ch_047, ch_048, ch_049, ch_050, ch_051]
last_updated: 2026-07-29
---

# iter10 dogfood 轨迹

> AHE §演化循环 Evaluate 步骤。本文档记录 iter10 的执行轨迹、决策点、失败分类与 outcome。

## 任务概览

**任务类别**：执行类（AHE iteration，harness 重组 + 目录语义分离）
**任务描述**：harness 文件集中（MEMORY/experiences/manifests/extensions/metadata-convention/evaluations → harness/）+ testing/experiment 分离（experiments/ci → testing/ci）

## 失败分类

| # | 失败描述 | 类别 | 处理 |
|---|---------|------|------|
| F1 | **PM 语义误判（L-11）**：用户指令"加一个 step12：'关闭iter10后，开始Phase5'"被误判为"跳过 iter10"，一度标记 DEFERRED | **PM 决策类（严重）** | 用户及时纠正（"我让你做完iter10之后做Phase5"），PM 立即恢复正确流程 + 沉淀 L-11 教训 |
| F2 | R1/R2 隐藏代码依赖（run_l3_perf.py 硬编码 + pre-commit.template） | R3 类风险 | 启动前扫描识别 + 迁移后修复（run_l3_perf.py 改 Path(__file__).parent 彻底消除） |
| F3 | 批量替换遗漏（run_l3_perf.py 的 Path 拼接 "experiments" / "ci" 不匹配 experiments/ci/ 字符串） | 工具使用 | 迁移后二次扫描发现 + 手工修复 |

## 关键决策点

| DP | 决策内容 | 规范来源 | 备注 |
|----|---------|---------|------|
| DP1 | harness/ 含 MEMORY/experiences/manifests/extensions/metadata-convention/evaluations（不含 AGENTS.md/opencode.json/.opencode/，opencode 框架约束） | 用户指令 + opencode customize-opencode skill 确认 | opencode 固定路径不可移 |
| DP2 | testing/ci/ 承载 CI 基础设施（从 experiments/ci/ 迁入） | 用户指令"用例和 experiment 混用不对" | CI 是正式测试基础设施 |
| DP3 | run_l3_perf.py 改用 Path(__file__).parent 彻底消除硬编码 | R2 对策 | 一劳永逸消除目录迁移脆弱性 |
| DP4 | docs/agents/ 迁空后删除 + 同步所有语义引用 | R4 对策 | 避免"空目录 + stale 指针"双源困惑 |
| DP5 | manifests/ 内部 file_path 字段保留旧路径（历史快照，L-07 教训） | L-07 教训 | manifest 是历史决策记录 |

## L-XX 教训对照（决策前自检）

- **L-07**（predicted_impact 重结构轻交叉引用）：启动前 explore agent 全量扫描识别 165 处引用 + 7 风险点，commit 前全部修复。**§A1 对策第 3 次完整生效**。
- **L-08**（AHE 规范解读层错误）：commit 前需验证 `.opencode/skills/agentic-harness-engineering/` 未被污染（通用 skill 不可修改）
- **L-10**（规范存在 ≠ 实际执行）：iter10 主动启动（用户指令 + PM 识别），验证 §9 enforcement 在 iter9 引入后首次实际触发
- **L-11**（PM 语义误判）：**本 iter 首次出现并沉淀**——用户双引号包裹的"关闭iter10后"被误判为"取消"，实际是"完成"

## Analyze

### 量化指标

- **物理迁移**：7 类资产（MEMORY/experiences/manifests/extensions/metadata-convention/evaluations/experiments-ci）→ harness/ + testing/
- **cross-ref 修复**：~270 处（235 非Harness文件 + 35 裸 MEMORY/experiences + 14 harness 内部互引 + 2 testing/ci 内部 + 4 隐藏代码依赖）
- **L4 C1-C10**：全 PASS
- **权限更新**：5 agent 文件 × 2 行新 glob = 10 处
- **新教训**：L-11（PM 语义误判）
- **新 README**：3 个（harness/ + testing/ + experiments/）

### L-11 沉淀判断

L-11 在 iter10 首次出现（用户纠正后 PM 立即纠正流程），但模式本身是"PM 决策类错误"——影响面大（一度导致 iter10 误标 DEFERRED + Phase 5 误启动）。立即沉淀（不等反复 3 次），对策已在 §L-11 列出。

### 未触发的 manifest predicted_impact

| predicted | 是否触发 | 说明 |
|-----------|---------|------|
| 6 base agent + AGENTS.md §4 启动链断裂 | 未触发 | 原子同步完成 + grep 验证零残留 |
| run_l3_perf.py L3 静默 FAIL | 未触发 | R2 修复（Path(__file__).parent）+ L4 PASS 验证 |
| pre-commit.template hook broken | 未触发 | R1 修复 + 当前 hook 未安装（无即时风险） |
| 通用 AHE skill 被误修改（L-08） | 未触发 | 替换脚本排除 .opencode/skills/agentic-harness-engineering/ |

## Outcome

✅ **iter10 整体成功**——L4 C1-C10 全 PASS，270 处 cross-ref 修复，R1/R2 隐藏代码依赖彻底消除。

✅ **L-07 教训对策 §A1 第 3 次完整生效**——iter9（200 处）/ iter10（165 处）连续两次 commit 前全量扫描修复，无 post-commit surprise。

✅ **L-11 教训首次沉淀**——PM 语义误判的对策（双引号语义 + 歧义动词清单 + 确认优先）。

⚠️ **L1 smoke PowerShell 中文编码问题**（预先存在，与 iter10 无关）——run_smoke.ps1 的中文注释在 PowerShell 5.1 下解析异常。需 Phase 5+ 独立修复（可能改用英文注释或 UTF-8 BOM）。

⚠️ **pre-commit hook 未安装**——当前 `.git/hooks/pre-commit` 仅 sample，无即时风险。但 pre-commit.template 已修复，未来 `install_hook.ps1` 可正确工作。

## 待 dogfood 验证（Phase 5 启动时）

1. agent 启动链是否稳定（base agent → harness/extensions/ 加载）
2. L1 smoke 是否在 Phase 5 环境正常工作
3. testing/ci/ L1-L4 在 Phase 5 子任务中实际触发
4. L-11 对策在后续 PM 决策中实际生效
