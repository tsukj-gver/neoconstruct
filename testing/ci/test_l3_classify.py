"""L3 判据单元测试（构造 mock 验证 classify 函数的所有分支）。

覆盖：
- 边界场景（rs_ns<300 / speedup in [9,11]）：>0.5 WARN, >1 FAIL
- 普通场景：>10% WARN, >20% FAIL
- 特殊场景：baseline>=10x → new<10x 直接 FAIL（无 WARN）
- 性能提升：IMPROVED
- 豁免：FAIL → WARN
- 无测量：SKIPPED
- 无 baseline：NEW
"""
from __future__ import annotations

import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from run_l3_perf import (  # noqa: E402
    BaselinePoint, MeasuredPoint, Verdict,
    classify, load_baseline, load_exemptions,
    DEFAULT_BASELINE, DEFAULT_EXEMPTIONS,
)


def _mkbaseline(speedup_x, rs_ns=None, ctor="X", sid="S", direction="parse"):
    return BaselinePoint(
        constructor=ctor, scenario_id=sid, direction=direction,
        speedup_x=speedup_x, rs_ns_per_call=rs_ns,
        meets_10x=(speedup_x is not None and speedup_x >= 10.0),
        phase="test",
    )


def _mkmeasured(speedup, rs_ns=500.0):
    return MeasuredPoint(rs_ns=rs_ns, pc_ns=rs_ns * speedup, speedup=speedup)


def test_boundary_warn():
    """边界场景（rs<300ns），delta_x 在 (0.5, 1.0] → WARN。

    注意：baseline 必须 <10x，避免触发特殊场景硬约束 #5（覆盖边界判定）。
    """
    bp = _mkbaseline(9.5, rs_ns=200.0, sid="boundary_warn")  # <10x + rs<300ns
    mp = _mkmeasured(8.7, rs_ns=200.0)  # delta_x = -0.8
    v = classify(bp, mp, exemptions={})
    assert v.category == "boundary", v
    assert v.verdict == "WARN", f"expected WARN got {v.verdict}"
    assert abs(v.delta_x - (-0.8)) < 1e-9


def test_boundary_fail():
    """边界场景，|delta_x| > 1.0 → FAIL（baseline <10x 避免触发特殊）。"""
    bp = _mkbaseline(9.5, rs_ns=200.0, sid="boundary_fail")  # <10x + rs<300ns
    mp = _mkmeasured(7.5, rs_ns=200.0)  # delta_x = -2.0
    v = classify(bp, mp, exemptions={})
    assert v.category == "boundary"
    assert v.verdict == "FAIL"


def test_boundary_pass():
    """边界场景，|delta_x| <= 0.5 → PASS（baseline <10x 避免触发特殊）。"""
    bp = _mkbaseline(9.5, rs_ns=200.0, sid="boundary_pass")  # <10x + rs<300ns
    mp = _mkmeasured(9.3, rs_ns=200.0)  # delta_x = -0.2
    v = classify(bp, mp, exemptions={})
    assert v.verdict == "PASS"


def test_normal_warn():
    """普通场景，|delta_pct| in (10%, 20%] → WARN。"""
    bp = _mkbaseline(20.0, rs_ns=5000.0, sid="normal_warn")
    mp = _mkmeasured(16.5, rs_ns=5000.0)  # delta_pct = -17.5%
    v = classify(bp, mp, exemptions={})
    assert v.category == "normal"
    assert v.verdict == "WARN"


def test_normal_fail():
    """普通场景，|delta_pct| > 20% → FAIL。"""
    bp = _mkbaseline(20.0, rs_ns=5000.0, sid="normal_fail")
    mp = _mkmeasured(14.0, rs_ns=5000.0)  # delta_pct = -30%
    v = classify(bp, mp, exemptions={})
    assert v.verdict == "FAIL"


def test_normal_pass():
    """普通场景，|delta_pct| <= 10% → PASS。"""
    bp = _mkbaseline(20.0, rs_ns=5000.0, sid="normal_pass")
    mp = _mkmeasured(18.5, rs_ns=5000.0)  # delta_pct = -7.5%
    v = classify(bp, mp, exemptions={})
    assert v.verdict == "PASS"


def test_special_fail_hard_constraint():
    """特殊场景：baseline>=10x 且 new<10x → FAIL（即使 delta_x 小）。"""
    bp = _mkbaseline(10.16, rs_ns=3000.0, sid="special_case")  # >= 10x
    mp = _mkmeasured(9.95, rs_ns=3000.0)  # <10x, delta_x=-0.21（边界内）
    v = classify(bp, mp, exemptions={})
    assert v.category == "special"
    assert v.verdict == "FAIL"  # 硬约束 #5


