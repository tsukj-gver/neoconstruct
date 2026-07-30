---
id: ANALYSIS-phase6-pre
status: active
phase: "6"
task: "Phase 6 启动前置三合一分析（依赖核查 + 构造器分析 + 测试框架设计）"
last_updated: 2026-07-29
depends_on: [AGENTS.md, ADR-001~ADR-020, MEMORY.md, harness/experiences.md, plans/phase6-primitives-strings-adapter/总纲.md, plans/phase7-conditional-streams/总纲.md, docs/constructors-inventory.csv]
---

# Phase 6 启动前置分析报告（三合一）

> **路径变更说明**：PM 任务书指定输出到 `plans/phase6-primitives-strings-adapter/分析报告-Phase6启动前置.md`，
> 但 `AGENTS.md §2` ARCH 权限表只允许写 `docs/`，不允许写 `plans/`。按 LTM 三层结构
> （`MEMORY.md` L3 工件表），分析报告归到 `docs/analysis/`。**PM 如需在 plans/ 下保留索引，
> 可在 `plans/phase6-primitives-strings-adapter/索引.md` 加 cross-ref**。
>
> 本报告由 ARCH 在 DESIGNING 阶段产出，覆盖三件并行分析任务：
>
> - **任务 A**：Phase 6 ↔ Phase 7 依赖核查（防止 Phase 7 返工）
> - **任务 B**：Phase 6 ~30 个构造器分析（含子任务拆分建议）
> - **任务 C**：测试框架设计（Phase 6.0 基础设施）
>
> **信息源**：Python 原版 `construct/construct/core.py`（6447 行）/ `construct-rs/src/`（24 个 Node + Descriptor + Compile 系统）/ `construct-rs/tests/`（13 test_*.py + 6 benchmark）/ `experiments/`（50+ 脚本）/ `testing/ci/run_l2_functional.ps1`。
>
> **§0 原则对照**贯穿全文。已对照 L-01（中间表示层）/ L-04（跨阶段模式）/ L-09（性能测量）/ L-12（PM 角色越界）。

## 摘要

| 任务 | 关键发现 | PM 决策点 |
|------|---------|----------|
| A 依赖核查 | **Strings 在 Python 用 macro 嵌套 Prefixed/FixedSized，但 Rust 重写可独立 Node 实现避免跨 Phase 依赖**；Phase 7 的 `If`/`Switch` 默认值依赖 `Pass`（Streams 类，未在 Phase 6/7 总纲） | 2 个（Strings 实现路线 / Pass 调入 Phase 6） |
| B 构造器分析 | 30 个构造器中**别名 4 个零成本**（FormatField 别名）/ Float 6 个扩展 FormatField format char / Adapter 需引入"用户面 Adapter"机制 | 1 个（Adapter 用户面机制方向） |
| C 测试框架 | parity 模式重复 ~300 行（test_phase4_parity 809 行 vs test_bitstream_parity 521 行）；benchmark 散乱 6 处；conftest 仅 26 行 | 2 个（目录结构 + benchmark 框架选型） |

---

## §A 任务 A：Phase 6 ↔ Phase 7 依赖核查

### A.1 核查方法

逐一阅读 Python 原版 `construct/construct/core.py` 中 Phase 7 全部 8 个构造器的实现，
提取三维度依赖：

1. **基类继承**：直接父类（`Construct` / `Subconstruct` / `Adapter`）
2. **构造器引用**：`__init__` 或 `parse/build` 中实例化的其他构造器
3. **表达式系统依赖**：`evaluate` / `ExprProgram` / context lambda

输出依赖矩阵后，与 Phase 6 范围（Primitives 17 + Strings 7 + Adapter 6）+ 已实现构造器
（Phase 1-5 共 40 个）求差集，识别"Phase 7 需要 但 Phase 6 未包含"的构造器。

### A.2 Phase 7 构造器依赖矩阵

| Phase 7 构造器 | 基类 | 引用的其他构造器 | 表达式依赖 | Phase 6 是否提供 |
|---------------|------|------------------|-----------|-----------------|
| **If**(cond, subcon) | （macro）`IfThenElse` | `Pass`（else 分支） | condfunc（i64） | ❌ **Pass 未在 Phase 6/7 总纲** |
| **IfThenElse**(cond, then, else) | `Construct` 直接子类 | then/else 由用户传入（任意） | condfunc | ✅ 无外部依赖 |
| **Switch**(key, cases, default=**Pass**) | `Construct` 直接子类 | **`Pass` 默认 default**；cases.values() 由用户传入 | keyfunc | ❌ **Pass 未在 Phase 6/7 总纲** |
| **Select**(*subcons) | `Construct` 直接子类 | subcons 由用户传入；内部用 stream_tell/seek | 无 | ✅ 无外部依赖 |
| **FocusedSeq**(parsebuildfrom, *subcons) | `Construct` 直接子类 | 内部类似 Sequence（Phase 5+ 未实现）；常配合 Rebuild/Const | parsebuildfrom 求值 | ⚠️ 设计上类似 Sequence（未在 Phase 6/7） |
| **Pointer**(offset, subcon) | `Subconstruct` 子类 | 无；用 stream_seek/tell | offset 可为 context lambda | ✅ Subconstruct 是 Phase 6 范围 |
| **Prefixed**(lengthfield, subcon) | `Subconstruct` 子类 | 用 `BytesIOWithOffsets`（Rust 已有 ParseStream 子流支持） | lengthfield 为任意 Construct | ✅ Subconstruct 是 Phase 6 范围 |
| **Seek**(at, whence) | `Construct` 直接子类 | 无；仅调 stream_seek | at/whence 可为 lambda | ✅ 无外部依赖 |

**Phase 7 全部依赖均在 Phase 6 完成后可满足**，**除一项例外**：

> **关键发现 1**：`Pass`（no-op 构造器，`construct-rs` inventory 中归类 Streams，标注 "Phase 5+"）
> 是 `If`/`Switch` 默认值的依赖。**Phase 6/7 总纲均未列入 Pass**，但 Phase 7 一启动就需要它。
>
> **建议**：把 `Pass` 调入 Phase 6.3（Adapter 核心一起做，因 Pass 实现极简——parse 返回 None、build no-op、sizeof=0，单文件 < 100 行 Rust）。

### A.3 横向发现：Phase 6 Strings 的跨 Phase 依赖

阅读 Python 原版 `core.py:1747-1858` 后发现 **Python 用 macro 嵌套实现 Strings**：

