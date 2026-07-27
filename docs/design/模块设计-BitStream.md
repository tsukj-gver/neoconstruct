---
id: DESIGN-BitStream
status: active
phase: "3"
depends_on: [DESIGN-Architecture, ADR-009, ADR-010]
supersedes: []
superseded_by: []
last_updated: 2026-07-27
---

# 模块设计：BitStream 支持（Phase 3）

> **设计依据**：
> - `AGENTS.md` §0（核心原则：一次 FFI、无中间表示层、直接操作 Python 对象）、
>   §7（技术决策）、§8（编码红线）、§10（Python 参考速查）
> - `plans/phase3-bitstream/分析报告-Bitwise功能集.md`（功能集分析）
> - `plans/phase3-bitstream/总纲.md`（出口标准）
> - `construct-rs/src/stream.rs`（现有 ParseStream/BuildStream）
> - `construct-rs/src/nodes/mod.rs`（现有 Node enum + Construct trait）
> - Python 源码：`construct/construct/core.py`（Bitwise L1030、Bytewise L1081、
>   BitsInteger L1295、Padding L4136、BitStruct L4354、ByteSwapped L4817、
>   BitsSwapped L4836、Transformed L5233、Restreamed L5304）、
>   `construct/construct/lib/bitstream.py`（RestreamedBytesIO）、
>   `construct/construct/lib/binary.py`（bytes2bits 等转换工具）
>
> **角色**：ARCH
> **状态**：DESIGNING（修正版，针对 REV 驳回的 P1/P2 严重问题 + P3-P7/ST-2 建议修正）
> **创建时间**：2026-06-27
> **修正时间**：2026-06-27（ARCH 修正 P1-P7 + ST-2）

---

## 1. 概述

### 1.1 目标

为 construct-rs 引入位级（bit-level）二进制操作能力，支持在 bit 级别解析与构建数据。
覆盖 Python construct 2.10.70 的 Bitwise 完整功能集。

**性能目标**：BitStruct parse/build **≥10x** vs Python construct 2.10.70
（S-PERF 出口标准）。理想目标 ≥20x。

### 1.2 范围

| 构造器 | Python 行号 | 类型 | 本设计节点 |
|--------|------------|------|-----------|
| `Bitwise(subcon)` | 1030 | 核心包装器 | `BitwiseNode` |
| `BitStruct(...)` | 4354 | 语法糖 = `Bitwise(Struct(...))` | Python 侧 `BitStructMixin` + `BitwiseNode` |
| `BitsInteger(length, signed, swapped)` | 1295 | bit 原子 | `BitsIntegerNode` |
| `Bit` / `Nibble` / `Octet` | 1410/1414/1418 | 语法糖 = `BitsInteger(1/4/8)` | `BitsIntegerNode` |
| `Bytewise(subcon)` | 1081 | bit→byte 适配器 | `BytewiseNode` |
| `BitsSwapped(subcon)` | 4836 | 字节内 bit 序翻转 | `TransformNode`（BitSwap 变体） |
| `ByteSwapped(subcon)` | 4817 | 字节序翻转 | `TransformNode`（ByteSwap 变体） |
| `Padding(length, pattern)` | 4136 | 填充（bit/字节双义） | `BitPaddingNode`（bit 域）/ 复用字节级 `PaddingNode`（字节域） |

> **不在本阶段范围**：`Padded(length, subcon)`（带子构造器的填充）、
> `Restreamed`/`Transformed` 的通用 bytes→bytes 变换（仅实现 BitsSwapped/ByteSwapped 所需的两种）。

### 1.3 与现有架构的衔接

Phase 1-2 已实现 7 个 Node 变体（FormatField、Bytes、GreedyBytes、Struct、StructRef、
Tell、Computed），性能达 8-12x。本设计**不破坏**现有行为：

- 现有 `ParseStream` / `BuildStream` 的字节级 API（`read(n)` / `write(data)`）语义不变
- 现有 7 个 Node 的 parse/build/sizeof 行为不变
- 现有 `compile_schema` 签名向后兼容（新增可选参数）

**新增内容**：
- `ParseStream` / `BuildStream` 增加 bit 级状态与 API（§3）
- Node enum 新增 5 个变体（§6）
- `compile_schema` 增加 `bitwise` 与 `transform` 编译参数（§7）
- Python 侧新增 `BitStructMixin` 基类与 bit 描述符（§8）

---

## 2. 核心架构决策

### 2.1 决策 B1：不采用 Python 的"8 倍膨胀"bit 串机制

**Python 机制**（`lib/binary.py` `bytes2bits`）：把每个 bit 展开为一整个字节
（`\x00` 或 `\x01`），让 bit 流复用所有 bytes 接口。代价是 8 倍内存膨胀 + 8 倍读写次数
+ 查表 join 开销。

**construct-rs 决策**：**直接在原始字节上用位运算（shift + mask）操作 bit**，
不膨胀。理由：

1. **Stream 是纯 Rust 内部抽象（§0 / stream.rs L6）**——bit 流不需要复用 Python 的
   bytes 接口，Rust 内部可以直接用位运算，无需膨胀这一"适配手段"。
2. **一次 FFI 已满足**——Bitwise 的 subcon 在 Rust 内部执行，不跨 FFI，无需用膨胀
   来"复用字节节点的 Python 接口"。
3. **性能**——消除 8 倍膨胀是达成 ≥10x 目标的核心（详见 §10 性能假设）。

**后果**：bit 节点（BitsInteger 等）必须调用专门的 bit 级 API（`read_bits` / `write_bits`），
不能复用字节级 `read(n)`。这要求 Stream 抽象同时支持 bit 级和字节级操作（§3）。

### 2.2 决策 B2：ParseStream / BuildStream 增加 bit 游标状态

在现有字节游标（`pos`）基础上，新增 **bit 游标**（`bit_pos: u8`，取值 0-7）：

- `bit_pos == 0`：当前字节对齐，字节级 API（`read` / `write`）可用
- `bit_pos != 0`：处于字节内 bit 偏移，只有 bit 级 API（`read_bits` / `write_bits`）可用

**为什么不引入独立的 `BitStream` 类型**：
- `Construct` trait 的 parse/build 签名固定接收 `&mut ParseStream` / `&mut BuildStream`
  （nodes/mod.rs L77-105）。引入第二种 stream 类型要求修改 trait 签名，影响全部 7 个
  现有节点，破坏性极大且违背"无中间表示层"原则。
- bit 游标状态用同一个 Stream 类型表达，bit 节点和字节节点用不同 API 分流，
  互不干扰（bit 节点不会调用 `read`，字节节点在 bit 域内通过 `BytewiseNode` 重建对齐）。

### 2.3 决策 B3：bit 域内的 sizeof 单位是 bit，由边界节点做单位换算

Python 语义（分析报告 §6）：
- `Bitwise` 的 sizeof = `subcon.sizeof() // 8`（subcon 报 bit 数，外层报字节数）
- `Bytewise` 的 sizeof = `subcon.sizeof() * 8`（subcon 报字节数，外层报 bit 数）

**construct-rs 沿用此语义**：
- `BitsIntegerNode.sizeof()` = `length`（bit 数）
- `BitPaddingNode.sizeof()` = `length`（bit 数）
- `BitwiseNode.sizeof()` = `inner.sizeof() / 8`（bit→字节，非 8 倍数则 Err）
- `BytewiseNode.sizeof()` = `inner.sizeof() * 8`（字节→bit）
- `StructNode.sizeof()` 不变（累加字段 sizeof）——在 bit 域内累加的是 bit 数

**对齐校验**：`BitwiseNode.parse`/`build` 记录入口 `entry_bit_pos = stream.bit_pos()`，
退出时检查 `stream.bit_pos() == entry_bit_pos`（内层消耗的 bit 数是 8 倍数）。否则返回
`BitField` 错误。顶层 Bitwise 入口 bit_pos=0，退化为"退出 bit_pos==0"的绝对校验；
嵌套 Bitwise 入口 bit_pos 可能非 0，退出检查回到入口值（相对校验，见 §4.3、§9.3 BW-4）。
对齐 Python 的 `bits2bytes` 长度校验（`lib/binary.py` L118）与
`RestreamedBytesIO.close()` 缓冲清空校验（`bitstream.py` L50-54）。

### 2.4 决策 B4：Padding 单位由编译期上下文决定（bit 域 vs 字节域）

Python 的 `Padding(length)` 单位由所在 stream 决定（bit 流里是 bit，字节流里是字节）。
construct-rs 不膨胀，stream 类型统一，Padding 无法从 stream 类型判断单位。

**方案**：编译期由"是否在 Bitwise 子树内"决定。
- `compile_schema` 增加 `bitwise: bool` 参数（§7）。为 `true` 时，整个字段树在 bit 域，
  `Padding` 描述符编译为 `BitPaddingNode`（bit 单位）。
- 为 `false` 时，`Padding` 编译为字节级 `PaddingNode`（本阶段一并实现，见 §4.6）。

`BitwiseNode` 内部持有完整的 Node 子树（递归编译），因此 Bitwise 内部的 Padding
在编译时即处于 `bitwise=true` 上下文。

### 2.5 决策 B5：BitsSwapped / ByteSwapped 共用 TransformNode

Python 用通用 `Transformed(subcon, decodefunc, ...)` 实现。construct-rs 只需两种
字节级变换（`swapbytes` 整体反序、`swapbitsinbytes` 字节内 bit 反序），定义枚举
`ByteTransform` 即可，无需通用 bytes→bytes 函数指针（避免跨 FFI 传 Python callable）。

---

## 3. Stream 扩展设计

### 3.1 ParseStream 增加 bit 游标

