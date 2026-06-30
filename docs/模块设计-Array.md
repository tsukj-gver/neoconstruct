# 模块设计：Array 支持（Phase 4）

> **设计依据**：
> - `AGENTS.md` §0（核心原则：一次 FFI、无中间表示层、直接操作 Python 对象）、
>   §7（技术决策）、§8（编码红线）、§10（Python 参考速查）
> - `docs/设计决策记录.md`（跨阶段约束，含 Phase 4 决策 3 "Index 仅作为构造器字段"）
> - `plans/phase4-array/分析报告-Array功能集.md`（功能集分析）
> - `plans/phase4-array/总纲.md`（出口标准：S-FUNC、S-QUAL、S-PERF ≥10x）
> - `construct-rs/src/nodes/mod.rs`（现有 Node enum + Construct trait）
> - `construct-rs/src/stream.rs`（现有 ParseStream/BuildStream）
> - `construct-rs/src/context.rs`（现有 Context）
> - `construct-rs/src/expr.rs`（现有 ExprOp / ExprProgram）
> - `construct-rs/src/compile.rs`（现有编译管线）
> - `construct-rs/src/error.rs`（现有错误变体）
> - `construct-rs/python/construct/_mixin.py`（`_compile_expr_tree` 表达式编译管线）
> - Python 源码：`construct/construct/core.py`
>   （Array L2493、GreedyRange L2570、RepeatUntil L2637、Index L2934、
>   StopIf L4079、PrefixedArray L4934）、`construct/construct/lib/containers.py`（ListContainer）
>
> **角色**：ARCH
> **状态**：DESIGNING（v4，VET 驳回 V-1 后第四次修订，等待重新检视）
> **创建时间**：2026-06-29
> **修订时间**：2026-06-29（v2 修正 REV 驳回的 P1/P2/P3 + M1-M4；
>   v3 修正 REV 决策记录对照驳回的 V1/V2/V3/V4；
>   2026-06-30 v4 修正 VET 驳回的 V-1 PyCallable context proxy 不完整）

---

## 0. REV/VET 驳回修正摘要

### 0.1 v2 修正（P1/P2/P3 + M1-M4）

REV 在 v1 检视中驳回 3 个严重问题（P1/P2/P3）+ 4 个中等问题（M1-M4）。
v2 逐项修正：

| 编号 | 问题 | 修正位置 | 修正内容 |
|------|------|---------|---------|
| P1 | `has_expressions` 对 `StopIf(Expr)` 返回 false | §6.1.1 | 改为 `matches!(cond, Expr(_))`，附 StructRef 路径的根因说明 |
| P2 | RepeatUntil build discard 语义错 | §4.3.4、§7.3 RU-8 | `partial.append` 加 `if !self.discard` 守卫；新增 RU-8 边界 |
| P3 | Array 表达式 count 编译路径留 placeholder | §6.2.2、§12.2 | 完整编译路径（参照 BytesDescriptor）；明确 inner 表达式限制（P3.1）+ 扩展方案（P3.2） |
| M1 | GreedyRange 吞掉所有非 StopField 错误 | §9.5 GE-1 | 新增 §9.5 已知差异表，详述 ExplicitError 等价物缺失的决策与影响 |
| M2 | PrefixedArray count 溢出行为含糊 | §7.4 PA-5 | 改为"由 countfield 节点自行报错"，删除"回绕"误述 |
| M3 | sizeof 表达式 count 的 GIL 路径不明 | §4.1.4 | 明确 GIL 前置条件 + with_gil 闭包正确写法（避免借用逃逸） |
| M4 | RepeatUntil S-PERF 适用性歧义 | §8.2、§8.3（新增）、§12.1 | 明确"Array"指狭义 Array；RepeatUntil 拆 4.5a（PyCallable）+ 4.5b（Expr），4.5b 必须在 Phase 4 验收前完成 |

附加修正（连带）：
- §6.1.1 补全各 Node 的 has_expressions 实现细节（含 RepeatPredicate::Expr、PrefixedArray.countfield）
- §9.5 新增"已知行为差异表"，集中管理 LC-1~LC-4、IX-1、GE-1/GE-2、RU-build-1、PA-5、NE-expr-1/2
- §7.1 AR-10 更新（inner 表达式编译期失败）
- §11.5 / §12.7 自检与结论同步更新

### 0.2 v3 修正（V1/V2/V3/V4 —— 决策记录对照驳回）

REV 在 v2 复审通过后，对照 `docs/设计决策记录.md` 全部 12 条决策逐条审查，
发现违反 Phase 2 决策 1（废弃 `this`），驳回 2 个设计问题（V1/V2）+ 2 个文档表述
问题（V3/V4）。v3 逐项修正：

| 编号 | 问题 | 修正位置 | 修正内容 |
|------|------|---------|---------|
| V1 | `ExprOp::GetIndex` 用户面入口用 `this._index`，违反决策 1；GetIndex 实为死代码 | §3.3 整节重写、§9.4、§9.5 IX-1、§4.4、设计决策记录 Phase 4 决策 3 | **决策：删除 ExprOp::GetIndex**（不扩展表达式系统）。用户通过 `rfield(Index())` + 字段引用实现等价功能。已实现的 GetIndex 代码由 DEV 回退。详见 §3.3 |
| V2 | §9.4 表格把 `this.xxx` 列为 construct-rs 表达式系统输入 | §9.4 | 重构为三列对照（Python `this.xxx` 语法 / construct-rs 用户面语法 / Rust ExprOp），明确 construct-rs 不用 `this` |
| V3 | 25+ 处描述性引用 Python 代码时用 `this.xxx` 未标注 | §2.2、§2.3、§2.6、§3.2.2、§6.1.1、§6.2.2、§6.3.3、§7.1、§9.5、§12.2 等 | 全文清理：所有 `this.xxx` 描述性引用统一改为"Python 用户写 `this.xxx`（construct-rs 已废弃，改用 `<字段名>` 直接引用）"格式；示例性使用改为 construct-rs 合法语法 |
| V4 | §4.5.1 StopIfCondition::Expr 注释使用 `this.x == 0` | §4.5.1 | 改为 construct-rs 实际语法：`x == 0`（x 是字段名引用，编译为 `[GetInt(idx), Const(0), Eq]`） |

附加修正（连带）：
- 新增 `docs/设计决策记录.md` Phase 4 决策 3（Index 仅作为构造器字段）
- §0.2 新增 v3 修正摘要
- §11 自检章节同步更新

> **D1/D2/D3 文档小问题**（v2 复审附注）：D1（§4.1.4 sizeof 伪代码反例）、
> D2（§8.2 交叉引用笔误 "§8.5" 应为 "§8.3"）、D3（§6.2.1 调用 build_array_node
> 少传参数）仍然有效，v3 不在驳回范围，留作后续清理。

### 0.3 v4 修正（V-1 —— VET 驳回 PyCallable context proxy 不完整）

VET 在 4.5 子任务代码审查中驳回 1 个阻断性问题（V-1）+ 4 个次要问题（V-2~V-5）。
V-1 是行为破坏性变更：DEV 实现的 PyCallable 路径 context proxy 仅含 `_index`，
跳过了 ctx.fields() 字段复制，且 proxy 是 PyDict 而非 Container（不支持 attribute 访问）。
v4 逐项修正（V-2~V-5 留待 DEV 在 V-1 修复批次中附带处理，本节聚焦 V-1）：

| 编号 | 问题 | 修正位置 | 修正内容 |
|------|------|---------|---------|
| V-1 | PyCallable 路径 proxy 仅含 `_index`，谓词无法访问 ctx 字段；proxy 是 dict 不支持 attribute 访问 | §2.5、§4.3.3、§4.3.4、§7.3 RU-9、§8.3、设计决策记录 Phase 4 决策 4 | **决策：proxy 必须用 Container 包装 + 复制 ctx.fields() 全部字段 + 写 _index**（方案 A）。Container 通过 `__dict__ = self` 支持 attribute 访问，对齐 Python `predicate(e, list, context)` 中 context 是 Container 的语义。性能预期从 ≥3x 下调到 ≥1.5x（PyCallable 是兜底路径，Expr 路径 ≥10x 已达标） |

附加修正（连带）：
- §2.5 决策 A5 补充 PyCallable proxy 规范（从"DEV 决策"明确为硬性约束）
- §4.3.3 `call_repeat_predicate` + `build_context_proxy` 改为 Container 包装的硬性约束 + 实现伪代码
- §4.3.4 build PyCallable 路径同步更新
- §7.3 新增 RU-9 边界（谓词 context 访问）
- §8.1/§8.2/§8.3 更新 PyCallable 路径性能预期（≥3x → ≥1.5x）
- 新增 `docs/设计决策记录.md` Phase 4 决策 4（PyCallable proxy 用 Container 包装）
- §9.5 不新增已知差异条目（方案 A 行为对齐 Python，无差异）

> **V-2/V-3/V-4/V-5 不在本次设计修正范围**：
> - V-2（build partial 收集 elem 而非 buildret）：Adapter-as-inner-RepeatUntil 场景，
>   由 DEV 在 §9.5 新增 RU-build-2 条目文档化（与 NE-expr-1/2 同模式）。
> - V-3（非 callable 谓词）：Python 未文档化特性，由 DEV 在 §9.5 新增 RU-3b 条目文档化，
>   或在 RepeatUntilDescriptor.__init__ 兼容（DEV 决策）。
> - V-4（AST 谓词识别不支持负整数）：由 DEV 扩展 `_is_int_const` 识别 `UnaryOp(USub, Constant(int))`。
> - V-5（has_expressions 注释误导）：由 DEV 修正注释。
>
> 以上 4 项均为实现/文档层面，不需 ARCH 设计决策。

---

## 1. 概述

### 1.1 目标

为 construct-rs 引入"重复元素"构造器，支持解析与构建元素序列。覆盖 Python
construct 2.10.70 的 Array 完整功能集（不含 LazyArray，归入惰性解析阶段）。

**性能目标**：Array parse/build **≥10x** vs Python construct 2.10.70
（S-PERF 出口标准）。理想目标 ≥20x。

### 1.2 范围（按优先级）

| 优先级 | 构造器 | Python 行号 | 类型 | 本设计节点 |
|--------|--------|------------|------|-----------|
| P0 | `Array(count, subcon, discard)` | 2493 | 固定次数 | `ArrayNode` |
| P1 | `GreedyRange(subcon, discard)` | 2570 | 读到流结束 | `GreedyRangeNode` |
| P2 | `PrefixedArray(countfield, subcon)` | 4934 | 前缀长度 | `PrefixedArrayNode`（独立实现，§5） |
| P3 | `RepeatUntil(predicate, subcon, discard)` | 2637 | 谓词终止 | `RepeatUntilNode` |
| P4 | `Index` | 2934 | 取当前下标 | `IndexNode` |
| P5 | `StopIf(condfunc)` | 4079 | 早停信号 | `StopIfNode` |

> **不在本阶段范围**：
> - `LazyArray` / `LazyListContainer`（惰性解析，归入后续 Phase）
> - `Range`（2.11+ 才引入，2.10.70 不存在）
> - `Sequence`（独立 Phase，与 Struct 平行；StopIf 在 Sequence 的捕获留待 Sequence 阶段）

### 1.3 与现有架构的衔接

Phase 1-3 已实现 13 个 Node 变体。本设计**不破坏**现有行为：

- 现有 `ParseStream` / `BuildStream` 的字节级与 bit 级 API 语义不变；新增 `seek` 方法
  （§3.1，GreedyRange 硬依赖）
- 现有 13 个 Node 的 parse/build/sizeof 行为不变
- 现有 `Context` 接口不变；新增 `_index` 字段（§3.2，栈分配，零开销）
- 现有 `compile_schema` 签名向后兼容（新增可选参数）
- 现有 `ConstructError` 变体不变；新增 3 个变体（§3.4）

**新增内容**：
- `ParseStream` / `BuildStream` 新增 `seek` API（§3.1）
- `Context` 新增 `_index: Option<usize>` 字段 + 读写方法（§3.2，IndexNode 读取用）
- `ConstructError` 新增 `Range` / `Repeat` / `StopField` / `IndexField` 4 个变体（§3.4）
- Node enum 新增 6 个变体（§4）
- `compile_schema` 新增 Array 描述符识别分支（§6.2）
- Python 侧新增 Array 描述符与 `len_` 辅助函数（§6.3）

> **V1 修正（v3）**：不再扩展 ExprOp（不新增 `GetIndex`）。IndexNode 直接调
> `ctx.index()` 读下标，不走 ExprProgram。详见 §3.3。

---

## 2. 核心架构决策

### 2.1 决策 A1：Array 系列 Node 的 inner 用 `Box<Node>`（沿用 BitwiseNode 模式）

`ArrayNode` / `GreedyRangeNode` / `RepeatUntilNode` / `PrefixedArrayNode` 都持有
一棵完整的 `Node` 子树作为元素定义。Node 是递归类型——enum 内含 `Box<Node>` 变体
（如 `BitwiseNode.inner`）。Array 系列沿用此模式：

```rust
pub struct ArrayNode {
    inner: Box<Node>,      // 元素子树
    count: CountSource,    // 静态整数 / ExprProgram
    discard: bool,
}
```

**为什么不用 `Box<dyn Construct>`**：违反 AGENTS.md §7（禁止动态分派）。
enum_dispatch 静态分派是项目硬约束。

### 2.2 决策 A2：count 来源编译期分类（静态 / 表达式）

Python `Array(count, subcon)` 的 `count` 可以是：

1. 整数常量：`Array(5, Byte)`
2. 上下文 lambda（**Python 语法**）：`Array(this.length, Byte)`、`Array(lambda ctx: ctx.n, Byte)`

construct-rs 等价写法：count 是 `_FieldDescriptor`（字段名引用），如
`Array(length, Byte)`（其中 `length` 是已声明的 `_FieldDescriptor`）。

编译期把这两种来源编译为 `CountSource` 枚举：

```rust
pub enum CountSource {
    /// 编译期常量（如 `Array(5, Byte)`）。
    Const(usize),
    /// 表达式程序（如 `Array(length, Byte)`，length 是字段名引用，复用现有 ExprProgram）。
    Expr(ExprProgram),
}
```

运行时 `ArrayNode::eval_count(ctx, py)` 返回 `usize`。`Const` 路径零开销（直接返回），
`Expr` 路径走现有 `eval_expr_int`。

**为什么 `usize` 而非 `i64`**：
- Python 校验 `0 <= count`（core.py L2527），负数报 `RangeError`。
- Rust 在编译期或运行时把 i64 → usize 前先校验非负（§9.1 AR-2）。

### 2.3 决策 A3：`_index` 通过 Context 的栈分配字段传递（不走 PyDict）

Python 把 `_index` 写入 context dict（`context._index = i`），内层通过
`context.get("_index")` 或 `this._index`（**Python 语法**）读取。

construct-rs 中，`_index` 是 Context 的内部状态，不通过用户面表达式语法暴露。
用户访问下标的机制见 §3.3（通过 `rfield(Index())` + 字段引用）。

现有 Context 用 `expr_values_buf`（栈分配内联数组）加速 GetInt。但 `_index` 不是
字段，**不**应占用 `expr_values_buf` 的字段槽位（语义污染 + 编译期索引难以表达）。

**决策**：在 Context 增加 `_index: Option<usize>` 字段：

```rust
pub struct Context<'py> {
    fields: Option<Bound<'py, PyDict>>,
    parent: Option<&'py Context<'py>>,
    expr_values_buf: [*mut ffi::PyObject; MAX_INLINE_FIELDS],
    expr_values_len: usize,
    /// Phase 4 新增：当前数组迭代下标（仅 Array 系列节点设置）。
    /// `None` 表示当前不在数组迭代中（IndexNode 读到时返回 None / Py_None）。
    /// 栈分配，零堆开销。
    _index: Option<usize>,
}
```

**嵌套数组语义**（Array 内 Array）：

Python 在内层覆盖外层 `_index`。我们的设计：

- `ArrayNode.parse` 进入循环前保存 `old = ctx._index`，每次迭代设置 `ctx._index = Some(i)`，
  递归 inner 后**不**恢复（Python 也不恢复，inner 内的子 Struct 看到的是当前下标）。
- 循环结束后恢复 `ctx._index = old`（保证外层数组继续迭代时 _index 正确）。

IndexNode.parse 直接读 `ctx.index()`，None 时返回 Py_None（对齐 Python
`context.get("_index", None)` 的行为，详见 §4.4）。

> **V1 修正（v3）**：v1/v2 在此设计 `ExprOp::GetIndex` 指令读取 `ctx._index`，
> 但其用户面入口 `this._index` 违反 Phase 2 决策 1（废弃 `this`），实为死代码。
> v3 删除 GetIndex，IndexNode 直接调 `ctx.index()`。详见 §3.3 与设计决策记录
> Phase 4 决策 3。

**为什么不用 PyDict 路径**：
- `set_field("_index", v)` 每次迭代 ~50ns（dict 查找 + SetItem），100 元素 = 5μs。
- 栈字段 `ctx._index = Some(i)` 每次 ~1ns，100 元素 = 100ns。**50x 加速**。
- 与现有 Vec 化优化的精神一致（Context-Vec优化.md）。

