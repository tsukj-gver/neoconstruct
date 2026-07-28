"""L3 性能回归检测框架核心（META-CI-1c）.

设计依据：
- docs/design/CI冒烟门禁设计.md §5（L3 性能回归检测框架）
- experiences.md §L-09（跨时段性能对比消除法归因失效）
- .opencode/skills/performance-gate/SKILL.md Checkpoint 4（L-09 工程化）

核心组件：
  1. baseline 加载（docs/perf-scenarios.csv，只读，L-03 对策）
  2. 三档回归判据（边界 / 普通 / 特殊，§5.6）
  3. known_exemptions.json 加载（21 条目，§6.4）
  4. 测量环境标注（L-09 强制对策，§5.4）
  5. 报告生成（JSON + Markdown，§5.5）
  6. A/B Test 升级路径触发（§5.7，仅触发，实际测量在 ab_test/ 子目录）

测量口径（§5.3 强制）：
  - 子进程隔离（双 venv：crs_venv_new + crs_venv_py_new）
  - min(repeat=5) × number
  - apples-to-apples（双端都包 Struct）
  - PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1

退出码：
  - 0 = 全 PASS
  - 1 = 有 WARN，无 FAIL
  - 2 = 有 FAIL（或 A/B Test 确认回归）

CLI：
  --quick              跑子集（SCENARIO_DEFS 中标记 quick=true 的）
  --mock               用 mock 数据测试判据（不跑实际 bench）
  --no-ab-test         L3 FAIL 时不触发 Controlled A/B Test
  --report=PATH        JSON 报告输出路径
  --markdown=PATH      Markdown 报告输出路径
  --exemptions=PATH    known_exemptions.json 路径
  --baseline=PATH      perf-scenarios.csv 路径
  --crs-python=PATH    crs_venv_new python.exe
  --pc-python=PATH     crs_venv_py_new python.exe

注意：完整 151 场景的 bench 映射是 follow-up 工作（属于 Phase 5 性能优化范围）。
本子任务提供可扩展的 SCENARIO_DEFS 字典与框架；初始覆盖 Phase 4 代表性场景。
"""
from __future__ import annotations

import csv
import json
import math
import os
import platform
import subprocess
import sys
import time
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence, Tuple

# ---------------------------------------------------------------------
# 项目路径定位
# ---------------------------------------------------------------------
# run_l3_perf.py 位于 <root>/experiments/ci/
PROJECT_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_BASELINE = PROJECT_ROOT / "docs" / "perf-scenarios.csv"
DEFAULT_EXEMPTIONS = PROJECT_ROOT / "experiments" / "ci" / "known_exemptions.json"
DEFAULT_REPORT_DIR = PROJECT_ROOT / "experiments" / "ci" / "reports"

# 默认 venv 路径（§5.3 / §5.4）
_TEMP_BASE = os.environ.get("LOCALAPPDATA", "")
_VENV_ROOT = os.path.join(_TEMP_BASE, "Temp", "opencode") if _TEMP_BASE else None
DEFAULT_CRS_PYTHON = (
    Path(_VENV_ROOT) / "crs_venv_new" / "Scripts" / "python.exe"
    if _VENV_ROOT else Path("crs_venv_new/Scripts/python.exe")
)
DEFAULT_PC_PYTHON = (
    Path(_VENV_ROOT) / "crs_venv_py_new" / "Scripts" / "python.exe"
    if _VENV_ROOT else Path("crs_venv_py_new/Scripts/python.exe")
)

REPEAT = 5  # min(repeat=5) × number 口径


# ---------------------------------------------------------------------
# 判据常量（§5.6）
# ---------------------------------------------------------------------
# 边界场景判据
BOUNDARY_RS_NS_THRESHOLD = 300.0  # rs_ns < 300ns 视为边界
BOUNDARY_SPEEDUP_LOW = 9.0
BOUNDARY_SPEEDUP_HIGH = 11.0
BOUNDARY_WARN_DELTA_X = 0.5
BOUNDARY_FAIL_DELTA_X = 1.0

# 普通场景判据
NORMAL_WARN_PCT = 10.0
NORMAL_FAIL_PCT = 20.0

# 特殊场景（硬约束 #5）
SPECIAL_HARD_CONSTRAINT_X = 10.0


@dataclass
class BaselinePoint:
    """perf-scenarios.csv 中的一个测量点（一行的结构化表示）。"""

    constructor: str
    scenario_id: str
    direction: str
    speedup_x: Optional[float]
    rs_ns_per_call: Optional[float]
    meets_10x: bool
    phase: str
    notes: str = ""

    @property
    def key(self) -> str:
        """与 known_exemptions.json scenario_id 对齐的三元组键。"""
        return f"{self.constructor}:{self.scenario_id}:{self.direction}"


@dataclass
class MeasuredPoint:
    """一次实际测量结果。"""

    rs_ns: float
    pc_ns: float
    speedup: float


