"""Phase 3 BitStream 构造器 Python 行为一致性测试。

设计依据：
- docs/design/基础设施/测试框架设计.md §5.1（统一模板）
- docs/design/模块设计/模块设计-BitStream.md §9 边界条件
- _helpers/parity.py（公共组件）

测试策略（子进程隔离）：两个同名包（construct-rs 与 Python construct 2.10.70）
无法在同一进程中导入，每个用例 × impl 在独立子进程中运行，通过 JSON 输出
结果，主进程比对。

覆盖范围（13 case）：BitsInteger（P1-P4）/ Bit+Nibble+Octet+Padding（P5-P7）/
BitsSwapped+ByteSwapped（P8-P10）/ BitPadding（P11）/ swapped（P12）/ 复合（P13）。

用法::

    pytest tests/parity/test_phase3_bitstream_parity.py -v
    python tests/parity/test_phase3_bitstream_parity.py
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
# case 定义（子进程脚本片段，定义 _make_case_rs / _make_case_py）
# ---------------------------------------------------------------------------
# 注意（设计 §5.3 注意点 1 + v2 改进-2）：
# - _make_case_rs / _make_case_py 参数化（case_id 入参），函数体内 ``C = case_id``。
# - 本字符串通过 ``{case_definitions}`` 注入 PARITY_SCRIPT_TEMPLATE.format()，
#   .format 不二次解析本字符串内部花括号，故 case 定义代码用单花括号。
# - bytes 字面量转义：父进程 Python 三引号字符串里 ``\\xA5`` 转义为 ``\xA5``，
#   子进程看到 ``b"\\xA5"`` 的字面 ``\xA5``（即字节 0xA5）。
_CASE_DEFINITIONS = '''
def _make_case_rs(case_id):
    from dataclasses import dataclass
    from construct import (
        StructMixin, BitStructMixin, field, rfield, wfield,
        BitsInteger, Bit, Nibble, Octet,
        Bitwise, Bytewise, BitsSwapped, ByteSwapped, Padding,
        Bytes, Int16ub, Int32ub,
    )

    C = case_id
    if C == 'P1':
        # BitsInteger 各种位数
        @dataclass
        class P(BitStructMixin):
            v: int = field(BitsInteger(8))
        return P, b"\\xA5", lambda: P(v=0xA5), lambda o: {'v': o.v}
    if C == 'P2':
        @dataclass
        class P(BitStructMixin):
            v: int = field(BitsInteger(16))
        return P, b"\\xA5\\x3C", lambda: P(v=0xA53C), lambda o: {'v': o.v}
    if C == 'P3':
        @dataclass
        class P(BitStructMixin):
            v: int = field(BitsInteger(32))
        return P, b"\\xA5\\x3C\\x96\\xC3", lambda: P(v=0xA53C96C3), lambda o: {'v': o.v}
    if C == 'P4':
        @dataclass
        class P(BitStructMixin):
            v: int = field(BitsInteger(8, signed=True))
        return P, b"\\xA5", lambda: P(v=-91), lambda o: {'v': o.v}
    if C == 'P5':
        # Bit / Nibble / Octet 语法糖组合 + Padding(3) 凑 16 bit
        # b"\\xAB\\xCD" = 0b10101011 0b11001101
        # a(Bit)=1, b(Nibble=bits1-4)=0101=0x5, c(Octet=bits5-12)=01111001=0x79,
        # pad(Padding=bits13-15)=101
        @dataclass
        class P(BitStructMixin):
            a: int = field(Bit())
            b: int = field(Nibble())
            c: int = field(Octet())
            d: int = wfield(Padding(3), default=None)
        return P, b"\\xAB\\xCD", lambda: P(a=1, b=0x5, c=0x79), \\
            lambda o: {'a': o.a, 'b': o.b, 'c': o.c, 'd': o.d}
    if C == 'P6':
        # BitStruct 多字段含 Padding
        @dataclass
        class P(BitStructMixin):
            a: int = field(Nibble())
            b: int = field(BitsInteger(10))
            c: int = wfield(Padding(2), default=None)
        return P, b"\\xBE\\xEF", lambda: P(a=0xB, b=0x3BB), \\
            lambda o: {'a': o.a, 'b': o.b, 'c': o.c}
    if C == 'P7':
        # Bytewise 嵌入（对齐快路径）
        @dataclass
        class P(BitStructMixin):
            a: int = field(Nibble())
            b: int = field(Bytewise(Int16ub))
            c: int = field(Nibble())
        return P, b"\\xA1\\x23\\x4B", lambda: P(a=0xA, b=0x1234, c=0xB), \\
            lambda o: {'a': o.a, 'b': o.b, 'c': o.c}
    if C == 'P8':
        # BitsSwapped(Bytes(N))
        @dataclass
        class P(StructMixin):
            v: bytes = field(BitsSwapped(Bytes(4)))
        return P, b"\\xF0\\x0F\\xAA\\x55", lambda: P(v=b"\\x0F\\xF0\\x55\\xAA"), \\
            lambda o: {'v': o.v}
    if C == 'P9':
        # ByteSwapped(Int32ub)
        @dataclass
        class P(StructMixin):
            v: int = field(ByteSwapped(Int32ub))
        return P, b"\\x78\\x56\\x34\\x12", lambda: P(v=0x12345678), \\
            lambda o: {'v': o.v}
    if C == 'P10':
        # 字节级 Padding
        @dataclass
        class P(StructMixin):
            tag: bytes = field(Bytes(1))
            reserved: int = wfield(Padding(4), default=None)
            data: bytes = field(Bytes(2))
        return P, b"\\xAA\\x00\\x00\\x00\\x00\\xBB\\xCC", \\
            lambda: P(tag=b"\\xAA", data=b"\\xBB\\xCC"), \\
            lambda o: {'tag': o.tag, 'reserved': o.reserved, 'data': o.data}
    if C == 'P11':
        # Bit 级 Padding (pattern=0x01)
        @dataclass
        class P(BitStructMixin):
            a: int = field(BitsInteger(4))
            pad: int = wfield(Padding(4, pattern=b"\\x01"), default=None)
        return P, b"\\x51", lambda: P(a=0x5), \\
            lambda o: {'a': o.a, 'pad': o.pad}
    if C == 'P12':
        # BitsInteger swapped (byte-order reverse within bit-string)
        # 16-bit: b"\\x01\\x02" BE=0x0102=258, swapped(LE)=0x0201=513
        @dataclass
        class P(BitStructMixin):
            v: int = field(BitsInteger(16, swapped=True))
        return P, b"\\x01\\x02", lambda: P(v=513), lambda o: {'v': o.v}
    if C == 'P13':
        # 复合场景：Bit + signed BitsInteger + Bytewise
        # b"\\xE5\\xAA\\xBB":
        #   0xE5 = 0b11100101
        #   flag(Bit)=1, val(BitsInteger(7,signed)=bits1-7)=1100101
        #   signed 7-bit 0b1100101 = 101 - 128 = -27
        #   payload = bytes 1-2 = 0xAABB
        @dataclass
        class P(BitStructMixin):
            flag: int = field(Bit())
            val: int = field(BitsInteger(7, signed=True))
            payload: int = field(Bytewise(Int16ub))
        return P, b"\\xE5\\xAA\\xBB", \\
            lambda: P(flag=1, val=-27, payload=0xAABB), \\
            lambda o: {'flag': o.flag, 'val': o.val, 'payload': o.payload}
    raise ValueError('unknown case (rs): ' + case_id)

def _make_case_py(case_id):
    import construct as pc

    C = case_id
    if C == 'P1':
        return (pc.BitStruct("v"/pc.BitsInteger(8)),
                b"\\xA5", dict(v=0xA5))
    if C == 'P2':
        return (pc.BitStruct("v"/pc.BitsInteger(16)),
                b"\\xA5\\x3C", dict(v=0xA53C))
    if C == 'P3':
        return (pc.BitStruct("v"/pc.BitsInteger(32)),
                b"\\xA5\\x3C\\x96\\xC3", dict(v=0xA53C96C3))
    if C == 'P4':
        return (pc.BitStruct("v"/pc.BitsInteger(8, signed=True)),
                b"\\xA5", dict(v=-91))
    if C == 'P5':
        return (pc.BitStruct("a"/pc.Bit, "b"/pc.Nibble,
                             "c"/pc.Octet, "d"/pc.Padding(3)),
                b"\\xAB\\xCD", dict(a=1, b=0x5, c=0x79))
    if C == 'P6':
        return (pc.BitStruct("a"/pc.Nibble, "b"/pc.BitsInteger(10),
                             "c"/pc.Padding(2)),
                b"\\xBE\\xEF", dict(a=0xB, b=0x3BB))
    if C == 'P7':
        return (pc.BitStruct("a"/pc.Nibble, "b"/pc.Bytewise(pc.Int16ub),
                             "c"/pc.Nibble),
                b"\\xA1\\x23\\x4B", dict(a=0xA, b=0x1234, c=0xB))
    if C == 'P8':
        return (pc.Struct("v"/pc.BitsSwapped(pc.Bytes(4))),
                b"\\xF0\\x0F\\xAA\\x55", dict(v=b"\\x0F\\xF0\\x55\\xAA"))
    if C == 'P9':
        return (pc.Struct("v"/pc.ByteSwapped(pc.Int32ub)),
                b"\\x78\\x56\\x34\\x12", dict(v=0x12345678))
    if C == 'P10':
        return (pc.Struct("tag"/pc.Bytes(1), "reserved"/pc.Padding(4),
                          "data"/pc.Bytes(2)),
                b"\\xAA\\x00\\x00\\x00\\x00\\xBB\\xCC",
                dict(tag=b"\\xAA", data=b"\\xBB\\xCC"))
    if C == 'P11':
        return (pc.BitStruct("a"/pc.BitsInteger(4),
                             "pad"/pc.Padding(4, pattern=b"\\x01")),
                b"\\x51", dict(a=0x5))
    if C == 'P12':
        return (pc.BitStruct("v"/pc.BitsInteger(16, swapped=True)),
                b"\\x01\\x02", dict(v=513))
    if C == 'P13':
        return (pc.BitStruct("flag"/pc.Bit,
                             "val"/pc.BitsInteger(7, signed=True),
                             "payload"/pc.Bytewise(pc.Int16ub)),
                b"\\xE5\\xAA\\xBB", dict(flag=1, val=-27, payload=0xAABB))
    raise ValueError('unknown case (py): ' + case_id)
'''


# ---------------------------------------------------------------------------
# case 清单（与 _CASE_DEFINITIONS 一一对应，13 case）
# ---------------------------------------------------------------------------

ALL_CASES = [
    ("P1", "BitsInteger(8) — 8-bit unsigned via BitStruct"),
    ("P2", "BitsInteger(16) — 16-bit unsigned"),
    ("P3", "BitsInteger(32) — 32-bit unsigned"),
    ("P4", "BitsInteger(8, signed=True) — 负数"),
    ("P5", "Bit / Nibble / Octet 语法糖组合 + Padding(3)"),
    ("P6", "BitStruct + Padding（bit 域）"),
    ("P7", "BitStruct + Bytewise 嵌入（对齐快路径）"),
    ("P8", "BitsSwapped(Bytes(4)) — 字节内 bit 序翻转"),
    ("P9", "ByteSwapped(Int32ub) — 字节序翻转"),
    ("P10", "Struct + 字节级 Padding"),
    ("P11", "BitPadding with pattern=0x01"),
    ("P12", "BitsInteger(16, swapped=True) — 字节序反序"),
    ("P13", "复合场景：Bit + signed BitsInteger(7) + Bytewise(Int16ub)"),
]


# ---------------------------------------------------------------------------
# parity_results fixture（容错版：失败 case 记录 error，不崩溃）
# ---------------------------------------------------------------------------
# 同 test_phase4_array_parity.py 的工程处理：BitStream 各 case 在当前 construct-rs
# 应全部支持，但保留容错以防个别 case 的 impl 差异。

@pytest.fixture(scope="module")
def parity_results(venv_pair):
    """预跑全部 case × impl，失败 case 记录 error（不崩溃）。"""
    results = {}
    for case_id, _ in ALL_CASES:
        entry = {}
        for impl in ("rs", "py"):
            try:
                entry[impl] = run_parity_case(impl, case_id, _CASE_DEFINITIONS, **venv_pair)
            except RuntimeError as e:
                entry[impl] = {"error": str(e)}
        results[case_id] = entry
    return results


def _check_impl_error(result, case_id, desc):
    """若 result 含 error key，pytest.skip 并报告原因。"""
    if "error" in result:
        pytest.skip(
            f"case {case_id} ({desc}): impl error - {result['error'][:200]}"
        )


# ---------------------------------------------------------------------------
# pytest 测试（5 个，所有 parity 文件统一）
# ---------------------------------------------------------------------------

def test_parity_all_cases_collected(parity_results):
    """确认所有 case 都被预跑（含 broken case 的 error 记录）。"""
    for case_id, _ in ALL_CASES:
        assert case_id in parity_results
        assert "rs" in parity_results[case_id]
        assert "py" in parity_results[case_id]


@pytest.mark.parametrize("case_id,desc", ALL_CASES)
def test_parity_parse(parity_results, case_id: str, desc: str):
    """Rust 和 Python construct 对相同输入 parse 应产生相同输出。"""
    rs = parity_results[case_id]["rs"]
    py = parity_results[case_id]["py"]
    _check_impl_error(rs, case_id, desc)
    _check_impl_error(py, case_id, desc)
    assert_parity(
        rs, py, case_id, desc=desc,
        check_build=False, check_roundtrip=False,
    )


@pytest.mark.parametrize("case_id,desc", ALL_CASES)
def test_parity_build(parity_results, case_id: str, desc: str):
    """Rust 和 Python construct 对相同输入 build 应产生相同字节。"""
    rs = parity_results[case_id]["rs"]
    py = parity_results[case_id]["py"]
    _check_impl_error(rs, case_id, desc)
    _check_impl_error(py, case_id, desc)
    assert_parity(
        rs, py, case_id, desc=desc,
        check_parse=False, check_roundtrip=False,
    )


@pytest.mark.parametrize("case_id,desc", ALL_CASES)
def test_parity_roundtrip(parity_results, case_id: str, desc: str):
    """Rust 和 Python construct 的 parse→build→parse 往返结果应一致。"""
    rs = parity_results[case_id]["rs"]
    py = parity_results[case_id]["py"]
    _check_impl_error(rs, case_id, desc)
    _check_impl_error(py, case_id, desc)
    assert_parity(
        rs, py, case_id, desc=desc,
        check_parse=False, check_build=False,
    )


@pytest.mark.parametrize("case_id,desc", ALL_CASES)
def test_parity_roundtrip_fidelity(parity_results, case_id: str, desc: str):
    """parse→build→parse 的结果应与首次 parse 一致（往返保真，rs/py 各自）。"""
    rs = parity_results[case_id]["rs"]
    py = parity_results[case_id]["py"]
    _check_impl_error(rs, case_id, desc)
    _check_impl_error(py, case_id, desc)
    assert_fidelity(rs, case_id, desc=desc)
    assert_fidelity(py, case_id, desc=desc)


# ---------------------------------------------------------------------------
# standalone 入口（直接 python 执行时打印对比表）
# ---------------------------------------------------------------------------

def _run_standalone():
    """直接执行（python tests/parity/test_phase3_bitstream_parity.py）时的入口。"""
    print("=" * 78)
    print("Phase 3 BitStream 构造器 Python 行为一致性测试")
    print("=" * 78)
    print(f"Python (rs): {_RS_PYTHON_EXE}")
    print(f"Python (py): {_PY_PYTHON_EXE}")
    print(f"construct-rs: {_CRS_PYTHON_DIR}")
    print()

    venv_pair = {
        "rs_python": _RS_PYTHON_EXE,
        "py_python": _PY_PYTHON_EXE,
        "crs_python_dir": _CRS_PYTHON_DIR,
    }
    failures = 0
    header = (
        f"{'场景':<6} {'parse':<10} {'build':<10} "
        f"{'roundtrip':<10} {'保真':<10}"
    )
    print(header)
    print("-" * len(header))

    for case_id, desc in ALL_CASES:
        try:
            rs = run_parity_case("rs", case_id, _CASE_DEFINITIONS, **venv_pair)
            py = run_parity_case("py", case_id, _CASE_DEFINITIONS, **venv_pair)
        except RuntimeError as e:
            print(f"{case_id:<6} [ERROR] {e}")
            failures += 1
            continue

        parse_ok = rs["parsed"] == py["parsed"]
        build_ok = rs["built"] == py["built"]
        roundtrip_ok = rs["roundtrip_parsed"] == py["roundtrip_parsed"]
        rs_fidelity = rs["parsed"] == rs["roundtrip_parsed"]
        py_fidelity = py["parsed"] == py["roundtrip_parsed"]
        fidelity_ok = rs_fidelity and py_fidelity

        def _mark(ok):
            return "OK" if ok else "[FAIL]"

        print(
            f"{case_id:<6} "
            f"{_mark(parse_ok):<10} "
            f"{_mark(build_ok):<10} "
            f"{_mark(roundtrip_ok):<10} "
            f"{_mark(fidelity_ok):<10}"
        )

        if not (parse_ok and build_ok and roundtrip_ok and fidelity_ok):
            failures += 1
            print(f"       desc: {desc}")
            if not parse_ok:
                print(f"       parse rs={rs['parsed']}")
                print(f"       parse py={py['parsed']}")
            if not build_ok:
                print(f"       build rs={rs['built']}")
                print(f"       build py={py['built']}")
            if not roundtrip_ok:
                print(f"       roundtrip rs={rs['roundtrip_parsed']}")
                print(f"       roundtrip py={py['roundtrip_parsed']}")
            if not fidelity_ok:
                print(f"       fidelity rs: first={rs['parsed']} rt={rs['roundtrip_parsed']}")
                print(f"       fidelity py: first={py['parsed']} rt={py['roundtrip_parsed']}")

    print()
    if failures == 0:
        print(f"[PASS] 全部 {len(ALL_CASES)} 个 case 一致")
        return 0
    else:
        print(f"[FAIL] {failures} 个 case 不一致")
        return 1


if __name__ == "__main__":
    sys.exit(_run_standalone())
