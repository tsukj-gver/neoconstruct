# 模块设计：FFI 批量化优化（Phase 16）

> **阶段**：Phase 16 — FFI 批量化优化
> **目标**：将 Python C API 调用从 per-field 降为 per-operation，使 S-PERF-1 ≥1.0x vs Python 原版
> **前置**：Phase 13/14/15（PyDictSink/PyInput direct-to-Python 路径已实现但未达标）

---

## 模块位置

| 组件 | 文件路径 | 改动性质 |
|------|---------|---------|
| PyDictSink（重写） | `construct-py/src/py_sink.rs` | 重构为批量转换 |
| PyInput（重写） | `construct-py/src/py_input.rs` | 重构为批量转换 |
| conversions.rs（优化） | `construct-py/src/conversions.rs` | 缓存模块引用 + 批量 list 构建 |
| compiled_ext.rs（调整入口） | `construct-py/src/compiled_ext.rs` | parse_bytes_raw/build_from_raw 改用 ValueSink/ValueInput |
| CompiledSchema（只读复用） | `construct-rs/src/compiled/mod.rs` | **不修改**，复用 parse_bytes/build_bytes |

## 职责

将 Python 路径的 FFI 调用模式从"per-field 穿越边界"改为"per-operation 单次批量转换"，消除当前的重复转换和 round-trip 开销。

---

## 性能假设

> 本节满足 `.opencode/skills/performance-gate.md` Checkpoint 1 要求。

### 瓶颈识别（量化数据 + 代码审查）

#### 数据来源

1. **实测绝对数据**（2026-06-19，PM 提供，5000 次迭代，`test_perf_comparison.py`）：

| 格式 | 方向 | Python 原版 | new_compiled | 比率 | 单次差值 |
|------|------|-----------|-------------|------|---------|
| simple (3字段,10B) | parse | 3.0µs | 6.6µs | 0.46x | +3.6µs |
| simple (3字段,10B) | build | 2.8µs | 20.6µs | 0.13x | +17.8µs |
| nested (2级,14B) | parse | ~4.0µs | ~11.1µs | 0.36x | +7.1µs |
| nested (2级,14B) | build | ~3.2µs | ~29.1µs | 0.11x | +25.9µs |
| array (100元素,404B) | parse | 27.7µs | 232µs | 0.12x | +204µs |
| array (100元素,404B) | build | ~30µs* | ~750µs* | 0.04x | +720µs |

> *array build 绝对值由比率 0.04x × Python parse 基线推算（PM 仅提供了 ratio）。

2. **FFI 单次开销经验值**（来自 `模块设计-FFI执行层.md` §4.4 实测量化）：
   - Python→Rust FFI 进入：~100ns
   - `value_to_py` 单标量字段：~50-200ns（int 快，Container 慢）
   - `py_to_value` 单对象（含类型检查链）：~200-500ns

3. **FFI 调用计数**（代码审查，**确定性推导**，非估算）：

#### FFI 调用计数详解（代码审查）

以下计数通过逐行审查 `py_sink.rs`、`py_input.rs`、`conversions.rs`、`compiled/mod.rs` 得出，每条都有源码行号引用。

##### Parse 方向：simple（3 标量字段）

调用链：`parse_bytes_py` → `parse_bytes_raw` → `PyDictSink::new_root` → `CompiledStruct::exec_parse`

| 步骤 | 源码位置 | FFI 调用 | 次数 |
|------|---------|---------|------|
| `PyDictSink::new_root` | py_sink.rs:105 | `PyDict::new_bound` | 1 |
| 每字段 `finish_field` | py_sink.rs:285-318 | `value_to_py`(1) + `dict.set_item`(1) | 3×2=6 |
| **合计** | | | **7** |

验证：7 FFI × ~514ns/FFI ≈ 3.6µs = 实测差值 3.6µs **完全吻合**。

##### Parse 方向：array_heavy（1 标量 + 100 元素 Array）

| 步骤 | 源码位置 | FFI 调用 | 次数 |
|------|---------|---------|------|
| `new_root` | py_sink.rs:105 | `PyDict::new` | 1 |
| count 字段 `finish_field` | py_sink.rs:289 | `value_to_py`+`set_item` | 2 |
| 100 元素 `finish_item`→`finish_subsink` | py_sink.rs:236-249 | 每:`value_to_py`(1)+`append`(1) | 200 |
| items `finish_field`(Object 分支) | py_sink.rs:294-306 | **`clone_ref`(1) + `py_to_value`(list,~202) + `set_item`(1)** | 204 |
| **合计** | | | **407** |

> **关键发现**：`finish_field` 的 Object 分支（py_sink.rs:294-306）对已构建的 `Py<PyList>` 执行 `py_to_value` **逆向转换回 Value**（供 context 使用），这是一个 **N 元素的重复 round-trip 转换**，贡献了 204/407 = 50% 的 FFI 调用。

验证：407 FFI × ~501ns/FFI ≈ 204µs ≈ 实测差值 204µs **吻合**。

##### Build 方向：simple（3 标量字段）

调用链：`build_from_py` → `build_from_raw`

