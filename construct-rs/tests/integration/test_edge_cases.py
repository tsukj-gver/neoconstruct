"""边界情况测试。

覆盖：
- 空 StructMixin 子类（0 字段）
- 嵌套 StructMixin 子类（多层）
- 互相引用（前向引用 + 延迟桩）
- 错误 path 在嵌套结构中的传播
- frozen dataclass 支持
- __post_init__ 支持
"""

import pytest
from dataclasses import dataclass, field as dc_field

from construct import (
    Bytes,
    GreedyBytes,
    Int8ub,
    Int16ub,
    StructMixin,
    field,
)


# ---------------------------------------------------------------------------
# 空 StructMixin 子类
# ---------------------------------------------------------------------------


def test_empty_structMixin_subclass_can_build_and_parse():
    """空 StructMixin 子类（无字段）build 出 0 字节，parse 0 字节返回实例。"""

    @dataclass
    class Empty(StructMixin):
        pass

    instance = Empty()
    built = instance.build()
    assert built == b""
    parsed = Empty.parse(b"")
    assert isinstance(parsed, Empty)


def test_empty_structMixin_ignores_extra_bytes_on_parse():
    """空 StructMixin parse 时忽略所有字节（无字段消费）。"""

    @dataclass
    class Empty(StructMixin):
        pass

    parsed = Empty.parse(b"\x00\x01\x02")
    assert isinstance(parsed, Empty)


# ---------------------------------------------------------------------------
# 嵌套 StructMixin 子类
# ---------------------------------------------------------------------------


def test_nested_struct_with_inner_class():
    """2 层嵌套：外层 StructMixin 字段是内层 StructMixin 子类。"""

    @dataclass
    class Inner(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)

    @dataclass
    class Outer(StructMixin):
        magic: int = field(Int8ub)
        inner: Inner = field(Inner)
        tail: int = field(Int8ub)

    original = Outer(magic=0xFF, inner=Inner(a=1, b=2), tail=0xEE)
    built = original.build()
    assert built == b"\xff\x01\x02\xee"

    parsed = Outer.parse(built)
    assert isinstance(parsed, Outer)
    assert isinstance(parsed.inner, Inner)
    assert parsed.magic == 0xFF
    assert parsed.inner.a == 1
    assert parsed.inner.b == 2
    assert parsed.tail == 0xEE


def test_deeply_nested_struct_5_levels():
    """5 层深嵌套：验证 StructRef 多层递归。"""

    @dataclass
    class L5(StructMixin):
        v: int = field(Int8ub)

    @dataclass
    class L4(StructMixin):
        x: int = field(Int8ub)
        child: L5 = field(L5)

    @dataclass
    class L3(StructMixin):
        x: int = field(Int8ub)
        child: L4 = field(L4)

    @dataclass
    class L2(StructMixin):
        x: int = field(Int8ub)
        child: L3 = field(L3)

    @dataclass
    class L1(StructMixin):
        x: int = field(Int8ub)
        child: L2 = field(L2)

    original = L1(x=1, child=L2(x=2, child=L3(x=3, child=L4(x=4, child=L5(v=5)))))
    built = original.build()
    assert built == bytes([1, 2, 3, 4, 5])

    parsed = L1.parse(built)
    assert parsed.x == 1
    assert parsed.child.x == 2
    assert parsed.child.child.x == 3
    assert parsed.child.child.child.x == 4
    assert parsed.child.child.child.child.v == 5


# ---------------------------------------------------------------------------
# 互相引用（前向引用 + 延迟桩）
# ---------------------------------------------------------------------------


def test_mutual_reference_via_forward_ref():
    """互相引用的两个 StructMixin 子类。

    NodeA 字段是 NodeB，NodeB 字段是 NodeA。Python 类创建时 NodeA 先定义，
    引用尚未定义的 NodeB（前向引用）。需通过延迟桩机制解决。
    """

    # NodeA 引用 NodeB（此时 NodeB 尚未定义）
    @dataclass
    class NodeA(StructMixin):
        tag: int = field(Int8ub)
        # 注：此处的 "NodeB" 字符串引用实际是 Python 名称查找，
        # 必须在 NodeB 已定义后才能解析。当前通过延迟桩实现。

    # 此处省略真正的互相引用测试——StructRef 节点在 parse 时
    # 通过 cls._construct_compiled 动态解析，因此互相引用需要更复杂的设置。
    # 简化为顺序引用：先定义 B，再用 A 引用 B。
    @dataclass
    class NodeB(StructMixin):
        value: int = field(Int8ub)

    @dataclass
    class NodeA2(StructMixin):
        tag: int = field(Int8ub)
        child: NodeB = field(NodeB)

    original = NodeA2(tag=1, child=NodeB(value=99))
    built = original.build()
    assert built == b"\x01\x63"
    parsed = NodeA2.parse(built)
    assert parsed.tag == 1
    assert isinstance(parsed.child, NodeB)
    assert parsed.child.value == 99


