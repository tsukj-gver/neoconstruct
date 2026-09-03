"""Hex Parse Optimization Controlled A/B Test.

Focused retest of HX1/HX2/HD1 parse after the display-format optimization
(setattr + intern fmtstr).
- Positive control: UN1 Union parse (known >=10x)
- Negative control: CN1 Const build (known >=10x, same session)

Methodology:
  1. Alternating rs<->py per round (rs then py back-to-back, repeat >=5 rounds)
  2. >=5 samples per case, report min/max/mean/stddev
  3. Cross-time comparison: optimized vs pre-optimization baseline
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

import bench_adapters_struct_streams

# venv 解析（与 conftest.py 一致：环境变量 > 项目内 .venv/.venv-pc > sys.executable）
_PROJECT_ROOT = _BENCH_DIR.parent  # neoconstruct/
_DEFAULT_RS_PYTHON = _PROJECT_ROOT / ".venv"     # neoconstruct 扩展 venv
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


# Pre-optimization baseline (Controlled A/B Test)
PRE_OPT_BASELINE = {
    "HX1_parse": {"rs_ns": 581, "py_ns": 2519, "speedup": 4.36},
    "HX2_parse": {"rs_ns": 330, "py_ns": 2296, "speedup": 6.87},
    "HD1_parse": {"rs_ns": 324, "py_ns": 2244, "speedup": 6.88},
}

ROUNDS = 5


def measure_case_round(runner, case_id, direction, config):
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

    rs_result = runner.run_scenario("rs", scenario, rs_script)
    py_result = runner.run_scenario("py", scenario, py_script)
    sp = speedup_ratio(rs_result.median_ns, py_result.median_ns)
    return rs_result.median_ns, py_result.median_ns, sp


def run_ab_test(runner, case_id, constructor, direction, config, rounds=ROUNDS):
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
    print(f"Hex Parse Optimization A/B Test | {ts}")
    print("=" * 78)
    print(f"Python (rs): {rs_python}")
    print(f"Python (py): {py_python}")
    print(f"number={config.number}, iterations={config.iterations}, warmup={config.warmup}")
    print(f"Rounds per case: {ROUNDS} (alternating rs<->py)")
    print()

    all_results = []

    # --- Positive control ---
    print("=" * 60)
    print("POSITIVE CONTROL: UN1 Union parse (known >=10x)")
    print("=" * 60)
    result = run_ab_test(runner, "UN1", "Union", "parse", config)
    result["category"] = "positive_control"
    all_results.append(result)
    print(f"  -> mean={result['speedup_mean']:.2f}x "
          f"(min={result['speedup_min']:.2f}, max={result['speedup_max']:.2f}, "
          f"std={result['speedup_stdev']:.2f})")

    # --- Negative control ---
    print()
    print("=" * 60)
    print("NEGATIVE CONTROL: CN1 Const build (known >=10x)")
    print("=" * 60)
    result = run_ab_test(runner, "CN1", "Const", "build", config)
    result["category"] = "negative_control"
    all_results.append(result)
    print(f"  -> mean={result['speedup_mean']:.2f}x "
          f"(min={result['speedup_min']:.2f}, max={result['speedup_max']:.2f}, "
          f"std={result['speedup_stdev']:.2f})")

    # --- Target cases: HX1/HX2/HD1 parse ---
    hex_cases = [
        ("HX1", "Hex"),
        ("HX2", "Hex"),
        ("HD1", "HexDump"),
    ]

    print()
    print("=" * 60)
    print("TARGET CASES: HX1/HX2/HD1 parse (optimized)")
    print("=" * 60)
    for case_id, constructor in hex_cases:
        print(f"\n  [{case_id}] {constructor} parse:")
        result = run_ab_test(runner, case_id, constructor, "parse", config)
        result["category"] = "target"
        all_results.append(result)
        print(f"  -> mean={result['speedup_mean']:.2f}x "
              f"(min={result['speedup_min']:.2f}, max={result['speedup_max']:.2f}, "
              f"std={result['speedup_stdev']:.2f})")

    # --- Summary ---
    print()
    print("=" * 78)
    print("SUMMARY: Optimized vs Pre-optimization Baseline")
    print("=" * 78)
    print(f"{'Case':<8} {'Dir':<7} {'Pre-opt':>8} {'Post-opt':>8} "
          f"{'rs_pre':>7} {'rs_post':>7} {'Delta':>7} {'Min':>7} {'Max':>7} {'Std':>6}")
    print("-" * 78)

    for r in all_results:
        if r["category"] != "target":
            continue
        key = f"{r['case_id']}_{r['direction']}"
        pre = PRE_OPT_BASELINE.get(key, {})
        pre_sp = pre.get("speedup", 0)
        pre_rs = pre.get("rs_ns", 0)
        delta = r["speedup_mean"] - pre_sp
        delta_str = f"{delta:+.2f}x" if pre_sp > 0 else "N/A"
        print(f"{r['case_id']:<8} {r['direction']:<7} {pre_sp:>7.2f}x "
              f"{r['speedup_mean']:>7.2f}x {pre_rs:>6.0f}ns {r['rs_mean']:>6.0f}ns "
              f"{delta_str:>7} {r['speedup_min']:>6.2f}x {r['speedup_max']:>6.2f}x "
              f"{r['speedup_stdev']:>5.2f}")

    print()
    print("Controls:")
    for r in all_results:
        if r["category"] == "target":
            continue
        print(f"  {r['case_id']:<6} {r['direction']:<7} {r['category']:<20} "
              f"mean={r['speedup_mean']:.2f}x (std={r['speedup_stdev']:.2f})")

    # Save results
    out_path = _BENCH_DIR / "results" / "bench_hex_opt_ab_retest.json"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    output = {
        "timestamp": ts,
        "optimization": "setattr + intern fmtstr",
        "config": {
            "number": config.number,
            "iterations": config.iterations,
            "warmup": config.warmup,
            "rounds": ROUNDS,
        },
        "pre_opt_baseline": PRE_OPT_BASELINE,
        "results": all_results,
    }
    with out_path.open("w", encoding="utf-8") as f:
        json.dump(output, f, ensure_ascii=False, indent=2)
    print(f"\nResults saved to: {out_path}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
