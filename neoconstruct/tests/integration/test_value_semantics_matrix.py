"""字段值语义组合矩阵测试。

被测语义：值语义构造器（Const/Default/Rebuild/Computed/Padding/Peek/Seek/
Checksum/If 包装）× 字段模式（field/rfield/wfield）× 值时机（实例化/parse/
build 缺省/build 显式等值/build 显式不等值/build 显式 None）× 嵌套位置
（顶层/Prefixed×1/Prefixed×2/Switch 内）× 表达式形态（字段引用/算术/常量）×
表达式消费者（Bytes/Array/Switch/If/Computed/PrefixedArray）× 数据负例
（长度声明与实际 payload 不符）。

期望值来源：neoconstruct 自主语义（自身行为基线 + 语义自然性）。每用例断言
**对象字段值**（非仅字节）。

核心语义约定（本矩阵锁定的不变量）：

- 值提供型构造器（Const/Default 常量折叠）实例化即携带构造器值；
  非折叠表达式值（Default(expr)/Rebuild(expr)/Computed(expr)）实例化为 None。
- 低频控制与流操作（Peek/Seek/Checksum/Tell/StopIf/Check/Terminated/Element/
  Index/Pass）实例化可省（build 值不由实例决定）。
- build 循环按值来源分类统一 resolve：缺省补值（Const 补常量/Default 求值/
  Control 补哑值 None）、显式实例值尊重（Control 的显式值/parse 产出值透传
  回 context，后继表达式读到有效值——peek→派生→分派链 round-trip 保持）。
- 从零 build 缺值（Instance 字段实例值为 None）报 FieldValueMissingError，
  错误时机在 build（值使用点）而非实例化。
- Control 缺省哑值 None 流入后继表达式消费者时报错（显式失败优于静默错值）。
- 条件根（If/Switch/Select）按根节点分类为 Conditional：None（含缺省）透传
  分支自治——Pass 分支不写字节；实值分支收到 None 由分支节点自然报错。
- context= 注入参数在编译期拒绝（fail fast，消除静默忽略）。
- 数据-schema 不匹配（长度声明 vs 实际）parse 显式报 StreamError。

dataclass 一律定义在测试函数体内（与回归测试惯例一致）。
"""

from dataclasses import dataclass
from typing import Any

import pytest

from neoconstruct import (
    Array,
    Byte,
    Bytes,
    Checksum,
    ChecksumError,
    Computed,
    CompilationError,
    Const,
    ConstError,
    ConstructError,
    Default,
    FieldValueMissingError,
    FormatFieldError,
    GreedyBytes,
    If,
    Int8ub,
    Padding,
    Peek,
    Prefixed,
    PrefixedArray,
    Rebuild,
    Seek,
    StreamError,
    StructMixin,
    Switch,
    Tell,
    field,
    rfield,
    wfield,
)


def _modbus_crc16(data: bytes) -> bytes:
    """CRC-16/MODBUS（poly 0xA001 reversed, init 0xFFFF，小端 2 字节）。

    用于 Checksum 用例的确定性摘要函数：crc16(b'\\x01\\x02') == b'\\x81\\xe1'。
    """

    crc = 0xFFFF
    for byte in data:
        crc ^= byte
        for _ in range(8):
            if crc & 0x0001:
                crc = (crc >> 1) ^ 0xA001
            else:
                crc >>= 1
    return bytes([crc & 0xFF, (crc >> 8) & 0xFF])


# ---------------------------------------------------------------------------
# Const × field：int / bytes 两形态 × 六时机
# ---------------------------------------------------------------------------


class TestConstIntSixTimings:
    """Const(5, Int8ub) × field：实例化/parse/build 缺省/显式等值/不等/None。

    语义：Const 字段值恒为构造器值——实例化携带该值；build 缺省或 None 补
    常量；显式等值用之；不等值报 ConstError（值校验留在节点层）。
    """

    def test_instantiation_carries_const_value(self):
        """实例化：C2().c == 5（Const 值即字段默认值）。"""

        @dataclass
        class C2(StructMixin):
            c: int = field(Const(0x05, Int8ub))

        assert C2().c == 0x05

    def test_parse_returns_const(self):
        """parse：parse(b'\\x05').c == 5（校验相等后转发值）。"""

        @dataclass
        class C2(StructMixin):
            c: int = field(Const(0x05, Int8ub))

        parsed = C2.parse(b"\x05")
        assert parsed.c == 5

    def test_build_defaulted_uses_const(self):
        """build 缺省：C2().build() == b'\\x05'（resolve 补常量）。"""

        @dataclass
        class C2(StructMixin):
            c: int = field(Const(0x05, Int8ub))

        assert C2().build() == b"\x05"

    def test_build_explicit_equal_accepted(self):
        """build 显式等值：C2(c=5).build() == b'\\x05'。"""

        @dataclass
        class C2(StructMixin):
            c: int = field(Const(0x05, Int8ub))

        assert C2(c=5).build() == b"\x05"

    def test_build_explicit_unequal_const_error(self):
        """build 显式不等值：C2(c=9).build() → ConstError（节点校验）。"""

        @dataclass
        class C2(StructMixin):
            c: int = field(Const(0x05, Int8ub))

        with pytest.raises(ConstError):
            C2(c=9).build()

    def test_build_explicit_none_uses_const(self):
        """build 显式 None：C2(c=None).build() == b'\\x05'（None≡缺省）。"""

        @dataclass
        class C2(StructMixin):
            c: int = field(Const(0x05, Int8ub))

        assert C2(c=None).build() == b"\x05"


class TestConstBytesSixTimings:
    """Const(b'Z', Bytes(1)) × field：bytes 形态六时机。

    语义：与 int 形态一致——bytes 不可变标量可作为字段默认值携带。
    """

    def test_instantiation_carries_const_value(self):
        """实例化：P1().c == b'Z'（bytes 不可变标量→默认值携带）。"""

        @dataclass
        class P1(StructMixin):
            c: bytes = field(Const(b"Z", Bytes(1)))

        assert P1().c == b"Z"

    def test_parse_returns_const(self):
        """parse：parse(b'Z').c == b'Z'。"""

        @dataclass
        class P1(StructMixin):
            c: bytes = field(Const(b"Z", Bytes(1)))

        assert P1.parse(b"Z").c == b"Z"

    def test_build_defaulted_uses_const(self):
        """build 缺省：P1().build() == b'Z'。"""

        @dataclass
        class P1(StructMixin):
            c: bytes = field(Const(b"Z", Bytes(1)))

        assert P1().build() == b"Z"

    def test_build_explicit_equal_accepted(self):
        """build 显式等值：P1(c=b'Z').build() == b'Z'。"""

        @dataclass
        class P1(StructMixin):
            c: bytes = field(Const(b"Z", Bytes(1)))

        assert P1(c=b"Z").build() == b"Z"

    def test_build_explicit_unequal_const_error(self):
        """build 显式不等值：P1(c=b'X').build() → ConstError。"""

        @dataclass
        class P1(StructMixin):
            c: bytes = field(Const(b"Z", Bytes(1)))

        with pytest.raises(ConstError):
            P1(c=b"X").build()

    def test_build_explicit_none_uses_const(self):
        """build 显式 None：P1(c=None).build() == b'Z'。"""

        @dataclass
        class P1(StructMixin):
            c: bytes = field(Const(b"Z", Bytes(1)))

        assert P1(c=None).build() == b"Z"


