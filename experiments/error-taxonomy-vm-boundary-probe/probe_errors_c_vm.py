"""探针 C：G3 跟进——NamedTuple 运行时 / Timestamp / Adapter 正确签名 / StopField。

另验证 G4 表达式 VM 算术边界（除零 / 溢出 / 移位 / 大值消费）。
"""

import sys
import traceback

sys.path.insert(0, r"D:\Project\Github\neoconstruct\neoconstruct\python")

from dataclasses import dataclass  # noqa: E402
from typing import Any  # noqa: E402

import neoconstruct as nc  # noqa: E402
from neoconstruct import (  # noqa: E402
    Array,
    Bytes,
    Element,
    Int64sb,
    Int64ub,
    Int8ub,
    NamedTuple,
    RepeatUntil,
    StopIf,
    StructMixin,
    field,
    rfield,
)


def run(label, fn):
    print(f"===== {label} =====")
    try:
        result = fn()
        print(f"  [NO-RAISE] result = {result!r}")
    except Exception as e:
        print(f"  type    = {type(e).__name__}")
        print(f"  is CE   = {isinstance(e, nc.ConstructError)}")
        print(f"  message = {str(e)[:180]!r}")
        print(f"  path    = {getattr(e, 'path', '<no attr>')!r}")
    print()


# ---- NamedTuple ----

def namedtuple_parse_inner_not_struct():
    @dataclass
    class P(StructMixin):
        coord: Any = field(NamedTuple("coord", "x y", Int8ub))

    return P.parse(b"\x01\x02")


# ---- Timestamp（Int32ub 已在本文件顶部通过 nc.Int32ub 引用）----

def timestamp_bad_unit():
    return nc.Timestamp(nc.Int32ub, unit=object(), epoch=1)


def timestamp_bad_epoch():
    return nc.Timestamp(nc.Int32ub, unit=1, epoch=object())


def timestamp_bad_subcon():
    return nc.Timestamp(None, 1, 1970)


# ---- Adapter 正确签名（obj, context, path）----

def generic_from_user_valueerror():
    from neoconstruct import Adapter

    class Weird(Adapter):
        def _decode(self, obj, context, path):
            raise ValueError("user callback failure")

        def _encode(self, obj, context, path):
            return obj

    @dataclass
    class P(StructMixin):
        x: Any = field(Weird(Int8ub))

    return P.parse(b"\x01")


# ---- StopField：Array 内 StopIf 的 parse 输出 ----

def stopfield_array_output():
    @dataclass
    class Inner(StructMixin):
        a: int = field(Int8ub)
        stop: Any = rfield(StopIf(True))
        b: int = field(Int8ub, default=0)

    @dataclass
    class P(StructMixin):
        items: list = field(Array(2, Inner))

    pkt = P.parse(bytes([0x10, 0x20]))
    return [(i.a, i.b) for i in pkt.items]


# ---- G4：表达式 VM 算术边界 ----

def vm_floordiv_zero():
    @dataclass
    class P(StructMixin):
        n: int = field(Int8ub)
        c: int = rfield(nc.Computed(n // 0))

    return P.parse(b"\x05")


def vm_mod_zero():
    @dataclass
    class P(StructMixin):
        n: int = field(Int8ub)
        c: int = rfield(nc.Computed(n % 0))

    return P.parse(b"\x05")


def vm_i64_max_plus_one():
    @dataclass
    class P(StructMixin):
        n: int = field(Int64ub)
        c: int = rfield(nc.Computed(n + 1))

    pkt = P.parse((2**63 - 1).to_bytes(8, "big"))
    return pkt.c


def vm_i64_min_unary_neg():
    @dataclass
    class P(StructMixin):
        n: int = field(Int64sb)
        c: int = rfield(nc.Computed(-n))

    pkt = P.parse((-2**63).to_bytes(8, "big", signed=True))
    return pkt.c


def vm_big_mul_wraps():
    @dataclass
    class P(StructMixin):
        n: int = field(Int64ub)
        c: int = rfield(nc.Computed(n * n))

    pkt = P.parse((2**63 - 1).to_bytes(8, "big"))
    return pkt.c


def vm_shl_64():
    @dataclass
    class P(StructMixin):
        x: int = field(Int8ub)
        c: int = rfield(nc.Computed(x << 64))

    return P.parse(b"\x01").c


def vm_shl_63():
    @dataclass
    class P(StructMixin):
        x: int = field(Int8ub)
        c: int = rfield(nc.Computed(x << 63))

    return P.parse(b"\x01").c


def vm_shl_negative():
    @dataclass
    class P(StructMixin):
        x: int = field(Int8ub)
        c: int = rfield(nc.Computed(x << -1))

    return P.parse(b"\x01").c


def vm_shr_64():
    @dataclass
    class P(StructMixin):
        x: int = field(Int8ub)
        c: int = rfield(nc.Computed(x >> 64))

    return P.parse(b"\xff").c


def vm_bytes_len_overflow():
    # Bytes(n * 2)：n=2**62 → wrap 至 0 → parse 得 b""
    @dataclass
    class P(StructMixin):
        n: int = field(Int64ub)
        data: bytes = field(Bytes(n * 2))

    pkt = P.parse((2**62).to_bytes(8, "big"))
    return pkt.data


def vm_bytes_len_exceeds_stream():
    @dataclass
    class P(StructMixin):
        n: int = field(Int8ub)
        data: bytes = field(Bytes(n * 2))

    # n=100 → 需 200 字节，流仅 3 字节
    return P.parse(bytes([100, 1, 2]))


def vm_bytes_len_div_zero():
    @dataclass
    class P(StructMixin):
        n: int = field(Int8ub)
        data: bytes = field(Bytes(n // 0))

    return P.parse(bytes([5, 1, 2]))


if __name__ == "__main__":
    run("NamedTuple parse inner=Int8ub", namedtuple_parse_inner_not_struct)
    run("Timestamp bad unit", timestamp_bad_unit)
    run("Timestamp bad epoch", timestamp_bad_epoch)
    run("Timestamp bad subcon None", timestamp_bad_subcon)
    run("Generic from user ValueError", generic_from_user_valueerror)
    run("StopField Array output", stopfield_array_output)
    print("################ G4 VM ################")
    run("VM n // 0", vm_floordiv_zero)
    run("VM n % 0", vm_mod_zero)
    run("VM (2**63-1)+1", vm_i64_max_plus_one)
    run("VM -(i64::MIN)", vm_i64_min_unary_neg)
    run("VM (2**63-1)**2 wraps", vm_big_mul_wraps)
    run("VM 1 << 64", vm_shl_64)
    run("VM 1 << 63", vm_shl_63)
    run("VM 1 << -1", vm_shl_negative)
    run("VM 0xff >> 64", vm_shr_64)
    run("VM Bytes(2**62 * 2) wraps to 0", vm_bytes_len_overflow)
    run("VM Bytes(200) stream 3 bytes", vm_bytes_len_exceeds_stream)
    run("VM Bytes(n // 0)", vm_bytes_len_div_zero)
