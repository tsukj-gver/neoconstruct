"""Conditional 构造器 Python 行为一致性测试。

组件：_helpers/parity.py（公共组件）。

测试策略（子进程隔离）：每个用例 × impl 在独立子进程中运行，通过 JSON 输出
结果，主进程比对。与其他 parity 文件同模式。

覆盖范围（9 case）：
- IfThenElse（I1-I3）：常量 cond=True / 常量 cond=False / If 宏
- Switch（S1-S3）：int 常量 key / default=Pass / default=Int8ub
- Select（SL1-SL2）：第 1 候选成功 / 第 1 失败第 2 成功
- FocusedSeq（F1）：基础场景（parsebuildfrom="num"）

注：本文件 case 均为常量 cond/key；表达式 key/cond 的端到端回归用例见
``tests/integration/test_v0_1_1_regressions.py``。

用法::

    pytest tests/parity/test_conditional_parity.py -v
"""

from __future__ import annotations

import os
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
# 解析顺序与 conftest.py 一致（详见 README「测试环境变量」）：
#   环境变量 CRS_PYTHON / PC_PYTHON > 项目内 .venv / .venv-pc > sys.executable
_PROJECT_ROOT = Path(__file__).resolve().parent.parent.parent  # neoconstruct/


def _resolve_venv_python(env_var: str, venv_name: str) -> str:
    """按 fallback 链解析子进程 python.exe（兼容 Windows / POSIX venv 布局）。"""
    env = os.environ.get(env_var)
    if env and Path(env).exists():
        return env
    for rel in ("Scripts/python.exe", "bin/python"):
        candidate = _PROJECT_ROOT / venv_name / rel
        if candidate.exists():
            return str(candidate)
    return sys.executable


_RS_PYTHON_EXE = _resolve_venv_python("CRS_PYTHON", ".venv")
_PY_PYTHON_EXE = _resolve_venv_python("PC_PYTHON", ".venv-pc")

_CRS_PYTHON_DIR = str(Path(__file__).resolve().parent.parent.parent / "python")


# ---------------------------------------------------------------------------
# case 定义（子进程脚本片段）
# ---------------------------------------------------------------------------
_CASE_DEFINITIONS = '''
def _make_case_rs(case_id):
    from dataclasses import dataclass
    from neoconstruct import (
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
        # rs 侧 build 显式传值（包装器根按 Instance 分类：None ≡ 缺值；
        # Pass 分支编码忽略值，两端 build 字节一致）
        @dataclass
        class P(StructMixin):
            v: int = field(Switch(99, {1: Int8ub, 2: Int16ub}, default=Pass))
        data = bytes([])
        return P, data, lambda: P(v=0), lambda o: o.v

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
        # 非 focus 字段用 Pass（Python/neoconstruct 的 FocusedSeq 对非 focus 字段
        # build 时传 None；裸 Bytes build(None) 两端都会失败，故用 Pass 占位
        # 以验证 focus 提取 + 完整往返。）
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
# Standalone 模式：python tests/parity/test_conditional_parity.py
# ---------------------------------------------------------------------------
if __name__ == "__main__":
    print("=" * 70)
    print("Conditional parity test (standalone mode)")
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
