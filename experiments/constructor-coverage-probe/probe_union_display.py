"""G1 十六构造器用户面行为探针：Union / Hex / HexDump / Probe。"""

import io
import contextlib

from dataclasses import dataclass
from typing import Any

from neoconstruct import (
    Byte,
    Bytes,
    GreedyBytes,
    Hex,
    HexDump,
    Int16ub,
    Int32ub,
    Probe,
    StructMixin,
    Union,
    UnionError,
    field,
    rfield,
)


def banner(title):
    print(f"\n{'=' * 20} {title} {'=' * 20}")


banner("Union parsefrom=None")
@dataclass
class U(StructMixin):
    u: Any = field(Union(None, raw=Bytes(4), num=Int32ub))

u = U.parse(b"ABCD")
print("parse b'ABCD' ->", repr(u.u))
print("build dict raw:", U(u=dict(raw=b"ABCD")).build())
print("build dict num:", U(u=dict(num=0x41424344)).build())
try:
    print("build dict other:", U(u=dict(other=1)).build())
except Exception as e:
    print("build dict other ->", type(e).__name__, e)
print("roundtrip:", U.parse(b"ABCD").build())

banner("Union parsefrom=0 (seek forward)")
@dataclass
class U0(StructMixin):
    u: Any = field(Union(0, raw=Bytes(2), num=Int32ub))
    tail: bytes = field(GreedyBytes)

u0 = U0.parse(b"ABCD")
print("parse ->", repr(u0.u), "tail:", repr(u0.tail))

banner("Union parsefrom=name")
@dataclass
class UN(StructMixin):
    u: Any = field(Union("num", raw=Bytes(2), num=Int32ub))
    tail: bytes = field(GreedyBytes)

un = UN.parse(b"ABCD")
print("parse ->", repr(un.u), "tail:", repr(un.tail))

banner("Union parsefrom out-of-range")
@dataclass
class UX(StructMixin):
    u: Any = field(Union(5, raw=Bytes(2)))

try:
    UX.parse(b"AB")
except Exception as e:
    print("parsefrom=5 (2 subcons) ->", type(e).__name__, e)

banner("Union instantiation default")
try:
    print(repr(U().u))
except TypeError as e:
    print("U() -> TypeError:", e)

banner("Hex int")
@dataclass
class H(StructMixin):
    magic: Any = field(Hex(Int32ub))

h = H.parse(b"\x00\x00\x01\x02")
print("parse ->", repr(h.magic), "type:", type(h.magic).__name__, "int:", int(h.magic))
print("str():", str(h.magic))
print("build:", H(magic=0x102).build())
print("instantiation:", end=" ")
try:
    print(repr(H().magic))
except TypeError as e:
    print("TypeError")

banner("Hex bytes")
@dataclass
class HB(StructMixin):
    data: Any = field(Hex(Bytes(4)))

hb = HB.parse(b"\x00\x00\x01\x02")
print("parse ->", repr(hb.data), "type:", type(hb.data).__name__)
print("str():", str(hb.data))
print("build:", HB(data=b"\x00\x00\x01\x02").build())

banner("HexDump bytes")
@dataclass
class HD(StructMixin):
    data: Any = field(HexDump(Bytes(8)))

hd = HD.parse(b"\x00\x00\x01\x02" + b"ABCD")
print("parse -> type:", type(hd.data).__name__, "| bytes eq:", hd.data == b"\x00\x00\x01\x02ABCD")
print("str():")
print(str(hd.data))
print("build:", HD(data=b"\x00\x00\x01\x02ABCD").build())

banner("HexDump int (passthrough?)")
@dataclass
class HDI(StructMixin):
    v: Any = field(HexDump(Int32ub))

hdi = HDI.parse(b"\x00\x00\x01\x02")
print("parse ->", repr(hdi.v), "type:", type(hdi.v).__name__)

banner("Probe no-arg")
@dataclass
class PR(StructMixin):
    a: int = field(Byte)
    p: Any = rfield(Probe())
    b: int = field(Byte)

buf = io.StringIO()
with contextlib.redirect_stdout(buf):
    pr = PR.parse(b"\x01\x02")
out = buf.getvalue()
print("stdout:", repr(out))
print("a/b:", pr.a, pr.b)
print("roundtrip:", PR.parse(b"\x01\x02").build())

banner("Probe into=")
@dataclass
class PRI(StructMixin):
    a: int = field(Byte)
    p: Any = rfield(Probe(into="a"))
    b: int = field(Byte)

buf2 = io.StringIO()
with contextlib.redirect_stdout(buf2):
    PRI.parse(b"\x07\x08")
print("stdout:", repr(buf2.getvalue()))

banner("Probe lookahead")
@dataclass
class PRL(StructMixin):
    a: int = field(Byte)
    p: Any = rfield(Probe(lookahead=4))
    b: int = field(Byte)

buf3 = io.StringIO()
with contextlib.redirect_stdout(buf3):
    PRL.parse(b"\x0a\x0b\x0c\x0d")
print("stdout:", repr(buf3.getvalue()))

banner("Probe instantiation default")
try:
    print(repr(PR(a=1, b=2).p))
except TypeError as e:
    print("TypeError:", e)
