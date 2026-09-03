"""IEC 60870-5-104 协议系统测试。

协议参考：IEC 60870-5-104 (Telecontrol equipment and systems - Transmission protocols)

## 协议结构

IEC 104 是工业 SCADA 系统的远动协议，基于 TCP（端口号 2404）。
应用数据单元（APDU）格式::

    +-------+---------+--------------------+
    | Start | Length  | Control Field (4B) | [ASDU (variable, only I-frame)]
    +-------+---------+--------------------+
    | 0x68  | 1 byte  |       4 bytes      |
    +-------+---------+--------------------+

- **Start**: 始终 0x68
- **APDU Length**: 后续字节数（4 = 仅 APCI；4 + N = APCI + ASDU(N 字节)）
- **Control Field**: 4 字节，编码帧类型与序列号
- **ASDU**: 仅 I-frame 包含，应用数据

## Control Field 帧类型（按 bit 0、bit 1 of CF1 区分）

| type (bit0+2*bit1) | 类型 | 用途                  |
|--------------------|------|----------------------|
| 0 (bit0=0)         | I    | 信息传输（带 send/recv 序列号） |
| 1 (bit0=1, bit1=0) | S    | 编号监督（带 recv 序列号） |
| 3 (bit0=1, bit1=1) | U    | 未编号（STARTDT/STOPDT/TESTFR） |

### I-format bit 布局
```
CF1: [0, S6, S5, S4, S3, S2, S1, S0]   <- bit 0 + 低 7 位 send seq
CF2: [S14, S13, S12, S11, S10, S9, S8, S7]  <- 高 8 位 send seq
CF3: [0, R6, R5, R4, R3, R2, R1, R0]   <- bit 0 + 低 7 位 recv seq
CF4: [R14, R13, R12, R11, R10, R9, R8, R7]
```
send_seq = (CF2 << 7) | (CF1 >> 1)
recv_seq = (CF4 << 7) | (CF3 >> 1)

### S-format
- CF1 = 0x01（固定）
- CF2 = 0x00
- CF3/CF4 编码 recv seq（同 I-format 的 CF3/CF4）

### U-format
- CF1 bits 2-7 编码：STARTDT/STOPDT/TESTFR 的 act/con（成对，2 bit 每对）
- CF2 = CF3 = CF4 = 0x00
- 具体值（CF1）：
  - STARTDT act = 0x07（bits 2,3 set，0b00000111）
  - STARTDT con = 0x0B（bits 2,4 set，0b00001011）
  - STOPDT act = 0x13（bits 4,5 set，0b00010011）
  - STOPDT con = 0x23（bits 4,6 set，0b00100011）
  - TESTFR act = 0x43（bits 6,7 set，0b01000011）
  - TESTFR con = 0x83（bits 7,8 set... wait, 7-bit only, bit 8 doesn't exist）

## 覆盖能力

- **条件分支**：Peek(Int8ub) 预读 CF1 → Computed 算 frame_type → Switch 分派
- **嵌套 Struct**：每个帧类型的 control field 是独立的 StructMixin 子类
- **表达式**：Computed 用 `cf1 & 3` 提取 frame_type；用位运算重组 15-bit 序列号
- **常量**：Const(b"\\x68") 校验起始字节
"""

from __future__ import annotations

from dataclasses import dataclass

import pytest

from construct import (
    Bytes,
    Computed,
    Const,
    ConstError,
    Int8ub,
    Peek,
    StructMixin,
    Switch,
    field,
    rfield,
)


# ---------------------------------------------------------------------------
# U-format 帧功能码常量
# ---------------------------------------------------------------------------

# IEC 60870-5-104 U-format function codes（CF1 byte）
STARTDT_ACT = 0x07
STARTDT_CON = 0x0B
STOPDT_ACT = 0x13
STOPDT_CON = 0x23
TESTFR_ACT = 0x43
TESTFR_CON = 0x83

U_FUNCTION_NAMES = {
    STARTDT_ACT: "STARTDT act",
    STARTDT_CON: "STARTDT con",
    STOPDT_ACT: "STOPDT act",
    STOPDT_CON: "STOPDT con",
    TESTFR_ACT: "TESTFR act",
    TESTFR_CON: "TESTFR con",
}


# ---------------------------------------------------------------------------
# 协议定义
# ---------------------------------------------------------------------------


