"""补遗探针：Probe field 显式值 build / EnumIntegerString.intvalue / Union
嵌套位置 / Aligned 变长字段交互 / Timestamp ms 语义复核。"""

from dataclasses import dataclass
from typing import Any

from neoconstruct import (
    Byte,
    Bytes,
    Enum,
    Int8ub,
    Int16ub,
    Probe,
    StructMixin,
    Union,
    field,
    rfield,
)


@dataclass
class PF(StructMixin):
    a: int = field(Byte)
    p: Any = field(Probe())
    b: int = field(Byte)

import io, contextlib
buf = io.StringIO()
with contextlib.redirect_stdout(buf):
    out = PF(a=1, p=0, b=2).build()
print("field explicit p=0 build:", out, "| stdout had probe:", "Probe" in buf.getvalue())

# EnumIntegerString.intvalue
@dataclass
class E(StructMixin):
    op: Any = field(Enum(Int8ub, READ=1))

e = E.parse(b"\x01")
print("intvalue:", e.op.intvalue if hasattr(e.op, "intvalue") else "N/A")

# EnumInteger fallback: type name + int() + build
e2 = E.parse(b"\xff")
print("fallback type:", type(e2.op).__name__, "| int:", int(e2.op))
print("build with EnumInteger:", E(op=e2.op).build().hex())

# Union nested inside outer struct + tail read
@dataclass
class UN(StructMixin):
    head: int = field(Int8ub)
    u: Any = field(Union(None, raw=Bytes(2), num=Int16ub))
    tail: bytes = field(Bytes(1))

un = UN.parse(b"\x2a" + b"AB" + b"Z")
print("nested union:", un.head, un.u, un.tail)
print("nested build:", UN(head=0x2A, u=dict(raw=b"AB"), tail=b"Z").build() == b"\x2aABZ")

# Aligned + 变长字段（Bytes(x+1)）交互
from neoconstruct import Aligned

@dataclass
class AV(StructMixin):
    x: int = field(Int8ub)
    v: Any = field(Aligned(4, Bytes(x + 1)))

av = AV(x=1, v=b"ab")
print("aligned varlen build:", av.build().hex())
avp = AV.parse(b"\x01ab\x00\x00Z")
print("aligned varlen parse:", avp.x, avp.v)

# Timestamp 0.001 unit（毫秒 tick → 秒）
from neoconstruct import Timestamp, Int64ub

@dataclass
class TM(StructMixin):
    ts: Any = field(Timestamp(Int64ub, 0.001, 1970))

tm = TM.parse((3_600_000).to_bytes(8, "big"))
print("ms tick 3600000 ->", tm.ts)
print("ms roundtrip:", TM(ts=tm.ts).build() == (3_600_000).to_bytes(8, "big"))