```python
def PaddedString(length, encoding):
    return StringEncoded(FixedSized(length, NullStripped(GreedyBytes, pad=...)), encoding)

def PascalString(lengthfield, encoding):
    return StringEncoded(Prefixed(lengthfield, GreedyBytes), encoding)

def CString(encoding):
    return StringEncoded(NullTerminated(GreedyBytes, term=...), encoding)

def GreedyString(encoding):
    return StringEncoded(GreedyBytes, encoding)
```

**依赖追踪**：

| Phase 6 Strings 构造器 | Python 嵌套依赖 | Phase 归属 | Rust 实现选项 |
|-----------------------|----------------|-----------|--------------|
| `GreedyString` | GreedyBytes + StringEncoded | GreedyBytes 已实现（Phase 1） | 独立 Node 或嵌套 |
| `CString` | NullTerminated + GreedyBytes + StringEncoded | NullTerminated 是 Phase 6 | 独立 Node 或嵌套 |
| `PascalString` | **Prefixed**（Phase 7） + GreedyBytes + StringEncoded | **跨 Phase 依赖** | 必须独立 Node |
| `PaddedString` | **FixedSized**（未在 Phase 6/7 总纲） + NullStripped + GreedyBytes + StringEncoded | **跨 Phase + 未规划** | 必须独立 Node |
| `NullTerminated` | Subconstruct + GreedyBytes | Subconstruct 是 Phase 6 | 独立 Node 或 Subconstruct |
| `NullStripped` | Subconstruct + GreedyBytes | Subconstruct 是 Phase 6 | 独立 Node 或 Subconstruct |
| `StringEncoded` | Adapter | Adapter 是 Phase 6 | Adapter 子类（用户面 Adapter） |

**关键发现 2**：Python macro 模式产生**两条跨 Phase 依赖**：
- `PascalString` → `Prefixed`（Phase 7 Streams）
- `PaddedString` → `FixedSized`（Streams 类，未在 Phase 6/7 总纲）

**§0 原则对照**：Python 用 macro 嵌套是为了代码复用，但**每次嵌套都增加运行时层级**（Adapter 包装 Subconstruct 包装 GreedyBytes），违反"无中间表示层"精神。
construct-rs 重写应**避免嵌套**，每个 String 构造器实现为**独立 Node**，直接处理 byte→str 编解码 + 长度规则。

### A.4 Phase 6 Strings 推荐实现路线（避免跨 Phase 依赖）

**方案 A（推荐）**：所有 Strings 实现为独立 Node，零跨 Phase 依赖。

| 构造器 | Node 变体 | 实现核心（独立 Node） |
|--------|----------|---------------------|
| CString | `CStringNode` | 1-pass 扫描到 term 字节，bytes.decode(encoding)；build 反向 |
| NullTerminated | `NullTerminatedNode` | 同 CString 但保留 subcon（默认 GreedyBytes） |
| NullStripped | `NullStrippedNode` | 读全部 + rstrip(pad) + bytes.decode |
| PaddedString | `PaddedStringNode` | 读 length 字节 + rstrip(pad) + decode；build encode + 右侧 pad 到 length |
| PascalString | `PascalStringNode` | lengthfield.parse → 读 N 字节 → decode；build 反向（不需 Prefixed） |
| GreedyString | `GreedyStringNode` | 读到 EOF + decode（直接基于 GreedyBytes） |
| StringEncoded | （不实现为 Node） | 仅作为 Python 侧 macro 工具，由各 String Node 直接处理 encode/decode |

**优点**：
- 零跨 Phase 依赖（Phase 6 自洽）
- 性能最优（每个 Node 直接做 byte↔str，无中间 Adapter 层）
- §0 合规（无中间表示层）

**缺点**：
- 7 个 Node 共享 encode/decode 逻辑，需提取公共 helper（如 `apply_encoding(bytes, encoding) -> PyString`）
- Rust 侧需处理多编码（utf8/utf16/utf32/ascii），每编码独立路径

**方案 B（不推荐）**：照搬 Python macro 嵌套。
- 需要把 `Prefixed`（Phase 7）+ `FixedSized`（Streams）调入 Phase 6
- 性能损失：Adapter + Subconstruct 双层间接
- 违反 §0 #2（中间表示层）精神

### A.5 Phase 7 推荐依赖前置（最小集）

**仅需把 `Pass` 调入 Phase 6**（作为 Phase 6.3 Adapter 一部分）。

- `Pass` 是 Phase 7 启动的最小依赖（If/Switch 默认值）
- 实现成本极低（< 100 行 Rust，parse/build/sizeof 全 trivial）
- 归类：Streams 类（Python 原版），但实际是控制流基础设施

**其他 Phase 7 构造器（IfThenElse/Switch/Select/FocusedSeq/Pointer/Prefixed/Seek）**无需前置，
Phase 6 完成后可立即启动 Phase 7。

### A.6 §A 段 PM 决策点

**决策 A-1：Strings 实现路线**（方案 A 独立 Node vs 方案 B 嵌套 macro）
- **ARCH 推荐**：方案 A（独立 Node），原因：§0 合规 + 性能最优 + Phase 自洽
- **影响**：若选 A，Phase 6 Strings 子任务可在 Phase 6 自洽完成；若选 B，需扩 Phase 6 范围
  调入 Prefixed + FixedSized

**决策 A-2：Pass 调入 Phase 6**
- **ARCH 推荐**：是（作为 Phase 6.3 Adapter 的一部分，< 100 行）
- **影响**：Phase 7 启动时不再有 "If 默认值无 Pass 可用" 阻塞

---

## §B 任务 B：Phase 6 构造器分析

### B.1 分析维度（每构造器）

按 PM 任务书要求，每构造器分析 6 个维度：
1. Python 原版实现摘要（基类 + parse/build 核心）
2. 现有架构适配方案（新增 Node / 复用 / 编译期翻译）
3. §0 合规性检查
4. 依赖关系（前置 + 被依赖）
5. 实施优先级（被依赖数 + 用户面频率）
6. 子任务归属建议

### B.2 Primitives 17 个分析

#### B.2.1 整数别名（4 个，零成本）

| 构造器 | Python 实现 | 适配方案 | 工作量 |
|--------|------------|---------|--------|
| `Byte` | `Byte = Int8ub`（core.py:1524） | **Python 层别名**，零 Rust 改动 | < 5 行 Python |
| `Short` | `Short = Int16ub`（core.py:1525） | 同上 | < 5 行 |
| `Int` | `Int = Int32ub`（core.py:1526） | 同上 | < 5 行 |
| `Long` | `Long = Int64ub`（core.py:1527） | 同上 | < 5 行 |

