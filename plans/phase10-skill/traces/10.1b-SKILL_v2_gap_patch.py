"""Patch construct-rs-usage SKILL.md to fill validation gaps 1/2/3/5.

Idempotent guard: each replacement asserts the anchor exists exactly once.
Aborts with non-zero exit on any mismatch so we never silently corrupt file.

Encoding: read/write UTF-8 without BOM (matches existing file).

This script is a process artifact (DEV permission workaround for editing
.opencode/** which is not in opencode.json edit allowlist; see trace §7).
"""
import sys
from pathlib import Path

SKILL_PATH = Path(__file__).resolve().parents[3] / ".opencode" / "skills" / "construct-rs-usage" / "SKILL.md"


def apply_patch(text, old, new, label):
    count = text.count(old)
    if count == 0:
        print("[FAIL] {}: anchor not found".format(label), file=sys.stderr)
        sys.exit(1)
    if count > 1:
        print("[FAIL] {}: anchor found {} times (need unique)".format(label, count), file=sys.stderr)
        sys.exit(1)
    delta = len(new) - len(old)
    print("[OK] {}: 1 replacement applied ({:+d} chars)".format(label, delta))
    return text.replace(old, new, 1)


NEW_SECTIONS_455_456 = """#### 4.5.5 Prefixed 详解（includelength 两种模式）

**做什么**：用 length 字段定界 subcon 的子流。两种模式仅在"length 值是否含 lengthfield 自身字节"上有差异。

**模式对比**：

| 模式 | length 字段值 | parse 时 substream 长度 | 典型协议 |
|------|--------------|----------------------|---------|
| `includelength=False`（默认） | subcon 字节数 | = length 值 | 自定义 length field（仅计数 payload） |
| `includelength=True` | subcon 字节数 + sizeof(lengthfield) | = length 值 - sizeof(lengthfield) | SCTP chunk_length / 部分 TLS record（"含自身"语义） |

**核心区别**：`includelength=True` 时 length 字段的值**包含 lengthfield 自身大小**。注意是 `sizeof(lengthfield)`（如 `Int16ub`=2 字节、`Int32ub`=4 字节、`VarInt` 取实际编码长度），**不是固定 4 字节**。

**完整对照示例**（同一段 payload，两种模式 build 出的字节差异）：

```python
from construct import Prefixed, Int16ub, GreedyBytes

payload = b"\\xAA\\xBB\\xCC"

# ─── 模式 A：includelength=False（默认）───
# length 字段值 = subcon 字节数 = 3
d_a = Prefixed(Int16ub, GreedyBytes)
built_a = d_a.build(payload)
# built_a = b"\\x00\\x03" (Int16ub=3) + b"\\xAA\\xBB\\xCC"
assert built_a == b"\\x00\\x03\\xAA\\xBB\\xCC"
assert d_a.parse(built_a) == payload

# ─── 模式 B：includelength=True ───
# length 字段值 = subcon 字节数 + sizeof(Int16ub) = 3 + 2 = 5
d_b = Prefixed(Int16ub, GreedyBytes, includelength=True)
built_b = d_b.build(payload)
# built_b = b"\\x00\\x05" (Int16ub=5) + b"\\xAA\\xBB\\xCC"
assert built_b == b"\\x00\\x05\\xAA\\xBB\\xCC"
assert d_b.parse(built_b) == payload
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
    \"\"\"DATA chunk value（简化）：TSN + 变长 user_data\"\"\"
    tsn: int = field(Int32ub)
    user_data: bytes = field(GreedyBytes)   # 读到子流末尾

@dataclass
class AbortChunkValue(StructMixin):
    \"\"\"ABORT chunk value：可选错误原因（变长 raw bytes）\"\"\"
    error_causes: bytes = field(GreedyBytes)

@dataclass
class ChunkInner(StructMixin):
    \"\"\"单个 chunk：在 Prefixed substream 中（length 已被 Prefixed 消费）\"\"\"
    chunk_type: int = field(Int8ub)
    flags: int = field(Int8ub)
    value: object = field(Switch(chunk_type, {
        0: DataChunkValue,     # DATA
        6: AbortChunkValue,    # ABORT
    }))

@dataclass
class SCTPChunks(StructMixin):
    \"\"\"SCTP chunk 序列（此处省略 Common Header，仅演示 chunk 序列模式）\"\"\"
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
    b"\\x00\\x0D"            # chunk1 length=13
    b"\\x00\\x03"            # chunk1 type=0, flags=0x03
    b"\\x00\\x00\\x00\\x01"  # tsn=1
    b"hello"                 # user_data
    b"\\x00\\x04"            # chunk2 length=4
    b"\\x06\\x00"            # chunk2 type=6, flags=0x00
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

"""

