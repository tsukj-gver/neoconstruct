# 模块设计：Build 方向优化（Phase 16.5）

> **阶段**：Phase 16.5 — Build 方向性能修复（基于 Phase 16.4 profiling 实测数据）
> **目标**：simple build ≥1.0x（当前 0.74x）；nested build 改善（当前 0.61x → 预期 ~0.74x，**≥1.0x 推迟 Phase 17**）；评估 array build 是否可在 16.5 达标
> **前置**：Phase 16.4 已完成（71 变体 exec_build_py 全量实现 + VET 审查通过），但性能未达标
> **依据**：`docs/refactor/Phase16-build性能分析.md`（profiling 报告，§2-§5）
> **定位**：本文档是对 Phase 16 主设计（`模块设计-Python-first产出层.md`）的**增量修正**，仅修改 §4.4 / §4.6 的实现细节，不改变 PySink / PyInput / PyCompiledExec 的核心架构

> **⚠️ REV 修正说明（2026-06-20）**
>
> 本文档经 REV 设计检视后修正了 3 项必修问题（P-MUST-1/2/3）：
>
> | 编号 | 问题 | 修正内容 |
> |------|------|---------|
> | **P-MUST-1** | nested build 的 Python 基线错误（原标 ~3.2µs，实测 ~5.8µs）。导致汇总表"7.8µs = 1.05x 达标"是错误计算（实际 5.8/7.8 = 0.74x）。**nested build ≥1.0x 在 16.5 不可达标**，已诚实修正并推迟至 Phase 17 | §1.1 / §1.2 / §3.1 / §3.2 / §5 / §6 全部 nested build 相关预测重算 |
> | **P-MUST-2** | 方案 B 伪代码 `!field.flagbuildnone` 在 flagbuildnone 字段**有值**时错误返回 Value::None | §2.2 伪代码条件逻辑修正 |
> | **P-MUST-3** | §4.3 `std::mem::forget(obj)` 导致 PyList_Append 后 refcount 永久多 1（内存泄漏） | §4.3 删除 forget，让 obj 正常 drop |

---

## §1 优化目标（基于 profiling 实测，可证伪）

### 1.1 实测基线（profiling §1-§2）

| 格式 | 当前实际 | Python 原版基线 | 当前比率 | 设计预测 | 偏差倍数 |
|------|---------|---------------|---------|---------|---------|
| simple build (3 字段) | ~3.7µs | ~2.8µs | **0.74-0.82x** | ~5.4x | **6.5x** |
| nested build (2 级) | ~9.4µs | ~5.8µs | **0.61x** | ~3.1x | **5.1x** |
| array build (100 元素) | ~88µs | ~30µs | **0.31x** | ~5.4x | **17x** |

> **[REV P-MUST-1 修正]** nested build 的 Python 基线原标 ~3.2µs 是错误的。实测数据（`test_perf_comparison.py --iterations 5000`）：
> - Python nested build: 0.0290s / 5000 = **5.8µs/次**
> - Rust nested build: 0.0476s / 5000 = **9.52µs/次**
> - 实际比率: 5.8 / 9.52 = **0.61x**（比率本身正确，但 Python 基线应为 5.8µs 而非 3.2µs）
>
> 原设计假设 Python 原版对 2 级嵌套 Struct 的 build 只需 ~3.2µs，但实际 Python 原版的 Container 创建 + 嵌套字段遍历开销使其为 ~5.8µs。**这意味着 Rust 实现要达到 ≥1.0x 需 ≤5.8µs**（当前 9.4µs，需节省 ~3.6µs），详见 §3.1 nested build 分析。

**实测成本常量**（profiling §2.4 推导）：

| 项 | 实测值 | 设计假设 | 偏差 |
|----|-------|---------|------|
| 入口固定开销 | **1.18µs** | ~200ns | 5.9x |
| 每标量字段边际 | **~440ns** | ~140ns | 3.1x |
| 每 array 元素边际 | **~736ns** | ~35ns | 21x |
| 单次 FFI 成本 | **37-55ns** | ~40ns | ✅ 一致 |

**关键洞察**：FFI 单次成本预测**正确**，但 FFI 调用次数被严重低估（实际 8-12 次/字段 vs 预测 4 次/字段；array 实际 5-7 次/元素 vs 预测 ~1 次/元素）。

### 1.2 Phase 16.5 量化目标（可证伪）

| 格式 | Phase 16.5 目标 | 触发条件 | 验证方法 |
|------|----------------|---------|---------|
| **simple build** | **≥1.0x（≤2.8µs）** | 修复 RC1+RC2+RC3（§2.1+§2.2+§2.3） | `test_perf_comparison.py --format simple`，5000 iters |
| **nested build** | **改善至 ~0.74x（≤~7.9µs）；≥1.0x 推迟 Phase 17** [REV P-MUST-1 修正] | RC1+RC2+RC3+RC5 节省 ~1.5µs（9.4→~7.9µs），但 Python 基线实测 5.8µs（非原假设 3.2µs），≥1.0x 需 Rust ≤5.8µs，16.5 范围内不可达 | `test_perf_comparison.py --format nested`，5000 iters |
| **array build** | **≥1.0x（≤30µs）** 或 推迟 Phase 17 | 修复 RC1+RC2+RC3+RC5（§2.1-§2.4）；若不达标则推迟 | `test_perf_comparison.py --format array_heavy`，5000 iters |
| **回归** | 1515 纯 Rust 测试全 PASS + 61 dataclass 测试全 PASS | 每个子任务 | `cargo test` + `pytest tests/test_dataclass_api.py` |

**反例声明（验证策略）**：

| 若实测... | 说明的问题 | 应对方案 |
|-----------|-----------|---------|
| simple build < 1.0x | 固定开销 1.18µs 已占 42%，剩余 1.6µs 预算非常紧 | 进一步分析 ctx.insert / sub_field_py 的非 FFI 开销；考虑省略匿名字段 ctx_value 提取 |
| nested build < 1.0x（**预期，非例外**）[REV P-MUST-1 修正] | Python 基线实测 5.8µs（非原假设 3.2µs）；RC1-RC5 仅节省 ~1.5µs（9.4→~7.9µs），比率 ~0.73x | nested build ≥1.0x 明确推迟 Phase 17（schema-guided 类型已知 extract + 入口开销优化）；16.5 仅验收改善幅度（0.61x → ~0.74x） |
| array build < 1.0x | per-element exec_build_py 的 vtable + 分配开销主导 | 推迟 array build ≥1.0x 至 Phase 17（schema-guided 批量编码） |
| 任一场景出现功能回归（dataclass 测试失败） | RC1 list 占位符破坏 `this._.items[0]` 等深层引用 | 评估 dataclass 测试覆盖；必要时 RC1 改为"标量列表浅提取，复合元素存占位符" |

### 1.3 不在 Phase 16.5 范围

- **parse 方向 array 优化**：parse 不达标（array parse 0.55x）的瓶颈与 build 不同（详见 §4），属 Phase 17 范围
- **schema-guided 批量编码**（profiling §5.6 的 Phase 17 候选）：需要 SchemaCompiler 增强，风险较高
- **入口固定开销优化（RC4）**：1.18µs 的固定开销主要来自 pyo3 入口分发，非纯代码层可优化（需要 pyo3 内部改造或绕过 `#[pymethod]` 分发）
- **Container/ListContainer 创建优化（RC6）**：仅占 array build 的 ~2%，优先级最低

---

## §2 优化方案（基于 RC1-RC3 实测根因）

每个方案严格遵循：根因编号 → 代码位置 → 修改内容 → 量化预测 → 风险评估。

### 2.1 RC1 修复：shallow_py_to_value_for_ctx 的 list 分支改为空占位符

**根因引用**：profiling §3 RC1（致命，array 主导）

**问题位置**：`construct-rs/src/compiled/py_input.rs:285-296`

```rust
// 当前实现（错误：遍历全部元素）
if let Ok(list) = obj.downcast::<PyList>() {
    let mut items = Vec::with_capacity(list.len());
    for item in list {                                    // ← 遍历 N 个元素
        if let Some(scalar) = extract_scalar_short_circuit(&item) {
            items.push(scalar);                           // ← 每元素 3-6 FFI
        } else {
            items.push(Value::None);
        }
    }
    return Ok(Value::List(items));
}
```

**设计 §4.6 原意**（`模块设计-Python-first产出层.md:999-1001`）：

```rust
} else if let Ok(list) = obj.downcast::<PyList>() {
    // root 是 list：存空占位符（不递归元素）
    Value::List(Vec::new())
}
```

