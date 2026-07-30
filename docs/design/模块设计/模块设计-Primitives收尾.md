---
id: DESIGN-phase6-primitives
status: active
phase: "6"
task: "6.1 [Primitives 详细设计]"
revision: v2
depends_on: [ANALYSIS-phase6-pre, PLAN-phase6, DESIGN-test-framework, ADR-016, ADR-019, DESIGN-phase5-ffi]
last_updated: 2026-07-30
---

# 模块设计 - Primitives 收尾（Phase 6.1，17 个构造器）

> **输入**：
> - `docs/analysis/分析报告-Phase6启动前置.md §B`（789 行，17 个构造器实现摘要）
> - `plans/phase6-primitives-strings-adapter/总纲.md`（PM 5 项决策已锁定）
> - `construct-rs/src/nodes/format_field.rs`（现有 FormatFieldNode + PythonFormat enum）
> - `construct-rs/src/nodes/mod.rs`（现有 20 个 Node 变体）
> - Python construct `core.py:1124-1686`（FormatField / BytesInteger / VarInt / ZigZag 原版）
> - Python construct `lib/binary.py:38-92`（integer2bytes / bytes2integer 工具）
>
> **角色**：ARCH
> **状态**：DESIGNING v2（REV v1 驳回后修正，P1/P2/P3/P4，待 REV 复检）
> **创建时间**：2026-07-30
> **v2 修订（2026-07-30）**：P1 整数错误统一 `ConstructError::Integer`（VarInt/ZigZag/BytesInteger）/ P2 Float build 加 int→float 兼容 + BC-B15 / P3 VarInt slow-path 缓存编译 + §9.3 删除技术错误预案 / P4 Node 变体数 24→20
>
> **设计总览（一句话）**：17 个构造器分为 3 类——8 个纯别名（Python 层 alias 到 FormatFieldNode/BytesIntegerNode）/ 6 个 Float（扩展 PythonFormat enum +6 变体，Float16 用 `half` crate）/ 3 个新 Node 变体（VarIntNode / ZigZagNode / BytesIntegerNode）。**零 unsafe raw FFI**（BytesInteger 大整数走 Python `int.from_bytes` 慢路径，避开分析报告 §B.2.5 推测的 raw FFI）。

---

## §0 文档定位与范围

### 0.1 在范围内（17 个构造器）

| 子类 | 构造器 | 数量 | 实现方式 |
|------|--------|-----:|---------|
| 整数别名（FormatField） | Byte / Short / Int / Long | 4 | Python 层 alias，零 Rust 改动 |
| Float（IEEE 754） | Float32b / Float32l / Float64b / Float64l / Float16b / Float16l | 6 | 扩展 `PythonFormat` enum +6 变体，Float16 用 `half` crate |
| Int24 | Int24ub / Int24ul / Int24sb / Int24sl | 4 | Python 层 alias 到 `BytesIntegerNode(3, ...)` |
| 变长 | VarInt / ZigZag | 2 | 新 Node 变体（`VarIntNode` / `ZigZagNode`） |
| 大整数 | BytesInteger | 1 | 新 Node 变体（`BytesIntegerNode`），fast/slow 双路径 |

**统计**：纯别名 8 个 + PythonFormat 扩展 6 个 + 新 Node 变体 3 个（覆盖 3 个构造器）= 17 个构造器 ✅

### 0.2 不在范围内

- **Native endian 变体**（`Int24un` / `Int24sn` / `Float16n` / `Float32n` / `Float64n`）：Python construct 提供，但 native endian 在跨平台序列化协议中无意义（用户面极罕见）。本 phase 不实现，inventory 标注 `deferred`，Phase 7+ 评估。如用户有需求，DEV 可在 Python 层用 `sys.byteorder` 路由到 `*ub/*ul`（< 10 行 Python）。
- **Half / Single / Double 别名**（`Half = Float16b` 等）：Python 层 trivial 别名，由 Python wrapper 模块处理，不在 Rust 设计范围。
- **性能验证**：本文档仅给**可证伪预测**（L-02 对策），实测由 6.1 VET 用 `bench/bench_primitives.py` 完成（L-09 对策：Controlled A/B Test + 同会话测量）。

### 0.3 关键决策摘要（ARCH 决策 + PM 决策点）

| # | 决策 | ARCH 决定 | PM 决策点？ | 影响范围 |
|---|------|----------|-----------|---------|
| D-1 | Float16 实现 | `half` crate（成熟稳定，§1.3.1 详述） | 是（新增依赖） | Cargo.toml + Cargo.lock |
| D-2 | BytesInteger 大整数（>8 字节） | Python `int.from_bytes`/`int.to_bytes` 慢路径（CPython 公开稳定 API，**无 unsafe**） | 否（ARCH 已决，§1.6.1 详述） | BytesIntegerNode 实现 |
| D-3 | Int24 实现 | Python 层 alias 到 `BytesIntegerNode(3, ...)`，零 Rust 别名代码 | 否 | python wrapper 模块 |
| D-4 | Byte/Short/Int/Long 别名 | Python 层 alias 到 FormatFieldNode（Byte=Int8ub 等） | 否 | python wrapper 模块 |
| D-5 | VarInt / ZigZag sizeof | 返回 `Err(SizeofError)`（变长，与 Python 一致） | 否 | VarIntNode / ZigZagNode |

> **D-2 关键修正**：分析报告 §B.2.5 推测 BytesInteger 大整数需 `unsafe raw FFI`（`PyLong_FromByteArray`）。
> ARCH 在设计阶段发现更优方案：**调用 Python `int.from_bytes` / `int.to_bytes`**——CPython 公开稳定 API（自 Python 3.2 起），
> 通过 pyo3 `call_method` 调用，零 unsafe，零中间表示层。详见 §1.6.1 + §3 §0 原则对照。
> 此决策**降低**了 Phase 6.1 的实现风险（无 unsafe，无需 ADR-019 式 SAFETY 论证）。

---

## §1 17 个构造器的 Node 变体设计

### 1.1 分类总览

按"实现方式"而非"用户面名称"分组，因别名与新 Node 变体的工程量差异巨大：

| 实现方式 | 构造器 | Rust 工作量 | Python 工作量 | 复杂度 |
|---------|--------|-----------|--------------|------|
| **(A) Python 层别名**（零 Rust 改动） | Byte / Short / Int / Long（4） + Int24ub/ul/sb/sl（4） | 0 行 | ~30 行（python/construct/\_\_init\_\_.py 加 alias） | 极低 |
| **(B) 扩展 PythonFormat**（FormatFieldNode 复用） | Float32b/l + Float64b/l（4） + Float16b/l（2） | ~150 行（format_field.rs 扩展） | ~10 行（Python 单例定义） | 低 |
| **(C) 新 Node 变体** | VarInt / ZigZag / BytesInteger（3 Node） | ~400 行（3 个新文件） | ~20 行（Python 单例定义） | 中 |

**关键观察**：
- 8 个别名零 Rust 改动，但需前置 BytesIntegerNode（Int24 别名依赖）
- Float16 是唯一新依赖（`half` crate），其他 5 个 Float 复用现有 FormatFieldNode 数据流
- 3 个新 Node 变体的复杂度集中在 BytesIntegerNode（双路径 + signed/endian 矩阵）

### 1.2 类别 A：整数别名（8 个，零 Rust 改动）

#### 1.2.1 Byte / Short / Int / Long（FormatField 别名，4 个）

**Python 原版**（`core.py:1524-1527`）：

```python
Byte  = Int8ub    # FormatField(">", "B")
Short = Int16ub   # FormatField(">", "H")
Int   = Int32ub   # FormatField(">", "L")  ← 注意 Python 用 "L"（无符号 4 字节），非 "I"
Long  = Int64ub   # FormatField(">", "Q")
```

**construct-rs 实现**：Python wrapper 模块直接 alias 到现有 FormatFieldNode 单例：

```python
# python/construct/__init__.py（或 _primitives.py）
from ._singleton import Int8ub, Int16ub, Int32ub, Int64ub
Byte  = Int8ub
Short = Int16ub
Int   = Int32ub
Long  = Int64ub
```

**注意**：`Int` 在 Python construct 用 `FormatField(">", "L")`（无符号 4 字节）。Rust 侧 `PythonFormat::UnsignedInt32Big` 已对应 `>I`（同语义，Python struct 模块 `L` 和 `I` 在大端无符号 4 字节完全等价）。DEV 验证 parity 时确认 `Int.parse(b"\x00\x00\x01\x00") == 256` 与 Python 一致。

#### 1.2.2 Int24ub / Int24ul / Int24sb / Int24sl（BytesInteger 别名，4 个）

**Python 原版**（`core.py:1575-1593`）：

```python
Int24ub = BytesInteger(3, signed=False, swapped=False)
Int24ul = BytesInteger(3, signed=False, swapped=True)
Int24sb = BytesInteger(3, signed=True,  swapped=False)
Int24sl = BytesInteger(3, signed=True,  swapped=True)
```

**construct-rs 实现**：Python wrapper 模块用 `BytesInteger` 工厂函数（Rust 侧 `BytesIntegerNode` 的 Python 包装）：

```python
# python/construct/__init__.py
from ._primitives import BytesInteger
Int24ub = BytesInteger(3, signed=False, swapped=False)
Int24ul = BytesInteger(3, signed=False, swapped=True)
Int24sb = BytesInteger(3, signed=True,  swapped=False)
Int24sl = BytesInteger(3, signed=True,  swapped=True)
```

**前置依赖**：必须先实现 `BytesIntegerNode`（§1.6）。

**3 字节特殊性**：3 字节 ≤ 8 字节，走 BytesIntegerNode fast-path（§1.6.2）。signed 时需符号扩展（高位为 1 时填充 0xFF），endian 决定字节序。fast-path 已处理此矩阵，无需特殊代码。

### 1.3 类别 B：Float 系列（6 个，扩展 PythonFormat enum）

#### 1.3.1 Float16 方案选择：`half` crate vs 手写 bit 拼接

**ARCH 决策：用 `half` crate**。

| 维度 | half crate（推荐） | 手写 bit 拼接 |
|------|-----------------|--------------|
| **正确性** | 由社区验证（IEEE 754-2008 half 完整实现，含 subnormal / NaN / Inf payload） | 需手工实现 16↔32 位转换（~80 行），易遗漏 subnormal / NaN payload |
| **维护** | active 维护（last release 2025-12），Rust 核心团队推荐 | construct-rs 自维护，需自测 ~20 case |
| **性能** | `f16::from_le_bytes` / `to_le_bytes` 编译期内联，与 Rust std `f32` 同速（zero-cost abstraction） | 手写 bit 操作可能略快（~1-2ns），但被 FFI 噪声淹没 |
| **代码体积** | ~12KB 编译产物（half crate no_std，仅 f16 类型） | 0 新依赖，但 ~80 行新代码 |
| **风险** | 极低（crate 在 rust-gpu / bevy 等大项目验证） | 中（subnormal 处理易错，Python construct 依赖 struct 模块的 'e' 格式） |
| **依赖审核** | 单一 crate，无 transitive deps（仅依赖编译器内置） | 无依赖 |

**half crate 选用理由**：
1. **正确性优先**：Float16 的 subnormal（denormal）数 + NaN payload 在手写中极易出错（Python construct 用 CPython struct 模块，已含完整 IEEE 754-2008 实现，parity 测试需 100% 匹配）
2. **代码体积可接受**：~12KB 编译产物，远小于 FormatFieldNode 已有代码量
3. **与 Rust 标准库语义一致**：Rust 1.79+ 的 `std::half::f16`（nightly only）即基于 half crate，stable 路径用 half crate 是社区惯例
4. **零性能损失**：half crate 的 `f16::from_bits` / `to_bits` 是 const fn，编译期内联

**Cargo.toml 改动**：

```toml
[dependencies]
# ... 现有依赖 ...
half = "2.4"  # Phase 6.1：Float16 IEEE 754 半精度支持
```

> **PM 决策点 D-1**：是否接受新增 `half` crate 依赖？
> - **ARCH 推荐**：是（正确性 + 维护 + 性能三维度均优于手写）
> - **替代方案**：手写 ~80 行 bit 拼接（无新依赖，但 subnormal/NaN 易错）
> - **影响**：Cargo.toml + Cargo.lock 新增一行依赖；编译产物 +12KB
> - **风险**：极低（half crate 在 rust-gpu/bevy/wgpu 等大项目验证）

#### 1.3.2 PythonFormat enum 扩展（+6 变体）

现有 `PythonFormat` enum 含 16 个整数变体（`format_field.rs:38-71`）。扩展为 22 个，新增 6 个 Float 变体：

```rust
pub enum PythonFormat {
    // ---- 现有 16 个整数变体（不变） ----
    UnsignedInt8Big, UnsignedInt8Little, /* ... */ SignedInt64Little,

    // ---- Phase 6.1 新增：Float 系列 6 个变体 ----
    /// 半精度 IEEE 754，大端（`>e`，对应 `Float16b`）
    Float16Big,
    /// 半精度 IEEE 754，小端（`<e`，对应 `Float16l`）
    Float16Little,
    /// 单精度 IEEE 754，大端（`>f`，对应 `Float32b`）
    Float32Big,
    /// 单精度 IEEE 754，小端（`<f`，对应 `Float32l`）
    Float32Little,
    /// 双精度 IEEE 754，大端（`>d`，对应 `Float64b`）
    Float64Big,
    /// 双精度 IEEE 754，小端（`<d`，对应 `Float64l`）
    Float64Little,
}
```

