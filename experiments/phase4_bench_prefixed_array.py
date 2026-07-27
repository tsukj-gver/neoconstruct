"""Phase 4 子任务 4.3 性能基准：PrefixedArray parse/build（多维场景）。

按 performance-gate/SKILL.md S-PERF 测量口径：
- construct-rs 侧：通过 maturin develop 安装后，Python 调用用户面 API
- Python construct 侧：直接 import construct，调等效 API
- 子进程隔离（Rust 和 Python 包同名，不可在同一进程导入）
- 取 min(repeat=5) × number

场景矩阵（与 4.1/4.2 风格对齐，覆盖多维度）：
- 元素类型维度：Int8ub（1B 原子）/ Int32ub（4B 原子）/ Bytes(8)（8B blob）
                 / Struct{2×Int8ub}（2B 复合）
- countfield 类型维度：Byte（1B，N≤255）/ Int16ub（2B，N≤65535）
- 规模维度：64 / 255 / 1024 / 4096 元素（Int8ub 路径）
- 等字节维度：~1KB 上跨类型对比

countfield 选择规则：Byte countfield 的最大容量为 255，超出时使用 Int16ub。

场景列表（8 个）：
| #  | countfield | Element          | N elems | bytes/call | 维度作用                  |
|----|------------|------------------|---------|------------|---------------------------|
| P1 | Byte       | Int8ub           | 64      | 65         | 极小规模基线              |
| P2 | Byte       | Int8ub           | 255     | 256        | Byte cf 容量上限          |
| P3 | Int16ub    | Int8ub           | 1024    | 1026       | 中规模（~1KB）            |
| P4 | Int16ub    | Int8ub           | 4096    | 4098       | 大规模                    |
| P5 | Int16ub    | Int32ub          | 256     | 1026       | 多字节原子（~1KB）        |
| P6 | Int16ub    | Bytes(8)         | 128     | 1026       | bytes blob（~1KB）        |
| P7 | Int16ub    | Struct{2}        | 512     | 1026       | 复合元素（~1KB）          |
| P8 | Byte       | Struct{2}        | 100     | 201        | Byte cf + 复合元素        |
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
# 每个场景：(key, countfield_kind, element_kind, n_elems, label)
SCENARIOS = [
    ("p1_i8_n64",      "byte",   "i8",     64,   "PrefixedArray(Byte, Int8ub) N=64"),
    ("p2_i8_n255",     "byte",   "i8",     255,  "PrefixedArray(Byte, Int8ub) N=255"),
    ("p3_i8_n1024",    "int16ub","i8",     1024, "PrefixedArray(Int16ub, Int8ub) N=1024"),
    ("p4_i8_n4096",    "int16ub","i8",     4096, "PrefixedArray(Int16ub, Int8ub) N=4096"),
    ("p5_i32_n256",    "int16ub","i32",    256,  "PrefixedArray(Int16ub, Int32ub) N=256"),
    ("p6_bytes8_n128", "int16ub","bytes8", 128,  "PrefixedArray(Int16ub, Bytes(8)) N=128"),
    ("p7_struct2_n512","int16ub","struct", 512,  "PrefixedArray(Int16ub, Struct{2}) N=512"),
    ("p8_byte_struct", "byte",   "struct", 100,  "PrefixedArray(Byte, Struct{2}) N=100"),
]

NUMBER_FOR = {
    "p4_i8_n4096": 200,
    "p7_struct2_n512": 200,
}
DEFAULT_NUMBER = 1000
REPEAT = 5


def _encode_count(n, cf_kind):
    """编码 countfield 字节。"""
    if cf_kind == "byte":
        return bytes([n & 0xFF])
    elif cf_kind == "int16ub":
        return bytes([(n >> 8) & 0xFF, n & 0xFF])
    else:
        raise ValueError(cf_kind)


# ============================================================
# construct-rs 测量
# ============================================================
def bench_construct_rs():
    from dataclasses import dataclass
    from construct import (
        StructMixin, field, Int8ub, Int16ub, Int32ub, Bytes, PrefixedArray,
    )

    @dataclass
    class S2(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)

    compiled = {}
    for key, cf_kind, kind, n, _label in SCENARIOS:
        if cf_kind == "byte":
            countfield = Int8ub
        elif cf_kind == "int16ub":
            countfield = Int16ub
        else:
            raise ValueError(cf_kind)

        if kind == "i8":
            @dataclass
            class _C(StructMixin):
                items: list = field(PrefixedArray(countfield, Int8ub))
            data = _encode_count(n, cf_kind) + bytes(i % 256 for i in range(n))
            obj = _C(items=list(i % 256 for i in range(n)))
        elif kind == "i32":
            @dataclass
            class _C(StructMixin):
                items: list = field(PrefixedArray(countfield, Int32ub))
            data = _encode_count(n, cf_kind) + bytes((i) % 256 for i in range(n * 4))
            obj = _C(items=list(i % 256 for i in range(n)))
        elif kind == "bytes8":
            @dataclass
            class _C(StructMixin):
                items: list = field(PrefixedArray(countfield, Bytes(8)))
            data = _encode_count(n, cf_kind) + bytes((i) % 256 for i in range(n * 8))
            one = bytes((i) % 256 for i in range(8))
            obj = _C(items=[one] * n)
        elif kind == "struct":
            @dataclass
            class _C(StructMixin):
                items: list = field(PrefixedArray(countfield, S2))
            data = _encode_count(n, cf_kind) + bytes((i) % 256 for i in range(n * 2))
            obj = _C(items=[S2(a=i % 256, b=(i+1) % 256) for i in range(n)])
        else:
            raise ValueError(kind)

        number = NUMBER_FOR.get(key, DEFAULT_NUMBER)
        compiled[key] = (_C, data, obj, number)

    results = {}
    for key, _cf, _kind, _n, _label in SCENARIOS:
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
        Byte, Int16ub, Int32ub, Bytes, Struct, Container,
        PrefixedArray,
    )

    compiled = {}
    for key, cf_kind, kind, n, _label in SCENARIOS:
        if cf_kind == "byte":
            countfield = Byte
        elif cf_kind == "int16ub":
            countfield = Int16ub
        else:
            raise ValueError(cf_kind)

        if kind == "i8":
            pa = PrefixedArray(countfield, Byte)
            data = _encode_count(n, cf_kind) + bytes(i % 256 for i in range(n))
            obj = list(i % 256 for i in range(n))
        elif kind == "i32":
            pa = PrefixedArray(countfield, Int32ub)
            data = _encode_count(n, cf_kind) + bytes((i) % 256 for i in range(n * 4))
            obj = list(i % 256 for i in range(n))
        elif kind == "bytes8":
            pa = PrefixedArray(countfield, Bytes(8))
            data = _encode_count(n, cf_kind) + bytes((i) % 256 for i in range(n * 8))
            one = bytes((i) % 256 for i in range(8))
            obj = [one] * n
        elif kind == "struct":
            inner = Struct("a" / Byte, "b" / Byte)
            pa = PrefixedArray(countfield, inner)
            data = _encode_count(n, cf_kind) + bytes((i) % 256 for i in range(n * 2))
            obj = [Container(a=i % 256, b=(i+1) % 256) for i in range(n)]
        else:
            raise ValueError(kind)

        number = NUMBER_FOR.get(key, DEFAULT_NUMBER)
        compiled[key] = (pa, data, obj, number)

    results = {}
    for key, _cf, _kind, _n, _label in SCENARIOS:
        pa, data, obj, number = compiled[key]
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
        rs_python = r"<opencode-temp>\crs_venv2\Scripts\python.exe"
        py_python = r"<opencode-temp>\crs_venv_py\Scripts\python.exe"
        script = os.path.abspath(__file__)

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
    print("=" * 115)
    print(f"{'#':<6}{'场景':<44}{'N':>7}{'bytes/call':>13}"
          f"{'Py ns/call':>16}{'Rs ns/call':>16}{'加速比':>10}{'方向':>8}")
    print("-" * 115)

    # 计算每场景的字节数
    def bytes_for(cf_kind, kind, n):
        cf_bytes = 1 if cf_kind == "byte" else 2
        bpe = {"i8": 1, "i32": 4, "bytes8": 8, "struct": 2}[kind]
        return cf_bytes + n * bpe

    speedups = []
    for key, cf_kind, kind, n, label in SCENARIOS:
        bytes_per = bytes_for(cf_kind, kind, n)
        for direction, suffix in (("parse", "_parse"), ("build", "_build")):
            full_key = key + suffix
            py_ns = py_data[full_key]
            rs_ns = rs_data[full_key]
            speedup = py_ns / rs_ns
            speedups.append((key, label, n, kind, direction, py_ns, rs_ns, speedup))

            low_flag = " <4x!" if speedup < 4 else (" <10x" if speedup < 10 else "")
            if direction == "parse":
                print(f"{key[:6]:<6}{label:<44}{n:>7}{bytes_per:>13}"
                      f"{format_ns(py_ns):>16}{format_ns(rs_ns):>16}{speedup:>8.2f}x{low_flag}"
                      f"{direction:>8}")
            else:
                print(f"{'':<6}{'  └─ build':<44}{n:>7}{bytes_per:>13}"
                      f"{format_ns(py_ns):>16}{format_ns(rs_ns):>16}{speedup:>8.2f}x{low_flag}"
                      f"{direction:>8}")

    print("-" * 115)

    # 派生指标
    print()
    print("─── 派生指标 1：加速比按场景排序（由高到低） ───")
    sorted_sp = sorted(speedups, key=lambda x: -x[7])
    for key, label, n, _kind, direction, _py, _rs, sp in sorted_sp:
        flag = " <4x!" if sp < 4 else (" <10x" if sp < 10 else "")
        print(f"  {label + ' ' + direction:<54} {sp:>7.2f}x{flag}")

    print()
    print("─── 派生指标 2：parse/build 比率（同实现内部） ───")
    print(f"  {'场景':<44}{'N':>7}{'py p/b':>10}{'rs p/b':>10}{'方向一致':>12}")
    for key, _cf, kind, n, label in SCENARIOS:
        py_p = py_data[key + "_parse"]
        py_b = py_data[key + "_build"]
        rs_p = rs_data[key + "_parse"]
        rs_b = rs_data[key + "_build"]
        py_r = py_p / py_b
        rs_r = rs_p / rs_b
        consistent = "Y" if (py_r > 1) == (rs_r > 1) else "N"
        print(f"  {label:<44}{n:>7}{py_r:>9.2f}x{rs_r:>9.2f}x{consistent:>12}")

    print()
    print("─── 派生指标 3：Int16ub cf 规模扩展（ns/元素） ───")
    print(f"  {'N':>7}{'py parse ns/elem':>20}{'rs parse ns/elem':>20}"
          f"{'py build ns/elem':>20}{'rs build ns/elem':>20}")
    scale_keys = ["p3_i8_n1024", "p4_i8_n4096"]
    n_for = dict((k[0], k[3]) for k in SCENARIOS)
    # 加上 Byte cf 的小规模
    scale_keys_with_cf = [
        ("Byte cf", "p1_i8_n64"),
        ("Byte cf", "p2_i8_n255"),
        ("Int16ub cf", "p3_i8_n1024"),
        ("Int16ub cf", "p4_i8_n4096"),
    ]
    print(f"  {'cf 类型':<14}{'N':>7}{'py parse ns/elem':>20}{'rs parse ns/elem':>20}"
          f"{'py build ns/elem':>20}{'rs build ns/elem':>20}")
    for cf_label, key in scale_keys_with_cf:
        n = n_for[key]
        py_p = py_data[key + "_parse"] * 1e9 / n
        rs_p = rs_data[key + "_parse"] * 1e9 / n
        py_b = py_data[key + "_build"] * 1e9 / n
        rs_b = rs_data[key + "_build"] * 1e9 / n
        print(f"  {cf_label:<14}{n:>7}{py_p:>19.2f}{rs_p:>19.2f}{py_b:>19.2f}{rs_b:>19.2f}")

    print()
    print("─── 派生指标 4：等字节量级（~1KB）元素类型对比（Int16ub cf） ───")
    print(f"  {'元素类型':<22}{'N elems':>10}{'parse 加速比':>16}{'build 加速比':>16}")
    fixed_keys = [
        ("Int8ub (1B)",         "p3_i8_n1024"),
        ("Int32ub (4B)",        "p5_i32_n256"),
        ("Bytes(8) (8B)",       "p6_bytes8_n128"),
        ("Struct{2} (2B)",      "p7_struct2_n512"),
    ]
    for name, key in fixed_keys:
        n = n_for[key]
        sp_p = py_data[key + "_parse"] / rs_data[key + "_parse"]
        sp_b = py_data[key + "_build"] / rs_data[key + "_build"]
        print(f"  {name:<22}{n:>10}{sp_p:>15.2f}x{sp_b:>15.2f}x")

    # countfield 对比
    print()
    print("─── 派生指标 5：countfield 类型对比（Byte vs Int16ub） ───")
    print(f"  {'countfield':<14}{'元素':<14}{'N elems':>10}{'parse 加速比':>16}{'build 加速比':>16}")
    cf_keys = [
        ("Byte (1B)",    "Int8ub",     "p1_i8_n64"),
        ("Byte (1B)",    "Int8ub",     "p2_i8_n255"),
        ("Int16ub (2B)", "Int8ub",     "p3_i8_n1024"),
        ("Int16ub (2B)", "Int8ub",     "p4_i8_n4096"),
    ]
    for cf_name, elem_name, key in cf_keys:
        n = n_for[key]
        sp_p = py_data[key + "_parse"] / rs_data[key + "_parse"]
        sp_b = py_data[key + "_build"] / rs_data[key + "_build"]
        print(f"  {cf_name:<14}{elem_name:<14}{n:>10}{sp_p:>15.2f}x{sp_b:>15.2f}x")

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
    print("=" * 115)


if __name__ == "__main__":
    main()

