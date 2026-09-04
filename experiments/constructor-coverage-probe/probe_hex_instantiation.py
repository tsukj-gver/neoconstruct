"""Hex/HexDump 实例化语义差异复核。"""

from dataclasses import dataclass
from typing import Any

from neoconstruct import Bytes, Hex, HexDump, Int32ub, StructMixin, field


def check(cls, name):
    try:
        inst = cls()
        print(f"{cls.__name__}() OK -> {name} = {getattr(inst, name)!r}")
    except TypeError:
        print(f"{cls.__name__}() -> TypeError (required)")


@dataclass
class H1(StructMixin):
    v: Any = field(Hex(Int32ub))

check(H1, "v")

@dataclass
class H2(StructMixin):
    v: Any = field(Hex(Bytes(4)))

check(H2, "v")

@dataclass
class H3(StructMixin):
    v: Any = field(HexDump(Int32ub))

check(H3, "v")

@dataclass
class H4(StructMixin):
    v: Any = field(HexDump(Bytes(4)))

check(H4, "v")

@dataclass
class H5(StructMixin):
    v: Any = field(Bytes(4))

check(H5, "v")
