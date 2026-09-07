# Exploratory session 02: hostile/corrupt input, VM edge cases, DoS surface
from dataclasses import dataclass
from neoconstruct import (
    StructMixin, field, rfield, Int8ub, Int16ub, Int32ub, Bytes, Array,
    Computed, CString, GreedyRange, Prefixed, GreedyBytes, PrefixedArray,
)

print("=== A. corrupt length fields (untrusted input) ===")

@dataclass
class P1(StructMixin):
    count: int = field(Int32ub)
    items: list = field(Array(count, Int16ub))

# count says 100M elements but no data follows -> hang or clean error?
import time
t0 = time.time()
try:
    r = P1.parse(b"\xFF\xFF\xFF\xFF")
    print(f"count=0xFFFFFFFF parsed: {r.count} items={r.items[:3]}... len={len(r.items)}")
except Exception as e:
    print(f"count=0xFFFFFFFF: {type(e).__name__}: {str(e)[:100]}")
print(f"  took {time.time()-t0:.3f}s")

t0 = time.time()
try:
    r = P1.parse(b"\x7F\xFF\xFF\xFF" + b"\x00" * 100)
    print(f"count=0x7FFFFFFF partial data: len(items)={len(r.items)}")
except Exception as e:
    print(f"count=0x7FFFFFFF partial: {type(e).__name__}: {str(e)[:80]}")
print(f"  took {time.time()-t0:.3f}s")

@dataclass
class P1b(StructMixin):
    count: int = field(Int32ub)   # signed
    items: list = field(Array(count, Int8ub))
try:
    r = P1b.parse(b"\xFF\xFF\xFF\xFF")
    print("negative count (signed Int32sl):", r.count, len(r.items))
except Exception as e:
    print("negative count (signed):", type(e).__name__, str(e)[:100])

# Bytes with huge length
@dataclass
class P2(StructMixin):
    n: int = field(Int32ub)
    data: bytes = field(Bytes(n))
t0 = time.time()
try:
    r = P2.parse(b"\xFF\xFF\xFF\xFF")
    print("Bytes(0xFFFFFFFF):", len(r.data))
except Exception as e:
    print(f"Bytes(0xFFFFFFFF): {type(e).__name__}: {str(e)[:80]}")
print(f"  took {time.time()-t0:.3f}s")

print()
print("=== B. expression VM: division by zero (Rust panic risk?) ===")

@dataclass
class D(StructMixin):
    a: int = field(Int8ub)
    b: int = field(Int8ub)
    q: int = rfield(Computed(a // b))
    m: int = rfield(Computed(a % b))

try:
    r = D.parse(b"\x0A\x00")
    print("div-by-zero parse:", r)
except Exception as e:
    print(f"div-by-zero parse: {type(e).__name__}: {str(e)[:120]}")

# shift overflow (i64 VM)
@dataclass
class S(StructMixin):
    a: int = field(Int8ub)
    big: int = rfield(Computed(a << 62))

try:
    r = S.parse(b"\x02")
    print("a<<62 =", r.big, "(expect 2<<62 =", 2 << 62, ")")
except Exception as e:
    print(f"a<<62: {type(e).__name__}: {str(e)[:120]}")

try:
    r2 = S.parse(b"\x08")
    print("8<<62 =", r2.big, "python:", 8 << 62)
except Exception as e:
    print(f"8<<62: {type(e).__name__}: {str(e)[:120]}")

print()
print("=== C. string edge cases ===")

@dataclass
class StrP(StructMixin):
    name: str = field(CString("utf8"))

# no terminator at all
try:
    print(StrP.parse(b"hello"))
except Exception as e:
    print("no terminator:", type(e).__name__, str(e)[:100])

# invalid utf8
try:
    print(StrP.parse(b"\xff\xfe\x00"))
except Exception as e:
    print("invalid utf8:", type(e).__name__, str(e)[:100])

# empty string
try:
    print(StrP.parse(b"\x00"))
except Exception as e:
    print("empty:", type(e).__name__, str(e)[:100])

print()
print("=== D. GreedyRange with trailing partial element (corrupt tail) ===")

@dataclass
class Item(StructMixin):
    x: int = field(Int8ub)
    y: int = field(Int8ub)

@dataclass
class SeqP(StructMixin):
    items: list = field(GreedyRange(Item))

# 2 full items + 1 dangling byte
try:
    r = SeqP.parse(b"\x01\x02\x03\x04\x05")
    print(f"partial tail: len={len(r.items)} — dangling byte silently dropped? items={r.items}")
    rebuilt = r.build()
    print("rebuild != input:", rebuilt != b"\x01\x02\x03\x04\x05")
except Exception as e:
    print("partial tail:", type(e).__name__, str(e)[:100])

print()
print("=== E. Switch default=Pass round-trip invariant ===")
from neoconstruct import Switch

@dataclass
class TypeA(StructMixin):
    a: int = field(Int8ub)

@dataclass
class Frame(StructMixin):
    type_code: int = field(Int8ub)
    payload: object = field(Switch(type_code, {1: TypeA}))

f = Frame.parse(b"\x07\xAA")   # unknown type_code -> payload None
print("parsed:", f)
try:
    out = f.build()
    print("rebuild of unknown-switch:", out, "round-trip broken:", out != b"\x07\xAA")
except Exception as e:
    print("rebuild unknown-switch FAIL:", type(e).__name__, str(e)[:100])
