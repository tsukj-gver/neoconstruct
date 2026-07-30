---
id: DESIGN-Conditional
status: active
phase: "7"
task: "7.1 [Conditional 详细设计]"
depends_on: [AGENTS.md, ADR-014, ADR-022, ANALYSIS-phase7-pre, DESIGN-Array, DESIGN-Adapter核心, DESIGN-表达式系统, experiences.md]
last_updated: 2026-07-30
---

# 模块设计 - Conditional（IfThenElse / Switch / Select / FocusedSeq + ExplicitError）

> 本设计覆盖 Phase 7.1 子任务的全部范围：5 个 Conditional 构造器（If / IfThenElse /
> Switch / Select / FocusedSeq）+ ExplicitError 基础设施引入。
>
> **信息源**：
> - Python 原版：`construct/construct/core.py`（Select L3830 / If L3912 / IfThenElse L3944 /
>   Switch L4002 / FocusedSeq L3176；ExplicitError L89 / SelectError L109）
> - 分析报告：`docs/analysis/分析报告-Phase7启动前置.md §B.1 / §C.1 / §C.2`
> - PM 决策 1（Switch keyfunc = A+B 混合，拒绝 PyCallable）
> - PM 决策 2（引入 `ConstructError::Explicit` 变体）
> - 先例：`construct-rs/src/nodes/stop_if.rs`（Condition 模式）、`peek.rs`（ExplicitError 占位）、
>   `pass.rs`（If/Switch default 依赖）、`expr.rs`（ExprProgram）
>
> **§0 原则对照**贯穿全文（§7）。已对照 L-01（中间表示层）/ L-04（跨阶段模式未沉淀）/
> L-05（优化 A 路径忽略 B 路径）。

## 模块位置

| 组件 | 文件路径 | 说明 |
|------|---------|------|
| `IfThenElseNode` | `construct-rs/src/nodes/if_then_else.rs` | 新建 |
| `SwitchNode` + `SwitchKey` + `SwitchCase` | `construct-rs/src/nodes/switch.rs` | 新建 |
| `SelectNode` | `construct-rs/src/nodes/select.rs` | 新建 |
| `FocusedSeqNode` | `construct-rs/src/nodes/focused_seq.rs` | 新建 |
| `ConstructError::Explicit` / `::Select` | `construct-rs/src/error.rs` | 修改（新增 2 变体） |
| `Node` enum | `construct-rs/src/nodes/mod.rs` | 修改（+4 变体） |
| `peek.rs::is_explicit_error` | `construct-rs/src/nodes/peek.rs` | 修改（占位 → 真实分类） |
| 编译入口 | `construct-rs/src/compile.rs` | 修改（+4 Descriptor 分支） |
| Python 用户面 | `construct/_construct_rust.pyi` + `construct/core.py` | `If` macro（Python 层） |

**`If` 构造器**：Python macro（`If(cond, sub) = IfThenElse(cond, sub, Pass)`），**Rust 侧不新增 Node**——与 Python 原版 L3935 完全一致。

## 职责

为执行树提供运行时条件分支能力：
- **IfThenElse**：双分支（then/else）条件选择，复用 StopIf 的 Condition 模式。
- **Switch**：多分支（cases 映射）条件选择，keyfunc 走 ExprProgram(i64) + FieldRef(PyObject) 混合路径（PM 决策 1）。
- **Select**：尝试多个 subcon，首个成功的胜出；ExplicitError 穿透（PM 决策 2 引入变体后达成 Python parity）。
- **FocusedSeq**：context nesting + 返回单聚焦字段值（精细化 Adapter 基础设施）。
- **ExplicitError**：错误分类基础——Select / Peek 不吞此类错误，直接向上传播。

## 依赖图

```
                       ┌──────────────────────┐
                       │ ConstructError (err) │
                       │  + Explicit 变体     │
                       │  + Select 变体       │
                       └──────────┬───────────┘
                                  │ 被依赖
              ┌───────────────────┼───────────────────┐
              ▼                   ▼                   ▼
        ┌──────────┐        ┌──────────┐        ┌──────────┐
        │ IfThenElse│       │  Select  │       │  (Peek)  │
        │  Node    │        │  Node    │       │ 升级占位 │
        └────┬─────┘        └──────────┘        └──────────┘
             │ 依赖
             ▼
   ┌─────────────────────┐
   │ StopIfCondition 模式 │ ← Phase 4 先例
   │ ExprProgram (Phase 2)│
   │ PassNode    (Phase 6)│ ← else/default 分支
   └─────────────────────┘

   ┌──────────┐         ┌──────────────┐
   │  Switch  │ ←────── │ ExprProgram  │ (路线 A: i64 key)
   │  Node    │ ←────── │ FieldRef/ctx │ (路线 B: PyObject key)
   └──────────┘         └──────────────┘

   ┌────────────┐       ┌──────────────────┐
   │ FocusedSeq │ ←──── │ Context::new_child│ (context nesting)
   │  Node      │ ←──── │ StructField 复用  │ (字段结构，不复用 parse/build)
   └────────────┘       └──────────────────┘
```

## 与 Python 版本对应（总表）

| Python 类/方法 (core.py 行号) | Rust 类型/方法 | 备注 |
|------------------------------|---------------|------|
| `ExplicitError` (L89) | `ConstructError::Explicit { message, path }` | 新增变体 |
| `SelectError` (L109) | `ConstructError::Select { message, path }` | 新增变体 |
| `If` (L3912，macro) | Python macro `If(cond, sub) = IfThenElse(cond, sub, Pass)` | **Rust 不新增 Node** |
| `IfThenElse.__init__` (L3967) | `IfThenElseNode::new(cond, then_sub, else_sub)` | |
| `IfThenElse._parse` (L3974) | `IfThenElseNode::parse` | evaluate(cond) → 选 then/else → 委托 |
| `IfThenElse._build` (L3979) | `IfThenElseNode::build` | 对称 |
| `IfThenElse._sizeof` (L3984) | `IfThenElseNode::sizeof` | 对称 |
| `Switch.__init__` (L4031) | `SwitchNode::new(key, cases, default)` | cases 编译期展开为 Vec |
| `Switch._parse` (L4041) | `SwitchNode::parse` | keyfunc 求值 → i64 快速匹配 / PyObject `__eq__` → default |
| `Switch._build` (L4046) | `SwitchNode::build` | 对称 |
| `Switch._sizeof` (L4051) | `SwitchNode::sizeof` | try evaluate → 选 subcon sizeof；KeyError/AttributeError → SizeofError |
| `Select.__init__` (L3855) | `SelectNode::new(subcons)` | |
| `Select._parse` (L3860) | `SelectNode::parse` | fallback=tell → try → Explicit 穿透 / 其他吞+seek → SelectError |
| `Select._build` (L3873) | `SelectNode::build` | try 在 temp BuildStream → 成功 write / Explicit 穿透 / 其他吞 → SelectError |
| `FocusedSeq.__init__` (L3225) | `FocusedSeqNode::new(parsebuildfrom, fields, focus_index)` | parsebuildfrom 编译期解析为 focus_index |
| `FocusedSeq._parse` (L3236) | `FocusedSeqNode::parse` | context nesting + 返回 focus 字段值 |
| `FocusedSeq._build` (L3248) | `FocusedSeqNode::build` | context nesting + focus 字段传 obj，其余传 None |
| `FocusedSeq._sizeof` (L3261) | `FocusedSeqNode::sizeof` | context nesting + sum 字段 sizeof |

---

## 1. ExplicitError 基础设施引入（基础依赖，优先实施）

### 1.1 PM 决策 2 背景

ADR-022 PE-3 把 ExplicitError 推迟到 Phase 7。Python `Select._parse`/`_build` 与
`Peek` 明确 `except ExplicitError: raise`（不吞）。当前 `ConstructError` 无 Explicit 变体，
`peek.rs::is_explicit_error` 占位返回 `false`（所有错误都被吞）。PM 决策 2 接受选项 A：
引入 `ConstructError::Explicit` 变体。

### 1.2 新增错误变体

在 `construct-rs/src/error.rs` 的 `ConstructError` enum 新增两个变体：

```rust
pub enum ConstructError {
    // ... 既有变体 ...

    /// 显式错误：用户主动抛出（对应 Python construct `ExplicitError`，core.py L89）。
    ///
    /// **不被 Select / Peek 吞掉**——直接向上传播。Python 中由 `Error` 构造器
    /// （Phase 7 暂不实现）或用户在 `_emitparse`/Adapter 回调中主动抛出。
    ///
    /// Phase 7 引入（PM 决策 2 / ADR-022 PE-3 收尾）。
    /// 触发场景：
    /// - Select 遍历 subcons 时，某 subcon 抛 Explicit → Select 直接传播（不尝试后续）
    /// - Peek 预读时，inner 抛 Explicit → Peek 直接传播（不返回 Py_None）
    #[error("explicit error: {message} at {path}")]
    Explicit {
        /// 错误详情。
        message: String,
        /// 错误发生的路径。
        path: String,
    },

    /// Select 错误：所有 subcon 都未成功（对应 Python construct `SelectError`，core.py L109）。
    ///
    /// 触发场景：`SelectNode::parse` / `build` 遍历全部 subcons 后无成功者。
    #[error("select error: {message} at {path}")]
    Select {
        /// 错误详情（如 "no subconstruct matched"）。
        message: String,
        /// 错误发生的路径。
        path: String,
    },
}
```

### 1.3 配套修改（error.rs）

**`ExceptionClasses` 缓存新增 2 字段**（与 `string_error` 同模式）：

```rust
struct ExceptionClasses {
    // ... 既有字段 ...
    /// 对应 `ConstructError::Explicit`（Phase 7）。Python construct 的 `ExplicitError`。
    explicit_error: Py<PyType>,
    /// 对应 `ConstructError::Select`（Phase 7）。Python construct 的 `SelectError`。
    select_error: Py<PyType>,
}
```

**`init_exception_classes` 新增 2 行**（在 `string_error` 之后）：

```rust
let classes = ExceptionClasses {
    // ... 既有 ...
    string_error: get("StringError")?,
    explicit_error: get("ExplicitError")?,
    select_error: get("SelectError")?,
};
```

