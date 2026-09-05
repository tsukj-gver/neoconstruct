"""显示与调试构造器的用户面行为测试：Hex / HexDump / Probe。

被测语义：三类显示/调试构造器嵌入 field() 后的 parse 值类型（显示类子类）
与显示文本形态、build 透传对称、round-trip、Probe 的 stdout 输出与流位置
不扰动、实例化值语义（随 inner 根分类）。

期望值来源：SKILL 构造器文档契约（显示类形态）+ 语义自然性（显示包装不改
变字节语义）+ 既有绿灯行为基线。

核心语义约定（本文件锁定）：

- Hex(Int)：parse → HexDisplayedInteger（int 子类，值等于原整数，
  str() 为 0x 前导定宽 hex）；build 接受普通 int。
- Hex(Bytes)：parse → HexDisplayedBytes（bytes 子类，值等于原字节，
  str() 为 unhexlify(...) 形态）。
- HexDump(Bytes)：parse → HexDumpDisplayedBytes（bytes 子类，str() 为
  hexundump 三引号 hexdump 格式，含行偏移）；int 透传不包装（文档化）。
- 三者 build 透传 inner.build —— 字节语义与不包装时一致。
- Probe：parse/build 打印探针输出（分隔线 + "Probe, path is ..." + 可选
  Stream peek 行 + 可选字段值行）；不消费流位置（后继字段不受影响）。
  into=字段名 → 打印该字段值；lookahead=n → 打印后续 n 字节 hex；
  引用不存在的字段 → <field ... missing>。
- 显示包装字段的实例化默认随 inner 根节点分类（over Int → 必填；
  over Bytes → 可省 None）。
"""

import pytest

from dataclasses import dataclass
from typing import Any

from neoconstruct import (
    Byte,
    Bytes,
    Hex,
    HexDump,
    Int32ub,
    Probe,
    StructMixin,
    field,
    rfield,
)


class TestHexIntSemantics:
    """Hex(Int32ub) × field()：int 显示包装。"""

    def test_hex_int_parse_returns_hex_displayed_integer(self):
        """parse：b'\\x00\\x00\\x01\\x02' → HexDisplayedInteger(258)，
        int()==258，str()=='0x00000102'。[文档]"""

        @dataclass
        class H(StructMixin):
            magic: Any = field(Hex(Int32ub))

        parsed = H.parse(b"\x00\x00\x01\x02")
        assert int(parsed.magic) == 258
        assert isinstance(parsed.magic, int)
        assert str(parsed.magic) == "0x00000102"

    def test_hex_int_build_from_plain_int(self):
        """build：magic=0x102 → b'\\x00\\x00\\x01\\x02'（透传 inner）。[自然]"""

        @dataclass
        class H(StructMixin):
            magic: Any = field(Hex(Int32ub))

        assert H(magic=0x102).build() == b"\x00\x00\x01\x02"

    def test_hex_int_roundtrip(self):
        """round-trip：parse → build 字节不变。[自然]"""

        @dataclass
        class H(StructMixin):
            magic: Any = field(Hex(Int32ub))

        assert H.parse(b"\x00\x00\x01\x02").build() == b"\x00\x00\x01\x02"

    def test_hex_int_field_requires_explicit_value(self):
        """实例化：over Int → 必填（随 inner 根分类）。[基线]"""

        @dataclass
        class H(StructMixin):
            magic: Any = field(Hex(Int32ub))

        with pytest.raises(TypeError):
            H()


class TestHexBytesSemantics:
    """Hex(Bytes(4)) × field()：bytes 显示包装。"""

    def test_hex_bytes_parse_returns_hex_displayed_bytes(self):
        """parse：值等于原字节（bytes 子类），str() 为 unhexlify 形态。[文档]

        str() 契约：``unhexlify('...')`` 可 eval 代码形态（repr 可回粘贴
        执行），display 场景用户应使用 bytes 值本身或 hex()。
        """

        @dataclass
        class HB(StructMixin):
            data: Any = field(Hex(Bytes(4)))

        parsed = HB.parse(b"\x00\x00\x01\x02")
        assert parsed.data == b"\x00\x00\x01\x02"
        assert str(parsed.data) == "unhexlify('00000102')"

    def test_hex_bytes_build_roundtrip(self):
        """build/round-trip：data=bytes → 原字节；parse→build 稳定。[自然]"""

        @dataclass
        class HB(StructMixin):
            data: Any = field(Hex(Bytes(4)))

        assert HB(data=b"\xde\xad\xbe\xef").build() == b"\xde\xad\xbe\xef"
        assert HB.parse(b"\xde\xad\xbe\xef").build() == b"\xde\xad\xbe\xef"

    def test_hex_bytes_field_optional_instantiation(self):
        """实例化：over Bytes → 可省（None）——随 inner 根分类。[基线]"""

        @dataclass
        class HB(StructMixin):
            data: Any = field(Hex(Bytes(4)))

        assert HB().data is None


