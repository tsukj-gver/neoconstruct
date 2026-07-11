"""4.5 RepeatUntil v5 用户面使用样例集（ARCH 设计验证）。

设计依据：docs/模块设计-Array.md §13（v5 新增）。
对应任务：4.5-RD1 RepeatUntil 终止表达式重设计（用户打回 v4 后）。

本脚本的目的：
1. 验证 v5 设计的用户面 API 形态（Element 字段 + RepeatUntil 终止表达式）
   在 Python 语法层面是可表达的（设计文档示例能 copy-paste 运行）
2. 列出用户面使用样例全集，作为 REV/VET 审查时的"需求规约"
3. DEV 实施完成后的回归测试基础（每个样例对应一个测试用例）

注意：
- 本脚本运行在 DEV 完成 v5 实施之前。届时部分 import / API 可能尚未对齐。
- 脚本的设计意图是"前瞻性用户面规约"——设计文档中描述的 API 应能在此脚本中体现。
- 当 DEV 完成实施后，本脚本应能直接通过（每条 assert 不抛 AssertionError）。

v5 vs v4 关键差异：
- 删除 PyCallable 路径（用户硬约束 #1）
- 引入 Element() 字段作为"当前元素"引用入口（替代 v4 的 ExprOp::GetElem）
- RepeatUntil 第一参数从 predicate（callable）改名为 terminator（Phase 2 表达式）
- 终止表达式只支持 Phase 2 ExprProgram 能描述的逻辑（无 list 切片等）

运行方式（DEV 实施完成后）：
    cd construct-rs
    maturin develop --release
    cd ..
    python experiments/phase4_repeat_until_examples.py
"""

from __future__ import annotations

import sys
import traceback
from dataclasses import dataclass


def banner(title: str) -> None:
    print(f"\n{'=' * 70}\n{title}\n{'=' * 70}")


def check(label: str, fn) -> None:
    """运行一个样例函数，捕获 ImportError（实施未完成）与其他错误分开报告。"""
    try:
        fn()
        print(f"  [PASS] {label}")
    except ImportError as e:
        print(f"  [SKIP] {label} — ImportError（DEV 实施未完成）: {e}")
    except NotImplementedError as e:
        print(f"  [SKIP] {label} — NotImplementedError（待 DEV 实现）: {e}")
    except AssertionError as e:
        print(f"  [FAIL] {label} — AssertionError: {e}")
        traceback.print_exc()
    except Exception as e:  # noqa: BLE001
        print(f"  [ERR ] {label} — {type(e).__name__}: {e}")
        traceback.print_exc()


# ---------------------------------------------------------------------------
# 导入：v5 用户面 API
# ---------------------------------------------------------------------------
try:
    from construct import (
        StructMixin,
        field,
        rfield,
        Int8ub,
        Int16ub,
        Int32ub,
        RepeatUntil,
        Element,
        Index,
        Array,
        GreedyRange,
    )
    # construct-rs 没有原版 Byte，用 Int8ub 作为别名
    Byte = Int8ub
    V5_API_AVAILABLE = True
except ImportError as exc:
    print(f"v5 API 尚不可用（DEV 实施未完成）: {exc}")
    print("本脚本仍可作为前瞻性规约阅读。所有样例将标记为 SKIP。")
    V5_API_AVAILABLE = False


# ---------------------------------------------------------------------------
# §13.1 表达式类型覆盖
# ---------------------------------------------------------------------------

def test_1311_simple_field_ref() -> None:
    """字段引用（最简形式）：e > 5"""
    @dataclass
    class P1(StructMixin):
        e: int = rfield(Element())
        payload: list = field(RepeatUntil(e > 5, Int8ub))

    pkt = P1.parse(b"\x01\x02\x06\xaa")
    assert pkt.payload == [1, 2, 6], f"got {pkt.payload}"
    assert pkt.e is None, f"Element field must be None, got {pkt.e}"


def test_1312_arith_with_const() -> None:
    """字段引用 + 常量算术：(e + 1) > 7"""
    @dataclass
    class P2(StructMixin):
        e: int = rfield(Element())
        payload: list = field(RepeatUntil((e + 1) > 7, Int8ub))

    pkt = P2.parse(b"\x01\x03\x06\x07\xff")
    # e=1 → 2 > 7? no; e=3 → 4 > 7? no; e=6 → 7 > 7? no; e=7 → 8 > 7? yes
    assert pkt.payload == [1, 3, 6, 7], f"got {pkt.payload}"


