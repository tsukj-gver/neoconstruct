# AGENTS.md

> 项目级 System Rules（最高优先级）。所有角色（PM / ARCH / DEV / REV / VET / AUDITOR）启动时必读。
> 遵循 HARNESS.md v1.0 + AHE §演化循环（Evaluate → Analyze → Improve → Verify）。
> 详细角色职责见 `.opencode/agents/<role>.md`；详细规范入口见 `harness/MEMORY.md`。

## 0. 项目定位（违反即推倒重来）

**Python 项目**——交付 Python 包，用户是 Python 开发者。Rust 是内核实现手段，**无独立 Rust 用户**。
性能目标：**≥4x vs Python construct 2.10.70**（10x 为理想）。

**§0 核心原则（不可违反）**：
1. **一次 FFI**：编译 / parse / build 各只有一次 Python↔Rust 边界穿越，Rust 内部通过 CPython C API 直接操作 Python 对象
2. **无中间表示层**：parse 直接从 bytes 构造 PyObject；build 直接从 PyObject 读属性写字节，不在 Rust 侧引入独立的中间数据类型再转换（参考 pydantic-core）
3. **输出/输入侧无抽象 trait**：不在输入/输出侧引入 trait 抽象层（会使每字段跨越 FFI 边界）
4. **pyo3 是核心依赖**：单 crate，不存在独立的 Python 绑定层
5. **mashumaro 式 API**：`@dataclass class X(StructMixin)`，编译在 `__init_subclass__`
6. **构造器分派**：`enum_dispatch` 静态分派（参考 pydantic-core CombinedValidator）
7. **错误处理**：`Result<T, ConstructError>` + `thiserror`，错误携带 `path` 字段
8. **Stream 抽象**：纯 Rust 内部抽象，不跨 FFI
9. **独立分支定位（2026-09-04 起）**：neoconstruct 是独立库，Python `construct` 仅作 **benchmark 性能基线**（≥4x 目标度量用）。行为语义**自主定义**（用户面自然性优先），禁止"与原版对齐"作为设计/测试依据；源码中除 benchmark 相关外不引用 construct
10. **源码零过程信息（2026-09-04 起）**：源码（含测试/bench）不允许出现任务号/批次号/日期/开发环境信息；测试文件按被测语义命名（不带版本号）

> **违反 §0 的实现立即驳回**（实证后果见 `harness/experiences.md §L-01`）。

## 1. 工作流管道

```
PENDING → DESIGNING → DESIGN_REVIEW → CODING → CODE_REVIEW → ACCEPTED → AUDITED
 (PM)      (ARCH)        (REV)         (DEV)      (VET)       (PM)     (AUDITOR)
```

- **状态流转**：每个任务严格按管道推进，不可跳步。`trivial` 任务 PM 标注后可跳 DESIGNING + DESIGN_REVIEW；关联紧密的任务 PM 可合并为一次 ARCH 分派
- **任务标识格式**：`v<version>-<任务号> [任务名]`（如 `v0.1.1-1 [BUG 复现与遗漏调查]`）—— 全角色统一。**2026-09-03 起项目进入半正式发布状态，以版本号+任务号管理**（此前 phase 制的历史标识保持不变）
- **ACCEPTED 时 PM 必须产出 Evaluate 摘录**（AHE §演化循环）；**AUDITED 时 AUDITOR 检查摘录产出**
- **驳回规则**：REV→DESIGNING / VET→CODING / AUDITOR→PM（PM 补充缺失的管理工作），必须附具体原因
- **角色隔离**：DEV 不兼任 REV/VET（避免确认偏误）
- **设计质疑**：任何角色可标记 `[设计质疑]`，PM 转发 ARCH 必须回应
- 详见 `pm.md §工作流管道`（状态细节）+ `neoconstruct-ahe-practices §C`（Evaluate 协议）

## 2. 文件读写权限

