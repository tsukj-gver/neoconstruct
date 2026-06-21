# P0 性能优化分析：pydantic-core 对照报告

> **⚠️ 临时文档** — P0 优化验证通过后合并到正式设计文档。
>
> 本文档固化 P0 性能优化前的基线数据、瓶颈定位、pydantic-core 对照结论与优化方案。
> 优化实施并验证后，结论将合并至 `docs/` 下正式设计文档，本文档可归档/删除。

---

## 文档状态

| 项目 | 内容 |
|------|------|
| 创建时间 | 2026-06-22 |
| 角色 | ARCH（架构师） |
| 触发阶段 | Phase 1 垂直切片 — 性能优化（tag: pre-p0-optimization） |
| 数据来源 | `construct-rs/tests/bench_breakdown_results.txt`、`bench_stages_results.txt`、源码审查、pydantic-core 对照调查 |
| 事实核验 | 所有源码行号、benchmark 数据均经 ARCH 逐项核对，与仓库当前状态一致 |

---

## 1. 当前性能基线（P0 优化前）

### 1.1 测试环境

- Python 版本：3.14.2
- 平台：Windows-11-10.0.26200
- 处理器：Intel64 Family 6 Model 183（Intel 第 13/14 代酷睿架构）
- 基准标签：`pre-p0-optimization`（方案 B' 实施后）
- 测量方式：`timeit`，breakdown 取 50000×5 中位数，stages 取 10000×3 中位数

### 1.2 基准数据

**方案 B' 实施后基准（tag: pre-p0-optimization）**：

| 用例 | N | Rust _parse_raw (ns) | Full parse (ns) | Python construct (ns) | 加速比 |
|------|---|---------------------|-----------------|----------------------|--------|
| B1 | 3 | 1732 | 1798 | 3055 | 1.70x |
| B2 | 10 | 3325 | 3424 | 5933 | 1.73x |
| B3 | 50 | 12658 | 12806 | 22894 | 1.79x |
| B4 | 100 | 23410 | 23514 | 43089 | 1.83x |

> 数据出处：`bench_breakdown_results.txt`。Full parse 与 Rust _parse_raw 几乎相等（差值 < 1.5%），
> 证明实例构造在 Rust 内完成、无额外 Python 层开销（方案 B' 的预期）。

### 1.3 线性模型与开销分解

**每字段开销**（_parse_raw / N，随 N 增大收敛）：
- B1 (N=3):   577.46 ns/field
- B2 (N=10):  332.50 ns/field
- B3 (N=50):  253.15 ns/field
- B4 (N=100): 234.10 ns/field

**固定开销**（线性回归截距）：~1093 ns/call

> 结论：parse 耗时 = 固定开销（~1093 ns）+ 每字段开销（~234 ns）× N。线性模型成立。

### 1.4 与理论下界的差距（B4 实测，N=100）

来源：`bench_stages_results.txt`。该文件以 Python 等价操作拆解每字段开销：

| 组成 | ns/field |
|------|----------|
| 纯 C 层 `struct.unpack`（字节解包下界） | 88.13 |
| PyLong 构造 + dict SetItem 增量 | 62.93 |
| 实例构造摊销（`__new__` + `__dict__` 替换） | 2.66 |
| **Python 侧理论下界合计** | **153.71** |
| **Rust 内核实测** | **237.32** |
| **不明开销（Rust − 理论下界）** | **83.61** |

> 这 83.61 ns/field 的"不明开销"正是 P0 优化的目标区间。

---

## 2. 性能瓶颈精确定位（7 个问题）

DEV 通过分阶段计时（`bench_stages_results.txt`）和源码审查，定位到以下 7 个具体瓶颈。
所有源码行号基于 `pre-p0-optimization` tag 的仓库状态，经 ARCH 核对一致。

### 问题 1：Path::push_field 每字段 String 堆分配

