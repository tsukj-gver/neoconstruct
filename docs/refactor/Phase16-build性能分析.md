# Phase 16 Build 方向性能瓶颈分析

> **阶段**：Phase 16.4 完成后性能验收
> **分析师**：ARCH
> **日期**：2026-06-20
> **方法**：基于工具的性能分析（cProfile + 微基准 + 代码审查）
> **结论**：设计成本模型有 3 个根本性错误，导致 build 预测偏差 5-17x

---

## §1 问题陈述

Phase 16.4 完成了 build 方向的 Py 路径实现。性能测试结果：

| 格式 | build 预测 | build 实际 | 偏差倍数 |
|------|-----------|-----------|---------|
| simple (3字段) | ~5.4x (≤515ns) | **0.74-0.82x** (~3.7µs) | **6.5x** |
| nested (2级) | ~3.1x (≤1.0µs) | **0.61x** (~9.4µs) | **5.1x** |
| array (100元素) | ~5.4x (≤5.6µs) | **0.31x** (~88µs) | **17x** |

预测与实际差 5-17 倍。本报告用工具定位实际瓶颈的根因。

---

## §2 工具化数据采集

### 2.1 环境与基准

- **Python**：3.14.2（`PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1`）
- **construct 原版**：2.10.70
- **测量方法**：`time.perf_counter` 端到端，GC 禁用，20000 次迭代取平均
- **测试脚本**：`construct-py/tests/test_build_profile.py`、`test_build_microbench.py`、`test_build_ffi_cost.py`

### 2.2 cProfile 分析结果

对三种格式各运行 2000 次 `build_from_py`：

```
simple build:    2002 function calls — 仅 2000 次 build_from_py，0 次 Python 回调
nested build:    2002 function calls — 仅 2000 次 build_from_py，0 次 Python 回调
array_heavy:    10002 function calls — 2000 次 build_from_py + 8000 次 Container.__init__
```

**关键发现**：
- simple/nested 的全部时间在 Rust C 扩展内（cProfile 无法看到内部）
- array_heavy 每次 build 触发 **4 次 Container.__init__ + 2 次 ListContainer.__init__**
- 经隔离测试确认：Container 创建仅在 `Struct 包含 Array 字段` 时发生（4 Container + 2 ListContainer/构建）

### 2.3 微基准：固定开销与边际成本

#### 2.3.1 入口固定开销（0-字段 Struct）

```
0-field Struct build:  1.181µs  （纯入口开销）
```

设计预测 simple build 总耗时 ~515ns。**固定开销一项就是预测的 2.3 倍。**

#### 2.3.2 每标量字段边际成本

| 字段数 | 总耗时 | 边际/字段 |
|--------|--------|----------|
| 0 | 1.18µs | — |
| 1 | 1.73µs | 553ns |
| 2 | 2.04µs | 305ns |
| 3 | 2.43µs | 388ns |
| 5 | 3.36µs | 464ns |
| 10 | 5.57µs | 443ns |

**稳态边际：~440ns/字段**。设计预测 ~140ns/字段。**实际是预测的 3.1 倍。**

#### 2.3.3 Array 每元素边际成本

| 元素数 | 总耗时 | 边际/元素 |
|--------|--------|----------|
| 0 | 12.27µs | — (固定) |
| 1 | 13.88µs | 1.61µs |
| 5 | 17.05µs | 0.85µs |
| 10 | 21.31µs | 0.85µs |
| 50 | 51.44µs | 0.76µs |
| 100 | 88.24µs | 0.74µs |
| 200 | 161.8µs | 0.74µs |

**稳态边际：~736ns/元素**。设计预测 ~35ns/元素。**实际是预测的 21 倍。**

**0-元素 Array build 固定开销 12.27µs**（对比 0-字段 Struct 的 1.18µs：count + items 字段 + Container/ListContainer 创建额外贡献 ~11µs）。

#### 2.3.4 int-array vs bytes-array 对比（验证 shallow list 迭代）

| 元素数 | int array | bytes array | 差值/元素 |
|--------|-----------|-------------|----------|
| 10 | 19.7µs | 28.5µs | 875ns |
| 50 | 50.1µs | 93.1µs | 860ns |
| 100 | 170.4µs | 294.3µs | 1238ns |

bytes 比 int 慢 ~0.9-1.2µs/元素。差异来自 `extract_scalar_short_circuit` 的类型探测路径：int 在第 3 次探测（i64）命中，bytes 在第 6 次探测（&\[u8]）命中，多 ~3 次 FFI。

**这证明 `shallow_py_to_value_for_ctx` 确实在逐元素遍历 list**——如果只存空占位符（设计 §4.6 的声明），int 和 bytes 的每元素成本应该相同。

