"""4.x error path D-class optimization initial perf verify (DEV stage).

Tests 2 error-path scenarios + 1 negative control:
  1. a_err_eof_parse - Array(100, Int8ub) + 50B stream EOF
     Target: >=10x (design 4.x section 1 exit criteria)
     INVEST baseline: 6.45x
  2. p_err_overflow_build - PrefixedArray cf=Int8ub build overflow
     Target: ~3-4x (design section 2 engineering ceiling)
     INVEST baseline: 1.97x
  3. i02_idx_n100 parse - negative control (env drift validation)

Measurement protocol (performance-gate/SKILL.md S-PERF):
  - construct-rs side: maturin develop --release, user-facing API
  - Python construct side: import construct, equivalent API
  - Subprocess isolation (same package name, cannot coexist)
  - min(repeat=5) x number

NOTE: This is DEV initial data. VET will do Controlled A/B Test (per
design section 5.2.2 + L-09 countermeasure).
"""

import sys
import subprocess
import platform
import time
import os

try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
except Exception:
    pass

CRS_PYTHON = r"<opencode-temp>\crs_venv_new\Scripts\python.exe"
PC_PYTHON = r"<opencode-temp>\crs_venv_py_new\Scripts\python.exe"

REPEAT = 5


def measure(python_exe, code_template, number):
    """Run `number` iterations, return min ns/call (perf_counter-based).

    code_template uses %NUMBER% placeholder to avoid clashing with Python
    dict literals {...} in the embedded code.
    """
    actual_code = code_template.replace("%NUMBER%", str(number))
    proc = subprocess.run(
        [python_exe, "-c", actual_code],
        capture_output=True, text=True, timeout=300,
        env={**os.environ, "PYO3_USE_ABI3_FORWARD_COMPATIBILITY": "1"},
    )
    if proc.returncode != 0:
        print(f"STDERR: {proc.stderr[:500]}", file=sys.stderr)
        raise RuntimeError(f"subprocess failed: {proc.returncode}")
    return float(proc.stdout.strip().split("\n")[-1])


A_ERR_EOF_CRS = """
import time as _time
from dataclasses import dataclass
from construct import StructMixin, field, Array, Int8ub

@dataclass
class P(StructMixin):
    items: list = field(Array(100, Int8ub))

DATA = bytes(i % 256 for i in range(50))

for _ in range(50):
    try:
        P.parse(DATA)
    except Exception:
        pass

t0 = _time.perf_counter()
for _ in range(%NUMBER%):
    try:
        P.parse(DATA)
    except Exception:
        pass
elapsed = (_time.perf_counter() - t0) / %NUMBER%
print(elapsed * 1e9)
"""

A_ERR_EOF_PC = """
import time as _time
import construct as pc

d = pc.Struct("items" / pc.Array(100, pc.Int8ub))
DATA = bytes(i % 256 for i in range(50))

for _ in range(50):
    try:
        d.parse(DATA)
    except Exception:
        pass

t0 = _time.perf_counter()
for _ in range(%NUMBER%):
    try:
        d.parse(DATA)
    except Exception:
        pass
elapsed = (_time.perf_counter() - t0) / %NUMBER%
print(elapsed * 1e9)
"""

P_ERR_OVERFLOW_CRS = """
import time as _time
from dataclasses import dataclass
from construct import StructMixin, field, PrefixedArray, Int8ub

@dataclass
class P(StructMixin):
    items: list = field(PrefixedArray(Int8ub, Int8ub))

# list of 300 items, countfield (Int8ub max 255) cannot fit, build must fail
BIG_OBJ = P(items=list(range(300)))

for _ in range(50):
    try:
        BIG_OBJ.build()
    except Exception:
        pass

t0 = _time.perf_counter()
for _ in range(%NUMBER%):
    try:
        BIG_OBJ.build()
    except Exception:
        pass
elapsed = (_time.perf_counter() - t0) / %NUMBER%
print(elapsed * 1e9)
"""

