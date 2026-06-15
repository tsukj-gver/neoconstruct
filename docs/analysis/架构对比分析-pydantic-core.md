# 架构对比分析：pydantic-core vs construct-rs/construct-py

> **作者**：ARCH
> **日期**：2026-06-15
> **目的**：通过对比 pydantic-core（V2）的 Rust+PyO3 架构与 construct-rs/construct-py 的架构，
> 定位我们设计中导致 Python 调用场景性能仅为原版 ~0.2x（慢约 5 倍）的架构级缺陷，
> 并给出改进方向。

---

## 1. 执行摘要

基准测试表明，construct_rust 在 Python 调用场景下比 Python 原版 construct 慢约 5 倍
（几何平均 0.199x）。经过对 pydantic-core 与本项目的源码级深度对比，核心结论是：

**性能差距不是来源于单点优化缺失，而是来源于一个根本性的架构范式差异。**

pydantic-core 采用的是 **"build-once, run-pure-Rust"** 范式：在 Python 侧用 schema dict
声明结构，**一次性**编译成一棵完全在 Rust 内部的验证器树（`Arc<CombinedValidator>`）；
此后每次 `validate_python()` 只发生**一次** FFI 穿越，整棵树的遍历、值的读写全部在 Rust
侧完成，直接产出 `Py<PyAny>` 作为返回值。

construct-rs/construct-py 采用的是 **"per-operation bridging"** 范式：construct 树的
组合发生在 Python 侧（PyO3 wrapper 对象 + 运算符重载），每次 `parse()` / `build()` 虽然
只有一次顶层 FFI，但在树的内部，**每一个涉及 Python 回调的节点都会触发额外的 FFI 往返
和完整的 `Value ↔ PyObject` 递归转换**。强制使用 `Value` enum 作为中间表示、
`Box<dyn Construct>` 动态分发、`Context → Python Container` 的逐次全量转换，三者叠加
构成了性能损耗的主体。

**最严重的架构缺陷**是 §3.2 描述的 **Value enum 强制中转**：它不是某个函数的优化问题，
而是贯穿整个数据流的系统性开销——每一次 parse 产出 Value 再转 Python、每一次 build
从 Python 转 Value、每一个表达式求值都要双向转换。pydantic-core 通过让验证器直接操作
`Py<PyAny>` 从根本上消除了这层开销。

---

## 2. 架构模式对比

