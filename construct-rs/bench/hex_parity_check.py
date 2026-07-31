"""Quick parity check for HX1/HX2/HD1 after Plan A+B optimization.

Verifies that the display objects produced by optimized parse are behaviorally
identical to the pre-optimization version (str, repr, fmtstr attribute, type).
"""

from dataclasses import dataclass
from construct import (
    StructMixin, field, Hex, HexDump,
    Int8ub, Int16ub, Int32ub, Int64ub, Bytes,
)

# --- HX1: Hex(Int32ub) parse ---
@dataclass
class S_HX1(StructMixin):
    v: object = field(Hex(Int32ub))

data_hx1 = b'\x00\x00\x01\x02'
result_hx1 = S_HX1.parse(data_hx1)
v_hx1 = result_hx1.v

print("=== HX1: Hex(Int32ub) parse ===")
print(f"  value:    {v_hx1}")
print(f"  int:      {int(v_hx1)}")
print(f"  str:      {str(v_hx1)}")
print(f"  repr:     {repr(v_hx1)}")
print(f"  type:     {type(v_hx1).__name__}")
print(f"  fmtstr:   {getattr(v_hx1, 'fmtstr', 'MISSING')}")
print(f"  hex:      {hex(v_hx1)}")

assert int(v_hx1) == 258, f"Expected 258, got {int(v_hx1)}"
assert str(v_hx1) == "0x00000102", f"str mismatch: {str(v_hx1)}"
assert hasattr(v_hx1, 'fmtstr'), "fmtstr attribute missing"
assert v_hx1.fmtstr == "08X", f"fmtstr mismatch: {v_hx1.fmtstr}"
assert type(v_hx1).__name__ == "HexDisplayedInteger", f"type mismatch: {type(v_hx1).__name__}"

# --- HX2: Hex(Bytes(4)) parse ---
@dataclass
class S_HX2(StructMixin):
    v: object = field(Hex(Bytes(4)))

data_hx2 = b'\x00\x00\x01\x02'
result_hx2 = S_HX2.parse(data_hx2)
v_hx2 = result_hx2.v

print("\n=== HX2: Hex(Bytes(4)) parse ===")
print(f"  value:    {v_hx2}")
print(f"  str:      {str(v_hx2)}")
print(f"  repr:     {repr(v_hx2)}")
print(f"  type:     {type(v_hx2).__name__}")
print(f"  bytes:    {bytes(v_hx2)}")

assert bytes(v_hx2) == b'\x00\x00\x01\x02', f"bytes mismatch"
assert str(v_hx2) == "unhexlify('00000102')", f"str mismatch: {str(v_hx2)}"
assert type(v_hx2).__name__ == "HexDisplayedBytes", f"type mismatch: {type(v_hx2).__name__}"

# --- HD1: HexDump(Bytes(4)) parse ---
@dataclass
class S_HD1(StructMixin):
    v: object = field(HexDump(Bytes(4)))

data_hd1 = b'\x00\x00\x01\x02'
result_hd1 = S_HD1.parse(data_hd1)
v_hd1 = result_hd1.v

print("\n=== HD1: HexDump(Bytes(4)) parse ===")
print(f"  value:    {v_hd1}")
print(f"  str:      {str(v_hd1)}")
print(f"  repr:     {repr(v_hd1)}")
print(f"  type:     {type(v_hd1).__name__}")
print(f"  bytes:    {bytes(v_hd1)}")

assert bytes(v_hd1) == b'\x00\x00\x01\x02', f"bytes mismatch"
assert type(v_hd1).__name__ == "HexDumpDisplayedBytes", f"type mismatch: {type(v_hd1).__name__}"

# --- Build round-trip ---
built_hx1 = result_hx1.build()
print(f"\n=== Build round-trip ===")
print(f"  HX1 build: {built_hx1!r}")
assert built_hx1 == data_hx1, f"HX1 build mismatch: {built_hx1!r}"

built_hx2 = result_hx2.build()
print(f"  HX2 build: {built_hx2!r}")
assert built_hx2 == data_hx2, f"HX2 build mismatch: {built_hx2!r}"

built_hd1 = result_hd1.build()
print(f"  HD1 build: {built_hd1!r}")
assert built_hd1 == data_hd1, f"HD1 build mismatch: {built_hd1!r}"

# --- Different sizes for fmtstr ---
print("\n=== fmtstr size variants ===")
for name, subcon, data, expected_fmt in [
    ("Int8ub", Int8ub, b'\x42', "02X"),
    ("Int16ub", Int16ub, b'\x00\x42', "04X"),
    ("Int32ub", Int32ub, b'\x00\x00\x00\x42', "08X"),
    ("Int64ub", Int64ub, b'\x00\x00\x00\x00\x00\x00\x00\x42', "016X"),
]:
    @dataclass
    class S_Size(StructMixin):
        v: object = field(Hex(subcon))

    r = S_Size.parse(data)
    assert r.v.fmtstr == expected_fmt, f"{name}: expected fmtstr {expected_fmt}, got {r.v.fmtstr}"
    print(f"  Hex({name}): fmtstr={r.v.fmtstr}, value={int(r.v)}, str={str(r.v)}")

print("\n✅ ALL PARITY CHECKS PASSED")