# ---------------------------------------------------------------------------
# Default × field：常量 value / 表达式 value / 表达式 value 被表达式消费
# ---------------------------------------------------------------------------


class TestDefaultConstantSixTimings:
    """Default(Byte, 0) × field：常量默认值。

    语义：常量折叠——实例化携带 0；parse 转发 inner 值；build 缺省/None
    求值得 0；显式值优先；resolve 产**有效值**写 context（后继表达式读到
    节点实际使用的值而非 None）。
    """

    def test_instantiation_carries_folded_constant(self):
        """实例化：D0().d == 0（常量折叠为默认值）。"""

        @dataclass
        class D0(StructMixin):
            d: int = field(Default(Byte, 0))

        assert D0().d == 0

    def test_parse_forwards_inner(self):
        """parse 转发：parse(b'\\x07').d == 7（不使用默认值）。"""

        @dataclass
        class D0(StructMixin):
            d: int = field(Default(Byte, 0))

        assert D0.parse(b"\x07").d == 7

    def test_build_defaulted_evaluates(self):
        """build 缺省：D0().build() == b'\\x00'。"""

        @dataclass
        class D0(StructMixin):
            d: int = field(Default(Byte, 0))

        assert D0().build() == b"\x00"

    def test_build_explicit_overrides(self):
        """build 显式：D0(d=9).build() == b'\\x09'（显式值优先）。"""

        @dataclass
        class D0(StructMixin):
            d: int = field(Default(Byte, 0))

        assert D0(d=9).build() == b"\x09"

    def test_build_explicit_none_uses_default(self):
        """build 显式 None：D0(d=None).build() == b'\\x00'（None≡缺省）。"""

        @dataclass
        class D0(StructMixin):
            d: int = field(Default(Byte, 0))

        assert D0(d=None).build() == b"\x00"

    def test_defaulted_effective_value_visible_to_consumer(self):
        """context 有效值：d 缺省=0 时后继 Bytes(d+1) 读到 0（非 None）。

        resolve 把 Default 的有效值（0）写入 context，后继长度表达式读到
        正确类型与数值。
        """

        @dataclass
        class DC(StructMixin):
            x: int = field(Byte)
            d: int = field(Default(Byte, 0))
            y: bytes = field(Bytes(d + 1))

        # d resolve 0 → context 写 0 → y = Bytes(1) 读 1 字节
        assert DC(x=1, y=b"\xFF").build() == b"\x01\x00\xff"


class TestDefaultExprTimings:
    """Default(Byte, x+1) × field：表达式默认值。

    语义：表达式无法静态求值→实例化 None；build 缺省按 context 求值；
    显式值优先；显式 None 重求值；parse 转发 inner。
    """

    def test_instantiation_none(self):
        """实例化：D1(x=4).d is None（表达式值不可静态求值）。"""

        @dataclass
        class D1(StructMixin):
            x: int = field(Byte)
            d: int = field(Default(Byte, x + 1))

        assert D1(x=4).d is None

    def test_build_defaulted_evaluates(self):
        """build 缺省：D1(x=4).build() == b'\\x04\\x05'（求值 x+1）。"""

        @dataclass
        class D1(StructMixin):
            x: int = field(Byte)
            d: int = field(Default(Byte, x + 1))

        assert D1(x=4).build() == b"\x04\x05"

    def test_build_explicit_overrides(self):
        """build 显式：D1(x=4, d=9).build() == b'\\x04\\x09'。"""

        @dataclass
        class D1(StructMixin):
            x: int = field(Byte)
            d: int = field(Default(Byte, x + 1))

        assert D1(x=4, d=9).build() == b"\x04\x09"

    def test_parse_forwards_inner(self):
        """parse 转发：parse(b'\\x04\\t').d == 9（数据优先于默认值）。"""

        @dataclass
        class D1(StructMixin):
            x: int = field(Byte)
            d: int = field(Default(Byte, x + 1))

        assert D1.parse(b"\x04\x09").d == 9

    def test_explicit_none_re_evaluates(self):
        """build 显式 None：D1(x=4, d=None).build() == b'\\x04\\x05'。"""

        @dataclass
        class D1(StructMixin):
            x: int = field(Byte)
            d: int = field(Default(Byte, x + 1))

        assert D1(x=4, d=None).build() == b"\x04\x05"

    def test_expr_effective_value_visible_to_computed(self):
        """context 有效值：d 求值 5 时后继 Computed 读到 5。

        Default 的 build 有效值（求值结果或显式值）写入 context，
        后继 Computed 消费者读到正确值。
        """

        @dataclass
        class DE(StructMixin):
            x: int = field(Byte)
            d: int = field(Default(Byte, x + 1))
            c: int = rfield(Computed(d * 2))

        assert DE(x=4).build() == b"\x04\x05"  # Computed no-op 不写字节
        assert DE.parse(b"\x04\x05").c == 10


class TestDefaultExprConsumedByExpr:
    """x:Byte; d:Default(Byte,x+1); y:Bytes(d+1)：表达式默认值被表达式消费。

    语义：d 的 build 有效值（缺省求值或显式值）统一经 resolve 写 context，
    嵌套消费者 y 读到有效值（而非原始 None）。
    """

    def test_instantiation_optional(self):
        """实例化：D9(x=4) 可省 d/y（表达式派生字段→Optional）。"""

        @dataclass
        class D9(StructMixin):
            x: int = field(Byte)
            d: int = field(Default(Byte, x + 1))
            y: bytes = field(Bytes(d + 1))

        inst = D9(x=4)
        assert inst.x == 4
        assert inst.d is None

    def test_build_defaulted_effective_value_flows(self):
        """build 缺省：d 求值 5 → context 5 → y 读 6 字节。

        D9(x=4, y=b'abcdef').build() == b'\\x04\\x05abcdef'。
        """

        @dataclass
        class D9(StructMixin):
            x: int = field(Byte)
            d: int = field(Default(Byte, x + 1))
            y: bytes = field(Bytes(d + 1))

        assert D9(x=4, y=b"abcdef").build() == b"\x04\x05abcdef"

    def test_build_explicit_effective_value_flows(self):
        """build 显式：d=9 → context 9 → y 读 10 字节。"""

        @dataclass
        class D9(StructMixin):
            x: int = field(Byte)
            d: int = field(Default(Byte, x + 1))
            y: bytes = field(Bytes(d + 1))

        y10 = bytes(range(10))
        assert D9(x=4, d=9, y=y10).build() == b"\x04\x09" + y10

