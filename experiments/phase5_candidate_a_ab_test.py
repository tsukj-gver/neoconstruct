"""Phase 5 候选 A 实际收益微基准（补充 L-02 实证）。

目的：实测"cls.parse = schema._parse_raw 去层化"相比"StructMixin.parse classmethod"
的实际 ns 节省，验证设计 §7.1 候选 A "30-50ns 收益"估算。

方法：
- Controlled A/B Test（L-09 教训）：同一进程同一 schema 实例，
  A 状态：StructMixin.parse classmethod 路径（当前实现）
  B 状态：cls.parse = schema._parse_raw fast binding 路径（候选 A）
- 交替测量 A→B→A→B 至少 3 轮，每轮 200000 ops × 7 repeat
- 取 min(repeat) 降低噪声
"""
import statistics
import sys
import timeit
from dataclasses import dataclass
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "construct-rs" / "python"))

from construct import StructMixin, field, Int8ub  # noqa: E402

NUMBER = 200_000
REPEAT = 7
ROUNDS = 3  # A/B 交替轮数
WARMUP = 50_000


@dataclass
class B1Struct(StructMixin):
    """B1 场景：3 个 Int8ub 字段，无表达式，无 post_init。"""
    a: int = field(Int8ub)
    b: int = field(Int8ub)
    c: int = field(Int8ub)


schema = B1Struct._construct_compiled
data = b"\x01\x02\x03"


def bench_current():
    """A 状态：当前 StructMixin.parse classmethod 路径。"""
    # 确保走 classmethod（删除可能存在的 fast binding）
    for attr in ("_fast_binding",):
        try:
            delattr(B1Struct, attr)
        except AttributeError:
            pass
    # 确保 parse 走 StructMixin.parse（删除子类覆盖）
    try:
        delattr(B1Struct, "parse")
    except AttributeError:
        pass

    # 预热
    for _ in range(WARMUP):
        B1Struct.parse(data)
    times = timeit.repeat(lambda: B1Struct.parse(data), number=NUMBER, repeat=REPEAT)
    return min(times) / NUMBER * 1e9


def bench_fast_binding():
    """B 状态：候选 A fast binding 路径（cls.parse = schema._parse_raw）。"""
    B1Struct.parse = schema._parse_raw  # 挂载 fast binding

    # 预热
    for _ in range(WARMUP):
        B1Struct.parse(data)
    times = timeit.repeat(lambda: B1Struct.parse(data), number=NUMBER, repeat=REPEAT)
    return min(times) / NUMBER * 1e9


def reset():
    """恢复 B1Struct.parse 到继承的 StructMixin.parse。"""
    try:
        delattr(B1Struct, "parse")
    except AttributeError:
        pass


