---
id: DESIGN-CI冒烟门禁
status: active
phase: meta
depends_on: []
last_updated: 2026-07-28
---

# CI 冒烟门禁设计（META-CI-1）

> 子任务标识：`META-CI-1 [CI 冒烟门禁设计]`。本设计覆盖**功能 + 性能 + 一致性**三个维度的本地回归检测，
> 不依赖远程 CI 服务器，兼容项目"禁止 git push"的本地开发流程。

---

## §0 文档定位 + 设计目标

### 0.1 一句话定位

为 construct-rs 项目建立一套**本地可运行的冒烟门禁系统**，在每次代码改动后自动检测
**功能回归**（构造器行为偏离 Python construct）、**性能回归**（加速比下降）和**一致性漂移**
（CSV 单一事实源与代码脱节），防止跨 phase 改动引入隐蔽 BUG。

### 0.2 设计目标

| # | 目标 | 衡量标准 |
|---|------|---------|
| G1 | **防功能回归** | inventory.csv 中所有 `status=implemented` 的构造器（当前 40 个）每次改动后行为仍与 Python construct 一致 |
| G2 | **防性能回归** | perf-scenarios.csv 中所有测量点（当前 151 个）加速比不出现非预期下降；任何场景从 ≥10x 跌破 10x 必须阻断 |
| G3 | **防一致性漂移** | 双 CSV（inventory + perf-scenarios）格式合规、指针可达、字段自洽、与源码（`nodes/` + `_descriptors.py`）无遗漏 |
| G4 | **本地可运行** | 无远程仓库依赖；Windows PowerShell 5.1 环境下一条命令触发；执行时间分层可控 |
| G5 | **复用现有资产** | 直接复用 `experiments/` 下 phase1-4 的 bench/smoke 脚本与 E01_O1_ab Controlled A/B Test 工具链 |

### 0.3 非目标（明确边界）

- **不是持续集成服务器**：不提供夜间定时、不提供 Web Dashboard、不提供 PR 触发
- **不是发布流水线**：不涉及版本号 bump / changelog / publish / 打 tag
- **不替代 phase 验收**：phase 验收仍由 PM 按 `performance-gate SKILL Checkpoint 3` 执行；CI 冒烟门禁是"日常防回归"，验收是"阶段把关"，两者证据类型与严格度不同
- **不替代 Controlled A/B Test**：L3 性能门禁默认做"同会话单次对比 + baseline 对比"；仅当触发回归阈值时才自动升级为 Controlled A/B Test（L-09 对策）
- **不自动更新 baseline**：perf-scenarios.csv 的更新必须 PM 显式确认（防止回归被"洗白"）

### 0.4 规范来源（强制对照）

| 规范 | 来源 | 本设计的对应章节 |
|------|------|----------------|
| 质量门禁定义 | `AGENTS.md §3 全员红线` + `developer.md §自检清单` | §4 L1 |
| 性能回归方法学 | `harness/experiences.md §L-09` + `performance-gate/SKILL.md Checkpoint 4` | §4 L3 + §5 |
| 测量口径（子进程隔离 / min(repeat=5) × number / apples-to-apples） | `performance-gate/SKILL.md` + `plans/phase4-array/总纲.md §S-PERF` | §5.3 |
| 性能 baseline 单一事实源 | `docs/perf-scenarios.csv`（PM 主维护） | §5.2 |
| 构造器清单单一事实源 | `docs/constructors-inventory.csv` | §4 L2 + L4 |
| 一致性核查清单 | `harness/extensions/auditor-extension.md §第 7 类审计项` | §4 L4 |
| 现有 Controlled A/B Test 工具 | `experiments/E01_O1_ab_*`（bench + harness + stats） | §5.5 |
| 现有 bench/smoke 脚本 | `experiments/phase*_bench_*.py` + `phase*_smoke_*.py` | §1 + §4 L2/L3 |

---

## §1 现状分析（experiments/ 已有脚本调研）

### 1.1 脚本分类清单

调研 `experiments/` 目录（排除 `bench_phase33/target/` 与 `bench_bits/target/` 构建产物），按用途分 6 类：

#### A. 性能 bench 脚本（Python，子进程隔离，S-PERF 口径）

> **L3 调用策略（按 META-CI-1-REV OBS-1 修订）**：L3 仅调用每个 bench 的**最新版本**（v5 > v4 > v3 > v2 > 无后缀），旧版本仅作历史证据保留。DEV 实施 L3 时通过硬编码最新版文件名或目录扫描 + 版本号排序取最新。

| 脚本 | 覆盖构造器 | 测量点 | 复用价值 |
|------|-----------|--------|---------|
| `phase4_bench_array_v4.py` | Array | 15 场景 × parse/build = 30 点 | ★★★ 直接复用（apples-to-apples 方法论修正版） |
| `phase4_bench_greedy_range_v4.py` | GreedyRange | ~10 场景 × 2 = ~20 点 | ★★★ 直接复用 |
| `phase4_bench_prefixed_array_v3.py` | PrefixedArray | ~8 场景 × 2 = ~16 点 | ★★★ 直接复用 |
| `phase4_bench_index_stopif_v3.py` | Index + StopIf | ~12 场景 × 2 = ~24 点 | ★★★ 直接复用 |
| `phase4_bench_repeat_until_v5.py` | RepeatUntil | ~12 场景 × 2 = ~24 点 | ★★★ 直接复用（v5 重写版） |
| `phase4_47_perf_verify.py` | Array 系列（4.7 优化后） | 8 场景 × 1 方向 | ★★★ 直接复用 |
| `phase4_bench_summary.py` | 汇总器 | — | ★★ 可改造为 L3 报告生成器 |
| `bench_phase33_python.py` / `bench_bits_python.py` | Bitwise / BitStruct | 综合 2 点 | ★★ 需统一为 S-PERF 口径（当前口径未严格 apples-to-apples） |

**缺口**：Phase 1（FormatField B1-B7）+ Phase 2/2.5（Bytes/Computed E1-E3）无独立 bench 脚本，数据散落在 `plans/phase1-foundation/过程记录.md` 与 `plans/00-项目进度.md`。**需新建 `phase1_phase2_bench.py`** 补齐。

#### B. 功能 smoke test 脚本（Python，行为对比）

| 脚本 | 覆盖构造器 | 复用价值 |
|------|-----------|---------|
| `phase4_smoke_greedy_range.py` | GreedyRange | ★★★ 直接复用 |
| `phase4_smoke_prefixed_array.py` | PrefixedArray | ★★★ 直接复用 |
| `phase4_smoke_repeat_until.py` | RepeatUntil | ⚠️ **deprecated in v5（F.2 修订）**：v5 ADR-014 删除 PyCallable 路径后此脚本必然失败（CompilationError），保留作历史证据；L2 用 `phase4_repeat_until_examples.py`（v5 重写版）替代 |
| `phase4_repeat_until_examples.py`（**按 META-CI-1-REV OBS-1 补入**） | RepeatUntil v5 用户面 21 样例 | ★★★ 直接复用（4.5 v5 VET 审查报告 D2 节"用户面 20/20 PASS"的关键脚本，L2 必须调度） |
| `v4_smoke_test.py` | RepeatUntil V-1~V-5 修正验证 | ⚠️ **deprecated in v5（F.2 修订）**：v5 ADR-011 删除 Container lib 路径后此脚本必然失败（ModuleNotFoundError），保留作历史证据；L2 不调度 |
| `vet_phase3_binary.py` / `vet_phase32_bitstruct.py` / `vet_phase32_deep.py` | Bitwise / BitStruct | ★★★ 直接复用 |
| `vet_phase3_bits_integer.py` | BitsInteger | ⚠️ **known_broken（F.3 修订）**：`Bitwise(Bit).parse(b"\x80")` 在 Python 3.14 + construct 2.10.70 触发 RestreamedBytesIO 上游兼容性 bug，与 construct-rs 无关；L2 不调度（README §7.3） |
| `test_phase33.py`（**按 META-CI-1-REV OBS-1 补入**） | Phase 3.3 BitStruct 集成测试（test_bitstruct_with_padding 等） | ★★★ 直接复用（与 vet_phase32_bitstruct.py 同档，L2 必须调度） |
| `vet_4_5_ctx_proxy_test.py` | RepeatUntil Container proxy | ★★ 专题验证 |
| `vet_e01_repro.py` | E01 边界场景复现 | ★★ 回归专用 |

