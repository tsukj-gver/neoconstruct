# 模块设计：FFI 执行层（direct-to-Python + PyCallback + PyContextView）

> **文档性质**：模块设计（ARCH 产出）
> **日期**：2026-06-16
> **版本**：v2.1（修正 REV v2 复检的 I-INT-2：§2.1.4 节点归类错误）
> **状态**：DESIGNING（v2.1 修正完成，待 REV 复检）
> **覆盖子任务**：Phase 13 — FFI 执行层重写
>
> **v2 修正记录**：
> - B-FEAS-1（阻塞）：Input trait 新增 `as_value()` 方法 + 三类节点 exec_build 适配模式（§3.2/§3.3）
> - B-FEAS-2（阻塞）：CompiledExtension::ext_build 签名统一为 `&dyn Input`（§5.2/§5.3）
> - I-INT-1（重要）：OutputSink 新增 `finish_named_item` 方法，覆盖 Sequence named entry（§2.1.2）
> - I-FEAS-1（重要）：parse_bytes_py/build_from_py 改为 CompiledSchemaHolder 方法，规避孤儿规则（§2.3/§3.5）
> - I-COMP-1（重要）：PyContextView 从 raw pointer 改为 owned Context 快照，消除 use-after-free（§4.3）
> - I-INT-2（重要）：§2.1.4 移除 FocusedSeq/Select 的 finish_* 错误归类——两者为标量输出节点（set_scalar），非 child 整合模式
>
> **前置文档**：
> - `docs/refactor/顶层架构设计.md` — §2 数据流向图、§3.2 D2（Value 退出 Python 路径）、
>   §3.4 D4（惰性 Context）、§3.6 D6（per-tree FFI）、§8.2 验收标准
> - `docs/refactor/模块设计-编译执行树.md` — Phase 12 产物（CompiledNode / OutputSink /
>   SchemaCompiler / BuildConstruct）。本设计是其**直接延续**。
> - `construct-rs/src/compiled/mod.rs` — CompiledNode（71 variant）+ CompiledExec trait
> - `construct-rs/src/compiled/sink.rs` — OutputSink trait + ValueSink（Phase 12 纯 Rust 路径）
> - `construct-rs/src/compiled/expr.rs` — CompiledExpr（Native / Constant variant）
>
> **设计目标**：在 Phase 12 编译执行树（纯 Rust 路径，parse 返回 Value）之上，构建
> **direct-to-Python** 路径——parse 直接产出 `PyDict`（不经 Value::Container 递归中转），
> build 按需读 PyObject（不预转换整棵树）。同时引入 `PyCallback` 统一封装（执行树内唯一
> FFI 点）和 `PyContextView`（惰性 Context 映射），将 per-operation FFI 收敛为 per-tree FFI。

---

## 1. 概述与目标

### 1.1 Phase 12 → Phase 13 的衔接

Phase 12 完成了编译执行树的**基础设施**：

| Phase 12 产物 | Phase 13 在其上的工作 |
|--------------|---------------------|
| `CompiledNode`（71 variant，纯 Rust） | 新增 `External` variant（容纳 PyCallback，不依赖 pyo3） |
| `CompiledExec` trait（exec_parse/exec_build/exec_sizeof） | 不变，External variant 通过 trait object 转发 |
| `OutputSink` trait + `ValueSink` | **扩展** trait（新增 `into_produced`/`finish_field`/`finish_item`），新增 `PyDictSink` |
| `CompiledExpr`（Native/Constant） | 新增 `External` variant（容纳 Python callable 表达式） |
| `SchemaCompiler`（遍历 CombinedConstruct） | 不变；construct-py 层增加 PyCallback 检测 + 缓存触发 |
| `CompiledSchema::parse_bytes`（返回 Value） | **新增** `CompiledSchemaHolder::parse_bytes_py`（返回 PyObject）+ `build_from_py`（construct-py wrapper，I-FEAS-1） |

**关键不变量**：Phase 12 的纯 Rust 路径（`parse_bytes` 返回 Value）**完全不受影响**。
Phase 13 的新路径（`parse_bytes_py` 返回 PyObject）是 construct-py crate 的**增量**。

### 1.2 Phase 13 范围（明确边界）

| 在范围内 | 不在范围（Phase 14+） |
|---------|---------------------|
| `OutputSink` 协议扩展（ProducedOutput / finish_field / finish_item） | PyLazyProxy 完整延迟解析（Phase 13 先退化为立即解析） |
| `PyDictSink` 实现（direct-to-PyDict） | dataclass-first direct-to-dataclass（Phase 14） |
| `parse_bytes_py` / `build_from_py`（construct-py） | 构造器级别的 PyObject 内联优化 |
| `PyContextView`（惰性 Mapping 协议） | 字段引用的编译期死代码消除（跳过无用 context.insert） |
| `PyCallback` 节点（External variant + CompiledExtension trait） | 嵌套 Struct 的 PyObject→Value 反转消除 |
| `Input` trait + `PyInput`（build 方向惰性读 PyObject） | 性能基准与回归测试（Phase 14） |
| `CompiledExpr::External`（Python callable 表达式） | |
| construct-py 4 文件重写（conversions/expr_bridge/py_adapter/api） | |
| SchemaCompiler 衔接（编译触发 + 缓存） | |

### 1.3 核心设计决策速览

| 决策 | 选择 | 理由（详见对应章节） |
|------|------|---------------------|
| PyCallback variant 方案 | **方案 B（External + CompiledExtension trait）** | §5：construct-rs 无 pyo3；trait object 透明转发；零重复执行逻辑 |
| PyContextView FFI 开销 | **常见模式零 FFI（Rust 闭包）；退化模式 ~300ns/次** | §4：表达式编译覆盖率决定 |
| Lazy direct-to-Python | **PyLazyProxy 方案；初始退化为立即解析** | §5.4：延迟语义完整设计，实现分期 |
| 错误路径（REV #9） | **方案 A（丢弃部分 PyDict）** | §8：与 Python 原版一致，性能最优 |
| OutputSink 扩展 | **ProducedOutput + finish_field/finish_item** | §2.1：打破 Value 中转，支持 PyObject 直传 |

---

## 2. direct-to-Python parse（PyDictSink + parse_bytes_py）

### 2.1 OutputSink 协议扩展（前置改动）

**问题根源**（基于 `compiled/mod.rs:1581-1637` 的源码事实）：

`CompiledStruct::exec_parse` 通过 helper `exec_parse_named_child` 整合子结果：
```rust
// 当前 Phase 12 实现（mod.rs:1581-1591）
fn exec_parse_named_child(..., parent_sink, name) -> Result<Value> {
    let mut sub_sink = parent_sink.sub_sink_for_field(name)?;
    node.exec_parse(stream, ctx, sub_sink.as_mut())?;
    sub_sink.into_value()   // ← 提取 Value
}
// 调用方（mod.rs:1607-1620）：
let value = exec_parse_named_child(...)?;
sink.set_field(name, value.clone())?;   // ← 再写回 sink
child_ctx.insert(name.clone(), value);
```

子结果以 **Value** 在父子 sink 间传递（`into_value` → `set_field`）。对于 PyDictSink，
这会导致 `PyDict → Value`（into_value）→ `Value → PyDict`（set_field）的**往返转换**。

**解决方案**：扩展 OutputSink trait，让父子间结果传递**可绕过 Value**。

#### 2.1.1 ProducedOutput enum

新增一个表示 sink 最终产出的枚举。定义在 `construct-rs/src/compiled/sink.rs`：

```rust
/// A sink's final produced output after `exec_parse` completes.
///
/// This enum allows sinks to produce either a Rust [`Value`] (the Phase 12
/// pure-Rust path via [`ValueSink`]) or an opaque boxed object (the Phase 13
/// direct-to-Python path via `PyDictSink`). The opaque variant uses
/// `Box<dyn Any + Send + Sync>` so that construct-rs does **not** depend on
/// pyo3 — the actual `Py<PyDict>` / `Py<PyList>` is only downcastable inside
/// construct-py.
pub enum ProducedOutput {
    /// Value-producing sink (ValueSink path). The standard Phase 12 output.
    Value(Value),
    /// Object-producing sink (PyDictSink path). Contains a `Py<PyAny>` boxed
    /// as `dyn Any + Send + Sync`. `Py<PyAny>` is `Send + Sync` (pyo3 guarantee),
    /// so the bound is satisfied. Only construct-py can downcast this.
    Object(Box<dyn std::any::Any + Send + Sync>),
}
```

> **为何 `Box<dyn Any + Send + Sync>` 而非 `Box<dyn Any>`**：`CompiledSchema` 可能在多线程
> 上下文缓存（`OnceLock`），产出对象需要跨线程传递。`Py<PyAny>` 是 Send + Sync 的（pyo3
> 0.22），所以 bound 不排斥 Python 对象。

#### 2.1.2 OutputSink trait 扩展

```rust
pub trait OutputSink {
    // ── 现有方法保留（向后兼容）──
    fn set_scalar(&mut self, value: Value) -> Result<()>;
    fn set_field(&mut self, name: &str, value: Value) -> Result<()>;
    fn push_item(&mut self, value: Value) -> Result<()>;
    fn sub_sink_for_field(&mut self, name: &str) -> Result<Box<dyn OutputSink>>;
    fn sub_sink_for_item(&mut self) -> Result<Box<dyn OutputSink>>;
    fn finish_subsink(&mut self, sub: Box<dyn OutputSink>) -> Result<()>;
    fn into_value(self: Box<Self>) -> Result<Value>;

    // ── Phase 13 新增 ──

    /// Consumes the sink and returns its produced output.
    ///
    /// - `ValueSink` returns `ProducedOutput::Value` (wraps `into_value`).
    /// - `PyDictSink` returns `ProducedOutput::Object` (boxed `Py<PyAny>`).
    ///
    /// # Errors
    ///
    /// Propagates any conversion error.
    fn into_produced(self: Box<Self>) -> Result<ProducedOutput>;

    /// Finishes a **named Struct field** sub-sink: integrates its result into
    /// `self` via `set_field` (writes by name to a Container/PyDict) **and**
    /// returns the child's [`Value`] for context insertion.
    ///
    /// **Used by**: `CompiledStruct`, `CompiledUnion` —
    /// constructs whose output is a named-keyed container (dict).
    ///
    /// **Default implementation** (ValueSink-compatible): calls `into_value`
    /// + `set_field`. This preserves Phase 12 behavior exactly.
    ///
    /// **PyDictSink override**: extracts the child's `ProducedOutput`,
    /// converts to `PyObject` (if `Value`) or downcasts (if `Object`), writes
    /// directly to the parent `PyDict` via `set_item(name, py_obj)`, and
    /// returns the `Value` (converting `PyObject → Value` via `py_to_value`
    /// for context use).
    ///
    /// # Errors
    ///
    /// Propagates any sink/conversion error.
    fn finish_field(
        &mut self,
        name: &str,
        sub: Box<dyn OutputSink>,
    ) -> Result<Value> {
        // Default: Phase 12 behavior (Value round-trip).
        let value = sub.into_value()?;
        self.set_field(name, value.clone())?;
        Ok(value)
    }

    /// Finishes a **named Sequence entry** sub-sink: integrates its result
    /// into `self` via `push_item` (appends positionally to a List/PyList)
    /// **and** returns the child's [`Value`] for context insertion by name.
    ///
    /// **Used by**: `CompiledSequence` — Sequence produces a positional
    /// list (`ListContainer` / Python `list`), but entries may have names
    /// for context reference (e.g. `this.entry_name` in expressions).
    /// This is distinct from [`finish_field`](Self::finish_field) which
    /// writes by name to a Container.
    ///
    /// **Default implementation**: calls `into_value` + `push_item`.
    /// Returns the Value for the caller to insert into context by name.
    ///
    /// **PyDictSink override**: appends the child's PyObject to the parent
    /// `PyList` via `append`, returns the Value for context.
    ///
    /// # Errors
    ///
    /// Propagates any sink/conversion error.
    fn finish_named_item(
        &mut self,
        _name: &str,
        sub: Box<dyn OutputSink>,
    ) -> Result<Value> {
        // Default: Phase 12 behavior — positional append, name ignored
        // (name is only needed for context insertion by the caller).
        let value = sub.into_value()?;
        self.push_item(value.clone())?;
        Ok(value)
    }

    /// Finishes an **anonymous** child sub-sink (Array items, anonymous
    /// Sequence/Struct fields).
    ///
    /// **Default**: delegates to `finish_subsink` (push as item, no name).
    fn finish_item(&mut self, sub: Box<dyn OutputSink>) -> Result<()> {
        self.finish_subsink(sub)
    }
}
```

