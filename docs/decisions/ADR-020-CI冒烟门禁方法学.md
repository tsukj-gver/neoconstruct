---
id: ADR-020
title: CI 冒烟门禁方法学（4 层门禁 + Controlled A/B Test 性能回归检测）
status: accepted
phase: cross  # 跨阶段模式（与 ADR-016/017/018/019 一致）
decides: "本地 CI 系统 4 层门禁架构 + baseline 不自动更新 + 性能回归判据三档 + Controlled A/B Test 升级路径"
supersedes: []
superseded_by: ""
depends_on: []
last_updated: 2026-07-28
---

# ADR-020: CI 冒烟门禁方法学（4 层门禁 + Controlled A/B Test 性能回归检测）

## Context

### 用户原始指令

> 让 ARCH 设计 CI，冒烟门禁，把以前做的构造器的功能性能验证都做优化并带上，防止改动引入新 BUG。

四项诉求拆解：
1. **"设计 CI"** → 需要一套可运行的本地 CI 系统
2. **"冒烟门禁"** → 每次改动后快速检测回归，而非仅在 phase 验收时把关
3. **"以前做的构造器功能性能验证都带上"** → 复用 `experiments/` 下 phase1-4 的 bench/smoke 脚本与 Controlled A/B Test 工具链，不另起炉灶
4. **"防止改动引入新 BUG"** → 覆盖功能 + 性能 + 一致性三个维度的回归检测

### 项目背景与驱动力

| 驱动力 | 来源 | 对 CI 设计的约束 |
|--------|------|-----------------|
| 本地仓库，禁止 git push | `pm-extension.md §提交规范` | 排除 GitHub Actions / 远程 CI 服务器 |
| Windows PowerShell 5.1 运行环境 | 开发环境 | 脚本必须 PS 5.1 兼容（`;` 分隔 / `; if ($?) {}` 依赖 / comma 数组操作符陷阱） |
| 跨 phase 改动引入隐蔽回归风险 | Phase 4 多次性能调查（E01 O1 / 4.x p_err_overflow） | 需要日常化的回归检测，而非仅在 phase 验收时审 |
| L-03 教训（PM 接受不对等证据） | `harness/experiences.md §L-03` | baseline 不能自动更新，否则回归被"洗白"无法检测 |
| L-09 教训（跨时段性能对比消除法归因失效） | `harness/experiences.md §L-09` | 性能回归判定必须用 Controlled A/B Test，不可依赖跨时段单次对比 |
| 双 CSV 单一事实源 | `docs/constructors-inventory.csv` + `docs/perf-scenarios.csv` | 一致性核查自动化（AUDITOR 第 7 类日常化） |

### 与 META-CI 整体 ACCEPTED 的关系

本 ADR 由 META-CI 整体 ACCEPTED（2026-07-28，4 子任务 1a/1b/1c/1d 全部 VET 通过）触发起草。META-CI 是本 ADR 的**首次工程实现**，本 ADR 是 META-CI 方法学**跨阶段固化**（L-04 对策：新模式验证后必须沉淀为 ADR）。

## Decision

本 ADR 固化 4 个核心决策点（设计 §7.1 已锁定），构成 construct-rs 本地 CI 冒烟门禁方法学。

### 决策点 1：平台选型——PowerShell 脚本统一 runner + Python 子组件

**决策**：采用 PowerShell 脚本统一 runner（编排层）+ Python 子组件（数据层）的混合架构。

**架构**（设计 §2.3）：

```
testing/ci/
├── run_smoke.ps1              # 主入口（PowerShell，调度 L1-L4）
├── lib_smoke.ps1              # 共享函数（venv 解析 / 日志 / 环境采集）
├── run_l1_quality.ps1         # L1 质量门禁（PowerShell：cargo build/clippy/fmt/test/maturin）
├── run_l2_functional.ps1      # L2 功能门禁（PowerShell 编排，调 Python smoke 矩阵）
├── run_l3_perf.ps1            # L3 性能门禁编排（PowerShell，调 run_l3_perf.py）
├── run_l3_perf.py             # L3 性能回归（Python：baseline diff + 报告 + A/B Test 触发）
├── run_l4_consistency.py      # L4 一致性核查（Python：CSV + 指针 + 源码比对）
├── install_hook.ps1           # pre-commit hook 安装（默认不安装）
├── pre-commit.template        # POSIX sh hook 模板（-Level "L1,L4"）
├── ab_test/                   # Controlled A/B Test 升级路径（通用化 E01_O1_ab_*）
├── known_exemptions.json      # 已知豁免清单（21 条目）
└── reports/                   # 报告输出（JSON + Markdown）
```

