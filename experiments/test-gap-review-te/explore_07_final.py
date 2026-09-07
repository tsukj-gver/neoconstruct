# Exploratory session 07: None-payload rebuilds, error hierarchy, field() misuse, inheritance extras
from dataclasses import dataclass
from neoconstruct import (
    StructMixin, field, rfield, Int8ub, Int16ub, If, IfThenElse, Select,
    Switch, ConstructError, Pass, GreedyBytes, Int32ub,
)

print("=== AB. If(cond=False) parse->None then rebuild ===")
@dataclass
class IFC(StructMixin):
    x: int = field(Int8ub)
    v: object = field(If(x > 0, Int8ub))

r = IFC.parse(b"\x00")   # x=0 -> v None, no bytes
print("parsed:", r)
try:
    print("rebuild:", r.build())
except Exception as e:
    print(f"rebuild FAIL: {type(e).__name__}: {str(e)[:120]}")

print()
print("=== AC. Select fallback None rebuild ===")
@dataclass
class SelP(StructMixin):
    v: object = field(Select(If(False, Int8ub)))  # degenerate: always Pass-ish
try:
    r = SelP.parse(b"")
    print("parsed:", r)
    print("rebuild:", r.build())
except Exception as e:
    print(f"{type(e).__name__}: {str(e)[:120]}")

print()
print("=== AD. error class hierarchy (catchability) ===")
from neoconstruct import (
    StreamError, FieldValueMissingError, FormatFieldError, FieldLengthError,
    RangeError, StringError, IntegerError, ChecksumError, CompilationError,
)
import inspect
for cls in [StreamError, FieldValueMissingError, FormatFieldError, FieldLengthError,
            RangeError, StringError, IntegerError, ChecksumError, CompilationError]:
    print(f"  {cls.__name__}: bases={[c.__name__ for c in cls.__mro__[1:3]]}")
try:
    from neoconstruct import BitFieldError
    print("  BitFieldError present")
except ImportError:
    print("  BitFieldError NOT exported")
try:
    from neoconstruct import ValidationError
    print("  ValidationError present")
except ImportError:
    print("  ValidationError NOT exported")

# does the exception object carry structured path?
@dataclass
class E(StructMixin):
    a: int = field(Int8ub)
    b: int = field(Int8ub)
try:
    E.parse(b"\x01")
except ConstructError as e2:
    print("  attrs:", [a for a in dir(e2) if not a.startswith("_")])
    print("  has .path:", hasattr(e2, "path"), "| .path =", getattr(e2, "path", None))

print()
print("=== AE. field() misuse messages ===")
for label, fn in [
    ("field() no args", lambda: field()),
    ("field(42)", lambda: field(42)),
    ("field('x')", lambda: field("x")),
]:
    try:
        v = fn()
        print(f"  {label}: created {v!r}")
    except Exception as e:
        print(f"  {label}: {type(e).__name__}: {str(e)[:110]}")

print()
print("=== AF. inheritance extras ===")
@dataclass
class BaseH(StructMixin):
    magic: int = field(Int16ub)

@dataclass
class ExtEmpty(BaseH):
    pass

try:
    print("empty subclass build:", ExtEmpty(magic=0xCAFE).build())
    print("empty subclass parse:", ExtEmpty.parse(b"\xca\xfe"))
except Exception as e:
    print(f"empty subclass: {type(e).__name__}: {str(e)[:130]}")

# base with rfield
@dataclass
class BaseR(StructMixin):
    v: int = field(Int8ub)
    double: int = rfield(__import__("neoconstruct").Computed(v * 2))

@dataclass
class ExtR(BaseR):
    w: int = field(Int8ub)
try:
    print("rfield inherit:", ExtR(v=2, w=3).build())
except Exception as e:
    print(f"rfield inherit: {type(e).__name__}: {str(e)[:130]}")

print()
print("=== AG. GreedyBytes starvation (user error: trailing fields after greedy) ===")
@dataclass
class Greedy(StructMixin):
    data: bytes = field(GreedyBytes)
    tail: int = field(Int32ub)

r = Greedy.parse(b"\x01\x02\x03\x04\x05")
print(f"parsed data={r.data!r} tail={r.tail!r}  # tail starved -> stale/missing?")

print()
print("=== AH. Timestamp doc vs error mismatch ===")
from neoconstruct import Timestamp
try:
    ts = Timestamp(Int32ub, "unix", 0)
except Exception as e:
    print(f"Timestamp('unix'): {type(e).__name__}: {str(e)[:150]}")
try:
    ts = Timestamp(Int32ub, 1.0, 0)
    print("Timestamp(1.0) created:", ts)
except Exception as e:
    print(f"Timestamp(1.0): {type(e).__name__}: {str(e)[:120]}")
