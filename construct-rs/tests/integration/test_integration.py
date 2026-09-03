"""表达式集成测试：4 个 benchmark 场景的 parse/build 往返验证。

覆盖场景：
    - E1: 字段引用 + 表达式长度（``Bytes(count)``）
    - E2: 算术表达式（``Bytes(count + flag)``）
    - E3: Tell + Computed（流位置记录 + 表达式计算字段）
    - E4: 三种 mode 混合（RO / RW / WO）

每个场景测试：
    - parse 正确性（字段值）
    - build 正确性（字节输出）
    - parse → build → parse 往返一致性
    - 多组数据（不同 count/length 值）
"""

import pytest

from construct import (
    Bytes,
    Int8ub,
    Int16ub,
    StructMixin,
    Tell,
    Computed,
    field,
    rfield,
    wfield,
)
from dataclasses import dataclass


# ---------------------------------------------------------------------------
# 场景 E1：字段引用 + 表达式长度
# ---------------------------------------------------------------------------


@dataclass
class Packet1(StructMixin):
    """E1: ``count`` 字段引用 + ``Bytes(count)`` 表达式长度。"""

    count: int = field(Int8ub)
    data: bytes = field(Bytes(count))


class TestScenarioE1:
    """场景 E1：字段引用 + 表达式长度。"""

    def test_parse_correctness(self):
        """parse ``b'\\x04ABCD'`` → count=4, data=b'ABCD'。"""
        parsed = Packet1.parse(b"\x04ABCD")
        assert parsed.count == 4
        assert parsed.data == b"ABCD"

    def test_build_correctness(self):
        """build count=4, data=b'ABCD' → ``b'\\x04ABCD'``。"""
        msg = Packet1(count=4, data=b"ABCD")
        assert msg.build() == b"\x04ABCD"

    def test_round_trip(self):
        """parse → build → parse 往返一致。"""
        original = b"\x05hello"
        parsed = Packet1.parse(original)
        rebuilt = parsed.build()
        assert rebuilt == original
        re_parsed = Packet1.parse(rebuilt)
        assert re_parsed.count == parsed.count
        assert re_parsed.data == parsed.data

    @pytest.mark.parametrize(
        "count,payload",
        [
            (0, b""),
            (1, b"A"),
            (3, b"XYZ"),
            (10, b"0123456789"),
            (255, b"Z" * 255),
        ],
    )
    def test_multiple_data_values(self, count, payload):
        """多组数据：不同 count 值的 parse/build 往返。"""
        data = bytes([count]) + payload
        parsed = Packet1.parse(data)
        assert parsed.count == count
        assert parsed.data == payload
        assert parsed.build() == data


# ---------------------------------------------------------------------------
# 场景 E2：算术表达式
# ---------------------------------------------------------------------------


@dataclass
class Packet2(StructMixin):
    """E2: 算术表达式 ``Bytes(count + flag)``。"""

    count: int = field(Int8ub)
    flag: int = field(Int8ub)
    data: bytes = field(Bytes(count + flag))


class TestScenarioE2:
    """场景 E2：算术表达式。"""

    def test_parse_correctness(self):
        """parse ``b'\\x02\\x02ABCD'`` → count=2, flag=2, data=b'ABCD'。"""
        parsed = Packet2.parse(b"\x02\x02ABCD")
        assert parsed.count == 2
        assert parsed.flag == 2
        assert parsed.data == b"ABCD"

    def test_build_correctness(self):
        """build count=2, flag=2, data=b'ABCD' → ``b'\\x02\\x02ABCD'``。"""
        msg = Packet2(count=2, flag=2, data=b"ABCD")
        assert msg.build() == b"\x02\x02ABCD"

    def test_round_trip(self):
        """parse → build → parse 往返一致。"""
        # count=3, flag=1 → count+flag=4, data=4 bytes
        original = b"\x03\x01WXYZ"
        parsed = Packet2.parse(original)
        assert parsed.count == 3
        assert parsed.flag == 1
        assert parsed.data == b"WXYZ"
        rebuilt = parsed.build()
        assert rebuilt == original

    @pytest.mark.parametrize(
        "count,flag",
        [
            (0, 0),
            (1, 0),
            (0, 1),
            (2, 3),
            (5, 5),
        ],
    )
    def test_multiple_data_values(self, count, flag):
        """多组数据：不同 count/flag 组合的 parse/build 往返。"""
        length = count + flag
        payload = bytes(range(length))
        data = bytes([count, flag]) + payload
        parsed = Packet2.parse(data)
        assert parsed.count == count
        assert parsed.flag == flag
        assert parsed.data == payload
        assert parsed.build() == data


# ---------------------------------------------------------------------------
# 场景 E3：Tell + Computed
# ---------------------------------------------------------------------------


@dataclass
class Packet3(StructMixin):
    """E3: Tell + Computed 组合。"""

    start: int = rfield(Tell())
    header: bytes = field(Bytes(4))
    length: int = field(Int16ub)
    payload: bytes = field(Bytes(length))
    end: int = rfield(Tell())
    actual: int = rfield(Computed(end - start))