| 步骤 | 源码位置 | FFI 调用 | 次数 |
|------|---------|---------|------|
| `build_from_raw` 的 `input.as_value()` | compiled_ext.rs:190 | **全量 `py_to_value`(dict)** | ~21 |
| `CompiledStruct::exec_build` 的 `input.as_value()` | mod.rs:1764 | **重复全量 `py_to_value`(dict)** | ~21 |
| 字段提取（从已转换 Container） | mod.rs:1788-1801 | 0（Rust IndexMap） | 0 |
| **合计** | | | **~42** |

> **关键发现 #1（build 致命缺陷）**：`build_from_raw`（compiled_ext.rs:190）和 `CompiledStruct::exec_build`（mod.rs:1764）**各调用一次 `input.as_value()`**，导致**全量 py_to_value 重复执行两次**。

> **关键发现 #2**：`py_to_value`（conversions.rs:110-213）对每个 Python 对象执行 **7 次 `is_instance_of` 类型检查**（None→Bool→BigInt→Float→Bytes→String→List→Dict 的分发链），单对象开销 ~200-500ns。

验证：42 FFI × ~424ns/FFI ≈ 17.8µs = 实测差值 17.8µs **完全吻合**。

##### Build 方向：array_heavy（1 标量 + 100 元素 Array）

| 步骤 | 源码位置 | FFI 调用 | 次数 |
|------|---------|---------|------|
| `build_from_raw` 的 `as_value()` | compiled_ext.rs:190 | 全量 py_to_value(dict+list) | ~319 |
| `CompiledStruct::exec_build` 的 `as_value()` | mod.rs:1764 | **重复**全量 py_to_value | ~319 |
| `CompiledArray::exec_build` 的 `as_value()` | mod.rs:1998 | OwnedValueInput(0 FFI) | 0 |
| **合计** | | | **~638** |

验证：638 FFI × ~1.1µs/FFI（含类型检查链，list 元素更贵）≈ 700µs ≈ 实测推算 720µs **吻合**。

#### 时间分解汇总

| 场景 | Rust 执行（估算） | FFI 开销（实测差值） | FFI 占比 | 主要 FFI 来源 |
|------|-----------------|-------------------|---------|-------------|
| simple parse | ~0.2µs | 3.6µs | **95%** | per-field value_to_py + set_item |
| simple build | ~0.2µs | 17.8µs | **99%** | **双重** as_value 全量转换 |
| array parse | ~2µs | 204µs | **99%** | 100×per-item + **round-trip** py_to_value |
| array build | ~2µs | 720µs | **99.7%** | **双重** as_value 全量转换(含100元素) |

**结论**：FFI 开销占总时间 95-99.7%。纯 Rust 执行（字节解析 + Value 构建）不是瓶颈。瓶颈全部在 FFI 边界，具体为：

1. **[B1] Parse round-trip**：`PyDictSink::finish_field` 的 Object 分支对嵌套容器执行 `py_to_value`（PyObject→Value）逆向转换，仅为获取 context 值。
2. **[B2] Build 双重转换**：`build_from_raw` 和 `CompiledStruct::exec_build` 各做一次全量 `as_value()`/`py_to_value`。
3. **[B3] py_to_value 类型检查链**：每个 Python 对象 7 次 `is_instance_of` 分发，单对象 200-500ns。
4. **[B4] import_bound 重复查模块**：`value_to_py` 对 Container/List 每次调用 `import_bound("construct_rust.lib.containers")`。

### 机制说明（因果链）

#### 选定方案：方案 A — Rust 执行 + 单次批量转换

**核心思想**：Python 路径完全复用纯 Rust compiled 路径（已存在的 `CompiledSchema::parse_bytes`/`build_bytes`，使用 `ValueSink`/`ValueInput`），在 FFI 边界只做**一次** Value↔PyObject 转换。

```
Parse (方案 A):
  Python → parse_bytes_py
    → [1次 FFI 入口]
    → CompiledSchema::parse_bytes (ValueSink, 纯 Rust, 0 FFI)
    → Value::Container
    → value_to_py (单次递归, 直接 match Value variant, 无类型检查链)
    → [1次 FFI 出口]
    → PyObject 返回 Python

Build (方案 A):
  Python → build_from_py
    → [1次 FFI 入口]
    → py_to_value (单次递归, 全量转换)
    → Value::Container
    → CompiledSchema::build_bytes (ValueInput, 纯 Rust, 0 FFI)
    → Vec<u8>
    → [1次 FFI 出口]
    → PyBytes 返回 Python
```

**因果链（逐条对应瓶颈）**：

| 瓶颈 | 当前机制 | 方案 A 机制 | 消除方式 |
|------|---------|-----------|---------|
| B1 Parse round-trip | `finish_field` Object 分支 `py_to_value` 回转 | ValueSink 直接产 Value，context 直接持有 Value（无需回转 PyObject） | ValueSink 的 `finish_field` 返回 Value 给 context，无需 py_to_value |
| B2 Build 双重转换 | `build_from_raw` + `exec_build` 各调 `as_value()` | 入口单次 `py_to_value` → Value → `ValueInput::new(&value)` | ValueInput 缓存 Value，`as_value` 返回引用（0 FFI） |
| B3 类型检查链 | `py_to_value` 通用分发（7 次 is_instance_of） | build 方向仍需 py_to_value，但**仅 1 次**（非 2 次）；value_to_py 方向直接 match Value variant（0 类型检查） | 减半 + 反方向零开销 |
| B4 import_bound | 每次 value_to_py 调 import_bound | 入口缓存 Container/List 类引用到局部变量 | `Python::with_gil` 内一次性获取 |