> **三种 finish 方法的语义区分**（I-INT-1 修正）：
>
> | 方法 | 写入语义 | 返回值 | 适用节点 |
> |------|---------|--------|---------|
> | `finish_field(name, sub)` | `set_field(name, v)` — 按名字写 Container | `Value`（给 ctx） | Struct, Union |
> | `finish_named_item(name, sub)` | `push_item(v)` — 按顺序追加 List | `Value`（给 ctx） | Sequence（named entries） |
> | `finish_item(sub)` | `push_item(v)` — 按顺序追加 List | `()`（不需要） | Array, anonymous entries |
>
> **为什么 Sequence 用 `finish_named_item` 而非 `finish_field`**：
> Sequence 在 Python 原版中产出 `ListContainer`（位置列表），即使 entry 有名字。
> 名字仅用于 context 引用（`this.entry_name`），不影响输出结构。若误用
> `finish_field`（set_field），会将 Sequence 结果错误地写成 dict（Container），
> 破坏 round-trip 一致性。v1 文档仅定义了 `finish_field`，Sequence 的 named
> entry 被遗漏——v2 新增 `finish_named_item` 修正此缺口。
>
> **`finish_field` 返回 `Value` 的原因**：`CompiledStruct::exec_parse` 需要同时
> 更新 sink（输出）和 `child_ctx`（表达式求值用）。context 是 Rust 内部的
> `IndexMap<String, Value>`，永远用 `Value`。所以 `finish_field` 必须返回
> `Value` 给 context，即使 sink 路径用了 `PyObject`。对于 PyDictSink，复合子节点
> 的 `PyObject → Value` 反转在此发生（仅当该字段被 context 引用时；编译期优化
> 可跳过，见 §2.4）。

#### 2.1.3 ValueSink 适配（零行为变化）

```rust
impl OutputSink for ValueSink {
    // ... 所有现有方法不变 ...

    fn into_produced(self: Box<Self>) -> Result<ProducedOutput> {
        self.into_value().map(ProducedOutput::Value)
    }
    // finish_field / finish_item 使用默认实现（= Phase 12 行为）
}
```

#### 2.1.4 复合节点 exec_parse 改动（机械化）

**Struct 模式**（named field → `finish_field`）：

`exec_parse_named_child` helper 改为调用 `finish_field`：

```rust
// 改动后（mod.rs）：
fn exec_parse_named_child(
    node: &CompiledNode, stream: &mut CombinedStream, ctx: &mut Context,
    parent_sink: &mut dyn OutputSink, name: &str,
) -> Result<Value> {
    let mut sub_sink = parent_sink.sub_sink_for_field(name)?;
    node.exec_parse(stream, ctx, sub_sink.as_mut())?;
    parent_sink.finish_field(name, sub_sink)  // set_field + 返回 Value
}
```

`CompiledStruct::exec_parse` 循环简化（删除手动 `set_field`）：
```rust
// 改动后：
for field in &self.fields {
    match &field.name {
        Some(name) => {
            let value = exec_parse_named_child(&field.subcon, stream,
                &mut child_ctx, sink, name)?;
            child_ctx.insert(name.clone(), value);  // sink 已由 finish_field 整合
        }
        None => {
            let mut sub = sink.sub_sink_for_item()?;
            field.subcon.exec_parse(stream, &mut child_ctx, sub.as_mut())?;
            sink.finish_item(sub)?;
        }
    }
}
```

**Sequence 模式**（named entry → `finish_named_item`，I-INT-1 修正）：

Sequence 的 entry 即使有名字，也是位置追加到 List（非 set_field）。
`CompiledSequence::exec_parse` 使用 `finish_named_item`（而非 `finish_field`）：

```rust
// CompiledSequence::exec_parse 改动后：
for entry in &self.entries {
    match &entry.name {
        Some(name) => {
            // Named entry: positionally appended (push_item), name for ctx
            let mut sub = sink.sub_sink_for_item()?;
            entry.subcon.exec_parse(stream, &mut child_ctx, sub.as_mut())?;
            let value = sink.finish_named_item(name, sub)?;  // push_item + 返回 Value
            child_ctx.insert(name.clone(), value);            // context 按名字引用
        }
        None => {
            // Anonymous entry: positionally appended, no context insert
            let mut sub = sink.sub_sink_for_item()?;
            entry.subcon.exec_parse(stream, &mut child_ctx, sub.as_mut())?;
            sink.finish_item(sub)?;
        }
    }
}
```

> **Sequence vs Struct 的关键区别**：Struct 用 `finish_field`（set_field，dict 语义），
> Sequence 用 `finish_named_item`（push_item，list 语义）。混淆会导致 PyDictSink
> 将 Sequence 错误地写成 dict。v1 文档未区分两者，v2 通过新增 `finish_named_item`
> 修正。

**Array 模式**（anonymous item → `finish_item`）：

```rust
// CompiledArray::exec_parse 改动后：
for _ in 0..self.count {
    let mut sub = sink.sub_sink_for_item()?;
    self.subcon.exec_parse(stream, &mut child_ctx, sub.as_mut())?;
    sink.finish_item(sub)?;  // push_item，无名字，无 context 返回值
}
```

**影响范围**：`CompiledStruct`（finish_field）、`CompiledSequence`（finish_named_item）、
`CompiledUnion`（finish_field）、
`CompiledArray`/`CompiledArrayExpr`/`CompiledGreedyRange`/`CompiledRepeatUntil`
（finish_item）。
改动是机械的（`into_value + set_field/push_item` → 对应 finish 方法），不改变语义。
ValueSink 路径所有 Phase 12 测试保持 PASS（默认实现等价）。

> **非整合节点说明（I-INT-2 修正）**：`CompiledFocusedSeq` 和 `CompiledSelect`
> **不在**上述影响范围内——它们保留现有的 `sub_sink + into_value + set_scalar`
> 模式，**不**改为 finish_* 调用：
>
> - **FocusedSeq**（`mod.rs:2402-2453`）：逐字段解析子构造器，提取 Value 进 context
>   （用于表达式求值），但最终输出是 `sink.set_scalar(focused_value)`（line 2447）——
>   即被聚焦字段的标量值。节点产出**标量**，不是 Container。
> - **Select**（`mod.rs:2147-2180`）：逐候选构造器尝试解析，命中后
>   `sub_sink.into_value() → sink.set_scalar(matched_value)`（line 2159-2160）。
>   节点产出**标量**。
>
> **若误归类为 finish_* 的后果**：
> - FocusedSeq 在 PyDictSink 下，named children 的 finish_field 会将 sink 推入
>   Struct 模式（dict），随后 `set_scalar` 在 Struct 模式下直接 Err
>   （见 §2.2 方法表：set_scalar × Struct = Err）。
> - Select 若走 finish_item，输出从标量变成 List（push_item），破坏 parse/build 对称性
>   （build 方向 Select 消费标量 `&Value`，而非 List）。
>
> **实现指引**：FocusedSeq 的 named children 解析**不应**使用改动后的
> `exec_parse_named_child` helper（该 helper 现内含 `finish_field`→`set_field`），
> 而应内联 `sub_sink_for_field + exec_parse + into_value`（仅提取 Value 进 context，
> 不向 sink 写入），最终由 `set_scalar(focused_value)` 统一输出。Select 同理，保持
> `sub_sink_for_item + exec_parse + into_value + set_scalar` 原样不变。

### 2.2 PyDictSink 实现

定义在 `construct-py/src/py_sink.rs`（新增文件）。

```rust
//! Direct-to-Python output sink — writes parse results straight into a
//! `PyDict` / `PyList`, avoiding intermediate `Value::Container` trees.

use construct::compiled::sink::{OutputSink, ProducedOutput};
use construct::core::error::{ConstructError, Result};
use construct::value::Value;

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

/// `OutputSink` that writes directly to Python `dict` / `list` objects.
///
/// This is the Phase 13 direct-to-Python sink. It **never** accumulates a
/// `Value::Container` tree — composite fields are assembled via
/// `PyDict::set_item` / `PyList::append`, and leaf scalars are converted to
/// `PyObject` immediately in `set_scalar`.
pub struct PyDictSink {
    mode: PySinkMode,
}

enum PySinkMode {
    /// Nothing written yet; transitions on first write.
    Empty,
    /// Leaf: holds a single scalar `Value` (deposited by `set_scalar`).
    /// `into_produced` returns `ProducedOutput::Value` — the caller
    /// (parent's `finish_field`) converts to `PyObject`.
    Leaf(Option<Value>),
    /// Struct: owns a `PyDict`. `set_field` converts `Value → PyObject`
    /// immediately and calls `PyDict::set_item`. No `Value::Container`
    /// accumulation. `into_produced` returns `ProducedOutput::Object`.
    Struct { dict: Py<PyDict> },
    /// Array: owns a `PyList`. `push_item` converts + appends immediately.
    Array { list: Py<PyList> },
}
```

**核心方法行为**：

| 方法 | Empty | Leaf | Struct | Array |
|------|-------|------|--------|-------|
| `set_scalar(v)` | → Leaf(Some(v)) | Leaf(Some(v)) | Err | Err |
| `set_field(n,v)` | → Struct{dict}, set_item | Err | set_item | Err |
| `push_item(v)` | → Array{list}, append | Err | Err | append |
| `into_produced` | Value(None) | Value(v) | Object(dict) | Object(list) |
| `finish_field` | — | Value→Py→set_item | Object→set_item | — |
| `finish_named_item` | — | Value→Py→append | — | Object→append |
| `finish_item` | — | Value→Py→append | — | Object→append |

**set_scalar / set_field / push_item 实现要点**：

- `set_scalar(value)`：暂存 `Value`（**不立即转 PyObject**）。原因：叶子节点的标量值需要
  同时给 context（Value）和 sink（PyObject）。暂存 Value 让 `finish_field` 决定如何转换，
  避免重复转换。
- `set_field(name, value)`：在持有 GIL 的前提下 `value_to_py(py, &value)` →
  `dict.set_item(name, py_obj)`。**单字段标量转换**，非递归。
