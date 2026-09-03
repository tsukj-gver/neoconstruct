"""neoconstruct vs Python construct 2.10.70 BitStream 场景性能基准测试。

设计依据：子进程隔离方法与 bench_expr.py 一致。

覆盖范围：BitsInteger / Bitwise+BitStruct / Bytewise/BitsSwapped/ByteSwapped/Padding。
共 10 个场景 × {parse, build} 两个方向。

口径：
    本脚本严格遵循"用户面 API"测量口径：
    - **neoconstruct**：通过 ``maturin develop`` 安装到 venv，子进程中 Python ``timeit``
      调用真实用户面 API（``Packet.parse(data)`` / ``packet.build()``）。包含完整的
      Python 分发 + 单次 FFI 穿越 + Rust 内核执行开销。
    - **Python construct 2.10.70**：``pip install`` 安装到 venv，子进程中 Python ``timeit``
      调用等效 API（``fmt.parse(data)`` / ``fmt.build(obj)``）。

    两个包同名 ``construct``，无法在同一 Python 进程中同时导入。本脚本采用 **子进程隔离**
    策略：每个 impl × 场景 × 方向在独立子进程中运行，通过 JSON 输出结果，主进程合并
    打印对比表。

    **重要**：直接调用 Rust 库的基准（跳过 Python 分发和 FFI 开销）
    与本脚本口径不同；本脚本按用户面 API 口径测量，是性能数据的**权威来源**。

    **公平口径**：Python 侧统一用 ``BitStruct`` / ``Struct`` 包裹（返回
    Container），与 Rust 侧 ``BitStructMixin`` / ``StructMixin`` 的容器创建
    开销对等，避免 Rust 单侧承担 tp_new 容器开销导致对比不公平。

用法::

    python bench/bench_bitstream.py

输出：stdout 打印对比表 + 详细日志写 ``bench/results/benchmark_bitstream_results.txt``。

通过标准：
- BitStruct parse/build 几何平均 ≥10x vs Python construct 2.10.70
- 单点 <4x 视为门禁失败
"""

from __future__ import annotations

import argparse
import json
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

# neoconstruct python/ 目录（用于 sys.path 操纵，让 neoconstruct 覆盖 site-packages）
_CRS_PYTHON_DIR = str(
    Path(__file__).resolve().parent.parent / "python"
)

# Python 解释器（用于跑两个 impl）。默认与主进程相同。
# 子进程隔离保证两个同名包不会冲突。
_PYTHON_EXE = sys.executable