**`from_chars` 扩展**（`format_field.rs:84-110`）：现有 `from_chars` 仅匹配 `'B'/'b'/'H'/'h'/'I'/'i'/'Q'/'q'`，新增 `'e'/'f'/'d'` 三字符 × 2 endian = 6 组合。

**`byte_length` 扩展**（`format_field.rs:113-132`）：Float16=2 / Float32=4 / Float64=6。

**`fmtstr` 扩展**（`format_field.rs:137-156`）：返回 `">e"` / `"<e"` / `">f"` / `"<f"` / `">d"` / `"<d"`。

#### 1.3.3 FormatFieldNode.parse / build 扩展（Float 分支）

parse 路径新增 6 个分支（`format_field.rs:243-312` 之后）：

```rust
match self.format {
    // ---- 现有 16 个整数分支（不变） ----

    // ---- Phase 6.1 新增：Float 分支 ----
    PythonFormat::Float16Big => {
        let arr = read_array::<2>(stream, path)?;
        let f16_val = half::f16::from_be_bytes(arr);
        Ok(f16_val.to_f64().into_py(py))  // f16 → f64 → PyFloat
    }
    PythonFormat::Float16Little => {
        let arr = read_array::<2>(stream, path)?;
        let f16_val = half::f16::from_le_bytes(arr);
        Ok(f16_val.to_f64().into_py(py))
    }
    PythonFormat::Float32Big => {
        let arr = read_array::<4>(stream, path)?;
        Ok(f32::from_be_bytes(arr).into_py(py))  // f32 → PyFloat（pyo3 自动转）
    }
    PythonFormat::Float32Little => {
        let arr = read_array::<4>(stream, path)?;
        Ok(f32::from_le_bytes(arr).into_py(py))
    }
    PythonFormat::Float64Big => {
        let arr = read_array::<8>(stream, path)?;
        Ok(f64::from_be_bytes(arr).into_py(py))
    }
    PythonFormat::Float64Little => {
        let arr = read_array::<8>(stream, path)?;
        Ok(f64::from_le_bytes(arr).into_py(py))
    }
}
```

**关键设计点**：
- **Float16 → PyFloat**：Python 无 f16 类型，`half::f16::to_f64()` 转 f64 后 `into_py(py)` 创建 PyFloat。CPython struct 模块的 `'e'` 格式同样返回 Python float（f64），parity 一致 ✅
- **Float32 → PyFloat**：`f32::into_py(py)` pyo3 自动转 PyFloat（f32 → f64 提升，零精度损失因 IEEE 754 表示兼容）
- **NaN payload 一致性**：`half::f16` 完整保留 NaN payload（quiet NaN / signaling NaN），与 CPython struct 模块一致。parity 测试需覆盖（§7 case F16-4 NaN）

build 路径新增 6 个分支（`format_field.rs:324-399` 之后）：

```rust
match self.format {
    // ---- 现有 16 个整数分支（不变） ----

    // ---- Phase 6.1 新增：Float 分支 ----
    PythonFormat::Float16Big => {
        // P2 v2 修正：int→float 兼容（Python struct.pack 接受 int，mashumaro 运行时不强制）。
        let val: f64 = obj.extract::<f64>()
            .or_else(|_| obj.extract::<i64>().map(|i| i as f64))
            .or_else(|_| obj.extract::<u64>().map(|u| u as f64))
            .map_err(|_| make_build_error(fmtstr, obj, path))?;
        // P2 v2 parity 修正：Python struct.pack('>e', x) 对超 f16 范围的 finite float
        // 抛 OverflowError → construct FormatFieldError（实测 struct.pack('>e',70000.0) 报错，
        // BC-B12 v1 误判"返回 inf"已修正）。half::f16::from_f64 对超范围返回 inf（不报错），
        // 需手动检查以对齐 Python（inf/nan/正常值放行，由 from_f64 处理）。
        let f16_val = if val.is_finite() && val.abs() > 65504.0 {
            return Err(make_build_error(fmtstr, obj, path));
        } else {
            half::f16::from_f64(val)
        };
        stream.write(&f16_val.to_be_bytes());
    }
    PythonFormat::Float16Little => { /* 同 Float16Big（含 P2 int 兼容 + f16 范围检查），to_le_bytes */ }
    PythonFormat::Float32Big => {
        // P2 v2 修正：extract f32 优先（Python float，超范围→pyo3 OverflowError→FormatFieldError，
        // 对齐 Python struct.pack('>f',1e40) OverflowError→construct FormatFieldError，即 BC-B8）。
        // int 输入走 i64/u64 fallback：i64/u64::MAX < f32::MAX，as f32 不溢出，与 Python
        // struct.pack('>f', int) 路径（int→double→float，in range）parity 一致。
        let val: f32 = obj.extract::<f32>()
            .or_else(|_| obj.extract::<i64>().map(|i| i as f32))
            .or_else(|_| obj.extract::<u64>().map(|u| u as f32))
            .map_err(|_| make_build_error(fmtstr, obj, path))?;
        stream.write(&val.to_be_bytes());
    }
    PythonFormat::Float32Little => { /* 同 Float32Big（含 P2 int 兼容），to_le_bytes */ }
    PythonFormat::Float64Big => {
        // P2 v2 修正：int→float 兼容。f64 精度足以容纳所有 i64/u64，无范围检查。
        let val: f64 = obj.extract::<f64>()
            .or_else(|_| obj.extract::<i64>().map(|i| i as f64))
            .or_else(|_| obj.extract::<u64>().map(|u| u as f64))
            .map_err(|_| make_build_error(fmtstr, obj, path))?;
        stream.write(&val.to_be_bytes());
    }
    PythonFormat::Float64Little => { /* 同 Float64Big（含 P2 int 兼容），to_le_bytes */ }
}
```

**关键设计点（P2 v2 修正：int→float 兼容 + parity 校准）**：
- **int→float 兼容**：所有 6 个 Float build 分支加 i64/u64 fallback。Python `struct.pack` 接受 int（自动转 float，实测 `struct.pack('>f', 42) == b'42280000'`），mashumaro `v: float` 注解运行时不强制，用户传 int 常见。Rust 顺序：`extract f32/f64`（Python float）→ `i64 as f32/f64` → `u64 as f32/f64`（Python int）
- **Float16 build**：Python float/int（f64）→ `half::f16::from_f64`（IEEE 754 round-to-nearest-even）→ 2 字节。**parity 修正**：超 f16 范围的 finite float（如 70000）手动返回 FormatFieldError（对齐 `struct.pack('>e', 70000)` OverflowError；**BC-B12 v1 误判"返回 inf"已修正**）。CPython struct `'e'` 格式 parity 一致 ✅
- **Float32 build**：`extract::<f32>()` 优先（Python float，超范围→pyo3 OverflowError→FormatFieldError，即 BC-B8，对齐 `struct.pack('>f', 1e40)` OverflowError→construct FormatFieldError）。int 输入走 i64/u64 fallback（i64/u64::MAX < f32::MAX，`as f32` 不溢出）
- **Float64 build**：`extract::<f64>()` + i64/u64 fallback。f64 精度足够容纳所有 i64/u64，无范围检查
- **NaN/Inf build**：Python `float('nan')/float('inf')` 正常通过（实测 `struct.pack('>f', inf)==7f800000`、`nan==7fc00000`；`'>e' inf==7c00`、`nan==7e00`）。Float16 的 inf/nan 放行（仅 finite 且超范围才报错）

### 1.4 类别 C：变长整数（VarInt / ZigZag，2 个新 Node 变体）

#### 1.4.1 VarInt（LEB128 无符号变长整数）

**Python 原版**（`core.py:1601-1647`）：

```python
class VarInt(Construct):
    def _parse(self, stream, context, path):
        acc = []
        while True:
            b = byte2int(stream_read(stream, 1, path))
            acc.append(b & 0b01111111)
            if b & 0b10000000 == 0:
                break
        num = 0
        for b in reversed(acc):
            num = (num << 7) | b
        return num

    def _build(self, obj, stream, context, path):
        if not isinstance(obj, int): raise IntegerError(...)
        if obj < 0: raise IntegerError("VarInt cannot build from negative number")
        x = obj
        B = bytearray()
        while x > 0b01111111:
            B.append(0b10000000 | (x & 0b01111111))
            x >>= 7
        B.append(x)
        stream_write(stream, bytes(B), len(B), path)
        return obj
```

**Rust 实现**：`construct-rs/src/nodes/varint.rs`（新文件，~80 行）。

```rust
//! VarIntNode：LEB128 无符号变长整数（Google Protocol Buffers 编码）。
//!
//! Python 参考：`construct/construct/core.py:1601-1647`（VarInt 类）。
//!
//! ## 编码规则
//!
//! 每字节低 7 位是有效数据，最高位（MSB）=1 表示后续还有字节，MSB=0 表示终止。
//! 小整数（0-127）仅需 1 字节，大整数按 7 位分组扩展。
//!
//! ## 限制
//!
//! - 仅支持非负整数（build 负数返回 `IntegerError`，与 Python 一致）
//! - sizeof 永远返回 `Err(SizeofError)`（变长，无法预知）

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::conversion::IntoPy;
use pyo3::prelude::*;

/// LEB128 无符号变长整数节点（对应 Python construct `VarInt`）。
///
/// 单例语义（Python `VarInt` 是 `@singleton` class），Rust 侧可用 `VarIntNode::default()`
/// 或编译期单例。本节点无配置字段，所有实例等价。
#[derive(Debug, Clone, Copy, Default)]
pub struct VarIntNode;

impl VarIntNode {
    pub fn new() -> Self { Self }
}

impl super::Construct for VarIntNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        _ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // LEB128 解码循环
        let mut result: u64 = 0;
        let mut shift: u32 = 0;
        loop {
            let byte_chunk = stream.read(1, path)?;
            let b = byte_chunk[0];
            result |= ((b & 0x7F) as u64) << shift;
            if b & 0x80 == 0 { break; }
            shift += 7;
            // 防御：u64 最多 10 字节（70 位），超过则溢出
            if shift >= 64 {
                return Err(ConstructError::Generic {
                    message: "VarInt overflow: exceeds 64 bits".into(),
                    path: path.to_string(),
                });
            }
        }
        Ok(result.into_py(py))
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        _ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        // Python VarInt._build 顺序（core.py:1632-1636）：
        //   1. not isinstance(obj, int) → IntegerError "value {obj} is not an integer"
        //   2. obj < 0                  → IntegerError "VarInt cannot build from negative number {obj}"
        //   3. LEB128 编码（任意大小，无上限）
        // 错误类型统一用 `ConstructError::Integer`（对应 Python `IntegerError`，error.rs:191-196），
        // 与 §3 §0 #7 对照表一致。
        //
        // Rust 分三段处理（顺序对齐 Python 语义）：
        //   a. extract i64 成功 → 负数检查 → fast-path（v as u64，覆盖 i64 非负范围）
        //   b. extract i64 失败但 extract u64 成功 → fast-path（i64::MAX < v <= u64::MAX）
        //   c. 两者皆失败 → 若 obj 是 Python int 走大整数 slow-path，否则 IntegerError
        if let Ok(v) = obj.extract::<i64>() {
            if v < 0 {
                return Err(ConstructError::Integer {
                    message: format!("VarInt cannot build from negative number {}", v),
                    path: path.to_string(),
                });
            }
            let mut buf = [0u8; 10]; // u64 最多 10 字节
            let len = varint_encode_u64(v as u64, &mut buf);
            stream.write(&buf[..len]);
            return Ok(());
        }
        if let Ok(v) = obj.extract::<u64>() {
            let mut buf = [0u8; 10];
            let len = varint_encode_u64(v, &mut buf);
            stream.write(&buf[..len]);
            return Ok(());
        }
        if obj.is_instance_of::<pyo3::types::PyLong>() {
            // Python int 但 extract u64/i64 失败 → 大整数（> 2^64）走 slow-path
            return build_varint_bigint(py, obj, stream, path);
        }
        // 非 Python int（str/float/list 等）→ 与 Python "value {obj} is not an integer" 对齐
        Err(ConstructError::Integer {
            message: format!("value {} is not an integer", obj),
            path: path.to_string(),
        })
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        Err(ConstructError::Sizeof {
            message: "VarInt has variable size".into(),
            path: "root".into(),
        })
    }
}

/// LEB128 fast-path helper：编码 u64 到给定 10 字节缓冲区，返回写入字节数。
///
/// VarIntNode（§1.4.1）与 ZigZagNode（§1.4.2，经 ZigZag 正变换后）共用此 helper，
/// 避免两处重复字节编解码逻辑（ZigZag 设计 §1.4.2 已声明"提取为 varint_encode_bytes"）。
fn varint_encode_u64(mut x: u64, buf: &mut [u8; 10]) -> usize {
    let mut len = 0;
    while x > 0x7F {
        buf[len] = 0x80 | (x as u8 & 0x7F);
        x >>= 7;
        len += 1;
    }
    buf[len] = x as u8;
    len + 1
}

/// 缓存编译后的 VarInt 大整数编码函数（Python 层 `_varint_encode`）。
///
/// **P3 修正（v2）**：原 v1 设计每次 slow-path 调用都通过 `run_bound` 重新编译
/// 执行 Python 代码字符串定义函数，实现粗糙。v2 改为首次调用编译并缓存函数引用，
/// 后续 slow-path 仅 1 次 `call1` 跨 FFI（与 BytesInteger slow-path 同模式，与
/// error.rs 的 `EXCEPTIONS` 异常类缓存同模式）。
///
/// 选用缓存而非每次 `run_bound`：slow-path 虽罕见，但若用户在热路径反复 build
/// 大整数，每次重新编译 Python 代码（~微秒级 compile + exec）开销不可忽视。
static VARINT_ENCODER: pyo3::sync::GILOnceCell<Py<PyAny>> = pyo3::sync::GILOnceCell::new();

/// 获取（必要时编译并缓存）VarInt 大整数编码函数。
///
/// 幂等：模块初始化后后续调用直接返回缓存的 `Py<PyAny>` 引用。
fn get_varint_encoder(py: Python<'_>) -> PyResult<Py<PyAny>> {
    VARINT_ENCODER.get_or_try_init(py, || {
        // _varint_encode 与 Python VarInt._build（core.py:1637-1643）逐行等价：
        // 每 7 位 + MSB 续位的 LEB128 编码。
        let code = pyo3::ffi::c_str!(
            r#"
def _varint_encode(x):
    B = bytearray()
    while x > 0b01111111:
        B.append(0b10000000 | (x & 0b01111111))
        x >>= 7
    B.append(x)
    return bytes(B)
"#
        );
        let locals = pyo3::types::PyDict::new(py);
        py.run_bound(code, None, Some(&locals))?;
        let encoder = locals
            .get_item("_varint_encode")?
            .ok_or_else(|| {
                pyo3::exceptions::PyRuntimeError::new_err("_varint_encode not found after run")
            })?;
        encoder.extract::<Py<PyAny>>()
    })
}

/// VarInt 大整数（> 2^64）slow-path：调用缓存的 Python `_varint_encode` 函数。
///
/// 与 BytesInteger D-2 决策一致：罕见路径走 Python callable，无 unsafe，无中间类型。
///
/// **不能与 BytesInteger slow-path（`call_method("to_bytes")`）统一**：
/// VarInt 是 LEB128 编码（每 7 位 + MSB 续位），`int.to_bytes(length, 'big')` 是
/// 直接转 N 字节大端，两者编码逻辑完全不同（原 §9.3 预案的技术错误已删除，见 §9.3）。
fn build_varint_bigint(
    py: Python<'_>,
    obj: &Bound<'_, PyAny>,
    stream: &mut BuildStream,
    path: &Path,
) -> Result<(), ConstructError> {
    let encoder = get_varint_encoder(py).map_err(|e| ConstructError::Generic {
        message: format!("VarInt slow path init failed: {}", e),
        path: path.to_string(),
    })?;
    let bytes_obj: Py<PyAny> = encoder
        .call1(py, (obj,))
        .map_err(|e| ConstructError::Generic {
            message: format!("VarInt slow path encode failed: {}", e),
            path: path.to_string(),
        })?;
    let bytes_ref = bytes_obj
        .bind(py)
        .downcast::<pyo3::types::PyBytes>()
        .map_err(|_| ConstructError::Generic {
            message: "VarInt slow path: expected bytes".into(),
            path: path.to_string(),
        })?;
    stream.write(bytes_ref.as_bytes());
    Ok(())
}
```

