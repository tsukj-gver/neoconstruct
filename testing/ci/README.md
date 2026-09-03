# CI 冒烟门禁系统（experiments/ci）

> **META-CI** 交付物。设计依据：[`docs/design/基础设施/CI冒烟门禁设计.md`](../../docs/design/基础设施/CI冒烟门禁设计.md)。
> 目标：本地可运行的冒烟门禁，**不依赖远程 CI 服务器**，兼容"禁止 git push"的本地开发流程。

---

## 1. 系统介绍

neoconstruct 的 CI 冒烟门禁由 **4 层** + **Controlled A/B Test 升级路径**组成：

| 层次 | 名称 | 范围 | 耗时 | 脚本 |
|------|------|------|------|------|
| **L1** | 质量门禁 | cargo build / clippy / fmt / test + maturin develop | 30s-2min | `run_l1_quality.ps1` |
| **L2** | 功能门禁 | 40 个 implemented 构造器 × 用户面 smoke（与 Python construct 行为对比） | 3-8min | `run_l2_functional.ps1` + `experiments/phase{1,2}_smoke.py` |
| **L3** | 性能回归检测 | perf-scenarios.csv baseline diff + 三档判据 + Controlled A/B Test 升级 | 10-20min | `run_l3_perf.{ps1,py}` + `ab_test/` |
| **L4** | 一致性核查 | CSV 格式 / 指针可达 / 字段自洽 / 与源码无遗漏（C1-C10） | ~10s | `run_l4_consistency.py` |

**架构原则**（设计 §2.3）：
- PowerShell 负责"编排"（调度 + 退出码 + 报告）
- Python 负责"数据"（场景矩阵 / baseline diff / 统计分析）
- 复用而非重写（L2/L3 直接 subprocess 调用现有 `phase*_smoke_*.py` / `phase4_bench_*.py`）

---

## 2. 快速开始（5 分钟）

### 2.1 全量门禁（推荐 phase 验收前跑）

```powershell
powershell -File testing/ci/run_smoke.ps1 -Level All
```

等价于 `-Level "L1,L2,L3,L4"`，~15-30min。

### 2.2 快速门禁（pre-commit 默认）

```powershell
powershell -File testing/ci/run_smoke.ps1 -Level "L1,L4"
```

~30s-2min。覆盖质量（编译/clippy/fmt/test/maturin）+ 一致性（CSV 核查）。

> **PowerShell 5.1 注意**：`-Level` 参数接收逗号分隔字符串时**必须加引号**（详见 §6 OBS-1）。
> `-Level All` / `-Level L1` / `-Level L4` 无需引号。

### 2.3 其他常用调用

| 命令 | 含义 | 耗时 |
|------|------|------|
| `run_smoke.ps1 -Level L1` | 仅质量门禁 | 30s-2min |
| `run_smoke.ps1 -Level L2` | 仅功能门禁（需先成功 L1 安装 maturin develop） | 3-8min |
| `run_smoke.ps1 -Level L3 -L3QuickMode` | 仅性能门禁快速子集（5 场景） | 5-10min |
| `run_smoke.ps1 -Level L3` | 仅性能门禁全场景（同 L3QuickMode，因 SCENARIO_DEFS 当前仅 5 条） | 5-10min |
| `run_smoke.ps1 -Level L4` | 仅一致性核查 | ~10s |
| `run_smoke.ps1 -Level "L1,L2,L4"` | 不含性能的"日常"组合 | 3-8min |

### 2.4 退出码语义

| 层次 | 0 | 1 | 2 |
|------|---|---|---|
| L1 / L2 / L4 | PASS | FAIL | （参数错误） |
| L3 | 全 PASS | 有 WARN，无 FAIL | 有 FAIL（或 A/B Test 确认回归） |
| `run_smoke.ps1`（整体） | Overall PASS | Overall FAIL（任一层次失败） | （参数错误） |

> L3 的 WARN（1）在 `run_smoke.ps1` 整体判定中**视为通过**（不阻断）；只有 FAIL（2）才阻断 Overall。

---

## 3. 4 层门禁详细说明

### 3.1 L1 质量门禁（`run_l1_quality.ps1`）

**5 步命令序列**（任一失败则 L1 FAIL，fail-fast）：