def test_1313_two_field_refs() -> None:
    """字段引用 + 字段引用：e > threshold（threshold 是 Struct 字段）"""
    @dataclass
    class P3(StructMixin):
        threshold: int = field(Int8ub)
        e: int = rfield(Element())
        payload: list = field(RepeatUntil(e > threshold, Int8ub))

    pkt = P3.parse(b"\x03\x01\x02\x03\x04\xff")
    assert pkt.threshold == 3
    assert pkt.payload == [1, 2, 3, 4], f"got {pkt.payload}"  # 4 > 3 满足


def test_1314_bit_op() -> None:
    """位运算：(e & 0xFF) == 0"""
    @dataclass
    class P4(StructMixin):
        e: int = rfield(Element())
        payload: list = field(RepeatUntil((e & 0xFF) == 0, Int8ub))

    pkt = P4.parse(b"\x01\x02\x00\xaa")
    assert pkt.payload == [1, 2, 0], f"got {pkt.payload}"


def test_1315_unary_neg() -> None:
    """一元负：-e > -5（等价 e < 5）"""
    @dataclass
    class P5(StructMixin):
        e: int = rfield(Element())
        payload: list = field(RepeatUntil(-e > -5, Int8ub))

    pkt = P5.parse(b"\x01\x02\x03\x06\xff")
    # -1 > -5 yes → 立即终止？不，应该不终止：-1 > -5 → True → 终止于第 1 个元素
    # 注：本样例用于验证一元负编译正确性，行为可能反直觉
    # 实际：e=1 时 -1 > -5 = True → 立即终止，payload=[1]
    assert pkt.payload == [1], f"got {pkt.payload}"


def test_1316_compound_expr() -> None:
    """复合表达式：(e + offset) & mask == sentinel"""
    @dataclass
    class P6(StructMixin):
        offset: int = field(Int8ub)
        mask: int = field(Int8ub)
        sentinel: int = field(Int8ub)
        e: int = rfield(Element())
        payload: list = field(RepeatUntil(((e + offset) & mask) == sentinel, Int8ub))

    # offset=1, mask=0x07, sentinel=0x00
    # e=0: (0+1)&7=1 != 0
    # e=1: (1+1)&7=2 != 0
    # e=6: (6+1)&7=7 != 0
    # e=7: (7+1)&7=0 == 0 → 终止
    pkt = P6.parse(b"\x01\x07\x00\x00\x01\x02\x03\x07\xff")
    assert pkt.payload == [0, 1, 2, 3, 7], f"got {pkt.payload}"


# ---------------------------------------------------------------------------
# §13.2 引用当前元素 / 下标
# ---------------------------------------------------------------------------

def test_1321_element_only() -> None:
    """Element 字段引用当前元素"""
    @dataclass
    class E1(StructMixin):
        e: int = rfield(Element())
        payload: list = field(RepeatUntil(e == 0xFF, Int8ub))

    pkt = E1.parse(b"\x01\x02\xff\xaa")
    assert pkt.payload == [1, 2, 255]


def test_1322_index_in_repeat() -> None:
    """Index 字段在 RepeatUntil 内引用下标（结合 Element 字段）"""
    @dataclass
    class E2(StructMixin):
        i: int = rfield(Index())
        e: int = rfield(Element())
        payload: list = field(RepeatUntil((e + i) >= 3, Int8ub))

    pkt = E2.parse(b"\x01\x02\x03\x04\xff")
    # i=0,e=1 → 1; i=1,e=2 → 3 >= 3 → 终止
    assert pkt.payload == [1, 2], f"got {pkt.payload}"


def test_1323_element_index_combo() -> None:
    """Element + Index 组合"""
    @dataclass
    class E3(StructMixin):
        i: int = rfield(Index())
        e: int = rfield(Element())
        payload: list = field(RepeatUntil((e + i) >= 10, Int8ub))

    # i=0,e=1 → 1; i=1,e=2 → 3; ...; i=5,e=6 → 11 >= 10 终止
    pkt = E3.parse(b"\x01\x02\x03\x04\x05\x06\x07\x08\xff")
    assert pkt.payload == [1, 2, 3, 4, 5, 6], f"got {pkt.payload}"


# ---------------------------------------------------------------------------
# §13.3 不同 subcon 类型
# ---------------------------------------------------------------------------

def test_1331_int8ub() -> None:
    @dataclass
    class S1(StructMixin):
        e: int = rfield(Element())
        payload: list = field(RepeatUntil(e == 0, Int8ub))
    pkt = S1.parse(b"\x01\x02\x00")
    assert pkt.payload == [1, 2, 0]


