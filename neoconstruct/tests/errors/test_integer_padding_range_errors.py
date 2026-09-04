"""整数位域 / 填充 / 范围类异常用户面行为锁定：IntegerError / PaddingError / RangeError。

期望值来源标注：[文档]=异常类 docstring 契约；[自然]=语义自然性；
[基线]=实测绿灯行为锚定。
"""

import pytest
from dataclasses import dataclass
from typing import Any

from neoconstruct import (
    Array,
    BitStructMixin,
    BitsInteger,
    CompilationError,
    IntegerError,
    Int8ub,
    Padding,
    PaddingError,
    RangeError,
    StructMixin,
    field,
)


# ---------------------------------------------------------------------------
# IntegerError：BitsInteger 解析/构建失败
# ---------------------------------------------------------------------------


def test_bits_integer_out_of_range_raises_integer_error():
    """BitsInteger(8) build 传 256（超 8 位无符号范围）→ IntegerError。[文档]

    双重断言：类型 IntegerError；path 含字段名；消息含范围与实际值。
    """

    @dataclass
    class B(BitStructMixin):
        v: int = field(BitsInteger(8))

    with pytest.raises(IntegerError) as exc_info:
        B(v=256).build()

    assert "v" in (exc_info.value.path or "")
    assert "256" in str(exc_info.value)


def test_bits_integer_wrong_type_raises_integer_error():
    """BitsInteger build 传非 int 类型 → IntegerError。[文档]

    双重断言：类型；消息含实际类型名 str。
    """

    @dataclass
    class B(BitStructMixin):
        v: int = field(BitsInteger(8))

    with pytest.raises(IntegerError) as exc_info:
        B(v="zzz").build()  # type: ignore[arg-type]

    assert "str" in str(exc_info.value)


def test_bits_integer_valid_value_roundtrip():
    """合法值通过并 roundtrip（错误类旁路）。[自然]

    双重断言：parse 值；build 字节。
    """

    @dataclass
    class B(BitStructMixin):
        v: int = field(BitsInteger(8))

    assert B.parse(b"\x0a").v == 10
    assert B.build(B(v=10)) == b"\x0a"


# ---------------------------------------------------------------------------
# PaddingError：填充 pattern 非法
# ---------------------------------------------------------------------------


def test_bit_padding_invalid_pattern_raises_padding_error():
    """bit 域 Padding pattern=2（仅接受 0x00/0x01）→ 编译期 PaddingError。[文档]

    双重断言：类型 PaddingError；消息含 pattern 语义词；
    path 携带出错字段名上下文（编译期注入，非空串）。
    """

    with pytest.raises(PaddingError) as exc_info:

        @dataclass
        class B(BitStructMixin):
            a: int = field(BitsInteger(4))
            pad: Any = field(Padding(4, pattern=2))

    assert "pattern" in str(exc_info.value).lower() or "padding" in str(exc_info.value).lower()
    assert "pad" in (exc_info.value.path or "")


def test_bit_padding_valid_pattern_compiles_and_roundtrip():
    """合法 pattern（0/1）Padding 编译通过且 roundtrip。[自然]

    pattern=1 时填充位写 1：a=0xA → 字节 0xAF；parse 剥离填充仍得 0xA。
    """

    @dataclass
    class B(BitStructMixin):
        a: int = field(BitsInteger(4))
        pad: Any = field(Padding(4, pattern=1))

    assert B.parse(b"\xa0").a == 0xA
    assert B.parse(b"\xaf").a == 0xA
    assert B.build(B(a=0xA)) == b"\xaf"


# ---------------------------------------------------------------------------
# RangeError：Array count 与列表长度不符
# ---------------------------------------------------------------------------


def test_array_build_count_mismatch_raises_range_error():
    """Array(3, x) build 传 2 元素 → RangeError。[文档]

    双重断言：类型 RangeError；path 含字段名；消息含 expected 3 / found 2。
    """

    @dataclass
    class P(StructMixin):
        items: list = field(Array(3, Int8ub))

    with pytest.raises(RangeError) as exc_info:
        P(items=[1, 2]).build()

    assert "items" in (exc_info.value.path or "")
    msg = str(exc_info.value)
    assert "3" in msg and "2" in msg


def test_array_negative_count_rejected_at_compile_time():
    """Array(-1, x) 负 count → 编译期 CompilationError（先于运行时）。[文档]

    负 count 是声明错误而非数据错误，编译期即拒绝。
    """

    with pytest.raises(CompilationError):

        @dataclass
        class P(StructMixin):
            items: list = field(Array(-1, Int8ub))


def test_array_exact_count_roundtrip():
    """数量匹配时正常 roundtrip（错误类旁路）。[自然]"""

    @dataclass
    class P(StructMixin):
        items: list = field(Array(3, Int8ub))

    pkt = P.parse(b"\x01\x02\x03")
    assert pkt.items == [1, 2, 3]
    assert P(items=[1, 2, 3]).build() == b"\x01\x02\x03"
