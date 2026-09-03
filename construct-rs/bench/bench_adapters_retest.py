"""Controlled A/B Test retest — 14 parse <10x cases.

Methodology:
  1. Alternating rs↔py per round (rs then py back-to-back, repeat ≥5 rounds)
  2. Positive control: UN1 Union parse / SQ1 Sequence parse (known ≥10x)
  3. Negative control: build direction (known ≥10x, same session)
  4. Edge case ≥5 samples, report min/max/mean/stddev

Each round:
  - measure rs subprocess (crs venv, number=5000, repeat=10+3 warmup)
  - immediately measure py subprocess (py venv, same params)
  → one (rs_ns, py_ns) pair → one speedup sample
  5 rounds → 5 speedup samples → statistics

Usage::

    python bench/bench_adapters_retest.py
"""

from __future__ import annotations

import json
import os
import sys
import time
from pathlib import Path
from statistics import mean, stdev

_BENCH_DIR = Path(__file__).resolve().parent
if str(_BENCH_DIR) not in sys.path:
    sys.path.insert(0, str(_BENCH_DIR))

from _helpers.runner import BenchConfig, BenchRunner
from _helpers.stats import speedup_ratio

# Reuse bench_adapters_struct_streams.py infrastructure
import bench_adapters_struct_streams

# ---------------------------------------------------------------------------
# venv resolution (same as bench_adapters_struct_streams.py: env var > project .venv/.venv-pc > sys.executable)
# ---------------------------------------------------------------------------
_PROJECT_ROOT = _BENCH_DIR.parent  # construct-rs/
_DEFAULT_RS_PYTHON = _PROJECT_ROOT / ".venv"     # construct-rs 扩展 venv
_DEFAULT_PY_PYTHON = _PROJECT_ROOT / ".venv-pc"  # 原版参考 venv（construct==2.10.70）
_CRS_PYTHON_DIR = str(_BENCH_DIR.parent / "python")


def _resolve_venv(env_var, venv_dir):
    """三级解析：环境变量 → 项目内 venv → sys.executable（与 conftest.py 一致）。"""
    env = os.environ.get(env_var)
    if env and Path(env).exists():
        return env
    for rel in ("Scripts/python.exe", "bin/python"):
        candidate = Path(venv_dir) / rel
        if candidate.exists():
            return str(candidate)
    return sys.executable


# ---------------------------------------------------------------------------
# Case definitions
# ---------------------------------------------------------------------------

# 14 parse cases that were <10x in initial bench
RETEST_PARSE_CASES = [
    ("CN1", "Const"),
    ("CN2", "Const"),
    ("HX1", "Hex"),
    ("HX2", "Hex"),
    ("HD1", "HexDump"),
    ("AL2", "Aligned"),
    ("TM1", "Terminated"),
    ("EN1", "Enum"),
    ("EN2", "Enum"),
    ("FE1", "FlagsEnum"),
    ("MP1", "Mapping"),
    ("OO1", "OneOf"),
    ("NO1", "NoneOf"),
    ("NT1", "NamedTuple"),
]

# Positive controls: known ≥10x parse cases (verify measurement system works)
POSITIVE_CONTROLS = [
    ("UN1", "Union"),
    ("SQ1", "Sequence"),
]

# Negative controls: build direction (known ≥10x, same session → env drift check)
NEGATIVE_CONTROLS = [
    ("CN1", "Const"),
    ("TM1", "Terminated"),
    ("EN1", "Enum"),
    ("MP1", "Mapping"),
]

ROUNDS = 5  # ≥5 samples per case


