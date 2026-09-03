"""examples/example.py 的端到端验证。

覆盖范围：ModbusRTUMessage 的 build 与 parse 往返一致性。
"""

from dataclasses import dataclass

from neoconstruct import GreedyBytes, Int8ub, StructMixin, field


@dataclass
class ModbusRTUMessage(StructMixin):
    """example.py 中的 Modbus RTU 消息结构。

    3 字段：地址（Int8ub）+ 功能码（Int8ub）+ 数据（GreedyBytes）。
    """

    address: int = field(Int8ub)
    function_code: int = field(Int8ub)
    data: bytes = field(GreedyBytes)


def test_build_matches_expected_bytes():
    """构建出的字节流必须与 Python construct 等价的二进制布局一致。"""
    msg = ModbusRTUMessage(address=1, function_code=3, data=b"\x00\x01\x00\x02")
    built = msg.build()
    assert built == b"\x01\x03\x00\x01\x00\x02"


def test_parse_returns_instance_with_correct_fields():
    """parse 返回 ModbusRTUMessage 实例，字段值正确。"""
    data = b"\x01\x03\x00\x01\x00\x02"
    parsed = ModbusRTUMessage.parse(data)
    assert isinstance(parsed, ModbusRTUMessage)
    assert parsed.address == 1
    assert parsed.function_code == 3
    assert parsed.data == b"\x00\x01\x00\x02"


def test_roundtrip_preserves_data():
    """build → parse → build 应保持数据一致。"""
    original = ModbusRTUMessage(address=42, function_code=0x10, data=b"hello world")
    built = original.build()
    parsed = ModbusRTUMessage.parse(built)
    rebuilt = parsed.build()
    assert rebuilt == built
    assert parsed.address == original.address
    assert parsed.function_code == original.function_code
    assert parsed.data == original.data


def test_roundtrip_with_empty_data():
    """GreedyBytes 字段允许空数据。"""
    msg = ModbusRTUMessage(address=0, function_code=0, data=b"")
    built = msg.build()
    assert built == b"\x00\x00"
    parsed = ModbusRTUMessage.parse(built)
    assert parsed.data == b""


def test_roundtrip_with_large_data():
    """GreedyBytes 字段接受较长数据。"""
    payload = bytes(range(256)) * 4  # 1024 字节
    msg = ModbusRTUMessage(address=1, function_code=2, data=payload)
    built = msg.build()
    parsed = ModbusRTUMessage.parse(built)
    assert parsed.data == payload
    assert len(parsed.data) == 1024


def test_compiled_schema_is_cached_on_class():
    """编译产物 _construct_compiled 应作为类属性缓存（零开销查找）。"""
    assert hasattr(ModbusRTUMessage, "_construct_compiled")
    assert ModbusRTUMessage._construct_compiled is not None
    # 重复访问应返回同一对象（缓存）
    s1 = ModbusRTUMessage._construct_compiled
    s2 = ModbusRTUMessage._construct_compiled
    assert s1 is s2
