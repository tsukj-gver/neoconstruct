#!/usr/bin/env python3
"""Phase 16 build-path FFI cost isolation.

Measures:
1. Pure entry-point overhead (Struct with 0 fields)
2. Per-scalar-field marginal cost
3. shallow_py_to_value_for_ctx list-iteration cost (via array scaling delta)
4. Container/ListContainer creation cost (via cProfile correlation)
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


def measure(fn, iterations=20000):
    fn()
    gc.collect()
    gc_was = gc.isenabled()
    gc.disable()
    try:
        t0 = time.perf_counter()
        for _ in range(iterations):
            fn()
        return (time.perf_counter() - t0) / iterations
    finally:
        if gc_was:
            gc.enable()


def us(t):
    return f"{t*1e6:.3f}µs"


# =========================================================================
# 1. Pure entry-point overhead (0-field Struct)
# =========================================================================

print("=" * 72)
print("  1. Entry-point overhead isolation")
print("=" * 72)

# 0-field struct: pure FFI entry/exit + ctx setup + stream + PyBytes output
h0 = rs_compile_schema(RsStruct())
t0 = measure(lambda: h0.build_from_py({}))
print(f"  0-field Struct build:        {us(t0)}  (pure entry overhead)")

# 1-field int struct
h1 = rs_compile_schema(RsStruct("a" / RsInt32ub))
t1 = measure(lambda: h1.build_from_py({"a": 1}))
print(f"  1-field Struct build:        {us(t1)}")
print(f"  marginal 1st field:          {us(t1 - t0)}")

# 2-field int struct
h2 = rs_compile_schema(RsStruct("a" / RsInt32ub, "b" / RsInt32ub))
t2 = measure(lambda: h2.build_from_py({"a": 1, "b": 2}))
print(f"  2-field Struct build:        {us(t2)}")
print(f"  marginal 2nd field:          {us(t2 - t1)}")

# 3-field int struct
h3 = rs_compile_schema(RsStruct("a" / RsInt32ub, "b" / RsInt32ub, "c" / RsInt32ub))
t3 = measure(lambda: h3.build_from_py({"a": 1, "b": 2, "c": 3}))
print(f"  3-field Struct build:        {us(t3)}")
print(f"  marginal 3rd field:          {us(t3 - t2)}")

# 5-field int struct
h5 = rs_compile_schema(RsStruct(
    "a" / RsInt32ub, "b" / RsInt32ub, "c" / RsInt32ub,
    "d" / RsInt32ub, "e" / RsInt32ub,
))
t5 = measure(lambda: h5.build_from_py({"a": 1, "b": 2, "c": 3, "d": 4, "e": 5}))
print(f"  5-field Struct build:        {us(t5)}")
print(f"  marginal per field (3→5):    {us((t5 - t3) / 2)}")

# 10-field int struct
decls10 = [("x%d" % i) / RsInt32ub for i in range(10)]
h10 = rs_compile_schema(RsStruct(*decls10))
obj10 = {"x%d" % i: i for i in range(10)}
t10 = measure(lambda: h10.build_from_py(obj10))
print(f"  10-field Struct build:       {us(t10)}")
print(f"  marginal per field (5->10):  {us((t10 - t5) / 5)}")


# =========================================================================
# 2. Array element cost breakdown
# =========================================================================

print()
print("=" * 72)
print("  2. Array per-element cost (marginal)")
print("=" * 72)

base_data = {}
for n in [0, 1, 2, 5, 10, 20, 50, 100, 200]:
    decl = RsStruct("count" / RsInt32ub, "items" / RsArray(n, RsInt32ub))
    obj = {"count": n, "items": list(range(n))}
    h = rs_compile_schema(decl)
    iters = max(3000, 20000 // max(1, n))
    t = measure(lambda: h.build_from_py(obj), iterations=iters)
    base_data[n] = t
    print(f"  Array({n:>3}):  {us(t):>10}")

print()
print("  Marginal per-element (delta):")
prev_n, prev_t = None, None
for n in sorted(base_data):
    if prev_n is not None and n > prev_n:
        delta = base_data[n] - prev_t
        per_elem = delta / (n - prev_n)
        print(f"    {prev_n:>3}→{n:>3}:  per-elem = {us(per_elem)}")
    prev_n, prev_t = n, base_data[n]


# =========================================================================
# 3. shallow list iteration cost estimate
# =========================================================================

print()
print("=" * 72)
print("  3. Estimated shallow_py_to_value_for_ctx(list) cost")
print("=" * 72)
print("  (From array scaling: per-elem cost includes shallow iteration +")
print("   batch extract + per-elem build. Isolating via bytes-array comparison.)")

# Array of bytes(4) — different type probing path in extract_scalar_short_circuit
for n in [10, 50, 100]:
    # int array
    decl_int = RsStruct("items" / RsArray(n, RsInt32ub))
    obj_int = {"items": list(range(n))}
    h_int = rs_compile_schema(decl_int)
    t_int = measure(lambda: h_int.build_from_py(obj_int),
                    iterations=max(3000, 20000 // n))

    # bytes(4) array — extract_scalar_short_circuit probes bytes differently
    decl_bytes = RsStruct("items" / RsArray(n, RsBytes(4)))
    obj_bytes = {"items": [b"ABCD" for _ in range(n)]}
    h_bytes = rs_compile_schema(decl_bytes)
    t_bytes = measure(lambda: h_bytes.build_from_py(obj_bytes),
                     iterations=max(3000, 20000 // n))

    print(f"  n={n:>3}:  int_array={us(t_int):>10}  bytes_array={us(t_bytes):>10}  "
          f"diff={us(t_bytes - t_int):>10}")


# =========================================================================
# 4. Build input type effect (dict vs Container)
# =========================================================================

print()
print("=" * 72)
print("  4. Build input type effect (plain dict vs Container)")
print("=" * 72)

import construct_rust.lib.containers as containers

h_simple = rs_compile_schema(RsStruct("a" / RsInt32ub, "b" / RsInt32ub, "c" / RsInt32ub))

# Plain dict
obj_dict = {"a": 1, "b": 2, "c": 3}
t_dict = measure(lambda: h_simple.build_from_py(obj_dict))
print(f"  plain dict input:    {us(t_dict)}")

# Container (dict subclass)
obj_container = containers.Container(a=1, b=2, c=3)
t_container = measure(lambda: h_simple.build_from_py(obj_container))
print(f"  Container input:     {us(t_container)}")
print(f"  overhead:            {us(t_container - t_dict)}")
