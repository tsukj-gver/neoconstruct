"""G1 十六构造器用户面行为探针：ProcessXor / ProcessRotateLeft / Timestamp
/ Pointer / 补充（Union build None、Sequence build 缺元素、NamedTuple Sequence
build 非 tuple）。"""

from dataclasses import dataclass
from typing import Any

from neoconstruct import (
    Byte,
    Bytes,
    GreedyBytes,
    Int16ub,
    Int64ub,
    Int8ub,
    Pointer,
    ProcessRotateLeft,
    ProcessXor,
    Sequence,
    StructMixin,
    Timestamp,
    TimestampError,
    Union,
    UnionError,
    field,
    rfield,
)


def banner(title):
    print(f"\n{'=' * 20} {title} {'=' * 20}")


banner("ProcessXor int pad")
@dataclass
class X(StructMixin):
    key: int = field(Byte)
    data: Any = field(ProcessXor(0xF0, Int16ub))

x = X.parse(b"\x2a\x00\xff")
print("parse (key byte + xor'd int16):", x.key, repr(x.data), hex(int(x.data)))
print("build:", X(key=0x2A, data=0x0F0F).build().hex())
print("roundtrip:", X.parse(b"\x2a\x00\xff").build() == b"\x2a\x00\xff")

banner("ProcessXor bytes pad / pad=0")
@dataclass
class XB(StructMixin):
    data: Any = field(ProcessXor(b"\xf0\xf1", Int16ub))

xb = XB.parse(b"\x00\xff")
print("bytes pad parse:", repr(xb.data), hex(int(xb.data)))

@dataclass
class X0(StructMixin):
    data: Any = field(ProcessXor(0, Int16ub))

print("pad 0 parse:", repr(X0.parse(b"\x00\xff").data))
print("empty input all-same:", end=" ")
@dataclass
class XE(StructMixin):
    data: Any = field(ProcessXor(0xFF, Bytes(0)))
try:
    print(repr(XE.parse(b"").data), "build:", XE(data=b"").build())
except Exception as e:
    print(type(e).__name__, e)

banner("ProcessXor pad expr (field ref)")
@dataclass
class XP(StructMixin):
    pad: int = field(Byte)
    data: Any = field(ProcessXor(pad, Int16ub))

xp = XP.parse(b"\xf0\x00\xff")
print("expr pad parse:", xp.pad, repr(xp.data), hex(int(xp.data)))
print("expr pad build:", XP(pad=0xF0, data=0x0F0F).build().hex())

banner("ProcessXor callable rejected")
try:
    @dataclass
    class XC(StructMixin):
        data: Any = field(ProcessXor(lambda ctx: 0xFF, Int16ub))
    print("compiled OK?!")
except Exception as e:
    print("callable pad ->", type(e).__name__, str(e)[:100])

banner("ProcessRotateLeft group=1")
@dataclass
class R(StructMixin):
    data: Any = field(ProcessRotateLeft(4, 1, Int16ub))

r = R.parse(b"\x0f\xf0")
print("parse b'\\x0f\\xf0' ->", repr(r.data), hex(int(r.data)))
print("build 0xf00f:", R(data=0xF00F).build().hex())
print("roundtrip:", R.parse(b"\x0f\xf0").build() == b"\x0f\xf0")

banner("ProcessRotateLeft group=2 / amount=0")
@dataclass
class R2(StructMixin):
    data: Any = field(ProcessRotateLeft(4, 2, Int16ub))

print("group2 parse:", repr(R2.parse(b"\x0f\xf0").data), hex(int(R2.parse(b"\x0f\xf0").data)))

@dataclass
class R0(StructMixin):
    data: Any = field(ProcessRotateLeft(0, 1, Int16ub))

print("amount0 parse:", repr(R0.parse(b"\x0f\xf0").data), hex(int(R0.parse(b"\x0f\xf0").data)))

banner("ProcessRotateLeft group=0 -> RotationError")
@dataclass
class RG(StructMixin):
    data: Any = field(ProcessRotateLeft(4, 0, Int16ub))

