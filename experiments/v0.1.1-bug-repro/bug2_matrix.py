# BUG-2 v2 + C2/C3 矩阵：值提供型构造器的 dataclass default 行为
import traceback
from dataclasses import dataclass
from typing import Any
from construct import (StructMixin, field, rfield, wfield, Int8ub, Const,
                       Default, Rebuild, Computed, Bytes)

def show(name, fn):
    print(f"=== {name} ===")
    try:
        r = fn()
        print(f"[OK] {r}")
    except Exception as e:
        print(f"[FAIL] {type(e).__name__}: {str(e).splitlines()[0][:150]}")
    print()

# ---- Const(5, Int8ub)：带 subcon ----
def t_const_def():
    @dataclass
    class PC(StructMixin):
        v: int = field(Const(5, Int8ub))
    return PC()          # 期望：不报 missing argument
show("field(Const(5, Int8ub)) -> PC()", t_const_def)

def t_const_build():
    @dataclass
    class PC(StructMixin):
        v: int = field(Const(5, Int8ub))
    return PC(v=5).build()   # 期望 b'\x05'
show("field(Const(5, Int8ub)) -> PC(v=5).build()", t_const_build)

def t_const_parse():
    @dataclass
    class PC(StructMixin):
        v: int = field(Const(5, Int8ub))
    return PC.parse(b"\x05")
show("field(Const(5, Int8ub)) -> PC.parse(b'\\x05')", t_const_parse)

# ---- Default(Int8ub, 9) ----
def t_default_noarg():
    @dataclass
    class PD(StructMixin):
        v: int = field(Default(Int8ub, 9))
    return PD()          # 期望：不报 missing（build 用 9）
show("field(Default(Int8ub, 9)) -> PD()", t_default_noarg)

def t_default_build():
    @dataclass
    class PD(StructMixin):
        v: int = field(Default(Int8ub, 9))
    return PD(v=None).build()   # 期望 b'\x09'
show("field(Default(Int8ub, 9)) -> PD(v=None).build()", t_default_build)

# ---- Rebuild ----
def t_rebuild():
    @dataclass
    class PR(StructMixin):
        n: int = field(Int8ub)
        v: int = field(Rebuild(Int8ub, n))
    return PR(n=3).build()      # 期望 b'\x03\x03'
show("field(Rebuild(Int8ub, n)) -> PR(n=3).build()", t_rebuild)

def t_rebuild_noarg():
    @dataclass
    class PR(StructMixin):
        n: int = field(Int8ub)
        v: int = field(Rebuild(Int8ub, n))
    return PR(n=3)              # 期望：v 可缺省
show("field(Rebuild(Int8ub, n)) -> PR(n=3) [v omitted]", t_rebuild_noarg)

# ---- Computed 顶层字段（field 而非 rfield）----
def t_computed_field():
    @dataclass
    class PC(StructMixin):
        x: int = field(Int8ub)
        c: int = field(Computed(x + 1))
    return PC(x=1)              # c 是计算值，应可缺省？
show("field(Computed(x + 1)) -> PC(x=1) [c omitted]", t_computed_field)

# ---- rfield(Computed)（README 推荐 RO 模式）----
def t_rfield_computed():
    @dataclass
    class PC(StructMixin):
        x: int = field(Int8ub)
        c: int = rfield(Computed(x + 1))
    return PC(x=1), PC(x=1).c
show("rfield(Computed(x + 1)) -> PC(x=1)", t_rfield_computed)

# ---- wfield(Const)：只写 padding/reserved ----
def t_wfield_const():
    @dataclass
    class PW(StructMixin):
        v: int = wfield(Const(5, Int8ub))
    return PW(), PW().build()
show("wfield(Const(5, Int8ub)) -> PW()", t_wfield_const)

def t_wfield_padding():
    from construct import Padding
    @dataclass
    class PW(StructMixin):
        n: int = field(Int8ub)
        pad: Any = wfield(Padding(2))
    return PW(n=1), PW(n=1).build()
show("wfield(Padding(2)) -> PW(n=1)", t_wfield_padding)

# ---- field(default=...) 用户显式 default（对照，应 OK）----
def t_explicit_default():
    @dataclass
    class PE(StructMixin):
        v: int = field(Int8ub, default=7)
    return PE(), PE().build()
show("field(Int8ub, default=7) -> PE() [control]", t_explicit_default)

# ---- Const + 普通字段混合：P(n=1) 是否可行 ----
def t_mixed():
    @dataclass
    class PM(StructMixin):
        v: int = field(Const(5, Int8ub))
        n: int = field(Int8ub)
    return PM(n=1).build()      # 期望 b'\x05\x01'
show("Const+normal: PM(n=1).build()", t_mixed)
