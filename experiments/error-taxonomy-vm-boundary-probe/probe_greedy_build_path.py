"""探针 E：GreedyRange / Array 元素级 build 侧错误 path。"""

import sys

sys.path.insert(0, r"D:\Project\Github\neoconstruct\neoconstruct\python")

from dataclasses import dataclass  # noqa: E402

from neoconstruct import Array, GreedyRange, Int8ub, StructMixin, field  # noqa: E402


def run(label, fn):
    print(f"===== {label} =====")
    try:
        fn()
        print("  [NO-RAISE]")
    except Exception as e:
        print(f"  type = {type(e).__name__}  path = {getattr(e, 'path', '<no attr>')!r}")
        print(f"  msg  = {str(e)[:150]!r}")
    print()


def greedy_build_elem_path():
    @dataclass
    class P(StructMixin):
        items: list = field(GreedyRange(Int8ub))

    P(items=[1, 300, 3]).build()  # 第 2 元素超范围


def array_build_elem_path():
    @dataclass
    class P(StructMixin):
        items: list = field(Array(3, Int8ub))

    P(items=[1, 300, 3]).build()


def greedy_parse_validation_elem_path():
    from typing import Any
    from neoconstruct import OneOf, rfield

    @dataclass
    class Inner(StructMixin):
        v: int = field(OneOf(Int8ub, [1, 2]))

    @dataclass
    class P(StructMixin):
        items: list = field(GreedyRange(Inner))

    P.parse(bytes([1, 9]))  # 第 2 元素校验失败 → 元素级 path


if __name__ == "__main__":
    run("GreedyRange build 元素超范围", greedy_build_elem_path)
    run("Array build 元素超范围", array_build_elem_path)
    run("GreedyRange parse 元素校验失败", greedy_parse_validation_elem_path)
