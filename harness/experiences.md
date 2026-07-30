# Long-Term Memory — 跨阶段经验教训

> **HARNESS.md 组件 7：Long-Term Memory**
>
> 本文件是项目跨会话的持久知识存储。每条教训都是从真实失败中提炼的模式化根因，
> 非"最佳实践"或"理论建议"。每条教训必须能追溯到具体的失败事件（日期/Phase/证据）。
>
> **Agent 使用方式**：
> - 任何 agent（PM/ARCH/DEV/REV/VET/AUDITOR）启动时必须读取本文件
> - 在做决策前，对照本文件检查"当前情境是否匹配某条教训的模式"
> - 若匹配，必须主动应用对策，不可重复同一失败
> - 教训按"反复出现的次数 + 后果严重度"排序，越靠前优先级越高

---

## 教训索引（按优先级）

| ID | 模式名 | 反复次数 | 后果 | 对策所在节 |
|----|--------|---------|------|-----------|
| L-01 | 中间表示层违反 | 3+ | 推倒重来 / 性能 0.3-1.8x | §L-01 |
| L-02 | 理论估算替代实证数据 | 3+ | Phase 15 验收失败 / 设计返工 | §L-02 |
| L-03 | PM 接受不对等证据 | 2+ | Phase 15 错误验收 + tag | §L-03 |
| L-04 | 跨阶段模式未沉淀 | 2+ | Phase 4 重新决策 + 返工 | §L-04 |
| L-05 | 优化 A 路径忽略 B 路径 | 2+ | 设计修订 R1/R2 漏洞 | §L-05 |
| L-06 | 字段数混淆对照 | 1 | Phase 3 不对称误判 | §L-06 |
| L-07 | AHE predicted_impact 重结构轻交叉引用 | 3+ | iter3 漏报 42 / iter9 漏报 200 broken refs | §L-07 |
| L-08 | AHE 规范解读层错误（通用 vs 项目自定义混淆） | 2+ | iter5 误判 AUDITOR 缺位 / iter4 v1 误改通用 skill | §L-08 |
| L-09 | 跨时段性能对比消除法归因失效（边界场景） | 1+ | 4.6 E01 O1 回归误判（VET H1 错误论证"cargo 未重编译"） | §L-09 |
| L-10 | 规范存在 ≠ 实际执行（无 enforcement） | 1+（iter3→iter9） | §5 per-子任务规范 3 年未执行 / phase4 过程记录累积到 11837 行 | §L-10 |
| L-11 | PM 对用户指令的语义误判 | 1（iter10） | iter10 一度误标 DEFERRED（用户指令"关闭"被理解为"取消"，实际是"完成"） | §L-11 |
| L-12 | PM 角色越界深入技术/代码细节 | 1（Phase 6 立项） | PM 自己 grep Python 源码查构造器依赖（应分派 ARCH），违背 pm.md base §PM 不做的事 | §L-12 |
| L-13 | docstring 未跟随语法演进 | 1（Phase 7 审查） | Phase 2 废弃 `this` 但 15+ 处 docstring 示例残留 `this.xxx`，用户审阅时发现 | §L-13 |
| L-14 | 设计硬约束认知需交叉验证 | 1（RawCopy 质疑） | ARCH 把"CPython bytes 不可变→必须拷贝"当硬约束，漏了 Rust 内置 hashfunc 零拷贝路径 | §L-14 |

---

## L-01: 中间表示层违反

> **核心约束**（AGENTS.md §0）：parse 直接从 bytes 构造 PyObject；build 直接从
> PyObject 读属性写字节；**禁止**在 Rust 侧引入独立的中间数据类型再转换。

**模式描述**：实现或设计在 Python↔Rust 边界引入了"中间数据类型"（Rust enum、
跨 FFI 返回的 dict、per-field trait 抽象层），导致每次字段操作都跨越 FFI 边界，
累积开销使 Python 端性能仅为原版的 0.3-1.8x。

**反复出现事件**：

| # | 时间 | 事件 | 证据 |
|---|------|------|------|
| 1 | 2026-06-21 | **推倒重来**：1.5-1.8 实现将解析结果先构造为 Rust 中间类型再逐个转换为 Python 对象，并在输入/输出侧引入 trait 抽象层使每个字段都跨越 FFI 边界。性能 0.3-1.8x。 | AGENTS.md §0 历史教训 |
| 2 | 2026-06-22 | **Phase 1 设计修订 R3**：当前实现（1.5-1.8）的 `_parse_raw` 返回 dict 到 Python，Python 侧 `cls(**dict)`——dict 跨越 FFI 边界返回。ARCH/REV/PM 三方审查均未发现此违反。 | docs/archive/phase1/设计修订-parse路径优化.md §0 |
| 3 | 2026-06-19 | **Phase 15 验收失败**：PyDictSink 仍 per-field 调 PyDict_SetItem，FFI 调用次数未减少。parse 0.26x / build 0.08x。 | performance-gate/SKILL.md §历史教训 |

**根因**：
1. "中间表示"的边界判断不直观——Rust 内构造 PyDict 作为临时对象不算违反（它是 Python 对象本身的状态），但跨 FFI 返回 dict 算违反。设计阶段易混淆。
2. 三方审查（ARCH/REV/PM）在缺乏显式 §0 对照表时，倾向于"功能正确即可"，不主动检查每条 §0 原则。

**对策**（agent 必须执行）：
- **ARCH**：设计文档中涉及 parse/build 数据流时，必须包含"§0 原则对照表"（见 docs/archive/phase1/设计修订-parse路径优化.md §0.4 模板），逐条说明设计如何满足每条 §0 原则。
- **REV**：设计检视时，"§0 原则对照"是必查项——若设计文档未提供对照表，直接驳回。
- **DEV**：实现时若发现需要在 parse 返回 dict / build 接收 dict 跨 FFI，立即标记 `[设计质疑]`，不可自行实现。
- **PM**：验收 S-PERF 时，若性能 < 0.5x，优先怀疑中间表示层违反，分派 ARCH 做对照表审查。

