"""性能根因验证：分阶段计时（full parse / Rust _parse_raw / 实例构造成本）。

目的
----
「Rust 内构造实例」消除大扁平结构（B3=50字段、B4=100字段）
``cls(**dict)`` kwargs unpacking 的 O(N²) 瓶颈，本脚本分阶段计时验证其性能。

测量的阶段
----------
对于 ``Flat.parse(data)``，当前实现为：

    return schema._parse_raw(data)   # Rust 内完整构造实例并返回

因此分别测量：

1. **Full parse**：``Flat.parse(data)`` —— Rust 内构造 + FFI 返回实例
2. **Rust _parse_raw**：``schema._parse_raw(data)`` —— 仅 Rust 内核（实例已在 Rust 内构造）
3. **Python construct 基线**：``pc.Struct(...).parse(data)``

注意：``_parse_raw`` 直接返回实例（不是 dict），无 ``Flat(**raw_fields)``
步骤。Full parse ≈ Rust _parse_raw。

隔离策略
--------
两个 ``construct`` 同名包无法在同一进程共存，因此用**子进程隔离**：
- 子进程 A：sys.path 注入 construct-rs/python，测各阶段
- 子进程 B：默认 sys.path（site-packages 优先），测 Python 原版

用法::

    python bench/bench_breakdown.py

"""

from __future__ import annotations

import json
import os
import platform
import statistics
import subprocess
import sys
import textwrap
import time
from pathlib import Path

# timeit 参数
NUMBER = 50_000
REPEAT = 5

# construct-rs 的 python/ 包目录
_CRS_PYTHON_DIR = str(
    Path(__file__).resolve().parent.parent / "python"
)

# 用例：(case 名, 字段数)
CASES = [
    ("B1", 3),
    ("B2", 10),
    ("B3", 50),
    ("B4", 100),
]


# ---------------------------------------------------------------------------
# 子进程脚本：测量 construct-rs 多阶段
# ---------------------------------------------------------------------------

_RS_SCRIPT = textwrap.dedent(
    """
    import json
    import statistics
    import sys
    import timeit

    N = {n}
    NUMBER = {number}
    REPEAT = {repeat}

    # ---- sys.path 操纵：construct-rs 优先 ----
    sys.path.insert(0, {crs_python_dir!r})
    for k in list(sys.modules):
        if k == 'construct' or k.startswith('construct.'):
            del sys.modules[k]

    from dataclasses import dataclass
    from construct import StructMixin, field, Int32ub

    # ---- 构造 @dataclass Flat 类，N 个 Int32ub 字段 ----
    code_lines = ['@dataclass', 'class Flat(StructMixin):']
    for i in range(N):
        code_lines.append('    f{{}}: int = field(Int32ub)'.format(i))
    ns = {{'dataclass': dataclass, 'StructMixin': StructMixin,
           'field': field, 'Int32ub': Int32ub}}
    exec('\\n'.join(code_lines), ns)
    Flat = ns['Flat']
    schema = Flat._construct_compiled

    # ---- 数据：4N 字节，每 4 字节一个 Int32ub ----
    data = bytes((i % 256) for i in range(4 * N))

    # 预解析一次实例，用于后续验证字段读取
    pre_parsed = schema._parse_raw(data)

    # 校验：_parse_raw 返回实例（不是 dict）
    assert isinstance(pre_parsed, Flat), (
        '_parse_raw should return Flat instance, got {{}}'.format(type(pre_parsed))
    )
    # 字段可通过 getattr 访问
    for i in range(N):
        _ = getattr(pre_parsed, 'f{{}}'.format(i))

    # ---- 测量目标 ----
    # 1. Full parse（Rust 内构造实例 + FFI 返回）
    target_full = lambda: Flat.parse(data)
    # 2. 仅 Rust _parse_raw（直接返回实例）
    target_raw = lambda: schema._parse_raw(data)

    # ---- 预热 ----
    target_full(); target_raw()

    def measure(target):
        timer = timeit.Timer(target)
        times = timer.repeat(repeat=REPEAT, number=NUMBER)
        return (statistics.median(times) / NUMBER) * 1e9

    full_ns = measure(target_full)
    raw_ns = measure(target_raw)

    print(json.dumps({{
        'impl': 'rs',
        'n': N,
        'full_parse_ns': full_ns,
        'parse_raw_ns': raw_ns,
        'dataclass_init_ns': 0.0,           # 兼容字段，当前不再分离
        'dataclass_init_positional_ns': 0.0,  # 兼容字段
        'raw_then_init_ns': raw_ns,         # raw == full
    }}))
    """
)


