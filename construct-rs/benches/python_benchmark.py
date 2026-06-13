#!/usr/bin/env python3
"""Python baseline benchmark for construct-rs performance comparison.

This script measures the parse/build performance of the Python `construct`
library on the same workloads used by the Rust benchmarks
(``benches/core_constructs.rs`` and ``benches/gallery_formats.rs``).

Usage
-----
::

    cd construct                # the Python construct checkout
    ../construct-rs/benches/python_benchmark.py
    # or with a specific interpreter:
    python ../construct-rs/benches/python_benchmark.py --number 50000

The script prints a JSON document on stdout that can be diffed against the
Rust criterion results.  Example output::

    {
      "bytes_parse_100": {"mean_us": 0.42, "ops": 2380952},
      "struct_parse_10fields": {"mean_us": 1.87, "ops": 534759},
      ...
    }

Requirements: the ``construct`` package must be importable (it is, when run
from inside the ``construct/`` repository root).
"""

from __future__ import annotations

import argparse
import json
import statistics
import sys
import timeit
from typing import Callable, Dict

# --------------------------------------------------------------------------- 
# Import the Python construct library.
# --------------------------------------------------------------------------- 
try:
    from construct import (
        Bytes,
        Int32ub,
        VarInt,
        Struct,
        Array,
        Enum as CEnum,
    )
    from construct.gallery.elf import identifier as elf_identifier
    from construct.gallery.ut_index import UTIndex
except ImportError as exc:  # pragma: no cover - environment guard
    sys.stderr.write(
        "ERROR: could not import 'construct'.  Run this script from inside "
        "the construct/ repository root (where the .venv lives).\n"
        f"  {exc}\n"
    )
    sys.exit(1)


# ---------------------------------------------------------------------------
# Helper: run a callable repeatedly and report timing.
# ---------------------------------------------------------------------------

def measure(fn: Callable[[], object], number: int, repeat: int = 5) -> Dict[str, float]:
    """Return ``{"mean_us": ..., "median_us": ..., "ops": ...}`` for *fn*."""
    times = timeit.repeat(fn, number=number, repeat=repeat)
    per_call = [t / number for t in times]  # seconds per single call
    mean_us = statistics.mean(per_call) * 1e6
    median_us = statistics.median(per_call) * 1e6
    ops = number / min(times)  # best-case ops/sec
    return {
        "mean_us": round(mean_us, 4),
        "median_us": round(median_us, 4),
        "ops": round(ops, 1),
    }


# ---------------------------------------------------------------------------
# Workloads — mirror the Rust core_constructs / gallery_formats benches.
# ---------------------------------------------------------------------------

def workload_bytes_parse(number: int) -> Dict[str, float]:
    data = b"\x42" * 100
    c = Bytes(100)
    return measure(lambda: c.parse(data), number)


def workload_bytes_build(number: int) -> Dict[str, float]:
    c = Bytes(100)
    return measure(lambda: c.build(dict(data=b"\x42" * 100)), number)


def workload_int32ub_parse(number: int) -> Dict[str, float]:
    data = b"\x00\x00\x00\x2a"
    c = Int32ub
    return measure(lambda: c.parse(data), number)


def workload_int32ub_build(number: int) -> Dict[str, float]:
    c = Int32ub
    return measure(lambda: c.build(42), number)


def workload_varint_parse(number: int) -> Dict[str, float]:
    data = bytes([0xAC, 0x02])
    c = VarInt
    return measure(lambda: c.parse(data), number)


def workload_varint_build(number: int) -> Dict[str, float]:
    c = VarInt
    return measure(lambda: c.build(300), number)


def workload_struct_parse(number: int) -> Dict[str, float]:
    fields = [(f"f{i}", Int32ub if i == 0 else Bytes(1)) for i in range(10)]
    # Build a simple 10-field struct of single bytes.
    c = Struct(*[(f"f{i}", Bytes(1)) for i in range(10)])
    data = bytes(range(10))
    return measure(lambda: c.parse(data), number)


def workload_struct_build(number: int) -> Dict[str, float]:
    c = Struct(*[(f"f{i}", Bytes(1)) for i in range(10)])
    obj = {f"f{i}": bytes([i]) for i in range(10)}
    return measure(lambda: c.build(obj), number)


def workload_array_parse(number: int) -> Dict[str, float]:
    c = Array(100, Bytes(1))
    data = bytes(range(100))
    return measure(lambda: c.parse(data), number)


def workload_array_build(number: int) -> Dict[str, float]:
    c = Array(100, Bytes(1))
    obj = [bytes([i]) for i in range(100)]
    return measure(lambda: c.build(obj), number)


def workload_enum_parse(number: int) -> Dict[str, float]:
    c = CEnum(Bytes(1), a=0, b=1, c=2)
    return measure(lambda: c.parse(b"\x01"), number)


def workload_enum_build(number: int) -> Dict[str, float]:
    c = CEnum(Bytes(1), a=0, b=1, c=2)
    return measure(lambda: c.build("b"), number)


def workload_utindex_parse(number: int) -> Dict[str, float]:
    c = UTIndex
    data = c.build(1234567)
    return measure(lambda: c.parse(data), number)


def workload_utindex_build(number: int) -> Dict[str, float]:
    c = UTIndex
    return measure(lambda: c.build(1234567), number)


def workload_elf_identifier_parse(number: int) -> Dict[str, float]:
    # Minimal 16-byte ELF identifier.
    data = b"\x7fELF\x01\x01\x01\x00\x00" + b"\x00" * 7
    c = elf_identifier
    return measure(lambda: c.parse(data), number)


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

WORKLOADS = [
    ("bytes_parse_100", workload_bytes_parse),
    ("bytes_build_100", workload_bytes_build),
    ("int32ub_parse", workload_int32ub_parse),
    ("int32ub_build", workload_int32ub_build),
    ("varint_parse", workload_varint_parse),
    ("varint_build", workload_varint_build),
    ("struct_parse_10fields", workload_struct_parse),
    ("struct_build_10fields", workload_struct_build),
    ("array_parse_100", workload_array_parse),
    ("array_build_100", workload_array_build),
    ("enum_parse", workload_enum_parse),
    ("enum_build", workload_enum_build),
    ("utindex_parse", workload_utindex_parse),
    ("utindex_build", workload_utindex_build),
    ("elf_identifier_parse", workload_elf_identifier_parse),
]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "-n",
        "--number",
        type=int,
        default=20000,
        help="iterations per measurement (default: 20000)",
    )
    parser.add_argument(
        "--indent", type=int, default=2, help="JSON indent (default: 2)"
    )
    args = parser.parse_args()

    results: Dict[str, Dict[str, float]] = {}
    for name, fn in WORKLOADS:
        sys.stderr.write(f"  running {name} ... ")
        sys.stderr.flush()
        results[name] = fn(args.number)
        sys.stderr.write(
            f"{results[name]['mean_us']} us/call ({results[name]['ops']:.0f} ops/s)\n"
        )

    json.dump(results, sys.stdout, indent=args.indent)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
