"""字节变换与时间戳构造器的用户面行为测试：ProcessXor / ProcessRotateLeft
/ Timestamp。

被测语义：三类变换构造器嵌入 field() 后的 encode/decode 互逆（round-trip）
、pad/amount/group 参数面（定长 pad、bytes pad、字段表达式 pad、group 分支）
、空输入与恒等边界、callable 编译期拒绝、RotationError / TimestampError
错误路径、实例化值语义。

期望值来源：SKILL 构造器文档契约（padfunc 不接受 callable、Timestamp 参数
面）+ 语义自然性（变换对合互逆）+ 既有绿灯行为基线。

核心语义约定（本文件锁定）：

- ProcessXor pad：int（每字节同 pad）/ bytes（循环 pad）/ 0 恒等 /
  字段引用表达式（运行期求值）。XOR 对合 → parse/build 同变换。
- ProcessRotateLeft(amount, group)：group=1 单字节旋转移位；group>1 走
  通用 bit rotate 分支；amount=0 恒等；build 取负 amount（互逆）。
- ProcessRotateLeft group=0 → RotationError（parse 侧已验证）。
- ProcessXor padfunc 传 callable → 编译期 CompilationError。
- Timestamp(subcon, unit, epoch)：unit 是"每 tick 秒数"（0.001=毫秒 tick），
  epoch 为 int 年（1 月 1 日）；参数类型错 → TimestampError（macro 层
  fail fast）。arrow 缺失时跳过（可选依赖）。
"""

import pytest

from dataclasses import dataclass
from typing import Any

from neoconstruct import (
    Byte,
    Bytes,
    CompilationError,
    Int16ub,
    Int64ub,
    ProcessRotateLeft,
    ProcessXor,
    RotationError,
    StructMixin,
    TimestampError,
    field,
)

try:
    import arrow  # noqa: F401

    ARROW_AVAILABLE = True
except ImportError:
    ARROW_AVAILABLE = False

requires_arrow = pytest.mark.skipif(
    not ARROW_AVAILABLE, reason="Timestamp 依赖可选包 arrow（pip install arrow）"
)


class TestProcessXorSemantics:
    """ProcessXor(pad, subcon) × field()：XOR 字节变换。"""

    def test_processxor_int_pad_roundtrip(self):
        """int pad：parse b'\\x2a\\x00\\xff' → key=42, data=0xF00F（0xF0 pad
        对合）；build 还原原字节。[文档+自然]"""

        @dataclass
        class X(StructMixin):
            key: int = field(Byte)
            data: Any = field(ProcessXor(0xF0, Int16ub))

        parsed = X.parse(b"\x2a\x00\xff")
        assert parsed.key == 42
        assert int(parsed.data) == 0xF00F
        assert parsed.build() == b"\x2a\x00\xff"
        assert X(key=0x2A, data=0xF00F).build() == b"\x2a\x00\xff"

    def test_processxor_bytes_pad_cyclic(self):
        """bytes pad：b'\\xf0\\xf1' 循环 XOR —— b'\\x00\\xff' → 0xF00E。[文档]"""

        @dataclass
        class XB(StructMixin):
            data: Any = field(ProcessXor(b"\xf0\xf1", Int16ub))

        parsed = XB.parse(b"\x00\xff")
        assert int(parsed.data) == 0xF00E
        assert XB(data=0xF00E).build() == b"\x00\xff"

    def test_processxor_zero_pad_identity(self):
        """pad=0 恒等：数据不变（fast-path 语义）。[文档+自然]"""

        @dataclass
        class X0(StructMixin):
            data: Any = field(ProcessXor(0, Int16ub))

        parsed = X0.parse(b"\x00\xff")
        assert int(parsed.data) == 0x00FF
        assert X0(data=0x00FF).build() == b"\x00\xff"

    def test_processxor_empty_input_roundtrip(self):
        """空输入边界：Bytes(0) 变换后仍为空（build b''）。[自然]"""

        @dataclass
        class XE(StructMixin):
            data: Any = field(ProcessXor(0xFF, Bytes(0)))

        assert XE.parse(b"").data == b""
        assert XE(data=b"").build() == b""

    def test_processxor_field_expression_pad(self):
        """pad 字段引用：pad 字段值参与运行期 XOR（前序字段读 ctx）。[文档]"""

        @dataclass
        class XP(StructMixin):
            pad: int = field(Byte)
            data: Any = field(ProcessXor(pad, Int16ub))

        parsed = XP.parse(b"\xf0\x00\xff")
        assert parsed.pad == 0xF0
        assert int(parsed.data) == 0xF00F
        assert XP(pad=0xF0, data=0xF00F).build() == b"\xf0\x00\xff"

    def test_processxor_callable_pad_compile_rejected(self):
        """padfunc=lambda → 编译期 CompilationError（不接受 callable）。[文档]"""

        with pytest.raises(CompilationError):

            @dataclass
            class XC(StructMixin):
                data: Any = field(ProcessXor(lambda ctx: 0xFF, Int16ub))

    def test_processxor_field_requires_explicit_value(self):
        """实例化：必填（Instance 值来源 → TypeError）。[基线]"""

        @dataclass
        class X(StructMixin):
            key: int = field(Byte)
            data: Any = field(ProcessXor(0xF0, Int16ub))

        with pytest.raises(TypeError):
            X(key=1)