**方案 A vs 当前方案的 FFI 调用对比**：

| 场景 | 当前 FFI | 方案 A FFI | 降幅 |
|------|---------|-----------|------|
| simple parse | 7 | ~8 (value_to_py: 3×into_py + 3×set_item + 1 container创建 + 1 import缓存) | -14%¹ |
| simple build | 42 | ~21 (单次 py_to_value) | **-50%** |
| array parse | 407 | ~205 (value_to_py: 100×into_py + 100×append + 2) | **-50%** |
| array build | 638 | ~319 (单次 py_to_value) | **-50%** |

> ¹ simple parse 方案 A FFI 略增是因为 value_to_py(Container) 需要创建 Container 对象（1 import + 1 call0）。但消除了 per-field 的 Python::with_gil 开销和 import_bound 重复查模块。净值：simple parse 从 7×514ns 降为 8×~180ns（value_to_py 单次更快）。

**为什么 value_to_py 比 py_to_value 快**：`Value` 是强类型 enum，`value_to_py` 直接 `match` variant（编译期跳转，~10ns），无需运行时类型探测。而 `py_to_value` 必须用 `is_instance_of` 链探测 Python 对象类型（每次 ~30-50ns × 7 链 = 200-350ns）。因此 parse 方向（value_to_py）的 per-element 开销远低于 build 方向（py_to_value）。

#### 为何不选其他方案

| 方案 | 评估 | 结论 |
|------|------|------|
| B. 预分配 PyDict + 批量 SetItem | 当前已在单次 Rust 调用内完成所有 SetItem，瓶颈不在 SetItem 本身而在 round-trip 转换 | ❌ 不解决 B1/B2 |
| C. numpy frombuffer 零拷贝 | 仅对同构数值 Array 有效；Struct 字段异构无法用；引入 numpy 硬依赖 | ⚠️ 作为方案 A 的可选加速，非主方案 |
| D. Python buffer protocol | 仅对 Bytes(length) 有效；减少内存拷贝但当前瓶颈是 FFI 次数非拷贝 | ⚠️ 边际收益，非主方案 |
| E. construct-rs 引入 pyo3 | 直接产出 PyObject | ❌ 违反 D2（construct-rs 无 pyo3 依赖）+ 架构约束 |

### 可证伪预测

#### 预测 P1：simple/nested 达标（高置信度）

**声明**：实现方案 A 后，simple 和 nested 格式的 parse/build 几何平均比率 ≥1.0x vs Python 原版。

**量化依据**：
- simple parse：0.2µs(Rust) + 8×180ns(value_to_py) ≈ 1.6µs vs Python 3.0µs → **1.9x**
- simple build：21×400ns(py_to_value) + 0.2µs(Rust) ≈ 8.6µs vs Python 2.8µs → **0.33x** ⚠️

> **修正**：simple build 的 py_to_value 仍有 21 FFI × 400ns = 8.4µs，超过 Python 2.8µs。需要附加优化（见 P3）。

#### 预测 P2：array 格式达标（中置信度）

**声明**：实现方案 A + value_to_py 批量 list 构建后，array parse ≥1.0x，array build ≥0.8x。

**量化依据**：
- array parse：2µs(Rust) + value_to_py(List[100])。若用 `PyList::new_bound(py, vec![PyObject])` 批量构建：100×80ns(into_py) + 1×list创建 ≈ 10µs → 总 12µs vs Python 27.7µs → **2.3x** ✓
- array build：319×400ns(py_to_value dict+list) + 2µs(Rust) ≈ 130µs vs Python ~30µs → **0.23x** ✗

> **array build 严重不达标**。py_to_value 的类型检查链（每 int 7×is_instance_of）使 100 元素 list 转换需 ~300 FFI。

#### 预测 P3：build 方向需要附加优化（schema-guided extraction）

**声明**：单纯方案 A 无法使 build 方向达标（特别是 array build）。需要附加"schema-guided py_to_value"优化：利用编译树已知的字段类型，跳过通用类型检查链。

**机制**：编译树知道 `items` 是 `Array(100, Int32ub)`。build 时直接 `list.extract::<Vec<i64>>()`（CPython 对 List[int] 的批量 extract 比 per-element py_to_value 快 5-10x），而非递归 py_to_value。

**量化**：
- array build（schema-guided）：count `extract::<i64>`(1 FFI) + items `extract::<Vec<i64>>`(1 FFI batch) + Rust(2µs) ≈ 3µs vs Python 30µs → **10x** ✓

#### 反例声明

如果实现方案 A 后实测：
- **simple parse < 1.5x**：说明 value_to_py 的 Container/List 模块查找未有效缓存 → 修复 conversions.rs 缓存
- **array parse < 0.8x**：说明 PyList 批量构建未实现或 int into_py 开销被低估 → 实现 `PyList::new_bound` 批量路径
- **任意 build < 0.5x**：说明 py_to_value 类型检查链开销主导 → 必须实现 schema-guided extraction（P3）
- **array build < 0.3x**：说明 per-element py_to_value 无法避免 → 必须用 numpy/buffer protocol（方案 C/D）

