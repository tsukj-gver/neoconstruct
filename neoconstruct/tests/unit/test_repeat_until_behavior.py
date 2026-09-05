"""RepeatUntil 用户面行为锁定。

覆盖：
- callable 终止条件在描述符创建时被拒绝（编译期 ``CompilationError``）
- 终止表达式路径 parse/build 对称：Element 引用 / 跨字段哨兵 / 负数哨兵
- 首元素即满足哨兵 → 立即停止（单元素 list）
- 全流无哨兵到 EOF → ``StreamError``（元素级 path；哨兵是组帧契约，
  EOF 无哨兵即报错——[文档] RepeatUntil docstring 契约）
- build 无元素满足终止条件 → ``RepeatError``
- Prefixed 子流内 RepeatUntil（子流边界与哨兵交互）

期望值来源标注：[文档]=RepeatUntil/Element docstring 契约；
[自然]=语义自然性（parse/build 对称、最小惊讶）；[基线]=既有绿灯行为锚定。
"""

import pytest
from dataclasses import dataclass
from typing import Any

from neoconstruct import (
    CompilationError,
    Element,
    Int8sb,
    Int8ub,
    Prefixed,
    RepeatError,
    RepeatUntil,
    RepeatUntilDescriptor,
    StreamError,
    StructMixin,
    field,
    rfield,
)


# ---------------------------------------------------------------------------
# callable 拒绝（终止条件必须是字段表达式）
# ---------------------------------------------------------------------------


def test_callable_terminator_rejected_at_descriptor_creation():
    """Python callable 终止条件 → 描述符创建即抛 CompilationError。

    [文档] RepeatUntil docstring：终止条件必须是字段表达式（如 ``e > 5``），
    callable 不进入编译管线。错误消息应指引正确用法（Element 字段）。
    """

    @dataclass
    class P(StructMixin):
        e: int = rfield(Element())
        items: list = field(RepeatUntil(e > 5, Int8ub))  # 合法形态旁证

    with pytest.raises(CompilationError) as exc_info:
        RepeatUntilDescriptor(lambda x, lst, ctx: x > 5, Int8ub)

    msg = str(exc_info.value).lower()
    assert "callable" in msg or "field expression" in msg


def test_callable_function_terminator_rejected():
    """普通 Python 函数终止条件同样被拒绝（非 lambda 一视同仁）。[文档]"""

    def predicate(x, lst, ctx):
        return x > 5

    with pytest.raises(CompilationError):
        RepeatUntilDescriptor(predicate, Int8ub)


# ---------------------------------------------------------------------------
# 终止表达式路径：parse / build 对称
# ---------------------------------------------------------------------------


def test_element_expression_parse_stops_at_first_satisfying():
    """``RepeatUntil(e > 5, Int8ub)``：parse 在首个满足 e>5 的元素处停止。

    [文档] Element 引用当前元素；[自然] 终止元素包含在结果内。
    双重断言：items 逐值；Element 字段实例值恒为 None（借用值不落实例）。
    """

    @dataclass
    class P(StructMixin):
        e: int = rfield(Element())
        items: list = field(RepeatUntil(e > 5, Int8ub))

    pkt = P.parse(bytes([1, 2, 3, 4, 5, 6, 7, 8]))
    assert pkt.items == [1, 2, 3, 4, 5, 6]
    assert pkt.e is None


def test_element_expression_build_stops_at_first_satisfying():
    """build 遍历列表，写出至首个满足终止条件的元素后停止。[自然]

    双重断言：build 字节；parse(build(v)) 还原 v。
    """

    @dataclass
    class P(StructMixin):
        e: int = rfield(Element())
        items: list = field(RepeatUntil(e > 5, Int8ub))

    built = P.build(P(items=[1, 2, 3, 4, 5, 6]))
    assert built == bytes([1, 2, 3, 4, 5, 6])
    assert P.parse(built).items == [1, 2, 3, 4, 5, 6]


