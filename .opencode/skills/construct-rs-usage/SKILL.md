---
id: construct-rs-usage
name: construct-rs 用法
description: 用 construct-rs 定义和解析二进制协议。触发：实现二进制协议解析/构建时。
---

# construct-rs 用法 SKILL

> 本 SKILL 自包含。读完本文件即可从零实现复杂二进制协议（条件分支 + bitfield + CRC），
> 不需要查阅任何其他文件。所有示例可直接复制运行。

## 1. construct-rs 是什么（30 秒）

**construct-rs** 是高性能二进制解析/构建库：**Rust 内核 + Python `@dataclass` API**。
用 mashumaro 式声明语法表达二进制协议结构，性能 >=4x vs Python `construct` 2.10.70
（10x 为理想）。

**核心理念**：

- 用 `@dataclass class X(StructMixin)` 声明协议，字段用 `field(subcon)` 标注类型
- 编译在 `__init_subclass__`（类创建时自动触发），用户无感
- **parse/build 对称**：parse 把字节变对象，build 把对象变字节，互为逆运算
- **字段名直接引用**（不是 `this.xxx`）：`Bytes(n)` 中 `n` 是同一 Struct 中已声明的字段
- 一次 FFI：parse/build 各只有一次 Python<->Rust 边界穿越

**前置条件**：安装好 construct-rs（`maturin develop --release`），Python 中 `from construct import ...` 可用。

## 2. 最小示例（2 分钟上手）

```python
from dataclasses import dataclass
from construct import StructMixin, field, Int8ub, Int16ub, Bytes

@dataclass
class Header(StructMixin):
    magic: int = field(Int16ub)        # 2 字节大端无符号整数
    version: int = field(Int8ub)       # 1 字节
    payload: bytes = field(Bytes(4))   # 固定 4 字节

# build：对象 -> 字节
h = Header(magic=0xCAFE, version=1, payload=b"ABCD")
built = h.build()
assert built == b"\xCA\xFE\x01ABCD"

# parse：字节 -> 对象
parsed = Header.parse(built)
assert parsed == h
```

**关键 4 点**：

1. `@dataclass class X(StructMixin)` 双装饰（顺序：先 StructMixin 继承，再 @dataclass）
2. 字段声明用 `field(subcon)` —— `field` 是 RW 字段，parse 能读、build 要填
3. `parse` 是类方法 `Header.parse(bytes)`，返回 `Header` 实例
4. `build` 是实例方法 `instance.build()`，返回 bytes

## 3. API 速查

### 3.1 字段声明函数（3 种模式）

| 函数 | 模式 | 用途 | build 时 |
|------|------|------|---------|
| `field(subcon)` | RW（读写） | **默认**，普通字段 | 从实例属性取值 |
| `rfield(subcon)` | RO（只读） | 自动计算字段（Tell/Computed/Const/Rebuild/Check/Index/Element/Terminated/Default） | 自动算，不取实例值 |
| `wfield(subcon)` | WO（只写） | padding/reserved 字段 | 取实例值 |
| `field(subcon, default=X)` | RW + 默认值 | 可选字段 | 自动 kw_only=True |

**RO 字段**：`init=False`，不出现在 `__init__` 参数中。用于 `Tell()`/`Computed(expr)`/
`Const(...)` 等"build 时自动算"的字段。

**已知约束**：`Checksum` 和 `Peek` **不能用 `rfield()`**（不在 build_ro_value 允许列表），
必须用 `field()`。详见 5 易错点。

### 3.2 表达式系统（字段引用 + 算术）

字段名**直接引用**（不是 `this.xxx`，construct-rs 已废弃 `this`）。所有引用必须
指向同一 Struct 中**前序声明**的 int 字段。

```python
@dataclass
class P(StructMixin):
    count: int = field(Int8ub)                # 先声明
    items: list = field(Array(count, Byte))   # 后引用（数组长度 = count）
    total: int = rfield(Computed(count + 1))  # 表达式：count + 1
```

**支持的运算符**（编译为 Rust VM 指令，运行时零 FFI）：

- 算术：`+ - * // %`
- 位运算：`& | ^ << >>`
- 一元：`- ~`
- 比较：`== != < <= > >=`（返回 0/1）
- **不支持** `/`（浮点）、`and/or`（短路求值无法重载）
- **不支持浮点常量**（VM 栈是 i64）

### 3.3 构造器清单（按类别）

#### Primitives（整数/浮点）

| 名称 | 说明 |
|------|------|
| `Int8ub` `Int8ul` `Int8sb` `Int8sl` | 8-bit 整数（u/s = unsigned/signed，b/l = big/little endian） |
| `Int16ub` `Int16ul` `Int16sb` `Int16sl` | 16-bit 整数 |
| `Int24ub` `Int24ul` `Int24sb` `Int24sl` | 24-bit 整数（3 字节） |
| `Int32ub` `Int32ul` `Int32sb` `Int32sl` | 32-bit 整数 |
| `Int64ub` `Int64ul` `Int64sb` `Int64sl` | 64-bit 整数 |
| `Float16b` `Float16l` | 16-bit 浮点（half precision） |
| `Float32b` `Float32l` | 32-bit 浮点（single） |
| `Float64b` `Float64l` | 64-bit 浮点（double） |
| `Byte` `Short` `Int` `Long` | 别名（= Int8ub / Int16ub / Int32ub / Int64ub） |
| `Half` `Single` `Double` | 别名（= Float16b / Float32b / Float64b） |
| `Bytes(length)` | 定长字节串（length 是 int 或字段引用表达式） |
| `GreedyBytes` | 读到 EOF 的字节串（singleton，无参数） |
| `BytesInteger(length, signed=False, swapped=False)` | 任意字节数整数（1-16 字节） |
| `VarInt` | LEB128 无符号变长整数（singleton） |
| `ZigZag` | 有符号变长整数（VarInt + ZigZag 编码，singleton） |

#### Strings

| 名称 | 说明 |
|------|------|
| `CString(encoding, ...)` | C 风格 null 终止字符串（如 `CString("utf8")`） |
| `GreedyString(encoding)` | 读到 EOF 解码字符串 |
| `PaddedString(length, encoding)` | 固定长度填充字符串（右剥离 null） |
| `PascalString(lengthfield, encoding)` | 长度前缀字符串（如 `PascalString(Byte, "utf8")`） |
| `NullTerminated(subcon, term=b"\x00", ...)` | null 终止包装器（inner 任意） |
| `NullStripped(subcon, pad=b"\x00")` | null 剥离包装器（右剥离 pad） |

**编码约束**：必须用显式后缀编码（`utf8`/`utf_8`/`u8`/`utf_16_le`/`utf_16_be`/
`utf_32_le`/`utf_32_be`/`ascii`）。**不接受** `utf16`/`utf32` 无后缀形式。
#### Bit / Bitfield（必须在 BitStructMixin 或 Bitwise 内）

| 名称 | 说明 |
|------|------|
| `Bit()` | 1-bit 整数（= BitsInteger(1)） |
| `Nibble()` | 4-bit 整数（= BitsInteger(4)） |
| `Octet()` | 8-bit 整数（= BitsInteger(8)） |
| `BitsInteger(length, signed=False, swapped=False)` | 任意 bit 宽整数（length 是 int） |
| `Bitwise(subcon)` | bit 域包装器（在普通 Struct 嵌入 bit 域） |
| `Padding(length, pattern=b"\x00")` | 填充（bit 域中 length 单位是 bit） |
| `Bytewise(subcon)` | 在 bit 域内重建字节流（嵌套字节级 subcon） |
| `BitsSwapped(subcon)` | 字节内 bit 序翻转 |
| `ByteSwapped(subcon)` | 字节序整体翻转 |

#### Adapter（包装器）

| 名称 | 说明 |
|------|------|
| `Const(value, subcon=None)` | 常量字段（parse 校验、build 用 value） |
| `Default(subcon, value)` | 默认值（build 时 obj 为 None 用 value 表达式） |
| `Check(func)` | 断言（必须 rfield，func 是表达式，非真抛 CheckError） |
| `Rebuild(subcon, func)` | build 时重算（必须 rfield，func 是表达式） |
| `Computed(expr)` | 表达式计算（必须 rfield，expr 引用前序字段） |
| `Peek(subcon)` | 预读不消费（**必须 field()**，不消费字节） |
| `RawCopy(subcon)` | 捕获原始字节（parse 返回 dict） |
| `Subconstruct(subcon)` | 纯转发包装 |
| `Hex(subcon)` | Hex 显示包装（int/bytes/dict） |
| `HexDump(subcon)` | HexDump 显示包装（仅 bytes/dict） |
| `Enum(subcon, **mapping)` | 整数 <-> label 字符串映射 |
| `FlagsEnum(subcon, **flags)` | 整数 -> flags dict 映射 |
| `Mapping(subcon, mapping)` | 通用对象映射（无映射时报错，与 Enum 不同） |
| `OneOf(subcon, valids)` | 值 in valids 校验 |
| `NoneOf(subcon, invalids)` | 值 not in invalids 校验 |
| `Union(parsefrom, *subcons, **named)` | 多视角联合体 |
| `Checksum(field, hashfunc, ...)` | 校验和（详见 4.4） |
| `Tell()` | 记录当前流位置（singleton，必须 rfield） |
| `Terminated` | EOF 断言（singleton，未到 EOF 抛错） |
| `Probe(into=None, lookahead=None)` | 调试探针（打印 context） |
| `Pass` | No-op（singleton，parse 返回 None，build 不写字节） |

