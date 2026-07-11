"""4.7 性能验证：Array 系列 lazy path 优化前后对比。

测试 4.x-INVEST 报告中的 5 个目标场景（apples-to-apples 口径）：
  1. Array(100, Index()) parse    — i02
  2. Array(1000, Index()) parse   — i03
  3. Array(4096, Index()) parse   — i04
  4. Array(10, Index()) build     — i01 build
  5. GreedyRange(Int8ub) N=10 parse — g01

测量口径（AGENTS.md §6 S-PERF）：
  - construct-rs 侧：maturin develop --release 安装后，Python 调用户面 API
  - Python construct 侧：import construct，调等效 API
  - 子进程隔离（包同名，不可在同一进程导入）
  - 取 min(repeat=5) × number
  - apples-to-apples：双端都包 Struct

用法：
  & crs_venv_new\Scripts\python.exe experiments\phase4_47_perf_verify.py
"""

import sys
import timeit
import subprocess
import json
import os

try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
except Exception:
    pass

CRS_PYTHON = r"<opencode-temp>\crs_venv_new\Scripts\python.exe"
PC_PYTHON = r"<opencode-temp>\crs_venv_py_new\Scripts\python.exe"

REPEAT = 5


def measure_simple(python_exe, setup_code, stmt_code, number=3000):
    """在子进程中运行 timeit 测量。返回 min ns/call。"""
    # 将 setup_code 中的反斜杠和特殊字符转义，嵌入到 timeit 的 setup 字符串中
    # 使用 exec 方式更安全
    full_script = f"""
import timeit
import json

setup = '''{setup_code}'''

exec(setup)

t = timeit.repeat(stmt={stmt_code!r}, setup=setup, number={number}, repeat={REPEAT})
print(json.dumps({{"min_s": min(t), "number": {number}}}))
"""
    result = subprocess.run(
        [python_exe, "-c", full_script],
        capture_output=True, text=True, timeout=120
    )
    if result.returncode != 0:
        print(f"STDERR: {result.stderr[:500]}", file=sys.stderr)
        raise RuntimeError(f"subprocess failed: {result.returncode}")
    lines = result.stdout.strip().split('\n')
    data = json.loads(lines[-1])
    ns_per_call = (data["min_s"] / data["number"]) * 1e9
    return ns_per_call


def run_scenario(name, crs_setup, crs_stmt, pc_setup, pc_stmt, number=3000):
    """运行一个场景的双端测量。"""
    print(f"\n=== {name} (number={number}) ===", flush=True)
    crs_ns = measure_simple(CRS_PYTHON, crs_setup, crs_stmt, number)
    print(f"  construct-rs: {crs_ns:.1f} ns/call", flush=True)
    pc_ns = measure_simple(PC_PYTHON, pc_setup, pc_stmt, number)
    print(f"  python-construct: {pc_ns:.1f} ns/call", flush=True)
    speedup = pc_ns / crs_ns
    print(f"  加速比: {speedup:.2f}x {'OK >=10x' if speedup >= 10 else 'FAIL <10x'}", flush=True)
    return {"name": name, "crs_ns": crs_ns, "pc_ns": pc_ns, "speedup": speedup}


