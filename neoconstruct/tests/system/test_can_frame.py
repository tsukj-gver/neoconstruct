"""CAN 2.0A/B 帧协议系统测试。

协议参考：
- ISO 11898-1:2015（CAN specification）
- Linux SocketCAN can_frame 结构（linux/can.h）

## 协议结构（SocketCAN 字节级表示）

实际 CAN 总线是 bitstuff + 差分编码，但应用层接口（SocketCAN / Vector tools）
统一使用以下字节级结构（剥除 stuff bits）::

    +---------------------+-----+-----------------+
    |    can_id (4 bytes) | DLC | data (0-8 byte) |
    +---------------------+-----+-----------------+

- **can_id**: 32-bit（little-endian in SocketCAN），含 ID + 标志位
  - bit 31: ERR（错误帧）
  - bit 30: RTR（远程帧）
  - bit 29: EFF（扩展帧标志，1=29-bit ID, 0=11-bit ID）
  - bits 28-11: ext_id_low（扩展帧的 ID 高 18 位，仅 EFF=1 时有效）
  - bits 10-0: std_id（11-bit 标准 ID；扩展帧时为 ID 低 11 位）

- **DLC**: 1 字节（数据长度代码，0-8）

- **data**: DLC 字节的数据

## 标识符重建

- 标准帧（EFF=0）：identifier = std_id（11 bit）
- 扩展帧（EFF=1）：identifier = (ext_id_low << 11) | std_id（29 bit）

## 覆盖能力

- **bitfield**：BitStruct 32-bit ID + 标志位（Bit / BitsInteger）
- **条件分支**：在测试代码中根据 eff 标志重建实际标识符
  （neoconstruct 的 Computed 不能跨层引用子 BitStruct 字段，故在 Python 层处理）
- **变长字段**：Bytes(dlc) 引用 DLC 字段长度
"""

from __future__ import annotations

from dataclasses import dataclass

import pytest

from neoconstruct import (
    BitStructMixin,
    Bit,
    BitsInteger,
    Bytes,
    Int8ub,
    Padding,
    StructMixin,
    field,
    wfield,
)


# ---------------------------------------------------------------------------
# 协议定义
# ---------------------------------------------------------------------------


@dataclass
class CANIdentifier(BitStructMixin):
    """32-bit CAN ID + 标志位（与 Linux can_id_t 布局一致，大端 bit 序）。

    BitStruct 总长 = 1 + 1 + 1 + 18 + 11 = 32 bit = 4 字节（无须 padding）。

    Python construct 原版 BitStruct 默认使用大端 bit 序（MSB first），
    本实现保持一致以与 parity 对齐。
    """

    err: int = field(Bit())  # bit 31
    rtr: int = field(Bit())  # bit 30
    eff: int = field(Bit())  # bit 29
    ext_id_low: int = field(BitsInteger(18))  # bits 28-11
    std_id: int = field(BitsInteger(11))  # bits 10-0


@dataclass
class CANFrame(StructMixin):
    """CAN 帧（SocketCAN 字节级表示）。

    通过 ``can_id`` 字段（CANIdentifier BitStruct）解析 ID + 标志，
    DLC 决定 data 长度（0-8 字节）。

    标识符重建（标准 vs 扩展）在测试代码中完成（不能在 Computed 中跨层引用）。
    """

    can_id: CANIdentifier = field(CANIdentifier)
    dlc: int = field(Int8ub)
    data: bytes = field(Bytes(dlc))


# ---------------------------------------------------------------------------
# 工具函数
# ---------------------------------------------------------------------------


def encode_can_id(identifier: int, *, eff: bool, rtr: bool = False, err: bool = False) -> int:
    """从应用层 identifier + 标志构造 32-bit can_id 原始值。

    - 标准（EFF=0）：identifier 限 11-bit，放入 bits 10-0
    - 扩展（EFF=1）：identifier 限 29-bit，低 11 位放 std_id，高 18 位放 ext_id_low
    """
    if eff:
        assert 0 <= identifier < (1 << 29), f"ext id too big: {identifier}"
        std_id_part = identifier & 0x7FF
        ext_id_low_part = (identifier >> 11) & 0x3FFFF
    else:
        assert 0 <= identifier < (1 << 11), f"std id too big: {identifier}"
        std_id_part = identifier
        ext_id_low_part = 0
    raw = (
        (1 if err else 0) << 31
        | (1 if rtr else 0) << 30
        | (1 if eff else 0) << 29
        | ext_id_low_part << 11
        | std_id_part
    )
    return raw


def decode_identifier(can_id: CANIdentifier) -> int:
    """从 CANIdentifier 实例重建应用层 identifier。"""
    if can_id.eff:
        return (can_id.ext_id_low << 11) | can_id.std_id
    return can_id.std_id


def can_id_to_bytes(raw: int) -> bytes:
    """32-bit can_id raw 值 → 4 字节 big-endian（与 BitStruct 序对齐）。"""
    return bytes(
        [
            (raw >> 24) & 0xFF,
            (raw >> 16) & 0xFF,
            (raw >> 8) & 0xFF,
            raw & 0xFF,
        ]
    )


