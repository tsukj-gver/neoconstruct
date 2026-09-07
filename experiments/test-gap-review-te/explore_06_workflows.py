# Exploratory session 06: user workflows (replace/asdict), fwd/circular refs, rerun broken cases
from dataclasses import dataclass, replace, asdict, astuple
from neoconstruct import StructMixin, field, rfield, Int8ub, Int16ub, Int32ub, Array

print("=== T. dataclasses.replace / asdict / astuple on parsed objects ===")
@dataclass
class P(StructMixin):
    a: int = field(Int8ub)
    b: int = field(Int16ub)

p = P.parse(b"\x01\x00\x02")
try:
    p2 = replace(p, a=9)
    print("replace:", p2, "-> build:", p2.build())
except Exception as e:
    print(f"replace: {type(e).__name__}: {str(e)[:140]}")
try:
    print("asdict:", asdict(p))
except Exception as e:
    print(f"asdict: {type(e).__name__}: {str(e)[:140]}")
try:
    print("astuple:", astuple(p))
except Exception as e:
    print(f"astuple: {type(e).__name__}: {str(e)[:140]}")

print()
print("=== U. forward reference: parent defined before child ===")
try:
    @dataclass
    class Parent(StructMixin):
        n: int = field(Int8ub)
        child: object = field(__import__("neoconstruct").LazyForward("Child") if hasattr(__import__("neoconstruct"), "LazyForward") else Int8ub)
    print("no LazyForward; skip")
except Exception as e:
    print("skip:", e)

# The realistic pattern: parent references child class defined LATER in module
code = '''
from dataclasses import dataclass
from neoconstruct import StructMixin, field, Int8ub

@dataclass
class Parent(StructMixin):
    n: int = field(Int8ub)
    child: "Child" = None
import neoconstruct
'''
# Actually test: can field() reference a name not yet defined (string annotation)?
try:
    @dataclass
    class Parent2(StructMixin):
        n: int = field(Int8ub)
        child: "NotDefinedYet" = field(Int8ub)  # string annotation, subcon is Int8ub anyway
    print("string annotation + real subcon: OK:", Parent2.parse(b"\x01\x02"))
except Exception as e:
    print(f"string annotation: {type(e).__name__}: {str(e)[:120]}")

print()
print("=== V. recursive protocol (TLV containing itself) ===")
try:
    @dataclass
    class TLV(StructMixin):
        t: int = field(Int8ub)
        v: object = field(Int8ub)  # placeholder

    # true recursion: value can be a list of TLV
    from neoconstruct import GreedyRange
    @dataclass
    class TLVNode(StructMixin):
        t: int = field(Int8ub)
        children: list = field(GreedyRange("self"))  # self-reference attempt
    print("self-ref via string:", TLVNode)
except Exception as e:
    print(f"self-ref attempt: {type(e).__name__}: {str(e)[:140]}")

print()
print("=== W. GreedyRange(Array) nested huge count (rerun, fixed) ===")
import subprocess, sys
r = subprocess.run([sys.executable, "-c", '''
from neoconstruct import StructMixin, field, Int32ub, Int8ub, Array
from dataclasses import dataclass
@dataclass
class P(StructMixin):
    count: int = field(Int32ub)
    items: list = field(Array(count, Array(1000, Int8ub)))
try:
    r = P.parse((200000).to_bytes(4, "big") + b"\\x00" * 50)
    print("parsed", len(r.items))
except Exception as e:
    print("raised:", type(e).__name__, str(e)[:90])
'''], capture_output=True, text=True, timeout=120)
print("nested huge:", (r.stdout + r.stderr).strip()[:200], "rc=", r.returncode)

print()
print("=== X. Array with LITERAL huge constant (compile-time) ===")
r = subprocess.run([sys.executable, "-c", '''
from neoconstruct import StructMixin, field, Int8ub, Array
from dataclasses import dataclass
@dataclass
class P(StructMixin):
    items: list = field(Array(4294967295, Int8ub))
try:
    r = P.parse(b"\\x00" * 10)
    print("parsed", len(r.items))
except Exception as e:
    print("raised:", type(e).__name__, str(e)[:90])
'''], capture_output=True, text=True, timeout=120)
print("literal huge:", (r.stdout + r.stderr).strip()[:200], "rc=", r.returncode)

print()
print("=== Y. build path with fieldref count huge ===")
r = subprocess.run([sys.executable, "-c", '''
from neoconstruct import StructMixin, field, Int32ub, Int8ub, Array
from dataclasses import dataclass
@dataclass
class P(StructMixin):
    count: int = field(Int32ub)
    items: list = field(Array(count, Int8ub))
try:
    b = P(count=4294967295, items=[]).build()
    print("built", len(b))
except Exception as e:
    print("raised:", type(e).__name__, str(e)[:120])
'''], capture_output=True, text=True, timeout=120)
print("build count/list mismatch huge:", (r.stdout + r.stderr).strip()[:220], "rc=", r.returncode)

print()
print("=== Z. build count/list mismatch (small) ===")
@dataclass
class P2(StructMixin):
    count: int = field(Int8ub)
    items: list = field(Array(count, Int8ub))
try:
    print(P2(count=3, items=[1, 2]).build())
except Exception as e:
    print(f"count=3 items=2: {type(e).__name__}: {str(e)[:140]}")

print()
print("=== AA. kw_only default ordering ===")
try:
    @dataclass
    class P3(StructMixin):
        a: int = field(Int8ub, default=1)
        b: int = field(Int8ub)     # non-default after default
    print("P3 OK:", P3(b=2))
except Exception as e:
    print(f"ordering: {type(e).__name__}: {str(e)[:160]}")