1. `cargo build --release`
2. `cargo clippy --all-targets -- -D warnings`（零 warning）
3. `cargo fmt --check`
4. `cargo test --lib`
5. `maturin develop --release`（安装到 `crs_venv_new`，供 L2/L3 使用）

**全局前置环境变量**（D.1 文档修订）：
- `PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1` 必须在**全部 5 步之前**设置（设计 §4.1 原文仅写在步骤 5 注释中，已修订为"全局前置"——见 VET 审查报告 §D.1 评估）
- `VIRTUAL_ENV=<crs_venv_new>` 同步设置（maturin 识别）

**报告**：`reports/<timestamp>_L1_report.json`（含 `passed` / `steps[]` / `duration`）。

### 3.2 L2 功能门禁（`run_l2_functional.ps1` + `phase{1,2}_smoke.py`）

**覆盖范围**：`docs/constructors-inventory.csv` 中所有 `status=implemented` 的 40 个构造器（39 直接 smoke + StopIf 间接由 L1 单元测试覆盖）。`status=partial` 的 Container / ListContainer 不在范围（设计 §4.2 OBS-2）。

**调度矩阵**：12 个 smoke 脚本，按 Phase 1/2/3/3.2/3.3/4/4.5 顺序：

| Phase | Script | Venv | 期望 |
|-------|--------|------|------|
| 1 | `phase1_smoke.py` | CRS | pass（176 场景：FormatField 16 单例 × 4-5 值 + Bytes/GreedyBytes/Struct/ModbusRTU） |
| 2 | `phase2_smoke.py` | CRS | pass（28 场景：Bytes(count/field/arith) + Computed + Tell+Computed E3） |
| 3 | `vet_phase3_binary.py` | PC | pass |
| 3 | `vet_phase3_bits_integer.py` | PC | **known_broken**（Python 3.14 + construct 2.10.70 上游兼容性问题，详见 §7） |
| 3.2 | `vet_phase32_bitstruct.py` | CRS | pass |
| 3.2 | `vet_phase32_deep.py` | CRS | pass |
| 3.3 | `test_phase33.py` | CRS | pass |
| 4 | `phase4_smoke_greedy_range.py` | CRS | pass |
| 4 | `phase4_smoke_prefixed_array.py` | CRS | pass |
| 4 | `phase4_smoke_repeat_until.py` | CRS | **deprecated**（v4 PyCallable，ADR-014 删除路径；详见 §7） |
| 4 | `phase4_repeat_until_examples.py` | CRS | pass（v5 21 用户面样例） |
| 综合 | `v4_smoke_test.py` | CRS | **deprecated**（v4 Container lib，ADR-011 下线） |

**判定语义**：
- `pass`：必须通过（FAIL → L2 FAIL，阻断）
- `known_broken`：已知问题，FAIL 仅报告不阻断
- `deprecated`：已废弃（被新脚本取代），FAIL 仅报告不阻断

**报告**：`reports/<timestamp>_L2_report.json`（含 12 脚本逐项 + summary + crs/pc venv 路径）。

### 3.3 L3 性能回归检测（`run_l3_perf.{ps1,py}` + `ab_test/`）

**框架**（设计 §5）：
1. 读 baseline（`docs/perf-scenarios.csv`，**只读**——L-03 对策）
2. 采集测量环境（§5.4：Python 版本 / construct 版本 / commit / OS / CPU / 电源计划 + L-09 disclaimer）
3. 对每个测量点跑 bench（子进程隔离 + min(repeat=5) × number + apples-to-apples）
4. 与 baseline diff → 三档判据（§5.6）
5. 若有 FAIL → 触发 Controlled A/B Test 升级路径（§5.7）
6. 生成 JSON + Markdown 报告

**三档判据**（设计 §5.6 + L-09 对策）：

| 类别 | 判定条件 | WARN | FAIL |
|------|---------|------|------|
| **边界场景** | rs_ns < 300ns **或** baseline∈[9, 11] | `|Δx| > 0.5` | `|Δx| > 1.0` |
| **普通场景** | 非边界 | `|Δ%| > 10%` | `|Δ%| > 20%` |
| **特殊场景**（硬约束 #5） | baseline≥10x → new<10x | — | 直接 FAIL（无 WARN） |

**裁决规则**：当 baseline∈[10, 11] 同时命中"特殊场景"与"边界场景"时，**特殊场景优先**（直接 FAIL，无 WARN）。

