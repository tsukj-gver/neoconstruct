"""测 Rust 侧固定开销的边界场景：
1. 空 Struct（0 字段）parse/build —— FFI + Struct 入口下界
2. 1 字段 Struct(Int8ub) —— Struct 入口 + 1 字段读写
3. 1 字段 Struct(Struct{}) —— 嵌套 Struct（验证 StructNode 复用开销）
"""
import subprocess
import os
import sys

RS_PY = r"<opencode-temp>\crs_venv\Scripts\python.exe"
WORKDIR = r"<legacy-repo>\construct-rs"
NUMBER = 30000
REPEAT = 5

# 场景：空 Struct + 1字段 Struct + Array(0, Index()) 包装 + Array(1, Index())
SCENARIOS = [
    ("empty_struct", "空 Struct(0 字段)", """
import timeit
from dataclasses import dataclass
from construct import StructMixin

@dataclass
class E(StructMixin):
    pass

def run_parse():
    E.parse(b'')

e_obj = E.parse(b'')
def run_build():
    e_obj.build()

t = timeit.repeat(run_parse, number=""" + str(NUMBER) + """, repeat=""" + str(REPEAT) + """)
print('PARSE_NS', min(t) / """ + str(NUMBER) + """ * 1e9)
t = timeit.repeat(run_build, number=""" + str(NUMBER) + """, repeat=""" + str(REPEAT) + """)
print('BUILD_NS', min(t) / """ + str(NUMBER) + """ * 1e9)
"""),
    ("one_byte", "Struct{1 Int8ub}", """
import timeit
from dataclasses import dataclass
from construct import StructMixin, field, Int8ub

@dataclass
class E(StructMixin):
    x: int = field(Int8ub)

def run_parse():
    E.parse(b'\\x01')

e_obj = E(x=1)
def run_build():
    e_obj.build()

t = timeit.repeat(run_parse, number=""" + str(NUMBER) + """, repeat=""" + str(REPEAT) + """)
print('PARSE_NS', min(t) / """ + str(NUMBER) + """ * 1e9)
t = timeit.repeat(run_build, number=""" + str(NUMBER) + """, repeat=""" + str(REPEAT) + """)
print('BUILD_NS', min(t) / """ + str(NUMBER) + """ * 1e9)
"""),
    ("nested_empty_struct", "Struct{Struct{}}", """
import timeit
from dataclasses import dataclass
from construct import StructMixin, field

@dataclass
class Inner(StructMixin):
    pass

@dataclass
class E(StructMixin):
    inner: object = field(Inner)

def run_parse():
    E.parse(b'')

e_obj = E.parse(b'')
def run_build():
    e_obj.build()

t = timeit.repeat(run_parse, number=""" + str(NUMBER) + """, repeat=""" + str(REPEAT) + """)
print('PARSE_NS', min(t) / """ + str(NUMBER) + """ * 1e9)
t = timeit.repeat(run_build, number=""" + str(NUMBER) + """, repeat=""" + str(REPEAT) + """)
print('BUILD_NS', min(t) / """ + str(NUMBER) + """ * 1e9)
"""),
    ("array0_idx", "Struct{Array(0, Index())}", """
import timeit
from dataclasses import dataclass
from construct import StructMixin, field, Array, Index

@dataclass
class E(StructMixin):
    items: list = field(Array(0, Index()))

def run_parse():
    E.parse(b'')

e_obj = E.parse(b'')
def run_build():
    e_obj.build()

t = timeit.repeat(run_parse, number=""" + str(NUMBER) + """, repeat=""" + str(REPEAT) + """)
print('PARSE_NS', min(t) / """ + str(NUMBER) + """ * 1e9)
t = timeit.repeat(run_build, number=""" + str(NUMBER) + """, repeat=""" + str(REPEAT) + """)
print('BUILD_NS', min(t) / """ + str(NUMBER) + """ * 1e9)
"""),
    ("array1_idx", "Struct{Array(1, Index())}", """
import timeit
from dataclasses import dataclass
from construct import StructMixin, field, Array, Index

@dataclass
class E(StructMixin):
    items: list = field(Array(1, Index()))

def run_parse():
    E.parse(b'')

e_obj = E.parse(b'')
def run_build():
    e_obj.build()

t = timeit.repeat(run_parse, number=""" + str(NUMBER) + """, repeat=""" + str(REPEAT) + """)
print('PARSE_NS', min(t) / """ + str(NUMBER) + """ * 1e9)
t = timeit.repeat(run_build, number=""" + str(NUMBER) + """, repeat=""" + str(REPEAT) + """)
print('BUILD_NS', min(t) / """ + str(NUMBER) + """ * 1e9)
"""),
    ("array10_idx", "Struct{Array(10, Index())}", """
import timeit
from dataclasses import dataclass
from construct import StructMixin, field, Array, Index

@dataclass
class E(StructMixin):
    items: list = field(Array(10, Index()))

def run_parse():
    E.parse(b'')

e_obj = E.parse(b'')
def run_build():
    e_obj.build()

t = timeit.repeat(run_parse, number=""" + str(NUMBER) + """, repeat=""" + str(REPEAT) + """)
print('PARSE_NS', min(t) / """ + str(NUMBER) + """ * 1e9)
t = timeit.repeat(run_build, number=""" + str(NUMBER) + """, repeat=""" + str(REPEAT) + """)
print('BUILD_NS', min(t) / """ + str(NUMBER) + """ * 1e9)
"""),
    ("array100_idx", "Struct{Array(100, Index())}", """
import timeit
from dataclasses import dataclass
from construct import StructMixin, field, Array, Index

@dataclass
class E(StructMixin):
    items: list = field(Array(100, Index()))

def run_parse():
    E.parse(b'')

e_obj = E.parse(b'')
def run_build():
    e_obj.build()

t = timeit.repeat(run_parse, number=5000, repeat=""" + str(REPEAT) + """)
print('PARSE_NS', min(t) / 5000 * 1e9)
t = timeit.repeat(run_build, number=5000, repeat=""" + str(REPEAT) + """)
print('BUILD_NS', min(t) / 5000 * 1e9)
"""),
]

for key, label, code in SCENARIOS:
    print(f">>> {key}: {label}")
    proc = subprocess.run(
        [RS_PY, "-c", code],
        cwd=WORKDIR, capture_output=True, text=True,
        env={**os.environ, "PYO3_USE_ABI3_FORWARD_COMPATIBILITY": "1"},
    )
    if proc.returncode != 0:
        print(f"  FAILED: {proc.stderr[:500]}")
        continue
    for line in proc.stdout.strip().split("\n"):
        if line.strip():
            parts = line.split()
            print(f"  {parts[0]}: {float(parts[1]):.1f} ns")
    print()
