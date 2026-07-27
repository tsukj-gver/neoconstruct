"""Phase 4 子任务 4.1 性能基准 v2：Array parse/build（多维场景）。

按 performance-gate/SKILL.md S-PERF 测量口径：
- construct-rs 侧：通过 maturin develop 安装后，Python 调用用户面 API
- Python construct 侧：直接 import construct，调等效 API
- 子进程隔离（Rust 和 Python 包同名，不可在同一进程导入）
- 取 min(repeat=5) × number

v2 多维覆盖（PM 驳回 v1 后补充）：
- 元素类型维度：Int8ub（1B 原子）/ Int32ub（4B 原子）/ Bytes(8)（8B blob）
                 / Struct{2×Int8ub}（2B 复合）/ Struct{4×Int8ub}（4B 复合）
- 规模维度：N=10 / 100 / 1000 / 10000
- 方向维度：parse / build

场景矩阵（12 个）：
| #  | Element            | N     | 字节/调用 | 维度作用                  |
|----|--------------------|-------|----------|--------------------------|
| A1 | Int8ub             | 10    | 10       | 极小规模基线              |
| A2 | Int8ub             | 100   | 100      | v1 基线（横向对比）       |
| A3 | Int8ub             | 1000  | 1000     | 规模扩展                  |
| A4 | Int8ub             | 10000 | 10000    | 大规模（仅原子支持）      |
| A5 | Int32ub            | 100   | 400      | 多字节原子                |
| A6 | Int32ub            | 1000  | 4000     | 多字节原子规模扩展        |
| A7 | Bytes(8)           | 100   | 800      | bytes blob 元素           |
| A8 | Bytes(8)           | 1000  | 8000     | bytes blob 规模扩展       |
| A9 | Struct{2×Int8ub}   | 100   | 200      | 复合元素（v1 基线）       |
| A10| Struct{2×Int8ub}   | 1000  | 2000     | 复合元素规模扩展          |
| A11| Struct{4×Int8ub}   | 100   | 400      | 较大复合元素              |
| A12| Struct{4×Int8ub}   | 1000  | 4000     | 较大复合元素规模扩展      |
"""

import sys
import timeit
import os

# 强制 stdout 用 utf-8，避免 Windows GBK 控制台无法编码 ✓/✗/中文等字符
try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")
except Exception:
    pass


# ============================================================
# 场景定义（元数据，rs/py 两侧共用）
# ============================================================
# 每个场景： (key, element_kind, N, struct_n_fields, label)
#   element_kind: "i8" / "i32" / "bytes8" / "struct"
#   struct_n_fields: 仅 element_kind=="struct" 用，取 2 或 4
SCENARIOS = [
    ("a01_i8_n10",       "i8",     10,    0, "Array(10, Int8ub)"),
    ("a02_i8_n100",      "i8",     100,   0, "Array(100, Int8ub)"),
    ("a03_i8_n1000",     "i8",     1000,  0, "Array(1000, Int8ub)"),
    ("a04_i8_n10000",    "i8",     10000, 0, "Array(10000, Int8ub)"),
    ("a05_i32_n100",     "i32",    100,   0, "Array(100, Int32ub)"),
    ("a06_i32_n1000",    "i32",    1000,  0, "Array(1000, Int32ub)"),
    ("a07_bytes8_n100",  "bytes8", 100,   0, "Array(100, Bytes(8))"),
    ("a08_bytes8_n1000", "bytes8", 1000,  0, "Array(1000, Bytes(8))"),
    ("a09_s2_n100",      "struct", 100,   2, "Array(100, Struct{2})"),
    ("a10_s2_n1000",     "struct", 1000,  2, "Array(1000, Struct{2})"),
    ("a11_s4_n100",      "struct", 100,   4, "Array(100, Struct{4})"),
    ("a12_s4_n1000",     "struct", 1000,  4, "Array(1000, Struct{4})"),
]

# 每个场景的 number 设定（大规模用较小 number 避免超时）
NUMBER_FOR = {
    "a04_i8_n10000": 100,
    "a06_i32_n1000": 500,
    "a08_bytes8_n1000": 500,
    "a10_s2_n1000": 500,
    "a12_s4_n1000": 500,
}
DEFAULT_NUMBER = 1000
REPEAT = 5


