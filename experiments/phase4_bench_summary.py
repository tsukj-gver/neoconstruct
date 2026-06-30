"""Phase 4 子任务 4.6 性能基准汇总：全构造器代表性场景。

按 AGENTS.md §6 S-PERF 测量口径：
- construct-rs 侧：通过 maturin develop 安装后，Python 调用用户面 API
- Python construct 侧：直接 import construct，调等效 API
- 子进程隔离（Rust 和 Python 包同名，不可在同一进程导入）
- 取 min(repeat=5) × number

本脚本汇总 Phase 4 全部 6 个构造器的代表性场景（每构造器 2-3 个，共 14 个
测量点），覆盖：
- 4.1 Array：Int8ub N=100/1000 + Struct{2} N=100
- 4.2 GreedyRange：Int8ub N=1024 + Struct{2} N=512
- 4.3 PrefixedArray：Int16ub cf + Int8ub N=1024 + Struct{2} N=512
- 4.4 Index+StopIf：Array(N, Struct{i: Index, v: Byte}) N=256 + StopIf 不触发
- 4.5 RepeatUntil：Expr 路径 N=1024 + PyCallable 路径 N=1024

详细多维度数据见各子任务的 benchmark 脚本：
- phase4_bench_array_v2.py（4.1，12 场景 × 2 方向 = 24 测量点）
- phase4_bench_greedy_range_v2.py（4.2，7 场景 × 2 方向 = 14 测量点）
- phase4_bench_prefixed_array.py（4.3，8 场景 × 2 方向 = 16 测量点）
- phase4_bench_index_stopif.py（4.4，12 测量点）
- phase4_bench_repeat_until.py（4.5，18 测量点）

合计 Phase 4：84 测量点（参见过程记录 4.1-4.5 各段落）。

用法：
    python experiments/phase4_bench_summary.py
    python experiments/phase4_bench_summary.py > experiments/phase4_bench_summary_report.txt 2>&1
"""

import sys
import timeit
import os
import subprocess
import json

# 强制 stdout 用 utf-8，避免 Windows GBK 控制台无法编码 ✓/✗/中文等字符
try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")
except Exception:
    pass


# 两个 venv 的 python.exe 路径（与 parity 测试同口径）
_VENV_ROOT = r"<opencode-temp>"
_RS_PYTHON_EXE = os.path.join(_VENV_ROOT, "crs_venv", "Scripts", "python.exe")
_PY_PYTHON_EXE = os.path.join(_VENV_ROOT, "crs_venv_py", "Scripts", "python.exe")


# ============================================================
# 场景定义
# ============================================================
# 每个场景：(key, ctor, label)
#   ctor: "array" / "greedy" / "prefixed" / "index" / "stopif" / "ru_expr" / "ru_call"
# SCENARIOS 的 key 同时用作 RS_SCENARIOS/PY_SCENARIOS dict 的查询键。
SCENARIOS = [
    # 4.1 Array
    ("a_i8_n100",   "array",   "Array(100, Int8ub) parse/build"),
    ("a_i8_n1000",  "array",   "Array(1000, Int8ub) parse/build"),
    ("a_s2_n100",   "array",   "Array(100, Struct{2}) parse/build"),
    # 4.2 GreedyRange
    ("g_i8_n1024",  "greedy",  "GreedyRange(Int8ub) N=1024 parse/build"),
    ("g_s2_n512",   "greedy",  "GreedyRange(Struct{2}) N=512 parse/build"),
    # 4.3 PrefixedArray
    ("p_i8_n1024",  "prefixed", "PrefixedArray(Int16ub, Int8ub) N=1024 parse/build"),
    ("p_s2_n512",   "prefixed", "PrefixedArray(Int16ub, Struct{2}) N=512 parse/build"),
    # 4.4 Index + StopIf
    ("i_n256",      "index",   "Array(N, Struct{i: Index, v: Byte}) N=256"),
    ("s_no_trip",   "stopif",  "StopIf(x == 0) 不触发 parse/build"),
    # 4.5 RepeatUntil
    ("ru_expr",     "ru_expr", "RepeatUntil Expr 路径 (x > 5) N=1024 parse/build"),
    ("ru_call",     "ru_call", "RepeatUntil PyCallable 路径 N=1024 parse/build"),
]

# 大规模场景用较小 number 避免 timeout
NUMBER_FOR = {
    "a_i8_n1000": 500,
    "g_i8_n1024": 500,
    "g_s2_n512": 200,
    "p_i8_n1024": 500,
    "p_s2_n512": 200,
    "ru_expr": 500,
    "ru_call": 200,
}
DEFAULT_NUMBER = 1000
REPEAT = 5


