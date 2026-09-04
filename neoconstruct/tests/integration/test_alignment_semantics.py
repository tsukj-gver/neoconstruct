"""对齐构造器的用户面行为测试：Aligned / AlignedStruct。

被测语义：对齐包装与对齐结构宏的 parse padding 消费 / build padding 写出
（三态：恰整/差 1/差 n-1）/ 自定义 pattern / modulus 边界（最小值 2 的编译
期拒绝）/ modulus 字段表达式 / 变长字段交互 / AlignedStruct 多字段统一
模数 / 实例化值语义。

期望值来源：SKILL 构造器文档契约（modulus >= 2、pattern 1 字节）+ 语义
自然性（parse/build 对称、padding 字节确定）+ 既有绿灯行为基线。

核心语义约定（本文件锁定）：

- Aligned(modulus, inner)：inner 之后补 `(modulus - inner_size % modulus)
  % modulus` 个 pattern 字节；parse 消费 padding、build 写出同样字节
  （round-trip 字节稳定）。
- pattern 默认 b"\\x00"，可自定义（build 字节断言锁定）。
- modulus < 2 → 编译期 CompilationError（fail fast）。
- modulus 支持字段引用表达式（前序 int 字段）。
- 变长 inner（Bytes(x+1) 表达式长度）交互：按 inner 实际字节数对齐。
- AlignedStruct(modulus, **fields)：动态生成 dataclass，每字段独立对齐
  （宏展开）；字段实例化必填。
"""

import pytest

from dataclasses import dataclass
from typing import Any

from neoconstruct import (
    Aligned,
    AlignedStruct,
    Byte,
    Bytes,
    CompilationError,
    Int16ub,
    Int8ub,
    StructMixin,
    field,
)


class TestAlignedSemantics:
    """Aligned(4, ...) × field()：对齐三态 + pattern + 边界。"""

    @pytest.mark.parametrize(
        "inner,build_value,payload,pad_bytes",
        [
            # 差 n-1：inner 2 字节 + 2 padding
            (Int16ub, 1, b"\x00\x01", b"\x00\x00"),
            # 差 1：inner 3 字节 + 1 padding
            (Bytes(3), b"abc", b"abc", b"\x00"),
            # 恰整：inner 4 字节 + 0 padding
            (Bytes(4), b"abcd", b"abcd", b""),
        ],
        ids=["short-by-2", "short-by-1", "exact-multiple"],
    )
    def test_aligned_build_writes_expected_padding(self, inner, build_value, payload, pad_bytes):
        """build 三态：inner + 确定数量 padding（差2/差1/恰整）。[自然]"""

        @dataclass
        class A(StructMixin):
            v: Any = field(Aligned(4, inner))

        assert A(v=build_value).build() == payload + pad_bytes

    def test_aligned_parse_consumes_padding(self):
        """parse：消费 padding——后继字段从对齐边界后读取；round-trip 稳定。[自然]"""

        @dataclass
        class A(StructMixin):
            v: Any = field(Aligned(4, Int16ub))
            tail: bytes = field(Bytes(1))

        parsed = A.parse(b"\x00\x01\x00\x00Z")
        assert int(parsed.v) == 1
        assert parsed.tail == b"Z"
        assert parsed.build() == b"\x00\x01\x00\x00Z"

    def test_aligned_custom_pattern_bytes(self):
        """pattern=b'\\xff'：build 写 0xFF padding 字节。[文档]"""

        @dataclass
        class AP(StructMixin):
            v: Any = field(Aligned(4, Int16ub, pattern=b"\xff"))

        assert AP(v=1).build() == b"\x00\x01\xff\xff"

    def test_aligned_modulus_below_two_compile_rejected(self):
        """modulus=1 → 编译期 CompilationError + "must be >= 2" 语义词。[文档]"""

        with pytest.raises(CompilationError, match=">= 2"):

            @dataclass
            class A1(StructMixin):
                v: Any = field(Aligned(1, Int16ub))

    def test_aligned_modulus_field_expression(self):
        """modulus 字段引用：m=4 时按 4 对齐（表达式 modulus 路径）。[文档]"""

        @dataclass
        class AE(StructMixin):
            m: int = field(Int8ub)
            v: Any = field(Aligned(m, Int16ub))

        assert AE(m=4, v=1).build() == b"\x04\x00\x01\x00\x00"

    def test_aligned_variable_length_inner_interaction(self):
        """变长 inner：Bytes(x+1) 实际 2 字节 → 对齐到 4 补 2 字节。[自然]"""

        @dataclass
        class AV(StructMixin):
            x: int = field(Int8ub)
            v: Any = field(Aligned(4, Bytes(x + 1)))

        assert AV(x=1, v=b"ab").build() == b"\x01ab\x00\x00"
        parsed = AV.parse(b"\x01ab\x00\x00Z")
        assert parsed.v == b"ab"
        assert parsed.build() == b"\x01ab\x00\x00"

    def test_aligned_field_requires_explicit_value(self):
        """实例化：必填（Instance 值来源 → TypeError）。[基线]"""

        @dataclass
        class A(StructMixin):
            v: Any = field(Aligned(4, Int16ub))

        with pytest.raises(TypeError):
            A()


class TestAlignedStructSemantics:
    """AlignedStruct(modulus, **fields)：多字段统一模数宏。"""

    def test_alignedstruct_build_pads_each_field(self):
        """build：每字段独立对齐（a: 1+3 pad，b: 2+2 pad）。[文档]"""

        AP4 = AlignedStruct(4, a=Int8ub, b=Int16ub)

        assert AP4(a=1, b=2).build() == b"\x01\x00\x00\x00" + b"\x00\x02\x00\x00"

    def test_alignedstruct_parse_roundtrip(self):
        """parse：字段值正确 + padding 被消费（后继字段在边界上）；round-trip
        字节稳定。[自然]"""

        AP4 = AlignedStruct(4, a=Int8ub, b=Int16ub)

        raw = b"\x01\x00\x00\x00" + b"\x00\x02\x00\x00"
        parsed = AP4.parse(raw)
        assert parsed.a == 1
        assert parsed.b == 2
        assert parsed.build() == raw

    def test_alignedstruct_fields_required_instantiation(self):
        """实例化：动态类字段必填（缺 a/b → TypeError）。[基线]"""

        AP4 = AlignedStruct(4, a=Int8ub, b=Int16ub)

        with pytest.raises(TypeError):
            AP4()
        with pytest.raises(TypeError):
            AP4(a=1)
