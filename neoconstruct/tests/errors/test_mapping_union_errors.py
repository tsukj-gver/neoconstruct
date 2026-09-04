"""映射与联合类异常用户面行为锁定：MappingError / UnionError。

期望值来源标注：[文档]=异常类/构造器 docstring 契约；[自然]=语义自然性；
[基线]=实测绿灯行为锚定。
"""

import pytest
from dataclasses import dataclass
from typing import Any

from neoconstruct import (
    Bytes,
    Enum,
    Int16ub,
    Int8ub,
    Mapping,
    MappingError,
    StructMixin,
    Union,
    UnionError,
    field,
)


# ---------------------------------------------------------------------------
# MappingError：label ↔ value 查找失败
# ---------------------------------------------------------------------------


def test_enum_build_unknown_label_raises_mapping_error():
    """Enum build 传入未知 label → MappingError。[文档]

    双重断言：类型 MappingError；path 含字段名；消息含未知 label 'DELETE'。
    """

    @dataclass
    class P(StructMixin):
        op: Any = field(Enum(Int8ub, READ=1, WRITE=2))

    with pytest.raises(MappingError) as exc_info:
        P(op="DELETE").build()

    assert "op" in (exc_info.value.path or "")
    assert "DELETE" in str(exc_info.value)


def test_mapping_parse_unknown_key_raises_mapping_error():
    """Mapping parse 到未映射 key → MappingError。[文档]

    双重断言：类型；path 含字段名；消息含实际 key 3。
    """

    @dataclass
    class P(StructMixin):
        op: Any = field(Mapping(Int8ub, {1: "read", 2: "write"}))

    with pytest.raises(MappingError) as exc_info:
        P.parse(b"\x03")

    assert "op" in (exc_info.value.path or "")
    assert "3" in str(exc_info.value)


def test_mapping_build_unknown_value_raises_mapping_error():
    """Mapping build 传入未映射 value → MappingError（parse/build 对称）。[文档]"""

    @dataclass
    class P(StructMixin):
        op: Any = field(Mapping(Int8ub, {1: "read", 2: "write"}))

    with pytest.raises(MappingError) as exc_info:
        P(op="erase").build()

    assert "erase" in str(exc_info.value)


# ---------------------------------------------------------------------------
# Enum 未命中 fallback（与 MappingError 的差异是文档化行为）
# ---------------------------------------------------------------------------


def test_enum_parse_unknown_value_falls_back_to_int():
    """Enum parse 未命中映射 → int fallback（不报错）。[文档]

    Enum 与 Mapping 的差异：未命中时 Enum 回退原始 int，Mapping 报
    MappingError。双重断言：值 int(op)==3；build 回 b"\\x03"。
    """

    @dataclass
    class P(StructMixin):
        op: Any = field(Enum(Int8ub, READ=1, WRITE=2))

    pkt = P.parse(b"\x03")
    assert int(pkt.op) == 3
    assert P(op=3).build() == b"\x03"


def test_enum_parse_known_label_roundtrip():
    """Enum 命中 label：parse → 枚举字符串（int() 可取原值），build 对称。[文档]"""

    @dataclass
    class P(StructMixin):
        op: Any = field(Enum(Int8ub, READ=1, WRITE=2))

    pkt = P.parse(b"\x01")
    assert str(pkt.op) == "READ" or pkt.op == 1
    assert int(pkt.op) == 1
    assert P(op="READ").build() == b"\x01"
    assert P(op=1).build() == b"\x01"


# ---------------------------------------------------------------------------
# UnionError：build 无 subcon 匹配
# ---------------------------------------------------------------------------


def test_union_build_no_matching_branch_raises_union_error():
    """Union build 字典无任何分支键 → UnionError。[文档]

    双重断言：类型 UnionError；path 含字段名；消息含 "cannot build" 语义词。
    """

    @dataclass
    class P(StructMixin):
        u: Any = field(Union(0, raw=Bytes(2), n=Int16ub))

    with pytest.raises(UnionError) as exc_info:
        P(u=dict(zzz="nope")).build()

    assert "u" in (exc_info.value.path or "")
    assert "cannot build" in str(exc_info.value)


def test_union_parsefrom_branch_roundtrip():
    """Union parsefrom 指定分支 parse（错误类旁路 + 对称性）。[自然]

    双重断言：parse 分支值；roundtrip。
    """

    @dataclass
    class P(StructMixin):
        u: Any = field(Union(0, n=Int16ub))

    pkt = P.parse(b"\x00\x05")
    assert pkt.u["n"] == 5
    built = P(u=dict(n=5)).build()
    assert built == b"\x00\x05"