# ---------------------------------------------------------------------------
# 子进程测量脚本（每个 impl 在独立进程中运行）
# ---------------------------------------------------------------------------
#
# 子进程脚本接收 4 个参数：impl, case, direction, number, repeat, crs_python_dir。
# 输出一行 JSON 到 stdout。
#
# 两个 impl 的差异：
# - 'rs'：sys.path 操纵让 neoconstruct/python 优先；用户面 API 是 @dataclass 类
# - 'py'：sys.path 操纵移除 neoconstruct/python，使用 site-packages 的 Python 原版；
#         用户面 API 是 Bitwise/BitStruct/Struct 等构造器
#
# 各场景的用例定义（parse_data 和 build_input）在两个 impl 中保持语义等价。

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
    CRS_PYTHON_DIR = {crs_python_dir!r}

    # ---- sys.path 操纵 ----
    # 关键：两个 impl 必须导入不同的 construct 包，sys.path 必须严格隔离。
    # 默认 sys.path 中 site-packages 在前（含 Python 原版 construct），
    # neoconstruct/python 在后（通过 .pth 添加），需要显式重排。
    if IMPL == 'rs':
        # neoconstruct：强制移到 sys.path 最前（即使已存在也要重排）
        sys.path[:] = [p for p in sys.path if p != CRS_PYTHON_DIR]
        sys.path.insert(0, CRS_PYTHON_DIR)
    else:
        # Python construct：确保 neoconstruct/python 不在 sys.path 中
        sys.path[:] = [p for p in sys.path if p != CRS_PYTHON_DIR]
    # 清除已缓存的 construct 模块（防止父进程预导入的影响）
    for k in list(sys.modules):
        if k == 'construct' or k.startswith('construct.'):
            del sys.modules[k]

    # ---- 导入 ----
    if IMPL == 'rs':
        from dataclasses import dataclass
        from neoconstruct import (
            StructMixin, BitStructMixin, field, wfield,
            BitsInteger, Bit, Nibble, Octet,
            Bitwise, Bytewise, BitsSwapped, ByteSwapped, Padding,
            Bytes, Int16ub, Int32ub,
        )
    else:
        import construct as pc
        Bitwise = pc.Bitwise
        BitsInteger = pc.BitsInteger
        BitStruct = pc.BitStruct
        Padding = pc.Padding
        Bytes = pc.Bytes
        Bytewise = pc.Bytewise
        ByteSwapped = pc.ByteSwapped
        BitsSwapped = pc.BitsSwapped
        Struct = pc.Struct
        Nibble = pc.Nibble
        Bit = pc.Bit
        Octet = pc.Octet
        Int16ub = pc.Int16ub
        Int32ub = pc.Int32ub

    # ---- 用例定义 ----
    def _make_case(case):
        # 每个用例返回 (parse_target, build_target)
        # parse_target / build_target 是无参 callable，分别是 parse 和 build 的测量目标
        if IMPL == 'rs':
            return _make_case_rs(case)
        else:
            return _make_case_py(case)

    def _make_case_rs(case):
        if case == 'B1':
            # Bitwise(BitsInteger(8))
            @dataclass
            class P(BitStructMixin):
                v: int = field(BitsInteger(8))
            parse_data = b"\\xA5"
            parse_target = lambda: P.parse(parse_data)
            obj = P(v=0xA5)
            build_target = lambda: obj.build()
            return parse_target, build_target

        if case == 'B2':
            # Bitwise(BitsInteger(16))
            @dataclass
            class P(BitStructMixin):
                v: int = field(BitsInteger(16))
            parse_data = b"\\xA5\\x3C"
            parse_target = lambda: P.parse(parse_data)
            obj = P(v=0xA53C)
            build_target = lambda: obj.build()
            return parse_target, build_target

        if case == 'B3':
            # Bitwise(BitsInteger(32))
            @dataclass
            class P(BitStructMixin):
                v: int = field(BitsInteger(32))
            parse_data = b"\\xA5\\x3C\\x96\\xC3"
            parse_target = lambda: P.parse(parse_data)
            obj = P(v=0xA53C96C3)
            build_target = lambda: obj.build()
            return parse_target, build_target

        if case == 'B4':
            # Bitwise(BitsInteger(8, signed=True))
            @dataclass
            class P(BitStructMixin):
                v: int = field(BitsInteger(8, signed=True))
            parse_data = b"\\xA5"
            parse_target = lambda: P.parse(parse_data)
            obj = P(v=-91)
            build_target = lambda: obj.build()
            return parse_target, build_target

        if case == 'BS1':
            # BitStruct(Nibble, BitsInteger(10), Padding(2))
            @dataclass
            class P(BitStructMixin):
                a: int = field(Nibble())
                b: int = field(BitsInteger(10))
                c: int = wfield(Padding(2), default=None)
            parse_data = b"\\xBE\\xEF"
            parse_target = lambda: P.parse(parse_data)
            obj = P(a=0xB, b=0x3BB)
            build_target = lambda: obj.build()
            return parse_target, build_target

        if case == 'BS2':
            # BitStruct(Bit, Nibble, Octet, Padding(3))
            @dataclass
            class P(BitStructMixin):
                a: int = field(Bit())
                b: int = field(Nibble())
                c: int = field(Octet())
                d: int = wfield(Padding(3), default=None)
            parse_data = b"\\xAB\\xCD"
            parse_target = lambda: P.parse(parse_data)
            obj = P(a=1, b=0x5, c=0x79)
            build_target = lambda: obj.build()
            return parse_target, build_target

        if case == 'BW1':
            # BitStruct(Nibble, Bytewise(Int16ub), Nibble)
            @dataclass
            class P(BitStructMixin):
                a: int = field(Nibble())
                b: int = field(Bytewise(Int16ub))
                c: int = field(Nibble())
            parse_data = b"\\xA1\\x23\\x4B"
            parse_target = lambda: P.parse(parse_data)
            obj = P(a=0xA, b=0x1234, c=0xB)
            build_target = lambda: obj.build()
            return parse_target, build_target

        if case == 'BW2':
            # BitsSwapped(Bytes(4))
            @dataclass
            class P(StructMixin):
                v: bytes = field(BitsSwapped(Bytes(4)))
            parse_data = b"\\xF0\\x0F\\xAA\\x55"
            parse_target = lambda: P.parse(parse_data)
            obj = P(v=b"\\x0F\\xF0\\x55\\xAA")
            build_target = lambda: obj.build()
            return parse_target, build_target

        if case == 'BW3':
            # ByteSwapped(Int32ub)
            @dataclass
            class P(StructMixin):
                v: int = field(ByteSwapped(Int32ub))
            parse_data = b"\\x78\\x56\\x34\\x12"
            parse_target = lambda: P.parse(parse_data)
            obj = P(v=0x12345678)
            build_target = lambda: obj.build()
            return parse_target, build_target

        if case == 'BW4':
            # Struct(Bytes(1), Padding(4), Bytes(2))
            @dataclass
            class P(StructMixin):
                tag: bytes = field(Bytes(1))
                reserved: int = wfield(Padding(4), default=None)
                data: bytes = field(Bytes(2))
            parse_data = b"\\xAA\\x00\\x00\\x00\\x00\\xBB\\xCC"
            parse_target = lambda: P.parse(parse_data)
            obj = P(tag=b"\\xAA", data=b"\\xBB\\xCC")
            build_target = lambda: obj.build()
            return parse_target, build_target

        raise ValueError('unknown case: ' + case)

    def _make_case_py(case):
        # 公平口径：
        # 每个 case 的 Python 用例必须与 Rust 用例产生**对等**的容器/实例开销。
        # - Rust B1-B4 / BS1-BS2 / BW1 使用 BitStructMixin → 实例创建 + 单字段填充
        # - Rust BW2-BW4 使用 StructMixin → 实例创建 + 单字段填充
        # 因此 Python 侧也必须使用 BitStruct / Struct 包裹，返回 Container
        # （而非裸 int/bytes）。否则对比是不公平的：Rust 多了 tp_new 开销。
        if case == 'B1':
            fmt = BitStruct("v"/BitsInteger(8))
            parse_data = b"\\xA5"
            build_input = dict(v=0xA5)
        elif case == 'B2':
            fmt = BitStruct("v"/BitsInteger(16))
            parse_data = b"\\xA5\\x3C"
            build_input = dict(v=0xA53C)
        elif case == 'B3':
            fmt = BitStruct("v"/BitsInteger(32))
            parse_data = b"\\xA5\\x3C\\x96\\xC3"
            build_input = dict(v=0xA53C96C3)
        elif case == 'B4':
            fmt = BitStruct("v"/BitsInteger(8, signed=True))
            parse_data = b"\\xA5"
            build_input = dict(v=-91)
        elif case == 'BS1':
            fmt = BitStruct("a"/Nibble, "b"/BitsInteger(10), "c"/Padding(2))
            parse_data = b"\\xBE\\xEF"
            build_input = dict(a=0xB, b=0x3BB)
        elif case == 'BS2':
            fmt = BitStruct("a"/Bit, "b"/Nibble, "c"/Octet, "d"/Padding(3))
            parse_data = b"\\xAB\\xCD"
            build_input = dict(a=1, b=0x5, c=0x79)
        elif case == 'BW1':
            fmt = BitStruct("a"/Nibble, "b"/Bytewise(Int16ub), "c"/Nibble)
            parse_data = b"\\xA1\\x23\\x4B"
            build_input = dict(a=0xA, b=0x1234, c=0xB)
        elif case == 'BW2':
            # Python 侧用 Struct 包裹，与 Rust 侧 StructMixin + BitsSwapped(Bytes(4)) 对等
            fmt = Struct("v"/BitsSwapped(Bytes(4)))
            parse_data = b"\\xF0\\x0F\\xAA\\x55"
            build_input = dict(v=b"\\x0F\\xF0\\x55\\xAA")
        elif case == 'BW3':
            # Python 侧用 Struct 包裹，与 Rust 侧 StructMixin + ByteSwapped(Int32ub) 对等
            fmt = Struct("v"/ByteSwapped(Int32ub))
            parse_data = b"\\x78\\x56\\x34\\x12"
            build_input = dict(v=0x12345678)
        elif case == 'BW4':
            fmt = Struct("tag"/Bytes(1), "pad"/Padding(4), "data"/Bytes(2))
            parse_data = b"\\xAA\\x00\\x00\\x00\\x00\\xBB\\xCC"
            build_input = dict(tag=b"\\xAA", data=b"\\xBB\\xCC")
        else:
            raise ValueError('unknown case: ' + case)

        parse_target = lambda: fmt.parse(parse_data)
        build_target = lambda: fmt.build(build_input)
        return parse_target, build_target

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
    :param case: 场景名（'B1'..'BW4'）
    :param direction: 'parse' 或 'build'
    :return: JSON 解析后的结果字典。
    :raises RuntimeError: 子进程失败或输出无法解析。
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
        [_PYTHON_EXE, "-c", code],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"测量失败 impl={impl} case={case} direction={direction}\n"
            f"stderr: {result.stderr[-2000:]}"
        )
    lines = [line for line in result.stdout.strip().splitlines() if line.strip()]
    if not lines:
        raise RuntimeError(
            f"测量无输出 impl={impl} case={case} direction={direction}\n"
            f"stderr: {result.stderr[-2000:]}"
        )
    try:
        return json.loads(lines[-1])
    except json.JSONDecodeError as e:
        raise RuntimeError(
            f"无法解析 JSON 输出 impl={impl} case={case} direction={direction}\n"
            f"stdout: {result.stdout[-2000:]}\n"
            f"error: {e}"
        )