### 2.4 决策 A4：`StopFieldError` 用 Result 哨兵变体（不引入 panic / 异常）

Python 用异常做控制流（`StopIf` 抛 `StopFieldError`，`Struct`/`Sequence`/
`GreedyRange` 捕获视为正常终止）。Rust 不能用 panic（AGENTS.md §8 红线 2），
也不能引入新返回类型（破坏 Construct trait 签名）。

**决策**：在 `ConstructError` 增加 `StopField` 哨兵变体：

```rust
pub enum ConstructError {
    // ... 现有变体 ...
    /// Phase 4：早停信号（仅 StopIfNode 产生）。
    /// StructNode / SequenceNode / GreedyRangeNode 捕获此变体视为正常终止。
    /// 其他节点若意外收到此变体，按错误向上传播（防御性）。
    #[error("stop field signal at {path}")]
    StopField {
        path: String,
    },
}
```

**捕获语义**：

- `GreedyRangeNode.parse`：每次迭代 `match inner.parse(...) { Ok(v) => push, Err(StopField{..}) => break, Err(e) => return Err(e) }`
- `StructNode.parse` / `SequenceNode.parse`（Phase 5+）：同样匹配 StopField 后停止后续字段。
- `ArrayNode.parse`：**不**捕获 StopField（Python Array 也不捕获——Array 是固定次数，StopIf 在 Array 内无意义；用户若误用，错误向上传播）。

**为什么用错误枚举变体而非 `Result<Result<T, StopSignal>, E>`**：
- 不改 Construct trait 签名（13 个现有节点无需修改）。
- 错误路径开销可接受（StopField 仅在 StopIf 触发时构造，成功路径零成本）。
- enum_dispatch 自动生成的 match 代码无需调整。

### 2.5 决策 A5：RepeatUntil 谓词的两条路径（ExprProgram / Python callable）

Python `RepeatUntil(predicate, subcon)` 的 `predicate` 是 `(obj, list, context) -> bool`
lambda，每次迭代跨 FFI 调用。

为减少 FFI，**常见模式**在编译期识别并编译为 `ExprProgram`：

| Python 谓词模式 | 编译为 ExprProgram |
|----------------|-------------------|
| `lambda x, lst, ctx: x > N` | `[GetElem(0), Const(N), Gt]` |
| `lambda x, lst, ctx: x == N` | `[GetElem(0), Const(N), Eq]` |
| `lambda x, lst, ctx: x != N` | `[GetElem(0), Const(N), Ne]` |

**复杂模式**（如 `lambda x, lst, ctx: lst[-2:] == [0, 0]`）回落到 Python callable 路径。

设计：`RepeatUntilNode` 持有 `RepeatPredicate` 枚举：

```rust
pub enum RepeatPredicate {
    /// 编译期识别的简单表达式（仅依赖当前元素）。
    /// 求值时从 ctx._index 取当前元素（暂存于 ctx 的特殊槽位，§4.3.1）。
    Expr(ExprProgram),
    /// 复杂谓词，回落到 Python callable（每次迭代跨 FFI）。
    PyCallable(Py<PyAny>),
}
```

> **第一阶段实现范围（建议 PM 与 DEV 协调）**：
> Phase 4 首次落地可仅支持 `PyCallable` 路径（功能完整、行为正确），
> `Expr` 快路径作为性能优化延后到 Phase 4 收尾或独立子任务。
> 这样 S-FUNC 先达成，S-PERF 在 Expr 路径补全后达成 ≥10x。
> 具体决策由 PM 在子任务拆分时确定。

> **v4 决策（V-1 修正，写入设计决策记录 Phase 4 决策 4）**：PyCallable 路径
> 传给谓词的 context proxy **必须**满足以下硬性约束（消除 v3 §4.3.3 "DEV 决策"
> 的模糊空间，防止再次偏离）：
>
> 1. **proxy 类型**：必须是 Python `construct.lib.containers.Container` 实例
>    （不是原生 `dict`）。理由：Python `RepeatUntil._parse/_build` 直接把 Container
>    传给谓词（core.py L2681 `predicate(e, obj, context)` / L2697），Container 通过
>    `self.__dict__ = self`（containers.py L110）支持 attribute 访问。用户写
>    `lambda x, lst, ctx: x > ctx.threshold` 是 Python construct 文档示明的核心用法。
> 2. **proxy 内容**：必须包含 `ctx.fields()` 全部字段（浅复制）+ `_index`（每次迭代更新）。
>    理由：对齐 Python context 的字段可见性。仅含 `_index` 会使谓词访问任何 Struct 字段
>    时触发 `KeyError` / `AttributeError`，是行为破坏性变更。
> 3. **Container 类缓存**：Rust 侧在模块初始化（`_construct_rust`）时从
>    `construct.lib.containers` import `Container` 类并缓存为 `Py<PyType>`
>    （参照 `error.rs::init_exception_classes` 的 `ExceptionClasses` 模式）。
>    每次 proxy 构造时 `Container.call1((dict,))` 包装 dict。
> 4. **性能预期下调**：PyCallable 路径 S-PERF 目标从 ≥3x 下调到 **≥1.5x**
>    （§8.3 更新）。理由：Container 实例化 + 字段复制增加 ~400-700ns/iter 开销。
>    PyCallable 是兜底路径（Expr 路径 ≥10x 已达标），性能下降换取行为正确性。
>
> 详见 §4.3.3 / §4.3.4 实现伪代码。

### 2.6 决策 A6：PrefixedArray 用独立 Node（不依赖未实现的 FocusedSeq/Rebuild）

Python `PrefixedArray` 是宏（**Python 语法**）：`FocusedSeq("items", Rebuild(countfield, len_(this.items)), subcon[this.count])`。
其中 `this.items` / `this.count` 是 Python construct 的 `this` 引用语法，construct-rs 已废弃（见 Phase 2 决策 1）。

`FocusedSeq` / `Rebuild` 尚未实现（属于"字段引用回写"特性，Phase 5+ 范围）。
若等 FocusedSeq 实现再做 PrefixedArray，会阻塞 Phase 4 出口。

**决策**：实现独立的 `PrefixedArrayNode`，直接组合 `countfield` 与 `subcon`：

```rust
pub struct PrefixedArrayNode {
    /// 前缀字段（如 VarInt、Byte）。
    countfield: Box<Node>,
    /// 元素子树。
    inner: Box<Node>,
}
```

parse：`countfield.parse(stream)` → 得 count → `Array(count, inner).parse` 内联。
build：取 `len(list)` → `countfield.build(len)` → 遍历 list 调 `inner.build`。

**为什么内联而非构造临时 ArrayNode**：避免堆分配 ArrayNode 实例。直接在
PrefixedArrayNode::parse 内联循环，逻辑等价于 ArrayNode 但消除一层间接。

### 2.7 决策 A7：ListContainer 直接返回原生 `list`（不引入新 Python 类型）

Python `ListContainer` 是 `list` 子类，仅添加 `repr`/`str` 美化与 `search`/`search_all`。
`==` 与普通 list 完全一致。

**决策**：construct-rs 直接返回原生 `list`（`PyList`）。

理由：
1. 引入 `ListContainer` Python 子类需要 Rust 侧 pyclass + Python 侧类定义 + 类型注册，
   增加复杂度。
2. `repr` 美化是非必要功能（用户可用 `pprint` 或自定义）。
3. `search`/`search_all` 是工具方法，可后续作为独立函数提供（非类型绑定）。
4. 性能：原生 `PyList` 创建走 C API fast path，子类实例化走慢路径。

**已知限制**：用户代码若 `isinstance(result, ListContainer)` 检查会失败。
这是 Phase 4 的已知行为差异，文档标注（§9.5 LC-1）。

### 2.8 决策 A8：`discard` 用编译期标志（运行时零开销）

Python `Array(count, subcon, discard=True)` 仍消耗流但不收集结果。

**决策**：`discard: bool` 字段直接存在 ArrayNode/GreedyRangeNode/RepeatUntilNode 中。
parse 循环内 `if !self.discard { list.append(value) }`——分支预测稳定（每次相同），
CPU 预测准确率 100%，几乎零开销。

---

## 3. 基础设施扩展

### 3.1 ParseStream / BuildStream 新增 `seek`

GreedyRange 硬依赖 seek：失败时回退到最后成功位置。

#### 3.1.1 ParseStream::seek

```rust
impl<'a> ParseStream<'a> {
    /// 设置字节游标到 `pos`，同时重置 bit 游标为 0（字节对齐）。
    ///
    /// 用于 GreedyRange 失败回退（对齐 Python `stream_seek(stream, fallback, 0, path)`，
    /// `whence=0` 绝对定位）。
    ///
    /// # 边界
    ///
    /// - `pos > data.len()`：返回 `ConstructError::Stream`（含 expected/found）。
    /// - `bit_pos != 0` 时调用：先重置 bit_pos 为 0（GreedyRange 通常字节对齐，
    ///   此分支防御性兼容）。
    ///
    /// # 参数
    ///
    /// - `pos`：目标字节位置（绝对偏移，0-based）。
    /// - `path`：错误追踪路径。
    pub fn seek(&mut self, pos: usize, path: &Path) -> Result<(), ConstructError> {
        if pos > self.data.len() {
            return Err(ConstructError::Stream {
                message: format!(
                    "stream seek out of bounds, pos={}, data_len={}",
                    pos, self.data.len()
                ),
                path: path.to_string(),
            });
        }
        self.pos = pos;
        self.bit_pos = 0;
        Ok(())
    }
}
```

> **BuildStream 不需要 seek**：build 方向是顺序写入，GreedyRange build 失败时
> 不回退（直接返回错误）。若未来需求出现，可补充。

#### 3.1.2 ParseStream::tell 已存在

现有 `tell()` 返回 `self.pos`（字节游标），GreedyRange 直接用 `stream.tell()` 记录
fallback。**无需新增**。

### 3.2 Context 新增 `_index` 字段

#### 3.2.1 字段定义

在 `Context` 结构体新增 `_index: Option<usize>` 字段。所有现有构造函数
（`new_root` / `placeholder` / `new_child` / `new_child_placeholder`）初始化为 `None`。

```rust
impl<'py> Context<'py> {
    /// 设置当前数组迭代下标。
    /// 由 ArrayNode / GreedyRangeNode / RepeatUntilNode / PrefixedArrayNode 在
    /// 每次迭代前调用。
    #[inline]
    pub fn set_index(&mut self, index: usize) {
        self._index = Some(index);
    }

    /// 清除当前数组迭代下标。
    /// 由 Array 系列节点在循环结束后调用（恢复"不在数组中"状态）。
    #[inline]
    pub fn clear_index(&mut self) {
        self._index = None;
    }

    /// 读取当前数组迭代下标。
    /// IndexNode 与 ExprOp::GetIndex 使用。`None` 表示不在数组迭代中。
    #[inline]
    pub fn index(&self) -> Option<usize> {
        self._index
    }
}
```

#### 3.2.2 嵌套数组语义（关键）

Array 内 Array（如 `Array(3, Array(2, Byte))`）：

```
外层 i=0: ctx._index = Some(0)
  内层进入：保存 old_outer = Some(0)（在 ArrayNode::parse 局部变量）
  内层 j=0: ctx._index = Some(0)  ← 覆盖外层
  内层 j=1: ctx._index = Some(1)
  内层结束：ctx._index = old_outer = Some(0)  ← 恢复
外层 i=1: ctx._index = Some(1)
  ...
```

**实现**（ArrayNode::parse 伪代码）：

```rust
let old_index = ctx.index();           // 保存外层下标
for i in 0..count {
    ctx.set_index(i);                  // 设置当前下标
    let elem = self.inner.parse(...)?;
    if !self.discard { list.append(elem); }
}
match old_index {
    Some(idx) => ctx.set_index(idx),   // 恢复外层下标
    None => ctx.clear_index(),         // 或清除
}
```

**子 Struct 的 _index 可见性**：内层 Struct 通过 `new_child` 创建子 context，
子 context **不**自动继承父的 `_index`（Python 中 `_index` 是直接写在同一个
context dict 上的，子 context 通过 `_` 看父的 _index）。

设计选择：
- **选项 A**：子 context 继承父的 `_index`（new_child 时复制）
- **选项 B**：子 context 不继承，需要时通过 `parent()` 链查找

**采用选项 A**：在 `new_child` / `new_child_placeholder` 中复制父的 `_index` 到子。
理由：
- 子 Struct 的 Index 字段（`rfield(Index())`）应直接读到外层 Array 的下标（对齐
  Python 中 `this._index` 在子 Struct 中可见的行为——**Python 语法**，construct-rs
  通过 IndexNode + ctx._index 继承实现等价语义）
- 通过 parent 链查找的开销 = O(depth)，复制开销 = O(1)
- 内层 Array 自己 set_index 会覆盖复制来的值，符合嵌套语义

```rust
pub fn new_child(parent: &'py Context<'py>, py: Python<'py>) -> PyResult<Self> {
    Ok(Self {
        fields: Some(PyDict::new_bound(py)),
        parent: Some(parent),
        expr_values_buf: [std::ptr::null_mut(); MAX_INLINE_FIELDS],
        expr_values_len: 0,
        _index: parent._index,    // 继承父的 _index
    })
}
```

> **R4 inject_fields 路径**（struct_ref.rs）也需要同步继承 _index。
> DEV 实现时检查所有 Context 构造点。

### 3.3 ExprOp 不扩展（Index 通过字段引用复用 GetInt）

> **V1 修正（v3，REV 决策记录对照驳回）**：
> v1/v2 在此节设计了 `ExprOp::GetIndex` 指令，声称支持 `this._index` 表达式。
> 但这违反 Phase 2 决策 1（废弃 `this`）——construct-rs 表达式系统输入只有
> `_FieldDescriptor` / `_ExprRef` / 常量三种节点（表达式系统 §2.3 / §3.3），
> 不存在 `this._index` 节点类型。Python 侧 `_compile_expr_tree`
> （`_mixin.py` L475-533）也没有产生 `("getindex",)` 元组的分支。
> v1/v2 设计的 GetIndex 是事实上的死代码。
>
> **v3 决策（写入设计决策记录 Phase 4 决策 3）**：**不扩展 ExprOp**。
> 用户访问数组下标的机制如下：
>
> | 用户需求 | construct-rs 写法 | 编译结果 |
> |---------|------------------|---------|
> | 取当前下标值（作为字段） | `i: int = rfield(Index())` | IndexNode.parse 读 `ctx.index()` |
> | 在表达式中引用下标 | 先声明 Index 字段，再用字段名引用：`v: bytes = field(Bytes(i + 1))` | `[GetInt(idx_of_i), Const(1), Add]` |
>
> 这与 Phase 2 "字段名即引用、废弃 this" 的精神一致——所有引用统一走
> `_FieldDescriptor`，表达式系统输入类型保持纯粹。
>
> **对已实现代码的影响**（DEV 在后续子任务中执行回退）：
> - `expr.rs`：删除 `ExprOp::GetIndex` 变体、`compute_max_stack` 中的 GetIndex 分支、
>   `eval_expr_int` 中的 GetIndex 分支、相关单元测试（约 8 个）。
> - `compile.rs`：删除 `parse_expr_ops_from_py` 中的 `"getindex"` 分支。
> - `context.rs`：**保留** `_index` 字段与 `set_index`/`clear_index`/`index` 方法
>   （IndexNode.parse 仍需通过 `ctx.index()` 读取下标）。
> - `nodes/index.rs`（4.4 子任务实现）：IndexNode 直接调 `ctx.index()`，不走 ExprProgram。
>
> **边界说明**：
> - 用户代码 `rfield(Index())` 中 IndexNode.parse 在 `ctx.index() == None` 时返回
>   `Py_None`（对齐 Python Index 类行为，详见 §4.4.2）。
> - 用户代码 `i: int = rfield(Index()); v: bytes = field(Bytes(i + 1))` 中，
>   若 IndexNode 在 `ctx.index() == None` 时返回 Py_None，则 `i + 1` 表达式求值时
>   GetInt 会因类型不匹配（None 无法 extract 为 i64）抛 `ExprType` 错误。
>   这与 Python `this._index + 1` 在非数组上下文抛 TypeError 的行为一致——
>   无需特殊处理，原有的表达式类型检查路径自然覆盖。

### 3.4 ConstructError 新增变体

#### 3.4.1 `Range`（对应 Python `RangeError`）

```rust
/// Array count 无效（负数或与给定列表长度不符）。
/// 对应 Python construct 的 `RangeError`（core.py L2528、L2541、L2543）。
#[error("range error: {message} at {path}")]
Range {
    message: String,
    path: String,
},
```

**触发场景**：
- AR-2：count 表达式求值为负数
- AR-3：build 时 `len(obj) != count`
- AR-7：PrefixedArray count 为负

#### 3.4.2 `Repeat`（对应 Python `RepeatError`）

```rust
/// RepeatUntil build 时无元素满足谓词。
/// 对应 Python construct 的 `RepeatError`（core.py L2700）。
#[error("repeat error: {message} at {path}")]
Repeat {
    message: String,
    path: String,
},
```

**触发场景**：RU-3（build 遍历完列表无元素满足谓词）。

#### 3.4.3 `StopField`（早停哨兵，§2.4）

