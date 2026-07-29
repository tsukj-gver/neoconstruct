"""测试 Python 侧 field 函数与表达式类型系统（Phase 2 子任务 2.2）。

测试覆盖：
1. field/rfield/wfield 函数返回正确的 _FieldDescriptor（mode、default、context）
2. _FieldDescriptor 的属性、__set_name__、__class_getitem__
3. _ExprRef 表达式树构建（所有运算符重载）
4. _ExprRef 继续运算（嵌套表达式树）
5. _collect_field_descriptors 与 _apply_dataclass_field_config
6. dataclass 集成（RO init=False，default → kw_only）— 需要 Rust 扩展

设计依据：docs/design/模块设计/模块设计-表达式系统.md §2.1-§2.3
"""

import inspect
import operator
from dataclasses import dataclass

import pytest

from construct import StructMixin, field, rfield, wfield
from construct._mixin import (
    _MISSING,
    _ExprRef,
    _FieldDescriptor,
    _apply_dataclass_field_config,
    _collect_field_descriptors,
)

# 检查 Rust 扩展是否可用（Int8ub 来自 Rust pyclass）
try:
    from construct import Int8ub  # noqa: F401

    _HAS_RUST = True
except ImportError:
    _HAS_RUST = False

requires_rust = pytest.mark.skipif(
    not _HAS_RUST, reason="Rust extension not available"
)

# 测试用虚拟 subcon（field() 只存储 subcon，不检查类型）
_dummy = object()


# ======================================================================
# field / rfield / wfield 函数
# ======================================================================


class TestFieldFunctions:
    """测试 field/rfield/wfield 返回正确的 _FieldDescriptor。"""

    def test_field_returns_rw_mode(self):
        desc = field(_dummy)
        assert isinstance(desc, _FieldDescriptor)
        assert desc.mode == "rw"
        assert desc.subcon is _dummy

    def test_rfield_returns_ro_mode(self):
        desc = rfield(_dummy)
        assert isinstance(desc, _FieldDescriptor)
        assert desc.mode == "ro"
        assert desc.subcon is _dummy

    def test_wfield_returns_wo_mode(self):
        desc = wfield(_dummy)
        assert isinstance(desc, _FieldDescriptor)
        assert desc.mode == "wo"
        assert desc.subcon is _dummy

    def test_field_with_default(self):
        desc = field(_dummy, default=42)
        assert desc.default == 42

    def test_field_without_default(self):
        desc = field(_dummy)
        assert desc.default is _MISSING

    def test_wfield_with_default(self):
        desc = wfield(_dummy, default=0)
        assert desc.default == 0

    def test_wfield_without_default(self):
        desc = wfield(_dummy)
        assert desc.default is _MISSING

    def test_rfield_has_no_default_param(self):
        # rfield 不接受 default 参数（签名无此参数）
        with pytest.raises(TypeError):
            rfield(_dummy, default=1)  # type: ignore

    def test_field_with_context(self):
        ctx = {"key": "value"}
        desc = field(_dummy, context=ctx)
        assert desc._context_injections is ctx

    def test_rfield_with_context(self):
        ctx = {"key": "value"}
        desc = rfield(_dummy, context=ctx)
        assert desc._context_injections is ctx

    def test_wfield_with_context(self):
        ctx = {"key": "value"}
        desc = wfield(_dummy, context=ctx)
        assert desc._context_injections is ctx

    def test_field_default_none_is_not_missing(self):
        # field(subcon, default=None) 应存储 None，而非 _MISSING
        desc = field(_dummy, default=None)
        assert desc.default is None
        assert desc.default is not _MISSING

    def test_field_backward_compat(self):
        # Phase 1 用法：field(subcon) 仍然兼容（无 default、无 context）
        desc = field(_dummy)
        assert desc.mode == "rw"
        assert desc.default is _MISSING
        assert desc._context_injections is None


# ======================================================================
# _FieldDescriptor 属性
# ======================================================================


class TestFieldDescriptorAttributes:
    """测试 _FieldDescriptor 的属性和方法。"""

    def test_name_is_none_before_set_name(self):
        desc = field(_dummy)
        assert desc.name is None

    def test_set_name_fills_name(self):
        class _Owner:
            count = field(_dummy)

        assert _Owner.count.name == "count"

    def test_set_name_multiple_fields(self):
        class _Owner:
            alpha = field(_dummy)
            beta = rfield(_dummy)

        assert _Owner.alpha.name == "alpha"
        assert _Owner.beta.name == "beta"

    def test_repr_contains_name_mode_subcon(self):
        desc = field(_dummy)
        desc.name = "test_field"
        repr_str = repr(desc)
        assert "test_field" in repr_str
        assert "rw" in repr_str

    def test_repr_ro_field(self):
        desc = rfield(_dummy)
        desc.name = "crc"
        repr_str = repr(desc)
        assert "crc" in repr_str
        assert "ro" in repr_str

    def test_class_getitem_returns_class(self):
        result = _FieldDescriptor[int]
        assert result is _FieldDescriptor

    def test_class_getitem_str_arg(self):
        result = _FieldDescriptor[str]
        assert result is _FieldDescriptor

    def test_is_unhashable(self):
        # __eq__ 被重载 → __hash__ = None → 不可哈希
        desc = field(_dummy)
        with pytest.raises(TypeError):
            hash(desc)