NEW_SECTIONS_511_512 = """### 5.11 BitStruct 适合静态位域；动态位域请保留 raw byte + 事后解析

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

"""

NEW_SECTION_66 = """### 6.6 综合：SCTP（GreedyRange + Prefixed + Switch + BitStruct + 中间位置 checksum）

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
    \"\"\"RFC 4960 3.3.1 DATA chunk flags：5 reserved + U + B + E\"\"\"
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
    checksum=b"\\x00\\x00\\x00\\x00",   # 占位，事后由外部 crc32c() 填
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

"""


def main():
    text = SKILL_PATH.read_text(encoding="utf-8")
    original_len = len(text)
    original_lines = text.count("\n")

    # Patch 1: §3.3 Prefixed row — add includelength hint
    text = apply_patch(
        text,
        "| `Prefixed(lengthfield, subcon, includelength=False)` | 长度前缀子流（lengthfield 是字节计数） |",
        "| `Prefixed(lengthfield, subcon, includelength=False)` | 长度前缀子流（lengthfield 是字节计数；`includelength=True` 时 length 含 lengthfield 自身大小，详见 §4.5.5） |",
        "Patch 1 (§3.3 Prefixed row)",
    )

    # Patch 2: insert §4.5.5 + §4.5.6 before §4.6
    text = apply_patch(
        text,
        "### 4.6 字符串",
        NEW_SECTIONS_455_456 + "### 4.6 字符串",
        "Patch 2 (§4.5.5 + §4.5.6 insert)",
    )

    # Patch 3: append §5.11 + §5.12 at end of §5 (before §6)
    text = apply_patch(
        text,
        "## 6. 完整示例（递进复杂度）",
        NEW_SECTIONS_511_512 + "## 6. 完整示例（递进复杂度）",
        "Patch 3 (§5.11 + §5.12 insert)",
    )

    # Patch 4: insert §6.6 before §7
    text = apply_patch(
        text,
        "## 7. 调试技巧",
        NEW_SECTION_66 + "## 7. 调试技巧",
        "Patch 4 (§6.6 SCTP insert)",
    )

    # Patch 5: §8 cheat sheet — add two rows after the Peek row
    text = apply_patch(
        text,
        "| 预读不消费 | `field(Peek(Int8ub))`（必须 field！） |\n",
        "| 预读不消费 | `field(Peek(Int8ub))`（必须 field！） |\n"
        "| 长度前缀子流（length 含自身） | `field(Prefixed(Int16ub, Sub, includelength=True))`（详见 §4.5.5） |\n"
        "| 长度前缀变长记录序列 | `field(GreedyRange(Prefixed(Int16ub, Record, includelength=True)))`（详见 §4.5.6） |\n",
        "Patch 5 (§8 cheat sheet rows)",
    )

    SKILL_PATH.write_text(text, encoding="utf-8")
    new_lines = text.count("\n")
    print("\nDONE: {} -> {} bytes ({:+d}), {} -> {} lines ({:+d})".format(
        original_len, len(text), len(text) - original_len,
        original_lines, new_lines, new_lines - original_lines,
    ))


if __name__ == "__main__":
    main()
