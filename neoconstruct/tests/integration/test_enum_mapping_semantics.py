"""枚举与映射构造器的用户面行为测试：Enum / FlagsEnum / Mapping。

被测语义：三类整数映射构造器嵌入 field() 后的 parse 值类型 / build 取值面
（label、int、dict）/ round-trip 对称 / 未知值错误路径（MappingError 消息
语义词）/ 实例化值语义（Instance 必填）。

期望值来源：SKILL 构造器文档契约（文档即契约）+ 语义自然性（parse/build
对称、未知值显式报错优于静默错值）+ 既有绿灯行为基线。

核心语义约定（本文件锁定）：

- Enum parse 命中 label → EnumIntegerString（str 子类，int(obj)==原值、
  ==label 成立）；未命中 → EnumInteger（int 子类 fallback，不报错）。
- Enum build：label 查映射；int 直接透传（含未知 int）；未知 label →
  MappingError。
- FlagsEnum parse → dict（每 flag 一 bool，全键形态，**无内部标记键**——
  用户值不含下划线内部键，与 build 接受部分键 dict 形态对称）；
  build 接受部分键 dict / int / "A|B" 竖线串；未知 label → MappingError。
- Mapping parse/build 无映射 → MappingError（与 Enum 的 int fallback 是
  文档化差异）；映射对象为任意对象（同一性保持）。
- 三者均为 Instance 值来源：field() 无参实例化 → TypeError（必填）。
"""

import pytest

from dataclasses import dataclass
from typing import Any

from neoconstruct import (
    Byte,
    Enum,
    FlagsEnum,
    Mapping,
    MappingError,
    StructMixin,
    field,
)


# ---------------------------------------------------------------------------
# Enum：五分支判定表（命中 label / 未命中 fallback / build label / build int
# / build 未知 label）
# ---------------------------------------------------------------------------


class TestEnumSemantics:
    """Enum(Int8ub, READ=1, WRITE=2) × field()：判定表五分支 + 值语义。"""

    def test_enum_parse_hit_returns_label_string(self):
        """parse 命中：b'\\x01' → "READ"（EnumIntegerString，int()==1）。[文档]"""

        @dataclass
        class P(StructMixin):
            op: Any = field(Enum(Byte, READ=1, WRITE=2))

        parsed = P.parse(b"\x01")
        assert str(parsed.op) == "READ"
        assert parsed.op == "READ"
        assert int(parsed.op) == 1
        assert parsed.op.intvalue == 1

    def test_enum_parse_unknown_value_falls_back_to_int(self):
        """parse 未命中：b'\\xff' → int 255 fallback（不报错）。[文档]"""

        @dataclass
        class P(StructMixin):
            op: Any = field(Enum(Byte, READ=1, WRITE=2))

        parsed = P.parse(b"\xff")
        assert int(parsed.op) == 255
        assert isinstance(parsed.op, int)

    def test_enum_parse_fallback_roundtrip_preserves_bytes(self):
        """round-trip：fallback 值 build 回原字节（int 语义保持）。[自然]"""

        @dataclass
        class P(StructMixin):
            op: Any = field(Enum(Byte, READ=1, WRITE=2))

        parsed = P.parse(b"\xff")
        assert parsed.build() == b"\xff"

    def test_enum_build_from_label(self):
        """build label：op="READ" → b'\\x01'。[文档]"""

        @dataclass
        class P(StructMixin):
            op: Any = field(Enum(Byte, READ=1, WRITE=2))

        assert P(op="READ").build() == b"\x01"
        assert P(op="WRITE").build() == b"\x02"

    def test_enum_build_from_mapped_int(self):
        """build 已映射 int：op=2 → b'\\x02'（int 直接用）。[文档]"""

        @dataclass
        class P(StructMixin):
            op: Any = field(Enum(Byte, READ=1, WRITE=2))

        assert P(op=2).build() == b"\x02"

    def test_enum_build_unknown_int_passes_through(self):
        """build 未知 int：op=99 → b'\\x63'（int 透传，不报错）。[文档]"""

        @dataclass
        class P(StructMixin):
            op: Any = field(Enum(Byte, READ=1, WRITE=2))

        assert P(op=99).build() == b"\x63"

    def test_enum_build_unknown_label_raises_mapping_error(self):
        """build 未知 label：MappingError + 消息含 "no mapping"。[自然]"""

        @dataclass
        class P(StructMixin):
            op: Any = field(Enum(Byte, READ=1, WRITE=2))

        with pytest.raises(MappingError, match="no mapping"):
            P(op="NOPE").build()

    def test_enum_roundtrip_hit_label(self):
        """round-trip：命中 label parse → build 字节不变。[自然]"""

        @dataclass
        class P(StructMixin):
            op: Any = field(Enum(Byte, READ=1, WRITE=2))

        assert P.parse(b"\x02").build() == b"\x02"

    def test_enum_field_requires_explicit_value_on_instantiation(self):
        """实例化：field(Enum(...)) 必填（Instance 值来源 → TypeError）。[基线]"""

        @dataclass
        class P(StructMixin):
            op: Any = field(Enum(Byte, READ=1, WRITE=2))

        with pytest.raises(TypeError):
            P()


# ---------------------------------------------------------------------------
# FlagsEnum：全键 dict 形态 / 部分键 build / int 与竖线串 / 未知 label
# ---------------------------------------------------------------------------