**分层原则**：
- **PowerShell 负责"编排"**：调用 cargo / maturin / python.exe，管理 venv 路径，控制层次执行顺序
- **Python 负责"数据"**：L2/L3/L4 的场景矩阵、baseline diff、统计分析、报告生成
- **复用而非重写**：L2/L3 内部直接 `subprocess.run` 调用现有 `phase4_bench_*.py`，不复制逻辑

**论证要点**（设计 §2.4）：
1. **零新依赖**：PowerShell 5.1 是 Windows 内置；Python 已是项目核心运行时；不引入 just/cargo-task
2. **兼容现有资产**：`E01_O1_ab_harness.ps1` 的 `Set-State` / `Build-Install` / `Run-Bench` / `Write-Log` 模式直接被 `run_smoke.ps1` 继承
3. **可被 git hook 调用**：pre-commit hook 是 shell 脚本，可一行调用 `powershell -File run_smoke.ps1 -Level "L1,L4"`
4. **分层执行可控**：PowerShell 参数化 `-Level L1/L2/L3/L4/All`，不同触发时机跑不同层次

> **注**：设计文档 §A.1 原写 `-Mode quick/func/perf/full`，实施时改为 `-Level L1/L2/L3/L4`（更直观，与门禁层次命名直接对应）。本 ADR 固化为 `-Level` 命名（META-CI-1a D.2 + OBS-1a 文档修订）。

### 决策点 2：触发时机分层——pre-commit 默认不装 + 4 档手动触发

**决策**：采用层次 × 时机矩阵，pre-commit hook 默认不安装（避免"hook 太慢被绕过"反模式）。

**层次 × 时机矩阵**（设计 §3.1）：

| 触发时机 | L1 质量 | L2 功能 | L3 性能 | L4 一致性 | 预估耗时 | 阻断策略 |
|---------|---------|---------|---------|----------|---------|---------|
| **pre-commit hook（可选，默认不装）** | ✅ | ❌ | ❌ | ✅ | 30s-2min | 阻断 commit |
| **手动 `smoke-quick`（-Level "L1,L4"）** | ✅ | ❌ | ❌ | ✅ | 30s-2min | 报告 + 退出码 |
| **手动 `smoke-func`（-Level "L1,L2,L4"）** | ✅ | ✅ | ❌ | ✅ | 3-8min | 报告 + 退出码 |
| **手动 `smoke-perf`（-Level "L1,L3,L4"）** | ✅ | ❌ | ✅ | ✅ | 10-20min | 报告 + 退出码 |
| **手动 `smoke`（-Level All）** | ✅ | ✅ | ✅ | ✅ | 15-30min | 报告 + 退出码 |
| **phase 验收前（PM 触发）** | ✅ | ✅ | ✅ | ✅ | 15-30min | 报告（PM 结合 Checkpoint 3 判定） |

**触发时机设计理由**：
- **pre-commit hook 仅跑 L1+L4**：L2 需 maturin develop（~30s）+ smoke 矩阵（~3min）；L3 需 bench（~10min）——commit 频率太高，开发者会禁用 hook（"hook 太慢被绕过"反模式）
- **默认不安装**：提供 `install_hook.ps1`，开发者主动选择是否启用（幂等安装 + `-Force` 覆盖保护，详见 README §4.1）
- **不采用 pre-push hook**：本项目禁止 git push（pre-push 永远不会触发，定义会造成"hook 存在但无效"混淆）
- **不采用夜间定时**：本地开发环境非 24/7 运行，需 CI 服务器与"本地仓库"约束冲突

### 决策点 3：baseline 不自动更新（L-03 对策工程化）

**决策**：`docs/perf-scenarios.csv` 是性能 baseline 的**唯一事实源**（PM 主维护，详见 `pm-extension.md §构造器清单维护`）；L3 只读不写；任何 baseline 更新必须 PM 显式确认 + 附 Controlled A/B Test 证据。

