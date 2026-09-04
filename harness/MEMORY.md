---
id: MEMORY-root
status: active
phase: meta
last_updated: 2026-07-29
---

# MEMORY.md — Project Long-Term Memory Index

> **Phase 8/9/10 完成（2026-08-07）**：
> - Phase 8 部分通过：22 构造器实现 + 8.ENV（Python 3.13 非 abi3，放弃 abi3）+ 8.OPT-SHARED 共享税优化（ADR-023，GenericGetDict+KnownHash+ob_sval），42/48 PASS，6 项已知 LOW 边界（Hex/HexDump/NamedTuple/FlagsEnum 显示对象构造固有税）
> - Phase 9 系统测试：4 真实协议（Modbus RTU/CAN/IEC104/IPv4）91 测试全 PASS，round-trip + parity 通过，发现 4 个 API gap
> - Phase 10 用法 SKILL：`.opencode/skills/neoconstruct-usage/SKILL.md`（1701 行，100% API 覆盖），验证 agent 仅看 SKILL 单轮实现 SCTP 协议（53 round-trip + 8 parity 测试 PASS）
> - L-14 教训第 2/3 次触发（Hex parse 归因错误 + managed dict UB），已沉淀
> - 构造器总进度：~78→~100/134（~58%→~75%），剩余 ~30 wont_implement
> **Phase 7 完成（2026-07-30 ACCEPTED）**：
> - 7.1 Conditional（5 个：IfThenElse/Switch/Select/FocusedSeq + ExplicitError + If macro）
> - 7.2 Streams（3 个：Seek/Pointer/Prefixed + Stream 扩展：BuildStream.seek + ParseStream.seek_whence）
> - 7.3 bench 70 测量点 + A/B Test 复测 parse 全部 ≥10x（原 27 项不达标确认是测量漂移）
> - tag `phase-7-complete`
> - 教训 L-13（docstring 未跟随语法演进）+ L-14（设计硬约束认知需交叉验证）沉淀
> - BytesInteger u128 fast-path 修复（2.82x → 9.01x/12.02x，L-14 教训触发）
> 构造器总进度：~70→~78/134（~53%→~58%）
> - 6.0 测试框架重构（parity helper + bench runner + L2 双轨制）
> - 6.1 Primitives 收尾 17 个（VarInt/ZigZag/BytesInteger + Float half crate + 别名）
> - 6.2 Strings 7 个（6 Node + utf16/32 raw FFI，ADR-021）
> - 6.3 Adapter 核心 6+Pass（双层分离：内置 Rust Node / 用户面 Python，ADR-022）
> - 6.6 bench 223 测量点 + parity 17/17 + ADR-021/022 沉淀
> - 24 构造器 18/24 ≥10x，6 个不达标全部 PM 决策接受
> - tag `phase-6-complete`
> 构造器总进度：40→~70/134（31%→~53%）

# MEMORY.md — Project Long-Term Memory Index

> **HARNESS.md v1.0 §目录结构标准：Long-Term Memory 顶层文件。**
>
> 本文件是任何角色（PM/ARCH/DEV/REV/VET/AUDITOR）启动时的**第一入口**。
> 不存放具体知识——存放"知识在哪里"的指针。
> 具体知识分布在三层 LTM 中：本文件（索引）/ `experiences.md`（模式化失败教训）/ `docs/decisions/ADR-*.md`（具体决策）。

---

## 项目定位（一句话）

**正式名称：neoconstruct**（2026-09-03 起原名 construct-rs 停用；Python 导入 import neoconstruct，PyPI 包名 neoconstruct，Rust crate neoconstruct-core）。用 Rust 重写 Python `construct` 库的内核（pyo3 直接操作 CPython C API，无中间表示层），交付 Python 包，性能目标 ≥4x vs construct 2.10.70（10x 为理想）。详见 `AGENTS.md §0`。

---

## LTM 三层结构

| 层 | 文件 | 用途 | 维护者 |
|----|------|------|--------|
| **L0 索引** | `MEMORY.md`（本文件） | 顶层入口，指向所有 LTM | PM |
| **L1 教训** | `experiences.md` | 跨阶段模式化失败教训 L-01 ~ L-06 | 全员可新增（PM 维护） |
| **L2 决策** | `docs/decisions/ADR-*.md` | 单条决策的 context/decision/consequences | ARCH |
| **L3 工件** | `docs/design/` / `docs/reviews/` / `docs/analysis/` / `plans/` | 设计文档 / 审查 / 分析 / 过程记录 | 各角色 |
| **L3 单一事实源清单** | `docs/constructors-inventory.csv` + `docs/perf-scenarios.csv` | 构造器全集 + 性能场景测量点 | PM 主维护（详见 `pm-extension.md §构造器清单维护`） |

