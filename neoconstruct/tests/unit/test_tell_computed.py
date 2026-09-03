"""Tell + Computed 端到端测试。

覆盖：
- ``rfield(Tell())``：parse 返回流位置；build 不从实例取值（自动计算）
- ``rfield(Computed(end - start))``：parse/build 都通过表达式求值
- ``rfield(Computed(count * 2))``：引用 RW 字段
- Tell + Computed 组合往返
- 表达式引用前序 RO 字段（如 Computed(size) 引用另一个 Computed）
- 错误：Computed 缺少表达式 → CompilationError
- 错误：RO 字段 build 不从实例取值
"""

import pytest

from neoconstruct import (
    Bytes,
    Computed,
    CompilationError,
    Int8ub,
    Int16ub,
    StructMixin,
    Tell,
    field,
    rfield,
    wfield,
)
from dataclasses import dataclass


# ---------------------------------------------------------------------------
# Tell：基础位置记录
# ---------------------------------------------------------------------------


class TestTellBasic:
    """``rfield(Tell())`` 基础场景。"""

    def test_tell_at_start_returns_zero(self):
        """流开头 Tell() 返回 0。"""

        @dataclass
        class Msg(StructMixin):
            start: int = rfield(Tell())
            count: int = field(Int8ub)

        parsed = Msg.parse(b"\x05")
        assert parsed.start == 0
        assert parsed.count == 5

    def test_tell_after_field_returns_offset(self):
        """读完 1 字节后 Tell() 返回 1。"""

        @dataclass
        class Msg(StructMixin):
            count: int = field(Int8ub)
            end: int = rfield(Tell())

        parsed = Msg.parse(b"\x05")
        assert parsed.count == 5
        assert parsed.end == 1

    def test_tell_multiple_positions(self):
        """多个 Tell() 在不同位置返回正确的流位置。"""

        @dataclass
        class Msg(StructMixin):
            start: int = rfield(Tell())
            a: int = field(Int8ub)
            mid: int = rfield(Tell())
            b: int = field(Int16ub)
            end: int = rfield(Tell())

        # start=0, a=byte, mid=1, b=2 bytes, end=3
        parsed = Msg.parse(b"\x01\x02\x03")
        assert parsed.start == 0
        assert parsed.a == 1
        assert parsed.mid == 1
        assert parsed.b == 0x0203
        assert parsed.end == 3

    def test_tell_build_does_not_read_from_instance(self):
        """build：RO Tell 字段不从实例取值（实例无需提供该字段）。"""

        @dataclass
        class Msg(StructMixin):
            count: int = field(Int8ub)
            pos: int = rfield(Tell())

        # 实例只有 RW 字段（pos 由 build 自动计算）
        msg = Msg(count=5)
        built = msg.build()
        assert built == b"\x05"

    def test_tell_round_trip(self):
        """Tell 往返：parse → build。"""

        @dataclass
        class Msg(StructMixin):
            start: int = rfield(Tell())
            count: int = field(Int8ub)
            end: int = rfield(Tell())

        original = b"\x07"
        parsed = Msg.parse(original)
        assert parsed.start == 0
        assert parsed.count == 7
        assert parsed.end == 1

        # build：实例只需提供 count
        rebuilt = Msg(count=7).build()
        assert rebuilt == original


# ---------------------------------------------------------------------------
# Computed：基础表达式计算
# ---------------------------------------------------------------------------


