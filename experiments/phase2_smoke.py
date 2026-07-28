"""Phase 2 功能 smoke：Tell / Computed / Bytes(expr) 表达式系统.

设计依据：docs/design/CI冒烟门禁设计.md §4.2 + §1.1 B 类 + §6.2（子任务 1b 范围）。

覆盖范围（inventory.csv Phase 2 / 2.5 implemented）：
  - Bytes(count)（impl_phase=2 表达式路径）
  - Bytes(this.field)（字段引用长度——construct-rs 的字段引用语法）
  - Tell
  - Computed(expr)
  - Tell + Computed 组合（E3 场景）

设计原则：
  - 与 phase1_smoke.py / phase4_smoke_*.py 风格一致
  - 子进程隔离对照 Python construct 2.10.70
  - construct-rs 表达式语法：字段引用通过 _FieldDescriptor 重载运算符，
    如 ``count + flag`` 直接构建表达式树（不需要 this.count）

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
from typing import Any, Dict, List

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
            "phase2_smoke: required venv python missing:\n  "
            + "\n  ".join(missing)
            + "\n  (set CRS_VENV_PYTHON / PC_VENV_PYTHON env to override)\n"
        )
        sys.exit(1)


# ---------------------------------------------------------------------------
# Python construct 2.10.70 对照（子进程隔离）
# ---------------------------------------------------------------------------

# Python construct 用 this.field 引用字段；本子进程脚本封装通用场景。
_PC_RUNNER = """
import json
import sys
import traceback
from construct import (
    Struct, Bytes, GreedyBytes, Container, Tell, Computed, this
)
from construct import (
    Int8ub, Int16ub, Int32ub,
)


def run(scenario):
    tag = scenario["tag"]
    kind = scenario["kind"]
    if kind == "tell_computed":
        # Struct { start: Tell, count: Int8ub, end: Tell, size: Computed(end - start) }
        st = Struct(
            "start" / Tell,
            "count" / Int8ub,
            "end" / Tell,
            "size" / Computed(this.end - this.start),
        )
        data = bytes(scenario["data"])
        obj = st.parse(data)
        # 剔除 _io 等内部键
        clean = {}
        for k, v in dict(obj).items():
            if k.startswith("_"):
                continue
            if isinstance(v, bytes):
                clean[k] = list(v)
            else:
                clean[k] = v
        return {"parsed_dict": clean}
    elif kind == "bytes_const_length":
        # Struct { len_: Int8ub, data: Bytes(len_) }
        st = Struct(
            "len_" / Int8ub,
            "data" / Bytes(scenario["length"]),
        )
        data = bytes(scenario["data"])
        obj = st.parse(data)
        clean = {}
        for k, v in dict(obj).items():
            if k.startswith("_"):
                continue
            if isinstance(v, bytes):
                clean[k] = list(v)
            else:
                clean[k] = v
        return {"parsed_dict": clean}
    elif kind == "bytes_field_ref":
        # Struct { len_: Int8ub, data: Bytes(this.len_) }
        st = Struct(
            "len_" / Int8ub,
            "data" / Bytes(this.len_),
        )
        data = bytes(scenario["data"])
        obj = st.parse(data)
        clean = {}
        for k, v in dict(obj).items():
            if k.startswith("_"):
                continue
            if isinstance(v, bytes):
                clean[k] = list(v)
            else:
                clean[k] = v
        return {"parsed_dict": clean}
    elif kind == "bytes_field_ref_arith":
        # Struct { a: Int8ub, b: Int8ub, data: Bytes(this.a + this.b) }
        st = Struct(
            "a" / Int8ub,
            "b" / Int8ub,
            "data" / Bytes(this.a + this.b),
        )
        data = bytes(scenario["data"])
        obj = st.parse(data)
        clean = {}
        for k, v in dict(obj).items():
            if k.startswith("_"):
                continue
            if isinstance(v, bytes):
                clean[k] = list(v)
            else:
                clean[k] = v
        return {"parsed_dict": clean}
    elif kind == "computed_field_ref":
        # Struct { a: Int8ub, b: Int8ub, sum: Computed(this.a + this.b) }
        st = Struct(
            "a" / Int8ub,
            "b" / Int8ub,
            "sum" / Computed(this.a + this.b),
        )
        data = bytes(scenario["data"])
        obj = st.parse(data)
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
    """在 crs_venv_py_new 子进程中跑 Python construct 2.10.70。"""
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
# 结果收集器
# ---------------------------------------------------------------------------


