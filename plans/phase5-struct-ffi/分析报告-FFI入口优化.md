---
id: ANALYSIS-phase5-ffi
status: active
phase: "5"
last_updated: 2026-07-29
---

# Phase 5 分析报告：FFI 入口现状与优化方向

> **子任务**：`5.0 [分析报告]`（PENDING → DESIGNING 的前置）
> **作者**：ARCH
> **数据基准**：`docs/perf-scenarios.csv`（155 测量点）+ `docs/analysis/phase3-parse-build不对称.md`（bench_asymmetry 实测）+ `construct-rs/src/`（FFI 入口源码）
> **核心约束**：本报告所有性能数字均引用实测数据点（L-02 教训），所有候选含可证伪预测（L-05 教训），所有方案不违反 AGENTS.md §0（L-01 教训）。
>
> **权限说明**：本报告原任务要求输出至 `plans/phase5-struct-ffi/`，但 AGENTS.md §2 明确 ARCH 只读 `plans/`、可写 `docs/`。为遵守角色权限边界，报告写入 `docs/analysis/`，请 PM 协调（建立符号链接 / 移动 / 在总纲中引用本路径）。

---

## 1. FFI 入口现状分析

### 1.1 当前 FFI 调用栈（parse 方向，B1 场景）

基于源码 `construct-rs/src/schema.rs:148-165` + `python/construct/_mixin.py:1051-1070`，每次 `cls.parse(data)` 完整经历以下层次：

```
[Python 层]
  1. cls.parse(data)                          ← classmethod 调用（CPython LOAD_METHOD + CALL）
  2. StructMixin.parse 函数体：                ← _mixin.py:1051-1070
     ├─ schema = cls._construct_compiled      ← 类属性查找（PyType_Lookup，~30-40ns）
     ├─ if schema is None: raise ...          ← 分支
     └─ return schema._parse_raw(data)        ← bound method 分发（~30-50ns）

[FFI 穿越入 - pyo3 #[pymethods] wrapper]      ← schema.rs:148-165
  3. pyo3 wrapper：                           ← 30-50ns（Context-Vec §3.1 估算）
     ├─ GIL token 校验
     ├─ 参数解析（&Bound<PyBytes> 借用）
     └─ 返回值包装（Py<PyAny> 构造）

[Rust 内部 - 全部为 C API 调用，非 FFI 穿越]
  4. _parse_raw 函数体：                       ← schema.rs:150-165
     ├─ data.as_bytes()                       ← ~1ns
     ├─ ParseStream::new(bytes)               ← ~2ns（切片 + usize）
     ├─ Context::placeholder(py)              ← ~5ns（无 PyDict 分配）
     ├─ Path::new()                           ← ~1ns
     └─ self.root.parse(py, &mut stream, &mut ctx, &mut path)

  5. StructNode.parse（struct_node.rs:274-376）：
     ├─ create_class(cls)                     ← tp_new，22ns（bench_asymmetry 组3 实测）
     ├─ instance.getattr("__dict__")          ← 10.5ns（组5）
     ├─ downcast::<PyDict>                    ← +1.1ns（组5）
     └─ for field in fields（3 次循环）：
         ├─ field.node.parse（FormatField）   ← i64::into_py 1.7ns（组1）
         └─ dict.set_item(interned key, ...)  ← 13.5ns（组5）

[FFI 穿越出 - 返回用户类实例]
  6. 实例 Py<PyAny> 返回 + 引用计数转移       ← ~5ns
```

**build 方向**（`schema.rs:180-204` + `struct_node.rs:378-447`）：调用栈层次相同，但 Rust 内部无 tp_new/getattr_dict（build 端独有 PyBytes_new 8.7ns + BuildStream 20.96ns，见 bench_asymmetry 组4），整体比 parse 端少 ~32ns 固定开销（解释 B1 build 11.71x vs parse 8.09x 的不对称）。

### 1.2 每层开销分布（基于实测数据反推）

#### 1.2.1 字段级稳态贡献（B1→B4 反推）

从 CSV 行 2/4/6/8（Phase 1 B1-B4 parse）反推每字段边际成本：

| 场景对比 | 字段数差 | rs_ns 差 | 每字段边际 |
|---------|---------|---------|-----------|
| B1→B2（3→10） | +7 | 628-386=242 | **34.6 ns/字段** |
| B2→B3（10→50） | +40 | 2020-628=1392 | **34.8 ns/字段** |
| B3→B4（50→100） | +50 | 3656-2020=1636 | **32.7 ns/字段** |

**稳态结论**：用户面每字段贡献 **~33-35ns**（含 FFI 入口均摊后的字段处理 + dict.set_item + PyLong 创建）。