#### Conditional（条件分支）

| 名称 | 说明 |
|------|------|
| `If(condfunc, subcon)` | macro：等价 `IfThenElse(cond, sub, Pass)` |
| `IfThenElse(condfunc, then, else)` | 双分支 |
| `Switch(keyfunc, cases, default=Pass)` | 多分支（keyfunc 是字段引用或 int） |
| `Select(*subcons)` | 多尝试（首个成功者胜出） |
| `FocusedSeq(parsebuildfrom, *subcons)` | 聚焦字段序列（parse 返回聚焦字段值） |
| `StopIf(condfunc)` | 早停（必须 rfield，条件为真时停止后续字段/迭代） |
| `Renamed` | 命名包装器（通常用 `"name" / subcon` 创建） |

#### Streams（流操作）

| 名称 | 说明 |
|------|------|
| `Seek(at, whence=0)` | 流定位（whence: 0=Start, 1=Current, 2=End） |
| `Pointer(offset, subcon, relativeOffset=False)` | 绝对偏移读写（不占主流位置） |
| `Prefixed(lengthfield, subcon, includelength=False)` | 长度前缀子流（lengthfield 是字节计数；`includelength=True` 时 length 含 lengthfield 自身大小，详见 §4.5.5） |

#### Array（数组）

| 名称 | 说明 |
|------|------|
| `Array(count, subcon, discard=False)` | 固定次数数组（count 是 int 或字段引用） |
| `GreedyRange(subcon, discard=False)` | 读到流结束或子构造器失败的数组 |
| `PrefixedArray(countfield, subcon)` | 前缀长度数组（countfield 是元素计数） |
| `RepeatUntil(terminator, subcon, discard=False)` | 终止表达式数组（**不接收 lambda**，terminator 必须引用 Element 字段） |
| `Index()` | 取当前数组迭代下标（singleton，必须 rfield） |
| `Element()` | RepeatUntil 终止表达式中"当前元素"引用（singleton，必须 rfield） |

#### Struct（结构）

| 名称 | 说明 |
|------|------|
| `StructMixin`（基类） | 用户 Struct 基类（`@dataclass class X(StructMixin)`） |
| `BitStructMixin`（基类） | BitStruct 基类（字段总 bit 数必须 8 的倍数） |
| `Sequence(*subcons, **named)` | 位置序字段序列（parse 产出 list） |
| `AlignedStruct(modulus, **fields)` | 宏：每字段 `Aligned(modulus, ...)` 包装 |
| `Aligned(modulus, subcon, pattern=b"\x00")` | 单字段对齐（modulus 是 int >= 2） |
| `NamedTuple(name, fields, subcon)` | namedtuple 包装 |

#### Other

| 名称 | 说明 |
|------|------|
| `Timestamp(subcon, unit, epoch)` | Datetime <-> Arrow 对象（需 `pip install arrow`） |
| `ProcessXor(padfunc, subcon)` | XOR 字节变换（padfunc 不接受 callable） |
| `ProcessRotateLeft(amount, group, subcon)` | 位旋转左移 |
| `HashAlgo`（enum） | 内置哈希算法（MD5/SHA1/SHA256/SHA512/CRC32/ADLER32） |
| `Adapter`（基类） | 用户自定义 Adapter（继承写 `_decode`/`_encode`） |

### 3.4 基本使用模式

```python
from construct import StructMixin, BitStructMixin, field, rfield, wfield

@dataclass
class MyProtocol(StructMixin):
    # RW 字段（默认）
    header: int = field(Int16ub)
    # RW + 默认值（自动 kw_only=True）
    version: int = field(Int8ub, default=1)
    # RO 字段（build 时自动算，init=False）
    total_length: int = rfield(Computed(end - start))
    # WO 字段（parse 丢弃，build 取值，常用于 padding）
    reserved: bytes = wfield(Bytes(2))
```
## 4. 关键模式 Cookbook

> 本章是 SKILL 的核心：如何用 construct-rs 表达真实协议结构。每个模式都有
> 完整可运行代码，复制即可上手。

### 4.1 定义 Struct（含嵌套 + 对齐）

**做什么**：声明一个普通 Struct，含嵌套子 Struct 和对齐字段。

```python
from dataclasses import dataclass
from construct import (
    StructMixin, field, rfield, Int8ub, Int16ub, Int32ub, Bytes, Aligned,
)

@dataclass
class Inner(StructMixin):
    x: int = field(Int16ub)
    y: int = field(Int16ub)

@dataclass
class Packet(StructMixin):
    magic: int = field(Int32ub)
    # 嵌套子 Struct：直接把子类作为 subcon
    coord: Inner = field(Inner)
    # 对齐字段：4 字节边界对齐（不足补 0x00）
    flag: int = field(Aligned(4, Int8ub))
    tail: bytes = field(Bytes(2))

# build：对齐字段会自动补 padding
p = Packet(magic=1, coord=Inner(x=10, y=20), flag=1, tail=b"\xFF\xFF")
built = p.build()
# built = b"\x00\x00\x00\x01" (magic) + b"\x00\x0A\x00\x14" (coord)
#       + b"\x01" + b"\x00\x00\x00" (Aligned padding 到 4 字节边界)
#       + b"\xFF\xFF"

parsed = Packet.parse(built)
assert parsed == p
```

**关键点**：

- **嵌套 Struct**：把子类名直接作为 `field()` 的 subcon：`field(Inner)`
- **Aligned**：`Aligned(modulus, subcon)` 把字段补齐到 modulus 字节边界
- **字段顺序** = 字节顺序（PEP 526 注解插入顺序）

### 4.2 BitStruct（bitfield 位域分解）

**做什么**：把字节拆成 bit 级字段（如协议头的 Version/Flags 字段）。

```python
from dataclasses import dataclass
from construct import BitStructMixin, field, Bit, BitsInteger, Padding

@dataclass
class Header(BitStructMixin):    # 注意基类是 BitStructMixin，不是 StructMixin
    flag: int = field(Bit())              # 1 bit
    priority: int = field(BitsInteger(2)) # 2 bit
    reserved: int = field(BitsInteger(3)) # 3 bit
    value: int = field(BitsInteger(2))    # 2 bit = 总 8 bit = 1 字节

# parse：1 字节按 bit 拆开
h = Header.parse(b"\xAB")
# 0xAB = 0b10101011
# flag=1, priority=0b01=1, reserved=0b010=2, value=0b11=3

assert h.build() == b"\xAB"
```

**关键约束**：

- BitStruct 字段**总 bit 数必须是 8 的倍数**（否则 BitFieldError）
- 不足时用 `field(Padding(n))` 补齐（bit 域中 Padding 单位是 bit）
- bit 序默认**大端**（MSB first，与大多数网络协议一致）
- BitStruct 内部仍可嵌套完整子 Struct（通过 `Bytewise` 包装）

**真实示例：IPv4 Version+IHL（4+4 bit）**

```python
@dataclass
class VersionIHL(BitStructMixin):
    version: int = field(BitsInteger(4))   # 高 4 bit
    ihl: int = field(BitsInteger(4))       # 低 4 bit

# 字节 0x45 -> version=4, ihl=5
vi = VersionIHL.parse(b"\x45")
assert vi.version == 4 and vi.ihl == 5
```

**真实示例：IPv4 Flags+FragmentOffset（3+13 bit）**

```python
@dataclass
class FlagsFragment(BitStructMixin):
    reserved: int = field(Bit())               # 1 bit
    df: int = field(Bit())                     # Don't Fragment
    mf: int = field(Bit())                     # More Fragments
    fragment_offset: int = field(BitsInteger(13))  # 13 bit
    # 总 16 bit = 2 字节
```

### 4.3 条件分支（Switch / IfThenElse / Select / FocusedSeq）

**做什么**：根据字段值选择不同的子结构解析（如 Modbus 的功能码、IEC104 的帧类型）。

#### 4.3.1 Switch（最常用，多分支）

`Switch(keyfunc, cases, default=Pass)`：求值 keyfunc，在 cases dict 匹配。
**keyfunc 必须是简单字段引用或 int 常量**，复杂表达式请用 Computed 预计算。

```python
from construct import StructMixin, field, Int8ub, Int16ub, Switch

@dataclass
class TypeA(StructMixin):
    a: int = field(Int8ub)

@dataclass
class TypeB(StructMixin):
    b: int = field(Int16ub)

@dataclass
class Frame(StructMixin):
    type_code: int = field(Int8ub)
    # 根据 type_code 选择 payload 结构
    payload: object = field(Switch(type_code, {
        1: TypeA,    # type_code=1 -> TypeA
        2: TypeB,    # type_code=2 -> TypeB
    }))

# type_code=1 -> payload 是 TypeA
f1 = Frame.parse(b"\x01\xAA")
assert isinstance(f1.payload, TypeA) and f1.payload.a == 0xAA

# type_code=2 -> payload 是 TypeB
f2 = Frame.parse(b"\x02\x00\xBB")
assert isinstance(f2.payload, TypeB) and f2.payload.b == 0xBB
```

**关键点**：

- `Switch` 第一参数是**字段名引用**（不是 lambda、不是字符串）
- `cases` dict 的 key 是 int（协议字段值）
- `default=Pass`（默认）表示未命中时不解析（payload=None）
- 多个 type_code 共用同一结构：`{1: TypeA, 2: TypeA, 3: TypeB}`