**§0 合规**：✅ 直接复用已实现的 FormatFieldNode。
**依赖**：无前置；被依赖：用户协议常用别名。
**优先级**：低（功能等价已实现，仅 alias 补全）。
**建议**：与 Float 一起放在 6.1 子任务（一次 Python 侧 commit 完成）。

#### B.2.2 Float 系列（6 个，扩展 FormatField）

| 构造器 | Python 实现 | 适配方案 |
|--------|------------|---------|
| `Float16b/l` | `FormatField(">", "e")` / `FormatField("<", "e")`（core.py:1530-1536） | **扩展 PythonFormat enum** +6 变体（Float16/32/64 × Big/Little） |
| `Float32b/l` | `FormatField(">", "f")` / `FormatField("<", "f")`（core.py:1543-1549） | 同上 |
| `Float64b/l` | `FormatField(">", "d")` / `FormatField("<", "d")`（core.py:1556-1562） | 同上 |

**适配方案**：
- 在 `format_field.rs` 的 `PythonFormat` enum 增加 6 个 Float 变体
- 扩展 `from_chars` 支持 `'e'/'f'/'d'` 字符
- parse 路径：`f32::from_bytes` / `f64::from_bytes`（标准库 `from_le_bytes`/`from_be_bytes`）
- build 路径：`f32::to_le_bytes` / `to_be_bytes`
- Float16（`e` 格式）：用 `half` crate 或手写 half-precision 编解码（Rust 标准库无 f16）
- pyo3 转换：`f32/f64 → PyFloat`（已有支持）

**§0 合规**：✅ 复用 FormatFieldNode 数据流，仅扩展 format 处理。
**依赖**：无前置；被依赖：协议浮点字段。
**优先级**：高（用户面常用，且单点改造 FormatField 影响面小）。
**Float16 风险**：Rust 无 f16 原生类型，需引入 `half` crate 或自实现 bit 拼接。建议作为 6.1 的子点单独验证。

#### B.2.3 Int24 系列（4 个，依赖 BytesInteger）

| 构造器 | Python 实现 | 适配方案 |
|--------|------------|---------|
| `Int24ub/ul/sb/sl` | `BytesInteger(3, signed=..., swapped=...)`（core.py:1575-1593） | **先实现 BytesInteger，再 Python 层 alias** |

**关键**：Python Int24 是 `BytesInteger(3)` 的别名，必须先有 BytesInteger Node。
**优先级**：与 BytesInteger 绑定实现。

#### B.2.4 变长整数（2 个，独立 Node）

| 构造器 | Python 实现 | 适配方案 |
|--------|------------|---------|
| `VarInt` | `Construct` 直接子类（core.py:1601-1647），LEB128 编码 | **新增 VarIntNode** |
| `ZigZag` | `Construct` 直接子类（core.py:1651-1686），调 VarInt 内部 | **新增 ZigZagNode**（包装 VarInt 逻辑） |

**VarInt 实现核心**：
- parse：循环 `read 1 byte`，低 7 位累加，高位为 1 继续
- build：循环 `& 0x7F | 0x80` 写入，直到剩余 < 7 位
- sizeof：返回 Err（变长）

**ZigZag 实现核心**：
- parse：VarInt 解码后 `(n >> 1) ^ -(n & 1)`
- build：`(n << 1) ^ (n >> 63)` 后 VarInt 编码
- 可复用 VarIntNode 的字节读写逻辑

**§0 合规**：✅ 全 Rust 内部计算，零中间表示。
**性能预测**（L-02 对策：标注为量级估算，需实测）：
- VarInt 平均 1.5 字节（小整数），性能应与 FormatField 同量级（~10x）
- ZigZag 多一次 XOR + shift，应仍 ≥10x
**依赖**：无前置；被依赖：PascalString（推荐 lengthfield=VarInt）、protobuf-like 协议。
**优先级**：高（Phase 6 Strings 的 PascalString 推荐用 VarInt 作 lengthfield）。

#### B.2.5 大整数（1 个，独立 Node）

| 构造器 | Python 实现 | 适配方案 |
|--------|------------|---------|
| `BytesInteger` | `Construct` 直接子类（core.py:1203-1292），任意字节长度整数 | **新增 BytesIntegerNode** |

**实现核心**：
- parse：`read N bytes` → 按 signed/unsigned + endian 拼成 PyLong
- build：PyLong → 拆 N 字节 + endian
- 依赖 `bytes2integer` / `integer2bytes` 工具函数（Python 在 `lib/binary.py`）

**Rust 实现关键点**：
- 大整数（>8 字节）需用 `num-bigint` crate 或 pyo3 直接构造 PyLong（推荐后者，避免中间类型）
- pyo3 `PyLong::new(py, &BigInt)` 可直接从字节构造大整数（需研究 API）
- ≤8 字节走 fast-path（直接用 i64/u64）

**§0 合规**：⚠️ 需注意 —— `num-bigint` 作为中间类型可能违反"无中间表示"。建议：
- parse：bytes → 直接调 CPython `PyLong_FromByteArray`（unsafe raw FFI，参考 ADR-019 错误路径 fast-path）
- build：CPython `PyLong_AsByteArray`（同上）
- **决策点**：是否接受首次热路径 unsafe raw FFI（Phase 4.x 错误路径已有先例）

**性能预测**（量级估算）：
- 4 字节以下走 fast-path，~10x
- 16+ 字节走 raw FFI，应仍 ≥5x（CPython C API 直接构造）

**依赖**：无前置；被依赖：Int24 系列、大整数协议（crypto / hash）。
**优先级**：高（Int24 别名的前置）。

### B.3 Strings 7 个分析

按 §A.4 推荐方案 A（独立 Node），7 个 String 构造器均实现为独立 Node。

| 构造器 | Python 实现 | Rust Node | 编码依赖 |
|--------|------------|-----------|---------|
| `CString(enc)` | `StringEncoded(NullTerminated(GreedyBytes, term=...), enc)` | `CStringNode { encoding, term }` | utf8/16/32/ascii |
| `NullTerminated(subcon, term, include, consume, require)` | `Subconstruct` 子类（core.py:5050） | `NullTerminatedNode { inner: Box<Node>, term, ... }` | 无 |
| `NullStripped(subcon, pad)` | `Subconstruct` 子类（core.py:5123） | `NullStrippedNode { inner: Box<Node>, pad }` | 无 |
| `PaddedString(length, enc)` | `StringEncoded(FixedSized(L, NullStripped(GreedyBytes, pad=...)), enc)` | `PaddedStringNode { length, encoding, pad }` | utf8/16/32/ascii |
| `PascalString(lengthfield, enc)` | `StringEncoded(Prefixed(lengthfield, GreedyBytes), enc)` | `PascalStringNode { lengthfield: Box<Node>, encoding }` | utf8/16/32/ascii |
| `GreedyString(enc)` | `StringEncoded(GreedyBytes, enc)` | `GreedyStringNode { encoding }` | utf8/16/32/ascii |
| `StringEncoded(subcon, enc)` | `Adapter` 子类（core.py:1711） | （不实现为 Node） | 仅 Python 侧工具 |