class TestComputedBasic:
    """``rfield(Computed(expr))`` 基础场景。"""

    def test_computed_end_minus_start(self):
        """Computed(end - start)：字段差。"""

        @dataclass
        class Msg(StructMixin):
            start: int = rfield(Tell())
            count: int = field(Int8ub)
            end: int = rfield(Tell())
            size: int = rfield(Computed(end - start))

        parsed = Msg.parse(b"\x05")
        assert parsed.start == 0
        assert parsed.count == 5
        assert parsed.end == 1
        assert parsed.size == 1  # end - start = 1 - 0

    def test_computed_count_times_two(self):
        """Computed(count * 2)：引用 RW 字段。"""

        @dataclass
        class Msg(StructMixin):
            count: int = field(Int8ub)
            doubled: int = rfield(Computed(count * 2))

        parsed = Msg.parse(b"\x05")
        assert parsed.count == 5
        assert parsed.doubled == 10

    def test_computed_constant_via_expression(self):
        """Computed(count * 0 + 42)：通过表达式产生常量值。

        注意：``Computed(42)`` 直接传常量 int 不被支持（expr 必须是 FieldRef/ExprRef）。
        要产生常量值，需通过表达式（如 ``count * 0 + N``）。
        """

        @dataclass
        class Msg(StructMixin):
            count: int = field(Int8ub)
            magic: int = rfield(Computed(count * 0 + 42))

        parsed = Msg.parse(b"\x01")
        assert parsed.count == 1
        assert parsed.magic == 42

    def test_computed_build_does_not_read_from_instance(self):
        """build：RO Computed 字段不从实例取值。"""

        @dataclass
        class Msg(StructMixin):
            count: int = field(Int8ub)
            doubled: int = rfield(Computed(count * 2))

        # 实例只需提供 count
        msg = Msg(count=5)
        built = msg.build()
        assert built == b"\x05"

    def test_computed_round_trip(self):
        """Computed 往返：parse → build。"""

        @dataclass
        class Msg(StructMixin):
            count: int = field(Int8ub)
            squared: int = rfield(Computed(count * count))

        original = b"\x04"
        parsed = Msg.parse(original)
        assert parsed.count == 4
        assert parsed.squared == 16

        rebuilt = Msg(count=4).build()
        assert rebuilt == original


# ---------------------------------------------------------------------------
# Tell + Computed 组合
# ---------------------------------------------------------------------------


class TestTellComputedCombined:
    """Tell + Computed 组合场景。"""

    def test_packet_with_size_field(self):
        """完整 Packet：start, count, end, size。"""

        @dataclass
        class Packet(StructMixin):
            start: int = rfield(Tell())
            count: int = field(Int8ub)
            end: int = rfield(Tell())
            size: int = rfield(Computed(end - start))

        # parse
        parsed = Packet.parse(b"\x05")
        assert parsed.start == 0
        assert parsed.count == 5
        assert parsed.end == 1
        assert parsed.size == 1

        # build（RO 字段不从实例取值）
        rebuilt = Packet(count=5).build()
        assert rebuilt == b"\x05"

    def test_chained_computed_references(self):
        """表达式引用前序 RO 字段（Computed 引用另一个 Computed）。"""

        @dataclass
        class Msg(StructMixin):
            count: int = field(Int8ub)
            doubled: int = rfield(Computed(count * 2))
            quadrupled: int = rfield(Computed(doubled * 2))

        parsed = Msg.parse(b"\x03")
        assert parsed.count == 3
        assert parsed.doubled == 6
        assert parsed.quadrupled == 12

    def test_tell_referenced_by_computed(self):
        """Computed 引用 Tell 字段。"""

        @dataclass
        class Msg(StructMixin):
            a: int = field(Int8ub)
            pos: int = rfield(Tell())
            pos_again: int = rfield(Computed(pos))

        parsed = Msg.parse(b"\x07")
        assert parsed.a == 7
        assert parsed.pos == 1
        assert parsed.pos_again == 1  # == pos

    def test_full_round_trip_with_tell_and_computed(self):
        """完整往返：含 Tell + Computed 的复杂结构。"""

        @dataclass
        class Header(StructMixin):
            magic: int = field(Int8ub)
            start: int = rfield(Tell())
            length: int = field(Int8ub)
            end: int = rfield(Tell())
            span: int = rfield(Computed(end - start))

        original = b"\xaa\x03"
        parsed = Header.parse(original)
        assert parsed.magic == 0xAA
        assert parsed.start == 1
        assert parsed.length == 3
        assert parsed.end == 2
        assert parsed.span == 1

        rebuilt = Header(magic=0xAA, length=3).build()
        assert rebuilt == original


# ---------------------------------------------------------------------------
# 错误场景
# ---------------------------------------------------------------------------