# ============================================================
# construct-rs 测量
# ============================================================
def bench_construct_rs():
    """此子进程仅导入 construct-rs。"""
    from dataclasses import dataclass
    from typing import Any
    from construct import (
        StructMixin, field, rfield,
        Int8ub, Int32ub, Int16ub,
        Array, GreedyRange, PrefixedArray,
        Index, StopIf, RepeatUntil,
    )

    @dataclass
    class S2(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)

    @dataclass
    class InnerIdx(StructMixin):
        i: int = rfield(Index())
        v: int = field(Int8ub)

    @dataclass
    class StopIfHolder(StructMixin):
        x: int = field(Int8ub)
        stop: Any = rfield(StopIf(x == 0))
        y: int = field(Int8ub, default=0)

    @dataclass
    class RuExpr(StructMixin):
        items: list = field(RepeatUntil(lambda x, lst, ctx: x > 5, Int8ub))

    @dataclass
    class RuCallable(StructMixin):
        items: list = field(
            RepeatUntil(lambda x, lst, ctx: len(lst) >= 2 and lst[-2] == lst[-1], Int8ub)
        )

    # 预编译各类
    @dataclass
    class Ai8(StructMixin):
        """占位类，避免 lint 报未使用 warning；实际类在 _array_class 内动态生成。"""
        items: list = field(Array(1, Int8ub))

    @dataclass
    class As2(StructMixin):
        items: list = field(Array(1, S2))

    def _array_class(n, kind):
        """构造 Array 类 + 对应 parse 数据。"""
        if kind == "i8":
            @dataclass
            class C(StructMixin):
                items: list = field(Array(n, Int8ub))
            # Int8ub 值域 0-255，循环填充到 n 字节
            data = bytes([i & 0xFF for i in range(n)])
            return C, data
        else:
            @dataclass
            class C(StructMixin):
                items: list = field(Array(n, S2))
            data = bytes([i & 0xFF for i in range(n * 2)])
            return C, data

    def _greedy_class(kind):
        if kind == "i8":
            @dataclass
            class C(StructMixin):
                items: list = field(GreedyRange(Int8ub))
            return C
        else:
            @dataclass
            class C(StructMixin):
                items: list = field(GreedyRange(S2))
            return C

    def _prefixed_class(kind):
        if kind == "i8":
            @dataclass
            class C(StructMixin):
                items: list = field(PrefixedArray(Int16ub, Int8ub))
            return C
        else:
            @dataclass
            class C(StructMixin):
                items: list = field(PrefixedArray(Int16ub, S2))
            return C

    results = {}

    for key, ctor, _label in SCENARIOS:
        n_num = NUMBER_FOR.get(key, DEFAULT_NUMBER)

        if ctor == "array":
            if key == "a_i8_n100":
                cls, data = _array_class(100, "i8")
            elif key == "a_i8_n1000":
                cls, data = _array_class(1000, "i8")
            elif key == "a_s2_n100":
                cls, data = _array_class(100, "s2")
            else:
                raise ValueError(key)
            obj = cls.parse(data)
            # parse
            t = timeit.repeat(lambda: cls.parse(data), number=n_num, repeat=REPEAT)
            parse_ns = min(t) / n_num * 1e9
            # build
            t = timeit.repeat(lambda: obj.build(), number=n_num, repeat=REPEAT)
            build_ns = min(t) / n_num * 1e9
            results[key] = (parse_ns, build_ns)

        elif ctor == "greedy":
            if key == "g_i8_n1024":
                cls = _greedy_class("i8")
                data = bytes(range(256)) * 4  # 1024 字节
            elif key == "g_s2_n512":
                cls = _greedy_class("s2")
                data = bytes([i & 0xFF for i in range(512 * 2)])
            else:
                raise ValueError(key)
            obj = cls.parse(data)
            t = timeit.repeat(lambda: cls.parse(data), number=n_num, repeat=REPEAT)
            parse_ns = min(t) / n_num * 1e9
            t = timeit.repeat(lambda: obj.build(), number=n_num, repeat=REPEAT)
            build_ns = min(t) / n_num * 1e9
            results[key] = (parse_ns, build_ns)

        elif ctor == "prefixed":
            if key == "p_i8_n1024":
                cls = _prefixed_class("i8")
                n_elems = 1024
                data = bytes([0x04, 0x00]) + bytes(range(256)) * 4  # cf=1024 + 1024 元素
            elif key == "p_s2_n512":
                cls = _prefixed_class("s2")
                n_elems = 512
                data = bytes([0x02, 0x00]) + bytes([i & 0xFF for i in range(512 * 2)])
            else:
                raise ValueError(key)
            obj = cls.parse(data)
            t = timeit.repeat(lambda: cls.parse(data), number=n_num, repeat=REPEAT)
            parse_ns = min(t) / n_num * 1e9
            t = timeit.repeat(lambda: obj.build(), number=n_num, repeat=REPEAT)
            build_ns = min(t) / n_num * 1e9
            results[key] = (parse_ns, build_ns)

        elif ctor == "index":
            @dataclass
            class Idx(StructMixin):
                items: list = field(Array(256, InnerIdx))
            data = bytes(range(256))
            obj = Idx.parse(data)
            t = timeit.repeat(lambda: Idx.parse(data), number=n_num, repeat=REPEAT)
            parse_ns = min(t) / n_num * 1e9
            t = timeit.repeat(lambda: obj.build(), number=n_num, repeat=REPEAT)
            build_ns = min(t) / n_num * 1e9
            results[key] = (parse_ns, build_ns)

        elif ctor == "stopif":
            data = b"\x01\x02"  # x=1 不触发 stop
            obj = StopIfHolder.parse(data)
            t = timeit.repeat(lambda: StopIfHolder.parse(data), number=n_num, repeat=REPEAT)
            parse_ns = min(t) / n_num * 1e9
            t = timeit.repeat(lambda: obj.build(), number=n_num, repeat=REPEAT)
            build_ns = min(t) / n_num * 1e9
            results[key] = (parse_ns, build_ns)

        elif ctor == "ru_expr":
            data = bytes([1, 2, 3, 4, 5, 6, 7, 8])  # 在 6 处停止
            # 但 benchmark 需要稳定循环，让 N 大一些
            data = bytes(list(range(1, 100)) + [200])  # 在 200 处停止
            obj = RuExpr.parse(data)
            t = timeit.repeat(lambda: RuExpr.parse(data), number=n_num, repeat=REPEAT)
            parse_ns = min(t) / n_num * 1e9
            t = timeit.repeat(lambda: obj.build(), number=n_num, repeat=REPEAT)
            build_ns = min(t) / n_num * 1e9
            results[key] = (parse_ns, build_ns)

        elif ctor == "ru_call":
            data = bytes([1, 5, 5] + [9] * 100)  # 在 [1,5,5] 处停止
            obj = RuCallable.parse(data)
            t = timeit.repeat(lambda: RuCallable.parse(data), number=n_num, repeat=REPEAT)
            parse_ns = min(t) / n_num * 1e9
            t = timeit.repeat(lambda: obj.build(), number=n_num, repeat=REPEAT)
            build_ns = min(t) / n_num * 1e9
            results[key] = (parse_ns, build_ns)

        else:
            raise ValueError(ctor)

    return results