# ---------------------------------------------------------------------------
# 场景定义
# ---------------------------------------------------------------------------

CASES = [
    # BitsInteger（在 Bitwise 域内，用 BitStruct 包裹以与 Rust BitStructMixin 对等）
    ("B1", "BitsInteger", "BitStruct(v=BitsInteger(8))"),
    ("B2", "BitsInteger", "BitStruct(v=BitsInteger(16))"),
    ("B3", "BitsInteger", "BitStruct(v=BitsInteger(32))"),
    ("B4", "BitsInteger", "BitStruct(v=BitsInteger(8, signed=True))"),
    # Bitwise + BitStruct
    ("BS1", "BitStruct", "BitStruct(Nibble, BitsInteger(10), Padding(2))"),
    ("BS2", "BitStruct", "BitStruct(Bit, Nibble, Octet, Padding(3))"),
    # Bytewise / BitsSwapped / ByteSwapped / Padding
    ("BW1", "ByteSwap", "BitStruct(Nibble, Bytewise(Int16ub), Nibble)"),
    ("BW2", "ByteSwap", "Struct(v=BitsSwapped(Bytes(4)))"),
    ("BW3", "ByteSwap", "Struct(v=ByteSwapped(Int32ub))"),
    ("BW4", "ByteSwap", "Struct(Bytes(1), Padding(4), Bytes(2))"),
]