# ======================================================================
# 运算符重载：_FieldDescriptor
# ======================================================================


class TestFieldDescriptorOperators:
    """测试 _FieldDescriptor 的算术运算符重载（§2.3.3）。"""

    def setup_method(self):
        self.count = field(_dummy)
        self.flag = field(_dummy)

    # --- 二元算术 ---
    def test_add_two_fields(self):
        expr = self.count + self.flag
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.add
        assert expr.lhs is self.count
        assert expr.rhs is self.flag

    def test_add_field_and_int(self):
        expr = self.count + 1
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.add
        assert expr.lhs is self.count
        assert expr.rhs == 1

    def test_radd_int_and_field(self):
        expr = 1 + self.count
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.add
        assert expr.lhs == 1
        assert expr.rhs is self.count

    def test_sub_two_fields(self):
        expr = self.count - self.flag
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.sub
        assert expr.lhs is self.count
        assert expr.rhs is self.flag

    def test_rsub(self):
        expr = 1 - self.count
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.sub
        assert expr.lhs == 1
        assert expr.rhs is self.count

    def test_mul_two_fields(self):
        expr = self.count * self.flag
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.mul

    def test_rmul(self):
        expr = 2 * self.count
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.mul
        assert expr.lhs == 2

    def test_floordiv(self):
        expr = self.count // self.flag
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.floordiv

    def test_rfloordiv(self):
        expr = 10 // self.count
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.floordiv
        assert expr.lhs == 10

    def test_mod(self):
        expr = self.count % self.flag
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.mod

    def test_rmod(self):
        expr = 10 % self.count
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.mod
        assert expr.lhs == 10

    # --- 位运算 ---
    def test_bitand(self):
        expr = self.count & self.flag
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.and_

    def test_bitor(self):
        expr = self.count | self.flag
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.or_

    def test_bitxor(self):
        expr = self.count ^ self.flag
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.xor

    def test_lshift(self):
        expr = self.count << 1
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.lshift
        assert expr.rhs == 1

    def test_rshift(self):
        expr = self.count >> 1
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.rshift
        assert expr.rhs == 1

    # --- 一元运算 ---
    def test_neg(self):
        expr = -self.count
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.neg
        assert expr.lhs is self.count
        assert expr.rhs is None

    def test_pos_returns_self(self):
        expr = +self.count
        assert expr is self.count

    def test_invert(self):
        expr = ~self.count
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.invert
        assert expr.lhs is self.count
        assert expr.rhs is None

    # --- 比较运算 ---
    def test_eq(self):
        expr = self.count == self.flag
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.eq

    def test_ne(self):
        expr = self.count != self.flag
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.ne

    def test_lt(self):
        expr = self.count < self.flag
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.lt

    def test_le(self):
        expr = self.count <= self.flag
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.le

    def test_gt(self):
        expr = self.count > self.flag
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.gt

    def test_ge(self):
        expr = self.count >= self.flag
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.ge


# ======================================================================
# 运算符重载：_ExprRef（表达式继续运算）
# ======================================================================


class TestExprRefOperators:
    """测试 _ExprRef 的算术运算符重载。"""

    def setup_method(self):
        self.count = field(_dummy)
        self.flag = field(_dummy)
        self.inner = self.count + self.flag

    def test_expr_ref_add(self):
        expr = self.inner + 1
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.add
        assert expr.lhs is self.inner
        assert expr.rhs == 1

    def test_expr_ref_radd(self):
        expr = 1 + self.inner
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.add
        assert expr.lhs == 1
        assert expr.rhs is self.inner

    def test_expr_ref_sub(self):
        expr = self.inner - self.flag
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.sub

    def test_expr_ref_mul(self):
        expr = self.inner * 2
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.mul

    def test_expr_ref_floordiv(self):
        expr = self.inner // 2
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.floordiv

    def test_expr_ref_mod(self):
        expr = self.inner % 3
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.mod

    def test_expr_ref_bitand(self):
        expr = self.inner & 0xFF
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.and_

    def test_expr_ref_bitor(self):
        expr = self.inner | 0x10
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.or_

    def test_expr_ref_neg(self):
        expr = -self.inner
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.neg
        assert expr.lhs is self.inner
        assert expr.rhs is None

    def test_expr_ref_invert(self):
        expr = ~self.inner
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.invert

    def test_expr_ref_eq(self):
        expr = self.inner == 0
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.eq

    def test_expr_ref_lt(self):
        expr = self.inner < 10
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.lt

    def test_expr_ref_class_getitem(self):
        result = _ExprRef[int]
        assert result is _ExprRef

    def test_expr_ref_repr_shows_op_name(self):
        assert "add" in repr(self.inner)

    def test_expr_ref_is_unhashable(self):
        with pytest.raises(TypeError):
            hash(self.inner)