**公共组件建议**：
- 新增 `nodes/strings/` 子目录（与 `nodes/` 平级）
- 提取 `apply_encoding(bytes, encoding, py) -> PyString` 公共 helper
- 提取 `parse_encoding(obj: &PyAny, encoding) -> bytes` 公共 helper
- Rust 编码处理：utf8（标准库）/ utf16（标准库）/ utf32（手写或 crate）/ ascii（标准库）

**§0 合规**：✅ 每构造器独立 Node，直接 bytes↔str，无中间 Adapter 层。
**依赖**：仅 GreedyBytes（已实现 Phase 1）+ FormatField（lengthfield 用）。
**性能预测**（量级估算）：
- CString/GreedyString：单次扫描 + decode，应 ≥10x
- PaddedString/PascalString：多一次 length 处理，应 ≥8x（FFI 稀释）
- 编码开销：utf8 最快（Rust std 优化），utf16/32 略慢但仍应 ≥5x

### B.4 Adapter 核心 6 个分析

**这是 Phase 6 最大的设计难点**，因 construct-rs 目前无 Adapter/PyCallable 机制（仅 RepeatUntil v5 删除 PyCallable 后的 ExprProgram 路径）。

| 构造器 | Python 实现 | 用户面机制 | Rust 适配方案 |
|--------|------------|----------|--------------|
| `Subconstruct(subcon)` | 抽象基类（core.py:787），parse/build/sizeof 全转发 | 无用户逻辑 | **新增 SubconstructNode**（纯转发） |
| `Adapter(subcon)` | Subconstruct 子类（core.py:813），子类实现 `_decode`/`_encode` | **Python `_decode`/`_encode` 方法** | **需引入"用户面 Adapter"机制**（见下） |
| `SymmetricAdapter(subcon)` | Adapter 子类（core.py:837），`_encode = _decode` | 同 Adapter | 复用 Adapter 机制 |
| `Rebuild(subcon, func)` | Subconstruct 子类（core.py:2975），build 时用 func 求值 | func 可为 ExprProgram 或 PyCallable | **新增 RebuildNode**（func 走 ExprProgram 路径） |
| `RawCopy(subcon)` | Subconstruct 子类（core.py:4761），捕获 offset1/offset2/data/value | 无用户逻辑 | **新增 RawCopyNode**（返回 Container） |
| `Peek(subcon)` | Subconstruct 子类（core.py:4486），预读不消费 | 无用户逻辑 | **新增 PeekNode**（fallback + seek） |

**用户面 Adapter 机制设计难点**（PM 决策点）：

Python `Adapter` 允许用户继承并实现 `_decode(self, obj, context, path) -> Any` 与
`_encode(self, obj, context, path) -> Any`。Rust 适配有三条路线：

**路线 1（PyCallable，每次 FFI）**：
- 用户 _decode/_encode 作为 Python callable，Rust 端持 `Py<PyAny>` 引用
- 每次 parse/build 调用：`adapter.decode.call(py, (obj, ctx,))` 跨 FFI
- **违反 §0 #1（一次 FFI）精神**：parse 路径变成"read subcon → call _decode → return"两次 FFI
- **性能影响**：subcon parse 已 ~200ns，多一次 FFI ~50-100ns，加速比可能降到 5-8x
- **优点**：用户面完全兼容 Python 原版

**路线 2（编译期翻译，零 FFI）**：
- 用户用受限 DSL 写 _decode/_encode（如 `decode_expr(this_ * 2)` / `encode_expr(this_ >> 1)`）
- 编译期翻译为 ExprProgram，运行时零 FFI
- **优点**：性能最优，符合 §0
- **缺点**：用户面受限（仅表达式逻辑，不能任意 Python 代码）；需设计新 DSL

**路线 3（混合：内置 Adapter + 用户 PyCallable）**：
- 常用 Adapter（Hex/HexDump/Enum/Validator/Mapping/FlagsEnum）作为**内置 Node**（零 FFI）
- 用户自定义 Adapter 走 PyCallable（路线 1，慢路径）
- 用户面文档明确："内置 Adapter 性能最优；自定义 Adapter 走慢路径"
- **优点**：常用场景快 + 用户面兼容
- **缺点**：用户面有"性能双轨制"，需文档明确

**ARCH 推荐：路线 3（混合）**。理由：
- Phase 4.5 v5 已经证明"PyCallable 路径性能不达标"（RepeatUntil v4 用 PyCallable 时 6-9x，v5 改 ExprProgram 后 10-20x）
- 但完全禁止 PyCallable 会破坏用户面兼容（Python construct 的 Adapter 是核心 API）
- 路线 3 既保性能（内置 Adapter）又保兼容（用户 Adapter 可用）

**§0 合规性检查（Adapter 路线 3）**：
- §0 #1（一次 FFI）：内置 Adapter 满足；用户 Adapter 违反但用户主动选择慢路径（Phase 5 总纲原则）
- §0 #2（无中间表示层）：内置 Adapter 直接转换；用户 Adapter 有 Python 对象中间态但属用户域
- §0 #3（无 I/O trait 抽象层）：Subconstruct 仅持有 `Box<Node>`，不引入新 trait

**依赖关系**：
- 前置：无（Subconstruct 是抽象基类）
- 被依赖：Phase 7 Pointer/Prefixed/Peek（Subconstruct 子类）；Enum/Validator/Hex 等（Phase 6+ Adapter 子类）

**实施优先级**：
1. **Subconstruct + RawCopy + Peek**（无用户逻辑，独立 Node，最低风险）→ 6.3a
2. **Rebuild**（ExprProgram 路径，复用 Phase 2 表达式系统）→ 6.3b
3. **Adapter + SymmetricAdapter + Pass**（需用户面机制决策）→ 6.3c（最复杂）

### B.5 实施优先级矩阵（被依赖数 + 用户面频率）

