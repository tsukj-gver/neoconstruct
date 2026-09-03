"""bench 公共组件包。

导出公开 API（供 bench/bench_*.py 使用）。
"""

from .report import (
    BenchReport,
    check_gates,
    format_results_table,
    make_environment_dict,
    make_summary,
)
from .runner import ABResult, BenchConfig, BenchResult, BenchRunner
from .stats import ConfidenceInterval, mean, median, speedup_ratio, stdev_sample, welch_t

__all__ = [
    "BenchConfig",
    "BenchResult",
    "ABResult",
    "BenchRunner",
    "BenchReport",
    "check_gates",
    "format_results_table",
    "make_environment_dict",
    "make_summary",
    "mean",
    "median",
    "stdev_sample",
    "welch_t",
    "speedup_ratio",
    "ConfidenceInterval",
]