#### 4.3.2 Switch + Computed（复杂 keyfunc 预计算）

**重要易错点**：Switch 的 keyfunc 不能直接是复杂表达式（如 `byte & 3`）。需要先用
`Computed` 预计算到一个新字段，再 Switch 该字段。

```python
from construct import StructMixin, field, rfield, Int8ub, Computed, Peek, Switch

@dataclass
class FormatI(StructMixin):
    raw: int = field(Int8ub)

@dataclass
class FormatS(StructMixin):
    raw: int = field(Int8ub)

@dataclass
class Frame(StructMixin):
    # Peek 预读 1 字节（不消费，必须 field()）
    first_byte: int = field(Peek(Int8ub))
    # Computed 预计算 frame_type（必须 rfield）
    # 公式：(byte & 1) * (1 + 2 * ((byte >> 1) & 1))
    #   byte bit0=0 -> 0 (TypeI)
    #   byte bit0=1, bit1=0 -> 1 (TypeS)
    #   byte bit0=1, bit1=1 -> 3 (TypeU)
    frame_type: int = rfield(Computed(
        (first_byte & 1) * (1 + 2 * ((first_byte >> 1) & 1))
    ))
    payload: object = field(Switch(frame_type, {
        0: FormatI,
        1: FormatS,
    }))
```

#### 4.3.3 IfThenElse（双分支）

```python
from construct import IfThenElse, Bytes

@dataclass
class P(StructMixin):
    x: int = field(Int8ub)
    # x > 0 时 v 是 1 字节，否则 2 字节
    v: int = field(IfThenElse(x > 0, Int8ub, Int16ub))
```

#### 4.3.4 If（单分支，条件不满足时 Pass）

`If(cond, subcon)` 等价于 `IfThenElse(cond, subcon, Pass)`：

```python
from construct import If

@dataclass
class P(StructMixin):
    x: int = field(Int8ub)
    v: int = field(If(x > 0, Int8ub))   # x=0 时 v=None（不写字节）
```

#### 4.3.5 Select（多尝试，首个成功者胜出）

```python
from construct import Select, Int32ub, CString

d = Select(Int32ub, CString("utf8"))
# 优先尝试 Int32ub，失败则 CString
```

#### 4.3.6 FocusedSeq（聚焦字段序列）

```python
from construct import FocusedSeq, Const, Terminated, Bytes

# parse 返回聚焦字段 "num" 的值（255），不是整个 dict
d = FocusedSeq("num",
    Const(b"SIG"),        # 匿名字段（校验 magic）
    "num" / Bytes(1),     # 命名字段（focus，用 "name" / subcon 语法）
    Terminated,           # 匿名字段（EOF 断言）
)
result = d.parse(b"SIG\xFF")   # -> b"\xFF"
```
### 4.4 CRC / 校验（Checksum 构造器 + StreamRange 模式）

**做什么**：自动校验和计算（CRC16/CRC32/SHA256 等），parse 时验证、build 时计算。

construct-rs 的 `Checksum` 有两条路径，**强烈推荐 StreamRange 模式**（路径 B）。

#### 4.4.1 StreamRange 模式（推荐：用 Tell 标记起止位置）

用 `rfield(Tell())` 标记 body 起止位置，Checksum 自动算 `[start, end)` 字节的 hash。

**完整可运行示例（CRC16 + Python callable + StreamRange）**：

```python
from dataclasses import dataclass
from construct import (
    StructMixin, field, rfield, Int8ub, Bytes, Tell, Checksum,
)

def crc16_modbus(data: bytes) -> bytes:
    """CRC-16/MODBUS（poly 0xA001, init 0xFFFF, low byte first）"""
    crc = 0xFFFF
    for byte in data:
        crc ^= byte
        for _ in range(8):
            crc = (crc >> 1) ^ 0xA001 if crc & 1 else crc >> 1
    return bytes([crc & 0xFF, (crc >> 8) & 0xFF])

@dataclass
class Frame(StructMixin):
    # 1. 标记 body 起始位置（必须 rfield）
    body_start: int = rfield(Tell())
    # 2. body 字段（被校验的部分）
    address: int = field(Int8ub)
    command: int = field(Int8ub)
    data: bytes = field(Bytes(4))
    # 3. 标记 body 结束位置（必须 rfield）
    body_end: int = rfield(Tell())
    # 4. Checksum：必须用 field()（不能用 rfield！），引用 start/end 字段
    crc: bytes = field(
        Checksum(Bytes(2), crc16_modbus, start=body_start, end=body_end)
    )

# build：自动算 [body_start, body_end) = address+command+data 的 CRC
f = Frame(address=1, command=3, data=b"\x00\x01\x00\x0A")
built = f.build()
# built = address + command + data + crc16(address+command+data)

# parse：自动验证 CRC（不等抛 ChecksumError）
parsed = Frame.parse(built)
assert parsed == f   # round-trip 通过

# 篡改字节 -> ChecksumError
from construct import ChecksumError
try:
    Frame.parse(built[:-1] + bytes([built[-1] ^ 0xFF]))
    raise AssertionError("should raise ChecksumError")
except ChecksumError:
    pass
```

**关键易错点（务必遵守）**：

1. **Checksum 必须用 `field()`**，不能用 `rfield()`（Checksum/Peek 不在 build_ro_value
   允许列表）。`field()` 模式下 build 时实例的 crc 值被忽略，由 StreamRange 重算
2. **`body_start` / `body_end` 必须用 `rfield(Tell())`**（Tell 是 RO 字段）
3. **字段顺序**：`body_start` 必须在 body 字段之前，`body_end` 必须在 body 字段之后
4. **Checksum 字段必须在 `body_end` 之后**（先算出 end，才能引用）

#### 4.4.2 HashAlgo 内置算法（零拷贝推荐）

对于标准 hash（MD5/SHA1/SHA256/SHA512/CRC32/ADLER32），用 `HashAlgo` enum 启用
Rust 内置 hashfunc 零拷贝路径：

```python
from construct import HashAlgo, Checksum, Bytes, rfield, Tell, field

@dataclass
class SignedPacket(StructMixin):
    start: int = rfield(Tell())
    data: bytes = field(Bytes(32))
    end: int = rfield(Tell())
    # HashAlgo.SHA256 -> Rust 内置 sha2 crate 计算，零拷贝
    sig: bytes = field(Checksum(Bytes(32), HashAlgo.SHA256, start, end))
```

**HashAlgo 与 Python callable 的区别**：

- `HashAlgo.SHA256` → Rust 内置 crate，零拷贝，**推荐**
- `lambda d: hashlib.sha256(d).digest()` → Python callable，FFI 回调，性能差

**CRC32/ADLER32 字节序**：Rust 端返回 4 字节 big-endian
（对齐 `zlib.crc32(d).to_bytes(4, 'big')`）。需要 little-endian 自行转换。

#### 4.4.3 ContextBytes 模式（不推荐：bytesfunc 引用字段）

`Checksum(Bytes(2), hashfunc, bytesfunc="field_name")` 引用同 Struct 的 bytes 字段。
**已知限制**：`bytesfunc` 不能引用 `RawCopy` 包装的字段（dict 类型会失败）。
**优先用 StreamRange 模式**（4.4.1）。

#### 4.4.4 中间位置的 checksum（API gap）

如果 checksum 字段不在帧末尾（如 IPv4 报文头 offset 10-11 的 header checksum），
`Tell(start, end)` 模式不适用（start/end 需先于 checksum 字段）。
**变通方案**：把 checksum 当作普通 `Bytes(2)`，外部 Python 函数校验：

```python
@dataclass
class IPv4Header(StructMixin):
    # ... 其他字段 ...
    checksum: bytes = field(Bytes(2))   # 当作普通字节
    # ... 其他字段 ...

# 外部校验函数
def verify_ipv4_checksum(header_bytes: bytes) -> bool:
    return compute_ipv4_checksum(header_bytes) == b"\x00\x00"
```

### 4.5 变长数组

**做什么**：根据字段值决定数组长度（如协议头的 length/count 字段）。

#### 4.5.1 Array(count_field, subcon)（count 引用字段）

```python
from construct import StructMixin, field, Int8ub, Int16ub, Array

@dataclass
class Packet(StructMixin):
    count: int = field(Int8ub)              # 先声明 count
    items: list = field(Array(count, Int16ub))  # 数组长度引用 count

p = Packet.parse(b"\x03\x00\x01\x00\x02\x00\x03")
assert p.count == 3
assert p.items == [1, 2, 3]

assert Packet(count=2, items=[10, 20]).build() == b"\x02\x00\x0A\x00\x14"
```

#### 4.5.2 Bytes(length_field)（变长字节串）

```python
@dataclass
class DataFrame(StructMixin):
    dlc: int = field(Int8ub)
    data: bytes = field(Bytes(dlc))   # 字节长度 = dlc 字段值
```

#### 4.5.3 PrefixedArray（前缀长度数组）

`PrefixedArray(countfield, subcon)`：先解析 countfield 得到元素计数，再循环 N 次。

```python
from construct import PrefixedArray, Byte, Int32ub

@dataclass
class P(StructMixin):
    items: list = field(PrefixedArray(Byte, Int32ub))
    # Byte 是元素计数（不是字节计数！）

P.parse(b"\x02\x00\x00\x00\x01\x00\x00\x00\x02")
# -> P(items=[1, 2])
```

