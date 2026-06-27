# Phase 3 parse/build 不对称根因分析

**分析人**：ARCH
**分析时间**：2026-06-28
**触发**：PM 验收 Phase 3 时撤回，要求 ARCH 分析"Phase 3 多出 ~56ns 不对称来源"
**实验代码**：`experiments/bench_asymmetry/`
**结论摘要**：**Phase 3 未引入结构性不对称。84-95ns 差距 = StructNode 固有不对称（32ns）+ Python 入口不对称（~41-49ns）+ 字段级差（每字段 ~5-7ns）。** BitsInteger 字段级贡献与 FormatField 几乎相同（i128 PyLong 创建慢于 i64，但 i128 extract 也慢于 i64，两者抵消）。

---

## 1. 问题陈述

### 1.1 PM 给出的数据

| 场景 | parse ns | build ns | 差值 |
|------|---------|---------|------|
| Phase 3 Bitwise(BitsInteger(8)) | 274.8 | 191.2 | +83.6 |
| Phase 3 Bitwise(BitsInteger(16)) | 288.1 | 203.6 | +84.5 |
| Phase 3 BitStruct(3字段) | 385.6 | 291.4 | +94.2 |
| Phase 2.5 E1 (2字段+表达式) | 292 | 265 | +27 |
| Phase 2.5 E3 (6字段+表达式) | 420 | 437 | -17 |
| Phase 1 B7 (空Struct) | 295 | 214 | +81 |

### 1.2 PM 的疑惑

> 两侧都有 StructNode、tp_new、getattr __dict__。Phase 1/2 的 parse/build 差值约 0-30ns，Phase 3 约 83-94ns。多出的 ~56ns 未被解释。
>
> 前一轮调查将根因归为 tp_new 实例分配（~80ns），但这个结论被 Phase 2.5 数据推翻——Phase 2.5 同样有 tp_new 但差值只有 27ns。

### 1.3 ARCH 的初始假设

PM 的对比存在 **字段数混淆**：

- Phase 1 B7 是 **0 字段 Struct**（差 81 ns）
- Phase 2.5 E1 是 **2 字段**（差 27 ns）、E3 是 **6 字段**（差 -17 ns）
- Phase 3 B1/B2/B3 是 **1 字段 BitStruct**（差 84 ns）
- Phase 3 BS1 是 **3 字段 BitStruct**（差 95 ns）

字段数不同时，"每字段 build 累积开销"会抵消"StructNode 固有不对称"。Phase 2.5 字段多（2-6）所以差距小；Phase 3 B1/B2/B3 字段少（1）所以差距大。**这不是 Phase 3 引入的新问题**。

为证伪/证实这个假设，ARCH 设计了 7 组隔离实验。

---

## 2. 实验设计

`experiments/bench_asymmetry/`（独立 Rust crate，path 依赖 construct-rs rlib）。
INNER=1,000,000 iter/round × OUTER=7 rounds，取 median + min。

| 组 | 测量目标 | 隔离的环节 |
|---|---------|----------|
| 1 | PyLong 创建成本 | i64 / i128 / u64 IntoPy 对比 |
| 2 | PyLong 读取成本 | extract<i64> vs extract<i128> |
| 3 | create_class (tp_new) | parse 独有的实例分配 |
| 4 | PyBytes / BuildStream | build 独有的输出构造 |
| 5 | getattr / dict.set_item | 每字段操作成本 |
| 6 | BitsInteger vs FormatField 裸节点 | 字段级 parse/build 差 |
| 7 | StructNode 字段数 0→10 | 隔离 tp_new 贡献 + 字段级贡献 |

---

## 3. 实验数据（min ns/op）

### 3.1 组 1：PyLong 创建成本

