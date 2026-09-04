"""探针 A：G3 异常类触发验证——校验/映射/选择/联合族。

覆盖：CheckError / TerminatedError / ValidationError(OneOf/NoneOf) /
SelectError / MappingError(Enum build / Mapping parse+build) / UnionError。
每项打印实际异常类型 + message + path。
"""

import sys
import traceback

sys.path.insert(0, r"D:\Project\Github\neoconstruct\neoconstruct\python")

from dataclasses import dataclass  # noqa: E402
from typing import Any  # noqa: E402

import neoconstruct as nc  # noqa: E402
from neoconstruct import (  # noqa: E402
    Enum,
    GreedyString,
    Int16ub,
    Int32ub,
    Int8ub,
    Mapping,
    NoneOf,
    OneOf,
    Select,
    StructMixin,
    Terminated,
    Union,
    field,
    rfield,
)


def run(label, fn):
    print(f"===== {label} =====")
    try:
        fn()
    except Exception as e:
        print(f"  type    = {type(e).__name__}")
        print(f"  is CE   = {isinstance(e, nc.ConstructError)}")
        msg = str(e)
        print(f"  message = {msg[:160]!r}")
        path = getattr(e, "path", "<<no attr>>")
        print(f"  path    = {path!r}")
    print()


# ---- CheckError ----

def check_error_parse():
    @dataclass
    class P(StructMixin):
        x: int = field(Int8ub)
        _check: Any = rfield(nc.Check(x == 5))

    P.parse(b"\x03")


def check_error_build():
    @dataclass
    class P(StructMixin):
        x: int = field(Int8ub)
        _check: Any = rfield(nc.Check(x == 5))

    P(x=3).build()


# ---- TerminatedError ----

def terminated_error_parse():
    @dataclass
    class P(StructMixin):
        x: int = field(Int8ub)
        _term: Any = rfield(Terminated)

    P.parse(b"\x01\x02")


# ---- ValidationError：OneOf / NoneOf ----

def oneof_error_parse():
    @dataclass
    class P(StructMixin):
        x: int = field(OneOf(Int8ub, [1, 2, 3]))

    P.parse(b"\x05")


def oneof_error_build():
    @dataclass
    class P(StructMixin):
        x: int = field(OneOf(Int8ub, [1, 2, 3]))

    P(x=9).build()


def noneof_error_parse():
    @dataclass
    class P(StructMixin):
        x: int = field(NoneOf(Int8ub, [7, 8]))

    P.parse(b"\x07")


# ---- SelectError ----

def select_error_all_fail():
    @dataclass
    class P(StructMixin):
        x: Any = field(Select(Int16ub, Int32ub))

    P.parse(b"\x01")


# ---- MappingError ----

def enum_build_unknown_label():
    @dataclass
    class P(StructMixin):
        op: Any = field(Enum(Int8ub, READ=1, WRITE=2))

    P(op="DELETE").build()


def mapping_parse_unknown_key():
    @dataclass
    class P(StructMixin):
        op: Any = field(Mapping(Int8ub, {1: "read", 2: "write"}))

    P.parse(b"\x03")


def mapping_build_unknown_value():
    @dataclass
    class P(StructMixin):
        op: Any = field(Mapping(Int8ub, {1: "read", 2: "write"}))

    P(op="erase").build()


# ---- UnionError ----

def union_build_no_match():
    @dataclass
    class P(StructMixin):
        u: Any = field(Union(0, raw=nc.Bytes(2), n=Int16ub))

    P(u=dict(zzz="nope")).build()


def union_parsefrom_failure():
    @dataclass
    class P(StructMixin):
        u: Any = field(Union(0, raw=nc.Bytes(2)))

    P.parse(b"\x01")


# ---- StringError（顺带）----

def string_error_bad_utf8():
    @dataclass
    class P(StructMixin):
        s: str = field(GreedyString("utf8"))

    P.parse(b"\xff\xfe\xfd")


if __name__ == "__main__":
    run("CheckError parse", check_error_parse)
    run("CheckError build", check_error_build)
    run("TerminatedError parse", terminated_error_parse)
    run("ValidationError OneOf parse", oneof_error_parse)
    run("ValidationError OneOf build", oneof_error_build)
    run("ValidationError NoneOf parse", noneof_error_parse)
    run("SelectError all fail", select_error_all_fail)
    run("MappingError Enum build unknown label", enum_build_unknown_label)
    run("MappingError Mapping parse unknown key", mapping_parse_unknown_key)
    run("MappingError Mapping build unknown value", mapping_build_unknown_value)
    run("UnionError build no match", union_build_no_match)
    run("UnionError parsefrom failure", union_parsefrom_failure)
    run("StringError bad utf8", string_error_bad_utf8)
