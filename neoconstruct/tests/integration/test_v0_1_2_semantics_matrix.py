"""v0.1.2 字段值语义框架矩阵测试（v0.1.2-4 核心交付）。

期望值全部从设计锚点表推导（docs/design/基础设施/v0.1.2-字段值语义框架.md
v2.1 §2 锚点表 + §8 矩阵定义 + experiments/v0.1.2-spike/anchor3.py 实测
A1-A4/S1-S2/C1-C3/W1-W2/N1-N3），**不从当前实现记录**。

维度（设计 §8）：
- D1 值语义构造器（Const/Default/Rebuild/Computed/Padding/Peek/Seek/Checksum/
  If 传播）× D2 模式(field/rfield/wfield) × D3 时机(实例化/parse/build 缺省/
  显式等值/显式不等值/显式 None) × D4 位置(顶层/Prefixed×1/Prefixed×2/
  Switch 内) × D5 表达式(字段引用/算术/常量) × D6 消费者(Bytes/Array/Switch/
  If/Computed/PrefixedArray) × D7 数据负例（长度声明 vs 实际不符）

族分布（99 用例 = 设计 96 + VET N-1 维度补充 3 例）：
- A1-12  Const × field（int/bytes 两形态 × 六时机）
- B1-12  Default × field（常量/表达式 value）
- B13-15 [VET N-1 补充] Default 表达式 value 被表达式消费（[推导] 标注）
- C1-8   Rebuild × rfield/field
- D1-8   Computed × rfield/field（表达式/常量折叠）
- E1-8   Padding × wfield/field
- F1-16  Bytes(x+1) 表达式族（顶层/Prefixed×1/Prefixed×2/Switch/消费者面）
- G1-8   B1-B3 验收回归（用户场景）
- H1-4   边界（B4 context=/可变 Const/显式 default 优先/BitStruct Padding）
- I1-6   Peek × field（B5 Control 哑值 + IEC104 真实链 I5 按 v2.1 REV-N1）
- J1-4   Seek × field
- K1-5   Checksum × field（modbus crc16 callable + Tell start/end）
- N1-3   数据负例（StreamError，B2 回归锁定）
- W1-2   If 传播差异锁定（§5.1c 有意差异：框架报错 vs 原版成功/KeyError）

已知有意差异（用户面，设计 §5.1c/§5.1d/§10）：
- W1：field(If(cond,Padding)) 缺省 build → FieldValueMissingError
  （原版成功写 pattern，v0.1.2 不实现包装器传播）
- W2：field(If(cond,Byte)) 缺省 build → FieldValueMissingError
  （原版 KeyError，语义等价必填、错误形态统一为框架异常）
- I5 build 腿：Peek→Computed→Switch 链 build 报错（原版 round-trip 成立，
  §5.1d Control ctx 写 None 差异，v0.1.3 前该形态限 parse-only）
- C4：rfield 显式传值 → TypeError（rfield init=False，neoconstruct 实例化
  时机特有；原版无实例化时机）
- Bytes 显式 None：框架 FieldValueMissingError（原版 TypeError，均拒绝）

dataclass 一律定义在测试函数体内（对齐 test_v0_1_2_regressions.py 惯例）。
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

# FieldValueMissingError 为 v0.1.2 框架新增异常类（设计 §5.5）。
# 红灯先行阶段（框架落地前）该异常不存在——_FVM 为 None 时相关用例
# 显式 pytest.fail（诚实红灯，不用宽泛 Exception 匹配掩盖）。
try:  # pragma: no cover - 转绿后走 except 之外的分支
    from neoconstruct import FieldValueMissingError as _FVM
except ImportError:  # noqa: SIM105 - 红灯阶段记录
    _FVM = None


def _require_fvm():
    """断言 FieldValueMissingError 已实现（红灯阶段诚实失败）。"""
    if _FVM is None:
        pytest.fail(
            "红灯：FieldValueMissingError 尚未实现（v0.1.2-4 框架 §5.5 前置）"
        )


def _modbus_crc16(data: bytes) -> bytes:
    """CRC-16/MODBUS（poly 0xA001 reversed, init 0xFFFF，LE 2 字节）。

    与 tests/system/_system_helpers.modbus_crc16 同算法；锚点 C1：
    modbus_crc16(b'\\x01\\x02') == b'\\x81\\xe1'。
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
# A 族：Const × field × 六时机（A1-6 int / A7-12 bytes）
# ---------------------------------------------------------------------------


class TestAConstInt:
    """A1-6：Const(0x05, Int8ub) × field 六时机。

    锚点（设计 §2 表行 1）：flagbuildnone=T；parse=5；build 缺省/None→b'\\x05'；
    显式等值→用；不等→ConstError。
    维度：D1=Const D2=field D3=六时机 D4=顶层 D7=正例。
    """

    def test_a1_instantiation_value(self):
        """A1 实例化：C2().c == 5（init_default=Value(5)）。"""

        @dataclass
        class C2(StructMixin):
            c: int = field(Const(0x05, Int8ub))

        assert C2().c == 0x05

    def test_a2_parse_returns_const(self):
        """A2 parse：parse(b'\\x05').c == 5（校验相等转发值）。"""

        @dataclass
        class C2(StructMixin):
            c: int = field(Const(0x05, Int8ub))

        parsed = C2.parse(b"\x05")
        assert parsed.c == 5

    def test_a3_build_defaulted(self):
        """A3 build 缺省：C2().build() == b'\\x05'（resolve None→补常量）。"""

        @dataclass
        class C2(StructMixin):
            c: int = field(Const(0x05, Int8ub))

        assert C2().build() == b"\x05"

    def test_a4_build_explicit_equal(self):
        """A4 build 显式等值：C2(c=5).build() == b'\\x05'。"""

        @dataclass
        class C2(StructMixin):
            c: int = field(Const(0x05, Int8ub))

        assert C2(c=5).build() == b"\x05"

    def test_a5_build_explicit_unequal_const_error(self):
        """A5 build 显式不等值：C2(c=9).build() → ConstError（节点校验保留）。"""

        @dataclass
        class C2(StructMixin):
            c: int = field(Const(0x05, Int8ub))

        with pytest.raises(ConstError):
            C2(c=9).build()

    def test_a6_build_explicit_none_uses_value(self):
        """A6 build 显式 None：C2(c=None).build() == b'\\x05'（锚点行 1 末列）。"""

        @dataclass
        class C2(StructMixin):
            c: int = field(Const(0x05, Int8ub))

        assert C2(c=None).build() == b"\x05"


