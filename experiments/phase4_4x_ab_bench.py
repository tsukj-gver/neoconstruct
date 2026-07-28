"""4.x O1-O4 Controlled A/B Test bench (VET stage).

Differs from phase4_4x_err_perf.py (DEV single-shot):
  - Adds label argv for A/B identification (e.g. A1_Ooff / B1_Oon)
  - Writes per-run JSON to experiments/phase4_4x_ab_bench_<label>.json
  - One-line [SUMMARY] for harness log scraping

Scenarios (per design section 5.2):
  - a_err_eof parse          : positive control (O3 should improve)
  - p_err_overflow build     : positive control 2 (O3 should improve, capped ~3-4x)
  - i02_idx_n100 parse       : negative control (no error throw, env drift gauge)
  - a01_array_n100 parse     : negative control 2 (no error throw, normal path)

Measurement protocol (performance-gate SKILL S-PERF):
  - min(repeat=5) x number
  - subprocess isolation (CRS / PC separate venvs)
  - apples-to-apples (both sides wrapped in Struct)

Usage:
  & crs_venv_new\\Scripts\\python.exe experiments\\phase4_4x_ab_bench.py <label>
"""

import sys
import os
import json
import time
import subprocess
import platform

try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
except Exception:
    pass

CRS_PYTHON = r"<opencode-temp>\crs_venv_new\Scripts\python.exe"
PC_PYTHON = r"<opencode-temp>\crs_venv_py_new\Scripts\python.exe"

REPEAT = 5


def measure(python_exe, code_template, number):
    """Run `number` iterations, return min ns/call (perf_counter-based)."""
    actual_code = code_template.replace("%NUMBER%", str(number))
    proc = subprocess.run(
        [python_exe, "-c", actual_code],
        capture_output=True, text=True, timeout=300,
        env={**os.environ, "PYO3_USE_ABI3_FORWARD_COMPATIBILITY": "1"},
    )
    if proc.returncode != 0:
        sys.stderr.write(f"STDERR: {proc.stderr[:1000]}\n")
        raise RuntimeError(f"subprocess failed: rc={proc.returncode}")
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

A01_N100_CRS = """
import time as _time
from dataclasses import dataclass
from construct import StructMixin, field, Array, Int8ub

@dataclass
class P(StructMixin):
    items: list = field(Array(100, Int8ub))

DATA = bytes(100)

for _ in range(50):
    P.parse(DATA)

t0 = _time.perf_counter()
for _ in range(%NUMBER%):
    P.parse(DATA)
elapsed = (_time.perf_counter() - t0) / %NUMBER%
print(elapsed * 1e9)
"""

A01_N100_PC = """
import time as _time
import construct as pc

d = pc.Struct("items" / pc.Array(100, pc.Int8ub))
DATA = bytes(100)

for _ in range(50):
    d.parse(DATA)

t0 = _time.perf_counter()
for _ in range(%NUMBER%):
    d.parse(DATA)
elapsed = (_time.perf_counter() - t0) / %NUMBER%
print(elapsed * 1e9)
"""


def run_scenario(key, name, crs_code, pc_code, number):
    print(f"\n--- {name} (number={number}) ---", flush=True)
    crs_runs = [measure(CRS_PYTHON, crs_code, number) for _ in range(REPEAT)]
    pc_runs = [measure(PC_PYTHON, pc_code, number) for _ in range(REPEAT)]
    crs_min = min(crs_runs)
    pc_min = min(pc_runs)
    speedup = pc_min / crs_min
    mark = "OK>=10x" if speedup >= 10 else "<10x"
    print(f"  crs min: {crs_min:.1f} ns/call", flush=True)
    print(f"  pc  min: {pc_min:.1f} ns/call", flush=True)
    print(f"  speedup: {speedup:.2f}x  [{mark}]", flush=True)
    return {
        "key": key, "name": name, "number": number,
        "crs_ns": crs_min, "pc_ns": pc_min, "speedup": speedup,
        "crs_runs": crs_runs, "pc_runs": pc_runs,
    }


def main():
    label = sys.argv[1] if len(sys.argv) > 1 else "unknown"
    ts = time.strftime("%Y-%m-%d %H:%M:%S")

    print(f"\n{'='*70}")
    print(f"4.x O1-O4 A/B bench | label={label} | timestamp={ts}")
    print(f"{'='*70}", flush=True)
    print(f"OS: {platform.system()} {platform.release()}")
    print(f"CPU: {platform.processor()}")
    print(f"REPEAT: {REPEAT}", flush=True)

    results = []
    results.append(run_scenario(
        "a_err_eof", "a_err_eof parse [positive]", A_ERR_EOF_CRS, A_ERR_EOF_PC, 3000,
    ))
    results.append(run_scenario(
        "p_err_overflow", "p_err_overflow build [positive]", P_ERR_OVERFLOW_CRS, P_ERR_OVERFLOW_PC, 3000,
    ))
    results.append(run_scenario(
        "i02_idx", "i02_idx_n100 parse [negative]", I02_IDX_CRS, I02_IDX_PC, 3000,
    ))
    results.append(run_scenario(
        "a01_n100", "a01 Array(100, Int8ub) parse [negative]", A01_N100_CRS, A01_N100_PC, 3000,
    ))

    out = {
        "label": label, "timestamp": ts, "repeat": REPEAT,
        "results": results,
    }
    out_path = os.path.join(
        os.path.dirname(__file__),
        f"phase4_4x_ab_bench_{label}.json",
    )
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(out, f, ensure_ascii=False, indent=2)
    print(f"\n结果保存到 {out_path}", flush=True)

    parts = [f'{r["key"]}={r["speedup"]:.2f}x(rs={r["crs_ns"]:.0f}ns,pc={r["pc_ns"]:.0f}ns)'
             for r in results]
    print(f"\n[SUMMARY] label={label} | " + " | ".join(parts), flush=True)


if __name__ == "__main__":
    main()