class TestHexDumpSemantics:
    """HexDump(Bytes(8)) × field()：hexdump 显示包装。"""

    def test_hexdump_bytes_parse_display_format(self):
        """parse：值等于原字节；str() 为 hexundump 三引号格式（含行偏移
        '0000' 与 hex 列）。[文档]"""

        @dataclass
        class HD(StructMixin):
            data: Any = field(HexDump(Bytes(8)))

        parsed = HD.parse(b"\x00\x00\x01\x02" + b"ABCD")
        assert parsed.data == b"\x00\x00\x01\x02ABCD"
        rendered = str(parsed.data)
        assert "hexundump" in rendered
        assert "0000" in rendered
        assert "00 00 01 02 41 42 43 44" in rendered

    def test_hexdump_int_passthrough_not_wrapped(self):
        """parse int：透传普通 int（不包装——HexDump 仅 bytes/dict 形态）。[文档]"""

        @dataclass
        class HDI(StructMixin):
            v: Any = field(HexDump(Int32ub))

        parsed = HDI.parse(b"\x00\x00\x01\x02")
        assert int(parsed.v) == 258
        assert type(parsed.v) is int

    def test_hexdump_build_roundtrip(self):
        """build/round-trip：字节语义与不包装时一致。[自然]"""

        @dataclass
        class HD(StructMixin):
            data: Any = field(HexDump(Bytes(8)))

        payload = bytes(range(8))
        assert HD(data=payload).build() == payload
        assert HD.parse(payload).build() == payload


class TestProbeSemantics:
    """Probe × rfield()：调试探针输出与流位置不扰动。"""

    def test_probe_parse_prints_probe_banner_without_disturbing_stream(self, capsys):
        """parse：stdout 含探针横幅 + "Probe, path is"行；后继字段值正确
        （流位置不被探针扰动）。[文档+自然]

        双重断言：输出行（分隔线 + "Probe, path is root.p"）；流不扰动
        （b 字段值正确）。
        """

        @dataclass
        class PR(StructMixin):
            a: int = field(Byte)
            p: Any = rfield(Probe())
            b: int = field(Byte)

        parsed = PR.parse(b"\x01\x02")
        out = capsys.readouterr().out
        assert "Probe, path is root.p" in out
        assert "===" in out
        assert parsed.a == 1
        assert parsed.b == 2

    def test_probe_into_prints_field_value(self, capsys):
        """into=字段名：打印该字段当前值（repr 行精确匹配）。[文档]

        into 行是字段值的 repr 独占一行（非子串误命中）：a=7 → 行 "7"。
        """

        @dataclass
        class PRI(StructMixin):
            a: int = field(Byte)
            p: Any = rfield(Probe(into="a"))
            b: int = field(Byte)

        PRI.parse(b"\x07\x08")
        out = capsys.readouterr().out
        assert "Probe" in out
        assert "\n7\n" in out

    def test_probe_lookahead_prints_hex_of_next_bytes(self, capsys):
        """lookahead=n：打印 "Stream peek: <hex>"（后继 n 字节 hex）。[文档]"""

        @dataclass
        class PRL(StructMixin):
            a: int = field(Byte)
            p: Any = rfield(Probe(lookahead=3))
            b: int = field(Byte)

        PRL.parse(b"\x0a\x0b\x0c\x0d")
        out = capsys.readouterr().out
        assert "Stream peek: 0b0c0d" in out

    def test_probe_into_missing_field_reports_missing(self, capsys):
        """into=不存在的字段：输出 <field ... missing>（显式缺失提示）。[自然]"""

        @dataclass
        class PRM(StructMixin):
            a: int = field(Byte)
            p: Any = rfield(Probe(into="no_such_field"))
            b: int = field(Byte)

        PRM.parse(b"\x01\x02")
        out = capsys.readouterr().out
        assert "<field no_such_field missing>" in out

    def test_probe_roundtrip_after_parse(self):
        """round-trip：rfield(Probe()) parse → build 字节稳定（探针 no-op）。[自然]"""

        @dataclass
        class PR(StructMixin):
            a: int = field(Byte)
            p: Any = rfield(Probe())
            b: int = field(Byte)

        parsed = PR.parse(b"\x01\x02")
        assert parsed.build() == b"\x01\x02"
