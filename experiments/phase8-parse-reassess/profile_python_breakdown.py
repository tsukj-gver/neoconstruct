"""Python 原版 construct 2.10.70 parse 路径开销拆解 profiling（ARCH L-14 交叉验证）。

目的：
  对 14 项 parse <10x case + FE1-build，量化 Python 原版各构造器 parse 一次的
  "Adapter/Struct 调度框架税" vs "核心操作（字节解析 + 对象构造）" 占比。

  判定标准（用户决策标准）：
  - 框架税占比 ≥ 70%（核心操作 ≤ 30%）→ "Python 原版原始性能差" → 应能 10x+
  - 框架税占比 ≤ 50%（核心操作 ≥ 50%）→ "Python 原版原始性能好" → 10x 可能数学不可达
  - 中间 → 临界

测量三层（L-02 对策，量化数据非理论推算）：
  T_full        : 完整 `Struct(...).parse(data)` —— 与 bench_phase8 同口径（校验基准）
  T_subcon_only : 直接 `subcon._parse(stream, ctx, path)`（绕过 Struct 顶层但保留 Adapter 包装）
  T_core        : 核心操作的最朴素 Python 等价（绕过整个 construct 框架）

派生：
  框架税        = T_full - T_core（Container + path + Subconstruct._parse 包装 + Adapter._parse 包装 + 解释器调度）
  Adapter 包装税 = T_full - T_subcon_only
  核心操作占比   = T_core / T_full

说明：本脚本仅用 Python 原版 construct 2.10.70（不加载 construct-rs），
通过 PYTHONPATH 注入项目根 construct 子目录覆盖 site-packages 的残缺 namespace 包。
"""

import gc
import io
import os
import sys
import time
import timeit
import platform
import collections
import struct as _struct

# ---------------------------------------------------------------------------
# 强制使用项目根的原版 construct 2.10.70（覆盖 site-packages 的残缺 namespace 包）
# ---------------------------------------------------------------------------
_PROJ_ROOT = r"<legacy-repo>"
_CONSTRUCT_DIR = os.path.join(_PROJ_ROOT, "construct")
if _CONSTRUCT_DIR not in sys.path:
    sys.path.insert(0, _CONSTRUCT_DIR)
# 清理可能已加载的 namespace construct
for _k in list(sys.modules):
    if _k == "construct" or _k.startswith("construct."):
        del sys.modules[_k]

import construct as pc
from construct import (
    Struct, Const, Hex, HexDump, Enum, FlagsEnum, Mapping, OneOf, NoneOf,
    Terminated, Aligned, NamedTuple, Bytes, Byte, Int8ub, Int16ub, Int32ub, Int32ul,
    this,
)
from construct.lib.hex import (
    HexDisplayedInteger, HexDisplayedBytes, HexDisplayedDict,
    HexDumpDisplayedBytes, HexDumpDisplayedDict,
)
from construct.core import Container, FormatField


def bench(func, number=50000, repeat=7):
    """timeit 微基准，返回 best ns/op。"""
    gc.collect()
    times = timeit.repeat(func, number=number, repeat=repeat)
    return min(times) / number * 1e9


# ---------------------------------------------------------------------------
# 共享：构造 context + path（模拟 Struct 顶层 parse_stream 的环境）
# ---------------------------------------------------------------------------
def make_ctx():
    """模拟 construct.core.parse_stream 创建的 context。"""
    ctx = Container()
    ctx._parsing = True
    ctx._building = False
    ctx._sizing = False
    ctx._params = ctx
    return ctx


_PATH = "(parsing)"


# ---------------------------------------------------------------------------
# 工具：bytes → stream 包装（避免每个 case 重新 import io）
# ---------------------------------------------------------------------------
def _stream(data):
    return io.BytesIO(data)


# ---------------------------------------------------------------------------
# 各 case 的 3 级测量定义
# ---------------------------------------------------------------------------
# 每个 case 返回 dict：
#   {case_id, desc, t_full, t_subcon, t_core, full_ns, subcon_ns, core_ns}
# t_full/t_subcon/t_core 是 callable，被 bench() 调用

def make_CN1():
    """CN1: Const(b'IHDR') bytes 单字段，parse。"""
    s = Struct("m" / Const(b"IHDR"))
    data = b"IHDR"
    subcon = Const(b"IHDR")
    expected = b"IHDR"

    def t_full():
        s.parse(data)

    def t_subcon():
        st = _stream(data)
        subcon._parsereport(st, make_ctx(), _PATH)

    def t_core():
        # 核心：bytes 比较（仅字节读取 + 等值判断）
        data == expected

    return t_full, t_subcon, t_core


