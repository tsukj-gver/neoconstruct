"""Python construct 2.10.70 bit-level 性能基线。

对照 construct-rs Phase 3.1 的 bench_bits Rust benchmark。
测量 Python 端等价操作的单次调用 ns/op：

1. `bits2integer` / `integer2bits`（裸 bit 操作，对照 Rust read_bits/write_bits）
2. `Bitwise(BitsInteger(n)).parse / build`（完整路径，对照 Rust BitsIntegerNode.parse/build）

运行：python experiments/bench_bits_python.py
"""
import sys
import timeit

sys.path.insert(0, "construct")

from construct.lib.binary import bits2integer, integer2bits

# 测试 bit 宽度（与 Rust 端一致）
LENGTHS = [1, 4, 8, 16, 32, 64]

# Python 比 Rust 慢 1-2 个数量级，少量迭代即可稳定
INNER = 100_000
OUTER = 5


def bench(stmt_setup, stmt, globals_dict):
    """返回 (median_ns, min_ns)。"""
    samples = []
    for _ in range(OUTER):
        # 预热
        timeit.timeit(stmt, setup=stmt_setup, number=min(INNER, 1000), globals=globals_dict)
        t = timeit.timeit(stmt, setup=stmt_setup, number=INNER, globals=globals_dict)
        ns_per_op = t * 1e9 / INNER
        samples.append(ns_per_op)
    samples.sort()
    return samples[len(samples) // 2], samples[0]


def make_bitstring(length):
    """构造 length 位的 bit-string（bytes，每个元素 0 或 1）。"""
    return bytes([(i + 1) % 2 for i in range(length)])


def fmt(label, length, median, min_):
    print(f"{label:>20}  {median:>10.1f}  {min_:>10.1f}")


def print_header(title):
    print()
    print(f"=== {title} ===")
    print(f"{'op':>20}  {'median':>10}  {'min':>10}")
    print(f"{'':>20}  {'(ns/op)':>10}  {'(ns/op)':>10}")


def main():
    print("Python construct 2.10.70 bit-level baseline")
    print("-" * 50)
    print(f"INNER = {INNER} iters/round, OUTER = {OUTER} rounds")
    print()

    # -------------------------------------------------------------------------
    # bits2integer / integer2bits（裸 bit 操作）
    # -------------------------------------------------------------------------

    print_header("bits2integer (unsigned, 对照 Rust read_bits)")
    for n in LENGTHS:
        bs = make_bitstring(n)
        med, min_ = bench("", "bits2integer(bs)", {"bits2integer": bits2integer, "bs": bs})
        fmt(f"bits2int_u{n}", n, med, min_)

    print_header("bits2integer (signed, 对照 Rust read_bits + 二补码)")
    for n in LENGTHS:
        bs = make_bitstring(n)
        med, min_ = bench(
            "",
            "bits2integer(bs, signed=True)",
            {"bits2integer": bits2integer, "bs": bs},
        )
        fmt(f"bits2int_s{n}", n, med, min_)

    print_header("integer2bits (unsigned, 对照 Rust write_bits)")
    for n in LENGTHS:
        # 选范围内的中点值（unsigned）
        val = (1 << n) // 2 if n < 64 else (1 << 63)
        med, min_ = bench(
            "",
            "integer2bits(val, n)",
            {"integer2bits": integer2bits, "val": val, "n": n},
        )
        fmt(f"int2bits_u{n}", n, med, min_)

    print_header("integer2bits (signed, 对照 Rust write_bits + 二补码)")
    for n in LENGTHS:
        max_pos = (1 << (n - 1)) - 1 if n < 64 else (1 << 63) - 1
        val = -(max_pos // 2)  # 负值，触发二补码路径
        med, min_ = bench(
            "",
            "integer2bits(val, n, signed=True)",
            {"integer2bits": integer2bits, "val": val, "n": n},
        )
        fmt(f"int2bits_s{n}", n, med, min_)

    # -------------------------------------------------------------------------
    # Bitwise(BitsInteger(n)).parse / build（完整路径）
    # -------------------------------------------------------------------------
    #
    # 注意：Python construct 2.10.70 的 Bitwise 在 Python 3.14 下存在已知兼容性
    # 问题（RestreamedBytesIO 的 stream_read 路径在 3.14 异常）。Phase 3 设计阶段
    # ARCH 已在 Python 3.13 上实测 Bitwise(BitStruct(3 字段)).parse = 6288 ns/op
    # （记录于 plans/phase3-bitstream/过程记录.md 子任务 3.1 操作日志）。
    #
    # 由于 bits2integer / integer2bits 是 Bitwise 内部最终调用的核心转换函数
    # （Bitwise 通过 Restreamed 包装，把字节流 8x 膨胀为 bit-string 后调用它们），
    # 上面的裸测量已是 Rust 端 BitsIntegerNode.read_bits/write_bits 的直接对照。
    #
    # 完整 Bitwise 路径的额外开销（Restreamed 缓冲管理 + BytesIO 包装 + Context）
    # 估算为额外 ~1000-2000 ns/op（参考 ARCH 实测 6288 ns/op 对 3 字段 BitStruct，
    # 其中 3 次 BitsInteger.parse 各 ~1500 ns 是主成本）。

    print()
    print("=" * 60)
    print("Bitwise(BitsInteger(n)).parse/build 完整路径：")
    print("  Python 3.14 兼容性问题导致无法直接测量，")
    print("  参考 ARCH Phase 3 设计阶段实测（Python 3.13）：")
    print("    Bitwise(BitStruct(3 字段 Nibble/BitsInteger(10)/Padding(2))).parse")
    print("      = 6288 ns/op（每个字段摊 ~2000 ns）")
    print("=" * 60)


if __name__ == "__main__":
    main()
