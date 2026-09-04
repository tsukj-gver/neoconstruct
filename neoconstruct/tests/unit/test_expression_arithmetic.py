"""表达式 VM 算术边界行为锁定。

VM 是 i64 栈机：算术溢出采用回绕语义（wrapping），除零/取模零与负移位
返回显式 ``ConstructError`` 族异常，绝不 panic。本文件锁定各边界值的具体形态。

覆盖：
- ``//0`` / ``%0``：显式 ConstructError（消息含语义词 + path）
- i64 溢出边界（±2^63）：回绕语义（[基线] 锚定，VM 实现为 i64 栈机）
- 移位边界：``<<63`` / ``<<64``（移位量按 64 掩码）、负移位显式报错
- 消费者场景：``Bytes(表达式)`` 大值（溢出为负 → FieldLengthError；
  超流长 → StreamError；除零 → 显式错误）
- 整除/取模的 Python floor 语义（负数侧）
- 比较运算返回 0/1 整型

期望值来源标注：[文档]=expr 模块文档（回绕/floor 语义）；
[自然]=语义自然性；[基线]=实测绿灯行为锚定。
"""

import pytest
from dataclasses import dataclass

from neoconstruct import (
    Bytes,
    ConstructError,
    FieldLengthError,
    GenericConstructError,
    Int64sb,
    Int64ub,
    Int8sb,
    Int8ub,
    StreamError,
    StructMixin,
    field,
    rfield,
)
from neoconstruct import Computed


# ---------------------------------------------------------------------------
# 除零 / 取模零：显式异常（绝不 panic）
# ---------------------------------------------------------------------------


