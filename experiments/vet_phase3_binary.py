"""VET 验证：直接测试 construct.lib.binary 的工具函数行为。

绕过 Bitwise 包装（Python 3.14 兼容性问题），直接验证：
- bits2integer（signed/unsigned）
- integer2bits（范围检查）
- swapbytesinbits（字节序反序）

这些是 BitsIntegerNode 实现的核心对照。
"""
import sys
sys.path.insert(0, "construct")

from construct.lib.binary import bits2integer, integer2bits, swapbytesinbits


def check(label, got, expected):
    ok = got == expected
    status = "OK" if ok else "FAIL"
    print(f"[{status}] {label}: got={got!r} expected={expected!r}")
    return ok


def check_raises(label, fn, expected_exc):
    try:
        fn()
        print(f"[FAIL] {label}: expected {expected_exc.__name__} but no exception")
        return False
    except expected_exc:
        print(f"[OK] {label}: raised {expected_exc.__name__}")
        return True
    except Exception as e:
        print(f"[FAIL] {label}: expected {expected_exc.__name__} got {type(e).__name__}: {e}")
        return False


r = []

# ---------------------------------------------------------------------------
# bits2integer（对照 Rust read_bits + signed 处理）
# ---------------------------------------------------------------------------

# bit-string 用 bytes（每字节 0 或 1）
r.append(check("bits2integer([1]) -> 1",
               bits2integer(b"\x01"), 1))
r.append(check("bits2integer([0]) -> 0",
               bits2integer(b"\x00"), 0))
r.append(check("bits2integer([1,0,1,0]) -> 10",
               bits2integer(b"\x01\x00\x01\x00"), 10))
r.append(check("bits2integer(8x FF high nibble 1010) -> 0xA",
               bits2integer(bytes([1,0,1,0])), 10))

# signed
r.append(check("bits2integer([1]*8, signed) -> -1",
               bits2integer(bytes([1]*8), signed=True), -1))
r.append(check("bits2integer([0,1,1,1,1,1,1,1], signed) -> 127",
               bits2integer(bytes([0,1,1,1,1,1,1,1]), signed=True), 127))
r.append(check("bits2integer([1,0,0,0], signed) -> -8",
               bits2integer(bytes([1,0,0,0]), signed=True), -8))
r.append(check("bits2integer([1,1,1,1], signed) -> -1",
               bits2integer(bytes([1,1,1,1]), signed=True), -1))

# 64-bit signed min/max
bits_64_min = bytes([1] + [0]*63)  # 0b1000...0 = -2^63
bits_64_max = bytes([0] + [1]*63)  # 0b0111...1 = 2^63-1
r.append(check("bits2integer(64-bit min, signed) -> -9223372036854775808",
               bits2integer(bits_64_min, signed=True), -9223372036854775808))
r.append(check("bits2integer(64-bit max, signed) -> 9223372036854775807",
               bits2integer(bits_64_max, signed=True), 9223372036854775807))

# 64-bit unsigned max
r.append(check("bits2integer(64-bit all 1, unsigned) -> 18446744073709551615",
               bits2integer(bytes([1]*64), signed=False), 18446744073709551615))

# ---------------------------------------------------------------------------
# integer2bits（对照 Rust build 的范围检查 + 二补码）
# ---------------------------------------------------------------------------

r.append(check("integer2bits(19, 8)",
               integer2bits(19, 8), bytes([0,0,0,1,0,0,1,1])))
r.append(check("integer2bits(0, 4)",
               integer2bits(0, 4), bytes([0,0,0,0])))
r.append(check("integer2bits(-1, 8, signed)",
               integer2bits(-1, 8, signed=True), bytes([1]*8)))
r.append(check("integer2bits(-8, 4, signed)",
               integer2bits(-8, 4, signed=True), bytes([1,0,0,0])))
r.append(check("integer2bits(-128, 8, signed)",
               integer2bits(-128, 8, signed=True), bytes([1,0,0,0,0,0,0,0])))

