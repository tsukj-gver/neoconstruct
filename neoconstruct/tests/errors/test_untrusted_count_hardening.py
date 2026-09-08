"""不可信 count 字段加固：子进程崩溃隔离矩阵。

背景：按 count 预分配结果 Vec 的朴素实现面对不可信输入（恶意/损坏的
32-bit 长度字段置为 0xFFFFFFFF）会触发 34GB 级分配失败，进程直接
abort（rc=0xC0000409 族），Python 层无法 catch。

加固后预分配按流剩余长度封顶，count 超出实际可解析数量时由逐元素
parse 返回可 catch 的 ConstructError 族（StreamError / RangeError）。

测试架构：每个敌意用例在隔离子进程中运行——若加固回归（进程 abort），
子进程以非零码退出且不输出 CATCHABLE，主 pytest 进程存活并给出明确的
失败信号，而非整套测试崩溃。
"""

import subprocess

import pytest

from dataclasses import dataclass

from neoconstruct import (
    Array,
    ConstructError,
    Int8ub,
    PrefixedArray,
    StreamError,
    StructMixin,
    field,
)

# 子进程脚本模板：注入 neoconstruct python 目录到 sys.path 最前部，
# 执行敌意用例体；可 catch 的 ConstructError → 输出 CATCHABLE 正常退出。
_CHILD_TEMPLATE = """\
import sys

NEOCONSTRUCT_PYTHON_DIR = {neoconstruct_python_dir!r}
sys.path[:] = [p for p in sys.path if p != NEOCONSTRUCT_PYTHON_DIR]
sys.path.insert(0, NEOCONSTRUCT_PYTHON_DIR)

from dataclasses import dataclass
from neoconstruct import (
    Array, Bytes, ConstructError, GreedyBytes, Int8ub, Int16ub, Int32ub,
    Int32sl, PrefixedArray, StructMixin, field,
)

{body}
"""


def _run_isolated(rs_python, neoconstruct_python_dir, body):
    """在隔离子进程中执行敌意用例体，返回 CompletedProcess。"""
    script = _CHILD_TEMPLATE.format(
        neoconstruct_python_dir=neoconstruct_python_dir, body=body
    )
    return subprocess.run(
        [rs_python, "-c", script],
        capture_output=True,
        text=True,
        timeout=120,
    )


def _assert_catchable(proc, case_desc):
    """断言子进程正常退出且 ConstructError 被 catch（进程未死亡）。"""
    assert proc.returncode == 0, (
        f"{case_desc}: 子进程异常退出 rc={proc.returncode} "
        f"(stderr 尾部: {proc.stderr[-300:]!r})——预期可 catch 的 "
        "ConstructError，而非进程级 abort"
    )
    assert "CATCHABLE" in proc.stdout, (
        f"{case_desc}: 子进程未报告 catch（stdout={proc.stdout!r}, "
        f"stderr 尾部={proc.stderr[-300:]!r})"
    )


# ---------------------------------------------------------------------------
# 敌意矩阵：每个用例一个隔离子进程
# ---------------------------------------------------------------------------


def test_array_huge_literal_count_catchable(rs_python, crs_python_dir):
    """字面常量巨大 count（无任何不可信输入）→ StreamError，进程存活。"""
    body = """\
try:
    @dataclass
    class P(StructMixin):
        items: list = field(Array(4294967295, Int8ub))
    P.parse(b"\\x01")
except ConstructError:
    print("CATCHABLE")
"""
    proc = _run_isolated(rs_python, crs_python_dir, body)
    _assert_catchable(proc, "Array(0xFFFFFFFF, Int8ub) 字面常量")


def test_array_huge_count_from_stream_catchable(rs_python, crs_python_dir):
    """流内 count 字段为 0xFFFFFFFF → StreamError，进程存活。"""
    body = """\
try:
    @dataclass
    class P(StructMixin):
        count: int = field(Int32ub)
        items: list = field(Array(count, Int16ub))
    P.parse(b"\\xff\\xff\\xff\\xff")
except ConstructError:
    print("CATCHABLE")
"""
    proc = _run_isolated(rs_python, crs_python_dir, body)
    _assert_catchable(proc, "Array(count=0xFFFFFFFF) 流内 count")


