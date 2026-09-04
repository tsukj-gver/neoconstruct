"""v0.1.2-1 现状对照：同一矩阵在当前 neoconstruct 上的行为（B1-B4 形态记录）。"""
from dataclasses import dataclass

from neoconstruct import (
    Byte, Bytes, Int8ub, Const, Default, Rebuild, Computed, Padding, Tell,
    Prefixed, StructMixin, field, rfield, wfield,
)


def show(label, fn):
    try:
        r = fn()
        print(f"[OK ] {label}: {r!r}")
    except Exception as e:
        print(f"[ERR] {label}: {type(e).__name__}: {str(e)[:130]}")


# ---- Const 实例化值（B1）
@dataclass
class C2(StructMixin):
    c: int = field(Const(0x05, Int8ub))


show("B1a C2().c 实例化值", lambda: C2().c)
show("B1b C2().build()", lambda: C2().build())

# ---- Const x + Bytes(x+1)（B1 流入表达式）


@dataclass
class A(StructMixin):
    x: int = field(Const(1, Byte))
    y: bytes = field(Bytes(x + 1))


show("B1c A() 实例化", lambda: A())
show("B1d A(y=b'ab').x（用户补 y 后 x 仍是 None）", lambda: A(y=b"ab").x)
show("B1e A(y=b'ab').build()", lambda: A(y=b"ab").build())

# ---- rfield(Const) 路径


@dataclass
class A2(StructMixin):
    x: int = rfield(Const(1, Byte))
    y: bytes = field(Bytes(x + 1))


show("B1f rfield Const: A2(y=b'ab').build()", lambda: A2(y=b"ab").build())
show("B1g rfield Const: A2().y 实例化", lambda: A2().y)

# ---- B2 嵌套 Prefixed(Int8ub, Bytes(x+1))


@dataclass
class M(StructMixin):
    typ: int = field(Int8ub)
    payload: bytes = field(Prefixed(Int8ub, Bytes(typ + 1)))


show("B2a M.parse(b'\\x01\\x02ab')", lambda: M.parse(b"\x01\x02ab"))
show("B2b M(typ=1, payload=b'ab').build()", lambda: M(typ=1, payload=b"ab").build())

# ---- 双层 Prefixed
@dataclass
class M2(StructMixin):
    typ: int = field(Int8ub)
    payload: bytes = field(Prefixed(Int8ub, Prefixed(Int8ub, Bytes(typ + 1))))


show("B2c M2.parse(b'\\x01\\x03\\x02ab')", lambda: M2.parse(b"\x01\x03\x02\x61\x62"))

# ---- B3 wfield(Bytes(x+1))


@dataclass
class W(StructMixin):
    x: int = field(Int8ub)
    y: bytes = wfield(Bytes(x + 1))


show("B3a W() 实例化（missing argument）", lambda: W())
show("B3b W(x=1).build()（仍缺 y）", lambda: W(x=1).build())
show("B3c W(x=1, y=b'ab').build()", lambda: W(x=1, y=b"ab").build())
show("B3d W.parse(b'\\x01ab')（parse 后 y 不存在）",
     lambda: getattr(W.parse(b"\x01ab"), "y", "<no attr y>"))

# ---- Default 矩阵


@dataclass
class D0(StructMixin):
    d: int = field(Default(Byte, 0))


show("D-a D0().d 实例化值", lambda: D0().d)
show("D-b D0().build()", lambda: D0().build())
show("D-c D0(d=9).build()", lambda: D0(d=9).build())
show("D-d D0.parse(b'\\x07').d", lambda: D0.parse(b"\x07").d)


@dataclass
class D1(StructMixin):
    x: int = field(Byte)
    d: int = field(Default(Byte, x + 1))


show("D-e D1(x=4).build()（expr 默认）", lambda: D1(x=4).build())
show("D-f D1(x=4, d=9).build()（显式优先）", lambda: D1(x=4, d=9).build())

# ---- Rebuild / Computed / Padding / Tell


@dataclass
class R1(StructMixin):
    n: int = field(Byte)
    r: int = rfield(Rebuild(Byte, n * 2))


show("R-a R1(n=3).build()", lambda: R1(n=3).build())
show("R-b R1(n=3, r=99).build()（显式被忽略）", lambda: R1(n=3, r=99).build())
show("R-c R1.parse(b'\\x03\\xff')", lambda: R1.parse(b"\x03\xff"))
show("R-d R1().r 实例化值", lambda: R1().r)


@dataclass
class T1(StructMixin):
    x: int = field(Byte)
    c: int = rfield(Computed(x * 2))


show("T-a T1.parse(b'\\x05').c", lambda: T1.parse(b"\x05").c)
show("T-b T1(x=5).build()", lambda: T1(x=5).build())
show("T-c T1(x=5, c=123).build()（RO 不取实例值）", lambda: T1(x=5, c=123).build())


@dataclass
class P1(StructMixin):
    x: int = field(Byte)
    p: int = wfield(Padding(2))
    y: int = field(Byte)


show("P-a P1(x=1, y=2).build()", lambda: P1(x=1, y=2).build())
show("P-b P1.parse(b'\\x01\\x00\\x00\\x02')（wfield 丢弃）",
     lambda: P1.parse(b"\x01\x00\x00\x02"))
show("P-c P1(x=1, p=b'zz', y=2).build()（给值被忽略）", lambda: P1(x=1, p=b"zz", y=2).build())


@dataclass
class L1(StructMixin):
    a: int = rfield(Tell())
    x: int = field(Byte)
    b: int = rfield(Tell())


show("L-a L1(x=5).build()", lambda: L1(x=5).build())
show("L-b L1.parse(b'\\x05')", lambda: L1.parse(b"\x05"))

# ---- B4 context= 注入（跨层引用）


@dataclass
class Inner(StructMixin):
    n: int = field(Byte)


@dataclass
class OuterB4(StructMixin):
    count: int = field(Int8ub)
    items: object = field(Inner, context={"n": count})


show("B4a context= 参数被接受且编译通过？", lambda: OuterB4)
