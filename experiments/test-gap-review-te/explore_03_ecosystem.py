# Exploratory session 03: ecosystem integration & first-hour misuse patterns
from dataclasses import dataclass
print("=== F. forget @dataclass ===")
from neoconstruct import StructMixin, field, Int8ub

try:
    class NoDC(StructMixin):
        a: int = field(Int8ub)
    print("class created. parse:", NoDC.parse(b"\x05"))
except Exception as e:
    print(f"no-@dataclass: {type(e).__name__}: {str(e)[:160]}")

print()
print("=== G. dataclasses.field vs neoconstruct.field shadowing ===")
from dataclasses import dataclass as dc_dataclass
from dataclasses import field as dc_field

try:
    @dataclass
    class Shadowed(StructMixin):
        a: int = dc_field(default=5)   # user thinks this is neoconstruct field
    print("shadowed class created:", Shadowed(a=1))
    try:
        print("parse:", Shadowed.parse(b"\x05"))
    except Exception as e:
        print(f"  parse: {type(e).__name__}: {str(e)[:140]}")
except Exception as e:
    print(f"shadowed creation: {type(e).__name__}: {str(e)[:160]}")

print()
print("=== H. inheritance (base protocol header, extend in subclass) ===")
from neoconstruct import Int16ub

@dataclass
class BaseHeader(StructMixin):
    magic: int = field(Int16ub)

@dataclass
class ExtendedHeader(BaseHeader):
    version: int = field(Int8ub)

try:
    h = ExtendedHeader(magic=1, version=2)
    print("inherit build:", h.build())
    print("inherit parse:", ExtendedHeader.parse(b"\x00\x01\x02"))
except Exception as e:
    print(f"inheritance: {type(e).__name__}: {str(e)[:200]}")

print()
print("=== I. empty struct / only-rfield struct ===")
from neoconstruct import rfield, Tell, Computed

try:
    @dataclass
    class Empty(StructMixin):
        pass
    print("empty parse:", Empty.parse(b""))
    print("empty build:", Empty().build())
except Exception as e:
    print(f"empty: {type(e).__name__}: {str(e)[:140]}")

print()
print("=== J. class redefinition (Jupyter / importlib.reload pattern) ===")
import sys
for i in range(3):
    cls_name = f"Redefined{i}"
    cls = dataclass(type(cls_name, (StructMixin,), {
        "__annotations__": {"a": int, "b": int},
        "a": field(Int8ub),
        "b": field(Int8ub),
    }))
    r = cls.parse(b"\x01\x02")
    print(f"iteration {i}: {r}")

print()
print("=== K. hashability / dict usage ===")
@dataclass
class H(StructMixin):
    a: int = field(Int8ub)
try:
    hash(H(a=1))
    print("hashable: yes")
except TypeError as e:
    print("hashable: NO —", str(e)[:80])

print()
print("=== L. with statement / stream API surface ===")
print("StructMixin methods:", [m for m in dir(H) if not m.startswith("_")])
print("classmethods:", [m for m in dir(H) if m.startswith(("parse", "build", "sizeof"))])

import neoconstruct
print("top-level parse/build helpers:", [n for n in dir(neoconstruct) if n in ("parse", "build", "stream", "Struct")])

print()
print("=== M. non-ASCII / keyword field names ===")
try:
    @dataclass
    class KW(StructMixin):
        from_: int = field(Int8ub)   # keyword-ish name from CSVs
    print("from_ ok:", KW.parse(b"\x01"))
except Exception as e:
    print(f"from_: {type(e).__name__}: {str(e)[:120]}")

try:
    cls = dataclass(type("Uni", (StructMixin,), {
        "__annotations__": {"größe": int},
        "größe": field(Int8ub),
    }))
    print("unicode name ok:", cls.parse(b"\x01"))
except Exception as e:
    print(f"unicode field name: {type(e).__name__}: {str(e)[:160]}")

print()
print("=== N. nested error path quality ===")
from neoconstruct import Bytes, Array

@dataclass
class Inner(StructMixin):
    x: int = field(Int8ub)
    y: bytes = field(Bytes(2))

@dataclass
class Outer(StructMixin):
    n: int = field(Int8ub)
    items: list = field(Array(n, Inner))

try:
    Outer.parse(b"\x02\x01\x02\x03\x01")
except Exception as e:
    print(f"nested error: {type(e).__name__}: {str(e)[:200]}")