---

## L-02: 理论估算替代实证数据

> **核心约束**（performance-gate/SKILL.md Checkpoint 1）：性能假设必须有量化数据支撑
> （profiling/benchmark），非纯理论推算。

**模式描述**：设计阶段用"预计 Nx 加速"等理论推算代替实测数据，REV 接受后实现
完成才发现性能远低于预期。或者，PM 在缺乏实测对比的情况下用"编译通过"作为性能达标证据。

**反复出现事件**：

| # | 时间 | 事件 | 证据 |
|---|------|------|------|
| 1 | 2026-06-21 | 推倒重来前的设计声称理论加速，实际 0.3-1.8x。 | AGENTS.md §0 |
| 2 | 2026-06-19 | Phase 15 设计声称"5000x 加速"（推算），REV 接受。实际 0.26x。 | performance-gate/SKILL.md §历史教训 |
| 3 | 2026-06-28 | Phase 3 parse/build 不对称：前一轮调查将根因归为 tp_new（理论），被 Phase 2.5 数据推翻。需 7 组隔离实验才定位。 | docs/analysis/phase3-parse-build不对称.md §1.2 |

**根因**：
1. 理论估算容易给出吸引人的数字（"5000x"），但忽略了实际瓶颈（如 CPython 小整数缓存、pyo3 i128 任意精度路径）。
2. REV 在缺乏 profiling 数据时倾向于信任 ARCH 的推算。
3. "比旧路径快 X 倍"是相对基线，若旧路径本身很慢，X 倍仍可能不达标。

**对策**：
- **ARCH**：性能假设章节必须包含"瓶颈识别（量化数据 + 来源）"——不接受"我们认为 FFI 是瓶颈"。
- **REV**：检视清单明确检查"瓶颈识别有量化数据支撑"——非纯理论推算；检查基线是绝对基线（Python 原版）而非相对基线。
- **PM**：执行 Checkpoint 2 性能烟雾测试（首个端到端实现后），< 0.5x 暂停后续阶段。

---

## L-03: PM 接受不对等证据

> **核心约束**（performance-gate/SKILL.md Checkpoint 3）：性能假设必须有量化数据支撑
> S-PERF "≥Xx" 标准要求"对比数据表（多场景 × vs 绝对基线）"作为证据。

**模式描述**：PM 在验收时接受了类型不匹配的证据——如标准是"≥4x 性能"，
DEV 报告"bench 编译通过 / 1 个 test passed"，PM 标注"通过"并打 tag。

**反复出现事件**：

| # | 时间 | 事件 | 证据 |
|---|------|------|------|
| 1 | 2026-06-19 | Phase 15：S-PERF-2 "Python 路径 ≥1.0x"，DEV 报告"bench 编译通过，无对比基准"，PM 接受并打 tag。实际 parse 0.26x / build 0.08x。 | performance-gate/SKILL.md §历史教训 |
| 2 | 2026-07-11 | 架构审查发现：P0-3 lazy path 模式在 Phase 1 验证后未沉淀，导致 Phase 4 重新决策、重复实现，直到性能不达标才补救。PM 验收 Phase 1 时未检查"模式是否已沉淀"。 | docs/reviews/架构审查-重复代码与抽象质量.md §1.2 |

**根因**：
1. PM 倾向于信任其他角色的结论（"DEV 报告通过"），而非独立验证证据类型。
2. 验收清单中"证据类型匹配"未硬编码为强制检查项。

**对策**：
- **PM**：验收时严格执行证据类型对照表（`performance-gate/SKILL.md` + `pm.md §指标验收`）。证据类型不匹配 → 驳回，不可降低标准。
- **AUDITOR**：审计 PM 验收时，"证据类型匹配"是必查项——若 PM 接受了类型不匹配的证据，AUDITOR 必须驳回。
- **PM**：含性能标准的子任务验收，必须在过程记录中写入性能数据分析记录。**无分析记录的 ACCEPTED 状态无效。**

---

## L-04: 跨阶段模式未沉淀

> **核心约束**：新模式一旦在某个 Phase 验证，必须沉淀到 `docs/decisions/`（ADR-NNN）
> 或本 experiences.md，避免后续 Phase 重新决策。

**模式描述**：某个 Phase 中验证了优秀的模式（如 lazy path 优化），但未写入决策
记录。后续 Phase 遇到类似情境时，重新决策不用、重复实现，直到性能不达标才回头补救。

**反复出现事件**：

| # | 时间 | 事件 | 证据 |
|---|------|------|------|
| 1 | 2026-07-11 | **P0-3 lazy path 模式**：Phase 1 中由 StructNode 验证（成功路径不维护 Path 栈，子节点 Err 时重建）。未写入设计决策记录。Phase 4 Array 系列 4 个节点（array/greedy_range/prefixed_array/repeat_until）重新决策不用，重复实现 4 份 `restore_index` + 8 处 lazy path 错误传播。直到 4.7 性能不达标才补救。 | docs/reviews/架构审查-重复代码与抽象质量.md §1.2 |
| 2 | 2026-06-22 | Phase 1 设计修订 R3 的方案 B'（pydantic-core 式实例构造）在 R1/R2 中未被识别——直到 PM 指出 §0 违反才调查 pydantic-core。pydantic-core 的做法本应在 Phase 0 架构设计阶段就沉淀为参考。 | docs/archive/phase1/设计修订-parse路径优化.md R3 修订记录 |

**根因**：
1. Phase 验收清单只检查"功能/性能达标"，未检查"可复用模式是否已沉淀"。
2. 设计决策记录（`docs/decisions/`（ADR-NNN））的更新无明确触发时机。

**对策**：
- **PM**：Phase 验收清单新增"模式沉淀检查"——若本 Phase 验证了新的可复用模式（性能优化、错误处理范式、数据结构选择），必须确认已写入 `docs/decisions/`（ADR-NNN） 或本 experiences.md。
- **ARCH**：每个 Phase 设计时，先读取 `docs/decisions/`（ADR-NNN） 和本文件，确认是否已有可复用模式。避免重新决策。
- **AUDITOR**：审计 Phase 验收时，检查"模式沉淀"项是否执行。

