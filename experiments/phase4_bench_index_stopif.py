"""Phase 4 子任务 4.4 性能基准：Index + StopIf。

测量口径（AGENTS.md §6 S-PERF）：
- construct-rs 侧：maturin develop --release 安装后，Python 调用用户面 API。
- Python 侧：直接 import construct，调等效 API。
- 子进程隔离（两边包同名，不可同进程导入）。
- 取 min(repeat=5) × number（默认 1000）。
- 基准：Python construct 2.10.70（绝对基线）。

场景设计：
- I1: Index in Array (Array(N, Index)) parse/build — Index 节点纯读 ctx._index，理论 ≥20x
- I2: Index in Array(Struct) (Array(N, Struct{i: Index, v: Byte})) — Index 在 Struct 内通过继承读 _index
- S1: StopIf(x == 0) parse 不触发 — 表达式求值路径
- S2: StopIf(x == 0) parse 触发 — StopField 捕获路径
- S3: StopIf(True) parse — 常量 Always 路径
"""

import argparse
import json
import os
import statistics
import subprocess
import sys
import time
import timeit
from pathlib import Path


# -----------------------------------------------------------------------------
# Scenario definitions
# -----------------------------------------------------------------------------

# Rust 侧场景代码（construct-rs）
RUST_SCENARIOS = {
    "I1_parse": """
import timeit
from dataclasses import dataclass
from construct import StructMixin, field, rfield, Array, Index

@dataclass
class P(StructMixin):
    items: list = field(Array(N, Index()))

def run():
    P.parse(b'')

t = timeit.repeat(run, number=NUMBER, repeat=REPEAT)
print('TIME_NS', min(t) / NUMBER * 1e9)
""",
    "I1_build": """
import timeit
from dataclasses import dataclass
from construct import StructMixin, field, rfield, Array, Index

@dataclass
class P(StructMixin):
    items: list = field(Array(N, Index()))

p = P.parse(b'')
def run():
    p.build()

t = timeit.repeat(run, number=NUMBER, repeat=REPEAT)
print('TIME_NS', min(t) / NUMBER * 1e9)
""",
    "I2_parse": """
import timeit
from dataclasses import dataclass
from construct import StructMixin, field, rfield, Array, Index, Int8ub

@dataclass
class Item(StructMixin):
    i: int = rfield(Index())
    v: int = field(Int8ub)

@dataclass
class P(StructMixin):
    items: list = field(Array(N, Item))

DATA = bytes(i % 256 for i in range(N))

def run():
    P.parse(DATA)

t = timeit.repeat(run, number=NUMBER, repeat=REPEAT)
print('TIME_NS', min(t) / NUMBER * 1e9)
""",
    "I2_build": """
import timeit
from dataclasses import dataclass
from construct import StructMixin, field, rfield, Array, Index, Int8ub

@dataclass
class Item(StructMixin):
    i: int = rfield(Index())
    v: int = field(Int8ub)

@dataclass
class P(StructMixin):
    items: list = field(Array(N, Item))

DATA = bytes(i % 256 for i in range(N))
p = P.parse(DATA)

def run():
    p.build()

t = timeit.repeat(run, number=NUMBER, repeat=REPEAT)
print('TIME_NS', min(t) / NUMBER * 1e9)
""",
    "S1_parse_no_trigger": """
import timeit
from dataclasses import dataclass
from typing import Any
from construct import StructMixin, field, rfield, Int8ub, StopIf

@dataclass
class S(StructMixin):
    x: int = field(Int8ub)
    stop: Any = rfield(StopIf(x == 0))
    y: int = field(Int8ub)

DATA = b'\\x01\\x02'  # x=1, 不触发 stop

def run():
    S.parse(DATA)

t = timeit.repeat(run, number=NUMBER, repeat=REPEAT)
print('TIME_NS', min(t) / NUMBER * 1e9)
""",
    "S1_build_no_trigger": """
import timeit
from dataclasses import dataclass
from typing import Any
from construct import StructMixin, field, rfield, Int8ub, StopIf

@dataclass
class S(StructMixin):
    x: int = field(Int8ub)
    stop: Any = rfield(StopIf(x == 0))
    y: int = field(Int8ub)

s = S.parse(b'\\x01\\x02')

def run():
    s.build()

t = timeit.repeat(run, number=NUMBER, repeat=REPEAT)
print('TIME_NS', min(t) / NUMBER * 1e9)
""",
    "S2_parse_triggered": """
import timeit
from dataclasses import dataclass
from typing import Any
from construct import StructMixin, field, rfield, Int8ub, StopIf

@dataclass
class S(StructMixin):
    x: int = field(Int8ub)
    stop: Any = rfield(StopIf(x == 0))
    y: int = field(Int8ub)

DATA = b'\\x00'  # x=0, 触发 stop

def run():
    S.parse(DATA)

t = timeit.repeat(run, number=NUMBER, repeat=REPEAT)
print('TIME_NS', min(t) / NUMBER * 1e9)
""",
    "S3_parse_always": """
import timeit
from dataclasses import dataclass
from typing import Any
from construct import StructMixin, field, rfield, Int8ub, StopIf

@dataclass
class S(StructMixin):
    x: int = field(Int8ub)
    stop: Any = rfield(StopIf(True))
    y: int = field(Int8ub)

DATA = b'\\x42'  # Always 触发

def run():
    S.parse(DATA)

t = timeit.repeat(run, number=NUMBER, repeat=REPEAT)
print('TIME_NS', min(t) / NUMBER * 1e9)
""",
}