DIRECTIONS = ["parse", "build"]

# 场景详情（用于结果文件）
CASE_DETAILS = {
    "B1": "BitStruct(v=BitsInteger(8)) parse/build — 8-bit unsigned via Bitwise",
    "B2": "BitStruct(v=BitsInteger(16)) parse/build — 16-bit unsigned via Bitwise",
    "B3": "BitStruct(v=BitsInteger(32)) parse/build — 32-bit unsigned via Bitwise",
    "B4": "BitStruct(v=BitsInteger(8, signed=True)) parse/build — 8-bit signed via Bitwise",
    "BS1": "BitStruct 3 字段（4+10+2=16 bit）— 实测基线场景",
    "BS2": "BitStruct 4 字段含 Bit/Nibble/Octet（1+4+8+3=16 bit）",
    "BW1": "BitStruct + Bytewise 嵌入（4+16+4=24 bit，未对齐慢路径）",
    "BW2": "Struct(v=BitsSwapped(Bytes(4))) — TransformNode BitSwap（Struct 包裹）",
    "BW3": "Struct(v=ByteSwapped(Int32ub)) — TransformNode ByteSwap（Struct 包裹）",
    "BW4": "Struct + 字节级 Padding（1+4+2=7 字节）",
}

# 性能门禁
GATE_TARGET = 10.0  # 几何平均目标 ≥10x
GATE_FAIL = 4.0     # 单点 <4x 视为门禁失败


