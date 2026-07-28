"""VET 独立复测：E01 Array(0, Index()) parse 稳定性。

按 performance-gate SKILL S-PERF 口径：min(repeat=5) × number=10000。
连续运行 3 次独立采样，观察 E01 是否处于"测量噪声主导区间"（设计 §16.1.6 反例 3）。

4.7 PM 复测：10.56x（07-11，单次）
DEV 4.6 报告：9.44x（07-28，单次）
VET 4.6 复测：9.40x（07-28，单次）
"""

import sys
import timeit
import subprocess
import json

try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
except Exception:
    pass

CRS_PYTHON = r"<opencode-temp>\crs_venv_new\Scripts\python.exe"
PC_PYTHON = r"<opencode-temp>\crs_venv_py_new\Scripts\python.exe"

REPEAT = 5
NUMBER = 10000
ROUNDS = 3

CRS_SETUP = """
import construct as crs
from construct import StructMixin, field
class P(StructMixin):
    items: list = field(crs.Array(0, crs.Index()))
data = bytes(0)
pkt = P
"""

CRS_STMT = "pkt.parse(data)"

PC_SETUP = """
import construct as pc
pkt = pc.Struct("items" / pc.Array(0, pc.Index))
data = bytes(0)
"""

PC_STMT = "pkt.parse(data)"


def measure(python_exe, setup_code, stmt_code):
    full_script = f"""
import timeit
import json

setup = '''{setup_code}'''

exec(setup)

t = timeit.repeat(stmt={stmt_code!r}, setup=setup, number={NUMBER}, repeat={REPEAT})
print(json.dumps({{"min_s": min(t), "number": {NUMBER}}}))
"""
    result = subprocess.run(
        [python_exe, "-c", full_script],
        capture_output=True, text=True, timeout=120
    )
    if result.returncode != 0:
        raise RuntimeError(f"subprocess failed: {result.returncode}\n{result.stderr}")
    lines = result.stdout.strip().split('\n')
    data = json.loads(lines[-1])
    return (data["min_s"] / data["number"]) * 1e9


def main():
    print(f"E01 Array(0, Index()) parse - VET 复测稳定性 ({ROUNDS} rounds)")
    print(f"口径: min(repeat={REPEAT}) x number={NUMBER}, 子进程隔离, apples-to-apples")
    print()

    results = []
    for i in range(1, ROUNDS + 1):
        crs_ns = measure(CRS_PYTHON, CRS_SETUP, CRS_STMT)
        pc_ns = measure(PC_PYTHON, PC_SETUP, PC_STMT)
        speedup = pc_ns / crs_ns
        print(f"Round {i}: crs={crs_ns:.1f}ns  pc={pc_ns:.1f}ns  speedup={speedup:.2f}x {'OK' if speedup >= 10 else 'FAIL'}")
        results.append({"round": i, "crs_ns": crs_ns, "pc_ns": pc_ns, "speedup": speedup})

    speedups = [r["speedup"] for r in results]
    print()
    print(f"3-round summary: min={min(speedups):.2f}x  max={max(speedups):.2f}x  range={max(speedups)-min(speedups):.2f}x")
    print(f"all >=10x: {all(s >= 10 for s in speedups)}")


if __name__ == "__main__":
    main()
