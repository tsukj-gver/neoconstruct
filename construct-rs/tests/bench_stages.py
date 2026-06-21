"""分阶段计时：定位 Rust parse 内核 234ns/field 的具体开销来源。

目的
----
方案 B' 实施后 O(N²) 瓶颈已消除，但 Rust 内核每字段 ~234ns 远高于理论估算
~85-140ns/field。本脚本通过 6 个测量点的对照分解，推算出每字段的实际开销分布：

    1. 完整 parse（基线）       : Flat.parse(data) [Rust 全流程]
    2. Python struct.unpack ×100: 纯 C 层字节解包
    3. int.from_bytes + dict ×100: PyLong 构造 + PyDict_SetItem 的 Python 侧成本
    4. object.__new__ + setattr ×100: 实例 + 逐字段 setattr
    5. object.__new__ + __dict__ 替换: 实例 + 整体 dict 替换（Rust 路径的 Python 等价）
    6. Python construct 基线     : pc.Struct(...).parse(data) [绝对基线]

通过对比可推算：
- C 层 struct.unpack 的纯解析成本（测量点 2）
- PyLong 构造 + dict SetItem 的 Python 侧成本（测量点 3 - 测量点 2）
- 实例构造成本（测量点 4 / 5）
- 我们的 Rust 内核 vs 理论下界的差距（测量点 1 vs 测量点 3 + 5）

时间预算
--------
- number=10000, repeat=3
- 6 个测量点 × 30000 次迭代 ≈ 30-50s（控制在 60s 内）

隔离策略
--------
两个 ``construct`` 同名包无法在同一进程共存：
- 子进程 A：sys.path 注入 construct-rs/python，跑测量点 1（Rust parse）
  + 测量点 2-5（纯 Python 操作，不依赖 construct）
- 子进程 B：默认 sys.path（site-packages 优先），跑测量点 6（Python construct）

用法::

    python tests/bench_stages.py
"""

from __future__ import annotations

import json
import os
import platform
import statistics
import subprocess
import sys
import textwrap
import time
from pathlib import Path

# 时间预算：number=10000, repeat=3，总迭代 30000/测量点
NUMBER = 10_000
REPEAT = 3
# B4 配置：100 字段 Int32ub
N_FIELDS = 100

# construct-rs 的 python/ 包目录
_CRS_PYTHON_DIR = str(
    Path(__file__).resolve().parent.parent / "python"
)


# ---------------------------------------------------------------------------
# 子进程 A：测量点 1-5（Rust parse + 纯 Python 操作）
# ---------------------------------------------------------------------------

