"""Unit tests for Phase 10 sub-task 10.9: string constructs + gallery + lib.

Covers PaddedString/CString (PyO3), PascalString/GreedyString (pure Python),
Gallery parsers, lib submodules (binary/hex/bitstream/py3compat), and
debug stubs.
"""

import io
import pytest

import construct_rust
from construct_rust import (
    PaddedString, CString, PascalString, GreedyString,
    VarInt, Byte, Struct, Probe, Debugger,
    StringError, SizeofError, PaddingError,
)
from construct_rust.lib import binary, hex as hexmod, bitstream, py3compat


# =========================================================================
# PaddedString (PyO3)
# =========================================================================

class TestPaddedString:
    def test_parse_strips_padding(self):
        d = PaddedString(10, "utf8")
        assert d.parse(b"hello\x00\x00\x00\x00\x00") == "hello"

    def test_build_pads_to_length(self):
        d = PaddedString(10, "utf8")
        assert d.build("hello") == b"hello\x00\x00\x00\x00\x00"

    def test_sizeof(self):
        assert PaddedString(10, "utf8").sizeof() == 10

    def test_encoding_alias_utf_8(self):
        d = PaddedString(5, "utf_8")
        assert d.parse(b"hi\x00\x00\x00") == "hi"

    def test_build_too_long_raises(self):
        d = PaddedString(3, "utf8")
        with pytest.raises(Exception):
            d.build("hello")

    def test_all_null_returns_empty(self):
        d = PaddedString(4, "utf8")
        assert d.parse(b"\x00\x00\x00\x00") == ""

    def test_unknown_encoding_raises(self):
        with pytest.raises(StringError):
            PaddedString(10, "unknown")

    def test_callable_length_fallback(self):
        """PM decision 方案 A: callable length falls back to Python macro."""
        d = Struct(
            "n" / Byte,
            "s" / PaddedString(lambda ctx: ctx.n, "utf8"),
        )
        result = d.parse(b"\x05hello")
        assert result["s"] == "hello"


# =========================================================================
# CString (PyO3)
# =========================================================================

class TestCString:
    def test_parse_basic(self):
        d = CString("utf8")
        assert d.parse(b"hello\x00world") == "hello"

    def test_build_basic(self):
        d = CString("utf8")
        assert d.build("hello") == b"hello\x00"

    def test_sizeof_raises(self):
        with pytest.raises(SizeofError):
            CString("utf8").sizeof()

    def test_utf16_be(self):
        d = CString("utf_16_be")
        # "AB\0" in UTF-16 BE: 00 41 00 42 00 00
        assert d.parse(b"\x00\x41\x00\x42\x00\x00") == "AB"

    def test_eof_before_terminator_raises(self):
        d = CString("utf8")
        with pytest.raises(Exception):
            d.parse(b"hello")


# =========================================================================
# PascalString (pure Python)
# =========================================================================

class TestPascalString:
    def test_varint_utf8_roundtrip(self):
        d = PascalString(VarInt, "utf8")
        assert d.build("hello") == b"\x05hello"
        assert d.parse(b"\x05hello") == "hello"

    def test_empty_string(self):
        d = PascalString(VarInt, "utf8")
        assert d.build("") == b"\x00"
        assert d.parse(b"\x00") == ""

    def test_nested_in_struct(self):
        d = Struct("name" / PascalString(VarInt, "utf8"))
        result = d.parse(b"\x05hello")
        assert result["name"] == "hello"
        assert d.build({"name": "hello"}) == b"\x05hello"

    def test_build_then_parse(self):
        d = PascalString(VarInt, "utf8")
        data = d.build("world")
        assert d.parse(data) == "world"


# =========================================================================
# GreedyString (pure Python)
# =========================================================================

class TestGreedyString:
    def test_parse_utf8(self):
        d = GreedyString("utf8")
        assert d.parse(b"hello") == "hello"

    def test_empty_stream(self):
        d = GreedyString("utf8")
        assert d.parse(b"") == ""

    def test_build_non_str_raises(self):
        d = GreedyString("utf8")
        with pytest.raises(StringError):
            d.build(42)


# =========================================================================
# Gallery
# =========================================================================