**关键设计点**：
- **fast-path（u64 范围）**：i64/u64 双段 extract + `varint_encode_u64` helper，零 Python 调用，零中间表示。覆盖 99.9% 用户场景（协议整数通常 < 2^64）
- **slow-path（> 2^64）**：罕见路径，调用缓存的 Python `_varint_encode` 函数（`VARINT_ENCODER` GILOnceCell 缓存编译，避免每次 `run_bound`）。无 unsafe，无 num-bigint 中间类型
- **错误类型统一 IntegerError（P1 v2 修正）**：负数与非 int 分支均用 `ConstructError::Integer`（对应 Python `IntegerError`，error.rs:191-196），错误信息与 core.py:1634/1636 对齐。v1 误用 `FormatField` 已修正
- **三段 extract（P1 v2 修正）**：i64 → u64 → `is_instance_of::<PyLong>`，对齐 Python `isinstance(obj, int)` 语义，补全 v1 遗漏的"非 int 显式 IntegerError"分支
- **sizeof = Err**：与 Python `SizeofError` 一致（变长字段无法预知大小）

**Node enum 注册**（`mod.rs:169-235`）：

```rust
pub enum Node {
    // ... 现有 20 个变体 ...
    /// Phase 6.1：LEB128 无符号变长整数（对应 Python construct `VarInt`）。
    VarInt(VarIntNode),
    // ...
}
```

#### 1.4.2 ZigZag（有符号变长整数，包装 VarInt）

**Python 原版**（`core.py:1651-1686`）：

```python
class ZigZag(Construct):
    def _parse(self, stream, context, path):
        x = VarInt._parse(stream, context, path)
        if x & 1 == 0:
            x = x // 2
        else:
            x = -(x // 2 + 1)
        return x

    def _build(self, obj, stream, context, path):
        if not isinstance(obj, int): raise IntegerError(...)
        if obj >= 0:
            x = 2 * obj
        else:
            x = 2 * abs(obj) - 1
        VarInt._build(x, stream, context, path)
        return obj
```

**Rust 实现**：`construct-rs/src/nodes/zigzag.rs`（新文件，~60 行）。**复用 VarIntNode 的字节编解码**，仅做 ZigZag 数值变换。

```rust
//! ZigZagNode：有符号变长整数（Google Protocol Buffers ZigZag 编码）。
//!
//! Python 参考：`construct/construct/core.py:1651-1686`（ZigZag 类）。
//!
//! ## 编码规则
//!
//! ZigZag将有符号整数映射为无符号整数后再用 LEB128 编码：
//! - 0 → 0, -1 → 1, 1 → 2, -2 → 3, 2 → 4, ...
//! - 公式：zz(n) = (n << 1) ^ (n >> 63)  （Rust 算术右移）
//! - 反向：n = (zz >> 1) ^ -(zz & 1)
//!
//! ## 设计选择
//!
//! 不通过组合 VarIntNode 实现（避免 Box<Node> 间接），而是直接复用 VarIntNode
//! 的字节编解码 helper（提取为 `varint_encode_bytes` / `varint_decode_bytes` 函数）。
//! 理由：ZigZag 仅多一次 XOR + shift，组合 Node 会引入额外 dispatch 开销。

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::conversion::IntoPy;
use pyo3::prelude::*;
use super::varint::VarIntNode;  // 复用 fast-path helper

#[derive(Debug, Clone, Copy, Default)]
pub struct ZigZagNode;

impl ZigZagNode {
    pub fn new() -> Self { Self }
}

impl super::Construct for ZigZagNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 复用 VarIntNode 解码（得到无符号 zz 值），再做 ZigZag 反变换
        let zz_node = VarIntNode::new();
        let zz_py = zz_node.parse(py, stream, ctx, path)?;
        let zz: u64 = zz_py.bind(py).extract()
            .map_err(|_| ConstructError::Generic {
                message: "ZigZag: VarInt decode returned non-u64".into(),
                path: path.to_string(),
            })?;
        // ZigZag 反变换：n = (zz >> 1) ^ -(zz & 1)
        let signed: i64 = ((zz >> 1) as i64) ^ -((zz & 1) as i64);
        Ok(signed.into_py(py))
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        let val: i64 = obj.extract::<i64>().map_err(|_| {
            // Python ZigZag 仅检查 isinstance(obj, int)（core.py:1679），不检查范围
            // （Python int 无限精度，ZigZag 调 VarInt 处理任意大小）。Rust 限 i64
            // （§1.4.2 i64 范围决策）：非 int 或超 i64 都归 IntegerError，与 Python
            // IntegerError 对齐（P1 v2 修正：v1 误用 FormatField 已改）。
            ConstructError::Integer {
                message: format!("value {} is not an integer or out of i64 range", obj),
                path: path.to_string(),
            }
        })?;
        // ZigZag 正变换：zz = (val << 1) ^ (val >> 63)（Rust 算术右移）
        let zz: u64 = ((val << 1) ^ (val >> 63)) as u64;
        // 复用 VarIntNode build（fast-path）
        let zz_obj = zz.into_py(py);
        let zz_node = VarIntNode::new();
        zz_node.build(py, zz_obj.bind(py), stream, ctx, path)
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        Err(ConstructError::Sizeof {
            message: "ZigZag has variable size".into(),
            path: "root".into(),
        })
    }
}
```

**关键设计点**：
- **复用 VarIntNode**：parse 调 `VarIntNode.parse` 拿到 zz 值，build 先做 ZigZag 正变换再调 `VarIntNode.build`。零重复字节编解码代码
- **i64 范围限制**：ZigZag 通常用于 protobuf int32/int64，i64 范围足够。Python ZigZag 接受任意大整数，但实际用户面罕见 > i64。若需 > i64 支持，走 VarInt 慢路径（设计预留，但不在 6.1 范围）
- **算术右移**：Rust `i64 >> 63` 是算术右移（符号位扩展），与 Python `>>` 一致（Python 整数无限精度，但 ZigZag 公式在 64 位范围内行为一致）

### 1.5 类别 D：BytesInteger（1 个新 Node 变体）

#### 1.5.1 Python `int.from_bytes` 慢路径决策（D-2，避免 unsafe）

**ARCH 决策**：BytesInteger 大整数（> 8 字节）走 **Python `int.from_bytes` / `int.to_bytes` 慢路径**，**无 unsafe raw FFI**。

**决策路径对比**：

| 方案 | unsafe？ | 中间类型？ | 性能（大整数） | ABI 兼容 | 实现复杂度 |
|------|---------|----------|-------------|---------|----------|
| **A. Python int.from_bytes（推荐）** | 否 | 否（bytes → PyLong 直转） | ~5-8x（call_method ~80ns + Python 编码） | ✅ 稳定 ABI | 低（~20 行） |
| B. raw FFI `PyLong_AsByteArray`（分析报告推测） | 是 | 否 | ~8-12x（直接 C API） | ⚠️ `_PyLong_FromByteArray` 是 CPython 私有 | 中（需 ADR-019 式 SAFETY 论证） |
| C. num-bigint 中间类型 | 否 | **是**（违反 §0 #2） | ~3-5x（多一次 Rust→Rust 转换） | ✅ | 中（~40 行 + 新依赖） |

**方案 A 选用理由**：
1. **零 unsafe**：BytesInteger > 8 字节是罕见场景（协议通常用 Int24=3 字节，crypto 哈希用 32 字节但不在主路径），不值得为此引入 unsafe raw FFI
2. **零中间表示层**：`int.from_bytes(bytes, 'big', signed=True)` 返回 Python 原生 int，无 Rust 中间类型。bytes 本身是数据载体，不算"中间表示"
3. **CPython 稳定 API**：`int.from_bytes` / `int.to_bytes` 自 Python 3.2 起稳定，无 ABI 兼容风险
4. **实现简洁**：~20 行 pyo3 `call_method` 代码，无需 SAFETY 注释
5. **fast-path 覆盖主场景**：≤ 8 字节（含 Int24 = 3 字节）走 Rust 原生 fast-path，~10-15x。慢路径仅触发于显式 `BytesInteger(16+)` 罕见用法

> **与 ADR-019 的区别**：ADR-019 在错误路径首次使用 unsafe raw FFI，因错误路径性能要求严格（用户硬约束 #5 ≥10x）。
> BytesInteger 大整数不在性能主路径（罕见），无需 unsafe 换性能。这是"按场景选择工具"原则的体现。

#### 1.5.2 BytesIntegerNode 数据结构

```rust
//! BytesIntegerNode：任意字节长度整数（对应 Python construct `BytesInteger`）。
//!
//! Python 参考：`construct/construct/core.py:1203-1292`（BytesInteger 类）。
//!
//! ## 设计
//!
//! - length：编译期或表达式求值（Python 接受 int 或 context lambda）
//! - signed：bool（true = two's complement 有符号）
//! - swapped：bool（true = little endian，false = big endian）
//!
//! ## 双路径
//!
//! - fast-path（length ≤ 8）：Rust 原生 u64/i64 + 字节序转换，零 Python 调用
//! - slow-path（length > 8）：调用 Python `int.from_bytes` / `int.to_bytes`（D-2 决策）

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

/// 任意字节长度整数节点（对应 Python construct `BytesInteger`）。
///
/// Int24ub/ul/sb/sl 是 `BytesIntegerNode { length: 3, ... }` 的 Python 层别名。
#[derive(Debug, Clone)]
pub struct BytesIntegerNode {
    /// 字节长度（编译期已知）。Python 也支持 context lambda，本 phase 仅支持编译期常量
    /// （context lambda 走 Phase 7 Streams 的表达式路径）。
    length: usize,
    /// 是否有符号（two's complement）。
    signed: bool,
    /// 是否小端（true = little endian，false = big endian）。
    swapped: bool,
}

impl BytesIntegerNode {
    pub fn new(length: usize, signed: bool, swapped: bool) -> Self {
        Self { length, signed, swapped }
    }

    pub fn length(&self) -> usize { self.length }
    pub fn signed(&self) -> bool { self.signed }
    pub fn swapped(&self) -> bool { self.swapped }
}
```

