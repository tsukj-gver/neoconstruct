# Exploratory session 01: README basics + real-user first hour behaviors
from dataclasses import dataclass
from neoconstruct import StructMixin, field, Int16ub, Int8ub, Bytes

@dataclass
class Header(StructMixin):
    magic:   int   = field(Int16ub)
    version: int   = field(Int8ub)
    payload: bytes = field(Bytes(4))

h = Header(magic=0xCAFE, version=1, payload=b"ABCD")
assert h.build() == b"\xCA\xFE\x01ABCD"
assert Header.parse(b"\xCA\xFE\x01ABCD") == h
print("README example OK")

# --- Real user behaviors ---
# 1. repr of parsed object
p = Header.parse(b"\xCA\xFE\x01ABCD")
print("repr:", repr(p))
print("str:", str(p))

# 2. Equality with a plain dataclass twin? (dataclass eq semantics)
@dataclass
class HeaderTwin:
    magic: int
    version: int
    payload: bytes
print("eq with plain dataclass twin:", p == HeaderTwin(0xCAFE, 1, b"ABCD"))

# 3. What happens with wrong-size input (truncated)?
for bad in [b"", b"\xCA", b"\xCA\xFE", b"\xCA\xFE\x01", b"\xCA\xFE\x01ABC"]:
    try:
        Header.parse(bad)
        print(f"parse({bad!r}): NO ERROR")
    except Exception as e:
        print(f"parse({bad!r}): {type(e).__name__}: {str(e)[:140]}")

# 4. Extra trailing bytes?
try:
    r = Header.parse(b"\xCA\xFE\x01ABCDXXXX")
    print("trailing bytes: parsed OK, result:", r)
except Exception as e:
    print("trailing bytes:", type(e).__name__, str(e)[:140])

# 5. build with wrong types (user typos)
cases = [
    ("negative int", dict(magic=-1, version=1, payload=b"ABCD")),
    ("too big int", dict(magic=0x10000, version=1, payload=b"ABCD")),
    ("str payload", dict(magic=1, version=1, payload="ABCD")),
    ("bytearray payload", dict(magic=1, version=1, payload=bytearray(b"ABCD"))),
    ("memoryview", dict(magic=1, version=1, payload=memoryview(b"ABCD"))),
    ("short payload", dict(magic=1, version=1, payload=b"AB")),
    ("long payload", dict(magic=1, version=1, payload=b"ABCDEF")),
    ("bool version", dict(magic=1, version=True, payload=b"ABCD")),
    ("float version", dict(magic=1, version=1.0, payload=b"ABCD")),
    ("None version", dict(magic=1, version=None, payload=b"ABCD")),
]
for name, kw in cases:
    try:
        b = Header(**kw).build()
        print(f"build {name}: OK -> {b!r}")
    except Exception as e:
        print(f"build {name}: {type(e).__name__}: {str(e)[:120]}")

# 6. missing field on instantiation
try:
    Header(magic=1, version=1)
except Exception as e:
    print("missing payload:", type(e).__name__, str(e)[:100])

# 7. mutate then rebuild (parse -> edit -> build is THE core user loop)
p = Header.parse(b"\xCA\xFE\x01ABCD")
p.version = 2
print("mutate+rebuild:", p.build())

# 8. copy.deepcopy / pickle round-trip (users stash parsed msgs in queues)
import copy, pickle
try:
    print("deepcopy ok:", copy.deepcopy(p) == p)
except Exception as e:
    print("deepcopy FAIL:", type(e).__name__, str(e)[:120])
try:
    print("pickle ok:", pickle.loads(pickle.dumps(p)) == p)
except Exception as e:
    print("pickle FAIL:", type(e).__name__, str(e)[:120])

# 9. Can the class itself be pickled? (multiprocessing workers define protocols at module scope)
try:
    pickle.dumps(Header)
    print("class pickle: OK")
except Exception as e:
    print("class pickle FAIL:", type(e).__name__, str(e)[:120])

# 10. parse wrong input types
for bad_input in [None, "ABCD", 123, [1,2,3], bytearray(b"\xCA\xFE\x01ABCD"), memoryview(b"\xCA\xFE\x01ABCD")]:
    try:
        r = Header.parse(bad_input)
        print(f"parse({type(bad_input).__name__}): OK -> {r!r}")
    except Exception as e:
        print(f"parse({type(bad_input).__name__}): {type(e).__name__}: {str(e)[:100]}")

# 11. int subclasses (Enum/IntEnum are EXTREMELY common in real code)
import enum
class Ver(enum.IntEnum):
    V1 = 1
try:
    b = Header(magic=0xCAFE, version=Ver.V1, payload=b"ABCD").build()
    print("IntEnum build:", b)
except Exception as e:
    print("IntEnum build FAIL:", type(e).__name__, str(e)[:120])
