"""Adapters / Struct / Streams 性能基准（Controlled A/B Test vs Python construct 2.10.70）。

覆盖全部构造器（P0 + P1P2），每构造器 2-3 场景 × parse/build。

P0（8 构造器）:
- Const / Default / Check
- Hex / HexDump
- Checksum（B1 零拷贝 vs Python RawCopy+callable）
- Aligned
- Terminated

P1P2（10 构造器）:
- Enum / FlagsEnum / Mapping
- OneOf / NoneOf
- Union
- Sequence
- ProcessXor / ProcessRotateLeft
- NamedTuple

门禁：默认 ≥10x。
关注点（可能 <10x）：Hex（显示类构造）、Enum（dict lookup）。

测量口径：
- 子进程隔离（rs venv vs py venv）
- ≥5 次采样，报告 min/max/mean/stddev
- 绝对基线：Python construct==2.10.70

用法::

    python bench/bench_adapters_struct_streams.py
    python bench/bench_adapters_struct_streams.py --group p0
    python bench/bench_adapters_struct_streams.py --case CS1
"""

from __future__ import annotations

import argparse
import json
import os
import sys
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
# venv 解析（与其他 bench 脚本一致：环境变量 > 项目内 .venv/.venv-pc > sys.executable）
# ---------------------------------------------------------------------------
_PROJECT_ROOT = _BENCH_DIR.parent  # construct-rs/
_DEFAULT_RS_PYTHON = _PROJECT_ROOT / ".venv"     # construct-rs 扩展 venv
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
# P0 case 工厂
# ---------------------------------------------------------------------------
_P0_MAKE_CASE = r'''
def _make_p0_case(impl, case):
    from dataclasses import dataclass
    if impl == 'rs':
        from construct import (
            StructMixin, field, rfield,
            Const, Default, Check, Hex, HexDump, Checksum, HashAlgo,
            Aligned, Terminated, Tell,
            Byte, Int8ub, Int16ub, Int32ub, Int32ul, Bytes,
        )
    else:
        import construct as pc
        StructMixin = pc.Struct
        def field(d): return d
        def rfield(d): return d
        Const = pc.Const
        Hex = pc.Hex
        HexDump = pc.HexDump
        Aligned = pc.Aligned
        Terminated = pc.Terminated
        Byte = pc.Byte
        Int8ub = pc.Int8ub
        Int16ub = pc.Int16ub
        Int32ub = pc.Int32ub
        Int32ul = pc.Int32ul
        Bytes = pc.Bytes
        this = pc.this
        import hashlib

    C = case

    def mk_rs(fields_code, parse_data, build_kwargs_src):
        """动态构造 rs StructMixin dataclass。"""
        lines = ['@dataclass', 'class S(StructMixin):']
        ns = {'dataclass': dataclass, 'StructMixin': StructMixin,
              'field': field, 'rfield': rfield,
              'Const': Const, 'Default': Default, 'Check': Check,
              'Hex': Hex, 'HexDump': HexDump,
              'Aligned': Aligned, 'Terminated': Terminated, 'Tell': Tell,
              'Byte': Byte, 'Int8ub': Int8ub, 'Int16ub': Int16ub,
              'Int32ub': Int32ub, 'Int32ul': Int32ul, 'Bytes': Bytes}
        exec('\n'.join(lines + fields_code), ns)
        S = ns['S']
        build_kwargs = eval(build_kwargs_src, ns)
        obj = S(**build_kwargs)
        return (lambda: S.parse(parse_data)), (lambda: obj.build())

    def mk_py(subcons, parse_data, build_dict):
        s = pc.Struct(*subcons)
        obj_d = dict(build_dict)
        return (lambda: s.parse(parse_data)), (lambda: s.build(obj_d))

    # ===== Const =====
    if C == 'CN1':  # Const bytes
        if impl == 'rs':
            return mk_rs(['    m: bytes = field(Const(b"IHDR"))'],
                         b"IHDR", "dict(m=b'IHDR')")
        else:
            return mk_py(["m" / Const(b"IHDR")], b"IHDR", dict(m=None))
    if C == 'CN2':  # Const int
        if impl == 'rs':
            return mk_rs(['    m: int = field(Const(255, Int32ul))'],
                         b'\xff\x00\x00\x00', "dict(m=255)")
        else:
            return mk_py(["m" / Const(255, Int32ul)], b'\xff\x00\x00\x00', dict(m=None))

    # ===== Default =====
    # DF1: Default(Byte, 0) with bare int constant —— int 常量由
    # _extract_and_compile_exprs 编译为 Const ExprOp。
    if C == 'DF1':  # Default constant value (obj=None triggers default 0)
        if impl == 'rs':
            return mk_rs(['    val: int = field(Default(Byte, 0))'],
                         b'\x00', "dict(val=None)")
        else:
            return mk_py(["val" / pc.Default(Byte, 0)], b'\x00', dict(val=None))
    if C == 'DF2':  # Default with field reference expression (obj=None triggers expr eval)
        if impl == 'rs':
            return mk_rs(['    count: int = field(Byte)',
                          '    val: int = field(Default(Byte, count))'],
                         b'\x07\x07', "dict(count=7, val=None)")
        else:
            return mk_py(["count" / Byte, "val" / pc.Default(Byte, this.count)],
                         b'\x07\x07', dict(count=7, val=None))

    # ===== Check =====
    if C == 'CK1':  # Check passes
        if impl == 'rs':
            return mk_rs(['    a: int = field(Byte)',
                          '    b: int = field(Byte)',
                          '    _c: object = rfield(Check(a + b > 0))'],
                         b'\x01\x02', "dict(a=1, b=2)")
        else:
            return mk_py(["a" / Byte, "b" / Byte, "c" / pc.Check(this.a + this.b > 0)],
                         b'\x01\x02', dict(a=1, b=2, c=None))

    # ===== Hex =====
    if C == 'HX1':  # Hex int
        if impl == 'rs':
            return mk_rs(['    v: object = field(Hex(Int32ub))'],
                         b'\x00\x00\x01\x02', "dict(v=258)")
        else:
            return mk_py(["v" / Hex(Int32ub)], b'\x00\x00\x01\x02', dict(v=258))
    if C == 'HX2':  # Hex bytes
        if impl == 'rs':
            return mk_rs(['    v: object = field(Hex(Bytes(4)))'],
                         b'\x00\x00\x01\x02', "dict(v=b'\\x00\\x00\\x01\\x02')")
        else:
            return mk_py(["v" / Hex(Bytes(4))], b'\x00\x00\x01\x02',
                         dict(v=b'\x00\x00\x01\x02'))

    # ===== HexDump =====
    if C == 'HD1':  # HexDump bytes
        if impl == 'rs':
            return mk_rs(['    v: object = field(HexDump(Bytes(4)))'],
                         b'\x00\x00\x01\x02', "dict(v=b'\\x00\\x00\\x01\\x02')")
        else:
            return mk_py(["v" / HexDump(Bytes(4))], b'\x00\x00\x01\x02',
                         dict(v=b'\x00\x00\x01\x02'))

    # ===== Aligned =====
    if C == 'AL1':  # Aligned modulus=4 Int16ub
        if impl == 'rs':
            return mk_rs(['    v: int = field(Aligned(4, Int16ub))'],
                         b'\x00\x01\x00\x00', "dict(v=1)")
        else:
            return mk_py(["v" / Aligned(4, Int16ub)], b'\x00\x01\x00\x00', dict(v=1))
    if C == 'AL2':  # Aligned modulus=8 Bytes(3)
        if impl == 'rs':
            return mk_rs(['    v: bytes = field(Aligned(8, Bytes(3)))'],
                         b'\x01\x02\x03' + b'\x00'*5, "dict(v=b'\\x01\\x02\\x03')")
        else:
            return mk_py(["v" / Aligned(8, Bytes(3))],
                         b'\x01\x02\x03' + b'\x00'*5, dict(v=b'\x01\x02\x03'))

    # ===== Terminated =====
    if C == 'TM1':  # Terminated at EOF
        # rfield(Terminated) 支持：compute_ro_value 对 Terminated 返回
        # Py_None（build 是 no-op）。
        if impl == 'rs':
            return mk_rs(['    v: int = field(Byte)',
                          '    _t: object = rfield(Terminated)'],
                         b'\x01', "dict(v=1)")
        else:
            return mk_py(["v" / Byte, Terminated], b'\x01', dict(v=1))

    raise ValueError('unknown p0 case: ' + case)
'''