_RS_STAGES_SCRIPT = textwrap.dedent(
    """
    import json
    import statistics
    import sys
    import timeit
    import struct

    N = {n}
    NUMBER = {number}
    REPEAT = {repeat}

    # ---- sys.path 操纵：construct-rs 优先 ----
    sys.path.insert(0, {crs_python_dir!r})
    for k in list(sys.modules):
        if k == 'construct' or k.startswith('construct.'):
            del sys.modules[k]

    from dataclasses import dataclass
    from construct import StructMixin, field, Int32ub

    # ---- 构造 @dataclass Flat 类，N 个 Int32ub 字段（与 B4 一致）----
    code_lines = ['@dataclass', 'class Flat(StructMixin):']
    for i in range(N):
        code_lines.append('    f{{}}: int = field(Int32ub)'.format(i))
    ns = {{'dataclass': dataclass, 'StructMixin': StructMixin,
           'field': field, 'Int32ub': Int32ub}}
    exec('\\n'.join(code_lines), ns)
    Flat = ns['Flat']
    schema = Flat._construct_compiled

    # ---- 数据：4N 字节，每 4 字节一个 Int32ub ----
    data = bytes((i % 256) for i in range(4 * N))

    # 预热
    Flat.parse(data)

    # ---- 准备测量目标 ----
    # 提前构造字段名列表（避免在循环里重复构造 f-string）
    names = ['f{{}}'.format(i) for i in range(N)]
    # 提前 intern 字段名（dict set_item 用 intern 字符串更快）
    interned_names = [sys.intern(n) for n in names]

    # 测量点 1: 完整 parse（基线）—— Rust 全流程
    def m1_full_parse():
        return Flat.parse(data)

    # 测量点 2: 纯 Python struct.unpack × N（C 层字节解包基线）
    _fmt = '>I'
    def m2_struct_unpack():
        for i in range(N):
            struct.unpack(_fmt, data[i*4:i*4+4])
        return None

    # 测量点 3: int.from_bytes + dict set_item × N（Python 侧 PyLong + dict 成本）
    def m3_int_from_bytes_dict():
        d = dict()
        for i in range(N):
            d[interned_names[i]] = int.from_bytes(data[i*4:i*4+4], 'big')
        return d

    # 测量点 4: object.__new__ + N 次 setattr（实例 + 逐字段 setattr）
    # 注：纯 object 实例无 __dict__，必须用带 __dict__ 的普通类
    class _Plain:
        pass
    def m4_new_then_setattr():
        obj = _Plain.__new__(_Plain)
        for i in range(N):
            setattr(obj, interned_names[i], i)
        return obj

    # 测量点 5: object.__new__ + __dict__ 整体替换（Rust 路径的 Python 等价）
    # 模拟 create_class + force_setattr("__dict__", dict) 的一次性成本
    def m5_new_then_replace_dict():
        # 先建好 dict（与测量点 3 等价的开销）
        d = dict()
        for i in range(N):
            d[interned_names[i]] = int.from_bytes(data[i*4:i*4+4], 'big')
        # 实例 + __dict__ 替换
        inst = _Plain.__new__(_Plain)
        inst.__dict__ = d
        return inst

    targets = {{
        'm1_full_parse': m1_full_parse,
        'm2_struct_unpack': m2_struct_unpack,
        'm3_int_from_bytes_dict': m3_int_from_bytes_dict,
        'm4_new_then_setattr': m4_new_then_setattr,
        'm5_new_then_replace_dict': m5_new_then_replace_dict,
    }}

    # ---- 测量 ----
    results = {{}}
    for name, target in targets.items():
        # 预热 1 次
        target()
        timer = timeit.Timer(target)
        times = timer.repeat(repeat=REPEAT, number=NUMBER)
        per_call_ns = (statistics.median(times) / NUMBER) * 1e9
        results[name] = per_call_ns
        # 同时记录 min 以观察稳定性
        min_ns = (min(times) / NUMBER) * 1e9
        results[name + '_min'] = min_ns

    results['n'] = N
    print(json.dumps(results))
    """
)


# ---------------------------------------------------------------------------
# 子进程 B：测量点 6（Python construct 2.10.70 基线）
# ---------------------------------------------------------------------------

_PY_BASELINE_SCRIPT = textwrap.dedent(
    """
    import json
    import statistics
    import sys
    import timeit

    N = {n}
    NUMBER = {number}
    REPEAT = {repeat}

    # ---- sys.path 操纵：清除 construct-rs，使用 site-packages 原版 ----
    crs_dir = {crs_python_dir!r}
    sys.path[:] = [p for p in sys.path if p != crs_dir]
    for k in list(sys.modules):
        if k == 'construct' or k.startswith('construct.'):
            del sys.modules[k]

    import construct as pc

    # ---- 构造 Struct，N 个 Int32ub 字段 ----
    subcons = [('f{{}}'.format(i), pc.Int32ub) for i in range(N)]
    flat = pc.Struct(*[(name / cons) for name, cons in subcons])

    data = bytes((i % 256) for i in range(4 * N))

    target = lambda: flat.parse(data)
    target()  # 预热

    timer = timeit.Timer(target)
    times = timer.repeat(repeat=REPEAT, number=NUMBER)
    per_call_ns = (statistics.median(times) / NUMBER) * 1e9
    min_ns = (min(times) / NUMBER) * 1e9

    print(json.dumps({{
        'm6_python_construct': per_call_ns,
        'm6_python_construct_min': min_ns,
        'n': N,
    }}))
    """
)