---

## L-05: 优化 A 路径忽略 B 路径

> **核心约束**（performance-gate/SKILL.md §常见陷阱 1 + reviewer.md §性能检视）：
> 可证伪预测必须覆盖**所有**瓶颈来源，不仅被优化的路径。

**模式描述**：设计声称优化了某条 FFI/拷贝/转换路径，但未分析其他路径是否仍存在。
实现后总性能未达标，因为未被优化的路径成为新瓶颈。

**反复出现事件**：

| # | 时间 | 事件 | 证据 |
|---|------|------|------|
| 1 | 2026-06-22 | Phase 1 设计修订 R1/R2：优化了表达式求值的 FFI，但未分析 PyDict_SetItem 仍是 per-field FFI。R3 才补全。 | docs/archive/phase1/设计修订-parse路径优化.md R3 修订记录 |
| 2 | 2026-06-19 | Phase 15：设计声称消除表达式 FFI，但 PyDictSink 仍 per-field 调 PyDict_SetItem。FFI 调用次数未减少。 | performance-gate/SKILL.md §历史教训 |

**根因**：
1. 设计文档倾向于聚焦"本次优化的亮点"，对其他瓶颈来源只做泛泛描述。
2. REV 检视时未主动列出所有 FFI/拷贝/转换来源做对照。

**对策**：
- **ARCH**：性能假设章节必须列出设计中提到的**所有** FFI/拷贝/转换来源，每个来源都在"可证伪预测"中有对应声明。
- **REV**：检视清单明确——"列出设计中提到的所有 FFI/拷贝/转换来源，确认每个来源都在预测中有对应声明"。这是硬性检查项，不通过则驳回。
- **PM**：收到性能数据时，若整体未达标但被优化路径声称已改善，怀疑存在未优化路径成为新瓶颈（触发 pm-performance-validation skill）。

---

## L-06: 字段数混淆对照

**模式描述**：性能对比时混合了不同字段数的场景，导致结论错误。

**事件**：

| # | 时间 | 事件 | 证据 |
|---|------|------|------|
| 1 | 2026-06-28 | Phase 3 parse/build 不对称分析：PM 起初认为 Phase 3 多出 ~56ns 不对称是 Phase 3 引入的新问题。ARCH 指出对比存在字段数混淆——Phase 1 B7 是 0 字段、Phase 2.5 E1/E3 是 2-6 字段、Phase 3 是 1-3 字段。字段数不同时"每字段 build 累积开销"会抵消"StructNode 固有不对称"。需 7 组隔离实验才定位。 | docs/analysis/phase3-parse-build不对称.md §1.2-§1.3 |

**根因**：
1. 性能对比未控制变量（字段数、输入大小）。
2. 数据表呈现时未标注关键变量差异。

**对策**：
- **PM**：呈现性能数据时，必须标注每个场景的关键变量（字段数、输入大小、复杂度）。比较"差距"时先确认变量一致。
- **ARCH**：分析性能差距时，先做"变量隔离假设"——若数据可由已知变量差异解释，无需引入新假设。
- **PM**：触发 pm-performance-validation skill 时，检查"内部数据关系"维度——同字段数的场景成本应一致。

---

## L-07: AHE predicted_impact 重结构轻交叉引用

> **核心约束**（`.opencode/skills/construct-rs-ahe-practices/SKILL.md` §A1）：涉及文件移动/重命名/重组的 harness 修改，
> 必须在 `predicted_impact` 中列出所有可能 broken 的交叉引用。

**模式描述**：AHE Change Manifest 的 `predicted_impact` 字段在结构改造类修改中，
倾向于只预测"组件可观测性提升 / 检索成本下降"等结构性收益，遗漏"历史文件内部互相引用的旧路径会 broken"。

**反复出现事件**：

| # | 时间 | 事件 | 证据 |
|---|------|------|------|
| 1 | 2026-07-27 | **iteration 3 broken refs 风暴**：T1+T2+T3 文档 AHE 化重组后 commit，verify 时发现 42 处 broken refs 在活跃文件中（phase 总纲/过程记录/docs 内部互引），9 处 frontmatter 缺失。predicted_impact 完全未预测。已批量修复（16 active + 3 archive 文件路径替换 + 9 frontmatter 补加）。 | `harness/manifests/change_2026-07-27-docs-restructure.json` verification.regressions_observed |
| 2 | 2026-07-27 | **iteration 1 manifest pending 遗留**：iteration 1（experiences.md / AUDITOR 注册 / skill 规范化）的 verification 字段一直为 `pending`，从未做过验证。直到 iteration 3 收尾时按用户质询才补做。这是"verify 流程被跳过"的同根模式——predicted_impact 没要求"commit 前自验证"。 | `harness/manifests/change_2026-07-27.json` verification.status（此次补做后改为 verified） |
| 3 | 2026-07-29 | **iteration 9 启动前主动扫描发现 200 处 broken refs**：iter9（过程记录拆分 + 目录重组）启动前 PM 主动分派 explore agent 做 cross-ref 全量扫描（§A1 对策），识别 200 处潜在 broken（远超 iter3 的 42 处）+ 6 个风险点（R1-R7，含 R3 隐藏代码依赖 run_l4_consistency.py C5/C6）。**由于启动前扫描，所有 broken 在 commit 前批量修复，无 post-commit surprise**——L-07 对策 §A1 首次完整生效。但 137 处 P0 CSV 行号引用 + 1 处隐藏代码依赖（C5/C6 校验逻辑）仍需手工逐项处理（explore agent 不能自动修），实际修复耗时远超预期。 | `harness/manifests/change_2026-07-29-trace-split.json` cross_reference_check |

**根因**：