# ---------------------------------------------------------------------------
# Rebuild × rfield（RO）/ × field（RW）
# ---------------------------------------------------------------------------


class TestRebuildRfield:
    """Rebuild(Byte, n*2) × rfield：build 重算，实例值不参与。

    语义：RO 字段不入 __init__（既有语义）；build 忽略存储值重算表达式；
    parse 存 inner 值；显式传 RO 字段 → TypeError（init=False 既有语义）。
    """

    def test_instantiation_without_r(self):
        """实例化：R(n=3) 可省 r（rfield init=False）。"""

        @dataclass
        class R(StructMixin):
            n: int = field(Byte)
            r: int = rfield(Rebuild(Byte, n * 2))

        inst = R(n=3)
        assert inst.n == 3

    def test_build_recomputes(self):
        """build 缺省：R(n=3).build() == b'\\x03\\x06'（求值 n*2）。"""

        @dataclass
        class R(StructMixin):
            n: int = field(Byte)
            r: int = rfield(Rebuild(Byte, n * 2))

        assert R(n=3).build() == b"\x03\x06"

    def test_parse_stored_value_ignored_on_rebuild(self):
        """parse：parse(b'\\x03\\xff').r == 255；rebuild 忽略 255 重算。"""

        @dataclass
        class R(StructMixin):
            n: int = field(Byte)
            r: int = rfield(Rebuild(Byte, n * 2))

        parsed = R.parse(b"\x03\xff")
        assert parsed.n == 3
        assert parsed.r == 255
        assert parsed.build() == b"\x03\x06"

    def test_explicit_r_raises_typeerror(self):
        """显式传 r → TypeError（rfield init=False 既有语义）。"""

        @dataclass
        class R(StructMixin):
            n: int = field(Byte)
            r: int = rfield(Rebuild(Byte, n * 2))

        with pytest.raises(TypeError):
            R(n=3, r=99)


class TestRebuildField:
    """Rebuild(Byte, n*2) × field（RW）：实例值恒被忽略、重算。

    语义：RW 面实例化可省（build 值不由实例决定）；显式值与 None 同样
    被忽略（重算）；parse 存 inner 值。
    """

    def test_instantiation_optional(self):
        """实例化：R2(n=3) 可省 r（值来源=表达式重算→Optional）。"""

        @dataclass
        class R2(StructMixin):
            n: int = field(Byte)
            r: int = field(Rebuild(Byte, n * 2))

        inst = R2(n=3)
        assert inst.r is None

    def test_build_ignores_explicit_value(self):
        """build：R2(n=3, r=99).build() == b'\\x03\\x06'（忽略 99 重算）。"""

        @dataclass
        class R2(StructMixin):
            n: int = field(Byte)
            r: int = field(Rebuild(Byte, n * 2))

        assert R2(n=3, r=99).build() == b"\x03\x06"

    def test_build_defaulted_recomputes(self):
        """build 缺省：R2(n=3).build() == b'\\x03\\x06'。"""

        @dataclass
        class R2(StructMixin):
            n: int = field(Byte)
            r: int = field(Rebuild(Byte, n * 2))

        assert R2(n=3).build() == b"\x03\x06"

    def test_parse_stores_inner_value(self):
        """parse：parse(b'\\x03\\xff').r == 255（RW 存 inner 值）。"""

        @dataclass
        class R2(StructMixin):
            n: int = field(Byte)
            r: int = field(Rebuild(Byte, n * 2))

        assert R2.parse(b"\x03\xff").r == 255


# ---------------------------------------------------------------------------
# Computed × rfield（表达式）/ × field（常量折叠）
# ---------------------------------------------------------------------------


class TestComputedRfield:
    """Computed(x*2) × rfield：表达式计算值。

    语义：parse 求值存值；build no-op（不写不校验）；RO 不入 __init__。
    """

    def test_instantiation_c_not_in_init(self):
        """实例化：D(x=5) 可省 c；c 属性为 None（init=False default=None）。"""

        @dataclass
        class D(StructMixin):
            x: int = field(Byte)
            c: int = rfield(Computed(x * 2))

        inst = D(x=5)
        assert inst.c is None

    def test_build_noop(self):
        """build：D(x=5).build() == b'\\x05'（Computed 不写字节）。"""

        @dataclass
        class D(StructMixin):
            x: int = field(Byte)
            c: int = rfield(Computed(x * 2))

        assert D(x=5).build() == b"\x05"

    def test_parse_evaluates(self):
        """parse：parse(b'\\x05').c == 10（表达式求值存值）。"""

        @dataclass
        class D(StructMixin):
            x: int = field(Byte)
            c: int = rfield(Computed(x * 2))

        parsed = D.parse(b"\x05")
        assert parsed.x == 5
        assert parsed.c == 10

    def test_roundtrip_bytes_stable(self):
        """round-trip：parse → build 字节不变（Computed no-op）。"""

        @dataclass
        class D(StructMixin):
            x: int = field(Byte)
            c: int = rfield(Computed(x * 2))

        parsed = D.parse(b"\x05")
        assert parsed.build() == b"\x05"


class TestComputedConstField:
    """Computed(42) × field：常量折叠。

    语义：纯常量表达式折叠为字段默认值（实例化即携带 42）；build no-op；
    parse 求值返回常量；显式实例值被忽略（无字节语义）。
    """

    def test_instantiation_folded_constant(self):
        """实例化：DC().c == 42（常量折叠为默认值）。"""

        @dataclass
        class DC(StructMixin):
            c: int = field(Computed(42))

        assert DC().c == 42

    def test_build_noop(self):
        """build：DC().build() == b''（no-op 不写不校验）。"""

        @dataclass
        class DC(StructMixin):
            c: int = field(Computed(42))

        assert DC().build() == b""

    def test_parse_returns_constant(self):
        """parse：DC.parse(b'').c == 42。"""

        @dataclass
        class DC(StructMixin):
            c: int = field(Computed(42))

        assert DC.parse(b"").c == 42

    def test_build_ignores_explicit_value(self):
        """build 显式：DC(c=99).build() == b''（忽略，no-op）。"""

        @dataclass
        class DC(StructMixin):
            c: int = field(Computed(42))

        assert DC(c=99).build() == b""


