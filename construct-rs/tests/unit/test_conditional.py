"""Conditional 构造器 Python 用户面单元测试。

测试范围：
1. Descriptor 构造器属性
2. _expr_params 协议（编译期分类）
3. Renamed 包装器
4. 错误路径（复杂表达式拒绝）
5. （如 Rust 扩展可用）端到端 parse/build

需要 Rust 扩展的测试用 ``requires_rust`` 标记跳过。
"""

import pytest

from construct import (
    ExplicitError,
    SelectError,
    If,
    IfThenElse,
    IfThenElseDescriptor,
    Switch,
    SwitchDescriptor,
    Select,
    SelectDescriptor,
    FocusedSeq,
    FocusedSeqDescriptor,
    Renamed,
    Pass,
    Int8ub,
    Int16ub,
)
from construct._mixin import _FieldDescriptor, _ExprRef

# 检查 Rust 扩展是否可用
try:
    from construct import StructMixin, field  # noqa: F401

    _HAS_RUST = True
except ImportError:
    _HAS_RUST = False

requires_rust = pytest.mark.skipif(
    not _HAS_RUST, reason="Rust extension not available"
)


# ============================================================================
# 异常类导出
# ============================================================================


class TestExplicitSelectError:
    """ExplicitError / SelectError 异常类导出。"""

    def test_explicit_error_class_exists(self):
        """_errors.py 含 ExplicitError 类定义。"""
        assert ExplicitError.__name__ == "ExplicitError"
        # 继承自 ConstructError
        from construct import ConstructError

        assert issubclass(ExplicitError, ConstructError)

    def test_select_error_class_exists(self):
        """_errors.py 含 SelectError 类定义。"""
        assert SelectError.__name__ == "SelectError"
        from construct import ConstructError

        assert issubclass(SelectError, ConstructError)

    def test_explicit_error_carries_message_and_path(self):
        e = ExplicitError("user raised", "root.x")
        assert e.message == "user raised"
        assert e.path == "root.x"
        assert "user raised" in str(e)
        assert "root.x" in str(e)

    def test_select_error_carries_message_and_path(self):
        e = SelectError("no subconstruct matched", "root")
        assert e.message == "no subconstruct matched"
        assert e.path == "root"


# ============================================================================
# IfThenElseDescriptor
# ============================================================================


class TestIfThenElseDescriptor:
    """IfThenElseDescriptor 属性与 _expr_params 协议。"""

    def test_constructor_stores_attributes(self):
        desc = IfThenElseDescriptor(True, Int8ub, Int16ub)
        assert desc.condfunc is True
        assert desc.thensubcon is Int8ub
        assert desc.elsesubcon is Int16ub

    def test_expr_params_empty_for_bool_cond(self):
        """bool 常量 cond → _expr_params = {}（跳过编译）。"""
        desc = IfThenElseDescriptor(True, Int8ub, Int16ub)
        assert desc._expr_params == {}

        desc2 = IfThenElseDescriptor(False, Int8ub, Int16ub)
        assert desc2._expr_params == {}

    def test_expr_params_dict_for_expr_cond(self):
        """FieldRef/ExprRef cond → _expr_params = {"cond": <expr>}。"""
        # 用 _FieldDescriptor 模拟表达式（不需要真实字段名）
        expr = _FieldDescriptor(Int8ub, mode="rw")
        desc = IfThenElseDescriptor(expr, Int8ub, Int16ub)
        assert desc._expr_params == {"cond": expr}

    def test_factory_function_returns_descriptor(self):
        desc = IfThenElse(True, Int8ub, Int16ub)
        assert isinstance(desc, IfThenElseDescriptor)


# ============================================================================
# If macro
# ============================================================================


class TestIfMacro:
    """If macro：If(cond, sub) ≡ IfThenElse(cond, sub, Pass)。"""

    def test_if_returns_if_then_else_with_pass_else(self):
        desc = If(True, Int8ub)
        assert isinstance(desc, IfThenElseDescriptor)
        assert desc.condfunc is True
        assert desc.thensubcon is Int8ub
        # else 应为 Pass
        assert desc.elsesubcon is Pass


# ============================================================================
# SwitchDescriptor
# ============================================================================


class TestSwitchDescriptor:
    """SwitchDescriptor 属性与 _expr_params 协议。"""

    def test_constructor_stores_attributes(self):
        desc = SwitchDescriptor(2, {1: Int8ub, 2: Int16ub})
        assert desc.keyfunc == 2
        assert desc.cases == {1: Int8ub, 2: Int16ub}
        # default=None → Pass
        assert desc.default is Pass

    def test_constructor_default_explicit(self):
        desc = SwitchDescriptor(99, {}, default=Int8ub)
        assert desc.default is Int8ub

    def test_constructor_rejects_callable(self):
        """callable keyfunc（lambda）→ CompilationError。"""
        from construct import CompilationError

        with pytest.raises(CompilationError):
            SwitchDescriptor(lambda ctx: 1, {})

    def test_expr_params_empty_for_int_constant(self):
        desc = SwitchDescriptor(2, {1: Int8ub})
        assert desc._expr_params == {}

    def test_expr_params_empty_for_bool_constant(self):
        desc = SwitchDescriptor(True, {1: Int8ub})
        assert desc._expr_params == {}

    def test_expr_params_key_for_field_ref(self):
        expr = _FieldDescriptor(Int8ub, mode="rw")
        desc = SwitchDescriptor(expr, {1: Int8ub})
        assert desc._expr_params == {"key": expr}

    def test_factory_function_returns_descriptor(self):
        desc = Switch(2, {1: Int8ub})
        assert isinstance(desc, SwitchDescriptor)


