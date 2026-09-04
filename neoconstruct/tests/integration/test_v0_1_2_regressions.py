"""v0.1.2 回归测试：B1/B2/B3 止血修复行为锁定。

期望值来源：
- B1 值语义：construct 2.10.70 基线（experiments/v0.1.2-spike/anchor_construct.py
  实测：Const parse 后值入 Container、build 缺键用 value；Default 同模式）
- B2 嵌套算术表达式：原版 E3/E4 基线（原版等价写法 Prefixed(Int8ub,
  Bytes(字段引用 x+1)) parse b"\\x01\\x02ab" → Container(x=1, y=b'ab')；
  build → b'\\x01\\x02ab'）
- B3 wfield：原版 Padding parse 后值为 None（丢弃语义）；wfield 实例化
  不再强制哑值（对齐 v0.1.1-3 的 wfield(Padding) 修复）

覆盖：
- B1：field(Const(...)) 实例化值 = 常量；Const 字段被表达式消费的 build；
  Const bytes value；显式传错值 → ConstError（节点层校验兜底）
- B1：field(Default(Int8ub, N)) 实例化值 = N；表达式 value → None +
  build 求值（原版 D5 基线）
- B1f：rfield(Const(...)) 被表达式消费（compute_ro_value Const 分支）
- B1 同族：rfield(Padding(...)) build 写 pattern（原版 P2 基线）
- B2：BinExpr（x+1 / x*2）× Prefixed × [Bytes / Switch / Array / If /
  Computed] parse/build/roundtrip（组合矩阵抽样，全矩阵见
  experiments/v0.1.2-probe/matrix_b2.py）
- B3：wfield(Bytes(x+1)) 无参实例化 + 给值 build + parse 丢弃

dataclass 一律定义在测试函数体内（对齐 test_v0_1_1_regressions.py 惯例）。
"""

from dataclasses import dataclass
from typing import Any

import pytest

from neoconstruct import (
    Array,
    Byte,
    Bytes,
    Computed,
    Const,
    ConstError,
    Default,
    FieldValueMissingError,
    If,
    Int8ub,
    Padding,
    Prefixed,
    StructMixin,
    Switch,
    field,
    rfield,
    wfield,
)


# ---------------------------------------------------------------------------
# B1：field(Const(...)) 值语义
# ---------------------------------------------------------------------------


class TestB1ConstValueSemantics:
    """B1 回归：Const 字段实例化值 = 常量值（v0.1.2 前为 None）。"""

    def test_const_int_instantiated_value(self):
        """B1：``field(Const(0x05, Int8ub))`` 实例化后 ``.c == 5``。"""

        @dataclass
        class C2(StructMixin):
            c: int = field(Const(0x05, Int8ub))

        assert C2().c == 0x05
        assert C2().build() == b"\x05"

    def test_const_bytes_instantiated_value(self):
        """B1：``field(Const(b'Z', Bytes(1)))`` 实例化后 ``.c == b'Z'``。"""

        @dataclass
        class P1(StructMixin):
            c: bytes = field(Const(b"Z", Bytes(1)))

        assert P1().c == b"Z"
        assert P1().build() == b"Z"

    def test_const_field_consumed_by_expression_build(self):
        """B1 核心：Const x + Bytes(x+1) 的 build（repro 脚本 A 场景）。

        v0.1.2 前：隐式 default=None → ctx 拿到 None →
        "expression field has wrong type, expected integer"。
        """

        @dataclass
        class A(StructMixin):
            x: int = field(Const(1, Byte))
            y: bytes = field(Bytes(x + 1))

        inst = A(y=b"ab")
        assert inst.x == 1
        assert inst.build() == b"\x01ab"

    def test_const_field_expression_parse_unaffected(self):
        """B1：parse 路径不受影响（force_setattr 覆盖实例值）。

        注：parse b"\\x01\\x02ab" 时 y 读 x+1=2 字节（b'\\x02a'），
        尾部 'b' 不消费（Bytes(2) 定长）；roundtrip 字节 = x + y 的 2 字节。
        """

        @dataclass
        class A(StructMixin):
            x: int = field(Const(1, Byte))
            y: bytes = field(Bytes(x + 1))

        parsed = A.parse(b"\x01\x02ab")
        assert parsed.x == 1
        assert parsed.y == b"\x02a"
        assert parsed.build() == b"\x01\x02a"

    def test_const_explicit_wrong_value_still_const_error(self):
        """B1：显式传错值给 Const 字段 → 节点层 ConstError（校验兜底保留）。"""

        @dataclass
        class C2(StructMixin):
            c: int = field(Const(0x05, Int8ub))

        with pytest.raises(ConstError):
            C2(c=9).build()


