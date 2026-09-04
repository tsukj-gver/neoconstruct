"""字符串 / 时间戳 / 旋转类异常用户面行为锁定：StringError / TimestampError / RotationError。

期望值来源标注：[文档]=异常类/宏 docstring 契约；[自然]=语义自然性；
[基线]=实测绿灯行为锚定。
"""

import pytest
from dataclasses import dataclass

from neoconstruct import (
    CString,
    GreedyString,
    Int16ub,
    Int32ub,
    Int8ub,
    PaddedString,
    ProcessRotateLeft,
    RotationError,
    StringError,
    StructMixin,
    Timestamp,
    TimestampError,
    field,
)


# ---------------------------------------------------------------------------
# StringError：编解码失败
# ---------------------------------------------------------------------------


def test_string_error_on_invalid_utf8_parse():
    """GreedyString("utf8") parse 非法 UTF-8 字节序列 → StringError。[文档]

    双重断言：类型 StringError；path 含字段名；消息含编码名与失败原因。
    """

    @dataclass
    class P(StructMixin):
        s: str = field(GreedyString("utf8"))

    with pytest.raises(StringError) as exc_info:
        P.parse(b"\xff\xfe\xfd")

    assert "s" in (exc_info.value.path or "")
    msg = str(exc_info.value)
    assert "utf8" in msg and "decode" in msg.lower()


def test_string_error_on_ascii_with_non_ascii_chars_build():
    """ASCII 编码 build 非 ASCII 字符 → StringError。[文档]"""

    @dataclass
    class P(StructMixin):
        s: str = field(CString("ascii"))

    with pytest.raises(StringError) as exc_info:
        P(s="héllo").build()

    assert "s" in (exc_info.value.path or "")


def test_valid_string_roundtrip():
    """合法字符串编解码 roundtrip（错误类旁路）。[自然]"""

    @dataclass
    class P(StructMixin):
        s: str = field(GreedyString("utf8"))

    text = "héllo→世界"
    pkt = P.parse(text.encode("utf8"))
    assert pkt.s == text
    assert P(s=text).build() == text.encode("utf8")


def test_padded_string_roundtrip():
    """PaddedString 定长填充 roundtrip（错误类旁路）。[自然]"""

    @dataclass
    class P(StructMixin):
        s: str = field(PaddedString(8, "utf8"))

    assert P.parse(b"ab\x00\x00\x00\x00\x00\x00").s == "ab"
    assert P(s="ab").build() == b"ab\x00\x00\x00\x00\x00\x00"


# ---------------------------------------------------------------------------
# TimestampError：宏参数类型错误（Python 层构造期）
# ---------------------------------------------------------------------------


def test_timestamp_error_on_invalid_unit_type():
    """Timestamp unit 传非法类型 → TimestampError。[文档]

    双重断言：类型 TimestampError；消息含 unit 语义词。
    """

    with pytest.raises(TimestampError) as exc_info:
        Timestamp(Int32ub, unit=object(), epoch=1)

    assert "unit" in str(exc_info.value)


def test_timestamp_error_on_invalid_epoch_type():
    """Timestamp epoch 传非法类型 → TimestampError。[文档]"""

    with pytest.raises(TimestampError) as exc_info:
        Timestamp(Int32ub, unit=1, epoch=object())

    assert "epoch" in str(exc_info.value)


def test_timestamp_error_on_none_subcon():
    """Timestamp subcon 为 None → TimestampError（宏层提前校验）。[文档]"""

    with pytest.raises(TimestampError) as exc_info:
        Timestamp(None, 1, 1970)

    assert "subcon" in str(exc_info.value).lower()


# ---------------------------------------------------------------------------
# RotationError：ProcessRotateLeft 参数/数据长度非法
# ---------------------------------------------------------------------------


def test_rotation_error_on_data_not_multiple_of_group():
    """数据长度非 group 整数倍 → RotationError。[文档]

    Int16ub 数据 2 字节，group=3 → 2 % 3 != 0。
    双重断言：类型 RotationError；path 含字段名；消息含 group 语义词。
    """

    @dataclass
    class P(StructMixin):
        v: int = field(ProcessRotateLeft(4, 3, Int16ub))

    with pytest.raises(RotationError) as exc_info:
        P.parse(b"\x0f\xf0")

    assert "v" in (exc_info.value.path or "")
    assert "group" in str(exc_info.value)


def test_rotation_error_on_group_below_one():
    """group=0 → RotationError（组大小至少为 1）。[文档]"""

    @dataclass
    class P(StructMixin):
        v: int = field(ProcessRotateLeft(4, 0, Int16ub))

    with pytest.raises(RotationError) as exc_info:
        P.parse(b"\x0f\xf0")

    assert "group" in str(exc_info.value)


def test_rotation_valid_roundtrip():
    """合法旋转参数 roundtrip（错误类旁路）。[自然]

    每字节旋转 4 位：0xf00f ↔ b"\\x0f\\xf0"（encode/decode 互逆）。
    """

    @dataclass
    class P(StructMixin):
        v: int = field(ProcessRotateLeft(4, 1, Int16ub))

    assert P.parse(b"\x0f\xf0").v == 0xF00F
    assert P(v=0xF00F).build() == b"\x0f\xf0"