**agent 使用规则**：
- 启动时：先读本文件 → 再按角色读 `experiences.md` + 当前 phase 总纲
- 决策时：对照 `experiences.md` 检查"是否匹配 L-XX 模式" → 若涉及设计决策，查 `docs/decisions/`
- 沉淀时：新模式写入 `experiences.md`；新决策写入 `docs/decisions/ADR-NNN`

---

## 阶段索引（单一事实源：`plans/phaseN/总纲.md`）

| Phase | 状态 | Tag | 入口 |
|-------|------|-----|------|
| 0 架构设计 | ✅ | `phase-0-complete` | `plans/phase0-architecture/总纲.md` |
| 1 垂直切片 | ✅ | `phase-1-complete` | `plans/phase1-foundation/总纲.md` |
| 2 表达式系统 | ✅ | `phase-2-complete` | `plans/phase2-expression/总纲.md` |
| 2.5 性能优化 | ✅ | `phase-2.5-complete` | `plans/phase2.5-context-vec/总纲.md` |
| 3 BitStream | ✅ | `phase-3-complete` | `plans/phase3-bitstream/总纲.md` |
| 4 Array | ✅ 完成（2026-07-28，重审后重打 tag；iter9 拆分过程记录） | `phase-4-complete`（重打） | `plans/phase4-array/总纲.md`（traces 导航见 `索引.md`） |
| META-CI 冒烟门禁 | ✅ 完成（iter9 抽离到 plans/meta/） | — | `plans/meta/CI-冒烟门禁/索引.md` |
| META-INV 构造器清单 | ✅ 完成（iter9 抽离到 plans/meta/） | — | `plans/meta/INV-构造器清单/索引.md` |
| 5 Struct + FFI 入口优化 | ✅ 完成（2026-07-29 ACCEPTED，目标自然达成，tag phase-5-complete） | `phase-5-complete` | `plans/phase5-struct-ffi/总纲.md` |
| 6 Primitives 收尾 + Strings + Adapter 核心 | ✅ 完成（2026-07-30 ACCEPTED，tag phase-6-complete） | `phase-6-complete` | `plans/phase6-primitives-strings-adapter/总纲.md` |
| 7 Conditional + Streams | ✅ 完成（2026-07-30 ACCEPTED，tag phase-7-complete） | `phase-7-complete` | `plans/phase7-conditional-streams/总纲.md` |
| 8 Adapter核心+Struct收尾+Streams常用+Other常用 | ✅ 部分通过（2026-08-07，42/48 PASS，6 项已知 LOW，tag phase-8-complete） | `phase-8-complete` | `plans/phase8-adapters-struct-streams/总纲.md` |
| 9 系统测试 | ✅ 完成（2026-08-07，4 协议 91 测试 PASS，tag phase-9-complete） | `phase-9-complete` | `plans/phase9-system-test/总纲.md` |
| 10 用法 SKILL | ✅ 完成（2026-08-07，SKILL v2 + SCTP 验证通过，tag phase-10-complete） | `phase-10-complete` | `plans/phase10-skill/总纲.md` |
| **管理方式变更** | 2026-09-03 起进入半正式发布状态：任务标识 `v<version>-<任务号>`，目录 `plans/v<version>/`，tag `v<N>.<M>.<P>-complete`（phase 制历史标识不变） | — | `AGENTS.md §1` |
| v0.1.1（含 -8 更名 neoconstruct + PyPI 首发）| ✅ 已发布（2026-09-04：neoconstruct 0.1.1 上架 PyPI，21 产物 5 平台 ×3.10-3.13；历史已重写推送 GitHub；tag v0.1.1） | `v0.1.1` | `plans/v0.1.1/总纲.md` |
| v0.1.2 值语义修复 + 框架 | 🔵 进行中（0.1.2 止血已发布；ValueKind 框架实现 v0.1.2-4 CODING 中——设计 v2.2 定稿；管理批次含 §0 新增第 9/10 条：construct 仅 benchmark / 源码零过程信息） | — | `plans/v0.1.2/总纲.md` |