```rust
/// 解析流：包装输入字节切片，维护字节游标与 bit 游标。
///
/// bit 游标 `bit_pos`（0-7）记录当前字节内的 bit 偏移：
/// - 0 表示字节对齐，字节级 `read(n)` 可用
/// - 1-7 表示处于 bit 偏移，只有 bit 级 `read_bits(n)` 可用
///
/// bit 顺序固定为 MSB-first（bit 0 是字节的最高有效位），
/// 对齐 Python construct 默认行为（分析报告 §1.4）。
/// LSB-first 由外层 `BitsSwapped` 包装实现（§4.5）。
#[derive(Debug)]
pub struct ParseStream<'a> {
    data: &'a [u8],
    pos: usize,
    bit_pos: u8,  // 新增：0-7
}
```

**新增 bit 级 API**：

```rust
impl<'a> ParseStream<'a> {
    // --- 现有字节级 API 保持不变（read / read_remaining / tell / ...） ---
    // 但 read(n) 增加 debug_assert!(bit_pos == 0) 前置条件（见 §3.3）

    /// 读取 `n` 个 bit，返回 MSB-first 的整数。
    ///
    /// 对齐 Python `bits2integer`（`lib/binary.py` L57）。
    /// 当 bit_pos == 0 且 n >= 8 时，内部走批量字节路径（直接读 n/8 字节左移，
    /// 剩余 n%8 bit 逐位）——这是 BytewiseNode 在对齐时的快路径。
    ///
    /// `n` 为 0 时返回 0（不推进游标）。
    /// `n` 超过 64 时返回 ConstructError（u64 装不下；Python 用大整数，
    /// 但 construct-rs 的 BitsInteger 限定 length <= 64，覆盖所有实际用例）。
    pub fn read_bits(&mut self, n: usize, path: &Path) -> Result<u64, ConstructError>;

    /// 跳过 `n` 个 bit（Padding 用，不返回值）。
    ///
    /// 等价于 `read_bits(n)` 但不构造返回值（仍校验流足够）。
    pub fn skip_bits(&mut self, n: usize, path: &Path) -> Result<(), ConstructError>;

    /// 当前 bit 偏移（0-7）。0 表示字节对齐。
    pub fn bit_pos(&self) -> u8;

    /// 是否字节对齐（bit_pos == 0）。
    pub fn is_byte_aligned(&self) -> bool;
}
```

### 3.2 BuildStream 增加 bit 游标

```rust
/// 构建流：输出缓冲区，支持字节级与 bit 级写入。
///
/// bit 写入采用"部分字节缓冲"策略：bit_pos > 0 时，正在填充的字节暂存于
/// `current_byte`，满 8 bit 后 push 到 `buf`。
#[derive(Debug)]
pub struct BuildStream {
    buf: Vec<u8>,
    bit_pos: u8,        // 新增：0-7
    current_byte: u8,   // 新增：bit_pos > 0 时的部分字节
}

impl BuildStream {
    // --- 现有字节级 API 保持不变（write / tell / into_bytes / ...） ---
    // 但 write(data) 增加 debug_assert!(bit_pos == 0) 前置条件（见 §3.3）
    // into_bytes() 增加 debug_assert!(bit_pos == 0)（release 不 panic，见 §3.3 ST-2）

    /// 写入 `n` 个 bit（value 的低 n 位，MSB-first）。
    ///
    /// 对齐 Python `integer2bits`（`lib/binary.py` L6）。
    /// 当 bit_pos == 0 且 n >= 8 时走批量字节路径。
    pub fn write_bits(&mut self, value: u64, n: usize);

    /// 写入 `n` 个值为 `bit`（0 或 1）的 bit（Padding 用）。
    pub fn write_padding_bits(&mut self, n: usize, bit: u8);

    /// 当前 bit 偏移。
    pub fn bit_pos(&self) -> u8;

    /// 是否字节对齐。
    pub fn is_byte_aligned(&self) -> bool;
}
```

### 3.3 字节级 API 的对齐前置条件

现有 `ParseStream::read(n)` 与 `BuildStream::write(data)` 假设字节对齐。引入 bit 游标后：

**`ParseStream::read(n)`**：
- **debug 模式**：`debug_assert!(self.bit_pos == 0, "byte-level read in bit-unaligned position")`
- **release 模式**：若 `bit_pos != 0`，返回 `ConstructError::Stream`（携带 path，
  "byte-level read at bit offset %d"）——read 有返回值，可走正常错误传播。

**`BuildStream::write(data)`**（无返回值，无法返回 Err）：
- **debug 模式**：`debug_assert!(self.bit_pos == 0, "byte-level write in bit-unaligned position")`
- **release 模式**：行为为**未定义**（不 panic、不返回 Err，但会破坏输出缓冲一致性）。
  这与 `Vec::push` 等"无返回值且不失败"的 Rust API 一致。前置条件由文档约定：
  字节级 write 仅在 `bit_pos == 0` 时可调用。
- 调用方保证：字节节点（FormatField、Bytes）出现在 bit 域内**必须**通过 `BytewiseNode`
  包裹（§4.4），BytewiseNode 保证调用字节 API 时已对齐（或自行提取对齐缓冲）。
  直接在 bit 域调用字节级 write 是编译期/用法错误，应当 fail fast（debug_assert）。

**`BuildStream::into_bytes()`**（消费 self，返回最终字节）：
- **debug 模式**：`debug_assert!(self.bit_pos == 0, "into_bytes with partial bit byte")`
- **release 模式**：不 panic。若 `bit_pos != 0`，返回当前缓冲（部分字节 `current_byte`
  的已写 bit 丢失）。这是调用方 bug（BitwiseNode.build 退出时已校验 bit_pos 回到入口，
  顶层 Bitwise 保证 bit_pos==0），release 模式不为此增加运行时分支。

**理由**：debug_assert 在开发/测试环境（CI 跑 debug build）捕获调用方 bug；release
模式遵循 Rust "无额外运行时检查"惯例，与 AGENTS.md §8 红线 #2（禁止 panic）一致——
**所有 panic 路径均限定在 `debug_assert!`（仅 debug build 触发），release 永不 panic**。
read 的 release 路径有 Err 返回值（可恢复），write/into_bytes 的 release 路径为
"调用方契约违反，行为未定义但不 panic"（与 std `Vec`/`slice` 的 unsafe-adjacent API 一致）。

> **与 §9.7 ST-1/ST-2 的一致性**：ST-1（read 在未对齐时 release 返回 Stream Err）、
> ST-2（into_bytes 在未对齐时 debug_assert，release 不 panic）均在此明确。

### 3.4 bit 操作的批量优化

`read_bits` / `write_bits` 在 `bit_pos == 0` 且 `n >= 8` 时走**批量字节路径**：

```rust
// read_bits 批量路径伪码（bit_pos == 0）
let full_bytes = n / 8;
let rem_bits = n % 8;
let mut result: u64 = 0;
if full_bytes > 0 {
    let chunk = self.read(full_bytes, path)?;  // 字节级，零拷贝
    for &b in chunk {
        result = (result << 8) | b as u64;
    }
}
// 剩余 rem_bits 逐位读
for _ in 0..rem_bits {
    let byte = self.data[self.pos];
    let bit = (byte >> (7 - self.bit_pos)) & 1;
    result = (result << 1) | bit as u64;
    self.bit_pos += 1;
    if self.bit_pos == 8 { self.bit_pos = 0; self.pos += 1; }
}
```

**性能意义**：`Bytewise(Float32b)` 在 bit 对齐时，4 字节批量读取与直接 `Int32ub` 等价
（零额外开销）。这是 Bytewise 常见用例（BitStruct 内整字节字段）的快路径。
未对齐时（如 Nibble 后接 Bytewise），走逐 bit 路径，较慢但正确——与 Python 行为一致
（Python 未对齐时也慢）。

> **设计取舍**：批量优化仅在对齐时启用。未对齐的 bit 拼接（跨字节边界）保持逐 bit
> 简单实现，避免预计算掩码表的复杂度。若基准测试显示未对齐路径是瓶颈，再优化。

---

## 4. 节点详细设计

### 4.1 BitsIntegerNode（bit 原子整数）

**对应 Python**：`BitsInteger(length, signed, swapped)`（core.py L1295）、
`Bit` / `Nibble` / `Octet`（L1410/1414/1418，均为 `BitsInteger` 的语法糖）。

```rust
/// bit 级整数节点：读取/写入 `length` 个 bit，返回 Python int。
///
/// 必须在 bit 域（BitwiseNode 内）使用。parse 调用 `stream.read_bits(length)`，
/// build 调用 `stream.write_bits(value, length)`。
///
/// # signed 语义
///
/// 对齐 Python `bits2integer(data, signed)`（binary.py L57）：
/// signed=true 时，最高位为 1 表示负数（二补码）：`result - (1 << length)`。
///
/// # swapped 语义（字节序）
///
/// 对齐 Python `swapbytesinbits`（binary.py L135）：在 read/write 前后对 bit 串
/// 按 8 位组反序。**仅 length 为 8 倍数时有定义**（否则 Python 抛 ValueError）。
/// 对应"Byte-Level Little-Endian"（分析报告 §2 四种组合）。
///
/// # sizeof
///
/// 返回 `length`（bit 数）。在 bit 域内由 StructNode 累加。
#[derive(Debug, Clone)]
pub struct BitsIntegerNode {
    length: usize,
    signed: bool,
    swapped: bool,
}

impl BitsIntegerNode {
    pub fn new(length: usize, signed: bool, swapped: bool) -> Self;
    pub fn length(&self) -> usize;
    pub fn signed(&self) -> bool;
    pub fn swapped(&self) -> bool;
}
```

**parse 逻辑**：
1. 校验 `length > 0`（对齐 Python `IntegerError: length must be positive`）。
2. 若 `swapped`：校验 `length % 8 == 0`（否则 `IntegerError`，对齐 binary.py L144）。
3. `raw = stream.read_bits(length, path)?`（MSB-first u64）。
4. 若 `swapped`：对 raw 的 bit 表示做 `swapbytesinbits`（按 8 位组反序）。
   实现方式：将 raw 拆成 `length/8` 字节，反序字节序，再拼回 u64。