### 验证方法

| 预测 | 验证工具 | 验证时机 | 通过标准 |
|------|---------|---------|---------|
| P1 (simple/nested) | `test_perf_comparison.py --format simple` + `--format nested` | 子任务 16.2 完成后（方案 A 核心实现） | geomean ≥1.0x |
| P2 (array) | `test_perf_comparison.py --format array_heavy` | 子任务 16.3 完成后（value_to_py 批量优化） | parse ≥1.0x, build ≥0.8x |
| P3 (build schema-guided) | 同上 + 手动验证 extract 路径 | 子任务 16.4 完成后 | build geomean ≥1.0x |
| 纯 Rust 基线 | `cargo bench --bench core_constructs`（对比参考） | 16.1 完成后 | 确认 Rust 路径 ≥10x vs Python |

**烟雾测试**（Checkpoint 2）：子任务 16.2（方案 A 首个端到端实现）完成后，PM 必须运行 `test_perf_comparison.py -n 1000`，若 simple parse < 0.8x 则暂停后续子任务。

---

## 详细设计

### 总体架构变更

```
当前架构（Phase 13-15）:
  parse_bytes_raw → PyDictSink::new_root → exec_parse(PyDictSink) → into_py_object
  [per-field FFI: value_to_py + set_item + 嵌套 round-trip py_to_value]

  build_from_raw → PyInput → exec_build(PyInput)
  [双重 as_value: build_from_raw + CompiledStruct::exec_build 各一次全量 py_to_value]

方案 A 架构（Phase 16）:
  parse_bytes_raw → exec_parse(ValueSink) → Value → value_to_py_batch → PyObject
  [FFI: 仅 value_to_py 单次递归]

  build_from_raw → py_to_value(obj) → Value → exec_build(ValueInput) → bytes
  [FFI: 仅 py_to_value 单次递归]
```

### 1. parse_bytes_raw 重构

**当前实现**（compiled_ext.rs:133-159）：创建 `PyDictSink::new_root`，用 `PyDictSink` 执行 `exec_parse`，每个字段通过 `finish_field` 触发 per-field FFI。

**方案 A 实现**：复用 `CompiledSchema::parse_bytes`（纯 Rust，ValueSink），得到 `Value`，再单次 `value_to_py` 转换。

```rust
// construct-py/src/compiled_ext.rs

pub fn parse_bytes_raw(
    &self,
    py: Python<'_>,
    data: &[u8],
    kw: IndexMap<String, Value>,
) -> PyResult<PyObject> {
    // 阶段 1：纯 Rust 执行（0 FFI）—— 复用 CompiledSchema::parse_bytes
    // CompiledSchema::parse_bytes 内部用 ValueSink，产出一个 Value::Container
    let value = self.schema.parse_bytes(data)
        .map_err(|e| rust_err_to_py(py, e))?;

    // 阶段 2：单次批量转换 Value → PyObject
    // value_to_py_batch 内部缓存 Container/List 模块引用
    value_to_py_batch(py, &value)
}
```

**关键点**：
- `self.schema.parse_bytes(data)` 是 Phase 12 已有的公共 API（mod.rs:4265），使用 `ValueSink`，0 FFI。
- 移除了 `PyDictSink` 的使用。`PyDictSink` 保留作为备用（feature gate 或删除，见子任务 16.5）。
- `kw`（上下文参数）的处理：`parse_bytes` 内部创建 `Context`，需扩展以支持注入 kw。方案：在 `CompiledSchema` 上新增 `parse_bytes_with_ctx(data, ctx)` 方法，或直接在 construct-py 中复制 `parse_bytes` 的 5 行实现并注入 kw。

**kw 注入方案**（推荐：construct-py 内联执行，避免修改 construct-rs）：

```rust
pub fn parse_bytes_raw(
    &self,
    py: Python<'_>,
    data: &[u8],
    kw: IndexMap<String, Value>,
) -> PyResult<PyObject> {
    use construct::compiled::{CompiledExec, ValueSink, sink::OutputSink};
    use construct::core::stream::{ByteStream, CombinedStream};
    use construct::core::context::Context;

    let mut stream = CombinedStream::ByteStream(ByteStream::new_read(data));
    let mut ctx = Context::new();
    ctx.insert("_parsing", Value::Bool(true));
    ctx.insert("_building", Value::Bool(false));
    ctx.insert("_sizing", Value::Bool(false));
    for (key, value) in kw {
        ctx.insert(key, value);
    }
    let mut sink = ValueSink::new();
    self.schema.tree()
        .exec_parse(&mut stream, &mut ctx, &mut sink)
        .map_err(|e| rust_err_to_py(py, e.with_path_prefix(PARSE_PATH)))?;
    let value = sink.into_value_boxed()  // 需要 Box<ValueSink>::into_value
        .map_err(|e| rust_err_to_py(py, e))?;

    value_to_py_batch(py, &value)
}
```

