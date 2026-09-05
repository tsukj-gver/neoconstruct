"""表达式 VM 算术边界行为锁定。

VM 是 i64 栈机：Python 用户心智是任意精度整数，因此**算术溢出与移位越界
显式报错**（消息含 "overflow" 语义词），绝不静默回绕、绝不 panic。除零/
取模零与负移位同样返回显式 ``ConstructError`` 族异常。

覆盖：
- ``//0`` / ``%0``：显式 ConstructError（消息含语义词 + path）
- i64 溢出边界（±2^63）：加/乘/一元负/整除 MIN//-1 → 显式溢出错误
- 移位边界：``<<63``（值溢出）、``<<64`` / ``>>64``（移位量越界）→ 显式报错；
  负移位显式报错
- 消费者场景：``Bytes(表达式)`` 负长度（FieldLengthError 长度面防御）、
  表达式溢出（GenericConstructError 含 overflow 原因）、超流长（StreamError）、
  除零（显式错误）
- 整除/取模的 Python floor 语义（负数侧）
- 比较运算返回 0/1 整型

期望值来源标注：[文档]=溢出防御契约（显式失败优于静默错值）；
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
# i64 溢出边界：显式报错（Python 心智 = 任意精度，静默错值不可接受）
# ---------------------------------------------------------------------------


def test_i64_max_plus_one_raises_overflow_error():
    """``(2**63-1) + 1`` → 显式 ConstructError（消息含 "overflow"）。[文档]

    Python int 任意精度永不溢出；VM 溢出必须显式失败而非静默回绕。
    双重断言：异常类型 ConstructError；消息含 overflow 语义词；path 定位字段。
    """

    @dataclass
    class P(StructMixin):
        n: int = field(Int64ub)
        c: int = rfield(Computed(n + 1))

    with pytest.raises(ConstructError) as exc_info:
        P.parse((2**63 - 1).to_bytes(8, "big"))

    assert type(exc_info.value) is ConstructError
    assert "overflow" in str(exc_info.value)
    assert exc_info.value.path == "root.c"


def test_i64_min_unary_negation_raises_overflow_error():
    """一元负 ``-(i64::MIN)`` 数学结果 +2^63 超界 → 显式溢出错误。[文档]"""

    @dataclass
    class P(StructMixin):
        n: int = field(Int64sb)
        c: int = rfield(Computed(-n))

    with pytest.raises(ConstructError) as exc_info:
        P.parse((-(2**63)).to_bytes(8, "big", signed=True))

    assert type(exc_info.value) is ConstructError
    assert "overflow" in str(exc_info.value)


def test_large_multiplication_raises_overflow_error():
    """``(2**63-1) * (2**63-1)`` 数学结果远超 i64 → 显式溢出错误。[文档]

    校验和/缩放计算的极端值静默错值是协议解析最危险的缺陷形态。
    """

    @dataclass
    class P(StructMixin):
        n: int = field(Int64ub)
        c: int = rfield(Computed(n * n))

    with pytest.raises(ConstructError) as exc_info:
        P.parse((2**63 - 1).to_bytes(8, "big"))

    assert "overflow" in str(exc_info.value)


def test_floordiv_i64_min_by_minus_one_raises_overflow_error():
    """``i64::MIN // -1`` 数学结果 +2^63 超界 → 显式溢出错误。[文档]

    Python 任意精度下 -(-2**63) // -1 = 2**63 合法；i64 VM 必须显式报错
    而非回绕（``%`` 侧数学结果为 0，仍合法）。
    """

    @dataclass
    class P(StructMixin):
        n: int = field(Int64sb)
        q: int = rfield(Computed(n // -1))

    with pytest.raises(ConstructError) as exc_info:
        P.parse((-(2**63)).to_bytes(8, "big", signed=True))

    assert "overflow" in str(exc_info.value)


# ---------------------------------------------------------------------------
# 移位边界：移位量越界与值溢出均显式报错
# ---------------------------------------------------------------------------


def test_shl_63_raises_overflow_error():
    """``1 << 63`` 数学结果 2^63 超 i64 上界 → 显式溢出错误。[文档]

    Python ``1 << 63`` 是正整数 9223372036854775808；VM 静默给出 -2^63
    属符号翻转错值。
    """

    @dataclass
    class P(StructMixin):
        x: int = field(Int8ub)
        c: int = rfield(Computed(x << 63))

    with pytest.raises(ConstructError) as exc_info:
        P.parse(b"\x01")

    assert "overflow" in str(exc_info.value)


def test_shl_64_raises_shift_overflow_error():
    """``1 << 64`` 移位量 ≥ 64 越界 → 显式报错（含 overflow 语义词）。[文档]

    Python ``1 << 64`` 合法（任意精度）；VM 掩码移位量静默得 1 是错值。
    """

    @dataclass
    class P(StructMixin):
        x: int = field(Int8ub)
        c: int = rfield(Computed(x << 64))

    with pytest.raises(ConstructError) as exc_info:
        P.parse(b"\x01")

    assert "overflow" in str(exc_info.value)
    assert exc_info.value.path == "root.c"


def test_shr_64_raises_shift_overflow_error():
    """``0xff >> 64`` 移位量 ≥ 64 越界 → 显式报错（与 ``<<64`` 对称）。[文档]"""

    @dataclass
    class P(StructMixin):
        x: int = field(Int8ub)
        c: int = rfield(Computed(x >> 64))

    with pytest.raises(ConstructError) as exc_info:
        P.parse(b"\xff")

    assert "overflow" in str(exc_info.value)


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


def test_bytes_length_negative_raises_field_length_error():
    """``Bytes(0 - n)`` 求值负长度 → FieldLengthError（长度面防御）。[基线]

    消费者对长度表达式结果做负值防御，显式报错而非 panic/静默截断。
    """

    @dataclass
    class P(StructMixin):
        n: int = field(Int8ub)
        data: bytes = field(Bytes(0 - n))

    with pytest.raises(FieldLengthError) as exc_info:
        P.parse(b"\x05")

    assert "negative" in str(exc_info.value)


def test_bytes_length_expression_overflow_reports_overflow():
    """``Bytes(n * 2)`` 且 n=2**62 → 表达式乘法溢出 → 显式错误（含 overflow）。[文档]

    溢出在表达式求值面报错（GenericConstructError 含原因链），与长度面
    负值防御（FieldLengthError）分层：溢出先于长度检查发生。
    """

    @dataclass
    class P(StructMixin):
        n: int = field(Int64ub)
        data: bytes = field(Bytes(n * 2))

    with pytest.raises(GenericConstructError) as exc_info:
        P.parse((2**62).to_bytes(8, "big"))

    assert "overflow" in str(exc_info.value)


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