def make_CN2():
    """CN2: Const(255, Int32ul) int 单字段，parse。"""
    s = Struct("m" / Const(255, Int32ul))
    data = b'\xff\x00\x00\x00'
    subcon = Const(255, Int32ul)
    expected_int = 255

    def t_full():
        s.parse(data)

    def t_subcon():
        st = _stream(data)
        subcon._parsereport(st, make_ctx(), _PATH)

    def t_core():
        # 核心：struct.unpack + int 比较
        val = _struct.unpack("<I", data)[0]
        val == expected_int

    return t_full, t_subcon, t_core


def make_HX1():
    """HX1: Hex(Int32ub) int 显示，parse。"""
    s = Struct("v" / Hex(Int32ub))
    data = b'\x00\x00\x01\x02'
    subcon = Hex(Int32ub)
    inner = Int32ub
    intvalue = 258
    fmtstr = "08X"

    def t_full():
        s.parse(data)

    def t_subcon():
        st = _stream(data)
        subcon._parsereport(st, make_ctx(), _PATH)

    def t_core():
        # 核心：读 int + HexDisplayedInteger.new(intvalue, fmtstr)
        # 用预解析的 intvalue 模拟"如果 bytes 已读出 int，剩余就是 new 调用"
        HexDisplayedInteger.new(intvalue, fmtstr)

    return t_full, t_subcon, t_core


def make_HX2():
    """HX2: Hex(Bytes(4)) bytes 显示，parse。"""
    s = Struct("v" / Hex(Bytes(4)))
    data = b'\x00\x00\x01\x02'
    subcon = Hex(Bytes(4))

    def t_full():
        s.parse(data)

    def t_subcon():
        st = _stream(data)
        subcon._parsereport(st, make_ctx(), _PATH)

    def t_core():
        # 核心：bytes 切片 + HexDisplayedBytes(obj) 构造
        HexDisplayedBytes(data[:4])

    return t_full, t_subcon, t_core


def make_HD1():
    """HD1: HexDump(Bytes(4)) bytes 显示，parse。"""
    s = Struct("v" / HexDump(Bytes(4)))
    data = b'\x00\x00\x01\x02'
    subcon = HexDump(Bytes(4))

    def t_full():
        s.parse(data)

    def t_subcon():
        st = _stream(data)
        subcon._parsereport(st, make_ctx(), _PATH)

    def t_core():
        # 核心：bytes 切片 + HexDumpDisplayedBytes(obj) 构造
        HexDumpDisplayedBytes(data[:4])

    return t_full, t_subcon, t_core


def make_AL2():
    """AL2: Aligned(8, Bytes(3)) modulus=8，parse。"""
    s = Struct("v" / Aligned(8, Bytes(3)))
    data = b'\x01\x02\x03' + b'\x00' * 5
    subcon = Aligned(8, Bytes(3))

    def t_full():
        s.parse(data)

    def t_subcon():
        st = _stream(data)
        subcon._parsereport(st, make_ctx(), _PATH)

    def t_core():
        # 核心：bytes 切片（modulus padding 仅是 stream 位置推进，不算"逻辑操作"）
        data[:3]

    return t_full, t_subcon, t_core


def make_TM1():
    """TM1: Terminated at EOF，parse（嵌在 Struct{Byte, Terminated}）。"""
    s = Struct("v" / Byte, Terminated)
    data = b'\x01'
    subcon = Terminated  # Terminated 是 singleton instance（非 class）

    def t_full():
        s.parse(data)

    def t_subcon():
        st = _stream(b'')  # Terminated 期望 EOF
        subcon._parsereport(st, make_ctx(), _PATH)

    def t_core():
        # 核心：stream.read(1) 是否为空（仅 EOF 检查）
        # 用 data 的 bytes 切片模拟（实际无字节读取）
        len(data) == 1  # 仅比较，对应 "还有数据则报错" 的反向判定

    return t_full, t_subcon, t_core


def make_EN1():
    """EN1: Enum(Byte, 3 mappings) 正常映射，parse。"""
    decmapping = {1: "one", 2: "two", 3: "three"}
    s = Struct("e" / Enum(Byte, **{v: k for k, v in decmapping.items()}))
    data = b'\x01'
    subcon = Enum(Byte, **{v: k for k, v in decmapping.items()})

    def t_full():
        s.parse(data)

    def t_subcon():
        st = _stream(data)
        subcon._parsereport(st, make_ctx(), _PATH)

    def t_core():
        # 核心：读 byte（struct.unpack）+ dict 查找
        val = data[0]
        decmapping[val]

    return t_full, t_subcon, t_core