| 维度 | pydantic-core | construct-rs / construct-py | 影响 |
|------|--------------|---------------------------|------|
| **构建模型** | **Build-once**：`SchemaValidator::new()` 一次性将 Python schema dict 编译为 `Arc<CombinedValidator>` 树，运行时零构建开销 (`mod.rs:136-177`) | **Per-operation**：construct 树在 Python 侧通过运算符组合，每次 parse/build 都遍历这棵混合树 | pydantic 的验证器树是纯 Rust 结构，无 Python 对象参与运行时；我们的树节点本身可能是 PyO3 wrapper，运行时有额外的 Python 对象交互 |
| **树的位置** | **完全在 Rust 内部**：`SchemaValidator` 持有 `Arc<CombinedValidator>`，子验证器全是 Rust struct (`mod.rs:112-124`) | **跨 FFI 的混合树**：Struct/Sequence 的字段是 `Box<dyn Construct>`（Rust），但 Python 用户创建的 Adapter/Validator 是 Python 对象，通过 `PyConstructAdapter` 桥接 (`py_adapter.rs:49-52`) | 我们的树在 Rust↔Python 边界处频繁往返；pydantic 的树从不穿越边界 |
| **值表示** | **直接 `Py<PyAny>`**：`TypedDictValidator.validate()` 创建 `PyDict::new(py)` 直接填充，不经过 Rust enum (`typed_dict.rs:155, 225`) | **`Value` enum 强制中转**：parse 返回 `Value`，再 `value_to_py` 转换；build 用 `py_to_value` 转换输入 (`api.rs:96-97, 172, 186-189`) | 每次 parse/build 都有 O(n) 的递归转换开销（n = 数据深度），含堆分配 |
| **分发机制** | **`enum_dispatch` 零开销**：`CombinedValidator` 是一个 enum，`Validator` trait 通过宏生成 match 分发，可被编译器内联优化 (`mod.rs:725-824, 828-836`) | **`Box<dyn Construct>` 虚函数表分发**：每次 `parse`/`build`/`sizeof` 调用都是间接跳转，无法内联 (`core/mod.rs:50-66`) | 虚调用无法被编译器内联和优化；缓存局部性差（vtable 指针跳转） |
| **Python 回调处理** | **只持有 `Py<PyAny>` 引用**，调用时直接传 Python 对象：`self.func.call1(py, (input.to_object(py)?,))` (`function.rs:111-113`)。context 通过 `ValidationInfo` **按引用**传递 | **每次回调做完整 Context→Container 转换**：`PyClosure::evaluate` 调用 `context_to_py_container`，递归遍历整条 parent 链构建 Python dict (`expr_bridge.rs:62-83`) | 每个表达式求值都有 O(深度) 的 Context 转换开销，且包含 `_root`/`_params` 的递归注入 |
| **输入抽象** | **`Input` trait**：抽象 Python/JSON/String 三种输入源，validator 通过 `validate_int()`/`validate_dict()` 等方法按需读取，**不预转换** (`input_abstract.rs:59-150`) | **无输入抽象**：build 时 `py_to_value` 一次性递归转换整个输入树为 `Value` (`api.rs:172`) | pydantic 按需读取（惰性），我们全量预转换（急切）；大输入时差异显著 |
| **FFI 穿越次数** | **每次操作 1 次**：`validate_python()` 一次进入 Rust，整棵树遍历完毕后返回 (`mod.rs:181-213`) | **顶层 1 次 + 每个回调点 1 次**：顶层 parse/build 1 次，树内每个 PyClosure/PyConstructAdapter 节点各 1 次往返 | 树越深、Python 回调越多，FFI 开销线性增长 |
| **输出构建方式** | **直接构建 Python 对象**：验证过程中逐步构建 `PyDict`/`PyList`，验证完成即得到最终 Python 对象 (`typed_dict.rs:155`) | **先构建 Value，再整体转换**：parse 先构建完整的 `Value::Container`，再递归 `value_to_py` 转换 (`api.rs:96-97`) | 我们有双倍内存峰值（Value 树 + Python 对象树同时存在）+ 转换遍历开销 |

---

## 3. 架构缺陷分析

### 3.1 Per-operation FFI 穿越与混合树模型

**现状描述**

construct-py 的 construct 树是一棵**跨 FFI 边界的混合树**：
- Rust 侧：`Struct { fields: Vec<Renamed> }`，每个 `Renamed` 持有 `Box<dyn Construct>`
- Python 侧：用户用 `Int32ub + "name" / Byte` 等运算符组合出的 PyO3 wrapper 对象

当 Rust Struct 的某个字段是一个 Python 用户定义的 Adapter 时（通过 `PyConstructAdapter`
桥接），每次 `parse()` 的执行路径为：

```
Rust Struct.parse
  → PyConstructAdapter.parse (获取 GIL)
    → read_remaining → 创建 BytesIO (Python 对象)
    → Python Adapter.parse_stream (FFI 出)
      → 可能又调用 Rust construct (FFI 入)
    → tell() 同步 (FFI)
    → py_to_value 转换结果
  → 回到 Rust Struct.parse 继续下一字段
```

参见 `py_adapter.rs:195-259`，每次 PyConstructAdapter.parse 至少产生 **5 次 FFI 穿越**
（GIL 获取、BytesIO 创建、parse_stream 调用、tell 调用、py_to_value 内部的类型检查）。

**pydantic-core 如何解决**

pydantic-core 的验证器树在 `SchemaValidator::new()` 时就已经**完全编译为纯 Rust 结构**
（`mod.rs:147`：`build_validator(schema, config, &mut definitions_builder)`）。运行时调用
`validate_python()` 时，整棵 `CombinedValidator` 树的遍历**全部在 Rust 侧完成**，包括
嵌套的 TypedDict、List、Union 等。唯一可能回到 Python 的是用户注册的自定义函数
（`FunctionBeforeValidator` 等），且这种回调是**显式可选的**，而非架构固有开销。

**影响评估**
- **性能**：对于包含 Python Adapter 的 construct 树（这在实际使用中很常见，如 Enum、
  Validator、自定义 Adapter 子类），FFI 往返开销是主导因素
- **可维护性**：PyConstructAdapter 中的 BytesIO 同步逻辑（`py_adapter.rs:246-255`，
  `321-345`）极其复杂且脆弱，需要处理 StopField、seek 回退等边界情况