5. 若 `signed` 且最高位为 1：`value = raw as i64 - (1i64 << length)`；否则 `value = raw as i64`。
   - length > 64 时已在 read_bits 报错；length <= 63 时 i64 安全。
   - length == 64 且 signed：用 i128 中间值或 u64::from_be 处理（边界条件 §9）。
6. 返回 `PyLong`（`value.into_py(py)`）。

**build 逻辑**：
1. 校验 obj 是 Python int（`obj.is_instance_of::<PyLong>()`），否则 `IntegerError`（对齐 L1377）。
2. 校验 `length > 0`。
3. 若 `swapped`：校验 `length % 8 == 0`。
4. 提取 `value: i128`（`obj.extract::<i128>()`，覆盖 i64 范围与大正数报错路径）。
5. 范围校验（对齐 integer2fits L24）：根据 signed 计算 min/max，越界 `IntegerError`。
6. 若 value < 0：`raw = (value + (1 << length)) as u64`（二补码）。
7. 若 `swapped`：对 raw 做 `swapbytesinbits`。
8. `stream.write_bits(raw, length)`。

**sizeof**：`Ok(self.length)`（bit 数）。

### 4.2 BitPaddingNode（bit 级填充）

**对应 Python**：`Padding(length)` 在 bit 域内的行为（通过 Padded + Pass，core.py L4136/L4175）。

```rust
/// bit 级填充节点：跳过/写入 `length` 个 bit。
///
/// parse：`stream.skip_bits(length)`，返回 Python `None`。
/// build：`stream.write_padding_bits(length, pattern_bit)`，接受任意 obj（忽略）。
/// sizeof：`length`（bit 数）。
///
/// # pattern 语义（对齐 Python bit 域 Padding）
///
/// Python `Padding(length, pattern)` 在 bit 域（Bitwise 内）通过 `Padded._build`
/// 写 `self.pattern * pad`（core.py L4239）。此时 stream 是 8 倍膨胀的 bit 串，
/// 每"bit 字节"只能是 `\x00`（bit 0）或 `\x01`（bit 1）。后续 `bits2bytes`
/// （binary.py L109）用 `BITS2BYTES_CACHE` 查表，cache 键仅含 0x00/0x01 序列
/// （由 `bytes2bits(int2byte(i))` 生成，binary.py L108）。
///
/// 因此 pattern 在 bit 域内**只有两个合法值**：
/// - `b"\x00"`（默认）→ 写 0x00 字节 → 填充 bit 0
/// - `b"\x01"` → 写 0x01 字节 → 填充 bit 1
/// - 其他 pattern（如 0x80、0xff）→ Python 在 bits2bytes 阶段 KeyError
///
/// construct-rs 采取**严格校验**（fail-fast，优于 Python 的延迟 KeyError）：
/// `BitPaddingNode::new` 要求 `pattern ∈ {0x00, 0x01}`，否则返回
/// `ConstructError::Padding`（"bit-domain padding pattern must be 0x00 or 0x01"）。
/// `pattern_bit = pattern & 1`（对合法值等价于 pattern 本身）。
#[derive(Debug, Clone)]
pub struct BitPaddingNode {
    length: usize,
    pattern_bit: u8,  // 0 或 1（由合法 pattern 0x00/0x01 导出）
}

impl BitPaddingNode {
    /// 创建 bit 级填充节点。
    ///
    /// `pattern` 必须是 0x00 或 0x01（对齐 Python bit 域 Padding 的合法值），
    /// 其他值返回 `ConstructError::Padding`（对应 Python bits2bytes 的 KeyError，
    /// 但在编译期/构造期提前暴露）。
    pub fn new(length: usize, pattern: u8) -> Result<Self, ConstructError>;
    pub fn length(&self) -> usize;
    pub fn pattern_bit(&self) -> u8;
}
```

**parse**：`stream.skip_bits(length)?; Ok(PyNone)`。

**build**：忽略 obj，`stream.write_padding_bits(length, self.pattern_bit); Ok(())`。

**sizeof**：`Ok(self.length)`。

> **关于 pattern 校验的位置**：`new()` 返回 `Result`，编译期
> （`build_node_from_descriptor`，§7.2）即完成校验。非法 pattern 在编译 schema
> 时报错，而非 parse/build 时——这与 Python 在 bits2bytes 阶段才 KeyError 不同，
> 但更符合 Rust 的 fail-fast 原则，且错误信息更清晰。
>
> **关于"pattern 字节取 MSB"方案的否决**：早期设计曾考虑 `pattern_bit = pattern >> 7`
> （取 MSB），但这对最常见的非零 pattern `0x01` 给出错误结果（0x01 >> 7 = 0，
> 而 Python 实际填充 bit 1）。`pattern & 1`（LSB）才匹配 Python 的 0x00→0、
> 0x01→1 两个合法 case。配合 {0x00, 0x01} 严格校验，两者等价。

### 4.3 BitwiseNode（核心包装器，bit 域边界）

**对应 Python**：`Bitwise(subcon)`（core.py L1030）。

```rust
/// bit 域包装器：在其内部 subcon 上建立 bit 级游标，结束时校验"消耗 bit 数为 8 倍数"。
///
/// 内部 `inner` 是一棵完整的 Node 子树（通常是 StructNode，由 BitStructMixin 编译）。
///
/// # parse 流程
///
/// 1. **记录入口 bit 游标** `entry_bit_pos = stream.bit_pos()`。
///    - 顶层 Bitwise：entry_bit_pos == 0（字节对齐入口，由外层保证）。
///    - 嵌套 Bitwise（Bitwise(Bitwise(...))）：entry_bit_pos 可能为 0-7
///      （内层 Bitwise 在外层 bit 域内继续，bit_pos 延续不重置，见 §13.4）。
/// 2. 调用 `inner.parse(stream, ...)`——inner 内部的 bit 节点通过 read_bits 推进 bit_pos。
/// 3. **校验 `stream.bit_pos() == entry_bit_pos`**（内层消耗的 bit 数是 8 倍数，
///    即回到入口的字节内偏移），否则 `ConstructError::BitField`
///    （"bit stream not byte-aligned after Bitwise (entry=%d, exit=%d)"）。
///    对齐 Python `RestreamedBytesIO.close()` 的缓冲清空校验（bitstream.py L50-54）：
///    Bitwise 的 Restreamed 用 decoderunit=1 byte / encoderunit=8 bits
///    （core.py L1068），close() 检查 rbuffer/wbuffer 为空 ⟺ 消耗 bit 数为 8 倍数
///    ⟺ exit_bit_pos == entry_bit_pos（相对字节边界）。
///
/// # build 流程
///
/// 1. **记录入口 bit 游标** `entry_bit_pos = stream.bit_pos()`。
/// 2. 调用 `inner.build(obj, stream, ...)`——inner 内部的 bit 节点通过 write_bits
///    填充部分字节。
/// 3. **校验 `stream.bit_pos() == entry_bit_pos`**，否则 `ConstructError::BitField`。
///
/// # sizeof
///
/// `inner.sizeof(ctx)? / 8`——inner 报 bit 数，外层报字节数。
/// 若 `inner.sizeof()` 非 8 倍数，返回 Err（对齐 Python 的隐式约束：
/// build/parse 时暴露，sizeof 时也应当 fail fast）。
///
/// # 嵌套 Bitwise 的对齐语义（与 §9.3 BW-4 一致）
///
/// 顶层 Bitwise 入口 entry_bit_pos=0，退化为"退出时 bit_pos==0"的绝对校验。
/// 嵌套 Bitwise 入口 entry_bit_pos 可能为 1-7，退出检查回到入口值（相对校验）。
/// 两者统一为 `exit == entry`，无需特判。
#[derive(Debug)]
pub struct BitwiseNode {
    inner: Box<Node>,  // 持有子树根（Box 因 Node 递归）
}

impl BitwiseNode {
    pub fn new(inner: Node) -> Self;
    pub fn inner(&self) -> &Node;
}
```

> **关于 `Box<Node>`**：Node enum 变体通常持有非递归数据。BitwiseNode 的 inner 是
> 另一棵 Node 子树，必须 `Box` 以打破 enum 的无限大小。这是 Node enum 中首个
> 递归变体，enum_dispatch 仍正常工作（dispatch 到 BitwiseNode::parse，内部解引用 Box）。

### 4.4 BytewiseNode（bit→byte 适配器）

**对应 Python**：`Bytewise(subcon)`（core.py L1081）。必须在 Bitwise 内使用。

```rust
/// bit→byte 适配器：在 bit 域内为内部 subcon 重建字节对齐的子流。
///
/// # parse 流程（两条路径）
///
/// **对齐快路径**（`stream.bit_pos() == 0`）：
///   直接 `inner.parse(stream, ...)`——inner 是字节节点（FormatField/Bytes/Struct），
///   在同一 stream 上字节级读取，零额外开销。这是 Bytewise(Int16ub) 在 BitStruct
///   字节边界处的常见快路径。
///
/// **未对齐慢路径**（`stream.bit_pos() != 0`）：
///   1. `size = inner.sizeof(ctx)?`（字节节点必须有定长，否则 Err，对齐 Python
///      Transformed 路径要求定长）。
///   2. 从 bit 流提取 `size * 8` bit 到临时 `Vec<u8>` 缓冲（逐 bit 组装）。
///   3. 在临时缓冲上创建 `ParseStream`，`inner.parse(&mut sub_stream, ...)`。
///   4. 校验 sub_stream 被完全消耗（对齐 Python stream_tell 测量）。
///
/// # build 流程（对称）
///
/// **对齐快路径**：直接 `inner.build(obj, stream, ...)`。
/// **未对齐慢路径**：inner build 到临时 `BuildStream` → 提取字节 → 逐 bit 写入主流。
///
/// # sizeof
///
/// `inner.sizeof(ctx)? * 8`——inner 报字节数，外层报 bit 数。
#[derive(Debug)]
pub struct BytewiseNode {
    inner: Box<Node>,  // 字节级子树（FormatField/Bytes/Struct/StructRef 等）
}

impl BytewiseNode {
    pub fn new(inner: Node) -> Self;
    pub fn inner(&self) -> &Node;
}
```

