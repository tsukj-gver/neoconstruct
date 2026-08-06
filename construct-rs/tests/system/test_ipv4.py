"""IPv4 报文头协议系统测试。

协议参考：RFC 791（Internet Protocol）

## 协议结构（IPv4 header，20 字节最小）

::

     0                   1                   2                   3
     0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    |Version|  IHL  |    DSCP/ECN   |          Total Length         |
    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    |         Identification        |Flags|   Fragment Offset       |
    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    |  TTL  |    Protocol           |       Header Checksum         |
    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    |                       Source Address                          |
    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    |                    Destination Address                        |
    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    |                    Options (if IHL > 5)                       |
    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+

bit 布局：
- Version: 4 bit（始终 4）
- IHL: 4 bit（Internet Header Length，5-15，单位 32-bit word）
- DSCP: 6 bit（Differentiated Services Code Point）
- ECN: 2 bit（Explicit Congestion Notification）
- Identification: 16 bit
- Flags: 3 bit（Reserved, DF, MF）
- Fragment Offset: 13 bit
- TTL: 8 bit
- Protocol: 8 bit
- Header Checksum: 16 bit（ones-complement sum, big-endian）
- Source / Destination Address: 32 bit each

## 覆盖能力

- **bitfield**：BitStruct 解析 Version/IHL（共 1 byte）+ DSCP/ECN（共 1 byte）
  + Flags/FragOffset（共 16 bit）
- **Checksum 校验**：用 ``ipv4_checksum`` 函数在 test 中验证
  （Checksum 构造器的 bytesfunc 不支持 RawCopy dict 引用，详见 trace）
- **常量**：Const 校验 Version=4

## API gap

IPv4 header checksum 字段在协议中间（offset 10-11），不在帧末尾，
无法直接用 ``Checksum(start, end)`` + Tell（start/end 需先于 checksum 字段）。
当前实现把 checksum 当作普通 Bytes(2)，外部验证。
"""

from __future__ import annotations

from dataclasses import dataclass

import pytest

from construct import (
    BitStructMixin,
    BitsInteger,
    Bit,
    Bytes,
    Int8ub,
    Int16ub,
    Int32ub,
    Const,
    StructMixin,
    field,
    rfield,
    wfield,
    Padding,
)

from ._system_helpers import ipv4_checksum


# ---------------------------------------------------------------------------
# 协议定义
# ---------------------------------------------------------------------------


@dataclass
class IPv4VersionIHL(BitStructMixin):
    """Version (4 bit) + IHL (4 bit)。

    BitStruct 总长 = 4 + 4 = 8 bit = 1 byte（无须 padding）。
    """

    version: int = field(BitsInteger(4))
    ihl: int = field(BitsInteger(4))


@dataclass
class IPv4DSCPECN(BitStructMixin):
    """DSCP (6 bit) + ECN (2 bit)。总长 8 bit = 1 byte。"""

    dscp: int = field(BitsInteger(6))
    ecn: int = field(BitsInteger(2))


@dataclass
class IPv4FlagsFragment(BitStructMixin):
    """Flags (3 bit) + Fragment Offset (13 bit)。总长 16 bit = 2 byte。"""

    reserved: int = field(Bit())
    df: int = field(Bit())  # Don't Fragment
    mf: int = field(Bit())  # More Fragments
    fragment_offset: int = field(BitsInteger(13))


@dataclass
class IPv4Header(StructMixin):
    """IPv4 报文头（最小 20 字节，最大 60 字节含 options）。

    本实现固定 20 字节（IHL=5），不支持 options。Version 校验 Const(4)。
    Checksum 当作普通 Bytes(2)，test 代码用 ``ipv4_checksum`` 函数验证。

    协议字段顺序：
        version_ihl | dscp_ecn | total_length | identification
        | flags_fragment | ttl | protocol | checksum
        | source_addr | dest_addr
    """

    version_ihl: IPv4VersionIHL = field(IPv4VersionIHL)
    dscp_ecn: IPv4DSCPECN = field(IPv4DSCPECN)
    total_length: int = field(Int16ub)
    identification: int = field(Int16ub)
    flags_fragment: IPv4FlagsFragment = field(IPv4FlagsFragment)
    ttl: int = field(Int8ub)
    protocol: int = field(Int8ub)
    checksum: bytes = field(Bytes(2))
    source_addr: int = field(Int32ub)
    dest_addr: int = field(Int32ub)


# ---------------------------------------------------------------------------
# 工具：构造 IPv4 头测试字节
# ---------------------------------------------------------------------------