```rust
/// 早停信号（仅 StopIfNode 产生，GreedyRangeNode / StructNode / SequenceNode 捕获）。
/// 对应 Python construct 的 `StopFieldError`（core.py L4106）。
#[error("stop field signal at {path}")]
StopField {
    path: String,
},
```

#### 3.4.4 `IndexField`（对应 Python `IndexFieldError`）

```rust
/// Index 节点读取 _index 时上下文未提供（理论上不发生——Array 都会设置）。
/// 对应 Python construct 的 `IndexFieldError`。
#[error("index field error: {message} at {path}")]
IndexField {
    message: String,
    path: String,
},
```

> **保留但当前不触发**：当前设计 IndexNode 在 _index 为 None 时返回 Py_None
> （对齐 Python `context.get("_index", None)`），不报错。此变体保留供未来
> 严格模式使用（如 `Index` 强制要求在数组内）。Phase 4 实现可不映射到此变体。

#### 3.4.5 Python 异常类映射

`error.rs` 的 `init_exception_classes` 需要从 `construct._errors` 缓存 4 个新类：

```rust
range_error: Py<PyType>,           // construct.RangeError
repeat_error: Py<PyType>,          // construct.RepeatError
stop_field_error: Py<PyType>,      // construct.StopFieldError
index_field_error: Py<PyType>,     // construct.IndexFieldError
```

`select_exception_class` 增加 4 个分支。

> **StopField 不应跨 FFI**：StopField 是内部哨兵，正常路径被 GreedyRange 等捕获，
> 不应到达 FFI 入口。若意外到达（如 StopIf 在 Array 内），映射到 `stop_field_error`
> 让 Python 用户可识别（虽然这是用户误用）。

---

## 4. 各构造器详细设计

### 4.1 ArrayNode（P0，固定次数数组）

#### 4.1.1 数据结构

```rust
/// 固定次数数组节点。
/// 对应 Python construct `Array(count, subcon, discard)`（core.py L2493）。
#[derive(Debug)]
pub struct ArrayNode {
    /// 元素子树（递归 Box）。
    inner: Box<Node>,
    /// 元素数量来源（静态 / 表达式）。
    count: CountSource,
    /// 是否丢弃解析结果（仍消耗流）。
    discard: bool,
}

#[derive(Debug, Clone)]
pub enum CountSource {
    /// 编译期常量。
    Const(usize),
    /// 表达式程序（运行时求值）。
    Expr(ExprProgram),
}

impl ArrayNode {
    pub fn new(inner: Node, count: CountSource, discard: bool) -> Self {
        Self { inner: Box::new(inner), count, discard }
    }

    /// 求值元素数量。负数（i64 < 0）返回 `Range` 错误（对齐 core.py L2527-2528）。
    fn eval_count(&self, ctx: &Context<'_>, py: Python<'_>) -> Result<usize, ConstructError> {
        let count_i64 = match &self.count {
            CountSource::Const(n) => *n as i64,
            CountSource::Expr(prog) => crate::expr::eval_expr_int(prog, ctx, py)?,
        };
        if count_i64 < 0 {
            return Err(ConstructError::Range {
                message: format!("invalid count {}", count_i64),
                path: String::new(),
            });
        }
        Ok(count_i64 as usize)
    }
}
```

#### 4.1.2 parse 流程

对齐 Python `Array._parse`（core.py L2525-2536）：

```rust
fn parse<'py>(
    &self,
    py: Python<'py>,
    stream: &mut ParseStream<'_>,
    ctx: &mut Context<'py>,
    path: &mut Path,
) -> Result<Py<PyAny>, ConstructError> {
    let count = self.eval_count(ctx, py)?;      // AR-1: 求值 count

    // 预分配 PyList（count 个 None 占位，append 时替换）。
    // PyList::new(py, count) 比 append 循环快（一次性分配）。
    let list = PyList::new_bound(py, Vec::<Py<PyAny>>::with_capacity(count));

    let old_index = ctx.index();                // 保存外层 _index（嵌套数组）

    for i in 0..count {
        ctx.set_index(i);                       // AR-4: 设置 _index
        path.push_index(i);                     // 错误路径追踪
        let elem = match self.inner.parse(py, stream, ctx, path) {
            Ok(v) => v,
            Err(e) => {
                path.pop();
                // 恢复 _index（即使出错也要恢复，避免污染外层）
                match old_index { Some(idx) => ctx.set_index(idx), None => ctx.clear_index() }
                return Err(e);                  // AR-5: 错误向上传播
            }
        };
        path.pop();
        if !self.discard {
            list.append(elem).map_err(ConstructError::from)?;
        } else {
            drop(elem);
        }
    }

    match old_index { Some(idx) => ctx.set_index(idx), None => ctx.clear_index() }

    Ok(list.into_any())
}
```

**性能要点**：
- `PyList::new_bound(py, Vec::with_capacity(count))`：一次性预分配，避免 append 时
  多次 realloc（C API fast path）。
- `path.push_index/pop` 仅在错误路径有开销？**否**——当前设计每次迭代都 push/pop，
  开销 ~10ns × N。若性能不达标，可改为"P0-3 优化"风格（成功路径不 push，错误时
  push_path_segment）。**初始实现保留 push/pop 简化逻辑**，性能优化延后。

#### 4.1.3 build 流程

对齐 Python `Array._build`（core.py L2538-2551）：

```rust
fn build(
    &self,
    py: Python<'_>,
    obj: &Bound<'_, PyAny>,
    stream: &mut BuildStream,
    ctx: &mut Context<'_>,
    path: &mut Path,
) -> Result<(), ConstructError> {
    let count = self.eval_count(ctx, py)?;

    // 校验 obj 是 list/tuple，并取长度。
    let obj_len = if let Ok(list) = obj.downcast::<PyList>() {
        list.len()
    } else if let Ok(tuple) = obj.downcast::<PyTuple>() {
        tuple.len()
    } else {
        // 兜底：尝试 len(obj)（对任意可迭代对象）。
        // 性能差，仅在用户传入非 list/tuple 时触发。
        obj.len().map_err(|_| ConstructError::Generic {
            message: format!("Array build expects list/tuple, got {}", 
                obj.get_type().name().map(|n| n.to_string()).unwrap_or_default()),
            path: path.to_string(),
        })?
    };

    // AR-3: 长度校验
    if obj_len != count {
        return Err(ConstructError::Range {
            message: format!("expected {} elements, found {}", count, obj_len),
            path: path.to_string(),
        });
    }

    let old_index = ctx.index();
    let items: Vec<Py<PyAny>> = if let Ok(list) = obj.downcast::<PyList>() {
        list.iter().map(|b| b.unbind()).collect()
    } else if let Ok(tuple) = obj.downcast::<PyTuple>() {
        tuple.iter().map(|b| b.unbind()).collect()
    } else {
        // 兜底路径：转 list 后取（性能差）
        PyList::new_bound(py, obj.try_iter().map_err(|e| ConstructError::Generic {
            message: format!("not iterable: {}", e),
            path: path.to_string(),
        })?).into_any().downcast::<PyList>().unwrap().iter().map(|b| b.unbind()).collect()
    };

    for (i, elem) in items.into_iter().enumerate() {
        ctx.set_index(i);
        path.push_index(i);
        let elem_bound = elem.bind(py);
        if let Err(e) = self.inner.build(py, elem_bound, stream, ctx, path) {
            path.pop();
            match old_index { Some(idx) => ctx.set_index(idx), None => ctx.clear_index() }
            return Err(e);
        }
        path.pop();
    }

    match old_index { Some(idx) => ctx.set_index(idx), None => ctx.clear_index() }
    Ok(())
}
```

> **简化机会（DEV 决策）**：build 时可不预先 collect 到 Vec，直接遍历 list/tuple
> 迭代器。当前伪代码 collect 是为了同时支持 list/tuple/iterable 的统一处理。
> 若性能分析显示 collect 有显著开销，可拆分 list/tuple/iterable 三条分支。

#### 4.1.4 sizeof 流程

对齐 Python `Array._sizeof`（core.py L2553-2558）：

```rust
fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
    let count = match &self.count {
        CountSource::Const(n) => *n,
        CountSource::Expr(prog) => {
            // 表达式 count：尝试求值，失败（如字段缺失）返回 Err（对齐 SizeofError）。
            crate::expr::eval_expr_int(prog, ctx, Python::with_gil(|py| py))? as usize
        }
    };
    let elem_size = self.inner.sizeof(ctx)?;
    count.checked_mul(elem_size).ok_or_else(|| ConstructError::Generic {
        message: format!("array size overflow: {} * {}", count, elem_size),
        path: String::new(),
    })
}
```

> **GIL 前置条件（M3 修正，明确化）**：sizeof 接收 `&Context` 而非 `py: Python`
> （现有 Construct trait 签名）。若 count 是表达式，需获取 GIL 调 `eval_expr_int`。
>
> **契约明确**：
> 1. **sizeof 必须在已持有 GIL 的线程调用**。当前所有 sizeof 调用点都在
>    parse/build 内部（FFI 入口已持 GIL），此前置条件自然满足。Rust 侧不
>    显式断言 GIL（pyo3 内部会 panic，但实际不触发）。
> 2. **`Python::with_gil` 在已持有 GIL 时是 O(1)**（仅获取 token，不阻塞），
>    可接受。但需注意闭包返回值的生命周期——`Python::with_gil(|py| py)` 返回的
>    `Python<'_>` 借用闭包内的 GIL scope，**不能跨闭包边界使用**。正确写法是把
>    所有需要 `py` 的逻辑放进闭包内：
>    ```rust
>    let n = Python::with_gil(|py| -> Result<usize, ConstructError> {
>        let v = crate::expr::eval_expr_int(prog, ctx, py)?;
>        if v < 0 {
>            return Err(ConstructError::Range {
>                message: format!("sizeof array count {} is negative", v),
>                path: String::new(),
>            });
>        }
>        Ok(v as usize)
>    })?;
>    ```
>    DEV 实现时严格按此模式，避免借用逃逸。
> 3. **sizeof 不在热路径**（Python 用户极少对 Array 调 sizeof），with_gil 开销可接受。
>
> **替代方案（不采用）**：修改 Construct trait 的 sizeof 签名增加 `py: Python<'_>`
> 参数。优点是消除 with_gil；缺点是影响全部 13 个现有节点 + IndexNode/StopIfNode 的
> sizeof 签名，改动面大。PM 决策点 6 维持选项 A（with_gil）。

### 4.2 GreedyRangeNode（P1，读到流结束）

#### 4.2.1 数据结构

```rust
/// 读到流结束的数组节点。
/// 对应 Python construct `GreedyRange(subcon, discard)`（core.py L2570）。
#[derive(Debug)]
pub struct GreedyRangeNode {
    inner: Box<Node>,
    discard: bool,
}
```

#### 4.2.2 parse 流程

对齐 Python `GreedyRange._parse`（core.py L2599-2615）：

```rust
fn parse<'py>(...) -> Result<Py<PyAny>, ConstructError> {
    let list = PyList::new_bound(py, Vec::<Py<PyAny>>::new());  // 无法预估容量，动态增长
    let old_index = ctx.index();
    let mut i: usize = 0;

    loop {
        // 记录 fallback 位置（用于子构造器失败时回退）。
        let fallback = stream.tell();

        ctx.set_index(i);
        path.push_index(i);

        match self.inner.parse(py, stream, ctx, path) {
            Ok(elem) => {
                path.pop();
                if !self.discard {
                    list.append(elem).map_err(ConstructError::from)?;
                }
                i += 1;
                // 继续下一次迭代
            }
            Err(ConstructError::StopField { .. }) => {
                // StopIf 触发：正常终止（对齐 Python StopFieldError 捕获）。
                path.pop();
                stream.seek(fallback, path)?;   // 回退到 fallback（StopIf 不消耗字节）
                break;
            }
            Err(e) => {
                path.pop();
                // 区分错误类型（对齐 Python L2609-2614）：
                // - Stream 错误（EOF / 字节不足）：seek 回退，正常终止
                // - 其他错误（FormatField 类型错、表达式错等）：seek 回退，正常终止
                //   （Python 用 `except Exception` 捕获所有非 ExplicitError）
                // - 注意：ExplicitError 在 Python 中向上传播。
                //   construct-rs 没有 ExplicitError 等价物（无独立变体），
                //   所有错误都走"seek 回退 + 正常终止"路径（保守对齐）。
                //   未来若引入 ExplicitError 等价变体，再分流。
                let _ = stream.seek(fallback, path);   // 回退，忽略 seek 错误（已是要终止）
                break;
            }
        }
    }

    match old_index { Some(idx) => ctx.set_index(idx), None => ctx.clear_index() }
    Ok(list.into_any())
}
```

**关键差异 vs Python**：
- Python 用 `itertools.count()` 无限循环 + 异常退出。Rust 用 `loop` + `break`。
- Python `except StopFieldError` 与 `except Exception` 分流。Rust 用 `match` 错误变体。
  当前设计：StopField 单独处理，其他错误统一"回退 + 终止"。这与 Python 等价
  （Python 的 ExplicitError 在 construct-rs 中暂无对应）。

#### 4.2.3 build 流程

对齐 Python `GreedyRange._build`（core.py L2617-2628）：

```rust
fn build(...) -> Result<(), ConstructError> {
    // obj 必须是可迭代对象（list/tuple/任意 iterable）。
    let items: Vec<Py<PyAny>> = collect_iterable(py, obj, path)?;

    let old_index = ctx.index();
    for (i, elem) in items.into_iter().enumerate() {
        ctx.set_index(i);
        path.push_index(i);
        let elem_bound = elem.bind(py);
        match self.inner.build(py, elem_bound, stream, ctx, path) {
            Ok(()) => { path.pop(); }
            Err(ConstructError::StopField { .. }) => {
                // StopIf 触发：停止后续元素构建（对齐 Python L2627-2628）。
                path.pop();
                break;
            }
            Err(e) => {
                path.pop();
                match old_index { Some(idx) => ctx.set_index(idx), None => ctx.clear_index() }
                return Err(e);
            }
        }
    }
    match old_index { Some(idx) => ctx.set_index(idx), None => ctx.clear_index() }
    Ok(())
}
```

#### 4.2.4 sizeof

永远 Err（对齐 Python `GreedyRange._sizeof` L2630-2631）：

```rust
fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
    Err(ConstructError::Generic {
        message: "GreedyRange size is undefined".to_string(),
        path: String::new(),
    })
}
```

> **替代**：可新增 `ConstructError::Sizeof` 专门变体映射 Python `SizeofError`。
> 当前 `Generic` 已足够（错误消息明确）。PM 决策是否细化。

### 4.3 RepeatUntilNode（P3，谓词终止）

#### 4.3.1 数据结构

```rust
/// 谓词终止数组节点。
/// 对应 Python construct `RepeatUntil(predicate, subcon, discard)`（core.py L2637）。
#[derive(Debug)]
pub struct RepeatUntilNode {
    inner: Box<Node>,
    predicate: RepeatPredicate,
    discard: bool,
}

#[derive(Debug)]
pub enum RepeatPredicate {
    /// 编译期识别的简单表达式（仅依赖当前元素值）。
    /// 求值时通过特殊 ExprOp 读取"当前元素"（暂存在 ctx 的特殊槽位）。
    Expr(ExprProgram),
    /// 回落到 Python callable（复杂谓词）。
    PyCallable(Py<PyAny>),
}
```

#### 4.3.2 当前元素暂存（Expr 路径）

`Expr` 谓词需要访问"刚解析的元素值"。设计：在 Context 新增临时槽位：

```rust
pub struct Context<'py> {
    // ... 现有字段 ...
    _index: Option<usize>,
    /// Phase 4：RepeatUntil 当前元素暂存（仅 Expr 谓词路径使用）。
    /// PyCallable 路径不通过 ctx，直接在 Rust 层调 Python callable。
    _current_elem: Option<*mut ffi::PyObject>,  // borrowed ptr，不 incref
}
```

> **简化方案（推荐 PM 与 DEV 协调）**：Phase 4 首次实现**仅支持 PyCallable 路径**，
> 不引入 `_current_elem` 槽位与 `Expr` 谓词。理由：
> - PyCallable 路径功能完整、行为与 Python 完全一致。
> - Expr 路径是性能优化，可在 S-FUNC 达成后单独子任务实现。
> - 避免引入"当前元素 borrowed ptr"的生命周期复杂度。

**下文按"仅 PyCallable"路径描述**。Expr 路径的设计点保留在 §10 性能假设中作为后续优化方向。

#### 4.3.3 parse 流程（PyCallable 路径）

对齐 Python `RepeatUntil._parse`（core.py L2670-2682）：

