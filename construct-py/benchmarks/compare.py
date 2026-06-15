#!/usr/bin/env python3
"""Performance comparison: Python construct vs construct_rust (PyO3).

Both implementations are invoked from Python so the measurement reflects
end-to-end cost as seen by a Python user (including PyO3 FFI overhead on the
Rust side).  Workloads mirror ``construct-rs/benches/python_benchmark.py``.

Usage
-----
Run from the ``construct-py`` directory::

    python benchmarks/compare.py --number 20000

The script prints a Markdown table to stdout.  Use ``--json`` to additionally
emit machine-readable JSON.

Requirements
------------
* The original Python ``construct`` package, available at ``../../construct``
  (the read-only upstream checkout shipped with this repository).
* ``construct_rust`` installed via ``maturin develop``.
* ``arrow`` Python package (only because importing the upstream
  ``construct/gallery/__init__.py`` triggers a PE/COFF import that needs it).
"""

from __future__ import annotations

import argparse
import json
import os
import statistics
import sys
import timeit
from typing import Callable, Dict, List, Tuple

# Force UTF-8 on stdout/stderr so the Markdown table (and any non-ASCII) is
# emitted correctly on Windows GBK consoles.
for _stream in (sys.stdout, sys.stderr):
    if hasattr(_stream, "reconfigure"):
        try:
            _stream.reconfigure(encoding="utf-8")
        except (ValueError, OSError):
            pass

# ---------------------------------------------------------------------------
# Import the ORIGINAL Python construct first.
#
# We insert the upstream checkout at the front of sys.path so that
# ``import construct`` resolves to it (not to a site-packages copy).
# ---------------------------------------------------------------------------
_HERE = os.path.dirname(os.path.abspath(__file__))
# _CONSTRUCT_PY_DIR = construct-py/  (contains the construct_rust package)
_CONSTRUCT_PY_DIR = os.path.abspath(os.path.join(_HERE, ".."))
_ROOT = os.path.abspath(os.path.join(_HERE, "..", ".."))
_PY_CONSTRUCT_DIR = os.path.join(_ROOT, "construct")

if not os.path.isdir(_PY_CONSTRUCT_DIR):
    sys.stderr.write(
        f"ERROR: original construct checkout not found at {_PY_CONSTRUCT_DIR}\n"
    )
    sys.exit(1)

# Make the local construct_rust package importable even if ``maturin develop``
# has not installed it into site-packages (the prebuilt .pyd lives under
# ``construct-py/construct_rust/``).
if _CONSTRUCT_PY_DIR not in sys.path:
    sys.path.append(_CONSTRUCT_PY_DIR)

# Original Python construct must take precedence so it shadows any
# site-packages copy (and the construct_rust injection done by conftest.py).
sys.path.insert(0, _PY_CONSTRUCT_DIR)

import construct as py_construct  # noqa: E402  (original Python lib)
from construct import (  # noqa: E402
    Array as PyArray,
    Bytes as PyBytes,
    Const as PyConst,
    Byte as PyByte,
    Enum as PyEnum,
    Int32ub as PyInt32ub,
    Padding as PyPadding,
    Struct as PyStruct,
    VarInt as PyVarInt,
)
# The upstream gallery lives as a sibling of the inner ``construct`` package.
from gallery.elf import identifier as py_elf_identifier  # noqa: E402
from gallery.ut_index import UTIndex as PyUTIndexClass  # noqa: E402

# ---------------------------------------------------------------------------
# Import construct_rust (Rust-backed, PyO3 extension).
# ---------------------------------------------------------------------------
import construct_rust as rs_construct  # noqa: E402
from construct_rust import (  # noqa: E402
    Array as RsArray,
    Bytes as RsBytes,
    Const as RsConst,
    Byte as RsByte,
    Enum as RsEnum,
    Int32ub as RsInt32ub,
    Padding as RsPadding,
    Struct as RsStruct,
    VarInt as RsVarInt,
)
from construct_rust.gallery import UTIndex as RsUTIndexFactory  # noqa: E402

