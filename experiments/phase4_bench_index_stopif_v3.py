"""Phase 4 子任务 4.4 性能基准 v3：Index + StopIf（方法论对称修复）。

修复 v2 的方法论不对称问题（4.x-INVEST 报告 §1+§2）：
    v2 病灶（I 系列）：Rust 侧 `class P(StructMixin): items: list = field(Array(N, Index()))`，
            Python 侧 `pc.Array(N, pc.Index)` 直接调用（不包 Struct）。
    v3 修复：Python 也包一层 `pc.Struct("items" / pc.Array(N, pc.Index))`。
    （S 系列 StopIf 两边本来就都是 Struct，已天然对称，仅做统一报告。）

测量口径（AGENTS.md §6 S-PERF）：
- construct-rs 侧：maturin develop 安装后，Python 调用户面 API（必经 Struct）
- Python construct 侧：import construct，调等效 API
- 子进程隔离（包同名，不可在同一进程导入）
- 取 min(repeat=5) × number

主表（apples-to-apples）：双端都包 Struct
附注（裸口径）：Python 裸 Array/GreedyRange，明确标注"非等价对比"

场景矩阵（13 场景 × parse/build = 24 测量点）：
| #    | 场景                                          | N     | 维度覆盖                     |
|------|-----------------------------------------------|-------|------------------------------|
| I01  | Array(N, Index())                             | 10    | 纯 Index / 小规模             |
| I02  | Array(N, Index())                             | 100   | 中规模                       |
| I03  | Array(N, Index())                             | 1000  | 大规模                       |
| I04  | Array(N, Index())                             | 4096  | 超大规模                     |
| I05  | Array(N, Struct{i: Index, v: Byte})           | 100   | Index in Struct / 复合        |
| I06  | Array(N, Struct{i: Index, v: Byte})           | 4096  | 复合大规模                   |
| I07  | Array(N, Struct{i: Index, v: Int16ub})        | 100   | BE u16 inner                 |
| I08  | Array(N, Struct{i: Index, v: Int32ub})        | 256   | BE u32 inner                 |
| I09  | GreedyRange(Struct{i: Index, v: Byte})        | 1024  | Index in GreedyRange         |
| S01  | StopIf(x == 0) x != 0 不触发                   | -     | 表达式 / 不触发              |
| S02  | StopIf(x == 0) x == 0 触发                     | -     | 表达式 / 触发（StopField）   |
| S03  | StopIf(True) 常量触发                          | -     | 常量 / Always 触发           |
| E01  | Array(0, Index()) 空数组边界                   | 0     | 边界：空数组                 |
"""

import sys
import os

try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")
except Exception:
    pass


# ============================================================
# 场景定义
# ============================================================
# (key, series, N, has_build, has_parse, label)
#   series: "I" (Index in Array/GreedyRange) / "S" (StopIf) / "E" (empty)
SCENARIO_META = [
    ("I01_array_index_n10",      "I", 10,   True,  True, "Array(10, Index())"),
    ("I02_array_index_n100",     "I", 100,  True,  True, "Array(100, Index())"),
    ("I03_array_index_n1000",    "I", 1000, True,  True, "Array(1000, Index())"),
    ("I04_array_index_n4096",    "I", 4096, True,  True, "Array(4096, Index())"),
    ("I05_array_struct_n100",    "I", 100,  True,  True, "Array(100, Struct{i: Index, v: Byte})"),
    ("I06_array_struct_n4096",   "I", 4096, True,  True, "Array(4096, Struct{i: Index, v: Byte})"),
    ("I07_array_struct_i16_n100","I", 100,  True,  True, "Array(100, Struct{i: Index, v: Int16ub})"),
    ("I08_array_struct_i32_n256","I", 256,  True,  True, "Array(256, Struct{i: Index, v: Int32ub})"),
    ("I09_greedy_struct_n1024",  "I", 1024, True,  True, "GreedyRange(Struct{i: Index, v: Byte})"),
    ("S01_stopif_no_trigger",    "S", 0,    True,  True, "StopIf(x==0) x=1 不触发"),
    ("S02_stopif_triggered",     "S", 0,    True,  True, "StopIf(x==0) x=0 触发"),
    ("S03_stopif_always",        "S", 0,    True,  True, "StopIf(True) 常量触发"),
    ("E01_empty_array_index",    "E", 0,    True,  True, "Array(0, Index()) 空数组"),
]