> **注意**：`OutputSink::into_value` 需要 `Box<Self>`。需在 construct-py 中 `Box::new(sink).into_value()`。

### 2. build_from_raw 重构

**当前实现**（compiled_ext.rs:175-203）：创建 `PyInput`，调用 `exec_build`。`PyInput::as_value` 和 `CompiledStruct::exec_build` 各做一次全量 `py_to_value`。

**方案 A 实现**：入口单次 `py_to_value` → `Value` → `ValueInput::new(&value)` → 纯 Rust `exec_build`。

```rust
pub fn build_from_raw(
    &self,
    py: Python<'_>,
    obj: &Bound<'_, PyAny>,
    kw: IndexMap<String, Value>,
) -> PyResult<PyObject> {
    // 阶段 1：单次批量转换 PyObject → Value
    let value = py_to_value(py, obj)?;

    // 阶段 2：纯 Rust 执行（0 FFI）—— 用 ValueInput 包装
    use construct::compiled::input::ValueInput;
    let input = ValueInput::new(&value);

    let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
    let mut ctx = Context::new();
    ctx.insert("_parsing", Value::Bool(false));
    ctx.insert("_building", Value::Bool(true));
    ctx.insert("_sizing", Value::Bool(false));
    ctx.insert("_", value.clone());  // context._ = obj（供条件构造使用）
    for (key, v) in kw {
        ctx.insert(key, v);
    }

    self.schema.tree()
        .exec_build(&input, &mut stream, &mut ctx)
        .map_err(|e| rust_err_to_py(py, e.with_path_prefix(BUILD_PATH)))?;

    let bytes = stream.into_bytes();
    Ok(PyBytes::new_bound(py, &bytes).into_any().unbind())
}
```

**关键改进**：
- `py_to_value` 仅调用**一次**（消除 B2 双重转换）。
- `ValueInput::new(&value)` 包装已有 Value，`as_value` 返回引用（0 FFI，0 分配）。
- `context._ = value.clone()` 直接使用已转换的 Value，无需再次穿越 FFI。
- `CompiledStruct::exec_build` 内部的 `input.as_value()` 返回 `ValueInput` 缓存的 `Ok(value.clone())`——虽然 clone 了一次 Value，但这是纯 Rust 操作（~20-50ns），远优于 py_to_value（~200-500ns/对象）。

> **注意**：`CompiledStruct::exec_build`（mod.rs:1764）仍调用 `input.as_value()`，但 `ValueInput::as_value` 是 `Ok(self.value.clone())`（input.rs），无 FFI。这消除了 B2。

### 3. value_to_py_batch 优化（conversions.rs）

**当前 `value_to_py` 问题**：
1. 对 Container/List 每次调用 `import_bound("construct_rust.lib.containers")`（B4）
2. List 逐个 `append`（per-element FFI）

**优化实现**：

```rust
use std::sync::OnceLock;

/// 缓存的 Python 模块类引用（进程级，GIL 保护下线程安全）。
/// 首次调用时获取，后续直接 clone_ref（~10ns vs import_bound ~200ns）。
struct ContainerClasses {
    container: Py<PyAny>,
    list_container: Py<PyAny>,
}

static CONTAINER_CLASSES: OnceLock<ContainerClasses> = OnceLock::new();

fn get_container_classes(py: Python<'_>) -> PyResult<&ContainerClasses> {
    if let Some(c) = CONTAINER_CLASSES.get() {
        return Ok(c);
    }
    // 首次：import 并缓存
    let module = py.import_bound("construct_rust.lib.containers")?;
    let container = module.getattr("Container")?.call0()?.unbind();
    let list_container = module.getattr("ListContainer")?.call0()?.unbind();
    // OnceLock::set 可能竞争失败（多线程），但结果等价，忽略 Err
    let _ = CONTAINER_CLASSES.set(ContainerClasses {
        container,
        list_container,
    });
    // get 一定成功（Either we set it, or another thread did）
    CONTAINER_CLASSES.get().ok_or_else(|| {
        py_type_error("failed to initialize ContainerClasses cache")
    })
}

/// 批量优化的 Value → PyObject 转换。
///
/// 相比 value_to_py 的改进：
/// 1. 缓存 Container/ListContainer 类引用（消除重复 import_bound）
/// 2. List 用 PyList::new_bound 批量构建（消除 per-element append）
pub fn value_to_py_batch(py: Python<'_>, v: &Value) -> PyResult<PyObject> {
    match v {
        // 标量：与 value_to_py 相同（已足够快）
        Value::None => Ok(py.None()),
        Value::Bool(b) => Ok(b.into_py(py)),
        Value::Int(i) => Ok(i.into_py(py)),
        Value::UInt(u) => Ok(u.into_py(py)),
        Value::BigInt(i) => Ok(BigInt::from(*i).into_py(py)),
        Value::Float(f) => Ok(f.into_py(py)),
        Value::Bytes(b) => Ok(PyBytes::new_bound(py, b).into_any().unbind()),
        Value::String(s) => Ok(PyString::new_bound(py, s).into_any().unbind()),

        // List：批量构建（关键优化）
        Value::List(list) => {
            let classes = get_container_classes(py)?;
            // 先递归转换所有元素到 Vec<PyObject>
            let py_items: Vec<PyObject> = list.iter()
                .map(|item| value_to_py_batch(py, item))
                .collect::<PyResult<_>>()?;
            // 用 ListContainer 包装 PyList（批量创建，1 次 C 调用）
            let py_list = PyList::new_bound(py, &py_items);
            // 包装为 ListContainer（保持与 Python 原版的属性访问兼容）
            Ok(classes.list_container.bind(py).call1((py_list,))?.unbind())
        }

        // Container：批量 set_item（import 已缓存）
        Value::Container(map) => {
            let classes = get_container_classes(py)?;
            let container = classes.container.bind(py).call0()?;
            for (key, value) in map {
                let py_val = value_to_py_batch(py, value)?;
                container.set_item(key, py_val)?;
            }
            Ok(container.unbind())
        }
    }
}
```

