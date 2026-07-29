---
id: TRACE-meta-INV-v1v2
phase: meta
task: "META-INV 构造器清单整理（v1 已作废 + v2 CSV 格式）"
status: accepted
owners: [PM]
started: 2026-07-24
completed: 2026-07-25
manifest_refs: [ch_032]
last_updated: 2026-07-29
note: "iter9 拆分自原 phase4 过程记录.md 行 6738-6826。META-INV 是 cross-phase 资产，从 phase4 抽离到 plans/meta/"
---

## META-INV-1: construct 构造器清单整理 v1（已作废）

| 项目 | 内容 |
|------|------|
| 状态 | 已作废（用户反馈反对 markdown 形态） |
| 负责人 | ARCH |
| 完成时间 | 2026-07-28 |
| 作废原因 | 用户反馈"清单不适合 markdown 文档记录，使用表格模式更佳" → META-INV-2 重做为 CSV |

### 产出（已删除）

- `docs/constructors-inventory.md`（800 行，PM 已删除）

### 复用价值

v1 调研的全部数据（构造器全集 + 各 phase 性能数据 + 5 项关键发现）被 v2 CSV 完整复用。

---

## META-INV-2: construct 构造器清单整理 v2 - CSV 格式

| 项目 | 内容 |
|------|------|
| 状态 | 已完成（PM ACCEPTED） |
| 负责人 | ARCH（PM 独立验证） |
| 完成时间 | 2026-07-28 |
| 任务性质 | 调研整理（trivial，PM 标注跳过 DESIGN_REVIEW） |
| 委托记录 | ARCH 不可写 plans/，本段由 PM 代为 append |

### 操作日志

- [2026-07-28] [ARCH] 数据源复查（无重新调研，复用 META-INV-1 数据）：construct/__init__.py + construct-rs 已导出 + 22 个 Node 源文件 + 各 phase 总纲/过程记录 + plans/00-项目进度.md
- [2026-07-28] [ARCH] 分批写入两份 CSV（每批 ≤300 行）
- [2026-07-28] [PM] 独立验证：UTF-8 without BOM ✓ / LF 换行 ✓ / 列数合规（135 行×13 列 + 145 行×17 列，仅末尾 trailing newline）/ 数据样本正确

### 产出

| 文件 | 行数（含 header） | 用途 |
|------|------|------|
| `docs/constructors-inventory.csv` | 136 | 构造器全集，一行一构造器 |
| `docs/perf-scenarios.csv` | 145 | 场景级测量点，一行一场景 |

### 数据点统计

**主清单 134 个构造器**：implemented 40 / partial 2 / not_implemented 92

**性能场景 144 个测量点**：meets_10x=true 98（67.9%）/ meets_10x=false 46（32.1%）

### PM 独立核查发现（重要项目级洞察）

PM 在 CSV 验收时按 phase 聚合 < 10x 场景，发现项目级内部一致性问题：

| Phase | < 10x 测量点数 | 状态 |
|-------|--------------|------|
| 1 | 7 | ⚠️ 已 ACCEPTED + tag `phase-1-complete`，但 7 个测量点 <10x |
| 2 | 5 | ⚠️ 已 ACCEPTED + tag `phase-2-complete` |
| 2.5 | 6 | ⚠️ 已 ACCEPTED + tag `phase-2.5-complete` |
| 3 | 0 | ✅ 全部 ≥10x |
| 4 | 4 | 🔴 重审中（已知问题） |
| 4.4 | 4 | 🔴 重审中（4.7 已修复 3 个） |
| 4.5 | 14 | 🔴 v4 历史 PyCallable 数据（已被 v5 替代，历史快照非现状） |
| 4.6 | 6 | 🔴 收尾中 |

**关键洞察**：用户硬约束 #5 "禁止局部门禁" 的追溯适用范围未明确：
- 若仅适用 Phase 4 起则 Phase 1-3 历史 < 10x 可接受（硬约束前已 ACCEPTED）
- 若追溯适用则 Phase 1-3 也应复审

PM 转呈用户决策。CSV 如实反映项目状态（不掩盖）是 L-02 对策的正面实践。

### 与 META-INV-1 调研结果对照

**4 个关键发现全部保留**（在 CSV notes 列）：
- Phase 2-3 设计计划 4 个未实现 Node 变体（Const/Switch/IfThenElse/ContextParam）
- Phase 2.5 双口径（Vec 化前 vs 后）
- Transform 部分实现（仅 BitsSwapped/ByteSwapped 特化）
- Phase 4 v2 4 个 <10x 场景 3 个已 4.7 修复

**调整项**：Byte/Short/Int/Long 别名归类（由"已实现 alias" → not_implemented，更严格反映用户面状态）

### 备注

- 不创建配套 markdown（用户明确反对）
- 维护规则由 PM 后续写入 `harness/extensions/{pm,architect,auditor}-extension.md`
- CSV 顶部不加注释行（纯 header + data，工具兼容）
- 性能数据全部从源文件复制原值，未估算（L-02 对策）
- 数据来源指针精确到 文件:行号

---