**`is_builtin_class` 数组扩容 14 → 16**（O3 fast-path 前置条件）：

```rust
let builtin_ptrs: [*mut ffi::PyObject; 16] = [
    // ... 既有 14 项 ...
    self.string_error.as_ptr(),
    self.explicit_error.as_ptr(),
    self.select_error.as_ptr(),
];
```

**`message()` / `path()` / `set_path()` / `kind_str()` / `select_exception_class` 各加 2 分支**：

```rust
// message()
| ConstructError::Explicit { message, .. }
| ConstructError::Select { message, .. } => Some(message),

// path()
| ConstructError::Explicit { path, .. }
| ConstructError::Select { path, .. } => Some(path),

// set_path()
| ConstructError::Explicit { path, .. }
| ConstructError::Select { path, .. } => *path = new_path,

// kind_str()
ConstructError::Explicit { .. } => "Explicit",
ConstructError::Select { .. } => "Select",

// select_exception_class
ConstructError::Explicit { .. } => &classes.explicit_error,
ConstructError::Select { .. } => &classes.select_error,
```

### 1.4 Peek 升级（peek.rs）

`is_explicit_error` 占位改为真实分类（1 行）：

```rust
fn is_explicit_error(e: &ConstructError) -> bool {
    matches!(e, ConstructError::Explicit { .. })
}
```

Peek 的 parse 逻辑不变——`is_explicit_error` 返回 `true` 时，原有 `if is_explicit_error(&e) { return Err(e); }`
分支即生效，Explicit 错误向上传播，不被吞。

### 1.5 边界条件

- `ConstructError::Explicit` 仅由用户路径产生（AdapterCallback 回调抛 Python `ExplicitError` →
  pyo3 转 `PyErr` → `From<PyErr> for ConstructError` 转 `Generic`）。
  - **差异说明**：当前 `From<PyErr> for ConstructError`（error.rs L942）统一转为 `Generic`，
    无法保留 Python 侧的 `ExplicitError` 类型信息。Phase 7 范围内不修复（影响小：
    Select/Peek 用 `Explicit` 变体的场景目前仅限 Rust 内部主动构造，Python 侧抛
    `ExplicitError` 经 `From<PyErr>` 后被 Select/Peek 当 Generic 吞掉）。
  - **已知差异记录**：DEV 实施清单 §11-D1。
- `Select` / `Peek` 自身不主动构造 `Explicit`；它们仅识别并传播。
- `Select` 变体仅由 `SelectNode::parse` / `build` 在全部 subcons 失败后构造。

---

## 2. IfThenElseNode（含 If macro 说明）

### 2.1 Condition 模式复用决策

**ARCH 决策**：复用 Phase 4 的 `StopIfCondition` 类型（不新建 `IfThenElseCondition`）。
理由：

1. **同构类型**：`StopIfCondition`（stop_if.rs L48）的三个变体（`Always` / `Never` / `Expr(ExprProgram)`）
   与 IfThenElse 的条件分类**完全同构**——都是"常量 true / 常量 false / 表达式"。
2. **L-04 对策**（跨阶段模式未沉淀）：StopIf 已验证 Condition 模式（4 个 Node 用、性能达标），
   重新建同构类型属于重复决策。
3. **语义独立**：`StopIfCondition` 仅是类型名，其变体 `Always/Never/Expr` 不暗示"停止"——
   IfThenElse 用它表达"条件为真"的语义，与 StopIf 用它表达"条件为真→停止"是同一抽象。

**未来 ADR 触发点**：若 Phase 8+ 出现第 3 个使用 Condition 模式的构造器，应沉淀 ADR
将 `StopIfCondition` 重命名为 `Condition` 并提到 `nodes/common.rs`（或 `condition.rs`）。
本 Phase 不做此重命名（避免无关改动）。

**实施路径**：`if_then_else.rs` 通过 `use crate::nodes::stop_if::StopIfCondition;` 复用。

### 2.2 struct 定义

```rust
/// 双分支条件节点（对应 Python construct `IfThenElse`，core.py L3944）。
///
/// # 三方法行为
///
/// 对齐 Python `IfThenElse._parse` / `_build` / `_sizeof`（L3974-3987）：
/// - parse：求值 cond → 选 then/else → 委托 subcon.parse。
/// - build：求值 cond → 选 then/else → 委托 subcon.build。
/// - sizeof：求值 cond → 选 then/else → 委托 subcon.sizeof。
///
/// 求值失败（字段缺失等）错误向上传播（与 Computed/StopIf 同路径）。
///
/// # Condition 模式（复用 StopIfCondition）
///
/// - `Always` → 走 then 分支
/// - `Never` → 走 else 分支
/// - `Expr(prog)` → 求值非零走 then，为零走 else（对齐 Python truthy 语义）
///
/// # If macro
///
/// Python `If(cond, sub) = IfThenElse(cond, sub, Pass)`（L3935）。
/// Rust 不新增 IfNode——Python 用户面 macro 完成等价转换。
#[derive(Debug, Clone)]
pub struct IfThenElseNode {
    /// 条件：常量或表达式（复用 StopIfCondition）。
    cond: StopIfCondition,
    /// 条件为真时委托的子树。
    then_sub: Box<Node>,
    /// 条件为假时委托的子树（通常为 PassNode）。
    else_sub: Box<Node>,
}

impl IfThenElseNode {
    pub fn new(cond: StopIfCondition, then_sub: Node, else_sub: Node) -> Self {
        Self {
            cond,
            then_sub: Box::new(then_sub),
            else_sub: Box::new(else_sub),
        }
    }

    pub fn cond(&self) -> &StopIfCondition { &self.cond }
    pub fn then_sub(&self) -> &Node { &self.then_sub }
    pub fn else_sub(&self) -> &Node { &self.else_sub }

    /// has_expressions：仅 Expr 条件返回 true（触发 StructNode init_expr_values）。
    /// 与 StopIfNode::has_expressions 同模式。
    pub fn has_expressions(&self) -> bool {
        self.cond.is_expr()
    }
}
```

### 2.3 Construct impl

```rust
impl Construct for IfThenElseNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 求值 cond（复用 StopIfNode::eval_cond 的逻辑——但 eval_cond 是私有方法，
        // 这里内联或提取为公共辅助。实施建议：在 stop_if.rs 把 eval_cond 提为 pub(crate)）。
        let take_then = eval_condition(&self.cond, ctx, py)?;
        let sub = if take_then { &self.then_sub } else { &self.else_sub };
        sub.parse(py, stream, ctx, path)
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        let take_then = eval_condition(&self.cond, ctx, py)?;
        let sub = if take_then { &self.then_sub } else { &self.else_sub };
        sub.build(py, obj, stream, ctx, path)
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // 对齐 Python L3984-3987：求值 cond 选 subcon sizeof。
        // sizeof 接口无 py token——但 Expr 求值需要 py（GetInt 调 PyDict）。
        // 处理方式（与 ComputedNode::sizeof 同模式）：sizeof 仅对常量条件返回；
        // Expr 条件下抛 SizeofError（Generic）——因为无法在 sizeof 接口内求值表达式。
        match &self.cond {
            StopIfCondition::Always => self.then_sub.sizeof(ctx),
            StopIfCondition::Never => self.else_sub.sizeof(ctx),
            StopIfCondition::Expr(_) => Err(ConstructError::Generic {
                message: "IfThenElse size is undefined when condition is an expression"
                    .to_string(),
                path: String::new(),
            }),
        }
    }
}
```

**`eval_condition` 公共辅助**：将 `StopIfNode::eval_cond` 的逻辑提取为 `pub(crate)` 自由函数
（位置：`stop_if.rs` 末尾或新建 `nodes/common.rs`）：

```rust
/// 求值条件（StopIf / IfThenElse 共用）。
///
/// - `Always` → true
/// - `Never` → false
/// - `Expr(prog)` → fast-path try_eval_simple_cmp / 通用 eval_expr_int，非零为真
pub(crate) fn eval_condition(
    cond: &StopIfCondition,
    ctx: &Context<'_>,
    py: Python<'_>,
) -> Result<bool, ConstructError> {
    match cond {
        StopIfCondition::Always => Ok(true),
        StopIfCondition::Never => Ok(false),
        StopIfCondition::Expr(prog) => {
            let v = if let Some(fast) = prog.try_eval_simple_cmp(ctx, py) {
                fast?
            } else {
                eval_expr_int(prog, ctx, py)?
            };
            Ok(v != 0)
        }
    }
}
```

`StopIfNode::eval_cond` 改为转发到 `eval_condition`（行为不变，仅重构）。

### 2.4 编译入口（compile.rs）

新增 `"IfThenElseDescriptor"` 分支（参照 StopIfDescriptor L632 + BitwiseDescriptor 递归编译 inner 的模式）：

```rust
"IfThenElseDescriptor" => {
    return Ok(Node::IfThenElse(build_if_then_else_node(
        py, desc, field_index, expr_programs,
    )?));
}
```

`build_if_then_else_node`（参照 `build_stop_if_node` L1642）：
1. 读 `desc.condfunc` → 分类（bool → Always/Never；其他 → Expr，从 `expr_programs[field_index]["cond"]` 取）
2. 读 `desc.thensubcon` → 递归 `build_node_from_descriptor`
3. 读 `desc.elsesubcon` → 递归 `build_node_from_descriptor`
4. 返回 `IfThenElseNode::new`

**Python 描述符**：用户面 `IfThenElse(condfunc, then, else)` 在 `__init_subclass__` 编译期
生成 `IfThenElseDescriptor(condfunc, thensubcon, elsesubcon)`，与 StopIfDescriptor 同模式
（Python 侧实现由 DEV 完成，Rust 仅识别 type name）。

### 2.5 If macro（Python 用户面）

`If` 不新增 Rust Node。Python 侧 macro（与原版 L3935 一致）：

```python
def If(condfunc, subcon):
    return IfThenElse(condfunc, subcon, Pass)
```

Python 侧实现位置：`construct/core.py` 或 `construct/_construct_rust.pyi` shim。
DEV 实施时确认 Python 用户面导出 `If` 名字（与原版 API 一致）。

### 2.6 边界条件