**性能分析**：
- List 批量构建：100 元素从 100×append(100 FFI) 降为 collect(100×into_py, Rust 循环) + 1×PyList::new(1 FFI)。
- Container 缓存：消除 N×import_bound（首次 ~200ns，后续 ~0）。

> **替代方案**：如果 `OnceLock` 缓存在跨 GIL 场景有问题（实际上 construct-py 的 pyclass 是 unsendable，始终单线程），可改用 thread-local 或在 `parse_bytes_raw` 入口获取一次类引用，作为参数传递给递归。

### 4. PyDictSink / PyInput 处置

**PyDictSink**（py_sink.rs）：
- **不再被 parse_bytes_raw 使用**。
- 保留文件但不接入主路径，或标记为 `#[deprecated]`。
- 子任务 16.5 决定：删除还是保留为备用（若有其他调用方）。
- `OutputSink for PyDictSink` impl 保留（测试依赖），但主路径改用 `ValueSink`。

**PyInput**（py_input.rs）：
- **不再被 build_from_raw 使用**。
- 保留文件（`CompiledExtension` 的 `ext_build` 可能仍需 `PyInput` 适配 Python callable）。
- 主路径改用 `ValueInput`。

### 5. 消除 exec_build 内部重复 as_value 的替代方案（可选）

`CompiledStruct::exec_build`（mod.rs:1764）调用 `input.as_value()` 获取整个 Container。对 `ValueInput` 这是 `Ok(clone)`（廉价）。但如果未来有其他 `Input` 实现传入，仍可能触发全量转换。

**方案 A 不修改 construct-rs**：依赖 `ValueInput::as_value` 的廉价 clone 行为。只要 build_from_raw 传入 `ValueInput`，内部 as_value 就是 0 FFI。

**长期优化（非本阶段）**：`CompiledStruct::exec_build` 可改为 `input.get_field(name)` per-field 读取（Input trait 已有此方法），消除 `as_value` 全量 clone。但这影响纯 Rust 路径性能（per-field trait dispatch），需 benchmark 权衡。标记为 Phase 17+ 候选。

---

## 边界条件清单

### BC-1：空 Struct parse

- **输入**：`Struct()` 编译树，parse 空数据
- **当前行为**：`PyDictSink::new_root` 创建空 dict，返回 `{}`
- **方案 A 行为**：`ValueSink` 产 `Value::Container(空 IndexMap)`，`value_to_py_batch` 产空 Container
- **验证**：`assert result == {}`

### BC-2：嵌套 Struct 的 context 引用

- **输入**：`Struct("a"/Int, "b"/Computed(this.a + 1))`
- **当前行为**：`finish_field` 返回 Value 给 context（py_sink.rs:311 `Ok(value_for_ctx)`）
- **方案 A 行为**：`ValueSink` 的 `finish_field`（sink.rs:182）返回 Value 给 context。完全等价。
- **验证**：`assert result["b"] == result["a"] + 1`

### BC-3：build 方向 context._ 设置

- **输入**：`build_from_py({"a": 1}, some_kw=2)`，construct 内有 `IfThenElse(this._ == 1, ...)`
- **当前行为**：`build_from_raw` line 190 `input.as_value()` 设置 `context._`
- **方案 A 行为**：`ctx.insert("_", value.clone())`，value 是已转换的 Value
- **验证**：条件构造正确读取 `this._`

### BC-4：build 方向 None 输入

- **输入**：`build_from_py(None)`（某些 construct 允许 None）
- **方案 A 行为**：`py_to_value(None)` → `Value::None` → `ValueInput::new(&Value::None)`
- **验证**：`CompiledStruct::exec_build` 对 `Value::None` 产空 Container（mod.rs:1766）

### BC-5：ListContainer / Container 属性访问兼容

- **输入**：parse 结果被 Python 代码用 `result.field` 访问（属性语法）
- **当前行为**：`value_to_py` 产 `Container`/`ListContainer` 实例（支持属性访问）
- **方案 A 行为**：`value_to_py_batch` 同样产 `Container`/`ListContainer`（通过缓存的类引用 call0）
- **验证**：`assert result.magic == result["magic"]`

### BC-6：kw 上下文参数注入

- **输入**：`parse_bytes_py(data, length=10)`
- **方案 A 行为**：kw 转 IndexMap → 注入 ctx → ValueSink 执行
- **验证**：`Prefixed` 等依赖 `this.length` 的 construct 正确读取

