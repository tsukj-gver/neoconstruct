"""16 种 Int* 格式 + Bytes + GreedyBytes 的 parse/build 往返测试。

每个测试用例：
1. 构造单字段的 StructMixin 子类
2. build 已知值 → 字节
3. parse 字节 → 值
4. 验证值和字节与预期一致

字节布局参考 Python construct 2.10.70：
- Int8u*   : 1 字节，无符号
- Int8s*   : 1 字节，有符号（负数用补码）
- Int16u*  : 2 字节，大端 'HH' 或小端 'HH'
- Int16s*  : 2 字节，有符号
- Int32u*  : 4 字节
- Int64u*  : 8 字节
- 后缀 b=big endian，l=little endian
"""

import struct

import pytest

from neoconstruct import (
    Bytes,
    GreedyBytes,
    Int8ub,
    Int8ul,
    Int8sb,
    Int8sl,
    Int16ub,
    Int16ul,
    Int16sb,
    Int16sl,
    Int32ub,
    Int32ul,
    Int32sb,
    Int32sl,
    Int64ub,
    Int64ul,
    Int64sb,
    Int64sl,
    StructMixin,
    field,
)
from dataclasses import dataclass


# ---------------------------------------------------------------------------
# 16 种 Int* 格式的元数据表
# ---------------------------------------------------------------------------

# (描述符, Python struct 格式字符, 一组合法的测试值)
# struct 格式：'>' 大端，'<' 小端，无符号 'B/H/I/Q'，有符号 'b/h/i/q'
INT_FORMATS = [
    (Int8ub, ">B", [0, 1, 127, 255]),
    (Int8ul, "<B", [0, 1, 127, 255]),
    (Int8sb, ">b", [0, 1, 127, -1, -128]),
    (Int8sl, "<b", [0, 1, 127, -1, -128]),
    (Int16ub, ">H", [0, 1, 255, 256, 65535]),
    (Int16ul, "<H", [0, 1, 255, 256, 65535]),
    (Int16sb, ">h", [0, 1, 32767, -1, -32768]),
    (Int16sl, "<h", [0, 1, 32767, -1, -32768]),
    (Int32ub, ">I", [0, 1, 255, 65536, 2**32 - 1]),
    (Int32ul, "<I", [0, 1, 255, 65536, 2**32 - 1]),
    (Int32sb, ">i", [0, 1, 2**31 - 1, -1, -(2**31)]),
    (Int32sl, "<i", [0, 1, 2**31 - 1, -1, -(2**31)]),
    (Int64ub, ">Q", [0, 1, 2**32, 2**64 - 1]),
    (Int64ul, "<Q", [0, 1, 2**32, 2**64 - 1]),
    (Int64sb, ">q", [0, 1, 2**63 - 1, -1, -(2**63)]),
    (Int64sl, "<q", [0, 1, 2**63 - 1, -1, -(2**63)]),
]


def _make_int_class(descriptor, name=None):
    """动态构造单字段的 StructMixin 子类，用于测试。"""
    cls_name = name or f"IntTest_{descriptor!r}"

    @dataclass
    class IntTestClass(StructMixin):
        x: int = field(descriptor)

    IntTestClass.__name__ = cls_name
    IntTestClass.__qualname__ = cls_name
    return IntTestClass


# ---------------------------------------------------------------------------
# 参数化测试
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "descriptor,struct_fmt,values",
    INT_FORMATS,
    ids=[f[0].__class__.__name__ for f in INT_FORMATS],
)
def test_int_build_matches_struct_pack(descriptor, struct_fmt, values):
    """build 出的字节必须与 Python struct.pack 一致（绝对正确性）。"""
    klass = _make_int_class(descriptor)
    for v in values:
        instance = klass(x=v)
        built = instance.build()
        expected = struct.pack(struct_fmt, v)
        assert built == expected, (
            f"{descriptor} build({v}): got {built!r}, expected {expected!r}"
        )