#### 1.5.3 parse 双路径

```rust
impl super::Construct for BytesIntegerNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        _ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        if self.length == 0 {
            // P1 v2 修正：与 Python core.py:1249 `raise IntegerError(f"length {length}
            // must be positive")` 对齐，用 ConstructError::Integer（v1 误用 FormatField）。
            return Err(ConstructError::Integer {
                message: format!("length {} must be positive", self.length),
                path: path.to_string(),
            });
        }
        let data = stream.read(self.length, path)?;

        if self.length <= 8 {
            // ---- fast-path：Rust 原生 u64/i64 + endian 转换 ----
            // 将 data 补齐到 8 字节（按 endian 决定补齐方向），转 u64/i64，再转 PyLong
            let mut buf = [0u8; 8];
            if self.swapped {
                // little endian：data 放低位，高位补 0（或 signed 时补符号位）
                buf[..self.length].copy_from_slice(data);
                if self.signed && (data[self.length - 1] & 0x80 != 0) {
                    // 符号扩展：高位填充 0xFF
                    for b in &mut buf[self.length..] { *b = 0xFF; }
                }
                let val_u64 = u64::from_le_bytes(buf);
                if self.signed {
                    let val_i64 = val_u64 as i64;
                    Ok(val_i64.into_py(py))
                } else {
                    Ok(val_u64.into_py(py))
                }
            } else {
                // big endian：data 放高位（buf 末尾），低位补 0/0xFF
                buf[8 - self.length..].copy_from_slice(data);
                if self.signed && (data[0] & 0x80 != 0) {
                    for b in &mut buf[..8 - self.length] { *b = 0xFF; }
                }
                let val_u64 = u64::from_be_bytes(buf);
                if self.signed {
                    Ok((val_u64 as i64).into_py(py))
                } else {
                    Ok(val_u64.into_py(py))
                }
            }
        } else {
            // ---- slow-path：调用 Python int.from_bytes（D-2 决策，无 unsafe） ----
            parse_bigint_from_bytes(py, data, self.signed, self.swapped, path)
        }
    }
    // build / sizeof 见下文
}

/// Slow-path：调用 Python `int.from_bytes(data, byteorder, signed=signed)`。
fn parse_bigint_from_bytes<'py>(
    py: Python<'py>,
    data: &[u8],
    signed: bool,
    swapped: bool,
    path: &Path,
) -> Result<Py<PyAny>, ConstructError> {
    let byteorder = if swapped { "little" } else { "big" };
    let py_bytes = pyo3::types::PyBytes::new(py, data);
    let int_type = py.get_type::<pyo3::types::PyLong>();
    let result = int_type
        .call_method("from_bytes", (py_bytes, byteorder, signed), None)
        .map_err(|e| ConstructError::Integer {
            // P1 v2 修正：与 Python core.py:1256 `raise IntegerError(str(e))` 对齐。
            message: format!("int.from_bytes failed: {}", e),
            path: path.to_string(),
        })?;
    Ok(result.into_py(py))
}
```

#### 1.5.4 build 双路径

```rust
impl super::Construct for BytesIntegerNode {
    // ... parse（见 §1.5.3） ...

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        _ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        if self.length == 0 {
            // P1 v2 修正（parity 完备）：与 Python core.py:1262-1263 build 侧
            // `if length <= 0: raise IntegerError(...)` 对齐。parse 侧已在 §1.5.3 检查。
            return Err(ConstructError::Integer {
                message: format!("length {} must be positive", self.length),
                path: path.to_string(),
            });
        }
        if self.length <= 8 {
            // ---- fast-path：extract i64/u64 → 字节序转换 → write ----
            let (val_u64, is_negative_signed) = if self.signed {
                let val: i64 = obj.extract::<i64>()
                    .map_err(|_| make_int_error("value is not an integer or out of i64 range", obj, path))?;
                (val as u64, val < 0)
            } else {
                let val: u64 = obj.extract::<u64>()
                    .map_err(|_| make_int_error("value is not an integer or out of u64 range", obj, path))?;
                (val, false)
            };
            // 范围检查（与 Python integer2bytes 一致：超范围报 IntegerError）
            self.check_range(val_u64, is_negative_signed, obj, path)?;

            let mut buf = [0u8; 8];
            if self.swapped {
                buf = val_u64.to_le_bytes();
            } else {
                buf = val_u64.to_be_bytes();
            }
            // 截取 self.length 字节（按 endian 决定截取位置）
            let bytes_to_write: &[u8] = if self.swapped {
                &buf[..self.length]  // little endian：低字节在前
            } else {
                &buf[8 - self.length..]  // big endian：高字节在前
            };
            stream.write(bytes_to_write);
            Ok(())
        } else {
            // ---- slow-path：调用 Python int.to_bytes ----
            build_bigint_to_bytes(py, obj, self.length, self.signed, self.swapped, stream, path)
        }
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        Ok(self.length)
    }
}

/// Slow-path：调用 Python `obj.to_bytes(length, byteorder, signed=signed)`。
fn build_bigint_to_bytes(
    py: Python<'_>,
    obj: &Bound<'_, PyAny>,
    length: usize,
    signed: bool,
    swapped: bool,
    stream: &mut BuildStream,
    path: &Path,
) -> Result<(), ConstructError> {
    let byteorder = if swapped { "little" } else { "big" };
    let result = obj
        .call_method("to_bytes", (length, byteorder, signed), None)
        .map_err(|e| ConstructError::Integer {
            // P1 v2 修正：与 Python core.py:1267 `raise IntegerError(str(e))` 对齐。
            message: format!("int.to_bytes failed: {}", e),
            path: path.to_string(),
        })?;
    let bytes_ref = result
        .downcast::<pyo3::types::PyBytes>()
        .map_err(|_| ConstructError::Integer {
            message: "int.to_bytes did not return bytes".into(),
            path: path.to_string(),
        })?;
    stream.write(bytes_ref.as_bytes());
    Ok(())
}
```

**Node enum 注册**（`mod.rs`）：

```rust
pub enum Node {
    // ... 现有 20 个变体 ...
    VarInt(VarIntNode),       // §1.4.1
    ZigZag(ZigZagNode),       // §1.4.2
    BytesInteger(BytesIntegerNode),  // §1.5
}
```

**has_expressions 方法扩展**（`mod.rs:247-262`）：三个新 Node 都不含表达式，`has_expressions` 在 `_ => false` 默认分支已覆盖，无需新增分支。

---

## §2 接口签名

### 2.1 Rust 侧（已在 §1 详述，此处汇总）

#### 2.1.1 新增 Node 变体（3 个）

| Node 变体 | 文件 | struct 定义 | 公开方法 |
|----------|------|-----------|---------|
| `VarIntNode` | `nodes/varint.rs`（新） | `pub struct VarIntNode;`（unit struct） | `pub fn new() -> Self` |
| `ZigZagNode` | `nodes/zigzag.rs`（新） | `pub struct ZigZagNode;`（unit struct） | `pub fn new() -> Self` |
| `BytesIntegerNode` | `nodes/bytes_integer.rs`（新） | `pub struct BytesIntegerNode { length, signed, swapped }` | `pub fn new(length, signed, swapped) -> Self` + 3 个 getter |

#### 2.1.2 现有 Node 扩展

| 现有 Node | 扩展点 | 改动量 |
|----------|-------|------|
| `FormatFieldNode` | `PythonFormat` enum +6 Float 变体；`from_chars` 支持 `'e'/'f'/'d'`；parse/build +6 Float 分支 | ~150 行 |
| `Node` enum（`mod.rs`） | +3 变体（`VarInt` / `ZigZag` / `BytesInteger`） | +3 行 + use 语句 |

#### 2.1.3 辅助函数（私有不导出）

| 函数 | 文件 | 用途 |
|------|------|------|
| `varint_encode_u64` | `nodes/varint.rs` | LEB128 fast-path 编码 helper（VarInt/ZigZag 共用，P3 v2 引入） |
| `get_varint_encoder` | `nodes/varint.rs` | 获取/编译并缓存 Python `_varint_encode` 函数（P3 v2 引入，GILOnceCell 缓存） |
| `build_varint_bigint` | `nodes/varint.rs` | VarInt > 2^64 慢路径（调用缓存的 `_varint_encoder`） |
| `parse_bigint_from_bytes` | `nodes/bytes_integer.rs` | BytesInteger > 8 字节慢路径 |
| `build_bigint_to_bytes` | `nodes/bytes_integer.rs` | BytesInteger > 8 字节慢路径 |
| `make_int_error` | `nodes/bytes_integer.rs` | 构造 `ConstructError::Integer`（与 Python 错误信息对齐，§1.5.4 fast-path 用） |

**模块级静态量**：

| 量 | 文件 | 用途 |
|------|------|------|
| `VARINT_ENCODER` | `nodes/varint.rs` | `GILOnceCell<Py<PyAny>>`，缓存 Python `_varint_encode` 函数引用（P3 v2 引入） |

### 2.2 Python 侧（用户面 API，与 Python construct 2.10.70 一致）

#### 2.2.1 单例定义（`python/construct/__init__.py` 或 `_primitives.py`）

```python
# 整数别名（4 个，Python 层 alias 到 FormatFieldNode 单例）
Byte  = Int8ub    # FormatField(">", "B")
Short = Int16ub   # FormatField(">", "H")
Int   = Int32ub   # FormatField(">", "L") → PythonFormat::UnsignedInt32Big
Long  = Int64ub   # FormatField(">", "Q") → PythonFormat::UnsignedInt64Big

# Float 系列（6 个，FormatField 单例）
Float16b = FormatField(">", "e")  # PythonFormat::Float16Big
Float16l = FormatField("<", "e")  # PythonFormat::Float16Little
Float32b = FormatField(">", "f")  # PythonFormat::Float32Big
Float32l = FormatField("<", "f")  # PythonFormat::Float32Little
Float64b = FormatField(">", "d")  # PythonFormat::Float64Big
Float64l = FormatField("<", "d")  # PythonFormat::Float64Little

# 变长（2 个，新 Node 单例）
VarInt = VarInt()    # Rust 侧 VarIntNode::new()
ZigZag = ZigZag()    # Rust 侧 ZigZagNode::new()

# Int24 系列（4 个，BytesInteger 工厂调用）
Int24ub = BytesInteger(3, signed=False, swapped=False)
Int24ul = BytesInteger(3, signed=False, swapped=True)
Int24sb = BytesInteger(3, signed=True,  swapped=False)
Int24sl = BytesInteger(3, signed=True,  swapped=True)

# Python construct 兼容别名（可选，与 2.10.70 对齐）
Half = Float16b
Single = Float32b
Double = Float64b
```

#### 2.2.2 工厂函数签名

```python
# BytesInteger 工厂（与 Python construct 2.10.70 完全一致）
def BytesInteger(length: int, signed: bool = False, swapped: bool = False) -> Construct:
    """任意字节长度整数。

    参数：
        length: 字节数（1-N，编译期已知）
        signed: True = two's complement 有符号
        swapped: True = little endian，False = big endian

    返回：CompiledSchema（封装 BytesIntegerNode）

    异常：
        IntegerError: length <= 0
    """

# FormatField 工厂（已存在，扩展接受 'e'/'f'/'d'）
def FormatField(endianity: str, format: str) -> Construct:
    """使用 struct 模块格式打包/解包 CPU 大小整数与浮点。

    参数：
        endianity: '>' / '<' / '='
        format: 'B' / 'H' / 'L' / 'Q' / 'b' / 'h' / 'l' / 'q' / 'e' / 'f' / 'd' / '?'
            Phase 6.1 新增 'e'（Float16）/ 'f'（Float32）/ 'd'（Float64）
    """