**baseline 不自动更新**（L-03 对策）：L3 仅读 CSV，不写。更新由 PM 显式确认 + 附 A/B Test 证据。

**⚠️ 当前限制**：SCENARIO_DEFS 当前仅覆盖 **5 个 Phase 4 代表性场景**（4 quick + 1 非 quick）。其余 149 场景标记 SKIPPED。**全 151 场景映射为 Phase 5 follow-up**（详见 §7 当前限制）。

**Controlled A/B Test 升级路径**（`ab_test/`，L-09 核心对策）：

- 触发条件：L3 主跑出现 FAIL（非豁免）
- 流程：识别回归场景 → 定位质疑改动 → 构造阳性/阴性对照 → 交替测量（A→B→A→B→A→B 3 轮）→ welch_t 统计分析
- 判据：`|Δx|>0.5 → REGRESSION` / `|Δx|<0.3 → NOISE` / 介于 → INTERMEDIATE（加测）
- 实现：复用并参数化 `experiments/E01_O1_ab_{bench,harness,stats}.py`

**known_exemptions.json**（21 条目，PM 主维护）：
- 事项 A：18 条（Phase 1/2/2.5 历史 <10x 不追溯）
- 事项 B：1 条（Index E01 边界场景，B1 类小字段 FFI 稀释）
- 事项 C：2 条（4.x 错误路径 D 类剥离）

豁免项的 FAIL 自动降级为 WARN（不阻断 L3 整体）。

**报告**：`reports/<timestamp>_L3_report.{json,md}`。

### 3.4 L4 一致性核查（`run_l4_consistency.py`）

**10 项 check**（设计 §4.4，对应 AUDITOR 第 7 类审计项的自动化版本）：

| # | 检查项 | 通过判据 |
|---|--------|---------|
| C1 | inventory.csv 列数 = 13 | 不符的行号列表为空 |
| C2 | perf-scenarios.csv 列数 = 17 | 同上 |
| C3 | CSV 编码 UTF-8 without BOM | 无 BOM |
| C4 | impl_module 指向的 .rs 文件存在 | 所有文件存在 |
| C5 | inventory perf_data_source 指针可达 | 行号存在 |
| C6 | perf-scenarios data_source 指针可达 | 行号存在 |
| C7 | inventory implemented ↔ nodes/mod.rs 注册 | 全部对应 |
| C8 | meets_10x ↔ speedup_x 自洽 | 全部自洽 |
| C9 | status synced（无 stale not_implemented） | 全部同步 |
| C10 | RFC 4180 引号合规 | 全部合规 |

**特殊豁免**：`partial` 状态构造器（Container / ListContainer）不要求 C7 完整对应（按 §4.2 OBS-2 + ADR-011 ListContainer 返回原生 list）。

**报告**：`reports/<timestamp>_L4_report.json`（含 `summary{total,passed,failed}` + `checks[]`）。

---

## 4. 触发时机

设计 §3 定义 5 种触发时机（无 CI 服务器、无远程仓库依赖）：

| 触发时机 | L1 | L2 | L3 | L4 | 耗时 | 命令 |
|---------|----|----|----|----|------|------|
| **pre-commit（可选，默认不安装）** | ✅ | — | — | ✅ | 30s-2min | 安装后 git commit 自动触发 |
| **smoke-quick** | ✅ | — | — | ✅ | 30s-2min | `run_smoke.ps1 -Level "L1,L4"` |
| **smoke-func** | ✅ | ✅ | — | ✅ | 3-8min | `run_smoke.ps1 -Level "L1,L2,L4"` |
| **smoke-perf** | ✅ | — | ✅ | ✅ | 10-20min | `run_smoke.ps1 -Level "L1,L3,L4"` |
| **smoke 全量** | ✅ | ✅ | ✅ | ✅ | 15-30min | `run_smoke.ps1 -Level All` |
| **phase 验收（PM 触发）** | ✅ | ✅ | ✅ | ✅ | 15-30min | 同全量（PM 结合 `performance-gate SKILL Checkpoint 3` 判定） |

### 4.1 pre-commit hook（可选）