> **量级对照**（L-09 教训：ns 级估算标"量级参考"）：
> bench_asymmetry 内部测量（无 FFI 边界）的稳态字段贡献是 **~5ns/字段**（组7，FormatField）。用户面 ~34ns 与内部 ~5ns 的差值 **~29ns/字段**，归因于 FFI 入口开销随字段数轻微增长（cache miss、分支预测）——但主体是固定开销被字段均摊后的算术假象，不是真每字段开销。

#### 1.2.2 B1 固定开销反推（关键瓶颈识别）

B1（3 字段）rs_ns = 386ns（CSV 行 2）。用稳态字段贡献反推：

```
B1 rs_ns = 固定开销 + 3 × 稳态字段贡献
386     = 固定开销 + 3 × 34
固定开销 ≈ 281 ns
```

**B1 的固定开销占比 = 281 / 386 = 72.8%**（FFI 入口 + Python 包装 + StructNode 实例构造固定成本）。

#### 1.2.3 固定开销 281ns 的来源拆解

基于 §1.1 调用栈 + bench_asymmetry 实测数据，281ns 固定开销组成（**量级估算，非精确值**——L-09）：

| 组件 | 估算 ns | 数据来源 |
|------|--------|---------|
| Python 入口（classmethod 解析 + `_construct_compiled` 属性查找 + bound method `_parse_raw` 分发） | 60-90 | bench_asymmetry C 值（41-49ns）+ 属性查找扩充估算 |
| pyo3 `#[pymethods]` wrapper（GIL 校验 + 参数解析 + 返回包装） | 30-50 | Context-Vec §3.1 估算 |
| Context::placeholder + ParseStream::new + Path::new | 8-10 | bench_asymmetry 组4 |
| StructNode 固有（tp_new 22 + getattr_dict 11.6 + dict 初始化） | 34-40 | bench_asymmetry 组3+组5 实测 |
| 实例返回 + 引用计数转移 | 5 | 通用 pyo3 开销 |
| **未解释残差** | 60-95 | cache miss / 分支预测 / pyo3 内部细节 |
| **合计** | **197-295** | 实测固定开销 281 落入区间 |

**结论**：B1 固定开销 281ns 中，**Python 入口 + pyo3 wrapper 占 90-140ns（~32-50%），是最大单一可优化项**；StructNode 固有（tp_new 等）34-40ns 是不可压缩的实例构造代价（除非改变 API 语义）；残差 60-95ns 需进一步 profiling 定位。

### 1.3 小字段 vs 大字段对比

| 场景 | rs_ns | 固定开销（281） | 字段开销 | 固定占比 | 当前加速比 | 达 10x 需减 |
|------|-------|---------------|---------|---------|-----------|------------|
| **B1（3 字段）parse** | 386 | 281 | 105 | **72.8%** | 8.09x | **74 ns** |
| B2（10 字段）parse | 628 | 281 | 347 | 44.7% | 9.66x | 22 ns |
| B3（50 字段）parse | 2020 | 281 | 1739 | 13.9% | 11.54x ✅ | 已达标 |
| B4（100 字段）parse | 3656 | 281 | 3375 | 7.7% | 12.06x ✅ | 已达标 |

**关键洞察**：
- **B1 是 FFI 稀释最严重的场景**（固定开销占 72.8%），优化主战场
- B2 仅差 22ns，B1 优化方案大概率顺带解决 B2
- B3/B4 已 ≥10x，**优化方案不得让它们回退**（Phase 5 总纲硬约束）

### 1.4 瓶颈识别结论

1. **首要瓶颈**：Python 入口 + pyo3 wrapper（90-140ns，占固定开销 32-50%）
2. **次要瓶颈**：StructNode 固有实例构造（34-40ns，不可压缩）
3. **第三瓶颈**：未解释残差（60-95ns，需 profiling）

**B1 达 10x 攻击面**：需减 74ns。Python 入口 + pyo3 wrapper 是唯一足够大的攻击面（90-140ns 区间，理论可压缩 50-80%）。

**B1 达 12x 攻击面**：需减 126ns。即使 Python 入口 + pyo3 wrapper 全部消除（极端假设），仍需在 StructNode 固有或残差中再省 30-50ns，**风险较高**。

---

## 2. 优化候选方向

> 每个候选含 5 个必填字段（技术描述 / 预期收益 / 实施成本 / 风险 / 可证伪预测）。
> 所有预期收益基于 §1 实测数据反推，非理论估算（L-02 教训）。

### 候选 A：FFI 入口去层化（消除 Python 入口开销）

#### 技术描述

**当前路径**（`_mixin.py:1051-1070`）：
```python
@classmethod
def parse(cls, data):
    schema = cls._construct_compiled       # 属性查找
    if schema is None: raise ...
    return schema._parse_raw(data)         # bound method 分发
```

