# BUG-2: Const / Default 作为 field 时 dataclass 无 default -> P() 报 missing arguments
import traceback
from dataclasses import dataclass
from construct import StructMixin, field, Int8ub, Const, Default

print("=== Const case ===")
try:
    @dataclass
    class PC(StructMixin):
        v: int = field(Const(5))
    print("[compile] OK:", PC)
    try:
        obj = PC()
        print("[instantiate] PC() OK:", obj)
        try:
            print("[build] PC().build() =", obj.build())
        except Exception:
            print("[build] PC().build() FAILED:")
            traceback.print_exc()
    except TypeError:
        print("[instantiate] PC() FAILED with TypeError:")
        traceback.print_exc()
    # 显式传值 / parse 行为
    try:
        print("[parse] PC.parse(b'\\x05') =", PC.parse(b"\x05"))
        print("[build] PC(v=5).build() =", PC(v=5).build())
    except Exception:
        print("[parse/build] FAILED:")
        traceback.print_exc()
except Exception:
    print("[compile] __init_subclass__ FAILED:")
    traceback.print_exc()

print("\n=== Default case ===")
try:
    @dataclass
    class PD(StructMixin):
        v: int = field(Default(Int8ub, 9))
    print("[compile] OK:", PD)
    try:
        obj = PD()
        print("[instantiate] PD() OK:", obj)
        try:
            print("[build] PD().build() =", obj.build())
        except Exception:
            print("[build] PD().build() FAILED:")
            traceback.print_exc()
    except TypeError:
        print("[instantiate] PD() FAILED with TypeError:")
        traceback.print_exc()
    try:
        print("[parse] PD.parse(b'\\x07') =", PD.parse(b"\x07"))
    except Exception:
        print("[parse] FAILED:")
        traceback.print_exc()
except Exception:
    print("[compile] __init_subclass__ FAILED:")
    traceback.print_exc()

# 混合场景：Const 字段 + 普通字段
print("\n=== mixed: Const + normal field ===")
try:
    @dataclass
    class PM(StructMixin):
        v: int = field(Const(5))
        n: int = field(Int8ub)
    try:
        print("[instantiate] PM(n=1) OK:", PM(n=1))
    except TypeError:
        print("[instantiate] PM(n=1) FAILED:")
        traceback.print_exc()
except Exception:
    print("[compile] FAILED:")
    traceback.print_exc()