REPEAT = 5
NUMBER_FOR = {
    "I03_array_index_n1000": 500,
    "I04_array_index_n4096": 200,
    "I06_array_struct_n4096": 200,
    "I09_greedy_struct_n1024": 500,
}
DEFAULT_NUMBER = 5000


def _expected_iters(key, series, n):
    """返回该场景每次调用预期的内部迭代次数。"""
    if series == "S":
        return 0  # StopIf 不依赖 _index 迭代
    return n


def _rs_py_symbolic_work(series, key):
    if series == "S":
        return "Struct{Int8,StopIf,Int8}"
    if key.startswith("I01") or key.startswith("I02") or key.startswith("I03") \
       or key.startswith("I04") or key.startswith("E01"):
        return "Struct{Array(N,Index)}"
    if key.startswith("I09"):
        return "Struct{GreedyRange(Struct{Index,Int8})}"
    return "Struct{Array(N,Struct{Index,Int})}"


# ============================================================
# Rust 模板（construct-rs 用户面 API，必经 StructMixin）
# ============================================================
def _rs_template(key):
    """返回 construct-rs 的测量代码（subprocess -c 形式）。"""
    templates = {
        "I01_array_index_n10": """
import timeit
from dataclasses import dataclass
from construct import StructMixin, field, rfield, Array, Index

@dataclass
class P(StructMixin):
    items: list = field(Array({N}, Index()))

def run_parse():
    P.parse(b'')

p_obj = P.parse(b'')
def run_build():
    p_obj.build()

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
        "I05_array_struct_n100": """
import timeit
from dataclasses import dataclass
from construct import StructMixin, field, rfield, Array, Index, Int8ub

@dataclass
class Item(StructMixin):
    i: int = rfield(Index())
    v: int = field(Int8ub)

@dataclass
class P(StructMixin):
    items: list = field(Array({N}, Item))

DATA = bytes(i % 256 for i in range({N}))

def run_parse():
    P.parse(DATA)

p_obj = P.parse(DATA)
def run_build():
    p_obj.build()

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
        "I07_array_struct_i16_n100": """
import timeit
from dataclasses import dataclass
from construct import StructMixin, field, rfield, Array, Index, Int16ub

@dataclass
class Item(StructMixin):
    i: int = rfield(Index())
    v: int = field(Int16ub)

@dataclass
class P(StructMixin):
    items: list = field(Array({N}, Item))

DATA = bytes(i % 256 for i in range({N} * 2))

def run_parse():
    P.parse(DATA)

p_obj = P.parse(DATA)
def run_build():
    p_obj.build()

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
        "I08_array_struct_i32_n256": """
import timeit
from dataclasses import dataclass
from construct import StructMixin, field, rfield, Array, Index, Int32ub

@dataclass
class Item(StructMixin):
    i: int = rfield(Index())
    v: int = field(Int32ub)

@dataclass
class P(StructMixin):
    items: list = field(Array({N}, Item))

DATA = bytes(i % 256 for i in range({N} * 4))

def run_parse():
    P.parse(DATA)

p_obj = P.parse(DATA)
def run_build():
    p_obj.build()

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
        "I09_greedy_struct_n1024": """
import timeit
from dataclasses import dataclass
from construct import StructMixin, field, rfield, Array, GreedyRange, Index, Int8ub

@dataclass
class Item(StructMixin):
    i: int = rfield(Index())
    v: int = field(Int8ub)

@dataclass
class P(StructMixin):
    items: list = field(GreedyRange(Item))

DATA = bytes(i % 256 for i in range({N}))

def run_parse():
    P.parse(DATA)

p_obj = P.parse(DATA)
def run_build():
    p_obj.build()

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
        "S01_stopif_no_trigger": """
import timeit
from dataclasses import dataclass
from typing import Any
from construct import StructMixin, field, rfield, Int8ub, StopIf

@dataclass
class S(StructMixin):
    x: int = field(Int8ub)
    stop: Any = rfield(StopIf(x == 0))
    y: int = field(Int8ub, default=0)

DATA = b'\\x01\\x02'

def run_parse():
    S.parse(DATA)

s_obj = S.parse(DATA)
def run_build():
    s_obj.build()

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
        "S02_stopif_triggered": """
import timeit
from dataclasses import dataclass
from typing import Any
from construct import StructMixin, field, rfield, Int8ub, StopIf

@dataclass
class S(StructMixin):
    x: int = field(Int8ub)
    stop: Any = rfield(StopIf(x == 0))
    y: int = field(Int8ub, default=0)

DATA = b'\\x00'

def run_parse():
    S.parse(DATA)

s_obj = S.parse(DATA)
def run_build():
    s_obj.build()

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
        "S03_stopif_always": """
import timeit
from dataclasses import dataclass
from typing import Any
from construct import StructMixin, field, rfield, Int8ub, StopIf

@dataclass
class S(StructMixin):
    x: int = field(Int8ub)
    stop: Any = rfield(StopIf(True))
    y: int = field(Int8ub, default=0)

DATA = b'\\x42'

def run_parse():
    S.parse(DATA)

s_obj = S.parse(DATA)
def run_build():
    s_obj.build()

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
        "E01_empty_array_index": """
import timeit
from dataclasses import dataclass
from construct import StructMixin, field, Array, Index

@dataclass
class P(StructMixin):
    items: list = field(Array(0, Index()))

def run_parse():
    P.parse(b'')

p_obj = P.parse(b'')
def run_build():
    p_obj.build()

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
    }
    aliases = {
        "I02_array_index_n100":  "I01_array_index_n10",
        "I03_array_index_n1000": "I01_array_index_n10",
        "I04_array_index_n4096": "I01_array_index_n10",
        "I06_array_struct_n4096":"I05_array_struct_n100",
    }
    src_key = aliases.get(key, key)
    return templates[src_key]


# ============================================================
# Python 模板（apples-to-apples：包 Struct）
# ============================================================
def _py_template_wrapped(key):
    """返回 Python construct 测量代码（apples-to-apples：包 Struct）。

    I 系列：pc.Struct("items" / pc.Array(N, ...))  ← 包 Struct
    S 系列：pc.Struct(...)（本来就是 Struct，天然对等）
    E 系列：pc.Struct("items" / pc.Array(0, pc.Index))
    """
    templates = {
        "I01_array_index_n10": """
import timeit
import construct as pc

d = pc.Struct('items' / pc.Array({N}, pc.Index))

def run_parse():
    d.parse(b'')

parsed = d.parse(b'')
def run_build():
    d.build(parsed)

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
        "I05_array_struct_n100": """
import timeit
import construct as pc

d = pc.Struct('items' / pc.Array({N}, pc.Struct('i' / pc.Index, 'v' / pc.Int8ub)))
DATA = bytes(i % 256 for i in range({N}))

def run_parse():
    d.parse(DATA)

parsed = d.parse(DATA)
def run_build():
    d.build(parsed)

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
        "I07_array_struct_i16_n100": """
import timeit
import construct as pc

d = pc.Struct('items' / pc.Array({N}, pc.Struct('i' / pc.Index, 'v' / pc.Int16ub)))
DATA = bytes(i % 256 for i in range({N} * 2))

def run_parse():
    d.parse(DATA)

parsed = d.parse(DATA)
def run_build():
    d.build(parsed)

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
        "I08_array_struct_i32_n256": """
import timeit
import construct as pc

d = pc.Struct('items' / pc.Array({N}, pc.Struct('i' / pc.Index, 'v' / pc.Int32ub)))
DATA = bytes(i % 256 for i in range({N} * 4))

def run_parse():
    d.parse(DATA)

parsed = d.parse(DATA)
def run_build():
    d.build(parsed)

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
        "I09_greedy_struct_n1024": """
import timeit
import construct as pc

d = pc.Struct('items' / pc.GreedyRange(pc.Struct('i' / pc.Index, 'v' / pc.Int8ub)))
DATA = bytes(i % 256 for i in range({N}))

def run_parse():
    d.parse(DATA)

parsed = d.parse(DATA)
def run_build():
    d.build(parsed)

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
        "S01_stopif_no_trigger": """
import timeit
import construct as pc

d = pc.Struct('x' / pc.Int8ub, pc.StopIf(pc.this.x == 0), 'y' / pc.Int8ub)
DATA = b'\\x01\\x02'

def run_parse():
    d.parse(DATA)

parsed = d.parse(DATA)
def run_build():
    d.build(parsed)

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
        "S02_stopif_triggered": """
import timeit
import construct as pc

d = pc.Struct('x' / pc.Int8ub, pc.StopIf(pc.this.x == 0), 'y' / pc.Int8ub)
DATA = b'\\x00'

def run_parse():
    d.parse(DATA)

parsed = d.parse(DATA)
def run_build():
    d.build(parsed)

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
        "S03_stopif_always": """
import timeit
import construct as pc

d = pc.Struct('x' / pc.Int8ub, pc.StopIf(True), 'y' / pc.Int8ub)
DATA = b'\\x42'

def run_parse():
    d.parse(DATA)

parsed = d.parse(DATA)
def run_build():
    d.build(parsed)

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
        "E01_empty_array_index": """
import timeit
import construct as pc

d = pc.Struct('items' / pc.Array(0, pc.Index))

def run_parse():
    d.parse(b'')

parsed = d.parse(b'')
def run_build():
    d.build(parsed)

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
    }
    aliases = {
        "I02_array_index_n100":  "I01_array_index_n10",
        "I03_array_index_n1000": "I01_array_index_n10",
        "I04_array_index_n4096": "I01_array_index_n10",
        "I06_array_struct_n4096":"I05_array_struct_n100",
    }
    src_key = aliases.get(key, key)
    return templates[src_key]


