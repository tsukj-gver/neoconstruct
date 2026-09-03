"""v0.1.1 回归测试：BUG-1/BUG-2 红灯基线 + 行为锁定用例。

设计依据：
- ``plans/v0.1.1/traces/v0.1.1-1-BUG复现与遗漏调查.md`` §5.3 红灯基线清单
- 期望值全部取自该报告实测数据（§1.1 BUG-1 基线、§1.2 BUG-2 表格、
  §4.4 monkeypatch 验证表、P2/P3 条目实测）；Bitwise×Switch 与
  Prefixed×Array(count) 两项报告未给修复后 parse 期望值，用当前库即可
  运行的「常量 key / 常量 count 对照组」锚定（对照组喂相同数据实测，
  表达式变体应分派同分支同值——见各用例 docstring）。

覆盖（问题编号见调查报告 §4 P1-P10）：
- P1（BUG-1 本体）：Prefixed × Switch parse/build/roundtrip（typ=1/2 两分支）
- P1（C1 矩阵最小集）：PrefixedArray×Switch、Bitwise×Switch、Prefixed×If、
  Prefixed×Computed、Prefixed×Bytes(len)、Prefixed×Array(count)
- P2（BUG-2 本体）：field(Const(...)) / field(Default(...)) 无参实例化 + build
- P3：wfield(Padding(...)) 无参实例化 + build
- P2 泛化：field(Rebuild(...)) 无参实例化 + build
- P10（行为锁定，不加 xfail）：顶层 Switch(typ+1) parse/build
- P5（负例锁定，不加 xfail）：Const(5) 无 subcon → CompilationError

标记规则（PM 决策）：修复前应失败的用例标 ``xfail(strict=True)``——修复后
XPASS 会转为 FAILURE 报错，强制清理误留标记；P10/P5 为当前即通过的
锁定/负例用例，不加标记。

红灯用例的 dataclass 一律定义在测试函数体内：P1 类用例失败点在类定义
（``__init_subclass__`` 编译期），模块级定义会使整个文件收集失败。
"""

from dataclasses import dataclass
from typing import Any

import pytest

from construct import (
    Array,
    Bitwise,
    Bytes,
    CompilationError,
    Computed,
    Const,
    Default,
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
# P1 BUG-1 本体：Prefixed × Switch
# ---------------------------------------------------------------------------


class TestBug1PrefixedSwitch:
    """P1（BUG-1 本体）：Switch keyfunc 引用根字段，经 Prefixed 包装后编译期失败。

    修复后预期（报告 §1.1 原版基线 + §4.4 monkeypatch 验证）：
    - ``P.parse(b"\\x01\\x01\\x7f")`` → ``(typ=1, br=127)``
    - ``P.parse(b"\\x02\\x02AB")`` → ``(typ=2, br=b"AB")``
    - ``P(typ=1, br=127).build()`` → ``b"\\x01\\x01\\x7f"``
    - ``P(typ=2, br=b"AB").build()`` → ``b"\\x02\\x02AB"``
    """

    @pytest.mark.xfail(
        strict=True,
        raises=CompilationError,
        reason="P1: Switch keyfunc 引用根字段经 Prefixed 包装后无表达式程序（BUG-1 本体）",
    )
    def test_parse_typ1_branch(self):
        """P1: parse typ=1 → Int8ub 分支（期望值：报告 §1.1）。"""

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Prefixed(Int8ub, Switch(typ, {1: Int8ub, 2: Bytes(2)})))

        parsed = P.parse(b"\x01\x01\x7f")
        assert parsed.typ == 1
        assert parsed.br == 127

    @pytest.mark.xfail(
        strict=True,
        raises=CompilationError,
        reason="P1: Switch keyfunc 引用根字段经 Prefixed 包装后无表达式程序（BUG-1 本体）",
    )
    def test_parse_typ2_branch(self):
        """P1: parse typ=2 → Bytes(2) 分支（期望值：报告 §4.4）。"""

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Prefixed(Int8ub, Switch(typ, {1: Int8ub, 2: Bytes(2)})))

        parsed = P.parse(b"\x02\x02AB")
        assert parsed.typ == 2
        assert parsed.br == b"AB"

    @pytest.mark.xfail(
        strict=True,
        raises=CompilationError,
        reason="P1: Switch keyfunc 引用根字段经 Prefixed 包装后无表达式程序（BUG-1 本体）",
    )
    def test_build_typ1_branch(self):
        """P1: build typ=1 → ``b'\\x01\\x01\\x7f'``（期望值：报告 §4.4）。"""

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Prefixed(Int8ub, Switch(typ, {1: Int8ub, 2: Bytes(2)})))

        assert P(typ=1, br=127).build() == b"\x01\x01\x7f"

    @pytest.mark.xfail(
        strict=True,
        raises=CompilationError,
        reason="P1: Switch keyfunc 引用根字段经 Prefixed 包装后无表达式程序（BUG-1 本体）",
    )
    def test_build_typ2_branch(self):
        """P1: build typ=2 → ``b'\\x02\\x02AB'``（期望值：报告 §1.1 原版基线）。"""

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Prefixed(Int8ub, Switch(typ, {1: Int8ub, 2: Bytes(2)})))

        assert P(typ=2, br=b"AB").build() == b"\x02\x02AB"

    @pytest.mark.xfail(
        strict=True,
        raises=CompilationError,
        reason="P1: Switch keyfunc 引用根字段经 Prefixed 包装后无表达式程序（BUG-1 本体）",
    )
    def test_roundtrip_both_branches(self):
        """P1: parse → build 字节还原，两分支（期望值：报告 §4.4 build 行）。"""

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Prefixed(Int8ub, Switch(typ, {1: Int8ub, 2: Bytes(2)})))

        for original in (b"\x01\x01\x7f", b"\x02\x02AB"):
            parsed = P.parse(original)
            assert parsed.build() == original


