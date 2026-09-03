# BUG-1 对照实验 v2：表达式消费者 x 包装器 组合矩阵
import traceback
from dataclasses import dataclass
from typing import Any
from construct import (StructMixin, field, Int8ub, Prefixed, Switch, Bytes,
                       PrefixedArray, Computed, If, GreedyRange, Bitwise,
                       Hex, Union, Select, IfThenElse, Pass, Checksum)

results = []

def try_case(name, fn, expect):
    print(f"=== {name} ===")
    try:
        r = fn()
        print(f"[OK] {r}")
        results.append((name, "OK", "OK"))
    except Exception as e:
        msg = str(e).splitlines()[0][:140] if str(e) else type(e).__name__
        print(f"[FAIL] {type(e).__name__}: {msg}")
        results.append((name, "FAIL: " + msg, expect))
    print()

# ---- 消费者：Switch ----
def c_bare_switch():
    @dataclass
    class P(StructMixin):
        typ: int = field(Int8ub)
        br: Any = field(Switch(typ, {1: Int8ub, 2: Bytes(2)}))
    return P.parse(b"\x01\x7f")
try_case("bare Switch(typ)", c_bare_switch, "OK")

def c_pref_switch():
    @dataclass
    class P(StructMixin):
        typ: int = field(Int8ub)
        br: Any = field(Prefixed(Int8ub, Switch(typ, {1: Int8ub, 2: Bytes(2)})))
    return P.parse(b"\x01\x01\x7f")
try_case("Prefixed x Switch  [BUG-1]", c_pref_switch, "OK")

def c_prefarr_switch():
    @dataclass
    class P(StructMixin):
        typ: int = field(Int8ub)
        br: Any = field(PrefixedArray(Int8ub, Switch(typ, {1: Int8ub, 2: Bytes(2)})))
    return P.parse(b"\x01\x02\x01\x02")
try_case("PrefixedArray x Switch", c_prefarr_switch, "OK")

def c_bitwise_switch():
    @dataclass
    class P(StructMixin):
        typ: int = field(Int8ub)
        br: Any = field(Bitwise(Switch(typ, {1: Int8ub, 2: Bytes(2)})))
    return P
try_case("Bitwise x Switch", c_bitwise_switch, "OK")

def c_hex_switch():
    @dataclass
    class P(StructMixin):
        typ: int = field(Int8ub)
        br: Any = field(Hex(Switch(typ, {1: Int8ub, 2: Bytes(2)})))
    return P.parse(b"\x01\x7f")
try_case("Hex x Switch", c_hex_switch, "OK")

def c_union_switch():
    @dataclass
    class P(StructMixin):
        typ: int = field(Int8ub)
        br: Any = field(Union(None, Switch(typ, {1: Int8ub, 2: Bytes(2)}), Bytes(2)))
    return P.parse(b"\x01\x7f\x00")
try_case("Union x Switch", c_union_switch, "OK")

def c_select_switch():
    @dataclass
    class P(StructMixin):
        typ: int = field(Int8ub)
        br: Any = field(Select(Switch(typ, {1: Int8ub, 2: Bytes(2)}), Bytes(2)))
    return P.parse(b"\x01\x7f")
try_case("Select x Switch", c_select_switch, "OK")

# ---- 消费者：If ----
def c_bare_if():
    @dataclass
    class P(StructMixin):
        typ: int = field(Int8ub)
        br: Any = field(If(typ == 1, Int8ub))
    return P.parse(b"\x01\x7f")
try_case("bare If(typ==1)", c_bare_if, "OK")

def c_pref_if():
    @dataclass
    class P(StructMixin):
        typ: int = field(Int8ub)
        br: Any = field(Prefixed(Int8ub, If(typ == 1, Int8ub)))
    return P.parse(b"\x01\x01\x7f")
try_case("Prefixed x If", c_pref_if, "OK")

def c_prefarr_if():
    @dataclass
    class P(StructMixin):
        typ: int = field(Int8ub)
        br: Any = field(PrefixedArray(Int8ub, If(typ == 1, Int8ub)))
    return P.parse(b"\x01\x02\x7f\xff")
try_case("PrefixedArray x If", c_prefarr_if, "OK")

# ---- 消费者：Computed ----
def c_bare_computed():
    @dataclass
    class P(StructMixin):
        typ: int = field(Int8ub)
        br: Any = field(Computed(typ))
    return P.parse(b"\x01")
try_case("bare Computed(typ)", c_bare_computed, "OK")

def c_pref_computed():
    @dataclass
    class P(StructMixin):
        typ: int = field(Int8ub)
        br: Any = field(Prefixed(Int8ub, Computed(typ)))
    return P.parse(b"\x01\x01")
try_case("Prefixed x Computed", c_pref_computed, "OK")

# ---- 消费者：IfThenElse ----
def c_pref_ite():
    @dataclass
    class P(StructMixin):
        typ: int = field(Int8ub)
        br: Any = field(Prefixed(Int8ub, IfThenElse(typ == 1, Int8ub, Bytes(2))))
    return P.parse(b"\x01\x01\x7f")
try_case("Prefixed x IfThenElse", c_pref_ite, "OK")

# ---- 控制组：Prefixed 包非表达式子构造器 ----
def c_pref_plain():
    @dataclass
    class P(StructMixin):
        typ: int = field(Int8ub)
        br: Any = field(Prefixed(Int8ub, Int8ub))
    return P.parse(b"\x01\x01\x7f")
try_case("Prefixed x Int8ub (control)", c_pref_plain, "OK")

def c_pref_nested_bytes_len():
    # Bytes 的 length 表达式在包装内部（非顶层）
    @dataclass
    class P(StructMixin):
        typ: int = field(Int8ub)
        br: Any = field(Prefixed(Int8ub, Bytes(typ)))
    return P.parse(b"\x02\x02\x41\x42")
try_case("Prefixed x Bytes(typ)  length-expr nested", c_pref_nested_bytes_len, "OK")

def c_prefarr_nested_bytes_len():
    @dataclass
    class P(StructMixin):
        typ: int = field(Int8ub)
        br: Any = field(PrefixedArray(Int8ub, Bytes(typ)))
    return P.parse(b"\x02\x02\x41\x42")
try_case("PrefixedArray x Bytes(typ)", c_prefarr_nested_bytes_len, "OK")

print("\n===== SUMMARY =====")
for name, got, expect in results:
    status = "PASS" if got == "OK" else "FAIL"
    print(f"{status:4} | {name} | got={got[:100]}")