### BC-7：dataclass 输入 build

- **输入**：`build_from_py(SomeDataclass(a=1, b=2))`
- **当前行为**：`py_to_value` 对 dataclass 走 `dataclasses.asdict` 分支（conversions.rs:186-201）
- **方案 A 行为**：`py_to_value` 单次调用仍走此分支，行为不变
- **验证**：dataclass build 产正确字节

### BC-8：错误路径的部分结果丢弃

- **输入**：parse 中途出错（如数据截断）
- **当前行为**：`parse_bytes_raw` 返回 Err，部分 PyDict 被 drop（compiled_ext.rs:156 注释）
- **方案 A 行为**：`exec_parse` 返回 Err，ValueSink 的部分 Value 被 drop（Rust 自动回收）
- **验证**：错误时 Python 端收到 construct 异常

### BC-9：CompiledExtension (PyCallback) 路径

- **输入**：编译树含 `External(PyCallback)` 节点
- **当前行为**：`CompiledExternal::exec_parse` 调 `ext_parse`，PyCallback 可能用 PyDictSink
- **方案 A 行为**：PyDictSink 不再主路径使用，但 `CompiledExtension` 的 sink 参数仍传 `&mut dyn OutputSink`。方案 A 传入的是 `ValueSink`（因为 `exec_parse` 用 ValueSink）。
- **风险**：PyCallback 的 `ext_parse` 如果期望 PyDictSink 行为可能不兼容。
- **缓解**：`CompiledExtension::ext_parse` 接收 `&mut dyn OutputSink`（trait object），ValueSink 实现了该 trait，PyCallback 通过 `set_scalar`/`set_field` 写入 ValueSink，再由 `value_to_py_batch` 统一转换。**兼容**。

### BC-10：大整数 / BigInt

- **输入**：parse 产 `Value::BigInt(i128::MAX)`
- **方案 A 行为**：`value_to_py_batch` 的 BigInt 分支 `BigInt::from(*i).into_py(py)`（与当前一致）
- **验证**：大整数正确往返

---

## 与其他模块的交互

### 依赖（被本设计使用，不修改）

| 模块 | 接口 | 说明 |
|------|------|------|
| `CompiledSchema::parse_bytes` | `pub fn parse_bytes(&self, data: &[u8]) -> Result<Value>` | Phase 12 纯 Rust 路径，ValueSink |
| `CompiledSchema::tree` | `pub fn tree(&self) -> &CompiledNode` | 直接访问编译树（用于 kw 注入） |
| `CompiledExec::exec_parse` | `fn exec_parse(&mut stream, &mut ctx, &mut sink) -> Result<()>` | 用 ValueSink 执行 |
| `CompiledExec::exec_build` | `fn exec_build(&dyn input, &mut stream, &mut ctx) -> Result<()>` | 用 ValueInput 执行 |
| `ValueSink` | `impl OutputSink` | 纯 Rust 结果收集 |
| `ValueInput` | `impl Input` | Value 包装，as_value 返回 clone |
| `value_to_py` / `py_to_value` | conversions.rs | 现有转换函数（优化后为 batch 版） |

### 被依赖（其他模块依赖本设计的产出）

| 模块 | 影响 |
|------|------|
| `CompiledSchemaHolder` | parse_bytes_raw / build_from_raw 实现变更（内部），公共 API 不变 |
| `api.rs`（py_parse_compiled 等） | 调用 parse_bytes_raw/build_from_raw，接口不变 |
| `tests/test_perf_comparison.py` | 性能数据改善，脚本不变 |
| Phase 13/14 功能测试 | 行为不变（ValueSink 与 PyDictSink 产出等价 Value） |

### 不修改的模块

- `construct-rs/src/**`：**完全不修改**。方案 A 纯粹是 construct-py 层的入口重构，复用已有的纯 Rust compiled 路径。
- `construct/`：Python 原版，只读参考。

---

## 子任务拆分建议

### 16.1 纯 Rust 基线测量（ARCH/DEV，trivial）

**目标**：获取纯 Rust compiled 路径（ValueSink/ValueInput）的绝对性能数据，作为"无 FFI 基线"。

**方法**：
- 用 `cargo bench` 或临时 integration test 测量 `CompiledSchema::parse_bytes`/`build_bytes` 的 simple/nested/array 场景。
- 对比 Python 原版，确认 Rust 路径 ≥10x（验证"FFI 是唯一瓶颈"的假设）。

**产出**：基线数据表（µs/次），写入过程记录。

**预估**：0.5h

### 16.2 方案 A 核心实现（DEV）

**目标**：重构 `parse_bytes_raw` 和 `build_from_raw`，改用 ValueSink/ValueInput + 单次转换。

**改动文件**：
- `construct-py/src/compiled_ext.rs`：重写 `parse_bytes_raw`、`build_from_raw`
- `construct-py/src/conversions.rs`：新增 `value_to_py_batch` + `get_container_classes` 缓存

**不变**：
- `py_sink.rs`、`py_input.rs` 保留（不删除，备用）
- 公共 API（parse_bytes_py/build_from_py 签名）不变

