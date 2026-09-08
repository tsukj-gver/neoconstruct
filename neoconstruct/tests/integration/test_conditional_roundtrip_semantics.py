"""条件字段回环语义测试（条件根 Conditional 分类）。

被测语义：条件根（IfThenElse/Switch/Select 作为字段 subcon 根节点）的
build 值语义——resolve 对 None（含属性不存在）不报错，透传给节点 build，
分支自治：

- 选中 Pass 分支 → 不写字节（parse 产 None / 显式传 None / 属性不存在
  三来源同语义，build 均成功）。
- 选中哑值语义分支（Padding/Const/Default）→ 分支自身容忍 None 写字节。
- 选中实值分支（Byte/Bytes）而值为 None → 分支节点自然报错
  （防"改了条件没给值"的不一致静默）。
- ctx 写透传值（含 None），与 parse 对称；实例化默认值为 Optional。

七族用例（每用例 docstring 标注所属族与维度组合）：
- A 回环属性：parse(x).build() == x 对条件字段成立（互为逆运算）。
- B 不一致防护：cond 指向实值分支而值为 None → 分支自然报错。
- C None 三来源等价：parse 产出 / 显式传 None / 属性不存在同语义。
- D ctx 可见性：透传值（含 None）写入 ctx，后继消费者读到。
- E 分支内值提供器：Const/Default/Padding 分支使 None 也能 build。
- F init 面：条件根 init_default → Optional；显式 default= 优先。
- G 边界：嵌套条件 / Sequence 元素 / frozen / Select 负例。

dataclass 一律定义在测试函数体内（与回归测试惯例一致）。
"""

from dataclasses import dataclass
from typing import Any

import pytest

from neoconstruct import (
    Byte,
    Bytes,
    Const,
    ConstructError,
    Default,
    FormatFieldError,
    FieldValueMissingError,
    If,
    IfThenElse,
    Padding,
    Pass,
    Select,
    SelectError,
    Sequence,
    StructMixin,
    Switch,
    field,
    rfield,
    wfield,
)


# ---------------------------------------------------------------------------
# 族 A：回环属性（parse(x).build() == x）
# ---------------------------------------------------------------------------


class TestFamilyARoundtrip:
    """A 族：条件字段 parse 产物直接 build 还原原始数据。

    语义：互为逆运算在 parse→build 方向对条件字段成立——真分支值透传
    重写，Pass 分支 None 透传不写字节。
    """

    def test_if_true_branch_roundtrip(self):
        """A1 If(x>0, Byte) 真分支：parse 产物 build 还原字节与值。"""

        @dataclass
        class MI(StructMixin):
            x: int = field(Byte)
            c: Any = field(If(x > 0, Byte))

        parsed = MI.parse(b"\x01\x7f")
        assert (parsed.x, parsed.c) == (1, 0x7F)
        assert parsed.build() == b"\x01\x7f"

    def test_if_false_branch_none_roundtrip(self):
        """A2 If(x>0, Byte) 假分支：parse 产 c=None，build 还原。"""

        @dataclass
        class MI(StructMixin):
            x: int = field(Byte)
            c: Any = field(If(x > 0, Byte))

        parsed = MI.parse(b"\x00")
        assert parsed.c is None
        assert parsed.build() == b"\x00"

    def test_switch_case_branch_roundtrip(self):
        """A3 Switch(x, {1: Byte, 2: Bytes(2)})：case 分派 roundtrip。"""

        @dataclass
        class MS(StructMixin):
            x: int = field(Byte)
            s: Any = field(Switch(x, {1: Byte, 2: Bytes(2)}))

        parsed1 = MS.parse(b"\x01\x7f")
        assert parsed1.s == 0x7F
        assert parsed1.build() == b"\x01\x7f"

        parsed2 = MS.parse(b"\x02ab")
        assert parsed2.s == b"ab"
        assert parsed2.build() == b"\x02ab"

    def test_switch_default_pass_roundtrip(self):
        """A4 Switch 未命中 → default=Pass：parse 产 None，build 还原。"""

        @dataclass
        class MS(StructMixin):
            x: int = field(Byte)
            s: Any = field(Switch(x, {1: Byte}))

        parsed = MS.parse(b"\x09")
        assert parsed.s is None
        assert parsed.build() == b"\x09"

    def test_select_roundtrip(self):
        """A5 Select(Byte,)：parse 产物 build 还原。"""

        @dataclass
        class SL(StructMixin):
            v: Any = field(Select(Bytes(2), Byte))

        parsed = SL.parse(b"\x42")
        assert parsed.v == 0x42
        assert parsed.build() == b"\x42"

    def test_if_then_else_both_branches_roundtrip(self):
        """A6 IfThenElse(x>0, Byte, Bytes(2))：两分支各 roundtrip。"""

        @dataclass
        class IT(StructMixin):
            x: int = field(Byte)
            c: Any = field(IfThenElse(x > 0, Byte, Bytes(2)))

        then_parsed = IT.parse(b"\x01\x7f")
        assert then_parsed.c == 0x7F
        assert then_parsed.build() == b"\x01\x7f"

        else_parsed = IT.parse(b"\x00\xab\xcd")
        assert else_parsed.c == b"\xab\xcd"
        assert else_parsed.build() == b"\x00\xab\xcd"


