#!/usr/bin/env python3
"""Phase 16 build-direction profiling harness.

Runs cProfile against compiled.build_from_py for the three benchmark formats
(simple / nested / array_heavy), plus a baseline measurement vs the Python
original. cProfile only sees Python-level call frames; the bulk of the work
happens inside the Rust C extension, so the cProfile output is mainly useful
for spotting unexpected Python-level callbacks and for measuring the FFI
entry-point call counts.
"""

from __future__ import annotations

import cProfile
import gc
import os
import pstats
import sys
import time
from io import StringIO

# UTF-8 console output on Windows.
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

try:
    from construct import (  # noqa: E402
        Array as PyArray,
        Bytes as PyBytes,
        Int16ub as PyInt16ub,
        Int32ub as PyInt32ub,
        Struct as PyStruct,
    )
    _PY_OK = True
except Exception as exc:  # noqa: BLE001
    print(f"Python construct unavailable: {exc}")
    _PY_OK = False


# ---------- format definitions ----------

def make_simple():
    rs_decl = RsStruct("magic" / RsBytes(4), "a" / RsInt16ub, "b" / RsInt32ub)
    holder = rs_compile_schema(rs_decl)
    build_obj = {"magic": b"ABCD", "a": 1, "b": 2}
    if _PY_OK:
        py_decl = PyStruct("magic" / PyBytes(4), "a" / PyInt16ub, "b" / PyInt32ub)
    else:
        py_decl = None
    return holder, build_obj, py_decl


def make_nested():
    rs_decl = RsStruct(
        "header" / RsStruct("magic" / RsBytes(4), "version" / RsInt16ub),
        "payload" / RsStruct("size" / RsInt32ub, "flags" / RsInt32ub),
    )
    holder = rs_compile_schema(rs_decl)
    build_obj = {
        "header": {"magic": b"ABCD", "version": 1},
        "payload": {"size": 2, "flags": 3},
    }
    if _PY_OK:
        py_decl = PyStruct(
            "header" / PyStruct("magic" / PyBytes(4), "version" / PyInt16ub),
            "payload" / PyStruct("size" / PyInt32ub, "flags" / PyInt32ub),
        )
    else:
        py_decl = None
    return holder, build_obj, py_decl


def make_array_heavy():
    items = list(range(100))
    rs_decl = RsStruct("count" / RsInt32ub, "items" / RsArray(100, RsInt32ub))
    holder = rs_compile_schema(rs_decl)
    build_obj = {"count": 100, "items": items}
    if _PY_OK:
        py_decl = PyStruct("count" / PyInt32ub, "items" / PyArray(100, PyInt32ub))
    else:
        py_decl = None
    return holder, build_obj, py_decl


FORMATS = {
    "simple": make_simple,
    "nested": make_nested,
    "array_heavy": make_array_heavy,
}


# ---------- timing harness (no profiler, for clean numbers) ----------

def measure(fn, iterations):
    fn()
    gc.collect()
    gc_was = gc.isenabled()
    gc.disable()
    try:
        t0 = time.perf_counter()
        for _ in range(iterations):
            fn()
        return time.perf_counter() - t0
    finally:
        if gc_was:
            gc.enable()


def run_baseline(iterations):
    print("=" * 72)
    print(f"  Baseline timing ({iterations} iterations, no profiler)")
    print("=" * 72)
    for name, factory in FORMATS.items():
        holder, build_obj, py_decl = factory()

        def _new():
            holder.build_from_py(build_obj)

        new_t = measure(_new, iterations)

        if _PY_OK:
            def _py():
                py_decl.build(build_obj)
            py_t = measure(_py, iterations)
            ratio = py_t / new_t
            print(f"  [{name}] new={new_t*1e6/iterations:.3f}µs "
                  f"py={py_t*1e6/iterations:.3f}µs  ratio={ratio:.2f}x")
        else:
            print(f"  [{name}] new={new_t*1e6/iterations:.3f}µs")


# ---------- cProfile harness ----------

def run_cprofile(name, factory, iterations):
    holder, build_obj, _ = factory()

    def _run():
        for _ in range(iterations):
            holder.build_from_py(build_obj)

    # Warm up.
    holder.build_from_py(build_obj)

    prof = cProfile.Profile()
    prof.enable()
    _run()
    prof.disable()

    s = StringIO()
    ps = pstats.Stats(prof, stream=s).sort_stats("cumulative")
    ps.print_stats(30)
    print(f"\n===== cProfile: {name} build_from_py ({iterations} iters) =====")
    print(s.getvalue())


def main():
    iterations_baseline = 5000
    iterations_profile = 2000

    run_baseline(iterations_baseline)

    for name, factory in FORMATS.items():
        run_cprofile(name, factory, iterations_profile)


if __name__ == "__main__":
    main()