# ---------------------------------------------------------------------------
# 主流程
# ---------------------------------------------------------------------------

def run_all_benchmarks(cases: list[str]) -> dict:
    """运行全部 case × {parse, build} × {rs, py} 测量。

    :param cases: 要测量的 case ID 列表。
    :return: 嵌套字典 {case_id: {direction: {impl: result_dict}}}。
    """
    results = {}
    for case_id in cases:
        results[case_id] = {}
        for direction in DIRECTIONS:
            results[case_id][direction] = {}
            for impl in ["rs", "py"]:
                print(
                    f"  测量 {case_id:<4} {direction:<6} {impl}...",
                    end="",
                    flush=True,
                )
                r = _run_measurement(impl, case_id, direction)
                results[case_id][direction][impl] = r
                print(f" {r['per_call_ns']:.1f} ns/call")
    return results


def format_results_table(results: dict, cases: list[str]) -> str:
    """格式化结果为对比表。"""
    lines = []
    header = (
        f"{'场景':<6} {'方向':<6} {'Rust (ns)':<12} "
        f"{'Python (ns)':<14} {'加速比':<10} {'判定':<10}"
    )
    lines.append(header)
    lines.append("-" * len(header))

    for case_id in cases:
        for direction in DIRECTIONS:
            rs = results[case_id][direction]["rs"]["per_call_ns"]
            py = results[case_id][direction]["py"]["per_call_ns"]
            speedup = py / rs if rs > 0 else float("inf")

            if speedup >= GATE_TARGET:
                verdict = "[PASS]"
            elif speedup >= GATE_FAIL:
                verdict = "[WARN]"
            else:
                verdict = "[FAIL]"

            lines.append(
                f"{case_id:<6} {direction:<6} {rs:<12.1f} {py:<14.1f} "
                f"{speedup:<10.2f}x {verdict}"
            )
    return "\n".join(lines)


def compute_geomean_speedups(results: dict, cases: list[str]) -> dict:
    """计算每个方向的几何平均加速比。

    :return: {'parse': geomean, 'build': geomean, 'all': geomean}
    """
    import math

    def _geomean(values):
        if not values:
            return 0.0
        log_sum = sum(math.log(v) for v in values if v > 0)
        return math.exp(log_sum / len(values))

    parse_speedups = []
    build_speedups = []
    for case_id in cases:
        for direction in DIRECTIONS:
            rs = results[case_id][direction]["rs"]["per_call_ns"]
            py = results[case_id][direction]["py"]["per_call_ns"]
            sp = py / rs if rs > 0 else 0
            if direction == "parse":
                parse_speedups.append(sp)
            else:
                build_speedups.append(sp)

    return {
        "parse": _geomean(parse_speedups),
        "build": _geomean(build_speedups),
        "all": _geomean(parse_speedups + build_speedups),
    }


