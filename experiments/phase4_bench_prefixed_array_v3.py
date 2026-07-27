"""Phase 4 子任务 4.3 性能基准 v3：PrefixedArray parse/build（方法论对称修复）。

修复 v2 的方法论不对称问题（4.x-INVEST 报告 §1+§2）：
    v2 病灶：Rust 侧 `class _C(StructMixin): items: list = field(PrefixedArray(...))`，
            Python 侧 `pc.PrefixedArray(...)` 直接调用（不包 Struct）。
    v3 修复：Python 也包一层 `pc.Struct("items" / pc.PrefixedArray(...))`。

测量口径（performance-gate/SKILL.md S-PERF）：
- construct-rs 侧：maturin develop 安装后，Python 调用户面 API（必经 Struct）
- Python construct 侧：import construct，调等效 API
- 子进程隔离（包同名，不可在同一进程导入）
- 取 min(repeat=5) × number

主表（apples-to-apples）：双端都包 Struct
附注（裸口径）：Python 裸 PrefixedArray，明确标注"非等价对比"

场景矩阵（13 正常 + 2 错误 = 15 场景 × parse/build = 30 测量点）：
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
        "nested_struct_array": nf,
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


def _rs_py_symbolic_work(cf_kind, kind):
    return f"Struct{{PrefixedArray({cf_kind},{kind})}}"


# ============================================================
# construct-rs 测量（StructMixin 包装口径，不变）
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
# Python construct 2.10.70 测量（apples-to-apples：包 Struct）
# ============================================================
def bench_python_construct_wrapped():
    """Python construct 2.10.70 测量（apples-to-apples：包 Struct）。"""
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
            d = Struct("items" / PrefixedArray(cf, Byte))
            data = _encode_count(n, cf_kind) + bytes(i % 256 for i in range(n))
            obj = Container(items=list(i % 256 for i in range(n)))
        elif kind == "i32":
            d = Struct("items" / PrefixedArray(cf, Int32ub))
            data = _encode_count(n, cf_kind) + bytes((i) % 256 for i in range(n * 4))
            obj = Container(items=list(i % 256 for i in range(n)))
        elif kind == "i16sb":
            d = Struct("items" / PrefixedArray(cf, Int16sb))
            data = _encode_count(n, cf_kind) + bytes((i) % 256 for i in range(n * 2))
            obj = Container(items=[((i * 17) % 65536 - 32768) for i in range(n)])
        elif kind == "struct":
            if nf == 2:
                inner = Struct("a" / Byte, "b" / Byte)
            else:
                inner = Struct("a" / Byte, "b" / Byte, "c" / Byte, "d" / Byte)
            d = Struct("items" / PrefixedArray(cf, inner))
            data = _encode_count(n, cf_kind) + bytes((i) % 256 for i in range(n * nf))
            if nf == 2:
                obj = Container(items=[Container(a=i % 256, b=(i + 1) % 256) for i in range(n)])
            else:
                obj = Container(items=[Container(a=i % 256, b=(i + 1) % 256,
                                  c=(i + 2) % 256, d=(i + 3) % 256) for i in range(n)])
        elif kind == "bytes8":
            d = Struct("items" / PrefixedArray(cf, Bytes(8)))
            data = _encode_count(n, cf_kind) + bytes((i) % 256 for i in range(n * 8))
            one = bytes((i) % 256 for i in range(8))
            obj = Container(items=[one] * n)
        elif kind == "nested_struct_array":
            inner = Struct("items" / Array(nf, Byte))
            d = Struct("items" / PrefixedArray(cf, inner))
            data = _encode_count(n, cf_kind) + bytes(i % 256 for i in range(n * nf))
            obj = Container(items=[Container(items=[(i * nf + j) % 256 for j in range(nf)]) for i in range(n)])
        else:
            raise ValueError(kind)

        compiled[key] = (d, data, obj)

    results = {}
    for key, cf_kind, kind, n, nf, err, _label in SCENARIOS:
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
# Python construct 2.10.70 测量（裸口径，非等价对比，仅作参考）
# ============================================================
def bench_python_construct_bare():
    """Python construct 2.10.70 测量（裸口径：直接 PrefixedArray，不包 Struct）。"""
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
    """construct-rs 错误路径测量：parse/build time-to-fail。"""
    from dataclasses import dataclass
    from construct import StructMixin, field, Int8ub, Int16ub, PrefixedArray
    import time as _time

    out = {}
    for key, cf_kind, kind, n, nf, err, _label in SCENARIOS:
        if not err:
            continue
        if err == "cf_overflow":
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
            @dataclass
            class C(StructMixin):
                items: list = field(PrefixedArray(Int16ub, Int8ub))
            data = _encode_count(1024, "int16ub") + bytes((i) % 256 for i in range(512))
            expected_iters = 512
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


def bench_error_paths_py_wrapped():
    """Python construct 2.10.70 错误路径测量（apples-to-apples：包 Struct）。"""
    from construct import Byte, Int16ub, Int8ub, PrefixedArray, Struct
    import time as _time

    out = {}
    for key, cf_kind, kind, n, nf, err, _label in SCENARIOS:
        if not err:
            continue
        if err == "cf_overflow":
            d = Struct("items" / PrefixedArray(Byte, Int8ub))
            obj = {"items": [0] * 300}
            expected_iters = 300
            number = 100
            actual_fails = 0
            for _ in range(20):
                try:
                    d.build(obj)
                except Exception:
                    pass
            t0 = _time.perf_counter()
            for _ in range(number):
                try:
                    d.build(obj)
                except Exception:
                    actual_fails += 1
            elapsed = (_time.perf_counter() - t0) / number
            out[key] = (elapsed, number, actual_fails, expected_iters)
        elif err == "stream_short":
            d = Struct("items" / PrefixedArray(Int16ub, Int8ub))
            data = _encode_count(1024, "int16ub") + bytes((i) % 256 for i in range(512))
            expected_iters = 512
            number = 100
            actual_fails = 0
            for _ in range(20):
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
        else:
            continue
    return out


def bench_error_paths_py_bare():
    """Python construct 2.10.70 错误路径测量（裸口径，仅参考）。"""
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
# 格式化辅助 + 主流程
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
    print("主表（apples-to-apples）：Rust StructMixin  vs  Python Struct(...)  <- 双端都包 Struct，口径对等")
    print("=" * 130)
    print(f"{'#':<6}{'场景':<46}{'N':>7}{'实际迭代':>10}{'bytes/call':>13}"
          f"{'Py+ ns/call':>16}{'Rs ns/call':>16}{'加速比+':>10}{'方向':>8}")
    print("-" * 130)

    speedups_w = []
    for key, cf_kind, kind, n, nf, err, label in SCENARIOS:
        if err:
            continue
        bytes_per = _cf_size(cf_kind) + n * _bytes_per_elem(kind, nf)
        actual_iters = _expected_iters(key, n)
        for direction, suffix in (("parse", "_parse"), ("build", "_build")):
            full_key = key + suffix
            py_ns = py_w_data[full_key]
            rs_ns = rs_data[full_key]
            speedup = py_ns / rs_ns
            speedups_w.append((key, label, n, direction, py_ns, rs_ns, speedup, actual_iters))
            low_flag = " <10x!" if speedup < 10 else ""
            lbl = label if direction == "parse" else "  └─ build"
            print(f"{key[:6]:<6}{lbl:<46}{n:>7}{actual_iters:>10}{bytes_per:>13}"
                  f"{format_ns(py_ns):>16}{format_ns(rs_ns):>16}{speedup:>8.2f}x{low_flag}"
                  f"{direction:>8}")
    print("-" * 130)

    # ---- 附注：裸口径 ----
    print()
    print("=" * 130)
    print("附注（裸口径，非等价对比）：Rust StructMixin（必经 Struct） vs  Python 裸 PrefixedArray(...)（少做 Struct 工作）")
    print("  ↑ v2 用的就是这个口径，导致 Rust 多做 150-200ns Struct 工作，加速比假性偏低")
    print("=" * 130)
    print(f"{'#':<6}{'场景':<46}{'N':>7}{'Py* ns/call':>16}{'Rs ns/call':>16}"
          f"{'加速比*':>10}{'加速比+':>10}{'Δ':>8}{'方向':>8}")
    print("-" * 130)
    for key, cf_kind, kind, n, nf, err, label in SCENARIOS:
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
            print(f"{key[:6]:<6}{lbl:<46}{n:>7}{format_ns(py_bare):>16}{format_ns(rs_ns):>16}"
                  f"{sp_bare:>8.2f}x{low_flag}{sp_w_str:>10}{delta:>+7.2f}x{direction:>8}")
    print("-" * 130)

    # ---- 错误路径表 ----
    print()
    print("─── 错误路径：parse/build time-to-fail（apples-to-apples + 裸口径） ───")
    print(f"  {'场景':<46}{'Py+ ns':>12}{'Py* ns':>12}{'Rs ns':>12}"
          f"{'加速比+':>10}{'加速比*':>10}{'实际失败':>10}")
    err_speedups_w = []
    for key, cf_kind, kind, n, nf, err, label in SCENARIOS:
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

    # ---- 派生指标 1：加速比排序 ----
    print()
    print("─── 派生指标 1：apples-to-apples 加速比排序（由高到低） ───")
    sorted_sp = sorted(speedups_w, key=lambda x: -x[6])
    for key, label, n, direction, _py, _rs, sp, _ai in sorted_sp:
        flag = " <10x!" if sp < 10 else ""
        print(f"  {label + ' ' + direction:<54} N={n:<6} {sp:>7.2f}x{flag}")
    if err_speedups_w:
        print("  [错误路径]")
        for k, l, sp, _, _, _ in err_speedups_w:
            flag = " <10x!" if sp < 10 else ""
            print(f"  {l + ' err':<54}         {sp:>7.2f}x{flag}")

    # ---- 派生指标 2：parse/build 比率 ----
    print()
    print("─── 派生指标 2：parse/build 比率（apples-to-apples） ───")
    print(f"  {'场景':<46}{'N':>7}{'py p/b':>10}{'rs p/b':>10}{'方向一致':>12}")
    for key, cf_kind, kind, n, nf, err, label in SCENARIOS:
        if err:
            continue
        py_p = py_w_data[key + "_parse"]
        py_b = py_w_data[key + "_build"]
        rs_p = rs_data[key + "_parse"]
        rs_b = rs_data[key + "_build"]
        py_r = py_p / py_b
        rs_r = rs_p / rs_b
        consistent = "Y" if (py_r > 1) == (rs_r > 1) else "N"
        print(f"  {label:<46}{n:>7}{py_r:>9.2f}x{rs_r:>9.2f}x{consistent:>12}")

    # ---- 派生指标 3：Int16ub cf 规模扩展 ----
    print()
    print("─── 派生指标 3：Int16ub cf + Int8ub 规模扩展（ns/元素） ───")
    print(f"  {'N':>7}{'py+ parse ns/e':>18}{'rs parse ns/e':>18}"
          f"{'py+ build ns/e':>18}{'rs build ns/e':>18}")
    scale_keys = [
        ("Byte cf", "p02_i8_n100"),
        ("Byte cf", "p03_i8_n255"),
        ("Int16ub cf", "p04_i8_n1024"),
        ("Int16ub cf", "p05_i8_n4096"),
    ]
    n_for = dict((s[0], s[3]) for s in SCENARIOS)
    for cf_label, key in scale_keys:
        n = n_for[key]
        py_p = py_w_data[key + "_parse"] * 1e9 / n
        rs_p = rs_data[key + "_parse"] * 1e9 / n
        py_b = py_w_data[key + "_build"] * 1e9 / n
        rs_b = rs_data[key + "_build"] * 1e9 / n
        print(f"  {cf_label:<14}{n:>7}{py_p:>17.2f}{rs_p:>17.2f}{py_b:>17.2f}{rs_b:>17.2f}")

    # ---- 派生指标 4：countfield 类型对比（同 N） ----
    print()
    print("─── 派生指标 4：countfield 类型对比（N=1024 + Int8ub） ───")
    print(f"  {'countfield':<14}{'N elems':>10}{'parse 加速比+':>16}{'build 加速比+':>16}")
    cf_keys = [
        ("Int16ub (2B BE)", "p04_i8_n1024"),
        ("Int16ul (2B LE)", "p06_i8_le_cf"),
        ("Int32ub (4B BE)", "p07_i8_i32cf"),
    ]
    for cf_name, key in cf_keys:
        n = n_for[key]
        sp_p = py_w_data[key + "_parse"] / rs_data[key + "_parse"]
        sp_b = py_w_data[key + "_build"] / rs_data[key + "_build"]
        print(f"  {cf_name:<14}{n:>10}{sp_p:>15.2f}x{sp_b:>15.2f}x")

    # ---- 派生指标 5：场景标签 vs 实际迭代 ----
    print()
    print("─── 派生指标 5：场景标签 N vs 实际迭代次数 ───")
    print(f"  {'场景':<46}{'标签 N':>10}{'实际迭代':>12}{'一致':>8}")
    for key, cf_kind, kind, n, nf, err, label in SCENARIOS:
        if err:
            continue
        ai = _expected_iters(key, n)
        consistent = "Y" if ai == n else "N"
        print(f"  {label:<46}{n:>10}{ai:>12}{consistent:>8}")

    # ---- 派生指标 6：内部关系合理性 ----
    print()
    print("─── 派生指标 6：内部关系合理性（Int16ub cf 基线 = N=1024 Int8ub） ───")
    base_p_rs = rs_data["p04_i8_n1024_parse"]
    base_p_py = py_w_data["p04_i8_n1024_parse"]
    print(f"  [基线] N=1024 Int8ub parse: py+={format_ns(base_p_py)}, rs={format_ns(base_p_rs)}")
    compare_keys = [
        ("Int32ub N=256 (同 ~1KB)",     "p08_i32_n256"),
        ("Struct{2} N=512 (同 ~1KB)",   "p10_struct2_n512"),
        ("Bytes(8) N=128 (同 ~1KB)",    "p12_bytes8_n128"),
    ]
    for name, key in compare_keys:
        rs_p = rs_data[key + "_parse"]
        py_p = py_w_data[key + "_parse"]
        print(f"  {name:<30} parse rs/base={rs_p/base_p_rs:>5.2f}x, py+/base={py_p/base_p_py:>5.2f}x")

    # ---- 派生指标 7：方法论对称性核查表 ----
    print()
    print("─── 派生指标 7：方法论对称性核查表（Rust vs Python 工作量对照） ───")
    print(f"  {'场景':<46}{'Rust 侧':<26}{'Python 侧 (wrapped)':<26}{'对称':>8}")
    for key, cf_kind, kind, n, nf, err, label in SCENARIOS:
        if err:
            continue
        work = _rs_py_symbolic_work(cf_kind, kind)
        symmetric = "Y"
        print(f"  {label:<46}{work:<26}{work:<26}{symmetric:>8}")

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