def main():
    print(f"Python {sys.version.split()[0]}")
    print(f"NUMBER={NUMBER}, REPEAT={REPEAT}, A/B 交替 ROUNDS={ROUNDS}")
    print()

    # 验证两条路径输出一致
    reset()
    r1 = B1Struct.parse(data)
    B1Struct.parse = schema._parse_raw
    r2 = B1Struct.parse(data)
    reset()
    assert (r1.a, r1.b, r1.c) == (r2.a, r2.b, r2.c) == (1, 2, 3), "两条路径输出不一致"
    print(f"  正确性验证：两条路径输出一致 (a,b,c)=(1,2,3) ✓")
    print()

    # Controlled A/B Test
    a_results = []
    b_results = []

    print(f"{'轮次':>4} | {'A (current) ns':>16} | {'B (fast) ns':>16} | {'B-A (节省)':>12}")
    print("-" * 60)

    for round_idx in range(1, ROUNDS + 1):
        # A → B → A → B 交替（每轮内部也交替）
        a1 = bench_current()
        b1 = bench_fast_binding()
        a2 = bench_current()
        b2 = bench_fast_binding()
        reset()

        a_avg = (a1 + a2) / 2
        b_avg = (b1 + b2) / 2
        delta = b_avg - a_avg  # 负值表示 B 更快（节省）

        a_results.extend([a1, a2])
        b_results.extend([b1, b2])
        print(f"{round_idx:>4} | {a_avg:>15.2f}ns | {b_avg:>15.2f}ns | {delta:>+11.2f}ns")

    print()
    print("=" * 60)
    print("统计汇总")
    print("=" * 60)

    a_min = min(a_results)
    a_med = statistics.median(a_results)
    b_min = min(b_results)
    b_med = statistics.median(b_results)

    print(f"\nA 状态（current classmethod）:")
    print(f"  min  = {a_min:.2f} ns/op")
    print(f"  med  = {a_med:.2f} ns/op")
    print(f"\nB 状态（fast binding）:")
    print(f"  min  = {b_min:.2f} ns/op")
    print(f"  med  = {b_med:.2f} ns/op")

    delta_min = a_min - b_min
    delta_med = a_med - b_med
    print(f"\n候选 A 实际节省（B 比 A 快多少）:")
    print(f"  min-min: {delta_min:+.2f} ns/op")
    print(f"  med-med: {delta_med:+.2f} ns/op")

    print()
    print("=" * 60)
    print("对比设计 §7.1 估算")
    print("=" * 60)
    print(f"\n设计 §7.1 候选 A 收益估算：30-50 ns")
    print(f"实测（controlled A/B）：{delta_min:.1f} - {delta_med:.1f} ns")
    if delta_min < 30:
        print(f"⚠️ 实测下界 {delta_min:.1f}ns 低于设计估算下界 30ns")
        print(f"   原因分析：")
        print(f"   - Python 3.14 classmethod 调用开销可能低于设计估算的 ~10-15ns")
        print(f"   - 属性查找 _construct_compiled 命中 type dict 缓存，开销可能低于 ~20-30ns")
        print(f"   - bound method 分发开销可能低于 ~30-50ns")
    elif delta_med > 50:
        print(f"✅ 实测上界 {delta_med:.1f}ns 高于设计估算上界 50ns，候选 A 收益被低估")
    else:
        print(f"✅ 实测落入设计估算区间 30-50ns")

    # 写入结果
    out = Path(__file__).parent / "phase5_candidate_a_ab_result.md"
    lines = [
        "# Phase 5 候选 A Controlled A/B Test 结果",
        "",
        f"测量时间：{__import__('datetime').datetime.now().isoformat(timespec='seconds')}",
        f"Python：{sys.version.split()[0]}",
        f"采样：NUMBER={NUMBER}, REPEAT={REPEAT}, A/B 交替 ROUNDS={ROUNDS}",
        "",
        "## 场景",
        "",
        "- B1：3 个 Int8ub 字段 Struct（无表达式，无 post_init）",
        "- A 状态：StructMixin.parse classmethod 路径（当前实现）",
        "- B 状态：`cls.parse = schema._parse_raw` fast binding（候选 A）",
        "",
        "## 统计汇总",
        "",
        f"- A 状态 min: {a_min:.2f} ns/op, median: {a_med:.2f} ns/op",
        f"- B 状态 min: {b_min:.2f} ns/op, median: {b_med:.2f} ns/op",
        f"- 候选 A 实际节省: min-min {delta_min:+.2f} ns, med-med {delta_med:+.2f} ns",
        "",
        "## 对比设计 §7.1",
        "",
        f"- 设计估算：30-50 ns",
        f"- 实测：{delta_min:.1f} - {delta_med:.1f} ns",
        "",
        "## 逐轮原始数据",
        "",
        "| 轮次 | A min ns | B min ns | Δ (B-A) |",
        "|-----:|---------:|---------:|--------:|",
    ]
    for i in range(ROUNDS):
        a_avg = (a_results[i*2] + a_results[i*2+1]) / 2
        b_avg = (b_results[i*2] + b_results[i*2+1]) / 2
        lines.append(f"| {i+1} | {a_avg:.2f} | {b_avg:.2f} | {b_avg - a_avg:+.2f} |")
    out.write_text("\n".join(lines), encoding="utf-8")
    print(f"\n结果已写入：{out}")


if __name__ == "__main__":
    main()