@dataclass
class CaseResult:
    """单个 smoke 场景的结果。"""

    name: str
    passed: bool
    detail: str = ""


def _compare(name: str, expected: Any, actual: Any) -> CaseResult:
    if expected == actual:
        return CaseResult(name=name, passed=True)
    return CaseResult(
        name=name,
        passed=False,
        detail=f"expected={expected!r} actual={actual!r}",
    )


def _import_crs_construct() -> Any:
    """导入 construct-rs 包。"""
    sys.path.insert(0, str(PROJECT_ROOT / "construct-rs" / "python"))
    import construct  # type: ignore
    return construct


# ---------------------------------------------------------------------------
# 场景实现（construct-rs）
# ---------------------------------------------------------------------------


def test_bytes_const_length(crs: Any) -> List[CaseResult]:
    """Bytes(count) 定长字段（impl_phase=2 的 Bytes 表达式路径）。"""
    out: List[CaseResult] = []
    Bytes, Int8ub = crs.Bytes, crs.Int8ub
    StructMixin, field = crs.StructMixin, crs.field

    @dataclass
    class P(StructMixin):
        len_: int = field(Int8ub)
        data: bytes = field(Bytes(4))

    raw = b"\x04\xAA\xBB\xCC\xDD"
    parsed = P.parse(raw)
    out.append(_compare("Bytes(4).len_", 4, parsed.len_))
    out.append(_compare("Bytes(4).data", b"\xAA\xBB\xCC\xDD", parsed.data))

    # build
    msg = P(len_=4, data=b"\xAA\xBB\xCC\xDD")
    built = msg.build()
    out.append(_compare("Bytes(4).build", raw, built))

    # Python construct 对照
    scenario = {
        "tag": "Bytes(4)",
        "kind": "bytes_const_length",
        "length": 4,
        "data": list(raw),
    }
    pc = run_python_construct(scenario)
    if pc.get("ok"):
        pc_dict = pc["result"]["parsed_dict"]
        out.append(_compare("Bytes(4).len_ vs PC", pc_dict["len_"], parsed.len_))
        out.append(_compare(
            "Bytes(4).data vs PC",
            tuple(pc_dict["data"]),
            tuple(parsed.data),
        ))
    else:
        out.append(CaseResult(
            name="Bytes(4) vs PC",
            passed=False,
            detail=f"PC error: {pc.get('error_msg')}",
        ))
    return out


def test_bytes_field_ref(crs: Any) -> List[CaseResult]:
    """Bytes(this.len_) 字段引用长度（E2 场景：Bytes(count+flag) 简化版）。

    construct-rs 表达式：``Bytes(len_)`` 直接把 _FieldDescriptor 传给 Bytes，
    在编译期翻译为 ExprProgram。
    """
    out: List[CaseResult] = []
    Bytes, Int8ub = crs.Bytes, crs.Int8ub
    StructMixin, field = crs.StructMixin, crs.field

    @dataclass
    class P(StructMixin):
        len_: int = field(Int8ub)
        data: bytes = field(Bytes(len_))

    raw = b"\x03\xAA\xBB\xCC"
    parsed = P.parse(raw)
    out.append(_compare("Bytes(len_).len_", 3, parsed.len_))
    out.append(_compare("Bytes(len_).data", b"\xAA\xBB\xCC", parsed.data))

    # build（data 长度必须匹配 len_ 字段值；construct-rs 表达式 build 时求值）
    msg = P(len_=3, data=b"\xAA\xBB\xCC")
    built = msg.build()
    out.append(_compare("Bytes(len_).build", raw, built))

    # 长度变化场景
    msg2 = P(len_=2, data=b"\x11\x22")
    built2 = msg2.build()
    out.append(_compare("Bytes(len_).build len=2", b"\x02\x11\x22", built2))

    # Python construct 对照
    scenario = {
        "tag": "Bytes(this.len_)",
        "kind": "bytes_field_ref",
        "data": list(raw),
    }
    pc = run_python_construct(scenario)
    if pc.get("ok"):
        pc_dict = pc["result"]["parsed_dict"]
        out.append(_compare("Bytes(len_).len_ vs PC", pc_dict["len_"], parsed.len_))
        out.append(_compare(
            "Bytes(len_).data vs PC",
            tuple(pc_dict["data"]),
            tuple(parsed.data),
        ))
    else:
        out.append(CaseResult(
            name="Bytes(len_) vs PC",
            passed=False,
            detail=f"PC error: {pc.get('error_msg')}",
        ))
    return out