# ======================================================================
# 嵌套表达式树
# ======================================================================


class TestNestedExpressions:
    """测试嵌套表达式树的构建。"""

    def test_nested_mul(self):
        count = field(_dummy)
        flag = field(_dummy)
        expr = (count + flag) * 2
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.mul
        assert expr.rhs == 2
        inner = expr.lhs
        assert isinstance(inner, _ExprRef)
        assert inner.op is operator.add
        assert inner.lhs is count
        assert inner.rhs is flag

    def test_nested_sub_after_add(self):
        count = field(_dummy)
        flag = field(_dummy)
        expr = (count + flag) - 1
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.sub
        assert expr.rhs == 1
        inner = expr.lhs
        assert isinstance(inner, _ExprRef)
        assert inner.op is operator.add

    def test_deep_nesting(self):
        a = field(_dummy)
        b = field(_dummy)
        c = field(_dummy)
        expr = ((a + b) * c) - 1
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.sub
        assert expr.rhs == 1

        mul_node = expr.lhs
        assert isinstance(mul_node, _ExprRef)
        assert mul_node.op is operator.mul
        assert mul_node.rhs is c

        add_node = mul_node.lhs
        assert isinstance(add_node, _ExprRef)
        assert add_node.op is operator.add
        assert add_node.lhs is a
        assert add_node.rhs is b

    def test_comparison_with_arithmetic(self):
        count = field(_dummy)
        expr = (count + 1) > 0
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.gt
        assert expr.rhs == 0
        inner = expr.lhs
        assert isinstance(inner, _ExprRef)
        assert inner.op is operator.add

    def test_unary_in_nested(self):
        count = field(_dummy)
        flag = field(_dummy)
        # (-count) + flag
        expr = (-count) + flag
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.add
        assert expr.rhs is flag
        neg_node = expr.lhs
        assert isinstance(neg_node, _ExprRef)
        assert neg_node.op is operator.neg
        assert neg_node.rhs is None

    def test_expr_ref_continue_from_unary(self):
        count = field(_dummy)
        # ~count + 1
        expr = (~count) + 1
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.add
        assert expr.rhs == 1
        inv_node = expr.lhs
        assert isinstance(inv_node, _ExprRef)
        assert inv_node.op is operator.invert


# ======================================================================
# FieldRef 引用语义
# ======================================================================


class TestFieldRefSemantics:
    """测试 FieldRef 引用语义（字段名在类体中是 _FieldDescriptor 对象）。"""

    def test_field_ref_is_descriptor_object(self):
        # 在类体中，count 是 _FieldDescriptor 对象（不是 int 值）
        class _Owner:
            count: int = field(_dummy)

        # 类属性 count 取到的是 _FieldDescriptor 对象
        assert isinstance(_Owner.count, _FieldDescriptor)
        assert _Owner.count.name == "count"

    def test_field_ref_used_in_expression(self):
        # 模拟 Bytes(count) 的场景：count 作为表达式参数
        class _Owner:
            count: int = field(_dummy)
            flag: int = field(_dummy)

        # count + flag 创建表达式树
        expr = _Owner.count + _Owner.flag
        assert isinstance(expr, _ExprRef)
        assert expr.op is operator.add
        assert expr.lhs is _Owner.count
        assert expr.rhs is _Owner.flag

    def test_descriptor_identity_stable(self):
        # 同一字段的 _FieldDescriptor 对象 identity 不变
        class _Owner:
            count: int = field(_dummy)

        assert _Owner.count is _Owner.count  # 同一对象


# ======================================================================
# _collect_field_descriptors
# ======================================================================


