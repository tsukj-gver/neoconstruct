"""Phase 1 功能 smoke：FormatField / Bytes / GreedyBytes / Struct / StructRef.

设计依据：docs/design/CI冒烟门禁设计.md §4.2 + §1.1 B 类 + §6.2（子任务 1b 范围）。

覆盖范围（inventory.csv Phase 1 implemented）：
  - FormatField 16 个 Int 单例（Int8ub/Int8ul/Int8sb/Int8sl + Int16*4 + Int32*4 + Int64*4）
  - Bytes(count)（impl_phase=2，但 Phase 1 smoke 中作为定长字段类型验证）
  - GreedyBytes
  - Struct / StructRef（B1/B2 类型多字段场景）

设计原则：
  - 与 phase4_smoke_*.py 风格一致（@dataclass + StructMixin + assert）
  - 子进程隔离对照 Python construct 2.10.70（双 venv：crs_venv_new / crs_venv_py_new）
  - 主进程运行 construct-rs（crs_venv_new），通过 subprocess 跑 Python construct

退出码：0=PASS / 1=FAIL。
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import traceback
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Tuple

# ---------------------------------------------------------------------------
# 路径与 venv 配置
# ---------------------------------------------------------------------------

PROJECT_ROOT = Path(__file__).resolve().parent.parent
CRS_PYTHON = Path(os.environ.get(
    "CRS_VENV_PYTHON",
    r"<opencode-temp>\crs_venv_new\Scripts\python.exe",
))
PC_PYTHON = Path(os.environ.get(
    "PC_VENV_PYTHON",
    r"<opencode-temp>\crs_venv_py_new\Scripts\python.exe",
))


def _check_pythons() -> None:
    """验证两个 venv python.exe 可用，否则 exit 1。"""
    missing: List[str] = []
    if not CRS_PYTHON.exists():
        missing.append(f"CRS_VENV_PYTHON={CRS_PYTHON}")
    if not PC_PYTHON.exists():
        missing.append(f"PC_VENV_PYTHON={PC_PYTHON}")
    if missing:
        sys.stderr.write(
            "phase1_smoke: required venv python missing:\n  "
            + "\n  ".join(missing)
            + "\n  (set CRS_VENV_PYTHON / PC_VENV_PYTHON env to override)\n"
        )
        sys.exit(1)


# ---------------------------------------------------------------------------
# Python construct 2.10.70 对照（子进程隔离）
# ---------------------------------------------------------------------------

# 子进程脚本模板：导入 Python construct，运行场景函数，输出 JSON
_PC_RUNNER = """
import json
import sys
import traceback
from construct import Struct, Bytes, GreedyBytes, Container
from construct import (
    Int8ub, Int8ul, Int8sb, Int8sl,
    Int16ub, Int16ul, Int16sb, Int16sl,
    Int32ub, Int32ul, Int32sb, Int32sl,
    Int64ub, Int64ul, Int64sb, Int64sl,
)

_NAMED = {
    "Int8ub": Int8ub, "Int8ul": Int8ul, "Int8sb": Int8sb, "Int8sl": Int8sl,
    "Int16ub": Int16ub, "Int16ul": Int16ul, "Int16sb": Int16sb, "Int16sl": Int16sl,
    "Int32ub": Int32ub, "Int32ul": Int32ul, "Int32sb": Int32sb, "Int32sl": Int32sl,
    "Int64ub": Int64ub, "Int64ul": Int64ul, "Int64sb": Int64sb, "Int64sl": Int64sl,
}


def run(scenario):
    tag = scenario["tag"]
    kind = scenario["kind"]
    if kind == "struct":
        fields = []
        for fname, fmt_name in scenario["fields"]:
            subcon = _NAMED[fmt_name]
            fields.append(fname / subcon)
        st = Struct(*fields)
        if "parse_data" in scenario:
            obj = st.parse(bytes(scenario["parse_data"]))
            # Python construct 的 Container 含 _io (BytesIO) 等内部键，
            # 剔除以 JSON 序列化；仅保留用户字段。
            clean = {}
            for k, v in dict(obj).items():
                if k.startswith("_"):
                    continue
                if isinstance(v, bytes):
                    clean[k] = list(v)
                else:
                    clean[k] = v
            return {"parsed_dict": clean}
        else:
            data = st.build(Container(scenario["build_dict"]))
            return {"built": list(data)}
    elif kind == "bytes_fixed":
        subcon = Bytes(scenario["length"])
        data = bytes(scenario["data"])
        parsed = subcon.parse(data)
        built = subcon.build(parsed)
        return {"parsed": list(parsed), "built": list(built)}
    elif kind == "greedy_bytes":
        data = bytes(scenario["data"])
        parsed = GreedyBytes.parse(data)
        built = GreedyBytes.build(parsed)
        return {"parsed": list(parsed), "built": list(built)}
    else:
        raise ValueError("unknown kind: " + kind)