```

#### 2.2.3 用户面行为（与 Python construct 2.10.70 一致）

| 构造器 | parse 输入 | parse 输出 | build 输入 | build 输出 | sizeof |
|--------|-----------|----------|----------|---------|--------|
| `Byte` | 1 字节 | `int`（0-255） | `int`（0-255） | 1 字节 | `Ok(1)` |
| `Short` | 2 字节 BE | `int` | `int` | 2 字节 | `Ok(2)` |
| `Int` | 4 字节 BE | `int` | `int` | 4 字节 | `Ok(4)` |
| `Long` | 8 字节 BE | `int` | `int` | 8 字节 | `Ok(8)` |
| `Float16b` | 2 字节 BE | `float`（f64） | `float` | 2 字节 | `Ok(2)` |
| `Float32b` | 4 字节 BE | `float` | `float` | 4 字节 | `Ok(4)` |
| `Float64b` | 8 字节 BE | `float` | `float` | 8 字节 | `Ok(8)` |
| `Int24ub` | 3 字节 BE | `int`（0-16777215） | `int` | 3 字节 | `Ok(3)` |
| `VarInt` | 变长 LEB128 | `int`（≥ 0） | `int`（≥ 0） | 变长 | `Err(SizeofError)` |
| `ZigZag` | 变长 LEB128 | `int`（有符号） | `int` | 变长 | `Err(SizeofError)` |
| `BytesInteger(n)` | n 字节 | `int` | `int` | n 字节 | `Ok(n)` |

---

## §3 §0 原则对照表（L-01 对策，硬要求）

> **逐条对照 `AGENTS.md §0` 八条核心原则**。涉及 parse/build 数据流的设计必须提供此表，
> 未提供或违反的设计会被 REV 直接驳回。

| §0 原则 | 类别 A（别名 8 个） | 类别 B（Float 6 个） | 类别 C（VarInt/ZigZag） | 类别 D（BytesInteger） |
|---------|------------------|-------------------|---------------------|---------------------|
| **#1 一次 FFI** | ✅ Python alias 直接复用 FormatFieldNode/BytesIntegerNode，parse/build 各 1 次 FFI | ✅ parse 末尾 `into_py(py)` 创建 PyFloat（1 次内联 FFI）；build 开头 extract 链（f32/f64→i64→u64，首次成功即停，正常 Python float 仅 1 次 extract；**P2 v2 加 int 兼容**）。extract 链是对同一输入 obj 的类型探测，非数据流多次边界穿越，不违反 §0 #1 | ✅ VarInt parse 末尾 `result.into_py(py)`（1 次）；build fast-path 零 Python 调用，slow-path 1 次 `call1` | ✅ fast-path parse 1 次 `into_py`，build 1 次 `extract`；slow-path 1 次 `call_method`（仍单次 FFI） |
| **#2 无中间表示层** | ✅ 直接 bytes ↔ PyLong，无 Rust 中间类型 | ✅ `f32::from_be_bytes` 直接 → PyFloat（pyo3 内联）；Float16 用 `half::f16` 仅作位运算载体，非跨 FFI 中间类型 | ✅ fast-path `u64` 仅作位运算载体，非跨 FFI 返回；slow-path `bytes` 是数据本身 | ✅ fast-path `u64/i64` 是 Rust 原生整数，直接 ↔ PyLong；slow-path `bytes` 直接 ↔ PyLong，无 Rust 大整数中间类型（不引入 num-bigint） |
| **#3 输入输出侧无抽象 trait** | ✅ 无新 trait，FormatFieldNode/BytesIntegerNode 现有 trait 已覆盖 | ✅ 同左 | ✅ VarIntNode/ZigZagNode 直接 impl Construct，无新 trait | ✅ 同左 |
| **#4 pyo3 核心依赖** | ✅ 全部用 pyo3（into_py / extract / call_method） | ✅ 同左 + `half` crate 仅作纯 Rust 计算库（不跨 FFI） | ✅ fast-path 用 pyo3；slow-path 用 pyo3 `call1`/`call_method` 调 Python callable | ✅ slow-path 用 pyo3 `call_method("from_bytes"/"to_bytes")`，无 raw FFI |
| **#5 mashumaro 式 API** | ✅ 用户面 `@dataclass class P(StructMixin)` 用 `field(Float32b)` 不变 | ✅ 同左 | ✅ 同左 | ✅ 同左 |
| **#6 enum_dispatch 静态分派** | ✅ 别名复用现有 Node 变体，无新分派开销 | ✅ FormatFieldNode 现有分派，+6 Float 分支不影响分派结构 | ✅ Node enum +3 变体（VarInt/ZigZag/BytesInteger），编译期生成 match 分派 | ✅ 同左 |
| **#7 Result<T, ConstructError> + path** | ✅ 复用现有 FormatFieldError/IntegerError | ✅ Float 范围错误（如 f32 溢出）返回 FormatFieldError（与 Python struct.error 对齐） | ✅ VarInt 负数返回 IntegerError；ZigZag 非 int 返回 IntegerError | ✅ BytesInteger length=0 / 值超范围 / 非 int 返回 IntegerError（与 Python 对齐） |
| **#8 Stream 抽象纯 Rust 内部** | ✅ ParseStream/BuildStream 现有抽象 | ✅ 同左 | ✅ VarInt 循环调 `stream.read(1, path)` / `stream.write(&buf[..len])`，纯 Rust | ✅ BytesInteger fast/slow path 均用 ParseStream/BuildStream，slow-path 的 `PyBytes::new` 创建后立即 `as_bytes()` 借用 |

### 3.1 §0 #2 澄清：`half::f16` / `u64` / `bytes` 不是"中间表示层"

L-01 教训指出"中间表示层"指**跨 FFI 返回的 Rust enum / dict / per-field trait 抽象层**。本设计中：

- **`half::f16`**：仅作 Rust 内部位运算载体（`from_be_bytes` → `to_f64` → `into_py`），不跨 FFI 返回。等价于 `u16` 位拼接，仅是更安全的语法糖。
- **`u64` / `i64`**：Rust 原生整数，pyo3 直接转换为 PyLong（CPython 小整数缓存路径），无中间转换。
- **`bytes`（slow-path）**：bytes 是数据本身（不是"中间表示"），`PyBytes::new` 创建 Python bytes 对象后立即传给 `int.from_bytes`，无 Rust 中间枚举。

这与 L-01 反例（"解析结果先构造为 Rust enum 再逐个转 Python 对象"）有本质区别：本设计 parse 末尾直接 `into_py` 产出 Python 对象，无 Rust 中间枚举累积。

### 3.2 §0 #4 澄清：`half` crate 是"纯 Rust 计算依赖"，不破坏 pyo3 核心地位

`half` crate 仅提供 `f16` 类型的位运算（`from_be_bytes` / `to_f64` / `from_f64`），不涉及 FFI / Python 互操作。所有跨 Python 边界操作仍由 pyo3 处理。这与项目已用的 `thiserror` / `enum_dispatch` 同性质（纯 Rust 工具库），不违反 §0 #4。

---

## §4 性能假设（L-02 / L-05 对策）

> **量级参考，非精确值**（L-09 教训：ns 级效应常在测量噪声内）。
> 实测由 6.1 VET 用 `bench/bench_primitives.py` 完成（Controlled A/B Test + 同会话测量）。

### 4.1 瓶颈识别（量化数据 + 来源）

| 构造器类别 | Python 端主要开销 | 来源 / 量级 |
|----------|----------------|-----------|
| Byte/Short/Int/Long 别名 | Python struct.unpack（~600-800ns） + stream_read 调用（~200ns） | Phase 1 B1 基线（`docs/perf-scenarios.csv`：B1 parse py_ns ≈ 3123ns，含 FormatField 全开销） |
| Float32/64 | Python struct.unpack（~600-800ns）+ float 装箱 | 与 Int 同量级（struct.unpack 内部统一路径） |
| Float16 | Python struct.unpack 'e' 格式（CPython 3.6+ 支持，~700-900ns） | 略慢于 Float32（'e' 路径较新，优化较少） |
| Int24 | Python BytesInteger._parse（stream_read + bytes2integer 调 `int.from_bytes`）≈ 800-1200ns | Python BytesInteger 调 `int.from_bytes`（~300-500ns）+ stream_read（~200ns）+ Python 层 swapbytes（~100ns） |
| VarInt | Python 循环 `stream_read(1)` 每字节（~250-400ns/字节） + `acc.append` + `reversed` + `num << 7` | 平均 1.5 字节（小整数）≈ 400-700ns；大整数（10 字节）≈ 2500-4000ns |
| ZigZag | VarInt 开销 + 一次 XOR/shift（~50ns） | 与 VarInt 同量级 |
| BytesInteger ≤ 8 字节 | Python `int.from_bytes`（~300-500ns）+ stream_read | 与 Int24 同量级 |
| BytesInteger > 8 字节 | Python `int.from_bytes` 大整数（~500-2000ns，随长度增长） | 罕见场景 |

### 4.2 可证伪预测（覆盖所有 FFI/拷贝/转换来源，L-05 对策）

| 构造器 | Rust 端预期开销 | 预期加速比（vs Python） | FFI/拷贝/转换来源清单 |
|--------|--------------|-------------------|------------------|
| Byte/Short/Int/Long | ~30-50ns（FormatField fast-path） | **~10-15x**（与 Int8ub 一致） | (1) `stream.read` 1 次；(2) `from_be_bytes` 内联；(3) `into_py` 创建 PyLong |
| Float32/64 | ~30-50ns | **~10-15x** | (1) `stream.read` 1 次；(2) `f32::from_be_bytes` 内联；(3) `into_py` 创建 PyFloat |
| Float16 | ~40-60ns | **~10-15x** | (1) `stream.read` 1 次；(2) `half::f16::from_be_bytes` + `to_f64`（编译期内联）；(3) `into_py` 创建 PyFloat |
| Int24 | ~40-60ns（BytesInteger fast-path） | **~10-15x** | (1) `stream.read` 1 次；(2) buf 补齐 + `from_be_bytes` + 符号扩展；(3) `into_py` 创建 PyLong |
| VarInt（fast-path，u64 范围） | ~50-100ns（取决于字节数） | **~10-20x**（小整数更高） | (1) 循环 `stream.read(1)` 1-10 次；(2) 位运算（内联）；(3) `into_py` 创建 PyLong |
| ZigZag（fast-path） | ~60-120ns | **~10-15x** | 同 VarInt + (4) XOR/shift（内联） |
| BytesInteger ≤ 8 字节 | ~40-60ns | **~10-15x** | (1) `stream.read`；(2) buf 补齐 + endian 转换；(3) `into_py` |
| BytesInteger > 8 字节（slow-path） | ~150-400ns | **~5-8x**（slow-path 受 `call_method` 开销稀释） | (1) `stream.read`；(2) `PyBytes::new`；(3) `call_method("from_bytes")` 跨 FFI ~80ns；(4) Python int.from_bytes 执行 |

**L-05 对策自检**：上表"FFI/拷贝/转换来源清单"列覆盖了每个构造器的所有 FFI 来源（stream 操作 + into_py/extract + Python callable 调用）。REV 检视时确认每个来源都在预测中有对应声明。

### 4.3 预测的关键风险点

| 风险 | 概率 | 影响构造器 | 缓解措施 |
|------|-----|---------|---------|
| Float16 `half` crate 未如预期内联 | 低（half crate 文档明确 const fn） | Float16b/l | VET 实测 Float16 vs Float32 加速比应接近（差异 < 2x） |
| VarInt 循环 `stream.read(1)` 每字节 FFI 开销累积 | 中 | VarInt（大整数） | VET 实测 1 字节 vs 10 字节 VarInt 加速比应均 ≥10x |
| BytesInteger slow-path `call_method` 开销 > 100ns | 中 | BytesInteger > 8 字节 | 接受 ≥5x 加速比（罕见场景，不阻塞 ACCEPT） |
| Int24 符号扩展逻辑错误（罕见边界） | 低 | Int24sb/sl | parity 测试覆盖 -1 / 最小负数 / 最大正数（§7 case I24-3/4/5） |

### 4.4 性能门禁（VET 验收用）

> 与 `docs/perf-scenarios.csv` 现有场景对齐。Phase 6.1 新增 bench 场景：

| 场景 | 描述 | 加速比门禁 | 来源 |
|------|------|---------|------|
| `F32-parse` | Float32b parse 单字段 Struct | ≥ 10x | 与 B1（Int8ub parse）对齐 |
| `F64-parse` | Float64b parse 单字段 Struct | ≥ 10x | 同上 |
| `F16-parse` | Float16b parse 单字段 Struct | ≥ 8x（half crate 不可控因素，软目标） | 略低于 F32/F64 |
| `I24-parse` | Int24ub parse 单字段 Struct | ≥ 10x | BytesInteger fast-path |
| `VARINT-parse` | VarInt parse（小整数 1 字节） | ≥ 10x | LEB128 fast-path |
| `VARINT-build` | VarInt build（小整数） | ≥ 10x | LEB128 编码 |
| `BYTESINT-slow` | BytesInteger(16) parse（slow-path） | ≥ 4x（用户硬约束 #5 下限） | slow-path call_method 稀释 |

**未达标处理**（L-02 对策）：
- F32/F64/I24/VARINT < 10x：暂停 6.1 ACCEPT，ARCH 重新分析瓶颈（参考 Phase 5 FFI 优化模式）
- F16 < 8x：评估 half crate 是否被未内联，必要时改手写 bit 拼接（D-1 备选方案）
- BYTESINT-slow < 4x：评估改用 raw FFI（D-2 备选方案，需 PM 决策接受 unsafe）

---

## §5 边界条件清单（17 个构造器全覆盖）

> 从 Python 原版 `core.py:1124-1686` + `lib/binary.py:38-92` 提取所有边界条件。
> DEV 实施时每条边界条件必须有对应 unit/parity 测试（§7 模板已覆盖）。

### 5.1 整数别名（Byte/Short/Int/Long）

| # | 输入 | 预期行为 | Python 行号 |
|---|------|---------|-----------|
| BC-A1 | `Byte.build(0)` | 写入 `b'\x00'` | core.py:1524 |
| BC-A2 | `Byte.build(255)` | 写入 `b'\xFF'` | 同上 |
| BC-A3 | `Byte.build(256)` | IntegerError（256 > u8::MAX） | format_field.rs build_small_int try_into 失败 |
| BC-A4 | `Byte.build(-1)` | IntegerError（无符号不接受负数） | 同上 |
| BC-A5 | `Long.build(2**64)` | IntegerError（超出 u64 范围） | obj.extract::<u64> 失败 |
| BC-A6 | `Long.build(2**64 - 1)` | 写入 `b'\xFF' * 8` | 同上 |

### 5.2 Float 系列

| # | 输入 | 预期行为 | Python 行号 |
|---|------|---------|-----------|
| BC-B1 | `Float32b.parse(b'\x42\x28\x00\x00')` | 返回 `42.0` | core.py:1543 |
| BC-B2 | `Float32b.parse(b'\xC2\x28\x00\x00')` | 返回 `-42.0` | 同上 |
| BC-B3 | `Float32b.parse(b'\x00\x00\x00\x00')` | 返回 `0.0`（+0） | IEEE 754 |
| BC-B4 | `Float32b.parse(b'\x80\x00\x00\x00')` | 返回 `-0.0`（-0，与 Python 一致） | IEEE 754 |
| BC-B5 | `Float32b.parse(b'\x7F\xC0\x00\x00')` | 返回 `float('nan')`（quiet NaN） | IEEE 754 |
| BC-B6 | `Float32b.parse(b'\x7F\x80\x00\x00')` | 返回 `float('inf')` | IEEE 754 |
| BC-B7 | `Float32b.parse(b'\xFF\x80\x00\x00')` | 返回 `float('-inf')` | IEEE 754 |
| BC-B8 | `Float32b.build(1e40)` | FormatFieldError（1e40 超 f32 范围） | P2 v2：`extract::<f32>` 触发 pyo3 OverflowError（对齐 `struct.pack('>f',1e40)` OverflowError→construct FormatFieldError，core.py:1169 try/except） |
| BC-B9 | `Float32b.build(float('nan'))` | 写入 `b'\x7F\xC0\x00\x00'`（quiet NaN） | half::f16::from_f64(NaN) |
| BC-B10 | `Float16b.parse(b'\x3C\x00')` | 返回 `1.0`（f16 表示） | half::f16::from_be_bytes |
| BC-B11 | `Float16b.parse(b'\x03\xFF')` | 返回 subnormal 数（~6.1e-5） | half::f16 完整支持 subnormal |
| BC-B12 | `Float16b.build(70000.0)` | FormatFieldError（70000 超 f16 max 65504） | **P2 v2 修正**：v1 误判"返回 inf"。实测 `struct.pack('>e',70000)` OverflowError→construct FormatFieldError。Rust 手动 `val.is_finite() && val.abs() > 65504.0` 检查 |
| BC-B13 | `Float64b.parse(b'\x3F\xF0\x00\x00\x00\x00\x00\x00')` | 返回 `1.0` | IEEE 754 |
| BC-B14 | `Float32b.build("not a float")` | FormatFieldError（非 float 非 int） | P2 v2：extract f32/i64/u64 全失败（i64/u64 fallback 后仍失败才报错） |
| BC-B15 | `Float32b.build(42)` | 写入 `b'\x42\x28\x00\x00'`（int→f32，42.0） | P2 v2 新增：实测 `struct.pack('>f',42)==42280000`；extract f32 失败→i64 as f32 |
| BC-B16 | `Float16b.build(42)` | 写入 `b'\x51\x40'`（int→f16，42.0） | P2 v2 新增：实测 `struct.pack('>e',42)==5140`；extract f64 失败→i64 as f64→`from_f64` |
| BC-B17 | `Float64b.build(42)` | 写入 `b'\x40\x45\x00\x00\x00\x00\x00\x00'`（int→f64，42.0） | P2 v2 新增：实测 `struct.pack('>d',42)==4045000000000000` |

### 5.3 Int24 系列

| # | 输入 | 预期行为 | Python 行号 |
|---|------|---------|-----------|
| BC-C1 | `Int24ub.parse(b'\x00\x00\x01')` | 返回 `1` | BytesInteger fast-path big endian |
| BC-C2 | `Int24ub.parse(b'\xFF\xFF\xFF')` | 返回 `16777215`（最大无符号） | 同上 |
| BC-C3 | `Int24sb.parse(b'\xFF\xFF\xFF')` | 返回 `-1`（符号扩展） | BytesInteger fast-path signed big endian |
| BC-C4 | `Int24sb.parse(b'\x80\x00\x00')` | 返回 `-8388608`（最小有符号） | 同上 |
| BC-C5 | `Int24ul.parse(b'\x01\x00\x00')` | 返回 `1`（little endian） | BytesInteger fast-path swapped |
| BC-C6 | `Int24ub.build(16777215)` | 写入 `b'\xFF\xFF\xFF'` | BytesInteger build fast-path |
| BC-C7 | `Int24ub.build(16777216)` | IntegerError（超 3 字节无符号范围） | check_range 失败 |
| BC-C8 | `Int24ub.build(-1)` | IntegerError（无符号不接受负数） | 同上 |

### 5.4 VarInt

| # | 输入 | 预期行为 | Python 行号 |
|---|------|---------|-----------|
| BC-D1 | `VarInt.parse(b'\x01')` | 返回 `1`（单字节） | LEB128 fast-path |
| BC-D2 | `VarInt.parse(b'\x80\x01')` | 返回 `128`（双字节：0x80 续位 + 0x01） | 同上 |
| BC-D3 | `VarInt.parse(b'\xFF\xFF\xFF\xFF\x0F')` | 返回 `268435455`（5 字节，u32 最大） | LEB128 |
| BC-D4 | `VarInt.build(0)` | 写入 `b'\x00'` | LEB128 编码 |
| BC-D5 | `VarInt.build(128)` | 写入 `b'\x80\x01'` | 同上 |
| BC-D6 | `VarInt.build(-1)` | IntegerError（VarInt 拒绝负数，错误信息含 `-1`） | core.py:1635-1636 |
| BC-D7 | `VarInt.build("not int")` | IntegerError（值不是整数） | core.py:1633-1634 |
| BC-D8 | `VarInt.build(2**100)` | 写入 15 字节（slow-path：Python 大整数编码） | core.py:1617 示例 |
| BC-D9 | `VarInt.sizeof()` | SizeofError（变长） | Python 无 sizeof，但 Rust 侧返回 Err 与 Python 抛 SizeofError 等价 |

### 5.5 ZigZag

| # | 输入 | 预期行为 | Python 行号 |
|---|------|---------|-----------|
| BC-E1 | `ZigZag.parse(b'\x00')` | 返回 `0`（zz(0) = 0） | core.py:1670-1676 |
| BC-E2 | `ZigZag.parse(b'\x01')` | 返回 `-1`（zz(-1) = 1） | 同上 |
| BC-E3 | `ZigZag.parse(b'\x02')` | 返回 `1`（zz(1) = 2） | 同上 |
| BC-E4 | `ZigZag.build(-3)` | 写入 `b'\x05'`（zz(-3) = 5） | core.py:1678-1685 |
| BC-E5 | `ZigZag.build(3)` | 写入 `b'\x06'`（zz(3) = 6） | 同上 |
| BC-E6 | `ZigZag.build("not int")` | IntegerError | core.py:1679-1680 |

### 5.6 BytesInteger

| # | 输入 | 预期行为 | Python 行号 |
|---|------|---------|-----------|
| BC-F1 | `BytesInteger(4).parse(b'\x00\x00\x00\x13')` | 返回 `19` | core.py:1232 |
| BC-F2 | `BytesInteger(4).build(1)` | 写入 `b'\x00\x00\x00\x01'` | core.py:1235 |
| BC-F3 | `BytesInteger(4, signed=True).parse(b'\xFF\xFF\xFF\xFF')` | 返回 `-1` | fast-path signed |
| BC-F4 | `BytesInteger(4, swapped=True).parse(b'\x01\x00\x00\x00')` | 返回 `1` | fast-path swapped |
| BC-F5 | `BytesInteger(0)` | IntegerError（length 必须 > 0） | core.py:1248-1249 |
| BC-F6 | `BytesInteger(4).build("not int")` | IntegerError | core.py:1259-1260 |
| BC-F7 | `BytesInteger(4).build(2**32)` | IntegerError（超 4 字节无符号范围） | check_range 失败 |
| BC-F8 | `BytesInteger(16).parse(b'\x00' * 16)` | 返回 `0`（slow-path，16 字节 > 8） | parse_bigint_from_bytes |
| BC-F9 | `BytesInteger(16).build(2**128)` | IntegerError（超 16 字节范围） | Python to_bytes 触发 OverflowError → IntegerError |
| BC-F10 | `BytesInteger(16).build(2**64)` | 写入 16 字节（slow-path） | build_bigint_to_bytes |
| BC-F11 | `BytesInteger(1, swapped=True)` | 等价 `BytesInteger(1, swapped=False)`（单字节 endian 无意义） | fast-path 处理 |

---

## §6 DEV 实施清单 + 子任务拆分

### 6.1 实施顺序（5 个顺序步骤，依赖图）

```
步骤 1: Cargo.toml 加 half 依赖 + PythonFormat enum 扩展（Float 6 变体）
        ↓