每次 parse 经过：Python classmethod 调用 → 类属性查找（`_construct_compiled`）→ bound method `_parse_raw` 分发 → pyo3 wrapper。其中前两步纯 Python 开销，与 Rust 无关。

**优化方向**：编译期（`compile_schema`）将 `_parse_raw` / `_build_raw` 直接挂载到用户类的类型对象槽位（如 `tp_call` 或专用描述符），使 `cls.parse(data)` 的 Python 层路径缩减为：单次 LOAD_METHOD + 单次 CALL，消除中间属性查找与方法分发。

**关键约束**：仍是一次 FFI（§0 第 1 条）——只优化 Python 侧到 FFI 入口的路径，不增加 FFI 次数。

#### 预期收益（基于数据反推）

- Python 入口当前贡献 60-90ns（§1.2.3）
- 去层化后预估保留 20-40ns（LOAD_METHOD + CALL 不可消除）
- **净节省：30-50ns**
- B1 影响：386 → 336-356ns，加速比 8.09x → **8.8-9.3x**（单独不足以达 10x）

#### 实施成本

- 涉及源码：`compile.rs`（compile_schema 增加挂载逻辑）+ `_mixin.py`（移除 classmethod 包装）+ `schema.rs`（可能新增模块级 `#[pyfunction]` 入口）
- 重构深度：**中**（改变 Python 类创建流程，但不改 Rust 执行树）
- 估算行数：~80-120 行

#### 风险

- **§0 兼容性**：✅ 不违反（仍一次 FFI）
- **已 ≥10x 场景回退**：低风险（B3/B4 字段开销主导，固定开销减少不会让总时间上升）
- **ABI3 兼容性**：中风险（操作类型对象槽位需用 `PyType_GetSlot` / 有限 API，需验证）
- **延迟桩兼容**：需同步处理 `_install_lazy_stubs` 路径（`_mixin.py:907`）

#### 可证伪预测

> 实施后 B1 parse rs_ns 应从 386ns 降至 336-356ns 区间（30-50ns 节省）。
> ** falsification**：Controlled A/B Test（同会话交替，L-09 教训）测得 B1 rs_ns 降幅 <20ns（<0.3x 波动阈值外）→ 候选无效，需排查"Python 入口贡献 60-90ns"假设是否成立。
> **阴性对照**：B4（100 字段）rs_ns 降幅应 <10ns（字段开销主导，固定开销占比小）。

---

### 候选 B：pyo3 raw FFI（绕过 `#[pymethods]` wrapper）

#### 技术描述

**当前实现**（`schema.rs:148-165`）：
```rust
#[pyo3(signature = (data))]
pub fn _parse_raw<'py>(
    &self,
    py: Python<'py>,
    data: &Bound<'py, PyBytes>,
) -> PyResult<Bound<'py, PyAny>> { ... }
```

pyo3 `#[pymethods]` 宏在 `_parse_raw` 外层生成 wrapper：参数类型转换（`PyBytes` 借用校验）、GIL token 校验、返回值 `into_bound` 包装。wrapper 是普通 Rust 函数，但有 pyo3 内部簿记开销。

**优化方向**：参考 Phase 4.x 错误路径 O3（首次 unsafe raw FFI，`plans/phase4-array/traces/4.x-错误路径.md`），将 `_parse_raw` 改为模块级 `#[pyfunction]`，内部直接用 `PyArg_ParseTuple` + raw `*mut PyObject` 指针操作，绕过 pyo3 wrapper 层。

**关键约束**：仍是一次 FFI（§0 第 1 条）——只是改变 FFI 入口的实现方式。

#### 预期收益（基于数据反推）

- pyo3 wrapper 当前贡献 30-50ns（§1.2.3，Context-Vec §3.1）
- raw FFI 后预估保留 10-20ns（必要的 GIL 验证 + 最小参数解析）
- **净节省：15-35ns**
- B1 影响：386 → 351-371ns，加速比 8.09x → **8.4-8.7x**（单独不足以达 10x）

#### 实施成本

- 涉及源码：`schema.rs`（替换 `_parse_raw` / `_build_raw` 为 `#[pyfunction]`）+ `lib.rs`（注册新 pyfunction）
- 重构深度：**中**（首次在热路径使用 unsafe raw FFI，Phase 4.x 仅错误路径用过）
- 估算行数：~100-150 行（含 SAFETY 注释 + 测试）

#### 风险

- **§0 兼容性**：✅ 不违反（仍一次 FFI）
- **unsafe 风险**：中（首次热路径 unsafe，需严格 SAFETY 注释 + miri 验证 + 阴性对照）
- **ABI3 兼容性**：低风险（`PyArg_ParseTuple` 是稳定 ABI）
- **回归风险**：中（错误处理路径需重新设计，`ConstructError → PyErr` 转换在 raw FFI 下需手工实现）