try:
    RG.parse(b"\x0f\xf0")
except Exception as e:
    print("group 0 parse ->", type(e).__name__, str(e)[:90])
# 注：group=0 build 侧触发 Rust panic（divisor 0, process_rotate_left.rs:121）
# —— 已确认为缺陷，此探针不再重复触发（防子进程崩溃）。

banner("Timestamp epoch seconds")
@dataclass
class T(StructMixin):
    ts: Any = field(Timestamp(Int64ub, 1., 1970))

t = T.parse(b"\x00\x00\x00\x00ZIz\x00")
print("parse ->", repr(t.ts))
print("build:", T(ts=t.ts).build() == b"\x00\x00\x00\x00ZIz\x00")

banner("Timestamp ms + epoch offset")
@dataclass
class TM(StructMixin):
    ts: Any = field(Timestamp(Int64ub, 1000., 1970))

tm = TM.parse((3600 * 1000).to_bytes(8, "big"))
print("1h in ms ->", repr(tm.ts))

banner("Timestamp epoch=2000")
@dataclass
    # epoch year 2000
class T2(StructMixin):
    ts: Any = field(Timestamp(Int64ub, 1., 2000))

t2 = T2.parse((0).to_bytes(8, "big"))
print("t=0 at epoch 2000 ->", repr(t2.ts))

banner("Timestamp bad params")
try:
    Timestamp(Int64ub, None, 1970)
except Exception as e:
    print("unit=None ->", type(e).__name__, str(e)[:80])
try:
    Timestamp(Int64ub, 1., None)
except Exception as e:
    print("epoch=None ->", type(e).__name__, str(e)[:80])
try:
    Timestamp(None, 1., 1970)
except Exception as e:
    print("subcon=None ->", type(e).__name__, str(e)[:80])

banner("Pointer absolute")
@dataclass
class P(StructMixin):
    ptr: bytes = field(Pointer(8, Bytes(1)))

print("parse:", repr(P.parse(b"abcdefghijkl").ptr))
print("build zero-pad:", P(ptr=b"Z").build() == b"\x00" * 8 + b"Z")

banner("Pointer negative / relative")
@dataclass
class PN(StructMixin):
    ptr: bytes = field(Pointer(-2, Bytes(1)))

print("neg parse:", repr(PN.parse(b"abcdefgh").ptr))

@dataclass
class PR(StructMixin):
    head: bytes = field(Bytes(3))
    ptr: bytes = field(Pointer(2, Bytes(1), relativeOffset=True))
    tail: bytes = field(Bytes(1))

pr = PR.parse(b"abcdefg")
print("relative:", pr.head, pr.ptr, pr.tail)

banner("Pointer stream= rejected")
try:
    @dataclass
    class PS(StructMixin):
        ptr: bytes = field(Pointer(8, Bytes(1), stream=lambda ctx: None))
    print("compiled OK?!")
except Exception as e:
    print("stream= ->", type(e).__name__, str(e)[:80])

banner("Union build None")
@dataclass
class U(StructMixin):
    u: Any = field(Union(None, raw=Bytes(2)))

try:
    U(u=None).build()
except Exception as e:
    print("build None ->", type(e).__name__, str(e)[:80])

banner("Sequence build missing element (inner Default)")
from neoconstruct import Default, Int8ub as I8

@dataclass
class SV(StructMixin):
    seq: Any = field(Sequence(I8, Default(I8, 0)))

try:
    print("build [7]:", SV(seq=[7]).build().hex())
except Exception as e:
    print("build [7] ->", type(e).__name__, str(e)[:90])

banner("NamedTuple Sequence build non-iterable")
from neoconstruct import NamedTuple

@dataclass
class NS(StructMixin):
    pair: Any = field(NamedTuple("pair", "a b", Sequence(Int8ub, Int8ub)))

try:
    NS(pair=123).build()
except Exception as e:
    print("build int ->", type(e).__name__, str(e)[:90])