| # | 场景 | 预期行为 |
|---|------|---------|
| IF-1 | `cond = True`（常量）| 编译期分类为 `Always`，运行时零求值，直接走 then |
| IF-2 | `cond = False`（常量）| 编译期分类为 `Never`，运行时零求值，直接走 else |
| IF-3 | `cond = this.x > 0`，x=5 | Expr 求值=1 → 走 then |
| IF-4 | `cond = this.x > 0`，x=0 | Expr 求值=0 → 走 else |
| IF-5 | `cond = this.x > 0`，x 字段缺失 | `ExprFieldMissing`（与 Computed/StopIf 同路径） |
| IF-6 | `cond` 是 Expr，调 sizeof | 返回 `Generic`（SizeofError 等价）—— 无法在 sizeof 接口求值 |
| IF-7 | then_sub 与 else_sub sizeof 不一致 | 由 cond 决定运行时选谁；sizeof 仅常量路径返回确定值 |
| IF-8 | then_sub.parse 失败 | 错误向上传播（含 path，由 StructNode 重建） |
| IF-9 | `If` macro：`If(True, Byte)` | 等价 `IfThenElse(True, Byte, Pass)`，Pass.parse 返回 None |

---

## 3. SwitchNode（PM 决策 1：A+B 混合方案）

### 3.1 PM 决策 1 回顾

keyfunc 返回值类型决定实现路线。三条候选路线（分析报告 §C.1）：

| 路线 | keyfunc 形态 | cases 匹配方式 | FFI | 用户面兼容 |
|------|-------------|---------------|-----|----------|
| **A（ExprProgram i64）** | `this.n`（int 字段）/ int 常量 | Rust 内 i64 == | 零 | 仅 int key |
| **B（FieldRef + PyObject）** | `this.fieldname`（任意类型） | PyObject `__eq__` | N 次（N=cases） | int/str/bytes |
| **C（PyCallable）** | 任意 lambda | Rust→Python 回调 | 1+ 次 | 完全兼容 |

**PM 决策 1**：接受 A+B 混合，拒绝 C。理由（分析报告 §C.1）：
- ADR-014/022 脉络（RepeatUntil v5 删 PyCallable / AdapterCallback 拒绝任意 callable）
- 路线 A 覆盖 int key 主流场景（最常见用法）
- 路线 B 扩展 str/bytes key，cases 通常 ≤5 个，FFI 开销可接受
- 复杂表达式（`this.x + 1`）编译期拒绝，引导用户用 Computed 预计算

### 3.2 编译期 keyfunc 分类

Python 用户面：`Switch(keyfunc, cases, default=Pass)`。`keyfunc` 在 `__init_subclass__`
编译期由 Python 侧 `_extract_and_compile_exprs` 分类（与 StopIf 的 `condfunc` 同管道）：

| keyfunc 形式 | 编译期分类 | expr_programs 提供的形态 | Rust 路线 |
|-------------|-----------|------------------------|----------|
| `1`（int 常量）/ `True`/`False` | 常量 | `None`（无表达式） | `SwitchKey::ConstInt(i64)` |
| `this.n`（单字段引用，n 是 int 字段） | 单字段 int 表达式 | `{"key": [GetInt(idx)]}` | `SwitchKey::IntExpr(ExprProgram)` |
| `this.tag`（单字段引用，tag 是 str/bytes 字段） | 单字段 ref（非 int 表达式） | `{"key_field": "tag"}` 字符串 | `SwitchKey::FieldRef(FieldName)` |
| `this.x + 1` / `this.tag.lower()` / 任意 lambda | 复杂表达式 | 编译期拒绝 | — |

**编译期拒绝逻辑**：Python 侧识别 keyfunc 不是常量、不是单 FieldRef、不是单 int ExprRef 时，
抛 `CompilationError`，引导用户用 `"key" / Computed(this.x + 1)` 预计算后再 `Switch(this.key, ...)`。
此约束与 ADR-014 强制表达式的硬约束同脉络（构造器内部决策不背 PyCallable 包袱）。

### 3.3 struct 定义

```rust
/// Switch 的 keyfunc 来源（PM 决策 1：A+B 混合）。
///
/// 编译期从用户面 keyfunc 分类：
/// - `ConstInt(k)`：常量 int key（罕见，主要用于调试）
/// - `IntExpr(prog)`：单字段 int 表达式（`this.n`，n 是 int 字段）→ 路线 A（零 FFI）
/// - `FieldRef(name)`：单字段引用（`this.tag`，tag 是 str/bytes 字段）→ 路线 B（PyObject __eq__）
///
/// 复杂表达式（`this.x + 1`）编译期拒绝（引导用户用 Computed 预计算）。
#[derive(Debug, Clone)]
pub enum SwitchKey {
    /// 常量 int key（编译期已知）。
    ConstInt(i64),
    /// 单字段 int 表达式（路线 A）。
    /// 运行时求值 ExprProgram 得 i64 → Rust 内 i64 == 匹配 cases（零 FFI）。
    IntExpr(ExprProgram),
    /// 单字段引用（路线 B）。
    /// 运行时从 ctx 读 PyObject → cases 用 PyObject `__eq__` 匹配（每比较 1 FFI）。
    FieldRef(FieldName),
}

/// 单个 case 项：Python key + 预编译的 subcon。
///
/// 编译期从 Python dict `{key: subcon}` 展开。每个 case 同时缓存：
/// - `key_py`：Python 侧 key（int/str/bytes），用于 PyObject `__eq__`
/// - `key_int`：若 key 是 int，额外存 i64 用于零 FFI 快速匹配（路线 A 快速路径）
#[derive(Debug)]
pub struct SwitchCase {
    /// Python 侧 key（原始对象，用于 PyObject `__eq__`）。
    key_py: Py<PyAny>,
    /// 若 key 是 int，缓存其 i64 值（路线 A 快速匹配用）；否则 None。
    key_int: Option<i64>,
    /// 该 key 对应的子树。
    subcon: Box<Node>,
}

/// 多分支条件节点（对应 Python construct `Switch`，core.py L4002）。
///
/// # 三方法行为
///
/// 对齐 Python `Switch._parse` / `_build` / `_sizeof`（L4041-4058）：
/// - parse：求 key → 匹配 cases → 委托 subcon.parse（未命中走 default）
/// - build：对称
/// - sizeof：求 key → 匹配 → 委托 sizeof；求值失败 → SizeofError（Generic）
///
/// # 路线 A+B 混合匹配（PM 决策 1）
///
/// 求出 key 后的匹配顺序（核心优化）：
/// 1. **路线 A 快速路径**：若 key 是 IntExpr 求值结果 i64，遍历 cases 的 `key_int`，
///    Rust 内 i64 == 匹配（零 FFI）
/// 2. **路线 B 慢路径**：快速路径未命中，或 key 是 FieldRef(PyObject)，遍历 cases
///    用 `PyObject_RichCompare` 求 `__eq__`（每比较 1 FFI）
/// 3. **default 兜底**：全部未命中，委托 default subcon
///
/// # 已知限制（与 Python 不完全 parity）
///
/// - `default=Error` 不支持：Python 的 `Error` 是抛错构造器（core.py 未定义类），
///   Phase 7 default 仅接 Pass 或已实现 Node（文档化为已知限制）
#[derive(Debug)]
pub struct SwitchNode {
    /// keyfunc 编译期分类。
    key: SwitchKey,
    /// 编译期从 Python dict 展开的 cases 列表。
    cases: Vec<SwitchCase>,
    /// 默认 subcon（编译期保证非空——Python default=None 时设为 PassNode）。
    default: Box<Node>,
}

impl SwitchNode {
    pub fn new(key: SwitchKey, cases: Vec<SwitchCase>, default: Node) -> Self {
        Self {
            key,
            cases,
            default: Box::new(default),
        }
    }

    pub fn key(&self) -> &SwitchKey { &self.key }
    pub fn cases(&self) -> &[SwitchCase] { &self.cases }
    pub fn default(&self) -> &Node { &self.default }

    /// has_expressions：IntExpr 与 FieldRef 都引用 Struct 字段，返回 true。
    /// ConstInt 不引用字段，返回 false。
    pub fn has_expressions(&self) -> bool {
        !matches!(self.key, SwitchKey::ConstInt(_))
    }
}
```

### 3.4 key 求值 + 匹配算法

```rust
impl SwitchNode {
    /// 求值 keyfunc，返回 (i64 快速匹配值, PyObject 慢匹配值)。
    ///
    /// - ConstInt(k) → (Some(k), None) —— 仅走快速路径
    /// - IntExpr(prog) → (Some(求值结果), None) —— 仅走快速路径
    /// - FieldRef(name) → (None, Some(PyObject)) —— 仅走慢路径
    ///
    /// IntExpr 求值失败（字段缺失）错误向上传播（与 Computed 同路径）。
    fn eval_key<'py>(
        &self,
        ctx: &Context<'py>,
        py: Python<'py>,
    ) -> Result<(Option<i64>, Option<Py<PyAny>>), ConstructError> {
        match &self.key {
            SwitchKey::ConstInt(k) => Ok((Some(*k), None)),
            SwitchKey::IntExpr(prog) => {
                // 复用 ExprProgram::try_eval_simple_cmp fast-path（与 StopIf 同模式）。
                // 单字段 GetInt 是 try_eval_simple_cmp 未覆盖的模式（仅 1 op），
                // 直接走 eval_expr_int 通用路径。
                let v = eval_expr_int(prog, ctx, py)?;
                Ok((Some(v), None))
            }
            SwitchKey::FieldRef(name) => {
                // 从 ctx.fields dict 读 PyObject（key = interned field name）。
                // 实施细节：通过 ctx.get_field(name.py_name()) 或类似 API。
                // 字段缺失 → ExprFieldMissing（与 Computed 同路径）。
                let val = ctx.get_field_py(name.py_name(), py)?;
                Ok((None, Some(val)))
            }
        }
    }

    /// 匹配 cases：先 i64 快速路径，再 PyObject __eq__ 慢路径。
    ///
    /// 返回匹配的 subcon 引用；未命中返回 default。
    fn match_sub<'a>(
        &'a self,
        key_int: Option<i64>,
        key_py: Option<&Py<PyAny>>,
        py: Python<'_>,
    ) -> &'a Node {
        // 路线 A 快速路径
        if let Some(k) = key_int {
            for case in &self.cases {
                if let Some(ck) = case.key_int {
                    if ck == k {
                        return &case.subcon;
                    }
                }
            }
            // 快速路径未命中：若 key_py 也存在（不可能，eval_key 二选一），
            // 不再走慢路径——直接 default。但若某些 case 的 key 是 str/bytes
            // 而 keyfunc 返回 int，i64 != str/bytes 永不匹配，走 default 正确。
        }
        // 路线 B 慢路径
        if let Some(kp) = key_py {
            for case in &self.cases {
                // PyObject_RichCompare(kp, case.key_py, Py_EQ) == Py_True
                if py_eq(kp, &case.key_py, py) {
                    return &case.subcon;
                }
            }
        }
        &self.default
    }
}
```