@pytest.mark.parametrize(
    "descriptor,struct_fmt,values",
    INT_FORMATS,
    ids=[f[0].__class__.__name__ for f in INT_FORMATS],
)
def test_int_parse_matches_struct_unpack(descriptor, struct_fmt, values):
    """parse 出的值必须与 Python struct.unpack 一致。"""
    klass = _make_int_class(descriptor)
    for v in values:
        data = struct.pack(struct_fmt, v)
        parsed = klass.parse(data)
        assert parsed.x == v, (
            f"{descriptor} parse({data!r}): got {parsed.x}, expected {v}"
        )


@pytest.mark.parametrize(
    "descriptor,struct_fmt,values",
    INT_FORMATS,
    ids=[f[0].__class__.__name__ for f in INT_FORMATS],
)
def test_int_roundtrip_preserves_value(descriptor, struct_fmt, values):
    """build → parse → build 保持值一致。"""
    klass = _make_int_class(descriptor)
    for v in values:
        instance = klass(x=v)
        built1 = instance.build()
        parsed = klass.parse(built1)
        built2 = parsed.build()
        assert built1 == built2
        assert parsed.x == v


# ---------------------------------------------------------------------------
# Bytes(n) 往返测试
# ---------------------------------------------------------------------------


def _make_bytes_class(length):
    """构造一个 Bytes(length) 字段的 StructMixin 子类。"""

    @dataclass
    class BytesTestClass(StructMixin):
        x: bytes = field(Bytes(length))

    BytesTestClass.__name__ = f"BytesTestClass_{length}"
    BytesTestClass.__qualname__ = BytesTestClass.__name__
    return BytesTestClass


@pytest.mark.parametrize("length", [0, 1, 2, 4, 8, 16, 32, 64, 100])
def test_bytes_roundtrip(length):
    """Bytes(n) build/parse 往返一致，长度正确。"""
    klass = _make_bytes_class(length)
    payload = bytes((i * 7) & 0xFF for i in range(length))
    instance = klass(x=payload)
    built = instance.build()
    assert len(built) == length
    assert built == payload
    parsed = klass.parse(built)
    assert parsed.x == payload


def test_bytes_build_rejects_wrong_length():
    """Bytes(4) build 长度不匹配 → FieldLengthError（对齐 Python construct）。

    Python construct 的 stream_write 校验 len(data) == length，
    不匹配时抛 StreamError。neoconstruct 映射为 FieldLengthError。
    """
    from neoconstruct import FieldLengthError

    @dataclass
    class B4(StructMixin):
        x: bytes = field(Bytes(4))

    # 3 字节 → 太短，应报错
    with pytest.raises(FieldLengthError, match="expected 4, got 3"):
        B4(x=b"\x00\x01\x02").build()
    # 5 字节 → 太长，应报错
    with pytest.raises(FieldLengthError, match="expected 4, got 5"):
        B4(x=b"\x00\x01\x02\x03\x04").build()


def test_bytes_build_accepts_bytearray():
    """Bytes 接受 bytearray（bytes-like）输入——语义与 bytes 一致。"""

    @dataclass
    class B2(StructMixin):
        x: bytes = field(Bytes(2))

    # 真正传入 bytearray（socket.recv / readinto 等真实数据源形态）
    b = B2(x=bytearray(b"\xab\xcd"))
    assert b.build() == b"\xab\xcd"


def test_bytes_build_accepts_memoryview():
    """Bytes 接受 memoryview（bytes-like）输入——语义与 bytes 一致。"""

    @dataclass
    class B2(StructMixin):
        x: bytes = field(Bytes(2))

    b = B2(x=memoryview(b"\xab\xcd"))
    assert b.build() == b"\xab\xcd"


def test_bytes_build_bytearray_length_mismatch_raises():
    """bytes-like 输入同样参与 Bytes 长度校验。"""

    from neoconstruct import FieldLengthError

    @dataclass
    class B4(StructMixin):
        x: bytes = field(Bytes(4))

    with pytest.raises(FieldLengthError):
        B4(x=bytearray(b"\x00\x01\x02")).build()
    with pytest.raises(FieldLengthError):
        B4(x=memoryview(b"\x00\x01\x02\x03\x04")).build()


# ---------------------------------------------------------------------------
# GreedyBytes 往返测试
# ---------------------------------------------------------------------------