def test_bytes_field_ref_arith(crs: Any) -> List[CaseResult]:
    """Bytes(a + b) 字段引用 + 算术（E2 场景：Bytes(count+flag)）。

    construct-rs 表达式：``Bytes(a + b)`` 编译为 ExprProgram
    [getint(a), getint(b), add]。
    """
    out: List[CaseResult] = []
    Bytes, Int8ub = crs.Bytes, crs.Int8ub
    StructMixin, field = crs.StructMixin, crs.field

    @dataclass
    class P(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)
        data: bytes = field(Bytes(a + b))

    raw = b"\x02\x03\xAA\xBB\xCC\xDD\xEE"
    parsed = P.parse(raw)
    out.append(_compare("Bytes(a+b).a", 2, parsed.a))
    out.append(_compare("Bytes(a+b).b", 3, parsed.b))
    out.append(_compare("Bytes(a+b).data", b"\xAA\xBB\xCC\xDD\xEE", parsed.data))

    # Python construct 对照
    scenario = {
        "tag": "Bytes(a+b)",
        "kind": "bytes_field_ref_arith",
        "data": list(raw),
    }
    pc = run_python_construct(scenario)
    if pc.get("ok"):
        pc_dict = pc["result"]["parsed_dict"]
        out.append(_compare(
            "Bytes(a+b).data vs PC",
            tuple(pc_dict["data"]),
            tuple(parsed.data),
        ))
    else:
        out.append(CaseResult(
            name="Bytes(a+b) vs PC",
            passed=False,
            detail=f"PC error: {pc.get('error_msg')}",
        ))
    return out


def test_computed_field_ref(crs: Any) -> List[CaseResult]:
    """Computed(a + b) 字段引用求和。

    construct-rs 表达式：``Computed(a + b)`` 编译为 ExprProgram。
    """
    out: List[CaseResult] = []
    Int8ub = crs.Int8ub
    StructMixin, field, rfield, Tell, Computed = (
        crs.StructMixin, crs.field, crs.rfield, crs.Tell, crs.Computed
    )

    @dataclass
    class P(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)
        sum_: int = rfield(Computed(a + b))

    raw = b"\x05\x07"
    parsed = P.parse(raw)
    out.append(_compare("Computed(a+b).a", 5, parsed.a))
    out.append(_compare("Computed(a+b).b", 7, parsed.b))
    out.append(_compare("Computed(a+b).sum_", 12, parsed.sum_))

    # Python construct 对照
    scenario = {
        "tag": "Computed(a+b)",
        "kind": "computed_field_ref",
        "data": list(raw),
    }
    pc = run_python_construct(scenario)
    if pc.get("ok"):
        pc_dict = pc["result"]["parsed_dict"]
        out.append(_compare(
            "Computed(a+b).sum_ vs PC",
            pc_dict["sum"],
            parsed.sum_,
        ))
    else:
        out.append(CaseResult(
            name="Computed(a+b) vs PC",
            passed=False,
            detail=f"PC error: {pc.get('error_msg')}",
        ))
    return out


