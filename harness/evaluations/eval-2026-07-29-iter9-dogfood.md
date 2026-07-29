---
id: EVAL-2026-07-29-iter9-dogfood
phase: meta
task: "iter9 dogfood 轨迹（AHE iteration 9 — 过程记录拆分 + 目录重组）"
status: accepted
owners: [PM]
started: 2026-07-29
completed: 2026-07-29
manifest_refs: [ch_032, ch_033, ch_034, ch_035, ch_036, ch_037, ch_038, ch_039, ch_040, ch_041]
last_updated: 2026-07-29
---

# iter9 dogfood 轨迹

> AHE §演化循环 Evaluate 步骤。本文档记录 iter9（过程记录拆分 + 目录重组）的执行轨迹、决策点、失败分类与 outcome。

## 任务概览

**任务类别**：执行类（AHE iteration，Long-Term Memory 组件重组）
**任务描述**：phase4 过程记录.md（11837 行/755KB）拆分为 19 个 trace 文件 + docs/design/ 子目录化 + cross-phase META 抽离到 plans/meta/

## 工具调用序列

1. read harness/MEMORY.md / 文档元数据规范.md / harness/experiences.md / phase4 索引/总纲
2. skill agentic-harness-engineering（加载 AHE 工作流）
3. glob + grep 扫描文件结构和 cross-ref
4. task explore agent — 全量 cross-ref 扫描（L-07 对策）
5. write harness/manifests/change_2026-07-29-trace-split.json
6. edit .opencode/agents/{developer,vetter,reviewer,auditor}.md（4 个 permissions）
7. edit harness/metadata-convention.md（§5.1 目录结构 + §9 阈值）
8. PowerShell 创建 6 个新目录 + 拆分 19 个 trace 文件
9. Move-Item 10 个 docs/design/ 文件到子目录
10. Move-Item 归档原 过程记录.md → archive/
11. write 3 个索引文件（phase4 + meta/CI + meta/INV）
12. PowerShell 批量替换 137 处 CSV 行号引用 + 50+ 处段落引用 + 53 处设计文档引用 + 16 处预先 broken
13. edit run_l4_consistency.py（_parse_pointer + C5 + C6 支持新格式）
14. edit harness/MEMORY.md + harness/experiences.md（iter9 历史记录 + L-10 沉淀）

## 关键决策点（含规范来源标注）

| DP | 决策内容 | 规范来源 | 备注 |
|----|---------|---------|------|
| DP1 | 仅拆 phase4（不回溯 phase0-3）| 用户选择 + L-07 教训（成本/收益权衡）| 历史 phase 单文件结构稳定，回溯成本高于收益 |
| DP2 | META 类抽到 plans/meta/（与 phase 物理隔离）| HARNESS.md §7 组件正交性 | harness/MEMORY.md 标记 META-CI/INV 为 cross-phase 资产 |
| DP3 | traces/ 内文件名简化（无"过程记录-"前缀）| 路径已含语义 | 4.1-ArrayNode.md vs 过程记录-4.1-ArrayNode.md |
| DP4 | CSV 用文件级引用（无行号无锚点）| R3/R4 PM 决策 | 锚点稳定性差，行号会漂移，文件存在性是最小稳定集 |
| DP5 | archive/ 不加 agent 写权限 | 归档原则 | 归档只读，避免双源真相 |
| DP6 | plans/meta/ 由 PM 维护（ARCH 权限不改）| AGENTS.md §2 | PM 是 LTM 唯一维护者 |
| DP7 | trace 文件内 50 处 docs/design 旧引用批量修复 | L-07 教训（活跃文件必须有正确引用）| 与 archive/ 历史快照形成对比 |
| DP8 | §4.6-E01-INVEST 段归入 traces/4.6-集成测试.md | PM 决策 R2 | E01 是 4.6 期间 O1 回归调查 |
| DP9 | 4.0/4.5/4.6 超 1000 行阈值但合并（frontmatter 加豁免理由）| §9 豁免条款 | 同子任务多版本迭代整体性优先 |
| DP10 | run_l4_consistency.py 同步修改（_parse_pointer + C5/C6）| R3 隐藏代码依赖 | CSV 改格式后 L4 校验逻辑必须同步 |

## 失败分类

| # | 失败描述 | 类别 | 处理 |
|---|---------|------|------|
| F1 | PowerShell here-string 作函数参数语法错误 | 工具使用 | 改用变量先存储 here-string，再传函数 |
| F2 | PowerShell 脚本文件中文路径解析失败（脚本编码问题）| 工具使用 | 改用 bash 工具直接执行 PowerShell 命令（避免脚本文件）|
| F3 | PowerShell `$all += $body` 把数组转字符串 "System.Object[]" | 工具使用 | 改用 ArrayList.AddRange |
| F4 | 第一次拆分用 grep 行号（错误），实际 PowerShell Get-Content 真实行号不一致 | L-07 #3 类（数据口径不一致）| 重新用 PowerShell 真实行号拆分 |
| F5 | CSV 行号区间 `:NNNN-NNNN` 替换不完整，残留 `-NNNN` | 替换规则不完整 | 第二轮 regex 清理残留 |
| F6 | L-10 候选：iter3 §5 规范到 iter9 才首次执行（3 年间 6 个 phase 全部违反）| 规范无 enforcement | §9 加强制阈值 + AUDITOR 第 8 类审计 |
| F7 | L-07 #3：iter9 启动前扫描发现 200 处 broken refs（远超 iter3 的 42 处）| L-07 模式持续 | iter9 启动前 explore agent 主动扫描，所有 broken commit 前修复 |

