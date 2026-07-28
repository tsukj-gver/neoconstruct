"""E01 O1 A/B test 统计分析。读取 6 个 bench JSON，计算描述性统计 + 简单假设检验。"""
import json
import os
import math

HERE = os.path.dirname(os.path.abspath(__file__))

LABELS_A = ["A1_O1off", "A2_O1off", "A3_O1off"]  # O1 off
LABELS_B = ["B1_O1on", "B2_O1on", "B3_O1on"]     # O1 on
SCENARIOS = ["E01", "S01", "i01"]


def load(label):
    path = os.path.join(HERE, f"E01_O1_ab_bench_{label}.json")
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def extract(data, key):
    for r in data["results"]:
        if r["key"] == key:
            return r["speedup"], r["crs_ns"], r["pc_ns"]
    raise KeyError(key)


def mean(xs):
    return sum(xs) / len(xs)


def stdev_sample(xs):
    if len(xs) < 2:
        return float("nan")
    m = mean(xs)
    return math.sqrt(sum((x - m) ** 2 for x in xs) / (len(xs) - 1))


def welch_t(x1, x2):
    """Welch's t-statistic (descriptive only; n=3 too small for strong inference)."""
    m1, m2 = mean(x1), mean(x2)
    s1, s2 = stdev_sample(x1), stdev_sample(x2)
    n1, n2 = len(x1), len(x2)
    se = math.sqrt(s1 ** 2 / n1 + s2 ** 2 / n2)
    if se == 0:
        return float("inf") if m1 != m2 else 0.0
    return (m1 - m2) / se


print("=" * 78)
print("E01 O1 Controlled A/B Test - 统计分析")
print("=" * 78)
print(f"{'场景':<6}{'指标':<14}{'O1 off (A)':<28}{'O1 on (B)':<28}{'Δ(A-B)':<12}")
print("-" * 78)

for sc in SCENARIOS:
    A_sp, A_rs, A_pc = [], [], []
    B_sp, B_rs, B_pc = [], [], []
    for lab in LABELS_A:
        sp, rs, pc = extract(load(lab), sc)
        A_sp.append(sp); A_rs.append(rs); A_pc.append(pc)
    for lab in LABELS_B:
        sp, rs, pc = extract(load(lab), sc)
        B_sp.append(sp); B_rs.append(rs); B_pc.append(pc)

    for metric_name, A, B in [("speedup(x)", A_sp, B_sp),
                              ("rs_ns", A_rs, B_rs),
                              ("pc_ns", A_pc, B_pc)]:
        mA, mB = mean(A), mean(B)
        sA, sB = stdev_sample(A), stdev_sample(B)
        diff = mA - mB
        # pooled within-group noise (proxy for noise floor)
        pooled_noise = math.sqrt((sA ** 2 + sB ** 2) / 2)
        ratio = abs(diff) / pooled_noise if pooled_noise > 0 else float("inf")
        t = welch_t(A, B)
        a_str = f"{mA:.3f} ± {sA:.3f}  [{min(A):.3f},{max(A):.3f}]"
        b_str = f"{mB:.3f} ± {sB:.3f}  [{min(B):.3f},{max(B):.3f}]"
        d_str = f"{diff:+.3f}"
        print(f"{sc:<6}{metric_name:<14}{a_str:<28}{b_str:<28}{d_str:<12}")
        print(f"{'':6}{'':14}{'':28}{'':28}|Δ|/noise={ratio:.2f}  welch_t={t:+.2f}")
    print("-" * 78)

print()
print("判据对照：")
print("  E01 speedup Δ(A-B) = ?  (任务书：>0.5x 回归 / <0.3x 波动 / 0.3-0.5x 加测)")
print("  E01 rs_ns Δ       = ?  (若 O1 真有影响，Rust 侧 ns 应有可观测差异)")
print("  S01 (positive control) ：O1 应使其变快；若 Δ 不显著 → O1 效果本身在噪声内")
print("  i01 (negative control)：N=10 工作量足够大，O1 不应影响；用于检测系统漂移")