| 构造器 | 被依赖数（Phase 6/7+） | 用户面频率 | 综合优先级 |
|--------|---------------------|-----------|----------|
| BytesInteger | 高（Int24×4 依赖） | 中 | **P0** |
| VarInt | 高（PascalString 推荐 + protobuf） | 高 | **P0** |
| Float32/64 | 低 | 高（IEEE754 协议） | **P0** |
| Subconstruct | 高（Phase 6 Adapter + Phase 7 Streams 全依赖） | 低（抽象基类） | **P0** |
| CString / NullTerminated | 中（CString 用 NullTerminated） | 高 | **P1** |
| PascalString | 低 | 中 | **P1** |
| Adapter | 高（Enum/Validator 等 Phase 6+ 依赖） | 中 | **P1**（机制决策前置） |
| ZigZag | 低（依赖 VarInt） | 低 | **P2** |
| Float16 | 低 | 低 | **P2**（f16 风险） |
| Int24 ×4 | 低（依赖 BytesInteger） | 低 | **P2**（别名） |
| Byte/Short/Int/Long | 低 | 中 | **P3**（零成本别名） |
| GreedyString / NullStripped / PaddedString | 低 | 中 | **P2** |
| Rebuild / RawCopy / Peek | 低 | 低 | **P2** |
| SymmetricAdapter | 低（依赖 Adapter） | 低 | **P3** |

### B.6 子任务拆分建议

**ARCH 推荐 4 个子任务**（不是 PM 任务书提到的 3 个）：

| 子任务 | 范围 | 估计工作量 | 风险 |
|-------|------|----------|------|
| **6.0 测试框架基础设施** | §C 段设计：conftest 扩展 + parity helper + 目录结构 | 1-2 天 | 低 |
| **6.1 Primitives 收尾**（17 个） | Byte/Short/Int/Long 别名 + Float 系列 + BytesInteger + VarInt + ZigZag + Int24 别名 | 3-5 天 | 中（Float16 f16 + BytesInteger raw FFI） |
| **6.2 Strings**（7 个） | CString/NullTerminated/NullStripped/GreedyString/PaddedString/PascalString + StringEncoded Python 工具 | 3-4 天 | 中（多编码处理） |
| **6.3 Adapter 核心**（6 个 + Pass） | Subconstruct + RawCopy + Peek + Rebuild + Adapter + SymmetricAdapter + Pass | 4-6 天 | **高**（Adapter 用户面机制决策） |

**为何拆 4 个而非 3 个**：
- 6.0 测试基础设施必须先行（否则 6.1-6.3 的 parity 测试无处安放）
- Adapter 因用户面机制决策（路线 1/2/3）需要 PM 拍板，独立子任务便于聚焦讨论
- Strings 与 Primitives 实现模式差异大（Primitives 是 Node 扩展，Strings 是新 Node + 编码 helper）

**子任务依赖图**：
```
6.0 测试框架
   ↓
6.1 Primitives ─────┐
   ↓                │
6.2 Strings         │
                    │
6.3 Adapter (含 Pass) ←─ 依赖决策（路线 1/2/3）
```

6.1 / 6.2 / 6.3 在 6.0 完成后可并行（Strings 不依赖 Primitives，Adapter 不依赖 Strings）。

### B.7 §B 段 PM 决策点

**决策 B-1：Adapter 用户面机制方向**
- **ARCH 推荐**：路线 3（混合：内置 Adapter 零 FFI + 用户 Adapter PyCallable 慢路径）
- **影响**：决定 6.3 子任务的具体实现路径与用户面 API 设计
- **替代选项**：路线 1（全 PyCallable，性能损失）/ 路线 2（全 DSL，用户面受限）

---

## §C 任务 C：测试框架设计（Phase 6.0 基础设施）

### C.1 现状盘点

**测试代码散落三处**：

| 位置 | 文件数 | 总行数 | 用途 |
|------|-------|--------|------|
| `construct-rs/tests/` | 18 个 .py + 1 个 .rs | ~5800 行 | pytest 测试 + benchmark 脚本 |
| `experiments/` | 50+ 个 .py + .json + .md + .rs | ~10000+ 行 | vet 实验 / phase smoke / bench 历史 |
| `testing/ci/` | 13 个 .ps1 + .py + .json | ~2700 行 | L1-L4 门禁 runner |

**问题清单**（PM 调研补充验证）：

1. **parity 模式重复**：
   - `test_phase4_parity.py`（712 行）与 `test_bitstream_parity.py`（461 行）共享 ~300 行相同模板（子进程隔离脚本、`_normalize` 函数、`_run_parity_case` 函数、JSON 输出协议）
   - 每加一个 phase 的 parity 测试都要重写一遍模板

2. **L2 脚本清单硬编码**：
   - `run_l2_functional.ps1` `$MATRIX` 数组手动列 12 个脚本路径
   - 每加一个 phase 的 smoke 脚本要手工追加（OBS-1 已经手工补入过 phase3.3 / phase4_repeat_until_examples）

3. **benchmark 散乱**：
   - tests/ 内混放 5 个 bench/benchmark 脚本（425 / 770 / 358 / 310 / 384 行）
   - experiments/ 内有更多 phase4_bench_* 系列历史脚本（v2/v3/v4 多版本）
   - 无统一 benchmark 入口或报告格式

4. **conftest.py 仅 26 行**：只做 sys.path 修复（避免 site-packages construct 命中），无共享 fixtures / helpers

5. **测试分类模糊**：
   - `test_field_types.py`（621 行）混合了 unit + integration
   - `test_integration.py`（256 行）+ `test_roundtrip.py`（217 行）+ `test_edge_cases.py`（239 行）三者在功能上有重叠
   - `test_compile_errors.py` / `test_errors.py` 区分依据不强

6. **无统一错误路径 helper**：每个 test 文件自行构造错误场景

### C.2 测试分类原则

| 类别 | 范围 | 工具 | 目录 | 频率 |
|------|------|------|------|------|
| **Unit** | Rust 内部模块（format_field/bytes/expr 等）单一职责 | `cargo test` + 简单 pytest | `construct-rs/tests/unit/` | 每次提交 |
| **Parity** | 构造器行为与 Python construct 2.10.70 一致 | pytest + 子进程隔离 | `construct-rs/tests/parity/` | 每次 phase 子任务 |
| **Integration** | 多构造器组合（Struct+Array+Bitwise 嵌套等） | pytest | `construct-rs/tests/integration/` | 每 phase |
| **Errors** | 错误路径（StreamError/FieldError/StopField） | pytest | `construct-rs/tests/errors/` | 每 phase |
| **Benchmark** | 性能基线（与 Python 对比加速比） | 独立 Python 脚本 + Controlled A/B Test | `construct-rs/bench/` | 每 phase ACCEPT 前 |

