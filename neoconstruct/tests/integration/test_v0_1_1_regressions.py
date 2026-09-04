"""v0.1.1 回归测试：BUG-1/BUG-2 修复行为锁定 + 边界行为锁定用例。

期望值全部取自实测数据（原版 construct 2.10.70 基线）；Bitwise×Switch 与
Prefixed×Array(count) 两项无原版对照，用当前库即可运行的「常量 key /
常量 count 对照组」锚定（对照组喂相同数据实测，表达式变体应分派同分支
同值——见各用例 docstring）。

覆盖：
- BUG-1 回归：Prefixed × Switch parse/build/roundtrip（typ=1/2 两分支）
- BUG-1 同族矩阵：PrefixedArray×Switch、Bitwise×Switch、Prefixed×If、
  Prefixed×Computed、Prefixed×Bytes(len)、Prefixed×Array(count)
- BUG-2 回归：field(Const(...)) / field(Default(...)) 无参实例化 + build
- wfield(Padding(...)) 无参实例化 + build
- field(Rebuild(...)) 无参实例化 + build（值提供型同族泛化）
- 行为锁定：顶层 Switch(typ+1) parse/build（宽松行为，按功能保留）
- 负例锁定：Const(5) 无 subcon → CompilationError（与原版对齐）

dataclass 一律定义在测试函数体内：BUG-1 类用例的失败点在类定义
（``__init_subclass__`` 编译期），模块级定义会使整个文件收集失败。
"""

from dataclasses import dataclass
from typing import Any

import pytest

from neoconstruct import (
    Array,
    Bitwise,
    Bytes,
    CompilationError,
    Computed,
    Const,
    Default,
    FieldValueMissingError,
    If,
    Int8ub,
    Padding,
    Prefixed,
    PrefixedArray,
    Rebuild,
    StructMixin,
    Switch,
    field,
    wfield,
)


# ---------------------------------------------------------------------------
# BUG-1 回归：Prefixed × Switch
# ---------------------------------------------------------------------------


class TestBug1PrefixedSwitch:
    """BUG-1 回归：Switch keyfunc 引用根字段，经 Prefixed 包装后正常编译
    parse/build（v0.1.1 前该写法编译期失败）。

    预期行为（原版 construct 2.10.70 基线）：
    - ``P.parse(b"\\x01\\x01\\x7f")`` → ``(typ=1, br=127)``
    - ``P.parse(b"\\x02\\x02AB")`` → ``(typ=2, br=b"AB")``
    - ``P(typ=1, br=127).build()`` → ``b"\\x01\\x01\\x7f"``
    - ``P(typ=2, br=b"AB").build()`` → ``b"\\x02\\x02AB"``
    """

    def test_parse_typ1_branch(self):
        """BUG-1 回归：parse typ=1 → Int8ub 分支（原版基线期望值）。"""

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Prefixed(Int8ub, Switch(typ, {1: Int8ub, 2: Bytes(2)})))

        parsed = P.parse(b"\x01\x01\x7f")
        assert parsed.typ == 1
        assert parsed.br == 127

    def test_parse_typ2_branch(self):
        """BUG-1 回归：parse typ=2 → Bytes(2) 分支。"""

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Prefixed(Int8ub, Switch(typ, {1: Int8ub, 2: Bytes(2)})))

        parsed = P.parse(b"\x02\x02AB")
        assert parsed.typ == 2
        assert parsed.br == b"AB"

    def test_build_typ1_branch(self):
        """BUG-1 回归：build typ=1 → ``b'\\x01\\x01\\x7f'``。"""

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Prefixed(Int8ub, Switch(typ, {1: Int8ub, 2: Bytes(2)})))

        assert P(typ=1, br=127).build() == b"\x01\x01\x7f"

    def test_build_typ2_branch(self):
        """BUG-1 回归：build typ=2 → ``b'\\x02\\x02AB'``（原版基线期望值）。"""

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Prefixed(Int8ub, Switch(typ, {1: Int8ub, 2: Bytes(2)})))

        assert P(typ=2, br=b"AB").build() == b"\x02\x02AB"

    def test_roundtrip_both_branches(self):
        """BUG-1 回归：parse → build 字节还原，两分支。"""

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Prefixed(Int8ub, Switch(typ, {1: Int8ub, 2: Bytes(2)})))

        for original in (b"\x01\x01\x7f", b"\x02\x02AB"):
            parsed = P.parse(original)
            assert parsed.build() == original