# ---------------------------------------------------------------------------
# Padding × wfield（WO）/ × field（RW）
# ---------------------------------------------------------------------------


class TestPaddingWfield:
    """Padding(2) × wfield（WO）：build 写 pattern，parse 消费不存。

    语义：WO 字段 parse 消费字节但实例不携带值（p 为 None）；build 缺省/
    显式均写 pattern 字节（实例值不参与编码）。
    """

    def test_instantiation_optional(self):
        """实例化：E(x=1, y=2) 可省 p（填充值→Optional）。"""

        @dataclass
        class E(StructMixin):
            x: int = field(Byte)
            p: bytes = wfield(Padding(2))
            y: int = field(Byte)

        inst = E(x=1, y=2)
        assert inst.p is None

    def test_build_writes_pattern(self):
        """build 缺省：E(x=1, y=2).build() == b'\\x01\\x00\\x00\\x02'。"""

        @dataclass
        class E(StructMixin):
            x: int = field(Byte)
            p: bytes = wfield(Padding(2))
            y: int = field(Byte)

        assert E(x=1, y=2).build() == b"\x01\x00\x00\x02"

    def test_parse_consumes_not_stored(self):
        """parse：p 不携带解析值（WO 消费字节，实例值为 None）。"""

        @dataclass
        class E(StructMixin):
            x: int = field(Byte)
            p: bytes = wfield(Padding(2))
            y: int = field(Byte)

        parsed = E.parse(b"\x01\x00\x00\x02")
        assert (parsed.x, parsed.y) == (1, 2)
        assert parsed.p is None

    def test_build_ignores_explicit_value(self):
        """build 显式：p 值被忽略仍写 pattern（实例值不参与编码）。"""

        @dataclass
        class E(StructMixin):
            x: int = field(Byte)
            p: bytes = wfield(Padding(2))
            y: int = field(Byte)

        assert E(x=1, y=2, p=b"ZZ").build() == b"\x01\x00\x00\x02"


class TestPaddingField:
    """Padding(2) × field（RW）：parse 存 None（既有已发布行为）。

    语义：RW 填充字段 parse 后实例值为 None（属性存在）；build 显式值
    忽略（写 pattern）；实例化 None。
    """

    def test_instantiation_none(self):
        """实例化：EP(x=1, y=2).p is None。"""

        @dataclass
        class EP(StructMixin):
            x: int = field(Byte)
            p: bytes = field(Padding(2))
            y: int = field(Byte)

        assert EP(x=1, y=2).p is None

    def test_build_defaulted_writes_pattern(self):
        """build 缺省：EP(x=1, y=2).build() == b'\\x01\\x00\\x00\\x02'。"""

        @dataclass
        class EP(StructMixin):
            x: int = field(Byte)
            p: bytes = field(Padding(2))
            y: int = field(Byte)

        assert EP(x=1, y=2).build() == b"\x01\x00\x00\x02"

    def test_parse_stores_none(self):
        """parse：parse 后 p is None 且属性存在（RW 存 None 行为）。"""

        @dataclass
        class EP(StructMixin):
            x: int = field(Byte)
            p: bytes = field(Padding(2))
            y: int = field(Byte)

        parsed = EP.parse(b"\x01\x00\x00\x02")
        assert hasattr(parsed, "p")
        assert parsed.p is None

    def test_build_ignores_explicit_value(self):
        """build 显式：EP(x=1, y=2, p=b'ZZ') 仍写 pattern。"""

        @dataclass
        class EP(StructMixin):
            x: int = field(Byte)
            p: bytes = field(Padding(2))
            y: int = field(Byte)

        assert EP(x=1, y=2, p=b"ZZ").build() == b"\x01\x00\x00\x02"

# ---------------------------------------------------------------------------
# Bytes(x+1) 表达式族：顶层 / Prefixed×1 / Prefixed×2 / Switch 内 / 消费者面
# ---------------------------------------------------------------------------


class TestTopLevelExprField:
    """Bytes(x+1) × field 顶层：表达式派生字段的实例化与缺值时机。

    语义：表达式派生字段实例化可省（值由前序字段推导，错误时机后移到
    build 值使用点）；build 缺值报 FieldValueMissingError；Const 前序字段
    缺省时由常量补值流入表达式。
    """

    def test_instantiation_optional(self):
        """实例化：W(x=1) 可省 y（表达式派生→Optional）。"""

        @dataclass
        class W(StructMixin):
            x: int = field(Int8ub)
            y: bytes = field(Bytes(x + 1))

        inst = W(x=1)
        assert inst.y is None

    def test_build_missing_y_raises_field_value_missing(self):
        """build 缺 y：W(x=1).build() → FieldValueMissingError（时机=build）。"""

        @dataclass
        class W(StructMixin):
            x: int = field(Int8ub)
            y: bytes = field(Bytes(x + 1))

        with pytest.raises(FieldValueMissingError):
            W(x=1).build()

    def test_build_with_value(self):
        """build：W(x=1, y=b'ab').build() == b'\\x01ab'。"""

        @dataclass
        class W(StructMixin):
            x: int = field(Int8ub)
            y: bytes = field(Bytes(x + 1))

        assert W(x=1, y=b"ab").build() == b"\x01ab"

    def test_const_x_defaulted_flows_to_expr(self):
        """Const x 补值：W2(y=b'ab').build() == b'\\x01ab'（常量流入表达式）。"""

        @dataclass
        class W2(StructMixin):
            x: int = field(Const(1, Byte))
            y: bytes = field(Bytes(x + 1))

        assert W2(y=b"ab").build() == b"\x01ab"


class TestPrefixedExprField:
    """Prefixed(Int8ub, Bytes(x+1)) × field：嵌套×1 的长度表达式。

    语义：长度字段经 Prefixed 包装后表达式仍读宿主前序字段的有效值；
    parse/build/round-trip 对称。
    """

    def test_parse(self):
        """parse：M.parse(b'\\x01\\x02ab').p == b'ab'。"""

        @dataclass
        class M(StructMixin):
            x: int = field(Int8ub)
            p: bytes = field(Prefixed(Int8ub, Bytes(x + 1)))

        parsed = M.parse(b"\x01\x02ab")
        assert parsed.x == 1
        assert parsed.p == b"ab"

    def test_build(self):
        """build：M(x=1, p=b'ab').build() == b'\\x01\\x02ab'。"""

        @dataclass
        class M(StructMixin):
            x: int = field(Int8ub)
            p: bytes = field(Prefixed(Int8ub, Bytes(x + 1)))

        assert M(x=1, p=b"ab").build() == b"\x01\x02ab"

    def test_roundtrip(self):
        """round-trip：parse → build 字节不变。"""

        @dataclass
        class M(StructMixin):
            x: int = field(Int8ub)
            p: bytes = field(Prefixed(Int8ub, Bytes(x + 1)))

        parsed = M.parse(b"\x01\x02ab")
        assert parsed.build() == b"\x01\x02ab"

    def test_const_x_combination(self):
        """Const x：MC(p=b'ab').build() == b'\\x01\\x02ab'（常量补值流入）。"""

        @dataclass
        class MC(StructMixin):
            x: int = field(Const(1, Int8ub))
            p: bytes = field(Prefixed(Int8ub, Bytes(x + 1)))

        assert MC(p=b"ab").build() == b"\x01\x02ab"


