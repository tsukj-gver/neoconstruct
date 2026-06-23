"""construct-rs vs Python construct 2.10.70 性能基准测试。

设计依据：``docs/架构设计.md`` §G.4（验证方法）。

7 个用例矩阵（B1-B7），每个用例测量 parse 与 build 两个方向，对照：
- **construct-rs**：本项目（Rust 内核 + Python 包层）
- **Python construct 2.10.70**：绝对基线（pip install construct==2.10.70）

由于两个包同名（都叫 ``construct``），无法在同一 Python 进程中同时导入。
本脚本采用 **子进程隔离** 策略：每个 impl 在独立子进程中运行，
通过 JSON 输出结果，主进程合并打印对比表。

用法::

    python tests/benchmark.py

输出：stdout 打印对比表 + 详细日志写 ``tests/benchmark_results.txt``。

通过标准（§G.4.5）：
- B1-B5 加速比 ≥ 4x（硬目标）
- B6 深嵌套 ≥ 2x（已知退化）
- B7 功能性（不要求加速比）
- B1 < 0.5x → 暂停（性能门禁 §2）
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
NUMBER = 30_000
# 重复测量次数（取中位数）
REPEAT = 5

# construct-rs python/ 目录（用于 sys.path 操纵，让 construct-rs 覆盖 site-packages）
_CRS_PYTHON_DIR = str(
    Path(__file__).resolve().parent.parent / "python"
)


# ---------------------------------------------------------------------------
# 子进程测量脚本（每个 impl 在独立进程中运行）
# ---------------------------------------------------------------------------

# 单次测量脚本模板。子进程接收 case 名 + impl + direction，输出 JSON。
_MEASURE_SCRIPT = textwrap.dedent(
    """
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
        # construct-rs 优先：插入 python/ 到最前
        sys.path.insert(0, {crs_python_dir!r})
        # 清除可能预导入的 construct 模块
        for k in list(sys.modules):
            if k == 'construct' or k.startswith('construct.'):
                del sys.modules[k]
    else:
        # Python 原版：使用默认 sys.path（site-packages 优先）
        # 但要确保 construct-rs 不在前面
        crs_dir = {crs_python_dir!r}
        sys.path[:] = [p for p in sys.path if p != crs_dir]
        for k in list(sys.modules):
            if k == 'construct' or k.startswith('construct.'):
                del sys.modules[k]

    # ---- 导入与用例设置 ----
    from dataclasses import dataclass

    if IMPL == 'rs':
        from construct import (
            StructMixin, field,
            Int8ub, Int32ub, GreedyBytes, Bytes,
        )
    else:
        import construct as pc
        Struct = pc.Struct
        # 别名：构造相同的 Int 描述符
        Int8ub = pc.Int8ub
        Int32ub = pc.Int32ub
        GreedyBytes = pc.GreedyBytes
        Bytes = pc.Bytes

    # ---- 用例定义 ----
    def _make_case(case):
        if case == 'B1':
            # ModbusRTUMessage
            if IMPL == 'rs':
                @dataclass
                class Modbus(StructMixin):
                    address: int = field(Int8ub)
                    function_code: int = field(Int8ub)
                    data: bytes = field(GreedyBytes)
                data = bytes([1, 3, 0, 1, 0, 2])
                # parse: Modbus.parse(data)；build: Modbus(...).build()
                parse_target = lambda: Modbus.parse(data)
                build_obj = Modbus(address=1, function_code=3, data=b'\\x00\\x01\\x00\\x02')
                build_target = lambda: build_obj.build()
            else:
                modbus = Struct('address'/Int8ub, 'function_code'/Int8ub, 'data'/GreedyBytes)
                data = bytes([1, 3, 0, 1, 0, 2])
                parse_target = lambda: modbus.parse(data)
                build_dict = dict(address=1, function_code=3, data=b'\\x00\\x01\\x00\\x02')
                build_target = lambda: modbus.build(build_dict)
            return parse_target, build_target

        if case == 'B2':
            return _flat_case(case, n=10)
        if case == 'B3':
            return _flat_case(case, n=50)
        if case == 'B4':
            return _flat_case(case, n=100)
        if case == 'B5':
            return _nested_case(levels=2, fields_per_level=5)
        if case == 'B6':
            return _nested_case(levels=5, fields_per_level=2)
        if case == 'B7':
            return _empty_case()
        raise ValueError('unknown case: ' + case)

    def _flat_case(case, n):
        if IMPL == 'rs':
            attrs = {{'__annotations__': {{}}}}
            annotations = attrs['__annotations__']
            for i in range(n):
                annotations['f{{}}'.format(i)] = int
            # 用 exec 构造 dataclass
            code_lines = ['@dataclass', 'class Flat(StructMixin):']
            for i in range(n):
                code_lines.append('    f{{}}: int = field(Int32ub)'.format(i))
            ns = {{'dataclass': dataclass, 'StructMixin': StructMixin, 'field': field, 'Int32ub': Int32ub}}
            exec('\\n'.join(code_lines), ns)
            Flat = ns['Flat']
            # 数据：4n 字节，每 4 字节一个 Int32ub；值范围 0..255
            data = bytes((i % 256) for i in range(4 * n))
            # build 一个基准对象
            kwargs = {{'f{{}}'.format(i): i for i in range(n)}}
            obj = Flat(**kwargs)
            return (lambda: Flat.parse(data)), (lambda: obj.build())
        else:
            subcons = [('f{{}}'.format(i), Int32ub) for i in range(n)]
            flat = Struct(*[(name / cons) for name, cons in subcons])
            data = bytes((i % 256) for i in range(4 * n))
            build_dict = {{'f{{}}'.format(i): i for i in range(n)}}
            return (lambda: flat.parse(data)), (lambda: flat.build(build_dict))

    def _nested_case(levels, fields_per_level):
        # 由内向外构造：Innermost → ... → Outermost
        if IMPL == 'rs':
            # 最内层
            inner_code = ['@dataclass', 'class L{{}}(StructMixin):'.format(levels)]
            for j in range(fields_per_level):
                inner_code.append('    x{{}}: int = field(Int8ub)'.format(j))
            ns = {{'dataclass': dataclass, 'StructMixin': StructMixin, 'field': field, 'Int8ub': Int8ub}}
            exec('\\n'.join(inner_code), ns)
            current_cls = ns['L{{}}'.format(levels)]
            current_inst_args = {{'x{{}}'.format(j): j + 1 for j in range(fields_per_level)}}
            # 由内向外构造每一层
            for level in range(levels - 1, 0, -1):
                cls_name = 'L{{}}'.format(level)
                code_lines = ['@dataclass', 'class {{}}(StructMixin):'.format(cls_name)]
                # 本层 fields_per_level 个字段
                for j in range(fields_per_level):
                    code_lines.append('    x{{}}: int = field(Int8ub)'.format(j))
                # 一个子节点引用
                code_lines.append('    child: {{}} = field({{}})'.format(current_cls.__name__, current_cls.__name__))
                ns = dict(ns)
                ns[current_cls.__name__] = current_cls
                exec('\\n'.join(code_lines), ns)
                new_cls = ns[cls_name]
                # 构造基准实例
                new_args = {{'x{{}}'.format(j): j + 1 for j in range(fields_per_level)}}
                new_args['child'] = current_cls(**current_inst_args)
                current_cls = new_cls
                current_inst_args = new_args
            outer_cls = current_cls
            outer_inst = current_cls(**current_inst_args)
            # 数据 = 所有字段顺序排列（child 展开）
            total_bytes = levels * fields_per_level
            data = bytes([(i % 256) for i in range(1, total_bytes + 1)])
            return (lambda: outer_cls.parse(data)), (lambda: outer_inst.build())
        else:
            # Python construct：嵌套 Struct
            # 最内层
            inner_subcons = [('x{{}}'.format(j), Int8ub) for j in range(fields_per_level)]
            inner = Struct(*[(n / c) for n, c in inner_subcons])
            current = inner
            for level in range(levels - 1):
                outer_subcons = [('x{{}}'.format(j), Int8ub) for j in range(fields_per_level)]
                outer_subcons.append(('child', current))
                current = Struct(*[(n / c) for n, c in outer_subcons])
            outer_struct = current
            # 构造 build dict（嵌套 Container）
            def make_dict(level=levels):
                if level == 1:
                    return pc.Container(**{{'x{{}}'.format(j): j + 1 for j in range(fields_per_level)}})
                d = {{'x{{}}'.format(j): j + 1 for j in range(fields_per_level)}}
                d['child'] = make_dict(level - 1)
                return pc.Container(**d)
            total_bytes = levels * fields_per_level
            data = bytes([(i % 256) for i in range(1, total_bytes + 1)])
            build_dict = make_dict(levels)
            return (lambda: outer_struct.parse(data)), (lambda: outer_struct.build(build_dict))

    def _empty_case():
        if IMPL == 'rs':
            @dataclass
            class Empty(StructMixin):
                pass
            data = b''
            return (lambda: Empty.parse(data)), (lambda: Empty().build())
        else:
            empty = Struct()
            data = b''
            return (lambda: empty.parse(data)), (lambda: empty.build(pc.Container()))

    # ---- 测量 ----
    parse_target, build_target = _make_case(CASE)
    target = parse_target if DIRECTION == 'parse' else build_target

    # 预热（1 次，让任何惰性初始化生效）
    target()

    # 多次测量取中位数
    timer = timeit.Timer(target)
    times = timer.repeat(repeat=REPEAT, number=NUMBER)

    # 每次调用的纳秒数（中位数）
    per_call_ns = (statistics.median(times) / NUMBER) * 1e9

    # 输出 JSON
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
    :param case: 'B1'..'B7'
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
    # stdout 最后一行是 JSON（可能前面有 print 调试输出，取最后非空行）
    lines = [line for line in result.stdout.strip().splitlines() if line.strip()]
    return json.loads(lines[-1])