class TestProcessRotateLeftSemantics:
    """ProcessRotateLeft(amount, group, subcon) × field()：位旋转。"""

    def test_rotate_group1_roundtrip(self):
        """group=1：b'\\x0f\\xf0' → 0xF00F（每字节旋 4 位）；build 取负互逆。[文档]"""

        @dataclass
        class R(StructMixin):
            data: Any = field(ProcessRotateLeft(4, 1, Int16ub))

        parsed = R.parse(b"\x0f\xf0")
        assert int(parsed.data) == 0xF00F
        assert parsed.build() == b"\x0f\xf0"
        assert R(data=0xF00F).build() == b"\x0f\xf0"

    def test_rotate_group2_generic_branch(self):
        """group=2：b'\\x0f\\xf0' → 0xFF00（通用 bit rotate 分支）。[文档]"""

        @dataclass
        class R2(StructMixin):
            data: Any = field(ProcessRotateLeft(4, 2, Int16ub))

        parsed = R2.parse(b"\x0f\xf0")
        assert int(parsed.data) == 0xFF00
        assert R2(data=0xFF00).build() == b"\x0f\xf0"

    def test_rotate_zero_amount_identity(self):
        """amount=0 恒等：数据不变。[自然]"""

        @dataclass
        class R0(StructMixin):
            data: Any = field(ProcessRotateLeft(0, 1, Int16ub))

        parsed = R0.parse(b"\x0f\xf0")
        assert int(parsed.data) == 0x0FF0
        assert R0(data=0x0FF0).build() == b"\x0f\xf0"

    def test_rotate_group_zero_parse_raises_rotation_error(self):
        """group=0 parse：RotationError + "group" 语义词。[自然]"""

        @dataclass
        class RG(StructMixin):
            data: Any = field(ProcessRotateLeft(4, 0, Int16ub))

        with pytest.raises(RotationError, match="group"):
            RG.parse(b"\x0f\xf0")

    @pytest.mark.xfail(
        strict=True,
        reason="疑似库缺陷：group=0 build 侧 Rust panic（rem_euclid 除零，"
        "process_rotate_left build 分支在参数校验前取负 amount）——违反"
        "『非法输入返回 Err 而非 panic』红线；自然语义应与 parse 同报 "
        "RotationError",
    )
    def test_rotate_group_zero_build_raises_rotation_error(self):
        """group=0 build：应报 RotationError（与 parse 对称）。[自然]"""

        @dataclass
        class RG(StructMixin):
            data: Any = field(ProcessRotateLeft(4, 0, Int16ub))

        with pytest.raises(RotationError):
            RG(data=1).build()

    def test_rotate_field_requires_explicit_value(self):
        """实例化：必填（Instance 值来源 → TypeError）。[基线]"""

        @dataclass
        class R(StructMixin):
            data: Any = field(ProcessRotateLeft(4, 1, Int16ub))

        with pytest.raises(TypeError):
            R()


class TestTimestampSemantics:
    """Timestamp(subcon, unit, epoch) × field()：时间戳宏（arrow 可选依赖）。"""

    @requires_arrow
    def test_timestamp_epoch_seconds_parse_and_build(self):
        """unit=1 秒 / epoch=1970：已知字节 → 2018-01-01（文档示例值）；
        build 还原原字节。[文档]"""

        from neoconstruct import Timestamp

        @dataclass
        class T(StructMixin):
            ts: Any = field(Timestamp(Int64ub, 1.0, 1970))

        raw = b"\x00\x00\x00\x00ZIz\x00"
        parsed = T.parse(raw)
        assert arrow.get("2018-01-01T00:00:00+00:00") == parsed.ts
        assert T(ts=parsed.ts).build() == raw

    @requires_arrow
    def test_timestamp_millisecond_unit(self):
        """unit=0.001（毫秒 tick）：3_600_000 ticks → 1970-01-01T01:00；
        round-trip 字节稳定。[自然]"""

        from neoconstruct import Timestamp

        @dataclass
        class TM(StructMixin):
            ts: Any = field(Timestamp(Int64ub, 0.001, 1970))

        raw = (3_600_000).to_bytes(8, "big")
        parsed = TM.parse(raw)
        assert arrow.get("1970-01-01T01:00:00+00:00") == parsed.ts
        assert TM(ts=parsed.ts).build() == raw

    @requires_arrow
    def test_timestamp_epoch_year_offset(self):
        """epoch=2000：tick 0 → 2000-01-01（epoch 年偏移生效）。[自然]"""

        from neoconstruct import Timestamp

        @dataclass
        class T2(StructMixin):
            ts: Any = field(Timestamp(Int64ub, 1.0, 2000))

        parsed = T2.parse((0).to_bytes(8, "big"))
        assert arrow.get("2000-01-01T00:00:00+00:00") == parsed.ts

    @requires_arrow
    def test_timestamp_invalid_unit_raises_timestamp_error(self):
        """unit=None → TimestampError + "unit" 语义词（macro 层 fail fast）。[文档]"""

        from neoconstruct import Timestamp

        with pytest.raises(TimestampError, match="unit"):
            Timestamp(Int64ub, None, 1970)

    @requires_arrow
    def test_timestamp_invalid_epoch_raises_timestamp_error(self):
        """epoch=None → TimestampError + "epoch" 语义词。[文档]"""

        from neoconstruct import Timestamp

        with pytest.raises(TimestampError, match="epoch"):
            Timestamp(Int64ub, 1.0, None)

    @requires_arrow
    def test_timestamp_none_subcon_raises_timestamp_error(self):
        """subcon=None → TimestampError + "subcon" 语义词。[文档]"""

        from neoconstruct import Timestamp

        with pytest.raises(TimestampError, match="subcon"):
            Timestamp(None, 1.0, 1970)

    @requires_arrow
    def test_timestamp_field_requires_explicit_value(self):
        """实例化：必填（Instance 值来源 → TypeError）。[基线]"""

        from neoconstruct import Timestamp

        @dataclass
        class T(StructMixin):
            ts: Any = field(Timestamp(Int64ub, 1.0, 1970))

        with pytest.raises(TypeError):
            T()