**缺口**：Phase 1（FormatField / Bytes / Struct）+ Phase 2（Tell / Computed）无独立 smoke 脚本，散落在 `construct-rs/tests/` 单元测试中。L2 应**调用 `cargo test --lib`（L1 已含）+ 补建 phase1_phase2 用户面 smoke**。

#### C. Controlled A/B Test 工具（性能回归调查专用，L-09 对策）

| 脚本 | 用途 | 复用价值 |
|------|------|---------|
| `E01_O1_ab_bench.py` | A/B 测量脚本（含阳性/阴性对照） | ★★★ L3 升级路径直接复用 |
| `E01_O1_ab_harness.ps1` | 交替测量 harness（A→B→A→B→A→B + sleep 30s） | ★★★ L3 升级路径直接复用 |
| `E01_O1_ab_stats.py` | 统计分析（welch_t + 描述性统计） | ★★★ L3 升级路径直接复用 |

#### D. 性能调查脚本（一次性，非门禁）

| 脚本 | 用途 | 复用价值 |
|------|------|---------|
| `phase4_perf_investigation.py` + `phase4_investigation_data/` | 4.x-INVEST 调查 | ★ 历史证据，不纳入门禁 |
| `phase4_overhead_breakdown.py` + `phase4_overhead_breakdown_py.py` | 开销分解 | ★ 历史证据 |
| `phase4_47_path_verify.rs` | Rust 路径验证 | ★ 历史证据 |

#### E. Rust 独立 bench（不依赖 Python，非 S-PERF 口径）

| 脚本 | 用途 | 复用价值 |
|------|------|---------|
| `bench_phase33/`（Cargo 项目） | Phase 3 BitStruct Rust 元操作 | ★ 非用户面 API，不纳入 S-PERF 门禁，可作辅助证据 |
| `bench_bits/`（Cargo 项目） | Bit 操作元测量 | ★ 同上 |
| `bench_asymmetry/`（Cargo 项目，**按 META-CI-1-REV OBS-1 补入**） | Phase 3 parse/build 不对称根因分析实验（src/main.rs 222 行） | ★ 同 E 类决策"不纳入门禁"——Rust 独立 bench 非 S-PERF 口径，仅作 ARCH 性能分析辅助工具 |

> **决策**：L3 性能门禁**不纳入 Rust 独立 bench**（含 `bench_asymmetry/`），因为 S-PERF 口径要求"用户面 API 端到端测量"（`performance-gate SKILL Checkpoint 2`）。Rust 独立 bench 仅作 ARCH 性能分析的辅助工具。

#### F. AHE eval 产物（非门禁）

| 文件 | 用途 |
|------|------|
| `eval-2026-07-27-iter*-dogfood.md`（4 个） | AHE iteration dogfood 验证 |

这些是 AHE 流程产物，不属于 CI 门禁范畴。

### 1.2 现状问题总结

| # | 问题 | 影响 | 本设计对策 |
|---|------|------|-----------|
| P1 | 无统一入口 | 开发者需记住 10+ 脚本路径与参数 | §3 提供 `Invoke-Smoke` 单一入口 + 层次化子命令 |
| P2 | 无 baseline 对比 | 每次跑 bench 只得到绝对值，无法判断"是否回归" | §5 以 perf-scenarios.csv 为 baseline，自动 diff |
| P3 | 无测量环境标注 | 跨时段对比失效（L-09 根因） | §5.4 强制环境标注，无环境标注的报告无效 |
| P4 | 无回归判据量化 | 人工判断"差 0.5x 算不算回归" | §5.6 按 L-09 + Checkpoint 4 量化判据（边界/普通/特殊三档） |
| P5 | 无阳性/阴性对照 | 单次对比无法排除环境漂移 | §5.7 触发回归阈值时自动升级为 Controlled A/B Test |
| P6 | Phase 1/2 缺独立 bench/smoke | 早期构造器改动无门禁保护 | §6 子任务 META-CI-1b 含补建 |
| P7 | CSV 一致性靠人工审计 | AUDITOR 仅在 phase 验收时审，日常改动无保护 | §4 L4 自动化 AUDITOR 第 7 类 |

---

## §2 CI 平台选型 + 论证

### 2.1 约束条件

| 约束 | 来源 | 影响 |
|------|------|------|
| 本地仓库，禁止 git push | `pm-extension.md §提交规范` | 排除 GitHub Actions / 远程 CI |
| Windows PowerShell 5.1 | 运行环境 | 脚本必须 PowerShell 兼容；`&&` 不可用，需用 `; if ($?) {}` |
| 现有脚本风格 | `E01_O1_ab_harness.ps1` | 已有 PowerShell harness 先例，延续一致风格 |
| 双 venv 隔离 | `phase4_47_perf_verify.py` | `crs_venv_new`（construct-rs）+ `crs_venv_py_new`（Python construct） |
| maturin develop 安装 | `Cargo.toml` + `pyproject.toml` | L1/L3 前必须 `maturin develop --release` 装到 crs_venv_new |
| 不引入重依赖 | AGENTS.md 精神 | 避免 just/cargo-task 等需额外安装的工具 |

### 2.2 候选方案评估

| 方案 | 优点 | 缺点 | 结论 |
|------|------|------|------|
| **GitHub Actions** | 业界标准 | 需远程仓库；本项目禁 push | ❌ 排除 |
| **justfile（just）** | 跨平台、语法简洁 | 需额外安装 just；Windows 非默认 | ⚠️ 可选 wrapper，非主入口 |
| **Makefile** | 经典 | Windows 无原生 make；PowerShell 不友好 | ❌ 排除 |
| **cargo task（cargo-task）** | Rust 生态 | 需额外安装；仅适合 Rust 部分，无法管 Python bench | ❌ 排除 |
| **PowerShell 脚本** | 零新依赖 / Windows 原生 / 兼容现有 harness / 可被 git hook 调用 | 非 Rust 开发者略陌生 | ✅ **首选** |
| **纯 Python 脚本** | 跨平台 | 无法直接调 cargo/maturin（需 subprocess 包装）；与现有 harness 风格不一致 | ⚠️ 作为子组件（L2/L3 内部用 Python） |

### 2.3 决策：PowerShell 脚本统一 runner + Python 子组件

**架构**：

```
testing/ci/
├── run_smoke.ps1              # 主入口（PowerShell，调度 L1-L4）
├── lib_smoke.ps1              # 共享函数（venv 解析 / 日志 / 环境采集）
├── run_l1_quality.ps1         # L1 质量门禁（cargo build/clippy/fmt/test + maturin develop）
├── run_l2_functional.ps1      # L2 功能门禁（PowerShell 编排，调 Python smoke 脚本矩阵）
├── run_l3_perf.py             # L3 性能回归（Python，跑 bench + baseline diff + 报告）
├── run_l4_consistency.py      # L4 一致性核查（Python，CSV + 指针 + 源码比对）
├── ab_test/                   # Controlled A/B Test 升级路径（复用 E01_O1_ab_*）
│   ├── ab_harness.ps1         # 通用化 E01_O1_ab_harness.ps1（参数化 toggle 文件）
│   ├── ab_bench_template.py   # 通用化 E01_O1_ab_bench.py（参数化场景 + 对照组）
│   └── ab_stats.py            # 复用 E01_O1_ab_stats.py
└── reports/                   # 报告输出目录（JSON + Markdown，.gitignore）
```

**分层原则**：
- **PowerShell 负责"编排"**：调用 cargo / maturin / python.exe，管理 venv 路径，控制层次执行顺序
- **Python 负责"数据"**：L2/L3/L4 的场景矩阵、baseline diff、统计分析、报告生成
- **复用而非重写**：L2/L3 内部直接 `subprocess.run` 调用现有 `phase4_bench_*.py`，不复制逻辑

### 2.4 论证要点

1. **零新依赖**：PowerShell 5.1 是 Windows 内置；Python 已是项目核心运行时；不引入 just/cargo-task
2. **兼容现有资产**：`E01_O1_ab_harness.ps1` 的 `Set-State` / `Build-Install` / `Run-Bench` / `Write-Log` 模式直接被 `run_smoke.ps1` 继承
3. **可被 git hook 调用**：`pre-commit` / `pre-push` hook 是 shell 脚本，可一行调用 `powershell -File testing/ci/run_smoke.ps1 -Level "L1,L4"`（D.2 修订命名）
4. **分层执行可控**：PowerShell 参数化 `-Level L1/L2/L3/L4/All`（D.2 修订命名），不同触发时机跑不同层次
5. **报告可观测**：所有层次输出统一到 `testing/ci/reports/` 下，JSON（机器可读）+ Markdown（人工审查）

