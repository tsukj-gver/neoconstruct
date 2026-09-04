"""探针 D：深位置错误 path 质量 + 比较链 0/1 + RepeatUntil 组合面。

- Array 元素级 / GreedyRange 元素级 / Switch case 级 / Prefixed 子流级 /
  嵌套 Struct×3 / BitStruct 位域级错误 path
- Computed 比较链返回 0/1 型
- Prefixed 子流内 RepeatUntil
"""

import sys

sys.path.insert(0, r"D:\Project\Github\neoconstruct\neoconstruct\python")

from dataclasses import dataclass  # noqa: E402
from typing import Any  # noqa: E402

import neoconstruct as nc  # noqa: E402
from neoconstruct import (  # noqa: E402
    BitStructMixin,
    BitsInteger,
    Bytes,
    Element,
    GreedyRange,
    Int16ub,
    Int8ub,
    Prefixed,
    RepeatUntil,
    StructMixin,
    Switch,
    field,
    rfield,
)


def run(label, fn):
    print(f"===== {label} =====")
    try:
        result = fn()
        print(f"  [NO-RAISE] result = {result!r}")
    except Exception as e:
        print(f"  type = {type(e).__name__}  path = {getattr(e, 'path', '<no attr>')!r}")
        print(f"  msg  = {str(e)[:150]!r}")
    print()


def array_elem_path():
    @dataclass
    class Inner(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)

    @dataclass
    class P(StructMixin):
        items: list = field(nc.Array(3, Inner))

    P.parse(bytes([1, 2, 3, 4, 5]))  # 第 3 元素缺 b


def greedy_elem_path():
    @dataclass
    class Inner(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)

    @dataclass
    class P(StructMixin):
        items: list = field(GreedyRange(Inner))

    P.parse(bytes([1, 2, 3]))  # 第 2 元素缺 b


def switch_case_path():
    @dataclass
    class Inner(StructMixin):
        v: int = field(Int16ub)

    @dataclass
    class P(StructMixin):
        t: int = field(Int8ub)
        body: Any = field(Switch(t, {1: Inner, 2: Bytes(2)}))

    P.parse(bytes([1, 0x01]))  # case 1 → Inner.v 缺字节


def prefixed_substream_path():
    @dataclass
    class Inner(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)

    @dataclass
    class P(StructMixin):
        p: Any = field(Prefixed(Int8ub, Inner))

    P.parse(bytes([3, 1]))  # 子流声明 3 字节，实际子流内容不足


def nested3_path():
    @dataclass
    class C(StructMixin):
        d: int = field(Int8ub)

    @dataclass
    class B(StructMixin):
        c: C = field(C)

    @dataclass
    class A(StructMixin):
        b: B = field(B)

    A.parse(b"")  # 最内层缺字节


def bitstruct_field_path():
    @dataclass
    class B(BitStructMixin):
        x: int = field(BitsInteger(4))
        y: int = field(BitsInteger(4))

    B.build(B(x=1))  # y 缺值 → FieldValueMissing path?


def comparison_chain():
    @dataclass
    class P(StructMixin):
        x: int = field(Int8ub)
        c: int = rfield(nc.Computed((x > 3) + 10))

    return P.parse(b"\x05").c, P.parse(b"\x01").c


def repeat_until_in_prefixed():
    @dataclass
    class P(StructMixin):
        n: int = field(Int8ub)
        e: int = rfield(Element())
        items: list = field(nc.Prefixed(n, RepeatUntil(e > 5, Int8ub), includelength=False))

    # Prefixed(Int8ub, RepeatUntil)——直接内嵌
    @dataclass
    class Q(StructMixin):
        payload: Any = field(Prefixed(Int8ub, _RU()))

    return "见下方直接形态"


def _RU():
    @dataclass
    class RU(StructMixin):
        e: int = rfield(Element())
        items: list = field(RepeatUntil(e > 5, Int8ub))

    return RU


def prefixed_ru_direct():
    RU = _RU()

    @dataclass
    class P(StructMixin):
        payload: Any = field(Prefixed(Int8ub, RU))

    pkt = P.parse(bytes([3, 1, 6, 9]))
    return pkt.payload.items


if __name__ == "__main__":
    run("Array 元素级 path", array_elem_path)
    run("GreedyRange 元素级 path", greedy_elem_path)
    run("Switch case 级 path", switch_case_path)
    run("Prefixed 子流级 path", prefixed_substream_path)
    run("嵌套 Struct×3 path", nested3_path)
    run("BitStruct 字段缺值 path", bitstruct_field_path)
    run("比较链 (x>3)+10", comparison_chain)
    run("Prefixed 内 RepeatUntil", prefixed_ru_direct)
