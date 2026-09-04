"""Probe 值语义细查：field/rfield 两模式 × parse 存值 × build。"""

import io
import contextlib

from dataclasses import dataclass
from typing import Any

from neoconstruct import Byte, Probe, StructMixin, field, rfield


@dataclass
class PRr(StructMixin):
    a: int = field(Byte)
    p: Any = rfield(Probe())
    b: int = field(Byte)


buf = io.StringIO()
with contextlib.redirect_stdout(buf):
    pr = PRr.parse(b"\x01\x02")
print("rfield parse: p =", repr(pr.p))
try:
    print("rfield build:", PRr(a=1, b=2).build())
except Exception as e:
    print("rfield build from scratch ->", type(e).__name__, str(e)[:80])
try:
    print("rfield build after parse:", pr.build())
except Exception as e:
    print("rfield build after parse ->", type(e).__name__, str(e)[:80])


@dataclass
class PRf(StructMixin):
    a: int = field(Byte)
    p: Any = field(Probe())
    b: int = field(Byte)


buf2 = io.StringIO()
with contextlib.redirect_stdout(buf2):
    pf = PRf.parse(b"\x01\x02")
print("field parse: p =", repr(pf.p))
try:
    print("field build after parse:", pf.build())
except Exception as e:
    print("field build after parse ->", type(e).__name__, str(e)[:80])
try:
    print("field build from scratch:", PRf(a=1, b=2).build())
except Exception as e:
    print("field build from scratch ->", type(e).__name__, str(e)[:80])

# Probe(lookahead) parse 后 build
@dataclass
class PL(StructMixin):
    a: int = field(Byte)
    p: Any = rfield(Probe(lookahead=2))
    b: int = field(Byte)


buf3 = io.StringIO()
with contextlib.redirect_stdout(buf3):
    pl = PL.parse(b"\x01\x02")
print("lookahead p =", repr(pl.p))
try:
    print("lookahead build:", pl.build())
except Exception as e:
    print("lookahead build ->", type(e).__name__, str(e)[:80])
