"""重复与用户回调类异常用户面行为锁定：RepeatError / NamedTupleError / GenericConstructError。

另含用户语义异常在嵌入回调链中的行为（ExplicitError 透传 / CancelParsing
顶层捕获）与保留类可达性说明（StopFieldError / UnresolvedReferenceError /
SizeofError——当前无用户面正触发路径，以层次断言锁定）。

期望值来源标注：[文档]=异常类 docstring 契约；[自然]=语义自然性；
[基线]=实测绿灯行为锚定。
"""

import pytest
from dataclasses import dataclass
from typing import Any

from neoconstruct import (
    Adapter,
    CompilationError,
    ConstructError,
    Element,
    ExplicitError,
    GenericConstructError,
    Index,
    IndexFieldError,
    Int8ub,
    NamedTuple,
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
    """全流无哨兵至 EOF → StreamError，path 精确到失败元素索引。[文档]

    EOF 行为契约：哨兵是组帧契约（必须在流内出现），EOF 仍未出现即报错
    （"读到 EOF 停"是 GreedyRange 语义）；错误路径定位到首个失败元素。
    """

    @dataclass
    class P(StructMixin):
        e: int = rfield(Element())
        items: list = field(RepeatUntil(e > 200, Int8ub))

    with pytest.raises(StreamError) as exc_info:
        P.parse(bytes([1, 2, 3]))

    assert exc_info.value.path == "root.items[3]"


# ---------------------------------------------------------------------------
# NamedTupleError / CompilationError：inner 非容器编译期拒绝；字段提取失败
# ---------------------------------------------------------------------------


def test_named_tuple_non_container_inner_rejected_at_compile_time():
    """NamedTuple(inner=Int8ub) 非容器 inner → 编译期 CompilationError。[文档]

    inner 必须是 Struct/Sequence/Array/GreedyRange 类容器——描述符编译时
    即可静态判定，按项目 fail-fast 一贯策略在类创建（__init_subclass__）
    阶段拒绝，而非延迟到 parse。双重断言：类型 CompilationError；
    消息含容器指引语义词。
    """

    with pytest.raises(CompilationError) as exc_info:

        @dataclass
        class P(StructMixin):
            coord: Any = field(NamedTuple("coord", "x y", Int8ub))

    msg = str(exc_info.value)
    assert "NamedTuple" in msg
    assert "Struct" in msg or "container" in msg


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
# 用户语义异常：ExplicitError 透传 / CancelParsing 顶层捕获
# ---------------------------------------------------------------------------
#
# 契约（_errors.py docstring）：
# - ExplicitError：用户在 Adapter 回调中 raise 的 ExplicitError 是"显式终止"
#   信号，Select/Peek 等容错链**透传不吞**（不像普通回调异常那样包装降级）。
# - CancelParsing：用户主动取消，parse 入口捕获后返回 None（对齐 pass 语义）。


def test_user_explicit_error_propagates_through_select():
    """Adapter 回调 raise ExplicitError 经 Select → 向上传播，不被吞掉。[文档]

    Select 首候选的 ExplicitError 是用户显式信号：不 seek 回退、不尝试
    次候选、直接传播给调用者。双重断言：类型 ExplicitError；消息含用户
    原文；Select 未静默换用次候选。
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

    with pytest.raises(ExplicitError) as exc_info:
        P.parse(b"\x01\x02")

    assert "explicit stop" in str(exc_info.value)


def test_cancel_parsing_in_embedded_adapter_returns_none():
    """嵌入 Struct 的 Adapter 回调 raise CancelParsing → parse 返回 None。[文档]

    CancelParsing 是用户主动取消信号：跨内核边界（嵌入回调）后仍被顶层
    捕获（对齐 ``except CancelParsing: pass``），不被包装成
    GenericConstructError。
    """

    class Canceller(Adapter):
        def _decode(self, obj, context, path):
            from neoconstruct import CancelParsing as CP

            raise CP()

        def _encode(self, obj, context, path):
            return obj

    @dataclass
    class P(StructMixin):
        x: Any = field(Canceller(Int8ub))
        y: int = field(Int8ub)

    assert P.parse(b"\x01\x02") is None


# ---------------------------------------------------------------------------
# IndexFieldError：Index 字段不在数组迭代上下文中
# ---------------------------------------------------------------------------


def test_index_outside_array_raises_index_field_error():
    """Index() 用在非数组上下文 → IndexFieldError（显式报错）。[文档]

    Index 字段的语义是"当前数组迭代下标"；脱离数组迭代时无值可言，
    静默 None 会让哑值流入下游远处炸成难归因错误——显式报错更早暴露
    误用。双重断言：类型 IndexFieldError；path 含字段名。
    """

    @dataclass
    class P(StructMixin):
        i: int = rfield(Index())
        x: int = field(Int8ub)

    with pytest.raises(IndexFieldError) as exc_info:
        P.parse(b"\x01")

    assert "i" in (exc_info.value.path or "")
    assert "index" in str(exc_info.value).lower()


# ---------------------------------------------------------------------------
# 保留类：当前无用户面正触发路径（层次断言锁定可达性占位）
# ---------------------------------------------------------------------------
#
# 可达性说明（[文档] _errors.py docstring + 实测探针）：
# - StopFieldError：内部哨兵（被 Struct/GreedyRange 捕获视为正常终止），
#   实测 Array/Struct 内均不逃逸——真特性（有意暴露的内部信号）。
# - SizeofError：错误分类学已接线（内核 sizeof 失败变体）；用户面 sizeof
#   API 尚未提供，暂无正触发路径。
# - UnresolvedReferenceError：延迟桩机制保留变体；当前编译期不产生
#   unresolved 错误，用户面无自然触发路径。
# （ExplicitError / IndexFieldError / CancelParsing 已有正触发 sibling 用例。）


def test_reserved_error_classes_hierarchy():
    """三个保留类均为 ConstructError 子类（异常协议完整性；可达性占位）。[文档]

    层次断言仅锁定异常协议；各类的正触发路径见 sibling 用例或保留原因
    见上方可达性说明。
    """

    for cls in (
        StopFieldError,
        SizeofError,
        UnresolvedReferenceError,
    ):
        assert issubclass(cls, ConstructError)
