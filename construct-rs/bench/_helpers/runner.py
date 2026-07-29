"""bench 统一 runner：场景注册 + 子进程计时 + 统计（设计 §2.5）。

设计依据：
- docs/design/基础设施/测试框架设计.md §2.5
- 复用 benchmark.py 的子进程隔离 + timeit 模式

公开 API：
    - BenchConfig（dataclass）
    - BenchResult（dataclass）+ speedup_over
    - ABResult（dataclass）+ welch_t_speedup
    - BenchRunner（class）

设计原则：
    1. 子进程隔离：与 parity helper 同模式（两个 construct 同名包不能共存）
    2. 多次采样：默认 20 次 iteration 取中位数，降低噪声
    3. Controlled A/B：复用 E01_O1_ab_stats.py 的 Welch t 检验
    4. 报告对齐 perf-scenarios.csv：输出 JSON schema 与 PM 维护的 CSV 字段对齐
"""

from __future__ import annotations

import json
import subprocess
import sys
import timeit
from dataclasses import dataclass, field
from typing import Optional

from .stats import mean, median, speedup_ratio, stdev_sample, welch_t


@dataclass
class BenchConfig:
    """单次 benchmark 配置。

    设计 §5.3 注意点 3：number 默认 = 30000（与原 benchmark.py 一致），
    iterations 默认 = 20（比 REPEAT=5 多，统计更稳健），warmup = 5。
    """
    iterations: int = 20          # 每个场景的测量次数（取中位数）
    warmup: int = 5               # 预热次数（不计入统计）
    number: int = 30_000          # 每次 iteration 内的 timeit number（与 benchmark.py 一致）
    timeout_sec: float = 120.0    # 单次子进程超时


@dataclass
class BenchResult:
    """单次 benchmark 结果（单 impl × 单场景）。"""
    impl: str                     # "rs" 或 "py"
    scenario: str                 # 场景名（如 "B1-parse"）
    samples_ns: list              # 每次 iteration 的 per-call ns（已减 warmup）
    median_ns: float
    mean_ns: float
    stdev_ns: float
    min_ns: float
    max_ns: float

    def speedup_over(self, other):
        """计算相对另一个 impl 的加速比（other.median / self.median）。

        >1 表示 self 更快。
        """
        return speedup_ratio(self.median_ns, other.median_ns)


@dataclass
class ABResult:
    """Controlled A/B Test 结果。

    用途：对比两个优化方案（如 O1 off vs O1 on），用 Welch t 检验判断差异显著性。
    """
    scenario_a: str
    scenario_b: str
    results_a: list               # A 组重复测量（如 O1 off × 3 次）
    results_b: list               # B 组重复测量（如 O1 on × 3 次）

    def welch_t_speedup(self):
        """对 A/B 两组的 speedup 做 Welch t 检验（参考 E01_O1_ab_stats.py）。

        返回 (t_stat, speedups_a, speedups_b)：
            - t_stat: Welch t 统计量（描述性）
            - speedups_a: A 组每次测量的 speedup 列表
            - speedups_b: B 组每次测量的 speedup 列表
        """
        # 每组内 rs vs py 配对算 speedup
        sp_a = [speedup_ratio(r.median_ns, py.median_ns)
                for r, py in zip(self.results_a[::2], self.results_a[1::2])
                if r.impl == "rs" and py.impl == "py"]
        sp_b = [speedup_ratio(r.median_ns, py.median_ns)
                for r, py in zip(self.results_b[::2], self.results_b[1::2])
                if r.impl == "rs" and py.impl == "py"]
        return welch_t(sp_a, sp_b), sp_a, sp_b