def test_floordiv_by_zero_raises_explicit_error():
    """``Computed(n // 0)`` → ConstructError（基类映射），消息含除法语义词。[基线]

    双重断言：异常类型为 ConstructError（或其子类）；path 定位到表达式字段。
    """

    @dataclass
    class P(StructMixin):
        n: int = field(Int8ub)
        c: int = rfield(Computed(n // 0))

    with pytest.raises(ConstructError) as exc_info:
        P.parse(b"\x05")

    assert type(exc_info.value) is ConstructError
    assert "division by zero" in str(exc_info.value)
    assert exc_info.value.path == "root.c"


def test_modulo_by_zero_raises_explicit_error():
    """``Computed(n % 0)`` → ConstructError，消息含 modulo 语义词。[基线]"""

    @dataclass
    class P(StructMixin):
        n: int = field(Int8ub)
        c: int = rfield(Computed(n % 0))

    with pytest.raises(ConstructError) as exc_info:
        P.parse(b"\x05")

    assert type(exc_info.value) is ConstructError
    assert "modulo by zero" in str(exc_info.value)


def test_bytes_length_division_by_zero_raises_explicit_error():
    """消费者场景 ``Bytes(n // 0)``：求值失败 → GenericConstructError（含除零原因）。[基线]"""

    @dataclass
    class P(StructMixin):
        n: int = field(Int8ub)
        data: bytes = field(Bytes(n // 0))

    with pytest.raises(GenericConstructError) as exc_info:
        P.parse(bytes([5, 1, 2]))

    assert "division by zero" in str(exc_info.value)


# ---------------------------------------------------------------------------
# i64 溢出边界：回绕语义
# ---------------------------------------------------------------------------


def test_i64_max_plus_one_wraps_to_min():
    """``(2**63-1) + 1`` → 回绕为 -2**63。[基线]

    VM 算术为 i64 回绕（表达式典型值远低于边界；任意精度与性能目标冲突，
    为文档化行为）。
    """

    @dataclass
    class P(StructMixin):
        n: int = field(Int64ub)
        c: int = rfield(Computed(n + 1))

    pkt = P.parse((2**63 - 1).to_bytes(8, "big"))
    assert pkt.n == 2**63 - 1
    assert pkt.c == -(2**63)


def test_i64_min_unary_negation_wraps():
    """一元负 ``-(i64::MIN)`` → 回绕仍为 -2**63。[基线]"""

    @dataclass
    class P(StructMixin):
        n: int = field(Int64sb)
        c: int = rfield(Computed(-n))

    pkt = P.parse((-(2**63)).to_bytes(8, "big", signed=True))
    assert pkt.n == -(2**63)
    assert pkt.c == -(2**63)


def test_large_multiplication_wraps():
    """``(2**63-1) * (2**63-1)`` → 回绕结果 1。[基线]"""

    @dataclass
    class P(StructMixin):
        n: int = field(Int64ub)
        c: int = rfield(Computed(n * n))

    pkt = P.parse((2**63 - 1).to_bytes(8, "big"))
    assert pkt.c == 1


# ---------------------------------------------------------------------------
# 移位边界
# ---------------------------------------------------------------------------


def test_shl_63_boundary():
    """``1 << 63`` → i64 位模式即 -2**63。[基线]"""

    @dataclass
    class P(StructMixin):
        x: int = field(Int8ub)
        c: int = rfield(Computed(x << 63))

    assert P.parse(b"\x01").c == -(2**63)


def test_shl_64_masks_shift_amount():
    """``1 << 64`` → 移位量按 64 掩码（等价 <<0），结果 1。[基线]

    VM 移位采用 wrapping 语义（移位量取低 6 位），非任意精度左移。
    """

    @dataclass
    class P(StructMixin):
        x: int = field(Int8ub)
        c: int = rfield(Computed(x << 64))

    assert P.parse(b"\x01").c == 1


def test_shr_64_masks_shift_amount():
    """``0xff >> 64`` → 等价 >>0，结果 255。[基线]"""

    @dataclass
    class P(StructMixin):
        x: int = field(Int8ub)
        c: int = rfield(Computed(x >> 64))

    assert P.parse(b"\xff").c == 255


def test_negative_shift_raises_explicit_error():
    """``x << -1`` → GenericConstructError，消息含 "negative shift count"。[基线]

    负移位无定义语义，VM 显式报错而非 panic/静默。
    """

    @dataclass
    class P(StructMixin):
        x: int = field(Int8ub)
        c: int = rfield(Computed(x << -1))

    with pytest.raises(GenericConstructError) as exc_info:
        P.parse(b"\x01")

    assert "negative shift count" in str(exc_info.value)
    assert exc_info.value.path == "root.c"


# ---------------------------------------------------------------------------
# 消费者场景：Bytes(表达式) 大值
# ---------------------------------------------------------------------------


def test_bytes_length_overflow_to_negative_raises_field_length_error():
    """``Bytes(n * 2)`` 且 n=2**62 → 积回绕为负 → FieldLengthError。[基线]

    消费者对长度表达式结果做负值防御，显式报错而非 panic/静默截断。
    """

    @dataclass
    class P(StructMixin):
        n: int = field(Int64ub)
        data: bytes = field(Bytes(n * 2))

    with pytest.raises(FieldLengthError) as exc_info:
        P.parse((2**62).to_bytes(8, "big"))

    assert "negative" in str(exc_info.value)


def test_bytes_length_exceeding_stream_raises_stream_error():
    """``Bytes(n * 2)`` 长度合法但超剩余流 → StreamError（正常流错误路径）。[自然]"""

    @dataclass
    class P(StructMixin):
        n: int = field(Int8ub)
        data: bytes = field(Bytes(n * 2))

    with pytest.raises(StreamError) as exc_info:
        P.parse(bytes([100, 1, 2]))

    assert exc_info.value.path == "root.data"


# ---------------------------------------------------------------------------
# floor 语义与比较链
# ---------------------------------------------------------------------------


def test_floordiv_negative_follows_floor_semantics():
    """``-7 // 2 == -4``（向负无穷取整，非截断）。[文档] VM 实现 Python floor 语义。"""

    @dataclass
    class P(StructMixin):
        n: int = field(Int8sb)
        c: int = rfield(Computed(n // 2))

    assert P.parse(b"\xf9").c == -4  # -7 // 2 == -4


def test_modulo_negative_follows_divisor_sign():
    """``-7 % 2 == 1``（余数符号与除数一致）。[文档]"""

    @dataclass
    class P(StructMixin):
        n: int = field(Int8sb)
        c: int = rfield(Computed(n % 2))

    assert P.parse(b"\xf9").c == 1  # -7 % 2 == 1


def test_comparison_result_is_zero_or_one_int():
    """比较运算返回 0/1 整型：``(x > 3) + 10`` → 11 / 10。[基线]"""

    @dataclass
    class P(StructMixin):
        x: int = field(Int8ub)
        c: int = rfield(Computed((x > 3) + 10))

    assert P.parse(b"\x05").c == 11
    assert P.parse(b"\x01").c == 10
