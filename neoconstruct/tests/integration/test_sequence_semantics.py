"""位置序容器构造器的用户面行为测试：Sequence / NamedTuple。

被测语义：两类位置序容器嵌入 field() 后的 parse 产出形态（list /
namedtuple）、build 位置访问、命名 subcon、内嵌值语义构造器的元素级
resolve、数据不足错误路径、NamedTuple 两模式（Struct 按名 / Sequence 按
位）与 NamedTupleError、实例化值语义。

期望值来源：SKILL 构造器文档契约 + 语义自然性（位置序对称）+ 既有绿灯
行为基线。

核心语义约定（本文件锁定）：

- Sequence parse → list（按 subcon 顺序位置排列）；build 从 list 按位置
  取值；命名 subcon 仅作 context 名字（产出仍按位置）。
- Sequence 内嵌值语义构造器（如 Default）：build 时 list 缺元素由内层
  默认值补齐（元素级 resolve）；parse 时内层仍按数据读取（数据不足 →
  StreamError）。
- Sequence 字段实例化可省（None，Optional）。
- NamedTuple(name, fields, Struct 子类) parse → namedtuple（属性访问），
  build 直接接受 namedtuple 实例；
- NamedTuple over Sequence parse → factory(*list)；build 接受可迭代
  （namedtuple/tuple/list），不可迭代 → NamedTupleError。
- NamedTuple 字段实例化必填（Instance）。
"""

import pytest

from dataclasses import dataclass
from typing import Any

from neoconstruct import (
    Byte,
    Bytes,
    Default,
    GreedyBytes,
    Int8ub,
    NamedTuple,
    NamedTupleError,
    Sequence,
    StreamError,
    StructMixin,
    field,
)


class TestSequenceSemantics:
    """Sequence(Byte, Bytes(2), GreedyBytes) × field()：位置序 list。"""

    def test_sequence_parse_returns_positional_list(self):
        """parse：b'\\x01XYtail' → [1, b'XY', b'tail']（位置序值）。[文档]"""

        @dataclass
        class S(StructMixin):
            items: Any = field(Sequence(Byte, Bytes(2), GreedyBytes))

        parsed = S.parse(b"\x01XYtail")
        assert parsed.items == [1, b"XY", b"tail"]
        assert isinstance(parsed.items, list)

    def test_sequence_build_from_positional_list(self):
        """build：[1, b'XY', b'tail'] → b'\\x01XYtail'。[文档]"""

        @dataclass
        class S(StructMixin):
            items: Any = field(Sequence(Byte, Bytes(2), GreedyBytes))

        assert S(items=[1, b"XY", b"tail"]).build() == b"\x01XYtail"

    def test_sequence_roundtrip(self):
        """round-trip：parse → build 字节不变。[自然]"""

        @dataclass
        class S(StructMixin):
            items: Any = field(Sequence(Byte, Bytes(2), GreedyBytes))

        assert S.parse(b"\x01XYtail").build() == b"\x01XYtail"

    def test_sequence_named_subcons_still_positional(self):
        """命名 subcon：产出仍按位置（名字仅入 context）。[文档]"""

        @dataclass
        class SN(StructMixin):
            seq: Any = field(Sequence(count=Byte, data=GreedyBytes))

        parsed = SN.parse(b"\x03ABC")
        assert parsed.seq == [3, b"ABC"]
        assert SN(seq=[3, b"ABC"]).build() == b"\x03ABC"

    def test_sequence_field_instantiation_follows_composition(self):
        """实例化：随子树构成——含变长/值提供内层（GreedyBytes/Default）可省
        （None）；全定长原子内层必填（TypeError）。[基线]"""

        @dataclass
        class SV(StructMixin):
            items: Any = field(Sequence(Byte, Bytes(2), GreedyBytes))

        @dataclass
        class SD(StructMixin):
            items: Any = field(Sequence(Int8ub, Default(Int8ub, 0)))

        @dataclass
        class SF(StructMixin):
            items: Any = field(Sequence(Byte, Byte))

        assert SV().items is None
        assert SD().items is None
        with pytest.raises(TypeError):
            SF()

    def test_sequence_inner_default_fills_missing_element_on_build(self):
        """build 元素级值语义：list 缺元素由内层 Default 补默认值。[自然+基线]"""

        @dataclass
        class SV(StructMixin):
            seq: Any = field(Sequence(Int8ub, Default(Int8ub, 0)))

        # list 只给 1 个元素，第二个由 Default(Byte, 0) 补 0
        assert SV(seq=[7]).build() == b"\x07\x00"

    def test_sequence_parse_short_data_raises_stream_error(self):
        """parse 数据不足：显式 StreamError（内层 Default 不豁免 parse 读数据）。[自然]"""

        @dataclass
        class SV(StructMixin):
            seq: Any = field(Sequence(Int8ub, Default(Int8ub, 0)))

        with pytest.raises(StreamError):
            SV.parse(b"\x07")


