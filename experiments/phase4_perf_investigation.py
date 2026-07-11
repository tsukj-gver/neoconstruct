"""Phase 4 性能塌方根因调查：数据复测 + 同口径对照。

调查 17 个不达 10x 场景的根因。三个核心假设需要验证：

H1（benchmark 测错 - 结构不对称）：
    Phase 4 所有 benchmark 中，Rust 侧把 Array/GreedyRange/PrefixedArray 包在
    Struct 中（`class _C(StructMixin): items: list = field(Array(...))`），
    而 Python 侧直接用 `Array(...)` 不包 Struct。
    这给 Rust 侧加上了 ~150-200ns 的额外 Struct 开销，Python 侧没有。
    验证方法：对相同 Array，Python 也包一层 Struct 测量，比较加速比变化。

H2（小 N 噪声 - number 太小）：
    Phase 4 v3 用 NUMBER=1000（小场景）或更小（大场景）。
    Phase 1 B1-B4 用 NUMBER=30000。小 NUMBER 下 timer 噪声大。
    验证方法：用 NUMBER=30000 重测所有 17 个场景。

H3（Rust 侧固定开销不可压缩）：
    即便 H1/H2 都修正，Rust 侧仍有 FFI + StructNode 入口开销 ~250-350ns。
    对 Array(0) 这种零工作量场景，Python 也只需 ~700ns，比例天然受限于
    (Py_overhead + 0) / (Rs_overhead + 0)。
    验证方法：通过 N 扩展测量 (N=0/10/100) 拟合出 Rs_overhead 与 Py_overhead。

输出：data/*.json + 标准输出对比表。
"""

import json
import os
import subprocess
import sys
import time
from pathlib import Path

# 强制 stdout 用 utf-8
try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")
except Exception:
    pass


# ============================================================
# 测量参数（与 Phase 1 B1 完全一致的口径）
# ============================================================
# 按 N 自适应 NUMBER，避免大规模超时
def _number_for(label, n):
    """根据场景规模返回合适的 NUMBER。"""
    # 零工作量/小场景：用大 NUMBER 拿稳定读数（与 Phase 1 B1 一致）
    if "empty" in label or "stopif" in label or n == 0:
        return 30000
    if n <= 10:
        return 30000
    if n <= 100:
        return 5000
    if n <= 1000:
        return 1000
    if n <= 4096:
        return 300
    return 100


REPEAT = 5
WORKDIR = r"<legacy-repo>\construct-rs"
RS_PY = r"<opencode-temp>\crs_venv\Scripts\python.exe"
PY_PY = r"<opencode-temp>\crs_venv_py\Scripts\python.exe"


# ============================================================
# 场景定义
# ============================================================
# 每个场景：
#   key, label, n, rs_template_id, py_template_id, py_struct_template_id
# template_id 用于查 _RS_TEMPLATES / _PY_TEMPLATES / _PY_STRUCT_TEMPLATES
# 三个 template 都接受 {N} {NUMBER} {REPEAT} 占位。

# Phase 1 B1（基准对照）
B1 = ("B1_modbus", "Phase1-B1 (3 fields Struct + GreedyBytes)")

# 17 个不达 10x 场景
INVESTIGATION_SCENARIOS = [
    # 类别 1：小 N FFI 主导？
    ("a01_i8_n10_parse",     "Array(10, Int8ub) parse",              10),
    ("a01_i8_n10_build",     "Array(10, Int8ub) build",              10),
    ("g01_i8_n10_parse",     "GreedyRange(Int8ub) N=10 parse",       10),
    ("g01_i8_n10_build",     "GreedyRange(Int8ub) N=10 build",       10),
    ("p01_i8_n10_parse",     "PrefixedArray(Byte, Int8ub) N=10 parse",  10),
    ("p01_i8_n10_build",     "PrefixedArray(Byte, Int8ub) N=10 build",  10),
    ("i01_idx_n10_parse",    "Array(10, Index()) parse",             10),
    ("i01_idx_n10_build",    "Array(10, Index()) build",             10),
    # 类别 2：辅助节点结构性受限？
    ("i02_idx_n100_parse",   "Array(100, Index()) parse",            100),
    ("i03_idx_n1000_parse",  "Array(1000, Index()) parse",           1000),
    ("i04_idx_n4096_parse",  "Array(4096, Index()) parse",           4096),
    ("s01_stopif_no_parse",  "StopIf(x==0) 不触发 parse",            1),
    ("s03_stopif_true_parse","StopIf(True) 触发 parse",               1),
    # 类别 3：错误路径开销
    ("p_err_overflow_build", "PrefixedArray cf=300 build overflow err-build", 300),
    ("a_err_eof_parse",      "Array(100, Int8ub) 50B stream EOF err-parse",  100),
    # 类别 4：零工作量场景
    ("e01_empty_parse",      "Array(0, Index()) 空数组 parse",        0),
    ("e01_empty_build",      "Array(0, Index()) 空数组 build",        0),
]