class TestCollectFieldDescriptors:
    """测试字段描述符收集逻辑。"""

    def test_collect_returns_full_descriptors(self):
        class _Owner:
            a: int = field(_dummy)
            b: int = rfield(_dummy)
            c: int = wfield(_dummy, default=0)

        descriptors = _collect_field_descriptors(_Owner)
        assert len(descriptors) == 3
        assert descriptors[0][0] == "a"
        assert descriptors[0][1].mode == "rw"
        assert descriptors[1][0] == "b"
        assert descriptors[1][1].mode == "ro"
        assert descriptors[2][0] == "c"
        assert descriptors[2][1].mode == "wo"
        assert descriptors[2][1].default == 0

    def test_collect_preserves_declaration_order(self):
        class _Owner:
            zebra: int = field(_dummy)
            alpha: int = field(_dummy)
            mango: int = field(_dummy)

        descriptors = _collect_field_descriptors(_Owner)
        names = [name for name, _ in descriptors]
        assert names == ["zebra", "alpha", "mango"]

    def test_collect_skips_non_descriptors(self):
        class _Owner:
            real_field: int = field(_dummy)
            not_a_field = 42
            also_not: str = "hello"

        descriptors = _collect_field_descriptors(_Owner)
        assert len(descriptors) == 1
        assert descriptors[0][0] == "real_field"

    def test_collect_returns_descriptor_not_subcon(self):
        # 返回完整的 _FieldDescriptor 对象（含 mode/default），而非仅 subcon
        class _Owner:
            x: int = rfield(_dummy)

        descriptors = _collect_field_descriptors(_Owner)
        assert isinstance(descriptors[0][1], _FieldDescriptor)
        assert descriptors[0][1].mode == "ro"


# ======================================================================
# _apply_dataclass_field_config
# ======================================================================


class TestApplyDataclassFieldConfig:
    """测试 dataclass 字段配置注入（§2.2.1）。"""

    def test_ro_field_becomes_init_false(self):
        import dataclasses

        class _Owner:
            x: int = field(_dummy)
            y: int = rfield(_dummy)

        descriptors = _collect_field_descriptors(_Owner)
        _apply_dataclass_field_config(_Owner, descriptors)

        # x 替换为 dataclasses.field()（RW 无 default → 必填）
        assert isinstance(_Owner.x, dataclasses.Field)
        # y 被替换为 dataclasses.field（init=False）
        assert isinstance(_Owner.y, dataclasses.Field)
        assert _Owner.y.init is False

    def test_rw_with_default_becomes_kw_only(self):
        import dataclasses

        class _Owner:
            x: int = field(_dummy, default=42)

        descriptors = _collect_field_descriptors(_Owner)
        _apply_dataclass_field_config(_Owner, descriptors)

        assert isinstance(_Owner.x, dataclasses.Field)
        assert _Owner.x.kw_only is True

    def test_wo_with_default_becomes_kw_only(self):
        import dataclasses

        class _Owner:
            pad: int = wfield(_dummy, default=0)

        descriptors = _collect_field_descriptors(_Owner)
        _apply_dataclass_field_config(_Owner, descriptors)

        assert isinstance(_Owner.pad, dataclasses.Field)
        assert _Owner.pad.kw_only is True

    def test_rw_without_default_becomes_required(self):
        import dataclasses

        class _Owner:
            x: int = field(_dummy)

        descriptors = _collect_field_descriptors(_Owner)
        _apply_dataclass_field_config(_Owner, descriptors)

        # 替换为 dataclasses.field()（无默认值，必填 positional）
        assert isinstance(_Owner.x, dataclasses.Field)
        assert _Owner.x.default is dataclasses.MISSING

    def test_wo_without_default_becomes_required(self):
        import dataclasses

        class _Owner:
            pad: int = wfield(_dummy)

        descriptors = _collect_field_descriptors(_Owner)
        _apply_dataclass_field_config(_Owner, descriptors)

        assert isinstance(_Owner.pad, dataclasses.Field)
        assert _Owner.pad.default is dataclasses.MISSING

    def test_mixed_modes(self):
        import dataclasses

        class _Owner:
            a: int = field(_dummy)
            b: int = rfield(_dummy)
            c: int = field(_dummy, default=1)
            d: int = wfield(_dummy, default=0)

        descriptors = _collect_field_descriptors(_Owner)
        _apply_dataclass_field_config(_Owner, descriptors)

        # a: RW 无 default → dataclasses.field()（必填）
        assert isinstance(_Owner.a, dataclasses.Field)
        assert _Owner.a.default is dataclasses.MISSING
        # b: RO → init=False
        assert isinstance(_Owner.b, dataclasses.Field)
        assert _Owner.b.init is False
        # c: RW 有 default → kw_only
        assert isinstance(_Owner.c, dataclasses.Field)
        assert _Owner.c.kw_only is True
        # d: WO 有 default → kw_only
        assert isinstance(_Owner.d, dataclasses.Field)
        assert _Owner.d.kw_only is True