- `push_item(value)`：同理，`value_to_py` + `list.append(py_obj)`。

**finish_field 实现**（PyDictSink 核心，打破 Value 往返）：

```rust
fn finish_field(&mut self, name: &str, sub: Box<dyn OutputSink>) -> Result<Value> {
    let produced = sub.into_produced()?;
    Python::with_gil(|py| {
        let (value_for_ctx, py_obj) = match produced {
            ProducedOutput::Value(v) => {
                // 叶子子节点：v 是标量 Value。转 PyObject，v 原样给 context。
                let py_obj = value_to_py(py, &v).map_err(py_err_to_construct)?;
                (v, py_obj)
            }
            ProducedOutput::Object(obj) => {
                // 复合子节点：obj 是 Py<PyAny>（子 PyDict/PyList）。
                let py_any = obj.downcast_ref::<Py<PyAny>>()
                    .map_err(|_| ConstructError::Generic {
                        path: String::new(),
                        message: "PyDictSink: failed to downcast Object".into(),
                    })?
                    .clone();
                // PyObject → Value（给 context，仅当字段被表达式引用时实际使用）
                let v = py_to_value(py, py_any.bind(py))
                    .map_err(py_err_to_construct)?;
                (v, py_any)
            }
        };
        // 写入父 PyDict
        match &self.mode {
            PySinkMode::Struct { dict } => {
                dict.bind(py).set_item(name, py_obj).map_err(py_err_to_construct)?;
                Ok(value_for_ctx)
            }
            _ => Err(ConstructError::Generic {
                path: String::new(),
                message: "PyDictSink::finish_field on non-Struct sink".into(),
            }),
        }
    })
}
```

> **复合子节点的 PyObject → Value 反转**（`ProducedOutput::Object` 分支的 `py_to_value`）：
> 这仅在嵌套 Struct/Array 的字段被 context 引用（如 `this.inner.field`）时产生开销。
> 单层 Struct 的叶子字段走 `ProducedOutput::Value` 分支，**无反转**。编译期可分析字段
> 引用，跳过无引用字段的 context.insert + py_to_value（Phase 14 优化，见 §2.4）。

**finish_named_item 实现**（PyDictSink，Sequence named entry，I-INT-1 修正）：

与 `finish_field` 逻辑几乎相同，区别仅在写入父容器的方式：`finish_field` 用
`set_item(name, py_obj)`（dict 按名字写），`finish_named_item` 用 `append(py_obj)`
（list 顺序追加）。

```rust
fn finish_named_item(&mut self, _name: &str, sub: Box<dyn OutputSink>) -> Result<Value> {
    let produced = sub.into_produced()?;
    Python::with_gil(|py| {
        let (value_for_ctx, py_obj) = match produced {
            ProducedOutput::Value(v) => {
                let py_obj = value_to_py(py, &v).map_err(py_err_to_construct)?;
                (v, py_obj)
            }
            ProducedOutput::Object(obj) => {
                let py_any = obj.downcast_ref::<Py<PyAny>>()
                    .map_err(|_| ConstructError::Generic {
                        path: String::new(),
                        message: "PyDictSink: failed to downcast Object".into(),
                    })?
                    .clone();
                let v = py_to_value(py, py_any.bind(py))
                    .map_err(py_err_to_construct)?;
                (v, py_any)
            }
        };
        // 写入父 PyList（顺序追加，区别于 finish_field 的 set_item）
        match &self.mode {
            PySinkMode::Array { list } => {
                list.bind(py).append(py_obj).map_err(py_err_to_construct)?;
                Ok(value_for_ctx)
            }
            _ => Err(ConstructError::Generic {
                path: String::new(),
                message: "PyDictSink::finish_named_item on non-Array sink".into(),
            }),
        }
    })
}
```

### 2.3 parse_bytes_py 方法

定义在 construct-py 的 `CompiledSchemaHolder` wrapper（I-FEAS-1 修正）。

> **I-FEAS-1 修正说明**：v1 使用 `impl CompiledSchema { fn parse_bytes_py }`，
> 但 `CompiledSchema` 定义在 construct-rs crate，在 construct-py 中为其添加
> inherent impl 违反 Rust 孤儿规则（orphan rule：只能在自己 crate 中为外部类型
> 添加 inherent impl）。修正方案：在 construct-py 中定义 wrapper 结构体
> `CompiledSchemaHolder`（持有 `Arc<CompiledSchema>`），将 parse_bytes_py /
> build_from_py 作为 wrapper 的方法。这同时充当编译缓存载体（见 §7.3）。

```rust
// construct-py/src/compiled_ext.rs（新增）
use std::sync::Arc;
use construct::compiled::CompiledSchema;
use construct::core::context::Context;
use construct::core::stream::{ByteStream, CombinedStream};
use construct::value::Value;

/// construct-py wrapper around [`CompiledSchema`] — holds the compiled
/// execution tree and provides Python-facing parse/build entry points.
///
/// This wrapper exists to work around the Rust orphan rule: we cannot add
/// inherent methods to `CompiledSchema` (defined in construct-rs) from
/// construct-py. Instead, `CompiledSchemaHolder` owns an `Arc<CompiledSchema>`
/// and provides `parse_bytes_py` / `build_from_py` as its own methods.
///
/// It also serves as the compilation cache (see §7.3): stored in the PyO3
/// wrapper's `_compiled: OnceLock<Py<CompiledSchemaHolder>>` field.
pub struct CompiledSchemaHolder {
    schema: Arc<CompiledSchema>,
}

impl CompiledSchemaHolder {
    /// Creates a new holder wrapping a compiled schema.
    #[must_use]
    pub fn new(schema: Arc<CompiledSchema>) -> Self {
        Self { schema }
    }

    /// Parses binary data and returns the result as a Python object.
    ///
    /// **Direct-to-Python path** (Phase 13): the entire compiled tree is
    /// traversed in Rust while holding the GIL. Results are written directly
    /// into a `PyDict` / `PyList` via `PyDictSink` — no `Value::Container`
    /// intermediate tree is built.
    ///
    /// # Errors
    ///
    /// Returns `PyErr` (a construct exception subclass) on parse failure.
    /// On error, the partially-built `PyDict` is **dropped** (discarded) —
    /// the Python caller receives only the exception (REV #9, 方案 A, §8).
    pub fn parse_bytes_py(&self, py: Python<'_>, data: &[u8]) -> PyResult<PyObject> {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(data));
        let mut ctx = Context::new();
        ctx.insert("_parsing".into(), Value::Bool(true));
        let mut sink = PyDictSink::new_root(py);  // Struct-mode root, owns PyDict
        match self.schema.tree().exec_parse(&mut stream, &mut ctx, &mut sink) {
            Ok(()) => sink.into_py_object(),       // 提取根 PyDict（非 trait 方法）
            Err(e) => Err(rust_err_to_py(py, e.with_path_prefix(PARSE_PATH))),
        } // sink（PyDict）在 Err 路径被 drop，Python GC 回收
    }
}
```

> **顶层提取**：`parse_bytes_py` 不调用 `into_produced`（trait 方法），而是调用
> `PyDictSink::into_py_object`（inherent 方法），直接返回根 PyDict 的 `PyObject`。这避免
> 了 `Box<dyn Any>` 的装箱/拆箱开销（顶层不经过 ProducedOutput）。

### 2.4 context.insert 优化（前瞻，Phase 14）

`finish_field` 返回的 `Value` 用于 `child_ctx.insert(name, value)`。对于不被任何表达式
引用的字段，这次 insert 是无用功（尤其是复合子节点的 PyObject → Value 反转）。

**Phase 14 优化方向**：SchemaCompiler 在编译期分析字段引用图（哪些字段被 `this.field`
或更深层路径访问）。`CompiledField` 新增 `referenced: bool` 标志。`exec_parse` 中：

```rust
// Phase 14 优化后的 CompiledStruct::exec_parse
let value = exec_parse_named_child(...)?;  // finish_field 总是执行（写 sink）
if field.referenced {
    child_ctx.insert(name, value);  // 仅引用字段才更新 context
}
```

对于 PyDictSink，`finish_field` 可以在 `!field.referenced` 时跳过 `py_to_value` 反转
（Object 分支返回 `Value::None` 占位，因为不会被读）。这消除嵌套 Struct 的反转开销。

Phase 13 不实现此优化（接受嵌套 Struct 的反转，因旧架构的开销远大于此）。

---

## 3. Input trait（build 方向 PyInput）

### 3.1 问题

旧架构 `py_build`（`api.rs:166-193`）的流程：
```rust
let value = py_to_value(py, data)?;   // ← 全量递归 PyObject → Value
constr.build(&value, &mut stream, &mut ctx);
```
`py_to_value` 递归遍历整个 Python dict/list，预先转换为 `Value` 树。对于大对象（如 1000
元素 Array），这是 O(n) 全量转换，即使 build 只需按序读取。

### 3.2 Input trait 设计

借鉴 pydantic-core 的 `Input` trait（`validators/mod.rs` 中 `validate_python` 按需读字段），
定义按需读取的 trait。定义在 `construct-rs/src/compiled/input.rs`（新增）：

