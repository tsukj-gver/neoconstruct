# 模块设计：Array 支持（Phase 4）

> **设计依据**：
> - `AGENTS.md` §0（核心原则：一次 FFI、无中间表示层、直接操作 Python 对象）、
>   §7（技术决策）、§8（编码红线）、§10（Python 参考速查）
> - `plans/phase4-array/分析报告-Array功能集.md`（功能集分析）
> - `plans/phase4-array/总纲.md`（出口标准：S-FUNC、S-QUAL、S-PERF ≥10x）
> - `construct-rs/src/nodes/mod.rs`（现有 Node enum + Construct trait）
> - `construct-rs/src/stream.rs`（现有 ParseStream/BuildStream）
> - `construct-rs/src/context.rs`（现有 Context）
> - `construct-rs/src/expr.rs`（现有 ExprOp / ExprProgram）
> - `construct-rs/src/compile.rs`（现有编译管线）
> - `construct-rs/src/error.rs`（现有错误变体）
> - Python 源码：`construct/construct/core.py`
>   （Array L2493、GreedyRange L2570、RepeatUntil L2637、Index L2934、
>   StopIf L4079、PrefixedArray L4934）、`construct/construct/lib/containers.py`（ListContainer）
>
> **角色**：ARCH
> **状态**：DESIGNING
> **创建时间**：2026-06-29

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
- `Context` 新增 `_index: Option<usize>` 字段 + 读写方法（§3.2）
- `ExprOp` 新增 `GetIndex` 指令（§3.3）
- `ConstructError` 新增 `Range` / `Repeat` / `StopField` / `IndexField` 4 个变体（§3.4）
- Node enum 新增 6 个变体（§4）
- `compile_schema` 新增 Array 描述符识别分支（§6.2）
- Python 侧新增 Array 描述符与 `len_` 辅助函数（§6.3）

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
2. 上下文 lambda：`Array(this.length, Byte)`、`Array(lambda ctx: ctx.n, Byte)`

编译期把这两种来源编译为 `CountSource` 枚举：