- **位置**：`src/path.rs:66-68`
- **机制**：`push_field` 内部 `name.to_string()` 为每个字段分配一个 `String` 堆对象。
  即使 parse 成功（成功路径），路径栈仍在每字段分配并 `pop`。`Path::new()`（line 59-63）
  还额外分配 `"(root)".to_string()`。
- **估算**：30-50 ns/field
- **影响**：与模块文档"成功路径零成本"的承诺直接矛盾。成功路径不应有任何堆分配。
- **关键代码**：
  ```rust
  pub fn push_field(&mut self, name: &str) {
      self.segments.push(PathSegment::Field(name.to_string())); // 每字段分配
  }
  ```

### 问题 2：未启用 LTO

- **位置**：`Cargo.toml` 缺 `[profile.release]` 段（已确认：文件中无任何 `profile` 配置）
- **机制**：pyo3 提供大量小函数（`set_item`、`bind`、`into_py`、`force_setattr` 等），
  未启用跨 crate 内联时，每个调用点都是一次真实函数调用而非内联展开，
  阻止编译器消除调用开销。
- **估算**：5-15 ns/field
- **影响**：这是"免费"的优化（仅改 Cargo.toml），应作为 P0 首项。

### 问题 3：dict.set_item 的 map_err 闭包

- **位置**：`src/nodes/struct_node.rs:203-211`
- **机制**：每个字段的 `dict.set_item(...)` 后跟 `.map_err(|e| format!(...))`，
  闭包捕获 `field_name` 与 `path`。即使 set_item 几乎不失败，闭包构造仍带来开销。
- **估算**：2-5 ns/field
- **关键代码**：
  ```rust
  dict.set_item(field_name.py_name().bind(py), value.bind(py))
      .map_err(|e| ConstructError::Generic {
          message: format!("failed to set dict item for field '{}': {}", ...),
          path: path.to_string(),
      })?;
  ```

### 问题 4：PyLong 构造

- **位置**：`src/nodes/format_field.rs:282`（及同类 `into_py(py)` 调用）
- **结论**：`u32::into_py(py)` 内部走 `ffi::PyLong_FromUnsignedLong`，已是理论最优路径。
- **决定**：**不可优化**，排除出 P0 范围。

### 问题 5：分派机制

- **结论**：当前使用 `#[enum_dispatch]` 静态分派，与 pydantic-core 采用的 `CombinedValidator`
  机制完全一致，无需改动。
- **决定**：**不需改动**，排除出 P0 范围。

### 问题 6：Context 创建开销

- **位置**：`src/schema.rs:114`（`Context::new_root(py)?`）
- **机制**：Phase 1 的 parse 路径**完全不读取** context，但每次 parse 仍创建一个空 `PyDict`
  作为 root context。这是纯粹的固定浪费。
- **估算**：80-150 ns 固定开销
- **关键代码**：
  ```rust
  let mut ctx = Context::new_root(py)?; // Phase 1 不读，但每次都创建
  ```

### 问题 7：固定开销 ~1093 ns 构成拆解

固定开销是每次 parse 的不可避免成本，主要由 CPython 对象创建构成：

| 组成 | 估算 (ns) | 说明 |
|------|-----------|------|
| FFI 边界穿越（入） | 150-250 | pyo3 的 `_parse_raw` 调用开销 |
| `Context::new_root` | 80-150 | 问题 6，Phase 1 可省略 |
| `Path::new` | 50-100 | 问题 1 的固定部分，含 `"(root)".to_string()` |
| `PyDict::new_bound` | 80-150 | 存放解析结果的 dict |
| `create_class`（tp_new） | 200-400 | 创建用户 dataclass 实例 |
| `force_setattr` | 100-200 | 把 dict 挂到实例 `__dict__` |
| FFI 返回（出） | 50-100 | 返回 PyObject 给 Python |

> 合计 ~810-1250 ns，与实测 ~1093 ns 吻合。

---

## 3. pydantic-core 对照分析（7 个问题的答案）

ARCH 调查 pydantic-core（`refs/` 下参考项目）源码后，为上述 7 个问题逐一找到对照答案。
pydantic-core 是本项目的核心参照系（AGENTS.md §0 明确要求对标）。