**PrefixedArray vs Prefixed**：

- `PrefixedArray(Byte, Item)`：Byte 是**元素计数**，循环 N 次 Item.parse
- `Prefixed(Byte, Item)`：Byte 是**字节计数**，读 N 字节作为子流

#### 4.5.4 GreedyRange（读到 EOF）

```python
from construct import GreedyRange

@dataclass
class Packet(StructMixin):
    magic: int = field(Int8ub)
    payload: list = field(GreedyRange(Int8ub))   # 读到 EOF
```

#### 4.5.5 Prefixed 详解（includelength 两种模式）

**做什么**：用 length 字段定界 subcon 的子流。两种模式仅在"length 值是否含 lengthfield 自身字节"上有差异。

**模式对比**：

| 模式 | length 字段值 | parse 时 substream 长度 | 典型协议 |
|------|--------------|----------------------|---------|
| `includelength=False`（默认） | subcon 字节数 | = length 值 | 自定义 length field（仅计数 payload） |
| `includelength=True` | subcon 字节数 + sizeof(lengthfield) | = length 值 - sizeof(lengthfield) | SCTP chunk_length / 部分 TLS record（"含自身"语义） |

**核心区别**：`includelength=True` 时 length 字段的值**包含 lengthfield 自身大小**。注意是 `sizeof(lengthfield)`（如 `Int16ub`=2 字节、`Int32ub`=4 字节、`VarInt` 取实际编码长度），**不是固定 4 字节**。

**完整对照示例**（同一段 payload，两种模式 build 出的字节差异）：

```python
from dataclasses import dataclass
from construct import StructMixin, field, Prefixed, Int16ub, GreedyBytes

payload = b"\xAA\xBB\xCC"

# ─── 模式 A：includelength=False（默认）───
# length 字段值 = subcon 字节数 = 3
@dataclass
class ChunkA(StructMixin):
    data: bytes = field(Prefixed(Int16ub, GreedyBytes))

a = ChunkA(data=payload)
built_a = a.build()
# built_a = b"\x00\x03" (Int16ub=3) + b"\xAA\xBB\xCC"
assert built_a == b"\x00\x03\xAA\xBB\xCC"
assert ChunkA.parse(built_a).data == payload

# ─── 模式 B：includelength=True ───
# length 字段值 = subcon 字节数 + sizeof(Int16ub) = 3 + 2 = 5
@dataclass
class ChunkB(StructMixin):
    data: bytes = field(Prefixed(Int16ub, GreedyBytes, includelength=True))

b = ChunkB(data=payload)
built_b = b.build()
# built_b = b"\x00\x05" (Int16ub=5) + b"\xAA\xBB\xCC"
assert built_b == b"\x00\x05\xAA\xBB\xCC"
assert ChunkB.parse(built_b).data == payload
```

**字节差异**：两个 build 输出**长度相同**（都是 5 字节），但 Int16ub 的值不同（3 vs 5）——这是两种模式唯一的本质区别。

**何时用哪种**（按协议 spec 描述判断）：

- spec 说 "length 表示 X 的字节数"（不含 length 自身）→ `includelength=False`
- spec 说 "length 表示整个 chunk/record 的字节数"（含 length 自身 + 头部）→ `includelength=True`

**真实协议对照**：

| 协议 | length 字段 | spec 描述 | 选择 |
|------|-----------|----------|------|
| SCTP | chunk header offset 2-3（Int16ub） | "Chunk Length: ... including the Chunk Value ... and the Chunk Header"（含 header 4B） | `includelength=True` |
| PNG chunk | length offset 0-3（Int32ub） | "Length: ... data field (not including length/type/CRC)" | `includelength=False` |
| HTTP/2 frame | frame header length（24-bit） | "Length: ... frame payload (not including header)" | `includelength=False` |
| TLS 1.2 record | record header length（Int16ub） | "length of the following fields: fragment (no header)" | `includelength=False` |

**易错点**：SCTP 是少数 length 含自身的协议（含 lengthfield + type + flags + value）。见到协议 spec 写 "length includes header" 务必用 `includelength=True`，否则 substream 会**多读 sizeof(lengthfield) 字节**导致 wire 格式错误。

#### 4.5.6 长度前缀变长记录序列（GreedyRange + Prefixed）

**做什么**：解析"重复的 [length + record]"结构直到 EOF。这是 **SCTP chunks / TLS records / PNG chunks / HTTP/2 frames** 的通用模式。

**组合模式**：

```python
GreedyRange(Prefixed(length_field, RecordStruct, includelength=...))
```

**工作原理**：

1. GreedyRange 反复执行内部 subcon，直到 EOF 或 subcon 失败
2. 每次 Prefixed 读 length_field → 按 includelength 语义创建子流 → 在子流中 parse RecordStruct
3. 子流自动隔离每个 record：即便 RecordStruct 含变长字段（如 GreedyBytes），也不影响下一个 record 的边界

**完整示例（简化版 SCTP chunk 序列）**：

```python
from dataclasses import dataclass
from construct import (
    StructMixin, field, Int8ub, Int16ub, Int32ub, GreedyBytes,
    Prefixed, GreedyRange, Switch,
)

@dataclass
class DataChunkValue(StructMixin):
    """DATA chunk value（简化）：TSN + 变长 user_data"""
    tsn: int = field(Int32ub)
    user_data: bytes = field(GreedyBytes)   # 读到子流末尾

@dataclass
class AbortChunkValue(StructMixin):
    """ABORT chunk value：可选错误原因（变长 raw bytes）"""
    error_causes: bytes = field(GreedyBytes)

@dataclass
class ChunkInner(StructMixin):
    """单个 chunk：在 Prefixed substream 中（length 已被 Prefixed 消费）"""
    chunk_type: int = field(Int8ub)
    flags: int = field(Int8ub)
    value: object = field(Switch(chunk_type, {
        0: DataChunkValue,     # DATA
        6: AbortChunkValue,    # ABORT
    }))

@dataclass
class SCTPChunks(StructMixin):
    """SCTP chunk 序列（此处省略 Common Header，仅演示 chunk 序列模式）"""
    chunks: list = field(GreedyRange(
        Prefixed(Int16ub, ChunkInner, includelength=True)
    ))

# ─── 构造测试数据：2 个 chunk（DATA + ABORT）───
packet = SCTPChunks(chunks=[
    ChunkInner(chunk_type=0, flags=0x03,
               value=DataChunkValue(tsn=1, user_data=b"hello")),
    ChunkInner(chunk_type=6, flags=0x00,
               value=AbortChunkValue(error_causes=b"")),
])
built = packet.build()

# chunk1 = type(1) + flags(1) + value(4 + 5) = 11 字节，含 length(2) 自身 -> length=13
# chunk2 = type(1) + flags(1) + value(0)      = 2 字节， 含 length(2) 自身 -> length=4
assert built == (
    b"\x00\x0D"            # chunk1 length=13
    b"\x00\x03"            # chunk1 type=0, flags=0x03
    b"\x00\x00\x00\x01"  # tsn=1
    b"hello"                 # user_data
    b"\x00\x04"            # chunk2 length=4
    b"\x06\x00"            # chunk2 type=6, flags=0x00
)

# round-trip
parsed = SCTPChunks.parse(built)
assert len(parsed.chunks) == 2
assert isinstance(parsed.chunks[0].value, DataChunkValue)
assert parsed.chunks[0].value.tsn == 1
assert parsed.chunks[0].value.user_data == b"hello"
assert isinstance(parsed.chunks[1].value, AbortChunkValue)
assert parsed.build() == built
```

**关键技巧**：

- **GreedyBytes 读子流剩余字节**：`DataChunkValue.user_data = GreedyBytes` 自动读到 Prefixed substream 末尾，等价于 `chunk_length - 固定字段字节数`，**无需跨层引用 chunk_length**
- **Switch 在子流内分派**：不同 chunk_type 用不同 Struct，每个 Struct 末尾用 GreedyBytes 兜底消费子流剩余字节
- **chunk_length 含自身**：用 `includelength=True` 让 substream 长度 = length_value - sizeof(lengthfield) = chunk_length - 2

**为何不用 PrefixedArray？**

`PrefixedArray(countfield, subcon)` 的 countfield 是**元素计数**（如"3 个 chunk"），适用于"开头说总共有 N 个 record"。而 `GreedyRange + Prefixed` 处理的是"每个 record 自带 length"——两者完全不同。

**应用场景**：

| 协议 | record 结构 | length field | includelength |
|------|-----------|-------------|---------------|
| SCTP | type + flags + value | Int16ub | True（含 type+flags+length） |
| TLS 1.2 record | content_type + version + fragment | Int16ub | False（仅 fragment） |
| PNG chunk | length + type + data + CRC | Int32ub | False（仅 data） |
| HTTP/2 frame | length + type + flags + stream_id + payload | 24-bit | False（仅 payload） |

### 4.6 字符串

```python
from construct import CString, GreedyString, PaddedString, PascalString

@dataclass
class StringPacket(StructMixin):
    # C 风格 null 终止
    name: str = field(CString("utf8"))
    # 读到 EOF 解码
    description: str = field(GreedyString("utf8"))

# parse: name="hello", description="world"
p = StringPacket.parse(b"hello\x00world")
```

**编码注意**：必须用显式后缀（`utf8`/`utf_16_le`/...），**不接受** `utf16`/`utf32`。
### 4.7 嵌套 Struct + 跨层引用（重要 API gap）