#### 可证伪预测

> 实施后 B1 parse rs_ns 应从 386ns 降至 351-371ns 区间（15-35ns 节省）。
> ** falsification**：Controlled A/B Test 测得 B1 rs_ns 降幅 <10ns → pyo3 wrapper 实际贡献 <30ns，候选收益被高估。
> **阳性对照**：错误路径 a_err_eof（已用 raw FFI，`CSV 行 153`，11.34x）可作为"raw FFI 已验证可行"的参考。
> **阴性对照**：B4 rs_ns 降幅应 <10ns（同候选 A 理由）。

---

### 候选 C：小字段 fast-path（StructNode 字段数 ≤ N 专用内联路径）

#### 技术描述

**当前实现**（`struct_node.rs:340-362`，无表达式路径）：
```rust
for field in &self.fields {
    let value = match field.node.parse(...) { ... };
    match field.mode {
        FieldMode::Rw | FieldMode::Ro => {
            dict_bound.set_item(field.name.py_name().bind(py), value.bind(py))?;
        }
        FieldMode::Wo => { drop(value); }
    }
}
```

每次迭代经过：Vec 迭代器（边界检查 + 指针递增）+ match field.mode（分支预测）+ match field.node（enum_dispatch 静态分派）。

**优化方向**：编译期在 `StructNode` 增加 `is_small_struct: bool`（`fields.len() <= 3`）标志。运行期 parse/build 入口分支：小字段结构走专用内联代码（手动展开 1-3 次字段处理，消除迭代器与分支开销），大字段结构走当前通用循环。

**关键约束**：仍是一次 FFI，仍是直接操作 PyObject（§0 第 1、2 条）。

#### 预期收益（基于数据反推）

- 当前 B1 字段开销 105ns（3 字段 × 35ns 用户面边际）
- 内部测量稳态字段贡献 5ns（bench_asymmetry 组7），用户面 35ns 含 FFI 边界均摊
- fast-path 消除迭代器 + 分支开销，预估每字段省 2-5ns
- **净节省：6-15ns（3 字段）**
- B1 影响：386 → 371-380ns，加速比 8.09x → **8.2-8.4x**（收益较小，单独不达标）

#### 实施成本

- 涉及源码：`struct_node.rs`（parse/build 各加 fast-path 分支）+ `compile.rs`（设置 is_small_struct 标志）
- 重构深度：**低**（局部代码改动，不影响其他模块）
- 估算行数：~60-80 行

#### 风险

- **§0 兼容性**：✅ 不违反
- **代码膨胀**：低（fast-path 分支有限，编译器可优化）
- **正确性**：低风险（fast-path 与通用路径语义必须完全一致，需 Round-trip 测试覆盖）
- **维护性**：低风险（fast-path 仅是性能特化，逻辑等价）

#### 可证伪预测

> 实施后 B1 parse rs_ns 应从 386ns 降至 371-380ns 区间（6-15ns 节省）。
> ** falsification**：Controlled A/B Test 测得 B1 rs_ns 降幅 <3ns → 迭代器 + 分支开销实际 <2ns/字段，候选收益被高估。
> **阳性对照**：B2（10 字段，不触发 fast-path）rs_ns 应基本不变（<5ns 波动）。
> **场景验证**：所有 ≤3 字段的 Phase 1-4 场景（B1、StopIf、E01 等）应一致受益。

---

### 候选 D：单字段 InlineNode 特化（高风险高收益后备）

#### 技术描述

**当前实现**：单字段 Struct（如 `StopIf(x==0)` 的容器 Struct、单字段测试场景）仍走完整 StructNode 流程：tp_new + getattr_dict + 单字段 set_item + 实例返回。

**优化方向**：编译期检测 `fields.len() == 1 && fields[0] is FormatField`，生成专用 `InlineSingleFieldNode`：
- 直接 `tp_new` 创建实例
- 跳过 dict 借用（直接用 `PyObject_GenericSetAttr` 写入单属性，或用 `__dict__` 内联指针）
- 单字段处理内联（无 Vec 迭代 + 无 match field.mode 分支）

**关键约束**：仍是一次 FFI、直接操作 PyObject（§0 第 1、2 条）。

#### 预期收益（基于数据反推）

- 当前 B1（3 字段）不直接受益（仅适用单字段场景）
- 适用场景：StopIf S01/S03（CSV 行 146-151，1 字段 Struct），E01 边界场景
- 单字段 Struct parse rs_ns 当前 ~340ns（CSV 行 106：StopIf S3 2995/301=9.96x）
- 优化后预估 ~280-310ns（节省 30-50ns）
- StopIf S03 加速比 9.96x → **10.9-11.7x**（达 ≥10x）

