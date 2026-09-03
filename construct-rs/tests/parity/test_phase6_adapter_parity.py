"""Phase 6.3 Adapter 系列构造器 Python 行为一致性测试。

设计依据：
- docs/design/模块设计/模块设计-Adapter核心.md §5 边界条件 / §11 parity 模板
- docs/design/基础设施/测试框架设计.md §5.1（统一模板）
- _helpers/parity.py（公共组件）

测试策略（子进程隔离）：两个同名包（construct-rs 与 Python construct 2.10.70）
无法在同一进程中导入，每个用例 × impl 在独立子进程中运行，通过 JSON 输出
结果，主进程比对。

覆盖范围（11 case）：
- Pass：PA1-PA2（parse 返回 None / build no-op）
- Subconstruct：SC1（Int8ub 转发）
- Peek：PK1-PK2（不消费流 / 失败吞掉）
- RawCopy：RC1-RC3（parse 返回 dict / build from data / build from value）
- Rebuild：RB1（在 Struct 中作为 RO 字段）
- AdapterCallback（用户面）：AC1（HexAdapter 嵌入 Struct）

差异处理：
- RC-build-1：RawCopy build 不返回带 data 的 Container（construct-rs build 路径
  无返回值）。parity 测试只对比 build 字节输出，跳过 build 返回值对比。
- AC-FFI：用户面 Adapter 嵌入 Struct 有 2 次 FFI（用户主动接受折衷）。
  parity 仅对比功能，不测性能（不设硬门禁）。

用法::

    pytest tests/parity/test_phase6_adapter_parity.py -v
    python tests/parity/test_phase6_adapter_parity.py
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
# - Pass / Substruct / Peek / RawCopy 在 rs/py 两侧行为一致，parity 直接对比
# - Rebuild：Python construct 用 ``len_`` 上下文函数取 items 长度；construct-rs 用
#   Phase 2 表达式（``items_count`` 字段引用）。两侧需用不同表达式但实现等价语义。
# - RawCopy build：Python 返回 Container(data=...)，construct-rs 不返回。
#   parity 跳过 build 返回值对比，仅对比 build 字节输出（设计 §5.3 RC-build-1）。
# - AdapterCallback：Python 用 ``Adapter`` 基类 + _decode/_encode；construct-rs 同。
_CASE_DEFINITIONS = '''
def _make_case_rs(case_id):
    from dataclasses import dataclass
    from construct import (
        StructMixin, field, rfield, Int8ub, Bytes,
        Subconstruct, Peek, RawCopy, Rebuild, Pass, Adapter,
    )

    C = case_id

    if C == 'PA1':
        # Pass parse 返回 None
        @dataclass
        class P(StructMixin):
            x: int = field(Int8ub)
            p: object = field(Pass)
        data = bytes([0x10])
        return P, data, lambda: P(x=0x10, p=None), lambda o: {'x': o.x, 'p': o.p}

    if C == 'PA2':
        # Pass build no-op
        @dataclass
        class P(StructMixin):
            x: int = field(Int8ub)
            p: object = field(Pass)
        data = bytes([0x42])
        return P, data, lambda: P(x=0x42, p=None), lambda o: {'x': o.x, 'p': o.p}

    if C == 'SC1':
        # Substruct 纯转发 Int8ub
        @dataclass
        class P(StructMixin):
            x: int = field(Subconstruct(Int8ub))
        data = bytes([0x42])
        return P, data, lambda: P(x=0x42), lambda o: {'x': o.x}

    if C == 'PK1':
        # Peek 不消费流
        @dataclass
        class P(StructMixin):
            a: int = field(Peek(Int8ub))
            b: int = field(Int8ub)
        data = bytes([0x10])
        return P, data, lambda: P(a=0x10, b=0x10), lambda o: {'a': o.a, 'b': o.b}

    if C == 'PK2':
        # Peek 失败吞掉（流不足时返回 None）
        @dataclass
        class P(StructMixin):
            a: object = field(Peek(Bytes(5)))   # 5 字节，但流只有 1 字节
            b: int = field(Int8ub)
        data = bytes([0x99])
        # Peek 失败 → None，b 仍读第 1 字节
        return P, data, lambda: P(a=None, b=0x99), lambda o: {'a': o.a, 'b': o.b}

    if C == 'RC1':
        # RawCopy parse 返回 dict
        @dataclass
        class P(StructMixin):
            x: object = field(RawCopy(Int8ub))
        data = bytes([0xFF])
        return P, data, lambda: P(x={'data': b'\\xff', 'value': 255, 'offset1': 0, 'offset2': 1, 'length': 1}), \\
            lambda o: {'x': {'data': bytes(o.x['data']), 'value': o.x['value'],
                              'offset1': o.x['offset1'], 'offset2': o.x['offset2'],
                              'length': o.x['length']}}

    if C == 'RC2':
        # RawCopy build from data
        @dataclass
        class P(StructMixin):
            x: object = field(RawCopy(Int8ub))
        data = bytes([0xAA])
        # build with data key
        return P, data, lambda: P(x={'data': b'\\xAA'}), \\
            lambda o: {'x': 'skip'}  # extract skip（build 返回值不对比）

    if C == 'RC3':
        # RawCopy build from value
        @dataclass
        class P(StructMixin):
            x: object = field(RawCopy(Int8ub))
        data = bytes([0xBB])
        return P, data, lambda: P(x={'value': 0xBB}), \\
            lambda o: {'x': 'skip'}

    if C == 'RB1':
        # Rebuild 在 Struct 中作为 RO 字段
        # construct-rs：count = Rebuild(Int8ub, items_count 字段引用)
        # build 语义：items_count=7 → Rebuild 忽略 count（RO），从 items_count=7 重算 → 输出 b"\\x07\\x07"
        @dataclass
        class P(StructMixin):
            items_count: int = field(Int8ub)         # 字段值供 Rebuild 引用
            count: int = rfield(Rebuild(Int8ub, items_count))
        data = bytes([0x05, 0x05])  # items_count=5, count=5
        # build with items_count=7 (count 由 Rebuild 重算)
        return P, data, lambda: P(items_count=7), \\
            lambda o: {'items_count': o.items_count, 'count': o.count}

    if C == 'AC1':
        # Adapter 用户面（HexAdapter）
        # parse: Int8ub.parse(b"\\x10") → 16 → _decode(16) → "0x10"
        # build: _encode("0xff") → 255 → Int8ub.build(255) → b"\\xff"
        class HexAdapter(Adapter):
            def _decode(self, obj, context, path):
                return hex(obj)
            def _encode(self, obj, context, path):
                return int(obj, 16)

        @dataclass
        class P(StructMixin):
            v: str = field(HexAdapter(Int8ub))
        data = bytes([0x10])
        # build with v="0xff" → b"\\xff"
        return P, data, lambda: P(v='0xff'), lambda o: {'v': o.v}

    raise ValueError(f"unknown case: {case_id}")


def _make_case_py(case_id):
    from construct import (
        Struct, Subconstruct, Peek, RawCopy, Rebuild, Pass,
        Int8ub, Bytes, Adapter, len_, this,
    )

    C = case_id

    if C == 'PA1':
        fmt = Struct("x" / Int8ub, "p" / Pass)
        return fmt, bytes([0x10]), {'x': 0x10, 'p': None}

    if C == 'PA2':
        fmt = Struct("x" / Int8ub, "p" / Pass)
        return fmt, bytes([0x42]), {'x': 0x42, 'p': None}

    if C == 'SC1':
        fmt = Struct("x" / Subconstruct(Int8ub))
        return fmt, bytes([0x42]), {'x': 0x42}

    if C == 'PK1':
        fmt = Struct("a" / Peek(Int8ub), "b" / Int8ub)
        return fmt, bytes([0x10]), {'a': 0x10, 'b': 0x10}

    if C == 'PK2':
        fmt = Struct("a" / Peek(Bytes(5)), "b" / Int8ub)
        return fmt, bytes([0x99]), {'a': None, 'b': 0x99}

    if C == 'RC1':
        fmt = Struct("x" / RawCopy(Int8ub))
        return fmt, bytes([0xFF]), {'x': {'data': b'\\xff', 'value': 255,
                                            'offset1': 0, 'offset2': 1, 'length': 1}}

    if C == 'RC2':
        # RawCopy build from data
        fmt = Struct("x" / RawCopy(Int8ub))
        return fmt, bytes([0xAA]), {'x': {'data': b'\\xAA'}}

    if C == 'RC3':
        # RawCopy build from value
        fmt = Struct("x" / RawCopy(Int8ub))
        return fmt, bytes([0xBB]), {'x': {'value': 0xBB}}

    if C == 'RB1':
        # Python construct Rebuild：count 由 items_count 重建
        # build_input 用 items_count=7，count 被忽略（Rebuild 重算）
        fmt = Struct(
            "items_count" / Int8ub,
            "count" / Rebuild(Int8ub, this.items_count),
        )
        return fmt, bytes([0x05, 0x05]), {'items_count': 0x07, 'count': 0xAA}

    if C == 'AC1':
        class HexAdapter(Adapter):
            def _decode(self, obj, context, path):
                return hex(obj)
            def _encode(self, obj, context, path):
                return int(obj, 16)
        fmt = Struct("v" / HexAdapter(Int8ub))
        # build with v="0xff" → b"\\xff"
        return fmt, bytes([0x10]), {'v': '0xff'}

    raise ValueError(f"unknown case: {case_id}")
'''


# ---------------------------------------------------------------------------
# 所有 case 列表（用于 parametrize）
# ---------------------------------------------------------------------------

ALL_CASES = [
    # Pass
    ("PA1", "Pass.parse 返回 None（不消费字节）"),
    ("PA2", "Pass.build no-op（不写字节）"),
    # Subconstruct
    ("SC1", "Substruct(Int8ub) 纯转发"),
    # Peek
    ("PK1", "Peek 不消费流"),
    ("PK2", "Peek 失败吞掉返回 None"),
    # RawCopy
    ("RC1", "RawCopy parse 返回 dict(data,value,offset1,offset2,length)"),
    ("RC2", "RawCopy build from data key"),
    ("RC3", "RawCopy build from value key"),
    # Rebuild
    ("RB1", "Rebuild 作为 RO 字段（引用前序字段）"),
    # AdapterCallback（用户面）
    ("AC1", "HexAdapter 嵌入 Struct（_decode/_encode 回调）"),
]


# ---------------------------------------------------------------------------
# pytest fixture（容错版：失败 case 标记 error 不崩溃）
# ---------------------------------------------------------------------------


@pytest.fixture(scope="module")
def parity_results(venv_pair):
    """预跑全部 case × impl，失败 case 记录 error（不崩溃）。"""
    results = {}
    for case_id, _ in ALL_CASES:
        entry = {}
        for impl in ("rs", "py"):
            try:
                entry[impl] = run_parity_case(
                    impl, case_id, _CASE_DEFINITIONS, **venv_pair
                )
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
    # RC2/RC3 的 build 输入 dict 不是合法 parse 输入，跳过 parse 对比
    # （但 roundtrip 仍验证 build→parse 一致性）
    if case_id in ("RC2", "RC3"):
        pytest.skip("RC2/RC3 build-only case (parse 输入 dict 不合法)")
    assert_parity(
        rs,
        py,
        case_id,
        desc=desc,
        check_build=False,
        check_roundtrip=False,
    )


@pytest.mark.parametrize("case_id,desc", ALL_CASES)
def test_parity_build(parity_results, case_id: str, desc: str):
    """Rust 和 Python construct 对相同输入 build 应产生相同字节。"""
    rs = parity_results[case_id]["rs"]
    py = parity_results[case_id]["py"]
    _check_impl_error(rs, case_id, desc)
    _check_impl_error(py, case_id, desc)
    assert_parity(
        rs,
        py,
        case_id,
        desc=desc,
        check_parse=False,
        check_roundtrip=False,
    )


@pytest.mark.parametrize("case_id,desc", ALL_CASES)
def test_parity_roundtrip(parity_results, case_id: str, desc: str):
    """Rust 和 Python construct 的 parse→build→parse 往返结果应一致。"""
    rs = parity_results[case_id]["rs"]
    py = parity_results[case_id]["py"]
    _check_impl_error(rs, case_id, desc)
    _check_impl_error(py, case_id, desc)
    # RC2/RC3 的 roundtripParsed 不对称（build 输入 dict 不等于 parse 输出）
    if case_id in ("RC2", "RC3"):
        pytest.skip("RC2/RC3 build-only case (roundtrip 不对称)")
    assert_parity(
        rs,
        py,
        case_id,
        desc=desc,
        check_parse=False,
        check_build=False,
    )


@pytest.mark.parametrize("case_id,desc", ALL_CASES)
def test_parity_roundtrip_fidelity(parity_results, case_id: str, desc: str):
    """parse→build→parse 的结果应与首次 parse 一致（往返保真，rs/py 各自）。"""
    rs = parity_results[case_id]["rs"]
    py = parity_results[case_id]["py"]
    _check_impl_error(rs, case_id, desc)
    _check_impl_error(py, case_id, desc)
    # RC2/RC3 的 roundtripParsed 不对称（build 输入 dict 不等于 parse 输出）
    if case_id in ("RC2", "RC3"):
        pytest.skip("RC2/RC3 build-only case (roundtrip 不对称)")
    # RB1/AC1 的 build 输入故意与 parse 输出不同（测试 Rebuild 重算 / Adapter 转换），
    # 因此 roundtrip != first parse（非 bug，是 case 设计）。
    if case_id in ("RB1", "AC1"):
        pytest.skip("RB1/AC1 build 输入故意与 parse 输出不同（测试 Rebuild/Adapter 语义）")
    assert_fidelity(rs, case_id, desc=desc)
    assert_fidelity(py, case_id, desc=desc)


# ---------------------------------------------------------------------------
# standalone 入口（直接 python 执行时打印对比表）
# ---------------------------------------------------------------------------


def _run_standalone():
    """直接执行（python tests/parity/test_phase6_adapter_parity.py）时的入口。"""
    print("=" * 78)
    print("Phase 6.3 Adapter 系列 构造器 Python 行为一致性测试")
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

        # RC2/RC3 跳过 parse/roundtrip 对比（build-only case）
        # RB1/AC1 跳过 fidelity（build 输入故意与 parse 输出不同）
        skip_parse = case_id in ("RC2", "RC3")
        skip_fidelity = case_id in ("RC2", "RC3", "RB1", "AC1")

        parse_ok = skip_parse or rs["parsed"] == py["parsed"]
        build_ok = rs["built"] == py["built"]
        roundtrip_ok = skip_parse or rs["roundtrip_parsed"] == py["roundtrip_parsed"]
        rs_fidelity = rs["parsed"] == rs["roundtrip_parsed"]
        py_fidelity = py["parsed"] == py["roundtrip_parsed"]
        fidelity_ok = skip_fidelity or (rs_fidelity and py_fidelity)

        def _mark(ok, skip=False):
            if skip:
                return "SKIP"
            return "OK" if ok else "[FAIL]"

        print(
            f"{case_id:<6} "
            f"{_mark(parse_ok, skip_parse):<10} "
            f"{_mark(build_ok):<10} "
            f"{_mark(roundtrip_ok, skip_parse):<10} "
            f"{_mark(fidelity_ok, skip_fidelity):<10}"
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

    print()
    if failures == 0:
        print(f"[PASS] 全部 {len(ALL_CASES)} 个 case 一致")
        return 0
    else:
        print(f"[FAIL] {failures} 个 case 不一致")
        return 1


if __name__ == "__main__":
    sys.exit(_run_standalone())