```rust
//! Input abstraction for build-direction field reading.
//!
//! During `exec_build`, compiled nodes read field values from an [`Input`]
//! rather than receiving a complete [`Value`] tree. This enables lazy reading:
//! - Pure Rust path: `ValueInput` wraps a `&Value` (zero-cost, reads on demand).
//! - Python path (Phase 13): `PyInput` wraps a `&PyObject`, reading fields
//!   via FFI only when accessed.

use crate::core::error::Result;
use crate::value::Value;

/// A field-value reader for build-direction traversal.
///
/// Each compiled node variant's `exec_build` reads its input via this trait.
/// The trait provides two access modes:
///
/// - **Scalar access** ([`as_value`](Self::as_value)): for **leaf** and
///   **wrapper** nodes that consume the entire input as a single [`Value`].
///   `ValueInput` returns the wrapped `&Value` (clone); `PyInput` does one FFI
///   crossing to convert the PyObject to a Value.
/// - **Composite access** ([`get_field`](Self::get_field) /
///   [`get_index`](Self::get_index)): for **composite** nodes (Struct,
///   Sequence, Array) that read individual children by name or index.
///
/// Implementations:
/// - [`ValueInput`] — wraps `&Value`, the Phase 12 pure-Rust path.
/// - `PyInput` (construct-py) — wraps `&PyObject`, reads fields lazily via FFI.
pub trait Input {
    /// Returns the entire input as a single [`Value`].
    ///
    /// Used by **leaf nodes** (e.g. `CompiledFormatField`) and **wrapper
    /// nodes** (e.g. `CompiledAdapter`) whose `exec_build` delegates to
    /// `inner.build(&Value, ...)`. The `Construct::build` trait method
    /// still takes `&Value`, so the leaf/wrapper exec_build extracts the
    /// scalar via this method before calling `inner.build`.
    ///
    /// - `ValueInput::as_value` returns `self.value.clone()` (cheap — most
    ///   Value variants are `Copy` or small; `String`/`Bytes`/`Container`
    ///   clone is O(n) but unavoidable for build input).
    /// - `PyInput::as_value` performs one FFI crossing to convert the
    ///   entire PyObject to a Value (same cost as the old `py_to_value` on
    ///   a scalar — no tree recursion for leaf inputs).
    ///
    /// # Errors
    ///
    /// Returns a type-mismatch error if the underlying object cannot be
    /// converted to a [`Value`].
    fn as_value(&self) -> Result<Value>;

    /// Reads a named field, returning its value.
    ///
    /// Used by **composite nodes** (Struct, Sequence named entries) to read
    /// individual child values.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::FieldMissing`] if the field is absent and
    /// not optional.
    fn get_field(&self, name: &str) -> Result<Value>;

    /// Returns whether a named field is present (and non-None).
    fn has_field(&self, name: &str) -> bool;

    /// Reads the value at a sequential index (for Array/Sequence build).
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::Array`] if the index is out of bounds.
    fn get_index(&self, index: usize) -> Result<Value>;

    /// Returns the number of items (for repetition constructs).
    fn len(&self) -> usize;

    /// Returns a sub-input for a nested field (dict/dataclass inside dict).
    fn sub_input_field(&self, name: &str) -> Result<Box<dyn Input>>;

    /// Returns a sub-input for a sequential index.
    fn sub_input_index(&self, index: usize) -> Result<Box<dyn Input>>;
}

/// Phase 12 `Input` impl — wraps a `&Value` (zero-cost lazy read).
///
/// All methods read from the wrapped `&Value` with no FFI and no allocation
/// beyond the inevitable `Value::clone` in `as_value` / `get_field` (the
/// Value must be owned to be passed to `inner.build`).
pub struct ValueInput<'a> {
    value: &'a Value,
}

impl<'a> ValueInput<'a> {
    /// Creates a new `ValueInput` wrapping a reference to a [`Value`].
    #[must_use]
    pub fn new(value: &'a Value) -> Self {
        Self { value }
    }
}

impl<'a> Input for ValueInput<'a> {
    fn as_value(&self) -> Result<Value> {
        Ok(self.value.clone())
    }

    fn get_field(&self, name: &str) -> Result<Value> {
        let container = self.value.as_container()?;
        container
            .get(name)
            .cloned()
            .ok_or_else(|| ConstructError::FieldMissing {
                path: String::new(),
                field: name.to_string(),
            })
    }

    fn has_field(&self, name: &str) -> bool {
        self.value
            .as_container()
            .map(|c| c.get(name).is_some())
            .unwrap_or(false)
    }

    fn get_index(&self, index: usize) -> Result<Value> {
        let list = self.value.as_list()?;
        list.get(index)
            .cloned()
            .ok_or_else(|| ConstructError::Array {
                path: String::new(),
                index,
                len: list.len(),
            })
    }

    fn len(&self) -> usize {
        match self.value {
            Value::Container(c) => c.len(),
            Value::List(l) => l.len(),
            _ => 1,
        }
    }

    fn sub_input_field(&self, name: &str) -> Result<Box<dyn Input>> {
        // ValueInput borrows &'a Value, but Box<dyn Input> must be independent
        // of the parent's borrow. Clone the sub-value and wrap in OwnedValueInput.
        // Note: this clone is unavoidable for trait-object sub-inputs.
        let value = self.get_field(name)?;
        Ok(Box::new(OwnedValueInput::new(value)))
    }

    fn sub_input_index(&self, index: usize) -> Result<Box<dyn Input>> {
        let value = self.get_index(index)?;
        Ok(Box::new(OwnedValueInput::new(value)))
    }
}

/// Owned variant of [`ValueInput`] — holds an owned [`Value`] (for sub-inputs
/// that must outlive the borrow of their parent).
pub struct OwnedValueInput {
    value: Value,
}

impl OwnedValueInput {
    /// Creates a new `OwnedValueInput` from an owned [`Value`].
    #[must_use]
    pub fn new(value: Value) -> Self {
        Self { value }
    }
}

impl Input for OwnedValueInput {
    // Same logic as ValueInput but owns the Value.
    fn as_value(&self) -> Result<Value> {
        Ok(self.value.clone())
    }
    // ... get_field / has_field / get_index / len / sub_input_*: identical
    // to ValueInput (operating on &self.value).
}
```

> **为什么 `as_value` 返回 `Result<Value>` 而非 `Result<&Value>`**：
> 1. `Construct::build` 接受 `&Value`，leaf exec_build 调 `inner.build(&value)`。
>    若 `as_value` 返回 `&Value`，则对于 `ValueInput`（持有 `&Value`）可以
>    直接返回引用；但对于 `PyInput`（持有 `&PyObject`），转换出的 `Value`
>    是临时值，引用无法逃逸出方法——必须返回 owned `Value`。
> 2. 为统一 trait 签名，`as_value` 统一返回 `Result<Value>`（owned clone）。
>    `ValueInput::as_value` 的 `clone` 开销：标量 Value 是 `Copy`（Int/UInt/
>    Bool/None），零开销；String/Bytes clone 是 O(n) 但不可避免（build 输入
>    必须 owned）。`PyInput::as_value` 的开销：1 次 FFI + 标量转换。
>
> **`ValueInput` 的两种形态**：
> - `ValueInput<'a>`（借用）：exec_build 入口用，包装 `&Value`，零分配。
> - `OwnedValueInput`（owned）：sub_input 返回的 `Box<dyn Input>` 必须拥有
>   自己的数据（因为父 Input 可能已被 drop）。 OwnedValueInput 持有 owned
>   `Value`，逻辑与 ValueInput 完全一致。

### 3.3 exec_build 的 Input 化（前置改动）

当前 `CompiledExec::exec_build` 签名（`mod.rs:103-108`）：
```rust
fn exec_build(&self, input: &Value, stream: &mut CombinedStream, ctx: &mut Context) -> Result<()>;
```

改为接受 `&dyn Input`：
```rust
fn exec_build(&self, input: &dyn Input, stream: &mut CombinedStream, ctx: &mut Context) -> Result<()>;
```

**影响**：所有 variant 的 exec_build 签名变更。但读取语义不变——节点通过 Input
trait 方法按需读取，而非接收完整 `&Value`。

**纯 Rust 路径适配**：`ValueInput::new(&value)` 包装 Value，`get_field` 从 Container
中读。零开销（ValueInput 是薄包装，编译器内联）。

#### 3.3.1 三类节点的 exec_build 适配模式（B-FEAS-1 修正）

所有 72 个 CompiledNode variant 按其 exec_build 的输入消费方式分为三类。
每类有统一的适配模式，机械化改造：

**模式 A — 叶子节点（~17 个：FormatField/Bytes/VarInt/.../Pass/Tell/...）**

叶子节点嵌入声明树构造器（`inner: FormatField` 等），exec_build 委托给
`self.inner.build(&Value, ...)`。`Construct::build` 接受 `&Value`，所以叶子
节点必须从 `&dyn Input` 提取标量 Value。

```rust
// 改造前（impl_leaf_exec! 宏）：
fn exec_build(&self, input: &Value, stream, ctx) -> Result<()> {
    self.inner.build(input, stream, ctx)   // input: &Value
}

// 改造后：
fn exec_build(&self, input: &dyn Input, stream, ctx) -> Result<()> {
    let value = input.as_value()?;          // 提取标量 Value
    self.inner.build(&value, stream, ctx)   // inner.build 接受 &Value
}
```

- `ValueInput`：`as_value()` 返回 `self.value.clone()`，零 FFI。
- `PyInput`：`as_value()` 执行 1 次 FFI（PyObject→Value 标量转换），等价于
  旧架构对该叶子字段的单次 `py_to_value`。**无全量递归开销**。

**模式 B — 包装器节点（~25 个：Adapter/Validator/Enum/Const/.../Pointer/Prefixed/...）**

包装器节点持有 `inner: Box<CompiledNode>` + 包装数据（闭包/映射表/Value）。
exec_build 先处理输入（encode/validate/查表），再委托给 inner。

大多数包装器消费整个输入为标量（与叶子一样），因为它们的语义是"对单个值做变换"。
典型模式：先 `as_value()` 提取标量，变换后用 `ValueInput` 包装临时 Value 委托。

```rust
// Adapter 示例（impl_adapter_exec! 宏改造后）：
fn exec_build(&self, input: &dyn Input, stream, ctx) -> Result<()> {
    let value = input.as_value()?;                  // 提取标量
    let encoded = (self.encode)(&value, ctx)?;      // 变换
    let encoded_input = OwnedValueInput::new(encoded); // 包装临时 Value
    self.inner.exec_build(&encoded_input, stream, ctx) // 委托
}

// Const 示例（忽略输入，用自身固定值）：
fn exec_build(&self, input: &dyn Input, stream, ctx) -> Result<()> {
    // input 在 Const 中被忽略（或检查是否为 None）
    let const_input = OwnedValueInput::new(self.value.clone());
    self.inner.exec_build(&const_input, stream, ctx)
}

// Validator 示例（校验后透传）：
fn exec_build(&self, input: &dyn Input, stream, ctx) -> Result<()> {
    let value = input.as_value()?;
    (self.check)(&value, ctx)?;
    // 透传原始 input（不提取为 Value 再包装——性能优化）
    self.inner.exec_build(input, stream, ctx)
}
```

> **`OwnedValueInput` 包装临时 Value 的模式**：包装器变换产出的 `encoded` 是
> 局部 `Value`，需要作为 `&dyn Input` 传给 `self.inner.exec_build`。用
> `OwnedValueInput::new(encoded)` 包装（owned，满足 `Box<dyn Input>` 无生命周期
> 约束）。这是文档开头提到的"~15 处 `self.inner.exec_build(&临时Value, ...)` 模式"
> 的标准实现方式。`OwnedValueInput` 是薄包装（1 个 Value 字段），编译器内联后
> 与直接传 `&Value` 的开销差异仅为一次 `Value::clone`（as_value 中已发生）。

**模式 C — 复合节点（~6 个：Struct/Sequence/Array/ArrayExpr/GreedyRange/RepeatUntil）**

复合节点从 Input 中逐字段/逐元素读取，为每个子节点创建 sub-input 委托。
不需要 `as_value()`——直接用 `get_field` / `get_index` / `sub_input_*`。

```rust
// Struct 示例：
fn exec_build(&self, input: &dyn Input, stream, ctx) -> Result<()> {
    let mut child_ctx = ctx.subcontext();
    for field in &self.fields {
        match &field.name {
            Some(name) => {
                if field.flagbuildnone && !input.has_field(name) {
                    let none_input = OwnedValueInput::new(Value::None);
                    field.subcon.exec_build(&none_input, stream, &mut child_ctx)?;
                } else {
                    let sub = input.sub_input_field(name)?;
                    field.subcon.exec_build(sub.as_ref(), stream, &mut child_ctx)?;
                }
                // 更新 context（用 get_field 提取 Value）
                if let Ok(value) = input.get_field(name) {
                    child_ctx.insert(name.clone(), value);
                }
            }
            None => { /* anonymous field: skip */ }
        }
    }
    Ok(())
}

// Array 示例（固定 count）：
fn exec_build(&self, input: &dyn Input, stream, ctx) -> Result<()> {
    for i in 0..self.count {
        let sub = input.sub_input_index(i)?;
        self.subcon.exec_build(sub.as_ref(), stream, ctx)?;
    }
    Ok(())
}
```

**三类模式总结**：