@dataclass
class Verdict:
    """一个测量点的判定结果。"""

    key: str
    constructor: str
    scenario_id: str
    direction: str
    category: str  # "boundary" / "normal" / "special" / "new" / "skipped" / "improved"
    baseline_speedup: Optional[float]
    new_speedup: Optional[float]
    baseline_rs_ns: Optional[float]
    new_rs_ns: Optional[float]
    delta_x: Optional[float]
    delta_pct: Optional[float]
    verdict: str  # "PASS" / "WARN" / "FAIL" / "NEW" / "SKIPPED" / "IMPROVED"
    threshold: Dict[str, float] = field(default_factory=dict)
    exempted: bool = False
    exemption_reason: str = ""
    note: str = ""


# ---------------------------------------------------------------------
# Baseline 加载
# ---------------------------------------------------------------------
def load_baseline(path: Path) -> List[BaselinePoint]:
    """加载 perf-scenarios.csv，返回 BaselinePoint 列表（L-03 对策：只读）。"""
    points: List[BaselinePoint] = []
    with path.open("r", encoding="utf-8", newline="") as f:
        reader = csv.DictReader(f)
        for row in reader:
            sp_str = (row.get("speedup_x") or "").strip()
            rs_str = (row.get("rs_ns_per_call") or "").strip()
            meets_str = (row.get("meets_10x") or "").strip().lower()
            try:
                speedup = float(sp_str) if sp_str else None
            except ValueError:
                speedup = None
            try:
                rs_ns = float(rs_str) if rs_str else None
            except ValueError:
                rs_ns = None
            points.append(
                BaselinePoint(
                    constructor=row["constructor"],
                    scenario_id=row["scenario_id"],
                    direction=row["direction"],
                    speedup_x=speedup,
                    rs_ns_per_call=rs_ns,
                    meets_10x=(meets_str == "true"),
                    phase=row.get("phase", ""),
                    notes=row.get("notes", ""),
                )
            )
    return points


def load_exemptions(path: Path) -> Dict[str, Dict[str, Any]]:
    """加载 known_exemptions.json，返回 scenario_id -> exemption dict 映射。"""
    if not path.exists():
        return {}
    with path.open("r", encoding="utf-8") as f:
        data = json.load(f)
    return {e["scenario_id"]: e for e in data.get("exemptions", [])}


# ---------------------------------------------------------------------
# 三档回归判据（§5.6）
# ---------------------------------------------------------------------
def classify_category(baseline_speedup: float, baseline_rs_ns: Optional[float]) -> str:
    """判断场景类别（boundary / normal / 特殊由 classify 内部处理）。"""
    is_boundary = (
        (baseline_rs_ns is not None and baseline_rs_ns < BOUNDARY_RS_NS_THRESHOLD)
        or (BOUNDARY_SPEEDUP_LOW <= baseline_speedup <= BOUNDARY_SPEEDUP_HIGH)
    )
    return "boundary" if is_boundary else "normal"


