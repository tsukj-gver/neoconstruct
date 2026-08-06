"""Tests for construct-rs SCTP implementation.

Run with: crs venv python.
Tests:
  1. Round-trip parse -> build for each chunk type
  2. BitStruct field decomposition (DATA flags)
  3. Switch conditional dispatch
  4. Multi-chunk packet (GreedyRange)
  5. CRC32c external verification
"""
import sys
import os

# Ensure we import from this directory, not from construct-rs/python/construct
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from sctp_crs import (
    SCTPPacket, ChunkInner, DataChunkValue, InitChunkValue, SackChunkValue,
    AbortChunkValue, GapAckBlock, DataChunkFlags,
    crc32c, verify_sctp_checksum, attach_checksum,
)

PASS_COUNT = 0
FAIL_COUNT = 0


def check(name, cond, detail=""):
    global PASS_COUNT, FAIL_COUNT
    if cond:
        PASS_COUNT += 1
        print(f"  [PASS] {name}")
    else:
        FAIL_COUNT += 1
        print(f"  [FAIL] {name} {detail}")


# ============================================================================
# Test 1: DATA chunk (BitStruct flags + parse + build)
# ============================================================================
def test_data_chunk_flags():
    """Verify DATA chunk flags BitStruct: u=1, b=0, e=1 -> 0x05."""
    print("\n[Test] DATA chunk flags BitStruct decomposition")
    flags = DataChunkFlags(reserved=0, u=1, b=0, e=1)
    built = flags.build()
    check("flags build = 0x05 (00000 1 0 1)", built == b"\x05", f"got {built.hex()}")

    parsed = DataChunkFlags.parse(b"\x05")
    check("parsed.reserved == 0", parsed.reserved == 0)
    check("parsed.u == 1", parsed.u == 1)
    check("parsed.b == 0", parsed.b == 0)
    check("parsed.e == 1", parsed.e == 1)

    # Another combo: B=1, E=1 (beginning + ending fragment = unfragmented)
    flags2 = DataChunkFlags(reserved=0, u=0, b=1, e=1)
    built2 = flags2.build()
    # 00000 0 1 1 = 0b00000011 = 0x03
    check("B=1, E=1 -> 0x03", built2 == b"\x03", f"got {built2.hex()}")


def test_data_chunk_round_trip():
    """Round-trip a DATA chunk wrapped in a minimal packet."""
    print("\n[Test] DATA chunk round-trip (parse + build)")
    data_chunk = ChunkInner(
        chunk_type=0,
        flags_raw=0x03,  # B=1, E=1 (unfragmented)
        value=DataChunkValue(
            tsn=0x12345678,
            stream_id=0x0001,
            stream_seq=0x0001,
            ppid=0x00000018,  # IUA payload protocol id (example)
            user_data=b"Hello SCTP!",
        ),
    )
    packet = SCTPPacket(
        src_port=1234,
        dst_port=5678,
        verify_tag=0xDEADBEEF,
        checksum=b"\x00\x00\x00\x00",  # placeholder
        chunks=[data_chunk],
    )
    built = packet.build()
    print(f"  built packet ({len(built)} bytes): {built.hex()}")

    parsed = SCTPPacket.parse(built)
    check("parsed.src_port == 1234", parsed.src_port == 1234)
    check("parsed.dst_port == 5678", parsed.dst_port == 5678)
    check("parsed.verify_tag == 0xDEADBEEF", parsed.verify_tag == 0xDEADBEEF)
    check("len(parsed.chunks) == 1", len(parsed.chunks) == 1)
    check("parsed.chunks[0].chunk_type == 0", parsed.chunks[0].chunk_type == 0)
    check("parsed.chunks[0].flags_raw == 0x03", parsed.chunks[0].flags_raw == 0x03)
    check("value is DataChunkValue",
          isinstance(parsed.chunks[0].value, DataChunkValue))
    check("parsed tsn == 0x12345678", parsed.chunks[0].value.tsn == 0x12345678)
    check("parsed user_data == 'Hello SCTP!'",
          parsed.chunks[0].value.user_data == b"Hello SCTP!")

    # Round-trip: build(parsed) == built
    rebuilt = parsed.build()
    check("round-trip build == original", rebuilt == built,
          f"\n    rebuilt={rebuilt.hex()}\n    built  ={built.hex()}")