def main():
    payload = sys.stdin.read()
    scenario = json.loads(payload)
    try:
        result = run(scenario)
        sys.stdout.write(json.dumps({"ok": True, "result": result}))
    except Exception as e:
        sys.stdout.write(json.dumps({
            "ok": False,
            "error_type": type(e).__name__,
            "error_msg": str(e),
            "traceback": traceback.format_exc(),
        }))


main()
"""


def run_python_construct(scenario: Dict[str, Any]) -> Dict[str, Any]:
    """在 crs_venv_py_new 子进程中跑 Python construct 2.10.70。

    返回 {ok: bool, result|error}。
    """
    payload = json.dumps(scenario)
    try:
        proc = subprocess.run(
            [str(PC_PYTHON), "-c", _PC_RUNNER],
            input=payload,
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=30,
        )
    except subprocess.TimeoutExpired:
        return {"ok": False, "error_type": "Timeout", "error_msg": "30s"}
    if proc.returncode != 0:
        return {
            "ok": False,
            "error_type": "SubprocessExit",
            "error_msg": proc.stderr.strip()[:500],
        }
    try:
        return json.loads(proc.stdout)
    except json.JSONDecodeError as exc:
        return {
            "ok": False,
            "error_type": "JSONDecodeError",
            "error_msg": f"stdout={proc.stdout[:200]} err={exc}",
        }


# ---------------------------------------------------------------------------
# 构造器用户面定义（用于对照——动态生成）
# ---------------------------------------------------------------------------

# 16 个 FormatField 单例（impl_phase=1）
FORMAT_FIELDS: List[Tuple[str, str]] = [
    ("Int8ub", "Int8ub"),
    ("Int8ul", "Int8ul"),
    ("Int8sb", "Int8sb"),
    ("Int8sl", "Int8sl"),
    ("Int16ub", "Int16ub"),
    ("Int16ul", "Int16ul"),
    ("Int16sb", "Int16sb"),
    ("Int16sl", "Int16sl"),
    ("Int32ub", "Int32ub"),
    ("Int32ul", "Int32ul"),
    ("Int32sb", "Int32sb"),
    ("Int32sl", "Int32sl"),
    ("Int64ub", "Int64ub"),
    ("Int64ul", "Int64ul"),
    ("Int64sb", "Int64sb"),
    ("Int64sl", "Int64sl"),
]


def _import_crs_construct() -> Any:
    """导入 construct-rs 包（应在 crs_venv_new 中运行）。"""
    # 强制使用项目内 construct-rs（避免与 site-packages 冲突）
    sys.path.insert(0, str(PROJECT_ROOT / "construct-rs" / "python"))
    import construct  # type: ignore
    return construct


# ---------------------------------------------------------------------------
# 结果收集器
# ---------------------------------------------------------------------------


@dataclass
class CaseResult:
    """单个 smoke 场景的结果。"""

    name: str
    passed: bool
    detail: str = ""


def _compare(name: str, expected: Any, actual: Any) -> CaseResult:
    """通用值比较。"""
    if expected == actual:
        return CaseResult(name=name, passed=True)
    return CaseResult(
        name=name,
        passed=False,
        detail=f"expected={expected!r} actual={actual!r}",
    )


# ---------------------------------------------------------------------------
# 场景实现（construct-rs）
# ---------------------------------------------------------------------------


def test_formatfield_single(crs: Any, fmt_name: str) -> List[CaseResult]:
    """单个 FormatField parse/build 与 Python construct 对照。

    construct-rs 的 Int8ub 等是 FormatFieldDescriptor 单例（无 parse/build 方法），
    必须包到 1-字段 Struct 中通过用户面 API 调用。

    针对每种 Int 单例取若干代表性 byte pattern：
      - 全 0 / 全 1（极值）
      - 一组典型值
    """
    import re

    out: List[CaseResult] = []
    cls = getattr(crs, fmt_name)
    StructMixin = crs.StructMixin
    field = crs.field

    # 解析 width 与 signed（"Int8ub" -> width=1, signed=False）
    m = re.match(r"^Int(\d+)([us])([bl])$", fmt_name)
    if not m:
        return [CaseResult(
            name=f"{fmt_name} (parse name)",
            passed=False,
            detail=f"unrecognized format name: {fmt_name}",
        )]
    bits = int(m.group(1))
    width = bits // 8
    is_signed = (m.group(2) == "s")

    # 极值 + 中间值
    if not is_signed:
        test_values = [0, 1, 0x7F, (1 << bits) - 1]
    else:
        test_values = [0, 1, -1, -(1 << (bits - 1)), (1 << (bits - 1)) - 1]
    # 去重保持顺序
    seen: set = set()
    unique_values = []
    for v in test_values:
        if v not in seen:
            seen.add(v)
            unique_values.append(v)

    # 构造 1-字段 Struct：{ v: <fmt_name> }
    # dataclass 字段名必须合法标识符；这里用 v
    @dataclass
    class Single(StructMixin):
        v: int = field(cls)

    for v in unique_values:
        # construct-rs build
        msg = Single(v=v)
        built = msg.build()
        # construct-rs parse round-trip
        parsed = Single.parse(built)
        out.append(_compare(
            f"{fmt_name}({v}) round-trip parse.v",
            v,
            parsed.v,
        ))
        # 对照 Python construct
        scenario = {
            "tag": f"{fmt_name}/{v}",
            "kind": "struct",
            "fields": [("v", fmt_name)],
            "build_dict": {"v": v},
        }
        pc = run_python_construct(scenario)
        if not pc.get("ok"):
            out.append(CaseResult(
                name=f"{fmt_name}({v}) vs Python construct",
                passed=False,
                detail=f"PC runner error: {pc.get('error_type')}: {pc.get('error_msg')}",
            ))
            continue
        pc_result = pc["result"]
        # 比对 build 字节
        out.append(_compare(
            f"{fmt_name}({v}): rs vs pc bytes",
            tuple(pc_result["built"]),
            tuple(built),
        ))
    return out


def test_bytes_fixed(crs: Any) -> List[CaseResult]:
    """Bytes(count) parse/build round-trip + Python construct 对照。"""
    out: List[CaseResult] = []
    Bytes = crs.Bytes
    StructMixin = crs.StructMixin
    field = crs.field

    @dataclass
    class P(StructMixin):
        data: bytes = field(Bytes(4))

    # parse
    raw = b"\x01\x02\x03\x04"
    parsed = P.parse(raw)
    out.append(_compare("Bytes(4).parse", raw, parsed.data))

    # build
    msg = P(data=b"\xAA\xBB\xCC\xDD")
    built = msg.build()
    out.append(_compare("Bytes(4).build", b"\xAA\xBB\xCC\xDD", built))

    # Python construct 对照
    scenario = {
        "tag": "Bytes(4)",
        "kind": "bytes_fixed",
        "length": 4,
        "data": list(raw),
    }
    pc = run_python_construct(scenario)
    if pc.get("ok"):
        out.append(_compare(
            "Bytes(4).parse vs Python construct",
            tuple(pc["result"]["parsed"]),
            tuple(parsed.data),
        ))
    else:
        out.append(CaseResult(
            name="Bytes(4) vs Python construct",
            passed=False,
            detail=f"PC error: {pc.get('error_msg')}",
        ))
    return out


def test_greedy_bytes(crs: Any) -> List[CaseResult]:
    """GreedyBytes parse/build round-trip + Python construct 对照。"""
    out: List[CaseResult] = []
    GreedyBytes = crs.GreedyBytes
    StructMixin = crs.StructMixin
    field = crs.field

    @dataclass
    class P(StructMixin):
        data: bytes = field(GreedyBytes)

    raw = b"\xDE\xAD\xBE\xEF"
    parsed = P.parse(raw)
    out.append(_compare("GreedyBytes.parse", raw, parsed.data))

    msg = P(data=raw)
    built = msg.build()
    out.append(_compare("GreedyBytes.build", raw, built))

    # Python construct 对照（GreedyBytes 是 stream-only，在 Struct 外直接 parse）
    scenario = {
        "tag": "GreedyBytes",
        "kind": "greedy_bytes",
        "data": list(raw),
    }
    pc = run_python_construct(scenario)
    if pc.get("ok"):
        out.append(_compare(
            "GreedyBytes vs Python construct",
            tuple(pc["result"]["parsed"]),
            tuple(parsed.data),
        ))
    else:
        out.append(CaseResult(
            name="GreedyBytes vs Python construct",
            passed=False,
            detail=f"PC error: {pc.get('error_msg')}",
        ))
    return out


def test_struct_b1(crs: Any) -> List[CaseResult]:
    """B1 类型：3 字段 Struct round-trip + Python construct 对照。

    struct { a: Int8ub, b: Int16ub, c: Int32ub }
    """
    out: List[CaseResult] = []
    Int8ub, Int16ub, Int32ub = crs.Int8ub, crs.Int16ub, crs.Int32ub
    StructMixin, field = crs.StructMixin, crs.field

    @dataclass
    class B1(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int16ub)
        c: int = field(Int32ub)

    # parse
    raw = b"\x01\x02\x03\x04\x05\x06\x07"
    parsed = B1.parse(raw)
    out.append(_compare("B1.parse.a", 0x01, parsed.a))
    out.append(_compare("B1.parse.b", 0x0203, parsed.b))
    out.append(_compare("B1.parse.c", 0x04050607, parsed.c))

    # build round-trip
    msg = B1(a=0x01, b=0x0203, c=0x04050607)
    built = msg.build()
    out.append(_compare("B1.build", raw, built))

    # Python construct 对照 parse
    scenario = {
        "tag": "B1",
        "kind": "struct",
        "fields": [("a", "Int8ub"), ("b", "Int16ub"), ("c", "Int32ub")],
        "parse_data": list(raw),
    }
    pc = run_python_construct(scenario)
    if pc.get("ok"):
        pc_dict = pc["result"]["parsed_dict"]
        out.append(_compare("B1.a vs Python construct", pc_dict["a"], parsed.a))
        out.append(_compare("B1.b vs Python construct", pc_dict["b"], parsed.b))
        out.append(_compare("B1.c vs Python construct", pc_dict["c"], parsed.c))
    else:
        out.append(CaseResult(
            name="B1 vs Python construct",
            passed=False,
            detail=f"PC error: {pc.get('error_msg')}",
        ))
    return out


def test_struct_b2(crs: Any) -> List[CaseResult]:
    """B2 类型：10 字段 Struct（混合 Int8ub/Int16ub/Int32ub）。"""
    out: List[CaseResult] = []
    Int8ub, Int16ub, Int32ub = crs.Int8ub, crs.Int16ub, crs.Int32ub
    StructMixin, field = crs.StructMixin, crs.field

    # 10 字段定义（3*Int8ub + 3*Int16ub + 4*Int32ub）
    @dataclass
    class B2(StructMixin):
        f0: int = field(Int8ub)
        f1: int = field(Int8ub)
        f2: int = field(Int8ub)
        f3: int = field(Int16ub)
        f4: int = field(Int16ub)
        f5: int = field(Int16ub)
        f6: int = field(Int32ub)
        f7: int = field(Int32ub)
        f8: int = field(Int32ub)
        f9: int = field(Int32ub)

    # 构造测试数据：3*1 + 3*2 + 4*4 = 25 字节
    raw = bytes(range(25))
    parsed = B2.parse(raw)
    # 逐字段比对（前 3 个 Int8ub）
    out.append(_compare("B2.f0", 0, parsed.f0))
    out.append(_compare("B2.f1", 1, parsed.f1))
    out.append(_compare("B2.f2", 2, parsed.f2))
    # Int16ub BE: bytes 3-4 -> 0x0304
    out.append(_compare("B2.f3", 0x0304, parsed.f3))

    # build round-trip
    msg = B2(
        f0=0, f1=1, f2=2,
        f3=0x0304, f4=0x0506, f5=0x0708,
        f6=0x090A0B0C, f7=0x0D0E0F10,
        f8=0x11121314, f9=0x15161718,
    )
    built = msg.build()
    out.append(_compare("B2.build.length", 25, len(built)))

    # Python construct 对照 parse
    scenario = {
        "tag": "B2",
        "kind": "struct",
        "fields": [
            ("f0", "Int8ub"), ("f1", "Int8ub"), ("f2", "Int8ub"),
            ("f3", "Int16ub"), ("f4", "Int16ub"), ("f5", "Int16ub"),
            ("f6", "Int32ub"), ("f7", "Int32ub"),
            ("f8", "Int32ub"), ("f9", "Int32ub"),
        ],
        "parse_data": list(raw),
    }
    pc = run_python_construct(scenario)
    if pc.get("ok"):
        pc_dict = pc["result"]["parsed_dict"]
        for i in range(10):
            key = f"f{i}"
            out.append(_compare(f"B2.{key} vs PC", pc_dict[key], getattr(parsed, key)))
    else:
        out.append(CaseResult(
            name="B2 vs Python construct",
            passed=False,
            detail=f"PC error: {pc.get('error_msg')}",
        ))
    return out


def test_modbus_rtu(crs: Any) -> List[CaseResult]:
    """ModbusRTU 端到端用例（与 examples/example.py 对齐）。"""
    out: List[CaseResult] = []
    Int8ub, GreedyBytes = crs.Int8ub, crs.GreedyBytes
    StructMixin, field = crs.StructMixin, crs.field

    @dataclass
    class ModbusRTUMessage(StructMixin):
        address: int = field(Int8ub)
        function_code: int = field(Int8ub)
        data: bytes = field(GreedyBytes)

    msg = ModbusRTUMessage(address=1, function_code=3, data=b"\x00\x01\x00\x02")
    built = msg.build()
    out.append(_compare(
        "ModbusRTU.build",
        b"\x01\x03\x00\x01\x00\x02",
        built,
    ))
    parsed = ModbusRTUMessage.parse(built)
    out.append(_compare("ModbusRTU.parse.address", 1, parsed.address))
    out.append(_compare("ModbusRTU.parse.function_code", 3, parsed.function_code))
    out.append(_compare("ModbusRTU.parse.data", b"\x00\x01\x00\x02", parsed.data))
    return out


# ---------------------------------------------------------------------------
# 主入口
# ---------------------------------------------------------------------------


def _print_result(r: CaseResult) -> None:
    if r.passed:
        print(f"  [PASS] {r.name}")
    else:
        print(f"  [FAIL] {r.name}  {r.detail}")


def main() -> int:
    _check_pythons()
    print("=" * 60)
    print("Phase 1 smoke: FormatField / Bytes / GreedyBytes / Struct")
    print(f"  CRS venv python : {CRS_PYTHON}")
    print(f"  PC  venv python : {PC_PYTHON}")
    print("=" * 60)

    # 必须在 crs_venv_new 中（通过 L1 装好的 wheel 或 sys.path 注入）
    if sys.executable != str(CRS_PYTHON):
        # 不强制——开发期可手动指定；但报告路径
        print(
            f"[WARN] current python ({sys.executable}) != CRS_VENV_PYTHON; "
            "construct-rs must be importable from this process"
        )

    crs = _import_crs_construct()
    all_results: List[CaseResult] = []

    print("\n[Section 1] FormatField 16 个 Int 单例 × 极值/中间值（与 PC 对照）")
    for fmt_name, _ in FORMAT_FIELDS:
        try:
            rs = test_formatfield_single(crs, fmt_name)
        except Exception as exc:  # noqa: BLE001
            rs = [CaseResult(
                name=f"{fmt_name} (group)",
                passed=False,
                detail=f"exception: {type(exc).__name__}: {exc}",
            )]
            traceback.print_exc()
        all_results.extend(rs)

    print("\n[Section 2] Bytes(count)")
    try:
        all_results.extend(test_bytes_fixed(crs))
    except Exception as exc:  # noqa: BLE001
        all_results.append(CaseResult(
            name="Bytes (group)", passed=False,
            detail=f"exception: {type(exc).__name__}: {exc}",
        ))
        traceback.print_exc()

    print("\n[Section 3] GreedyBytes")
    try:
        all_results.extend(test_greedy_bytes(crs))
    except Exception as exc:  # noqa: BLE001
        all_results.append(CaseResult(
            name="GreedyBytes (group)", passed=False,
            detail=f"exception: {type(exc).__name__}: {exc}",
        ))
        traceback.print_exc()

    print("\n[Section 4] Struct B1 (3 字段)")
    try:
        all_results.extend(test_struct_b1(crs))
    except Exception as exc:  # noqa: BLE001
        all_results.append(CaseResult(
            name="B1 (group)", passed=False,
            detail=f"exception: {type(exc).__name__}: {exc}",
        ))
        traceback.print_exc()

    print("\n[Section 5] Struct B2 (10 字段)")
    try:
        all_results.extend(test_struct_b2(crs))
    except Exception as exc:  # noqa: BLE001
        all_results.append(CaseResult(
            name="B2 (group)", passed=False,
            detail=f"exception: {type(exc).__name__}: {exc}",
        ))
        traceback.print_exc()

    print("\n[Section 6] ModbusRTU 端到端用例")
    try:
        all_results.extend(test_modbus_rtu(crs))
    except Exception as exc:  # noqa: BLE001
        all_results.append(CaseResult(
            name="ModbusRTU (group)", passed=False,
            detail=f"exception: {type(exc).__name__}: {exc}",
        ))
        traceback.print_exc()

    # 汇总
    print("\n" + "=" * 60)
    failed = [r for r in all_results if not r.passed]
    for r in all_results:
        _print_result(r)
    print("=" * 60)
    print(f"Total: {len(all_results)}, PASS: {len(all_results) - len(failed)}, "
          f"FAIL: {len(failed)}")
    return 0 if not failed else 1


if __name__ == "__main__":
    sys.exit(main())
