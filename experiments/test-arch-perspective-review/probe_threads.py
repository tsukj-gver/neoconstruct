"""探针 2：并发使用（多线程共享同一编译类 parse/build）。

用户视角：服务进程里多个 worker 线程/asyncio 任务共用同一个协议类。
编译产物 `_construct_compiled` 是类级共享状态——若 Rust 侧有任何可变
全局/共享缓存，这里是数据竞争的温床。现有套件无任何并发用例
（结构性原因：所有测试都是单线程单类单次调用语义断言）。

运行：.venv/Scripts/python.exe probe_threads.py
"""

from __future__ import annotations

import sys
import threading

sys.path.insert(0, r"D:\Project\Github\neoconstruct\neoconstruct\python")

from dataclasses import dataclass

from neoconstruct import Array, Bytes, Checksum, Int16ub, Int8ub, StructMixin, Tell, field, rfield


def crc16(data: bytes) -> bytes:
    crc = 0xFFFF
    for b in data:
        crc ^= b
        for _ in range(8):
            crc = (crc >> 1) ^ 0xA001 if crc & 1 else crc >> 1
    return bytes([crc & 0xFF, (crc >> 8) & 0xFF])


@dataclass
class Item(StructMixin):
    x: int = field(Int8ub)
    y: int = field(Int16ub)


@dataclass
class Packet(StructMixin):
    start: int = rfield(Tell())
    count: int = field(Int8ub)
    items: list = field(Array(count, Item))
    blob: bytes = field(Bytes(4))
    end: int = rfield(Tell())
    crc: bytes = field(Checksum(Bytes(2), crc16, start=start, end=end))


N_THREADS = 8
N_ITER = 1000

data = Packet(count=3, items=[Item(1, 2), Item(3, 4), Item(5, 6)], blob=b"ABCD", crc=b"\x00\x00").build()
# build 时 crc 由 Checksum 重算，parse 验证
expected = Packet.parse(data)

errors: list[str] = []


def worker(tid: int) -> None:
    try:
        for i in range(N_ITER):
            p = Packet.parse(data)
            if p != expected:
                errors.append(f"t{tid}/{i}: wrong parse result")
                return
            if p.build() != data:
                errors.append(f"t{tid}/{i}: wrong build result")
                return
    except Exception as e:
        errors.append(f"t{tid}: {type(e).__name__}: {e}")


threads = [threading.Thread(target=worker, args=(t,)) for t in range(N_THREADS)]
for t in threads:
    t.start()
for t in threads:
    t.join()

print(f"threads={N_THREADS} iters={N_ITER}")
if errors:
    print("FAIL:", errors[:5])
else:
    print("OK: concurrent parse/build on shared class, all results correct")

# 并发定义新类（dev-in-prod 场景：动态协议注册）
def definer(seed: int) -> None:
    try:
        for i in range(50):
            ns = {"__annotations__": {"v": int}, "v": field(Int8ub)}
            cls = dataclass(type(f"Dyn{seed}_{i}", (StructMixin,), ns))
            cls.parse(b"\x05")
    except Exception as e:
        errors.append(f"definer{seed}: {type(e).__name__}: {e}")


dyn_errors: list[str] = []
ths = [threading.Thread(target=definer, args=(s,)) for s in range(4)]
for t in ths:
    t.start()
for t in ths:
    t.join()
print("concurrent class definition:", "OK" if not dyn_errors else dyn_errors[:3])