class TestDeepNestedExprField:
    """Prefixed×2 / Switch 内：深层嵌套的表达式长度。

    语义：双层 Prefixed 内表达式读宿主字段有效值；Switch case 内
    Bytes(x+1) 同样读宿主字段（case 选用不含同级同名表达式参数的分支，
    避开同字段内同名表达式冲突的编译期限制）。
    """

    def test_double_prefixed_parse(self):
        """parse：M2.parse(b'\\x01\\x03\\x02ab').p == b'ab'。"""

        @dataclass
        class M2(StructMixin):
            x: int = field(Int8ub)
            p: bytes = field(Prefixed(Int8ub, Prefixed(Int8ub, Bytes(x + 1))))

        parsed = M2.parse(b"\x01\x03\x02ab")
        assert parsed.p == b"ab"

    def test_double_prefixed_roundtrip(self):
        """round-trip：build → parse 字节/值稳定。"""

        @dataclass
        class M2(StructMixin):
            x: int = field(Int8ub)
            p: bytes = field(Prefixed(Int8ub, Prefixed(Int8ub, Bytes(x + 1))))

        assert M2(x=1, p=b"ab").build() == b"\x01\x03\x02ab"
        assert M2.parse(b"\x01\x03\x02ab").build() == b"\x01\x03\x02ab"

    def test_switch_case_expr_bytes(self):
        """Switch 内 Bytes(x+1)：key=1 → Bytes(2) 分支 round-trip。"""

        @dataclass
        class MS(StructMixin):
            x: int = field(Int8ub)
            s: Any = field(Switch(x, {1: Bytes(x + 1), 2: Byte}))

        assert MS.parse(b"\x01AB").s == b"AB"
        assert MS(x=1, s=b"AB").build() == b"\x01AB"

    def test_prefixed_switch_expr_key(self):
        """Prefixed × Switch(x+1)：key 表达式 round-trip。"""

        @dataclass
        class MSP(StructMixin):
            x: int = field(Int8ub)
            s: Any = field(Prefixed(Int8ub, Switch(x + 1, {1: Byte, 2: Bytes(2)})))

        parsed = MSP.parse(b"\x01\x02AB")
        assert parsed.s == b"AB"
        assert MSP(x=1, s=b"AB").build() == b"\x01\x02AB"


class TestExprConsumers:
    """表达式消费者面：Array/If/Computed/PrefixedArray 读 context 有效值。

    语义：四类消费者在 build 期读前序字段的有效值，round-trip 对称。
    """

    def test_array_count_expr(self):
        """Array(x+1, Byte)：count 表达式 round-trip。"""

        @dataclass
        class MA(StructMixin):
            x: int = field(Int8ub)
            items: list = field(Array(x + 1, Byte))

        parsed = MA.parse(b"\x01\x01\x02")
        assert parsed.items == [1, 2]
        assert MA(x=1, items=[1, 2]).build() == b"\x01\x01\x02"

    def test_if_cond_expr(self):
        """If(x>0, Byte)：cond 表达式（真分支需值 + 假分支 None 透传）。"""

        @dataclass
        class MI(StructMixin):
            x: int = field(Int8ub)
            c: Any = field(If(x > 0, Byte))

        parsed = MI.parse(b"\x01\x7f")
        assert parsed.c == 0x7F
        assert MI(x=1, c=0x7F).build() == b"\x01\x7f"
        # cond=0 → Pass：parse 0 数据字节（c=None）
        parsed0 = MI.parse(b"\x00")
        assert parsed0.c is None
        # 条件根 None 透传（分支自治）：cond=0 → Pass 分支不写字节；
        # parse 产物回 build（互为逆运算）
        assert MI(x=0, c=None).build() == b"\x00"
        assert parsed0.build() == b"\x00"

    def test_computed_expr_consumer(self):
        """Computed(x+1)：消费者 no-op + parse 求值。"""

        @dataclass
        class MC2(StructMixin):
            x: int = field(Int8ub)
            c: int = rfield(Computed(x + 1))

        assert MC2(x=4).build() == b"\x04"
        assert MC2.parse(b"\x04").c == 5

    def test_prefixed_array(self):
        """PrefixedArray(Int8ub, Byte)：round-trip。"""

        @dataclass
        class MPA(StructMixin):
            items: list = field(PrefixedArray(Int8ub, Byte))

        parsed = MPA.parse(b"\x02\x01\x02")
        assert parsed.items == [1, 2]
        assert MPA(items=[1, 2]).build() == b"\x02\x01\x02"


# ---------------------------------------------------------------------------
# 用户场景验收回归
# ---------------------------------------------------------------------------


