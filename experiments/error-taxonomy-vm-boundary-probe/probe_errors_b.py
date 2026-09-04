"""探针 B：G3 异常类触发验证——迭代/整数/填充/元组/时间戳/旋转/通用/哨兵族。

覆盖：RepeatError / RangeError / IntegerError / PaddingError / NamedTupleError /
TimestampError / RotationError / GenericConstructError / ExplicitError /
StopFieldError / IndexFieldError / SizeofError / UnresolvedReferenceError。
"""

import sys
import traceback

sys.path.insert(0, r"D:\Project\Github\neoconstruct\neoconstruct\python")

from dataclasses import dataclass  # noqa: E402
from typing import Any  # noqa: E402

import neoconstruct as nc  # noqa: E402
from neoconstruct import (  # noqa: E402
    Array,
    BitStructMixin,
    BitsInteger,
    Element,
    Int16ub,
    Int8ub,
    Padding,
    ProcessRotateLeft,
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
        if result is not None:
            print(f"  [NO-RAISE] result = {result!r}")
        else:
            print("  [NO-RAISE]")
    except Exception as e:
        print(f"  type    = {type(e).__name__}")
        print(f"  is CE   = {isinstance(e, nc.ConstructError)}")
        msg = str(e)
        print(f"  message = {msg[:180]!r}")
        path = getattr(e, "path", "<<no attr>>")
        print(f"  path    = {path!r}")
    print()


# ---- RepeatError ----

def repeat_error_build():
    @dataclass
    class P(StructMixin):
        e: int = rfield(Element())
        items: list = field(RepeatUntil(e > 5, Int8ub))

    P(items=[1, 2, 3]).build()


def repeat_until_eof_no_sentinel():
    @dataclass
    class P(StructMixin):
        e: int = rfield(Element())
        items: list = field(RepeatUntil(e > 200, Int8ub))

    # 全流无哨兵 → EOF：StreamError 还是别的？
    P.parse(bytes([1, 2, 3]))


# ---- RangeError ----

def range_error_build_count_mismatch():
    @dataclass
    class P(StructMixin):
        items: list = field(Array(3, Int8ub))

    P(items=[1, 2]).build()


def range_error_negative_count():
    # 负 count：构造期 / 编译期 / 运行期？
    @dataclass
    class P(StructMixin):
        items: list = field(Array(-1, Int8ub))

    return "compiled"


# ---- IntegerError（BitsInteger）----

def integer_error_out_of_range():
    @dataclass
    class B(BitStructMixin):
        v: int = field(BitsInteger(8))

    B(v=256).build()


def integer_error_wrong_type():
    @dataclass
    class B(BitStructMixin):
        v: int = field(BitsInteger(8))

    B(v="zzz").build()  # type: ignore[arg-type]


# ---- PaddingError ----

def padding_error_bit_pattern():
    # bit 域 pattern 仅接受 0/1 → 编译期 PaddingError
    @dataclass
    class B(BitStructMixin):
        a: int = field(BitsInteger(4))
        pad: Any = field(Padding(4, pattern=2))


def padding_error_byte_pattern_range():
    @dataclass
    class P(StructMixin):
        pad: Any = field(Padding(4, pattern=256))


# ---- NamedTupleError ----

def namedtuple_error_inner_not_struct():
    @dataclass
    class P(StructMixin):
        coord: Any = field(nc.NamedTuple("coord", "x y", Int8ub))

    return "compiled"


# ---- TimestampError ----

def timestamp_error_bad_unit():
    nc.Timestamp(Int32ub, unit=object(), epoch=1)


def timestamp_error_bad_epoch():
    nc.Timestamp(Int32ub, unit=1, epoch=object())


# ---- RotationError ----

def rotation_error_misaligned():
    @dataclass
    class P(StructMixin):
        v: int = field(ProcessRotateLeft(4, 3, Int16ub))

    # Int16ub 数据 2 字节，group=3 → 2 % 3 != 0 → RotationError
    P.parse(b"\x0f\xf0")


def rotation_error_group_zero():
    @dataclass
    class P(StructMixin):
        v: int = field(ProcessRotateLeft(4, 0, Int16ub))

    P.parse(b"\x0f\xf0")


# ---- GenericConstructError / ExplicitError（用户 Adapter 路径）----

def generic_error_from_user_adapter():
    from neoconstruct import Adapter

    class Weird(Adapter):
        def _decode(self, obj, ctx):
            raise ValueError("user callback failure")

        def _encode(self, obj, ctx):
            return obj

    @dataclass
    class P(StructMixin):
        x: Any = field(Weird(Int8ub))

    P.parse(b"\x01")


def explicit_error_via_select():
    from neoconstruct import Adapter, ExplicitError, Select

    class Exploder(Adapter):
        def _decode(self, obj, ctx):
            raise ExplicitError("explicit stop")

        def _encode(self, obj, ctx):
            return obj

    @dataclass
    class P(StructMixin):
        x: Any = field(Select(Exploder(Int8ub), Int16ub))

    P.parse(b"\x01\x02")


# ---- StopFieldError：逃逸场景 ----

def stop_field_escape_from_array():
    @dataclass
    class Inner(StructMixin):
        a: int = field(Int8ub)
        stop: Any = rfield(StopIf(True))
        b: int = field(Int8ub, default=0)

    @dataclass
    class P(StructMixin):
        items: list = field(Array(2, Inner))

    P.parse(bytes([0x10, 0x20]))


def stop_field_escape_from_sequence():
    @dataclass
    class P(StructMixin):
        seq: Any = field(nc.Sequence(Int8ub, Int8ub))

    return "skip"


# ---- IndexFieldError / SizeofError：可达性 ----

def index_field_accessibility():
    return "IndexFieldError reserved; Index 缺 ctx 时返回 None（docstring）"


def sizeof_api_availability():
    @dataclass
    class P(StructMixin):
        x: int = field(Int8ub)

    return "has sizeof attr: {}".format(hasattr(P, "sizeof"))


# ---- UnresolvedReferenceError：可达性 ----

def unresolved_ref_attempt():
    # 尝试：B 引用 A，A 是"正常编译"的类；再把 A._construct_compiled 置 None
    # 模拟 A 为延迟桩后 B.parse 的行为（验证错误形态，非用户面正例）。
    @dataclass
    class A(StructMixin):
        x: int = field(Int8ub)

    @dataclass
    class B(StructMixin):
        a: A = field(A)

    A._construct_compiled = None  # 模拟延迟桩
    try:
        B.parse(b"\x01")
    finally:
        pass


if __name__ == "__main__":
    run("RepeatError build no satisfy", repeat_error_build)
    run("RepeatUntil EOF no sentinel", repeat_until_eof_no_sentinel)
    run("RangeError build count mismatch", range_error_build_count_mismatch)
    run("Array negative count", range_error_negative_count)
    run("IntegerError out of range", integer_error_out_of_range)
    run("IntegerError wrong type", integer_error_wrong_type)
    run("PaddingError bit pattern", padding_error_bit_pattern)
    run("PaddingError byte pattern 256", padding_error_byte_pattern_range)
    run("NamedTuple inner not struct", namedtuple_error_inner_not_struct)
    run("TimestampError bad unit", timestamp_error_bad_unit)
    run("TimestampError bad epoch", timestamp_error_bad_epoch)
    run("RotationError misaligned", rotation_error_misaligned)
    run("RotationError group zero", rotation_error_group_zero)
    run("Generic from user adapter", generic_error_from_user_adapter)
    run("ExplicitError via Select", explicit_error_via_select)
    run("StopField escape from Array", stop_field_escape_from_array)
    run("IndexField accessibility", index_field_accessibility)
    run("sizeof API availability", sizeof_api_availability)
    run("UnresolvedReference attempt", unresolved_ref_attempt)