设计明确声明"root 是 list：存空占位符（不递归元素）"，但实现者在 `shallow_py_to_value_for_ctx` 中将此规则放宽为"提取所有标量元素"。实测 int-array vs bytes-array 每元素差 ~875-1238ns（profiling §2.3.4）直接证明此遍历存在。

**修改方案**：

```rust
// 修改后（对齐设计 §4.6 原意）
if let Ok(_list) = obj.downcast::<PyList>() {
    // 不遍历元素 —— ctx 表达式 rarely 引用 list 元素。
    // 设计 §4.6 已知限制：`this._.list_field[0]` 返回空占位。
    return Ok(Value::List(Vec::new()));
}
```

**同步修改**：`py_input.rs:264-308` 的 `shallow_py_to_value_for_ctx` 函数文档注释需要更新，明确"list 分支返回空 List 占位符（不遍历元素）"。

**量化预测**（基于 profiling §2.3.3 + §4.1）：

| 场景 | RC1 浪费 | 修复后节省 | 修复后总耗时 |
|------|---------|----------|------------|
| array build (100 元素) | 100 × ~5 FFI × ~40ns ≈ **20µs** | ~20µs | ~88µs → **~68µs** |
| simple build | 0（无 list 字段） | 0 | 不变 |
| nested build | 0-1 个 list 嵌套字段（基准格式无） | ~0-5µs（视格式而定） | 不变 |

**风险**：

1. **`this._.items[0]` 等深层引用失效**：返回空 list 导致下标访问失败。需评估 61 个 dataclass 测试是否有此模式。
   - 缓解：先运行全量 dataclass 测试，若有失败则回滚此修改并采用折中方案（见下方"折中方案"）
   - 折中方案：仅对元素全部为标量的 list 做浅提取，对包含复合元素的 list 存空占位符（保留对纯标量列表的支持，避免对混合列表的全量遍历）

2. **CompiledArray::exec_build_py 已有 batch extract 路径**：实际 array build 走 `extract_batch_int_py`（裸 CPython API），不走 shallow list 路径。RC1 的浪费发生在 `CompiledStruct::exec_build_py` 对 `items` 字段调 `extract_ctx_value_py` 时，**只为 ctx 插入而遍历**（实际 ctx 从未读取这个 list）。
   - 验证：profiling §2.3.4 已通过 int-array vs bytes-array 差异证明此遍历确实发生

**兼容性**：
- 公共 API 不变（`PyInput::extract_ctx_value_py` 签名不变）
- 行为变化：`this._.list_field[0]` / `this._.list_field.length` 等表达式失效（设计 §4.6 已声明的已知限制）
- 测试影响：需运行 61 个 dataclass 测试 + 1515 纯 Rust 测试确认无回归

**优先级**：🔴 **P0（必须）** — 对 array build 贡献 ~20µs 节省（~23% 改善）

---

### 2.2 RC2 修复：CompiledStruct::exec_build_py 消除 per-field extract_ctx_value_py 冗余

**根因引用**：profiling §3 RC2（致命，全格式）

**问题位置**：`construct-rs/src/compiled/py_exec.rs:1207-1247`（CompiledStruct::exec_build_py）

```rust
// 当前实现（每字段提取 ctx value）
for field in &self.fields {
    let field_input: Box<dyn PyInput> = match &field.name {
        Some(name) => { /* sub_field_py */ }
        None => Box::new(PyScalarInput::new(Value::None)),
    };
    let ctx_value = field_input.extract_ctx_value_py(py)?;  // ← 设计草图没有
    if let Some(name) = &field.name {
        child_ctx.insert(name.clone(), ctx_value);            // ← 设计草图有
    }
    field.subcon.exec_build_py(py, &*field_input, ...)?;     // ← 子节点会再次 extract
}
```

**设计 §4.4 代码草图**（`模块设计-Python-first产出层.md:756-803`）：草图先提取标量一次（`sub.extract_scalar_py()?`），存入 ctx，然后传递 `field_input`（仍是 sub）给子节点。但子节点的 leaf `exec_build_py` 又会调 `extract_scalar_py`（py_exec.rs:204），导致**第二次提取**——这就是 RC3。

**根本矛盾**：设计草图暗示"Struct 提取一次，子节点复用"，但子节点接收的是 `&dyn PyInput`（仍指向 Python 对象），无法复用已提取的 Value。这是设计文档未约束实现细节导致的偏差（profiling §4.2 根本原因 1）。

**修改方案 B**（推荐，彻底消除 RC2+RC3）：Struct 先提取标量一次，用 `PyScalarInput` 包装传递给子节点：

```rust
// 修改后（方案 B：单次提取 + PyScalarInput 传递）
for field in &self.fields {
    // 1. 获取 field_input（PyDictInput 包装，指向 Python 字段对象）
    let field_input: Box<dyn PyInput> = match &field.name {
        Some(name) => {
            if field.flagbuildnone && !input.has_field_py(py, name) {
                Box::new(PyScalarInput::new(Value::None))
            } else {
                input.sub_field_py(py, name)?
            }
        }
        None => Box::new(PyScalarInput::new(Value::None)),
    };

    // 2. 仅在字段被命名时提取 ctx value（供 this.field 表达式）
    //    提取一次，同时用于 ctx 和子节点 build。
    // [REV P-MUST-2 修正] flagbuildnone 字段仅在"字段不存在"时用 Value::None。
    //    原伪代码 `!field.flagbuildnone` 会在 flagbuildnone 字段**有值**时
    //    错误地返回 Value::None，破坏 Optional 类构造器 build。
    let build_value = if let Some(name) = &field.name {
        if field.flagbuildnone && !input.has_field_py(py, name) {
            Value::None  // 字段可选且不存在 → None
        } else {
            field_input.extract_ctx_value_py(py)?  // 字段存在 → 正常提取
        }
    } else {
        // 匿名字段：不提取
        Value::None
    };

    // 3. ctx 插入（供后续字段的 this.prev_field 表达式）
    if let Some(name) = &field.name {
        child_ctx.insert(name.clone(), build_value.clone());
    }

    // 4. 决定传给子节点的 input：
    //    - 标量字段（leaf）：用 PyScalarInput 包装已提取的 Value，子节点不再 FFI
    //    - 复合字段（嵌套 Struct/Sequence/Array）：保留原 field_input（子节点需自行递归）
    let child_input: Box<dyn PyInput> = if is_scalar_value(&build_value) {
        Box::new(PyScalarInput::new(build_value))
    } else {
        field_input  // 复合字段：传原 input
    };

    field.subcon.exec_build_py(py, &*child_input, stream, &mut child_ctx)?;
}
```

**关键决策点**：如何判断"标量字段 vs 复合字段"？

- **方案 B-1（运行时判断）**：通过 `extract_ctx_value_py` 返回的 Value 类型判断。若 `Value::Int/UInt/Float/Bool/Bytes/String/None` → 标量；若 `Value::Container/List` → 复合。
- **方案 B-2（编译期判断，更优）**：在 SchemaCompiler 中为每个字段标注 `is_leaf: bool`（基于 `field.subcon` 是否为 leaf 编译节点）。CompiledStruct 在编译期已知哪些字段是 leaf。**推荐 B-2**，但需要修改 `CompiledStructField` 结构（+1 字节），属编译树层的小改动。

**折中实现（推荐）**：在 Phase 16.5 采用 B-1（运行时判断），不修改编译树结构；若性能仍不够，Phase 17 改为 B-2。

**辅助函数**：

```rust
/// 判断 Value 是否为标量（用于 Struct exec_build_py 决定是否包装为 PyScalarInput）。
fn is_scalar_value(v: &Value) -> bool {
    matches!(
        v,
        Value::None | Value::Bool(_) | Value::Int(_) | Value::UInt(_)
            | Value::BigInt(_) | Value::Float(_) | Value::Bytes(_) | Value::String(_)
    )
}
```

**量化预测**（基于 profiling §2.4 + §4.1，FFI 成本 ~40-55ns/次）：

每标量字段 FFI 变化：

| 步骤 | 当前（RC2+RC3） | 方案 B 后 |
|------|---------------|---------|
| has_field_py (PyDict_Contains) | 1 FFI | 1 FFI |
| sub_field_py (GetItem + clone.unbind) | 2 FFI | 2 FFI |
| extract_ctx_value_py → extract_scalar_short_circuit | 3-5 FFI（含 RC5 ~2 FFI） | 3-5 FFI（同） |
| **leaf exec_build_py 的 extract_scalar_py** | **3-5 FFI（重复！）** | **0 FFI（PyScalarInput 直接 clone Value）** |
| ctx.insert | 0 | 0 |
| inner.build | 0 | 0 |
| **合计/字段** | **9-13 FFI** | **6-8 FFI** |

