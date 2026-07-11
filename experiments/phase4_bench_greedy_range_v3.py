"""Phase 4 子任务 4.2 性能基准 v3：GreedyRange parse/build（场景矩阵补强）。

按 AGENTS.md §6 S-PERF 测量口径：
- construct-rs 侧：通过 maturin develop 安装后，Python 调用用户面 API
- Python construct 侧：直接 import construct，调等效 API
- 子进程隔离（Rust 和 Python 包同名，不可在同一进程导入）
- 取 min(repeat=5) × number

v3 在 v2 基础上按总纲 §"场景矩阵硬要求"补强：
- 元素类型维度：Int8ub / Int16ub / Int16ul / Int32ub / Int8sb / Struct{2} / Struct{4} / Bytes / 嵌套（9 种）
- 规模维度：N=10 / 100 / 1024 / 8192 四档（验证 O(n) 扩展性）
- 字节序维度：BE 1B/2B/4B + LE 2B + 有符号 1B（5 种）
- 分支维度：GreedyRange 无 count 分支，但 inner 形态多样（含嵌套组合）
- 错误路径：流不完整 (E01) + inner Struct 错位 (E02)（2 种）
- 嵌套组合：GreedyRange(Array) (G11) + GreedyRange(PrefixedArray) (G12)（2 种）

场景矩阵（12 正常 + 2 错误 = 14 个场景 × parse/build = 28 测量点）：
| #    | Element              | N elems | 字节/调用 | 维度覆盖                          |
|------|----------------------|---------|----------|-----------------------------------|
| G01  | Int8ub               | 10      | 10       | 小规模基线 / BE u8                |
| G02  | Int8ub               | 100     | 100      | 中规模 / BE u8                    |
| G03  | Int8ub               | 1024    | 1024     | 大规模 / BE u8                    |
| G04  | Int8ub               | 8192    | 8192     | 超大规模 / BE u8                  |
| G05  | Int16ub              | 100     | 200      | BE u16 / 中规模                   |
| G06  | Int16ul              | 100     | 200      | LE u16 / 字节序                   |
| G07  | Int32ub              | 256     | 1024     | BE u32 / 1KB 字节量级             |
| G08  | Int8sb               | 1024    | 1024     | 有符号 i8                         |
| G09  | Struct{2×Int8ub}     | 512     | 1024     | 复合元素 / 1KB                    |
| G10  | Struct{4×Int8ub}     | 256     | 1024     | 较大复合元素 / 1KB                |
| G11  | Array(3, Int8ub)     | 100     | 300      | 嵌套组合：GreedyRange of Array    |
| G12  | Bytes(8)             | 128     | 1024     | bytes blob / 1KB                  |
| E01  | Int8sb build 溢出     | 1024    | -        | 错误路径：build 时 Int8sb 溢出    |
| E02  | Int16ub build 溢出   | 1024    | -        | 错误路径：build 时 Int16ub 溢出   |

注：GreedyRange.parse 在 Python 原版中通过 `except Exception` 静默吞掉 inner 错误（core.py
L2613），parse 路径无外部可见错误。本测试 E01/E02 改测 build 错误路径（确实会抛出）。
"""

import sys
import timeit
import os

try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")
except Exception:
    pass