> **设计说明**：BytewiseNode 的对齐快路径是性能关键——它让 `BitStruct(a/Nibble, b/Bytewise(Int32ub))`
> 中 b 的读取在 Nibble 对齐后（bit_pos=4）走慢路径，但 `BitStruct(b/Bytewise(Int32ub), a/Nibble)`
> 中 b 在开头（bit_pos=0）走快路径。鼓励用户把 Bytewise 字段放在 BitStruct 开头或
> 8 倍数边界处以获得最佳性能。

### 4.5 TransformNode（BitsSwapped / ByteSwapped）

**对应 Python**：`BitsSwapped`（core.py L4836，用 swapbitsinbytes）、
`ByteSwapped`（core.py L4817，用 swapbytes）。两者均为字节级变换，要求 subcon 定长。

```rust
/// 字节级变换节点：读取 subcon.sizeof() 字节，按变换函数处理后喂给 subcon。
///
/// 对应 Python Transformed(subcon, func, size, func, size)。
/// 本阶段仅支持两种变换（见 ByteTransform），覆盖 BitsSwapped/ByteSwapped。
///
/// # parse
///
/// 1. `size = inner.sizeof(ctx)?`（定长要求，否则 Err，对齐 ByteSwapped L4832）。
/// 2. `data = stream.read(size, path)?`（字节级，要求 bit_pos == 0）。
/// 3. `data = transform.apply(data)`（原地或新分配）。
/// 4. 在 data 上创建临时 ParseStream，`inner.parse(&mut sub_stream, ...)`。
///
/// # build
///
/// 1. inner build 到临时 BuildStream。
/// 2. `data = transform.apply(&sub_stream.into_bytes())`。
/// 3. `stream.write(&data)`。
///
/// # sizeof
///
/// `inner.sizeof(ctx)?`（变换不改变长度）。
#[derive(Debug)]
pub struct TransformNode {
    inner: Box<Node>,
    transform: ByteTransform,
}

/// 字节级变换种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteTransform {
    /// 整体字节反序（swapbytes，binary.py L123）。用于 ByteSwapped。
    /// 例：[0x01, 0x02, 0x03] → [0x03, 0x02, 0x01]
    ByteSwap,
    /// 每字节内 bit 反序（swapbitsinbytes，binary.py L149）。用于 BitsSwapped。
    /// 例：[0xF0, 0x00] → [0x0F, 0x00]
    BitSwap,
}

impl ByteTransform {
    /// 对 data 就地/新建应用变换，返回变换后的 Vec<u8>。
    pub fn apply(&self, data: &[u8]) -> Vec<u8>;
}

impl TransformNode {
    pub fn new(inner: Node, transform: ByteTransform) -> Self;
    pub fn inner(&self) -> &Node;
    pub fn transform(&self) -> ByteTransform;
}
```

**ByteSwap.apply**：`data.iter().rev().copied().collect()`（整体反序）。

**BitSwap.apply**：每字节用预计算的 `BIT_REVERSE_TABLE: [u8; 256]`（编译期常量，
对应 Python `SWAPBITSINBYTES_CACHE`，binary.py L149）查表。

> **为什么 BitsSwapped/ByteSwapped 用 TransformNode 而非 BitwiseNode**：
> 这两者是**字节域**变换（要求 bit_pos == 0，在字节流上操作），与 BitwiseNode 的
> bit 域机制正交。BitsSwapped 改变的是"字节内 bit 的解释顺序"，ByteSwapped 改变的是
> "字节序"，都是在已对齐的字节上做后处理。它们可以包裹 BitwiseNode
> （`BitsSwapped(Bitwise(...))`），此时 TransformNode 在外层先变换字节再交给 Bitwise。
>
> **已知限制（Phase 3.1）：变长 subcon 不支持**。Python `BitsSwapped`（core.py L4838
> docstring 明确 "Does NOT require a fixed sized subcon"）和 `ByteSwapped` 在 subcon
> 变长时走 `Restreamed` 回退（core.py L4858-4859、L4828-4829），encoderunit=1 逐字节
> 变换。construct-rs 的 TransformNode 统一要求定长（step 1 调用 `inner.sizeof()`，
> §9.5 TR-1）。因此 `BitsSwapped(GreedyBytes)` 等变长用例在 Phase 3.1 会返回
> `SizeofError`/`Generic`，Python 成功。
>
> **处理方式**：标记为 Phase 3.1 已知限制（见 §13.5）。§1.2 已排除"通用 Restreamed
> 字节级变换"的实现，仅支持 BitsSwapped/ByteSwapped 的定长路径。变长支持若需要，
> 后续阶段可扩展 TransformNode 增加"逐字节流式变换"路径（参考 RestreamedBytesIO
> encoderunit=1 模式）。注意：常见用例（`BitsSwapped(Bitwise(...))`、
> `ByteSwapped(Bytes(N))`）subcon 均为定长，Phase 3.1 覆盖主要场景。

### 4.6 PaddingNode（字节级填充，补全 Phase 1 遗留）

**对应 Python**：`Padding(length)` 在字节域的行为（core.py L4136）。

> Python `Padding(length)` = `Padded(length, Pass, pattern)`。本设计在字节域实现
> 简化版（subcon 固定为 Pass，不暴露 Padded 的通用形式）。

```rust
/// 字节级填充节点：跳过/写入 `length` 个字节。
///
/// parse：`stream.read(length)?` 丢弃，返回 Python `None`。
/// build：写入 `pattern * length`。
/// sizeof：`length`。
///
/// length 可为常量或表达式（复用 BytesLength，与 BytesNode 一致）。
#[derive(Debug, Clone)]
pub struct PaddingNode {
    length: BytesLength,  // 复用 nodes/bytes.rs 的 BytesLength
    pattern: u8,          // 单字节 pattern（默认 0）
}

impl PaddingNode {
    pub fn new_const(length: usize, pattern: u8) -> Self;
    pub fn new_expr(program: ExprProgram, pattern: u8) -> Self;
}
```

**parse**：求值 length → `stream.read(length, path)?`（丢弃）→ `Ok(PyNone)`。
**build**：求值 length → `stream.write(&[pattern; length])`（或预分配 Vec）。
**sizeof**：Const → `Ok(length)`；Expr → Err（同 BytesNode）。

> **length < 0 / 表达式求值为负**：返回 `PaddingError`（对齐 Python Padded L4219）。
> 用 `ConstructError::FieldLength` 或新增 `ConstructError::Padding` 变体（§7.3）。

### 4.7 语法糖节点说明（Bit / Nibble / Octet / BitStruct）

**Bit / Nibble / Octet**：不新增 Node 变体，编译期展开为 `BitsIntegerNode`：
- `Bit()` → `BitsIntegerNode::new(1, false, false)`
- `Nibble()` → `BitsIntegerNode::new(4, false, false)`
- `Octet()` → `BitsIntegerNode::new(8, false, false)`

展开在 Python 侧描述符层完成（`Bit()` 返回 `BitsInteger(1)` 描述符），Rust 侧只看到
`BitsIntegerNode`。

**BitStruct**：不新增 Node 变体，等价于 `BitwiseNode(StructNode(...))`。
- Python 侧提供 `BitStructMixin` 基类（§8），`__init_subclass__` 标注 `bitwise=True`。
- 编译时（§7），`compile_schema` 见 `bitwise=True`，把根 `StructNode` 包入 `BitwiseNode`，
  且字段树在 `bitwise=true` 上下文下编译（Padding → BitPaddingNode）。

---

## 5. bit 转换工具（Rust 实现）

对应 Python `lib/binary.py`。construct-rs 不需要全部工具（直接位运算替代），
但 `swapbytesinbits` 语义需要显式实现（用于 BitsInteger.swapped）。

### 5.1 swapbytesinbits（用于 BitsInteger swapped）

**Python**（binary.py L135）：对 bit 串按 8 位组反序组顺序。要求长度 8 倍数。

**Rust 实现**（在 `BitsIntegerNode` 内部，对 u64 操作）：

```rust
/// 对 length-bit 的值做 swapbytesinbits（按 8 位组反序）。
/// length 必须是 8 倍数。
fn swapbytesinbits_u64(raw: u64, length: usize) -> u64 {
    debug_assert!(length % 8 == 0 && length <= 64);
    let n_bytes = length / 8;
    let mut result = 0u64;
    // raw 的高 length 位是有效 bit，按字节（8 位组）反序
    for i in 0..n_bytes {
        // 取 raw 的第 i 个字节（从高到低）
        let shift = length - 8 * (i + 1);
        let byte = ((raw >> shift) & 0xFF) as u8;
        // 放到 result 的反序位置（从低到高）
        result |= (byte as u64) << (8 * i);
    }
    result
}
```

**注意**：这里 raw 是 `read_bits` 返回的 MSB-first 整数（高位在前）。swapbytesinbits
改变的是"字节序"（8 位组的顺序），不改变字节内 bit 顺序。等价于把 big-endian 字节序
转成 little-endian 字节序。

### 5.2 BIT_REVERSE_TABLE（用于 TransformNode BitSwap）

**Python** `SWAPBITSINBYTES_CACHE`（binary.py L149）：每字节内 bit 反序。

**Rust**：编译期常量查表（`const` 或 `lazy_static` / `once_cell`）。

