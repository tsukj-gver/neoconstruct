"""B2 触发条件补充：x=Const 场景的 parse 独立测试 + 错读形态检查。"""
from dataclasses import dataclass

from neoconstruct import Byte, Bytes, Const, Int8ub, Prefixed, StructMixin, field

print("=== U1: x=Const(1) + Prefixed(Int8ub, Bytes(x+1)) parse ===")


@dataclass
class U1(StructMixin):
    x: int = field(Const(1, Byte))
    p: bytes = field(Prefixed(Int8ub, Bytes(x + 1)))


try:
    print("U1 parse:", U1.parse(b"\x01\x02ab"))
except Exception as e:
    print("U1 parse FAIL:", type(e).__name__, str(e)[:250])

print("=== U2: 前置字段错位检查——Const(97) 在 Prefixed(Bytes(x+1)) 之后 ===")


@dataclass
class U2(StructMixin):
    x: int = field(Int8ub)
    p: bytes = field(Prefixed(Int8ub, Bytes(x + 1)))
    c: int = field(Const(97, Byte))  # 期望 'a'


# 数据: x=0x05, 长度=6? 不——正确语义: x=5, inner Bytes(6) 读 6 字节, 长度=6
# 构造正确数据: b"\x05" + b"\x06" + b"ab\xcd\xef" (6 字节) + b"a" (Const 97)
data = b"\x05\x06abcdef" + b"a"
try:
    r = U2.parse(data)
    print("U2 parse:", r)
except Exception as e:
    print("U2 parse FAIL:", type(e).__name__, str(e)[:250])

print("=== U3: x=Const + Prefixed(Bytes(x+1)) roundtrip via parse ===")
try:
    r = U1.parse(b"\x01\x02ab")
    print("U1 roundtrip build:", r.build())
except Exception as e:
    print("U1 roundtrip FAIL:", type(e).__name__, str(e)[:250])
