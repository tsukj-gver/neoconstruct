"""值域校验构造器的用户面行为测试：OneOf / NoneOf。

被测语义：两类值域校验构造器嵌入 field() 后的 parse 放行/拒绝、build 侧
对称校验（错误时机双面锁定）、round-trip、错误类型与消息语义词。

期望值来源：SKILL 构造器文档契约 + 语义自然性（parse/build 两侧校验对称）
+ 既有绿灯行为基线。

核心语义约定（本文件锁定）：

- OneOf：inner 结果 ∈ valids 放行（值原样转发），∉ valids → ValidationError
  （parse 与 build 两侧对称）。
- NoneOf：inner 结果 ∉ invalids 放行，∈ invalids → ValidationError。
- 错误消息含 "validation" 语义词。
- 两者均为 Instance 值来源：field() 无参实例化 → TypeError。
"""

import pytest

from dataclasses import dataclass
from typing import Any

from neoconstruct import (
    Byte,
    NoneOf,
    OneOf,
    StructMixin,
    ValidationError,
    field,
)


class TestOneOfSemantics:
    """OneOf(Byte, [1, 2, 3]) × field()：合法值集合校验。"""

    def test_oneof_parse_valid_value_passes_through(self):
        """parse 合法值：b'\\x02' → 2（值原样转发）。[文档]"""

        @dataclass
        class O(StructMixin):
            v: Any = field(OneOf(Byte, [1, 2, 3]))

        parsed = O.parse(b"\x02")
        assert parsed.v == 2

    def test_oneof_parse_invalid_raises_validation_error(self):
        """parse 非法值：b'\\xff' → ValidationError + "validation" 语义词。[文档]"""

        @dataclass
        class O(StructMixin):
            v: Any = field(OneOf(Byte, [1, 2, 3]))

        with pytest.raises(ValidationError, match="validation"):
            O.parse(b"\xff")

    def test_oneof_build_valid_value_passes(self):
        """build 合法值：v=3 → b'\\x03'。[自然]"""

        @dataclass
        class O(StructMixin):
            v: Any = field(OneOf(Byte, [1, 2, 3]))

        assert O(v=3).build() == b"\x03"

    def test_oneof_build_invalid_raises_validation_error(self):
        """build 非法值：v=9 → ValidationError（parse/build 校验对称）。[自然]"""

        @dataclass
        class O(StructMixin):
            v: Any = field(OneOf(Byte, [1, 2, 3]))

        with pytest.raises(ValidationError, match="validation"):
            O(v=9).build()

    def test_oneof_roundtrip(self):
        """round-trip：合法值 parse → build 字节不变。[自然]"""

        @dataclass
        class O(StructMixin):
            v: Any = field(OneOf(Byte, [1, 2, 3]))

        assert O.parse(b"\x01").build() == b"\x01"

    def test_oneof_field_requires_explicit_value(self):
        """实例化：必填（Instance 值来源 → TypeError）。[基线]"""

        @dataclass
        class O(StructMixin):
            v: Any = field(OneOf(Byte, [1, 2, 3]))

        with pytest.raises(TypeError):
            O()


class TestNoneOfSemantics:
    """NoneOf(Byte, [1, 2, 3]) × field()：禁值集合校验。"""

    def test_noneof_parse_allowed_value_passes(self):
        """parse 非禁值：b'\\xff' → 255 放行。[文档]"""

        @dataclass
        class N(StructMixin):
            v: Any = field(NoneOf(Byte, [1, 2, 3]))

        parsed = N.parse(b"\xff")
        assert parsed.v == 255

    def test_noneof_parse_forbidden_raises_validation_error(self):
        """parse 禁值：b'\\x01' → ValidationError。[文档]"""

        @dataclass
        class N(StructMixin):
            v: Any = field(NoneOf(Byte, [1, 2, 3]))

        with pytest.raises(ValidationError, match="validation"):
            N.parse(b"\x01")

    def test_noneof_build_forbidden_raises_validation_error(self):
        """build 禁值：v=1 → ValidationError（两侧对称）。[自然]"""

        @dataclass
        class N(StructMixin):
            v: Any = field(NoneOf(Byte, [1, 2, 3]))

        with pytest.raises(ValidationError, match="validation"):
            N(v=1).build()

    def test_noneof_roundtrip(self):
        """round-trip：非禁值 parse → build 字节不变。[自然]"""

        @dataclass
        class N(StructMixin):
            v: Any = field(NoneOf(Byte, [1, 2, 3]))

        assert N.parse(b"\x7f").build() == b"\x7f"

    def test_noneof_field_requires_explicit_value(self):
        """实例化：必填（Instance 值来源 → TypeError）。[基线]"""

        @dataclass
        class N(StructMixin):
            v: Any = field(NoneOf(Byte, [1, 2, 3]))

        with pytest.raises(TypeError):
            N()