```rust
/// 预计算的 bit 反序表：BIT_REVERSE_TABLE[b] = b 的 bit 序反序。
/// 例：BIT_REVERSE_TABLE[0xF0] = 0x0F, [0x01] = 0x80。
const BIT_REVERSE_TABLE: [u8; 256] = {
    let mut table = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let mut v = i as u8;
        let mut r = 0u8;
        let mut j = 0;
        while j < 8 {
            r = (r << 1) | (v & 1);
            v >>= 1;
            j += 1;
        }
        table[i] = r;
        i += 1;
    }
    table
};
```

> 用 `const` 表 + 索引替代 Python 的 dict 查表，零运行时初始化开销。

### 5.3 不需要的工具（直接位运算替代）

| Python 工具 | construct-rs 替代 |
|------------|------------------|
| `bytes2bits`（8 倍展开） | `read_bits` 直接位运算（决策 B1） |
| `bits2bytes`（8 倍收缩） | `write_bits` 直接位运算 |
| `integer2bits`（int→bit 串） | `write_bits(value, n)` |
| `bits2integer`（bit 串→int） | `read_bits(n)` + signed 处理 |
| `swapbytes`（整体反序） | `data.iter().rev()` |

---

## 6. Node enum 扩展

### 6.1 新增变体

在 `construct-rs/src/nodes/mod.rs` 的 `Node` enum 新增 5 个变体：

```rust
#[derive(Debug)]
#[enum_dispatch(Construct)]
pub enum Node {
    // ... 现有 7 个变体 ...
    FormatField(FormatFieldNode),
    Bytes(BytesNode),
    GreedyBytes(GreedyBytesNode),
    Struct(StructNode),
    StructRef(StructRefNode),
    Tell(TellNode),
    Computed(ComputedNode),
    // --- Phase 3 新增 ---
    /// bit 原子整数（BitsInteger/Bit/Nibble/Octet）。
    BitsInteger(BitsIntegerNode),
    /// bit 级填充（BitStruct 内 Padding）。
    BitPadding(BitPaddingNode),
    /// bit 域包装器（Bitwise/BitStruct 根）。
    Bitwise(BitwiseNode),
    /// bit→byte 适配器（Bitwise 内 Bytewise）。
    Bytewise(BytewiseNode),
    /// 字节级变换（BitsSwapped/ByteSwapped）。
    Transform(TransformNode),
    /// 字节级填充（普通 Struct 内 Padding）。
    Padding(PaddingNode),
}
```

> **新增 6 个变体**（5 个 bit 相关 + 1 个字节级 Padding）。enum_dispatch 自动为新增
> 变体生成 `Construct` trait 的 match 分派。

### 6.2 `has_expressions` / `compute_ro_value` 扩展

`Node::has_expressions()`（mod.rs L156）：新增变体默认返回 `false`，但
`BitwiseNode` / `BytewiseNode` / `TransformNode` 需递归检查内部子树：

```rust
pub fn has_expressions(&self) -> bool {
    match self {
        Node::Struct(s) => s.has_expressions(),
        Node::Bitwise(b) => b.inner().has_expressions(),
        Node::Bytewise(b) => b.inner().has_expressions(),
        Node::Transform(t) => t.inner().has_expressions(),
        _ => false,
    }
}
```

> **关于 `BitsInteger` / `Padding` / `BitPadding` / `Padding`（字节级）节点**：
> Phase 3.1 这四个节点的 length 均为常量（表达式 length 在 `build_node_from_descriptor`
> 编译期被显式拒绝，见 §7.2 P4），因此 `has_expressions` 返回 `false` 是正确的。
> 后续 Phase 3.2 若引入 `BitsLength`/`BytesLength` 枚举（表达式 length），
> 这些节点需改为 `match` 分支检查 length 是否为 Expr 变体（参考 BytesNode 现有模式）。

`Node::compute_ro_value()`（mod.rs L189）：新增变体均非 RO 节点，落入既有 `_` 分支
（返回 Generic 错误）。无需修改。

### 6.3 新增模块文件

```
construct-rs/src/nodes/
├── bits_integer.rs    ← BitsIntegerNode
├── bit_padding.rs     ← BitPaddingNode
├── bitwise.rs         ← BitwiseNode
├── bytewise.rs        ← BytewiseNode
├── transform.rs       ← TransformNode + ByteTransform + BIT_REVERSE_TABLE
└── padding.rs         ← PaddingNode（字节级）
```

每个文件遵循现有节点模块的组织（模块级文档 + struct + Construct impl + `#[cfg(test)] mod tests`）。

---

## 7. 编译管线集成

### 7.1 compile_schema 签名扩展

`compile_schema`（compile.rs L88）新增两个可选参数，向后兼容：

```rust
#[pyfunction]
#[pyo3(signature = (cls, field_names, descriptors, modes=None, expr_programs=None, bitwise=False))]
pub fn compile_schema(
    py: Python<'_>,
    cls: &Bound<'_, PyType>,
    field_names: Vec<String>,
    descriptors: Vec<Py<PyAny>>,
    modes: Option<Vec<String>>,
    expr_programs: Option<Vec<Option<Py<PyAny>>>>,
    bitwise: Option<bool>,  // 新增：是否编译为 bit 域（BitStructMixin 传 true）
) -> PyResult<CompiledSchema>;
```

**`bitwise` 参数的作用**：
- `false`（默认）：字段树在字节域编译。Padding → PaddingNode（字节级）。
- `true`：字段树在 bit 域编译。Padding → BitPaddingNode。根 StructNode 被包入 BitwiseNode。

### 7.2 build_node_from_descriptor 扩展

`build_node_from_descriptor`（compile.rs L228）增加 `bitwise: bool` 参数（传递给递归），
并新增对 bit 描述符的识别（按 type name，复用 §3.7 的纯 Python 描述符识别模式）：

```rust
fn build_node_from_descriptor(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    bitwise: bool,  // 新增
) -> Result<Node, ConstructError> {
    // ... 现有 1-5 分支（FormatField/Bytes/GreedyBytes/StructRef/Tell/Computed）...

    // Phase 3 新增描述符识别（type name 匹配）：
    match type_name.to_str()? {
        // BitsInteger 描述符 → BitsIntegerNode
        "BitsIntegerDescriptor" => {
            // Phase 3.1：仅支持常量 length（整数）。
            // 表达式 length（FieldRef/ExprRef，_expr_params 非空）显式拒绝：
            //   返回 ConstructError::Compilation（清晰错误，闭环 §8.2 _expr_params 路径）。
            // Phase 3.2 再实现 BitsLength 枚举（与 BytesNode 对称）。
            if /* desc.length 是表达式（_is_expr） */ {
                return Err(ConstructError::Compilation {
                    message: "BitsInteger expression length not supported in Phase 3.1 \
                              (use a constant length)".into(),
                    path: format!("field {}", field_index),
                });
            }
            // 常量 length：从 desc.length 提取 i64，校验非负（避免 usize 回绕），
            // 再转为 usize（对齐 P6：编译期检测负 length）。
            let length_i64: i64 = desc.getattr("length")?.extract()?;
            if length_i64 < 0 {
                return Err(ConstructError::Compilation {
                    message: format!("BitsInteger length {} must be non-negative", length_i64),
                    path: format!("field {}", field_index),
                });
            }
            let length = length_i64 as usize;
            // 校验 length <= MAX_BITS_INTEGER (64)，提前暴露 BI-3（对齐 P5）。
            if length > MAX_BITS_INTEGER {
                return Err(ConstructError::Compilation {
                    message: format!(
                        "BitsInteger length {} exceeds 64-bit limit (max {})",
                        length, MAX_BITS_INTEGER
                    ),
                    path: format!("field {}", field_index),
                });
            }
            let signed: bool = desc.getattr("signed")?.extract()?;
            let swapped: bool = desc.getattr("swapped")?.extract()?;
            return Ok(Node::BitsInteger(BitsIntegerNode::new(length, signed, swapped)));
        }
        // Padding 描述符 → 根据 bitwise 选择 BitPaddingNode 或 PaddingNode
        "PaddingDescriptor" => {
            // Phase 3.1：length 同上（常量，校验非负）。表达式 length 同样拒绝。
            if /* desc.length 是表达式 */ {
                return Err(ConstructError::Compilation {
                    message: "Padding expression length not supported in Phase 3.1".into(),
                    path: format!("field {}", field_index),
                });
            }
            let length_i64: i64 = desc.getattr("length")?.extract()?;
            if length_i64 < 0 {
                return Err(ConstructError::Compilation {
                    message: format!("Padding length {} must be non-negative", length_i64),
                    path: format!("field {}", field_index),
                });
            }
            let length = length_i64 as usize;
            let pattern: u8 = desc.getattr("pattern")?.extract()?;
            return if bitwise {
                // bit 域：BitPaddingNode::new 严格校验 pattern ∈ {0x00, 0x01}（P1）
                Ok(Node::BitPadding(BitPaddingNode::new(length, pattern)?))
            } else {
                Ok(Node::Padding(PaddingNode::new_const(length, pattern)))
            };
        }
        // Bytewise 描述符 → BytewiseNode（递归编译内部 subcon）
        "BytewiseDescriptor" => {
            let inner_desc = desc.getattr("subcon")?;
            let inner_node = build_node_from_descriptor(py, &inner_desc, field_index, expr_programs, false)?;
            return Ok(Node::Bytewise(BytewiseNode::new(inner_node)));
        }
        // BitsSwapped / ByteSwapped → TransformNode
        "BitsSwappedDescriptor" => {
            let inner_desc = desc.getattr("subcon")?;
            let inner_node = build_node_from_descriptor(py, &inner_desc, field_index, expr_programs, false)?;
            return Ok(Node::Transform(TransformNode::new(inner_node, ByteTransform::BitSwap)));
        }
        "ByteSwappedDescriptor" => {
            let inner_desc = desc.getattr("subcon")?;
            let inner_node = build_node_from_descriptor(py, &inner_desc, field_index, expr_programs, false)?;
            return Ok(Node::Transform(TransformNode::new(inner_node, ByteTransform::ByteSwap)));
        }
        // Bitwise 描述符 → BitwiseNode（递归编译内部 subcon，bitwise=true）
        "BitwiseDescriptor" => {
            let inner_desc = desc.getattr("subcon")?;
            let inner_node = build_node_from_descriptor(py, &inner_desc, field_index, expr_programs, true)?;
            return Ok(Node::Bitwise(BitwiseNode::new(inner_node)));
        }
        _ => {}
    }
    // ... 现有 6 兜底 ...
}
```