> **状态图例**：⚪ 未开始 / 🔵 进行中 / ✅ 完成 / 🔴 阻塞。状态细节由 `plans/phaseN/总纲.md` 维护（单一事实源），本表只索引。
> **iter9 结构变更**：phase4 过程记录.md（11837 行）拆分为 12 个 phase4 traces + 7 个 META traces；META 类 cross-phase 资产抽到 `plans/meta/`；`docs/design/` 子目录化（模块设计/ + 基础设施/）。详见 `plans/phase4-array/索引.md` 和 `harness/metadata-convention.md §5.1/§9`。

---

## Harness 组件清单

> **规范来源**列说明（L-08 对策）：标注每个组件的来源是 AHE 通用规范还是项目自定义。
> 通用规范组件不可被项目特定内容污染；项目自定义组件不适用于 AHE iteration 自身的 verify。

| HARNESS 组件 | 文件位置 | 规范来源 |
|-------------|---------|---------|
| System Rules | `AGENTS.md` / `SOUL.md`（待建） | 项目自定义（AGENTS.md），但映射 HARNESS.md §7 组件 |
| Tool Descriptions | opencode.json（声明 permission） | 项目自定义（opencode 框架约定） |
| Tool Implementations | opencode 内置（bash/edit/glob/grep/read/write 等） | 项目自定义（opencode 框架约定） |
| Middleware | （未启用） | HARNESS.md §7 定义，项目未启用 |
| Skills | `.opencode/skills/{agentic-harness-engineering, neoconstruct-ahe-practices, performance-gate, pm-performance-validation}/SKILL.md` | `agentic-harness-engineering` = HARNESS.md 通用；`neoconstruct-ahe-practices` = 项目级补充；`performance-gate` / `pm-performance-validation` = 项目自定义 |
| Sub-Agents | `.opencode/agents/{pm,architect,developer,reviewer,vetter,auditor}.md` | **全部项目自定义**（HARNESS.md / 通用 AHE skill 未定义具体角色；AUDITOR 是项目业务流程审计员，不审计 AHE iteration） |
| Long-Term Memory | `MEMORY.md`（本文件）+ `experiences.md` + `docs/decisions/` | HARNESS.md §7 定义结构，内容由项目填充 |

---

## AHE 演化历史

| 迭代 | 时间 | Manifest | 主要变更 | Verdict |
|------|------|----------|---------|---------|
| 1 | 2026-07-27 | `harness/manifests/change_2026-07-27.json` | 建立 LTM（harness/experiences.md）/ 规范 skill 目录 / AUDITOR 注册对齐 / 建立 manifests 基础设施 | **verified**（iteration 3 收尾时补做） |
| 2 | 2026-07-27 | `harness/manifests/change_2026-07-27-websearch.json` | 启用 websearch | partial（配置层就位，运行时未生效，用户主动 skip） |
| 3 | 2026-07-27 | `harness/manifests/change_2026-07-27-docs-restructure.json` | 记录/设计文档 AHE 化（T1 结构正交 + T2 frontmatter + T3 ADR） | partial（predicted_impact 漏报 42 broken refs + 9 frontmatter 缺失，已当场修复） |
| 4 | 2026-07-27 | `harness/manifests/change_2026-07-27-skill-iteration-lessons.json` | 沉淀 L-07 教训；新建项目级 skill `neoconstruct-ahe-practices`（不动通用 AHE skill） | **partial**（iter5 dogfood 验证：neoconstruct-ahe-practices skill 被 PM 主动加载 ✓；但"文件重组时主动执行 §A1 cross-ref check" + "通用 skill 同步不冲突" 两个预测未触发——等待下次文件重组 iteration） |
| 5 | 2026-07-27 | `harness/manifests/change_2026-07-27-iter5-evaluate-step.json` | 补 AHE §演化循环缺失的 Evaluate 步骤（项目级 §C 协议）；沉淀 L-08（AHE 规范解读层错误）；MEMORY.md Harness 组件清单加规范来源列；项目级 skill 加 §D AHE 规范解读检查清单 | **verified_with_followups**（iter6 dogfood 4/4 预测全部触发验证场景且通过；followup: §D 长期生效需 iter7+ 持续观察） |
| 6 | 2026-07-27 | `harness/manifests/change_2026-07-27-iter6-workflow-integration.json` | AHE §演化循环融入项目工作流（AGENTS.md §1 + pm.md / auditor.md 同步）；AUDITOR 加第 6 类审计项 + L-08 范畴提示 | **partial**（iter7 dogfood 1/4 预测触发验证；3 个预测需 phase 子任务场景——等待 Phase 5+ 启动） |
| 7 | 2026-07-27 | `harness/manifests/change_2026-07-27-iter7-agents-slim.json` | AGENTS.md 瘦身重构（457→66 行，缩减 85.6%）；内容按四维度分类迁移到 agent 文件；§5 内容准入标准建立；38 个活跃文件 cross-reference 修复 | **partial**（iter8 dogfood 1/4 预测触发——§5 自我应用；1 个结构变更——启动入口重定义为 base+extension；2 个未触发） |
| 8 | 2026-07-27 | `harness/manifests/change_2026-07-27-iter8-base-extension-split.json` | base/extension 分层架构（跨工程化 + 渐进式披露）；§5 下沉到 pm-extension；6 base + 6 extension 拆分；阶段 2 全面粒度调正（base 详细化通用清单，extension 仅项目特定）；启动加载量平均 282 行（减半目标达成） | partial（两轮迭代：初版 + 用户评估后粒度调正） |
| 9 | 2026-07-29 | `harness/manifests/change_2026-07-29-trace-split.json` | 过程记录拆分 + 目录重组：phase4 过程记录.md（11837 行）→ 12 个 phase4 traces + 7 个 META traces 抽离到 plans/meta/（CI-冒烟门禁/ + INV-构造器清单/）；docs/design/ 子目录化（模块设计/ + 基础设施/）；§5.1 目录结构 + §9 强制阈值（1000 行/5 子任务）；CSV source_ref 改文件级引用；run_l4_consistency.py C5/C6 支持新格式；16 个 python/tests + 53 个文档 cross-ref 同步修复 | verified_with_followups（启动前 explore agent 全量扫描 L-07 对策，识别 200 处 broken refs + 6 个风险点） |
| 10 | 2026-07-29 | `harness/manifests/change_2026-07-29-iter10-harness-centralize.json` + `harness/manifests/iter10-scan-report.md` | harness 文件集中（MEMORY/experiences/manifests/extensions/metadata-convention/evaluations → harness/）+ testing/experiment 分离（experiments/ci → testing/ci）。38 文件 / 165 引用修复 + R1/R2 隐藏代码依赖（run_l3_perf.py 改 Path(__file__).parent + pre-commit.template 路径同步）。中途 L-11 事件（PM 语义误判，一度误标 DEFERRED，用户纠正后完成） | verified_with_followups（L4 C1-C10 全 PASS；followup: Phase 5 启动时验证启动链稳定 + L1 smoke PowerShell 中文编码问题预存） |