无核心 failure（无需求错误 / 无架构问题 / 无质量门禁失败）。

## L-XX 教训对照（决策前自检）

- **L-07**（predicted_impact 重结构轻交叉引用）：iter9 启动前主动分派 explore agent 全量扫描，发现 200 处 broken + 6 个风险点（含 R3 隐藏代码依赖），全部 commit 前修复。**§A1 对策首次完整生效**。
- **L-04**（跨阶段模式未沉淀）：META-CI 整体 ACCEPTED 后已沉淀为 ADR-020，iter9 进一步把 cross-phase 资产物理抽离到 plans/meta/，强化 L-04 对策。
- **L-08**（AHE 规范解读层错误）：iter9 严格区分通用 AHE 规范（HARNESS.md 组件正交性）vs 项目自定义（§5.1 目录结构），未混淆。
- **L-10 候选**（规范存在 ≠ 实际执行）：iter9 实际触发——§5 规范自 iter3 已存在但 3 年未执行。本 iter 沉淀为正式 L-10 教训 + §9 enforcement 补强。

## Analyze

### 量化指标

- **拆分覆盖率**：12080 行（19 个 trace）vs 11837 行（原文件）= 102.1%（多出 243 行为 frontmatter）
- **CSV 引用迁移**：137 处 P0（perf 121 + inventory 16）+ 16 处 `-NNNN` 残留二次清理 = 全部 PASS
- **cross-ref 总修复**：~250 处（CSV 137 + 段落 5 + 设计文档 131 + trace 内 50 + python/tests 16）
- **L4 C5/C6**：PASS（run_l4_consistency.py 改造后）
- **权限更新**：4 个 agent 文件 × 2 行新 glob = 8 处
- **新规范条款**：§5.1 目录结构约定 + §9 强制拆分阈值（iter9 引入）
- **新教训**：L-10（规范存在 ≠ 实际执行）+ L-07 事件#3

### L-10 候选沉淀判断

L-10 在 iter9 首次出现（iter3→iter9 3 年间 §5 未执行），但模式本身是"反复出现"性质——任何无 enforcement 的规范都会重演。已正式沉淀到 harness/experiences.md §L-10 + §9 补强 enforcement。**不需等反复 3 次才沉淀**（条件：(1) 模式根因清晰；(2) 对策已实施；(3) 单次后果严重——3 年累积 11837 行 臃肿 + iter9 重组成本）。

### 未触发的 manifest predicted_impact

| predicted | 是否触发 | 说明 |
|-----------|---------|------|
| opencode glob 不支持 `**` 跨多级目录 | 未触发 | PowerShell Test-Path 验证通过；权限规则匹配 |
| AUDITOR 第 8 类审计是否实际生效 | 未触发 | iter9 后无 phase 验收场景，等 Phase 5+ |
| L4 校验逻辑漏改 silent fail | 未触发 | ch_040 配对修复 + 手动跑 C5/C6 验证 PASS |
| 历史 phase 0-3 是否被未来 agent 误拆 | 未触发 | 等 Phase 5+ agent 实际操作 |

## Outcome

✅ **iter9 整体成功**——所有 P0（CSV 数据完整性）/ P1（权限/规范/段落引用）/ P2（设计文档引用）/ R3（隐藏代码依赖）/ R7（预先 broken）全部修复。L4 C5/C6 全 PASS。

✅ **L-07 教训对策 §A1 首次完整生效**——启动前 explore agent 全量扫描 + commit 前批量修复，无 post-commit surprise。L-07 教训第 3 次出现（200 处 broken 远超 iter3 的 42 处），证明"文件重组类 iteration 必须强制 §A1 全量扫描"是对有效对策。

✅ **L-10 教训正式沉淀**——§5.1 目录结构 + §9 强制阈值 + AUDITOR 第 8 类审计三位一体 enforcement。

⚠️ **执行效率改进点**：
- PowerShell 脚本文件中文路径处理不可靠，应优先用 bash 工具直接执行 PowerShell 命令
- 多切片数组传递需用 ArrayList.AddRange 而非 `+=`
- 行号口径在不同工具间（grep vs PowerShell Get-Content）不一致，应固定用 PowerShell 真实行号

## 待 dogfood 验证（写入 manifest verification.scheduled_at）

1. Phase 5 启动时验证：单子任务检索 token 是否真的下降（vs phase0-3 旧结构）
2. §9 强制阈值是否实际阻止 phase5+ 臃肿
3. AUDITOR 第 8 类审计是否在 Phase 5+ 阶段审计中实际触发
4. plans/meta/ 是否被未来 phase 实际引用（cross-phase 抽离的有效性）
5. L4 门禁在新格式下持续工作（CSV 文件级引用稳定性）
6. L-10 教训是否在 iter10+ 真正阻止"规范无 enforcement"重演