### 问题 1 答案：pydantic-core 的路径追踪

pydantic-core **完全不在成功路径维护路径栈**：

- `ValidationState.field_name: Option<Bound<PyString>>` —— 引用**已有**的 Python 字符串对象，
  零分配（不是创建新 String）。
- 使用 RAII scope guard 设置/恢复字段名：`mem::replace` 保存旧值，`Drop` 时恢复。
- `Location` 类型只在 **Err 路径**构建；`Location::Empty` 变体零成本（成功路径不触及）。
- **代码位置**：`validation_state.rs:39, 98-103, 271-278`；`model_fields.rs:334-335`

> 教训：路径信息应**延迟到出错时才构建**。成功路径只持有一个轻量的"当前字段名引用"。

### 问题 2 答案：pydantic-core 的 Cargo.toml

```toml
[profile.release]
lto = "fat"
codegen-units = 1
strip = true
```

- **代码位置**：`Cargo.toml:57-60`
- `lto = "fat"`：跨 crate 全量内联，pyo3 小函数得以内联进 validator。
- `codegen-units = 1`：单编译单元，最大化优化空间。
- `strip = true`：剥离符号减小二进制（次要收益）。

### 问题 3 答案：pydantic-core 的 dict set_item

```rust
model_dict.set_item(&field.name, value)?;  // 直接 ?，无 map_err
```

- `ValResult` 实现 `From<PyErr>`，使 `?` 自动把 `PyErr` 转为 `ValResult:: PyErr`，无需闭包。
- 错误上下文（字段名、路径）由**上层统一处理**，不在每个 set_item 点重复构造。
- **代码位置**：`model_fields.rs:363, 688`

### 问题 4 答案：PyLong 构造

- pydantic-core 的 JSON 路径同样从 Rust 整数创建 PyLong（`ffi::PyLong_FromLongLong`）。
- 与我们的 `into_py(py)` 路径**等价**，已是理论下界。
- **结论**：印证问题 4 的判断——此路径不可优化。

### 问题 5 答案：分派机制

- pydantic-core 同样使用 `#[enum_dispatch]` + `CombinedValidator` enum。
- 与我们**完全相同**。
- 额外优化（非分派本身）：validator 单例缓存（`LazyLock<Arc<CombinedValidator>>`），
  避免重复构造验证器树——这是编译期优化，不在 P0 parse 开销范围内。

### 问题 6 答案：pydantic-core 的状态管理

- `ValidationState` **全栈分配**，零 Python 对象创建。
- `ValidationState::new()` 所有字段初始化为 `None` 或默认值。
- `data: Option<Bound<PyDict>>` 只有在需要时才设置（引用正在构建的 dict），而非每次创建。
- **代码位置**：`validation_state.rs:21-46, 49-67`

> 教训：状态对象不应预创建 Python 对象；按需（lazy）才创建。

### 问题 7 答案：pydantic-core 的固定开销

- pydantic-core 同样有 `tp_new` + `PyDict::new` + `force_setattr`。
- pydantic-core 甚至做 **4 次** `force_setattr`（我们只 1 次）。
- pydantic-core 相比我们**省掉**的：Context 创建 + Path 创建（合计 130-250 ns）。
- **核心结论**：固定开销的**绝大部分是 CPython 不可压缩成本**（对象创建、tp_new、FFI），
  无法通过 Rust 侧优化消除。可压缩的只有 Context（问题 6）和 Path（问题 1）这两项。

---

## 4. P0 优化方案

基于第 3 节的 pydantic-core 对照答案，P0 范围聚焦在**可压缩项**（问题 1、2、3、6）。
问题 4（PyLong）、问题 5（分派）经对照确认无需改动，排除出 P0。

