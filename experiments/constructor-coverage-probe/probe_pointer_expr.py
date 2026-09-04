"""Pointer offset 表达式路径探针。"""

from dataclasses import dataclass

from neoconstruct import Bytes, Int8ub, Pointer, StructMixin, field


@dataclass
class P(StructMixin):
    off: int = field(Int8ub)
    ptr: bytes = field(Pointer(off, Bytes(1)))

p = P.parse(b"\x03XYZW")
print("expr offset parse: off=3 ptr=", p.ptr)

@dataclass
class P2(StructMixin):
    off: int = field(Int8ub)
    ptr: bytes = field(Pointer(off + 1, Bytes(1)))

p2 = P2.parse(b"\x03XYZW")
print("expr offset+1 parse: ptr=", p2.ptr)
print("build:", P2(off=3, ptr=b"W").build().hex())
