"""Mini fix: add missing blank line before §5.11 (Markdown style).

Run after 10.1b-SKILL_v2_gap_patch.py. Idempotent: aborts if anchor absent.
"""
import sys
from pathlib import Path

SKILL_PATH = Path(__file__).resolve().parents[3] / ".opencode" / "skills" / "construct-rs-usage" / "SKILL.md"


def main():
    text = SKILL_PATH.read_text(encoding="utf-8")
    old = "- `utf16` / `utf_32` / `u16` ✗（无后缀形式被拒绝）\n### 5.11 BitStruct"
    new = "- `utf16` / `utf_32` / `u16` ✗（无后缀形式被拒绝）\n\n### 5.11 BitStruct"
    count = text.count(old)
    # Idempotent: if already fixed (blank line present), the strict anchor won't match
    if count == 0:
        # Check if already fixed
        if "- `utf16` / `utf_32` / `u16` ✗（无后缀形式被拒绝）\n\n### 5.11 BitStruct" in text:
            print("[SKIP] already fixed (blank line present)")
            return
        print("[FAIL] anchor not found and not already fixed", file=sys.stderr)
        sys.exit(1)
    if count > 1:
        print("[FAIL] anchor found {} times".format(count), file=sys.stderr)
        sys.exit(1)
    text = text.replace(old, new, 1)
    SKILL_PATH.write_text(text, encoding="utf-8")
    print("[OK] blank line added before §5.11")


if __name__ == "__main__":
    main()
