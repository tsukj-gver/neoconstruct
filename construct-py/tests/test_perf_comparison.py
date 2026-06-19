#!/usr/bin/env python3
"""User-level performance comparison across three parse/build paths.

This benchmark measures **end-to-end** throughput as seen by a Python user
(including all PyO3 FFI overhead). It compares three paths:

* **Path A — ``python_original``**: the upstream Python ``construct`` package.
  This is the reference baseline.
* **Path B — ``old_ffi``**: the construct-py declarative API
  (``Struct(...).parse(data)`` / ``.build(obj)``), which walks the declaration
  tree through ``py_parse`` / ``py_build`` — every operation incurs a
  ``Value`` <-> ``PyObject`` round-trip (the per-operation FFI path).
* **Path C — ``new_compiled``**: the Phase 12-15 direct-to-Python path.
  ``compile_schema(fmt)`` pre-compiles the declaration into a
  ``CompiledSchemaHolder``; ``holder.parse_bytes_py(data)`` /
  ``holder.build_from_py(obj)`` traverse the compiled tree while holding the
  GIL and write results straight into a ``PyDict`` / read input lazily — no
  ``Value::Container`` intermediate tree is built.

Three formats of increasing complexity are measured:

* ``simple``      — 3-field flat Struct, ~10 bytes.
* ``nested``      — 2-level nested Struct, ~14 bytes.
* ``array_heavy`` — 100-element ``Array`` of ``Int32ub``, ~404 bytes.

Usage
-----
Run from anywhere (the script resolves its own paths)::

    python construct-py/tests/test_perf_comparison.py
    python construct-py/tests/test_perf_comparison.py --iterations 2000

Requirements
------------
* ``construct_rust`` installed via ``maturin develop`` (into the active env).
* The upstream Python ``construct`` package (optional — if unavailable, only
  paths B and C are compared and path A is reported as unavailable).
"""

from __future__ import annotations

import argparse
import gc
import os
import sys
import time
from typing import Any, Callable, Dict, List, Optional, Tuple

# Force UTF-8 on stdout/stderr so the table (and any non-ASCII) is emitted
# correctly on Windows GBK consoles.
for _stream in (sys.stdout, sys.stderr):
    if hasattr(_stream, "reconfigure"):
        try:
            _stream.reconfigure(encoding="utf-8")
        except (ValueError, OSError):
            pass

# ---------------------------------------------------------------------------
# Path setup: make the local construct_rust package importable even if
# ``maturin develop`` has not installed it into site-packages (the prebuilt
# .pyd lives under ``construct-py/construct_rust/``).
# ---------------------------------------------------------------------------
_HERE = os.path.dirname(os.path.abspath(__file__))
_CONSTRUCT_PY_DIR = os.path.abspath(os.path.join(_HERE, ".."))
if _CONSTRUCT_PY_DIR not in sys.path:
    sys.path.append(_CONSTRUCT_PY_DIR)


# ---------------------------------------------------------------------------
# Import the three paths. Each import is guarded so a missing dependency is
# reported per-path rather than aborting the whole benchmark.
# ---------------------------------------------------------------------------
PathCall = Callable[[], Any]
PathResult = Optional[Dict[str, float]]  # {"parse": float, "build": float} or None


def _try_import_python_original() -> Tuple[bool, str]:
    """Tries to import the upstream Python ``construct`` package.

    Returns ``(ok, label)`` where ``label`` describes the outcome.
    """
    try:
        import construct  # noqa: F401
        return True, "ok"
    except Exception as exc:  # pragma: no cover - environment-dependent
        return False, f"{type(exc).__name__}: {exc}"


def _try_import_construct_rust() -> Tuple[bool, str]:
    """Tries to import the ``construct_rust`` native extension."""
    try:
        import construct_rust  # noqa: F401
        return True, "ok"
    except Exception as exc:  # pragma: no cover - environment-dependent
        return False, f"{type(exc).__name__}: {exc}"


# Resolve availability up front so we can report it in the header.
_PY_ORIG_OK, _PY_ORIG_MSG = _try_import_python_original()
_RS_OK, _RS_MSG = _try_import_construct_rust()