class TestAConstBytes:
    """A7-12：Const(b'Z', Bytes(1)) × field 六时机（bytes 形态，B1 验收 P1）。

    维度：D1=Const(bytes) D2=field D3=六时机 D4=顶层。
    """

    def test_a7_instantiation_value(self):
        """A7 实例化：P1().c == b'Z'（bytes 不可变标量 → init_default=Value）。"""

        @dataclass
        class P1(StructMixin):
            c: bytes = field(Const(b"Z", Bytes(1)))

        assert P1().c == b"Z"

    def test_a8_parse_returns_const(self):
        """A8 parse：parse(b'Z').c == b'Z'。"""

        @dataclass
        class P1(StructMixin):
            c: bytes = field(Const(b"Z", Bytes(1)))

        assert P1.parse(b"Z").c == b"Z"

    def test_a9_build_defaulted(self):
        """A9 build 缺省：P1().build() == b'Z'。"""

        @dataclass
        class P1(StructMixin):
            c: bytes = field(Const(b"Z", Bytes(1)))

        assert P1().build() == b"Z"

    def test_a10_build_explicit_equal(self):
        """A10 build 显式等值：P1(c=b'Z').build() == b'Z'。"""

        @dataclass
        class P1(StructMixin):
            c: bytes = field(Const(b"Z", Bytes(1)))

        assert P1(c=b"Z").build() == b"Z"

    def test_a11_build_explicit_unequal_const_error(self):
        """A11 build 显式不等值：P1(c=b'X').build() → ConstError。"""

        @dataclass
        class P1(StructMixin):
            c: bytes = field(Const(b"Z", Bytes(1)))

        with pytest.raises(ConstError):
            P1(c=b"X").build()

    def test_a12_build_explicit_none_uses_value(self):
        """A12 build 显式 None：P1(c=None).build() == b'Z'。"""

        @dataclass
        class P1(StructMixin):
            c: bytes = field(Const(b"Z", Bytes(1)))

        assert P1(c=None).build() == b"Z"


# ---------------------------------------------------------------------------
# B 族：Default × field（B1-6 常量 value / B7-12 表达式 value）
# ---------------------------------------------------------------------------


class TestBDefaultConstant:
    """B1-6：Default(Byte, 0) × field。

    锚点（§2 表行 2）：实例化=0；parse 转发；build 缺省/None→b'\\x00'；显式→用之。
    维度：D1=Default(常量) D2=field D3=六时机 D4=顶层。
    """

    def test_b1_instantiation_value(self):
        """B1 实例化：D0().d == 0（ops==[Const(0)] 常量折叠）。"""

        @dataclass
        class D0(StructMixin):
            d: int = field(Default(Byte, 0))

        assert D0().d == 0

    def test_b2_parse_forwards_inner(self):
        """B2 parse 转发：parse(b'\\x07').d == 7（不用默认值）。"""

        @dataclass
        class D0(StructMixin):
            d: int = field(Default(Byte, 0))

        assert D0.parse(b"\x07").d == 7

    def test_b3_build_defaulted(self):
        """B3 build 缺省：D0().build() == b'\\x00'。"""

        @dataclass
        class D0(StructMixin):
            d: int = field(Default(Byte, 0))

        assert D0().build() == b"\x00"

    def test_b4_build_explicit(self):
        """B4 build 显式：D0(d=9).build() == b'\\x09'。"""

        @dataclass
        class D0(StructMixin):
            d: int = field(Default(Byte, 0))

        assert D0(d=9).build() == b"\x09"

    def test_b5_build_explicit_none_uses_default(self):
        """B5 build 显式 None：D0(d=None).build() == b'\\x00'（锚点行 2 末列）。"""

        @dataclass
        class D0(StructMixin):
            d: int = field(Default(Byte, 0))

        assert D0(d=None).build() == b"\x00"

    def test_b6_default_effective_value_visible_to_consumer(self):
        """B6 ctx 有效值：d 缺省=0 时后继 Bytes(d+1) 读到 0（非 None）。

        锚点 §2 机制 3：buildret Default=有效值 → 后继表达式读到节点实际使用的值。
        维度：D5=常量 D6=Bytes 消费者。
        """

        @dataclass
        class DC(StructMixin):
            x: int = field(Byte)
            d: int = field(Default(Byte, 0))
            y: bytes = field(Bytes(d + 1))

        # d resolve 0 → ctx 写 0 → y = Bytes(1) 读 1 字节
        assert DC(x=1, y=b"\xFF").build() == b"\x01\x00\xff"


class TestBDefaultExpr:
    """B7-12：Default(Byte, x+1) × field（表达式 value）。

    锚点（§2 表行 3）：实例化=None；build(x=4)→b'\\x04\\x05'；显式 9→b'\\x04\\t'。
    维度：D1=Default(表达式) D2=field D3=六时机 D5=算术。
    """

    def test_b7_instantiation_none(self):
        """B7 实例化：D1(x=4).d is None（表达式不可静态求值）。"""

        @dataclass
        class D1(StructMixin):
            x: int = field(Byte)
            d: int = field(Default(Byte, x + 1))

        assert D1(x=4).d is None

    def test_b8_build_defaulted_evaluates(self):
        """B8 build 缺省：D1(x=4).build() == b'\\x04\\x05'（求值 x+1）。"""

        @dataclass
        class D1(StructMixin):
            x: int = field(Byte)
            d: int = field(Default(Byte, x + 1))

        assert D1(x=4).build() == b"\x04\x05"

    def test_b9_build_explicit_overrides(self):
        """B9 build 显式：D1(x=4, d=9).build() == b'\\x04\\t'。"""

        @dataclass
        class D1(StructMixin):
            x: int = field(Byte)
            d: int = field(Default(Byte, x + 1))

        assert D1(x=4, d=9).build() == b"\x04\x09"

    def test_b10_parse_forwards_inner(self):
        """B10 parse 转发：parse(b'\\x04\\t').d == 9（显式字节优先于默认）。"""

        @dataclass
        class D1(StructMixin):
            x: int = field(Byte)
            d: int = field(Default(Byte, x + 1))

        assert D1.parse(b"\x04\x09").d == 9

    def test_b11_explicit_none_re_evaluates(self):
        """B11 显式 None：D1(x=4, d=None).build() == b'\\x04\\x05'（重求值）。"""

        @dataclass
        class D1(StructMixin):
            x: int = field(Byte)
            d: int = field(Default(Byte, x + 1))

        assert D1(x=4, d=None).build() == b"\x04\x05"

    def test_b12_expr_value_effective_in_ctx(self):
        """B12 ctx 有效值：d 求值 5 时后继 Computed 读到 5（B1e 根因验证）。

        锚点 §2 机制 3 + §5.2 ★写有效值：Default resolve 产有效值写 ctx。
        维度：D6=Computed 消费者。
        """

        @dataclass
        class DE(StructMixin):
            x: int = field(Byte)
            d: int = field(Default(Byte, x + 1))
            c: int = rfield(Computed(d * 2))

        assert DE(x=4).build() == b"\x04\x05"  # Computed no-op 不写字节
        assert DE.parse(b"\x04\x05").c == 10