class TestGallery:
    def test_elf_factory(self):
        from construct_rust.gallery import elf
        d = elf()
        with pytest.raises(Exception):
            d.parse(b"NOT-ELF")

    def test_pe32file_factory(self):
        from construct_rust.gallery import pe32file
        d = pe32file()
        with pytest.raises(Exception):
            d.parse(b"XXXX")

    def test_ut_index_factory(self):
        from construct_rust.gallery import UTIndex
        d = UTIndex()
        assert d.parse(b"\x01") == 1

    def test_gallery_submodule_import(self):
        from construct_rust.gallery.elf import elf
        from construct_rust.gallery.pe32coff import pe32file
        from construct_rust.gallery.ut_index import UTIndex
        assert elf is not None
        assert pe32file is not None
        assert UTIndex is not None


# =========================================================================
# lib/binary
# =========================================================================

class TestLibBinary:
    def test_integer2bits(self):
        assert binary.integer2bits(19, 8) == b"\x00\x00\x00\x01\x00\x00\x01\x01"

    def test_integer2bits_width_zero(self):
        with pytest.raises(ValueError):
            binary.integer2bits(0, 0)

    def test_integer2bytes(self):
        assert binary.integer2bytes(19, 4) == b"\x00\x00\x00\x13"

    def test_bits2integer(self):
        assert binary.bits2integer(b"\x01\x00\x00\x01\x01") == 19

    def test_bits2integer_signed(self):
        assert binary.bits2integer(b"\x01\x00", signed=True) == -2

    def test_bytes2integer(self):
        assert binary.bytes2integer(b"\x00\x00\x00\x13") == 19

    def test_bytes2bits_bits2bytes_roundtrip(self):
        data = b"hello"
        bits = binary.bytes2bits(data)
        assert binary.bits2bytes(bits) == data

    def test_swapbytes(self):
        assert binary.swapbytes(b"abcd") == b"dcba"

    def test_swapbitsinbytes(self):
        assert binary.swapbitsinbytes(b"\xf0\x00") == b"\x0f\x00"

    def test_hexlify_unhexlify(self):
        assert binary.unhexlify(binary.hexlify(b"abc")) == b"abc"

    def test_bits2integer_empty_raises(self):
        with pytest.raises(ValueError):
            binary.bits2integer(b"")

    def test_bits2bytes_bad_length_raises(self):
        with pytest.raises(ValueError):
            binary.bits2bytes(b"\x01\x00")


# =========================================================================
# lib/hex
# =========================================================================

class TestLibHex:
    def test_hexdump_hexundump_roundtrip(self):
        data = b"0" * 32
        dumped = hexmod.hexdump(data, 16)
        undumped = hexmod.hexundump(dumped, 16)
        assert undumped == data


# =========================================================================
# lib/bitstream
# =========================================================================

class TestLibBitstream:
    def test_restreamed_bytesio_read(self):
        sub = io.BytesIO(b"abc")
        # Identity decoder/encoder (passthrough)
        rb = bitstream.RestreamedBytesIO(
            sub, lambda d: d, 1, lambda d: d, 1
        )
        assert rb.read(3) == b"abc"

    def test_rebuffered_bytesio_read(self):
        sub = io.BytesIO(b"hello world")
        rb = bitstream.RebufferedBytesIO(sub)
        assert rb.read(5) == b"hello"


# =========================================================================
# lib/py3compat
# =========================================================================

class TestLibPy3compat:
    def test_int2byte_byte2int_roundtrip(self):
        for i in range(256):
            assert py3compat.byte2int(py3compat.int2byte(i)) == i

    def test_str2bytes_bytes2str_roundtrip(self):
        assert py3compat.bytes2str(py3compat.str2bytes("hello")) == "hello"


# =========================================================================
# debug stubs
# =========================================================================

class TestDebugStubs:
    def test_probe_instantiable(self):
        p = Probe()
        assert p is not None

    def test_debugger_instantiable(self):
        d = Debugger(Byte)
        assert d is not None

    def test_probe_parse_does_not_crash(self, capsys):
        p = Probe()
        d = Struct("x" / Byte, Probe())
        result = d.parse(b"\x05")
        assert result["x"] == 5

    def test_debugger_build_works(self):
        """Debugger delegates to subcon via dual-path; successful build works."""
        d = Debugger(Byte)
        assert d.build(42) == b"\x2a"

    def test_debugger_parse_works(self):
        """Debugger parse delegates to subcon via dual-path."""
        d = Debugger(Byte)
        assert d.parse(b"\x2a") == 42
