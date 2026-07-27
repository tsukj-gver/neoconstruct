"""Phase 4 子任务 4.2 性能基准 v2：GreedyRange parse/build（多维场景）。

按 performance-gate/SKILL.md S-PERF 测量口径：
- construct-rs 侧：通过 maturin develop 安装后，Python 调用用户面 API
- Python construct 侧：直接 import construct，调等效 API
- 子进程隔离（Rust 和 Python 包同名，不可在同一进程导入）
- 取 min(repeat=5) × number

v2 多维覆盖（PM 驳回 v1 后补充）：
- 元素类型维度：Int8ub（1B 原子）/ Int32ub（4B 原子）/ Bytes(8)（8B blob）
                 / Struct{2×Int8ub}（2B 复合）
- 规模维度：64 / 1024 / 8192 元素（Int8ub 路径）
- 等字节维度：在 ~1KB / ~8KB 字节量级上对比不同元素类型

场景矩阵（7 个）：
| #  | Element            | N elems | 字节/调用 | 维度作用                  |
|----|--------------------|---------|----------|---------------------------|
| G1 | Int8ub             | 64      | 64       | 极小规模基线              |
| G2 | Int8ub             | 1024    | 1024     | v1 基线（横向对比）       |
| G3 | Int8ub             | 8192    | 8192     | 大规模                    |
| G4 | Int32ub            | 256     | 1024     | 多字节原子（1KB 字节量级）|
| G5 | Int32ub            | 2048    | 8192     | 多字节原子（8KB 字节量级）|
| G6 | Bytes(8)           | 128     | 1024     | bytes blob（1KB 字节量级）|
| G7 | Struct{2×Int8ub}   | 512     | 1024     | 复合元素（1KB 字节量级）  |
"""

import sys
import timeit
import os

# 强制 stdout 用 utf-8
try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")
except Exception:
    pass


# ============================================================
# 场景定义（元数据，rs/py 两侧共用）
# ============================================================
# 每个场景：(key, element_kind, n_elems, bytes_per_elem, label)
SCENARIOS = [
    ("g1_i8_n64",      "i8",     64,   1, "GreedyRange(Int8ub) N=64"),
    ("g2_i8_n1024",    "i8",     1024, 1, "GreedyRange(Int8ub) N=1024"),
    ("g3_i8_n8192",    "i8",     8192, 1, "GreedyRange(Int8ub) N=8192"),
    ("g4_i32_n256",    "i32",    256,  4, "GreedyRange(Int32ub) N=256"),
    ("g5_i32_n2048",   "i32",    2048, 4, "GreedyRange(Int32ub) N=2048"),
    ("g6_bytes8_n128", "bytes8", 128,  8, "GreedyRange(Bytes(8)) N=128"),
    ("g7_struct2_n512", "struct", 512,  2, "GreedyRange(Struct{2}) N=512"),
]

NUMBER_FOR = {
    "g3_i8_n8192": 200,
    "g5_i32_n2048": 200,
    "g7_struct2_n512": 500,
}
DEFAULT_NUMBER = 1000
REPEAT = 5


# ============================================================
# construct-rs 测量
# ============================================================
def bench_construct_rs():
    from dataclasses import dataclass
    from construct import (
        StructMixin, field, Int8ub, Int32ub, Bytes, GreedyRange,
    )

    @dataclass
    class S2(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)

    compiled = {}
    for key, kind, n, bpe, _label in SCENARIOS:
        if kind == "i8":
            @dataclass
            class _C(StructMixin):
                items: list = field(GreedyRange(Int8ub))
            data = bytes(i % 256 for i in range(n))
            obj = _C(items=list(i % 256 for i in range(n)))
        elif kind == "i32":
            @dataclass
            class _C(StructMixin):
                items: list = field(GreedyRange(Int32ub))
            data = bytes((i) % 256 for i in range(n * 4))
            obj = _C(items=list(i % 256 for i in range(n)))
        elif kind == "bytes8":
            @dataclass
            class _C(StructMixin):
                items: list = field(GreedyRange(Bytes(8)))
            data = bytes((i) % 256 for i in range(n * 8))
            one = bytes((i) % 256 for i in range(8))
            obj = _C(items=[one] * n)
        elif kind == "struct":
            @dataclass
            class _C(StructMixin):
                items: list = field(GreedyRange(S2))
            data = bytes((i) % 256 for i in range(n * 2))
            obj = _C(items=[S2(a=i % 256, b=(i+1) % 256) for i in range(n)])
        else:
            raise ValueError(kind)

        number = NUMBER_FOR.get(key, DEFAULT_NUMBER)
        compiled[key] = (_C, data, obj, number)

    results = {}
    for key, _kind, _n, _bpe, _label in SCENARIOS:
        cls, data, obj, number = compiled[key]
        parse_ns = min(
            timeit.repeat(lambda c=cls, d=data: c.parse(d), number=number, repeat=REPEAT)
        ) / number
        build_ns = min(
            timeit.repeat(lambda o=obj: o.build(), number=number, repeat=REPEAT)
        ) / number
        results[key + "_parse"] = parse_ns
        results[key + "_build"] = build_ns
    return results


