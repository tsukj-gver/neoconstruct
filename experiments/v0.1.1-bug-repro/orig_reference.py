# 原版 construct 2.10.70 行为对照
from construct import Struct, Int8ub, Prefixed, Switch, Bytes, Const, Default, this
import construct
print("version:", construct.__version__)

# 1. Prefixed x Switch(this.typ) —— BUG-1 原版对照
d = Struct(
    "typ" / Int8ub,
    "br" / Prefixed(Int8ub, Switch(this.typ, {1: Int8ub, 2: Bytes(2)})),
)
print("parse typ=1:", d.parse(b"\x01\x01\x7f"))
print("parse typ=2:", d.parse(b"\x02\x02AB"))
print("build typ=1:", d.build({"typ": 1, "br": 127}))
print("build typ=2:", d.build({"typ": 2, "br": b"AB"}))

# 1b. 嵌套 Struct 内 Switch(this.typ) —— ctx 隔离对照
d1b = Struct(
    "typ" / Int8ub,
    "inner" / Struct("v" / Switch(this.typ, {1: Int8ub})),
)
try:
    print("nested-struct parse:", d1b.parse(b"\x01\x7f"))
except Exception as e:
    print("nested-struct parse FAILED:", type(e).__name__, str(e)[:80])

# 2. Const(5) 无 subcon —— BUG-2 原版对照
try:
    c = Const(5)
    print("Const(5) subcon:", c.subcon)
    print("Const(5).build(None):", c.build(None))
except Exception as e:
    print("Const(5) FAILED:", type(e).__name__, str(e)[:100])

# 3. Struct 内 Const：build 时缺 key
d2 = Struct("v" / Const(5, Int8ub), "n" / Int8ub)
print("build missing v (Const):", d2.build({"n": 1}))

# 4. Struct 内 Default：build 时缺 key
d3 = Struct("v" / Default(Int8ub, 9), "n" / Int8ub)
print("build missing v (Default):", d3.build({"n": 1}))

# 5. Const(b'...') 无 subcon
c2 = Const(b"SIG")
print("Const(b'SIG').build(None):", c2.build(None))

# 6. Const bool / str 无 subcon
try:
    c3 = Const("AB")
    print("Const('AB') subcon:", c3.subcon, "build:", c3.build(None))
except Exception as e:
    print("Const('AB') FAILED:", type(e).__name__, str(e)[:100])
