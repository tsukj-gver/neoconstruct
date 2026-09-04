"""Probe into/lookahead 细查（复核此前 '<no context>' 观察）。"""

import contextlib
import io

from dataclasses import dataclass
from typing import Any

from neoconstruct import Byte, Probe, StructMixin, field, rfield


@dataclass
class P1(StructMixin):
    a: int = field(Byte)
    p: Any = rfield(Probe(into="a"))
    b: int = field(Byte)


buf = io.StringIO()
with contextlib.redirect_stdout(buf):
    P1.parse(b"\x07\x08")
print("into='a' output:")
print(buf.getvalue())
print("---")

@dataclass
class P2(StructMixin):
    a: int = field(Byte)
    p: Any = rfield(Probe(lookahead=3))
    b: int = field(Byte)


buf2 = io.StringIO()
with contextlib.redirect_stdout(buf2):
    P2.parse(b"\x0a\x0b\x0c\x0d")
print("lookahead=3 output:")
print(buf2.getvalue())
print("---")

@dataclass
class P3(StructMixin):
    a: int = field(Byte)
    p: Any = rfield(Probe(into="missing_field"))
    b: int = field(Byte)


buf3 = io.StringIO()
with contextlib.redirect_stdout(buf3):
    P3.parse(b"\x01\x02")
print("into='missing_field' output:")
print(buf3.getvalue())
