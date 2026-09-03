"""@dataclass 与 StructMixin 集成测试（从 test_field_types.py 拆分）。

这些测试需要 Rust 扩展（__init_subclass__ 调用 compile_schema），验证 @dataclass
与 field/rfield/wfield/default/kw_only 的集成行为。
"""

import inspect
from dataclasses import dataclass

import pytest

from neoconstruct import StructMixin, field, rfield, wfield

# 检查 Rust 扩展是否可用（Int8ub 来自 Rust pyclass）
try:
    from neoconstruct import Int8ub  # noqa: F401

    _HAS_RUST = True
except ImportError:
    _HAS_RUST = False

requires_rust = pytest.mark.skipif(
    not _HAS_RUST, reason="Rust extension not available"
)


@requires_rust
class TestDataclassIntegration:
    """测试 @dataclass 与 StructMixin 的集成。

    这些测试需要 Rust 扩展（__init_subclass__ 调用 compile_schema）。
    """

    def test_ro_field_excluded_from_init(self):
        @dataclass
        class Packet(StructMixin):
            x: int = field(Int8ub)
            y: int = rfield(Int8ub)

        sig = inspect.signature(Packet.__init__)
        assert "x" in sig.parameters
        assert "y" not in sig.parameters

    def test_default_field_is_kw_only(self):
        @dataclass
        class Packet(StructMixin):
            x: int = field(Int8ub)
            version: int = field(Int8ub, default=1)

        sig = inspect.signature(Packet.__init__)
        version_param = sig.parameters["version"]
        assert version_param.kind == inspect.Parameter.KEYWORD_ONLY

    def test_default_field_uses_default_value(self):
        @dataclass
        class Packet(StructMixin):
            x: int = field(Int8ub)
            version: int = field(Int8ub, default=1)

        p = Packet(x=5)
        assert p.x == 5
        assert p.version == 1

    def test_default_field_overridable(self):
        @dataclass
        class Packet(StructMixin):
            x: int = field(Int8ub)
            version: int = field(Int8ub, default=1)

        p = Packet(x=5, version=2)
        assert p.version == 2

    def test_rfield_before_field_no_error(self):
        @dataclass
        class Packet(StructMixin):
            crc: int = rfield(Int8ub)
            data: int = field(Int8ub)

        sig = inspect.signature(Packet.__init__)
        assert "data" in sig.parameters
        assert "crc" not in sig.parameters

    def test_wfield_in_init(self):
        @dataclass
        class Packet(StructMixin):
            x: int = field(Int8ub)
            pad: int = wfield(Int8ub, default=0)

        sig = inspect.signature(Packet.__init__)
        assert "pad" in sig.parameters
        pad_param = sig.parameters["pad"]
        assert pad_param.kind == inspect.Parameter.KEYWORD_ONLY

    def test_mixed_modes_full(self):
        @dataclass
        class Packet(StructMixin):
            count: int = field(Int8ub)
            crc: int = rfield(Int8ub)
            reserved: int = wfield(Int8ub, default=0)
            version: int = field(Int8ub, default=1)

        sig = inspect.signature(Packet.__init__)
        # count 和 reserved/version 在 init 中
        assert "count" in sig.parameters
        assert "reserved" in sig.parameters
        assert "version" in sig.parameters
        # crc (RO) 不在 init 中
        assert "crc" not in sig.parameters

        # count 是 positional
        count_param = sig.parameters["count"]
        assert count_param.kind in (
            inspect.Parameter.POSITIONAL_OR_KEYWORD,
            inspect.Parameter.POSITIONAL_ONLY,
        )

        # reserved 和 version 是 keyword-only
        assert sig.parameters["reserved"].kind == inspect.Parameter.KEYWORD_ONLY
        assert sig.parameters["version"].kind == inspect.Parameter.KEYWORD_ONLY

    def test_backward_compat_early_pattern(self):
        # 早期用法仍然兼容
        @dataclass
        class SimpleMsg(StructMixin):
            address: int = field(Int8ub)

        msg = SimpleMsg(address=42)
        assert msg.address == 42