class TestBDefaultExprConsumed:
    """B13-15：[VET N-1 补充] Default 表达式 value 被另一表达式消费。

    场景（v0.1.2-2 VET N-1）：``x:Byte; d:Default(Byte,x+1); y:Bytes(d+1)``。
    止血边界（表达式 value 无法静态求值 → ctx 写原始 None → wrong type）由
    框架 resolve 统一修复：d 的 build 有效值（求值或显式）写 ctx。
    期望按 ValueKind=Default 的 resolve 规则推导，标注 [推导]。
    维度：D1=Default(表达式) D5=字段引用嵌套 D6=Bytes 消费者。
    """

    def test_b13_instantiation_optional(self):
        """B13 [推导] 实例化：D9(x=4) 可省 d/y（expr_derived → Optional）。"""

        @dataclass
        class D9(StructMixin):
            x: int = field(Byte)
            d: int = field(Default(Byte, x + 1))
            y: bytes = field(Bytes(d + 1))

        inst = D9(x=4)
        assert inst.x == 4
        assert inst.d is None

    def test_b14_build_defaulted_effective_value_flows(self):
        """B14 [推导] build 缺省：d 求值 5 → ctx 5 → y 读 6 字节。

        D9(x=4, y=b'abcdef').build() == b'\\x04\\x05abcdef'。
        """

        @dataclass
        class D9(StructMixin):
            x: int = field(Byte)
            d: int = field(Default(Byte, x + 1))
            y: bytes = field(Bytes(d + 1))

        assert D9(x=4, y=b"abcdef").build() == b"\x04\x05abcdef"

    def test_b15_build_explicit_effective_value_flows(self):
        """B15 [推导] build 显式：d=9 → ctx 9 → y 读 10 字节。"""

        @dataclass
        class D9(StructMixin):
            x: int = field(Byte)
            d: int = field(Default(Byte, x + 1))
            y: bytes = field(Bytes(d + 1))

        y10 = bytes(range(10))
        assert D9(x=4, d=9, y=y10).build() == b"\x04\x09" + y10


# ---------------------------------------------------------------------------
# C 族：Rebuild × rfield（C1-4）/ × field（C5-8）
# ---------------------------------------------------------------------------


class TestCRebuildRfield:
    """C1-4：Rebuild × rfield（RO）。

    锚点（§2 表行 4）：实例化 r 不入 init；build(n=3)→b'\\x03\\x06'（重算）；
    parse→r=255；显式传 r→TypeError（有意差异：neoconstruct rfield init=False）。
    维度：D1=Rebuild D2=rfield D3=时机。
    """

    def test_c1_instantiation_without_r(self):
        """C1 实例化：R(n=3) 可省 r（rfield init=False）。"""

        @dataclass
        class R(StructMixin):
            n: int = field(Byte)
            r: int = rfield(Rebuild(Byte, n * 2))

        inst = R(n=3)
        assert inst.n == 3

    def test_c2_build_recomputes(self):
        """C2 build 缺省：R(n=3).build() == b'\\x03\\x06'（求值 n*2）。"""

        @dataclass
        class R(StructMixin):
            n: int = field(Byte)
            r: int = rfield(Rebuild(Byte, n * 2))

        assert R(n=3).build() == b"\x03\x06"

    def test_c3_parse_stored_value_ignored_on_rebuild(self):
        """C3 parse：parse(b'\\x03\\xff').r == 255；rebuild 忽略 255 重算 → b'\\x03\\x06'。"""

        @dataclass
        class R(StructMixin):
            n: int = field(Byte)
            r: int = rfield(Rebuild(Byte, n * 2))

        parsed = R.parse(b"\x03\xff")
        assert parsed.n == 3
        assert parsed.r == 255
        assert parsed.build() == b"\x03\x06"

    def test_c4_explicit_r_raises_typeerror(self):
        """C4 显式传 r → TypeError（有意差异：rfield 不在 __init__ 签名，记录）。"""

        @dataclass
        class R(StructMixin):
            n: int = field(Byte)
            r: int = rfield(Rebuild(Byte, n * 2))

        with pytest.raises(TypeError):
            R(n=3, r=99)


class TestCRebuildField:
    """C5-8：Rebuild × field（RW）。

    锚点（§2 表行 4）：r=99 仍写 b'\\x06'（忽略实例值重算）。
    维度：D1=Rebuild D2=field。
    """

    def test_c5_instantiation_optional(self):
        """C5 实例化：R2(n=3) 可省 r（ValueKind=Rebuild → Optional）。"""

        @dataclass
        class R2(StructMixin):
            n: int = field(Byte)
            r: int = field(Rebuild(Byte, n * 2))

        inst = R2(n=3)
        assert inst.r is None

    def test_c6_build_ignores_explicit_value(self):
        """C6 build：R2(n=3, r=99).build() == b'\\x03\\x06'（忽略 99 重算）。"""

        @dataclass
        class R2(StructMixin):
            n: int = field(Byte)
            r: int = field(Rebuild(Byte, n * 2))

        assert R2(n=3, r=99).build() == b"\x03\x06"

    def test_c7_build_defaulted_recomputes(self):
        """C7 build 缺省：R2(n=3).build() == b'\\x03\\x06'。"""

        @dataclass
        class R2(StructMixin):
            n: int = field(Byte)
            r: int = field(Rebuild(Byte, n * 2))

        assert R2(n=3).build() == b"\x03\x06"

    def test_c8_parse_stores_inner_value(self):
        """C8 parse：parse(b'\\x03\\xff').r == 255（RW 存 inner 值）。"""

        @dataclass
        class R2(StructMixin):
            n: int = field(Byte)
            r: int = field(Rebuild(Byte, n * 2))

        assert R2.parse(b"\x03\xff").r == 255


# ---------------------------------------------------------------------------
# D 族：Computed × rfield（D1-4 表达式）/ × field（D5-8 常量折叠）
# ---------------------------------------------------------------------------