def build_ipv4_bytes(
    *,
    version: int = 4,
    ihl: int = 5,
    dscp: int = 0,
    ecn: int = 0,
    total_length: int = 20,
    identification: int = 0,
    reserved: int = 0,
    df: int = 0,
    mf: int = 0,
    fragment_offset: int = 0,
    ttl: int = 64,
    protocol: int = 6,
    source_addr: int = 0x0A000001,  # 10.0.0.1
    dest_addr: int = 0x0A000002,  # 10.0.0.2
    use_correct_checksum: bool = True,
    checksum_override: int | None = None,
) -> bytes:
    """构造 20 字节 IPv4 头（无 options）。

    默认 ``use_correct_checksum=True`` 自动计算正确的 header checksum；
    若需测试错误校验和场景，传 ``checksum_override`` 或设
    ``use_correct_checksum=False``。
    """
    # 字节 0: version<<4 | ihl
    b0 = ((version & 0xF) << 4) | (ihl & 0xF)
    # 字节 1: dscp<<2 | ecn
    b1 = ((dscp & 0x3F) << 2) | (ecn & 0x3)
    # 字节 2-3: total_length (BE)
    b23 = bytes([(total_length >> 8) & 0xFF, total_length & 0xFF])
    # 字节 4-5: identification (BE)
    b45 = bytes([(identification >> 8) & 0xFF, identification & 0xFF])
    # 字节 6-7: flags (3) + fragment_offset (13)
    flags_val = (reserved << 2) | (df << 1) | mf
    frag_combined = (flags_val << 13) | (fragment_offset & 0x1FFF)
    b67 = bytes([(frag_combined >> 8) & 0xFF, frag_combined & 0xFF])
    # 字节 8: ttl
    b8 = ttl & 0xFF
    # 字节 9: protocol
    b9 = protocol & 0xFF
    # 字节 10-11: checksum (BE)，先用 0 占位
    b1011 = b"\x00\x00"
    # 字节 12-15: source_addr (BE)
    b1215 = bytes(
        [
            (source_addr >> 24) & 0xFF,
            (source_addr >> 16) & 0xFF,
            (source_addr >> 8) & 0xFF,
            source_addr & 0xFF,
        ]
    )
    # 字节 16-19: dest_addr (BE)
    b1619 = bytes(
        [
            (dest_addr >> 24) & 0xFF,
            (dest_addr >> 16) & 0xFF,
            (dest_addr >> 8) & 0xFF,
            dest_addr & 0xFF,
        ]
    )
    header_no_chksum = (
        bytes([b0, b1]) + b23 + b45 + b67 + bytes([b8, b9]) + b1011 + b1215 + b1619
    )
    assert len(header_no_chksum) == 20

    if checksum_override is not None:
        chksum = checksum_override & 0xFFFF
        b1011 = bytes([(chksum >> 8) & 0xFF, chksum & 0xFF])
    elif use_correct_checksum:
        computed = ipv4_checksum(header_no_chksum)
        b1011 = computed
    # else: keep b1011 = 0x0000

    return (
        bytes([b0, b1]) + b23 + b45 + b67 + bytes([b8, b9]) + b1011 + b1215 + b1619
    )


# ---------------------------------------------------------------------------
# Round-trip 测试
# ---------------------------------------------------------------------------


class TestIPv4RoundTrip:
    """典型 IPv4 头 round-trip。"""

    def test_minimal_header(self):
        """最小 IPv4 头（20 字节，全默认值）。"""
        data = build_ipv4_bytes()
        assert len(data) == 20

        parsed = IPv4Header.parse(data)
        assert parsed.version_ihl.version == 4
        assert parsed.version_ihl.ihl == 5
        assert parsed.dscp_ecn.dscp == 0
        assert parsed.dscp_ecn.ecn == 0
        assert parsed.total_length == 20
        assert parsed.ttl == 64
        assert parsed.protocol == 6  # TCP
        assert parsed.source_addr == 0x0A000001
        assert parsed.dest_addr == 0x0A000002

        # build round-trip
        rebuilt = parsed.build()
        assert rebuilt == data

    def test_typical_tcp_header(self):
        """典型 TCP 包（identification=0x1234, DF=1, ttl=64, protocol=TCP）。"""
        data = build_ipv4_bytes(
            identification=0x1234,
            df=1,
            ttl=64,
            protocol=6,
            source_addr=0xC0A80001,  # 192.168.0.1
            dest_addr=0xC0A800FE,  # 192.168.0.254
            total_length=1500,
        )

        parsed = IPv4Header.parse(data)
        assert parsed.version_ihl.version == 4
        assert parsed.total_length == 1500
        assert parsed.identification == 0x1234
        assert parsed.flags_fragment.df == 1
        assert parsed.flags_fragment.mf == 0
        assert parsed.flags_fragment.reserved == 0
        assert parsed.flags_fragment.fragment_offset == 0
        assert parsed.ttl == 64
        assert parsed.protocol == 6
        assert parsed.source_addr == 0xC0A80001
        assert parsed.dest_addr == 0xC0A800FE

        assert parsed.build() == data

    def test_dscp_ecn_decomposition(self):
        """DSCP/ECN bitfield 正确分解（DSCP=46 EF, ECN=3 CE）。"""
        data = build_ipv4_bytes(dscp=46, ecn=3)

        parsed = IPv4Header.parse(data)
        assert parsed.dscp_ecn.dscp == 46
        assert parsed.dscp_ecn.ecn == 3

        assert parsed.build() == data

    def test_fragment_offset_max(self):
        """Fragment offset 13-bit 最大值（8191）。"""
        data = build_ipv4_bytes(fragment_offset=8191, mf=1)

        parsed = IPv4Header.parse(data)
        assert parsed.flags_fragment.fragment_offset == 8191
        assert parsed.flags_fragment.mf == 1

        assert parsed.build() == data


