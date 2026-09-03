"""Phase 7 集成性能基准（Conditional + Streams）。

设计依据：
- docs/design/基础设施/测试框架设计.md §5.2（bench 统一模板）
- bench/_helpers/runner.py（BenchRunner）
- plans/phase7-conditional-streams/总纲.md（PM 决策 1-3）

覆盖 7 个构造器 × 5 场景 × parse/build = 70 测量点：

7.1 Conditional（4 个，If 是 Python macro 不独立 bench）：
- IfThenElse（ITE1-5）：常量 cond / Expr cond / 嵌套 / 多字段
- Switch（SW1-5）：int 常量 key / 多 cases / default / 嵌套 / 大 subcon
- Select（SL1-5）：候选成功 / 第 2 个成功 / 3 候选 / CString / 多字段
- FocusedSeq（FS1-5）：基础 / 倒序 / 大 subcon / 3 字段 / 多字段

7.2 Streams（3 个）：
- Seek（SK1-5）：whence=0/1/2 / Expr at / 大 buffer
- Pointer（PT1-5）：正 / 负 / relative / Int32ub / 多字段
- Prefixed（PF1-5）：Byte/Int16ub/VarInt lengthfield / 大数据 / includelength

门禁：
- 全部 ≥10x（与 Phase 4/6 一致的硬约束）

**Switch str key 说明（PM 任务书要求"int+str"）**：
PM 决策 1 推荐 A+B 混合（int ExprProgram 零 FFI + str FieldRef 少量 FFI）。
DEV 7.1 [设计质疑] 2 + VET 观察 O-1 确认：**Python 用户面 str-key 路径不可达**
（_FieldDescriptor 不携带类型元数据，所有 field 引用走 IntExpr 路径）。
str-key FieldRef 路径仅通过 Rust API 直接构造（switch.rs 单元测试 SW-5/SW-6/SW-12 覆盖）。
bench 测量 Python 端可达的 const int key 场景（≥10x 预期）；
str-key 性能数据由 Rust 单元测试覆盖，不独立 bench。

用法::

    python bench/bench_phase7.py
    python bench/bench_phase7.py --group conditional
    python bench/bench_phase7.py --group streams
    python bench/bench_phase7.py --case ITE1 --case PF3
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
# venv 解析（与 bench_phase6.py 一致：环境变量 > 项目内 .venv/.venv-pc > sys.executable）
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
# 子进程测量脚本模板（与 bench_phase6.py 一致）
# ---------------------------------------------------------------------------
_PHASE7_MAKE_CASE = r'''
def _make_phase7_case(impl, case):
    from dataclasses import dataclass
    if impl == 'rs':
        from construct import (
            StructMixin, field, rfield,
            Int8ub, Int16ub, Int32ub, Bytes, Byte, VarInt, Pass,
            CString, GreedyBytes, GreedyRange, Int32ul, Array,
            IfThenElse, Switch, Select, FocusedSeq, Renamed,
            Seek, Pointer, Prefixed,
        )
    else:
        import construct as pc
        StructMixin = pc.Struct
        def field(d, *, default=None):
            return d
        def rfield(d):
            return d
        Int8ub = pc.Int8ub
        Int16ub = pc.Int16ub
        Int32ub = pc.Int32ub
        Bytes = pc.Bytes
        Byte = pc.Byte
        VarInt = pc.VarInt
        Pass = pc.Pass
        CString = lambda enc: pc.CString(enc)
        GreedyBytes = pc.GreedyBytes
        GreedyRange = pc.GreedyRange
        Int32ul = pc.Int32ul
        Array = pc.Array
        IfThenElse = pc.IfThenElse
        Switch = pc.Switch
        Select = pc.Select
        FocusedSeq = pc.FocusedSeq
        Seek = pc.Seek
        Pointer = pc.Pointer
        Prefixed = pc.Prefixed
        # Python construct 的 Renamed 用 "/" 运算符（call site 处理）
        # FocusedSeq 用 "name"/subcon 语法，本 helper 通过 dict 形式传入
        def Renamed(name, subcon):
            return name / subcon
        # Python construct Struct 接收 *subcons list，与 rs 的 dataclass 形式不同。
        # 适配：当 impl == 'py' 时，跳过 dataclass 装饰，直接构造 pc.Struct。

    C = case

    # ===== IfThenElse (ITE1-5) =====
    if C == 'ITE1':
        # 常量 cond=True → then=Int8ub 走
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                v: int = field(IfThenElse(True, Int8ub, Pass))
            data = b"\x42"
            obj = P(v=0x42)
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("v" / IfThenElse(True, Int8ub, Pass))
            data = b"\x42"
            obj_d = dict(v=0x42)
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'ITE2':
        # 常量 cond=False → else=Pass 走
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                v: object = field(IfThenElse(False, Int8ub, Pass))
            data = b""
            obj = P(v=None)
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("v" / IfThenElse(False, Int8ub, Pass))
            data = b""
            obj_d = dict(v=None)
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'ITE3':
        # 常量 cond=True → then=Int32ub 走（放大工作量）
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                v: int = field(IfThenElse(True, Int32ub, Pass))
            data = b"\x00\x00\x00\x05"
            obj = P(v=5)
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("v" / IfThenElse(True, Int32ub, Pass))
            data = b"\x00\x00\x00\x05"
            obj_d = dict(v=5)
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'ITE4':
        # 嵌套 IfThenElse（深度）
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                v: int = field(IfThenElse(True, IfThenElse(True, Int8ub, Pass), Pass))
            data = b"\x42"
            obj = P(v=0x42)
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("v" / IfThenElse(True, IfThenElse(True, Int8ub, Pass), Pass))
            data = b"\x42"
            obj_d = dict(v=0x42)
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'ITE5':
        # 多字段 Struct {a, b: IfThenElse(True, Int8ub, Pass)}
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                a: int = field(Int8ub)
                b: int = field(IfThenElse(True, Int8ub, Pass))
            data = b"\x01\x02"
            obj = P(a=1, b=2)
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("a" / Int8ub, "b" / IfThenElse(True, Int8ub, Pass))
            data = b"\x01\x02"
            obj_d = dict(a=1, b=2)
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    # ===== Switch (SW1-5) — const int key（Python 用户面 str-key 不可达） =====
    if C == 'SW1':
        # const key=1, 3 cases → Int8ub
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                v: int = field(Switch(1, {1: Int8ub, 2: Int16ub, 3: Int32ub}))
            data = b"\x42"
            obj = P(v=0x42)
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("v" / Switch(1, {1: Int8ub, 2: Int16ub, 3: Int32ub}))
            data = b"\x42"
            obj_d = dict(v=0x42)
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'SW2':
        # const key=99 → default=Pass（未命中）
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                v: object = field(Switch(99, {1: Int8ub, 2: Int16ub, 3: Int32ub}, default=Pass))
            data = b""
            obj = P(v=None)
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("v" / Switch(99, {1: Int8ub, 2: Int16ub, 3: Int32ub}))
            data = b""
            obj_d = dict(v=None)
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'SW3':
        # const key=4, 5 cases → 命中末尾 case（线性查找远端）
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                v: int = field(Switch(4, {1: Int8ub, 2: Int8ub, 3: Int8ub, 4: Int8ub, 5: Int8ub}))
            data = b"\x42"
            obj = P(v=0x42)
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("v" / Switch(4, {1: Int8ub, 2: Int8ub, 3: Int8ub, 4: Int8ub, 5: Int8ub}))
            data = b"\x42"
            obj_d = dict(v=0x42)
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'SW4':
        # 嵌套 Switch
        if impl == 'rs':
            inner = Switch(1, {1: Int8ub, 2: Int16ub})
            @dataclass
            class P(StructMixin):
                v: int = field(Switch(1, {1: inner, 2: Int32ub}))
            data = b"\x42"
            obj = P(v=0x42)
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            inner = Switch(1, {1: Int8ub, 2: Int16ub})
            s = pc.Struct("v" / Switch(1, {1: inner, 2: Int32ub}))
            data = b"\x42"
            obj_d = dict(v=0x42)
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'SW5':
        # Switch with Int32ub case（放大工作量）
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                v: int = field(Switch(2, {1: Int8ub, 2: Int32ub, 3: Int8ub}))
            data = b"\x00\x00\x00\x05"
            obj = P(v=5)
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("v" / Switch(2, {1: Int8ub, 2: Int32ub, 3: Int8ub}))
            data = b"\x00\x00\x00\x05"
            obj_d = dict(v=5)
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    # ===== Select (SL1-5) =====
    if C == 'SL1':
        # Select(Int8ub, Int16ub) → 第 1 个成功
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                v: int = field(Select(Int8ub, Int16ub))
            data = b"\x42"
            obj = P(v=0x42)
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("v" / Select(Int8ub, Int16ub))
            data = b"\x42"
            obj_d = dict(v=0x42)
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'SL2':
        # Select(Int16ub, Int8ub) → 第 1 个失败 + 第 2 个成功
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                v: int = field(Select(Int16ub, Int8ub))
            data = b"\x42"
            obj = P(v=0x42)
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("v" / Select(Int16ub, Int8ub))
            data = b"\x42"
            obj_d = dict(v=0x42)
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'SL3':
        # 3 候选 → 第 1 个成功
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                v: int = field(Select(Int8ub, Int16ub, Int32ub))
            data = b"\x42"
            obj = P(v=0x42)
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("v" / Select(Int8ub, Int16ub, Int32ub))
            data = b"\x42"
            obj_d = dict(v=0x42)
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'SL4':
        # Select(Int32ub, CString('utf8')) → CString 成功（变长 string）
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                v: object = field(Select(Int32ub, CString("utf8")))
            data = b"hello\x00"
            obj = P(v="hello")
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("v" / Select(Int32ub, CString("utf8")))
            data = b"hello\x00"
            obj_d = dict(v="hello")
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'SL5':
        # 多字段 Struct {a, b: Select(Int8ub, Int16ub)}
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                a: int = field(Int8ub)
                b: int = field(Select(Int8ub, Int16ub))
            data = b"\x01\x02"
            obj = P(a=1, b=2)
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("a" / Int8ub, "b" / Select(Int8ub, Int16ub))
            data = b"\x01\x02"
            obj_d = dict(a=1, b=2)
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    # ===== FocusedSeq (FS1-5) =====
    if C == 'FS1':
        # 基础：FocusedSeq("num", Pass, Renamed("num", Int8ub))
        # 非 focus 字段用 Pass 占位（VET O-2 / 7.1-7.2 联合验证修复 2-B）
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                v: int = field(FocusedSeq("num", Pass, Renamed("num", Int8ub)))
            data = b"\xff"
            obj = P(v=0xFF)
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("v" / FocusedSeq("num", Pass, "num" / Int8ub))
            data = b"\xff"
            obj_d = dict(v=0xFF)
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'FS2':
        # 倒序：Renamed 在前 + Pass 在后
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                v: int = field(FocusedSeq("num", Renamed("num", Int8ub), Pass))
            data = b"\xff"
            obj = P(v=0xFF)
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("v" / FocusedSeq("num", "num" / Int8ub, Pass))
            data = b"\xff"
            obj_d = dict(v=0xFF)
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'FS3':
        # 大 subcon：Int32ub focus
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                v: int = field(FocusedSeq("num", Pass, Renamed("num", Int32ub)))
            data = b"\x00\x00\x00\x05"
            obj = P(v=5)
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("v" / FocusedSeq("num", Pass, "num" / Int32ub))
            data = b"\x00\x00\x00\x05"
            obj_d = dict(v=5)
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'FS4':
        # 3 字段：Pass + focus + Pass
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                v: int = field(FocusedSeq("num",
                    Pass, Renamed("num", Int8ub), Pass))
            data = b"\xff"
            obj = P(v=0xFF)
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("v" / FocusedSeq("num",
                Pass, "num" / Int8ub, Pass))
            data = b"\xff"
            obj_d = dict(v=0xFF)
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'FS5':
        # focus 在中间 + 多个匿名 Pass
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                v: int = field(FocusedSeq("num",
                    Pass, Pass, Renamed("num", Int8ub), Pass))
            data = b"\xff"
            obj = P(v=0xFF)
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("v" / FocusedSeq("num",
                Pass, Pass, "num" / Int8ub, Pass))
            data = b"\xff"
            obj_d = dict(v=0xFF)
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    # ===== Seek (SK1-5) — 包装在 Struct 内（Sequence 未实现，7.2 DEV 报告 §3）=====
    # 注意：Seek 用 field 包装（非 rfield）—— Seek 的 build 接受任意值（flagbuildnone=True），
    # 不作为 RO 字段（设计 §3.5；VET §3 compute_ro_value 未追加 Seek）。
    if C == 'SK1':
        # Seek(5, whence=0=Start)
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                pos: int = field(Seek(5))
                tail: bytes = field(Bytes(1))
            data = b"01234x"
            obj = P(pos=5, tail=b"x")
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("pos" / Seek(5), "tail" / Bytes(1))
            data = b"01234x"
            obj_d = dict(pos=5, tail=b"x")
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'SK2':
        # Seek(0, whence=1=Current) — 相对当前位置 0
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                head: bytes = field(Bytes(3))
                pos: int = field(Seek(0, 1))
                tail: bytes = field(Bytes(1))
            data = b"abcX"
            obj = P(head=b"abc", pos=3, tail=b"X")
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("head" / Bytes(3), "pos" / Seek(0, 1), "tail" / Bytes(1))
            data = b"abcX"
            obj_d = dict(head=b"abc", pos=3, tail=b"X")
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'SK3':
        # Seek(-2, whence=2=End) — 从 EOF 退 2 字节（需要前置 head 让 buffer 非空，
        # 否则 build 模式 buffer 为空 whence=End + at<0 → Err）
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                head: bytes = field(Bytes(8))
                pos: int = field(Seek(-2, 2))
                tail: bytes = field(Bytes(1))
            data = b"abcdefgh"
            obj = P(head=b"abcdefgh", pos=6, tail=b"g")
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("head" / Bytes(8), "pos" / Seek(-2, 2), "tail" / Bytes(1))
            data = b"abcdefgh"
            obj_d = dict(head=b"abcdefgh", pos=6, tail=b"g")
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'SK4':
        # 大 buffer + Seek(5000, 0)
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                pos: int = field(Seek(5000))
                tail: bytes = field(Bytes(1))
            data = b"\x00" * 5000 + b"X"
            obj = P(pos=5000, tail=b"X")
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("pos" / Seek(5000), "tail" / Bytes(1))
            data = b"\x00" * 5000 + b"X"
            obj_d = dict(pos=5000, tail=b"X")
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'SK5':
        # 多字段 Struct + Seek(2)
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                head: bytes = field(Bytes(2))
                pos: int = field(Seek(2))
                tail: bytes = field(Bytes(1))
            data = b"ab\x00x"
            obj = P(head=b"ab", pos=2, tail=b"x")
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("head" / Bytes(2), "pos" / Seek(2), "tail" / Bytes(1))
            data = b"ab\x00x"
            obj_d = dict(head=b"ab", pos=2, tail=b"x")
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    # ===== Pointer (PT1-5) =====
    if C == 'PT1':
        # Pointer(8, Bytes(1)) 正 offset
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                ptr: bytes = field(Pointer(8, Bytes(1)))
                direct: bytes = field(Bytes(1))
            data = b"0123456789ab"  # 12 bytes
            obj = P(ptr=b"8", direct=b"0")
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("ptr" / Pointer(8, Bytes(1)), "direct" / Bytes(1))
            data = b"0123456789ab"
            obj_d = dict(ptr=b"8", direct=b"0")
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'PT2':
        # Pointer(-2, Bytes(1)) 负 offset（从 EOF）
        # 加 head 让 buffer 非空（否则 build 时空 buffer whence=End + at<0 → Err）
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                head: bytes = field(Bytes(8))
                ptr: bytes = field(Pointer(-2, Bytes(1)))
            data = b"abcdefgh"
            obj = P(head=b"abcdefgh", ptr=b"g")
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("head" / Bytes(8), "ptr" / Pointer(-2, Bytes(1)))
            data = b"abcdefgh"
            obj_d = dict(head=b"abcdefgh", ptr=b"g")
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'PT3':
        # Pointer(5, Bytes(2)) subcon 多字节
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                ptr: bytes = field(Pointer(5, Bytes(2)))
                direct: bytes = field(Bytes(1))
            data = b"01234XYZw"  # 9 bytes
            obj = P(ptr=b"XY", direct=b"0")
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("ptr" / Pointer(5, Bytes(2)), "direct" / Bytes(1))
            data = b"01234XYZw"
            obj_d = dict(ptr=b"XY", direct=b"0")
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'PT4':
        # Pointer(8, Int32ub) 大 subcon
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                ptr: int = field(Pointer(8, Int32ub))
                direct: bytes = field(Bytes(1))
            data = b"01234567\x00\x00\x00\x05x"
            obj = P(ptr=5, direct=b"0")
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("ptr" / Pointer(8, Int32ub), "direct" / Bytes(1))
            data = b"01234567\x00\x00\x00\x05x"
            obj_d = dict(ptr=5, direct=b"0")
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'PT5':
        # Pointer in 多字段 Struct（3 字段）
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                a: bytes = field(Bytes(1))
                ptr: bytes = field(Pointer(5, Bytes(1)))
                b: bytes = field(Bytes(1))
            data = b"01234Xy"  # 7 bytes
            obj = P(a=b"0", ptr=b"X", b=b"1")
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("a" / Bytes(1), "ptr" / Pointer(5, Bytes(1)), "b" / Bytes(1))
            data = b"01234Xy"
            obj_d = dict(a=b"0", ptr=b"X", b=b"1")
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    # ===== Prefixed (PF1-5) =====
    if C == 'PF1':
        # Prefixed(Byte, Bytes(3))
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                x: bytes = field(Prefixed(Byte, Bytes(3)))
            data = b"\x03abc"
            obj = P(x=b"abc")
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("x" / Prefixed(Byte, Bytes(3)))
            data = b"\x03abc"
            obj_d = dict(x=b"abc")
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'PF2':
        # Prefixed(Byte, Bytes(64)) 大数据
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                x: bytes = field(Prefixed(Byte, Bytes(64)))
            data = b"\x40" + b"d" * 64
            obj = P(x=b"d" * 64)
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("x" / Prefixed(Byte, Bytes(64)))
            data = b"\x40" + b"d" * 64
            obj_d = dict(x=b"d" * 64)
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'PF3':
        # Prefixed(Int16ub, Bytes(3))
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                x: bytes = field(Prefixed(Int16ub, Bytes(3)))
            data = b"\x00\x03abc"
            obj = P(x=b"abc")
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("x" / Prefixed(Int16ub, Bytes(3)))
            data = b"\x00\x03abc"
            obj_d = dict(x=b"abc")
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'PF4':
        # Prefixed(VarInt, GreedyRange(Int32ul)) — 常见协议模式
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                x: object = field(Prefixed(VarInt, GreedyRange(Int32ul)))
            data = b"\x08abcdefgh"  # VarInt(8) + 2 Int32ul
            obj = P(x=[0x64636261, 0x68676665])  # "abcd", "efgh" LE
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("x" / Prefixed(VarInt, GreedyRange(Int32ul)))
            data = b"\x08abcdefgh"
            obj_d = dict(x=[0x64636261, 0x68676665])
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    if C == 'PF5':
        # Prefixed(Int16ub, Bytes(3), includelength=True)
        if impl == 'rs':
            @dataclass
            class P(StructMixin):
                x: bytes = field(Prefixed(Int16ub, Bytes(3), includelength=True))
            data = b"\x00\x05abc"  # length=5 = 3 + 2 (lengthfield 自身)
            obj = P(x=b"abc")
            return (lambda: P.parse(data)), (lambda: obj.build())
        else:
            s = pc.Struct("x" / Prefixed(Int16ub, Bytes(3), includelength=True))
            data = b"\x00\x05abc"
            obj_d = dict(x=b"abc")
            return (lambda: s.parse(data)), (lambda: s.build(obj_d))

    raise ValueError('unknown case: ' + case)
'''


# ---------------------------------------------------------------------------
# 场景清单：Phase 7（7 个构造器 × 5 场景 = 35 case）
# ---------------------------------------------------------------------------
# (case_id, constructor, group, desc, path_type, scale)
CONDITIONAL_CASES = [
    # IfThenElse
    ("ITE1", "IfThenElse", "conditional", "常量 cond=True → then=Int8ub", "nested_in_struct", "N=1"),
    ("ITE2", "IfThenElse", "conditional", "常量 cond=False → else=Pass", "nested_in_struct", "N=1"),
    ("ITE3", "IfThenElse", "conditional", "常量 cond=True → then=Int32ub (大工作量)", "nested_in_struct", "N=1"),
    ("ITE4", "IfThenElse", "conditional", "嵌套 IfThenElse（深度）", "nested_in_struct", "N=1"),
    ("ITE5", "IfThenElse", "conditional", "多字段 Struct {a, b: IfThenElse}", "nested_in_struct", "N=2"),
    # Switch（const int key，Python 用户面 str-key 不可达，见文件 docstring）
    ("SW1", "Switch", "conditional", "const key=1, 3 cases → Int8ub", "nested_in_struct", "N=1"),
    ("SW2", "Switch", "conditional", "const key=99 → default=Pass", "nested_in_struct", "N=1"),
    ("SW3", "Switch", "conditional", "const key=4, 5 cases → 末尾命中", "nested_in_struct", "N=1"),
    ("SW4", "Switch", "conditional", "嵌套 Switch", "nested_in_struct", "N=1"),
    ("SW5", "Switch", "conditional", "Switch with Int32ub case", "nested_in_struct", "N=1"),
    # Select
    ("SL1", "Select", "conditional", "Select(Int8ub, Int16ub) 第 1 个成功", "nested_in_struct", "N=1"),
    ("SL2", "Select", "conditional", "Select(Int16ub, Int8ub) 第 2 个成功", "nested_in_struct", "N=1"),
    ("SL3", "Select", "conditional", "Select 3 候选 → 第 1 个成功", "nested_in_struct", "N=1"),
    ("SL4", "Select", "conditional", "Select(Int32ub, CString) CString 成功", "nested_in_struct", "N=1"),
    ("SL5", "Select", "conditional", "多字段 Struct {a, b: Select}", "nested_in_struct", "N=2"),
    # FocusedSeq
    ("FS1", "FocusedSeq", "conditional", "FocusedSeq(Pass, Renamed(num, Int8ub))", "nested_in_struct", "N=1"),
    ("FS2", "FocusedSeq", "conditional", "FocusedSeq 倒序（Renamed 在前）", "nested_in_struct", "N=1"),
    ("FS3", "FocusedSeq", "conditional", "FocusedSeq Int32ub focus（大 subcon）", "nested_in_struct", "N=1"),
    ("FS4", "FocusedSeq", "conditional", "3 字段 FocusedSeq", "nested_in_struct", "N=1"),
    ("FS5", "FocusedSeq", "conditional", "focus 在中间（多匿名 Pass）", "nested_in_struct", "N=1"),
]

STREAMS_CASES = [
    # Seek
    ("SK1", "Seek", "streams", "Seek(5, whence=0=Start)", "nested_in_struct", "N=1"),
    ("SK2", "Seek", "streams", "Seek(0, whence=1=Current)", "nested_in_struct", "N=1"),
    ("SK3", "Seek", "streams", "Seek(-2, whence=2=End)", "nested_in_struct", "N=1"),
    ("SK4", "Seek", "streams", "大 buffer Seek(5000)", "nested_in_struct", "N=1"),
    ("SK5", "Seek", "streams", "多字段 Struct + Seek(2)", "nested_in_struct", "N=2"),
    # Pointer
    ("PT1", "Pointer", "streams", "Pointer(8, Bytes(1)) 正 offset", "nested_in_struct", "N=2"),
    ("PT2", "Pointer", "streams", "Pointer(-2, Bytes(1)) 负 offset", "nested_in_struct", "N=2"),
    ("PT3", "Pointer", "streams", "Pointer(5, Bytes(2)) subcon 多字节", "nested_in_struct", "N=2"),
    ("PT4", "Pointer", "streams", "Pointer(8, Int32ub) 大 subcon", "nested_in_struct", "N=2"),
    ("PT5", "Pointer", "streams", "Pointer in 3 字段 Struct", "nested_in_struct", "N=3"),
    # Prefixed
    ("PF1", "Prefixed", "streams", "Prefixed(Byte, Bytes(3))", "nested_in_struct", "N=1"),
    ("PF2", "Prefixed", "streams", "Prefixed(Byte, Bytes(64)) 大数据", "nested_in_struct", "N=1"),
    ("PF3", "Prefixed", "streams", "Prefixed(Int16ub, Bytes(3))", "nested_in_struct", "N=1"),
    ("PF4", "Prefixed", "streams", "Prefixed(VarInt, GreedyRange(Int32ul))", "nested_in_struct", "N=1"),
    ("PF5", "Prefixed", "streams", "Prefixed(Int16ub, Bytes(3), includelength=True)", "nested_in_struct", "N=1"),
]


ALL_BENCH_CASES = CONDITIONAL_CASES + STREAMS_CASES

# 门禁：所有 Phase 7 构造器硬门禁 ≥10x
GATES = {case_id: 10.0 for case_id, *_ in ALL_BENCH_CASES}


def _build_script(impl, case_id, direction, number, repeat, crs_python_dir):
    """生成完整的子进程测量脚本（与 bench_phase6 同模板）。"""
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

{_PHASE7_MAKE_CASE}

parse_target, build_target = _make_phase7_case(IMPL, CASE)
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
    parser = argparse.ArgumentParser(description="construct-rs Phase 7 性能基准测试")
    parser.add_argument("--group", action="append",
                        choices=["conditional", "streams"],
                        help="仅测量指定分组（可多次指定），默认全部")
    parser.add_argument("--case", action="append",
                        help="仅测量指定 case（可多次指定）")
    parser.add_argument("--constructor", action="append",
                        help="仅测量指定 constructor（可多次指定，如 IfThenElse）")
    parser.add_argument("--iterations", type=int, default=10,
                        help="每次测量 iteration 数（默认 10）")
    parser.add_argument("--warmup", type=int, default=3, help="预热次数（默认 3）")
    parser.add_argument("--number", type=int, default=5000,
                        help="timeit number（默认 5000，与 Phase 6 一致）")
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
    print("Phase 7 集成性能基准（Conditional + Streams）")
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
            print(f"  测量 {scenario:<24}...", end="", flush=True)
            try:
                rs_result = runner.run_scenario(
                    "rs", scenario,
                    _build_script("rs", case_id, direction, config.number, repeat,
                                  _CRS_PYTHON_DIR),
                )
                py_result = runner.run_scenario(
                    "py", scenario,
                    _build_script("py", case_id, direction, config.number, repeat,
                                  _CRS_PYTHON_DIR),
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

    # 写 CSV 行（供 PM 追加到 perf-scenarios.csv）
    csv_path = _BENCH_DIR / "results" / "bench_phase7_csv.json"
    csv_path.parent.mkdir(parents=True, exist_ok=True)
    with csv_path.open("w", encoding="utf-8") as f:
        json.dump(csv_rows, f, ensure_ascii=False, indent=2)
    print(f"\nCSV 数据（待 PM 追加 perf-scenarios.csv）：{csv_path}")

    # 写完整报告
    report_path = _BENCH_DIR / "results" / "bench_phase7"
    report.write(report_path)
    print(f"详细结果：{report_path}.json / .md")

    return 0 if not gate_failures else 2


if __name__ == "__main__":
    sys.exit(main())