**Baseline 读取规则**（设计 §5.2）：
- L3 只读 `docs/perf-scenarios.csv`，不修改
- 按 `constructor + scenario_id + direction` 三元组唯一定位测量点
- 新场景（baseline 中不存在）标记 `NEW`，不触发回归判据，提示 PM 是否纳入 baseline

**Baseline 更新流程（禁止自动更新）**（设计 §5.2 + §5.8）：

```
L3 报告标注"建议更新 baseline 的场景列表"
  ↓
PM 审查报告，确认性能变化是"改进"而非"回归"
  ↓
PM 手动编辑 docs/perf-scenarios.csv（遵守 pm-extension.md §CSV 编辑规范）
  ↓
下次 L3 以新 baseline 为准
```

**禁止自动更新的理由**（L-03 对策）：若 L3 自动把回归后的差值写入 baseline，则回归被"洗白"，无法再检测。L-03 教训核心是"PM 接受不对等证据"——baseline 自动更新等同于 PM 把"回归判定权"让渡给脚本，违反证据类型对照原则。

**baseline 更新触发**（PM 人工）：
- Controlled A/B Test 确认波动 + 跨时段环境漂移显著 → PM 更新 perf-scenarios.csv，附 A/B Test 报告指针到 `notes` 列
- 性能改进（`IMPROVED`）累积 >20% → PM 评估更新 baseline

### 决策点 4：性能回归判据三档——边界/普通/特殊（L-09 对策工程化）

**决策**：性能回归判据按场景类别分三档，特殊场景（baseline ≥10x → new <10x）直接 FAIL（硬约束 #5：禁止局部门禁）。

**场景分类**（设计 §5.6）：

| 类别 | 判定条件 | 来源 |
|------|---------|------|
| **边界场景** | `rs_ns_per_call < 300` OR `baseline_speedup ∈ [9, 11]` | L-09：Rust 侧 <300ns 时消除法归因失效 |
| **普通场景** | 非边界场景 | 默认 |
| **特殊场景** | `baseline_speedup ≥ 10` 且 `new_speedup < 10` | 总纲 §S-PERF 硬约束 #5（禁止局部门禁） |

**判据表**（`delta_x = new_speedup - baseline_speedup`，`delta_pct = delta_x / baseline_speedup × 100`）：

| 类别 | WARN 阈值 | FAIL 阈值（阻断） |
|------|----------|------------------|
| 边界场景 | `abs(delta_x) > 0.5` | `abs(delta_x) > 1.0` |
| 普通场景 | `abs(delta_pct) > 10%` | `abs(delta_pct) > 20%` |
| 特殊场景 | — | `baseline ≥10x → new <10x`（无 WARN，直接 FAIL） |

**双向判定**：
- 性能**下降**（delta < 0）：按上表判定回归
- 性能**提升**（delta > 0）：标 `IMPROVED`，不阻断（但 PM 应考虑更新 baseline）

**特殊豁免**（仅 PM 显式声明）：已知待优化的 <10x 场景（如 Phase 1 B1 类、Phase 4 StopIf B1 类）在 `reports/known_exemptions.json` 列出，L3 跳过其 FAIL 判定仅 WARN。豁免清单由 PM 维护，每次 phase 验收时复审。

### Controlled A/B Test 升级路径（L-09 核心对策工程化）

**触发条件**：L3 主跑出现 FAIL（含特殊场景 FAIL）

**升级流程**（设计 §5.7，对应 `performance-gate SKILL Checkpoint 4`）：

1. **识别回归场景**：从 L3 报告 `results` 中筛选 `verdict=FAIL`
2. **定位质疑改动**：`git log --oneline baseline_commit..HEAD`
3. **构造对照组**：
   - **阳性对照**：与质疑改动相关、baseline 中预期应有效应的场景（验证 toggle 真实性）
   - **阴性对照**：与质疑改动完全无关、工作量足够大的场景（验证环境漂移幅度）