if not _RS_OK:
    sys.stderr.write(
        "FATAL: construct_rust is not importable — cannot run benchmark.\n"
        f"  Reason: {_RS_MSG}\n"
        "  Install it with: maturin develop --manifest-path "
        "construct-py/Cargo.toml --release\n"
    )
    sys.exit(1)

# Import the construct_rust symbols we need.
from construct_rust import (  # noqa: E402
    Array as RsArray,
    Bytes as RsBytes,
    Int16ub as RsInt16ub,
    Int32ub as RsInt32ub,
    Struct as RsStruct,
    compile_schema as rs_compile_schema,
)

# Import Python original symbols if available.
if _PY_ORIG_OK:
    from construct import (  # noqa: E402
        Array as PyArray,
        Bytes as PyBytes,
        Int16ub as PyInt16ub,
        Int32ub as PyInt32ub,
        Struct as PyStruct,
    )


# ===========================================================================
# Format definitions
# ===========================================================================
#
# Each format is defined by a factory function returning the *declaration*
# object for each path. The positional ``/`` Renamed syntax is identical
# between the Python original and construct_rust, so the three definitions
# are structurally identical — ensuring a fair comparison.


def make_simple():
    """``simple`` — 3-field flat Struct.

    ``Struct("magic" / Bytes(4), "a" / Int16ub, "b" / Int32ub)`` → 10 bytes.

    Returns ``(declarations, data, build_obj)`` where ``declarations`` is a
    dict mapping ``path_name -> (decl, compiled_holder_or_None)``.
    """
    if _PY_ORIG_OK:
        py_decl = PyStruct("magic" / PyBytes(4), "a" / PyInt16ub, "b" / PyInt32ub)
    else:
        py_decl = None
    rs_decl = RsStruct("magic" / RsBytes(4), "a" / RsInt16ub, "b" / RsInt32ub)
    holder = rs_compile_schema(rs_decl)

    data = b"ABCD" + (1).to_bytes(2, "big") + (2).to_bytes(4, "big")
    build_obj = {"magic": b"ABCD", "a": 1, "b": 2}
    return {
        "python_original": (py_decl, None),
        "old_ffi": (rs_decl, None),
        "new_compiled": (rs_decl, holder),
    }, data, build_obj


def make_nested():
    """``nested`` — 2-level nested Struct.

    ``Struct("header" / Struct("magic"/Bytes(4), "version"/Int16ub),
              "payload" / Struct("size"/Int32ub, "flags"/Int32ub))`` → 14 bytes.
    """
    if _PY_ORIG_OK:
        py_decl = PyStruct(
            "header" / PyStruct("magic" / PyBytes(4), "version" / PyInt16ub),
            "payload" / PyStruct("size" / PyInt32ub, "flags" / PyInt32ub),
        )
    else:
        py_decl = None
    rs_decl = RsStruct(
        "header" / RsStruct("magic" / RsBytes(4), "version" / RsInt16ub),
        "payload" / RsStruct("size" / RsInt32ub, "flags" / RsInt32ub),
    )
    holder = rs_compile_schema(rs_decl)

    data = (
        b"ABCD" + (1).to_bytes(2, "big")
        + (2).to_bytes(4, "big")
        + (3).to_bytes(4, "big")
    )
    build_obj = {
        "header": {"magic": b"ABCD", "version": 1},
        "payload": {"size": 2, "flags": 3},
    }
    return {
        "python_original": (py_decl, None),
        "old_ffi": (rs_decl, None),
        "new_compiled": (rs_decl, holder),
    }, data, build_obj