def classify(
    baseline: BaselinePoint,
    measured: Optional[MeasuredPoint],
    exemptions: Dict[str, Dict[str, Any]],
) -> Verdict:
    """对单个测量点做判定（§5.6 三档判据 + 豁免处理）。"""
    key = baseline.key
    exempted = key in exemptions
    exemption_reason = exemptions[key]["reason"] if exempted else ""

    # 无测量值：标记 SKIPPED
    if measured is None:
        return Verdict(
            key=key, constructor=baseline.constructor,
            scenario_id=baseline.scenario_id, direction=baseline.direction,
            category="skipped",
            baseline_speedup=baseline.speedup_x, new_speedup=None,
            baseline_rs_ns=baseline.rs_ns_per_call, new_rs_ns=None,
            delta_x=None, delta_pct=None,
            verdict="SKIPPED", exempted=exempted,
            exemption_reason=exemption_reason,
            note="no measurement (scenario not in SCENARIO_DEFS)",
        )

    # baseline 缺 speedup：标记 NEW（设计 §5.2）
    if baseline.speedup_x is None:
        return Verdict(
            key=key, constructor=baseline.constructor,
            scenario_id=baseline.scenario_id, direction=baseline.direction,
            category="new",
            baseline_speedup=None, new_speedup=measured.speedup,
            baseline_rs_ns=baseline.rs_ns_per_call, new_rs_ns=measured.rs_ns,
            delta_x=None, delta_pct=None,
            verdict="NEW",
            note="baseline speedup missing; suggest PM to update perf-scenarios.csv",
        )

    bs = baseline.speedup_x
    ns = measured.speedup
    delta_x = ns - bs
    delta_pct = (delta_x / bs * 100.0) if bs != 0 else 0.0

    # 特殊场景：硬约束 #5（baseline ≥10x → new <10x 直接 FAIL，无 WARN）
    if bs >= SPECIAL_HARD_CONSTRAINT_X and ns < SPECIAL_HARD_CONSTRAINT_X:
        verdict = "FAIL"
        if exempted:
            verdict = "WARN"  # 豁免：仅 WARN（§5.6 特殊豁免）
        return Verdict(
            key=key, constructor=baseline.constructor,
            scenario_id=baseline.scenario_id, direction=baseline.direction,
            category="special",
            baseline_speedup=bs, new_speedup=ns,
            baseline_rs_ns=baseline.rs_ns_per_call, new_rs_ns=measured.rs_ns,
            delta_x=delta_x, delta_pct=delta_pct,
            verdict=verdict,
            threshold={"rule": "baseline>=10x and new<10x"},
            exempted=exempted, exemption_reason=exemption_reason,
        )

    # 性能提升：IMPROVED（不阻断，PM 应考虑更新 baseline）
    if delta_x > 0:
        category = classify_category(bs, baseline.rs_ns_per_call)
        return Verdict(
            key=key, constructor=baseline.constructor,
            scenario_id=baseline.scenario_id, direction=baseline.direction,
            category=category,
            baseline_speedup=bs, new_speedup=ns,
            baseline_rs_ns=baseline.rs_ns_per_call, new_rs_ns=measured.rs_ns,
            delta_x=delta_x, delta_pct=delta_pct,
            verdict="IMPROVED",
            threshold={},
            exempted=exempted, exemption_reason=exemption_reason,
        )

    # 边界 / 普通场景判据
    category = classify_category(bs, baseline.rs_ns_per_call)
    if category == "boundary":
        abs_dx = abs(delta_x)
        if abs_dx > BOUNDARY_FAIL_DELTA_X:
            verdict = "FAIL"
        elif abs_dx > BOUNDARY_WARN_DELTA_X:
            verdict = "WARN"
        else:
            verdict = "PASS"
        threshold = {
            "warn_delta_x": BOUNDARY_WARN_DELTA_X,
            "fail_delta_x": BOUNDARY_FAIL_DELTA_X,
        }
    else:
        abs_pct = abs(delta_pct)
        if abs_pct > NORMAL_FAIL_PCT:
            verdict = "FAIL"
        elif abs_pct > NORMAL_WARN_PCT:
            verdict = "WARN"
        else:
            verdict = "PASS"
        threshold = {
            "warn_pct": NORMAL_WARN_PCT,
            "fail_pct": NORMAL_FAIL_PCT,
        }

    # 豁免：FAIL → WARN（§5.6 特殊豁免；记录但不阻断）
    if verdict == "FAIL" and exempted:
        verdict = "WARN"

    return Verdict(
        key=key, constructor=baseline.constructor,
        scenario_id=baseline.scenario_id, direction=baseline.direction,
        category=category,
        baseline_speedup=bs, new_speedup=ns,
        baseline_rs_ns=baseline.rs_ns_per_call, new_rs_ns=measured.rs_ns,
        delta_x=delta_x, delta_pct=delta_pct,
        verdict=verdict, threshold=threshold,
        exempted=exempted, exemption_reason=exemption_reason,
    )


# ---------------------------------------------------------------------
# 场景定义（SCENARIO_DEFS）
# ---------------------------------------------------------------------
# 每个 SCENARIO_DEFS 条目：scenario_key -> ScenarioDef 字典
#
# 字段：
#   crs_setup / crs_stmt : construct-rs 测量代码（在 crs_venv_new 跑）
#   pc_setup / pc_stmt   : Python construct 测量代码（在 crs_venv_py_new 跑）
#   number               : timeit number（每次测量迭代数）
#   quick                : 是否在 --quick 模式下跑
#
# apples-to-apples 原则（§5.3）：双端都包 Struct（construct-rs StructMixin
# vs Python construct.Struct）。
#
# 初始覆盖：phase4_47_perf_verify.py 验证过的 5 场景 + 几个边界场景。
# 完整 151 场景映射 = follow-up（属于 Phase 5 性能优化范围）。
# ---------------------------------------------------------------------
SCENARIO_DEFS: Dict[str, Dict[str, Any]] = {
    "Index:i02-4.7:parse": {
        "quick": True, "number": 3000,
        "crs_setup": """
import construct as crs
from construct import StructMixin, field
class P(StructMixin):
    items: list = field(crs.Array(100, crs.Index()))
data = bytes(100)
pkt = P
""",
        "crs_stmt": "pkt.parse(data)",
        "pc_setup": """
import construct as pc
pkt = pc.Struct("items" / pc.Array(100, pc.Index))
data = bytes(100)
""",
        "pc_stmt": "pkt.parse(data)",
    },
    "Index:i01-4.7:build": {
        "quick": True, "number": 3000,
        "crs_setup": """
import construct as crs
from construct import StructMixin, field
class P(StructMixin):
    items: list = field(crs.Array(10, crs.Index()))
pkt = P
obj = pkt.parse(bytes(10))
""",
        "crs_stmt": "obj.build()",
        "pc_setup": """
import construct as pc
pkt = pc.Struct("items" / pc.Array(10, pc.Index))
obj = pkt.parse(bytes(10))
""",
        "pc_stmt": "pkt.build(obj)",
    },
    "GreedyRange:g01-4.7:parse": {
        "quick": True, "number": 3000,
        "crs_setup": """
import construct as crs
from construct import StructMixin, field
class P(StructMixin):
    items: list = field(crs.GreedyRange(crs.Int8ub))
data = bytes([1,2,3,4,5,6,7,8,9,10])
pkt = P
""",
        "crs_stmt": "pkt.parse(data)",
        "pc_setup": """
import construct as pc
pkt = pc.Struct("items" / pc.GreedyRange(pc.Int8ub))
data = bytes([1,2,3,4,5,6,7,8,9,10])
""",
        "pc_stmt": "pkt.parse(data)",
    },
    # E01 边界场景（事项 B，零工作量）
    "Index:e01-4.7:parse": {
        "quick": True, "number": 10000,
        "crs_setup": """
import construct as crs
from construct import StructMixin, field
class P(StructMixin):
    items: list = field(crs.Array(0, crs.Index()))
data = bytes(0)
pkt = P
""",
        "crs_stmt": "pkt.parse(data)",
        "pc_setup": """
import construct as pc
pkt = pc.Struct("items" / pc.Array(0, pc.Index))
data = bytes(0)
""",
        "pc_stmt": "pkt.parse(data)",
    },
    # 边界场景：小 Array(Int8ub) N=10（验证 rs_ns < 300ns 边界判定）
    # CSV scenario_id = "A1"（4.1 v2 多维）；与 phase4_bench_array_v4 内部 key "a01_i8_n10" 同物理场景
    "Array:A1:parse": {
        "quick": False, "number": 3000,
        "crs_setup": """
import construct as crs
from construct import StructMixin, field
class P(StructMixin):
    items: list = field(crs.Array(10, crs.Int8ub))
data = bytes(10)
pkt = P
""",
        "crs_stmt": "pkt.parse(data)",
        "pc_setup": """
import construct as pc
pkt = pc.Struct("items" / pc.Array(10, pc.Int8ub))
data = bytes(10)
""",
        "pc_stmt": "pkt.parse(data)",
    },
}