步骤 2: FormatFieldNode.parse/build 加 Float 分支 + 单元测试
        ↓
步骤 3: BytesIntegerNode（fast + slow path）+ 单元测试
        ↓
步骤 4: VarIntNode + ZigZagNode + 单元测试
        ↓
步骤 5: Python 层 alias（Byte/Short/Int/Long + Int24 + Float 单例 + VarInt/ZigZag 单例）
        + Node enum 注册 + parity 测试
```

**为何此顺序**：
- 步骤 1-2 是 Float（独立路径，不依赖其他新 Node）
- 步骤 3 BytesIntegerNode 是 Int24 别名的前置（步骤 5 依赖）
- 步骤 4 VarIntNode 是 ZigZagNode 的前置（ZigZag 复用 VarInt 字节编解码）
- 步骤 5 是用户面 alias + 集成测试，依赖所有 Rust 侧就绪

### 6.2 每步骤工作量估算

| 步骤 | Rust 工作量 | Python 工作量 | 测试工作量 | 总计 |
|------|----------|-------------|---------|-----|
| 1. half 依赖 + PythonFormat 扩展 | ~50 行（enum + from_chars + byte_length + fmtstr） | 0 | ~30 行（unit test） | 0.5 天 |
| 2. FormatFieldNode Float 分支 | ~100 行（parse 6 分支 + build 6 分支） | 0 | ~100 行（unit + parity） | 1 天 |
| 3. BytesIntegerNode | ~150 行（struct + parse 双路径 + build 双路径 + check_range） | 0 | ~120 行（unit + parity） | 1.5 天 |
| 4. VarIntNode + ZigZagNode | ~140 行（VarInt 80 + ZigZag 60） | 0 | ~100 行（unit + parity） | 1 天 |
| 5. Python alias + Node 注册 + parity 集成 | ~10 行（mod.rs use + enum + has_expressions） | ~50 行（singleton 定义） | ~150 行（parity 文件） | 1 天 |
| **合计** | **~450 行 Rust** | **~50 行 Python** | **~500 行测试** | **~5 天** |

### 6.3 文件清单（DEV 实施时创建/修改）

| 文件 | 类型 | 改动 |
|------|------|------|
| `construct-rs/Cargo.toml` | 修改 | +1 行 `half = "2.4"` |
| `construct-rs/src/nodes/format_field.rs` | 修改 | PythonFormat +6 变体 + from_chars/byte_length/fmtstr 扩展 + parse/build Float 分支 |
| `construct-rs/src/nodes/varint.rs` | **新建** | VarIntNode + build_varint_bigint |
| `construct-rs/src/nodes/zigzag.rs` | **新建** | ZigZagNode（复用 VarIntNode） |
| `construct-rs/src/nodes/bytes_integer.rs` | **新建** | BytesIntegerNode + 2 slow-path helper + check_range |
| `construct-rs/src/nodes/mod.rs` | 修改 | +3 use 语句 + Node enum +3 变体 |
| `construct-rs/python/construct/_primitives.py` | 修改或新建 | 单例定义（Byte/Short/Int/Long + Float + Int24 + VarInt/ZigZag） |
| `construct-rs/python/construct/__init__.py` | 修改 | 导出新单例 |
| `construct-rs/tests/parity/test_phase6_primitives_parity.py` | **新建** | 17 个构造器 parity 测试（用 6.0 helper，§7 模板） |
| `construct-rs/tests/unit/test_format_field_float.rs` 或 `.py` | **新建** | Float 系列 unit 测试（pyo3 round-trip） |
| `construct-rs/bench/bench_primitives.py` | **新建** | 性能基准（用 6.0 BenchRunner，§7.2 模板） |
| `docs/constructors-inventory.csv` | 修改 | +17 骨架行（ARCH 已追加，见 §8） |

### 6.4 DEV 自检清单（提交前必过）

- [ ] `cargo build` 编译通过
- [ ] `cargo clippy` 零 warning（含 half crate 调用）
- [ ] `cargo fmt --check` 格式正确
- [ ] `cargo test` 全 PASS（含新 unit 测试）
- [ ] `pytest tests/parity/test_phase6_primitives_parity.py -v` 全 17 构造器 × 多 case PASS
- [ ] `python bench/bench_primitives.py` 输出加速比 ≥ §4.4 门禁
- [ ] Python `from construct import *` 可导入全部新单例
- [ ] parity 测试 NaN/Inf 由 normalize 统一标记（不需手动标记，参考测试框架设计 §5.1.3）

---

## §7 parity 测试模板（用 6.0 新框架）

### 7.1 parity 文件结构（DEV 照抄 §5.1.1 模板）

文件路径：`construct-rs/tests/parity/test_phase6_primitives_parity.py`

```python
# tests/parity/test_phase6_primitives_parity.py
"""Phase 6.1 Primitives 构造器 Python 行为一致性测试。

设计依据：
- docs/design/基础设施/测试框架设计.md §5.1（统一模板）
- docs/design/模块设计/模块设计-Primitives收尾.md §5（边界条件清单）
- _helpers/parity.py（公共组件）

覆盖：17 个构造器（Byte/Short/Int/Long + Float32/64/16 b/l + Int24ub/ul/sb/sl
      + VarInt + ZigZag + BytesInteger）

用法：
    pytest tests/parity/test_phase6_primitives_parity.py -v
    python tests/parity/test_phase6_primitives_parity.py
"""

