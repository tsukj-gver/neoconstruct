"""bench 统计函数（设计 §2.6）。

设计依据：
- docs/design/基础设施/测试框架设计.md §2.6
- 复用 experiments/E01_O1_ab_stats.py line 26-45 的 mean/stdev_sample/welch_t 实现

公开 API：
    - mean(xs) / stdev_sample(xs) / median(xs)
    - welch_t(x1, x2)
    - speedup_ratio(rs_ns, py_ns)
    - ConfidenceInterval (dataclass + from_samples classmethod)
"""

from __future__ import annotations

import math
import statistics
from dataclasses import dataclass
from typing import Sequence


def mean(xs):
    """算术平均。空序列返回 0.0（不抛异常，便于报告生成）。"""
    if not xs:
        return 0.0
    return sum(xs) / len(xs)


def stdev_sample(xs):
    """样本标准差（n-1 分母）。len < 2 返回 nan。"""
    if len(xs) < 2:
        return float("nan")
    m = mean(xs)
    return math.sqrt(sum((x - m) ** 2 for x in xs) / (len(xs) - 1))


def median(xs):
    """中位数。空序列返回 0.0（不抛异常）。"""
    if not xs:
        return 0.0
    return statistics.median(xs)


def welch_t(x1, x2):
    """Welch's t-statistic（描述性，不查 t 分布表）。

    参考：experiments/E01_O1_ab_stats.py line 37-45。
    若分母 se == 0：返回 inf（m1 != m2）或 0.0（m1 == m2）。
    """
    m1, m2 = mean(x1), mean(x2)
    s1, s2 = stdev_sample(x1), stdev_sample(x2)
    n1, n2 = len(x1), len(x2)
    se = math.sqrt(s1 ** 2 / n1 + s2 ** 2 / n2)
    if se == 0:
        return float("inf") if m1 != m2 else 0.0
    return (m1 - m2) / se


def speedup_ratio(rs_ns, py_ns):
    """加速比 = py_ns / rs_ns。rs_ns == 0 返回 inf。"""
    if rs_ns == 0:
        return float("inf")
    return py_ns / rs_ns


# t 分布双侧临界值（confidence=0.95），按样本量 n 查表。
# 用于 ConfidenceInterval.from_samples 的简易近似（n >= 5 时合理）。
_T_VALUES_95 = {
    5: 2.776, 6: 2.571, 7: 2.447, 8: 2.365, 9: 2.306, 10: 2.262,
    15: 2.145, 20: 2.093, 25: 2.064, 30: 2.045, 40: 2.021, 60: 2.000,
    120: 1.980,
}


def _t_value(n):
    """查 t 临界值（n>=5）。n 不在表中时取最接近的较大 key，n>120 取 1.96（正态近似）。"""
    if n < 5:
        # 小样本不稳健，返回 n=5 的值（保守）
        return _T_VALUES_95[5]
    keys = sorted(_T_VALUES_95.keys())
    for k in keys:
        if n <= k:
            return _T_VALUES_95[k]
    return 1.960  # n > 120，正态近似


@dataclass
class ConfidenceInterval:
    """简易置信区间（基于 t 分布近似，n >= 5 时合理）。"""
    mean: float
    low: float
    high: float
    n: int

    @classmethod
    def from_samples(cls, xs, confidence=0.95):
        """从样本计算置信区间。

        实现：mean ± t_value * stdev_sample / sqrt(n)
        t_value 用查表常量（n=5: 2.776, n=10: 2.262, n=20: 2.093, n=30: 2.045）。
        仅支持 confidence=0.95（其他置信度需扩展 _T_VALUES 表）。
        """
        if confidence != 0.95:
            raise NotImplementedError("仅支持 confidence=0.95，扩展 _T_VALUES_95 后支持其他值")
        n = len(xs)
        if n == 0:
            return cls(mean=0.0, low=0.0, high=0.0, n=0)
        m = mean(xs)
        if n < 2:
            return cls(mean=m, low=m, high=m, n=n)
        sd = stdev_sample(xs)
        t = _t_value(n)
        half = t * sd / math.sqrt(n)
        return cls(mean=m, low=m - half, high=m + half, n=n)