@dataclass
class IFormatControl(StructMixin):
    """I-format 控制域（4 字节）。

    序列号通过 Computed + 位运算表达式重建。
    """

    cf1: int = field(Int8ub)
    cf2: int = field(Int8ub)
    cf3: int = field(Int8ub)
    cf4: int = field(Int8ub)
    # 表达式：send_seq = (cf2 << 7) | (cf1 >> 1)
    send_seq: int = rfield(Computed((cf2 << 7) | (cf1 >> 1)))
    recv_seq: int = rfield(Computed((cf4 << 7) | (cf3 >> 1)))


@dataclass
class SFormatControl(StructMixin):
    """S-format 控制域（4 字节）。

    CF1 = 0x01 固定（bit0=1, bit1=0）。
    """

    cf1: int = field(Const(0x01, Int8ub))
    cf2: int = field(Const(0x00, Int8ub))
    cf3: int = field(Int8ub)
    cf4: int = field(Int8ub)
    recv_seq: int = rfield(Computed((cf4 << 7) | (cf3 >> 1)))


@dataclass
class UFormatControl(StructMixin):
    """U-format 控制域（4 字节）。

    CF1 = STARTDT/STOPDT/TESTFR act/con 之一，CF2-CF4 = 0。
    """

    cf1: int = field(Int8ub)
    cf2: int = field(Const(0x00, Int8ub))
    cf3: int = field(Const(0x00, Int8ub))
    cf4: int = field(Const(0x00, Int8ub))


@dataclass
class APCI(StructMixin):
    """IEC 104 APCI + 控制域（不含 ASDU）。

    通过 Peek 预读 CF1，Computed 计算 frame_type，Switch 分派到 I/S/U 控制域结构。

    frame_type 值（按 IEC 60870-5-104 §4 协议规则）：
    - 0 → I-format（CF1 bit 0 = 0）
    - 1 → S-format（CF1 bits 0,1 = 1,0）
    - 3 → U-format（CF1 bits 0,1 = 1,1）

    **关键公式**：``cf1 & 3`` 不能直接区分 I-frame（因 send_seq 的低位会
    出现在 cf1 bit 1 上）。需要用乘法 + 位运算组合::

        frame_type = (cf1 & 1) * (1 + 2 * ((cf1 >> 1) & 1))

    验证：
    - I-frame cf1=0x02 (send_seq=1): (0)*(1+2*1) = 0 ✓
    - S-frame cf1=0x01: (1)*(1+0) = 1 ✓
    - U-frame cf1=0x03: (1)*(1+2) = 3 ✓

    注：``cf1_peek`` 必须用 ``field()`` (RW) 而非 ``rfield()`` (RO)，因为
    PeekNode 不在 build_ro_value 允许列表中（与 ChecksumNode 同情况）。
    """

    start: bytes = field(Const(b"\x68"))
    apdu_length: int = field(Int8ub)
    cf1_peek: int = field(Peek(Int8ub))
    frame_type: int = rfield(Computed((cf1_peek & 1) * (1 + 2 * ((cf1_peek >> 1) & 1))))
    control_field: object = field(
        Switch(
            frame_type,
            {
                0: IFormatControl,
                1: SFormatControl,
                3: UFormatControl,
            },
        )
    )


# ---------------------------------------------------------------------------
# 工具：构造 I/S/U 测试字节
# ---------------------------------------------------------------------------


def build_i_format_bytes(send_seq: int, recv_seq: int, asdu: bytes = b"") -> bytes:
    """构造 I-format APDU（6 字节 APCI + ASDU）。"""
    cf1 = (send_seq << 1) & 0xFE  # bit 0 = 0
    cf2 = (send_seq >> 7) & 0xFF
    cf3 = (recv_seq << 1) & 0xFE
    cf4 = (recv_seq >> 7) & 0xFF
    control = bytes([cf1, cf2, cf3, cf4])
    length = len(control) + len(asdu)
    return bytes([0x68, length]) + control + asdu


def build_s_format_bytes(recv_seq: int) -> bytes:
    """构造 S-format APDU（6 字节 APCI，无 ASDU）。"""
    cf1 = 0x01
    cf2 = 0x00
    cf3 = (recv_seq << 1) | 0x01  # bit 0 = 1 (S-frame marker)
    # Actually S-format requires CF3 = (recv_seq << 1) | 1? Let me re-check.
    # No: S-format CF3 bit 0 = 0 (same as I-format). The CF1 bit 0 = 1 distinguishes S from I.
    # The recv_seq is in CF3 bits 1-7 and CF4.
    cf3 = (recv_seq << 1) & 0xFE
    cf4 = (recv_seq >> 7) & 0xFF
    control = bytes([cf1, cf2, cf3, cf4])
    return bytes([0x68, 4]) + control


