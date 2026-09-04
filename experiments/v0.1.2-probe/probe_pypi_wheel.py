"""在 PyPI 0.1.1 wheel 环境下复现 B2：expected 97, found 1。"""
from dataclasses import dataclass

import neoconstruct
print("neoconstruct from:", neoconstruct.__file__)

from neoconstruct import Byte, Bytes, Int8ub, Prefixed, StructMixin, field

print("=== V1: 普通字段 + Prefixed(Bytes(x+1)) ===")


@dataclass
class V1(StructMixin):
    x: int = field(Int8ub)
    p: bytes = field(Prefixed(Int8ub, Bytes(x + 1)))


try:
    print("V1 parse:", V1.parse(b"\x01\x02ab"))
    print("V1 build:", V1(x=1, p=b"ab").build())
except Exception as e:
    print("V1 FAIL:", type(e).__name__, str(e)[:200])

print("=== V2: 数据含 97 的形态（探 stream 错位） ===")
# x=1, 长度=2, 子流 b"ab"; 若错位，长度字段读到 'a'(97) → 读 97 字节只 1 字节
try:
    print("V1 parse2:", V1.parse(b"\x01\x61\x62"))
except Exception as e:
    print("V1 parse2 FAIL:", type(e).__name__, str(e)[:200])