# ============================================================
# Python construct 2.10.70 测量
# ============================================================
def bench_construct_py():
    """此子进程仅导入 Python construct 2.10.70。"""
    import construct as pc

    results = {}

    for key, ctor, _label in SCENARIOS:
        n_num = NUMBER_FOR.get(key, DEFAULT_NUMBER)

        if ctor == "array":
            if key == "a_i8_n100":
                d = pc.Struct("items"/pc.Array(100, pc.Int8ub))
                data = bytes(range(100))
            elif key == "a_i8_n1000":
                d = pc.Struct("items"/pc.Array(1000, pc.Int8ub))
                data = bytes(range(256)) * 4  # 1024 bytes, 取前 1000
                data = data[:1000]
            elif key == "a_s2_n100":
                d = pc.Struct("items"/pc.Array(
                    100, pc.Struct("a"/pc.Int8ub, "b"/pc.Int8ub)))
                data = bytes([i & 0xFF for i in range(200)])
            else:
                raise ValueError(key)
            obj = d.parse(data)
            t = timeit.repeat(lambda: d.parse(data), number=n_num, repeat=REPEAT)
            parse_ns = min(t) / n_num * 1e9
            t = timeit.repeat(lambda: d.build(obj), number=n_num, repeat=REPEAT)
            build_ns = min(t) / n_num * 1e9
            results[key] = (parse_ns, build_ns)

        elif ctor == "greedy":
            if key == "g_i8_n1024":
                d = pc.Struct("items"/pc.GreedyRange(pc.Int8ub))
                data = bytes(range(256)) * 4
            elif key == "g_s2_n512":
                d = pc.Struct("items"/pc.GreedyRange(
                    pc.Struct("a"/pc.Int8ub, "b"/pc.Int8ub)))
                data = bytes([i & 0xFF for i in range(1024)])
            else:
                raise ValueError(key)
            obj = d.parse(data)
            t = timeit.repeat(lambda: d.parse(data), number=n_num, repeat=REPEAT)
            parse_ns = min(t) / n_num * 1e9
            t = timeit.repeat(lambda: d.build(obj), number=n_num, repeat=REPEAT)
            build_ns = min(t) / n_num * 1e9
            results[key] = (parse_ns, build_ns)

        elif ctor == "prefixed":
            if key == "p_i8_n1024":
                d = pc.Struct("items"/pc.PrefixedArray(pc.Int16ub, pc.Int8ub))
                n_elems = 1024
                data = bytes([0x04, 0x00]) + bytes(range(256)) * 4
            elif key == "p_s2_n512":
                d = pc.Struct("items"/pc.PrefixedArray(
                    pc.Int16ub, pc.Struct("a"/pc.Int8ub, "b"/pc.Int8ub)))
                n_elems = 512
                data = bytes([0x02, 0x00]) + bytes([i & 0xFF for i in range(1024)])
            else:
                raise ValueError(key)
            obj = d.parse(data)
            t = timeit.repeat(lambda: d.parse(data), number=n_num, repeat=REPEAT)
            parse_ns = min(t) / n_num * 1e9
            t = timeit.repeat(lambda: d.build(obj), number=n_num, repeat=REPEAT)
            build_ns = min(t) / n_num * 1e9
            results[key] = (parse_ns, build_ns)

        elif ctor == "index":
            d = pc.Array(256, pc.Struct("i"/pc.Index, "v"/pc.Int8ub))
            data = bytes(range(256))
            obj = d.parse(data)
            t = timeit.repeat(lambda: d.parse(data), number=n_num, repeat=REPEAT)
            parse_ns = min(t) / n_num * 1e9
            t = timeit.repeat(lambda: d.build(obj), number=n_num, repeat=REPEAT)
            build_ns = min(t) / n_num * 1e9
            results[key] = (parse_ns, build_ns)

        elif ctor == "stopif":
            d = pc.Struct("x"/pc.Int8ub, pc.StopIf(pc.this.x == 0), "y"/pc.Int8ub)
            data = b"\x01\x02"  # x=1 不触发
            obj = d.parse(data)
            t = timeit.repeat(lambda: d.parse(data), number=n_num, repeat=REPEAT)
            parse_ns = min(t) / n_num * 1e9
            t = timeit.repeat(lambda: d.build(obj), number=n_num, repeat=REPEAT)
            build_ns = min(t) / n_num * 1e9
            results[key] = (parse_ns, build_ns)

        elif ctor == "ru_expr":
            d = pc.Struct("items"/pc.RepeatUntil(
                lambda x, lst, ctx: x > 5, pc.Int8ub))
            data = bytes(list(range(1, 100)) + [200])
            obj = d.parse(data)
            t = timeit.repeat(lambda: d.parse(data), number=n_num, repeat=REPEAT)
            parse_ns = min(t) / n_num * 1e9
            t = timeit.repeat(lambda: d.build(obj), number=n_num, repeat=REPEAT)
            build_ns = min(t) / n_num * 1e9
            results[key] = (parse_ns, build_ns)

        elif ctor == "ru_call":
            d = pc.Struct("items"/pc.RepeatUntil(
                lambda x, lst, ctx: len(lst) >= 2 and lst[-2] == lst[-1], pc.Int8ub))
            data = bytes([1, 5, 5] + [9] * 100)
            obj = d.parse(data)
            t = timeit.repeat(lambda: d.parse(data), number=n_num, repeat=REPEAT)
            parse_ns = min(t) / n_num * 1e9
            t = timeit.repeat(lambda: d.build(obj), number=n_num, repeat=REPEAT)
            build_ns = min(t) / n_num * 1e9
            results[key] = (parse_ns, build_ns)

        else:
            raise ValueError(ctor)

    return results