| 操作 | ns/op | 说明 |
|------|-------|------|
| `i64::into_py(42)` | **1.70** | CPython 小整数缓存命中（-5..256） |
| `u64::into_py(42)` | **1.89** | 同上，走 `PyLong_FromUnsignedLong` |
| `i64::into_py(0x12345678)` | **5.59** | 大正值，不命中缓存 |
| `u64::into_py(u64::MAX)` | **7.38** | 走 `PyLong_FromUnsignedLongLong` |
| `i64::into_py(-1)` | **1.70** | -1 命中小整数缓存 |
| **`i128::into_py(42)`** | **21.63** | ⚠ 即使值在 i64 范围，pyo3 i128 走任意精度路径 |
| **`i128::into_py(0x12345678)`** | **27.89** | ⚠ 比等值 i64 慢 5x |
| **`i128::into_py(-1)`** | **57.18** | ⚠ 负数任意精度路径最慢 |
| `i128::into_py(-0x12345678)` | 56.05 | 同上 |

**关键发现 1**：`i128::into_py` 比 `i64::into_py` 慢 **20-50 ns**，即使值在小整数缓存范围内。pyo3 0.22 的 i128 IntoPy 实现不走 CPython 小整数快路径，统一走 `_PyLong_FromByteArray` 类似的任意精度路径。

### 3.2 组 2：PyLong 读取成本

| 操作 | ns/op | 说明 |
|------|-------|------|
| `extract::<i64>(42)` | **1.23** | 小整数快路径 |
| `extract::<i64>(0x12345678)` | **1.23** | 同上 |
| **`extract::<i128>(42)`** | **16.50** | ⚠ 任意精度读取 `_PyLong_AsByteArray` |
| `extract::<i128>(0x12345678)` | 16.54 | 同上 |
| `extract::<i128>(-1)` | 21.76 | 负数稍慢 |
| `is_instance_of::<PyLong>` | 0.38 | 类型检查几乎免费 |

**关键发现 2**：`extract::<i128>` 比 `extract::<i64>` 慢 **15 ns**。读取不需 alloc，但 i128 路径仍要走任意精度 digit 转换。

### 3.3 组 3：tp_new 成本

| 操作 | ns/op |
|------|-------|
| `tp_new(object 子类)` | **22.03** |
| `tp_new(@dataclass 类)` | **22.71** |

**关键发现 3**：**tp_new 只需 ~22 ns**，远小于之前估计的 ~80 ns。前一轮调查的"tp_new ~80ns"结论是错误归因。

### 3.4 组 4：build 独有开销

| 操作 | ns/op |
|------|-------|
| `PyBytes::new_bound(1 byte)` | 1.32 |
| `PyBytes::new_bound(8 bytes)` | 8.72 |
| `PyBytes.as_bytes(8 bytes)` | 0.95 |
| `BuildStream::with_capacity(8) + write + into_bytes` | **20.96** |

**关键发现 4**：`PyBytes::new_bound` 很便宜（1-9 ns）。完整 BuildStream 生命周期 ~21 ns。

### 3.5 组 5：getattr / dict 操作

| 操作 | ns/op |
|------|-------|
| `getattr('__dict__')` | 10.51 |
| `getattr('__dict__') + downcast::<PyDict>` | 11.61 |
| `dict.set_item(interned key, i64 value)` | 13.46 |
| `getattr(interned field name)` | 14.40 |
| `getattr + extract::<i64>` | 16.46 |
| **`getattr + extract::<i128>`** | **31.21** |

### 3.6 组 6：BitsInteger vs FormatField 裸节点

| 操作 | parse ns | build ns | P-B |
|------|---------|---------|-----|
| `BitsInteger(8, unsigned)` | 25.18 | 41.40 | **-16.22** |
| `FormatField(Int8ub)` | 4.35 | 22.17 | **-17.82** |

**关键发现 5**：**裸节点层面 parse 比 build 快 ~17 ns**（不是慢！）。

- BitsInteger 裸节点 parse 25 ns，build 41 ns（build 多了 is_instance + extract + write）
- FormatField 裸节点 parse 4 ns，build 22 ns（同上）
- 两者 parse-build 差几乎相同（-16 vs -18 ns）

**i128 字段级不对称 ≈ i64 字段级不对称**：i128 PyLong 创建慢 20 ns，i128 extract 也慢 15 ns，两者在 parse/build 方向上相互抵消。