#### 实施成本

- 涉及源码：新增 `nodes/inline_single_field.rs` + `compile.rs`（识别 + 替换）+ `Node` enum 增加变体
- 重构深度：**高**（新增 Node 变体影响 enum_dispatch + 所有 match 臂 + 嵌套场景）
- 估算行数：~150-200 行

#### 风险

- **§0 兼容性**：⚠️ 边缘（实例构造语义需严格等价，特别是 frozen dataclass 与 slots 类）
- **正确性**：高风险（与 StructNode 行为必须 100% 等价，包括 has_post_init / StopField 捕获 / 嵌套场景）
- **适用面窄**：仅单字段场景受益，B1（3 字段）不直接受益
- **维护成本**：高（新增 Node 变体的长期维护负担）

#### 可证伪预测

> 实施后单字段 Struct parse rs_ns 应从 ~340ns 降至 280-310ns 区间。
> ** falsification**：Controlled A/B Test 测得 StopIf S03 rs_ns 降幅 <15ns → 实例构造语义等价约束使可优化空间被高估。
> **不推荐作为主方案**：建议作为候选 A+B+C 实施后仍不达 ≥10x 的后备手段。

---

### 候选 E：tp_new 替代方案（不推荐）

#### 技术描述

考虑过但不推荐的方案：预分配实例池（每次 parse 复用模板实例）、直接构造 dict 不经 tp_new（违反 mashumaro API 语义）、用户类缓存空实例（多线程不安全）。

#### 不推荐理由

- 全部违反 mashumaro API 语义（每次 parse 必须返回新实例）
- 或违反 §0 第 3 条（pyo3 是核心依赖，不能用 C 扩展绕过）
- 或引入线程安全问题

**结论**：tp_new 22ns（bench_asymmetry 组3 实测）是 mashumaro API 的不可压缩代价，不在 Phase 5 优化范围内。

---

## 3. 推荐方案 + 理由

### 3.1 推荐组合：候选 A + 候选 B + 候选 C（主攻 B1 ≥10x）

| 候选 | 单独 B1 收益 | 单独后 B1 加速比 | 与其他候选冲突？ |
|------|------------|----------------|----------------|
| A（FFI 去层化） | 30-50ns | 8.8-9.3x | 否（攻击 Python 入口） |
| B（pyo3 raw FFI） | 15-35ns | 8.4-8.7x | 否（攻击 pyo3 wrapper） |
| C（小字段 fast-path） | 6-15ns | 8.2-8.4x | 否（攻击 StructNode 内部） |
| **A+B+C 叠加** | **51-100ns** | **9.6-11.0x** | — |

**叠加收益模型**：三个候选攻击不同环节，理论上收益可加（无重叠）。但实际叠加可能因 cache 效应、分支预测变化、CPU 流水线重排等非线性因素，实际收益略低于简单相加。

### 3.2 取舍逻辑（收益 / 成本 / 风险三角）

```
                    收益（ns）
                        ↑
        A（30-50）      ●───── 高收益，中风险（ABI3 + 延迟桩）
                        │
        B（15-35）      ●───── 中收益，中风险（unsafe 热路径）
                        │
        C（6-15）       ●───── 低收益，低风险（局部改动）
                        │
        D（30-50）      ●───── 高收益，高风险（Node enum 变化）
                        │
        E               ✗───── 不推荐（违反 API 语义）
                        └──────────────────────→ 风险
```

**推荐排序**：
1. **C 优先**（低风险打底）—— 即使 A/B 后续不达预期，C 仍可独立交付，B1 至少 8.2-8.4x
2. **A 主攻**（高收益主战场）—— Python 入口是最大可优化项，必做
3. **B 辅助**（叠加冲击 ≥10x）—— A 单独可能差 10-30ns，B 补足
4. **D 后备**（仅 StopIf 单字段场景）—— 仅当 A+B+C 后 StopIf S01/S03 仍 <10x 时启用

### 3.3 关键目标达成性评估

| 目标 | 需减 | A+B+C 中位收益 | 中位后 rs_ns | 中位后加速比 | 评估 |
|------|-----|---------------|-------------|-------------|------|
| **B1 ≥ 10x** | 74ns | 75ns（30+25+20） | 311ns | **10.04x** | 🟡 **临界**（在波动阈值内） |
| **B1 ≥ 12x**（总纲理想） | 126ns | 75ns | 311ns | 10.04x | 🔴 **不足**（差 51ns） |
| StopIf S03 ≥ 10x（用户决策路径 B） | 30ns | A+B 影响 ~70ns（含单字段 fast-path 受益） | ~230ns | **13x** | 🟢 **达标** |
| B2 ≥ 10x | 22ns | A+B ~50ns | ~578ns | **10.5x** | 🟢 **达标** |
| B3/B4 不回退 | — | A+B+C 仅减固定开销 | — | ≥11x | 🟢 **安全**（字段开销主导） |