---

## §3 触发时机设计

### 3.1 层次 × 时机矩阵

| 触发时机 | L1 质量 | L2 功能 | L3 性能 | L4 一致性 | 预估耗时 | 阻断策略 |
|---------|---------|---------|---------|----------|---------|---------|
| **pre-commit hook（可选）** | ✅ | ❌ | ❌ | ✅ | 30s-2min | 阻断 commit |
| **手动 `smoke-quick`** | ✅ | ❌ | ❌ | ✅ | 30s-2min | 报告 + 退出码 |
| **手动 `smoke-func`** | ✅ | ✅ | ❌ | ✅ | 3-8min | 报告 + 退出码 |
| **手动 `smoke-perf`** | ✅ | ❌ | ✅ | ✅ | 10-20min | 报告 + 退出码 |
| **手动 `smoke`（全量）** | ✅ | ✅ | ✅ | ✅ | 15-30min | 报告 + 退出码 |
| **phase 验收前（PM 触发）** | ✅ | ✅ | ✅ | ✅ | 15-30min | 报告（PM 结合 Checkpoint 3 判定） |

### 3.2 触发时机设计理由

#### pre-commit hook（可选，默认不安装）
- **跑 L1+L4**：质量门禁（编译/clippy/fmt/test）+ 一致性核查（CSV 格式 + 指针）
- **不跑 L2/L3**：L2 需 maturin develop（~30s）+ smoke 矩阵（~3min）；L3 需 bench（~10min）——commit 频率太高，开发者会禁用 hook
- **默认不安装**：提供 `testing/ci/install_hook.ps1`，开发者主动选择是否启用（避免"hook 太慢被绕过"反模式）
- **阻断策略**：L1/L4 失败 → 阻断 commit（exit 1）；开发者可用 `git commit --no-verify` 跳过（仅紧急情况，会在 commit message 留痕）

#### 手动触发（主推）
- **`smoke-quick`**：开发中频繁跑（每改一个小改动）——L1+L4，~30s-2min
- **`smoke-func`**：改完一个构造器跑——L1+L2+L4，~3-8min
- **`smoke-perf`**：怀疑性能影响时跑——L1+L3+L4，~10-20min
- **`smoke`**：phase 验收前 / 大改动后跑——全量，~15-30min

#### phase 验收（PM 触发）
- PM 在 ACCEPTED 前跑全量 `smoke`，结合 `performance-gate SKILL Checkpoint 3` 判定证据类型
- **CI 冒烟门禁不替代 Checkpoint 3**：Checkpoint 3 要求 PM 独立核查证据类型与数值；CI 只提供数据，PM 仍需做"证据类型对照"

### 3.3 不采用 pre-push hook 的理由

- 本项目**禁止 git push**（`pm-extension.md §提交规范`），pre-push hook 永远不会触发
- 定义 pre-push hook 会造成"hook 存在但无效"的混淆
- 若未来启用远程仓库，可补 pre-push hook 跑全量 `smoke`

### 3.4 不采用夜间定时的理由

- 本地开发环境非 24/7 运行
- 夜间定时需 CI 服务器（如 Jenkins），与"本地仓库"约束冲突
- 性能 baseline 漂移检测已由 L3 的"同会话单次对比 + baseline diff"覆盖

---

## §4 门禁层次设计（L1-L4 详细规范）

### 4.1 L1 质量门禁

**规范来源**：`AGENTS.md §3 全员红线` + `developer.md §自检清单` + `Cargo.toml [profile.release]`

**执行脚本**：`testing/ci/run_l1_quality.ps1`

> **全局环境前置（D.1 修订，META-CI-1a DEV [设计质疑]）**：以下命令序列全部步骤（1-5）必须在设置 `PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1` 后执行。Python 3.14 + pyo3 0.22.6 下，step 1 的 `cargo build --release` 就会触发 abi3 forward compat 检查，若仅在 step 5 设置，step 1-4 全部 FAIL。1a VET §4 D.1 评估已实测确认。

**命令序列**（顺序执行，任一失败则 L1 FAIL）：

```powershell
# 全局前置：PYO3 abi3 forward compat（影响 step 1-5 全部，D.1 修订）
$env:PYO3_USE_ABI3_FORWARD_COMPATIBILITY = "1"
# 1. cargo build --release（验证编译）
cargo build --release
# 2. cargo clippy --all-targets（零 warning，-D warnings 升级为 error）
cargo clippy --all-targets -- -D warnings
# 3. cargo fmt --check（格式）
cargo fmt --check
# 4. cargo test --lib（单元测试）
cargo test --lib
# 5. maturin develop --release（安装到 crs_venv_new，供 L2/L3 使用）
maturin develop --release
```

**通过判据**：
- 步骤 1-5 全部 exit 0
- 步骤 2 输出无任何 warning（`-D warnings` 保证）
- 步骤 4 全部 test PASS

**失败处理**：
- 步骤 1/2/3/4 失败 → 报告具体命令 + stderr 摘要，阻断后续 L2/L3
- 步骤 5 失败 → 报告 maturin 错误（常见：venv 路径错 / pyo3 abi3 不兼容）

**环境前置条件**：
- 工作目录：`construct-rs/`（Cargo.toml 所在）
- 环境变量：`VIRTUAL_ENV=<opencode-temp>\crs_venv_new`（maturin 识别）
- 工具链：cargo + maturin 在 PATH

**耗时**：~30s-2min（取决于改动是否触发重编译；fat LTO + codegen-units=1 使 release build 较慢）

> **实现注记（OBS-3a 修订，META-CI-1a VET OBS-3 + 1b VET OBS-VET-4）**：`run_l1_quality.ps1` L87 存在一处裸 `Write-Host $tailPreview`（输出尾部 5 行 stderr 预览），未封装在 `Write-CiLog` 内。这是**合理例外**——`Write-CiLog` 设计为单行日志，无法承载多行 stderr 预览。`run_l2_functional.ps1` L224 同模式（1b VET OBS-VET-4 合并处理）。设计允许此类"多行预览"场景使用裸 `Write-Host`，其他场景必须封装在 `Write-CiLog`。

### 4.2 L2 功能验证

**覆盖范围**：`docs/constructors-inventory.csv` 中所有 `status=implemented` 的构造器（当前 ~40 个）

> **partial 构造器处理（按 META-CI-1-REV OBS-2 修订）**：`status=partial` 的构造器（Container / ListContainer）**不在 L2 单独 smoke 范围**——这些是内部使用的辅助类型（按 ADR-011 ListContainer 返回原生 `list`，无独立 parse/build 路径），其行为覆盖在 RepeatUntil / Struct 等组合构造器的 smoke 测试中。本规则与 §4.4 L4 的 partial 豁免规则对齐。

**执行脚本**：`testing/ci/run_l2_functional.ps1`（PowerShell 编排，调 Python smoke 矩阵）

> **runner 语言修订（F.1，META-CI-1b DEV [设计偏离]）**：设计 §2.3 原分层"Python 负责 L2/L3/L4 数据"。实施时 L2 runner 改为 PowerShell（`run_l2_functional.ps1`，与 L1 `run_l1_quality.ps1` 风格一致），内部仍 `subprocess` 调用 Python smoke 脚本矩阵。PM 在 1b 任务分派时确定此偏离，§2.3 分层精神（PowerShell 编排 + Python 数据）仍遵守，仅 L2 runner 顶层语言从 Python 改为 PowerShell。

**数据源分层**：