4. **交替测量**：`A(质疑改动 off) → B(质疑改动 on) → A → B → A → B`（3 轮 6 次），每次 `cargo build --release && maturin develop --release` + sleep 30s
5. **统计分析**：复用 `ab_test/ab_stats.py`（welch_t + 描述性统计 + |Δ|/noise ratio）
6. **结论判据**（同 Checkpoint 4）：
   - `abs(delta_x) > 0.5` → 回归确认，进 Step 2 排查
   - `abs(delta_x) < 0.3` → 测量波动，不阻断
   - `0.3-0.5` → 加测 5 轮

**L-09 工程化要点**：
- **禁止消除法归因**：VET 之前 H1 论断"O1 只改 stop_if.rs，E01 不调用 StopIf，cargo 未重编译"被 Controlled A/B Test 证伪（A/B 产生不同 DLL）。"未改/未传播"假设必须实测验证（DLL hash / 汇编对比）
- **强制阴性对照**：无关场景也出现类似摆动 → 环境漂移是主因（i01 阴性对照在 4.6 E01 调查中证明环境漂移 0.29x 幅度）
- **同会话交替**：跨时段单次对比（如 4.7 PM 验收 10.56x vs 4.6 VET 复测 9.51x，17 天跨度）无法分离因果，必须同会话交替测量

## 适用范围

本 ADR 适用于 construct-rs 项目全部 phase 的日常开发与 phase 验收回归检测。

- **L1 质量**：每次改动后可跑（pre-commit 或 smoke-quick）
- **L2 功能**：改完一个构造器后跑（smoke-func）
- **L3 性能**：怀疑性能影响时跑（smoke-perf），phase 验收前必跑
- **L4 一致性**：每次改动后可跑（CSV 格式 + 指针 + 自洽，~10s）
- **Controlled A/B Test**：L3 触发回归时自动升级（手动确认后跑）

**不替代 phase 验收**：phase 验收仍由 PM 按 `performance-gate SKILL Checkpoint 3` 执行；CI 冒烟门禁是"日常防回归"，验收是"阶段把关"，两者证据类型与严格度不同。

**不触碰 §0 核心原则**：CI 是测试基础设施，位于 parse/build 运行时路径之外。设计 §8 §0 原则对照表逐条核对，CI 通过验证而非改变来支持 §0 原则——若 CI 检测到性能回归（如某改动意外引入中间表示层导致加速比下降），CI 会 FAIL 并触发调查，反而强化 §0 原则执行。

## 禁止行为

- ❌ baseline 自动更新（L3 写 perf-scenarios.csv）—— 违反 L-03 对策
- ❌ 用跨时段单次对比替代 Controlled A/B Test —— 违反 L-09 对策
- ❌ 用消除法归因（"X 没改 → 排除 X"）不做 DLL hash 实测验证 —— 违反 L-09 对策
- ❌ L3 跳过阳性/阴性对照直接判定回归 —— 违反 L-09 对策
- ❌ L3 跳过特殊场景 FAIL 判定（baseline ≥10x → new <10x 不阻断）—— 违反硬约束 #5（禁止局部门禁）
- ❌ pre-commit hook 默认安装 + 跑全量 L2/L3 —— "hook 太慢被绕过"反模式

## Consequences

### 4 层门禁覆盖（量化）

| 门禁层 | 覆盖范围 | 数据点 | 来源 |
|--------|---------|--------|------|
| L1 质量 | cargo build/clippy/fmt/test + maturin develop | 5 步全 PASS | `AGENTS.md §3` + `developer.md §自检清单` |
| L2 功能 | 40 构造器 × 用户面 smoke（39 直接 smoke + StopIf 间接覆盖） | 双对照 Python construct 2.10.70 | `docs/constructors-inventory.csv` 40 implemented |
| L3 性能 | baseline diff + 三档判据 | **154 baseline 数据点**（当前 SCENARIO_DEFS 覆盖 5 场景，全场景 Phase 5 follow-up） | `docs/perf-scenarios.csv` |
| L4 一致性 | CSV 格式 + 指针 + 自洽 + 源码比对 | **CSV 一致性 10 项 check（C1-C10）** | `harness/extensions/auditor-extension.md §第 7 类审计项` |

### 当前限制（L3 仅覆盖 5 场景）

L3 当前 `SCENARIO_DEFS` 仅映射 5 个场景（`docs/perf-scenarios.csv` 154 个测量点中，5 个有 bench 脚本映射 + quick 标记）。剩余 149 个测量点在 L3 主跑中标记 `SKIPPED`，不参与回归判定。