```rust
pub enum CountSource {
    /// 编译期常量（如 `Array(5, Byte)`）。
    Const(usize),
    /// 表达式程序（如 `Array(this.length, Byte)`，复用现有 ExprProgram）。
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
`context.get("_index")` 或 `this._index` 读取。

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

`this._index` 表达式（§3.3 `GetIndex`）直接读 `ctx._index`，None 时返回 0（对齐
Python `context.get("_index", None)` 在表达式上下文中的行为，详见 §3.3 边界）。

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

### 2.6 决策 A6：PrefixedArray 用独立 Node（不依赖未实现的 FocusedSeq/Rebuild）

Python `PrefixedArray` 是宏：`FocusedSeq("items", Rebuild(countfield, len_(this.items)), subcon[this.count])`。

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
- 子 Struct 字段表达式 `this._index` 应直接读到外层 Array 的下标（Python 行为）
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

### 3.3 ExprOp 新增 `GetIndex`

支持 `this._index` 表达式（如 `Array(3, Computed(this._index + 1))`）。

#### 3.3.1 指令定义

```rust
pub enum ExprOp {
    // ... 现有 17 个变体 ...
    /// Phase 4：从 ctx 读取当前数组迭代下标，压入栈顶。
    /// 执行：`ctx.index().unwrap_or(0) as i64` → `stack.push(i64)`。
    /// 不在数组中时返回 0（对齐 Python `context.get("_index", 0)` 在 int 上下文的行为）。
    GetIndex,
}
```

#### 3.3.2 边界：`_index` 为 None 时的返回值

Python `Index._parse` 返回 `context.get("_index", None)`（None）；
`Computed(this._index + 1)` 中 `this._index` 若为 None，Python 会因 `None + 1`
抛 TypeError。

construct-rs 的处理：
- `ExprOp::GetIndex` 在 `ctx.index() == None` 时返回 `0`（避免 TypeError）。
- `IndexNode.parse` 在 `ctx.index() == None` 时返回 `Py_None`（对齐 Python Index 类）。

**差异说明**：`Computed(this._index + 1)` 在 construct-rs 中若不在数组内会得到 `1`
（Python 抛 TypeError）。这是已知行为差异，文档标注（§9.5 IX-1）。

> **替代方案**（DEV 可选）：`GetIndex` 在 None 时返回 `ExprFieldMissing` 错误，
> 严格对齐 Python TypeError。但代价是合法用法（如 IndexNode 内部读 _index）
> 也需要错误处理路径。权衡后选择"返回 0"的宽松语义。

#### 3.3.3 编译期翻译

Python 侧编译器（`_compile_expr_tree`）识别 `this._index` 表达式节点，翻译为
`ExprOp::GetIndex` 元组 `("getindex",)`。Rust 侧 `parse_expr_ops_from_py` 增加
`"getindex"` 分支。

#### 3.3.4 `compute_max_stack` 更新

`GetIndex` 与 `GetInt` / `Const` 同属"压栈"指令，栈深 +1：

```rust
ExprOp::GetInt(_) | ExprOp::Const(_) | ExprOp::GetIndex => {
    depth += 1;
    // ...
}
```

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

> **GIL 注意**：sizeof 接收 `&Context` 而非 `py: Python`（现有签名）。
> 若 count 是表达式，需获取 GIL 调 `eval_expr_int`。`Python::with_gil` 在已持有 GIL
> 时是 O(1)（仅获取 token），可接受。**替代**：若 DEV 发现此路径有性能问题，
> 可修改 Construct trait 的 sizeof 签名增加 `py: Python<'_>` 参数（影响全部节点，
> 需 PM 评估）。当前 sizeof 路径少且非热路径，保留 with_gil 调用。

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

        // 调用谓词：predicate(elem, list, context_proxy)
        // Python 签名：(obj, list, context) -> bool
        let stop = call_repeat_predicate(py, predicate, &elem, list.as_any(), ctx)?;
        if stop {
            break;      // RU-2: 谓词为真时终止（最后元素被包含）
        }

        i += 1;
    }

    match old_index { Some(idx) => ctx.set_index(idx), None => ctx.clear_index() }
    Ok(list.into_any())
}
```

**call_repeat_predicate 辅助函数**：

```rust
/// 调用 RepeatUntil 谓词，返回是否应终止。
/// 对齐 Python `predicate(obj, list, context)`。
fn call_repeat_predicate(
    py: Python<'_>,
    predicate: &Bound<'_, PyAny>,
    elem: &Py<PyAny>,
    list: &Bound<'_, PyAny>,
    ctx: &Context<'_>,
) -> Result<bool, ConstructError> {
    // 构造 context proxy 传给 Python 谓词。
    // 简化：复用现有 ctx.fields() 或构造临时 dict。
    // 性能注意：每次迭代构造 context 是开销来源（~200-500ns）。
    let ctx_proxy = build_context_proxy(py, ctx)?;
    let result = predicate.call1((elem.bind(py), list, &ctx_proxy))?;
    let truthy: bool = result.is_truthy()?;
    Ok(truthy)
}
```

> **`build_context_proxy` 设计选择**（DEV 决策）：
> - 简单方案：每次构造新 PyDict，从 ctx.fields() 复制字段 + 写 _index。
>   开销 ~500ns/次，N 次迭代 = N×500ns。
> - 复用方案：在 ctx 中维护一个长生命周期的 PyDict proxy，每次更新 _index。
>   复杂度高，需管理引用计数。
>
> Phase 4 首版用简单方案，性能不达标再优化。

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

        partial.append(elem.clone_ref(py)).map_err(ConstructError::from)?;
        let stop = call_repeat_predicate(py, predicate, elem_bound, partial.as_any(), ctx)?;
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

#### 4.4.1 数据结构

```rust
/// 取当前数组迭代下标的节点。
/// 对应 Python construct `Index`（core.py L2934）。
#[derive(Debug)]
pub struct IndexNode;
```

#### 4.4.2 parse / build / sizeof

```rust
impl Construct for IndexNode {
    fn parse<'py>(&self, py: Python<'py>, _stream, ctx, _path) -> Result<Py<PyAny>, ConstructError> {
        // 对齐 Python `context.get("_index", None)`。
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
    /// 表达式（如 `this.x == 0`）。
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
        Node::Index(_) | Node::StopIf(_) => false,   // 不含表达式字段（StopIf 的 Expr 在节点自身）
    }
}
```

`ArrayNode::has_expressions` 等返回 `self.inner.has_expressions() || self.count.is_expr()`
（count 为 Expr 时也算表达式）。

> **注意**：`StopIf(Expr)` 节点自身持有表达式程序，但其表达式不引用 Struct 字段
> （引用的是当前 context 中的值，由外层 Struct 提供）。`has_expressions` 用于决定
> StructNode 是否创建带 PyDict 的 context——若 Struct 含 StopIf(Expr)，应返回 true。
> 此判断在 StructNode 编译期通过 `expr_programs` 参数确定（已有机制），
> `StopIf(Expr).has_expressions()` 返回 false 不影响（StructNode 看的是字段级别的
> expr_programs）。**DEV 实现时验证此路径**。

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

```rust
fn build_array_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
) -> Result<ArrayNode, ConstructError> {
    // 1. 递归编译 subcon。
    let subcon_desc = desc.getattr("subcon").map_err(|e| ConstructError::Compilation {
        message: format!("ArrayDescriptor missing 'subcon': {}", e),
    })?;
    let inner_node = build_node_from_descriptor(py, &subcon_desc, field_index, &[], false)?;

    // 2. 解析 count：常量 or 表达式。
    let count_desc = desc.getattr("count").map_err(|e| ...)?;
    let count = if let Ok(n) = count_desc.extract::<i64>() {
        if n < 0 {
            return Err(ConstructError::Compilation {
                message: format!("Array count {} must be non-negative", n),
            });
        }
        CountSource::Const(n as usize)
    } else {
        // 表达式 count：从 expr_programs 取（需调用方传入，或从 desc 提取）
        // 简化：Phase 4 假设 compile_schema 已传入 expr_programs，
        //       此处从 expr_programs[field_index]["count"] 取。
        //       此处签名需调整，传 expr_programs 进来。
        // 暂用 placeholder，DEV 实现时细化。
        return Err(ConstructError::Compilation {
            message: "Array expression count not yet supported in build_array_node".to_string(),
        });
    };

    // 3. discard 标志。
    let discard: bool = desc.getattr("discard")?.extract().unwrap_or(false);

    Ok(ArrayNode::new(inner_node, count, discard))
}
```

> **expr_programs 传递**：当前 `build_node_from_descriptor` 接收 `expr_programs` 切片。
> Array 的 count 表达式需从 `expr_programs[field_index]` 取。但 Array 是 subcon，
> 其 expr_programs 应在递归调用时正确传递。**DEV 实现时**：
> - 若 Array 是 Struct 字段的 subcon，`field_index` 是 Struct 字段索引，
>   `expr_programs[field_index]` 是该字段的 count 表达式。
> - 若 Array 嵌套（Array 内 Array），内层 Array 的 count 表达式存储位置需设计。
>   Python 侧 `_compile_expr_tree` 已处理嵌套（每个 subcon 独立编译）。
>
> **简化首版**：仅支持 Array count 为编译期常量（`CountSource::Const`）。
> 表达式 count 延后到 Phase 4 后续子任务（与 Bytes 表达式长度同样路径）。
> PM 在子任务拆分时区分 4.0a（常量 count）与 4.0b（表达式 count）。

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

Python construct 的 `len_(this.items)` 用于 PrefixedArray 的 Rebuild。
本设计 PrefixedArray 不依赖 Rebuild（§5），故 `len_` **不在 Phase 4 实现**。
若用户代码引用 `len_`，文档提示用 PrefixedArray 节点替代手动组合。

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
| AR-10 | inner 是表达式长度 Bytes | 表达式从 ctx 取值（_index 可用） |

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
| RU-7 | discard=True | 仍调谓词（用空 list），不收集元素 |

### 7.4 PrefixedArray 边界

| ID | 场景 | 预期行为 |
|----|------|---------|
| PA-1 | countfield 解析的 count 为负 | 返回 `Range` 错误 |
| PA-2 | countfield 解析的 count 不是整数 | 返回 `Range` 错误 |
| PA-3 | count = 0 | 返回空 list（countfield 已消耗） |
| PA-4 | count 大于实际可解析元素 | 第 N 个元素失败时错误向上传播 |
| PA-5 | build 时 list 长度 > countfield 容量 | 取决于 countfield 实现（如 Byte 溢出回绕） |
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

| 场景 | 预测加速比 | 验证方法 |
|------|-----------|---------|
| Array(100, Byte) parse | ≥15x | timeit, N=1000, 取 min |
| Array(100, Byte) build | ≥12x | timeit, N=1000 |
| Array(100, Struct{2 fields}) parse | ≥8x | Struct 内层有更多 C API 开销 |
| GreedyRange(Byte) parse 100 元素 | ≥8x | seek 开销 + 动态长度 |
| PrefixedArray(Byte, Byte) parse 100 元素 | ≥10x | 等同 Array + 1 次 countfield |
| RepeatUntil(lambda x,l,c: x>50, Byte) parse | ≥3x | PyCallable 路径慢，Expr 路径补全后 ≥8x |
| Index（在 Array 内） | ≥20x | 仅读 ctx 字段 |

**证伪条件**：
- 若 Array(100, Byte) parse 加速比 < 10x，视为性能未达标，需排查：
  - PyList::new_bound 预分配是否生效
  - path.push_index/pop 是否成为瓶颈（若是，改用 P0-3 风格错误路径重建）
  - ctx.set_index 是否被错误地走 PyDict 路径
- 若加速比 < 4x（S-PERF 硬目标），视为架构失败，回退设计。

### 8.3 验证脚本设计

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

### 8.4 性能假设的关键依赖

1. **PyList::new_bound + Vec 预分配**：消除 append 循环的 realloc。
   - 若 PyList 内部仍多次 realloc（容量预测不准），加速比下降。
   - 验证：用 `PyList::new_bound(py, Vec::with_capacity(count))` 而非空 List + append。
2. **ctx._index 栈字段**：不走 PyDict。
   - 若误用 `ctx.set_field("_index", i)`，加速比降 5-10x。
   - 验证：profile 检查 PyDict_SetItem 调用次数（应为字段数，非字段数×N）。
3. **path push/pop 开销**：当前每次迭代调用。
   - 若 path.push_index 的 Vec::push 成本 ~10ns 累积过大（N=100 时 1μs），可改为
     P0-3 风格（成功路径不维护，错误时重建）。首版保留 push/pop，性能不达标再优化。
4. **RepeatUntil 的 PyCallable 路径**：每次迭代跨 FFI。
   - N=100 时 100 次 Python 调用 = ~50-100μs，仅 3-5x 加速。
   - Expr 路径补全后消除 FFI，预期 ≥8x。

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

| Python 表达式 | Rust ExprOp | 备注 |
|--------------|-------------|------|
| `this._index` | `GetIndex` | 新增（§3.3） |
| `this.<field>` | `GetInt(idx)` | 现有 |
| 算术/位/比较 | `Add` / `Sub` / ... / `Gt` / ... | 现有 |

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
✅ 表达式映射完整（§9.4）

### 11.2 边界条件完整性

✅ Array 边界 10 条（§7.1）
✅ GreedyRange 边界 9 条（§7.2）
✅ RepeatUntil 边界 7 条（§7.3）
✅ PrefixedArray 边界 6 条（§7.4）
✅ Index 边界 4 条（§7.5）
✅ StopIf 边界 7 条（§7.6）
✅ ListContainer 兼容性 4 条（§7.7）
✅ 嵌套与组合 7 条（§7.8）

### 11.3 与现有架构的兼容性

✅ 不修改 Construct trait 签名（13 个现有节点无需改动）
✅ 不修改现有 Node 变体的行为
✅ 不修改 ParseStream/BuildStream 的现有 API（仅新增 seek）
✅ Context 新增字段不影响现有构造函数语义（_index 默认 None）
✅ compile_schema 向后兼容（新增描述符识别分支）
✅ StructNode 仅增加 StopField 捕获分支（不破坏现有行为）

### 11.4 性能假设的瓶颈覆盖

✅ FFI 调用（仅 RepeatUntil PyCallable 路径有，其他无）
✅ PyList 创建与 append（§8.1）
✅ ctx._index 写入（§8.1）
✅ path push/pop（§8.1）
✅ 子节点 parse 分派（§8.1）

### 11.5 编码红线遵守

✅ 无 `unwrap()` / `expect()` 在非测试代码（伪代码中的 unwrap 仅为示意，DEV 实现时
   用 `?` 或 match）
✅ 无 panic（错误返回 Err）
✅ 无 TODO/FIXME（设计文档中无）
✅ 无硬编码魔法数字（常量在模块顶部定义）
✅ 所有 pub 项有 `///` 文档注释（DEV 实现时补全）
✅ 错误携带 path（所有新增错误变体都有 path 字段）
✅ parse/build 对称性（所有 Array 节点同时支持 parse 和 build）

