# 模块设计：Python-first 产出层（Phase 16）

> **阶段**：Phase 16 — Python-first 产出层重写
> **目标**：让 Rust 编译执行树直接产出 PyObject，消除 Value 中间类型在 parse/build 关键路径上的全部开销，使 S-PERF-1 ≥1.0x vs Python 原版
> **前置**：Phase 12（编译执行树）+ Phase 13/14（PyDictSink/PyInput direct-to-Python 路径，未达标）
> **定位依据**：AGENTS.md §0（项目定位修正）、§7（核心技术决策更新）

---

## §1 设计目标与约束

### 1.1 最高优先级约束（来自 §0）

> **这是一个 Python 项目。** 交付物是 Python 包。Rust 代码是内核实现手段，不是独立产品。

由此推导出的硬约束：

| 编号 | 约束 | 来源 |
|------|------|------|
| C1 | pyo3 是核心依赖，不是可选附加 | §0 第 2 条 |
| C2 | Rust 代码应直接产出 PyObject，不产出中间 Rust 类型再转换 | §0 第 2 条 + §7 |
| C3 | Value 枚举不应出现在 Python parse/build 关键路径上 | §0 第 4 条 + §7 |
| C4 | 不允许以"Rust 纯净性"为由牺牲 Python 性能 | §0 第 3 条 |
| C5 | 纯 Rust 单元测试路径（1515 个 `#[cfg(test)]` 测试）必须保留 | 任务要求 |

### 1.2 性能目标

| 格式 | 方向 | Python 原版基线 | 当前(new_compiled) | 当前比率 | **Phase 16 目标** |
|------|------|---------------|-------------------|---------|-----------------|
| simple (3字段,10B) | parse | 3.0µs | 6.6µs | 0.46x | **≥1.0x (≤3.0µs)** |
| simple (3字段,10B) | build | 2.8µs | 20.6µs | 0.13x | **≥1.0x (≤2.8µs)** |
| nested (2级,14B) | parse | ~4.0µs | ~11.1µs | 0.36x | **≥1.0x (≤4.0µs)** |
| nested (2级,14B) | build | ~3.2µs | ~29.1µs | 0.11x | **≥1.0x (≤3.2µs)** |
| array (100元素,404B) | parse | 27.7µs | 232µs | 0.12x | **≥1.0x (≤27.7µs)** |
| array (100元素,404B) | build | ~30µs | ~750µs | 0.04x | **≥1.0x (≤30µs)** |

> 基线数据来源：2026-06-19 PM 实测（`test_perf_comparison.py`，5000 次迭代），见 `docs/refactor/模块设计-FFI批量化优化.md` §性能假设。

### 1.3 不保护旧架构

以下 Phase 1-15 的架构元素**不是要优化的对象，而是要消除的对象**：

- `Value` 枚举作为 parse/build 产出载体（→ 改为 PyObject 直接产出）
- `OutputSink` trait 的 `set_scalar(Value)` / `set_field(Value)` 接口（→ Py 路径不经过此 trait）
- `conversions.rs` 的通用 `value_to_py` / `py_to_value`（→ 大幅缩减，仅保留 Dynamic 逃生舱路径）
- `PyDictSink`（construct-py）的 per-field Value→PyObject 转换（→ 被新 PySink 替代）
- `ProducedOutput` enum 的 `Box<dyn Any>` 装箱/拆箱（→ 消除）

---

## §2 技术方案选择：pyo3 如何进入 construct-rs

### 2.1 方案对比

| 方案 | 描述 | 优点 | 缺点 | 结论 |
|------|------|------|------|------|
| **A. pyo3 作为 optional feature** | construct-rs 新增 `pyo3 = { optional = true }`，Py 路径代码用 `#[cfg(feature = "python")]` 门控 | 最小改动；编译执行树 + PySink/exec_parse_py 全在 construct-rs 内（无跨 crate trait object 问题）；纯 Rust 测试不受影响 | construct-rs 不再"纯净"（但 §0 C4 已否决此约束） | **✅ 选定** |
| B. 合并两个 crate | construct-rs + construct-py 合并为单 crate | 消除 crate 边界 | 破坏构建系统、破坏 1515 个测试、改动巨大 | ❌ 过度 |
| C. PySink 留在 construct-py | 不修改 construct-rs，在 construct-py 写 71-variant 手动 dispatch | construct-rs 零改动 | 手动 71-arm match 极易与 enum 变更不同步；CompiledExtension/External 跨 crate 集成困难；PyInput 反射 construct-rs 内部类型 | ❌ 脆弱 |

### 2.2 选定方案 A 的论证

**为何 A 优于 C（关键决策）**：

编译执行树 `CompiledNode`（71 变体）定义在 `construct-rs/src/compiled/mod.rs`。Py 路径需要为**每个变体**实现 `exec_parse_py` / `exec_build_py`。如果这些实现留在 construct-py（方案 C），则：

1. construct-py 必须 `match` 所有 71 个 `CompiledNode` 变体——每次 construct-rs 新增变体（如未来 Phase），construct-py 的 match 就会漏项，编译失败或运行时 panic。
2. `CompiledExtension` trait（Phase 13 扩展点）定义在 construct-rs。Py 路径需要与之集成（`ext_parse_py`），跨 crate 修改 trait 定义违反 orphan rule。
3. 叶节点需要访问 `self.inner`（如 `CompiledFormatField.inner: FormatField`）的**内部字段**（如 byte_size、format kind）来做类型已知的 PyObject 构造。这些字段大多不是 `pub` 的——construct-py 无法访问。

方案 A 将 Py 路径放在 construct-rs 内，与编译树同 crate，可以自由访问内部字段、与 enum_dispatch / CompiledExtension 无缝集成。

**为何 A 不违反 §0**：

§0 C4 明确："不允许以'Rust 纯净性'为由牺牲 Python 性能。'construct-rs 不依赖 pyo3'不是一个有效约束"。方案 A 引入 pyo3 是 C1/C2 的直接要求。

### 2.3 Cargo.toml 变更

```toml
# construct-rs/Cargo.toml (新增)
[dependencies]
# ... 现有依赖不变 ...
pyo3 = { version = "0.22", optional = true, default-features = false }

[features]
default = []
# ... 现有 features 不变 ...
python = ["dep:pyo3"]
```

```toml
# construct-py/Cargo.toml (修改依赖声明)
[dependencies]
construct = { path = "../construct-rs", features = ["byteorder", "bigint", "compression", "python"] }
# pyo3 版本必须与 construct-rs 一致，避免双链接
pyo3 = { version = "0.22", features = ["num-bigint", "multiple-pymethods"] }
```

**关键约束**：construct-rs 和 construct-py 必须使用**相同版本**的 pyo3，否则会出现双链接冲突。由于 construct-py 依赖 construct-rs，Cargo 会统一版本——只要版本号一致。

### 2.4 cfg 门控策略

所有 Py 路径代码用 `#[cfg(feature = "python")]` 门控：

```rust
// construct-rs/src/compiled/py_sink.rs (新文件)
#![cfg(feature = "python")]
//! Python-direct output sink — defined in construct-rs under the "python" feature.
use pyo3::prelude::*;
// ...

// construct-rs/src/compiled/mod.rs (新增模块声明)
#[cfg(feature = "python")]
pub mod py_sink;
#[cfg(feature = "python")]
pub mod py_input;
#[cfg(feature = "python")]
pub mod py_exec;  // PyCompiledExec trait + dispatch
```

**编译矩阵**：

| 命令 | feature | Py 路径 | Value 路径 | 用途 |
|------|---------|--------|-----------|------|
| `cargo test` (construct-rs) | (无 python) | ❌ 不编译 | ✅ | 1515 个纯 Rust 单元测试 |
| `cargo build --features python` (construct-rs) | python | ✅ | ✅ | 验证 Py 路径编译 |
| `maturin develop` (construct-py) | python (传递) | ✅ | ✅ | Python 包构建 |

**纯 Rust 测试不受影响**：`exec_parse` / `exec_build`（Value 路径）的代码完全不变。新增的 `exec_parse_py` / `exec_build_py` 在无 `python` feature 时不参与编译。1515 个测试全部原样通过。

---

## §3 CompiledNode 直接产出 PyObject 的接口设计（parse 方向）

### 3.1 设计决策：新增 `PyCompiledExec` trait + 宏生成 dispatch

**选项分析**（回应任务问题 2）：

| 选项 | 描述 | 评估 |
|------|------|------|
| a. 新增 trait 方法 `exec_parse_py` | 在 `CompiledExec` trait 上加 cfg-gated 方法 | ❌ enum_dispatch 对单 enum 多 trait 的支持不确定；且污染 Value 路径的 trait 定义 |
| b. 泛型化 OutputSink `<T>` | `OutputSink<T>` 其中 T=Value 或 PyObject | ❌ leaf 的 `Construct::parse` 返回 Value 非 T；需重写所有 leaf parse 逻辑；enum_dispatch 不支持泛型 trait |
| c. **独立 `PyCompiledExec` trait + 宏 dispatch** | 新 trait，各变体各自实现，宏生成 71-arm match | **✅ 选定**：最稳健，不碰 Value 路径 trait，dispatch 显式可控 |

**为何不用 enum_dispatch 做第二个 trait 的 dispatch**：`CompiledNode` enum 已有 `#[enum_dispatch(CompiledExec)]`。对同一 enum 添加第二个 `#[enum_dispatch(PyCompiledExec)]` 的行为未被 enum_dispatch 文档明确保证。为确保正确性，Py 路径的 dispatch 使用**宏生成的显式 match**。该宏列出所有变体，若 construct-rs 新增变体而忘记实现 `PyCompiledExec`，编译期即报错（trait 未实现）。

### 3.2 PySink trait 设计

`PySink` 是 parse 方向的 Python 直接产出 sink。核心原则：**leaf 产出的标量同时携带 Value（供 context 表达式求值）和 PyObject（供输出），一次解码两份产出，零 round-trip**。

