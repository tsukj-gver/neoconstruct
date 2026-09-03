# C1/C3 补充：嵌套 Struct 跨层引用 + wo/Rebuild 语义 + RepeatUntil/Checksum 包装
import traceback
from dataclasses import dataclass
from typing import Any
from construct import (StructMixin, field, rfield, wfield, Int8ub, Const,
                       Default, Rebuild, Computed, Bytes, Prefixed, Switch,
                       Element, RepeatUntil, Checksum, Padding, Tell)

def show(name, fn):
    print(f"=== {name} ===")
    try:
        r = fn()
        print(f"[OK] {r}")
    except Exception as e:
        print(f"[FAIL] {type(e).__name__}: {str(e).splitlines()[0][:150]}")
    print()

# ---- 嵌套 StructMixin 子类 x Switch(内层自身字段) ----
def t_nested_switch_inner():
    @dataclass
    class Inner(StructMixin):
        t: int = field(Int8ub)
        v: Any = field(Switch(t, {1: Int8ub, 2: Bytes(2)}))
    @dataclass
    class Outer(StructMixin):
        typ: int = field(Int8ub)
        inner: Inner = field(Inner)
    return Outer.parse(b"\x01\x01\x7f")
show("nested Inner{Switch(inner t)} parse", t_nested_switch_inner)

# ---- 嵌套 Struct x Switch(外层字段)：用户期望透传 ----
def t_nested_switch_outer():
    @dataclass
    class Inner(StructMixin):
        v: Any = field(Switch(typ, {1: Int8ub, 2: Bytes(2)}))   # typ 是外层字段
    @dataclass
    class Outer(StructMixin):
        typ: int = field(Int8ub)
        inner: Inner = field(Inner)
    return Outer
show("nested Inner{Switch(outer typ)} define", t_nested_switch_outer)

# ---- wo 字段 build 传值语义 ----
def t_wo_build():
    @dataclass
    class PW(StructMixin):
        n: int = field(Int8ub)
        pad: Any = wfield(Padding(2))
    return PW(n=1, pad=None).build()
show("wfield(Padding(2)) PW(n=1, pad=None).build()", t_wo_build)

def t_wo_build2():
    @dataclass
    class PW(StructMixin):
        n: int = field(Int8ub)
        pad: Any = wfield(Padding(2))
    return PW(n=1, pad='whatever').build()
show("wfield(Padding(2)) PW(n=1, pad='whatever').build()", t_wo_build2)

# ---- Rebuild 传值：v 显式传 0，build 用 n 还是 0？ ----
def t_rebuild_override():
    @dataclass
    class PR(StructMixin):
        n: int = field(Int8ub)
        v: int = field(Rebuild(Int8ub, n))
    return PR(n=3, v=0).build()
show("Rebuild PR(n=3, v=0).build()", t_rebuild_override)

# ---- RepeatUntil 被包装 ----
def t_repeat_until_wrapped():
    @dataclass
    class P(StructMixin):
        e: Any = rfield(Element())
        items: Any = field(Prefixed(Int8ub, RepeatUntil(lambda e, ctx: False, Bytes(1))))
    return P
show("Prefixed x RepeatUntil(lambda) define", t_repeat_until_wrapped)

def t_repeat_until_wrapped2():
    @dataclass
    class P(StructMixin):
        e: Any = rfield(Element())
        items: Any = field(Prefixed(Int8ub, RepeatUntil(e == b'\x00', Bytes(1))))
    return P
show("Prefixed x RepeatUntil(e == b'\\x00') define", t_repeat_until_wrapped2)

# ---- Checksum 被包装（bytesfunc 引用外层） ----
def t_checksum_wrapped():
    @dataclass
    class P(StructMixin):
        data: bytes = field(Bytes(4))
        crc: int = field(Prefixed(Int8ub, Checksum(Bytes(2))))
    return P
show("Prefixed x Checksum(Bytes(2)) define", t_checksum_wrapped)

# ---- Switch key 为常量 + Prefixed（边界：无需表达式）----
def t_pref_const_switch():
    @dataclass
    class P(StructMixin):
        typ: int = field(Int8ub)
        br: Any = field(Prefixed(Int8ub, Switch(1, {1: Int8ub, 2: Bytes(2)})))
    return P.parse(b"\x01\x01\x7f")
show("Prefixed x Switch(1 const) parse [boundary]", t_pref_const_switch)

# ---- Switch 在顶层但 keyfunc 引用表达式 x+1（设计内拒绝，对照组）----
def t_switch_complex():
    @dataclass
    class P(StructMixin):
        typ: int = field(Int8ub)
        br: Any = field(Switch(typ + 1, {2: Int8ub}))
    return P
show("bare Switch(typ+1) define [designed rejection]", t_switch_complex)

# ---- 顶层 Switch key = Computed 预计算字段（官方建议 workaround）----
def t_switch_workaround():
    @dataclass
    class P(StructMixin):
        typ: int = field(Int8ub)
        br: Any = field(Switch(typ, {1: Int8ub, 2: Bytes(2)}))
        wrapped: Any = field(Prefixed(Int8ub, Bytes(2)))
    return P.parse(b"\x01\x7f\x02AB")
show("no-wrapper struct with multiple fields [sanity]", t_switch_workaround)