**`py_eq` 辅助**（位置：switch.rs 内私有函数）：

```rust
/// PyObject `__eq__` 比较（CPython `PyObject_RichCompare` 包装）。
///
/// 返回 true 当且仅当 `a == b`（Py_EQ）且结果非异常。
/// 异常（如自定义 `__eq__` 抛错）按 Python 语义视为不匹配（返回 false），
/// 与 Python `dict.get` 的 `__hash__ + __eq__` 一致。
fn py_eq(a: &Py<PyAny>, b: &Py<PyAny>, py: Python<'_>) -> bool {
    use pyo3::ffi;
    // SAFETY: 持有 GIL（py token），a/b 是有效 PyObject。
    let result = unsafe { ffi::PyObject_RichCompare(a.as_ptr(), b.as_ptr(), ffi::Py_EQ) };
    if result.is_null() {
        // 比较抛异常：清除异常，按不匹配处理。
        unsafe { ffi::PyErr_Clear(); }
        return false;
    }
    let is_true = unsafe { ffi::PyObject_IsTrue(result) } == 1;
    unsafe { ffi::Py_DecRef(result); };
    is_true
}
```

### 3.5 Construct impl

```rust
impl Construct for SwitchNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let (key_int, key_py) = self.eval_key(ctx, py)?;
        let sub = self.match_sub(key_int, key_py.as_ref(), py);
        sub.parse(py, stream, ctx, path)
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        let (key_int, key_py) = self.eval_key(ctx, py)?;
        let sub = self.match_sub(key_int, key_py.as_ref(), py);
        sub.build(py, obj, stream, ctx, path)
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // 对齐 Python L4051-4058：try evaluate(keyfunc) → match → sizeof；
        // except (KeyError, AttributeError) → SizeofError。
        //
        // sizeof 接口无 py token，无法求值表达式。处理：
        // - ConstInt(k) → match_sub → sizeof（确定路径）
        // - IntExpr / FieldRef → 返回 Generic（SizeofError 等价）
        match &self.key {
            SwitchKey::ConstInt(k) => {
                let sub = self.match_sub(Some(*k), None, Python::with_gil(|py| py));
                // 注：Python::with_gil 在 sizeof 接口下取 GIL 仅用于 match_sub 的
                // PyObject 比较（ConstInt 时无 PyObject 比较，py token 不实际使用）。
                // 实施时若 match_sub 的 py 参数仅用于 py_eq 而 key_py=None，
                // 可重构 match_sub 使 py 参数可选——DEV 决定。
                sub.sizeof(ctx)
            }
            SwitchKey::IntExpr(_) | SwitchKey::FieldRef(_) => Err(ConstructError::Generic {
                message: "Switch size is undefined when keyfunc is not a constant".to_string(),
                path: String::new(),
            }),
        }
    }
}
```

**sizeof 实现注解**：`Python::with_gil` 在 `sizeof` 中取 GIL 是次优解。替代方案：
将 `match_sub` 拆为 `match_sub_int`（无需 py）和 `match_sub_py`（需 py），`sizeof` 仅调
`match_sub_int`。DEV 实施时选择更清晰的方案。本设计推荐拆分（避免 sizeof 路径无谓获取 GIL）。

### 3.6 编译入口（compile.rs）

新增 `"SwitchDescriptor"` 分支：

```rust
"SwitchDescriptor" => {
    return Ok(Node::Switch(build_switch_node(
        py, desc, field_index, expr_programs,
    )?));
}
```

`build_switch_node` 流程：
1. 读 `desc.keyfunc` → 分类（参照 IfThenElse 的 cond 分类逻辑）：
   - `extract::<bool>()` 成功 → 转为 `ConstInt(1)` / `ConstInt(0)`（罕见，Python bool 也是 int）
   - `extract::<i64>()` 成功（int 常量）→ `ConstInt(k)`
   - 从 `expr_programs[field_index]` 取：
     - 有 `"key"` 键（ExprOp 列表，单 GetInt）→ `IntExpr(ExprProgram::new(ops))`
     - 有 `"key_field"` 键（字符串字段名）→ `FieldRef(FieldName::new(py, name))`
     - 都无 → `CompilationError`（复杂表达式拒绝，引导用户用 Computed 预计算）
2. 读 `desc.cases`（Python dict）→ 遍历展开为 `Vec<SwitchCase>`：
   - 每个 `(key_py, subcon_desc)` → `SwitchCase { key_py, key_int: key_py.extract::<i64>().ok(), subcon: Box::new(build_node_from_descriptor(subcon_desc, ...)) }`
3. 读 `desc.default`（Python 对象）：
   - `None` → `PassNode::new()`（对齐 Python L4032-4033）
   - 其他 → 递归 `build_node_from_descriptor`
4. 返回 `SwitchNode::new`

**注**：Python 用户面 `default=None` 时由 Python 侧替换为 `Pass` 实例（与原版 L4032 一致），
Rust 收到的 `desc.default` 已是 `PassDescriptor` 或其他 Node Descriptor，不会是 `None`。
但 Rust 侧仍防御性检查：若 `desc.default` 是 `None`（理论上不应发生），按 `PassNode` 处理。

### 3.7 边界条件

| # | 场景 | 预期行为 |
|---|------|---------|
| SW-1 | `cases = {}`（空）+ `default=Pass` | 任意 key 都走 default（Pass） |
| SW-2 | `keyfunc = this.n`，n=2，cases={1:A, 2:B} | IntExpr 求值=2 → 快速路径匹配 B |
| SW-3 | `keyfunc = this.n`，n=99，cases={1:A, 2:B} | 快速路径全部未命中 → default |
| SW-4 | `keyfunc = this.n`，n 字段缺失 | `ExprFieldMissing` |
| SW-5 | `keyfunc = this.tag`，tag="foo"，cases={"foo":A, "bar":B} | FieldRef 读 PyObject → 慢路径 `__eq__` 匹配 A |
| SW-6 | `keyfunc = this.tag`，tag="x"，cases={"foo":A} | 慢路径 `__eq__` 全部 false → default |
| SW-7 | cases key 类型与 keyfunc 返回不匹配（int keyfunc, str cases） | 快速路径 i64 != None（cases.key_int 全 None）→ 慢路径（key_py=None 不走）→ default |
| SW-8 | `default=Error`（Python） | **Phase 7 不支持**——Error 构造器未实现。文档化为已知限制 |
| SW-9 | IntExpr 路径 sizeof 调用 | 返回 `Generic`（无法求值） |
| SW-10 | ConstInt(2) 路径 sizeof 调用 | match cases → 委托 sizeof（确定路径） |
| SW-11 | cases 中 case 的 subcon 是复杂 Node（如嵌套 Struct） | 递归 parse/build 正常工作 |
| SW-12 | 慢路径 `__eq__` 抛异常 | `py_eq` 捕获 → 视为不匹配 → 继续 / default |

---

## 4. SelectNode

### 4.1 行为概述

Python `Select` 遍历 subcons 尝试 parse/build，首个成功者胜出。关键语义（core.py L3860-3884）：

```python
def _parse(self, stream, context, path):
    for sc in self.subcons:
        fallback = stream_tell(stream, path)
        try:
            obj = sc._parsereport(stream, context, path)
        except ExplicitError:
            raise                  # ← ExplicitError 不吞，直接传播
        except Exception:
            stream_seek(stream, fallback, 0, path)   # ← 失败回退流位置
        else:
            return obj
    raise SelectError("no subconstruct matched", path=path)

def _build(self, obj, stream, context, path):
    for sc in self.subcons:
        try:
            data = sc.build(obj, **context)   # ← 在新 BytesIO 尝试
        except ExplicitError:
            raise
        except Exception:
            pass
        else:
            stream_write(stream, data, len(data), path)
            return obj
    raise SelectError("no subconstruct matched: %s" % (obj,), path=path)
```

### 4.2 struct 定义

```rust
/// 多分支尝试节点（对应 Python construct `Select`，core.py L3830）。
///
/// # 三方法行为
///
/// ## parse
///
/// 遍历 subcons：
/// 1. `fallback = stream.tell()`
/// 2. 调 `sub.parse(...)`
///    - `Ok(obj)` → return obj（短路，后续不尝试）
///    - `Err(Explicit)` → **直接 return Err**（Python `except ExplicitError: raise`）
///    - `Err(其他)` → `stream.seek(fallback)` 继续
/// 3. 全部失败 → `ConstructError::Select`
///
/// ## build
///
/// 遍历 subcons：
/// 1. 创建临时 `BuildStream`（与 PrefixedArray build 同模式）
/// 2. 调 `sub.build(obj, &mut temp_stream, ...)`
///    - `Ok(())` → 把 temp_stream 字节写到主 stream → return Ok(())（短路）
///    - `Err(Explicit)` → **直接 return Err**（穿透）
///    - `Err(其他)` → 丢弃 temp_stream 继续
/// 3. 全部失败 → `ConstructError::Select`
///
/// ## sizeof
///
/// 永远返回 `Err`（对齐 Python `Select._sizeof` 抛 SizeofError——Python 未定义 _sizeof
/// 时默认 raise SizeofError）。
///
/// # ExplicitError 集成（PM 决策 2）
///
/// `ConstructError::Explicit` 不被 Select 吞掉，直接向上传播。
/// 这与 Python parity——Python 用户用 `Error` 构造器（Phase 7 暂不实现）或
/// Adapter 回调主动抛 ExplicitError 时，Select 必须传播而非吞掉。
///
/// # 已知差异
///
/// Python `Select._build` 在 inner build 抛非 Explicit 错误时**不回退 stream**
/// （因为 build 到 temp BytesIO，失败时 temp 丢弃即可）。construct-rs 行为一致。
#[derive(Debug)]
pub struct SelectNode {
    /// 候选 subcon 列表（编译期保证非空——Python 允许空 Select 但 parse 立即抛 SelectError）。
    subcons: Vec<Node>,
}

impl SelectNode {
    pub fn new(subcons: Vec<Node>) -> Self {
        Self { subcons }
    }

    pub fn subcons(&self) -> &[Node] { &self.subcons }

    /// has_expressions：递归检查任一 subcons 子树。
    /// 与 Bitwise/Transform 同模式。
    pub fn has_expressions(&self) -> bool {
        self.subcons.iter().any(|s| s.has_expressions())
    }
}
```