from __future__ import annotations
import sys
from pathlib import Path

_TESTSDIR = Path(__file__).resolve().parent.parent
if str(_TESTSDIR) not in sys.path:
    sys.path.insert(0, str(_TESTSDIR))

from _helpers.parity import (
    make_parity_results_fixture,
    assert_parity,
    assert_fidelity,
)

# ===== case 定义（子进程脚本片段） =====
_CASE_DEFINITIONS = r'''
def _make_case_rs(case_id):
    from dataclasses import dataclass
    from construct import (StructMixin, field, Byte, Short, Int, Long,
        Float16b, Float16l, Float32b, Float32l, Float64b, Float64l,
        Int24ub, Int24ul, Int24sb, Int24sl, VarInt, ZigZag, BytesInteger)

    if case_id == 'B-1':   # Byte
        @dataclass
        class P(StructMixin):
            v: int = field(Byte)
        return P, b"\x42", lambda: P(v=66), lambda o: {'v': o.v}
    if case_id == 'F32-1':   # Float32b 正数
        @dataclass
        class P(StructMixin):
            v: float = field(Float32b)
        return P, b"\x42\x28\x00\x00", lambda: P(v=42.0), lambda o: {'v': o.v}
    if case_id == 'F32-4':   # Float32b NaN（normalize 统一标记，case 不手动标记）
        @dataclass
        class P(StructMixin):
            v: float = field(Float32b)
        return P, b"\x7f\xc0\x00\x00", lambda: P(v=float('nan')), \
            lambda o: {'v': o.v}
    if case_id == 'F16-1':   # Float16b 基本
        @dataclass
        class P(StructMixin):
            v: float = field(Float16b)
        return P, b"\x3c\x00", lambda: P(v=1.0), lambda o: {'v': o.v}
    if case_id == 'I24-1':   # Int24ub
        @dataclass
        class P(StructMixin):
            v: int = field(Int24ub)
        return P, b"\x00\xff\xff", lambda: P(v=65535), lambda o: {'v': o.v}
    if case_id == 'I24-3':   # Int24sb -1（符号扩展）
        @dataclass
        class P(StructMixin):
            v: int = field(Int24sb)
        return P, b"\xff\xff\xff", lambda: P(v=-1), lambda o: {'v': o.v}
    if case_id == 'VAR-1':   # VarInt 小整数
        @dataclass
        class P(StructMixin):
            v: int = field(VarInt)
        return P, b"\x01", lambda: P(v=1), lambda o: {'v': o.v}
    if case_id == 'VAR-2':   # VarInt 多字节
        @dataclass
        class P(StructMixin):
            v: int = field(VarInt)
        return P, b"\x80\x01", lambda: P(v=128), lambda o: {'v': o.v}
    if case_id == 'ZZ-1':    # ZigZag 负数
        @dataclass
        class P(StructMixin):
            v: int = field(ZigZag)
        return P, b"\x05", lambda: P(v=-3), lambda o: {'v': o.v}
    if case_id == 'BI-1':    # BytesInteger fast-path
        @dataclass
        class P(StructMixin):
            v: int = field(BytesInteger(4))
        return P, b"\x00\x00\x00\x13", lambda: P(v=19), lambda o: {'v': o.v}
    if case_id == 'BI-2':    # BytesInteger slow-path（16 字节）
        @dataclass
        class P(StructMixin):
            v: int = field(BytesInteger(16))
        # 2**64 in 16 bytes big-endian
        return P, b"\x00\x00\x00\x00\x00\x00\x00\x00\x01\x00\x00\x00\x00\x00\x00\x00", \
            lambda: P(v=2**128), lambda o: {'v': o.v}
    raise ValueError('unknown case (rs): ' + case_id)


def _make_case_py(case_id):
    import construct as pc
    if case_id == 'B-1':
        return (pc.Struct("v"/pc.Byte), b"\x42", dict(v=66))
    if case_id == 'F32-1':
        return (pc.Struct("v"/pc.Float32b), b"\x42\x28\x00\x00", dict(v=42.0))
    if case_id == 'F32-4':
        return (pc.Struct("v"/pc.Float32b), b"\x7f\xc0\x00\x00", dict(v=float('nan')))
    if case_id == 'F16-1':
        return (pc.Struct("v"/pc.Float16b), b"\x3c\x00", dict(v=1.0))
    if case_id == 'I24-1':
        return (pc.Struct("v"/pc.Int24ub), b"\x00\xff\xff", dict(v=65535))
    if case_id == 'I24-3':
        return (pc.Struct("v"/pc.Int24sb), b"\xff\xff\xff", dict(v=-1))
    if case_id == 'VAR-1':
        return (pc.VarInt, b"\x01", 1)
    if case_id == 'VAR-2':
        return (pc.VarInt, b"\x80\x01", 128)
    if case_id == 'ZZ-1':
        return (pc.ZigZag, b"\x05", -3)
    if case_id == 'BI-1':
        return (pc.BytesInteger(4), b"\x00\x00\x00\x13", 19)
    if case_id == 'BI-2':
        return (pc.BytesInteger(16),
                b"\x00\x00\x00\x00\x00\x00\x00\x00\x01\x00\x00\x00\x00\x00\x00\x00",
                2**128)
    raise ValueError('unknown case (py): ' + case_id)
'''

# ===== case 清单（按构造器分组） =====
ALL_CASES = [
    # 整数别名（4）
    ("B-1", "Byte 基本（66）"),
    ("S-1", "Short 基本（256）"),
    ("I-1", "Int 基本（65536）"),
    ("L-1", "Long 基本（2**32）"),
    # Float 系列（6 × 多边界 = ~18 case）
    ("F32-1", "Float32b 正数（42.0）"),
    ("F32-2", "Float32b 负数（-42.0）"),
    ("F32-3", "Float32b ±0"),
    ("F32-4", "Float32b NaN（normalize 统一标记）"),
    ("F32-5", "Float32b ±Infinity（normalize 统一标记）"),
    ("F32-6", "Float32b subnormal"),
    ("F64-1", "Float64b 正数"),
    ("F64-2", "Float64b 精度（π）"),
    ("F16-1", "Float16b 基本（1.0）"),
    ("F16-2", "Float16b subnormal（normalize round(6)）"),
    ("F16-3", "Float16b NaN"),
    # Int24（4 × 边界 = ~8 case）
    ("I24-1", "Int24ub 65535"),
    ("I24-2", "Int24ub 最大（16777215）"),
    ("I24-3", "Int24sb -1（符号扩展）"),
    ("I24-4", "Int24sb 最小（-8388608）"),
    ("I24-5", "Int24ul little endian"),
    # VarInt（边界 case）
    ("VAR-1", "VarInt 单字节（1）"),
    ("VAR-2", "VarInt 多字节（128）"),
    ("VAR-3", "VarInt u32 最大"),
    # ZigZag
    ("ZZ-1", "ZigZag 负数（-3）"),
    ("ZZ-2", "ZigZag 0"),
    ("ZZ-3", "ZigZag 正数（3）"),
    # BytesInteger
    ("BI-1", "BytesInteger(4) fast-path"),
    ("BI-2", "BytesInteger(16) slow-path"),
]

# ===== 自动生成 parity_results fixture =====
parity_results = make_parity_results_fixture(ALL_CASES, _CASE_DEFINITIONS)


# ===== 标准 5 个测试函数（所有 parity 文件统一） =====

def test_parity_all_cases_collected(parity_results):
    """确认所有 case 都成功执行（无子进程错误）。"""
    for case_id, _ in ALL_CASES:
        assert case_id in parity_results
        assert "rs" in parity_results[case_id]
        assert "py" in parity_results[case_id]


# test_parity_parse / test_parity_build / test_parity_roundtrip / test_parity_fidelity
# 用 @pytest.mark.parametrize("case_id,desc", ALL_CASES) 自动展开
# 详细实现参考测试框架设计 §5.1.1
```

### 7.2 bench 文件结构（用 6.0 BenchRunner，§5.2 模板）

文件路径：`construct-rs/bench/bench_primitives.py`

```python
# bench/bench_primitives.py（节选）
_MEASURE_SCRIPTS = {
    "F32-parse": '''<子进程脚本：Float32b parse 单字段 Struct，NUMBER=30000>''',
    "F64-parse": '''<同上，Float64b>''',
    "F16-parse": '''<同上，Float16b>''',
    "I24-parse": '''<同上，Int24ub>''',
    "VARINT-parse": '''<同上，VarInt>''',
    "VARINT-build": '''<VarInt build 单字段>''',
    "BYTESINT-slow": '''<BytesInteger(16) parse，slow-path>''',
}