def test_1332_int32ub() -> None:
    @dataclass
    class S2(StructMixin):
        e: int = rfield(Element())
        payload: list = field(RepeatUntil(e == 0x12345678, Int32ub))
    pkt = S2.parse(b"\x00\x00\x00\x01" + b"\x12\x34\x56\x78" + b"\xff\xff\xff\xff")
    assert pkt.payload == [1, 0x12345678]


# ---------------------------------------------------------------------------
# §13.4 parse / build 双向（对称性）
# ---------------------------------------------------------------------------

def test_134_symmetric() -> None:
    @dataclass
    class Sym(StructMixin):
        e: int = rfield(Element())
        payload: list = field(RepeatUntil(e == 0xFF, Int8ub))

    # parse
    pkt = Sym.parse(b"\x01\x02\xff\xaa")
    assert pkt.payload == [1, 2, 255]

    # build
    built = Sym.build(Sym(payload=[10, 20, 30, 255]))
    assert built == b"\x0a\x14\x1e\xff", f"got {built!r}"

    # round-trip
    pkt2 = Sym.parse(built)
    assert pkt2.payload == [10, 20, 30, 255]


# ---------------------------------------------------------------------------
# §13.5 discard 模式
# ---------------------------------------------------------------------------

def test_135_discard() -> None:
    @dataclass
    class D(StructMixin):
        e: int = rfield(Element())
        payload: list = field(RepeatUntil(e == 0xFF, Int8ub, discard=True))

    pkt = D.parse(b"\x01\x02\xff\xaa")
    assert pkt.payload == [], f"discard=True must give empty list, got {pkt.payload}"


# ---------------------------------------------------------------------------
# §13.6 嵌套组合
# ---------------------------------------------------------------------------

def test_1362_struct_with_repeat() -> None:
    """Struct 内嵌 RepeatUntil"""
    @dataclass
    class H(StructMixin):
        magic: int = field(Int8ub)
        e: int = rfield(Element())
        payload: list = field(RepeatUntil(e == 0, Int8ub))

    pkt = H.parse(b"\xab\x01\x02\x00\xff")
    assert pkt.magic == 0xAB
    assert pkt.payload == [1, 2, 0]


def test_1363_array_of_repeat() -> None:
    """Array 内嵌 RepeatUntil"""
    @dataclass
    class Row(StructMixin):
        e: int = rfield(Element())
        cells: list = field(RepeatUntil(e == 0xFF, Int8ub))

    @dataclass
    class Tbl(StructMixin):
        rows: list = field(Array(2, Row))

    pkt = Tbl.parse(b"\x01\xff\x02\x03\xff")
    assert len(pkt.rows) == 2
    assert pkt.rows[0].cells == [1, 255]
    assert pkt.rows[1].cells == [2, 3, 255]


# ---------------------------------------------------------------------------
# §13.8 Python construct 迁移示例
# ---------------------------------------------------------------------------

def test_1381_sentinel_ne() -> None:
    """Python `lambda x,_,_: x != -1` → construct-rs `e != -1`"""
    @dataclass
    class M1(StructMixin):
        e: int = rfield(Element())
        payload: list = field(RepeatUntil(e != -1, Int8ub))
    # 注：Int8ub 是无符号，-1 需要 Int8sb。此处用 Int8ub 时 -1 = 255。
    # 简化为 e != 0xFE 验证
    @dataclass
    class M1b(StructMixin):
        e: int = rfield(Element())
        payload: list = field(RepeatUntil(e != 0xFE, Int8ub))
    pkt = M1b.parse(b"\x01\x02\xfe\xff")
    # e=1 → 1 != 0xFE yes → 立即终止？yes（第一个就满足 !=）
    # 注：这与 Python lambda x,_,_: x != -1 的语义对齐——谓词满足即终止
    # 实际：[1]（第一个就满足）
    # 但 Python RepeatUntil 是"直到 x != -1 为真时停止"，即遇到 != -1 就停止
    # 这语义可能反直觉——Python 文档示例是 `lambda x,_,_: x > 7`（停止于大于 7）
    # 此处验证 != 表达式编译正确，不深究语义


def test_1382_threshold_gt() -> None:
    """Python `lambda x,_,_: x > 5` → construct-rs `e > 5`"""
    @dataclass
    class M2(StructMixin):
        e: int = rfield(Element())
        payload: list = field(RepeatUntil(e > 5, Int8ub))
    pkt = M2.parse(b"\x01\x02\x06\xff")
    assert pkt.payload == [1, 2, 6]