class TestUserScenarioRegressions:
    """用户场景回归防线：常量补值/ctx 有效值/缺值时机/RO 常量字段。

    语义：既有交付行为的回归锁定（Const 实例化值、常量+表达式组合、
    wfield 表达式缺值报错、rfield(Const) 组合）。
    """

    def test_const_instantiation(self):
        """A() 实例化 x == 1（Const 隐式默认值）。"""

        @dataclass
        class A(StructMixin):
            x: int = field(Const(1, Byte))

        assert A().x == 1

    def test_const_plus_expr_build(self):
        """A(y=b'ab').build() == b'\\x01ab'（Const 缺省补值流入表达式）。"""

        @dataclass
        class A(StructMixin):
            x: int = field(Const(1, Byte))
            y: bytes = field(Bytes(x + 1))

        assert A(y=b"ab").build() == b"\x01ab"

    def test_const_plus_expr_parse_roundtrip(self):
        """A.parse(b'\\x01\\x02ab') round-trip（尾部字节不消费，Bytes 定长）。"""

        @dataclass
        class A(StructMixin):
            x: int = field(Const(1, Byte))
            y: bytes = field(Bytes(x + 1))

        parsed = A.parse(b"\x01\x02ab")
        assert (parsed.x, parsed.y) == (1, b"\x02a")
        assert parsed.build() == b"\x01\x02a"

    def test_const_int_instantiation(self):
        """C2().c == 5（Const int 形态）。"""

        @dataclass
        class C2(StructMixin):
            c: int = field(Const(0x05, Int8ub))

        assert C2().c == 5

    def test_const_bytes_instantiation(self):
        """P1(tag=1).c == b'Z'（Const bytes 形态）。"""

        @dataclass
        class P1(StructMixin):
            tag: int = field(Int8ub)
            c: bytes = field(Const(b"Z", Bytes(1)))

        assert P1(tag=1).c == b"Z"

    def test_wfield_expr_build_missing_raises(self):
        """W(x=1) 实例化 OK + build 缺 y → FieldValueMissingError。"""

        @dataclass
        class W(StructMixin):
            x: int = field(Int8ub)
            y: bytes = wfield(Bytes(x + 1))

        inst = W(x=1)
        with pytest.raises(FieldValueMissingError):
            inst.build()

    def test_wfield_expr_build_with_value(self):
        """W(x=1, y=b'ab').build() == b'\\x01ab'。"""

        @dataclass
        class W(StructMixin):
            x: int = field(Int8ub)
            y: bytes = wfield(Bytes(x + 1))

        assert W(x=1, y=b"ab").build() == b"\x01ab"

    def test_rfield_const_expr_build(self):
        """rfield(Const) + Bytes(x+1) build 正常（RO 常量字段补值）。"""

        @dataclass
        class A2(StructMixin):
            x: int = rfield(Const(1, Byte))
            y: bytes = field(Bytes(x + 1))

        assert A2(y=b"ab").build() == b"\x01ab"

# ---------------------------------------------------------------------------
# 边界：context= 注入 / 可变 Const / 显式 default 优先 / BitStruct Padding
# ---------------------------------------------------------------------------


class TestBoundaryConditions:
    """边界条件：注入拒绝、可变默认值防护、显式 default 优先、bit 域对称。

    语义：context= 注入在编译期显式拒绝（fail fast）；Const 可变对象不作为
    共享默认值（实例化 None）；用户显式 default 永远优先于框架派生；
    BitStruct 域内 Padding 分类与字节域一致。
    """

    def test_context_injection_compile_rejected(self):
        """field(..., context={...}) → 编译期 CompilationError。

        注入映射在当前版本不提供（消除静默忽略），编译期（类定义阶段）
        显式报错。
        """
        with pytest.raises(CompilationError):

            @dataclass
            class CI(StructMixin):
                x: int = field(Int8ub, context={"k": 1})

    def test_const_mutable_value_instantiation_none(self):
        """Const(list) 可变对象 → 实例化 None（防共享可变默认值）。"""

        @dataclass
        class CM(StructMixin):
            c: Any = field(Const([1, 2], Bytes(2)))

        assert CM().c is None

    def test_explicit_default_precedence(self):
        """显式 default= 永远优先于框架派生。"""

        @dataclass
        class ED(StructMixin):
            x: int = field(Const(1, Byte), default=7)
            d: int = field(Default(Byte, 9), default=5)

        assert ED().x == 7
        assert ED().d == 5

    def test_bitstruct_padding_symmetry(self):
        """BitStruct 域内 Padding：parse 存 None / build 写 pattern 对称。"""

        from neoconstruct import Bit, BitsInteger, BitStructMixin

        @dataclass
        class BP(BitStructMixin):
            a: int = field(Bit())
            pad: Any = field(Padding(2))
            b: int = field(BitsInteger(5))

        # bits：a=1 | pad=00 | b=10101 → 0b1_00_10101 = 0x95
        assert BP(a=1, b=0x15).build() == bytes([0b1_00_10101])
        parsed = BP.parse(bytes([0b1_00_10101]))
        assert (parsed.a, parsed.b) == (1, 0x15)
        assert parsed.pad is None


# ---------------------------------------------------------------------------
# Peek × field：哑值可省 / parse 存有效值 / build no-op / 值透传链
# ---------------------------------------------------------------------------


class TestPeekField:
    """Peek(Int8ub) × field：控制类字段的值语义。

    语义：实例化可省（build 值不由实例决定）；parse 预读存有效值（流回退，
    后继字段重读同位置）；build 缺省/显式均 no-op（实例值不参与编码）；
    parse 产出的有效值经实例透传回 context——peek→派生表达式→分派链
    round-trip 保持；从零 build 的哑值 None 流入后继消费者时显式报错。
    """

    def test_instantiation_optional(self):
        """实例化：PK(x=9) 可省 p（控制类→哑值 Optional）。"""

        @dataclass
        class PK(StructMixin):
            p: int = field(Peek(Byte))
            x: int = field(Byte)

        inst = PK(x=9)
        assert inst.p is None

    def test_parse_stores_effective_value(self):
        """parse 存有效值：parse(b'\\x05\\x99') → p==5；Peek 不消费流。"""

        @dataclass
        class PK(StructMixin):
            p: int = field(Peek(Byte))
            x: int = field(Byte)

        parsed = PK.parse(b"\x05\x99")
        assert parsed.p == 5
        # Peek 后流回退：x 重读首字节 0x05（0x99 不消费）
        assert parsed.x == 5

    def test_build_defaulted_noop(self):
        """build 缺省 no-op：PK(x=9).build() == b'\\x09'（Peek 0 字节）。"""

        @dataclass
        class PK(StructMixin):
            p: int = field(Peek(Byte))
            x: int = field(Byte)

        assert PK(x=9).build() == b"\x09"

    def test_build_explicit_ignored(self):
        """build 显式忽略：PK(p=5, x=9).build() == b'\\x09'（字节同缺省）。"""

        @dataclass
        class PK(StructMixin):
            p: int = field(Peek(Byte))
            x: int = field(Byte)

        assert PK(p=5, x=9).build() == b"\x09"

    def test_peek_derive_dispatch_roundtrip(self):
        """peek→Computed→Switch 链 round-trip 全绿。

        parse 腿：peek 存有效值 → Computed 求值 → Switch 正确分派。
        build 腿：RW getattr 取到 parse 存的有效值 → context 透传有效值
        → Computed 求值成功 → 分派一致（parse 产出值的透传语义）。
        """

        @dataclass
        class Body1(StructMixin):
            v: int = field(Byte)
            w: int = field(Byte)

        @dataclass
        class Body3(StructMixin):
            v: int = field(Byte)
            w: int = field(Byte)
            u: int = field(Byte)

        @dataclass
        class IECFrame(StructMixin):
            cf1_peek: int = field(Peek(Byte))
            frame_type: int = rfield(
                Computed((cf1_peek & 1) * (1 + 2 * ((cf1_peek >> 1) & 1)))
            )
            body: Any = field(Switch(frame_type, {0: Byte, 1: Body1, 3: Body3}))

        # parse 腿全链：cf1=0x03 → frame_type=3 → Body3（3 字节，覆盖被
        # peek 的字节——流已回退）分派
        parsed = IECFrame.parse(b"\x03\x01\x02")
        assert parsed.cf1_peek == 3
        assert parsed.frame_type == 3
        assert (parsed.body.v, parsed.body.w, parsed.body.u) == (3, 1, 2)

        # build 腿：parse 存的有效值透传 → 分派一致 → round-trip
        # （Peek build no-op，字节由 body 分支完整重写）
        assert parsed.build() == b"\x03\x01\x02"

    def test_from_scratch_build_dummy_none_flowing_raises(self):
        """从零 build + 后继 Computed 引用 peek → 报错。

        缺省哑值 None 流入 context，Computed 消费 None → 表达式求值的
        类型错误（显式报错优于静默错值）。
        """

        @dataclass
        class PK6(StructMixin):
            p: int = field(Peek(Byte))
            c: int = rfield(Computed(p + 1))

        inst = PK6()
        with pytest.raises(ConstructError):
            inst.build()