# ============================================================
# Rust 模板（construct-rs 用户面 API）
# ============================================================
def _rs_template(key):
    """返回 construct-rs 测量代码。

    所有场景都使用 StructMixin 包装（与 Phase 4 v3 benchmark 一致）。
    """
    n_placeholder = "{N}"
    templates = {
        # ===== Phase 1 B1 基准 =====
        "B1_modbus": """
import timeit
from dataclasses import dataclass
from construct import StructMixin, field, Int8ub, GreedyBytes

@dataclass
class B1(StructMixin):
    address: int = field(Int8ub)
    function_code: int = field(Int8ub)
    data: bytes = field(GreedyBytes)

DATA = bytes([1, 3, 0, 1, 0, 2])

def run_parse():
    B1.parse(DATA)

b1_obj = B1.parse(DATA)
def run_build():
    b1_obj.build()

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
t = timeit.repeat(run_build, number={NUMBER}, repeat={REPEAT})
print('BUILD_NS', min(t) / {NUMBER} * 1e9)
""",
        # ===== 类别 1：小 N =====
        "a01_i8_n10": """
import timeit
from dataclasses import dataclass
from construct import StructMixin, field, Array, Int8ub

N = {N}

@dataclass
class P(StructMixin):
    items: list = field(Array(N, Int8ub))

DATA = bytes(i % 256 for i in range(N))

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
        "g01_i8_n10": """
import timeit
from dataclasses import dataclass
from construct import StructMixin, field, GreedyRange, Int8ub

N = {N}

@dataclass
class P(StructMixin):
    items: list = field(GreedyRange(Int8ub))

DATA = bytes(i % 256 for i in range(N))

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
        "p01_i8_n10": """
import timeit
from dataclasses import dataclass
from construct import StructMixin, field, PrefixedArray, Int8ub

N = {N}

@dataclass
class P(StructMixin):
    items: list = field(PrefixedArray(Int8ub, Int8ub))

DATA = bytes([N]) + bytes(i % 256 for i in range(N))

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
        # Array(N, Index()) — 同时用于 i01/i02/i03/i04/e01
        "i01_idx_nN": """
import timeit
from dataclasses import dataclass
from construct import StructMixin, field, Array, Index

N = {N}

@dataclass
class P(StructMixin):
    items: list = field(Array(N, Index()))

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
        # StopIf 单点（在 Struct 中）
        "stopif_no_template": """
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
        "stopif_true_template": """
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
        # 错误路径：PrefixedArray cf=300 build overflow
        "p_err_overflow": """
import timeit
from dataclasses import dataclass
from construct import StructMixin, field, PrefixedArray, Int8ub

@dataclass
class P(StructMixin):
    items: list = field(PrefixedArray(Int8ub, Int8ub))

# list 长度 300，超出 Byte 表示范围 [0, 255]
BIG_OBJ = P(items=list(range(300)))

def run_build():
    try:
        BIG_OBJ.build()
    except Exception:
        pass

# 预热
for _ in range(50):
    try:
        BIG_OBJ.build()
    except Exception:
        pass

import time as _time
t0 = _time.perf_counter()
for _ in range({NUMBER}):
    try:
        BIG_OBJ.build()
    except Exception:
        pass
elapsed = (_time.perf_counter() - t0) / {NUMBER}
print('ERR_BUILD_NS', elapsed * 1e9)
""",
        # 错误路径：Array(100, Int8ub) + 50B stream
        "a_err_eof": """
import timeit
from dataclasses import dataclass
from construct import StructMixin, field, Array, Int8ub

@dataclass
class P(StructMixin):
    items: list = field(Array(100, Int8ub))

DATA = bytes(i % 256 for i in range(50))

def run_parse():
    try:
        P.parse(DATA)
    except Exception:
        pass

# 预热
for _ in range(50):
    try:
        P.parse(DATA)
    except Exception:
        pass

import time as _time
t0 = _time.perf_counter()
for _ in range({NUMBER}):
    try:
        P.parse(DATA)
    except Exception:
        pass
elapsed = (_time.perf_counter() - t0) / {NUMBER}
print('ERR_PARSE_NS', elapsed * 1e9)
""",
    }
    # 路由
    if key == "B1_modbus":
        return templates["B1_modbus"]
    if key == "a01_i8_n10":
        return templates["a01_i8_n10"]
    if key == "g01_i8_n10":
        return templates["g01_i8_n10"]
    if key == "p01_i8_n10":
        return templates["p01_i8_n10"]
    if key in ("i01_idx_n10", "i02_idx_n100", "i03_idx_n1000",
               "i04_idx_n4096", "e01_empty_idx"):
        return templates["i01_idx_nN"]
    if key == "s01_stopif_no":
        return templates["stopif_no_template"]
    if key == "s03_stopif_true":
        return templates["stopif_true_template"]
    if key == "p_err_overflow":
        return templates["p_err_overflow"]
    if key == "a_err_eof":
        return templates["a_err_eof"]
    raise KeyError(key)


