"""Phase 5 子任务 5.6a 基线重建：Controlled A/B Test（L-09 对策）.

目的
----
REV v2 设计检视 P2-1 关键发现：ARCH micro-benchmark 测得当前 B1 parse ~265ns，
但 perf-scenarios.csv 记录 Phase 1 B1 rs_ns=386ns。差异 121ns 可能是测量环境漂移
（L-09）或真实改进。本脚本用同会话 Controlled A/B Test 重建 B1 当前真实基线，
确认 B1 是否已达 >=10x。

任务标识：5.6a [基线重建]
状态：DESIGN_REVIEW -> CODING（基线重建是 CODING 前的性能确认）
PM 决策 4（plans/phase5-struct-ffi/总纲.md）

方法
----
- **同会话 Controlled A/B Test**（L-09 教训）：
  - A = Python construct 2.10.70 baseline
  - B = construct-rs 当前实现
  - 每场景内 A->B->A->B->A->B 交替（3 rounds），消除环境漂移
  - 每轮 A 和 B 紧邻测量（<1s 间隔），3 轮间隔 ~30s sleep
- **子进程隔离**：A 和 B 在独立子进程跑（同名 construct 包冲突）
- **S-PERF 口径**：min(repeat=N) x number（performance-gate SKILL Checkpoint 2）
- **统计**：每场景 3 个 speedup 样本 -> mean +/- stddev

场景矩阵
--------
- B1-modbus: ModbusRTU = Int8ub x2 + GreedyBytes (与 perf-scenarios.csv 行 2 对齐, rs~386ns)
- B1-int3  : Int8ub x3 (与 phase5_candidate_a_ab_test.py 对齐, rs~265ns)
- B2       : Int8ub x10 (与 CSV 行 4 对齐)
- B3       : Int8ub x50 (与 CSV 行 6 对齐)
- B4       : Int8ub x100 (与 CSV 行 8 对齐)
- S01      : StopIf(x==0) x=1 不触发 parse (与 CSV 行 146 对齐, O1 后 10.72x)
- S03      : StopIf(True) 常量触发 parse (与 CSV 行 150 对齐, O1 后 10.93x)

用法
----
    python baseline_ab.py
    python baseline_ab.py --quick      # 快速验证模式（小采样）
    python baseline_ab.py --rounds 4   # 自定义轮数

输出
----
- 控制台：逐轮原始数据 + 统计汇总表
- JSON：experiments/phase5_5_6a/baseline_ab_result.json
"""
from __future__ import annotations

import argparse
import datetime
import json
import math
import os
import platform
import statistics
import subprocess
import sys
import time
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

# --------------------------------------------------------------------
# 路径与配置
# --------------------------------------------------------------------
_REPO_ROOT = Path(__file__).resolve().parents[2]
_CRS_PYTHON_DEFAULT = str(
    Path(os.environ.get("LOCALAPPDATA", ""))
    / "Temp" / "opencode" / "crs_venv_new" / "Scripts" / "python.exe"
)
_PC_PYTHON_DEFAULT = str(
    Path(os.environ.get("LOCALAPPDATA", ""))
    / "Temp" / "opencode" / "crs_venv_py_new" / "Scripts" / "python.exe"
)

# 默认采样参数（与 phase5_candidate_a_ab_test.py 同量级，确保 ns 级稳定）
NUMBER_DEFAULT = 100_000       # timeit number（每次测量调用次数）
REPEAT_DEFAULT = 5             # timeit repeat（取 min）
ROUNDS_DEFAULT = 3             # A/B 交替轮数
SLEEP_BETWEEN_ROUNDS = 30      # 轮间 sleep 秒数（防 CPU 热节流）

# 快速验证模式（流程自检用）
NUMBER_QUICK = 5_000
REPEAT_QUICK = 3
ROUNDS_QUICK = 2


# --------------------------------------------------------------------
# 场景定义：每个场景有 crs（construct-rs）和 pc（Python construct）两套 setup+stmt
# --------------------------------------------------------------------
# setup 代码在子进程中 exec，stmt 是 timeit 测量语句（引用 setup 定义的变量）
# 为避免转义问题，setup 和 stmt 分开传递；子进程内 exec(setup) 然后跑 timeit

def _flat_struct_crs(n: int) -> str:
    """构造 n 个 Int8ub 字段的 construct-rs Struct setup 代码。"""
    lines = [
        "from dataclasses import dataclass",
        "from construct import StructMixin, field, Int8ub",
        "",
        "@dataclass",
        "class S(StructMixin):",
    ]
    for i in range(n):
        lines.append(f"    f{i}: int = field(Int8ub)")
    lines.extend([
        "",
        f"DATA = bytes((i % 256) for i in range({n}))",
        "",
    ])
    return "\n".join(lines)


