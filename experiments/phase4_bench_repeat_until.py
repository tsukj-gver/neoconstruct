"""Phase 4 子任务 4.5 性能基准：RepeatUntil parse/build（Expr vs PyCallable 双路径）。

按 performance-gate/SKILL.md S-PERF 测量口径：
- construct-rs 侧：通过 maturin develop 安装后，Python 调用用户面 API
- Python construct 侧：直接 import construct，调等效 API
- 子进程隔离（Rust 和 Python 包同名，不可在同一进程导入）
- 取 min(repeat=5) × number

设计文档 §8.3 性能目标（v4 V-1 修正后）：
- Expr 路径 ≥ 8x
- PyCallable 路径 ≥ 1.5x（v4：Container proxy + 字段复制开销；PyCallable 是兜底路径）

场景矩阵（覆盖两个路径 × 多个规模 × 多个元素类型）：

Expr 路径（简单 lambda `lambda x,_,_: x > N`，自动编译为 ExprProgram）：
| #  | Element  | N elems | 字节/call | 说明                       |
|----|----------|---------|----------|----------------------------|
| E1 | Int8ub   | 64      | 64       | 小规模基线                 |
| E2 | Int8ub   | 1024    | 1024     | 中规模                     |
| E3 | Int8ub   | 8192    | 8192     | 大规模（验证扩展性）       |
| E4 | Int32ub  | 256     | 1024     | 多字节原子（1KB 字节量级） |

PyCallable 路径（复杂 lambda，每次迭代跨 FFI）：
| #  | Element  | N elems | 字节/call | 说明                       |
|----|----------|---------|----------|----------------------------|
| P1 | Int8ub   | 64      | 64       | 小规模                     |
| P2 | Int8ub   | 1024    | 1024     | 中规模                     |
| P3 | Int32ub  | 256     | 1024     | 多字节原子                 |

补充：
| #  | 说明                                                          |
|----|---------------------------------------------------------------|
| M1 | Expr 路径，谓词左侧常量（`lambda x,_,_: 5 < x`，触发翻转分支）|
| M2 | PyCallable 路径，list 依赖谓词（`lambda x,lst,c: lst[-1]==N`）|
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
# (key, path, element_kind, n_elems, bytes_per_elem, label)
SCENARIOS = [
    # Expr 路径
    ("e1_i8_n64",    "expr",   "i8",     64,    1, "Expr RepeatUntil(Int8ub) N=64"),
    ("e2_i8_n1024",  "expr",   "i8",     1024,  1, "Expr RepeatUntil(Int8ub) N=1024"),
    ("e3_i8_n8192",  "expr",   "i8",     8192,  1, "Expr RepeatUntil(Int8ub) N=8192"),
    ("e4_i32_n256",  "expr",   "i32",    256,   4, "Expr RepeatUntil(Int32ub) N=256"),
    # PyCallable 路径
    ("p1_i8_n64",    "call",   "i8",     64,    1, "Callable RepeatUntil(Int8ub) N=64"),
    ("p2_i8_n1024",  "call",   "i8",     1024,  1, "Callable RepeatUntil(Int8ub) N=1024"),
    ("p3_i32_n256",  "call",   "i32",    256,   4, "Callable RepeatUntil(Int32ub) N=256"),
    # 补充
    ("m1_flip",      "expr_flip", "i8",  1024,  1, "Expr flipped predicate N=1024"),
    ("m2_list_dep",  "call_list", "i8",  1024,  1, "Callable list-dependent N=1024"),
]

NUMBER_FOR = {
    "e3_i8_n8192": 100,
    "e2_i8_n1024": 500,
    "p2_i8_n1024": 500,
    "m1_flip": 500,
    "m2_list_dep": 500,
}
DEFAULT_NUMBER = 1000
REPEAT = 5


# ============================================================
# construct-rs 测量
# ============================================================
def bench_construct_rs():
    from dataclasses import dataclass
    from construct import (
        StructMixin, field, Int8ub, Int32ub, RepeatUntil,
    )

    compiled = {}
    for key, path, kind, n, bpe, _label in SCENARIOS:
        if kind == "i8":
            elem_fmt = Int8ub
            elem_size = 1
        elif kind == "i32":
            elem_fmt = Int32ub
            elem_size = 4
        else:
            raise ValueError(kind)

        # 选择谓词 + 构造匹配的 data/obj
        if path in ("expr", "expr_flip"):
            # 简单 lambda：自动编译为 Expr 路径
            # 谓词：x > 200，需要在最后放一个 > 200 的值
            if path == "expr":
                predicate = lambda x, lst, ctx: x > 200
            else:
                predicate = lambda x, lst, ctx: 200 < x
            # data: 前 n-1 个为 1，最后为 255（满足 x > 200）
            if kind == "i8":
                data_bytes = [1] * (n - 1) + [255]
                obj_list = list(data_bytes)
                data = bytes(data_bytes)
            else:  # i32 (big-endian)
                data_bytes = b"".join(b"\x00\x00\x00\x01" for _ in range(n - 1)) + b"\x00\x00\x00\xff"
                obj_list = [1] * (n - 1) + [255]
                data = data_bytes
        elif path == "call":
            # 复杂 lambda：用 len(lst) 触发 PyCallable 路径回落
            predicate = lambda x, lst, ctx: x > 200 if len(lst) >= 0 else False
            if kind == "i8":
                data_bytes = [1] * (n - 1) + [255]
                obj_list = list(data_bytes)
                data = bytes(data_bytes)
            else:  # i32 (big-endian)
                data_bytes = b"".join(b"\x00\x00\x00\x01" for _ in range(n - 1)) + b"\x00\x00\x00\xff"
                obj_list = [1] * (n - 1) + [255]
                data = data_bytes
        elif path == "call_list":
            # 真正依赖 list 的谓词
            predicate = lambda x, lst, ctx: len(lst) >= 2 and lst[-2] == lst[-1]
            # data: 前 n-2 个为 1，最后两个为 42, 42
            if kind == "i8":
                data_bytes = [1] * (n - 2) + [42, 42]
                obj_list = list(data_bytes)
                data = bytes(data_bytes)
            else:  # i32 (big-endian)
                data_bytes = b"".join(b"\x00\x00\x00\x01" for _ in range(n - 2)) + b"\x00\x00\x00\x2a" * 2
                obj_list = [1] * (n - 2) + [42, 42]
                data = data_bytes
        else:
            raise ValueError(path)

        @dataclass
        class _C(StructMixin):
            payload: list = field(RepeatUntil(predicate, elem_fmt))

        number = NUMBER_FOR.get(key, DEFAULT_NUMBER)
        compiled[key] = (_C, data, _C(payload=obj_list), number)

    results = {}
    for key, _path, _kind, _n, _bpe, _label in SCENARIOS:
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
    from construct import RepeatUntil, Byte, Int32ub

    compiled = {}
    for key, path, kind, n, bpe, _label in SCENARIOS:
        if kind == "i8":
            elem_fmt = Byte
        elif kind == "i32":
            elem_fmt = Int32ub
        else:
            raise ValueError(kind)

        if path in ("expr", "expr_flip"):
            if path == "expr":
                predicate = lambda x, lst, ctx: x > 200
            else:
                predicate = lambda x, lst, ctx: 200 < x
            if kind == "i8":
                data_bytes = [1] * (n - 1) + [255]
                obj_list = list(data_bytes)
                data = bytes(data_bytes)
            else:  # i32 big-endian
                data_bytes = b"".join(b"\x00\x00\x00\x01" for _ in range(n - 1)) + b"\x00\x00\x00\xff"
                obj_list = [1] * (n - 1) + [255]
                data = data_bytes
        elif path == "call":
            predicate = lambda x, lst, ctx: x > 200 if len(lst) >= 0 else False
            if kind == "i8":
                data_bytes = [1] * (n - 1) + [255]
                obj_list = list(data_bytes)
                data = bytes(data_bytes)
            else:  # i32 big-endian
                data_bytes = b"".join(b"\x00\x00\x00\x01" for _ in range(n - 1)) + b"\x00\x00\x00\xff"
                obj_list = [1] * (n - 1) + [255]
                data = data_bytes
        elif path == "call_list":
            predicate = lambda x, lst, ctx: len(lst) >= 2 and lst[-2] == lst[-1]
            if kind == "i8":
                data_bytes = [1] * (n - 2) + [42, 42]
                obj_list = list(data_bytes)
                data = bytes(data_bytes)
            else:  # i32 big-endian
                data_bytes = b"".join(b"\x00\x00\x00\x01" for _ in range(n - 2)) + b"\x00\x00\x00\x2a" * 2
                obj_list = [1] * (n - 2) + [42, 42]
                data = data_bytes
        else:
            raise ValueError(path)

        d = RepeatUntil(predicate, elem_fmt)
        number = NUMBER_FOR.get(key, DEFAULT_NUMBER)
        compiled[key] = (d, data, obj_list, number)

    results = {}
    for key, _path, _kind, _n, _bpe, _label in SCENARIOS:
        d, data, obj, number = compiled[key]
        parse_ns = min(
            timeit.repeat(lambda dd=d, x=data: dd.parse(x), number=number, repeat=REPEAT)
        ) / number
        build_ns = min(
            timeit.repeat(lambda dd=d, x=obj: dd.build(x), number=number, repeat=REPEAT)
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
    print("=" * 120)
    print(f"{'#':<14}{'场景':<40}{'N':>7}{'bytes/call':>13}"
          f"{'Py ns/call':>16}{'Rs ns/call':>16}{'加速比':>10}{'目标':>8}{'达标':>8}")
    print("-" * 120)

    speedups = []
    for key, path, kind, n, bpe, label in SCENARIOS:
        bytes_per = n * bpe
        # 设定目标：Expr 路径 >=8x，PyCallable 路径 >=1.5x（v4 V-1，原 ≥3x 下调）
        if path.startswith("expr"):
            target = 8.0
            target_str = ">=8x"
        else:
            target = 1.5
            target_str = ">=1.5x"

        for direction, suffix in (("parse", "_parse"), ("build", "_build")):
            full_key = key + suffix
            py_ns = py_data[full_key]
            rs_ns = rs_data[full_key]
            speedup = py_ns / rs_ns
            speedups.append((key, label, path, n, bpe, direction, py_ns, rs_ns, speedup, target))

            low_flag = "" if speedup >= target else " <target!"
            if direction == "parse":
                print(f"{key:<14}{label:<40}{n:>7}{bytes_per:>13}"
                      f"{format_ns(py_ns):>16}{format_ns(rs_ns):>16}{speedup:>8.2f}x"
                      f"{target_str:>8}{'PASS' if speedup >= target else 'FAIL':>8}")
            else:
                print(f"{'':<14}{'  └─ build':<40}{n:>7}{bytes_per:>13}"
                      f"{format_ns(py_ns):>16}{format_ns(rs_ns):>16}{speedup:>8.2f}x"
                      f"{target_str:>8}{'PASS' if speedup >= target else 'FAIL':>8}")

    print("-" * 120)

    # 派生指标
    print()
    print("─── 派生指标 1：Expr vs PyCallable 路径对比（Int8ub N=1024） ───")
    print(f"  {'方向':<10}{'Py Callable':>18}{'Rs Callable':>18}{'Rs Expr':>18}"
          f"{'Callable 加速':>16}{'Expr 加速':>16}{'Expr/Callable':>16}")
    for direction in ("parse", "build"):
        py_call = py_data[f"p2_i8_n1024_{direction}"]
        rs_call = rs_data[f"p2_i8_n1024_{direction}"]
        rs_expr = rs_data[f"e2_i8_n1024_{direction}"]
        call_speedup = py_call / rs_call
        expr_speedup = py_call / rs_expr
        ratio = rs_call / rs_expr  # Expr 比 Callable 快多少倍
        print(f"  {direction:<10}{format_ns(py_call):>18}{format_ns(rs_call):>18}"
              f"{format_ns(rs_expr):>18}{call_speedup:>14.2f}x{expr_speedup:>14.2f}x"
              f"{ratio:>14.2f}x")

    print()
    print("─── 派生指标 2：加速比按场景排序（由高到低） ───")
    sorted_sp = sorted(speedups, key=lambda x: -x[8])
    for key, label, path, n, _bpe, direction, _py, _rs, sp, target in sorted_sp:
        flag = " PASS" if sp >= target else " FAIL"
        print(f"  {label + ' ' + direction:<45} {sp:>7.2f}x{flag}")

    print()
    print("─── 派生指标 3：parse/build 比率（同实现内部） ───")
    print(f"  {'场景':<40}{'N':>7}{'py p/b':>10}{'rs p/b':>10}{'方向一致':>12}")
    for key, _path, _kind, n, bpe, label in SCENARIOS:
        py_p = py_data[key + "_parse"]
        py_b = py_data[key + "_build"]
        rs_p = rs_data[key + "_parse"]
        rs_b = rs_data[key + "_build"]
        py_r = py_p / py_b
        rs_r = rs_p / rs_b
        consistent = "Y" if (py_r > 1) == (rs_r > 1) else "N"
        print(f"  {label:<40}{n:>7}{py_r:>9.2f}x{rs_r:>9.2f}x{consistent:>12}")

    print()
    print("─── 派生指标 4：Int8ub Expr 路径规模扩展 ───")
    print(f"  {'N':>7}{'py parse ns/elem':>20}{'rs parse ns/elem':>20}"
          f"{'py build ns/elem':>20}{'rs build ns/elem':>20}")
    scale_keys = ["e1_i8_n64", "e2_i8_n1024", "e3_i8_n8192"]
    n_for = {s[0]: s[3] for s in SCENARIOS}
    for key in scale_keys:
        n = n_for[key]
        py_p = py_data[key + "_parse"] * 1e9 / n
        rs_p = rs_data[key + "_parse"] * 1e9 / n
        py_b = py_data[key + "_build"] * 1e9 / n
        rs_b = rs_data[key + "_build"] * 1e9 / n
        print(f"  {n:>7}{py_p:>19.2f}{rs_p:>19.2f}{py_b:>19.2f}{rs_b:>19.2f}")

    # 异常检测
    print()
    print("─── 异常项检测 ───")
    fails = [(lbl, d, sp, t) for _, lbl, _, _, _, d, _, _, sp, t in speedups if sp < t]
    if fails:
        print(f"  [FAIL] 未达目标的场景 ({len(fails)}):")
        for lbl, d, sp, t in fails:
            print(f"    {lbl} {d}: {sp:.2f}x (target >= {t}x)")
    else:
        print("  [PASS] 全部场景达标")

    # S-PERF 出口
    print()
    print("─── S-PERF 出口标准 ───")
    expr_fails = [(lbl, d, sp) for _, lbl, p, _, _, d, _, _, sp, t in speedups
                  if p.startswith("expr") and sp < t]
    call_fails = [(lbl, d, sp) for _, lbl, p, _, _, d, _, _, sp, t in speedups
                  if not p.startswith("expr") and sp < t]
    if not expr_fails and not call_fails:
        print(f"  [PASS] Expr 路径全部 >= 8x；PyCallable 路径全部 >= 1.5x (v4 V-1)")
    else:
        if expr_fails:
            print(f"  [FAIL] Expr 路径未达 8x ({len(expr_fails)}/{len([s for s in speedups if s[2].startswith('expr')])}):")
            for lbl, d, sp in expr_fails:
                print(f"    {lbl} {d}: {sp:.2f}x")
        if call_fails:
            print(f"  [FAIL] PyCallable 路径未达 1.5x ({len(call_fails)}/{len([s for s in speedups if not s[2].startswith('expr')])}):")
            for lbl, d, sp in call_fails:
                print(f"    {lbl} {d}: {sp:.2f}x")
    print("=" * 120)


if __name__ == "__main__":
    main()