# ============================================================
# construct-rs 测量
# ============================================================
def bench_construct_rs():
    """construct-rs 测量（此子进程仅导入 construct-rs）。"""
    from dataclasses import dataclass
    from construct import (
        StructMixin, field, Int8ub, Int32ub, Bytes, Array,
    )

    @dataclass
    class S2(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)

    @dataclass
    class S4(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)
        c: int = field(Int8ub)
        d: int = field(Int8ub)

    # 为每个场景动态生成一个顶层 StructMixin 类
    # 用预编译的方式减少 lambda 内的查找开销（保持测量口径纯净）
    compiled = {}  # key -> (parse_callable, build_obj, data, number)

    for key, kind, n, nf, _label in SCENARIOS:
        if kind == "i8":
            @dataclass
            class _C(StructMixin):
                items: list = field(Array(n, Int8ub))
            data = bytes(i % 256 for i in range(n))
            obj = _C(items=list(i % 256 for i in range(n)))
        elif kind == "i32":
            @dataclass
            class _C(StructMixin):
                items: list = field(Array(n, Int32ub))
            data = bytes((i % 256) for i in range(n * 4))
            obj = _C(items=list(i % 256 for i in range(n)))
        elif kind == "bytes8":
            @dataclass
            class _C(StructMixin):
                items: list = field(Array(n, Bytes(8)))
            # n * 8 bytes
            data = bytes((i) % 256 for i in range(n * 8))
            one = bytes((i) % 256 for i in range(8))
            obj = _C(items=[one] * n)
        elif kind == "struct":
            inner = S2 if nf == 2 else S4
            @dataclass
            class _C(StructMixin):
                items: list = field(Array(n, inner))
            data = bytes((i) % 256 for i in range(n * nf))
            if nf == 2:
                inners = [S2(a=i % 256, b=(i + 1) % 256) for i in range(n)]
            else:
                inners = [S4(a=i % 256, b=(i+1) % 256, c=(i+2) % 256, d=(i+3) % 256) for i in range(n)]
            obj = _C(items=inners)
        else:
            raise ValueError(kind)

        number = NUMBER_FOR.get(key, DEFAULT_NUMBER)
        compiled[key] = (_C, data, obj, number)

    results = {}
    for key, _kind, _n, _nf, _label in SCENARIOS:
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
    """Python construct 2.10.70 测量（此子进程仅导入 construct）。"""
    from construct import (
        Struct, Array, Byte, Int32ub, Bytes, Container,
    )

    compiled = {}
    for key, kind, n, nf, _label in SCENARIOS:
        if kind == "i8":
            arr = Array(n, Byte)
            data = bytes(i % 256 for i in range(n))
            obj = list(i % 256 for i in range(n))
        elif kind == "i32":
            arr = Array(n, Int32ub)
            data = bytes((i % 256) for i in range(n * 4))
            obj = list(i % 256 for i in range(n))
        elif kind == "bytes8":
            arr = Array(n, Bytes(8))
            data = bytes((i) % 256 for i in range(n * 8))
            one = bytes((i) % 256 for i in range(8))
            obj = [one] * n
        elif kind == "struct":
            if nf == 2:
                inner = Struct("a" / Byte, "b" / Byte)
            else:
                inner = Struct("a" / Byte, "b" / Byte, "c" / Byte, "d" / Byte)
            arr = Array(n, inner)
            data = bytes((i) % 256 for i in range(n * nf))
            if nf == 2:
                obj = [Container(a=i % 256, b=(i + 1) % 256) for i in range(n)]
            else:
                obj = [Container(a=i % 256, b=(i+1) % 256, c=(i+2) % 256, d=(i+3) % 256) for i in range(n)]
        else:
            raise ValueError(kind)

        number = NUMBER_FOR.get(key, DEFAULT_NUMBER)
        compiled[key] = (arr, data, obj, number)

    results = {}
    for key, _kind, _n, _nf, _label in SCENARIOS:
        arr, data, obj, number = compiled[key]
        parse_ns = min(
            timeit.repeat(lambda a=arr, d=data: a.parse(d), number=number, repeat=REPEAT)
        ) / number
        build_ns = min(
            timeit.repeat(lambda a=arr, o=obj: a.build(o), number=number, repeat=REPEAT)
        ) / number
        results[key + "_parse"] = parse_ns
        results[key + "_build"] = build_ns
    return results