| # | 优化项 | 对应问题 | 预期收益（每字段） | 预期收益（固定） | 实施方式 | 参照 |
|---|--------|---------|-------------------|-----------------|---------|------|
| P0-1 | 启用 fat LTO + codegen-units=1 | 问题 2 | 5-15 ns | ~0 | `Cargo.toml` 加 `[profile.release]` 段 | pydantic-core `Cargo.toml:57-60` |
| P0-2 | 移除 Phase 1 的 `Context::new_root` | 问题 6 | 0 | 80-150 ns | `schema.rs:114` 改为不创建 Context（传占位/`Option`） | pydantic-core 按需创建 state |
| P0-3 | 移除成功路径的 Path 维护 | 问题 1 | 25-45 ns | 50-100 ns | `path.rs` + `struct_node.rs` 改为错误路径延迟构建 Path（RAII + 引用已有 PyString） | pydantic-core `validation_state.rs` |
| P0-4 | `set_item` 改用 `?` + `From<PyErr>` | 问题 3 | 2-5 ns | 0 | `struct_node.rs:203-211` 移除 `map_err`，错误上下文上移到统一层 | pydantic-core `model_fields.rs:363` |

### 4.1 P0 优化预期总收益

- **每字段合计**：32-65 ns/field（P0-1 + P0-3 + P0-4）
- **固定合计**：130-250 ns/call（P0-2 + P0-3 固定部分）

> 折算到 B4（N=100）：减少约 32×100 + 190 ≈ 3390 ns，占当前 23514 ns 的 ~14%。
> 每字段从 ~234 ns 降至 ~190 ns 左右，加速比从 1.83x 提升到约 2.3x。

### 4.2 实施顺序与风险

1. **P0-1（LTO）先行**：零代码风险，纯配置改动，先建立"优化后基线"。
2. **P0-4（set_item）紧随**：改动局部、风险低，验证错误信息仍可追溯。
3. **P0-2（Context）**：需调整 `parse` 签名/传参，涉及多节点，但逻辑简单。
4. **P0-3（Path）殿后**：改动最广（path.rs + 所有调用点），RAII 重构需谨慎，
   必须保证错误路径的 `path.to_string()` 仍能正确还原完整路径。

---

## 5. 预期效果

P0 优化后预估加速比（线性模型：固定开销 + 每字段开销 × N）：

| 场景 | 当前固定 (ns) | 当前每字段 (ns) | 当前实测 (ns) | 当前加速比 | P0 后预估 (ns) | P0 后加速比 |
|------|--------------|----------------|--------------|-----------|---------------|------------|
| B1 (N=3) | 1093 | 234 | 1798 | 1.70x | ~850 + 200×3 ≈ 1450 | ~2.0x |
| B2 (N=10) | 1093 | 234 | 3424 | 1.73x | ~850 + 200×10 ≈ 2850 | ~2.1x |
| B3 (N=50) | 1093 | 234 | 12806 | 1.79x | ~850 + 200×50 ≈ 10850 | ~2.1x |
| B4 (N=100) | 1093 | 234 | 23514 | 1.83x | ~850 + 200×100 ≈ 20850 | ~2.1-2.3x |

> 注：P0 后每字段取乐观值 ~200 ns（234 − 34 中值），固定取乐观值 ~850 ns。
> B4 大结构受益最大（每字段优化 ×100 次累积）。

---

## 6. 4x 目标可达性分析

项目性能目标是 **≥4x vs Python construct**（AGENTS.md §0），10x 为理想目标。
P0 优化后距离 4x 仍有显著差距，分场景分析：

### 6.1 B4（大结构，100 字段）

- P0 后：~2.3x
- 要达到 4x：每字段需从 ~200 ns 降到 ~108 ns
  （目标总耗时 = 43089 / 4 ≈ 10772 ns，扣除固定 ~850 ns，剩 ~9922 ns / 100 ≈ 99 ns/field）
- 理论下界参考：`bench_stages_results.txt` 测得 Python 侧等价操作下界 **153.71 ns/field**
  （struct.unpack + PyLong + dict SetItem + 实例摊销）。