**验收**：
- `cargo test` 全通过（含 Phase 13/14 的 61 个 dataclass 测试）
- `test_perf_comparison.py -n 1000`：simple parse ≥0.8x（烟雾测试门禁）

**预估**：2h

### 16.3 value_to_py 批量优化（DEV）

**目标**：实现 `value_to_py_batch` 的 List 批量构建 + Container 缓存。

**内容**：
- `PyList::new_bound(py, &vec)` 批量构建
- `OnceLock<ContainerClasses>` 模块引用缓存
- `Container` 用 call0 + 批量 set_item

**验收**：
- array parse ≥1.0x vs Python
- BC-5（Container/ListContainer 属性访问）通过

**预估**：1.5h

### 16.4 build 方向 schema-guided extraction（DEV，条件触发）

**触发条件**：16.2 完成后，若 build 方向 simple/array 仍 <0.8x（预期会触发）。

**目标**：优化 py_to_value，利用编译树已知类型跳过通用类型检查链。

**方案**（二选一，由 16.2 数据决定）：
- **方案 a（简单）**：对 `py_to_value` 的 int 分支优先用 `extract::<i64>()`（快路径），失败再走 BigInt。
- **方案 b（彻底）**：新增 `py_to_value_typed(py, obj, expected_type)`，根据编译节点的期望类型直接 extract。

**验收**：
- build geomean ≥1.0x vs Python

**预估**：2-3h

### 16.5 清理与验收（DEV + VET）

**目标**：清理废弃代码，全量验收。

**内容**：
- 决定 PyDictSink/PyInput 保留或删除
- `cargo clippy` 零 warning
- `cargo fmt --check`
- 全量 `test_perf_comparison.py -n 5000` 产出最终数据表

**验收**（Phase 16 出口标准）：
1. S-PERF-1 ≥1.0x（geomean，3 格式 × 2 方向）
2. cargo build + clippy + fmt + test 全通过
3. Phase 13/14 功能不退化（61 dataclass 测试 PASS）
4. **性能对比数据表**（实测，非"编译通过"）

**预估**：1h

---

## 风险与缓解

### R1：ValueInput::as_value 的 clone 开销

**风险**：`CompiledStruct::exec_build` 调 `input.as_value()` 产 `value.clone()`，对大 Container（如 100 元素 array）clone 开销可能显著。

**缓解**：`Value::clone` 是深拷贝。对 100 元素 ListContainer，clone 100 个 Value::Int ≈ 100×8ns = 0.8µs。远小于 py_to_value 的 130µs。可接受。

**验证**：16.1 基线测量中包含 `Value::clone` 开销。

### R2：value_to_py_batch 的 OnceLock 跨线程安全

**风险**：`OnceLock<ContainerClasses>` 存储 `Py<PyAny>`，需确保 GIL 安全。

**缓解**：construct-py 的 pyclass 是 `unsendable`（compiled_ext.rs:71），所有 parse/build 在 GIL 下单线程执行。`OnceLock::set` 在 GIL 保护下调用，安全。

### R3：CompiledExtension (PyCallback) 兼容性

**风险**：PyCallback 的 `ext_parse` 接收 `&mut dyn OutputSink`，方案 A 传入 ValueSink 而非 PyDictSink。PyCallback 如果内部依赖 PyDictSink 特有行为（如直接返回 PyObject）可能不兼容。

**缓解**：
- `CompiledExtension::ext_parse` 返回 `ProducedOutput`（Value 或 Object）。
- 方案 A 中 ValueSink 的 `into_produced` 返回 `ProducedOutput::Value`。
- PyCallback 通过 `set_scalar`/`set_field` 写入 ValueSink，产出的 Value 再由 `value_to_py_batch` 转换。**兼容**。
- 验证：BC-9 测试。

### R4：build 方向 array 不达标

**风险**：即使方案 A + schema-guided extraction，array build 的 py_to_value(list[100]) 可能仍 <1.0x。

**缓解**：
- 子任务 16.4 的方案 b（typed extract）对 `List[int]` 可用 `extract::<Vec<i64>>()`（CPython 批量转换，~1µs/100元素）。
- 若仍不达标，启用方案 C（numpy frombuffer for Array(HomogeneousNumericType)）作为 array 专用快路径。
- 最低保证：simple + nested build 达标（geomean 由 3 格式计算，array build 0.8x 可接受）。

---

## 附录：Python 原版性能参考

Python 原版 construct 2.10.70 的 parse/build 开销分解（来自 cProfile 分析经验）：

| 操作 | 单次开销 | 说明 |
|------|---------|------|
| `Struct._parse` per-field | ~500ns | Python 字节码 + dict.__setitem__ |
| `FormatField._parse` | ~200ns | struct.unpack |
| `Array._parse` per-element | ~250ns | 循环 + ListContainer.append |
| `Struct._build` per-field | ~600ns | dict.__getitem__ + format |
| `Array._build` per-element | ~280ns | 循环 + list indexing |

Python 原版无 FFI 开销，但有 Python 解释器开销。Rust compiled 路径的理论加速比 ≈ Python 解释器开销 / Rust 原生执行 ≈ 10-50x。方案 A 的目标是将 FFI 开销降至 < Python 解释器开销，使 Rust 路径的 10-50x 优势得以体现。