def build_u_format_bytes(u_function: int) -> bytes:
    """构造 U-format APDU（6 字节 APCI，无 ASDU）。"""
    control = bytes([u_function, 0x00, 0x00, 0x00])
    return bytes([0x68, 4]) + control


# ---------------------------------------------------------------------------
# Round-trip 测试
# ---------------------------------------------------------------------------


class TestIEC104RoundTrip:
    """I/S/U 三种帧类型 round-trip 验证。"""

    def test_i_format_basic(self):
        """I-format：send_seq=0, recv_seq=0（启动后首帧）。"""
        data = build_i_format_bytes(send_seq=0, recv_seq=0)
        # 0x68 0x04 0x00 0x00 0x00 0x00
        assert data == bytes([0x68, 0x04, 0x00, 0x00, 0x00, 0x00])

        parsed = APCI.parse(data)
        assert parsed.frame_type == 0
        assert isinstance(parsed.control_field, IFormatControl)
        assert parsed.control_field.send_seq == 0
        assert parsed.control_field.recv_seq == 0

        assert parsed.build() == data

    def test_i_format_with_sequence(self):
        """I-format：send_seq=12, recv_seq=7（典型数据传输）。"""
        data = build_i_format_bytes(send_seq=12, recv_seq=7)
        # cf1 = (12 << 1) = 0x18, cf2 = 0
        # cf3 = (7 << 1) = 0x0E, cf4 = 0
        assert data == bytes([0x68, 0x04, 0x18, 0x00, 0x0E, 0x00])

        parsed = APCI.parse(data)
        assert parsed.frame_type == 0
        assert parsed.control_field.send_seq == 12
        assert parsed.control_field.recv_seq == 7

        assert parsed.build() == data

    def test_i_format_with_asdu(self):
        """I-format 带 ASDU（length > 4）。

        ASDU 部分用 GreedyBytes 捕获剩余字节（在 IFormatAPDU 中，非 APCI 类）。
        本测试只验证 APCI 部分，APDU 字节会包含 ASDU 但 APCI.parse 只读 6 字节。
        """
        asdu = bytes([0x01, 0x01, 0x06, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00])
        data = build_i_format_bytes(send_seq=1, recv_seq=0, asdu=asdu)
        assert len(data) == 6 + len(asdu)

        parsed = APCI.parse(data)
        assert parsed.frame_type == 0
        assert parsed.control_field.send_seq == 1
        assert parsed.control_field.recv_seq == 0
        assert parsed.apdu_length == 4 + len(asdu)

        # APCI 仅消费 6 字节，build 输出仅 APCI 部分
        rebuilt = parsed.build()
        assert rebuilt == bytes([0x68, 4 + len(asdu), 0x02, 0x00, 0x00, 0x00])

    def test_s_format(self):
        """S-format：recv_seq=5。"""
        data = build_s_format_bytes(recv_seq=5)
        # cf1 = 0x01, cf2 = 0x00, cf3 = (5 << 1) = 0x0A, cf4 = 0
        assert data == bytes([0x68, 0x04, 0x01, 0x00, 0x0A, 0x00])

        parsed = APCI.parse(data)
        assert parsed.frame_type == 1
        assert isinstance(parsed.control_field, SFormatControl)
        assert parsed.control_field.recv_seq == 5

        assert parsed.build() == data

    @pytest.mark.parametrize(
        "u_func,name",
        [
            (STARTDT_ACT, "STARTDT act"),
            (STARTDT_CON, "STARTDT con"),
            (STOPDT_ACT, "STOPDT act"),
            (STOPDT_CON, "STOPDT con"),
            (TESTFR_ACT, "TESTFR act"),
            (TESTFR_CON, "TESTFR con"),
        ],
    )
    def test_u_format(self, u_func, name):
        """U-format：6 种功能（STARTDT/STOPDT/TESTFR act/con）。"""
        data = build_u_format_bytes(u_func)

        parsed = APCI.parse(data)
        assert parsed.frame_type == 3
        assert isinstance(parsed.control_field, UFormatControl)
        assert parsed.control_field.cf1 == u_func
        # 验证其他字节为 0
        assert parsed.control_field.cf2 == 0
        assert parsed.control_field.cf3 == 0
        assert parsed.control_field.cf4 == 0

        assert parsed.build() == data


# ---------------------------------------------------------------------------
# 复杂场景：序列号边界 + 常量校验
# ---------------------------------------------------------------------------