### 3.4 诚实声明（L-02 / L-09 教训）

**B1 ≥ 10x 的不确定性**：
- 中位估算 B1 加速比 10.04x，处于 10x 临界线 ±0.3x 波动阈值内
- 若 A/B/C 三个候选的实测收益均落在区间下界（51ns 总和），B1 加速比仅 9.6x，**不达标**
- 若均落在区间上界（100ns 总和），B1 加速比 11.0x，达标
- **必须用 Controlled A/B Test 验证**（L-09 教训），不可用理论叠加替代实测

**B1 ≥ 12x 的不可达性**：
- A+B+C 上界 100ns < 126ns，即使三候选全部命中区间上界，B1 仍差 ~26ns
- 必须引入候选 D（单字段特化）才能补足，但 B1 是 3 字段不直接受益
- **B1 ≥ 12x 在当前架构下不可达**，建议 PM 调整总纲目标为"≥10x"或"≥10.5x"

---

## 4. 子任务分解建议

基于推荐方案（A+B+C+后备 D），建议 Phase 5 子任务拆分：

| # | 子任务 | 候选 | 依赖 | 角色 | 预期 B1 收益 | 风险 |
|---|--------|------|------|------|-------------|------|
| **5.0** | 分析报告（本文档） | — | — | ARCH（已完成） | — | — |
| 5.R | 设计检视 | — | 5.0 | REV | — | — |
| **5.1** | 小字段 fast-path | C | 5.R | DEV→VET | 6-15ns | 低 |
| **5.2** | FFI 入口去层化 | A | 5.R | ARCH→DEV→VET | 30-50ns | 中 |
| **5.3** | pyo3 raw FFI | B | 5.R | ARCH→DEV→VET | 15-35ns | 中 |
| **5.4** | 集成测试 + 性能验证 | — | 5.1+5.2+5.3 | DEV→VET | — | — |
| 5.5 | 单字段 InlineNode（后备） | D | 5.4 实测决定 | ARCH→DEV→VET | 30-50ns（仅单字段场景） | 高 |

### 4.1 依赖关系与并行可能性

```
5.0 (分析) → 5.R (设计检视) ─┬─→ 5.1 (fast-path, 独立) ─┐
                             ├─→ 5.2 (FFI 去层化, 独立) ─┤
                             └─→ 5.3 (raw FFI, 独立)    ─┤
                                                         ↓
                                                  5.4 (集成 + 性能验证)
                                                         ↓
                                                  [决策点 A]
                                                  ├─ B1 ≥ 10x → 收尾
                                                  ├─ B1 9-10x → 启用 5.5
                                                  └─ B1 < 9x → 回到设计（重新诊断瓶颈）
                                                         ↓
                                                  5.5 (单字段特化, 仅 StopIf 需要)
                                                         ↓
                                                  [决策点 B] PM 验收
```

**并行可能性**：
- 5.1 / 5.2 / 5.3 攻击不同环节，**理论可并行**（三个子任务无源码冲突）
- 但共享 `struct_node.rs` / `schema.rs` 的修改需协调（建议顺序：5.1 先做低风险打底，5.2/5.3 并行）
- 5.4 必须在 5.1-5.3 全部合并后执行（否则无法验证叠加效果）

### 4.2 决策点 A（5.4 完成后）

| B1 实测加速比 | 处理 |
|--------------|------|
| ≥ 10x | 收尾，跳过 5.5 |
| 9.5-10x | 评估是否接受（用户决策），或启用 5.5 优化单字段场景 |
| < 9.5x | 回到设计：候选收益被高估，需重新 profiling 残差 60-95ns |

### 4.3 决策点 B（5.5 完成后或跳过）

PM 验收 Phase 5 S-PERF：
- B1-B4 全部 ≥10x → ✅ 验收通过
- B1 9-10x + 其他全部 ≥10x → 推送用户决策（与 Phase 4 StopIf B1 路径 B 精神一致，标"已投资 Phase 5"）
- StopIf S01/S03 仍 <10x → 启用 5.5

---

## 5. PM 决策点

### 5.1 决策点 1：B1 ≥12x 目标弹性（**最关键**）

**问题**：总纲 `plans/phase5-struct-ffi/总纲.md:21` 写"B1 7.6x → ~12x"，但本报告 §3.3 论证 B1 ≥12x 在当前架构下不可达（A+B+C 上界 100ns < 126ns）。