# ---------------------------------------------------------------------------
# Seek × field：哑值可省 / parse 新位置 / build 移针 / 绝对定位 round-trip
# ---------------------------------------------------------------------------


class TestSeekField:
    """Seek × field：流定位控制字段的值语义。

    语义：实例化可省；parse 产出新流位置（存入实例）；build 移动流指针
    （实例值不参与）；绝对定位场景 round-trip。
    """

    def test_instantiation_optional(self):
        """实例化：SK(a=.., b=..) 可省 s（控制类→哑值 Optional）。"""

        @dataclass
        class SK(StructMixin):
            a: bytes = field(Bytes(2))
            s: int = field(Seek(1, 1))
            b: int = field(Byte)

        inst = SK(a=b"ab", b=9)
        assert inst.s is None

    def test_parse_returns_new_position(self):
        """parse 值=新流位置：s == 3（前移 1 后的位置）。"""

        @dataclass
        class SK2(StructMixin):
            a: bytes = field(Bytes(2))
            s: int = field(Seek(1, 1))
            b: int = field(Byte)

        parsed = SK2.parse(b"abcd")
        assert parsed.s == 3
        assert parsed.b == ord("d")

    def test_build_seeks_writes_skip_pattern(self):
        """build 缺省 seek 移动：b'ab' + skip1 + \\x09 == b'ab\\x00\\t'。"""

        @dataclass
        class SK3(StructMixin):
            a: bytes = field(Bytes(2))
            s: int = field(Seek(1, 1))
            b: int = field(Byte)

        assert SK3(a=b"ab", b=9).build() == b"ab\x00\t"

    def test_seek_zero_absolute_roundtrip(self):
        """Seek(0, 0) 绝对定位：parse s==0，round-trip 字节稳定。"""

        @dataclass
        class SK4(StructMixin):
            s: int = field(Seek(0, 0))
            a: int = field(Byte)

        parsed = SK4.parse(b"\x05")
        assert parsed.s == 0
        assert parsed.a == 5
        assert SK4(a=5).build() == b"\x05"

# ---------------------------------------------------------------------------
# Checksum × field：哑值可省 / parse 校验存 digest / build 重算 / 失败报错
# ---------------------------------------------------------------------------


class TestChecksumField:
    """Checksum(Bytes(2), crc16) × field + Tell start/end：校验和值语义。

    语义：实例化可省（build 值=重算 digest）；parse 校验通过存 digest、
    不符报 ChecksumError；build 缺省/显式错误值均重算写入（实例值不参与
    编码）；start/end 引用 Tell 字段（build 期流位置经 context 提供）。
    """

    def test_instantiation_optional(self):
        """实例化：Pkt(addr=.., data=..) 可省 crc（控制类→哑值 Optional）。"""

        @dataclass
        class Pkt(StructMixin):
            body_start: int = rfield(Tell())
            addr: int = field(Byte)
            fc: int = field(Byte)
            data: bytes = field(Bytes(2))
            body_end: int = rfield(Tell())
            crc: bytes = field(
                Checksum(Bytes(2), _modbus_crc16, start=body_start, end=body_end)
            )

        inst = Pkt(addr=1, fc=2, data=b"\x03\x04")
        assert inst.crc is None

    def test_parse_verifies_and_stores_digest(self):
        """parse 校验通过 + crc == 重算 digest。"""

        @dataclass
        class Pkt(StructMixin):
            body_start: int = rfield(Tell())
            addr: int = field(Byte)
            fc: int = field(Byte)
            data: bytes = field(Bytes(2))
            body_end: int = rfield(Tell())
            crc: bytes = field(
                Checksum(Bytes(2), _modbus_crc16, start=body_start, end=body_end)
            )

        payload = bytes([0x01, 0x02, 0x03, 0x04])
        frame = payload + _modbus_crc16(payload)
        parsed = Pkt.parse(frame)
        assert parsed.crc == _modbus_crc16(payload)

    def test_build_defaulted_recomputes(self):
        """build 缺省重算：b'\\x01\\x02\\x81\\xe1'（crc16 校验和）。"""

        @dataclass
        class Pkt2(StructMixin):
            body_start: int = rfield(Tell())
            addr: int = field(Byte)
            fc: int = field(Byte)
            body_end: int = rfield(Tell())
            crc: bytes = field(
                Checksum(Bytes(2), _modbus_crc16, start=body_start, end=body_end)
            )

        assert Pkt2(addr=1, fc=2).build() == b"\x01\x02\x81\xe1"

    def test_build_explicit_wrong_value_ignored(self):
        """build 显式错误值忽略：仍写正确 digest（实例值不参与编码）。"""

        @dataclass
        class Pkt2(StructMixin):
            body_start: int = rfield(Tell())
            addr: int = field(Byte)
            fc: int = field(Byte)
            body_end: int = rfield(Tell())
            crc: bytes = field(
                Checksum(Bytes(2), _modbus_crc16, start=body_start, end=body_end)
            )

        assert Pkt2(addr=1, fc=2, crc=b"\x00\x00").build() == b"\x01\x02\x81\xe1"

    def test_parse_mismatch_raises_checksum_error(self):
        """parse 校验失败 → ChecksumError。"""

        @dataclass
        class Pkt2(StructMixin):
            body_start: int = rfield(Tell())
            addr: int = field(Byte)
            fc: int = field(Byte)
            body_end: int = rfield(Tell())
            crc: bytes = field(
                Checksum(Bytes(2), _modbus_crc16, start=body_start, end=body_end)
            )

        bad_frame = bytes([0x01, 0x02, 0x00, 0x00])
        with pytest.raises(ChecksumError):
            Pkt2.parse(bad_frame)


