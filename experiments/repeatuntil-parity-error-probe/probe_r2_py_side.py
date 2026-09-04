"""探针：py 侧（原版 construct 2.10.70）R2 新形态验证。

R2 重定义为"哨兵来自前序字段"：
    pc.Struct("stop"/pc.Int8ub,
              "items"/pc.RepeatUntil(lambda x, l, ctx: x == ctx.stop, pc.Int8ub))
数据 bytes([5, 1, 2, 5, 9])，期望 stop=5, items=[1, 2, 5]。
"""

import sys
import traceback

CRS_PYTHON_DIR = r"D:\Project\Github\neoconstruct\neoconstruct\python"
sys.path[:] = [p for p in sys.path if p != CRS_PYTHON_DIR]

import construct as pc  # noqa: E402


def probe_r2_py():
    d = pc.Struct(
        "stop" / pc.Int8ub,
        "items" / pc.RepeatUntil(lambda x, l, ctx: x == ctx.stop, pc.Int8ub),
    )
    parsed = d.parse(bytes([5, 1, 2, 5, 9]))
    print("  stop =", parsed["stop"], "items =", list(parsed["items"]))
    built = d.build(dict(stop=5, items=[1, 2, 5]))
    print("  build bytes =", built.hex())
    reparsed = d.parse(built)
    print("  roundtrip   =", list(reparsed["items"]))


if __name__ == "__main__":
    print("===== probe_r2_py =====")
    try:
        probe_r2_py()
        print("  [OK]")
    except Exception:
        for line in traceback.format_exc().splitlines()[-8:]:
            print("  " + line)