def measure_case_round(runner, case_id, direction, config):
    """Measure one (rs, py) pair for a case — one round.

    Returns (rs_median_ns, py_median_ns, speedup).
    """
    dispatcher, src_group = bench_adapters_struct_streams._resolve_case_dispatcher(case_id)
    make_case_src = bench_adapters_struct_streams._resolve_make_case_src(src_group)
    repeat = config.iterations + config.warmup
    scenario = f"{case_id}-{direction}"

    rs_script = bench_adapters_struct_streams._build_script(
        "rs", case_id, direction, config.number, repeat,
        _CRS_PYTHON_DIR, make_case_src, dispatcher,
    )
    py_script = bench_adapters_struct_streams._build_script(
        "py", case_id, direction, config.number, repeat,
        _CRS_PYTHON_DIR, make_case_src, dispatcher,
    )

    # Alternating: rs first, then py immediately after
    rs_result = runner.run_scenario("rs", scenario, rs_script)
    py_result = runner.run_scenario("py", scenario, py_script)
    sp = speedup_ratio(rs_result.median_ns, py_result.median_ns)
    return rs_result.median_ns, py_result.median_ns, sp


def run_ab_test(runner, case_id, constructor, direction, config, rounds=ROUNDS):
    """Run A/B test for one case: `rounds` alternating rs↔py pairs.

    Returns dict with statistics.
    """
    speedups = []
    rs_samples = []
    py_samples = []

    for r in range(rounds):
        rs_ns, py_ns, sp = measure_case_round(runner, case_id, direction, config)
        speedups.append(sp)
        rs_samples.append(rs_ns)
        py_samples.append(py_ns)
        print(f"    round {r+1}/{rounds}: rs={rs_ns:.0f}ns py={py_ns:.0f}ns "
              f"speedup={sp:.2f}x", flush=True)

    return {
        "case_id": case_id,
        "constructor": constructor,
        "direction": direction,
        "rounds": rounds,
        "speedups": speedups,
        "rs_samples": rs_samples,
        "py_samples": py_samples,
        "speedup_min": min(speedups),
        "speedup_max": max(speedups),
        "speedup_mean": mean(speedups),
        "speedup_stdev": stdev(speedups) if len(speedups) > 1 else 0.0,
        "rs_mean": mean(rs_samples),
        "py_mean": mean(py_samples),
    }


def classify(result, gate=10.0):
    """Classify retest result.

    Returns (verdict, detail).
    - DRIFT: ≥10x → measurement drift confirmed (initial <10x was false negative)
    - STRUCTURAL: 4x ≤ speedup < 10x → real structural boundary
    - ANOMALY: <4x → anomalous, needs investigation
    """
    sp_mean = result["speedup_mean"]
    sp_min = result["speedup_min"]
    if sp_min >= gate:
        return "DRIFT", f"all rounds ≥{gate}x (min={sp_min:.2f}x) → drift confirmed"
    if sp_mean >= gate:
        return "DRIFT", f"mean ≥{gate}x ({sp_mean:.2f}x) but min={sp_min:.2f}x → likely drift"
    if sp_mean >= 4.0:
        return "STRUCTURAL", f"mean={sp_mean:.2f}x (4-10x range) → structural boundary"
    return "ANOMALY", f"mean={sp_mean:.2f}x (<4x) → anomalous"