# ---------------------------------------------------------------------------
# Checksum 专用 case 工厂（双轨：rs B1 零拷贝 vs py RawCopy+callable）
# ---------------------------------------------------------------------------
_CS_MAKE_CASE = r'''
def _make_cs_case(impl, case):
    import hashlib
    C = case

    # data_size 参数
    if C == 'CS1':
        n = 64
    elif C == 'CS2':
        n = 1024
    else:
        raise ValueError('unknown cs case: ' + case)

    data_block = bytes(range(256)) * (n // 256) + bytes(range(n % 256))
    digest = hashlib.sha256(data_block).digest()
    full_data = data_block + digest

    if impl == 'rs':
        from dataclasses import dataclass
        from construct import (StructMixin, field, rfield, Bytes, Tell,
                               Checksum, HashAlgo)

        @dataclass
        class Packet(StructMixin):
            start: int = rfield(Tell())
            data: bytes = field(Bytes(n))
            end: int = rfield(Tell())
            checksum: bytes = field(Checksum(Bytes(32), HashAlgo.SHA256,
                                             start=start, end=end))

        # checksum 用 field()（Checksum::build 忽略 obj，内部计算 hash）
        obj = Packet(data=data_block, checksum=digest)
        return (lambda: Packet.parse(full_data)), (lambda: obj.build())
    else:
        import construct as pc

        d = pc.Struct(
            "fields" / pc.RawCopy(pc.Bytes(n)),
            "checksum" / pc.Checksum(pc.Bytes(32),
                                     lambda data: hashlib.sha256(data).digest(),
                                     pc.this.fields.data),
        )
        build_obj = dict(fields=dict(data=data_block), checksum=digest)
        return (lambda: d.parse(full_data)), (lambda: d.build(build_obj))
'''