- **扩展性**：每新增一种 Python 互操作方式，都需要在 FFI 边界做适配

---

### 3.2 Value enum 作为强制中间表示的性能开销（最严重缺陷）

**现状描述**

`Value` enum（`value.rs:40-67`）是我们设计的核心动态类型系统，包含 10 个 variant。
它作为**所有 parse 产出和 build 输入的强制中间表示**：

```
Python 调用 build(obj):
  py_to_value(obj) → Value    ← 递归转换，O(深度) 堆分配
  construct.build(&Value)      ← Rust 内核处理
  (对于 parse:)
  construct.parse() → Value   ← Rust 内核产出
  value_to_py(&Value) → PyObj ← 递归转换，O(深度) 堆分配
```

具体开销来源（参见 `conversions.rs`）：
1. **`py_to_value`**（`conversions.rs:110-183`）：对每个 Python 对象做 9 次
   `is_instance_of` 检查（None→Bool→BigInt→Float→Bytes→ByteArray→String→List→Dict），
   然后递归转换容器内的每个元素
2. **`value_to_py`**（`conversions.rs:43-97`）：对 List 和 Container variant，
   **每次都尝试 `import_bound("construct_rust.lib.containers")`** 来创建
   ListContainer/Container 实例（`conversions.rs:62-64, 81-84`），这是一个 Python
   模块导入操作
3. **双倍内存峰值**：parse 完成时，Value 树和 Python 对象树同时存在于内存中，
   直到 Value 被 drop

**pydantic-core 如何解决**

pydantic-core 的验证器**直接操作 Python 对象**，没有中间 Rust enum：
- `TypedDictValidator.validate()` 直接 `PyDict::new(py)` 创建输出字典
  （`typed_dict.rs:155`），每个字段验证后直接 `output_dict.set_item(&field.name, value)`
  （`typed_dict.rs:225`），`value` 本身就是 `Py<PyAny>`
- 输入侧通过 `Input` trait 的 `validate_dict()` / `validate_int()` 等方法**按需读取**，
  不预转换整个输入树
- 整个验证过程中**零中间 Rust 数据结构**，零额外堆分配

**影响评估**
- **性能（致命）**：对于任何包含 Container 的解析结果（Struct 是最常用的 construct），
  都有完整的 Value::Container → Python Container 转换开销，包括递归遍历 + 每层
  `import_bound` 尝试。这是 5x 性能差距的**最大单一贡献因素**
- **可维护性**：Value 与 Python 类型的双向转换逻辑分散在 `conversions.rs`（788 行）、
  `expr_bridge.rs`（909 行）、`py_adapter.rs` 中，维护成本高
- **扩展性**：新增一种 Python 类型支持需要同时修改 `py_to_value` 和 `value_to_py`

> **结论**：这是本报告识别出的**最严重架构缺陷**。它不是局部优化问题，而是贯穿
> 整个数据流的设计决策。解决它需要根本性地重新思考"Rust 内核是否需要自己的
> 动态类型系统"这个问题。

---

### 3.3 `Box<dyn Construct>` 动态分发的缓存不友好性

**现状描述**

`Construct` trait 使用 trait object 分发（`core/mod.rs:39-101`）：
- `Struct.fields: Vec<Renamed>`，`Renamed.inner: Box<dyn Construct>`
- `Subconstruct.subcon: Box<dyn Construct>`
- 所有复合构造器都持有 `Box<dyn Construct>` 类型的子构造器

每次 `parse()` / `build()` / `sizeof()` 调用都是通过虚函数表（vtable）的间接调用。

**pydantic-core 如何解决**

使用 `enum_dispatch` 宏（`mod.rs:5, 724, 828`）：
- `CombinedValidator` 是一个 enum，包含所有验证器 variant
- `Validator` trait 通过 `#[enum_dispatch(CombinedValidator)]` 生成 match 分发代码
- 编译器可以对 match 分支做内联优化，分支预测器也能很好地预测（类型通常稳定）

关键区别：`enum_dispatch` 生成的分发是**直接 match**（编译器可见的控制流），
而 `Box<dyn>` 是**运行时间接跳转**（编译器不可见，无法内联）。