def _run_rs_stages() -> dict:
    """子进程 A：测量点 1-5。"""
    code = _RS_STAGES_SCRIPT.format(
        n=N_FIELDS, number=NUMBER, repeat=REPEAT,
        crs_python_dir=_CRS_PYTHON_DIR,
    )
    env = os.environ.copy()
    env["PYO3_USE_ABI3_FORWARD_COMPATIBILITY"] = "1"
    result = subprocess.run(
        [sys.executable, "-c", code],
        capture_output=True, text=True, encoding="utf-8",
        errors="replace", check=False, env=env,
    )
    if result.returncode != 0:
        raise RuntimeError(
            "construct-rs 阶段测量失败\nstdout:\n{}\nstderr:\n{}".format(
                result.stdout, result.stderr)
        )
    lines = [ln for ln in result.stdout.strip().splitlines() if ln.strip()]
    return json.loads(lines[-1])


def _run_py_baseline() -> dict:
    """子进程 B：测量点 6。"""
    code = _PY_BASELINE_SCRIPT.format(
        n=N_FIELDS, number=NUMBER, repeat=REPEAT,
        crs_python_dir=_CRS_PYTHON_DIR,
    )
    env = os.environ.copy()
    env["PYO3_USE_ABI3_FORWARD_COMPATIBILITY"] = "1"
    result = subprocess.run(
        [sys.executable, "-c", code],
        capture_output=True, text=True, encoding="utf-8",
        errors="replace", check=False, env=env,
    )
    if result.returncode != 0:
        raise RuntimeError(
            "Python construct 基线测量失败\nstdout:\n{}\nstderr:\n{}".format(
                result.stdout, result.stderr)
        )
    lines = [ln for ln in result.stdout.strip().splitlines() if ln.strip()]
    return json.loads(lines[-1])


# ---------------------------------------------------------------------------
# 报告生成
# ---------------------------------------------------------------------------


