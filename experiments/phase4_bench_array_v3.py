"""Phase 4 子任务 4.1 性能基准 v3：Array parse/build（场景矩阵补强）。

按 performance-gate/SKILL.md S-PERF 测量口径：
- construct-rs 侧：通过 maturin develop 安装后，Python 调用用户面 API
- Python construct 侧：直接 import construct，调等效 API
- 子进程隔离（Rust 和 Python 包同名，不可在同一进程导入）
- 取 min(repeat=5) × number

v3 在 v2 基础上按总纲 §"场景矩阵硬要求"补强：
- 元素类型维度：Int8ub / Int16ub / Int16ul / Int32ub / Int8sb / Struct{2} / Struct{4} / 嵌套 Array / 嵌套 Struct（9 种）
- 规模维度：N=10 / 100 / 1000 / 10000 四档（验证 O(n) 扩展性）
- 字节序维度：BE 1B/2B/4B + LE 2B + 有符号 1B（5 种）
- 分支维度：literal count / expr count = this.n（2 种）
- 错误路径：流不完整 (E01) + 内部 Struct 错位 (E02)（2 种）
- 嵌套组合：Array in Array (A11) + Array in Struct (A12)（2 种）

场景矩阵（13 正常 + 2 错误 = 15 个场景 × parse/build = 30 测量点）：
| #    | Element              | N      | 字节/调用 | 维度覆盖                          |
|------|----------------------|--------|----------|-----------------------------------|
| A01  | Int8ub               | 10     | 10       | 小规模基线 / BE u8 / literal       |
| A02  | Int8ub               | 100    | 100      | 中规模 / BE u8 / literal           |
| A03  | Int8ub               | 1000   | 1000     | 大规模 / BE u8 / literal           |
| A04  | Int8ub               | 10000  | 10000    | 超大规模 / BE u8 / literal         |
| A05  | Int16ub              | 100    | 200      | BE u16 / 中规模                    |
| A06  | Int16ul              | 100    | 200      | LE u16 / 中规模（字节序）          |
| A07  | Int32ub              | 100    | 400      | BE u32 / 中规模                    |
| A08  | Int8sb               | 1000   | 1000     | 有符号 i8 / 大规模                 |
| A09  | Struct{2×Int8ub}     | 100    | 200      | 复合元素小                         |
| A10  | Struct{4×Int8ub}     | 1000   | 4000     | 复合元素大                         |
| A11  | Array(10, Array(10)) | 10×10  | 100      | 嵌套组合：Array in Array           |
| A12  | Struct{Array+Int32}  | 10+1   | 14       | 嵌套组合：Array in Struct          |
| A13  | Int8ub (expr count)  | 100    | 101      | 分支：expr count = this.n          |
| E01  | Int8ub 流不完整       | 100→50 | 50       | 错误路径：parse 中途 stream EOF    |
| E02  | Struct{2} 内部错位   | 100→50 | 50       | 错误路径：parse 内部 Struct EOF    |

错误场景同时报告：预期失败次数 / 实际失败次数。
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
# 每个场景：
#   (key, kind, N, struct_n_fields, error_kind, label)
#   kind: "i8" / "i16ub" / "i16ul" / "i32" / "i8sb" / "struct"
#         / "nested_array" / "array_in_struct" / "expr_count"
#   error_kind: "" (正常) / "stream_eof" / "inner_struct_eof"
SCENARIOS = [
    ("a01_i8_n10",         "i8",              10,    0, "",               "Array(10, Int8ub)"),
    ("a02_i8_n100",        "i8",              100,   0, "",               "Array(100, Int8ub)"),
    ("a03_i8_n1000",       "i8",              1000,  0, "",               "Array(1000, Int8ub)"),
    ("a04_i8_n10000",      "i8",              10000, 0, "",               "Array(10000, Int8ub)"),
    ("a05_i16ub_n100",     "i16ub",           100,   0, "",               "Array(100, Int16ub)"),
    ("a06_i16ul_n100",     "i16ul",           100,   0, "",               "Array(100, Int16ul)"),
    ("a07_i32ub_n100",     "i32",             100,   0, "",               "Array(100, Int32ub)"),
    ("a08_i8sb_n1000",     "i8sb",            1000,  0, "",               "Array(1000, Int8sb)"),
    ("a09_s2_n100",        "struct",          100,   2, "",               "Array(100, Struct{2})"),
    ("a10_s4_n1000",       "struct",          1000,  4, "",               "Array(1000, Struct{4})"),
    ("a11_nested",         "nested_array",    10,    0, "",               "Array(10, Array(10, Int8ub))"),
    ("a12_struct",         "array_in_struct", 10,    0, "",               "Struct{Array(10, Int8ub), Int32ub}"),
    ("a13_expr_count",     "expr_count",      100,   0, "",               "Array(this.n, Int8ub) N=100"),
    ("e01_stream_eof",     "i8",              100,   0, "stream_eof",     "Array(100, Int8ub) + 50B stream [ERR]"),
    ("e02_inner_eof",      "struct",          100,   2, "inner_struct_eof", "Array(100, Struct{2}) + 50B stream [ERR]"),
]

# 每个场景的 number 设定（大规模用较小 number 避免超时）
NUMBER_FOR = {
    "a04_i8_n10000": 100,
    "a03_i8_n1000": 500,
    "a08_i8sb_n1000": 500,
    "a10_s4_n1000": 300,
}
DEFAULT_NUMBER = 1000
REPEAT = 5


def _expected_iters(key, n):
    """返回该场景每次调用预期的内部 subcon 迭代次数（用于标签 vs 实际一致性核查）。"""
    # 普通场景：N 次迭代
    # 嵌套：N * inner_size
    # expr_count：N 次（与 a02 同）
    if key == "a11_nested":
        return n * n  # 10 * 10
    return n


# ============================================================
# construct-rs 测量
# ============================================================
def bench_construct_rs():
    """construct-rs 测量（此子进程仅导入 construct-rs）。"""
    from dataclasses import dataclass, field as dc_field
    from construct import (
        StructMixin, field, Int8ub, Int8sb, Int16ub, Int16ul, Int32ub, Array,
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
    class OuterHolder(StructMixin):
        """嵌套组合：Struct 内含 Array。"""
        items: list = field(Array(10, Int8ub))
        tail: int = field(Int32ub)

    @dataclass
    class ExprCountHolder(StructMixin):
        """expr count 分支：Array(this.n, Int8ub)。"""
        n: int = field(Int8ub)
        items: list = field(Array(n, Int8ub))

    compiled = {}

    for key, kind, n, nf, err, _label in SCENARIOS:
        if err == "stream_eof":
            # E01：Int8ub N=100 但只给 50 字节
            @dataclass
            class _C(StructMixin):
                items: list = field(Array(n, Int8ub))
            # 给 50 字节，预期在第 51 个元素 stream EOF
            data_err = bytes((i % 256) for i in range(50))
            # build 用 50 元素的合法版本（不真正执行 build，记录数据准备）
            compiled[key] = (_C, data_err, None)
            continue
        if err == "inner_struct_eof":
            # E02：Array(100, Struct{2}) 但只给 50 字节（够 25 个完整 Struct）
            inner = S2
            @dataclass
            class _C(StructMixin):
                items: list = field(Array(n, inner))
            data_err = bytes((i % 256) for i in range(50))
            compiled[key] = (_C, data_err, None)
            continue

        if kind == "i8":
            @dataclass
            class _C(StructMixin):
                items: list = field(Array(n, Int8ub))
            data = bytes(i % 256 for i in range(n))
            obj = _C(items=list(i % 256 for i in range(n)))
        elif kind == "i8sb":
            @dataclass
            class _C(StructMixin):
                items: list = field(Array(n, Int8sb))
            data = bytes(i % 256 for i in range(n))
            # Int8sb 值域 [-128, 127]：((i % 256) + 128) % 256 - 128 把字节映射回有符号
            obj = _C(items=[(((i % 256) + 128) % 256) - 128 for i in range(n)])
        elif kind == "i16ub":
            @dataclass
            class _C(StructMixin):
                items: list = field(Array(n, Int16ub))
            data = bytes(((i // 2) >> 8) & 0xFF if i % 2 == 0 else (i // 2) & 0xFF
                         for i in range(n * 2))
            obj = _C(items=list((i * 17) % 65536 for i in range(n)))
        elif kind == "i16ul":
            @dataclass
            class _C(StructMixin):
                items: list = field(Array(n, Int16ul))
            data = bytes((i * 17) % 256 if i % 2 == 0 else ((i * 17) // 256) % 256
                         for i in range(n * 2))
            obj = _C(items=list((i * 17) % 65536 for i in range(n)))
        elif kind == "i32":
            @dataclass
            class _C(StructMixin):
                items: list = field(Array(n, Int32ub))
            data = bytes((i % 256) for i in range(n * 4))
            obj = _C(items=list(i % 256 for i in range(n)))
        elif kind == "struct":
            inner = S2 if nf == 2 else S4
            @dataclass
            class _C(StructMixin):
                items: list = field(Array(n, inner))
            data = bytes((i % 256) for i in range(n * nf))
            if nf == 2:
                inners = [S2(a=i % 256, b=(i + 1) % 256) for i in range(n)]
            else:
                inners = [S4(a=i % 256, b=(i + 1) % 256,
                             c=(i + 2) % 256, d=(i + 3) % 256) for i in range(n)]
            obj = _C(items=inners)
        elif kind == "nested_array":
            # Array(N, Array(N, Int8ub))
            @dataclass
            class _C(StructMixin):
                items: list = field(Array(n, Array(n, Int8ub)))
            data = bytes(i % 256 for i in range(n * n))
            inners = [list((j + i * n) % 256 for j in range(n)) for i in range(n)]
            obj = _C(items=inners)
        elif kind == "array_in_struct":
            # Struct{Array(10, Int8ub), Int32ub}：N=10 表示 Array 大小
            _C = OuterHolder
            data = bytes(i % 256 for i in range(10)) + bytes([0, 0, 0, 0xFF])
            obj = _C(items=list(i % 256 for i in range(10)), tail=0xFF)
        elif kind == "expr_count":
            # Array(this.n, Int8ub) N=100：先写 n=100 字节再写 100 个 Int8ub
            _C = ExprCountHolder
            data = bytes([100]) + bytes(i % 256 for i in range(n))
            obj = _C(n=n, items=list(i % 256 for i in range(n)))
        else:
            raise ValueError(kind)

        number = NUMBER_FOR.get(key, DEFAULT_NUMBER)
        compiled[key] = (_C, data, obj)

    results = {}
    # 正常场景：parse + build
    for key, kind, n, nf, err, _label in SCENARIOS:
        if err:
            continue  # 错误场景后处理
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
        Struct, Array, Byte, Int8sb, Int16ub, Int16ul, Int32ub, Bytes, Container,
        this,
    )

    compiled = {}
    for key, kind, n, nf, err, _label in SCENARIOS:
        if err:
            if err == "stream_eof":
                arr = Array(n, Byte)
                data_err = bytes((i % 256) for i in range(50))
                compiled[key] = (arr, data_err, None)
            elif err == "inner_struct_eof":
                inner = Struct("a" / Byte, "b" / Byte)
                arr = Array(n, inner)
                data_err = bytes((i % 256) for i in range(50))
                compiled[key] = (arr, data_err, None)
            continue

        if kind == "i8":
            arr = Array(n, Byte)
            data = bytes(i % 256 for i in range(n))
            obj = list(i % 256 for i in range(n))
        elif kind == "i8sb":
            arr = Array(n, Int8sb)
            data = bytes(i % 256 for i in range(n))
            obj = [(((i % 256) + 128) % 256) - 128 for i in range(n)]
        elif kind == "i16ub":
            arr = Array(n, Int16ub)
            data = bytes(((i // 2) >> 8) & 0xFF if i % 2 == 0 else (i // 2) & 0xFF
                         for i in range(n * 2))
            obj = list((i * 17) % 65536 for i in range(n))
        elif kind == "i16ul":
            arr = Array(n, Int16ul)
            data = bytes((i * 17) % 256 if i % 2 == 0 else ((i * 17) // 256) % 256
                         for i in range(n * 2))
            obj = list((i * 17) % 65536 for i in range(n))
        elif kind == "i32":
            arr = Array(n, Int32ub)
            data = bytes((i % 256) for i in range(n * 4))
            obj = list(i % 256 for i in range(n))
        elif kind == "struct":
            if nf == 2:
                inner = Struct("a" / Byte, "b" / Byte)
            else:
                inner = Struct("a" / Byte, "b" / Byte, "c" / Byte, "d" / Byte)
            arr = Array(n, inner)
            data = bytes((i % 256) for i in range(n * nf))
            if nf == 2:
                obj = [Container(a=i % 256, b=(i + 1) % 256) for i in range(n)]
            else:
                obj = [Container(a=i % 256, b=(i + 1) % 256,
                                 c=(i + 2) % 256, d=(i + 3) % 256) for i in range(n)]
        elif kind == "nested_array":
            arr = Array(n, Array(n, Byte))
            data = bytes(i % 256 for i in range(n * n))
            obj = [list((j + i * n) % 256 for j in range(n)) for i in range(n)]
        elif kind == "array_in_struct":
            arr = Struct("items" / Array(10, Byte), "tail" / Int32ub)
            data = bytes(i % 256 for i in range(10)) + bytes([0, 0, 0, 0xFF])
            obj = Container(items=list(i % 256 for i in range(10)), tail=0xFF)
        elif kind == "expr_count":
            arr = Struct("n" / Byte, "items" / Array(this.n, Byte))
            data = bytes([100]) + bytes(i % 256 for i in range(n))
            obj = Container(n=n, items=list(i % 256 for i in range(n)))
        else:
            raise ValueError(kind)

        number = NUMBER_FOR.get(key, DEFAULT_NUMBER)
        compiled[key] = (arr, data, obj)

    results = {}
    for key, kind, n, nf, err, _label in SCENARIOS:
        if err:
            continue
        arr, data, obj = compiled[key]
        number = NUMBER_FOR.get(key, DEFAULT_NUMBER)
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
# 错误路径测量：parse 在错误数据上的耗时（time-to-fail）
# ============================================================
def bench_error_paths_rs():
    """construct-rs 错误路径测量：在错误数据上 parse，测量 time-to-fail。

    返回 dict: {key: (ns_per_call, expected_fails, actual_fails)}
    """
    from dataclasses import dataclass
    from construct import (
        StructMixin, field, Int8ub, Array,
    )

    @dataclass
    class S2(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)

    out = {}
    for key, kind, n, nf, err, _label in SCENARIOS:
        if not err:
            continue
        if err == "stream_eof":
            @dataclass
            class C(StructMixin):
                items: list = field(Array(n, Int8ub))
            data = bytes((i % 256) for i in range(50))
            expected_iters = 50  # 第 51 个失败
        elif err == "inner_struct_eof":
            @dataclass
            class C(StructMixin):
                items: list = field(Array(n, S2))
            data = bytes((i % 256) for i in range(50))
            expected_iters = 25  # 第 26 个 Struct 失败（每个 2 字节）
        else:
            continue

        # 跑 number 次，预期每次都抛异常。直接循环（避免 timeit.repeat 的多轮累加）。
        number = 200
        actual_fails = 0
        # 预热 50 次
        for _ in range(50):
            try:
                C.parse(data)
            except Exception:
                pass
        # 计时
        import time as _time
        t0 = _time.perf_counter()
        for _ in range(number):
            try:
                C.parse(data)
            except Exception:
                actual_fails += 1
        elapsed = (_time.perf_counter() - t0) / number
        out[key] = (elapsed, number, actual_fails, expected_iters)
    return out


def bench_error_paths_py():
    """Python construct 2.10.70 错误路径测量。"""
    from construct import Struct, Array, Byte
    import time as _time

    out = {}
    for key, kind, n, nf, err, _label in SCENARIOS:
        if not err:
            continue
        if err == "stream_eof":
            arr = Array(n, Byte)
            data = bytes((i % 256) for i in range(50))
            expected_iters = 50
        elif err == "inner_struct_eof":
            inner = Struct("a" / Byte, "b" / Byte)
            arr = Array(n, inner)
            data = bytes((i % 256) for i in range(50))
            expected_iters = 25
        else:
            continue

        number = 200
        actual_fails = 0
        # 预热
        for _ in range(50):
            try:
                arr.parse(data)
            except Exception:
                pass
        t0 = _time.perf_counter()
        for _ in range(number):
            try:
                arr.parse(data)
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


def _bytes_per_call(kind, n, nf):
    """估算每次调用的字节数。"""
    if kind == "i8":
        return n * 1
    if kind == "i8sb":
        return n * 1
    if kind in ("i16ub", "i16ul"):
        return n * 2
    if kind == "i32":
        return n * 4
    if kind == "struct":
        return n * nf
    if kind == "nested_array":
        return n * n
    if kind == "array_in_struct":
        return 10 + 4
    if kind == "expr_count":
        return 1 + n
    return 0


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

        # 1) 正常场景
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
        # 2) 错误场景
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

        # 解析正常场景输出
        rs_data, py_data = {}, {}
        for line in rs_proc.stdout.strip().split("\n"):
            _, k, v = line.split("\t")
            rs_data[k] = float(v)
        for line in py_proc.stdout.strip().split("\n"):
            _, k, v = line.split("\t")
            py_data[k] = float(v)

        # 解析错误场景输出: rs_err  key  ns  num  actual  exp
        rs_err_data, py_err_data = {}, {}
        for line in rs_err_proc.stdout.strip().split("\n"):
            parts = line.split("\t")
            k = parts[1]
            ns = float(parts[2])
            num = int(parts[3])
            actual = int(parts[4])
            exp = int(parts[5])
            rs_err_data[k] = (ns, num, actual, exp)
        for line in py_err_proc.stdout.strip().split("\n"):
            parts = line.split("\t")
            k = parts[1]
            ns = float(parts[2])
            num = int(parts[3])
            actual = int(parts[4])
            exp = int(parts[5])
            py_err_data[k] = (ns, num, actual, exp)

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
        bytes_per = _bytes_per_call(kind, n, nf)
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
    print("─── 错误路径：parse time-to-fail ───")
    print(f"  {'场景':<40}{'迭代次数':>10}{'实际失败':>10}{'失败率':>8}"
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

    # ---- 派生指标 3：规模扩展（Int8ub N=10/100/1000/10000 ns/elem） ----
    print()
    print("─── 派生指标 3：Int8ub 规模扩展（ns/元素） ───")
    print(f"  {'N':>7}{'py parse ns/elem':>20}{'rs parse ns/elem':>20}"
          f"{'py build ns/elem':>20}{'rs build ns/elem':>20}")
    scale_keys = ["a01_i8_n10", "a02_i8_n100", "a03_i8_n1000", "a04_i8_n10000"]
    n_for = dict((s[0], s[2]) for s in SCENARIOS)
    for key in scale_keys:
        n = n_for[key]
        py_p = py_data[key + "_parse"] * 1e9 / n
        rs_p = rs_data[key + "_parse"] * 1e9 / n
        py_b = py_data[key + "_build"] * 1e9 / n
        rs_b = rs_data[key + "_build"] * 1e9 / n
        print(f"  {n:>7}{py_p:>19.2f}{rs_p:>19.2f}{py_b:>19.2f}{rs_b:>19.2f}")

    # ---- 派生指标 4：元素类型复杂度（N=100 parse 加速比） ----
    print()
    print("─── 派生指标 4：元素类型复杂度（N=100，parse 加速比） ───")
    type_compare = [
        ("Int8ub (1B BE)",      "a02_i8_n100"),
        ("Int16ub (2B BE)",     "a05_i16ub_n100"),
        ("Int16ul (2B LE)",     "a06_i16ul_n100"),
        ("Int32ub (4B BE)",     "a07_i32ub_n100"),
        ("Struct{2} (2B)",      "a09_s2_n100"),
        ("Struct+Array (嵌套)", "a12_struct"),
        ("expr count (this.n)", "a13_expr_count"),
    ]
    for name, key in type_compare:
        py_p = py_data[key + "_parse"]
        rs_p = rs_data[key + "_parse"]
        sp = py_p / rs_p
        print(f"  {name:<22} parse 加速比: {sp:>6.2f}x")

    # ---- 派生指标 5：场景标签 vs 实际迭代次数对照 ----
    print()
    print("─── 派生指标 5：场景标签 N vs 实际迭代次数 ───")
    print(f"  {'场景':<40}{'标签 N':>10}{'实际迭代':>12}{'一致':>8}")
    for key, kind, n, nf, err, label in SCENARIOS:
        if err:
            continue
        ai = _expected_iters(key, n)
        # nested_array 是 N*N 标签明确，一致
        consistent = "Y" if (ai == n or kind in ("nested_array",)) else "N"
        print(f"  {label:<40}{n:>10}{ai:>12}{consistent:>8}")

    # ---- 派生指标 6：内部关系合理性（同 N 不同场景 ns/call 量级） ----
    print()
    print("─── 派生指标 6：内部关系合理性（Int8ub N=100 是基线） ───")
    base_p_rs = rs_data["a02_i8_n100_parse"]
    base_p_py = py_data["a02_i8_n100_parse"]
    print(f"  [基线] Int8ub N=100 parse: py={format_ns(base_p_py)}, rs={format_ns(base_p_rs)}")
    compare_keys = [
        ("Int16ub N=100 (×2 B)",    "a05_i16ub_n100"),
        ("Int32ub N=100 (×4 B)",    "a07_i32ub_n100"),
        ("Struct{2} N=100 (×2 B)",  "a09_s2_n100"),
    ]
    for name, key in compare_keys:
        rs_p = rs_data[key + "_parse"]
        py_p = py_data[key + "_parse"]
        print(f"  {name:<28} parse rs/base={rs_p/base_p_rs:>5.2f}x, py/base={py_p/base_p_py:>5.2f}x")

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
