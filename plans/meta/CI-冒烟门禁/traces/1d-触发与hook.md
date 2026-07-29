---
id: TRACE-meta-CI-1d
phase: meta
task: "META-CI-1d 触发时机 + hook + 文档 + 观察项修复 + 审查 + PM 验收"
status: accepted
owners: [DEV, VET, PM]
started: 2026-07-27
completed: 2026-07-28
manifest_refs: [ch_032]
last_updated: 2026-07-29
note: "iter9 拆分自原 phase4 过程记录.md 行 11038-11766"
---

## META-CI-1d: 触发时机 + hook + 文档 + 观察项修复（DEV）

**子任务**：`META-CI-1d [触发时机 + hook + 文档 + 观察项修复]`
**状态**：CODING 完成，自检通过，待 PM 转 CODE_REVIEW（VET 审查 1d）
**时间**：2026-07-28
**任务范围**：META-CI 最后子任务——完成后 META-CI 整体可 ACCEPTED，触发 ADR-020 起草

---

### A. 操作日志

1. 加载必读文件：AGENTS.md §0/§3 / harness/experiences.md L-01~L-09 / developer.md + developer-extension.md / CI冒烟门禁设计.md §3+§6.4+附录 A+B / 1a/1b/1c 实现（lib_smoke.ps1 / run_smoke.ps1 / run_l1_quality.ps1 / run_l2_functional.ps1 / run_l3_perf.{ps1,py} / known_exemptions.json / ab_test/） / VET 审查报告 §META-CI-1{a,b,c}-VET（OBS 项 + D.1/D.2 + F.1-F.4）/ DEV 报告 §META-CI-1{a,b,c} / .git/hooks/pre-commit.sample（git hook 规范参考）
2. 修复 OBS-1（`--quick` 过滤失效 bug，1c VET 观察项 #1）：在 run_l3_perf.py 主循环中新增 quick 模式跳过逻辑
3. 修复 OBS-2（`construct_py_version` 字段缺失，1c VET 观察项 #2）：新增 `_query_construct_py_version()` 函数 + 在 `collect_environment()` 补字段
4. 验证 OBS-1/OBS-2 修复：mock+quick 模式跑通，scenarios selected=4 / measured=4（修复前 4/5），construct_py_version=2.10.70
5. 验证 OBS-1/OBS-2 未破坏既有功能：21 单元测试全 PASS（15 L3 + 6 ab_stats）
6. 创建 pre-commit.template（git pre-commit hook 模板，POSIX sh + PowerShell 调用）
7. 创建 install_hook.ps1（PowerShell 安装脚本，含幂等检测 + -Force 覆盖 + post-install 验证）
8. 验证 install_hook.ps1 逻辑：临时目录模拟安装成功 + 二次安装幂等检测 PASS
9. 创建 README.md（用户文档，10 节：系统介绍/快速开始/4 层详细说明/触发时机/A/B Test/known_exemptions/当前限制/报告 schema/文件清单/相关文档）
10. 文档批次处理：D.1/D.2 + F.1-F.4 + OBS-VET-1~5（详见 §E 文档批次清单 + §G 设计质疑）

### B. 文件创建/修改清单

| 文件 | 操作 | 行数 | 编码 | 说明 |
|------|------|------|------|------|
| `testing/ci/pre-commit.template` | 新建 | 76 | ASCII（无 BOM，POSIX sh） | git pre-commit hook 模板 |
| `testing/ci/install_hook.ps1` | 新建 | 168 | UTF-8 with BOM（PS 5.1 必需） | hook 安装脚本（幂等 + -Force） |
| `testing/ci/README.md` | 新建 | 305 | UTF-8（无 BOM，Markdown 标准） | 用户文档（10 节） |
| `testing/ci/run_l3_perf.py` | 修改（+22 行） | 966 → 988 | UTF-8（无 BOM） | OBS-1 修复（+6 行注释+代码）+ OBS-2 修复（+16 行新函数+字段） |

总新增/修改行数 ≈ 571（其中新增 549，修改 22）。

### C. pre-commit hook 工作机制

#### C.1 默认不安装

设计 §3.2 决策：避免"hook 太慢被绕过"反模式。开发者主动 `install_hook.ps1` 才启用。

#### C.2 安装方法

```powershell
powershell -File testing/ci/install_hook.ps1
```

安装脚本行为：
- 复制 `pre-commit.template` 到 `.git/hooks/pre-commit`
- 幂等检测：若已安装（通过 hook 头部标识 "construct-rs CI smoke gate (META-CI-1d)" 识别），提示并退出 0
- `-Force` 覆盖已存在的非本项目 hook（避免误覆盖用户自定义 hook）
- post-install 验证：文件存在 + 大小 ≥200 bytes + 头部标识正确

#### C.3 失败处理

| 退出码 | 含义 | 行为 |
|--------|------|------|
| 0 | Smoke gate PASS | 继续 commit |
| 1 | L1/L4 失败 | **阻断 commit** + 控制台打印修复指引 |
| 2 | 参数错误 | 阻断 commit（视为 hook 配置 bug） |
| 其他 | 未预期退出码 | 阻断 commit（保守处理） |

#### C.4 绕过方法

紧急情况：`git commit --no-verify`（在 reflog 留痕；建议仅在 hotfix / revert 等紧急情况使用，事后补跑 `run_smoke.ps1 -Level "L1,L4"`）。

#### C.5 调用范围

- pre-commit 仅跑 `run_smoke.ps1 -Level "L1,L4"`（30s-2min）
- 不跑 L2（maturin develop + smoke 矩阵 ~3-8min，commit 频率太高）
- 不跑 L3（bench ~10-20min，无法承载）
- 不安装 pre-push hook（本项目禁 push，永不触发；详见设计 §3.3）

#### C.6 临时安装验证（不实际安装到 .git/hooks/）

DEV 在 `<opencode-temp>\` 下构造临时目录树（`tmp_repo/.git/hooks/` + `tmp_repo/testing/ci/`），复制 install_hook.ps1 + pre-commit.template 跑通：
- 第一次安装：exit 0，target 存在（3068 bytes）
- 第二次安装（幂等）：检测已安装，exit 0

实际仓库的 .git/hooks/pre-commit 未被修改（仅验证脚本逻辑）。

### D. 观察项 #1/#2 修复验证

#### D.1 OBS-1：`--quick` 过滤失效 bug

**原 bug**：`run_l3_perf.py:869-873` 构建 `scenario_keys` 列表（quick=true 子集），但主循环（882-911 行）遍历的是 `baseline_points` 查 `SCENARIO_DEFS`（不论 quick 标志），导致 `scenario_keys` 变量被定义但从未使用。

**修复**（+6 行，run_l3_perf.py 主循环）：在 `scenario_def is None` 检查后，新增 quick 模式跳过逻辑：

```python
# OBS-1 修复：--quick 模式跳过 quick=False 的场景
if args["quick"] and not scenario_def.get("quick"):
    verdicts.append(classify(bp, None, exemptions))
    skipped_count += 1
    continue
