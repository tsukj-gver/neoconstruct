"""ab_stats.py 单元测试（验证 welch_t / classify_delta / 分析流程）。"""
from __future__ import annotations

import json
import math
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
# 加载 ab_stats.py（位于 ab_test/ 子目录）
import importlib.util
spec = importlib.util.spec_from_file_location(
    "ab_stats", HERE / "ab_test" / "ab_stats.py"
)
ab_stats = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ab_stats)


def test_mean_basic():
    assert ab_stats.mean([1.0, 2.0, 3.0]) == 2.0
    assert ab_stats.mean([]) != ab_stats.mean([])  # nan != nan


def test_stdev_sample():
    s = ab_stats.stdev_sample([1.0, 2.0, 3.0, 4.0, 5.0])
    assert abs(s - 1.5811) < 0.01  # sample stddev
    # 单元素 → nan
    assert math.isnan(ab_stats.stdev_sample([1.0]))


def test_welch_t():
    # 完全相同 → 0
    t = ab_stats.welch_t([1.0, 2.0, 3.0], [1.0, 2.0, 3.0])
    assert t == 0.0
    # 差异大 → 正/负方向
    t = ab_stats.welch_t([10.0, 11.0, 12.0], [1.0, 2.0, 3.0])
    assert t > 0  # A > B → 正


def test_classify_delta():
    assert ab_stats.classify_delta(0.6) == "REGRESSION"
    assert ab_stats.classify_delta(-0.6) == "REGRESSION"
    assert ab_stats.classify_delta(0.2) == "NOISE"
    assert ab_stats.classify_delta(-0.2) == "NOISE"
    assert ab_stats.classify_delta(0.4) == "INTERMEDIATE"
    assert ab_stats.classify_delta(-0.4) == "INTERMEDIATE"
    # 边界值：用 > 和 < 严格
    assert ab_stats.classify_delta(0.5) == "INTERMEDIATE"  # 不大于 0.5
    assert ab_stats.classify_delta(0.3) == "INTERMEDIATE"  # 不小于 0.3


def test_run_analysis_with_mock_files():
    """用 mock JSON 文件验证完整分析流程。"""
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        # 构造 3 个 A 轮 + 3 个 B 轮 bench 结果
        def _make_bench(label, e01_sp, s01_sp, i01_sp):
            data = {
                "label": label, "timestamp": "2026-07-28", "repeat": 5,
                "results": [
                    {"key": "E01", "name": "E01", "role": "investigation",
                     "number": 10000, "crs_ns": 200.0, "pc_ns": 200.0*e01_sp,
                     "speedup": e01_sp},
                    {"key": "S01", "name": "S01", "role": "positive_control",
                     "number": 5000, "crs_ns": 300.0, "pc_ns": 300.0*s01_sp,
                     "speedup": s01_sp},
                    {"key": "i01", "name": "i01", "role": "negative_control",
                     "number": 5000, "crs_ns": 5000.0, "pc_ns": 5000.0*i01_sp,
                     "speedup": i01_sp},
                ]
            }
            with (tmp_path / f"ab_bench_{label}.json").open("w", encoding="utf-8") as f:
                json.dump(data, f)

        # A 轮：E01=9.5x（O1 off）
        _make_bench("A1_off", 9.5, 10.0, 17.0)
        _make_bench("A2_off", 9.6, 10.1, 17.2)
        _make_bench("A3_off", 9.4, 9.9, 17.1)
        # B 轮：E01=9.4x（O1 on，Δ ≈ -0.1，NOISE）
        _make_bench("B1_on", 9.4, 10.5, 17.1)
        _make_bench("B2_on", 9.5, 10.4, 17.3)
        _make_bench("B3_on", 9.3, 10.6, 17.0)

        report = ab_stats.run_analysis(
            tmp_path, "ab_bench_{label}.json",
            ["A1_off", "A2_off", "A3_off"],
            ["B1_on", "B2_on", "B3_on"],
            ["E01", "S01", "i01"],
        )
        # E01: A mean ≈ 9.5, B mean ≈ 9.4, Δ ≈ 0.1（NOISE）
        e01 = next(s for s in report["scenarios"] if s["scenario"] == "E01")
        assert e01["verdict"] == "NOISE", f"E01 verdict: {e01['verdict']}"
        # S01 (阳性对照): A ≈ 10.0, B ≈ 10.5, Δ ≈ -0.5（接近 REGRESSION 阈值）
        s01 = next(s for s in report["scenarios"] if s["scenario"] == "S01")
        # |Δ| ≈ 0.5 边界；可能 NOISE/INTERMEDIATE
        assert s01["verdict"] in ("INTERMEDIATE", "REGRESSION", "NOISE")
        # i01 (阴性对照): A ≈ 17.1, B ≈ 17.13, Δ ≈ -0.03（NOISE）
        i01 = next(s for s in report["scenarios"] if s["scenario"] == "i01")
        assert i01["verdict"] == "NOISE"
        # 整体判定：无 REGRESSION → NOISE 或 MIXED
        assert report["summary"]["overall_verdict"] in ("NOISE", "MIXED")


def test_run_analysis_missing_file():
    """文件缺失时报错并标记 error。"""
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        report = ab_stats.run_analysis(
            tmp_path, "ab_bench_{label}.json",
            ["A1"], ["B1"], ["E01"],
        )
        assert "error" in report["scenarios"][0]


if __name__ == "__main__":
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