class TestDComputedRfield:
    """D1-4：Computed(x*2) × rfield。

    锚点（§2 表行 6）：parse→c=10；build no-op；实例化 c 不入 init。
    维度：D1=Computed(表达式) D2=rfield。
    """

    def test_d1_instantiation_c_not_in_init(self):
        """D1 实例化：D(x=5) 可省 c；c 属性为 None（init=False default=None）。"""

        @dataclass
        class D(StructMixin):
            x: int = field(Byte)
            c: int = rfield(Computed(x * 2))

        inst = D(x=5)
        assert inst.c is None

    def test_d2_build_noop(self):
        """D2 build：D(x=5).build() == b'\\x05'（Computed 不写字节）。"""

        @dataclass
        class D(StructMixin):
            x: int = field(Byte)
            c: int = rfield(Computed(x * 2))

        assert D(x=5).build() == b"\x05"

    def test_d3_parse_evaluates(self):
        """D3 parse：parse(b'\\x05').c == 10（表达式求值存值）。"""

        @dataclass
        class D(StructMixin):
            x: int = field(Byte)
            c: int = rfield(Computed(x * 2))

        parsed = D.parse(b"\x05")
        assert parsed.x == 5
        assert parsed.c == 10

    def test_d4_roundtrip_bytes_stable(self):
        """D4 round-trip：parse → build 字节不变（Computed no-op）。"""

        @dataclass
        class D(StructMixin):
            x: int = field(Byte)
            c: int = rfield(Computed(x * 2))

        parsed = D.parse(b"\x05")
        assert parsed.build() == b"\x05"


class TestDComputedConstField:
    """D5-8：Computed(42) × field（常量折叠，§5.3）。

    锚点（§2 表行 7 + §5.3）：ops==[Const(42)] → init_default=42。
    维度：D1=Computed(常量) D2=field。
    """

    def test_d5_instantiation_folded_constant(self):
        """D5 实例化：DC().c == 42（常量折叠，§5.3）。"""

        @dataclass
        class DC(StructMixin):
            c: int = field(Computed(42))

        assert DC().c == 42

    def test_d6_build_noop(self):
        """D6 build：DC().build() == b''（no-op 不写不校验）。"""

        @dataclass
        class DC(StructMixin):
            c: int = field(Computed(42))

        assert DC().build() == b""

    def test_d7_parse_returns_constant(self):
        """D7 parse：DC.parse(b'').c == 42。"""

        @dataclass
        class DC(StructMixin):
            c: int = field(Computed(42))

        assert DC.parse(b"").c == 42

    def test_d8_build_ignores_explicit_value(self):
        """D8 build 显式：DC(c=99).build() == b''（忽略，no-op）。"""

        @dataclass
        class DC(StructMixin):
            c: int = field(Computed(42))

        assert DC(c=99).build() == b""


# ---------------------------------------------------------------------------
# E 族：Padding × wfield（E1-4）/ × field（E5-8）
# ---------------------------------------------------------------------------


class TestEPaddingWfield:
    """E1-4：Padding(2) × wfield（WO）。

    锚点（§2 表行 8）：parse 消费不存；build 缺省/显式均写 pattern。
    维度：D1=Padding D2=wfield。
    """

    def test_e1_instantiation_optional(self):
        """E1 实例化：E(x=1, y=2) 可省 p（Void → Optional）。"""

        @dataclass
        class E(StructMixin):
            x: int = field(Byte)
            p: bytes = wfield(Padding(2))
            y: int = field(Byte)

        inst = E(x=1, y=2)
        assert inst.p is None

    def test_e2_build_writes_pattern(self):
        """E2 build 缺省：E(x=1, y=2).build() == b'\\x01\\x00\\x00\\x02'。"""

        @dataclass
        class E(StructMixin):
            x: int = field(Byte)
            p: bytes = wfield(Padding(2))
            y: int = field(Byte)

        assert E(x=1, y=2).build() == b"\x01\x00\x00\x02"

    def test_e3_parse_consumes_not_stored(self):
        """E3 parse：p 无实例属性（WO 消费字节不存储）。"""

        @dataclass
        class E(StructMixin):
            x: int = field(Byte)
            p: bytes = wfield(Padding(2))
            y: int = field(Byte)

        parsed = E.parse(b"\x01\x00\x00\x02")
        assert (parsed.x, parsed.y) == (1, 2)
        assert not hasattr(parsed, "p")

    def test_e4_build_ignores_explicit_value(self):
        """E4 build 显式：p 值被忽略仍写 pattern（锚点行 8 显式列）。"""

        @dataclass
        class E(StructMixin):
            x: int = field(Byte)
            p: bytes = wfield(Padding(2))
            y: int = field(Byte)

        assert E(x=1, y=2, p=b"ZZ").build() == b"\x01\x00\x00\x02"


class TestEPaddingField:
    """E5-8：Padding(2) × field（RW）。

    锚点（§2 表行 8 + P1）：parse **存 None**；显式忽略；实例化 None。
    维度：D1=Padding D2=field。
    """

    def test_e5_instantiation_none(self):
        """E5 实例化：EP(x=1, y=2).p is None。"""

        @dataclass
        class EP(StructMixin):
            x: int = field(Byte)
            p: bytes = field(Padding(2))
            y: int = field(Byte)

        assert EP(x=1, y=2).p is None

    def test_e6_build_defaulted_writes_pattern(self):
        """E6 build 缺省：EP(x=1, y=2).build() == b'\\x01\\x00\\x00\\x02'。"""

        @dataclass
        class EP(StructMixin):
            x: int = field(Byte)
            p: bytes = field(Padding(2))
            y: int = field(Byte)

        assert EP(x=1, y=2).build() == b"\x01\x00\x00\x02"

    def test_e7_parse_stores_none(self):
        """E7 parse：parse 后 p is None 且**存在**属性（锚点 P1：Padding 存 None）。"""

        @dataclass
        class EP(StructMixin):
            x: int = field(Byte)
            p: bytes = field(Padding(2))
            y: int = field(Byte)

        parsed = EP.parse(b"\x01\x00\x00\x02")
        assert hasattr(parsed, "p")
        assert parsed.p is None

    def test_e8_build_ignores_explicit_value(self):
        """E8 build 显式：EP(x=1, y=2, p=b'ZZ') 仍写 pattern。"""

        @dataclass
        class EP(StructMixin):
            x: int = field(Byte)
            p: bytes = field(Padding(2))
            y: int = field(Byte)

        assert EP(x=1, y=2, p=b"ZZ").build() == b"\x01\x00\x00\x02"


# ---------------------------------------------------------------------------
# F 族：Bytes(x+1) 表达式族（F1-4 顶层 / F5-8 Prefixed×1 / F9-12 嵌套×2 /
# F13-16 消费者面）
# ---------------------------------------------------------------------------


