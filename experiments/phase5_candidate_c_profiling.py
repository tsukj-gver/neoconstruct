"""Phase 5 候选 C 收益 profiling（P0-2 修正依据）。

目的：实测 StructNode.parse 字段循环的 per-field 边际成本，
反推候选 C fast-path 的真实可优化空间（验证设计 §4.3 "6-15ns 收益"是否成立）。

方法学（L-02 教训：实证数据替代理论估算）：
- 固定字段类型为 Int8ub（最简单的 1 字节格式字段，单次 parse ~20-30ns）
- 固定无表达式 / 无 __post_init__（候选 C 触发条件）
- 变化字段数：0, 1, 2, 3, 4, 5, 10
- 测量每个场景的 rs_ns（Rust 侧 ns/op）
- per-field 边际成本 = (rs_ns[N] - rs_ns[0]) / N
  这个边际成本包含：field.node.parse 实际工作 + match field.mode + dict.set_item + 循环迭代
- 候选 C 可优化的是边际成本中的"循环结构开销"（match + iterator bounds），
  而非"实际工作"（field.node.parse + dict.set_item）

判定：
- 若 per-field 边际成本中"循环结构开销"占比 <10%（<3ns/字段），候选 C 收益为
  0-3ns × 3 字段 = 0-9ns，远低于设计声称的 6-15ns，应下调。
- 若占比 10-25%（3-7ns/字段），候选 C 收益维持 6-15ns 估算。

输出：直接打印 + 写入 experiments/phase5_candidate_c_profiling_result.md
"""
import statistics
import sys
import timeit
from dataclasses import dataclass
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "construct-rs" / "python"))

from construct import StructMixin, field, Int8ub  # noqa: E402

NUMBER = 200_000  # 高采样数降低噪声
REPEAT = 7        # 多轮取最小（timeit 标准做法）
WARMUP = 50_000   # 预热 cache


def make_class(n_fields: int):
    """动态构造 N 字段的 StructMixin 子类（全部 Int8ub，无表达式，无 post_init）。"""
    namespace = {"__annotations__": {}}
    for i in range(n_fields):
        name = f"f{i}"
        namespace[name] = field(Int8ub)
        namespace["__annotations__"][name] = int
    cls = type(f"S{n_fields}", (StructMixin,), namespace)
    # 手动触发 __init_subclass__ + @dataclass 装饰（模拟正常用法）
    from dataclasses import dataclass as dc
    return dc(cls)


def bench_parse(n_fields: int) -> float:
    """测量 N 字段 Struct 的 parse ns/op（min of REPEAT rounds × NUMBER ops）。"""
    cls = make_class(n_fields)
    data = bytes(n_fields)  # 全 0 字节

    # 预热
    for _ in range(WARMUP):
        cls.parse(data)

    times = timeit.repeat(lambda: cls.parse(data), number=NUMBER, repeat=REPEAT)
    best_sec = min(times)
    ns_per_op = best_sec / NUMBER * 1e9
    return ns_per_op