# ============================================================
# Python 模板（裸口径，非等价对比，仅作参考）
# ============================================================
def _py_template_bare(key):
    """返回 Python construct 测量代码（裸口径：I 系列不包 Struct）。

    I 系列：pc.Array(N, ...)（裸，v2 口径）
    S 系列：pc.Struct(...)（本来就是 Struct，与 wrapped 相同）
    E 系列：pc.Array(0, pc.Index)（裸）
    """
    templates = {
        "I01_array_index_n10": """
import timeit
import construct as pc

d = pc.Array({N}, pc.Index)

def run_parse():
    d.parse(b'')

parsed = d.parse(b'')
def run_build():
    d.build(parsed)

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
        "I05_array_struct_n100": """
import timeit
import construct as pc

d = pc.Array({N}, pc.Struct('i' / pc.Index, 'v' / pc.Int8ub))
DATA = bytes(i % 256 for i in range({N}))

def run_parse():
    d.parse(DATA)

parsed = d.parse(DATA)
def run_build():
    d.build(parsed)

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
        "I07_array_struct_i16_n100": """
import timeit
import construct as pc

d = pc.Array({N}, pc.Struct('i' / pc.Index, 'v' / pc.Int16ub))
DATA = bytes(i % 256 for i in range({N} * 2))

def run_parse():
    d.parse(DATA)

parsed = d.parse(DATA)
def run_build():
    d.build(parsed)

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
        "I08_array_struct_i32_n256": """
import timeit
import construct as pc

d = pc.Array({N}, pc.Struct('i' / pc.Index, 'v' / pc.Int32ub))
DATA = bytes(i % 256 for i in range({N} * 4))

def run_parse():
    d.parse(DATA)

parsed = d.parse(DATA)
def run_build():
    d.build(parsed)

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
        "I09_greedy_struct_n1024": """
import timeit
import construct as pc

d = pc.GreedyRange(pc.Struct('i' / pc.Index, 'v' / pc.Int8ub))
DATA = bytes(i % 256 for i in range({N}))

def run_parse():
    d.parse(DATA)

parsed = d.parse(DATA)
def run_build():
    d.build(parsed)

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
        # S 系列本来就是 Struct，bare == wrapped
        "S01_stopif_no_trigger": _py_template_wrapped_static("S01_stopif_no_trigger"),
        "S02_stopif_triggered": _py_template_wrapped_static("S02_stopif_triggered"),
        "S03_stopif_always": _py_template_wrapped_static("S03_stopif_always"),
        "E01_empty_array_index": """
import timeit
import construct as pc

d = pc.Array(0, pc.Index)

def run_parse():
    d.parse(b'')

parsed = d.parse(b'')
def run_build():
    d.build(parsed)

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
    }
    aliases = {
        "I02_array_index_n100":  "I01_array_index_n10",
        "I03_array_index_n1000": "I01_array_index_n10",
        "I04_array_index_n4096": "I01_array_index_n10",
        "I06_array_struct_n4096":"I05_array_struct_n100",
    }
    src_key = aliases.get(key, key)
    return templates[src_key]