def test_negative_sentinel_parse_and_build():
    """负数哨兵（``e == -1``，Int8sb）：0xFF 解析为 -1 并在该处停止。[自然]

    双重断言：parse 值（含终止元素 -1）；build 字节 b"\\x01\\x02\\x03\\xff"。
    """

    @dataclass
    class P(StructMixin):
        e: int = rfield(Element())
        items: list = field(RepeatUntil(e == -1, Int8sb))

    pkt = P.parse(bytes([1, 2, 3, 0xFF, 9]))
    assert pkt.items == [1, 2, 3, -1]
    built = P.build(P(items=[1, 2, 3, -1]))
    assert built == bytes([1, 2, 3, 0xFF])
    assert P.parse(built).items == [1, 2, 3, -1]


def test_sentinel_from_prior_field_parse_and_build():
    """哨兵来自前序字段（``e == stop``）：终止条件跨字段引用。[自然]

    双重断言：parse 两字段值；build 字节；roundtrip。
    """

    @dataclass
    class P(StructMixin):
        stop: int = field(Int8ub)
        e: int = rfield(Element())
        items: list = field(RepeatUntil(e == stop, Int8ub))

    pkt = P.parse(bytes([5, 1, 2, 5, 9]))
    assert pkt.stop == 5
    assert pkt.items == [1, 2, 5]

    built = P.build(P(stop=5, items=[1, 2, 5]))
    assert built == bytes([5, 1, 2, 5])
    reparsed = P.parse(built)
    assert reparsed.stop == 5 and reparsed.items == [1, 2, 5]


def test_first_element_satisfies_terminator_stops_immediately():
    """首元素即满足哨兵 → 单元素 list，不消耗后续字节。[自然]

    双重断言：parse items == [1]；build([1]) == b"\\x01"。
    """

    @dataclass
    class P(StructMixin):
        e: int = rfield(Element())
        items: list = field(RepeatUntil(e > 0, Int8ub))

    pkt = P.parse(bytes([1, 9, 9]))
    assert pkt.items == [1]
    assert P.build(P(items=[1])) == b"\x01"


# ---------------------------------------------------------------------------
# 错误路径
# ---------------------------------------------------------------------------


def test_eof_without_sentinel_raises_stream_error_with_element_path():
    """全流无哨兵至 EOF → StreamError，path 定位到首个失败元素索引。[文档]

    EOF 行为契约：RepeatUntil 的哨兵是组帧契约（build 侧同样强制"某元素
    必须满足"），整流读尽仍未出现满足元素即报错——"读到 EOF 停"是
    GreedyRange 的语义，两者不可混。双重断言：类型 StreamError；
    path == root.items[3]（元素级定位）。
    """

    @dataclass
    class P(StructMixin):
        e: int = rfield(Element())
        items: list = field(RepeatUntil(e > 200, Int8ub))

    with pytest.raises(StreamError) as exc_info:
        P.parse(bytes([1, 2, 3]))

    assert exc_info.value.path == "root.items[3]"


def test_build_without_satisfying_element_raises_repeat_error():
    """build 遍历完列表无元素满足终止条件 → RepeatError。[文档]

    双重断言：异常类型；path 含字段名 items；消息含 "terminator" 语义词。
    """

    @dataclass
    class P(StructMixin):
        e: int = rfield(Element())
        items: list = field(RepeatUntil(e > 5, Int8ub))

    with pytest.raises(RepeatError) as exc_info:
        P(items=[1, 2, 3]).build()

    assert "items" in (exc_info.value.path or "")
    assert "terminator" in str(exc_info.value)


# ---------------------------------------------------------------------------
# 组合面
# ---------------------------------------------------------------------------


def test_repeat_until_inside_prefixed_substream():
    """Prefixed 子流内 RepeatUntil：哨兵在子流内判定，主流不受影响。[自然]

    双重断言：payload.items 值；roundtrip 字节一致。
    """

    @dataclass
    class RU(StructMixin):
        e: int = rfield(Element())
        items: list = field(RepeatUntil(e > 5, Int8ub))

    @dataclass
    class P(StructMixin):
        payload: Any = field(Prefixed(Int8ub, RU))

    pkt = P.parse(bytes([2, 1, 6]))
    assert pkt.payload.items == [1, 6]

    built = P.build(P(payload=RU(items=[1, 6])))
    assert built == bytes([2, 1, 6])
    assert P.parse(built).payload.items == [1, 6]
