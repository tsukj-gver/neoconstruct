"""construct-rs vs Python construct 2.10.70 表达式场景性能基准测试。

设计依据：``docs/design/模块设计/模块设计-表达式系统.md`` §7.1（验证场景）、§8（性能预测）、
§8.7（性能假设可证伪性）。

3 个表达式用例（E1-E3），每个用例测量 parse 与 build 两个方向，对照：
- **construct-rs**：本项目（Rust 内核 + Python 包层）
- **Python construct 2.10.70**：绝对基线（pip install construct==2.10.70）

由于两个包同名（都叫 ``construct``），无法在同一 Python 进程中同时导入。
本脚本采用 **子进程隔离** 策略：每个 impl 在独立子进程中运行，
通过 JSON 输出结果，主进程合并打印对比表。

用法::

    python tests/benchmark_expr.py

输出：stdout 打印对比表 + 详细日志写 ``tests/benchmark_expr_results.txt``。

通过标准（§8.7）：
- E1-E3 parse 方向 ≥8x（硬目标）
- E1-E3 build 方向 ≥7x（CONFIRM-1 决定）
- 全部 <4x 视为设计失败，暂停后续开发
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import statistics
import subprocess
import sys
import textwrap
import time
from pathlib import Path

# 每次测量的最小调用次数（timeit number）
NUMBER = 100_000
# 重复测量次数（取中位数）
REPEAT = 5

# construct-rs python/ 目录（用于 sys.path 操纵，让 construct-rs 覆盖 site-packages）
_CRS_PYTHON_DIR = str(
    Path(__file__).resolve().parent.parent / "python"
)


# ---------------------------------------------------------------------------
# 子进程测量脚本（每个 impl 在独立进程中运行）
# ---------------------------------------------------------------------------

_MEASURE_SCRIPT = textwrap.dedent(
    """\
    import json
    import statistics
    import sys
    import timeit

    IMPL = {impl!r}
    CASE = {case!r}
    DIRECTION = {direction!r}
    NUMBER = {number!r}
    REPEAT = {repeat!r}

    # ---- sys.path 操纵 ----
    if IMPL == 'rs':
        sys.path.insert(0, {crs_python_dir!r})
        for k in list(sys.modules):
            if k == 'construct' or k.startswith('construct.'):
                del sys.modules[k]
    else:
        crs_dir = {crs_python_dir!r}
        sys.path[:] = [p for p in sys.path if p != crs_dir]
        for k in list(sys.modules):
            if k == 'construct' or k.startswith('construct.'):
                del sys.modules[k]

    # ---- 导入 ----
    from dataclasses import dataclass

    if IMPL == 'rs':
        from construct import (
            StructMixin, field, rfield,
            Int8ub, Int16ub, Bytes, Tell, Computed,
        )
    else:
        import construct as pc
        Struct = pc.Struct
        this = pc.this
        Int8ub = pc.Int8ub
        Int16ub = pc.Int16ub
        Bytes = pc.Bytes
        Tell = pc.Tell
        Computed = pc.Computed

    # ---- 用例定义 ----
    def _make_case(case):
        if case == 'E1':
            # 场景 1：字段引用 + 表达式长度
            if IMPL == 'rs':
                @dataclass
                class Packet(StructMixin):
                    count: int = field(Int8ub)
                    data: bytes = field(Bytes(count))

                data = b'\\x04ABCD'
                parse_target = lambda: Packet.parse(data)
                build_obj = Packet(count=4, data=b'ABCD')
                build_target = lambda: build_obj.build()
            else:
                pkt = Struct('count'/Int8ub, 'data'/Bytes(this.count))
                data = b'\\x04ABCD'
                parse_target = lambda: pkt.parse(data)
                build_dict = dict(count=4, data=b'ABCD')
                build_target = lambda: pkt.build(build_dict)
            return parse_target, build_target

        if case == 'E2':
            # 场景 2：算术表达式
            if IMPL == 'rs':
                @dataclass
                class Packet(StructMixin):
                    count: int = field(Int8ub)
                    flag: int = field(Int8ub)
                    data: bytes = field(Bytes(count + flag))

                data = b'\\x02\\x02ABCD'
                parse_target = lambda: Packet.parse(data)
                build_obj = Packet(count=2, flag=2, data=b'ABCD')
                build_target = lambda: build_obj.build()
            else:
                pkt = Struct(
                    'count'/Int8ub, 'flag'/Int8ub,
                    'data'/Bytes(this.count + this.flag),
                )
                data = b'\\x02\\x02ABCD'
                parse_target = lambda: pkt.parse(data)
                build_dict = dict(count=2, flag=2, data=b'ABCD')
                build_target = lambda: pkt.build(build_dict)
            return parse_target, build_target

        if case == 'E3':
            # 场景 3：Tell + Computed
            if IMPL == 'rs':
                @dataclass
                class Packet(StructMixin):
                    start: int = rfield(Tell())
                    header: bytes = field(Bytes(4))
                    length: int = field(Int16ub)
                    payload: bytes = field(Bytes(length))
                    end: int = rfield(Tell())
                    actual: int = rfield(Computed(end - start))

                data = b'ABCD\\x00\\x04DATA'
                parse_target = lambda: Packet.parse(data)
                build_obj = Packet(header=b'ABCD', length=4, payload=b'DATA')
                build_target = lambda: build_obj.build()
            else:
                pkt = Struct(
                    'start'/Tell,
                    'header'/Bytes(4),
                    'length'/Int16ub,
                    'payload'/Bytes(this.length),
                    'end'/Tell,
                    'actual'/Computed(this.end - this.start),
                )
                data = b'ABCD\\x00\\x04DATA'
                parse_target = lambda: pkt.parse(data)
                build_dict = dict(header=b'ABCD', length=4, payload=b'DATA')
                build_target = lambda: pkt.build(build_dict)
            return parse_target, build_target

        raise ValueError('unknown case: ' + case)

    # ---- 测量 ----
    parse_target, build_target = _make_case(CASE)
    target = parse_target if DIRECTION == 'parse' else build_target

    # 预热（1 次，让任何惰性初始化生效）
    target()

    # 多次测量取中位数
    timer = timeit.Timer(target)
    times = timer.repeat(repeat=REPEAT, number=NUMBER)

    per_call_ns = (statistics.median(times) / NUMBER) * 1e9

    print(json.dumps({{
        'impl': IMPL,
        'case': CASE,
        'direction': DIRECTION,
        'per_call_ns': per_call_ns,
        'median_s': statistics.median(times),
        'min_s': min(times),
        'all_s': times,
    }}))
    """
)


def _run_measurement(impl: str, case: str, direction: str) -> dict:
    """在子进程中运行一次测量。

    :param impl: 'rs' 或 'py'
    :param case: 'E1'..'E3'
    :param direction: 'parse' 或 'build'
    :return: JSON 解析后的结果字典。
    """
    code = _MEASURE_SCRIPT.format(
        impl=impl,
        case=case,
        direction=direction,
        number=NUMBER,
        repeat=REPEAT,
        crs_python_dir=_CRS_PYTHON_DIR,
    )
    result = subprocess.run(
        [sys.executable, "-c", code],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"测量失败 impl={impl} case={case} direction={direction}\n"
            f"stderr: {result.stderr}"
        )
    lines = [line for line in result.stdout.strip().splitlines() if line.strip()]
    return json.loads(lines[-1])


# ---------------------------------------------------------------------------
# 主流程
# ---------------------------------------------------------------------------

CASES = ["E1", "E2", "E3"]
DIRECTIONS = ["parse", "build"]

CASE_DESCRIPTIONS = {
    "E1": "字段引用 + 表达式长度 (count + Bytes(count))",
    "E2": "算术表达式 (count + flag + Bytes(count+flag))",
    "E3": "Tell + Computed (6 字段: Tell, Bytes(4), Int16ub, Bytes(len), Tell, Computed)",
}

# 性能门禁标准（§8.7）
GATE_PARSE = 8.0   # parse 方向硬目标 ≥8x
GATE_BUILD = 7.0   # build 方向 CONFIRM-1 决定 ≥7x
GATE_FAIL = 4.0    # <4x 视为设计失败


def run_all_benchmarks() -> dict:
    """运行全部 E1-E3 × {parse, build} × {rs, py} 测量。"""
    results = {}
    for case in CASES:
        results[case] = {}
        for direction in DIRECTIONS:
            results[case][direction] = {}
            for impl in ["rs", "py"]:
                print(
                    f"  测量 {case} {direction} {impl}...",
                    end="",
                    flush=True,
                )
                r = _run_measurement(impl, case, direction)
                results[case][direction][impl] = r
                print(f" {r['per_call_ns']:.1f} ns/call")
    return results


def format_results_table(results: dict) -> str:
    """格式化结果为对比表。"""
    lines = []
    header = (
        f"{'用例':<4} {'方向':<6} {'construct-rs (ns)':<18} "
        f"{'Python construct (ns)':<22} {'加速比':<10} {'判定':<12}"
    )
    lines.append(header)
    lines.append("-" * len(header))

    for case in CASES:
        for direction in DIRECTIONS:
            rs = results[case][direction]["rs"]["per_call_ns"]
            py = results[case][direction]["py"]["per_call_ns"]
            speedup = py / rs if rs > 0 else float("inf")

            gate = GATE_PARSE if direction == "parse" else GATE_BUILD
            if speedup >= gate:
                verdict = "[PASS]"
            elif speedup >= GATE_FAIL:
                verdict = "[WARN]"
            else:
                verdict = "[FAIL]"

            lines.append(
                f"{case:<4} {direction:<6} {rs:<18.1f} {py:<22.1f} "
                f"{speedup:<10.2f}x {verdict}"
            )
    return "\n".join(lines)


def check_gates(results: dict) -> list[str]:
    """检查通过标准，返回失败消息列表（空表示全部通过）。"""
    failures = []

    def _speedup(case, direction):
        rs = results[case][direction]["rs"]["per_call_ns"]
        py = results[case][direction]["py"]["per_call_ns"]
        return py / rs if rs > 0 else 0

    for case in CASES:
        # parse 方向 ≥8x
        sp = _speedup(case, "parse")
        if sp < GATE_PARSE:
            failures.append(
                f"硬目标失败：{case} parse 加速比 {sp:.2f}x < {GATE_PARSE}x"
            )
        if sp < GATE_FAIL:
            failures.append(
                f"设计失败：{case} parse 加速比 {sp:.2f}x < {GATE_FAIL}x "
                f"（暂停后续开发）"
            )

        # build 方向 ≥7x
        sp_b = _speedup(case, "build")
        if sp_b < GATE_BUILD:
            failures.append(
                f"目标失败：{case} build 加速比 {sp_b:.2f}x < {GATE_BUILD}x"
            )

    return failures


def write_results_file(results: dict, table: str, failures: list[str]) -> Path:
    """将完整结果写入 tests/benchmark_expr_results.txt。"""
    out_path = Path(__file__).resolve().parent / "benchmark_expr_results.txt"

    lines = []
    lines.append("=" * 70)
    lines.append("construct-rs vs Python construct 2.10.70 表达式场景性能测试结果")
    lines.append("=" * 70)
    lines.append("")
    lines.append("环境信息：")
    lines.append(f"  Python 版本    : {sys.version.split()[0]}")
    lines.append(f"  平台           : {platform.platform()}")
    lines.append(f"  处理器         : {platform.processor() or '未知'}")
    lines.append(f"  machine        : {platform.machine()}")
    lines.append(f"  timeit number  : {NUMBER}")
    lines.append(f"  repeat (中位数): {REPEAT}")
    lines.append(f"  测量时间       : {time.strftime('%Y-%m-%d %H:%M:%S')}")
    lines.append("")
    lines.append(f"通过标准（§8.7）：")
    lines.append(f"  - E1-E3 parse ≥{GATE_PARSE}x（硬目标）")
    lines.append(f"  - E1-E3 build ≥{GATE_BUILD}x（CONFIRM-1 决定）")
    lines.append(f"  - <{GATE_FAIL}x 视为设计失败，暂停后续开发")
    lines.append("")
    lines.append("对比表：")
    lines.append(table)
    lines.append("")
    lines.append("用例说明：")
    for case, desc in CASE_DESCRIPTIONS.items():
        lines.append(f"  {case}: {desc}")
    lines.append("")
    lines.append("门禁检查：")
    if failures:
        for f in failures:
            lines.append(f"  [FAIL] {f}")
    else:
        lines.append("  [PASS] 全部通过")
    lines.append("")
    lines.append("原始数据（per_call_ns = 每次调用纳秒数）：")
    for case in CASES:
        for direction in DIRECTIONS:
            for impl in ("rs", "py"):
                r = results[case][direction][impl]
                lines.append(
                    f"  {case} {direction} {impl}: "
                    f"median={r['per_call_ns']:.1f}ns  "
                    f"min={r['min_s'] / NUMBER * 1e9:.1f}ns"
                )
    lines.append("")

    out_path.write_text("\n".join(lines), encoding="utf-8")
    return out_path


def main():
    parser = argparse.ArgumentParser(
        description="construct-rs 表达式场景性能基准测试"
    )
    parser.add_argument(
        "--case",
        action="append",
        help="仅测量指定用例（可多次指定），默认全部 E1-E3",
    )
    args = parser.parse_args()

    global CASES
    if args.case:
        CASES = args.case

    print("=" * 70)
    print("construct-rs vs Python construct 2.10.70 表达式场景性能测试")
    print("=" * 70)
    print(f"Python 版本: {sys.version.split()[0]}")
    print(f"平台       : {platform.platform()}")
    print(f"timeit number={NUMBER}, repeat={REPEAT} (取中位数)")
    print()
    print("开始测量（每个用例 × 方向 × impl 在独立子进程中运行）...")

    results = run_all_benchmarks()

    print()
    print("对比表：")
    table = format_results_table(results)
    print(table)
    print()

    failures = check_gates(results)
    print("门禁检查：")
    if failures:
        for f in failures:
            print(f"  [FAIL] {f}")
    else:
        print("  [PASS] 全部通过")

    out_path = write_results_file(results, table, failures)
    print()
    print(f"详细结果已写入: {out_path}")

    return 0 if not failures else 1


if __name__ == "__main__":
    sys.exit(main())