def _flat_struct_pc(n: int) -> str:
    """构造 n 个 Int8ub 字段的 Python construct Struct setup 代码。"""
    fields = ", ".join(f"'f{i}' / pc.Int8ub" for i in range(n))
    return (
        "import construct as pc\n"
        f"d = pc.Struct({fields})\n"
        f"DATA = bytes((i % 256) for i in range({n}))\n"
    )


# 全部场景（顺序即测量顺序）
def build_scenarios() -> List[Dict[str, Any]]:
    """返回场景列表，每场景含 key/name/number/crs_setup/crs_stmt/pc_setup/pc_stmt/note."""
    scenarios: List[Dict[str, Any]] = []

    # B1-modbus: ModbusRTU = Int8ub x2 + GreedyBytes (benchmark.py 同口径)
    scenarios.append({
        "key": "B1-modbus",
        "name": "B1 ModbusRTU (Int8ub x2 + GreedyBytes) parse",
        "number": NUMBER_DEFAULT,
        "crs_setup": (
            "from dataclasses import dataclass\n"
            "from construct import StructMixin, field, Int8ub, GreedyBytes\n"
            "\n"
            "@dataclass\n"
            "class Modbus(StructMixin):\n"
            "    address: int = field(Int8ub)\n"
            "    function_code: int = field(Int8ub)\n"
            "    data: bytes = field(GreedyBytes)\n"
            "\n"
            "DATA = bytes([1, 3, 0, 1, 0, 2])\n"
        ),
        "crs_stmt": "Modbus.parse(DATA)",
        "pc_setup": (
            "import construct as pc\n"
            "d = pc.Struct('address' / pc.Int8ub, 'function_code' / pc.Int8ub, 'data' / pc.GreedyBytes)\n"
            "DATA = bytes([1, 3, 0, 1, 0, 2])\n"
        ),
        "pc_stmt": "d.parse(DATA)",
        "note": "对照 perf-scenarios.csv 行 2 (rs~386ns, 8.09x)",
    })

    # B1-int3: Int8ub x3 纯 Struct (与 candidate_a_ab_test.py 同口径)
    scenarios.append({
        "key": "B1-int3",
        "name": "B1 Int8ub x3 Struct parse (candidate_a 口径)",
        "number": NUMBER_DEFAULT,
        "crs_setup": _flat_struct_crs(3),
        "crs_stmt": "S.parse(DATA)",
        "pc_setup": _flat_struct_pc(3),
        "pc_stmt": "d.parse(DATA)",
        "note": "对照 phase5_candidate_a_ab_result.md (rs~265ns)",
    })

    # B2/B3/B4: Int8ub x{10,50,100}
    for n, csv_row, csv_rs, csv_x in [
        (10, 4, 628, 9.66),
        (50, 6, 2020, 11.54),
        (100, 8, 3656, 12.06),
    ]:
        scenarios.append({
            "key": f"B2-int{n}" if n == 10 else f"B{n//10*3}-int{n}" if False else f"B{n//10+1}-int{n}",
            "name": f"B Int8ub x{n} Struct parse",
            "number": NUMBER_DEFAULT,
            "crs_setup": _flat_struct_crs(n),
            "crs_stmt": "S.parse(DATA)",
            "pc_setup": _flat_struct_pc(n),
            "pc_stmt": "d.parse(DATA)",
            "note": f"对照 perf-scenarios.csv 行 {csv_row} (rs~{csv_rs}ns, {csv_x}x)",
        })

    # 修正 B2/B3/B4 的 key（保持与 CSV 一致的命名）
    scenarios[-3]["key"] = "B2-int10"
    scenarios[-2]["key"] = "B3-int50"
    scenarios[-1]["key"] = "B4-int100"

    # S01: StopIf(x==0) x=1 不触发 parse (4.6 O1 后)
    scenarios.append({
        "key": "S01-stopif-no-trig",
        "name": "S01 StopIf(x==0) x=1 不触发 parse",
        "number": NUMBER_DEFAULT,
        "crs_setup": (
            "from dataclasses import dataclass\n"
            "from typing import Any\n"
            "from construct import StructMixin, field, rfield, Int8ub, StopIf\n"
            "\n"
            "@dataclass\n"
            "class S(StructMixin):\n"
            "    x: int = field(Int8ub)\n"
            "    stop: Any = rfield(StopIf(x == 0))\n"
            "    y: int = field(Int8ub, default=0)\n"
            "\n"
            "DATA = b'\\x01\\x02'\n"
        ),
        "crs_stmt": "S.parse(DATA)",
        "pc_setup": (
            "import construct as pc\n"
            "d = pc.Struct('x' / pc.Int8ub, pc.StopIf(pc.this.x == 0), 'y' / pc.Int8ub)\n"
            "DATA = b'\\x01\\x02'\n"
        ),
        "pc_stmt": "d.parse(DATA)",
        "note": "对照 CSV 行 146 (4.6 O1 VET 10.72x)",
    })

    # S03: StopIf(True) 常量触发 parse (4.6 O1 后)
    scenarios.append({
        "key": "S03-stopif-always",
        "name": "S03 StopIf(True) 常量触发 parse",
        "number": NUMBER_DEFAULT,
        "crs_setup": (
            "from dataclasses import dataclass\n"
            "from typing import Any\n"
            "from construct import StructMixin, field, rfield, Int8ub, StopIf\n"
            "\n"
            "@dataclass\n"
            "class S(StructMixin):\n"
            "    x: int = field(Int8ub)\n"
            "    stop: Any = rfield(StopIf(True))\n"
            "    y: int = field(Int8ub, default=0)\n"
            "\n"
            "DATA = b'\\x42'\n"
        ),
        "crs_stmt": "S.parse(DATA)",
        "pc_setup": (
            "import construct as pc\n"
            "d = pc.Struct('x' / pc.Int8ub, pc.StopIf(True), 'y' / pc.Int8ub)\n"
            "DATA = b'\\x42'\n"
        ),
        "pc_stmt": "d.parse(DATA)",
        "note": "对照 CSV 行 150 (4.6 O1 VET 10.93x)",
    })

    return scenarios