def main():
    print(f"Python {sys.version}")
    print(f"NUMBER={NUMBER}, REPEAT={REPEAT}, WARMUP={WARMUP}")
    print()

    # 测量各字段数
    results = {}
    for n in [0, 1, 2, 3, 4, 5, 10]:
        ns = bench_parse(n)
        results[n] = ns
        print(f"  S{n:>2}.parse({n}B): {ns:7.2f} ns/op")

    print()
    print("=" * 70)
    print("Per-field 边际成本分析")
    print("=" * 70)

    base = results[0]
    print(f"\n基准（0 字段）rs_ns = {base:.2f} ns")
    print(f"  （含 Python 入口 + FFI + tp_new + getattr_dict + 实例返回）")
    print()
    print(f"{'字段数':>6} | {'rs_ns':>8} | {'Δ vs base':>10} | {'per-field 边际':>14}")
    print("-" * 60)

    marginal_costs = []
    for n in [1, 2, 3, 4, 5, 10]:
        delta = results[n] - base
        per_field = delta / n
        marginal_costs.append((n, per_field))
        print(f"{n:>6} | {results[n]:>7.2f}ns | {delta:>+9.2f}ns | {per_field:>11.2f} ns/field")

    print()
    print("=" * 70)
    print("候选 C 可优化空间反推")
    print("=" * 70)

    # per-field 边际成本 = field.node.parse（实际工作） + match field.mode + dict.set_item + iter 开销
    # 已知：Int8ub 的 parse 实际工作 ~10-15ns（读 1 字节 + 创建 PyLong）
    #       dict.set_item ~13.5ns（设计 §1.2，phase3 不对称分析实测）
    # 所以：循环结构开销 ≈ per_field - (10-15) - 13.5
    # 候选 C 可优化的是这个"循环结构开销"
    avg_marginal_1_3 = statistics.mean([m for n, m in marginal_costs if n in (1, 2, 3)])
    print(f"\n1-3 字段场景平均 per-field 边际成本: {avg_marginal_1_3:.2f} ns/field")
    print(f"  其中已知固定项（不可优化）：")
    print(f"    - Int8ub parse 实际工作 ≈ 10-15 ns（读 1 字节 + PyLong 创建）")
    print(f"    - PyDict_SetItem        ≈ 13.5 ns（设计 §1.2 实测）")
    print(f"  两者合计 ≈ 23.5-28.5 ns/field")
    print()
    loop_overhead_low = avg_marginal_1_3 - 28.5
    loop_overhead_high = avg_marginal_1_3 - 23.5
    print(f"循环结构开销估算（per-field 边际 - 固定项）：")
    print(f"  上界（固定项取下界 23.5）: {loop_overhead_high:+.2f} ns/field")
    print(f"  下界（固定项取上界 28.5）: {loop_overhead_low:+.2f} ns/field")
    print()
    print(f"候选 C fast-path 收益修正（3 字段 × 循环开销）：")
    save_low = max(0, loop_overhead_low) * 3
    save_high = max(0, loop_overhead_high) * 3
    print(f"  下界：{save_low:.2f} ns")
    print(f"  上界：{save_high:.2f} ns")
    print(f"  中位：{(save_low + save_high) / 2:.2f} ns")
    print()
    print(f"对比设计 §4.3 原估算：6-15 ns")
    if save_high < 6:
        verdict = "实际收益 <6ns，原估算 6-15ns 显著高估（应下调至 0-{}ns）".format(int(save_high))
    elif save_low < 3:
        verdict = "实际收益 0-{}ns，原估算 6-15ns 高估（应下调）".format(int(save_high))
    else:
        verdict = f"实际收益 {save_low:.1f}-{save_high:.1f}ns，与原估算 6-15ns 接近"
    print(f"结论：{verdict}")

    # 写入结果文件
    out = Path(__file__).parent / "phase5_candidate_c_profiling_result.md"
    lines = [
        "# Phase 5 候选 C 收益 profiling 结果",
        "",
        f"测量时间：{__import__('datetime').datetime.now().isoformat(timespec='seconds')}",
        f"Python：{sys.version.split()[0]}",
        f"采样：NUMBER={NUMBER}, REPEAT={REPEAT}, WARMUP={WARMUP}（min of REPEAT）",
        "",
        "## 原始测量数据（rs_ns = Rust 侧 ns/op）",
        "",
        "| 字段数 | rs_ns (ns/op) |",
        "|-------:|--------------:|",
    ]
    for n in [0, 1, 2, 3, 4, 5, 10]:
        lines.append(f"| {n} | {results[n]:.2f} |")
    lines += [
        "",
        "## Per-field 边际成本",
        "",
        "| 字段数 | Δ vs 0 字段 | per-field 边际 |",
        "|------:|-----------:|--------------:|",
    ]
    for n in [1, 2, 3, 4, 5, 10]:
        delta = results[n] - results[0]
        lines.append(f"| {n} | {delta:+.2f} ns | {delta/n:.2f} ns/field |")
    lines += [
        "",
        "## 候选 C 可优化空间反推",
        "",
        f"- 1-3 字段平均 per-field 边际成本：{avg_marginal_1_3:.2f} ns/field",
        f"- 已知固定项（Int8ub parse ~10-15ns + PyDict_SetItem 13.5ns）：23.5-28.5 ns/field",
        f"- 循环结构开销估算：{loop_overhead_low:+.2f} ~ {loop_overhead_high:+.2f} ns/field",
        f"- 3 字段总可优化空间：{save_low:.2f} - {save_high:.2f} ns",
        "",
        f"## 结论",
        "",
        f"- 设计 §4.3 原估算候选 C 收益：6-15 ns",
        f"- 实测修正后估算：{save_low:.1f} - {save_high:.1f} ns",
        f"- {verdict}",
    ]
    out.write_text("\n".join(lines), encoding="utf-8")
    print(f"\n结果已写入：{out}")


if __name__ == "__main__":
    main()
