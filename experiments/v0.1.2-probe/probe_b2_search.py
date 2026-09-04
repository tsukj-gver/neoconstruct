"""B2 触发条件搜索：x 为 Const / 两层包装 / 组合场景。"""
from dataclasses import dataclass
from typing import Any

from neoconstruct import (
    Byte, Bytes, Const, Int8ub, Prefixed, StructMixin, Switch, field,
)

print("=== T1: x=Const + Prefixed(Bytes(x+1)) ===")


@dataclass
class T1(StructMixin):
    x: int = field(Const(1, Byte))
    p: bytes = field(Prefixed(Int8ub, Bytes(x + 1)))


try:
    b = T1(p=b"ab").build()
    print("T1 build:", b)
    print("T1 parse:", T1.parse(b"\x01\x02ab"))
except Exception as e:
    print("T1 FAIL:", type(e).__name__, str(e)[:200])

print("=== T2: x=Const(97) + 后续 Const + Prefixed(Bytes(x+1)) ===")


@dataclass
class T2(StructMixin):
    x: int = field(Byte)
    c: int = field(Const(97, Byte))  # 'a'
    p: bytes = field(Prefixed(Int8ub, Bytes(x + 1)))


try:
    b = T2(x=1, c=None, p=b"ab").build()
    print("T2 build:", b)
    print("T2 parse:", T2.parse(b"\x01" + b"a" + b"\x02ab"))
except Exception as e:
    print("T2 FAIL:", type(e).__name__, str(e)[:200])

print("=== T3: 两层 Prefixed ===")


@dataclass
class T3(StructMixin):
    x: int = field(Int8ub)
    p: bytes = field(Prefixed(Int8ub, Prefixed(Int8ub, Bytes(x + 1))))


try:
    b = T3(x=1, p=b"ab").build()
    print("T3 build:", b)
    print("T3 parse:", T3.parse(b"\x01\x03\x02ab"))
except Exception as e:
    print("T3 FAIL:", type(e).__name__, str(e)[:200])

print("=== T4: x=Const + 顶层 Bytes(x+1)（对照，正常） ===")


@dataclass
class T4(StructMixin):
    x: int = field(Const(1, Byte))
    p: bytes = field(Bytes(x + 1))


try:
    print("T4 parse:", T4.parse(b"\x01ab"))
    print("T4 build:", T4(p=b"ab").build())  # B1 场景：预期 build 失败（x=None）
except Exception as e:
    print("T4 FAIL:", type(e).__name__, str(e)[:200])

print("=== T5: Switch(typ+1) 嵌套 Prefixed（总纲 P10 说的正常形态对照） ===")


@dataclass
class T5(StructMixin):
    typ: int = field(Int8ub)
    br: Any = field(Prefixed(Int8ub, Switch(typ + 1, {2: Bytes(2), 3: Byte})))


try:
    print("T5 parse:", T5.parse(b"\x01\x02\xab\xcd"))
    print("T5 build:", T5(typ=1, br=b"\xab\xcd").build())
except Exception as e:
    print("T5 FAIL:", type(e).__name__, str(e)[:200])