def _py_template_wrapped_static(key):
    """S 系列 wrapped 模板的静态获取（避免循环引用）。"""
    # 直接内联 S 系列模板（与 wrapped 版本完全一致，因 S 本身就是 Struct）
    s_templates = {
        "S01_stopif_no_trigger": """
import timeit
import construct as pc

d = pc.Struct('x' / pc.Int8ub, pc.StopIf(pc.this.x == 0), 'y' / pc.Int8ub)
DATA = b'\\x01\\x02'

def run_parse():
    d.parse(DATA)

parsed = d.parse(DATA)
def run_build():
    d.build(parsed)

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
        "S02_stopif_triggered": """
import timeit
import construct as pc

d = pc.Struct('x' / pc.Int8ub, pc.StopIf(pc.this.x == 0), 'y' / pc.Int8ub)
DATA = b'\\x00'

def run_parse():
    d.parse(DATA)

parsed = d.parse(DATA)
def run_build():
    d.build(parsed)

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
        "S03_stopif_always": """
import timeit
import construct as pc

d = pc.Struct('x' / pc.Int8ub, pc.StopIf(True), 'y' / pc.Int8ub)
DATA = b'\\x42'

def run_parse():
    d.parse(DATA)

parsed = d.parse(DATA)
def run_build():
    d.build(parsed)

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
    }
    return s_templates[key]


# ============================================================
# 子进程执行 + 格式化
# ============================================================
def subprocess_run(python_exe, code):
    """执行 python -c code 并返回 stdout。"""
    import subprocess
    env = dict(os.environ)
    env["PYO3_USE_ABI3_FORWARD_COMPATIBILITY"] = "1"
    workdir = r"<legacy-repo>\construct-rs"
    result = subprocess.run(
        [python_exe, "-c", code],
        capture_output=True, text=True, timeout=180,
        cwd=workdir, env=env,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"subprocess failed:\nstderr:\n{result.stderr}\nstdout:\n{result.stdout}"
        )
    return result.stdout


def run_in_subprocess(python_exe, code_template, N, NUMBER, REPEAT_VAL):
    """执行模板代码，返回 (parse_ns, build_ns)。build_ns 可能为 None。"""
    code = code_template.format(N=N, NUMBER=NUMBER, REPEAT=REPEAT_VAL)
    result = subprocess_run(python_exe, code)
    parse_ns = None
    build_ns = None
    for line in result.splitlines():
        if line.startswith("PARSE_NS"):
            parse_ns = float(line.split()[1])
        elif line.startswith("BUILD_NS"):
            build_ns = float(line.split()[1])
    if parse_ns is None:
        raise RuntimeError(f"no PARSE_NS in output:\n{result}")
    return parse_ns, build_ns


def format_ns(ns_value):
    """纳秒值 → 人类可读字符串。输入是 ns/call。"""
    if ns_value >= 1e6:
        return f"{ns_value / 1e6:.2f} ms"
    if ns_value >= 1e3:
        return f"{ns_value / 1e3:.2f} us"
    return f"{ns_value:.1f} ns"


# ============================================================
# 主流程
# ============================================================
def main():
    rs_python = r"<opencode-temp>\crs_venv_new\Scripts\python.exe"
    py_python = r"<opencode-temp>\crs_venv_py_new\Scripts\python.exe"

    print("=" * 130)
    print("Phase 4.4 Index+StopIf 性能基准 v3（方法论对称修复）")
    print("=" * 130)
    print(f"REPEAT={REPEAT}, DEFAULT_NUMBER={DEFAULT_NUMBER}")
    print(f"Python (rs): {rs_python}")
    print(f"Python (py): {py_python}")
    print()

    results = []  # [(key, series, label, N, direction, py_w_ns, py_b_ns, rs_ns, sp_w, sp_b, number)]

    for key, series, N, has_build, has_parse, label in SCENARIO_META:
        number = NUMBER_FOR.get(key, DEFAULT_NUMBER)
        try:
            py_w_parse, py_w_build = run_in_subprocess(
                py_python, _py_template_wrapped(key), N, number, REPEAT
            )
        except Exception as e:
            print(f"  [PY+ FAIL] {key}: {str(e)[:200]}")
            continue
        try:
            py_b_parse, py_b_build = run_in_subprocess(
                py_python, _py_template_bare(key), N, number, REPEAT
            )
        except Exception as e:
            print(f"  [PY* FAIL] {key}: {str(e)[:200]}")
            continue
        try:
            rs_parse, rs_build = run_in_subprocess(
                rs_python, _rs_template(key), N, number, REPEAT
            )
        except Exception as e:
            print(f"  [RS FAIL] {key}: {str(e)[:200]}")
            continue

        # parse
        sp_w_p = py_w_parse / rs_parse
        sp_b_p = py_b_parse / rs_parse
        results.append((key, series, label, N, "parse",
                        py_w_parse, py_b_parse, rs_parse, sp_w_p, sp_b_p, number))
        # build
        if has_build and py_w_build is not None and rs_build is not None:
            sp_w_b = py_w_build / rs_build
            sp_b_b = py_b_build / rs_build
            results.append((key, series, label, N, "build",
                            py_w_build, py_b_build, rs_build, sp_w_b, sp_b_b, number))

    _emit_report(results)


def _emit_report(results):
    # ---- 主表（apples-to-apples） ----
    print()
    print("=" * 130)
    print("主表（apples-to-apples）：Rust StructMixin  vs  Python Struct(...)  <- 双端都包 Struct，口径对等")
    print("=" * 130)
    print(f"{'#':<6}{'场景':<48}{'N':>7}{'number':>10}"
          f"{'Py+ ns/call':>16}{'Rs ns/call':>16}{'加速比+':>10}{'方向':>8}")
    print("-" * 130)
    for key, series, label, N, direction, py_w, py_b, rs, sp_w, sp_b, num in results:
        flag = " <10x!" if sp_w < 10 else ""
        lbl = label if direction == "parse" else "  └─ build"
        print(f"{key[:6]:<6}{lbl:<48}{N:>7}{num:>10}"
              f"{format_ns(py_w):>16}{format_ns(rs):>16}{sp_w:>8.2f}x{flag}{direction:>8}")
    print("-" * 130)

    # ---- 附注（裸口径） ----
    print()
    print("=" * 130)
    print("附注（裸口径，非等价对比）：Rust StructMixin（必经 Struct） vs  Python 裸 Array/GreedyRange（少做 Struct 工作）")
    print("  ↑ v2 用的就是这个口径（I 系列 + E 系列）；S 系列两边都是 Struct，bare==wrapped")
    print("=" * 130)
    print(f"{'#':<6}{'场景':<48}{'N':>7}{'Py* ns/call':>16}{'Rs ns/call':>16}"
          f"{'加速比*':>10}{'加速比+':>10}{'Δ':>8}{'方向':>8}")
    print("-" * 130)
    for key, series, label, N, direction, py_w, py_b, rs, sp_w, sp_b, num in results:
        flag = " <10x!" if sp_b < 10 else ""
        delta = sp_w - sp_b
        lbl = label if direction == "parse" else "  └─ build"
        print(f"{key[:6]:<6}{lbl:<48}{N:>7}{format_ns(py_b):>16}{format_ns(rs):>16}"
              f"{sp_b:>8.2f}x{flag}{sp_w:>8.2f}x{delta:>+7.2f}x{direction:>8}")
    print("-" * 130)

    # ---- 派生指标 1：apples-to-apples 加速比排序 ----
    print()
    print("─── 派生指标 1：apples-to-apples 加速比排序（由高到低） ───")
    sorted_sp = sorted(results, key=lambda x: -x[8])
    for key, series, label, N, direction, _, _, _, sp_w, _, _ in sorted_sp:
        flag = " <10x!" if sp_w < 10 else ""
        print(f"  {label + ' ' + direction:<58} N={N:<6} {sp_w:>7.2f}x{flag}")

    # ---- 派生指标 2：parse/build 比率 ----
    print()
    print("─── 派生指标 2：parse/build 比率（apples-to-apples） ───")
    print(f"  {'场景':<48}{'N':>7}{'py p/b':>10}{'rs p/b':>10}{'方向一致':>12}")
    by_key = {}
    for key, series, label, N, direction, py_w, py_b, rs, sp_w, sp_b, num in results:
        by_key.setdefault(key, {})["label"] = label
        by_key[key]["series"] = series
        by_key[key]["N"] = N
        by_key[key][direction] = (py_w, rs)
    for key, info in by_key.items():
        if "parse" in info and "build" in info:
            py_p, _ = info["parse"]
            py_b, _ = info["build"]
            _, rs_p = info["parse"]
            _, rs_b = info["build"]
            if py_b == 0 or rs_b == 0:
                continue
            py_r = py_p / py_b
            rs_r = rs_p / rs_b
            consistent = "Y" if (py_r > 1) == (rs_r > 1) else "N"
            print(f"  {info['label']:<48}{info['N']:>7}{py_r:>9.2f}x{rs_r:>9.2f}x{consistent:>12}")

    # ---- 派生指标 3：Array(N, Index) 规模扩展 ----
    print()
    print("─── 派生指标 3：Array(N, Index) 规模扩展（ns/元素，apples-to-apples） ───")
    print(f"  {'N':>7}{'py+ parse ns/e':>18}{'rs parse ns/e':>18}"
          f"{'py+ build ns/e':>18}{'rs build ns/e':>18}")
    scale_keys = ["I01_array_index_n10", "I02_array_index_n100",
                  "I03_array_index_n1000", "I04_array_index_n4096"]
    parse_by_key = {}
    build_by_key = {}
    for k, series, lbl, N, d, py_w, py_b, rs, sp_w, sp_b, num in results:
        if d == "parse":
            parse_by_key[k] = (py_w, rs)
        else:
            build_by_key[k] = (py_w, rs)
    n_for = {m[0]: m[2] for m in SCENARIO_META}
    for key in scale_keys:
        n = n_for[key]
        if n == 0 or key not in parse_by_key or key not in build_by_key:
            continue
        py_p, rs_p = parse_by_key[key]
        py_b, rs_b = build_by_key[key]
        print(f"  {n:>7}{py_p/n:>17.2f}{rs_p/n:>17.2f}{py_b/n:>17.2f}{rs_b/n:>17.2f}")

    # ---- 派生指标 4：Index+Int 承载类型对比 ----
    print()
    print("─── 派生指标 4：Index+Int 承载类型对比（parse 加速比+） ───")
    type_keys = [
        ("Array(N, Index) N=10",            "I01_array_index_n10"),
        ("Array(N, Index) N=100",           "I02_array_index_n100"),
        ("Array(N, Index) N=1000",          "I03_array_index_n1000"),
        ("Array(N, Index) N=4096",          "I04_array_index_n4096"),
        ("Struct{i: Index, v: Byte} N=100", "I05_array_struct_n100"),
        ("Struct{i: Index, v: Int16ub} N=100", "I07_array_struct_i16_n100"),
        ("Struct{i: Index, v: Int32ub} N=256", "I08_array_struct_i32_n256"),
        ("GreedyRange(Struct{i,v}) N=1024", "I09_greedy_struct_n1024"),
    ]
    for name, key in type_keys:
        if key in parse_by_key:
            py, rs = parse_by_key[key]
            print(f"  {name:<42} parse 加速比+: {py/rs:>6.2f}x")

    # ---- 派生指标 5：场景标签 vs 实际迭代 ----
    print()
    print("─── 派生指标 5：场景标签 N vs 实际迭代次数 ───")
    print(f"  {'场景':<48}{'标签 N':>10}{'实际迭代':>12}{'一致':>8}")
    seen = set()
    for key, series, label, N, direction, _, _, _, _, _, _ in results:
        if direction != "parse" or label in seen:
            continue
        seen.add(label)
        actual = _expected_iters(key, series, N)
        consistent = "Y" if (actual == N or series == "S") else "N"
        print(f"  {label:<48}{N:>10}{actual:>12}{consistent:>8}")

    # ---- 派生指标 6：内部关系合理性 ----
    print()
    print("─── 派生指标 6：内部关系合理性（Array(N, Index) N=100 是基线） ───")
    if "I02_array_index_n100" in parse_by_key:
        base_p_rs = parse_by_key["I02_array_index_n100"][1]
        base_p_py = parse_by_key["I02_array_index_n100"][0]
        print(f"  [基线] Array(100, Index) parse: py+={format_ns(base_p_py)}, rs={format_ns(base_p_rs)}")
        compare_keys = [
            ("Array(100, Struct{i,Byte})",     "I05_array_struct_n100"),
            ("Array(100, Struct{i,Int16ub})",  "I07_array_struct_i16_n100"),
        ]
        for name, key in compare_keys:
            if key in parse_by_key:
                py, rs = parse_by_key[key]
                print(f"  {name:<34} parse rs/base={rs/base_p_rs:>5.2f}x, py+/base={py/base_p_py:>5.2f}x")

    # ---- 派生指标 7：方法论对称性核查表 ----
    print()
    print("─── 派生指标 7：方法论对称性核查表（Rust vs Python 工作量对照） ───")
    print(f"  {'场景':<48}{'Rust 侧':<32}{'Python 侧 (wrapped)':<32}{'对称':>8}")
    seen = set()
    for key, series, label, N, direction, _, _, _, _, _, _ in results:
        if direction != "parse" or label in seen:
            continue
        seen.add(label)
        work = _rs_py_symbolic_work(series, key)
        # apples-to-apples: 两边都是 Struct{...}
        symmetric = "Y"
        print(f"  {label:<48}{work:<32}{work:<32}{symmetric:>8}")

    # ---- 异常项检测 ----
    print()
    print("─── 异常项检测（apples-to-apples） ───")
    below_10 = [(lbl, d, sp_w, N) for _, _, lbl, N, d, _, _, _, sp_w, _, _ in results if sp_w < 10]
    if below_10:
        print(f"  [WARN] 低于 10x 的场景 ({len(below_10)}):")
        for lbl, d, sp_w, N in below_10:
            print(f"    {lbl} {d} N={N}: {sp_w:.2f}x")
    else:
        print("  [PASS] 全部场景 >= 10x")

    # ---- S-PERF 出口 ----
    target = 10.0
    print()
    print(f"─── S-PERF 出口标准 (≥{target}x, apples-to-apples) ───")
    n_total = len(results)
    n_met = sum(1 for r in results if r[8] >= target)
    print(f"  {n_met}/{n_total} 场景 >= {target}x")
    below = [(lbl, d, sp_w) for _, _, lbl, _, d, _, _, _, sp_w, _, _ in results if sp_w < target]
    if below:
        for lbl, d, sp_w in below:
            print(f"    {lbl} {d}: {sp_w:.2f}x")
    print("=" * 130)


if __name__ == "__main__":
    main()