# ============================================================
# Python construct 2.10.70 测量
# ============================================================
def bench_python_construct():
    from construct import (
        GreedyRange, Byte, Int32ub, Bytes, Struct, Container,
    )

    compiled = {}
    for key, kind, n, bpe, _label in SCENARIOS:
        if kind == "i8":
            gr = GreedyRange(Byte)
            data = bytes(i % 256 for i in range(n))
            obj = list(i % 256 for i in range(n))
        elif kind == "i32":
            gr = GreedyRange(Int32ub)
            data = bytes((i) % 256 for i in range(n * 4))
            obj = list(i % 256 for i in range(n))
        elif kind == "bytes8":
            gr = GreedyRange(Bytes(8))
            data = bytes((i) % 256 for i in range(n * 8))
            one = bytes((i) % 256 for i in range(8))
            obj = [one] * n
        elif kind == "struct":
            inner = Struct("a" / Byte, "b" / Byte)
            gr = GreedyRange(inner)
            data = bytes((i) % 256 for i in range(n * 2))
            obj = [Container(a=i % 256, b=(i+1) % 256) for i in range(n)]
        else:
            raise ValueError(kind)

        number = NUMBER_FOR.get(key, DEFAULT_NUMBER)
        compiled[key] = (gr, data, obj, number)

    results = {}
    for key, _kind, _n, _bpe, _label in SCENARIOS:
        gr, data, obj, number = compiled[key]
        parse_ns = min(
            timeit.repeat(lambda g=gr, d=data: g.parse(d), number=number, repeat=REPEAT)
        ) / number
        build_ns = min(
            timeit.repeat(lambda g=gr, o=obj: g.build(o), number=number, repeat=REPEAT)
        ) / number
        results[key + "_parse"] = parse_ns
        results[key + "_build"] = build_ns
    return results


# ============================================================
# 格式化与汇总
# ============================================================
def format_ns(seconds):
    if seconds >= 1e-3:
        return f"{seconds * 1e6:.2f} us"
    return f"{seconds * 1e9:.0f} ns"


def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else "rs"

    if mode == "rs":
        results = bench_construct_rs()
        for k, v in results.items():
            print(f"rs\t{k}\t{v:.9f}")
    elif mode == "py":
        results = bench_python_construct()
        for k, v in results.items():
            print(f"py\t{k}\t{v:.9f}")
    elif mode == "compare":
        import subprocess

        workdir = r"<legacy-repo>\construct-rs"
        rs_python = r"<opencode-temp>\crs_venv\Scripts\python.exe"
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

        _emit_report(rs_data, py_data)
    else:
        print(f"Unknown mode: {mode}", file=sys.stderr)
        sys.exit(1)