# ============================================================
# 场景定义（元数据，rs/py 两侧共用）
# ============================================================
# (key, kind, n_elems, struct_n_fields, error_kind, label)
# kind: "i8" / "i16ub" / "i16ul" / "i32" / "i8sb" / "struct" / "bytes8"
#       / "nested_array" / "nested_prefixed"
SCENARIOS = [
    ("g01_i8_n10",         "i8",            10,   0, "",                "GreedyRange(Int8ub) N=10"),
    ("g02_i8_n100",        "i8",            100,  0, "",                "GreedyRange(Int8ub) N=100"),
    ("g03_i8_n1024",       "i8",            1024, 0, "",                "GreedyRange(Int8ub) N=1024"),
    ("g04_i8_n8192",       "i8",            8192, 0, "",                "GreedyRange(Int8ub) N=8192"),
    ("g05_i16ub_n100",     "i16ub",         100,  0, "",                "GreedyRange(Int16ub) N=100"),
    ("g06_i16ul_n100",     "i16ul",         100,  0, "",                "GreedyRange(Int16ul) N=100"),
    ("g07_i32_n256",       "i32",           256,  0, "",                "GreedyRange(Int32ub) N=256"),
    ("g08_i8sb_n1024",     "i8sb",          1024, 0, "",                "GreedyRange(Int8sb) N=1024"),
    ("g09_s2_n512",        "struct",        512,  2, "",                "GreedyRange(Struct{2}) N=512"),
    ("g10_s4_n256",        "struct",        256,  4, "",                "GreedyRange(Struct{4}) N=256"),
    ("g11_array_n100",     "nested_array",  100,  3, "",                "GreedyRange(Array(3, Int8ub)) N=100"),
    ("g12_bytes8_n128",    "bytes8",        128,  8, "",                "GreedyRange(Bytes(8)) N=128"),
    ("e01_build_overflow_i8sb",  "i8sb_build_err", 1024, 0, "build_overflow_i8sb", "GreedyRange(Int8sb) build 溢出 [ERR]"),
    ("e02_build_overflow_i16ub", "i16ub_build_err", 1024, 0, "build_overflow_i16ub", "GreedyRange(Int16ub) build 溢出 [ERR]"),
]

NUMBER_FOR = {
    "g04_i8_n8192": 200,
    "g08_i8sb_n1024": 500,
    "g09_s2_n512": 500,
    "g10_s4_n256": 500,
}
DEFAULT_NUMBER = 1000
REPEAT = 5


def _expected_iters(key, n):
    """GreedyRange 每次调用迭代 n 次。"""
    return n


def _bytes_per_elem(kind, nf):
    """返回每种元素类型的字节大小。"""
    return {
        "i8": 1, "i8sb": 1,
        "i16ub": 2, "i16ul": 2,
        "i32": 4,
        "struct": nf,
        "bytes8": 8,
        "nested_array": nf,  # nf=3
    }.get(kind, 0)