# ---------------------------------------------------------------------------
# P1P2 case 工厂
# ---------------------------------------------------------------------------
_P1P2_MAKE_CASE = r'''
def _make_p1p2_case(impl, case):
    from dataclasses import dataclass
    if impl == 'rs':
        from construct import (
            StructMixin, field, rfield,
            Enum, FlagsEnum, Mapping, OneOf, NoneOf,
            Union, Sequence, NamedTuple,
            ProcessXor, ProcessRotateLeft,
            Byte, Int8ub, Int16ub, Int32ub, Bytes,
        )
    else:
        import construct as pc
        StructMixin = pc.Struct
        def field(d): return d
        def rfield(d): return d
        Enum = pc.Enum
        FlagsEnum = pc.FlagsEnum
        OneOf = pc.OneOf
        NoneOf = pc.NoneOf
        Union = pc.Union
        Sequence = pc.Sequence
        NamedTuple = pc.NamedTuple
        ProcessXor = pc.ProcessXor
        ProcessRotateLeft = pc.ProcessRotateLeft
        Byte = pc.Byte
        Int8ub = pc.Int8ub
        Int16ub = pc.Int16ub
        Int32ub = pc.Int32ub
        Int32ul = pc.Int32ul
        Bytes = pc.Bytes
        this = pc.this

    C = case

    def mk_rs(fields_code, parse_data, build_kwargs_src):
        lines = ['@dataclass', 'class S(StructMixin):']
        ns = {'dataclass': dataclass, 'StructMixin': StructMixin,
              'field': field, 'rfield': rfield,
              'Enum': Enum, 'FlagsEnum': FlagsEnum, 'Mapping': Mapping,
              'OneOf': OneOf, 'NoneOf': NoneOf,
              'Union': Union, 'Sequence': Sequence, 'NamedTuple': NamedTuple,
              'ProcessXor': ProcessXor, 'ProcessRotateLeft': ProcessRotateLeft,
              'Byte': Byte, 'Int8ub': Int8ub, 'Int16ub': Int16ub,
              'Int32ub': Int32ub, 'Bytes': Bytes}
        exec('\n'.join(lines + fields_code), ns)
        S = ns['S']
        build_kwargs = eval(build_kwargs_src, ns)
        obj = S(**build_kwargs)
        return (lambda: S.parse(parse_data)), (lambda: obj.build())

    def mk_py(subcons, parse_data, build_dict):
        s = pc.Struct(*subcons)
        obj_d = dict(build_dict)
        return (lambda: s.parse(parse_data)), (lambda: s.build(obj_d))

    # ===== Enum =====
    if C == 'EN1':  # Enum normal mapping
        if impl == 'rs':
            return mk_rs(['    e: object = field(Enum(Byte, one=1, two=2, three=3))'],
                         b'\x01', "dict(e='one')")
        else:
            return mk_py(["e" / Enum(Byte, one=1, two=2, three=3)], b'\x01',
                         dict(e='one'))
    if C == 'EN2':  # Enum with 8 mappings (more dict lookup)
        m = dict(one=1, two=2, three=3, four=4, five=5, six=6, seven=7, eight=8)
        if impl == 'rs':
            src = '    e: object = field(Enum(Byte, **_m))'
            lines = ['@dataclass', 'class S(StructMixin):', src]
            ns = {'dataclass': dataclass, 'StructMixin': StructMixin,
                  'field': field, 'Enum': Enum, 'Byte': Byte, '_m': m}
            exec('\n'.join(lines), ns)
            S = ns['S']
            obj = S(e='four')
            return (lambda: S.parse(b'\x04')), (lambda: obj.build())
        else:
            return mk_py(["e" / Enum(Byte, **m)], b'\x04', dict(e='four'))

    # ===== FlagsEnum =====
    if C == 'FE1':  # FlagsEnum 4 flags
        if impl == 'rs':
            return mk_rs(['    f: object = field(FlagsEnum(Byte, one=1, two=2, four=4, eight=8))'],
                         b'\x05', "dict(f=dict(one=True, two=False, four=True, eight=False))")
        else:
            return mk_py(["f" / FlagsEnum(Byte, one=1, two=2, four=4, eight=8)],
                         b'\x05', dict(f=dict(one=True, two=False, four=True, eight=False)))

    # ===== Mapping =====
    if C == 'MP1':  # Mapping int->str
        if impl == 'rs':
            return mk_rs(['    m: object = field(Mapping(Byte, {1: "a", 2: "b", 3: "c"}))'],
                         b'\x01', "dict(m='a')")
        else:
            # py Mapping expects {label: wirevalue}
            return mk_py(["m" / pc.Mapping(Byte, {'a': 1, 'b': 2, 'c': 3})],
                         b'\x01', dict(m='a'))

    # ===== OneOf =====
    if C == 'OO1':  # OneOf valid
        if impl == 'rs':
            return mk_rs(['    v: int = field(OneOf(Byte, [1, 2, 3]))'],
                         b'\x01', "dict(v=1)")
        else:
            return mk_py(["v" / OneOf(Byte, [1, 2, 3])], b'\x01', dict(v=1))

    # ===== NoneOf =====
    if C == 'NO1':  # NoneOf pass
        if impl == 'rs':
            return mk_rs(['    v: int = field(NoneOf(Byte, [1, 2, 3]))'],
                         b'\xff', "dict(v=255)")
        else:
            return mk_py(["v" / NoneOf(Byte, [1, 2, 3])], b'\xff', dict(v=255))

    # ===== Union =====
    if C == 'UN1':  # Union multi-view
        if impl == 'rs':
            return mk_rs(['    u: object = field(Union(0, raw=Bytes(4), ints=Int32ub))'],
                         b'\x00\x00\x01\x02', "dict(u=dict(raw=b'\\x00\\x00\\x01\\x02', ints=258))")
        else:
            return mk_py(["u" / Union(0, "raw" / Bytes(4), "ints" / Int32ub)],
                         b'\x00\x00\x01\x02', dict(u=dict(raw=b'\x00\x00\x01\x02', ints=258)))

    # ===== Sequence =====
    if C == 'SQ1':  # Sequence 3 fields
        if impl == 'rs':
            return mk_rs(['    seq: object = field(Sequence(Int8ub, Int8ub, Int8ub))'],
                         b'\x01\x02\x03', "dict(seq=[1, 2, 3])")
        else:
            return mk_py(["seq" / Sequence(Int8ub, Int8ub, Int8ub)],
                         b'\x01\x02\x03', dict(seq=[1, 2, 3]))

    # ===== ProcessXor =====
    if C == 'PX1':  # ProcessXor single byte
        if impl == 'rs':
            return mk_rs(['    v: int = field(ProcessXor(0xff, Int32ub))'],
                         b'\xff\xff\xfe\xfd', "dict(v=258)")
        else:
            return mk_py(["v" / ProcessXor(0xff, Int32ub)],
                         b'\xff\xff\xfe\xfd', dict(v=258))

    # ===== ProcessRotateLeft =====
    if C == 'PR1':  # ProcessRotateLeft
        if impl == 'rs':
            return mk_rs(['    v: int = field(ProcessRotateLeft(4, 1, Int32ub))'],
                         b'\xf0\x00\x00\x10', "dict(v=258)")
        else:
            return mk_py(["v" / ProcessRotateLeft(4, 1, Int32ub)],
                         b'\xf0\x00\x00\x10', dict(v=258))

    # ===== NamedTuple =====
    if C == 'NT1':  # NamedTuple over inner Struct
        if impl == 'rs':
            lines = [
                '@dataclass',
                'class Inner(StructMixin):',
                '    x: int = field(Int8ub)',
                '    y: int = field(Int8ub)',
                '@dataclass',
                'class S(StructMixin):',
                '    nt: object = field(NamedTuple("coord", "x y", Inner))',
            ]
            ns = {'dataclass': dataclass, 'StructMixin': StructMixin,
                  'field': field, 'NamedTuple': NamedTuple, 'Int8ub': Int8ub}
            exec('\n'.join(lines), ns)
            S = ns['S']
            Inner = ns['Inner']
            Coord = type(Inner.parse(b'\x01\x02'))  # namedtuple type
            obj = S(nt=Coord(x=1, y=2))
            return (lambda: S.parse(b'\x01\x02')), (lambda: obj.build())
        else:
            d = pc.Struct("nt" / NamedTuple("coord", "x y", pc.Struct("x" / Int8ub, "y" / Int8ub)))
            coord_nt = NamedTuple("coord", "x y", pc.Struct("x" / Int8ub, "y" / Int8ub))
            parsed = coord_nt.parse(b'\x01\x02')
            return (lambda: d.parse(b'\x01\x02')), (lambda: d.build(dict(nt=parsed)))

    raise ValueError('unknown p1p2 case: ' + case)
'''


