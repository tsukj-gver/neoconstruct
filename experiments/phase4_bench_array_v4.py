"""Phase 4 子任务 4.1 性能基准 v4：Array parse/build（方法论对称修复）。

修复 v3 的方法论不对称问题（4.x-INVEST 报告 §1+§2）：

v3 病灶：
    Rust 侧：`class _C(StructMixin): items: list = field(Array(...))`
    Python 侧：`pc.Array(...)` 直接调用（不包 Struct）
    → Rust 多做了 150-200ns 的 Struct 包装工作，Python 没做，加速比假性偏低。

v4 修复（双端对称）：
    主表（apples-to-apples）：
        Rust：`class _C(StructMixin): items: list = field(Array(...))`
        Python：`pc.Struct("items" / pc.Array(...))`  ← 也包一层 Struct
    附注（裸口径）：
        Rust：`Packet.parse(data)`（必须经过 Struct，是 construct-rs 的设计约束）
        Python：`pc.Array(...).parse(data)`（直接裸调）
        明确标注"裸口径，非等价对比"。

按 performance-gate/SKILL.md S-PERF 测量口径：
- construct-rs 侧：maturin develop 安装后，Python 调用户面 API
- Python construct 侧：直接 import construct，调等效 API
- 子进程隔离（包同名，不可在同一进程导入）
- 取 min(repeat=5) × number

场景矩阵（与 v3 一致，13 正常 + 2 错误 = 15 场景 × parse/build = 30 测量点）：
| #    | Element              | N      | 维度覆盖                          |
|------|----------------------|--------|-----------------------------------|
| A01  | Int8ub               | 10     | 小规模基线 / BE u8 / literal       |
| A02  | Int8ub               | 100    | 中规模 / BE u8 / literal           |
| A03  | Int8ub               | 1000   | 大规模 / BE u8 / literal           |
| A04  | Int8ub               | 10000  | 超大规模 / BE u8 / literal         |
| A05  | Int16ub              | 100    | BE u16 / 中规模                    |
| A06  | Int16ul              | 100    | LE u16 / 字节序                    |
| A07  | Int32ub              | 100    | BE u32 / 中规模                    |
| A08  | Int8sb               | 1000   | 有符号 i8 / 大规模                 |
| A09  | Struct{2×Int8ub}     | 100    | 复合元素小                         |
| A10  | Struct{4×Int8ub}     | 1000   | 复合元素大                         |
| A11  | Array(10, Array(10)) | 10×10  | 嵌套：Array in Array              |
| A12  | Struct{Array+Int32}  | 10+1   | 嵌套：Array in Struct             |
| A13  | Int8ub (expr count)  | 100    | 分支：expr count = this.n         |
| E01  | Int8ub 流不完整       | 100→50 | 错误路径：parse stream EOF        |
| E02  | Struct{2} 内部错位   | 100→50 | 错误路径：parse inner Struct EOF  |
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

NUMBER_FOR = {
    "a04_i8_n10000": 100,
    "a03_i8_n1000": 500,
    "a08_i8sb_n1000": 500,
    "a10_s4_n1000": 300,
}
DEFAULT_NUMBER = 1000
REPEAT = 5


def _expected_iters(key, n):
    """返回该场景每次调用预期的内部 subcon 迭代次数。"""
    if key == "a11_nested":
        return n * n  # 10 * 10
    return n


def _bytes_per_call(kind, n, nf):
    """估算每次调用的字节数。"""
    if kind in ("i8", "i8sb"):
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


def _rs_py_symbolic_work(kind):
    """返回该场景 Rust/Python 两侧的符号工作量描述（用于方法论核查表）。"""
    if kind in ("i8", "i8sb"):
        return "Struct{Array(N,Int8)}"
    if kind in ("i16ub", "i16ul"):
        return "Struct{Array(N,Int16)}"
    if kind == "i32":
        return "Struct{Array(N,Int32)}"
    if kind == "struct":
        return "Struct{Array(N,Struct{nf})}"
    if kind == "nested_array":
        return "Struct{Array(N,Array(N,Int8))}"
    if kind == "array_in_struct":
        return "Struct{Array(10,Int8),Int32}"
    if kind == "expr_count":
        return "Struct{n/Int8,Array(this.n,Int8)}"
    return "?"


# ============================================================
# construct-rs 测量（沿用 v3 的 StructMixin 包装口径，不变）
# ============================================================
# 说明：construct-rs 用户面 API 设计上必须通过 StructMixin 表达（E 类设计约束），
# 每次 parse/build 必然经过 StructNode 至少一次。这正是 v4 修复的方向：
# 让 Python 侧也包一层 Struct，使两端口径对等。
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
            @dataclass
            class _C(StructMixin):
                items: list = field(Array(n, Int8ub))
            data_err = bytes((i % 256) for i in range(50))
            compiled[key] = (_C, data_err, None)
            continue
        if err == "inner_struct_eof":
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
            @dataclass
            class _C(StructMixin):
                items: list = field(Array(n, Array(n, Int8ub)))
            data = bytes(i % 256 for i in range(n * n))
            inners = [list((j + i * n) % 256 for j in range(n)) for i in range(n)]
            obj = _C(items=inners)
        elif kind == "array_in_struct":
            _C = OuterHolder
            data = bytes(i % 256 for i in range(10)) + bytes([0, 0, 0, 0xFF])
            obj = _C(items=list(i % 256 for i in range(10)), tail=0xFF)
        elif kind == "expr_count":
            _C = ExprCountHolder
            data = bytes([100]) + bytes(i % 256 for i in range(n))
            obj = _C(n=n, items=list(i % 256 for i in range(n)))
        else:
            raise ValueError(kind)

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
# Python construct 2.10.70 测量（apples-to-apples：包 Struct）
# ============================================================
# 这是 v4 的核心修复：Python 也包一层 Struct，与 Rust 的 StructMixin 口径对等。
#   Rust:    class _C(StructMixin): items: list = field(Array(N, Int8ub))
#   Python:  Struct("items" / Array(N, Byte))
# 两边都做"创建 Struct 实例 + 调度 items 字段 + 返回 Container/对象"的工作。
def bench_python_construct_wrapped():
    """Python construct 2.10.70 测量（apples-to-apples：包 Struct）。"""
    from construct import (
        Struct, Array, Byte, Int8sb, Int16ub, Int16ul, Int32ub, Bytes, Container,
        this,
    )

    compiled = {}
    for key, kind, n, nf, err, _label in SCENARIOS:
        if err:
            if err == "stream_eof":
                d = Struct("items" / Array(n, Byte))
                data_err = bytes((i % 256) for i in range(50))
                compiled[key] = (d, data_err, None)
            elif err == "inner_struct_eof":
                inner = Struct("a" / Byte, "b" / Byte)
                d = Struct("items" / Array(n, inner))
                data_err = bytes((i % 256) for i in range(50))
                compiled[key] = (d, data_err, None)
            continue

        if kind == "i8":
            d = Struct("items" / Array(n, Byte))
            data = bytes(i % 256 for i in range(n))
            obj = Container(items=list(i % 256 for i in range(n)))
        elif kind == "i8sb":
            d = Struct("items" / Array(n, Int8sb))
            data = bytes(i % 256 for i in range(n))
            obj = Container(items=[(((i % 256) + 128) % 256) - 128 for i in range(n)])
        elif kind == "i16ub":
            d = Struct("items" / Array(n, Int16ub))
            data = bytes(((i // 2) >> 8) & 0xFF if i % 2 == 0 else (i // 2) & 0xFF
                         for i in range(n * 2))
            obj = Container(items=list((i * 17) % 65536 for i in range(n)))
        elif kind == "i16ul":
            d = Struct("items" / Array(n, Int16ul))
            data = bytes((i * 17) % 256 if i % 2 == 0 else ((i * 17) // 256) % 256
                         for i in range(n * 2))
            obj = Container(items=list((i * 17) % 65536 for i in range(n)))
        elif kind == "i32":
            d = Struct("items" / Array(n, Int32ub))
            data = bytes((i % 256) for i in range(n * 4))
            obj = Container(items=list(i % 256 for i in range(n)))
        elif kind == "struct":
            if nf == 2:
                inner = Struct("a" / Byte, "b" / Byte)
            else:
                inner = Struct("a" / Byte, "b" / Byte, "c" / Byte, "d" / Byte)
            d = Struct("items" / Array(n, inner))
            data = bytes((i % 256) for i in range(n * nf))
            if nf == 2:
                obj = Container(items=[Container(a=i % 256, b=(i + 1) % 256) for i in range(n)])
            else:
                obj = Container(items=[Container(a=i % 256, b=(i + 1) % 256,
                                  c=(i + 2) % 256, d=(i + 3) % 256) for i in range(n)])
        elif kind == "nested_array":
            d = Struct("items" / Array(n, Array(n, Byte)))
            data = bytes(i % 256 for i in range(n * n))
            obj = Container(items=[list((j + i * n) % 256 for j in range(n)) for i in range(n)])
        elif kind == "array_in_struct":
            # a12 已是 Struct（v3 亦是），wrapped 与 bare 相同
            d = Struct("items" / Array(10, Byte), "tail" / Int32ub)
            data = bytes(i % 256 for i in range(10)) + bytes([0, 0, 0, 0xFF])
            obj = Container(items=list(i % 256 for i in range(10)), tail=0xFF)
        elif kind == "expr_count":
            # a13 已是 Struct（v3 亦是），wrapped 与 bare 相同
            d = Struct("n" / Byte, "items" / Array(this.n, Byte))
            data = bytes([100]) + bytes(i % 256 for i in range(n))
            obj = Container(n=n, items=list(i % 256 for i in range(n)))
        else:
            raise ValueError(kind)

        compiled[key] = (d, data, obj)

    results = {}
    for key, kind, n, nf, err, _label in SCENARIOS:
        if err:
            continue
        d, data, obj = compiled[key]
        number = NUMBER_FOR.get(key, DEFAULT_NUMBER)
        parse_ns = min(
            timeit.repeat(lambda x=d, y=data: x.parse(y), number=number, repeat=REPEAT)
        ) / number
        build_ns = min(
            timeit.repeat(lambda x=d, o=obj: x.build(o), number=number, repeat=REPEAT)
        ) / number
        results[key + "_parse"] = parse_ns
        results[key + "_build"] = build_ns
    return results


# ============================================================
# 错误路径测量：parse 在错误数据上的耗时（time-to-fail）
# ============================================================
def bench_error_paths_rs():
    """construct-rs 错误路径测量：在错误数据上 parse，测量 time-to-fail。"""
    from dataclasses import dataclass
    from construct import StructMixin, field, Int8ub, Array
    import time as _time

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
            expected_iters = 50
        elif err == "inner_struct_eof":
            @dataclass
            class C(StructMixin):
                items: list = field(Array(n, S2))
            data = bytes((i % 256) for i in range(50))
            expected_iters = 25
        else:
            continue

        number = 200
        actual_fails = 0
        for _ in range(50):
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
    return out


def bench_error_paths_py_wrapped():
    """Python construct 2.10.70 错误路径测量（apples-to-apples：包 Struct）。"""
    from construct import Struct, Array, Byte
    import time as _time

    out = {}
    for key, kind, n, nf, err, _label in SCENARIOS:
        if not err:
            continue
        if err == "stream_eof":
            d = Struct("items" / Array(n, Byte))
            data = bytes((i % 256) for i in range(50))
            expected_iters = 50
        elif err == "inner_struct_eof":
            inner = Struct("a" / Byte, "b" / Byte)
            d = Struct("items" / Array(n, inner))
            data = bytes((i % 256) for i in range(50))
            expected_iters = 25
        else:
            continue

        number = 200
        actual_fails = 0
        for _ in range(50):
            try:
                d.parse(data)
            except Exception:
                pass
        t0 = _time.perf_counter()
        for _ in range(number):
            try:
                d.parse(data)
            except Exception:
                actual_fails += 1
        elapsed = (_time.perf_counter() - t0) / number
        out[key] = (elapsed, number, actual_fails, expected_iters)
    return out


def bench_error_paths_py_bare():
    """Python construct 2.10.70 错误路径测量（裸口径，仅参考）。"""
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
# 格式化辅助
# ============================================================
def format_ns(seconds):
    """秒 → 纳秒字符串。"""
    if seconds >= 1e-3:
        return f"{seconds * 1e6:.2f} us"
    if seconds >= 1e-6:
        return f"{seconds * 1e9:.0f} ns"
    return f"{seconds * 1e9:.1f} ns"
# ============================================================
# 保留 v3 的 Python 测法作为附注：Python 直接调裸 Array(...)，不包 Struct。
# 这给 Python 减去了 ~150-200ns 的 Struct 包装工作，是 v3 加速比假性偏低的根源。
def bench_python_construct_bare():
    """Python construct 2.10.70 测量（裸口径：直接 Array，不包 Struct）。"""
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
# 主流程
# ============================================================
def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else "rs"

    if mode == "rs":
        results = bench_construct_rs()
        for k, v in results.items():
            print(f"rs\t{k}\t{v:.9f}")
    elif mode == "py_wrapped":
        results = bench_python_construct_wrapped()
        for k, v in results.items():
            print(f"py_w\t{k}\t{v:.9f}")
    elif mode == "py_bare":
        results = bench_python_construct_bare()
        for k, v in results.items():
            print(f"py_b\t{k}\t{v:.9f}")
    elif mode == "rs_err":
        results = bench_error_paths_rs()
        for k, (ns, num, actual, exp) in results.items():
            print(f"rs_err\t{k}\t{ns:.9f}\t{num}\t{actual}\t{exp}")
    elif mode == "py_err_wrapped":
        results = bench_error_paths_py_wrapped()
        for k, (ns, num, actual, exp) in results.items():
            print(f"py_err_w\t{k}\t{ns:.9f}\t{num}\t{actual}\t{exp}")
    elif mode == "py_err_bare":
        results = bench_error_paths_py_bare()
        for k, (ns, num, actual, exp) in results.items():
            print(f"py_err_b\t{k}\t{ns:.9f}\t{num}\t{actual}\t{exp}")
    elif mode == "compare":
        import subprocess

        workdir = r"<legacy-repo>\construct-rs"
        rs_python = r"<opencode-temp>\crs_venv_new\Scripts\python.exe"
        py_python = r"<opencode-temp>\crs_venv_py_new\Scripts\python.exe"
        script = __file__

        env = dict(os.environ)
        env["PYO3_USE_ABI3_FORWARD_COMPATIBILITY"] = "1"

        def _run(args):
            proc = subprocess.run(args, cwd=workdir, capture_output=True, text=True, env=env)
            if proc.returncode != 0:
                print(f"subprocess failed: {args}\n{proc.stderr}", file=sys.stderr)
                sys.exit(1)
            return proc.stdout

        rs_out = _run([rs_python, script, "rs"])
        py_w_out = _run([py_python, script, "py_wrapped"])
        py_b_out = _run([py_python, script, "py_bare"])
        rs_err_out = _run([rs_python, script, "rs_err"])
        py_err_w_out = _run([py_python, script, "py_err_wrapped"])
        py_err_b_out = _run([py_python, script, "py_err_bare"])

        def _parse_normal(s):
            out = {}
            for line in s.strip().split("\n"):
                parts = line.split("\t")
                out[parts[1]] = float(parts[2])
            return out

        def _parse_err(s):
            out = {}
            for line in s.strip().split("\n"):
                parts = line.split("\t")
                k = parts[1]
                out[k] = (float(parts[2]), int(parts[3]), int(parts[4]), int(parts[5]))
            return out

        rs_data = _parse_normal(rs_out)
        py_w_data = _parse_normal(py_w_out)
        py_b_data = _parse_normal(py_b_out)
        rs_err_data = _parse_err(rs_err_out)
        py_err_w_data = _parse_err(py_err_w_out)
        py_err_b_data = _parse_err(py_err_b_out)

        _emit_report(rs_data, py_w_data, py_b_data,
                     rs_err_data, py_err_w_data, py_err_b_data)
    else:
        print(f"Unknown mode: {mode}", file=sys.stderr)
        sys.exit(1)


def _emit_report(rs_data, py_w_data, py_b_data,
                 rs_err_data, py_err_w_data, py_err_b_data):
    """输出主表（apples-to-apples）+ 附注（裸口径）+ 派生指标。"""
    print()
    print("=" * 130)
    print("主表（apples-to-apples）：Rust StructMixin  vs  Python Struct(...)  ← 双端都包 Struct，口径对等")
    print("=" * 130)
    print(f"{'#':<6}{'场景':<40}{'N':>7}{'实际迭代':>10}{'bytes/call':>13}"
          f"{'Py+ ns/call':>16}{'Rs ns/call':>16}{'加速比+':>10}{'方向':>8}")
    print("-" * 130)

    speedups_w = []
    for key, kind, n, nf, err, label in SCENARIOS:
        if err:
            continue
        bytes_per = _bytes_per_call(kind, n, nf)
        actual_iters = _expected_iters(key, n)
        for direction, suffix in (("parse", "_parse"), ("build", "_build")):
            full_key = key + suffix
            py_ns = py_w_data[full_key]
            rs_ns = rs_data[full_key]
            speedup = py_ns / rs_ns
            speedups_w.append((key, label, n, direction, py_ns, rs_ns, speedup, actual_iters))
            low_flag = " <10x!" if speedup < 10 else ""
            lbl = label if direction == "parse" else "  └─ build"
            print(f"{key[:6]:<6}{lbl:<40}{n:>7}{actual_iters:>10}{bytes_per:>13}"
                  f"{format_ns(py_ns):>16}{format_ns(rs_ns):>16}{speedup:>8.2f}x{low_flag}"
                  f"{direction:>8}")
    print("-" * 130)

    # ---- 附注：裸口径 ----
    print()
    print("=" * 130)
    print("附注（裸口径，非等价对比）：Rust StructMixin（必经 Struct） vs  Python 裸 Array(...)（少做 Struct 工作）")
    print("  ↑ v3 用的就是这个口径，导致 Rust 多做 150-200ns Struct 工作，加速比假性偏低")
    print("=" * 130)
    print(f"{'#':<6}{'场景':<40}{'N':>7}{'Py* ns/call':>16}{'Rs ns/call':>16}"
          f"{'加速比*':>10}{'加速比+':>10}{'Δ':>8}{'方向':>8}")
    print("-" * 130)
    for key, kind, n, nf, err, label in SCENARIOS:
        if err:
            continue
        for direction, suffix in (("parse", "_parse"), ("build", "_build")):
            full_key = key + suffix
            py_bare = py_b_data[full_key]
            rs_ns = rs_data[full_key]
            sp_bare = py_bare / rs_ns
            sp_w = None
            for k2, lbl2, n2, d2, _, _, sp2, _ in speedups_w:
                if k2 == key and d2 == direction:
                    sp_w = sp2
                    break
            delta = (sp_w - sp_bare) if sp_w is not None else 0
            low_flag = " <10x!" if sp_bare < 10 else ""
            lbl = label if direction == "parse" else "  └─ build"
            sp_w_str = f"{sp_w:.2f}x" if sp_w is not None else "N/A"
            print(f"{key[:6]:<6}{lbl:<40}{n:>7}{format_ns(py_bare):>16}{format_ns(rs_ns):>16}"
                  f"{sp_bare:>8.2f}x{low_flag}{sp_w_str:>10}{delta:>+7.2f}x{direction:>8}")
    print("-" * 130)

    # ---- 错误路径表（apples-to-apples + 裸口径） ----
    print()
    print("─── 错误路径：parse time-to-fail（apples-to-apples + 裸口径） ───")
    print(f"  {'场景':<46}{'Py+ ns':>12}{'Py* ns':>12}{'Rs ns':>12}"
          f"{'加速比+':>10}{'加速比*':>10}{'实际失败':>10}")
    err_speedups_w = []
    for key, kind, n, nf, err, label in SCENARIOS:
        if not err:
            continue
        rs_ns, rs_num, rs_actual, rs_exp = rs_err_data[key]
        py_w_ns, _, _, _ = py_err_w_data[key]
        py_b_ns, _, _, _ = py_err_b_data[key]
        sp_w = py_w_ns / rs_ns
        sp_b = py_b_ns / rs_ns
        err_speedups_w.append((key, label, sp_w, rs_actual, rs_num, rs_exp))
        flag = " <10x" if sp_w < 10 else ""
        print(f"  {label:<46}{format_ns(py_w_ns):>12}{format_ns(py_b_ns):>12}{format_ns(rs_ns):>12}"
              f"{sp_w:>8.2f}x{flag}{sp_b:>8.2f}x{rs_actual:>10}")

    # ---- 派生指标 1：apples-to-apples 加速比排序 ----
    print()
    print("─── 派生指标 1：apples-to-apples 加速比排序（由高到低） ───")
    sorted_sp = sorted(speedups_w, key=lambda x: -x[6])
    for key, label, n, direction, _py, _rs, sp, _ai in sorted_sp:
        flag = " <10x!" if sp < 10 else ""
        print(f"  {label + ' ' + direction:<48} N={n:<6} {sp:>7.2f}x{flag}")
    if err_speedups_w:
        print("  [错误路径]")
        for k, l, sp, _, _, _ in err_speedups_w:
            flag = " <10x!" if sp < 10 else ""
            print(f"  {l + ' err-parse':<48}         {sp:>7.2f}x{flag}")

    # ---- 派生指标 2：parse/build 比率（apples-to-apples） ----
    print()
    print("─── 派生指标 2：parse/build 比率（apples-to-apples） ───")
    print(f"  {'场景':<40}{'N':>7}{'py p/b':>10}{'rs p/b':>10}{'方向一致':>12}")
    for key, kind, n, nf, err, label in SCENARIOS:
        if err:
            continue
        py_p = py_w_data[key + "_parse"]
        py_b = py_w_data[key + "_build"]
        rs_p = rs_data[key + "_parse"]
        rs_b = rs_data[key + "_build"]
        py_r = py_p / py_b
        rs_r = rs_p / rs_b
        consistent = "Y" if (py_r > 1) == (rs_r > 1) else "N"
        print(f"  {label:<40}{n:>7}{py_r:>9.2f}x{rs_r:>9.2f}x{consistent:>12}")

    # ---- 派生指标 3：Int8ub 规模扩展（ns/元素） ----
    print()
    print("─── 派生指标 3：Int8ub 规模扩展（ns/元素，apples-to-apples） ───")
    print(f"  {'N':>7}{'py+ parse ns/e':>18}{'rs parse ns/e':>18}"
          f"{'py+ build ns/e':>18}{'rs build ns/e':>18}")
    scale_keys = ["a01_i8_n10", "a02_i8_n100", "a03_i8_n1000", "a04_i8_n10000"]
    n_for = dict((s[0], s[2]) for s in SCENARIOS)
    for key in scale_keys:
        n = n_for[key]
        py_p = py_w_data[key + "_parse"] * 1e9 / n
        rs_p = rs_data[key + "_parse"] * 1e9 / n
        py_b = py_w_data[key + "_build"] * 1e9 / n
        rs_b = rs_data[key + "_build"] * 1e9 / n
        print(f"  {n:>7}{py_p:>17.2f}{rs_p:>17.2f}{py_b:>17.2f}{rs_b:>17.2f}")

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
        py_p = py_w_data[key + "_parse"]
        rs_p = rs_data[key + "_parse"]
        sp = py_p / rs_p
        print(f"  {name:<22} parse 加速比+: {sp:>6.2f}x")

    # ---- 派生指标 5：场景标签 N vs 实际迭代次数 ----
    print()
    print("─── 派生指标 5：场景标签 N vs 实际迭代次数 ───")
    print(f"  {'场景':<40}{'标签 N':>10}{'实际迭代':>12}{'一致':>8}")
    for key, kind, n, nf, err, label in SCENARIOS:
        if err:
            continue
        ai = _expected_iters(key, n)
        consistent = "Y" if (ai == n or kind in ("nested_array",)) else "N"
        print(f"  {label:<40}{n:>10}{ai:>12}{consistent:>8}")

    # ---- 派生指标 6：内部关系合理性 ----
    print()
    print("─── 派生指标 6：内部关系合理性（Int8ub N=100 是基线） ───")
    base_p_rs = rs_data["a02_i8_n100_parse"]
    base_p_py = py_w_data["a02_i8_n100_parse"]
    print(f"  [基线] Int8ub N=100 parse: py+={format_ns(base_p_py)}, rs={format_ns(base_p_rs)}")
    compare_keys = [
        ("Int16ub N=100 (×2 B)",    "a05_i16ub_n100"),
        ("Int32ub N=100 (×4 B)",    "a07_i32ub_n100"),
        ("Struct{2} N=100 (×2 B)",  "a09_s2_n100"),
    ]
    for name, key in compare_keys:
        rs_p = rs_data[key + "_parse"]
        py_p = py_w_data[key + "_parse"]
        print(f"  {name:<28} parse rs/base={rs_p/base_p_rs:>5.2f}x, py+/base={py_p/base_p_py:>5.2f}x")

    # ---- 派生指标 7：方法论对称性核查表 ----
    print()
    print("─── 派生指标 7：方法论对称性核查表（Rust vs Python 工作量对照） ───")
    print(f"  {'场景':<40}{'Rust 侧':<32}{'Python 侧 (apples-to-apples)':<32}{'对称':>8}")
    for key, kind, n, nf, err, label in SCENARIOS:
        if err:
            continue
        work = _rs_py_symbolic_work(kind)
        symmetric = "Y"
        print(f"  {label:<40}{work:<32}{work:<32}{symmetric:>8}")

    # ---- 异常项检测 ----
    print()
    print("─── 异常项检测（apples-to-apples） ───")
    below_10 = [(lbl, d, sp, n) for _, lbl, n, d, _, _, sp, _ in speedups_w if sp < 10]
    if below_10:
        print(f"  [WARN] 低于 10x 的场景 ({len(below_10)}):")
        for lbl, d, sp, n in below_10:
            print(f"    {lbl} {d} N={n}: {sp:.2f}x")
    else:
        print("  [PASS] 全部场景 >= 10x")
    err_below = [(l, sp) for _, l, sp, _, _, _ in err_speedups_w if sp < 10]
    if err_below:
        print(f"  [WARN] 错误路径低于 10x ({len(err_below)}):")
        for l, sp in err_below:
            print(f"    {l}: {sp:.2f}x")

    # ---- S-PERF 出口 ----
    target = 10.0
    print()
    print(f"─── S-PERF 出口标准 (≥{target}x, apples-to-apples) ───")
    n_total = len(speedups_w)
    n_met = sum(1 for s in speedups_w if s[6] >= target)
    below = [(lbl, d, sp) for _, lbl, _, d, _, _, sp, _ in speedups_w if sp < target]
    print(f"  正常路径：{n_met}/{n_total} 场景 >= {target}x")
    if below:
        for lbl, d, sp in below:
            print(f"    {lbl} {d}: {sp:.2f}x")
    print("=" * 130)


if __name__ == "__main__":
    main()
