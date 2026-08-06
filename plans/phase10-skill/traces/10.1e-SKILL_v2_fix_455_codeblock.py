"""Fix §4.5.5 code block: Prefixed must be wrapped in @dataclass (descriptor).

The original v2 patch used `d_a = Prefixed(...); d_a.build(payload)` which fails
with AttributeError. Fix to the idiomatic @dataclass wrapping pattern that
matches the rest of the SKILL (and the verified byte values stay identical).
"""
import sys
from pathlib import Path

SKILL_PATH = Path(__file__).resolve().parents[3] / ".opencode" / "skills" / "construct-rs-usage" / "SKILL.md"

OLD_BLOCK = """```python
from construct import Prefixed, Int16ub, GreedyBytes

payload = b"\\xAA\\xBB\\xCC"

# ─── 模式 A：includelength=False（默认）───
# length 字段值 = subcon 字节数 = 3
d_a = Prefixed(Int16ub, GreedyBytes)
built_a = d_a.build(payload)
# built_a = b"\\x00\\x03" (Int16ub=3) + b"\\xAA\\xBB\\xCC"
assert built_a == b"\\x00\\x03\\xAA\\xBB\\xCC"
assert d_a.parse(built_a) == payload

# ─── 模式 B：includelength=True ───
# length 字段值 = subcon 字节数 + sizeof(Int16ub) = 3 + 2 = 5
d_b = Prefixed(Int16ub, GreedyBytes, includelength=True)
built_b = d_b.build(payload)
# built_b = b"\\x00\\x05" (Int16ub=5) + b"\\xAA\\xBB\\xCC"
assert built_b == b"\\x00\\x05\\xAA\\xBB\\xCC"
assert d_b.parse(built_b) == payload
```"""

NEW_BLOCK = """```python
from dataclasses import dataclass
from construct import StructMixin, field, Prefixed, Int16ub, GreedyBytes

payload = b"\\xAA\\xBB\\xCC"

# ─── 模式 A：includelength=False（默认）───
# length 字段值 = subcon 字节数 = 3
@dataclass
class ChunkA(StructMixin):
    data: bytes = field(Prefixed(Int16ub, GreedyBytes))

a = ChunkA(data=payload)
built_a = a.build()
# built_a = b"\\x00\\x03" (Int16ub=3) + b"\\xAA\\xBB\\xCC"
assert built_a == b"\\x00\\x03\\xAA\\xBB\\xCC"
assert ChunkA.parse(built_a).data == payload

# ─── 模式 B：includelength=True ───
# length 字段值 = subcon 字节数 + sizeof(Int16ub) = 3 + 2 = 5
@dataclass
class ChunkB(StructMixin):
    data: bytes = field(Prefixed(Int16ub, GreedyBytes, includelength=True))

b = ChunkB(data=payload)
built_b = b.build()
# built_b = b"\\x00\\x05" (Int16ub=5) + b"\\xAA\\xBB\\xCC"
assert built_b == b"\\x00\\x05\\xAA\\xBB\\xCC"
assert ChunkB.parse(built_b).data == payload
```"""


def main():
    text = SKILL_PATH.read_text(encoding="utf-8")
    count = text.count(OLD_BLOCK)
    if count == 0:
        # Idempotent: check if already fixed
        if NEW_BLOCK in text:
            print("[SKIP] §4.5.5 code block already fixed")
            return
        print("[FAIL] §4.5.5 old code block not found", file=sys.stderr)
        sys.exit(1)
    if count > 1:
        print("[FAIL] §4.5.5 old code block found {} times".format(count), file=sys.stderr)
        sys.exit(1)
    text = text.replace(OLD_BLOCK, NEW_BLOCK, 1)
    SKILL_PATH.write_text(text, encoding="utf-8")
    delta = len(NEW_BLOCK) - len(OLD_BLOCK)
    print("[OK] §4.5.5 code block fixed to @dataclass wrapping ({:+d} chars)".format(delta))


if __name__ == "__main__":
    main()