def build_report(rs: dict, py: dict) -> str:
    """生成完整分析报告。"""
    n = rs["n"]
    lines = []

    # ---- 测量结果表 ----
    lines.append("=" * 78)
    lines.append("分阶段计时结果（N={} 字段 Int32ub）".format(n))
    lines.append("=" * 78)
    lines.append("")

    header = (
        "| {:<2} | {:<32} | {:>12} | {:>12} |".format(
            "#", "测量点", "median (ns)", "min (ns)")
    )
    lines.append(header)
    lines.append("|" + "-" * 76 + "|")

    points = [
        ("1", "完整 parse (Flat.parse) [Rust 全流程]",
         rs["m1_full_parse"], rs["m1_full_parse_min"]),
        ("2", "Python struct.unpack × N",
         rs["m2_struct_unpack"], rs["m2_struct_unpack_min"]),
        ("3", "int.from_bytes + dict set_item × N",
         rs["m3_int_from_bytes_dict"], rs["m3_int_from_bytes_dict_min"]),
        ("4", "object.__new__ + setattr × N",
         rs["m4_new_then_setattr"], rs["m4_new_then_setattr_min"]),
        ("5", "__new__ + __dict__ 整体替换",
         rs["m5_new_then_replace_dict"], rs["m5_new_then_replace_dict_min"]),
        ("6", "Python construct 2.10.70 baseline",
         py["m6_python_construct"], py["m6_python_construct_min"]),
    ]

    for num, desc, med, mn in points:
        lines.append("| {:<2} | {:<32} | {:>12.1f} | {:>12.1f} |".format(
            num, desc, med, mn))
    lines.append("")

    # ---- 每字段开销分解 ----
    lines.append("=" * 78)
    lines.append("每字段开销分解（实测 vs 理论）")
    lines.append("=" * 78)
    lines.append("")

    m1 = rs["m1_full_parse"]
    m2 = rs["m2_struct_unpack"]
    m3 = rs["m3_int_from_bytes_dict"]
    m4 = rs["m4_new_then_setattr"]
    m5 = rs["m5_new_then_replace_dict"]
    m6 = py["m6_python_construct"]

    per_field_rust = m1 / n
    per_field_pystruct = m2 / n
    per_field_pylong_dict = m3 / n
    per_field_setattr = m4 / n
    per_field_replace = m5 / n
    per_field_py_construct = m6 / n

    # 增量分析
    pure_unpack = m2 / n  # 测量点 2：纯 C 层 struct.unpack
    pylong_dict_extra = (m3 - m2) / n  # 测量点 3 - 2: PyLong + dict set_item 增量
    instance_setattr = m4 / n  # 测量点 4: __new__ + setattr × N
    instance_replace = m5 / n  # 测量点 5: __new__ + dict 替换

    lines.append("每字段平均开销（ns/field）：")
    lines.append("")
    lines.append("  Rust 全流程（测量点 1）        : {:>7.2f} ns/field".format(
        per_field_rust))
    lines.append("  Python struct.unpack（测量点 2）: {:>7.2f} ns/field".format(
        per_field_pystruct))
    lines.append("  PyLong+dict（测量点 3）         : {:>7.2f} ns/field".format(
        per_field_pylong_dict))
    lines.append("  setattr × N（测量点 4）         : {:>7.2f} ns/field".format(
        per_field_setattr))
    lines.append("  __dict__ 替换（测量点 5）       : {:>7.2f} ns/field".format(
        per_field_replace))
    lines.append("  Python construct（测量点 6）    : {:>7.2f} ns/field".format(
        per_field_py_construct))
    lines.append("")

    lines.append("增量分析：")
    lines.append("  纯 C 层 struct.unpack 成本        : {:>7.2f} ns/field  (测量点 2 / N)".format(
        pure_unpack))
    lines.append("  PyLong 构造 + dict SetItem 增量   : {:>7.2f} ns/field  (测量点 3 - 测量点 2)".format(
        pylong_dict_extra))
    lines.append("  object.__new__ + setattr × N     : {:>7.2f} ns/field  (测量点 4 / N)".format(
        instance_setattr))
    lines.append("")

    # ---- Rust vs 理论下界 ----
    lines.append("=" * 78)
    lines.append("Rust 内核 vs 理论下界（Python 等价操作之和）")
    lines.append("=" * 78)
    lines.append("")

    # 理论下界 = 测量点 2（字节解包）+ PyLong 增量 + dict set_item 增量
    # 但测量点 3 已含 bytes 切片（int.from_bytes 内部）
    # 更准确的下界估算：测量点 2（C 解包）+ (测量点 3 - 测量点 2)（PyLong+dict 增量）+ 实例构造摊销
    # 实例构造摊销 = 测量点 5 中除 dict 构建外的额外开销 / N
    instance_overhead = (m5 - m3) / n  # __new__ + __dict__ 替换的额外开销（去 dict 构建）

    theoretical_floor = pure_unpack + pylong_dict_extra + max(0, instance_overhead)
    lines.append("理论下界（Python 侧各阶段之和）：")
    lines.append("  纯 struct.unpack (C 层)         : {:>7.2f} ns/field".format(pure_unpack))
    lines.append("  + PyLong 构造 + dict SetItem    : {:>7.2f} ns/field".format(pylong_dict_extra))
    lines.append("  + 实例构造摊销(__new__+替换)   : {:>7.2f} ns/field".format(max(0, instance_overhead)))
    lines.append("  ────────────────────────────────────────────")
    lines.append("  合计理论下界                    : {:>7.2f} ns/field".format(theoretical_floor))
    lines.append("")
    lines.append("  Rust 实测（测量点 1 / N）       : {:>7.2f} ns/field".format(per_field_rust))
    lines.append("  差距（Rust - 理论下界）         : {:>7.2f} ns/field  ← 不明开销".format(
        per_field_rust - theoretical_floor))
    lines.append("")

    # ---- 加速比 ----
    lines.append("=" * 78)
    lines.append("加速比对比")
    lines.append("=" * 78)
    lines.append("")
    lines.append("  Rust vs Python construct        : {:>5.2f}x".format(m6 / m1))
    lines.append("  Rust vs 测量点 3 (PyLong+dict)  : {:>5.2f}x".format(m3 / m1))
    lines.append("  Rust vs 测量点 5 (__dict__ 替换): {:>5.2f}x".format(m5 / m1))
    lines.append("")

    # ---- Python 基线稳定性 ----
    lines.append("=" * 78)
    lines.append("Python 基线稳定性确认（同环境同时测量）")
    lines.append("=" * 78)
    lines.append("")
    lines.append("  测量点 6 median : {:.1f} ns".format(m6))
    lines.append("  测量点 6 min    : {:.1f} ns".format(py["m6_python_construct_min"]))
    lines.append("  抖动 (med-min)/med : {:.2%}".format(
        (m6 - py["m6_python_construct_min"]) / m6 if m6 > 0 else 0))
    lines.append("  解析：抖动 < 5% 视为环境稳定".format())
    lines.append("")

    return "\n".join(lines)