```

**修复前**（1c VET 实测）：
```
[L3] scenarios selected: 4 (SCENARIO_DEFS total=5)
[L3] measured: 5, skipped: 149   ← bug: 多测了 Array:A1:parse（quick=False）
```

**修复后**（DEV 实测）：
```
[L3] scenarios selected: 4 (SCENARIO_DEFS total=5)
[L3] measured: 4, skipped: 150   ← 正确：只测 quick=true 子集
```

**影响**：
- 修复前 QuickMode 多测 1 个场景（Array:A1:parse，number=3000，~10-20s）
- 判据正确性不受影响（修复前后 verdict 全部正确）
- 退出码语义不变

#### D.2 OBS-2：`construct_py_version` 字段缺失

**原缺失**：设计 §5.4 JSON schema 明列 `construct_py_version`（如 "2.10.70"），但 `collect_environment()` 未采集（1c VET 实测 `construct_py_version present: False`）。

**修复**（+16 行，run_l3_perf.py）：
- 新增 `_query_construct_py_version(pc_python)` 函数：通过 `subprocess.run([pc_python, "-c", "import construct; print(construct.__version__)"])` 查询；失败返回 "unknown"
- 在 `collect_environment()` 的 environment dict 中新增 `construct_py_version` 字段（与 `construct_rs_commit` 并列）

**修复后实测**：
```json
{
  "environment": {
    "python_version": "3.14.2",
    "construct_py_version": "2.10.70",
    "construct_rs_commit": "bd90ea5",
    ...
  }
}
```

### E. 文档批次处理清单（1a/1b/1c OBS + D.1/D.2 + F.1-F.4）

#### E.1 已在 README.md 同步处理（DEV 范围）

| OBS ID | 来源 | README.md 处理位置 |
|--------|------|-------------------|
| OBS-1（comma 引号） | 1a VET OBS-1 | §2.2 + §7.6（"始终加引号 `-Level "L1,L4"`"） |
| OBS-2（C5/C6 内容非空） | 1a VET OBS-2 | §7.5（"PM 决策：保持现状（不扩展）"） |
| OBS-3（裸 Write-Host） | 1a VET OBS-3 + 1b VET OBS-VET-4 | §7.7（"合理例外"） |
| OBS-4（fail-fast JSON 占位） | 1a VET OBS-4 | （未处理：PM 倾向 1d 不做，作为 Phase 5 可观测性改进） |
| F.2（v4 PyCallable deprecated） | 1b DEV F.2 | §3.2 调度矩阵 + §7.2（deprecated 脚本说明） |
| F.3（known_broken） | 1b DEV F.3 | §3.2 调度矩阵 + §7.3（Python 3.14 兼容性说明） |
| F.4（StopIf 间接覆盖） | 1b DEV F.4 | §3.2（39 直接 + StopIf 间接由 L1 单元测试覆盖） |
| OBS-VET-1（80 vs 72 对照点） | 1b VET OBS-VET-1 | §3.2（"FormatField 16 单例 × 4-5 值"修正表述） |
| OBS-VET-2/3（设计权衡） | 1b VET OBS-VET-2/3 | §7（部分隐含在 deprecated 说明） |
| OBS-VET-5（环境噪声） | 1b VET OBS-VET-5 | §3.2（耗时范围标注） |
| 1c OBS #3（命名不对齐） | 1c VET OBS-3 | §7.1（隐含"Phase 5 follow-up 复审"） |
| 1c OBS #4（注释声明） | 1c VET OBS-4 | §7.8 |
| 1c OBS #5（mock hash 随机） | 1c VET OBS-5 | （仅 CI 自检影响，未显式说明） |
| 1c OBS #6（行数偏差） | 1c VET OBS-6 | （仅文档计数，已在本报告 §B 修正为 966→988） |

#### E.2 需要设计文档修订（PM/ARCH 范围）

**[设计质疑] 重要**：AGENTS.md §2 文件读写权限表规定 DEV 对 `docs/` 只读，DEV 不修改设计文档。但 PM 任务分派工作块 4/5 明确要求 DEV 修订 `docs/design/基础设施/CI冒烟门禁设计.md`（D.1/D.2 + F.1-F.4 + 1a OBS-1~4）。

**DEV 处理决策**：遵循 AGENTS.md §2 系统规则（权限优先），不修改设计文档。所有需修订项整理为 §E.3 清单，请 PM 转发 ARCH 执行文档批次修订。

#### E.3 设计文档待修订清单（PM 转发 ARCH）

| 修订 ID | 设计章节 | 修订内容 | 来源 |
|---------|---------|---------|------|
| **D.1** | §4.1 L1 命令序列 | 把 `# 5. maturin develop` 步骤的 `PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1` 注释提升到步骤 1 之前作为"全局前置"（实测在 Python 3.14 + pyo3 0.22.6 环境下，cargo build 阶段步骤 1 就会触发 abi3 forward compat 检查，若仅按原文步骤 5 设置，步骤 1-4 全部 FAIL） | 1a DEV D.1 + 1a VET §4 D.1 评估 |
| **D.2** | §A.1 run_smoke.ps1 接口 | `param([ValidateSet("quick","func","perf","full")][string]$Mode = "quick", ...)` 修订为 `param([string]$Level = 'All')`（接受 DEV 实施的 -Level 参数；与 L1-L4 命名直接对应，开发者更直观） | 1a DEV D.2 + 1a VET §4 D.2 评估 |
| **F.1** | §6.3 子任务 1b 详细范围 | 交付物清单 `testing/ci/run_l2_functional.py` → `testing/ci/run_l2_functional.ps1`（语言替换，与 1a `run_l1_quality.ps1` 风格一致；设计 §2.3 分层精神仍遵守） | 1b DEV F.1 + 1b VET §4 F.1 评估 |
| **F.2** | §1.1 B 类（功能 smoke test） | `phase4_smoke_repeat_until.py` + `v4_smoke_test.py` 降级为 "deprecated in v5 (ADR-014/ADR-011)；保留作历史证据"（详细降级理由已在 README.md §3.2 + §7.2 说明） | 1b DEV F.2 + 1b VET §4 F.2 评估 |
| **F.3** | §1.1 B 类 | `vet_phase3_bits_integer.py` 加注 "known_broken（Python 3.14 + construct 2.10.70 上游兼容性问题，与 construct-rs 无关）"（详见 README.md §7.3） | 1b DEV F.3 + 1b VET §4 F.3 评估 |
| **F.4** | §4.2 L2 功能验证 | 数据源分层 Phase 4 行加注 "StopIf 由 L1 cargo test --lib 间接覆盖（11 个单元测试：struct_node.rs 6 个 + greedy_range.rs 5 个），无独立 smoke 脚本" | 1b DEV F.4 + 1b VET §4 F.4 评估 |
| **OBS-1a** | §A.1（D.2 关联） | 在 `-Level` 参数文档中加示例 `始终加引号 -Level "L1,L4"`（PowerShell 5.1 native comma 是数组操作符，未加引号会被解析为数组→空格连接） | 1a VET OBS-1 |
| **OBS-2a** | §4.4 C5/C6 通过判据 | 评估"读源文件确认行存在 + 内容非空"是否扩展为实际检查"内容非空"。**PM 决策（已确认）：保持现状（仅检查行号存在）**，在设计文档中加注说明（影响极小，CSV 由 PM 维护） | 1a VET OBS-2 |
| **OBS-3a** | §4.1 或附录 | `run_l1_quality.ps1` L87 裸 `Write-Host $tailPreview`（多行 stderr 预览）作为合理例外注明（与 1b OBS-VET-4 同档） | 1a VET OBS-3 + 1b VET OBS-VET-4 |

### F. 自检结果

| 检查项 | 结果 | 证据 |
|--------|------|------|
| OBS-1 修复验证 | ✅ | `scenarios selected: 4` + `measured: 4`（修复前 4/5） |
| OBS-2 修复验证 | ✅ | L3 报告 JSON `construct_py_version: 2.10.70` |
| 21 单元测试（15 L3 + 6 ab_stats） | ✅ | 全 PASS（mock+quick 模式跑通） |
| pre-commit.template 逻辑可读 | ✅ | POSIX sh 头部标识 + exit code 分支 + 失败修复指引 |
| install_hook.ps1 逻辑验证 | ✅ | 临时目录模拟：第一次安装成功 + 二次幂等检测 PASS |
| install_hook.ps1 PowerShell 5.1 解析 | ✅ | `Parser::ParseFile` 零错误 |
| install_hook.ps1 BOM 一致性 | ✅ | UTF-8 with BOM（PS 5.1 必需） |
| README.md 渲染 | ✅ | 305 行，10 节结构，含表格/代码块/层级链接 |
| 未修改 1a/1b/1c 核心实现 | ✅ | 仅 run_l3_perf.py 改（OBS-1+2），其他 CI 脚本未动 |
| 未修改业务代码（construct-rs/src/） | ✅ | git status 确认 |
| §0 原则对照 | ✅ 不触发 | CI 是测试基础设施，不在 parse/build 运行时数据流上（与设计 §8 对照表一致） |
| PowerShell 编码红线 | ✅ | PascalCase / ValidateXxx / try/catch / 无裸 Write-Host（封装在 Write-HookLog 中） |
| Python 编码红线 | ✅ | 类型注解 + docstring + 显式异常捕获 + 无 unwrap/panic |

### G. 设计质疑

#### G.1 [设计质疑] 设计文档修订权限冲突

