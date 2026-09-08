"""编译期拒绝语义：StructMixin 继承拒绝与 dataclasses.field 遮蔽检测。

覆盖两类用户误用：

1. 继承已编译的结构类——编译器只收集子类自身类体的 field() 声明，
   继承会静默丢基类字段（build 产出缺字段的错误字节）。字节排布有
   位置语义，继承无法表达基类字段占位，故在 ``__init_subclass__``
   编译期显式拒绝，引导用组合（字段嵌套子 Struct）替代。
2. ``dataclasses.field`` 遮蔽误用——import 了 dataclasses 的 field 而非
   本库的 field，parse 静默忽略字节返回默认值。编译期检测两种可判定
   形态（default 包装协议描述符 / 整类无 field() 声明）。
"""

from dataclasses import dataclass, field as dc_field, make_dataclass
from typing import Any

import pytest

from neoconstruct import (
    BitStructMixin,
    CompilationError,
    Int8ub,
    Int16ub,
    Nibble,
    StructMixin,
    field,
)


# ---------------------------------------------------------------------------
# 继承拒绝：单继承 / 多层 / 已编译基类 / 动态创建
# ---------------------------------------------------------------------------


def test_single_inheritance_rejected_at_class_creation():
    """单继承已编译结构类 → 类创建即 CompilationError（含理由与引导）。"""

    @dataclass
    class BaseHeader(StructMixin):
        magic: int = field(Int16ub)

    with pytest.raises(CompilationError) as exc_info:

        @dataclass
        class ExtendedHeader(BaseHeader):
            version: int = field(Int8ub)

    msg = str(exc_info.value)
    assert "不支持继承" in msg
    assert "空间位置语义" in msg  # 一句理由
    assert "组合" in msg  # 引导


def test_empty_subclass_also_rejected():
    """空子类（无新增字段）继承同样拒绝——基类字段照丢。"""

    @dataclass
    class Base(StructMixin):
        magic: int = field(Int16ub)

    with pytest.raises(CompilationError):

        @dataclass
        class ExtEmpty(Base):
            pass


def test_multilevel_and_multiple_base_inheritance_rejected():
    """多层链在第二级即拒绝；多基类形态（Base, StructMixin）同样拒绝。"""

    @dataclass
    class Base(StructMixin):
        magic: int = field(Int16ub)

    # 第二级拒绝（第三级因此永远无法经由普通语法定义）
    with pytest.raises(CompilationError):

        @dataclass
        class Mid(Base):
            extra: int = field(Int8ub)

    # 多基类形态
    with pytest.raises(CompilationError):

        @dataclass
        class Multi(Base, StructMixin):
            extra: int = field(Int8ub)


def test_dynamic_class_creation_via_make_dataclass_rejected():
    """make_dataclass 动态创建（bases 含结构类）同样触发编译期拒绝。"""

    @dataclass
    class Base(StructMixin):
        magic: int = field(Int16ub)

    with pytest.raises(CompilationError):
        make_dataclass(
            "Dynamic",
            [("extra", Any, field(Int8ub))],
            bases=(Base,),
        )


def test_inheritance_from_compiled_and_used_base_rejected():
    """基类已完成编译并实际使用（parse/build）后再继承 → 仍拒绝。"""

    @dataclass
    class Base(StructMixin):
        magic: int = field(Int16ub)

    assert Base.parse(b"\xca\xfe").magic == 0xCAFE
    assert Base(magic=1).build() == b"\x00\x01"

    with pytest.raises(CompilationError):

        class Sub(Base):
            pass


def test_bitstruct_framework_base_still_allowed():
    """框架入口基类（StructMixin / BitStructMixin）子类化不受影响。"""

    @dataclass
    class Bits(BitStructMixin):
        hi: int = field(Nibble())
        lo: int = field(Nibble())

    assert Bits.parse(b"\x80").hi == 8


def test_plain_mixin_combination_still_allowed():
    """与非 StructMixin 的普通 mixin 组合不受继承拒绝影响。"""

    class PlainMixin:
        def helper(self):
            return 42

    @dataclass
    class WithPlain(PlainMixin, StructMixin):
        x: int = field(Int8ub)

    obj = WithPlain.parse(b"\x05")
    assert obj.x == 5
    assert obj.helper() == 42


def test_composition_pattern_builds_and_parses():
    """组合模式（字段嵌套子 Struct）正常工作——替代继承的推荐用法。"""

    @dataclass
    class Inner(StructMixin):
        magic: int = field(Int16ub)

    @dataclass
    class Outer(StructMixin):
        inner: Inner = field(Inner)
        version: int = field(Int8ub)

    built = Outer(inner=Inner(magic=0xCAFE), version=7).build()
    assert built == b"\xca\xfe\x07"
    parsed = Outer.parse(built)
    assert parsed.inner.magic == 0xCAFE
    assert parsed.version == 7
    assert parsed.build() == built


# ---------------------------------------------------------------------------
# dataclasses.field 遮蔽检测
# ---------------------------------------------------------------------------


def test_dc_field_wrapping_descriptor_rejected():
    """dataclasses.field(default=<协议描述符>) → 编译期拒绝（提示正确 import）。"""

    with pytest.raises(CompilationError) as exc_info:

        @dataclass
        class Shadowed(StructMixin):
            a: int = dc_field(default=Int8ub)
            b: int = field(Int8ub)

    msg = str(exc_info.value)
    assert "dataclasses.field" in msg
    assert "neoconstruct" in msg  # 引导 import 本库的 field


def test_all_dc_fields_class_rejected():
    """整类字段都来自 dataclasses.field（无任何 field() 声明）→ 编译期拒绝。"""

    with pytest.raises(CompilationError) as exc_info:

        @dataclass
        class Shadowed(StructMixin):
            a: int = dc_field(default=5)

    msg = str(exc_info.value)
    assert "dataclasses.field" in msg
    assert "field" in msg


def test_mixed_dc_field_with_binary_field_still_allowed():
    """混合字段模式（二进制字段 + 普通辅助 dc_field 标量默认值）不受影响。"""

    @dataclass
    class Mixed(StructMixin):
        a: int = field(Int8ub)
        note: str = dc_field(default="n/a")

    parsed = Mixed.parse(b"\x07")
    assert parsed.a == 7
    assert parsed.note == "n/a"
    assert parsed.build() == b"\x07"
