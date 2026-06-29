"""Phase 4 子任务 4.2 性能基准：GreedyRange parse/build。

按 AGENTS.md §6 S-PERF 测量口径：
- construct-rs 侧：通过 maturin develop 安装后，Python 调用用户面 API
- Python construct 侧：直接 import construct，调等效 API
- 子进程隔离（Rust 和 Python 包同名，不可在同一进程导入）
- 取 min(repeat=5) * number=1000

场景（设计 §8.2）：
- GreedyRange(Int8ub) parse 1024 元素（1 KB）
- GreedyRange(Int8ub) build 1024 元素

S-PERF 出口标准：≥10x（与 4.1 ArrayNode 同级，不纳入 §8.3 的 RepeatUntil 例外）。
GreedyRange 与 Array 性能特性接近：内层 parse 循环 + ctx.set_index + PyList.append。
主要差异：
- 无法预知元素数量，PyList 动态增长（无预分配）
- 每次迭代额外记录 fallback = stream.tell()（~1ns）
- 失败时 seek 回退（仅一次开销，对总耗时影响小）
"""

import sys
import timeit
import os


def bench_construct_rs():
    """construct-rs 测量（此子进程仅导入 construct-rs）。"""
    from dataclasses import dataclass

    from construct import StructMixin, field, Int8ub, GreedyRange

    # 场景 1: GreedyRange(Int8ub) - 1 KB = 1024 字节 = 1024 个元素
    @dataclass
    class GR(StructMixin):
        items: list = field(GreedyRange(Int8ub))

    # 准备数据
    N = 1024
    data_gr = bytes(i % 256 for i in range(N))  # 0..255 循环，共 1024 字节
    obj_gr = GR(items=list(i % 256 for i in range(N)))

    NUMBER = 1000
    REPEAT = 5

    # Parse benchmarks
    rs_gr_parse = min(
        timeit.repeat(lambda: GR.parse(data_gr), number=NUMBER, repeat=REPEAT)
    ) / NUMBER

    # Build benchmarks
    rs_gr_build = min(
        timeit.repeat(lambda: obj_gr.build(), number=NUMBER, repeat=REPEAT)
    ) / NUMBER

    return {
        "gr_parse": rs_gr_parse,
        "gr_build": rs_gr_build,
    }


def bench_python_construct():
    """Python construct 2.10.70 测量（此子进程仅导入 construct）。"""
    from construct import GreedyRange, Byte

    # 场景 1: GreedyRange(Byte) - 1 KB
    gr = GreedyRange(Byte)
    N = 1024
    data_gr = bytes(i % 256 for i in range(N))
    obj_gr = list(i % 256 for i in range(N))

    NUMBER = 1000
    REPEAT = 5

    py_gr_parse = min(
        timeit.repeat(lambda: gr.parse(data_gr), number=NUMBER, repeat=REPEAT)
    ) / NUMBER

    py_gr_build = min(
        timeit.repeat(lambda: gr.build(obj_gr), number=NUMBER, repeat=REPEAT)
    ) / NUMBER

    return {
        "gr_parse": py_gr_parse,
        "gr_build": py_gr_build,
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
        print(
            f"{'场景':<30} {'Python ns/call':>18} {'Rust ns/call':>18} "
            f"{'加速比':>10}"
        )
        print("-" * 96)

        # 解析比率：parse / build （每个实现内部）
        def parse_build_ratio(data):
            return data["gr_parse"] / data["gr_build"]

        py_ratio = parse_build_ratio(py_data)
        rs_ratio = parse_build_ratio(rs_data)

        scenarios = [
            ("GreedyRange(Int8ub) parse 1KB", "gr_parse"),
            ("GreedyRange(Int8ub) build 1KB", "gr_build"),
        ]

        speedups = []
        for name, key in scenarios:
            py_ns = py_data[key]
            rs_ns = rs_data[key]
            speedup = py_ns / rs_ns
            speedups.append((name, speedup, py_ns < rs_ns))
            low_flag = " [WARN <4x]" if speedup < 4 else ""
            print(
                f"{name:<30} {format_ns(py_ns):>18} {format_ns(rs_ns):>18} "
                f"{speedup:>8.2f}x{low_flag}"
            )

        print("-" * 96)
        print(f"{'parse/build 比率 (py)':<30} {py_ratio:>18.2f}x")
        print(f"{'parse/build 比率 (rs)':<30} {rs_ratio:>18.2f}x")
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
                f"[INFO] parse/build 方向不一致 (py {py_ratio:.2f}x vs rs {rs_ratio:.2f}x)"
            )
            print("  （参考 4.1：build 无 PyLong 分配开销，rs 通常 build > parse）")

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
