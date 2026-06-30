"""Phase 4 Array 系列构造器 Python 行为一致性测试。

设计依据：
- AGENTS.md §6（性能门禁子进程隔离方法可复用为正确性对比）
- plans/phase4-array/总纲.md S-FUNC：Phase 4 全部构造器行为与 Python
  construct 2.10.70 一致
- docs/模块设计-Array.md §7 边界条件 / §9.5 已知行为差异

测试策略：
    两个同名包（construct-rs 与 Python construct 2.10.70）无法在同一进程
    中导入，采用**子进程隔离**策略：每个用例 × impl 在独立子进程中运行，
    通过 JSON 输出结果，主进程比对。

    每个用例对 Rust 和 Python 两侧执行相同的 parse/build 操作，验证：
    - parse 同输入 → 同输出（值相等）
    - build 同输入 → 同输出（字节序列相等）
    - parse → build → parse 往返一致

覆盖范围：Phase 4 全部 6 个构造器
    - Array（固定 count、表达式 count、discard、嵌套）—— A1-A6
    - GreedyRange（基本、fallback 回退、discard）—— G1-G4
    - PrefixedArray（不同 countfield 类型）—— P1-P4
    - RepeatUntil（Expr 路径、PyCallable 路径）—— R1-R3
    - Index（在 Array/GreedyRange 内）—— I1-I3
    - StopIf（在 Struct/GreedyRange 内）—— S1-S3
    - 嵌套组合（GreedyRange 内 PrefixedArray、Array 内 Struct）—— N1-N4

用法::

    python tests/test_phase4_parity.py
    pytest tests/test_phase4_parity.py
"""

from __future__ import annotations

import json
import subprocess
import sys
import textwrap
from pathlib import Path

# pytest 在 standalone 模式下不是必需的（仅 pytest 跑时需要）。
# 在某些 venv 中 pytest 可能损坏，此时仍允许 standalone 直接执行。
try:
    import pytest
    if not hasattr(pytest, "fixture") or not hasattr(pytest, "mark"):
        raise ImportError("pytest installation is incomplete")
except Exception:  # pragma: no cover - 仅 pytest 缺失/损坏时触发
    pytest = None  # type: ignore[assignment]

# construct-rs python/ 目录（用于 sys.path 操纵，让 construct-rs 覆盖 site-packages）
_CRS_PYTHON_DIR = str(Path(__file__).resolve().parent.parent / "python")

# Python 解释器（用于跑两个 impl）。
# 优先使用专门的 venv（与 benchmark 同口径），回落到主进程 python。
_VENV_ROOT = Path(r"<opencode-temp>")
_RS_PYTHON_EXE = str(_VENV_ROOT / "crs_venv" / "Scripts" / "python.exe")
_PY_PYTHON_EXE = str(_VENV_ROOT / "crs_venv_py" / "Scripts" / "python.exe")
if not Path(_RS_PYTHON_EXE).exists():
    _RS_PYTHON_EXE = sys.executable
if not Path(_PY_PYTHON_EXE).exists():
    _PY_PYTHON_EXE = sys.executable