# Python construct 侧场景代码
PY_SCENARIOS = {
    "I1_parse": """
import timeit
import construct as pycon

d = pycon.Array(N, pycon.Index)

def run():
    d.parse(b'')

t = timeit.repeat(run, number=NUMBER, repeat=REPEAT)
print('TIME_NS', min(t) / NUMBER * 1e9)
""",
    "I1_build": """
import timeit
import construct as pycon

d = pycon.Array(N, pycon.Index)
parsed = d.parse(b'')

def run():
    d.build(parsed)

t = timeit.repeat(run, number=NUMBER, repeat=REPEAT)
print('TIME_NS', min(t) / NUMBER * 1e9)
""",
    "I2_parse": """
import timeit
import construct as pycon

d = pycon.Array(N, pycon.Struct('i' / pycon.Index, 'v' / pycon.Int8ub))
DATA = bytes(i % 256 for i in range(N))

def run():
    d.parse(DATA)

t = timeit.repeat(run, number=NUMBER, repeat=REPEAT)
print('TIME_NS', min(t) / NUMBER * 1e9)
""",
    "I2_build": """
import timeit
import construct as pycon

d = pycon.Array(N, pycon.Struct('i' / pycon.Index, 'v' / pycon.Int8ub))
DATA = bytes(i % 256 for i in range(N))
parsed = d.parse(DATA)

def run():
    d.build(parsed)

t = timeit.repeat(run, number=NUMBER, repeat=REPEAT)
print('TIME_NS', min(t) / NUMBER * 1e9)
""",
    "S1_parse_no_trigger": """
import timeit
import construct as pycon

d = pycon.Struct('x' / pycon.Int8ub, pycon.StopIf(pycon.this.x == 0), 'y' / pycon.Int8ub)
DATA = b'\\x01\\x02'

def run():
    d.parse(DATA)

t = timeit.repeat(run, number=NUMBER, repeat=REPEAT)
print('TIME_NS', min(t) / NUMBER * 1e9)
""",
    "S1_build_no_trigger": """
import timeit
import construct as pycon

d = pycon.Struct('x' / pycon.Int8ub, pycon.StopIf(pycon.this.x == 0), 'y' / pycon.Int8ub)
parsed = d.parse(b'\\x01\\x02')

def run():
    d.build(parsed)

t = timeit.repeat(run, number=NUMBER, repeat=REPEAT)
print('TIME_NS', min(t) / NUMBER * 1e9)
""",
    "S2_parse_triggered": """
import timeit
import construct as pycon

d = pycon.Struct('x' / pycon.Int8ub, pycon.StopIf(pycon.this.x == 0), 'y' / pycon.Int8ub)
DATA = b'\\x00'

def run():
    d.parse(DATA)

t = timeit.repeat(run, number=NUMBER, repeat=REPEAT)
print('TIME_NS', min(t) / NUMBER * 1e9)
""",
    "S3_parse_always": """
import timeit
import construct as pycon

d = pycon.Struct('x' / pycon.Int8ub, pycon.StopIf(True), 'y' / pycon.Int8ub)
DATA = b'\\x42'

def run():
    d.parse(DATA)

t = timeit.repeat(run, number=NUMBER, repeat=REPEAT)
print('TIME_NS', min(t) / NUMBER * 1e9)
""",
}


# 场景的 N / NUMBER 配置（I1/I2 是数组，按规模测试）
SCENARIO_CONFIGS = [
    # (scenario_prefix, N, NUMBER)
    ("I1", 256, 10000),
    ("I1", 4096, 1000),
    ("I2", 256, 10000),
    ("I2", 4096, 1000),
]