| 模式 | 适用节点 | Input 方法 | 典型行数 |
|------|---------|-----------|---------|
| A. 叶子 | 17 个 leaf variant | `as_value()` | 3 行 |
| B. 包装器 | 25 个 wrapper variant | `as_value()` + `OwnedValueInput` 包装临时值 | 4-6 行 |
| C. 复合 | 6 个 composite variant | `get_field`/`get_index`/`sub_input_*` | 10-15 行 |

> **纯 Rust 路径零行为变化保证**：所有三个模式在 `ValueInput` 实现下，
> 行为与 Phase 12 的 `&Value` 签名完全一致。`as_value()` 返回 `Value::clone`
> （标量为 Copy，零开销），`get_field` 从 Container 中 clone（与当前
> `input.as_container()?.get(name)` 等价）。Phase 12 全部测试保持 PASS。

### 3.4 PyInput 实现（construct-py）

```rust
// construct-py/src/py_input.rs（新增）
/// `Input` impl that lazily reads fields from a Python `dict` / dataclass.
///
/// Each `get_field` call is one FFI crossing (Python → Rust value), but only
/// for the requested field — no pre-conversion of the entire tree.
pub struct PyInput<'py> {
    py: Python<'py>,
    obj: &'py Bound<'py, PyAny>,  // dict / dataclass / list
}

impl<'py> Input for PyInput<'py> {
    fn as_value(&self) -> Result<Value> {
        // 将整个 PyObject 转换为 Value（单次 FFI，标量转换，无递归树构建）
        py_to_value(self.py, self.obj).map_err(py_err_to_construct)
    }

    fn get_field(&self, name: &str) -> Result<Value> {
        // dict.__getitem__ 或 dataclass 属性访问
        let item = self.obj.get_item(name)
            .or_else(|_| self.obj.getattr(name))  // dataclass fallback
            .map_err(|_| ConstructError::FieldMissing {
                path: String::new(), field: name.to_string(),
            })?;
        py_to_value(self.py, &item).map_err(py_err_to_construct)
    }
    // ... has_field / get_index / sub_input_* ...
}
```

### 3.5 build_from_py 方法

```rust
impl CompiledSchemaHolder {
    /// Builds binary data from a Python object, returning it as `PyBytes`.
    ///
    /// Uses [`PyInput`] for lazy field reading — only fields actually
    /// consumed by `exec_build` incur FFI, not the entire object.
    ///
    /// # Errors
    ///
    /// Returns `PyErr` on build failure.
    pub fn build_from_py(&self, py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let input = PyInput::new(py, obj);
        let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
        let mut ctx = Context::new();
        ctx.insert("_building".into(), Value::Bool(true));
        self.schema.tree().exec_build(&input, &mut stream, &mut ctx)
            .map_err(|e| rust_err_to_py(py, e.with_path_prefix(BUILD_PATH)))?;
        let bytes = stream.into_bytes();  // ByteStream → Vec<u8>
        Ok(PyBytes::new_bound(py, &bytes).into_any().unbind())
    }
}
```

**对比旧架构**（`api.rs:166-193`）：
- 旧：`py_to_value` 全量递归 → `build(&value)` → bytes
- 新：`PyInput` 惰性包装 → `exec_build(&input)` 按需读 → bytes

**FFI 开销**：每个被 build 实际读取的字段产生 1 次 FFI（`get_field`）。对比旧架构的
全量递归 `py_to_value`（无论是否需要都转换所有字段），新架构按需读取，未访问字段零开销。

---

## 4. PyContextView（惰性映射 + REV #3 量化）

### 4.1 问题（REV #3）

旧架构 `context_to_py_container`（`conversions.rs:234-289`）在**每次**表达式求值时，
递归遍历整条 parent 链构建 Python Container，并注入 `_root` / `_params`（额外两次递归）。
对于 100 元素 Array + `this.length` 表达式，就是 100 次完整 Context 重建（~50µs/次）。

### 4.2 双保险设计（D4 决策）

| 路径 | 覆盖场景 | FFI 次数 |
|------|---------|---------|
| **Rust 闭包**（CompiledExpr::Native） | `this.field`、`this._._.field`、常量算术、`len_`/`sum_`/`abs_` | **0** |
| **PyContextView**（惰性 Mapping） | 用户 lambda、动态属性等无法静态分析的表达式 | 按需，每访问 1 字段 1 次 |

常见模式（this.field）在 SchemaCompiler 阶段编译为 Rust 闭包，运行时**零 FFI**。
PyContextView 仅在退化路径（PyCallback 表达式）使用。

### 4.3 PyContextView 设计

定义在 `construct-py/src/py_context.rs`（新增）。实现 Python `Mapping` 协议的 `#[pyclass]`：

```rust
/// Lazy Python view over a Rust [`Context`].
///
/// Implements the `Mapping` protocol (`__getitem__`/`__len__`/`__contains__`/
/// `keys`/`values`). Each field access converts a **single** `Value → PyObject`
/// on demand — never recursively builds the entire Container.
///
/// # I-COMP-1 修正：内存安全方案
///
/// PyContextView 持有一个 **owned Context 快照**（`Context` 实现了 `Clone`），
/// 而非 raw pointer。这消除了 use-after-free 风险：即使 Python 代码在
/// parse/build 帧结束后仍持有 PyContextView，快照仍然有效。
///
/// 快照在 PyContextView 创建时（PyCallback/ExprExtension 求值入口）一次性
/// 拷贝完成。后续的 `__getitem__` 访问从快照中按需读取，零 unsafe。
#[pyclass(name = "ContextView", unsendable)]
pub struct PyContextView {
    /// Owned snapshot of the Rust Context at creation time.
    /// Safe: no dangling pointer — the data is owned by this struct.
    ctx: Context,
    /// Field-level conversion cache (key → PyObject). Repeat accesses to the
    /// same field skip the Value→PyObject conversion after the first hit.
    cache: IndexMap<String, PyObject>,
}
```

> **I-COMP-1 修正：从 raw pointer 到 owned snapshot**
>
> **v1 问题**：PyContextView 持有 `*const Context`，Context 是栈上分配的结构体，
> parse/build 帧结束后销毁。若 Python 代码跨调用持有 PyContextView（如存入全局
> 变量、从 callable 返回），访问 ctx_ptr 是 use-after-free（UB）。
>
> **v2 修正**：改为 owned `Context` 快照。`Context` 已实现 `#[derive(Clone)]`，
> clone 会深拷贝 `fields`（IndexMap）+ 递归拷贝 parent 链。
>
> **选项评估**（为何选 C）：
>
> | 选项 | 可行性 | 理由 |
> |------|--------|------|
> | A. Context Arc 化 | **拒绝** | Context 是栈上结构，parse/build 热路径中频繁 `insert`/`merge`/`subcontext`（mutable 操作）。Arc 化需 `Arc<Mutex<Context>>` 或 `Arc<RefCell>`，性能毁灭性下降，且跨线程语义复杂 |
> | B. epoch/generation | **备选** | PyContextView 记录创建时 epoch，访问时校验。能检测 use-after-free 但不阻止（仍返回 error 而非正常值）。增加 thread-local epoch 计数器，实现复杂度中等。**保留为后续优化**：若快照开销在 profile 中显著，可改用 epoch 守卫 + raw pointer（fast path）回退到快照（safe path） |
> | C. 深拷贝快照 | **✓ 采纳** | 100% 安全，零 unsafe，实现最简。开销：一次 `Context::clone()`，克隆 IndexMap + parent 链（O(总字段数)）。由于 PyContextView 仅在 <5% 退化路径创建（用户 lambda），且 Context 字段是 `Value`（标量多为 Copy，Container/List clone 是浅层 Vec/IndexMap clone），开销可接受 |
> | D. unsendable + 生命周期 | **拒绝** | `#[pyclass(unsendable)]` 仅防跨线程，不防同线程跨调用。Python 可在同线程内跨 parse 调用持有 PyContextView → UB 未消除 |
>
> **快照开销量化**：典型 Context（3 层嵌套 Struct，每层 10 字段）的 clone ≈
> 3 × IndexMap(10 entries) clone ≈ 30 × Value clone ≈ 1-3µs。对比旧架构的
> `context_to_py_container`（相同场景 ~50µs 递归转换），快照开销仍低一个数量级。
> 且快照发生在退化路径（<5%），不是热路径。

**`__getitem__` 实现**（零 unsafe）：
```rust
#[pymethods]
impl PyContextView {
    fn __getitem__(&self, py: Python<'_>, key: &str) -> PyResult<PyObject> {
        // 特殊键：_ / _root / _params（惰性子视图，从快照派生）
        match key {
            "_" => self.parent_view(py),          // parent context 的 PyContextView
            "_root" => self.root_view(py),        // root struct context
            "_params" => self.params_view(py),    // 顶层 params context
            _ => {
                // 缓存命中？
                if let Some(cached) = self.cache.get(key) {
                    return Ok(cached.clone_ref(py));
                }
                // 按需转换单字段（从 owned 快照读取，零 unsafe）
                let value = self.ctx.get_recursive(key).ok_or_else(|| {
                    pyo3::exceptions::PyKeyError::new_err(key.to_string())
                })?;
                let py_obj = value_to_py(py, value)?;
                // Note: &self is immutable; cache mutation needs interior
                // mutability. Use RefCell<IndexMap> or accept that caching
                // is best-effort (skip on borrowed path). For Phase 13,
                // use `&mut self` in __getitem__ (PyO3 allows &mut self
                // for __getitem__ when the pyclass is not shared).
                Ok(py_obj)
            }
        }
    }
}
```

> **`get_recursive`**：Context 现有的沿 parent 链向上查找方法（`context.rs`），
> 操作于 owned 快照，**零 unsafe，零 Python 参与**。特殊键 `_`/`_root`/`_params`
> 返回**惰性子视图**（新的 PyContextView，持有从快照派生的 parent/root/params
> 快照），而非递归构建 Container——这修正了旧架构 `context_to_py_container_impl`
> 的全量递归缺陷。
>
> **子视图创建**：`parent_view(py)` 从 `self.ctx.parent()` 克隆出 parent Context，
> 创建新的 PyContextView。root_view/params_view 类似（沿 parent 链遍历到根/params 层）。
> 每个子视图创建开销 = 一次 Context 子树 clone。仅当 Python callable 显式访问
> `_`/`_root`/`_params` 时发生（退化路径中的退化路径，极罕见）。

**PyContextView 创建入口**：

```rust
impl PyContextView {
    /// Creates a PyContextView from a live Context (during PyCallback execution).
    ///
    /// Takes a snapshot (clone) of the Context — safe even if the original
    /// Context is later dropped.
    pub fn from_context(ctx: &Context) -> Self {
        PyContextView {
            ctx: ctx.clone(),    // owned snapshot
            cache: IndexMap::new(),
        }
    }
}
```

在 `PyCallback::ext_parse` / `ExprExtension::ext_eval` 中创建：
```rust
// PyCallback::ext_parse（Expr/Compute kind）：
let ctx_view = PyContextView::from_context(ctx);  // 快照
let result = self.callable.call1(py, (ctx_view,))?;
```

### 4.4 REV #3 — PyContextView FFI 开销量化

**单次 `__getitem__` 开销分解**：
| 操作 | 估计耗时 | 说明 |
|------|---------|------|
| Python→Rust FFI 进入 | ~100ns | pyo3 方法调用开销 |
| `get_recursive`（Rust 内） | ~20ns | IndexMap 哈希查找 + parent 链遍历 |
| `value_to_py` 单字段 | ~50-200ns | 取决于类型（int 快，Container 慢） |
| 缓存命中路径 | ~120ns | FFI + clone_ref（跳过转换） |
| **首次访问合计** | ~170-320ns | |
| **缓存命中合计** | ~120ns | |