class TestB1DefaultValueSemantics:
    """B1 回归：Default 字段实例化值（int 常量 → 值；表达式 → None）。"""

    def test_default_int_constant_instantiated_value(self):
        """B1：``field(Default(Int8ub, 9))`` 实例化后 ``.d == 9``。"""

        @dataclass
        class D0(StructMixin):
            d: int = field(Default(Int8ub, 9))

        assert D0().d == 9
        # 实例值 9 非 None → Default 用显式值（与 build 缺键用默认值同字节）
        assert D0().build() == b"\t"

    def test_default_explicit_value_overrides(self):
        """B1：显式传值 → Default 用显式值（原版 D3 基线）。"""

        @dataclass
        class D0(StructMixin):
            d: int = field(Default(Int8ub, 9))

        assert D0(d=5).build() == b"\x05"

    def test_default_expr_value_none_then_build_evaluates(self):
        """B1：表达式 value → 实例化 None，build 时求值（原版 D5 基线）。"""

        @dataclass
        class D1(StructMixin):
            x: int = field(Byte)
            d: int = field(Default(Byte, x + 1))

        inst = D1(x=4)
        assert inst.d is None
        assert inst.build() == b"\x04\x05"


class TestB1fRfieldConst:
    """B1f 回归：rfield(Const(...)) 被表达式消费（compute_ro_value Const 分支）。

    v0.1.2 前报 "compute_ro_value: node type Const is not a valid RO node"。
    """

    def test_rfield_const_expression_build(self):
        """B1f：rfield Const x + Bytes(x+1) 的 build 与 roundtrip。

        注：parse 消费 3 字节（x=1 + y=2 字节 b'\\x02a'），'b' 不消费。
        """

        @dataclass
        class A2(StructMixin):
            x: int = rfield(Const(1, Byte))
            y: bytes = field(Bytes(x + 1))

        assert A2(y=b"ab").build() == b"\x01ab"
        assert A2.parse(b"\x01\x02ab").build() == b"\x01\x02a"


class TestB1RfieldPadding:
    """B1 同族回归：rfield(Padding(...)) build 写 pattern（README 宣称用法）。

    v0.1.2 前报 "compute_ro_value: node type Padding is not a valid RO node"。
    """

    def test_rfield_padding_build_and_parse(self):
        """B1 同族：rfield Padding build 写 2 字节 0x00（原版 P2 基线）。"""

        @dataclass
        class RP(StructMixin):
            x: int = field(Byte)
            p: int = rfield(Padding(2))
            y: int = field(Byte)

        assert RP(x=1, y=2).build() == b"\x01\x00\x00\x02"
        parsed = RP.parse(b"\x01\x00\x00\x02")
        assert (parsed.x, parsed.y) == (1, 2)
        assert parsed.p is None  # Padding parse 后值为 None（原版 P1 基线）


# ---------------------------------------------------------------------------
# B2：BinExpr（算术表达式）× 嵌套包装器
# ---------------------------------------------------------------------------


