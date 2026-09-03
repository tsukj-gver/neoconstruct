"""Primitives / Strings / Adapter 集成性能基准。

组件：bench/_helpers/runner.py（BenchRunner）。

覆盖 24 个独立 Node × 5 场景 × parse/build = 240 测量点：
- Primitives（13）：VarInt / ZigZag / BytesInteger +
  Float16b/l + Float32b/l + Float64b/l + Int24ub/ul/sb/sl
- Strings（6）：CString / GreedyString / PaddedString / PascalString /
  NullTerminated / NullStripped
- Adapter（5）：Subconstruct / Peek / RawCopy / Rebuild / Pass

别名（Byte/Short/Int/Long/Half/Single/Double）是 FormatField / BytesInteger
的简单别名，不独立 bench（与 Int8ub/Int16ub 等基线一致）。

用户面 Adapter（Adapter/SymmetricAdapter）不设硬门禁，不进入本 bench
（已知折衷：用户面 Adapter 嵌入 Struct 有 2 次 FFI 回调开销）。

门禁：
- 内置 Adapter / Strings / Primitives：≥10x
- Pass：no-op，主要测 Struct 包装开销，≥10x 期望但允许边界场景退化

用法::

    python bench/bench_primitives_strings_adapter.py
    python bench/bench_primitives_strings_adapter.py --group primitives
    python bench/bench_primitives_strings_adapter.py --case V1 --case C1
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import textwrap
import time
from pathlib import Path

_BENCH_DIR = Path(__file__).resolve().parent
if str(_BENCH_DIR) not in sys.path:
    sys.path.insert(0, str(_BENCH_DIR))

from _helpers.report import (
    BenchReport,
    check_gates,
    format_results_table,
    make_environment_dict,
    make_summary,
)
from _helpers.runner import BenchConfig, BenchRunner
from _helpers.stats import speedup_ratio

# ---------------------------------------------------------------------------
# venv 解析（与 bench_struct.py 一致：环境变量 > 项目内 .venv/.venv-pc > sys.executable）
# ---------------------------------------------------------------------------
_PROJECT_ROOT = _BENCH_DIR.parent  # neoconstruct/
_DEFAULT_RS_PYTHON = _PROJECT_ROOT / ".venv"     # neoconstruct 扩展 venv
_DEFAULT_PY_PYTHON = _PROJECT_ROOT / ".venv-pc"  # 原版参考 venv（construct==2.10.70）
_CRS_PYTHON_DIR = str(_BENCH_DIR.parent / "python")


def _resolve_venv(env_var, venv_dir):
    """三级解析：环境变量 → 项目内 venv → sys.executable（与 conftest.py 一致）。"""
    env = os.environ.get(env_var)
    if env and Path(env).exists():
        return env
    for rel in ("Scripts/python.exe", "bin/python"):
        candidate = Path(venv_dir) / rel
        if candidate.exists():
            return str(candidate)
    return sys.executable


# ---------------------------------------------------------------------------
# 子进程测量脚本模板
# ---------------------------------------------------------------------------
# 设计要点：
# - 模板用 .format 替换 6 个占位符：impl / case / direction / number / repeat / crs_python_dir
# - 字面花括号用 {{ }} 转义
# - case 定义在 _MAKE_CASE 内，按 CASE 标识分发
_MEASURE_SCRIPT = textwrap.dedent(
    """
    import json
    import sys
    import timeit

    IMPL = {impl!r}
    CASE = {case!r}
    DIRECTION = {direction!r}
    NUMBER = {number!r}
    REPEAT = {repeat!r}
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

    # ---- case 工厂 ----
    {make_case_src}

    parse_target, build_target = _make_case(IMPL, CASE)
    target = parse_target if DIRECTION == 'parse' else build_target

    # 多次测量
    timer = timeit.Timer(target)
    times = timer.repeat(repeat=REPEAT, number=NUMBER)
    per_call_ns = [(t / NUMBER) * 1e9 for t in times]

    print(json.dumps({{
        'impl': IMPL,
        'case': CASE,
        'direction': DIRECTION,
        'per_call_ns': per_call_ns,
    }}))
    """
)


# ---------------------------------------------------------------------------
# Primitives case 工厂
# ---------------------------------------------------------------------------
# 设计：两个 impl 共享同一个 desc 变量名（通过 alias 对齐），case 内只引用统一符号
_PRIMITIVES_MAKE_CASE = r'''
def _make_case(impl, case):
    from dataclasses import dataclass
    if impl == 'rs':
        from neoconstruct import (
            StructMixin, field,
            VarInt, ZigZag, BytesInteger,
            Float16b, Float16l, Float32b, Float32l, Float64b, Float64l,
            Int24ub, Int24ul, Int24sb, Int24sl,
            Array,
        )
    else:
        import construct as pc
        StructMixin = pc.Struct
        Array = pc.Array
        VarInt = pc.VarInt
        ZigZag = pc.ZigZag
        BytesInteger = pc.BytesInteger
        Float16b = pc.Float16b
        Float16l = pc.Float16l
        Float32b = pc.Float32b
        Float32l = pc.Float32l
        Float64b = pc.Float64b
        Float64l = pc.Float64l
        Int24ub = pc.Int24ub
        Int24ul = pc.Int24ul
        Int24sb = pc.Int24sb
        Int24sl = pc.Int24sl

    C = case

    def single(desc, parse_data, build_val):
        """通用单字段 case：Struct{v: desc}。"""
        if impl == 'rs':
            @dataclass
            class S(StructMixin):
                v: object = field(desc)
            obj = S(v=build_val)
            return (lambda: S.parse(parse_data)), (lambda: obj.build())
        else:
            s = pc.Struct("v" / desc)
            obj_d = dict(v=build_val)
            return (lambda: s.parse(parse_data)), (lambda: s.build(obj_d))

    def multi(n_fields, desc, parse_data, build_vals):
        """多字段 case：Struct{a/b/c...: desc}。"""
        if impl == 'rs':
            field_names = [chr(ord('a') + i) for i in range(n_fields)]
            code_lines = ['@dataclass', 'class S(StructMixin):']
            for fn in field_names:
                code_lines.append(f'    {fn}: object = field(desc)')
            ns = {'dataclass': dataclass, 'StructMixin': StructMixin, 'field': field, 'desc': desc}
            exec('\n'.join(code_lines), ns)
            S = ns['S']
            kwargs = {fn: v for fn, v in zip(field_names, build_vals)}
            obj = S(**kwargs)
            return (lambda: S.parse(parse_data)), (lambda: obj.build())
        else:
            field_names = [chr(ord('a') + i) for i in range(n_fields)]
            s = pc.Struct(*[fn / desc for fn in field_names])
            obj_d = {fn: v for fn, v in zip(field_names, build_vals)}
            return (lambda: s.parse(parse_data)), (lambda: s.build(obj_d))

    def array_case(n, desc, parse_data, build_list):
        """Array 字段 case：Struct{v: Array(n, desc)}。"""
        if impl == 'rs':
            @dataclass
            class S(StructMixin):
                v: object = field(Array(n, desc))
            obj = S(v=build_list)
            return (lambda: S.parse(parse_data)), (lambda: obj.build())
        else:
            s = pc.Struct("v" / pc.Array(n, desc))
            obj_d = dict(v=build_list)
            return (lambda: s.parse(parse_data)), (lambda: s.build(obj_d))

    # ===== VarInt =====
    if C == 'V1':  # 单字节 1
        return single(VarInt, b"\x01", 1)
    if C == 'V2':  # 双字节 128
        return single(VarInt, b"\x80\x01", 128)
    if C == 'V3':  # 五字节 u32 max
        return single(VarInt, b"\xff\xff\xff\xff\x0f", 4294967295)
    if C == 'V4':  # 十字节 u64 max
        return single(VarInt, b"\xff\xff\xff\xff\xff\xff\xff\xff\xff\x01", 18446744073709551615)
    if C == 'V5':  # Array(N=100, VarInt) 单字节元素
        return array_case(100, VarInt, b"\x01" * 100, list(range(100)))

    # ===== ZigZag =====
    if C == 'Z1':  # 0
        return single(ZigZag, b"\x00", 0)
    if C == 'Z2':  # -1
        return single(ZigZag, b"\x01", -1)
    if C == 'Z3':  # +大数 (2^31 - 1)
        return single(ZigZag, b"\xfe\xff\xff\xff\x0f", 2147483647)
    if C == 'Z4':  # -大数
        return single(ZigZag, b"\xfd\xff\xff\xff\x0f", -2147483648)
    if C == 'Z5':  # Struct{a, b, c: ZigZag}
        return multi(3, ZigZag, b"\x05\x06\x00", [-3, 3, 0])

    # ===== BytesInteger =====
    if C == 'B1':  # 1 byte fast-path
        return single(BytesInteger(1), b"\x05", 5)
    if C == 'B2':  # 2 bytes
        return single(BytesInteger(2), b"\x01\x00", 256)
    if C == 'B3':  # 4 bytes
        return single(BytesInteger(4), b"\x00\x00\x00\x05", 5)
    if C == 'B4':  # 8 bytes fast-path 边界
        return single(BytesInteger(8), b"\x00\x00\x00\x00\x00\x00\x00\x05", 5)
    if C == 'B5':  # 16 bytes slow-path
        return single(BytesInteger(16),
                      b"\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x05", 5)

    # ===== Float 系列 (5 场景 × 6 编码 = 30 case) =====
    _float_values = {
        'F1': (1.0, b'\x3c\x00', b'\x00\x3c', b'\x3f\x80\x00\x00', b'\x00\x00\x80\x3f',
               b'\x3f\xf0\x00\x00\x00\x00\x00\x00', b'\x00\x00\x00\x00\x00\x00\xf0\x3f'),
        'F2': (3.14, b'\x42\x48', b'\x48\x42', b'\x40\x48\xf5\xc3', b'\xc3\xf5\x48\x40',
               b'\x40\x09\x1e\xb8\x51\xeb\x85\x1f', b'\x1f\x85\xeb\x51\xb8\x1e\x09\x40'),
        'F3': (-42.0, b'\xd1\x80', b'\x80\xd1', b'\xc2\x28\x00\x00', b'\x00\x00\x28\xc2',
               b'\xc0\x45\x00\x00\x00\x00\x00\x00', b'\x00\x00\x00\x00\x00\x50\x45\xc0'),
        'F4': (1e10, b'\x6f\x00', b'\x00\x6f', b'\x50\x15\x02\xf9', b'\xf9\x02\x15\x50',
               b'\x42\x02\xa0\x5f\x20\x00\x00\x00', b'\x00\x00\x00\x20\x5f\xa0\x02\x42'),
        'F5': (6.022e23, b'\x7b\x50', b'\x50\x7b', b'\x66\xed\x0c\x21', b'\x21\x0c\xed\x66',
               b'\x44\xc4\xd3\x35\x04\x00\x00\x00', b'\x00\x00\x00\x04\x35\xd3\xc4\x44'),
    }
    for _key, _vals in _float_values.items():
        _val, _b16b, _b16l, _b32b, _b32l, _b64b, _b64l = _vals
        if C == 'F16b-' + _key:
            return single(Float16b, _b16b, _val)
        if C == 'F16l-' + _key:
            return single(Float16l, _b16l, _val)
        if C == 'F32b-' + _key:
            return single(Float32b, _b32b, _val)
        if C == 'F32l-' + _key:
            return single(Float32l, _b32l, _val)
        if C == 'F64b-' + _key:
            return single(Float64b, _b64b, _val)
        if C == 'F64l-' + _key:
            return single(Float64l, _b64l, _val)

    # ===== Int24 系列 (4 编码 × 4 单值 + 1 Array = 17 case) =====
    _int24_values = {
        'I1': (0, b'\x00\x00\x00'),
        'I2': (65535, b'\x00\xff\xff'),
        'I3': (16777215, b'\xff\xff\xff'),
        'I4': (-1, b'\xff\xff\xff'),
    }
    for _key, (_val, _be_bytes) in _int24_values.items():
        _le_bytes = _be_bytes[::-1]
        if C == 'I24ub-' + _key:
            return single(Int24ub, _be_bytes, _val)
        if C == 'I24ul-' + _key:
            return single(Int24ul, _le_bytes, _val)
        if C == 'I24sb-' + _key:
            return single(Int24sb, _be_bytes, _val)
        if C == 'I24sl-' + _key:
            return single(Int24sl, _le_bytes, _val)
    if C == 'I24ub-ARR':
        return array_case(100, Int24ub, b'\x00\x00\x01' * 100, [1] * 100)

    raise ValueError('unknown primitives case: ' + case)
'''


# ---------------------------------------------------------------------------
# Strings case 工厂
# ---------------------------------------------------------------------------
_STRINGS_MAKE_CASE = r'''
def _make_strings_case(impl, case):
    from dataclasses import dataclass
    if impl == 'rs':
        from neoconstruct import (
            StructMixin, field,
            CString, GreedyString, PaddedString, PascalString,
            NullTerminated, NullStripped,
            Byte, Int16ub, Int8ub, Int32ub, Bytes, GreedyBytes, VarInt,
        )
    else:
        import construct as pc
        StructMixin = pc.Struct
        CString = pc.CString
        GreedyString = pc.GreedyString
        PaddedString = pc.PaddedString
        PascalString = pc.PascalString
        NullTerminated = pc.NullTerminated
        NullStripped = pc.NullStripped
        Byte = pc.Byte
        Int16ub = pc.Int16ub
        Int8ub = pc.Int8ub
        Int32ub = pc.Int32ub
        Bytes = pc.Bytes
        GreedyBytes = pc.GreedyBytes
        VarInt = pc.VarInt

    C = case

    def single(desc, parse_data, build_val):
        """通用单字段 case：Struct{v: desc}。"""
        if impl == 'rs':
            @dataclass
            class S(StructMixin):
                v: object = field(desc)
            obj = S(v=build_val)
            return (lambda: S.parse(parse_data)), (lambda: obj.build())
        else:
            s = pc.Struct("v" / desc)
            obj_d = dict(v=build_val)
            return (lambda: s.parse(parse_data)), (lambda: s.build(obj_d))

    # ===== CString (5 场景) =====
    if C == 'C1':
        return single(CString("utf8"), b"hi\x00", "hi")
    if C == 'C2':
        s_val = "abcdefgh" * 4
        return single(CString("utf8"), s_val.encode("utf8") + b"\x00", s_val)
    if C == 'C3':
        return single(CString("ascii"), b"hi\x00", "hi")
    if C == 'C4':
        return single(CString("utf_16_le"), b"h\x00i\x00\x00\x00", "hi")
    if C == 'C5':
        return single(CString("utf_16_be"), b"\x00h\x00i\x00\x00", "hi")

    # ===== GreedyString (5 场景) =====
    if C == 'G1':
        return single(GreedyString("utf8"), b"hi", "hi")
    if C == 'G2':
        s_val = "abcdefgh" * 8
        return single(GreedyString("utf8"), s_val.encode("utf8"), s_val)
    if C == 'G3':
        return single(GreedyString("ascii"), b"hello", "hello")
    if C == 'G4':
        return single(GreedyString("utf_16_le"), b"h\x00i\x00", "hi")
    if C == 'G5':
        return single(GreedyString("utf_16_be"), b"\x00h\x00i", "hi")

    # ===== PaddedString (5 场景) =====
    if C == 'P1':
        return single(PaddedString(10, "utf8"), b"hello\x00\x00\x00\x00\x00", "hello")
    if C == 'P2':
        s_val = "a" * 50
        return single(PaddedString(100, "utf8"), s_val.encode("utf8") + b"\x00" * 50, s_val)
    if C == 'P3':
        return single(PaddedString(10, "ascii"), b"hi\x00\x00\x00\x00\x00\x00\x00\x00", "hi")
    if C == 'P4':
        return single(PaddedString(8, "utf_16_le"), b"h\x00i\x00\x00\x00\x00\x00", "hi")
    if C == 'P5':
        return single(PaddedString(8, "utf_16_be"), b"\x00h\x00i\x00\x00\x00\x00", "hi")

    # ===== PascalString (5 场景) =====
    if C == 'PS1':
        return single(PascalString(Byte, "utf8"), b"\x05hello", "hello")
    if C == 'PS2':
        s_val = "a" * 200
        return single(PascalString(Byte, "utf8"), bytes([200]) + s_val.encode("utf8"), s_val)
    if C == 'PS3':
        return single(PascalString(Int16ub, "utf8"), b"\x00\x05hello", "hello")
    if C == 'PS4':
        return single(PascalString(VarInt, "utf8"), b"\x05hello", "hello")
    if C == 'PS5':
        return single(PascalString(Byte, "utf_16_le"), b"\x02h\x00i\x00", "hi")

    # ===== NullTerminated (5 场景) =====
    if C == 'NT1':
        return single(NullTerminated(GreedyBytes), b"hi\x00", b"hi")
    if C == 'NT2':
        data_val = b"abcdefgh" * 8
        return single(NullTerminated(GreedyBytes), data_val + b"\x00", data_val)
    if C == 'NT3':
        return single(NullTerminated(Byte), b"\x42\x00", 0x42)
    if C == 'NT4':
        return single(NullTerminated(GreedyBytes, term=b"\xff"), b"hi\xff", b"hi")
    if C == 'NT5':
        return single(NullTerminated(Int32ub), b"\x00\x00\x00\x05\x00", 5)

    # ===== NullStripped (5 场景) =====
    if C == 'NS1':
        return single(NullStripped(GreedyBytes), b"abc\x00\x00", b"abc")
    if C == 'NS2':
        return single(NullStripped(GreedyBytes, pad=b"\x00\x00"), b"ab\x00\x00\x00\x00", b"ab")
    if C == 'NS3':
        return single(NullStripped(Byte), b"\x42\x00\x00", 0x42)
    if C == 'NS4':
        return single(NullStripped(Bytes(4)), b"\x01\x02\x03\x04\x00\x00\x00\x00", b"\x01\x02\x03\x04")
    if C == 'NS5':
        return single(NullStripped(Int32ub), b"\x00\x00\x00\x05\x00\x00\x00\x00", 5)

    raise ValueError('unknown strings case: ' + case)
'''


# ---------------------------------------------------------------------------
# Adapter case 工厂
# ---------------------------------------------------------------------------
_ADAPTER_MAKE_CASE = r'''
def _make_adapter_case(impl, case):
    from dataclasses import dataclass
    if impl == 'rs':
        from neoconstruct import (
            StructMixin, field, rfield,
            Subconstruct, Peek, RawCopy, Rebuild, Pass,
            Byte, Int16ub, Int8ub, Int32ub, Bytes, Array,
        )
    else:
        import construct as pc
        StructMixin = pc.Struct
        Subconstruct = pc.Subconstruct
        Peek = pc.Peek
        RawCopy = pc.RawCopy
        Rebuild = pc.Rebuild
        Pass = pc.Pass
        Byte = pc.Byte
        Int16ub = pc.Int16ub
        Int8ub = pc.Int8ub
        Int32ub = pc.Int32ub
        Bytes = pc.Bytes
        Array = pc.Array
        this = pc.this

    C = case

    # helper: 单字段 case（v: desc）
    def single(desc, parse_data, build_val):
        if impl == 'rs':
            @dataclass
            class S(StructMixin):
                v: object = field(desc)
            obj = S(v=build_val)
            return (lambda: S.parse(parse_data)), (lambda: obj.build())
        else:
            s = pc.Struct("v" / desc)
            obj_d = dict(v=build_val)
            return (lambda: s.parse(parse_data)), (lambda: s.build(obj_d))

    # helper: 多字段 case（字段名 a/b/c...）
    def multi(field_specs, parse_data, build_kwargs):
        """field_specs: [(name, desc_or_special, is_rfield)]."""
        if impl == 'rs':
            code_lines = ['@dataclass', 'class S(StructMixin):']
            ns = {'dataclass': dataclass, 'StructMixin': StructMixin,
                  'field': field, 'rfield': rfield}
            for fname, desc, is_rfield in field_specs:
                kw = 'rfield' if is_rfield else 'field'
                code_lines.append(f'    {fname}: object = {kw}({fname}_desc)')
                ns[f'{fname}_desc'] = desc
            exec('\n'.join(code_lines), ns)
            S = ns['S']
            obj = S(**build_kwargs)
            return (lambda: S.parse(parse_data)), (lambda: obj.build())
        else:
            # py 端：rfield 当 field 处理
            subcons = []
            for fname, desc, is_rfield in field_specs:
                subcons.append(fname / desc)
            s = pc.Struct(*subcons)
            obj_d = dict(build_kwargs)
            return (lambda: s.parse(parse_data)), (lambda: s.build(obj_d))

    # ===== Subconstruct (5 场景) =====
    if C == 'SC1':
        return single(Subconstruct(Int8ub), b"\x42", 0x42)
    if C == 'SC2':
        return single(Subconstruct(Int32ub), b"\x00\x00\x00\x05", 5)
    if C == 'SC3':
        data = bytes(range(8))
        return single(Subconstruct(Bytes(8)), data, data)
    if C == 'SC4':
        return multi(
            [('a', Subconstruct(Int8ub), False), ('b', Subconstruct(Int8ub), False)],
            b"\x01\x02",
            dict(a=1, b=2),
        )
    if C == 'SC5':
        data = bytes(range(100))
        return single(Array(100, Subconstruct(Int8ub)), data, list(range(100)))

    # ===== Peek (5 场景) =====
    if C == 'PK1':
        return multi(
            [('a', Peek(Int8ub), False), ('b', Int8ub, False)],
            b"\x10",
            dict(a=0x10, b=0x10),
        )
    if C == 'PK2':
        data = b"\x00\x00\x00\x05"
        return multi(
            [('a', Peek(Int32ub), False), ('b', Bytes(4), False)],
            data,
            dict(a=5, b=data),
        )
    if C == 'PK3':
        return multi(
            [('a', Peek(Int8ub), False), ('b', Peek(Int8ub), False), ('c', Int8ub, False)],
            b"\x10",
            dict(a=0x10, b=0x10, c=0x10),
        )
    if C == 'PK4':
        return multi(
            [('a', Peek(Bytes(5)), False), ('b', Int8ub, False)],
            b"\x99",
            dict(a=None, b=0x99),
        )
    if C == 'PK5':
        return multi(
            [('a', Peek(Int16ub), False), ('b', Int16ub, False)],
            b"\x01\x02",
            dict(a=0x0102, b=0x0102),
        )

    # ===== RawCopy (5 场景) =====
    if C == 'RC1':
        return single(RawCopy(Int8ub), b"\xff", {"data": b"\xff"})
    if C == 'RC2':
        data = b"\x00\x00\x00\x05"
        return single(RawCopy(Int32ub), data, {"data": data})
    if C == 'RC3':
        data = bytes(range(8))
        return single(RawCopy(Bytes(8)), data, {"data": data})
    if C == 'RC4':
        return multi(
            [('a', RawCopy(Int8ub), False), ('b', Int8ub, False)],
            b"\x01\x02",
            dict(a={"data": b"\x01"}, b=0x02),
        )
    if C == 'RC5':
        data = bytes(range(100))
        return single(Array(100, RawCopy(Int8ub)), data,
                      [{"data": bytes([i])} for i in range(100)])

    # ===== Rebuild (5 场景) =====
    # Rebuild 必须作为 RO 字段（rfield 包装），build 时由表达式重算
    # 表达式 func 必须是 FieldRef（_FieldDescriptor 对象，即其他字段的 field() 返回值）
    if C == 'RB1':
        # Struct{n: Int8ub, count: Rebuild(Int8ub, n)}
        if impl == 'rs':
            n_field = field(Int8ub)
            @dataclass
            class S(StructMixin):
                n: object = n_field
                count: object = rfield(Rebuild(Int8ub, n_field))
            data = b"\x05\x05"
            obj = S(n=7)
            return (lambda: S.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("n" / pc.Int8ub, "count" / pc.Rebuild(pc.Int8ub, this.n))
            data = b"\x05\x05"
            obj_d = dict(n=7, count=0xAA)
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))
    if C == 'RB2':
        if impl == 'rs':
            n_field = field(Int8ub)
            @dataclass
            class S(StructMixin):
                n: object = n_field
                count: object = rfield(Rebuild(Int16ub, n_field))
            data = b"\x05\x00\x05"
            obj = S(n=7)
            return (lambda: S.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("n" / pc.Int8ub, "count" / pc.Rebuild(pc.Int16ub, this.n))
            data = b"\x05\x00\x05"
            obj_d = dict(n=7, count=0xAA)
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))
    if C == 'RB3':
        if impl == 'rs':
            n_field = field(Int8ub)
            @dataclass
            class S(StructMixin):
                n: object = n_field
                count: object = rfield(Rebuild(Int32ub, n_field))
            data = b"\x05\x00\x00\x00\x05"
            obj = S(n=7)
            return (lambda: S.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("n" / pc.Int8ub, "count" / pc.Rebuild(pc.Int32ub, this.n))
            data = b"\x05\x00\x00\x00\x05"
            obj_d = dict(n=7, count=0xAA)
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))
    if C == 'RB4':
        if impl == 'rs':
            n_field = field(Int8ub)
            @dataclass
            class S(StructMixin):
                n: object = n_field
                count: object = rfield(Rebuild(Int8ub, n_field))
                pad: object = field(Int8ub)
            data = b"\x05\x05\x00"
            obj = S(n=7, pad=0xFF)
            return (lambda: S.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("n" / pc.Int8ub, "count" / pc.Rebuild(pc.Int8ub, this.n), "pad" / pc.Int8ub)
            data = b"\x05\x05\x00"
            obj_d = dict(n=7, count=0xAA, pad=0xFF)
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))
    if C == 'RB5':
        if impl == 'rs':
            n_field = field(Int8ub)
            @dataclass
            class S(StructMixin):
                n: object = n_field
                count: object = rfield(Rebuild(Int8ub, n_field))
                data: object = field(Bytes(4))
            data_bytes = b"\x05\x05abcd"
            obj = S(n=7, data=b"abcd")
            return (lambda: S.parse(data_bytes)), (lambda: obj.build())
        else:
            s = pc.Struct("n" / pc.Int8ub, "count" / pc.Rebuild(pc.Int8ub, this.n), "data" / pc.Bytes(4))
            data_bytes = b"\x05\x05abcd"
            obj_d = dict(n=7, count=0xAA, data=b"abcd")
            return (lambda: s.parse(data_bytes)), (lambda: s.build(obj_d))

    # ===== Pass (5 场景) =====
    if C == 'PA1':
        return multi(
            [('x', Int8ub, False), ('p', Pass, False)],
            b"\x42",
            dict(x=0x42, p=None),
        )
    if C == 'PA2':
        return multi(
            [('p', Pass, False), ('x', Int8ub, False)],
            b"\x42",
            dict(p=None, x=0x42),
        )
    if C == 'PA3':
        return multi(
            [('x', Int8ub, False), ('p', Pass, False), ('y', Int8ub, False)],
            b"\x01\x02",
            dict(x=1, p=None, y=2),
        )
    if C == 'PA4':
        return multi(
            [('p', Pass, False)],
            b"",
            dict(p=None),
        )
    if C == 'PA5':
        return multi(
            [('a', Int8ub, False), ('b', Pass, False), ('c', Pass, False), ('d', Int8ub, False)],
            b"\x01\x02",
            dict(a=1, b=None, c=None, d=2),
        )

    raise ValueError('unknown adapter case: ' + case)
'''


# ---------------------------------------------------------------------------
# 场景清单（24 个独立 Node × 5 场景）
# ---------------------------------------------------------------------------
# 每个 case 归属一个构造器（用于报告分组 + CSV 行）
# (case_id, constructor, group, desc, path_type, scale)
PRIMITIVES_CASES = [
    # VarInt
    ("V1", "VarInt", "primitives", "VarInt(1) 单字节", "standalone", "N=1"),
    ("V2", "VarInt", "primitives", "VarInt(128) 双字节", "standalone", "N=1"),
    ("V3", "VarInt", "primitives", "VarInt(u32 max) 五字节", "standalone", "N=1"),
    ("V4", "VarInt", "primitives", "VarInt(u64 max) 十字节", "standalone", "N=1"),
    ("V5", "VarInt", "primitives", "Array(100, VarInt) N=100", "nested_in_struct", "N=100"),
    # ZigZag
    ("Z1", "ZigZag", "primitives", "ZigZag(0)", "standalone", "N=1"),
    ("Z2", "ZigZag", "primitives", "ZigZag(-1)", "standalone", "N=1"),
    ("Z3", "ZigZag", "primitives", "ZigZag(2^31-1)", "standalone", "N=1"),
    ("Z4", "ZigZag", "primitives", "ZigZag(-2^31)", "standalone", "N=1"),
    ("Z5", "ZigZag", "primitives", "Struct{a,b,c: ZigZag}", "standalone", "N=3"),
    # BytesInteger
    ("B1", "BytesInteger", "primitives", "BytesInteger(1) fast-path", "standalone", "N=1"),
    ("B2", "BytesInteger", "primitives", "BytesInteger(2)", "standalone", "N=1"),
    ("B3", "BytesInteger", "primitives", "BytesInteger(4)", "standalone", "N=1"),
    ("B4", "BytesInteger", "primitives", "BytesInteger(8) fast-path 边界", "standalone", "N=1"),
    ("B5", "BytesInteger", "primitives", "BytesInteger(16) slow-path", "standalone", "N=1"),
]

# Float 系列：6 编码 × 5 场景 = 30 case
_FLOAT_VALS_KEYS = ["F1", "F2", "F3", "F4", "F5"]
_FLOAT_ENC_DESC = {
    "F16b": ("Float16b", "Float16b 半精度 BE"),
    "F16l": ("Float16l", "Float16l 半精度 LE"),
    "F32b": ("Float32b", "Float32b 单精度 BE"),
    "F32l": ("Float32l", "Float32l 单精度 LE"),
    "F64b": ("Float64b", "Float64b 双精度 BE"),
    "F64l": ("Float64l", "Float64l 双精度 LE"),
}
for _enc, (_constructor, _desc) in _FLOAT_ENC_DESC.items():
    for _vk in _FLOAT_VALS_KEYS:
        PRIMITIVES_CASES.append((
            f"{_enc}-{_vk}", _constructor, "primitives",
            f"{_desc} 场景 {_vk}", "standalone", "N=1",
        ))

# Int24 系列：4 编码 × 4 单值 + 1 Array = 17 case
_INT24_DESC = {
    "I24ub": ("Int24ub", "Int24ub 3 字节无符号 BE"),
    "I24ul": ("Int24ul", "Int24ul 3 字节无符号 LE"),
    "I24sb": ("Int24sb", "Int24sb 3 字节有符号 BE"),
    "I24sl": ("Int24sl", "Int24sl 3 字节有符号 LE"),
}
for _enc, (_constructor, _desc) in _INT24_DESC.items():
    # I1/I2/I3 + I4 (signed 专用 -1)
    for _key in ["I1", "I2", "I3"]:
        PRIMITIVES_CASES.append((
            f"{_enc}-{_key}", _constructor, "primitives",
            f"{_desc} 场景 {_key}", "standalone", "N=1",
        ))
    # I4 仅 signed
    if _enc in ("I24sb", "I24sl"):
        PRIMITIVES_CASES.append((
            f"{_enc}-I4", _constructor, "primitives",
            f"{_desc} 场景 I4 (-1)", "standalone", "N=1",
        ))
# Int24ub Array(N=100)
PRIMITIVES_CASES.append((
    "I24ub-ARR", "Int24ub", "primitives",
    "Array(100, Int24ub) N=100", "nested_in_struct", "N=100",
))

# Strings 6 个 Node × 5 场景 = 30 case
_STRINGS_DESC = {
    "C": ("CString", "CString"),
    "G": ("GreedyString", "GreedyString"),
    "P": ("PaddedString", "PaddedString"),
    "PS": ("PascalString", "PascalString"),
    "NT": ("NullTerminated", "NullTerminated"),
    "NS": ("NullStripped", "NullStripped"),
}
STRINGS_CASES = []
for _prefix, (_constructor, _label) in _STRINGS_DESC.items():
    for _i in range(1, 6):
        STRINGS_CASES.append((
            f"{_prefix}{_i}", _constructor, "strings",
            f"{_label} 场景 {_i}", "standalone", "N=1",
        ))

# Adapter 5 个 Node × 5 场景 = 25 case
_ADAPTER_DESC = {
    "SC": ("Subconstruct", "Subconstruct"),
    "PK": ("Peek", "Peek"),
    "RC": ("RawCopy", "RawCopy"),
    "RB": ("Rebuild", "Rebuild"),
    "PA": ("Pass", "Pass"),
}
ADAPTER_CASES = []
for _prefix, (_constructor, _label) in _ADAPTER_DESC.items():
    for _i in range(1, 6):
        ADAPTER_CASES.append((
            f"{_prefix}{_i}", _constructor, "adapter",
            f"{_label} 场景 {_i}",
            "nested_in_struct" if _prefix in ("PK", "RC", "RB", "PA") else "standalone",
            "N=1",
        ))

ALL_BENCH_CASES = PRIMITIVES_CASES + STRINGS_CASES + ADAPTER_CASES

# 门禁：全部构造器硬门禁 ≥10x
GATES = {case_id: 10.0 for case_id, *_ in ALL_BENCH_CASES}


def _resolve_make_case_src(group):
    """根据 group 返回对应的 _make_case 源代码片段。"""
    if group == "primitives":
        return _PRIMITIVES_MAKE_CASE
    if group == "strings":
        return _STRINGS_MAKE_CASE
    if group == "adapter":
        return _ADAPTER_MAKE_CASE
    raise ValueError(f"unknown group: {group}")


def _resolve_case_dispatcher(group):
    """根据 group 返回 case 分派函数名（用于脚本内的 _make_case 调用）。"""
    if group == "primitives":
        return "_make_case"
    if group == "strings":
        return "_make_strings_case"
    if group == "adapter":
        return "_make_adapter_case"
    raise ValueError(f"unknown group: {group}")


def main():
    parser = argparse.ArgumentParser(description="neoconstruct Primitives/Strings/Adapter 性能基准测试")
    parser.add_argument("--group", action="append",
                        choices=["primitives", "strings", "adapter"],
                        help="仅测量指定分组（可多次指定），默认全部")
    parser.add_argument("--case", action="append",
                        help="仅测量指定 case（可多次指定）")
    parser.add_argument("--constructor", action="append",
                        help="仅测量指定 constructor（可多次指定，如 VarInt）")
    parser.add_argument("--iterations", type=int, default=10,
                        help="每次测量 iteration 数（默认 10）")
    parser.add_argument("--warmup", type=int, default=3, help="预热次数（默认 3）")
    parser.add_argument("--number", type=int, default=5000,
                         help="timeit number（默认 5000）")
    args = parser.parse_args()

    # 过滤 case
    selected = ALL_BENCH_CASES
    if args.group:
        selected = [c for c in selected if c[2] in args.group]
    if args.case:
        selected = [c for c in selected if c[0] in args.case]
    if args.constructor:
        selected = [c for c in selected if c[1] in args.constructor]
    if not selected:
        print("[ERROR] 没有匹配的 case")
        return 1

    config = BenchConfig(
        iterations=args.iterations,
        warmup=args.warmup,
        number=args.number,
    )

    rs_python = _resolve_venv("CRS_PYTHON", _DEFAULT_RS_PYTHON)
    py_python = _resolve_venv("PC_PYTHON", _DEFAULT_PY_PYTHON)

    runner = BenchRunner(rs_python, py_python, _CRS_PYTHON_DIR, config=config)

    print("=" * 78)
    print("集成性能基准（Primitives + Strings + Adapter）")
    print("=" * 78)
    print(f"Python (rs): {rs_python}")
    print(f"Python (py): {py_python}")
    print(f"number={config.number}, iterations={config.iterations}, warmup={config.warmup}")
    print(f"Selected: {len(selected)} cases × parse+build = {len(selected)*2} 测量点")
    print()

    all_results = []
    paired_for_summary = []
    csv_rows = []
    failures = []

    for case_id, constructor, group, desc, path_type, scale in selected:
        for direction in ("parse", "build"):
            scenario = f"{case_id}-{direction}"
            repeat = config.iterations + config.warmup
            dispatcher = _resolve_case_dispatcher(group)
            make_case_src = _resolve_make_case_src(group)
            print(f"  测量 {scenario:<24}...", end="", flush=True)
            try:
                rs_result = runner.run_scenario(
                    "rs", scenario,
                    _build_script("rs", case_id, direction, config.number, repeat,
                                  _CRS_PYTHON_DIR, make_case_src, dispatcher),
                )
                py_result = runner.run_scenario(
                    "py", scenario,
                    _build_script("py", case_id, direction, config.number, repeat,
                                  _CRS_PYTHON_DIR, make_case_src, dispatcher),
                )
            except RuntimeError as e:
                # 错误消息可能含非 GBK 字符（PowerShell 默认 GBK 输出），用 ascii 安全打印
                msg = str(e).encode("ascii", errors="replace").decode("ascii")
                print(f" [ERROR] {msg[:300]}")
                failures.append((scenario, msg))
                continue

            all_results.append(rs_result)
            all_results.append(py_result)
            sp = speedup_ratio(rs_result.median_ns, py_result.median_ns)
            paired_for_summary.append({"rs": rs_result, "py": py_result, "speedup": sp})
            csv_rows.append({
                "case_id": case_id,
                "constructor": constructor,
                "group": group,
                "desc": desc,
                "direction": direction,
                "path_type": path_type,
                "scale": scale,
                "py_ns": py_result.median_ns,
                "rs_ns": rs_result.median_ns,
                "speedup": sp,
            })
            mark = "OK" if sp >= 10.0 else "LOW"
            print(f" rs={rs_result.median_ns:.0f}ns py={py_result.median_ns:.0f}ns "
                  f"speedup={sp:.2f}x [{mark}]")

    print()
    print("对比表（前 30 行）：")
    print(format_results_table(all_results[:60]))

    # 构造报告
    report = BenchReport(
        timestamp=time.strftime("%Y-%m-%dT%H:%M:%S"),
        environment=make_environment_dict(config),
        results=all_results,
        summary=make_summary(paired_for_summary),
    )
    report.summary["failed_gates"] = [scenario for scenario, _ in failures]

    # 门禁检查
    gate_failures = check_gates(report, GATES)
    print()
    print(f"门禁检查（≥10x）：{len(all_results)//2 - len(gate_failures)}/{len(all_results)//2} PASS")
    if gate_failures:
        print(f"  {len(gate_failures)} 项未达标：")
        for f in gate_failures[:20]:
            print(f"    {f}")
        if len(gate_failures) > 20:
            print(f"    ... 还有 {len(gate_failures) - 20} 项")

    # 写 CSV 行（供追加到 perf-scenarios.csv）
    csv_path = _BENCH_DIR / "results" / "bench_primitives_strings_adapter_csv.json"
    csv_path.parent.mkdir(parents=True, exist_ok=True)
    with csv_path.open("w", encoding="utf-8") as f:
        json.dump(csv_rows, f, ensure_ascii=False, indent=2)
    print(f"\nCSV 数据（供追加 perf-scenarios.csv）：{csv_path}")

    # 写完整报告
    report_path = _BENCH_DIR / "results" / "bench_primitives_strings_adapter"
    report.write(report_path)
    print(f"详细结果：{report_path}.json / .md")

    return 0 if not gate_failures else 2


# 占位常量（脚本生成用；子进程测量脚本统一由 _build_script 生成）


def _build_script(impl, case_id, direction, number, repeat, crs_python_dir,
                  make_case_src, dispatcher):
    """生成完整的子进程测量脚本（已替换占位符）。"""
    # 注入 sys.path 操纵 + dispatcher 调用
    full = f'''
import json
import sys
import timeit

IMPL = {impl!r}
CASE = {case_id!r}
DIRECTION = {direction!r}
NUMBER = {number!r}
REPEAT = {repeat!r}
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

{make_case_src}

parse_target, build_target = {dispatcher}(IMPL, CASE)
target = parse_target if DIRECTION == 'parse' else build_target

timer = timeit.Timer(target)
times = timer.repeat(repeat=REPEAT, number=NUMBER)
per_call_ns = [(t / NUMBER) * 1e9 for t in times]

print(json.dumps({{
    'impl': IMPL,
    'case': CASE,
    'direction': DIRECTION,
    'per_call_ns': per_call_ns,
}}))
'''
    return full


if __name__ == "__main__":
    sys.exit(main())