class TestNamedTupleSemantics:
    """NamedTuple × field()：Struct 按名模式 / Sequence 按位模式。"""

    def test_namedtuple_over_struct_parse_returns_namedtuple(self):
        """Struct 模式 parse：b'\\x01\\x02' → coord(x=1, y=2)（属性访问）。[文档]"""

        @dataclass
        class Coord(StructMixin):
            x: int = field(Int8ub)
            y: int = field(Int8ub)

        @dataclass
        class NP(StructMixin):
            coord: Any = field(NamedTuple("coord", "x y", Coord))

        parsed = NP.parse(b"\x01\x02")
        assert parsed.coord.x == 1
        assert parsed.coord.y == 2
        assert tuple(parsed.coord) == (1, 2)

    def test_namedtuple_over_struct_build_roundtrip(self):
        """Struct 模式 build：namedtuple 实例直接可用；round-trip 稳定。[文档]"""

        @dataclass
        class Coord(StructMixin):
            x: int = field(Int8ub)
            y: int = field(Int8ub)

        @dataclass
        class NP(StructMixin):
            coord: Any = field(NamedTuple("coord", "x y", Coord))

        parsed = NP.parse(b"\x05\x06")
        assert NP(coord=parsed.coord).build() == b"\x05\x06"
        assert parsed.build() == b"\x05\x06"

    def test_namedtuple_over_sequence_parse_returns_namedtuple(self):
        """Sequence 模式 parse：按位置解包 → pair(a=10, b=11)。[文档]"""

        @dataclass
        class NS(StructMixin):
            pair: Any = field(NamedTuple("pair", "a b", Sequence(Int8ub, Int8ub)))

        parsed = NS.parse(b"\x0a\x0b")
        assert parsed.pair.a == 10
        assert parsed.pair.b == 11
        assert tuple(parsed.pair) == (10, 11)

    def test_namedtuple_over_sequence_build_roundtrip(self):
        """Sequence 模式 build/round-trip：namedtuple → list 按位写出。[自然]"""

        @dataclass
        class NS(StructMixin):
            pair: Any = field(NamedTuple("pair", "a b", Sequence(Int8ub, Int8ub)))

        parsed = NS.parse(b"\x0a\x0b")
        assert NS(pair=parsed.pair).build() == b"\x0a\x0b"
        assert parsed.build() == b"\x0a\x0b"

    def test_namedtuple_build_non_iterable_raises_namedtuple_error(self):
        """Sequence 模式 build 不可迭代：NamedTupleError + "not iterable" 语义词。[自然]"""

        @dataclass
        class NS(StructMixin):
            pair: Any = field(NamedTuple("pair", "a b", Sequence(Int8ub, Int8ub)))

        with pytest.raises(NamedTupleError, match="not iterable"):
            NS(pair=123).build()

    def test_namedtuple_field_requires_explicit_value(self):
        """实例化：必填（Instance 值来源 → TypeError）。[基线]"""

        @dataclass
        class Coord(StructMixin):
            x: int = field(Int8ub)
            y: int = field(Int8ub)

        @dataclass
        class NP(StructMixin):
            coord: Any = field(NamedTuple("coord", "x y", Coord))

        with pytest.raises(TypeError):
            NP()