1. **通用 AHE skill 流程抽象**：`.opencode/skills/agentic-harness-engineering/SKILL.md` 工作流 2 的"步骤 5 预测影响"是抽象描述（"哪些任务应该修复，哪些可能回归"），没有针对"文件重组"这类高频修改场景给出**强制检查项**。manifest schema 中 `at_risk_regressions` 是开放列表，agent 凭直觉填——直觉通常聚焦"功能回归"，遗漏"路径回归"。通用 skill 不应被项目污染，需要项目级补充。
2. **Verify 工作流触发条件过严**：通用 skill 工作流 3 原要求"到了 scheduled_at 时间"才验证，但 scheduled_at 通常设为几周后。commit 前的自验证不在流程中，导致问题积压到下次审计才发现。
3. **agent 单次操作聚焦"做修改"，不主动扫描"修改后果"**：grep 全工作区的 broken refs 不在标准工具链中。

**对策**（agent 必须执行）：

- **PM（AHE iteration 执行者）**：每次 commit 前必须执行 `.opencode/skills/construct-rs-ahe-practices/SKILL.md` §B1 自动扫描：
  - Broken refs 扫描：`grep -r "\.md" --include="*.md" . | 验证目标存在`
  - Frontmatter 完整性扫描：所有规范要求 frontmatter 的文件必须有 `id` / `status` / `phase` / `last_updated`
- **PM**：涉及文件移动/重命名/重组时，在 `.opencode/skills/construct-rs-ahe-practices/SKILL.md` §A1（Cross-Reference Migration Check）中**强制**用 grep 列出所有引用，作为 `at_risk_regressions` 的具体条目
- **AUDITOR（审计 AHE iteration 时）**：检查 manifest 的 `predicted_impact.at_risk_regressions` 是否覆盖了"路径 broken"类别。若修改涉及文件重组但 at_risk_regressions 为空或仅含功能项 → 驳回（§B3）
- **任何角色**：发现 manifest predicted_impact 漏报回归时，必须在 verification 的 `false_predictions` 字段如实记录（§B2），**不可静默修复**

---

## L-08: AHE 规范解读层错误（通用 vs 项目自定义混淆）

> **核心约束**（HARNESS.md §演化循环 + AGENTS.md §1 工作流管道）：AHE 规范（HARNESS.md / 通用 AHE skill）
> 定义跨 Agent 的通用要求；项目 AGENTS.md / 项目级 skill / `.opencode/agents/*.md` 定义项目特定角色与流程。
> 两者不可混淆——通用工具不可被项目特定内容污染，项目角色不可被当作通用规范要求。

**模式描述**：PM 在执行 AHE iteration 时，混淆"通用 AHE 规范要求"与"项目自定义角色/流程"，
产生两类相反方向的错误：
- **错误 A（项目→通用误读）**：把项目自定义角色当作 AHE 规范要求。如：要求项目 AUDITOR 审计 AHE iteration（AHE §演化循环根本无 AUDITOR 角色）
- **错误 B（通用←项目污染）**：把项目特定内容写入通用工具。如：把 L-07 对策写进通用 AHE skill 的工作流

**反复出现事件**：

| # | 时间 | 事件 | 证据 |
|---|------|------|------|
| 1 | 2026-07-27 | **iter5 准备阶段（错误 A）**：PM 把"4 次 iteration 无 AUDITOR 审计"当作结构性漏洞，提议用项目 auditor agent 审计 manifest。用户驳斥："AUDITOR 是否 AHE 规定的 auditor，还是只是项目优化前的 auditor，这两者不一定等价"。查 HARNESS.md §演化循环 + 通用 AHE skill 工作流 3 确认：AHE Verify 是"执行者自我验证 + 下一轮 Evaluate 时间独立"，根本无 AUDITOR 角色。项目 AUDITOR 是业务流程审计员（`auditor.md` "审 PM 的管理"），与 AHE iteration verify 流程不等价。 | 本 iteration manifest `ch_014` / 用户对话 |
| 2 | 2026-07-27 | **iter4 v1（错误 B）**：PM 把 L-07 对策直接写进通用 AHE skill 的工作流 2/3。用户驳斥："改 AHE 的 SKILL.md，这符合 AHE 的做法吗？" → revert。当时归因为"通用工具 vs 项目实践边界"，实际是 L-08 同一根因（AHE 规范解读层错误）。 | `harness/manifests/change_2026-07-27-skill-iteration-lessons.json` ch_012 |

**根因**：

1. **harness/MEMORY.md Harness 组件清单未标注"规范来源"**：agent 看到清单时无法分辨哪些来自 HARNESS.md 硬要求、哪些是项目自定义
2. **通用 AHE skill 与项目角色之间无显式映射文档**：PM 在 AHE iteration 中默认"项目有的角色/流程都适用于 AHE"
3. **通用 AHE skill 工作流 3（Verify）描述了"步骤"但未约束"谁执行"**：agent 倾向于套用项目角色，而非按 AHE 规范的"执行者自我验证"
4. **PM 跳过 Evaluate 步骤**：iter1-4 全是凭直觉/审计发现改进，未跑真实任务收集轨迹。Evidence-Driven 原则缺失导致"凭感觉"判断，范畴错误无法被轨迹证伪

**对策**（agent 必须执行）：

- **PM（AHE iteration 执行者）**：每次 AHE iteration 开始前，执行 `.opencode/skills/construct-rs-ahe-practices/SKILL.md` §D "AHE 规范解读检查清单"，明确"本 iteration 涉及的角色/流程，哪些是 AHE 规范要求、哪些是项目自定义"
- **PM**：任何"X 角色未执行 Y 流程"的判断，必须先查 HARNESS.md / 通用 AHE skill 确认 Y 是否 AHE 规范要求；若仅在项目 AGENTS.md / `auditor.md` 等中出现 → 是项目自定义，不适用于 AHE iteration
- **PM**：harness/MEMORY.md Harness 组件清单已新增"规范来源"列，启动时必读
- **PM**：AHE iteration 引用规范条款时，必须显式标注来源（`HARNESS.md §X` / `AHE skill 工作流 Y` / `AGENTS.md §Z` / `construct-rs-ahe-practices §W` / `auditor.md` 等）
- **PM**：每次 AHE iteration 必须先做 Evaluate（即使是最小 dogfood —— iteration 自身执行轨迹），不允许跳过。详见 `construct-rs-ahe-practices §D`