class TestFTopLevelExpr:
    """F1-4：Bytes(x+1) × field 顶层（B3/B1e）。

    锚点（§2 表行 12 + §8 F 行）：W(x=1) 可实例化；build 缺 y→FieldValueMissing
    （时机后移，对齐原版 KeyError 时机）；Const x 常量补值后 build 成功。
    维度：D5=算术 D4=顶层 D3=时机。
    """

    def test_f1_instantiation_optional(self):
        """F1 实例化：W(x=1) 可省 y（Instance{expr_derived} → Optional）。"""

        @dataclass
        class W(StructMixin):
            x: int = field(Int8ub)
            y: bytes = field(Bytes(x + 1))

        inst = W(x=1)
        assert inst.y is None

    def test_f2_build_missing_y_field_value_missing(self):
        """F2 build 缺 y：W(x=1).build() → FieldValueMissingError（B3 时机后移）。"""

        @dataclass
        class W(StructMixin):
            x: int = field(Int8ub)
            y: bytes = field(Bytes(x + 1))

        _require_fvm()
        with pytest.raises(_FVM):
            W(x=1).build()

    def test_f3_build_with_value(self):
        """F3 build：W(x=1, y=b'ab').build() == b'\\x01ab'。"""

        @dataclass
        class W(StructMixin):
            x: int = field(Int8ub)
            y: bytes = field(Bytes(x + 1))

        assert W(x=1, y=b"ab").build() == b"\x01ab"

    def test_f4_const_x_defaulted_build(self):
        """F4 Const x 补值：W2(y=b'ab').build() == b'\\x01ab'（B1e）。"""

        @dataclass
        class W2(StructMixin):
            x: int = field(Const(1, Byte))
            y: bytes = field(Bytes(x + 1))

        assert W2(y=b"ab").build() == b"\x01ab"


class TestFPrefixedExpr:
    """F5-8：Prefixed(Int8ub, Bytes(x+1)) × field（嵌套×1，B2）。

    锚点（§8 F 行 + E3/E4）：parse(b'\\x01\\x02ab')→p=b'ab'；round-trip；
    Const x 组合。
    维度：D4=Prefixed×1 D5=算术 D6=Bytes。
    """

    def test_f5_parse(self):
        """F5 parse：M.parse(b'\\x01\\x02ab').p == b'ab'。"""

        @dataclass
        class M(StructMixin):
            x: int = field(Int8ub)
            p: bytes = field(Prefixed(Int8ub, Bytes(x + 1)))

        parsed = M.parse(b"\x01\x02ab")
        assert parsed.x == 1
        assert parsed.p == b"ab"

    def test_f6_build(self):
        """F6 build：M(x=1, p=b'ab').build() == b'\\x01\\x02ab'。"""

        @dataclass
        class M(StructMixin):
            x: int = field(Int8ub)
            p: bytes = field(Prefixed(Int8ub, Bytes(x + 1)))

        assert M(x=1, p=b"ab").build() == b"\x01\x02ab"

    def test_f7_roundtrip(self):
        """F7 round-trip：parse → build 字节不变。"""

        @dataclass
        class M(StructMixin):
            x: int = field(Int8ub)
            p: bytes = field(Prefixed(Int8ub, Bytes(x + 1)))

        parsed = M.parse(b"\x01\x02ab")
        assert parsed.build() == b"\x01\x02ab"

    def test_f8_const_x_combination(self):
        """F8 Const x：MC(p=b'ab').build() == b'\\x01\\x02ab'（x 常量补值）。"""

        @dataclass
        class MC(StructMixin):
            x: int = field(Const(1, Int8ub))
            p: bytes = field(Prefixed(Int8ub, Bytes(x + 1)))

        assert MC(p=b"ab").build() == b"\x01\x02ab"


class TestFDeepNestedExpr:
    """F9-12：Prefixed×2 / Switch 内（嵌套×2）。

    锚点（§8 F 行）：parse(b'\\x01\\x03\\x02ab')；round-trip；case 内 Bytes(x+1)。
    维度：D4=Prefixed×2/Switch 内 D5=算术。
    """

    def test_f9_double_prefixed_parse(self):
        """F9 parse：M2.parse(b'\\x01\\x03\\x02ab').p == b'ab'。"""

        @dataclass
        class M2(StructMixin):
            x: int = field(Int8ub)
            p: bytes = field(Prefixed(Int8ub, Prefixed(Int8ub, Bytes(x + 1))))

        parsed = M2.parse(b"\x01\x03\x02ab")
        assert parsed.p == b"ab"

    def test_f10_double_prefixed_roundtrip(self):
        """F10 round-trip：build → parse 字节/值稳定。"""

        @dataclass
        class M2(StructMixin):
            x: int = field(Int8ub)
            p: bytes = field(Prefixed(Int8ub, Prefixed(Int8ub, Bytes(x + 1))))

        assert M2(x=1, p=b"ab").build() == b"\x01\x03\x02ab"
        assert M2.parse(b"\x01\x03\x02ab").build() == b"\x01\x03\x02ab"

    def test_f11_switch_case_expr_bytes(self):
        """F11 Switch 内 Bytes(x+1)：key=1 → Bytes(2) 分支 round-trip。

        维度：D6=Switch 消费者 D5=字段引用。
        """

        @dataclass
        class MS(StructMixin):
            x: int = field(Int8ub)
            s: Any = field(Switch(x, {1: Bytes(x + 1), 2: Bytes(1)}))

        assert MS.parse(b"\x01AB").s == b"AB"
        assert MS(x=1, s=b"AB").build() == b"\x01AB"

    def test_f12_prefixed_switch_expr_key(self):
        """F12 Prefixed × Switch(x+1)：key 表达式（B2 矩阵形态）round-trip。"""

        @dataclass
        class MSP(StructMixin):
            x: int = field(Int8ub)
            s: Any = field(Prefixed(Int8ub, Switch(x + 1, {1: Byte, 2: Bytes(2)})))

        parsed = MSP.parse(b"\x01\x02AB")
        assert parsed.s == b"AB"
        assert MSP(x=1, s=b"AB").build() == b"\x01\x02AB"