```rust
fn parse<'py>(...) -> Result<Py<PyAny>, ConstructError> {
    let list = PyList::new_bound(py, Vec::<Py<PyAny>>::new());
    let old_index = ctx.index();
    let predicate = match &self.predicate {
        RepeatPredicate::PyCallable(p) => p.bind(py),
        RepeatPredicate::Expr(_) => unreachable!("Expr 路径未实现（§4.3.2 简化）"),
    };
    // v4：缓存 Container 类（模块初始化时已缓存，此处取用，零开销）
    let container_cls = crate::container_cache::container_class(py)?;
    let mut i: usize = 0;

    loop {
        ctx.set_index(i);
        path.push_index(i);

        let elem = match self.inner.parse(py, stream, ctx, path) {
            Ok(v) => v,
            Err(e) => {
                path.pop();
                match old_index { Some(idx) => ctx.set_index(idx), None => ctx.clear_index() }
                return Err(e);   // RU-1: 失败直接上抛（不像 GreedyRange 回退）
            }
        };
        path.pop();

        if !self.discard {
            list.append(elem.clone_ref(py)).map_err(ConstructError::from)?;
        }

        // v4：构造 Container proxy（含 ctx.fields() 字段 + _index），调用谓词。
        // 对齐 Python `predicate(e, obj, context)` 中 context 是 Container。
        let ctx_proxy = build_context_proxy(py, &container_cls, ctx)?;
        let stop = call_repeat_predicate(py, predicate, elem.bind(py), list.as_any(), &ctx_proxy)?;
        if stop {
            break;      // RU-2: 谓词为真时终止（最后元素被包含）
        }

        i = i.saturating_add(1);
    }

    match old_index { Some(idx) => ctx.set_index(idx), None => ctx.clear_index() }
    Ok(list.into_any())
}
```

**call_repeat_predicate 辅助函数**：

```rust
/// 调用 RepeatUntil 谓词，返回是否应终止。
/// 对齐 Python `predicate(obj, list, context)`。
///
/// # v4 决策（V-1 修正）
///
/// context proxy **必须**是 `construct.lib.containers.Container` 实例（不是 dict），
/// 包含 `ctx.fields()` 全部字段（浅复制）+ `_index`。理由：
/// - Python `RepeatUntil._parse/_build` 把 Container 直接传给谓词（core.py L2681/L2697）；
/// - Container 通过 `self.__dict__ = self`（containers.py L110）支持 attribute 访问；
/// - 用户 `lambda x, lst, ctx: x > ctx.threshold` 是文档示明的核心用法，必须可用。
///
/// Container 类在模块初始化时缓存为 `Py<PyType>`（参照 init_exception_classes）。
fn call_repeat_predicate(
    py: Python<'_>,
    predicate: &Bound<'_, PyAny>,
    elem: &Bound<'_, PyAny>,
    list: &Bound<'_, PyAny>,
    ctx_proxy: &Bound<'_, PyAny>,   // Container 实例（由 build_context_proxy 构造）
) -> Result<bool, ConstructError> {
    let result = predicate.call1((elem, list, ctx_proxy))?;
    let truthy: bool = result.is_truthy()?;
    Ok(truthy)
}
```

> **`build_context_proxy` 设计（v4 硬性约束，原 "DEV 决策" 删除）**：
>
> 每次迭代构造 Container proxy，包含 `ctx.fields()` 全部字段（浅复制）+ `_index`。
>
> ```rust
> /// 构造 Container proxy（每次迭代调用）。
> ///
> /// v4 决策（V-1）：proxy 必须是 Container 实例，对齐 Python 谓词收到的 context。
> fn build_context_proxy<'py>(
>     py: Python<'py>,
>     container_cls: &Bound<'py, PyType>,  // 缓存的 Container 类
>     ctx: &Context<'_>,
> ) -> PyResult<Bound<'py, PyAny>> {
>     // 1. 构造临时 PyDict，复制 ctx.fields() + 写 _index。
>     let dict = PyDict::new_bound(py);
>     if let Some(fields) = ctx.fields() {
>         for item in fields.iter() {
>             let (key, value) = item;
>             dict.set_item(&key, &value)?;
>         }
>     }
>     match ctx.index() {
>         Some(i) => { dict.set_item("_index", i.into_py(py).bind(py))?; }
>         None => { dict.set_item("_index", py.None().bind(py))?; }
>     }
>     // 2. 用 Container 包装 dict（Container(__init__) 接受 dict，自动 __dict__ = self）。
>     container_cls.call1((dict,)).map(|obj| obj.into_any())
> }
> ```
>
> **性能开销估算**（每迭代）：
> - PyDict::new_bound + 字段复制：~200-400ns（字段数 N）
> - Container 实例化（`Container.__init__` 调用）：~200-500ns
> - 合计：~400-900ns/iter
> - 加上原有 PyCallable FFI 调用（~175-225ns/iter），总开销 ~575-1125ns/iter
> - Python 原版每元素 ~270-300ns/iter，预期加速比 ~1.5-2.5x（PyCallable 路径）。
>
> **优化方向（可选，非 Phase 4 必需）**：在节点入口预创建一个长生命周期 Container，
> 每次迭代 `Container.clear()` + 字段更新 + `_index` 设置。但 Container.clear() +
> 逐字段 set_item 的开销可能与新建 Container 相当，且增加代码复杂度。首版用
> "每次新建 Container" 简单方案，性能不达标再优化。
>
> **为什么不用原生 dict**：dict 不支持 `ctx.threshold`（attribute 访问），仅支持
> `ctx['threshold']`（item 访问）。Python Container 两者都支持。即使文档说明
> "仅支持 item 访问"，用户的 `ctx.threshold` 写法会静默失败——属于"隐性破坏"，
> 不可接受。详见 §0.3 v4 修正 V-1 决策依据。

#### 4.3.4 build 流程

对齐 Python `RepeatUntil._build`（core.py L2684-2701）：

```rust
fn build(...) -> Result<(), ConstructError> {
    let items: Vec<Py<PyAny>> = collect_iterable(py, obj, path)?;
    let old_index = ctx.index();
    let predicate = match &self.predicate {
        RepeatPredicate::PyCallable(p) => p.bind(py),
        RepeatPredicate::Expr(_) => unreachable!(),
    };
    // v4：缓存 Container 类（模块初始化时已缓存，此处取用，零开销）
    let container_cls = crate::container_cache::container_class(py)?;

    let mut matched = false;
    let mut partial = PyList::new_bound(py, Vec::<Py<PyAny>>::new());

    for (i, elem) in items.into_iter().enumerate() {
        ctx.set_index(i);
        path.push_index(i);
        let elem_bound = elem.bind(py);
        if let Err(e) = self.inner.build(py, elem_bound, stream, ctx, path) {
            path.pop();
            match old_index { Some(idx) => ctx.set_index(idx), None => ctx.clear_index() }
            return Err(e);
        }
        path.pop();

        // P2 修正：对齐 Python L2694-2696 的 discard 语义。
        // Python 源码：
        //   if not discard:
        //       retlist.append(buildret)
        //       partiallist.append(buildret)   ← partiallist 仅在非 discard 时 append
        //   if predicate(e, partiallist, context):
        //       break
        // 关键：discard=True 时 partiallist 始终为空，谓词收到空 list。
        // 对依赖 list 内容的谓词（如 lambda x,lst,c: lst[-2:] == [0,0]），
        // discard=True 与 discard=False 会产生不同的终止时机——这是 Python 的语义。
        if !self.discard {
            partial.append(elem.clone_ref(py)).map_err(ConstructError::from)?;
        }

        // v4：构造 Container proxy（含 ctx.fields() 字段 + _index），调用谓词。
        let ctx_proxy = build_context_proxy(py, &container_cls, ctx)?;
        let stop = call_repeat_predicate(py, predicate, elem_bound, partial.as_any(), &ctx_proxy)?;
        if stop {
            matched = true;
            break;
        }
    }

    match old_index { Some(idx) => ctx.set_index(idx), None => ctx.clear_index() }

    if !matched {
        // RU-3: 无元素满足谓词
        return Err(ConstructError::Repeat {
            message: "expected any item to match predicate, when building".to_string(),
            path: path.to_string(),
        });
    }
    Ok(())
}
```

#### 4.3.5 sizeof

永远 Err（对齐 Python L2703-2704）。

### 4.4 IndexNode（P4，取当前下标）

> **V1 修正（v3）**：IndexNode 是 construct-rs 中**唯一**的用户面下标访问机制。
> v1/v2 设计了 `ExprOp::GetIndex` 作为表达式内引用下标的入口，但用户面写法
> `this._index` 违反 Phase 2 决策 1，已被删除（§3.3）。
> 用户要在表达式中引用下标，先用 Index 字段声明再用字段名引用：
>
> ```python
> @dataclass
> class Inner(StructMixin):
>     i: int = rfield(Index())               # IndexNode，parse 得到当前下标
>     v: bytes = field(Bytes(i + 1))         # 字段名引用 i，编译为 [GetInt(0), Const(1), Add]
> ```

#### 4.4.1 数据结构

```rust
/// 取当前数组迭代下标的节点。
/// 对应 Python construct `Index`（core.py L2934）。
///
/// construct-rs 中 IndexNode 是用户访问数组下标的唯一机制（v3 决策，§3.3）。
/// 直接调 `ctx.index()` 读取，不走 ExprProgram。
#[derive(Debug)]
pub struct IndexNode;
```

#### 4.4.2 parse / build / sizeof

```rust
impl Construct for IndexNode {
    fn parse<'py>(&self, py: Python<'py>, _stream, ctx, _path) -> Result<Py<PyAny>, ConstructError> {
        // 对齐 Python `context.get("_index", None)`（Python 语法）。
        match ctx.index() {
            Some(i) => Ok(i.into_py(py)),       // 返回 PyLong
            None => Ok(py.None()),               // 返回 Py_None
        }
    }

    fn build(&self, py: Python<'_>, _obj, _stream, ctx, _path) -> Result<(), ConstructError> {
        // 对齐 Python `Index._build`：返回 _index（但 build 不写流）。
        // IndexNode 的 sizeof=0，build 是 no-op。
        let _ = ctx.index();   // 不实际使用（保持接口一致）
        let _ = py;
        Ok(())
    }

    fn sizeof(&self, _ctx) -> Result<usize, ConstructError> {
        Ok(0)   // 对齐 Python `Index._sizeof` 返回 0
    }
}
```

> **build 语义**：Python `Index._build(obj, ...)` 返回 `context._index`，但
> build 结果不影响输出（Index 不写字节）。我们的 build 是 no-op，与 Python
> 等价（Python 的返回值在 Sequence 上下文中可能有用，但 IndexNode 的 build
> 不需要返回值，因为是 sizeof=0 字段）。
>
> **在表达式中引用下标的边界**：用户写 `i: int = rfield(Index()); v: bytes = field(Bytes(i + 1))`
> 时，若 IndexNode 在 `ctx.index() == None`（不在数组中）返回 Py_None，
> `i + 1` 表达式求值时 GetInt 会因 None 无法 extract 为 i64 抛 `ExprType` 错误。
> 这与 Python `this._index + 1`（**Python 语法**）在非数组上下文抛 TypeError
> 的行为一致——无需特殊处理。

### 4.5 StopIfNode（P5，早停信号）

#### 4.5.1 数据结构

```rust
/// 早停条件节点。
/// 对应 Python construct `StopIf(condfunc)`（core.py L4079）。
#[derive(Debug)]
pub struct StopIfNode {
    /// 条件：编译期常量（bool）或表达式程序。
    cond: StopIfCondition,
}

#[derive(Debug)]
pub enum StopIfCondition {
    /// 常量 true（永远停止，主要用于调试）。
    Always,
    /// 常量 false（永远不停止，主要用于调试）。
    Never,
    /// 表达式（如 `x == 0`，其中 `x` 是字段名引用，编译为 `[GetInt(idx), Const(0), Eq]`；
    /// construct-rs 不使用 Python 的 `this.x == 0` 语法）。
    Expr(ExprProgram),
}
```

#### 4.5.2 parse / build / sizeof

```rust
impl Construct for StopIfNode {
    fn parse<'py>(&self, py: Python<'py>, _stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
        let stop = self.eval_cond(ctx, py)?;
        if stop {
            // 抛出早停哨兵（被外层 Struct/Sequence/GreedyRange 捕获）。
            Err(ConstructError::StopField { path: path.to_string() })
        } else {
            Ok(py.None())
        }
    }

    fn build(&self, py, _obj, _stream, ctx, path) -> Result<(), ConstructError> {
        let stop = self.eval_cond(ctx, py)?;
        if stop {
            Err(ConstructError::StopField { path: path.to_string() })
        } else {
            Ok(())
        }
    }

    fn sizeof(&self, _ctx) -> Result<usize, ConstructError> {
        // 对齐 Python `StopIf._sizeof`：永远 SizeofError。
        Err(ConstructError::Generic {
            message: "StopIf size is undefined".to_string(),
            path: String::new(),
        })
    }
}

impl StopIfNode {
    fn eval_cond(&self, ctx: &Context<'_>, py: Python<'_>) -> Result<bool, ConstructError> {
        match &self.cond {
            StopIfCondition::Always => Ok(true),
            StopIfCondition::Never => Ok(false),
            StopIfCondition::Expr(prog) => {
                let v = crate::expr::eval_expr_int(prog, ctx, py)?;
                Ok(v != 0)
            }
        }
    }
}
```

> **StopIf 在 Struct 中的捕获**：当前 StructNode.parse 不识别 StopField
> （StructNode 设计于 Phase 1，先于 StopField 变体）。Phase 4 需在 StructNode.parse
> 增加 StopField 捕获分支：子字段返回 StopField 时，StructNode 停止后续字段，
> 正常返回当前实例。这是**对现有 StructNode 的小修改**，§6.1 详述。
>
> **是否阻塞**：此修改是 Phase 4 的必要配套（否则 StopIf 在 Struct 内无效）。
> DEV 实现时一并修改 struct_node.rs。

### 4.6 PrefixedArrayNode（P2，前缀长度数组）

#### 4.6.1 数据结构

```rust
/// 前缀长度数组节点。
/// 对应 Python construct `PrefixedArray(countfield, subcon)`（core.py L4934）。
///
/// 不依赖 FocusedSeq/Rebuild（未实现），独立实现 parse/build。
#[derive(Debug)]
pub struct PrefixedArrayNode {
    /// 计数字段（如 VarInt、Byte、Int16ub 等）。
    countfield: Box<Node>,
    /// 元素子树。
    inner: Box<Node>,
}
```

#### 4.6.2 parse 流程

对齐 Python `PrefixedArray._emitparse`（core.py L4961-4962）：

```rust
fn parse<'py>(...) -> Result<Py<PyAny>, ConstructError> {
    // 1. 解析 countfield 得到 count。
    let count_obj = self.countfield.parse(py, stream, ctx, path)?;
    let count_i64: i64 = count_obj.bind(py).extract()
        .map_err(|_| ConstructError::Range {
            message: "PrefixedArray countfield did not produce an integer".to_string(),
            path: path.to_string(),
        })?;
    if count_i64 < 0 {
        return Err(ConstructError::Range {
            message: format!("invalid PrefixedArray count {}", count_i64),
            path: path.to_string(),
        });
    }
    let count = count_i64 as usize;

    // 2. 内联 Array 逻辑（避免构造临时 ArrayNode 实例）。
    let list = PyList::new_bound(py, Vec::<Py<PyAny>>::with_capacity(count));
    let old_index = ctx.index();

    for i in 0..count {
        ctx.set_index(i);
        path.push_index(i);
        let elem = match self.inner.parse(py, stream, ctx, path) {
            Ok(v) => v,
            Err(e) => {
                path.pop();
                match old_index { Some(idx) => ctx.set_index(idx), None => ctx.clear_index() }
                return Err(e);
            }
        };
        path.pop();
        list.append(elem).map_err(ConstructError::from)?;
    }

    match old_index { Some(idx) => ctx.set_index(idx), None => ctx.clear_index() }
    Ok(list.into_any())
}
```

#### 4.6.3 build 流程

对齐 Python `PrefixedArray._emitbuild`（core.py L4965-4966）：

```rust
fn build(...) -> Result<(), ConstructError> {
    // 1. 取 list 长度。
    let items: Vec<Py<PyAny>> = collect_iterable(py, obj, path)?;
    let count = items.len();

    // 2. 先构建 countfield（写入长度）。
    let count_py = count.into_py(py);
    self.countfield.build(py, count_py.bind(py), stream, ctx, path)?;

    // 3. 遍历构建元素。
    let old_index = ctx.index();
    for (i, elem) in items.into_iter().enumerate() {
        ctx.set_index(i);
        path.push_index(i);
        let elem_bound = elem.bind(py);
        if let Err(e) = self.inner.build(py, elem_bound, stream, ctx, path) {
            path.pop();
            match old_index { Some(idx) => ctx.set_index(idx), None => ctx.clear_index() }
            return Err(e);
        }
        path.pop();
    }
    match old_index { Some(idx) => ctx.set_index(idx), None => ctx.clear_index() }
    Ok(())
}
```

#### 4.6.4 sizeof

```rust
fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
    // countfield 大小 + 0（元素数量运行时未知，无法静态计算）。
    // 对齐 Python PrefixedArray._actualsize（需流上下文）→ sizeof 总是 SizeofError。
    // 但若 count 已知（如 ctx 中有），可计算。
    // 简化：永远 Err（保守对齐）。
    Err(ConstructError::Generic {
        message: "PrefixedArray size depends on stream data".to_string(),
        path: String::new(),
    })
}
```

> **替代**（DEV 决策）：若需 sizeof 支持，可对静态 count（countfield 是常量表达式）
> 计算 `countfield.sizeof() + count * inner.sizeof()`。当前保守 Err。

---

## 5. PrefixedArray 的组合方式（决策对比）

### 5.1 选项 A：独立 Node（采用，§4.6）