# ---------------------------------------------------------------------------
# 族 B：不一致防护（cond 指向实值分支而值为 None → 分支自然报错）
# ---------------------------------------------------------------------------


class TestFamilyBInconsistencyGuard:
    """B 族：条件选择实值分支而值为 None → 分支节点自然报错。

    语义：错误形态是分支自身的编码错误（如 FormatFieldError），非字段层
    FieldValueMissingError——"改了条件没给值"显式失败而非静默。
    """

    def test_if_byte_branch_none_raises_branch_error(self):
        """If(x>0, Byte) 真分支 + c=None → 分支节点自然报 FormatFieldError。"""

        @dataclass
        class MI(StructMixin):
            x: int = field(Byte)
            c: Any = field(If(x > 0, Byte))

        with pytest.raises(FormatFieldError):
            MI(x=1, c=None).build()

    def test_switch_bytes_branch_none_raises_branch_error(self):
        """Switch case → Bytes(2) 分支 + s=None → 分支节点自然报错（非缺值错）。"""

        @dataclass
        class MS(StructMixin):
            x: int = field(Byte)
            s: Any = field(Switch(x, {2: Bytes(2)}))

        with pytest.raises(ConstructError) as exc_info:
            MS(x=2, s=None).build()
        assert not isinstance(exc_info.value, FieldValueMissingError)

    def test_if_then_else_else_branch_none_raises(self):
        """IfThenElse 实值分支（Byte）+ None → 报错；对照：Pass 分支不报。"""

        @dataclass
        class IT(StructMixin):
            x: int = field(Byte)
            c: Any = field(IfThenElse(x > 0, Pass, Byte))

        with pytest.raises(ConstructError):
            IT(x=0, c=None).build()
        # 对照：真分支 Pass 不消费值 → 成功
        assert IT(x=1, c=None).build() == b"\x01"

    def test_if_byte_branch_omitted_raises_branch_error(self):
        """省略 c（属性不存在 ≡ None）同样分支自然报错。"""

        @dataclass
        class MI(StructMixin):
            x: int = field(Byte)
            c: Any = field(If(x > 0, Byte))

        with pytest.raises(FormatFieldError):
            MI(x=1).build()


# ---------------------------------------------------------------------------
# 族 C：None 三来源等价（parse 产出 / 显式传 None / 属性不存在）
# ---------------------------------------------------------------------------


