---
id: CONVENTION-frontmatter
status: active
phase: meta
last_updated: 2026-07-29
---

# 文档元数据规范（frontmatter）

> AHE iteration 3 引入。所有 `docs/` 下文档和 `plans/phaseN/过程记录-*.md` 必须包含 YAML frontmatter。
> AHE iteration 9（2026-07-29）补强：加 §5.1 目录结构约定 + §9 强制拆分阈值（关闭 L-10 候选"规范无 enforcement"漏洞）。
> 本规范由 harness/metadata-convention.md / §10 引用。

## 1. 为什么需要 frontmatter

AHE 三层可观测性的"组件可观测性"要求每个工件可被机器解析。frontmatter 让 agent 启动时能：

- `grep "status: superseded"` 找出所有已废弃决策
- `grep "phase: 4"` 找出 Phase 4 相关工件
- 通过 `depends_on` / `supersedes` 构建依赖图
- 通过 `last_updated` 判断文档新鲜度

## 2. 通用 schema

所有 frontmatter 必须包含 `id` / `status` / `last_updated`。其他字段按文档类型可选。

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `id` | string | ✅ | 全局唯一标识。格式 `<TYPE>-<NNN>` 或 `<TYPE>-<name>` |
| `status` | enum | ✅ | `active` / `superseded` / `deprecated` / `archived` / `draft` / `accepted` / `rejected` / `pending` |
| `phase` | string | ✅ | 关联 Phase（`0`/`1`/`2`/`2.5`/`3`/`4`/`meta`/`cross`） |
| `last_updated` | date | ✅ | YYYY-MM-DD |
| `depends_on` | string[] | 可选 | 依赖的其他文档 id 列表 |
| `supersedes` | string[] | 可选 | 本文档替代的旧文档 id 列表 |
| `superseded_by` | string[] | 可选 | 本文档被哪个新文档替代 |
| `tags` | string[] | 可选 | 自由标签 |

## 3. 文档类型 id 前缀

| 前缀 | 类型 | 位置 |
|------|------|------|
| `DESIGN-*` | 设计文档 | `docs/design/` |
| `ADR-*` | 决策记录 | `docs/decisions/` |
| `REVIEW-*` | 架构审查 | `docs/reviews/` |
| `ANALYSIS-*` | 分析报告 | `docs/analysis/` |
| `TRACE-*` | 过程记录（per-子任务） | `plans/phaseN/traces/X.Y-name.md` 或 `plans/phaseN/过程记录.md`（历史 phase） |
| `TRACE-meta-*` | cross-phase 资产过程记录 | `plans/meta/<子项目>/traces/X-name.md` |
| `CONVENTION-*` | 规范约定 | `docs/` 顶层 |
| `LESSON-*` | 教训 | `experiences.md` 内部段 id |

## 4. 设计文档模板

```markdown
---
id: DESIGN-<Name>
status: active            # active / superseded / archived
phase: <N>
depends_on: [DESIGN-xxx, ADR-yyy]
supersedes: []
last_updated: YYYY-MM-DD
---

# 模块设计 - <Name>

（文档正文）
```

## 5. 过程记录模板（per-子任务）

```markdown
---
id: TRACE-<phase>.<task>
phase: <N>
task: "<X.Y 子任务标题>"
status: accepted          # pending / designing / design_reviewed / coding / code_reviewed / accepted / rejected
owners: []                # 角色缩写列表（ARCH / DEV / REV / VET / PM / AUDITOR）
started: YYYY-MM-DD
completed: YYYY-MM-DD     # 留空表示未完成
manifest_refs: []         # 关联的 AHE Change Manifest ch_XXX
---

## 操作日志
- [YYYY-MM-DD] [角色] 操作描述

## [角色专属段落]
（REV / VET / ARCH / DEV / PM / AUDITOR）

## 备注
```

> 注：原 harness/metadata-convention.md 的"状态/负责人/开始时间/完成时间"表格字段全部迁入 frontmatter。正文只保留时序化日志和分析段。

### 5.1 目录结构约定（iter9 引入）