def test_exempted_fail_to_warn():
    """豁免：FAIL → WARN。"""
    bp = _mkbaseline(10.16, rs_ns=3000.0, sid="exempt_target", ctor="Index")
    bp_dir = "parse"
    bp.direction = bp_dir
    mp = _mkmeasured(9.5, rs_ns=3000.0)  # 触发硬约束 FAIL
    key = bp.key
    exemptions = {key: {"reason": "test exempt", "action": "skip_fail_verdict"}}
    v = classify(bp, mp, exemptions)
    assert v.exempted
    assert v.verdict == "WARN"  # 豁免降级
    assert "test exempt" in v.exemption_reason


def test_improved():
    """性能提升：IMPROVED（不阻断）。"""
    bp = _mkbaseline(10.0, rs_ns=1000.0, sid="improved")
    mp = _mkmeasured(12.0, rs_ns=1000.0)  # delta_x = +2.0
    v = classify(bp, mp, exemptions={})
    assert v.verdict == "IMPROVED"
    assert v.delta_x > 0


def test_skipped_no_measurement():
    """无测量值：SKIPPED。"""
    bp = _mkbaseline(10.0, rs_ns=1000.0, sid="skipped")
    v = classify(bp, None, exemptions={})
    assert v.verdict == "SKIPPED"
    assert v.category == "skipped"


def test_new_baseline_missing():
    """baseline speedup 缺失：NEW。"""
    bp = _mkbaseline(None, rs_ns=None, sid="new")
    mp = _mkmeasured(11.0, rs_ns=900.0)
    v = classify(bp, mp, exemptions={})
    assert v.verdict == "NEW"
    assert v.category == "new"
    assert v.baseline_speedup is None


def test_baseline_load_real():
    """加载真实 perf-scenarios.csv（154 数据点）。"""
    points = load_baseline(DEFAULT_BASELINE)
    assert len(points) == 154, f"expected 154 got {len(points)}"
    # 抽样检查
    keys = {p.key for p in points}
    assert "FormatField:B1:parse" in keys
    assert "Index:E01-4.7-verify:parse" in keys


def test_exemptions_load_real():
    """加载真实 known_exemptions.json（21 条目）。"""
    exemptions = load_exemptions(DEFAULT_EXEMPTIONS)
    assert len(exemptions) == 21, f"expected 21 got {len(exemptions)}"
    # 抽样检查
    assert "FormatField:B1:parse" in exemptions
    assert "Index:E01-4.7-verify:parse" in exemptions  # 事项 B
    assert "PrefixedArray:p_err-4.x:build" in exemptions  # 事项 C
    assert "Array:a_err-4.x:parse" in exemptions  # 事项 C


def test_speedup_boundary_in_range_9_to_11():
    """speedup in [9, 11] 触发边界判定（即使 rs_ns 较大）。"""
    bp = _mkbaseline(10.5, rs_ns=10000.0, sid="sp_boundary")  # sp 在 [9,11] 内
    mp = _mkmeasured(9.3, rs_ns=10000.0)  # delta_x = -1.2
    v = classify(bp, mp, exemptions={})
    # 注意：先检查特殊场景——baseline 10.5>=10 + new 9.3<10 → 特殊 FAIL
    assert v.category == "special"
    assert v.verdict == "FAIL"


def test_speedup_boundary_no_special_when_baseline_below_10():
    """baseline 9.5x 在 [9,11] 内但 <10x，不触发硬约束。"""
    bp = _mkbaseline(9.5, rs_ns=10000.0, sid="sp_below_10")  # 在 [9,11] 内但 <10
    mp = _mkmeasured(8.5, rs_ns=10000.0)  # delta_x = -1.0（边界 FAIL 阈值）
    v = classify(bp, mp, exemptions={})
    assert v.category == "boundary"
    # delta_x = -1.0 严格不大于 1.0，所以是 WARN（用 > 不是 >=）
    assert v.verdict == "WARN"


if __name__ == "__main__":
    # 简单 runner：列出所有 test_ 函数并执行
    test_funcs = [(n, globals()[n]) for n in sorted(globals()) if n.startswith("test_")]
    passed = 0
    failed = 0
    for name, fn in test_funcs:
        try:
            fn()
            print(f"  PASS  {name}")
            passed += 1
        except AssertionError as e:
            print(f"  FAIL  {name}: {e}")
            failed += 1
        except Exception as e:
            print(f"  ERROR {name}: {type(e).__name__}: {e}")
            failed += 1
    print(f"\n{passed} passed, {failed} failed, {len(test_funcs)} total")
    sys.exit(0 if failed == 0 else 1)