# ---------------------------------------------------------------------------
# 场景清单
# ---------------------------------------------------------------------------
# (case_id, constructor, group, desc, path_type, scale)
P0_CASES = [
    # Const / Default / Check
    ("CN1", "Const", "p0", "Const(b'IHDR') bytes 单字段", "standalone", "N=1"),
    ("CN2", "Const", "p0", "Const(255, Int32ul) int 单字段", "standalone", "N=1"),
    ("DF1", "Default", "p0", "Default(Byte, count) obj provided (pass-through)", "nested_in_struct", "N=2"),
    ("DF2", "Default", "p0", "Default(Byte, count) 表达式默认值", "nested_in_struct", "N=2"),
    ("CK1", "Check", "p0", "Check(a+b>0) 表达式断言", "nested_in_struct", "N=2"),
    # Hex / HexDump
    ("HX1", "Hex", "p0", "Hex(Int32ub) int 显示", "standalone", "N=1"),
    ("HX2", "Hex", "p0", "Hex(Bytes(4)) bytes 显示", "standalone", "N=1"),
    ("HD1", "HexDump", "p0", "HexDump(Bytes(4)) bytes 显示", "standalone", "N=1"),
    # Aligned
    ("AL1", "Aligned", "p0", "Aligned(4, Int16ub) modulus=4", "standalone", "N=1"),
    ("AL2", "Aligned", "p0", "Aligned(8, Bytes(3)) modulus=8", "standalone", "N=1"),
    # Terminated
    ("TM1", "Terminated", "p0", "Terminated EOF 断言", "nested_in_struct", "N=1"),
]

