# Exploratory session 05b: fixed Array boundary + VarInt inside struct + PrefixedArray huge
import subprocess, sys, time

def run_case(name, code, timeout=180):
    t0 = time.time()
    try:
        r = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, timeout=timeout)
        el = time.time() - t0
        status = "OK" if r.returncode == 0 else f"rc={r.returncode}"
        out = (r.stdout + r.stderr).strip().replace("\n", " | ")[:260]
        print(f"--- {name} [{el:.1f}s] {status}: {out}")
    except subprocess.TimeoutExpired:
        print(f"--- {name}: TIMEOUT after {timeout}s")

print("=== Array abort boundary (fixed hex) ===")
cases = [
    ("524288 elems, no data", "524288"),
    ("2097152 elems, no data", "2097152"),
    ("16000000 elems, no data", "16000000"),
    ("160000000 elems, no data", "160000000"),
]
for label, n in cases:
    run_case(f"Array count={label}", f'''
from neoconstruct import StructMixin, field, Int32ub, Int16ub, Array
from dataclasses import dataclass
@dataclass
class P1(StructMixin):
    count: int = field(Int32ub)
    items: list = field(Array(count, Int16ub))
data = ({n}).to_bytes(4, "big") + b"\\x00" * 100
try:
    r = P1.parse(data)
    print("parsed", len(r.items))
except MemoryError as e:
    print("MemoryError (python-level):", e)
except Exception as e:
    print("raised:", type(e).__name__, str(e)[:90])
''')

print()
print("=== PrefixedArray huge count ===")
run_case("PrefixedArray count=0xFFFFFFFF", '''
from neoconstruct import StructMixin, field, Int32ub, Int8ub, PrefixedArray
from dataclasses import dataclass
@dataclass
class P(StructMixin):
    items: list = field(PrefixedArray(Int32ub, Int8ub))
try:
    r = P.parse(b"\\xff\\xff\\xff\\xff")
    print("parsed", len(r.items))
except Exception as e:
    print("raised:", type(e).__name__, str(e)[:90])
''')

print()
print("=== GreedyRange(Array) nested huge ===")
run_case("Array(count, Array(1000, Int8ub)) huge", '''
from neoconstruct import StructMixin, field, Int32ub, Int16ub, Int8ub, Array
from dataclasses import dataclass
@dataclass
class P(StructMixin):
    count: int = field(Int32ub)
    items: list = field(Array(count, Array(1000, Int8ub)))
try:
    r = P.parse((200000).to_bytes(4, "big") + b"\\x00" * 50)
    print("parsed", len(r.items))
except Exception as e:
    print("raised:", type(e).__name__, str(e)[:90]
''')

print()
print("=== VarInt malformed inside struct ===")
run_case("VarInt overlong", '''
from neoconstruct import StructMixin, field, VarInt
from dataclasses import dataclass
@dataclass
class P(StructMixin):
    v: int = field(VarInt)
for blob in (b"\\xff"*9 + b"\\x01", b"\\xff"*10, b"\\xff"*11 + b"\\x00", b"\\x80"*30):
    try:
        print("parsed", P.parse(blob).v)
    except Exception as e:
        print("raised:", type(e).__name__, str(e)[:100])
''')

print()
print("=== BytesInteger malformed / huge ===")
run_case("BytesInteger(16)", '''
from neoconstruct import StructMixin, field, BytesInteger
from dataclasses import dataclass
@dataclass
class P(StructMixin):
    v: int = field(BytesInteger(16))
try:
    print(P.parse(b"\\x00"*16).v)
except Exception as e:
    print("raised:", type(e).__name__, str(e)[:100]
''')