**做什么**：在父 Struct 中嵌入子 Struct（包括 BitStruct），并访问子 Struct 的字段。

**已知限制**：`Computed` / `Switch` 的表达式**不能跨层引用子 BitStruct/Struct 字段**。
表达式只能引用同 Struct 内的兄弟字段（siblings）。

**错误示例**（编译失败）：

```python
@dataclass
class CANID(BitStructMixin):
    eff: int = field(Bit())
    std_id: int = field(BitsInteger(11))

@dataclass
class CANFrame(StructMixin):
    can_id: CANID = field(CANID)
    # 错误：不能跨层引用 can_id.eff
    is_extended: int = rfield(Computed(can_id.eff))   # CompilationError!
```

**变通方案**：跨层计算在 Python 层（parse 后做）：

```python
@dataclass
class CANFrame(StructMixin):
    can_id: CANID = field(CANID)
    dlc: int = field(Int8ub)
    data: bytes = field(Bytes(dlc))

# 解析后用 Python 属性访问做跨层运算
f = CANFrame.parse(data)
if f.can_id.eff:
    identifier = (f.can_id.ext_id_low << 11) | f.can_id.std_id
else:
    identifier = f.can_id.std_id
```

**正确做法**：表达式只引用**同层兄弟字段**。如果需要"先解析一个值，再用它做条件"，
就在同层用 `Peek` + `Computed` 预计算（见 4.3.2）。

### 4.8 Enum / Mapping / FlagsEnum

#### 4.8.1 Enum（整数 <-> label 字符串）

```python
from construct import Enum, Int8ub

e = Enum(Int8ub, READ=1, WRITE=2, EXEC=4)
e.parse(b"\x01")   # -> "READ"（EnumIntegerString，int(obj)==1）
e.parse(b"\xff")   # -> 255（EnumInteger fallback，不报错）
e.build("READ")    # -> b"\x01"
e.build(99)        # -> b"\x63"（int 直接用）
```

嵌入 Struct：

```python
@dataclass
class P(StructMixin):
    op: str = field(Enum(Int8ub, READ=1, WRITE=2))
```

#### 4.8.2 FlagsEnum（整数 -> flags dict）

```python
from construct import FlagsEnum, Int8ub

fe = FlagsEnum(Int8ub, READ=1, WRITE=2, EXEC=4)
fe.parse(b"\x03")
# -> dict(_flagsenum=True, READ=True, WRITE=True, EXEC=False)
fe.build(dict(READ=True, WRITE=True))   # -> b"\x03"
```

#### 4.8.3 Mapping（通用对象映射，无映射时报错）

```python
from construct import Mapping, Int8ub

x = object()
m = Mapping(Int8ub, {0: x, 1: None})
m.parse(b"\x00")   # -> x
m.parse(b"\xff")   # -> MappingError（与 Enum 不同，无 fallback）
```

### 4.9 表达式进阶（位运算重组 + 条件求值）

**真实示例：IEC 104 序列号重组（15-bit 跨字节）**

```python
@dataclass
class IFormatControl(StructMixin):
    cf1: int = field(Int8ub)
    cf2: int = field(Int8ub)
    cf3: int = field(Int8ub)
    cf4: int = field(Int8ub)
    # send_seq = 高 8 位 (cf2) << 7 | 低 7 位 (cf1 >> 1)
    send_seq: int = rfield(Computed((cf2 << 7) | (cf1 >> 1)))
    recv_seq: int = rfield(Computed((cf4 << 7) | (cf3 >> 1)))
```

**关键点**：

- 表达式 `cf2 << 7 | cf1 >> 1` 直接使用字段名（cf1/cf2/cf3/cf4）
- 位运算 + 算术可任意组合，编译为 Rust VM 指令
- Computed 字段必须用 `rfield`，且表达式引用的字段必须**前序声明**

### 4.10 StopIf（早停）

```python
from construct import StopIf, GreedyRange

@dataclass
class Item(StructMixin):
    x: int = field(Int8ub)
    # x == 0xFF 时停止 GreedyRange 迭代
    stop = rfield(StopIf(x == 0xFF))

# 用法：把 Item 嵌入 GreedyRange
```

### 4.11 Rebuild / Default（build 时自动填充）

```python
from construct import Rebuild, Default

@dataclass
class P(StructMixin):
    x: int = field(Int8ub)
    # y = x + 1（必须 rfield，build 时自动算）
    y: int = rfield(Rebuild(Int8ub, x + 1))
    # z 默认 = 0（field()，build 时若 z=None 则用默认值）
    z: int = field(Default(Int8ub, 0))
```

## 5. 易错点 + API gap 替代方案

> 本章是 Phase 9 系统测试实战发现的所有易错点。**照着做能避免 90% 的坑**。

### 5.1 [必读] Checksum 必须用 field()，不能用 rfield()

**症状**：`rfield(Checksum(...))` 在 build 时报错：
`compute_ro_value: node type Checksum(...) is not a valid RO node`

**原因**：Rust 端 build_ro_value 允许列表不含 ChecksumNode。

**正确做法**：用 `field()`（RW 模式）。build 时实例的 crc 值被忽略，由 StreamRange 重算。

```python
# 错误
crc: bytes = rfield(Checksum(Bytes(2), fn, start, end))
# 正确
crc: bytes = field(Checksum(Bytes(2), fn, start, end))
```

### 5.2 [必读] Peek 必须用 field()，不能用 rfield()

同 5.1，PeekNode 也不在 build_ro_value 允许列表。

```python
# 错误
peek: int = rfield(Peek(Int8ub))
# 正确
peek: int = field(Peek(Int8ub))
```

### 5.3 [必读] Checksum + RawCopy dict 不工作 -> 用 StreamRange

**症状**：`Checksum(Bytes(2), fn, bytesfunc="body")` 引用 RawCopy 字段时报
`ChecksumError: bytes field 'body' is not bytes-like`（因为 RawCopy 返回 dict，
不是 bytes）。

**替代方案**：用 StreamRange 模式（`Tell start` + `Tell end` + Checksum(start, end)）。
详见 4.4.1。

### 5.4 [必读] Computed/Switch 表达式不能跨层引用子 BitStruct 字段

**症状**：`Computed(can_id.eff)` 编译失败（`can_id` 是子 BitStruct）。

**替代方案**：跨层计算在 Python 层做（parse 后用属性访问）。详见 4.7。

### 5.5 [必读] Switch keyfunc 必须是简单字段引用或 int

**症状**：`Switch(byte & 3, {...})` 复杂表达式编译可能通过但**语义错误**
（如 IEC 104 cf1 & 3 因 send_seq 位污染）。

**替代方案**：先用 Computed 预计算到一个新字段，再 Switch 该字段名：

```python
frame_type: int = rfield(Computed((cf1_peek & 1) * (1 + 2 * ((cf1_peek >> 1) & 1))))
payload: object = field(Switch(frame_type, {0: I, 1: S, 3: U}))
```

### 5.6 Const 参数顺序：(value, subcon) 非 (subcon, value)

**正确**：`Const(0x01, Int8ub)`（value 在前）
**错误**：`Const(Int8ub, 0x01)`

bytes value 时 subcon 可省略（自动推断 `Bytes(len(value))`）：

```python
Const(b"\x68")           # 等价 Const(b"\x68", Bytes(1))
Const(b"IHDR")           # 等价 Const(b"IHDR", Bytes(4))
Const(0x01, Int8ub)      # int value 必须显式 subcon
```

### 5.7 BitStruct 字段总 bit 数必须 8 的倍数

**症状**：BitStruct 总 bit 数不是 8 的倍数时 parse/build 抛 BitFieldError。

**解决**：用 `Padding(n)` 补齐（bit 域中 Padding 单位是 bit）：

```python
@dataclass
class Bits(BitStructMixin):
    flag: int = field(Bit())              # 1 bit
    value: int = field(BitsInteger(10))   # 10 bit
    reserved: int = field(BitsInteger(5)) # 5 bit = 总 16 bit = 2 字节 ✓
```

### 5.8 表达式不支持 / 浮点和 and/or

- `/` 不支持（结果可能浮点，VM 栈是 i64）。用 `//`（整除）。
- `and` / `or` 不支持（Python 短路求值无法重载）。用 `&` / `|`（位运算）。
- 浮点常量不支持（如 `Computed(0.5 * x)`）。所有常量必须 int。

### 5.9 字段引用必须前序声明

表达式只能引用**当前字段之前**声明的字段（不能前向引用）：

```python
@dataclass
class P(StructMixin):
    a: int = field(Int8ub)
    b: int = rfield(Computed(a + 1))   # ✓ a 在 b 之前
    # c: int = field(Computed(b + 1))  # 引用 b 没问题（b 在 c 之前）
```

前向引用（c 引用后面声明的字段）会触发延迟编译机制，但仅对**类型引用**（嵌套子类）
有效，**表达式引用前序字段是硬约束**。

### 5.10 编码必须用显式后缀

- `utf8` / `utf_8` / `u8` ✓
- `utf_16_le` / `utf_16_be` ✓
- `utf_32_le` / `utf_32_be` ✓
- `ascii` ✓
- `utf16` / `utf_32` / `u16` ✗（无后缀形式被拒绝）

### 5.11 BitStruct 适合静态位域；动态位域请保留 raw byte + 事后解析

