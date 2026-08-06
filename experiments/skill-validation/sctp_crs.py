"""SCTP (Stream Control Transmission Protocol) packet - construct-rs implementation.

Simplified per RFC 4960:
  * Common Header (12 B): src_port + dst_port + verify_tag + checksum (CRC32c)
  * Chunks: GreedyRange; each chunk = chunk_length-prefixed (type + flags + value)
  * Supported chunk types: DATA (0), INIT (1), SACK (3), ABORT (6)

Three required features:
  * Bitfield: DATA chunk flags (5 reserved + U + B + E = 8 bit, BitStruct)
  * Conditional dispatch: Switch on chunk_type -> different chunk value structs
  * Checksum: CRC32c in common header (offset 8-11)

Note on chunk padding:
  SCTP requires each chunk to be padded to a 4-byte boundary (padding bytes
  not counted in chunk_length). This simplified impl ASSUMES chunk_length is
  always a multiple of 4 (caller responsibility). Padding handling requires
  extra work because chunk_length is consumed inside Prefixed and not exposed
  in the outer context (SKILL gap - no clean API for "prefix-length-aware
  alignment padding").

Note on checksum position:
  SCTP checksum is at offset 8-11 (middle of common header), NOT at packet
  trailer. Per SKILL 4.4.4, the Checksum constructor only works for trailer
  position (StreamRange pattern requires start/end before checksum field).
  We use plain Bytes(4) + external Python verify function (crc32c).
"""
from dataclasses import dataclass

from construct import (
    StructMixin, BitStructMixin, field, rfield,
    Int8ub, Int16ub, Int32ub, Bytes, GreedyBytes,
    Bit, BitsInteger,
    Switch, GreedyRange, Prefixed,
    Array,
)


# ---------------------------------------------------------------------------
# CRC32c (Castagnoli) - SCTP checksum per RFC 4960 Appendix B
# ---------------------------------------------------------------------------
_CRC32C_POLY = 0x82F63B78  # CRC32c reversed polynomial


def crc32c(data: bytes) -> bytes:
    """Compute CRC32c (Castagnoli) checksum.

    Returns 4 bytes in little-endian byte order (matches typical SCTP wire
    encoding; both implementations in this validation use the same encoding
    so parity is unaffected).
    """
    crc = 0xFFFFFFFF
    for byte in data:
        crc ^= byte
        for _ in range(8):
            crc = (crc >> 1) ^ _CRC32C_POLY if crc & 1 else crc >> 1
    return (~crc & 0xFFFFFFFF).to_bytes(4, "little")


# ---------------------------------------------------------------------------
# DATA chunk flags - BitStruct (bitfield decomposition)
# ---------------------------------------------------------------------------
@dataclass
class DataChunkFlags(BitStructMixin):
    """DATA chunk flags (RFC 4960 3.3.1) - 8 bit total.

    MSB-first layout (as displayed in RFC ASCII art):
        bits 7-3 : reserved (5 bit, should be 0)
        bit 2    : U (Unordered)
        bit 1    : B (Beginning fragment)
        bit 0    : E (Ending fragment)
    """
    reserved: int = field(BitsInteger(5))
    u: int = field(Bit())
    b: int = field(Bit())
    e: int = field(Bit())


# ---------------------------------------------------------------------------
# Chunk value structs (one per supported chunk_type)
# ---------------------------------------------------------------------------
@dataclass
class DataChunkValue(StructMixin):
    """DATA chunk value: TSN + stream info + PPID + User Data."""
    tsn: int = field(Int32ub)
    stream_id: int = field(Int16ub)
    stream_seq: int = field(Int16ub)
    ppid: int = field(Int32ub)
    user_data: bytes = field(GreedyBytes)  # remainder of chunk substream


@dataclass
class InitChunkValue(StructMixin):
    """INIT chunk value: 5 mandatory fields + optional TLV params."""
    initiate_tag: int = field(Int32ub)
    a_rwnd: int = field(Int32ub)
    num_outbound_streams: int = field(Int16ub)
    num_inbound_streams: int = field(Int16ub)
    initial_tsn: int = field(Int32ub)
    params: bytes = field(GreedyBytes)  # optional/variable-length parameters


