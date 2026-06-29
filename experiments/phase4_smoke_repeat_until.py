"""Phase 4 子任务 4.5 冒烟测试：RepeatUntil (Expr + PyCallable 双路径)。"""

import sys
from dataclasses import dataclass

try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
except Exception:
    pass

from construct import (
    StructMixin, field, Int8ub, RepeatUntil, RepeatError,
)


@dataclass
class PacketExpr(StructMixin):
    """Expr 路径：简单 lambda 自动编译为 ExprProgram。"""
    payload: list = field(RepeatUntil(lambda x, lst, ctx: x > 5, Int8ub))


@dataclass
class PacketCallable(StructMixin):
    """PyCallable 路径：复杂 lambda 回落到 Python callable。"""
    payload: list = field(
        RepeatUntil(lambda x, lst, ctx: len(lst) >= 2 and lst[-2:] == [0, 0], Int8ub)
    )


@dataclass
class PacketDiscard(StructMixin):
    """discard=True：parse 返回空 list 但消耗流。"""
    payload: list = field(RepeatUntil(lambda x, lst, ctx: x > 5, Int8ub, discard=True))


def main():
    print("=== Test 1: Expr path parse + build ===")
    data = bytes([1, 2, 3, 4, 5, 6, 7, 8])
    pkt = PacketExpr.parse(data)
    print(f"  parse({list(data)}) = {pkt}")
    assert pkt.payload == [1, 2, 3, 4, 5, 6], f"got {pkt.payload}"
    built = pkt.build()
    print(f"  build() = {list(built)}")
    assert built == bytes([1, 2, 3, 4, 5, 6])
    print("  PASS")

    print("=== Test 2: PyCallable path parse + build ===")
    data = bytes([1, 0, 0, 9, 9])
    pkt = PacketCallable.parse(data)
    print(f"  parse({list(data)}) = {pkt}")
    assert pkt.payload == [1, 0, 0], f"got {pkt.payload}"
    built = pkt.build()
    print(f"  build() = {list(built)}")
    assert built == bytes([1, 0, 0])
    print("  PASS")

    print("=== Test 3: discard=True (parse) ===")
    data = bytes([1, 2, 3, 4, 5, 6, 7, 8])
    pkt = PacketDiscard.parse(data)
    print(f"  parse({list(data)}) = {pkt}")
    assert pkt.payload == [], f"got {pkt.payload}"
    print("  PASS")

    print("=== Test 4: RepeatError on build (no match) ===")
    try:
        PacketExpr(items=[1, 2, 3]).build()  # type: ignore[arg-type]
        print("  FAIL: expected RepeatError")
        sys.exit(1)
    except RepeatError as e:
        print(f"  Got expected RepeatError: {e}")
    except Exception as e:
        # PacketExpr 不接 items 参数；用正确的构造方式
        pass

    # 用正确方式触发 RepeatError
    try:
        pkt = PacketExpr(payload=[1, 2, 3])  # 都不 > 5
        pkt.build()
        print("  FAIL: expected RepeatError")
        sys.exit(1)
    except RepeatError as e:
        print(f"  Got expected RepeatError: {e}")
        print("  PASS")

    print("=== Test 5: Round-trip ===")
    pkt = PacketExpr(payload=[1, 2, 3, 100])
    built = pkt.build()
    print(f"  PacketExpr([1,2,3,100]).build() = {list(built)}")
    assert built == bytes([1, 2, 3, 100])
    pkt2 = PacketExpr.parse(built)
    assert pkt2.payload == [1, 2, 3, 100]
    print("  PASS")

    print()
    print("=== All smoke tests PASSED ===")


if __name__ == "__main__":
    main()