def _make_greedy_class():

    @dataclass
    class GreedyTestClass(StructMixin):
        x: bytes = field(GreedyBytes)

    return GreedyTestClass


@pytest.mark.parametrize(
    "payload",
    [
        b"",
        b"\x00",
        b"\x00\x01",
        b"hello",
        b"hello world",
        bytes(range(256)),
        bytes(range(256)) * 10,  # 2560 字节
    ],
)
def test_greedy_bytes_roundtrip(payload):
    """GreedyBytes build/parse 往返保持字节完全一致。"""
    klass = _make_greedy_class()
    instance = klass(x=payload)
    built = instance.build()
    assert built == payload
    parsed = klass.parse(built)
    assert parsed.x == payload


def test_greedy_bytes_consumes_all_remaining():
    """GreedyBytes 应消费流中所有剩余字节。"""
    klass = _make_greedy_class()
    data = b"\x01\x02\x03\x04\x05"
    parsed = klass.parse(data)
    assert parsed.x == data


def test_greedy_bytes_followed_by_nothing():
    """GreedyBytes 作为唯一字段时正确处理。"""
    klass = _make_greedy_class()
    parsed = klass.parse(b"")
    assert parsed.x == b""


def test_greedy_bytes_build_accepts_bytes_like():
    """GreedyBytes build 接受 bytearray / memoryview（语义与 bytes 一致）。"""
    klass = _make_greedy_class()
    assert klass(x=bytearray(b"\x01\x02\x03")).build() == b"\x01\x02\x03"
    assert klass(x=memoryview(b"\x04\x05")).build() == b"\x04\x05"


# ---------------------------------------------------------------------------
# 多字段混合往返测试
# ---------------------------------------------------------------------------


def test_mixed_fields_roundtrip():
    """Int + Bytes + Int + GreedyBytes 组合的往返。"""

    @dataclass
    class Mixed(StructMixin):
        a: int = field(Int16ub)
        b: bytes = field(Bytes(3))
        c: int = field(Int32ul)
        d: bytes = field(GreedyBytes)

    original = Mixed(a=0x1234, b=b"\xab\xcd\xef", c=0xCAFEBABE, d=b"\x00\x11\x22")
    built = original.build()
    # 验证字节布局
    assert built[:2] == b"\x12\x34"  # Int16ub 大端
    assert built[2:5] == b"\xab\xcd\xef"
    assert built[5:9] == bytes([0xBE, 0xBA, 0xFE, 0xCA])  # Int32ul 小端
    assert built[9:] == b"\x00\x11\x22"

    parsed = Mixed.parse(built)
    assert parsed.a == 0x1234
    assert parsed.b == b"\xab\xcd\xef"
    assert parsed.c == 0xCAFEBABE
    assert parsed.d == b"\x00\x11\x22"


# ---------------------------------------------------------------------------
# 浮点格式别名（Half/Single/Double）：别名即同物 + 往返
# ---------------------------------------------------------------------------


def test_float_aliases_are_identical_objects():
    """Half/Single/Double 是 Float16b/32b/64b 的同一对象（别名即同物）。[自然]"""

    from neoconstruct import Double, Float16b, Float32b, Float64b, Half, Single

    assert Half is Float16b
    assert Single is Float32b
    assert Double is Float64b


@pytest.mark.parametrize(
    "alias,struct_fmt,value",
    [
        ("Half", ">e", 1.0),
        ("Single", ">f", 100.0),
        ("Double", ">d", 1234.5),
    ],
)
def test_float_alias_roundtrip_matches_struct_pack(alias, struct_fmt, value):
    """别名 build 与 struct.pack 一致；build→parse 往返保持值。[自然]"""

    from neoconstruct import Double, Half, Single

    descriptor = {"Half": Half, "Single": Single, "Double": Double}[alias]

    @dataclass
    class FloatAlias(StructMixin):
        v: float = field(descriptor)

    built = FloatAlias(v=value).build()
    assert built == struct.pack(struct_fmt, value)
    parsed = FloatAlias.parse(built)
    assert parsed.v == value
    assert parsed.build() == built
