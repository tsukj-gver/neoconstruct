"""Controlled A/B Test 统计分析（META-CI-1c，L-09 对策工程化）.

通用化自 experiments/E01_O1_ab_stats.py：
- 移除硬编码 LABELS_A / LABELS_B / SCENARIOS
- 通过 CLI 参数 / JSON 配置参数化

用法：
  python ab_stats.py --a-labels A1,A2,A3 --b-labels B1,B2,B3 \\
      --bench-pattern "ab_bench_{label}.json" \\
      --scenarios E01,S01,i01

或：
  python ab_stats.py --config ab_stats_config.json

判据（performance-gate SKILL Checkpoint 4）：
  |Δ| > 0.5x   → 回归确认
  |Δ| < 0.3x   → 测量波动
  0.3-0.5x     → 中间情况，加测 5 轮
"""
from __future__ import annotations

import json
import math
import os
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence, Tuple

# 判据阈值（performance-gate SKILL Checkpoint 4）
REGRESSION_DELTA_X = 0.5     # >0.5x → 回归
NOISE_DELTA_X = 0.3          # <0.3x → 波动
# 0.3-0.5x → 中间情况，加测


def mean(xs: List[float]) -> float:
    """算术平均。"""
    return sum(xs) / len(xs) if xs else float("nan")


def stdev_sample(xs: List[float]) -> float:
    """样本标准差（n-1 分母）。"""
    if len(xs) < 2:
        return float("nan")
    m = mean(xs)
    return math.sqrt(sum((x - m) ** 2 for x in xs) / (len(xs) - 1))


def welch_t(x1: List[float], x2: List[float]) -> float:
    """Welch's t-statistic（描述性，n=3 时仅供方向参考）。

    公式：(mean1 - mean2) / sqrt(s1^2/n1 + s2^2/n2)
    """
    m1, m2 = mean(x1), mean(x2)
    s1, s2 = stdev_sample(x1), stdev_sample(x2)
    n1, n2 = len(x1), len(x2)
    if math.isnan(s1) or math.isnan(s2):
        return float("nan")
    se = math.sqrt(s1 ** 2 / n1 + s2 ** 2 / n2)
    if se == 0:
        return float("inf") if m1 != m2 else 0.0
    return (m1 - m2) / se


def pooled_noise(x1: List[float], x2: List[float]) -> float:
    """组内噪声代理（pooled within-group stddev）。"""
    s1, s2 = stdev_sample(x1), stdev_sample(x2)
    if math.isnan(s1) or math.isnan(s2):
        return 0.0
    return math.sqrt((s1 ** 2 + s2 ** 2) / 2)


def delta_to_noise_ratio(diff: float, noise: float) -> float:
    """|Δ|/noise 比率（用于判断信号强度）。"""
    return abs(diff) / noise if noise > 0 else float("inf")


def classify_delta(delta_x: float) -> str:
    """根据 Checkpoint 4 判据分类 Δspeedup。"""
    abs_d = abs(delta_x)
    if abs_d > REGRESSION_DELTA_X:
        return "REGRESSION"
    if abs_d < NOISE_DELTA_X:
        return "NOISE"
    return "INTERMEDIATE"


def load_bench_result(
    bench_dir: Path,
    bench_pattern: str,
    label: str,
) -> Dict[str, Any]:
    """加载单次 bench 结果 JSON。

    bench_pattern 含 {label} 占位符，如 "ab_bench_{label}.json"。
    """
    fname = bench_pattern.replace("{label}", label)
    path = bench_dir / fname
    if not path.exists():
        raise FileNotFoundError(f"bench result not found: {path}")
    with path.open("r", encoding="utf-8") as f:
        return json.load(f)


def extract_scenario_metrics(
    bench_data: Dict[str, Any],
    scenario_key: str,
) -> Tuple[float, float, float]:
    """从 bench_data["results"] 中提取指定场景的 (speedup, rs_ns, pc_ns)。"""
    for r in bench_data.get("results", []):
        if r.get("key") == scenario_key:
            return float(r["speedup"]), float(r["crs_ns"]), float(r["pc_ns"])
    raise KeyError(f"scenario '{scenario_key}' not found in bench results")