class TestFamilyCNoneSourceEquivalence:
    """C 族：None ≡ 缺席，来源无关——三来源 build 字节一致。

    语义：parse 产出的 None（Pass 分支）、显式传 None、属性不存在
    （未初始化实例）在 resolve 处同语义（透传），build 输出一致。
    """

    def test_parse_produced_none_builds(self):
        """C1 parse 产出 None（Pass 分支）→ build == b'\\x00'。"""

        @dataclass
        class MI(StructMixin):
            x: int = field(Byte)
            c: Any = field(If(x > 0, Byte))

        assert MI.parse(b"\x00").build() == b"\x00"

    def test_explicit_none_builds(self):
        """C2 显式传 c=None → build == b'\\x00'（与 C1 同字节）。"""

        @dataclass
        class MI(StructMixin):
            x: int = field(Byte)
            c: Any = field(If(x > 0, Byte))

        assert MI(x=0, c=None).build() == b"\x00"

    def test_absent_attribute_builds(self):
        """C3 属性不存在（删类级默认使 getattr 抛 AttributeError）→ 同 b'\\x00'。"""

        @dataclass
        class MI(StructMixin):
            x: int = field(Byte)
            c: Any = field(If(x > 0, Byte))

        obj = MI.__new__(MI)
        obj.x = 0
        # 删除类级默认（Optional 字段的 kw_only default=None 类属性），
        # 使 getattr(obj, "c") 真正抛 AttributeError —— resolve 的
        # "属性不存在 ≡ None" 分支端到端覆盖（类为函数内局部定义，无共享）。
        delattr(MI, "c")
        assert obj.build() == b"\x00"


# ---------------------------------------------------------------------------
# 族 D：ctx 可见性（透传值写入 ctx，后继消费者读到）
# ---------------------------------------------------------------------------


class TestFamilyDContextVisibility:
    """D 族：条件字段的透传值（有效值/None）写入 ctx，与 parse 对称。

    语义：后继表达式消费者经 ctx 读条件字段值——有效值分派一致，
    None 流入时显式报错（显式失败优于静默错值）。
    """

    def test_effective_value_flows_to_consumer(self):
        """D1 If 真值 + 后继 Bytes(c) 读 ctx → roundtrip。"""

        @dataclass
        class DC(StructMixin):
            x: int = field(Byte)
            c: Any = field(If(x > 0, Byte))
            n: bytes = field(Bytes(c))

        parsed = DC.parse(b"\x01\x02ab")
        assert (parsed.x, parsed.c, parsed.n) == (1, 2, b"ab")
        assert parsed.build() == b"\x01\x02ab"
        assert DC(x=1, c=2, n=b"ab").build() == b"\x01\x02ab"

    def test_none_flows_to_consumer_raises(self):
        """D2 x=0 → c=None 写入 ctx → Bytes(c) 消费 None → 显式报错。"""

        @dataclass
        class DC(StructMixin):
            x: int = field(Byte)
            c: Any = field(If(x > 0, Byte))
            n: bytes = field(Bytes(c))

        with pytest.raises(ConstructError):
            DC(x=0).build()

    def test_conditional_produced_key_drives_switch(self):
        """D3 条件产出值作 Switch 分派键：parse/build 分派一致。"""

        @dataclass
        class DS(StructMixin):
            has_k: int = field(Byte)
            k: Any = field(If(has_k > 0, Byte))
            v: Any = field(Switch(k, {5: Byte}))

        parsed = DS.parse(b"\x01\x05\x09")
        assert (parsed.has_k, parsed.k, parsed.v) == (1, 5, 9)
        assert parsed.build() == b"\x01\x05\x09"
        assert DS(has_k=1, k=5, v=9).build() == b"\x01\x05\x09"


# ---------------------------------------------------------------------------
# 族 E：分支内值提供器（Const/Default/Padding 分支容忍 None）
# ---------------------------------------------------------------------------