| 层 | 构造器 | smoke 脚本 | 说明 |
|----|--------|-----------|------|
| Phase 1 | FormatField / Bytes / GreedyBytes / Struct / StructRef | `cargo test --lib`（L1 已含）+ **新建 `phase1_smoke.py`**（用户面 ModbusRTU 样例） | 当前缺独立 smoke，靠单元测试 + 新建用户面样例补齐 |
| Phase 2/2.5 | Tell / Computed | **新建 `phase2_smoke.py`**（表达式 + Computed round-trip） | 同上 |
| Phase 3 | Bitwise / BitStruct / Bit/Nibble/Octet / Padding / BitsSwapped / ByteSwapped / Bytewise | `vet_phase3_binary.py` + `vet_phase32_bitstruct.py` + `vet_phase32_deep.py` + `test_phase33.py`（**按 OBS-1 补入**） | ★ 直接复用 |
| Phase 3 (known_broken) | BitsInteger | ~~`vet_phase3_bits_integer.py`~~（**F.3 修订**：上游兼容性 bug，L2 不调度） | ⚠️ 见 §1.1 B 类 F.3 标注 |
| Phase 4 | GreedyRange / PrefixedArray / RepeatUntil | `phase4_smoke_greedy_range.py` + `phase4_smoke_prefixed_array.py` + `phase4_repeat_until_examples.py`（**按 OBS-1 补入**） | ★ 直接复用 |
| Phase 4 (间接覆盖) | Array / Index / StopIf / Element | `cargo test --lib`（L1 已含）单元测试间接覆盖 | **F.4 修订**：StopIf/Array/Index/Element 无独立 smoke 脚本，由 L1 `cargo test --lib` 单元测试覆盖（struct_node.rs 6 个 + greedy_range.rs 5 个 = 11 个）；F.2 deprecated 的 `phase4_smoke_repeat_until.py` / `v4_smoke_test.py` 已移除 |
| ~~partial~~ | ~~Container / ListContainer~~ | ~~无独立 smoke~~ | 不在 L2 范围（行为覆盖在 RepeatUntil / Struct smoke 中） |

**通过判据**：
- 所有 smoke 脚本 exit 0（脚本内部 assert 全 PASS）
- 每个脚本的行为与 Python construct 一致（脚本内部已做对比，如 `phase4_smoke_greedy_range.py` 的 `assert p.items == [1,2,3,4,5]`）

**失败处理**：
- 任一 smoke 脚本失败 → 报告脚本名 + 失败 assert 行 + 实际值 vs 期望值
- 不阻断其他 smoke 脚本（继续跑完，汇总所有失败）

**特殊处理**：
- RepeatUntil PyCallable 路径已删除（ADR-014），smoke 不含 PyCallable 场景
- 错误路径（D 类）不在 L2 验证范围（属于性能 S-PERF 范畴，由 L3 覆盖）

**耗时**：~3-8min（smoke 脚本本身快，主要耗时在 maturin develop 已由 L1 完成 + Python 启动开销）

### 4.3 L3 性能回归检测（概要）

**详见 §5**。本节只给层次定位。

**覆盖范围**：`docs/perf-scenarios.csv` 中所有测量点（当前 151 个）

**核心机制**：
- 跑 bench 得到新 speedup_x → 与 baseline（perf-scenarios.csv）diff → 按回归判据判定
- 触发回归阈值 → 自动升级为 Controlled A/B Test（§5.7）

**耗时**：~10-20min（151 场景 × 1 轮 min(repeat=5) × number；触发 A/B Test 时 +5-10min）

### 4.4 L4 一致性核查

**规范来源**：`harness/extensions/auditor-extension.md §第 7 类审计项`（AUDITOR 在 phase 验收时审；L4 把它日常化）

**执行脚本**：`testing/ci/run_l4_consistency.py`（Python，纯静态检查，不需要跑 bench）

**核查清单**（与 AUDITOR 第 7 类一一对应，自动化版本）：

| # | 检查项 | 实现 | 通过判据 |
|---|--------|------|---------|
| C1 | inventory.csv 格式合规 | 解析 CSV，校验每行列数 = 13 | 列数不符的行号列表为空 |
| C2 | perf-scenarios.csv 格式合规 | 同上，列数 = 17 | 同上 |
| C3 | CSV 编码 UTF-8 without BOM | 读首 3 字节检查 BOM | 无 BOM |
| C4 | CSV 换行 LF | 读文件检查 `\r\n` | 无 `\r\n` |
| C5 | inventory.csv `perf_data_source` 指针可达 | 解析 `文件:行号` / `文件:行号-行号`，读源文件确认行存在 + 内容非空 | 所有指针可达 |
| C6 | perf-scenarios.csv `data_source` 指针可达 | 同上 | 同上 |

> **OBS-2a 修订（META-CI-1a VET OBS-2）**：C5/C6 当前**仅检查指针行号存在**，未检查"内容非空"（即未校验指针指向的行是否有有效数据）。**PM 决策保持现状**（CSV 由 PM 维护，影响极小），不强制扩展检查。1a VET 实测确认此限制对 L4 阻断能力无实质影响（PM 维护的 CSV 不会有空行指针）。
| C7 | perf-scenarios.csv `meets_10x` 与 `speedup_x` 自洽 | `speedup_x ≥ 10.0 ↔ meets_10x=true` | 不自洽的行号列表为空 |
| C8 | inventory.csv `status=implemented` 构造器 ↔ `nodes/mod.rs` 注册 | 解析 `nodes/mod.rs` 的 `pub mod` / `pub use` 列出已注册节点模块；比对 `impl_module` 字段 | 所有 implemented 构造器的 impl_module 指向的 .rs 文件存在 |
| C9 | inventory.csv `status=implemented` 构造器 ↔ `_descriptors.py` 导出 | 解析 `construct-rs/python/construct/_descriptors.py` 的导出符号；比对构造器名 | 所有 implemented 构造器在 _descriptors.py 有对应导出 |
| C10 | perf-scenarios.csv 的 `constructor` 字段值 ∈ inventory.csv 的 `name` 集合 | 集合差集 | 差集为空 |

**通过判据**：C1-C10 全部 PASS

**失败处理**：
- 任一项 FAIL → 报告具体不一致项（行号 / 字段 / 期望 vs 实际）
- 不阻断其他检查项（继续跑完，汇总）

**耗时**：~10s（纯静态检查，无 IO 密集操作）

**特殊豁免规则**（与 AUDITOR 第 7 类一致）：
- `partial` 状态构造器（如 Container / ListContainer）不要求 C8/C9 完整对应
- `not_implemented` 构造器跳过 C8/C9

---

## §5 性能回归检测框架（核心）

> 本章是 META-CI-1 的核心。设计严格对齐 `harness/experiences.md §L-09` + `performance-gate/SKILL.md Checkpoint 4`，
> 把"跨时段性能对比消除法归因失效"的教训工程化为可自动执行的回归检测框架。

### 5.1 框架架构

```
┌─────────────────────────────────────────────────────────────┐
│  run_l3_perf.py（Python 主调度）                              │
│                                                              │
│  1. 读 baseline: docs/perf-scenarios.csv                     │
│  2. 采集环境: python_ver / venv / CPU / OS / timestamp       │
│  3. 对每个测量点:                                             │
│     ├─ 调用现有 phase4_bench_*.py（子进程隔离）                │
│     ├─ 得到新 speedup_x / rs_ns / pc_ns                      │
│     └─ 与 baseline diff → 判定（PASS/WARN/FAIL）              │
│  4. 若有 FAIL → 触发 Controlled A/B Test 升级路径（§5.7）      │
│  5. 生成报告: reports/l3_<timestamp>.json + .md               │
│  6. 退出码: 0=全 PASS / 1=有 WARN / 2=有 FAIL                 │
└─────────────────────────────────────────────────────────────┘
```

### 5.2 Baseline 管理

**单一事实源**：`docs/perf-scenarios.csv`（PM 主维护，详见 `pm-extension.md §构造器清单维护`）

**Baseline 字段**（L3 使用）：
- `constructor` + `scenario_id` + `direction`：唯一定位一个测量点
- `speedup_x`：回归判据主指标
- `rs_ns_per_call`：辅助指标（L-09 边界场景判据用）
- `meets_10x`：C7 一致性核查用
- `phase`：用于筛选"本 phase 关心的场景"

**Baseline 读取规则**：
- L3 只读 `docs/perf-scenarios.csv`，不修改
- 多个测量点共享同一 `scenario_id`（如 B1 有 parse + build 两行）时，按 `constructor + scenario_id + direction` 三元组唯一定位
- 若 bench 跑出的场景在 baseline 中**不存在**（新场景）→ 标记为 `NEW`，不触发回归判据，提示 PM 是否纳入 baseline

**Baseline 更新流程**（**禁止自动更新**）：
1. L3 报告标注"建议更新 baseline 的场景列表"
2. PM 审查报告，确认性能变化是"改进"而非"回归"
3. PM 手动编辑 `docs/perf-scenarios.csv`（遵守 `pm-extension.md §CSV 编辑规范`）
4. 下次 L3 以新 baseline 为准

> **禁止自动更新的理由**：若 L3 自动把回归后的差值写入 baseline，则回归被"洗白"，无法再检测。baseline 更新必须人工确认（L-03 对策：PM 不接受不对等证据）。