class TestIEC104SequenceBoundaries:
    """15-bit 序列号边界测试（0 / 32767 / wrap-around）。"""

    @pytest.mark.parametrize(
        "send_seq,recv_seq",
        [
            (0, 0),
            (1, 0),
            (0, 1),
            (32766, 32767),  # 接近最大值
            (32767, 0),  # 15-bit 最大值
            (32767, 32767),
        ],
    )
    def test_i_format_sequence_round_trip(self, send_seq, recv_seq):
        """I-format 各种 15-bit 序列号组合 round-trip。"""
        data = build_i_format_bytes(send_seq=send_seq, recv_seq=recv_seq)
        parsed = APCI.parse(data)
        assert parsed.control_field.send_seq == send_seq
        assert parsed.control_field.recv_seq == recv_seq
        assert parsed.build() == data


class TestIEC104StartByteValidation:
    """Const(b'\\x68') 校验起始字节。"""

    def test_wrong_start_byte_raises(self):
        """起始字节非 0x68 → ConstError。"""
        bad_data = bytes([0x00, 0x04, 0x01, 0x00, 0x00, 0x00])
        with pytest.raises(ConstError):
            APCI.parse(bad_data)

    def test_correct_start_byte_accepted(self):
        """起始字节 0x68 → parse 成功。"""
        good_data = bytes([0x68, 0x04, 0x01, 0x00, 0x00, 0x00])
        parsed = APCI.parse(good_data)
        assert parsed.start == b"\x68"


# ---------------------------------------------------------------------------
# 复杂场景：S-format 常量字段校验
# ---------------------------------------------------------------------------


class TestIEC104ConstFields:
    """S-format / U-format 的 Const 字段（固定字节）。"""

    def test_s_format_cf1_must_be_01(self):
        """S-format CF1 必须 = 0x01，否则 ConstError。"""
        # 假设有人手工构造一个 frame_type=1 但 CF1=0x03 的非法帧
        # frame_type = 0x03 & 3 = 3，会被 Switch 路由到 U-format，不会触发 S 的 Const 校验
        # 此处用 frame_type=1 (CF1=0x05，bit 0=1, bit 1=0, bit 2=1) 但 cf2 != 0 来测试
        bad_data = bytes([0x68, 0x04, 0x05, 0xFF, 0x00, 0x00])
        with pytest.raises(ConstError):
            APCI.parse(bad_data)

    def test_u_format_cf2_must_be_00(self):
        """U-format CF2 必须 = 0x00，否则 ConstError。"""
        bad_data = bytes([0x68, 0x04, STARTDT_ACT, 0xFF, 0x00, 0x00])
        with pytest.raises(ConstError):
            APCI.parse(bad_data)


# ---------------------------------------------------------------------------
# Parity vs Python construct 2.10.70
# ---------------------------------------------------------------------------