**症状**：同一字节在不同上下文（如不同 chunk_type）下含义不同。例如 SCTP DATA chunk flags 是 U/B/E（3 个功能位），ABORT chunk flags 是 T bit + 7 reserved。无法让一个 BitStruct 字段同时满足两种解读。

**原因**：BitStruct 字段类型在编译期固定，运行时无法根据上下文（如 chunk_type）动态切换 BitStruct 类型。

**替代方案**：在 Struct 字段层用 raw `Int8ub` 保留原字节，在 Python 层根据上下文事后用相应 BitStruct.parse() 解码：

```python
@dataclass
class DataChunkFlags(BitStructMixin):
    reserved: int = field(BitsInteger(5))
    u: int = field(Bit())
    b: int = field(Bit())
    e: int = field(Bit())

@dataclass
class ChunkInner(StructMixin):
    chunk_type: int = field(Int8ub)
    flags_raw: int = field(Int8ub)     # 保留 raw byte（动态 bitfield）
    # ...

# 事后解码（根据 chunk_type 选择对应 BitStruct）
parsed = ChunkInner.parse(...)
if parsed.chunk_type == 0:   # DATA
    flags = DataChunkFlags.parse(bytes([parsed.flags_raw]))
    # 用 flags.u / flags.b / flags.e
elif parsed.chunk_type == 6: # ABORT
    # 用其他 BitStruct 解码同一字节
    ...
```

**关键认知**：BitStruct 适合"位含义与上下文无关"的静态分解（如 IPv4 version/IHL、CAN ID flags）；动态位域（位含义依赖上下文）必须 raw byte + 事后 BitStruct.parse，不要试图用 Switch 动态绑定 BitStruct 类型。

### 5.12 Switch default=Pass 在 Prefixed 子流中可能留下未消费字节

**症状**：`Switch(key, cases)` 默认 `default=Pass`（key 未命中时不解析）。当 Switch 嵌在 `Prefixed` 子流内、且 key 未命中时，子流剩余字节未被消费。

**后果**：

- 若 Prefixed 内的子构造器**必须消费完整个子流**（如末尾有 GreedyBytes 兜底），Pass 不消费字节会让后续字段读到错误位置
- 若 Prefixed 是 GreedyRange 内的元素，子流未消费可能让 GreedyRange 异常终止（迭代失败 → 列表提前结束或为空）

**替代方案**：

- 若希望未知 key 的 record 仍能保留 raw bytes 供事后分析：用 `default=GreedyBytes` 而非 `default=Pass`，让 default 分支消费剩余子流
- 若 key 未命中即视为协议错误：保持 `default=Pass`，但确保 Switch 是子流中**最后一个字段**（剩余字节不影响后续 parse）

```python
# 推荐：default=GreedyBytes 兜底未知 chunk_type
value: object = field(Switch(chunk_type, {
    0: DataChunkValue,
    6: AbortChunkValue,
}, default=GreedyBytes))   # 未知 type -> value 字段返回 raw bytes，并消费子流
```

## 6. 完整示例（递进复杂度）

> 本章用 4 个真实工业/网络协议演示如何组合使用上述模式。
> **注意**：这些协议是 SKILL 的样例，验证时不能用这些协议。

### 6.1 简单：定长 Struct（基础）

**协议**：3 字节定长 header（magic + version + type）

```python
from dataclasses import dataclass
from construct import StructMixin, field, Int8ub, Int16ub

@dataclass
class Header(StructMixin):
    magic: int = field(Int16ub)
    version: int = field(Int8ub)
    msg_type: int = field(Int8ub)

# build / parse round-trip
h = Header(magic=0xCAFE, version=1, msg_type=2)
assert Header.parse(h.build()) == h
```

### 6.2 BitStruct：IPv4 报文头（bitfield + 多个子结构）

**协议**：RFC 791 IPv4 报文头（最小 20 字节，含 3 个 BitStruct 子字段）

```python
from dataclasses import dataclass
from construct import (
    StructMixin, BitStructMixin, field, BitsInteger, Bit,
    Bytes, Int8ub, Int16ub, Int32ub,
)

@dataclass
class VersionIHL(BitStructMixin):
    """4 bit version + 4 bit IHL = 1 字节"""
    version: int = field(BitsInteger(4))
    ihl: int = field(BitsInteger(4))

@dataclass
class DSCPECN(BitStructMixin):
    """6 bit DSCP + 2 bit ECN = 1 字节"""
    dscp: int = field(BitsInteger(6))
    ecn: int = field(BitsInteger(2))

@dataclass
class FlagsFragment(BitStructMixin):
    """3 bit flags + 13 bit fragment offset = 2 字节"""
    reserved: int = field(Bit())
    df: int = field(Bit())    # Don't Fragment
    mf: int = field(Bit())    # More Fragments
    fragment_offset: int = field(BitsInteger(13))

@dataclass
class IPv4Header(StructMixin):
    version_ihl: VersionIHL = field(VersionIHL)
    dscp_ecn: DSCPECN = field(DSCPECN)
    total_length: int = field(Int16ub)
    identification: int = field(Int16ub)
    flags_fragment: FlagsFragment = field(FlagsFragment)
    ttl: int = field(Int8ub)
    protocol: int = field(Int8ub)
    # 注意：IPv4 header checksum 在协议中间（offset 10-11），不能用 Tell+Checksum 模式
    # 用普通 Bytes(2)，外部 Python 函数校验
    checksum: bytes = field(Bytes(2))
    source_addr: int = field(Int32ub)
    dest_addr: int = field(Int32ub)

# 构造一个 minimal IPv4 头
import struct
# Version=4, IHL=5, DSCP=0, ECN=0, total_length=20, id=0, flags=0, frag=0,
# ttl=64, protocol=6, checksum=0, src=10.0.0.1, dst=10.0.0.2
raw = bytes([0x45, 0x00]) + struct.pack(">HH", 20, 0) + b"\x00\x00"
raw += bytes([64, 6, 0, 0]) + struct.pack(">II", 0x0A000001, 0x0A000002)

parsed = IPv4Header.parse(raw)
assert parsed.version_ihl.version == 4
assert parsed.version_ihl.ihl == 5
assert parsed.ttl == 64
assert parsed.protocol == 6
# round-trip
assert parsed.build() == raw
```

### 6.3 条件分支 + CRC：Modbus RTU 帧

**协议**：Address(1B) + FC(1B) + Data(变长) + CRC16(2B LE)

- **Switch** on function_code（不同 FC 不同 Data 结构）
- **Checksum + StreamRange** 自动 CRC16 校验/计算
- **Array(count_field, subcon)** FC=0x10 用数组表示寄存器列表

```python
from dataclasses import dataclass
from construct import (
    StructMixin, field, rfield, Int8ub, Int16ub, Bytes,
    Array, Switch, Tell, Checksum,
)

def crc16_modbus(data: bytes) -> bytes:
    """CRC-16/MODBUS（poly 0xA001 bit-reversed, init 0xFFFF, LE）"""
    crc = 0xFFFF
    for byte in data:
        crc ^= byte
        for _ in range(8):
            crc = (crc >> 1) ^ 0xA001 if crc & 1 else crc >> 1
    return bytes([crc & 0xFF, (crc >> 8) & 0xFF])

# FC=0x03 Read Holding Registers Request
@dataclass
class ReadHoldingReq(StructMixin):
    starting_address: int = field(Int16ub)
    quantity: int = field(Int16ub)

# FC=0x06 Write Single Register Request
@dataclass
class WriteSingleReq(StructMixin):
    register_address: int = field(Int16ub)
    register_value: int = field(Int16ub)

# FC=0x10 Write Multiple Registers Request
@dataclass
class WriteMultiReq(StructMixin):
    starting_address: int = field(Int16ub)
    quantity: int = field(Int16ub)
    byte_count: int = field(Int8ub)
    # 数组长度引用 quantity 字段（FieldRef count）
    values: list = field(Array(quantity, Int16ub))

@dataclass
class ModbusRTUFrame(StructMixin):
    # 1. 标记 body 起始（必须 rfield）
    body_start: int = rfield(Tell())
    address: int = field(Int8ub)
    function_code: int = field(Int8ub)
    # 2. Switch on function_code 选择 payload 结构
    payload: object = field(Switch(function_code, {
        0x01: ReadHoldingReq,
        0x03: ReadHoldingReq,
        0x06: WriteSingleReq,
        0x10: WriteMultiReq,
    }))
    body_end: int = rfield(Tell())
    # 3. Checksum 必须用 field()（不能 rfield），引用 start/end
    crc: bytes = field(
        Checksum(Bytes(2), crc16_modbus, start=body_start, end=body_end)
    )

# 真实 Modbus spec 示例：slave=0x11, FC=0x03, addr=0x006B, qty=0x0003
# CRC = 0x7687（low byte 0x76 first）
spec_data = bytes([0x11, 0x03, 0x00, 0x6B, 0x00, 0x03, 0x76, 0x87])

parsed = ModbusRTUFrame.parse(spec_data)
assert parsed.address == 0x11
assert parsed.function_code == 0x03
assert isinstance(parsed.payload, ReadHoldingReq)
assert parsed.payload.starting_address == 0x006B
assert parsed.payload.quantity == 0x0003
assert parsed.crc == bytes([0x76, 0x87])

# round-trip
assert parsed.build() == spec_data

# 篡改 CRC -> ChecksumError
from construct import ChecksumError
tampered = spec_data[:-1] + bytes([spec_data[-1] ^ 0xFF])
try:
    ModbusRTUFrame.parse(tampered)
    raise AssertionError("expected ChecksumError")
except ChecksumError:
    pass
```
### 6.4 综合：IEC 60870-5-104（Peek + Computed + Switch + Const）