**分类判定规则**：
- 测试单个构造器/节点的单一方法 → Unit
- 测试 `construct-rs.parse(x) == python_construct.parse(x)` → Parity
- 测试 2+ 构造器组合的端到端场景 → Integration
- 测试错误条件触发与错误信息 → Errors
- 测试 ns/µs 级耗时与加速比 → Benchmark

### C.3 目录结构设计

```
construct-rs/
├── tests/
│   ├── conftest.py                    # 扩展：共享 fixtures + sys.path
│   ├── _helpers/                      # 公共组件（私有命名 _ 前缀）
│   │   ├── __init__.py
│   │   ├── parity.py                  # 子进程隔离 + JSON 通信 + run_parity_case
│   │   ├── normalize.py               # _normalize（Container/bytes 递归规范化）
│   │   ├── fixtures.py                # 共享 fixtures（rs_venv / py_venv / case_template）
│   │   └── encoding_parity.py         # 多编码字符串对比工具
│   ├── unit/                          # 单一职责 unit test
│   │   ├── test_compile_errors.py     # （迁移自 tests/）
│   │   ├── test_field_types.py        # （拆分：unit 部分迁入）
│   │   ├── test_expr_compile.py
│   │   ├── test_bytes_expr.py
│   │   └── test_repeat_until_descriptor.py
│   ├── parity/                        # Python 行为一致性
│   │   ├── test_phase3_bitstream_parity.py    # 改名自 test_bitstream_parity.py
│   │   ├── test_phase4_array_parity.py        # 改名自 test_phase4_parity.py
│   │   ├── test_phase6_primitives_parity.py   # 新增（Phase 6.1）
│   │   ├── test_phase6_strings_parity.py      # 新增（Phase 6.2）
│   │   └── test_phase6_adapter_parity.py      # 新增（Phase 6.3）
│   ├── integration/                   # 多构造器组合
│   │   ├── test_roundtrip.py          # （迁移自 tests/）
│   │   ├── test_integration.py        # （迁移自 tests/）
│   │   └── test_edge_cases.py         # （迁移自 tests/）
│   ├── errors/                        # 错误路径
│   │   ├── test_errors.py             # （迁移自 tests/）
│   │   └── test_compile_errors.py     # 或合并到 unit/
│   └── rust/                          # Rust 内部测试（bench_context_ops.rs 等）
│       └── bench_context_ops.rs
├── bench/                             # benchmark 独立目录（与 tests/ 平级）
│   ├── README.md                      # 使用说明 + 运行方式
│   ├── _helpers/
│   │   ├── __init__.py
│   │   ├── runner.py                  # 统一 runner（多次采样 + stddev）
│   │   ├── stats.py                   # welch_t / 加速比计算（参考 E01_O1_ab_stats.py）
│   │   └── report.py                  # 报告生成（txt + json）
│   ├── bench_struct.py                # 改名自 tests/benchmark.py（Phase 1 B1-B4）
│   ├── bench_bitstream.py             # 改名自 tests/benchmark_bitstream.py（Phase 3）
│   ├── bench_expr.py                  # 改名自 tests/benchmark_expr.py（Phase 2）
│   ├── bench_stages.py                # 改名自 tests/bench_stages.py
│   └── bench_breakdown.py             # 改名自 tests/bench_breakdown.py
└── ...
```

**设计要点**：
- `_helpers/` 用下划线前缀，pytest 不收集为 test 模块
- parity/integration/errors 文件名前缀 `test_phase{N}_*.py`，方便 L2 自动发现按 phase 过滤
- bench/ 独立目录与 tests/ 平级，明确"benchmark 不是测试"（performance-gate SKILL §Checkpoint 2）

### C.4 公共组件设计

#### C.4.1 conftest.py 扩展（~80-100 行）

现有 26 行保留（sys.path 操纵），新增 fixtures：

```python
@pytest.fixture(scope="session")
def rs_python():
    """CRS venv python.exe（含 construct-rs wheel）"""

@pytest.fixture(scope="session")
def py_python():
    """PC venv python.exe（含 Python construct 2.10.70）"""

@pytest.fixture(scope="session")
def crs_python_dir():
    """construct-rs python/ 目录（sys.path 注入用）"""

@pytest.fixture
def roundtrip_template():
    """通用 round-trip 测试模板：parse → build → parse 比对"""

@pytest.fixture
def smoke_case_template():
    """smoke 用例模板：定义 case_id + setup + expected"""
```

#### C.4.2 `_helpers/parity.py`（~150 行）

提取自 test_phase4_parity.py / test_bitstream_parity.py 共同部分：

```python
def run_parity_case(impl: str, case_id: str, case_script: str,
                    rs_python: str, py_python: str,
                    crs_python_dir: str) -> dict:
    """子进程隔离运行单个 parity case。
    
    参数：
        impl: "rs" 或 "py"
        case_id: 用例标识（如 "A1"）
        case_script: 子进程内执行的 Python 代码（含 case 定义 + _normalize）
        rs_python / py_python: 两 venv 的 python.exe
        crs_python_dir: construct-rs python/ 目录
    
    返回：{"parsed": ..., "built": ..., "roundtrip_parsed": ...}
    """

def assert_parity(results: dict, case_id: str, desc: str = ""):
    """断言 rs 与 py 结果一致（parse / build / roundtrip 三维度）"""

def assert_fidelity(results: dict, case_id: str, impl: str):
    """断言单 impl 的 parse → build → parse 保真"""
```

#### C.4.3 `_helpers/normalize.py`（~50 行）

提取自现有 parity 测试中的 `_normalize` 函数：

```python
def normalize(value):
    """规范化输出值，便于跨实现比较。
    - dict/Container → 排序 (key, value) 列表（递归），过滤 _io 等内部键
    - list/ListContainer → 列表（递归）
    - bytes → {'__bytes__': hex} 包装
    - 其他原样返回
    """
```

#### C.4.4 `_helpers/fixtures.py`（~100 行）

共享 pytest fixtures 与 case 注册机制：

```python
class CaseRegistry:
    """phase × case 注册表，供 parametrize 自动展开"""
    def register(self, phase: str, case_id: str, desc: str, builder): ...
    def all_cases(self, phase: str = None) -> list: ...
```

### C.5 L2 门禁改造方案

**现状**（`run_l2_functional.ps1`）：
- `$MATRIX` 数组硬编码 12 个脚本路径（line 81-115）
- 每加一个 phase smoke 脚本要手工追加，OBS-1 已补入 2 个

**改造方案**：从硬编码脚本清单 → **pytest 自动发现 + 双轨制**

**双轨制设计**：

