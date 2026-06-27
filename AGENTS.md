# AGENTS.md

> 本文档包含 construct-rs 项目中所有角色在任何任务、任何状态下都必须知晓的信息。
> 无论你被分配为何种角色（PM / ARCH / DEV / REV / VET），开始工作前必须阅读并遵守本文档。

---

## 0. 项目定位（最高优先级，所有决策的前提）

**这是一个 Python 项目。**

交付物是 Python 包。用户是 Python 开发者。性能目标：**≥4x vs Python construct 2.10.70**（10x 为理想目标）。

Rust 代码是**内核实现手段**，不是独立产品。没有任何 Rust 用户。不存在"纯 Rust 使用场景"。

### 核心原则（所有架构决策必须遵守）

1. **一次 FFI**：编译、parse、build 各只有一次 Python↔Rust 边界穿越。Rust 内部通过 CPython C API 直接操作 Python 对象。

2. **无中间表示层**：parse 直接从 bytes 构造 PyObject，build 直接从 PyObject 读属性写字节，不在 Rust 侧引入独立的中间数据类型再转换。参考 pydantic-core。

3. **pyo3 是核心依赖**：单 crate，不存在独立的 Python 绑定层。

4. **mashumaro 式 API**：`@dataclass class X(StructMixin)`，不照搬 pydantic BaseModel。编译在 `__init_subclass__` 中完成，用户无感。

> **历史教训**：此前的实现将解析结果先构造为 Rust 中间类型再逐个转换为 Python 对象，
> 并在输入/输出侧引入 trait 抽象层使每个字段都跨越 FFI 边界，
> 这些间接层的累积开销使 Python 端性能仅为原版的 0.3-1.8x。
> 本项目于 2026-06-21 推倒重来，坚持一次 FFI、直接操作 Python 对象的设计。

---

## 1. 项目简介

