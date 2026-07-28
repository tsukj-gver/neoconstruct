"""E01 O1 回归根因调查：Controlled A/B Test 测量脚本。

测量 3 个场景（子进程隔离，apples-to-apples）：
  - E01 Array(0, Index()) parse        — 调查目标（O1 是否影响？）
  - S01 StopIf(x==0) x=1 parse          — 阳性对照（O1 应使其变快，确认 O1 toggle 生效）
  - i01 Array(10, Index()) parse        — 阴性对照（N=10 工作量足够大，O1 不应显著影响）

口径：performance-gate SKILL S-PERF
  - min(repeat=5) × number
  - 子进程隔离（双 venv 互不可见）
  - apples-to-apples（双端包 Struct）

用法（由 ARCH A/B test harness 调用）：
  & crs_venv_new\\Scripts\\python.exe experiments\\E01_O1_ab_bench.py <label>

<label> 会写入结果 JSON，用于区分 A 轮（O1 off）/ B 轮（O1 on）。
"""

import sys
import os
import json
import time
import subprocess
import timeit

try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
except Exception:
    pass

CRS_PYTHON = r"<opencode-temp>\crs_venv_new\Scripts\python.exe"
PC_PYTHON = r"<opencode-temp>\crs_venv_py_new\Scripts\python.exe"

REPEAT = 5


def measure(python_exe, setup_code, stmt_code, number, repeat=REPEAT):
    """在子进程中运行 timeit，返回 min ns/call。"""
    full_script = f"""
import timeit
import json

setup = '''{setup_code}'''

exec(setup)

t = timeit.repeat(stmt={stmt_code!r}, setup=setup, number={number}, repeat={repeat})
print(json.dumps({{"min_s": min(t), "number": {number}}}))
"""
    env = dict(os.environ)
    env["PYO3_USE_ABI3_FORWARD_COMPATIBILITY"] = "1"
    result = subprocess.run(
        [python_exe, "-c", full_script],
        capture_output=True, text=True, timeout=180,
        env=env,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"subprocess failed: rc={result.returncode}\nstderr:\n{result.stderr}"
        )
    lines = result.stdout.strip().split('\n')
    data = json.loads(lines[-1])
    return (data["min_s"] / data["number"]) * 1e9


# ============================================================
# 场景定义（apples-to-apples：双端都包 Struct）
# ============================================================
SCENARIOS = [
    # (key, label, number, crs_setup, crs_stmt, pc_setup, pc_stmt)
    (
        "E01", "E01 Array(0, Index()) parse", 10000,
        """
import construct as crs
from construct import StructMixin, field
class P(StructMixin):
    items: list = field(crs.Array(0, crs.Index()))
data = bytes(0)
pkt = P
""",
        "pkt.parse(data)",
        """
import construct as pc
pkt = pc.Struct("items" / pc.Array(0, pc.Index))
data = bytes(0)
""",
        "pkt.parse(data)",
    ),
    (
        "S01", "S01 StopIf(x==0) x=1 parse [positive control]", 5000,
        """
import construct as crs
from construct import StructMixin, field, rfield, Int8ub, StopIf
from dataclasses import dataclass
from typing import Any
@dataclass
class S(StructMixin):
    x: int = field(Int8ub)
    stop: Any = rfield(StopIf(x == 0))
    y: int = field(Int8ub, default=0)
data = b'\\x01\\x02'
pkt = S
""",
        "pkt.parse(data)",
        """
import construct as pc
pkt = pc.Struct('x' / pc.Int8ub, pc.StopIf(pc.this.x == 0), 'y' / pc.Int8ub)
data = b'\\x01\\x02'
""",
        "pkt.parse(data)",
    ),
    (
        "i01", "i01 Array(10, Index()) parse [negative control]", 5000,
        """
import construct as crs
from construct import StructMixin, field
class P(StructMixin):
    items: list = field(crs.Array(10, crs.Index()))
data = bytes(10)
pkt = P
""",
        "pkt.parse(data)",
        """
import construct as pc
pkt = pc.Struct("items" / pc.Array(10, pc.Index))
data = bytes(10)
""",
        "pkt.parse(data)",
    ),
]


def main():
    label = sys.argv[1] if len(sys.argv) > 1 else "unknown"
    ts = time.strftime("%Y-%m-%d %H:%M:%S")

    print(f"\n{'='*70}")
    print(f"A/B bench run | label={label} | timestamp={ts}")
    print(f"{'='*70}", flush=True)

    results = []
    for key, name, number, crs_setup, crs_stmt, pc_setup, pc_stmt in SCENARIOS:
        print(f"\n--- {name} (number={number}) ---", flush=True)
        crs_ns = measure(CRS_PYTHON, crs_setup, crs_stmt, number)
        print(f"  construct-rs: {crs_ns:.1f} ns/call", flush=True)
        pc_ns = measure(PC_PYTHON, pc_setup, pc_stmt, number)
        print(f"  python-construct: {pc_ns:.1f} ns/call", flush=True)
        speedup = pc_ns / crs_ns
        mark = "OK>=10x" if speedup >= 10 else "<10x"
        print(f"  speedup: {speedup:.2f}x  [{mark}]", flush=True)
        results.append({
            "key": key, "name": name, "number": number,
            "crs_ns": crs_ns, "pc_ns": pc_ns, "speedup": speedup,
        })

    out = {
        "label": label, "timestamp": ts,
        "repeat": REPEAT, "results": results,
    }
    out_path = os.path.join(
        os.path.dirname(__file__),
        f"E01_O1_ab_bench_{label}.json",
    )
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(out, f, ensure_ascii=False, indent=2)
    print(f"\n结果已保存到 {out_path}", flush=True)

    # 一行汇总便于日志提取
    parts = [f'{r["key"]}={r["speedup"]:.2f}x(rs={r["crs_ns"]:.0f}ns,pc={r["pc_ns"]:.0f}ns)'
             for r in results]
    print(f"\n[SUMMARY] label={label} | " + " | ".join(parts), flush=True)


if __name__ == "__main__":
    main()