每字段节省 ~3-5 FFI × ~40ns = **120-200ns**。

| 格式 | 字段数 | 节省 | 当前耗时 | 方案 B 后 |
|------|-------|------|---------|----------|
| simple build | 3 | ~450-600ns | ~3.7µs | **~3.1-3.25µs**（仍 > Python 2.8µs） |
| nested build | 6（含嵌套） | ~900-1200ns | ~9.4µs | **~8.3-8.5µs**（远 > Python 5.8µs [REV P-MUST-1 修正]） |
| array build | 2（count 标量 + items） | count 节省 ~150ns；items 是复合字段不节省 | ~88µs → ~68µs（RC1 后） | **~67.8µs** |

**结论**：仅修 RC2+RC3 不足以让 simple/nested build 达标（与 profiling §5.5 一致）。**必须同时修 RC1（对 array）和考虑 RC5（对 simple/nested）**。即使修 RC1+RC2+RC3+RC5，nested build 仍不达标（见 §3.1 [REV P-MUST-1 修正]），但 simple build 可达标。

**风险**：

1. **复合字段判断错误**：若 `extract_ctx_value_py` 对复合字段返回了标量 Value（如 `Adapter(Struct)` 编码后是标量），会错误包装为 PyScalarInput，导致子节点 `sub_field_py` 失败。
   - 缓解：通过 61 个 dataclass 测试覆盖；Adapter/Validator 的 `exec_build_py` 已自行调 `extract_ctx_value_py`（py_exec.rs:319），不依赖父 Struct 传 PyScalarInput
   - **关键不变量** [REV P-IMP-1 修正]：wrapper 节点的 `exec_build_py` 有两种模式，方案 B 对两者都安全：
     - **模式 A（构造 PyScalarInput）**：Adapter/ExprAdapter/SymmetricAdapter/Const/Enum/FlagsEnum/Mapping 等先调 `extract_ctx_value_py`，然后构造 `PyScalarInput` 传给 inner（见 py_exec.rs:319, 440, 481, 535, 601, 818, 897）。方案 B 传 PyScalarInput 时，wrapper 的 `extract_ctx_value_py` 返回 clone 的值——行为正确。
     - **模式 B（传原 input）**：Validator/ExprValidator 先调 `extract_ctx_value_py` 做 check，然后**将原 input 传给 inner**（py_exec.rs:389-399），不构造 PyScalarInput。方案 B 对标量字段传 PyScalarInput 时，Validator 的 `extract_ctx_value_py` 返回 clone 的标量值，check 通过后传给 inner leaf——行为正确（因 Validator 字段的输入值本身就是标量）。
     - **结论**：无论 wrapper 是构造 PyScalarInput 还是传原 input，方案 B 都安全。

2. **PyScalarInput 的 extract_ctx_value_py 行为**：`PyScalarInput::extract_ctx_value_py`（py_input.rs:548-550）返回 `self.value.clone()`。包装标量后，子节点若再调 `extract_ctx_value_py` 不会出错，只是冗余。可接受。

3. **匿名字段路径**：当前实现匿名字段用 `PyScalarInput(Value::None)`，方案 B 保持此行为（匿名字段不提取，不影响正确性）。

**兼容性**：
- 公共 API 不变
- 行为变化：仅性能优化，输出字节序列完全一致
- 测试影响：无（仅优化，不改语义）

**优先级**：🔴 **P0（必须）** — 全格式受益，每字段节省 120-200ns

---

### 2.3 RC3 修复：标量单次提取（与 §2.2 合并实施）

**根因引用**：profiling §3 RC3（重要，全格式）

**问题位置**：与 §2.2 相同（py_exec.rs:1226 + py_exec.rs:204）

**说明**：RC3 是 RC2 的直接后果——`extract_ctx_value_py` 提取一次，leaf `exec_build_py` 的 `extract_scalar_py` 再提取一次。§2.2 的方案 B（用 PyScalarInput 包装）同时消除 RC2 和 RC3，无需独立方案。

**量化预测**：见 §2.2 表格（已包含 RC3 的消除效果）。

**优先级**：🔴 **P0（与 §2.2 合并实施）**

---

### 2.4 RC5 修复（可选）：bool-before-int 探测开销

**根因引用**：profiling §3 RC5（次要）

**问题位置**：`construct-rs/src/compiled/py_input.rs:220-227`

```rust
// 当前实现（bool 探测在 int 之前）
fn extract_scalar_short_circuit(obj: &Bound<'_, PyAny>) -> Option<Value> {
    if obj.is_none() { return Some(Value::None); }
    // bool before int — bool is a subclass of int in Python.
    if let Ok(b) = obj.extract::<bool>() {            // ← int 会触发
        if obj.is_instance_of::<PyBool>() {           // ← int 会失败
            return Some(Value::Bool(b));
        }
    }
    if let Ok(i) = obj.extract::<i64>() { ... }
    // ...
}
```

**问题**：对每个 int 值，`extract::<bool>()` 可能成功（truthy int 返回 true），然后 `is_instance_of::<PyBool>()` 失败。浪费 ~2 FFI/int ≈ 80ns/int。

**修改方案**：直接用 `is_instance_of::<PyBool>()` 作为 bool 判定（绕过 `extract::<bool>()` 的内部转换）：

```rust
fn extract_scalar_short_circuit(obj: &Bound<'_, PyAny>) -> Option<Value> {
    if obj.is_none() { return Some(Value::None); }
    // 优化：直接检查类型，避免 extract::<bool> 对 int 的额外开销。
    // PyBool 是 PyLong 的子类，is_instance_of::<PyBool> 对纯 int 返回 false。
    if obj.is_instance_of::<PyBool>() {
        if let Ok(b) = obj.extract::<bool>() {
            return Some(Value::Bool(b));
        }
    }
    if let Ok(i) = obj.extract::<i64>() {
        return Some(Value::Int(i));
    }
    // ... 其余探测不变
}
```

**量化预测**：

| 格式 | int 字段数 | 节省/int | 总节省 |
|------|----------|---------|-------|
| simple build | 3 | ~80ns | ~240ns |
| nested build | 4-6 | ~80ns | ~320-480ns |
| array build | 1（count） + 100（items，但走 batch） | ~80ns/count | ~80ns（items 走 batch 不经此路径） |

**RC5 对 array build 的影响有限**：因为 array build 的 100 元素走 `extract_batch_int_py`（裸 CPython API，不经过 `extract_scalar_short_circuit`），RC5 仅对 count 字段生效。

**更激进方案（Phase 17 候选）**：schema-guided 类型已知 extract。编译树已知 leaf 类型（如 FormatField with int format），可生成专用的 `extract_int_raw`（直接 `PyLong_AsLongLong`，无类型探测）。但需要修改 SchemaCompiler，风险较高，**不在 16.5 范围**。

**风险**：

1. **PyBool 子类处理**：Python 中 `True`/`False` 是 `bool`（PyBool 的实例），`bool` 是 `int` 的子类。`is_instance_of::<PyBool>` 对 `True`/`False` 返回 true，对 `1`/`0` 返回 false。语义正确。
2. **非 bool 对象的 `is_instance_of` 失败**：pyo3 的 `is_instance_of` 内部用 `PyObject_TypeCheck` + `PyType_IsSubtype`，对非类型对象会返回 false（不会 panic）。

**兼容性**：行为完全一致（True/False 仍提取为 Bool，int 仍提取为 Int），仅性能优化。

**优先级**：🟡 **P1（可选）** — simple/nested 受益 ~240-480ns，是 simple build 达标的关键余量

---

### 2.5 16.5 范围 vs Phase 17 范围

| 优化 | 16.5 范围 | 原因 |
|------|----------|------|
| RC1（list 占位符） | ✅ 是 | 改动 < 10 行，无接口变更 |
| RC2+RC3（Struct 单次提取） | ✅ 是 | 改动 ~30 行，影响 CompiledStruct 一个变体 |
| RC5（bool 探测优化） | ✅ 是（可选） | 改动 < 10 行，纯优化 |
| RC4（入口固定开销） | ❌ 否 | 1.18µs 主要来自 pyo3 `#[pymethod]` 分发，需绕过 pyo3 入口层 |
| RC6（Container 创建） | ❌ 否 | 仅占 array ~2%，优先级低 |
| Schema-guided 批量编码 | ❌ 否 | 需要 SchemaCompiler 增强，风险高 |
| Schema-guided 类型已知 extract | ❌ 否 | 同上 |