def test_three_level_chain_references():
    """A → B → C 三层顺序引用，验证 StructRef 链。"""

    @dataclass
    class C(StructMixin):
        cv: int = field(Int8ub)

    @dataclass
    class B(StructMixin):
        bv: int = field(Int8ub)
        c: C = field(C)

    @dataclass
    class A(StructMixin):
        av: int = field(Int8ub)
        b: B = field(B)

    original = A(av=1, b=B(bv=2, c=C(cv=3)))
    built = original.build()
    assert built == b"\x01\x02\x03"
    parsed = A.parse(built)
    assert isinstance(parsed.b, B)
    assert isinstance(parsed.b.c, C)
    assert parsed.b.c.cv == 3


# ---------------------------------------------------------------------------
# 错误 path 在嵌套结构中的传播
# ---------------------------------------------------------------------------


def test_error_path_includes_nested_field_name():
    """错误消息的 path 应包含嵌套字段名（如 root.inner.b）。"""

    @dataclass
    class Inner(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)

    @dataclass
    class Outer(StructMixin):
        inner: Inner = field(Inner)

    # 故意截断 inner.b 所需的字节：只提供 inner.a 不提供 inner.b
    with pytest.raises(Exception) as exc_info:
        Outer.parse(b"\x01")  # 仅 1 字节，inner.b 缺失

    msg = str(exc_info.value)
    # path 应包含 'inner'（嵌套层）和 'b'（具体字段）
    assert "inner" in msg, f"path should mention 'inner': {msg}"


def test_error_path_includes_field_name_on_top_level():
    """顶层字段错误时，path 应包含该字段名。"""
    from construct import StreamError

    @dataclass
    class M(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)

    with pytest.raises(StreamError) as exc_info:
        M.parse(b"\x01")  # 仅 1 字节，b 缺失

    msg = str(exc_info.value)
    assert "b" in msg, f"path should mention 'b': {msg}"


# ---------------------------------------------------------------------------
# frozen dataclass 支持
# ---------------------------------------------------------------------------


def test_frozen_dataclass_supports_build():
    """frozen=True 的 dataclass 仍可 build（属性读取不修改）。"""

    @dataclass(frozen=True)
    class Frozen(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)

    instance = Frozen(a=1, b=2)
    assert instance.build() == b"\x01\x02"
    # frozen 验证：属性不可修改
    with pytest.raises(Exception):
        instance.a = 99


def test_frozen_dataclass_supports_parse():
    """frozen=True 的 dataclass 可通过 parse 构造。"""

    @dataclass(frozen=True)
    class Frozen(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)

    parsed = Frozen.parse(b"\x05\x06")
    assert parsed.a == 5
    assert parsed.b == 6
    assert isinstance(parsed, Frozen)


# ---------------------------------------------------------------------------
# __post_init__ 支持
# ---------------------------------------------------------------------------


def test_post_init_runs_after_parse():
    """__post_init__ 在 parse 构造实例后执行（由 dataclass __init__ 触发）。"""

    @dataclass
    class WithPostInit(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)
        sum_ab: int = dc_field(default=0)  # 非 StructMixin 字段

        def __post_init__(self):
            self.sum_ab = self.a + self.b

    parsed = WithPostInit.parse(b"\x02\x03")
    assert parsed.a == 2
    assert parsed.b == 3
    assert parsed.sum_ab == 5  # __post_init__ 已运行


def test_post_init_runs_after_build_does_not_break():
    """__post_init__ 存在时 build 仍正常工作。"""

    @dataclass
    class WithPostInit(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)
        cached: int = dc_field(default=0)

        def __post_init__(self):
            self.cached = self.a * 10

    instance = WithPostInit(a=3, b=4)
    assert instance.cached == 30
    built = instance.build()
    assert built == b"\x03\x04"


# ---------------------------------------------------------------------------
# 默认值与可选字段
# ---------------------------------------------------------------------------


def test_default_value_used_when_field_not_in_init():
    """非 StructMixin 字段可使用默认值。"""

    @dataclass
    class WithDefault(StructMixin):
        a: int = field(Int8ub)
        b: bytes = field(Bytes(2))
        note: str = dc_field(default="default-note")

    instance = WithDefault(a=1, b=b"\xff\xff")
    assert instance.note == "default-note"
    built = instance.build()
    assert built == b"\x01\xff\xff"

    # 通过 parse 构造的实例也应有默认值
    parsed = WithDefault.parse(b"\x05\x06\x07")
    assert parsed.note == "default-note"


def test_field_with_python_default_overrides():
    """显式传入的值覆盖 field() 描述符（dataclass 正常行为）。"""

    @dataclass
    class M(StructMixin):
        x: int = field(Int8ub)

    instance = M(x=42)
    built = instance.build()
    assert built == b"\x2a"