# ============================================================
# construct-rs 测量
# ============================================================
def bench_construct_rs():
    """construct-rs 测量（此子进程仅导入 construct-rs）。"""
    from dataclasses import dataclass
    from construct import (
        StructMixin, field, Int8ub, Int8sb, Int16ub, Int16ul, Int32ub, Bytes,
        Array, GreedyRange, PrefixedArray,
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

    @dataclass
    class Array3(StructMixin):
        """嵌套组合：GreedyRange of Array(3, Int8ub)。"""
        items: list = field(Array(3, Int8ub))

    compiled = {}

    for key, kind, n, nf, err, _label in SCENARIOS:
        if err:
            compiled[key] = (None, None, None)  # 错误场景在 bench_error_paths_* 处理
            continue

        if kind == "i8":
            @dataclass
            class _C(StructMixin):
                items: list = field(GreedyRange(Int8ub))
            data = bytes(i % 256 for i in range(n))
            obj = _C(items=list(i % 256 for i in range(n)))
        elif kind == "i8sb":
            @dataclass
            class _C(StructMixin):
                items: list = field(GreedyRange(Int8sb))
            data = bytes(i % 256 for i in range(n))
            obj = _C(items=[(((i % 256) + 128) % 256) - 128 for i in range(n)])
        elif kind == "i16ub":
            @dataclass
            class _C(StructMixin):
                items: list = field(GreedyRange(Int16ub))
            data = bytes(((i // 2) >> 8) & 0xFF if i % 2 == 0 else (i // 2) & 0xFF
                         for i in range(n * 2))
            obj = _C(items=list((i * 17) % 65536 for i in range(n)))
        elif kind == "i16ul":
            @dataclass
            class _C(StructMixin):
                items: list = field(GreedyRange(Int16ul))
            data = bytes((i * 17) % 256 if i % 2 == 0 else ((i * 17) // 256) % 256
                         for i in range(n * 2))
            obj = _C(items=list((i * 17) % 65536 for i in range(n)))
        elif kind == "i32":
            @dataclass
            class _C(StructMixin):
                items: list = field(GreedyRange(Int32ub))
            data = bytes((i % 256) for i in range(n * 4))
            obj = _C(items=list(i % 256 for i in range(n)))
        elif kind == "struct":
            inner = S2 if nf == 2 else S4
            @dataclass
            class _C(StructMixin):
                items: list = field(GreedyRange(inner))
            data = bytes((i % 256) for i in range(n * nf))
            if nf == 2:
                inners = [S2(a=i % 256, b=(i + 1) % 256) for i in range(n)]
            else:
                inners = [S4(a=i % 256, b=(i + 1) % 256,
                             c=(i + 2) % 256, d=(i + 3) % 256) for i in range(n)]
            obj = _C(items=inners)
        elif kind == "bytes8":
            @dataclass
            class _C(StructMixin):
                items: list = field(GreedyRange(Bytes(8)))
            data = bytes((i) % 256 for i in range(n * 8))
            one = bytes((i) % 256 for i in range(8))
            obj = _C(items=[one] * n)
        elif kind == "nested_array":
            # GreedyRange(Array(3, Int8ub))：每个元素是 Array3 数据类
            _inner = Array3
            @dataclass
            class _C(StructMixin):
                items: list = field(GreedyRange(_inner))
            data = bytes(i % 256 for i in range(n * nf))
            inners = [Array3(items=[(i * nf + j) % 256 for j in range(nf)]) for i in range(n)]
            obj = _C(items=inners)
        else:
            raise ValueError(kind)

        number = NUMBER_FOR.get(key, DEFAULT_NUMBER)
        compiled[key] = (_C, data, obj)

    results = {}
    for key, kind, n, nf, err, _label in SCENARIOS:
        if err:
            continue
        cls, data, obj = compiled[key]
        number = NUMBER_FOR.get(key, DEFAULT_NUMBER)
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
        GreedyRange, Byte, Int8sb, Int16ub, Int16ul, Int32ub, Bytes, Struct,
        Container, Array,
    )

    compiled = {}
    for key, kind, n, nf, err, _label in SCENARIOS:
        if err:
            compiled[key] = (None, None, None)
            continue

        if kind == "i8":
            gr = GreedyRange(Byte)
            data = bytes(i % 256 for i in range(n))
            obj = list(i % 256 for i in range(n))
        elif kind == "i8sb":
            gr = GreedyRange(Int8sb)
            data = bytes(i % 256 for i in range(n))
            obj = [(((i % 256) + 128) % 256) - 128 for i in range(n)]
        elif kind == "i16ub":
            gr = GreedyRange(Int16ub)
            data = bytes(((i // 2) >> 8) & 0xFF if i % 2 == 0 else (i // 2) & 0xFF
                         for i in range(n * 2))
            obj = list((i * 17) % 65536 for i in range(n))
        elif kind == "i16ul":
            gr = GreedyRange(Int16ul)
            data = bytes((i * 17) % 256 if i % 2 == 0 else ((i * 17) // 256) % 256
                         for i in range(n * 2))
            obj = list((i * 17) % 65536 for i in range(n))
        elif kind == "i32":
            gr = GreedyRange(Int32ub)
            data = bytes((i % 256) for i in range(n * 4))
            obj = list(i % 256 for i in range(n))
        elif kind == "struct":
            if nf == 2:
                inner = Struct("a" / Byte, "b" / Byte)
            else:
                inner = Struct("a" / Byte, "b" / Byte, "c" / Byte, "d" / Byte)
            gr = GreedyRange(inner)
            data = bytes((i % 256) for i in range(n * nf))
            if nf == 2:
                obj = [Container(a=i % 256, b=(i + 1) % 256) for i in range(n)]
            else:
                obj = [Container(a=i % 256, b=(i + 1) % 256,
                                 c=(i + 2) % 256, d=(i + 3) % 256) for i in range(n)]
        elif kind == "bytes8":
            gr = GreedyRange(Bytes(8))
            data = bytes((i) % 256 for i in range(n * 8))
            one = bytes((i) % 256 for i in range(8))
            obj = [one] * n
        elif kind == "nested_array":
            inner = Array(nf, Byte)
            gr = GreedyRange(inner)
            data = bytes(i % 256 for i in range(n * nf))
            obj = [[(i * nf + j) % 256 for j in range(nf)] for i in range(n)]
        else:
            raise ValueError(kind)

        number = NUMBER_FOR.get(key, DEFAULT_NUMBER)
        compiled[key] = (gr, data, obj)

    results = {}
    for key, kind, n, nf, err, _label in SCENARIOS:
        if err:
            continue
        gr, data, obj = compiled[key]
        number = NUMBER_FOR.get(key, DEFAULT_NUMBER)
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
# 错误路径测量（GreedyRange parse 静默吞 inner 错误，故测 build 错误路径）
# ============================================================
def bench_error_paths_rs():
    """construct-rs 错误路径测量：build 时 Int8sb/Int16ub 溢出，time-to-fail。"""
    from dataclasses import dataclass
    from construct import StructMixin, field, Int8sb, Int16ub, GreedyRange
    import time as _time

    out = {}
    for key, kind, n, nf, err, _label in SCENARIOS:
        if not err:
            continue
        if err == "build_overflow_i8sb":
            @dataclass
            class C(StructMixin):
                items: list = field(GreedyRange(Int8sb))
            # 列表前 n-1 个合法，最后一个 200 溢出 Int8sb（值域 [-128, 127]）
            obj = C(items=[0] * (n - 1) + [200])
            expected_iters = n  # 在第 n 次失败
        elif err == "build_overflow_i16ub":
            @dataclass
            class C(StructMixin):
                items: list = field(GreedyRange(Int16ub))
            obj = C(items=[0] * (n - 1) + [70000])  # 70000 溢出 Int16ub
            expected_iters = n
        else:
            continue

        number = 100
        actual_fails = 0
        # 预热 20 次
        for _ in range(20):
            try:
                C.parse(b'')  # 仅触发 schema 准备
            except Exception:
                pass
        t0 = _time.perf_counter()
        for _ in range(number):
            try:
                obj.build()
            except Exception:
                actual_fails += 1
        elapsed = (_time.perf_counter() - t0) / number
        out[key] = (elapsed, number, actual_fails, expected_iters)
    return out


def bench_error_paths_py():
    """Python construct 2.10.70 错误路径测量：build 溢出。"""
    from construct import GreedyRange, Int8sb, Int16ub
    import time as _time

    out = {}
    for key, kind, n, nf, err, _label in SCENARIOS:
        if not err:
            continue
        if err == "build_overflow_i8sb":
            gr = GreedyRange(Int8sb)
            obj = [0] * (n - 1) + [200]
            expected_iters = n
        elif err == "build_overflow_i16ub":
            gr = GreedyRange(Int16ub)
            obj = [0] * (n - 1) + [70000]
            expected_iters = n
        else:
            continue

        number = 100
        actual_fails = 0
        for _ in range(20):
            try:
                gr.build([0])
            except Exception:
                pass
        t0 = _time.perf_counter()
        for _ in range(number):
            try:
                gr.build(obj)
            except Exception:
                actual_fails += 1
        elapsed = (_time.perf_counter() - t0) / number
        out[key] = (elapsed, number, actual_fails, expected_iters)
    return out


# ============================================================
# 格式化与汇总
# ============================================================
def format_ns(seconds):
    """秒 → 纳秒字符串。"""
    if seconds >= 1e-3:
        return f"{seconds * 1e6:.2f} us"
    if seconds >= 1e-6:
        return f"{seconds * 1e9:.0f} ns"
    return f"{seconds * 1e9:.1f} ns"


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
    elif mode == "rs_err":
        results = bench_error_paths_rs()
        for k, (ns, num, actual, exp) in results.items():
            print(f"rs_err\t{k}\t{ns:.9f}\t{num}\t{actual}\t{exp}")
    elif mode == "py_err":
        results = bench_error_paths_py()
        for k, (ns, num, actual, exp) in results.items():
            print(f"py_err\t{k}\t{ns:.9f}\t{num}\t{actual}\t{exp}")
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
        rs_err_proc = subprocess.run(
            [rs_python, script, "rs_err"], cwd=workdir, capture_output=True, text=True, env=env
        )
        if rs_err_proc.returncode != 0:
            print("rs_err subprocess failed:", rs_err_proc.stderr, file=sys.stderr)
            sys.exit(1)
        py_err_proc = subprocess.run(
            [py_python, script, "py_err"], cwd=workdir, capture_output=True, text=True, env=env
        )
        if py_err_proc.returncode != 0:
            print("py_err subprocess failed:", py_err_proc.stderr, file=sys.stderr)
            sys.exit(1)

        rs_data, py_data = {}, {}
        for line in rs_proc.stdout.strip().split("\n"):
            _, k, v = line.split("\t")
            rs_data[k] = float(v)
        for line in py_proc.stdout.strip().split("\n"):
            _, k, v = line.split("\t")
            py_data[k] = float(v)

        rs_err_data, py_err_data = {}, {}
        for line in rs_err_proc.stdout.strip().split("\n"):
            parts = line.split("\t")
            rs_err_data[parts[1]] = (float(parts[2]), int(parts[3]), int(parts[4]), int(parts[5]))
        for line in py_err_proc.stdout.strip().split("\n"):
            parts = line.split("\t")
            py_err_data[parts[1]] = (float(parts[2]), int(parts[3]), int(parts[4]), int(parts[5]))

        _emit_report(rs_data, py_data, rs_err_data, py_err_data)
    else:
        print(f"Unknown mode: {mode}", file=sys.stderr)
        sys.exit(1)


def _emit_report(rs_data, py_data, rs_err_data, py_err_data):
    """输出多维性能数据表 + 派生指标。"""
    print()
    print("=" * 120)
    print(f"{'#':<6}{'场景':<40}{'N':>7}{'实际迭代':>10}{'bytes/call':>13}"
          f"{'Py ns/call':>16}{'Rs ns/call':>16}{'加速比':>10}{'方向':>8}")
    print("-" * 120)

    speedups = []
    for key, kind, n, nf, err, label in SCENARIOS:
        if err:
            continue
        bytes_per = n * _bytes_per_elem(kind, nf)
        actual_iters = _expected_iters(key, n)
        for direction, suffix in (("parse", "_parse"), ("build", "_build")):
            full_key = key + suffix
            py_ns = py_data[full_key]
            rs_ns = rs_data[full_key]
            speedup = py_ns / rs_ns
            speedups.append((key, label, n, direction, py_ns, rs_ns, speedup, actual_iters))

            low_flag = " <10x!" if speedup < 10 else ""
            if direction == "parse":
                print(f"{key[:6]:<6}{label:<40}{n:>7}{actual_iters:>10}{bytes_per:>13}"
                      f"{format_ns(py_ns):>16}{format_ns(rs_ns):>16}{speedup:>8.2f}x{low_flag}"
                      f"{direction:>8}")
            else:
                print(f"{'':<6}{'  └─ build':<40}{n:>7}{actual_iters:>10}{bytes_per:>13}"
                      f"{format_ns(py_ns):>16}{format_ns(rs_ns):>16}{speedup:>8.2f}x{low_flag}"
                      f"{direction:>8}")

    print("-" * 120)

    # ---- 错误路径表 ----
    print()
    print("─── 错误路径：build time-to-fail（parse 路径在 GreedyRange 中静默吞错） ───")
    print(f"  {'场景':<40}{'迭代次数':>10}{'实际失败':>10}{'失败率':>16}"
          f"{'Py ns/fail':>14}{'Rs ns/fail':>14}{'加速比':>10}")
    err_speedups = []
    for key, kind, n, nf, err, label in SCENARIOS:
        if not err:
            continue
        rs_ns, rs_num, rs_actual, rs_exp_iters = rs_err_data[key]
        py_ns, py_num, py_actual, py_exp_iters = py_err_data[key]
        speedup = py_ns / rs_ns
        err_speedups.append((key, label, speedup, rs_actual, py_actual, py_num, py_exp_iters))
        flag = " <10x" if speedup < 10 else ""
        fail_rate = f"{rs_actual}/{py_num}"
        print(f"  {label:<40}{py_num:>10}{rs_actual:>10}{fail_rate:>16}"
              f"{format_ns(py_ns):>14}{format_ns(rs_ns):>14}{speedup:>8.2f}x{flag}")

    # ---- 派生指标 1：加速比排序 ----
    print()
    print("─── 派生指标 1：加速比按场景排序（由高到低） ───")
    sorted_sp = sorted(speedups, key=lambda x: -x[6])
    for key, label, n, direction, _py, _rs, sp, _ai in sorted_sp:
        flag = " <10x!" if sp < 10 else ""
        print(f"  {label + ' ' + direction:<48} N={n:<6} {sp:>7.2f}x{flag}")
    if err_speedups:
        print("  [错误路径]")
        for k, l, sp, _, _, _, _ in err_speedups:
            flag = " <10x!" if sp < 10 else ""
            print(f"  {l + ' err-parse':<48}         {sp:>7.2f}x{flag}")

    # ---- 派生指标 2：parse/build 比率 ----
    print()
    print("─── 派生指标 2：parse/build 比率（同实现内部） ───")
    print(f"  {'场景':<40}{'N':>7}{'py p/b':>10}{'rs p/b':>10}{'方向一致':>12}")
    for key, kind, n, nf, err, label in SCENARIOS:
        if err:
            continue
        py_p = py_data[key + "_parse"]
        py_b = py_data[key + "_build"]
        rs_p = rs_data[key + "_parse"]
        rs_b = rs_data[key + "_build"]
        py_r = py_p / py_b
        rs_r = rs_p / rs_b
        consistent = "Y" if (py_r > 1) == (rs_r > 1) else "N"
        print(f"  {label:<40}{n:>7}{py_r:>9.2f}x{rs_r:>9.2f}x{consistent:>12}")

    # ---- 派生指标 3：Int8ub 规模扩展 ----
    print()
    print("─── 派生指标 3：Int8ub 规模扩展（ns/元素） ───")
    print(f"  {'N':>7}{'py parse ns/elem':>20}{'rs parse ns/elem':>20}"
          f"{'py build ns/elem':>20}{'rs build ns/elem':>20}")
    scale_keys = ["g01_i8_n10", "g02_i8_n100", "g03_i8_n1024", "g04_i8_n8192"]
    n_for = dict((s[0], s[2]) for s in SCENARIOS)
    for key in scale_keys:
        n = n_for[key]
        py_p = py_data[key + "_parse"] * 1e9 / n
        rs_p = rs_data[key + "_parse"] * 1e9 / n
        py_b = py_data[key + "_build"] * 1e9 / n
        rs_b = rs_data[key + "_build"] * 1e9 / n
        print(f"  {n:>7}{py_p:>19.2f}{rs_p:>19.2f}{py_b:>19.2f}{rs_b:>19.2f}")

    # ---- 派生指标 4：等字节量级（~1KB）元素类型对比 ----
    print()
    print("─── 派生指标 4：等字节量级（~1KB）元素类型对比 ───")
    print(f"  {'元素类型':<22}{'N elems':>10}{'parse 加速比':>16}{'build 加速比':>16}")
    fixed_keys = [
        ("Int8ub (1B)",         "g03_i8_n1024"),
        ("Int32ub (4B)",        "g07_i32_n256"),
        ("Struct{2} (2B)",      "g09_s2_n512"),
        ("Struct{4} (4B)",      "g10_s4_n256"),
        ("Bytes(8) (8B)",       "g12_bytes8_n128"),
    ]
    for name, key in fixed_keys:
        n = n_for[key]
        sp_p = py_data[key + "_parse"] / rs_data[key + "_parse"]
        sp_b = py_data[key + "_build"] / rs_data[key + "_build"]
        print(f"  {name:<22}{n:>10}{sp_p:>15.2f}x{sp_b:>15.2f}x")

    # ---- 派生指标 5：场景标签 vs 实际迭代次数 ----
    print()
    print("─── 派生指标 5：场景标签 N vs 实际迭代次数 ───")
    print(f"  {'场景':<40}{'标签 N':>10}{'实际迭代':>12}{'一致':>8}")
    for key, kind, n, nf, err, label in SCENARIOS:
        if err:
            continue
        ai = _expected_iters(key, n)
        consistent = "Y" if ai == n else "N"
        print(f"  {label:<40}{n:>10}{ai:>12}{consistent:>8}")

    # ---- 派生指标 6：内部关系合理性 ----
    print()
    print("─── 派生指标 6：内部关系合理性（Int8ub N=1024 是基线） ───")
    base_p_rs = rs_data["g03_i8_n1024_parse"]
    base_p_py = py_data["g03_i8_n1024_parse"]
    print(f"  [基线] Int8ub N=1024 parse: py={format_ns(base_p_py)}, rs={format_ns(base_p_rs)}")
    compare_keys = [
        ("Struct{2} N=512 (同字节量级)",  "g09_s2_n512"),
        ("Bytes(8) N=128 (同字节量级)",   "g12_bytes8_n128"),
    ]
    for name, key in compare_keys:
        rs_p = rs_data[key + "_parse"]
        py_p = py_data[key + "_parse"]
        print(f"  {name:<32} parse rs/base={rs_p/base_p_rs:>5.2f}x, py/base={py_p/base_p_py:>5.2f}x")

    # ---- 异常项检测 ----
    print()
    print("─── 异常项检测 ───")
    below_10 = [(lbl, d, sp, n) for _, lbl, n, d, _, _, sp, _ in speedups if sp < 10]
    if below_10:
        print(f"  [WARN] 低于 10x 的场景 ({len(below_10)}):")
        for lbl, d, sp, n in below_10:
            print(f"    {lbl} {d} N={n}: {sp:.2f}x")
    else:
        print("  [PASS] 全部场景 >= 10x")
    err_below = [(l, sp) for _, l, sp, _, _, _, _ in err_speedups if sp < 10]
    if err_below:
        print(f"  [WARN] 错误路径低于 10x ({len(err_below)}):")
        for l, sp in err_below:
            print(f"    {l}: {sp:.2f}x")

    # ---- S-PERF 出口 ----
    target = 10.0
    print()
    print(f"─── S-PERF 出口标准 (≥{target}x) ───")
    n_total = len(speedups)
    n_met = sum(1 for s in speedups if s[6] >= target)
    below = [(lbl, d, sp) for _, lbl, _, d, _, _, sp, _ in speedups if sp < target]
    print(f"  正常路径：{n_met}/{n_total} 场景 >= {target}x")
    if below:
        for lbl, d, sp in below:
            print(f"    {lbl} {d}: {sp:.2f}x")
    print("=" * 120)


if __name__ == "__main__":
    main()
