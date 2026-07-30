"""Phase 7 子任务 7.3b parse 方向 Controlled A/B Test 复测（L-09 对策）.

目的
----
Phase 7.3 bench 发现 parse 方向 27/35 不达标（普遍 7-10x）。ARCH 建议先做
Controlled A/B Test 确认稳态（Phase 5.6a 模式——单次测量可能有 ±1.5x 漂移）。

本脚本用同会话 Controlled A/B Test 复测 5 个典型 parse case，确认：
  - 如果复测后 ≥10x → 测量漂移导致原数据低估，parse 方向实际达标
  - 如果复测后仍 <10x → 确认 parse 方向结构性不达标，PM 决策接受（或启动 Phase 8）

任务标识：7.3b [parse 方向 Controlled A/B Test 复测]
触发：plans/phase7-conditional-streams/traces/7.3-集成测试与性能验证.md §决策点 1
方法：plans/phase5-struct-ffi/traces/5.6a-基线重建.md（同模式复用）
教训：harness/experiences.md §L-09（跨时段性能对比消除法归因失效）

方法
----
- **同会话 Controlled A/B Test**（L-09 教训）：
  - A = Python construct 2.10.70 baseline
  - B = construct-rs 当前实现
  - 每场景内 A->B->A->B->A->B 交替（3 rounds），消除环境漂移
  - 每轮 A 和 B 紧邻测量（<1s 间隔），3 轮间隔 30s sleep
- **子进程隔离**：A 和 B 在独立子进程跑（同名 construct 包冲突）
- **S-PERF 口径**：min(repeat=5) x number=100000（performance-gate SKILL Checkpoint 2）
- **统计**：每场景 3 个 speedup 样本 -> mean +/- stddev（样本标准差 n-1）

场景矩阵（5 个典型 parse case，覆盖 4 个 Conditional + 1 个 Streams）
--------------------------------------------------------------------
- ITE1-parse : IfThenElse(True, Int8ub, Pass) parse        (原测 7.73x)
- SW1-parse  : Switch(1, {1: Int8ub, ...}) parse           (原测 7.60x)
- SL1-parse  : Select(Int8ub, Int16ub) parse               (原测 7.52x)
- SK1-parse  : Struct(Seek(5), Bytes(1)) parse             (原测 9.17x)
- PT1-parse  : Struct(Pointer(8, Bytes(1)), Bytes(1)) parse (原测 10.13x，边缘)

阳性对照（已 ≥10x 场景，验证测试基础设施正常）：
- FS1-parse  : FocusedSeq("num", Pass, Renamed("num", Int8ub)) parse (原测 10.89x)

阴性对照（Phase 5 已确认 ≥10x 的非 Phase 7 场景，验证环境漂移幅度）：
- B1-int3    : Int8ub x3 Struct parse (Phase 5.6a: 12.12x)

用法
----
    python parse_ab.py
    python parse_ab.py --quick      # 快速验证模式（小采样）
    python parse_ab.py --rounds 4   # 自定义轮数

输出
----
- 控制台：逐轮原始数据 + 统计汇总表
- JSON：experiments/phase7_3b_parse_ab/parse_ab_result.json
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
from typing import Any, Dict, List, Optional

# --------------------------------------------------------------------
# 路径与配置
# --------------------------------------------------------------------
_CRS_PYTHON_DEFAULT = str(
    Path(os.environ.get("LOCALAPPDATA", ""))
    / "Temp" / "opencode" / "crs_venv_new" / "Scripts" / "python.exe"
)
_PC_PYTHON_DEFAULT = str(
    Path(os.environ.get("LOCALAPPDATA", ""))
    / "Temp" / "opencode" / "crs_venv_py_new" / "Scripts" / "python.exe"
)

# 默认采样参数（与 phase5_5_6a/baseline_ab.py 完全对齐，确保 ns 级稳定）
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
def build_scenarios() -> List[Dict[str, Any]]:
    """返回场景列表，每场景含 key/name/number/crs_setup/crs_stmt/pc_setup/pc_stmt/note.

    场景定义与 ``construct-rs/bench/bench_phase7.py`` 同口径（双端 Struct 包装）。
    """
    scenarios: List[Dict[str, Any]] = []

    # ===== 调查目标 5 个 parse case（原测 7-10x，需复测稳态） =====

    # ITE1-parse: IfThenElse(True, Int8ub, Pass) parse — 原测 7.73x
    scenarios.append({
        "key": "ITE1-parse",
        "name": "ITE1 IfThenElse(True, Int8ub, Pass) parse",
        "number": NUMBER_DEFAULT,
        "crs_setup": (
            "from dataclasses import dataclass\n"
            "from construct import StructMixin, field, Int8ub, Pass, IfThenElse\n"
            "\n"
            "@dataclass\n"
            "class P(StructMixin):\n"
            "    v: int = field(IfThenElse(True, Int8ub, Pass))\n"
            "\n"
            "DATA = b'\\x42'\n"
        ),
        "crs_stmt": "P.parse(DATA)",
        "pc_setup": (
            "import construct as pc\n"
            "d = pc.Struct('v' / pc.IfThenElse(True, pc.Int8ub, pc.Pass))\n"
            "DATA = b'\\x42'\n"
        ),
        "pc_stmt": "d.parse(DATA)",
        "note": "Phase 7.3 原测 7.73x（trace §不达标清单）",
    })

    # SW1-parse: Switch(1, {1: Int8ub, 2: Int16ub, 3: Int32ub}) parse — 原测 7.60x
    scenarios.append({
        "key": "SW1-parse",
        "name": "SW1 Switch(1, {1: Int8ub, ...}) int-key parse",
        "number": NUMBER_DEFAULT,
        "crs_setup": (
            "from dataclasses import dataclass\n"
            "from construct import StructMixin, field, Int8ub, Int16ub, Int32ub, Switch\n"
            "\n"
            "@dataclass\n"
            "class P(StructMixin):\n"
            "    v: int = field(Switch(1, {1: Int8ub, 2: Int16ub, 3: Int32ub}))\n"
            "\n"
            "DATA = b'\\x42'\n"
        ),
        "crs_stmt": "P.parse(DATA)",
        "pc_setup": (
            "import construct as pc\n"
            "d = pc.Struct('v' / pc.Switch(1, {1: pc.Int8ub, 2: pc.Int16ub, 3: pc.Int32ub}))\n"
            "DATA = b'\\x42'\n"
        ),
        "pc_stmt": "d.parse(DATA)",
        "note": "Phase 7.3 原测 7.60x（trace §不达标清单）",
    })

    # SL1-parse: Select(Int8ub, Int16ub) parse — 原测 7.52x
    scenarios.append({
        "key": "SL1-parse",
        "name": "SL1 Select(Int8ub, Int16ub) parse",
        "number": NUMBER_DEFAULT,
        "crs_setup": (
            "from dataclasses import dataclass\n"
            "from construct import StructMixin, field, Int8ub, Int16ub, Select\n"
            "\n"
            "@dataclass\n"
            "class P(StructMixin):\n"
            "    v: int = field(Select(Int8ub, Int16ub))\n"
            "\n"
            "DATA = b'\\x42'\n"
        ),
        "crs_stmt": "P.parse(DATA)",
        "pc_setup": (
            "import construct as pc\n"
            "d = pc.Struct('v' / pc.Select(pc.Int8ub, pc.Int16ub))\n"
            "DATA = b'\\x42'\n"
        ),
        "pc_stmt": "d.parse(DATA)",
        "note": "Phase 7.3 原测 7.52x（trace §不达标清单）",
    })

    # SK1-parse: Struct(Seek(5), Bytes(1)) parse — 原测 9.17x
    scenarios.append({
        "key": "SK1-parse",
        "name": "SK1 Struct(Seek(5), Bytes(1)) parse",
        "number": NUMBER_DEFAULT,
        "crs_setup": (
            "from dataclasses import dataclass\n"
            "from construct import StructMixin, field, Bytes, Seek\n"
            "\n"
            "@dataclass\n"
            "class P(StructMixin):\n"
            "    pos: int = field(Seek(5))\n"
            "    tail: bytes = field(Bytes(1))\n"
            "\n"
            "DATA = b'01234x'\n"
        ),
        "crs_stmt": "P.parse(DATA)",
        "pc_setup": (
            "import construct as pc\n"
            "d = pc.Struct('pos' / pc.Seek(5), 'tail' / pc.Bytes(1))\n"
            "DATA = b'01234x'\n"
        ),
        "pc_stmt": "d.parse(DATA)",
        "note": "Phase 7.3 原测 9.17x（trace §不达标清单）",
    })

    # PT1-parse: Struct(Pointer(8, Bytes(1)), Bytes(1)) parse — 原测 10.13x（边缘）
    scenarios.append({
        "key": "PT1-parse",
        "name": "PT1 Struct(Pointer(8, Bytes(1)), Bytes(1)) parse",
        "number": NUMBER_DEFAULT,
        "crs_setup": (
            "from dataclasses import dataclass\n"
            "from construct import StructMixin, field, Bytes, Pointer\n"
            "\n"
            "@dataclass\n"
            "class P(StructMixin):\n"
            "    ptr: bytes = field(Pointer(8, Bytes(1)))\n"
            "    direct: bytes = field(Bytes(1))\n"
            "\n"
            "DATA = b'0123456789ab'\n"
        ),
        "crs_stmt": "P.parse(DATA)",
        "pc_setup": (
            "import construct as pc\n"
            "d = pc.Struct('ptr' / pc.Pointer(8, pc.Bytes(1)), 'direct' / pc.Bytes(1))\n"
            "DATA = b'0123456789ab'\n"
        ),
        "pc_stmt": "d.parse(DATA)",
        "note": "Phase 7.3 原测 10.13x（trace §边缘达标）",
    })

    # ===== 阳性对照：FS1-parse FocusedSeq（Phase 7.3 已 ≥10x，验证测试基础设施） =====
    scenarios.append({
        "key": "FS1-parse",
        "name": "FS1 FocusedSeq('num', Pass, Renamed('num', Int8ub)) parse [positive]",
        "number": NUMBER_DEFAULT,
        "crs_setup": (
            "from dataclasses import dataclass\n"
            "from construct import (\n"
            "    StructMixin, field, Int8ub, Pass, FocusedSeq, Renamed,\n"
            ")\n"
            "\n"
            "@dataclass\n"
            "class P(StructMixin):\n"
            "    v: int = field(FocusedSeq('num', Pass, Renamed('num', Int8ub)))\n"
            "\n"
            "DATA = b'\\xff'\n"
        ),
        "crs_stmt": "P.parse(DATA)",
        "pc_setup": (
            "import construct as pc\n"
            "d = pc.Struct('v' / pc.FocusedSeq('num', pc.Pass, 'num' / pc.Int8ub))\n"
            "DATA = b'\\xff'\n"
        ),
        "pc_stmt": "d.parse(DATA)",
        "note": "Phase 7.3 原测 10.89x（阳性对照：已 ≥10x，验证测试链正常）",
    })

    # ===== 阴性对照：B1-int3（Phase 5.6a 已 12.12x，验证环境漂移幅度） =====
    scenarios.append({
        "key": "B1-int3",
        "name": "B1 Int8ub x3 Struct parse [negative control]",
        "number": NUMBER_DEFAULT,
        "crs_setup": (
            "from dataclasses import dataclass\n"
            "from construct import StructMixin, field, Int8ub\n"
            "\n"
            "@dataclass\n"
            "class S(StructMixin):\n"
            "    f0: int = field(Int8ub)\n"
            "    f1: int = field(Int8ub)\n"
            "    f2: int = field(Int8ub)\n"
            "\n"
            "DATA = bytes((i % 256) for i in range(3))\n"
        ),
        "crs_stmt": "S.parse(DATA)",
        "pc_setup": (
            "import construct as pc\n"
            "d = pc.Struct('f0' / pc.Int8ub, 'f1' / pc.Int8ub, 'f2' / pc.Int8ub)\n"
            "DATA = bytes((i % 256) for i in range(3))\n"
        ),
        "pc_stmt": "d.parse(DATA)",
        "note": "Phase 5.6a 实测 12.12x（阴性对照：与 Phase 7 无关，验证环境漂移幅度）",
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
    print(f"Phase 7.3b parse Controlled A/B Test | timestamp={ts}")
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
        "label": "phase7_3b_parse_ab",
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
    print(f"\n{'=' * 100}")
    print("Phase 7.3b parse Controlled A/B Test Summary (L-09 对策)")
    print(f"{'=' * 100}")
    header = (
        f"{'scenario':<24}{'A(pc) ns':<22}{'B(crs) ns':<22}"
        f"{'speedup x':<18}{'verdict':<14}"
    )
    print(header)
    print("-" * 100)
    for sc in result["scenarios"]:
        st = sc["stats"]
        a_str = f"{st['A_pc_ns']['mean']:.0f} +/- {st['A_pc_ns']['stdev']:.0f}"
        b_str = f"{st['B_crs_ns']['mean']:.0f} +/- {st['B_crs_ns']['stdev']:.0f}"
        sp_mean = st["speedup_x"]["mean"]
        sp_std = st["speedup_x"]["stdev"]
        sp_str = f"{sp_mean:.2f} +/- {sp_std:.2f}"
        if "[positive]" in sc["name"] or "[negative" in sc["name"]:
            verdict = "control"
        elif sp_mean >= 10:
            verdict = ">=10x OK"
        elif sp_mean >= 9.7:
            verdict = "~10x NEAR"
        else:
            verdict = "<10x"
        print(f"{sc['key']:<24}{a_str:<22}{b_str:<22}{sp_str:<18}{verdict:<14}")
    print("-" * 100)
    print(f"\nEnvironment:")
    env = result["env"]
    print(f"  crs python : {env['crs_python_version']}  ({env['crs_python']})")
    print(f"  pc  python : {env['pc_python_version']}  ({env['pc_python']})")
    print(f"  platform   : {env['platform']}")
    print(f"  processor  : {env['processor']}")
    print(f"  number={env['number']}, repeat={env['repeat']}, rounds={env['rounds']}")


def main(argv: Optional[List[str]] = None) -> int:
    parser = argparse.ArgumentParser(
        description="Phase 7.3b parse Controlled A/B Test"
    )
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
        Path(__file__).parent / "parse_ab_result.json"
    )
    write_report(result, out_path)

    return 0


if __name__ == "__main__":
    sys.exit(main())