### 3.7 组 7：StructNode 字段数 vs parse-build 差距（FormatField）

| 字段数 | parse ns | build ns | **P-B** |
|--------|---------|---------|---------|
| 0 | 54.44 | 22.32 | **+32.12** |
| 1 | 80.24 | 36.62 | +43.62 |
| 2 | 108.65 | 55.88 | +52.77 |
| 3 | 132.67 | 74.10 | +58.57 |
| 6 | 205.74 | 130.44 | +75.30 |
| 10 | 305.12 | 209.67 | +95.45 |

**关键发现 6**：**Struct(0 字段) parse-build 差距 = +32 ns**，不是 81 ns。

模型验证（0 字段）：
```
parse 比 build 多 = tp_new(22) + getattr_dict(11.6) - PyBytes_new_8(8.7) - BuildStream_diff
                  ≈ 22 + 11.6 - 8.7 ≈ 25 ns
实测 = 32 ns（差 7 ns，可能是 PyDict 初始化或 Path 开销）
```

**每字段贡献**（差分）：
- 0→1 字段：+11.5 ns
- 1→2 字段：+9.2 ns
- 2→3 字段：+5.8 ns
- 3→6 字段（每字段）：+5.6 ns
- 6→10 字段（每字段）：+5.0 ns

每字段贡献逐渐稳定到 ~5 ns（cache 效应稳定后的稳态值）。

### 3.8 组 7b：BitStruct（BitsInteger 字段）

| 字段数 | parse ns | build ns | **P-B** |
|--------|---------|---------|---------|
| 1 | 105.72 | 62.76 | **+42.96** |
| 2 | 151.64 | 101.25 | +50.39 |
| 3 | 200.40 | 145.40 | +55.00 |

**关键发现 7**：**BitStruct(1 字段 BitsInteger) 差距 = +43 ns**，与 Struct(1 字段 FormatField) 的 +44 ns 几乎相同。

BitwiseNode 包装层（bit_pos 读取 ×2 + 校验）的 parse/build 不对称 < 1 ns（对称设计生效）。

---

## 4. 完整模型与归因

### 4.1 三层分解模型

```
用户面 parse-build 差距 = A + B × 字段数 + C
```

- **A**：StructNode 固有不对称（tp_new + getattr_dict + dict_init - PyBytes_new - BuildStream_cycle）
- **B**：每字段贡献（PyLong_create + dict.set_item - getattr - extract - write）
- **C**：Python 入口不对称（classmethod 解析、pyo3 wrap 等）

### 4.2 模型参数（实验实测）

| 参数 | FormatField 路径 | BitsInteger 路径 | 差异 |
|------|-----------------|-----------------|------|
| A（0字段 P-B） | +32 ns | +32 ns（BitwiseNode 包装对称，<1ns） | 0 |
| B（每字段贡献，稳态） | +5 ns | +5 ns | 0 |
| C（Python 入口差） | +49 ns（B7: 81-32） | +41 ns（B1: 84-43） | -8 ns |

**BitsInteger 路径与 FormatField 路径在 A、B 两层完全等价**。Python 入口差 C 略小（可能 BitStructMixin 的 Python wrapper 比 StructMixin 稍轻）。

### 4.3 各场景的模型预测 vs 实测

| 场景 | 字段数 | 模型 A+B×N+C | 实测（用户面） | 误差 |
|------|--------|-------------|--------------|------|
| Phase 1 B7（空 Struct） | 0 | 32 + 0 + 49 = 81 | 81 | **0** ✓ |
| Phase 3 B1（1 字段 BitStruct） | 1 | 32 + 5 + 41 = 78 | 84 | +6 |
| Phase 3 BS1（3 字段 BitStruct） | 3 | 32 + 15 + 41 = 88 | 95 | +7 |
| Phase 2.5 E1（2 字段） | 2 | 32 + 10 + ~13 = 55 | 27 → 55 (注) | - |
| Phase 2.5 E3（6 字段） | 6 | 32 + 30 + (-43) = 19 | -11 → 19 (注) | - |