# ---------------------------------------------------------------------------
# Round-trip 测试
# ---------------------------------------------------------------------------


class TestCANFrameRoundTrip:
    """标准帧 / 扩展帧 round-trip。"""

    def test_standard_frame_minimal(self):
        """标准帧：ID=0x123, DLC=0, 无数据。"""
        raw = encode_can_id(0x123, eff=False)
        data = can_id_to_bytes(raw) + bytes([0x00])
        # 5 bytes total: 4 can_id + 1 dlc

        parsed = CANFrame.parse(data)
        assert parsed.can_id.eff == 0
        assert parsed.can_id.rtr == 0
        assert parsed.can_id.err == 0
        assert parsed.can_id.std_id == 0x123
        assert parsed.can_id.ext_id_low == 0
        assert parsed.dlc == 0
        assert parsed.data == b""

        assert decode_identifier(parsed.can_id) == 0x123
        assert parsed.build() == data

    def test_standard_frame_with_data(self):
        """标准帧带数据：ID=0x456, DLC=4, data=0xDEADBEEF。"""
        raw = encode_can_id(0x456, eff=False)
        payload = bytes([0xDE, 0xAD, 0xBE, 0xEF])
        data = can_id_to_bytes(raw) + bytes([len(payload)]) + payload

        parsed = CANFrame.parse(data)
        assert parsed.can_id.std_id == 0x456
        assert parsed.dlc == 4
        assert parsed.data == payload
        assert decode_identifier(parsed.can_id) == 0x456

        assert parsed.build() == data

    def test_extended_frame(self):
        """扩展帧：ID=0x1234567 (29-bit)。"""
        identifier = 0x1234567
        raw = encode_can_id(identifier, eff=True)
        # std_id = identifier & 0x7FF = 0x567
        # ext_id_low = (identifier >> 11) & 0x3FFFF = 0x2468
        payload = bytes([0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08])
        data = can_id_to_bytes(raw) + bytes([len(payload)]) + payload

        parsed = CANFrame.parse(data)
        assert parsed.can_id.eff == 1
        assert parsed.can_id.std_id == 0x567
        assert parsed.can_id.ext_id_low == 0x2468
        assert decode_identifier(parsed.can_id) == identifier
        assert parsed.dlc == 8
        assert parsed.data == payload

        assert parsed.build() == data

    def test_remote_frame(self):
        """远程帧（RTR=1）：标准 ID，无数据。"""
        raw = encode_can_id(0x7FF, eff=False, rtr=True)
        data = can_id_to_bytes(raw) + bytes([0x00])

        parsed = CANFrame.parse(data)
        assert parsed.can_id.rtr == 1
        assert parsed.can_id.eff == 0
        assert decode_identifier(parsed.can_id) == 0x7FF

        assert parsed.build() == data

    def test_error_frame(self):
        """错误帧（ERR=1）。"""
        raw = encode_can_id(0x001, eff=False, err=True)
        data = can_id_to_bytes(raw) + bytes([0x00])

        parsed = CANFrame.parse(data)
        assert parsed.can_id.err == 1
        assert parsed.build() == data


# ---------------------------------------------------------------------------
# bitfield 边界测试
# ---------------------------------------------------------------------------


class TestBitFieldBoundaries:
    """BitStruct 字段宽度边界条件。"""

    @pytest.mark.parametrize(
        "identifier",
        [0x000, 0x001, 0x7FE, 0x7FF],  # 11-bit 边界
        ids=["min", "lo", "hi-1", "max"],
    )
    def test_standard_id_boundaries(self, identifier):
        """11-bit 标准 ID 边界值（0x000 / 0x7FF）。"""
        raw = encode_can_id(identifier, eff=False)
        data = can_id_to_bytes(raw) + bytes([0x00])
        parsed = CANFrame.parse(data)
        assert decode_identifier(parsed.can_id) == identifier
        assert parsed.build() == data

    @pytest.mark.parametrize(
        "identifier",
        [0x00000, 0x00001, (1 << 29) - 2, (1 << 29) - 1],  # 29-bit 边界
        ids=["min", "lo", "hi-1", "max"],
    )
    def test_extended_id_boundaries(self, identifier):
        """29-bit 扩展 ID 边界值。"""
        raw = encode_can_id(identifier, eff=True)
        data = can_id_to_bytes(raw) + bytes([0x00])
        parsed = CANFrame.parse(data)
        assert decode_identifier(parsed.can_id) == identifier
        assert parsed.build() == data

    def test_dlc_zero_to_eight(self):
        """DLC 0-8 边界（CAN 规范限制）。"""
        for dlc in range(0, 9):
            raw = encode_can_id(0x100, eff=False)
            payload = bytes([0xAA] * dlc)
            data = can_id_to_bytes(raw) + bytes([dlc]) + payload
            parsed = CANFrame.parse(data)
            assert parsed.dlc == dlc
            assert parsed.data == payload
            assert parsed.build() == data