# PyO3 does not expose ``gallery::elf::identifier`` directly, so rebuild the
# equivalent identifier Struct using construct_rust primitives.  This is the
# exact same field layout the upstream Python ``identifier`` Struct uses.
rs_elf_identifier = RsStruct(
    "signature" / RsConst(b"\x7fELF"),
    "elfclass" / RsEnum(RsByte, ELFCLASSNONE=0, ELFCLASS32=1, ELFCLASS64=2),
    "encoding" / RsEnum(RsByte, ELFDATANONE=0, LSB=1, MSB=2),
    "version" / RsEnum(RsByte, EV_NONE=0, EV_CURRENT=1),
    "osabi"
    / RsEnum(
        RsByte,
        ELFOSABI_NONE=0,
        ELFOSABI_HPUX=1,
        ELFOSABI_NETBSD=2,
        ELFOSABI_GNU=3,
        ELFOSABI_SOLARIS=6,
        ELFOSABI_AIX=7,
        ELFOSABI_IRIX=8,
        ELFOSABI_FREEBSD=9,
        ELFOSABI_TRU64=10,
        ELFOSABI_MODESTO=11,
        ELFOSABI_OPENBSD=12,
        ELFOSABI_OPENVMS=13,
        ELFOSABI_NSK=14,
        ELFOSABI_AROS=15,
        ELFOSABI_FENIXOS=16,
        ELFOSABI_CLOUDABI=17,
        ELFOSABI_OPENVOS=18,
    ),
    "abiversion" / RsByte,
    RsPadding(7),
)


# ---------------------------------------------------------------------------
# Measurement helper.
# ---------------------------------------------------------------------------
def measure(fn: Callable[[], object], number: int = 20000, repeat: int = 5) -> Dict[str, float]:
    """Return ``{"median_us", "mean_us", "ops"}`` for *fn*."""
    times = timeit.repeat(fn, number=number, repeat=repeat)
    per_call = [t / number for t in times]
    return {
        "median_us": round(statistics.median(per_call) * 1e6, 4),
        "mean_us": round(statistics.mean(per_call) * 1e6, 4),
        "ops": round(number / min(times), 0),
    }


# ---------------------------------------------------------------------------
# Workload definitions.
#
# Each workload pre-builds the construct instances and payload data *outside*
# the timed region, then returns two zero-argument callables (py_fn, rs_fn)
# that perform the actual parse/build.
# ---------------------------------------------------------------------------
def make_workloads() -> List[Tuple[str, Callable[[], object], Callable[[], object]]]:
    # Bytes(100)
    py_bytes_100 = PyBytes(100)
    rs_bytes_100 = RsBytes(100)
    bytes_data = b"\x42" * 100

    # Int32ub
    int32_data = b"\x00\x00\x00\x2a"

    # VarInt
    varint_data = bytes([0xAC, 0x02])

    # 10-field Struct of Bytes(1)
    py_struct = PyStruct(*[f"f{i}" / PyBytes(1) for i in range(10)])
    rs_struct = RsStruct(*[f"f{i}" / RsBytes(1) for i in range(10)])
    struct_data = bytes(range(10))
    struct_obj = {f"f{i}": bytes([i]) for i in range(10)}

    # Array(100, Bytes(1))
    py_array = PyArray(100, PyBytes(1))
    rs_array = RsArray(100, RsBytes(1))
    array_data = bytes(range(100))
    array_obj = [bytes([i]) for i in range(100)]

    # Enum(Byte, a/b/c).  Uses Byte (= Int8ub) rather than Bytes(1) so the
    # decoded value is an int (matching the int keys), which both
    # implementations handle identically.
    py_enum = PyEnum(PyByte, a=0, b=1, c=2)
    rs_enum = RsEnum(RsByte, a=0, b=1, c=2)
    enum_data = b"\x01"

    # UTIndex (Python class needs instantiation; Rust factory too)
    py_utindex = PyUTIndexClass()
    rs_utindex = RsUTIndexFactory()
    # Use a single canonical byte payload for parse on both sides.
    utindex_data = py_utindex.build(1234567)

    # ELF identifier (16-byte minimal header)
    elf_id_data = b"\x7fELF\x01\x01\x01\x00\x00" + b"\x00" * 7

    return [
        (
            "bytes_parse_100",
            lambda: py_bytes_100.parse(bytes_data),
            lambda: rs_bytes_100.parse(bytes_data),
        ),
        (
            "bytes_build_100",
            lambda: py_bytes_100.build(bytes_data),
            lambda: rs_bytes_100.build(bytes_data),
        ),
        (
            "int32ub_parse",
            lambda: PyInt32ub.parse(int32_data),
            lambda: RsInt32ub.parse(int32_data),
        ),
        (
            "int32ub_build",
            lambda: PyInt32ub.build(42),
            lambda: RsInt32ub.build(42),
        ),
        (
            "varint_parse",
            lambda: PyVarInt.parse(varint_data),
            lambda: RsVarInt.parse(varint_data),
        ),
        (
            "varint_build",
            lambda: PyVarInt.build(300),
            lambda: RsVarInt.build(300),
        ),
        (
            "struct_parse_10fields",
            lambda: py_struct.parse(struct_data),
            lambda: rs_struct.parse(struct_data),
        ),
        (
            "struct_build_10fields",
            lambda: py_struct.build(struct_obj),
            lambda: rs_struct.build(struct_obj),
        ),
        (
            "array_parse_100",
            lambda: py_array.parse(array_data),
            lambda: rs_array.parse(array_data),
        ),
        (
            "array_build_100",
            lambda: py_array.build(array_obj),
            lambda: rs_array.build(array_obj),
        ),
        (
            "enum_parse",
            lambda: py_enum.parse(enum_data),
            lambda: rs_enum.parse(enum_data),
        ),
        (
            "enum_build",
            lambda: py_enum.build("b"),
            lambda: rs_enum.build("b"),
        ),
        (
            "utindex_parse",
            lambda: py_utindex.parse(utindex_data),
            lambda: rs_utindex.parse(utindex_data),
        ),
        (
            "utindex_build",
            lambda: py_utindex.build(1234567),
            lambda: rs_utindex.build(1234567),
        ),
        (
            "elf_identifier_parse",
            lambda: py_elf_identifier.parse(elf_id_data),
            lambda: rs_elf_identifier.parse(elf_id_data),
        ),
    ]


