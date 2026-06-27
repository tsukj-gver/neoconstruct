"""VET 验证脚本：Python construct BitsInteger 行为基线。

用于对照 construct-rs Phase 3.1 BitsIntegerNode 的实现。
不依赖 construct-rs 扩展模块（仅用 Python 原版 construct）。

运行：python experiments/vet_phase3_bits_integer.py
"""

from construct import Bitwise, BitsInteger, Bit, Nibble, Octet, StreamError, IntegerError


def check(label, got, expected):
    ok = got == expected
    status = "OK" if ok else "FAIL"
    print(f"[{status}] {label}: got={got!r} expected={expected!r}")
    return ok


results = []

# ---------------------------------------------------------------------------
# parse 基本用例
# ---------------------------------------------------------------------------

# 1-bit
results.append(check("Bit parse 0x80 -> 1",
                     Bitwise(Bit).parse(b"\x80"), 1))
results.append(check("Bit parse 0x00 -> 0",
                     Bitwise(Bit).parse(b"\x00"), 0))

# Nibble
results.append(check("Nibble parse 0xA5 high -> 10",
                     Bitwise(Nibble).parse(b"\xA5"), 10))

# Octet
results.append(check("Octet parse 0x42 -> 66",
                     Bitwise(Octet).parse(b"\x42"), 66))

# 12-bit
results.append(check("BitsInteger(12) parse 0xABC0 -> 0xABC",
                     Bitwise(BitsInteger(12)).parse(b"\xAB\xC0"), 0xABC))

# ---------------------------------------------------------------------------
# signed
# ---------------------------------------------------------------------------

results.append(check("BitsInteger(8,signed) parse 0xFF -> -1",
                     Bitwise(BitsInteger(8, signed=True)).parse(b"\xFF"), -1))
results.append(check("BitsInteger(8,signed) parse 0x7F -> 127",
                     Bitwise(BitsInteger(8, signed=True)).parse(b"\x7F"), 127))
results.append(check("BitsInteger(4,signed) parse 0xF0 high -> -1",
                     Bitwise(BitsInteger(4, signed=True)).parse(b"\xF0"), -1))
results.append(check("BitsInteger(4,signed) parse 0x80 high -> -8",
                     Bitwise(BitsInteger(4, signed=True)).parse(b"\x80"), -8))

# ---------------------------------------------------------------------------
# swapped (little-endian byte order)
# ---------------------------------------------------------------------------

# swapped=True: raw BE=0x0100=256, swapbytesinbits -> 0x0001=1
results.append(check("BitsInteger(16,swapped) parse [0x01,0x00] -> 1",
                     Bitwise(BitsInteger(16, swapped=True)).parse(b"\x01\x00"), 1))
# swapped=True 8-bit: no change
results.append(check("BitsInteger(8,swapped) parse 0xAB -> 0xAB",
                     Bitwise(BitsInteger(8, swapped=True)).parse(b"\xAB"), 0xAB))

# ---------------------------------------------------------------------------
# build
# ---------------------------------------------------------------------------

results.append(check("BitsInteger(16,swapped) build 1 -> [0x01,0x00]",
                     Bitwise(BitsInteger(16, swapped=True)).build(1), b"\x01\x00"))

results.append(check("BitsInteger(8) build 0x42 -> [0x42]",
                     Bitwise(BitsInteger(8)).build(0x42), b"\x42"))

results.append(check("BitsInteger(8,signed) build -1 -> [0xFF]",
                     Bitwise(BitsInteger(8, signed=True)).build(-1), b"\xFF"))

# ---------------------------------------------------------------------------
# 错误：length=0
# ---------------------------------------------------------------------------

try:
    Bitwise(BitsInteger(0)).parse(b"\x00")
    results.append(check("BitsInteger(0) parse raises IntegerError", False, True))
except IntegerError:
    results.append(check("BitsInteger(0) parse raises IntegerError", True, True))

# ---------------------------------------------------------------------------
# 错误：swapped + length%8!=0
# ---------------------------------------------------------------------------

try:
    # 注意：Python 先读取 bit，再 swapbytesinbits 报错
    Bitwise(BitsInteger(12, swapped=True)).parse(b"\xAB\xC0")
    results.append(check("BitsInteger(12,swapped) parse raises IntegerError", False, True))
except IntegerError:
    results.append(check("BitsInteger(12,swapped) parse raises IntegerError", True, True))

# ---------------------------------------------------------------------------
# 错误：build 非 int
# ---------------------------------------------------------------------------

try:
    Bitwise(BitsInteger(8)).build("hello")
    results.append(check("BitsInteger(8) build 'hello' raises IntegerError", False, True))
except IntegerError:
    results.append(check("BitsInteger(8) build 'hello' raises IntegerError", True, True))

# ---------------------------------------------------------------------------
# 错误：build 超范围
# ---------------------------------------------------------------------------

try:
    Bitwise(BitsInteger(8)).build(256)
    results.append(check("BitsInteger(8) build 256 raises IntegerError", False, True))
except IntegerError:
    results.append(check("BitsInteger(8) build 256 raises IntegerError", True, True))

try:
    Bitwise(BitsInteger(8)).build(-1)
    results.append(check("BitsInteger(8) build -1 raises IntegerError", False, True))
except IntegerError:
    results.append(check("BitsInteger(8) build -1 raises IntegerError", True, True))

# ---------------------------------------------------------------------------
# 错误：流不足
# ---------------------------------------------------------------------------

try:
    Bitwise(BitsInteger(12)).parse(b"\xFF")
    results.append(check("BitsInteger(12) parse 1 byte raises StreamError", False, True))
except StreamError:
    results.append(check("BitsInteger(12) parse 1 byte raises StreamError", True, True))
except IntegerError:
    # 某些版本可能用 IntegerError 包装
    results.append(check("BitsInteger(12) parse 1 byte raises (IntegerError)", True, True))

# ---------------------------------------------------------------------------
# sizeof
# ---------------------------------------------------------------------------

results.append(check("BitsInteger(8).sizeof() in Bitwise == 1 byte",
                     Bitwise(BitsInteger(8)).sizeof(), 1))
results.append(check("BitsInteger(12).sizeof() in Bitwise",
                     Bitwise(BitsInteger(12)).sizeof(), 2))  # 12 bits -> 2 bytes (Bitwise rounds?)

# Python Bitwise sizeof: subcon.sizeof() // 8 (but 12//8 = 1, not 2...)
# Actually Bitwise uses restreamed, sizeof may differ. Let's check actual.

# ---------------------------------------------------------------------------
# 汇总
# ---------------------------------------------------------------------------

passed = sum(results)
total = len(results)
print(f"\n{'='*60}")
print(f"Passed: {passed}/{total}")
if passed != total:
    print("SOME CHECKS FAILED")
    exit(1)
else:
    print("ALL CHECKS PASSED")