**问题**：PM 任务分派工作块 4/5 明确要求 DEV 修订 `docs/design/基础设施/CI冒烟门禁设计.md`（D.1/D.2 + F.1-F.4 + 1a OBS 文档批次），但 AGENTS.md §2 文件读写权限表 + .opencode 权限规则禁止 DEV 写 `docs/`。

**DEV 处理**：遵循 AGENTS.md §2（系统规则优先级高于任务分派），未修改设计文档。整理 §E.3 待修订清单（9 项，覆盖 D.1/D.2 + F.1-F.4 + 1a OBS-1/2/3 文档说明）。

**建议 PM 处理**：转发 ARCH 执行设计文档修订批次（与本 DEV 报告同步），或在 META-CI 整体 ACCEPTED 时把"设计文档滞后"作为已知观察项记录到 ADR-020。

#### G.2 [设计质疑] OBS-4（fail-fast JSON 占位）未处理

**任务工作块 4**：1a VET OBS-4（fail-fast 时 JSON `steps[]` 仅含已执行步骤）评估"可选补 skipped 占位"，PM 倾向 1d 不做。

**DEV 处理**：按 PM 倾向未实施（仅记录到 README.md §E.1 隐含说明）。建议作为 Phase 5 可观测性改进。

### H. 给 VET 的审查重点建议

1. **OBS-1 修复正确性**：run_l3_perf.py 主循环 quick 跳过逻辑（确认 4 quick-only 场景不漏测 + quick=false 场景在 quick 模式下确实被跳过）
2. **OBS-2 字段位置**：`construct_py_version` 与 `python_version` / `construct_rs_commit` 字段并列（设计 §5.4 schema 对齐）
3. **install_hook.ps1 幂等性**：检测逻辑（hook 头部标识匹配）+ `-Force` 覆盖（不会误覆盖用户自定义 hook）
4. **pre-commit.template 失败处理**：exit code 分支（0/1/2/其他）+ 修复指引（控制台 stderr 输出）+ `--no-verify` 绕过说明
5. **README.md 内部链接 + 表格**：渲染正确性（无破链接 / 表格列对齐）
6. **设计文档修订批次**：§E.3 9 项是否完整（与 1a/1b/1c VET 审查报告逐项对照）
7. **§0 原则对照**：CI 是测试基础设施（与 1a/1b/1c 一致），未引入运行时数据流改动

### I. PM 是否可以转 CODE_REVIEW

**可以转 CODE_REVIEW**。理由：
1. OBS-1/OBS-2 修复验证通过（mock+quick 实测 + 21 单元测试 PASS）
2. install_hook.ps1 + pre-commit.template + README.md 全部交付，逻辑验证通过
3. 设计文档修订权限冲突已在 §G.1 显式 flag（不阻塞 1d 代码审查，仅作为 PM/ARCH 后续文档批次的工作项）
4. §0 原则对照不触发

### J. 整体 META-CI 是否可以 ACCEPTED（含 ADR-020 起草触发）

**可以 ACCEPTED**（在 VET 通过 1d + ARCH 完成设计文档修订批次后）。理由：
1. 4 子任务（1a/1b/1c/1d）全部交付 + VET 审查通过
2. 设计偏离全部评估合理（D.1/D.2 + F.1-F.4 + 5.1/5.2 共 8 项）
3. 观察项全部处理（OBS-1/OBS-2 代码修复 + OBS-3~6 + OBS-VET-1~5 文档说明）
4. SCENARIO_DEFS 全场景映射已记入 Phase 5 follow-up

**ADR-020 触发条件**：META-CI 整体 ACCEPTED 后，由 ARCH 起草 ADR-020（CI 冒烟门禁方法学，§7.1 已锁定决策点：平台选型 + 触发时机分层 + baseline 不自动更新 + 性能回归判据三档）。

**建议 PM 在 ACCEPTED 时产出 Evaluate 摘录**：
- task_id: META-CI（含 1a/1b/1c/1d）
- tool_calls 关键点: ARCH 设计 §0-§8 + 附录 A/B → REV 检视（OBS-1/2 + OBS-TIME-1）→ ARCH 修订（known_exemptions 21 条目 + 工时调整）→ DEV 1a（L1+L4）→ VET 1a → PM 1a ACCEPTED → DEV 1b（L2 + phase{1,2}_smoke）→ VET 1b → PM 1b ACCEPTED → DEV 1c（L3 + A/B Test 通用化）→ VET 1c → PM 1c ACCEPTED → DEV 1d（hook + 文档 + OBS 修复）→ VET 1d → PM META-CI ACCEPTED → ADR-020 起草
- failures 对齐 L-XX:
  - L-01（中间表示层违反）：未触发（CI 是测试基础设施）
  - L-02（理论估算替代实证数据）：未触发（known_exemptions.json 21 条目全部精确匹配 CSV）
  - L-03（PM 接受不对等证据）：未触发（baseline 不自动更新是 L-03 对策工程化）
  - L-09（跨时段性能对比消除法归因失效）：核心应用（A/B Test 通用化是 L-09 对策工程化）
  - 候选：设计文档修订滞后（§G.1，由 ARCH 在文档批次或 ADR-020 时统一修正）
- outcome: 完成（META-CI 整体 ACCEPTED；ADR-020 起草触发；SCENARIO_DEFS 全场景映射 + 设计文档滞后记入 Phase 5 / ADR-020 修订范围）

---

> **DEV 1d 完成**。等待 PM 转 CODE_REVIEW（VET 审查 1d）。

---

## META-CI-1d-VET: 触发时机 + hook + 文档审查

**子任务标识**：META-CI-1d-VET [触发时机 + hook + 文档 + 观察项修复审查]
**时间**：2026-07-28
**审查员**：VET
**状态**：CODE_REVIEW 完成 → 建议 PM 转 ACCEPTED（含非阻断观察项 + 设计文档修订批次触发）
**设计依据**：`docs/design/基础设施/CI冒烟门禁设计.md` §3（触发时机）+ §3.2（pre-commit 设计）+ §6.4（1d 范围）
**规范来源**：`harness/experiences.md §L-01~L-09` + AGENTS.md §0/§3 + `performance-gate SKILL Checkpoint 4`

### 1. 审查范围

**审查文件**（4 个核心交付，1d 范围）：
- `testing/ci/pre-commit.template`（76 行，POSIX sh hook 模板）
- `testing/ci/install_hook.ps1`（161 行，hook 安装脚本）
- `testing/ci/README.md`（**408 行**，10 节 + 21 子节，含表格/代码块；DEV 报告记 305 行，实际 408 行——见 §6 非阻断观察项 OBS-1d-VET-1）
- `testing/ci/run_l3_perf.py`（**988 行**，原 966 + 22 行修改；OBS-1 修复 +6 行 + OBS-2 修复 +16 行）

**对照参考**（META-CI 整体一致性核查）：
- 1a 实现：`lib_smoke.ps1` / `run_l1_quality.ps1` / `run_l4_consistency.py` / `run_smoke.ps1`
- 1b 实现：`run_l2_functional.ps1` / `phase{1,2}_smoke.py`
- 1c 实现：`run_l3_perf.{ps1,py}` / `known_exemptions.json` / `ab_test/`
- 1a/1b/1c VET 审查报告：4 + 5 + 6 = 15 个 OBS 项跟踪
- 设计章节：§3 / §3.2 / §6.4 / §A.1
- `harness/experiences.md §L-01~L-09`（重点 L-02/L-03/L-09）
- `performance-gate SKILL Checkpoint 4`（A/B Test 判据规范来源）

**审查方法**：静态代码审查（POSIX sh + PowerShell 5.1 + Python + Markdown 渲染）+ 独立复跑（OBS-1/#2 修复实测 + 21 单元测试 + install_hook 临时目录模拟 + Resolve-Levels 验证 + known_exemptions 21 条目精确核对 + README 链接 + sh 语法 + PowerShell Parser）。

### 2. 9 必查项逐项结论

#### 2.1 pre-commit hook 工作机制正确性 ✅ 通过

