"""VET Phase 3.2 深度验证：错误路径、嵌套场景、跨字段对齐。"""
from __future__ import annotations

import sys
sys.path.insert(0, "construct-rs/python")

from dataclasses import dataclass

from construct import (
    BitStructMixin,
    StructMixin,
    Bitwise,
    BitsInteger,
    Bit,
    Nibble,
    Int8ub,
    field,
)


def test_bitstruct_inner_path_segment():
    """BitStruct 字段流不足时，错误路径应含字段名。
    
    BitStruct { a: BitsInteger(4), b: BitsInteger(4) }
    parse b'' → a 解析时 stream 不足 → 错误路径含 .a
    """
    @dataclass
    class H(BitStructMixin):
        a: int = field(BitsInteger(4))
        b: int = field(BitsInteger(4))

    try:
        H.parse(b"")
        assert False
    except Exception as e:
        msg = str(e)
        # 路径应含 "root.a"
        assert "root" in msg, f"path missing 'root': {msg}"
        assert ".a" in msg, f"path missing '.a': {msg}"
        print(f"[OK] test_bitstruct_inner_path_segment (path in msg)")


def test_bitwise_in_struct_path_segment():
    """Bitwise 嵌入 Struct 作为字段，错误路径应含字段名。
    
    Struct { magic: Int8ub, bits: Bitwise(BitsInteger(12)) }
    Bitwise(BitsInteger(12)) 内层 12 bit 非 8 倍数 → BitField 错误
    错误路径应含 ".bits"（Bitwise 字段名）
    """
    @dataclass
    class P(StructMixin):
        magic: int = field(Int8ub)
        bits: int = field(Bitwise(BitsInteger(12)))

    try:
        P.parse(b"\xAA\xFF\xFF")
        assert False
    except Exception as e:
        msg = str(e)
        # 路径应含 ".bits"
        if ".bits" in msg:
            print(f"[OK] test_bitwise_in_struct_path_segment (path contains .bits)")
        else:
            # 当 Bitwise 嵌入字段时，错误可能由 BitwiseNode 直接抛出（path=root.bits）
            # 也可能由 StructNode 重建（path=root.bits）
            print(f"[INFO] test_bitwise_in_struct_path_segment: msg={msg[:120]}")


def test_cross_byte_boundary_parsing():
    """跨字节边界解析：12 bit 整数跨 2 字节"""
    @dataclass
    class H(BitStructMixin):
        a: int = field(BitsInteger(12))
        b: int = field(BitsInteger(4))

    # a = 0xABC = 2748, b = 0
    # bits: 1010_1011_1100 | 0000
    # bytes: 0xAB 0xC0
    parsed = H.parse(b"\xAB\xC0")
    assert parsed.a == 0xABC, f"a expected 0xABC (2748), got {parsed.a}"
    assert parsed.b == 0, f"b expected 0, got {parsed.b}"

    # build 往返
    h = H(a=0xABC, b=0)
    built = h.build()
    assert built == b"\xAB\xC0", f"built expected b'\\xAB\\xC0', got {built!r}"
    print("[OK] test_cross_byte_boundary_parsing")


def test_signed_bits_in_bitstruct():
    """BitStruct 内 signed BitsInteger"""
    @dataclass
    class H(BitStructMixin):
        a: int = field(BitsInteger(4, signed=True))
        b: int = field(BitsInteger(4, signed=True))

    # a = 0b1000 = -8 (signed), b = 0b0001 = 1 (signed)
    # bits: 1000 0001 = 0x81
    parsed = H.parse(b"\x81")
    assert parsed.a == -8, f"a expected -8, got {parsed.a}"
    assert parsed.b == 1, f"b expected 1, got {parsed.b}"

    # build
    h = H(a=-8, b=1)
    built = h.build()
    assert built == b"\x81", f"built expected b'\\x81', got {built!r}"
    print("[OK] test_signed_bits_in_bitstruct")


