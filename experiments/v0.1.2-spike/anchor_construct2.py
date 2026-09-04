"""锚点补充：各构造器实例的 flagbuildnone 属性 + Tell 行为 + 嵌套 expr ctx 语义。"""
from construct import (
    Padding, Tell, Pass, Bytes, Byte, Const, Default, Rebuild, Computed,
    Prefixed, Int8ub, Struct, this,
)

for label, c in [
    ("Padding(2)", Padding(2)),
    ("Tell", Tell),
    ("Pass", Pass),
    ("Bytes(2)", Bytes(2)),
    ("Byte", Byte),
    ("Const(5,Byte)", Const(5, Byte)),
    ("Default(Byte,0)", Default(Byte, 0)),
    ("Rebuild(Byte, lambda c: 1)", Rebuild(Byte, lambda c: 1)),
    ("Computed(42)", Computed(42)),
    ("Prefixed(Int8ub, Bytes(2))", Prefixed(Int8ub, Bytes(2))),
]:
    print("%-40r flagbuildnone=%s" % (label, c.flagbuildnone))

print()
print("L1 Tell parse:", Struct("a" / Tell, "x" / Byte, "b" / Tell).parse(b"\x05"))
print("L1 Tell build:", Struct("a" / Tell, "x" / Byte, "b" / Tell).build({"x": 5}))

# 嵌套 Prefixed 内表达式的 ctx 读取时机：表达式在 parse 时读的 x 是外层已 parse 的值
print()
print("E7 双层嵌套 Prefixed(Int8ub, Prefixed(Int8ub, Bytes(this.x+1))) parse:",
      Struct("x" / Byte, "y" / Prefixed(Int8ub, Prefixed(Int8ub, Bytes(this.x + 1)))).parse(b"\x01\x03\x02ab"))
print("E7 build:",
      Struct("x" / Byte, "y" / Prefixed(Int8ub, Prefixed(Int8ub, Bytes(this.x + 1)))).build({"x": 1, "y": b"ab"}))

# build 时 context.update(obj)：表达式可以读到「用户给的值」而非「已 build 的值」吗？
# 验证：Switch key 用 this.x，x 字段本身也 build —— ctx 读取顺序
from construct import Switch
print()
print("N1 Switch(this.x) build（x 键存在）:",
      Struct("x" / Byte, "s" / Switch(this.x, {1: Byte, 2: Bytes(2)})).build({"x": 1, "s": 9}))
# x 键缺失但 ctx 先 update 了 obj：Switch 读 this.x 时读到什么？
try:
    print("N2 Switch(this.x) build（x 键缺失）:",
          Struct("x" / Byte, "s" / Switch(this.x, {1: Byte, 2: Bytes(2)})).build({"s": 9}))
except Exception as e:
    print("N2 ERR:", type(e).__name__, str(e)[:120])
