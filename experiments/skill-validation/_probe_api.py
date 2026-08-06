"""Minimal probes to verify SKILL API behavior before committing to SCTP design.
Run only with construct-rs venv.
"""
from dataclasses import dataclass
from construct import (
    StructMixin, BitStructMixin, field, rfield, wfield,
    Int8ub, Int16ub, Int32ub, Bytes, GreedyBytes,
    Bit, BitsInteger,
    Switch, GreedyRange, Prefixed,
    Array,
)


# Probe 1: Prefixed with includelength=True
# According to SKILL: "lengthfield 是字节计数"
# Test: does includelength=True mean substream = length_value - len(lengthfield_bytes)?
@dataclass
class InnerProbe(StructMixin):
    type_byte: int = field(Int8ub)
    flag_byte: int = field(Int8ub)
    payload: bytes = field(GreedyBytes)


@dataclass
class OuterProbe(StructMixin):
    chunk: InnerProbe = field(Prefixed(Int16ub, InnerProbe, includelength=True))


def probe_prefixed_includelength():
    """Build a chunk: type=0xAB, flag=0xCD, payload=0x1122.
    Expected chunk_length (含 type+flags+length+payload) = 2 + 2 + 2 = 6.
    So bytes should be: 00 06 AB CD 11 22."""
    inner = InnerProbe(type_byte=0xAB, flag_byte=0xCD, payload=b"\x11\x22")
    outer = OuterProbe(chunk=inner)
    built = outer.build()
    print(f"[Probe1 Prefixed includelength=True] built={built.hex()}")
    # Round-trip
    parsed = OuterProbe.parse(built)
    print(f"  parsed.chunk.type_byte=0x{parsed.chunk.type_byte:02X}, "
          f"flag_byte=0x{parsed.chunk.flag_byte:02X}, payload={parsed.chunk.payload.hex()}")
    assert parsed.chunk.type_byte == 0xAB
    assert parsed.chunk.flag_byte == 0xCD
    assert parsed.chunk.payload == b"\x11\x22"
    print("  PASS")


# Probe 2: GreedyRange of Prefixed
@dataclass
class MultiChunk(StructMixin):
    chunks: list = field(GreedyRange(Prefixed(Int16ub, InnerProbe, includelength=True)))


def probe_greedy_range_prefixed():
    """Build two chunks, then parse."""
    chunks_data = [
        InnerProbe(type_byte=0x01, flag_byte=0x00, payload=b"\xAA"),
        InnerProbe(type_byte=0x02, flag_byte=0x00, payload=b"\xBB\xCC"),
    ]
    mc = MultiChunk(chunks=chunks_data)
    built = mc.build()
    print(f"[Probe2 GreedyRange+Prefixed] built={built.hex()}")
    parsed = MultiChunk.parse(built)
    print(f"  parsed chunks count={len(parsed.chunks)}")
    assert len(parsed.chunks) == 2
    assert parsed.chunks[0].type_byte == 0x01
    assert parsed.chunks[0].payload == b"\xAA"
    assert parsed.chunks[1].type_byte == 0x02
    assert parsed.chunks[1].payload == b"\xBB\xCC"
    print("  PASS")


# Probe 3: BitStruct with 8-bit total
@dataclass
class FlagsProbe(BitStructMixin):
    reserved: int = field(BitsInteger(5))
    u: int = field(Bit())
    b: int = field(Bit())
    e: int = field(Bit())


def probe_bitstruct_8bit():
    """DATA chunk flags pattern: reserved=0, u=1, b=0, e=1
    MSB first: 00000 1 0 1 = 0b00000101 = 0x05."""
    flags = FlagsProbe(reserved=0, u=1, b=0, e=1)
    built = flags.build()
    print(f"[Probe3 BitStruct 8-bit] built={built.hex()} (expected 05)")
    assert built == b"\x05", f"expected 05, got {built.hex()}"
    parsed = FlagsProbe.parse(b"\x05")
    assert parsed.u == 1 and parsed.b == 0 and parsed.e == 1
    print("  PASS")