# ============================================================================
# Test 2: INIT chunk (variable-length params via GreedyBytes)
# ============================================================================
def test_init_chunk():
    print("\n[Test] INIT chunk (Switch dispatch + GreedyBytes params)")
    init_chunk = ChunkInner(
        chunk_type=1,
        flags_raw=0x00,
        value=InitChunkValue(
            initiate_tag=0xCAFEBABE,
            a_rwnd=0x00010000,
            num_outbound_streams=10,
            num_inbound_streams=10,
            initial_tsn=0x00000001,
            params=b"\x00\x05\x00\x08\x0a\x00\x00\x01",  # bogus IPv4 param TLV
        ),
    )
    packet = SCTPPacket(
        src_port=2222, dst_port=3333, verify_tag=0,
        checksum=b"\x00\x00\x00\x00",
        chunks=[init_chunk],
    )
    built = packet.build()
    print(f"  built packet ({len(built)} bytes): {built.hex()}")

    parsed = SCTPPacket.parse(built)
    check("chunk_type == 1", parsed.chunks[0].chunk_type == 1)
    check("value is InitChunkValue",
          isinstance(parsed.chunks[0].value, InitChunkValue))
    check("initiate_tag == 0xCAFEBABE",
          parsed.chunks[0].value.initiate_tag == 0xCAFEBABE)
    check("num_outbound_streams == 10",
          parsed.chunks[0].value.num_outbound_streams == 10)
    check("params preserved",
          parsed.chunks[0].value.params == b"\x00\x05\x00\x08\x0a\x00\x00\x01")


# ============================================================================
# Test 3: SACK chunk (Array with count field reference)
# ============================================================================
def test_sack_chunk():
    print("\n[Test] SACK chunk (Array(count_field, subcon) pattern)")
    sack_chunk = ChunkInner(
        chunk_type=3,
        flags_raw=0x00,
        value=SackChunkValue(
            cum_tsn_ack=0x100,
            a_rwnd=0x00020000,
            num_gap_blocks=2,
            num_dup_tsns=1,
            gap_blocks=[
                GapAckBlock(start=1, end=2),
                GapAckBlock(start=5, end=6),
            ],
            dup_tsns=[0x200],
        ),
    )
    packet = SCTPPacket(
        src_port=4444, dst_port=5555, verify_tag=0x11112222,
        checksum=b"\x00\x00\x00\x00",
        chunks=[sack_chunk],
    )
    built = packet.build()
    print(f"  built packet ({len(built)} bytes): {built.hex()}")

    parsed = SCTPPacket.parse(built)
    check("chunk_type == 3", parsed.chunks[0].chunk_type == 3)
    check("value is SackChunkValue",
          isinstance(parsed.chunks[0].value, SackChunkValue))
    check("cum_tsn_ack == 0x100", parsed.chunks[0].value.cum_tsn_ack == 0x100)
    check("len(gap_blocks) == 2", len(parsed.chunks[0].value.gap_blocks) == 2)
    check("gap_blocks[0].start == 1", parsed.chunks[0].value.gap_blocks[0].start == 1)
    check("gap_blocks[1].end == 6", parsed.chunks[0].value.gap_blocks[1].end == 6)
    check("len(dup_tsns) == 1", len(parsed.chunks[0].value.dup_tsns) == 1)
    check("dup_tsns[0] == 0x200", parsed.chunks[0].value.dup_tsns[0] == 0x200)


