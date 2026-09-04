"""校验与选择类异常用户面行为锁定：CheckError / TerminatedError / ValidationError / SelectError。

每异常类 ≥1 正例（正确触发 + 类型断言 + 消息含关键定位信息）。
期望值来源标注：[文档]=异常类 docstring 契约；[自然]=语义自然性；
[基线]=实测绿灯行为锚定。
"""

import pytest
from dataclasses import dataclass
from typing import Any

from neoconstruct import (
    Check,
    CheckError,
    Int16ub,
    Int32ub,
    Int8ub,
    NoneOf,
    OneOf,
    Select,
    SelectError,
    StructMixin,
    Terminated,
    TerminatedError,
    ValidationError,
    field,
    rfield,
)


# ---------------------------------------------------------------------------
# CheckError：断言表达式求值为假
# ---------------------------------------------------------------------------


def test_check_error_on_false_expression_parse():
    """Check(x == 5) 在 parse 时表达式为假 → CheckError。[文档]

    双重断言：类型 CheckError；path 含字段名 _check；消息含 check 语义词。
    """

    @dataclass
    class P(StructMixin):
        x: int = field(Int8ub)
        _check: Any = rfield(Check(x == 5))

    with pytest.raises(CheckError) as exc_info:
        P.parse(b"\x03")

    assert "_check" in (exc_info.value.path or "")
    assert "check" in str(exc_info.value)


def test_check_error_on_false_expression_build():
    """Check 表达式在 build 时同样求值校验 → CheckError。[文档]"""

    @dataclass
    class P(StructMixin):
        x: int = field(Int8ub)
        _check: Any = rfield(Check(x == 5))

    with pytest.raises(CheckError):
        P(x=3).build()


def test_check_passes_when_expression_true():
    """表达式为真时 Check 无感通过（值语义旁路）。[自然]

    双重断言：parse 字段值；build 字节。
    """

    @dataclass
    class P(StructMixin):
        x: int = field(Int8ub)
        _check: Any = rfield(Check(x == 5))

    assert P.parse(b"\x05").x == 5
    assert P(x=5).build() == b"\x05"


# ---------------------------------------------------------------------------
# TerminatedError：EOF 断言
# ---------------------------------------------------------------------------


def test_terminated_error_on_trailing_bytes():
    """Terminated 在 stream 有剩余字节时 → TerminatedError。[文档]

    双重断言：类型；path 含 _term；消息含 end of stream 语义词。
    """

    @dataclass
    class P(StructMixin):
        x: int = field(Int8ub)
        _term: Any = rfield(Terminated)

    with pytest.raises(TerminatedError) as exc_info:
        P.parse(b"\x01\x02")

    assert "_term" in (exc_info.value.path or "")
    assert "end of stream" in str(exc_info.value)


def test_terminated_passes_at_exact_eof():
    """恰好消费完所有字节时 Terminated 通过。[自然]

    双重断言：parse 值；roundtrip 字节。
    """

    @dataclass
    class P(StructMixin):
        x: int = field(Int8ub)
        _term: Any = rfield(Terminated)

    assert P.parse(b"\x07").x == 7
    assert P(x=7).build() == b"\x07"


# ---------------------------------------------------------------------------
# ValidationError：OneOf / NoneOf 值集合校验
# ---------------------------------------------------------------------------


def test_one_of_rejects_value_outside_set_on_parse():
    """OneOf parse 到非合法集值 → ValidationError。[文档]

    双重断言：类型；path 含字段名；消息含实际失败值 5。
    """

    @dataclass
    class P(StructMixin):
        x: int = field(OneOf(Int8ub, [1, 2, 3]))

    with pytest.raises(ValidationError) as exc_info:
        P.parse(b"\x05")

    assert (exc_info.value.path or "") == "root.x"
    assert "5" in str(exc_info.value)


def test_one_of_rejects_value_outside_set_on_build():
    """OneOf build 传入非合法集值 → ValidationError（parse/build 对称）。[自然]"""

    @dataclass
    class P(StructMixin):
        x: int = field(OneOf(Int8ub, [1, 2, 3]))

    with pytest.raises(ValidationError):
        P(x=9).build()


def test_none_of_rejects_value_in_forbidden_set():
    """NoneOf parse 到禁集值 → ValidationError。[文档]"""

    @dataclass
    class P(StructMixin):
        x: int = field(NoneOf(Int8ub, [7, 8]))

    with pytest.raises(ValidationError) as exc_info:
        P.parse(b"\x07")

    assert "7" in str(exc_info.value)


def test_validation_accepts_valid_value_roundtrip():
    """合法值通过校验并 roundtrip。[自然]

    双重断言：parse 值；build 字节。
    """

    @dataclass
    class P(StructMixin):
        x: int = field(OneOf(Int8ub, [1, 2, 3]))

    assert P.parse(b"\x02").x == 2
    assert P(x=2).build() == b"\x02"


# ---------------------------------------------------------------------------
# SelectError：所有 subcon 都未成功
# ---------------------------------------------------------------------------


def test_select_error_when_all_subcons_fail():
    """Select 全部候选失败 → SelectError。[文档]

    双重断言：类型 SelectError；path 含字段名；消息含 no subconstruct 语义词。
    """

    @dataclass
    class P(StructMixin):
        x: Any = field(Select(Int16ub, Int32ub))

    with pytest.raises(SelectError) as exc_info:
        P.parse(b"\x01")

    assert "x" in (exc_info.value.path or "")
    assert "no subconstruct matched" in str(exc_info.value)


def test_select_returns_first_successful_subcon():
    """Select 首个成功候选胜出（错误类旁路）。[自然]

    双重断言：parse 值；roundtrip 字节。
    """

    @dataclass
    class P(StructMixin):
        x: Any = field(Select(Int16ub, Int32ub))

    pkt = P.parse(b"\x00\x05")
    assert pkt.x == 5
    assert P(x=5).build() == b"\x00\x05"