1. **新轨（推荐）**：pytest 自动发现 `tests/parity/` + `tests/integration/` 下所有 test_*.py
   ```powershell
   # run_l2_functional.ps1 新增逻辑
   $pytestArgs = @("tests/parity/", "tests/integration/", "-v", "--json-report")
   & $CrsPython -m pytest $pytestArgs
   ```
   - 优点：自动发现，新测试零配置接入
   - 缺点：依赖 pytest 已安装（CRS venv 已有）

2. **旧轨（保留）**：`experiments/phase*_smoke.py` 系列 smoke 脚本继续作为补充
   - 保留原因：smoke 脚本是"端到端用户视角"，pytest 是"测试视角"，两者互补
   - 改造：`$MATRIX` 改为 glob 自动展开（`Get-ChildItem experiments/phase*_smoke*.py`），不再手工列路径

**迁移路径**：
- Phase 6.0：保留双轨制，新轨（pytest）作为主路径，旧轨（smoke）作为补充
- Phase 6+：每个 phase 的 parity 测试只接入新轨（pytest）
- Phase 7+：评估是否完全废弃旧轨（smoke 脚本迁入 pytest 或删除）

### C.6 Benchmark 框架选型

**ARCH 不推荐 pytest-benchmark**，理由：
- pytest-benchmark 适合纯 Python 函数级 benchmark（同一进程内重复调用）
- 本项目 benchmark 是**跨进程**（Rust vs Python construct），需子进程隔离 + Controlled A/B Test（L-09 对策）
- pytest-benchmark 无法表达"加速比"概念，需自定义统计逻辑

**ARCH 推荐：自建轻量 benchmark 框架**（基于现有 `E01_O1_ab_stats.py` 经验）：

```python
# bench/_helpers/runner.py
class BenchRunner:
    def __init__(self, rs_python, py_python, iterations=20, warmup=5): ...
    def run(self, scenario: Callable, payload) -> BenchResult: ...
    def controlled_ab_test(self, scenario_a, scenario_b, payload) -> ABResult: ...

# bench/_helpers/stats.py  
def welch_t(a_samples, b_samples) -> float: ...
def speedup_ratio(rs_ns, py_ns) -> float: ...
def format_report(results: list[BenchResult]) -> str: ...
```

**优点**：
- 完全控制子进程隔离 / 多次采样 / stddev 计算（L-09 对策）
- 复用 E01_O1_ab_stats.py 已验证的统计逻辑
- 报告格式与 `docs/perf-scenarios.csv` 兼容（PM 主维护）

**目录**：`construct-rs/bench/`（与 tests/ 平级）

### C.7 迁移计划（渐进式，6 步）

**原则**：
- 不一次性迁移所有现有测试（高风险）
- 每步可独立验证（每步后 `cargo test` + pytest 全 PASS）
- 新代码用新框架，旧代码渐进迁入

| 步骤 | Phase | 工作 | 验证 |
|------|-------|------|------|
| 1 | 6.0a | 建立 `tests/_helpers/` + `tests/conftest.py` 扩展 + `tests/{unit,parity,integration,errors}/` 目录 | 现有 12 个 test_*.py 仍可运行（位置不变） |
| 2 | 6.0b | 迁移 `test_phase4_parity.py` → `tests/parity/test_phase4_array_parity.py`（用新 helper） | pytest 通过 + L2 通过 |
| 3 | 6.0c | 迁移 `test_bitstream_parity.py` → `tests/parity/test_phase3_bitstream_parity.py` | 同上 |
| 4 | 6.0d | 迁移 5 个 bench/benchmark 脚本 → `bench/`（仅改名 + 提取公共组件） | bench 输出与原一致 |
| 5 | 6.0e | L2 改造：`run_l2_functional.ps1` 新增 pytest 自动发现轨 | L2 报告含新旧两轨结果 |
| 6 | 6.0f | 迁移剩余 test_*.py（test_errors / test_roundtrip / test_integration 等）按分类入目录 | 全部测试 PASS |

**Phase 6.1-6.3 期间**：
- 新增 parity 测试直接写到 `tests/parity/test_phase6_*.py`，用新 helper
- 不强制立即迁移剩余旧测试（按步骤 6 在 Phase 6 末期完成）

### C.8 §C 段 PM 决策点

**决策 C-1：目录结构方案**
- **ARCH 推荐**：`tests/{unit,parity,integration,errors}/` + `bench/` 平级（详见 §C.3）
- **影响**：Phase 6.0 实施的基础设施；6.1-6.3 测试归属
- **替代选项**：按 phase 分（`tests/phase3/` / `tests/phase4/` 等）—— 不推荐，因 phase 是历史维度而非测试类型维度

**决策 C-2：Benchmark 框架**
- **ARCH 推荐**：自建轻量框架（基于 E01_O1_ab_stats.py 经验），放 `bench/` 目录
- **影响**：Phase 6+ 性能验证流程；perf-scenarios.csv 数据采集方式
- **替代选项**：引入 pytest-benchmark —— 不推荐（不匹配跨进程隔离需求）

---

## §D PM 需做的决策汇总（≤ 5 个）

| # | 决策 | ARCH 推荐 | 影响范围 | 优先级 |
|---|------|----------|---------|--------|
| **1** | **Strings 实现路线**：方案 A 独立 Node（避免跨 Phase 依赖）vs 方案 B 嵌套 macro（需调入 Prefixed/FixedSized） | 方案 A | Phase 6.2 范围 + 是否扩 Phase 6 调入 Prefixed/FixedSized | **P0**（影响 6.2 启动） |
| **2** | **Adapter 用户面机制方向**：路线 1 全 PyCallable / 路线 2 全 DSL / 路线 3 混合（内置 + PyCallable 慢路径） | 路线 3 | Phase 6.3 用户面 API + 性能 | **P0**（影响 6.3 启动） |
| **3** | **Pass 调入 Phase 6**：作为 Phase 6.3 一部分（< 100 行 Rust）vs 推迟到 Phase 7 | 调入 Phase 6.3 | Phase 7 启动是否被 Pass 阻塞 | **P1** |
| **4** | **测试目录结构**：`tests/{unit,parity,integration,errors}/` + `bench/` 平级 vs 其他方案 | 推荐 | Phase 6.0 基础设施 | **P0**（6.0 启动前） |
| **5** | **Benchmark 框架**：自建轻量（基于 E01_O1 经验）vs pytest-benchmark | 自建 | Phase 6+ 性能验证流程 | **P1** |