# --------------------------------------------------------------------
# 子进程测量（S-PERF 口径：min(repeat) / number）
# --------------------------------------------------------------------
def measure_subprocess(
    python_exe: str,
    setup_code: str,
    stmt: str,
    number: int,
    repeat: int,
) -> float:
    """在子进程中跑 timeit，返回 min ns/call.

    :param python_exe: python.exe 绝对路径
    :param setup_code: setup 代码（exec 执行，定义 DATA/schema 等变量）
    :param stmt: timeit 测量语句（字符串形式，引用 setup 定义的变量）
    :param number: timeit number
    :param repeat: timeit repeat（取 min）
    :return: min ns/call（float）
    """
    full_script = (
        "import timeit, json, sys\n"
        "setup = " + repr(setup_code) + "\n"
        "exec(setup)\n"
        "t = timeit.repeat(stmt=" + repr(stmt) + ", setup=setup, "
        "number=" + str(number) + ", repeat=" + str(repeat) + ")\n"
        "print(json.dumps({'min_s': min(t), 'number': " + str(number) + "}))\n"
    )
    env = dict(os.environ)
    env["PYO3_USE_ABI3_FORWARD_COMPATIBILITY"] = "1"
    result = subprocess.run(
        [python_exe, "-c", full_script],
        capture_output=True,
        text=True,
        timeout=300,
        env=env,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"subprocess failed (rc={result.returncode})\n"
            f"stderr:\n{result.stderr[:2000]}"
        )
    lines = [ln for ln in result.stdout.strip().splitlines() if ln.strip()]
    if not lines:
        raise RuntimeError(f"empty stdout. stderr:\n{result.stderr[:1000]}")
    data = json.loads(lines[-1])
    return (data["min_s"] / data["number"]) * 1e9


# --------------------------------------------------------------------
# 统计辅助
# --------------------------------------------------------------------
def mean(xs: List[float]) -> float:
    return sum(xs) / len(xs) if xs else float("nan")


def stdev_sample(xs: List[float]) -> float:
    if len(xs) < 2:
        return float("nan")
    return statistics.stdev(xs)


def fmt_mean_std(xs: List[float], fmt: str = ".2f") -> str:
    if not xs:
        return "n/a"
    m = mean(xs)
    s = stdev_sample(xs)
    if math.isnan(s):
        return f"{m:{fmt}}"
    return f"{m:{fmt}} +/- {s:{fmt}}"