### 4.3 Construct impl

```rust
impl Construct for SelectNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        for sub in &self.subcons {
            let fallback = stream.tell();
            match sub.parse(py, stream, ctx, path) {
                Ok(obj) => return Ok(obj),
                Err(e) => {
                    // Explicit 不吞，直接传播（Python L3865-3866）。
                    if matches!(e, ConstructError::Explicit { .. }) {
                        return Err(e);
                    }
                    // 其他错误：seek 回 fallback，继续尝试下一个。
                    // seek 失败也忽略（与 Python except Exception 一致）。
                    let _ = stream.seek(fallback, path);
                }
            }
        }
        Err(ConstructError::Select {
            message: "no subconstruct matched".to_string(),
            path: path.to_string(),
        })
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        for sub in &self.subcons {
            // 在临时 BuildStream 上尝试（与 PrefixedArray build 同模式）。
            let mut temp_stream = BuildStream::new();
            match sub.build(py, obj, &mut temp_stream, ctx, path) {
                Ok(()) => {
                    // 成功：把 temp 字节写到主 stream。
                    let bytes = temp_stream.as_bytes();
                    stream.write(bytes);
                    return Ok(());
                }
                Err(e) => {
                    if matches!(e, ConstructError::Explicit { .. }) {
                        return Err(e);
                    }
                    // 其他错误：丢弃 temp，继续尝试。
                }
            }
        }
        Err(ConstructError::Select {
            message: format!("no subconstruct matched: {}", obj),
            path: path.to_string(),
        })
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // Python Select 无 _sizeof 方法定义，默认抛 SizeofError。
        Err(ConstructError::Generic {
            message: "Select size is undefined".to_string(),
            path: String::new(),
        })
    }
}
```

### 4.4 编译入口（compile.rs）

新增 `"SelectDescriptor"` 分支（参照 PrefixedArrayDescriptor 递归编译 subcon 列表的模式）：

```rust
"SelectDescriptor" => {
    let subcons_list = desc
        .getattr("subcons")
        .map_err(|e| ConstructError::Compilation {
            message: format!("SelectDescriptor missing 'subcons' attribute: {}", e),
        })?;
    let subcons_iter = subcons_list.try_iter().map_err(|e| ConstructError::Compilation {
        message: format!("SelectDescriptor 'subcons' is not iterable: {}", e),
    })?;
    let mut nodes = Vec::new();
    for sub_desc in subcons_iter {
        let sub_node = build_node_from_descriptor(
            py, &sub_desc?, field_index, expr_programs, field_names, false,
        )?;
        nodes.push(sub_node);
    }
    return Ok(Node::Select(SelectNode::new(nodes)));
}
```

### 4.5 边界条件

| # | 场景 | 预期行为 |
|---|------|---------|
| SL-1 | `subcons = []`（空） | parse 立即 `Select`；build 立即 `Select` |
| SL-2 | 第 1 个 subcon 成功 | 短路 return，后续不尝试 |
| SL-3 | 第 1 个失败（Stream 错误）+ 第 2 个成功 | 第 1 个 seek 回 fallback；第 2 个成功 |
| SL-4 | 某 subcon 抛 `Explicit` | **立即传播**（不尝试后续，不 seek 回退） |
| SL-5 | 全部失败 | `ConstructError::Select` |
| SL-6 | build 方向：第 1 个失败 + 第 2 个成功 | temp_stream 丢弃；第 2 个 temp 字节写到主 stream |
| SL-7 | build 方向：某 subcon 抛 `Explicit` | **立即传播**（不写主 stream） |
| SL-8 | sizeof 调用 | 永远 `Generic`（SizeofError 等价） |
| SL-9 | `Optional(subcon)` macro（= `Select(subcon, Pass)`） | Python macro；Rust 侧 Select 行为正确（Pass 总成功） |

---

## 5. FocusedSeqNode

### 5.1 是否复用 StructNode 逻辑（ARCH 决策：不复用）

**两条候选路线**（分析报告 §B.1.4）：
- **路线 A**：复用 StructNode 机制（`FocusedSeqNode { fields, focus_index }` 委托 StructNode parse/build）
- **路线 B**：独立 Node，自行管理 context nesting

**ARCH 决策：路线 B（独立 Node，不复用 StructNode）**。理由：

1. **返回值类型不兼容**：StructNode.parse 返回用户类实例（R4：借用实例 `__dict__`），
   FocusedSeq.parse 需返回单字段值（聚焦字段的 PyObject）。若复用 StructNode.parse，
   需在外层"提取实例的 focus 属性"——这又跨 FFI（getattr），且语义上 FocusedSeq 没有"用户类"
   （Python FocusedSeq 直接返回 finalret，不构造实例）。
2. **build 输入语义不同**：StructNode.build 从用户实例逐字段 getattr；
   FocusedSeq.build 接收单值 obj，只赋给 focus 字段，其余字段传 None（对齐 Python L3254）。
3. **context nesting 来源不同**：StructNode 的 context nesting 由 StructRefNode 委托时
   通过 `Context::new_child_placeholder` 注入；FocusedSeq 需**主动** new_child（Python L3237 显式 `Container(_ = context, ...)`）。
4. **耦合代价**：复用 StructNode 需在 StructNode 内部加"focus 字段提取钩子"或
   "返回单字段值模式"，破坏 StructNode 的单一职责（字段序列根节点）。

**FocusedSeq 独立实现的成本**：context nesting 逻辑（`Context::new_child` + 字段遍历 +
set_field）~100 行，远小于复用 StructNode 引入的耦合复杂度。

### 5.2 struct 定义

```rust
/// FocusedSeq 字段（与 StructField 平行但独立，避免 mode 字段——FocusedSeq 字段无 RW/RO/WO 区分）。
#[derive(Debug)]
pub struct FocusedSeqField {
    /// 字段名（None 表示匿名字段，对应 Python `Renamed` 无名字段）。
    name: Option<FieldName>,
    /// 子节点。
    node: Node,
}

/// 聚焦字段序列节点（对应 Python construct `FocusedSeq`，core.py L3176）。
///
/// # 三方法行为
///
/// ## parse（对齐 Python L3236-3246）
///
/// 1. `Context::new_child` 创建嵌套 context（`_` 指向外层）
/// 2. （若 has_expressions）`init_expr_values(n_fields)`
/// 3. 遍历 fields：
///    - `field.node.parse(...)` → 求值
///    - 若 `field.name` 非 None：`ctx.set_field_at(idx, name, value)` 写入 child ctx
///    - 若 `field.name == focus_name`：记录 `finalret = value`
/// 4. return `finalret`（聚焦字段的值）
/// 5. 若 focus 字段未被执行（focus_name 不匹配任何字段名）→ `Generic`（UnboundLocalError 等价）
///
/// ## build（对齐 Python L3248-3259）
///
/// 1. `Context::new_child` 创建嵌套 context
/// 2. （若 has_expressions）`init_expr_values(n_fields)`
/// 3. **预置** focus 字段值：`ctx.set_field_at(focus_idx, focus_name, obj)`
/// 4. 遍历 fields：
///    - 若 `field.name == focus_name`：调 `field.node.build(obj, ...)`
///    - 否则：调 `field.node.build(None, ...)`（非聚焦字段传 None）
///    - 若 `field.name` 非 None 且 buildret 非 None：`ctx.set_field_at` 更新
/// 5. return `Ok(())`
///
/// ## sizeof（对齐 Python L3261-3267）
///
/// 1. `Context::new_child`（nested ctx）
/// 2. sum 所有 fields sizeof；任一 Err → 整体 Err
///
/// # context nesting
///
/// FocusedSeq 主动 new_child（与 StructRefNode 委托 StructNode 的隐式 nesting 不同）。
/// child ctx 的 `_` 指向外层 ctx（Python L3237：`Container(_ = context, ...)`）。
///
/// # has_expressions
///
/// 递归检查 fields 子树（与 Bitwise 同模式）。
#[derive(Debug)]
pub struct FocusedSeqNode {
    /// 有序字段列表。
    fields: Vec<FocusedSeqField>,
    /// 聚焦字段在 fields 中的索引（编译期从 parsebuildfrom 解析）。
    /// 编译期保证：fields[focus_idx].name == Some(parsebuildfrom)。
    focus_idx: usize,
    /// 该 FocusedSeq 是否含表达式（编译期计算，递归子树）。
    has_expressions: bool,
}

impl FocusedSeqNode {
    pub fn new(
        fields: Vec<FocusedSeqField>,
        focus_idx: usize,
        has_expressions: bool,
    ) -> Self {
        Self { fields, focus_idx, has_expressions }
    }

    pub fn fields(&self) -> &[FocusedSeqField] { &self.fields }
    pub fn focus_idx(&self) -> usize { self.focus_idx }
    pub fn has_expressions(&self) -> bool { self.has_expressions }
}
```

### 5.3 Construct impl（关键算法）

