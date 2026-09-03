# 边界补充：Array count 嵌套 / Switch(typ+1) 运行时 / Checksum 正确用法
import traceback
from dataclasses import dataclass
from typing import Any
from construct import (StructMixin, field, rfield, wfield, Int8ub, Bytes,
                       Prefixed, Switch, Array, Checksum, HashAlgo)

def show(name, fn):
    print(f"=== {name} ===")
    try:
        r = fn()
        print(f"[OK] {r}")
    except Exception as e:
        print(f"[FAIL] {type(e).__name__}: {str(e).splitlines()[0][:150]}")
    print()

def t_array_wrapped():
    @dataclass
    class P(StructMixin):
        typ: int = field(Int8ub)
        br: Any = field(Prefixed(Int8ub, Array(typ, Int8ub)))
    return P
show("Prefixed x Array(typ) define", t_array_wrapped)

def t_array_bare():
    @dataclass
    class P(StructMixin):
        typ: int = field(Int8ub)
        br: Any = field(Array(typ, Int8ub))
    return P.parse(b"\x02\x01\x02")
show("bare Array(typ) parse [control]", t_array_bare)

def t_switch_plus_runtime():
    @dataclass
    class P(StructMixin):
        typ: int = field(Int8ub)
        br: Any = field(Switch(typ + 1, {2: Int8ub, 3: Bytes(2)}))
    return P.parse(b"\x01\x7f"), P(typ=1, br=127).build()
show("bare Switch(typ+1) parse+build runtime", t_switch_plus_runtime)

def t_checksum_wrapped():
    @dataclass
    class P(StructMixin):
        data: bytes = field(Bytes(4))
        crc: Any = field(Prefixed(Int8ub, Checksum(lambda data: 0)))
    return P
show("Prefixed x Checksum(lambda:0) define", t_checksum_wrapped)

def t_checksum_bare():
    @dataclass
    class P(StructMixin):
        data: bytes = field(Bytes(4))
        crc: Any = field(Checksum(lambda data: 0))
    return P.parse(b"\x01\x02\x03\x04\x00\x00")
show("bare Checksum(lambda) define/parse [control]", t_checksum_bare)