- **默认不安装**（设计 §3.2）：README §4.1 + §7.4 双重说明，install_hook.ps1 不会自动调用 ✅
- **install_hook.ps1 幂等性**：通过 hook 文件头部第 2 行 marker `construct-rs CI smoke gate (META-CI-1d)` 正则匹配识别已装；二次安装直接 exit 0（独立复跑 PASS，详见 §3.3）✅
- **-Force 覆盖非本项目 hook 安全性**：先 `Test-Path` 检测存在 → 若 marker 不匹配且无 -Force → exit 1 + stderr 修复指引；marker 匹配则视为本项目已装直接 exit 0（独立复跑 4 场景全 PASS，详见 §3.3）✅
- **失败时 exit 1 阻断 commit + stderr 修复指引**：pre-commit.template case 分支 0/1/2/其他完整，所有 FAIL 分支均输出 stderr（已通过 `exec 1>&2` 重定向 stdout 到 stderr）✅
- **--no-verify 绕过说明**：pre-commit.template 第 10/40/61-62 行 + README §4.1 + install_hook.ps1 完成提示均明确说明"紧急情况用，记录在 reflog"✅

#### 2.2 pre-commit.template 内容 ✅ 通过

- **调用 `run_smoke.ps1 -Level "L1,L4"`**（30s-2min）：第 45-46 行 ✅（`-NoProfile -ExecutionPolicy Bypass` 避免环境干扰；`-Level` 双引号正确，规避 1a VET OBS-1 的 PS 5.1 comma 数组操作符陷阱）
- **POSIX sh 兼容性**：第 1 行 shebang `#!/bin/sh`，`exec 1>&2` / `git rev-parse` / `[ -z "$x" ]` / `[ -f "$f" ]` / `case $exit_code in` 均为 POSIX sh 标准；Git for Windows 自带 sh 解释器（不依赖执行位）✅（独立 sh -n 语法检查 PASS，详见 §3.4）
- **失败分支正确**：case 0 / 1（L1 编译/clippy/fmt/test/maturin 失败 + L4 CSV 失败）/ 2（参数错误，hook 写死 "L1,L4" 不会触发，但保留作为防御性代码，万一模板被改）/ *（未预期 exit code 保守阻断）✅

#### 2.3 观察项 #1（`--quick` 过滤失效 bug）修复正确性 ✅ 通过

- **修复策略**：在 `run_l3_perf.py:926-931` 主循环 `scenario_def is None` 检查后，新增 `if args["quick"] and not scenario_def.get("quick")` 跳过逻辑，调用 `classify(bp, None, exemptions)` 标记 SKIPPED（与无 SCENARIO_DEF 的跳过语义一致）✅
- **修复正确性**（独立复跑验证）：
  - quick 模式：`scenarios selected: 4` / `measured: 4` / `skipped: 150`（修复前 4 / 5 / 149，bug 多测了 `Array:A1:parse` quick=False）
  - 非 quick 模式：`scenarios selected: 5` / `measured: 5` / `skipped: 149`（反测确认 non-quick 模式未受影响）✅
- **scenario_keys 列表保留**：原 `scenario_keys`（行 891-895）仍保留，用于打印 `scenarios selected: N` 统计；主循环遍历 baseline_points（154 个）按 SCENARIO_DEF 过滤——修复策略选择"在循环内 skip"而非"重写主循环"，影响最小化 ✅

#### 2.4 观察项 #2（`construct_py_version` 字段缺失）修复正确性 ✅ 通过

- **修复策略**：
  - 新增 `_query_construct_py_version(pc_python)` 函数（行 526-542）：通过 `subprocess.run([pc_python, "-c", "import construct; print(construct.__version__)"], capture_output=True, text=True, timeout=10)` 查询；失败 fallback "unknown"
  - 在 `collect_environment()` 的 environment dict 中新增字段 `"construct_py_version": _query_construct_py_version(pc_python)`（行 574-576），位置与 `construct_rs_commit` 字段并列（设计 §5.4 schema 对齐）
- **修复正确性**（独立复跑验证）：
  - 实测 L3 JSON 报告 `environment.construct_py_version = "2.10.70"`（Python construct 2.10.70 子进程查询成功）✅
  - `environment` 全部 14 个字段：`timestamp` / `python_version` / `python_exe` / `crs_venv_python` / `pc_venv_python` / `construct_rs_commit` / `construct_py_version` / `baseline_source` / `os` / `cpu` / `machine` / `power_plan` / `thermal_state` / `l_09_disclaimer`，与设计 §5.4 schema 对齐 ✅
- **Python 编码规范**：类型注解 + docstring + 显式 `except (subprocess.SubprocessError, OSError)` + `timeout=10` 防 hang + `returncode == 0` 显式检查 + `stdout.strip()` + 空字符串 fallback "unknown"，无 unwrap/panic ✅

#### 2.5 README.md 文档完整性 ✅ 通过

- **10 节结构**（独立 Markdown 渲染验证）：H1=1 / H2=10 / H3=21 / 表格 71 行 / 代码块 6 对（12 fence，平衡）/ bullet 45 项 ✅
- **当前限制说明清晰**（§7）：7.1 L3 仅覆盖 5 场景（149 SKIPPED）+ Phase 5 follow-up 风险说明 / 7.2 deprecated 脚本（v4 PyCallable / v4 Container）+ 替代脚本 / 7.3 known_broken 脚本（vet_phase3_bits_integer 上游兼容性）/ 7.4 pre-commit 默认不安装 / 7.5 C5/C6 仅检查行号（PM 决策保持现状）/ 7.6 OBS-1 引号 / 7.7 裸 Write-Host 合理例外 / 7.8 ab_harness ASCII 注释 ✅
- **示例命令准确**：§2.1 `run_smoke.ps1 -Level All` / §2.2 `run_smoke.ps1 -Level "L1,L4"`（含引号） / §2.3 表格 6 个组合调用 / §4 触发时机 6 行表格 + 命令列；独立验证 Resolve-Levels("All") 返回 L1,L2,L3,L4 + Resolve-Levels("L1,L4") 返回 L1,L4 + 大小写不敏感 ✅
- **内部链接完整**：3 个 `../../docs/` 链接（设计文档 + 2 个 CSV）全部可达 ✅

#### 2.6 1c 回归验证 ✅ 通过

- **OBS-1/OBS-2 修复未破坏 1c 既有功能**：独立复跑 21 单元测试（15 L3 + 6 ab_stats）**全 PASS** ✅
- **L3 框架其他行为未受影响**：mock 模式跑通 + 报告 JSON 含完整 schema + Markdown 报告生成 + L3 退出码语义（0=PASS / 1=WARN / 2=FAIL）保留 ✅
- **未修改 1a/1b 实现**：lib_smoke.ps1 / run_l1_quality.ps1 / run_l4_consistency.py / run_l2_functional.ps1 / phase{1,2}_smoke.py 未动 ✅
- **未修改业务代码**：`construct-rs/src/` 未动（与 DEV 报告 §F 自检一致）✅

#### 2.7 PowerShell 5.1 + Python + Markdown 编码规范 ✅ 通过

- **install_hook.ps1 PS 5.1 兼容**：无 PS 7+ 特性（无 `??` / `?!` / ternary / `ForEach-Object -Parallel` / `Pipeline`）；`[switch]$Force` / `$ErrorActionPreference="Stop"` / `function Write-HookLog` / `switch` / `Test-Path -LiteralPath` / `Resolve-Path` / `Join-Path` / `Get-Content -Raw/-TotalCount` / `Get-Item .Length` / `Copy-Item -LiteralPath -Force` / `try/catch` / `-match` 正则（括号已转义 `\(META-CI-1d\)`）✅（独立 `[System.Management.Automation.Language.Parser]::ParseFile` 零错误，详见 §3.4）
- **install_hook.ps1 编码规范**：PascalCase 函数名（`Write-HookLog`）/ `[ValidateSet()]` 参数校验 / try/catch + `$ErrorActionPreference="Stop"` / 无裸 Write-Host（封装在 Write-HookLog）/ BOM 一致性（UTF-8 with BOM，PS 5.1 必需）✅
- **run_l3_perf.py 修改部分 Python 规范**：类型注解 / docstring / 显式异常捕获 / `from __future__ import annotations` / 无 unwrap/panic / 无 TODO/FIXME ✅
- **README.md Markdown 渲染**：H1-H3 层级正确 / 表格列对齐 / 代码块 6 对平衡 / 链接全可达 ✅

#### 2.8 设计文档修订决策评估 ✅ 合理

DEV 标记 [设计质疑] 的 9 项设计文档修订（D.1/D.2 + F.1-F.4 + OBS-1a/2a/3a），DEV 不能写 docs/（AGENTS.md §2），PM 决策合并到 ADR-020 起草时由 ARCH 统一处理。