class TestScenarioE3:
    """场景 E3：Tell + Computed。"""

    def test_parse_correctness(self):
        """parse → start=0, header=4字节, length=N, payload=N字节, end=6+N, actual=6+N。"""
        # header=ABCD(4), length=0x0003(2 bytes big-endian=3), payload=XYZ(3)
        data = b"ABCD\x00\x03XYZ"
        parsed = Packet3.parse(data)
        assert parsed.start == 0
        assert parsed.header == b"ABCD"
        assert parsed.length == 3
        assert parsed.payload == b"XYZ"
        assert parsed.end == 9  # 0 + 4 (header) + 2 (length) + 3 (payload)
        assert parsed.actual == 9  # end - start = 9 - 0

    def test_build_correctness(self):
        """build（RO 字段不从实例取值）→ 字节一致。"""
        msg = Packet3(header=b"ABCD", length=3, payload=b"XYZ")
        built = msg.build()
        assert built == b"ABCD\x00\x03XYZ"

    def test_round_trip(self):
        """parse → build → parse 往返一致。"""
        original = b"WXYZ\x00\x05hello"
        parsed = Packet3.parse(original)
        assert parsed.start == 0
        assert parsed.header == b"WXYZ"
        assert parsed.length == 5
        assert parsed.payload == b"hello"
        assert parsed.end == 11
        assert parsed.actual == 11
        rebuilt = parsed.build()
        assert rebuilt == original

    @pytest.mark.parametrize(
        "header_bytes,length_val,payload_bytes",
        [
            (b"AAAA", 0, b""),
            (b" HDR", 1, b"\x01"),
            (b"HEAD", 4, b"DATA"),
            (b"LONG", 10, b"0123456789"),
        ],
    )
    def test_multiple_data_values(self, header_bytes, length_val, payload_bytes):
        """多组数据：不同 length 值的 parse/build 往返。"""
        data = (
            header_bytes
            + length_val.to_bytes(2, "big")
            + payload_bytes
        )
        parsed = Packet3.parse(data)
        assert parsed.header == header_bytes
        assert parsed.length == length_val
        assert parsed.payload == payload_bytes
        assert parsed.actual == len(data)
        rebuilt = parsed.build()
        assert rebuilt == data


# ---------------------------------------------------------------------------
# 场景 E4：三种 mode 混合
# ---------------------------------------------------------------------------


@dataclass
class Packet4(StructMixin):
    """E4: RO / RW / WO 三种 mode 混合。

    字段布局（字节顺序 = 声明顺序）::

        start    = Tell()           → RO，0 字节
        count    = Int8ub           → RW，1 字节
        data     = Bytes(count)     → RW，count 字节
        reserved = Int8ub           → WO，1 字节（padding）
        end      = Tell()           → RO，0 字节
        size     = Computed(end-start) → RO，0 字节
    """

    start: int = rfield(Tell())
    count: int = field(Int8ub)
    data: bytes = field(Bytes(count))
    reserved: int = wfield(Int8ub, default=0)
    end: int = rfield(Tell())
    size: int = rfield(Computed(end - start))


class TestScenarioE4:
    """场景 E4：三种 mode 混合。"""

    def test_parse_correctness(self):
        """parse → start=0, count=N, data=N字节, reserved 被消费但不存入实例, end=2+N, size=2+N。"""
        # count=4, data=ABCD(4), reserved=0xFF(1 byte, consumed but discarded)
        data = b"\x04ABCD\xff"
        parsed = Packet4.parse(data)
        assert parsed.start == 0
        assert parsed.count == 4
        assert parsed.data == b"ABCD"
        assert parsed.end == 6  # 1 (count) + 4 (data) + 1 (reserved)
        assert parsed.size == 6  # end - start

    def test_parse_reserved_not_in_instance_dict(self):
        """WO 字段 parse 时被消费但不存入实例 ``__dict__``。"""
        data = b"\x04ABCD\xff"
        parsed = Packet4.parse(data)
        # reserved 不应在实例 __dict__ 中（WO 语义）
        assert "reserved" not in parsed.__dict__

    def test_build_correctness(self):
        """build → reserved 用 default=0 写入，RO 字段自动计算。"""
        msg = Packet4(count=4, data=b"ABCD")
        built = msg.build()
        # start(0) + count(\x04) + data(ABCD) + reserved(\x00) + end(0) + size(0)
        assert built == b"\x04ABCD\x00"

    def test_build_reserved_uses_default(self):
        """build 时 reserved 写入 default=0（不取实例值）。"""
        msg = Packet4(count=2, data=b"XY")
        built = msg.build()
        assert built == b"\x02XY\x00"

    def test_round_trip(self):
        """parse → build → parse 往返一致。"""
        original = b"\x03ABC\x00"
        parsed = Packet4.parse(original)
        assert parsed.count == 3
        assert parsed.data == b"ABC"
        rebuilt = parsed.build()
        assert rebuilt == original
        re_parsed = Packet4.parse(rebuilt)
        assert re_parsed.count == parsed.count
        assert re_parsed.data == parsed.data

    @pytest.mark.parametrize(
        "count,payload",
        [
            (0, b""),
            (1, b"A"),
            (5, b"HELLO"),
            (8, b"12345678"),
        ],
    )
    def test_multiple_data_values(self, count, payload):
        """多组数据：不同 count 值的 parse/build 往返。"""
        data = bytes([count]) + payload + b"\x00"
        parsed = Packet4.parse(data)
        assert parsed.count == count
        assert parsed.data == payload
        assert parsed.size == len(data)
        rebuilt = parsed.build()
        assert rebuilt == data