> **递归 `bitwise` 上下文传播**：
> - 进入 `BitwiseDescriptor` / `bitwise=true` 根时，内部递归调用传 `true`。
> - 进入 `BytewiseDescriptor` / `BitsSwappedDescriptor` / `ByteSwappedDescriptor` 时，
>   内部递归传 `false`（这些节点重建字节域）。
> - 这样 `Bitwise(Bytewise(Padding(4)))` 中，最内层 Padding 编译为 PaddingNode（字节级），
>   因为 Bytewise 已转回字节域。语义与 Python 一致。

### 7.3 错误类型扩展

`ConstructError`（error.rs）新增一个变体用于 bit 域对齐错误：

```rust
pub enum ConstructError {
    // ... 现有变体 ...
    /// bit 域对齐错误：Bitwise/Bytewise 结束时 bit 游标非 0。
    /// 对齐 Python RestreamedBytesIO.close() 的残留校验。
    BitField {
        message: String,
        path: String,
    },
}
```

映射到 Python：`BitFieldError` 或回退到 `StreamError`/`ValueError`
（`_errors.py` 中若无 `BitFieldError`，用 `StreamError`）。

**Phase 3 用到的其他错误变体**：

| 变体 | 用途 | Phase 3 引入点 |
|------|------|---------------|
| `Compilation { message, path }` | 编译期错误（表达式 length 拒绝、负 length、length>64、非法 pattern）。
                                  **复用 Phase 1-2 既有变体**（compile.rs 已用）。 | §7.2（P4/P5/P6 修复） |
| `Padding { message, path }` | bit 域 Padding pattern 非 {0x00, 0x01}（P1 修复）。
                               **需新增**（若 Phase 1-2 未定义）。映射 Python `PaddingError`。
                               若不想新增变体，可复用 `FieldLength`（语义略宽，但 Python
                               PaddingError 本身也是 FieldError 族）。 | §4.2 BitPaddingNode::new |
| `FieldLength { ... }` | 字节域 Padding 负长度（既有，对齐 Python PaddingError）。 | §4.6 PaddingNode |

> **Padding 变体的取舍建议**：推荐**新增 `ConstructError::Padding`** 变体，原因：
> 1. Python 有独立的 `PaddingError`（core.py L4146），映射清晰；
> 2. bit 域 pattern 错误与字节域 length 错误语义不同（一个是 pattern 非法，一个是 length 负），
>    分开比塞进 `FieldLength` 更清晰；
> 3. 若 DEV 倾向最小变更，可暂复用 `FieldLength`，标记为后续重构点。

---

## 8. Python 层 API

### 8.1 BitStructMixin 基类

在 `construct-rs/python/construct/_mixin.py` 新增 `BitStructMixin`：

```python
class BitStructMixin:
    """BitStruct 基类。子类用 @dataclass 装饰，字段使用 bit 级描述符
    （Nibble、BitsInteger、BitPadding 等）。

    等价于 Python construct 的 BitStruct。编译时整体被 BitwiseNode 包裹，
    字段树在 bit 域编译（Padding → BitPaddingNode）。

    使用方式::

        @dataclass
        class Header(BitStructMixin):
            flag: int = field(Bit())           # 1 bit
            value: int = field(BitsInteger(10)) # 10 bits
            reserved: int = rfield(Padding(5))  # 5 bits padding

        Header.parse(b'\\xbe\\xef')  # 2 字节 = 16 bits
    """

    def __init_subclass__(cls, **kwargs):
        # 设置 bitwise 标志，供 _compile_schema_for_class 读取
        cls._construct_bitwise = True
        super().__init_subclass__(**kwargs)
```

`_compile_schema_for_class`（_mixin.py L716）扩展：读取 `getattr(cls, '_construct_bitwise', False)`，
传入 `compile_schema(..., bitwise=bitwise)`。

### 8.2 bit 描述符（纯 Python，type-name 识别）

在 `construct-rs/python/construct/_descriptors.py`（或新建 `_bit_descriptors.py`）定义：

```python
class BitsIntegerDescriptor:
    """BitsInteger(length, signed=False, swapped=False) 描述符。

    _expr_params 返回 {"length": <value>}（若 length 是 FieldRef/ExprRef）。
    signed/swapped 为常量 bool，不参与表达式编译。
    """
    __slots__ = ("length", "signed", "swapped")
    _expr_params = property(lambda self: {"length": self.length} if _is_expr(self.length) else {})

    def __init__(self, length, signed=False, swapped=False):
        self.length = length
        self.signed = signed
        self.swapped = swapped


def BitsInteger(length, signed=False, swapped=False):
    return BitsIntegerDescriptor(length, signed, swapped)

def Bit():    return BitsInteger(1)
def Nibble(): return BitsInteger(4)
def Octet():  return BitsInteger(8)


class PaddingDescriptor:
    """Padding(length, pattern=b'\\x00') 描述符。单位由编译期 bitwise 上下文决定。"""
    __slots__ = ("length", "pattern")
    _expr_params = property(lambda self: {"length": self.length} if _is_expr(self.length) else {})

    def __init__(self, length, pattern=b"\x00"):
        self.length = length
        self.pattern = pattern[0]  # 取首字节

def Padding(length, pattern=b"\x00"):
    return PaddingDescriptor(length, pattern)


class BytewiseDescriptor:
    __slots__ = ("subcon",)
    _expr_params = {}
    def __init__(self, subcon): self.subcon = subcon

def Bytewise(subcon): return BytewiseDescriptor(subcon)


class BitsSwappedDescriptor:
    __slots__ = ("subcon",)
    _expr_params = {}
    def __init__(self, subcon): self.subcon = subcon

def BitsSwapped(subcon): return BitsSwappedDescriptor(subcon)


class ByteSwappedDescriptor:
    __slots__ = ("subcon",)
    _expr_params = {}
    def __init__(self, subcon): self.subcon = subcon

def ByteSwapped(subcon): return ByteSwappedDescriptor(subcon)


class BitwiseDescriptor:
    __slots__ = ("subcon",)
    _expr_params = {}
    def __init__(self, subcon): self.subcon = subcon

def Bitwise(subcon): return BitwiseDescriptor(subcon)
```

> **`length` 表达式支持**：BitsInteger(length) 和 Padding(length) 的 length 可以是
> FieldRef/ExprRef（如 `BitsInteger(count)`）。`_expr_params` 协议返回
> `{"length": <expr>}`，复用 Phase 2 的表达式编译管线。Rust 侧 `BitsIntegerNode`
> 的 length 字段需扩展为 `BitsLength`（Const 或 Expr），与 `BytesNode` 对称。
> 为简化首版实现，**Phase 3.1 仅支持常量 length**，表达式 length 标记为后续子任务。

---

## 9. 边界条件清单

### 9.1 BitsIntegerNode

| # | 场景 | 预期行为 | Python 对应 |
|---|------|---------|------------|
| BI-1 | `length == 0` | parse/build 返回 `IntegerError`（length must be positive） | core.py L1366 |
| BI-2 | `length < 0`（常量负数） | **编译期** `Compilation` 错误（"length must be non-negative"）。
                                      实现于 `build_node_from_descriptor`（§7.2）：
                                      i64 提取后校验 < 0，避免 usize 回绕 | —（construct-rs 加严） |
| BI-3 | `length > 64` | **编译期** `Compilation` 错误（"length exceeds 64-bit limit"）。
                                      实现于 `build_node_from_descriptor`（§7.2）。
                                      Python 无此限制（大整数）；construct-rs 限定 u64 | —（construct-rs 文档化限定） |
| BI-4 | `length == 64` 且 `signed=true` | 用 i128 中间值计算二补码，避免溢出 | — |
| BI-5 | `signed=true`，最高位为 1 | 返回负数（`raw - (1 << length)`） | binary.py L73 |
| BI-6 | `swapped=true` 且 `length % 8 != 0` | parse/build 返回 `IntegerError`（LE 仅定义于 8 倍数） | core.py L1370 + binary.py L144 |
| BI-7 | build 时 obj 非 int | 返回 `IntegerError`（value is not an integer） | core.py L1377 |
| BI-8 | build 时 value 超出范围 | 返回 `IntegerError`（out of range） | binary.py L24 |
| BI-9 | 流中 bit 数不足 | parse 返回 `Stream` 错误（expected N, found M） | stream_read |

### 9.2 BitPaddingNode

| # | 场景 | 预期行为 |
|---|------|---------|
| BP-1 | `length == 0` | parse 不推进游标，返回 None；build 不写入 |
| BP-2 | 流中 bit 数不足 | parse 返回 `Stream` 错误 |
| BP-3 | build 时 obj 为任意值（含 None） | 忽略 obj，写入 pattern bit |

### 9.3 BitwiseNode

| # | 场景 | 预期行为 | Python 对应 |
|---|------|---------|------------|
| BW-1 | inner 消耗 bit 数非 8 倍数 | parse/build 返回 `BitField` 错误（exit bit_pos ≠ entry bit_pos）。
                                  校验为 `exit_bit_pos == entry_bit_pos`（§4.3），顶层 Bitwise
                                  入口 bit_pos=0 退化为"退出 bit_pos==0" | RestreamedBytesIO.close L50 |
