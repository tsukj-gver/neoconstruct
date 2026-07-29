---
id: SCAN-iter10
status: deferred
phase: meta
last_updated: 2026-07-29
---

# AHE Iteration 10 — Cross-Reference 全量扫描报告

> **状态**：DEFERRED（iter10 物理迁移延后，本报告留存作未来重启施工蓝图）
> **扫描时间**：2026-07-29 | **扫描方法**：explore agent + grep + 代码路径常量审查
> **关联 manifest**：`harness/manifests/change_2026-07-29-iter10-deferred.json`

## 1. 总览

| 维度 | 数量 |
|------|------|
| 受影响文件总数（活跃） | ~38 个 |
| 总引用数 | ~165 处 |
| P0（启动入口/数据完整性/隐藏代码依赖） | ~14 文件 / ~70 处 |
| P1（CI hook / 项目级 skill） | ~6 文件 / ~25 处 |
| P2（普通文档/过程记录） | ~18 文件 / ~70 处 |
| 历史快照（不修复） | 2 archive + 9 manifest JSON |

**关键确认**：`harness/` 与 `testing/` 目录尚不存在（干净迁移）；`construct-rs/`（非 src）零引用；`.git/hooks/pre-commit` 未安装（仅 sample）。

**L-08 防护**：`.opencode/skills/agentic-harness-engineering/`（通用 AHE skill）的所有 harness/MEMORY.md/experiences.md/manifests/ 引用**不在扫描修复范围**——它们描述 HARNESS.md §7 通用跨工程标准。

---

## 2. P0-A：启动入口与 agent 加载（不修则 agent 启动即 broken）

### 文件 1：`AGENTS.md`（根目录，opencode 入口）
| 行 | 当前 | 修复 |
|----|------|------|
| 5 | `MEMORY.md` | `harness/MEMORY.md` |
| 66 | `MEMORY.md`（L0 索引） | `harness/MEMORY.md` |
| 67 | `experiences.md`（L1 教训） | `harness/experiences.md` |
| 69 | `harness/extensions/<role>-extension.md` | `harness/extensions/<role>-extension.md` |
| 73 | `harness/metadata-convention.md` | `harness/metadata-convention.md` |

### 文件 2-7：`.opencode/agents/{pm,architect,developer,reviewer,vetter,auditor}.md`（6 个 base agent "启动加载"段）
| 文件 | 行 | 当前 | 修复 |
|------|----|------|------|
| pm.md | 38 | `harness/extensions/pm-extension.md` | `harness/extensions/pm-extension.md` |
| pm.md | 136 | `MEMORY.md`（L0 索引） | `harness/MEMORY.md` |
| architect.md | 25 | `harness/extensions/architect-extension.md` | `harness/extensions/architect-extension.md` |
| developer.md | 41 | `harness/extensions/developer-extension.md` | `harness/extensions/developer-extension.md` |
| reviewer.md | 41 | `harness/extensions/reviewer-extension.md` | `harness/extensions/reviewer-extension.md` |
| vetter.md | 41 | `harness/extensions/vetter-extension.md` | `harness/extensions/vetter-extension.md` |
| auditor.md | 44 | `harness/extensions/auditor-extension.md` | `harness/extensions/auditor-extension.md` |

---

## 3. P0-B：隐藏代码依赖（R3 类风险，grep 文档扫描会漏）

### 文件 8：`experiments/ci/run_l3_perf.py`（迁到 `testing/ci/run_l3_perf.py`）
> `PROJECT_ROOT = Path(__file__).resolve().parents[2]`（行 60）—— **不需改**（parents[2] 在 experiments/ci/ 和 testing/ci/ 都指向项目根）

| 行 | 当前 | 修复（或改用 `Path(__file__).parent / ...`） |
|----|------|------|
| 62 | `PROJECT_ROOT / "experiments" / "ci" / "known_exemptions.json"` | `PROJECT_ROOT / "testing" / "ci" / "known_exemptions.json"` |
| 63 | `PROJECT_ROOT / "experiments" / "ci" / "reports"` | `PROJECT_ROOT / "testing" / "ci" / "reports"` |
| 769 | `PROJECT_ROOT / "experiments" / "ci" / "ab_test" / "ab_harness.ps1"` | `PROJECT_ROOT / "testing" / "ci" / "ab_test" / "ab_harness.ps1"` |
| 59 注释 | `<root>/experiments/ci/` | `<root>/testing/ci/` |

**反向验证**：`run_l4_consistency.py` 行 43 `PROJECT_ROOT = Path(__file__).resolve().parents[2]` + 行 45-50 指向 docs/ 和 construct-rs/ —— **全部不需改**。