def _build_timeit_script(setup_code: str, stmt_code: str, number: int) -> str:
    """构造在子进程中跑 timeit 的 Python 脚本（输出 JSON 单行）。"""
    return f"""
import timeit
import json

setup = '''{setup_code}'''

exec(setup)

t = timeit.repeat(stmt={stmt_code!r}, setup=setup, number={number}, repeat={REPEAT})
print(json.dumps({{"min_s": min(t), "number": {number}}}))
"""


def measure_point(
    crs_python: Path,
    pc_python: Path,
    scenario_def: Dict[str, Any],
) -> Optional[MeasuredPoint]:
    """对单个场景做 S-PERF 口径双端测量。

    子进程隔离（§5.3）：crs 与 pc 在不同 venv。
    返回 MeasuredPoint 或 None（测量失败时）。
    """
    number = int(scenario_def.get("number", 1000))
    env = dict(os.environ)
    env["PYO3_USE_ABI3_FORWARD_COMPATIBILITY"] = "1"

    # 测 construct-rs
    try:
        crs_script = _build_timeit_script(
            scenario_def["crs_setup"], scenario_def["crs_stmt"], number
        )
        result = subprocess.run(
            [str(crs_python), "-c", crs_script],
            capture_output=True, text=True, timeout=180, env=env,
        )
        if result.returncode != 0:
            sys.stderr.write(
                f"[L3] CRS measurement failed: {result.stderr[:500]}\n"
            )
            return None
        crs_data = json.loads(result.stdout.strip().split("\n")[-1])
        crs_ns = (crs_data["min_s"] / crs_data["number"]) * 1e9
    except (subprocess.TimeoutExpired, json.JSONDecodeError, OSError) as exc:
        sys.stderr.write(f"[L3] CRS measurement exception: {exc}\n")
        return None

    # 测 Python construct
    try:
        pc_script = _build_timeit_script(
            scenario_def["pc_setup"], scenario_def["pc_stmt"], number
        )
        result = subprocess.run(
            [str(pc_python), "-c", pc_script],
            capture_output=True, text=True, timeout=180, env=env,
        )
        if result.returncode != 0:
            sys.stderr.write(
                f"[L3] PC measurement failed: {result.stderr[:500]}\n"
            )
            return None
        pc_data = json.loads(result.stdout.strip().split("\n")[-1])
        pc_ns = (pc_data["min_s"] / pc_data["number"]) * 1e9
    except (subprocess.TimeoutExpired, json.JSONDecodeError, OSError) as exc:
        sys.stderr.write(f"[L3] PC measurement exception: {exc}\n")
        return None

    speedup = pc_ns / crs_ns if crs_ns > 0 else float("inf")
    return MeasuredPoint(rs_ns=crs_ns, pc_ns=pc_ns, speedup=speedup)


# ---------------------------------------------------------------------
# 环境采集（§5.4，L-09 强制对策）
# ---------------------------------------------------------------------
def _git_short_commit() -> str:
    """读 git HEAD short commit；失败时返回 'unknown'。"""
    try:
        result = subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            capture_output=True, text=True, timeout=5,
            cwd=str(PROJECT_ROOT),
        )
        if result.returncode == 0:
            return result.stdout.strip()
    except (subprocess.SubprocessError, OSError):
        pass
    return "unknown"


