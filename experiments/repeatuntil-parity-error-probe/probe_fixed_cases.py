"""探针：G2 修复后 R1/R2/R3 的 rs 侧验证（Element 表达式路径）。

- R1: RepeatUntil(e > 5, Int8ub) —— Element 表达式
- R2: RepeatUntil(e == stop, Int8ub) —— 哨兵来自前序字段（跨字段引用）
- R3: RepeatUntil(e != -1, Int8sb) —— 负数哨兵常量

验证 parse 结果 + build 字节 + roundtrip。
"""

import sys
import traceback

sys.path.insert(0, r"D:\Project\Github\neoconstruct\neoconstruct\python")

from dataclasses import dataclass  # noqa: E402

from neoconstruct import (  # noqa: E402
    Element,
    Int8sb,
    Int8ub,
    RepeatUntil,
    StructMixin,
    field,
    rfield,
)


def probe_r1():
    @dataclass
    class P(StructMixin):
        e: int = rfield(Element())
        items: list = field(RepeatUntil(e > 5, Int8ub))

    pkt = P.parse(bytes([1, 2, 3, 4, 5, 6, 7, 8]))
    print("  parse items =", pkt.items)
    built = P.build(P(items=[1, 2, 3, 4, 5, 6]))
    print("  build bytes =", built.hex())
    print("  roundtrip   =", P.parse(built).items)


def probe_r2():
    @dataclass
    class P(StructMixin):
        stop: int = field(Int8ub)
        e: int = rfield(Element())
        items: list = field(RepeatUntil(e == stop, Int8ub))

    pkt = P.parse(bytes([5, 1, 2, 5, 9]))
    print("  stop =", pkt.stop, "items =", pkt.items)
    built = P.build(P(stop=5, items=[1, 2, 5]))
    print("  build bytes =", built.hex())
    print("  roundtrip   =", P.parse(built).items)


def probe_r3():
    @dataclass
    class P(StructMixin):
        e: int = rfield(Element())
        items: list = field(RepeatUntil(e == -1, Int8sb))

    pkt = P.parse(bytes([1, 2, 3, 0xFF, 9]))
    print("  parse items =", pkt.items)
    built = P.build(P(items=[1, 2, 3, -1]))
    print("  build bytes =", built.hex())
    print("  roundtrip   =", P.parse(built).items)


if __name__ == "__main__":
    for name in ("probe_r1", "probe_r2", "probe_r3"):
        print(f"===== {name} =====")
        try:
            globals()[name]()
            print("  [OK]")
        except Exception:
            for line in traceback.format_exc().splitlines()[-8:]:
                print("  " + line)
        print()
