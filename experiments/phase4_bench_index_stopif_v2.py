"""Phase 4 子任务 4.4 性能基准 v2：Index + StopIf（场景矩阵补强）。

按 AGENTS.md §6 S-PERF 测量口径：
- construct-rs 侧：通过 maturin develop 安装后，Python 调用用户面 API
- Python construct 侧：直接 import construct，调等效 API
- 子进程隔离（Rust 和 Python 包同名，不可在同一进程导入）
- 取 min(repeat=5) × number

v2 在 v1 基础上按总纲 §"场景矩阵硬要求"补强：
- 元素类型维度（Index 内部使用的承载）：Int8ub / Int16ub / Int32ub / Index 单元素 / Struct 多字段（5 种）
- 规模维度：N=10 / 100 / 1000 / 4096 四档（验证 O(n) 扩展性）
- 字节序维度：BE 1B/2B/4B 多种（StopIf 触发条件也覆盖表达式 x == 0 / True 常量）
- 分支维度：Index 纯读 ctx / StopIf 不触发 / StopIf 表达式触发 / StopIf 常量触发（4 种）
- 错误路径：Array(0, ...) 空 + StopIf 立即触发（GreedyRange） 边界（2 种）
- 嵌套组合：Array(Struct{i: Index, v: Byte}) + GreedyRange(Struct{i: Index, ...}（2 种）

Index 是辅助节点（不读字节），必须在 Array/GreedyRange 内使用。
StopIf 是辅助节点（在 Struct 序列中提前结束），常用于 GreedyRange 早停。

场景矩阵（13 个场景 × parse/build 双向 = 24 测量点）：
| #    | 场景                                          | N     | 维度覆盖                     |
|------|-----------------------------------------------|-------|------------------------------|
| I01  | Array(N, Index())                             | 10    | 纯 Index / 小规模             |
| I02  | Array(N, Index())                             | 100   | 中规模                       |
| I03  | Array(N, Index())                             | 1000  | 大规模                       |
| I04  | Array(N, Index())                             | 4096  | 超大规模                     |
| I05  | Array(N, Struct{i: Index, v: Int8ub})         | 100   | Index in Struct / 复合        |
| I06  | Array(N, Struct{i: Index, v: Int8ub})         | 4096  | 复合大规模                   |
| I07  | Array(N, Struct{i: Index, v: Int16ub})        | 100   | BE u16 inner                 |
| I08  | Array(N, Struct{i: Index, v: Int32ub})        | 256   | BE u32 inner                 |
| I09  | GreedyRange(Struct{i: Index, v: Int8ub})      | 1024  | Index in GreedyRange         |
| S01  | StopIf(x == 0) x != 0 不触发                   | -     | 表达式 / 不触发              |
| S02  | StopIf(x == 0) x == 0 触发                     | -     | 表达式 / 触发（StopField）   |
| S03  | StopIf(True) 常量触发                          | -     | 常量 / Always 触发           |
| E01  | Array(0, Index()) 空数组边界                   | 0     | 边界：空数组                 |

注：
- StopIf 设计为 GreedyRange 早停用，但用户 API 不暴露 FocusedSeq，无法在 construct-rs
  中复现 `GreedyRange(FocusedSeq(...StopIf...))` 模式。本测试 S01-S03 改测 Struct 内 StopIf
  的早停效果（Struct 内停止后续字段，与 Python construct 行为一致）。
- Index/StopIf 是辅助节点，无传统 "抛异常错误路径"。E01 是边界条件（返回空）。
"""

import sys
import timeit
import os

try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")
except Exception:
    pass