def compute_derived_metrics(results: dict, cases: list[str]) -> dict:
    """计算派生指标。

    :return: dict 含：
        - per_case: list of {case, rs_parse, rs_build, py_parse, py_build,
          rs_ratio (build/parse), py_ratio (build/parse), speedup_parse,
          speedup_build, speedup_all, low_flag, dir_mismatch_flag}
        - low_speedup_cases: 加速比 <4x 的场景列表
        - direction_mismatch_cases: parse/build 方向与 Python 相反的场景列表
        - sorted_by_complexity: 按加速比升序排列的场景列表
    """
    per_case = []
    low_cases = []
    mismatch_cases = []

    for case_id in cases:
        rs_p = results[case_id]["parse"]["rs"]["per_call_ns"]
        rs_b = results[case_id]["build"]["rs"]["per_call_ns"]
        py_p = results[case_id]["parse"]["py"]["per_call_ns"]
        py_b = results[case_id]["build"]["py"]["per_call_ns"]

        sp_p = py_p / rs_p if rs_p > 0 else 0
        sp_b = py_b / rs_b if rs_b > 0 else 0

        # parse/build 比率：值 >1 表示 build 比 parse 慢，<1 表示 build 比 parse 快
        rs_ratio = rs_b / rs_p if rs_p > 0 else 0
        py_ratio = py_b / py_p if py_p > 0 else 0

        # 方向一致性：Rust 和 Python 的 build-vs-parse 关系是否一致
        # 即 (rs_build > rs_parse) 应与 (py_build > py_parse) 一致
        rs_dir_is_build_slower = rs_b > rs_p
        py_dir_is_build_slower = py_b > py_p
        dir_mismatch = rs_dir_is_build_slower != py_dir_is_build_slower

        # 加速比 <4x 标记（任一方向低于 4x 即标记）
        low_flag = sp_p < GATE_FAIL or sp_b < GATE_FAIL

        entry = {
            "case": case_id,
            "rs_parse": rs_p,
            "rs_build": rs_b,
            "py_parse": py_p,
            "py_build": py_b,
            "rs_ratio": rs_ratio,
            "py_ratio": py_ratio,
            "speedup_parse": sp_p,
            "speedup_build": sp_b,
            "speedup_geomean": (sp_p * sp_b) ** 0.5,
            "low_flag": low_flag,
            "dir_mismatch": dir_mismatch,
        }
        per_case.append(entry)

        if low_flag:
            low_cases.append(entry)
        if dir_mismatch:
            mismatch_cases.append(entry)

    # 按几何平均加速比升序排列（最慢的在前）
    sorted_by_complexity = sorted(
        per_case, key=lambda e: e["speedup_geomean"]
    )

    return {
        "per_case": per_case,
        "low_speedup_cases": low_cases,
        "direction_mismatch_cases": mismatch_cases,
        "sorted_by_complexity": sorted_by_complexity,
    }


def format_derived_metrics(metrics: dict) -> str:
    """格式化派生指标为可读字符串。"""
    lines = []
    lines.append("=" * 78)
    lines.append("派生指标")
    lines.append("=" * 78)

    # 1. parse/build 比率 + 方向一致性
    lines.append("")
    lines.append(
        "1) parse/build 比率（build_ns / parse_ns，>1 = build 较慢）："
    )
    lines.append(
        f"  {'场景':<6} {'Rust比率':<10} {'Py比率':<10} {'方向一致':<10}"
    )
    lines.append("  " + "-" * 50)
    for e in metrics["per_case"]:
        consistent = "一致" if not e["dir_mismatch"] else "[不一致]"
        lines.append(
            f"  {e['case']:<6} "
            f"{e['rs_ratio']:<10.2f} "
            f"{e['py_ratio']:<10.2f} "
            f"{consistent:<10}"
        )

    # 2. 加速比按场景复杂度排列（升序，最差在前）
    lines.append("")
    lines.append(
        "2) 加速比按场景复杂度排列（geomean parse×build，升序，最差在前）："
    )
    lines.append(
        f"  {'场景':<6} {'parse x':<10} {'build x':<10} {'geomean x':<12} {'备注':<10}"
    )
    lines.append("  " + "-" * 60)
    for e in metrics["sorted_by_complexity"]:
        note = "<4x" if e["low_flag"] else ""
        if e["dir_mismatch"]:
            note = ("方向不一致" if note == "" else note + "+方向不一致")
        lines.append(
            f"  {e['case']:<6} "
            f"{e['speedup_parse']:<10.2f} "
            f"{e['speedup_build']:<10.2f} "
            f"{e['speedup_geomean']:<12.2f} "
            f"{note:<10}"
        )

    # 3. 低于 4x 的场景单独标注
    lines.append("")
    lines.append("3) 低于 4x 的场景（parse 或 build 任一方向）：")
    if metrics["low_speedup_cases"]:
        for e in metrics["low_speedup_cases"]:
            lines.append(
                f"  [WARN] {e['case']}: "
                f"parse={e['speedup_parse']:.2f}x, build={e['speedup_build']:.2f}x"
            )
    else:
        lines.append("  无（全部场景 ≥4x）")

    # 4. parse/build 方向与参考实现相反的场景
    lines.append("")
    lines.append(
        "4) parse/build 方向与 Python construct 不一致的场景："
    )
    if metrics["direction_mismatch_cases"]:
        for e in metrics["direction_mismatch_cases"]:
            lines.append(
                f"  [WARN] {e['case']}: "
                f"Rust build/parse={e['rs_ratio']:.2f} "
                f"vs Py build/parse={e['py_ratio']:.2f}"
            )
    else:
        lines.append("  无（全部场景方向一致）")

    return "\n".join(lines)