# ---------------------------------------------------------------------------
# 主流程
# ---------------------------------------------------------------------------

CASES = ["B1", "B2", "B3", "B4", "B5", "B6", "B7"]
DIRECTIONS = ["parse", "build"]

# 用例描述（用于报告）
CASE_DESCRIPTIONS = {
    "B1": "ModbusRTUMessage (3 字段：Int8ub×2 + GreedyBytes)",
    "B2": "扁平 Struct 10 字段 (Int32ub×10)",
    "B3": "扁平 Struct 50 字段 (Int32ub×50)",
    "B4": "扁平 Struct 100 字段 (Int32ub×100)",
    "B5": "嵌套 Struct 2 层 × 5 字段",
    "B6": "深嵌套 Struct 5 层 × 2 字段",
    "B7": "空 Struct (0 字段)",
}


def run_all_benchmarks() -> dict:
    """运行全部 B1-B7 × {parse, build} × {rs, py} 测量。

    :return: 嵌套字典 {case: {direction: {impl: result_dict}}}
    """
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
        f"{'Python construct (ns)':<22} {'加速比':<10} {'判定':<8}"
    )
    lines.append(header)
    lines.append("-" * len(header))

    for case in CASES:
        for direction in DIRECTIONS:
            rs = results[case][direction]["rs"]["per_call_ns"]
            py = results[case][direction]["py"]["per_call_ns"]
            speedup = py / rs if rs > 0 else float("inf")

            # 判定（使用 ASCII 字符，避免 Windows 控制台编码问题）
            if case in ("B1", "B2", "B3", "B4", "B5"):
                verdict = "[PASS]" if speedup >= 4.0 else "[FAIL]"
            elif case == "B6":
                verdict = "[PASS]" if speedup >= 2.0 else "[FAIL]"
            else:  # B7
                verdict = "[-]"

            lines.append(
                f"{case:<4} {direction:<6} {rs:<18.1f} {py:<22.1f} "
                f"{speedup:<10.2f}x {verdict}"
            )
    return "\n".join(lines)