**16.5 不实施 RC4/RC6/schema-guided 的论证**：

- RC4 的 1.18µs 是 simple build 目标 2.8µs 的 **42%**。即使完全消除 RC1+RC2+RC3+RC5（节省 ~600-900ns），simple build 仍约为 1.18 + 3 × ~150ns（优化后单字段成本）= 1.18 + 0.45 = **~1.63µs** → ~1.72x ✓ 达标。**16.5 不需修 RC4 也能让 simple build 达标**。
- RC6 的 ~1.9µs 仅占 array build 88µs 的 2%，对达标无影响。
- **[REV P-MUST-1 修正]** 上述论证仅适用于 simple build。对 nested build，即使修 RC1+RC2+RC3+RC5（节省 ~1.5µs），优化后 ~7.9µs 仍 > Python 实测基线 5.8µs（比率 ~0.73x）。**nested build ≥1.0x 需要 Phase 17 的 RC4 + schema-guided 类型已知 extract**（详见 §3.1）。

---

## §3 性能预测（基于实测 FFI 成本 ~40-55ns/次）

> **重要**：本节预测严格基于 profiling §2.4 推导的 37-55ns/FFI 成本，不再使用设计 §6.3 的 ~40ns 假设（因设计 §6.3 漏算 extract_ctx_value_py 的 5 FFI/字段，导致原预测失效）。

### 3.1 修复后的 FFI 调用次数推导

#### Simple build（3 int 字段）

**入口固定开销**（profiling §2.3.1）：1.18µs ≈ ~22-30 FFI（按 ~40-55ns/FFI 推算）

**每字段 FFI**（修复 RC2+RC3+RC5 后）：

| 步骤 | FFI 次数 |
|------|---------|
| has_field_py (PyDict_Contains) | 1 |
| sub_field_py (PyDict_GetItem + clone().unbind()) | 2 |
| extract_ctx_value_py → extract_scalar_short_circuit（RC5 优化后） | 2（PyBool 检查 + i64 extract） |
| leaf exec_build_py：PyScalarInput 直接 clone Value | 0（消除 RC3） |
| ctx.insert | 0（Rust） |
| inner.build (byteorder) | 0（Rust） |
| **小计/字段** | **5 FFI** |

3 字段 = 15 FFI

**输出**：1 PyBytes::new ≈ 1 FFI

**总 FFI**：~22-30（入口） + 15（字段） + 1（输出） = **~38-46 FFI**

**预期耗时**：38-46 FFI × ~40-55ns + Rust 内部分配开销 ≈ **1.52-2.53µs + ~200ns Rust = ~1.7-2.7µs**

**对比 Python 2.8µs → ~1.0-1.65x** ✅ **达标**（取中位数 ~1.3x）

#### Nested build（2 级，4-6 标量字段 + 1 嵌套 dict 访问）

基准格式：`Struct("header"/Struct("magic"/Bytes(4), "version"/Int16ub), "payload"/Struct("size"/Int32ub, "flags"/Int32ub))`

> **[REV P-MUST-1 修正]** 本节原使用 FFI 推導法预测 nested build ~2.2-3.3µs → "1.0-1.5x 达标"。
> 但该推導方法在 Phase 16 已被证明**低估 5.1 倍**（预测 3.1x，实际 0.61x），因为：
> (1) FFI 调用次数被低估（实际 8-12 次/字段 vs 推導 4 次/字段）；
> (2) 非 FFI 的 Rust 内部开销（Box 分配 / clone / IndexMap insert / ContainerClasses::clone 的 with_gil）未被计入。
> **因此，FFI 推導法在本文档中仅作参考，不作为达标判断依据。以下以减法估算（基于实测边际成本）为准。**

##### 减法估算（可靠，基于实测边际成本）

| 项 | 实测值 | 来源 |
|----|-------|------|
| 当前 nested build 总耗时 | **~9.4µs**（实测 9.52µs） | profiling §1 + PM 性能测试 |
| RC2+RC3 节省（6 字段 × 120-200ns） | ~0.9-1.2µs | §2.2 量化预测表 |
| RC5 节省（4-6 int 字段 × ~80ns） | ~0.32-0.48µs | §2.4 量化预测表 |
| **总节省** | **~1.2-1.7µs** | — |
| **优化后预测** | **~7.7-8.2µs** | 9.4µs - ~1.5µs |

##### 优化后比率计算

| Python 基线 | 优化后 Rust 耗时 | 比率 | 是否达标 |
|------------|----------------|------|---------|
| ~~3.2µs~~（原假设，错误） | ~7.9µs | ~~3.2/7.9 = 0.41x~~ | ❌ |
| **5.8µs**（实测，正确） | **~7.9µs** | **5.8/7.9 = 0.73x** | ❌ |

**结论：nested build ≥1.0x 在 Phase 16.5 不可达标。**

##### 为什么不可达标

要达到 ≥1.0x，Rust nested build 需 ≤ 5.8µs（Python 实测基线）。当前 9.4µs，需节省 **~3.6µs**。

| 优化项 | 可节省 | 16.5 是否实施 |
|--------|-------|-------------|
| RC2+RC3（单次提取） | ~1.2µs | ✅ 是 |
| RC5（bool 探测优化） | ~0.4µs | ✅ 是 |
| RC4（入口固定开销 1.18µs） | 最多 ~1.0µs（即使完全消除） | ❌ 否（需绕过 pyo3 入口层） |
| schema-guided 类型已知 extract | ~0.5-1.0µs（预估） | ❌ 否（Phase 17） |
| **16.5 范围内总计** | **~1.6µs** | — |
| **所需节省** | **~3.6µs** | — |

即使将 RC4（Phase 17）也纳入，总计 ~2.6µs 节省，优化后 ~6.8µs，比率 5.8/6.8 = 0.85x——**仍不达标**。根本原因是 nested build 的 Python 原版本身就有 ~5.8µs 的基线开销（Container 创建 + 嵌套遍历），Rust 的 FFI 间接开销（每字段 6-8 FFI）使得即使优化后仍高于此基线。

##### nested build ≥1.0x 推迟至 Phase 17

nested build 要达到 ≥1.0x（Rust ≤5.8µs），需要以下 Phase 17 优化：
1. **RC4（入口固定开销优化）**：绕过 pyo3 `#[pymethod]` 分发，预计节省 ~0.6-1.0µs
2. **schema-guided 类型已知 extract**：编译树已知 leaf 类型，生成专用 `extract_int_raw`（直接 `PyLong_AsLongLong`），消除 `extract_scalar_short_circuit` 的类型探测开销，预计节省 ~0.3-0.5µs/字段
3. **ContainerClasses::clone 优化**（VET 16.2 建议改进 2）：用 `clone_ref(py)` 替代 `with_gil`，预计节省 ~5-10ns/字段

**16.5 验收标准修正**：nested build 验收标准从"≥1.0x"改为"**改善幅度**"（当前 0.61x → 目标 ~0.74x，即从 9.4µs 降至 ~7.9µs）。≥1.0x 列为 Phase 17 目标。

> **对原 FFI 推導的保留（仅供参考，不作达标判断依据）**：
> 原推導给出 ~45-53 FFI × ~40-55ns + ~400ns Rust ≈ 2.2-3.3µs。
> 但 Phase 16 实测证明此方法将 nested build 高估为 3.1x（实际 0.61x），偏差 5.1x。
> 偏差来源：FFI 次数低估 + 未计入的 Rust 开销（Box<dyn> 分配、IndexMap insert、ContainerClasses clone 等）。
> **此推導已被废弃，仅供 Phase 17 成本模型参考。**

#### Array build（100 元素 int array）

基准格式：`Struct{ count: Int, items: Array(100, Int32ub) }`

**入口固定开销**：1.18µs

**count 字段**（标量）：5 FFI（修复后，见 simple build 推导）

**items 字段**（Array，复合字段）：
- has_field_py + sub_field_py (3 FFI)
- extract_ctx_value_py（RC1 修复后）：list 分支直接返回空 Value::List，~2 FFI（downcast + 返回）
- ctx.insert：0
- 传原 field_input 给 CompiledArray::exec_build_py

**CompiledArray::exec_build_py**（py_exec.rs:1345-1388）：
- batch extract 路径（input.extract_batch_int_py）：
  - PyList_Check + PyList_Size：2 FFI
  - 100 × PyList_GetItem + PyLong_AsLongLong：200 FFI
  - 共 ~202 FFI ≈ ~8-10µs