class TestB2BinExprNestedInWrapper:
    """B2 回归：算术表达式（x+1 / x*2）嵌套包装器后 parse/build/roundtrip。

    期望值 = 原版 construct 2.10.70 基线（anchor E3/E4）+ 矩阵探针语义推导。
    全矩阵（5 消费者 × 5 包装器 × 3 形态 = 80 用例）见
    experiments/v0.1.2-probe/matrix_b2.py；此处锁定代表性组合。
    """

    def test_prefixed_bytes_add(self):
        """B2：Prefixed × Bytes(x+1)（原版 E3/E4 基线）。"""

        @dataclass
        class M(StructMixin):
            x: int = field(Int8ub)
            p: bytes = field(Prefixed(Int8ub, Bytes(x + 1)))

        assert M.parse(b"\x01\x02ab") == M(x=1, p=b"ab")
        assert M(x=1, p=b"ab").build() == b"\x01\x02ab"

    def test_prefixed_bytes_mul(self):
        """B2：Prefixed × Bytes(x*2)（乘法形态）。"""

        @dataclass
        class M(StructMixin):
            x: int = field(Int8ub)
            p: bytes = field(Prefixed(Int8ub, Bytes(x * 2)))

        assert M.parse(b"\x01\x02ab") == M(x=1, p=b"ab")
        assert M(x=1, p=b"ab").build() == b"\x01\x02ab"

    def test_prefixed_switch_add(self):
        """B2：Prefixed × Switch(x+1)（key=2 → Bytes(2) 分支）。"""

        @dataclass
        class M(StructMixin):
            x: int = field(Int8ub)
            br: Any = field(
                Prefixed(Int8ub, Switch(x + 1, {1: Byte, 2: Bytes(2)}))
            )

        parsed = M.parse(b"\x01\x02AB")
        assert (parsed.x, parsed.br) == (1, b"AB")
        assert M(x=1, br=b"AB").build() == b"\x01\x02AB"

    def test_prefixed_array_count_add(self):
        """B2：Prefixed × Array(x+1, Byte)（count 表达式）。"""

        @dataclass
        class M(StructMixin):
            x: int = field(Int8ub)
            items: list = field(Prefixed(Int8ub, Array(x + 1, Byte)))

        parsed = M.parse(b"\x01\x02\x01\x02")
        assert (parsed.x, parsed.items) == (1, [1, 2])
        assert M(x=1, items=[1, 2]).build() == b"\x01\x02\x01\x02"

    def test_prefixed_if_cond_add(self):
        """B2：Prefixed × If(x+1, Byte)（cond 表达式非零 → then 分支）。"""

        @dataclass
        class M(StructMixin):
            x: int = field(Int8ub)
            c: Any = field(Prefixed(Int8ub, If(x + 1, Byte)))

        parsed = M.parse(b"\x01\x01\x7f")
        assert (parsed.x, parsed.c) == (1, 0x7F)
        assert M(x=1, c=0x7F).build() == b"\x01\x01\x7f"

    def test_prefixed_if_cond_zero_else_branch(self):
        """B2：Prefixed × If(x-1, Byte)（cond 求值 0 → Pass，0 字节）。

        包装器根（Prefixed）按 Instance 分类：显式 None ≡ 缺值 →
        FieldValueMissingError（缺值统一语义）；显式传值成功（假分支
        Pass 不消费值）。
        """

        @dataclass
        class M(StructMixin):
            x: int = field(Int8ub)
            c: Any = field(Prefixed(Int8ub, If(x - 1, Byte)))

        parsed = M.parse(b"\x01\x00")
        assert (parsed.x, parsed.c) == (1, None)
        assert M(x=1, c=0).build() == b"\x01\x00"
        with pytest.raises(FieldValueMissingError):
            M(x=1, c=None).build()

    def test_prefixed_computed_add(self):
        """B2：Prefixed × Computed(x+1)（0 数据字节，值 = 表达式求值）。

        已知边界：Computed 嵌套在包装器内时，字段值提供型判断不递归
        （顶层 subcon 是 PrefixedDescriptor 而非 ComputedDescriptor），
        实例化仍需显式传 c（build 忽略传入值）——留给 v0.1.2-1 值语义
        规格统一收敛。
        """

        @dataclass
        class M(StructMixin):
            x: int = field(Int8ub)
            c: int = field(Prefixed(Int8ub, Computed(x + 1)))

        parsed = M.parse(b"\x01\x00")
        assert (parsed.x, parsed.c) == (1, 2)
        assert M(x=1, c=2).build() == b"\x01\x00"

    def test_double_prefixed_bytes_add(self):
        """B2：两层 Prefixed × Bytes(x+1)（spike B2c 场景）。"""

        @dataclass
        class M2(StructMixin):
            x: int = field(Int8ub)
            p: bytes = field(Prefixed(Int8ub, Prefixed(Int8ub, Bytes(x + 1))))

        assert M2.parse(b"\x01\x03\x02ab") == M2(x=1, p=b"ab")
        assert M2(x=1, p=b"ab").build() == b"\x01\x03\x02ab"


# ---------------------------------------------------------------------------
# B3：wfield 表达式长度字段
# ---------------------------------------------------------------------------


class TestB3WfieldExprLength:
    """B3 回归：wfield(Bytes(x+1)) 不再强制哑值（隐式 default=None）。

    v0.1.2 前：实例化报 TypeError missing argument（与 wfield(Padding)
    的 v0.1.1-3 修复前同症状）。
    """

    def test_wfield_expr_len_instantiation_without_value(self):
        """B3：W(x=1) 无参实例化成功（y 隐式 default=None）。"""

        @dataclass
        class W(StructMixin):
            x: int = field(Int8ub)
            y: bytes = wfield(Bytes(x + 1))

        inst = W(x=1)
        assert inst.x == 1
        assert inst.y is None

    def test_wfield_expr_len_build_with_value(self):
        """B3：W(x=1, y=b'ab') build 正常（spike B3c 基线）。"""

        @dataclass
        class W(StructMixin):
            x: int = field(Int8ub)
            y: bytes = wfield(Bytes(x + 1))

        assert W(x=1, y=b"ab").build() == b"\x01ab"

    def test_wfield_expr_len_parse_discards_value(self):
        """B3：parse 后 wo 字段值丢弃（spike B3d 基线）。"""

        @dataclass
        class W(StructMixin):
            x: int = field(Int8ub)
            y: bytes = wfield(Bytes(x + 1))

        parsed = W.parse(b"\x01ab")
        assert parsed.x == 1
        assert parsed.y is None