**场景对比（100 元素 Array + `this.length`）**：
| 架构 | 每元素开销 | 100 元素总开销 | 相对 |
|------|-----------|--------------|------|
| 旧架构（全量 Context 转换） | ~50µs | ~5ms | 1x（基线） |
| PyContextView（退化路径，无缓存） | ~250ns | ~25µs | **200x 更快** |
| PyContextView（退化路径，有缓存） | ~120ns×100 + 250ns×1 | ~12µs | **400x 更快** |
| **Rust 闭包（常见模式）** | ~10ns | ~1µs | **5000x 更快** |

**结论**：
1. **常见模式（`this.field`）**：表达式编译为 Rust 闭包，**0 次 FFI**。5000x 加速。
   覆盖率：construct 原版测试中 ~95% 的表达式（Path/BinExpr/UniExpr/FuncPath 均可编译）。
2. **退化模式（用户 lambda）**：PyContextView 按需转换，200-400x 加速。字段缓存进一步优化。
3. **PyContextView 的 FFI 开销不是性能瓶颈**——它仅影响 <5% 的退化路径，且即使退化也快 200x。

### 4.5 表达式编译覆盖边界

| Python 表达式 | 编译产物 | FFI |
|--------------|---------|-----|
| `this.len` | CompiledExpr::Native(Path) | 0 |
| `this._._.count` | CompiledExpr::Native(Path 多层) | 0 |
| `this.len + 1` | CompiledExpr::Native(BinExpr) | 0 |
| `len_(this.data)` | CompiledExpr::Native(FuncPath) | 0 |
| `this.data[0]` | CompiledExpr::Native(Index) | 0 |
| `lambda ctx: ctx.x * ctx.y` | CompiledExpr::**External**(PyCallback) | 每次求值 1 次 |
| 任意 Python callable | CompiledExpr::External(PyCallback) | 每次求值 1 次 |

无法静态分析的表达式（lambda、动态 callable）退化为 `CompiledExpr::External`，求值时
创建 PyContextView 调用 Python callable。

---

## 5. PyCallback 节点（External variant + CompiledExtension trait）

### 5.1 方案选择（A / B / C）

**问题**：`CompiledNode` 在 construct-rs（无 pyo3 依赖）。PyCallback 需持有 `Py<PyAny>`。

| 方案 | 描述 | 评估 |
|------|------|------|
| A. construct-py wrapper enum | `PyCompiledNode { Native(CompiledNode), PyCallback(...) }` | ✗ 拒绝：CompiledNode 子节点引用是 `Box<CompiledNode>`，无法容纳 PyCallback；要么全新树（重复执行逻辑），要么退化为 Dynamic（无性能优势） |
| **B. External variant + CompiledExtension trait** | CompiledNode 新增 `External(CompiledExternal { inner: Box<dyn CompiledExtension> })`；construct-py 的 PyCallback 实现 CompiledExtension | **✓ 采纳**：construct-rs 无 pyo3；trait object 透明转发；零重复；enum_dispatch 统一分发 |
| C. 执行树外部拦截 | PyCallback 不入树，编译流程拦截 | ✗ 拒绝：执行树遍历有序嵌套，无法在树中间干净插入 FFI |

**方案 B 精细版**（关键改进：用 trait object 而非 `Box<dyn Any>`，避免 downcast 开销）：

construct-rs 定义扩展点 trait + variant，construct-py 实现。construct-rs **完全不认识 pyo3**。

### 5.2 CompiledExtension trait（construct-rs）

定义在 `construct-rs/src/compiled/ext.rs`（新增）：

```rust
//! Extension point for non-Rust compiled nodes (Phase 13 PyCallback).
//!
//! [`CompiledExtension`] is implemented by construct-py's `PyCallback` to
//! embed Python callables in the execution tree without construct-rs
//! depending on pyo3. The trait methods mirror [`CompiledExec`] but are
//! called via a trait object (`Box<dyn CompiledExtension + Send + Sync>`).

use crate::compiled::input::Input;
use crate::compiled::sink::OutputSink;
use crate::core::context::Context;
use crate::core::error::Result;
use crate::core::stream::CombinedStream;

/// Trait for externally-defined compiled nodes.
///
/// construct-rs defines this trait + the `External` variant; construct-py
/// provides the implementation (`PyCallback`). This is the **only** tree-internal
/// FFI point (D6 decision): a tree with zero `External` nodes has zero
/// tree-internal FFI.
///
/// # B-FEAS-2 修正
///
/// `ext_build` 接收 `&dyn Input`（与 [`CompiledExec::exec_build`] 一致），
/// 而非 `&Value`。CompiledExtension trait 在 construct-rs 中定义，可直接引用
/// [`Input`] trait。construct-py 的 `PyCallback` 实现用 `PyInput` 适配。
pub trait CompiledExtension: Send + Sync {
    /// Parses via the extension (e.g., calls a Python callable).
    /// Result is written to `sink` (scalar via `set_scalar`).
    fn ext_parse(
        &self,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn OutputSink,
    ) -> Result<()>;

    /// Builds via the extension.
    ///
    /// Receives `&dyn Input` — the extension reads its build value via
    /// [`Input::as_value`] (for scalar-consuming extensions) or
    /// [`Input::get_field`] / [`Input::get_index`] (for composite extensions).
    fn ext_build(
        &self,
        input: &dyn Input,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()>;

    /// Computes size via the extension.
    fn ext_sizeof(&self, ctx: &Context) -> Result<usize>;
}
```

> **B-FEAS-2 修正说明**：v1 中 `ext_build` 接收 `&Value`，与 §3.3 的
> `exec_build(&dyn Input)` 签名矛盾。修正后 `ext_build` 也接收 `&dyn Input`，
> 与 `CompiledExec::exec_build` 完全一致。CompiledExtension trait 在 construct-rs
> 中定义（与 Input trait 同 crate），可直接引用。construct-py 的 `PyCallback`
> 实现 `ext_build` 时，通过 `PyInput`（构造-py 中定义的 Input impl）适配
> PyObject——`input.as_value()` 执行 1 次 FFI 获取标量 Value，传给 Python callable。

### 5.3 CompiledNode::External variant（construct-rs）

在 `compiled/mod.rs` 新增 variant + 结构体 + CompiledExec impl：

```rust
/// Compiled node for an externally-defined extension (Phase 13 PyCallback).
///
/// The inner `Box<dyn CompiledExtension>` is opaque to construct-rs.
/// construct-py fills it with `PyCallback` (holding `Py<PyAny>`).
/// `CompiledExec` methods transparently forward to the extension.
pub struct CompiledExternal {
    pub inner: Box<dyn crate::compiled::ext::CompiledExtension>,
}

impl CompiledExec for CompiledExternal {
    fn exec_parse(&self, stream: &mut CombinedStream, ctx: &mut Context,
                  sink: &mut dyn OutputSink) -> Result<()> {
        self.inner.ext_parse(stream, ctx, sink)  // 透明转发
    }
    fn exec_build(&self, input: &dyn Input, stream: &mut CombinedStream,
                  ctx: &mut Context) -> Result<()> {
        self.inner.ext_build(input, stream, ctx)  // 透明转发（B-FEAS-2: &dyn Input）
    }
    fn exec_sizeof(&self, ctx: &Context) -> Result<usize> {
        self.inner.ext_sizeof(ctx)
    }
}

// CompiledNode enum 新增（变为 72 variant）：
pub enum CompiledNode {
    // ... 现有 71 variant ...
    /// External extension node (Phase 13 PyCallback). Opaque to construct-rs.
    External(CompiledExternal),
}
```

> **variant 计数变化**：Phase 12 = 71（70 内置 + Dynamic）。Phase 13 = 72（+ External）。
> 纯 Rust 路径中 External 不可达（SchemaCompiler 不产出 External），零影响。

### 5.4 PyCallback 实现（construct-py）

```rust
// construct-py/src/py_callback.rs（新增）
use construct::compiled::ext::CompiledExtension;
use construct::compiled::sink::OutputSink;

/// Kind of Python callback (determines calling convention).
pub enum CallbackKind {
    /// User-defined Adapter/Validator/construct: callable(obj, stream, **kw).
    Construct,
    /// Expression callable: callable(context) → Value.
    Expr,
    /// Compute function: callable(context) → Value.
    Compute,
}

/// A Python callable embedded in the compiled execution tree.
///
/// This is the **sole** tree-internal FFI point. When `exec_parse` reaches
/// this node, it crosses into Python (GIL), creates a lazy `PyContextView`,
/// calls the callable, converts the result, and writes to `sink`.
pub struct PyCallback {
    callable: Py<PyAny>,
    kind: CallbackKind,
}

impl CompiledExtension for PyCallback {
    fn ext_parse(&self, stream: &mut CombinedStream, ctx: &mut Context,
                 sink: &mut dyn OutputSink) -> Result<()> {
        Python::with_gil(|py| {
            match self.kind {
                CallbackKind::Construct => {
                    // 用户 Python construct：调 parse_stream（BytesIO 策略，
                    // 复用旧 PyConstructAdapter 逻辑，但包装为执行树节点）
                    let value = self.call_parse_stream(py, stream, ctx)?;
                    sink.set_scalar(value)
                }
                CallbackKind::Expr | CallbackKind::Compute => {
                    // 表达式/计算回调：创建 PyContextView（快照），调 callable(context)
                    let ctx_view = PyContextView::from_context(ctx);  // I-COMP-1: owned 快照
                    let result = self.callable.call1(py, (ctx_view,))
                        .map_err(|e| pyerr_to_construct(e))?;
                    let value = py_to_value(py, result.bind(py))?;
                    sink.set_scalar(value)
                }
            }
        })
    }
    // ... ext_build / ext_sizeof 同理 ...
}
```

### 5.5 CompiledExpr::External（表达式 PyCallback）

`CompiledExpr`（`expr.rs`）同样新增 External variant：

```rust
#[derive(Clone, Debug)]
pub enum CompiledExpr {
    Native(CombinedExpr),
    Constant(Value),
    /// External expression (Phase 13 Python callable). Evaluated via FFI.
    /// Arc for cheap cloning (CompiledExpr derives Clone).
    External(std::sync::Arc<dyn ExprExtension + Send + Sync>),
}

/// Expression extension trait (mirror of CompiledExtension for expressions).
pub trait ExprExtension: Send + Sync {
    fn ext_eval(&self, ctx: &Context, obj: Option<&Value>) -> Result<Value>;
}
```

construct-py 的 `PyExprCallback` 实现 `ExprExtension`，求值时创建 PyContextView 调用 callable。

### 5.6 REV #7 — Lazy 不兼容 direct-to-PyDict

**问题**：Lazy 构造器返回惰性代理（延迟解析），不能直接写入立即返回的 PyDict。

**Phase 12 现状**（`mod.rs:608-612`）：`CompiledLazy { inner: Box<CompiledNode> }` 是
**立即委托**（eager delegation）——exec_parse 直接调 inner.exec_parse，无真正延迟。
纯 Rust 路径（返回 Value）无法表达延迟。

**Phase 13 方案：PyLazyProxy（完整设计，分期实现）**：