class TestFConsumers:
    """F13-16：消费者面（Array/If/Computed/PrefixedArray）。

    锚点（§8 F 行）：各消费者读 ctx 有效值；round-trip。
    维度：D6=四消费者 D5=算术。
    """

    def test_f13_array_count_expr(self):
        """F13 Array(x+1, Byte)：count 表达式 round-trip。"""

        @dataclass
        class MA(StructMixin):
            x: int = field(Int8ub)
            items: list = field(Array(x + 1, Byte))

        parsed = MA.parse(b"\x01\x01\x02")
        assert parsed.items == [1, 2]
        assert MA(x=1, items=[1, 2]).build() == b"\x01\x01\x02"

    def test_f14_if_cond_expr(self):
        """F14 If(x>0, Byte)：cond 表达式（真分支 + 零分支）。"""

        @dataclass
        class MI(StructMixin):
            x: int = field(Int8ub)
            c: Any = field(If(x > 0, Byte))

        parsed = MI.parse(b"\x01\x7f")
        assert parsed.c == 0x7F
        assert MI(x=1, c=0x7F).build() == b"\x01\x7f"
        # cond=0 → Pass：0 数据字节
        parsed0 = MI.parse(b"\x00")
        assert parsed0.c is None
        assert MI(x=0, c=None).build() == b"\x00"

    def test_f15_computed_expr_consumer(self):
        """F15 Computed(x+1)：rfield 消费者 no-op + parse 求值。"""

        @dataclass
        class MC2(StructMixin):
            x: int = field(Int8ub)
            c: int = rfield(Computed(x + 1))

        assert MC2(x=4).build() == b"\x04"
        assert MC2.parse(b"\x04").c == 5

    def test_f16_prefixed_array(self):
        """F16 PrefixedArray(Int8ub, Byte)：round-trip。"""

        @dataclass
        class MPA(StructMixin):
            items: list = field(PrefixedArray(Int8ub, Byte))

        parsed = MPA.parse(b"\x02\x01\x02")
        assert parsed.items == [1, 2]
        assert MPA(items=[1, 2]).build() == b"\x02\x01\x02"


# ---------------------------------------------------------------------------
# G 族：B1-B3 验收回归（G1-8，用户场景）
# ---------------------------------------------------------------------------


class TestGAcceptanceRegressions:
    """G1-8：v0.1.2 验收场景回归（止血已转绿项保留作防线，§8 G 行）。

    维度：跨族组合（用户场景）。
    """

    def test_g1_const_instantiation(self):
        """G1：A() 实例化 x==1（Const 隐式 default）。"""

        @dataclass
        class A(StructMixin):
            x: int = field(Const(1, Byte))

        assert A().x == 1

    def test_g2_const_plus_expr_build(self):
        """G2：A(y=b'ab').build() == b'\\x01ab'（B1e）。"""

        @dataclass
        class A(StructMixin):
            x: int = field(Const(1, Byte))
            y: bytes = field(Bytes(x + 1))

        assert A(y=b"ab").build() == b"\x01ab"

    def test_g3_const_plus_expr_parse_roundtrip(self):
        """G3：A.parse(b'\\x01\\x02ab') round-trip（尾部字节不消费，Bytes 定长）。"""

        @dataclass
        class A(StructMixin):
            x: int = field(Const(1, Byte))
            y: bytes = field(Bytes(x + 1))

        parsed = A.parse(b"\x01\x02ab")
        assert (parsed.x, parsed.y) == (1, b"\x02a")
        assert parsed.build() == b"\x01\x02a"

    def test_g4_const_int_instantiation(self):
        """G4：C2().c == 5（Const int）。"""

        @dataclass
        class C2(StructMixin):
            c: int = field(Const(0x05, Int8ub))

        assert C2().c == 5

    def test_g5_const_bytes_instantiation(self):
        """G5：P1(tag=1).c == b'Z'（Const bytes）。"""

        @dataclass
        class P1(StructMixin):
            tag: int = field(Int8ub)
            c: bytes = field(Const(b"Z", Bytes(1)))

        assert P1(tag=1).c == b"Z"

    def test_g6_wfield_expr_build_missing_raises(self):
        """G6：W(x=1) 实例化 OK + build 缺 y → FieldValueMissingError。"""

        @dataclass
        class W(StructMixin):
            x: int = field(Int8ub)
            y: bytes = wfield(Bytes(x + 1))

        inst = W(x=1)
        _require_fvm()
        with pytest.raises(_FVM):
            inst.build()

    def test_g7_wfield_expr_build_with_value(self):
        """G7：W(x=1, y=b'ab').build() == b'\\x01ab'。"""

        @dataclass
        class W(StructMixin):
            x: int = field(Int8ub)
            y: bytes = wfield(Bytes(x + 1))

        assert W(x=1, y=b"ab").build() == b"\x01ab"

    def test_g8_rfield_const_expr_build(self):
        """G8：rfield(Const)+Bytes(x+1) build OK（B1f）。"""

        @dataclass
        class A2(StructMixin):
            x: int = rfield(Const(1, Byte))
            y: bytes = field(Bytes(x + 1))

        assert A2(y=b"ab").build() == b"\x01ab"


# ---------------------------------------------------------------------------
# H 族：边界（H1-4）
# ---------------------------------------------------------------------------


class TestHBoundaries:
    """H1-4：边界条件（§8 H 行 + §10）。

    维度：D1/D2 交叉边界。
    """

    def test_h1_context_injection_compile_rejected(self):
        """H1（B4）：field(..., context={...}) → 编译期 CompilationError。

        设计 §5.6：v0.1.2 编译期拒绝（消除静默忽略），v0.1.3 提供注入。
        编译发生在 __init_subclass__（类体定义阶段）。
        """
        with pytest.raises(CompilationError):

            @dataclass
            class CI(StructMixin):
                x: int = field(Int8ub, context={"k": 1})

    def test_h2_const_mutable_value_instantiation_none(self):
        """H2（§10.1）：Const(list) 可变对象 → 实例化 None（防共享可变默认值）。"""

        @dataclass
        class CM(StructMixin):
            c: Any = field(Const([1, 2], Bytes(2)))

        assert CM().c is None

    def test_h3_explicit_default_precedence(self):
        """H3（§10.2）：显式 default= 永远优先于框架派生。"""

        @dataclass
        class ED(StructMixin):
            x: int = field(Const(1, Byte), default=7)
            d: int = field(Default(Byte, 9), default=5)

        assert ED().x == 7
        assert ED().d == 5

    def test_h4_bitstruct_padding_symmetry(self):
        """H4（§10.9）：BitStruct 域内 Padding 分类同字节域（parse 存 None /
        build 写 pattern 对称）。"""

        from neoconstruct import BitStructMixin, Bit, BitsInteger

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
# I 族：Peek × field（I1-6，v2 P1 关联 + B5）
# ---------------------------------------------------------------------------