- **判断**：~108 ns/field **低于**已测的 153.71 ns/field Python 等价下界，
  意味着 4x 在"返回 dataclass 实例"的当前架构下**接近物理极限**。
  理论上需突破 PyLong/dict 的 CPython 调用成本才可达，**理论可行但需激进优化**。

### 6.2 B1（小结构，3 字段）

- P0 后：~2.0x
- 固定开销 ~850 ns 占总耗时 ~40-50%。
- **判断**：固定开销主要是 CPython 不可压缩成本（tp_new、PyDict::new、force_setattr、FFI），
  问题 7 的对照分析已证明这部分无法在 Rust 侧消除。
- **结论**：**当前架构（返回 dataclass 实例）下，小结构物理上无法达到 4x**。

### 6.3 总体结论

- P0 能把加速比从 ~1.8x 提升到 ~2.0-2.3x，是必要的基础优化。
- 4x 目标在当前架构下对**小结构不可达**，对**大结构接近物理极限**。
- 要逼近 4x，必须进入第 7 节的激进优化路径，突破 CPython 对象创建成本。

---

## 7. 激进优化路径（后续评估，非 P0 范围）

以下方案可能突破 CPython 不可压缩成本，但均涉及架构权衡，列为后续阶段评估项：

1. **放弃 ABI3，使用 `_PyDict_NewPresized(N)`**
   - 预分配 N 个槽位的 dict，避免 dict 扩容重哈希。
   - 代价：丧失 ABI3 跨版本二进制兼容，需按 Python 版本编译。
   - 预期：节省 dict 扩容开销，对大结构（N=100）收益明显。

2. **Flat struct fast path（编译期专用代码）**
   - 在 `__init_subclass__` 编译期为"全 FormatField 的扁平 Struct"生成专用 parse 函数，
     完全跳过通用分派与节点遍历。
   - 预期：消除分派 + 路径维护 + 逐节点调用，逼近 struct.unpack 下界。

3. **避免 create_class（直接返回 dict 或缓存实例）**
   - 部分场景（如纯数据交换）直接返回 dict，跳过 tp_new + force_setattr。
   - 或缓存 `PyType` / 实例原型，复用已分配对象。
   - 代价：改变返回类型语义，需用户显式 opt-in。

---

## 附录 A：数据来源与事实核验清单

ARCH 在编写本文档时对以下引用逐项核验，确认与 `pre-p0-optimization` tag 仓库状态一致：

| 引用 | 核验结果 |
|------|---------|
| `bench_breakdown_results.txt` B1-B4 数据 | ✓ 一致（B4 raw=23410.1, full=23513.8, py=43088.8） |
| `bench_stages_results.txt` 理论下界 153.71 ns/field | ✓ 一致 |
| `src/path.rs:66-68` push_field 的 `name.to_string()` | ✓ 精确（line 67） |
| `src/path.rs:59-63` Path::new 的 `"(root)".to_string()` | ✓ 精确（line 61） |
| `Cargo.toml` 缺 `[profile.release]` | ✓ 确认（文件无 profile 配置） |
| `src/nodes/struct_node.rs:203-211` set_item + map_err | ✓ 精确 |
| `src/nodes/format_field.rs:282` `u32::from_be_bytes(arr).into_py(py)` | ✓ 精确 |
| `src/schema.rs:114` `Context::new_root(py)?` | ✓ 精确 |

## 附录 B：相关文件索引

- 基准数据：`construct-rs/tests/bench_breakdown_results.txt`、`bench_stages_results.txt`
- 受影响源码（P0 优化将修改）：
  - `construct-rs/Cargo.toml`（P0-1）
  - `construct-rs/src/schema.rs`（P0-2）
  - `construct-rs/src/path.rs`（P0-3）
  - `construct-rs/src/nodes/struct_node.rs`（P0-3, P0-4）
- pydantic-core 参照（只读）：`refs/` 下相关项目
- 项目性能目标：`AGENTS.md` §0（≥4x，10x 理想）

---

> **文档结束** — 待 P0 优化实施并通过性能验证后，由 PM 决定合并至正式设计文档。