def write_results_file(report: str, rs: dict, py: dict) -> Path:
    out_path = Path(__file__).resolve().parent / "bench_stages_results.txt"

    lines = []
    lines.append("=" * 78)
    lines.append("construct-rs parse 内核分阶段计时（234ns/field 开销定位）")
    lines.append("=" * 78)
    lines.append("")
    lines.append("环境信息：")
    lines.append("  Python 版本    : {}".format(sys.version.split()[0]))
    lines.append("  平台           : {}".format(platform.platform()))
    lines.append("  处理器         : {}".format(platform.processor() or "未知"))
    lines.append("  timeit number  : {}".format(NUMBER))
    lines.append("  repeat (中位数): {}".format(REPEAT))
    lines.append("  字段数 N       : {} (B4 配置)".format(N_FIELDS))
    lines.append("  测量时间       : {}".format(time.strftime("%Y-%m-%d %H:%M:%S")))
    lines.append("")
    lines.append("测量点说明：")
    lines.append("  1. Flat.parse(data)              —— Rust 全流程（基线）")
    lines.append("  2. struct.unpack × N             —— 纯 C 层字节解包")
    lines.append("  3. int.from_bytes + dict set_item —— PyLong 构造 + PyDict_SetItem")
    lines.append("  4. object.__new__ + setattr × N  —— 实例 + 逐字段 setattr")
    lines.append("  5. __new__ + __dict__ 整体替换   —— Rust 路径的 Python 等价")
    lines.append("  6. Python construct 2.10.70      —— 绝对基线")
    lines.append("")
    lines.append(report)
    lines.append("")
    lines.append("原始 JSON：")
    lines.append("  RS  : " + json.dumps(rs, ensure_ascii=False))
    lines.append("  PY  : " + json.dumps(py, ensure_ascii=False))

    out_path.write_text("\n".join(lines), encoding="utf-8")
    return out_path


def main() -> int:
    print("=" * 78)
    print("construct-rs parse 内核分阶段计时（234ns/field 定位）")
    print("=" * 78)
    print("Python 版本: {}".format(sys.version.split()[0]))
    print("平台       : {}".format(platform.platform()))
    print("timeit number={}, repeat={}, N={} 字段 (B4)".format(
        NUMBER, REPEAT, N_FIELDS))
    print()

    print("[1/2] 测量 construct-rs + 纯 Python 操作（测量点 1-5）...")
    rs = _run_rs_stages()
    print("  m1_full_parse          : {:.1f} ns".format(rs["m1_full_parse"]))
    print("  m2_struct_unpack       : {:.1f} ns".format(rs["m2_struct_unpack"]))
    print("  m3_int_from_bytes_dict : {:.1f} ns".format(rs["m3_int_from_bytes_dict"]))
    print("  m4_new_then_setattr    : {:.1f} ns".format(rs["m4_new_then_setattr"]))
    print("  m5_new_then_replace    : {:.1f} ns".format(rs["m5_new_then_replace_dict"]))
    print()

    print("[2/2] 测量 Python construct 2.10.70 基线（测量点 6）...")
    py = _run_py_baseline()
    print("  m6_python_construct    : {:.1f} ns".format(py["m6_python_construct"]))
    print()

    report = build_report(rs, py)
    out_path = write_results_file(report, rs, py)

    # 打印报告（容错：Windows GBK 控制台）
    try:
        print(report)
    except UnicodeEncodeError:
        sys.stdout.reconfigure(errors="replace")  # type: ignore[attr-defined]
        print(report)

    print("详细结果已写入: {}".format(out_path))
    return 0


if __name__ == "__main__":
    sys.exit(main())