- 100 × PyScalarInput::new + leaf exec_build_py：
  - PyScalarInput::new：0 FFI（Rust 分配）
  - leaf exec_build_py（FormatField）：PyScalarInput 直接 clone Value + inner.build，**0 FFI**
  - ctx.insert REPETITION_INDEX_KEY：0 FFI（Rust）
  - 共 0 FFI/元素 × 100 = 0 FFI

**Container/ListContainer 创建**（RC6）：~1.9µs（profiling §3 RC6）

**输出**：1 PyBytes::new ≈ 1 FFI

**总 FFI**：~22-30（入口） + 5（count） + 5（items 字段处理） + 202（batch extract） + 0（元素 build） + 1（输出） = **~235-243 FFI**

**预期耗时**：235-243 FFI × ~40-55ns + Container 开销 + Rust 开销
≈ 9.4-13.4µs + 1.9µs（Container）+ ~1µs（Rust） = **~12-16µs（无 RC5）/ ~12-16µs（RC5 对 array 无显著影响）**

**但**：profiling §2.3.3 显示当前 100 元素 array build 实测 88µs，0 元素 array build 实测 12.27µs。**0 元素已 12.27µs**，说明固定开销 + Container 创建已占 ~12µs。

**保守预测**（基于 profiling §5.5 表格）：修复 RC1+RC2+RC3 后 array build ≈ **~67-70µs**（仅消除 RC1 的 ~20µs）；若再修 RC5 ≈ **~67µs**（RC5 对 array 无显著影响）。

**对比 Python ~30µs → ~0.43-0.46x** ❌ **不达标**

**结论**：**array build 在 16.5 无法达标**。需要 Phase 17 的 schema-guided 批量编码（profiling §5.6），将 per-element exec_build_py 调用替换为裸 byteorder 批量编码。

#### 汇总表

| 格式 | 当前 | 修 RC1+RC2+RC3 后 | 再修 RC5 | Python 基线 | 是否达标 |
|------|------|------------------|---------|-----------|---------|
| simple | 3.7µs (0.74x) | ~3.1µs (0.9x) | **~2.7µs (1.04x)** | 2.8µs | ✅ 修 RC5 后达标 |
| nested | 9.4µs (0.61x) | ~8.3µs (**0.70x**) [REV P-MUST-1 修正] | **~7.9µs (0.73x)** [REV P-MUST-1 修正] | **5.8µs** [REV P-MUST-1 修正] | ❌ **不达标（推迟 Phase 17）** |
| array | 88µs (0.31x) | ~68µs (0.44x) | ~67µs (0.45x) | 30µs | ❌ 不达标（需 Phase 17） |

> **[REV P-MUST-1 修正] 关于 nested build 的全面修正**：
>
> 原汇总表标注 nested build "7.8µs (1.05x 边缘达标)"是基于错误的 Python 基线 ~3.2µs 计算的。
> 实际 Python nested build 基线经 `test_perf_comparison.py --iterations 5000` 实测为 **5.8µs**（0.0290s/5000）。
> 重算：
> - 修 RC1+RC2+RC3 后 ~8.3µs：5.8/8.3 = **0.70x**（非 1.0x）
> - 再修 RC5 后 ~7.9µs：5.8/7.9 = **0.73x**（非 1.05x）
>
> **nested build ≥1.0x 需 Rust ≤5.8µs**，当前 9.4µs，16.5 范围内最多节省 ~1.5µs → ~7.9µs，比率 ~0.73x。
> 根本原因：Python 原版 nested build 本身就需 5.8µs（Container 创建 + 嵌套遍历），Rust 的 per-field FFI 开销使优化后仍高于此基线。
> **nested build ≥1.0x 推迟至 Phase 17**（需 RC4 入口开销优化 + schema-guided 类型已知 extract）。
> 16.5 验收标准修正为：nested build 改善幅度（0.61x → ~0.73x）。

### 3.2 性能预测的可证伪性

本节预测的可证伪条件：

| 预测 | 可证伪条件 | 验证方法 |
|------|----------|---------|
| simple build ≥1.0x（修 RC1-RC5 后） | 实测 < 1.0x（>2.8µs） | `test_perf_comparison.py --format simple -n 5000` |
| nested build ~0.73x（修 RC1-RC5 后）[REV P-MUST-1 修正] | 实测偏离 ~0.73x 预测超过 ±0.1x | `test_perf_comparison.py --format nested -n 5000` |
| array build < 1.0x（修 RC1-RC5 后） | 实测 ≥1.0x（≤30µs） | `test_perf_comparison.py --format array_heavy -n 5000` |

> **[REV P-MUST-1 修正]** nested build 的可证伪条件已从"≥1.0x"改为"~0.73x ±0.1x"。
> nested build ≥1.0x 已明确推迟至 Phase 17（详见 §3.1 nested build 分析），16.5 仅验收改善幅度。

**若 simple 仍未达标**：
1. 用 profiling 工具（cProfile + 微基准）重新分析瓶颈
2. 优先检查 RC4（入口固定开销）是否可优化（绕过 pyo3 `#[pymethod]` 分发）
3. 评估是否需要 schema-guided 类型已知 extract（Phase 17）

**若 nested build 改善幅度不及预期（< 0.65x）** [REV P-MUST-1 修正]：
- 说明 RC2+RC3+RC5 的节省量低于 ~1.5µs 预测
- 用 profiling 定位额外开销源（如 ContainerClasses::clone 的 with_gil、IndexMap insert）
- 记录实测数据，作为 Phase 17 成本模型输入

**若 array build 反而达标**：
- 说明 Container 创建开销（RC6）在 Python 原版中也存在，且 Python 基线可能比 30µs 慢
- 重新校准 Python 基线

---

## §4 Parse 方向分析（profiling 之外的补充）

### 4.1 当前 parse 方向性能（PM 烟雾测试记录）

| 格式 | parse (Python) | parse (Rust) | 比率 | 达标（≥1.0x） |
|------|---------------|-------------|------|-------------|
| simple (3 字段) | 3.06µs | 1.58µs | **1.94x** | ✅ |
| nested (2 级) | 6.08µs | 3.98µs | **1.53x** | ✅ |
| array (100 元素) | 28.2µs | 51.5µs | **0.55x** | ❌ |

simple/nested parse 已达标，**array parse 不达标**（0.55x）。

### 4.2 Array parse 瓶颈分析

**与 build 方向的差异**：

| 维度 | Build 方向瓶颈（RC1-RC3） | Parse 方向瓶颈 |
|------|------------------------|--------------|
| 输入侧开销 | extract_ctx_value_py / extract_scalar_py 重复提取 | 无（parse 不读 Python 输入） |
| 输出侧开销 | 无（build 写 bytes，纯 Rust） | PyLong_FromLongLong + PyList_Append per element |
| Container 创建 | 仅 Struct+Array 触发（RC6 ~1.9µs） | 同 build |
| Box<dyn PySink> | per field ~20ns（B7） | per element ~20ns（B7，100 元素 = 2µs） |

**关键洞察**：parse 方向的 array 瓶颈与 build 方向**根本不同**：
- Build 方向的瓶颈是**输入侧的 per-element/field extract**（RC1-RC3）
- Parse 方向的瓶颈是**输出侧的 per-element PyObject 创建 + PyList append**

因此，RC1-RC3 的修复**对 parse 方向无影响**。

**Array parse 每 element 操作**（基于 py_exec.rs:1322-1343 + py_sink.rs 的 finish_item_py）：

| 步骤 | 操作 | FFI/element |
|------|------|------------|
| FormatField.parse (struct.unpack) | 0（Rust） | 0 |
| value_to_py_scalar | 1（PyLong_FromLongLong） | 1 |
| deposit_leaf | 0（Rust，存入 ctx_fields/obj） | 0 |
| sub_py_item | 0（Rust，返回 Empty sub-sink） | 0 |
| finish_item_py → list.append | 1（PyList_Append 或 call_method1） | 1 |
| ctx.insert REPETITION_INDEX_KEY | 0（Rust） | 0 |
| Box<dyn PySink> vtable + 分配 | 0 FFI，~20ns Rust | 0 |
| **合计/element** | **2 FFI + ~20ns Rust** | **2** |

100 元素 = 200 FFI ≈ 8-10µs + 100 × ~20ns Rust = 2µs + Container 开销 ~1.9µs + 入口固定开销 1.18µs

**理论预测**：~13-15µs（vs 当前实测 51.5µs）

