"""Phase 4 子任务 4.1 性能基准：Array parse/build。

按 AGENTS.md §6 S-PERF 测量口径：
- construct-rs 侧：通过 maturin develop 安装后，Python 调用用户面 API
- Python construct 侧：直接 import construct，调等效 API
- 子进程隔离（Rust 和 Python 包同名，不可在同一进程导入）
- 取 min(repeat=5) * number=1000

场景（设计 §8.2）：
- Array(100, Int8ub) parse / build
- Array(10, Struct{2 fields}) parse / build
"""

import sys
import timeit
import os


def bench_construct_rs():
    """construct-rs 测量（此子进程仅导入 construct-rs）。"""
    from dataclasses import dataclass

    from construct import StructMixin, field, Int8ub, Array

    # 场景 1: Array(100, Int8ub) - 常量 count
    @dataclass
    class Arr100(StructMixin):
        items: list = field(Array(100, Int8ub))

    # 场景 2: Array(10, Struct{2 字段})
    @dataclass
    class Inner(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)

    @dataclass
    class Arr10Struct(StructMixin):
        items: list = field(Array(10, Inner))

    # 准备数据
    data_arr100 = bytes(range(100))
    obj_arr100 = Arr100(items=list(range(100)))

    data_arr10s = bytes((i * 2) % 256 for i in range(20))
    inner_objs = [Inner(a=i, b=i + 1) for i in range(10)]
    obj_arr10s = Arr10Struct(items=inner_objs)

    NUMBER = 1000
    REPEAT = 5

    # Parse benchmarks
    rs_arr100_parse = min(
        timeit.repeat(lambda: Arr100.parse(data_arr100), number=NUMBER, repeat=REPEAT)
    ) / NUMBER

    rs_arr10s_parse = min(
        timeit.repeat(lambda: Arr10Struct.parse(data_arr10s), number=NUMBER, repeat=REPEAT)
    ) / NUMBER

    # Build benchmarks
    rs_arr100_build = min(
        timeit.repeat(lambda: obj_arr100.build(), number=NUMBER, repeat=REPEAT)
    ) / NUMBER

    rs_arr10s_build = min(
        timeit.repeat(lambda: obj_arr10s.build(), number=NUMBER, repeat=REPEAT)
    ) / NUMBER

    return {
        "arr100_parse": rs_arr100_parse,
        "arr100_build": rs_arr100_build,
        "arr10s_parse": rs_arr10s_parse,
        "arr10s_build": rs_arr10s_build,
    }


def bench_python_construct():
    """Python construct 2.10.70 测量（此子进程仅导入 construct）。"""
    from construct import Struct, Array, Byte, Int8ub

    # 场景 1: Array(100, Byte)
    arr100 = Array(100, Byte)
    data_arr100 = bytes(range(100))
    obj_arr100 = list(range(100))

    # 场景 2: Array(10, Struct("a"/Byte, "b"/Byte))
    inner_struct = Struct("a" / Byte, "b" / Byte)
    arr10s = Array(10, inner_struct)
    data_arr10s = bytes((i * 2) % 256 for i in range(20))
    # Container 是 dict-like，构造 list of Container
    from construct import Container

    obj_arr10s = [Container(a=i, b=i + 1) for i in range(10)]

    NUMBER = 1000
    REPEAT = 5

    py_arr100_parse = min(
        timeit.repeat(lambda: arr100.parse(data_arr100), number=NUMBER, repeat=REPEAT)
    ) / NUMBER

    py_arr10s_parse = min(
        timeit.repeat(lambda: arr10s.parse(data_arr10s), number=NUMBER, repeat=REPEAT)
    ) / NUMBER

    py_arr100_build = min(
        timeit.repeat(lambda: arr100.build(obj_arr100), number=NUMBER, repeat=REPEAT)
    ) / NUMBER

    py_arr10s_build = min(
        timeit.repeat(lambda: arr10s.build(obj_arr10s), number=NUMBER, repeat=REPEAT)
    ) / NUMBER

    return {
        "arr100_parse": py_arr100_parse,
        "arr100_build": py_arr100_build,
        "arr10s_parse": py_arr10s_parse,
        "arr10s_build": py_arr10s_build,
    }


def format_ns(seconds):
    """秒 → 纳秒字符串。"""
    return f"{seconds * 1e9:.0f} ns"