```rust
/// Python-facing lazy proxy. Returned by Lazy nodes in the direct-to-Python path.
/// Delays actual parsing until first field access.
#[pyclass(name = "LazyContainer")]
pub struct PyLazyProxy {
    compiled: Box<CompiledNode>,       // 延迟解析的子树
    data: std::sync::Arc<[u8]>,        // 原始字节（零拷贝共享）
    offset: usize,                     // 解析起始位置
    ctx_snapshot: Context,             // context 快照（深拷贝，含 parent 链）
    parsed: std::sync::OnceLock<PyObject>,  // 延迟解析结果缓存
}

#[pymethods]
impl PyLazyProxy {
    fn __getitem__(&self, py: Python<'_>, key: &str) -> PyResult<PyObject> {
        let parsed = self.ensure_parsed(py)?;
        parsed.bind(py).get_item(key)
    }
    fn __getattr__(&self, py: Python<'_>, name: &str) -> PyResult<PyObject> {
        let parsed = self.ensure_parsed(py)?;
        parsed.bind(py).getattr(name)
    }
}

impl PyLazyProxy {
    fn ensure_parsed(&self, py: Python<'_>) -> PyResult<&PyObject> {
        self.parsed.get_or_try_init(|| {
            let mut stream = ByteStream::new_read_at(&self.data, self.offset);
            let mut ctx = self.ctx_snapshot.clone();
            let mut sink = PyDictSink::new_root(py);
            self.compiled.exec_parse(&mut stream, &mut ctx, &mut sink)?;
            sink.into_py_object()
        })
    }
}
```

**PyDictSink 的 Lazy 支持**：OutputSink 新增可选方法（默认返回 false）：

```rust
pub trait OutputSink {
    /// Attempts to set a lazy value. Returns `true` if the sink supports
    /// lazy values (PyDictSink creates PyLazyProxy); `false` if not
    /// (ValueSink — caller should eagerly parse).
    fn try_set_lazy(&mut self, _lazy: LazyValue) -> Result<bool> {
        Ok(false)  // default: not supported
    }
}

pub struct LazyValue {
    pub compiled: Box<CompiledNode>,
    pub data: std::sync::Arc<[u8]>,
    pub offset: usize,
    pub ctx: Context,
}
```

`CompiledLazy::exec_parse` 调用 `sink.try_set_lazy(lazy)`：
- PyDictSink：创建 PyLazyProxy，set_item 到 PyDict，返回 true（延迟）
- ValueSink：返回 false，CompiledLazy 立即委托 inner.exec_parse（Phase 12 行为）

**Phase 13 实现分期**：
- **13.x（初始）**：Lazy 退化为立即解析（`try_set_lazy` 总返回 false）。正确性保证，性能次优。
- **13.y（后续）**：实现 PyLazyProxy 真正延迟。需要 Context 的深拷贝（parent 链完整快照）。
  适用于大文件场景（Lazy 的核心价值）。

> **分期理由**：PyLazyProxy 的 Context 快照（parent 链深拷贝）实现复杂，且 Lazy 主要用于
> 大文件优化场景。Phase 13 的核心目标是 direct-to-Python + PyCallback，Lazy 延迟可独立交付。
> 立即解析不影响正确性，仅影响大文件场景的内存峰值。

---

## 6. construct-py 重写计划（4 文件变更）

### 6.1 变更总览

| 文件 | 当前行数 | Phase 13 后 | 变更性质 |
|------|---------|------------|---------|
| `conversions.rs` | 788 | ~300 | **大幅简化**：保留标量 `value_to_py`/`py_to_value`；删除 `context_to_py_container`（被 PyContextView 取代） |
| `expr_bridge.rs` | 909 | ~200 | **大幅简化**：PyClosure → CompiledExpr::External；5 个 func 桥接 → PyCallback 节点 |
| `py_adapter.rs` | 691 | **废弃** | PyConstructAdapter 被 PyCallback(CallbackKind::Construct) 取代 |
| `api.rs` | 595 | ~250 | **重写**：py_parse/py_build → CompiledSchema::parse_bytes_py/build_from_py |
| **新增** `py_sink.rs` | — | ~250 | PyDictSink 实现 |
| **新增** `py_input.rs` | — | ~150 | PyInput 实现 |
| **新增** `py_context.rs` | — | ~200 | PyContextView 实现 |
| **新增** `py_callback.rs` | — | ~200 | PyCallback + CallbackKind |
| **新增** `compiled_ext.rs` | — | ~180 | CompiledSchemaHolder（wrapper）+ parse_bytes_py/build_from_py + 缓存 |

### 6.2 conversions.rs 简化

**保留**：`value_to_py`（标量 Value → PyObject，PyDictSink 的 set_field 用）、
`py_to_value`（PyObject → Value，PyInput 的 get_field 用）。这两个是**单值转换**，非递归。

**删除**：`context_to_py_container`（788 行中约 100 行）——被 PyContextView 的惰性按字段转换取代。

**调整**：`value_to_py` 对 `Value::Container`/`Value::List` 的递归分支保留（PyDictSink 的
set_field 收到 Container 标量时需要递归转，但这是单字段的 Container，非整棵树）。

### 6.3 expr_bridge.rs 简化

**当前**（909 行）：`PyClosure`（包装任意 callable 为 Evaluate）+ 5 个 func 桥接
（`py_to_cond_func`/`py_to_key_func`/`py_to_check_func`/`py_to_compute_func`/`py_to_repeat_predicate`）。
每个桥接都在闭包内调 `context_to_py_container`（全量转换）。

**Phase 13 后**（~200 行）：每个桥接改为创建 `PyExprCallback`（实现 ExprExtension），

```rust
pub fn py_to_key_func(callable: PyObject) -> KeyFunc {
    // KeyFunc 现在是 Arc<dyn Fn + Send + Sync>（Phase 12 B4 迁移后）
    let ext: Arc<dyn ExprExtension + Send + Sync> = Arc::new(PyExprCallback::new(callable));
    Arc::new(move |ctx| ext.ext_eval(ctx, None))
}
```

PyExprCallback::ext_eval 内部创建 PyContextView（`from_context` 快照，I-COMP-1），调 callable，转结果。
**无 context_to_py_container 全量转换**。

### 6.4 py_adapter.rs 废弃

`PyConstructAdapter`（691 行，BytesIO 全量拷贝策略）被 `PyCallback { kind: Construct }` 取代。
用户 Python construct 子类在 SchemaCompiler 阶段编译为 `CompiledNode::External(PyCallback)`。

**`extract_subcon` 保留**：constructs_composite.rs 依赖它从 PyO3 wrapper 提取 CombinedConstruct。
新增逻辑：检测 Python callable 来源 → 编译为 External 而非 Dynamic。

### 6.5 api.rs 重写

```rust
// 重写后的 api.rs 核心
pub fn py_parse(constr_wrapper: &PyAny, py: Python<'_>, data: &[u8],
                kw: IndexMap<String, Value>) -> PyResult<PyObject> {
    // 1. 从 wrapper 提取/编译 CompiledSchemaHolder（首次编译，后续缓存）
    let holder = CompiledSchemaHolder::get_or_init(constr_wrapper)?;
    // 2. 注入 kw 为顶层 context（通过 holder.parse_bytes_py 内部处理，
    //    或在 holder 方法中接受 kw 参数）
    // 3. direct-to-Python parse（I-FEAS-1: holder 方法，非 CompiledSchema 方法）
    holder.parse_bytes_py(py, data)
}
```

旧的 `py_parse`/`py_build`/`py_parse_stream`/`py_build_stream` 全部改为调用 CompiledSchema。

---

## 7. SchemaCompiler 衔接（编译触发 + 缓存）

### 7.1 从 PyO3 wrapper 提取 CombinedConstruct

基于源码事实（`constructs_composite.rs:104-108`）：每个 PyO3 wrapper 持有
`inner: construct::constructs::Struct`（Phase 11 后为 CombinedConstruct 的具体 variant）。

**提取流程**：
```rust
pub fn get_or_compile(wrapper: &Bound<'_, PyAny>) -> PyResult<&CompiledSchema> {
    // wrapper 持有 compiled_cache: OnceLock<CompiledSchema>（新增字段）
    wrapper.getattr("_compiled")?.extract::<CompiledCacheRef>()
        .and_then(|cache| cache.get_or_compile(wrapper))
}
```

### 7.2 PyCallback 检测（编译期）

SchemaCompiler 遍历 CombinedConstruct 声明树时，遇到 `Dynamic` variant 检测内部是否为
Python callable：

```rust
// construct-py 层的编译扩展
fn compile_dynamic(inner: &Arc<dyn Construct>) -> CompiledNode {
    if let Some(py_callback) = try_extract_pycallback(inner) {
        // Python callable → External variant
        CompiledNode::External(CompiledExternal { inner: Box::new(py_callback) })
    } else {
        // 纯 Rust dynamic → Dynamic variant（Phase 12 行为）
        CompiledNode::Dynamic(CompiledDynamic { inner: inner.clone() })
    }
}
```

### 7.3 缓存机制

每个 PyO3 wrapper 新增 `_compiled: OnceLock<Py<CompiledSchemaHolder>>` 字段。
首次 parse/build 时触发编译，后续 `OnceLock::get` 命中（~1ns 原子读）。

`CompiledSchemaHolder`（定义在 construct-py 的 `compiled_ext.rs`，见 §2.3）持有
`Arc<CompiledSchema>`，同时作为 parse_bytes_py/build_from_py 的方法载体（I-FEAS-1）。

```rust
// construct-py/src/compiled_ext.rs
impl CompiledSchemaHolder {
    /// 编译或获取缓存的 CompiledSchema。
    /// 首次调用从 PyO3 wrapper 提取 CombinedConstruct 并编译；
    /// 后续调用从 OnceLock 缓存命中。
    pub fn get_or_init(wrapper: &Bound<'_, PyAny>) -> PyResult<&Self> {
        // OnceLock 逻辑：首次编译，后续缓存
        // wrapper.getattr("_compiled")?.get_or_try_init(|| {
        //     let schema = compile_from_wrapper(wrapper)?;
        //     Py::new(wrapper.py(), CompiledSchemaHolder::new(Arc::new(schema)))
        // })
    }
}
```

> **I-FEAS-1 一致性**：`CompiledSchemaHolder` 是 construct-py 中定义的 wrapper，
> 持有 `Arc<CompiledSchema>`。`parse_bytes_py`/`build_from_py` 是其 inherent 方法
> （见 §2.3/§3.5），不违反孤儿规则。`_compiled` 字段缓存的是
> `Py<CompiledSchemaHolder>`（Python 对象引用），而非裸 `CompiledSchema`。

### 7.4 与 Phase 12 SchemaCompiler 的兼容

Phase 12 的 `SchemaCompiler::compile(&CombinedConstruct)` 完全复用——它遍历 Rust 声明树，
产出 CompiledNode。Phase 13 的 PyCallback 检测是在 construct-py 层的**预处理**：
Python 声明树中的 callable 先被提取为 PyCallback，注入 CombinedConstruct::Dynamic 的
"标记"，然后 SchemaCompiler 遍历时遇到标记产出 External variant。

> **不修改 Phase 12 的 SchemaCompiler**：编译核心逻辑（遍历、表达式预编译、映射固化）
> 在 construct-rs，保持纯 Rust。construct-py 只做"Python callable → PyCallback 包装"的
> 预处理，和"编译产物缓存"的后处理。

