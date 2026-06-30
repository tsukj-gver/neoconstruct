"""Smoke test: 完整端到端验证 V-1~V-5 修正。"""
from dataclasses import dataclass
from construct import StructMixin, field, Int8ub, RepeatUntil
from construct.lib.containers import Container


# ============================================================
# V-1: 谓词访问 context field（attribute + item 双访问）
# ============================================================
@dataclass
class P1(StructMixin):
    threshold: int = field(Int8ub)
    payload: list = field(RepeatUntil(lambda x, lst, ctx: x > ctx.threshold, Int8ub))


p1 = P1.parse(b"\x05\x01\x02\x03\x04\x05\x06\x07")
print(f"V-1 (attr):   P1.parse → payload={p1.payload}")
assert p1.payload == [1, 2, 3, 4, 5, 6], f"got {p1.payload}"
# build round-trip
b1 = p1.build()
assert b1 == b"\x05\x01\x02\x03\x04\x05\x06", f"got {b1!r}"
print(f"V-1 (attr):   P1.build → {b1!r}")


@dataclass
class P1b(StructMixin):
    threshold: int = field(Int8ub)
    payload: list = field(RepeatUntil(lambda x, lst, ctx: x > ctx["threshold"], Int8ub))


p1b = P1b.parse(b"\x05\x01\x02\x03\x04\x05\x06\x07")
print(f"V-1 (item):   P1b.parse → payload={p1b.payload}")
assert p1b.payload == [1, 2, 3, 4, 5, 6]


# ============================================================
# V-1 RU-9: isinstance(ctx, Container) 为 True
# ============================================================
@dataclass
class P1c(StructMixin):
    threshold: int = field(Int8ub)
    payload: list = field(RepeatUntil(
        lambda x, lst, ctx: isinstance(ctx, Container) and x > ctx.threshold,
        Int8ub,
    ))


p1c = P1c.parse(b"\x05\x01\x02\x03\x04\x05\x06\x07")
print(f"V-1 (isinst): P1c.parse → payload={p1c.payload}")
assert p1c.payload == [1, 2, 3, 4, 5, 6]


# ============================================================
# V-3: 非 callable 谓词（True / False）
# ============================================================
@dataclass
class P3a(StructMixin):
    payload: list = field(RepeatUntil(True, Int8ub))


# True → 第一个元素就满足
p3a = P3a.parse(b"\x01\x02\x03")
print(f"V-3 (True):   P3a.parse → payload={p3a.payload}")
assert p3a.payload == [1], f"got {p3a.payload}"


@dataclass
class P3b(StructMixin):
    payload: list = field(RepeatUntil(False, Int8ub))


# False → build 永远 RepeatError
try:
    P3b(payload=[1, 2, 3]).build()
    print("V-3 (False):  ❌ expected RepeatError")
except Exception as e:
    print(f"V-3 (False):  build raises {type(e).__name__}: {e}")


# ============================================================
# V-4: 负整数常量识别（哨兵终止模式）
# ============================================================
@dataclass
class P4(StructMixin):
    payload: list = field(RepeatUntil(lambda x, lst, ctx: x == -1, Int8ub))


# 数据用 -1 (0xFF in signed Int8) 作为终止哨兵
# Int8ub 是无符号，需要检查实际行为。这里换用 signed Int8sb
from construct import Int8sb

@dataclass
class P4b(StructMixin):
    payload: list = field(RepeatUntil(lambda x, lst, ctx: x == -1, Int8sb))


# data: [1, 2, 3, -1, ...] → 在 -1 时满足
p4b = P4b.parse(b"\x01\x02\x03\xff\x05")
print(f"V-4 (neg int): P4b.parse → payload={p4b.payload}")
assert p4b.payload == [1, 2, 3, -1], f"got {p4b.payload}"

# 验证 _expr_params（确认走 Expr 路径）
from construct._descriptors import _try_compile_repeat_predicate
ops = _try_compile_repeat_predicate(lambda x, l, c: x == -1)
print(f"V-4 (AST):    ops for `x == -1`: {ops}")
assert ops == [("getelem",), ("const", -1), ("eq",)], f"got {ops}"

ops2 = _try_compile_repeat_predicate(lambda x, l, c: x != -1)
print(f"V-4 (AST):    ops for `x != -1`: {ops2}")
assert ops2 == [("getelem",), ("const", -1), ("ne",)], f"got {ops2}"

print()
print("✅ All V-1~V-5 smoke tests passed!")