# ---------------------------------------------------------------------------
# BUG-1 同族矩阵：包装器 × 表达式消费者
# ---------------------------------------------------------------------------


class TestC1WrapperConsumerMatrix:
    """BUG-1 同族矩阵：表达式消费者嵌套在各包装器内均可正常编译解析。

    每个组合至少 parse 一例；期望值来源见各 docstring。
    """

    def test_prefixed_array_switch_parse(self):
        """BUG-1 同族：PrefixedArray×Switch parse。"""

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(
                PrefixedArray(Int8ub, Switch(typ, {1: Int8ub, 2: Bytes(2)}))
            )

        parsed = P.parse(b"\x01\x02\x05\x06")
        assert parsed.typ == 1
        assert parsed.br == [5, 6]

    def test_prefixed_array_switch_build(self):
        """BUG-1 同族：PrefixedArray×Switch build → ``b'\\x01\\x02\\x05\\x06'``。"""

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(
                PrefixedArray(Int8ub, Switch(typ, {1: Int8ub, 2: Bytes(2)}))
            )

        assert P(typ=1, br=[5, 6]).build() == b"\x01\x02\x05\x06"

    def test_bitwise_switch_parse(self):
        """BUG-1 同族：Bitwise×Switch parse（typ=1 → Int8ub 分支）。

        期望值锚定：用当前库可运行的常量 key 对照组
        ``Bitwise(Switch(1, {1: Int8ub, 2: Bytes(2)}))`` 实测——parse
        ``b'\\x01\\x7f'`` → br=127；typ=1 时表达式变体分派同分支，应同值。
        """

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Bitwise(Switch(typ, {1: Int8ub, 2: Bytes(2)})))

        parsed = P.parse(b"\x01\x7f")
        assert parsed.typ == 1
        assert parsed.br == 127

    def test_prefixed_if_parse_true_branch(self):
        """BUG-1 同族：Prefixed×If parse，cond 为真。"""

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Prefixed(Int8ub, If(typ == 1, Int8ub)))

        parsed = P.parse(b"\x01\x01\x7f")
        assert parsed.typ == 1
        assert parsed.br == 127

    def test_prefixed_if_parse_false_branch(self):
        """BUG-1 同族：Prefixed×If parse，cond 为假 → br=None。"""

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Prefixed(Int8ub, If(typ == 1, Int8ub)))

        parsed = P.parse(b"\x00\x00")
        assert parsed.typ == 0
        assert parsed.br is None

    def test_prefixed_computed_parse(self):
        """BUG-1 同族：Prefixed×Computed parse → br=typ。"""

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Prefixed(Int8ub, Computed(typ)))

        parsed = P.parse(b"\x01\x00")
        assert parsed.typ == 1
        assert parsed.br == 1

    def test_prefixed_computed_build(self):
        """BUG-1 同族：Prefixed×Computed build → ``b'\\x01\\x00'``。

        Computed 不消费字节，前缀长度为 0。包装器根（Prefixed）按
        Instance 分类：显式 None ≡ 缺值 → FieldValueMissingError
        （值语义框架的缺值统一语义）；显式传非 None 值成功（值不参与
        编码——Computed no-op）。
        """

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Prefixed(Int8ub, Computed(typ)))

        assert P(typ=1, br=0).build() == b"\x01\x00"
        with pytest.raises(FieldValueMissingError):
            P(typ=1, br=None).build()

    def test_prefixed_bytes_len_parse(self):
        """BUG-1 同族：Prefixed×Bytes(typ) parse。"""

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Prefixed(Int8ub, Bytes(typ)))

        parsed = P.parse(b"\x02\x02AB")
        assert parsed.typ == 2
        assert parsed.br == b"AB"

    def test_prefixed_bytes_len_build(self):
        """BUG-1 同族：Prefixed×Bytes(typ) build → ``b'\\x02\\x02XY'``。"""

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Prefixed(Int8ub, Bytes(typ)))

        assert P(typ=2, br=b"XY").build() == b"\x02\x02XY"

    def test_prefixed_array_count_parse(self):
        """BUG-1 同族：Prefixed×Array(typ) parse。

        期望值锚定：用当前库可运行的常量 count 对照组
        ``Prefixed(Int8ub, Array(2, Int8ub))`` 实测——parse
        ``b'\\x02\\x02\\x01\\x02'`` → br=[1, 2]；
        typ=2 时表达式变体 count 同值，应同结果。
        """

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Prefixed(Int8ub, Array(typ, Int8ub)))

        parsed = P.parse(b"\x02\x02\x01\x02")
        assert parsed.typ == 2
        assert parsed.br == [1, 2]