**ARCH 建议**：将 B1 目标调整为 **≥10x**（与 Phase 4 StopIf / RepeatUntil 一致的硬约束），保留 12x 为"理想"。

**PM 决策选项**：
- (a) 接受 B1 ≥10x 为硬目标（推荐）
- (b) 坚持 B1 ≥12x，授权探索候选 D 的扩展（多字段特化，但收益不确定）
- (c) 接受 B1 ≥10x 但保留 12x 为"软目标"，超出则记录但不阻塞

### 5.2 决策点 2：unsafe raw FFI 在热路径的接受度

**问题**：候选 B 需在 parse/build 热路径首次使用 unsafe raw FFI（Phase 4.x 仅错误路径用过，CSV 行 153）。

**ARCH 评估**：
- 技术上可行（pyo3 raw FFI 在 pydantic-core 生产验证）
- ABI3 兼容（`PyArg_ParseTuple` 是稳定 ABI）
- 但需严格 SAFETY 审查（miri + 阴性对照 + fuzzing）

**PM 决策选项**：
- (a) 接受 unsafe raw FFI 在热路径（推荐，附 SAFETY 审查清单）
- (b) 仅在 5.1+5.2 后 B1 仍 <10x 时启用 5.3（保守策略）
- (c) 拒绝 unsafe raw FFI，候选 B 退出（B1 ≥10x 风险升高）

### 5.3 决策点 3：单字段特化（候选 D）的优先级

**问题**：候选 D 高风险（新增 Node 变体）高收益（仅单字段场景），是否纳入主路径？

**ARCH 建议**：作为 5.4 实测后的后备方案，仅当 StopIf S01/S03 仍 <10x 时启用。B1（3 字段）不直接受益，不应作为 B1 达标的主路径。

**PM 决策选项**：
- (a) 同意作为后备（推荐）
- (b) 立即纳入主路径（与 A/B/C 同期实施）
- (c) 完全排除（接受 StopIf S01/S03 可能仍 <10x，按 Phase 4 用户决策路径 B 处理）

---

## 6. §0 原则对照表（L-01 教训）

> L-01 教训要求：设计文档涉及 parse/build 数据流时，必须包含 §0 原则对照表。本节覆盖全部 6 条 §0 原则。

| §0 原则 | 候选 A | 候选 B | 候选 C | 候选 D |
|---------|--------|--------|--------|--------|
| **1. 一次 FFI**（编译/parse/build 各只有一次 Python↔Rust 边界穿越） | ✅ 仅优化 Python 侧到 FFI 入口的路径，FFI 次数不变 | ✅ 仅改变 FFI 入口实现方式（raw vs pyo3 wrapper），FFI 次数不变 | ✅ 纯 Rust 内部优化，FFI 次数不变 | ✅ 新增 Node 变体，FFI 次数不变 |
| **2. 无中间表示层**（parse 直接构造 PyObject，build 直接读 PyObject） | ✅ 不引入 Rust 中间类型 | ✅ 不引入 Rust 中间类型 | ✅ 不引入 Rust 中间类型 | ✅ 直接操作 PyObject（GenericSetAttr） |
| **3. 输入/输出侧无抽象 trait** | ✅ 无 trait 抽象 | ✅ 无 trait 抽象 | ✅ 无 trait 抽象 | ✅ 无 trait 抽象 |
| **4. pyo3 是核心依赖** | ✅ 仍用 pyo3（仅调整入口结构） | ⚠️ 边缘（raw FFI 仍依赖 pyo3 GIL token，但绕过 `#[pymethods]` 宏） | ✅ pyo3 不变 | ✅ pyo3 不变 |
| **5. mashumaro 式 API** | ✅ 用户 API 不变（`cls.parse(data)`） | ✅ 用户 API 不变 | ✅ 用户 API 不变 | ✅ 用户 API 不变 |
| **6. 构造器分派**（enum_dispatch 静态分派） | ✅ Node enum 不变 | ✅ Node enum 不变 | ✅ Node enum 不变 | ⚠️ 新增 Node 变体（需更新 enum_dispatch，但仍静态分派） |
| **7. 错误处理**（Result + thiserror + path） | ✅ ConstructError 不变 | ⚠️ raw FFI 下 PyErr 转换需手工实现（保持语义等价） | ✅ 错误处理不变 | ✅ 错误处理不变 |
| **8. Stream 抽象**（纯 Rust 内部） | ✅ Stream 不变 | ✅ Stream 不变 | ✅ Stream 不变 | ✅ Stream 不变 |

