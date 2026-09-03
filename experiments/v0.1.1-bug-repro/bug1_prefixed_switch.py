# BUG-1: Switch 被 Prefixed 包装后，keyfunc 引用根 Struct 字段
import traceback
from dataclasses import dataclass
from typing import Any
from construct import StructMixin, field, Int8ub, Prefixed, Switch, Bytes

print("=== defining class P (typ + br: Prefixed(Int8ub, Switch(typ, ...))) ===")
try:
    @dataclass
    class P(StructMixin):
        typ: int = field(Int8ub)
        br: Any = field(Prefixed(Int8ub, Switch(typ, {1: Int8ub, 2: Bytes(2)})))
    print("[compile] __init_subclass__ OK:", P)
except Exception:
    print("[compile] FAILED at __init_subclass__:")
    traceback.print_exc()
    raise SystemExit(1)

# typ=1 -> body 是 Int8ub, Prefixed 长度=1
print("\n=== parse b'\\x01\\x01\\x7f' (typ=1, len=1, val=127) ===")
try:
    p = P.parse(b"\x01\x01\x7f")
    print("[parse] OK:", p)
except Exception:
    print("[parse] FAILED:")
    traceback.print_exc()

print("\n=== build P(typ=1, br=127) ===")
try:
    b = P(typ=1, br=127).build()
    print("[build] OK:", b)
except Exception:
    print("[build] FAILED:")
    traceback.print_exc()

# typ=2 -> body 是 Bytes(2)
print("\n=== parse b'\\x02\\x02AB' (typ=2, len=2, b'AB') ===")
try:
    p = P.parse(b"\x02\x02AB")
    print("[parse] OK:", p)
except Exception:
    print("[parse] FAILED:")
    traceback.print_exc()

print("\n=== build P(typ=2, br=b'AB') ===")
try:
    b = P(typ=2, br=b"AB").build()
    print("[build] OK:", b)
except Exception:
    print("[build] FAILED:")
    traceback.print_exc()

# 对照：不带 Prefixed 包装的裸 Switch 是否正常
print("\n=== control: bare Switch(typ, ...) no Prefixed ===")
from construct import Struct
try:
    @dataclass
    class P2(StructMixin):
        typ: int = field(Int8ub)
        br: Any = field(Switch(typ, {1: Int8ub, 2: Bytes(2)}))
    p2 = P2.parse(b"\x01\x7f")
    print("[parse] OK:", p2)
    b2 = P2(typ=1, br=127).build()
    print("[build] OK:", b2)
except Exception:
    print("[control] FAILED:")
    traceback.print_exc()
