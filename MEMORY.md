---
id: MEMORY-root
status: active
phase: meta
last_updated: 2026-07-27
---

# MEMORY.md — Project Long-Term Memory Index

> **HARNESS.md v1.0 §目录结构标准：Long-Term Memory 顶层文件。**
>
> 本文件是任何角色（PM/ARCH/DEV/REV/VET/AUDITOR）启动时的**第一入口**。
> 不存放具体知识——存放"知识在哪里"的指针。
> 具体知识分布在三层 LTM 中：本文件（索引）/ `experiences.md`（模式化失败教训）/ `docs/decisions/ADR-*.md`（具体决策）。

---

## 项目定位（一句话）

用 Rust 重写 Python `construct` 库的内核（pyo3 直接操作 CPython C API，无中间表示层），交付 Python 包，性能目标 ≥4x vs construct 2.10.70（10x 为理想）。详见 `AGENTS.md §0`。

---

## LTM 三层结构

| 层 | 文件 | 用途 | 维护者 |
|----|------|------|--------|
| **L0 索引** | `MEMORY.md`（本文件） | 顶层入口，指向所有 LTM | PM |
| **L1 教训** | `experiences.md` | 跨阶段模式化失败教训 L-01 ~ L-06 | 全员可新增（PM 维护） |
| **L2 决策** | `docs/decisions/ADR-*.md` | 单条决策的 context/decision/consequences | ARCH |
| **L3 工件** | `docs/design/` / `docs/reviews/` / `docs/analysis/` / `plans/` | 设计文档 / 审查 / 分析 / 过程记录 | 各角色 |

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
| 4 Array | 🔴 重审 | — | `plans/phase4-array/总纲.md` |
| 5+ | ⚪ | — | 待用户指定 |

> **状态图例**：⚪ 未开始 / 🔵 进行中 / ✅ 完成 / 🔴 阻塞。状态细节由 `plans/phaseN/总纲.md` 维护（单一事实源），本表只索引。

---

## Harness 组件清单

| HARNESS 组件 | 文件位置 |
|-------------|---------|
| System Rules | `AGENTS.md` / `SOUL.md`（待建） |
| Tool Descriptions | opencode.json（声明 permission） |
| Tool Implementations | opencode 内置（bash/edit/glob/grep/read/write 等） |
| Middleware | （未启用） |
| Skills | `.opencode/skills/{agentic-harness-engineering, construct-rs-ahe-practices, performance-gate, pm-performance-validation}/SKILL.md` |
| Sub-Agents | `.opencode/agents/{pm,architect,developer,reviewer,vetter,auditor}.md` |
| Long-Term Memory | `MEMORY.md`（本文件）+ `experiences.md` + `docs/decisions/` |

---

## AHE 演化历史

| 迭代 | 时间 | Manifest | 主要变更 | Verdict |
|------|------|----------|---------|---------|
| 1 | 2026-07-27 | `manifests/change_2026-07-27.json` | 建立 LTM（experiences.md）/ 规范 skill 目录 / AUDITOR 注册对齐 / 建立 manifests 基础设施 | **verified**（iteration 3 收尾时补做） |
| 2 | 2026-07-27 | `manifests/change_2026-07-27-websearch.json` | 启用 websearch | partial（配置层就位，运行时未生效，用户主动 skip） |
| 3 | 2026-07-27 | `manifests/change_2026-07-27-docs-restructure.json` | 记录/设计文档 AHE 化（T1 结构正交 + T2 frontmatter + T3 ADR） | partial（predicted_impact 漏报 42 broken refs + 9 frontmatter 缺失，已当场修复） |
| 4 | 2026-07-27 | `manifests/change_2026-07-27-skill-iteration-lessons.json` | 沉淀 L-07 教训；新建项目级 skill `construct-rs-ahe-practices`（不动通用 AHE skill） | pending |

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

---

## 关键性能快照

| Phase | 场景 | parse / build 加速比 | 数据源 |
|-------|------|---------------------|--------|
| 1 | B1-B4（3-100 字段） | 7.6-13.7x / 11.7-17.4x | `plans/00-项目进度.md` |
| 2.5 | E1-E3（Bytes/Tell） | ~10-12x | `plans/00-项目进度.md` |
| 3 | BitStruct | 12x / 17x | `plans/00-项目进度.md` |
| 4 | Array 系列 | 重审中（用户打回） | `plans/phase4-array/总纲.md` |

> 详细数据见各 phase 总纲 / 过程记录。性能门禁规则见 `.opencode/skills/performance-gate/SKILL.md`。

---

## 活跃的设计质疑

（无）

> 历史质疑见各过程记录。

---

## 维护规则

- **新增文档**：必须在 `docs/design/` / `docs/decisions/` / `docs/reviews/` / `docs/analysis/` / `plans/phaseN/` 之下，并加 frontmatter
- **状态变更**：phase 状态变更只改 `plans/phaseN/总纲.md`，本文件不记录细节
- **新 ADR**：编号自增，supersede 旧决策时必须在旧 ADR 的 frontmatter 加 `superseded_by`
- **新教训**：写入 `experiences.md`，必须有 ≥1 个真实事件证据
