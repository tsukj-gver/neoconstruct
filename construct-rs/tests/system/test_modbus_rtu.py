"""Modbus RTU 协议系统测试。

协议参考：Modbus Application Protocol Specification V1.1b3
       Modbus over Serial Line Specification V1.02

## 协议结构

Modbus RTU 帧格式::

    +---------+---------------+-----------------+----------+
    | Address | Function Code |     Data        |   CRC16  |
    | 1 byte  |    1 byte     | variable length |  2 bytes |
    +---------+---------------+-----------------+----------+

- **Address**: 从站地址（0-247），0 为广播
- **Function Code**: 1-127，决定 Data 字段的结构
- **Data**: 长度与结构由 Function Code 决定
- **CRC16**: CRC-16/MODBUS（polynomial 0xA001 bit-reversed, init 0xFFFF），
  低字节在前（little-endian）

## 覆盖能力

- **条件分支**：Switch on function_code（不同 FC 有不同的 Data 结构）
- **CRC/校验**：Checksum 构造器 + Python callable + StreamRange (Tell start/end)
- **嵌套 Struct**：每种 FC 的 Data 是独立的 StructMixin 子类
- **数组**：Write Multiple Registers 用 Array 表示寄存器列表

## 覆盖的功能码

| FC  | 名称                       | 请求数据结构                       |
|-----|----------------------------|------------------------------------|
| 0x03 | Read Holding Registers     | starting_address + quantity        |
| 0x06 | Write Single Register      | register_address + register_value  |
| 0x10 | Write Multiple Registers   | starting_address + quantity + bc + values |
| 0x01 | Read Coils                 | starting_address + quantity        |

异常响应（FC | 0x80）数据结构为 1 字节异常码，本测试略。
"""

from __future__ import annotations

from dataclasses import dataclass

import pytest

from construct import (
    Array,
    Bytes,
    Checksum,
    Int8ub,
    Int16ub,
    StructMixin,
    Switch,
    Tell,
    field,
    rfield,
)

from ._system_helpers import modbus_crc16


# ---------------------------------------------------------------------------
# 协议定义
# ---------------------------------------------------------------------------


@dataclass
class ReadHoldingRegistersReq(StructMixin):
    """FC=0x03 请求：起始地址 + 数量。

    Wire 格式（4 字节）：[starting_address_hi, _lo, quantity_hi, _lo]
    """

    starting_address: int = field(Int16ub)
    quantity: int = field(Int16ub)


@dataclass
class WriteSingleRegisterReq(StructMixin):
    """FC=0x06 请求：寄存器地址 + 寄存器值。"""

    register_address: int = field(Int16ub)
    register_value: int = field(Int16ub)


@dataclass
class WriteMultipleRegistersReq(StructMixin):
    """FC=0x10 请求：起始地址 + 数量 + 字节计数 + 寄存器值数组。

    Wire 格式：[addr_hi, _lo, qty_hi, _lo, byte_count, value_0_hi, _lo, ...]
    byte_count = quantity * 2
    """

    starting_address: int = field(Int16ub)
    quantity: int = field(Int16ub)
    byte_count: int = field(Int8ub)
    # 数组长度引用 quantity 字段（Array 接受 FieldRef count）
    values: list = field(Array(quantity, Int16ub))


@dataclass
class ModbusRTUFrame(StructMixin):
    """Modbus RTU 帧（请求方向，覆盖 FC 0x01/0x03/0x06/0x10）。

    CRC16 通过 Checksum 构造器 + Python callable + StreamRange 自动计算/校验：

    - parse：读 2 字节 → 计算 [body_start, body_end) 的 CRC16 → 比较
    - build：计算已写入字节 [body_start, body_end) 的 CRC16 → 写入

    注意：Checksum 必须用 ``field()`` (RW)，因为 Rust 端 build_ro_value 列表
    不含 ChecksumNode（详见 checksum.rs）。RW 模式下 build 时实例的 crc 值
    被忽略，由 StreamRange 重算。

    Switch on function_code：根据功能码选择 Data 解析方式。
    """

    body_start: int = rfield(Tell())
    address: int = field(Int8ub)
    function_code: int = field(Int8ub)
    payload: object = field(
        Switch(
            function_code,
            {
                0x01: ReadHoldingRegistersReq,  # Read Coils（同 0x03 结构）
                0x02: ReadHoldingRegistersReq,  # Read Discrete Inputs（同结构）
                0x03: ReadHoldingRegistersReq,  # Read Holding Registers
                0x04: ReadHoldingRegistersReq,  # Read Input Registers（同结构）
                0x05: WriteSingleRegisterReq,  # Write Single Coil（同 0x06 结构）
                0x06: WriteSingleRegisterReq,  # Write Single Register
                0x10: WriteMultipleRegistersReq,  # Write Multiple Registers
            },
        )
    )
    body_end: int = rfield(Tell())
    crc: bytes = field(
        Checksum(
            Bytes(2),
            modbus_crc16,
            start=body_start,
            end=body_end,
        )
    )