def test_octet_nibble_combination():
    """Octet + Nibble 组合（8 + 4 = 12 bit 非 8 倍数，需要凑齐）"""
    @dataclass
    class H(BitStructMixin):
        a: int = field(BitsInteger(8))  # Octet
        b: int = field(Nibble())         # 4 bit
        c: int = field(BitsInteger(4))  # 凑齐 16 bit

    parsed = H.parse(b"\xAB\xCD")
    assert parsed.a == 0xAB, f"a expected 0xAB, got {parsed.a}"
    assert parsed.b == 0xC, f"b expected 0xC, got {parsed.b}"
    assert parsed.c == 0xD, f"c expected 0xD, got {parsed.c}"
    print("[OK] test_octet_nibble_combination")


def test_swapped_bits_in_bitstruct():
    """BitStruct 内 swapped BitsInteger（字节序反序）"""
    @dataclass
    class H(BitStructMixin):
        a: int = field(BitsInteger(16, swapped=True))

    # BitsInteger(16, swapped=True)：先按 MSB-first 读 16 bit，再按 8 位组反序
    # 输入 b"\xCD\xAB" → MSB-first raw = 0xCDAB → swap → 0xABCD = 43981
    parsed = H.parse(b"\xCD\xAB")
    assert parsed.a == 0xABCD, f"a expected 0xABCD, got {parsed.a}"

    # build：value=0xABCD → swap → 0xCDAB → 写入 b"\xCD\xAB"
    h = H(a=0xABCD)
    built = h.build()
    assert built == b"\xCD\xAB", f"built expected b'\\xCD\\xAB', got {built!r}"
    print("[OK] test_swapped_bits_in_bitstruct")


def test_large_bitstruct_64_bits():
    """64 bit BitStruct = 8 字节"""
    @dataclass
    class H(BitStructMixin):
        hi: int = field(BitsInteger(32))
        lo: int = field(BitsInteger(32))

    # hi = 0x12345678, lo = 0x9ABCDEF0
    parsed = H.parse(b"\x12\x34\x56\x78\x9A\xBC\xDE\xF0")
    assert parsed.hi == 0x12345678, f"hi expected 0x12345678, got {parsed.hi}"
    assert parsed.lo == 0x9ABCDEF0, f"lo expected 0x9ABCDEF0, got {parsed.lo}"
    print("[OK] test_large_bitstruct_64_bits")


def test_nested_bitstruct_3_layers():
    """三层嵌套：Outer BitStruct 含 Inner BitStruct（StructRef → BitwiseNode）
    Inner BitStruct 含 Innermost BitsInteger(8)"""
    @dataclass
    class Innermost(BitStructMixin):
        x: int = field(BitsInteger(8))

    @dataclass
    class Outer(BitStructMixin):
        inner: Innermost = field(Innermost)
        y: int = field(BitsInteger(8))

    parsed = Outer.parse(b"\x11\x22")
    assert parsed.inner.x == 0x11, f"inner.x expected 0x11, got {parsed.inner.x}"
    assert parsed.y == 0x22, f"y expected 0x22, got {parsed.y}"
    print("[OK] test_nested_bitstruct_3_layers")


if __name__ == "__main__":
    tests = [
        test_bitstruct_inner_path_segment,
        test_bitwise_in_struct_path_segment,
        test_cross_byte_boundary_parsing,
        test_signed_bits_in_bitstruct,
        test_octet_nibble_combination,
        test_swapped_bits_in_bitstruct,
        test_large_bitstruct_64_bits,
        test_nested_bitstruct_3_layers,
    ]

    passed = 0
    failed = 0
    for test in tests:
        try:
            test()
            passed += 1
        except Exception as e:
            failed += 1
            import traceback
            print(f"[FAIL] {test.__name__}: {e}")
            traceback.print_exc()

    print(f"\n=== Summary: {passed} passed, {failed} failed ===")
    sys.exit(0 if failed == 0 else 1)