**预测 vs 实测的差距**（~36-38µs）说明 parse 方向也有未识别的瓶颈。可能来源：
1. **Container/ListContainer 子类 append 的方法解析开销**：当前 `finish_item_py` 用 `list.call_method1("append", (obj,))`（py_sink.rs:527-529），经 Python 方法解析 ~25ns/元素，比裸 `PyList_Append` 慢 ~15ns/元素。100 元素 = ~1.5µs（不足以解释 36µs 差距）
2. **sub_py_item 每次 Box::new + 容器状态切换**：100 元素 × ~50ns = 5µs
3. **PyLong 创建对小整数（<256）的整数缓存命中失败**：Python 缓存 -5 到 256 的 int，但 PyLong_FromLongLong 可能不命中缓存路径
4. **PyDictSink2 内部 IndexMap（ctx_fields）的 per-element insert**：~30ns/元素 = 3µs

### 4.3 Parse 方向优化建议（16.5 范围内的快速修复）

**P1（建议在 16.5 一并实施）**：将 `finish_item_py` 的 `list.call_method1("append", ...)` 替换为裸 `pyo3::ffi::PyList_Append`：

```rust
// construct-rs/src/compiled/py_sink.rs（修改 finish_item_py / finish_named_item_py）
fn finish_item_py(&mut self, py: Python<'_>, sub: Box<dyn PySink>) -> Result<()> {
    let (value, obj) = sub.into_pair(py)?;
    self.ctx_items.push(value);
    // 优化：用裸 PyList_Append 替代 call_method1("append")，节省 ~15ns/元素
    // SAFETY: list 是 PyAny 的 list 子类（ListContainer），PyList_Append 对 list 子类等价。
    // obj 是 owned PyObject，PyList_Append 会 incref。
    unsafe {
        let ret = pyo3::ffi::PyList_Append(self.obj.as_ptr(), obj.as_ptr());
        if ret == -1 {
            return Err(py_err_to_construct(py, pyo3::err::PyErr::fetch(py)));
        }
    }
    // [REV P-MUST-3 修正] 删除 std::mem::forget(obj)。
    // PyList_Append 内部已对 obj incref（refcount +1，归 list 持有）。
    // obj 作为 owned PyObject，正常 drop 会 decref（refcount -1），
    // 净效果 = +1（list 持有的引用），引用计数正确。
    // 若 forget(obj) 则跳过 drop 的 decref，导致 refcount 永远多 1 → 内存泄漏。
    Ok(())
}
```

**注意**：`PyList_Append` 仅对原生 `PyList` 有效。`ListContainer` 是 `list` 子类，但其底层存储仍是 `PyListObject`，`PyList_Append` 可直接操作。依据：CPython 3.x `Objects/listobject.c` 的 `PyList_Append` 内部先用 `PyList_Check` 接受子类，然后直接操作 `PyListObject` 的 `ob_item` 数组。`ListContainer` 是 `class ListContainer(list)` 无额外实例字段，内存布局与 `PyListObject` 完全兼容，`PyList_Append` 安全。仍需通过单元测试验证。

**预期收益**：每元素节省 ~15ns × 100 = **~1.5µs**。array parse 从 51.5µs → ~50µs（仍不达标，但接近）。

**P2（Phase 17，不在 16.5 范围）**：批量 PyObject 构建。对同构数值数组，用 `PyList::new_bound(py, &vec)` 一次性构建 list（内部仍 per-element，但避免 trait dispatch + 状态机切换）。

**P3（Phase 17，不在 16.5 范围）**：numpy frombuffer 零拷贝。对同构 int 数组，用 `numpy.frombuffer` 创建 ndarray，避免 100× PyLong 创建。但需引入 numpy 依赖，超出 16.5 scope。

### 4.4 Parse 方向结论

| 优化 | 16.5 范围 | 预期 array parse 比率 |
|------|----------|--------------------|
| 当前（无优化） | — | 0.55x |
| P1（PyList_Append 替代 call_method1） | ✅ 是（建议） | ~0.56x（改善有限） |
| P2（PyList::new_bound 批量构建） | ❌ 否 | ~1.0-1.5x（Phase 17） |
| P3（numpy frombuffer） | ❌ 否 | ~3-5x（Phase 17） |

**建议**：16.5 一并实施 P1（与 RC5 修复在同一文件 py_sink.rs/py_input.rs，改动小）。array parse ≥1.0x 推迟到 Phase 17。

---

## §5 子任务拆分（供 PM 参考）

### 5.1 推荐子任务结构

| 子任务 | 内容 | 改动文件 | 预计行数 | 优先级 |
|-------|------|---------|---------|-------|
| **16.5.1** | RC1 修复：shallow_py_to_value_for_ctx list 分支改为空占位符 | py_input.rs | < 10 | 🔴 P0 |
| **16.5.2** | RC2+RC3 修复：CompiledStruct exec_build_py 方案 B（单次提取 + PyScalarInput 传递）+ is_scalar_value 辅助函数 | py_exec.rs | ~30-40 | 🔴 P0 |
| **16.5.3** | RC5 修复：extract_scalar_short_circuit bool 探测顺序优化 | py_input.rs | < 10 | 🟡 P1 |
| **16.5.4**（可选） | Parse 方向 P1：finish_item_py 用裸 PyList_Append 替代 call_method1 | py_sink.rs | ~15 | 🟡 P1 |
| **16.5.5** | 性能验收 + 设计文档修正（更新主设计 §4.4/§4.6/§6.3） | docs/ | 文档 | 🔴 P0 |

### 5.2 各子任务的详细要求

#### 16.5.1：RC1 修复

**输入**：本设计文档 §2.1
**DEV 任务**：
1. 修改 `py_input.rs:285-296` 的 list 分支为空占位符返回
2. 更新 `shallow_py_to_value_for_ctx` 的文档注释（明确"list 分支不遍历元素"）
3. 运行 61 个 dataclass 测试确认无回归（重点关注使用 `this._.items[...]` 等表达式的测试）
4. 单元测试：补充 `shallow_py_to_value_for_ctx_list_returns_empty_placeholder` 测试

**验收标准**：
- `cargo test --features python` 全 PASS
- 61 个 dataclass 测试全 PASS（或明确记录哪些测试因设计 §4.6 已知限制而失败）
- 微基准验证：100 元素 array build 边际成本从 ~736ns/元素 降至 ~536ns/元素（节省 ~200ns/元素，与 RC1 浪费量一致）

#### 16.5.2：RC2+RC3 修复

**输入**：本设计文档 §2.2
**DEV 任务**：
1. 在 py_exec.rs 添加 `is_scalar_value(&Value) -> bool` 辅助函数
2. 修改 `CompiledStruct::exec_build_py`（py_exec.rs:1207-1247）：
   - 对所有有名字的字段调 `extract_ctx_value_py`（**注意** [REV P-MUST-2 修正]：flagbuildnone 字段**有值**时也需提取，仅在 flagbuildnone && !has_field 时用 Value::None——详见 §2.2 修正后伪代码）
   - 标量字段用 `PyScalarInput::new(build_value)` 包装传给子节点
   - 复合字段保留原 `field_input`
3. 修改 `CompiledSequence::exec_build_py`（py_exec.rs:1289-1320）：同样的方案 B（Sequence 字段也是命名/匿名的标量或复合）
4. 修改 `CompiledUnion::exec_build_py`（py_exec.rs:1732-1772）：同样处理（虽然 Union 只构建一个字段，但该字段可能是标量）
5. 单元测试：补充 `compiled_struct_exec_build_py_scalar_field_uses_py_scalar_input` 测试，验证子节点收到 PyScalarInput
6. 单元测试 [REV P-MUST-2 修正]：补充 `compiled_struct_exec_build_py_flagbuildnone_with_value` 测试，验证 flagbuildnone 字段有值时正常提取（非 Value::None）

**验收标准**：
- `cargo test --features python` 全 PASS
- 61 个 dataclass 测试全 PASS（重点关注 Optional 类构造器）
- 微基准验证：simple build 每字段边际成本从 ~440ns 降至 ~240-320ns（节省 ~120-200ns/字段）

**风险提示** [REV P-IMP-1 修正]：CompiledAdapter/Validator/Const/Enum 等 wrapper 的 `exec_build_py` **不受影响**。wrapper 节点有两种模式：构造 PyScalarInput（Adapter/Const/Enum）或传原 input（Validator），方案 B 对两者都安全（详见 §2.2 关键不变量）。但需测试覆盖确认。

#### 16.5.3：RC5 修复

**输入**：本设计文档 §2.4
**DEV 任务**：
1. 修改 `py_input.rs:220-227` 的 `extract_scalar_short_circuit`：将 `is_instance_of::<PyBool>` 检查前置
2. 单元测试：补充 `extract_scalar_short_circuit_int_does_not_trigger_bool_extract` 测试