# ---------------------------------------------------------------------------
# 工具：根据 body 字节构造完整的 Modbus 帧
# ---------------------------------------------------------------------------


def build_modbus_bytes(address: int, function_code: int, body_after_fc: bytes) -> bytes:
    """构造 Modbus 帧：address + fc + body + CRC16（LE）。

    用于测试中生成预期字节。本函数仅用作 test oracle，不依赖 construct-rs。
    """
    head = bytes([address, function_code]) + body_after_fc
    return head + modbus_crc16(head)


# ---------------------------------------------------------------------------
# Round-trip 测试
# ---------------------------------------------------------------------------


class TestModbusRoundTrip:
    """每个功能码的 parse → build → parse round-trip 验证。"""

    def test_read_holding_registers_request(self):
        """FC=0x03 请求：spec 文档示例（slave=0x11, addr=0x006B, qty=0x0003）。

        Spec CRC = 0x7687（低字节 0x76，高字节 0x87）。
        """
        # 构造完整帧（spec 真实 CRC）
        data = build_modbus_bytes(
            address=0x11, function_code=0x03, body_after_fc=b"\x00\x6B\x00\x03"
        )
        assert data == bytes([0x11, 0x03, 0x00, 0x6B, 0x00, 0x03, 0x76, 0x87])

        parsed = ModbusRTUFrame.parse(data)
        assert parsed.address == 0x11
        assert parsed.function_code == 0x03
        assert isinstance(parsed.payload, ReadHoldingRegistersReq)
        assert parsed.payload.starting_address == 0x006B
        assert parsed.payload.quantity == 0x0003
        assert parsed.crc == bytes([0x76, 0x87])  # LE: low byte first

        # round-trip
        rebuilt = parsed.build()
        assert rebuilt == data

    def test_write_single_register_request(self):
        """FC=0x06 请求：写单个寄存器。"""
        data = build_modbus_bytes(
            address=0x01, function_code=0x06, body_after_fc=b"\x00\x08\x00\x0A"
        )
        # address=0x01, FC=0x06, register_address=0x0008, value=0x000A
        parsed = ModbusRTUFrame.parse(data)
        assert parsed.address == 0x01
        assert parsed.function_code == 0x06
        assert isinstance(parsed.payload, WriteSingleRegisterReq)
        assert parsed.payload.register_address == 0x0008
        assert parsed.payload.register_value == 0x000A

        rebuilt = parsed.build()
        assert rebuilt == data

    def test_write_multiple_registers_request(self):
        """FC=0x10 请求：写多个寄存器（含数组）。

        - starting_address = 0x0001
        - quantity = 0x0002
        - byte_count = 0x04
        - values = [0x000A, 0x0102]
        """
        body_after_fc = bytes(
            [0x00, 0x01, 0x00, 0x02, 0x04, 0x00, 0x0A, 0x01, 0x02]
        )
        data = build_modbus_bytes(
            address=0x10, function_code=0x10, body_after_fc=body_after_fc
        )

        parsed = ModbusRTUFrame.parse(data)
        assert parsed.address == 0x10
        assert parsed.function_code == 0x10
        assert isinstance(parsed.payload, WriteMultipleRegistersReq)
        assert parsed.payload.starting_address == 0x0001
        assert parsed.payload.quantity == 0x0002
        assert parsed.payload.byte_count == 0x04
        assert parsed.payload.values == [0x000A, 0x0102]

        rebuilt = parsed.build()
        assert rebuilt == data

    def test_read_coils_request(self):
        """FC=0x01 请求：读线圈（与 0x03 同结构，验证 Switch 多 case 共用 body）。"""
        data = build_modbus_bytes(
            address=0x05, function_code=0x01, body_after_fc=b"\x00\x13\x00\x19"
        )

        parsed = ModbusRTUFrame.parse(data)
        assert parsed.function_code == 0x01
        assert isinstance(parsed.payload, ReadHoldingRegistersReq)
        assert parsed.payload.starting_address == 0x0013
        assert parsed.payload.quantity == 0x0019

        rebuilt = parsed.build()
        assert rebuilt == data