**限制来源**：META-CI-1c DEV 实施时，SCENARIO_DEFS 全场景映射工作量较大（需适配 5+ 个 bench 脚本的不同接口），记入 Phase 5 follow-up（TD-META-CI-1）。

**风险说明**：当前 L3 仅保护 5 个场景的性能回归，其余 149 个场景的性能回归依赖 phase 验收时的 PM 独立复测。Phase 5 补全 SCENARIO_DEFS 后，L3 可覆盖全部 154 场景。

### known_exemptions.json（21 条目）

L3 通过 `reports/known_exemptions.json`（PM 主维护）管理已知豁免场景：

| 事项 | 条目数 | 说明 |
|------|--------|------|
| 事项 A（硬约束 #5 仅 Phase 4 起适用） | 18 | Phase 1（7）+ Phase 2（5）+ Phase 2.5（6）共 18 个 <10x 场景不追溯复审 |
| 事项 B（StopIf B1 类候选扩展） | 1 | `Index:E01-4.7-verify:parse` 边界场景（9.51x，B1 类小字段 FFI 稀释） |
| 事项 C（错误路径 D 类剥离） | 2 | `PrefixedArray:p_err-4.x:build`（2.12x）+ `Array:a_err-4.x:parse`（6.93x，4.x 已闭环） |

**合计 21 条目**，全部 `action=skip_fail_verdict`（L3 不阻断 FAIL，仅 WARN），`review_at=Phase 5`（除事项 C 的 a_err-4.x 标 `review_at=4.x ACCEPTED`）。

**维护规则**：scenario_id 命名 `<constructor>:<scenario_id>:<direction>` 三元组（与 perf-scenarios.csv 一致，L4 C10 一致性核查要求）；baseline_speedup 字段从 perf-scenarios.csv 复制（L-02 对策：数据必须从源文件复制，不凭记忆）；PM 主维护，L3 只读。

### Phase 5 follow-up 清单（META-CI 技术债务）

以下 META-CI 遗留技术债务记入 Phase 5（或本次 ADR-020 起草时处理）：

| TD | 内容 | 处理时机 |
|----|------|---------|
| TD-META-CI-1 | SCENARIO_DEFS 全场景映射（当前 5/154） | Phase 5（与 Phase 1/2 bench 补建合并） |
| TD-META-CI-2 | 9 项设计文档修订（D.1/D.2 + F.1-F.4 + OBS-1a/2a/3a） | **本次 ADR-020 起草时统一修正**（已处理，见下方"本次同步修订"） |
| TD-META-CI-3 | 1a OBS-4 fail-fast JSON 占位（`steps[]` 含 skipped 占位） | Phase 5 可观测性改进 |
| TD-META-CI-4 | 1c OBS #5 mock 用 `hash()` 随机性（CI 自检影响） | Phase 5（CI 自检影响） |
| TD-META-CI-5 | OBS-1d-VET-2 Markdown 报告未含 `construct_py_version` | **本次 ADR-020 起草时补**（已处理：`run_l3_perf.py:build_report_markdown` 新增 `- construct_py_version` 行） |
| TD-META-CI-6 | OBS-1d-VET-3 `power_plan` fallback "unknown" | Phase 5（Windows 电源计划自动采集） |

### 本次 ADR-020 起草时同步处理的修订

**TD-META-CI-2（9 项设计文档修订）**——已修订 `docs/design/基础设施/CI冒烟门禁设计.md`：