```rust
// construct-rs/src/compiled/py_sink.rs
#![cfg(feature = "python")]

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use crate::core::error::Result;
use crate::value::Value;

/// Python-direct output sink for parse direction.
///
/// Each method receives `py: Python<'_>` (the GIL token, threaded once
/// from the entry point). The sink maintains EITHER:
/// - a pending scalar `(Value, PyObject)` pair (leaf scenario), OR
/// - a `Py<PyDict>` + parallel `IndexMap<String, Value>` (Struct scenario), OR
/// - a `Py<PyList>` + parallel `Vec<Value>` (Array scenario).
///
/// The parallel Value tree is for **context expression evaluation only**
/// (this.field references). It is NOT the output — the output is the
/// PyObject tree. The Value tree is ~100x cheaper than the FFI round-trips
/// it eliminates (see §6 B1 analysis).
pub trait PySink {
    /// Leaf deposit: stores both the scalar Value (for ctx) and PyObject
    /// (for output). Both are produced from a single decode — no round-trip.
    ///
    /// # Errors
    /// Propagates any error from sink state validation.
    fn deposit_leaf(&mut self, py: Python<'_>, value: Value, obj: PyObject) -> Result<()>;

    /// Creates a sub-sink for a named Struct field.
    fn sub_py_field(&mut self, name: &str) -> Result<Box<dyn PySink>>;

    /// Creates a sub-sink for a sequential Array item.
    fn sub_py_item(&mut self) -> Result<Box<dyn PySink>>;

    /// Finishes a named field sub-sink: integrates its PyObject into the
    /// parent's PyDict (via `PyDict_SetItem`) AND returns the child's Value
    /// for context insertion. **No round-trip** — the Value comes from the
    /// child's parallel Value tree, not from py_to_value(PyObject).
    ///
    /// # Errors
    /// Propagates any error from sink integration.
    fn finish_field_py(
        &mut self,
        py: Python<'_>,
        name: &str,
        sub: Box<dyn PySink>,
    ) -> Result<Value>;

    /// Finishes an anonymous item sub-sink (Array elements, anonymous
    /// Sequence/Struct fields). Integrates PyObject into parent's PyList.
    fn finish_item_py(&mut self, py: Python<'_>, sub: Box<dyn PySink>) -> Result<()>;

    /// Finishes a named Sequence entry (like finish_field_py but appends to
    /// PyList instead of PyDict).
    fn finish_named_item_py(
        &mut self,
        py: Python<'_>,
        name: &str,
        sub: Box<dyn PySink>,
    ) -> Result<Value>;

    /// Consumes the sink and returns the root PyObject.
    fn into_root_py(self: Box<Self>, py: Python<'_>) -> Result<PyObject>;
}
```

### 3.3 具体 PySink 实现：`PyDictSink2`

替代 construct-py 的旧 `PyDictSink`。定义在 construct-rs 内（可访问 pyo3 + Value）。

```rust
/// Internal mode of PyDictSink2.
///
/// [C-CRIT-1 修正 §3.3.1] `dict`/`list` 字段实际类型为 `Py<PyAny>`（可为 Container/
/// ListContainer 子类）。此处先展示 plain PyDict/PyList 的概念结构，
/// 实际实现见 §3.3.1（使用 Container/ListContainer 作为底层存储）。
enum PySinkMode {
    /// Nothing deposited yet.
    Empty,
    /// Leaf: holds (Value for ctx, PyObject for output).
    Leaf { value: Value, obj: PyObject },
    /// Struct: owns dict-like (Container or PyDict) + IndexMap (ctx Values).
    Struct {
        dict: Py<PyAny>,  // §3.3.1: Container(dict 子类) 或 plain PyDict
        ctx_fields: IndexMap<String, Value>,
    },
    /// Array: owns list-like (ListContainer or PyList) + Vec (ctx Values).
    Array {
        list: Py<PyAny>,  // §3.3.1: ListContainer(list 子类) 或 plain PyList
        ctx_items: Vec<Value>,
    },
}

pub struct PyDictSink2 {
    mode: PySinkMode,
}

impl PyDictSink2 {
    /// Root sink pre-initialized as Struct with an empty PyDict.
    pub fn new_root_struct(py: Python<'_>) -> Self {
        Self {
            mode: PySinkMode::Struct {
                dict: PyDict::new_bound(py).unbind(),
                ctx_fields: IndexMap::new(),
            },
        }
    }

    pub fn new_empty() -> Self {
        Self { mode: PySinkMode::Empty }
    }
}
```

**finish_field_py 实现（关键：零 round-trip）**：

```rust
fn finish_field_py(
    &mut self,
    py: Python<'_>,
    name: &str,
    sub: Box<dyn PySink>,
) -> Result<Value> {
    // Consume the child sink → get its (Value, PyObject) pair.
    let (value, obj) = sub.into_value_obj_pair(py)?;

    match &mut self.mode {
        PySinkMode::Struct { dict, ctx_fields } => {
            // Output: ONE PyDict_SetItem call.
            dict.bind(py).set_item(name, &obj)
                .map_err(py_err_to_construct)?;
            // Ctx: store Value (cheap Rust op, no FFI).
            ctx_fields.insert(name.to_string(), value.clone());
            Ok(value) // Return for parent's ctx.insert(name, value).
        }
        _ => Err(ConstructError::Generic {
            path: String::new(),
            message: format!("finish_field_py({name}) on non-Struct sink"),
        }),
    }
}
```

**对比旧 PyDictSink::finish_field（B1 瓶颈）**：

| 步骤 | 旧 PyDictSink (B1) | 新 PyDictSink2 |
|------|-------------------|----------------|
| 子 sink 产出 | `ProducedOutput::Object(Py<PyList>)` | `(Value::List, PyObject)` |
| 获取 ctx Value | `py_to_value(PyObject)` ← **N 元素 round-trip** | 直接返回子 sink 的 Value ← **零 round-trip** |
| 写入 PyDict | `dict.set_item(name, py_obj)` | `dict.set_item(name, obj)` |
| FFI 次数（100元素嵌套） | 1 clone_ref + ~202 py_to_value + 1 set_item = **204** | 1 set_item = **1** |

### 3.3.1 Container/ListContainer 包装机制

> **[REV 检视修正 C-CRIT-1]** 原设计 §3.3 的 `PyDictSink2` 创建 plain PyDict/PyList，
> `into_root_py` 返回 raw PyDict/PyList —— 无 Container/ListContainer 包装。
> 这将导致 `result.field` 属性访问失效（BC-5），61 个 dataclass 测试可能失败。
> 以下补充包装方案。

**问题背景**：Python 原版 construct 的 parse 结果必须包装在 `Container`（dict 子类，支持
`result.field` 属性访问 + `search()`/`search_all()`）和 `ListContainer`（list 子类）中。
当前 `conversions.rs:58-95` 的 `value_to_py` 对 List/Container 分支显式 import 并构造这些子类。
包装必须递归覆盖**所有嵌套的复合节点**（顶层 + 嵌套 Struct/Array），否则
`result.nested.field` 的属性访问在中间层级失效。

**选定方案：从创建起使用 Container/ListContainer 作为底层存储**

关键洞察：`Container` 是 `dict` 子类（`class Container(dict)`，见
`construct-py/construct_rust/lib/containers.py:94`），`ListContainer` 是 `list` 子类
（`class ListContainer(list)`，line 235）。因此：

1. `PyDict_SetItem` / `PyDict_SetItemString` 可直接作用于 Container 实例（dict 子类）
2. `PyList_Append` / `PyList_SET_ITEM` 可直接作用于 ListContainer 实例（list 子类）
3. pyo3 的 `downcast::<PyDict>` 对 Container 返回 `Ok`（因为 `PyDict_Check` 接受子类）

因此**从创建起就使用 Container/ListContainer**，而非创建 plain PyDict 后再包装（后者需要
额外复制开销）。所有后续的 `set_item`/`append` 操作行为与 plain dict/list 完全一致。

**实现设计**：

```rust
/// 缓存的 Container/ListContainer 类引用（每次 parse 调用获取一次，sub-sink 共享）。
///
/// 若 construct_rust.lib.containers 不可导入（如 cargo test 环境），
/// container_cls / listcontainer_cls 为 None，sub-sink 退化到 plain PyDict/PyList。
#[derive(Clone)]
pub struct ContainerClasses {
    /// construct_rust.lib.containers.Container（dict 子类），None 则不可用
    container_cls: Option<Py<PyAny>>,
    /// construct_rust.lib.containers.ListContainer（list 子类），None 则不可用
    listcontainer_cls: Option<Py<PyAny>>,
}

impl ContainerClasses {
    /// 在 root sink 创建时获取一次。import_bound ~200-400ns（2 次 import），
    /// 之后所有 sub-sink 通过 Clone 共享（Py<T> 的 clone 是 refcount inc，~5ns）。
    ///
    /// **优化**：生产环境可用 `static OnceLock<ContainerClasses>` 全局缓存类引用，
    /// 使 import 仅在进程首次 parse 调用时发生。5000 次迭代基准测试中，
    /// import 开销摊薄至 ~0.08ns/次（可忽略）。`Py<PyAny>` 在 pyo3 0.22 是
    /// `Send + Sync`，满足 `OnceLock` 的线程安全要求。
    pub fn obtain(py: Python<'_>) -> Self {
        let container_cls = py
            .import_bound("construct_rust.lib.containers")
            .and_then(|m| m.getattr("Container"))
            .ok()
            .map(|cls| cls.unbind());
        let listcontainer_cls = py
            .import_bound("construct_rust.lib.containers")
            .and_then(|m| m.getattr("ListContainer"))
            .ok()
            .map(|cls| cls.unbind());
        Self { container_cls, listcontainer_cls }
    }
}

pub struct PyDictSink2 {
    mode: PySinkMode,
    /// 共享的类引用（root 获取，sub-sink Clone 继承）
    classes: ContainerClasses,
}
```

**sub-sink 创建逻辑（关键：从创建起用 Container/ListContainer）**：

```rust
impl PyDictSink2 {
    /// Root sink：创建 Container 实例（若可用）或 plain PyDict。
    pub fn new_root_struct(py: Python<'_>) -> Self {
        let classes = ContainerClasses::obtain(py);
        let dict = match &classes.container_cls {
            Some(cls) => {
                // Container() → 空容器，self.__dict__ = self 启用属性访问
                cls.call0(py).unwrap_or_else(|_| {
                    PyDict::new_bound(py).into_any().unbind()
                })
            }
            None => PyDict::new_bound(py).into_any().unbind(),
        };
        // downcast::<PyDict>() 对 Container 返回 Ok（dict 子类）
        Self {
            mode: PySinkMode::Struct { dict, ctx_fields: IndexMap::new() },
            classes,
        }
    }

    /// Struct 子 sink：创建 Container（继承 classes）
    pub fn sub_struct(py: Python<'_>, classes: &ContainerClasses) -> Self {
        let dict = classes.container_cls.as_ref()
            .and_then(|cls| cls.call0(py).ok())
            .unwrap_or_else(|| PyDict::new_bound(py).into_any().unbind());
        Self {
            mode: PySinkMode::Struct { dict, ctx_fields: IndexMap::new() },
            classes: classes.clone(),
        }
    }

    /// Array 子 sink：创建 ListContainer（继承 classes）
    pub fn sub_array(py: Python<'_>, classes: &ContainerClasses) -> Self {
        let list = classes.listcontainer_cls.as_ref()
            .and_then(|cls| cls.call0(py).ok())
            .unwrap_or_else(|| PyList::empty_bound(py).into_any().unbind());
        Self {
            mode: PySinkMode::Array { list, ctx_items: Vec::new() },
            classes: classes.clone(),
        }
    }
}
```

> **注意**：PySinkMode 的 `dict`/`list` 字段类型从 `Py<PyDict>`/`Py<PyList>` 改为
> `Py<PyAny>`（因为可能是 Container/ListContainer 子类）。操作时用
> `bound.set_item(k, v)`（PyObject_SetItem，对 dict 子类等价于 PyDict_SetItem）
> 或 `bound.downcast::<PyDict>()` 后用 PyDict 方法。DEV 可测量两者差异后择优。

**finish_field_py / finish_item_py 的 set_item 行为不变**：

Container 作为 dict 子类，`set_item(name, obj)` 行为与 plain PyDict 完全一致。
ListContainer 作为 list 子类，`append(obj)` 行为与 plain PyList 完全一致。
Container 的 `__dict__ = self`（在 `__init__` 中设置）确保后续属性访问可用。

**包装开销估算**：

| 场景 | 复合节点数 | Container/ListContainer 开销 | 备注 |
|------|----------|---------------------------|------|
| import（root 一次） | 2 次 import_bound | ~400ns | 仅 root sink |
| simple struct (1 Container) | 1 | 1× call0(~150ns) | 替代 PyDict::new(~30ns)，净增 ~120ns |
| nested struct (2 Container) | 2 | 2× call0(~300ns) | 净增 ~240ns |
| array (1 Container + 1 ListContainer) | 2 | 2× call0(~300ns) | 净增 ~240ns |

> **关键**：call0（`Container()`）创建 dict 子类实例，CPython 分配 dict 内部结构 + 设置
> `__dict__ = self`。开销 ~100-200ns，略高于 `PyDict::new`（~30ns），但远低于
> "先建 plain dict 再复制到 Container"（后者需 O(n) 复制）。

### 3.4 标量快速转换：`value_to_py_scalar`

替代 `conversions.rs` 的通用 `value_to_py`。仅处理标量（leaf 产出），**单次 match，无类型检查链，无 import_bound**：

```rust
/// Fast scalar Value → PyObject conversion for leaf deposits.
///
/// Unlike conversions.rs::value_to_py, this function:
/// - Does NOT do 7x is_instance_of type checks (Value is already typed).
/// - Does NOT call import_bound for Container/List (composites manage
///   PyDict/PyList directly via PySink, never through this function).
/// - Is a single match arm per scalar type → one C API call.
///
/// # Panics
/// Panics in debug mode if given a Container/List (should never happen at
/// leaf level — composites bypass this function).
pub fn value_to_py_scalar(py: Python<'_>, v: &Value) -> PyResult<PyObject> {
    match v {
        Value::None => Ok(py.None()),
        Value::Bool(b) => Ok(b.into_py(py)),
        Value::Int(i) => Ok(i.into_py(py)),       // PyLong_FromLongLong
        Value::UInt(u) => Ok(u.into_py(py)),       // PyLong_FromUnsignedLongLong
        Value::BigInt(i) => Ok((*i).into_py(py)),  // via num_bigint
        Value::Float(f) => Ok(f.into_py(py)),      // PyFloat_FromDouble
        Value::Bytes(b) => Ok(PyBytes::new_bound(py, b).into_any().unbind()),
        Value::String(s) => Ok(PyString::new_bound(py, s).into_any().unbind()),
        // Container/List should never reach here at leaf level.
        Value::Container(_) | Value::List(_) => {
            // Fallback for safety (e.g. Enum producing Container for FlagsEnum).
            crate::compiled::py_sink::value_to_py_composite_fallback(py, v)
        }
    }
}
```

**性能**：单标量 ~30-80ns（PyLong_FromLongLong ≈ 30ns，PyBytes::new ≈ 50ns，PyString::new ≈ 50ns）。对比旧 `value_to_py` 的标量路径：~50-200ns（因 import_bound 尝试 + 错误处理开销）。对比旧 `py_to_value` 的 7×类型链：200-500ns。

### 3.5 PyCompiledExec trait + leaf 实现

```rust
// construct-rs/src/compiled/py_exec.rs
#![cfg(feature = "python")]

use pyo3::prelude::*;
use crate::core::context::Context;
use crate::core::error::Result;
use crate::core::stream::CombinedStream;
use crate::compiled::py_sink::{PySink, value_to_py_scalar};

/// Python-direct execution trait (cfg-gated under "python" feature).
///
/// Each CompiledNode variant implements this trait for the Python path.
/// The Value-path `CompiledExec` trait is UNCHANGED — both coexist.
pub trait PyCompiledExec {
    /// Parses from stream, writing results directly to a PySink as PyObject.
    /// Value is used only as a transient scalar carrier (for ctx) and is
    /// immediately converted to PyObject — no Value output tree accumulates.
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()>;

    /// Builds binary data, reading from a PyInput (PyObject-based).
    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn crate::compiled::py_input::PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()>;
}
```

**Leaf 实现（embed-and-delegate + PyObject 直出）**：

```rust
/// Macro: generates PyCompiledExec for leaf nodes that embed a Construct.
///
/// The leaf calls self.inner.parse() (shared decode logic, returns Value),
/// then immediately converts Value → PyObject via value_to_py_scalar (single
/// match, no type chain), and deposits BOTH into the sink.
macro_rules! impl_leaf_py_exec {
    ($($ty:ty),* $(,)?) => {
        $(
            impl PyCompiledExec for $ty {
                fn exec_parse_py(
                    &self,
                    py: Python<'_>,
                    stream: &mut CombinedStream,
                    ctx: &mut Context,
                    sink: &mut dyn PySink,
                ) -> Result<()> {
                    // Shared decode: returns scalar Value (cheap, ~5ns).
                    let value = self.inner.parse(stream, ctx)?;
                    // One C API call (type known from Value variant).
                    let obj = value_to_py_scalar(py, &value)
                        .map_err(py_err_to_construct)?;
                    // Deposit both: Value for ctx, PyObject for output.
                    sink.deposit_leaf(py, value, obj)
                }

                fn exec_build_py(&self, ...) -> Result<()> {
                    // See §4 for build direction design.
                    ...
                }
            }
        )*
    };
}

impl_leaf_py_exec! {
    CompiledFormatField, CompiledBytes, CompiledGreedyBytes, CompiledBytesExpr,
    CompiledBytesInteger, CompiledBitsInteger, CompiledVarInt, CompiledZigZag,
    CompiledFlag, CompiledCString, CompiledPaddedString,
    CompiledPass, CompiledTerminated, CompiledTell, CompiledSeek,
    CompiledSeekExpr, CompiledError,
}
```

**关于"不经过 Value"的精确含义**：

leaf 调用 `self.inner.parse()` 产出一个**标量 Value**（如 `Value::Int(42)`）。这个 Value 是：
- 寄存器级瞬态（~5ns 构造），不是树节点
- 立即被消费（转 PyObject + 存 ctx）
- **从不累积成输出树**

这满足 §0 C3 的精神："Value 不应出现在 Python 关键路径上"——关键路径指 Value→PyObject 转换层和 Value 输出树累积。标量瞬态 Value 是 decode 逻辑的天然产物，消除它需要重写所有 leaf 的 decode 逻辑（产出裸 i64/f64 而非 Value），代价高、收益 <5%（详见 §6.6 可选精化）。

**Composite 实现（CompiledStruct 示例）**：

```rust
impl PyCompiledExec for CompiledStruct {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let mut child_ctx = ctx.subcontext();
        for field in &self.fields {
            match &field.name {
                Some(name) => {
                    let mut sub = sink.sub_py_field(name)?;
                    match field.subcon.exec_parse_py(
                        py, stream, &mut child_ctx, sub.as_mut(),
                    ) {
                        Ok(()) => {}
                        Err(ConstructError::StopField { .. }) => break,
                        Err(e) => return Err(e.with_path_prefix(name)),
                    }
                    // finish_field_py: ONE PyDict_SetItem + returns Value for ctx.
                    let value = sink.finish_field_py(py, name, sub)?;
                    child_ctx.insert(name.clone(), value);
                }
                None => {
                    let mut sub = sink.sub_py_item()?;
                    match field.subcon.exec_parse_py(
                        py, stream, &mut child_ctx, sub.as_mut(),
                    ) {
                        Ok(()) => sink.finish_item_py(py, sub)?,
                        Err(ConstructError::StopField { .. }) => break,
                        Err(e) => return Err(e.with_path_prefix("(anonymous)")),
                    }
                }
            }
        }
        Ok(())
    }
    // exec_build_py: see §4
}
```

### 3.6 Dispatch 函数（宏生成 71-arm match）

```rust
/// Dispatches exec_parse_py over all CompiledNode variants.
///
/// Generated by macro to ensure completeness: if a new variant is added
/// to CompiledNode without a PyCompiledExec impl, this fails to compile.
#[cfg(feature = "python")]
pub fn exec_parse_py_dispatch(
    node: &CompiledNode,
    py: Python<'_>,
    stream: &mut CombinedStream,
    ctx: &mut Context,
    sink: &mut dyn PySink,
) -> Result<()> {
    match node {
        CompiledNode::FormatField(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Bytes(n) => n.exec_parse_py(py, stream, ctx, sink),
        // ... 全部 71 变体 ...
        CompiledNode::Struct(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Dynamic(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::External(n) => n.exec_parse_py(py, stream, ctx, sink),
    }
}
```

> **实现说明**：dispatch 使用宏 `py_dispatch_match!` 生成，变体列表从 `CompiledNode` enum 定义提取。CODING 阶段确认 enum_dispatch 是否支持双 trait；若不支持，使用此宏方案。

### 3.7 逃生舱路径：CompiledDynamic / CompiledExternal

| 节点 | Py 路径行为 | 理由 |
|------|-----------|------|
| `CompiledDynamic` | 调 `inner.parse()` → Value → `value_to_py_scalar`/`value_to_py_composite_fallback` → deposit | Dynamic 持有 `Arc<dyn Construct>`（声明树），其 parse 必然返回 Value。这是逃生舱，非热路径。 |
| `CompiledExternal` | 调 `ext_parse()` → `ProducedOutput`；若 Value 则转 PyObject deposit，若 Object 则直接 deposit | Phase 13 Python 回调扩展点。回调产出 Value 或 PyObject，统一适配。 |
| Adapter/Validator 等 wrapper | 调 inner.exec_parse_py 得 sub-sink 结果，应用 decode/check 后重新 deposit | 复用 `exec_parse_inner_value` 的 Py 版本（sub-sink 用 PyDictSink2）。 |

---

## §4 build 方向：PyObject 直接消费

### 4.1 当前瓶颈（B2 + B3 回顾）

当前 build 方向有两个致命瓶颈：

```
build_from_raw (compiled_ext.rs:190):
  input.as_value()  ← 全量 py_to_value(dict) [B2 第一次]

CompiledStruct::exec_build (mod.rs:1764):
  input.as_value()  ← 全量 py_to_value(dict) [B2 第二次！]
  遍历字段: OwnedValueInput::new(value.clone()) → child.exec_build
```

每次 `as_value()` 对整个 dict 执行 `py_to_value`，内含 B3（每对象 7×类型检查）。3 字段 dict ≈ 21 FFI/次，执行 2 次 = 42 FFI。100 元素 array ≈ 319 FFI/次，执行 2 次 = 638 FFI。

### 4.2 PyInput trait 设计（PyObject 直接读取）

```rust
// construct-rs/src/compiled/py_input.rs
#![cfg(feature = "python")]

use pyo3::prelude::*;
use crate::core::error::{ConstructError, Result};
use crate::value::Value;

/// Python-direct input source for build direction.
///
/// Unlike the old PyInput (which converts to Value via py_to_value on every
/// access), PyInput provides:
/// - Per-field PyObject access (no full-tree conversion)
/// - Schema-guided extraction at leaf level (type-known extract, no type chain)
pub trait PyInput {
    /// Extracts this input as a scalar Value for leaf build.
    ///
    /// Leaf nodes call this to get the value to encode. Internally uses
    /// type-known extract (e.g. extract::<i64>) — NOT the generic 7x type
    /// chain. Returns Value for compatibility with inner.build(&Value).
    ///
    /// # Errors
    /// Returns ConstructError::TypeMismatch on extraction failure.
    fn extract_scalar_py(&self, py: Python<'_>) -> Result<Value>;

    /// Returns whether a named field is present and non-None.
    fn has_field_py(&self, py: Python<'_>, name: &str) -> bool;

    /// Returns a sub-input for a named field (Struct build).
    fn sub_field_py(&self, py: Python<'_>, name: &str) -> Result<Box<dyn PyInput>>;

    /// Returns a sub-input for a sequential index (Array build).
    fn sub_index_py(&self, py: Python<'_>, index: usize) -> Result<Box<dyn PyInput>>;

    /// Returns the number of items (for repetition constructs).
    fn len_py(&self, py: Python<'_>) -> usize;

    /// Schema-guided batch extraction for homogeneous arrays.
    ///
    /// If the compiled node knows its element type (e.g. Array of Int32ub),
    /// this extracts all elements in one C call (e.g. extract::<Vec<i64>>),
    /// avoiding per-element FFI. Returns None if batch extraction is not
    /// applicable (heterogeneous or non-numeric elements).
    fn extract_batch_int_py(&self, py: Python<'_>) -> Result<Option<Vec<i64>>>;
}
```

### 4.3 Leaf exec_build_py（类型已知 extract）

```rust
/// Macro: generates PyCompiledExec::exec_build_py for integer leaf nodes.
///
/// Uses type-known extract (extract::<i64/u64>) — ONE C call, no type chain.
macro_rules! impl_int_leaf_build_py {
    ($ty:ty, $extract_fn:expr) => {
        impl PyCompiledExec for $ty {
            fn exec_build_py(&self, py, input, stream, ctx) -> Result<()> {
                // Type-known extract: ONE C call (not 7x is_instance_of chain).
                let value = input.extract_scalar_py(py)?;
                self.inner.build(&value, stream, ctx)
            }
            // exec_parse_py inherited from impl_leaf_py_exec!
        }
    };
}
```

**对比旧路径**：

| 步骤 | 旧路径 (PyInput::as_value) | 新路径 (extract_scalar_py) |
|------|---------------------------|---------------------------|
| 类型探测 | 7× is_instance_of (None→Bool→BigInt→Float→Bytes→String→List→Dict) | 0（类型已知，直接 extract） |
| FFI 次数/标量 | ~7 | **1** (extract::<i64>) |
| 单标量耗时 | 200-500ns | **30-50ns** |

### 4.4 Composite exec_build_py（CompiledStruct，消除 B2）

```rust
impl PyCompiledExec for CompiledStruct {
    fn exec_build_py(&self, py, input, stream, ctx) -> Result<()> {
        let mut child_ctx = ctx.subcontext();

        // NO as_value() — no full dict conversion! Each field extracted individually.
        for field in &self.fields {
            let name_ref = field.name.as_deref();

            // Per-field sub-input (one PyDict_GetItem, no full conversion).
            let build_value = if field.flagbuildnone {
                if name_ref.map_or(false, |n| input.has_field_py(py, n)) {
                    let sub = input.sub_field_py(py, name_ref.unwrap())?;
                    sub.extract_scalar_py(py)?
                } else {
                    Value::None
                }
            } else {
                match name_ref {
                    Some(n) => {
                        if !input.has_field_py(py, n) {
                            return Err(ConstructError::FieldMissing {
                                path: String::new(),
                                field: n.to_string(),
                            });
                        }
                        let sub = input.sub_field_py(py, n)?;
                        sub.extract_scalar_py(py)?
                    }
                    None => Value::None,
                }
            };

            if let Some(ref name) = field.name {
                child_ctx.insert(name.clone(), build_value.clone());
            }

            // Delegate to child's exec_build_py with a per-field PyInput.
            let field_input = input.sub_field_py(py, name_ref.unwrap_or(""))?;
            match field.subcon.exec_build_py(py, field_input, stream, &mut child_ctx) {
                Ok(()) => {}
                Err(ConstructError::StopField { .. }) => return Ok(()),
                Err(e) => return Err(e.with_path_prefix(name_ref.unwrap_or("(anonymous)"))),
            }
        }
        Ok(())
    }
}
```

**B2 消除验证**：

| | 旧路径 | 新路径 |
|---|--------|--------|
| `build_from_raw` 的 as_value | 全量 py_to_value(dict) ≈ 21 FFI (3字段) | **消除**（无 as_value） |
| `CompiledStruct::exec_build` 的 as_value | 重复全量 py_to_value ≈ 21 FFI | **消除**（per-field extract） |
| 字段提取 FFI (3字段) | 0（从已转 Container 读） | 3× extract ≈ 3 FFI |
| **总计 (3字段)** | **~42 FFI** | **~3 FFI** (-93%) |
| **总计 (100元素 array)** | **~638 FFI** | **~101 FFI** (裸 API batch extract, 见 §4.5.1) (-84%) |

> **[REV 检视修正 P-CRIT-1]** 原 "~2 FFI" 已修正为 ~101（裸 CPython API 仍为 per-element）。
> 但相对旧路径 638 FFI 仍有 ~84% 降幅（主要来自消除双重 py_to_value + 类型链）。
> B2（双重全量转换）和 B3（类型检查链）仍完全消除。

### 4.5 Schema-guided batch extraction（Array build 优化）

> **[REV 检视修正 P-CRIT-1]** 本节原始版本声称 `extract::<Vec<i64>>` 是"一次 C 调用遍历 CPython list 内部数组"。
> REV 实证 pyo3 0.22.6 源码（`types/sequence.rs:494-529`）确认这是 **per-element 迭代**：
> ```rust
> for item in seq.iter()? {           // per-element PySequence_GetItem
>     v.push(item?.extract::<T>()?);  // per-element PyLong_AsLongLong
> }
> ```
> 100 元素实际 ~202 次 C 调用（非 1 次），耗时 ~3-5µs（非 ~500ns）。
> 以下修正为裸 CPython API 方案（绕过 pyo3 抽象开销）。

对于同构数值数组（如 `Array(100, Int32ub)`），编译树知道元素类型。`CompiledArray::exec_build_py` 可调用 `extract_batch_int_py` 提取整个 `Vec<i64>`。

**关键修正**：pyo3 的 `extract::<Vec<i64>>` 本身是 per-element 的（~202 C 调用/100 元素）。batch extract 相对 per-index 的真实优势**不是**"~100x FFI 减少"，而是：
1. **避免 per-element `Box<dyn PyInput>` 分配 + vtable 分派**（B7，~20ns/元素 = 2µs/100 元素）
2. **避免 per-element `PySequence_GetItem`（方法解析）**，改用 `PyList_GET_ITEM` 宏（直接数组访问，~2ns）
3. 更好的缓存局部性（连续遍历 list 内部 `ob_item` 数组）

因此 batch extract 相对 per-index 的真实加速比约 **~2-2.5x**（来自避免 B7 + 用宏替代方法解析），而非声称的 ~100x。

#### 4.5.1 首选方案：裸 CPython API 手动循环（绕过 pyo3 extract 抽象）

```rust
impl PyCompiledExec for CompiledArray {
    fn exec_build_py(&self, py, input, stream, ctx) -> Result<()> {
        // FAST PATH: homogeneous int array — 裸 CPython API 批量提取。
        if let Some(elements) = input.extract_batch_int_py(py)? {
            if elements.len() != self.count {
                return Err(ConstructError::Array { /* ... */ });
            }
            for (i, elem) in elements.iter().enumerate() {
                ctx.insert(REPETITION_INDEX_KEY, Value::UInt(i as u64));
                let v = Value::Int(*elem);
                let elem_input = PyScalarInput::new(v); // 包装已提取的 Value
                self.subcon.exec_build_py(py, &elem_input, stream, ctx)?;
            }
            return Ok(());
        }

        // FALLBACK: 异构元素 — per-index extract。
        for i in 0..self.count {
            ctx.insert(REPETITION_INDEX_KEY, Value::UInt(i as u64));
            let sub = input.sub_index_py(py, i)?;
            self.subcon.exec_build_py(py, sub, stream, ctx)?;
        }
        Ok(())
    }
}
```

**`extract_batch_int_py` 实现（裸 CPython API）**：

> **[REV 检视修正 P-CRIT-1]** 首选使用裸 CPython API 而非 pyo3 `extract::<Vec<i64>>`。
> 理由：pyo3 extract 经过 `PySequence_Check` + `PySequence_GetItem`（方法解析，~25ns/元素）+
> `extract::<i64>`（类型检查封装，~40ns/元素）。裸 API 直接用 `PyList_GET_ITEM` 宏
> （~2ns）+ `PyLong_AsLongLong`（~30ns），省去方法解析和类型封装。

```rust
/// 裸 CPython API 批量提取 Vec<i64>。
///
/// 使用 PyList_GET_SIZE + PyList_GET_ITEM(宏) + PyLong_AsLongLong，
/// 绕过 pyo3 的 PySequence 协议和 extract 抽象开销。
///
/// 优势 vs pyo3 extract::<Vec<i64>>：
/// - PyList_GET_ITEM 是宏（直接 ob_item 数组访问，~2ns）vs PySequence_GetItem（~25ns）
/// - PyLong_AsLongLong 相同（~30ns），但无 extract 类型封装开销
/// - 优势 vs per-index sub_index_py：避免 100× Box<dyn PyInput> + vtable（B7，~2µs）
///
/// # Errors
/// 若输入不是 list、元素不是 int、或发生溢出，返回 Err（调用方退化到 per-index）。
unsafe fn extract_batch_int_raw(py: Python<'_>, obj: &Bound<PyAny>) -> Result<Option<Vec<i64>>> {
    use pyo3::ffi::*;
    let ptr = obj.as_ptr();
    // 仅处理原生 list（PyList_Check 是宏，接受子类如 ListContainer）
    if PyList_Check(ptr) == 0 {
        return Ok(None); // 非 list → 调用方退化到 per-index
    }
    let len = PyList_GET_SIZE(ptr) as usize;
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        let item = PyList_GET_ITEM(ptr, i as Py_ssize_t);
        // PyLong_AsLongLong 对非 int 返回 -1 + PyErr — 退化到 per-index
        if PyErr_Occurred() != std::ptr::null_mut() {
            return Ok(None);
        }
        let val = PyLong_AsLongLong(item);
        if val == -1 && PyErr_Occurred() != std::ptr::null_mut() {
            PyErr_Clear();
            return Ok(None); // 溢出或非 int → 退化
        }
        out.push(val);
    }
    Ok(Some(out))
}
```

#### 4.5.2 100 元素 int array build FFI 对比（修正后）

> **[REV 检视修正 P-CRIT-1]** 修正 FFI 计数：batch extract 实际为 per-element（裸 API ~101 C 函数调用
> 含 100× PyLong_AsLongLong；PyList_GET_ITEM/GET_SIZE 是宏不计 C 调用），非原设计的 ~2。

| 方案 | C 函数调用次数 | 耗时估算 | vs Python ~30µs | 备注 |
|------|--------------|---------|-----------------|------|
| 旧路径 (双重 py_to_value) | ~638 | ~750µs | 0.04x | 含 7× 类型链/元素 |
| per-index extract (PyInput trait) | ~300 | ~20-25µs | ~1.2-1.5x | 含 100× Box<dyn> + vtable |
| pyo3 extract::<Vec\<i64\>> | ~202 | ~5-8µs | ~4-6x | PySequence_GetItem 方法解析开销 |
| **裸 CPython API batch** | **~101** | **~3.5-4µs** | **~7-8x** | PyList_GET_ITEM 宏 + PyLong_AsLongLong |

> **batch vs per-index 真实优势**：~2-2.5x（来自消除 B7 Box/vtable + 宏替代方法解析），
> 非原设计的 ~100x FFI 减少。但 ~7-8x vs Python 原版仍远超 ≥1.0x 目标。

### 4.6 build 方向 `context._` 处理（浅转换方案）

> **[REV 检视修正 C-CRIT-2]** 原设计 §4 的 PyInput trait **没有 `as_value()` 方法**，
> 新 `build_from_raw` 如何设置 `ctx["_"]` 未定义。
> 当前 `compiled_ext.rs:190` 通过 `input.as_value()` 设置 `ctx["_"]`（供
> `IfThenElse(this._ == ...)` 等条件构造使用）。

**问题背景**：Python 原版 construct 在 build 时，`context._` 指向当前正在构建的对象，
供表达式 `this._` 引用（如 `IfThenElse(this._.type == 1, ...)`）。当前 Rust 实现
（`compiled_ext.rs:190`）在 root 入口设置 `ctx["_"] = py_to_value(input)`（完整递归转换）。

新设计消除了 `as_value()`（per-field extract 替代全量转换），因此需要明确 `ctx["_"]`
的设置方案。选项分析：

| 选项 | 描述 | 评估 |
|------|------|------|
| a. 入口处全量 `py_to_value`（仅用于 ctx["_"]） | 恢复 B2 第一次转换 | ❌ array build 需递归转换 100 元素（~30µs），破坏性能 |
| b. 跳过 ctx["_"] | 不设置 | ❌ 破坏 `IfThenElse(this._ == ...)` 正确性 |
| c. **入口处浅转换（非递归，仅 root 标量字段）** | 见下方方案 | **✅ 选定** |
| d. PyInput 增加 `as_value_for_ctx()` | 仍需全量转换 | ❌ 同 a |
| e. Context 支持 PyObject 引用 | 扩展 Value 枚举 | ❌ 侵入性大，污染 Value 路径 |

**选定方案 c：浅转换（shallow py_to_value）**

在 `build_from_raw` 入口处，对 root 输入做**一层深度**的转换：
- 对 dict 输入：提取每个 key，对 value 尝试标量 extract（int/float/bool/bytes/string/None）
- 对非标量 value（嵌套 dict/list）：存为**浅占位符**（`Value::Container(empty)` / `Value::List(empty)`），**不递归**

**关键性质**：成本为 `O(root_scalar_fields)` 而非 `O(total_elements)`。
- simple struct (3 标量)：3 × ~50ns = ~150ns
- array (count 标量 + items list)：1 × ~50ns + 1 占位符 ~30ns = ~80ns
- nested (header dict + data bytes)：1 占位符 ~30ns + 1 × ~50ns = ~80ns

```rust
/// 浅转换 root dict 为 Value::Container，用于 ctx["_"]。
///
/// 仅提取顶层标量字段；嵌套 dict/list 存为空占位符。
/// 这使得 `this._.scalar_field` 表达式正确工作，
/// 而 `this._.nested.deep_field` 返回空占位（已知限制，见下）。
///
/// # 成本
/// O(root_scalar_fields) ~ 50ns/标量字段 + 30ns/嵌套占位符。
/// 对 array (100 元素) 仅 ~80ns（不递归进 list），vs 全量转换 ~30µs。
fn shallow_py_to_value_for_ctx(py: Python<'_>, obj: &Bound<PyAny>) -> Value {
    // 尝试作为 dict 处理（Container/dataclass 经 asdict 后也是 dict）
    if let Ok(dict) = obj.downcast::<PyDict>() {
        let mut map = IndexMap::new();
        for (key, value) in dict.iter() {
            let key_str: String = match key.extract() {
                Ok(s) => s,
                Err(_) => continue,
            };
            // 标量：快速 extract（无 7× 类型链，按优先级短路）
            let val = shallow_extract_scalar(py, &value)
                .unwrap_or_else(|| {
                    // 嵌套 dict/list：存空占位符（不递归）
                    if value.is_instance_of::<PyDict>().unwrap_or(false) {
                        Value::Container(IndexMap::new())
                    } else if value.is_instance_of::<PyList>().unwrap_or(false) {
                        Value::List(Vec::new())
                    } else {
                        Value::None
                    }
                });
            map.insert(key_str, val);
        }
        Value::Container(map)
    } else if let Ok(list) = obj.downcast::<PyList>() {
        // root 是 list：存空占位符（不递归元素）
        Value::List(Vec::new())
    } else {
        // root 是标量：直接 extract
        shallow_extract_scalar(py, obj).unwrap_or(Value::None)
    }
}

/// 按优先级短路提取标量（bool→int→float→bytes→string→None），
/// 每类仅 1 次 is_instance_of 检查，最多 6 次（vs py_to_value 的 7+ 次）。
fn shallow_extract_scalar(py: Python<'_>, obj: &Bound<PyAny>) -> Option<Value> {
    if obj.is_none() { return Some(Value::None); }
    if let Ok(b) = obj.extract::<bool>() { return Some(Value::Bool(b)); }
    if let Ok(i) = obj.extract::<i64>() { return Some(Value::Int(i)); }
    if let Ok(u) = obj.extract::<u64>() { return Some(Value::UInt(u)); }
    if let Ok(f) = obj.extract::<f64>() { return Some(Value::Float(f)); }
    if let Ok(b) = obj.extract::<&[u8]>() { return Some(Value::Bytes(b.to_vec())); }
    if let Ok(s) = obj.extract::<String>() { return Some(Value::String(s)); }
    None
}
```

**`build_from_raw` 入口整合**：

```rust
pub fn build_from_raw(&self, py: Python<'_>, obj: &Bound<PyAny>, kw: IndexMap<String, Value>) -> PyResult<PyObject> {
    let input = PyDictInput::from_bound(py, obj);  // 新 PyInput 实现
    let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
    let mut ctx = Context::new();
    ctx.insert("_parsing", Value::Bool(false));
    ctx.insert("_building", Value::Bool(true));
    ctx.insert("_sizing", Value::Bool(false));
    // [C-CRIT-2] ctx["_"] = 浅转换（非递归），供 this._ 表达式使用
    ctx.insert("_", shallow_py_to_value_for_ctx(py, obj));
    for (key, value) in kw {
        ctx.insert(key, value);
    }
    // exec_build_py_dispatch 使用 per-field extract（无 as_value 全量转换）
    match exec_build_py_dispatch(self.schema.tree(), py, &input, &mut stream, &mut ctx) {
        Ok(()) => { /* ... */ }
        Err(e) => Err(rust_err_to_py(py, e.with_path_prefix(BUILD_PATH))),
    }
}
```

**已知限制（Phase 16 scope）**：

| 场景 | 浅转换行为 | 正确性 | 影响 |
|------|-----------|--------|------|
| `this._.scalar_field == val` | ✅ 正确提取标量 | ✅ 正确 | 常见模式 |
| `this._.nested_struct.field` | ⚠️ nested 存为空 Container | ❌ 不正确 | 罕见模式 |
| `this._.list_field[0]` | ⚠️ list 存为空 List | ❌ 不正确 | 罕见模式 |
| `this._.count > 0` | ✅ 正确提取标量 | ✅ 正确 | 常见模式 |

> **正确性影响**：基准格式（simple/nested/array）不使用 `this._`，性能不受影响。
> 61 个 dataclass 测试中若有使用 `this._.nested.field` 深层引用的，可能失败。
> 这类失败应在 16.4 验收时识别；若有，Phase 16 限制为"root 标量 this._ 支持"，
> 深层引用可在 Phase 17 通过 Context 支持 PyObject 引用（选项 e）解决。
>
> **为何不在 Phase 16 做全量转换**：全量 `py_to_value` 对 array build 的 100 元素
> 需 ~30µs，使 array build 从 ~5.6µs（~5.4x）退回 ~35µs（~0.85x），**直接不达标**。
> 浅转换将开销限制在 ~80-150ns，保持 array build ≥4.0x。

---

## §5 Value 路径保留策略

### 5.1 双路径共存架构

```
┌─────────────────────────────────────────────────────┐
│                  CompiledNode (71 变体)              │
│                                                     │
│  ┌─────────────────────┐  ┌──────────────────────┐ │
│  │  CompiledExec       │  │  PyCompiledExec      │ │
│  │  (Value 路径)        │  │  (PyObject 路径)      │ │
│  │  #[cfg(always)]     │  │  #[cfg(python)]      │ │
│  │                     │  │                      │ │
│  │  exec_parse         │  │  exec_parse_py       │ │
│  │  exec_build         │  │  exec_build_py       │ │
│  │  → ValueSink        │  │  → PySink (PyDict)   │ │
│  │  → ValueInput       │  │  → PyInput (PyObject)│ │
│  └─────────┬───────────┘  └──────────┬───────────┘ │
│            │                         │             │
│     cargo test (1515)         maturin develop      │
│     (无 python feature)        (python feature)    │
└─────────────────────────────────────────────────────┘
```

### 5.2 共享逻辑

两条路径共享以下不变的核心逻辑：

| 共享组件 | 位置 | 说明 |
|---------|------|------|
| 编译树结构 (CompiledNode + 71 变体) | `compiled/mod.rs` | 完全共享，零修改 |
| SchemaCompiler | `compiler/` | 完全共享 |
| 流读写 (ByteStream / CombinedStream) | `core/stream.rs` | 完全共享 |
| Context (IndexMap + parent) | `core/context.rs` | 完全共享（ctx 内部仍用 Value） |
| Leaf decode/encode 逻辑 | 各 construct 的 `parse`/`build` | Value 路径直接用；Py 路径通过 `inner.parse()` 复用 |
| 错误类型 (ConstructError) | `core/error.rs` | 完全共享 |

### 5.3 Value 在新架构中的角色

| 场景 | Value 是否参与 | 说明 |
|------|--------------|------|
| 纯 Rust 单元测试 (1515) | ✅ 完整参与 | exec_parse/exec_build 不变 |
| Python parse（leaf 标量） | ⚠️ 瞬态参与 | leaf parse 返回标量 Value，立即转 PyObject + 存 ctx |
| Python parse（ctx 表达式） | ✅ 参与 | Context 持有 Value 供 CompiledExpr 求值 |
| Python parse（输出） | ❌ 不参与 | 输出是 PyObject 树，不是 Value 树 |
| Python build（leaf 标量） | ⚠️ 瞬态参与 | extract 后包装为 Value 供 inner.build 使用 |
| Python build（ctx 表达式） | ✅ 参与 | 同 parse |
| Dynamic/External 逃生舱 | ✅ 完整参与 | 回调产出 Value，转换后 deposit |

**结论**：Value 从"产出载体"降级为"内部瞬态标量 + ctx 载体"。不再有 Value 输出树、不再有 Value→PyObject 批量转换层。

### 5.4 旧 PyDictSink / PyInput / conversions.rs 的命运

| 文件 | 当前角色 | Phase 16 处置 |
|------|---------|-------------|
| `construct-py/src/py_sink.rs` (PyDictSink) | Python parse sink | **废弃**：被 construct-rs 的 `PyDictSink2` 替代。删除或标记 `#[deprecated]` |
| `construct-py/src/py_input.rs` (PyInput) | Python build input | **废弃**：被 construct-rs 的 `PyInput` trait + impl 替代 |
| `construct-py/src/conversions.rs` (788 行) | Value↔PyObject 双向转换 | **大幅缩减**：`value_to_py` 仅保留给 Dynamic 逃生舱（Container/List 包装现由 PySink §3.3.1 直接处理，不再经 conversions.rs）；`py_to_value` 仅保留给 kw 参数转换 + ctx["_"] 浅转换（§4.6）+ Dynamic 回退。预计缩减至 ~200 行 |
| `construct-rs/src/compiled/sink.rs` (ValueSink) | 纯 Rust 测试 sink | **保留不变** |
| `construct-rs/src/compiled/input.rs` (ValueInput) | 纯 Rust 测试 input | **保留不变** |

---

## §6 性能假设

> 本节满足 `.opencode/skills/performance-gate.md` Checkpoint 1 要求。

### 6.1 瓶颈识别（全部来源，量化）

#### 数据来源

1. **实测绝对数据**（2026-06-19，PM 提供，5000 次迭代，`test_perf_comparison.py`，vs Python 原版 construct 2.10.70）：见 §1.2 表格。
2. **FFI 单次开销经验值**（来自旧设计 `模块设计-FFI批量化优化.md` §4.4 实测）：
   - `PyLong_FromLongLong` (int into_py): ~30ns
   - `PyDict_SetItem`: ~30ns
   - `PyList_Append`: ~30ns
   - `py_to_value` 单对象（含 7×类型链）: ~200-500ns
   - `value_to_py` 单标量（含 import_bound 尝试）: ~100-200ns
   - `Python::with_gil` (GIL 已持有时): ~20-40ns
   - `Box::new` + vtable dispatch: ~15-25ns
3. **FFI 调用计数**（代码审查，确定性推导）：见旧设计 §瓶颈识别 + 下方补充。

#### 全部瓶颈来源清单

| 编号 | 瓶颈 | 来源 | 影响（当前实测） |
|------|------|------|----------------|
| **B1** | Parse round-trip：`PyDictSink::finish_field` Object 分支对嵌套容器执行 `py_to_value`（PyObject→Value）逆向转换，仅为获取 context 值 | py_sink.rs:294-306 | array parse 贡献 204/407 = **50% FFI** |
| **B2** | Build 双重转换：`build_from_raw` 和 `CompiledStruct::exec_build` 各调一次 `as_value()`/`py_to_value` 全量转换 | compiled_ext.rs:190 + mod.rs:1764 | simple build **50% FFI**；array build **50% FFI** |
| **B3** | 类型检查链：`py_to_value` 对每个 Python 对象执行 7 次 `is_instance_of` | conversions.rs:110-213 | 每对象 200-500ns，build 方向主导 |
| **B4** | 重复 import_bound：`value_to_py` 对 Container/List 每次调用 `import_bound("construct_rust.lib.containers")` | conversions.rs:62-64, 81-84 | parse 方向每 Container/List ~200ns |
| **B5** | Value 输出树累积：构建 `Value::Container(IndexMap)` / `Value::List(Vec)` 然后递归 `value_to_py` 遍历 | sink.rs ValueSink + conversions.rs | 分配 + 遍历开销，~10ns/字段 |
| **B6** | Per-field `Python::with_gil`：`PyDictSink` 的 `set_field`/`push_item`/`finish_field` 每次都调 `Python::with_gil` | py_sink.rs 全文 | 每字段 ~20-40ns |
| **B7** | `Box<dyn OutputSink>` 虚分派 + 堆分配：每个 `sub_sink_for_field`/`sub_sink_for_item` 分配 Box + vtable | sink.rs trait object | 每字段 ~20ns |
| **B8** | `ProducedOutput` 装箱/拆箱：`Box<dyn Any + Send + Sync>` 用于跨 crate 传递 PyObject | sink.rs:44-50 | 每复合节点 ~30ns + downcast |

### 6.2 机制说明（因果链：本设计如何消除每个瓶颈）

| 瓶颈 | 当前机制 | 本设计机制 | 消除方式 |
|------|---------|-----------|---------|
| **B1** | `finish_field` Object 分支 `py_to_value` 回转 | `PySink::finish_field_py` 子 sink 同时携带 Value(ctx) + PyObject(output)，直接返回 Value，**零 round-trip** | PySink 双产物设计 §3.2-3.3 |
| **B2** | `build_from_raw` + `exec_build` 各调 `as_value()` 全量转换 | `exec_build_py` 用 per-field `sub_field_py` + `extract_scalar_py`，**无 as_value**，无全量转换 | PyInput trait §4.2-4.4 |
| **B3** | `py_to_value` 通用分发（7×is_instance_of） | build: 类型已知 `extract::<i64>()`（1 C call）；parse: `value_to_py_scalar` 单 match（1 C call） | §3.4 + §4.3 |
| **B4** | `value_to_py` 每次 import_bound | Container/List 类引用在 root sink 创建时获取**一次**；标量路径不经 import | §3.3 root sink |
| **B5** | Value 输出树 + 递归 value_to_py | 输出是 PyObject 树（PyDict/PyList 直接构建）；Value 仅作 ctx 瞬态标量 | §3 PySink |
| **B6** | Per-field Python::with_gil | `py: Python<'py>` 从入口 threading 一次，所有方法接收引用，**零 with_gil** | §3.2 trait 签名 |
| **B7** | Box<dyn OutputSink> per field | PySink 仍用 Box（B7 影响 ~20ns/字段，非主导；Phase 17 可改为栈式 sink） | 保留，标记 Phase 17 优化 |
| **B8** | ProducedOutput Box<dyn Any> | PySink 直接返回 PyObject，无 Box<dyn Any> | §3.2 into_root_py |

> **B7 保留理由**：B7 每字段 ~20ns，在 simple(3字段)=60ns、array(100元素)=2µs 中均非主导（B1-B6 各贡献 100ns-200µs）。改为栈式 sink 需重构 sub-sink 生命周期，风险高收益低，留待 Phase 17。

### 6.3 可证伪预测

#### 预测 P1：simple parse/build ≥3.0x（高置信度）

> **[REV 检视修正 C-CRIT-1/C-CRIT-2]** 修正：parse 方向新增 Container 包装开销
> （call0 替代 PyDict::new，净增 ~120ns/复合节点）；build 方向新增 ctx["_"] 浅转换
> 开销（~150ns，见 §4.6）。Container import 经 OnceLock 全局缓存后摊薄至 ~0。

**声明**：simple 格式（3 标量字段）parse 和 build 均达到 ≥3.0x vs Python 原版。

**量化推导**：

*Simple parse (3 int 字段，FormatField Int32ub ×3)：*

| 步骤 | 操作 | FFI 次数 | 耗时估算 |
|------|------|---------|---------|
| root Container 创建 | 1 cls.call0（替代 PyDict::new） | 1 | 150ns |
| 3× leaf decode | inner.parse (struct.unpack) | 0 (Rust) | 3×20ns = 60ns |
| 3× value_to_py_scalar | 3 PyLong_FromLongLong | 3 | 3×30ns = 90ns |
| 3× finish_field_py | 3 PyDict_SetItem（Container 是 dict 子类） | 3 | 3×30ns = 90ns |
| 3× ctx Value insert | IndexMap insert | 0 (Rust) | 3×10ns = 30ns |
| **合计** | | **7** | **~420ns** |

vs Python 3.0µs → **~7.1x** ✓（原设计 ~10x，因 Container call0 增加 ~120ns）

*Simple build (3 int 字段)：*

| 步骤 | 操作 | FFI 次数 | 耗时估算 |
|------|------|---------|---------|
| ctx["_"] 浅转换 | 浅 py_to_value（root dict，3 标量字段） | ~4 | ~150ns |
| 3× has_field_py | 3 PyDict_Contains | 3 | 3×25ns = 75ns |
| 3× sub_field_py | 3 PyDict_GetItem | 3 | 3×25ns = 75ns |
| 3× extract_scalar_py | 3 extract::<i64> | 3 | 3×40ns = 120ns |
| 3× encode + write | inner.build (byteorder) | 0 (Rust) | 3×15ns = 45ns |
| 1× PyBytes::new (output) | 1 PyBytes_FromStringAndSize | 1 | 50ns |
| **合计** | | **~14** | **~515ns** |

vs Python 2.8µs → **~5.4x** ✓（原设计 ~7.7x，因 ctx["_"] 浅转换增加 ~150ns）

#### 预测 P2：nested parse/build ≥2.0x（高置信度）

> **[REV 检视修正 C-CRIT-1/C-CRIT-2]** 修正：parse 新增 2× Container call0（净增 ~240ns）；
> build 新增 ctx["_"] 浅转换（~150ns）。

**声明**：nested 格式（2 级嵌套 Struct）parse 和 build 均达到 ≥2.0x。

**量化推导**（nested = Struct{ header: Struct{ magic: Int, count: Int }, data: Bytes(4) }，~6 字段）：

*Nested parse*：~6 标量字段 × ~100ns/字段 (FFI + Rust) ≈ 600ns + 2× Container call0(300ns，替代 2× PyDict::new) ≈ **~900ns**。vs Python ~4.0µs → **~4.4x** ✓

*Nested build*：~6 字段 × per-field extract (~140ns) ≈ 840ns + ctx["_"] 浅转换(150ns) + output(50ns) ≈ **~1040ns**。vs Python ~3.2µs → **~3.1x** ✓

> nested 的 PySink 嵌套 sub-sink 产生 2×Box (B7 ≈ 40ns)，已包含在估算中。

#### 预测 P3：array parse ≥2.0x（中高置信度）

> **[REV 检视修正 C-CRIT-1]** 修正：新增 Container + ListContainer 包装开销（2× call0 ~300ns）。

**声明**：array 格式（1 标量 + 100 元素 Int 数组）parse 达到 ≥2.0x。

**量化推导**：

| 步骤 | 操作 | FFI 次数 | 耗时估算 |
|------|------|---------|---------|
| root Container 创建 | 1 cls.call0 | 1 | 150ns |
| count 字段 finish_field_py | value_to_py + set_item | 2 | 60ns |
| 100× leaf decode | struct.unpack ×100 | 0 (Rust) | 100×20ns = 2µs |
| 100× value_to_py_scalar | PyLong ×100 | 100 | 100×30ns = 3µs |
| 100× PyList_Append (Container 是 list 子类) | append ×100 | 100 | 3µs |
| items ListContainer 创建 | 1 cls.call0 | 1 | 150ns |
| items finish_field_py | set_item (零 round-trip!) | 1 | 30ns |
| 100× ctx Value push | Vec push | 0 (Rust) | 100×5ns = 500ns |
| **合计 (append 路径)** | | **~206** | **~8.9µs** |

vs Python 27.7µs → **~3.1x** ✓

> **关键**：旧路径 array parse = 232µs（407 FFI），本设计 = ~8.9µs（~206 FFI）。**降幅 96%**。B1（204 FFI round-trip）完全消除。Container/ListContainer 包装仅增加 ~300ns（2× call0）。

#### 预测 P4：array build ≥4.0x（中置信度，依赖裸 CPython API batch extract）

> **[REV 检视修正 P-CRIT-1]** 原版本声称 batch extract 为 1 FFI/~500ns → ~11.5x。
> 修正：pyo3 extract 是 per-element；改用裸 CPython API 后 ~101 C 调用/~3.5-4µs → ~7-8x。
> 仍达 ≥4.0x 目标，但余量从原设计缩窄。

**声明**：array 格式 build 达到 ≥4.0x（需实现裸 CPython API batch extract，见 §4.5.1）。

**量化推导**：

| 步骤 | 操作 | C 调用次数 | 耗时估算 |
|------|------|----------|---------|
| count has_field + sub + extract | 3 (PyDict ops) | 3 | 90ns |
| items extract_batch_int_py | **裸 API**: GET_SIZE(宏) + 100× GET_ITEM(宏) + 100× AsLongLong | 101 | ~3.5µs |
| 100× encode + write | byteorder ×100 | 0 (Rust) | 100×15ns = 1.5µs |
| 100× ctx Value insert | Vec push | 0 (Rust) | 500ns |
| output PyBytes::new | 1 | 1 | 50ns |
| **合计** | | **~105** | **~5.6µs** |

vs Python ~30µs → **~5.4x** ✓（保守取裸 API 下限 ~4µs → ~7.5x 上限）

> **关键依赖**：`extract_batch_int_py` 必须使用裸 CPython API（§4.5.1），而非 pyo3
> `extract::<Vec<i64>>`（后者 per-element 方法解析，~5-8µs）。若裸 API 实现受阻，
> 退化到 per-index（~1.2-1.5x），geomean 仍 ≥1.0x（见 P5）。

#### 预测 P5：geomean（全部 6 场景）≥2.0x

> **[REV 检视修正 P-CRIT-1/C-CRIT-1/C-CRIT-2]** 修正后各预测：
> P1 parse ~7.1x / build ~5.4x；P2 parse ~4.4x / build ~3.1x；
> P3 parse ~3.1x；P4 build ~5.4x（裸 API）。

**声明**：simple/nested/array × parse/build 共 6 场景的几何平均 ≥1.0x（S-PERF-1），预期 ≥2.0x。

**量化**：geomean(7.1, 5.4, 4.4, 3.1, 3.1, 5.4) ≈ **4.6x**。即使 array build 裸 API 受阻退化到 per-index（~1.3x），geomean(7.1, 5.4, 4.4, 3.1, 3.1, 1.3) ≈ **3.6x**，仍远超 1.0x 目标。

> **保守下界**：若 Container call0 和 ctx["_"] 浅转换开销均被低估 2x（各 +300ns/+300ns），
> simple parse ~720ns(~4.2x) / build ~815ns(~3.4x)，geomean 仍 ≥2.5x。S-PERF-1 达标置信度高。

### 6.4 反例声明（预测为假时的应对）

| 如果实测... | 说明的问题 | 应对方案 |
|------------|-----------|---------|
| simple parse < 3.0x | value_to_py_scalar 或 PyDict_SetItem 开销被低估 | 检查 pyo3 into_py 实现；考虑用裸 `PyLong_FromLongLong` C API 替代 into_py |
| simple build < 2.0x | extract_scalar_py 的 extract 开销主导 | 检查 pyo3 extract 路径；考虑用裸 `PyLong_AsLongLong` 替代 |
| array parse < 1.5x | 100× PyLong 创建 + append 开销主导 | 必须实现 PyList batch 构建（`PyList::new_bound(py, &vec)`）；若仍不够，分析是否 PyLong 创建本身是瓶颈（不可避免） |
| **任意 build < 1.0x** | extract 类型探测仍有开销 / B7 Box 开销主导 | 实现 schema-guided typed extract（编译树携带 expected_type hint）；评估 B7 栈式 sink |
| array build < 1.0x | 裸 CPython API batch extract 未生效或 per-element PyLong_AsLongLong 主导 | **[P-CRIT-1 修正]** batch extract 已首选裸 API（§4.5.1）；若仍不达标，退化到 per-index extract（~1.2-1.5x，geomean 仍 ≥1.0x） |
| 整体 geomean < 1.0x | 设计存在未识别的系统性瓶颈 | 回 ARCH 做 cProfile + perf 分析，定位新的瓶颈来源 |

### 6.5 验证方法

| 预测 | 验证工具 | 验证时机 | 通过标准 |
|------|---------|---------|---------|
| P1 (simple) | `test_perf_comparison.py --format simple` | 16.3 完成后（首个端到端 Py 路径） | parse+build geomean ≥3.0x |
| P2 (nested) | `test_perf_comparison.py --format nested` | 16.3 完成后 | parse+build geomean ≥2.0x |
| P3 (array parse) | `test_perf_comparison.py --format array_heavy` | 16.4 完成后 | parse ≥2.0x |
| P4 (array build) | 同上 + 手动验证 batch extract 路径 | 16.4 完成后 | build ≥4.0x |
| P5 (geomean) | 全量 `test_perf_comparison.py -n 5000` | 16.5 验收 | geomean(6 场景) ≥1.0x（S-PERF-1） |
| 纯 Rust 基线不变 | `cargo test` (construct-rs, 无 python feature) | 每个子任务 | 1515 测试全 PASS |

**烟雾测试（Checkpoint 2）**：子任务 16.3（首个端到端 Py 路径 parse+build）完成后，PM 必须运行 `test_perf_comparison.py -n 1000`，若 simple parse < 1.0x → 🔴 红灯，暂停后续子任务，回 ARCH 分析。

### 6.6 可选精化（Phase 17 候选，不影响 Phase 16 达标）

| 精化 | 描述 | 预期收益 | Phase 16 是否需要 |
|------|------|---------|-----------------|
| ScalarValue 枚举 | leaf decode 返回 `ScalarValue`(i64/u64/f64/bytes/str) 而非 Value，完全消除瞬态 Value | ~5ns/字段 | ❌ 不需要（瞬态 Value ~5ns 远低于噪声） |
| 栈式 PySink | 用 arena/栈替代 per-field `Box<dyn PySink>` | ~20ns/字段 (B7) | ❌ 不需要（非主导瓶颈） |
| 裸 CPython API | 用 `PyLong_FromLongLong` 等裸 C API 替代 pyo3 `into_py` | ~10-20ns/调用 | ⚠️ **parse 标量路径仅在不达标时启用；build batch extract 已设为首选（§4.5.1，修正 P-CRIT-1）** |
| numpy frombuffer | Array(同构数值) 用 numpy 零拷贝 | array parse 额外 2-3x | ❌ 不需要（batch PyList 已够） |

---

## §7 对现有代码的影响分析

### 7.1 construct-rs（新增 Py 路径，Value 路径不变）

| 文件 | 改动类型 | 说明 |
|------|---------|------|
| `Cargo.toml` | **修改** | 新增 `pyo3 = { optional = true }` + `python = ["dep:pyo3"]` feature |
| `src/compiled/mod.rs` | **修改(小)** | 新增 `#[cfg(feature="python")] pub mod py_sink/py_input/py_exec;` 模块声明。CompiledNode/CompiledExec **零修改** |
| `src/compiled/py_sink.rs` | **新增** | PySink trait + PyDictSink2 实现 + value_to_py_scalar + value_to_py_composite_fallback |
| `src/compiled/py_input.rs` | **新增** | PyInput trait + PyDictInput/PyScalarInput 实现 + extract_batch_int_py |
| `src/compiled/py_exec.rs` | **新增** | PyCompiledExec trait + 各变体 impl（宏生成 leaf/wrapper/composite）+ dispatch 函数 |
| `src/compiled/sink.rs` | **零修改** | ValueSink + OutputSink 不变（纯 Rust 测试依赖） |
| `src/compiled/input.rs` | **零修改** | ValueInput + Input 不变 |
| `src/compiled/build.rs` | **零修改** | BuildConstruct (compile 逻辑) 不变 |
| `src/core/**` | **零修改** | Context/Stream/Error/Construct 不变 |
| `src/value.rs` | **零修改** | Value 枚举不变（内部使用） |
| `src/constructs/**` | **零修改** | 各 construct 的 parse/build/sizeof 不变（Py 路径通过 inner.parse/build 复用） |

**construct-rs 改动规模**：~3 个新文件（~800-1200 行），1 个 Cargo.toml 修改，1 个 mod.rs 模块声明。现有代码零修改。

### 7.2 construct-py（入口切换到 Py 路径）

| 文件 | 改动类型 | 说明 |
|------|---------|------|
| `Cargo.toml` | **修改** | construct 依赖加 `features = ["python"]` |
| `src/compiled_ext.rs` | **重写入口** | `parse_bytes_raw` / `build_from_raw` 改用 `exec_parse_py_dispatch` / `exec_build_py_dispatch` + PyDictSink2 + PyInput。删除旧 PyDictSink/PyInput 调用 |
| `src/py_sink.rs` | **删除或 deprecated** | 旧 PyDictSink 被 construct-rs 的 PyDictSink2 替代 |
| `src/py_input.rs` | **删除或 deprecated** | 旧 PyInput 被 construct-rs 的 PyInput trait 替代 |
| `src/conversions.rs` | **大幅缩减** | [C-CRIT-1 修正] `value_to_py` 缩减为 Dynamic 逃生舱专用（Container/List 包装现由 PySink §3.3.1 处理）；[C-CRIT-2 修正] `py_to_value` 缩减为 kw 参数转换 + ctx["_"] 浅转换（§4.6）+ Dynamic 回退。从 788 行降至 ~250 行 |
| `src/api.rs` | **零修改** | Python-facing API (parse_bytes_py/build_from_py 签名) 不变 |
| `src/exceptions.rs` | **零修改** | 异常映射不变 |
| `src/constructs_*.rs` | **零修改** | PyStruct/PyFormatField 等声明层不变 |

**construct-py 改动规模**：1 个文件重写入口（~100 行），2 个文件删除/缩减，1 个大幅缩减。公共 Python API 不变。

### 7.3 不受影响的组件

| 组件 | 理由 |
|------|------|
| `construct/` (Python 原版) | 只读参考，永不修改 |
| `plans/phase1-15/` | 已验收阶段，历史记录 |
| construct-rs 的 1515 个 `#[cfg(test)]` 测试 | 使用 Value 路径（exec_parse/exec_build），完全不变 |
| construct-py 的 61 个 dataclass 功能测试 | 测试 Python-facing API 行为，API 不变，行为等价 |

### 7.4 关键风险文件

| 文件 | 风险 | 缓解 |
|------|------|------|
| `py_exec.rs` (新) | 71 变体的 PyCompiledExec 实现量大，容易遗漏变体 | 宏生成 + dispatch 函数编译期检查（漏实现 → 编译失败） |
| `compiled_ext.rs` (重写) | 入口逻辑变更可能影响 kw 参数注入 / context._ 设置 | 保持 context 初始化逻辑（_parsing/_building/_sizing/_）不变；BC 测试覆盖 |
| `conversions.rs` (缩减) | Dynamic 逃生舱仍需 value_to_py 完整支持 | 保留 value_to_py 的 Container/List 分支（仅 Dynamic 调用）；测试覆盖 |

---

## §8 测试策略

### 8.1 测试矩阵

| 测试类型 | 路径 | 工具 | 数量 | Phase 16 要求 |
|---------|------|------|------|-------------|
| 纯 Rust 单元测试 | Value (exec_parse/exec_build) | `cargo test` (无 python) | 1515 | **全部 PASS，零修改** |
| Py 路径单元测试 | PyObject (exec_parse_py/exec_build_py) | `cargo test --features python` (construct-rs) | ~100 (新增) | 覆盖 leaf/wrapper/composite 的 Py 路径正确性 |
| construct-py 集成测试 | Python-facing API | `cargo test` (construct-py) | ~61 (现有) | **全部 PASS**（行为等价验证） |
| 性能测试 | 端到端 Python | `test_perf_comparison.py -n 5000` | 6 场景 | geomean ≥1.0x (S-PERF-1) |

### 8.2 Py 路径单元测试设计（construct-rs，`--features python`）

新增测试验证 Py 路径产出与 Value 路径等价：

```rust
#[cfg(feature = "python")]
#[cfg(test)]
mod py_tests {
    // 对每个场景：Value 路径 parse → Value；Py 路径 parse → PyObject；
    // 断言两者的语义等价（PyObject 转回 Value 后 == Value 路径结果）。

    #[test]
    fn py_parse_simple_struct_matches_value_path() {
        // Struct(magic/Int32ub, count/Int32ub)
        // Value: exec_parse → ValueSink → Value::Container
        // Py:    exec_parse_py → PyDictSink2 → PyObject (PyDict)
        // Assert: py_to_value(pydict) == value_path_result
    }

    #[test]
    fn py_parse_array_100_ints_matches_value_path() { ... }

    #[test]
    fn py_build_simple_struct_matches_value_path() { ... }

    #[test]
    fn py_build_array_batch_extract() { ... }

    // 边界条件测试
    #[test]
    fn py_parse_empty_struct() { ... }
    #[test]
    fn py_parse_nested_struct_ctx_reference() { ... }  // this.field
    #[test]
    fn py_build_none_input() { ... }
    #[test]
    fn py_build_missing_field_error() { ... }
}
```

### 8.3 行为等价验证策略

**核心原则**：Py 路径的输出（PyObject）必须与 Value 路径的输出（Value）语义等价。

验证方法：
1. 对同一输入数据，分别用 Value 路径和 Py 路径 parse
2. 将 Py 路径的 PyObject 通过 `py_to_value`（缩减后的版本，仅用于测试）转回 Value
3. 断言两个 Value 相等

这确保 Py 路径不引入语义偏差。

### 8.4 边界条件清单

| 编号 | 场景 | 验证要点 |
|------|------|---------|
| BC-1 | 空 Struct parse | PyDictSink2 root 返回空 dict `{}` |
| BC-2 | 嵌套 Struct 的 context 引用 (`this.a + 1`) | ctx Value 正确插入，Computed 字段正确求值 |
| BC-3 | build 方向 `context._` 设置 | ✅ [C-CRIT-2 修正 §4.6] 条件构造 (`IfThenElse(this._ == ...)`) 正确读取；浅转换支持 root 标量字段，嵌套引用为已知限制 |
| BC-4 | build 方向 None 输入 | extract_scalar_py 对 None 返回 Value::None |
| BC-5 | Container/ListContainer 属性访问兼容 | ✅ [C-CRIT-1 修正 §3.3.1] parse 结果用 `result.field` 访问；PySink 从创建起使用 Container/ListContainer |
| BC-6 | kw 上下文参数注入 | `parse_bytes_py(data, length=10)` 的 kw 正确注入 ctx |
| BC-7 | dataclass 输入 build | PyInput 的 as_value 回退正确处理 dataclass（经缩减后 py_to_value） |
| BC-8 | 错误路径部分结果丢弃 | parse 中途出错，部分 PyDict 被 drop，Python 收到异常 |
| BC-9 | CompiledExternal (PyCallback) 路径 | ext_parse 产出 ProducedOutput 正确适配 PySink |
| BC-10 | 大整数 / BigInt | value_to_py_scalar 的 BigInt 分支正确 |
| BC-11 | Adapter/Validator wrapper | exec_parse_py 正确应用 decode/check 后重新 deposit |
| BC-12 | Array batch extract 类型不匹配 | extract_batch_int_py 返回 None，退化到 per-index |
| BC-13 | GreedyRange（变长数组）parse | 循环 finish_item_py 正确追加 PyList |

---

## §9 子任务拆分建议（给 PM 参考）

### 16.1 Python-first 产出层设计（ARCH）— 本文档

- **状态**：进行中
- **产出**：本设计文档
- **预估**：已完成

### 16.2 pyo3 引入 + PySink/PyInput 基础设施（DEV）

**目标**：在 construct-rs 引入 pyo3 optional feature，实现 PySink trait + PyDictSink2 + PyInput trait + PyDictInput + value_to_py_scalar。

**改动文件**：
- `construct-rs/Cargo.toml`：加 pyo3 optional + python feature
- `construct-rs/src/compiled/py_sink.rs`（新）：PySink trait + PyDictSink2 + value_to_py_scalar
- `construct-rs/src/compiled/py_input.rs`（新）：PyInput trait + PyDictInput + PyScalarInput
- `construct-rs/src/compiled/mod.rs`：加模块声明

**验收**：
- `cargo build --features python` 编译通过
- `cargo test`（无 python）1515 测试全 PASS（回归）
- PySink/PyInput 单元测试（~20 个）PASS

**预估**：2-3h

### 16.3 Leaf + Composite 的 exec_parse_py 实现 + 入口切换（DEV）

**目标**：实现全部变体的 `exec_parse_py`，切换 `parse_bytes_raw` 到 Py 路径。

**改动文件**：
- `construct-rs/src/compiled/py_exec.rs`（新）：PyCompiledExec trait + 全变体 impl + dispatch
- `construct-py/src/compiled_ext.rs`：`parse_bytes_raw` 改用 `exec_parse_py_dispatch` + PyDictSink2

**验收**：
- `cargo test --features python`：Py 路径 parse 测试全 PASS
- **烟雾测试（Checkpoint 2）**：`test_perf_comparison.py -n 1000`，simple parse ≥1.0x（🔴 <1.0x 则暂停）
- construct-py 61 个 dataclass 测试 PASS（parse 方向）

**预估**：3-4h

### 16.4 exec_build_py 实现 + schema-guided extract（DEV）

**目标**：实现全部变体的 `exec_build_py`，切换 `build_from_raw` 到 Py 路径，实现裸 CPython API batch extract + ctx["_"] 浅转换。

**改动文件**：
- `construct-rs/src/compiled/py_exec.rs`：补全 exec_build_py impl
- `construct-rs/src/compiled/py_input.rs`：补全 extract_batch_int_py（**裸 CPython API，§4.5.1**）
- `construct-rs/src/compiled/py_sink.rs`（或独立模块）：补全 shallow_py_to_value_for_ctx（**§4.6 ctx["_"] 浅转换**）
- `construct-py/src/compiled_ext.rs`：`build_from_raw` 改用 `exec_build_py_dispatch` + PyInput + ctx["_"] 浅转换

**验收**：
- `cargo test --features python`：Py 路径 build 测试全 PASS
- `test_perf_comparison.py -n 1000`：simple/nested/array build ≥1.0x
- construct-py 61 个 dataclass 测试 PASS（build 方向）

**预估**：3-4h

### 16.5 conversions.rs 缩减 + 清理 + 全量验收（DEV + VET）

**目标**：缩减 conversions.rs，删除/deprecated 旧 PyDictSink/PyInput，全量性能验收。

**改动文件**：
- `construct-py/src/conversions.rs`：缩减至 ~250 行（保留 Dynamic 逃生舱 + Container 包装）
- `construct-py/src/py_sink.rs`：删除或 `#[deprecated]`
- `construct-py/src/py_input.rs`：删除或 `#[deprecated]`

**验收（Phase 16 出口标准）**：
1. `cargo build` + `cargo clippy`（零 warning）+ `cargo fmt --check` + `cargo test`（含 `--features python`）全通过
2. `cargo test`（construct-rs，无 python）1515 测试全 PASS
3. construct-py 61 dataclass 测试全 PASS
4. **性能对比数据表**：`test_perf_comparison.py -n 5000`，6 场景 geomean ≥1.0x (S-PERF-1)
5. **代码审查确认**：Value 不出现在 Python parse/build 输出路径上（仅在 ctx/瞬态标量中出现）

**预估**：2h

### 子任务依赖图

```
16.1 (ARCH 设计) ✅
  ↓
16.2 (pyo3 + PySink/PyInput 基础)
  ↓
16.3 (exec_parse_py + parse 入口) ← 烟雾测试检查点
  ↓
16.4 (exec_build_py + build 入口)
  ↓
16.5 (清理 + 全量验收) ← S-PERF-1 验收
```

> **16.3 是关键检查点**：如果 simple parse <1.0x，说明 PySink 设计有未预见问题，必须暂停并回 ARCH 分析。不要在 parse 不达标时继续实现 build。

---

## 附录 A：与旧方案 A（已废弃）的对比

| 维度 | 旧方案 A (FFI批量化优化) | 本设计 (Python-first) |
|------|------------------------|---------------------|
| 架构定位 | "纯 Rust 库 + construct-py 修复转换层" | "Python 项目 + Rust 内核直出 PyObject" |
| pyo3 位置 | 仅在 construct-py | **construct-rs (optional feature)** |
| Value 角色 | 产出载体（ValueSink → value_to_py 批量转换） | 内部瞬态标量 + ctx 载体（不作出树） |
| parse FFI 模式 | ValueSink 0 FFI → value_to_py 单次递归 | PySink per-field 直出 PyObject（零 round-trip） |
| build FFI 模式 | py_to_value 单次全量 → ValueInput 0 FFI | PyInput per-field 类型已知 extract（零全量转换） |
| B1 (round-trip) | ValueSink 无 round-trip（但需 value_to_py 遍历） | PySink 零 round-trip（双产物） |
| B2 (double convert) | 消除（单次 py_to_value） | 消除（per-field extract，无 as_value） |
| B3 (type chain) | 减半（py_to_value 仍有链，value_to_py 无链） | **消除**（类型已知 extract/match） |
| B4 (import_bound) | 缓存 OnceLock | root 一次获取（标量路径不经 import） |
| simple parse 预测 | 1.9x | **~7.1x**（修正后，含 Container 包装） |
| simple build 预测 | 0.33x（不达标！） | **~5.4x**（修正后，含 ctx["_"] 浅转换） |
| array build 预测 | 0.23x（不达标！） | **~5.4x**（裸 API batch，修正 P-CRIT-1） |

**旧方案 A 为何被废弃**：在"construct-rs 不碰 pyo3"伪约束下，build 方向必须经过 py_to_value 全量转换（类型检查链），无法达标。本设计通过将 PySink/PyInput/exec_py 引入 construct-rs，使类型已知 extract 成为可能，从根本上消除 B2/B3。

## 附录 B：Python 原版性能参考

Python 原版 construct 2.10.70 开销分解（来自旧设计附录 + cProfile 经验）：

| 操作 | 单次开销 | 说明 |
|------|---------|------|
| `Struct._parse` per-field | ~500ns | Python 字节码 + dict.__setitem__ |
| `FormatField._parse` | ~200ns | struct.unpack |
| `Array._parse` per-element | ~250ns | 循环 + ListContainer.append |
| `Struct._build` per-field | ~600ns | dict.__getitem__ + format |
| `Array._build` per-element | ~280ns | 循环 + list indexing |

Python 原版无 FFI 开销，但有 Python 解释器开销。Rust 内核 decode 比 Python struct.unpack 快 ~10x。本设计的目标是将 FFI 开销降至 < Python 解释器开销，使 Rust 的 decode 优势得以体现。
