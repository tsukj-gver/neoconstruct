"""bench 报告生成。

公开 API：
    - BenchReport（dataclass）+ to_json / to_markdown / write
    - check_gates(report, gates)
    - format_results_table(results)
"""

from __future__ import annotations

import json
import platform
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional

from .runner import BenchResult
from .stats import speedup_ratio


@dataclass
class BenchReport:
    """单次 bench 运行的完整报告。"""
    timestamp: str
    environment: dict              # python 版本 / platform / processor
    results: list                  # list[BenchResult]
    summary: dict                  # 汇总：speedup 中位数 / min / max / fail_count

    def to_json(self):
        """序列化为 JSON（与 perf-scenarios.csv 字段对齐）。"""
        return json.dumps({
            "timestamp": self.timestamp,
            "environment": self.environment,
            "results": [
                {
                    "scenario": r.scenario,
                    "impl": r.impl,
                    "median_ns": r.median_ns,
                    "mean_ns": r.mean_ns,
                    "stdev_ns": r.stdev_ns,
                    "min_ns": r.min_ns,
                    "max_ns": r.max_ns,
                    "samples_ns": r.samples_ns,
                }
                for r in self.results
            ],
            "summary": self.summary,
        }, indent=2, ensure_ascii=False)

    def to_markdown(self):
        """序列化为 Markdown 表格（人类可读报告）。"""
        lines = ["# Bench Report", ""]
        lines.append(f"- **Timestamp**: {self.timestamp}")
        lines.append(f"- **Python**: {self.environment.get('python_version', '?')}")
        lines.append(f"- **Platform**: {self.environment.get('platform', '?')}")
        lines.append(f"- **number**: {self.environment.get('number', '?')}, "
                     f"iterations: {self.environment.get('iterations', '?')}")
        lines.append("")
        lines.append("## Results")
        lines.append("")
        lines.append(format_results_table(self.results))
        lines.append("")
        s = self.summary
        lines.append("## Summary")
        lines.append("")
        lines.append(f"- speedup median: {s.get('speedup_median', '?')}")
        lines.append(f"- speedup min: {s.get('speedup_min', '?')}")
        lines.append(f"- speedup max: {s.get('speedup_max', '?')}")
        gates = s.get("failed_gates", [])
        if gates:
            lines.append(f"- failed gates: {gates}")
        else:
            lines.append("- failed gates: none")
        return "\n".join(lines)

    def write(self, path):
        """同时写 JSON + Markdown 到 path（path 不含扩展名）。"""
        path = Path(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.with_suffix(".json").write_text(self.to_json(), encoding="utf-8")
        path.with_suffix(".md").write_text(self.to_markdown(), encoding="utf-8")


def check_gates(report, gates):
    """性能门禁检查。

    参数：
        report: BenchReport 实例
        gates: dict，如 {"B1": 4.0, "B6": 2.0}（case → 最低 speedup）

    返回：失败消息列表（空 = 全部通过）。
    """
    failures = []
    # 按 scenario 分组 rs/py
    by_scenario = {}
    for r in report.results:
        by_scenario.setdefault(r.scenario, {})[r.impl] = r

    for scenario, impls in by_scenario.items():
        if "rs" not in impls or "py" not in impls:
            continue
        # gate 匹配：scenario 形如 "B1-parse"，gate key 是 "B1"
        for gate_key, threshold in gates.items():
            if scenario.startswith(gate_key):
                sp = speedup_ratio(impls["rs"].median_ns, impls["py"].median_ns)
                if sp < threshold:
                    failures.append(
                        f"门禁失败：{scenario} 加速比 {sp:.2f}x < {threshold}x"
                    )
                break
    return failures


def format_results_table(results):
    """格式化对比表。

    列：场景 | impl | median ns | mean ± stdev | min | max | speedup
    """
    lines = []
    header = (
        f"{'场景':<12} {'impl':<5} {'median ns':<12} "
        f"{'mean ± stdev':<22} {'min':<10} {'max':<10} {'speedup':<10}"
    )
    lines.append(header)
    lines.append("-" * len(header))

    # 按 scenario 分组，每组 rs + py 两行
    by_scenario = {}
    for r in results:
        by_scenario.setdefault(r.scenario, {})[r.impl] = r

    for scenario in sorted(by_scenario.keys()):
        impls = by_scenario[scenario]
        for impl in ("rs", "py"):
            if impl not in impls:
                continue
            r = impls[impl]
            speedup_str = ""
            if impl == "rs" and "py" in impls:
                sp = speedup_ratio(r.median_ns, impls["py"].median_ns)
                speedup_str = f"{sp:.2f}x"
            mean_std = f"{r.mean_ns:.1f} ± {r.stdev_ns:.1f}"
            lines.append(
                f"{scenario:<12} {impl:<5} {r.median_ns:<12.1f} "
                f"{mean_std:<22} {r.min_ns:<10.1f} {r.max_ns:<10.1f} "
                f"{speedup_str:<10}"
            )
    return "\n".join(lines)


def make_environment_dict(config):
    """构造环境信息字典（用于 BenchReport）。"""
    return {
        "python_version": sys.version.split()[0],
        "platform": platform.platform(),
        "processor": platform.processor() or "unknown",
        "number": config.number,
        "iterations": config.iterations,
    }


def make_summary(paired_results):
    """从 run_paired 的结果列表构造 summary dict。

    paired_results: list of {"rs": BenchResult, "py": BenchResult, "speedup": float}
    """
    speedups = [p["speedup"] for p in paired_results]
    if not speedups:
        return {"speedup_median": 0, "speedup_min": 0, "speedup_max": 0, "failed_gates": []}
    return {
        "speedup_median": sorted(speedups)[len(speedups) // 2],
        "speedup_min": min(speedups),
        "speedup_max": max(speedups),
        "failed_gates": [],
    }