# ---------------------------------------------------------------------------
# P1 C1 矩阵最小集：包装器 × 表达式消费者
# ---------------------------------------------------------------------------


class TestC1WrapperConsumerMatrix:
    """P1（C1 矩阵最小集）：表达式消费者嵌套在各包装器内 → 同族编译期失败。

    每个组合至少 parse 一例（v0.1.1-2 任务要求）；期望值来源见各 docstring。
    """

    @pytest.mark.xfail(
        strict=True,
        raises=CompilationError,
        reason="P1: PrefixedArray × Switch 嵌套表达式无程序（C1 矩阵）",
    )
    def test_prefixed_array_switch_parse(self):
        """P1: PrefixedArray×Switch parse（期望值：报告 §4.4 PrefixedArray 行）。"""

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(
                PrefixedArray(Int8ub, Switch(typ, {1: Int8ub, 2: Bytes(2)}))
            )

        parsed = P.parse(b"\x01\x02\x05\x06")
        assert parsed.typ == 1
        assert parsed.br == [5, 6]

    @pytest.mark.xfail(
        strict=True,
        raises=CompilationError,
        reason="P1: PrefixedArray × Switch 嵌套表达式无程序（C1 矩阵）",
    )
    def test_prefixed_array_switch_build(self):
        """P1: PrefixedArray×Switch build → ``b'\\x01\\x02\\x05\\x06'``（报告 §4.4）。"""

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(
                PrefixedArray(Int8ub, Switch(typ, {1: Int8ub, 2: Bytes(2)}))
            )

        assert P(typ=1, br=[5, 6]).build() == b"\x01\x02\x05\x06"

    @pytest.mark.xfail(
        strict=True,
        raises=CompilationError,
        reason="P1: Bitwise × Switch 嵌套表达式无程序（C1 矩阵）",
    )
    def test_bitwise_switch_parse(self):
        """P1: Bitwise×Switch parse（typ=1 → Int8ub 分支）。

        期望值锚定：报告未给修复后值，用当前库可运行的常量 key 对照组
        ``Bitwise(Switch(1, {1: Int8ub, 2: Bytes(2)}))`` 实测——parse
        ``b'\\x01\\x7f'`` → br=127（2026-09-03 实测）；typ=1 时表达式
        变体分派同分支，应同值。
        """

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Bitwise(Switch(typ, {1: Int8ub, 2: Bytes(2)})))

        parsed = P.parse(b"\x01\x7f")
        assert parsed.typ == 1
        assert parsed.br == 127

    @pytest.mark.xfail(
        strict=True,
        raises=CompilationError,
        reason="P1: Prefixed × If 嵌套表达式无程序（C1 矩阵）",
    )
    def test_prefixed_if_parse_true_branch(self):
        """P1: Prefixed×If parse，cond 为真（期望值：报告 §4.4 If 行）。"""

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Prefixed(Int8ub, If(typ == 1, Int8ub)))

        parsed = P.parse(b"\x01\x01\x7f")
        assert parsed.typ == 1
        assert parsed.br == 127

    @pytest.mark.xfail(
        strict=True,
        raises=CompilationError,
        reason="P1: Prefixed × If 嵌套表达式无程序（C1 矩阵）",
    )
    def test_prefixed_if_parse_false_branch(self):
        """P1: Prefixed×If parse，cond 为假 → br=None（期望值：报告 §4.4 If 行）。"""

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Prefixed(Int8ub, If(typ == 1, Int8ub)))

        parsed = P.parse(b"\x00\x00")
        assert parsed.typ == 0
        assert parsed.br is None

    @pytest.mark.xfail(
        strict=True,
        raises=CompilationError,
        reason="P1: Prefixed × Computed 嵌套表达式无程序（C1 矩阵）",
    )
    def test_prefixed_computed_parse(self):
        """P1: Prefixed×Computed parse → br=typ（期望值：报告 §4.4 Computed 行）。"""

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Prefixed(Int8ub, Computed(typ)))

        parsed = P.parse(b"\x01\x00")
        assert parsed.typ == 1
        assert parsed.br == 1

    @pytest.mark.xfail(
        strict=True,
        raises=CompilationError,
        reason="P1: Prefixed × Computed 嵌套表达式无程序（C1 矩阵）",
    )
    def test_prefixed_computed_build(self):
        """P1: Prefixed×Computed build → ``b'\\x01\\x00'``（期望值：报告 §4.4）。

        Computed 不消费字节，前缀长度为 0；``br=None`` 显式传值以解耦
        P2（隐式 default）修复状态。
        """

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Prefixed(Int8ub, Computed(typ)))

        assert P(typ=1, br=None).build() == b"\x01\x00"

    @pytest.mark.xfail(
        strict=True,
        raises=CompilationError,
        reason="P1: Prefixed × Bytes(len) 嵌套表达式无程序（C1 矩阵）",
    )
    def test_prefixed_bytes_len_parse(self):
        """P1: Prefixed×Bytes(typ) parse（期望值：报告 §4.4 Bytes 行）。"""

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Prefixed(Int8ub, Bytes(typ)))

        parsed = P.parse(b"\x02\x02AB")
        assert parsed.typ == 2
        assert parsed.br == b"AB"

    @pytest.mark.xfail(
        strict=True,
        raises=CompilationError,
        reason="P1: Prefixed × Bytes(len) 嵌套表达式无程序（C1 矩阵）",
    )
    def test_prefixed_bytes_len_build(self):
        """P1: Prefixed×Bytes(typ) build → ``b'\\x02\\x02XY'``（期望值：报告 §4.4）。"""

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Prefixed(Int8ub, Bytes(typ)))

        assert P(typ=2, br=b"XY").build() == b"\x02\x02XY"

    @pytest.mark.xfail(
        strict=True,
        raises=CompilationError,
        reason="P1: Prefixed × Array(count) 嵌套表达式无程序（C1 矩阵）",
    )
    def test_prefixed_array_count_parse(self):
        """P1: Prefixed×Array(typ) parse。

        期望值锚定：报告未给修复后值，用当前库可运行的常量 count 对照组
        ``Prefixed(Int8ub, Array(2, Int8ub))`` 实测——parse
        ``b'\\x02\\x02\\x01\\x02'`` → br=[1, 2]（2026-09-03 实测）；
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
# P2（BUG-2 本体）/ P3 / P2 泛化：值提供型构造器 default 推导
# ---------------------------------------------------------------------------


class TestBug2ValueProviderDefaults:
    """P2（BUG-2 本体）/ P3 / P2 泛化：值提供型构造器作为 field/wfield 时强制实参。

    修复后预期（报告 §2.2 修复方向）：Const/Default/Rebuild/Padding 类字段
    获得隐式 ``default=None``（+ ``kw_only=True``），build 由节点层补值
    （节点层「值提供」语义已支持，见报告 §1.2 关键事实）。
    """

    @pytest.mark.xfail(
        strict=True,
        raises=TypeError,
        reason="P2: field(Const(5, Int8ub)) 缺隐式 default，无参实例化报 TypeError（BUG-2 本体）",
    )
    def test_const_field_no_arg_instantiation_and_build(self):
        """P2: ``field(Const(5, Int8ub))`` → ``PC()`` 可实例化，build 用常量 5。

        期望值：报告 §1.2（``PC(v=None).build()`` → ``b'\\x05'``，节点层已支持）。
        """

        @dataclass
        class PC(StructMixin):
            v: int = field(Const(5, Int8ub))

        msg = PC()
        assert msg.build() == b"\x05"

    @pytest.mark.xfail(
        strict=True,
        raises=TypeError,
        reason="P2: field(Default(Int8ub, 9)) 缺隐式 default，无参实例化报 TypeError（BUG-2 本体）",
    )
    def test_default_field_no_arg_instantiation_and_build(self):
        """P2: ``field(Default(Int8ub, 9))`` → ``PD()`` 可实例化，build 用默认 9。

        期望值：报告 §1.2（``PD(v=None).build()`` → ``b'\\t'``）。
        """

        @dataclass
        class PD(StructMixin):
            v: int = field(Default(Int8ub, 9))

        msg = PD()
        assert msg.build() == b"\t"

    @pytest.mark.xfail(
        strict=True,
        raises=TypeError,
        reason="P3: wfield(Padding(2)) 缺隐式 default，要求哑值实参",
    )
    def test_wfield_padding_no_arg_instantiation_and_build(self):
        """P3: ``wfield(Padding(2))`` → ``PW(n=1)`` 可实例化（值被忽略）。

        期望值：报告 P3 条目（``PW(n=1, pad=None).build()`` →
        ``b'\\x01\\x00\\x00'``——传任意值均被忽略）。
        """

        @dataclass
        class PW(StructMixin):
            n: int = field(Int8ub)
            pad: bytes = wfield(Padding(2))

        msg = PW(n=1)
        assert msg.build() == b"\x01\x00\x00"

    @pytest.mark.xfail(
        strict=True,
        raises=TypeError,
        reason="P2: field(Rebuild(Int8ub, n)) 缺隐式 default，强制传被丢弃的哑值（P2 泛化）",
    )
    def test_rebuild_field_no_arg_instantiation_and_build(self):
        """P2 泛化: ``field(Rebuild(Int8ub, n))`` → ``PR(n=3)`` 可实例化，build 用表达式值。

        期望值：报告 P2 条目（``PR(n=3, v=0).build()`` → ``b'\\x03\\x03'``，
        传入的 v 被忽略，build 用表达式值 n=3）。
        """

        @dataclass
        class PR(StructMixin):
            n: int = field(Int8ub)
            v: int = field(Rebuild(Int8ub, n))

        msg = PR(n=3)
        assert msg.build() == b"\x03\x03"


# ---------------------------------------------------------------------------
# P10 行为锁定：顶层 Switch(typ+1)
# ---------------------------------------------------------------------------


class TestP10TopLevelComplexSwitchLock:
    """P10（行为锁定）：顶层 Switch(typ+1) 复杂 int 表达式 key 可用——按功能保留。

    调查报告 P10/P6：该行为超出设计文档承诺（「复杂表达式编译期拒绝」
    未实施），实测可用且无害，PM 决策锁定为功能（总纲 §3.1）。
    期望值：报告 c1_edge.py 实测（2026-09-03）。
    """

    def test_parse_and_build(self):
        """P10: Switch(typ+1) parse ``b'\\x01\\x7f'`` → br=127；build 还原。"""

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Switch(typ + 1, {2: Int8ub, 3: Bytes(2)}))

        parsed = P.parse(b"\x01\x7f")
        assert parsed.typ == 1
        assert parsed.br == 127

        assert P(typ=1, br=127).build() == b"\x01\x7f"

    def test_roundtrip(self):
        """P10: parse → build → parse 往返一致。"""

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
# P5 负例锁定：Const(5) 无 subcon
# ---------------------------------------------------------------------------


class TestP5ConstWithoutSubconNegative:
    """P5（负例锁定）：Const(5)（非 bytes、无 subcon）→ CompilationError。

    行为与原版 construct 2.10.70 对齐（原版同样失败），PM 决策不修、
    以负例锁定（总纲 §3.1）。
    """

    def test_const_int_without_subcon_raises(self):
        """P5: ``field(Const(5))`` 类定义即报 CompilationError，文案给出正确示例。"""
        with pytest.raises(CompilationError) as exc_info:

            @dataclass
            class Bad(StructMixin):  # noqa: F841
                v: int = field(Const(5))

        assert "subcon" in str(exc_info.value)