def check_gates(results: dict) -> list[str]:
    """检查通过标准，返回失败消息列表（空表示全部通过）。"""
    failures = []

    def _speedup(case, direction):
        if case not in results or direction not in results[case]:
            return None
        if "rs" not in results[case][direction] or "py" not in results[case][direction]:
            return None
        rs = results[case][direction]["rs"]["per_call_ns"]
        py = results[case][direction]["py"]["per_call_ns"]
        return py / rs if rs > 0 else 0

    # 烟雾测试：B1 ≥ 0.5x（性能门禁 §2）
    for direction in DIRECTIONS:
        sp = _speedup("B1", direction)
        if sp is not None and sp < 0.5:
            failures.append(
                f"门禁失败：B1 {direction} 加速比 {sp:.2f}x < 0.5x "
                f"（性能门禁 §2，需暂停后续阶段）"
            )

    # 硬目标：B1-B5 ≥ 4x
    for case in ("B1", "B2", "B3", "B4", "B5"):
        for direction in DIRECTIONS:
            sp = _speedup(case, direction)
            if sp is not None and sp < 4.0:
                failures.append(
                    f"硬目标失败：{case} {direction} 加速比 {sp:.2f}x < 4.0x"
                )

    # 深嵌套：B6 ≥ 2x
    for direction in DIRECTIONS:
        sp = _speedup("B6", direction)
        if sp is not None and sp < 2.0:
            failures.append(
                f"深嵌套退化：B6 {direction} 加速比 {sp:.2f}x < 2.0x "
                f"（已知退化，但仍需 ≥ 2x）"
            )

    return failures


def write_results_file(results: dict, table: str, failures: list[str]) -> Path:
    """将完整结果写入 tests/benchmark_results.txt。"""
    out_path = Path(__file__).resolve().parent / "benchmark_results.txt"

    lines = []
    lines.append("=" * 70)
    lines.append("construct-rs vs Python construct 2.10.70 性能基准测试结果")
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
    lines.append("通过标准（§G.4.5）：")
    lines.append("  - B1-B5 ≥ 4x（硬目标）")
    lines.append("  - B6 深嵌套 ≥ 2x（已知退化）")
    lines.append("  - B7 功能性（不要求加速比）")
    lines.append("  - B1 < 0.5x → 暂停（性能门禁 §2）")
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
    parser = argparse.ArgumentParser(description="construct-rs 性能基准测试")
    parser.add_argument(
        "--case",
        action="append",
        help="仅测量指定用例（可多次指定），默认全部 B1-B7",
    )
    args = parser.parse_args()

    global CASES
    if args.case:
        CASES = args.case

    print("=" * 70)
    print("construct-rs vs Python construct 2.10.70 性能基准测试")
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

    # 退出码：0=全部通过，1=有门禁失败
    return 0 if not failures else 1


if __name__ == "__main__":
    sys.exit(main())