**验收标准**：
- `cargo test --features python` 全 PASS
- 微基准验证：int 字段 extract 边际成本降低 ~80ns

#### 16.5.4：Parse 方向 P1（可选，建议一并实施）

**输入**：本设计文档 §4.3
**DEV 任务**：
1. 修改 `py_sink.rs:finish_item_py` 和 `finish_named_item_py`：用 `pyo3::ffi::PyList_Append` 替代 `call_method1("append")`
2. 补充 SAFETY 注释（PyList_Append 对 list 子类等价——CPython `Objects/listobject.c` 中 PyList_Append 用 `PyList_Check` 接受子类）
3. **[REV P-MUST-3 修正] 不使用 `std::mem::forget(obj)`**：PyList_Append 已 incref，obj 正常 drop（decref）使净引用计数正确。forget 会泄漏。
4. 单元测试：验证 ListContainer 子类的 append 行为正确；**验证引用计数无泄漏**（append 后 obj 的 refcount 应为 1，非 2）

**验收标准**：
- `cargo test --features python` 全 PASS
- 微基准验证：array parse 100 元素边际成本降低 ~15ns/元素
- **[REV P-MUST-3 修正] 引用计数验证**：100 元素 array parse 后无内存泄漏（可用 `sys.getrefcount` 或重复运行检查 RSS 增长）

#### 16.5.5：性能验收 + 设计文档修正

**PM 任务**：
1. 运行全量性能测试：`test_perf_comparison.py -n 5000`
2. 验证 simple build ≥1.0x；记录 nested build 改善幅度（预期 ~0.73x，≥1.0x 推迟 Phase 17）[REV P-MUST-1 修正]
3. 明确记录 array build 比率（预期不达标，作为 Phase 17 输入）
4. 修正主设计文档（`模块设计-Python-first产出层.md`）：
   - §4.4：补充 CompiledStruct exec_build_py 的实际实现细节（方案 B）
   - §4.6：明确 shallow_py_to_value_for_ctx 的 list 分支为空占位符
   - §6.3：修正 simple/nested/array build 的 FFI 计数（基于 profiling 实测）
   - §6.3：修正固定开销估算（1.18µs vs 设计假设 ~200ns）
   - §6.3：修正 nested build Python 基线（~5.8µs，非 ~3.2µs）[REV P-MUST-1 修正]
5. 决策：是否需要 Phase 17（基于 nested build/array build/array parse 的实测结果）

### 5.3 子任务依赖关系

```
16.5.1 (RC1) ─┐
              ├─→ 16.5.5 (验收 + 文档修正)
16.5.2 (RC2+RC3) ─┤
                   │
16.5.3 (RC5) ─────┤
                   │
16.5.4 (parse P1) ─┘
```

**16.5.1-16.5.4 可并行实施**（不同文件 / 不同函数），16.5.5 在前四个完成后执行。

**建议**：合并 16.5.1+16.5.2+16.5.3 为单次 DEV 分派（build 方向核心修复），16.5.4 单独分派（parse 方向优化），16.5.5 由 PM 执行。

### 5.4 总体验收标准（Phase 16.5 出口）

| 项 | 标准 | 验证方法 |
|----|------|---------|
| 编译 | `cargo build --features python` 零 error | 自动 |
| Clippy | `cargo clippy --features python --all-targets` 零 warning | 自动 |
| 格式 | `cargo fmt --check` 通过 | 自动 |
| 纯 Rust 测试 | 1515 个测试全 PASS | `cargo test`（无 python feature） |
| Python feature 测试 | 全 PASS（`--test-threads=1`） | `cargo test --features python` |
| Dataclass 测试 | 61 个全 PASS（或明确记录已知限制） | `pytest tests/test_dataclass_api.py` |
| **simple build** | **≥1.0x（≤2.8µs）** | `test_perf_comparison.py --format simple -n 5000` |
| **nested build** | **改善至 ~0.73x（≤~7.9µs）；≥1.0x 推迟 Phase 17** [REV P-MUST-1 修正] | `test_perf_comparison.py --format nested -n 5000` |
| array build | 记录实测比率（预期 < 1.0x，推迟 Phase 17） | 同上 |
| array parse | 记录实测比率（预期 < 1.0x，推迟 Phase 17） | 同上 |
| 设计文档 | 主设计 §4.4/§4.6/§6.3 修正完毕 | 文档审查 |

### 5.5 是否需要 Phase 17 的判断

**需要 Phase 17 的条件**（任一满足）：
1. array build < 1.0x（预期会满足）
2. array parse < 1.0x（预期会满足）
3. **nested build < 1.0x（预期会满足）** [REV P-MUST-1 修正]：Python 基线实测 5.8µs，16.5 优化后 ~7.9µs（0.73x），不达标
4. simple build < 1.0x（理论上不会，但余量仅 ~4%）

**Phase 17 候选内容**（基于 profiling §5.6 + 本文 §4.3）：
1. **Schema-guided 批量编码**（build 方向 array）：编译树识别"Array 的 subcon 是定宽 FormatField"，生成裸 byteorder 批量编码路径，跳过 per-element exec_build_py
2. **Schema-guided 类型已知 extract**（build 方向标量）：编译树已知 leaf 类型，生成专用 extract 函数（如 `extract_int_raw` 直接 `PyLong_AsLongLong`，无类型探测）
3. **批量 PyObject 构建**（parse 方向 array）：用 `PyList::new_bound(py, &vec)` 一次性构建 list
4. **numpy frombuffer 零拷贝**（parse 方向 array）：对同构 int 数组用 numpy，避免 100× PyLong 创建
5. **入口固定开销优化**（RC4）：评估绕过 pyo3 `#[pymethod]` 分发的可能性（如裸 `PyCFunction` 注册）

**建议**：PM 在 16.5 验收后，基于实测数据决策 Phase 17 的范围。预期 Phase 17 **必须启动**（nested build 0.73x + array build/parse 不达标）。Phase 17 候选内容中，**schema-guided 类型已知 extract + RC4 入口开销优化**是 nested build 达标的关键，**schema-guided 批量编码**是 array build 达标的关键。

---

## §6 风险与缓解

### 6.1 设计层风险

| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| RC1 修复破坏 dataclass 测试（`this._.items[...]` 模式） | 中 | 高（功能回归） | 16.5.1 验收时全量运行 dataclass 测试；若有失败，回滚 RC1 或改为折中方案（仅对纯标量列表浅提取） |
| 方案 B 的 `is_scalar_value` 误判复合字段为标量 | 低 | 中（特定格式构建失败） | 通过 wrapper 节点的 exec_build_py 不依赖父传 PyScalarInput 的不变量保证；61 个 dataclass 测试覆盖 |
| nested build ≥1.0x 在 16.5 不可达标 [REV P-MUST-1 修正] | **确定**（已实测） | 中（Phase 16.5 目标缩减） | Python 基线实测 5.8µs（非原假设 3.2µs）；16.5 仅验收改善幅度（0.61x→~0.73x）；≥1.0x 推迟 Phase 17（RC4 + schema-guided extract） |
| flagbuildnone 字段方案 B 行为错误（P-MUST-2） | 中（伪代码有 bug） | 高（Optional 类构造器 build 失败） | 已修正伪代码：flagbuildnone 字段有值时正常提取（§2.2）；DEV 实现时需测试覆盖 Optional 类构造器 |

### 6.2 实施层风险

| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| PyList_Append 对 ListContainer 子类的兼容性（16.5.4） | 低 | 中（parse 行为错误） | 单元测试覆盖 ListContainer 实例；若失败，回退到 call_method1 |
| PyList_Append 引用计数泄漏 [REV P-MUST-3 修正] | ~~中~~ → **已消除** | ~~高（每次 append 泄漏 1 refcount）~~ | 已删除 `std::mem::forget(obj)`，obj 正常 drop 使引用计数正确；DEV 需验证引用计数无泄漏 |
| 优化后 Rust 内部分配开销（Box/Vec/clone）主导 | 中 | 低（性能不达标但不会更差） | 用 profiling 定位；评估栈式 sink（Phase 17） |
| Python 3.14 + pyo3 0.22 的测试线程限制 | 已知 | 低（`--test-threads=1`） | 已在 16.2-16.4 验证，沿用相同测试方式 |

### 6.3 项目层风险

| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| Phase 16 持续延期，影响后续 Phase 17-19 | 中 | 中 | 16.5 严格限定范围（仅 RC1-RC5 修复），不引入新架构 |
| array build/parse + nested build 不达标被用户感知 [REV P-MUST-1 修正] | 中 | 中（项目定位受损） | Phase 17 立即启动 schema-guided 优化（含 nested build 的类型已知 extract + RC4）；在 README/CHANGELOG 注明当前版本 nested/array 性能限制 |

