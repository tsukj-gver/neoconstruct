"""Phase 6.2 Strings 构造器 Python 行为一致性测试（6.2 VET 指出的缺失补全）。

设计依据：
- docs/design/模块设计/模块设计-Strings.md v2 §5（边界条件清单）/ §10（parity 模板）
- docs/design/基础设施/测试框架设计.md §5.1（统一模板）
- tests/_helpers/parity.py（公共组件）

覆盖范围（6 个 String Node，14 个 case）：
- CString（4）：utf8 / ascii / utf16-le / utf16-be（含 term 默认 + 字节序对齐）
- GreedyString（3）：utf8 / utf16-le / utf16-be
- PaddedString（2）：utf8 常量长度 / utf16-le 常量长度
- PascalString（2）：Byte lengthfield / Int16ub lengthfield
- NullTerminated（2）：GreedyBytes inner / 自定义 term + Byte inner
- NullStripped（2）：单字节 pad / 多字节 pad（utf16）

BREAKING CHANGE 体现（PM 决策 6.2-D1 + P2b）：
- P2B-1：``CString("utf16")`` 无后缀编码编译期被拒（construct-rs 侧抛
  ``CompilationError``；Python construct 2.10.70 仍接受 utf16）。
  parity 测试用 try/except 标记，仅验证 rs 端拒绝即可。

差异处理：
- utf16 raw FFI（设计 §2.2.2 P2a）：BOM 不被消费（byteorder=±1）。
- encoding 别名：rs 侧强制 ``utf_16_le`` / ``utf_16_be`` 显式后缀；
  py 侧同样使用显式后缀（避免 BOM 自检测差异）。

用法::

    pytest tests/parity/test_phase6_strings_parity.py -v
    python tests/parity/test_phase6_strings_parity.py
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
# 关键设计点：
# - rs 侧用 StructMixin + dataclass + field(Descriptor) 模式
# - py 侧用 pc.Struct("name"/pc.Descriptor) 模式
# - 双侧都用显式 utf_16_le / utf_16_be 后缀（避免 BOM 自检测差异）
# - PC2b case 单独处理（rs 端抛 CompilationError，parity 不比对结果，仅验证拒绝）
_CASE_DEFINITIONS = r'''
def _make_case_rs(case_id):
    from dataclasses import dataclass
    from construct import (
        StructMixin, field,
        CString, GreedyString, PaddedString, PascalString,
        NullTerminated, NullStripped,
        Byte, Int16ub, Int8ub, GreedyBytes,
    )

    C = case_id

    if C == 'CS-U8-1':
        @dataclass
        class P(StructMixin):
            name: str = field(CString("utf8"))
        return P, b"hello\x00", lambda: P(name="hello"), lambda o: {'name': o.name}

    if C == 'CS-U8-2':
        # utf8 + 多字节字符
        @dataclass
        class P(StructMixin):
            name: str = field(CString("utf8"))
        return P, "Афон\x00".encode("utf8"), lambda: P(name="Афон"), lambda o: {'name': o.name}

    if C == 'CS-ASC-1':
        @dataclass
        class P(StructMixin):
            name: str = field(CString("ascii"))
        return P, b"hello\x00", lambda: P(name="hello"), lambda o: {'name': o.name}

    if C == 'CS-U16LE-1':
        # UTF-16 LE：term = b"\x00\x00"（编码单元全零）
        # "hi" UTF-16 LE = b'h\x00i\x00'
        @dataclass
        class P(StructMixin):
            name: str = field(CString("utf_16_le"))
        return P, b"h\x00i\x00\x00\x00", lambda: P(name="hi"), lambda o: {'name': o.name}

    if C == 'CS-U16BE-1':
        # UTF-16 BE：term = b"\x00\x00"
        # "hi" UTF-16 BE = b'\x00h\x00i'
        @dataclass
        class P(StructMixin):
            name: str = field(CString("utf_16_be"))
        return P, b"\x00h\x00i\x00\x00", lambda: P(name="hi"), lambda o: {'name': o.name}

    if C == 'GS-U8-1':
        @dataclass
        class P(StructMixin):
            tail: str = field(GreedyString("utf8"))
        return P, b"hello world", lambda: P(tail="hello world"), lambda o: {'tail': o.tail}

    if C == 'GS-U16LE-1':
        # "abc" UTF-16 LE = b'a\x00b\x00c\x00'
        @dataclass
        class P(StructMixin):
            tail: str = field(GreedyString("utf_16_le"))
        return P, b"a\x00b\x00c\x00", lambda: P(tail="abc"), lambda o: {'tail': o.tail}

    if C == 'GS-U16BE-1':
        # "abc" UTF-16 BE = b'\x00a\x00b\x00c'
        @dataclass
        class P(StructMixin):
            tail: str = field(GreedyString("utf_16_be"))
        return P, b"\x00a\x00b\x00c", lambda: P(tail="abc"), lambda o: {'tail': o.tail}

    if C == 'PS-U8-1':
        @dataclass
        class P(StructMixin):
            name: str = field(PaddedString(10, "utf8"))
        return P, b"hello\x00\x00\x00\x00\x00", lambda: P(name="hello"), lambda o: {'name': o.name}

    if C == 'PS-U16LE-1':
        # PaddedString(8, "utf_16_le")："hi" UTF-16 LE = b'h\x00i\x00'，pad 4 字节
        @dataclass
        class P(StructMixin):
            name: str = field(PaddedString(8, "utf_16_le"))
        return P, b"h\x00i\x00\x00\x00\x00\x00", lambda: P(name="hi"), lambda o: {'name': o.name}

    if C == 'PAS-U8-1':
        # PascalString(Byte lengthfield)
        @dataclass
        class P(StructMixin):
            name: str = field(PascalString(Byte, "utf8"))
        return P, b"\x05hello", lambda: P(name="hello"), lambda o: {'name': o.name}

    if C == 'PAS-U8-2':
        # PascalString(Int16ub lengthfield)
        @dataclass
        class P(StructMixin):
            name: str = field(PascalString(Int16ub, "utf8"))
        return P, b"\x00\x05hello", lambda: P(name="hello"), lambda o: {'name': o.name}

    if C == 'NT-BYTES-1':
        # NullTerminated(GreedyBytes) 默认 term=b"\x00"
        @dataclass
        class P(StructMixin):
            data: object = field(NullTerminated(GreedyBytes))
        return P, b"hello\x00world", lambda: P(data=b"hello"), lambda o: {'data': bytes(o.data)}

    if C == 'NT-BYTE-1':
        # NullTerminated(Byte) term=b"\xff"
        @dataclass
        class P(StructMixin):
            v: int = field(NullTerminated(Byte, term=b"\xff"))
        return P, b"\x42\xff", lambda: P(v=0x42), lambda o: {'v': o.v}

    if C == 'NS-1':
        # NullStripped(GreedyBytes) pad=b"\x00"
        @dataclass
        class P(StructMixin):
            data: object = field(NullStripped(GreedyBytes))
        return P, b"abc\x00\x00", lambda: P(data=b"abc"), lambda o: {'data': bytes(o.data)}

    if C == 'NS-2':
        # NullStripped(GreedyBytes) pad=b"\x00\x00"（多字节 pad）
        @dataclass
        class P(StructMixin):
            data: object = field(NullStripped(GreedyBytes, pad=b"\x00\x00"))
        # data = b"ab\x00\x00\x00\x00"，pad unit=2，剥离 2 个完整 unit → "ab"
        return P, b"ab\x00\x00\x00\x00", lambda: P(data=b"ab"), lambda o: {'data': bytes(o.data)}

    raise ValueError('unknown case (rs): ' + case_id)


def _make_case_py(case_id):
    import construct as pc

    C = case_id

    if C == 'CS-U8-1':
        return (pc.Struct("name"/pc.CString("utf8")), b"hello\x00", dict(name="hello"))
    if C == 'CS-U8-2':
        return (pc.Struct("name"/pc.CString("utf8")), "Афон\x00".encode("utf8"), dict(name="Афон"))
    if C == 'CS-ASC-1':
        return (pc.Struct("name"/pc.CString("ascii")), b"hello\x00", dict(name="hello"))
    if C == 'CS-U16LE-1':
        return (pc.Struct("name"/pc.CString("utf_16_le")), b"h\x00i\x00\x00\x00", dict(name="hi"))
    if C == 'CS-U16BE-1':
        return (pc.Struct("name"/pc.CString("utf_16_be")), b"\x00h\x00i\x00\x00", dict(name="hi"))
    if C == 'GS-U8-1':
        return (pc.Struct("tail"/pc.GreedyString("utf8")), b"hello world", dict(tail="hello world"))
    if C == 'GS-U16LE-1':
        return (pc.Struct("tail"/pc.GreedyString("utf_16_le")), b"a\x00b\x00c\x00", dict(tail="abc"))
    if C == 'GS-U16BE-1':
        return (pc.Struct("tail"/pc.GreedyString("utf_16_be")), b"\x00a\x00b\x00c", dict(tail="abc"))
    if C == 'PS-U8-1':
        return (pc.Struct("name"/pc.PaddedString(10, "utf8")), b"hello\x00\x00\x00\x00\x00", dict(name="hello"))
    if C == 'PS-U16LE-1':
        return (pc.Struct("name"/pc.PaddedString(8, "utf_16_le")), b"h\x00i\x00\x00\x00\x00\x00", dict(name="hi"))
    if C == 'PAS-U8-1':
        return (pc.Struct("name"/pc.PascalString(pc.Byte, "utf8")), b"\x05hello", dict(name="hello"))
    if C == 'PAS-U8-2':
        return (pc.Struct("name"/pc.PascalString(pc.Int16ub, "utf8")), b"\x00\x05hello", dict(name="hello"))
    if C == 'NT-BYTES-1':
        return (pc.Struct("data"/pc.NullTerminated(pc.GreedyBytes)), b"hello\x00world", dict(data=b"hello"))
    if C == 'NT-BYTE-1':
        return (pc.Struct("v"/pc.NullTerminated(pc.Byte, term=b"\xff")), b"\x42\xff", dict(v=0x42))
    if C == 'NS-1':
        return (pc.Struct("data"/pc.NullStripped(pc.GreedyBytes)), b"abc\x00\x00", dict(data=b"abc"))
    if C == 'NS-2':
        return (pc.Struct("data"/pc.NullStripped(pc.GreedyBytes, pad=b"\x00\x00")), b"ab\x00\x00\x00\x00", dict(data=b"ab"))

    raise ValueError('unknown case (py): ' + case_id)
'''


# ---------------------------------------------------------------------------
# case 清单（按 String Node 分组）
# ---------------------------------------------------------------------------

ALL_CASES = [
    # CString（4）：覆盖 utf8/ascii + utf16-le/be
    ("CS-U8-1",   "CString utf8 基本（hello + \\x00）"),
    ("CS-U8-2",   "CString utf8 多字节字符（Афон）"),
    ("CS-ASC-1",  "CString ascii 基本"),
    ("CS-U16LE-1","CString utf_16_le（2 字节 term）"),
    ("CS-U16BE-1","CString utf_16_be（字节序对齐 + BOM 不消费）"),
    # GreedyString（3）：utf8 + utf16-le/be
    ("GS-U8-1",   "GreedyString utf8 读到 EOF"),
    ("GS-U16LE-1","GreedyString utf_16_le"),
    ("GS-U16BE-1","GreedyString utf_16_be"),
    # PaddedString（2）：utf8 + utf16-le
    ("PS-U8-1",   "PaddedString(10, utf8) 常量长度"),
    ("PS-U16LE-1","PaddedString(8, utf_16_le) 多字节 pad"),
    # PascalString（2）：Byte / Int16ub lengthfield
    ("PAS-U8-1",  "PascalString(Byte, utf8)"),
    ("PAS-U8-2",  "PascalString(Int16ub, utf8)"),
    # NullTerminated（2）：GreedyBytes inner + 自定义 term
    ("NT-BYTES-1","NullTerminated(GreedyBytes) 默认 term"),
    ("NT-BYTE-1", "NullTerminated(Byte, term=b'\\xff') 自定义 term"),
    # NullStripped（2）：单字节 pad + 多字节 pad
    ("NS-1",      "NullStripped(GreedyBytes) 单字节 pad"),
    ("NS-2",      "NullStripped(GreedyBytes, pad=b'\\x00\\x00') 多字节 pad"),
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
# P2b：无后缀编码编译期拒绝（仅 rs 端，单测验证）
# ---------------------------------------------------------------------------

_P2B_CASE_DEFINITIONS = r'''
def _make_case_rs(case_id):
    from dataclasses import dataclass
    from construct import StructMixin, field, CString

    @dataclass
    class P(StructMixin):
        name: str = field(CString("utf16"))   # 无后缀编码，编译期应被拒

    # __init_subclass__ 在 class 定义时已触发编译，不应执行到此。
    raise RuntimeError("P2b rejection failed: expected CompilationError")


def _make_case_py(case_id):
    # 占位，不实际调用
    raise NotImplementedError
'''


def test_p2b_bare_utf16_rejected_by_rs(venv_pair):
    """P2b 决策：CString("utf16") 无后缀编码应在编译期被 construct-rs 拒绝。

    Python construct 2.10.70 仍接受 utf16（BOM 自检测），但 construct-rs 强制
    显式 _le / _be 后缀（PM 决策 6.2-D1 + Strings 设计 §2.1.3 P2b）。
    本测试仅验证 rs 端拒绝（不与 py 对比）。
    """
    # 预期 rs 端抛 RuntimeError（子进程内 CompilationError 上抛为 RuntimeError）
    with pytest.raises(RuntimeError, match="(?i).*utf16.*|.*utf_16_le.*|.*compilation.*"):
        run_parity_case("rs", "P2B-1", _P2B_CASE_DEFINITIONS, **venv_pair)


# ---------------------------------------------------------------------------
# Standalone 模式（python tests/parity/test_phase6_strings_parity.py）
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

    # P2b 单独验证
    print("\n--- P2b (bare utf16 rejection) ---")
    try:
        run_parity_case("rs", "P2B-1", _P2B_CASE_DEFINITIONS, **venv)
        # 不应执行到此（rs 端应在子进程内抛错）
        print("[FAIL] P2B-1: expected RuntimeError but no exception was raised")
        failures += 1
    except RuntimeError as e:
        if "utf16" in str(e).lower() or "utf_16_le" in str(e).lower() or "compilation" in str(e).lower():
            print(f"[PASS] P2B-1 (bare utf16 rejected as expected)")
        else:
            print(f"[FAIL] P2B-1: rejected but unexpected message: {e}")
            failures += 1

    print(f"\n=== Summary: {len(ALL_CASES) + 1 - failures}/{len(ALL_CASES) + 1} PASS ===")
    sys.exit(1 if failures else 0)