def check_gates(results: dict, cases: list[str]) -> list[str]:
    """检查通过标准，返回失败消息列表（空表示全部通过）。"""
    failures = []

    # 1. 单点检查：<4x 视为门禁失败
    for case_id in cases:
        for direction in DIRECTIONS:
            rs = results[case_id][direction]["rs"]["per_call_ns"]
            py = results[case_id][direction]["py"]["per_call_ns"]
            speedup = py / rs if rs > 0 else 0
            if speedup < GATE_FAIL:
                failures.append(
                    f"门禁失败：{case_id} {direction} 加速比 {speedup:.2f}x "
                    f"< {GATE_FAIL}x"
                )

    # 2. 几何平均检查（≥10x）
    geomeans = compute_geomean_speedups(results, cases)
    if geomeans["parse"] < GATE_TARGET:
        failures.append(
            f"几何平均门禁失败：parse {geomeans['parse']:.2f}x < {GATE_TARGET}x"
        )
    if geomeans["build"] < GATE_TARGET:
        failures.append(
            f"几何平均门禁失败：build {geomeans['build']:.2f}x < {GATE_TARGET}x"
        )

    return failures


def write_results_file(
    results: dict,
    table: str,
    geomeans: dict,
    failures: list[str],
    cases: list[str],
    derived_text: str = "",
) -> Path:
    """将完整结果写入 bench/results/benchmark_bitstream_results.txt。"""
    out_path = Path(__file__).resolve().parent / "results" / "benchmark_bitstream_results.txt"

    # 构造 case → (分组名, desc) 映射
    case_meta = {cid: (phase, desc) for cid, phase, desc in CASES}

    lines = []
    lines.append("=" * 78)
    lines.append("neoconstruct vs Python construct 2.10.70 BitStream 性能测试结果")
    lines.append("=" * 78)
    lines.append("")
    lines.append("环境信息：")
    lines.append(f"  Python 版本     : {sys.version.split()[0]}")
    lines.append(f"  Python 解释器   : {_PYTHON_EXE}")
    lines.append(f"  平台            : {platform.platform()}")
    lines.append(f"  处理器          : {platform.processor() or '未知'}")
    lines.append(f"  machine         : {platform.machine()}")
    lines.append(f"  timeit number   : {NUMBER}")
    lines.append(f"  repeat (中位数) : {REPEAT}")
    lines.append(f"  测量时间        : {time.strftime('%Y-%m-%d %H:%M:%S')}")
    lines.append("")
    lines.append("测量口径：")
    lines.append("  - neoconstruct：maturin develop 安装到 venv，Python timeit 调用")
    lines.append("    用户面 API（Packet.parse / packet.build）。一次 FFI 穿越。")
    lines.append("  - Python construct 2.10.70：pip install 安装到 venv，Python timeit")
    lines.append("    调用等效 API（fmt.parse / fmt.build）。")
    lines.append("  - 子进程隔离：两个同名 construct 包在独立进程中运行，避免 sys.path 冲突。")
    lines.append("  - 相同 timeit 参数（NUMBER/REPEAT），取相同统计量（中位数）。")
    lines.append("  - 公平口径修订：所有场景 Python 侧均用 BitStruct/Struct 包裹，")
    lines.append("    与 Rust 侧 BitStructMixin/StructMixin 创建容器/实例对等。")
    lines.append("")
    lines.append(f"通过标准：")
    lines.append(f"  - 几何平均 ≥{GATE_TARGET}x（parse 和 build 两个方向）")
    lines.append(f"  - 单点 <{GATE_FAIL}x 视为门禁失败")
    lines.append("")
    lines.append("对比表：")
    lines.append(table)
    lines.append("")
    lines.append("几何平均加速比：")
    lines.append(f"  parse 方向 : {geomeans['parse']:.2f}x")
    lines.append(f"  build 方向 : {geomeans['build']:.2f}x")
    lines.append(f"  全部       : {geomeans['all']:.2f}x")
    lines.append("")
    if derived_text:
        lines.append(derived_text)
        lines.append("")
    lines.append("场景说明：")
    current_phase = None
    for case_id in cases:
        phase, desc = case_meta[case_id]
        if phase != current_phase:
            current_phase = phase
            lines.append(f"  -- {phase} --")
        lines.append(f"  {case_id}: {desc}")
    lines.append("")
    lines.append("门禁检查：")
    if failures:
        for f in failures:
            lines.append(f"  [FAIL] {f}")
    else:
        lines.append("  [PASS] 全部通过")
    lines.append("")
    lines.append("原始数据（per_call_ns = 每次调用纳秒数，min_ns = 单轮最小值）：")
    for case_id in cases:
        for direction in DIRECTIONS:
            for impl in ("rs", "py"):
                r = results[case_id][direction][impl]
                min_ns = r["min_s"] / NUMBER * 1e9
                lines.append(
                    f"  {case_id:<4} {direction:<6} {impl}: "
                    f"median={r['per_call_ns']:.1f}ns  min={min_ns:.1f}ns"
                )
    lines.append("")

    out_path.write_text("\n".join(lines), encoding="utf-8")
    return out_path