---

## L-09: 跨时段性能对比消除法归因失效（边界场景）

> **核心约束**（performance-gate/SKILL.md Checkpoint 2 性能烟雾测试 + 测量口径"子进程隔离"）：
> 性能对比必须控制变量。当工作量极小（Rust 侧 <300ns）时，消除法归因（"X 没改 → X 不是原因"）失效——
> 跨时段测量环境漂移（CPython 重启 / venv 重建 / CPU 状态 / 时段差异）足以制造 ≥1x 的加速比摆动，
> 而真正的代码效应可能仅在噪声内（<0.3x）。

**模式描述**：

边界场景（如零工作量 / 单字段小 Struct / 空 Array）下，Rust 侧耗时极小（<300ns），
加速比对环境极度敏感（±20ns 即 ±1x 波动）。当出现"跨时段 A vs B 测量值变化 ≥0.5x"时，
通过"消除法"（X 没改 → 排除 X）推断"必定是某个改动 Y 引入"——但**消除法的"X 没改"前提可能错误**
（如 crate 整体 inlining 实际有变化），且**未排除测量环境漂移**这一更大因果源。

具体表现：
1. VET/PM 用消除法归因（"O1 只改 stop_if.rs，E01 不调用 StopIf，所以 O1 不影响 E01"）
2. 论证中的"未重编译/未传播"假设未实测验证（如"cargo 未重编译"论断事实错误——实测 cargo 重编译产生不同 DLL）
3. 跨时段单次测量对比替代同会话 Controlled A/B Test
4. 漏掉阴性对照（与质疑改动无关的场景是否也出现类似摆动）

**反复出现事件**：

| # | 时间 | 事件 | 证据 |
|---|------|------|------|
| 1 | 2026-07-28 | **4.6 E01 O1 回归误判**：4.6 ACCEPTED 后用户质疑 E01 从 4.7 PM 验收 10.56x 变为 VET 复测 9.51x（-1.05x），可能是 O1 引入。VET 之前 H1 排查论断"O1 只改 stop_if.rs，E01 不调用 StopIf，cargo 未重编译"——**"cargo 未重编译"被 Controlled A/B Test 证伪**（A/B 产生不同 DLL：516608 vs 517120 bytes）。但 VET 最终结论方向正确：Controlled A/B Test 实测 E01 O1 off 9.604±0.093x vs on 9.582±0.196x（Δ=+0.023x，远 <0.3x 波动阈值）；E01 rs_ns 204.4 vs 204.6（welch_t<0.2，统计上一致）。根因是 17 天跨度测量环境漂移；阴性对照 i01（与 O1 无关）同会话内出现 0.29x 摆动证明环境漂移足以解释观察差距。 | `docs/analysis/E01-O1-regression-investigation.md` / `plans/phase4-array/traces/4.6-集成测试.md §4.6-E01-INVEST` |

**根因**：

1. **消除法归因在边界场景失效**：当工作量极小时，所有"未改"的代码路径仍可能通过 crate 整体 inlining / codegen units / LTO 决策被间接影响。"X 没改"假设需要实测验证（DLL hash / 汇编对比），不能凭直觉判定。
2. **跨时段测量未做对照**：性能基准（如 4.7 PM 验收时的 10.56x）与复测值（4.6 的 9.51x）通常间隔多日，测量环境（CPython 状态 / venv / CPU 节流）漂移叠加，单次对比无法分离因果。
3. **缺阴性对照**：未同时测量"与质疑改动无关的场景"作为基线。如果无关场景也出现类似摆动，则环境漂移是主因。
4. **PM 倾向于接受角色结论**：VET 给出 H1-H4 排查 + 结论，PM 在 ACCEPTED 时直接采信。如果 VET 的论证路径错误但结论碰巧正确，PM 仍会做出正确决策但理由不立——下次类似情境可能误判。

**对策**（agent 必须执行）：

- **VET / 任何性能调查角色**：怀疑性能回归时，**必须做 Controlled A/B Test**（同会话交替测量 + 阳性对照 + 阴性对照），不可依赖跨时段单次测量对比或消除法归因。
- **VET**：消除法论证中的"未改/未传播"假设**必须实测验证**（如 `git stash` + DLL hash 对比 + 汇编对比），不可凭直觉判定。
- **PM**：验收涉及性能回归判定时，必须确认调查报告含 Controlled A/B Test 数据；仅有消除法归因的判定**驳回**，要求重做受控实验。
- **PM**：调查报告必须有**阴性对照**（与质疑改动无关的场景），证明环境漂移的幅度。
- **ARCH**：性能预测章节涉及 ns 级估算时，必须标注"量级参考"（不可声称"O1 节省 ~10ns"等精确值），因为 ns 级效应常在测量噪声内。

**测量方法学建议**（PM 评估后决定是否写入 performance-gate SKILL 或新 ADR）：

1. 跨时段性能对比必须**标注测量环境**（venv 版本 / Python 重启状态 / 时段 / CPU 型号 / OS 版本）
2. 边界场景（Rust 侧 <300ns）**必须多次采样**（≥5 次），用统计区间而非单点值
3. 怀疑回归时**优先做 Controlled A/B Test**（同会话交替 A→B→A→B 至少 3 轮），统计判据：差异 >0.5x → 回归；<0.3x → 波动；中间加测

---

## L-10: 规范存在 ≠ 实际执行（无 enforcement）

> **核心约束**（`harness/metadata-convention.md §9` iter9 引入）：规范必须有 enforcement 机制——强制阈值 + 审计项 + 工具校验，三者缺一即可被绕过。