class TestFamilyEBranchValueProviders:
    """E 族：条件分支内是值提供器时，None 也能 build 成功。

    语义：分支节点的 None 容忍（Const 补值/Default 补默认/Padding 写
    pattern）在条件根透传路径下保持——build 值由分支语义决定。
    """

    def test_const_branch_none_builds(self):
        """E1 If(x>0, Const(5, Byte))：c=None → Const 分支写 5；roundtrip。"""

        @dataclass
        class EC(StructMixin):
            x: int = field(Byte)
            c: Any = field(If(x > 0, Const(5, Byte)))

        parsed = EC.parse(b"\x01\x05")
        assert parsed.c == 5
        assert parsed.build() == b"\x01\x05"
        assert EC(x=1).build() == b"\x01\x05"
        assert EC(x=1, c=None).build() == b"\x01\x05"

    def test_default_branch_none_builds(self):
        """E2 Switch → Default(Byte, 0) 分支：None → 写默认 0。"""

        @dataclass
        class ED(StructMixin):
            x: int = field(Byte)
            c: Any = field(Switch(x, {1: Default(Byte, 0)}))

        assert ED(x=1).build() == b"\x01\x00"
        assert ED(x=1, c=None).build() == b"\x01\x00"
        assert ED.parse(b"\x01\x09").c == 9

    def test_padding_branch_none_builds(self):
        """E3 If(x>0, Padding(2))：None → 写 pattern；roundtrip。"""

        @dataclass
        class EP(StructMixin):
            x: int = field(Byte)
            p: Any = field(If(x > 0, Padding(2)))

        parsed = EP.parse(b"\x01\x00\x00")
        assert parsed.p is None
        assert parsed.build() == b"\x01\x00\x00"
        assert EP(x=1).build() == b"\x01\x00\x00"

    def test_both_padding_branches_none_builds(self):
        """E4 IfThenElse(x>0, Padding(1), Padding(2))：两哑值分支均容忍 None。"""

        @dataclass
        class EB(StructMixin):
            x: int = field(Byte)
            p: Any = field(IfThenElse(x > 0, Padding(1), Padding(2)))

        assert EB(x=1).build() == b"\x01\x00"
        assert EB(x=0).build() == b"\x00\x00\x00"
        assert EB(x=1, p=None).build() == b"\x01\x00"
        assert EB(x=0, p=None).build() == b"\x00\x00\x00"


# ---------------------------------------------------------------------------
# 族 F：init 面（条件根 init_default → Optional；显式 default= 优先）
# ---------------------------------------------------------------------------


class TestFamilyFInitDefaults:
    """F 族：default=None 只管 __init__；条件根实例化可省。

    语义：条件根 init_default → Optional（R4）；用户显式 default=
    永远优先；RO/WO 条件根的取值路径（AttributeError ≡ None）。
    """

    def test_conditional_field_omittable_at_init(self):
        """F1 field(If(...)) 实例化可省：MI(x=0) → c is None。"""

        @dataclass
        class MI(StructMixin):
            x: int = field(Byte)
            c: Any = field(If(x > 0, Byte))

        inst = MI(x=0)
        assert inst.c is None

    def test_explicit_default_wins(self):
        """F2 显式 default=0x7F 优先于框架 Optional：实例化携带该值。"""

        @dataclass
        class MD(StructMixin):
            x: int = field(Byte)
            c: Any = field(If(x > 0, Byte), default=0x7F)

        assert MD(x=1).c == 0x7F
        assert MD(x=1).build() == b"\x01\x7f"
        assert MD(x=0).build() == b"\x00"

    def test_rfield_conditional_builds_without_attribute(self):
        """F3 rfield(Switch→Pass)：init=False 从零 build 无属性 → None 透传。"""

        @dataclass
        class FR(StructMixin):
            x: int = field(Byte)
            s: Any = rfield(Switch(99, {1: Byte}, default=Pass))

        assert FR(x=5).build() == b"\x05"
        parsed = FR.parse(b"\x05")
        assert parsed.s is None
        assert parsed.build() == b"\x05"

    def test_wfield_conditional_false_branch_roundtrip(self):
        """F4 wfield(If(x>0, Byte))：假分支 parse→build 回环；真分支 WO 丢弃值不可回。"""

        @dataclass
        class FW(StructMixin):
            x: int = field(Byte)
            c: Any = wfield(If(x > 0, Byte))

        # 假分支：parse 不消费字节（getattr 落到类级默认 None ≡ 缺席）→ Pass → 回环
        parsed = FW.parse(b"\x00")
        assert parsed.c is None
        assert parsed.build() == b"\x00"
        # 真分支：parse 消费字节但 WO 丢弃（getattr 得 None）→ 分支自然报错
        parsed1 = FW.parse(b"\x01\x7f")
        with pytest.raises(ConstructError):
            parsed1.build()
        # 真分支显式提供值可 build
        assert FW(x=1, c=0x7F).build() == b"\x01\x7f"