# --------------------------------------------------------------------
# 主流程
# --------------------------------------------------------------------
def run_baseline(
    crs_python: str,
    pc_python: str,
    scenarios: List[Dict[str, Any]],
    rounds: int,
    number: int,
    repeat: int,
    sleep_between_rounds: int,
) -> Dict[str, Any]:
    """跑 Controlled A/B Test，返回完整结果 dict."""
    ts = datetime.datetime.now().isoformat(timespec="seconds")
    print(f"\n{'=' * 78}")
    print(f"Phase 5.6a Baseline Controlled A/B Test | timestamp={ts}")
    print(f"  CRS python : {crs_python}")
    print(f"  PC  python : {pc_python}")
    print(f"  scenarios  : {len(scenarios)}")
    print(f"  number     : {number}")
    print(f"  repeat     : {repeat} (take min)")
    print(f"  rounds     : {rounds} (A=pc, B=crs, A->B per round)")
    print(f"  sleep      : {sleep_between_rounds}s between rounds")
    print(f"{'=' * 78}\n", flush=True)

    all_results: List[Dict[str, Any]] = []

    for sc in scenarios:
        key = sc["key"]
        name = sc["name"]
        note = sc.get("note", "")
        sc_number = sc.get("number", number)
        print(f"----- Scenario: {name} -----", flush=True)
        if note:
            print(f"  note: {note}", flush=True)

        a_pcs: List[float] = []   # A = Python construct baseline
        b_crs: List[float] = []   # B = construct-rs
        per_round: List[Dict[str, Any]] = []

        for r in range(1, rounds + 1):
            # A -> B 紧邻交替（每轮内 A 先 B 后）
            t0 = time.time()
            a_ns = measure_subprocess(
                pc_python, sc["pc_setup"], sc["pc_stmt"], sc_number, repeat
            )
            b_ns = measure_subprocess(
                crs_python, sc["crs_setup"], sc["crs_stmt"], sc_number, repeat
            )
            elapsed = time.time() - t0
            sp = a_ns / b_ns if b_ns > 0 else float("inf")
            a_pcs.append(a_ns)
            b_crs.append(b_ns)
            per_round.append({
                "round": r,
                "A_pc_ns": a_ns,
                "B_crs_ns": b_ns,
                "speedup": sp,
                "elapsed_s": elapsed,
            })
            print(
                f"  R{r}: A(pc)={a_ns:>8.1f}ns  B(crs)={b_ns:>8.1f}ns  "
                f"speedup={sp:>6.2f}x  [{elapsed:.1f}s]",
                flush=True,
            )
            if r < rounds and sleep_between_rounds > 0:
                time.sleep(sleep_between_rounds)

        # 统计
        speeds = [pr["speedup"] for pr in per_round]
        sc_result = {
            "key": key,
            "name": name,
            "note": note,
            "number": sc_number,
            "per_round": per_round,
            "stats": {
                "A_pc_ns": {
                    "mean": mean(a_pcs), "stdev": stdev_sample(a_pcs),
                    "min": min(a_pcs), "max": max(a_pcs),
                    "samples": a_pcs,
                },
                "B_crs_ns": {
                    "mean": mean(b_crs), "stdev": stdev_sample(b_crs),
                    "min": min(b_crs), "max": max(b_crs),
                    "samples": b_crs,
                },
                "speedup_x": {
                    "mean": mean(speeds), "stdev": stdev_sample(speeds),
                    "min": min(speeds), "max": max(speeds),
                    "samples": speeds,
                },
            },
        }
        all_results.append(sc_result)
        print(
            f"  SUMMARY: speedup={fmt_mean_std(speeds)}x  "
            f"B(crs)={fmt_mean_std(b_crs)}ns  A(pc)={fmt_mean_std(a_pcs)}ns\n",
            flush=True,
        )

    return {
        "label": "phase5_5_6a_baseline",
        "timestamp": ts,
        "env": {
            "crs_python": crs_python,
            "pc_python": pc_python,
            "crs_python_version": _get_python_version(crs_python),
            "pc_python_version": _get_python_version(pc_python),
            "platform": platform.platform(),
            "processor": platform.processor(),
            "machine": platform.machine(),
            "number": number,
            "repeat": repeat,
            "rounds": rounds,
            "sleep_between_rounds": sleep_between_rounds,
        },
        "scenarios": all_results,
    }


def _get_python_version(python_exe: str) -> str:
    """获取 Python 版本字符串。"""
    try:
        r = subprocess.run(
            [python_exe, "--version"],
            capture_output=True, text=True, timeout=30,
        )
        return r.stdout.strip() or r.stderr.strip()
    except Exception as exc:
        return f"<error: {exc}>"