### 5.3 测量口径（强制遵守）

**规范来源**：`performance-gate/SKILL.md` + `plans/phase4-array/总纲.md §S-PERF` + `phase4_47_perf_verify.py` 实现

| 要素 | 要求 | 实现位置 |
|------|------|---------|
| 子进程隔离 | construct-rs 与 Python construct 不可在同一进程导入（包同名） | 双 venv：`crs_venv_new` + `crs_venv_py_new`，`subprocess.run([python_exe, "-c", script])` |
| min(repeat=5) × number | `timeit.repeat(stmt, setup, number=N, repeat=5)` 取 min | 复用 `phase4_47_perf_verify.py:measure_simple` |
| apples-to-apples | 双端都包 Struct（construct-rs 用 `StructMixin`，Python 用 `pc.Struct`） | 复用 `phase4_bench_array_v4.py` 方法论 |
| number 自适应 | 大规模场景降 number（避免单次 >2s） | 复用 `phase4_bench_array_v4.py:NUMBER_FOR` 字典 |
| 环境变量 | `PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1` | 复用 `E01_O1_ab_bench.py:measure` |

**口径不可降级**：L3 必须严格遵守上述口径。任何"为了快而简化"（如同一进程导入 / 减少 repeat / 裸口径对比）都会使数据失效，违反 L-09 对策。

### 5.4 测量环境标注（L-09 强制对策）

**每次 L3 报告必须包含**（缺失则报告无效）：

```json
{
  "environment": {
    "timestamp": "2026-07-28T14:30:00+08:00",
    "python_version": "3.14.0",
    "crs_venv": "<opencode-temp>\\crs_venv_new",
    "pc_venv": "<opencode-temp>\\crs_venv_py_new",
    "construct_rs_commit": "abc1234",
    "construct_py_version": "2.10.70",
    "os": "Windows 10.0.19045",
    "cpu": "Intel(R) Core(TM) i7-XXXX @ 2.xGHz",
    "power_plan": "High Performance",
    "thermal_state": "nominal"
  }
}
```

**采集实现**：
- `python_version`：`sys.version`
- `commit`：`git rev-parse --short HEAD`
- `cpu` / `os`：`platform.processor()` / `platform.platform()`
- `power_plan`（Windows）：`powercfg /getactivepowerscheme`
- `thermal_state`：难以自动采集，标注"unknown"或开发者手动备注

**跨 baseline 对比时**：报告必须附"baseline 测量环境"段落（从 baseline 报告 JSON 读取），并列出环境差异。若 baseline 无环境标注（历史数据）→ 标注"baseline 环境未知，跨时段对比仅供参考，回归判定以 Controlled A/B Test 为准"（L-09 对策）。

### 5.5 报告生成（JSON + Markdown Schema）

**JSON Schema**（`reports/l3_<timestamp>.json`）：

```json
{
  "run_id": "l3_20260728_143000",
  "triggered_by": "manual",
  "mode": "perf",
  "environment": { /* §5.4 */ },
  "baseline_source": "docs/perf-scenarios.csv@abc1234",
  "summary": {
    "total": 151,
    "pass": 140,
    "warn": 8,
    "fail": 3,
    "new": 0,
    "skipped": 0
  },
  "results": [
    {
      "constructor": "Array",
      "scenario_id": "4.6-1",
      "direction": "parse",
      "baseline_speedup": 11.37,
      "new_speedup": 11.20,
      "delta_x": -0.17,
      "delta_pct": -1.5,
      "rs_ns_baseline": 2553,
      "rs_ns_new": 2590,
      "verdict": "PASS",
      "category": "normal",
      "threshold": {"warn_pct": 10, "fail_pct": 20}
    }
  ],
  "ab_test_triggered": ["Array:4.6-1:parse"],
  "ab_test_reports": ["reports/ab_Array_4.6-1_parse_<timestamp>.json"]
}
```

**Markdown Schema**（`reports/l3_<timestamp>.md`，人工审查用）：

```markdown
# L3 性能回归报告 <run_id>

## 环境
（§5.4 环境信息 + baseline 环境差异）

## 汇总
| 指标 | 值 |
|------|----|
| 总场景 | 151 |
| PASS | 140 |
| WARN | 8 |
| FAIL | 3 |

## FAIL 明细（需处理）
| 构造器 | 场景 | 方向 | baseline | new | Δx | 判定 |
|--------|------|------|----------|-----|-----|------|
| Array | 4.6-1 | parse | 11.37x | 9.20x | -2.17x | FAIL(≥10x→<10x) |

## WARN 明细（关注）
...

## 触发的 Controlled A/B Test
- Array:4.6-1:parse → reports/ab_Array_4.6-1_parse_<timestamp>.json
```

### 5.6 回归判据（边界/普通/特殊三档）

**规范来源**：`harness/experiences.md §L-09` + `performance-gate/SKILL.md Checkpoint 4` 结论判据表

**场景分类**：

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

**特殊豁免**（仅 PM 显式声明）：
- 已知待优化的 <10x 场景（如 Phase 4 StopIf B1 类、Phase 1 B1 类，已记入 Phase 5 待办）→ 在 `reports/known_exemptions.json` 列出，L3 跳过其 FAIL 判定，仅 WARN
- 豁免清单由 PM 维护，每次 phase 验收时复审

### 5.7 Controlled A/B Test 升级路径（L-09 核心对策）

**触发条件**：L3 主跑出现 FAIL（含特殊场景 FAIL）

**升级流程**：

1. **识别回归场景**：从 L3 报告 `results` 中筛选 `verdict=FAIL`
2. **定位质疑改动**：`git log --oneline baseline_commit..HEAD`，识别可能引入回归的 commit / 文件
3. **构造对照组**：
   - **阳性对照**：与质疑改动相关、baseline 中预期应有效应的场景（验证 toggle 真实性）
   - **阴性对照**：与质疑改动完全无关、工作量足够大的场景（验证环境漂移幅度）
4. **交替测量**：`A(质疑改动 off) → B(质疑改动 on) → A → B → A → B`（3 轮 6 次），每次 `cargo build --release && maturin develop --release` + sleep 30s
5. **统计分析**：复用 `E01_O1_ab_stats.py`（welch_t + 描述性统计 + |Δ|/noise ratio）
6. **结论判据**（同 Checkpoint 4）：
   - `abs(delta_x) > 0.5` → 回归确认，进 Step 2 排查
   - `abs(delta_x) < 0.3` → 测量波动，不阻断
   - `0.3-0.5` → 加测 5 轮

**实现复用**：
- `ab_harness.ps1`：通用化 `E01_O1_ab_harness.ps1`，参数化 `$TOGGLE_FILE`（质疑改动文件）+ `$BASE_COMMIT`（off 状态 commit）
- `ab_bench_template.py`：通用化 `E01_O1_ab_bench.py`，参数化场景列表 + 阳性/阴性对照
- `ab_stats.py`：直接复用 `E01_O1_ab_stats.py`

**报告**：每个 FAIL 场景生成独立 A/B Test 报告 `reports/ab_<constructor>_<scenario>_<direction>_<timestamp>.json`

**退出码**：A/B Test 确认回归 → L3 退出码 2（FAIL）；确认波动 → 退出码 1（WARN）

### 5.8 失败处理与 baseline 更新流程

```
L3 主跑
  ├─ 全 PASS → 退出码 0，结束
  ├─ 有 WARN，无 FAIL → 退出码 1，报告供开发者参考
  └─ 有 FAIL → 触发 Controlled A/B Test（§5.7）
       ├─ A/B 确认回归 → 退出码 2，阻断；开发者修复后重跑 L3
       ├─ A/B 确认波动 → 退出码 1；PM 评估是否更新 baseline
       └─ A/B 不确定（0.3-0.5）→ 加测；仍不确定则 PM 人工裁决
```

**baseline 更新触发**（PM 人工）：
- A/B Test 确认波动 + 跨时段环境漂移显著 → PM 更新 perf-scenarios.csv，附 A/B Test 报告指针到 `notes` 列
- 性能改进（`IMPROVED`）累积 >20% → PM 评估更新 baseline

**禁止**：
- 自动更新 baseline（§5.2）
- 跳过 A/B Test 直接判定回归（L-09 对策）
- 用单次跨时段对比替代 Controlled A/B Test（L-09 对策）

---

## §6 子任务分解建议

META-CI-1 工作量较大（涉及 PowerShell + Python + bench 矩阵整合 + A/B Test 通用化），建议拆为 4 个子任务。PM 也可合并为 2 个（见 §6.5）。