**影响评估**
- **性能**：对于深层嵌套的 construct 树（如 10 层 Struct），每次 parse 要做 10+ 次
  虚函数调用。每个虚调用有 vtable 查找开销 + 无法内联 + 可能的分支预测失败
- **缓存局部性**：`Box<dyn Construct>` 的实际数据分散在堆上，不同节点的数据可能
  不在同一缓存行，导致 cache miss
- **权衡**：`Box<dyn Construct>` 提供了异构集合的能力（Vec 中存放不同类型的
  construct），这是 construct 的核心需求。但 `enum_dispatch` 同样能提供这个能力，
  且无运行时开销

---

### 3.4 Context → Python Container 的逐次全量转换

**现状描述**

当 construct 树中的表达式（如 `Bytes(this.length)`）需要求值时，`PyClosure::evaluate`
（`expr_bridge.rs:60-83`）的执行路径：

```rust
fn evaluate(&self, ctx: &Context, _obj: Option<&Value>) -> Result<Value> {
    Python::with_gil(|py| {
        // 1. Context → Python dict（重！）
        let py_ctx = context_to_py_container(py, ctx)?;
        // 2. 调用 Python callable
        let result = self.call_single(py, py_ctx.bind(py))?;
        // 3. 返回值 → Value
        py_to_value(py, result.bind(py))?
    })
}
```

`context_to_py_container`（`conversions.rs:234-289`）的开销：
1. **递归遍历整个 Context 链**：当前层 → parent → parent's parent → ...
2. **每层都创建新的 Python Container 对象**（含 `import_bound` 尝试）
3. **注入 `_root` 和 `_params`**：额外遍历 parent 链找到 topmost 和 root 上下文，
   再递归转换它们（`conversions.rs:273-286`）
4. **每个字段的值都递归 `value_to_py`**

对于一个 3 层嵌套的 Struct（parent 链深度 3），每次表达式求值要创建 3+ 个 Python
Container 对象，遍历所有字段。如果一个 Array 有 100 个元素且每个元素的 Struct 中有
一个 `this.length` 表达式，那么就是 100 次完整的 Context 转换。

**pydantic-core 如何解决**

`FunctionBeforeValidator` 等（`function.rs:84-150`）持有 `Py<PyAny>` 函数引用和
`Py<PyAny>` config。调用时：
```rust
self.func.call1(py, (input.to_object(py)?, info))
```
其中 `info` 是 `ValidationInfo`，它**只持有一个 `&Bound<PyAny>` context 引用**
（`function.rs:110`），不做任何转换。`input.to_object(py)` 对于 Python 输入来说
几乎零开销（输入本身就是 Python 对象）。

关键差异：pydantic 的 context 是**用户传入的 Python 对象的引用**，不是 Rust 结构体
转换来的。我们的 Context 是 Rust 的 `IndexMap<String, Value>` 结构，每次传给 Python
都必须转换。

**影响评估**
- **性能（严重）**：表达式密集的 construct（如 `Struct("a" / Byte, "b" / Bytes(this.a))`）
  中，每个字段的表达式求值都有完整的 Context 转换开销
- **正确性风险**：`_root` / `_params` 的注入逻辑（`conversions.rs:269-286`）试图
  模拟 Python construct 的 context 语义，但这是一个脆弱的近似实现

---

### 3.5 PyConstructAdapter 的 BytesIO 全量拷贝

**现状描述**

`PyConstructAdapter`（`py_adapter.rs:49-52`）是连接 Python 用户自定义 construct 与
Rust 内核的桥梁。它的 parse 实现（`py_adapter.rs:195-259`）采用 "Read-all + BytesIO"
策略：

1. 记录当前流位置（`start_pos`）
2. **读取所有剩余字节**（`stream.read_remaining()`）—— 这是 O(n) 的拷贝
3. 创建 Python `io.BytesIO` 对象，将所有字节传入 —— 又一次 O(n) 拷贝
4. 调用 Python 的 `parse_stream`
5. 调用 `tell()` 同步位置
6. `seek(start_pos + consumed)` 恢复 Rust 流位置
7. `py_to_value` 转换结果

build 方向（`py_adapter.rs:262-358`）更复杂：
1. `value_to_py` 转换输入
2. 创建空 BytesIO
3. 调用 Python `build_stream`
4. `getvalue()` 读取全部字节 —— O(n) 拷贝
5. `write_bytes` 写回 Rust 流 —— O(n) 拷贝
6. 处理 seek 回退、StopField 刷新等边界情况