# ============================================================================
# Test 4: ABORT chunk
# ============================================================================
def test_abort_chunk():
    print("\n[Test] ABORT chunk (unmapped value dispatch)")
    abort_chunk = ChunkInner(
        chunk_type=6,
        flags_raw=0x00,
        value=AbortChunkValue(
            error_causes=b"\x00\x0c\x00\x08Invalid Stream",  # bogus error TLV
        ),
    )
    packet = SCTPPacket(
        src_port=6666, dst_port=7777, verify_tag=0x33334444,
        checksum=b"\x00\x00\x00\x00",
        chunks=[abort_chunk],
    )
    built = packet.build()
    parsed = SCTPPacket.parse(built)
    check("chunk_type == 6", parsed.chunks[0].chunk_type == 6)
    check("value is AbortChunkValue",
          isinstance(parsed.chunks[0].value, AbortChunkValue))
    check("error_causes preserved",
          parsed.chunks[0].value.error_causes == b"\x00\x0c\x00\x08Invalid Stream")


# ============================================================================
# Test 5: Multi-chunk packet (GreedyRange)
# ============================================================================
def test_multi_chunk():
    print("\n[Test] Multi-chunk packet (GreedyRange of DATA + DATA)")
    chunks = [
        ChunkInner(chunk_type=0, flags_raw=0x03,
                   value=DataChunkValue(
                       tsn=0x01, stream_id=0, stream_seq=0, ppid=0,
                       user_data=b"chunk1")),
        ChunkInner(chunk_type=0, flags_raw=0x03,
                   value=DataChunkValue(
                       tsn=0x02, stream_id=0, stream_seq=1, ppid=0,
                       user_data=b"chunk2-data-longer")),
    ]
    packet = SCTPPacket(
        src_port=8888, dst_port=9999, verify_tag=0x55556666,
        checksum=b"\x00\x00\x00\x00",
        chunks=chunks,
    )
    built = packet.build()
    print(f"  built packet ({len(built)} bytes): {built.hex()}")

    parsed = SCTPPacket.parse(built)
    check("len(chunks) == 2", len(parsed.chunks) == 2)
    check("chunk[0].user_data == 'chunk1'",
          parsed.chunks[0].value.user_data == b"chunk1")
    check("chunk[1].user_data == 'chunk2-data-longer'",
          parsed.chunks[1].value.user_data == b"chunk2-data-longer")
    check("chunk[0].tsn == 1", parsed.chunks[0].value.tsn == 0x01)
    check("chunk[1].tsn == 2", parsed.chunks[1].value.tsn == 0x02)


# ============================================================================
# Test 6: Switch default branch (unknown chunk type -> value=None)
# ============================================================================
def test_unknown_chunk_type():
    print("\n[Test] Switch default branch (unknown chunk_type=99 -> value=None)")
    # NOTE: chunk must be fully consumed inside Prefixed substream.
    # With Switch default=Pass (no value bytes consumed), chunk_length must
    # equal 4 (type + flags + 2-byte length) -> substream = 2 bytes (type+flags).
    raw_chunk = b"\x00\x04\x63\x00"  # len=4, type=99, flags=0, no value bytes
    packet_bytes = (
        b"\x22\x22"  # src_port
        b"\x33\x33"  # dst_port
        b"\x44\x44\x44\x44"  # verify_tag
        b"\x00\x00\x00\x00"  # checksum (zero)
        + raw_chunk
    )
    parsed = SCTPPacket.parse(packet_bytes)
    check("len(chunks) == 1", len(parsed.chunks) == 1)
    check("chunk_type == 99", parsed.chunks[0].chunk_type == 99)
    check("value is None (Switch default=Pass)",
          parsed.chunks[0].value is None)


