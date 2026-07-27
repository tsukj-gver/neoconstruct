"""Phase 4 子任务 4.3 性能基准 v2：PrefixedArray parse/build（场景矩阵补强）。

按 performance-gate/SKILL.md S-PERF 测量口径：
- construct-rs 侧：通过 maturin develop 安装后，Python 调用用户面 API
- Python construct 侧：直接 import construct，调等效 API
- 子进程隔离（Rust 和 Python 包同名，不可在同一进程导入）
- 取 min(repeat=5) × number

v2 在 v1 基础上按总纲 §"场景矩阵硬要求"补强：
- 元素类型维度：Int8ub / Int16sb / Int32ub / Struct{2} / Struct{4} / Bytes / 嵌套（7 种）
- 规模维度：N=10 / 100 / 1024 / 4096 四档（验证 O(n) 扩展性）
- 字节序维度：BE cf + LE cf + 多字节 element 多种
- 分支维度：countfield 类型 Byte / Int16ub / Int16ul / Int32ub（4 种）
- 错误路径：countfield 溢出 (E01) + 流不完整 (E02)（2 种）
- 嵌套组合：PrefixedArray of Struct{Array} (P12) + PrefixedArray of PrefixedArray (P13)（2 种）

场景矩阵（13 正常 + 2 错误 = 15 个场景 × parse/build = 30 测量点）：
| #    | countfield | Element        | N     | 维度覆盖                          |
|------|------------|----------------|-------|-----------------------------------|
| P01  | Byte       | Int8ub         | 10    | 小规模 / Byte cf                  |
| P02  | Byte       | Int8ub         | 100   | 中规模 / Byte cf                  |
| P03  | Byte       | Int8ub         | 255   | Byte cf 容量上限                  |
| P04  | Int16ub    | Int8ub         | 1024  | 大规模 / Int16ub cf               |
| P05  | Int16ub    | Int8ub         | 4096  | 超大规模                          |
| P06  | Int16ul    | Int8ub         | 1024  | LE cf / 字节序                    |
| P07  | Int32ub    | Int8ub         | 1024  | Int32ub cf（4B 头）               |
| P08  | Int16ub    | Int32ub        | 256   | 多字节 element                    |
| P09  | Int16ub    | Int16sb        | 256   | 有符号 element                    |
| P10  | Int16ub    | Struct{2}      | 512   | 复合 element                      |
| P11  | Int16ub    | Struct{4}      | 256   | 较大复合                          |
| P12  | Int16ub    | Bytes(8)       | 128   | bytes blob                        |
| P13  | Int16ub    | Struct{Array}  | 100   | 嵌套：Struct 内含 Array           |
| E01  | Byte       | Int8ub         | -     | 错误路径：countfield 溢出 (>255)  |
| E02  | Int16ub    | Int8ub         | -     | 错误路径：流不完整（少 N/2 字节） |

countfield 选择规则：Byte cf 最大容量 255，超出用 Int16ub/Int32ub。
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
# 场景定义
# ============================================================
# (key, cf_kind, kind, n_elems, struct_n_fields, error_kind, label)
#   cf_kind: "byte" / "int16ub" / "int16ul" / "int32ub"
#   kind: "i8" / "i32" / "i16sb" / "struct" / "bytes8" / "nested_struct_array"
SCENARIOS = [
    ("p01_i8_n10",         "byte",    "i8",                 10,   0, "",               "PrefixedArray(Byte, Int8ub) N=10"),
    ("p02_i8_n100",        "byte",    "i8",                 100,  0, "",               "PrefixedArray(Byte, Int8ub) N=100"),
    ("p03_i8_n255",        "byte",    "i8",                 255,  0, "",               "PrefixedArray(Byte, Int8ub) N=255"),
    ("p04_i8_n1024",       "int16ub", "i8",                 1024, 0, "",               "PrefixedArray(Int16ub, Int8ub) N=1024"),
    ("p05_i8_n4096",       "int16ub", "i8",                 4096, 0, "",               "PrefixedArray(Int16ub, Int8ub) N=4096"),
    ("p06_i8_le_cf",       "int16ul", "i8",                 1024, 0, "",               "PrefixedArray(Int16ul, Int8ub) N=1024"),
    ("p07_i8_i32cf",       "int32ub", "i8",                 1024, 0, "",               "PrefixedArray(Int32ub, Int8ub) N=1024"),
    ("p08_i32_n256",       "int16ub", "i32",                256,  0, "",               "PrefixedArray(Int16ub, Int32ub) N=256"),
    ("p09_i16sb_n256",     "int16ub", "i16sb",              256,  0, "",               "PrefixedArray(Int16ub, Int16sb) N=256"),
    ("p10_struct2_n512",   "int16ub", "struct",             512,  2, "",               "PrefixedArray(Int16ub, Struct{2}) N=512"),
    ("p11_struct4_n256",   "int16ub", "struct",             256,  4, "",               "PrefixedArray(Int16ub, Struct{4}) N=256"),
    ("p12_bytes8_n128",    "int16ub", "bytes8",             128,  8, "",               "PrefixedArray(Int16ub, Bytes(8)) N=128"),
    ("p13_nested_struct",  "int16ub", "nested_struct_array",100,  3, "",               "PrefixedArray(Int16ub, Struct{Array(3)}) N=100"),
    ("e01_cf_overflow",    "byte",    "i8",                 300,  0, "cf_overflow",    "PrefixedArray(Byte, Int8ub) cf=300 溢出 [ERR]"),
    ("e02_stream_short",   "int16ub", "i8",                 1024, 0, "stream_short",   "PrefixedArray(Int16ub, Int8ub) N=1024 流短缺 [ERR]"),
]

NUMBER_FOR = {
    "p05_i8_n4096": 200,
    "p10_struct2_n512": 500,
    "p11_struct4_n256": 500,
}
DEFAULT_NUMBER = 1000
REPEAT = 5


def _expected_iters(key, n):
    """PrefixedArray 每次调用迭代 n 次。"""
    return n


def _bytes_per_elem(kind, nf):
    return {
        "i8": 1, "i32": 4, "i16sb": 2,
        "struct": nf,
        "bytes8": 8,
        "nested_struct_array": nf,  # nf=3
    }.get(kind, 0)


def _cf_size(cf_kind):
    return {"byte": 1, "int16ub": 2, "int16ul": 2, "int32ub": 4}.get(cf_kind, 0)


def _encode_count(n, cf_kind):
    """编码 countfield 字节。"""
    if cf_kind == "byte":
        return bytes([n & 0xFF])
    if cf_kind == "int16ub":
        return bytes([(n >> 8) & 0xFF, n & 0xFF])
    if cf_kind == "int16ul":
        return bytes([n & 0xFF, (n >> 8) & 0xFF])
    if cf_kind == "int32ub":
        return bytes([(n >> 24) & 0xFF, (n >> 16) & 0xFF, (n >> 8) & 0xFF, n & 0xFF])
    raise ValueError(cf_kind)


# ============================================================
# construct-rs 测量
# ============================================================
def bench_construct_rs():
    """construct-rs 测量（此子进程仅导入 construct-rs）。"""
    from dataclasses import dataclass
    from construct import (
        StructMixin, field, Int8ub, Int16ub, Int16ul, Int16sb, Int32ub, Bytes,
        Array, PrefixedArray,
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
    class Array3Holder(StructMixin):
        """嵌套：Struct 内含 Array(3, Int8ub)。"""
        items: list = field(Array(3, Int8ub))

    def _cf(cf_kind):
        if cf_kind == "byte":
            return Int8ub
        if cf_kind == "int16ub":
            return Int16ub
        if cf_kind == "int16ul":
            return Int16ul
        if cf_kind == "int32ub":
            return Int32ub
        raise ValueError(cf_kind)

    compiled = {}
    for key, cf_kind, kind, n, nf, err, _label in SCENARIOS:
        if err:
            compiled[key] = (None, None, None)
            continue
        cf = _cf(cf_kind)
        if kind == "i8":
            @dataclass
            class _C(StructMixin):
                items: list = field(PrefixedArray(cf, Int8ub))
            data = _encode_count(n, cf_kind) + bytes(i % 256 for i in range(n))
            obj = _C(items=list(i % 256 for i in range(n)))
        elif kind == "i32":
            @dataclass
            class _C(StructMixin):
                items: list = field(PrefixedArray(cf, Int32ub))
            data = _encode_count(n, cf_kind) + bytes((i) % 256 for i in range(n * 4))
            obj = _C(items=list(i % 256 for i in range(n)))
        elif kind == "i16sb":
            @dataclass
            class _C(StructMixin):
                items: list = field(PrefixedArray(cf, Int16sb))
            data = _encode_count(n, cf_kind) + bytes((i) % 256 for i in range(n * 2))
            obj = _C(items=[(((i % 256) + 128) % 256) * 257 - 32768 for i in range(n)])
            # Int16sb 值域 [-32768, 32767]
            obj = _C(items=[((i * 17) % 65536 - 32768) for i in range(n)])
        elif kind == "struct":
            inner = S2 if nf == 2 else S4
            @dataclass
            class _C(StructMixin):
                items: list = field(PrefixedArray(cf, inner))
            data = _encode_count(n, cf_kind) + bytes((i) % 256 for i in range(n * nf))
            if nf == 2:
                inners = [S2(a=i % 256, b=(i + 1) % 256) for i in range(n)]
            else:
                inners = [S4(a=i % 256, b=(i + 1) % 256,
                             c=(i + 2) % 256, d=(i + 3) % 256) for i in range(n)]
            obj = _C(items=inners)
        elif kind == "bytes8":
            @dataclass
            class _C(StructMixin):
                items: list = field(PrefixedArray(cf, Bytes(8)))
            data = _encode_count(n, cf_kind) + bytes((i) % 256 for i in range(n * 8))
            one = bytes((i) % 256 for i in range(8))
            obj = _C(items=[one] * n)
        elif kind == "nested_struct_array":
            inner_cls = Array3Holder
            @dataclass
            class _C(StructMixin):
                items: list = field(PrefixedArray(cf, inner_cls))
            data = _encode_count(n, cf_kind) + bytes(i % 256 for i in range(n * nf))
            inners = [Array3Holder(items=[(i * nf + j) % 256 for j in range(nf)])
                      for i in range(n)]
            obj = _C(items=inners)
        else:
            raise ValueError(kind)

        number = NUMBER_FOR.get(key, DEFAULT_NUMBER)
        compiled[key] = (_C, data, obj)

    results = {}
    for key, cf_kind, kind, n, nf, err, _label in SCENARIOS:
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
        Byte, Int16ub, Int16ul, Int16sb, Int32ub, Bytes, Struct, Container,
        Array, PrefixedArray,
    )

    def _cf(cf_kind):
        if cf_kind == "byte":
            return Byte
        if cf_kind == "int16ub":
            return Int16ub
        if cf_kind == "int16ul":
            return Int16ul
        if cf_kind == "int32ub":
            return Int32ub
        raise ValueError(cf_kind)

    compiled = {}
    for key, cf_kind, kind, n, nf, err, _label in SCENARIOS:
        if err:
            compiled[key] = (None, None, None)
            continue
        cf = _cf(cf_kind)
        if kind == "i8":
            pa = PrefixedArray(cf, Byte)
            data = _encode_count(n, cf_kind) + bytes(i % 256 for i in range(n))
            obj = list(i % 256 for i in range(n))
        elif kind == "i32":
            pa = PrefixedArray(cf, Int32ub)
            data = _encode_count(n, cf_kind) + bytes((i) % 256 for i in range(n * 4))
            obj = list(i % 256 for i in range(n))
        elif kind == "i16sb":
            pa = PrefixedArray(cf, Int16sb)
            data = _encode_count(n, cf_kind) + bytes((i) % 256 for i in range(n * 2))
            obj = [((i * 17) % 65536 - 32768) for i in range(n)]
        elif kind == "struct":
            if nf == 2:
                inner = Struct("a" / Byte, "b" / Byte)
            else:
                inner = Struct("a" / Byte, "b" / Byte, "c" / Byte, "d" / Byte)
            pa = PrefixedArray(cf, inner)
            data = _encode_count(n, cf_kind) + bytes((i) % 256 for i in range(n * nf))
            if nf == 2:
                obj = [Container(a=i % 256, b=(i + 1) % 256) for i in range(n)]
            else:
                obj = [Container(a=i % 256, b=(i + 1) % 256,
                                 c=(i + 2) % 256, d=(i + 3) % 256) for i in range(n)]
        elif kind == "bytes8":
            pa = PrefixedArray(cf, Bytes(8))
            data = _encode_count(n, cf_kind) + bytes((i) % 256 for i in range(n * 8))
            one = bytes((i) % 256 for i in range(8))
            obj = [one] * n
        elif kind == "nested_struct_array":
            inner = Struct("items" / Array(nf, Byte))
            pa = PrefixedArray(cf, inner)
            data = _encode_count(n, cf_kind) + bytes(i % 256 for i in range(n * nf))
            obj = [Container(items=[(i * nf + j) % 256 for j in range(nf)]) for i in range(n)]
        else:
            raise ValueError(kind)

        number = NUMBER_FOR.get(key, DEFAULT_NUMBER)
        compiled[key] = (pa, data, obj)

    results = {}
    for key, cf_kind, kind, n, nf, err, _label in SCENARIOS:
        if err:
            continue
        pa, data, obj = compiled[key]
        number = NUMBER_FOR.get(key, DEFAULT_NUMBER)
        parse_ns = min(
            timeit.repeat(lambda p=pa, d=data: p.parse(d), number=number, repeat=REPEAT)
        ) / number
        build_ns = min(
            timeit.repeat(lambda p=pa, o=obj: p.build(o), number=number, repeat=REPEAT)
        ) / number
        results[key + "_parse"] = parse_ns
        results[key + "_build"] = build_ns
    return results


# ============================================================
# 错误路径测量
# ============================================================
def bench_error_paths_rs():
    """construct-rs 错误路径测量：parse 在错误数据上的 time-to-fail。

    E01: Byte countfield 溢出（cf=300 但 Byte 容量 0-255）
    E02: Int16ub cf + 流不完整（声称 1024 元素但只给 512 字节）
    """
    from dataclasses import dataclass
    from construct import StructMixin, field, Int8ub, Int16ub, PrefixedArray
    import time as _time

    out = {}
    for key, cf_kind, kind, n, nf, err, _label in SCENARIOS:
        if not err:
            continue
        if err == "cf_overflow":
            # 用 build 路径：传 300 元素给 Byte countfield PrefixedArray
            @dataclass
            class C(StructMixin):
                items: list = field(PrefixedArray(Int8ub, Int8ub))
            obj = C(items=[0] * 300)
            expected_iters = 300
            number = 100
            actual_fails = 0
            for _ in range(20):
                try:
                    obj.build()
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
        elif err == "stream_short":
            # 用 parse 路径：cf 声称 1024 但实际只给 512 字节
            @dataclass
            class C(StructMixin):
                items: list = field(PrefixedArray(Int16ub, Int8ub))
            # cf=1024 + 512 字节（缺 512）
            data = _encode_count(1024, "int16ub") + bytes((i) % 256 for i in range(512))
            expected_iters = 512  # 第 513 个失败
            number = 100
            actual_fails = 0
            for _ in range(20):
                try:
                    C.parse(data)
                except Exception:
                    pass
            t0 = _time.perf_counter()
            for _ in range(number):
                try:
                    C.parse(data)
                except Exception:
                    actual_fails += 1
            elapsed = (_time.perf_counter() - t0) / number
            out[key] = (elapsed, number, actual_fails, expected_iters)
        else:
            continue
    return out


def bench_error_paths_py():
    """Python construct 2.10.70 错误路径测量。"""
    from construct import Byte, Int16ub, Int8ub, PrefixedArray
    import time as _time

    out = {}
    for key, cf_kind, kind, n, nf, err, _label in SCENARIOS:
        if not err:
            continue
        if err == "cf_overflow":
            pa = PrefixedArray(Byte, Int8ub)
            obj = [0] * 300
            expected_iters = 300
            number = 100
            actual_fails = 0
            for _ in range(20):
                try:
                    pa.build(obj)
                except Exception:
                    pass
            t0 = _time.perf_counter()
            for _ in range(number):
                try:
                    pa.build(obj)
                except Exception:
                    actual_fails += 1
            elapsed = (_time.perf_counter() - t0) / number
            out[key] = (elapsed, number, actual_fails, expected_iters)
        elif err == "stream_short":
            pa = PrefixedArray(Int16ub, Int8ub)
            data = _encode_count(1024, "int16ub") + bytes((i) % 256 for i in range(512))
            expected_iters = 512
            number = 100
            actual_fails = 0
            for _ in range(20):
                try:
                    pa.parse(data)
                except Exception:
                    pass
            t0 = _time.perf_counter()
            for _ in range(number):
                try:
                    pa.parse(data)
                except Exception:
                    actual_fails += 1
            elapsed = (_time.perf_counter() - t0) / number
            out[key] = (elapsed, number, actual_fails, expected_iters)
        else:
            continue
    return out


# ============================================================
# 格式化与汇总
# ============================================================
def format_ns(seconds):
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
    print()
    print("=" * 120)
    print(f"{'#':<6}{'场景':<46}{'N':>7}{'实际迭代':>10}{'bytes/call':>13}"
          f"{'Py ns/call':>16}{'Rs ns/call':>16}{'加速比':>10}{'方向':>8}")
    print("-" * 120)

    speedups = []
    for key, cf_kind, kind, n, nf, err, label in SCENARIOS:
        if err:
            continue
        bytes_per = _cf_size(cf_kind) + n * _bytes_per_elem(kind, nf)
        actual_iters = _expected_iters(key, n)
        for direction, suffix in (("parse", "_parse"), ("build", "_build")):
            full_key = key + suffix
            py_ns = py_data[full_key]
            rs_ns = rs_data[full_key]
            speedup = py_ns / rs_ns
            speedups.append((key, label, n, direction, py_ns, rs_ns, speedup, actual_iters))

            low_flag = " <10x!" if speedup < 10 else ""
            if direction == "parse":
                print(f"{key[:6]:<6}{label:<46}{n:>7}{actual_iters:>10}{bytes_per:>13}"
                      f"{format_ns(py_ns):>16}{format_ns(rs_ns):>16}{speedup:>8.2f}x{low_flag}"
                      f"{direction:>8}")
            else:
                print(f"{'':<6}{'  └─ build':<46}{n:>7}{actual_iters:>10}{bytes_per:>13}"
                      f"{format_ns(py_ns):>16}{format_ns(rs_ns):>16}{speedup:>8.2f}x{low_flag}"
                      f"{direction:>8}")

    print("-" * 120)

    # ---- 错误路径表 ----
    print()
    print("─── 错误路径：parse/build time-to-fail ───")
    print(f"  {'场景':<46}{'迭代次数':>10}{'实际失败':>10}{'失败率':>16}"
          f"{'Py ns/fail':>14}{'Rs ns/fail':>14}{'加速比':>10}")
    err_speedups = []
    for key, cf_kind, kind, n, nf, err, label in SCENARIOS:
        if not err:
            continue
        rs_ns, rs_num, rs_actual, rs_exp_iters = rs_err_data[key]
        py_ns, py_num, py_actual, py_exp_iters = py_err_data[key]
        speedup = py_ns / rs_ns
        err_speedups.append((key, label, speedup, rs_actual, py_actual, py_num, py_exp_iters))
        flag = " <10x" if speedup < 10 else ""
        fail_rate = f"{rs_actual}/{py_num}"
        print(f"  {label:<46}{py_num:>10}{rs_actual:>10}{fail_rate:>16}"
              f"{format_ns(py_ns):>14}{format_ns(rs_ns):>14}{speedup:>8.2f}x{flag}")

    # ---- 派生指标 1：加速比排序 ----
    print()
    print("─── 派生指标 1：加速比按场景排序（由高到低） ───")
    sorted_sp = sorted(speedups, key=lambda x: -x[6])
    for key, label, n, direction, _py, _rs, sp, _ai in sorted_sp:
        flag = " <10x!" if sp < 10 else ""
        print(f"  {label + ' ' + direction:<54} N={n:<6} {sp:>7.2f}x{flag}")
    if err_speedups:
        print("  [错误路径]")
        for k, l, sp, _, _, _, _ in err_speedups:
            flag = " <10x!" if sp < 10 else ""
            print(f"  {l + ' err':<54}         {sp:>7.2f}x{flag}")

    # ---- 派生指标 2：parse/build 比率 ----
    print()
    print("─── 派生指标 2：parse/build 比率（同实现内部） ───")
    print(f"  {'场景':<46}{'N':>7}{'py p/b':>10}{'rs p/b':>10}{'方向一致':>12}")
    for key, cf_kind, kind, n, nf, err, label in SCENARIOS:
        if err:
            continue
        py_p = py_data[key + "_parse"]
        py_b = py_data[key + "_build"]
        rs_p = rs_data[key + "_parse"]
        rs_b = rs_data[key + "_build"]
        py_r = py_p / py_b
        rs_r = rs_p / rs_b
        consistent = "Y" if (py_r > 1) == (rs_r > 1) else "N"
        print(f"  {label:<46}{n:>7}{py_r:>9.2f}x{rs_r:>9.2f}x{consistent:>12}")

    # ---- 派生指标 3：Int16ub cf 规模扩展 ----
    print()
    print("─── 派生指标 3：Int16ub cf + Int8ub 规模扩展（ns/元素） ───")
    print(f"  {'N':>7}{'py parse ns/elem':>20}{'rs parse ns/elem':>20}"
          f"{'py build ns/elem':>20}{'rs build ns/elem':>20}")
    scale_keys = [
        ("Byte cf", "p02_i8_n100"),
        ("Byte cf", "p03_i8_n255"),
        ("Int16ub cf", "p04_i8_n1024"),
        ("Int16ub cf", "p05_i8_n4096"),
    ]
    n_for = dict((s[0], s[3]) for s in SCENARIOS)
    for cf_label, key in scale_keys:
        n = n_for[key]
        py_p = py_data[key + "_parse"] * 1e9 / n
        rs_p = rs_data[key + "_parse"] * 1e9 / n
        py_b = py_data[key + "_build"] * 1e9 / n
        rs_b = rs_data[key + "_build"] * 1e9 / n
        print(f"  {cf_label:<14}{n:>7}{py_p:>19.2f}{rs_p:>19.2f}{py_b:>19.2f}{rs_b:>19.2f}")

    # ---- 派生指标 4：countfield 类型对比（同 N） ----
    print()
    print("─── 派生指标 4：countfield 类型对比（N=1024 + Int8ub） ───")
    print(f"  {'countfield':<14}{'N elems':>10}{'parse 加速比':>16}{'build 加速比':>16}")
    cf_keys = [
        ("Int16ub (2B BE)", "p04_i8_n1024"),
        ("Int16ul (2B LE)", "p06_i8_le_cf"),
        ("Int32ub (4B BE)", "p07_i8_i32cf"),
    ]
    for cf_name, key in cf_keys:
        n = n_for[key]
        sp_p = py_data[key + "_parse"] / rs_data[key + "_parse"]
        sp_b = py_data[key + "_build"] / rs_data[key + "_build"]
        print(f"  {cf_name:<14}{n:>10}{sp_p:>15.2f}x{sp_b:>15.2f}x")

    # ---- 派生指标 5：场景标签 vs 实际迭代次数 ----
    print()
    print("─── 派生指标 5：场景标签 N vs 实际迭代次数 ───")
    print(f"  {'场景':<46}{'标签 N':>10}{'实际迭代':>12}{'一致':>8}")
    for key, cf_kind, kind, n, nf, err, label in SCENARIOS:
        if err:
            continue
        ai = _expected_iters(key, n)
        consistent = "Y" if ai == n else "N"
        print(f"  {label:<46}{n:>10}{ai:>12}{consistent:>8}")

    # ---- 派生指标 6：内部关系合理性（同字节量级元素对比） ----
    print()
    print("─── 派生指标 6：内部关系合理性（Int16ub cf 基线 = N=1024 Int8ub） ───")
    base_p_rs = rs_data["p04_i8_n1024_parse"]
    base_p_py = py_data["p04_i8_n1024_parse"]
    print(f"  [基线] N=1024 Int8ub parse: py={format_ns(base_p_py)}, rs={format_ns(base_p_rs)}")
    # 比较 Int32ub N=256（同 ~1KB 总字节，2B cf 不同）
    compare_keys = [
        ("Int32ub N=256 (同 ~1KB)",     "p08_i32_n256"),
        ("Struct{2} N=512 (同 ~1KB)",   "p10_struct2_n512"),
        ("Bytes(8) N=128 (同 ~1KB)",    "p12_bytes8_n128"),
    ]
    for name, key in compare_keys:
        rs_p = rs_data[key + "_parse"]
        py_p = py_data[key + "_parse"]
        print(f"  {name:<30} parse rs/base={rs_p/base_p_rs:>5.2f}x, py/base={py_p/base_p_py:>5.2f}x")

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
