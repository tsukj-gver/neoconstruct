"""Bytes 表达式长度端到端测试。

覆盖：
- ``field(Bytes(count))``：简单字段引用
- ``field(Bytes(count + 1))``：算术表达式
- ``field(Bytes(count * 2))``：乘法表达式
- ``field(Bytes(4))``：常量长度（向后兼容）
- ``field(Bytes(0))``：零长度
- round-trip：parse → build 可还原
- 错误：WO 字段引用 → CompilationError
- 错误：前向引用 → CompilationError
"""

import struct as pystruct

import pytest

from neoconstruct import (
    Bytes,
    CompilationError,
    Int8ub,
    StructMixin,
    field,
    rfield,
    wfield,
)
from dataclasses import dataclass


# ---------------------------------------------------------------------------
# 简单字段引用：Bytes(count)
# ---------------------------------------------------------------------------


class TestSimpleFieldReference:
    """``Bytes(count)`` — 简单字段引用。"""

    def test_parse_reads_field_value_bytes(self):
        """count=3 → data 读取 3 字节。"""

        @dataclass
        class Msg(StructMixin):
            count: int = field(Int8ub)
            data: bytes = field(Bytes(count))

        parsed = Msg.parse(b"\x03ABCDEF")
        assert parsed.count == 3
        assert parsed.data == b"ABC"

    def test_parse_count_zero_reads_empty_bytes(self):
        """count=0 → data 读取 0 字节。"""

        @dataclass
        class Msg(StructMixin):
            count: int = field(Int8ub)
            data: bytes = field(Bytes(count))

        parsed = Msg.parse(b"\x00ABCDEF")
        assert parsed.count == 0
        assert parsed.data == b""

    def test_parse_count_max_u8(self):
        """count=255 → data 读取 255 字节。"""

        @dataclass
        class Msg(StructMixin):
            count: int = field(Int8ub)
            data: bytes = field(Bytes(count))

        payload = b"\xff" + b"X" * 255
        parsed = Msg.parse(payload)
        assert parsed.count == 255
        assert parsed.data == b"X" * 255

    def test_build_writes_field_value_bytes(self):
        """build：count=3, data=b'ABC' → \\x03ABC。"""

        @dataclass
        class Msg(StructMixin):
            count: int = field(Int8ub)
            data: bytes = field(Bytes(count))

        msg = Msg(count=3, data=b"ABC")
        built = msg.build()
        assert built == b"\x03ABC"

    def test_round_trip_preserves_data(self):
        """parse → build round-trip 可还原。"""

        @dataclass
        class Msg(StructMixin):
            count: int = field(Int8ub)
            data: bytes = field(Bytes(count))

        original = b"\x05hello"
        parsed = Msg.parse(original)
        rebuilt = parsed.build()
        assert rebuilt == original


# ---------------------------------------------------------------------------
# 算术表达式：Bytes(count + N)
# ---------------------------------------------------------------------------


class TestArithmeticExpression:
    """``Bytes(count + 1)`` — 算术加法表达式。"""

    def test_parse_addition(self):
        """count=3 → data 读取 4 字节。"""

        @dataclass
        class Msg(StructMixin):
            count: int = field(Int8ub)
            data: bytes = field(Bytes(count + 1))

        parsed = Msg.parse(b"\x03ABCD")
        assert parsed.count == 3
        assert parsed.data == b"ABCD"

    def test_build_addition(self):
        """build：count=2, data=b'ABC' → \\x02ABC。"""

        @dataclass
        class Msg(StructMixin):
            count: int = field(Int8ub)
            data: bytes = field(Bytes(count + 1))

        msg = Msg(count=2, data=b"ABC")
        built = msg.build()
        assert built == b"\x02ABC"

    def test_round_trip_addition(self):
        """round-trip：加法表达式。"""

        @dataclass
        class Msg(StructMixin):
            count: int = field(Int8ub)
            data: bytes = field(Bytes(count + 2))

        original = b"\x03XXXXX"
        parsed = Msg.parse(original)
        assert parsed.count == 3
        assert parsed.data == b"XXXXX"
        rebuilt = parsed.build()
        assert rebuilt == original


# ---------------------------------------------------------------------------
# 乘法表达式：Bytes(count * N)
# ---------------------------------------------------------------------------