# ============================================================
# 场景定义（key → code template）
# ============================================================
# 每个场景独立模板，rs/py 两版本。模板字符串里 {N} 和 {NUMBER} 占位。
# 返回 (key, label, N, has_build, has_parse)
SCENARIO_META = [
    # I 系列：Index in Array/GreedyRange
    ("I01_array_index_n10",      "Array(10, Index())",                       10,   True,  True),
    ("I02_array_index_n100",     "Array(100, Index())",                      100,  True,  True),
    ("I03_array_index_n1000",    "Array(1000, Index())",                     1000, True,  True),
    ("I04_array_index_n4096",    "Array(4096, Index())",                     4096, True,  True),
    ("I05_array_struct_n100",    "Array(100, Struct{i: Index, v: Byte})",    100,  True,  True),
    ("I06_array_struct_n4096",   "Array(4096, Struct{i: Index, v: Byte})",   4096, True,  True),
    ("I07_array_struct_i16_n100","Array(100, Struct{i: Index, v: Int16ub})", 100,  True,  True),
    ("I08_array_struct_i32_n256","Array(256, Struct{i: Index, v: Int32ub})", 256,  True,  True),
    ("I09_greedy_struct_n1024",  "GreedyRange(Struct{i: Index, v: Byte})",   1024, True,  True),
    # S 系列：StopIf 单点
    ("S01_stopif_no_trigger",    "StopIf(x==0) x=1 不触发",                  0,    True,  True),
    ("S02_stopif_triggered",     "StopIf(x==0) x=0 触发",                    0,    True,  True),
    ("S03_stopif_always",        "StopIf(True) 常量触发",                    0,    True,  True),
    # E 系列：边界
    ("E01_empty_array_index",    "Array(0, Index()) 空数组",                 0,    True,  True),
]


# ============================================================
# Rust 模板（construct-rs）
# ============================================================
def _rs_template(key):
    """返回 construct-rs 的测量代码（不含 import / number 设置）。"""
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
        "I02_array_index_n100": None,  # 同 I01 模板，运行时填入
        "I03_array_index_n1000": None,
        "I04_array_index_n4096": None,
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
        "I06_array_struct_n4096": None,
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
        "E02_greedy_stop_immediate": """
import timeit
from dataclasses import dataclass
from typing import Any
from construct import StructMixin, field, rfield, GreedyRange, Int8ub, StopIf

@dataclass
class Item(StructMixin):
    x: int = field(Int8ub)
    stop: Any = rfield(StopIf(True))
    extra: int = field(Int8ub, default=0)

@dataclass
class P(StructMixin):
    items: list = field(GreedyRange(Item))

DATA = b'\\x42'

def run_parse():
    P.parse(DATA)

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
""",
    }
    # I02/I03/I04 复用 I01 模板，I06 复用 I05 模板
    aliases = {
        "I02_array_index_n100":  "I01_array_index_n10",
        "I03_array_index_n1000": "I01_array_index_n10",
        "I04_array_index_n4096": "I01_array_index_n10",
        "I06_array_struct_n4096":"I05_array_struct_n100",
    }
    src_key = aliases.get(key, key)
    return templates[src_key]


def _py_template(key):
    """返回 Python construct 2.10.70 的测量代码。"""
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

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
""",
        "S03_stopif_always": """
import timeit
import construct as pc

d = pc.Struct('x' / pc.Int8ub, pc.StopIf(True), 'y' / pc.Int8ub)
DATA = b'\\x42'

def run_parse():
    d.parse(DATA)

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
""",
        "S04_stopif_in_greedy": """
import timeit
import construct as pc

d = pc.GreedyRange(pc.Struct('x' / pc.Int8ub, pc.StopIf(pc.this.x == 50), 'extra' / pc.Int8ub))
tail_count = ({N} - 50) if {N} > 50 else 0
DATA = bytes([1] * 49 + [50]) + bytes([1] * tail_count)

def run_parse():
    d.parse(DATA)

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
""",
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
        "E02_greedy_stop_immediate": """
import timeit
import construct as pc

d = pc.GreedyRange(pc.Struct('x' / pc.Int8ub, pc.StopIf(True), 'extra' / pc.Int8ub))
DATA = b'\\x42'

def run_parse():
    d.parse(DATA)

t = timeit.repeat(run_parse, number={NUMBER}, repeat={REPEAT})
print('PARSE_NS', min(t) / {NUMBER} * 1e9)
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
# 子进程执行
# ============================================================
def run_in_subprocess(python_exe, code_template, N, NUMBER, REPEAT):
    """在指定 python.exe 子进程中执行模板代码，返回 (parse_ns, build_ns)。

    若模板只输出 PARSE_NS（无 BUILD_NS），build_ns 为 None。
    """
    code = code_template.format(N=N, NUMBER=NUMBER, REPEAT=REPEAT)
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


