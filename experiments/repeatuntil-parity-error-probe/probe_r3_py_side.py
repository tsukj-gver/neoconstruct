"""探针：py 侧（原版 construct 2.10.70）R3 哨兵修正验证。

R3 谓词修正为 `x == -1`（在哨兵 -1 处停止，与 case 描述"哨兵 -1"对齐）：
数据 bytes([1, 2, 3, 0xFF, 9])，期望 items=[1, 2, 3, -1]。
"""

import sys
import traceback

CRS_PYTHON_DIR = r"D:\Project\Github\neoconstruct\neoconstruct\python"
sys.path[:] = [p for p in sys.path if p != CRS_PYTHON_DIR]

import construct as pc  # noqa: E402


def probe_r3_py():
    d = pc.Struct(
        "items" / pc.RepeatUntil(lambda x, l, ctx: x == -1, pc.Int8sb),
    )
    parsed = d.parse(bytes([1, 2, 3, 0xFF, 9]))
    print("  items =", list(parsed["items"]))
    built = d.build(dict(items=[1, 2, 3, -1]))
    print("  build bytes =", built.hex())
    reparsed = d.parse(built)
    print("  roundtrip   =", list(reparsed["items"]))


if __name__ == "__main__":
    print("===== probe_r3_py =====")
    try:
        probe_r3_py()
        print("  [OK]")
    except Exception:
        for line in traceback.format_exc().splitlines()[-8:]:
            print("  " + line)