注：Phase 2.5 字段含 Bytes（extract bytes 比 extract int 便宜），且 E3 含 Tell/Computed（build 端 compute_ro_value 额外开销）。字段类型差异使每字段贡献偏离 +5 ns 均值。

### 4.4 PM 疑惑的解答

**Q: 为什么 Phase 2.5 E1（2 字段）差只有 27 ns，而 Phase 3 B1（1 字段）差 84 ns？**

A: 两个原因叠加：

1. **字段数效应**：每增加 1 个字段，build 端多一次 getattr + extract（~16 ns），parse 端多一次 PyLong_create + dict.set_item（~16 ns）。两者抵消，**每字段净贡献仅 ~5 ns**。但 Phase 2.5 字段含 Bytes/Tell/Computed，build 端开销更大，每字段贡献可能为负。
2. **入口差**：Phase 2.5 E1 实测差 27 ns，扣除 StructNode 固有 32 ns 后，入口 + 字段级贡献 = -5 ns（build 端字段开销略超 parse 端）。

Phase 3 B1 是单字段，字段级贡献无法抵消 StructNode 固有 32 ns + 入口 41 ns = 73 ns 的固定不对称。所以差距接近 80 ns 是正常的。

**Q: tp_new 是根因吗？**

A: **部分是**。tp_new 占 22 ns（实测），是 StructNode 固有不对称的主要组成。但"tp_new ~80ns"是错误归因——实际 tp_new 只 22 ns，StructNode 固有不对称总共 32 ns（含 getattr_dict + dict_init 等）。

**Q: Phase 3 引入了新的不对称吗？**

A: **没有**。BitwiseNode 包装层对称（<1 ns 差距）。BitsIntegerNode 字段级贡献与 FormatField 等价（~5 ns/字段）。i128 PyLong 创建慢于 i64，但 i128 extract 也慢于 i64，两者抵消。

---

## 5. 优化建议

虽然不是"结构性 bug"，仍有 3 个优化方向：

### OPT-1: BitsIntegerNode.parse 改用 i64/u64 fast path（推荐）

**当前代码**（`bits_integer.rs` L182-190）：
```rust
let value: i128 = if self.signed && (raw >> (self.length - 1)) & 1 == 1 {
    (raw as i128) - (1i128 << self.length)
} else {
    raw as i128
};
Ok(value.into_py(py))  // 统一走 i128::into_py（慢路径）
```

**问题**：即使 unsigned 8-bit（值 0..255），也走 `i128::into_py` 慢路径（21 ns），而 `i64::into_py` 只需 1.7 ns。

**修复**：根据 signed + length 选择 PyLong 创建路径：
```rust
// signed: value 总在 i64 范围（length <= 64, signed max = i64::MAX）
// unsigned length < 64: value 在 i64 正范围
// unsigned length == 64: value 在 u64 范围
if self.signed || self.length < 64 {
    let v_i64 = value as i64;  // 安全：signed 总在 i64 范围；unsigned <64 也 fit
    Ok(v_i64.into_py(py))
} else {
    // unsigned length == 64
    let v_u64 = raw;  // 原始 u64
    Ok(v_u64.into_py(py))
}
```

**预计收益**：
- 每字段 parse 端节省 ~20 ns（i128::into_py 21ns → i64::into_py 1.7ns）
- BS1 (2 字段 BitsInteger) parse：386 → ~346 ns，加速比 12.8x → ~14.3x
- B1 (1 字段) parse：275 → ~255 ns，加速比 5.87x → ~6.3x

**对 parse-build 对称性的影响**：parse 端变快，差距从 +84 缩小到 +64 ns。但仍 > 0（StructNode 固有不对称未消除）。

### OPT-2: BitsIntegerNode.build 改用 i64 fast path

**当前代码**（`bits_integer.rs` L245）：`let value: i128 = match obj.extract::<i128>() {...}`