def _emit_report(rs_data, py_data):
    print()
    print("=" * 110)
    print(f"{'#':<5}{'场景':<35}{'N':>7}{'bytes/call':>13}"
          f"{'Py ns/call':>16}{'Rs ns/call':>16}{'加速比':>10}{'方向':>8}")
    print("-" * 110)

    speedups = []
    for key, kind, n, bpe, label in SCENARIOS:
        bytes_per = n * bpe
        for direction, suffix in (("parse", "_parse"), ("build", "_build")):
            full_key = key + suffix
            py_ns = py_data[full_key]
            rs_ns = rs_data[full_key]
            speedup = py_ns / rs_ns
            speedups.append((key, label, n, bpe, direction, py_ns, rs_ns, speedup))

            low_flag = " <4x!" if speedup < 4 else (" <10x" if speedup < 10 else "")
            if direction == "parse":
                print(f"{key[:5]:<5}{label:<35}{n:>7}{bytes_per:>13}"
                      f"{format_ns(py_ns):>16}{format_ns(rs_ns):>16}{speedup:>8.2f}x{low_flag}"
                      f"{direction:>8}")
            else:
                print(f"{'':<5}{'  └─ build':<35}{n:>7}{bytes_per:>13}"
                      f"{format_ns(py_ns):>16}{format_ns(rs_ns):>16}{speedup:>8.2f}x{low_flag}"
                      f"{direction:>8}")

    print("-" * 110)

    # 派生指标
    print()
    print("─── 派生指标 1：加速比按场景排序（由高到低） ───")
    sorted_sp = sorted(speedups, key=lambda x: -x[7])
    for key, label, n, _bpe, direction, _py, _rs, sp in sorted_sp:
        flag = " <4x!" if sp < 4 else (" <10x" if sp < 10 else "")
        print(f"  {label + ' ' + direction:<45} {sp:>7.2f}x{flag}")

    print()
    print("─── 派生指标 2：parse/build 比率（同实现内部） ───")
    print(f"  {'场景':<35}{'N':>7}{'py p/b':>10}{'rs p/b':>10}{'方向一致':>12}")
    for key, kind, n, bpe, label in SCENARIOS:
        py_p = py_data[key + "_parse"]
        py_b = py_data[key + "_build"]
        rs_p = rs_data[key + "_parse"]
        rs_b = rs_data[key + "_build"]
        py_r = py_p / py_b
        rs_r = rs_p / rs_b
        consistent = "Y" if (py_r > 1) == (rs_r > 1) else "N"
        print(f"  {label:<35}{n:>7}{py_r:>9.2f}x{rs_r:>9.2f}x{consistent:>12}")

    print()
    print("─── 派生指标 3：Int8ub 规模扩展（ns/元素） ───")
    print(f"  {'N':>7}{'py parse ns/elem':>20}{'rs parse ns/elem':>20}"
          f"{'py build ns/elem':>20}{'rs build ns/elem':>20}")
    scale_keys = ["g1_i8_n64", "g2_i8_n1024", "g3_i8_n8192"]
    n_for = dict((k[0], k[2]) for k in SCENARIOS)
    for key in scale_keys:
        n = n_for[key]
        py_p = py_data[key + "_parse"] * 1e9 / n
        rs_p = rs_data[key + "_parse"] * 1e9 / n
        py_b = py_data[key + "_build"] * 1e9 / n
        rs_b = rs_data[key + "_build"] * 1e9 / n
        print(f"  {n:>7}{py_p:>19.2f}{rs_p:>19.2f}{py_b:>19.2f}{rs_b:>19.2f}")

    print()
    print("─── 派生指标 4：等字节量级（~1KB）元素类型对比 ───")
    print(f"  {'元素类型':<22}{'N elems':>10}{'parse 加速比':>16}{'build 加速比':>16}")
    fixed_keys = [
        ("Int8ub (1B)",         "g2_i8_n1024"),
        ("Int32ub (4B)",        "g4_i32_n256"),
        ("Bytes(8) (8B)",       "g6_bytes8_n128"),
        ("Struct{2} (2B)",      "g7_struct2_n512"),
    ]
    for name, key in fixed_keys:
        n = n_for[key]
        sp_p = py_data[key + "_parse"] / rs_data[key + "_parse"]
        sp_b = py_data[key + "_build"] / rs_data[key + "_build"]
        print(f"  {name:<22}{n:>10}{sp_p:>15.2f}x{sp_b:>15.2f}x")

    # 异常检测
    print()
    print("─── 异常项检测 ───")
    below_4 = [(lbl, d, sp) for _, lbl, _, _, d, _, _, sp in speedups if sp < 4]
    below_10 = [(lbl, d, sp) for _, lbl, _, _, d, _, _, sp in speedups if 4 <= sp < 10]
    if below_4:
        print(f"  [WARN] 低于 4x 的场景 ({len(below_4)}):")
        for lbl, d, sp in below_4:
            print(f"    {lbl} {d}: {sp:.2f}x")
    else:
        print("  [PASS] 全部场景 >= 4x")
    if below_10:
        print(f"  [INFO] 4x-10x 之间的场景 ({len(below_10)}):")
        for lbl, d, sp in below_10:
            print(f"    {lbl} {d}: {sp:.2f}x")

    # S-PERF 出口
    target = 10.0
    print()
    print(f"─── S-PERF 出口标准 (≥{target}x) ───")
    all_met = all(sp >= target for *_x, sp in speedups)
    if all_met:
        print(f"  [PASS] 全部 {len(speedups)} 个场景 >= {target}x")
    else:
        below = [(lbl, d, sp) for _, lbl, _, _, d, _, _, sp in speedups if sp < target]
        print(f"  [WARN] {len(below)}/{len(speedups)} 场景低于 {target}x:")
        for lbl, d, sp in below:
            print(f"    {lbl} {d}: {sp:.2f}x")
    print("=" * 110)


if __name__ == "__main__":
    main()
