"""测 Python construct 2.10.70 的同等场景：空 Struct / 1 字段 Struct / Array(0, Index) / 等。
"""
import subprocess
import os
import sys

PY_PY = r"<opencode-temp>\crs_venv_py\Scripts\python.exe"
NUMBER = 30000
REPEAT = 5

SCENARIOS = [
    ("empty_struct", "pc.Struct() 空结构", """
import timeit
import construct as pc

d = pc.Struct()

def run_parse():
    d.parse(b'')

parsed = d.parse(b'')
def run_build():
    d.build(parsed)

t = timeit.repeat(run_parse, number=""" + str(NUMBER) + """, repeat=""" + str(REPEAT) + """)
print('PARSE_NS', min(t) / """ + str(NUMBER) + """ * 1e9)
t = timeit.repeat(run_build, number=""" + str(NUMBER) + """, repeat=""" + str(REPEAT) + """)
print('BUILD_NS', min(t) / """ + str(NUMBER) + """ * 1e9)
"""),
    ("one_byte", "pc.Struct('x'/Int8ub)", """
import timeit
import construct as pc

d = pc.Struct('x' / pc.Int8ub)

def run_parse():
    d.parse(b'\\x01')

parsed = d.parse(b'\\x01')
def run_build():
    d.build(parsed)

t = timeit.repeat(run_parse, number=""" + str(NUMBER) + """, repeat=""" + str(REPEAT) + """)
print('PARSE_NS', min(t) / """ + str(NUMBER) + """ * 1e9)
t = timeit.repeat(run_build, number=""" + str(NUMBER) + """, repeat=""" + str(REPEAT) + """)
print('BUILD_NS', min(t) / """ + str(NUMBER) + """ * 1e9)
"""),
    ("array0_idx", "pc.Struct('items'/Array(0, Index))", """
import timeit
import construct as pc

d = pc.Struct('items' / pc.Array(0, pc.Index))

def run_parse():
    d.parse(b'')

parsed = d.parse(b'')
def run_build():
    d.build(parsed)

t = timeit.repeat(run_parse, number=""" + str(NUMBER) + """, repeat=""" + str(REPEAT) + """)
print('PARSE_NS', min(t) / """ + str(NUMBER) + """ * 1e9)
t = timeit.repeat(run_build, number=""" + str(NUMBER) + """, repeat=""" + str(REPEAT) + """)
print('BUILD_NS', min(t) / """ + str(NUMBER) + """ * 1e9)
"""),
    ("array0_bare", "pc.Array(0, Index) 直接（无 Struct）", """
import timeit
import construct as pc

d = pc.Array(0, pc.Index)

def run_parse():
    d.parse(b'')

parsed = d.parse(b'')
def run_build():
    d.build(parsed)

t = timeit.repeat(run_parse, number=""" + str(NUMBER) + """, repeat=""" + str(REPEAT) + """)
print('PARSE_NS', min(t) / """ + str(NUMBER) + """ * 1e9)
t = timeit.repeat(run_build, number=""" + str(NUMBER) + """, repeat=""" + str(REPEAT) + """)
print('BUILD_NS', min(t) / """ + str(NUMBER) + """ * 1e9)
"""),
    ("array1_idx", "pc.Struct('items'/Array(1, Index))", """
import timeit
import construct as pc

d = pc.Struct('items' / pc.Array(1, pc.Index))

def run_parse():
    d.parse(b'')

parsed = d.parse(b'')
def run_build():
    d.build(parsed)

t = timeit.repeat(run_parse, number=""" + str(NUMBER) + """, repeat=""" + str(REPEAT) + """)
print('PARSE_NS', min(t) / """ + str(NUMBER) + """ * 1e9)
t = timeit.repeat(run_build, number=""" + str(NUMBER) + """, repeat=""" + str(REPEAT) + """)
print('BUILD_NS', min(t) / """ + str(NUMBER) + """ * 1e9)
"""),
    ("array10_idx", "pc.Struct('items'/Array(10, Index))", """
import timeit
import construct as pc

d = pc.Struct('items' / pc.Array(10, pc.Index))

def run_parse():
    d.parse(b'')

parsed = d.parse(b'')
def run_build():
    d.build(parsed)

t = timeit.repeat(run_parse, number=""" + str(NUMBER) + """, repeat=""" + str(REPEAT) + """)
print('PARSE_NS', min(t) / """ + str(NUMBER) + """ * 1e9)
t = timeit.repeat(run_build, number=""" + str(NUMBER) + """, repeat=""" + str(REPEAT) + """)
print('BUILD_NS', min(t) / """ + str(NUMBER) + """ * 1e9)
"""),
    ("array100_idx", "pc.Struct('items'/Array(100, Index))", """
import timeit
import construct as pc

d = pc.Struct('items' / pc.Array(100, pc.Index))

def run_parse():
    d.parse(b'')

parsed = d.parse(b'')
def run_build():
    d.build(parsed)

t = timeit.repeat(run_parse, number=5000, repeat=""" + str(REPEAT) + """)
print('PARSE_NS', min(t) / 5000 * 1e9)
t = timeit.repeat(run_build, number=5000, repeat=""" + str(REPEAT) + """)
print('BUILD_NS', min(t) / 5000 * 1e9)
"""),
]

for key, label, code in SCENARIOS:
    print(f">>> {key}: {label}")
    proc = subprocess.run(
        [PY_PY, "-c", code],
        capture_output=True, text=True,
    )
    if proc.returncode != 0:
        print(f"  FAILED: {proc.stderr[:500]}")
        continue
    for line in proc.stdout.strip().split("\n"):
        if line.strip():
            parts = line.split()
            print(f"  {parts[0]}: {float(parts[1]):.1f} ns")
    print()