# ---------------------------------------------------------------------------
# 子进程脚本：测量 Python construct 2.10.70 基线
# ---------------------------------------------------------------------------

_PY_SCRIPT = textwrap.dedent(
    """
    import json
    import statistics
    import sys
    import timeit

    N = {n}
    NUMBER = {number}
    REPEAT = {repeat}

    # ---- sys.path 操纵：清除 construct-rs，使用 site-packages 原版 ----
    crs_dir = {crs_python_dir!r}
    sys.path[:] = [p for p in sys.path if p != crs_dir]
    for k in list(sys.modules):
        if k == 'construct' or k.startswith('construct.'):
            del sys.modules[k]

    import construct as pc

    # ---- 构造 Struct，N 个 Int32ub 字段 ----
    subcons = [('f{{}}'.format(i), pc.Int32ub) for i in range(N)]
    flat = pc.Struct(*[(name / cons) for name, cons in subcons])

    # ---- 数据：4N 字节 ----
    data = bytes((i % 256) for i in range(4 * N))

    target = lambda: flat.parse(data)
    target()  # 预热

    timer = timeit.Timer(target)
    times = timer.repeat(repeat=REPEAT, number=NUMBER)
    py_ns = (statistics.median(times) / NUMBER) * 1e9

    print(json.dumps({{
        'impl': 'py',
        'n': N,
        'parse_ns': py_ns,
    }}))
    """
)


def _run_rs(n: int) -> dict:
    """在子进程中测量 construct-rs 三阶段。"""
    code = _RS_SCRIPT.format(
        n=n, number=NUMBER, repeat=REPEAT, crs_python_dir=_CRS_PYTHON_DIR,
    )
    env = os.environ.copy()
    # ABI3 forward compat（与 conftest/test 同环境）
    env["PYO3_USE_ABI3_FORWARD_COMPATIBILITY"] = "1"
    result = subprocess.run(
        [sys.executable, "-c", code],
        capture_output=True, text=True, encoding="utf-8",
        errors="replace", check=False, env=env,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"construct-rs 测量失败 n={n}\nstdout:\n{result.stdout}\n"
            f"stderr:\n{result.stderr}"
        )
    lines = [ln for ln in result.stdout.strip().splitlines() if ln.strip()]
    return json.loads(lines[-1])


def _run_py(n: int) -> dict:
    """在子进程中测量 Python construct 基线。"""
    code = _PY_SCRIPT.format(
        n=n, number=NUMBER, repeat=REPEAT, crs_python_dir=_CRS_PYTHON_DIR,
    )
    env = os.environ.copy()
    env["PYO3_USE_ABI3_FORWARD_COMPATIBILITY"] = "1"
    result = subprocess.run(
        [sys.executable, "-c", code],
        capture_output=True, text=True, encoding="utf-8",
        errors="replace", check=False, env=env,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"Python construct 测量失败 n={n}\nstdout:\n{result.stdout}\n"
            f"stderr:\n{result.stderr}"
        )
    lines = [ln for ln in result.stdout.strip().splitlines() if ln.strip()]
    return json.loads(lines[-1])


# ---------------------------------------------------------------------------
# 报告输出
# ---------------------------------------------------------------------------


def format_report(results: list[dict]) -> str:
    """格式化完整测量报告。"""
    lines = []

    # ---- 逐用例详细 ----
    for r in results:
        case = r["case"]
        n = r["n"]
        full = r["full_parse_ns"]
        raw = r["parse_raw_ns"]
        py = r["py_parse_ns"]

        full_speedup = (py / full) if full > 0 else 0
        raw_speedup = (py / raw) if raw > 0 else 0

        lines.append(f"=== {case} ({n} fields) ===")
        lines.append(f"Full parse (Flat.parse):      {full:>10.1f} ns")
        lines.append(f"Rust _parse_raw:              {raw:>10.1f} ns  "
                      f"(raw == full，实例在 Rust 内构造)")
        lines.append(f"Python construct:             {py:>10.1f} ns")
        lines.append(f"Full 加速比:                  {full_speedup:>5.2f}x")
        lines.append(f"Rust-only 加速比:             {raw_speedup:>5.2f}x")
        lines.append("")

    # ---- 汇总表 ----
    lines.append("=== 汇总 ===")
    header = (
        f"| {'用例':<4} | {'N':>3} | "
        f"{'Rust raw':>9} | {'Full':>9} | {'Python':>9} | "
        f"{'Full x':>6} | {'Raw x':>6} |"
    )
    lines.append(header)
    lines.append("|" + "-" * (len(header) - 2) + "|")
    for r in results:
        full = r["full_parse_ns"]
        raw = r["parse_raw_ns"]
        py = r["py_parse_ns"]
        full_x = py / full if full > 0 else 0
        raw_x = py / raw if raw > 0 else 0
        lines.append(
            f"| {r['case']:<4} | {r['n']:>3} | "
            f"{raw:>9.1f} | {full:>9.1f} | {py:>9.1f} | "
            f"{full_x:>5.2f}x | {raw_x:>5.2f}x |"
        )
    lines.append("")

    # ---- Rust _parse_raw 增长模型（线性性验证）----
    if len(results) >= 3:
        lines.append("=== Rust _parse_raw 增长模型 ===")
        lines.append("  _parse_raw / N（应为常数，证明线性）：")
        for r in results:
            ratio = r["parse_raw_ns"] / r["n"] if r["n"] > 0 else 0
            lines.append(
                f"    {r['case']} (N={r['n']:>3}): {ratio:>7.2f} ns/N  "
                f"(raw={r['parse_raw_ns']:.1f} ns)"
            )
        lines.append("")

        # 加速比对比
        lines.append("=== 加速比 vs Python construct ===")
        for r in results:
            full = r["full_parse_ns"]
            py = r["py_parse_ns"]
            full_x = py / full if full > 0 else 0
            gate = "✓ PASS" if full_x >= 4.0 else ("~ smoke" if full_x >= 0.5 else "✗ FAIL")
            lines.append(
                f"    {r['case']} (N={r['n']:>3}): {full_x:>5.2f}x  [{gate}]  "
                f"(4x target, 0.5x smoke gate)"
            )
        lines.append("")

    return "\n".join(lines)