### 2.4 有效 FFI 成本推导

从每字段边际成本 ~440ns 反推：

```
每字段操作（代码审查）：
  sub_field_py:    get_item(1 FFI) + clone/unbind(1 FFI) = 2 FFI
  extract_ctx_value_py:  extract_scalar_short_circuit = 3-5 FFI
  leaf exec_build_py:    extract_scalar_py = 3-5 FFI（重复！）
  inner.build:    0 FFI（纯 Rust）
合计: 8-12 FFI/字段
```

反推单次 FFI 成本：
- 8 FFI × X = 440ns → X = 55ns
- 12 FFI × X = 440ns → X = 37ns

**有效 FFI 成本约 37-55ns**，与设计假设的 ~40ns 基本吻合。

**结论：FFI 单次成本估计正确，但 FFI 调用次数被严重低估（实际 8-12 次/字段 vs 预测 4 次/字段）。**

---

## §3 根因分析（基于数据）

### RC1 🔴 致命（array 主导）：`shallow_py_to_value_for_ctx` 逐元素遍历 list

**位置**：`py_input.rs:286-296`

```rust
// 实际实现（逐元素遍历）
if let Ok(list) = obj.downcast::<PyList>() {
    let mut items = Vec::with_capacity(list.len());
    for item in list {                                    // ← 遍历全部元素！
        if let Some(scalar) = extract_scalar_short_circuit(&item) {
            items.push(scalar);
        } else {
            items.push(Value::None);
        }
    }
    return Ok(Value::List(items));
}
```

**设计 §4.6 的声明**：
> "对 list：存空占位符（不递归元素）"
> `Value::List(Vec::new())`

**矛盾**：实际实现遍历全部 N 个元素，每个元素执行 `extract_scalar_short_circuit`（3-6 次 FFI），产出 `Value::List` 存入 ctx。但基准格式不使用 `this.items` 表达式，**这个 Value::List 从未被读取**。

**成本量化**（100 元素 array）：
- 100 元素 × ~5 FFI × ~40ns = ~20µs 纯浪费
- 实测每元素边际 736ns 中约 ~150-200ns 来自此遍历

**验证**：§2.3.4 的 int vs bytes 差异直接证明此遍历存在。

### RC2 🔴 致命（全格式）：`extract_ctx_value_py` 对每个 Struct 字段调用

**位置**：`py_exec.rs:1226`（CompiledStruct::exec_build_py）

```rust
// 实际实现
for field in &self.fields {
    let field_input = input.sub_field_py(py, name)?;        // ← 设计有
    let ctx_value = field_input.extract_ctx_value_py(py)?;  // ← 设计没有！
    child_ctx.insert(name.clone(), ctx_value);               // ← 设计有
    field.subcon.exec_build_py(py, &*field_input, ...)?;   // ← 设计有
}
```

**设计 §4.4 的代码草图**：

```rust
// 设计草图（无 extract_ctx_value_py）
for field in &self.fields {
    let build_value = sub.extract_scalar_py(py)?;   // 只提取一次
    child_ctx.insert(name.clone(), build_value.clone());
    field.subcon.exec_build_py(py, field_input, ...)?;
}
```

**矛盾**：设计草图只提取标量一次（extract_scalar_py），实际实现提取两次（extract_ctx_value_py + 子节点 extract_scalar_py）。

**成本量化**：
- 每标量字段：extract_ctx_value_py 额外贡献 ~3-5 FFI = ~150-200ns
- Array 字段：extract_ctx_value_py 触发 RC1 的全列表遍历 = N × ~150ns

### RC3 🟡 重要（全格式）：标量双重提取

**位置**：py_exec.rs:1226 + py_exec.rs:204

CompiledStruct::exec_build_py 对每个字段调 `extract_ctx_value_py`（提取标量到 Value），然后子节点 leaf 的 exec_build_py 又调 `extract_scalar_py`（再次提取同一个标量）。

**同一标量被从 Python 对象提取两次**：
- 第一次：extract_ctx_value_py → extract_scalar_short_circuit → Value::Int(42)
- 第二次：leaf exec_build_py → extract_scalar_py → extract_scalar_short_circuit → Value::Int(42)

**成本**：每标量字段 ~3-5 FFI 冗余 = ~150ns 浪费。

### RC4 🟡 重要（固定开销）：入口固定开销 1.18µs

**实测**：0-字段 Struct build = 1.18µs。

**设计假设**：~200ns 固定开销。

