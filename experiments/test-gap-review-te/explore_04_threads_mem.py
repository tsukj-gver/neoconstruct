# Exploratory session 04: deepen F/G/H + threads + memory + recursion
from dataclasses import dataclass
from neoconstruct import StructMixin, field, rfield, Int8ub, Int16ub

print("=== F2. forget @dataclass: what do you actually get? ===")
class NoDC(StructMixin):
    a: int = field(Int8ub)

r = NoDC.parse(b"\x05")
print("type:", type(r).__name__, "| hasattr a:", hasattr(r, "a"))
if hasattr(r, "a"):
    print("  a =", r.a)
try:
    r2 = NoDC(a=9)
    print("  NoDC(9) works, .a =", getattr(r2, "a", "MISSING"))
    try:
        print("  build:", r2.build())
    except Exception as e:
        print(f"  build: {type(e).__name__}: {str(e)[:100]}")
except Exception as e:
    print(f"  NoDC(9): {type(e).__name__}: {str(e)[:100]}")

print()
print("=== G2. shadowed dataclasses.field: bytes consumed? ===")
from dataclasses import field as dc_field

@dataclass
class Shadowed(StructMixin):
    a: int = dc_field(default=5)

inst = Shadowed.parse(b"\x00")
print("parse(b'\\x00') ->", inst, "  # default 5 kept, input ignored => silent wrong parse")

print()
print("=== H2. inheritance: silent build corruption detail ===")
@dataclass
class BaseHeader(StructMixin):
    magic: int = field(Int16ub)

@dataclass
class ExtendedHeader(BaseHeader):
    version: int = field(Int8ub)

b = ExtendedHeader(magic=0xCAFE, version=7).build()
print("build:", b, " len:", len(b), " # magic bytes silently missing!")
print("sizeof expectations: 3 bytes expected, got", len(b))
# parse crash class:
try:
    ExtendedHeader.parse(b"\x00\x01\x02")
except Exception as e:
    print(f"parse: {type(e).__name__} (not ConstructError!): {str(e)[:100]}")

print()
print("=== O. threads: concurrent parse/build of same class ===")
import threading

@dataclass
class P(StructMixin):
    a: int = field(Int8ub)
    b: int = field(Int16ub)

errors = []
def worker(n):
    try:
        for i in range(2000):
            data = bytes([i % 256]) + (i * 7 % 65536).to_bytes(2, "big")
            p = P.parse(data)
            assert p.build() == data, f"roundtrip mismatch {data!r} -> {p!r} -> {p.build()!r}"
    except Exception as e:
        errors.append(f"t{n}: {type(e).__name__}: {e}")

ts = [threading.Thread(target=worker, args=(i,)) for i in range(8)]
for t in ts: t.start()
for t in ts: t.join()
print("concurrent parse/build:", "OK" if not errors else errors[:3])

print()
print("=== P. concurrent class compilation (Jupyter parallel / metaclass races) ===")
errors2 = []
def definer(n):
    try:
        for i in range(50):
            cls = dataclass(type(f"C{n}_{i}", (StructMixin,), {
                "__annotations__": {"x": int, "y": int},
                "x": field(Int8ub), "y": field(Int8ub),
            }))
            cls.parse(b"\x01\x02")
    except Exception as e:
        errors2.append(f"d{n}: {type(e).__name__}: {e}")

ts2 = [threading.Thread(target=definer, args=(i,)) for i in range(4)]
for t in ts2: t.start()
for t in ts2: t.join()
print("concurrent class creation:", "OK" if not errors2 else errors2[:3])

print()
print("=== Q. memory: parse loop leak check ===")
import gc, os
try:
    import psutil; HAVE = True
except ImportError:
    HAVE = False
    print("(no psutil; using coarse check via tracemalloc)")
    import tracemalloc
    tracemalloc.start()

blob = b"\x0A\x00\x0F" * 10000
@dataclass
class Big(StructMixin):
    n: int = field(Int8ub)
    items: bytes = field(Int16ub)  # placeholder type fix below

# simpler: fixed struct
@dataclass
class S3(StructMixin):
    a: int = field(Int8ub)
    b: int = field(Int8ub)
    c: int = field(Int8ub)

data = bytes(3000)
if HAVE:
    proc = psutil.Process(os.getpid())
    gc.collect(); base = proc.memory_info().rss
    for _ in range(200000):
        S3.parse(data)
    gc.collect()
    after = proc.memory_info().rss
    print(f"rss before={base/1e6:.1f}MB after={after/1e6:.1f}MB delta={(after-base)/1e6:.2f}MB")
else:
    gc.collect(); snap1 = tracemalloc.take_snapshot()
    for _ in range(100000):
        S3.parse(data)
    gc.collect(); snap2 = tracemalloc.take_snapshot()
    diff = snap2.compare_to(snap1, "lineno")
    total = sum(d.size_diff for d in diff)
    print(f"tracemalloc total diff: {total/1e6:.2f}MB")
    for d in diff[:5]:
        print("  ", d)

print()
print("=== R. class (re)definition leak: __init_subclass__ compile cache ===")
if HAVE:
    proc = psutil.Process(os.getpid())
    gc.collect(); base = proc.memory_info().rss
    for i in range(3000):
        cls = dataclass(type(f"Tmp{i}", (StructMixin,), {
            "__annotations__": {"x": int, "y": int, "z": int},
            "x": field(Int8ub), "y": field(Int8ub), "z": field(Int8ub),
        }))
        cls.parse(b"\x01\x02\x03")
    gc.collect()
    after = proc.memory_info().rss
    print(f"3000 class defs: rss delta={(after-base)/1e6:.2f}MB")
else:
    print("(skipped, no psutil)")

print()
print("=== S. deep nesting recursion ===")
def make_nested(depth):
    prev = None
    for i in range(depth):
        annotations = {"v": int}
        vals = {"v": field(Int8ub)}
        if prev is not None:
            annotations["child"] = prev
            vals["child"] = field(prev)
        cls = dataclass(type(f"L{i}", (StructMixin,), {
            "__annotations__": annotations, **vals}))
        prev = cls
    return prev

for depth in (32, 128, 256):
    try:
        top = make_nested(depth)
        # build bytes bottom-up: single byte chain
        b = bytes(depth)
        obj = top.parse(b)
        print(f"depth {depth}: parse OK")
    except RecursionError as e:
        print(f"depth {depth}: RecursionError")
    except Exception as e:
        print(f"depth {depth}: {type(e).__name__}: {str(e)[:80]}")