def _py_template(key):
    """返回 Python construct 2.10.70 测量代码（直接 Array，无 Struct 包装）。

    与 Phase 4 v3 benchmark 完全一致：直接 Array/GreedyRange/PrefixedArray，
    不包 Struct。
    """
    templates = {
        "B1_modbus": """
import timeit
import construct as pc

d = pc.Struct('address' / pc.Int8ub, 'function_code' / pc.Int8ub, 'data' / pc.GreedyBytes)
DATA = bytes([1, 3, 0, 1, 0, 2])

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
        "a01_i8_n10": """
import timeit
import construct as pc

N = {N}
d = pc.Array(N, pc.Int8ub)
DATA = bytes(i % 256 for i in range(N))

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
        "g01_i8_n10": """
import timeit
import construct as pc

N = {N}
d = pc.GreedyRange(pc.Int8ub)
DATA = bytes(i % 256 for i in range(N))

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
        "p01_i8_n10": """
import timeit
import construct as pc

N = {N}
d = pc.PrefixedArray(pc.Int8ub, pc.Int8ub)
DATA = bytes([N]) + bytes(i % 256 for i in range(N))

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
        "i01_idx_nN": """
import timeit
import construct as pc

N = {N}
d = pc.Array(N, pc.Index)

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
        "stopif_no_template": """
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
        "stopif_true_template": """
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
        "p_err_overflow": """
import timeit
import construct as pc

d = pc.PrefixedArray(pc.Int8ub, pc.Int8ub)
BIG = list(range(300))

def run_build():
    try:
        d.build(BIG)
    except Exception:
        pass

for _ in range(50):
    try:
        d.build(BIG)
    except Exception:
        pass

import time as _time
t0 = _time.perf_counter()
for _ in range({NUMBER}):
    try:
        d.build(BIG)
    except Exception:
        pass
elapsed = (_time.perf_counter() - t0) / {NUMBER}
print('ERR_BUILD_NS', elapsed * 1e9)
""",
        "a_err_eof": """
import timeit
import construct as pc

d = pc.Array(100, pc.Int8ub)
DATA = bytes(i % 256 for i in range(50))

def run_parse():
    try:
        d.parse(DATA)
    except Exception:
        pass

for _ in range(50):
    try:
        d.parse(DATA)
    except Exception:
        pass

import time as _time
t0 = _time.perf_counter()
for _ in range({NUMBER}):
    try:
        d.parse(DATA)
    except Exception:
        pass
elapsed = (_time.perf_counter() - t0) / {NUMBER}
print('ERR_PARSE_NS', elapsed * 1e9)
""",
    }
    if key == "B1_modbus":
        return templates["B1_modbus"]
    if key == "a01_i8_n10":
        return templates["a01_i8_n10"]
    if key == "g01_i8_n10":
        return templates["g01_i8_n10"]
    if key == "p01_i8_n10":
        return templates["p01_i8_n10"]
    if key in ("i01_idx_n10", "i02_idx_n100", "i03_idx_n1000",
               "i04_idx_n4096", "e01_empty_idx"):
        return templates["i01_idx_nN"]
    if key == "s01_stopif_no":
        return templates["stopif_no_template"]
    if key == "s03_stopif_true":
        return templates["stopif_true_template"]
    if key == "p_err_overflow":
        return templates["p_err_overflow"]
    if key == "a_err_eof":
        return templates["a_err_eof"]
    raise KeyError(key)


def _py_struct_template(key):
    """返回 Python construct 测量代码，但包一层 Struct（apples-to-apples）。

    用于验证 H1：如果 Python 也包 Struct，加速比会显著上升。
    """
    templates = {
        "a01_i8_n10": """
import timeit
import construct as pc

N = {N}
d = pc.Struct('items' / pc.Array(N, pc.Int8ub))
DATA = bytes(i % 256 for i in range(N))

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
        "g01_i8_n10": """
import timeit
import construct as pc

N = {N}
d = pc.Struct('items' / pc.GreedyRange(pc.Int8ub))
DATA = bytes(i % 256 for i in range(N))

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
        "p01_i8_n10": """
import timeit
import construct as pc

N = {N}
d = pc.Struct('items' / pc.PrefixedArray(pc.Int8ub, pc.Int8ub))
DATA = bytes([N]) + bytes(i % 256 for i in range(N))

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
        # i01_idx_n10 / i02..i04 / e01_empty_idx 全部复用此模板
        "i01_idx_n10": """
import timeit
import construct as pc

N = {N}
d = pc.Struct('items' / pc.Array(N, pc.Index))

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
        "i02_idx_n100": None,
        "i03_idx_n1000": None,
        "i04_idx_n4096": None,
        "e01_empty_idx": None,
    }
    if templates.get(key) is None and key in (
        "i02_idx_n100", "i03_idx_n1000", "i04_idx_n4096", "e01_empty_idx"
    ):
        return templates["i01_idx_n10"]
    return templates.get(key)


# ============================================================
# 模板路由：场景 key → 三套模板
# ============================================================
def _resolve_templates(scenario_key):
    """根据场景 key 返回 (rs_tpl_key, py_tpl_key, py_struct_tpl_key or None)。

    py_struct_tpl_key 为 None 表示该场景无 Struct 包装对照（如 StopIf 必须在 Struct 中）。
    """
    mapping = {
        "B1_modbus":            ("B1_modbus",         "B1_modbus",         None),
        "a01_i8_n10_parse":     ("a01_i8_n10",        "a01_i8_n10",        "a01_i8_n10"),
        "a01_i8_n10_build":     ("a01_i8_n10",        "a01_i8_n10",        "a01_i8_n10"),
        "g01_i8_n10_parse":     ("g01_i8_n10",        "g01_i8_n10",        "g01_i8_n10"),
        "g01_i8_n10_build":     ("g01_i8_n10",        "g01_i8_n10",        "g01_i8_n10"),
        "p01_i8_n10_parse":     ("p01_i8_n10",        "p01_i8_n10",        "p01_i8_n10"),
        "p01_i8_n10_build":     ("p01_i8_n10",        "p01_i8_n10",        "p01_i8_n10"),
        "i01_idx_n10_parse":    ("i01_idx_n10",       "i01_idx_n10",       "i01_idx_n10"),
        "i01_idx_n10_build":    ("i01_idx_n10",       "i01_idx_n10",       "i01_idx_n10"),
        "i02_idx_n100_parse":   ("i02_idx_n100",      "i02_idx_n100",      "i02_idx_n100"),
        "i03_idx_n1000_parse":  ("i03_idx_n1000",     "i03_idx_n1000",     "i03_idx_n1000"),
        "i04_idx_n4096_parse":  ("i04_idx_n4096",     "i04_idx_n4096",     "i04_idx_n4096"),
        "s01_stopif_no_parse":  ("s01_stopif_no",     "s01_stopif_no",     None),
        "s03_stopif_true_parse":("s03_stopif_true",   "s03_stopif_true",   None),
        "p_err_overflow_build": ("p_err_overflow",    "p_err_overflow",    None),
        "a_err_eof_parse":      ("a_err_eof",         "a_err_eof",         None),
        "e01_empty_parse":      ("e01_empty_idx",     "e01_empty_idx",     "e01_empty_idx"),
        "e01_empty_build":      ("e01_empty_idx",     "e01_empty_idx",     "e01_empty_idx"),
    }
    return mapping[scenario_key]


def _measure_direction(template_str, n, number, python_exe):
    """根据模板内容打印的标签自动识别 parse_ns/build_ns/err_*_ns。

    返回 dict {label: ns_value}。python_exe 指定使用哪个 Python 解释器。
    """
    code = template_str.format(N=n, NUMBER=number, REPEAT=REPEAT)
    proc = subprocess.run(
        [python_exe, "-c", code],
        cwd=WORKDIR, capture_output=True, text=True,
        env={**os.environ, "PYO3_USE_ABI3_FORWARD_COMPATIBILITY": "1"},
    )
    if proc.returncode != 0:
        raise RuntimeError("subprocess failed:\n" + proc.stderr)
    out = {}
    for line in proc.stdout.strip().split("\n"):
        parts = line.split()
        if len(parts) == 2:
            label, val = parts
            out[label] = float(val)
    return out


def _rs_measure(scenario_key, n):
    """Rust 侧测量（使用 construct-rs venv）。返回 dict。"""
    rs_tpl_key, _, _ = _resolve_templates(scenario_key)
    tpl = _rs_template(rs_tpl_key)
    number = _number_for(scenario_key, n)
    return _measure_direction(tpl, n, number, RS_PY), number


def _py_measure(scenario_key, n, struct_wrapped):
    """Python 侧测量（使用 construct venv）。

    struct_wrapped=False 用直接 Array 模板，True 用 Struct 包装模板。
    """
    _, py_tpl_key, py_struct_tpl_key = _resolve_templates(scenario_key)
    if struct_wrapped:
        if py_struct_tpl_key is None:
            return None, None
        tpl = _py_struct_template(py_struct_tpl_key)
    else:
        tpl = _py_template(py_tpl_key)
    number = _number_for(scenario_key, n)
    return _measure_direction(tpl, n, number, PY_PY), number


# ============================================================
# 主流程
# ============================================================
def main():
    # 选择哪些场景在主流程跑（B1 + 17 个）
    # 命令行参数支持单场景运行
    if len(sys.argv) > 1 and sys.argv[1] != "all":
        keys_to_run = [sys.argv[1]]
    else:
        keys_to_run = [s[0] for s in INVESTIGATION_SCENARIOS] + [B1[0]]

    results = {}

    print("=" * 110)
    print("Phase 4 性能塌方根因调查：同次测量（B1 + 17 个不达 10x 场景）")
    print("=" * 110)

    for scenario_key in keys_to_run:
        # 找到 label 和 n
        label = None
        n = 0
        for k, lbl, nv in INVESTIGATION_SCENARIOS:
            if k == scenario_key:
                label = lbl
                n = nv
                break
        if label is None and scenario_key == B1[0]:
            label = B1[1]
            n = 6  # Modbus B1 数据长度

        print(f"\n>>> 场景 {scenario_key}: {label} (N={n})")

        # Rust 测量
        rs_out, rs_number = _rs_measure(scenario_key, n)
        print(f"  [rs ] NUMBER={rs_number}: {rs_out}")

        # Python 直接（与 v3 benchmark 一致）
        py_out, py_number = _py_measure(scenario_key, n, struct_wrapped=False)
        if py_out is not None:
            print(f"  [py*] NUMBER={py_number}: {py_out}  (* = 直接 Array，无 Struct)")

        # Python Struct 包装（apples-to-apples）
        py_s_out, _ = _py_measure(scenario_key, n, struct_wrapped=True)
        if py_s_out is not None:
            print(f"  [py+] NUMBER={py_number}: {py_s_out}  (+ = Struct 包装)")

        results[scenario_key] = {
            "label": label,
            "n": n,
            "rs": rs_out,
            "py_bare": py_out,
            "py_struct": py_s_out,
            "rs_number": rs_number,
        }

    # 保存数据
    out_dir = Path(__file__).parent / "phase4_investigation_data"
    out_dir.mkdir(exist_ok=True)
    out_file = out_dir / "measurements.json"
    with open(out_file, "w", encoding="utf-8") as f:
        json.dump(results, f, indent=2, ensure_ascii=False)
    print(f"\n[数据已保存到 {out_file}]")


if __name__ == "__main__":
    main()