**默认不安装**（设计 §3.2 避免反模式）：
- 提供 `install_hook.ps1`，开发者主动选择是否启用
- 跑 L1+L4（30s-2min）；不跑 L2/L3（太慢会被绕过）
- 失败时阻断 commit（`exit 1`）；紧急情况可用 `git commit --no-verify` 绕过

**安装**：

```powershell
powershell -File testing/ci/install_hook.ps1
```

**卸载**：

```powershell
Remove-Item -LiteralPath .git/hooks/pre-commit
```

**重复安装幂等**：脚本检测已安装（通过 hook 头部标识）并提示，不会覆盖。

### 4.2 不采用 pre-push hook 的理由

本项目**禁止 git push**（`pm-extension.md §提交规范`），pre-push hook 永远不会触发（设计 §3.3）。定义它会造成"hook 存在但无效"的混淆。

### 4.3 不采用夜间定时的理由

本地开发环境非 24/7 运行；夜间定时需 CI 服务器，与"本地仓库"约束冲突（设计 §3.4）。性能 baseline 漂移检测已由 L3 的"同会话单次对比 + baseline diff"覆盖。

---

## 5. A/B Test 升级路径（L-09 对策工程化）

**L-09 教训**：跨时段性能对比的"消除法归因"在边界场景（Rust 侧 <300ns）失效。必须用 Controlled A/B Test（同会话交替测量 + 阳性对照 + 阴性对照）才能可靠归因。

**L3 工程化**（设计 §5.7）：
- L3 主跑出现 FAIL → 自动触发 A/B Test
- `ab_test/ab_harness.ps1` 通用化 `E01_O1_ab_harness.ps1`（参数化 toggle 文件 + base commit）
- `ab_test/ab_bench.py` 通用化 `E01_O1_ab_bench.py`（参数化场景 + role 字段保留阳性/阴性对照语义）
- `ab_test/ab_stats.py` 复用 `E01_O1_ab_stats.py`（welch_t + describe + classify_delta）

**判据**（与 `performance-gate SKILL Checkpoint 4` 一致）：
- `|Δx| > 0.5x → REGRESSION`（确认回归，阻断）
- `|Δx| < 0.3x → NOISE`（测量波动，不阻断）
- `0.3-0.5x → INTERMEDIATE`（加测 5 轮）

**应用实例**：
- 4.x O1-O4 调查（`experiments/phase4_4x_ab_*.py`）：a_err_eof A=9.82x / B=11.34x，Δ=+1.53x 显著生效
- E01 O1 调查（`experiments/E01_O1_ab_*`）：E01 边界场景根因分析

---

## 6. known_exemptions.json 维护说明

**位置**：`testing/ci/known_exemptions.json`（PM 主维护）

**Schema**：每条豁免含 `scenario_id`（与 perf-scenarios.csv `<constructor>:<scenario>:<direction>` 三元组对齐）/ `user_decision`（A/B/C）/ `phase` / `baseline_speedup` / `reason` / `action`（当前仅 `skip_fail_verdict`）/ `review_at` / `phase_5_todo`。

**当前 21 条目**：详见设计 §6.4。

**更新触发**（设计 §6.4 维护规则）：
1. 用户决策变更（如事项 A 重新解读）
2. 豁免场景实际达标（如 O1 后 StopIf B1 已 ≥10x，应从豁免中移除）
3. 新豁免场景出现（需 PM 显式声明 + 附用户决策依据）

**L3 只读**：L3 不会自动更新此文件。豁免项的 FAIL 仅降级为 WARN，仍在报告中可见。

**维护规则**：
- `scenario_id` 命名：`<constructor>:<scenario_id>:<direction>` 三元组（与 perf-scenarios.csv 一致）
- `baseline_speedup` 字段：从 perf-scenarios.csv 复制（L-02 对策：不凭记忆）
- PM 维护，每次 phase 验收时复审

---

## 7. 当前限制（重要）

### 7.1 L3 仅覆盖 5 场景

`run_l3_perf.py` 的 `SCENARIO_DEFS` 当前仅含 5 条目（4 quick + 1 非 quick），覆盖 Phase 4 代表性场景：
- `Index:i02-4.7:parse` / `Index:i01-4.7:build` / `GreedyRange:g01-4.7:parse` / `Index:e01-4.7:parse`（4 quick）
- `Array:A1:parse`（非 quick，边界场景）