```rust
impl Construct for FocusedSeqNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 1. context nesting
        let mut child_ctx = Context::new_child(ctx, py)?;
        if self.has_expressions {
            child_ctx.init_expr_values(self.fields.len());
        }

        // 2. 遍历 fields
        let mut finalret: Option<Py<PyAny>> = None;
        for (idx, field) in self.fields.iter().enumerate() {
            let parseret = field.node.parse(py, stream, &mut child_ctx, path)?;
            if let Some(name) = &field.name {
                // 命名字段写入 child ctx（与 StructNode 同模式）
                child_ctx.set_field_at(idx, name.py_name(), parseret.bind(py), py)?;
            }
            if idx == self.focus_idx {
                finalret = Some(parseret);
            }
        }

        // 3. 返回 focus 字段值（编译期保证 focus_idx 有效，finalret 必为 Some）
        finalret.ok_or_else(|| ConstructError::Generic {
            message: "FocusedSeq focus field not executed (compiler invariant violated)"
                .to_string(),
            path: path.to_string(),
        })
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        let mut child_ctx = Context::new_child(ctx, py)?;
        if self.has_expressions {
            child_ctx.init_expr_values(self.fields.len());
        }

        // 预置 focus 字段值（Python L3252: context[parsebuildfrom] = obj）
        let focus_field = &self.fields[self.focus_idx];
        if let Some(name) = &focus_field.name {
            child_ctx.set_field_at(self.focus_idx, name.py_name(), obj, py)?;
        }

        for (idx, field) in self.fields.iter().enumerate() {
            // focus 字段传 obj，其余传 None
            let build_obj = if idx == self.focus_idx {
                obj
            } else {
                py.None().bind(py)
            };
            field.node.build(py, build_obj, stream, &mut child_ctx, path)?;
            // buildret 非 None 时更新 ctx（Python L3255-3256）。
            // construct-rs build 不返回值，省略此步（与 PrefixedArray 同简化）。
        }
        Ok(())
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // sizeof 接口无 py token，无法 new_child（new_child 需要 py 创建 PyDict）。
        // 替代：直接在父 ctx 上 sum sizeof（context nesting 仅影响字段间引用，
        // 不影响静态 sizeof——Python L3265 也是 sum(sc._sizeof for sc in subcons)）。
        // 字段间引用的 sizeof（如 Padding(lambda this: this.count)）通过 ExprProgram 求值，
        // 但 ExprProgram 求值需要 py——sizeof 接口的限制使此类场景返回 Err（与 Python 一致：
        // Python L3266-3267 except (KeyError, AttributeError) → SizeofError）。
        let mut total = 0usize;
        for field in &self.fields {
            total = total.checked_add(field.node.sizeof(ctx)?).ok_or_else(|| {
                ConstructError::Generic {
                    message: "FocusedSeq sizeof overflow".to_string(),
                    path: String::new(),
                }
            })?;
        }
        Ok(total)
    }
}
```

**sizeof 注解**：FocusedSeq 的 sizeof 不做 context nesting（与 Python L3261-3265 的
`Container(_ = context, ...)` 严格对照有差异）。理由：
1. sizeof 接口 `fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError>` 无 `py` token，
   无法 `Context::new_child`（需创建 PyDict）。
2. sizeof 的语义是"静态大小"——若任一字段大小依赖嵌套 ctx 的字段引用（如
   `Padding(this.count)`），Python 在 except (KeyError, AttributeError) 路径抛 SizeofError；
   construct-rs 在 ExprProgram 求值失败时同样抛 Err（Generic/ExprFieldMissing）。
3. 实际场景：FocusedSeq 的字段大小通常不依赖 nested ctx（依赖的话用户应自行处理），
   此差异不影响主流用法。

### 5.4 编译入口（compile.rs）

新增 `"FocusedSeqDescriptor"` 分支：

```rust
"FocusedSeqDescriptor" => {
    return Ok(Node::FocusedSeq(build_focused_seq_node(
        py, desc, field_index, expr_programs, field_names,
    )?));
}
```

`build_focused_seq_node` 流程：
1. 读 `desc.parsebuildfrom` → 解析为字段名字符串（Python str 或 context lambda）
   - 若是 str → 直接用作 focus 字段名
   - 若是 lambda（FieldRef 单字段）→ 从 expr_programs 取字段名
   - 其他 → `CompilationError`
2. 读 `desc.subcons`（Python 列表）→ 遍历展开为 `Vec<FocusedSeqField>`：
   - 每个元素是 `Renamed` 包装（含 name + subcon）或匿名 subcon
   - 提取 name（Option<String>）→ `Option<FieldName>`（Some 时 intern）
   - 递归 `build_node_from_descriptor` subcon
3. 在 fields 中查找 focus_name 匹配的 index → `focus_idx`
   - 未找到 → `CompilationError`（Python UnboundLocalError 等价，编译期暴露）
4. 计算 `has_expressions`：递归 fields 子树（与 Bitwise 同模式）
5. 返回 `FocusedSeqNode::new`

### 5.5 边界条件

| # | 场景 | 预期行为 |
|---|------|---------|
| FS-1 | `FocusedSeq("num", Const(b"SIG"), "num"/Byte, Terminated)` parse `b"SIG\xff"` | 返回 255（num 字段值） |
| FS-2 | parsebuildfrom 不匹配任何字段名 | **编译期** `CompilationError`（比 Python UnboundLocalError 更早暴露） |
| FS-3 | 非聚焦字段 build 时收到 None | 该字段 subcon 必须支持 build(None)（如 Rebuild/Computed/Const/Pass） |
| FS-4 | 聚焦字段本身是匿名（name=None） | **编译期** `CompilationError`（parsebuildfrom 必须匹配命名字段） |
| FS-5 | 字段间引用（"data" / Padding(this.count)） | context nesting 使 `this.count` 在 child ctx 中可读 |
| FS-6 | sizeof 调用且某字段大小依赖嵌套 ctx | 返回 Err（ExprFieldMissing 等，对齐 Python SizeofError） |
| FS-7 | 嵌套 FocusedSeq（FocusedSeq 内 FocusedSeq） | context 链式 nesting（child 的 `_` 指向外层 child） |
| FS-8 | focus 字段在 fields 中重复名（理论上不应发生） | 编译期取第一个匹配（与 Python `finalret` 覆盖语义一致） |
| FS-9 | PrefixedArray 内部用 FocusedSeq（Python L4956） | construct-rs PrefixedArrayNode 已独立实现，不依赖 FocusedSeq |

---

## 6. Node enum 扩展（nodes/mod.rs）

在 `Node` enum 新增 4 个变体（位置：现有 40 变体之后，Phase 7 区块）：

```rust
pub enum Node {
    // ... 既有 40 变体 ...

    // === Phase 7.1 Conditional（4 个新变体） ===
    /// 双分支条件节点（对应 Python construct `IfThenElse`，Phase 7.1）。
    /// 条件可为常量或 ExprProgram；then/else 都是 Box<Node>。
    IfThenElse(IfThenElseNode),
    /// 多分支条件节点（对应 Python construct `Switch`，Phase 7.1）。
    /// keyfunc 走 ExprProgram(i64) + FieldRef(PyObject) 混合（PM 决策 1）。
    Switch(SwitchNode),
    /// 多分支尝试节点（对应 Python construct `Select`，Phase 7.1）。
    /// 遍历 subcons，首个成功者胜出；ExplicitError 穿透。
    Select(SelectNode),
    /// 聚焦字段序列节点（对应 Python construct `FocusedSeq`，Phase 7.1）。
    /// context nesting + 返回单聚焦字段值。
    FocusedSeq(FocusedSeqNode),
}
```

`has_expressions` 方法新增 4 分支：

```rust
Node::IfThenElse(i) => i.has_expressions(),
Node::Switch(s) => s.has_expressions(),
Node::Select(s) => s.has_expressions(),
Node::FocusedSeq(f) => f.has_expressions(),
```

**`compute_ro_value`**：4 个新 Node 都**不作为 RO 字段**（与 Struct/Bytes等同级，非 RO 语义）。
现有 `_ => Err(Generic)` 兜底分支自动覆盖，无需修改。

`mod.rs` 的 `pub mod` 声明新增 4 行：

```rust
pub mod focused_seq;
pub mod if_then_else;
pub mod select;
pub mod switch;
```

---

## 7. §0 原则对照表（L-01 对策，硬要求）

| §0 原则 | Conditional 设计如何满足 | 验证点 |
|---------|------------------------|--------|
| **#1 一次 FFI** | IfThenElse：cond 走 ExprProgram（零 FFI），then/else 是 `Box<Node>`（Rust 内部）。Switch 路线 A：IntExpr 求值 i64 后 Rust 内 `==` 匹配（零 FFI）。Switch 路线 B：FieldRef 读 PyObject 后 `PyObject_RichCompare`（每比较 1 FFI，N=cases 数，用户主动选 str/bytes key）。Select：subcons 都是 `Vec<Node>`，tell/seek 是 Rust 内部。FocusedSeq：context nesting 纯 Rust（`Context::new_child`），字段都是 `Node`。 | parse/build 各只有一次 Python↔Rust 边界穿越（FFI 入口）；内部决策零 FFI（路线 A）或受控 FFI（路线 B，cases 数决定）。 |
| **#2 无中间表示层** | IfThenElse：subcon.parse 直接产 PyObject。Switch：matched subcon.parse 直接产 PyObject（key 匹配仅用 i64 / PyObject 比较，不引入中间数据类型）。Select：subcon.parse 直接产 PyObject；build temp BuildStream 是输出缓冲本身（非中间数据类型，与 PrefixedArray 同）。FocusedSeq：focus 字段值就是 subcon.parse 产出的 PyObject。 | 全程无"先构造 Rust enum/dict 再转 PyObject"的中间层。 |
| **#3 输入输出侧无 trait 抽象层** | 4 个新 Node 都持 `Box<Node>` / `Vec<Node>`，不引入新 trait。subcon 调用通过 enum_dispatch 静态分派。 | 无 `dyn Construct` / `Box<dyn Subcon>` 等抽象。 |
| **#4 pyo3 核心依赖** | 单 crate；4 个新 Node 都在 `construct-rs/src/nodes/`。 | 无独立 Python 绑定层。 |
| **#5 mashumaro 式 API** | N/A（Conditional 不涉及用户面 dataclass；用户面 `If`/`Switch`/... 通过 Python 描述符系统，与原版一致）。 | — |
| **#6 enum_dispatch 静态分派** | 4 个新 Node 加入 `Node` enum（§6），enum_dispatch 自动生成 match 分派。 | 无 `Box<dyn>` 动态分派。 |
| **#7 Result<T, ConstructError> + thiserror** | 新增 `Explicit` / `Select` 变体（§1.2）；`path` 字段贯穿；`push_path_segment` 自动支持新变体（§1.3 set_path）。 | 所有错误走 `Result`；新变体映射到 Python 异常类（ExplicitError / SelectError）。 |
| **#8 Stream 抽象纯 Rust 内部** | IfThenElse/Switch/FocusedSeq：不直接操作流（委托 subcon）。Select：用 `ParseStream::tell/seek` 与 `BuildStream::new/write`（纯 Rust 内部，已有 API）。 | 无 Stream trait 跨 FFI。 |

