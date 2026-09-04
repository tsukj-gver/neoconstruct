"""G1 十六构造器用户面行为探针：Enum/FlagsEnum/Mapping/OneOf/NoneOf。

逐构造器探测 field() 嵌入路径的 parse/build/round-trip/实例化/错误形态，
输出实际行为供测试期望值推导（期望值三源：文档/自然/基线）。
"""

from dataclasses import dataclass
from typing import Any

from neoconstruct import (
    Byte,
    Enum,
    FlagsEnum,
    Mapping,
    MappingError,
    NoneOf,
    OneOf,
    StructMixin,
    ValidationError,
    field,
)


def banner(title):
    print(f"\n{'=' * 20} {title} {'=' * 20}")


banner("Enum basic")
@dataclass
class P(StructMixin):
    op: Any = field(Enum(Byte, READ=1, WRITE=2))

p = P.parse(b"\x01")
print("parse b'\\x01' ->", repr(p.op), "| str:", str(p.op), "| int:", int(p.op), "| ==:", p.op == "READ")
p2 = P.parse(b"\xff")
print("parse b'\\xff' ->", repr(p2.op), "type:", type(p2.op).__name__)
print("build label:", P(op="READ").build())
print("build int:", P(op=2).build())
try:
    print("build unknown label:", P(op="NOPE").build())
except Exception as e:
    print("build unknown label ->", type(e).__name__, e)
try:
    print("build unknown int:", P(op=99).build())
except Exception as e:
    print("build unknown int 99 ->", type(e).__name__, e)
print("roundtrip:", P.parse(b"\x01").build())

banner("Enum instantiation default")
try:
    print("P() default op:", repr(P().op))
except TypeError as e:
    print("P() -> TypeError:", e)

banner("FlagsEnum")
@dataclass
class F(StructMixin):
    flags: Any = field(FlagsEnum(Byte, READ=1, WRITE=2, EXEC=4))

f = F.parse(b"\x03")
print("parse b'\\x03' ->", repr(f.flags))
f0 = F.parse(b"\x00")
print("parse b'\\x00' ->", repr(f0.flags))
print("build partial dict:", F(flags=dict(READ=True)).build())
try:
    print("build int:", F(flags=3).build())
except Exception as e:
    print("build int 3 ->", type(e).__name__, e)
try:
    print("build str 'READ|WRITE':", F(flags="READ|WRITE").build())
except Exception as e:
    print("build str ->", type(e).__name__, e)
try:
    print("build unknown key:", F(flags=dict(NOPE=True)).build())
except Exception as e:
    print("build unknown key ->", type(e).__name__, e)
try:
    print("instantiation default:", repr(F().flags))
except TypeError as e:
    print("F() -> TypeError:", e)
print("roundtrip:", F.parse(b"\x05").build())

banner("Mapping")
SENTINEL = object()
@dataclass
class M(StructMixin):
    v: Any = field(Mapping(Byte, {0: "ZERO", 1: "ONE"}))

m = M.parse(b"\x00")
print("parse b'\\x00' ->", repr(m.v))
try:
    M.parse(b"\xff")
except Exception as e:
    print("parse b'\\xff' ->", type(e).__name__, e)
print("build 'ONE':", M(v="ONE").build())
try:
    print("build unknown:", M(v="TWO").build())
except Exception as e:
    print("build unknown ->", type(e).__name__, e)
try:
    print("instantiation default:", repr(M().v))
except TypeError as e:
    print("M() -> TypeError:", e)
print("roundtrip:", M.parse(b"\x01").build())

banner("OneOf / NoneOf")
@dataclass
class O(StructMixin):
    v: Any = field(OneOf(Byte, [1, 2, 3]))

print("parse valid:", repr(O.parse(b"\x02").v))
try:
    O.parse(b"\xff")
except Exception as e:
    print("parse invalid ->", type(e).__name__, e)
try:
    print("build invalid:", O(v=9).build())
except Exception as e:
    print("build invalid ->", type(e).__name__, e)
print("instantiation:", end=" ")
try:
    print(repr(O().v))
except TypeError as e:
    print("O() -> TypeError:", e)

@dataclass
class N(StructMixin):
    v: Any = field(NoneOf(Byte, [1, 2, 3]))

print("parse allowed:", repr(N.parse(b"\xff").v))
try:
    N.parse(b"\x01")
except Exception as e:
    print("parse forbidden ->", type(e).__name__, e)
try:
    print("build forbidden:", N(v=1).build())
except Exception as e:
    print("build forbidden ->", type(e).__name__, e)
