"""实例化值语义（D2/D3）快速探针：Aligned / ProcessXor / ProcessRotateLeft
/ Timestamp / HexDump。"""

from dataclasses import dataclass
from typing import Any

from neoconstruct import (
    Aligned,
    Byte,
    HexDump,
    Bytes,
    Int16ub,
    Int32ub,
    Int64ub,
    ProcessRotateLeft,
    ProcessXor,
    StructMixin,
    Timestamp,
    field,
)


def check(name, cls, **kwargs):
    try:
        inst = cls(**kwargs)
        v = getattr(inst, name)
        print(f"{cls.__name__}() OK -> {name} = {v!r}")
    except TypeError as e:
        print(f"{cls.__name__}() -> TypeError (required)")


@dataclass
class A(StructMixin):
    v: Any = field(Aligned(4, Int16ub))

check("v", A)

@dataclass
class X(StructMixin):
    k: int = field(Byte)
    v: Any = field(ProcessXor(0xF0, Int16ub))

check("v", X, k=1)

@dataclass
class R(StructMixin):
    v: Any = field(ProcessRotateLeft(4, 1, Int16ub))

check("v", R)

@dataclass
class T(StructMixin):
    v: Any = field(Timestamp(Int64ub, 1.0, 1970))

check("v", T)

@dataclass
class H(StructMixin):
    v: Any = field(HexDump(Bytes(4)))

check("v", H)

# AlignedStruct 字段实例化（动态类 kw_only 行为）
from neoconstruct import AlignedStruct

AP = AlignedStruct(4, a=Byte, b=Int16ub)
try:
    print("AlignedStruct() ->", AP())
except TypeError as e:
    print("AlignedStruct() -> TypeError:", str(e)[:60])
try:
    print("AlignedStruct(a=1) ->", AP(a=1))
except TypeError as e:
    print("AlignedStruct(a=1) -> TypeError:", str(e)[:60])