**优点**：
- 不依赖未实现的 FocusedSeq/Rebuild
- 实现简单，逻辑清晰
- 性能：无间接层

**缺点**：
- 与 Python 实现不一致（Python 是宏组合）
- 若未来实现 FocusedSeq，需考虑是否重构

### 5.2 选项 B：宏展开为节点组合（拒绝）

**思路**：在编译期把 `PrefixedArray(countfield, subcon)` 展开为：
```rust
Node::Struct(StructNode {
    fields: vec![
        StructField { name: "count", node: Node::Rebuild(...), mode: Ro },
        StructField { name: "items", node: Node::Array(...), mode: Rw },
    ],
    ...
})
```

**拒绝理由**：
- Rebuild 节点未实现（依赖"build 时从 context 计算"的能力）
- StructNode 返回的是用户类实例（不是 list），与 PrefixedArray 应返回 list 不符
- 需要引入 FocusedSeq 节点（取字段之一作为整体返回值），增加复杂度
- 性能：多一层 Struct 抽象

### 5.3 决策：采用选项 A（独立 PrefixedArrayNode）

Phase 4 用独立 Node 实现。若未来 FocusedSeq/Rebuild 落地，可保留独立 Node
（性能更优）或迁移到组合方案（行为更一致）。当前选 A 不阻塞未来选项。

---

## 6. 与现有架构的集成

### 6.1 Node enum 扩展

在 `nodes/mod.rs` 的 `Node` enum 新增 6 个变体：

```rust
#[derive(Debug)]
#[enum_dispatch(Construct)]
pub enum Node {
    // ... 现有 13 个变体 ...

    // --- Phase 4：Array 系列 ---
    /// 固定次数数组（对应 Python `Array(count, subcon, discard)`）。
    Array(ArrayNode),
    /// 读到流结束的数组（对应 Python `GreedyRange(subcon, discard)`）。
    GreedyRange(GreedyRangeNode),
    /// 谓词终止数组（对应 Python `RepeatUntil(predicate, subcon, discard)`）。
    RepeatUntil(RepeatUntilNode),
    /// 前缀长度数组（对应 Python `PrefixedArray(countfield, subcon)`）。
    PrefixedArray(PrefixedArrayNode),
    /// 取当前下标（对应 Python `Index`）。
    Index(IndexNode),
    /// 早停信号（对应 Python `StopIf(condfunc)`）。
    StopIf(StopIfNode),
}
```

新增 `pub mod` 声明：

```rust
pub mod array;
pub mod greedy_range;
pub mod repeat_until;
pub mod prefixed_array;
pub mod index;
pub mod stop_if;
```

#### 6.1.1 `has_expressions` 扩展

`Node::has_expressions` 需递归检查 Array 系列的 inner：

```rust
pub fn has_expressions(&self) -> bool {
    match self {
        // 现有分支 ...
        Node::Array(a) => a.has_expressions(),
        Node::GreedyRange(g) => g.has_expressions(),
        Node::RepeatUntil(r) => r.has_expressions(),
        Node::PrefixedArray(p) => p.has_expressions(),
        Node::Index(_) => false,                       // IndexNode 不引用任何 Struct 字段
        Node::StopIf(s) => s.has_expressions(),        // StopIf(Expr) 引用 Struct 字段
    }
}
```

各 Node 的实现：

```rust
impl ArrayNode {
    pub fn has_expressions(&self) -> bool {
        self.inner.has_expressions() || matches!(self.count, CountSource::Expr(_))
    }
}

impl GreedyRangeNode {
    pub fn has_expressions(&self) -> bool {
        self.inner.has_expressions()
    }
}

impl RepeatUntilNode {
    pub fn has_expressions(&self) -> bool {
        // Expr 谓词引用 Struct 字段（如 `RepeatUntil(x > 0, ...)`，x 是字段名引用，
        // **Python 写法为 `this.x > 0`**，construct-rs 不用 `this`）；
        // PyCallable 谓词不参与 ExprProgram 路径，但 inner 可能含表达式。
        self.inner.has_expressions() || matches!(self.predicate, RepeatPredicate::Expr(_))
    }
}

impl PrefixedArrayNode {
    pub fn has_expressions(&self) -> bool {
        // countfield 是独立 Node（如 VarInt、Byte），其本身通常无表达式；
        // inner 子树可能含表达式。
        self.countfield.has_expressions() || self.inner.has_expressions()
    }
}

impl IndexNode {
    pub fn has_expressions(&self) -> bool {
        false   // 仅读 ctx._index（v3：不走 ExprProgram），不引用 Struct 字段
    }
}

impl StopIfNode {
    pub fn has_expressions(&self) -> bool {
        // 关键：StopIf(x == 0)（x 是字段名引用，**Python 写法为 `this.x == 0`**）
        // 中的 x 引用 Struct 字段 x，经 expr_values_buf 取值；
        // 必须返回 true 触发 StructNode 创建 child ctx。
        matches!(self.cond, StopIfCondition::Expr(_))
    }
}

impl StopIfCondition {
    pub fn is_expr(&self) -> bool {
        matches!(self, StopIfCondition::Expr(_))
    }
}
```

> **P1 关键说明（REV 驳回修正）**：`StopIf(Expr)` 节点自身的表达式程序
> （如 `x == 0` 编译为 `[GetInt(0), Const(0), Eq]`，其中 `x` 是字段名引用，
> **Python 等价写法为 `this.x == 0`**）**确实引用 Struct 字段**
> （此例中的 `x`）。`has_expressions()` 必须返回 true，原因有二：
>
> 1. **StructRef 路径**（struct_ref.rs L171/L194）：当 StructRef 引用一个含
>    `Array(5, StopIf(x > 10))`（x 是字段名引用，**Python 写法 `this.x > 10`**）
>    字段的 Struct 时，`root.has_expressions()` 会递归到 StopIf；若返回 false，
>    则 StructRef 不创建 child context、不调用 `init_expr_values`，
>    导致 StopIf 求值时 GetInt 命中空 buf 报错。
>
> 2. **StructRef 不依赖 expr_programs**：虽然顶层 StructNode 通过
>    `expr_programs` 参数知道哪些字段含表达式（编译期），但 StructRef 在
>    **运行时**重新解析 schema 并通过 `root.has_expressions()` 决定是否创建
>    child context。这是运行时路径，与编译期的 expr_programs 是两套机制。
>    因此 `has_expressions()` 必须独立正确。
>
> `StopIf(Always)` / `StopIf(Never)` 不引用字段（编译期常量），返回 false。
> `IndexNode` 仅读 `ctx._index`，不引用 Struct 字段，返回 false。

#### 6.1.2 `compute_ro_value` 扩展

`StopIfNode` 不作为 RO 字段（其 build 行为是检查条件而非计算值）。
`IndexNode` 可作为 RO 字段（值来自 ctx._index，不从实例取）。
但 Index 通常用作普通字段（Rw），不强制 RO。

当前 `compute_ro_value` 不增加 Array 系列分支。若 DEV 发现需要（如 Index 作为 RO），
再补充。

### 6.2 compile.rs 编译管线扩展

#### 6.2.1 描述符识别（按 type_name 字符串）

新增 6 个描述符类型名识别分支（与现有 BitwiseDescriptor 等模式一致）：

```rust
match type_name.to_str()? {
    // ... 现有分支 ...

    "ArrayDescriptor" => return Ok(Node::Array(build_array_node(py, desc, field_index)?)),
    "GreedyRangeDescriptor" => return Ok(Node::GreedyRange(build_greedy_range_node(...)?)),
    "RepeatUntilDescriptor" => return Ok(Node::RepeatUntil(build_repeat_until_node(...)?)),
    "PrefixedArrayDescriptor" => return Ok(Node::PrefixedArray(build_prefixed_array_node(...)?)),
    "IndexDescriptor" => return Ok(Node::Index(IndexNode)),
    "StopIfDescriptor" => return Ok(Node::StopIf(build_stop_if_node(py, desc, field_index, expr_programs)?)),
    _ => {}
}
```

#### 6.2.2 build_array_node 辅助函数

**完整编译设计（P3 修正）**：参照 compile.rs L266-308 的 `BytesDescriptor` 表达式长度
编译路径，给出 ArrayDescriptor 的 count 表达式完整编译路径。

**Python 侧 ArrayDescriptor**：

```python
class ArrayDescriptor:
    """Array(count, subcon, discard) 描述符。"""
    __slots__ = ("count", "subcon", "discard")

    def __init__(self, count, subcon, discard=False):
        self.count = count
        self.subcon = subcon
        self.discard = discard

    @property
    def _expr_params(self):
        """表达式参数协议（与 BytesDescriptor._expr_params 同模式）。

        返回 {"count": self.count}。当 count 是 int 时常量跳过编译；
        是 _FieldDescriptor/_ExprRef 时编译为 ExprOp 列表。
        """
        if isinstance(self.count, int):
            return {}
        return {"count": self.count}
```

**Rust 侧 build_array_node**：

```rust
/// 编译 ArrayDescriptor → ArrayNode。
///
/// 设计参照 build_node_from_descriptor 的 BytesDescriptor 分支（compile.rs L266-308）：
/// 1. 从 desc 读取 count / subcon / discard
/// 2. count 分类：usize 常量 → CountSource::Const；
///    非常量 → 从 expr_programs[field_index]["count"] 取 ExprOp 列表 → CountSource::Expr
/// 3. 递归编译 subcon（沿用 field_index，inner 共享外层字段的 expr_values_buf）
fn build_array_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    bitwise: bool,
) -> Result<ArrayNode, ConstructError> {
    // 1. 递归编译 subcon（沿用 field_index；详见下方"嵌套 inner 表达式"说明）。
    let subcon_desc = desc.getattr("subcon").map_err(|e| ConstructError::Compilation {
        message: format!("ArrayDescriptor missing 'subcon': {}", e),
    })?;
    let inner_node = build_node_from_descriptor(py, &subcon_desc, field_index, expr_programs, bitwise)?;

    // 2. 解析 count：常量 or 表达式。
    let count_obj = desc.getattr("count").map_err(|e| ConstructError::Compilation {
        message: format!("ArrayDescriptor missing 'count': {}", e),
    })?;

    let count = if let Ok(n) = count_obj.extract::<i64>() {
        // 常量路径（Phase 4 首版必须支持，零运行时开销）
        if n < 0 {
            return Err(ConstructError::Compilation {
                message: format!("Array count {} must be non-negative", n),
            });
        }
        CountSource::Const(n as usize)
    } else {
        // 表达式路径：从 expr_programs[field_index]["count"] 取 ExprOp 列表。
        // 与 BytesDescriptor 的 "length" 键完全同模式。
        let field_exprs = expr_programs
            .get(field_index)
            .and_then(Option::as_ref)
            .ok_or_else(|| ConstructError::Compilation {
                message: format!(
                    "Array field has non-constant count but no expression program was provided \
                     (field index {})", field_index
                ),
            })?;

        let field_exprs_dict = field_exprs
            .bind(py)
            .downcast::<PyDict>()
            .map_err(|_| ConstructError::Compilation {
                message: "Array expression program must be a dict".to_string(),
            })?;

        let ops_list = field_exprs_dict
            .get_item("count")
            .map_err(|e| ConstructError::Compilation {
                message: format!("failed to get 'count' from Array expression programs: {}", e),
            })?
            .ok_or_else(|| ConstructError::Compilation {
                message: format!(
                    "Array field has non-constant count but 'count' key missing in expression \
                     program (field index {})", field_index
                ),
            })?;

        let ops = parse_expr_ops_from_py(&ops_list)?;
        let program = ExprProgram::new(ops);
        CountSource::Expr(program)
    };

    // 3. discard 标志。
    let discard: bool = desc
        .getattr("discard")
        .and_then(|d| d.extract())
        .unwrap_or(false);

    Ok(ArrayNode::new(inner_node, count, discard))
}
```

> **CountSource::is_expr 辅助**：DEV 实现时给 `CountSource` 加
> `pub fn is_expr(&self) -> bool { matches!(self, CountSource::Expr(_)) }`，
> 供 `has_expressions` 使用（§6.1.1）。

##### P3.1 嵌套 inner 表达式的支持范围（关键决策）

`build_node_from_descriptor` 递归调用 `build_array_node` 时，**沿用同一个 field_index
和 expr_programs 切片**（参照 BitwiseDescriptor 分支 compile.rs L412-413）。这意味着：

> **语法约定（v3）**：下表中 `this.n` / `this.m` / `this.x` 等是 **Python construct
> 语法**。construct-rs 中等价写法是字段名直接引用（如 `n`、`m`、`x` 是已声明的
> `_FieldDescriptor`），不用 `this.` 前缀。本表沿用 Python 语法仅为对照 Python
> construct 用户的心智模型。

| 场景（Python 语法对照） | 支持情况 | 说明 |
|------|---------|------|
| `Array(this.n, Byte)` | ✅ Phase 4 支持 | 顶层 count 表达式，从 expr_programs[N]["count"] 取 |
| `Array(N, StopIf(this.x))` | ❌ 编译期失败 | inner StopIf 的 "cond" 表达式未被 Python 侧收集（见下方） |
| `Array(N, Bytes(this.m))` | ❌ 编译期失败 | inner Bytes 的 "length" 表达式未被收集 |
| `Array(N, Array(this.m, Byte))` | ❌ 编译期失败 | 内层 Array 的 count 表达式未被收集 |
| `Bitwise(Bytes(this.m))` | ❌ Phase 1-3 已不可用 | 同根问题（_extract_and_compile_exprs 不递归） |

**根因**：Python 侧 `_extract_and_compile_exprs`（_mixin.py L583-615）**只对字段顶层
subcon 调用一次**（_mixin.py L633-634：`subcon = desc.subcon;
_extract_and_compile_exprs(subcon, ...)`），**不递归进入 inner**。当 subcon 是
BitwiseDescriptor / ArrayDescriptor / 等"包装型"描述符（`_expr_params = {}` 或仅含
自身参数）时，inner 的表达式参数不会被收集到 expr_programs 中。

Rust 侧递归调用 `build_node_from_descriptor(inner_desc, field_index, expr_programs, ..)`
时，若 inner 是含表达式的描述符（Bytes(expr_length)、Computed、StopIf(expr_cond) 等），
会从 `expr_programs[field_index]` 查找 "length"/"func"/"cond" 键——但 Python 侧未填入，
报 CompilationError（"key missing"）。**这是 Phase 1-3 既有的限制，Array 与之一致。**

**Phase 4 的支持范围明确为**：

- ✅ **顶层 count 表达式**：`Array(n, simple_subcon)`（`n` 是字段名引用，**Python 写法 `this.n`**），
  simple_subcon 是 FormatField / 常量 Bytes / 常量 BitsInteger / Padding 等不含表达式的描述符。
- ❌ **inner 含表达式**：留待"嵌套表达式收集"特性（独立子任务，建议 Phase 4.0b 或
  Phase 5+，需扩展 `_extract_and_compile_exprs` 递归收集 inner，并解决命名空间冲突
  ——见下方 P3.2）。

**StopIf 在 Struct 直接字段中**（非 inner）依然支持：`@dataclass class S(StructMixin):
x: int = field(Byte); stop = rfield(StopIf(x))`（`x` 是字段名引用，**Python 写法 `this.x`**）。
StopIfDescriptor 是字段，field_index 在字段列表中，其 "cond" 表达式从
`expr_programs[field_index]["cond"]` 取（与 ComputedDescriptor 的 "func" 同模式），
不受上述 inner 限制影响。

##### P3.2 嵌套 inner 表达式的扩展方案（后续子任务参考）

> **语法约定（v3）**：本节示例中 `this.m` / `this.x` 等是 **Python construct 语法**，
> 仅供对照。construct-rs 等价写法是字段名直接引用（`m`、`x`）。

若未来需支持 `Array(N, Bytes(m))`（**Python 写法 `Bytes(this.m)`**）、
`Array(N, StopIf(x))`（**Python 写法 `StopIf(this.x)`**），扩展点：

1. **Python 侧**：修改 `_extract_and_compile_exprs` 递归进入包装型描述符的 `subcon`
   属性，用**路径化键名**避免命名空间冲突：
   ```python
   # 例如 Array(N, Array(m, Byte))（Python 写法 Array(N, Array(this.m, Byte))）
   # 的 expr_programs[N] 结构：
   {
       "count": <outer count ops>,           # 顶层 Array count
       "inner.count": <inner count ops>,     # 内层 Array count（路径化键名）
   }
   # 或用嵌套 dict：{"count": [...], "inner": {"count": [...]}}
   ```
2. **Rust 侧**：`build_array_node` 递归时，从 `expr_programs[field_index]` 取
   `"inner.count"` / `"inner.length"` / `"inner.cond"` 等路径化键。
3. **路径生成规则**：包装型描述符（Bitwise / Bytewise / Array / GreedyRange /
   PrefixedArray / RepeatUntil）递归时给键名前缀 `"inner."`。多重嵌套按层级累加。

**复杂度评估**：~2-3 个子任务（Python 编译器扩展 + Rust build_*_node 路径化取键 +
端到端测试）。**不阻塞 Phase 4 出口**——S-FUNC 仅要求覆盖 Python 顶层 count 表达式。

