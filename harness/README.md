---
id: README-harness
status: active
phase: meta
last_updated: 2026-07-29
---

# harness/ — Agent Harness 资产集中目录

> iter10（2026-07-29）引入。集中所有 Agent Harness 相关资产（HARNESS.md §7 组件归类）。
> opencode 框架约束的文件（AGENTS.md / opencode.json / .opencode/）仍在根目录，本目录承载其余 harness 资产。

## 目录结构

```
harness/
├── MEMORY.md                # L0 索引（项目长期记忆入口）
├── experiences.md           # L1 教训（L-01~L-11 模式化失败）
├── metadata-convention.md   # 文档元数据规范（frontmatter + 目录结构 + 强制阈值）
├── extensions/              # 项目特定 agent extension（6 个角色）
│   ├── pm-extension.md
│   ├── architect-extension.md
│   ├── developer-extension.md
│   ├── reviewer-extension.md
│   ├── vetter-extension.md
│   └── auditor-extension.md
├── manifests/               # AHE Change Manifest（iter1~iter10 历史 + 扫描报告）
│   ├── change_*.json
│   └── iter10-scan-report.md
└── evaluations/             # AHE dogfood evaluate 轨迹
    └── eval-*.md
```

## 启动加载

任何角色启动时，AGENTS.md §4 入口会按序指向本目录：
- `harness/MEMORY.md`（L0 索引）
- `harness/experiences.md`（L1 教训）
- `harness/extensions/<role>-extension.md`（项目特定）
- `harness/metadata-convention.md`（过程记录填写时）

## 维护责任

| 文件 | 维护者 |
|------|--------|
| MEMORY.md / experiences.md | PM（LTM 唯一维护者） |
| metadata-convention.md | PM |
| extensions/<role>-extension.md | 各角色（自己维护） |
| manifests/ | PM（历史快照，不回溯修改） |
| evaluations/ | PM（每 AHE iteration 产出） |

## 不在本目录的 harness 资产（opencode 框架约束）

- `AGENTS.md`（根目录，opencode instructions 入口）
- `opencode.json`（根目录，opencode 配置）
- `.opencode/agents/`（6 个 base agent，opencode 固定路径）
- `.opencode/skills/`（项目自定义 skill，opencode 固定路径）
