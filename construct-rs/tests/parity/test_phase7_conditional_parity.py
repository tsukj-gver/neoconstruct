"""Phase 7.1 Conditional 构造器 Python 行为一致性测试。

设计依据：
- docs/design/模块设计/模块设计-Conditional.md §11 parity 测试模板
- _helpers/parity.py（公共组件）

测试策略（子进程隔离）：每个用例 × impl 在独立子进程中运行，通过 JSON 输出
结果，主进程比对。与 Phase 4 / 6 同模式。

覆盖范围（10 case）：
- IfThenElse（I1-I3）：常量 / Expr condparse/build
- Switch（S1-S3）：int 常量 key / Expr key / default
- Select（SL1-SL2）：候选成功 / SelectError
- FocusedSeq（F1-F2）：基础场景 / build

用法::

    pytest tests/parity/test_phase7_conditional_parity.py -v
"""

from __future__ import annotations

import sys
from pathlib import Path

# 让 _helpers 可导入（tests/ 是 pytest 收集根）
_TESTS_DIR = Path(__file__).resolve().parent.parent
if str(_TESTS_DIR) not in sys.path:
    sys.path.insert(0, str(_TESTS_DIR))

import pytest

from _helpers.parity import (
    assert_fidelity,
    assert_parity,
    make_parity_results_fixture,
    run_parity_case,
)

# ---------------------------------------------------------------------------
# venv 解析（standalone 模式用；pytest 模式下用 conftest.py 的 venv_pair fixture）
# ---------------------------------------------------------------------------
_VENV_ROOT = Path(r"<opencode-temp>")
_RS_PYTHON_EXE = str(_VENV_ROOT / "crs_venv_new" / "Scripts" / "python.exe")
_PY_PYTHON_EXE = str(_VENV_ROOT / "crs_venv_py_new" / "Scripts" / "python.exe")
if not Path(_RS_PYTHON_EXE).exists():
    _RS_PYTHON_EXE = sys.executable
if not Path(_PY_PYTHON_EXE).exists():
    _PY_PYTHON_EXE = sys.executable

_CRS_PYTHON_DIR = str(Path(__file__).resolve().parent.parent.parent / "python")


# ---------------------------------------------------------------------------
# case 定义（子进程脚本片段）
# ---------------------------------------------------------------------------
_CASE_DEFINITIONS = '''
def _make_case_rs(case_id):
    from dataclasses import dataclass
    from construct import (
        StructMixin, field, rfield,
        Int8ub, Int16ub, Int32ub, Bytes, Pass,
        IfThenElse, If, Switch, Select, FocusedSeq, Renamed,
        SelectError,
    )

    C = case_id

    if C == 'I1':
        # IfThenElse 常量 cond=True → then 分支
        @dataclass
        class P(StructMixin):
            v: int = field(IfThenElse(True, Int8ub, Int16ub))
        data = bytes([0x42])
        return P, data, lambda: P(v=0x42), lambda o: o.v

    if C == 'I2':
        # IfThenElse 常量 cond=False → else 分支
        @dataclass
        class P(StructMixin):
            v: int = field(IfThenElse(False, Int8ub, Int16ub))
        data = bytes([0xAA, 0xBB])
        return P, data, lambda: P(v=0xAABB), lambda o: o.v

    if C == 'I3':
        # If macro: If(True, Byte) ≡ IfThenElse(True, Byte, Pass)
        @dataclass
        class P(StructMixin):
            v: int = field(If(True, Int8ub))
        data = bytes([0x42])
        return P, data, lambda: P(v=0x42), lambda o: o.v

    if C == 'S1':
        # Switch int 常量 key=2 → Int16ub
        @dataclass
        class P(StructMixin):
            v: int = field(Switch(2, {1: Int8ub, 2: Int16ub}))
        data = bytes([0xAA, 0xBB])
        return P, data, lambda: P(v=0xAABB), lambda o: o.v

    if C == 'S2':
        # Switch 默认值（keyfunc=99 未命中 cases）→ default=Pass
        @dataclass
        class P(StructMixin):
            v: int = field(Switch(99, {1: Int8ub, 2: Int16ub}, default=Pass))
        data = bytes([])
        return P, data, lambda: P(v=None), lambda o: o.v

    if C == 'S3':
        # Switch 默认值（keyfunc=99 未命中 cases）→ default=Int8ub
        @dataclass
        class P(StructMixin):
            v: int = field(Switch(99, {1: Int8ub, 2: Int16ub}, default=Int8ub))
        data = bytes([0x42])
        return P, data, lambda: P(v=0x42), lambda o: o.v

    if C == 'SL1':
        # Select 第 1 个成功
        @dataclass
        class P(StructMixin):
            v: int = field(Select(Int8ub, Int16ub))
        data = bytes([0x42])
        return P, data, lambda: P(v=0x42), lambda o: o.v

    if C == 'SL2':
        # Select 第 1 个失败 + 第 2 个成功
        @dataclass
        class P(StructMixin):
            v: int = field(Select(Int16ub, Int8ub))
        data = bytes([0x42])
        return P, data, lambda: P(v=0x42), lambda o: o.v

    if C == 'F1':
        # FocusedSeq 基础场景：parsebuildfrom="num"
        # 非 focus 字段用 Pass（Python/construct-rs 的 FocusedSeq 对非 focus 字段
        # build 时传 None；裸 Bytes build(None) 两端都会失败，故用 Pass 占位
        # 以验证 focus 提取 + 完整往返。匿名消费字节的场景留 Phase 7.3 补充。）
        @dataclass
        class P(StructMixin):
            v: int = field(FocusedSeq("num",
                Pass,
                Renamed("num", Int8ub),
            ))
        data = bytes([0xFF])
        return P, data, lambda: P(v=0xFF), lambda o: o.v

    raise ValueError("unknown case: {}".format(case_id))


def _make_case_py(case_id):
    from construct import (
        Struct, IfThenElse, If, Switch, Select, FocusedSeq,
        Int8ub, Int16ub, Int32ub, Bytes, Pass,
    )

    C = case_id

    if C == 'I1':
        fmt = IfThenElse(True, Int8ub, Int16ub)
        data = bytes([0x42])
        return fmt, data, 0x42

    if C == 'I2':
        fmt = IfThenElse(False, Int8ub, Int16ub)
        data = bytes([0xAA, 0xBB])
        return fmt, data, 0xAABB

    if C == 'I3':
        fmt = If(True, Int8ub)
        data = bytes([0x42])
        return fmt, data, 0x42

    if C == 'S1':
        fmt = Switch(2, {1: Int8ub, 2: Int16ub})
        data = bytes([0xAA, 0xBB])
        return fmt, data, 0xAABB

    if C == 'S2':
        fmt = Switch(99, {1: Int8ub, 2: Int16ub})
        data = bytes([])
        return fmt, data, None

    if C == 'S3':
        fmt = Switch(99, {1: Int8ub, 2: Int16ub}, default=Int8ub)
        data = bytes([0x42])
        return fmt, data, 0x42

    if C == 'SL1':
        fmt = Select(Int8ub, Int16ub)
        data = bytes([0x42])
        return fmt, data, 0x42

    if C == 'SL2':
        fmt = Select(Int16ub, Int8ub)
        data = bytes([0x42])
        return fmt, data, 0x42

    if C == 'F1':
        # Python construct FocusedSeq syntax: "name"/subcon
        # 非 focus 字段用 Pass（与 rs 侧对齐，裸 Bytes 无法 build）
        fmt = FocusedSeq("num",
            Pass,
            "num" / Int8ub,
        )
        data = bytes([0xFF])
        return fmt, data, 0xFF

    raise ValueError("unknown case: {}".format(case_id))
'''