CS_CASES = [
    # Checksum（特殊：B1 零拷贝 vs Python RawCopy+callable）
    ("CS1", "Checksum", "p0", "Checksum B1 SHA256 N=64 data", "nested_in_struct", "N=64"),
    ("CS2", "Checksum", "p0", "Checksum B1 SHA256 N=1024 data", "nested_in_struct", "N=1024"),
]

P1P2_CASES = [
    # Enum / FlagsEnum / Mapping
    ("EN1", "Enum", "p1p2", "Enum(Byte, 3 mappings) 正常映射", "standalone", "N=1"),
    ("EN2", "Enum", "p1p2", "Enum(Byte, 8 mappings) 多映射", "standalone", "N=1"),
    ("FE1", "FlagsEnum", "p1p2", "FlagsEnum(Byte, 4 flags)", "standalone", "N=1"),
    ("MP1", "Mapping", "p1p2", "Mapping(Byte, 3 pairs) 正常映射", "standalone", "N=1"),
    # OneOf / NoneOf
    ("OO1", "OneOf", "p1p2", "OneOf(Byte, [1,2,3])", "standalone", "N=1"),
    ("NO1", "NoneOf", "p1p2", "NoneOf(Byte, [1,2,3])", "standalone", "N=1"),
    # Union
    ("UN1", "Union", "p1p2", "Union(0, raw=Bytes(4), ints=Int32ub)", "standalone", "N=1"),
    # Sequence
    ("SQ1", "Sequence", "p1p2", "Sequence(Int8ub, Int8ub, Int8ub)", "standalone", "N=3"),
    # ProcessXor / ProcessRotateLeft
    ("PX1", "ProcessXor", "p1p2", "ProcessXor(0xff, Int32ub)", "standalone", "N=1"),
    ("PR1", "ProcessRotateLeft", "p1p2", "ProcessRotateLeft(4, 1, Int32ub)", "standalone", "N=1"),
    # NamedTuple
    ("NT1", "NamedTuple", "p1p2", "NamedTuple over Struct{2}", "nested_in_struct", "N=1"),
]