@dataclass
class GapAckBlock(StructMixin):
    """SACK Gap Ack Block: 2-byte start offset + 2-byte end offset."""
    start: int = field(Int16ub)
    end: int = field(Int16ub)


@dataclass
class SackChunkValue(StructMixin):
    """SACK chunk value: cumulative TSN ack + a_rwnd + gap blocks + dup TSNs."""
    cum_tsn_ack: int = field(Int32ub)
    a_rwnd: int = field(Int32ub)
    num_gap_blocks: int = field(Int16ub)
    num_dup_tsns: int = field(Int16ub)
    gap_blocks: list = field(Array(num_gap_blocks, GapAckBlock))
    dup_tsns: list = field(Array(num_dup_tsns, Int32ub))


@dataclass
class AbortChunkValue(StructMixin):
    """ABORT chunk value: optional error causes (raw TLV bytes)."""
    error_causes: bytes = field(GreedyBytes)


# ---------------------------------------------------------------------------
# Chunk inner struct (inside Prefixed substream, after length consumed)
# ---------------------------------------------------------------------------
@dataclass
class ChunkInner(StructMixin):
    """Single chunk: type + flags + value (length consumed by Prefixed).

    Conditional dispatch on chunk_type via Switch:
        0 -> DataChunkValue
        1 -> InitChunkValue
        3 -> SackChunkValue
        6 -> AbortChunkValue
    Default (unmapped type) -> None (Switch default=Pass).
    """
    chunk_type: int = field(Int8ub)
    flags_raw: int = field(Int8ub)
    value: object = field(Switch(chunk_type, {
        0: DataChunkValue,
        1: InitChunkValue,
        3: SackChunkValue,
        6: AbortChunkValue,
    }))


# ---------------------------------------------------------------------------
# SCTP packet
# ---------------------------------------------------------------------------
@dataclass
class SCTPPacket(StructMixin):
    """SCTP packet = Common Header (12B) + one or more chunks.

    Each chunk wrapped in `Prefixed(Int16ub, ChunkInner, includelength=True)`:
      * Int16ub reads chunk_length (whole-chunk byte count including header)
      * includelength=True -> substream = chunk_length - 2 bytes (type + flags + value)
      * GreedyRange repeats until EOF
    """
    src_port: int = field(Int16ub)
    dst_port: int = field(Int16ub)
    verify_tag: int = field(Int32ub)
    # Checksum at offset 8-11 (middle of common header) -> plain Bytes(4)
    # (cannot use Checksum ctor for middle-position checksum per SKILL 4.4.4)
    checksum: bytes = field(Bytes(4))
    chunks: list = field(GreedyRange(Prefixed(Int16ub, ChunkInner, includelength=True)))


# ---------------------------------------------------------------------------
# External checksum helpers (parse-time verify / build-time compute)
# ---------------------------------------------------------------------------
def compute_sctp_checksum(packet_bytes_no_checksum: bytes) -> bytes:
    """Compute CRC32c checksum for SCTP packet.

    Caller must pass full packet bytes with the checksum field (offset 8-11)
    set to all zeros (per RFC 4960 section 6.8 -- CRC computed over whole
    packet with checksum field treated as 0).
    """
    return crc32c(packet_bytes_no_checksum)


def verify_sctp_checksum(packet_bytes: bytes) -> bool:
    """Verify SCTP packet checksum.

    Returns True iff CRC32c of (packet with checksum zeroed) equals stored.
    """
    stored = packet_bytes[8:12]
    zeroed = packet_bytes[:8] + b"\x00\x00\x00\x00" + packet_bytes[12:]
    return stored == crc32c(zeroed)


def attach_checksum(packet_no_checksum_field: bytes) -> bytes:
    """Convenience: insert CRC32c at offset 8-11.

    Argument is the packet bytes with placeholder (e.g. zeros) at offset 8-11.
    Returns a new bytes with the checksum field populated.
    """
    crc = compute_sctp_checksum(
        packet_no_checksum_field[:8] + b"\x00\x00\x00\x00" + packet_no_checksum_field[12:]
    )
    return packet_no_checksum_field[:8] + crc + packet_no_checksum_field[12:]