# ---------------------------------------------------------------------------
# BUG-2 回归：值提供型构造器隐式 default 推导
# ---------------------------------------------------------------------------


class TestBug2ValueProviderDefaults:
    """BUG-2 回归：值提供型构造器作为 field/wfield 时无需强制实参。

    预期行为：Const/Default/Rebuild/Padding 类字段
    获得隐式 ``default=None``（+ ``kw_only=True``），build 由节点层补值
    （节点层「值提供」语义已支持）。
    """

    def test_const_field_no_arg_instantiation_and_build(self):
        """BUG-2 回归：``field(Const(5, Int8ub))`` → ``PC()`` 可实例化，build 用常量 5。"""

        @dataclass
        class PC(StructMixin):
            v: int = field(Const(5, Int8ub))

        msg = PC()
        assert msg.build() == b"\x05"

    def test_default_field_no_arg_instantiation_and_build(self):
        """BUG-2 回归：``field(Default(Int8ub, 9))`` → ``PD()`` 可实例化，build 用默认 9。"""

        @dataclass
        class PD(StructMixin):
            v: int = field(Default(Int8ub, 9))

        msg = PD()
        assert msg.build() == b"\t"

    def test_wfield_padding_no_arg_instantiation_and_build(self):
        """BUG-2 同族：``wfield(Padding(2))`` → ``PW(n=1)`` 可实例化（值被忽略）。

        build 输出 ``b'\\x01\\x00\\x00'``——传入任意 pad 值均被忽略。
        """

        @dataclass
        class PW(StructMixin):
            n: int = field(Int8ub)
            pad: bytes = wfield(Padding(2))

        msg = PW(n=1)
        assert msg.build() == b"\x01\x00\x00"

    def test_rebuild_field_no_arg_instantiation_and_build(self):
        """BUG-2 同族泛化：``field(Rebuild(Int8ub, n))`` → ``PR(n=3)`` 可实例化。

        build 输出 ``b'\\x03\\x03'``——传入的 v 被忽略，build 用表达式值 n=3。
        """

        @dataclass
        class PR(StructMixin):
            n: int = field(Int8ub)
            v: int = field(Rebuild(Int8ub, n))

        msg = PR(n=3)
        assert msg.build() == b"\x03\x03"


# ---------------------------------------------------------------------------
# 行为锁定：顶层 Switch(typ+1)
# ---------------------------------------------------------------------------


class TestTopLevelComplexSwitchLock:
    """行为锁定：顶层 Switch(typ+1) 复杂 int 表达式 key 可用。

    实测行为：顶层 Switch 的 key 支持「字段引用 + 常量运算」的复杂
    int 表达式（超出「复杂表达式编译期拒绝」的边界），宽松且无害。
    """

    def test_parse_and_build(self):
        """行为锁定：Switch(typ+1) parse ``b'\\x01\\x7f'`` → br=127；build 还原。"""

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Switch(typ + 1, {2: Int8ub, 3: Bytes(2)}))

        parsed = P.parse(b"\x01\x7f")
        assert parsed.typ == 1
        assert parsed.br == 127

        assert P(typ=1, br=127).build() == b"\x01\x7f"

    def test_roundtrip(self):
        """行为锁定：parse → build → parse 往返一致。"""

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Switch(typ + 1, {2: Int8ub, 3: Bytes(2)}))

        original = b"\x01\x7f"
        parsed = P.parse(original)
        assert parsed.build() == original
        re_parsed = P.parse(parsed.build())
        assert (re_parsed.typ, re_parsed.br) == (parsed.typ, parsed.br)


# ---------------------------------------------------------------------------
# 负例锁定：Const(5) 无 subcon
# ---------------------------------------------------------------------------


class TestConstWithoutSubconNegative:
    """负例锁定：Const(5)（非 bytes、无 subcon）→ CompilationError。

    行为与原版 construct 2.10.70 对齐（原版同样失败）。
    """

    def test_const_int_without_subcon_raises(self):
        """负例锁定：``field(Const(5))`` 类定义即报 CompilationError，文案给出正确示例。"""
        with pytest.raises(CompilationError) as exc_info:

            @dataclass
            class Bad(StructMixin):  # noqa: F841
                v: int = field(Const(5))

        assert "subcon" in str(exc_info.value)