**结论**：Conditional 设计整体符合 §0 全部 8 条原则。唯一需用户主动决策的是 Switch 路线 B
（str/bytes key）的少量 FFI——但这是用户选 str/bytes key 的固有成本，且 cases 通常 ≤5 个，
FFI 开销可接受（详见 §8 性能假设）。

---

## 8. 性能假设（L-02 / L-05 对策）

### 8.1 瓶颈识别（量化数据 + 来源）

| Node | 主要开销来源 | 量化参考（来自既有 Phase 数据） |
|------|------------|-------------------------|
| IfThenElse | cond 求值（ExprProgram）+ 委托 subcon | ExprProgram 求值 ~6-36ns（Phase 2.5）；委托成本 = subcon 自身 |
| Switch 路线 A | keyfunc 求值 + i64 == 匹配（线性扫描 cases） | 求值 ~6-36ns；i64 == ~1ns/op；cases ≤5 → 匹配 ≤5ns |
| Switch 路线 B | keyfunc 读 PyObject + `PyObject_RichCompare` × N | 读字段 ~10-15ns（PyDict_GetItem）；RichCompare ~50-100ns/op × N（N=cases） |
| Select | tell + 候选 parse + seek（失败时） | tell/seek ~5-10ns；候选 parse 成本 = subcon 自身 |
| FocusedSeq | new_child（PyDict 创建）+ 字段遍历 | new_child ~80-150ns（PyDict new_bound）；字段遍历 = 各字段 subcon 之和 |

### 8.2 可证伪预测（覆盖所有 FFI/拷贝/转换来源）

| 构造器 | 预测加速比（vs Python 2.10.70） | 主要 FFI 来源 | 备注 |
|--------|-----------------------------|-------------|------|
| IfThenElse | ≥10x | cond 求值（路线 Expr）：GetInt 1 次 FFI（PyDict_GetItem，已优化为 expr_values_buf 指针读，~1ns） | Python 走 `evaluate(condfunc, context)` 调用开销 ~500ns+，Rust VM ~6-36ns |
| Switch 路线 A（int key, ≤5 cases） | ≥10x | keyfunc 求值 1 次（GetInt ~1ns） | Python `evaluate` + `dict.get` ~1µs |
| Switch 路线 B（str key, ≤5 cases） | ≥4x（保守）/ ≥10x（理想） | keyfunc 读字段 1 次（~10-15ns）+ `__eq__` ≤5 次（~50-100ns/次） | Python 走 `dict.get` 用 `__hash__ + __eq__`，开销类似但 Python 整体慢 |
| Select（2 候选，第 1 个成功） | ≥10x | 候选 parse 1 次（无失败 seek） | Python try/except 异常处理开销显著 |
| Select（2 候选，第 1 失败第 2 成功） | ≥4x | 候选 1 parse + seek + 候选 2 parse | seek 增加成本；Python 异常处理更慢 |
| FocusedSeq（3 字段，无嵌套） | ≥10x | new_child PyDict 创建 ~80-150ns | Python Container(_=...) 创建 ~500ns+ |

### 8.3 L-05 对策（优化 A 路径忽略 B 路径）

逐一列出设计中**所有** FFI/拷贝/转换来源：

1. **IfThenElse.parse**：cond 求值（GetInt 可能 FFI）+ subcon.parse（内部）。✅ 已在预测中声明。
2. **Switch.parse 路线 A**：GetInt（已优化 expr_values_buf 指针读，~1ns，接近零 FFI）+ i64 == + subcon.parse。✅
3. **Switch.parse 路线 B**：`Context::get_field_py`（PyDict_GetItem，~10-15ns）+ `PyObject_RichCompare` × N + subcon.parse。✅
4. **Select.parse**：tell + 候选 parse（失败时 seek）+ 成功候选 return。✅
5. **Select.build**：temp BuildStream 创建（Rust Vec）+ 候选 build（失败丢弃）+ 成功 write。✅
6. **FocusedSeq.parse**：new_child（PyDict new_bound ~80-150ns）+ set_field_at × N（PyDict_SetItem ~10-15ns/op）。✅
7. **FocusedSeq.build**：new_child + set_field_at × N + 字段 build。✅

**所有来源都在 §8.2 预测中有对应声明**。无"被忽略的 FFI 路径"。

### 8.4 量级标注（L-09 对策）

ns 级估算仅作"量级参考"——实际性能以 PM 复测数据为准。Switch 路线 B 的 `__eq__` 开销
（~50-100ns/op）是 CPython `PyObject_RichCompare` 的典型量级，但实际值受比较对象类型
（int 短路 / str hash / 自定义 `__eq__`）影响，可能有 2-5x 波动。

---

## 9. 与其他模块的交互

### 9.1 依赖（前置条件，Phase 6 完成后已就绪）

| 依赖项 | 来源 | 用途 |
|--------|------|------|
| `StopIfCondition` | Phase 4（stop_if.rs） | IfThenElse 复用（条件分类） |
| `ExprProgram` + `eval_expr_int` | Phase 2（expr.rs） | IfThenElse / Switch 路线 A 求值 |
| `try_eval_simple_cmp` | Phase 4.5 v5.1（expr.rs） | `eval_condition` fast-path（与 StopIf 同） |
| `PassNode` | Phase 6.3（pass.rs） | IfThenElse else / Switch default 默认值 |
| `Context::new_child` / `set_field_at` / `get_field_py` | Phase 1/2（context.rs） | FocusedSeq context nesting |
| `ParseStream::tell/seek` / `BuildStream::new/write` | Phase 1（stream.rs） | Select 候选尝试 |
| `FieldName`（intern PyString） | Phase 1（struct_node.rs） | Switch FieldRef / FocusedSeq 字段名 |
| `ConstructError` enum + Python 异常类缓存 | Phase 1/6（error.rs） | Explicit / Select 变体集成 |
| Descriptor 系统（type name 识别） | Phase 1-6（compile.rs） | 4 个新 Descriptor 编译入口 |

### 9.2 被依赖（后续 Phase 可能用到）

| 被依赖项 | 后续用途 |
|---------|---------|
| IfThenElse | 用户协议条件字段（常见，Phase 8+ 用户场景） |
| Switch | TLV 解析、多类型分支（常见） |
| Select | Union（Phase 8+ Select 是 Union 的基础）/ Optional macro（=`Select(subcon, Pass)`） |
| FocusedSeq | 用户面"精细适配器"（少见，PrefixedArray 已独立实现） |
| `ConstructError::Explicit` | Peek 升级（§1.4）/ 未来 `Error` 构造器（Phase 8+） |
| `eval_condition` 公共辅助 | 未来使用 Condition 模式的构造器（若出现第 3 个，触发 ADR 重命名为 `Condition`） |

### 9.3 跨阶段决策检查（L-04 对策）

| ADR / 教训 | 应用情况 |
|-----------|---------|
| ADR-014（RepeatUntil 删 PyCallable） | Switch 拒绝路线 C（PyCallable），与 ADR-014 脉络一致 |
| ADR-022 PE-3（ExplicitError 推迟到 Phase 7） | §1 引入 Explicit 变体，收尾 ADR-022 PE-3 |
| L-01（中间表示层） | §7 §0 对照表逐条声明 |
| L-04（跨阶段模式未沉淀） | §2.1 复用 StopIfCondition（不新建同构类型）；未来 ADR 触发点已标注 |
| L-05（优化 A 路径忽略 B 路径） | §8.3 列出所有 FFI 来源 |
| L-09（ns 级量级标注） | §8.4 ns 估算标注"量级参考" |

---

## 10. DEV 实施清单

### 10.1 实施顺序（推荐）

```
1. ExplicitError 基础设施（error.rs 新变体 + ExceptionClasses + peek 升级）
   ↓ （Select 依赖 Explicit 变体）
2. SelectNode（最简，验证 Explicit 集成）
   ↓
3. IfThenElseNode（复用 StopIfCondition + eval_condition 提取）
   ↓ （同时落地 If macro Python 侧）
4. SwitchNode（路线 A+B，最复杂之一）
   ↓
5. FocusedSeqNode（context nesting，最复杂之二）
   ↓
6. Node enum 扩展 + compile.rs 4 个 Descriptor 入口
   ↓
7. parity 测试 + 性能测试
```

### 10.2 关键实施检查项

**error.rs**：
- [ ] `ConstructError` 加 `Explicit { message, path }` + `Select { message, path }` 变体
- [ ] `ExceptionClasses` 加 `explicit_error` + `select_error` 字段
- [ ] `init_exception_classes` 加 2 行 `get("ExplicitError")?` / `get("SelectError")?`
- [ ] `is_builtin_class` 数组扩容 14 → 16（O3 fast-path）
- [ ] `message()` / `path()` / `set_path()` / `kind_str()` / `select_exception_class` 各加 2 分支
- [ ] Python `construct._errors` 已导出 `ExplicitError` / `SelectError`（core.py L89/L109 有定义，确认 `_errors` 模块导出）

**peek.rs**：
- [ ] `is_explicit_error` 占位改为 `matches!(e, ConstructError::Explicit { .. })`
- [ ] 新增测试：Peek parse inner 抛 Explicit 时穿透（需手动构造 Explicit 错误场景）

**if_then_else.rs（新建）**：
- [ ] `use crate::nodes::stop_if::StopIfCondition;`
- [ ] struct + new + 访问器 + has_expressions
- [ ] Construct impl（parse/build/sizeof）
- [ ] 单元测试覆盖 IF-1 ~ IF-9