**模式描述**：项目级规范文档（如 `harness/metadata-convention.md §5` 的 per-子任务文件模板）虽然定义了"应该怎么做"，但若**没有强制阈值 / 没有 AUDITOR 审计项 / 没有工具校验**，agent 实际执行时会因"短期便利"持续违反规范，导致规范形同虚设，问题累积到爆发才被注意。

**反复出现事件**：

| # | 时间 | 事件 | 证据 |
|---|------|------|------|
| 1 | 2026-07-27 → 2026-07-29 | **§5 per-子任务规范 3 年未执行**：iter3（2026-07-27）引入 `harness/metadata-convention.md §5` per-子任务文件模板（`plans/phaseN/过程记录-X.Y.md` + `TRACE-<phase>.<task>` id 格式），明确禁止单文件过程记录。但到 iter9（2026-07-29）才首次执行——期间 6 个 phase（0/1/2/2.5/3/4）全部使用单文件 `过程记录.md`，phase4 累积到 11837 行/755KB，检索 token 严重浪费。规范存在但无 enforcement：无强制行数阈值、无 AUDITOR 第 8 类审计项、无工具校验。 | `harness/metadata-convention.md §5`（iter3 版本） vs `plans/phase4-array/过程记录.md`（iter9 前单文件 11837 行）|

**根因**：

1. **规范的"建议性"陷阱**：规范文档使用"应该 / 必须"等措辞，但没有"违反时如何检测 / 谁负责检测 / 检测后如何反馈"的闭环。agent 读规范时认为"知道了"，但执行时遇到具体场景（如"这个 phase 还有 1 个子任务就完成，拆 5 个文件 vs 1 个文件"）会选短期便利。
2. **缺少审计触发点**：AUDITOR 第 1-7 类审计项均不涉及"文件大小趋势 / 子任务数趋势"，AUDITOR 阶段审计时不会发现 §5 违反。
3. **缺少工具校验**：CI 的 L4 一致性核查（`run_l4_consistency.py`）检查 CSV 与代码的一致性，但不检查"过程记录文件是否走 §5 目录结构"。
4. **跨阶段遗忘**：iter3 引入 §5 后，后续 phase 启动时 agent 会读规范，但"启动加载"和"实际执行"之间存在大量中间环节（设计/开发/审查），每环节都可能"忘了"§5。

**对策**（iter9 引入，agent 必须执行）：

- **PM**（规范维护者）：新增 / 修改规范时必须同步设计 enforcement 机制：
  - **强制阈值**：明确"超过 X 行 / Y 个段必须拆分"等可量化判据（§9 加 1000 行 / 5 子任务阈值）
  - **AUDITOR 审计项**：新增审计类别指向新规范（§9 加 AUDITOR 第 8 类审计：trace 文件大小趋势）
  - **工具校验**（如可能）：在 `run_l4_consistency.py` 或其他 CI 工具中加新检查项
- **AUDITOR**：每 phase 验收 + 阶段审计时，核查新规范 enforcement 是否实际生效（不仅检查规范文本，更检查执行数据）
- **PM**（启动新 phase 时）：在总纲中显式引用相关规范（如"本 phase 过程记录按 §5.1 目录结构 + §9 阈值执行"），作为 phase 启动检查清单一部分
- **任何角色**：发现规范违反时（即使没有 AUDITOR 指出），必须在过程记录中追加 SPLIT-FLAG 标记

**iter9 enforcement 实施情况**：

- §5.1 加目录结构约定（plans/phaseN/traces/ + archive/ + plans/meta/）
- §9 加强制拆分阈值（1000 行 / 5 子任务 / cross-phase 内容）
- AUDITOR 第 8 类审计：trace 文件大小趋势 + cross-phase 内容归位
- 历史 phase0-3 豁免（不回溯），phase5+ 严格按 §5.1 + §9 执行
- 验证：iter9 phase4 已实际拆分（11837 行 → 19 个 trace，最大文件 1951 行 4.5-RepeatUntil.md，frontmatter note 字段记录豁免理由）

---

## L-11: PM 对用户指令的语义误判

> **核心约束**：PM 处理用户指令时，必须精确解析语义标记（双引号 / 代码块 / 列表等）；遇到歧义动词（"关闭 / 完成 / 结束"）必须区分"完成当前任务"vs"取消当前任务"，不确定时确认而非推断。

**模式描述**：用户用双引号包裹一段文字表示"step 的标题/内容描述"，PM 误将引号内动词理解为"执行动作"。或者用户用简短指令表达复合意图，PM 拆解时丢失关键语义。

**反复出现事件**：

| # | 时间 | 事件 | 证据 |
|---|------|------|------|
| 1 | 2026-07-29 | **iter10 误标 DEFERRED**：用户指令"你加一个 step12：'关闭iter10后，开始Phase5'，iter10留记录即可"。PM 将双引号内的"关闭iter10后"误解为"取消/跳过 iter10"，实际语义是"完成 iter10 后"。PM 一度把 iter10 标记为 DEFERRED + 跳过物理迁移 + 直接启动 Phase 5。用户及时纠正（"我让你做完iter10之后做Phase5，你怎么把iter10停了？"）。 | 本次对话 + iter10 manifest 重写（deferred → in-progress） |

**根因**：

1. **双引号语义未精确解析**：用户用双引号包裹"关闭iter10后，开始Phase5"——这是 step 的**标题/内容描述**（新增 step 的名字），不是"取消 iter10"的执行指令。PM 误将引号内"关闭"理解为"取消"。
2. **歧义动词未区分**：中文"关闭"可指"完成并收尾"（close an iteration）或"中止/取消"（cancel）。PM 选择了后者，未对照上下文（用户明确说"加一个 step"，说明是追加步骤，不是取消现有任务）。
3. **PM 自行推断而非确认**：PM 遇到语义模糊时选择了"推断 + 执行"而非"确认后再执行"。这是 PM 角色最危险的失败模式——PM 是决策者，但决策前提是对指令的精确理解。

**对策**（PM 必须执行）：