# ---------------------------------------------------------------------------
# Reporting.
# ---------------------------------------------------------------------------
def print_markdown_table(results: List[Dict]) -> None:
    """Print a Markdown comparison table to stdout."""
    print()
    print("## 性能对比结果")
    print()
    print("| 工作负载 | Python 原版 (μs) | construct_rust (μs) | 加速比 |")
    print("|----------|-----------------|---------------------|--------|")
    for r in results:
        py_us = r["python"]["median_us"]
        rs_us = r["rust"]["median_us"]
        speedup = r["speedup"]
        marker = "(faster)" if speedup >= 1.0 else "(slower)"
        print(
            f"| {r['name']} | {py_us:.4f} | {rs_us:.4f} | "
            f"{speedup:.2f}x {marker} |"
        )

    py_total = sum(r["python"]["median_us"] for r in results)
    rs_total = sum(r["rust"]["median_us"] for r in results)
    overall = py_total / rs_total if rs_total > 0 else float("inf")
    print(f"| **总和(中位数累加)** | {py_total:.4f} | {rs_total:.4f} | **{overall:.2f}x** |")
    print()

    # Geomean speedup (more representative than arithmetic mean for ratios).
    import math

    log_sum = sum(math.log(r["speedup"]) for r in results if r["speedup"] > 0)
    geomean = math.exp(log_sum / len(results)) if results else 0.0
    print(f"- **几何平均加速比**: {geomean:.3f}x")
    print(f"- **算术平均加速比**: "
          f"{sum(r['speedup'] for r in results) / len(results):.3f}x")

    faster = sum(1 for r in results if r["speedup"] >= 1.0)
    slower = len(results) - faster
    print(f"- 加速的工作负载: {faster}/{len(results)}")
    print(f"- 减速的工作负载: {slower}/{len(results)}")
    print()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "-n", "--number", type=int, default=20000,
        help="iterations per measurement (default: 20000)",
    )
    parser.add_argument(
        "-r", "--repeat", type=int, default=5,
        help="number of measurement repeats (default: 5)",
    )
    parser.add_argument(
        "--json", action="store_true",
        help="also emit machine-readable JSON to stdout",
    )
    parser.add_argument(
        "--warmup", type=int, default=1000,
        help="warmup iterations before timing (default: 1000)",
    )
    args = parser.parse_args()

    workloads = make_workloads()
    sys.stderr.write(
        f"Running {len(workloads)} workloads x 2 implementations "
        f"(number={args.number}, repeat={args.repeat})\n"
    )

    results: List[Dict] = []
    for name, py_fn, rs_fn in workloads:
        sys.stderr.write(f"  {name:<25s} ... ")
        sys.stderr.flush()
        # Warmup both sides so JIT/cache effects are stable.
        if args.warmup > 0:
            for _ in range(args.warmup):
                py_fn()
                rs_fn()
        py_res = measure(py_fn, number=args.number, repeat=args.repeat)
        rs_res = measure(rs_fn, number=args.number, repeat=args.repeat)
        speedup = (
            py_res["median_us"] / rs_res["median_us"]
            if rs_res["median_us"] > 0
            else float("inf")
        )
        results.append({
            "name": name,
            "python": py_res,
            "rust": rs_res,
            "speedup": round(speedup, 3),
        })
        sys.stderr.write(
            f"py={py_res['median_us']:.3f}us  "
            f"rs={rs_res['median_us']:.3f}us  "
            f"({speedup:.2f}x)\n"
        )

    print_markdown_table(results)
    if args.json:
        sys.stdout.write("\n```json\n")
        json.dump(results, sys.stdout, indent=2)
        sys.stdout.write("\n```\n")


if __name__ == "__main__":
    main()
