"""用户报告场景复现：Const 字段被表达式消费 × build/parse 组合。"""
from dataclasses import dataclass

from neoconstruct import Byte, Bytes, Const, Int8ub, StructMixin, field

print("=== A: Const x + Bytes(x+1) build ===")


@dataclass
class A(StructMixin):
    x: int = field(Const(1, Byte))
    y: bytes = field(Bytes(x + 1))


try:
    print("A().build():", A().build())
except Exception as e:
    print("A().build() FAIL:", type(e).__name__, str(e)[:150])
try:
    print("A(y=b'ab').build():", A(y=b"ab").build())
except Exception as e:
    print("A(y=..).build() FAIL:", type(e).__name__, str(e)[:150])

print("=== B: 同结构 parse（x=1 校验过, y 应读 x+1=2 字节）===")
try:
    r = A.parse(b"\x01\x02ab")
    print("A.parse OK:", r)
except Exception as e:
    print("A.parse FAIL:", type(e).__name__, str(e)[:150])

print("=== C: 普通 Int8ub x + Bytes(x+1) build ===")


@dataclass
class C(StructMixin):
    x: int = field(Int8ub)
    y: bytes = field(Bytes(x + 1))


try:
    print("C build:", C(x=1, y=b"ab").build())
except Exception as e:
    print("C build FAIL:", type(e).__name__, str(e)[:150])