# ---------------------------------------------------------------------------
# Parity vs Python construct 2.10.70
# ---------------------------------------------------------------------------


class TestCANParity:
    """parity vs Python construct 2.10.70（子进程隔离）。

    两端使用相同的 BitStruct 32-bit ID + 标志布局，parse/build 应产生
    完全一致的字节和字段值。
    """

    @pytest.mark.parametrize(
        "raw_id_hex,dlc,payload_hex",
        [
            ("00000123", 0, ""),  # 标准帧 ID=0x123
            ("00000456", 4, "deadbeef"),  # 标准帧 + 数据
            ("20012345", 8, "0102030405060708"),  # 扩展帧 ID=0x12345, eff=1
        ],
    )
    def test_parse_parity(self, raw_id_hex, dlc, payload_hex):
        """parse 同一字节流，rs 与 py 字段值一致。"""
        from ._system_helpers import run_parity_snippet

        data_hex = raw_id_hex + format(dlc, "02x") + payload_hex

        rs_code = f"""
import json
from dataclasses import dataclass, asdict
from neoconstruct import (
    StructMixin, BitStructMixin, field, Bit, BitsInteger, Bytes, Int8ub,
)

@dataclass
class CANID(BitStructMixin):
    err: int = field(Bit())
    rtr: int = field(Bit())
    eff: int = field(Bit())
    ext_id_low: int = field(BitsInteger(18))
    std_id: int = field(BitsInteger(11))

@dataclass
class F(StructMixin):
    can_id: CANID = field(CANID)
    dlc: int = field(Int8ub)
    data: bytes = field(Bytes(dlc))

p = F.parse(bytes.fromhex('{data_hex}'))
result = {{
    'can_id': asdict(p.can_id),
    'dlc': p.dlc,
    'data': p.data.hex(),
}}
print(json.dumps(result))
"""
        py_code = f"""
import json
import construct as pc

CANID = pc.BitStruct(
    'err' / pc.Bit,
    'rtr' / pc.Bit,
    'eff' / pc.Bit,
    'ext_id_low' / pc.BitsInteger(18),
    'std_id' / pc.BitsInteger(11),
)
F = pc.Struct(
    'can_id' / CANID,
    'dlc' / pc.Int8ub,
    'data' / pc.Bytes(pc.this.dlc),
)

p = F.parse(bytes.fromhex('{data_hex}'))
def _strip(d):
    return {{k: v for k, v in dict(d).items() if not str(k).startswith('_')}}
result = {{
    'can_id': _strip(p.can_id),
    'dlc': p.dlc,
    'data': p.data.hex(),
}}
print(json.dumps(result))
"""
        rs_result = run_parity_snippet("rs", rs_code, py_code)
        py_result = run_parity_snippet("py", rs_code, py_code)
        assert rs_result == py_result, (
            f"CAN parse parity mismatch\n"
            f"  data: {data_hex}\n"
            f"  rs: {rs_result}\n"
            f"  py: {py_result}"
        )

    def test_build_parity(self):
        """build 相同字段值 → 字节一致。"""
        from ._system_helpers import run_parity_snippet

        # 标准帧 ID=0x123, DLC=3, data=aabbcc
        # can_id raw bytes: 00 00 01 23（big-endian）, dlc=03, data=aabbcc
        test_data = bytes.fromhex("00000123" + "03" + "aabbcc")

        rs_code = f"""
import json
from dataclasses import dataclass
from neoconstruct import (
    StructMixin, BitStructMixin, field, Bit, BitsInteger, Bytes, Int8ub,
)

@dataclass
class CANID(BitStructMixin):
    err: int = field(Bit())
    rtr: int = field(Bit())
    eff: int = field(Bit())
    ext_id_low: int = field(BitsInteger(18))
    std_id: int = field(BitsInteger(11))

@dataclass
class F(StructMixin):
    can_id: CANID = field(CANID)
    dlc: int = field(Int8ub)
    data: bytes = field(Bytes(dlc))

p = F.parse(bytes.fromhex('{test_data.hex()}'))
b = p.build()
print(json.dumps({{'built': b.hex()}}))
"""
        py_code = f"""
import json
import construct as pc

CANID = pc.BitStruct(
    'err' / pc.Bit,
    'rtr' / pc.Bit,
    'eff' / pc.Bit,
    'ext_id_low' / pc.BitsInteger(18),
    'std_id' / pc.BitsInteger(11),
)
F = pc.Struct(
    'can_id' / CANID,
    'dlc' / pc.Int8ub,
    'data' / pc.Bytes(pc.this.dlc),
)

p = F.parse(bytes.fromhex('{test_data.hex()}'))
b = F.build(p)
print(json.dumps({{'built': b.hex()}}))
"""
        rs_result = run_parity_snippet("rs", rs_code, py_code)
        py_result = run_parity_snippet("py", rs_code, py_code)
        assert rs_result["built"] == py_result["built"], (
            f"CAN build parity mismatch\n"
            f"  rs: {rs_result['built']}\n"
            f"  py: {py_result['built']}"
        )
        assert rs_result["built"] == test_data.hex()