def main():
    parser = argparse.ArgumentParser(
        description="neoconstruct BitStream 场景性能基准测试（用户面 API 口径）"
    )
    parser.add_argument(
        "--case",
        action="append",
        help="仅测量指定场景（可多次指定），默认全部 10 个场景",
    )
    parser.add_argument(
        "--list",
        action="store_true",
        help="列出所有场景并退出",
    )
    args = parser.parse_args()

    if args.list:
        print("可用场景：")
        current_phase = None
        for case_id, phase, desc in CASES:
            if phase != current_phase:
                current_phase = phase
                print(f"  -- {phase} --")
            print(f"  {case_id:<5} {desc}")
        return 0

    # 过滤场景
    selected_cases = [cid for cid, _, _ in CASES]
    if args.case:
        unknown = set(args.case) - set(selected_cases)
        if unknown:
            print(f"错误：未知场景 {unknown}")
            print(f"可用场景：{selected_cases}")
            return 1
        selected_cases = args.case

    print("=" * 78)
    print("neoconstruct vs Python construct 2.10.70 BitStream 性能测试")
    print("=" * 78)
    print(f"Python 版本     : {sys.version.split()[0]}")
    print(f"Python 解释器   : {_PYTHON_EXE}")
    print(f"neoconstruct 源 : {_CRS_PYTHON_DIR}")
    print(f"timeit number={NUMBER}, repeat={REPEAT} (取中位数)")
    print()
    print(f"测量 {len(selected_cases)} 个场景（{', '.join(selected_cases)}）")
    print("每个场景 × 方向 × impl 在独立子进程中运行...")
    print()

    results = run_all_benchmarks(selected_cases)

    print()
    print("对比表：")
    table = format_results_table(results, selected_cases)
    print(table)
    print()

    geomeans = compute_geomean_speedups(results, selected_cases)
    print("几何平均加速比：")
    print(f"  parse : {geomeans['parse']:.2f}x")
    print(f"  build : {geomeans['build']:.2f}x")
    print(f"  全部  : {geomeans['all']:.2f}x")
    print()

    metrics = compute_derived_metrics(results, selected_cases)
    derived_text = format_derived_metrics(metrics)
    print(derived_text)
    print()

    failures = check_gates(results, selected_cases)
    print("门禁检查：")
    if failures:
        for f in failures:
            print(f"  [FAIL] {f}")
    else:
        print(f"  [PASS] 全部通过（≥{GATE_TARGET}x）")

    out_path = write_results_file(
        results, table, geomeans, failures, selected_cases, derived_text
    )
    print()
    print(f"详细结果已写入: {out_path}")

    return 0 if not failures else 1


if __name__ == "__main__":
    sys.exit(main())