**逐项评估**：

| 修订 ID | 必要性 | 评估 |
|---------|--------|------|
| **D.1**（PYO3 ABI3 提升为全局前置） | **必要** | 设计 §4.1 原文把 `PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1` 仅写在 step 5 注释；实测 Python 3.14 + pyo3 0.22.6 下，step 1 的 `cargo build --release` 就会触发 abi3 forward compat 检查，若仅按原文步骤 5 设置，步骤 1-4 全部 FAIL。1a VET §4 D.1 评估已实测确认。 |
| **D.2**（-Mode 改 -Level） | **必要** | 设计 §A.1 写 `param([ValidateSet(...)]$Mode = "quick", ...)`，实际实施是 `param([string]$Level = 'All')`（DEV/REV/PM 共识改的，更直观，L1-L4 命名直接对应）。设计文档滞后于实施。 |
| **F.1**（run_l2_functional.py → .ps1） | **必要** | 设计 §6.3 1b 详细范围交付物清单写 `.py`，实际是 `.ps1`（语言替换，与 1a `run_l1_quality.ps1` 风格一致；设计 §2.3 分层精神仍遵守）。 |
| **F.2**（v4 PyCallable 脚本标 deprecated） | **必要** | 设计 §1.1 B 类清单未标 deprecated；v5 ADR-011/ADR-014 已删除 PyCallable / Container lib 路径，`phase4_smoke_repeat_until.py` 与 `v4_smoke_test.py` 必然失败（CompilationError / ModuleNotFoundError），必须降级标注。 |
| **F.3**（vet_phase3_bits_integer 标 known_broken） | **必要** | 设计 §1.1 B 类未注 known_broken；`Bitwise(Bit).parse(b"\x80")` 在 Python 3.14 + construct 2.10.70 触发 RestreamedBytesIO 上游兼容性 bug，与 construct-rs 无关。 |
| **F.4**（StopIf 间接覆盖说明） | **必要** | 设计 §4.2 L2 数据源分层未说明 StopIf 由 L1 cargo test --lib 间接覆盖（struct_node.rs 6 个 + greedy_range.rs 5 个 = 11 个单元测试）。 |
| **OBS-1a**（-Level 加引号示例） | **必要** | 1a VET OBS-1 实测确认 PS 5.1 native comma 是数组操作符，未加引号会被解析为数组→空格连接→Resolve-Levels 报错。设计 §A.1（D.2 关联）应加示例 `-Level "L1,L4"`。 |
| **OBS-2a**（C5/C6 内容非空判定） | **可选** | 1a VET OBS-2 实测 C5/C6 仅检查行号存在未检查内容非空。**PM 已决策保持现状**（CSV 由 PM 维护，影响极小），仅需在设计文档加注说明（不强制扩展）。 |
| **OBS-3a**（裸 Write-Host 合理例外注明） | **可选** | 1a VET OBS-3 + 1b VET OBS-VET-4 实测 run_l1_quality.ps1 L87 + run_l2_functional.ps1 L224 各 1 处裸 `Write-Host $tailPreview`（多行 stderr 预览），Write-CiLog 设计为单行无法承载。属合理例外，在设计文档加注即可。 |

**结论**：9 项中 **7 项必要**（D.1/D.2/F.1-F.4/OBS-1a，必须修订）+ **2 项可选**（OBS-2a/OBS-3a，加注说明即可）。PM 决策"合并到 ADR-020 起草时由 ARCH 统一处理"合理，避免分散修订。但需在 META-CI 整体 ACCEPTED 时**显式 flag**设计文档滞后状态作为 ADR-020 起草范围（不能遗忘）。详见 §5。

#### 2.9 §0 原则对照 ✅ 不触发

CI 是测试基础设施，§0 八条原则对照：

| §0 原则 | META-CI-1d 触发情况 |
|---------|--------------------|
| 1. 一次 FFI | ❌ 不触发（CI 不在 parse/build 运行时数据流上） |
| 2. 无中间表示层 | ❌ 不触发（CI 不引入 parse/build 中间数据类型） |
| 3. 输出/输入侧无抽象 trait | ❌ 不触发（CI PowerShell/Python 是测试编排，无 trait 抽象层） |
| 4. pyo3 是核心依赖 | ❌ 不触发（CI L1 调 maturin develop，L3 通过 bench 间接验证 pyo3 路径，未直接引入 pyo3 依赖） |
| 5. mashumaro 式 API | ❌ 不触发（CI SCENARIO_DEFS 验证 StructMixin API，未改 API） |
| 6. 构造器分派 | ❌ 不触发（CI 不涉及构造器分派） |
| 7. 错误处理 | ❌ 不触发（CI 错误是 exit code + JSON 报告，非 ConstructError） |
| 8. Stream 抽象 | ❌ 不触发（CI 不涉及 Stream） |

**确认未引入运行时数据流改动**：CI 全部位于 `testing/ci/` 下，未修改 `construct-rs/src/`（git status + 独立审查确认）。与 1a/1b/1c 一致。

### 3. 独立复跑结果

**复跑环境**：Windows 11 10.0.26200 / Python 3.14.2 / Intel Core i7-13700K / commit=bd90ea5 / 双 venv crs_venv_new (construct-rs 0.1.0) + crs_venv_py_new (Python construct 2.10.70)

#### 3.1 OBS-1 修复验证（mock + quick 模式）✅ PASS

```
& crs_venv_new\python.exe run_l3_perf.py --mock --quick --no-ab-test
==== L3 Performance Gate start (mock=True, quick=True) ====
[L3] baseline loaded: 154 points from perf-scenarios.csv
[L3] exemptions loaded: 21 entries
[L3] env: commit=bd90ea5
[L3] scenarios selected: 4 (SCENARIO_DEFS total=5)
[L3] measured: 4, skipped: 150
[L3] summary: total=154 pass=1 warn=0 fail=2 new=0 improved=1 skipped=150
```

- **修复后**：`scenarios selected: 4` / `measured: 4` / `skipped: 150`
- **修复前**（1c VET 实测）：`scenarios selected: 4` / `measured: 5` / `skipped: 149`（bug：多测了 `Array:A1:parse` quick=False）
- **影响**：QuickMode 少测 1 个场景（~10-20s），判据正确性不受影响（修复前后 verdict 全部正确）

#### 3.2 OBS-2 修复验证（JSON 报告 schema）✅ PASS

实测 L3 JSON 报告 environment 字段（用 Python `json.load` 解析确认）：

```json
{
  "timestamp": "2026-07-28T20:45:15+08:00",
  "python_version": "3.14.2",
  "python_exe": "...crs_venv_new\\Scripts\\python.exe",
  "crs_venv_python": "...crs_venv_new\\Scripts\\python.exe",
  "pc_venv_python": "...crs_venv_py_new\\Scripts\\python.exe",
  "construct_rs_commit": "bd90ea5",
  "construct_py_version": "2.10.70",   ← OBS-2 修复新增字段
  "baseline_source": "docs\\perf-scenarios.csv@bd90ea5",
  "os": "Windows-11-10.0.26200-SP0",
  "cpu": "Intel64 Family 6 Model 183 Stepping 1, GenuineIntel",
  "machine": "AMD64",
  "power_plan": "unknown",
  "thermal_state": "unknown",
  "l_09_disclaimer": "跨时段性能对比在边界场景（Rust<300ns）失效..."
}
```

- `construct_py_version: "2.10.70"` ✅（pc_python 子进程查询成功）
- 14 个字段与设计 §5.4 schema 对齐 ✅
- `construct_py_version` 与 `construct_rs_commit` 字段并列 ✅
- JSON 文件 UTF-8 无 BOM（首字节 0x7B = `{`）✅

**OBS-1 反测（非 quick 模式）确认未破坏**：
```
--mock --no-ab-test（无 --quick）
[L3] scenarios selected: 5 (SCENARIO_DEFS total=5)
[L3] measured: 5, skipped: 149
[L3] summary: total=154 pass=1 warn=0 fail=1 new=0 improved=3 skipped=149
```
- 非 quick 模式仍测 5 个（含 `Array:A1:parse`），未受 OBS-1 修复影响 ✅

#### 3.3 install_hook.ps1 临时目录模拟（不实际安装到 .git/hooks/）✅ 4 场景全 PASS