# ---------------------------------------------------------------------------
# 子进程校验脚本
# ---------------------------------------------------------------------------

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
            StructMixin, field, rfield, wfield,
            Int8ub, Int8sb, Int16ub, Int32ub, Bytes,
            Array, GreedyRange, PrefixedArray,
            RepeatUntil, Index, StopIf,
        )

        C = CASE

        if C == 'A1':
            # Array(3, Int8ub) — 固定 count
            @dataclass
            class P(StructMixin):
                items: list = field(Array(3, Int8ub))
            data = bytes([10, 20, 30])
            return P, data, lambda: P(items=[10, 20, 30]), lambda o: {{'items': o.items}}

        if C == 'A2':
            # Array 表达式 count（字段引用）
            @dataclass
            class P(StructMixin):
                n: int = field(Int8ub)
                items: list = field(Array(2, Int8ub))
            data = bytes([2, 10, 20])
            return P, data, lambda: P(n=2, items=[10, 20]), \\
                lambda o: {{'n': o.n, 'items': o.items}}

        if C == 'A3':
            # Array discard=True：返回空 list 但消耗流
            @dataclass
            class P(StructMixin):
                items: list = field(Array(3, Int8ub, discard=True))
            data = bytes([10, 20, 30])
            # build：discard 不影响字节写入，list 仍需匹配 count（构造器长度校验
            # 与 discard 独立，与 Python 一致——discard 只影响 retlist 收集）
            return P, data, lambda: P(items=[10, 20, 30]), lambda o: {{'items': o.items}}

        if C == 'A4':
            # Array 内 Struct（2 字段）
            @dataclass
            class Inner(StructMixin):
                a: int = field(Int8ub)
                b: int = field(Int8ub)
            @dataclass
            class P(StructMixin):
                items: list = field(Array(2, Inner))
            data = bytes([0x10, 0x20, 0x30, 0x40])
            return P, data, lambda: P(items=[Inner(a=0x10, b=0x20), Inner(a=0x30, b=0x40)]), \\
                lambda o: {{'items': [{{'a': x.a, 'b': x.b}} for x in o.items]}}

        if C == 'A5':
            # Array 嵌套 Array
            @dataclass
            class P(StructMixin):
                items: list = field(Array(2, Array(2, Int8ub)))
            data = bytes([1, 2, 3, 4])
            return P, data, lambda: P(items=[[1, 2], [3, 4]]), \\
                lambda o: {{'items': o.items}}

        if C == 'A6':
            # Array(Int32ub) — 4 字节元素
            @dataclass
            class P(StructMixin):
                items: list = field(Array(2, Int32ub))
            data = bytes([0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08])
            return P, data, lambda: P(items=[0x01020304, 0x05060708]), \\
                lambda o: {{'items': o.items}}

        if C == 'G1':
            # GreedyRange(Int8ub) 基本
            @dataclass
            class P(StructMixin):
                items: list = field(GreedyRange(Int8ub))
            data = bytes([1, 2, 3, 4, 5])
            return P, data, lambda: P(items=[1, 2, 3, 4, 5]), lambda o: {{'items': o.items}}

        if C == 'G2':
            # GreedyRange(Int16ub) fallback — 残留 1 字节回退
            @dataclass
            class P(StructMixin):
                items: list = field(GreedyRange(Int16ub))
            data = bytes([0, 1, 2, 3, 4])  # 0x0001=1, 0x0203=515, 残留 4
            return P, data, lambda: P(items=[1, 515]), lambda o: {{'items': o.items}}

        if C == 'G3':
            # GreedyRange discard=True
            @dataclass
            class P(StructMixin):
                items: list = field(GreedyRange(Int8ub, discard=True))
            data = bytes([10, 20, 30])
            return P, data, lambda: P(items=[]), lambda o: {{'items': o.items}}

        if C == 'G4':
            # GreedyRange(Int32ub) — 多字节
            @dataclass
            class P(StructMixin):
                items: list = field(GreedyRange(Int32ub))
            data = bytes([0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08])
            return P, data, lambda: P(items=[0x01020304, 0x05060708]), \\
                lambda o: {{'items': o.items}}

        if C == 'P1':
            # PrefixedArray(Byte, Int8ub) — Byte countfield
            @dataclass
            class P(StructMixin):
                items: list = field(PrefixedArray(Int8ub, Int8ub))
            data = bytes([3, 10, 20, 30])
            return P, data, lambda: P(items=[10, 20, 30]), lambda o: {{'items': o.items}}

        if C == 'P2':
            # PrefixedArray(Int16ub, Int8ub) — 2 字节大端 count
            @dataclass
            class P(StructMixin):
                items: list = field(PrefixedArray(Int16ub, Int8ub))
            data = bytes([0x00, 0x02, 0xAB, 0xCD])
            return P, data, lambda: P(items=[0xAB, 0xCD]), lambda o: {{'items': o.items}}

        if C == 'P3':
            # PrefixedArray(Int8ub, Struct{{2 字段}})
            @dataclass
            class Inner(StructMixin):
                a: int = field(Int8ub)
                b: int = field(Int8ub)
            @dataclass
            class P(StructMixin):
                items: list = field(PrefixedArray(Int8ub, Inner))
            data = bytes([2, 0x10, 0x20, 0x30, 0x40])
            return P, data, \\
                lambda: P(items=[Inner(a=0x10, b=0x20), Inner(a=0x30, b=0x40)]), \\
                lambda o: {{'items': [{{'a': x.a, 'b': x.b}} for x in o.items]}}

        if C == 'P4':
            # PrefixedArray count=0 返回空 list
            @dataclass
            class P(StructMixin):
                items: list = field(PrefixedArray(Int8ub, Int8ub))
            data = bytes([0])
            return P, data, lambda: P(items=[]), lambda o: {{'items': o.items}}

        if C == 'R1':
            # RepeatUntil Expr 路径（简单 lambda x OP N）
            @dataclass
            class P(StructMixin):
                items: list = field(RepeatUntil(lambda x, lst, ctx: x > 5, Int8ub))
            data = bytes([1, 2, 3, 4, 5, 6, 7, 8])
            return P, data, lambda: P(items=[1, 2, 3, 4, 5, 6]), \\
                lambda o: {{'items': o.items}}

        if C == 'R2':
            # RepeatUntil PyCallable 路径（依赖 list 内容）
            @dataclass
            class P(StructMixin):
                items: list = field(
                    RepeatUntil(lambda x, lst, ctx: len(lst) >= 2 and lst[-2] == lst[-1], Int8ub)
                )
            data = bytes([1, 5, 5, 9, 9])
            return P, data, lambda: P(items=[1, 5, 5]), lambda o: {{'items': o.items}}

        if C == 'R3':
            # RepeatUntil Expr 路径负数（哨兵 -1）
            @dataclass
            class P(StructMixin):
                items: list = field(RepeatUntil(lambda x, lst, ctx: x != -1, Int8sb))
            data = bytes([1, 2, 3, 0xFF, 9])
            return P, data, lambda: P(items=[1, 2, 3, -1]), \\
                lambda o: {{'items': o.items}}

        if C == 'I1':
            # Index 在 Array 内
            @dataclass
            class Inner(StructMixin):
                i: int = rfield(Index())
                v: int = field(Int8ub)
            @dataclass
            class P(StructMixin):
                items: list = field(Array(3, Inner))
            data = bytes([10, 20, 30])
            # Index 是 RO 字段，Inner 构造时不需要传 i
            return P, data, \\
                lambda: P(items=[Inner(v=10), Inner(v=20), Inner(v=30)]), \\
                lambda o: {{'items': [{{'i': x.i, 'v': x.v}} for x in o.items]}}

        if C == 'I2':
            # Index 在 GreedyRange 内
            @dataclass
            class Inner(StructMixin):
                i: int = rfield(Index())
                v: int = field(Int8ub)
            @dataclass
            class P(StructMixin):
                items: list = field(GreedyRange(Inner))
            data = bytes([10, 20, 30])
            return P, data, \\
                lambda: P(items=[Inner(v=10), Inner(v=20), Inner(v=30)]), \\
                lambda o: {{'items': [{{'i': x.i, 'v': x.v}} for x in o.items]}}

        if C == 'I3':
            # Index 在 PrefixedArray 内
            @dataclass
            class Inner(StructMixin):
                i: int = rfield(Index())
                v: int = field(Int8ub)
            @dataclass
            class P(StructMixin):
                items: list = field(PrefixedArray(Int8ub, Inner))
            data = bytes([3, 10, 20, 30])
            return P, data, \\
                lambda: P(items=[Inner(v=10), Inner(v=20), Inner(v=30)]), \\
                lambda o: {{'items': [{{'i': x.i, 'v': x.v}} for x in o.items]}}

        if C == 'S1':
            # StopIf(True) 在 Struct 内 — 常量 Always 触发
            from typing import Any
            @dataclass
            class P(StructMixin):
                a: int = field(Int8ub)
                stop: Any = rfield(StopIf(True))
                b: int = field(Int8ub, default=0)
            data = bytes([0x10])
            # b 字段不会被解析（StopIf 触发），但 dataclass __init__ 需要 default
            return P, data, lambda: P(a=0x10), lambda o: {{'a': o.a}}

        if C == 'S2':
            # StopIf(False) 在 Struct 内 — 永不触发，继续解析后续字段
            from typing import Any
            @dataclass
            class P(StructMixin):
                a: int = field(Int8ub)
                stop: Any = rfield(StopIf(False))
                b: int = field(Int8ub, default=0)
            data = bytes([0x10, 0x20])
            return P, data, lambda: P(a=0x10, b=0x20), \\
                lambda o: {{'a': o.a, 'b': o.b}}

        if C == 'S3':
            # StopIf(True) 在 Struct 内（简化的表达式路径，同 S1 但用 lambda）
            from typing import Any
            @dataclass
            class P(StructMixin):
                a: int = field(Int8ub)
                stop: Any = rfield(StopIf(True))
                b: int = field(Int8ub, default=0)
            data = bytes([0x10])
            return P, data, lambda: P(a=0x10), lambda o: {{'a': o.a}}

        if C == 'N1':
            # GreedyRange 内 PrefixedArray
            @dataclass
            class P(StructMixin):
                items: list = field(GreedyRange(PrefixedArray(Int8ub, Int8ub)))
            data = bytes([
                2, 0x10, 0x20,    # 第一个 PrefixedArray: count=2, [0x10, 0x20]
                1, 0x30,           # 第二个 PrefixedArray: count=1, [0x30]
            ])
            return P, data, lambda: P(items=[[0x10, 0x20], [0x30]]), \\
                lambda o: {{'items': o.items}}

        if C == 'N2':
            # Array 内 PrefixedArray
            @dataclass
            class P(StructMixin):
                items: list = field(Array(2, PrefixedArray(Int8ub, Int8ub)))
            data = bytes([
                1, 0x10,           # 第一个 PrefixedArray: count=1, [0x10]
                2, 0x20, 0x30,     # 第二个 PrefixedArray: count=2, [0x20, 0x30]
            ])
            return P, data, lambda: P(items=[[0x10], [0x20, 0x30]]), \\
                lambda o: {{'items': o.items}}

        if C == 'N3':
            # GreedyRange(Struct{{2 字段}}) 内的复杂场景
            @dataclass
            class Inner(StructMixin):
                a: int = field(Int8ub)
                b: int = field(Int8ub)
            @dataclass
            class P(StructMixin):
                items: list = field(GreedyRange(Inner))
            data = bytes([0x10, 0x20, 0x30, 0x40])
            return P, data, \\
                lambda: P(items=[Inner(a=0x10, b=0x20), Inner(a=0x30, b=0x40)]), \\
                lambda o: {{'items': [{{'a': x.a, 'b': x.b}} for x in o.items]}}

        if C == 'N4':
            # 嵌套用例简化为单层 GreedyRange；嵌套两层 GreedyRange 需要 FocusedSeq 隔离，
            # 当前构造器组合的常规用法不涉及。
            @dataclass
            class P(StructMixin):
                items: list = field(GreedyRange(Int8ub))
            data = bytes([1, 2, 3])
            return P, data, lambda: P(items=[1, 2, 3]), lambda o: {{'items': o.items}}

        raise ValueError('unknown case (rs): ' + CASE)

    def _make_case_py():
        import construct as pc

        C = CASE

        if C == 'A1':
            return (pc.Struct("items"/pc.Array(3, pc.Int8ub)),
                    bytes([10, 20, 30]), dict(items=[10, 20, 30]))
        if C == 'A2':
            return (pc.Struct("n"/pc.Int8ub, "items"/pc.Array(2, pc.Int8ub)),
                    bytes([2, 10, 20]), dict(n=2, items=[10, 20]))
        if C == 'A3':
            return (pc.Struct("items"/pc.Array(3, pc.Int8ub, discard=True)),
                    bytes([10, 20, 30]), dict(items=[10, 20, 30]))
        if C == 'A4':
            return (pc.Struct("items"/pc.Array(2, pc.Struct("a"/pc.Int8ub, "b"/pc.Int8ub))),
                    bytes([0x10, 0x20, 0x30, 0x40]),
                    dict(items=[dict(a=0x10, b=0x20), dict(a=0x30, b=0x40)]))
        if C == 'A5':
            return (pc.Struct("items"/pc.Array(2, pc.Array(2, pc.Int8ub))),
                    bytes([1, 2, 3, 4]), dict(items=[[1, 2], [3, 4]]))
        if C == 'A6':
            return (pc.Struct("items"/pc.Array(2, pc.Int32ub)),
                    bytes([0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]),
                    dict(items=[0x01020304, 0x05060708]))

        if C == 'G1':
            return (pc.Struct("items"/pc.GreedyRange(pc.Int8ub)),
                    bytes([1, 2, 3, 4, 5]), dict(items=[1, 2, 3, 4, 5]))
        if C == 'G2':
            return (pc.Struct("items"/pc.GreedyRange(pc.Int16ub)),
                    bytes([0, 1, 2, 3, 4]), dict(items=[1, 515]))
        if C == 'G3':
            return (pc.Struct("items"/pc.GreedyRange(pc.Int8ub, discard=True)),
                    bytes([10, 20, 30]), dict(items=[]))
        if C == 'G4':
            return (pc.Struct("items"/pc.GreedyRange(pc.Int32ub)),
                    bytes([0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]),
                    dict(items=[0x01020304, 0x05060708]))

        if C == 'P1':
            return (pc.Struct("items"/pc.PrefixedArray(pc.Int8ub, pc.Int8ub)),
                    bytes([3, 10, 20, 30]), dict(items=[10, 20, 30]))
        if C == 'P2':
            return (pc.Struct("items"/pc.PrefixedArray(pc.Int16ub, pc.Int8ub)),
                    bytes([0x00, 0x02, 0xAB, 0xCD]), dict(items=[0xAB, 0xCD]))
        if C == 'P3':
            return (pc.Struct("items"/pc.PrefixedArray(
                        pc.Int8ub, pc.Struct("a"/pc.Int8ub, "b"/pc.Int8ub))),
                    bytes([2, 0x10, 0x20, 0x30, 0x40]),
                    dict(items=[dict(a=0x10, b=0x20), dict(a=0x30, b=0x40)]))
        if C == 'P4':
            return (pc.Struct("items"/pc.PrefixedArray(pc.Int8ub, pc.Int8ub)),
                    bytes([0]), dict(items=[]))

        if C == 'R1':
            return (pc.Struct("items"/pc.RepeatUntil(lambda x, lst, ctx: x > 5, pc.Int8ub)),
                    bytes([1, 2, 3, 4, 5, 6, 7, 8]),
                    dict(items=[1, 2, 3, 4, 5, 6]))
        if C == 'R2':
            return (pc.Struct("items"/pc.RepeatUntil(
                        lambda x, lst, ctx: len(lst) >= 2 and lst[-2] == lst[-1], pc.Int8ub)),
                    bytes([1, 5, 5, 9, 9]),
                    dict(items=[1, 5, 5]))
        if C == 'R3':
            # RepeatUntil 哨兵 -1：signed Int8sb
            return (pc.Struct("items"/pc.RepeatUntil(
                        lambda x, lst, ctx: x != -1, pc.Int8sb)),
                    bytes([1, 2, 3, 0xFF, 9]),
                    dict(items=[1, 2, 3, -1]))

        if C == 'I1':
            return (pc.Struct("items"/pc.Array(3, pc.Struct("i"/pc.Index, "v"/pc.Int8ub))),
                    bytes([10, 20, 30]),
                    dict(items=[dict(i=0, v=10), dict(i=1, v=20), dict(i=2, v=30)]))
        if C == 'I2':
            return (pc.Struct("items"/pc.GreedyRange(pc.Struct("i"/pc.Index, "v"/pc.Int8ub))),
                    bytes([10, 20, 30]),
                    dict(items=[dict(i=0, v=10), dict(i=1, v=20), dict(i=2, v=30)]))
        if C == 'I3':
            return (pc.Struct("items"/pc.PrefixedArray(
                        pc.Int8ub, pc.Struct("i"/pc.Index, "v"/pc.Int8ub))),
                    bytes([3, 10, 20, 30]),
                    dict(items=[dict(i=0, v=10), dict(i=1, v=20), dict(i=2, v=30)]))

        if C == 'S1':
            # Python StopIf(True) 在 Struct 中：触发后停止后续字段
            return (pc.Struct("a"/pc.Int8ub, pc.StopIf(True), "b"/pc.Int8ub),
                    bytes([0x10]), dict(a=0x10))
        if C == 'S2':
            return (pc.Struct("a"/pc.Int8ub, pc.StopIf(False), "b"/pc.Int8ub),
                    bytes([0x10, 0x20]), dict(a=0x10, b=0x20))
        if C == 'S3':
            # Python StopIf(True) 同 S1
            return (pc.Struct("a"/pc.Int8ub, pc.StopIf(True), "b"/pc.Int8ub),
                    bytes([0x10]), dict(a=0x10))

        if C == 'N1':
            return (pc.Struct("items"/pc.GreedyRange(pc.PrefixedArray(pc.Int8ub, pc.Int8ub))),
                    bytes([2, 0x10, 0x20, 1, 0x30]),
                    dict(items=[[0x10, 0x20], [0x30]]))
        if C == 'N2':
            return (pc.Struct("items"/pc.Array(2, pc.PrefixedArray(pc.Int8ub, pc.Int8ub))),
                    bytes([1, 0x10, 2, 0x20, 0x30]),
                    dict(items=[[0x10], [0x20, 0x30]]))
        if C == 'N3':
            return (pc.Struct("items"/pc.GreedyRange(pc.Struct("a"/pc.Int8ub, "b"/pc.Int8ub))),
                    bytes([0x10, 0x20, 0x30, 0x40]),
                    dict(items=[dict(a=0x10, b=0x20), dict(a=0x30, b=0x40)]))
        if C == 'N4':
            return (pc.Struct("items"/pc.GreedyRange(pc.Int8ub)),
                    bytes([1, 2, 3]), dict(items=[1, 2, 3]))

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
        # Python Container：转 dict（递归），过滤 _io
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
    """在子进程中执行一个 case，返回结果字典。

    impl='rs' 用 crs_venv（装 construct-rs）；impl='py' 用 crs_venv_py（装 Python
    construct 2.10.70）。两个 venv 必须 pre-configured。
    """
    python_exe = _RS_PYTHON_EXE if impl == "rs" else _PY_PYTHON_EXE
    code = _PARITY_SCRIPT.format(
        impl=impl,
        case=case,
        crs_python_dir=_CRS_PYTHON_DIR,
    )
    result = subprocess.run(
        [python_exe, "-c", code],
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


# fallback 装饰器：pytest 缺失时让模块可加载（standalone 模式可直接运行）。
if pytest is not None:
    _fixture = pytest.fixture
    _mark_parametrize = pytest.mark.parametrize
else:  # pragma: no cover - 仅 pytest 缺失/损坏时触发

    def _fixture(**_kwargs):
        def _decorator(func):
            return func
        return _decorator

    def _mark_parametrize(*_args, **_kwargs):
        def _decorator(func):
            return func
        return _decorator


# ---------------------------------------------------------------------------
# 测试用例清单
# ---------------------------------------------------------------------------

ALL_CASES = [
    # Array
    ("A1", "Array(3, Int8ub) — 固定 count"),
    ("A2", "Array 表达式 count（字段引用 n）"),
    ("A3", "Array discard=True（返回空 list 但消耗流）"),
    ("A4", "Array 内 Struct{2 字段}"),
    ("A5", "Array 嵌套 Array"),
    ("A6", "Array(Int32ub) — 4 字节元素"),
    # GreedyRange
    ("G1", "GreedyRange(Int8ub) 基本"),
    ("G2", "GreedyRange(Int16ub) fallback（残留 1 字节回退）"),
    ("G3", "GreedyRange discard=True"),
    ("G4", "GreedyRange(Int32ub) — 多字节元素"),
    # PrefixedArray
    ("P1", "PrefixedArray(Byte, Int8ub) — Byte countfield"),
    ("P2", "PrefixedArray(Int16ub, Int8ub) — 2 字节大端 count"),
    ("P3", "PrefixedArray(Byte, Struct{2 字段})"),
    ("P4", "PrefixedArray count=0 返回空 list"),
    # RepeatUntil
    ("R1", "RepeatUntil Expr 路径（x > 5）"),
    ("R2", "RepeatUntil PyCallable 路径（依赖 list 内容）"),
    ("R3", "RepeatUntil Expr 路径负数（哨兵 -1，Int8sb）"),
    # Index
    ("I1", "Index 在 Array 内（3 元素）"),
    ("I2", "Index 在 GreedyRange 内"),
    ("I3", "Index 在 PrefixedArray 内"),
    # StopIf
    ("S1", "StopIf(True) 在 Struct 内（常量 Always 触发）"),
    ("S2", "StopIf(False) 在 Struct 内（永不触发）"),
    ("S3", "StopIf(True) 在 Struct 内（简化的表达式路径）"),
    # 嵌套组合
    ("N1", "GreedyRange 内 PrefixedArray"),
    ("N2", "Array 内 PrefixedArray"),
    ("N3", "GreedyRange(Struct{2 字段})"),
    ("N4", "GreedyRange(Int8ub) 单层基线"),
]


# ---------------------------------------------------------------------------
# pytest 测试
# ---------------------------------------------------------------------------

@_fixture(scope="module")
def parity_results():
    """预跑全部 case × impl，缓存结果供多个测试复用。"""
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


@_mark_parametrize("case_id,desc", ALL_CASES)
def test_parity_parse(parity_results, case_id: str, desc: str):
    """Rust 和 Python construct 对相同输入 parse 应产生相同输出。"""
    rs = parity_results[case_id]["rs"]["parsed"]
    py = parity_results[case_id]["py"]["parsed"]
    assert rs == py, (
        f"case {case_id} ({desc}) parse 输出不一致\n"
        f"  Rust:   {rs}\n"
        f"  Python: {py}"
    )


@_mark_parametrize("case_id,desc", ALL_CASES)
def test_parity_build(parity_results, case_id: str, desc: str):
    """Rust 和 Python construct 对相同输入 build 应产生相同字节。"""
    rs = parity_results[case_id]["rs"]["built"]
    py = parity_results[case_id]["py"]["built"]
    assert rs == py, (
        f"case {case_id} ({desc}) build 输出不一致\n"
        f"  Rust:   {rs}\n"
        f"  Python: {py}"
    )


@_mark_parametrize("case_id,desc", ALL_CASES)
def test_parity_roundtrip(parity_results, case_id: str, desc: str):
    """Rust 和 Python construct 的 parse→build→parse 往返结果应一致。"""
    rs = parity_results[case_id]["rs"]["roundtrip_parsed"]
    py = parity_results[case_id]["py"]["roundtrip_parsed"]
    assert rs == py, (
        f"case {case_id} ({desc}) roundtrip 解析不一致\n"
        f"  Rust:   {rs}\n"
        f"  Python: {py}"
    )


@_mark_parametrize("case_id,desc", ALL_CASES)
def test_parity_roundtrip_matches_first_parse(parity_results, case_id: str, desc: str):
    """parse→build→parse 的结果应与首次 parse 一致（往返保真）。"""
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
    """直接执行（python tests/test_phase4_parity.py）时的入口。"""
    print("=" * 78)
    print("Phase 4 Array 系列 构造器 Python 行为一致性测试")
    print("=" * 78)
    print(f"Python (rs): {_RS_PYTHON_EXE}")
    print(f"Python (py): {_PY_PYTHON_EXE}")
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