**pydantic-core 如何解决**

pydantic-core 不存在这个问题，因为它的验证器树完全在 Rust 内，不存在"Rust 树中
嵌入 Python 验证器"的场景。用户自定义函数通过 `FunctionBeforeValidator` /
`FunctionAfterValidator` 嵌入，这些回调操作的是 `Py<PyAny>` 值（已经是 Python 对象），
不需要流拷贝。

**影响评估**
- **性能（严重）**：对于大文件解析（如 ELF/PE 格式），如果树的某层有 Python
  Adapter，整个剩余字节都会被拷贝两次（read_remaining → BytesIO 构造）
- **内存**：峰值内存 = 原始数据 × 2（Rust 缓冲区 + Python BytesIO）
- **正确性**：read_remaining 假设可以读取所有剩余数据，对于真正的流式数据源
  （网络、管道）不可用

---

### 3.6 Construct trait 设计：构建与运行时未分离

**现状描述**

我们的 `Construct` trait（`core/mod.rs:39-101`）将三个职责合并在一个 trait 中：
- `parse()` —— 运行时解析
- `build()` —— 运行时构建
- `sizeof()` —— 编译期/运行时大小计算
- `flagbuildnone()` —— 元数据属性
- `build_effective()` —— 运行时构建变体

每个 construct 实例**既是 schema 定义又是运行时执行器**。没有分离"schema 构建阶段"
和"运行时执行阶段"。

**pydantic-core 如何解决**

pydantic-core 明确分离了两个阶段（`mod.rs:508-518, 829-865`）：
- **`BuildValidator` trait**（构建阶段）：`fn build(schema, config, definitions) -> Arc<CombinedValidator>`
  —— 从 schema dict 构建验证器，只在 `SchemaValidator::new()` 时调用一次
- **`Validator` trait**（运行时）：`fn validate(py, input, state) -> ValResult<Py<PyAny>>`
  —— 每次验证调用

这种分离带来的好处：
1. 构建阶段可以做优化（如字段查找路径预计算、严格模式判断等），结果缓存在验证器 struct 中
2. 运行时阶段只关注性能关键路径，不重复解析配置
3. 同一个 schema 可以构建出不同的验证器（如 strict/lax 模式）

**影响评估**
- **性能**：我们的 construct 在每次 parse/build 时都可能重复计算本可在"构建阶段"
  缓存的信息（如 Renamed 的名字路径、表达式的编译结果等）
- **可扩展性**：无法为同一棵 construct 树生成不同优化级别的执行器
- **权衡**：construct 的声明式 API（`"name" / Byte + "count" / Byte`）是它的核心
  用户体验，pydantic 的 schema-dict 方式更底层。但这种分离可以在内部实现而不影响 API

---

### 3.7 Stream 抽象：Python 调用场景下的过度设计

**现状描述**

我们设计了自定义的 `Stream` trait（`core/stream.rs`，基于 Cursor），支持
`read_bytes` / `write_bytes` / `seek` / `tell` / `read_remaining` 等操作。
在 Rust 纯内核使用时，这个抽象是合理的。

但在 Python 调用场景下，Stream 抽象引入了额外的间接层：
- `py_parse` 创建 `ByteStream`（Rust 的 Cursor 包装），parse 完毕后丢弃
- `py_parse_stream` 创建 `PyStream`（包装 Python 流对象），每次 `read_bytes` 都
  穿越到 Python 调用 `.read()`
- `PyConstructAdapter` 完全绕过 Stream 抽象，直接用 BytesIO

**pydantic-core 如何解决**

pydantic-core 没有流的概念（验证的是已有的 Python 对象/JSON 文本），所以这个对比
不完全适用。但 pydantic 的 `Input` trait 提供了一个启示：它抽象的是**输入数据的
读取方式**（按需读取 int/str/dict），而不是一个通用的流接口。

**影响评估**
- **性能（轻微）**：Stream trait 的虚函数分发（`&mut dyn Stream`）有少量开销
- **复杂度**：Stream、ByteStream、PyStream 三层抽象增加了理解和维护成本
- **判断**：Stream 抽象本身不是性能瓶颈（主要开销在 Value 转换），但在 Python
  调用场景下，它确实是一个"为了 Rust 内核纯度而存在"的间接层。对于纯 Python
  使用的场景，直接操作 bytes slice 可能更高效

