"""Phase 3 BitStream 构造器 Python 行为一致性测试。

设计依据：
- performance-gate/SKILL.md（性能门禁子进程隔离方法可复用为正确性对比）
- plans/phase3-bitstream/总纲.md S-FUNC：Bitwise 相关构造器行为与 Python
  construct 2.10.70 一致
- docs/模块设计-BitStream.md §9 边界条件

测试策略：
    两个同名包（construct-rs 与 Python construct 2.10.70）无法在同一进程
    中导入，采用**子进程隔离**策略：每个用例 × impl 在独立子进程中运行，
    通过 JSON 输出结果，主进程比对。

    每个用例对 Rust 和 Python 两侧执行相同的 parse/build 操作，验证：
    - parse 同输入 → 同输出（值相等）
    - build 同输入 → 同输出（字节序列相等）
    - parse → build → parse 往返一致

覆盖范围：Phase 3 全部 8 个构造器
    - BitsInteger / Bit / Nibble / Octet（Phase 3.1）
    - Bitwise / BitStruct（Phase 3.2）
    - Bytewise / BitsSwapped / ByteSwapped / Padding（Phase 3.3）

用法::

    python tests/test_bitstream_parity.py
    pytest tests/test_bitstream_parity.py
"""

from __future__ import annotations

import json
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest

# construct-rs python/ 目录（用于 sys.path 操纵，让 construct-rs 覆盖 site-packages）
_CRS_PYTHON_DIR = str(
    Path(__file__).resolve().parent.parent / "python"
)

# Python 解释器（用于跑两个 impl）。默认与主进程相同。
_PYTHON_EXE = sys.executable


# ---------------------------------------------------------------------------
# 子进程校验脚本
# ---------------------------------------------------------------------------

# 子进程脚本接收一个 case_id 和 impl，输出该 case 在该 impl 下的 parse 与 build
# 结果（JSON）。每个 case 在两侧的 _make_case 函数中定义"对等"的用例。