**协议**：Start(0x68) + Length(1B) + Control Field(4B)

帧类型按 CF1 的 bit 0/bit 1 区分：
- I-format（bit0=0）：信息传输，含 15-bit send_seq（跨字节）
- S-format（bit0=1, bit1=0）：编号监督
- U-format（bit0=1, bit1=1）：未编号控制

**关键挑战**：用 `Peek` 预读 CF1 不消费字节，`Computed` 算 frame_type，`Switch` 分派。

```python
from dataclasses import dataclass
from construct import (
    StructMixin, field, rfield, Int8ub, Bytes, Const, Computed, Peek, Switch,
)

# I-format：4 字节控制域 + 序列号重组（15-bit 跨字节）
@dataclass
class IFormatControl(StructMixin):
    cf1: int = field(Int8ub)
    cf2: int = field(Int8ub)
    cf3: int = field(Int8ub)
    cf4: int = field(Int8ub)
    # send_seq = 高 8 位 << 7 | 低 7 位
    send_seq: int = rfield(Computed((cf2 << 7) | (cf1 >> 1)))
    recv_seq: int = rfield(Computed((cf4 << 7) | (cf3 >> 1)))

# S-format：CF1=0x01 固定
@dataclass
class SFormatControl(StructMixin):
    cf1: int = field(Const(0x01, Int8ub))   # 注意 Const 顺序：(value, subcon)
    cf2: int = field(Const(0x00, Int8ub))
    cf3: int = field(Int8ub)
    cf4: int = field(Int8ub)
    recv_seq: int = rfield(Computed((cf4 << 7) | (cf3 >> 1)))

# U-format：CF2-CF4 全 0
@dataclass
class UFormatControl(StructMixin):
    cf1: int = field(Int8ub)
    cf2: int = field(Const(0x00, Int8ub))
    cf3: int = field(Const(0x00, Int8ub))
    cf4: int = field(Const(0x00, Int8ub))

@dataclass
class APCI(StructMixin):
    # Const 校验起始字节（bytes value 自动推断 Bytes(1)）
    start: bytes = field(Const(b"\x68"))
    apdu_length: int = field(Int8ub)
    # Peek 预读 CF1（必须 field()，不消费字节）
    cf1_peek: int = field(Peek(Int8ub))
    # Computed 预计算 frame_type（必须 rfield）
    # 公式：(bit0) * (1 + 2 * (bit1))
    #   bit0=0 -> 0 (I)
    #   bit0=1, bit1=0 -> 1 (S)
    #   bit0=1, bit1=1 -> 3 (U)
    frame_type: int = rfield(Computed(
        (cf1_peek & 1) * (1 + 2 * ((cf1_peek >> 1) & 1))
    ))
    # Switch 分派（注意：keyfunc 是 frame_type 字段名引用，不是表达式）
    control_field: object = field(Switch(frame_type, {
        0: IFormatControl,
        1: SFormatControl,
        3: UFormatControl,
    }))

# 测试 STARTDT act U-format：start=0x68, len=4, CF=07 00 00 00
u_data = b"\x68\x04\x07\x00\x00\x00"
parsed = APCI.parse(u_data)
assert parsed.start == b"\x68"
assert parsed.apdu_length == 4
assert parsed.frame_type == 3
assert isinstance(parsed.control_field, UFormatControl)
assert parsed.control_field.cf1 == 0x07
assert parsed.build() == u_data   # round-trip

# 测试 S-format：start=0x68, len=4, CF=01 00 00 00
s_data = b"\x68\x04\x01\x00\x00\x00"
parsed = APCI.parse(s_data)
assert parsed.frame_type == 1
assert isinstance(parsed.control_field, SFormatControl)

# 测试 I-format：start=0x68, len=4, CF=02 00 04 00（send_seq=1, recv_seq=2）
i_data = b"\x68\x04\x02\x00\x04\x00"
parsed = APCI.parse(i_data)
assert parsed.frame_type == 0
assert isinstance(parsed.control_field, IFormatControl)
assert parsed.control_field.send_seq == 1   # (0x00 << 7) | (0x02 >> 1) = 1
assert parsed.control_field.recv_seq == 2   # (0x00 << 7) | (0x04 >> 1) = 2
```

### 6.5 BitStruct + 变长：CAN 2.0 帧

**协议**：can_id(4B BitStruct) + DLC(1B) + data(0-8B 变长)

```python
from dataclasses import dataclass
from construct import (
    StructMixin, BitStructMixin, field, Bit, BitsInteger, Bytes, Int8ub,
)

@dataclass
class CANIdentifier(BitStructMixin):
    """32-bit CAN ID + 标志（大端 bit 序，与 SocketCAN can_id_t 一致）
    总 bit 数：1+1+1+18+11 = 32 = 4 字节（无须 padding）
    """
    err: int = field(Bit())               # bit 31
    rtr: int = field(Bit())               # bit 30
    eff: int = field(Bit())               # bit 29（1=扩展帧 29-bit ID）
    ext_id_low: int = field(BitsInteger(18))  # bits 28-11（扩展帧 ID 高位）
    std_id: int = field(BitsInteger(11))      # bits 10-0（标准 ID 或扩展 ID 低位）

@dataclass
class CANFrame(StructMixin):
    can_id: CANIdentifier = field(CANIdentifier)
    dlc: int = field(Int8ub)
    # 变长 data：Bytes(dlc) 引用 DLC 字段
    data: bytes = field(Bytes(dlc))

# 标准帧：ID=0x123, DLC=0, 无数据
# can_id raw = 0x00000123（big-endian: 00 00 01 23）
std_data = bytes([0x00, 0x00, 0x01, 0x23, 0x00])
parsed = CANFrame.parse(std_data)
assert parsed.can_id.eff == 0
assert parsed.can_id.std_id == 0x123
assert parsed.dlc == 0
assert parsed.data == b""
assert parsed.build() == std_data

# 标识符重建（标准 vs 扩展）在 Python 层做（Computed 不能跨层引用子 BitStruct 字段）
def decode_identifier(can_id: CANIdentifier) -> int:
    if can_id.eff:
        return (can_id.ext_id_low << 11) | can_id.std_id
    return can_id.std_id

assert decode_identifier(parsed.can_id) == 0x123

# 扩展帧：ID=0x1234567 (29-bit)
# eff=1, ext_id_low = (0x1234567 >> 11) & 0x3FFFF = 0x2468
# std_id = 0x1234567 & 0x7FF = 0x567
# can_id raw = (1 << 29) | (0x2468 << 11) | 0x567 = 0x20012345（不对，重新算）
# 用 encode_can_id 工具函数构造字节：
def encode_can_id(identifier: int, eff: bool) -> int:
    if eff:
        std_part = identifier & 0x7FF
        ext_part = (identifier >> 11) & 0x3FFFF
    else:
        std_part = identifier
        ext_part = 0
    return ((1 if eff else 0) << 29) | (ext_part << 11) | std_part

raw_id = encode_can_id(0x1234567, eff=True)
ext_bytes = bytes([(raw_id >> 24) & 0xFF, (raw_id >> 16) & 0xFF,
                   (raw_id >> 8) & 0xFF, raw_id & 0xFF])
ext_data = ext_bytes + bytes([8]) + bytes([0xAA] * 8)
parsed = CANFrame.parse(ext_data)
assert parsed.can_id.eff == 1
assert decode_identifier(parsed.can_id) == 0x1234567
```

### 6.6 综合：SCTP（GreedyRange + Prefixed + Switch + BitStruct + 中间位置 checksum）

**协议**：RFC 4960 SCTP —— Common Header(12B) + 一个或多个 chunk，每个 chunk = length-prefixed (type + flags + value)。

**关键挑战**（集成多个 SKILL 模式）：

1. **chunk 序列**：`GreedyRange(Prefixed(Int16ub, ChunkInner, includelength=True))`（详见 §4.5.6）
2. **DATA chunk flags bitfield**：5 reserved + U + B + E = 8 bit（详见 §4.2）
3. **chunk_type 分派**：Switch 分派不同 chunk value 结构（详见 §4.3.1）
4. **checksum 在 Common Header 中间**（offset 8-11）：不能用 Checksum 构造器（详见 §4.4.4），用 Bytes(4) + 外部 CRC32c 函数
5. **动态 bitfield**：flags 字段含义依赖 chunk_type，保留 raw byte + 事后 BitStruct.parse（详见 §5.11）