class TestIPeek:
    """I1-6：Peek(Int8ub) × field。

    锚点（§2 表行 10 + anchor3 A1-A4）：实例化=None（Control 哑值可省，B5）；
    parse 存有效值（p=5）；build 缺省 no-op（b'\\x01'）；显式忽略（同字节）。
    维度：D1=Peek D2=field D3=时机。
    """

    def test_i1_instantiation_optional(self):
        """I1（B5）：PK(x=9) 可省 p（Control → Optional 哑值）。"""

        @dataclass
        class PK(StructMixin):
            p: int = field(Peek(Byte))
            x: int = field(Byte)

        inst = PK(x=9)
        assert inst.p is None

    def test_i2_parse_stores_effective_value(self):
        """I2：parse 存有效值：parse(b'\\x05\\x99').p == 5（锚点 A4，流不消费）。"""

        @dataclass
        class PK(StructMixin):
            p: int = field(Peek(Byte))
            x: int = field(Byte)

        parsed = PK.parse(b"\x05\x99")
        assert parsed.p == 5
        assert parsed.x == 0x99

    def test_i3_build_defaulted_noop(self):
        """I3：build 缺省 no-op：PK(x=9).build() == b'\\x09'（锚点 A1：Peek 0 字节）。"""

        @dataclass
        class PK(StructMixin):
            p: int = field(Peek(Byte))
            x: int = field(Byte)

        assert PK(x=9).build() == b"\x09"

    def test_i4_build_explicit_ignored(self):
        """I4：build 显式忽略：PK(p=5, x=9).build() == b'\\x09'（字节同 I3，A2）。"""

        @dataclass
        class PK(StructMixin):
            p: int = field(Peek(Byte))
            x: int = field(Byte)

        assert PK(p=5, x=9).build() == b"\x09"

    def test_i5_iec104_real_chain(self):
        """I5（v2.1 REV-N1）：IEC104 真实链 parse 全链 PASS + build 腿报错锁定。

        链：cf1_peek(Peek) → Computed(引用 cf1_peek) → Switch 分派
        （test_iec104:180-191 形态）。parse 腿：peek 存有效值[A4] → Computed
        求值 → Switch 正确分派。build 腿：§5.1d 有意差异——Control 忽略实例值
        → ctx 写 None → Computed 消费 None 必败（v0.1.3 前该形态限 parse-only）。
        """

        @dataclass
        class IECFrame(StructMixin):
            cf1_peek: int = field(Peek(Byte))
            frame_type: int = rfield(
                Computed(
                    (cf1_peek & 1) * (1 + 2 * ((cf1_peek >> 1) & 1))
                )
            )
            body: Any = field(
                Switch(frame_type, {0: Byte, 1: Bytes(2), 3: Bytes(3)})
            )

        # parse 腿全链 PASS：cf1=0x03 → frame_type=3 → Bytes(3) 分支
        parsed = IECFrame.parse(b"\x03" + b"\x01\x02\x03")
        assert parsed.cf1_peek == 3
        assert parsed.frame_type == 3
        assert parsed.body == b"\x01\x02\x03"

        # build 腿报错锁定（§5.1d 有意差异）
        with pytest.raises(ConstructError):
            parsed.build()

    def test_i6_build_computed_consuming_peek_raises(self):
        """I6：从零 build + 后继 Computed 引用 peek → 报错（哑值 None 流入，
        锚点 A3：原版 TypeError 等价语义）。"""

        @dataclass
        class PK6(StructMixin):
            p: int = field(Peek(Byte))
            c: int = rfield(Computed(p + 1))

        # B5：p 可省（红灯阶段实例化即 TypeError）
        inst = PK6()
        with pytest.raises(ConstructError):
            inst.build()


# ---------------------------------------------------------------------------
# J 族：Seek × field（J1-4）
# ---------------------------------------------------------------------------


class TestJSeek:
    """J1-4：Seek × field。

    锚点（§2 表行 11 + anchor3 S1-S2）：实例化=None（可省）；parse 值=新流
    位置；build 缺省 seek 移动（忽略值）；round-trip（绝对定位）。
    维度：D1=Seek D2=field D3=时机。
    """

    def test_j1_instantiation_optional(self):
        """J1（B5）：SK(a=.., b=..) 可省 s（Control → Optional）。"""

        @dataclass
        class SK(StructMixin):
            a: bytes = field(Bytes(2))
            s: int = field(Seek(1, 1))
            b: int = field(Byte)

        inst = SK(a=b"ab", b=9)
        assert inst.s is None

    def test_j2_parse_returns_new_position(self):
        """J2：parse 值=新流位置：s == 3（锚点 S2）。"""

        @dataclass
        class SK2(StructMixin):
            a: bytes = field(Bytes(2))
            s: int = field(Seek(1, 1))
            b: int = field(Byte)

        parsed = SK2.parse(b"abcd")
        assert parsed.s == 3
        assert parsed.b == ord("d")

    def test_j3_build_seeks_writes_skip_pattern(self):
        """J3：build 缺省 seek 移动：b'ab' + skip1 + \\x09 == b'ab\\x00\\t'（S1）。"""

        @dataclass
        class SK3(StructMixin):
            a: bytes = field(Bytes(2))
            s: int = field(Seek(1, 1))
            b: int = field(Byte)

        assert SK3(a=b"ab", b=9).build() == b"ab\x00\t"

    def test_j4_seek_zero_absolute_roundtrip(self):
        """J4：Seek(0, 0) 绝对定位：parse s==0，round-trip 字节稳定。"""

        @dataclass
        class SK4(StructMixin):
            s: int = field(Seek(0, 0))
            a: int = field(Byte)

        parsed = SK4.parse(b"\x05")
        assert parsed.s == 0
        assert parsed.a == 5
        assert SK4(a=5).build() == b"\x05"


# ---------------------------------------------------------------------------
# K 族：Checksum × field（K1-5）
# ---------------------------------------------------------------------------