---

### 3.8 表达式系统的 FFI 开销叠加

**现状描述**

construct 的表达式系统（`this.field`、`this.field + 1` 等）在 Python 侧是
`Path`/`BinExpr` 等 Python 对象。当它们嵌入 Rust construct 树时，通过
`PyClosure`（`expr_bridge.rs:40-83`）桥接。

每次表达式求值的完整开销（以 `Bytes(this.length)` 为例，在一个 100 元素的 Array 中）：

```
对每个元素:
  PyClosure.evaluate(ctx)
    → Python::with_gil (GIL 获取，如果已持有则轻量)
    → context_to_py_container(ctx)        ← O(深度) Container 创建
      → import_bound("...containers")     ← 模块查找
      → 遍历所有字段 value_to_py          ← 递归转换
      → 递归处理 parent 链               ← 又一层完整转换
      → 注入 _root, _params              ← 额外两次递归转换
    → callable.call1(py, (py_ctx,))       ← FFI 调用 + Python 执行
    → py_to_value(result)                 ← 递归转换返回值
```

100 个元素 = 100 次上述完整流程。

此外，`expr_bridge.rs` 中还有 6 个额外的函数类型适配器
（`py_to_cond_func`、`py_to_key_func`、`py_to_check_func`、`py_to_decode_func`、
`py_to_encode_func`、`py_to_repeat_predicate`），每个都重复着相同的
Context→Container→调用→Value 模式。

**pydantic-core 如何解决**

pydantic 的 `FunctionBeforeValidator`（`function.rs:96-118`）调用路径：
```rust
let r = if self.info_arg {
    let info = ValidationInfo::new(py, state, &self.config, field_name);
    self.func.call1(py, (input.to_object(py)?, info))
} else {
    self.func.call1(py, (input.to_object(py)?,))
};
```
- `input.to_object(py)`：对于 Python 输入，几乎零开销（返回引用）
- `ValidationInfo::new`：构造一个轻量的 Python 对象，持有 context 的**引用**，
  不做转换
- 没有 Rust→Python 的数据结构转换

**影响评估**
- **性能（严重）**：表达式密集的 construct 是最坏情况，每个字段的表达式求值
  开销可能超过实际解析开销
- **正确性**：`context_to_py_container` 中 `_root`/`_params` 的注入是为了模拟
  Python construct 的 context 语义，但如果 Rust 内核的 Context 管理与 Python
  原版有任何细微差异，会导致表达式求值结果不一致

---

## 4. 领域差异分析

在将 pydantic-core 的做法迁移到 construct 时，必须诚实面对两个库的领域差异。
以下几方面 pydantic 的做法**不能直接搬用**：

### 4.1 二进制解析 vs 数据验证

| 特性 | construct | pydantic |
|------|-----------|----------|
| 输入 | 字节流（面向字节序列） | Python 对象 / JSON 文本（面向已结构化数据） |
| 核心操作 | 从字节流读取 → 产出结构化数据 | 验证已有结构化数据 → 产出（可能转换后的）结构化数据 |
| 对称性 | 需要 parse（字节→值）**和** build（值→字节）两个方向 | 主要是单向验证（输入→验证结果），序列化是独立的模块 |

**影响**：construct **必须**有一个表示"从字节解析出的值"的中间结构。pydantic 可以
直接操作输入的 Python 对象，因为输入本身就是 Python 对象。construct 的 parse 产出
是全新的数据，必须先在某个表示中构建。

**但这不意味着必须用 Value enum**：pydantic-core 的 TypedDictValidator 在验证过程中
直接构建 `PyDict`——construct 的 Struct 也可以在 parse 过程中直接构建 Python Container，
跳过 Value::Container 中间步骤。差异在于：construct 的 parse 需要从字节流读取（这个
步骤确实需要某种 Rust 内部的临时表示），但**读取后可以直接写入 Python 对象**，而不
是先写入 Value::Container 再转换。

### 4.2 双向对称性

construct 的每个构造器必须同时支持 parse 和 build。这意味着：
- parse 方向：字节 → Rust 处理 → 需要产出某种值表示
- build 方向：需要接受某种值表示 → Rust 处理 → 字节

pydantic 只需要单向（validate），所以可以始终直接操作 Python 对象。

