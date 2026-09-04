"""重复与用户回调类异常用户面行为锁定：RepeatError / NamedTupleError / GenericConstructError。

另含保留类可达性说明（ExplicitError / IndexFieldError / StopFieldError /
SizeofError / UnresolvedReferenceError——当前无用户面正触发路径，以层次断言锁定）。

期望值来源标注：[文档]=异常类 docstring 契约；[自然]=语义自然性；
[基线]=实测绿灯行为锚定。
"""

import pytest
from dataclasses import dataclass
from typing import Any

from neoconstruct import (
    Adapter,
    ConstructError,
    Element,
    ExplicitError,
    GenericConstructError,
    IndexFieldError,
    Int8ub,
    NamedTuple,
    NamedTupleError,
    RepeatError,
    RepeatUntil,
    SizeofError,
    StopFieldError,
    StreamError,
    StructMixin,
    UnresolvedReferenceError,
    field,
    rfield,
)


# ---------------------------------------------------------------------------
# RepeatError：RepeatUntil build 无元素满足终止条件
# ---------------------------------------------------------------------------


def test_repeat_error_on_build_without_satisfying_element():
    """RepeatUntil(e > 5) build [1,2,3]（无元素满足）→ RepeatError。[文档]

    双重断言：类型 RepeatError；path 含字段名；消息含 terminator 语义词。
    （本用例与 unit/test_repeat_until_behavior.py 中同名场景互为错误族锚点。）
    """

    @dataclass
    class P(StructMixin):
        e: int = rfield(Element())
        items: list = field(RepeatUntil(e > 5, Int8ub))

    with pytest.raises(RepeatError) as exc_info:
        P(items=[1, 2, 3]).build()

    assert "items" in (exc_info.value.path or "")
    assert "terminator" in str(exc_info.value)


def test_repeat_until_eof_without_sentinel_stream_error_element_path():
    """全流无哨兵至 EOF → StreamError，path 精确到失败元素索引。[基线]"""

    @dataclass
    class P(StructMixin):
        e: int = rfield(Element())
        items: list = field(RepeatUntil(e > 200, Int8ub))

    with pytest.raises(StreamError) as exc_info:
        P.parse(bytes([1, 2, 3]))

    assert exc_info.value.path == "root.items[3]"


# ---------------------------------------------------------------------------
# NamedTupleError：inner 非法或字段提取失败
# ---------------------------------------------------------------------------


def test_named_tuple_error_on_non_sequence_inner_parse():
    """NamedTuple(inner=Int8ub) parse 产出 int 非 list → NamedTupleError。[文档]

    NamedTuple 要求 inner 是 Struct/Sequence/Array/GreedyRange 类容器；
    非容器 inner 在运行时字段提取阶段显式报错。
    双重断言：类型 NamedTupleError；path 含字段名。
    """

    @dataclass
    class P(StructMixin):
        coord: Any = field(NamedTuple("coord", "x y", Int8ub))

    with pytest.raises(NamedTupleError) as exc_info:
        P.parse(b"\x01\x02")

    assert "coord" in (exc_info.value.path or "")


# ---------------------------------------------------------------------------
# GenericConstructError：用户 Adapter 回调异常
# ---------------------------------------------------------------------------


def test_generic_error_from_user_callback_exception():
    """Adapter._decode 内 raise ValueError → GenericConstructError。[文档]

    用户回调异常被包装为 GenericConstructError（消息含回调名与原始异常链）。
    双重断言：类型 GenericConstructError；path 含字段名；消息含原始 ValueError 内容。
    """

    class Weird(Adapter):
        def _decode(self, obj, context, path):
            raise ValueError("user callback failure")

        def _encode(self, obj, context, path):
            return obj

    @dataclass
    class P(StructMixin):
        x: Any = field(Weird(Int8ub))

    with pytest.raises(GenericConstructError) as exc_info:
        P.parse(b"\x01")

    assert "x" in (exc_info.value.path or "")
    assert "user callback failure" in str(exc_info.value)


def test_generic_error_from_wrong_callback_signature():
    """Adapter 回调签名错误（少参数）→ GenericConstructError（含签名提示）。[基线]"""

    class WrongArity(Adapter):
        def _decode(self, obj):  # 缺 context/path 参数
            return obj

        def _encode(self, obj, context, path):
            return obj

    @dataclass
    class P(StructMixin):
        x: Any = field(WrongArity(Int8ub))

    with pytest.raises(GenericConstructError) as exc_info:
        P.parse(b"\x01")

    assert "_decode" in str(exc_info.value)


# ---------------------------------------------------------------------------
# 保留类：当前无用户面正触发路径（层次断言锁定）
# ---------------------------------------------------------------------------
#
# 可达性说明（[文档] _errors.py docstring + 实测探针）：
# - ExplicitError：用户在 Adapter 回调中 raise 的 Python 异常经内核统一包装为
#   Generic（From<PyErr> 仅特判 CancelParsing），Select/Peek 无法识别其原始
#   类型——文档化已知差异，保留类供内核 Explicit 变体映射。
# - IndexFieldError：Index 缺上下文时返回 None（宽松模式），此异常保留供
#   未来严格模式。
# - StopFieldError：内部哨兵（被 Struct/GreedyRange 捕获视为正常终止），
#   实测 Array/Struct 内均不逃逸。
# - SizeofError：用户面暂无 sizeof 调用入口。
# - UnresolvedReferenceError：延迟桩机制保留变体；当前编译期不产生
#   unresolved 错误，用户面无自然触发路径。


def test_reserved_error_classes_hierarchy():
    """五个保留类均为 ConstructError 子类（异常协议完整性）。[文档]"""

    for cls in (
        ExplicitError,
        IndexFieldError,
        StopFieldError,
        SizeofError,
        UnresolvedReferenceError,
    ):
        assert issubclass(cls, ConstructError)


def test_user_explicit_error_wrapped_as_generic_currently():
    """Adapter 回调 raise ExplicitError 经 Select：当前被包装吞掉（文档化已知差异）。[基线]

    Select 后继候选成功接管解析——不向上传播。此为 [文档] 声明的已知差异
    （内核 From<PyErr> 统一转 Generic），锁定现状防回归恶化；内核 Explicit
    变体路径修复后本用例应转为断言传播。
    """

    class Exploder(Adapter):
        def _decode(self, obj, context, path):
            from neoconstruct import ExplicitError as EE

            raise EE("explicit stop")

        def _encode(self, obj, context, path):
            return obj

    from neoconstruct import Int16ub, Select

    @dataclass
    class P(StructMixin):
        x: Any = field(Select(Exploder(Int8ub), Int16ub))

    # 首候选显式失败被吞，次候选 Int16ub 成功
    pkt = P.parse(b"\x01\x02")
    assert pkt.x == 0x0102