def test_tell_computed(crs: Any) -> List[CaseResult]:
    """E3 场景：Tell + Computed 组合（记录流位置 + 计算段长度）。

    construct-rs 表达式：
      - Tell()：rfield(Tell())，build 时取流位置
      - Computed(end - start)：编译为 ExprProgram [getint(start), getint(end), sub]
    """
    out: List[CaseResult] = []
    Int8ub = crs.Int8ub
    StructMixin, field, rfield, Tell, Computed = (
        crs.StructMixin, crs.field, crs.rfield, crs.Tell, crs.Computed
    )

    @dataclass
    class P(StructMixin):
        start: int = rfield(Tell())
        count: int = field(Int8ub)
        end: int = rfield(Tell())
        size: int = rfield(Computed(end - start))

    raw = b"\xAB"
    parsed = P.parse(raw)
    out.append(_compare("Tell/Computed.start", 0, parsed.start))
    out.append(_compare("Tell/Computed.count", 0xAB, parsed.count))
    out.append(_compare("Tell/Computed.end", 1, parsed.end))
    out.append(_compare("Tell/Computed.size", 1, parsed.size))

    # build（start/size 是 RO，build 时自动计算）
    msg = P(count=0xAB)
    built = msg.build()
    out.append(_compare("Tell/Computed.build", raw, built))

    # Python construct 对照
    scenario = {
        "tag": "Tell+Computed",
        "kind": "tell_computed",
        "data": list(raw),
    }
    pc = run_python_construct(scenario)
    if pc.get("ok"):
        pc_dict = pc["result"]["parsed_dict"]
        out.append(_compare("Tell/Computed.start vs PC", pc_dict["start"], parsed.start))
        out.append(_compare("Tell/Computed.count vs PC", pc_dict["count"], parsed.count))
        out.append(_compare("Tell/Computed.end vs PC", pc_dict["end"], parsed.end))
        out.append(_compare("Tell/Computed.size vs PC", pc_dict["size"], parsed.size))
    else:
        out.append(CaseResult(
            name="Tell+Computed vs PC",
            passed=False,
            detail=f"PC error: {pc.get('error_msg')}",
        ))
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
    print("Phase 2 smoke: Tell / Computed / Bytes(expr)")
    print(f"  CRS venv python : {CRS_PYTHON}")
    print(f"  PC  venv python : {PC_PYTHON}")
    print("=" * 60)

    if sys.executable != str(CRS_PYTHON):
        print(
            f"[WARN] current python ({sys.executable}) != CRS_VENV_PYTHON; "
            "construct-rs must be importable from this process"
        )

    crs = _import_crs_construct()
    all_results: List[CaseResult] = []

    print("\n[Section 1] Bytes(count) 定长")
    try:
        all_results.extend(test_bytes_const_length(crs))
    except Exception as exc:  # noqa: BLE001
        all_results.append(CaseResult(
            name="Bytes(count) (group)", passed=False,
            detail=f"exception: {type(exc).__name__}: {exc}",
        ))
        traceback.print_exc()

    print("\n[Section 2] Bytes(this.len_) 字段引用长度")
    try:
        all_results.extend(test_bytes_field_ref(crs))
    except Exception as exc:  # noqa: BLE001
        all_results.append(CaseResult(
            name="Bytes(this.len_) (group)", passed=False,
            detail=f"exception: {type(exc).__name__}: {exc}",
        ))
        traceback.print_exc()

    print("\n[Section 3] Bytes(a+b) 字段引用 + 算术（E2 场景）")
    try:
        all_results.extend(test_bytes_field_ref_arith(crs))
    except Exception as exc:  # noqa: BLE001
        all_results.append(CaseResult(
            name="Bytes(a+b) (group)", passed=False,
            detail=f"exception: {type(exc).__name__}: {exc}",
        ))
        traceback.print_exc()

    print("\n[Section 4] Computed(a+b) 字段引用求和")
    try:
        all_results.extend(test_computed_field_ref(crs))
    except Exception as exc:  # noqa: BLE001
        all_results.append(CaseResult(
            name="Computed(a+b) (group)", passed=False,
            detail=f"exception: {type(exc).__name__}: {exc}",
        ))
        traceback.print_exc()

    print("\n[Section 5] Tell + Computed (E3 场景)")
    try:
        all_results.extend(test_tell_computed(crs))
    except Exception as exc:  # noqa: BLE001
        all_results.append(CaseResult(
            name="Tell+Computed (group)", passed=False,
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