### 文件 9：`experiments/ci/pre-commit.template`（迁到 `testing/ci/pre-commit.template`）
> ⚠️ 这是 pre-commit hook 模板，复制到 .git/hooks/pre-commit 后是静态文本

| 行 | 当前 | 修复 |
|----|------|------|
| 13 注释 | `experiments/ci/install_hook.ps1` | `testing/ci/install_hook.ps1` |
| **32 关键** | `SMOKE_RUNNER="$REPO_ROOT/experiments/ci/run_smoke.ps1"` | `SMOKE_RUNNER="$REPO_ROOT/testing/ci/run_smoke.ps1"` |

**幸运点**：`install_hook.ps1` 本身无硬编码 experiments 路径——用 `$PSScriptRoot` + `Join-Path $scriptDir "..\.."` 动态解析（行 54、63），迁移后仍正确（仍 2 级深度）。

---

## 4. P0-C：被迁移文件自指/内部互引

### 文件 10：`MEMORY.md` → `harness/MEMORY.md`（~12 处内部引用）
- 行 19/34/40/41/42/108/159：`experiences.md` → `harness/experiences.md`
- 行 62：`harness/metadata-convention.md §5.1/§9` → `harness/metadata-convention.md`
- 行 79：`MEMORY.md（本文件）+ harness/experiences.md` → `harness/MEMORY.md（本文件）+ harness/experiences.md`
- 行 87-95 AHE 历史表：`harness/manifests/change_*.json` × 9 → `harness/harness/manifests/change_*.json`

### 文件 11：`experiences.md` → `harness/experiences.md`（5 处）
- 行 242/251：`MEMORY.md` → `harness/MEMORY.md`
- 行 308/310/316：`harness/metadata-convention.md` → `harness/metadata-convention.md`

### 文件 12-17：`harness/extensions/*.md` → `harness/extensions/*.md`（6 个 extension 文件）
各 extension 内部引用需同步：
- `harness/extensions/<role>-extension.md` → `harness/extensions/<role>-extension.md`
- `experiences.md` → `harness/experiences.md`
- `MEMORY.md` → `harness/MEMORY.md`
- `harness/metadata-convention.md` → `harness/metadata-convention.md`
- `harness/extensions/` → `harness/extensions/`

### 文件 18：`harness/metadata-convention.md` → `harness/metadata-convention.md`
- 行 12 自指更新

---

## 5. P1：CI 基础设施文档与项目级 skill

### 文件 19：`experiments/ci/README.md` → `testing/ci/README.md`
- 约 12 处 `experiments/ci/` → `testing/ci/`
- 行 399：`experiences.md §L-09` → `harness/experiences.md §L-09`
- 行 3 相对链接 `../../docs/design/...` —— **不需改**（testing/ci/ 同为 2 级深度）

### 文件 20：`docs/design/基础设施/CI冒烟门禁设计.md`
- 约 20 处 `experiments/ci/` → `testing/ci/`（行 174/197/199/221/254/299/345/631-661/757/804/823/844）
- 行 51/343：`harness/extensions/auditor-extension.md` → `harness/extensions/auditor-extension.md`

### 文件 21：`docs/decisions/ADR-020-CI冒烟门禁方法学.md`
- 行 53/263/316：`experiments/ci/` → `testing/ci/`
- 行 210/313/314：`harness/extensions/auditor-extension.md` → `harness/extensions/auditor-extension.md`

### 文件 22：`.opencode/skills/construct-rs-ahe-practices/SKILL.md`
- ~12 处：`experiences.md` → `harness/experiences.md`（行 32/96/112/117/129/135/172/204/205/216）
- 行 206：`harness/metadata-convention.md` → `harness/metadata-convention.md`
- 行 208：`harness/evaluations/eval-2026-07-27-iter5-dogfood.md` → `harness/evaluations/eval-2026-07-27-iter5-dogfood.md`

### 文件 23：`.opencode/skills/performance-gate/SKILL.md`
- 行 171/183：`experiences.md §L-09` → `harness/experiences.md §L-09`

### 文件 24：`plans/meta/CI-冒烟门禁/索引.md`
- 行 32/45：`experiments/ci/` → `testing/ci/`

---

## 6. P2：普通文档/过程记录引用

### 文件 25：`plans/00-项目进度.md`
- 行 10（**相对链接**）：`[harness/MEMORY.md](../MEMORY.md)` → `[harness/MEMORY.md](../harness/MEMORY.md)`
- 行 14/22：`MEMORY.md` → `harness/MEMORY.md`

### 文件 26：`plans/phase4-array/索引.md`
- 行 59：`MEMORY.md §关键性能快照` → `harness/MEMORY.md §关键性能快照`