ALL_BENCH_CASES = P0_CASES + CS_CASES + P1P2_CASES

# 门禁：默认 ≥10x。Checksum 取项目目标 ≥4x（vs Python 原版基线）。
GATES = {}
for case_id, *_ in ALL_BENCH_CASES:
    if case_id.startswith("CS"):
        GATES[case_id] = 4.0  # Checksum 项目目标 ≥4x
    else:
        GATES[case_id] = 10.0


def _resolve_make_case_src(group):
    if group == "p0":
        return _P0_MAKE_CASE
    if group == "p1p2":
        return _P1P2_MAKE_CASE
    if group == "p0_cs":
        return _CS_MAKE_CASE
    raise ValueError(f"unknown group: {group}")


def _resolve_case_dispatcher(case_id):
    if case_id.startswith("CS"):
        return "_make_cs_case", "p0_cs"
    if case_id in {c[0] for c in P0_CASES}:
        return "_make_p0_case", "p0"
    return "_make_p1p2_case", "p1p2"


def _build_script(impl, case_id, direction, number, repeat, crs_python_dir,
                  make_case_src, dispatcher):
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


def main():
    parser = argparse.ArgumentParser(description="construct-rs Adapters/Struct/Streams 性能基准测试")
    parser.add_argument("--group", action="append", choices=["p0", "p1p2"],
                        help="仅测量指定分组（可多次指定），默认全部")
    parser.add_argument("--case", action="append",
                        help="仅测量指定 case（可多次指定）")
    parser.add_argument("--constructor", action="append",
                        help="仅测量指定 constructor（可多次指定）")
    parser.add_argument("--iterations", type=int, default=10,
                        help="每次测量 iteration 数（默认 10）")
    parser.add_argument("--warmup", type=int, default=3, help="预热次数（默认 3）")
    parser.add_argument("--number", type=int, default=5000,
                        help="timeit number（默认 5000）")
    args = parser.parse_args()

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
    print("性能基准（Controlled A/B Test vs Python construct 2.10.70）")
    print("=" * 78)
    print(f"Python (rs): {rs_python}")
    print(f"Python (py): {py_python}")
    print(f"number={config.number}, iterations={config.iterations}, warmup={config.warmup}")
    print(f"Selected: {len(selected)} cases x parse+build = {len(selected)*2} 测量点")
    print()

    all_results = []
    paired_for_summary = []
    csv_rows = []
    failures = []

    for case_id, constructor, group, desc, path_type, scale in selected:
        for direction in ("parse", "build"):
            scenario = f"{case_id}-{direction}"
            repeat = config.iterations + config.warmup
            dispatcher, src_group = _resolve_case_dispatcher(case_id)
            make_case_src = _resolve_make_case_src(src_group)
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
                msg = str(e).encode("ascii", errors="replace").decode("ascii")
                print(f" [ERROR] {msg[:300]}")
                failures.append((scenario, msg))
                continue

            all_results.append(rs_result)
            all_results.append(py_result)
            sp = speedup_ratio(rs_result.median_ns, py_result.median_ns)
            paired_for_summary.append({"rs": rs_result, "py": py_result, "speedup": sp})
            gate = GATES.get(case_id, 10.0)
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
                "gate": gate,
            })
            mark = "OK" if sp >= gate else "LOW"
            print(f" rs={rs_result.median_ns:.0f}ns py={py_result.median_ns:.0f}ns "
                  f"speedup={sp:.2f}x [{mark}]")

    print()
    print("对比表（前 30 行）：")
    print(format_results_table(all_results[:60]))

    report = BenchReport(
        timestamp=time.strftime("%Y-%m-%dT%H:%M:%S"),
        environment=make_environment_dict(config),
        results=all_results,
        summary=make_summary(paired_for_summary),
    )
    report.summary["failed_gates"] = [scenario for scenario, _ in failures]

    gate_failures = check_gates(report, GATES)
    print()
    print(f"门禁检查：{len(all_results)//2 - len(gate_failures)}/{len(all_results)//2} PASS")
    if gate_failures:
        print(f"  {len(gate_failures)} 项未达标：")
        for f in gate_failures[:20]:
            print(f"    {f}")
        if len(gate_failures) > 20:
            print(f"    ... 还有 {len(gate_failures) - 20} 项")

    csv_path = _BENCH_DIR / "results" / "bench_adapters_struct_streams_csv.json"
    csv_path.parent.mkdir(parents=True, exist_ok=True)
    with csv_path.open("w", encoding="utf-8") as f:
        json.dump(csv_rows, f, ensure_ascii=False, indent=2)
    print(f"\nCSV 数据：{csv_path}")

    report_path = _BENCH_DIR / "results" / "bench_adapters_struct_streams"
    report.write(report_path)
    print(f"详细结果：{report_path}.json / .md")

    return 0 if not gate_failures else 2


if __name__ == "__main__":
    sys.exit(main())