---

## §7 与 Python 版本的对应关系

Phase 16.5 的优化**不改变** Rust 实现与 Python 原版的语义对应关系。所有优化均为内部性能改进，输出字节序列与 Python 原版完全一致。

| Python 类/方法 | Rust 类型/方法 | Phase 16.5 变化 |
|---------------|--------------|----------------|
| `Construct._build` | `PyCompiledExec::exec_build_py` | CompiledStruct 实现优化（方案 B），签名不变 |
| `Container.__init__` | `ContainerClasses::new_container` | 不变 |
| `context._` 设置 | `shallow_py_to_value_for_ctx` | list 分支改为空占位符（对齐设计 §4.6 原意） |
| `Struct._build` 字段遍历 | `CompiledStruct::exec_build_py` 字段循环 | 单次提取 + PyScalarInput 传递（消除重复 extract） |
| `Array._build` 元素遍历 | `CompiledArray::exec_build_py` batch extract | 不变（16.5 不修改 CompiledArray） |

---

## §8 边界条件清单

| 编号 | 场景 | 预期行为 | 验证方法 |
|------|------|---------|---------|
| BC-1 | RC1 修复后 `this._.list_field[0]` 表达式 | 返回空 list，下标访问失败（设计 §4.6 已知限制） | dataclass 测试覆盖；若失败，记录为已知限制 |
| BC-2 | RC2 方案 B 对 Adapter(Scalar) 字段 [REV P-IMP-2 修正] | 方案 B 对复合字段（含 Adapter）传**原 field_input**（PyDictInput），非 PyScalarInput。Adapter exec_build_py 自行调 extract_ctx_value_py 从原 input 提取值。对**标量字段**（is_scalar_value=true），方案 B 传 PyScalarInput，但 wrapper 的 extract_ctx_value_py 返回 clone——行为正确 | 单元测试 + dataclass 测试覆盖 Adapter |
| BC-3 | RC2 方案 B 对匿名字段 | 匿名字段用 PyScalarInput(Value::None)，不调 extract_ctx_value_py | 单元测试（与当前行为一致） |
| BC-4 | RC2 方案 B 对 flagbuildnone 字段（字段不存在） | flagbuildnone && !has_field 时用 PyScalarInput(Value::None)，不调 extract | 单元测试（py_exec.rs:1218-1220 现有逻辑） |
| **BC-NEW-1** | RC2 方案 B 对 flagbuildnone 字段（字段**有值**） [REV P-MUST-2 修正] | flagbuildnone=true 但 has_field=true 时，**正常提取** field_input.extract_ctx_value_py，返回真实值（非 Value::None）。修正后的伪代码（§2.2）正确处理此场景。**原伪代码 `!field.flagbuildnone` 会错误返回 Value::None，破坏 Optional 类构造器 build** | 单元测试：Optional(Int32ub) 字段有值时 build 产出正确字节 |
| BC-5 | RC5 修复后 `True`/`False` 提取 | 仍提取为 Value::Bool（PyBool 检查前置不影响语义） | 单元测试 |
| BC-6 | RC5 修复后 `1`/`0` 提取 | 仍提取为 Value::Int（PyBool 检查失败后走 i64 分支） | 单元测试 |
| BC-7 | 16.5.4 PyList_Append 对 ListContainer [REV P-MUST-3 修正] | 行为与 call_method1("append") 一致（ListContainer 是 list 子类，CPython PyList_Append 接受子类）；obj 正常 drop，**无内存泄漏**（已删除 forget） | 单元测试 + 引用计数验证 |
| BC-8 | 16.5.4 PyList_Append 对 plain PyList | 行为正确（PyList_Append 原生支持）；obj 正常 drop | 单元测试 |
| BC-9 | 空 Struct build（0 字段） | 行为不变（循环不进入） | 沿用 16.4 测试 |
| BC-10 | 空 Array build（count=0） | batch extract 路径 ints.len()==0 == count，正确处理 | 沿用 16.4 测试 |

---

## §9 与其他模块的交互

### 9.1 依赖模块（已设计/已实现）

| 模块 | 关系 | Phase 16.5 影响 |
|------|------|----------------|
| `compiled/py_input.rs`（16.2 已实现） | 修改 shallow_py_to_value_for_ctx + extract_scalar_short_circuit | RC1 + RC5 修改 |
| `compiled/py_exec.rs`（16.3/16.4 已实现） | 修改 CompiledStruct::exec_build_py + CompiledSequence + CompiledUnion | RC2+RC3 修改 |
| `compiled/py_sink.rs`（16.2 已实现） | 修改 finish_item_py / finish_named_item_py | 16.5.4 修改（parse P1） |
| `compiled/mod.rs`（Phase 12 已实现） | 不修改 | — |
| `compiler/`（Phase 12 已实现） | 不修改（schema-guided 优化推迟到 Phase 17） | — |
| `core/`（Phase 1 已实现） | 不修改 | — |

### 9.2 被依赖模块

无。Phase 16.5 是 build 方向性能修复，不被其他模块依赖。

### 9.3 设计文档更新（Phase 16.5 完成后）

主设计文档（`模块设计-Python-first产出层.md`）需要更新以下章节：

| 章节 | 修正内容 |
|------|---------|
| §4.4 | CompiledStruct::exec_build_py 代码草图补充方案 B 的实际实现（单次提取 + PyScalarInput） |
| §4.6 | 明确 shallow_py_to_value_for_ctx 的 list 分支为空占位符（对齐实现） |
| §6.1 | 固定开销从 ~200ns 修正为 ~1.18µs（基于 profiling 实测） |
| §6.3 P1 | simple build FFI 从 ~14 修正为 ~38-46（含入口开销） |
| §6.3 P2 | nested build FFI 从 ~10 修正为 ~45-53；**Python 基线从 ~3.2µs 修正为 ~5.8µs** [REV P-MUST-1 修正]；预测从 ~3.1x 修正为 ~0.73x（16.5 范围内不达标，≥1.0x 推迟 Phase 17） |
| §6.3 P4 | array build 预测从 ~5.4x 修正为 ~0.4x（实测），Phase 17 schema-guided 优化可达标 |

这些修正不影响已通过的 16.1-16.4 验收，仅作为 Phase 17 及后续阶段的成本模型基线。

---

## 附录 A：实测数据来源索引

| 数据 | 来源 | 文件位置 |
|------|------|---------|
| 当前 build 性能（0.74x/0.61x/0.31x） | profiling §1 | `Phase16-build性能分析.md:13-21` |
| 固定开销 1.18µs | profiling §2.3.1 | `Phase16-build性能分析.md:54` |
| 每标量字段边际 ~440ns | profiling §2.3.2 | `Phase16-build性能分析.md:70` |
| 每元素边际 ~736ns | profiling §2.3.3 | `Phase16-build性能分析.md:83` |
| int vs bytes array 差异（证明 RC1） | profiling §2.3.4 | `Phase16-build性能分析.md:91-98` |
| FFI 单次成本 37-55ns | profiling §2.4 | `Phase16-build性能分析.md:113-117` |
| RC1-RC6 根因分析 | profiling §3 | `Phase16-build性能分析.md:123-243` |
| 优化建议 P0-P2 | profiling §5 | `Phase16-build性能分析.md:275-394` |
| 当前 parse 性能（1.94x/1.53x/0.55x） | PM 烟雾测试记录 | `过程记录.md:354-358` |

## 附录 B：术语表

| 术语 | 含义 |
|------|------|
| RC1-RC6 | Root Cause 1-6，profiling 识别的 6 个性能瓶颈根因 |
| 方案 B | CompiledStruct exec_build_py 的单次提取 + PyScalarInput 传递方案（消除 RC2+RC3） |
| PyScalarInput | 包装已知 Value 的 PyInput 实现，extract_scalar_py 直接 clone（零 FFI） |
| schema-guided | 编译树在编译期已知的类型信息指导运行时优化（如类型已知 extract、批量编码） |
| batch extract | 用裸 CPython API（PyList_GET_ITEM + PyLong_AsLongLong）批量提取 list 元素 |
| FFI | Foreign Function Interface，此处指 Rust 调用 CPython C API 的边界 |
| shallow_py_to_value_for_ctx | 将 Python 对象浅转换为 Value（供 ctx 表达式使用），不递归复合类型 |
| extract_ctx_value_py | PyInput trait 方法，提取适合 ctx 插入的 Value（标量直接返回，复合 shallow 转换） |