- **双引号语义**：用户指令含双引号包裹的文字时，默认理解为**标题/名称/描述**，不是执行动作
- **歧义动词清单**：以下动词必须结合上下文区分"完成 vs 取消"——关闭 / 结束 / 停 / 完 / 收尾 / done / close / finish / stop
- **上下文校验**：遇到歧义动词时，检查上下文是否有"加 / 新增 / 追加"等词（如有，歧义动词大概率是"完成后的动作"而非"取消"）
- **确认优先**：PM 不确定用户意图时，**必须用 question 工具或直接回复确认**，不可自行推断后执行——尤其是涉及"取消/跳过/中止"等不可逆决策时
- **指令复述**：PM 在执行前用一句话复述对用户指令的理解（如"我理解你的意思是 X，对吗？"），给用户纠偏机会

**iter10 实际处理**：

- 用户纠正后，PM 立即恢复 iter10 正常流程（Step 1-11 全部执行）
- iter10 最终状态：verified_with_followups（L4 C1-C10 全 PASS，物理迁移 + cross-ref 修复完成）
- 本教训在 Step 11 沉淀

---

## L-12: PM 角色越界深入技术/代码细节

> **核心约束**（`.opencode/agents/pm.md` §PM 不做的事）：PM 不编写业务代码 / 不深入实现细节（代码如何实现、API 签名、数据结构布局）/ 不做开销拆解 / 不诊断 bug 根因 / 不读代码 diff 来理解实现。**需要这些信息时分派 ARCH/DEV，不自己跑工具查**。

**模式描述**：PM 在做规划/验收/范围决策时，遇到需要"理解代码或技术依赖"的情境，没有分派给 ARCH/DEV，而是自己用 grep / read / bash 工具直接查源码、读实现、分析依赖。结果是 PM 陷入了本应由技术角色承担的细节工作，违反角色边界，效率低下（PM 不如 ARCH 熟悉代码），且偏离 PM 的核心职责（流程管理 / 决策 / 验收）。

**反复出现事件**：

| # | 时间 | 事件 | 证据 |
|---|------|------|------|
| 1 | 2026-07-29 | **Phase 6 立项时 PM 自己 grep Python construct 源码查 Phase 7 构造器依赖**：用户要求"确认 Phase 6 范围是否包含 Phase 7 需要的构造器，避免 Phase 7 返工"。PM 没有分派 ARCH 做依赖核查，而是自己用 `Select-String` / `Get-ChildItem` 反复 grep `construct/lib/*.py` 找 `class If/IfThenElse/Switch/...` 定义。grep 因文件路径/编码问题失败后 PM 还在调整 grep 模式继续查。用户打断："你走远了，这个任务你应该分派做，PM 不应该陷入到具体的代码或任务中。" | 本对话 + pm.md §PM 不做的事 |

**根因**：

1. **角色边界识别失败**：pm.md base 明确列了"PM 不做的事"含"不深入实现细节 / 不读代码 diff 来理解实现"，但 PM 在具体场景下没识别出"查依赖关系 = 深入实现细节"。PM 给自己找了借口："只是查依赖，不是写代码"——但依赖分析属于 ARCH 分析报告范围，PM 应分派不应自做。
2. **"快速自己查更快"的诱惑**：分派 ARCH 有 task 调用开销（写 prompt + 等结果），PM 自己 grep 几秒就有结果。短期看似高效，但：(a) PM 不如 ARCH 熟悉代码，容易漏依赖；(b) PM 越界一次就破坏一次角色边界；(c) grep 失败时 PM 会越陷越深（本事件中 PM 连续调整 grep 模式 3 次仍未停下）。
3. **未对照 base agent 边界**：PM 决策前应对照 pm.md §PM 不做的事，本事件中 PM 完全跳过了这步自检。

**对策**（PM 必须执行）：

- **决策前自检**：PM 遇到"需要理解代码/技术细节"的情境时，先问自己"这件事属于 pm.md §PM 不做的事 吗？"。以下场景**必须分派**，PM 不可自做：
  - 查代码依赖关系（构造器之间的引用 / import / 基类）
  - 读源码理解实现
  - 诊断 bug 根因
  - 评估 API 签名 / 数据结构布局
  - 做开销拆解（哪个函数贡献多少成本）
- **分派优先于自查**：即使"自己 grep 几秒就能查到"，也要分派给 ARCH/DEV。理由：(a) 角色边界不可因效率破坏；(b) ARCH 的分析报告是可追溯的工件，PM 的 grep 不是；(c) ARCH 可能识别出 PM 没想到的依赖维度
- **触发条件清单**（PM 出现以下行为时立即停下，改为分派）：
  - 在 bash 工具里跑 grep / Select-String 查源码（>1 次）
  - 用 read 工具读 .rs / .py 业务代码（非过程记录/总纲）
  - 调整 grep 模式重试（说明第一次没找到，已经陷入"排查"模式）
- **教训对照强化**：PM 决策前自检清单加一条"是否需要深入代码/技术细节？是 → 分派"

**Phase 6 立项实际处理**：

- 用户纠正后，PM 立即把"Phase 6↔Phase 7 依赖核查 + Phase 6 分析报告 + 测试框架设计"打包分派给 ARCH
- 本教训立即沉淀（不等反复 3 次——PM 角色边界破坏会持续侵蚀流程管理质量）

---

## L-13: docstring 未跟随语法演进

> **核心约束**：语法/设计变更时，必须同步清理所有 docstring / 注释 / 示例代码中的旧语法残留。docstring 是用户直接看到的 API 文档，旧语法残留会误导用户。

**模式描述**：项目演进中某个语法或 API 被废弃（如 Phase 2 废弃 `this`，改为字段名直接引用），实现代码和设计文档同步更新了，但 docstring / 注释 / 示例代码中的旧语法未清理。结果用户读 docstring 时看到旧语法，照抄后失败（NameError / ImportError），或对设计意图产生误解。

**反复出现事件**：