## 项目级里程碑（非 AHE iteration）

| 里程碑 | 时间 | 内容 | 状态 |
|--------|------|------|------|
| Phase 4 Array | 2026-07-28 | 6 构造器（Array/GreedyRange/PrefixedArray/RepeatUntil/Index/StopIf）+ 重审 + 4.6 O1 优化 + 4.x 错误路径优化 | ✅ 完成（tag `phase-4-complete` @ f157bbe） |
| 构造器清单单一事实源 | 2026-07-28 | 双 CSV（inventory 134 构造器 + perf-scenarios 154 测量点）+ 维护规则（PM 主维护 + ARCH 骨架 + AUDITOR 第 7 类审计） | ✅ 完成 |
| META-CI 冒烟门禁系统 | 2026-07-28 | 4 层门禁（L1 质量 / L2 功能 / L3 性能 / L4 一致性）+ Controlled A/B Test 通用化（L-09 工程化）+ known_exemptions 21 条目 | ✅ 完成（ADR-020 沉淀） |
| L-09 教训沉淀 | 2026-07-28 | 跨时段性能对比消除法归因失效（边界场景）+ Controlled A/B Test 对策 | ✅ 完成（harness/experiences.md + performance-gate SKILL Checkpoint 4） |

## 教训索引

详见 `experiences.md`，按优先级排序：

