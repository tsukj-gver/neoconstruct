"""construct-rs vs Python construct 2.10.70 Struct 性能基准测试（B1-B7）。

组件：bench/_helpers/runner.py（BenchRunner）。

7 个用例（B1-B7），每个测 parse + build，对照 construct-rs vs Python construct 2.10.70。

用法::

    python bench/bench_struct.py
    python bench/bench_struct.py --case B1 --case B2
    python bench/bench_struct.py --iterations 10

通过标准：
    - B1-B5 ≥ 4x（硬目标）
    - B6 深嵌套 ≥ 2x（已知退化）
    - B7 功能性（不要求加速比）
"""

from __future__ import annotations

import argparse
import os
import sys
import textwrap
import time
from pathlib import Path

_BENCH_DIR = Path(__file__).resolve().parent
if str(_BENCH_DIR) not in sys.path:
    sys.path.insert(0, str(_BENCH_DIR))

from _helpers.report import (
    BenchReport,
    check_gates,
    format_results_table,
    make_environment_dict,
    make_summary,
)
from _helpers.runner import BenchConfig, BenchRunner

# ---------------------------------------------------------------------------
# venv 解析（三级，与 conftest.py 一致）
# 现行约定（详见 README「测试环境变量」）：
#   环境变量 CRS_PYTHON / PC_PYTHON > 项目内 .venv / .venv-pc > sys.executable
# ---------------------------------------------------------------------------
_PROJECT_ROOT = _BENCH_DIR.parent  # construct-rs/
_DEFAULT_RS_PYTHON = _PROJECT_ROOT / ".venv"     # construct-rs 扩展 venv
_DEFAULT_PY_PYTHON = _PROJECT_ROOT / ".venv-pc"  # 原版参考 venv（construct==2.10.70）
_CRS_PYTHON_DIR = str(_BENCH_DIR.parent / "python")


def _resolve_venv(env_var, venv_dir):
    """三级解析：环境变量 → 项目内 venv → sys.executable（与 conftest.py 一致）。

    venv_dir 为 venv 根目录，内部按 Windows（Scripts/python.exe）与
    POSIX（bin/python）两种布局探测。
    """
    env = os.environ.get(env_var)
    if env and Path(env).exists():
        return env
    for rel in ("Scripts/python.exe", "bin/python"):
        candidate = Path(venv_dir) / rel
        if candidate.exists():
            return str(candidate)
    return sys.executable


