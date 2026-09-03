"""Phase 8 OPT-SHARED Controlled A/B Test retest (post O1-A + O1-B + O2-A).

Target: 7 LOW cases + positive controls + key TM1 shared-tax check.
Baseline: `bench/results/bench_phase8_313.json` (Python 3.13 non-abi3, 8.ENV §4.1).

Methodology (L-09 + design §4.2):
  1. Alternating rs↔py per round (rs then py back-to-back, repeat ≥5 rounds)
  2. Positive control: TM1 / EN1 / MP1 (3.13 baseline ≥10x — verify no regression)
  3. Negative control: build direction (same session env drift check)
  4. Failure criterion: TM1 rs_ns > 200ns → shared-tax optimization failed
     (design §3.3 + ADR-023 §预测; theoretical ~165-180ns + margin)

Reuses `bench_phase8_ab_retest.py` infrastructure. Environment variables:
  - CRS_PYTHON: rs venv (default: project-local .venv)
  - PC_PYTHON:  py venv (default: project-local .venv-pc)

Usage::

    python bench/bench_opt_shared_retest.py
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

import bench_phase8
import bench_phase8_ab_retest as ab
from _helpers.runner import BenchConfig, BenchRunner
from _helpers.stats import speedup_ratio

# ---------------------------------------------------------------------------
# venv resolution (env var > project .venv/.venv-pc > sys.executable, same as conftest.py)
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
# OPT-SHARED target cases
# ---------------------------------------------------------------------------

# 7 LOW cases from 3.13 baseline (8.ENV §4.1) — primary OPT-SHARED targets
LOW_CASES = [
    ("CN1", "Const"),
    ("HX1", "Hex"),
    ("HX2", "Hex"),
    ("HD1", "HexDump"),
    ("FE1", "FlagsEnum"),  # parse direction
    ("NT1", "NamedTuple"),
]

# FE1 build is also LOW but a known limitation (PM decision accepted <10x)
LOW_BUILD_CASES = [
    ("FE1", "FlagsEnum"),  # build direction
]

# Positive controls: 3.13 baseline already ≥10x — verify no regression
POSITIVE_CONTROLS = [
    ("TM1", "Terminated"),  # ★ key shared-tax probe (theory ~165-180ns)
    ("EN1", "Enum"),
    ("MP1", "Mapping"),
]

# Negative controls: build direction (same session env drift check)
NEGATIVE_CONTROLS = [
    ("CN1", "Const"),
    ("TM1", "Terminated"),
]

# Acceptance criterion (design §3.3)
TM1_FAILURE_NS = 200  # TM1 rs_ns > 200ns → shared-tax optimization failed
TM1_TARGET_NS = 180   # TM1 rs_ns ≤ 180ns (shared_tax ≤ 150 + 30 inner)

ROUNDS = 5


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
    print(f"Phase 8 OPT-SHARED Retest (O1-A + O1-B + O2-A) | {ts}")
    print("=" * 78)
    print(f"Python (rs): {rs_python}")
    print(f"Python (py): {py_python}")
    print(f"number={config.number}, iterations={config.iterations}, warmup={config.warmup}")
    print(f"Rounds per case: {ROUNDS} (alternating rs<->py)")
    print(f"Failure criterion: TM1 parse rs_ns > {TM1_FAILURE_NS}ns")
    print()

    all_results = []

    # --- Positive controls (verify no regression) ---
    print("=" * 60)
    print(f"POSITIVE CONTROLS (3.13 baseline ≥10x — verify no regression)")
    print("=" * 60)
    for case_id, constructor in POSITIVE_CONTROLS:
        print(f"\n  [{case_id}] {constructor} parse:")
        result = ab.run_ab_test(runner, case_id, constructor, "parse", config, rounds=ROUNDS)
        result["category"] = "positive_control"
        all_results.append(result)
        print(f"  → mean={result['speedup_mean']:.2f}x "
              f"(min={result['speedup_min']:.2f}, max={result['speedup_max']:.2f}, "
              f"std={result['speedup_stdev']:.2f}) rs_mean={result['rs_mean']:.0f}ns")

    # --- Negative controls (build direction — env drift check) ---
    print()
    print("=" * 60)
    print("NEGATIVE CONTROLS (build direction — env drift check)")
    print("=" * 60)
    for case_id, constructor in NEGATIVE_CONTROLS:
        print(f"\n  [{case_id}] {constructor} build:")
        result = ab.run_ab_test(runner, case_id, constructor, "build", config, rounds=ROUNDS)
        result["category"] = "negative_control"
        all_results.append(result)
        print(f"  → mean={result['speedup_mean']:.2f}x "
              f"(min={result['speedup_min']:.2f}, max={result['speedup_max']:.2f}, "
              f"std={result['speedup_stdev']:.2f}) rs_mean={result['rs_mean']:.0f}ns")

    # --- LOW parse cases ---
    print()
    print("=" * 60)
    print("LOW PARSE CASES (OPT-SHARED primary targets)")
    print("=" * 60)
    for case_id, constructor in LOW_CASES:
        print(f"\n  [{case_id}] {constructor} parse:")
        result = ab.run_ab_test(runner, case_id, constructor, "parse", config, rounds=ROUNDS)
        result["category"] = "low_parse"
        all_results.append(result)
        print(f"  → mean={result['speedup_mean']:.2f}x "
              f"(min={result['speedup_min']:.2f}, max={result['speedup_max']:.2f}, "
              f"std={result['speedup_stdev']:.2f}) rs_mean={result['rs_mean']:.0f}ns")

    # --- LOW build cases (known limitation) ---
    print()
    print("=" * 60)
    print("LOW BUILD CASES (known limitation — PM decision accepted <10x)")
    print("=" * 60)
    for case_id, constructor in LOW_BUILD_CASES:
        print(f"\n  [{case_id}] {constructor} build:")
        result = ab.run_ab_test(runner, case_id, constructor, "build", config, rounds=ROUNDS)
        result["category"] = "low_build"
        all_results.append(result)
        print(f"  → mean={result['speedup_mean']:.2f}x "
              f"(min={result['speedup_min']:.2f}, max={result['speedup_max']:.2f}, "
              f"std={result['speedup_stdev']:.2f}) rs_mean={result['rs_mean']:.0f}ns")

    # --- Summary table ---
    print()
    print("=" * 90)
    print("SUMMARY (post OPT-SHARED O1-A + O1-B + O2-A)")
    print("=" * 90)
    print(f"{'Case':<6} {'Constructor':<16} {'Dir':<7} {'Mean':>8} {'Min':>8} "
          f"{'Max':>8} {'Std':>7} {'rs_mean':>9} {'Cat':<18}")
    print("-" * 90)
    for r in all_results:
        print(f"{r['case_id']:<6} {r['constructor']:<16} {r['direction']:<7} "
              f"{r['speedup_mean']:>7.2f}x {r['speedup_min']:>7.2f}x "
              f"{r['speedup_max']:>7.2f}x {r['speedup_stdev']:>6.2f}  "
              f"{r['rs_mean']:>7.0f}ns {r['category']:<18}")

    # --- Shared-tax TM1 verdict ---
    print()
    print("=" * 60)
    print("SHARED-TAX TM1 VERDICT (design §3.3)")
    print("=" * 60)
    tm1 = next((r for r in all_results
                if r["case_id"] == "TM1" and r["direction"] == "parse"), None)
    if tm1:
        tm1_rs = tm1["rs_mean"]
        if tm1_rs > TM1_FAILURE_NS:
            verdict = f"[FAIL] TM1 rs_mean={tm1_rs:.0f}ns > {TM1_FAILURE_NS}ns"
            print(f"  {verdict}")
            print(f"     -> shared-tax optimization failed (theory ~165-180ns + margin)")
        elif tm1_rs <= TM1_TARGET_NS:
            verdict = f"[TARGET MET] TM1 rs_mean={tm1_rs:.0f}ns <= {TM1_TARGET_NS}ns"
            print(f"  {verdict}")
            print(f"     -> shared-tax in target range (~150ns tax + ~30ns inner)")
        else:
            verdict = f"[IMPROVED] TM1 rs_mean={tm1_rs:.0f}ns <= {TM1_FAILURE_NS}ns (failure threshold)"
            print(f"  {verdict}")
            print(f"     -> not failed but above target ({TM1_TARGET_NS}ns); L-09 noise band")

    # Save results
    out_path = _BENCH_DIR / "results" / "bench_opt_shared_retest.json"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    output = {
        "timestamp": ts,
        "phase": "8.OPT-SHARED",
        "config": {
            "number": config.number,
            "iterations": config.iterations,
            "warmup": config.warmup,
            "rounds": ROUNDS,
        },
        "env": {
            "rs_python": rs_python,
            "py_python": py_python,
        },
        "criteria": {
            "tm1_failure_ns": TM1_FAILURE_NS,
            "tm1_target_ns": TM1_TARGET_NS,
        },
        "results": all_results,
        "tm1_verdict": verdict if tm1 else "TM1 not measured",
    }
    with out_path.open("w", encoding="utf-8") as f:
        json.dump(output, f, ensure_ascii=False, indent=2)
    print(f"\nResults saved to: {out_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())