### 文件 27：`experiments/ci/` PowerShell 脚本注释（迁到 testing/ci/）
- lib_smoke.ps1:23、run_l3_perf.ps1:30、run_l2_functional.ps1:49、run_smoke.ps1:11/39、ab_test/ab_harness.ps1:31、run_l4_consistency.py:42、run_l3_perf.py:59
- 注释中 `experiments/ci/` → `testing/ci/`（~8 处注释，功能用 $PSScriptRoot 不受影响）

### 文件 29-34：`harness/evaluations/eval-*.md`（5 个 dogfood 轨迹 → `harness/evaluations/`）
内部引用同步：
- `eval-2026-07-27-iter5-dogfood.md`：7 处 harness/experiences.md
- `eval-2026-07-27-iter6-dogfood.md`：3 处
- `eval-2026-07-27-iter7-dogfood.md`：3 处 文档元数据规范.md
- `eval-2026-07-27-iter8-dogfood.md`：5 处 harness/extensions/ + manifests/
- `eval-2026-07-29-iter9-dogfood.md`：3 处 harness/MEMORY.md + 文档元数据规范.md + manifests/

### 文件 35-40：`plans/meta/CI-冒烟门禁/traces/*.md`（6 个 trace）
含 experiments/ci/ + harness/MEMORY.md + harness/experiences.md + harness/extensions/ 引用

### 文件 41-52：`plans/phase4-array/traces/*.md`（12 个 trace）
含 harness/MEMORY.md + harness/experiences.md + harness/extensions/ + 文档元数据规范.md 引用

### 文件 53：`plans/meta/INV-构造器清单/traces/v1v2-CSV格式.md`
- 行 96：`harness/extensions/{pm,architect,auditor}-extension.md` → `harness/extensions/`

### 文件 54：`docs/reviews/架构审查-重复代码与抽象质量.md`
- 行 847：`harness/metadata-convention.md` → `harness/metadata-convention.md`

---

## 7. 不修复（历史快照，L-07 教训保留）

| 文件 | 理由 |
|------|------|
| `plans/phase4-array/archive/过程记录-archive-20260728.md` | status: archived，~30 处旧引用作时间胶囊 |
| `manifests/*.json`（9 个） | manifest 的 file_path 是历史决策记录，非活跃指针 |

---

## 8. 特殊风险点

| R | 风险 | 对策 |
|---|------|------|
| R1 | pre-commit.template 行 32 硬编码（复制到 .git/hooks/ 后静态） | 迁移 PR 提示"如已安装 hook 需 install_hook.ps1 -Force 重装" |
| R2 | run_l3_perf.py 3 处硬编码（不修则 L3 静默 FAIL） | 改为 `Path(__file__).parent / ...` 一劳永逸 |
| R3 | 6 base agent + AGENTS.md §4 "启动链"必须原子同步 | 单 commit 一次性改完 7 文件 |
| R4 | harness/extensions/ 迁空后需删除空目录 | 同步所有语义引用 |
| R5 | plans/00-项目进度.md 相对链接 ../MEMORY.md 会 broken | 改为 ../harness/MEMORY.md |
| R6 | 通用 AHE skill 是诱饵，不可修改（L-08） | commit 前验证 `git diff .opencode/skills/agentic-harness-engineering/` 为空 |
| R7 | eval-*.md 双向引用（自身迁移 + 被 manifest trace_file 引用） | manifest 保留旧路径（历史快照），SKILL.md 行 208 必须更新 |

---

## 9. 建议执行顺序（重启时参考）

### 阶段 1：启动链不断裂（原子提交）
1. 创建 harness/ + testing/ 目录
2. 同步修改 AGENTS.md §4（5 处）+ 6 个 base agent（7 处）
3. git mv + 内部引用同步：harness/MEMORY.md / harness/experiences.md / harness/extensions/* / 文档元数据规范.md
4. 验证：`grep -r "harness/extensions/" .opencode/agents/ AGENTS.md` 零命中

### 阶段 2：CI 功能不断裂
5. git mv experiments/ci/ → testing/ci/
6. 修代码硬编码（run_l3_perf.py + pre-commit.template）
7. 修 CI 文档（README + 设计 + ADR-020）
8. 验证：run_smoke.ps1 -Level L4 实测 PASS

### 阶段 3：项目级 skill + 项目工件
9. 修 construct-rs-ahe-practices SKILL.md（~12 处）
10. 修 performance-gate SKILL.md（2 处）
11. 修 plans/ 各索引 + traces
12. git mv eval-*.md → harness/evaluations/，同步内部引用
13. git mv manifests/ → harness/manifests/

### 阶段 4：清理
14. 删 experiments/__pycache__/
15. 新建 experiments/README.md
16. 更新 harness/MEMORY.md AHE 历史表

### 阶段 5：验证
17. grep 全工作区残留检查
18. git diff 通用 AHE skill 验证为空
19. L1+L4 实测 PASS