> **PM 决策点（更新 §12.2）**：Phase 4 首版仅支持顶层 count 表达式。
> inner 含表达式的场景（`Array(N, Bytes(m))`（Python 写法 `Bytes(this.m)`）、
> `Array(N, StopIf(x))`（Python 写法 `StopIf(this.x)`））
> 归入 Phase 4.0b 或 Phase 5+，与现有 `Bitwise(Bytes(m))`（Python 写法
> `Bitwise(Bytes(this.m))`）的限制一致。

#### 6.2.3 其他 build_*_node 函数

模式类似 build_array_node：递归编译 subcon + 提取参数。
详细签名 DEV 实现时参照现有 `build_bits_integer_node` / `build_padding_node`。

### 6.3 Python 层 API

#### 6.3.1 描述符定义（Python 侧）

在 `construct/_constructors.py`（或类似位置）新增：

```python
class ArrayDescriptor:
    """Array(count, subcon, discard) 的描述符。"""
    def __init__(self, count, subcon, discard=False):
        self.count = count
        self.subcon = subcon
        self.discard = discard
        self._expr_params = {}
        # 若 count 是 FieldRef/ExprRef，编译期生成 "count" 键的 ExprOp 列表
        if isinstance(count, ExprMixin) or callable(count):
            self._expr_params["count"] = _compile_expr_tree(count)

class GreedyRangeDescriptor:
    def __init__(self, subcon, discard=False):
        self.subcon = subcon
        self.discard = discard

class RepeatUntilDescriptor:
    def __init__(self, predicate, subcon, discard=False):
        self.predicate = predicate
        self.subcon = subcon
        self.discard = discard

class PrefixedArrayDescriptor:
    def __init__(self, countfield, subcon):
        self.countfield = countfield
        self.subcon = subcon

class IndexDescriptor:
    pass   # 无参数

class StopIfDescriptor:
    def __init__(self, condfunc):
        self.condfunc = condfunc
        self._expr_params = {}
        if isinstance(condfunc, ExprMixin):
            self._expr_params["cond"] = _compile_expr_tree(condfunc)
```

#### 6.3.2 用户 API（构造器函数）

```python
def Array(count, subcon, discard=False):
    return ArrayDescriptor(count, subcon, discard)

def GreedyRange(subcon, discard=False):
    return GreedyRangeDescriptor(subcon, discard)

def RepeatUntil(predicate, subcon, discard=False):
    return RepeatUntilDescriptor(predicate, subcon, discard)

def PrefixedArray(countfield, subcon):
    return PrefixedArrayDescriptor(countfield, subcon)

# Index 与 StopIf 是类（无参数 / 单参数）
class Index:
    def __init__(self):
        pass

class StopIf:
    def __init__(self, condfunc):
        self.condfunc = condfunc
```

#### 6.3.3 `len_` 辅助函数

Python construct 的 `len_(this.items)`（**Python 语法**，`this.items` 引用 items 字段）
用于 PrefixedArray 的 Rebuild。本设计 PrefixedArray 不依赖 Rebuild（§5），故 `len_`
**不在 Phase 4 实现**。若用户代码引用 `len_`，文档提示用 PrefixedArray 节点替代手动组合。

> **若未来实现 Rebuild**：`len_` 表达式编译为 `ExprOp::Len`（取 list 长度）。
> 当前 ExprOp 无 Len 指令，需新增。延后到 Rebuild 阶段。

### 6.4 StructNode 的 StopField 捕获修改

当前 `StructNode::parse`（struct_node.rs L315-356）：

```rust
for (idx, field) in self.fields.iter().enumerate() {
    let value = match field.node.parse(py, stream, ctx, path) {
        Ok(v) => v,
        Err(mut e) => {
            e.push_path_segment(field.name.rust_name());
            return Err(e);
        }
    };
    // ... 处理 value ...
}
```

**Phase 4 修改**：增加 StopField 捕获：

```rust
for (idx, field) in self.fields.iter().enumerate() {
    let value = match field.node.parse(py, stream, ctx, path) {
        Ok(v) => v,
        Err(ConstructError::StopField { .. }) => {
            // StopIf 触发：停止后续字段，正常返回当前实例。
            // 已解析的字段已在 dict 中，未解析的字段不写入（对齐 Python Struct 行为）。
            break;
        }
        Err(mut e) => {
            e.push_path_segment(field.name.rust_name());
            return Err(e);
        }
    };
    // ... 处理 value ...
}
```

**build 方向同样修改**：子字段 build 返回 StopField 时停止后续字段。

> **影响范围**：仅 struct_node.rs 一处。修改是新增分支，不破坏现有行为
> （现有节点不产生 StopField）。
>
> **Sequence 节点**：Phase 4 不实现 Sequence。StopIf 在 Sequence 的捕获
> 留待 Sequence 阶段。

### 6.5 Python 异常类映射

`error.rs::init_exception_classes` 增加 4 个新类的缓存：

```rust
let classes = ExceptionClasses {
    // ... 现有字段 ...
    range_error: get("RangeError")?,
    repeat_error: get("RepeatError")?,
    stop_field_error: get("StopFieldError")?,
    index_field_error: get("IndexFieldError")?,
};
```

`select_exception_class` 增加 4 个分支：

```rust
ConstructError::Range { .. } => &classes.range_error,
ConstructError::Repeat { .. } => &classes.repeat_error,
ConstructError::StopField { .. } => &classes.stop_field_error,
ConstructError::IndexField { .. } => &classes.index_field_error,
```

`build_test_classes`（error.rs 测试辅助）同步增加 4 个测试类定义。

---

## 7. 边界条件清单

### 7.1 Array 边界

| ID | 场景 | 预期行为 |
|----|------|---------|
| AR-1 | count 为常量正整数 | 正常解析/构建 N 个元素 |
| AR-2 | count 表达式求值为负数 | 返回 `Range` 错误（"invalid count -5"） |
| AR-3 | build 时 `len(obj) != count` | 返回 `Range` 错误（"expected N elements, found M"） |
| AR-4 | 嵌套 Array（Array 内 Array） | 内层 _index 覆盖外层，结束后恢复 |
| AR-5 | 子构造器 parse 失败 | 错误向上传播，已解析元素丢弃 |
| AR-6 | count = 0 | 返回空 list，不消耗字节 |
| AR-7 | discard=True | 仍消耗字节，返回空 list |
| AR-8 | count 超过 usize::MAX（i64 表达式） | 求值时 as usize 截断（wrapping），后续 Stream 错误 |
| AR-9 | inner 是 Struct | 每个 elem 是 Struct 实例，list 是 PyList of instances |
| AR-10 | inner 是表达式长度 Bytes（`Array(N, Bytes(m))`，**Python 写法 `Bytes(this.m)`**） | **编译期失败**（CompilationError，"length key missing"）—— inner 表达式收集未实现，§6.2.2 P3.1 |

### 7.2 GreedyRange 边界

| ID | 场景 | 预期行为 |
|----|------|---------|
| GR-1 | 空流（立即 EOF） | 返回空 list |
| GR-2 | 第一个元素解析失败 | 回退到 pos=0，返回空 list |
| GR-3 | 第 N 个元素解析失败 | 回退到第 N 个起始位置，返回前 N-1 个元素 |
| GR-4 | 内含 StopIf | StopField 触发时正常终止（不回退 fallback 之后） |
| GR-5 | stream 末尾部分元素（字节不足） | 回退，返回完整解析的元素 |
| GR-6 | discard=True | 仍消耗字节，返回空 list |
| GR-7 | build 时空列表 | 不写字节，正常返回 |
| GR-8 | build 时某元素失败 | 错误向上传播（不回退，build 无 seek） |
| GR-9 | sizeof | 永远 Err |

### 7.3 RepeatUntil 边界

| ID | 场景 | 预期行为 |
|----|------|---------|
| RU-1 | 子构造器解析失败 | 错误直接上抛（不回退，不像 GreedyRange） |
| RU-2 | 谓词在第 N 个元素为真 | 返回前 N+1 个元素（最后触发的被包含） |
| RU-3 | build 时无元素满足谓词 | 返回 `Repeat` 错误 |
| RU-4 | 谓词 Python callable 抛异常 | 异常向上传播（Generic 错误） |
| RU-5 | 空流且谓词立即满足（不可能，因需先 parse） | 至少 parse 一个元素 |
| RU-6 | sizeof | 永远 Err |
| RU-7 | parse 方向 discard=True | 仍调谓词（用空 list），不收集元素到 obj list |
| RU-8 | build 方向 discard=True（P2 修正） | partiallist 始终为空，谓词收到空 list；对 `lambda x,lst,c: lst[-2:]==[0,0]` 这类依赖 list 内容的谓词会产生与 discard=False 不同的终止时机（对齐 Python core.py L2694-2696） |
| RU-9 | PyCallable 谓词访问 context 字段（v4 新增）<br>例：`lambda x, lst, ctx: x > ctx.threshold` | proxy 是 Container（含 ctx.fields() 全部字段 + `_index`），attribute 与 item 访问均可用。对齐 Python core.py L2681/L2697（Container 通过 `__dict__ = self` 支持 attribute 访问）。详见 §2.5 决策 A5 v4 补充 + §4.3.3 `build_context_proxy` |

### 7.4 PrefixedArray 边界

| ID | 场景 | 预期行为 |
|----|------|---------|
| PA-1 | countfield 解析的 count 为负 | 返回 `Range` 错误 |
| PA-2 | countfield 解析的 count 不是整数 | 返回 `Range` 错误 |
| PA-3 | count = 0 | 返回空 list（countfield 已消耗） |
| PA-4 | count 大于实际可解析元素 | 第 N 个元素失败时错误向上传播 |
| PA-5 | build 时 list 长度超出 countfield 表示范围 | 由 countfield 节点自行报错（如 FormatFieldNode 抛 `FormatField`/`Stream` 错误），PrefixedArrayNode 不做额外校验 |
| PA-6 | sizeof | 永远 Err（保守，§4.6.4） |

### 7.5 Index 边界

| ID | 场景 | 预期行为 |
|----|------|---------|
| IX-1 | 在 Array 内 | 返回当前 _index（PyLong） |
| IX-2 | 不在任何 Array 内（ctx._index=None） | 返回 Py_None（对齐 Python） |
| IX-3 | sizeof | 返回 0（不消耗字节） |
| IX-4 | build | no-op（不写字节） |

### 7.6 StopIf 边界

| ID | 场景 | 预期行为 |
|----|------|---------|
| SI-1 | 在 Struct 内，条件为真 | 抛 StopField，Struct 捕获，停止后续字段 |
| SI-2 | 在 GreedyRange 内，条件为真 | 抛 StopField，GreedyRange 捕获，停止迭代 |
| SI-3 | 在 Array 内，条件为真 | Array **不**捕获，StopField 向上传播（用户误用） |
| SI-4 | 在 Sequence 内（未实现） | 当前不捕获，错误传播（Phase 5+ 补全） |
| SI-5 | 顶层（无外层捕获） | StopField 到达 FFI，映射到 StopFieldError |
| SI-6 | 条件为常量 false | 永远不停止，正常继续 |
| SI-7 | sizeof | 永远 Err |

### 7.7 ListContainer 兼容性

| ID | 场景 | 预期行为 |
|----|------|---------|
| LC-1 | `isinstance(result, ListContainer)` | **失败**（construct-rs 返回原生 list） |
| LC-2 | `result == [1,2,3]` | 正常比较（list == list） |
| LC-3 | `result.search(...)` | **AttributeError**（原生 list 无 search 方法） |
| LC-4 | `repr(result)` | 与 Python 不同（原生 list repr，非 ListContainer 美化） |

### 7.8 嵌套与组合

| ID | 场景 | 预期行为 |
|----|------|---------|
| NE-1 | Array 内 Array | 多维 list，_index 嵌套覆盖/恢复（§3.2.2） |
| NE-2 | Struct 内 Array | Array 作为字段值，返回 list |
| NE-3 | Array 内 Struct | 每个 elem 是 Struct 实例 |
| NE-4 | GreedyRange 内 Struct | 同上，读到流结束 |
| NE-5 | PrefixedArray 内 Struct | count 个 Struct 实例 |
| NE-6 | Bitwise 内 Array | Array 在 bit 域内，inner 是 BitsInteger 等 |
| NE-7 | Array 内 Bitwise | 每个 elem 是 bit 域解析结果 |

---

## 8. 性能假设（S-PERF 验证基础）

### 8.1 瓶颈识别

Python construct `Array(N, Byte).parse` 的主要开销：

| 开销来源 | 单次耗时（估算） | N=100 总耗时 |
|---------|----------------|-------------|
| `_parsereport` 调用（Python 函数调用） | ~1-2μs | 100-200μs |
| `ListContainer.append` | ~100-200ns | 10-20μs |
| `context._index = i`（dict 写入） | ~50-100ns | 5-10μs |
| `evaluate(self.count, context)` | ~200ns（仅一次） | 0.2μs |
| **总计** | | ~120-230μs |

construct-rs `ArrayNode.parse` 的预期开销：

| 开销来源 | 单次耗时（估算） | N=100 总耗时 |
|---------|----------------|-------------|
| `self.inner.parse`（match 分派 + read + PyLong 创建） | ~15-30ns | 1.5-3μs |
| `PyList.append`（C API） | ~30-50ns | 3-5μs |
| `ctx.set_index`（栈字段写入） | ~1ns | 0.1μs |
| `eval_count`（仅一次） | ~5-30ns | 0.03μs |
| `path.push_index/pop` | ~10ns × 2 | 2μs |
| **总计** | | ~7-10μs |

**预测加速比**：120/8 ≈ **15x**（保守），230/7 ≈ **30x**（理想）。

### 8.2 可证伪预测

| 场景 | 预测加速比 | 验证方法 | S-PERF 出口 |
|------|-----------|---------|-----------|
| Array(100, Byte) parse | ≥15x | timeit, N=1000, 取 min | ✅ 纳入 |
| Array(100, Byte) build | ≥12x | timeit, N=1000 | ✅ 纳入 |
| Array(100, Struct{2 fields}) parse | ≥8x | Struct 内层有更多 C API 开销 | ✅ 纳入 |
| GreedyRange(Byte) parse 100 元素 | ≥8x | seek 开销 + 动态长度 | ✅ 纳入 |
| PrefixedArray(Byte, Byte) parse 100 元素 | ≥10x | 等同 Array + 1 次 countfield | ✅ 纳入 |
| RepeatUntil(lambda x,l,c: x>50, Byte) parse（PyCallable） | ≥1.5x | v4：Container proxy + 字段复制开销；PyCallable 是兜底路径 | ⚠️ 见 §8.3 |
| RepeatUntil 同上（Expr 谓词） | ≥8x | Expr 路径消除 FFI | ⚠️ 见 §8.3 |
| Index（在 Array 内） | ≥20x | 仅读 ctx 字段 | ✅ 纳入 |

**证伪条件**：
- 若 Array(100, Byte) parse 加速比 < 10x，视为性能未达标，需排查：
  - PyList::new_bound 预分配是否生效
  - path.push_index/pop 是否成为瓶颈（若是，改用 P0-3 风格错误路径重建）
  - ctx.set_index 是否被错误地走 PyDict 路径
- 若加速比 < 4x（S-PERF 硬目标），视为架构失败，回退设计。

### 8.3 RepeatUntil 的 S-PERF 适用性（M4 修正，明确化）

总纲 §S-PERF："Array parse/build ≥10x vs Python construct"——
"Array" 在 PM 与 ARCH 共识下指**狭义 Array**（ArrayNode），不含 RepeatUntil。
RepeatUntil 因谓词跨 FFI 的固有特性，单独评估。

**S-PERF 出口对 RepeatUntil 的判定规则**：

1. **PyCallable 路径（首版）**：**≥1.5x** 即视为可接受（v4 下调，原 ≥3x）。
   理由（v4 修正）：
   - v4 决策（V-1）要求 PyCallable 路径 proxy 必须是 Container 实例（含 ctx.fields()
     全部字段 + _index），对齐 Python Container 语义。Container 实例化 + 字段复制
     增加 ~400-700ns/iter 开销。
   - 加上原有 FFI 调用 Python callable（~175-225ns/iter），总开销 ~575-1125ns/iter。
   - Python 原版每元素 ~270-300ns/iter，预期加速比 ~1.5-2.5x（取下限 ≥1.5x 作为出口）。
   - **PyCallable 是兜底路径**：仅当用户写复杂谓词（依赖 list 内容或 context 字段）
     时才走此路径。简单谓词（绝大多数场景）自动编译为 Expr 路径（≥10x）。
   - 行为正确性优先于性能（AGENTS.md §0）：Python 兼容性是核心目标，
     不允许为性能砍掉 `ctx.threshold` 这种 Python 文档示明的用法。

2. **Expr 谓词路径（4.5b）**：≥8x。理由：Expr 路径消除 FFI（谓词在 Rust 内求值），
   加速比应接近 Array 的水平。Expr 路径**应作为 Phase 4 内的必要子任务**，
   不延后到 Phase 5+——否则 RepeatUntil 性能不达标。

3. **S-PERF 基准报告**：S-PERF 阶段需对 RepeatUntil 分别测量 PyCallable 与 Expr
   两条路径，单独列出。若仅 PyCallable 路径，需在报告中明确标注
   "RepeatUntil 当前仅 PyCallable，性能 1.5-2.5x；Expr 路径待 4.5b"。
   v4 后报告还需测量"谓词访问 ctx 字段"场景（如 `lambda x, lst, ctx: x > ctx.threshold`），
   验证 Container proxy 行为正确。