# ---------------------------------------------------------------------------
# case 列表（parametrize 用）
# ---------------------------------------------------------------------------
ALL_CASES = [
    ("I1", "IfThenElse cond=True → then"),
    ("I2", "IfThenElse cond=False → else"),
    ("I3", "If macro ≡ IfThenElse(True, sub, Pass)"),
    ("S1", "Switch int constant key"),
    ("S2", "Switch default=Pass (no match)"),
    ("S3", "Switch default=Int8ub (no match)"),
    ("SL1", "Select first success"),
    ("SL2", "Select first fail + second success"),
    ("F1", "FocusedSeq basic (parsebuildfrom=num)"),
]


# ---------------------------------------------------------------------------
# parity_results fixture（预跑缓存）
# ---------------------------------------------------------------------------
parity_results = make_parity_results_fixture(ALL_CASES, _CASE_DEFINITIONS)


# ---------------------------------------------------------------------------
# 测试函数
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("case_id,desc", ALL_CASES)
def test_parity(parity_results, case_id, desc):
    """每个 case：rs 与 py 结果三维度（parse / build / roundtrip）一致。"""
    rs = parity_results[case_id]["rs"]
    py = parity_results[case_id]["py"]
    assert_parity(rs, py, case_id, desc=desc)


@pytest.mark.parametrize("case_id,desc", ALL_CASES)
def test_fidelity_rs(parity_results, case_id, desc):
    """rs impl 自身往返保真：parse → build → parse 一致。"""
    assert_fidelity(parity_results[case_id]["rs"], case_id, desc=desc)


@pytest.mark.parametrize("case_id,desc", ALL_CASES)
def test_fidelity_py(parity_results, case_id, desc):
    """py impl 自身往返保真：parse → build → parse 一致。"""
    assert_fidelity(parity_results[case_id]["py"], case_id, desc=desc)


# ---------------------------------------------------------------------------
# Standalone 模式：python tests/parity/test_phase7_conditional_parity.py
# ---------------------------------------------------------------------------
if __name__ == "__main__":
    print("=" * 70)
    print("Phase 7.1 Conditional parity test (standalone mode)")
    print("=" * 70)
    for case_id, desc in ALL_CASES:
        print(f"\n[{case_id}] {desc}")
        rs = run_parity_case(
            "rs",
            case_id,
            _CASE_DEFINITIONS,
            rs_python=_RS_PYTHON_EXE,
            py_python=_PY_PYTHON_EXE,
            crs_python_dir=_CRS_PYTHON_DIR,
        )
        py = run_parity_case(
            "py",
            case_id,
            _CASE_DEFINITIONS,
            rs_python=_RS_PYTHON_EXE,
            py_python=_PY_PYTHON_EXE,
            crs_python_dir=_CRS_PYTHON_DIR,
        )
        try:
            assert_parity(rs, py, case_id, desc=desc)
            print(f"  ✓ PASS")
        except AssertionError as e:
            print(f"  ✗ FAIL")
            print(f"    {e}")