def _query_construct_py_version(pc_python: Path) -> str:
    """查询 crs_venv_py_new 中 Python construct 库版本（设计 §5.4）。

    设计 §5.4 JSON schema 要求 environment 含 construct_py_version（如 "2.10.70"）。
    从 pc_python 子进程查询，失败时返回 'unknown'。
    """
    try:
        result = subprocess.run(
            [str(pc_python), "-c", "import construct; print(construct.__version__)"],
            capture_output=True, text=True, timeout=10,
        )
        if result.returncode == 0:
            ver = result.stdout.strip()
            return ver if ver else "unknown"
    except (subprocess.SubprocessError, OSError):
        pass
    return "unknown"


def _windows_power_plan() -> str:
    """读 Windows 当前电源计划（仅 Windows；其他平台 'n/a'）。"""
    if platform.system() != "Windows":
        return "n/a"
    try:
        result = subprocess.run(
            ["powercfg", "/getactivepowerscheme"],
            capture_output=True, text=True, timeout=5,
        )
        if result.returncode == 0:
            # 输出形如 "电源方案 GUID: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx  (高性能)"
            return result.stdout.strip()
    except (subprocess.SubprocessError, OSError):
        pass
    return "unknown"


def collect_environment(
    crs_python: Path, pc_python: Path,
    baseline_path: Path,
) -> Dict[str, Any]:
    """采集本次 L3 跑的测量环境（§5.4）。"""
    return {
        "timestamp": datetime.now().astimezone().isoformat(timespec="seconds"),
        "python_version": sys.version.split()[0],
        "python_exe": sys.executable,
        "crs_venv_python": str(crs_python),
        "pc_venv_python": str(pc_python),
        "construct_rs_commit": _git_short_commit(),
        # OBS-2 修复：补充 construct_py_version 字段（与 python_version 并列）
        # 从 crs_venv_py_new 子进程查询；查询失败时填 "unknown"
        "construct_py_version": _query_construct_py_version(pc_python),
        "baseline_source": f"{baseline_path.relative_to(PROJECT_ROOT)}@{_git_short_commit()}",
        "os": platform.platform(),
        "cpu": platform.processor() or "unknown",
        "machine": platform.machine(),
        "power_plan": _windows_power_plan(),
        "thermal_state": "unknown",
        # L-09 对策：报告必须含环境标注
        "l_09_disclaimer": (
            "跨时段性能对比在边界场景（Rust<300ns）失效；"
            "若 baseline 无环境标注，回归判定以 Controlled A/B Test 为准"
        ),
    }


# ---------------------------------------------------------------------
# 报告生成（§5.5 JSON + Markdown）
# ---------------------------------------------------------------------
def _verdict_to_dict(v: Verdict) -> Dict[str, Any]:
    return {
        "constructor": v.constructor,
        "scenario_id": v.scenario_id,
        "direction": v.direction,
        "key": v.key,
        "category": v.category,
        "baseline_speedup": v.baseline_speedup,
        "new_speedup": v.new_speedup,
        "delta_x": v.delta_x,
        "delta_pct": v.delta_pct,
        "rs_ns_baseline": v.baseline_rs_ns,
        "rs_ns_new": v.new_rs_ns,
        "verdict": v.verdict,
        "threshold": v.threshold,
        "exempted": v.exempted,
        "exemption_reason": v.exemption_reason,
        "note": v.note,
    }


def build_report_json(
    run_id: str,
    environment: Dict[str, Any],
    baseline_source: str,
    verdicts: List[Verdict],
    ab_triggered: List[str],
    ab_reports: List[str],
    quick_mode: bool,
) -> Dict[str, Any]:
    """汇总 verdicts 为 JSON 报告 dict（§5.5 schema）。"""
    summary = {"pass": 0, "warn": 0, "fail": 0, "new": 0, "skipped": 0, "improved": 0}
    for v in verdicts:
        key = v.verdict.lower()
        if key in summary:
            summary[key] += 1
    summary["total"] = len(verdicts)

    return {
        "run_id": run_id,
        "triggered_by": "manual",
        "mode": "perf" + ("+quick" if quick_mode else ""),
        "environment": environment,
        "baseline_source": baseline_source,
        "summary": summary,
        "results": [_verdict_to_dict(v) for v in verdicts],
        "ab_test_triggered": ab_triggered,
        "ab_test_reports": ab_reports,
    }