def make_array_heavy():
    """``array_heavy`` — 100-element Array of Int32ub.

    ``Struct("count" / Int32ub, "items" / Array(100, Int32ub))`` → 404 bytes.
    """
    items = list(range(100))
    if _PY_ORIG_OK:
        py_decl = PyStruct("count" / PyInt32ub, "items" / PyArray(100, PyInt32ub))
    else:
        py_decl = None
    rs_decl = RsStruct("count" / RsInt32ub, "items" / RsArray(100, RsInt32ub))
    holder = rs_compile_schema(rs_decl)

    data = (100).to_bytes(4, "big") + b"".join(i.to_bytes(4, "big") for i in items)
    build_obj = {"count": 100, "items": items}
    return {
        "python_original": (py_decl, None),
        "old_ffi": (rs_decl, None),
        "new_compiled": (rs_decl, holder),
    }, data, build_obj


FORMATS: List[Tuple[str, Callable[[], Tuple[dict, bytes, dict]]]] = [
    ("simple", make_simple),
    ("nested", make_nested),
    ("array_heavy", make_array_heavy),
]


# ===========================================================================
# Path executors
# ===========================================================================


def exec_parse(path_name: str, decl: Any, holder: Any, data: bytes) -> Any:
    """Runs one parse call for the given path. Raises on error."""
    if path_name == "python_original":
        return decl.parse(data)
    if path_name == "old_ffi":
        return decl.parse(data)
    if path_name == "new_compiled":
        return holder.parse_bytes_py(data)
    raise ValueError(f"unknown path: {path_name}")


def exec_build(path_name: str, decl: Any, holder: Any, obj: dict) -> Any:
    """Runs one build call for the given path. Raises on error."""
    if path_name == "python_original":
        return decl.build(obj)
    if path_name == "old_ffi":
        return decl.build(obj)
    if path_name == "new_compiled":
        return holder.build_from_py(obj)
    raise ValueError(f"unknown path: {path_name}")


def measure(
    fn: Callable[[], Any],
    iterations: int,
) -> Tuple[Optional[float], Optional[str]]:
    """Times ``fn`` over ``iterations`` calls.

    Returns ``(elapsed_seconds, error_message)``. Exactly one element is
    non-None: on success ``error_message`` is ``None``; on failure
    ``elapsed_seconds`` is ``None`` and ``error_message`` describes the
    exception.

    The garbage collector is disabled during the timed region (matching
    ``timeit`` semantics) so periodic GC pauses do not skew the result.
    """
    try:
        # Warm-up: one call to trigger any lazy initialisation / caching.
        fn()
        gc.collect()
        gc_was_enabled = gc.isenabled()
        gc.disable()
        try:
            start = time.perf_counter()
            for _ in range(iterations):
                fn()
            elapsed = time.perf_counter() - start
        finally:
            if gc_was_enabled:
                gc.enable()
        return elapsed, None
    except Exception as exc:  # noqa: BLE001 — report, do not crash
        return None, f"{type(exc).__name__}: {exc}"


# ===========================================================================
# Reporting
# ===========================================================================

# Column widths for the table.
_W_PATH = 20
_W_NUM = 11
_W_RATIO = 15


def _fmt_secs(val: Optional[float]) -> str:
    if val is None:
        return "ERROR".rjust(_W_NUM)
    return f"{val:.4f}".rjust(_W_NUM)


def _fmt_ratio(val: Optional[float], is_reference: bool = False) -> str:
    if is_reference:
        return "(reference)".rjust(_W_RATIO)
    if val is None:
        return "n/a".rjust(_W_RATIO)
    return f"{val:.2f}x".rjust(_W_RATIO)


def _print_table_header() -> None:
    print()
    # Column widths mirror the data rows: label(20) secs(11) secs(11) ratio(15) ratio(15).
    h = (
        f"  {'Path':<{_W_PATH}}"
        f"{'parse (s)':>{_W_NUM}}"
        f"{'build (s)':>{_W_NUM}}"
        f"{'parse ratio':>{_W_RATIO}}"
        f"{'build ratio':>{_W_RATIO}}"
    )
    print(h)
    print("  " + "-" * 70)


def _print_row(label: str, parse_s: Optional[float], build_s: Optional[float],
               parse_ratio: Optional[float], build_ratio: Optional[float],
               is_reference: bool = False) -> None:
    print(
        f"  {label:<{_W_PATH}}{_fmt_secs(parse_s)}{_fmt_secs(build_s)}"
        f"{_fmt_ratio(parse_ratio, is_reference)}{_fmt_ratio(build_ratio, is_reference)}"
    )