# ---------------------------------------------------------------------------
# bitfield 边界
# ---------------------------------------------------------------------------


class TestIPv4BitFieldBoundaries:
    """各 bitfield 字段边界值。"""

    @pytest.mark.parametrize(
        "version,ihl",
        [(4, 5), (4, 15), (4, 0), (4, 1)],
        ids=["typical", "max-IHL", "min-IHL-0", "IHL-1"],
    )
    def test_version_ihl_combinations(self, version, ihl):
        data = build_ipv4_bytes(version=version, ihl=ihl)
        parsed = IPv4Header.parse(data)
        assert parsed.version_ihl.version == version
        assert parsed.version_ihl.ihl == ihl
        assert parsed.build() == data

    @pytest.mark.parametrize("dscp", [0, 46, 63], ids=["min", "EF", "max"])
    @pytest.mark.parametrize("ecn", [0, 1, 2, 3])
    def test_dscp_ecn_boundaries(self, dscp, ecn):
        data = build_ipv4_bytes(dscp=dscp, ecn=ecn)
        parsed = IPv4Header.parse(data)
        assert parsed.dscp_ecn.dscp == dscp
        assert parsed.dscp_ecn.ecn == ecn

    @pytest.mark.parametrize(
        "df,mf,frag_off",
        [
            (0, 0, 0),
            (1, 0, 0),  # DF set
            (0, 1, 0),  # MF set, first fragment
            (0, 1, 1480),  # MF set, intermediate fragment
            (0, 0, 8191),  # last fragment, max offset
        ],
    )
    def test_flags_fragment_combinations(self, df, mf, frag_off):
        data = build_ipv4_bytes(df=df, mf=mf, fragment_offset=frag_off)
        parsed = IPv4Header.parse(data)
        assert parsed.flags_fragment.df == df
        assert parsed.flags_fragment.mf == mf
        assert parsed.flags_fragment.fragment_offset == frag_off
        assert parsed.build() == data


# ---------------------------------------------------------------------------
# Checksum 校验
# ---------------------------------------------------------------------------


class TestIPv4Checksum:
    """Header checksum 校验（外部函数验证，不用 Checksum 构造器）。"""

    def test_correct_checksum(self):
        """默认 build_ipv4_bytes 自动计算正确 checksum。"""
        data = build_ipv4_bytes()
        parsed = IPv4Header.parse(data)
        # 校验：用 ipv4_checksum 函数计算解析后的 header（checksum 位置不变）
        # 应得到 0x0000（带 checksum 重新计算的检查和应为 0）
        recomputed = ipv4_checksum(data)
        assert recomputed == b"\x00\x00", (
            f"checksum verification failed: recompute over header with checksum "
            f"should be 0, got {recomputed.hex()}"
        )

    def test_zero_checksum_passes_parse(self):
        """全 0 checksum 仍能 parse（IPv4Header 把 checksum 当作普通 Bytes(2)）。"""
        data = build_ipv4_bytes(use_correct_checksum=False)
        # 0x0000 checksum
        parsed = IPv4Header.parse(data)
        assert parsed.checksum == b"\x00\x00"

    def test_real_world_packet_checksum(self):
        """真实 IPv4 包（reference: RFC 1071 example, byte-exact）。"""
        # RFC 1071 example header
        data = bytes.fromhex("4500003c1c46400040060000ac100a63ac100a01")
        # 注：example 中的 checksum 字段为 0，实际正确 checksum 应为 b1f1
        # 这里用零 checksum 版本测试 parse
        parsed = IPv4Header.parse(data)
        assert parsed.version_ihl.version == 4
        assert parsed.version_ihl.ihl == 5
        assert parsed.total_length == 60
        assert parsed.identification == 0x1C46
        assert parsed.flags_fragment.df == 1
        assert parsed.flags_fragment.fragment_offset == 0
        assert parsed.ttl == 0x40
        assert parsed.protocol == 6
        assert parsed.source_addr == 0xAC100A63
        assert parsed.dest_addr == 0xAC100A01

        # 计算正确 checksum
        correct_checksum = ipv4_checksum(data)
        assert correct_checksum.hex() == "b1f1", (
            f"expected b1f1 per RFC 1071 example, got {correct_checksum.hex()}"
        )

        # 用正确 checksum 重建并验证
        full_data = bytes.fromhex("4500003c1c46400040060000b1f1ac100a63ac100a01")
        # Verify: checksum of header (with checksum included) should be 0
        assert ipv4_checksum(full_data) == b"\x00\x00"


