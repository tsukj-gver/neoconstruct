"""编译期错误测试。

覆盖：
    - 拼写错误：``Bytes(cout)`` → CompilationError
    - 前向引用：``data`` 在 ``later`` 之前声明但引用 ``later`` → CompilationError
    - WO 字段引用：表达式引用 WO 字段 → CompilationError
    - 不支持的运算符（浮点常量）：``Bytes(2.5)`` → CompilationError
"""

import pytest

from neoconstruct import (
    Bytes,
    CompilationError,
    Computed,
    Int8ub,
    StructMixin,
    field,
    rfield,
    wfield,
)
from dataclasses import dataclass


class TestCompileErrors:
    """编译期错误检测。"""

    def test_typo_in_field_reference_raises(self):
        """拼写错误：``Bytes(cout)`` 中 ``cout`` 不是已声明字段 → CompilationError。

        ``cout`` 在类体中未声明，Python 会抛 ``NameError``（在类体执行阶段）。
        """
        with pytest.raises(NameError):
            # count 拼写为 cout → NameError（cout 未定义）
            @dataclass
            class Bad(StructMixin):  # noqa: F841
                count: int = field(Int8ub)
                data: bytes = field(Bytes(cout))  # type: ignore[name-defined]  # noqa: F821

    def test_forward_reference_raises(self):
        """前向引用：``data`` 在 ``later`` 之前声明但引用 ``later`` → CompilationError。

        通过提前创建 field 对象放在后序位置来模拟前向引用。
        """
        later_field = field(Int8ub)  # 先创建 field 对象

        with pytest.raises(CompilationError) as exc_info:

            @dataclass
            class Bad(StructMixin):  # noqa: F841
                data: bytes = field(Bytes(later_field))
                later: int = later_field  # later 在 data 之后定义

        msg = str(exc_info.value)
        assert "后序" in msg or "forward" in msg.lower()

    def test_wo_field_reference_raises(self):
        """WO 字段引用：表达式引用 WO 字段 → CompilationError。"""
        with pytest.raises(CompilationError) as exc_info:

            @dataclass
            class Bad(StructMixin):  # noqa: F841
                count: int = wfield(Int8ub)
                data: bytes = field(Bytes(count))

        assert "WO" in str(exc_info.value)

    def test_self_reference_raises_nameerror(self):
        """自引用：``data`` 在自身定义行引用 ``data`` → Python ``NameError``。

        ``data: bytes = field(Bytes(data))`` 中 RHS 先求值，此时 ``data`` 尚未绑定，
        Python 直接抛 ``NameError``。前向引用检查（编译期）的等价场景由
        ``test_forward_reference_raises`` 覆盖。
        """
        with pytest.raises(NameError):

            @dataclass
            class Bad(StructMixin):  # noqa: F841
                count: int = field(Int8ub)
                data: bytes = field(Bytes(data))  # type: ignore[name-defined]  # noqa: F821

    def test_float_constant_in_bytes_raises(self):
        """浮点常量：``Bytes(2.5)`` → CompilationError（VM 栈为 i64）。

        ``Bytes(2.5)`` 传入 float，在编译期表达式翻译时被拒绝。
        """
        with pytest.raises(CompilationError):

            @dataclass
            class Bad(StructMixin):  # noqa: F841
                count: int = field(Int8ub)
                data: bytes = field(Bytes(2.5))  # type: ignore[arg-type]

    def test_float_constant_in_arithmetic_raises(self):
        """浮点常量在算术表达式中：``Bytes(count + 1.5)`` → CompilationError。"""
        with pytest.raises(CompilationError):

            @dataclass
            class Bad(StructMixin):  # noqa: F841
                count: int = field(Int8ub)
                data: bytes = field(Bytes(count + 1.5))

    def test_computed_without_expression_raises(self):
        """Computed 缺少表达式 → CompilationError。"""
        with pytest.raises(CompilationError):

            @dataclass
            class Bad(StructMixin):  # noqa: F841
                count: int = field(Int8ub)
                magic: int = rfield(Computed(None))

    def test_computed_forward_reference_raises(self):
        """Computed 前向引用 → CompilationError。"""
        count_field = field(Int8ub)

        with pytest.raises(CompilationError):

            @dataclass
            class Bad(StructMixin):  # noqa: F841
                doubled: int = rfield(Computed(count_field * 2))
                count: int = count_field