# ---------------------------------------------------------------------------
# 复杂场景：CRC 校验 + 错误路径
# ---------------------------------------------------------------------------


class TestModbusCRCCheck:
    """Checksum 节点的 CRC 校验逻辑。"""

    def test_valid_crc_passes(self):
        """正确 CRC → parse 成功，CRC 字段返回原值。"""
        data = build_modbus_bytes(0x01, 0x03, b"\x00\x01\x00\x0A")
        parsed = ModbusRTUFrame.parse(data)
        expected_crc = modbus_crc16(data[:-2])
        assert parsed.crc == expected_crc

    def test_wrong_crc_raises_checksum_error(self):
        """错误 CRC → 抛 ChecksumError。"""
        from construct import ChecksumError

        data = bytearray(
            build_modbus_bytes(0x01, 0x03, b"\x00\x01\x00\x0A")
        )
        # 翻转最后一个字节（CRC high byte）
        data[-1] ^= 0xFF
        with pytest.raises(ChecksumError):
            ModbusRTUFrame.parse(bytes(data))


# ---------------------------------------------------------------------------
# 复杂场景：分支覆盖
# ---------------------------------------------------------------------------


class TestModbusSwitchCoverage:
    """Switch 各 case 覆盖。"""

    @pytest.mark.parametrize(
        "fc,body_after_fc,payload_type",
        [
            (0x01, b"\x00\x01\x00\x0A", ReadHoldingRegistersReq),
            (0x02, b"\x00\x01\x00\x0A", ReadHoldingRegistersReq),
            (0x03, b"\x00\x01\x00\x0A", ReadHoldingRegistersReq),
            (0x04, b"\x00\x01\x00\x0A", ReadHoldingRegistersReq),
            (0x05, b"\x00\x08\x00\x01", WriteSingleRegisterReq),
            (0x06, b"\x00\x08\xFF\x00", WriteSingleRegisterReq),
        ],
    )
    def test_switch_dispatches_to_correct_body(
        self, fc, body_after_fc, payload_type
    ):
        """不同功能码应分派到对应 body 类型。"""
        data = build_modbus_bytes(0x01, fc, body_after_fc)
        parsed = ModbusRTUFrame.parse(data)
        assert parsed.function_code == fc
        assert isinstance(parsed.payload, payload_type)
        assert parsed.build() == data


# ---------------------------------------------------------------------------
# Parity vs Python construct 2.10.70
# ---------------------------------------------------------------------------


# Python construct 实现（用于 parity 对比）
_PY_CONSTRUCT_IMPL = """
import construct as pc

# Python construct Modbus 实现：lambda + RawCopy（原版典型用法）
ReadHoldingRegistersReq = pc.Struct(
    'starting_address' / pc.Int16ub,
    'quantity' / pc.Int16ub,
)
WriteSingleRegisterReq = pc.Struct(
    'register_address' / pc.Int16ub,
    'register_value' / pc.Int16ub,
)
WriteMultipleRegistersReq = pc.Struct(
    'starting_address' / pc.Int16ub,
    'quantity' / pc.Int16ub,
    'byte_count' / pc.Int8ub,
    'values' / pc.Array(pc.this.quantity, pc.Int16ub),
)

def _crc16(data):
    crc = 0xFFFF
    for byte in data:
        crc ^= byte
        for _ in range(8):
            if crc & 1:
                crc = (crc >> 1) ^ 0xA001
            else:
                crc >>= 1
    return bytes([crc & 0xFF, (crc >> 8) & 0xFF])

ModbusRTUFrame = pc.Struct(
    'body_start' / pc.Tell,
    'address' / pc.Int8ub,
    'function_code' / pc.Int8ub,
    'payload' / pc.Switch(pc.this.function_code, {
        0x01: ReadHoldingRegistersReq,
        0x02: ReadHoldingRegistersReq,
        0x03: ReadHoldingRegistersReq,
        0x04: ReadHoldingRegistersReq,
        0x05: WriteSingleRegisterReq,
        0x06: WriteSingleRegisterReq,
        0x10: WriteMultipleRegistersReq,
    }),
    'body_end' / pc.Tell,
    'crc' / pc.Checksum(pc.Bytes(2), _crc16, pc.this.data[:-2]),
)
"""