# ---------------------------------------------------------------------------
# 族 G：边界（嵌套条件 / Sequence 元素 / frozen / Select 负例）
# ---------------------------------------------------------------------------


class TestFamilyGBoundaries:
    """G 族：条件根语义的边界形态。

    语义：嵌套条件（外层根分类透传到内层分支自治）、Sequence 位置元素
    （None 透传 + 元素缺位 ≡ None）、frozen 实例（resolve 只 getattr）、
    Select 全候选失败的显式负例。
    """

    def test_nested_conditional_none_passthrough(self):
        """G1 嵌套条件：None 经外层透传到内层分支自治。

        注：内外两层均用表达式 cond 会触发同名 'cond' 扁平键冲突的
        编译期拒绝（既有诚实限制），故内层用常量 cond。
        """

        @dataclass
        class GN(StructMixin):
            x: int = field(Byte)
            c: Any = field(If(x > 0, IfThenElse(False, Byte, Pass)))

        # x=1 → 外层真 → 内层常量假 → Pass → None（None 经外层透传到内层）
        parsed = GN.parse(b"\x01")
        assert parsed.c is None
        assert parsed.build() == b"\x01"
        # x=0 → 外层 Pass → None
        parsed0 = GN.parse(b"\x00")
        assert parsed0.c is None
        assert parsed0.build() == b"\x00"

        # 内层常量真分支：None 透传到内层实值分支 → 自然报错；显式值成功
        @dataclass
        class GN2(StructMixin):
            x: int = field(Byte)
            c: Any = field(If(x > 0, IfThenElse(True, Byte, Pass)))

        assert GN2(x=1, c=0x7F).build() == b"\x01\x7f"
        with pytest.raises(FormatFieldError):
            GN2(x=1).build()
        assert GN2(x=0).build() == b"\x00"

    def test_sequence_conditional_element(self):
        """G2 Sequence 内条件元素：None 透传 / 元素缺位 ≡ None / 实值分支。"""

        @dataclass
        class GS(StructMixin):
            items: list = field(Sequence(Byte, IfThenElse(False, Byte, Pass)))

        parsed = GS.parse(b"\x01")
        assert parsed.items == [1, None]
        assert parsed.build() == b"\x01"
        # None 元素透传 → Pass 分支不写字节
        assert GS(items=[1, None]).build() == b"\x01"
        # 元素缺位 ≡ None（list 短缺）
        assert GS(items=[1]).build() == b"\x01"

        # 实值分支（常量真条件）：元素值参与编码
        @dataclass
        class GS2(StructMixin):
            items: list = field(Sequence(Byte, IfThenElse(True, Byte, Pass)))

        assert GS2(items=[1, 2]).build() == b"\x01\x02"
        # 真分支元素 None → 分支自然报错
        with pytest.raises(ConstructError):
            GS2(items=[1, None]).build()

    def test_frozen_dataclass_conditional_roundtrip(self):
        """G3 frozen 实例：resolve 只 getattr 不 setattr → 回环成立。"""

        @dataclass(frozen=True)
        class GF(StructMixin):
            x: int = field(Byte)
            c: Any = field(If(x > 0, Byte))

        parsed = GF.parse(b"\x01\x7f")
        assert parsed.build() == b"\x01\x7f"
        parsed0 = GF.parse(b"\x00")
        assert parsed0.c is None
        assert parsed0.build() == b"\x00"

    def test_select_all_candidates_fail(self):
        """G4 Select 全候选 parse 失败 → SelectError（显式负例）。"""

        @dataclass
        class SP(StructMixin):
            v: Any = field(Select(Bytes(2)))

        with pytest.raises(SelectError):
            SP.parse(b"\x42")

    def test_switch_expr_key_conditional_root_roundtrip(self):
        """G5 Switch(x+1, ...) 表达式键 + 条件根字段：roundtrip。"""

        @dataclass
        class SK(StructMixin):
            x: int = field(Byte)
            s: Any = field(Switch(x + 1, {2: Byte}))

        parsed = SK.parse(b"\x01\x7f")
        assert parsed.s == 0x7F
        assert parsed.build() == b"\x01\x7f"