def analyze_scenario(
    bench_dir: Path,
    bench_pattern: str,
    a_labels: List[str],
    b_labels: List[str],
    scenario_key: str,
) -> Dict[str, Any]:
    """对单个场景做 A/B 统计分析。

    返回 dict 含：
      scenario / metric / A_mean / A_stddev / A_min / A_max
      B_mean / B_stddev / B_min / B_max
      delta (A-B) / abs_delta / welch_t / pooled_noise / delta_noise_ratio
      verdict (REGRESSION / NOISE / INTERMEDIATE)
    """
    a_sp, a_rs, a_pc = [], [], []
    b_sp, b_rs, b_pc = [], [], []
    for lab in a_labels:
        data = load_bench_result(bench_dir, bench_pattern, lab)
        sp, rs, pc = extract_scenario_metrics(data, scenario_key)
        a_sp.append(sp); a_rs.append(rs); a_pc.append(pc)
    for lab in b_labels:
        data = load_bench_result(bench_dir, bench_pattern, lab)
        sp, rs, pc = extract_scenario_metrics(data, scenario_key)
        b_sp.append(sp); b_rs.append(rs); b_pc.append(pc)

    out: Dict[str, Any] = {
        "scenario": scenario_key,
        "n_A": len(a_sp), "n_B": len(b_sp),
    }
    for metric_name, A, B in [
        ("speedup_x", a_sp, b_sp),
        ("rs_ns", a_rs, b_rs),
        ("pc_ns", a_pc, b_pc),
    ]:
        mA, mB = mean(A), mean(B)
        sA, sB = stdev_sample(A), stdev_sample(B)
        diff = mA - mB
        noise = pooled_noise(A, B)
        ratio = delta_to_noise_ratio(diff, noise)
        t = welch_t(A, B)
        out[metric_name] = {
            "A_mean": mA, "A_stddev": sA, "A_min": min(A) if A else None, "A_max": max(A) if A else None,
            "B_mean": mB, "B_stddev": sB, "B_min": min(B) if B else None, "B_max": max(B) if B else None,
            "delta_A_minus_B": diff,
            "abs_delta": abs(diff),
            "welch_t": t,
            "pooled_noise": noise,
            "delta_to_noise": ratio,
        }
    # 判据仅针对 speedup_x
    sp_delta = out["speedup_x"]["delta_A_minus_B"]
    out["verdict"] = classify_delta(sp_delta)
    out["verdict_rule"] = (
        f"|Δ|>{REGRESSION_DELTA_X}=REGRESSION / "
        f"|Δ|<{NOISE_DELTA_X}=NOISE / "
        f"intermediate=需加测"
    )
    return out


def run_analysis(
    bench_dir: Path,
    bench_pattern: str,
    a_labels: List[str],
    b_labels: List[str],
    scenarios: List[str],
) -> Dict[str, Any]:
    """对多个场景做完整 A/B 统计分析。"""
    results = []
    for sc in scenarios:
        try:
            r = analyze_scenario(bench_dir, bench_pattern, a_labels, b_labels, sc)
            results.append(r)
        except (FileNotFoundError, KeyError) as exc:
            results.append({"scenario": sc, "error": str(exc)})
    # 汇总判定
    regressions = [r for r in results if r.get("verdict") == "REGRESSION"]
    noises = [r for r in results if r.get("verdict") == "NOISE"]
    intermediates = [r for r in results if r.get("verdict") == "INTERMEDIATE"]
    overall_verdict = "REGRESSION" if regressions else (
        "NOISE" if len(noises) == len(results) else "MIXED"
    )
    return {
        "summary": {
            "total": len(results),
            "regression": len(regressions),
            "noise": len(noises),
            "intermediate": len(intermediates),
            "overall_verdict": overall_verdict,
        },
        "scenarios": results,
    }


def _parse_csv_arg(s: str) -> List[str]:
    """解析命令行逗号分隔参数。"""
    return [x.strip() for x in s.split(",") if x.strip()]