其余 149 个 baseline 数据点标记为 `SKIPPED`（无 SCENARIO_DEF）。**全 151 场景映射为 Phase 5 follow-up**（设计 §6.4 已预留）。

**风险**：Phase 4 内非 SCENARIO_DEFS 场景（如 PrefixedArray / RepeatUntil）改动时 L3 无法检测。Phase 1/2/3 回归保护由 L1（`cargo test --lib`）+ L2（用户面 smoke）部分覆盖。

### 7.2 deprecated 脚本（v4 PyCallable）

`run_l2_functional.ps1` 调度矩阵中的 2 个 deprecated 脚本：

| Script | 原因 | 替代 |
|--------|------|------|
| `phase4_smoke_repeat_until.py` | v5 ADR-014 删除 PyCallable 路径，`RepeatUntil(lambda, ...)` 必然 CompilationError | `phase4_repeat_until_examples.py`（v5 21 用户面样例） |
| `v4_smoke_test.py` | v5 ADR-011 Container lib 下线，`from construct.lib.containers import Container` 必然 ModuleNotFoundError | 同上 |

deprecated 脚本仍跑（保留作历史证据），FAIL 仅报告不阻断 L2（设计 §1.1 B 类文档滞后于 v5 决策，本 README 同步修订）。

### 7.3 known_broken 脚本（Python 3.14 兼容性）

`vet_phase3_bits_integer.py` 标 `known_broken`：
- 不依赖 neoconstruct，仅用 Python 原版 construct
- `Bitwise(Bit).parse(b"\x80")` 在 Python 3.14 + construct 2.10.70 触发 `RestreamedBytesIO` 兼容性问题（上游 bug）
- 与 neoconstruct 无关，FAIL 仅报告不阻断 L2

**长期**：上游修复后可重测此脚本是否恢复 PASS。

### 7.4 pre-commit hook 默认不安装

设计 §3.2 决策：避免"hook 太慢被绕过"反模式。开发者主动 `install_hook.ps1` 才启用。

### 7.5 L4 C5/C6 仅检查行号存在

设计 §4.4 通过判据要求"读源文件确认行存在 + 内容非空"，实际实现仅检查行号存在（OBS-2）。影响极小（CSV 由 PM 维护）。PM 决策：保持现状（不扩展），Phase 5 follow-up 时若需严格化再补。

### 7.6 OBS-1：`-Level` 多值需加引号

PowerShell 5.1 native comma 是数组操作符。`-Level L1,L4`（未加引号）被解析为数组 → 空格连接 → `Resolve-Levels` 报错。

**正确写法**：始终加引号：`-Level "L1,L4"`（或用 `-Level All` / `-Level L1` / `-Level L4` 单值形态）。

### 7.7 OBS-3 / OBS-VET-4：裸 `Write-Host`（合理例外）

`run_l1_quality.ps1` L87 与 `run_l2_functional.ps1` L224 各有 1 处裸 `Write-Host $tailPreview`（输出多行 stderr 预览）。`Write-CiLog` 设计为单行 + 前缀，无法承载多行内容。属合理例外（1a/1b VET 均接受）。

### 7.8 ab_harness.ps1 注释声明（OBS-4）

`ab_harness.ps1:17` 注释声明"全 ASCII 注释"，实际含中文。因文件 UTF-8 with BOM，PowerShell 5.1 能正确识别，不影响功能。

---

## 8. 报告路径 + JSON Schema 说明

### 8.1 报告路径

所有报告输出到 `testing/ci/reports/`（`.gitignore` 排除）：

| 文件 | 内容 |
|------|------|
| `<timestamp>_L1_report.json` | L1 五步序列逐项 + 总体 passed |
| `<timestamp>_L2_report.json` | L2 12 脚本逐项 + summary + venv 路径 |
| `<timestamp>_L3_report.json` | L3 verdicts 列表 + summary + environment + A/B Test 触发情况 |
| `<timestamp>_L3_report.md` | L3 人工审查 Markdown 报告（FAIL/WARN/IMPROVED/NEW 明细表） |
| `<timestamp>_L4_report.json` | L4 10 项 check 逐项 + summary |
| `<timestamp>_smoke_summary.json` | `run_smoke.ps1` 整体汇总（levels × passed × exit_code × report_path） |
| `ab_<constructor>_<scenario>_<direction>_<ts>.json` | Controlled A/B Test 独立报告（每个 FAIL 场景一份） |