### 6.1 推荐拆法：4 个子任务

| 子任务标识 | 范围 | 依赖 | 预估 DEV 工时 | 优先级 |
|-----------|------|------|--------------|--------|
| `META-CI-1a [L1+L4 门禁实现]` | `run_l1_quality.ps1` + `run_l4_consistency.py` + `lib_smoke.ps1` 共享函数 + `run_smoke.ps1` 骨架（仅 -Level "L1,L4"） | 无 | 0.5-1 天 | P0（最小可用门禁） |
| `META-CI-1b [L2 功能门禁实现]` | `run_l2_functional.ps1`（PowerShell 编排，调现有 smoke 矩阵）+ **新建 `phase1_smoke.py` + `phase2_smoke.py`** 补齐早期构造器 | META-CI-1a | 1-1.5 天 | P1 |
| `META-CI-1c [L3 性能回归检测框架]` | `run_l3_perf.py`（baseline diff + 报告 + 环境标注）+ `ab_test/` 通用化（参数化 `E01_O1_ab_*`） | META-CI-1a | **3-5 天**（按 META-CI-1-REV OBS-TIME-1 修订：原 2-3 天估计偏低，因 L3 涉及 baseline diff 逻辑 + 环境采集 + 适配 5+ 个 bench 脚本的不同接口 + JSON/MD 报告 + A/B Test 通用化 + known_exemptions.json 维护，复杂度显著高于其他子任务） | P1（核心，复杂度最高） |
| `META-CI-1d [触发时机 + hook 安装 + 文档]` | `install_hook.ps1`（pre-commit 可选安装）+ 使用文档（README）+ `-Level L2/L3/All` 完善（D.2 命名） | META-CI-1a/1b/1c | 0.5 天 | P2 |

### 6.2 子任务 1a 详细范围（P0，最小可用门禁）

**交付物**：
- `testing/ci/lib_smoke.ps1`：共享函数（`Resolve-Venv` / `Write-CiLog` / `Invoke-Cargo` / `Collect-Environment`）
- `testing/ci/run_l1_quality.ps1`：§4.1 命令序列
- `testing/ci/run_l4_consistency.py`：§4.4 C1-C10 检查项
- `testing/ci/run_smoke.ps1`：主入口，支持 `-Level "L1,L4"`（L1+L4，D.2 命名）
- `testing/ci/reports/.gitkeep`

**验收标准**：
- S-QUAL：clippy 零 warning + fmt 通过（PowerShell 脚本不强制 fmt，但 Python 部分需通过）
- S-FUNC：`run_smoke.ps1 -Level "L1,L4"` 在干净仓库上全 PASS（D.2 命名）
- S-ARCH：L4 的 C1-C10 全部实现

### 6.3 子任务 1b 详细范围

**交付物**：
- `testing/ci/run_l2_functional.ps1`（**F.1 修订**：runner 语言从 .py 改为 .ps1，与 L1 风格一致；内部仍 subprocess 调 Python smoke 脚本矩阵）：调度 Phase 1-4 的 smoke 脚本矩阵
- `experiments/phase1_smoke.py`（**新建**）：FormatField / Bytes / Struct 用户面 smoke（ModbusRTU 样例 round-trip）
- `experiments/phase2_smoke.py`（**新建**）：Tell / Computed / 表达式 smoke

**验收标准**：
- S-FUNC：所有 implemented 构造器至少有 1 个 smoke 场景覆盖
- S-ARCH：smoke 矩阵与 inventory.csv 的 implemented 集合对应

### 6.4 子任务 1c 详细范围（核心）

**交付物**：
- `testing/ci/run_l3_perf.py`：§5.1 框架（baseline 读取 + 环境采集 + 场景调度 + diff + 报告 + A/B Test 升级触发）
- `testing/ci/run_smoke.ps1` 扩展 `-Level "L1,L3,L4"`（perf 模式，D.2 命名）
- `testing/ci/ab_test/ab_harness.ps1`：通用化 `E01_O1_ab_harness.ps1`
- `testing/ci/ab_test/ab_bench_template.py`：通用化 `E01_O1_ab_bench.py`
- `testing/ci/ab_test/ab_stats.py`：复用 `E01_O1_ab_stats.py`
- `testing/ci/reports/known_exemptions.json`：已知豁免清单（**初始含 21 条目**，对齐用户决策事项 A/B/C，按 META-CI-1-REV 关键缺陷修订）

**known_exemptions.json schema 与初始内容**（按 META-CI-1-REV §5.5 修订要求 + 数据源 `docs/perf-scenarios.csv` 实测）：

