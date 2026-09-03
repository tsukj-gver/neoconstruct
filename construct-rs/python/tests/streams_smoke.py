"""Streams parity smoke test.

All descriptors are wrapped in Struct (construct-rs limitation: standalone
parse/build on descriptors not supported; only StructMixin subclasses are
parseable/buildable directly).
"""
from construct import (
    StructMixin, field, rfield, Bytes, Byte, Int16ub,
    GreedyBytes, VarInt, GreedyRange, Int32ul,
    Seek, Pointer, Prefixed,
    CompilationError,
)
from dataclasses import dataclass


def _wrap_single(descriptor):
    """Wrap a single descriptor in a Struct for standalone parse/build."""
    @dataclass
    class _S(StructMixin):
        x: object = field(descriptor)
    return _S


def test_pointer_parse_positive():
    """Pointer(8, Bytes(1)) reads byte at position 8."""
    S = _wrap_single(Pointer(8, Bytes(1)))
    result = S.parse(b"abcdefghijkl")
    assert result.x == b"i", f"got {result.x!r}"
    print(f"PASS: test_pointer_parse_positive (x={result.x!r})")


def test_pointer_build_zero_pad():
    """Pointer(8, Bytes(1)).build(b'Z') zero-pads to position 8."""
    S = _wrap_single(Pointer(8, Bytes(1)))
    result = S.build(S(x=b"Z"))
    assert result == b"\x00" * 8 + b"Z", f"got {result!r}"
    print(f"PASS: test_pointer_build_zero_pad")


def test_pointer_negative_offset():
    """Pointer(-2, Bytes(1)) reads 2 bytes before EOF."""
    S = _wrap_single(Pointer(-2, Bytes(1)))
    result = S.parse(b"abcdefgh")
    assert result.x == b"g", f"got {result.x!r}"
    print(f"PASS: test_pointer_negative_offset (x={result.x!r})")


def test_pointer_in_struct_no_advance():
    """Struct(ptr=Pointer, direct=Bytes): direct reads from pos 0 (Pointer seeked back)."""
    @dataclass
    class P(StructMixin):
        ptr: bytes = field(Pointer(5, Bytes(1)))
        direct: bytes = field(Bytes(1))

    result = P.parse(b"01234Xy")
    assert result.ptr == b"X", f"ptr got {result.ptr!r}"
    assert result.direct == b"0", f"direct got {result.direct!r}"
    print(f"PASS: test_pointer_in_struct_no_advance")


def test_pointer_relative_offset():
    """Pointer(offset, subcon, relativeOffset=True) seeks relative to current."""
    @dataclass
    class P(StructMixin):
        head: bytes = field(Bytes(3))  # consume 3 bytes -> tell=3
        ptr: bytes = field(Pointer(2, Bytes(1), relativeOffset=True))  # seek to 3+2=5
        tail: bytes = field(Bytes(1))  # read at pos 3 (Pointer seeked back)

    result = P.parse(b"abcdefg")
    assert result.head == b"abc", f"head got {result.head!r}"
    assert result.ptr == b"f", f"ptr got {result.ptr!r}"
    assert result.tail == b"d", f"tail got {result.tail!r}"
    print(f"PASS: test_pointer_relative_offset")


def test_prefixed_parse_basic():
    """Prefixed(Byte, Bytes(3)) reads 3 bytes after length prefix."""
    S = _wrap_single(Prefixed(Byte, Bytes(3)))
    result = S.parse(b"\x03abc")
    assert result.x == b"abc", f"got {result.x!r}"
    print(f"PASS: test_prefixed_parse_basic")


def test_prefixed_build_basic():
    """Prefixed(Byte, Bytes(3)).build writes length prefix then data."""
    S = _wrap_single(Prefixed(Byte, Bytes(3)))
    result = S.build(S(x=b"abc"))
    assert result == b"\x03abc", f"got {result!r}"
    print(f"PASS: test_prefixed_build_basic")


def test_prefixed_round_trip():
    """Prefixed build -> parse returns original data."""
    S = _wrap_single(Prefixed(Byte, Bytes(3)))
    data = S.build(S(x=b"xyz"))
    parsed = S.parse(data)
    assert parsed.x == b"xyz", f"got {parsed.x!r}"
    print(f"PASS: test_prefixed_round_trip")


def test_prefixed_includelength():
    """Prefixed(includelength=True): length includes lengthfield size."""
    S = _wrap_single(Prefixed(Int16ub, Bytes(3), includelength=True))
    # lengthfield=5 = 3 bytes data + 2 bytes lengthfield
    result = S.parse(b"\x00\x05abc")
    assert result.x == b"abc", f"got {result.x!r}"
    print(f"PASS: test_prefixed_includelength")


def test_prefixed_subconsumes_less():
    """subcon consumes less than length: rest of substream ignored."""
    S = _wrap_single(Prefixed(Byte, Bytes(1)))
    result = S.parse(b"\x05abcde")
    assert result.x == b"a", f"got {result.x!r}"
    print(f"PASS: test_prefixed_subconsumes_less")