def make_EN2():
    """EN2: Enum(Byte, 8 mappings)，parse。"""
    decmapping = {1: "one", 2: "two", 3: "three", 4: "four",
                  5: "five", 6: "six", 7: "seven", 8: "eight"}
    s = Struct("e" / Enum(Byte, **{v: k for k, v in decmapping.items()}))
    data = b'\x04'
    subcon = Enum(Byte, **{v: k for k, v in decmapping.items()})

    def t_full():
        s.parse(data)

    def t_subcon():
        st = _stream(data)
        subcon._parsereport(st, make_ctx(), _PATH)

    def t_core():
        val = data[0]
        decmapping[val]

    return t_full, t_subcon, t_core


def make_FE1_parse():
    """FE1 parse: FlagsEnum(Byte, 4 flags)，parse。"""
    flags = {"one": 1, "two": 2, "four": 4, "eight": 8}
    s = Struct("f" / FlagsEnum(Byte, **flags))
    data = b'\x05'
    subcon = FlagsEnum(Byte, **flags)

    def t_full():
        s.parse(data)

    def t_subcon():
        st = _stream(data)
        subcon._parsereport(st, make_ctx(), _PATH)

    def t_core():
        # 核心：4 次位运算 + Container() 构造（与 FlagsEnum._decode 等价）
        val = data[0]
        obj2 = Container()
        obj2._flagsenum = True
        obj2["one"] = (val & 1 == 1)
        obj2["two"] = (val & 2 == 2)
        obj2["four"] = (val & 4 == 4)
        obj2["eight"] = (val & 8 == 8)

    return t_full, t_subcon, t_core


def make_FE1_build():
    """FE1 build: FlagsEnum(Byte, 4 flags)，build (dict→int)。"""
    flags = {"one": 1, "two": 2, "four": 4, "eight": 8}
    s = Struct("f" / FlagsEnum(Byte, **flags))
    build_obj = dict(f=dict(one=True, two=False, four=True, eight=False))
    subcon = FlagsEnum(Byte, **flags)

    def t_full():
        s.build(build_obj)

    def t_subcon():
        # build 路径：encode dict → int → Byte._build
        # 这里测纯 FlagsEnum._encode(dict) → int
        st = _stream(b'')
        subcon._build(dict(one=True, two=False, four=True, eight=False),
                      st, make_ctx(), _PATH)

    def t_core():
        # 核心：dict 遍历 + 4 次位或（与 FlagsEnum._encode 等价）
        obj = dict(one=True, two=False, four=True, eight=False)
        flags_val = 0
        for name, value in obj.items():
            if not name.startswith("_"):
                if value:
                    flags_val |= flags[name]

    return t_full, t_subcon, t_core


def make_MP1():
    """MP1: Mapping(Byte, 3 pairs)，parse。"""
    mapping_py = {"a": 1, "b": 2, "c": 3}  # construct 原版 Mapping(label: wirevalue)
    decmapping = {v: k for k, v in mapping_py.items()}
    s = Struct("m" / Mapping(Byte, mapping_py))
    data = b'\x01'
    subcon = Mapping(Byte, mapping_py)

    def t_full():
        s.parse(data)

    def t_subcon():
        st = _stream(data)
        subcon._parsereport(st, make_ctx(), _PATH)

    def t_core():
        val = data[0]
        decmapping[val]

    return t_full, t_subcon, t_core


def make_OO1():
    """OO1: OneOf(Byte, [1,2,3])，parse。"""
    valids = [1, 2, 3]
    s = Struct("v" / OneOf(Byte, valids))
    data = b'\x01'
    subcon = OneOf(Byte, valids)

    def t_full():
        s.parse(data)

    def t_subcon():
        st = _stream(data)
        subcon._parsereport(st, make_ctx(), _PATH)

    def t_core():
        # 核心：读 byte + in list 检查
        val = data[0]
        val in valids

    return t_full, t_subcon, t_core


def make_NO1():
    """NO1: NoneOf(Byte, [1,2,3])，parse。"""
    invalids = [1, 2, 3]
    s = Struct("v" / NoneOf(Byte, invalids))
    data = b'\xff'
    subcon = NoneOf(Byte, invalids)

    def t_full():
        s.parse(data)

    def t_subcon():
        st = _stream(data)
        subcon._parsereport(st, make_ctx(), _PATH)

    def t_core():
        val = data[0]
        val not in invalids

    return t_full, t_subcon, t_core


def make_NT1():
    """NT1: NamedTuple over Struct{x: Byte, y: Byte}，parse。"""
    inner_struct = Struct("x" / Int8ub, "y" / Int8ub)
    s = Struct("nt" / NamedTuple("coord", "x y", inner_struct))
    data = b'\x01\x02'
    subcon = NamedTuple("coord", "x y", inner_struct)
    Coord = collections.namedtuple("coord", "x y")

    def t_full():
        s.parse(data)

    def t_subcon():
        st = _stream(data)
        subcon._parsereport(st, make_ctx(), _PATH)

    def t_core():
        # 核心：2 字节读取 + namedtuple 实例化（绕过 Struct/Adapter 包装）
        x = data[0]
        y = data[1]
        Coord(x=x, y=y)

    return t_full, t_subcon, t_core


