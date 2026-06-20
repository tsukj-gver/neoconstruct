#!/usr/bin/env python3
"""Phase 16 build-path micro-benchmark: isolate per-operation costs.

This script measures the Python-visible cost of individual operations that
the Rust build path performs, by timing equivalent Python-level operations
and Rust entry points. The goal is to identify which operations dominate
the build time.
"""

from __future__ import annotations

import gc
import os
import sys
import time

for _stream in (sys.stdout, sys.stderr):
    if hasattr(_stream, "reconfigure"):
        try:
            _stream.reconfigure(encoding="utf-8")
        except (ValueError, OSError):
            pass

_HERE = os.path.dirname(os.path.abspath(__file__))
_CONSTRUCT_PY_DIR = os.path.abspath(os.path.join(_HERE, ".."))
if _CONSTRUCT_PY_DIR not in sys.path:
    sys.path.append(_CONSTRUCT_PY_DIR)

from construct_rust import (  # noqa: E402
    Array as RsArray,
    Bytes as RsBytes,
    Int16ub as RsInt16ub,
    Int32ub as RsInt32ub,
    Struct as RsStruct,
    compile_schema as rs_compile_schema,
)


def measure(fn, iterations=10000):
    fn()  # warmup
    gc.collect()
    gc_was = gc.isenabled()
    gc.disable()
    try:
        t0 = time.perf_counter()
        for _ in range(iterations):
            fn()
        elapsed = time.perf_counter() - t0
    finally:
        if gc_was:
            gc.enable()
    return elapsed / iterations


def fmt(secs):
    return f"{secs * 1e6:.3f}µs" if secs >= 1e-6 else f"{secs * 1e9:.1f}ns"


# =========================================================================
# Test 1: Build time breakdown by format
# =========================================================================

print("=" * 72)
print("  Test 1: Build time per format")
print("=" * 72)

formats = {
    "simple (3 fields)": (
        RsStruct("magic" / RsBytes(4), "a" / RsInt16ub, "b" / RsInt32ub),
        {"magic": b"ABCD", "a": 1, "b": 2},
    ),
    "nested (2-level)": (
        RsStruct(
            "header" / RsStruct("magic" / RsBytes(4), "version" / RsInt16ub),
            "payload" / RsStruct("size" / RsInt32ub, "flags" / RsInt32ub),
        ),
        {"header": {"magic": b"ABCD", "version": 1}, "payload": {"size": 2, "flags": 3}},
    ),
    "array (100 elem)": (
        RsStruct("count" / RsInt32ub, "items" / RsArray(100, RsInt32ub)),
        {"count": 100, "items": list(range(100))},
    ),
    "array (1 elem)": (
        RsStruct("count" / RsInt32ub, "items" / RsArray(1, RsInt32ub)),
        {"count": 100, "items": [42]},
    ),
    "array (10 elem)": (
        RsStruct("count" / RsInt32ub, "items" / RsArray(10, RsInt32ub)),
        {"count": 100, "items": list(range(10))},
    ),
    "struct-only (2 int)": (
        RsStruct("a" / RsInt32ub, "b" / RsInt32ub),
        {"a": 1, "b": 2},
    ),
}

for name, (decl, obj) in formats.items():
    holder = rs_compile_schema(decl)
    t = measure(lambda: holder.build_from_py(obj))
    print(f"  {name:30s}  {fmt(t)}")


# =========================================================================
# Test 2: Scaling — array build vs element count
# =========================================================================

print()
print("=" * 72)
print("  Test 2: Array build scaling (elements → time)")
print("=" * 72)
print(f"  {'elems':>6}  {'time':>12}  {'per-elem':>12}  {'overhead':>12}")

for n in [0, 1, 5, 10, 50, 100, 200, 500]:
    decl = RsStruct("count" / RsInt32ub, "items" / RsArray(n, RsInt32ub))
    obj = {"count": n, "items": list(range(n))}
    holder = rs_compile_schema(decl)
    t = measure(lambda: holder.build_from_py(obj), iterations=max(2000, 10000 // max(1, n)))
    per_elem = t / max(1, n)
    # Linear regression hint: overhead = t - per_elem * n
    print(f"  {n:>6}  {fmt(t):>12}  {fmt(per_elem):>12}  {fmt(t - per_elem * n):>12}")


# =========================================================================
# Test 3: Python-dict access cost (baseline for FFI overhead estimation)
# =========================================================================

print()
print("=" * 72)
print("  Test 3: Python-level baseline costs (for FFI comparison)")
print("=" * 72)

d = {"a": 1, "b": 2, "c": 3}
lst = list(range(100))

# dict getitem
t = measure(lambda: d.get("a"))
print(f"  dict.get('a'):              {fmt(t)}")

# dict contains
t = measure(lambda: "a" in d)
print(f"  'a' in dict:                {fmt(t)}")

# list getitem
t = measure(lambda: lst[50])
print(f"  list[50]:                   {fmt(t)}")

# int to bytes (what Python construct does for build)
t = measure(lambda: (42).to_bytes(4, "big"))
print(f"  (42).to_bytes(4,'big'):     {fmt(t)}")


# =========================================================================
# Test 4: Pure-Rust build path (Value path, no FFI) for comparison
# =========================================================================

print()
print("=" * 72)
print("  Test 4: parse (Py path) vs build (Py path) comparison")
print("=" * 72)

for name, (decl, obj) in formats.items():
    holder = rs_compile_schema(decl)
    # Build to get data, then parse it back
    data = holder.build_from_py(obj)
    t_build = measure(lambda: holder.build_from_py(obj))
    t_parse = measure(lambda: holder.parse_bytes_py(data))
    print(f"  {name:30s}  build={fmt(t_build):>10}  parse={fmt(t_parse):>10}  "
          f"build/parse={t_build/t_parse:.2f}x")


# =========================================================================
# Test 5: Context value extraction cost isolation
# =========================================================================

print()
print("=" * 72)
print("  Test 5: Isolating extract_ctx_value overhead")
print("=" * 72)
print("  (Comparing Struct-with-expr vs Struct-without-expr build)")
print()

# A struct WITHOUT any context expressions (this._ or this.field references)
decl_no_expr = RsStruct("a" / RsInt32ub, "b" / RsInt32ub, "c" / RsInt32ub)
obj3 = {"a": 1, "b": 2, "c": 3}
holder = rs_compile_schema(decl_no_expr)
t = measure(lambda: holder.build_from_py(obj3))
print(f"  3-int struct build:         {fmt(t)}")

# vs single int build
decl_one = RsStruct("a" / RsInt32ub)
obj1 = {"a": 1}
holder1 = rs_compile_schema(decl_one)
t1 = measure(lambda: holder1.build_from_py(obj1))
print(f"  1-int struct build:         {fmt(t1)}")
print(f"  marginal cost per field:    {fmt((t - t1) / 2)}")
print(f"  (design predicted ~140ns/field; FFI ~4 calls)")