class TestModbusParity:
    """parity vs Python construct 2.10.70（子进程隔离）。

    construct-rs 的 Checksum(StreamRange) 与 Python 原版的 Checksum(bytesfunc=RawCopy.data)
    路径不同但应产生相同的 parse/build 结果。

    注：Python 原版 Checksum 的 bytesfunc 接收 context，``this.data[:-2]`` 引用
    RawCopy 捕获的 raw bytes（剥掉末 2 字节 CRC）。本测试对比核心字段：
    address / function_code / payload.* / built bytes。
    """

    @pytest.mark.parametrize(
        "address,fc,body_after_fc",
        [
            (0x11, 0x03, b"\x00\x6B\x00\x03"),  # spec 示例
            (0x01, 0x06, b"\x00\x08\x00\x0A"),
            (0x10, 0x10, b"\x00\x01\x00\x02\x04\x00\x0A\x01\x02"),
        ],
    )
    def test_parse_parity(self, address, fc, body_after_fc):
        """parse 结果在 rs / py 两端一致（核心字段，过滤内部状态如 _io）。"""
        from ._system_helpers import run_parity_snippet

        data_hex = build_modbus_bytes(address, fc, body_after_fc).hex()

        # 仅对比 address / function_code / payload 字段
        rs_code = f"""
import json
from dataclasses import dataclass, asdict
from construct import (
    StructMixin, field, Int8ub, Int16ub, Array, Switch,
)

@dataclass
class ReadReq(StructMixin):
    starting_address: int = field(Int16ub)
    quantity: int = field(Int16ub)

@dataclass
class WriteSingle(StructMixin):
    register_address: int = field(Int16ub)
    register_value: int = field(Int16ub)

@dataclass
class WriteMulti(StructMixin):
    starting_address: int = field(Int16ub)
    quantity: int = field(Int16ub)
    byte_count: int = field(Int8ub)
    values: list = field(Array(quantity, Int16ub))

@dataclass
class M(StructMixin):
    address: int = field(Int8ub)
    function_code: int = field(Int8ub)
    payload: object = field(Switch(function_code, {{
        0x01: ReadReq, 0x02: ReadReq, 0x03: ReadReq, 0x04: ReadReq,
        0x05: WriteSingle, 0x06: WriteSingle, 0x10: WriteMulti,
    }}))

p = M.parse(bytes.fromhex('{data_hex}'))
result = {{
    'address': p.address,
    'function_code': p.function_code,
    'payload': asdict(p.payload) if hasattr(p.payload, '__dataclass_fields__') else None,
}}
print(json.dumps(result))
"""
        py_code = f"""
import json
import construct as pc

ReadReq = pc.Struct('starting_address' / pc.Int16ub, 'quantity' / pc.Int16ub)
WriteSingle = pc.Struct('register_address' / pc.Int16ub, 'register_value' / pc.Int16ub)
WriteMulti = pc.Struct(
    'starting_address' / pc.Int16ub,
    'quantity' / pc.Int16ub,
    'byte_count' / pc.Int8ub,
    'values' / pc.Array(pc.this.quantity, pc.Int16ub),
)
M = pc.Struct(
    'address' / pc.Int8ub,
    'function_code' / pc.Int8ub,
    'payload' / pc.Switch(pc.this.function_code, {{
        0x01: ReadReq, 0x02: ReadReq, 0x03: ReadReq, 0x04: ReadReq,
        0x05: WriteSingle, 0x06: WriteSingle, 0x10: WriteMulti,
    }}),
)

p = M.parse(bytes.fromhex('{data_hex}'))
# Python construct 的 Container 含 _io 等内部状态字段，需过滤 _ 开头键
def _strip(d):
    return {{k: v for k, v in dict(d).items() if not str(k).startswith('_')}}
result = {{
    'address': p.address,
    'function_code': p.function_code,
    'payload': _strip(p.payload) if hasattr(p.payload, 'items') else None,
}}
print(json.dumps(result))
"""
        rs_result = run_parity_snippet("rs", rs_code, py_code)
        py_result = run_parity_snippet("py", rs_code, py_code)
        assert rs_result == py_result, (
            f"parity mismatch for fc={fc:#x}\n"
            f"  rs: {rs_result}\n"
            f"  py: {py_result}"
        )

    def test_build_parity(self):
        """build 相同 payload → 字节一致（含 CRC）。

        这是最强的 parity 测试：两端实现完全独立的 CRC 计算，
        若 build 字节一致则证明两端逻辑等价。
        """
        from ._system_helpers import run_parity_snippet

        # 使用 spec 示例的真实帧
        test_data = bytes([0x11, 0x03, 0x00, 0x6B, 0x00, 0x03, 0x76, 0x87])

        rs_code = f"""
import json
from dataclasses import dataclass
from construct import (
    StructMixin, field, rfield, Bytes, Tell, Checksum, Int8ub, Int16ub,
    Array, Switch,
)

def crc16(data):
    crc = 0xFFFF
    for byte in data:
        crc ^= byte
        for _ in range(8):
            if crc & 1:
                crc = (crc >> 1) ^ 0xA001
            else:
                crc >>= 1
    return bytes([crc & 0xFF, (crc >> 8) & 0xFF])

@dataclass
class ReadReq(StructMixin):
    starting_address: int = field(Int16ub)
    quantity: int = field(Int16ub)

@dataclass
class M(StructMixin):
    body_start: int = rfield(Tell())
    address: int = field(Int8ub)
    function_code: int = field(Int8ub)
    payload: object = field(Switch(function_code, {{
        0x03: ReadReq,
    }}))
    body_end: int = rfield(Tell())
    crc: bytes = field(Checksum(Bytes(2), crc16, start=body_start, end=body_end))

p = M.parse(bytes.fromhex('{test_data.hex()}'))
b = p.build()
print(json.dumps({{'built': b.hex()}}))
"""
        py_code = f"""
import json
import construct as pc

ReadReq = pc.Struct('starting_address' / pc.Int16ub, 'quantity' / pc.Int16ub)

def crc16(data):
    crc = 0xFFFF
    for byte in data:
        crc ^= byte
        for _ in range(8):
            if crc & 1:
                crc = (crc >> 1) ^ 0xA001
            else:
                crc >>= 1
    return bytes([crc & 0xFF, (crc >> 8) & 0xFF])

# Python 原版 Checksum 的 bytesfunc 接收 context，引用 this._io 或 RawCopy dict.data
# 此处用最简单的形式：checksum 在帧末尾，bytesfunc 引用整个 raw bytes（剥末 2 字节）
M = pc.Struct(
    'body' / pc.RawCopy(pc.Struct(
        'address' / pc.Int8ub,
        'function_code' / pc.Int8ub,
        'payload' / pc.Switch(pc.this.function_code, {{
            0x03: ReadReq,
        }}),
    )),
    'crc' / pc.Checksum(pc.Bytes(2), crc16, pc.this.body.data),
)

p = M.parse(bytes.fromhex('{test_data.hex()}'))
b = M.build(p)
print(json.dumps({{'built': b.hex()}}))
"""
        rs_result = run_parity_snippet("rs", rs_code, py_code)
        py_result = run_parity_snippet("py", rs_code, py_code)
        assert rs_result["built"] == py_result["built"], (
            f"build parity mismatch\n"
            f"  rs built: {rs_result['built']}\n"
            f"  py built: {py_result['built']}"
        )
        assert rs_result["built"] == test_data.hex()