def gen_full_scenarios():
    """生成所有场景的 (name, code, N, NUMBER) 元组列表。"""
    scenarios = []
    # I1 / I2 数组场景：parse + build × 多规模
    for prefix, N, NUMBER in SCENARIO_CONFIGS:
        for direction in ("parse", "build"):
            key = f"{prefix}_{direction}"
            scenarios.append((f"{key}_N{N}", key, N, NUMBER))
    # S1 / S2 / S3 单点场景（无 N，固定 NUMBER）
    for key in ("S1_parse_no_trigger", "S1_build_no_trigger",
                "S2_parse_triggered", "S3_parse_always"):
        scenarios.append((key, key, 0, 50000))
    return scenarios


# -----------------------------------------------------------------------------
# 子进程执行
# -----------------------------------------------------------------------------


def run_in_subprocess(python_exe: str, code: str, N: int, NUMBER: int, repeat: int,
                       setup_imports: str = "") -> float:
    """在子进程中执行场景代码，返回 ns/call。"""
    full_code = f"""
REPEAT = {repeat}
NUMBER = {NUMBER}
N = {N}
{setup_imports}
""" + code
    result = subprocess.run(
        [python_exe, "-c", full_code],
        capture_output=True,
        text=True,
        timeout=120,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"subprocess failed (rc={result.returncode}):\n"
            f"stderr: {result.stderr}\n"
            f"stdout: {result.stdout}"
        )
    # 解析 "TIME_NS <float>" 行
    for line in result.stdout.splitlines():
        if line.startswith("TIME_NS"):
            return float(line.split()[1])
    raise RuntimeError(f"no TIME_NS in stdout:\n{result.stdout}\nstderr:\n{result.stderr}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--rust-python",
        default=r"<opencode-temp>\crs_venv2\Scripts\python.exe",
        help="Python 解释器路径（已安装 construct-rs）",
    )
    parser.add_argument(
        "--py-python",
        default=r"<opencode-temp>\crs_venv_py\Scripts\python.exe",
        help="Python 解释器路径（已安装 Python construct）",
    )
    parser.add_argument("--repeat", type=int, default=5)
    parser.add_argument("--output", default=None, help="输出 JSON 报告路径")
    args = parser.parse_args()

    scenarios = gen_full_scenarios()
    results = []

    print(f"{'scenario':<32} {'N':>6} {'Py ns/call':>14} {'Rs ns/call':>14} {'speedup':>10}")
    print("-" * 80)

    for name, key, N, NUMBER in scenarios:
        # Python construct 侧
        try:
            py_ns = run_in_subprocess(
                args.py_python, PY_SCENARIOS[key], N, NUMBER, args.repeat
            )
        except Exception as e:
            print(f"  [PY FAIL] {name}: {e}")
            continue
        # construct-rs 侧
        try:
            rs_ns = run_in_subprocess(
                args.rust_python, RUST_SCENARIOS[key], N, NUMBER, args.repeat
            )
        except Exception as e:
            print(f"  [RS FAIL] {name}: {e}")
            continue

        speedup = py_ns / rs_ns
        flag_4x = "[<4x]" if speedup < 4 else "[OK]"
        print(f"{name:<32} {N:>6} {py_ns:>14.1f} {rs_ns:>14.1f} {speedup:>9.2f}x {flag_4x}")
        results.append({
            "scenario": name,
            "key": key,
            "N": N,
            "NUMBER": NUMBER,
            "py_ns_per_call": py_ns,
            "rs_ns_per_call": rs_ns,
            "speedup": speedup,
        })

    # 派生指标
    print("\n" + "=" * 80)
    print("Derived metrics")
    print("=" * 80)
    # 检查所有场景是否 >= 4x
    below_4x = [r for r in results if r["speedup"] < 4]
    if below_4x:
        print(f"Below 4x ({len(below_4x)}):")
        for r in below_4x:
            print(f"  {r['scenario']}: {r['speedup']:.2f}x")
    else:
        print("All scenarios >= 4x")

    # parse vs build 比率
    print("\nparse/build ratio:")
    for prefix in ("I1", "I2", "S1"):
        parse_rs = [r for r in results if r["key"].startswith(prefix) and "parse" in r["key"]]
        build_rs = [r for r in results if r["key"].startswith(prefix) and "build" in r["key"]]
        if parse_rs and build_rs:
            pr = parse_rs[0]
            br = build_rs[0]
            rs_ratio = pr["rs_ns_per_call"] / br["rs_ns_per_call"] if br["rs_ns_per_call"] else 0
            py_ratio = pr["py_ns_per_call"] / br["py_ns_per_call"] if br["py_ns_per_call"] else 0
            print(f"  {prefix}: Rs parse/build={rs_ratio:.2f}, Py parse/build={py_ratio:.2f}")

    if args.output:
        Path(args.output).parent.mkdir(parents=True, exist_ok=True)
        with open(args.output, "w") as f:
            json.dump(results, f, indent=2)
        print(f"\nReport saved to: {args.output}")


if __name__ == "__main__":
    main()
