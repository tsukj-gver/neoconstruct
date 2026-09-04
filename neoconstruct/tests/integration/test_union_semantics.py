"""Union 构造器的用户面行为测试。

被测语义：多视角联合体嵌入 field() 后的 parse 全视角 dict / parsefrom 定位
策略（None / int 索引 / name）/ build 按首个命中名分派 / 错误路径
（UnionError 两形态）/ 实例化值语义 / round-trip。

期望值来源：SKILL 构造器文档契约 + 语义自然性（多视角读同一回退点、build
单视角写出）+ 既有绿灯行为基线。

核心语义约定（本文件锁定）：

- parse 从回退点（fallback）逐命名 subcon 解析，全部视角写入 dict；
  parsefrom=None 流停在回退点（后继字段重读同位置）；parsefrom=int/name
  流前进到该 subcon 的 consumed 前沿。
- build：obj 为 dict，取**首个**在 dict 中出现的命名 subcon 写出；
  无命中名 → UnionError。
- parsefrom 越界 → UnionError（显式报错优于静默）。
- Union 字段实例化为 Optional（可省，None ≡ 缺值 → build 期
  FieldValueMissingError，值缺失时机后移与框架一致）。
"""

import pytest

from dataclasses import dataclass
from typing import Any

from neoconstruct import (
    Bytes,
    FieldValueMissingError,
    GreedyBytes,
    Int16ub,
    Int32ub,
    Int8ub,
    StructMixin,
    Union,
    UnionError,
    field,
)


class TestUnionParseViews:
    """Union parse：多视角 dict + parsefrom 流定位策略。"""

    def test_union_parsefrom_none_parses_all_views_and_stays_at_fallback(self):
        """parse(None)：dict 含全部视角；流回退点不动（后继 tail 重读起点）。[文档+自然]"""

        @dataclass
        class U(StructMixin):
            u: Any = field(Union(None, raw=Bytes(4), num=Int32ub))
            tail: bytes = field(GreedyBytes)

        parsed = U.parse(b"ABCD")
        assert parsed.u["raw"] == b"ABCD"
        assert parsed.u["num"] == 0x41424344
        # parsefrom=None：流停在回退点 → tail 从头读
        assert parsed.tail == b"ABCD"

    def test_union_parsefrom_index_seeks_forward(self):
        """parse(0)：流前进到 subcons[0] 前沿（tail 读视角 0 之后）。[文档+自然]"""

        @dataclass
        class U(StructMixin):
            u: Any = field(Union(0, raw=Bytes(2), num=Int32ub))
            tail: bytes = field(GreedyBytes)

        parsed = U.parse(b"ABCD")
        assert parsed.u["raw"] == b"AB"
        assert parsed.u["num"] == 0x41424344
        # forwards[0]=2（raw 视角消费 2 字节）→ tail 从偏移 2 读
        assert parsed.tail == b"CD"

    def test_union_parsefrom_name_resolves_view(self):
        """parse("num")：按名解析视角索引，流前进到 num 前沿。[文档]"""

        @dataclass
        class U(StructMixin):
            u: Any = field(Union("num", raw=Bytes(2), num=Int32ub))
            tail: bytes = field(GreedyBytes)

        parsed = U.parse(b"ABCD")
        # forwards[num]=4（Int32ub 消费 4 字节）→ tail 为空
        assert parsed.u["num"] == 0x41424344
        assert parsed.tail == b""


class TestUnionBuild:
    """Union build：dict 首个命中名分派 + 错误路径。"""

    def test_union_build_selects_named_view(self):
        """build：dict 含 raw → 按该视角写出字节。[自然]"""

        @dataclass
        class U(StructMixin):
            u: Any = field(Union(None, raw=Bytes(4), num=Int32ub))

        assert U(u=dict(raw=b"ABCD")).build() == b"ABCD"
        assert U(u=dict(num=0x41424344)).build() == b"ABCD"

    def test_union_build_unknown_name_raises_union_error(self):
        """build 无命中名：UnionError + "cannot build" 语义词。[自然]"""

        @dataclass
        class U(StructMixin):
            u: Any = field(Union(None, raw=Bytes(2)))

        with pytest.raises(UnionError, match="cannot build"):
            U(u=dict(other=1)).build()

    def test_union_parsefrom_out_of_range_raises_union_error(self):
        """parsefrom 越界：UnionError + "out of range" 语义词。[自然]"""

        @dataclass
        class U(StructMixin):
            u: Any = field(Union(5, raw=Bytes(2)))

        with pytest.raises(UnionError, match="out of range"):
            U.parse(b"AB")


class TestUnionValueSemantics:
    """Union 字段值语义（ValueKind 框架 D2/D3 面）。"""

    def test_union_field_instantiation_optional(self):
        """实例化：可省（u=None）——包装器字段缺省错误时机后移。[基线]"""

        @dataclass
        class U(StructMixin):
            u: Any = field(Union(None, raw=Bytes(2)))

        inst = U()
        assert inst.u is None

    def test_union_build_missing_value_raises_field_value_missing(self):
        """build 缺值：u=None ≡ 缺省 → FieldValueMissingError（值使用点报错）。[基线]"""

        @dataclass
        class U(StructMixin):
            u: Any = field(Union(None, raw=Bytes(2)))

        with pytest.raises(FieldValueMissingError):
            U(u=None).build()

    def test_union_roundtrip(self):
        """round-trip：parse → build 字节稳定。[自然]"""

        @dataclass
        class U(StructMixin):
            u: Any = field(Union(None, raw=Bytes(4), num=Int32ub))

        assert U.parse(b"ABCD").build() == b"ABCD"

    def test_union_embedded_between_fields_roundtrip(self):
        """嵌入位置：head + Union(None,...) + tail（流回退语义下 tail 从回退点
        读到 EOF——与视角字节重叠，build 逐字段重写）。[自然+基线]"""

        @dataclass
        class UN(StructMixin):
            head: int = field(Int8ub)
            u: Any = field(Union(None, raw=Bytes(2), num=Int16ub))
            tail: bytes = field(GreedyBytes)

        parsed = UN.parse(b"\x2a" + b"AB" + b"Z")
        assert parsed.head == 0x2A
        assert parsed.u["raw"] == b"AB"
        # Union(None) 流回退 → GreedyBytes 从回退点读到 EOF
        assert parsed.tail == b"ABZ"
        built = UN(head=0x2A, u=dict(raw=b"AB"), tail=b"ABZ").build()
        assert built == b"\x2a" + b"AB" + b"ABZ"