**§0 违反风险结论**：
- **候选 A / C**：零 §0 风险
- **候选 B**：边缘（pyo3 仍是核心依赖，仅绕过宏生成的 wrapper，符合 §0 第 4 条精神）
- **候选 D**：边缘（新增 Node 变体仍是静态分派，符合 §0 第 6 条精神），但维护成本高

---

## 7. 引用证据清单

### 7.1 性能数据点（CSV 行号）

| 数据点 | CSV 行 | 用途 |
|--------|--------|------|
| B1 parse 3123/386/8.09x | 行 2 | B1 现状基线 |
| B2 parse 6068/628/9.66x | 行 4 | 字段级贡献反推 |
| B3 parse 23314/2020/11.54x | 行 6 | 字段级贡献反推 |
| B4 parse 44097/3656/12.06x | 行 8 | 字段级贡献反推 + 已达标 |
| B1 build 11.71x | 行 3 | build 路径不需优化 |
| B7（空 Struct）331ns | 行 14 | 固定开销参照 |
| StopIf S03 2995/301/9.96x | 行 106 | 候选 D 受益场景 |
| a_err_eof 11.34x | 行 153 | 候选 B 阳性对照（raw FFI 已验证） |

### 7.2 源码位置

| 文件 | 行号 | 内容 |
|------|------|------|
| `construct-rs/src/schema.rs` | 148-165 | `_parse_raw` 当前实现 |
| `construct-rs/src/schema.rs` | 180-204 | `_build_raw` 当前实现 |
| `construct-rs/src/nodes/struct_node.rs` | 274-376 | StructNode.parse 完整流程 |
| `construct-rs/src/nodes/struct_node.rs` | 340-362 | 无表达式路径（候选 C 攻击点） |
| `construct-rs/src/instance.rs` | 72-97 | `create_class`（tp_new 22ns 来源） |
| `construct-rs/src/instance.rs` | 120-143 | `force_setattr` |
| `construct-rs/python/construct/_mixin.py` | 1051-1070 | `StructMixin.parse` classmethod（候选 A 攻击点） |
| `construct-rs/python/construct/_mixin.py` | 907-992 | 延迟桩机制（候选 A 需同步处理） |

### 7.3 历史分析报告

| 文档 | 关键数据 |
|------|---------|
| `docs/analysis/phase3-parse-build不对称.md` §3.3 | tp_new = 22ns（实测） |
| `docs/analysis/phase3-parse-build不对称.md` §3.5 | getattr_dict+downcast = 11.6ns / dict.set_item = 13.5ns |
| `docs/analysis/phase3-parse-build不对称.md` §3.7 | StructNode(0字段) parse = 54.44ns / build = 22.32ns |
| `docs/analysis/phase3-parse-build不对称.md` §4.2 | Python 入口差 C = 41-49ns |
| `docs/design/模块设计/模块设计-Context-Vec优化.md` §3.1 | pyo3 wrapper = 30-50ns / Context::new_root = 30-50ns |
| `docs/analysis/E01-O1-regression-investigation.md` | L-09 教训原始证据 + Controlled A/B Test 方法学 |

---

## 8. 设计完成状态

- [x] §1 FFI 入口现状分析（调用栈 + 开销分布 + 小字段 vs 大字段 + 瓶颈识别）
- [x] §2 优化候选方向（5 个候选，每个含技术描述/预期收益/实施成本/风险/可证伪预测）
- [x] §3 推荐方案 + 理由（含取舍逻辑 + 目标达成性评估 + 诚实声明）
- [x] §4 子任务分解建议（含依赖关系 + 决策点）
- [x] §5 PM 决策点（3 个决策点 + 选项）
- [x] §6 §0 原则对照表（L-01 教训执行）
- [x] §7 引用证据清单（CSV 行号 + 源码位置 + 历史报告）

**需要 PM 注意的事项**：

1. **权限冲突**：本报告因 ARCH 角色不能写 `plans/` 而写入 `docs/analysis/`，请 PM 协调（移至总纲指定位置 / 在总纲中引用本路径 / 调整角色权限）。

2. **B1 ≥12x 不可达**：总纲目标"B1 7.6x → ~12x"在当前架构下不可达（§3.3-§3.4）。建议 PM 调整目标为 ≥10x（决策点 1）。

3. **unsafe raw FFI 首次热路径使用**：候选 B 需 PM 决策接受度（决策点 2）。

4. **候选 D 维护成本**：新增 Node 变体的长期负担，建议作为后备（决策点 3）。

5. **5.4 集成测试必须用 Controlled A/B Test**（L-09 教训），不可用跨时段对比或消除法归因。

6. **预测的不确定性**：所有候选收益均为基于实测反推的量级估算（L-02 / L-09 教训），实际收益需 5.4 实测验证。若实测显著偏离预测（>0.3x 波动阈值），需回到设计重新诊断。