def _print_errors(errors: List[str]) -> None:
    if not errors:
        return
    print("\n  Errors encountered during measurement:")
    for line in errors:
        print(f"    {line}")


def run_benchmark(iterations: int) -> int:
    """Runs the full benchmark and prints the comparison table.

    Returns a process exit code: ``0`` if at least one format produced data
    for both old_ffi and new_compiled, ``1`` otherwise.
    """
    # Header.
    print("=" * 72)
    print(f"  User-Level Performance Comparison ({iterations} iterations)")
    print("=" * 72)
    print()
    print(f"  Python interpreter : {sys.version.split()[0]} ({sys.executable})")

    if _PY_ORIG_OK:
        import construct
        print(f"  Path A (reference) : Python construct {construct.__version__}")
    else:
        print("  Path A (reference) : Python construct UNAVAILABLE "
              f"({_PY_ORIG_MSG})")

    import construct_rust
    print(f"  Paths B & C        : construct_rust {construct_rust.__version__}")
    print()
    print("  Path A = python_original  (Python construct, reference baseline)")
    print("  Path B = old_ffi          (construct_rust declaration-tree path)")
    print("  Path C = new_compiled     (construct_rust compiled direct-to-Python)")
    print()

    produced_data = False
    all_errors: List[str] = []

    for fmt_name, factory in FORMATS:
        print(f"  [{fmt_name}]")
        declarations, data, build_obj = factory()

        # Pre-build the zero-arg callables for each path × operation.
        parse_calls: Dict[str, Callable[[], Any]] = {}
        build_calls: Dict[str, Callable[[], Any]] = {}
        for path_name, (decl, holder) in declarations.items():
            if decl is None and path_name == "python_original":
                continue  # Python original unavailable
            parse_calls[path_name] = (
                lambda pn=path_name, d=decl, h=holder: exec_parse(pn, d, h, data)
            )
            build_calls[path_name] = (
                lambda pn=path_name, d=decl, h=holder: exec_build(pn, d, h, build_obj)
            )

        # Measure.
        results: Dict[str, Tuple[Optional[float], Optional[str]]] = {}
        for path_name in parse_calls:
            p_elapsed, p_err = measure(parse_calls[path_name], iterations)
            b_elapsed, b_err = measure(build_calls[path_name], iterations)
            results[path_name] = (p_elapsed, b_elapsed)
            if p_err:
                all_errors.append(f"[{fmt_name}/{path_name} parse] {p_err}")
            if b_err:
                all_errors.append(f"[{fmt_name}/{path_name} build] {b_err}")

        # Reference times (python_original) for ratio computation.
        ref = results.get("python_original", (None, None))
        ref_parse, ref_build = ref

        # Table.
        _print_table_header()

        ratios: Dict[str, Tuple[Optional[float], Optional[float]]] = {}
        for path_name in ("python_original", "old_ffi", "new_compiled"):
            if path_name not in results:
                continue
            p_elapsed, b_elapsed = results[path_name]
            if path_name == "python_original":
                p_ratio: Optional[float] = None
                b_ratio: Optional[float] = None
                _print_row(path_name, p_elapsed, b_elapsed, None, None,
                           is_reference=True)
            else:
                # ratio = reference_time / path_time (>1.0 ⇒ faster than ref)
                p_ratio = (ref_parse / p_elapsed) if (
                    ref_parse is not None and p_elapsed is not None
                ) else None
                b_ratio = (ref_build / b_elapsed) if (
                    ref_build is not None and b_elapsed is not None
                ) else None
                _print_row(path_name, p_elapsed, b_elapsed, p_ratio, b_ratio)
            ratios[path_name] = (p_ratio, b_ratio)

        print("  " + "-" * 70)

        # new vs old speedup (old_time / new_time; >1.0 ⇒ new is faster).
        old_parse, old_build = results.get("old_ffi", (None, None))
        new_parse, new_build = results.get("new_compiled", (None, None))
        if old_parse is not None and new_parse is not None and new_parse > 0:
            speedup_parse = old_parse / new_parse
        else:
            speedup_parse = None
        if old_build is not None and new_build is not None and new_build > 0:
            speedup_build = old_build / new_build
        else:
            speedup_build = None

        # "new vs old speedup" label, with the two ratio values aligned to the
        # parse_ratio / build_ratio columns (skip the two seconds columns).
        label = "new vs old speedup"
        spacer = " " * (_W_NUM * 2)  # skip parse_s + build_s columns
        print(
            f"  {label:<{_W_PATH}}{spacer}"
            f"{_fmt_ratio(speedup_parse, False)}{_fmt_ratio(speedup_build, False)}"
        )
        print()

        if new_parse is not None and old_parse is not None:
            produced_data = True

    _print_errors(all_errors)

    # Summary verdict against the S-PERF ≥1.0x standard (new path vs Python
    # original, geometric mean across formats where available).
    print()
    print("=" * 72)
    print("  Verdict (S-PERF: new_compiled ratio vs python_original ≥ 1.0x)")
    print("=" * 72)
    _summarize_verdict(iterations)

    return 0 if produced_data else 1