def main():
    config = BenchConfig(
        iterations=10,
        warmup=3,
        number=5000,
    )

    rs_python = _resolve_venv("CRS_PYTHON", _DEFAULT_RS_PYTHON)
    py_python = _resolve_venv("PC_PYTHON", _DEFAULT_PY_PYTHON)

    runner = BenchRunner(rs_python, py_python, _CRS_PYTHON_DIR, config=config)

    ts = time.strftime("%Y-%m-%d %H:%M:%S")
    print("=" * 78)
    print(f"Controlled A/B Test Retest | {ts}")
    print("=" * 78)
    print(f"Python (rs): {rs_python}")
    print(f"Python (py): {py_python}")
    print(f"number={config.number}, iterations={config.iterations}, warmup={config.warmup}")
    print(f"Rounds per case: {ROUNDS} (alternating rs<->py)")
    print()

    all_results = []

    # --- Positive controls ---
    print("=" * 60)
    print("POSITIVE CONTROLS (known ≥10x — verify measurement system)")
    print("=" * 60)
    for case_id, constructor in POSITIVE_CONTROLS:
        print(f"\n  [{case_id}] {constructor} parse:")
        result = run_ab_test(runner, case_id, constructor, "parse", config)
        verdict, detail = classify(result)
        result["category"] = "positive_control"
        result["verdict"] = verdict
        result["detail"] = detail
        all_results.append(result)
        print(f"  → mean={result['speedup_mean']:.2f}x "
              f"(min={result['speedup_min']:.2f}, max={result['speedup_max']:.2f}, "
              f"std={result['speedup_stdev']:.2f}) [{verdict}]")

    # --- Negative controls (build direction) ---
    print()
    print("=" * 60)
    print("NEGATIVE CONTROLS (build direction — same session env drift check)")
    print("=" * 60)
    for case_id, constructor in NEGATIVE_CONTROLS:
        print(f"\n  [{case_id}] {constructor} build:")
        result = run_ab_test(runner, case_id, constructor, "build", config)
        verdict, detail = classify(result)
        result["category"] = "negative_control"
        result["verdict"] = verdict
        result["detail"] = detail
        all_results.append(result)
        print(f"  → mean={result['speedup_mean']:.2f}x "
              f"(min={result['speedup_min']:.2f}, max={result['speedup_max']:.2f}, "
              f"std={result['speedup_stdev']:.2f}) [{verdict}]")

    # --- Retest cases (14 parse <10x) ---
    print()
    print("=" * 60)
    print("RETEST: 14 parse cases (<10x initial measurement)")
    print("=" * 60)
    for case_id, constructor in RETEST_PARSE_CASES:
        print(f"\n  [{case_id}] {constructor} parse:")
        result = run_ab_test(runner, case_id, constructor, "parse", config)
        verdict, detail = classify(result)
        result["category"] = "retest"
        result["verdict"] = verdict
        result["detail"] = detail
        all_results.append(result)
        print(f"  → mean={result['speedup_mean']:.2f}x "
              f"(min={result['speedup_min']:.2f}, max={result['speedup_max']:.2f}, "
              f"std={result['speedup_stdev']:.2f}) [{verdict}]")
        print(f"    {detail}")

    # --- Summary ---
    print()
    print("=" * 78)
    print("SUMMARY")
    print("=" * 78)
    print(f"{'Case':<6} {'Constructor':<16} {'Dir':<7} {'Mean':>7} {'Min':>7} "
          f"{'Max':>7} {'Std':>6} {'Verdict':<12}")
    print("-" * 78)
    for r in all_results:
        print(f"{r['case_id']:<6} {r['constructor']:<16} {r['direction']:<7} "
              f"{r['speedup_mean']:>6.2f}x {r['speedup_min']:>6.2f}x "
              f"{r['speedup_max']:>6.2f}x {r['speedup_stdev']:>5.2f}  "
              f"{r['verdict']:<12}")

    # Count verdicts for retest cases
    retest_results = [r for r in all_results if r["category"] == "retest"]
    drift_count = sum(1 for r in retest_results if r["verdict"] == "DRIFT")
    structural_count = sum(1 for r in retest_results if r["verdict"] == "STRUCTURAL")
    anomaly_count = sum(1 for r in retest_results if r["verdict"] == "ANOMALY")
    print()
    print(f"Retest verdicts: {drift_count} DRIFT / {structural_count} STRUCTURAL / "
          f"{anomaly_count} ANOMALY (of {len(retest_results)} cases)")

    # Save results
    out_path = _BENCH_DIR / "results" / "bench_adapters_retest.json"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    output = {
        "timestamp": ts,
        "config": {
            "number": config.number,
            "iterations": config.iterations,
            "warmup": config.warmup,
            "rounds": ROUNDS,
        },
        "results": all_results,
        "summary": {
            "retest_drift": drift_count,
            "retest_structural": structural_count,
            "retest_anomaly": anomaly_count,
            "retest_total": len(retest_results),
        },
    }
    with out_path.open("w", encoding="utf-8") as f:
        json.dump(output, f, ensure_ascii=False, indent=2)
    print(f"\nResults saved to: {out_path}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