| 修订 ID | 修订位置 | 修订内容 | 来源 |
|---------|---------|---------|------|
| D.1 | §4.1 | PYO3 env 全局前置（影响 cargo build/clippy/test/maturin 全部步骤） | 1a DEV [设计质疑] |
| D.2 | §A.1 | `-Mode quick/func/perf/full` → `-Level L1/L2/L3/L4` | 1a DEV [设计质疑] |
| F.1 | §6.3 | `run_l2_functional.py` → `run_l2_functional.ps1`（与 L1 风格一致） | 1b DEV [设计偏离] |
| F.2 | §1.1 B 类 | `phase4_smoke_repeat_until.py` + `v4_smoke_test.py` 标 deprecated（v5 ADR-014/ADR-011 已替代） | 1b DEV [设计偏离] |
| F.3 | §1.1 B 类 | `vet_phase3_bits_integer.py` 标 known_broken（Python 3.14 + construct 2.10.70 上游兼容性 bug） | 1b DEV [设计偏离] |
| F.4 | §4.2 | StopIf/Array 无独立 smoke，由 L1 `cargo test --lib` 单元测试间接覆盖（struct_node.rs 6 个 + greedy_range.rs 5 个 = 11 个） | 1b DEV [设计偏离] |
| OBS-1a | §A.1 | `-Level "L1,L4"` 始终加引号（PS 5.1 native comma 是数组操作符，未加引号被解析为数组→空格连接→Resolve-Levels 报错） | 1a VET OBS-1 |
| OBS-2a | §4.4 | C5/C6 当前仅检查指针行号存在，未检查"内容非空"（PM 决策保持现状，加注释说明） | 1a VET OBS-2 |
| OBS-3a | §4.1 | `run_l1_quality.ps1` L87 裸 `Write-Host $tailPreview` 合理例外（多行 stderr 预览，Write-CiLog 设计为单行无法承载） | 1a VET OBS-3 |

**TD-META-CI-5（Markdown 报告 construct_py_version）**——已修订 `testing/ci/run_l3_perf.py:build_report_markdown`（line 656 后新增 `- construct_py_version` 行，与 JSON schema 对齐）。

### 跨阶段模式沉淀（L-04 对策）

本 ADR 沉淀 META-CI 整体方法学，避免后续 phase 重新决策 CI 架构（L-04 对策：新模式验证后必须沉淀为 ADR）。后续 phase 的 CI 改动（如 Phase 5 补全 SCENARIO_DEFS）应引用本 ADR + `docs/design/基础设施/CI冒烟门禁设计.md`，而非重新设计。

## Alternatives Considered

### 替代方案 1：GitHub Actions（远程 CI）

**描述**：业界标准远程 CI，配置 `.github/workflows/*.yml`，PR 触发自动跑。

**为何不采用**：本项目**禁止 git push**（`pm-extension.md §提交规范`），无远程仓库，GitHub Actions 永远不会触发。定义远程 CI 会造成"配置存在但无效"的混淆（与 pre-push hook 同理）。

### 替代方案 2：单一 cargo task（Rust 生态 CI）

**描述**：用 `cargo-task` 或 `xtask` 管理 CI 流程，Rust 原生。

**为何不采用**：
1. 仅适合 Rust 部分（cargo build/clippy/test），无法管 Python bench/smoke 矩阵
2. 需额外安装 `cargo-task`，违反"不引入重依赖"约束（AGENTS.md 精神）
3. CI 需要跨 Rust + Python 双语言编排，cargo task 单语言能力不足

### 替代方案 3：自动更新 baseline

**描述**：L3 每次跑完自动把新测量值写入 `docs/perf-scenarios.csv`，baseline 持续滚动。

**为何不采用**：违反 L-03 对策（PM 接受不对等证据）。若 L3 自动把回归后的差值写入 baseline，则回归被"洗白"，无法再检测。baseline 更新必须人工确认性能变化是"改进"而非"回归"，并附 Controlled A/B Test 证据。

### 替代方案 4：justfile（跨平台任务运行器）

**描述**：用 `just` 管理 CI 任务，跨平台、语法简洁。

**为何不采用（作为主入口）**：需额外安装 `just`；Windows 非默认安装。可作为可选 wrapper（非主入口），但项目延续 `E01_O1_ab_harness.ps1` 的 PowerShell 先例，统一风格避免引入新工具。

### 替代方案 5：把 L-09 对策提取为通用 AHE skill

**描述**：本设计的"4 层门禁 + baseline diff + A/B Test 升级路径"模式具有跨工程通用性，提取为通用 AHE skill。

**为何不采用**：PowerShell runner 是 Windows 特定（跨工程若非 Windows 需重写）；perf-scenarios.csv schema 是项目特定；A/B Test 工具链（`E01_O1_ab_*`）是项目特定。提取为通用 skill 会触发 L-08（AHE 规范解读层错误：通用/项目混淆）。本设计作为 construct-rs 项目特定资产保留在 `docs/design/`。

## Relations