**影响**：build 方向确实需要从 Python 对象读取输入。但这里可以借鉴 pydantic 的
`Input` trait 思路——按需从 Python 对象读取字段值（`obj.get_item("field")`），
而不是预先用 `py_to_value` 递归转换整个输入树。

### 4.3 表达式系统的复杂性

construct 的表达式系统（`this.x.y`、`this._.count + 1`、`len(obj_) > 0`）比
pydantic 的字段引用复杂得多：
- pydantic 的 validator 只需要访问当前字段值和 context（一个用户提供的 dict）
- construct 的表达式可以沿 context parent 链向上查找（`this._._.field`），
  可以引用被构建的对象本身（`obj_`），支持任意 Python 表达式

**影响**：Context → Python 的转换在 construct 中确实更复杂（需要处理 parent 链、
`_root`、`_params`）。pydantic 只传一个 context 引用即可。这是 construct 领域固有的
复杂性，但可以通过更好的架构（如惰性 Context 视图）来缓解，而非每次全量转换。

### 4.4 流式操作

construct 需要支持 seek/tell（`Tell`、`Seek`、`Pointer`、`Anchor` 等构造器），
这是 pydantic 完全不需要的能力。

**影响**：Stream 抽象在 construct 中是**必要的**（不能像 pydantic 那样只处理
已有对象）。但问题不在于 Stream 是否需要，而在于 Stream 是否应该是 trait object
（`&mut dyn Stream`）。对于纯字节流操作，直接传 `&mut [u8]` + position 可能更高效。

### 4.5 小结：哪些可以借鉴，哪些不能

| pydantic 做法 | 能否用于 construct | 说明 |
|--------------|-------------------|------|
| build-once 验证器树 | **可以** | construct 树也可以在构建时编译为纯 Rust 执行树 |
| `enum_dispatch` 分发 | **可以** | 替代 `Box<dyn Construct>`，零开销 |
| 直接操作 `Py<PyAny>` | **部分可以** | parse 方向需要中间表示，但可以更轻量；build 方向可以用 Input trait 按需读取 |
| 不转换 context | **需要改造** | construct 的 context 更复杂，但可以做惰性视图而非全量转换 |
| `Input` trait 抽象 | **可以** | build 方向按需读取 Python 对象字段，不预转换 |
| 无 Stream 抽象 | **不能搬用** | construct 领域必须有流操作能力 |

---

## 5. 改进方向建议

### 5.1 短期优化（不改架构，减少 FFI 开销）

这些优化可以在当前架构上实施，预计可带来 1.5-2x 提升：

1. **缓存 `import_bound` 结果**：`value_to_py` 中每次创建 Container/ListContainer
   都调用 `import_bound("construct_rust.lib.containers")`（`conversions.rs:62-64,
   81-84`）。应在模块初始化时缓存 `Py<PyType>` 引用，运行时直接使用

2. **批量 Context 转换优化**：`context_to_py_container` 中 `_root`/`_params` 的
   注入（`conversions.rs:269-286`）每次都重新遍历 parent 链。可以在 Context 中
   缓存上一次转换的 Python 对象（带版本号失效），避免重复转换

3. **PyConstructAdapter 零拷贝 parse**：对于 `ByteStream`（内存中的 Cursor），
   `read_remaining` 可以返回 `&[u8]` 引用而非拷贝。BytesIO 可以用 `memoryview`
   包装避免拷贝

4. **减少表达式求值中的冗余转换**：`PyClosure::evaluate` 每次都做
   `py_to_value` 转换返回值。如果表达式结果类型可预测（如总是返回 int），
   可以用专用的快速路径

5. **GIL 复用**：确保整个 parse/build 操作在单次 GIL 获取内完成，
   避免表达式求值时反复获取/释放 GIL

### 5.2 中期重构（调整 FFI 边界粒度）

这些改动需要调整架构，但不需要完全推倒重来，预计可带来 3-5x 提升：

1. **引入"Python-native parse"模式**：当检测到 construct 树完全由 Rust 内核
   构造器组成（无 Python 回调）时，跳过所有 Value↔Python 转换，parse 直接
   构建 Python Container，build 直接从 Python 对象按需读取。这类似于 pydantic
   的 TypedDictValidator 直接操作 PyDict

2. **Context 惰性视图**：用 Python 的 `Mapping` 协议实现一个 Rust 支持的
   Context 视图对象，传给 Python 表达式时**不转换数据**，而是在 Python 访问
   `ctx["field"]` 时才按需转换单个值。这消除全量 Context 转换开销

