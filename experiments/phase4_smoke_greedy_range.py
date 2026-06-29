"""Phase 4 子任务 4.2 端到端冒烟测试。"""
from dataclasses import dataclass
from construct import StructMixin, field, Int8ub, Int16ub, GreedyRange


@dataclass
class P(StructMixin):
    items: list = field(GreedyRange(Int8ub))


# 1) 基本 parse 读到 EOF
data = bytes([1, 2, 3, 4, 5])
p = P.parse(data)
print("parse:", p.items)
assert p.items == [1, 2, 3, 4, 5], p.items


# 2) 子构造器失败回退（Int16ub 残留 1 字节）
@dataclass
class P2(StructMixin):
    items: list = field(GreedyRange(Int16ub))


p2 = P2.parse(bytes([0, 1, 2, 3, 4]))
print("fallback:", p2.items)
assert p2.items == [1, 515], p2.items  # 0x0001=1, 0x0203=515, 第5字节回退


# 3) discard=True
@dataclass
class P3(StructMixin):
    items: list = field(GreedyRange(Int8ub, discard=True))


p3 = P3.parse(bytes([10, 20, 30]))
print("discard:", p3.items)
assert p3.items == [], p3.items


# 4) build
p.items = [100, 110, 120]
out = p.build()
print("build:", out)
assert out == bytes([100, 110, 120]), out


# 5) sizeof 报错
try:
    P.sizeof()
    print("sizeof: FAIL (should raise)")
except Exception as e:
    print("sizeof error:", type(e).__name__)


# 6) 空流
p_empty = P.parse(b"")
print("empty stream:", p_empty.items)
assert p_empty.items == []

print("ALL OK")