---

## 12. 遗留问题与 PM 决策点

### 12.1 PM 决策点 1：RepeatUntil 谓词路径范围

**问题**：RepeatUntil 的 Expr 谓词路径（§4.3.2）是否在 Phase 4 实现？

**建议**：Phase 4 首版仅实现 PyCallable 路径（功能完整），Expr 路径作为后续优化。
理由：
- PyCallable 路径行为正确，S-FUNC 可达成。
- Expr 路径需要 `_current_elem` 槽位 + `ExprOp::GetElem` + Python 编译器识别，
  工作量约 1-2 子任务。
- S-PERF 在 Array/GreedyRange/PrefixedArray 上可达成 ≥10x，RepeatUntil 单独
  可能仅 3-5x。若 PM 接受 RepeatUntil 性能延后，首版不实现 Expr。

**PM 行动**：在子任务拆分时明确 4.0a（PyCallable）与 4.0b（Expr）的范围。

### 12.2 PM 决策点 2：Array count 表达式支持范围

**问题**：Array 的 count 表达式（如 `Array(this.length, Byte)`）是否在 Phase 4 实现？

**建议**：与 Bytes 表达式长度（已实现）走相同路径，**应一并实现**。
理由：
- ExprProgram 基础设施已具备。
- CountSource::Expr 变体已设计。
- 不实现会显著限制 Array 的实用性（用户无法用 `Array(n, Byte)` 引用前序字段）。

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
- 性能分析后再决策（§8.4 关键依赖 3）。

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
✅ 边界条件覆盖完整
✅ 遗留问题已明确（6 个 PM 决策点）

**建议 PM 在子任务拆分时**：
1. 4.0a：基础设施（ParseStream.seek、Context._index、ExprOp::GetIndex、错误变体、
   Python 异常类映射）—— 这是所有 Array 节点的前置依赖
2. 4.1：ArrayNode（P0）
3. 4.2：GreedyRangeNode（P1）
4. 3：PrefixedArrayNode（P2）
5. 4.4：IndexNode（P4）+ StopIfNode（P5）+ StructNode 的 StopField 捕获修改
6. 4.5：RepeatUntilNode（P3，首版仅 PyCallable）
7. 4.6：S-FUNC 验证 + S-PERF 基准

子任务可合并（如 4.1+4.2 一次 DEV 编码），由 PM 决定。

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