# ---------------------------------------------------------------------------
# 子进程测量脚本模板（测量段输出 per_call_ns list 而非 median，
# REPEAT = warmup + iterations）
# ---------------------------------------------------------------------------
# 模板占位符：{impl!r} {case!r} {direction!r} {number!r} {repeat!r} {crs_python_dir!r}
# 字面花括号用 {{ }} 转义（.format 解析）。
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

    from dataclasses import dataclass

    if IMPL == 'rs':
        from construct import (
            StructMixin, field,
            Int8ub, Int32ub, GreedyBytes, Bytes,
        )
    else:
        import construct as pc
        Struct = pc.Struct
        Int8ub = pc.Int8ub
        Int32ub = pc.Int32ub
        GreedyBytes = pc.GreedyBytes
        Bytes = pc.Bytes

    def _make_case(case):
        if case == 'B1':
            if IMPL == 'rs':
                @dataclass
                class Modbus(StructMixin):
                    address: int = field(Int8ub)
                    function_code: int = field(Int8ub)
                    data: bytes = field(GreedyBytes)
                data = bytes([1, 3, 0, 1, 0, 2])
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
            code_lines = ['@dataclass', 'class Flat(StructMixin):']
            for i in range(n):
                code_lines.append('    f{{}}: int = field(Int32ub)'.format(i))
            ns = {{'dataclass': dataclass, 'StructMixin': StructMixin, 'field': field, 'Int32ub': Int32ub}}
            exec('\\n'.join(code_lines), ns)
            Flat = ns['Flat']
            data = bytes((i % 256) for i in range(4 * n))
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
        if IMPL == 'rs':
            inner_code = ['@dataclass', 'class L{{}}(StructMixin):'.format(levels)]
            for j in range(fields_per_level):
                inner_code.append('    x{{}}: int = field(Int8ub)'.format(j))
            ns = {{'dataclass': dataclass, 'StructMixin': StructMixin, 'field': field, 'Int8ub': Int8ub}}
            exec('\\n'.join(inner_code), ns)
            current_cls = ns['L{{}}'.format(levels)]
            current_inst_args = {{'x{{}}'.format(j): j + 1 for j in range(fields_per_level)}}
            for level in range(levels - 1, 0, -1):
                cls_name = 'L{{}}'.format(level)
                code_lines = ['@dataclass', 'class {{}}(StructMixin):'.format(cls_name)]
                for j in range(fields_per_level):
                    code_lines.append('    x{{}}: int = field(Int8ub)'.format(j))
                code_lines.append('    child: {{}} = field({{}})'.format(current_cls.__name__, current_cls.__name__))
                ns = dict(ns)
                ns[current_cls.__name__] = current_cls
                exec('\\n'.join(code_lines), ns)
                new_cls = ns[cls_name]
                new_args = {{'x{{}}'.format(j): j + 1 for j in range(fields_per_level)}}
                new_args['child'] = current_cls(**current_inst_args)
                current_cls = new_cls
                current_inst_args = new_args
            outer_cls = current_cls
            outer_inst = current_cls(**current_inst_args)
            total_bytes = levels * fields_per_level
            data = bytes([(i % 256) for i in range(1, total_bytes + 1)])
            return (lambda: outer_cls.parse(data)), (lambda: outer_inst.build())
        else:
            inner_subcons = [('x{{}}'.format(j), Int8ub) for j in range(fields_per_level)]
            inner = Struct(*[(n / c) for n, c in inner_subcons])
            current = inner
            for level in range(levels - 1):
                outer_subcons = [('x{{}}'.format(j), Int8ub) for j in range(fields_per_level)]
                outer_subcons.append(('child', current))
                current = Struct(*[(n / c) for n, c in outer_subcons])
            outer_struct = current
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
            obj = Empty()
            return (lambda: Empty.parse(data)), (lambda: obj.build())
        else:
            empty = Struct()
            data = b''
            return (lambda: empty.parse(data)), (lambda: empty.build(pc.Container()))

    parse_target, build_target = _make_case(CASE)
    target = parse_target if DIRECTION == 'parse' else build_target

    # 多次测量（REPEAT = warmup + iterations，runner 丢弃前 warmup 个 sample）
    timer = timeit.Timer(target)
    times = timer.repeat(repeat=REPEAT, number=NUMBER)

    # 每次 iteration 的 per-call ns（list，runner 计算统计）
    per_call_ns = [(t / NUMBER) * 1e9 for t in times]

    print(json.dumps({{
        'impl': IMPL,
        'case': CASE,
        'direction': DIRECTION,
        'per_call_ns': per_call_ns,
    }}))
    """
)


# ---------------------------------------------------------------------------
# 场景元数据
# ---------------------------------------------------------------------------
CASES = ["B1", "B2", "B3", "B4", "B5", "B6", "B7"]
DIRECTIONS = ["parse", "build"]

CASE_DESCRIPTIONS = {
    "B1": "ModbusRTUMessage (3 字段：Int8ub×2 + GreedyBytes)",
    "B2": "扁平 Struct 10 字段 (Int32ub×10)",
    "B3": "扁平 Struct 50 字段 (Int32ub×50)",
    "B4": "扁平 Struct 100 字段 (Int32ub×100)",
    "B5": "嵌套 Struct 2 层 × 5 字段",
    "B6": "深嵌套 Struct 5 层 × 2 字段",
    "B7": "空 Struct (0 字段)",
}

# 性能门禁
GATES = {
    "B1": 4.0, "B2": 4.0, "B3": 4.0, "B4": 4.0, "B5": 4.0,
    "B6": 2.0,
    # B7 无门禁（功能性）
}


def main():
    parser = argparse.ArgumentParser(description="construct-rs Struct 性能基准测试")
    parser.add_argument("--case", action="append", help="仅测量指定用例（可多次指定），默认全部 B1-B7")
    parser.add_argument("--iterations", type=int, default=20, help="每次测量 iteration 数（默认 20）")
    parser.add_argument("--warmup", type=int, default=5, help="预热次数（默认 5）")
    parser.add_argument("--number", type=int, default=30_000, help="timeit number（默认 30000）")
    args = parser.parse_args()

    cases = args.case if args.case else CASES
    config = BenchConfig(
        iterations=args.iterations,
        warmup=args.warmup,
        number=args.number,
    )

    rs_python = _resolve_venv("CRS_PYTHON", _DEFAULT_RS_PYTHON)
    py_python = _resolve_venv("PC_PYTHON", _DEFAULT_PY_PYTHON)

    runner = BenchRunner(rs_python, py_python, _CRS_PYTHON_DIR, config=config)

    print("=" * 70)
    print("construct-rs vs Python construct 2.10.70 Struct 性能基准测试")
    print("=" * 70)
    print(f"Python (rs): {rs_python}")
    print(f"Python (py): {py_python}")
    print(f"number={config.number}, iterations={config.iterations}, warmup={config.warmup}")
    print()

    all_results = []
    paired_for_summary = []
    for case in cases:
        for direction in DIRECTIONS:
            scenario = f"{case}-{direction}"
            repeat = config.iterations + config.warmup
            print(f"  测量 {scenario}...", end="", flush=True)

            rs_script = _MEASURE_SCRIPT.format(
                impl="rs", case=case, direction=direction,
                number=config.number, repeat=repeat, crs_python_dir=_CRS_PYTHON_DIR,
            )
            py_script = _MEASURE_SCRIPT.format(
                impl="py", case=case, direction=direction,
                number=config.number, repeat=repeat, crs_python_dir=_CRS_PYTHON_DIR,
            )

            try:
                rs_result = runner.run_scenario("rs", scenario, rs_script)
                py_result = runner.run_scenario("py", scenario, py_script)
            except RuntimeError as e:
                print(f" [ERROR] {e}")
                return 1

            # runner 内部已丢弃 warmup，这里再确认（run_scenario 返回的 samples_ns 已是 iterations 个）
            all_results.append(rs_result)
            all_results.append(py_result)
            from _helpers.stats import speedup_ratio
            sp = speedup_ratio(rs_result.median_ns, py_result.median_ns)
            paired_for_summary.append({"rs": rs_result, "py": py_result, "speedup": sp})
            print(f" rs={rs_result.median_ns:.1f}ns py={py_result.median_ns:.1f}ns speedup={sp:.2f}x")

    print()
    print("对比表：")
    print(format_results_table(all_results))

    # 构造报告
    report = BenchReport(
        timestamp=time.strftime("%Y-%m-%dT%H:%M:%S"),
        environment=make_environment_dict(config),
        results=all_results,
        summary=make_summary(paired_for_summary),
    )

    # 门禁检查
    failures = check_gates(report, GATES)
    report.summary["failed_gates"] = failures

    print()
    print("门禁检查：")
    if failures:
        for f in failures:
            print(f"  [FAIL] {f}")
    else:
        print("  [PASS] 全部通过")

    # 写报告
    report_path = _BENCH_DIR / "results" / "bench_struct"
    report.write(report_path)
    print(f"\n详细结果已写入: {report_path}.json / .md")

    return 0 if not failures else 1


if __name__ == "__main__":
    sys.exit(main())