**来源分解**（估算）：
| 组件 | 估算耗时 |
|------|---------|
| pyo3 `#[pymethod]` 分发 + 参数提取 | ~200-300ns |
| `opt_kw_to_indexmap(py, None)` | ~50ns |
| PyDictInput::from_bound (incref) | ~50ns |
| ByteStream::new_write | ~50ns |
| Context::new + 3 inserts | ~100ns |
| root extract_ctx_value_py（空 dict） | ~100ns |
| exec_build_py_dispatch (match) | ~20ns |
| stream.into_bytes + PyBytes::new | ~100ns |
| **小计** | **~670-870ns** |
| **未解释差距** | **~310-510ns** |

未解释的 ~310-510ns 可能来自 pyo3 内部 GIL 管理、错误处理路径初始化、或其他框架开销。

### RC5 🟢 次要：extract_scalar_short_circuit 的 bool-before-int 探测

**位置**：py_input.rs:221-227

```rust
if let Ok(b) = obj.extract::<bool>() {
    if obj.is_instance_of::<PyBool>() {   // ← int 会触发但失败
        return Some(Value::Bool(b));
    }
}
```

对每个 int 值，`extract::<bool>()` 可能成功（truthy int 返回 true），然后 `is_instance_of::<PyBool>()` 失败。浪费 ~2 FFI/int。

**成本**：~80ns/int 标量。在全 int 格式（如 array）中累积。

### RC6 🟢 次要（array）：Container/ListContainer 创建

**实测**：Struct+Array build 触发 4 Container + 2 ListContainer 创建。

**来源**：无法通过 Python 侧追踪精确定位（调用栈显示直接来自 Rust C 扩展内部，无中间 Python 帧）。推测与 ContainerClasses 的 OnceLock 初始化或某个 pyo3 操作的类型检查链有关。

**成本**：Container.__init__ ≈ 375ns/次 × 4 + ListContainer ≈ 200ns/次 × 2 = ~1.9µs。占 array build 88µs 的 ~2%。

---

## §4 预测偏差归因

### 4.1 预测 vs 实际的差距分解

| 偏差来源 | simple build | array build |
|---------|-------------|-------------|
| **RC1** shallow list 遍历 | — (无 list 字段) | **~15-20µs** (100元素×~150-200ns) |
| **RC2** per-field extract_ctx_value_py | ~450ns (3字段×~150ns) | ~450ns (count) + **~15-20µs** (items, 与RC1重叠) |
| **RC3** 双重提取 | ~450ns (3字段×~150ns) | ~150ns (count) |
| **RC4** 固定开销低估 | ~980ns (1180-200) | ~980ns |
| **RC5** bool 探测 | ~240ns (3字段×~80ns) | ~8µs (100元素×~80ns) |
| **RC6** Container 创建 | — | ~1.9µs |
| **设计预测总耗时** | 515ns | 5.6µs |
| **实际总耗时** | ~3.7µs | ~88µs |
| **偏差** | 7.2x | 15.7x |

### 4.2 根本原因总结

设计成本模型有 **三个根本性错误**：

1. **代码草图与实现不一致（RC2）**：设计 §4.4 的 Struct build 代码草图不含 `extract_ctx_value_py`，但实际实现每字段调用。设计者写代码草图时假想了一种更优的实现（只提取一次标量），但实现者（DEV）独立加入了 ctx value 提取逻辑。**这是设计文档未约束实现细节导致的。**

2. **设计声明与实现矛盾（RC1）**：设计 §4.6 明确声明 list "存空占位符（不递归元素）"，但实现 `shallow_py_to_value_for_ctx` 逐元素遍历。实现者可能认为"浅转换"意味着"不递归嵌套 dict，但提取 list 标量元素"，而设计者意图是"不递归任何元素"。

3. **固定开销低估（RC4）**：设计假设 pyo3 入口开销 ~200ns，实际 ~1.2µs。pyo3 的 `#[pymethod]` 分发涉及 Python 帧创建、参数解析、GIL 确认等隐式开销，远超裸 C API 调用。

---

## §5 优化建议

### 5.1 P0：消除 shallow list 遍历（对应 RC1）

**方案**：修改 `shallow_py_to_value_for_ctx` 的 list 分支，存空占位符（回归设计 §4.6 原意）：

```rust
// 修改后
if let Ok(_list) = obj.downcast::<PyList>() {
    // 不遍历元素 —— ctx 表达式 rarely 引用 list 元素
    return Ok(Value::List(Vec::new()));
}
```

**预期收益**：array build 100 元素减少 ~15-20µs → 从 88µs 降至 ~68-73µs。仍不达标（目标 ~30µs），但改善 ~20%。