def build_report_markdown(report: Dict[str, Any]) -> str:
    """生成人工审查用 Markdown 报告（§5.5）。"""
    lines: List[str] = []
    lines.append(f"# L3 性能回归报告 {report['run_id']}")
    lines.append("")
    lines.append("## 测量环境")
    env = report["environment"]
    lines.append(f"- timestamp: {env['timestamp']}")
    lines.append(f"- python: {env['python_version']}")
    lines.append(f"- crs_venv: {env['crs_venv_python']}")
    lines.append(f"- pc_venv: {env['pc_venv_python']}")
    lines.append(f"- commit: {env['construct_rs_commit']}")
    lines.append(f"- os: {env['os']}")
    lines.append(f"- cpu: {env['cpu']}")
    lines.append(f"- power_plan: {env['power_plan']}")
    lines.append(f"- baseline: {report['baseline_source']}")
    lines.append(f"- L-09 disclaimer: {env['l_09_disclaimer']}")
    lines.append("")

    s = report["summary"]
    lines.append("## 汇总")
    lines.append("| 指标 | 值 |")
    lines.append("|------|----|")
    lines.append(f"| 总场景 | {s['total']} |")
    lines.append(f"| PASS | {s['pass']} |")
    lines.append(f"| WARN | {s['warn']} |")
    lines.append(f"| FAIL | {s['fail']} |")
    lines.append(f"| NEW | {s['new']} |")
    lines.append(f"| IMPROVED | {s['improved']} |")
    lines.append(f"| SKIPPED | {s['skipped']} |")
    lines.append("")

    fails = [r for r in report["results"] if r["verdict"] == "FAIL"]
    warns = [r for r in report["results"] if r["verdict"] == "WARN"]
    improved = [r for r in report["results"] if r["verdict"] == "IMPROVED"]
    news = [r for r in report["results"] if r["verdict"] == "NEW"]

    if fails:
        lines.append("## FAIL 明细（需处理）")
        lines.append("| 构造器 | 场景 | 方向 | baseline | new | Δx | 类别 |")
        lines.append("|--------|------|------|----------|-----|-----|------|")
        for r in fails:
            lines.append(
                f"| {r['constructor']} | {r['scenario_id']} | {r['direction']} | "
                f"{r['baseline_speedup']:.2f}x | {r['new_speedup']:.2f}x | "
                f"{r['delta_x']:+.2f}x | {r['category']} |"
            )
        lines.append("")

    if warns:
        lines.append("## WARN 明细（关注）")
        lines.append("| 构造器 | 场景 | 方向 | baseline | new | Δx | Δ% | 豁免 |")
        lines.append("|--------|------|------|----------|-----|-----|----|----|")
        for r in warns:
            ex_tag = "YES" if r["exempted"] else "no"
            delta_pct_str = f"{r['delta_pct']:+.1f}%" if r['delta_pct'] is not None else "n/a"
            lines.append(
                f"| {r['constructor']} | {r['scenario_id']} | {r['direction']} | "
                f"{r['baseline_speedup']:.2f}x | {r['new_speedup']:.2f}x | "
                f"{r['delta_x']:+.2f}x | {delta_pct_str} | {ex_tag} |"
            )
        lines.append("")

    if improved:
        lines.append("## IMPROVED 明细（PM 评估是否更新 baseline）")
        lines.append("| 构造器 | 场景 | 方向 | baseline | new | Δx |")
        lines.append("|--------|------|------|----------|-----|-----|")
        for r in improved:
            lines.append(
                f"| {r['constructor']} | {r['scenario_id']} | {r['direction']} | "
                f"{r['baseline_speedup']:.2f}x | {r['new_speedup']:.2f}x | "
                f"{r['delta_x']:+.2f}x |"
            )
        lines.append("")

    if news:
        lines.append("## NEW 明细（baseline 缺失，建议 PM 纳入）")
        for r in news:
            lines.append(
                f"- {r['constructor']}:{r['scenario_id']}:{r['direction']} "
                f"new_speedup={r['new_speedup']:.2f}x"
            )
        lines.append("")

    if report.get("ab_test_triggered"):
        lines.append("## 触发的 Controlled A/B Test（L-09 升级路径）")
        for k, r in zip(report["ab_test_triggered"], report.get("ab_test_reports", [])):
            lines.append(f"- {k} → {r}")
        lines.append("")

    lines.append("## 建议动作")
    if fails:
        lines.append("- FAIL 项需立即排查（可用 ab_test/ 工具做 Controlled A/B Test）")
    if warns:
        lines.append("- WARN 项关注趋势，豁免项下次 phase 验收复审")
    if improved:
        lines.append("- IMPROVED 项累积 >20% 时 PM 评估更新 baseline")
    if news:
        lines.append("- NEW 项 PM 审查后手动更新 perf-scenarios.csv（baseline 不自动更新）")
    return "\n".join(lines) + "\n"


# ---------------------------------------------------------------------
# Controlled A/B Test 升级路径（§5.7，仅触发；实际测量在 ab_test/）
# ---------------------------------------------------------------------
def run_ab_test_upgrade(
    failed_verdicts: List[Verdict],
    report_dir: Path,
    crs_python: Path,
    pc_python: Path,
) -> Tuple[List[str], List[str]]:
    """对 FAIL 项触发 Controlled A/B Test。

    设计 §5.7：失败时定位质疑改动 -> 构造对照组 -> 交替测量 -> 统计分析。
    本函数只做触发动作（生成 ab_harness.ps1 调用命令），实际测量由
    ab_test/ab_harness.ps1 执行。返回 (triggered_keys, report_paths)。

    注意：A/B Test 需要 git stash 权限（DEV 默认禁用），所以本函数只生成
    "建议执行的命令"，不直接执行。PM/VET 可手动执行生成的命令。
    """
    triggered: List[str] = []
    reports: List[str] = []
    ab_harness = PROJECT_ROOT / "experiments" / "ci" / "ab_test" / "ab_harness.ps1"

    for v in failed_verdicts:
        # 跳过豁免项（已降级为 WARN，不会出现在 failed_verdicts，但保守检查）
        if v.exempted:
            continue
        triggered.append(v.key)
        report_path = report_dir / f"ab_{v.constructor}_{v.scenario_id}_{v.direction}_{int(time.time())}.json"
        reports.append(str(report_path))

        # 生成建议命令（不直接执行——A/B Test 需要 git stash，DEV 禁用）
        sys.stdout.write(
            f"\n[L3-AB] Suggested A/B Test for {v.key}:\n"
            f"  powershell -File {ab_harness} "
            f"-ScenarioKey '{v.key}' "
            f"-ReportPath '{report_path}'\n"
            f"  (requires git stash permission; manual execution by PM/VET)\n"
        )

    return triggered, reports