def _summarize_verdict(iterations: int) -> None:
    """Re-runs a lightweight measurement to summarise whether the new path
    meets the S-PERF ≥1.0x standard against the Python original.

    This recomputes ratios so the summary is self-contained. If the Python
    original is unavailable, the verdict is reported as not-assessable.
    """
    if not _PY_ORIG_OK:
        print("  Python original unavailable — cannot assess S-PERF vs reference.")
        print("  (Comparing new_compiled vs old_ffi only; see the tables above.)")
        return

    import math
    parse_log_sum = 0.0
    build_log_sum = 0.0
    count = 0
    for fmt_name, factory in FORMATS:
        declarations, data, build_obj = factory()
        py_decl = declarations["python_original"][0]
        rs_decl = declarations["new_compiled"][0]
        holder = declarations["new_compiled"][1]

        # Measure each path once more (single repeat is fine for a summary).
        def _py_parse():
            py_decl.parse(data)

        def _py_build():
            py_decl.build(build_obj)

        def _new_parse():
            holder.parse_bytes_py(data)

        def _new_build():
            holder.build_from_py(build_obj)

        py_p, _ = measure(_py_parse, iterations)
        py_b, _ = measure(_py_build, iterations)
        new_p, _ = measure(_new_parse, iterations)
        new_b, _ = measure(_new_build, iterations)

        if py_p and new_p and new_p > 0:
            parse_log_sum += math.log(py_p / new_p)
        if py_b and new_b and new_b > 0:
            build_log_sum += math.log(py_b / new_b)
        count += 1

    if count > 0:
        geomean_parse = math.exp(parse_log_sum / count)
        geomean_build = math.exp(build_log_sum / count)
        print(f"  Geomean parse ratio (new vs py): {geomean_parse:.3f}x  "
              f"{'PASS' if geomean_parse >= 1.0 else 'FAIL'} (≥1.0x)")
        print(f"  Geomean build ratio (new vs py): {geomean_build:.3f}x  "
              f"{'PASS' if geomean_build >= 1.0 else 'FAIL'} (≥1.0x)")
    else:
        print("  No formats available for summary.")


# ===========================================================================
# Entry point
# ===========================================================================


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "-n", "--iterations", type=int, default=5000,
        help="iterations per measurement (default: 5000)",
    )
    parser.add_argument(
        "--format", choices=["simple", "nested", "array_heavy"], default=None,
        help="run only the named format (default: all three)",
    )
    args = parser.parse_args()

    if args.iterations < 1:
        sys.stderr.write("--iterations must be >= 1\n")
        sys.exit(2)

    global FORMATS
    if args.format is not None:
        FORMATS = [(n, f) for (n, f) in FORMATS if n == args.format]

    code = run_benchmark(args.iterations)
    sys.exit(code)


if __name__ == "__main__":
    main()
