"""Controlled A/B Test 测量脚本（META-CI-1c，L-09 对策工程化）.

通用化自 experiments/E01_O1_ab_bench.py：
- 移除硬编码 SCENARIOS 列表
- 通过 JSON 配置文件参数化场景列表 + 对照组
- 保留 S-PERF 测量口径（min(repeat=5) × number / 子进程隔离 / apples-to-apples）

JSON 配置 schema（ab_bench_config.json）：

    {
      "crs_python": "C:/.../crs_venv_new/Scripts/python.exe",
      "pc_python": "C:/.../crs_venv_py_new/Scripts/python.exe",
      "repeat": 5,
      "output_pattern": "ab_bench_{label}.json",
      "scenarios": [
        {
          "key": "E01",
          "name": "E01 Array(0, Index()) parse",
          "number": 10000,
          "role": "investigation",
          "crs_setup": "...",
          "crs_stmt": "pkt.parse(data)",
          "pc_setup": "...",
          "pc_stmt": "pkt.parse(data)"
        },
        {
          "key": "S01",
          "name": "S01 ...",
          "role": "positive_control",
          ...
        },
        {
          "key": "i01",
          "name": "i01 ...",
          "role": "negative_control",
          ...
        }
      ]
    }

  role 字段（设计 §5.7）：
    - "investigation"    调查目标（质疑改动是否影响此场景）
    - "positive_control" 阳性对照（预期有效应，验证 toggle 真实性）
    - "negative_control" 阴性对照（预期无效应，验证环境漂移幅度）

用法：
  python ab_bench.py --label A1_O1off --config ab_bench_config.json
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence

REPEAT_DEFAULT = 5


def measure(
    python_exe: str,
    setup_code: str,
    stmt_code: str,
    number: int,
    repeat: int = REPEAT_DEFAULT,
) -> float:
    """在子进程中跑 timeit，返回 min ns/call（S-PERF 口径）。

    设置 PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1（§5.3 测量口径）。
    """
    full_script = f"""
import timeit
import json

setup = '''{setup_code}'''

exec(setup)

t = timeit.repeat(stmt={stmt_code!r}, setup=setup, number={number}, repeat={repeat})
print(json.dumps({{"min_s": min(t), "number": {number}}}))
"""
    env = dict(os.environ)
    env["PYO3_USE_ABI3_FORWARD_COMPATIBILITY"] = "1"
    result = subprocess.run(
        [python_exe, "-c", full_script],
        capture_output=True, text=True, timeout=180,
        env=env,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"subprocess failed: rc={result.returncode}\nstderr:\n{result.stderr[:1500]}"
        )
    lines = result.stdout.strip().split("\n")
    data = json.loads(lines[-1])
    return (data["min_s"] / data["number"]) * 1e9


def load_config(config_path: Path) -> Dict[str, Any]:
    """加载 JSON 配置。"""
    with config_path.open("r", encoding="utf-8") as f:
        return json.load(f)


def run_bench(
    config: Dict[str, Any],
    label: str,
    output_path: Optional[Path] = None,
) -> Dict[str, Any]:
    """对 config["scenarios"] 中所有场景跑测量，输出结果 dict。"""
    crs_python = config["crs_python"]
    pc_python = config["pc_python"]
    repeat = int(config.get("repeat", REPEAT_DEFAULT))
    scenarios = config["scenarios"]
    ts = time.strftime("%Y-%m-%d %H:%M:%S")

    print(f"\n{'=' * 70}")
    print(f"A/B bench run | label={label} | timestamp={ts}")
    print(f"  scenarios: {len(scenarios)}")
    print(f"  crs_python: {crs_python}")
    print(f"  pc_python: {pc_python}")
    print(f"{'=' * 70}", flush=True)

    results: List[Dict[str, Any]] = []
    for sc in scenarios:
        key = sc["key"]
        name = sc.get("name", key)
        number = int(sc["number"])
        role = sc.get("role", "investigation")
        print(f"\n--- {name} (number={number}, role={role}) ---", flush=True)

        crs_ns = measure(crs_python, sc["crs_setup"], sc["crs_stmt"], number, repeat)
        print(f"  construct-rs: {crs_ns:.1f} ns/call", flush=True)
        pc_ns = measure(pc_python, sc["pc_setup"], sc["pc_stmt"], number, repeat)
        print(f"  python-construct: {pc_ns:.1f} ns/call", flush=True)
        speedup = pc_ns / crs_ns if crs_ns > 0 else float("inf")
        mark = "OK>=10x" if speedup >= 10 else "<10x"
        print(f"  speedup: {speedup:.2f}x  [{mark}]", flush=True)

        results.append({
            "key": key, "name": name, "role": role,
            "number": number,
            "crs_ns": crs_ns, "pc_ns": pc_ns, "speedup": speedup,
        })

    out = {
        "label": label,
        "timestamp": ts,
        "repeat": repeat,
        "results": results,
    }

    if output_path is not None:
        output_path.parent.mkdir(parents=True, exist_ok=True)
        with output_path.open("w", encoding="utf-8") as f:
            json.dump(out, f, ensure_ascii=False, indent=2)
        print(f"\n结果已保存到 {output_path}", flush=True)

    # 单行 SUMMARY（与 E01_O1_ab_bench.py 兼容，供 harness 日志提取）
    parts = [
        f'{r["key"]}={r["speedup"]:.2f}x(rs={r["crs_ns"]:.0f}ns,pc={r["pc_ns"]:.0f}ns)'
        for r in results
    ]
    print(f"\n[SUMMARY] label={label} | " + " | ".join(parts), flush=True)

    return out


def _parse_args(argv: Sequence[str]) -> Dict[str, Any]:
    args: Dict[str, Any] = {"label": "unknown", "config": None, "output": None}
    i = 0
    while i < len(argv):
        a = argv[i]
        if a == "--label" and i + 1 < len(argv):
            args["label"] = argv[i + 1]; i += 2; continue
        if a == "--config" and i + 1 < len(argv):
            args["config"] = argv[i + 1]; i += 2; continue
        if a == "--output" and i + 1 < len(argv):
            args["output"] = argv[i + 1]; i += 2; continue
        if a in ("-h", "--help"):
            sys.stdout.write(__doc__ or "")
            sys.exit(0)
        sys.stderr.write(f"unknown arg: {a}\n")
        sys.exit(2)
    return args


def main(argv: Optional[Sequence[str]] = None) -> int:
    """CLI 主入口。"""
    args = _parse_args(list(sys.argv[1:] if argv is None else argv))
    if not args["config"]:
        sys.stderr.write("required: --config <ab_bench_config.json>\n")
        return 2

    config = load_config(Path(args["config"]))
    output_path = Path(args["output"]) if args["output"] else None

    try:
        run_bench(config, args["label"], output_path)
    except (RuntimeError, subprocess.SubprocessError, KeyError) as exc:
        sys.stderr.write(f"bench failed: {exc}\n")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