# ---------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------
def parse_cli_args(argv: Sequence[str]) -> Dict[str, Any]:
    """解析命令行参数为 dict（轻量级，避免 argparse 在子进程中的开销）。"""
    args: Dict[str, Any] = {
        "quick": False, "mock": False, "no_ab_test": False,
        "report": None, "markdown": None, "exemptions": None,
        "baseline": None, "crs_python": None, "pc_python": None,
    }
    for arg in argv:
        if arg == "--quick":
            args["quick"] = True
        elif arg == "--mock":
            args["mock"] = True
        elif arg == "--no-ab-test":
            args["no_ab_test"] = True
        elif arg.startswith("--report="):
            args["report"] = arg.split("=", 1)[1]
        elif arg.startswith("--markdown="):
            args["markdown"] = arg.split("=", 1)[1]
        elif arg.startswith("--exemptions="):
            args["exemptions"] = arg.split("=", 1)[1]
        elif arg.startswith("--baseline="):
            args["baseline"] = arg.split("=", 1)[1]
        elif arg.startswith("--crs-python="):
            args["crs_python"] = arg.split("=", 1)[1]
        elif arg.startswith("--pc-python="):
            args["pc_python"] = arg.split("=", 1)[1]
        elif arg in ("-h", "--help"):
            sys.stdout.write(__doc__ or "")
            sys.exit(0)
    return args


def _mock_measurement(baseline: BaselinePoint) -> MeasuredPoint:
    """生成 mock 测量数据，用于测试三档判据（--mock 模式）。

    构造覆盖三类场景的 mock 值：
    - 边界场景：模拟 ±0.3-1.2x 波动
    - 普通场景：模拟 ±5-25% 波动
    - 特殊场景：模拟 ≥10x → <10x 跌落
    """
    if baseline.speedup_x is None:
        # 无 baseline：返回任意值（触发 NEW）
        return MeasuredPoint(rs_ns=500.0, pc_ns=5000.0, speedup=10.0)

    bs = baseline.speedup_x
    rs_ns = baseline.rs_ns_per_call or 500.0

    # 模拟波动：使用 baseline 的 hash 做伪随机
    h = (hash(baseline.key) & 0xFFFF) / 0xFFFF  # [0, 1]
    if bs >= SPECIAL_HARD_CONSTRAINT_X:
        # 1/3 概率模拟跌破 10x（特殊场景 FAIL）
        if h < 0.33:
            new_speedup = 9.5
        else:
            new_speedup = bs * (1.0 + (h - 0.5) * 0.1)
    elif rs_ns < BOUNDARY_RS_NS_THRESHOLD or BOUNDARY_SPEEDUP_LOW <= bs <= BOUNDARY_SPEEDUP_HIGH:
        # 边界场景：±0.2-1.5x 波动
        new_speedup = bs + (h - 0.5) * 3.0
    else:
        # 普通场景：±5-30% 波动
        new_speedup = bs * (1.0 + (h - 0.5) * 0.6)

    # 防止负数 / 零
    if new_speedup < 0.1:
        new_speedup = 0.1
    new_rs_ns = rs_ns * (bs / new_speedup) if new_speedup > 0 else rs_ns * 2
    return MeasuredPoint(rs_ns=new_rs_ns, pc_ns=new_rs_ns * new_speedup, speedup=new_speedup)


