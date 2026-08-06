"""SCTP packet - Python construct 2.10.70 parity implementation.

Mirrors sctp_crs.py one-to-one using the original Python construct library
so the two implementations can be cross-checked for byte-for-byte parity.
"""
from construct import (
    Struct, BitStruct, Switch, GreedyRange, Prefixed, Array,
    Int8ub, Int16ub, Int32ub, Bytes, GreedyBytes,
    Bit, BitsInteger, this,
)


# ---------------------------------------------------------------------------
# CRC32c (Castagnoli) - identical algorithm to sctp_crs.crc32c
# ---------------------------------------------------------------------------
_CRC32C_POLY = 0x82F63B78


def crc32c(data: bytes) -> bytes:
    crc = 0xFFFFFFFF
    for byte in data:
        crc ^= byte
        for _ in range(8):
            crc = (crc >> 1) ^ _CRC32C_POLY if crc & 1 else crc >> 1
    return (~crc & 0xFFFFFFFF).to_bytes(4, "little")


# ---------------------------------------------------------------------------
# DATA chunk flags - BitStruct (mirror of DataChunkFlags in sctp_crs.py)
# ---------------------------------------------------------------------------
data_chunk_flags = BitStruct(
    "reserved" / BitsInteger(5),
    "u" / Bit,
    "b" / Bit,
    "e" / Bit,
)


# ---------------------------------------------------------------------------
# Chunk value structs (mirror of *ChunkValue dataclasses in sctp_crs.py)
# ---------------------------------------------------------------------------
data_chunk_value = Struct(
    "tsn" / Int32ub,
    "stream_id" / Int16ub,
    "stream_seq" / Int16ub,
    "ppid" / Int32ub,
    "user_data" / GreedyBytes,
)

init_chunk_value = Struct(
    "initiate_tag" / Int32ub,
    "a_rwnd" / Int32ub,
    "num_outbound_streams" / Int16ub,
    "num_inbound_streams" / Int16ub,
    "initial_tsn" / Int32ub,
    "params" / GreedyBytes,
)

gap_ack_block = Struct(
    "start" / Int16ub,
    "end" / Int16ub,
)

sack_chunk_value = Struct(
    "cum_tsn_ack" / Int32ub,
    "a_rwnd" / Int32ub,
    "num_gap_blocks" / Int16ub,
    "num_dup_tsns" / Int16ub,
    "gap_blocks" / Array(this.num_gap_blocks, gap_ack_block),
    "dup_tsns" / Array(this.num_dup_tsns, Int32ub),
)

abort_chunk_value = Struct(
    "error_causes" / GreedyBytes,
)


# ---------------------------------------------------------------------------
# Chunk inner struct (mirror of ChunkInner)
# ---------------------------------------------------------------------------
chunk_inner = Struct(
    "chunk_type" / Int8ub,
    "flags_raw" / Int8ub,
    "value" / Switch(this.chunk_type, {
        0: data_chunk_value,
        1: init_chunk_value,
        3: sack_chunk_value,
        6: abort_chunk_value,
    }),
)

# A single chunk = length-prefixed ChunkInner.
# includelength=True matches construct-rs semantics exactly:
#   read Int16ub = N, substream = N - 2 bytes (length_value includes the
#   length field's own 2 bytes, so substream is N - 2 = type+flags+value).
chunk = Prefixed(Int16ub, chunk_inner, includelength=True)

# ---------------------------------------------------------------------------
# SCTP packet (mirror of SCTPPacket)
# ---------------------------------------------------------------------------
sctp_packet = Struct(
    "src_port" / Int16ub,
    "dst_port" / Int16ub,
    "verify_tag" / Int32ub,
    "checksum" / Bytes(4),
    "chunks" / GreedyRange(chunk),
)