_PARITY_SCRIPT = textwrap.dedent(
    """\
    import json
    import sys

    IMPL = {impl!r}
    CASE = {case!r}
    CRS_PYTHON_DIR = {crs_python_dir!r}

    # ---- sys.path 操纵 ----
    if IMPL == 'rs':
        sys.path[:] = [p for p in sys.path if p != CRS_PYTHON_DIR]
        sys.path.insert(0, CRS_PYTHON_DIR)
    else:
        sys.path[:] = [p for p in sys.path if p != CRS_PYTHON_DIR]
    for k in list(sys.modules):
        if k == 'construct' or k.startswith('construct.'):
            del sys.modules[k]

    def _normalize(value):
        \"\"\"规范化输出值，便于跨实现比较。

        - dict / Container → 排序后的 (key, value) 列表（递归），过滤 _io 等内部键
        - list / ListContainer → 列表（递归）
        - bytes → {{'__bytes__': hex}} 包装
        - 其它（int/bool/None）原样返回
        \"\"\"
        if isinstance(value, dict):
            return {{k: _normalize(value[k])
                    for k in sorted(value.keys())
                    if not k.startswith('_')}}
        if isinstance(value, (list, tuple)):
            return [_normalize(v) for v in value]
        if isinstance(value, (bytes, bytearray)):
            return {{'__bytes__': bytes(value).hex()}}
        return value

    # ---- 用例定义 ----
    def _make_case():
        if IMPL == 'rs':
            return _make_case_rs()
        else:
            return _make_case_py()

    def _make_case_rs():
        from dataclasses import dataclass
        from construct import (
            StructMixin, BitStructMixin, field, rfield, wfield,
            BitsInteger, Bit, Nibble, Octet,
            Bitwise, Bytewise, BitsSwapped, ByteSwapped, Padding,
            Bytes, Int16ub, Int32ub,
        )

        C = CASE
        if C == 'P1':
            # BitsInteger 各种位数
            @dataclass
            class P(BitStructMixin):
                v: int = field(BitsInteger(8))
            return P, b"\\xA5", lambda: P(v=0xA5), lambda o: {{'v': o.v}}
        if C == 'P2':
            @dataclass
            class P(BitStructMixin):
                v: int = field(BitsInteger(16))
            return P, b"\\xA5\\x3C", lambda: P(v=0xA53C), lambda o: {{'v': o.v}}
        if C == 'P3':
            @dataclass
            class P(BitStructMixin):
                v: int = field(BitsInteger(32))
            return P, b"\\xA5\\x3C\\x96\\xC3", lambda: P(v=0xA53C96C3), lambda o: {{'v': o.v}}
        if C == 'P4':
            @dataclass
            class P(BitStructMixin):
                v: int = field(BitsInteger(8, signed=True))
            return P, b"\\xA5", lambda: P(v=-91), lambda o: {{'v': o.v}}
        if C == 'P5':
            # Bit / Nibble / Octet 语法糖组合 + Padding(3) 凑 16 bit
            # b"\xAB\xCD" = 0b10101011 0b11001101
            # a(Bit)=1, b(Nibble=bits1-4)=0101=0x5, c(Octet=bits5-12)=01111001=0x79,
            # pad(Padding=bits13-15)=101
            @dataclass
            class P(BitStructMixin):
                a: int = field(Bit())
                b: int = field(Nibble())
                c: int = field(Octet())
                d: int = wfield(Padding(3), default=None)
            return P, b"\\xAB\\xCD", lambda: P(a=1, b=0x5, c=0x79), \\
                lambda o: {{'a': o.a, 'b': o.b, 'c': o.c, 'd': o.d}}
        if C == 'P6':
            # BitStruct 多字段含 Padding
            @dataclass
            class P(BitStructMixin):
                a: int = field(Nibble())
                b: int = field(BitsInteger(10))
                c: int = wfield(Padding(2), default=None)
            return P, b"\\xBE\\xEF", lambda: P(a=0xB, b=0x3BB), \\
                lambda o: {{'a': o.a, 'b': o.b, 'c': o.c}}
        if C == 'P7':
            # Bytewise 嵌入（对齐快路径）
            @dataclass
            class P(BitStructMixin):
                a: int = field(Nibble())
                b: int = field(Bytewise(Int16ub))
                c: int = field(Nibble())
            return P, b"\\xA1\\x23\\x4B", lambda: P(a=0xA, b=0x1234, c=0xB), \\
                lambda o: {{'a': o.a, 'b': o.b, 'c': o.c}}
        if C == 'P8':
            # BitsSwapped(Bytes(N))
            @dataclass
            class P(StructMixin):
                v: bytes = field(BitsSwapped(Bytes(4)))
            return P, b"\\xF0\\x0F\\xAA\\x55", lambda: P(v=b"\\x0F\\xF0\\x55\\xAA"), \\
                lambda o: {{'v': o.v}}
        if C == 'P9':
            # ByteSwapped(Int32ub)
            @dataclass
            class P(StructMixin):
                v: int = field(ByteSwapped(Int32ub))
            return P, b"\\x78\\x56\\x34\\x12", lambda: P(v=0x12345678), \\
                lambda o: {{'v': o.v}}
        if C == 'P10':
            # 字节级 Padding
            @dataclass
            class P(StructMixin):
                tag: bytes = field(Bytes(1))
                reserved: int = wfield(Padding(4), default=None)
                data: bytes = field(Bytes(2))
            return P, b"\\xAA\\x00\\x00\\x00\\x00\\xBB\\xCC", \\
                lambda: P(tag=b"\\xAA", data=b"\\xBB\\xCC"), \\
                lambda o: {{'tag': o.tag, 'reserved': o.reserved, 'data': o.data}}
        if C == 'P11':
            # Bit 级 Padding (pattern=0x01)
            @dataclass
            class P(BitStructMixin):
                a: int = field(BitsInteger(4))
                pad: int = wfield(Padding(4, pattern=b"\\x01"), default=None)
            return P, b"\\x51", lambda: P(a=0x5), \\
                lambda o: {{'a': o.a, 'pad': o.pad}}
        if C == 'P12':
            # BitsInteger swapped (byte-order reverse within bit-string)
            # 16-bit: b"\x01\x02" BE=0x0102=258, swapped(LE)=0x0201=513
            @dataclass
            class P(BitStructMixin):
                v: int = field(BitsInteger(16, swapped=True))
            return P, b"\\x01\\x02", lambda: P(v=513), lambda o: {{'v': o.v}}
        if C == 'P13':
            # 复合场景：Bit + signed BitsInteger + Bytewise
            # b"\xE5\xAA\xBB":
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
                lambda o: {{'flag': o.flag, 'val': o.val, 'payload': o.payload}}
        raise ValueError('unknown case (rs): ' + CASE)

    def _make_case_py():
        import construct as pc

        C = CASE
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
        raise ValueError('unknown case (py): ' + CASE)

    # ---- 执行 ----
    parsed = None
    built = None
    roundtrip_parsed = None

    if IMPL == 'rs':
        cls, parse_data, build_factory, extract = _make_case()
        # parse
        parsed_obj = cls.parse(parse_data)
        parsed = extract(parsed_obj)
        # build
        built = build_factory().build()
        # round-trip: parse → build → parse
        reparsed = cls.parse(built)
        roundtrip_parsed = extract(reparsed)
    else:
        fmt, parse_data, build_input = _make_case()
        # parse
        parsed_obj = fmt.parse(parse_data)
        parsed = dict(parsed_obj) if hasattr(parsed_obj, 'items') else parsed_obj
        # build
        built = fmt.build(build_input)
        # round-trip
        reparsed = fmt.parse(built)
        roundtrip_parsed = dict(reparsed) if hasattr(reparsed, 'items') else reparsed

    print(json.dumps({{
        'impl': IMPL,
        'case': CASE,
        'parsed': _normalize(parsed),
        'built': built.hex(),
        'roundtrip_parsed': _normalize(roundtrip_parsed),
    }}))
    """
)