**风险**：若格式使用 `this._.items[0]` 或 `this._.items.length` 等表达式，浅占位返回空 list 导致不正确。需评估 61 个 dataclass 测试是否有此模式。

### 5.2 P0：消除 per-field extract_ctx_value_py 的冗余提取（对应 RC2 + RC3）

**方案 A（最小改动）**：CompiledStruct::exec_build_py 仅在有 ctx 表达式需求时提取 ctx value。

编译树在编译期已知哪些字段被表达式引用（SchemaCompiler 已有此信息）。对于不被 `this.field` 引用的字段，跳过 extract_ctx_value_py。

```rust
// 伪代码
for field in &self.fields {
    let field_input = input.sub_field_py(py, name)?;
    if fieldreferenced_in_exprs {  // 编译期标志
        let ctx_value = field_input.extract_ctx_value_py(py)?;
        child_ctx.insert(name.clone(), ctx_value);
    }
    field.subcon.exec_build_py(py, &*field_input, ...)?;
}
```

**预期收益**：simple build 从 ~3.7µs 降至 ~2.8µs（减少 3 × ~150ns = ~450ns + 消除双重提取 ~450ns ≈ ~900ns）。达到 ~1.0x（Python 原版 ~3.0µs）。

**方案 B（更彻底）**：leaf exec_build_py 直接接收已提取的 Value，跳过 extract_scalar_py：

将 CompiledStruct::exec_build_py 改为先提取标量（一次），存入 ctx，然后传递 PyScalarInput 给子节点：

```rust
for field in &self.fields {
    let field_input = input.sub_field_py(py, name)?;
    let scalar = field_input.extract_scalar_py(py)?;   // 只提取一次
    if let Some(name) = &field.name {
        child_ctx.insert(name.clone(), scalar.clone());
    }
    // 用 PyScalarInput 包装已提取的 Value，子节点不再提取
    let scalar_input = PyScalarInput::new(scalar);
    field.subcon.exec_build_py(py, &scalar_input, ...)?;
}
```

注意：这只对 leaf 字段有效。对于 composite 字段（如 nested struct），需要保留 field_input 让子节点自己提取。

### 5.3 P1：消除 bool-before-int 探测开销（对应 RC5）

**方案**：对编译树已知的整型字段（FormatField with int format），使用裸 `PyLong_AsLongLong` 替代 extract_scalar_short_circuit 的多类型探测。

```rust
// 编译期已知类型为 int 时
fn extract_int_raw(obj: &Bound<PyAny>) -> Result<Value> {
    // 直接 PyLong_AsLongLong，无类型探测链
    let val = unsafe { pyffi::PyLong_AsLongLong(obj.as_ptr()) };
    if val == -1 && unsafe { !pyffi::PyErr_Occurred().is_null() } {
        return Err(type_mismatch);
    }
    Ok(Value::Int(val))
}
```

**预期收益**：每 int 字段减少 ~80ns。array build 100 元素减少 ~8µs。

### 5.4 P2：减少 Container 创建开销（对应 RC6）

**方案**：定位 Container/ListContainer 创建源（需 Rust 侧 instrumentation），评估是否可避免。

由于 Container 创建仅占 array build 的 ~2%，此项优先级最低。

### 5.5 预期效果汇总

| 优化 | simple | nested | array(100) |
|------|--------|--------|-----------|
| 当前 | 3.7µs (0.8x) | 9.4µs (0.6x) | 88µs (0.3x) |
| +P0 (5.1 list占位) | 3.7µs (0.8x) | 9.4µs (0.6x) | ~70µs (0.4x) |
| +P0 (5.2 单次提取) | ~2.8µs (1.1x) | ~6.5µs (0.9x) | ~65µs (0.4x) |
| +P1 (5.3 int直取) | ~2.5µs (1.2x) | ~5.5µs (1.1x) | ~55µs (0.5x) |

**结论**：即使实施全部 P0+P1 优化，array build 仍不达标（目标 ~30µs / ≥1.0x，优化后 ~55µs / 0.5x）。

### 5.6 架构级建议：array build 需要更深层的批量化

array build 的根本问题是：即使消除了 shallow list 遍历和双重提取，**每元素仍有 ~500ns 的 per-element 开销**（batch extract PyLong_AsLongLong + PyScalarInput + FormatField exec_build_py + inner.build）。

要达到 ≥1.0x（~30µs），需要 per-element 成本 ≤ ~180ns（(30µs - 12µs固定) / 100）。

**方案**：schema-guided 批量编码。编译树已知 Array 的元素类型是 Int32ub。对于同构 int 数组，**跳过 per-element exec_build_py 调用**，直接用裸 CPython API 提取整个 Vec<i64>，然后用 Rust 的 byteorder 批量编码为 bytes：