| ID | 模式 | 触发场景 |
|----|------|---------|
| L-01 | 中间表示层违反 | parse/build 数据流设计 |
| L-02 | 理论估算替代实证数据 | 性能预测 |
| L-03 | PM 接受不对等证据 | 性能验收 |
| L-04 | 跨阶段模式未沉淀 | Phase 验收 |
| L-05 | 优化 A 路径忽略 B 路径 | 性能设计 |
| L-06 | 字段数混淆对照 | 性能数据对比 |
| L-07 | predicted_impact 重结构轻交叉引用 | AHE harness 重组 |
| L-08 | AHE 规范解读层错误（通用 vs 项目自定义混淆） | AHE iteration 中引用角色/流程时 |
| L-09 | 跨时段性能对比消除法归因失效（边界场景） | 性能回归判定 / 跨时段性能对比 |
| L-10 | 规范存在 ≠ 实际执行（无 enforcement） | iter9 沉淀：iter3 §5 per-子任务规范到 iter9 才首次执行 |
| L-11 | PM 对用户指令的语义误判 | iter10 沉淀：双引号"关闭iter10"被误判为"取消"，实际是"完成" |
| L-12 | PM 角色越界深入技术/代码细节 | Phase 6 立项沉淀：PM 自己 grep 源码查依赖（应分派 ARCH），违背 pm.md base §PM 不做的事 |
| L-13 | docstring 未跟随语法演进 | Phase 7 审查沉淀：Phase 2 废弃 `this` 但 15+ docstring 残留 `this.xxx` |
| L-14 | 设计硬约束认知需交叉验证 | RawCopy 质疑沉淀：ARCH 把"必须拷贝"当硬约束，漏了 Rust 内置 hashfunc 零拷贝路径 |
| L-15 | 权限白名单绕过与审查角色写代码 | 两次触发（v0.1.1：PM 分派越权×4 + VET bash 绕过；v0.1.2：DEV 写 Temp 复数次[平台工具描述诱导] + PM 越权未遂删 DEV 过程文件 + commit 未检混入半成品）；对策=分派对照权限表 + 拦截即上报 + 过程文件归任务所有者 + commit 前核对 status 明细 |

---

## 关键性能快照

| Phase | 场景 | parse / build 加速比 | 数据源 |
|-------|------|---------------------|--------|
| 1 | B1-B4（3-100 字段） | 7.6-13.7x / 11.7-17.4x | `plans/00-项目进度.md` |
| 2.5 | E1-E3（Bytes/Tell） | ~10-12x | `plans/00-项目进度.md` |
| 3 | BitStruct | 12x / 17x | `plans/00-项目进度.md` |
| 4 | Array/GreedyRange/PrefixedArray/RepeatUntil/Index | ≥10x 全场景达标（O1 后） | `docs/perf-scenarios.csv` |
| 4 | StopIf | 10.72-15.69x（O1 后 VET 复测） | `docs/perf-scenarios.csv` |
| 4 | E01 Array(0 Index) parse | 9.51x（B1 类边界场景，待 Phase 5 复审） | `docs/perf-scenarios.csv` |
| 4.x | a_err_eof（错误路径） | 11.34x（O1-O4 后 VET Controlled A/B Test，首次 unsafe raw FFI） | `docs/perf-scenarios.csv` |
| 4.x | p_err_overflow（错误路径） | 2.72x（Python baseline 5.6µs 结构性边界，用户决策方案 A 接受） | `docs/perf-scenarios.csv` |
| 5 | B1-B4（3-100 字段 Struct parse） | 11.64-13.93x（5.6a 基线重建 Controlled A/B Test，原 Phase 1 7.6-13.7x 自然达标） | `docs/perf-scenarios.csv` + `plans/phase5-struct-ffi/traces/5.6a-基线重建.md` |
| 5 | StopIf S01/S03 | 12.29x / 12.32x（原 9.63x/9.93x 自然达标） | 同上 |

> 详细数据见各 phase 总纲 / 过程记录。性能门禁规则见 `.opencode/skills/performance-gate/SKILL.md`。

---

## 活跃的设计质疑

（无）

> 历史质疑见各过程记录。
>
> **已解决（2026-07-28）**：
> - 硬约束 #5 "禁止局部门禁" 追溯适用范围——用户决策仅 Phase 4 起适用；Phase 1/2/2.5 共 18 个 <10x 测量点不追溯复审，记入 Phase 5 待办（Struct + FFI 入口优化）。详见 `plans/phase4-array/总纲.md §用户决策`
> - p_err_overflow 错误路径 D 类——用户决策方案 A（接受单点 <10x 作为 Python baseline 短导致的固有边界，与 StopIf B1 同档处理）。详见 `plans/phase4-array/总纲.md §用户决策（2026-07-28 补充）`

---

## 维护规则

- **新增文档**：必须在 `docs/design/` / `docs/decisions/` / `docs/reviews/` / `docs/analysis/` / `plans/phaseN/` 之下，并加 frontmatter
- **状态变更**：phase 状态变更只改 `plans/phaseN/总纲.md`，本文件不记录细节
- **新 ADR**：编号自增，supersede 旧决策时必须在旧 ADR 的 frontmatter 加 `superseded_by`
- **新教训**：写入 `experiences.md`，必须有 ≥1 个真实事件证据