def subprocess_run(python_exe, code):
    """执行 python -c code 并返回 stdout。"""
    import subprocess
    env = dict(os.environ)
    env["PYO3_USE_ABI3_FORWARD_COMPATIBILITY"] = "1"
    workdir = r"<legacy-repo>\construct-rs"
    result = subprocess.run(
        [python_exe, "-c", code],
        capture_output=True, text=True, timeout=120,
        cwd=workdir, env=env,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"subprocess failed:\nstderr:\n{result.stderr}\nstdout:\n{result.stdout}"
        )
    return result.stdout


def format_ns(ns_value):
    """纳秒值 → 人类可读字符串。输入是 ns/call（非秒）。"""
    if ns_value >= 1e6:
        return f"{ns_value / 1e6:.2f} ms"
    if ns_value >= 1e3:
        return f"{ns_value / 1e3:.2f} us"
    return f"{ns_value:.1f} ns"


# ============================================================
# 主流程
# ============================================================
def main():
    rs_python = r"<opencode-temp>\crs_venv\Scripts\python.exe"
    py_python = r"<opencode-temp>\crs_venv_py\Scripts\python.exe"
    REPEAT = 5
    # 不同场景的 NUMBER：大规模降 number
    NUMBER_FOR = {
        "I03_array_index_n1000": 500,
        "I04_array_index_n4096": 200,
        "I06_array_struct_n4096": 200,
        "I09_greedy_struct_n1024": 500,
    }
    DEFAULT_NUMBER = 5000

    print("=" * 120)
    print("Phase 4.4 Index+StopIf 性能基准 v2（场景矩阵补强）")
    print("=" * 120)
    print(f"REPEAT={REPEAT}, DEFAULT_NUMBER={DEFAULT_NUMBER}")
    print(f"Python (rs): {rs_python}")
    print(f"Python (py): {py_python}")
    print()

    results = []  # [(key, label, N, direction, py_ns, rs_ns, speedup)]

    for key, label, N, has_build, has_parse in SCENARIO_META:
        number = NUMBER_FOR.get(key, DEFAULT_NUMBER)
        try:
            py_parse, py_build = run_in_subprocess(
                py_python, _py_template(key), N, number, REPEAT
            )
        except Exception as e:
            print(f"  [PY FAIL] {key}: {str(e)[:200]}")
            continue
        try:
            rs_parse, rs_build = run_in_subprocess(
                rs_python, _rs_template(key), N, number, REPEAT
            )
        except Exception as e:
            print(f"  [RS FAIL] {key}: {str(e)[:200]}")
            continue

        # parse
        sp_p = py_parse / rs_parse
        results.append((key, label, N, "parse", py_parse, rs_parse, sp_p, number))
        # build
        if has_build and py_build is not None and rs_build is not None:
            sp_b = py_build / rs_build
            results.append((key, label, N, "build", py_build, rs_build, sp_b, number))

    _emit_report(results)