SCENARIOS = list(_MEASURE_SCRIPTS.keys())
GATES = {
    "F32-parse": 10.0, "F64-parse": 10.0, "F16-parse": 8.0,
    "I24-parse": 10.0, "VARINT-parse": 10.0, "VARINT-build": 10.0,
    "BYTESINT-slow": 4.0,
}
```

### 7.3 case_definitions 字符串转义注意点

参考测试框架设计 §5.3 注意点 1：
- `_CASE_DEFINITIONS` 用 `r'''...'''` raw string，避免 `\\x42` 双重转义
- bytes 字面量 `b"\x42"` 在 raw string 内写为 `b"\x42"`（单层反斜杠）
- Python wrapper 的 `Float('nan')` 在 raw string 内直接写 `float('nan')`
- normalize 的 NaN/Inf 处理由 `_helpers/normalize.py` 统一（测试框架设计 §5.1.3），case 内**不要手动标记**

---

## §8 与其他模块的交互

### 8.1 依赖（本设计读取/复用）

| 依赖 | 类型 | 用途 |
|------|------|------|
| `AGENTS.md §0` | 规范 | 8 条核心原则逐条对照（§3） |
| `harness/experiences.md §L-01` | 教训 | 中间表示层对策（§3 §0 #2 澄清） |
| `harness/experiences.md §L-02` | 教训 | 理论估算替代实证对策（§4 性能假设） |
| `harness/experiences.md §L-05` | 教训 | 优化 A 路径忽略 B 路径（§4.2 FFI 来源清单） |
| `harness/experiences.md §L-09` | 教训 | 跨时段性能对比归因失效（§4 量级参考 + 同会话测量） |
| `ADR-019` | ADR | 错误路径 raw FFI 先例（BytesInteger D-2 决策对比参考） |
| `ADR-016` | ADR | lazy path 错误传播（新 Node parse/build 错误路径继承） |
| `docs/analysis/分析报告-Phase6启动前置.md §B` | 分析 | 17 个构造器实现摘要（输入） |
| `plans/phase6-primitives-strings-adapter/总纲.md` | 决策 | PM 5 项决策（Strings 路线 / Adapter 双层 / Pass / 测试结构 / bench 框架） |
| `docs/design/基础设施/测试框架设计.md §5` | 设计 | parity / bench 统一模板（§7） |
| `construct-rs/src/nodes/format_field.rs` | 现有源码 | PythonFormat enum + FormatFieldNode（Float 扩展基础） |
| `construct-rs/src/nodes/mod.rs` | 现有源码 | Node enum + Construct trait（+3 变体注册） |
| `construct/construct/core.py:1124-1686` | Python 原版 | FormatField / BytesInteger / VarInt / ZigZag 实现 |
| `construct/construct/lib/binary.py:38-92` | Python 原版 | integer2bytes / bytes2integer 工具（slow-path 等价逻辑） |
| `half` crate（新增） | 第三方依赖 | Float16 IEEE 754 半精度（D-1） |

### 8.2 被依赖（下游使用本设计）

| 下游 | 用途 | 接入点 |
|------|------|--------|
| **Phase 6.2 Strings** | PascalString 推荐用 VarInt 作 lengthfield | `VarIntNode` 直接作为 PascalStringNode 的 lengthfield 子 Node |
| **Phase 6.3 Adapter** | Rebuild/RawCopy 可能包装 BytesInteger | `BytesIntegerNode` 作为 inner Node |
| **Phase 7 Streams** | Prefixed 长度字段常用 VarInt | 同 6.2 |
| **Phase 7 Conditional** | If/Switch 默认值 Pass 不依赖本设计 | 无 |
| **后续协议构造器** | 用户协议常用 Float / Int24 / VarInt | 全部 17 个构造器进入用户面 API |
| **perf-scenarios.csv** | Phase 6.1 性能数据归档 | `bench/bench_primitives.py` 输出对齐字段 |
| **constructors-inventory.csv** | 17 个构造器进度更新 | §8.3 ARCH 已追加骨架行 |

### 8.3 constructors-inventory.csv 骨架行（ARCH 已追加）

按 ARCH extension §新构造器设计同步清单要求，ARCH 在设计文档落地时同步追加 inventory.csv 骨架行（仅填 category/name/python_class/status/notes，其余列 PM 在 ACCEPTED 时填）。

**17 个新增骨架行**（已追加到 `docs/constructors-inventory.csv`）：

```csv
Primitives,Byte,FormatField (alias),not_implemented,,,,,,,,"设计见 docs/design/模块设计/模块设计-Primitives收尾.md §1.2.1"
Primitives,Short,FormatField (alias),not_implemented,,,,,,,,"设计见 docs/design/模块设计/模块设计-Primitives收尾.md §1.2.1"
Primitives,Int,FormatField (alias),not_implemented,,,,,,,,"设计见 docs/design/模块设计/模块设计-Primitives收尾.md §1.2.1"
Primitives,Long,FormatField (alias),not_implemented,,,,,,,,"设计见 docs/design/模块设计/模块设计-Primitives收尾.md §1.2.1"
Primitives,Float16b,FormatField,not_implemented,,,,,,,,"设计见 docs/design/模块设计/模块设计-Primitives收尾.md §1.3;half crate"
Primitives,Float16l,FormatField,not_implemented,,,,,,,,"设计见 docs/design/模块设计/模块设计-Primitives收尾.md §1.3;half crate"
Primitives,Float32b,FormatField,not_implemented,,,,,,,,"设计见 docs/design/模块设计/模块设计-Primitives收尾.md §1.3"
Primitives,Float32l,FormatField,not_implemented,,,,,,,,"设计见 docs/design/模块设计/模块设计-Primitives收尾.md §1.3"
Primitives,Float64b,FormatField,not_implemented,,,,,,,,"设计见 docs/design/模块设计/模块设计-Primitives收尾.md §1.3"
Primitives,Float64l,FormatField,not_implemented,,,,,,,,"设计见 docs/design/模块设计/模块设计-Primitives收尾.md §1.3"
Primitives,Int24ub,BytesInteger (alias),not_implemented,,,,,,,,"设计见 docs/design/模块设计/模块设计-Primitives收尾.md §1.2.2"
Primitives,Int24ul,BytesInteger (alias),not_implemented,,,,,,,,"设计见 docs/design/模块设计/模块设计-Primitives收尾.md §1.2.2"
Primitives,Int24sb,BytesInteger (alias),not_implemented,,,,,,,,"设计见 docs/design/模块设计/模块设计-Primitives收尾.md §1.2.2"
Primitives,Int24sl,BytesInteger (alias),not_implemented,,,,,,,,"设计见 docs/design/模块设计/模块设计-Primitives收尾.md §1.2.2"
Primitives,VarInt,VarInt,not_implemented,,,,,,,,"设计见 docs/design/模块设计/模块设计-Primitives收尾.md §1.4.1"
Primitives,ZigZag,ZigZag,not_implemented,,,,,,,,"设计见 docs/design/模块设计/模块设计-Primitives收尾.md §1.4.2"
Primitives,BytesInteger,BytesInteger,not_implemented,,,,,,,,"设计见 docs/design/模块设计/模块设计-Primitives收尾.md §1.5"
```

**Native endian 变体**（`Int24un` / `Int24sn` / `Float16n` / `Float32n` / `Float64n`）：本 phase 不实现（§0.2 已说明），暂不追加 inventory 行；Phase 7+ 评估时再追加 `deferred` 行。

---

## §9 PM 决策点汇总

### 9.1 必须决策（阻塞 6.1 启动）

| # | 决策 | ARCH 推荐 | 影响 | 阻塞？ |
|---|------|----------|------|--------|
| **D-1** | Float16 实现：`half` crate vs 手写 bit 拼接 | **`half` crate**（正确性 + 维护 + 性能三维度均优） | Cargo.toml 新增 1 行依赖；编译产物 +12KB | **是**（DEV 实施步骤 1 前需 PM 确认） |

### 9.2 已由 ARCH 决策（不需 PM 介入，REV 检视时确认）

| # | 决策 | ARCH 决定 | 理由 |
|---|------|----------|------|
| D-2 | BytesInteger 大整数（>8 字节）实现 | Python `int.from_bytes`/`int.to_bytes` 慢路径（无 unsafe） | 罕见场景不值得 unsafe；CPython 公开稳定 API；与 ADR-019 错误路径 raw FFI 区别（性能要求不同） |
| D-3 | Int24 实现 | Python 层 alias 到 `BytesIntegerNode(3, ...)` | 与 Python construct 一致；零 Rust 别名代码 |
| D-4 | Byte/Short/Int/Long 别名 | Python 层 alias 到 FormatFieldNode | 与 Python construct 一致；零 Rust 改动 |
| D-5 | VarInt / ZigZag sizeof | 返回 `Err(SizeofError)` | 变长字段，与 Python 一致 |
| D-6 | Native endian 变体（Float*n / Int2*n） | 本 phase 不实现，Phase 7+ 评估 | 跨平台协议无意义，用户面极罕见 |

### 9.3 设计质疑响应预案

若 REV/DEV/VET 在检视/实施中提出质疑，ARCH 按以下预案响应：

| 可能质疑 | ARCH 响应预案 |
|---------|------------|
| "为何不引入 num-bigint 处理大整数？" | 引用 §1.5.1 D-2 决策表：num-bigint 是中间类型（违反 §0 #2），且性能未必优于 Python int.from_bytes |
| "为何 ZigZag 不组合 VarIntNode（用 Box<Node>）？" | 引用 §1.4.2：组合 Node 引入额外 dispatch 开销，ZigZag 仅多一次 XOR/shift，直接复用 VarIntNode 字节编解码更高效 |
| "Float16 用 half crate 引入新依赖，是否违反 §0 #4？" | 引用 §3.2 澄清：half 是纯 Rust 计算库（与 thiserror/enum_dispatch 同性质），不破坏 pyo3 核心地位 |
| "BytesInteger slow-path 用 call_method 是否违反 §0 #1 一次 FFI？" | 引用 §3 §0 #1：slow-path 仍单次 FFI（call_method 算 1 次），与 fast-path 的 into_py 同为单次 FFI，仅开销略高（~80ns vs ~20ns） |
| "VarInt > 2^64 slow-path 用 pyo3 run_bound 执行 Python 代码是否过度？" | **v2 已修正（P3）**：§1.4.1 改为 `VARINT_ENCODER` GILOnceCell 缓存编译 `_varint_encode` 函数（首次编译，后续复用，与 error.rs `EXCEPTIONS` 同模式）。**不能用 `obj.call_method("to_bytes")` 统一**——VarInt 是 LEB128（每 7 位 + MSB 续位），`int.to_bytes` 是直接转 N 字节大端，编码逻辑不等价（原 v1 预案的技术错误已删除） |

---

## §10 与历史决策的关系

### 10.1 与 ADR-019（错误路径 raw FFI）的对比

| 维度 | ADR-019（错误路径） | 本设计 D-2（BytesInteger slow-path） |
|------|------------------|--------------------------------|
| 路径类型 | 错误路径（罕见但性能硬约束 ≥10x） | 数据路径 slow-path（罕见且性能软约束 ≥4x） |
| FFI 方式 | unsafe raw C API（PyType_GenericAlloc 等） | pyo3 `call_method`（公开稳定 API） |
| 性能要求 | ≥10x（用户硬约束 #5） | ≥4x（用户硬约束 #5 下限） |
| unsafe？ | 是（4 个 unsafe 块，SAFETY 注释） | **否** |
| 决策依据 | 错误路径性能不达标需 unsafe 换性能 | 数据路径 slow-path 罕见，不值得 unsafe |

**结论**：本设计**不延用** ADR-019 的 unsafe raw FFI 模式，因 BytesInteger slow-path 性能要求宽松（≥4x），且罕见场景不值得引入 unsafe 风险。这是"按场景选择工具"原则的体现。

### 10.2 与 Phase 5 FFI 入口优化设计的关系

Phase 5 设计（`模块设计-FFI入口优化.md`）的 A+B+C 优化针对 Struct 入口开销（一次 FFI 边界穿越）。本设计的 17 个构造器作为 Struct 字段的 inner Node，**直接受益于 Phase 5 优化**（parse/build 各一次 FFI 入口，inner Node 在 Rust 内部零 FFI）。无需本设计重复优化 FFI 入口。

### 10.3 与 Phase 6.0 测试框架设计的关系

本设计 §7 parity / bench 模板严格遵循 6.0 测试框架设计的 §5.1 / §5.2 统一模板。DEV 实施 6.1 时**直接套用** 6.0 helper（`run_parity_case` / `make_parity_results_fixture` / `BenchRunner`），无需重写模板代码。

---

> **设计完成时间**：v1 2026-07-30 / **v2 修订** 2026-07-30
> **下一步**：6.1 v2 设计复检（REV 确认 P1/P2/P3/P4 修正到位）→ 通过则 PM 决策 D-1（half crate）→ DEV 实施（按 §6 步骤顺序）
> **未覆盖项**：Native endian 变体（D-6，Phase 7+ 评估）；BytesInteger context lambda length（Phase 7 Streams 表达式路径）

---

## §11 v2 修订记录（REV v1 驳回后修正）

**驳回依据**：`plans/phase6-primitives-strings-adapter/traces/6.1-REV检视.md`（P1/P2/P3 必须修正，P4 建议）

### P1：错误类型映射统一 `ConstructError::Integer`（高严重度，已修正）

**问题**：v1 在 §1.4.1 VarInt 负数 / §1.4.2 ZigZag 非 int / §1.5.3 BytesInteger length==0 / slow-path 多处用 `ConstructError::FormatField`，Python 原版抛 `IntegerError`，导致 Python 侧 `except IntegerError` 捕获不到，破坏用户面 parity。v1 §3 对照表写"IntegerError"但代码示例用 FormatField——自相矛盾。

**修正**（全部改为 `ConstructError::Integer`，与 error.rs:191-196 + select_exception_class 映射一致）：
- §1.4.1 VarInt build：负数分支 + 非 int 分支（v2 补全三段 extract：i64→u64→`is_instance_of::<PyLong>`），错误信息对齐 core.py:1634/1636
- §1.4.2 ZigZag build 非 int分支：信息 `value {obj} is not an integer or out of i64 range`
- §1.5.3 BytesInteger parse length==0 + slow-path `parse_bigint_from_bytes`
- §1.5.4 BytesInteger build length==0（v2 补全，parity 对齐 core.py:1262-1263）+ slow-path `build_bigint_to_bytes` 两处

**验证**：grep 确认 Primitives 文档无整数相关 `FormatField` 残留（Float 范围/类型错误保留 FormatField 正确，对齐 struct.error）。

### P2：Float build 加 int→float 兼容 + parity 校准（高严重度，已修正）

**问题**：v1 Float build 用 `extract::<f32>()`/`extract::<f64>()`，pyo3 对 Python int 返回 Err。Python `struct.pack('>f', 42)` 接受 int（实测 `== b'42280000'`），mashumaro `v: float` 注解运行时不强制，用户传 int 常见。

**修正**：
- 所有 6 个 Float build 分支加 `or_else` 链：`extract f32/f64`（Python float）→ `i64 as f32/f64` → `u64 as f32/f64`（Python int）
- **parity 校准（实证）**：基于 `construct.core.FormatField._build`（core.py:1166-1170）用 `try/except Exception` 把 `struct.pack` 所有异常转 `FormatFieldError`，实测确认：
  - BC-B8（`Float32b.build(1e40)`）：v1 正确（FormatFieldError，对齐 `struct.pack('>f',1e40)` OverflowError）
  - BC-B12（`Float16b.build(70000.0)`）：**v1 误判"返回 inf"已修正**为 FormatFieldError（实测 `struct.pack('>e',70000)` OverflowError；Rust 手动 `val.abs() > 65504.0` 检查）
  - BC-B14（`Float32b.build("not a float")`）：extract f32/i64/u64 全失败
  - BC-B15/16/17 新增（int→float 兼容）：`Float32b/16b/64b.build(42)` 实测字节序列

### P3：VarInt slow-path 缓存编译 + §9.3 删除技术错误预案（中严重度，已修正）

**问题**：v1 `build_varint_bigint` 每次调用 `run_bound` 重新编译 Python 代码；§9.3 预案"改 `call_method('to_bytes')`"有技术错误（VarInt LEB128 ≠ `int.to_bytes`）。

**修正**：
- §1.4.1 引入 `VARINT_ENCODER: GILOnceCell<Py<PyAny>>` + `get_varint_encoder`，首次调用编译 `_varint_encode` 并缓存，后续复用（与 error.rs `EXCEPTIONS` 同模式）
- §1.4.1 提取 `varint_encode_u64` fast-path helper（VarInt/ZigZag 共用，消除字节编解码重复）
- §9.3 预案改为"v2 已修正"，明确说明不能用 `to_bytes` 统一（LEB128 vs 直接转字节）

### P4：Node 变体数 24→20（低严重度，已修正）

**问题**：v1 §1.1 / §1.4.1 / §1.5 多处写"现有 24 个 Node 变体"，实际 mod.rs 20 个变体。

**修正**：grep + replaceAll 全部改为 20（line 16/480/838）。

### 附带发现（提示 PM，非本次范围）

- `docs/design/模块设计/模块设计-Adapter核心.md` 也有"24 个 Node 变体"（line 18/428/432）。Adapter 模块设计当时基于 Phase 6.1 前的 Node enum，若 Adapter 实施时 Node 已含 6.1 新增 3 变体，则应为 23。建议 PM 在 6.3 Adapter 设计阶段校正。