# ============================================================================
# Test 7: CRC32c checksum external computation
# ============================================================================
def test_checksum():
    print("\n[Test] CRC32c external checksum (compute + verify)")
    # A minimal packet
    packet = SCTPPacket(
        src_port=1234, dst_port=5678, verify_tag=0xDEADBEEF,
        checksum=b"\x00\x00\x00\x00",
        chunks=[ChunkInner(chunk_type=0, flags_raw=0x03,
                            value=DataChunkValue(
                                tsn=0x12345678, stream_id=1, stream_seq=1,
                                ppid=0, user_data=b"test"))],
    )
    built_zero = packet.build()
    crc = compute_sctp_checksum(built_zero)
    print(f"  computed CRC32c = {crc.hex()}")

    # Attach and verify
    with_crc = attach_checksum(built_zero)
    check("verify_sctp_checksum(with_crc) == True",
          verify_sctp_checksum(with_crc) is True)

    # Tamper -> verify should fail
    tampered = bytearray(with_crc)
    tampered[12] ^= 0xFF  # flip a byte in the DATA chunk
    check("verify_sctp_checksum(tampered) == False",
          verify_sctp_checksum(bytes(tampered)) is False)

    # Round-trip: parse the with_crc packet (checksum is just Bytes(4))
    parsed = SCTPPacket.parse(with_crc)
    check("parsed.checksum matches computed crc",
          parsed.checksum == crc)


# ============================================================================
# Test 8: Integrated BitStruct + Switch + CRC (the 3-feature combo)
# ============================================================================
def test_three_features_integrated():
    """Demonstrate BitStruct (DataChunkFlags) + Switch (chunk_type) + CRC32c
    working together in a realistic SCTP packet parse flow.
    """
    print("\n[Test] Three features integrated (BitStruct + Switch + CRC32c)")
    # Build a real DATA packet
    data_chunk = ChunkInner(
        chunk_type=0,
        flags_raw=0x03,  # B=1, E=1 (unfragmented, ordered)
        value=DataChunkValue(
            tsn=0x42, stream_id=0, stream_seq=0, ppid=0,
            user_data=b"payload bytes here",
        ),
    )
    packet = SCTPPacket(
        src_port=100, dst_port=200, verify_tag=0xABCDEF01,
        checksum=b"\x00\x00\x00\x00",
        chunks=[data_chunk],
    )
    built = packet.build()

    # Feature 1: CRC32c - compute and embed
    with_crc = attach_checksum(built)
    check("CRC32c computed (4 bytes)",
          len(with_crc[8:12]) == 4 and with_crc[8:12] != b"\x00\x00\x00\x00")
    check("CRC32c verify True", verify_sctp_checksum(with_crc))

    # Parse the CRC-protected packet
    parsed = SCTPPacket.parse(with_crc)
    check("CRC32c parse preserved", parsed.checksum == with_crc[8:12])

    # Feature 2: Switch dispatched DATA chunk value
    check("Switch dispatched DataChunkValue",
          isinstance(parsed.chunks[0].value, DataChunkValue))
    check("Switch preserved tsn", parsed.chunks[0].value.tsn == 0x42)

    # Feature 3: BitStruct - decode DATA flags from raw byte
    flags = DataChunkFlags.parse(bytes([parsed.chunks[0].flags_raw]))
    check("BitStruct flags.b == 1 (Beginning)", flags.b == 1)
    check("BitStruct flags.e == 1 (Ending)", flags.e == 1)
    check("BitStruct flags.u == 0 (Ordered)", flags.u == 0)
    print(f"  decoded DATA flags: reserved={flags.reserved}, "
          f"u={flags.u}, b={flags.b}, e={flags.e}")

    # Round-trip the whole CRC-protected packet
    # (need to preserve checksum bytes for byte-equal round-trip)
    rebuilt = parsed.build()
    check("round-trip preserves CRC bytes",
          rebuilt[8:12] == with_crc[8:12])
    check("round-trip byte-equal",
          rebuilt == with_crc)


# import here to keep test functions above tidy
from sctp_crs import compute_sctp_checksum


# ============================================================================
# Main
# ============================================================================
def main():
    print("=== construct-rs SCTP tests ===")
    test_data_chunk_flags()
    test_data_chunk_round_trip()
    test_init_chunk()
    test_sack_chunk()
    test_abort_chunk()
    test_multi_chunk()
    test_unknown_chunk_type()
    test_checksum()
    test_three_features_integrated()

    print()
    print(f"=== Summary: {PASS_COUNT} passed, {FAIL_COUNT} failed ===")
    if FAIL_COUNT > 0:
        sys.exit(1)


if __name__ == "__main__":
    main()