3. **分离 build 阶段和运行时阶段**：引入 `BuildConstruct` trait（schema →
   编译后的执行器），在 construct 树构建时预计算字段路径、表达式编译结果等，
   缓存在执行器 struct 中。运行时只执行性能关键路径

4. **用 `enum_dispatch` 替代 `Box<dyn Construct>`**：定义
   `CombinedConstruct` enum，用宏生成 match 分发。这是纯机械替换，
   风险低，收益明确

5. **PyClosure 编译优化**：在构建阶段检测表达式类型（Path、BinExpr、lambda），
   为常见模式（如 `this.field` 的单层路径访问）生成优化的 Rust 原生求值路径，
   避免 FFI 调用

### 5.3 长期愿景（理想架构）

如果推倒重来，理想的 construct-rs 架构应该是：

```
┌─────────────────────────────────────────────────────┐
│  Python 层：声明式 API（不变）                       │
│  Struct("a"/Byte, "b"/Bytes(this.a))                │
└──────────────────────┬──────────────────────────────┘
                       │ 构建时（一次）
                       ▼
┌─────────────────────────────────────────────────────┐
│  编译层：schema → 执行树                             │
│  遍历 Python construct 对象，编译为纯 Rust 执行树    │
│  - 表达式编译为 Rust 闭包或字节码                    │
│  - 字段路径预计算                                    │
│  - Python 回调封装为 PyCallback 节点                 │
│  产出：CompiledParser { tree: CompiledNode }        │
└──────────────────────┬──────────────────────────────┘
                       │ 运行时（每次调用）
                       ▼
┌─────────────────────────────────────────────────────┐
│  执行层：纯 Rust 遍历                                │
│  CompiledParser.parse_bytes(data)                   │
│  → 单次 FFI 进入                                    │
│  → 整棵树遍历在 Rust 内完成                          │
│  → 直接构建 PyDict/PyList（不经 Value enum）         │
│  → Python 回调仅在遇到 PyCallback 节点时触发         │
│  → 返回 Py<PyAny>                                   │
└─────────────────────────────────────────────────────┘
```

核心原则：
- **build-once**：schema 编译为执行树，运行时零构建开销
- **direct-to-Python**：parse 直接构建 Python 对象，不经 Value enum
- **lazy context**：Context 作为惰性视图传给 Python，不预转换
- **enum_dispatch**：执行树用 enum 分发，零虚函数开销
- **selective FFI**：仅在 Python 回调节点穿越 FFI，其余全在 Rust 内

这与 pydantic-core 的架构高度一致，同时保留了 construct 领域的特殊需求
（双向对称、流操作、表达式系统）。

---

## 6. 结论

construct_rust 在 Python 调用场景下比 Python 原版慢 5 倍的根本原因，是架构范式
选择导致的系统性开销，而非局部优化不足。通过与 pydantic-core 的深度对比，
我们识别出 8 个架构级缺陷，其中最严重的是：

1. **Value enum 强制中转（§3.2）**——贯穿整个数据流的 O(n) 递归转换开销
2. **Context 全量转换（§3.4）**——每次表达式求值都重建整个 Context 的 Python 镜像
3. **混合树 FFI 往返（§3.1/§3.5）**——Python 回调节点导致多次 FFI 穿越和数据拷贝

pydantic-core 的成功经验告诉我们：**Rust+PyO3 的高性能来自让 Rust 尽可能多地"拥有"
运行时执行路径**——验证器树完全在 Rust 内、直接操作 Python 对象、最小化 FFI 穿越。
我们的架构恰恰相反：让 Python 和 Rust 在树的每个节点交替掌管控制权。

改进的可行路径是分阶段的：短期通过缓存和零拷贝优化缓解症状（1.5-2x），中期通过
FFI 边界调整和惰性 Context 显著改善（3-5x），长期通过 build-once 编译模型和
direct-to-Python 执行达到与 pydantic-core 相当的性能水平。

值得强调的是，这些改进**不需要牺牲 construct 的声明式 API 用户体验**——用户仍然
写 `Struct("a"/Byte, "b"/Bytes(this.a))`，变化发生在内部执行模型。这正是
pydantic V2 的成功之道：API 保持 Pythonic，引擎完全 Rust 化。
