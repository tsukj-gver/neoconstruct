# Exploratory session 05: abort boundary, GIL hold/interrupt latency, VarInt malformed, Timestamp dep
import subprocess, sys, time
from dataclasses import dataclass

def run_case(name, code, timeout=120):
    t0 = time.time()
    try:
        r = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, timeout=timeout)
        el = time.time() - t0
        status = "OK" if r.returncode == 0 else f"rc={r.returncode}"
        out = (r.stdout + r.stderr).strip().replace("\n", " | ")[:300]
        print(f"--- {name} [{el:.1f}s] {status}: {out}")
    except subprocess.TimeoutExpired:
        print(f"--- {name}: TIMEOUT after {timeout}s")

print("=== Array abort boundary: which counts kill the process ===")
for count_hex, label in [
    ("\\x00\\x01\\x86\\xA0", "100_000 (real 200KB data)"),
    ("\\x00\\x98\\x96\\x80", "160_000_000 no data"),
    ("\\x00\\x20\\x00\\x00", "2_097_152 = 2M elems, no data"),
    ("\\x00\\x08\\x00\\x00", "524_288 no data"),
]:
    run_case(f"Array count={label}", f'''
from neoconstruct import StructMixin, field, Int32ub, Int16ub, Array
from dataclasses import dataclass
@dataclass
class P1(StructMixin):
    count: int = field(Int32ub)
    items: list = field(Array(count, Int16ub))
data = bytes.fromhex("{count_hex}") + b"\\x00" * 100
try:
    r = P1.parse(data)
    print("parsed", len(r.items))
except MemoryError as e:
    print("MemoryError (python-level):", e)
except Exception as e:
    print("raised:", type(e).__name__, str(e)[:90])
''')

print()
print("=== Array BUILD side: huge list from user ===")
run_case("build Array 50M list", '''
from neoconstruct import StructMixin, field, Int8ub, Int16ub, Array
from dataclasses import dataclass
@dataclass
class P(StructMixin):
    items: list = field(Array(50_000_000, Int8ub))
try:
    b = P(items=[1]*50_000_000).build()
    print("built", len(b))
except Exception as e:
    print("raised:", type(e).__name__, str(e)[:90])
''', timeout=180)

print()
print("=== VarInt malformed (overlong LEB128) ===")
run_case("VarInt 11x 0xff", '''
from neoconstruct import VarInt
for blob in (b"\\xff"*10, b"\\xff"*11 + b"\\x00", b"\\x80"*20):
    try:
        v = VarInt.parse(blob)
        print("parsed", v)
    except Exception as e:
        print("raised:", type(e).__name__, str(e)[:90])
''')

print()
print("=== Timestamp without arrow installed ===")
run_case("Timestamp no arrow", '''
from neoconstruct import Timestamp, Int32ub
from dataclasses import dataclass
from neoconstruct import StructMixin, field
try:
    ts = Timestamp(Int32ub, "unix", 0)
    print("created:", ts)
    try:
        print(ts.parse(b"\\x00\\x00\\x00\\x01"))
    except Exception as e:
        print("parse:", type(e).__name__, str(e)[:150])
except Exception as e:
    print("create:", type(e).__name__, str(e)[:200])
''')

print()
print("=== GIL hold: single big parse call latency (UI/async stall risk) ===")
run_case("Array 10M elems parse timing", '''
from neoconstruct import StructMixin, field, Int32ub, Int16ub, Array
from dataclasses import dataclass
import time
@dataclass
class P(StructMixin):
    count: int = field(Int32ub)
    items: list = field(Array(count, Int16ub))
data = (10_000_000).to_bytes(4, "big") + b"\\x00" * 20_000_000
t0 = time.time()
r = P.parse(data)
t1 = time.time()
print(f"parse 10M-elem array: {t1-t0:.3f}s")
t0 = time.time()
r.build()
t1 = time.time()
print(f"build 10M-elem array: {t1-t0:.3f}s")
''', timeout=300)

print()
print("=== KeyboardInterrupt during long parse (signal latency) ===")
run_case("SIGINT during 10M parse", r'''
from neoconstruct import StructMixin, field, Int32ub, Int16ub, Array
from dataclasses import dataclass
import time, signal
@dataclass
class P(StructMixin):
    count: int = field(Int32ub)
    items: list = field(Array(count, Int16ub))
data = (10_000_000).to_bytes(4, "big") + b"\x00" * 20_000_000
def fire():
    time.sleep(0.05)
    signal.raise_signal(signal.SIGINT)
import threading
threading.Thread(target=fire, daemon=True).start()
t0 = time.time()
try:
    r = P.parse(data)
    print("parse finished without interrupt", time.time()-t0)
except KeyboardInterrupt:
    print(f"KeyboardInterrupt after {time.time()-t0:.3f}s (latency vs 0.05s fire)")
''', timeout=300)
