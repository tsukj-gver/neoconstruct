# isolate the abort cases one at a time, run via subprocess to survive crashes
import subprocess, sys, textwrap

CASES = {
    "A1 Array(count=0xFFFFFFFF) no data": '''
from neoconstruct import StructMixin, field, Int8ub, Int32ub, Int16ub, Array
from dataclasses import dataclass
@dataclass
class P1(StructMixin):
    count: int = field(Int32ub)
    items: list = field(Array(count, Int16ub))
try:
    r = P1.parse(b"\\xFF\\xFF\\xFF\\xFF")
    print("parsed:", r.count, len(r.items))
except Exception as e:
    print("raised:", type(e).__name__, str(e)[:80])
''',
    "A2 Array(count=0xFFFFFFFF) 100B data": '''
from neoconstruct import StructMixin, field, Int32ub, Int16ub, Array
from dataclasses import dataclass
@dataclass
class P1(StructMixin):
    count: int = field(Int32ub)
    items: list = field(Array(count, Int16ub))
try:
    r = P1.parse(b"\\xFF\\xFF\\xFF\\xFF" + b"\\x00"*100)
    print("parsed:", r.count, len(r.items))
except Exception as e:
    print("raised:", type(e).__name__, str(e)[:80])
''',
    "A3 Bytes(0xFFFFFFFF)": '''
from neoconstruct import StructMixin, field, Int32ub, Bytes
from dataclasses import dataclass
@dataclass
class P2(StructMixin):
    n: int = field(Int32ub)
    data: bytes = field(Bytes(n))
try:
    r = P2.parse(b"\\xFF\\xFF\\xFF\\xFF")
    print("parsed:", len(r.data))
except Exception as e:
    print("raised:", type(e).__name__, str(e)[:80])
''',
    "A4 Prefixed huge length": '''
from neoconstruct import StructMixin, field, Int32ub, Prefixed, GreedyBytes
from dataclasses import dataclass
@dataclass
class P3(StructMixin):
    data: bytes = field(Prefixed(Int32ub, GreedyBytes))
try:
    r = P3.parse(b"\\xFF\\xFF\\xFF\\xFF")
    print("parsed:", len(r.data))
except Exception as e:
    print("raised:", type(e).__name__, str(e)[:80])
''',
    "A5 signed negative count": '''
from neoconstruct import StructMixin, field, Int32sb, Int8ub, Array
from dataclasses import dataclass
@dataclass
class P1b(StructMixin):
    count: int = field(Int32sb)
    items: list = field(Array(count, Int8ub))
try:
    r = P1b.parse(b"\\xFF\\xFF\\xFF\\xFF")
    print("parsed:", r.count, len(r.items))
except Exception as e:
    print("raised:", type(e).__name__, str(e)[:80])
''',
    "B1 Computed a//0": '''
from neoconstruct import StructMixin, field, rfield, Int8ub, Computed
from dataclasses import dataclass
@dataclass
class D(StructMixin):
    a: int = field(Int8ub)
    b: int = field(Int8ub)
    q: int = rfield(Computed(a // b))
try:
    r = D.parse(b"\\x0A\\x00")
    print("parsed:", r)
except Exception as e:
    print("raised:", type(e).__name__, str(e)[:120])
''',
    "B2 Computed shift overflow": '''
from neoconstruct import StructMixin, field, rfield, Int8ub, Computed
from dataclasses import dataclass
@dataclass
class S(StructMixin):
    a: int = field(Int8ub)
    big: int = rfield(Computed(a << 62))
try:
    print("2<<62:", S.parse(b"\\x02").big)
except Exception as e:
    print("raised:", type(e).__name__, str(e)[:120])
try:
    print("8<<62:", S.parse(b"\\x08").big)
except Exception as e:
    print("raised 8:", type(e).__name__, str(e)[:120])
''',
    "C1 CString no terminator": '''
from neoconstruct import StructMixin, field, CString
from dataclasses import dataclass
@dataclass
class StrP(StructMixin):
    name: str = field(CString("utf8"))
try:
    print("no-term:", StrP.parse(b"hello"))
except Exception as e:
    print("raised:", type(e).__name__, str(e)[:100])
try:
    print("invalid-utf8:", StrP.parse(b"\\xff\\xfe\\x00"))
except Exception as e:
    print("raised:", type(e).__name__, str(e)[:100])
''',
    "D1 GreedyRange partial tail": '''
from neoconstruct import StructMixin, field, Int8ub, GreedyRange
from dataclasses import dataclass
@dataclass
class Item(StructMixin):
    x: int = field(Int8ub)
    y: int = field(Int8ub)
@dataclass
class SeqP(StructMixin):
    items: list = field(GreedyRange(Item))
try:
    r = SeqP.parse(b"\\x01\\x02\\x03\\x04\\x05")
    print("len:", len(r.items), "dangling dropped, rebuild==", r.build())
except Exception as e:
    print("raised:", type(e).__name__, str(e)[:100])
''',
    "E1 Switch default Pass roundtrip": '''
from neoconstruct import StructMixin, field, Int8ub, Switch
from dataclasses import dataclass
@dataclass
class TypeA(StructMixin):
    a: int = field(Int8ub)
@dataclass
class Frame(StructMixin):
    type_code: int = field(Int8ub)
    payload: object = field(Switch(type_code, {1: TypeA}))
f = Frame.parse(b"\\x07\\xAA")
print("parsed:", f)
try:
    print("rebuild:", f.build())
except Exception as e:
    print("raised:", type(e).__name__, str(e)[:100])
''',
}

for name, code in CASES.items():
    r = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, timeout=60)
    status = "EXITED" if r.returncode == 0 else f"rc={r.returncode}"
    print(f"--- {name}: {status}")
    out = (r.stdout + r.stderr).strip()
    print("   ", out.replace("\n", "\n    ")[:400])