# ============================================================================
# SelectDescriptor
# ============================================================================


class TestSelectDescriptor:
    """SelectDescriptor 属性。"""

    def test_constructor_stores_subcons(self):
        desc = SelectDescriptor([Int8ub, Int16ub])
        assert desc.subcons == [Int8ub, Int16ub]

    def test_factory_function_returns_descriptor(self):
        desc = Select(Int8ub, Int16ub)
        assert isinstance(desc, SelectDescriptor)
        assert len(desc.subcons) == 2


# ============================================================================
# FocusedSeqDescriptor + Renamed
# ============================================================================


class TestFocusedSeqDescriptor:
    """FocusedSeqDescriptor 属性。"""

    def test_constructor_stores_attributes(self):
        desc = FocusedSeqDescriptor(
            "num",
            [Renamed("num", Int8ub), Int16ub],
        )
        assert desc.parsebuildfrom == "num"
        assert len(desc.subcons) == 2

    def test_constructor_rejects_non_string_parsebuildfrom(self):
        """parsebuildfrom 必须是 str（lambda 不支持）。"""
        from construct import CompilationError

        with pytest.raises(CompilationError):
            FocusedSeqDescriptor(lambda ctx: "num", [Int8ub])

    def test_factory_function_returns_descriptor(self):
        desc = FocusedSeq("num", Renamed("num", Int8ub))
        assert isinstance(desc, FocusedSeqDescriptor)


class TestRenamed:
    """Renamed 包装器。"""

    def test_constructor_stores_name_and_subcon(self):
        r = Renamed("foo", Int8ub)
        assert r.name == "foo"
        assert r.subcon is Int8ub

    def test_repr_format(self):
        r = Renamed("foo", Int8ub)
        s = repr(r)
        assert "foo" in s
        assert "Int8ub" in s or "FormatField" in s


# ============================================================================
# 端到端测试（需 Rust 扩展）
# ============================================================================


@requires_rust
class TestEndToEndIfThenElse:
    """IfThenElse 端到端 parse/build（需 Rust 扩展）。"""

    def test_constant_true_uses_then(self):
        from dataclasses import dataclass

        from construct import StructMixin, field

        @dataclass
        class P(StructMixin):
            v: int = field(IfThenElse(True, Int8ub, Int16ub))

        parsed = P.parse(b"\x42")
        assert parsed.v == 0x42

        built = P(v=0x42).build()
        assert built == b"\x42"

    def test_constant_false_uses_else(self):
        from dataclasses import dataclass

        from construct import StructMixin, field

        @dataclass
        class P(StructMixin):
            v: int = field(IfThenElse(False, Int8ub, Int16ub))

        parsed = P.parse(b"\xAA\xBB")
        assert parsed.v == 0xAABB

        built = P(v=0xAABB).build()
        assert built == b"\xAA\xBB"


@requires_rust
class TestEndToEndSwitch:
    """Switch 端到端 parse/build（需 Rust 扩展）。"""

    def test_int_constant_key_matches(self):
        from dataclasses import dataclass

        from construct import StructMixin, field

        @dataclass
        class P(StructMixin):
            v: int = field(Switch(2, {1: Int8ub, 2: Int16ub}))

        parsed = P.parse(b"\xAA\xBB")
        assert parsed.v == 0xAABB

    def test_no_match_falls_to_pass_default(self):
        from dataclasses import dataclass

        from construct import StructMixin, field

        @dataclass
        class P(StructMixin):
            v: int = field(Switch(99, {1: Int8ub, 2: Int16ub}))

        # default=Pass → parse 返回 None
        parsed = P.parse(b"")
        assert parsed.v is None


@requires_rust
class TestEndToEndSelect:
    """Select 端到端 parse/build（需 Rust 扩展）。"""

    def test_first_success(self):
        from dataclasses import dataclass

        from construct import StructMixin, field

        @dataclass
        class P(StructMixin):
            v: int = field(Select(Int8ub, Int16ub))

        parsed = P.parse(b"\x42")
        assert parsed.v == 0x42

    def test_second_success_after_first_fail(self):
        from dataclasses import dataclass

        from construct import StructMixin, field

        @dataclass
        class P(StructMixin):
            v: int = field(Select(Int16ub, Int8ub))

        # Int16ub 需要 2 字节，仅 1 字节可用 → 失败；Int8ub 成功
        parsed = P.parse(b"\x42")
        assert parsed.v == 0x42


@requires_rust
class TestEndToEndFocusedSeq:
    """FocusedSeq 端到端 parse/build（需 Rust 扩展）。"""

    def test_basic_parse_returns_focus_field(self):
        from dataclasses import dataclass

        from construct import StructMixin, field
        from construct import Bytes

        @dataclass
        class P(StructMixin):
            v: int = field(FocusedSeq(
                "num",
                Bytes(3),                # 匿名 "SIG" 前缀
                Renamed("num", Int8ub),  # focus 字段
            ))

        # b"SIG\xff" → Bytes(3) 消费 "SIG"，Int8ub 解析 0xFF=255
        parsed = P.parse(b"SIG\xff")
        assert parsed.v == 0xFF
