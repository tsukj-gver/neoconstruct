"""Python construct 2.10.70 Phase 3.3 性能基线。

对照 construct-rs Phase 3.3 的 bench_phase33 Rust benchmark，测量等价构造器的
parse/build ns/op。涵盖 Bytewise、BitsSwapped、ByteSwapped、Padding、端到端 BitStruct。

环境：CPython 3.14.2 + construct 2.10.70
（Python 3.14 下 Bitwise/BitStruct 等已实测可用，详见 plans/phase3-bitstream/过程记录.md
 子任务 3.3 性能补测段）

运行：python experiments/bench_phase33_python.py
"""

import sys
import timeit

sys.path.insert(0, "construct")

from construct import (
    Bitwise,
    BitsInteger,
    BitStruct,
    Padding,
    Bytes,
    Bytewise,
    ByteSwapped,
    BitsSwapped,
    Struct,
    Nibble,
    Int8ub,
    Int16ub,
    Int32ub,
)

# Python 比 Rust 慢 1-2 个数量级，少量迭代即可稳定
INNER = 20_000
OUTER = 5


def bench(stmt, globs):
    """返回 (median_ns, min_ns)。"""
    samples = []
    for _ in range(OUTER):
        # 预热
        timeit.timeit(stmt, setup="", number=min(INNER, 500), globals=globs)
        t = timeit.timeit(stmt, setup="", number=INNER, globals=globs)
        samples.append(t * 1e9 / INNER)
    samples.sort()
    return samples[len(samples) // 2], samples[0]


# 场景：(name, fmt, parse_data, build_obj_factory)
# build_obj_factory 是返回 build 输入的 lambda（避免在 module load 时构造 Container）
scenarios = [
    (
        "Bitwise(BitsInteger(8))",
        Bitwise(BitsInteger(8)),
        b"\xA5",
        lambda: 0xA5,
    ),
    (
        "Bitwise(BitsInteger(16))",
        Bitwise(BitsInteger(16)),
        b"\xA5\x3C",
        lambda: 0xA53C,
    ),
    (
        "Bitwise(Bytewise(Int8ub))",
        Bitwise(Bytewise(Int8ub)),
        b"\x42",
        lambda: 0x42,
    ),
    (
        "Bitwise(Bytewise(Bytes(4)))",
        Bitwise(Bytewise(Bytes(4))),
        b"\xA5\x3C\x96\xC3",
        lambda: b"\xA5\x3C\x96\xC3",
    ),
    (
        "BitStruct(Nibble, Bytewise(Int16ub), Nibble)",
        BitStruct("a" / Nibble, "b" / Bytewise(Int16ub), "c" / Nibble),
        b"\xA1\x23\x4B",
        lambda: {"a": 0xA, "b": 0x1234, "c": 0xB},
    ),
    (
        "BitsSwapped(Bytes(4))",
        BitsSwapped(Bytes(4)),
        b"\xF0\x0F\xAA\x55",
        lambda: b"\x0F\xF0\x55\xAA",
    ),
    (
        "ByteSwapped(Int32ub)",
        ByteSwapped(Int32ub),
        b"\x78\x56\x34\x12",
        lambda: 0x12345678,
    ),
    (
        "BitStruct(Nibble, BitPadding(4))",
        BitStruct("a" / Nibble, "pad" / Padding(4)),
        b"\xA5",
        lambda: {"a": 0xA, "pad": None},
    ),
    (
        "Struct(Bytes(1), Padding(4), Bytes(2))",
        Struct("tag" / Bytes(1), "pad" / Padding(4), "data" / Bytes(2)),
        b"\xAA\x00\x00\x00\x00\xBB\xCC",
        lambda: {"tag": b"\xAA", "pad": None, "data": b"\xBB\xCC"},
    ),
    (
        "BitStruct(Nibble, BitsInteger(10), BitPadding(2))",
        BitStruct("a" / Nibble, "b" / BitsInteger(10), "c" / Padding(2)),
        b"\xBE\xEF",
        lambda: {"a": 0xB, "b": 0x1F7, "c": None},
    ),
]


def main():
    print("Python construct 2.10.70 Phase 3.3 baseline")
    print("-" * 70)
    print(f"INNER = {INNER} iters/round, OUTER = {OUTER} rounds (取 median + min)")
    print()
    print(
        f"{'scenario':<48}  {'parse_med':>9}  {'parse_min':>9}  "
        f"{'build_med':>9}  {'build_min':>9}"
    )
    print(
        f"{'':<48}  {'(ns/op)':>9}  {'(ns/op)':>9}  "
        f"{'(ns/op)':>9}  {'(ns/op)':>9}"
    )

    for name, fmt, parse_data, build_factory in scenarios:
        try:
            parsed = fmt.parse(parse_data)
        except Exception as e:
            print(
                f"{name:<48}  PARSE ERR: {type(e).__name__}: {str(e)[:60]}"
            )
            continue

        # 把 parsed 确定为 build 输入。对 BitStruct 返回 Container，可直接 build。
        # 对于直接 build 的对象（如 BitsSwapped(Bytes)），parse 返回 bytes，build 接收 bytes；
        # 但 BitsSwapped(Bytes(4)) 的 build 输入是变换前的 bytes（即 0F F0 55 AA）。
        # 这与 Rust 端的 to_pybytes 一致。
        build_input = build_factory()

        try:
            # 对于 Struct，build_input 是 dict；construct 接受 dict 或 Container
            fmt.build(build_input)
        except Exception as e:
            print(
                f"{name:<48}  BUILD ERR: {type(e).__name__}: {str(e)[:60]}"
            )
            continue

        # 跑 parse benchmark
        parse_med, parse_min = bench(
            "fmt.parse(parse_data)",
            {"fmt": fmt, "parse_data": parse_data},
        )

        # 跑 build benchmark
        # 对 dict 输入需要每次 fresh 的 dict 防止 Python 端可能 mutate（实际不会，但保险）
        build_med, build_min = bench(
            "fmt.build(build_input)",
            {"fmt": fmt, "build_input": build_input},
        )

        print(
            f"{name:<48}  {parse_med:>9.0f}  {parse_min:>9.0f}  "
            f"{build_med:>9.0f}  {build_min:>9.0f}"
        )


if __name__ == "__main__":
    main()