**stop_if.rs（小修改）**：
- [ ] 提取 `eval_condition` 为 `pub(crate)` 自由函数（或位置 `nodes/common.rs`）
- [ ] `StopIfNode::eval_cond` 转发到 `eval_condition`（行为不变）
- [ ] 既有 StopIf 测试全部仍 PASS（回归）

**switch.rs（新建）**：
- [ ] `SwitchKey` enum + `SwitchCase` struct + `SwitchNode` struct
- [ ] `eval_key` + `match_sub` + `py_eq` 私有辅助
- [ ] Construct impl（sizeof 拆 `match_sub_int` / `match_sub_py` 避免 sizeof 取 GIL）
- [ ] 单元测试覆盖 SW-1 ~ SW-12

**select.rs（新建）**：
- [ ] `SelectNode` struct + 访问器 + has_expressions
- [ ] Construct impl（parse 含 Explicit 穿透；build 用 temp BuildStream）
- [ ] 单元测试覆盖 SL-1 ~ SL-9（SL-4/SL-7 需手动构造 Explicit 错误）

**focused_seq.rs（新建）**：
- [ ] `FocusedSeqField` + `FocusedSeqNode` struct
- [ ] Construct impl（new_child + 字段遍历 + focus 返回）
- [ ] 单元测试覆盖 FS-1 ~ FS-9

**nodes/mod.rs**：
- [ ] `pub mod` 4 行
- [ ] `Node` enum 加 4 变体（位置：Phase 7.1 区块）
- [ ] `has_expressions` 加 4 分支
- [ ] `compute_ro_value` 无需改（4 新 Node 走 `_ => Err` 兜底）

**compile.rs**：
- [ ] 4 个 Descriptor 分支（IfThenElseDescriptor / SwitchDescriptor / SelectDescriptor / FocusedSeqDescriptor）
- [ ] 4 个 `build_*_node` 辅助函数（参照 `build_stop_if_node` 模式）
- [ ] Switch 编译期分类逻辑（ConstInt / IntExpr / FieldRef / 复杂表达式拒绝）

**Python 用户面**：
- [ ] `If` macro：`def If(cond, sub): return IfThenElse(cond, sub, Pass)`
- [ ] 4 个 Descriptor Python 类（IfThenElseDescriptor / SwitchDescriptor / SelectDescriptor / FocusedSeqDescriptor）
- [ ] `construct.__init__` 导出 If / IfThenElse / Switch / Select / FocusedSeq 名字

### 10.3 质量门禁（每个 Node 实施完成后）

- [ ] `cargo build` 通过
- [ ] `cargo clippy` 零 warning
- [ ] `cargo fmt --check` 通过
- [ ] `cargo test` 全 PASS（含新单元测试 + 既有回归）
- [ ] parity 测试：对应 Python construct 用法的行为对齐
- [ ] 性能测试：§8.2 预测加速比达标（≥4x 必需，≥10x 理想）

---

## 11. parity 测试模板

### 11.1 IfThenElse parity

```python
from construct import IfThenElse, If, Struct, Int8ub, Int16ub, Pass

# 基础 parity
d = IfThenElse(this.x > 0, Int8ub, Int16ub)
assert d.parse(b"\xff", x=1) == 255         # then 分支
assert d.parse(b"\xff\x01", x=0) == 0xff01  # else 分支（Int16ub big-endian）
assert d.build(255, x=1) == b"\xff"
assert d.build(255, x=0) == b"\x00\xff"

# If macro parity
d2 = If(this.x > 0, Byte)
assert d2.parse(b"\xff", x=1) == 255
assert d2.parse(b"", x=0) is None           # Pass.parse 返回 None
assert d2.build(255, x=1) == b"\xff"
assert d2.build(255, x=0) == b""            # Pass.build 不写字节
```

### 11.2 Switch parity

```python
from construct import Switch, Int8ub, Int16ub, Int32ub, Pass

# 路线 A：int key
d = Switch(this.n, {1: Int8ub, 2: Int16ub, 4: Int32ub})
assert d.parse(b"\x05", n=1) == 5
assert d.parse(b"\x00\x05", n=2) == 5
assert d.parse(b"\x00\x00\x00\x05", n=4) == 5
assert d.parse(b"", n=99) is None           # default=Pass
assert d.build(5, n=1) == b"\x05"
assert d.build(5, n=4) == b"\x00\x00\x00\x05"

# 路线 B：str key
d2 = Switch(this.tag, {"int": Int8ub, "long": Int16ub})
# 注：tag 字段需在外层 Struct 中先 parse
# FocusedSeq 或 Struct 场景验证

# default 非 Pass
d3 = Switch(this.n, {}, default=Byte)
assert d3.parse(b"\x01", n=255) == 1
assert d3.build(1, n=255) == b"\x01"
```

### 11.3 Select parity

```python
from construct import Select, Int32ub, CString, Pass, Optional

# 基础 parity
d = Select(Int32ub, CString("utf8"))
assert d.parse(b"\x00\x00\x00\x01") == 1
assert d.parse(b"hello\x00") == "hello"
assert d.build(1) == b"\x00\x00\x00\x01"
assert d.build(u"Афон") == b"\xd0\x90\xd1\x84\xd0\xbe\xd0\xbd\x00"

# 全部失败 → SelectError
try:
    d.parse(b"")  # Int32ub 需要 4 字节，CString 需要 null 终止
    assert False, "should raise SelectError"
except SelectError:
    pass

# Optional macro = Select(subcon, Pass)
d2 = Optional(Int64ul)
assert d2.parse(b"12345678") == 4050765991979987505
assert d2.parse(b"") is None
assert d2.build(1) == b"\x01\x00\x00\x00\x00\x00\x00\x00"
assert d2.build(None) == b""
```

### 11.4 FocusedSeq parity

```python
from construct import FocusedSeq, Const, Byte, Padding, Terminated

# 基础 parity（Python doc 示例）
d = FocusedSeq("num", Const(b"SIG"), "num"/Byte, Terminated)
assert d.parse(b"SIG\xff") == 255
assert d.build(255) == b"SIG\xff"

# 字段间引用
d2 = FocusedSeq("count",
    "count" / Byte,
    "data" / Padding(lambda this: this.count - 1),  # count - sizeof(count_field)
)
assert d2.build(4) == b"\x04\x00\x00\x00"
```

### 11.5 ExplicitError parity（差异文档化）

```python
# Phase 7 已知差异（DEV §11-D1）：
# Python 用户在 Adapter 回调中抛 ExplicitError 时，经 pyo3 From<PyErr> for ConstructError
# 转为 ConstructError::Generic，Select/Peek 无法识别为 Explicit，会被吞掉。
#
# Python parity 在"Rust 内部主动构造 ConstructError::Explicit"场景下成立
# （Phase 7 范围内 Rust 不主动构造 Explicit，仅集成变体供未来 Error 构造器使用）。
#
# 测试方法（单元测试，Rust 侧）：
# - 在 Select 的某 subcon 内手动返回 Err(ConstructError::Explicit {...})
# - 验证 Select.parse 立即返回该 Err（不 seek 回退，不尝试后续）
```

---

## 12. 已知差异汇总（DEV 文档化责任）

| # | 差异 | 原因 | 处理 |
|---|------|------|------|
| D1 | Python 用户 Adapter 回调抛 ExplicitError → Rust 侧 `From<PyErr>` 转 Generic → Select/Peek 吞掉 | `From<PyErr> for ConstructError`（error.rs L942）统一转 Generic，丢失类型信息 | Phase 7 不修复（影响小）；文档化；未来 Phase 8+ 修复 `From<PyErr>` 保留 Python 异常类映射 |
| D2 | `Switch default=Error` 不支持 | Python `Error` 构造器 core.py 未定义类，Phase 7 不实现 | 文档化；default 仅接 Pass 或已实现 Node |
| D3 | `Switch keyfunc = this.x + 1`（复杂表达式）编译期拒绝 | PM 决策 1：拒绝 PyCallable，引导用户用 Computed 预计算 | 编译期 `CompilationError` + 错误消息引导 |
| D4 | FocusedSeq sizeof 不做 context nesting | sizeof 接口无 py token，无法 new_child | 字段大小依赖嵌套 ctx 时返回 Err（对齐 Python SizeofError） |
| D5 | Pointer `stream` 参数（换流）不支持 | （Phase 7.2 范围，本设计不涉及） | 7.2 文档化 |
| D6 | Select build 在 temp BuildStream 上尝试（无 sub-stream seek） | construct-rs BuildStream 是 Vec<u8>，temp 失败即丢弃 | 与 Python temp BytesIO 行为等价 |

---

## 13. PM 决策点

本设计在 PM 已下达的 2 项决策基础上展开，**无新增 PM 决策点**。

| PM 决策 | 应用位置 | 状态 |
|---------|---------|------|
| 决策 1（Switch keyfunc = A+B 混合，拒绝 PyCallable） | §3 全节 | 已应用 |
| 决策 2（引入 `ConstructError::Explicit` 变体） | §1 全节 | 已应用 |

**ARCH 待确认事项**（非决策，仅需 PM 知会）：

1. **FocusedSeq 实施优先级**：分析报告 §B.1.4 标注 P2（用户面频率低，PrefixedArray 已独立实现）。
   本设计将 FocusedSeq 纳入 7.1 范围（与 PM 任务书一致）。DEV 实施时若时间紧张，可考虑
   拆为 7.1a（IfThenElse/Switch/Select + ExplicitError）+ 7.1b（FocusedSeq），由 PM 决定。
2. **`eval_condition` 重构**：从 `StopIfNode::eval_cond`（私有方法）提取为 `pub(crate)` 自由函数。
   此为内部重构，不影响公开 API。`stop_if.rs` 既有测试应全 PASS（行为不变）。PM 知会即可。
3. **Switch sizeof 的 `match_sub` 拆分**：设计推荐拆为 `match_sub_int`（无 py）和
   `match_sub_py`（需 py），避免 sizeof 路径无谓获取 GIL。DEV 实施时选择更清晰方案即可。

---

> **设计完成时间**：2026-07-30
> **下一步**：PM 安排 REV 设计检视（DESIGN_REVIEW 阶段），重点检查 §0 对照表 + Switch A+B 方案 + FocusedSeq 不复用 StructNode 的决策。



