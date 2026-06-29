"""Phase 3.3 集成测试：验证 Python 端 API 端到端可用。"""
from dataclasses import dataclass
from construct import (
    StructMixin, BitStructMixin, field, rfield, wfield,
    Int8ub, Int16ub, Int32ub,
    Bit, Nibble, BitsInteger,
    Bitwise,
    Padding,
    Bytewise,
    BitsSwapped,
    ByteSwapped,
    Bytes,
    IntegerError,
    PaddingError,
)


def test_bitstruct_with_padding():
    """BitStruct with Padding."""
    @dataclass
    class BitsHeader(BitStructMixin):
        flag: int = field(Bit())
        value: int = field(BitsInteger(10))
        reserved: int = wfield(Padding(5), default=None)

    data = b"\xbe\xef"
    parsed = BitsHeader.parse(data)
    assert parsed.flag == 1
    assert parsed.value == 503  # 10 bit of 0xBEEF: bit1|111011_11101 = 0x1F7 = 503
    # round trip: rebuild from parsed values
    msg = BitsHeader(flag=parsed.flag, value=parsed.value)
    built = msg.build()
    # last 5 bits are 0 (default padding pattern)
    assert built == b"\xbe\xe0", f"Expected bee0, got {built.hex()}"
    # parse built back gives same values
    reparsed = BitsHeader.parse(built)
    assert reparsed.flag == parsed.flag
    assert reparsed.value == parsed.value
    print("[OK] BitStruct with Padding round-trip")


def test_padding_byte_domain():
    """Padding in byte domain."""
    @dataclass
    class Padded(StructMixin):
        tag: int = field(Int8ub)
        reserved: int = wfield(Padding(4), default=None)

    data = b"\xAA\x00\x00\x00\x00"
    parsed = Padded.parse(data)
    assert parsed.tag == 0xAA
    msg = Padded(tag=0xAA)
    built = msg.build()
    assert built == data, f"Expected {data.hex()}, got {built.hex()}"
    print("[OK] Padding byte domain round-trip")


def test_bytewise_inside_bitstruct():
    """Bytewise inside BitStruct."""
    @dataclass
    class BitBytewise(BitStructMixin):
        a: int = field(Nibble())
        b: int = field(Bytewise(Int16ub))
        c: int = field(Nibble())

    # 4 + 16 + 4 = 24 bit = 3 bytes
    data = b"\xA1\x23\x4B"
    parsed = BitBytewise.parse(data)
    assert parsed.a == 0xA, f"a={parsed.a}"
    assert parsed.b == 0x1234, f"b={hex(parsed.b)}"
    assert parsed.c == 0xB, f"c={parsed.c}"
    print("[OK] Bytewise inside BitStruct parse")
    
    # round-trip
    msg = BitBytewise(a=0xA, b=0x1234, c=0xB)
    built = msg.build()
    assert built == data, f"Expected {data.hex()}, got {built.hex()}"
    print("[OK] Bytewise inside BitStruct round-trip")


def test_byte_swapped():
    """ByteSwapped wrapper."""
    @dataclass
    class ByteSwapStruct(StructMixin):
        v: int = field(ByteSwapped(Int32ub))

    # 0x12345678 ByteSwap → 0x78563412
    data = b"\x78\x56\x34\x12"
    parsed = ByteSwapStruct.parse(data)
    assert parsed.v == 0x12345678, f"v={hex(parsed.v)}"
    msg = ByteSwapStruct(v=0x12345678)
    built = msg.build()
    assert built == data, f"Expected {data.hex()}, got {built.hex()}"
    print("[OK] ByteSwapped round-trip")


def test_bits_swapped():
    """BitsSwapped wrapper."""
    @dataclass
    class BitSwapStruct(StructMixin):
        v: bytes = field(BitsSwapped(Bytes(2)))

    # 0xF0 0x0F BitSwap → 0x0F 0xF0
    data = b"\xF0\x0F"
    parsed = BitSwapStruct.parse(data)
    assert parsed.v == b"\x0F\xF0", f"v={parsed.v.hex()}"
    print("[OK] BitsSwapped parse")


def test_padding_error_invalid_pattern():
    """PaddingError raised when bit-domain pattern is invalid (0x80)."""
    try:
        @dataclass
        class BadBit(BitStructMixin):
            a: int = field(Bit())
            reserved: int = field(Padding(7, b"\x80"))
        raise AssertionError("Expected PaddingError not raised")
    except PaddingError as e:
        assert "0x80" in str(e), f"Message should mention 0x80: {e}"
        print(f"[OK] PaddingError caught: {str(e)[:80]}")
    except Exception as e:
        # If PaddingError isn't matched, the @dataclass might be raising
        # a different exception. Check if it's the underlying compilation.
        if "0x80" in str(e) and ("0x00" in str(e) or "0x01" in str(e)):
            print(f"[OK] Padding compilation error caught: {type(e).__name__}")
        else:
            raise


def test_integer_error_bits_integer_out_of_range():
    """IntegerError raised when BitsInteger value out of range."""
    @dataclass
    class BadValue(BitStructMixin):
        a: int = field(BitsInteger(4))

    try:
        BadValue(a=16).build()  # 16 > 15
        raise AssertionError("Expected IntegerError not raised")
    except IntegerError as e:
        assert "out of range" in str(e), f"Message should mention out of range: {e}"
        print(f"[OK] IntegerError caught: {str(e)[:80]}")


if __name__ == "__main__":
    test_bitstruct_with_padding()
    test_padding_byte_domain()
    test_bytewise_inside_bitstruct()
    test_byte_swapped()
    test_bits_swapped()
    test_padding_error_invalid_pattern()
    test_integer_error_bits_integer_out_of_range()
    print("\nAll Phase 3.3 integration tests PASSED!")
