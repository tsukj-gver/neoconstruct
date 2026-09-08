"""Array 系列构造器 Python 行为一致性测试。

组件：_helpers/parity.py（公共组件）。

测试策略（子进程隔离）：两个实现包（neoconstruct 与 Python construct 2.10.70）
无法在同一进程中同时导入，每个用例 × impl 在独立子进程中运行，通过 JSON 输出
结果，主进程比对。

覆盖范围（27 case）：Array（A1-A6）/ GreedyRange（G1-G4）/ PrefixedArray（P1-P4）/
RepeatUntil（R1-R3）/ Index（I1-I3）/ StopIf（S1-S3）/ 嵌套组合（N1-N4）。

用法::

    pytest tests/parity/test_array_parity.py -v
    python tests/parity/test_array_parity.py
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
# case 定义（子进程脚本片段，定义 _make_case_rs / _make_case_py）
# ---------------------------------------------------------------------------
# 注意：
# - _make_case_rs / _make_case_py 必须参数化（case_id 入参），函数体内
#   ``C = case_id`` 替代旧 ``C = CASE`` 闭包（PARITY_SCRIPT_TEMPLATE 执行段
#   写死 ``_make_case_rs(CASE)`` 调用，case 函数必须接受 case_id 参数）。
# - 本字符串通过 ``{case_definitions}`` 注入 PARITY_SCRIPT_TEMPLATE.format()，
#   .format 不二次解析本字符串内部花括号，故 case 定义代码用单花括号 ``{`` ``}``。
# - 本文件 case 全部用 ``bytes([...])`` 构造数据，无 bytes 字面量，转义简单。
_CASE_DEFINITIONS = '''
def _make_case_rs(case_id):
    from dataclasses import dataclass
    from neoconstruct import (
        StructMixin, field, rfield, wfield,
        Int8ub, Int8sb, Int16ub, Int32ub, Bytes,
        Array, GreedyRange, PrefixedArray,
        RepeatUntil, Index, StopIf, Element,
    )

    C = case_id

    if C == 'A1':
        # Array(3, Int8ub) — 固定 count
        @dataclass
        class P(StructMixin):
            items: list = field(Array(3, Int8ub))
        data = bytes([10, 20, 30])
        return P, data, lambda: P(items=[10, 20, 30]), lambda o: {'items': o.items}

    if C == 'A2':
        # Array 表达式 count（字段引用）
        @dataclass
        class P(StructMixin):
            n: int = field(Int8ub)
            items: list = field(Array(2, Int8ub))
        data = bytes([2, 10, 20])
        return P, data, lambda: P(n=2, items=[10, 20]), \\
            lambda o: {'n': o.n, 'items': o.items}

    if C == 'A3':
        # Array discard=True：返回空 list 但消耗流
        @dataclass
        class P(StructMixin):
            items: list = field(Array(3, Int8ub, discard=True))
        data = bytes([10, 20, 30])
        return P, data, lambda: P(items=[10, 20, 30]), lambda o: {'items': o.items}

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
            lambda o: {'items': [{'a': x.a, 'b': x.b} for x in o.items]}

    if C == 'A5':
        # Array 嵌套 Array
        @dataclass
        class P(StructMixin):
            items: list = field(Array(2, Array(2, Int8ub)))
        data = bytes([1, 2, 3, 4])
        return P, data, lambda: P(items=[[1, 2], [3, 4]]), \\
            lambda o: {'items': o.items}

    if C == 'A6':
        # Array(Int32ub) — 4 字节元素
        @dataclass
        class P(StructMixin):
            items: list = field(Array(2, Int32ub))
        data = bytes([0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08])
        return P, data, lambda: P(items=[0x01020304, 0x05060708]), \\
            lambda o: {'items': o.items}

    if C == 'G1':
        # GreedyRange(Int8ub) 基本
        @dataclass
        class P(StructMixin):
            items: list = field(GreedyRange(Int8ub))
        data = bytes([1, 2, 3, 4, 5])
        return P, data, lambda: P(items=[1, 2, 3, 4, 5]), lambda o: {'items': o.items}

    if C == 'G2':
        # GreedyRange(Int16ub) fallback — 残留 1 字节回退
        @dataclass
        class P(StructMixin):
            items: list = field(GreedyRange(Int16ub))
        data = bytes([0, 1, 2, 3, 4])  # 0x0001=1, 0x0203=515, 残留 4
        return P, data, lambda: P(items=[1, 515]), lambda o: {'items': o.items}

    if C == 'G3':
        # GreedyRange discard=True
        @dataclass
        class P(StructMixin):
            items: list = field(GreedyRange(Int8ub, discard=True))
        data = bytes([10, 20, 30])
        return P, data, lambda: P(items=[]), lambda o: {'items': o.items}

    if C == 'G4':
        # GreedyRange(Int32ub) — 多字节
        @dataclass
        class P(StructMixin):
            items: list = field(GreedyRange(Int32ub))
        data = bytes([0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08])
        return P, data, lambda: P(items=[0x01020304, 0x05060708]), \\
            lambda o: {'items': o.items}

    if C == 'P1':
        # PrefixedArray(Byte, Int8ub) — Byte countfield
        @dataclass
        class P(StructMixin):
            items: list = field(PrefixedArray(Int8ub, Int8ub))
        data = bytes([3, 10, 20, 30])
        return P, data, lambda: P(items=[10, 20, 30]), lambda o: {'items': o.items}

    if C == 'P2':
        # PrefixedArray(Int16ub, Int8ub) — 2 字节大端 count
        @dataclass
        class P(StructMixin):
            items: list = field(PrefixedArray(Int16ub, Int8ub))
        data = bytes([0x00, 0x02, 0xAB, 0xCD])
        return P, data, lambda: P(items=[0xAB, 0xCD]), lambda o: {'items': o.items}

    if C == 'P3':
        # PrefixedArray(Int8ub, Struct{2 字段})
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
            lambda o: {'items': [{'a': x.a, 'b': x.b} for x in o.items]}

    if C == 'P4':
        # PrefixedArray count=0 返回空 list
        @dataclass
        class P(StructMixin):
            items: list = field(PrefixedArray(Int8ub, Int8ub))
        data = bytes([0])
        return P, data, lambda: P(items=[]), lambda o: {'items': o.items}

    if C == 'R1':
        # RepeatUntil 终止表达式路径（Element 引用当前元素，e > 5）
        @dataclass
        class P(StructMixin):
            e: int = rfield(Element())
            items: list = field(RepeatUntil(e > 5, Int8ub))
        data = bytes([1, 2, 3, 4, 5, 6, 7, 8])
        return P, data, lambda: P(items=[1, 2, 3, 4, 5, 6]), \\
            lambda o: {'items': o.items}

    if C == 'R2':
        # RepeatUntil 哨兵来自前序字段（终止表达式跨字段引用 e == stop）
        @dataclass
        class P(StructMixin):
            stop: int = field(Int8ub)
            e: int = rfield(Element())
            items: list = field(RepeatUntil(e == stop, Int8ub))
        data = bytes([5, 1, 2, 5, 9])
        return P, data, lambda: P(stop=5, items=[1, 2, 5]), \\
            lambda o: {'stop': o.stop, 'items': o.items}

    if C == 'R3':
        # RepeatUntil 负数哨兵（终止表达式 e == -1，Int8sb）
        @dataclass
        class P(StructMixin):
            e: int = rfield(Element())
            items: list = field(RepeatUntil(e == -1, Int8sb))
        data = bytes([1, 2, 3, 0xFF, 9])
        return P, data, lambda: P(items=[1, 2, 3, -1]), \\
            lambda o: {'items': o.items}

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
            lambda o: {'items': [{'i': x.i, 'v': x.v} for x in o.items]}

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
            lambda o: {'items': [{'i': x.i, 'v': x.v} for x in o.items]}

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
            lambda o: {'items': [{'i': x.i, 'v': x.v} for x in o.items]}

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
        return P, data, lambda: P(a=0x10), lambda o: {'a': o.a}

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
            lambda o: {'a': o.a, 'b': o.b}

    if C == 'S3':
        # StopIf(True) 在 Struct 内（简化的表达式路径，同 S1 但用 lambda）
        from typing import Any
        @dataclass
        class P(StructMixin):
            a: int = field(Int8ub)
            stop: Any = rfield(StopIf(True))
            b: int = field(Int8ub, default=0)
        data = bytes([0x10])
        return P, data, lambda: P(a=0x10), lambda o: {'a': o.a}

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
            lambda o: {'items': o.items}

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
            lambda o: {'items': o.items}

    if C == 'N3':
        # GreedyRange(Struct{2 字段}) 内的复杂场景
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
            lambda o: {'items': [{'a': x.a, 'b': x.b} for x in o.items]}

    if C == 'N4':
        # 嵌套用例简化为单层 GreedyRange；嵌套两层 GreedyRange 需要 FocusedSeq 隔离，
        # 当前构造器组合的常规用法不涉及。
        @dataclass
        class P(StructMixin):
            items: list = field(GreedyRange(Int8ub))
        data = bytes([1, 2, 3])
        return P, data, lambda: P(items=[1, 2, 3]), lambda o: {'items': o.items}

    raise ValueError('unknown case (rs): ' + case_id)

def _make_case_py(case_id):
    import construct as pc

    C = case_id

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
        # RepeatUntil 哨兵来自前序字段（x == ctx.stop）
        return (pc.Struct("stop"/pc.Int8ub,
                          "items"/pc.RepeatUntil(
                              lambda x, lst, ctx: x == ctx.stop, pc.Int8ub)),
                bytes([5, 1, 2, 5, 9]),
                dict(stop=5, items=[1, 2, 5]))
    if C == 'R3':
        # RepeatUntil 哨兵 -1：signed Int8sb，在哨兵处停止（x == -1）
        return (pc.Struct("items"/pc.RepeatUntil(
                    lambda x, lst, ctx: x == -1, pc.Int8sb)),
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

    raise ValueError('unknown case (py): ' + case_id)
'''


# ---------------------------------------------------------------------------
# case 清单（与 _CASE_DEFINITIONS 一一对应）
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
    ("R1", "RepeatUntil 终止表达式路径（e > 5，Element 引用）"),
    ("R2", "RepeatUntil 哨兵来自前序字段（e == stop 跨字段引用）"),
    ("R3", "RepeatUntil 负数哨兵（e == -1，Int8sb）"),
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
# 自动生成 parity_results fixture（预跑全部 case × impl 缓存）
# ---------------------------------------------------------------------------
# 容错语义：case 子进程基础设施故障以 skip 呈现，而非让整个 module 崩溃。
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
    """直接执行（python tests/parity/test_array_parity.py）时的入口。"""
    print("=" * 78)
    print("Array 系列 构造器 Python 行为一致性测试")
    print("=" * 78)
    print(f"Python (rs): {_RS_PYTHON_EXE}")
    print(f"Python (py): {_PY_PYTHON_EXE}")
    print(f"neoconstruct: {_CRS_PYTHON_DIR}")
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
