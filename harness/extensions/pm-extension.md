---
id: EXT-pm
project: neoconstruct
phase: meta
last_updated: 2026-07-27
---

# PM 项目特定拓展（neoconstruct）

> 配合 `.opencode/agents/pm.md`（跨工程 base）使用。
> 工作流状态机与子任务标识格式见 `AGENTS.md §1`（不在本文件重复）。

## 项目特定证据要求

- **绝对基线**：性能对比必须 vs Python `construct==2.10.70`（不可用相对基线如旧路径）
- **测量口径**：从用户面 API 测量（maturin develop + timeit），子进程隔离（详见 `.opencode/skills/performance-gate/SKILL.md`）
- **性能数据验证 skill**：收到性能数据时触发 `.opencode/skills/pm-performance-validation/SKILL.md`

## Evaluate 摘录项目特定要求

**规范来源**：`AGENTS.md §1`（融入 AHE §演化循环）

每个子任务 ACCEPTED 时，PM 在过程记录 append 一段 Evaluate 摘录：

```
## Evaluate 摘录（AHE §演化循环）
- task_id: <phase>.<子任务编号>
- tool_calls 关键点: [简述]
- failures 对齐 L-XX: [L-XX 或 "候选 L-XX" 或 "无"]
- outcome: [完成/部分/失败]
```

**无 Evaluate 摘录的 ACCEPTED 状态无效**。这是 AHE Evidence-Driven 原则的实现——产出可被下一轮 Evaluate / AUDITOR 审计证伪的证据。

## 提交规范（项目特定）

PM 是 git 主操作者（DEV 可做 checkpoint 提交，REV/VET/AUDITOR 禁止任何 git 写操作）。

**分支策略**：直接在 `main` 分支开发，不使用特性分支。

**提交时机**：
```
任务 ACCEPTED → PM 执行 git add + git commit
    ↓
版本验收通过 → PM 执行 git tag v<N>.<M>.<P>（semver tag 即发布触发 tag，release.yml 由 v* 触发构建发布；不再使用 -complete 后缀——避免误触发重复发布）
```

DEV 在开发过程中可随时执行 `git add` + `git commit` 作为 checkpoint。

**commit message 格式**（2026-09-03 半正式发布状态起，版本制）：`<type>(v<version>): 任务号 描述`
示例：`fix(v0.1.1): v0.1.1-3 修复 Switch ctx 透传` / `test(v0.1.1): v0.1.1-2 补全 BUG 回归用例` / `docs(v0.1.1): 更新总纲`
（历史 phase 制 commit 格式 `feat(phaseN): X.Y 描述` 对既有提交保持不变）

**tag 格式**：`v<N>.<M>.<P>`（如 `v0.1.1`；阶段完成与发布合并为一个 tag——release.yml 触发条件 `v*`，-complete 后缀会误触发，已废弃；历史 `phase-N-complete` 保持不变）

**禁止事项**（全员）：所有角色禁止 `git push`（本地仓库）；REV/VET/AUDITOR 禁止 `git add` / `git commit` / `git tag` / `git checkout` / `git revert` 等任何写操作；禁止提交 `construct/` 目录下的任何变更（Python 原版仓库有独立 git）。

## opencode 角色分派机制（项目特定）

PM 通过 opencode 的 Task 工具分派任务给子 agent：

```
.opencode/agents/
├── pm.md          ← PM（主 agent，mode: primary）
├── architect.md   ← ARCH（子 agent）
├── developer.md   ← DEV（子 agent）
├── reviewer.md    ← REV（子 agent）— 设计检视
├── vetter.md      ← VET（子 agent）— 代码审查
└── auditor.md     ← AUDITOR（子 agent）— PM 验收管理审计
```

**Task 工具调用**：
```
subagent_type: "architect" | "developer" | "reviewer" | "vetter" | "auditor"
description: "3-5词任务描述"
prompt: "包含子任务信息、必读文件、输出要求的完整指令"
```

**Agent 文件写入规范**（PM 分派时强制要求）：子 agent 写入大文件必须分批（每次 ≤300 行），禁止一次性 Write/Edit 超长内容，禁止在单个 Task prompt 中要求一次性超大输出。

## AGENTS.md 维护标准——内容下沉位置（项目特定）

不满足 AGENTS.md 准入标准（详见 base §AGENTS.md 维护标准）的内容应放在：

- 角色专属操作流程 → `.opencode/agents/<role>.md`（base）+ `harness/extensions/<role>-extension.md`（项目特定）
- 详细规范模板（如 frontmatter / 过程记录格式）→ `docs/`
- 跨阶段教训 → `experiences.md` / `docs/decisions/ADR-*.md`
- AHE 工作流补充 → `.opencode/skills/neoconstruct-ahe-practices/SKILL.md`
- 性能门禁 → `.opencode/skills/performance-gate/SKILL.md`

## 流程恢复（项目特定路径）

每次启动读取（通用流程见 base）：
1. `MEMORY.md`（L0 索引：项目定位 / 阶段索引 / Harness 组件 / 教训索引 / 性能快照）
2. `experiences.md`（L1 教训：L-01~L-08 模式化失败）
3. 当前 `plans/phaseN/总纲.md`（单一事实源）
4. 当前 phase 过程记录

## 可写文件（项目特定）

- `plans/**`（总纲、过程记录、分析报告）
- `docs/**`（设计、决策、审查、分析、规范——除 `harness/extensions/` 由各 agent 维护）
- `.opencode/`（agents base / skills / 全局配置）
- `AGENTS.md`（System Rules 核心，按 base §AGENTS.md 维护标准 维护）
- `experiments/**`
- `MEMORY.md` / `experiences.md`（LTM）

## 构造器清单维护（PM 主维护责任）

**单一事实源**：`docs/constructors-inventory.csv`（主清单）+ `docs/perf-scenarios.csv`（性能场景）

**PM 是清单主维护者**——其他角色按列辅助。维护触发时机：

| 触发时机 | PM 动作 | 涉及 CSV 列 |
|---------|---------|-------------|
| 子任务 ACCEPTED 时 | 同步状态 + 性能数据 + 不达标场景 | inventory: status/impl_phase/impl_module/parse_min_x~perf_data_source/unmet_scenarios；perf-scenarios: 追加测量点行 |
| 用户面 Python 导出变更 | 同步公开导出条目 | inventory: 新增/删除行 |
| 性能数据复测（如环境升级后） | 更新 perf-scenarios 测量点 + inventory min/max 范围 | perf-scenarios: py_ns_per_call/rs_ns_per_call/speedup_x/meets_10x；inventory: parse_min_x~build_max_x |

**ARCH 辅助**：新构造器设计文档落地时（DESIGNING 阶段），ARCH 在 inventory.csv 追加骨架行（status=not_implemented，仅填 category/name/python_class/notes）。详见 `architect-extension.md §新构造器设计同步清单`。

**AUDITOR 审计**：Phase 阶段验收时，AUDITOR 必查"清单与代码一致性"（status 字段 ↔ 实际实现、impl_module ↔ 真实文件存在、perf_data_source ↔ 源文件存在）。详见 `auditor-extension.md §构造器清单一致性审计`。

**维护红线**：
- 数据来源指针必须精确到 `文件:行号`（L-02 对策）
- 性能数据必须从源文件复制原值，不可估算（L-02 对策）
- 不删除历史测量点（如 v4 PyCallable 数据保留作趋势证据，phase 列标版本）
- 未测量字段留空，不填 0/N/A/null
- 编辑 CSV 后必须用工具验证列数合规（每行 13 列 inventory / 17 列 perf-scenarios）