| BW-2 | inner.sizeof() 非 8 倍数 | sizeof 返回 `BitField`/`Generic` 错误 | 隐式约束 |
| BW-3 | inner 含表达式字段（sizeof Err） | sizeof 返回 Err（向上传播） | — |
| BW-4 | 嵌套 Bitwise（Bitwise(Bitwise(...))） | 内层 Bitwise 要求进入时 bit_pos 任意，退出时回到入口 bit_pos | Python 同 |
| BW-5 | 空 BitStruct（Bitwise(Struct())） | parse/build 不读写，sizeof=0 | — |

### 9.4 BytewiseNode

| # | 场景 | 预期行为 | Python 对应 |
|---|------|---------|------------|
| BY-1 | inner.sizeof() 返回 Err（变长） | parse/build 返回 `BitField` 错误（Bytewise 需定长 subcon） | Transformed 要求 |
| BY-2 | bit 对齐（bit_pos==0） | 走快路径，零额外开销 | — |
| BY-3 | bit 未对齐（bit_pos!=0） | 走慢路径，逐 bit 提取到临时缓冲 | — |
| BY-4 | 在 Bitwise 外使用 | **已知差异（非语义对齐点）**：Python `Bytewise` 在 Bitwise 外用时，
                          `Transformed` 读 `size*8` 字节后 `bits2bytes` 查表 KeyError
                          （core.py L1106）。construct-rs 的 BytewiseNode 在 Bitwise 外
                          （bit_pos==0 的字节流）走对齐快路径，等价于直接 `inner`，不报错。
                          两者都属"语义无意义用法"，但异常方式不同。文档化为已知差异，
                          不试图复现 Python 的 KeyError（construct-rs 不膨胀，无 bits2bytes 查表阶段） | Python KeyError vs construct-rs 无错 |

### 9.5 TransformNode

| # | 场景 | 预期行为 | Python 对应 |
|---|------|---------|------------|
| TR-1 | inner.sizeof() 返回 Err（变长 subcon） | 返回 `SizeofError`/`Generic`（TransformNode 需定长 subcon）。
                                                  **Phase 3.1 已知限制**（§4.5、§13.5）：
                                                  Python BitsSwapped/ByteSwapped 对变长 subcon
                                                  走 Restreamed 回退（encoderunit=1 逐字节变换），
                                                  construct-rs 暂不支持 | core.py L4832（ByteSwapped 定长路径）；
                                                  L4858-4859（BitsSwapped Restreamed 回退，Phase 3.1 不支持） |
| TR-2 | BitSwap 应用 | 每字节 bit 反序（查表） | swapbitsinbytes |
| TR-3 | ByteSwap 应用 | 整体字节反序 | swapbytes |
| TR-4 | inner 是 BitwiseNode | 先变换字节再进入 bit 域（外层 Transform 先于内层 Bitwise） | BitsSwapped(Bitwise(...)) |

### 9.6 PaddingNode（字节级）

| # | 场景 | 预期行为 | Python 对应 |
|---|------|---------|------------|
| PD-1 | length 表达式求值为负 | 返回 `FieldLength`/`Padding` 错误 | Padded L4219 |
| PD-2 | 流中字节不足 | parse 返回 `Stream` 错误 | stream_read |
| PD-3 | length == 0 | 不读写，返回 None | — |

### 9.7 Stream bit 游标

| # | 场景 | 预期行为 |
|---|------|---------|
| ST-1 | bit 域内调用字节级 `read(n)`（bit_pos!=0） | debug 断言失败 / release 返回 `Stream` 错误 |
| ST-2 | `into_bytes()` 时 bit_pos != 0 | **debug_assert!** 失败（debug 模式）；release 不 panic，
                                      返回当前缓冲（部分字节丢失，调用方 bug）。详见 §3.3 |
| ST-3 | read_bits(n) 流不足 | 返回 `Stream` 错误（expected N bits, found M） |
| ST-4 | write_bits(value, n) 中 value 高位被截断 | 仅取低 n 位（对齐 integer2fits 的 mask 行为） |

---

## 10. 性能假设（性能门禁 §1）

### 10.1 瓶颈识别（Python construct 的 Bitwise 开销来源）

| 瓶颈 | Python 来源 | 开销量级 |
|------|------------|---------|
| 8 倍内存膨胀 | `bytes2bits` 把每 bit 展开为 1 字节（binary.py L96） | 8x 分配 + GC 压力 |
| 8 倍读写次数 | bit 流每 bit 一次 stream_read/write | 8x I/O 调用 |
| 查表 + join | `BYTES2BITS_CACHE` dict 查表 + `b"".join`（binary.py L105） | O(n) 哈希 + 拼接 |
| RestreamedBytesIO 缓冲管理 | read/write 循环 + 缓冲切片（bitstream.py L18-48） | 每 chunk 多次切片 |
| bits2integer Python 循环 | `for b in data: number = (number << 1) \| b`（binary.py L70） | 纯 Python 逐位 |

### 10.2 可证伪预测

**预测**：construct-rs 的 BitStruct parse/build **≥10x** vs Python construct 2.10.70。

**理由（逐一对应瓶颈）**：

1. **消除 8 倍膨胀**：construct-rs 直接位运算，零额外分配。BitStruct(2 字节=16 bit)
   Python 需分配 16 字节 bit 串 + 多次切片；construct-rs 仅 read_bits 操作原始 2 字节。
   → 预期贡献 ~3-4x。
2. **消除 Python 循环**：Rust 位运算（shift + mask）编译为 native 指令，
   bits2integer 的 Python 循环 vs Rust 批量字节路径，~10-20x 单点差距。
   → 预期贡献 ~3-5x。
3. **消除 RestreamedBytesIO 缓冲**：construct-rs 无缓冲层，直接在 stream 上操作。
   → 预期贡献 ~2x。
4. **PyLong 构造**：parse 返回 Python int（`value.into_py`），仍有一次 PyObject 分配，
   但仅一次（一次 FFI 已满足），vs Python 多次中间对象。→ 不构成瓶颈。

**累积预测**：3-4x（膨胀）× 2x（循环→位运算，保守，因 PyLong 构造不变）≈ 6-8x，
加上缓冲层消除与零成本抽象，预计达 **10-15x**。

**基准场景**（S-PERF 验证用）：
- `BitStruct(a/Nibble, b/BitsInteger(10), c/Padding(1))`（2 字节，16 bit）
- parse 10000 次 + build 10000 次，对比 Python construct 2.10.70
- 目标：≥10x（< 10x 触发性能门禁 §2 暂停）

### 10.3 验证方法

1. 在 `construct-rs/tests/benchmark.py` 新增 `bench_bitstruct` 基准。
2. 对比对象：Python construct 2.10.70 的等价 `BitStruct`。
3. 报告：parse ns/op、build ns/op、speedup ratio。
4. 若 < 10x，用 `bench_breakdown.py` 风格定位瓶颈（read_bits vs write_bits vs PyLong 构造）。

### 10.4 性能风险点

- **未对齐 Bytewise 慢路径**：逐 bit 提取 32 bit 给 Float32b 较慢。但这是边缘用例，
  对齐快路径已覆盖常见场景。若基准显示是瓶颈，可加掩码预计算优化。
- **`Box<Node>` 间接寻址**：BitwiseNode/BytewiseNode/TransformNode 的 inner 经 Box，
  多一次指针解引用。但这是每字段一次（非每 bit），可忽略。
- **enum_dispatch 与递归 Node**：BitwiseNode 内部的 Node 子树仍走 enum_dispatch 静态分派，
  无动态分派开销。但子树根的 match 多了一层，预期 < 5% 影响。

---

## 11. 与 Python 版本对应表

| Python 类/函数 | construct-rs 类型/节点 | 备注 |
|---------------|----------------------|------|
| `Bitwise(subcon)` (core.py L1030) | `BitwiseDescriptor` → `Node::Bitwise(BitwiseNode)` | 递归编译 subcon，bitwise=true |
| `BitStruct(...)` (L4354) | `BitStructMixin` → 根 `BitwiseNode(StructNode)` | 语法糖，无独立 Node |
| `BitsInteger(length, signed, swapped)` (L1295) | `BitsIntegerDescriptor` → `Node::BitsInteger` | length 常量（表达式后续） |
| `Bit()` (L1410) | `BitsInteger(1)` 描述符 | Python 侧展开 |
| `Nibble()` (L1414) | `BitsInteger(4)` 描述符 | Python 侧展开 |
| `Octet()` (L1418) | `BitsInteger(8)` 描述符 | Python 侧展开 |
| `Bytewise(subcon)` (L1081) | `BytewiseDescriptor` → `Node::Bytewise` | 对齐快路径 + 未对齐慢路径 |
| `BitsSwapped(subcon)` (L4836) | `BitsSwappedDescriptor` → `Node::Transform(BitSwap)` | swapbitsinbytes |
| `ByteSwapped(subcon)` (L4817) | `ByteSwappedDescriptor` → `Node::Transform(ByteSwap)` | swapbytes |
| `Padding(length, pattern)` (L4136) bit 域 | `PaddingDescriptor` → `Node::BitPadding` | bitwise=true 时 |
| `Padding(length, pattern)` 字节域 | `PaddingDescriptor` → `Node::Padding` | bitwise=false 时 |
| `Transformed` (L5233) | `Node::Transform`（仅 2 种变换） | 不实现通用 bytes→bytes |
| `Restreamed` (L5304) | 不直接对应（被 BitwiseNode 的 stream bit 游标替代） | 决策 B1：不膨胀 |
| `bytes2bits` (binary.py L96) | `read_bits` 位运算 | 决策 B1 |
| `bits2bytes` (L109) | `write_bits` 位运算 | 决策 B1 |
| `integer2bits` (L6) | `write_bits(value, n)` | 位运算 |
| `bits2integer` (L57) | `read_bits(n)` + signed 处理 | 位运算 |
| `swapbytesinbits` (L135) | `swapbytesinbits_u64`（§5.1） | BitsInteger swapped |
| `swapbitsinbytes` (L149) | `BIT_REVERSE_TABLE`（§5.2） | TransformNode BitSwap |
| `swapbytes` (L123) | `iter().rev()` | TransformNode ByteSwap |
| `RestreamedBytesIO` (bitstream.py L6) | ParseStream/BuildStream 的 bit 游标 | 决策 B2 |