```json
{
  "version": "1.0",
  "last_updated": "2026-07-28",
  "exemptions": [
    // ─── 事项 A：硬约束 #5 仅 Phase 4 起适用，Phase 1/2/2.5 共 18 个 <10x 不追溯 ───
    // 数据源：docs/perf-scenarios.csv L2-27 实测，speedup_x 逐项核对
    {"scenario_id": "FormatField:B1:parse",        "user_decision": "A", "phase": 1,    "baseline_speedup": 8.09, "reason": "事项 A: 硬约束 #5 仅 Phase 4 起适用；Phase 1 共 7 个 <10x 不追溯复审", "action": "skip_fail_verdict", "review_at": "Phase 5", "phase_5_todo": true},
    {"scenario_id": "FormatField:B2:parse",        "user_decision": "A", "phase": 1,    "baseline_speedup": 9.66, "reason": "事项 A: Phase 1 <10x 不追溯", "action": "skip_fail_verdict", "review_at": "Phase 5", "phase_5_todo": true},
    {"scenario_id": "FormatField:B5:parse",        "user_decision": "A", "phase": 1,    "baseline_speedup": 3.09, "reason": "事项 A: 嵌套 Struct 5 层 Phase 1 <10x", "action": "skip_fail_verdict", "review_at": "Phase 5", "phase_5_todo": true},
    {"scenario_id": "FormatField:B5:build",        "user_decision": "A", "phase": 1,    "baseline_speedup": 4.02, "reason": "事项 A: 嵌套 Struct 5 层 Phase 1 <10x", "action": "skip_fail_verdict", "review_at": "Phase 5", "phase_5_todo": true},
    {"scenario_id": "FormatField:B6:parse",        "user_decision": "A", "phase": 1,    "baseline_speedup": 3.74, "reason": "事项 A: 深嵌套 10 层 Phase 1 <10x", "action": "skip_fail_verdict", "review_at": "Phase 5", "phase_5_todo": true},
    {"scenario_id": "FormatField:B6:build",        "user_decision": "A", "phase": 1,    "baseline_speedup": 5.07, "reason": "事项 A: 深嵌套 10 层 Phase 1 <10x", "action": "skip_fail_verdict", "review_at": "Phase 5", "phase_5_todo": true},
    {"scenario_id": "FormatField:B7:parse",        "user_decision": "A", "phase": 1,    "baseline_speedup": 5.17, "reason": "事项 A: 空 Struct 0 字段 Phase 1 <10x", "action": "skip_fail_verdict", "review_at": "Phase 5", "phase_5_todo": true},

    {"scenario_id": "Bytes:E1-p2:parse",           "user_decision": "A", "phase": 2,    "baseline_speedup": 7.11, "reason": "事项 A: Phase 2 共 5 个 <10x 不追溯", "action": "skip_fail_verdict", "review_at": "Phase 5", "phase_5_todo": true},
    {"scenario_id": "Bytes:E1-p2:build",           "user_decision": "A", "phase": 2,    "baseline_speedup": 9.54, "reason": "事项 A: Phase 2 <10x 不追溯", "action": "skip_fail_verdict", "review_at": "Phase 5", "phase_5_todo": true},
    {"scenario_id": "Bytes:E2-p2:parse",           "user_decision": "A", "phase": 2,    "baseline_speedup": 8.51, "reason": "事项 A: Phase 2 <10x 不追溯", "action": "skip_fail_verdict", "review_at": "Phase 5", "phase_5_todo": true},
    {"scenario_id": "Computed:E3-p2:parse",        "user_decision": "A", "phase": 2,    "baseline_speedup": 8.24, "reason": "事项 A: Phase 2 <10x 不追溯", "action": "skip_fail_verdict", "review_at": "Phase 5", "phase_5_todo": true},
    {"scenario_id": "Computed:E3-p2:build",        "user_decision": "A", "phase": 2,    "baseline_speedup": 8.65, "reason": "事项 A: Phase 2 <10x 不追溯", "action": "skip_fail_verdict", "review_at": "Phase 5", "phase_5_todo": true},

    {"scenario_id": "Bytes:E1-p25:parse",          "user_decision": "A", "phase": "2.5", "baseline_speedup": 7.71, "reason": "事项 A: Phase 2.5 Vec 化前共 6 个 <10x 不追溯", "action": "skip_fail_verdict", "review_at": "Phase 5", "phase_5_todo": true},
    {"scenario_id": "Bytes:E1-p25:build",          "user_decision": "A", "phase": "2.5", "baseline_speedup": 9.80, "reason": "事项 A: Phase 2.5 <10x 不追溯", "action": "skip_fail_verdict", "review_at": "Phase 5", "phase_5_todo": true},
    {"scenario_id": "Bytes:E2-p25:parse",          "user_decision": "A", "phase": "2.5", "baseline_speedup": 9.22, "reason": "事项 A: Phase 2.5 <10x 不追溯", "action": "skip_fail_verdict", "review_at": "Phase 5", "phase_5_todo": true},
    {"scenario_id": "Computed:E3-p25:parse",       "user_decision": "A", "phase": "2.5", "baseline_speedup": 8.18, "reason": "事项 A: Phase 2.5 <10x 不追溯", "action": "skip_fail_verdict", "review_at": "Phase 5", "phase_5_todo": true},
    {"scenario_id": "Computed:E3-p25:build",       "user_decision": "A", "phase": "2.5", "baseline_speedup": 7.90, "reason": "事项 A: Phase 2.5 <10x 不追溯（未达 10x）", "action": "skip_fail_verdict", "review_at": "Phase 5", "phase_5_todo": true},
    {"scenario_id": "Bytes:E1-final:parse",        "user_decision": "A", "phase": "2.5", "baseline_speedup": 9.90, "reason": "事项 A: Phase 2.5 最终 <10x（接近阈值）", "action": "skip_fail_verdict", "review_at": "Phase 5", "phase_5_todo": true},

    // ─── 事项 B：StopIf B1 类候选扩展（E01 边界场景）──────────────────────────────
    // 注：StopIf S01/S02/S03 O1 优化后已全部 ≥10x（CSV L146-151，10.72-15.69x），无需豁免
    {"scenario_id": "Index:E01-4.7-verify:parse",  "user_decision": "B", "phase": "4.6", "baseline_speedup": 9.51, "reason": "事项 B: Array(0 Index) parse 边界场景，B1 类小字段 FFI 稀释；与 StopIf B1 同根因；待 Phase 5 复审", "action": "skip_fail_verdict", "review_at": "Phase 5", "phase_5_todo": true},

    // ─── 事项 C：错误路径 D 类剥离为 4.x 子任务 ─────────────────────────────────
    // 数据源：CSV L115-116 实测，scenario_id 用 CSV 原值（p_err-4.x / a_err-4.x），phase 字段记数据来源阶段（4.6）
    {"scenario_id": "PrefixedArray:p_err-4.x:build", "user_decision": "C", "phase": "4.6", "baseline_speedup": 2.12, "reason": "事项 C: 错误路径 D 类剥离为 4.x（Python baseline 4864ns 结构性瓶颈；4.x 内子项 p_err_overflow 预测最优 ~3-5x）", "action": "skip_fail_verdict", "review_at": "4.x ACCEPTED + Phase 5", "phase_5_todo": true, "subtask": "4.x"},
    {"scenario_id": "Array:a_err-4.x:parse",         "user_decision": "C", "phase": "4.6", "baseline_speedup": 6.93, "reason": "事项 C: 错误路径 D 类剥离为 4.x（4.x 内子项 a_err_eof，O1-O4 优化预测达 ≥10x）", "action": "skip_fail_verdict", "review_at": "4.x ACCEPTED", "phase_5_todo": false, "subtask": "4.x"}
  ]
}
```

**条目统计**：21 条 = 事项 A 18 条（Phase 1 × 7 + Phase 2 × 5 + Phase 2.5 × 6）+ 事项 B 1 条（E01 边界场景）+ 事项 C 2 条（p_err-4.x + a_err-4.x 错误路径 D 类剥离至 4.x）。

**维护规则**：
1. **scenario_id 命名**：`<constructor>:<scenario_id>:<direction>` 三元组（与 perf-scenarios.csv 一致，L4 C10 一致性核查要求）
2. **baseline_speedup 字段**：从 perf-scenarios.csv 复制（L-02 对策：数据必须从源文件复制，不凭记忆）
3. **action 字段**：当前仅 `skip_fail_verdict`（L3 不阻断 FAIL，仅 WARN）；后续可扩展 `warn_only` / `ignore` 等
4. **review_at 字段**：豁免复审时机（Phase 5 / 子任务 ACCEPTED 等）
5. **更新触发**：① 用户决策变更（如事项 A 重新解读）② 豁免场景实际达标（如 O1 后 StopIf B1 已 ≥10x，应从豁免中移除）③ 新豁免场景出现（需 PM 显式声明 + 附用户决策依据）
6. **PM 维护**：豁免清单由 PM 主维护（与 perf-scenarios.csv 同档），L3 只读

**验收标准**：
- S-FUNC：在当前 HEAD 上跑 L3，151 场景全部能产生 verdict（PASS/WARN/FAIL/NEW/SKIPPED）
- S-PERF：测量口径符合 §5.3（子进程隔离 + min(repeat=5) × number + apples-to-apples）
- S-ARCH：A/B Test 升级路径可在人工构造的回归场景上触发并产生报告

**注意**：本子任务**不**包含新建 bench 场景——只复用现有 `phase4_bench_*.py`。Phase 1/2 的 bench 缺口（§1.1 P6）单列为 follow-up（可并入 Phase 5 性能优化子任务）。

### 6.5 合并方案：2 个子任务（备选）

若 PM 认为拆 4 个过细，可合并：
- `META-CI-1a' [L1+L2+L4 门禁实现]`（合并 1a + 1b）
- `META-CI-1b' [L3 性能回归检测 + 触发时机]`（合并 1c + 1d）

**ARCH 建议**：采用 4 子任务拆法。理由：
1. L3（1c）复杂度显著高于其他（涉及 A/B Test 通用化 + 统计分析），独立便于 REVIEW
2. 1a 作为 P0 可先行交付，提供"最小可用门禁"，不必等 L3 完成
3. 1d 文档与 hook 安装依赖前三个的接口稳定，放最后避免反复改

---

## §7 跨阶段模式沉淀建议

### 7.1 建议沉淀为 ADR

> **ADR 编号分配（PM 2026-07-28 决策）**：本设计的核心 ADR 编号正式定为 **ADR-020 = CI 冒烟门禁方法学**（由 ARCH 在 META-CI-1 ACCEPTED 时起草）。4.x 错误路径优化的"错误抛出 fast-path"已分配 ADR-019，互不冲突。

本设计的两个核心决策值得固化为 ADR，供后续 phase 与跨工程复用：

#### ADR-020: CI 冒烟门禁方法学（按 PM 编号分配修订）

> **已正式沉淀（2026-07-28）**：本节候选预览已由 `docs/decisions/ADR-020-CI冒烟门禁方法学.md` 正式固化（META-CI 整体 ACCEPTED 触发）。以下候选描述保留作设计历史，正式决策以 ADR-020 为准（含 D.2 命名修订：`-Mode quick/func/perf/full` 已改为 `-Level L1/L2/L3/L4`）。

**decides**：construct-rs 采用"本地 PowerShell + Python 混合 runner + 四层门禁（L1 质量 / L2 功能 / L3 性能 / L4 一致性）+ perf-scenarios.csv 为性能 baseline"的冒烟门禁架构，不依赖远程 CI。

**需固化的决策点**：
1. 平台选型（PowerShell 首选，排除 GitHub Actions / justfile / Makefile，论证见 §2.2）
2. 触发时机分层（pre-commit 仅 L1+L4，手动触发分 quick/func/perf/full，论证见 §3）
3. baseline 不自动更新（§5.2，L-03 对策）
4. 性能回归判据三档（边界/普通/特殊，§5.6，L-09 对策）

**Consequences**：
- 正面：跨 phase 复用 / 防回归 / 把 L-09 教训工程化
- 负面：需维护 `testing/ci/` 代码（~5-8 个脚本）；L3 单次跑 10-20min
- 中性：依赖 Windows PowerShell（跨平台迁移需重写 runner）