def test_prefixed_varint_greedyrange():
    """Prefixed(VarInt, GreedyRange(Int32ul)) - common pattern."""
    S = _wrap_single(Prefixed(VarInt, GreedyRange(Int32ul)))
    # VarInt encodes byte count = 8 (2 x Int32ul = 4 bytes each)
    # Then 8 bytes of Int32ul data
    result = S.parse(b"\x08abcdefgh")
    assert len(result.x) == 2, f"got {result.x!r}"
    print(f"PASS: test_prefixed_varint_greedyrange (len={len(result.x)})")


def test_prefixed_greedybytes():
    """Prefixed(Byte, GreedyBytes) - read N bytes as bytes."""
    S = _wrap_single(Prefixed(Byte, GreedyBytes))
    result = S.parse(b"\x05hello")
    assert result.x == b"hello", f"got {result.x!r}"
    print(f"PASS: test_prefixed_greedybytes")


def test_pointer_build_overwrite_in_struct():
    """Pointer in Struct: build overwrites position, other fields write their own."""
    @dataclass
    class P(StructMixin):
        ptr: bytes = field(Pointer(2, Bytes(1)))
        direct: bytes = field(Bytes(1))

    # build: ptr first (seek 2, write Z, seek back), then direct (write X at pos 0)
    # Result: buf = ['X', 0, 'Z']
    result = P.build(P(ptr=b"Z", direct=b"X"))
    assert result[0:1] == b"X", f"pos 0 got {result!r}"
    assert result[2:3] == b"Z", f"pos 2 got {result!r}"
    print(f"PASS: test_pointer_build_overwrite_in_struct (result={result!r})")


def test_seek_in_struct():
    """Seek in Struct: moves stream position."""
    @dataclass
    class P(StructMixin):
        head: bytes = field(Bytes(0))  # consume 0 bytes
        seek_pos: int = rfield(Seek(5))
        tail: bytes = field(Bytes(1))  # read at pos 5

    result = P.parse(b"01234x")
    assert result.seek_pos == 5, f"seek_pos got {result.seek_pos!r}"
    assert result.tail == b"x", f"tail got {result.tail!r}"
    print(f"PASS: test_seek_in_struct")


def test_seek_whence_end_in_struct():
    """Seek with whence=2 from EOF."""
    @dataclass
    class P(StructMixin):
        seek_pos: int = rfield(Seek(-2, 2))
        tail: bytes = field(Bytes(1))

    result = P.parse(b"abcdefgh")
    assert result.seek_pos == 6, f"seek_pos got {result.seek_pos!r}"
    assert result.tail == b"g", f"tail got {result.tail!r}"
    print(f"PASS: test_seek_whence_end_in_struct")


# ---------------------------------------------------------------------------
# parity 补充：Pointer(stream=) 编译期拒绝
# ---------------------------------------------------------------------------


def test_pointer_stream_non_none_rejected():
    """Pointer(stream=非 None) 编译期拒绝（已知限制）。

    Python construct 允许 Pointer 换流（stream=context lambda）；
    construct-rs 不支持，编译期抛 CompilationError。
    """
    try:
        @dataclass
        class P(StructMixin):
            x: bytes = field(Pointer(8, Bytes(1), stream=lambda ctx: None))
        # 若 __init_subclass__ 没抛，显式失败
        print("FAIL: Pointer(stream=非 None) 应该编译期拒绝")
        return
    except CompilationError as e:
        msg = str(e)
        # 错误消息含 "stream" 提示
        assert "stream" in msg.lower() or "Pointer" in msg, \
            f"错误消息应含 'stream' 或 'Pointer'，实际：{msg!r}"
        print(f"PASS: test_pointer_stream_non_none_rejected (err={msg[:80]!r})")
        return
    except Exception as e:
        print(f"FAIL: 期望 CompilationError，实际 {type(e).__name__}: {e!r}")
        return


if __name__ == "__main__":
    tests = [
        test_pointer_parse_positive,
        test_pointer_build_zero_pad,
        test_pointer_negative_offset,
        test_pointer_in_struct_no_advance,
        test_pointer_relative_offset,
        test_pointer_build_overwrite_in_struct,
        test_seek_in_struct,
        test_seek_whence_end_in_struct,
        test_prefixed_parse_basic,
        test_prefixed_build_basic,
        test_prefixed_round_trip,
        test_prefixed_includelength,
        test_prefixed_subconsumes_less,
        test_prefixed_varint_greedyrange,
        test_prefixed_greedybytes,
        test_pointer_stream_non_none_rejected,
    ]
    passed = 0
    failed = 0
    for t in tests:
        try:
            t()
            passed += 1
        except Exception as e:
            print(f"FAIL: {t.__name__}: {e!r}")
            failed += 1
    print()
    print(f"{passed} passed, {failed} failed")
    if failed:
        exit(1)