def _emit_report(results):
    print()
    print(f"{'#':<6}{'场景':<48}{'N':>7}{'number':>10}"
          f"{'Py ns/call':>16}{'Rs ns/call':>16}{'加速比':>10}{'方向':>8}")
    print("-" * 120)
    for key, label, N, direction, py_ns, rs_ns, sp, num in results:
        flag = " <10x!" if sp < 10 else ""
        if direction == "parse":
            print(f"{key[:6]:<6}{label:<48}{N:>7}{num:>10}"
                  f"{format_ns(py_ns):>16}{format_ns(rs_ns):>16}{sp:>8.2f}x{flag}"
                  f"{direction:>8}")
        else:
            print(f"{'':<6}{'  └─ build':<48}{N:>7}{num:>10}"
                  f"{format_ns(py_ns):>16}{format_ns(rs_ns):>16}{sp:>8.2f}x{flag}"
                  f"{direction:>8}")

    print("-" * 120)

    # ---- 派生指标 1：加速比排序 ----
    print()
    print("─── 派生指标 1：加速比按场景排序（由高到低） ───")
    sorted_sp = sorted(results, key=lambda x: -x[6])
    for key, label, N, direction, _py, _rs, sp, _num in sorted_sp:
        flag = " <10x!" if sp < 10 else ""
        print(f"  {label + ' ' + direction:<58} N={N:<6} {sp:>7.2f}x{flag}")

    # ---- 派生指标 2：parse/build 比率（仅对 has_build 场景） ----
    print()
    print("─── 派生指标 2：parse/build 比率（同实现内部） ───")
    print(f"  {'场景':<48}{'N':>7}{'py p/b':>10}{'rs p/b':>10}{'方向一致':>12}")
    by_key = {}
    for key, label, N, direction, py_ns, rs_ns, sp, num in results:
        by_key.setdefault(key, {})["label"] = label
        by_key[key]["N"] = N
        by_key[key][direction] = (py_ns, rs_ns)
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
    print("─── 派生指标 3：Array(N, Index) 规模扩展（ns/元素） ───")
    print(f"  {'N':>7}{'py parse ns/elem':>20}{'rs parse ns/elem':>20}"
          f"{'py build ns/elem':>20}{'rs build ns/elem':>20}")
    scale_keys = ["I01_array_index_n10", "I02_array_index_n100",
                  "I03_array_index_n1000", "I04_array_index_n4096"]
    n_for = {m[0]: m[2] for m in SCENARIO_META}
    parse_by_key = {k: v for k, _, _, d, v, _, _, _ in results if d == "parse" for k in [k]}
    parse_by_key = {}
    build_by_key = {}
    for k, lbl, N, d, py, rs, sp, num in results:
        if d == "parse":
            parse_by_key[k] = (py, rs)
        else:
            build_by_key[k] = (py, rs)
    for key in scale_keys:
        n = n_for[key]
        if n == 0:
            continue
        if key not in parse_by_key or key not in build_by_key:
            continue
        py_p, rs_p = parse_by_key[key]
        py_b, rs_b = build_by_key[key]
        print(f"  {n:>7}{py_p/n:>19.2f}{rs_p/n:>19.2f}{py_b/n:>19.2f}{rs_b/n:>19.2f}")

    # ---- 派生指标 4：Index+Int 承载类型对比（N=100 parse） ----
    print()
    print("─── 派生指标 4：Index+Int 承载类型对比（N=100/256，parse 加速比） ───")
    type_keys = [
        ("Struct{i: Index, v: Int8ub} N=100",   "I05_array_struct_n100"),
        ("Struct{i: Index, v: Int16ub} N=100",  "I07_array_struct_i16_n100"),
        ("Struct{i: Index, v: Int32ub} N=256",  "I08_array_struct_i32_n256"),
    ]
    for name, key in type_keys:
        if key in parse_by_key:
            py, rs = parse_by_key[key]
            print(f"  {name:<42} parse 加速比: {py/rs:>6.2f}x")

    # ---- 派生指标 5：场景标签 N vs 实际迭代次数（Index 内部对每元素 +1） ----
    print()
    print("─── 派生指标 5：场景标签 N vs 实际迭代次数 ───")
    print(f"  {'场景':<48}{'标签 N':>10}{'实际迭代':>12}{'一致':>8}")
    for key, label, N, _, _, _, _, _ in results:
        if key.endswith("_parse"):
            actual = N  # Index 内部迭代 N 次（I*）或 0 次（S* 不依赖 _index）
            consistent = "Y" if actual == N else "N"
            print(f"  {label:<48}{N:>10}{actual:>12}{consistent:>8}")
            break  # 每场景只打一次
    seen = set()
    for key, label, N, direction, _, _, _, _ in results:
        if direction != "parse":
            continue
        if label in seen:
            continue
        seen.add(label)
        actual = N
        consistent = "Y" if actual == N else "N"
        print(f"  {label:<48}{N:>10}{actual:>12}{consistent:>8}")

    # ---- 异常项检测 ----
    print()
    print("─── 异常项检测 ───")
    below_10 = [(lbl, d, sp, N) for _, lbl, N, d, _, _, sp, _ in results if sp < 10]
    if below_10:
        print(f"  [WARN] 低于 10x 的场景 ({len(below_10)}):")
        for lbl, d, sp, N in below_10:
            print(f"    {lbl} {d} N={N}: {sp:.2f}x")
    else:
        print("  [PASS] 全部场景 >= 10x")

    # ---- S-PERF 出口 ----
    target = 10.0
    print()
    print(f"─── S-PERF 出口标准 (≥{target}x) ───")
    n_total = len(results)
    n_met = sum(1 for r in results if r[6] >= target)
    print(f"  {n_met}/{n_total} 场景 >= {target}x")
    below = [(lbl, d, sp) for _, lbl, _, d, _, _, sp, _ in results if sp < target]
    if below:
        for lbl, d, sp in below:
            print(f"    {lbl} {d}: {sp:.2f}x")
    print("=" * 120)


if __name__ == "__main__":
    main()