def main(argv: Optional[Sequence[str]] = None) -> int:
    """CLI 主入口。"""
    argv = list(sys.argv[1:] if argv is None else argv)
    a_labels: List[str] = []
    b_labels: List[str] = []
    scenarios: List[str] = []
    bench_pattern = "ab_bench_{label}.json"
    bench_dir = Path(".")
    output: Optional[str] = None
    config_path: Optional[str] = None

    i = 0
    while i < len(argv):
        a = argv[i]
        if a == "--a-labels" and i + 1 < len(argv):
            a_labels = _parse_csv_arg(argv[i + 1]); i += 2; continue
        if a == "--b-labels" and i + 1 < len(argv):
            b_labels = _parse_csv_arg(argv[i + 1]); i += 2; continue
        if a == "--scenarios" and i + 1 < len(argv):
            scenarios = _parse_csv_arg(argv[i + 1]); i += 2; continue
        if a == "--bench-pattern" and i + 1 < len(argv):
            bench_pattern = argv[i + 1]; i += 2; continue
        if a == "--bench-dir" and i + 1 < len(argv):
            bench_dir = Path(argv[i + 1]); i += 2; continue
        if a == "--output" and i + 1 < len(argv):
            output = argv[i + 1]; i += 2; continue
        if a == "--config" and i + 1 < len(argv):
            config_path = argv[i + 1]; i += 2; continue
        if a in ("-h", "--help"):
            sys.stdout.write(__doc__ or "")
            return 0
        sys.stderr.write(f"unknown arg: {a}\n")
        return 2

    # 从 config 覆盖
    if config_path:
        with open(config_path, encoding="utf-8") as f:
            cfg = json.load(f)
        a_labels = cfg.get("a_labels", a_labels)
        b_labels = cfg.get("b_labels", b_labels)
        scenarios = cfg.get("scenarios", scenarios)
        bench_pattern = cfg.get("bench_pattern", bench_pattern)
        bench_dir = Path(cfg.get("bench_dir", str(bench_dir)))

    if not a_labels or not b_labels or not scenarios:
        sys.stderr.write(
            "required: --a-labels, --b-labels, --scenarios "
            "(or --config with these keys)\n"
        )
        return 2

    report = run_analysis(bench_dir, bench_pattern, a_labels, b_labels, scenarios)

    # 控制台表格输出
    print("=" * 90)
    print(f"Controlled A/B Test Statistical Analysis")
    print(f"  A labels: {a_labels}")
    print(f"  B labels: {b_labels}")
    print(f"  scenarios: {scenarios}")
    print(f"  bench_dir: {bench_dir}")
    print("=" * 90)
    print(f"{'scenario':<10}{'metric':<12}{'A mean±std':<26}{'B mean±std':<26}{'Δ(A-B)':<10}{'verdict'}")
    print("-" * 90)
    for sc in report["scenarios"]:
        if "error" in sc:
            print(f"{sc['scenario']:<10}  ERROR: {sc['error']}")
            continue
        for m in ("speedup_x", "rs_ns", "pc_ns"):
            d = sc[m]
            a_str = f"{d['A_mean']:.3f}±{d['A_stddev']:.3f}"
            b_str = f"{d['B_mean']:.3f}±{d['B_stddev']:.3f}"
            delta_str = f"{d['delta_A_minus_B']:+.3f}"
            v = sc["verdict"] if m == "speedup_x" else ""
            print(f"{sc['scenario']:<10}{m:<12}{a_str:<26}{b_str:<26}{delta_str:<10}{v}")
        print("-" * 90)
    print(f"\nOverall verdict: {report['summary']['overall_verdict']}")
    print(f"  regression={report['summary']['regression']} "
          f"noise={report['summary']['noise']} "
          f"intermediate={report['summary']['intermediate']}")

    if output:
        out_path = Path(output)
        out_path.parent.mkdir(parents=True, exist_ok=True)
        with out_path.open("w", encoding="utf-8") as f:
            json.dump(report, f, ensure_ascii=False, indent=2)
        print(f"Report written: {out_path}")

    # 退出码：REGRESSION=2 / NOISE=0 / MIXED/INTERMEDIATE=1
    return 2 if report["summary"]["overall_verdict"] == "REGRESSION" else (
        0 if report["summary"]["overall_verdict"] == "NOISE" else 1
    )


if __name__ == "__main__":
    sys.exit(main())