将 Python 库 [construct](https://github.com/construct/construct) (v2.10.70) 的核心内核用 Rust 重写，**使其更快**。Construct 是一个声明式、对称的二进制数据解析与构建库——同一套定义既能解析（parse）二进制数据，也能构建（build）二进制数据。

**Python 原版源码位置**：`construct/construct/`（在本项目目录下）
- `core.py`（6447行）：所有构造器实现，是核心中的核心
- `expr.py`：表达式系统（this/obj_ 引用）
- `lib/containers.py`：Container / ListContainer 数据结构
- `lib/binary.py`：整数/位/字节转换
- `lib/bitstream.py`：流包装器
- `gallery/`：格式解析器示例（ELF, PE/COFF 等）

## 2. 目录结构

```
construct_rust/
├── construct/              ← Python 原版仓库（只读参考，禁止修改）
├── refs/                   ← 参考项目（pydantic-core, mashumaro）
├── examples/               ← 用户 API 定义（example.py, construct.py）
├── plans/                  ← 阶段计划
│   ├── 00-项目进度.md      ← 总进度仪表盘（PM 恢复工作入口）
│   ├── phase0-architecture/← Phase 0 架构设计
│   └── phase1-foundation/  ← Phase 1 垂直切片
└── docs/                   ← 设计文档
    ├── 架构设计.md          ← Phase 0 产出的核心设计（待创建）
    └── analysis/           ← 分析报告（pydantic-core 对比、mashumaro 分析）
```

## 3. 工作流概要

每个子任务严格按以下管道流转，不可跳步：

```
PENDING → DESIGNING → DESIGN_REVIEW → CODING → CODE_REVIEW → ACCEPTED
 (PM)      (ARCH)        (REV)         (DEV)      (VET)       (PM)
```

- **PENDING**：PM 从总纲选取任务
- **DESIGNING**：ARCH 编写/确认模块设计文档
- **DESIGN_REVIEW**：REV 检视设计的延续性、性能、整体性、可行性、完备性
- **CODING**：DEV 编码 + 单元测试 + 自检
- **CODE_REVIEW**：VET 审查代码逻辑、行为一致性、错误处理、安全、边界条件
- **ACCEPTED**：PM 确认完成

**驳回规则**：REV 可驳回至 DESIGNING；VET 可驳回至 CODING。驳回必须附具体原因。

**角色隔离**：同一子任务中，DEV 不得兼任 REV 或 VET。REV（设计检视）和 VET（代码审查）必须是不同 agent，避免确认偏误。

**简化流程**：对于 `trivial` 性质的子任务（如项目初始化、纯文档更新），PM 可标注为 trivial，跳过 DESIGNING 和 DESIGN_REVIEW 阶段，仅走 CODING → CODE_REVIEW。

**合并设计**：关联紧密的多个子任务，PM 可合并为一次 ARCH 分派，产出一份合并的模块设计文档。

**设计质疑（Argue 机制）**：任何角色在执行任务过程中发现设计文档存在不合理、冲突或遗漏时，可向 PM 提出质疑。PM 将质疑转发给 ARCH，ARCH 必须回应。详见 `docs/workflow/工作流文档.md`。

## 4. 文件读写规则

| 角色 | 可写 | 只读 |
|------|------|------|
| PM | 除业务代码外的全部（`plans/`、`docs/`、`.opencode/`、`experiments/`） | `construct-rs/src/`、`construct/` |
| ARCH | `docs/`（设计文档）、`experiments/`（实验） | `plans/`、`construct/` |
| DEV | `construct-rs/src/**`、`plans/phaseN/过程记录.md`（开发日志）、`experiments/`（实验） | `docs/`、`plans/phaseN/总纲.md` |
| REV | `plans/phaseN/过程记录.md`（设计检视结果）、`experiments/`（实验） | 全部 |
| VET | `plans/phaseN/过程记录.md`（代码审查结果）、`experiments/`（实验） | 全部 |

`experiments/` 是试验场，任何角色都可以在此写代码做实验（测元操作耗时、验证行为、诊断问题），不受编码规范约束，不影响交付物。

**禁止修改**：
- `construct-rs/src/` 下的业务代码（仅 DEV 可写）
- `construct/` 目录下的任何文件（Python 原版，只读参考）
- 已验收阶段的总纲文件（除非 PM 授权）
- 非自己角色负责的文件段落

## 5. 过程记录格式

每个子任务的过程记录段必须包含以下结构：

```markdown
## 子任务 X.Y: [任务名]

| 项目 | 内容 |
|------|------|
| 状态 | 未开始 / 设计中 / 待检视 / 开发中 / 待审查 / 已通过 / 已驳回 |
| 负责人 | [角色缩写] |
| 开始时间 | YYYY-MM-DD |
| 完成时间 | YYYY-MM-DD |

### 操作日志
- [时间] [角色] 操作描述

### [角色专属段落]
（REV：设计检视结果 / VET：代码审查结果 / ARCH：设计说明 / DEV：自检结果）

### 备注
（补充说明、驳回原因记录等）
```

## 6. 常用命令

> **注意**：所有 cargo 命令必须在 `construct-rs/` 目录下执行（使用 bash 工具的 `workdir="construct-rs"` 参数）。

```bash
# 编译检查
cargo build
cargo clippy                        # 零 warning 要求

# 测试
cargo test                          # 全部 PASS 要求
cargo test --features compression   # 含可选依赖测试
cargo test -- --nocapture           # 显示 println! 输出

# 代码格式
cargo fmt --check                   # 格式检查
cargo fmt                           # 自动格式化

# 文档
cargo doc --no-deps --open          # 生成并查看文档

# 基准测试（Phase 9+）
cargo bench
```

**所有阶段出口必须通过**：`cargo build` + `cargo clippy` + `cargo fmt --check` + `cargo test`

### ⚠️ 性能门禁（涉及性能的阶段必须遵守）

详见 `.opencode/skills/performance-gate.md`。三条硬规则：

1. **设计阶段**：ARCH 必须在设计中写入"性能假设"（瓶颈识别 + 可证伪预测 + 验证方法）。REV 必须检查预测覆盖了**所有**瓶颈来源。
2. **首个实现后**：PM 必须执行性能烟雾测试（vs 绝对基线，非旧路径）。< 0.5x → 暂停后续阶段。
3. **验收阶段**：PM 必须检查证据类型匹配。"≥1.0x"要求对比数据表，不接受"编译通过"。

**S-PERF 基线必须用绝对基线**（Python 原版 construct），不可用相对基线（旧路径）。

### Git 命令

```bash
# PM 专用（所有角色可查看）
git status                          # 查看工作区状态
git diff                            # 查看未暂存的变更
git log --oneline -10               # 查看最近提交

# PM 专用（仅 PM 可执行）
git add <files>                     # 暂存文件
git commit -m "feat(phaseN): X.Y 描述"  # 子任务提交
git tag phase-N-complete            # 阶段验收标签
```

## 7. 核心技术决策（全员须知）

> ⚠️ 以下决策由 Phase 0 架构设计最终确定。当前为暂定方向，ARCH 在设计过程中可细化。

| 决策 | 内容 | 理由 |
|------|------|------|
| **pyo3 定位** | pyo3 是核心依赖，非 optional feature。parse 直接产 `Py<PyAny>`，build 直接接收 `&Bound<PyAny>` | 消除中间表示层。详见 §0 |
| **直接操作 Python 对象** | parse 从 bytes 直接构造 PyObject；build 从 PyObject 直接读属性写字节，Rust 侧不引入独立的中间数据类型 | 中间类型转换层是不可接受的开销（见 §0 历史教训） |
| **输出/输入侧无抽象 trait** | 参考pydantic-core：不在输入/输出侧引入 trait 抽象层，直接操作 CPython C API | trait 抽象层会使每字段都跨越 FFI 边界，是不可接受的开销（见 §0 历史教训） |
| **单 crate** | 不存在独立的 Python 绑定层，pyo3 在主 crate 中 | 消除 crate 间的边界开销 |
| **mashumaro 式 API** | `@dataclass class X(StructMixin)`，编译在 `__init_subclass__` 中 | 用户无感，不照搬 pydantic BaseModel |
| **一次 FFI** | 编译、parse、build 各只有一次 Python↔Rust 边界穿越 | 每字段跨越 FFI 是不可接受的开销（见 §0 历史教训） |
| **构造器分派** | `enum_dispatch` 静态分派（参考 pydantic-core CombinedValidator） | 避免 `Box<dyn>` 动态分派开销 |
| **错误处理** | `Result<T, ConstructError>` + `thiserror` | 统一错误体系，`?` 传播 |
| **Stream 抽象** | 纯 Rust 内部抽象，不跨 FFI | parse/build 边界内完整处理 |

## 8. Rust 编码红线（全员必须遵守）

1. **禁止 `unwrap()` / `expect()` 在非测试代码中出现** — 所有可能失败的操作必须返回 `Result`
2. **禁止 panic** — 非法输入必须返回 `Err` 而非 panic
3. **禁止 `TODO` / `FIXME`** — 当轮任务当轮解决，或明确创建新子任务
4. **禁止硬编码魔法数字** — 使用常量或枚举
5. **所有 `pub` 项必须有 `///` 文档注释**
6. **错误必须携带 `path` 字段** — 用于追踪出错位置
7. **parse/build 对称性** — 所有构造器必须同时支持 parse 和 build

## 9. 阶段依赖关系

```
Phase 0 (架构设计)
  ↓
Phase 1 (垂直切片) ←── 验证架构可行性 + 性能 >4x
  ↓
Phase 2+ ←── 由用户在 Phase 1 验收通过后指定
```

> 阶段规划在 Phase 0 架构设计完成后由 PM 根据设计文档制定。

**禁止**：在依赖阶段未完成验收时开始后续阶段的子任务。

## 10. Python 参考速查

开发任何模块时，首先定位 Python 源码中的对应实现：

| Rust 模块 | Python 源码位置 |
|-----------|---------------|
| Construct trait | `construct/construct/core.py` → `Construct` 类 (line ~321) |
| 动态类型 | 对应 Python 的动态类型 + `Container` / `ListContainer`（直接以 PyObject 表达，无 Rust 中间枚举） |
| Context | `construct/construct/core.py` → parse/build 中的 `context` 参数 |
| Stream | `construct/construct/core.py` → `stream_read` 等辅助函数 |
| 错误 | `construct/construct/core.py` → 文件末尾 ~40 个 Exception 子类 |
| 原子构造器 | `construct/construct/core.py` → `Bytes`, `FormatField`, `VarInt` 等 |
| 复合构造器 | `construct/construct/core.py` → `Struct`, `Sequence`, `Array` 等 |
| 适配器 | `construct/construct/core.py` → `Adapter`, `Enum`, `Validator` 等 |
| 表达式 | `construct/construct/expr.py` → `Path`, `BinExpr` 等 |
| 流操作 | `construct/construct/core.py` → `Bitwise`, `Pointer`, `Prefixed` 等 |
| 惰性解析 | `construct/construct/core.py` → `Lazy`, `LazyStruct` 等 |
| 容器类型 | `construct/construct/lib/containers.py` |
| 二进制工具 | `construct/construct/lib/binary.py` |
| 流包装器 | `construct/construct/lib/bitstream.py` |
| 格式示例 | `construct/gallery/` 和 `construct/deprecated_gallery/` |

## 11. 问题处理优先级

当发现问题时，按以下优先级处理：

1. **设计缺陷** → 驳回至 DESIGNING，ARCH 修正设计文档
2. **实现遗漏** → 驳回至 CODING，DEV 补充实现
3. **测试遗漏** → 驳回至 CODING，DEV 补充测试
4. **接口不一致** → 驳回至 CODING，DEV 修正；涉及设计变更则先回退至 DESIGNING
5. **逻辑错误** → 驳回至 CODING，DEV 修正
6. **代码质量** → 驳回至 CODING，DEV 修正

所有驳回都必须在过程记录中记录原因，修复后需重新走完整的验证+审查流程。

## 12. opencode 角色分派机制

本项目通过 opencode 的 agent 系统实现角色分派。每个角色对应一个 agent 文件：

```
.opencode/agents/
├── pm.md          ← PM（主 agent，mode: primary）
├── architect.md   ← ARCH（子 agent，mode: subagent）
├── developer.md   ← DEV（子 agent，mode: subagent）
├── reviewer.md    ← REV（子 agent，mode: subagent）— 设计检视
├── vetter.md      ← VET（子 agent，mode: subagent）— 代码审查
```

> **已移除** `validator.md`（REF 角色已并入 VET）。

### PM 分派方式

PM 通过 opencode 的 Task 工具分派任务给子 agent：

```
Task 工具参数：
  subagent_type: "architect" | "developer" | "reviewer" | "vetter"
  description: "3-5词任务描述"
  prompt: "包含子任务信息、必读文件、输出要求的完整指令"
```

### 各角色 agent 的文件权限

| 角色 agent | 可写范围 | 禁止写入 |
|-----------|---------|---------|
| pm | `plans/`（含 `00-项目进度.md`） | `construct-rs/`, `docs/`, `construct/` |
| architect | `docs/` | `construct-rs/`, `plans/`, `construct/` |
| developer | `construct-rs/`, `plans/*/过程记录.md` | `docs/`, `construct/` |
| reviewer | `plans/*/过程记录.md` | `construct-rs/`, `docs/`, `construct/` |
| vetter | `plans/*/过程记录.md` | `construct-rs/`, `docs/`, `construct/` |

### 切换 agent

- PM 是默认主 agent，用户直接与 PM 对话
- 如需直接使用其他角色，可在 opencode 中切换 agent
- 所有角色 agent 的完整指令见 `.opencode/agents/*.md`

### Agent 文件写入规范（全员必须遵守）

子 agent 在写入大文件时**必须分批操作**，严禁一次性写入超长内容：

1. **禁止一次性创建/覆盖超过 ~300 行的文件**。对于大型设计文档或源文件，必须：
   - 先用 `Write` 工具创建文件并写入第一部分（文档头部 + 前几节）
   - 再用 `Edit` 工具追加后续章节，每次追加不超过 ~300 行
   - 每次写入后确认成功再继续下一批

2. **禁止在单个 Task prompt 中要求 agent 一次性产出超大输出**。PM 分派任务时应：
   - 明确告知 agent 分批写入策略
   - 对于大型文档，可在 prompt 中指定分批计划（如"先写第 1-4 节，再追加第 5-8 节"）

3. **原因**：单次写入超长内容会导致 agent 响应超时或被截断，浪费大量时间。

## 13. Git 工作流

### 分支策略

直接在 `main` 分支上开发，不使用特性分支。

### 提交规范

| 操作 | 执行者 | 规则 |
|------|--------|------|
| `git commit` | PM + DEV | PM：每个子任务 ACCEPTED 后正式提交；DEV：开发过程中可做 checkpoint 提交防止丢失 |
| `git tag` | 仅 PM | 阶段验收通过后打 tag |
| `git status` / `git diff` | 所有角色 | 只读，用于查看变更 |

**commit message 格式**：
```
feat(phaseN): X.Y 子任务描述
```
示例：
- `feat(phase1): 1.1 项目初始化`
- `feat(phase2): 2.3 FormatField 实现`
- `docs(phase1): 更新总设计文档`

**tag 格式**：
```
phase-N-complete
```
示例：`phase-1-complete`、`phase-2-complete`

### 提交时机

```
子任务 ACCEPTED → PM 执行 git add + git commit
    ↓
... 所有子任务完成 ...
    ↓
阶段验收通过 → PM 执行 git tag phase-N-complete
```

DEV 在开发过程中可随时执行 `git add` + `git commit` 作为 checkpoint，防止工作丢失。

### 禁止事项

- REV / VET **禁止**执行 `git add`、`git commit`、`git tag`、`git push`
- 所有角色**禁止**执行 `git push`（本地仓库，无需远程推送）
- **禁止**提交 `construct/` 目录下的任何变更（Python 原版仓库有独立 git）