# ---------------------------------------------------------------------------
# 控制类构造器 × rfield（RO 面）：build 可用性锁定
# ---------------------------------------------------------------------------


class TestControlRfieldBuildUsable:
    """rfield(Peek/Seek/Checksum)：RO 面控制字段的 build 可用性。

    语义：值来源分类接管 build 分派后，控制类 RO 字段从实例透传缺省哑值
    → 节点按自身语义处理（Peek no-op / Seek 移针 / Checksum 重算），
    RO 面 build 不再因取值通路缺失而必错。
    """

    def test_rfield_peek_build_noop(self):
        """rfield(Peek)：build no-op，后继字段正常写字节。"""

        @dataclass
        class RP(StructMixin):
            p: int = rfield(Peek(Byte))
            x: int = field(Byte)

        assert RP(x=9).build() == b"\x09"
        assert RP.parse(b"\x09").build() == b"\x09"

    def test_rfield_seek_build_moves_stream(self):
        """rfield(Seek)：build 移动流指针（跳过字节由填充 pattern 补齐）。"""

        @dataclass
        class RS(StructMixin):
            a: bytes = field(Bytes(2))
            s: int = rfield(Seek(1, 1))
            b: int = field(Byte)

        assert RS(a=b"ab", b=9).build() == b"ab\x00\t"

    def test_rfield_checksum_roundtrip(self):
        """rfield(Checksum)：build 重算 digest + parse 校验 round-trip。"""

        @dataclass
        class RC(StructMixin):
            body_start: int = rfield(Tell())
            addr: int = field(Byte)
            fc: int = field(Byte)
            body_end: int = rfield(Tell())
            crc: bytes = rfield(
                Checksum(Bytes(2), _modbus_crc16, start=body_start, end=body_end)
            )

        built = RC(addr=1, fc=2).build()
        assert built == b"\x01\x02\x81\xe1"
        parsed = RC.parse(built)
        assert parsed.crc == b"\x81\xe1"
        assert parsed.build() == built


# ---------------------------------------------------------------------------
# 数据负例：长度声明与实际 payload 不符 → 显式 StreamError
# ---------------------------------------------------------------------------


class TestDataNegatives:
    """数据-schema 不匹配 → 显式 StreamError（非静默错读）。

    语义：长度声明超过剩余字节（顶层/Prefixed/嵌套 Prefixed）parse 显式
    报 StreamError，错误消息含期望/实际长度的关键语义词。
    """

    def test_bytes_length_overrun(self):
        """Bytes(n) 声明 5 实际剩 3 → StreamError（含 "expected 5"）。"""

        @dataclass
        class NB(StructMixin):
            n: int = field(Int8ub)
            data: bytes = field(Bytes(n))

        with pytest.raises(StreamError) as exc_info:
            NB.parse(b"\x05abc")
        assert "expected 5" in str(exc_info.value)

    def test_prefixed_length_mismatch(self):
        """Prefixed(Int8ub, GreedyBytes) 长度声明 5 实际 3 → StreamError。"""

        @dataclass
        class NP(StructMixin):
            p: bytes = field(Prefixed(Int8ub, GreedyBytes))

        with pytest.raises(StreamError):
            NP.parse(b"\x05abc")

    def test_nested_prefixed_inner_mismatch(self):
        """嵌套 Prefixed 内层需求不符 → StreamError。"""

        @dataclass
        class NP2(StructMixin):
            x: int = field(Int8ub)
            p: bytes = field(Prefixed(Int8ub, Prefixed(Int8ub, Bytes(x + 1))))

        # 内层声明 2 但仅剩 1 字节
        with pytest.raises(StreamError):
            NP2.parse(b"\x01\x02\x01a")


# ---------------------------------------------------------------------------
# 包装器根分类：条件根（If/Switch/Select）None 透传分支自治
# ---------------------------------------------------------------------------


class TestWrapperRootClassification:
    """field(If(cond, ...)) 条件根：None（含缺省）透传，分支自治。

    语义：条件根按根节点分类为 Conditional——resolve 对 None（含属性不存在）
    不报错，透传给节点 build。选中 Pass 分支 → 不写字节；选中哑值语义分支
    （Padding）→ 分支自身忽略值写 pattern；选中实值分支（Byte）而值为 None
    → 由分支节点自然报错（防"改了条件没给值"的不一致静默）；显式传值仍成功。
    """

    def test_if_padding_defaulted_build_writes_pattern(self):
        """field(If(flag>0, Padding(2))) 缺省 build → Padding 分支忽略值写 pattern。"""

        @dataclass
        class W1C(StructMixin):
            flag: int = field(Byte)
            p: Any = field(If(flag > 0, Padding(2)))

        assert W1C(flag=1).build() == b"\x01\x00\x00"

    def test_if_padding_explicit_none_build_writes_pattern(self):
        """显式 None ≡ 缺省（None ≡ 缺席，来源无关）→ 同样写 pattern。"""

        @dataclass
        class W1C(StructMixin):
            flag: int = field(Byte)
            p: Any = field(If(flag > 0, Padding(2)))

        assert W1C(flag=1, p=None).build() == b"\x01\x00\x00"

    def test_if_byte_defaulted_build_raises_branch_error(self):
        """field(If(flag>0, Byte)) 缺省 build → 实值分支收到 None 自然报错。"""

        @dataclass
        class W2C(StructMixin):
            flag: int = field(Byte)
            b: Any = field(If(flag > 0, Byte))

        # 分支节点（Byte）自然报错，非字段层 FieldValueMissingError
        with pytest.raises(FormatFieldError):
            W2C(flag=1).build()

    def test_if_byte_false_branch_defaulted_build_writes_nothing(self):
        """cond 假 → Pass 分支不写字节（缺省/None 均可 build）。"""

        @dataclass
        class W2C(StructMixin):
            flag: int = field(Byte)
            b: Any = field(If(flag > 0, Byte))

        assert W2C(flag=0).build() == b"\x00"
        assert W2C(flag=0, b=None).build() == b"\x00"

    def test_if_byte_explicit_value_builds(self):
        """field(If(flag>0, Byte)) 显式传值仍成功。"""

        @dataclass
        class W2C(StructMixin):
            flag: int = field(Byte)
            b: Any = field(If(flag > 0, Byte))

        assert W2C(flag=1, b=0x7F).build() == b"\x01\x7f"