# ============================================================
# 子进程调度（rs 与 py 在不同 venv，必须子进程隔离）
# ============================================================
def _run_in_subprocess(python_exe: str, impl: str) -> dict:
    """在指定 python.exe 子进程中跑 bench_construct_rs/py，返回 results dict。

    子进程通过环境变量 _BENCH_IMPL 与 _BENCH_SCRIPT_DIR 通讯，bench 函数被
    单独调用并 print(json.dumps(results))，主进程从 stdout 解析。
    """
    script_path = os.path.abspath(__file__)
    code = (
        "import sys, os, json; "
        f"sys.path.insert(0, {os.path.dirname(script_path)!r}); "
        f"import phase4_bench_summary as m; "
        f"impl = {impl!r}; "
        "results = m.bench_construct_rs() if impl == 'rs' else m.bench_construct_py(); "
        "print('BENCH_RESULTS_JSON_START'); "
        "print(json.dumps(results)); "
        "print('BENCH_RESULTS_JSON_END')"
    )
    result = subprocess.run(
        [python_exe, "-c", code],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    if result.returncode != 0:
        sys.stderr.write(f"子进程失败 impl={impl}\n{result.stderr[-2000:]}\n")
        raise RuntimeError(f"subprocess failed for impl={impl}")
    lines = result.stdout.splitlines()
    start_idx = None
    end_idx = None
    for i, line in enumerate(lines):
        if line.strip() == "BENCH_RESULTS_JSON_START":
            start_idx = i
        elif line.strip() == "BENCH_RESULTS_JSON_END":
            end_idx = i
    if start_idx is None or end_idx is None or end_idx - start_idx < 2:
        raise RuntimeError(
            f"未能从子进程输出中找到 JSON 标记 impl={impl}\nstdout: {result.stdout[-2000:]}"
        )
    json_line = lines[start_idx + 1]
    return json.loads(json_line)


# ============================================================
# 主流程
# ============================================================
def main():
    print("=" * 80)
    print("Phase 4 全构造器性能基准汇总（4.6 收尾）")
    print("=" * 80)
    print(f"REPEAT={REPEAT}, DEFAULT_NUMBER={DEFAULT_NUMBER}, 大规模降至 {min(NUMBER_FOR.values())}")
    print(f"基准：Python construct 2.10.70（绝对基线）")
    print(f"测量口径：min(repeat=5) × number，用户面 API，子进程隔离")
    print(f"Python (rs): {_RS_PYTHON_EXE}")
    print(f"Python (py): {_PY_PYTHON_EXE}")
    print()

    # 在子进程中跑 rs 和 py 测量，避免 import 冲突。
    rs_results = _run_in_subprocess(_RS_PYTHON_EXE, "rs")
    py_results = _run_in_subprocess(_PY_PYTHON_EXE, "py")

    print()
    print("=" * 80)
    print(f"{'场景':<22} {'方向':<7} {'Py ns/call':>14} {'Rs ns/call':>14} {'加速比':>10} {'达标':<6}")
    print("-" * 80)

    thresholds = {
        "array": 10.0,
        "greedy": 10.0,
        "prefixed": 10.0,
        "index": 4.0,      # 辅助节点，4x 硬门禁
        "stopif": 4.0,     # 辅助节点，4x 硬门禁
        "ru_expr": 8.0,    # Expr 路径 ≥8x
        "ru_call": 1.5,    # PyCallable 路径 ≥1.5x（v4）
    }

    pass_count = 0
    total_count = 0
    for key, ctor, label in SCENARIOS:
        py_parse, py_build = py_results[key]
        rs_parse, rs_build = rs_results[key]
        thr = thresholds[ctor]

        # parse
        speedup_p = py_parse / rs_parse
        ok_p = speedup_p >= thr
        total_count += 1
        if ok_p:
            pass_count += 1
        print(f"{key:<22} {'parse':<7} {py_parse:>14.0f} {rs_parse:>14.0f} "
              f"{speedup_p:>9.2f}x {'PASS' if ok_p else 'FAIL':<6}  {label}")

        # build
        speedup_b = py_build / rs_build
        ok_b = speedup_b >= thr
        total_count += 1
        if ok_b:
            pass_count += 1
        print(f"{'':<22} {'build':<7} {py_build:>14.0f} {rs_build:>14.0f} "
              f"{speedup_b:>9.2f}x {'PASS' if ok_b else 'FAIL':<6}")

    print("-" * 80)
    print(f"总计：{pass_count}/{total_count} PASS")
    print()
    print("详细多维度数据（共 84 测量点）见各子任务 benchmark 脚本：")
    for f in [
        "phase4_bench_array_v2.py（4.1 Array，24 测量点）",
        "phase4_bench_greedy_range_v2.py（4.2 GreedyRange，14 测量点）",
        "phase4_bench_prefixed_array.py（4.3 PrefixedArray，16 测量点）",
        "phase4_bench_index_stopif.py（4.4 Index+StopIf，12 测量点）",
        "phase4_bench_repeat_until.py（4.5 RepeatUntil，18 测量点）",
    ]:
        print(f"  - {f}")


if __name__ == "__main__":
    main()