class BenchRunner:
    """统一 benchmark runner：场景注册 + 子进程计时 + 统计。

    用途：替代 tests/ 下 5 个 bench 脚本各自实现的"子进程隔离 + timeit + 统计"逻辑。
    """

    def __init__(self, rs_python, py_python, crs_python_dir, config=None):
        """初始化 runner。

        参数：
            rs_python: CRS venv python.exe（impl="rs" 时使用）
            py_python: PC venv python.exe（impl="py" 时使用）
            crs_python_dir: construct-rs/python/ 目录（sys.path 注入用）
            config: BenchConfig 实例，None 用默认值
        """
        self.rs_python = rs_python
        self.py_python = py_python
        self.crs_python_dir = crs_python_dir
        self.config = config if config is not None else BenchConfig()

    def run_scenario(self, impl, scenario_name, measure_script):
        """运行单个场景 × 单 impl，返回 BenchResult。

        参数：
            impl: "rs" 或 "py"
            scenario_name: 场景名（用于报告标识）
            measure_script: 子进程脚本字符串（与 benchmark.py _MEASURE_SCRIPT 同模式），
                必须输出 JSON：{"per_call_ns": [float, ...], ...}

        实现要点：
            1. 子进程内用 timeit.repeat(number=config.number, repeat=config.iterations + config.warmup)
            2. 丢弃前 config.warmup 个 sample
            3. 剩余 sample 转 per_call_ns（每个 sample / number * 1e9）
            4. 计算 median / mean / stdev / min / max

        measure_script 约定：脚本需定义 IMPL / CASE / DIRECTION / NUMBER / REPEAT /
        CRS_PYTHON_DIR 变量（由 runner 通过字符串替换注入），并在末尾 print JSON。
        最简模板见 bench_struct.py 的 _MEASURE_SCRIPT。
        """
        python_exe = self.rs_python if impl == "rs" else self.py_python

        # measure_script 是完整子进程脚本（含变量绑定 + timeit + json.dumps）
        result = subprocess.run(
            [python_exe, "-c", measure_script],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=False,
            timeout=self.config.timeout_sec,
        )
        if result.returncode != 0:
            raise RuntimeError(
                f"bench 子进程失败 impl={impl} scenario={scenario_name}\n"
                f"stderr (tail 2000): {result.stderr[-2000:]}"
            )
        lines = [line for line in result.stdout.strip().splitlines() if line.strip()]
        if not lines:
            raise RuntimeError(
                f"bench 子进程无输出 impl={impl} scenario={scenario_name}\n"
                f"stderr (tail 2000): {result.stderr[-2000:]}"
            )
        try:
            data = json.loads(lines[-1])
        except json.JSONDecodeError as e:
            raise RuntimeError(
                f"bench JSON 解析失败 impl={impl} scenario={scenario_name}\n"
                f"stdout (tail 2000): {result.stdout[-2000:]}\n"
                f"error: {e}"
            )

        samples_ns = data["per_call_ns"]
        return BenchResult(
            impl=impl,
            scenario=scenario_name,
            samples_ns=samples_ns,
            median_ns=median(samples_ns),
            mean_ns=mean(samples_ns),
            stdev_ns=stdev_sample(samples_ns),
            min_ns=min(samples_ns) if samples_ns else 0.0,
            max_ns=max(samples_ns) if samples_ns else 0.0,
        )

    def run_paired(self, scenario_name, measure_script):
        """运行 rs + py 配对场景，返回 dict。

        返回：{"rs": BenchResult, "py": BenchResult, "speedup": float}
            speedup = py.median_ns / rs.median_ns（>1 表示 rs 更快）
        """
        rs_result = self.run_scenario("rs", scenario_name, measure_script)
        py_result = self.run_scenario("py", scenario_name, measure_script)
        return {
            "rs": rs_result,
            "py": py_result,
            "speedup": speedup_ratio(rs_result.median_ns, py_result.median_ns),
        }

    def controlled_ab_test(
        self,
        scenario_a_name,
        scenario_b_name,
        measure_script_a,
        measure_script_b,
        repeats=3,
    ):
        """Controlled A/B Test（复用 E01_O1_ab_stats.py 经验）。

        用途：对比两个优化方案（如 O1 off vs O1 on），用 Welch t 检验判断差异显著性。

        参数：
            scenario_a_name / scenario_b_name: A/B 场景名
            measure_script_a / measure_script_b: A/B 子进程脚本
            repeats: 每组重复测量次数（E01 用 3）
        """
        results_a = []
        results_b = []
        for i in range(repeats):
            results_a.append(self.run_scenario("rs", f"{scenario_a_name}-r{i}", measure_script_a))
            results_b.append(self.run_scenario("rs", f"{scenario_b_name}-r{i}", measure_script_b))
        return ABResult(
            scenario_a=scenario_a_name,
            scenario_b=scenario_b_name,
            results_a=results_a,
            results_b=results_b,
        )
