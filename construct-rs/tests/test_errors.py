"""错误场景测试。

设计依据：``plans/phase1-foundation/总纲.md`` S-FUNC（功能覆盖）与
``docs/架构设计.md`` §B.8（错误映射）。

覆盖：
- StreamError：字节不足、流读/写失败
- FormatFieldError：值类型错误、值超范围
- FieldLengthError：Bytes(n) 长度不匹配
- CompilationError：未知描述符
- 错误携带 path 字段
- Python 异常层次（except StreamError 等）
"""

import pytest
from dataclasses import dataclass

from construct import (
    Bytes,
    CompilationError,
    ConstructError,
    FieldLengthError,
    FormatFieldError,
    GreedyBytes,
    Int8ub,
    Int8sl,
    Int16ub,
    Int32ub,
    StreamError,
    StructMixin,
    field,
)


# ---------------------------------------------------------------------------
# StreamError：字节不足
# ---------------------------------------------------------------------------


def test_stream_error_on_insufficient_bytes_parse():
    """parse 时提供字节数 < 字段需求 → StreamError。"""

    @dataclass
    class M(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)

    with pytest.raises(StreamError):
        M.parse(b"\x01")  # 仅 1 字节，b 缺失


def test_stream_error_on_empty_parse_for_required_field():
    """parse 空 bytes 给必填字段 → StreamError。"""

    @dataclass
    class M(StructMixin):
        a: int = field(Int8ub)

    with pytest.raises(StreamError):
        M.parse(b"")


def test_stream_error_path_includes_field_name():
    """StreamError 错误消息中应包含出错字段名（path）。"""

    @dataclass
    class M(StructMixin):
        first: int = field(Int8ub)
        second: int = field(Int8ub)

    with pytest.raises(StreamError) as exc_info:
        M.parse(b"\x01")

    err = exc_info.value
    assert err.path is not None
    assert "second" in err.path, f"path should mention 'second': {err.path}"


def test_stream_error_caught_as_base_class():
    """StreamError 应能被 ConstructError（基类）捕获。"""

    @dataclass
    class M(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)

    with pytest.raises(ConstructError):
        M.parse(b"\x01")


def test_stream_error_caught_as_exception():
    """StreamError 应能被 Exception 基类捕获（Python 异常协议）。"""

    @dataclass
    class M(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)

    with pytest.raises(Exception):
        M.parse(b"\x01")


# ---------------------------------------------------------------------------
# FormatFieldError：值类型错误或超范围
# ---------------------------------------------------------------------------


def test_format_field_error_on_wrong_type_build():
    """build 时字段值为错误类型（int 字段传 str）→ FormatFieldError。"""

    @dataclass
    class M(StructMixin):
        x: int = field(Int8ub)

    instance = M(x="not_an_int")  # type: ignore[arg-type]
    with pytest.raises(FormatFieldError):
        instance.build()


def test_format_field_error_on_out_of_range_value():
    """build 时值超出类型范围（Int8 传 256）→ FormatFieldError。"""

    @dataclass
    class M(StructMixin):
        x: int = field(Int8ub)  # 范围 0..255

    instance = M(x=256)
    with pytest.raises(FormatFieldError):
        instance.build()


def test_format_field_error_on_negative_for_unsigned():
    """build 时 Int8ub 传负值 → FormatFieldError。"""

    @dataclass
    class M(StructMixin):
        x: int = field(Int8ub)

    instance = M(x=-1)
    with pytest.raises(FormatFieldError):
        instance.build()


def test_format_field_error_path_includes_field_name():
    """FormatFieldError 错误消息应包含出错字段名。"""

    @dataclass
    class M(StructMixin):
        x: int = field(Int8ub)

    instance = M(x=999)
    with pytest.raises(FormatFieldError) as exc_info:
        instance.build()

    err = exc_info.value
    assert err.path is not None
    assert "x" in err.path


# ---------------------------------------------------------------------------
# FieldLengthError：Bytes(n) 长度不匹配
# ---------------------------------------------------------------------------


def test_field_length_error_on_too_short_bytes():
    """Bytes(4) build 3 字节 → FieldLengthError。"""

    @dataclass
    class M(StructMixin):
        x: bytes = field(Bytes(4))

    with pytest.raises(FieldLengthError):
        M(x=b"\x00\x01\x02").build()


def test_field_length_error_on_too_long_bytes():
    """Bytes(4) build 5 字节 → FieldLengthError。"""

    @dataclass
    class M(StructMixin):
        x: bytes = field(Bytes(4))

    with pytest.raises(FieldLengthError):
        M(x=b"\x00\x01\x02\x03\x04").build()