> **额外提示**（不需 PM 决策但需知会）：
>
> - **BytesInteger raw FFI 风险**：≤8 字节走 fast-path；>8 字节需 CPython `PyLong_FromByteArray` raw FFI（首次热路径 unsafe raw FFI，参考 ADR-019 错误路径先例）。ARCH 将在 6.1 详细设计文档中标注，由 REV 审查 unsafe SAFETY 注释。
> - **Float16 f16 风险**：Rust 标准库无 f16 原生类型，需引入 `half` crate 或手写 bit 拼接。ARCH 将在 6.1 详细设计中评估两方案。
> - **路径变更**：本报告写于 `docs/analysis/`（ARCH 权限范围），PM 如需 `plans/phase6-primitives-strings-adapter/` 下索引，可在该目录 `索引.md` 加 cross-ref。

---

## 附录 A：Python 原版源码定位（参考实现速查）

| Phase 6 构造器 | core.py 行号 | 备注 |
|---------------|-------------|------|
| Byte/Short/Int/Long 别名 | 1524-1527 | FormatField 别名 |
| Float16/32/64 b/l | 1530-1566 | FormatField 'e'/'f'/'d' |
| Int24 系列 | 1575-1597 | BytesInteger(3) 别名 |
| VarInt | 1601-1647 | Construct 直接子类 |
| ZigZag | 1651-1686 | Construct 直接子类（调 VarInt） |
| BytesInteger | 1203-1292 | Construct 直接子类 |
| CString | 1811-1834 | macro = StringEncoded(NullTerminated) |
| PascalString | 1778-1808 | macro = StringEncoded(Prefixed) |
| PaddedString | 1747-1775 | macro = StringEncoded(FixedSized) |
| GreedyString | 1837-1858 | macro = StringEncoded(GreedyBytes) |
| NullTerminated | 5050-5120 | Subconstruct 子类 |
| NullStripped | 5123-5178 | Subconstruct 子类 |
| StringEncoded | 1711-1744 | Adapter 子类 |
| Subconstruct | 787-810 | 抽象基类 |
| Adapter | 813-834 | Subconstruct 子类 |
| SymmetricAdapter | 837-846 | Adapter 子类 |
| Rebuild | 2975-3028 | Subconstruct 子类 |
| RawCopy | 4761-4814 | Subconstruct 子类 |
| Peek | 4486-4545 | Subconstruct 子类 |
| Pass（Streams 类） | 4687-4723 | Construct 直接子类（no-op） |

| Phase 7 构造器 | core.py 行号 | 备注 |
|---------------|-------------|------|
| If | 3912-3941 | macro = IfThenElse(..., Pass) |
| IfThenElse | 3944-3999 | Construct 直接子类 |
| Switch | 4002-4076 | Construct 直接子类（default=Pass） |
| Select | 3830-3884 | Construct 直接子类 |
| FocusedSeq | 3176-3318 | Construct 直接子类（PrefixedArray 内部用） |
| Pointer | 4384-4483 | Subconstruct 子类 |
| Prefixed | 4862-4931 | Subconstruct 子类 |
| Seek | 4594-4643 | Construct 直接子类 |

## 附录 B：现有 Node 系统清单（construct-rs）

**已实现 24 个 Node 变体**（`construct-rs/src/nodes/mod.rs`）：

- 原子节点（4）：FormatField / Bytes / GreedyBytes / BitsInteger
- 复合节点（5）：Struct / StructRef / Bitwise / Bytewise / Transform
- RO 节点（2）：Tell / Computed
- 填充节点（2）：BitPadding / Padding
- Array 系列（5）：Array / GreedyRange / PrefixedArray / RepeatUntil / Index
- 控制流（2）：StopIf / Element

**Phase 6 预计新增 Node**（按子任务）：

| 子任务 | 新增 Node | 数量 |
|-------|----------|------|
| 6.1 | VarIntNode / ZigZagNode / BytesIntegerNode + FormatField Float 扩展（不算新 Node） | 3 |
| 6.2 | CStringNode / NullTerminatedNode / NullStrippedNode / PaddedStringNode / PascalStringNode / GreedyStringNode | 6 |
| 6.3 | SubconstructNode / AdapterNode / SymmetricAdapterNode / RebuildNode / RawCopyNode / PeekNode / PassNode | 7 |
| **合计** | | **16 个新 Node**（24 → 40） |

完成后构造器清单 `docs/constructors-inventory.csv` 总进度预计：
- 当前 40/134（31%）
- Phase 6 完成：40 + 16 + 17（Primitives 含别名） = 73 个构造器有 Rust 实现（含 Python alias）
- 进度：~55%（与 MEMORY.md "Phase 6 完成后预计达 ~53%" 一致）

---

## 附录 C：§0 原则对照总览（L-01 对策）

本报告全部分析已对照 AGENTS.md §0 八条核心原则：

| §0 原则 | §A 段 | §B 段 | §C 段 |
|---------|-------|-------|-------|
| #1 一次 FFI | ✅ 推荐 Strings 独立 Node（避免多层 Adapter 嵌套） | ⚠️ Adapter 路线 1 违反；路线 3 仅用户 Adapter 违反（用户主动选择） | N/A |
| #2 无中间表示层 | ✅ Strings 方案 A 不引入 Python macro 的 Adapter 层 | ⚠️ BytesInteger raw FFI 路径需审查；用户 Adapter 中间态属用户域 | N/A |
| #3 输入输出侧无 trait 抽象层 | ✅ Subconstruct 仅 `Box<Node>`，不引入新 trait | ✅ Adapter 路线 3 不引入新 trait | N/A |
| #4 pyo3 核心依赖 | ✅ 所有新 Node 用 pyo3 直接操作 PyObject | ✅ 同左 | N/A |
| #5 mashumaro 式 API | N/A（不涉及用户面 dataclass） | ✅ Adapter 不破坏 StructMixin 用户面 | N/A |
| #6 enum_dispatch 静态分派 | ✅ 所有新 Node 加入 `Node` enum | ✅ 同左 | N/A |
| #7 Result<T, ConstructError> | ✅ 所有新 Node parse/build 返回 Result | ✅ 同左 | N/A |
| #8 Stream 抽象纯 Rust 内部 | ✅ Strings 直接操作 ParseStream/BuildStream | ✅ Pointer/Prefixed 同左（Phase 7） | N/A |

**结论**：Phase 6 设计方案（§A 推荐方案 A + §B 路线 3）整体符合 §0 原则，唯一需 PM 拍板的是 Adapter 用户面 PyCallable 路径的"性能 vs 兼容性"取舍。

---

> **报告完成时间**：2026-07-29
> **下一步**：PM 决策 5 项 → 6.0 测试框架启动 → 6.1/6.2/6.3 并行启动