---

## 8. 错误路径处理（REV #9）

### 8.1 方案选择

**问题**：parse 中途出错时，已部分写入的 PyDict 如何处理？

| 方案 | 描述 | 评估 |
|------|------|------|
| **A. 丢弃部分 PyDict** | 返回错误，Python 侧拿到 Err；PyDict 被 drop | **✓ 采纳**：与 Python 原版一致；性能最优；实现简单 |
| B. 部分结果 + 错误 | 返回半成品 PyDict + 错误信息 | ✗ 拒绝：改变 API 语义（parse 要么完整要么报错）；与原版不兼容 |
| C. 先 ValueSink 再转 PyDict | parse 前完整用 ValueSink 解析，成功后转 PyDict | ✗ 拒绝：退化到旧路径（Value 全量中转），丧失 direct-to-Python 优势 |

### 8.2 方案 A 实现细节

```rust
// CompiledSchemaHolder 的方法（I-FEAS-1: 不直接 impl CompiledSchema）
impl CompiledSchemaHolder {
    pub fn parse_bytes_py(&self, py: Python<'_>, data: &[u8]) -> PyResult<PyObject> {
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(data));
        let mut ctx = Context::new();
        let mut sink = PyDictSink::new_root(py);
        match self.schema.tree().exec_parse(&mut stream, &mut ctx, &mut sink) {
            Ok(()) => sink.into_py_object(),   // 成功：返回完整 PyDict
            Err(e) => Err(rust_err_to_py(py, e.with_path_prefix(PARSE_PATH))),
        }
        // ↑ Err 路径：sink（PyDictSink）在函数返回时被 drop。
        //   PyDictSink 持有的 Py<PyDict> 被 drop → PyO3 减引用计数 → Python GC 回收。
        //   Python 侧只拿到异常，看不到部分 PyDict。
    }
}
```

**错误信息充分性**：`ConstructError` 携带 `path` 字段（如 `"(parsing) -> header -> magic"`），
用户能定位出错字段位置。部分结果对调试无额外价值（字段路径已足够）。

### 8.3 PyCallback 错误传播

PyCallback 节点调用 Python callable 时，callable 可能抛异常。错误传播路径：

```
Python callable raises PyErr
    ↓
PyCallback::ext_parse 捕获 PyErr → 转 ConstructError（Expr/Generic/FieldMissing）
    ↓
exec_parse 返回 Err(ConstructError)
    ↓
parse_bytes_py 返回 Err(PyErr)（rust_err_to_py 还原异常子类）
    ↓
Python 侧拿到原始异常（StreamError/CheckError/...）
```

> KeyError 特殊处理（`expr_bridge.rs:103-118` 旧逻辑保留）：callable 抛 KeyError →
> ConstructError::FieldMissing → Python 侧重新抛 KeyError（匹配 `Computed(this.missing)` 语义）。

### 8.4 部分结果的替代方案（前瞻）

若未来需要部分结果（如调试模式），可通过 feature gate 提供方案 B 的变体：
`parse_bytes_py_partial` 返回 `(PyDict, Option<PyErr>)`。但这不是 Phase 13 范围。

---

## 9. REV 问题回应汇总

### REV #3：PyContextView FFI 开销量化

**回应：确认设计正确。FFI 开销已量化，不是瓶颈。**

- **常见模式（this.field，~95% 覆盖）**：编译为 Rust 闭包，**0 次 FFI**。5000x 加速。
- **退化模式（lambda，~5%）**：PyContextView 按需转换，200-400x 加速。
- **缓存机制**：PyContextView 内置字段级缓存（IndexMap<String, PyObject>），重复访问第二次起跳过转换。
- **量化数据**：见 §4.4 表格。100 元素 Array + this.length：旧 ~5ms → 新 ~1µs（闭包）/ ~12µs（PyContextView+缓存）。

详见 §4.4。

### REV #7：Lazy 不兼容 direct-to-PyDict

**回应：PyLazyProxy 方案设计完成，分期实现。**

- **完整方案**：PyLazyProxy（Python 代理对象）+ OutputSink::try_set_lazy（PyDictSink 支持，ValueSink 不支持）。
- **Phase 13 初始**：退化为立即解析（try_set_lazy 总返回 false）。正确性保证。
- **Phase 13 后续 / Phase 14**：实现 PyLazyProxy 真正延迟（Context 深拷贝快照）。
- **分期理由**：Lazy 核心价值在大文件场景；Phase 13 主目标是 direct-to-Python + PyCallback；
  立即解析不影响正确性。

详见 §5.6。

### REV #9：错误路径 PyDict 处理语义

**回应：采纳方案 A（丢弃部分 PyDict）。**

- **与 Python 原版一致**：原版出错也丢弃部分结果。
- **性能最优**：无额外开销。
- **错误信息充分**：ConstructError.path 提供字段级定位。
- **实现**：PyDict 在 Err 路径被 drop，Python GC 回收。Python 侧只拿异常。

详见 §8。

---

## 10. 子任务拆分建议

Phase 13 建议拆分为 8 个子任务，按依赖顺序：

### 13.1 OutputSink + Input 协议扩展（construct-rs，前置）
- 新增 `ProducedOutput` enum + `into_produced` / `finish_field` / `finish_named_item` / `finish_item` / `try_set_lazy` 方法
- ValueSink 适配（into_produced 包装 into_value；finish_named_item 默认 = push_item）
- 复合节点 exec_parse 改用对应 finish 方法（Struct→finish_field, Sequence→finish_named_item, Array→finish_item）
- 新增 `Input` trait（含 `as_value`/`get_field`/`get_index`/`sub_input_*`）+ `ValueInput`/`OwnedValueInput`（B-FEAS-1）
- 所有节点 exec_build 签名改为 `&dyn Input`（三类适配模式：叶子 as_value / 包装器 as_value+OwnedValueInput / 复合 get_field+sub_input）
- **验证**：Phase 12 全部测试保持 PASS（ValueSink/ValueInput 默认实现等价）；`cargo test` 全绿

### 13.2 CompiledExtension trait + External variant（construct-rs）
- 新增 `compiled/ext.rs`：CompiledExtension trait（ext_build 接收 `&dyn Input`，B-FEAS-2）+ ExprExtension trait
- CompiledNode 新增 External variant（72 variant）；CompiledExpr 新增 External variant
- CompiledExternal 的 CompiledExec impl（透明转发，exec_build 签名与 CompiledExec 一致）
- **验证**：纯 Rust 路径 External 不可达；编译通过；`cargo test` 全绿

### 13.3 PyDictSink 实现（construct-py）
- 新增 `py_sink.rs`：PyDictSink（Struct/Array/Leaf 模式）
- 实现 OutputSink 全部方法（含 finish_field 的 PyObject 直传）
- 单元测试：叶子标量、Struct 字段、Array 元素、嵌套 Struct
- **验证**：PyDictSink 独立测试（不接 CompiledSchema）；value_to_py 标量转换正确

### 13.4 PyInput 实现（construct-py）
- 新增 `py_input.rs`：PyInput（按需读 PyObject 字段）
- 实现 Input trait 全部方法
- 单元测试：dict 字段、dataclass 属性、list 索引、嵌套
- **验证**：PyInput 独立测试；对比旧 py_to_value 全量转换（按需性验证）

### 13.5 PyContextView 实现（construct-py）
- 新增 `py_context.rs`：PyContextView（Mapping 协议 + 字段缓存）
- 持有 owned `Context` 快照（I-COMP-1：消除 use-after-free，非 raw pointer）
- `__getitem__` / `__len__` / `__contains__` / `keys` / `values`
- 特殊键 `_` / `_root` / `_params`（从快照派生惰性子视图）
- 单元测试：单字段访问、缓存命中、parent 链、特殊键、跨调用持有安全性
- **验证**：PyContextView 独立测试；FFI 计数验证（访问 1 字段 = 1 次 FFI）；use-after-free 场景不 crash

### 13.6 PyCallback + CompiledExpr::External 实现（construct-py）
- 新增 `py_callback.rs`：PyCallback + CallbackKind + PyExprCallback
- 实现 CompiledExtension（Construct/Expr/Compute 三种 kind）
- 实现 ExprExtension（PyExprCallback）
- KeyError → FieldMissing 特殊处理
- 单元测试：3 种 CallbackKind 各一例；FFI 计数验证
- **验证**：含 1 个 PyCallback 的树，单次 parse 树内 FFI == 1

### 13.7 api.rs 重写 + SchemaCompiler 衔接（construct-py）
- 重写 `api.rs`：py_parse/py_build → parse_bytes_py/build_from_py
- 新增 `compiled_ext.rs`：CompiledSchemaHolder + get_or_compile（OnceLock 缓存）
- PyCallback 检测（Dynamic → External 预处理）
- conversions.rs 简化（删除 context_to_py_container）
- expr_bridge.rs 简化（PyClosure → PyExprCallback）
- py_adapter.rs 废弃标记
- **验证**：construct 原版测试通过率 ≥85%（S-FUNC-1）；缓存命中验证

### 13.8 PyLazyProxy（分期，可推迟）
- 新增 `py_lazy.rs`：PyLazyProxy + OutputSink::try_set_lazy 实现
- Context 深拷贝快照
- 单元测试：延迟解析、首次访问触发、缓存
- **验证**：大文件场景内存峰值降低

### 依赖关系

```
13.1 (协议扩展) ──┬──► 13.3 (PyDictSink) ──┐
                  ├──► 13.4 (PyInput)    ──┤
13.2 (External) ──┤                        ├──► 13.7 (api 重写) ──► 13.8 (PyLazyProxy)
                  ├──► 13.5 (PyContextView)─┤
                  └──► 13.6 (PyCallback)  ──┘
```

13.1 和 13.2 是前置（construct-rs 改动）。13.3-13.6 可并行（construct-py 各模块独立）。
13.7 是集成（依赖 13.3-13.6）。13.8 可推迟到 Phase 14。

### 出口标准

- `cargo build` + `cargo clippy` + `cargo fmt --check` + `cargo test` 全绿（construct-rs + construct-py）
- S-ARCH-1（per-tree FFI）：纯内置构造器树 0 树内 FFI；含 1 PyCallback = 1 FFI
- S-ARCH-2（Value 退出 Python 路径）：parse_bytes_py 不经过 Value::Container 中转
- S-FUNC-1（原版测试通过率）：≥85%

---

## 附录：与顶层架构设计 §8.2 验收标准的对应

| 验收标准（§8.2） | 本设计覆盖章节 | 实现子任务 |
|-----------------|--------------|-----------|
| 8.2.2 CompiledSchema parse_bytes_py/build_from_py | §2.3（CompiledSchemaHolder wrapper）, §3.5 | 13.3, 13.4, 13.7 |
| 8.2.4 Context 惰性视图 PyContextView | §4 | 13.5 |
| 8.2.6 表达式编译 CompiledExpr External | §4.5, §5.5 | 13.6 |
| 8.2.7 PyCallback 节点 | §5 | 13.2, 13.6 |
| S-ARCH-1 per-tree FFI | §5.1（External 是唯一树内 FFI 点） | 13.6 验证 |
| S-ARCH-2 Value 退出 Python 路径 | §2.1（ProducedOutput 打破 Value 中转） | 13.3 验证 |
```