```
plans/
├── 00-项目进度.md                         # 跨 phase 项目总览
├── meta/                                  # cross-phase 资产（非任何 phase 专属）
│   ├── <子项目名>/                        # 如 CI-冒烟门禁 / INV-构造器清单
│   │   ├── 索引.md                        # 子项目导航（含 traces 列表 + 设计/ADR 引用）
│   │   └── traces/                        # 子项目过程记录
│   │       └── X-name.md
│   └── <子项目名>/
├── phaseN-<name>/
│   ├── 总纲.md                            # phase 入口（保持根目录）
│   ├── 索引.md                            # phase 索引（重写为 traces 导航）
│   ├── 分析报告-*.md                      # phase 输入（可选）
│   ├── traces/                            # 【iter9+ 强制】子任务过程记录
│   │   └── X.Y-name.md
│   └── archive/                           # 【iter9+ 强制】历史归档
│       └── 过程记录-archive-YYYYMMDD.md   # 拆分前原文件（status: archived）
└── phase0-3/                              # 历史 phase（保持单文件 过程记录.md，不回溯）
```

**分类准则**：
- **phase 专属内容**（4.0 设计、4.1 实施、4.5 v5 重写等）→ `phaseN/traces/`
- **cross-phase 资产**（META-CI 门禁、META-INV 清单等）→ `meta/<子项目>/traces/`
- **判别问题**：内容会被多个 phase 引用吗？是 → meta/；否 → phase/traces/

**traces/ 文件名规范**：`X.Y-name.md`（如 `4.1-ArrayNode.md`），路径已含语义，不再加"过程记录-"前缀。

## 6. ADR 模板

详见 `docs/decisions/README.md`。

```markdown
---
id: ADR-NNN
status: accepted          # proposed / accepted / deprecated / superseded
phase: <N>
decides: "<一句话决策>"
supersedes: []            # 被本 ADR 替代的旧 ADR
superseded_by: []         # 替代本 ADR 的新 ADR
depends_on: []            # 本 ADR 依赖的前置 ADR
last_updated: YYYY-MM-DD
---

# ADR-NNN: <决策标题>

## Context（背景）
（为什么需要这个决策？面临什么问题？）

## Decision（决策）
（具体决策内容）

## Consequences（后果）
- 正面：
- 负面：
- 中性：

## Relations（关系）
- 引用证据：`docs/xxx.md §Y`
- 关联教训：`experiences.md#L-XX`
- 实施：`<phase>/<task>`
```

## 7. 命名约定

- 文件名：`<中文类型>-<Name>.md` 或 `ADR-NNN-<title>.md`
- frontmatter 的 `id` 必须英文，便于 grep
- 文件正文可中文

## 8. 填写责任

| 文档类型 | 谁填 frontmatter | 谁更新 status |
|---------|----------------|--------------|
| DESIGN-* | ARCH 创建时 | ARCH（superseded 时） |
| ADR-* | ARCH 提出决策时 | ARCH（status 流转） |
| REVIEW-* | REV / VET 创建时 | 创建者 |
| ANALYSIS-* | 创建者 | 创建者 |
| TRACE-* | PM 分派任务时创建骨架 | 各角色按流转更新 |
| TRACE-meta-* | PM 创建子项目时 | PM + 各角色按流转更新 |

## 9. 强制拆分阈值（iter9 引入，AUDITOR 第 8 类审计项）

> **背景**：iter3 §5 per-子任务规范存在但到 iter9 才首次执行（6 个 phase 全部违反），根因是规范无 enforcement。本节补强。

### 触发阈值（满足任一即必须拆分）

1. **行数阈值**：单文件 > **1000 行**
2. **子任务数阈值**：单文件含 > **5 个子任务 H2 段**（`## 子任务` / `## META-` 等）
3. **跨类内容**：phase 专属文件中出现 META 类（cross-phase）内容

### 处理流程

1. PM 识别（任意时机）→ 在过程记录追加 `## SPLIT-FLAG` 段标记
2. PM 启动拆分子任务（按 §5.1 目录结构 + frontmatter 模板）
3. AUDITOR 阶段审计时核查 trace 文件大小趋势（第 8 类审计项）

### 历史 phase 豁免

phase0-3 已完成 + 单文件结构稳定，不强制回溯拆分（成本高于收益）。iter9 仅对 phase4（11837 行异常臃肿）+ META 类（结构性错位）执行。**phase5+ 严格按 §5.1 + 本节执行。**

### AUDITOR 审计依据

- 每 phase 验收时核查：phase 过程记录是否走 §5.1 目录结构（phase5+ 强制）
- 阶段审计时核查：trace 文件大小趋势（max < 1000 行；超阈值必须 PM 标注 SPLIT-FLAG）
- 跨 phase 审计时核查：cross-phase 内容是否归入 plans/meta/（META-XXX 不应堆积在 phase 目录）