- **关联教训**：
  - `harness/experiences.md §L-09`（跨时段性能对比消除法归因失效）→ 决策点 4（性能回归判据三档）+ Controlled A/B Test 升级路径是 L-09 对策的**工程化**
  - `harness/experiences.md §L-03`（PM 接受不对等证据）→ 决策点 3（baseline 不自动更新）是 L-03 对策的**工程化**
  - `harness/experiences.md §L-04`（跨阶段模式未沉淀）→ 本 ADR 本身是 L-04 对策的执行（META-CI 整体 ACCEPTED 触发沉淀）
  - `harness/experiences.md §L-02`（理论估算替代实证数据）→ known_exemptions.json 21 条目全部精确匹配 CSV（VET 抽样 6 条对照源行号），baseline_speedup 字段从源文件复制
- **关联规范**：
  - `.opencode/skills/performance-gate/SKILL.md Checkpoint 4`（Cross-Time Performance Comparison / 回归判定）→ Controlled A/B Test 升级路径的判据规范来源（>0.5x 回归 / <0.3x 波动 / 0.3-0.5x 加测）
  - `harness/extensions/auditor-extension.md §第 7 类审计项`（构造器清单一致性）→ L4 一致性核查（C1-C10）的规范来源，L4 是其自动化日常版本
  - `harness/extensions/pm-extension.md §构造器清单维护`（PM 主维护责任）→ baseline 维护责任归属，L3 只读不写
- **设计文档**：`docs/design/基础设施/CI冒烟门禁设计.md`（854 行，§0-§8 + 附录 A/B）—— 本 ADR 的完整设计依据，本次 ADR-020 起草时同步修订 9 项滞后（TD-META-CI-2）
- **实现位置**：`testing/ci/`（run_smoke.ps1 / lib_smoke.ps1 / run_l1_quality.ps1 / run_l2_functional.ps1 / run_l3_perf.{ps1,py} / run_l4_consistency.py / install_hook.ps1 / pre-commit.template / ab_test/ / known_exemptions.json / reports/）
- **实测数据**：`plans/meta/CI-冒烟门禁/traces/1a-L1L4门禁.md` / `1b-L2功能门禁.md` / `1c-L3性能回归.md` / `1d-触发与hook.md`（4 子任务 DEV 报告 + VET 审查报告）+ `plans/meta/CI-冒烟门禁/traces/整体-PM阶段验收.md`（整体 ACCEPTED 记录，2026-07-28；iter9 抽离到 plans/meta/）
- **首次验证**：META-CI 整体（1a/1b/1c/1d，2026-07-28 ACCEPTED，4 子任务全部 VET 通过）
- **未来复用**：Phase 5+（SCENARIO_DEFS 全场景映射补全）及未来 Phase 的 CI 改动应引用本 ADR + 设计文档，而非重新设计

### 与 ADR-019 的并列关系

ADR-019 与 ADR-020 同为 Phase 4 验证的跨阶段模式规范（phase: cross），但维度不同：

| 维度 | ADR-019（错误抛出 fast-path） | ADR-020（CI 冒烟门禁方法学） |
|------|------------------------------|------------------------------|
| 优化/固化对象 | Rust → Python PyErr 构造路径（运行时） | 本地 CI 系统（测试基础设施） |
| 核心机制 | 绕过 `__init__` 直接构造异常实例 | 4 层门禁 + baseline diff + Controlled A/B Test |
| 关联教训 | L-09（Controlled A/B Test 验证 O1-O4 生效） | L-03 + L-09 + L-04（多对策工程化） |
| 收益场景 | 所有错误抛出场景（Phase 4.x 验证） | 日常开发 + phase 验收回归检测（META-CI 验证） |
| 是否触碰 §0 | O3 使用 raw CPython C API（§0 第 4 条 pyo3 是核心依赖，不违反） | CI 是测试基础设施，§0 八条均不触发（设计 §8 对照表） |

**协同关系**：ADR-019 的 Controlled A/B Test 验证方法（welch_t + 阳性/阴性对照 + DLL hash 对比）被 ADR-020 的 Controlled A/B Test 升级路径（决策点 4 + §5.7）通用化工程化——ADR-019 是"首次用 Controlled A/B Test 验证性能优化生效"，ADR-020 是"把 Controlled A/B Test 固化为 CI 自动升级路径"。