#### ADR-021: 性能 baseline 单一事实源与更新流程（可选，若 PM 认为 ADR-020 过长）

**decides**：perf-scenarios.csv 是性能 baseline 的唯一事实源；CI 不自动更新；更新必须 PM 显式确认并附 A/B Test 证据。

### 7.2 不沉淀为 ADR 的部分

- §4 L1 命令序列：已在 `AGENTS.md §3` 定义，CI 只是执行，不需重复 ADR
- §4 L4 检查清单：已在 `auditor-extension.md §第 7 类` 定义，CI 是自动化版本，不需 ADR
- §5.3 测量口径：已在 `performance-gate/SKILL.md` 定义，CI 遵守即可

### 7.3 是否值得跨工程复用

本设计的"四层门禁 + baseline diff + A/B Test 升级路径"模式具有跨工程通用性，但：
- PowerShell runner 是 Windows 特定（跨工程若非 Windows 需重写）
- perf-scenarios.csv schema 是项目特定
- A/B Test 工具链（`E01_O1_ab_*`）是项目特定

**建议**：不提取为通用 AHE skill（避免 L-08 通用/项目混淆）。本设计作为 construct-rs 项目特定资产保留在 `docs/design/`。

---

## §8 §0 原则对照表

> CI 冒烟门禁**不涉及 parse/build 数据流**（CI 是测试基础设施，不在运行时数据路径上），但仍逐条对照 `AGENTS.md §0 核心原则`，确认设计不违反。

| §0 原则 | CI 设计对照 | 结论 |
|---------|------------|------|
| 1. **一次 FFI**：parse/build 各只有一次 Python↔Rust 边界穿越 | CI 不在 parse/build 运行时路径；CI 调用 bench 脚本时，bench 脚本本身遵守一次 FFI（由实现保证，CI 不改变） | ✅ 不涉及，不违反 |
| 2. **无中间表示层**：parse 直接构造 PyObject / build 直接读 PyObject | CI 不引入任何 parse/build 中间数据类型；CI 报告用 JSON 是 CI 自身产物，非 parse/build 中间层 | ✅ 不涉及，不违反 |
| 3. **输出/输入侧无抽象 trait** | CI 不在 parse/build 输入输出侧引入 trait；CI runner 的 PowerShell/Python 分层是测试基础设施的编排，非数据流抽象 | ✅ 不涉及，不违反 |
| 4. **pyo3 是核心依赖** | CI L1 含 `maturin develop --release`（安装 pyo3 扩展到 venv）；CI L3 通过 bench 脚本间接验证 pyo3 路径正确 | ✅ 遵守，CI 利用 pyo3 但不改变其核心地位 |
| 5. **mashumaro 式 API**：`@dataclass class X(StructMixin)` | CI L2 smoke 脚本使用 `StructMixin` 用户面 API（如 `phase4_smoke_greedy_range.py`），验证 mashumaro 式 API 行为正确 | ✅ 遵守，CI 验证 API 不改变它 |
| 6. **构造器分派**：enum_dispatch 静态分派 | CI 不涉及分派机制；CI 通过 `nodes/mod.rs` 静态扫描（L4 C8）验证分派注册完整 | ✅ 不涉及，不违反 |
| 7. **错误处理**：Result<T, ConstructError> + thiserror + path | CI 自身错误处理：PowerShell 用 `$ErrorActionPreference="Stop"` + exit code；Python 用异常 + JSON 报告。CI 不改变 construct-rs 的错误处理 | ✅ 遵守，CI 自身错误处理独立于业务错误处理 |
| 8. **Stream 抽象**：纯 Rust 内部 | CI 不涉及 Stream；CI 验证 Stream 相关构造器（Bytes / GreedyRange 等）行为正确，但不改变 Stream 抽象 | ✅ 不涉及，不违反 |

**总结**：CI 冒烟门禁设计**完全不触碰 §0 核心原则**。CI 是"测试基础设施"，位于 parse/build 运行时路径之外。CI 通过验证而非改变来支持 §0 原则——若 CI 检测到性能回归（如某改动意外引入中间表示层导致加速比下降），CI 会 FAIL 并触发调查，反而强化 §0 原则的执行。

---

## 附录 A：关键脚本接口约定（供 DEV 实现参考）

> 本附录给出核心脚本的接口签名，**不是完整实现**（实现属 CODING 阶段）。目的是让 REV 检视时能确认接口设计合理，让 DEV 实现时有明确蓝图。

### A.1 `testing/ci/run_smoke.ps1`

> **D.2 修订（META-CI-1a DEV [设计质疑]）**：参数命名从 `-Mode quick/func/perf/full` 改为 `-Level L1/L2/L3/L4`（更直观，与门禁层次命名直接对应）。实施时用 `[string]$Level`（非 `ValidateSet`），内部 `Resolve-Levels` 函数解析 `All` / 单层 / 逗号分隔组合。

```powershell
param(
    # D.2 修订：-Mode 改 -Level，支持 L1/L2/L3/L4/All 或逗号分隔组合
    [string]$Level = 'All',

    [switch]$AllowRegression,   # 跳过 L3 FAIL 阻断（仅警告）
    [switch]$NoAbTest,          # L3 FAIL 时不触发 Controlled A/B Test
    [switch]$UpdateBaseline,    # 禁止：baseline 更新走人工，此参数仅作错误提示
    [string]$ReportDir = (Join-Path $PSScriptRoot "reports")
)
# 行为：按 $Level 调度 L1-L4（Resolve-Levels 解析），汇总报告，返回 exit code（0/1/2）
```

> **OBS-1a 修订（META-CI-1a VET OBS-1）**：多 level 组合（如 L1+L4）**必须加引号** `-Level "L1,L4"`。PS 5.1 native comma 是数组操作符，未加引号的 `-Level L1,L4` 会被解析为数组→空格连接字符串→`Resolve-Levels` 报错。单层（`-Level L1`）和 `-Level All` 不受影响。pre-commit.template 已用 `-Level "L1,L4"`（双引号）规避此陷阱。

### A.2 `testing/ci/run_l3_perf.py`

```python
def main():
    baseline = load_baseline("docs/perf-scenarios.csv")  # List[Dict]
    env = collect_environment()                          # §5.4
    results = []
    for point in baseline:
        new = run_bench_point(point)                     # 调用现有 phase4_bench_*.py
        verdict = classify(point, new)                   # §5.6 三档判据
        results.append((point, new, verdict))
    report = build_report(results, env, baseline_commit)
    write_json(report, f"reports/l3_{timestamp}.json")
    write_markdown(report, f"reports/l3_{timestamp}.md")
    fails = [r for r in results if r.verdict == "FAIL"]
    if fails and not args.no_ab_test:
        for f in fails:
            run_ab_test(f)                               # §5.7 升级路径
    return exit_code(results)                            # 0/1/2
```

### A.3 `testing/ci/run_l4_consistency.py`

```python
def check_c7_meets_10x_self_consistent(row):
    """speedup_x ≥ 10.0 ↔ meets_10x == 'true'"""
    speedup = float(row["speedup_x"]) if row["speedup_x"] else None
    meets = row["meets_10x"].strip().lower() == "true"
    if speedup is None:
        return meets == False or row["speedup_x"] == ""  # 空值允许 meets=false
    return (speedup >= 10.0) == meets
```

---

## 附录 B：与 AUDITOR 第 7 类的关系

L4 一致性核查（§4.4）是 `auditor-extension.md §第 7 类审计项`的**自动化日常版本**：

| AUDITOR 第 7 类（phase 验收时人工审） | L4（每次改动自动跑） |
|--------------------------------------|---------------------|
| 在 phase 打 tag 前一次性审 | 开发者每次跑 smoke-quick 时自动跑 |
| AUDITOR 手工执行 7 项检查 | 脚本自动执行 C1-C10（覆盖面更广） |
| 发现问题→驳回 PM | 发现问题→报告 + 退出码非 0 |

**关系定位**：L4 不替代 AUDITOR。AUDITOR 仍需在 phase 验收时审（L4 只能查"格式/指针/自洽"，AUDITOR 还需审"PM 验收证据类型/模式沉淀"等 L4 无法自动化的项）。L4 把 AUDITOR 第 7 类的**机械检查部分**日常化，让 AUDITOR 聚焦**判断性检查**。

---

> **设计完成**。本设计覆盖 META-CI-1 任务要求的全部 9 节（§0-§8）。等 PM 转交 REV 检视。