class TestIEC104Parity:
    """parity vs Python construct 2.10.70（子进程隔离）。

    Python 原版可用 cf1 字段表达式 + Switch 实现等价逻辑。
    本测试仅对比 APCI 字段（start / apdu_length / frame_type / control_field 字段）。
    """

    @pytest.mark.parametrize(
        "data_bytes",
        [
            # I-format: send=0, recv=0
            bytes([0x68, 0x04, 0x00, 0x00, 0x00, 0x00]),
            # I-format: send=12, recv=7
            bytes([0x68, 0x04, 0x18, 0x00, 0x0E, 0x00]),
            # S-format: recv=5
            bytes([0x68, 0x04, 0x01, 0x00, 0x0A, 0x00]),
            # U-format: STARTDT act
            bytes([0x68, 0x04, STARTDT_ACT, 0x00, 0x00, 0x00]),
            # U-format: TESTFR con
            bytes([0x68, 0x04, TESTFR_CON, 0x00, 0x00, 0x00]),
        ],
        ids=[
            "I-format-0-0",
            "I-format-12-7",
            "S-format-recv5",
            "U-format-STARTDT-act",
            "U-format-TESTFR-con",
        ],
    )
    def test_parse_parity(self, data_bytes):
        """parse 同字节流 → rs 与 py 字段值一致。"""
        from ._system_helpers import run_parity_snippet

        data_hex = data_bytes.hex()

        rs_code = f"""
import json
from dataclasses import dataclass, asdict
from construct import (
    StructMixin, field, rfield, Bytes, Int8ub, Const, Computed, Peek, Switch,
)

@dataclass
class ICtrl(StructMixin):
    cf1: int = field(Int8ub)
    cf2: int = field(Int8ub)
    cf3: int = field(Int8ub)
    cf4: int = field(Int8ub)
    send_seq: int = rfield(Computed((cf2 << 7) | (cf1 >> 1)))
    recv_seq: int = rfield(Computed((cf4 << 7) | (cf3 >> 1)))

@dataclass
class SCtrl(StructMixin):
    cf1: int = field(Const(0x01, Int8ub))
    cf2: int = field(Const(0x00, Int8ub))
    cf3: int = field(Int8ub)
    cf4: int = field(Int8ub)
    recv_seq: int = rfield(Computed((cf4 << 7) | (cf3 >> 1)))

@dataclass
class UCtrl(StructMixin):
    cf1: int = field(Int8ub)
    cf2: int = field(Const(0x00, Int8ub))
    cf3: int = field(Const(0x00, Int8ub))
    cf4: int = field(Const(0x00, Int8ub))

@dataclass
class APCI(StructMixin):
    start: bytes = field(Const(b"\\x68"))
    apdu_length: int = field(Int8ub)
    cf1_peek: int = field(Peek(Int8ub))
    frame_type: int = rfield(Computed((cf1_peek & 1) * (1 + 2 * ((cf1_peek >> 1) & 1))))
    control_field: object = field(Switch(frame_type, {{
        0: ICtrl, 1: SCtrl, 3: UCtrl,
    }}))

p = APCI.parse(bytes.fromhex('{data_hex}'))
# Convert control_field dataclass to dict for JSON
def _conv(o):
    if hasattr(o, '__dataclass_fields__'):
        # 过滤 RO 字段（None 值）
        return {{k: getattr(o, k) for k in o.__dataclass_fields__ if getattr(o, k) is not None}}
    if isinstance(o, (bytes, bytearray)):
        return {{'__bytes__': o.hex()}}
    return o

result = {{
    'apdu_length': p.apdu_length,
    'frame_type': p.frame_type,
    'control_field': _conv(p.control_field),
}}
print(json.dumps(result))
"""
        py_code = f"""
import json
import construct as pc

ICtrl = pc.Struct(
    'cf1' / pc.Int8ub,
    'cf2' / pc.Int8ub,
    'cf3' / pc.Int8ub,
    'cf4' / pc.Int8ub,
    'send_seq' / pc.Computed(lambda ctx: (ctx.cf2 << 7) | (ctx.cf1 >> 1)),
    'recv_seq' / pc.Computed(lambda ctx: (ctx.cf4 << 7) | (ctx.cf3 >> 1)),
)
SCtrl = pc.Struct(
    'cf1' / pc.Const(0x01, pc.Int8ub),
    'cf2' / pc.Const(0x00, pc.Int8ub),
    'cf3' / pc.Int8ub,
    'cf4' / pc.Int8ub,
    'recv_seq' / pc.Computed(lambda ctx: (ctx.cf4 << 7) | (ctx.cf3 >> 1)),
)
UCtrl = pc.Struct(
    'cf1' / pc.Int8ub,
    'cf2' / pc.Const(0x00, pc.Int8ub),
    'cf3' / pc.Const(0x00, pc.Int8ub),
    'cf4' / pc.Const(0x00, pc.Int8ub),
)

APCI = pc.Struct(
    'start' / pc.Const(b"\x68"),
    'apdu_length' / pc.Int8ub,
    'cf1_peek' / pc.Peek(pc.Int8ub),
    'frame_type' / pc.Computed(lambda ctx: (ctx.cf1_peek & 1) * (1 + 2 * ((ctx.cf1_peek >> 1) & 1))),
    'control_field' / pc.Switch(lambda ctx: ctx.frame_type, {{
        0: ICtrl, 1: SCtrl, 3: UCtrl,
    }}),
)

p = APCI.parse(bytes.fromhex('{data_hex}'))
def _strip(d):
    return {{k: v for k, v in dict(d).items() if not str(k).startswith('_')}}
result = {{
    'apdu_length': p.apdu_length,
    'frame_type': p.frame_type,
    'control_field': _strip(p.control_field),
}}
print(json.dumps(result))
"""
        rs_result = run_parity_snippet("rs", rs_code, py_code)
        py_result = run_parity_snippet("py", rs_code, py_code)
        assert rs_result == py_result, (
            f"IEC104 parity mismatch for data {data_hex}\n"
            f"  rs: {rs_result}\n"
            f"  py: {py_result}"
        )
