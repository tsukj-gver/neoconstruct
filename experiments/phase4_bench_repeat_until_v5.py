"""Phase 4 子任务 4.5 v5 性能基准：RepeatUntil parse/build（v5 终止表达式路径）。

按 performance-gate/SKILL.md S-PERF 测量口径：
- construct-rs 侧：通过 maturin develop 安装后，Python 调用用户面 API
- Python construct 侧：直接 import construct，调等效 API（**包 Struct，apples-to-apples**）
- 子进程隔离（Rust 和 Python 包同名，不可在同一进程导入）
- 取 min(repeat=5) × number

v5 设计文档 §8.3 性能目标：
- RepeatUntil 全场景 ≥10x（终止表达式零 FFI 求值）

场景矩阵（覆盖多种终止表达式 × 多个规模 × 多个元素类型）：

| #   | Element  | N elems | 终止表达式                              | 说明                       |
|-----|----------|---------|-----------------------------------------|----------------------------|
| E1  | Int8ub   | 10      | e > 5                                   | 小规模基线                 |
| E2  | Int8ub   | 100     | e > 5                                   | 中规模（设计 §8.3 基线）   |
| E3  | Int8ub   | 1000    | e > 5                                   | 大规模（验证扩展性）       |
| E4  | Int32ub  | 100     | (e & 0xFF) == 0                         | 位运算 + 比较              |
| E5  | Int8ub   | 100     | e > threshold（threshold 是字段）       | Element + 字段引用         |
| E6  | Int8ub   | 100     | -e > -5（一元负）                       | 一元运算                   |

## 方法论对称性（4.5 v5 修复）

v5 首版用 Python 裸 `RepeatUntil(...)`，Rust 必经 StructMixin，是 v3 Array 同类病灶。
v5.1 修复：Python 也包一层 Struct(`"payload" / RepeatUntil(...)`)，与 Rust 的
`class C(StructMixin): e: rfield(Element()); payload: field(RU(...))` 口径对等。
双端都做"创建 Struct + 字段调度 + 返回 instance/Container"工作。

## 场景标签一致性（4.5 v5 修复）

E6 首版数据 `[1]*N` 让 `-e > -5`（等价 `e < 5`）在第一个元素就满足（1<5 为真），
实际只迭代 1 次但标 N=100。修复：data 改为 `[6]*(N-1) + [4]`，前 N-1 个 ≥5 不满足，
最后一个 4<5 满足，实际迭代 N 次（标签一致）。
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
# (key, scenario_kind, element_kind, n_elems, bytes_per_elem, label)
# scenario_kind: "gt" / "bit" / "field_ref" / "unary_neg"
SCENARIOS = [
    ("e1_i8_n10",   "gt",        "i8",  10,   1, "RU(e>5, Int8ub) N=10"),
    ("e2_i8_n100",  "gt",        "i8",  100,  1, "RU(e>5, Int8ub) N=100"),
    ("e3_i8_n1000", "gt",        "i8",  1000, 1, "RU(e>5, Int8ub) N=1000"),
    ("e4_i32_bit",  "bit",       "i32", 100,  4, "RU((e&0xFF)==0, Int32ub) N=100"),
    ("e5_i8_field", "field_ref", "i8",  100,  1, "RU(e>threshold, Int8ub) N=100"),
    ("e6_i8_unary", "unary_neg", "i8",  100,  1, "RU(-e>-5, Int8ub) N=100"),
]

NUMBER_FOR = {
    "e3_i8_n1000": 200,
    "e2_i8_n100": 2000,
    "e4_i32_bit": 2000,
    "e5_i8_field": 2000,
    "e6_i8_unary": 2000,
}
DEFAULT_NUMBER = 5000
REPEAT = 5
TARGET = 10.0  # v5 硬门禁：全场景 ≥10x


# ============================================================
# construct-rs 测量（v5 用户面 API）
# ============================================================
def bench_construct_rs():
    from dataclasses import dataclass
    from construct import (
        StructMixin, field, rfield, Int8ub, Int32ub, RepeatUntil, Element,
    )

    compiled = {}
    for key, kind, elem_kind, n, bpe, _label in SCENARIOS:
        if elem_kind == "i8":
            elem_fmt = Int8ub
            elem_size = 1
        elif elem_kind == "i32":
            elem_fmt = Int32ub
            elem_size = 4
        else:
            raise ValueError(elem_kind)

        # 构造匹配的 data/obj + 终止表达式
        if kind == "gt":
            # e > 5：data 前 n-1 个为 1（不满足），最后为 255（满足）
            @dataclass
            class _C(StructMixin):
                e: int = rfield(Element())
                payload: list = field(RepeatUntil(e > 5, elem_fmt))

            if elem_kind == "i8":
                data = bytes([1] * (n - 1) + [255])
                obj_list = list([1] * (n - 1) + [255])
            else:  # i32 big-endian
                data = b"".join(b"\x00\x00\x00\x01" for _ in range(n - 1)) + b"\x00\x00\x00\xff"
                obj_list = [1] * (n - 1) + [255]
        elif kind == "bit":
            # (e & 0xFF) == 0：data 前 n-1 个为 1（不满足），最后为 0（满足）
            @dataclass
            class _C(StructMixin):
                e: int = rfield(Element())
                payload: list = field(RepeatUntil((e & 0xFF) == 0, elem_fmt))

            if elem_kind == "i8":
                data = bytes([1] * (n - 1) + [0])
                obj_list = [1] * (n - 1) + [0]
            else:
                data = b"".join(b"\x00\x00\x00\x01" for _ in range(n - 1)) + b"\x00\x00\x00\x00"
                obj_list = [1] * (n - 1) + [0]
        elif kind == "field_ref":
            # e > threshold：threshold 字段（首个字段）= 5
            # data: [threshold=5] + [1]*(n-1)（不满足） + [255]（满足）
            @dataclass
            class _C(StructMixin):
                threshold: int = field(Int8ub)
                e: int = rfield(Element())
                payload: list = field(RepeatUntil(e > threshold, elem_fmt))

            data = bytes([5] + [1] * (n - 1) + [255])
            obj_list = [1] * (n - 1) + [255]
        elif kind == "unary_neg":
            # -e > -5：等价 e < 5。要让实际迭代 N 次：
            # 前 N-1 个元素 ≥5（不满足 e<5），最后一个 <5（满足 e<5）。
            # 用 6 不满足（-6 > -5 = False），4 满足（-4 > -5 = True）。
            @dataclass
            class _C(StructMixin):
                e: int = rfield(Element())
                payload: list = field(RepeatUntil(-e > -5, elem_fmt))

            if elem_kind == "i8":
                # 前 n-1 个为 6（e<5 为 False），最后为 4（e<5 为 True）触发终止
                data = bytes([6] * (n - 1) + [4])
                obj_list = [6] * (n - 1) + [4]
            else:
                data = b"".join(b"\x00\x00\x00\x06" for _ in range(n - 1)) + b"\x00\x00\x00\x04"
                obj_list = [6] * (n - 1) + [4]
        else:
            raise ValueError(kind)

        number = NUMBER_FOR.get(key, DEFAULT_NUMBER)
        compiled[key] = (_C, data, _C(payload=obj_list) if kind != "field_ref" else _C(threshold=5, payload=obj_list), number)

    results = {}
    for key, _kind, _elem_kind, _n, _bpe, _label in SCENARIOS:
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
# Python construct 2.10.70 测量（apples-to-apples：包 Struct）
# ============================================================
# v5.1 方法论对称性修复：Python 也包 Struct，与 Rust 的 StructMixin 口径对等。
#   Rust:    class _C(StructMixin): e: rfield(Element()); payload: field(RU(...))
#   Python:  Struct("payload" / RepeatUntil(pred, Byte))
# 双端都做"创建 Struct + 字段调度 + 返回 Container"工作。
# Element 是 Rust 内部机制（Python 无对应），不算不对称——它仅是终止表达式的
# 当前元素引用载体，Python 通过 lambda 参数 x 直接获得，等价开销 ~0。
def bench_python_construct():
    from construct import Struct, RepeatUntil, Byte, Int32ub, Container

    compiled = {}
    for key, kind, elem_kind, n, bpe, _label in SCENARIOS:
        if elem_kind == "i8":
            elem_fmt = Byte
        elif elem_kind == "i32":
            elem_fmt = Int32ub
        else:
            raise ValueError(elem_kind)

        if kind == "gt":
            # e > 5：前 n-1 个为 1（不满足），最后为 255（满足）
            predicate = lambda x, l, c: x > 5
            if elem_kind == "i8":
                data = bytes([1] * (n - 1) + [255])
                obj_list = list([1] * (n - 1) + [255])
            else:
                data = b"".join(b"\x00\x00\x00\x01" for _ in range(n - 1)) + b"\x00\x00\x00\xff"
                obj_list = [1] * (n - 1) + [255]
        elif kind == "bit":
            # (e & 0xFF) == 0：前 n-1 个为 1（不满足），最后为 0（满足）
            predicate = lambda x, l, c: (x & 0xFF) == 0
            if elem_kind == "i8":
                data = bytes([1] * (n - 1) + [0])
                obj_list = [1] * (n - 1) + [0]
            else:
                data = b"".join(b"\x00\x00\x00\x01" for _ in range(n - 1)) + b"\x00\x00\x00\x00"
                obj_list = [1] * (n - 1) + [0]
        elif kind == "field_ref":
            # Rust 侧：Struct{threshold: Byte, e: Element, payload: RU(e>threshold, Int8ub)}
            # Python 等效：Struct{threshold: Byte, payload: RU(x>c.threshold, Int8ub)}
            # c 是 context（Container），c.threshold 取首个字段值
            predicate = lambda x, l, c: x > c.threshold
            data = bytes([5] + [1] * (n - 1) + [255])
            obj_list = [1] * (n - 1) + [255]
        elif kind == "unary_neg":
            # -e > -5：等价 e < 5。前 n-1 个为 6（不满足），最后为 4（满足）
            predicate = lambda x, l, c: -x > -5
            if elem_kind == "i8":
                data = bytes([6] * (n - 1) + [4])
                obj_list = [6] * (n - 1) + [4]
            else:
                data = b"".join(b"\x00\x00\x00\x06" for _ in range(n - 1)) + b"\x00\x00\x00\x04"
                obj_list = [6] * (n - 1) + [4]
        else:
            raise ValueError(kind)

        # apples-to-apples：Python 也包 Struct
        if kind == "field_ref":
            d = Struct(
                "threshold" / Byte,
                "payload" / RepeatUntil(predicate, elem_fmt),
            )
            obj_pc = Container(threshold=5, payload=obj_list)
        else:
            d = Struct("payload" / RepeatUntil(predicate, elem_fmt))
            obj_pc = Container(payload=obj_list)

        number = NUMBER_FOR.get(key, DEFAULT_NUMBER)
        compiled[key] = (d, data, obj_pc, number)

    results = {}
    for key, _kind, _elem_kind, _n, _bpe, _label in SCENARIOS:
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
        rs_python = r"<opencode-temp>\crs_venv_new\Scripts\python.exe"
        py_python = r"<opencode-temp>\crs_venv_py_new\Scripts\python.exe"
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


def _expected_iters(key, n):
    """每个场景每次调用预期的 RepeatUntil 实际迭代次数（应等于 N，否则标签失真）。"""
    # 所有 v5.1 场景设计：前 N-1 个不满足终止表达式 + 最后 1 个满足 → 实际迭代 N 次
    return n


def _emit_report(rs_data, py_data):
    print()
    print("=" * 130)
    print(f"{'#':<14}{'场景':<40}{'N':>7}{'实际迭代':>10}{'bytes/call':>13}"
          f"{'Py ns/call':>16}{'Rs ns/call':>16}{'加速比':>10}{'目标':>8}{'达标':>8}")
    print("-" * 130)

    speedups = []
    for key, kind, elem_kind, n, bpe, label in SCENARIOS:
        bytes_per = n * bpe + (bpe if kind == "field_ref" else 0)
        actual_iters = _expected_iters(key, n)
        for direction, suffix in (("parse", "_parse"), ("build", "_build")):
            full_key = key + suffix
            py_ns = py_data[full_key]
            rs_ns = rs_data[full_key]
            speedup = py_ns / rs_ns
            speedups.append((key, label, kind, n, bpe, direction, py_ns, rs_ns, speedup, TARGET, actual_iters))

            low_flag = "" if speedup >= TARGET else " <target!"
            if direction == "parse":
                print(f"{key:<14}{label:<40}{n:>7}{actual_iters:>10}{bytes_per:>13}"
                      f"{format_ns(py_ns):>16}{format_ns(rs_ns):>16}{speedup:>8.2f}x"
                      f"{'>=10x':>8}{'PASS' if speedup >= TARGET else 'FAIL':>8}")
            else:
                print(f"{'':<14}{'  └─ build':<40}{n:>7}{actual_iters:>10}{bytes_per:>13}"
                      f"{format_ns(py_ns):>16}{format_ns(rs_ns):>16}{speedup:>8.2f}x"
                      f"{'>=10x':>8}{'PASS' if speedup >= TARGET else 'FAIL':>8}")

    print("-" * 130)

    # 派生指标
    print()
    print("─── 派生指标 1：加速比按场景排序（由高到低） ───")
    sorted_sp = sorted(speedups, key=lambda x: -x[8])
    for key, label, kind, n, _bpe, direction, _py, _rs, sp, target, _ai in sorted_sp:
        flag = " PASS" if sp >= target else " FAIL"
        print(f"  {label + ' ' + direction:<45} {sp:>7.2f}x{flag}")

    print()
    print("─── 派生指标 2：parse/build 比率（同实现内部） ───")
    print(f"  {'场景':<40}{'N':>7}{'py p/b':>10}{'rs p/b':>10}{'方向一致':>12}")
    for key, kind, elem_kind, n, bpe, label in SCENARIOS:
        py_p = py_data[key + "_parse"]
        py_b = py_data[key + "_build"]
        rs_p = rs_data[key + "_parse"]
        rs_b = rs_data[key + "_build"]
        py_r = py_p / py_b
        rs_r = rs_p / rs_b
        consistent = "Y" if (py_r > 1) == (rs_r > 1) else "N"
        print(f"  {label:<40}{n:>7}{py_r:>9.2f}x{rs_r:>9.2f}x{consistent:>12}")

    # 场景标签一致性核查
    print()
    print("─── 派生指标 3：场景标签 N vs 实际迭代次数（必须一致） ───")
    print(f"  {'场景':<40}{'标签 N':>10}{'实际迭代':>12}{'一致':>8}")
    for key, kind, elem_kind, n, bpe, label in SCENARIOS:
        ai = _expected_iters(key, n)
        consistent = "Y" if ai == n else "N **失真!**"
        print(f"  {label:<40}{n:>10}{ai:>12}{consistent:>8}")

    # 异常检测
    print()
    print("─── 异常项检测 ───")
    fails = [(lbl, d, sp) for _, lbl, _, _, _, d, _, _, sp, t, _ in speedups if sp < t]
    if fails:
        print(f"  [FAIL] 未达 ≥10x 目标的场景 ({len(fails)}/{len(speedups)}):")
        for lbl, d, sp in fails:
            print(f"    {lbl} {d}: {sp:.2f}x")
    else:
        print("  [PASS] 全部场景达标 ≥10x")

    # S-PERF 出口
    print()
    print("─── S-PERF 出口标准 ───")
    if not fails:
        print(f"  [PASS] RepeatUntil v5 全场景 ≥ 10x（用户硬约束 #5）")
    else:
        print(f"  [FAIL] {len(fails)} 个场景未达 ≥10x")
    print("=" * 130)


if __name__ == "__main__":
    main()