def _run_parity_case(impl: str, case: str) -> dict:
    """在子进程中执行一个 case，返回结果字典。"""
    code = _PARITY_SCRIPT.format(
        impl=impl,
        case=case,
        crs_python_dir=_CRS_PYTHON_DIR,
    )
    result = subprocess.run(
        [_PYTHON_EXE, "-c", code],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"parity 子进程失败 impl={impl} case={case}\n"
            f"stderr: {result.stderr[-2000:]}"
        )
    lines = [line for line in result.stdout.strip().splitlines() if line.strip()]
    if not lines:
        raise RuntimeError(
            f"parity 子进程无输出 impl={impl} case={case}\n"
            f"stderr: {result.stderr[-2000:]}"
        )
    try:
        return json.loads(lines[-1])
    except json.JSONDecodeError as e:
        raise RuntimeError(
            f"parity JSON 解析失败 impl={impl} case={case}\n"
            f"stdout: {result.stdout[-2000:]}\n"
            f"error: {e}"
        )


# ---------------------------------------------------------------------------
# 测试用例清单
# ---------------------------------------------------------------------------

# 每个 case 描述：覆盖的构造器、说明
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
# pytest 测试
# ---------------------------------------------------------------------------

@pytest.fixture(scope="module")
def parity_results():
    """预跑全部 case × impl，缓存结果供多个测试复用。

    一次性跑完所有子进程比每个测试单独跑更快（减少 pytest 收集开销）。
    """
    results = {}
    for case_id, _ in ALL_CASES:
        rs = _run_parity_case("rs", case_id)
        py = _run_parity_case("py", case_id)
        results[case_id] = {"rs": rs, "py": py}
    return results


def test_parity_all_cases_collected(parity_results):
    """确认所有 case 都成功执行（无子进程错误）。"""
    for case_id, _ in ALL_CASES:
        assert case_id in parity_results
        assert "rs" in parity_results[case_id]
        assert "py" in parity_results[case_id]


@pytest.mark.parametrize("case_id,desc", ALL_CASES)
def test_parity_parse(parity_results, case_id: str, desc: str):
    """Rust 和 Python construct 对相同输入 parse 应产生相同输出。"""
    rs = parity_results[case_id]["rs"]["parsed"]
    py = parity_results[case_id]["py"]["parsed"]
    assert rs == py, (
        f"case {case_id} ({desc}) parse 输出不一致\n"
        f"  Rust:   {rs}\n"
        f"  Python: {py}"
    )


@pytest.mark.parametrize("case_id,desc", ALL_CASES)
def test_parity_build(parity_results, case_id: str, desc: str):
    """Rust 和 Python construct 对相同输入 build 应产生相同字节。"""
    rs = parity_results[case_id]["rs"]["built"]
    py = parity_results[case_id]["py"]["built"]
    assert rs == py, (
        f"case {case_id} ({desc}) build 输出不一致\n"
        f"  Rust:   {rs}\n"
        f"  Python: {py}"
    )


@pytest.mark.parametrize("case_id,desc", ALL_CASES)
def test_parity_roundtrip(parity_results, case_id: str, desc: str):
    """Rust 和 Python construct 的 parse→build→parse 往返结果应一致。"""
    rs = parity_results[case_id]["rs"]["roundtrip_parsed"]
    py = parity_results[case_id]["py"]["roundtrip_parsed"]
    assert rs == py, (
        f"case {case_id} ({desc}) roundtrip 解析不一致\n"
        f"  Rust:   {rs}\n"
        f"  Python: {py}"
    )


@pytest.mark.parametrize("case_id,desc", ALL_CASES)
def test_parity_roundtrip_matches_first_parse(parity_results, case_id: str, desc: str):
    """parse→build→parse 的结果应与首次 parse 一致（往返保真）。

    分别检查 Rust 和 Python 各自的往返保真。
    """
    for impl in ("rs", "py"):
        first = parity_results[case_id][impl]["parsed"]
        roundtrip = parity_results[case_id][impl]["roundtrip_parsed"]
        assert first == roundtrip, (
            f"case {case_id} ({desc}) impl={impl} 往返保真失败\n"
            f"  first parse:    {first}\n"
            f"  roundtrip parse: {roundtrip}"
        )


# ---------------------------------------------------------------------------
# 主流程：直接 python 运行（不通过 pytest）时打印对比表
# ---------------------------------------------------------------------------

def _run_standalone():
    """直接执行（python tests/test_bitstream_parity.py）时的入口。

    打印对比表并返回 0/1 退出码。
    """
    print("=" * 78)
    print("Phase 3 BitStream 构造器 Python 行为一致性测试")
    print("=" * 78)
    print(f"Python: {_PYTHON_EXE}")
    print(f"construct-rs: {_CRS_PYTHON_DIR}")
    print()

    failures = 0
    header = (
        f"{'场景':<6} {'parse':<10} {'build':<10} "
        f"{'roundtrip':<10} {'保真':<10}"
    )
    print(header)
    print("-" * len(header))

    for case_id, desc in ALL_CASES:
        try:
            rs = _run_parity_case("rs", case_id)
            py = _run_parity_case("py", case_id)
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