---

## 12. 与其他模块的交互

### 12.1 依赖（本设计依赖的已实现模块）

| 模块 | 依赖点 |
|------|--------|
| `stream.rs`（ParseStream/BuildStream） | **需修改**：增加 bit_pos 状态与 bit 级 API（§3） |
| `nodes/mod.rs`（Node enum + Construct trait） | **需修改**：新增 6 个变体 + has_expressions 递归（§6） |
| `nodes/bytes.rs`（BytesLength） | **复用**：PaddingNode 的 length 类型 |
| `compile.rs`（compile_schema + build_node_from_descriptor） | **需修改**：新增 bitwise 参数 + 6 种描述符识别（§7） |
| `schema.rs`（CompiledSchema） | **无需修改**：BitwiseNode 作为根节点，static_size 缓存自动工作 |
| `expr.rs`（ExprProgram） | **复用**：PaddingNode/BitsIntegerNode 表达式 length（后续子任务） |
| `error.rs`（ConstructError） | **需修改**：新增 BitField 变体（§7.3） |
| `context.rs`（Context） | **无需修改**：bit 节点不特殊使用 context |
| `instance.rs` | **无需修改**：BitStructMixin 走标准 StructNode 实例化路径 |

### 12.2 被依赖（本设计将影响的后续模块）

| 后续模块 | 影响 |
|--------|------|
| Flag / FlagsEnum（Phase 4+） | Flag = 1 bit 整数，可在 BitStruct 内复用 BitsIntegerNode |
| Switch / IfThenElse（Phase 4+） | 可嵌套在 BitStruct 内（作为 BitwiseNode 内部子树） |
| Prefixed / Pointer（Phase 4+） | 一般不在 bit 域用，但架构上不阻止 |

### 12.3 与架构设计文档的兼容性

本设计**不修改** `docs/design/架构设计.md` 的核心约定：
- §C.1 Construct trait 签名不变（parse/build/sizeof 三方法）
- §C.4 Stream 抽象"纯 Rust 内部，不跨 FFI"——bit 游标仍在此约束内
- §B.2 compile_schema 一次 FFI——新增 bitwise 参数不破坏
- §0 一次 FFI、无中间表示层——bit 节点直接产 `Py<PyAny>`（PyLong/PyNone），无 Rust 中间类型

**唯一需在架构设计文档补充的点**：Node enum 的递归变体（`Box<Node>`）。
BitwiseNode/BytewiseNode/TransformNode 首次引入递归 Node，建议在架构设计 §C.2 补注。
不构成冲突，仅为补充说明。

---

## 13. 需要 PM 决策的事项

### 13.1 [需确认] BitsInteger 表达式 length 是否纳入 Phase 3.1

本设计 §8.2 标注"Phase 3.1 仅支持常量 length"。理由：
- BitsInteger(count) 这种动态 bit 宽度较少见
- 表达式 length 需扩展 BitsIntegerNode.length 为 `BitsLength` 枚举（与 BytesNode 对称），
  增加实现复杂度

**请 PM 决策**：Phase 3.1 是否包含表达式 length？
- **选项 A（推荐）**：Phase 3.1 仅常量 length，表达式 length 作为 Phase 3.2 子任务
- **选项 B**：Phase 3.1 一次性支持表达式 length（增加 ~20% 实现工作量）

### 13.2 [需确认] PaddingNode 字节级是否纳入 Phase 3.1

Padding 在字节域的使用（普通 Struct 内 `Padding(4)` 跳过 4 字节）是 Phase 1/2 的遗留
（之前未实现）。本设计 §4.6 一并补全。

**请 PM 决策**：字节级 PaddingNode 是否纳入 Phase 3.1？
- **选项 A（推荐）**：纳入（实现简单，~30 行代码，且 BitStruct 用户的 Padding 在 bit 域，
  普通用户的 Padding 在字节域，两者都常见）
- **选项 B**：仅实现 BitPaddingNode，字节级 Padding 推迟到后续

### 13.3 [需确认] Bytewise 未对齐慢路径的实现优先级

Bytewise 未对齐慢路径（§4.4，逐 bit 提取到临时缓冲）实现较复杂（~50 行）。
对齐快路径是性能关键且实现简单。

**请 PM 决策**：
- **选项 A（推荐）**：Phase 3.1 实现两条路径（完整 Bytewise 语义）
- **选项 B**：Phase 3.1 仅实现对齐快路径，未对齐返回 `BitField` 错误（限制用户必须
  把 Bytewise 字段放在字节边界），未对齐慢路径作为 Phase 3.2

### 13.4 [信息] Bitwise 嵌套 Bitwise 的语义

Python 允许 `Bitwise(Bitwise(...))`，但内层 Bitwise 在已膨胀的 bit 流上再膨胀。
construct-rs 不膨胀，内层 Bitwise 直接延续 bit_pos（不重置）。语义上：
- 外层 Bitwise 进入 bit 域，bit_pos 从 0 开始
- 内层 Bitwise 进入"嵌套 bit 域"，bit_pos 从当前值继续
- 内层 Bitwise 退出时校验 bit_pos 回到入口值（消耗 8 倍数 bit）

这与 Python 行为一致（Python 内层 Bitwise 在已膨胀流上工作，等价于继续读 bit）。
**无需 PM 决策**，记录此处的语义澄清。

### 13.5 [信息] BitsSwapped / ByteSwapped 变长 subcon 限制（Phase 3.1 已知限制）

Python `BitsSwapped`（core.py L4838 docstring "Does NOT require a fixed sized subcon"）
和 `ByteSwapped` 对变长 subcon 走 `Restreamed` 回退（L4858-4859 / L4828-4829），
encoderunit=1 逐字节变换。construct-rs 的 `TransformNode`（§4.5）统一要求定长 subcon
（step 1 调 `inner.sizeof()`），因此 `BitsSwapped(GreedyBytes)` 等变长用例在 Phase 3.1
返回 `SizeofError`/`Generic`。

**理由**：
- §1.2 已排除通用 `Restreamed` 字节级变换（仅实现 BitsSwapped/ByteSwapped 的定长路径）
- 常见用例（`BitsSwapped(Bitwise(...))`、`ByteSwapped(Bytes(N))`）subcon 均为定长，
  Phase 3.1 覆盖主要场景
- 变长逐字节流式变换需扩展 TransformNode 增加"无预知 size 的流式 apply"路径，
  增加 ~40 行实现，与 Phase 3.1 的核心目标（BitStruct 性能）关联度低

**无需 PM 决策**，标记为已知限制，文档化于 §4.5、§9.5 TR-1。若后续用户反馈需要，
可作为 Phase 3.2+ 子任务补充。

---

## 14. 设计完整性自检

### 14.1 Python API 映射覆盖

分析报告 §2 的 9 个构造器全部覆盖（§11 对应表）：
- Bitwise ✓（BitwiseNode）
- BitStruct ✓（BitStructMixin + BitwiseNode）
- Bytewise ✓（BytewiseNode）
- BitsSwapped ✓（TransformNode BitSwap）
- ByteSwapped ✓（TransformNode ByteSwap）
- BitsInteger ✓（BitsIntegerNode）
- Bit / Nibble / Octet ✓（Python 侧展开为 BitsInteger）
- Padding ✓（BitPaddingNode + PaddingNode）

**公开 API 覆盖率**：分析报告 §4 的 10 个公开名称（BitwisableString 除外，它是
FlagsEnum 语法糖，与 bit 流无关），全部覆盖。

### 14.2 边界条件覆盖

分析报告 §7 的注意事项全部处理：
- Bytewise 必须在 Bitwise 内 → BytewiseNode 设计为 bit 域内使用（§4.4）
- BitwisableString 与 bit 流无关 → 不在本阶段范围
- Padding 需同步实现 → BitPaddingNode + PaddingNode（§4.2/§4.6）
- 不自动 padding，剩余 bit 报错 → BitwiseNode 对齐校验（§4.3 BW-1）

### 14.3 编码红线遵守（AGENTS.md §8）

| 红线 | 遵守情况 |
|------|---------|
| 禁止 unwrap/expect（非测试） | 所有 bit 操作返回 Result / write_padding_bits 不失败 |
| 禁止 panic | read_bits 流不足返回 Err；write_bits 不失败（无返回值）。
              **所有 panic 路径限定为 `debug_assert!`（仅 debug build 触发）**：
              read/write/into_bytes 的 bit_pos 前置条件、swapbytesinbits_u64 的
              debug_assert（§3.3、§5.1、§9.7 ST-2）。release 永不 panic |
| 禁止 TODO/FIXME | 无 |
| 禁止硬编码魔法数字 | length 64（u64 上限）用常量 `MAX_BITS_INTEGER: usize = 64` |
| pub 项有文档注释 | 所有新增 pub struct/fn/method 带 `///` |
| 错误携带 path | read_bits/skip_bits 接收 path 参数；BitField/Compilation/Padding 错误含 path |
| parse/build 对称性 | 所有节点同时实现 parse 和 build |

---

> **设计文档结束（修正版 v2）**。本次修正针对 REV 驳回的 2 项严重问题（P1/P2）
> 和 5 项非阻塞建议（P3-P7 + ST-2），详见过程记录 §"ARCH 修正说明（v2）"。
> 等待 REV 复审。