```python
from dataclasses import dataclass
from construct import (
    StructMixin, BitStructMixin, field,
    Int8ub, Int16ub, Int32ub, Bytes, GreedyBytes,
    Bit, BitsInteger,
    Prefixed, GreedyRange, Switch,
)

# ─── DATA chunk flags：BitStruct（8 bit 位域分解）───
@dataclass
class DataChunkFlags(BitStructMixin):
    """RFC 4960 3.3.1 DATA chunk flags：5 reserved + U + B + E"""
    reserved: int = field(BitsInteger(5))
    u: int = field(Bit())   # Unordered
    b: int = field(Bit())   # Beginning fragment
    e: int = field(Bit())   # Ending fragment

# ─── chunk value 结构（每种 chunk_type 一个）───
@dataclass
class DataChunkValue(StructMixin):
    tsn: int = field(Int32ub)
    stream_id: int = field(Int16ub)
    stream_seq: int = field(Int16ub)
    ppid: int = field(Int32ub)
    user_data: bytes = field(GreedyBytes)   # 读到子流末尾

@dataclass
class AbortChunkValue(StructMixin):
    error_causes: bytes = field(GreedyBytes)

# ─── chunk inner：在 Prefixed 子流中（length 已被消费）───
@dataclass
class ChunkInner(StructMixin):
    chunk_type: int = field(Int8ub)
    flags_raw: int = field(Int8ub)   # 动态 bitfield：保留 raw byte，事后用 BitStruct 解码
    value: object = field(Switch(chunk_type, {
        0: DataChunkValue,    # DATA
        6: AbortChunkValue,   # ABORT
    }))

# ─── SCTP packet ───
@dataclass
class SCTPPacket(StructMixin):
    src_port: int = field(Int16ub)
    dst_port: int = field(Int16ub)
    verify_tag: int = field(Int32ub)
    # checksum 在 offset 8-11（中间位置）-> 用 Bytes(4) + 外部 CRC32c 函数（§4.4.4）
    checksum: bytes = field(Bytes(4))
    chunks: list = field(GreedyRange(
        Prefixed(Int16ub, ChunkInner, includelength=True)
    ))


# ─── 构造一个含 DATA chunk 的 SCTP 包 ───
# 简化：checksum 字段先填 0，实际使用时由外部 crc32c() 函数填入（见 §4.4.4 模式）
data_chunk = ChunkInner(
    chunk_type=0,
    flags_raw=0x03,    # reserved=0, u=0, b=1, e=1（开始+结束 = 单片段消息）
    value=DataChunkValue(
        tsn=1, stream_id=0, stream_seq=0, ppid=0,
        user_data=b"hello",
    ),
)
packet = SCTPPacket(
    src_port=1234, dst_port=5678, verify_tag=0,
    checksum=b"\x00\x00\x00\x00",   # 占位，事后由外部 crc32c() 填
    chunks=[data_chunk],
)
built = packet.build()

# parse + round-trip
parsed = SCTPPacket.parse(built)
assert parsed.src_port == 1234
assert len(parsed.chunks) == 1
assert isinstance(parsed.chunks[0].value, DataChunkValue)
assert parsed.chunks[0].value.user_data == b"hello"
assert parsed.build() == built

# ─── 事后解码 DATA flags 为 BitStruct（动态 bitfield 变通，§5.11）───
if parsed.chunks[0].chunk_type == 0:
    flags = DataChunkFlags.parse(bytes([parsed.chunks[0].flags_raw]))
    assert flags.b == 1 and flags.e == 1   # B=1, E=1
```

**关键点总结**（指向 SKILL 各章节）：

- **GreedyRange + Prefixed**：处理 chunk 序列（详见 §4.5.6）
- **`includelength=True`**：SCTP chunk_length 含自身（详见 §4.5.5）
- **BitStruct 事后解码**：DATA flags 是动态 bitfield（依赖 chunk_type），用 raw Int8ub + 事后 BitStruct.parse（详见 §5.11）
- **中间位置 checksum**：用 Bytes(4) + 外部 crc32c() 函数，不能用 Checksum 构造器（详见 §4.4.4）
- **完整 SCTP 实现参考**：`experiments/skill-validation/sctp_crs.py`（验证 agent 凭 SKILL 单轮实现的完整 SCTP，含 53 项 round-trip + 8 项 parity 测试）

## 7. 调试技巧

### 7.1 Probe 打印 context

```python
from construct import Probe

@dataclass
class P(StructMixin):
    a: int = field(Int8ub)
    _debug: None = rfield(Probe())    # parse 时打印整个 context
    b: int = field(Int8ub)
```

### 7.2 Tell 标记位置

```python
@dataclass
class P(StructMixin):
    pos1: int = rfield(Tell())
    data: bytes = field(Bytes(4))
    pos2: int = rfield(Tell())
    # parse 后 pos1=0, pos2=4
```

### 7.3 编译错误排查

常见错误：

- `字段引用未找到` -> 字段名拼写错误，或被引用字段是 WO 模式（WO 不写 context）
- `字段的表达式引用了后序字段` -> 表达式只能引用前序字段
- `不支持的表达式运算符` -> 用了 `/` 或浮点运算
- `Checksum requires either bytesfunc or both start and end` -> Checksum 参数缺失
- `BitFieldError` -> BitStruct 字段总 bit 数不是 8 的倍数

### 7.4 parity 对比 Python construct 时的注意事项

如需与 Python `construct` 2.10.70 对比（parity 测试）：

- Python construct 的 `Container` 含 `_io` 等内部状态字段，JSON 比对前必须过滤
  `_*` 开头键：`{k: v for k, v in dict(d).items() if not str(k).startswith('_')}`
- construct-rs 的 `Sequence` build 不含 RO 字段占位（Python 原版需含 None 占位）
- 字段名引用语法不同：construct-rs 用字段名直接引用，Python construct 用 `this.xxx`

## 8. 速查表（最常用模式）

| 我要做... | 用什么 |
|----------|-------|
| 定长整数字段 | `field(Int8ub)` / `field(Int16ub)` / ... |
| 定长字节串 | `field(Bytes(N))` |
| 读到 EOF 的字节 | `field(GreedyBytes)` |
| 字段引用其他字段长度 | `field(Bytes(my_count_field))` |
| 自动算的字段（build 时算） | `rfield(Tell())` / `rfield(Computed(expr))` |
| 常量字段 | `field(Const(b"SIG"))` / `field(Const(0x01, Int8ub))` |
| padding | `wfield(Bytes(N))` / `wfield(Padding(N))` |
| 嵌套子 Struct | `field(ChildStruct)` |
| bitfield | 继承 `BitStructMixin`，字段用 `Bit()` / `BitsInteger(N)` |
| 条件分支（多） | `field(Switch(key_field, {1: A, 2: B}))` |
| 条件分支（双） | `field(IfThenElse(cond, A, B))` |
| 复杂 keyfunc | 先 `rfield(Computed(expr))` 再 Switch 该字段 |
| CRC/Checksum | `rfield(Tell())` + body + `rfield(Tell())` + `field(Checksum(...))` |
| 数组（变长） | `field(Array(count_field, subcon))` |
| 数组（前缀计数） | `field(PrefixedArray(Int8ub, subcon))` |
| 字符串（C 风格） | `field(CString("utf8"))` |
| 枚举映射 | `field(Enum(Int8ub, READ=1, WRITE=2))` |
| 预读不消费 | `field(Peek(Int8ub))`（必须 field！） |
| 长度前缀子流（length 含自身） | `field(Prefixed(Int16ub, Sub, includelength=True))`（详见 §4.5.5） |
| 长度前缀变长记录序列 | `field(GreedyRange(Prefixed(Int16ub, Record, includelength=True)))`（详见 §4.5.6） |

## 9. 实现协议的步骤（推荐流程）

1. **画协议字段图**：列出每个字段的字节/bit 偏移、类型、含义
2. **识别 bitfield**：把 bit 级字段抽成 `BitStructMixin` 子类
3. **识别条件分支**：找出决定后续结构的"类型字段"
4. **设计 Computed 预计算**：复杂 keyfunc 先用 Computed 算到新字段
5. **设计 Checksum**：用 Tell(start) + body + Tell(end) + Checksum 模式
6. **写 @dataclass 类**：从外到内，先父 Struct 再子 Struct
7. **测试 round-trip**：构造典型字节 -> parse -> build -> 比对
8. **测试边界**：字段最大/最小值、错误 CRC、未知 type_code

## 附录：完整 import 速查

```python
from dataclasses import dataclass
from construct import (
    # 基类
    StructMixin, BitStructMixin,
    # 字段声明
    field, rfield, wfield,
    # 整数（最常用）
    Int8ub, Int8ul, Int16ub, Int16ul, Int32ub, Int32ul, Int64ub, Int64ul,
    Byte, Short, Int, Long,
    # 字节
    Bytes, GreedyBytes, BytesInteger,
    # 变长整数
    VarInt, ZigZag,
    # 字符串
    CString, GreedyString, PaddedString, PascalString,
    # bit 域
    Bit, Nibble, Octet, BitsInteger, Bitwise, Padding,
    # 数组
    Array, GreedyRange, PrefixedArray,
    # Adapter
    Const, Default, Check, Computed, Peek, RawCopy, Rebuild,
    Enum, FlagsEnum, Mapping, OneOf, NoneOf, Union,
    Hex, HexDump, Subconstruct, Tell, Terminated, Probe, Pass,
    # 条件
    If, IfThenElse, Switch, Select, FocusedSeq, StopIf,
    # 流
    Seek, Pointer, Prefixed,
    # 对齐
    Aligned, AlignedStruct,
    # 校验
    Checksum, HashAlgo,
    # 结构
    Sequence, NamedTuple,
    # 其他
    Timestamp, ProcessXor, ProcessRotateLeft, Adapter,
    # 异常
    ConstructError, ChecksumError, ConstError, CheckError,
    ValidationError, MappingError, SelectError, TerminatedError,
)
```

---

> **本 SKILL 结束**。读完本文件 + 跑过本章示例，你已能用 construct-rs 实现任何
> 真实二进制协议。如遇问题，先查 5 易错点。