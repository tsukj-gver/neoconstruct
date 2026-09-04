"""Pointer 流定位构造器的用户面行为测试（门禁内固化）。

被测语义：绝对/负/相对偏移读写、Struct 内不推进主流位置、build 零填充与
覆盖语义、offset 字段表达式、stream= 换流参数编译期拒绝、round-trip。

期望值来源：SKILL 构造器文档契约（stream 不支持——编译期拒绝）+ 语义
自然性（Pointer 定位读写后回到主流原位置）+ 既有绿灯行为基线。

核心语义约定（本文件锁定）：

- Pointer(offset, subcon)：seek 到 offset 处理 subcon，再 seek 回原位置
  （不占主流位置——后继字段从 Pointer 之前的流位置继续）。
- offset >= 0 绝对定位；offset < 0 从 EOF 向前；relativeOffset=True 相对
  当前位置。
- build：目标位置超出现有长度 → 零填充间隙；写入已写区域 → 覆盖该位置
  字节（主流位置仍不变）。
- offset 支持字段引用表达式（含算术），运行期按 ctx 前序字段求值。
- stream= 非 None → 编译期 CompilationError（换流不支持，fail fast）。
"""

import pytest

from dataclasses import dataclass

from neoconstruct import (
    Bytes,
    CompilationError,
    Int8ub,
    Pointer,
    StructMixin,
    field,
)


class TestPointerParse:
    """Pointer parse：定位读取 + 主流位置不变。"""

    def test_pointer_absolute_offset_parse(self):
        """绝对偏移：Pointer(8, Bytes(1)) 读位置 8 的字节。[自然]"""

        @dataclass
        class P(StructMixin):
            ptr: bytes = field(Pointer(8, Bytes(1)))

        parsed = P.parse(b"abcdefghijkl")
        assert parsed.ptr == b"i"

    def test_pointer_negative_offset_parse_from_end(self):
        """负偏移：Pointer(-2, ...) 从 EOF 向前读（倒数第 2 字节）。[自然]"""

        @dataclass
        class P(StructMixin):
            ptr: bytes = field(Pointer(-2, Bytes(1)))

        parsed = P.parse(b"abcdefgh")
        assert parsed.ptr == b"g"

    def test_pointer_relative_offset_parse(self):
        """relativeOffset=True：相对当前位置 +2 定位（head 3 字节后 → 位置 5）。[文档]"""

        @dataclass
        class P(StructMixin):
            head: bytes = field(Bytes(3))
            ptr: bytes = field(Pointer(2, Bytes(1), relativeOffset=True))
            tail: bytes = field(Bytes(1))

        parsed = P.parse(b"abcdefg")
        assert parsed.head == b"abc"
        assert parsed.ptr == b"f"
        # Pointer 不占主流位置：tail 从位置 3 读
        assert parsed.tail == b"d"

    def test_pointer_does_not_advance_main_stream(self):
        """主流位置不变：ptr 之后 direct 从 Pointer 之前的流位置读取。[自然]"""

        @dataclass
        class P(StructMixin):
            ptr: bytes = field(Pointer(5, Bytes(1)))
            direct: bytes = field(Bytes(1))

        parsed = P.parse(b"01234Xy")
        assert parsed.ptr == b"X"
        assert parsed.direct == b"0"

    def test_pointer_offset_field_expression(self):
        """offset 表达式：Pointer(off+1, ...) 运行期按前序字段求值。[文档]"""

        @dataclass
        class P2(StructMixin):
            off: int = field(Int8ub)
            ptr: bytes = field(Pointer(off + 1, Bytes(1)))

        parsed = P2.parse(b"\x03XYZW")
        assert parsed.off == 3
        assert parsed.ptr == b"W"  # 位置 3+1=4


class TestPointerBuild:
    """Pointer build：零填充 + 覆盖语义。"""

    def test_pointer_build_zero_pads_gap(self):
        """build 超前偏移：b'\\x00'*8 + b'Z'（间隙零填充）。[自然]"""

        @dataclass
        class P(StructMixin):
            ptr: bytes = field(Pointer(8, Bytes(1)))

        assert P(ptr=b"Z").build() == b"\x00" * 8 + b"Z"

    def test_pointer_build_overwrites_target_position(self):
        """build 到已写区域：先写的 direct 在位置 0，Pointer 写位置 2 覆盖
        零填充字节；主流位置不变（direct 之后主流仍在 1）。[基线]"""

        @dataclass
        class P(StructMixin):
            ptr: bytes = field(Pointer(2, Bytes(1)))
            direct: bytes = field(Bytes(1))

        built = P(ptr=b"Z", direct=b"X").build()
        assert built[0:1] == b"X"
        assert built[2:3] == b"Z"
        assert built == b"X\x00Z"

    def test_pointer_roundtrip(self):
        """round-trip：parse → build 字节稳定（目标字节重写回原位置）。[自然]"""

        @dataclass
        class P(StructMixin):
            head: bytes = field(Bytes(2))
            ptr: bytes = field(Pointer(4, Bytes(2)))
            tail: bytes = field(Bytes(2))

        raw = b"ab" + b"\x00\x00" + b"XY"
        parsed = P.parse(raw)
        assert parsed.ptr == b"XY"
        assert parsed.build() == raw


class TestPointerCompileBoundaries:
    """Pointer 编译期边界。"""

    def test_pointer_stream_param_compile_rejected(self):
        """stream= 非 None → 编译期 CompilationError + "stream" 语义词
        （换流不支持，fail fast）。[文档]"""

        with pytest.raises(CompilationError, match="stream"):

            @dataclass
            class PS(StructMixin):
                ptr: bytes = field(Pointer(8, Bytes(1), stream=lambda ctx: None))

    def test_pointer_field_optional_instantiation(self):
        """实例化：随 inner 根分类——over Bytes 可省（None，缺值错误后移到
        build 值使用点）。[基线]"""

        @dataclass
        class P(StructMixin):
            ptr: bytes = field(Pointer(8, Bytes(1)))

        assert P().ptr is None