在 `<opencode-temp>\vet_1d_hook_test\tmp_repo\` 下构造 `.git/hooks/` + `testing/ci/` 目录树，复制 install_hook.ps1 + pre-commit.template 跑通：

| 测试场景 | 行为 | 退出码 | 结果 |
|---------|------|--------|------|
| 1. 第一次安装 | 复制 + post-install 验证（marker + size） | 0 | ✅ target 3068 bytes |
| 2. 二次幂等安装 | 检测 marker 匹配 → 提示已装 | 0 | ✅ 不覆盖 |
| 3. 非 -Force 覆盖非本项目 hook（用户自定义） | marker 不匹配 → refuse + stderr 指引 | 1 | ✅ 安全拒绝 |
| 4. -Force 覆盖 | Overwriting + 复制 + post-install 验证 | 0 | ✅ 成功覆盖 |

post-install marker 检查：target 文件头部 3 行内包含 `construct-rs CI smoke gate`（pre-commit.template 第 2 行）✅

**关键安全验证**：
- 实际仓库 `.git/hooks/pre-commit` **未被修改**（仍不存在）✅
- marker regex `-match "construct-rs CI smoke gate \(META-CI-1d\)"` 正确转义括号，用户自定义 hook 不会被误识别 ✅
- `Resolve-Path "..\.."` 从 `testing/ci/` 正确上溯到项目根 ✅

#### 3.4 21 单元测试（1c 回归）✅ 全 PASS

写 Python 简易 runner 手工调用所有 `test_*.py` 顶层函数（pytest 风格函数；crs venv 无 pytest）：

```
=== test_l3_classify.py (15 case) ===
PASS  test_baseline_load_real              PASS  test_normal_warn
PASS  test_boundary_fail                   PASS  test_normal_pass
PASS  test_boundary_pass                   PASS  test_skipped_no_measurement
PASS  test_boundary_warn                   PASS  test_special_fail_hard_constraint
PASS  test_exempted_fail_to_warn           PASS  test_speedup_boundary_in_range_9_to_11
PASS  test_exemptions_load_real            PASS  test_speedup_boundary_no_special_when_baseline_below_10
PASS  test_improved                        PASS  test_new_baseline_missing
PASS  test_normal_fail
=== test_ab_stats.py (6 case) ===
PASS  test_classify_delta                  PASS  test_run_analysis_with_mock_files
PASS  test_mean_basic                      PASS  test_stdev_sample
PASS  test_run_analysis_missing_file       PASS  test_welch_t