def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else "rs"

    if mode == "rs":
        results = bench_construct_rs()
        # 输出 TSV 格式：key\tns_per_call
        for k, v in results.items():
            print(f"rs\t{k}\t{v:.9f}")
    elif mode == "py":
        results = bench_python_construct()
        for k, v in results.items():
            print(f"py\t{k}\t{v:.9f}")
    elif mode == "compare":
        # 需要先在子进程中跑 rs 和 py
        # 两个不同的 venv，避免同名包冲突
        import subprocess

        workdir = r"<legacy-repo>\construct-rs"
        rs_python = r"<opencode-temp>\crs_venv2\Scripts\python.exe"
        py_python = r"<opencode-temp>\crs_venv_py\Scripts\python.exe"
        script = __file__

        env = dict(os.environ)
        env["PYO3_USE_ABI3_FORWARD_COMPATIBILITY"] = "1"

        rs_proc = subprocess.run(
            [rs_python, script, "rs"], cwd=workdir, capture_output=True, text=True, env=env
        )
        if rs_proc.returncode != 0:
            print("rs subprocess failed:", rs_proc.stderr, file=sys.stderr)
            sys.exit(1)

        py_proc = subprocess.run(
            [py_python, script, "py"], cwd=workdir, capture_output=True, text=True, env=env
        )
        if py_proc.returncode != 0:
            print("py subprocess failed:", py_proc.stderr, file=sys.stderr)
            sys.exit(1)

        rs_data = {}
        py_data = {}
        for line in rs_proc.stdout.strip().split("\n"):
            _, k, v = line.split("\t")
            rs_data[k] = float(v)
        for line in py_proc.stdout.strip().split("\n"):
            _, k, v = line.split("\t")
            py_data[k] = float(v)

        # 汇总输出
        print()
        print("=" * 96)
        print(f"{'场景':<30} {'Python ns/call':>18} {'Rust ns/call':>18} {'加速比':>10} {'parse/build 比率':>20}")
        print("-" * 96)

        # 解析比率：parse / build （每个实现内部）
        def parse_build_ratio(data):
            return data["arr100_parse"] / data["arr100_build"]

        py_ratio = parse_build_ratio(py_data)
        rs_ratio = parse_build_ratio(rs_data)

        scenarios = [
            ("Array(100, Int8ub) parse", "arr100_parse"),
            ("Array(100, Int8ub) build", "arr100_build"),
            ("Array(10, Struct2) parse", "arr10s_parse"),
            ("Array(10, Struct2) build", "arr10s_build"),
        ]

        speedups = []
        for name, key in scenarios:
            py_ns = py_data[key]
            rs_ns = rs_data[key]
            speedup = py_ns / rs_ns
            speedups.append((name, speedup, py_ns < rs_ns))  # (name, ratio, low?)
            low_flag = " [WARN <4x]" if speedup < 4 else ""
            print(
                f"{name:<30} {format_ns(py_ns):>18} {format_ns(rs_ns):>18} "
                f"{speedup:>8.2f}x{low_flag}"
            )

        print("-" * 96)
        print(
            f"{'parse/build 比率 (py)':<30} {'':>18} {'':>18} {py_ratio:>18.2f}x"
        )
        print(
            f"{'parse/build 比率 (rs)':<30} {'':>18} {'':>18} {rs_ratio:>18.2f}x"
        )
        print()

        # 加速比按场景排列（验证趋势）
        print("加速比排序（由高到低，验证是否符合预期）：")
        for name, sp, _ in sorted(speedups, key=lambda x: -x[1]):
            print(f"  {name:<30} {sp:>6.2f}x")

        # 低于 4x 的场景单独标注
        low = [(n, s) for n, s, low in speedups if low]
        if low:
            print()
            print("[WARN] 低于 4x 的场景:")
            for n, s in low:
                print(f"  {n:<30} {s:>6.2f}x")
        else:
            print()
            print("[PASS] 全部场景 >= 4x")

        # parse/build 方向一致性
        print()
        if (py_ratio > 1) == (rs_ratio > 1):
            print(
                f"[PASS] parse/build 方向一致 (py {py_ratio:.2f}x vs rs {rs_ratio:.2f}x)"
            )
        else:
            print(
                f"[WARN] parse/build 方向不一致! py {py_ratio:.2f}x vs rs {rs_ratio:.2f}x"
            )

        print("=" * 96)

        # S-PERF 出口判断
        target_speedup = 10.0
        all_met = all(s >= target_speedup for _, s, _ in speedups)
        print()
        if all_met:
            print(f"[PASS] 全部场景 >= {target_speedup}x (满足 S-PERF >=10x 出口标准)")
        else:
            below = [(n, s) for n, s, _ in speedups if s < target_speedup]
            print(f"[WARN] 以下场景低于 {target_speedup}x:")
            for n, s in below:
                print(f"    {n:<30} {s:>6.2f}x")
    else:
        print(f"Unknown mode: {mode}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