def main():
    results = []

    # 场景 1: Array(100, Index()) parse — i02
    results.append(run_scenario(
        "i02 Array(100, Index()) parse",
        crs_setup="""
import construct as crs
from construct import StructMixin, field
class P(StructMixin):
    items: list = field(crs.Array(100, crs.Index()))
data = bytes(100)
pkt = P
""",
        crs_stmt="pkt.parse(data)",
        pc_setup="""
import construct as pc
pkt = pc.Struct("items" / pc.Array(100, pc.Index))
data = bytes(100)
""",
        pc_stmt="pkt.parse(data)",
        number=3000,
    ))

    # 场景 2: Array(1000, Index()) parse — i03
    results.append(run_scenario(
        "i03 Array(1000, Index()) parse",
        crs_setup="""
import construct as crs
from construct import StructMixin, field
class P(StructMixin):
    items: list = field(crs.Array(1000, crs.Index()))
data = bytes(1000)
pkt = P
""",
        crs_stmt="pkt.parse(data)",
        pc_setup="""
import construct as pc
pkt = pc.Struct("items" / pc.Array(1000, pc.Index))
data = bytes(1000)
""",
        pc_stmt="pkt.parse(data)",
        number=500,
    ))

    # 场景 3: Array(4096, Index()) parse — i04
    results.append(run_scenario(
        "i04 Array(4096, Index()) parse",
        crs_setup="""
import construct as crs
from construct import StructMixin, field
class P(StructMixin):
    items: list = field(crs.Array(4096, crs.Index()))
data = bytes(4096)
pkt = P
""",
        crs_stmt="pkt.parse(data)",
        pc_setup="""
import construct as pc
pkt = pc.Struct("items" / pc.Array(4096, pc.Index))
data = bytes(4096)
""",
        pc_stmt="pkt.parse(data)",
        number=200,
    ))

    # 场景 4: Array(10, Index()) build — i01 build
    results.append(run_scenario(
        "i01 Array(10, Index()) build",
        crs_setup="""
import construct as crs
from construct import StructMixin, field
class P(StructMixin):
    items: list = field(crs.Array(10, crs.Index()))
obj = P()
obj.items = list(range(10))
""",
        crs_stmt="obj.build()",
        pc_setup="""
import construct as pc
pkt = pc.Struct("items" / pc.Array(10, pc.Index))
obj = pc.Container(items=list(range(10)))
""",
        pc_stmt="pkt.build(obj)",
        number=5000,
    ))

    # 场景 5: GreedyRange(Int8ub) N=10 parse — g01
    results.append(run_scenario(
        "g01 GreedyRange(Int8ub) N=10 parse",
        crs_setup="""
import construct as crs
from construct import StructMixin, field
class P(StructMixin):
    items: list = field(crs.GreedyRange(crs.Int8ub))
data = bytes(10)
pkt = P
""",
        crs_stmt="pkt.parse(data)",
        pc_setup="""
import construct as pc
pkt = pc.Struct("items" / pc.GreedyRange(pc.Int8ub))
data = bytes(10)
""",
        pc_stmt="pkt.parse(data)",
        number=5000,
    ))

    # 额外：验证结构性 <10x 场景（4.x-INVEST 报告预期不达标）
    # 场景 6: Array(10, Index()) parse — i01 parse (structural)
    results.append(run_scenario(
        "i01 Array(10, Index()) parse [structural]",
        crs_setup="""
import construct as crs
from construct import StructMixin, field
class P(StructMixin):
    items: list = field(crs.Array(10, crs.Index()))
data = bytes(10)
pkt = P
""",
        crs_stmt="pkt.parse(data)",
        pc_setup="""
import construct as pc
pkt = pc.Struct("items" / pc.Array(10, pc.Index))
data = bytes(10)
""",
        pc_stmt="pkt.parse(data)",
        number=5000,
    ))

    # 场景 7: Array(10, Int8ub) parse — a01 parse (structural)
    results.append(run_scenario(
        "a01 Array(10, Int8ub) parse [structural]",
        crs_setup="""
import construct as crs
from construct import StructMixin, field
class P(StructMixin):
    items: list = field(crs.Array(10, crs.Int8ub))
data = bytes(10)
pkt = P
""",
        crs_stmt="pkt.parse(data)",
        pc_setup="""
import construct as pc
pkt = pc.Struct("items" / pc.Array(10, pc.Int8ub))
data = bytes(10)
""",
        pc_stmt="pkt.parse(data)",
        number=5000,
    ))

    # 场景 8: Array(0, Index()) parse — e01 parse (structural, zero work)
    results.append(run_scenario(
        "e01 Array(0, Index()) parse [structural]",
        crs_setup="""
import construct as crs
from construct import StructMixin, field
class P(StructMixin):
    items: list = field(crs.Array(0, crs.Index()))
data = bytes(0)
pkt = P
""",
        crs_stmt="pkt.parse(data)",
        pc_setup="""
import construct as pc
pkt = pc.Struct("items" / pc.Array(0, pc.Index))
data = bytes(0)
""",
        pc_stmt="pkt.parse(data)",
        number=10000,
    ))

    # 汇总
    print("\n" + "=" * 70)
    print("性能验证汇总（4.7 lazy path 优化后）")
    print("=" * 70)
    print(f"{'场景':<40} {'Rs(ns)':<12} {'Py(ns)':<12} {'加速比':<10} {'>=10x?'}")
    print("-" * 70)
    for r in results:
        mark = "OK" if r["speedup"] >= 10 else "FAIL"
        print(f"{r['name']:<40} {r['crs_ns']:<12.1f} {r['pc_ns']:<12.1f} {r['speedup']:<10.2f} {mark}")

    # 保存 JSON
    out_path = os.path.join(os.path.dirname(__file__), "phase4_47_perf_results.json")
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(results, f, ensure_ascii=False, indent=2)
    print(f"\n结果已保存到 {out_path}")


if __name__ == "__main__":
    main()