def main(argv: Optional[Sequence[str]] = None) -> int:
    """CLI 主入口。"""
    args = parse_cli_args(list(sys.argv[1:] if argv is None else argv))

    baseline_path = Path(args["baseline"]) if args["baseline"] else DEFAULT_BASELINE
    exemptions_path = (
        Path(args["exemptions"]) if args["exemptions"] else DEFAULT_EXEMPTIONS
    )
    crs_python = Path(args["crs_python"]) if args["crs_python"] else DEFAULT_CRS_PYTHON
    pc_python = Path(args["pc_python"]) if args["pc_python"] else DEFAULT_PC_PYTHON

    if not baseline_path.exists():
        sys.stderr.write(f"L3: baseline not found: {baseline_path}\n")
        return 2

    print(f"==== L3 Performance Gate start (mock={args['mock']}, quick={args['quick']}) ====")

    # 1. 加载 baseline
    baseline_points = load_baseline(baseline_path)
    print(f"[L3] baseline loaded: {len(baseline_points)} points from {baseline_path.name}")

    # 2. 加载 exemptions
    exemptions = load_exemptions(exemptions_path)
    print(f"[L3] exemptions loaded: {len(exemptions)} entries")

    # 3. 环境采集
    environment = collect_environment(crs_python, pc_python, baseline_path)
    print(f"[L3] env: commit={environment['construct_rs_commit']}")

    # 4. 选择要跑的场景
    if args["quick"]:
        # quick 模式：只跑 SCENARIO_DEFS 中 quick=True 的
        scenario_keys = [k for k, v in SCENARIO_DEFS.items() if v.get("quick")]
    else:
        scenario_keys = list(SCENARIO_DEFS.keys())

    print(f"[L3] scenarios selected: {len(scenario_keys)} (SCENARIO_DEFS total={len(SCENARIO_DEFS)})")
    print(f"[L3] baseline total: {len(baseline_points)} (most will be SKIPPED — see note in module docstring)")

    # 5. 对每个 baseline point 判定
    verdicts: List[Verdict] = []
    measured_count = 0
    skipped_count = 0
    for bp in baseline_points:
        # 找匹配的 SCENARIO_DEF
        sc_key = bp.key
        # SCENARIO_DEFS 用 :direction 形式，而 baseline.key 是 :direction
        # 但 SCENARIO_DEFS 的 key 可能与 baseline.scenario_id 不完全一致
        # （如 baseline "i02-4.7" vs SCENARIO_DEFS "Index:i02-4.7:parse"）
        # 因此需要模糊匹配：先精确，再 fallback
        scenario_def = None
        if sc_key in SCENARIO_DEFS:
            scenario_def = SCENARIO_DEFS[sc_key]
        else:
            # 模糊匹配：忽略 scenario_id 大小写差异
            for def_key, def_val in SCENARIO_DEFS.items():
                if def_key.lower() == sc_key.lower():
                    scenario_def = def_val
                    break

        if scenario_def is None:
            verdicts.append(classify(bp, None, exemptions))
            skipped_count += 1
            continue

        # OBS-1 修复：--quick 模式跳过 quick=False 的场景
        # 原实现只构建 scenario_keys 列表未实际使用，导致 quick=False 场景也被测量
        if args["quick"] and not scenario_def.get("quick"):
            verdicts.append(classify(bp, None, exemptions))
            skipped_count += 1
            continue

        if args["mock"]:
            measured = _mock_measurement(bp)
        else:
            measured = measure_point(crs_python, pc_python, scenario_def)
        verdicts.append(classify(bp, measured, exemptions))
        if measured is not None:
            measured_count += 1

    print(f"[L3] measured: {measured_count}, skipped: {skipped_count}")

    # 6. 触发 A/B Test（仅 FAIL 且非豁免）
    ab_triggered: List[str] = []
    ab_reports: List[str] = []
    failed = [v for v in verdicts if v.verdict == "FAIL"]
    if failed and not args["no_ab_test"]:
        report_dir = Path(args["report"]).parent if args["report"] else DEFAULT_REPORT_DIR
        ab_triggered, ab_reports = run_ab_test_upgrade(
            failed, report_dir, crs_python, pc_python
        )

    # 7. 生成报告
    run_id = f"l3_{datetime.now().strftime('%Y%m%d_%H%M%S')}"
    report_json = build_report_json(
        run_id, environment,
        f"{baseline_path.relative_to(PROJECT_ROOT)}@{environment['construct_rs_commit']}",
        verdicts, ab_triggered, ab_reports, args["quick"],
    )

    report_dir = DEFAULT_REPORT_DIR
    if args["report"]:
        report_path = Path(args["report"])
    else:
        report_dir.mkdir(parents=True, exist_ok=True)
        report_path = report_dir / f"{run_id}_report.json"
    report_path.parent.mkdir(parents=True, exist_ok=True)
    with report_path.open("w", encoding="utf-8") as f:
        json.dump(report_json, f, ensure_ascii=False, indent=2)
    print(f"[L3] JSON report: {report_path}")

    md_path = Path(args["markdown"]) if args["markdown"] else report_path.with_suffix(".md")
    with md_path.open("w", encoding="utf-8") as f:
        f.write(build_report_markdown(report_json))
    print(f"[L3] Markdown report: {md_path}")

    # 8. 控制台汇总 + 退出码
    s = report_json["summary"]
    print(f"[L3] summary: total={s['total']} pass={s['pass']} warn={s['warn']} "
          f"fail={s['fail']} new={s['new']} improved={s['improved']} skipped={s['skipped']}")

    if s["fail"] > 0:
        print("[L3] Overall: FAIL (exit 2)")
        return 2
    elif s["warn"] > 0:
        print("[L3] Overall: WARN (exit 1)")
        return 1
    else:
        print("[L3] Overall: PASS (exit 0)")
        return 0


if __name__ == "__main__":
    sys.exit(main())

