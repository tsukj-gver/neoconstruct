"""Verify the code examples added to SKILL v2 §4.5.5 / §4.5.6 / §6.6 actually run.

This guards against byte-value miscalculation in the assert statements.
"""
import sys


def test_section_455_prefixed_includelength():
    """§4.5.5 对照示例：includelength False vs True 的字节差异。
    Prefixed 是描述符，必须用 @dataclass 包装后才能 build/parse。
    """
    from dataclasses import dataclass
    from construct import StructMixin, field, Prefixed, Int16ub, GreedyBytes

    payload = b"\xAA\xBB\xCC"

    # 模式 A：includelength=False（默认）
    @dataclass
    class ChunkA(StructMixin):
        data: bytes = field(Prefixed(Int16ub, GreedyBytes))

    a = ChunkA(data=payload)
    built_a = a.build()
    assert built_a == b"\x00\x03\xAA\xBB\xCC", "§4.5.5 mode A build mismatch: {}".format(built_a.hex())
    assert ChunkA.parse(built_a).data == payload

    # 模式 B：includelength=True
    @dataclass
    class ChunkB(StructMixin):
        data: bytes = field(Prefixed(Int16ub, GreedyBytes, includelength=True))

    b_inst = ChunkB(data=payload)
    built_b = b_inst.build()
    assert built_b == b"\x00\x05\xAA\xBB\xCC", "§4.5.5 mode B build mismatch: {}".format(built_b.hex())
    assert ChunkB.parse(built_b).data == payload

    print("[OK] §4.5.5 Prefixed includelength 对照示例 (built_a={}, built_b={})".format(
        built_a.hex(), built_b.hex()))


def test_section_456_sctp_chunks():
    """§4.5.6 简化版 SCTP chunk 序列：DATA + ABORT 的 build 字节。"""
    from dataclasses import dataclass
    from construct import (
        StructMixin, field, Int8ub, Int16ub, Int32ub, GreedyBytes,
        Prefixed, GreedyRange, Switch,
    )

    @dataclass
    class DataChunkValue(StructMixin):
        tsn: int = field(Int32ub)
        user_data: bytes = field(GreedyBytes)

    @dataclass
    class AbortChunkValue(StructMixin):
        error_causes: bytes = field(GreedyBytes)

    @dataclass
    class ChunkInner(StructMixin):
        chunk_type: int = field(Int8ub)
        flags: int = field(Int8ub)
        value: object = field(Switch(chunk_type, {
            0: DataChunkValue,
            6: AbortChunkValue,
        }))

    @dataclass
    class SCTPChunks(StructMixin):
        chunks: list = field(GreedyRange(
            Prefixed(Int16ub, ChunkInner, includelength=True)
        ))

    packet = SCTPChunks(chunks=[
        ChunkInner(chunk_type=0, flags=0x03,
                   value=DataChunkValue(tsn=1, user_data=b"hello")),
        ChunkInner(chunk_type=6, flags=0x00,
                   value=AbortChunkValue(error_causes=b"")),
    ])
    built = packet.build()
    expected = (
        b"\x00\x0D"
        b"\x00\x03"
        b"\x00\x00\x00\x01"
        b"hello"
        b"\x00\x04"
        b"\x06\x00"
    )
    assert built == expected, "§4.5.6 build mismatch:\n  got:      {}\n  expected: {}".format(
        built.hex(), expected.hex())

    parsed = SCTPChunks.parse(built)
    assert len(parsed.chunks) == 2
    assert isinstance(parsed.chunks[0].value, DataChunkValue)
    assert parsed.chunks[0].value.tsn == 1
    assert parsed.chunks[0].value.user_data == b"hello"
    assert isinstance(parsed.chunks[1].value, AbortChunkValue)
    assert parsed.build() == built

    print("[OK] §4.5.6 SCTP chunk 序列 (built={})".format(built.hex()))


def test_section_66_full_sctp():
    """§6.6 完整 SCTP packet（Common Header + DATA chunk）round-trip。"""
    from dataclasses import dataclass
    from construct import (
        StructMixin, BitStructMixin, field,
        Int8ub, Int16ub, Int32ub, Bytes, GreedyBytes,
        Bit, BitsInteger,
        Prefixed, GreedyRange, Switch,
    )

    @dataclass
    class DataChunkFlags(BitStructMixin):
        reserved: int = field(BitsInteger(5))
        u: int = field(Bit())
        b: int = field(Bit())
        e: int = field(Bit())

    @dataclass
    class DataChunkValue(StructMixin):
        tsn: int = field(Int32ub)
        stream_id: int = field(Int16ub)
        stream_seq: int = field(Int16ub)
        ppid: int = field(Int32ub)
        user_data: bytes = field(GreedyBytes)

    @dataclass
    class AbortChunkValue(StructMixin):
        error_causes: bytes = field(GreedyBytes)

    @dataclass
    class ChunkInner(StructMixin):
        chunk_type: int = field(Int8ub)
        flags_raw: int = field(Int8ub)
        value: object = field(Switch(chunk_type, {
            0: DataChunkValue,
            6: AbortChunkValue,
        }))

    @dataclass
    class SCTPPacket(StructMixin):
        src_port: int = field(Int16ub)
        dst_port: int = field(Int16ub)
        verify_tag: int = field(Int32ub)
        checksum: bytes = field(Bytes(4))
        chunks: list = field(GreedyRange(
            Prefixed(Int16ub, ChunkInner, includelength=True)
        ))

    data_chunk = ChunkInner(
        chunk_type=0,
        flags_raw=0x03,
        value=DataChunkValue(
            tsn=1, stream_id=0, stream_seq=0, ppid=0,
            user_data=b"hello",
        ),
    )
    packet = SCTPPacket(
        src_port=1234, dst_port=5678, verify_tag=0,
        checksum=b"\x00\x00\x00\x00",
        chunks=[data_chunk],
    )
    built = packet.build()

    parsed = SCTPPacket.parse(built)
    assert parsed.src_port == 1234
    assert len(parsed.chunks) == 1
    assert isinstance(parsed.chunks[0].value, DataChunkValue)
    assert parsed.chunks[0].value.user_data == b"hello"
    assert parsed.build() == built

    # 事后解码 DATA flags
    flags = DataChunkFlags.parse(bytes([parsed.chunks[0].flags_raw]))
    assert flags.b == 1 and flags.e == 1
    # 验证 flags_raw=0x03 的 bit 分解：0x03 = 0b00000011 -> reserved=0, u=0, b=1, e=1
    assert flags.reserved == 0 and flags.u == 0

    print("[OK] §6.6 完整 SCTP packet round-trip (built={})".format(built.hex()))


def main():
    try:
        test_section_455_prefixed_includelength()
    except Exception as e:
        print("[FAIL] §4.5.5: {}: {}".format(type(e).__name__, e))
        sys.exit(1)

    try:
        test_section_456_sctp_chunks()
    except Exception as e:
        print("[FAIL] §4.5.6: {}: {}".format(type(e).__name__, e))
        sys.exit(1)

    try:
        test_section_66_full_sctp()
    except Exception as e:
        print("[FAIL] §6.6: {}: {}".format(type(e).__name__, e))
        sys.exit(1)

    print("\nALL TESTS PASS — SKILL v2 新增示例字节值与 construct-rs 实际行为一致")


if __name__ == "__main__":
    main()