**子任务拆分建议（更新 §12.1）**：
- 4.5a：RepeatUntilNode PyCallable 路径（功能完整，3-5x）
- 4.5b：RepeatUntilNode Expr 谓词路径（性能补全，≥8x）
- 4.5b **必须在 Phase 4 验收前完成**，不可延后到 Phase 5+。

### 8.4 验证脚本设计

`experiments/phase4_bench.py`（PM 在 S-PERF 阶段执行）：

```python
import timeit
from construct import Array as PyArray, Byte
from construct_rust import Array as RsArray, ...   # 或通过 StructMixin 子类

# 场景定义
scenarios = [
    ("Array(100, Byte) parse", lambda: PyArray(100, Byte).parse(DATA), ...),
    ("Array(100, Byte) build", ...),
    ...
]

# 两边相同 timeit 参数
NUMBER = 1000
REPEAT = 5

results = []
for name, py_fn, rs_fn in scenarios:
    py_times = timeit.repeat(py_fn, number=NUMBER, repeat=REPEAT)
    rs_times = timeit.repeat(rs_fn, number=NUMBER, repeat=REPEAT)
    py_min = min(py_times) / NUMBER
    rs_min = min(rs_times) / NUMBER
    speedup = py_min / rs_min
    results.append((name, py_min, rs_min, speedup))

# 输出派生指标
print("场景 | Python ns/call | Rust ns/call | 加速比 | parse/build 比率 | 低于4x?")
...
```

**测量口径**（AGENTS.md §6 S-PERF）：
- construct-rs 侧：`maturin develop --release` 安装后，Python 调用用户面 API
- Python 侧：直接 `import construct`，调等效 API
- 子进程隔离（两边包同名，不可同进程导入）
- 取 `min(repeat=5)` × `number=1000`

### 8.5 性能假设的关键依赖

1. **PyList::new_bound + Vec 预分配**：消除 append 循环的 realloc。
   - 若 PyList 内部仍多次 realloc（容量预测不准），加速比下降。
   - 验证：用 `PyList::new_bound(py, Vec::with_capacity(count))` 而非空 List + append。
2. **ctx._index 栈字段**：不走 PyDict。
   - 若误用 `ctx.set_field("_index", i)`，加速比降 5-10x。
   - 验证：profile 检查 PyDict_SetItem 调用次数（应为字段数，非字段数×N）。
3. **path push/pop 开销**：当前每次迭代调用。
   - 若 path.push_index 的 Vec::push 成本 ~10ns 累积过大（N=100 时 1μs），可改为
     P0-3 风格（成功路径不维护，错误时重建）。首版保留 push/pop，性能不达标再优化。
4. **RepeatUntil 的 PyCallable 路径**：每次迭代跨 FFI + Container proxy 构造。
   - N=100 时 100 次 Python 调用 + 100 次 Container 实例化 = ~50-110μs，仅 1.5-2.5x 加速。
   - v4 后开销结构（每 iter）：FFI 调用 ~175-225ns + Container 实例化 + 字段复制
     ~400-700ns + 其他（list.append / path push）~50ns = ~625-975ns/iter。
   - Expr 路径补全后消除 FFI 与 Container proxy，预期 ≥8x。

---

## 9. 与 Python 版本的对应

### 9.1 类/方法映射表

| Python 类/方法 | Rust 类型/方法 | 行号 | 状态 |
|---------------|---------------|------|------|
| `Array` | `ArrayNode` | 2493 | ✅ 完整映射 |
| `Array.__init__(count, subcon, discard)` | `ArrayNode::new(inner, count, discard)` | 2520 | ✅ |
| `Array._parse` | `ArrayNode::parse` | 2525 | ✅ |
| `Array._build` | `ArrayNode::build` | 2538 | ✅ |
| `Array._sizeof` | `ArrayNode::sizeof` | 2553 | ✅ |
| `GreedyRange` | `GreedyRangeNode` | 2570 | ✅ |
| `GreedyRange._parse` | `GreedyRangeNode::parse` | 2599 | ✅（简化错误分流） |
| `GreedyRange._build` | `GreedyRangeNode::build` | 2617 | ✅ |
| `GreedyRange._sizeof` | `GreedyRangeNode::sizeof` | 2630 | ✅（永远 Err） |
| `RepeatUntil` | `RepeatUntilNode` | 2637 | ✅（首版仅 PyCallable） |
| `RepeatUntil._parse` | `RepeatUntilNode::parse` | 2670 | ✅ |
| `RepeatUntil._build` | `RepeatUntilNode::build` | 2684 | ✅ |
| `RepeatUntil._sizeof` | `RepeatUntilNode::sizeof` | 2703 | ✅（永远 Err） |
| `PrefixedArray` | `PrefixedArrayNode` | 4934 | ✅（独立实现，不依赖 FocusedSeq） |
| `PrefixedArray._emitparse` | `PrefixedArrayNode::parse` | 4961 | ✅ |
| `PrefixedArray._emitbuild` | `PrefixedArrayNode::build` | 4965 | ✅ |
| `Index` | `IndexNode` | 2934 | ✅ |
| `Index._parse` | `IndexNode::parse` | 2965 | ✅ |
| `Index._build` | `IndexNode::build` | 2968 | ✅（no-op） |
| `Index._sizeof` | `IndexNode::sizeof` | 2971 | ✅（返回 0） |
| `StopIf` | `StopIfNode` | 4079 | ✅ |
| `StopIf._parse` | `StopIfNode::parse` | 4103 | ✅（StopField 哨兵） |
| `StopIf._build` | `StopIfNode::build` | 4108 | ✅ |
| `StopIf._sizeof` | `StopIfNode::sizeof` | 4113 | ✅（永远 Err） |
| `ListContainer` | 原生 `PyList` | containers.py | ⚠️ 不引入子类（§2.7） |
| `LazyArray` | — | 6118 | ❌ 延后到惰性阶段 |
| `LazyListContainer` | — | — | ❌ 延后 |
| `len_` | — | — | ❌ PrefixedArray 不依赖（§6.3.3） |

### 9.2 异常映射

| Python 异常 | Rust 错误变体 | 备注 |
|------------|--------------|------|
| `RangeError` | `ConstructError::Range` | 新增 |
| `RepeatError` | `ConstructError::Repeat` | 新增 |
| `StopFieldError` | `ConstructError::StopField` | 新增（哨兵） |
| `IndexFieldError` | `ConstructError::IndexField` | 新增（保留，首版不触发） |
| `SizeofError`（GreedyRange/RepeatUntil/StopIf） | `ConstructError::Generic` | 复用现有 |
| `StreamError` | `ConstructError::Stream` | 复用现有 |

### 9.3 Python context 字段对应

| Python context 字段 | Rust Context 字段 | 备注 |
|--------------------|--------------------|------|
| `context._index` | `Context._index: Option<usize>` | 新增（§3.2） |
| `context.<field>` | `Context.expr_values_buf[idx]` 或 `fields` PyDict | 现有 |
| `context._`（外层） | `Context.parent: Option<&Context>` | 现有 |

### 9.4 Python 表达式对应

> **V2 修正（v3）**：本表重构为三列对照，明确 construct-rs 用户面**不使用 `this.xxx` 语法**。
> construct-rs 表达式系统输入是 `_FieldDescriptor` / `_ExprRef` 对象（字段名直接引用），
> 详见表达式系统 §2.3。Phase 2 决策 1 明确禁止 `this.xxx` 语法。

| Python construct 语法 | construct-rs 用户面语法 | Rust ExprOp | 备注 |
|-----------------------|------------------------|-------------|------|
| `this.<field>`（如 `this.count`） | `<field>`（如 `count`，字段名直接引用，是 `_FieldDescriptor` 对象） | `GetInt(idx)` | 现有；编译期 `id(descriptor)` 绑定字段索引 |
| 算术/位/比较（如 `this.a + this.b`） | 字段名 + 运算符（如 `a + b`，编译为 `_ExprRef` 树） | `Add` / `Sub` / ... / `Gt` / ... | 现有；表达式系统 §2.3.3 |
| `this._index`（数组下标引用） | **不直接支持**——通过 Index 字段 + 字段名引用实现：`i: int = rfield(Index()); v: bytes = field(Bytes(i + 1))` | `[GetInt(idx_of_i), Const(1), Add]` | v3 决策（§3.3）；不引入 `GetIndex` 指令，复用 GetInt |

> **v3 决策说明**：v1/v2 设计了 `ExprOp::GetIndex` 指令作为 `this._index` 的对应物，
> 但用户面入口违反 Phase 2 决策 1（废弃 `this`）。v3 删除 GetIndex，统一通过
> IndexNode + 字段引用机制实现等价功能。详见 §3.3 与设计决策记录 Phase 4 决策 3。

### 9.5 已知行为差异（M1 修正 + 整理）

construct-rs 与 Python construct 2.10.70 的有意行为差异，列于此集中管理。
PM/REV 验收时需逐项确认（用户文档应注明）。

| ID | 差异点 | Python 行为 | construct-rs 行为 | 理由 / 影响范围 |
|----|--------|------------|------------------|----------------|
| LC-1 | `isinstance(result, ListContainer)` | True | **False**（返回原生 list） | ListContainer 子类实例化走 Python 慢路径；§2.7 决策 A7 |
| LC-2 | `result.search(name, value)` | 可用 | **AttributeError**（原生 list 无 search） | 工具方法可后续作为独立函数提供 |
| LC-3 | `repr(result)` / `str(result)` | ListContainer 美化（缩进展示） | 原生 list repr | 用户可用 pprint 替代；非核心功能 |
| LC-4 | `result == [1,2,3]` | True（值相等） | True | 值比较一致，无差异 |
| IX-1 | 数组下标引用不在数组内（Python 写法 `Computed(this._index + 1)`） | `TypeError: None + 1` | construct-rs 等价写法 `i: int = rfield(Index()); v = rfield(Computed(i + 1))` 在 `i` 为 None 时 `i + 1` 表达式求值抛 `ExprType`（None 无法 extract 为 i64） | v3：行为与 Python 一致（都报错）。v1/v2 设计的 `GetIndex` 在 None 时返回 0 导致行为差异，已删除（§3.3） |
| GE-1 | GreedyRange 内部子构造器抛 `ExplicitError` | 向上传播（不回退） | **无 ExplicitError 等价物**，与其他错误一样 seek 回退 + 正常终止 | 见下方详述 |
| GE-2 | GreedyRange 内部子构造器抛 FormatField/Stream 等普通错误 | seek 回退 + 正常终止 | 同 Python（seek 回退 + 正常终止） | 行为一致 |
| RU-build-1 | RepeatUntil build discard=True 时谓词收到的 list | 空 list（始终为空） | 空 list（对齐 Python，§4.3.4 P2 修正） | 行为一致 |
| PA-5 | PrefixedArray build 时 list 长度超出 countfield 表示范围 | countfield 抛 FormatFieldError/StreamError | 由 countfield 节点自行报错（同方向） | 行为一致，§7.4 PA-5 |
| NE-expr-1 | `Array(N, Bytes(m))` inner 含表达式（**Python 写法 `Bytes(this.m)`**） | 支持 | **编译期失败**（CompilationError） | 与 `Bitwise(Bytes(m))`（Python 写法 `Bitwise(Bytes(this.m))`）同限制；§6.2.2 P3.1 |
| NE-expr-2 | `Array(N, StopIf(x))` inner 含表达式（**Python 写法 `StopIf(this.x)`**） | 支持 | **编译期失败** | 同上，需扩展 _extract_and_compile_exprs 递归 |

#### M1 详述：GreedyRange 错误吞掉（GE-1）

Python `GreedyRange._parse`（core.py L2599-2615）区分三类异常：

```python
try:
    for i in itertools.count():
        ...
except StopFieldError:      # 1. 早停信号：正常终止，不回退
    pass
except ExplicitError:        # 2. 显式不可恢复错误：向上传播
    raise
except Exception:            # 3. 其他错误（含 Stream/FormatField 等）：seek 回退 + 正常终止
    stream_seek(stream, fallback, 0, path)
```

construct-rs 当前设计（§4.2.2）的分流：

```rust
match self.inner.parse(...) {
    Ok(elem) => { ... },
    Err(StopField { .. }) => { stream.seek(fallback, ...)?; break; },  // 1. 同 Python
    Err(e) => { let _ = stream.seek(fallback, ...); break; },          // 2+3 合并
}
```

**差异**：construct-rs 没有 `ConstructError::Explicit` 等价变体。Python 的
`ExplicitError` 是用户/库显式标记的"不可恢复错误"（在 Stream/FieldError 之上），
construct-rs 把所有非 StopField 错误都视为"流终止信号"（回退 + 正常终止）。

**影响**：
- **FormatField/Stream 错误**（如字节不足、整数溢出）：行为与 Python 一致（GE-2）。
- **用户主动抛 ExplicitError**：Python 让错误传播（GreedyRange 不终止），construct-rs
  把它当成普通错误终止 GreedyRange。**但**：当前 construct-rs 不暴露任何
  ExplicitError 触发 API（无对应描述符），所以**实际无用户场景触发此差异**。

**决策**：**不引入 `ConstructError::Explicit`**。理由：
1. 当前用户面 API 不暴露 ExplicitError 触发方式，差异不可达。
2. 引入新变体需要：(a) Python 侧暴露 `ExplicitError` 类；(b) Rust 侧新增 ConstructError
   变体 + 全节点链路正确传播；(c) 测试覆盖。工作量大，收益小。
3. Python `ExplicitError` 极少用（仅在 Switch/If ThenElse 等高级特性的错误分支），
   Phase 4 不覆盖这些特性。

**后续处理（标记为已知问题）**：若未来 Phase 5+ 实现 Switch / IfThenElse 并暴露
`ExplicitError` 用户 API，再补充 `ConstructError::Explicit` 变体并在 GreedyRange 分流。
当前 GE-1 作为已知差异记录，**不阻塞 Phase 4 出口**。

---

## 10. 后续优化方向（非 Phase 4 范围）

### 10.1 RepeatUntil 的 Expr 谓词路径

§4.3.2 简化为仅 PyCallable。Expr 路径补全后预期性能提升 2-3x。

**实现要点**：
1. 新增 `ExprOp::GetElem` 指令：从 ctx._current_elem 取 borrowed PyObject 指针，
   调 `PyLong_AsLongLong`（仅支持整数元素）。
2. Context 新增 `_current_elem: Option<*mut ffi::PyObject>` 槽位（borrowed）。
3. `RepeatUntilNode::parse` Expr 路径：每次迭代 parse 后设置 _current_elem，
   调 `eval_expr_int(&prog, ctx, py)`。
4. Python 侧 `_compile_expr_tree` 识别简单谓词模式（如 `lambda x,_,__: x > N`）。

### 10.2 path push/pop 优化（P0-3 风格）

若性能分析显示 path.push_index/pop 成为瓶颈（~2μs/100 元素），改为成功路径不维护：

```rust
// 成功路径不 push/pop
let elem = self.inner.parse(py, stream, ctx, path)?;
// 错误路径在 ArrayNode::parse 的 Err 分支内 push_index 后 return
```

但这要求子节点错误时 path 是 "root" 状态（不含数组索引），ArrayNode 在错误分支
push_index 重建。与现有 StructNode 的 P0-3 优化一致。

### 10.3 LazyArray / LazyListContainer

Python `LazyArray` 使用惰性解析（按需解析元素）。归入惰性解析 Phase（与 Lazy、
LazyStruct 同阶段）。

### 10.4 Rebuild / FocusedSeq 与 PrefixedArray 重构

若未来实现 Rebuild / FocusedSeq，PrefixedArray 可迁移到组合方案（§5.2 选项 B）。
当前独立 Node 方案性能更优，迁移非必要。

### 10.5 path.push_index 的整数格式化优化

当前 `Path::push_index` 用 `format!("[{}]", idx)`，每次分配 String。
可改为 `Path` 内部用 `Vec<usize>` 存索引，Display 时一次性格式化。
但这是 Phase 1 遗留优化，不在 Phase 4 范围。

---

## 11. 设计完整性自检

### 11.1 API 映射完整性

✅ 覆盖 Python Array 功能集的全部公开方法（§9.1）
✅ 异常映射完整（§9.2）
✅ Context 字段映射完整（§9.3）
✅ 表达式映射完整（§9.4，v3 重构为三列对照，明确 construct-rs 不用 `this.xxx`）
✅ 决策记录对照完整（v3 修正后，Phase 2 决策 1 不再违反，详见 §11.7）

### 11.2 边界条件完整性

✅ Array 边界 10 条（§7.1，AR-10 已更新为"inner 表达式编译期失败"）
✅ GreedyRange 边界 9 条（§7.2）
✅ RepeatUntil 边界 8 条（§7.3，新增 RU-8 build discard 语义）
✅ PrefixedArray 边界 6 条（§7.4，PA-5 措辞修正）
✅ Index 边界 4 条（§7.5）
✅ StopIf 边界 7 条（§7.6）
✅ ListContainer 兼容性 4 条（§7.7）
✅ 嵌套与组合 7 条（§7.8）
✅ 已知行为差异 11 条（§9.5，含 LC/IX/GE/RU-build/PA/NE-expr 全集）

