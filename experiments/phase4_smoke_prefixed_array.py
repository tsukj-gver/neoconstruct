"""Phase 4 子任务 4.3 端到端冒烟测试：PrefixedArray。

验证 PrefixedArrayNode 通过 Python 用户面 API 的端到端行为。
"""

import sys

try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")
except Exception:
    pass

from dataclasses import dataclass
from construct import (
    StructMixin,
    field,
    Int8ub,
    Int16ub,
    Int32ub,
    Bytes,
    PrefixedArray,
    RangeError,
    StreamError,
)


# ============================================================
# 测试场景
# ============================================================

def test_basic_parse_byte_byte():
    """PrefixedArray(Int8ub, Int8ub) 基本 parse。"""
    @dataclass
    class P(StructMixin):
        items: list = field(PrefixedArray(Int8ub, Int8ub))

    data = bytes([3, 10, 20, 30])
    obj = P.parse(data)
    assert obj.items == [10, 20, 30], f"got {obj.items}"
    # 反向 build
    built = obj.build()
    assert built == data, f"build mismatch: {built.hex()} vs {data.hex()}"
    print("[OK] basic_parse_byte_byte")


def test_zero_count():
    """PrefixedArray count=0 返回空 list 但消耗 countfield。"""
    @dataclass
    class P(StructMixin):
        items: list = field(PrefixedArray(Int8ub, Int8ub))

    data = bytes([0, 0xFF, 0xFF])
    obj = P.parse(data)
    assert obj.items == [], f"got {obj.items}"
    built = obj.build()
    assert built == bytes([0]), f"build mismatch: {built.hex()}"
    print("[OK] zero_count")


def test_int16ub_countfield():
    """PrefixedArray(Int16ub, Byte) —— 2 字节大端 count。"""
    @dataclass
    class P(StructMixin):
        items: list = field(PrefixedArray(Int16ub, Int8ub))

    data = bytes([0x00, 0x02, 0xAB, 0xCD])
    obj = P.parse(data)
    assert obj.items == [0xAB, 0xCD], f"got {obj.items}"
    built = obj.build()
    assert built == data, f"build mismatch: {built.hex()}"
    print("[OK] int16ub_countfield")


def test_count_insufficient_stream_error():
    """count > 实际可解析元素数 → StreamError。"""
    @dataclass
    class P(StructMixin):
        items: list = field(PrefixedArray(Int8ub, Int8ub))

    try:
        P.parse(bytes([5, 1, 2]))
        assert False, "should raise"
    except StreamError:
        pass
    print("[OK] count_insufficient_stream_error")


def test_countfield_signed_negative_range_error():
    """countfield 是有符号 8-bit，解析得到负数 → RangeError。"""
    from construct import Int8sb
    @dataclass
    class P(StructMixin):
        items: list = field(PrefixedArray(Int8sb, Int8ub))

    try:
        P.parse(bytes([0xFF, 1, 2]))
        assert False, "should raise"
    except RangeError:
        pass
    print("[OK] countfield_signed_negative_range_error")


def test_build_count_overflow():
    """PA-5: count 超出 countfield 范围 → FormatFieldError。"""
    @dataclass
    class P(StructMixin):
        items: list = field(PrefixedArray(Int8ub, Int8ub))

    obj = P(items=list(range(256)))  # 256 超出 u8 范围
    try:
        obj.build()
        assert False, "should raise"
    except Exception as e:
        # FormatFieldError 或构造错误
        assert "error during building" in str(e).lower() or "out of range" in str(e).lower(), \
               f"unexpected error: {type(e).__name__}: {e}"
    print("[OK] build_count_overflow")


def test_nested_prefixed_array():
    """PrefixedArray 嵌套 PrefixedArray。"""
    @dataclass
    class Outer(StructMixin):
        items: list = field(PrefixedArray(Int8ub, PrefixedArray(Int8ub, Int8ub)))

    data = bytes([
        2,           # outer count=2
        1, 0x10,     # inner 0: count=1, [0x10]
        2, 0x20, 0x30,  # inner 1: count=2, [0x20, 0x30]
    ])
    obj = Outer.parse(data)
    assert obj.items == [[0x10], [0x20, 0x30]], f"got {obj.items}"
    built = obj.build()
    assert built == data, f"build mismatch: {built.hex()}"
    print("[OK] nested_prefixed_array")


def test_int32ub_inner():
    """PrefixedArray(Int8ub, Int32ub) —— 4 字节元素。"""
    @dataclass
    class P(StructMixin):
        items: list = field(PrefixedArray(Int8ub, Int32ub))

    # 2 个 Int32ub：0x01020304, 0x05060708
    data = bytes([2, 1, 2, 3, 4, 5, 6, 7, 8])
    obj = P.parse(data)
    assert obj.items == [0x01020304, 0x05060708], f"got {obj.items}"
    built = obj.build()
    assert built == data, f"build mismatch: {built.hex()}"
    print("[OK] int32ub_inner")


def main():
    test_basic_parse_byte_byte()
    test_zero_count()
    test_int16ub_countfield()
    test_count_insufficient_stream_error()
    test_countfield_signed_negative_range_error()
    test_build_count_overflow()
    test_nested_prefixed_array()
    test_int32ub_inner()
    print()
    print("All smoke tests passed!")


if __name__ == "__main__":
    main()