# ---------------------------------------------------------------------------
# Case 注册表
# ---------------------------------------------------------------------------
CASES = [
    ("CN1",       "Const b'IHDR' bytes",                    make_CN1),
    ("CN2",       "Const(255, Int32ul) int",                make_CN2),
    ("HX1",       "Hex(Int32ub) int 显示",                  make_HX1),
    ("HX2",       "Hex(Bytes(4)) bytes 显示",               make_HX2),
    ("HD1",       "HexDump(Bytes(4)) bytes 显示",           make_HD1),
    ("AL2",       "Aligned(8, Bytes(3))",                   make_AL2),
    ("TM1",       "Terminated at EOF（嵌 Struct）",         make_TM1),
    ("EN1",       "Enum(Byte, 3 mappings)",                 make_EN1),
    ("EN2",       "Enum(Byte, 8 mappings)",                 make_EN2),
    ("FE1-parse", "FlagsEnum(Byte, 4 flags) parse",         make_FE1_parse),
    ("FE1-build", "FlagsEnum(Byte, 4 flags) build dict→int", make_FE1_build),
    ("MP1",       "Mapping(Byte, 3 pairs)",                 make_MP1),
    ("OO1",       "OneOf(Byte, [1,2,3])",                   make_OO1),
    ("NO1",       "NoneOf(Byte, [1,2,3])",                  make_NO1),
    ("NT1",       "NamedTuple over Struct{2}",              make_NT1),
]


def main():
    print("=" * 96)
    print("Python 原版 construct 2.10.70 parse 路径开销拆解 profiling")
    print(f"Python {sys.version.split()[0]} | {platform.machine()} | {platform.system()}")
    print(f"construct path: {pc.__file__}")
    print(f"number=50000, repeat=7 (best ns/op)")
    print("=" * 96)
    print()
    print(f"{'Case':<11} {'T_full':>8} {'T_subcon':>9} {'T_core':>8}  "
          f"{'框架税':>8} {'Adapter税':>9} {'核心占比':>8}  {'判定':<14} 说明")
    print("-" * 96)

    results = []
    for case_id, desc, factory in CASES:
        t_full, t_subcon, t_core = factory()
        full_ns = bench(t_full)
        subcon_ns = bench(t_subcon)
        core_ns = bench(t_core)
        framework_tax = full_ns - core_ns
        adapter_tax = full_ns - subcon_ns
        core_ratio = core_ns / full_ns if full_ns else 0.0

        # 判定（用户决策标准）
        if core_ratio >= 0.50:
            verdict = "原始好(<10x)"
        elif core_ratio <= 0.30:
            verdict = "原始差(应10x+)"
        else:
            verdict = "临界"

        print(f"{case_id:<11} {full_ns:>8.0f} {subcon_ns:>9.0f} {core_ns:>8.0f}  "
              f"{framework_tax:>+8.0f} {adapter_tax:>+9.0f} {core_ratio*100:>7.1f}%  {verdict:<14} {desc}")
        results.append({
            "case_id": case_id,
            "desc": desc,
            "full_ns": full_ns,
            "subcon_ns": subcon_ns,
            "core_ns": core_ns,
            "framework_tax_ns": framework_tax,
            "adapter_tax_ns": adapter_tax,
            "core_ratio": core_ratio,
            "verdict": verdict,
        })

    print()
    print("=" * 96)
    print("派生分析")
    print("=" * 96)

    # 框架税占比汇总
    print(f"\n{'Case':<11} {'框架税占比':>10} {'Adapter税占比':>13} {'subcon→core':>12}  说明")
    print("-" * 96)
    for r in results:
        full = r["full_ns"]
        fw_ratio = r["framework_tax_ns"] / full * 100 if full else 0
        ad_ratio = r["adapter_tax_ns"] / full * 100 if full else 0
        # subcon → core 的差额 = _decode/_parse 后处理本身的开销
        subcon_to_core = r["subcon_ns"] - r["core_ns"]
        print(f"{r['case_id']:<11} {fw_ratio:>9.1f}% {ad_ratio:>12.1f}% "
              f"{subcon_to_core:>+11.0f}ns  {r['desc']}")

    # 输出 JSON（供报告引用）
    import json
    out_path = os.path.join(os.path.dirname(__file__), "profile_python_breakdown.json")
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump({
            "python_version": sys.version.split()[0],
            "construct_path": pc.__file__,
            "number": 50000,
            "repeat": 7,
            "results": results,
        }, f, indent=2, ensure_ascii=False)
    print(f"\nJSON 已写入: {out_path}")


if __name__ == "__main__":
    main()