### 11.3 与现有架构的兼容性

✅ 不修改 Construct trait 签名（13 个现有节点无需改动）
✅ 不修改现有 Node 变体的行为
✅ 不修改 ParseStream/BuildStream 的现有 API（仅新增 seek）
✅ Context 新增字段不影响现有构造函数语义（_index 默认 None）
✅ compile_schema 向后兼容（新增描述符识别分支）
✅ StructNode 仅增加 StopField 捕获分支（不破坏现有行为）
✅ has_expressions 修正不影响现有 13 个 Node 变体（仅 StopIf/Index 新增分支）

### 11.4 性能假设的瓶颈覆盖

✅ FFI 调用（RepeatUntil PyCallable 路径有，Expr 路径消除）
✅ PyList 创建与 append（§8.1）
✅ ctx._index 写入（§8.1）
✅ path push/pop（§8.1、§8.5）
✅ 子节点 parse 分派（§8.1）
✅ RepeatUntil S-PERF 适用性（§8.3 单独明确）

### 11.5 编码红线遵守

✅ 无 `unwrap()` / `expect()` 在非测试代码（伪代码中的 unwrap 仅为示意，DEV 实现时
   用 `?` 或 match）
✅ 无 panic（错误返回 Err）
✅ 无 TODO/FIXME（设计文档中无；v1 的 placeholder 已在 v2 补全）
✅ 无硬编码魔法数字（常量在模块顶部定义）
✅ 所有 pub 项有 `///` 文档注释（DEV 实现时补全）
✅ 错误携带 path（所有新增错误变体都有 path 字段）
✅ parse/build 对称性（所有 Array 节点同时支持 parse 和 build）

### 11.6 REV 驳回修正自检（v2 新增）

| 编号 | 自检项 | 结论 |
|------|--------|------|
| P1 | §6.1.1 中 `StopIf(s)` 的 has_expressions 是否返回 `matches!(cond, Expr(_))`？ | ✅ |
| P1 | 是否附 StructRef 路径（struct_ref.rs L171/L194）的根因说明？ | ✅ |
| P2 | §4.3.4 build 伪代码中 `partial.append` 是否在 `if !self.discard` 守卫内？ | ✅ |
| P2 | §7.3 是否新增 RU-8 build discard 边界条目？ | ✅ |
| P3 | §6.2.2 是否给出完整的表达式 count 编译路径（无 placeholder）？ | ✅ |
| P3 | 是否明确 inner 含表达式的支持范围（P3.1）与扩展方案（P3.2）？ | ✅ |
| M1 | §9.5 是否新增 GE-1（GreedyRange 错误吞掉）条目并附决策说明？ | ✅ |
| M2 | §7.4 PA-5 是否删除"回绕"误述，改为"countfield 自行报错"？ | ✅ |
| M3 | §4.1.4 是否明确 sizeof 的 GIL 前置条件 + with_gil 闭包正确写法？ | ✅ |
| M4 | §8.3 是否明确 S-PERF 出口对 RepeatUntil 的判定规则？ | ✅ |
| M4 | §12.1 是否明确 4.5a/4.5b 拆分且 4.5b 必须在 Phase 4 验收前完成？ | ✅ |

### 11.7 REV 决策记录对照驳回修正自检（v3 新增）

| 编号 | 自检项 | 结论 |
|------|--------|------|
| V1 | §3.3 是否删除 ExprOp::GetIndex（不再扩展表达式系统）？ | ✅ 整节重写为"不扩展 ExprOp" |
| V1 | 是否在设计决策记录 Phase 4 决策 3 写入"Index 仅作为构造器字段"？ | ✅ |
| V1 | 是否明确用户访问下标的机制（`rfield(Index())` + 字段引用）？ | ✅ §3.3 表格 + §4.4 示例 |
| V1 | 是否说明已实现代码（expr.rs/compile.rs 的 GetIndex）需 DEV 回退？ | ✅ §3.3 + §12.7 |
| V1 | 是否保留 Context._index 字段（IndexNode 仍需通过 ctx.index() 读）？ | ✅ §3.3 明确"保留" |
| V2 | §9.4 是否重构为三列对照（Python / construct-rs / ExprOp）？ | ✅ |
| V2 | 是否明确 construct-rs 用户面不用 `this.xxx` 语法？ | ✅ §9.4 表头 + 表内说明 |
| V3 | §2.2 CountSource::Expr 示例的 `this.length` 是否标注 Python 语法？ | ✅ |
| V3 | §2.3 的 `this._index` 描述是否改为标注 Python 语法或删除？ | ✅ 重写为"不通过用户面表达式暴露" |
| V3 | §2.6 PrefixedArray 宏 `len_(this.items)` 是否标注 Python 语法？ | ✅ |
| V3 | §3.2.2 `this._index` 是否标注 Python 语法？ | ✅ |
| V3 | §6.1.1 has_expressions 注释中 `this.x` 是否改为 construct-rs 语法或标注？ | ✅ 改为字段名引用 + Python 写法注释 |
| V3 | §6.2.2 P3.1/P3.2 表格中 `this.n`/`this.m`/`this.x` 是否标注 Python 语法？ | ✅ 表前加语法约定 + 表内字段名引用 |
| V3 | §6.3.3 `len_(this.items)` 是否标注 Python 语法？ | ✅ |
| V3 | §7.1 AR-10 边界 `Bytes(this.m)` 是否标注 Python 语法？ | ✅ |
| V3 | §9.5 NE-expr-1/2 差异表 `this.m`/`this.x` 是否标注 Python 语法？ | ✅ |
| V3 | §12.2 PM 决策点 2 表格 `this.xxx` 是否标注 Python 语法？ | ✅ 表前加语法约定 |
| V4 | §4.5.1 StopIfCondition::Expr 注释 `this.x == 0` 是否改为 construct-rs 语法？ | ✅ 改为 `x == 0` + 编译产物说明 |

### 11.8 VET 驳回修正自检（v4 新增）

| 编号 | 自检项 | 结论 |
|------|--------|------|
| V-1 | §2.5 决策 A5 是否补充 PyCallable proxy 硬性约束（Container + 字段复制）？ | ✅ 补充 4 条约束 + 决策依据 |
| V-1 | §4.3.3 `build_context_proxy` 是否改为 Container 包装的硬性约束（删除"DEV 决策"模糊空间）？ | ✅ 提供完整实现伪代码 + 性能估算 |
| V-1 | §4.3.3 / §4.3.4 伪代码是否传 container_cls 并构造 Container proxy？ | ✅ parse / build 均更新 |
| V-1 | §7.3 是否新增 RU-9（谓词访问 context 字段）边界？ | ✅ |
| V-1 | §8.2/§8.3 是否更新 PyCallable 路径性能预期（≥3x → ≥1.5x）？ | ✅ 含 v4 决策依据说明 |
| V-1 | §8.5 关键依赖 4 是否更新开销结构（含 Container 实例化）？ | ✅ |
| V-1 | §9.5 是否需要新增已知差异条目？ | ✅ 不新增（方案 A 行为对齐 Python，无差异） |
| V-1 | 设计决策记录 Phase 4 是否新增决策 4？ | ✅ "PyCallable proxy 必须用 Container 包装" |
| V-2~V-5 | 是否明确这些次要问题由 DEV 在 V-1 修复批次中附带处理？ | ✅ §0.3 末尾说明 |

---

## 12. 遗留问题与 PM 决策点

### 12.1 PM 决策点 1：RepeatUntil 谓词路径范围（M4 已明确）

**问题**：RepeatUntil 的 Expr 谓词路径（§4.3.2）是否在 Phase 4 实现？

**结论（M4 修正后）**：**Expr 路径必须在 Phase 4 验收前完成**（子任务 4.5b），
不可延后到 Phase 5+。

理由（详见 §8.3 S-PERF 适用性）：
- PyCallable 路径（4.5a）功能完整、行为正确，但性能仅 1.5-2.5x（v4 后物理上限）。
- 若仅交付 PyCallable，RepeatUntil 不满足 S-PERF 出口标准。
- Expr 路径（4.5b）消除 FFI 与 Container proxy 开销，预期 ≥8x，满足出口标准。
- 4.5b 工作量：`_current_elem` 槽位 + `ExprOp::GetElem`（仅整数元素）+
  Python 编译器识别简单谓词模式 + 端到端测试。约 1-2 个子任务。

**Expr 路径已知限制**（写入设计文档 §10.1）：
- 仅支持整数元素（PyLong_AsLongLong）。Struct 元素的 RepeatUntil 仍走 PyCallable。
- 仅识别简单谓词模式（`lambda x,_,_: x OP N`，OP ∈ {>, >=, ==, !=, <, <=}）。
- 复杂谓词（如 `lambda x,lst,c: lst[-2:] == [0,0]`）回落到 PyCallable。

**子任务拆分**：
- 4.5a：RepeatUntilNode PyCallable 路径（功能完整，3-5x）—— S-FUNC 出口
- 4.5b：RepeatUntilNode Expr 谓词路径（性能补全，≥8x）—— S-PERF 出口
- 4.5b **必须在 Phase 4 验收前完成**。

### 12.2 PM 决策点 2：Array count 表达式支持范围（P3 已明确）

**问题**：Array 的 count 表达式（**Python 写法 `Array(this.length, Byte)`**；
construct-rs 等价写法 `Array(length, Byte)`，`length` 是字段名引用）支持范围？

**结论（P3 修正后）**：

> **语法约定（v3）**：下表"场景"列同时给出 Python 写法（含 `this.`）与 construct-rs
> 等价写法（字段名直接引用），便于 Python construct 用户对照。

| 场景 | Phase 4 支持 | 说明 |
|------|------------|------|
| 顶层 count 表达式（construct-rs：`Array(n, simple_subcon)`；Python 写法：`Array(this.n, ...)`） | ✅ 支持 | 从 `expr_programs[field_index]["count"]` 取，参照 BytesDescriptor 表达式长度（§6.2.2 完整路径） |
| inner 含表达式（construct-rs：`Array(N, Bytes(m))`；Python 写法：`Array(N, Bytes(this.m))`） | ❌ 编译期失败 | 与 `Bitwise(Bytes(m))`（Python 写法 `Bitwise(Bytes(this.m))`）同限制；§6.2.2 P3.1 |
| inner 含表达式（construct-rs：`Array(N, StopIf(x))`；Python 写法：`Array(N, StopIf(this.x))`） | ❌ 编译期失败 | 同上 |
| 嵌套 Array 表达式（construct-rs：`Array(N, Array(m, Byte))`；Python 写法：`Array(N, Array(this.m, Byte))`） | ❌ 编译期失败 | 同上 |
| StopIf 作为 Struct 直接字段（construct-rs：`rfield(StopIf(x))`；Python 写法：`rfield(StopIf(this.x))`） | ✅ 支持 | 与 ComputedDescriptor 的 "func" 同模式 |

理由：
- 顶层 count 表达式：ExprProgram 基础设施已具备，完整编译路径已在 §6.2.2 设计。
  不实现会显著限制 Array 的实用性（用户无法用 `Array(n, Byte)` 引用前序字段）。
- inner 含表达式：Python 侧 `_extract_and_compile_exprs` 当前不递归（_mixin.py L583-615），
  需扩展递归收集 + 解决命名空间冲突。属于独立特性，建议 Phase 4.0b 或 Phase 5+。

**PM 行动**：
- 子任务 4.1（ArrayNode）必须支持顶层 count 表达式（参照 §6.2.2 完整编译路径）。
- 若需要 inner 含表达式支持，新增子任务（Phase 4.0b 或后续），扩展 Python 侧
  `_extract_and_compile_exprs` 递归（§6.2.2 P3.2 扩展方案）。
- **与现有 `Bitwise(Bytes(m))` 限制一致**（Python 写法 `Bitwise(Bytes(this.m))`）：
  在用户文档中明确说明"包装型描述符的 inner 不支持含表达式的子描述符，需扁平化到字段层级"。

**PM 行动**：确认 expr_programs 在编译期的传递路径（§6.2.2）。

### 12.3 PM 决策点 3：sizeof 是否支持静态 PrefixedArray

**问题**：PrefixedArray 的 sizeof 当前保守返回 Err（§4.6.4）。是否支持静态计算？

**建议**：首版保守 Err。理由：
- 实际场景中 PrefixedArray 的 count 来自流，sizeof 几乎不可静态计算。
- 若 countfield 是 ConstExpression，理论上可计算，但场景罕见。
- 保守 Err 不阻塞用户（用户极少对 PrefixedArray 调 sizeof）。

**PM 行动**：确认是否接受保守 Err，或要求 DEV 实现静态路径。

### 12.4 PM 决策点 4：ListContainer 子类是否引入

**问题**：是否实现 ListContainer Python 子类（§2.7）？

**建议**：不实现。理由：
- 性能代价（子类实例化慢路径）。
- repr 美化非必要。
- search/search_all 可作为独立函数。

**PM 行动**：确认接受行为差异（§9.5 LC-1~LC-4），或在 Phase 4 后单独子任务实现。

### 12.5 PM 决策点 5：path.push_index 是否首版优化

**问题**：Array 循环内每次 push_index/pop（~2μs/100 元素）是否首版就用 P0-3 风格优化？

**建议**：首版保留 push/pop，性能不达标再优化。理由：
- 优化增加代码复杂度（错误路径需手动重建）。
- 2μs/100 元素对 ≥10x 目标影响有限（占总耗时 ~20%）。
- 性能分析后再决策（§8.5 关键依赖 3）。

**PM 行动**：在 S-PERF 阶段若发现 path push/pop 是瓶颈，新增子任务优化。

### 12.6 PM 决策点 6：sizeof 签名是否增加 py 参数

**问题**：当前 `sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError>` 无 `py`
参数。Array 表达式 count 的 sizeof 需 GIL 调 `eval_expr_int`（§4.1.4）。

**选项**：
- A：sizeof 内部用 `Python::with_gil`（O(1) 若已持有 GIL，首版采用）
- B：修改 Construct trait，sizeof 增加 `py: Python<'_>` 参数（影响全部节点）

**建议**：选项 A。sizeof 不是热路径，with_gil 开销可接受。

**PM 行动**：若 DEV 实现时发现选项 A 有问题，再讨论选项 B。

### 12.7 设计完整性结论

✅ 设计覆盖全部 P0-P5 构造器
✅ 与现有架构兼容（无破坏性变更）
✅ 性能假设具体可证伪
✅ 边界条件覆盖完整（含 REV 驳回修正后的 RU-8 build discard、AR-10 inner 限制）
✅ 遗留问题已明确（6 个 PM 决策点，其中 1/2 已根据 REV 驳回收敛）
✅ REV 驳回的 3 个严重问题（P1/P2/P3）已修正（v2）
✅ REV 驳回的 4 个中等问题（M1/M2/M3/M4）已修正（v2）
✅ REV 决策记录对照驳回的 2 个设计问题（V1/V2）已修正（v3）
✅ REV 决策记录对照驳回的 2 个文档表述问题（V3/V4）已修正（v3）

**建议 PM 在子任务拆分时**：
1. 4.0a：基础设施（ParseStream.seek、Context._index、错误变体、Python 异常类映射）
   —— 这是所有 Array 节点的前置依赖
   - **v3 修正**：不再包含 ExprOp::GetIndex（已删除，§3.3）。**4.1 子任务已实现的
     GetIndex 相关代码（expr.rs / compile.rs "getindex" 分支）需由 DEV 在新子任务中回退**。
2. 4.1：ArrayNode（P0，含顶层 count 表达式，§6.2.2 完整编译路径）—— **已实现，
   需附加 GetIndex 回退子任务**
3. 4.2：GreedyRangeNode（P1）
4. 4.3：PrefixedArrayNode（P2）
5. 4.4：IndexNode（P4，**直接调 `ctx.index()`，不走 ExprProgram**）+ StopIfNode（P5）
   + StructNode 的 StopField 捕获修改 + has_expressions 修正（§6.1.1，StopIf(Expr) 返回 true）
6. 4.5a：RepeatUntilNode PyCallable 路径（功能完整，S-FUNC 出口）
7. 4.5b：RepeatUntilNode Expr 谓词路径（性能补全，S-PERF 出口，必须在 Phase 4 验收前完成）
8. 4.6：S-FUNC 验证 + S-PERF 基准（含 RepeatUntil 双路径对比）

子任务可合并（如 4.1+4.2 一次 DEV 编码），由 PM 决定。

**REV 修正后无需重新检视全部内容**，仅针对修改部分重新检视即可。

---

## 13. 参考文献

- `AGENTS.md` §0、§7、§8、§10
- `plans/phase4-array/分析报告-Array功能集.md`
- `plans/phase4-array/总纲.md`
- `docs/架构设计.md` §C.1-C.6
- `docs/模块设计-BitStream.md`（Box<Node> 递归模式参考）
- `docs/模块设计-表达式系统.md`（ExprProgram / ExprOp 参考）
- `docs/模块设计-Context-Vec优化.md`（Context 栈分配模式参考）
- `construct/construct/core.py` L2493-2704、L2934-2972、L4079-4131、L4934-4983
- `construct/construct/lib/containers.py`（ListContainer）

---

**设计完成。等待 PM 与 REV 审查。**