# ============================================================
# 格式化与汇总
# ============================================================
def format_ns(seconds):
    """秒 → 纳秒字符串。"""
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
        # crs_venv = construct-rs 安装；crs_venv_py = python construct 2.10.70 安装
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
    """输出多维性能数据表 + 派生指标。"""
    print()
    print("=" * 110)
    print(f"{'#':<5}{'场景':<28}{'N':>7}{'bytes/call':>13}"
          f"{'Py ns/call':>16}{'Rs ns/call':>16}{'加速比':>10}{'方向':>8}")
    print("-" * 110)

    # 主表（parse + build 合并展示，每场景两行）
    speedups = []  # (key, label, n, direction, py, rs, speedup)
    for key, kind, n, nf, label in SCENARIOS:
        if kind == "i8":
            bytes_per = n * 1
        elif kind == "i32":
            bytes_per = n * 4
        elif kind == "bytes8":
            bytes_per = n * 8
        elif kind == "struct":
            bytes_per = n * nf
        else:
            bytes_per = 0

        for direction, suffix in (("parse", "_parse"), ("build", "_build")):
            full_key = key + suffix
            py_ns = py_data[full_key]
            rs_ns = rs_data[full_key]
            speedup = py_ns / rs_ns
            speedups.append((key, label, n, direction, py_ns, rs_ns, speedup))

            low_flag = " <4x!" if speedup < 4 else (" <10x" if speedup < 10 else "")
            idx_str = "" if direction == "parse" else ""
            if direction == "parse":
                print(f"{key[:5]:<5}{label:<28}{n:>7}{bytes_per:>13}"
                      f"{format_ns(py_ns):>16}{format_ns(rs_ns):>16}{speedup:>8.2f}x{low_flag}"
                      f"{direction:>8}")
            else:
                print(f"{'':<5}{'  └─ build':<28}{n:>7}{bytes_per:>13}"
                      f"{format_ns(py_ns):>16}{format_ns(rs_ns):>16}{speedup:>8.2f}x{low_flag}"
                      f"{direction:>8}")

    print("-" * 110)

    # ---- 派生指标 ----
    print()
    print("─── 派生指标 1：加速比按场景排序（由高到低） ───")
    sorted_sp = sorted(speedups, key=lambda x: -x[6])
    for key, label, n, direction, _py, _rs, sp in sorted_sp:
        flag = " <4x!" if sp < 4 else (" <10x" if sp < 10 else "")
        print(f"  {label + ' ' + direction:<35} N={n:<6} {sp:>7.2f}x{flag}")

    print()
    print("─── 派生指标 2：parse/build 比率（同实现内部） ───")
    print(f"  {'场景':<28}{'N':>7}{'py p/b':>10}{'rs p/b':>10}{'方向一致':>12}")
    for key, kind, n, nf, label in SCENARIOS:
        py_p = py_data[key + "_parse"]
        py_b = py_data[key + "_build"]
        rs_p = rs_data[key + "_parse"]
        rs_b = rs_data[key + "_build"]
        py_r = py_p / py_b
        rs_r = rs_p / rs_b
        consistent = "Y" if (py_r > 1) == (rs_r > 1) else "N"
        print(f"  {label:<28}{n:>7}{py_r:>9.2f}x{rs_r:>9.2f}x{consistent:>12}")

    print()
    print("─── 派生指标 3：规模扩展趋势（Int8ub N=10/100/1000/10000，ns/元素） ───")
    print(f"  {'N':>7}{'py parse ns/elem':>20}{'rs parse ns/elem':>20}"
          f"{'py build ns/elem':>20}{'rs build ns/elem':>20}")
    scale_keys = ["a01_i8_n10", "a02_i8_n100", "a03_i8_n1000", "a04_i8_n10000"]
    for key in scale_keys:
        n = dict((k[0], k[2]) for k in SCENARIOS)[key]
        # 转换为 ns/elem 避免科学计数法精度问题
        py_p = py_data[key + "_parse"] * 1e9 / n
        rs_p = rs_data[key + "_parse"] * 1e9 / n
        py_b = py_data[key + "_build"] * 1e9 / n
        rs_b = rs_data[key + "_build"] * 1e9 / n
        print(f"  {n:>7}{py_p:>19.2f}{rs_p:>19.2f}{py_b:>19.2f}{rs_b:>19.2f}")

    print()
    print("─── 派生指标 4：元素类型复杂度趋势（N=100，parse 加速比） ───")
    type_compare = [
        ("Int8ub(1B)",        "a02_i8_n100"),
        ("Int32ub(4B)",       "a05_i32_n100"),
        ("Bytes(8)",          "a07_bytes8_n100"),
        ("Struct{2}(2B)",     "a09_s2_n100"),
        ("Struct{4}(4B)",     "a11_s4_n100"),
    ]
    for name, key in type_compare:
        py_p = py_data[key + "_parse"]
        rs_p = rs_data[key + "_parse"]
        sp = py_p / rs_p
        print(f"  {name:<20} parse 加速比: {sp:>6.2f}x")

    # ---- 异常检测 ----
    print()
    print("─── 异常项检测 ───")
    below_4 = [(lbl, d, sp) for _, lbl, _, d, _, _, sp in speedups if sp < 4]
    below_10 = [(lbl, d, sp) for _, lbl, _, d, _, _, sp in speedups if 4 <= sp < 10]
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

    # ---- S-PERF 出口判断 ----
    target = 10.0
    print()
    print(f"─── S-PERF 出口标准 (≥{target}x) ───")
    all_met = all(sp >= target for _, _, _, _, _, _, sp in speedups)
    if all_met:
        print(f"  [PASS] 全部 {len(speedups)} 个场景 >= {target}x")
    else:
        below = [(lbl, d, sp) for _, lbl, _, d, _, _, sp in speedups if sp < target]
        print(f"  [WARN] {len(below)}/{len(speedups)} 场景低于 {target}x:")
        for lbl, d, sp in below:
            print(f"    {lbl} {d}: {sp:.2f}x")
    print("=" * 110)


if __name__ == "__main__":
    main()