class TestMultiplicationExpression:
    """``Bytes(count * 2)`` — 乘法表达式。"""

    def test_parse_multiplication(self):
        """count=3 → data 读取 6 字节。"""

        @dataclass
        class Msg(StructMixin):
            count: int = field(Int8ub)
            data: bytes = field(Bytes(count * 2))

        parsed = Msg.parse(b"\x03ABCDEF")
        assert parsed.count == 3
        assert parsed.data == b"ABCDEF"

    def test_round_trip_multiplication(self):
        """round-trip：乘法表达式。"""

        @dataclass
        class Msg(StructMixin):
            count: int = field(Int8ub)
            data: bytes = field(Bytes(count * 2))

        original = b"\x04ABCDEFGH"
        parsed = Msg.parse(original)
        rebuilt = parsed.build()
        assert rebuilt == original


# ---------------------------------------------------------------------------
# 常量长度（向后兼容）
# ---------------------------------------------------------------------------


class TestConstBackwardCompat:
    """``Bytes(4)`` — 常量长度（早期用法兼容）。"""

    def test_parse_const_length(self):
        """常量长度 4 → 读取 4 字节。"""

        @dataclass
        class Msg(StructMixin):
            data: bytes = field(Bytes(4))

        parsed = Msg.parse(b"ABCD")
        assert parsed.data == b"ABCD"

    def test_build_const_length(self):
        """build：常量长度 4。"""

        @dataclass
        class Msg(StructMixin):
            data: bytes = field(Bytes(4))

        msg = Msg(data=b"ABCD")
        built = msg.build()
        assert built == b"ABCD"

    def test_round_trip_const(self):
        """round-trip：常量长度。"""

        @dataclass
        class Msg(StructMixin):
            data: bytes = field(Bytes(4))

        original = b"WXYZ"
        parsed = Msg.parse(original)
        rebuilt = parsed.build()
        assert rebuilt == original


# ---------------------------------------------------------------------------
# 三字段表达式
# ---------------------------------------------------------------------------


class TestMultiFieldExpression:
    """多字段 Struct 中 Bytes 使用前序字段引用。"""

    def test_three_field_struct(self):
        """header + count + data(Bytes(count))。"""

        @dataclass
        class Packet(StructMixin):
            header: int = field(Int8ub)
            count: int = field(Int8ub)
            data: bytes = field(Bytes(count))

        parsed = Packet.parse(b"\xaa\x03XYZ")
        assert parsed.header == 0xAA
        assert parsed.count == 3
        assert parsed.data == b"XYZ"

    def test_two_expression_fields(self):
        """两个表达式字段引用同一个前序字段。"""

        @dataclass
        class Dual(StructMixin):
            count: int = field(Int8ub)
            data1: bytes = field(Bytes(count))
            data2: bytes = field(Bytes(count))

        parsed = Dual.parse(b"\x02ABCD")
        assert parsed.count == 2
        assert parsed.data1 == b"AB"
        assert parsed.data2 == b"CD"


# ---------------------------------------------------------------------------
# 错误路径
# ---------------------------------------------------------------------------


class TestErrorCases:
    """编译期错误检测。"""

    def test_wo_field_reference_raises(self):
        """表达式引用 WO 字段 → CompilationError。"""

        with pytest.raises(CompilationError) as exc_info:

            @dataclass
            class Bad(StructMixin):
                count: int = wfield(Int8ub)
                data: bytes = field(Bytes(count))

        assert "WO" in str(exc_info.value)

    def test_forward_reference_raises(self):
        """前向引用（引用后序字段）→ CompilationError。

        通过提前创建 field 对象并放在后序位置来模拟前向引用：
        count_field 在 data 之后定义，但 data 的表达式引用了它。
        """

        count_field = field(Int8ub)

        with pytest.raises(CompilationError) as exc_info:

            @dataclass
            class Bad(StructMixin):
                data: bytes = field(Bytes(count_field))
                count: int = count_field

        assert "后序" in str(exc_info.value) or "forward" in str(exc_info.value).lower()


# ---------------------------------------------------------------------------
# RO 字段 + Bytes 表达式
# ---------------------------------------------------------------------------


class TestRoFieldExpression:
    """RO 字段可以被表达式引用。"""

    def test_ro_field_reference_ok(self):
        """引用 RO 字段 → 正常编译和解析。"""

        @dataclass
        class Msg(StructMixin):
            count: int = rfield(Int8ub)
            data: bytes = field(Bytes(count))

        parsed = Msg.parse(b"\x04ABCD")
        assert parsed.count == 4
        assert parsed.data == b"ABCD"