# Probe 4: Switch dispatch
@dataclass
class TypeA(StructMixin):
    a_val: int = field(Int8ub)


@dataclass
class TypeB(StructMixin):
    b_val: int = field(Int16ub)


@dataclass
class SwitchProbe(StructMixin):
    type_code: int = field(Int8ub)
    body: object = field(Switch(type_code, {1: TypeA, 2: TypeB}))


def probe_switch():
    """type=1 -> TypeA (1B), type=2 -> TypeB (2B)."""
    p1 = SwitchProbe(type_code=1, body=TypeA(a_val=0xAA))
    built1 = p1.build()
    print(f"[Probe4 Switch type=1] built={built1.hex()} (expected 01aa)")
    parsed1 = SwitchProbe.parse(built1)
    assert isinstance(parsed1.body, TypeA)
    assert parsed1.body.a_val == 0xAA

    p2 = SwitchProbe(type_code=2, body=TypeB(b_val=0xBBCC))
    built2 = p2.build()
    print(f"[Probe4 Switch type=2] built={built2.hex()} (expected 02bbcc)")
    parsed2 = SwitchProbe.parse(built2)
    assert isinstance(parsed2.body, TypeB)
    assert parsed2.body.b_val == 0xBBCC
    print("  PASS")


# Probe 5: BitStruct + Switch inside Prefixed (full SCTP chunk simulation)
@dataclass
class DataInner(StructMixin):
    chunk_type: int = field(Int8ub)
    flags_raw: int = field(Int8ub)
    body: int = field(Int8ub)


@dataclass
class InitInner(StructMixin):
    chunk_type: int = field(Int8ub)
    flags_raw: int = field(Int8ub)
    body: int = field(Int16ub)


@dataclass
class ChunkProbe(StructMixin):
    chunk_type: int = field(Int8ub)
    flags_raw: int = field(Int8ub)
    body: object = field(Switch(chunk_type, {0: Int8ub, 1: Int16ub}))


@dataclass
class PacketProbe(StructMixin):
    chunks: list = field(GreedyRange(Prefixed(Int16ub, ChunkProbe, includelength=True)))


def probe_full_chunk_sim():
    """One DATA-like chunk (type=0, flags=0xAB, body=1B), one INIT-like (type=1, flags=0, body=2B)."""
    p = PacketProbe(chunks=[
        ChunkProbe(chunk_type=0, flags_raw=0xAB, body=0xCD),
        ChunkProbe(chunk_type=1, flags_raw=0x00, body=0x1234),
    ])
    built = p.build()
    print(f"[Probe5 PacketProbe] built={built.hex()}")
    # Chunk 1: length=2+1+1+1=5 -> 0005 00 AB CD
    # Chunk 2: length=2+1+1+2=6 -> 0006 01 00 1234
    expected = "000500abc d000601001234".replace(" ", "")
    print(f"  expected={expected}")
    parsed = PacketProbe.parse(built)
    assert len(parsed.chunks) == 2
    assert parsed.chunks[0].chunk_type == 0
    assert parsed.chunks[0].body == 0xCD
    assert parsed.chunks[1].chunk_type == 1
    assert parsed.chunks[1].body == 0x1234
    print("  PASS")


if __name__ == "__main__":
    print("=== SKILL API Probes (construct-rs) ===")
    print()
    try:
        probe_prefixed_includelength()
    except Exception as e:
        print(f"  FAIL: {type(e).__name__}: {e}")

    try:
        probe_greedy_range_prefixed()
    except Exception as e:
        print(f"  FAIL: {type(e).__name__}: {e}")

    try:
        probe_bitstruct_8bit()
    except Exception as e:
        print(f"  FAIL: {type(e).__name__}: {e}")

    try:
        probe_switch()
    except Exception as e:
        print(f"  FAIL: {type(e).__name__}: {e}")

    try:
        probe_full_chunk_sim()
    except Exception as e:
        print(f"  FAIL: {type(e).__name__}: {e}")