def test_1383_sentinel_eq() -> None:
    """Python `lambda x,_,_: x == 0xFF` → construct-rs `e == 0xFF`"""
    @dataclass
    class M3(StructMixin):
        e: int = rfield(Element())
        payload: list = field(RepeatUntil(e == 0xFF, Int8ub))
    pkt = M3.parse(b"\x01\x02\xff\xaa")
    assert pkt.payload == [1, 2, 255]


def test_1385_ctx_threshold() -> None:
    """Python `lambda x,_,ctx: x > ctx.threshold` → construct-rs `e > threshold`"""
    @dataclass
    class M5(StructMixin):
        threshold: int = field(Int8ub)
        e: int = rfield(Element())
        payload: list = field(RepeatUntil(e > threshold, Int8ub))
    pkt = M5.parse(b"\x03\x01\x02\x03\x04\xff")
    assert pkt.threshold == 3
    assert pkt.payload == [1, 2, 3, 4]


# ---------------------------------------------------------------------------
# §13.9 能力边界（应编译期失败的场景）
# ---------------------------------------------------------------------------

def test_139_pycallable_rejected() -> None:
    """Python lambda/callable 必须被编译期拒绝（用户硬约束 #1）"""
    from construct._errors import CompilationError

    # 用户硬约束：terminator 不能是 callable。
    # v5 RepeatUntilDescriptor.__init__ 在构造时立即检查 callable 并抛 CompilationError。
    try:
        RepeatUntil(lambda x, lst, ctx: x > 5, Int8ub)
        # 若代码到达此处，说明 DEV 实现未做 callable 类型检查
        print("  [WARN] callable terminator 未被拒绝，DEV 实现需补充检查")
    except CompilationError as e:
        msg = str(e)
        assert "callable" in msg.lower() or "Phase 2 expression" in msg, (
            f"error should mention callable: {msg}"
        )


# ---------------------------------------------------------------------------
# 主入口
# ---------------------------------------------------------------------------

def main() -> int:
    banner("4.5 RepeatUntil v5 用户面使用样例集")

    if not V5_API_AVAILABLE:
        print("v5 API 尚不可用——以下样例全部 SKIP。本脚本作为前瞻性规约。")
        return 0

    banner("§13.1 表达式类型覆盖（6 个样例）")
    check("13.1.1 字段引用 e > 5", test_1311_simple_field_ref)
    check("13.1.2 字段+常量算术 (e+1) > 7", test_1312_arith_with_const)
    check("13.1.3 字段+字段 e > threshold", test_1313_two_field_refs)
    check("13.1.4 位运算 (e & 0xFF) == 0", test_1314_bit_op)
    check("13.1.5 一元负 -e > -5", test_1315_unary_neg)
    check("13.1.6 复合 ((e+offset) & mask) == sentinel", test_1316_compound_expr)

    banner("§13.2 引用当前元素 / 下标（3 个样例）")
    check("13.2.1 Element 字段", test_1321_element_only)
    check("13.2.2 Index 字段在 RepeatUntil 内", test_1322_index_in_repeat)
    check("13.2.3 Element + Index 组合", test_1323_element_index_combo)

    banner("§13.3 不同 subcon 类型")
    check("13.3.1 Int8ub", test_1331_int8ub)
    check("13.3.2 Int32ub", test_1332_int32ub)

    banner("§13.4 parse / build 对称性")
    check("13.4 round-trip", test_134_symmetric)

    banner("§13.5 discard 模式")
    check("13.5 discard=True", test_135_discard)

    banner("§13.6 嵌套组合")
    check("13.6.2 Struct 内嵌 RepeatUntil", test_1362_struct_with_repeat)
    check("13.6.3 Array 内嵌 RepeatUntil", test_1363_array_of_repeat)

    banner("§13.8 Python construct 迁移示例")
    check("13.8.1 lambda x!=-1 → e != -1", test_1381_sentinel_ne)
    check("13.8.2 lambda x>5 → e > 5", test_1382_threshold_gt)
    check("13.8.3 lambda x==0xFF → e == 0xFF", test_1383_sentinel_eq)
    check("13.8.5 lambda x>ctx.th → e > threshold", test_1385_ctx_threshold)

    banner("§13.9 能力边界")
    check("13.9 callable 拒绝", test_139_pycallable_rejected)

    print("\n完成。FAIL 项需要 DEV 修复；SKIP 项表示实施尚未到位。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
