---
id: ADR-INDEX
status: active
phase: meta
last_updated: 2026-07-28
---

# ADR Index — 设计决策索引

> 本目录存放所有跨阶段设计决策（Architecture Decision Record）。每条决策一个文件，独立可追溯。
> 状态流转：`proposed` → `accepted` → （被替代时）`superseded` / （弃用时）`deprecated`。
> 决策演化通过 `supersedes` / `superseded_by` 字段链接。

## 全部 ADR 一览

### Phase 2：表达式系统

| ADR | 状态 | 决策 | 备注 |
|-----|------|------|------|
| [ADR-001](ADR-001-废弃this.md) | accepted | 废弃 `this`，用编译期字段引用 | |
| [ADR-002](ADR-002-不提供len.md) | accepted | 不提供 `len_`，用 `Tell()` + `Computed()` | |
| [ADR-003](ADR-003-嵌套跨层显式context.md) | accepted | 嵌套跨层引用用显式 `context=` | |
| [ADR-004](ADR-004-三种field函数.md) | accepted | 三种 field 函数 RW/RO/WO | |
| [ADR-005](ADR-005-default自动kw_only.md) | accepted | `default` 自动 `kw_only` | |
| [ADR-006](ADR-006-表达式VM仅i64.md) | accepted | 表达式 VM 仅 i64，无浮点/lambda/字符串 | |

### Phase 2.5：性能优化

| ADR | 状态 | 决策 | 备注 |
|-----|------|------|------|
| [ADR-007](ADR-007-Context-Vec化.md) | accepted | Context Vec 化：GetInt 走编译期索引数组 | |
| [ADR-008](ADR-008-parse借用实例dict.md) | accepted | parse 借用实例 `__dict__` 直接填充 | 历史修订见 `docs/archive/phase1/` |

### Phase 3：BitStream

| ADR | 状态 | 决策 | 备注 |
|-----|------|------|------|
| [ADR-009](ADR-009-bit顺序默认MSB.md) | accepted | bit 顺序默认 MSB-first | |
| [ADR-010](ADR-010-剩余位不自动padding.md) | accepted | 剩余位不自动 padding | |

### Phase 4：Array

| ADR | 状态 | 决策 | 备注 |
|-----|------|------|------|
| [ADR-011](ADR-011-ListContainer原生list.md) | accepted | ListContainer 返回原生 list | |
| [ADR-012](ADR-012-StopField用Result哨兵.md) | accepted | StopField 用 Result 哨兵变体 | |
| [ADR-013](ADR-013-RepeatUntil-PyCallable路径.md) | **superseded** | RepeatUntil PyCallable 谓词 Container 包装（v4） | 被 ADR-014 替代（v5 用户打回） |
| [ADR-014](ADR-014-RepeatUntil-终止表达式.md) | accepted | RepeatUntil 终止表达式 = Phase 2 表达式（v5 用户硬约束） | supersedes ADR-013 |
| [ADR-015](ADR-015-Element仅作为构造器字段.md) | accepted | Element 仅作为构造器字段（v5） | 与 ADR-011（Index 作为字段）平行 |

### 跨阶段：已验证模式规范

| ADR | 状态 | 决策 | 备注 |
|-----|------|------|------|
| [ADR-016](ADR-016-P0-3-lazy-path错误传播.md) | accepted | P0-3 lazy path 错误传播模式 | Phase 1 验证，Phase 4.7 推广 |
| [ADR-017](ADR-017-Vec中转PyList构建.md) | accepted | Vec 中转 PyList 构建模式 | Phase 4.7 验证 |
| [ADR-018](ADR-018-index-save-restore配对.md) | accepted | `_index` save/restore 配对模式 | Phase 4 验证，4.5 v5 整合为 Context::restore_index |
| [ADR-019](ADR-019-错误抛出fast-path.md) | accepted | 错误抛出 fast-path（绕过 Python `__init__` 直接构造异常实例） | Phase 4.x 验证（a_err_eof 11.34x，首次 unsafe raw FFI） |
| [ADR-020](ADR-020-CI冒烟门禁方法学.md) | accepted | CI 冒烟门禁方法学（4 层门禁 + Controlled A/B Test 性能回归检测） | META-CI 整体验证（4 子任务 1a/1b/1c/1d，L-03 + L-09 对策工程化） |

## 状态图例

- `proposed` — 已提出，待讨论
- `accepted` — 已接受，当前有效
- `superseded` — 被新 ADR 替代（看 `superseded_by`）
- `deprecated` — 弃用，无替代

## 维护规则

- 新增 ADR：编号自增（下一号 21）
- 修订决策：**不修改原 ADR**，新建 ADR 并在原 ADR 加 `superseded_by`，新 ADR 加 `supersedes`
- 模式采用：新设计文档必须在 frontmatter `depends_on` 列出相关 ADR
- 教训关联：若决策源于失败教训，在 ADR 的 Relations 段引用 `harness/experiences.md#L-XX`
