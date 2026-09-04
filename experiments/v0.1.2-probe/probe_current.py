"""v0.1.2-2 止血修复前的现状探针：B2（BinExpr 嵌套包装器）/ B3（wo 表达式长度）。"""
from dataclasses import dataclass

from neoconstruct import (
    Byte, Bytes, Int8ub, Prefixed, PrefixedArray, StructMixin, field, wfield,
)

print("=== B2: Prefixed(Int8ub, Bytes(x+1)) ===")


@dataclass
class M1(StructMixin):
    x: int = field(Int8ub)
    p: bytes = field(Prefixed(Int8ub, Bytes(x + 1)))


# 期望语义：x=1 → 内层 Bytes 读 2 字节；Prefixed 长度字段=3（子流 3 字节）
# build: M1(x=1, p=b"ab").build() == b"\x01\x03ab"
try:
    print("M1 build:", M1(x=1, p=b"ab").build())
except Exception as e:
    print("M1 build FAIL:", type(e).__name__, str(e)[:200])

# parse: b"\x01\x02ab" → x=1, 长度=2, 子流 b"ab", inner Bytes(x+1=2) 读 "ab"
try:
    print("M1 parse:", M1.parse(b"\x01\x02ab"))
except Exception as e:
    print("M1 parse FAIL:", type(e).__name__, str(e)[:200])

print("=== B2b: 字段引用形态（v0.1.1-3 已修，对照） ===")


@dataclass
class M2(StructMixin):
    x: int = field(Int8ub)
    n: int = field(Int8ub)
    p: bytes = field(Prefixed(Int8ub, Bytes(n)))


try:
    print("M2 build:", M2(x=1, n=2, p=b"ab").build())
    print("M2 parse:", M2.parse(b"\x01\x02\x02ab"))
except Exception as e:
    print("M2 FAIL:", type(e).__name__, str(e)[:200])

print("=== B3: wfield(Bytes(x+1)) ===")


@dataclass
class W1(StructMixin):
    x: int = field(Int8ub)
    pad: bytes = wfield(Bytes(x + 1))


try:
    print("W1 build:", W1(x=1).build())
except Exception as e:
    print("W1 build FAIL:", type(e).__name__, str(e)[:200])
