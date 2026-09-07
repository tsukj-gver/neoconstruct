"""探针 5：规模化输入（大缓冲 / 大数组 / 高频小包）。

用户视角：解析 pcap 导出的大文件、固件镜像、百万级记录——内存与时间
是否仍线性可控；服务端高频小包的每包开销。性能承诺（>=4x）的回归守卫
不在 pytest 套件内（tests/integration/test_checksum_bench.py 无 test_ 函数，
pytest 收集为 0；tests/rust 是 Rust 侧 bench）。本探针只测"规模化可行性"，
不做精确基准。

运行：.venv/Scripts/python.exe probe_scale.py
"""

from __future__ import annotations

import sys
import time

sys.path.insert(0, r"D:\Project\Github\neoconstruct\neoconstruct\python")

from dataclasses import dataclass

from neoconstruct import Array, GreedyRange, Int8ub, Int16ub, StructMixin, field

try:
    import tracemalloc

    HAS_TRACE = True
except ImportError:
    HAS_TRACE = False


@dataclass
class Record(StructMixin):
    a: int = field(Int8ub)
    b: int = field(Int16ub)


# --- 5.1 大数组：1M 条 3 字节记录 = 3MB 输入 ---
N = 1_000_000
data = b"".join(b"\x01\x00\x02" for _ in range(N))
assert len(data) == 3 * N


@dataclass
class ManyRecords(StructMixin):
    count: int = field(Int32ub := __import__("neoconstruct").Int32ub)
    items: list = field(Array(count, Record))


t0 = time.perf_counter()
p = ManyRecords.parse(N.to_bytes(4, "big") + data)
t1 = time.perf_counter()
print(f"Array parse 1M records (3MB): {t1 - t0:.2f}s, len(items)={len(p.items)}")

t0 = time.perf_counter()
blob = p.build()
t1 = time.perf_counter()
print(f"Array build 1M records: {t1 - t0:.2f}s, bytes={len(blob)}")

# --- 5.2 GreedyRange 大流 ---
greedy_data = data


@dataclass
class GreedyRecords(StructMixin):
    items: list = field(GreedyRange(Record))


t0 = time.perf_counter()
g = GreedyRecords.parse(greedy_data)
t1 = time.perf_counter()
print(f"GreedyRange parse 1M records: {t1 - t0:.2f}s, len={len(g.items)}")

# --- 5.3 大缓冲内存跟踪（tracemalloc）---
if HAS_TRACE:
    tracemalloc.start()
    p2 = ManyRecords.parse(N.to_bytes(4, "big") + data)
    cur, peak = tracemalloc.get_traced_memory()
    tracemalloc.stop()
    print(f"parse 3MB input: traced current={cur / 1e6:.1f}MB peak={peak / 1e6:.1f}MB "
          f"(input 3.0MB, 放大系数={peak / 3e6:.2f}x)")

# --- 5.4 高频小包（服务端形态）：20 万次 8 字节包 ---
@dataclass
class SmallPacket(StructMixin):
    t: int = field(Int8ub)
    n: int = field(Int8ub)
    payload: bytes = field(Array(n, Int8ub))


pkt = SmallPacket(t=1, n=4, payload=[1, 2, 3, 4]).build()
t0 = time.perf_counter()
M = 200_000
for _ in range(M):
    SmallPacket.parse(pkt)
t1 = time.perf_counter()
print(f"small-packet parse {M}x 8-byte frames: {t1 - t0:.2f}s -> {(t1 - t0) / M * 1e6:.1f} us/packet")