| 角色 | 可写 | 只读/禁止 |
|------|------|----------|
| PM | 除业务代码外的全部（`harness/`、`plans/`、`docs/`、`.opencode/`、`testing/`、`experiments/`） | `neoconstruct/src/`、`construct/` |
| ARCH | `docs/`（设计文档）、`experiments/`、`harness/extensions/`（自己角色）、`testing/`（设计调整） | `plans/`、`construct/` |
| DEV | `neoconstruct/**`（Rust src / Python 包 / tests / bench）、`.github/workflows/**`、`plans/**/traces/**`、`plans/meta/**`、`testing/`（CI runner 实施）、`harness/`（自身轨迹记录） | `docs/`、`plans/*/总纲.md` |
| REV / VET / AUDITOR | `plans/**/traces/**`、`plans/meta/**`、`testing/`、`harness/` | 全部 |

**禁止**（全员）：`construct/`（Python 原版只读参考，如存在）/ `neoconstruct/src/` 业务代码（仅 DEV）/ 已验收阶段总纲（除非 PM 授权）/ `.opencode/skills/agentic-harness-engineering/`（通用 AHE skill，L-08 防护，不可项目化修改）

## 3. 全员红线

- **§0 不可违反**：parse 返回 dict 跨 FFI / build 接收 dict 跨 FFI / 引入输入输出 trait 抽象层 → 立即驳回（详见 `harness/experiences.md §L-01`）
- **质量门禁**（每次出口必须通过）：`cargo build` + `cargo clippy`（零 warning）+ `cargo fmt --check` + `cargo test`（全 PASS）。详见 `developer.md §自检清单`
- **Rust 编码红线**：禁止 `unwrap()`/`expect()`/panic 在非测试代码 / 禁止 `TODO`/`FIXME` / 禁止硬编码魔法数字 / 所有 `pub` 项必须有 `///` 文档注释 / parse/build 对称。详见 `developer.md §Rust 编码红线`
- **权限执行红线**：禁止用 bash/终端绕过 edit/write 工具的权限白名单（拦截即停止并上报 PM）；REV/VET/AUDITOR 不编写任何代码（含探针/验证脚本——需要时移交 PM 分派 DEV）；项目产物与环境依赖禁止放工作区外临时目录。详见 `harness/experiences.md §L-15`
- **跨阶段决策**：所有新功能设计不可违反 `docs/decisions/`（ADR-001~ADR-NNN，索引 `docs/decisions/README.md`）
- **阶段依赖**：依赖阶段未完成验收时禁止开始后续阶段子任务
- **Git 禁止**：所有角色禁止 `git push`；REV/VET/AUDITOR 禁止任何 git 写操作（详见 `pm.md §提交规范`）
- **问题处理优先级**：设计缺陷 > 实现遗漏 > 测试遗漏 > 接口不一致 > 逻辑错误 > 代码质量
- **Agent 文件写入**：禁止一次性 Write/Edit 超过 ~300 行，必须分批

## 4. 启动入口

任何角色启动时按序读取：

1. 本文件（System Rules 核心）
2. `harness/MEMORY.md`（L0 索引：项目定位 / 阶段索引 / Harness 组件 / 教训索引 / 性能快照）
3. `harness/experiences.md`（L1 教训：L-01~L-10 模式化失败，全员决策前对照）
4. `.opencode/agents/<role>.md`（**通用 base**：跨工程身份/职责/返回格式框架）
5. `harness/extensions/<role>-extension.md`（**项目特定 extension**：工作流状态/检查清单/文件路径，由 base 强制约定加载）
6. `plans/phaseN/总纲.md`（当前 phase 单一事实源）
7. 涉及 AHE iteration 时：`.opencode/skills/neoconstruct-ahe-practices/SKILL.md`
8. 涉及性能子任务时：`.opencode/skills/performance-gate/SKILL.md`
9. 涉及过程记录填写时：`harness/metadata-convention.md`

## 5. 内容准入标准

PM 是 AGENTS.md 的唯一维护者。修改本文件前必须对照 **`pm.md §AGENTS.md 维护标准`** 的三准则（全员必读 / 稳定性高 / 不可下沉）。违反准则的内容应驳回。

---

> 本文件保持精简。详细操作见各入口文件。
