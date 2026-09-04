"""v0.1.2-1 spike A/B 微基准 + 机制验证。

机制验证：
  1. v0/v1 build 输出字节一致（行为等价）
  2. v0 ctx 的 'c' 槽位 = None（B1e 缺陷形态：None 流入表达式）
  3. v1 ctx 的 'c' 槽位 = 5（有效值回写，B1e 的机制解）
  4. v1 Instance 路径 None → 明确报错（对齐 construct KeyError 时机后移）

性能（L-09 方法：同会话交替 A/B，≥3 轮，统计判据 |Δ| < 0.3x 波动阈）：
  v0 = 现状形态（FieldMode match + 节点内 None 特判 + ctx 写原始值）
  v1 = 框架形态（ValueKind match + resolve + ctx 写有效值）
"""
import statistics
import time

import valuekind_spike as vs


class Obj:
    """模拟用户实例：x0/x1/x2 正常值，c/d 为隐式 default=None（B1 场景）。"""

    __slots__ = ("x0", "x1", "x2", "c", "d")

    def __init__(self):
        self.x0 = 1
        self.x1 = 2
        self.x2 = b"ab"
        self.c = None
        self.d = None


class ObjNoneX0:
    __slots__ = ("x0", "x1", "x2", "c", "d")

    def __init__(self):
        self.x0 = None
        self.x1 = 2
        self.x2 = b"ab"
        self.c = None
        self.d = None


def mechanism_checks():
    o = Obj()
    b0 = vs.build_v0(o)
    b1 = vs.build_v1(o)
    print("[1] v0/v1 字节一致:", b0 == b1, b1.hex())
    ctx0 = vs.probe_ctx_v0(o)
    ctx1 = vs.probe_ctx_v1(o)
    print("[2] v0 ctx['c'] =", repr(ctx0["c"]), "（缺陷形态：None）")
    print("[3] v1 ctx['c'] =", repr(ctx1["c"]), "/ ctx['r'] =", repr(ctx1["r"]),
          "（有效值回写 + Derived 读有效 x0）")
    try:
        vs.build_v1(ObjNoneX0())
        print("[4] v1 None 实例值：未报错（异常！）")
    except ValueError as e:
        print("[4] v1 None 实例值 -> ValueError:", e)
    # v0 对 None 实例值的表现（x0=None 会 extract 失败）
    try:
        vs.build_v0(ObjNoneX0())
        print("[4b] v0 None 实例值：未报错")
    except Exception as e:
        print("[4b] v0 None 实例值 ->", type(e).__name__, str(e)[:60])


def bench(iters=20000, batches=9, rounds=3):
    o = Obj()
    # warmup
    for _ in range(2000):
        vs.build_v0(o)
        vs.build_v1(o)

    results = {"v0": [], "v1": []}
    for r in range(rounds):
        samples = {"v0": [], "v1": []}
        for b in range(batches):
            for ver, fn in (("v0", vs.build_v0), ("v1", vs.build_v1)):
                t0 = time.perf_counter_ns()
                for _ in range(iters):
                    fn(o)
                dt = time.perf_counter_ns() - t0
                samples[ver].append(dt / iters)
        m0 = statistics.median(samples["v0"])
        m1 = statistics.median(samples["v1"])
        results["v0"].append(m0)
        results["v1"].append(m1)
        print(f"round {r+1}: v0 median {m0:8.2f} ns/op | v1 median {m1:8.2f} ns/op "
              f"| Δ {m1-m0:+7.2f} ns ({(m1-m0)/m0*100:+5.2f}%)")
    g0 = statistics.median(results["v0"])
    g1 = statistics.median(results["v1"])
    delta = (g1 - g0) / g0
    print(f"\n总中位: v0 {g0:.2f} ns/op | v1 {g1:.2f} ns/op | Δ {delta*100:+.2f}%")
    print("判据（L-09）: |Δ| < 30%（波动阈） →", "PASS 无回退" if abs(delta) < 0.30 else "FAIL 需分析")


if __name__ == "__main__":
    print("=== 机制验证 ===")
    mechanism_checks()
    print("\n=== A/B 微基准（同会话交替，3 轮 × 9 批 × 20000 次）===")
    bench()
