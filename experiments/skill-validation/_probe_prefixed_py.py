"""Probe: does Python construct 2.10.70 Prefixed(includelength=True)
match construct-rs Prefixed(includelength=True) semantics?

construct-rs (verified by _probe_api.py Probe1):
  Prefixed(Int16ub, X, includelength=True):
    read Int16ub = N, substream = N - 2 bytes (length includes lengthfield size)
"""
from construct import Struct, Int16ub, Int8ub, Bytes, Prefixed, GreedyBytes

# Inner: type(1) + flags(1) + payload(N bytes)
inner = Struct(
    "type_byte" / Int8ub,
    "flag_byte" / Int8ub,
    "payload" / GreedyBytes,
)

# Test 1: includelength=False (default)
print("=== Test includelength=False ===")
outer_default = Prefixed(Int16ub, inner)  # default includelength=False
# Build with type=0xAB, flags=0xCD, payload=0x1122
# Expect substream length = 4 bytes (type+flags+payload), so length_value = 4
# Wire: 0004 ABCD 1122
built_default = outer_default.build(dict(type_byte=0xAB, flag_byte=0xCD, payload=b"\x11\x22"))
print(f"  built={built_default.hex()}")
parsed_default = outer_default.parse(b"\x00\x04\xAB\xCD\x11\x22")
print(f"  parse(0004ABCD1122): type=0x{parsed_default.type_byte:02X} "
      f"flag=0x{parsed_default.flag_byte:02X} payload={parsed_default.payload.hex()}")

# Test 2: includelength=True
print("\n=== Test includelength=True ===")
outer_incl = Prefixed(Int16ub, inner, includelength=True)
built_incl = outer_incl.build(dict(type_byte=0xAB, flag_byte=0xCD, payload=b"\x11\x22"))
print(f"  built={built_incl.hex()}")
# Try parsing construct-rs format: chunk_length=6 (whole chunk), wire = 0006 ABCD 1122
try:
    parsed_incl = outer_incl.parse(b"\x00\x06\xAB\xCD\x11\x22")
    print(f"  parse(0006ABCD1122): type=0x{parsed_incl.type_byte:02X} "
          f"flag=0x{parsed_incl.flag_byte:02X} payload={parsed_incl.payload.hex()}")
except Exception as e:
    print(f"  parse(0006ABCD1122) FAILED: {type(e).__name__}: {e}")