| # | 时间 | 事件 | 证据 |
|---|------|------|------|
| 1 | 2026-07-30 | **Phase 2 废弃 `this` 但 docstring 残留**：construct-rs 在 Phase 2 明确废弃 `this.xxx` 语法（改为字段名直接引用 `Bytes(count)`），但 `_descriptors.py`（15+ 处）+ `_conditional.py`（8+ 处）+ ARCH 质疑文档（3 处）的 docstring/示例仍写 `this.xxx`。用户审阅 ARCH 质疑文档时发现并质疑"this 不是废弃了吗"。 | `docs/design/queries/确认-表达式系统this语法.md`（ARCH 全量 grep 确认实现零残留，纯文档笔误） |

**根因**：

1. **变更范围不完整**：Phase 2 废弃 `this` 时，改了实现（编译路径走 `id(descriptor) → field_index`）+ 设计文档（表达式系统设计），但没清理 docstring 示例。变更范围清单遗漏了"所有 docstring / 注释 / 示例代码"。
2. **无 CI 检查**：没有"语法废弃后 grep 残留"的自动化检查。如果 CI 有 `grep "this\." construct-rs/python/` 断言为 0，Phase 2 完成时就能发现。
3. **docstring 不在 reviewer 检查清单**：REV 审查设计文档时聚焦架构/接口/性能，不逐行检查 docstring 示例语法是否与当前语法一致。

**对策**（agent 必须执行）：

- **PM**（变更管理）：任何语法/API 废弃时，变更范围清单必须含"全工作区 grep 旧语法"步骤。具体：
  - 废弃 `this` → `grep -rn "\bthis\b" construct-rs/python/ construct-rs/src/ docs/` 应为 0（不含注释引用 Python 原版）
  - 废弃 `PyCallable` → `grep -rn "PyCallable\|callable" construct-rs/python/` 检查
- **DEV**（代码维护）：新增/修改 docstring 时，示例代码必须用**当前项目语法**（非 Python 原版语法）。如需引用原版作对比，显式标注"Python construct 原版写法"
- **REV/VET**（审查）：审查清单加一条"docstring 示例语法是否与当前项目一致"
- **CI**（自动化）：考虑在 L1/L4 门禁加"废弃语法残留检查"（维护一个 deprecated_patterns.json）

---

## L-14: 设计硬约束认知需交叉验证

> **核心约束**：识别"硬约束"（XX 不可行 / XX 是固有开销）时，必须枚举至少 2 条实现路径交叉验证。不可基于单一实现路径推断硬约束。

**模式描述**：ARCH 在设计分析中识别出某个"硬约束"（如"CPython bytes 不可变 → 跨 FFI 必须拷贝"），但这个约束是基于**单一实现路径**（Python callable 跨 FFI）推断的，没有交叉验证**其他路径**（如 Rust 内置实现零拷贝）。结果用户追问时发现约束不成立——存在更优路径被遗漏。

**反复出现事件**：

| # | 时间 | 事件 | 证据 |
|---|------|------|------|
| 1 | 2026-07-30 | **RawCopy/Checksum "必须拷贝"硬约束被推翻**：ARCH 第一轮回应说"CPython bytes 不可变 → 跨 FFI 必须拷贝"是硬约束，RawCopy 拷贝不可优化。用户追问"为什么要在 Python 层面拷贝"——ARCH 补充评估发现：如果 hashfunc 在 Rust 层面实现（sha2 crate），整个流程零拷贝（FFI 入口 `PyBytes::as_bytes()` 借用 buffer 不拷贝 + Rust 内置哈希操作 `&[u8]`）。ARCH 承认"把单一路径当硬约束"是盲点。 | `docs/design/queries/质疑-能否不用RawCopy.md §7-§10`（ARCH 修正 6 处原结论） |

**根因**：

1. **单一路径推断**：ARCH 分析 Checksum 时聚焦"Python callable hashfunc"这条路径（Python construct 原版模式），推断"跨 FFI 传 bytes 必须拷贝"。没有枚举"Rust 内置 hashfunc"这条替代路径做交叉验证。
2. **"硬约束"标签的惰性**：一旦某个结论被标为"硬约束/固有开销"，后续分析不再质疑它。ARCH 在 §0 共通根因预判中写"parse PyObject 构造是 CPython 固有税"——虽然部分正确，但这个标签阻碍了进一步优化思考。
3. **§0 对照的副作用**：§0 原则对照容易产生"这是硬约束"的直觉——但 §0 约束的是架构层面（无中间表示层 / 无 trait 抽象），不约束实现层面（如 hashfunc 在 Rust 还是 Python）。

**对策**（agent 必须执行）：

- **ARCH**（设计分析）：识别"硬约束"时必须执行**交叉验证清单**：
  - 该约束是基于哪条实现路径推断的？
  - 是否有替代实现路径（如 Rust 内置 vs Python callable / 编译期 vs 运行期 / 安全 vs unsafe）？
  - 替代路径是否被 §0 或其他 ADR 排除？如未排除，不可标"硬约束"
- **PM**（验收转发）：用户质疑"硬约束"时，PM 必须**立即转发 ARCH 重新评估**，不可替 ARCH 辩护（如"ARCH 说了是硬约束"）。PM 无技术能力判断硬约束是否成立。
- **REV**（设计检视）：审查设计文档中的"硬约束/固有开销/不可避免"等措辞时，检查是否有交叉验证证据。如仅有单一路径论证 → 标注"需交叉验证"
- **任何角色**：遇到"XX 不可行"的结论时，自问"是否枚举了所有实现路径？"

---

## 维护规则

- **新增教训**：当某次失败符合"模式化"（非偶发）特征，新增条目。每条教训必须有 ≥1 个真实事件证据（时间 + 证据文件路径）。
- **不删除**：教训一旦写入不删除。若对策失效，标记为"对策失效"并补充新对策。
- **引用而非复制**：本文件只记录模式 + 证据指针。具体技术细节引用源文件（如 `docs/archive/phase1/设计修订-parse路径优化.md §0`）。
- **Agent 自检**：每次决策前，agent 自问"当前情境是否匹配 L-XX？" 若匹配，必须主动应用对策并在过程记录中注明引用的教训 ID。