class TestErrorCases:
    """编译期与运行时错误检测。"""

    def test_computed_without_expression_raises(self):
        """Computed 缺少表达式（直接用 None）→ CompilationError。

        构造一个 ComputedDescriptor 但 expr 是非 FieldRef/ExprRef 的常量值，
        _extract_and_compile_exprs 不会为常量生成 expr_programs，
        导致 Rust 侧找不到 "func" 键 → CompilationError。
        """

        with pytest.raises(CompilationError) as exc_info:

            @dataclass
            class Bad(StructMixin):
                # Computed(None)：expr 是 Python None，非 FieldRef/ExprRef，
                # _expr_params 返回 {"func": None}，但 _extract_and_compile_exprs
                # 不为 None（非 FieldRef/ExprRef/int）编译表达式 → expr_programs 为空
                # → Rust 侧 "func" 键缺失
                v: int = rfield(Computed(None))

        msg = str(exc_info.value)
        assert "func" in msg or "expression" in msg.lower() or "Computed" in msg

    def test_computed_forward_reference_raises(self):
        """Computed 前向引用 → CompilationError。"""

        count_field = field(Int8ub)  # 先创建 field 对象

        with pytest.raises(CompilationError) as exc_info:

            @dataclass
            class Bad(StructMixin):
                v: int = rfield(Computed(count_field))
                count: int = count_field  # 在 v 之后定义 → 前向引用

        assert "后序" in str(exc_info.value) or "forward" in str(
            exc_info.value
        ).lower()

    def test_ro_tell_field_not_in_init(self):
        """RO 字段不出现在 __init__ 参数中（init=False）。"""

        @dataclass
        class Msg(StructMixin):
            count: int = field(Int8ub)
            pos: int = rfield(Tell())

        # __init__ 只接受 count（pos 是 init=False）
        import inspect

        sig = inspect.signature(Msg.__init__)
        assert "count" in sig.parameters
        assert "pos" not in sig.parameters

    def test_ro_computed_field_not_in_init(self):
        """RO Computed 字段不出现在 __init__ 参数中。"""

        @dataclass
        class Msg(StructMixin):
            count: int = field(Int8ub)
            doubled: int = rfield(Computed(count * 2))

        import inspect

        sig = inspect.signature(Msg.__init__)
        assert "count" in sig.parameters
        assert "doubled" not in sig.parameters


# ---------------------------------------------------------------------------
# 边界场景
# ---------------------------------------------------------------------------


class TestEdgeCases:
    """边界与特殊场景。"""

    def test_only_tell_fields(self):
        """只有 Tell() 字段的结构。"""

        @dataclass
        class Msg(StructMixin):
            a: int = rfield(Tell())
            b: int = rfield(Tell())

        parsed = Msg.parse(b"")
        assert parsed.a == 0
        assert parsed.b == 0  # 没有字节消费，位置不变

        built = Msg().build()
        assert built == b""

    def test_tell_with_no_payload(self):
        """Tell 在空流上工作。"""

        @dataclass
        class Msg(StructMixin):
            pos: int = rfield(Tell())

        parsed = Msg.parse(b"")
        assert parsed.pos == 0

    def test_computed_with_bytes_field_reference(self):
        """Computed 引用 Bytes 表达式长度的字段。"""

        @dataclass
        class Msg(StructMixin):
            count: int = field(Int8ub)
            data: bytes = field(Bytes(count))
            data_len: int = rfield(Computed(count))

        parsed = Msg.parse(b"\x03ABC")
        assert parsed.count == 3
        assert parsed.data == b"ABC"
        assert parsed.data_len == 3  # == count

    def test_negative_computed_result(self):
        """Computed 结果为负数（合法）。"""

        @dataclass
        class Msg(StructMixin):
            a: int = field(Int8ub)
            b: int = field(Int8ub)
            diff: int = rfield(Computed(a - b))

        # a=1, b=5 → diff = 1 - 5 = -4
        parsed = Msg.parse(b"\x01\x05")
        assert parsed.a == 1
        assert parsed.b == 5
        assert parsed.diff == -4

    def test_tell_after_bytes_with_expr_length(self):
        """Tell 在 Bytes(表达式长度) 之后返回正确位置。"""

        @dataclass
        class Msg(StructMixin):
            count: int = field(Int8ub)
            data: bytes = field(Bytes(count))
            end: int = rfield(Tell())

        parsed = Msg.parse(b"\x04ABCD")
        assert parsed.count == 4
        assert parsed.data == b"ABCD"
        assert parsed.end == 5  # 1 (count) + 4 (data)