P_ERR_OVERFLOW_PC = """
import time as _time
import construct as pc

d = pc.Struct("items" / pc.PrefixedArray(pc.Int8ub, pc.Int8ub))
BIG_OBJ = {"items": list(range(300))}

for _ in range(50):
    try:
        d.build(BIG_OBJ)
    except Exception:
        pass

t0 = _time.perf_counter()
for _ in range(%NUMBER%):
    try:
        d.build(BIG_OBJ)
    except Exception:
        pass
elapsed = (_time.perf_counter() - t0) / %NUMBER%
print(elapsed * 1e9)
"""

I02_IDX_CRS = """
import time as _time
from dataclasses import dataclass
from construct import StructMixin, field, Array, Index

@dataclass
class P(StructMixin):
    items: list = field(Array(100, Index()))

DATA = bytes(100)

for _ in range(50):
    P.parse(DATA)

t0 = _time.perf_counter()
for _ in range(%NUMBER%):
    P.parse(DATA)
elapsed = (_time.perf_counter() - t0) / %NUMBER%
print(elapsed * 1e9)
"""

I02_IDX_PC = """
import time as _time
import construct as pc

d = pc.Struct("items" / pc.Array(100, pc.Index))
DATA = bytes(100)

for _ in range(50):
    d.parse(DATA)

t0 = _time.perf_counter()
for _ in range(%NUMBER%):
    d.parse(DATA)
elapsed = (_time.perf_counter() - t0) / %NUMBER%
print(elapsed * 1e9)
"""


def run_scenario(name, crs_code, pc_code, number):
    print(f"\n=== {name} (number={number}) ===", flush=True)
    crs_runs = [measure(CRS_PYTHON, crs_code, number) for _ in range(REPEAT)]
    pc_runs = [measure(PC_PYTHON, pc_code, number) for _ in range(REPEAT)]
    crs_min = min(crs_runs)
    pc_min = min(pc_runs)
    speedup = pc_min / crs_min
    print(f"  crs min: {crs_min:.1f} ns/call (runs: {[f'{r:.0f}' for r in crs_runs]})", flush=True)
    print(f"  pc  min: {pc_min:.1f} ns/call (runs: {[f'{r:.0f}' for r in pc_runs]})", flush=True)
    print(f"  speedup: {speedup:.2f}x", flush=True)
    return {"name": name, "crs_min_ns": crs_min, "pc_min_ns": pc_min, "speedup": speedup,
            "crs_runs": crs_runs, "pc_runs": pc_runs}


def main():
    print("=" * 60)
    print("4.x error-path D-class optimization - DEV initial perf verify")
    print("=" * 60)
    print(f"time: {time.strftime('%Y-%m-%d %H:%M:%S')}")
    print(f"OS: {platform.system()} {platform.release()}")
    print(f"CPU: {platform.processor()}")
    print(f"CRS python: {CRS_PYTHON}")
    print(f"PC  python: {PC_PYTHON}")
    print(f"REPEAT: {REPEAT} (5 runs per scenario, report min)")

    results = []

    results.append(run_scenario(
        "a_err_eof parse (Array(100, Int8ub) + 50B EOF)",
        A_ERR_EOF_CRS, A_ERR_EOF_PC, number=3000,
    ))

    results.append(run_scenario(
        "p_err_overflow build (PrefixedArray Int8ub cf build overflow)",
        P_ERR_OVERFLOW_CRS, P_ERR_OVERFLOW_PC, number=3000,
    ))

    results.append(run_scenario(
        "i02_idx_n100 parse (negative control)",
        I02_IDX_CRS, I02_IDX_PC, number=3000,
    ))

    print("\n" + "=" * 60)
    print("Summary")
    print("=" * 60)
    print(f"{'scenario':<60} {'crs ns':>10} {'pc ns':>10} {'speedup':>8}")
    for r in results:
        print(f"{r['name']:<60} {r['crs_min_ns']:>10.1f} {r['pc_min_ns']:>10.1f} {r['speedup']:>7.2f}x")

    a_err_eof = next(r for r in results if r["name"].startswith("a_err_eof"))
    if a_err_eof["speedup"] >= 10.0:
        print(f"\n[PASS] a_err_eof >=10x target met (actual {a_err_eof['speedup']:.2f}x)")
    else:
        print(f"\n[WARN] a_err_eof <10x (actual {a_err_eof['speedup']:.2f}x) - VET Controlled A/B Test required")


if __name__ == "__main__":
    main()