class TestFlagsEnumSemantics:
    """FlagsEnum(Byte, READ=1, WRITE=2, EXEC=4) × field()。"""

    def test_flagsenum_parse_multi_flags_full_dict(self):
        """parse b'\\x03' → 全键 dict（READ/WRITE=True, EXEC=False），无内部标记键。[文档]

        parse 产出的用户值不携带任何下划线内部键（内核实现细节不泄漏到值）；
        ``parsed.flags == {"READ": True}`` 类比较与 ``dict(**flags)`` 展开
        因此可用。双重断言：值逐键相等；无 "_flagsenum" 键。
        """

        @dataclass
        class F(StructMixin):
            flags: Any = field(FlagsEnum(Byte, READ=1, WRITE=2, EXEC=4))

        parsed = F.parse(b"\x03")
        assert parsed.flags == {
            "READ": True,
            "WRITE": True,
            "EXEC": False,
        }
        assert "_flagsenum" not in parsed.flags
        assert not [k for k in parsed.flags if k.startswith("_")]

    def test_flagsenum_parse_zero_all_false(self):
        """parse b'\\x00' → 全 False 边界。[自然]"""

        @dataclass
        class F(StructMixin):
            flags: Any = field(FlagsEnum(Byte, READ=1, WRITE=2, EXEC=4))

        parsed = F.parse(b"\x00")
        assert parsed.flags["READ"] is False
        assert parsed.flags["WRITE"] is False
        assert parsed.flags["EXEC"] is False

    def test_flagsenum_build_partial_dict(self):
        """build 部分键 dict：dict(READ=True) → b'\\x01'（缺省键为 False）。[文档]"""

        @dataclass
        class F(StructMixin):
            flags: Any = field(FlagsEnum(Byte, READ=1, WRITE=2, EXEC=4))

        assert F(flags=dict(READ=True)).build() == b"\x01"
        assert F(flags=dict(READ=True, WRITE=True)).build() == b"\x03"

    def test_flagsenum_build_from_int_and_pipe_string(self):
        """build int=3 / "READ|WRITE" → b'\\x03'（两种便捷形态）。[文档]"""

        @dataclass
        class F(StructMixin):
            flags: Any = field(FlagsEnum(Byte, READ=1, WRITE=2, EXEC=4))

        assert F(flags=3).build() == b"\x03"
        assert F(flags="READ|WRITE").build() == b"\x03"

    def test_flagsenum_build_unknown_label_raises_mapping_error(self):
        """build 未知 label：MappingError + 消息含 "unknown label"。[自然]"""

        @dataclass
        class F(StructMixin):
            flags: Any = field(FlagsEnum(Byte, READ=1, WRITE=2, EXEC=4))

        with pytest.raises(MappingError, match="unknown label"):
            F(flags=dict(NOPE=True)).build()

    def test_flagsenum_roundtrip(self):
        """round-trip：parse b'\\x05' → build 回 b'\\x05'。[自然]"""

        @dataclass
        class F(StructMixin):
            flags: Any = field(FlagsEnum(Byte, READ=1, WRITE=2, EXEC=4))

        assert F.parse(b"\x05").build() == b"\x05"

    def test_flagsenum_field_requires_explicit_value(self):
        """实例化：必填（Instance 值来源 → TypeError）。[基线]"""

        @dataclass
        class F(StructMixin):
            flags: Any = field(FlagsEnum(Byte, READ=1, WRITE=2, EXEC=4))

        with pytest.raises(TypeError):
            F()


# ---------------------------------------------------------------------------
# Mapping：通用对象映射（无映射报错——与 Enum fallback 的文档化差异）
# ---------------------------------------------------------------------------


class TestMappingSemantics:
    """Mapping(Byte, {0: obj0, 1: obj1}) × field()：对象同一性保持 + 双向报错。"""

    def test_mapping_parse_known_returns_mapped_object_identity(self):
        """parse 命中：返回映射对象本体（同一性 is 保持）。[文档]"""

        obj0, obj1 = object(), object()

        @dataclass
        class M(StructMixin):
            v: Any = field(Mapping(Byte, {0: obj0, 1: obj1}))

        assert M.parse(b"\x00").v is obj0
        assert M.parse(b"\x01").v is obj1

    def test_mapping_parse_unknown_raises_mapping_error(self):
        """parse 未命中：MappingError + 消息含 "no decoding mapping"（无 fallback，
        与 Enum 的差异是文档化行为）。[文档]"""

        @dataclass
        class M(StructMixin):
            v: Any = field(Mapping(Byte, {0: "ZERO", 1: "ONE"}))

        with pytest.raises(MappingError, match="no decoding mapping"):
            M.parse(b"\xff")

    def test_mapping_build_from_mapped_object(self):
        """build：v="ONE" → b'\\x01'（反向映射）。[文档]"""

        @dataclass
        class M(StructMixin):
            v: Any = field(Mapping(Byte, {0: "ZERO", 1: "ONE"}))

        assert M(v="ZERO").build() == b"\x00"
        assert M(v="ONE").build() == b"\x01"

    def test_mapping_build_unknown_object_raises_mapping_error(self):
        """build 未映射对象：MappingError + 消息含 "no encoding mapping"。[自然]"""

        @dataclass
        class M(StructMixin):
            v: Any = field(Mapping(Byte, {0: "ZERO", 1: "ONE"}))

        with pytest.raises(MappingError, match="no encoding mapping"):
            M(v="TWO").build()

    def test_mapping_roundtrip(self):
        """round-trip：parse → build 字节不变。[自然]"""

        @dataclass
        class M(StructMixin):
            v: Any = field(Mapping(Byte, {0: "ZERO", 1: "ONE"}))

        assert M.parse(b"\x01").build() == b"\x01"

    def test_mapping_field_requires_explicit_value(self):
        """实例化：必填（Instance 值来源 → TypeError）。[基线]"""

        @dataclass
        class M(StructMixin):
            v: Any = field(Mapping(Byte, {0: "ZERO", 1: "ONE"}))

        with pytest.raises(TypeError):
            M()
