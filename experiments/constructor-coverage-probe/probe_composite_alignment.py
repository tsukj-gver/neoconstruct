"""G1 十六构造器用户面行为探针：Sequence / NamedTuple / Aligned / AlignedStruct
/ Half / Double 别名。"""

from dataclasses import dataclass
from typing import Any

from neoconstruct import (
    Aligned,
    AlignedStruct,
    Byte,
    Bytes,
    Default,
    Double,
    Float16b,
    Float32b,
    Float64b,
    GreedyBytes,
    Half,
    Int16ub,
    Int8ub,
    Int32ub,
    NamedTuple,
    PaddingError,
    Sequence,
    Single,
    StructMixin,
    field,
    rfield,
)


def banner(title):
    print(f"\n{'=' * 20} {title} {'=' * 20}")


banner("Sequence basic")
@dataclass
class S(StructMixin):
    items: Any = field(Sequence(Byte, Bytes(2), GreedyBytes))

s = S.parse(b"\x01XYtail")
print("parse ->", repr(s.items))
print("build:", S(items=[1, b"XY", b"tail"]).build())
print("instantiation:", end=" ")
try:
    print(repr(S().items))
except TypeError as e:
    print("TypeError")
print("roundtrip:", S.parse(b"\x01XYtail").build())

banner("Sequence named kw")
@dataclass
class SN(StructMixin):
    seq: Any = field(Sequence(count=Byte, data=GreedyBytes))

sn = SN.parse(b"\x03ABC")
print("parse ->", repr(sn.seq))
print("build:", SN(seq=[3, b"ABC"]).build())

banner("Sequence inner value-semantic field")
@dataclass
class SV(StructMixin):
    seq: Any = field(Sequence(Int8ub, Default(Int8ub, 0)))

try:
    sv = SV.parse(b"\x07")
    print("parse ->", repr(sv.seq))
except Exception as e:
    print("parse ->", type(e).__name__, str(e)[:80])

banner("NamedTuple over Struct subclass")
@dataclass
class Coord(StructMixin):
    x: int = field(Int8ub)
    y: int = field(Int8ub)

@dataclass
class NP(StructMixin):
    coord: Any = field(NamedTuple("coord", "x y", Coord))

np = NP.parse(b"\x01\x02")
print("parse ->", repr(np.coord), "type:", type(np.coord).__name__)
print("attr access:", np.coord.x, np.coord.y)
print("build:", NP(coord=np.coord).build())
print("instantiation:", end=" ")
try:
    print(repr(NP().coord))
except TypeError:
    print("TypeError")
print("roundtrip:", NP.parse(b"\x05\x06").build())

banner("NamedTuple over Sequence")
@dataclass
class NS(StructMixin):
    pair: Any = field(NamedTuple("pair", "a b", Sequence(Int8ub, Int8ub)))

ns = NS.parse(b"\x0a\x0b")
print("parse ->", repr(ns.pair), "type:", type(ns.pair).__name__)
print("build:", NS(pair=ns.pair).build())

banner("NamedTuple error path (wrong build type)")
@dataclass
class NB(StructMixin):
    coord: Any = field(NamedTuple("coord", "x y", Coord))

try:
    print(NB(coord=123).build())
except Exception as e:
    print("build int ->", type(e).__name__, str(e)[:90])

banner("Aligned basic three states")
@dataclass
class A(StructMixin):
    v: Any = field(Aligned(4, Int16ub))

print("parse aligned:", repr(A.parse(b"\x00\x01\x00\x00").v))
print("build:", A(v=1).build())
a2 = A.parse(b"\x00\x01\x00\x00tail")
print("consumes padding:", repr(a2.v), "roundtrip bytes:", a2.build())

banner("Aligned pattern")
@dataclass
class AP(StructMixin):
    v: Any = field(Aligned(4, Int16ub, pattern=b"\xff"))

print("build pattern:", AP(v=1).build())

banner("Aligned modulus=1 / modulus expr")
try:
    @dataclass
    class A1(StructMixin):
        v: Any = field(Aligned(1, Int16ub))
    print("Aligned(1,...) compiled OK; build:", A1(v=1).build())
except Exception as e:
    print("Aligned(1, ...) ->", type(e).__name__, str(e)[:90])

@dataclass
class AE(StructMixin):
    m: int = field(Int8ub)
    v: Any = field(Aligned(m, Int16ub))

try:
    print("expr modulus build:", AE(m=4, v=1).build())
except Exception as e:
    print("expr modulus build ->", type(e).__name__, str(e)[:90])

banner("AlignedStruct")
AP4 = AlignedStruct(4, a=Int8ub, b=Int16ub)
print("type:", type(AP4).__name__, "fields:", [f.name for f in AP4.__dataclass_fields__.values()])
inst = AP4(a=1, b=2)
print("build:", inst.build())
parsed = AP4.parse(b"\x01\x00\x00\x00\x00\x02\x00\x00")
print("parse:", parsed.a, parsed.b, "roundtrip:", parsed.build() == b"\x01\x00\x00\x00\x00\x02\x00\x00")

banner("Half/Double aliases")
print("Half is Float16b:", Half is Float16b)
print("Double is Float64b:", Double is Float64b)
print("Single is Float32b:", Single is Float32b)

@dataclass
class FD(StructMixin):
    h: float = field(Half)
    d: float = field(Double)

fd = FD.parse(b"\x3c\x00" + b"\x40\x59\x00\x00\x00\x00\x00\x00")
print("half:", fd.h, "double:", fd.d)
print("build:", FD(h=1.0, d=100.0).build())
print("roundtrip:", FD.parse(FD(h=1.0, d=100.0).build()).build() == FD(h=1.0, d=100.0).build())