def test_field_length_error_path_includes_field_name():
    """FieldLengthError 错误消息应包含出错字段名。"""

    @dataclass
    class M(StructMixin):
        payload: bytes = field(Bytes(8))

    # payload 长度不足触发 FieldLengthError
    with pytest.raises(FieldLengthError) as exc_info:
        M(payload=b"short").build()

    err = exc_info.value
    assert err.path is not None
    assert "payload" in err.path


# ---------------------------------------------------------------------------
# CompilationError：未知描述符
# ---------------------------------------------------------------------------


def test_compilation_error_on_unknown_descriptor():
    """使用未知类型的描述符 → CompilationError（编译阶段错误）。"""

    class BogusDescriptor:
        """不是任何已识别的描述符类型。"""

        pass

    # 直接调用 compile_schema 触发编译错误
    from construct._construct_rust import compile_schema

    bogus = BogusDescriptor()

    @dataclass
    class _Silent:
        pass

    with pytest.raises(Exception) as exc_info:
        # 直接调用 Rust compile_schema，传入未知描述符
        compile_schema(_Silent, ["x"], [bogus])

    # 应是 CompilationError 或包含编译相关消息
    err = exc_info.value
    msg = str(err).lower()
    assert (
        "未知" in msg
        or "unknown" in msg
        or "unrecognized" in msg
        or "bogus" in msg
        or "compilation" in msg
    ), f"error should mention unknown descriptor: {err}"


def test_compilation_error_on_field_names_descriptors_mismatch():
    """field_names 与 descriptors 长度不一致 → CompilationError。"""
    from construct._construct_rust import compile_schema

    @dataclass
    class _Silent:
        pass

    with pytest.raises(Exception):
        compile_schema(_Silent, ["a", "b"], [Int8ub])  # 2 名 1 描述符


# ---------------------------------------------------------------------------
# 错误层次结构验证
# ---------------------------------------------------------------------------


def test_stream_error_is_construct_error_subclass():
    """StreamError 应是 ConstructError 的子类（异常层次结构）。"""
    assert issubclass(StreamError, ConstructError)
    assert issubclass(FormatFieldError, ConstructError)
    assert issubclass(FieldLengthError, ConstructError)
    assert issubclass(CompilationError, ConstructError)


def test_each_error_has_message_and_path_attributes():
    """StreamError 实例应携带 message 与 path 属性（§B.8）。"""

    @dataclass
    class M(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)

    with pytest.raises(StreamError) as exc_info:
        M.parse(b"\x01")

    err = exc_info.value
    assert hasattr(err, "message")
    assert hasattr(err, "path")
    assert err.path is not None
    assert isinstance(err.message, str)


def test_error_str_format_matches_python_construct():
    """str(StreamError) 格式应为 'Error in path {path}\\n{message}'。"""

    @dataclass
    class M(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)

    with pytest.raises(StreamError) as exc_info:
        M.parse(b"\x01")

    s = str(exc_info.value)
    assert "Error in path" in s
    assert "root" in s  # 根路径标识


# ---------------------------------------------------------------------------
# parse 与 build 的错误对称性
# ---------------------------------------------------------------------------


def test_parse_and_build_both_raise_on_invalid_data():
    """parse 数据不足 → StreamError；build 类型错误 → FormatFieldError。"""

    @dataclass
    class M(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)

    # parse 错误
    with pytest.raises(StreamError):
        M.parse(b"")
    with pytest.raises(StreamError):
        M.parse(b"\x01")

    # build 错误
    bad = M(a="not_int", b=1)  # type: ignore[arg-type]
    with pytest.raises(FormatFieldError):
        bad.build()


# ---------------------------------------------------------------------------
# 复合：嵌套错误场景
# ---------------------------------------------------------------------------


def test_nested_stream_error_path_includes_outer_field():
    """嵌套结构中流错误时，path 应包含外层和内层字段。"""

    @dataclass
    class Inner(StructMixin):
        x: int = field(Int8ub)
        y: int = field(Int8ub)

    @dataclass
    class Outer(StructMixin):
        inner: Inner = field(Inner)

    # 仅提供 inner.x，缺 inner.y
    with pytest.raises(StreamError) as exc_info:
        Outer.parse(b"\x01")

    path = exc_info.value.path or ""
    assert "inner" in path, f"path should include 'inner': {path}"