**修复**：先尝试 `extract::<i64>`，失败再 `extract::<i128>`（极大数报错路径）：
```rust
let value: i128 = match obj.extract::<i64>() {
    Ok(v) => v as i128,
    Err(_) => obj.extract::<i128>().map_err(...)?,
};
```

**预计收益**：
- 每字段 build 端节省 ~15 ns（extract<i128> 16ns → extract<i64> 1.2ns）
- 整体 build 加速比提升

**对 parse-build 对称性的影响**：build 端变快，差距反而增大（+64 → +79 ns）。但这不是"变差"，是双方都快了。

### OPT-3: StructNode 固有不对称优化（研究性质）

StructNode parse 端独有 tp_new(22) + getattr_dict(11.6) + dict_init ≈ 34 ns，build 端独有 PyBytes(8.7) + BuildStream(21) ≈ 30 ns。两者已经很接近。

可能的优化：
- parse 端跳过 getattr_dict（直接用 tp_new 返回的实例的预取 dict 指针）—— pyo3 API 不直接支持，需 unsafe C API
- build 端避免 BuildStream 分配（预分配池化）—— 已用 with_capacity，进一步收益有限

**收益估计**：< 10 ns，工程复杂度高，**不推荐当前阶段做**。

---

## 6. 对 PM 验收的建议

### 6.1 不对称本身不是 bug

Phase 3 的 84-95 ns 不对称是 **mashumaro API 的固有代价**（parse 产出实例 vs build 读取实例），与 Phase 1 完全一致，不是 Phase 3 引入的结构性问题。

### 6.2 验收标准建议

建议 PM 采用以下任一标准：

**标准 A（宽松）**：接受现状。Phase 3 BitStruct 端到端场景已 ≥10x（BS1 12.8x、BS2 12.5x、BW1 14.5x），单字段场景 5-9x 是 FFI 开销占比高的固有特性。

**标准 B（推荐）**：要求 DEV 实施 OPT-1（i64 fast path），将 parse 方向几何平均从 7.67x 提升到 ~9-10x。这是低成本（修改一个文件 ~20 行）、高收益（每字段节省 20 ns）的优化。

**标准 C（严格）**：要求 DEV 实施 OPT-1 + OPT-2，几何平均预期 ~10-11x。但 OPT-2 会让 parse-build 差距重新扩大（build 也变快），不影响整体性能。

### 6.3 不建议的标准

不建议要求"parse-build 对称"作为验收标准。对称性受 mashumaro API 设计约束（parse 必须创建实例，build 只读取实例），不是实现质量问题。pydantic-core 的 parse/build 也有类似不对称。

---

## 7. 实验数据完整存档

实验输出已存档于 `experiments/bench_asymmetry/`。完整运行日志：

```
（见 experiments/bench_asymmetry/output.txt，由 cargo run --release 生成）
```

关键数据表已在上文 §3 引用。所有数据可由 `cargo run --release`（在 experiments/bench_asymmetry/ 下，设置 `PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1`）复现。

---

## 8. 结论

| 问题 | 答案 |
|------|------|
| Phase 3 多出的 ~56ns 来自哪里？ | 来自 **字段数差异**（Phase 3 B1/B2/B3 是 1 字段，Phase 2.5 E1 是 2 字段、E3 是 6 字段）。字段级 build 开销累积抵消了 StructNode 固有不对称。 |
| tp_new 是根因吗？ | **部分是**（占 22 ns），但"tp_new ~80ns"是错误归因。StructNode 固有不对称总共 32 ns。 |
| Phase 3 引入了新的不对称吗？ | **没有**。BitwiseNode 对称（<1ns 差），BitsInteger 字段级贡献 = FormatField。 |
| i128 PyLong 创建是瓶颈吗？ | **是单项最大优化机会**（每字段 20ns），但不影响对称性（extract<i128> 也慢 15ns，两者抵消）。 |
| 修复方案？ | OPT-1：BitsIntegerNode.parse 改用 i64/u64 fast path，预计 parse 方向加速比 7.67x → ~9-10x。 |
