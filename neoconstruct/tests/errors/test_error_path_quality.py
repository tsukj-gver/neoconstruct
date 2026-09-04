"""错误 path 定位质量锁定：深嵌套位置的错误路径形态。

覆盖：Array 元素级 / GreedyRange 元素级（build 侧）/ Switch case 级 /
Prefixed 子流级 / 嵌套 Struct×3 / BitStruct 位域级 / RepeatUntil 元素级。
path 形如 ``root.items[2].b`` ——字段名链 + 元素索引。

期望值来源标注：[文档]=错误携带 path 的 API 承诺；[基线]=实测绿灯行为锚定。
"""

import pytest
from dataclasses import dataclass
from typing import Any

from neoconstruct import (
    Array,
    BitStructMixin,
    BitsInteger,
    Bytes,
    Element,
    FormatFieldError,
    GreedyRange,
    Int16ub,
    Int8ub,
    Prefixed,
    RepeatUntil,
    StreamError,
    StructMixin,
    Switch,
    field,
    rfield,
)


def test_array_element_level_path_on_parse():
    """Array(3, Inner{a,b}) 第 3 元素缺 b → path == root.items[2].b。[基线]"""

    @dataclass
    class Inner(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)

    @dataclass
    class P(StructMixin):
        items: list = field(Array(3, Inner))

    with pytest.raises(StreamError) as exc_info:
        P.parse(bytes([1, 2, 3, 4, 5]))

    assert exc_info.value.path == "root.items[2].b"


def test_array_element_level_path_on_build():
    """Array(3, Int8ub) build 第 2 元素超范围 → path == root.items[1]。[基线]"""

    @dataclass
    class P(StructMixin):
        items: list = field(Array(3, Int8ub))

    with pytest.raises(FormatFieldError) as exc_info:
        P(items=[1, 300, 3]).build()

    assert exc_info.value.path == "root.items[1]"


def test_greedy_range_element_level_path_on_build():
    """GreedyRange(Int8ub) build 第 2 元素超范围 → path == root.items[1]。[基线]"""

    @dataclass
    class P(StructMixin):
        items: list = field(GreedyRange(Int8ub))

    with pytest.raises(FormatFieldError) as exc_info:
        P(items=[1, 300, 3]).build()

    assert exc_info.value.path == "root.items[1]"


def test_switch_case_field_level_path_on_parse():
    """Switch 命中 case 内 Struct 字段错误 → path 含 case 字段链 root.body.v。[基线]"""

    @dataclass
    class Inner(StructMixin):
        v: int = field(Int16ub)

    @dataclass
    class P(StructMixin):
        t: int = field(Int8ub)
        body: Any = field(Switch(t, {1: Inner, 2: Bytes(2)}))

    with pytest.raises(StreamError) as exc_info:
        P.parse(bytes([1, 0x01]))

    assert exc_info.value.path == "root.body.v"


def test_prefixed_substream_level_path_on_parse():
    """Prefixed 子流内容不足 → path 定位到子流字段 root.p。[基线]"""

    @dataclass
    class Inner(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)

    @dataclass
    class P(StructMixin):
        p: Any = field(Prefixed(Int8ub, Inner))

    with pytest.raises(StreamError) as exc_info:
        P.parse(bytes([3, 1]))

    assert exc_info.value.path == "root.p"


def test_three_level_nested_struct_path_on_parse():
    """A→B→C 三层嵌套最内层缺字节 → path == root.b.c.d。[基线]"""

    @dataclass
    class C(StructMixin):
        d: int = field(Int8ub)

    @dataclass
    class B(StructMixin):
        c: C = field(C)

    @dataclass
    class A(StructMixin):
        b: B = field(B)

    with pytest.raises(StreamError) as exc_info:
        A.parse(b"")

    assert exc_info.value.path == "root.b.c.d"


def test_bitstruct_field_path_on_build_missing_value():
    """BitStruct build 缺字段值 → 消息含字段级位置 root.y。[基线]

    FieldValueMissingError 的 path 属性为 None（Display 全文内嵌路径），
    断言 str(e) 含 "root.y"。
    """

    @dataclass
    class B(BitStructMixin):
        x: int = field(BitsInteger(4))
        y: int = field(BitsInteger(4))

    with pytest.raises(Exception) as exc_info:
        B.build(B(x=1))

    assert "root.y" in str(exc_info.value)


def test_repeat_until_element_level_path_on_eof():
    """RepeatUntil EOF 失败 → path 含元素索引 root.items[3]。[基线]"""

    @dataclass
    class P(StructMixin):
        e: int = rfield(Element())
        items: list = field(RepeatUntil(e > 200, Int8ub))

    with pytest.raises(StreamError) as exc_info:
        P.parse(bytes([1, 2, 3]))

    assert exc_info.value.path == "root.items[3]"
