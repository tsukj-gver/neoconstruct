"""4.x O1-O4 Controlled A/B Test 统计分析.

读 6 份 JSON (A1/A2/A3 + B1/B2/B3) 算:
  - mean / stddev / min / max per condition
  - speedup delta (B - A)
  - welch_t (unequal variance t-test) for rs_ns
  - 判据 (per performance-gate SKILL Checkpoint 4):
      |delta_speedup| > 0.5  -> O1-O4 引入显著变化
      |delta_speedup| < 0.3  -> 测量波动
      0.3-0.5              -> 中间情况
"""

import json
import math
import os
import sys

try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
except Exception:
    pass

HERE = os.path.dirname(__file__)
LABELS_A = ["A1_Ooff", "A2_Ooff", "A3_Ooff"]
LABELS_B = ["B1_Oon", "B2_Oon", "B3_Oon"]


def load_runs(label):
    path = os.path.join(HERE, f"phase4_4x_ab_bench_{label}.json")
    with open(path, "r", encoding="utf-8") as f:
        return json.load(f)


def stats(values):
    n = len(values)
    mean = sum(values) / n
    var = sum((v - mean) ** 2 for v in values) / (n - 1) if n > 1 else 0.0
    return {
        "n": n, "mean": mean, "stddev": math.sqrt(var),
        "min": min(values), "max": max(values),
    }


def welch_t(a, b):
    """Welch's t-statistic (unequal variance). Returns (t, df_approx)."""
    sa, sb = stats(a), stats(b)
    if sa["stddev"] == 0 and sb["stddev"] == 0:
        return float("inf") if sa["mean"] != sb["mean"] else 0.0, sa["n"] + sb["n"] - 2
    va, vb = sa["stddev"] ** 2, sb["stddev"] ** 2
    na, nb = sa["n"], sb["n"]
    t = (sa["mean"] - sb["mean"]) / math.sqrt(va / na + vb / nb)
    # Welch-Satterthwaite df
    num = (va / na + vb / nb) ** 2
    den = (va ** 2) / (na ** 2 * (na - 1)) + (vb ** 2) / (nb ** 2 * (nb - 1))
    df = num / den if den > 0 else float("nan")
    return t, df


def t_critical_two_sided(df):
    """Approximate two-sided critical values for t-distribution.
    Returns (t@alpha=0.05, t@alpha=0.01). For df>=5 these are close enough
    to studentized ranges for engineering judgement."""
    # Lookup table for common df values (two-sided)
    table = {
        2: (4.303, 9.925),
        3: (3.182, 5.841),
        4: (2.776, 4.604),
        5: (2.571, 4.032),
        6: (2.447, 3.707),
        8: (2.306, 3.355),
        10: (2.228, 3.169),
        15: (2.131, 2.947),
        30: (2.042, 2.750),
    }
    if df < 2:
        return (float("inf"), float("inf"))
    # Find closest df
    keys = sorted(table.keys())
    closest = min(keys, key=lambda k: abs(k - df))
    return table[closest]


def main():
    print("=" * 72)
    print("4.x O1-O4 Controlled A/B Test 统计分析")
    print("=" * 72)

    runs_A = [load_runs(l) for l in LABELS_A]
    runs_B = [load_runs(l) for l in LABELS_B]

    keys = [r["key"] for r in runs_A[0]["results"]]

    print(f"\n{'='*72}")
    print(f"{'场景':<22} {'cond':<6} {'speedup mean':>13} {'stddev':>8} {'min':>7} {'max':>7}  | {'rs_ns mean':>10}")
    print(f"{'-'*72}")

    summary = []
    for k in keys:
        sp_A = [next(r["speedup"] for r in run["results"] if r["key"] == k) for run in runs_A]
        sp_B = [next(r["speedup"] for r in run["results"] if r["key"] == k) for run in runs_B]
        rs_A = [next(r["crs_ns"] for r in run["results"] if r["key"] == k) for run in runs_A]
        rs_B = [next(r["crs_ns"] for r in run["results"] if r["key"] == k) for run in runs_B]

        sA_sp, sB_sp = stats(sp_A), stats(sp_B)
        sA_rs, sB_rs = stats(rs_A), stats(rs_B)

        print(f"{k:<22} {'A(off)':<6} {sA_sp['mean']:>13.3f} {sA_sp['stddev']:>8.3f} "
              f"{sA_sp['min']:>7.2f} {sA_sp['max']:>7.2f}  | {sA_rs['mean']:>10.1f}")
        print(f"{'':<22} {'B(on)':<6} {sB_sp['mean']:>13.3f} {sB_sp['stddev']:>8.3f} "
              f"{sB_sp['min']:>7.2f} {sB_sp['max']:>7.2f}  | {sB_rs['mean']:>10.1f}")

        delta_sp = sB_sp["mean"] - sA_sp["mean"]
        delta_rs = sB_rs["mean"] - sA_rs["mean"]
        t_rs, df_rs = welch_t(rs_A, rs_B)
        t_sp, df_sp = welch_t(sp_A, sp_B)
        tcrit_05, tcrit_01 = t_critical_two_sided(min(df_rs, df_sp))

        # Checkpoint 4 judgment (on |delta_speedup|)
        if abs(delta_sp) > 0.5:
            verdict_sp = f"O1-O4 显著生效 (>0.5x): delta={'+' if delta_sp>0 else ''}{delta_sp:.2f}x"
        elif abs(delta_sp) < 0.3:
            verdict_sp = f"测量波动 (<0.3x): delta={'+' if delta_sp>0 else ''}{delta_sp:.2f}x"
        else:
            verdict_sp = f"中间情况 (0.3-0.5x): delta={'+' if delta_sp>0 else ''}{delta_sp:.2f}x -- 加测"

        # t-test significance on rs_ns (B<A means improvement)
        sig_rs = "显著(B<A)" if (t_rs < 0 and abs(t_rs) > tcrit_05) else \
                 ("显著(B>A)" if (t_rs > 0 and abs(t_rs) > tcrit_05) else "不显著")
        sig_sp = "显著" if abs(t_sp) > tcrit_05 else "不显著"

        print(f"  -> delta_speedup = {'+' if delta_sp>0 else ''}{delta_sp:.2f}x  "
              f"delta_rs_ns = {'+' if delta_rs>0 else ''}{delta_rs:.0f}ns  "
              f"welch_t(rs)={t_rs:.2f} (df={df_rs:.1f}, tcrit_05={tcrit_05:.2f})  "
              f"[{sig_rs}]")
        print(f"  -> Checkpoint 4 判据: {verdict_sp}")
        print()

        summary.append({
            "key": k,
            "A_sp_mean": sA_sp["mean"], "B_sp_mean": sB_sp["mean"],
            "delta_sp": delta_sp, "verdict_sp": verdict_sp,
            "A_rs_mean": sA_rs["mean"], "B_rs_mean": sB_rs["mean"],
            "delta_rs": delta_rs, "welch_t_rs": t_rs, "sig_rs": sig_rs,
        })

    print(f"{'='*72}")
    print("汇总判据:")
    for s in summary:
        print(f"  {s['key']:<22} {s['verdict_sp']}")
    print(f"{'='*72}")


if __name__ == "__main__":
    main()