def write_results_file(report: str, results: list[dict]) -> Path:
    """写完整结果到 bench/results/bench_breakdown_results.txt。"""
    out_path = Path(__file__).resolve().parent / "results" / "bench_breakdown_results.txt"

    lines = []
    lines.append("=" * 72)
    lines.append("construct-rs parse 分阶段计时（根因验证）")
    lines.append("=" * 72)
    lines.append("")
    lines.append("环境信息：")
    lines.append(f"  Python 版本    : {sys.version.split()[0]}")
    lines.append(f"  平台           : {platform.platform()}")
    lines.append(f"  处理器         : {platform.processor() or '未知'}")
    lines.append(f"  timeit number  : {NUMBER}")
    lines.append(f"  repeat (中位数): {REPEAT}")
    lines.append(f"  测量时间       : {time.strftime('%Y-%m-%d %H:%M:%S')}")
    lines.append("")
    lines.append("测量阶段：")
    lines.append("  - Full parse           : Flat.parse(data) [Rust 内完整构造实例]")
    lines.append("  - Rust _parse_raw      : schema._parse_raw(data) [直接返回实例]")
    lines.append("  - Python construct     : pc.Struct(...).parse(data) [绝对基线]")
    lines.append("")
    lines.append(report)
    lines.append("")
    lines.append("原始 JSON：")
    for r in results:
        lines.append("  " + json.dumps(r, ensure_ascii=False))

    out_path.write_text("\n".join(lines), encoding="utf-8")
    return out_path


def main() -> int:
    print("=" * 72)
    print("construct-rs parse 分阶段计时（根因验证）")
    print("=" * 72)
    print(f"Python 版本: {sys.version.split()[0]}")
    print(f"平台       : {platform.platform()}")
    print(f"timeit number={NUMBER}, repeat={REPEAT} (取中位数)")
    print()
    print(f"用例: {', '.join(c for c, _ in CASES)}")
    print()

    results = []
    for case, n in CASES:
        print(f"  测量 {case} (N={n}) construct-rs 多阶段...", end="", flush=True)
        rs = _run_rs(n)
        print(f" full={rs['full_parse_ns']:.1f}ns "
              f"raw={rs['parse_raw_ns']:.1f}ns")

        print(f"  测量 {case} (N={n}) Python construct 基线...", end="", flush=True)
        py = _run_py(n)
        print(f" {py['parse_ns']:.1f}ns")

        results.append({
            "case": case,
            "n": n,
            "full_parse_ns": rs["full_parse_ns"],
            "parse_raw_ns": rs["parse_raw_ns"],
            "py_parse_ns": py["parse_ns"],
        })

    print()
    report = format_report(results)
    # 先写文件（UTF-8），避免 Windows 控制台 GBK 编码失败导致丢数据
    out_path = write_results_file(report, results)

    # 再打印到 stdout（容错：Windows GBK 控制台可能无法编码中文）
    try:
        print(report)
    except UnicodeEncodeError:
        # 回退：用 ASCII 安全方式打印
        sys.stdout.reconfigure(errors="replace")  # type: ignore[attr-defined]
        print(report)

    print(f"详细结果已写入: {out_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