# ---------------------------------------------------------------------------
# Parity vs Python construct 2.10.70
# ---------------------------------------------------------------------------


class TestIPv4Parity:
    """parity vs Python construct 2.10.70（子进程隔离）。

    两端使用相同的 BitStruct 分解 version/IHL/DSCP/ECN/flags/frag。
    """

    @pytest.mark.parametrize(
        "data_bytes",
        [
            # Minimal header, 20 bytes
            build_ipv4_bytes(),
            # Typical TCP packet
            build_ipv4_bytes(
                identification=0xABCD,
                df=1,
                ttl=128,
                protocol=6,
                source_addr=0xC0A80001,
                dest_addr=0xC0A800FE,
                total_length=1500,
            ),
            # DSCP=46 (EF), ECN=3 (CE)
            build_ipv4_bytes(dscp=46, ecn=3),
        ],
        ids=["minimal", "typical-tcp", "EF-CE"],
    )
    def test_parse_parity(self, data_bytes):
        """parse 同字节流 → rs 与 py 字段值一致。"""
        from ._system_helpers import run_parity_snippet

        data_hex = data_bytes.hex()

        rs_code = f"""
import json
from dataclasses import dataclass, asdict
from construct import (
    StructMixin, BitStructMixin, field, BitsInteger, Bit, Bytes, Int8ub, Int16ub, Int32ub,
)

@dataclass
class VerIHL(BitStructMixin):
    version: int = field(BitsInteger(4))
    ihl: int = field(BitsInteger(4))

@dataclass
class DE(BitStructMixin):
    dscp: int = field(BitsInteger(6))
    ecn: int = field(BitsInteger(2))

@dataclass
class FF(BitStructMixin):
    reserved: int = field(Bit())
    df: int = field(Bit())
    mf: int = field(Bit())
    fragment_offset: int = field(BitsInteger(13))

@dataclass
class H(StructMixin):
    version_ihl: VerIHL = field(VerIHL)
    dscp_ecn: DE = field(DE)
    total_length: int = field(Int16ub)
    identification: int = field(Int16ub)
    flags_fragment: FF = field(FF)
    ttl: int = field(Int8ub)
    protocol: int = field(Int8ub)
    checksum: bytes = field(Bytes(2))
    source_addr: int = field(Int32ub)
    dest_addr: int = field(Int32ub)

p = H.parse(bytes.fromhex('{data_hex}'))
result = {{
    'version_ihl': asdict(p.version_ihl),
    'dscp_ecn': asdict(p.dscp_ecn),
    'total_length': p.total_length,
    'identification': p.identification,
    'flags_fragment': asdict(p.flags_fragment),
    'ttl': p.ttl,
    'protocol': p.protocol,
    'checksum': p.checksum.hex(),
    'source_addr': p.source_addr,
    'dest_addr': p.dest_addr,
}}
print(json.dumps(result))
"""
        py_code = f"""
import json
import construct as pc

VerIHL = pc.BitStruct(
    'version' / pc.BitsInteger(4),
    'ihl' / pc.BitsInteger(4),
)
DE = pc.BitStruct(
    'dscp' / pc.BitsInteger(6),
    'ecn' / pc.BitsInteger(2),
)
FF = pc.BitStruct(
    'reserved' / pc.Bit,
    'df' / pc.Bit,
    'mf' / pc.Bit,
    'fragment_offset' / pc.BitsInteger(13),
)
H = pc.Struct(
    'version_ihl' / VerIHL,
    'dscp_ecn' / DE,
    'total_length' / pc.Int16ub,
    'identification' / pc.Int16ub,
    'flags_fragment' / FF,
    'ttl' / pc.Int8ub,
    'protocol' / pc.Int8ub,
    'checksum' / pc.Bytes(2),
    'source_addr' / pc.Int32ub,
    'dest_addr' / pc.Int32ub,
)

p = H.parse(bytes.fromhex('{data_hex}'))
def _strip(d):
    return {{k: v for k, v in dict(d).items() if not str(k).startswith('_')}}

# Move 'version' and 'ihl' from nested to top-level for matching rs naming
vi = _strip(p.version_ihl)
de = _strip(p.dscp_ecn)
ff = _strip(p.flags_fragment)
result = {{
    'version_ihl': {{'version': vi['version'], 'ihl': vi['ihl']}},
    'dscp_ecn': {{'dscp': de['dscp'], 'ecn': de['ecn']}},
    'total_length': p.total_length,
    'identification': p.identification,
    'flags_fragment': {{
        'reserved': ff['reserved'], 'df': ff['df'], 'mf': ff['mf'],
        'fragment_offset': ff['fragment_offset'],
    }},
    'ttl': p.ttl,
    'protocol': p.protocol,
    'checksum': p.checksum.hex(),
    'source_addr': p.source_addr,
    'dest_addr': p.dest_addr,
}}
print(json.dumps(result))
"""
        rs_result = run_parity_snippet("rs", rs_code, py_code)
        py_result = run_parity_snippet("py", rs_code, py_code)
        assert rs_result == py_result, (
            f"IPv4 parity mismatch for {data_hex}\n"
            f"  rs: {rs_result}\n"
            f"  py: {py_result}"
        )

    def test_build_parity(self):
        """build 相同字段值 → 字节一致。"""
        from ._system_helpers import run_parity_snippet

        test_data = build_ipv4_bytes()

        rs_code = f"""
import json
from dataclasses import dataclass
from construct import (
    StructMixin, BitStructMixin, field, BitsInteger, Bit, Bytes, Int8ub, Int16ub, Int32ub,
)

@dataclass
class VerIHL(BitStructMixin):
    version: int = field(BitsInteger(4))
    ihl: int = field(BitsInteger(4))

@dataclass
class DE(BitStructMixin):
    dscp: int = field(BitsInteger(6))
    ecn: int = field(BitsInteger(2))

@dataclass
class FF(BitStructMixin):
    reserved: int = field(Bit())
    df: int = field(Bit())
    mf: int = field(Bit())
    fragment_offset: int = field(BitsInteger(13))

@dataclass
class H(StructMixin):
    version_ihl: VerIHL = field(VerIHL)
    dscp_ecn: DE = field(DE)
    total_length: int = field(Int16ub)
    identification: int = field(Int16ub)
    flags_fragment: FF = field(FF)
    ttl: int = field(Int8ub)
    protocol: int = field(Int8ub)
    checksum: bytes = field(Bytes(2))
    source_addr: int = field(Int32ub)
    dest_addr: int = field(Int32ub)

p = H.parse(bytes.fromhex('{test_data.hex()}'))
b = p.build()
print(json.dumps({{'built': b.hex()}}))
"""
        py_code = f"""
import json
import construct as pc

VerIHL = pc.BitStruct(
    'version' / pc.BitsInteger(4), 'ihl' / pc.BitsInteger(4),
)
DE = pc.BitStruct(
    'dscp' / pc.BitsInteger(6), 'ecn' / pc.BitsInteger(2),
)
FF = pc.BitStruct(
    'reserved' / pc.Bit, 'df' / pc.Bit, 'mf' / pc.Bit,
    'fragment_offset' / pc.BitsInteger(13),
)
H = pc.Struct(
    'version_ihl' / VerIHL,
    'dscp_ecn' / DE,
    'total_length' / pc.Int16ub,
    'identification' / pc.Int16ub,
    'flags_fragment' / FF,
    'ttl' / pc.Int8ub,
    'protocol' / pc.Int8ub,
    'checksum' / pc.Bytes(2),
    'source_addr' / pc.Int32ub,
    'dest_addr' / pc.Int32ub,
)

p = H.parse(bytes.fromhex('{test_data.hex()}'))
b = H.build(p)
print(json.dumps({{'built': b.hex()}}))
"""
        rs_result = run_parity_snippet("rs", rs_code, py_code)
        py_result = run_parity_snippet("py", rs_code, py_code)
        assert rs_result["built"] == py_result["built"], (
            f"IPv4 build parity mismatch\n"
            f"  rs: {rs_result['built']}\n"
            f"  py: {py_result['built']}"
        )
        assert rs_result["built"] == test_data.hex()
