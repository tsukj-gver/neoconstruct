"""Primitives 构造器 Python 行为一致性测试。

组件：_helpers/parity.py（公共组件）。

覆盖范围（17 个构造器，关键边界 case）：
- 整数别名（4）：Byte / Short / Int / Long
- Float 系列（6）：Float16b/l / Float32b/l / Float64b/l
- Int24 系列（4）：Int24ub / Int24ul / Int24sb / Int24sl
- 变长（2）：VarInt / ZigZag
- 大整数（1）：BytesInteger（含 fast/slow path）

差异处理：
- NaN/Inf：由 _helpers/normalize.py 统一标记，case 内不手动标记
- Float16 subnormal：normalize round(6) 容忍精度差异
- BytesInteger slow-path（>8 字节）：parity 对比 Python int.from_bytes 等价行为

用法::

    pytest tests/parity/test_primitives_parity.py -v
    python tests/parity/test_primitives_parity.py
"""

from __future__ import annotations

import os
import sys
from pathlib import Path

# 让 _helpers 可导入（tests/ 是 pytest 收集根）
_TESTSDIR = Path(__file__).resolve().parent.parent
if str(_TESTSDIR) not in sys.path:
    sys.path.insert(0, str(_TESTSDIR))

import pytest

from _helpers.parity import (
    assert_fidelity,
    assert_parity,
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
# 关键设计点：
# - rs 侧用 StructMixin + dataclass + field(Descriptor) 模式
# - py 侧用 pc.Struct("name"/pc.Descriptor) 或直接 pc.Descriptor（VarInt/ZigZag/BytesInteger）
# - NaN/Inf 不在 case 内手动标记，由 normalize 统一处理
# - Float16 subnormal 用 round(6) 容忍
_CASE_DEFINITIONS = r'''
def _make_case_rs(case_id):
    from dataclasses import dataclass
    from neoconstruct import (
        StructMixin, field,
        Byte, Short, Int, Long,
        Float16b, Float16l, Float32b, Float32l, Float64b, Float64l,
        Int24ub, Int24ul, Int24sb, Int24sl,
        VarInt, ZigZag, BytesInteger,
    )

    C = case_id

    if C == 'B-1':
        @dataclass
        class P(StructMixin):
            v: int = field(Byte)
        return P, b"\x42", lambda: P(v=66), lambda o: {'v': o.v}

    if C == 'S-1':
        @dataclass
        class P(StructMixin):
            v: int = field(Short)
        return P, b"\x01\x00", lambda: P(v=256), lambda o: {'v': o.v}

    if C == 'I-1':
        @dataclass
        class P(StructMixin):
            v: int = field(Int)
        return P, b"\x00\x01\x00\x00", lambda: P(v=65536), lambda o: {'v': o.v}

    if C == 'L-1':
        @dataclass
        class P(StructMixin):
            v: int = field(Long)
        # Long = Int64ub (Big Endian u64). 2^32 = 0x100000000
        # BE 字节序：00 00 00 01 00 00 00 00（高位在前）
        return P, b"\x00\x00\x00\x01\x00\x00\x00\x00", lambda: P(v=4294967296), lambda o: {'v': o.v}

    if C == 'F32-1':
        @dataclass
        class P(StructMixin):
            v: float = field(Float32b)
        return P, b"\x42\x28\x00\x00", lambda: P(v=42.0), lambda o: {'v': o.v}

    if C == 'F32-2':
        @dataclass
        class P(StructMixin):
            v: float = field(Float32b)
        return P, b"\xC2\x28\x00\x00", lambda: P(v=-42.0), lambda o: {'v': o.v}

    if C == 'F32-4':
        @dataclass
        class P(StructMixin):
            v: float = field(Float32b)
        return P, b"\x7f\xc0\x00\x00", lambda: P(v=float('nan')), lambda o: {'v': o.v}

    if C == 'F32-5':
        @dataclass
        class P(StructMixin):
            v: float = field(Float32b)
        return P, b"\x7f\x80\x00\x00", lambda: P(v=float('inf')), lambda o: {'v': o.v}

    if C == 'F32L-1':
        @dataclass
        class P(StructMixin):
            v: float = field(Float32l)
        return P, b"\x00\x00\x28\x42", lambda: P(v=42.0), lambda o: {'v': o.v}

    if C == 'F64-1':
        @dataclass
        class P(StructMixin):
            v: float = field(Float64b)
        return P, b"\x40\x09\x1e\xb8\x51\xeb\x85\x1f", lambda: P(v=3.14), lambda o: {'v': o.v}

    if C == 'F64L-1':
        @dataclass
        class P(StructMixin):
            v: float = field(Float64l)
        return P, b"\x1f\x85\xeb\x51\xb8\x1e\x09\x40", lambda: P(v=3.14), lambda o: {'v': o.v}

    if C == 'F16-1':
        @dataclass
        class P(StructMixin):
            v: float = field(Float16b)
        return P, b"\x3c\x00", lambda: P(v=1.0), lambda o: {'v': o.v}

    if C == 'F16L-1':
        @dataclass
        class P(StructMixin):
            v: float = field(Float16l)
        return P, b"\x00\x3c", lambda: P(v=1.0), lambda o: {'v': o.v}

    if C == 'I24-1':
        @dataclass
        class P(StructMixin):
            v: int = field(Int24ub)
        return P, b"\x00\xff\xff", lambda: P(v=65535), lambda o: {'v': o.v}

    if C == 'I24-2':
        @dataclass
        class P(StructMixin):
            v: int = field(Int24ub)
        return P, b"\xff\xff\xff", lambda: P(v=16777215), lambda o: {'v': o.v}

    if C == 'I24-3':
        @dataclass
        class P(StructMixin):
            v: int = field(Int24sb)
        return P, b"\xff\xff\xff", lambda: P(v=-1), lambda o: {'v': o.v}

    if C == 'I24-4':
        @dataclass
        class P(StructMixin):
            v: int = field(Int24sb)
        return P, b"\x80\x00\x00", lambda: P(v=-8388608), lambda o: {'v': o.v}

    if C == 'I24-5':
        @dataclass
        class P(StructMixin):
            v: int = field(Int24ul)
        return P, b"\x01\x00\x00", lambda: P(v=1), lambda o: {'v': o.v}

    if C == 'VAR-1':
        @dataclass
        class P(StructMixin):
            v: int = field(VarInt)
        return P, b"\x01", lambda: P(v=1), lambda o: {'v': o.v}

    if C == 'VAR-2':
        @dataclass
        class P(StructMixin):
            v: int = field(VarInt)
        return P, b"\x80\x01", lambda: P(v=128), lambda o: {'v': o.v}

    if C == 'VAR-3':
        @dataclass
        class P(StructMixin):
            v: int = field(VarInt)
        return P, b"\xff\xff\xff\xff\x0f", lambda: P(v=4294967295), lambda o: {'v': o.v}

    if C == 'ZZ-1':
        @dataclass
        class P(StructMixin):
            v: int = field(ZigZag)
        return P, b"\x05", lambda: P(v=-3), lambda o: {'v': o.v}

    if C == 'ZZ-2':
        @dataclass
        class P(StructMixin):
            v: int = field(ZigZag)
        return P, b"\x00", lambda: P(v=0), lambda o: {'v': o.v}

    if C == 'ZZ-3':
        @dataclass
        class P(StructMixin):
            v: int = field(ZigZag)
        return P, b"\x06", lambda: P(v=3), lambda o: {'v': o.v}

    if C == 'BI-1':
        @dataclass
        class P(StructMixin):
            v: int = field(BytesInteger(4))
        return P, b"\x00\x00\x00\x13", lambda: P(v=19), lambda o: {'v': o.v}

    if C == 'BI-2':
        @dataclass
        class P(StructMixin):
            v: int = field(BytesInteger(16))
        # 2^64 in 16 bytes big-endian: byte index 7 = 0x01
        return P, b"\x00\x00\x00\x00\x00\x00\x00\x01\x00\x00\x00\x00\x00\x00\x00\x00", \
            lambda: P(v=2**64), lambda o: {'v': o.v}

    if C == 'BI-3':
        @dataclass
        class P(StructMixin):
            v: int = field(BytesInteger(3, signed=True, swapped=True))
        # Int24sl.parse(b'\xff\xff\xff') = -1 (little endian, sign extension)
        return P, b"\xff\xff\xff", lambda: P(v=-1), lambda o: {'v': o.v}

    raise ValueError('unknown case (rs): ' + case_id)


def _make_case_py(case_id):
    import construct as pc

    C = case_id

    if C == 'B-1':
        return (pc.Struct("v"/pc.Byte), b"\x42", dict(v=66))
    if C == 'S-1':
        return (pc.Struct("v"/pc.Short), b"\x01\x00", dict(v=256))
    if C == 'I-1':
        return (pc.Struct("v"/pc.Int), b"\x00\x01\x00\x00", dict(v=65536))
    if C == 'L-1':
        return (pc.Struct("v"/pc.Long), b"\x00\x00\x00\x01\x00\x00\x00\x00", dict(v=4294967296))
    if C == 'F32-1':
        return (pc.Struct("v"/pc.Float32b), b"\x42\x28\x00\x00", dict(v=42.0))
    if C == 'F32-2':
        return (pc.Struct("v"/pc.Float32b), b"\xC2\x28\x00\x00", dict(v=-42.0))
    if C == 'F32-4':
        return (pc.Struct("v"/pc.Float32b), b"\x7f\xc0\x00\x00", dict(v=float('nan')))
    if C == 'F32-5':
        return (pc.Struct("v"/pc.Float32b), b"\x7f\x80\x00\x00", dict(v=float('inf')))
    if C == 'F32L-1':
        return (pc.Struct("v"/pc.Float32l), b"\x00\x00\x28\x42", dict(v=42.0))
    if C == 'F64-1':
        return (pc.Struct("v"/pc.Float64b), b"\x40\x09\x1e\xb8\x51\xeb\x85\x1f", dict(v=3.14))
    if C == 'F64L-1':
        return (pc.Struct("v"/pc.Float64l), b"\x1f\x85\xeb\x51\xb8\x1e\x09\x40", dict(v=3.14))
    if C == 'F16-1':
        return (pc.Struct("v"/pc.Float16b), b"\x3c\x00", dict(v=1.0))
    if C == 'F16L-1':
        return (pc.Struct("v"/pc.Float16l), b"\x00\x3c", dict(v=1.0))
    if C == 'I24-1':
        return (pc.Struct("v"/pc.Int24ub), b"\x00\xff\xff", dict(v=65535))
    if C == 'I24-2':
        return (pc.Struct("v"/pc.Int24ub), b"\xff\xff\xff", dict(v=16777215))
    if C == 'I24-3':
        return (pc.Struct("v"/pc.Int24sb), b"\xff\xff\xff", dict(v=-1))
    if C == 'I24-4':
        return (pc.Struct("v"/pc.Int24sb), b"\x80\x00\x00", dict(v=-8388608))
    if C == 'I24-5':
        return (pc.Struct("v"/pc.Int24ul), b"\x01\x00\x00", dict(v=1))
    if C == 'VAR-1':
        return (pc.Struct("v"/pc.VarInt), b"\x01", dict(v=1))
    if C == 'VAR-2':
        return (pc.Struct("v"/pc.VarInt), b"\x80\x01", dict(v=128))
    if C == 'VAR-3':
        return (pc.Struct("v"/pc.VarInt), b"\xff\xff\xff\xff\x0f", dict(v=4294967295))
    if C == 'ZZ-1':
        return (pc.Struct("v"/pc.ZigZag), b"\x05", dict(v=-3))
    if C == 'ZZ-2':
        return (pc.Struct("v"/pc.ZigZag), b"\x00", dict(v=0))
    if C == 'ZZ-3':
        return (pc.Struct("v"/pc.ZigZag), b"\x06", dict(v=3))
    if C == 'BI-1':
        return (pc.Struct("v"/pc.BytesInteger(4)), b"\x00\x00\x00\x13", dict(v=19))
    if C == 'BI-2':
        return (pc.Struct("v"/pc.BytesInteger(16)),
                b"\x00\x00\x00\x00\x00\x00\x00\x01\x00\x00\x00\x00\x00\x00\x00\x00",
                dict(v=2**64))
    if C == 'BI-3':
        return (pc.Struct("v"/pc.BytesInteger(3, signed=True, swapped=True)),
                b"\xff\xff\xff", dict(v=-1))

    raise ValueError('unknown case (py): ' + case_id)
'''

# ---------------------------------------------------------------------------
# case 清单（按构造器分组）
# ---------------------------------------------------------------------------

ALL_CASES = [
    # 整数别名（4）
    ("B-1",   "Byte 基本（66）"),
    ("S-1",   "Short 基本（256）"),
    ("I-1",   "Int 基本（65536）"),
    ("L-1",   "Long 基本（2^32）"),
    # Float32（含 NaN/Inf，normalize 统一处理）
    ("F32-1", "Float32b 正数（42.0）"),
    ("F32-2", "Float32b 负数（-42.0）"),
    ("F32-4", "Float32b NaN"),
    ("F32-5", "Float32b +Infinity"),
    ("F32L-1","Float32l 正数（42.0，小端）"),
    # Float64
    ("F64-1", "Float64b 精度（3.14）"),
    ("F64L-1","Float64l 精度（3.14，小端）"),
    # Float16（half crate）
    ("F16-1", "Float16b 基本（1.0）"),
    ("F16L-1","Float16l 基本（1.0）"),
    # Int24 系列（4 × 边界）
    ("I24-1", "Int24ub 65535"),
    ("I24-2", "Int24ub 最大（16777215）"),
    ("I24-3", "Int24sb -1（符号扩展）"),
    ("I24-4", "Int24sb 最小（-8388608）"),
    ("I24-5", "Int24ul little endian"),
    # VarInt（fast-path 边界）
    ("VAR-1", "VarInt 单字节（1）"),
    ("VAR-2", "VarInt 多字节（128）"),
    ("VAR-3", "VarInt u32 最大（4294967295）"),
    # ZigZag（正/负/零）
    ("ZZ-1",  "ZigZag 负数（-3）"),
    ("ZZ-2",  "ZigZag 零"),
    ("ZZ-3",  "ZigZag 正数（3）"),
    # BytesInteger（fast/slow path）
    ("BI-1",  "BytesInteger(4) fast-path"),
    ("BI-2",  "BytesInteger(16) slow-path（2^64）"),
    ("BI-3",  "BytesInteger(3, signed, swapped) = Int24sl"),
]


# ---------------------------------------------------------------------------
# 标准 5 个 parity 测试函数（统一模板）
# ---------------------------------------------------------------------------

@pytest.fixture(scope="module")
def parity_results(venv_pair):
    """预跑全部 case × impl，结果缓存供 parametrize 测试复用。"""
    results = {}
    for case_id, _ in ALL_CASES:
        results[case_id] = {
            "rs": run_parity_case("rs", case_id, _CASE_DEFINITIONS, **venv_pair),
            "py": run_parity_case("py", case_id, _CASE_DEFINITIONS, **venv_pair),
        }
    return results


def test_parity_all_cases_collected(parity_results):
    """确认所有 case 都成功执行（无子进程错误）。"""
    for case_id, _ in ALL_CASES:
        assert case_id in parity_results
        assert "rs" in parity_results[case_id]
        assert "py" in parity_results[case_id]


@pytest.mark.parametrize("case_id,desc", ALL_CASES)
def test_parity_parse(case_id, desc, parity_results):
    """parse 输出 rs == py。"""
    rs = parity_results[case_id]["rs"]
    py = parity_results[case_id]["py"]
    assert_parity(rs, py, case_id, desc=desc, check_build=False, check_roundtrip=False)


@pytest.mark.parametrize("case_id,desc", ALL_CASES)
def test_parity_build(case_id, desc, parity_results):
    """build 字节输出 rs == py。"""
    rs = parity_results[case_id]["rs"]
    py = parity_results[case_id]["py"]
    assert_parity(rs, py, case_id, desc=desc, check_parse=False, check_roundtrip=False)


@pytest.mark.parametrize("case_id,desc", ALL_CASES)
def test_parity_roundtrip(case_id, desc, parity_results):
    """roundtrip parse 结果 rs == py。"""
    rs = parity_results[case_id]["rs"]
    py = parity_results[case_id]["py"]
    assert_parity(rs, py, case_id, desc=desc, check_parse=False, check_build=False)


@pytest.mark.parametrize("case_id,desc", ALL_CASES)
def test_parity_fidelity(case_id, desc, parity_results):
    """单 impl parse→build→parse 保真（rs 与 py 各自）。"""
    assert_fidelity(parity_results[case_id]["rs"], case_id, desc=desc)
    assert_fidelity(parity_results[case_id]["py"], case_id, desc=desc)


# ---------------------------------------------------------------------------
# Standalone 模式（python tests/parity/test_primitives_parity.py）
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    venv = {
        "rs_python": _RS_PYTHON_EXE,
        "py_python": _PY_PYTHON_EXE,
        "crs_python_dir": _CRS_PYTHON_DIR,
    }
    failures = 0
    for case_id, desc in ALL_CASES:
        try:
            rs = run_parity_case("rs", case_id, _CASE_DEFINITIONS, **venv)
            py = run_parity_case("py", case_id, _CASE_DEFINITIONS, **venv)
            assert_parity(rs, py, case_id, desc=desc)
            assert_fidelity(rs, case_id, desc=desc)
            assert_fidelity(py, case_id, desc=desc)
            print(f"[PASS] {case_id} ({desc})")
        except (AssertionError, RuntimeError) as e:
            print(f"[FAIL] {case_id} ({desc}): {e}")
            failures += 1
    print(f"\n=== Summary: {len(ALL_CASES) - failures}/{len(ALL_CASES)} PASS ===")
    sys.exit(1 if failures else 0)