### 8.2 L3 报告 JSON Schema 摘要

详见设计 §5.5。关键字段：

```json
{
  "run_id": "l3_<timestamp>",
  "triggered_by": "manual",
  "mode": "perf" | "perf+quick",
  "environment": {
    "python_version": "3.14.2",
    "construct_py_version": "2.10.70",
    "neoconstruct_commit": "<short hash>",
    "l_09_disclaimer": "跨时段性能对比在边界场景（Rust<300ns）失效..."
  },
  "summary": { "total": 154, "pass": N, "warn": N, "fail": N, "new": N, "improved": N, "skipped": N },
  "results": [
    { "key": "Array:A1:parse", "category": "normal|boundary|special|...", "verdict": "PASS|WARN|FAIL|...", "delta_x": ..., "delta_pct": ..., ... }
  ],
  "ab_test_triggered": [...],
  "ab_test_reports": [...]
}
```

---

## 9. 文件清单

```
testing/ci/
├── README.md                 # 本文件
├── install_hook.ps1          # pre-commit hook 安装脚本（1d）
├── pre-commit.template       # pre-commit hook 模板（1d，安装时复制到 .git/hooks/pre-commit）
├── lib_smoke.ps1             # 共享函数（日志/路径/子进程/报告，1a）
├── run_smoke.ps1             # 主入口（-Level 调度 L1-L4，1a/1b/1c 迭代扩展）
├── run_l1_quality.ps1        # L1 质量门禁（1a）
├── run_l2_functional.ps1     # L2 功能门禁（1b，含 phase{1,2}_smoke.py 调度）
├── run_l3_perf.ps1           # L3 性能门禁 PowerShell 编排（1c，forward 到 run_l3_perf.py）
├── run_l3_perf.py            # L3 性能门禁 Python 主调度（1c）
├── run_l4_consistency.py     # L4 一致性核查（1a）
├── known_exemptions.json     # L3 性能豁免清单（21 条目，PM 主维护，1c）
├── ab_test/                  # Controlled A/B Test 通用化工具（1c）
│   ├── ab_harness.ps1        # 通用化 E01_O1_ab_harness.ps1
│   ├── ab_bench.py           # 通用化 E01_O1_ab_bench.py
│   └── ab_stats.py           # 复用 E01_O1_ab_stats.py
├── test_l3_classify.py       # L3 判据单元测试（1c，15 case）
├── test_ab_stats.py          # ab_stats 单元测试（1c，6 case）
└── reports/                  # 报告输出目录（.gitkeep 占位）
```

外部依赖（smoke 矩阵与 bench 原版）：
- `experiments/phase{1,2}_smoke.py`（1b 新建，用户面 smoke）
- `experiments/phase{4,32,33}_*.py` / `vet_phase3_*.py`（既有脚本，L2 调度）
- `experiments/phase4_bench_*.py`（既有 bench，L3 计划复用，全场景映射为 Phase 5 follow-up）
- `experiments/E01_O1_ab_*.py`（既有 A/B Test 工具，L3 ab_test/ 通用化基础）

---

## 10. 相关文档

- 设计：[`docs/design/基础设施/CI冒烟门禁设计.md`](../../docs/design/基础设施/CI冒烟门禁设计.md)（§0-§8 + 附录 A/B，4 层门禁完整规范）
- L-09 教训：`harness/experiences.md §L-09`（跨时段性能对比消除法归因失效）
- 性能门禁 SKILL：`.opencode/skills/performance-gate/SKILL.md`（Checkpoint 4 = L-09 工程化判据）
- 构造器清单：[`docs/constructors-inventory.csv`](../../docs/constructors-inventory.csv)（L2/L4 单一事实源，PM 主维护）
- 性能 baseline：[`docs/perf-scenarios.csv`](../../docs/perf-scenarios.csv)（L3 baseline，PM 主维护）
- 质量门禁定义：`AGENTS.md §3` + `developer.md §自检清单`（L1 命令序列规范来源）

---

> **META-CI-1d** 子任务交付。本 README 同步处理 1a OBS-1/2/3 + 1b F.1-F.4 + OBS-VET-1~5 + 1c 5 个观察项 + D.1/D.2 文档修订批次。