class TestKChecksum:
    """K1-5：Checksum(Bytes(2), crc16) × field（modbus M 场景，B5）。

    锚点（§2 表行 12 + anchor3 C1-C3）：实例化=None（可省）；parse 校验+存
    digest；build 缺省重算（b'\\x01\\x02\\x81\\xe1'）；显式错误值忽略；校验失败
    → ChecksumError。start/end 引用 Tell 字段（body_start/body_end 形态）。
    维度：D1=Checksum D2=field D3=时机 D5=Tell 引用。
    """

    def test_k1_instantiation_optional(self):
        """K1（B5）：Pkt(addr=.., data=..) 可省 crc（Control → Optional）。"""

        @dataclass
        class Pkt(StructMixin):
            body_start: int = rfield(Tell())
            addr: int = field(Byte)
            fc: int = field(Byte)
            data: bytes = field(Bytes(2))
            body_end: int = rfield(Tell())
            crc: bytes = field(
                Checksum(
                    Bytes(2),
                    _modbus_crc16,
                    start=body_start,
                    end=body_end,
                )
            )

        inst = Pkt(addr=1, fc=2, data=b"\x03\x04")
        assert inst.crc is None

    def test_k2_parse_verifies_and_stores_digest(self):
        """K2：parse 校验通过 + crc == 重算 digest。"""

        @dataclass
        class Pkt(StructMixin):
            body_start: int = rfield(Tell())
            addr: int = field(Byte)
            fc: int = field(Byte)
            data: bytes = field(Bytes(2))
            body_end: int = rfield(Tell())
            crc: bytes = field(
                Checksum(
                    Bytes(2),
                    _modbus_crc16,
                    start=body_start,
                    end=body_end,
                )
            )

        payload = bytes([0x01, 0x02, 0x03, 0x04])
        frame = payload + _modbus_crc16(payload)
        parsed = Pkt.parse(frame)
        assert parsed.crc == _modbus_crc16(payload)

    def test_k3_build_defaulted_recomputes(self):
        """K3：build 缺省重算：b'\\x01\\x02\\x81\\xe1'（锚点 C1）。"""

        @dataclass
        class Pkt2(StructMixin):
            body_start: int = rfield(Tell())
            addr: int = field(Byte)
            fc: int = field(Byte)
            body_end: int = rfield(Tell())
            crc: bytes = field(
                Checksum(
                    Bytes(2),
                    _modbus_crc16,
                    start=body_start,
                    end=body_end,
                )
            )

        assert Pkt2(addr=1, fc=2).build() == b"\x01\x02\x81\xe1"

    def test_k4_build_explicit_wrong_value_ignored(self):
        """K4：build 显式错误值忽略：仍写正确 digest（锚点 C2）。"""

        @dataclass
        class Pkt2(StructMixin):
            body_start: int = rfield(Tell())
            addr: int = field(Byte)
            fc: int = field(Byte)
            body_end: int = rfield(Tell())
            crc: bytes = field(
                Checksum(
                    Bytes(2),
                    _modbus_crc16,
                    start=body_start,
                    end=body_end,
                )
            )

        assert Pkt2(addr=1, fc=2, crc=b"\x00\x00").build() == b"\x01\x02\x81\xe1"

    def test_k5_parse_mismatch_raises_checksum_error(self):
        """K5：parse 校验失败 → ChecksumError（锚点 C3）。"""

        @dataclass
        class Pkt2(StructMixin):
            body_start: int = rfield(Tell())
            addr: int = field(Byte)
            fc: int = field(Byte)
            body_end: int = rfield(Tell())
            crc: bytes = field(
                Checksum(
                    Bytes(2),
                    _modbus_crc16,
                    start=body_start,
                    end=body_end,
                )
            )

        bad_frame = bytes([0x01, 0x02, 0x00, 0x00])
        with pytest.raises(ChecksumError):
            Pkt2.parse(bad_frame)


# ---------------------------------------------------------------------------
# N 族：数据负例（N1-3，B2 回归锁定）
# ---------------------------------------------------------------------------


class TestNDataNegatives:
    """N1-3：数据-schema 不匹配 → 显式 StreamError（非静默错读）。

    锚点（§2 表行 14-15 + anchor3 N1-N3）：长度声明 vs 实际 payload 不符。
    维度：D7=数据负例。
    """

    def test_n1_bytes_length_overrun(self):
        """N1：Bytes(n) 声明 5 实际剩 3 → StreamError（"expected 5, found 3"）。"""

        @dataclass
        class NB(StructMixin):
            n: int = field(Int8ub)
            data: bytes = field(Bytes(n))

        with pytest.raises(StreamError) as exc_info:
            NB.parse(b"\x05abc")
        assert "expected 5" in str(exc_info.value)

    def test_n2_prefixed_length_mismatch(self):
        """N2：Prefixed(Int8ub, GreedyBytes) 长度声明 5 实际 3 → StreamError。"""

        @dataclass
        class NP(StructMixin):
            p: bytes = field(Prefixed(Int8ub, __import__("neoconstruct").GreedyBytes))

        with pytest.raises(StreamError):
            NP.parse(b"\x05abc")

    def test_n3_nested_prefixed_inner_mismatch(self):
        """N3：嵌套 Prefixed 内层需求不符 → StreamError。"""

        from neoconstruct import GreedyBytes

        @dataclass
        class NP2(StructMixin):
            x: int = field(Int8ub)
            p: bytes = field(Prefixed(Int8ub, Prefixed(Int8ub, Bytes(x + 1))))

        # 内层声明 2 但仅剩 1 字节
        with pytest.raises(StreamError):
            NP2.parse(b"\x01\x02\x01a")


# ---------------------------------------------------------------------------
# W 族：If 传播差异锁定（W1-2，§5.1c）
# ---------------------------------------------------------------------------


class TestWPropagationDivergence:
    """W1-2：包装器传播差异（§5.1c 有意差异锁定）。

    原版传播规则（§2 [AST]）使"包装器包裹全哑值分支"可省；框架按根节点判定
    → Instance{expr_derived} → 缺省报 FieldValueMissing（过严方向，显式安全）。
    维度：D1=If 传播 D2=field D3=build 缺省。
    """

    def test_w1_if_padding_defaulted_build_raises(self):
        """W1：field(If(flag>0, Padding(2))) 缺省 build → FieldValueMissingError。

        有意差异：原版成功写 pattern（and(Padding=T,Pass=T)=T，实测 W1）；
        v0.1.2 不实现包装器传播（§5.1c），过严方向错误形态为显式框架异常。
        """

        @dataclass
        class W1C(StructMixin):
            flag: int = field(Byte)
            p: Any = field(If(flag > 0, Padding(2)))

        _require_fvm()
        with pytest.raises(_FVM):
            W1C(flag=1).build()

    def test_w2_if_byte_defaulted_build_raises(self):
        """W2：field(If(flag>0, Byte)) 缺省 build → FieldValueMissingError。

        有意差异：原版 KeyError: 字段名（实测 W2）——语义等价必填，错误形态
        统一为框架异常。
        """

        @dataclass
        class W2C(StructMixin):
            flag: int = field(Byte)
            b: Any = field(If(flag > 0, Byte))

        _require_fvm()
        with pytest.raises(_FVM):
            W2C(flag=1).build()