# 范围错误
r.append(check_raises("integer2bits(256, 8) -> ValueError",
                      lambda: integer2bits(256, 8), ValueError))
r.append(check_raises("integer2bits(-1, 8) -> ValueError (unsigned)",
                      lambda: integer2bits(-1, 8), ValueError))
r.append(check_raises("integer2bits(128, 8, signed) -> ValueError",
                      lambda: integer2bits(128, 8, signed=True), ValueError))
r.append(check_raises("integer2bits(0, 0) -> ValueError (width=0)",
                      lambda: integer2bits(0, 0), ValueError))

# ---------------------------------------------------------------------------
# swapbytesinbits（对照 Rust swapbytesinbits_u64）
# ---------------------------------------------------------------------------

# 16 bit: [byte0, byte1] -> [byte1, byte0]
data16 = bytes([0,0,0,0,0,0,0,1,  0,0,0,0,0,0,1,0])  # byte0=0x01, byte1=0x02
# swapbytesinbits -> byte1 first: [0,0,0,0,0,0,1,0, 0,0,0,0,0,0,0,1]
r.append(check("swapbytesinbits(16-bit 0x0102) -> 0x0201 bit-string",
               swapbytesinbits(data16),
               bytes([0,0,0,0,0,0,1,0, 0,0,0,0,0,0,0,1])))

# 8 bit: single byte, no change
data8 = bytes([1,0,1,0,0,1,0,1])  # 0xA5
r.append(check("swapbytesinbits(8-bit) -> unchanged",
               swapbytesinbits(data8), data8))

# 24 bit: [b0, b1, b2] -> [b2, b1, b0]
data24 = bytes([0,0,0,0,0,0,0,1,  0,0,0,0,0,0,1,0,  0,0,0,0,0,1,0,0])
swapped24 = swapbytesinbits(data24)
r.append(check("swapbytesinbits(24-bit) reverses byte groups",
               swapped24,
               bytes([0,0,0,0,0,1,0,0,  0,0,0,0,0,0,1,0,  0,0,0,0,0,0,0,1])))

# %8 != 0 错误
r.append(check_raises("swapbytesinbits(12-bit) -> ValueError",
                      lambda: swapbytesinbits(bytes([1]*12)), ValueError))

# ---------------------------------------------------------------------------
# 综合验证：swapbytesinbits 对 bits2integer 结果的影响
# 对应 Rust: raw = read_bits(n); if swapped { raw = swapbytesinbits_u64(raw, n) }
# ---------------------------------------------------------------------------

# 模拟 BitsInteger(16, swapped=True).parse([0x01, 0x00])
# Python 路径：
#   1. stream_read 读 16 bit -> data = bytes2bits(b'\x01\x00')
#   2. swapbytesinbits(data)
#   3. bits2integer(swapped_data)
from construct.lib.binary import bytes2bits
raw_bytes = b"\x01\x00"
data = bytes2bits(raw_bytes)  # 膨胀为 16 bit 字节串
print(f"\n[INFO] bytes2bits(b'\\x01\\x00') = {data}")
swapped = swapbytesinbits(data)
print(f"[INFO] swapbytesinbits(...) = {swapped}")
result = bits2integer(swapped)
r.append(check("BitsInteger(16,swapped) parse [0x01,0x00] -> 1 (full Python path)",
               result, 1))

# 对比 Rust 路径：
#   raw = read_bits(16) = 0x0100 (MSB-first, byte 0x01 first)
#   swapbytesinbits_u64(0x0100, 16) = ?
#     i=0: byte=(0x0100>>8)&0xFF=0x01, position 0
#     i=1: byte=(0x0100>>0)&0xFF=0x00, position 8
#     result = 0x01 | (0x00<<8) = 0x01
#   value = 0x01 = 1  ✓

# ---------------------------------------------------------------------------
# 汇总
# ---------------------------------------------------------------------------

passed = sum(r)
total = len(r)
print(f"\n{'='*60}")
print(f"Passed: {passed}/{total}")
exit(0 if passed == total else 1)