def write_report(result: Dict[str, Any], output_path: Path) -> None:
    """写 JSON 结果。"""
    output_path.parent.mkdir(parents=True, exist_ok=True)
    with output_path.open("w", encoding="utf-8") as f:
        json.dump(result, f, ensure_ascii=False, indent=2)
    print(f"\n结果 JSON 已保存到: {output_path}", flush=True)


def print_summary_table(result: Dict[str, Any]) -> None:
    """打印汇总对比表。"""
    print(f"\n{'=' * 90}")
    print("Phase 5.6a Baseline Summary (Controlled A/B Test)")
    print(f"{'=' * 90}")
    header = (
        f"{'scenario':<22}{'A(pc) ns':<22}{'B(crs) ns':<22}"
        f"{'speedup x':<18}{'verdict':<10}"
    )
    print(header)
    print("-" * 90)
    for sc in result["scenarios"]:
        st = sc["stats"]
        a_str = f"{st['A_pc_ns']['mean']:.0f} +/- {st['A_pc_ns']['stdev']:.0f}"
        b_str = f"{st['B_crs_ns']['mean']:.0f} +/- {st['B_crs_ns']['stdev']:.0f}"
        sp_mean = st["speedup_x"]["mean"]
        sp_std = st["speedup_x"]["stdev"]
        sp_str = f"{sp_mean:.2f} +/- {sp_std:.2f}"
        if sp_mean >= 10:
            verdict = ">=10x OK"
        elif sp_mean >= 9.7:
            verdict = "~10x NEAR"
        else:
            verdict = "<10x"
        print(f"{sc['key']:<22}{a_str:<22}{b_str:<22}{sp_str:<18}{verdict:<10}")
    print("-" * 90)
    print(f"\nEnvironment:")
    env = result["env"]
    print(f"  crs python : {env['crs_python_version']}  ({env['crs_python']})")
    print(f"  pc  python : {env['pc_python_version']}  ({env['pc_python']})")
    print(f"  platform   : {env['platform']}")
    print(f"  processor  : {env['processor']}")
    print(f"  number={env['number']}, repeat={env['repeat']}, rounds={env['rounds']}")


def main(argv: Optional[List[str]] = None) -> int:
    parser = argparse.ArgumentParser(description="Phase 5.6a baseline Controlled A/B Test")
    parser.add_argument("--crs-python", default=_CRS_PYTHON_DEFAULT,
                        help="construct-rs venv python.exe")
    parser.add_argument("--pc-python", default=_PC_PYTHON_DEFAULT,
                        help="Python construct venv python.exe")
    parser.add_argument("--rounds", type=int, default=ROUNDS_DEFAULT,
                        help=f"A/B 交替轮数 (default {ROUNDS_DEFAULT})")
    parser.add_argument("--number", type=int, default=NUMBER_DEFAULT,
                        help=f"timeit number (default {NUMBER_DEFAULT})")
    parser.add_argument("--repeat", type=int, default=REPEAT_DEFAULT,
                        help=f"timeit repeat (default {REPEAT_DEFAULT})")
    parser.add_argument("--sleep", type=int, default=SLEEP_BETWEEN_ROUNDS,
                        help=f"轮间 sleep 秒数 (default {SLEEP_BETWEEN_ROUNDS})")
    parser.add_argument("--quick", action="store_true",
                        help="快速验证模式（小采样）")
    parser.add_argument("--output", default=None,
                        help="输出 JSON 路径")
    args = parser.parse_args(argv)

    if args.quick:
        number = NUMBER_QUICK
        repeat = REPEAT_QUICK
        rounds = ROUNDS_QUICK
        sleep = 5
    else:
        number = args.number
        repeat = args.repeat
        rounds = args.rounds
        sleep = args.sleep

    # 校验 python.exe 存在
    for py, label in [(args.crs_python, "CRS"), (args.pc_python, "PC")]:
        if not Path(py).exists():
            sys.stderr.write(f"{label} python not found: {py}\n")
            return 2

    scenarios = build_scenarios()

    result = run_baseline(
        crs_python=args.crs_python,
        pc_python=args.pc_python,
        scenarios=scenarios,
        rounds=rounds,
        number=number,
        repeat=repeat,
        sleep_between_rounds=sleep,
    )

    print_summary_table(result)

    out_path = Path(args.output) if args.output else (
        Path(__file__).parent / "baseline_ab_result.json"
    )
    write_report(result, out_path)

    return 0


if __name__ == "__main__":
    sys.exit(main())