Total: 21, Passed: 21, Failed: 0
```

OBS-1/#2 修复未破坏 1c 既有功能 ✅

#### 3.5 README.md 渲染 + sh/PS 语法验证 ✅ PASS

- **Markdown 结构**：H1=1 / H2=10 / H3=21 / 表格行 71 / 代码块 6 对（12 fence，平衡）/ bullet 45 项
- **内部链接**：3 个 `../../docs/` 链接（设计 + 2 个 CSV）全可达 ✅
- **sh 语法**：用 `C:\Program Files\Git\bin\sh.exe -n pre-commit.template` 验证，exit 0 ✅
- **PowerShell Parser**：`[System.Management.Automation.Language.Parser]::ParseFile(install_hook.ps1)` 0 错误 ✅
- **Resolve-Levels 验证**（dot-source 测试）：
  - `Resolve-Levels "All"` → `L1,L2,L3,L4` ✅
  - `Resolve-Levels "L1,L4"` → `L1,L4` ✅
  - `Resolve-Levels "l1,l4"` → `L1,L4`（大小写不敏感）✅

#### 3.6 known_exemptions.json 21 条目精确核对 ✅ PASS

```powershell
$ke = ConvertFrom-Json (Get-Content known_exemptions.json -Raw)
A=18 B=1 C=2 (sum=21)
by_phase: 1=7, 2=5, 2.5=6, 4.6=3 (sum=21)
scenario_id unique OK
```

- A/B/C 类计数与 `statistics.by_user_decision` 一致 ✅
- 按 phase 计数与 `statistics.by_phase` 一致 ✅
- 21 个 scenario_id 全部唯一 ✅
- 与 1c VET §4 交叉核验一致（6 关键样本含 FormatField:B1:parse=8.09 / Bytes:E1-final:parse=9.90 / Index:E01-4.7-verify:parse=9.51 / PrefixedArray:p_err-4.x:build=2.12 / Array:a_err-4.x:parse=6.93 等）✅
- L-02 对策：全部从 CSV 复制，不凭记忆 ✅

### 4. META-CI 整体一致性核查

由于 1d 是 META-CI 最后一个子任务，VET 顺便做整体一致性核查（首次）。

#### 4.1 4 子任务接口一致性 ✅ PASS

| 维度 | L1 | L2 | L3 | L4 |
|------|----|----|----|----|
| 编排脚本 | `run_l1_quality.ps1` | `run_l2_functional.ps1` | `run_l3_perf.ps1` → `run_l3_perf.py` | `run_l4_consistency.py` |
| 调用方式 | PS 子进程 | PS 子进程 | PS → Python 子进程 | Python 子进程 |
| 入口参数 | `-ReportPath` / `-VenvPath` | `-ReportPath` / `-CrsPython` | `-ReportPath` / `-CrsPython` / `-PcPython` / `-QuickMode` / `-MockMode` / `-NoAbTest` | `--report=` |
| 报告路径 | `reports/<ts>_L1_report.json` | `reports/<ts>_L2_report.json` | `reports/<ts>_L3_report.{json,md}` | `reports/<ts>_L4_report.json` |
| 退出码 | 0=PASS / 1=FAIL | 0=PASS / 1=FAIL | 0=PASS / 1=WARN / 2=FAIL（三档） | 0=PASS / 1=FAIL |
| venv | crs_venv_new | crs_venv_new (CRS) | crs_venv_new (CRS) + crs_venv_py_new (PC) | crs_venv_new |

**`run_smoke.ps1` 主入口**：
- `-Level` 参数统一（L1/L2/L3/L4/All/逗号分隔组合），1a/1b/1c 迭代扩展支持
- hashtable splatting 转发参数到子脚本（按命名参数匹配，避免位置参数陷阱）
- L3 退出码特殊处理：`if ($lv -eq "L3") { $lvPassed = ($subCode -le 1) }`（WARN=1 不阻断 Overall）✅
- Overall 退出码：0=PASS / 1=FAIL（任一层次失败）✅

**一致性结论**：4 层接口设计统一，退出码语义清晰，PowerShell/Python 分工与设计 §2.3 分层一致（PS 编排 + Python 数据）。

#### 4.2 run_smoke.ps1 -Level All 是否真的覆盖 L1+L2+L3+L4 ✅ PASS

独立复跑验证 `Resolve-Levels("All")` 返回 `@("L1", "L2", "L3", "L4")`（不区分大小写）。foreach 循环按 `L1` / `L2` / `L3` / `L4` 顺序调用 Invoke-L1Gate / Invoke-L2Gate / Invoke-L3Gate / Invoke-L4Gate。

注：1a 阶段 All = L1+L4；1b 阶段扩展为 L1+L2+L4；1c 阶段扩展为 L1+L2+L3+L4（与设计 §6.1 一致）。

#### 4.3 known_exemptions.json 在 L3 中正确加载 ✅ PASS

`load_exemptions(path)` 函数（`run_l3_perf.py:183-189`）：
- 路径不存在时返回 `{}`（无豁免）✅
- 读 JSON 文件 → `{e["scenario_id"]: e for e in data["exemptions"]}` → dict 映射 ✅
- key 三元组 `<constructor>:<scenario_id>:<direction>` 与 perf-scenarios.csv BaselinePoint.key 对齐 ✅
- 在 `classify()` 中通过 `key in exemptions` 判定豁免，FAIL 自动降级 WARN ✅

实测 21 条目全部加载（`[L3] exemptions loaded: 21 entries`），与 1c VET §4 交叉核验一致。

#### 4.4 1a/1b/1c VET 15 个 OBS 项跟踪状态 ✅ 全部处理

| OBS 来源 | 数量 | 1d 处理状态 |
|---------|------|------------|
| 1a VET OBS-1（comma 引号） | 1 | README §2.2+§7.6 ✅ |
| 1a VET OBS-2（C5/C6 内容非空） | 1 | README §7.5（PM 决策保持现状）+ 设计修订 OBS-2a（可选） ✅ |
| 1a VET OBS-3（裸 Write-Host） | 1 | README §7.7 + 设计修订 OBS-3a（可选） ✅ |
| 1a VET OBS-4（fail-fast JSON 占位） | 1 | 未处理（PM 倾向 1d 不做，DEV §G.2 标 [设计质疑]，作为 Phase 5 可观测性改进） ✅ 已知未处理 |
| 1b VET OBS-VET-1（80 vs 72 对照点） | 1 | README §3.2（修正表述）✅ |
| 1b VET OBS-VET-2/3（设计权衡） | 2 | README §7（隐含在 deprecated 说明）✅ |
| 1b VET OBS-VET-4（run_l2_functional.ps1 L224 Write-Host） | 1 | README §7.7（与 1a OBS-3 合并）✅ |
| 1b VET OBS-VET-5（环境噪声） | 1 | README §3.2（耗时范围标注）✅ |
| 1c VET OBS #1（--quick 过滤失效） | 1 | **1d 代码修复 + 验证** ✅ |
| 1c VET OBS #2（construct_py_version 缺失） | 1 | **1d 代码修复 + 验证** ✅ |
| 1c VET OBS #3（命名不对齐） | 1 | README §7.1（隐含 Phase 5 follow-up 复审）✅ |
| 1c VET OBS #4（ab_harness ASCII 注释声明） | 1 | README §7.8 ✅ |
| 1c VET OBS #5（mock hash 随机） | 1 | 未显式说明（仅 CI 自检影响） ✅ 已知未处理 |
| 1c VET OBS #6（行数偏差） | 1 | 已在 1d DEV 报告 §B 修正为 966→988 ✅ |

**15/15 OBS 全部处理**（含 2 项已知未处理但有意识推迟：1a OBS-4 + 1c OBS #5，均为非阻断）。

### 5. 设计文档修订决策评估

DEV 标 [设计质疑] 9 项设计文档修订（§E.3 清单），PM 决策"合并到 ADR-020 起草时由 ARCH 统一处理"。VET 评估：

**PM 决策合理性**：✅ 合理。

**理由**：
1. **AGENTS.md §2 权限规则**：DEV 对 `docs/` 只读，不能直接修改设计文档。DEV 遵循系统规则优先（高于任务分派），不修改设计文档是正确决策。
2. **修订内容性质**：9 项均为"设计文档滞后于实施"（D.1/D.2/F.1-F.4 + OBS-1a 必要）或"PM 决策保持现状需加注说明"（OBS-2a/3a 可选），不影响 META-CI 实施已交付的功能。
3. **集中修订 vs 分散修订**：在 ADR-020 起草时统一修订，避免分散，且 ADR-020 本身就要总结 META-CI 方法学（§7.1 已锁定决策点），与设计文档修订天然耦合。
4. **README 已同步说明**：1d README §3.1+§7+§E.1 已说明 9 项修订的全部内容，开发者文档不滞后。

**关键风险点（PM 必须显式 flag）**：
- META-CI 整体 ACCEPTED 后，**到 ADR-020 起草完成前**会存在"实现已交付但设计文档滞后"的过渡状态。
- 这种状态可接受（README 已替代说明），但**不能遗忘**——必须在 META-CI ACCEPTED 时把"设计文档滞后 9 项"作为 ADR-020 起草范围的明确条目（建议在 ADR-020 草案 §附录或 §修订历史列明）。
- 若 ADR-020 起草推迟，应在 harness/MEMORY.md / Phase 5 follow-up 清单中跟踪。

**7 项必要 + 2 项可选**：详见 §2.8 评估表。

### 6. 非阻断性观察项

| OBS ID | 位置 | 描述 | 影响 | 建议 |
|--------|------|------|------|------|
| **OBS-1d-VET-1**（行数偏差） | DEV 报告 §B | DEV 报告 README.md 记 305 行，**实际 408 行**（差异 103 行）；run_l3_perf.py 记 966→988，实际相符。属 1c OBS-#6 同类（仅文档计数偏差）。 | 仅文档计数；功能与内容质量无影响。 | 已知；DEV 在 1c OBS-#6 已记录同类偏差，本项可同样视为文档计数可接受。无需修复。 |
| **OBS-1d-VET-2**（Markdown 报告未含 construct_py_version） | `run_l3_perf.py:645-715` `build_report_markdown` | OBS-2 修复后 JSON 报告含 `construct_py_version`，但**Markdown 报告**（build_report_markdown）的"测量环境"节未读取该字段（仅列 timestamp / python_version / crs_venv / pc_venv / commit / os / cpu / power_plan / baseline / l_09_disclaimer）。 | Markdown 报告是人工审查用，未显示 Python construct 版本只是信息略少；JSON schema 已完整。 | 非阻断；建议 Phase 5 follow-up 时在 `build_report_markdown` 加一行 `- construct(py): {env['construct_py_version']}`。 |
| **OBS-1d-VET-3**（power_plan fallback "unknown"） | `_windows_power_plan()` 行 545-559 | 实测 `power_plan: "unknown"`，子进程 `powercfg /getactivepowerscheme` 未返回 0。可能与 PS 5.1 调用 native cmd 的权限或编码有关。 | L-09 disclaimer 已声明"跨时段对比归因以 Controlled A/B Test 为准"，power_plan 缺失不影响判据；环境信息略不完整。 | 非阻断；建议 Phase 5 follow-up 时排查 powercfg 调用（可能需 `powercfg.exe` 全路径或 admin 权限）。 |
| **OBS-1d-VET-4**（hook 写死 "L1,L4"） | `pre-commit.template:46` | hook 直接写死 `run_smoke.ps1 -Level "L1,L4"`，无法在 commit 时改层次。 | 设计 §3.2 决策 pre-commit 跑 L1+L4（30s-2min），不跑 L2/L3 是有意决策（避免"hook 太慢被绕过"反模式）。开发者要跑全量可直接 `run_smoke.ps1 -Level All`，与 hook 解耦。 | 非阻断；属设计决策，文档已说明。无需修改。 |
| **OBS-1d-VET-5**（1a OBS-4 fail-fast JSON 占位未处理） | `run_l1_quality.ps1` + README §E.1 | 1a VET OBS-4（fail-fast 时 JSON `steps[]` 仅含已执行步骤，未占位 skipped 步骤）PM 倾向 1d 不做，DEV §G.2 标 [设计质疑] PM 已确认。 | 仅 CI 报告消费方需注意 step 索引跳跃；功能不受影响。 | 已知未处理；作为 Phase 5 可观测性改进 follow-up。 |

**说明**：5 个观察项**全部非阻断**。其中 OBS-1d-VET-4（hook 写死）和 OBS-1d-VET-5（1a OBS-4 推迟）属于设计决策层，不属代码缺陷；OBS-1d-VET-1（行数偏差）属文档计数；OBS-1d-VET-2（Markdown 报告字段）和 OBS-1d-VET-3（power_plan fallback）属次要信息完整性，建议 Phase 5 follow-up 处理。

### 7. VET 总体结论

**结论**：**通过（含非阻断观察项）**。

**通过理由**：

1. **逻辑正确性 ✅**：pre-commit hook（POSIX sh + PowerShell + git case 分支）+ install_hook.ps1（4 场景幂等/-Force 覆盖安全）+ run_l3_perf.py（OBS-1/OBS-2 修复点）逻辑全部正确；与设计 §3+§3.2+§5.4+§6.4 一致。

2. **行为一致性 ✅**：
   - OBS-1 修复：实测 `measured: 4 / skipped: 150`（修复前 5/149）✅
   - OBS-2 修复：实测 `construct_py_version: "2.10.70"` ✅
   - install_hook.ps1：4 场景实测行为符合预期 ✅
   - Resolve-Levels：All/L1,L4/l1,l4 三种输入实测正确 ✅

3. **错误处理 ✅**：
   - install_hook.ps1：缺 template / 缺 .git / 已装非 -Force / -Force 覆盖 / post-install 验证 全部 exit code 正确
   - pre-commit.template：exit code 0/1/2/* case 分支完整 + stderr 修复指引
   - run_l3_perf.py：subprocess 异常显式捕获 + timeout + fallback "unknown"

4. **资源安全 ✅**：无 unsafe / 无 unwrap/panic / 无内存泄漏风险（PowerShell + Python 子进程隔离，CPython GC 管理）

5. **API 一致性 ✅**：
   - `-Level` 参数贯穿 run_smoke.ps1 + README 文档示例
   - 报告路径 `reports/<ts>_<level>_report.{json,md}` 统一
   - 双 venv 切换（crs_venv_new + crs_venv_py_new）一致
   - JSON schema 与设计 §5.4 对齐

6. **边界条件 ✅**：
   - install_hook.ps1：缺 hooks 目录 → 创建；非本项目 hook → refuse；marker 缺失 → exit 1
   - pre-commit.template：REPO_ROOT 解析失败 → exit 1；SMOKE_RUNNER 缺失 → exit 1
   - OBS-1 修复：quick 模式跳过 quick=false 场景，但 non-quick 模式仍测全部（反测确认）

7. **§0 原则对照 ✅ 不触发**：CI 是测试基础设施（与 1a/1b/1c 一致）

8. **9 必查项 ✅ 全部通过**（详见 §2 逐项结论）

9. **15 个 1a/1b/1c OBS 项 ✅ 全部处理**（含 2 项有意识推迟 Phase 5）

10. **META-CI 整体一致性 ✅**：4 子任务接口统一 + -Level All 覆盖 4 层 + known_exemptions.json 21 条目正确加载

### 8. META-CI 整体 ACCEPTED 建议

**建议 PM 可以**：
1. **ACCEPT META-CI-1d** ✅
2. **ACCEPT META-CI 整体**（1a + 1b + 1c + 1d 全部通过 VET 审查）✅
3. **触发 ADR-020 起草**（META-CI 方法学：平台选型 + 触发时机分层 + baseline 不自动更新 + 性能回归判据三档 + A/B Test 通用化）

**ADR-020 起草范围必须包含**：
- META-CI §7.1 已锁定决策点（平台选型 / 触发时机 / baseline / 判据三档）
- **§5 中 9 项设计文档修订**（7 项必要 D.1/D.2/F.1-F.4/OBS-1a + 2 项可选 OBS-2a/3a）→ 同步修订 `docs/design/基础设施/CI冒烟门禁设计.md`
- **Phase 5 follow-up 清单**：
  - SCENARIO_DEFS 全 151 场景映射
  - 1a VET OBS-4（fail-fast JSON 占位，可观测性改进）
  - 1c VET OBS #5（mock hash 随机性，CI 自检影响）
  - OBS-1d-VET-2（Markdown 报告 construct_py_version 字段）
  - OBS-1d-VET-3（power_plan powercfg 调用排查）

**建议 PM 在 ACCEPTED 时产出 Evaluate 摘录**（AHE §演化循环）：
- task_id: META-CI（含 1a/1b/1c/1d）
- tool_calls 关键点：ARCH 设计 §0-§8 + 附录 A/B → REV 检视 → ARCH 修订（known_exemptions 21 条目）→ DEV 1a → VET 1a → PM 1a ACCEPTED → DEV 1b → VET 1b → PM 1b ACCEPTED → DEV 1c → VET 1c → PM 1c ACCEPTED → DEV 1d → VET 1d（本审查）→ PM META-CI ACCEPTED → ADR-020 起草
- failures 对齐 L-XX：
  - L-01（中间表示层违反）：未触发（CI 是测试基础设施）
  - L-02（理论估算替代实证数据）：未触发（known_exemptions 21 条目全部精确匹配 CSV）
  - L-03（PM 接受不对等证据）：未触发（baseline 不自动更新 + Controlled A/B Test 是 L-03/L-09 对策工程化）
  - L-09（跨时段性能对比消除法归因失效）：核心应用（A/B Test 通用化是 L-09 对策工程化）
  - 候选：设计文档滞后 9 项（由 ARCH 在 ADR-020 起草时统一修正）
- outcome: 完成（META-CI 整体 ACCEPTED；ADR-020 起草触发；SCENARIO_DEFS 全场景映射 + 设计文档滞后 + 1a OBS-4 + 1c OBS-#5 + OBS-1d-VET-2/3 记入 Phase 5 / ADR-020 修订范围）

---

> **VET 1d 完成**。META-CI 整体可 ACCEPTED；触发 ADR-020 起草（含 9 项设计文档修订 + Phase 5 follow-up 清单）。

---

## META-CI-1d: PM 验收分析记录（ACCEPTED）

| 项目 | 内容 |
|------|------|
| 状态 | ACCEPTED |
| 验收时间 | 2026-07-28 |
| 验收依据 | DEV 自检报告（§META-CI-1d）+ VET 审查报告（§META-CI-1d-VET，独立复跑确认） |

### PM 独立验证清单

#### 1. 数据逻辑自洽性核查

| 维度 | 核查结论 |
|------|---------|
| OBS-1 修复（`--quick` 过滤失效） | ✅ VET 独立复跑实测 measured: 4（修复前 5），未破坏 non-quick 模式 |
| OBS-2 修复（`construct_py_version`） | ✅ VET 独立复跑实测 JSON 含 `construct_py_version: 2.10.70` |
| pre-commit hook 工作机制 | ✅ install_hook.ps1 4 场景模拟（首次装 / 幂等 / 非 -Force 拒绝 / -Force 覆盖）全 exit code 正确；实际仓库 .git/hooks/ 未被修改 |
| 1c 回归验证 | ✅ 21 单元测试 PASS（OBS-1/#2 修复未破坏既有功能） |
| META-CI 整体一致性 | ✅ 4 子任务 -Level 参数贯穿 / 报告路径统一 / -Level All 覆盖 L1+L2+L3+L4 / known_exemptions 加载 |

#### 2. 设计文档修订决策

PM 决策：**9 项设计文档修订合并到 ADR-020 起草时由 ARCH 统一处理**。

理由：
- DEV 不能写 docs/（AGENTS.md §2 权限规则）
- 9 项都是小修订（措辞 / 命名 / 标注），不影响代码功能
- ADR-020 起草时会重新审视 META-CI 整体方法学，统一修订避免分散

**VET 评估**：7 项必要 + 2 项可选，PM 决策合理。**关键提醒**：ACCEPTED 时必须把"9 项设计文档滞后"显式记入 ADR-020 起草范围条目（已记入下方 §META-CI 整体 ACCEPTED）。

#### 3. 观察项处理

| 观察项 | PM 决策 |
|--------|---------|
| OBS-1d-VET-1（README 行数 305 vs 实际 408） | 非阻断。文档计数偏差，ADR-020 起草时统一修正 |
| OBS-1d-VET-2（Markdown 报告未含 construct_py_version） | 非阻断。JSON 已完整，Markdown 后续补 |
| OBS-1d-VET-3（power_plan fallback "unknown"） | 非阻断。L-09 disclaimer 已覆盖 |
| OBS-1d-VET-4（hook 写死 "L1,L4"） | 非阻断。设计决策（pre-commit 只跑快速门禁） |
| OBS-1d-VET-5（1a OBS-4 fail-fast JSON 占位） | 非阻断。Phase 5 可观测性改进 |

#### 4. 质量门禁

| 项目 | 结果 |
|------|------|
| OBS-1/#2 修复 | ✅ VET 独立复跑实测通过 |
| 21 单元测试 | ✅ 全 PASS（1c 回归） |
| install_hook.ps1 模拟 | ✅ 4 场景全 exit code 正确 |
| README 渲染 | ✅ 10 节 + 21 子节 + 表格 + 链接全可达 |
| §0 原则对照 | ✅ CI 是测试基础设施，§0 八条均不触发 |

### PM 验收结论

**ACCEPTED**。

### Evaluate 摘录（AHE §演化循环）

- task_id: META-CI-1d
- tool_calls 关键点: ARCH 设计（§3 触发时机 + §6.4 子任务 1d）→ DEV 实施（hook + README + OBS-1/#2 修复 + 文档批次）→ VET 独立复跑（OBS 修复 + 21 单元测试 + install_hook 模拟 + 整体一致性）→ PM ACCEPTED
- failures 对齐 L-XX:
  - L-01~L-09：均未触发
  - 候选：设计文档滞后 9 项（由 ARCH 在 ADR-020 起草时统一修正）
- outcome: 完成（META-CI-1d ACCEPTED；META-CI 整体 ACCEPTED；ADR-020 起草触发）

---