```rust
impl CompiledArray {
    fn exec_build_py(&self, py, input, stream, ctx) -> Result<()> {
        if let Some(ints) = input.extract_batch_int_py(py)? {
            // 批量编码：100 个 i64 → 400 bytes，纯 Rust，~1µs
            let mut buf = Vec::with_capacity(ints.len() * 4);
            for v in &ints {
                buf.extend_from_slice(&v.to_be_bytes()[..4]);
            }
            stream.write_bytes(&buf)?;
            return Ok(());
        }
        // fallback: per-element
    }
}
```

**预期**：array build 从 ~55µs（P0+P1 后）降至 ~15-20µs（batch encode ~1µs + 固定开销 ~12µs + batch extract ~5µs）。达到 ~1.5-2.0x。

**注意**：这要求编译树在编译期识别"Array 的 subcon 是定宽 FormatField"，并生成批量编码路径。这是 SchemaCompiler 的增强，属于 Phase 17 候选。

---

## §6 设计文档修正建议

基于本次分析，`模块设计-Python-first产出层.md` 应做以下修正：

| 章节 | 问题 | 修正建议 |
|------|------|---------|
| §4.4 CompiledStruct::exec_build_py 代码草图 | 缺少 extract_ctx_value_py 调用（实际实现有） | 补充实际实现的完整代码，或明确标注"代码草图仅示意，实际实现见 py_exec.rs" |
| §4.6 shallow_py_to_value_for_ctx list 分支 | 声明"不递归元素"，实现递归 | 修正设计为"不遍历元素"并在实现中对齐 |
| §6.3 P1 simple build FFI 计数 | 漏算 extract_ctx_value_py 的 ~5 FFI/字段 | 每字段从 4 FFI 修正为 8-12 FFI |
| §6.3 P4 array build per-element | 漏算 shallow list 遍历的 N×5 FFI | per-element 从 ~1 FFI 修正为 ~7 FFI |
| §6.1 固定开销 | 假设 ~200ns，实际 ~1.2µs | 修正固定开销估算 |
| 新增 §4.7 | 需补充 CompiledStruct::exec_build_py 的 ctx value 提取策略 | 明确：仅对被表达式引用的字段提取 ctx value |

---

## §7 下一步建议

1. **立即（Phase 16.5 范围内）**：实施 P0 优化（§5.1 + §5.2 方案 B），预计 simple build 达标（≥1.0x），nested 接近达标。
2. **Phase 16.5 验收**：重新运行 `test_perf_comparison.py`，确认 simple/nested build ≥1.0x。
3. **Phase 17（如 array build 仍不达标）**：实施 §5.6 的 schema-guided 批量编码，或将 array build 的 ≥1.0x 目标推迟到 Phase 17。
4. **设计文档修正**：按 §6 修正设计文档，确保后续阶段的成本模型基于实测数据而非假设。

---

## 附录 A：测试脚本清单

| 脚本 | 用途 |
|------|------|
| `construct-py/tests/test_build_profile.py` | cProfile + baseline 计时 |
| `construct-py/tests/test_build_microbench.py` | 格式对比 + 缩放分析 |
| `construct-py/tests/test_build_ffi_cost.py` | 固定开销 + 边际成本 + 类型对比 |

## 附录 B：原始数据

### B.1 cProfile（array_heavy, 2000 iters）

```
ncalls  tottime  percall  cumtime  filename
     1    0.000              0.183   _run
  2000    0.180    0.000    0.183   build_from_py (C extension)
  8000    0.003    0.000    0.003   containers.py:118(Container.__init__)
```

### B.2 Array 缩放完整数据

```
elems      time    per-elem    overhead
    0   12.27µs   12.27µs     12.27µs
    1   13.88µs    1.61µs      0.0ns
    2   14.52µs    0.64µs      0.0ns
    5   17.05µs    0.85µs      0.0ns
   10   21.31µs    0.85µs      0.0ns
   20   28.95µs    0.76µs      0.0ns
   50   51.44µs    0.75µs      0.0ns
  100   88.24µs    0.74µs      0.0ns
  200  161.81µs    0.74µs      0.0ns
```

### B.3 有效 FFI 成本推导

```
每字段边际: 440ns
每字段 FFI: 8-12 次（代码审查）
推导 FFI 成本: 37-55ns/次（与设计 ~40ns 一致）

每元素边际: 736ns
每元素 FFI: 5-7 次（shallow遍历 + batch extract + build）
推导 FFI 成本: 105-147ns/次
（高于标量路径，因 pyo3 list 迭代 + extract 抽象开销）
```