def test_prefixed_array_huge_count_catchable(rs_python, crs_python_dir):
    """PrefixedArray 前缀为 0xFFFFFFFF → StreamError，进程存活。"""
    body = """\
try:
    @dataclass
    class P(StructMixin):
        items: list = field(PrefixedArray(Int32ub, Int8ub))
    P.parse(b"\\xff\\xff\\xff\\xff")
except ConstructError:
    print("CATCHABLE")
"""
    proc = _run_isolated(rs_python, crs_python_dir, body)
    _assert_catchable(proc, "PrefixedArray(Int32ub, Int8ub) 大 count")


def test_negative_count_catchable(rs_python, crs_python_dir):
    """负 count（有符号前缀 -1）→ RangeError，进程存活。[对照：本就安全]"""
    body = """\
try:
    @dataclass
    class P(StructMixin):
        items: list = field(PrefixedArray(Int32sl, Int8ub))
    P.parse(b"\\xff\\xff\\xff\\xff")
except ConstructError:
    print("CATCHABLE")
"""
    proc = _run_isolated(rs_python, crs_python_dir, body)
    _assert_catchable(proc, "PrefixedArray(Int32sl) 负 count")


def test_huge_const_bytes_length_catchable(rs_python, crs_python_dir):
    """Bytes 巨大常量长度 → StreamError，进程存活。[对照：本就安全]"""
    body = """\
try:
    @dataclass
    class P(StructMixin):
        data: bytes = field(Bytes(4294967295))
    P.parse(b"\\x01\\x02")
except ConstructError:
    print("CATCHABLE")
"""
    proc = _run_isolated(rs_python, crs_python_dir, body)
    _assert_catchable(proc, "Bytes(0xFFFFFFFF) 大长度")


def test_greedy_bytes_normal_parse_survives(rs_python, crs_python_dir):
    """GreedyBytes 常规解析不受加固影响。[对照]"""
    body = """\
@dataclass
class P(StructMixin):
    rest: bytes = field(GreedyBytes)
parsed = P.parse(b"\\x01\\x02\\x03")
assert parsed.rest == b"\\x01\\x02\\x03"
print("CATCHABLE")
"""
    proc = _run_isolated(rs_python, crs_python_dir, body)
    _assert_catchable(proc, "GreedyBytes 常规解析")


# ---------------------------------------------------------------------------
# 进程内行为：加固不改变合法路径语义
# ---------------------------------------------------------------------------


def test_array_normal_parse_still_works():
    """常规 Array（count 与数据匹配）解析结果不变。"""

    @dataclass
    class P(StructMixin):
        items: list = field(Array(3, Int8ub))

    parsed = P.parse(b"\x01\x02\x03")
    assert parsed.items == [1, 2, 3]


def test_array_large_count_with_full_data_still_works():
    """大 count 且数据充足（1000 元素）不受封顶影响。"""

    @dataclass
    class P(StructMixin):
        items: list = field(Array(1000, Int8ub))

    data = bytes(i % 256 for i in range(1000))
    parsed = P.parse(data)
    assert len(parsed.items) == 1000
    assert parsed.items[999] == 999 % 256
    assert P(items=parsed.items).build() == data


def test_array_count_exceeding_data_raises_stream_error_in_process():
    """count 超出数据量（进程内）→ StreamError 且 path 定位到元素级。"""

    @dataclass
    class P(StructMixin):
        items: list = field(Array(5, Int8ub))

    with pytest.raises(StreamError) as exc_info:
        P.parse(b"\x01\x02")
    assert exc_info.value.path == "root.items[2]"


def test_prefixed_array_normal_roundtrip_still_works():
    """常规 PrefixedArray parse/build 往返不变。"""

    @dataclass
    class P(StructMixin):
        items: list = field(PrefixedArray(Int8ub, Int8ub))

    parsed = P.parse(b"\x03\x0a\x0b\x0c")
    assert parsed.items == [10, 11, 12]
    assert P(items=[10, 11, 12]).build() == b"\x03\x0a\x0b\x0c"


def test_construct_error_family_importable():
    """ConstructError 族异常可导入（catch 面 sanity）。"""
    assert issubclass(StreamError, ConstructError)
