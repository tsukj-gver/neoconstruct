"""Hex/HexDump 显示类构造开销 profiling（ARCH 根因验证）。

目的：量化 Hex parse 路径各步骤开销，验证以下假设：
  H1: HexDisplayedInteger.new (Python 字节码) 比直接 C 级构造 (cls(intvalue)+setattr) 贵
  H2: int 子类 setattr (obj.fmtstr=...) 是 CPython 固有开销
  H3: bytes 路径 (HexDisplayedBytes(bytes)) 比 int 路径轻（无 setattr）
  H4: getattr(cls, "new") + staticmethod 调度开销可观

说明：本脚本在 Python 层测量（不跨 FFI）。绝对值与 Rust 跨 FFI 调用不同，
但各步骤的相对比例可外推到 Rust 侧（pyo3 call_method1 内部走相同的 C API 路径，
仅多一次 FFI 边界）。结论标注"量级参考"（L-09 教训）。

Python 版本：见运行时输出（与项目 venv 可能不同，仅影响绝对值，不影响比例）。
"""

import gc
import platform
import sys
import timeit


# ---------------------------------------------------------------------------
# 显示类定义（复制自 construct/lib/hex.py，去掉 py3compat 依赖）
# ---------------------------------------------------------------------------

class HexDisplayedInteger(int):
    """Used internally."""

    def __str__(self):
        return "0x" + format(self, self.fmtstr).upper()

    @staticmethod
    def new(intvalue, fmtstr):
        obj = HexDisplayedInteger(intvalue)
        obj.fmtstr = fmtstr
        return obj


class HexDisplayedBytes(bytes):
    """Used internally."""

    def __str__(self):
        if not hasattr(self, "render"):
            import binascii
            self.render = "unhexlify(%r)" % (binascii.hexlify(self).decode(),)
        return self.render


# ---------------------------------------------------------------------------
# 被测函数
# ---------------------------------------------------------------------------

FMT = "08X"
INTVALUE = 258  # 超出小整数缓存（-5..256），确保 PyLong 分配路径
BYTESVALUE = b'\x00\x00\x01\x02'


def new_full():
    """完整 new 调用（Python 字节码路径）— 对应 Rust call_method1('new', ...)."""
    HexDisplayedInteger.new(INTVALUE, FMT)


def new_decomposed():
    """拆解为 C 级：cls(intvalue) + setattr（去掉 staticmethod getattr + 函数帧）.

    对应方案 A：Rust 内 cls.call1((intvalue,)) + setattr.
    """
    obj = HexDisplayedInteger(INTVALUE)
    obj.fmtstr = FMT


def getattr_only():
    """仅 getattr(cls, 'new') — 验证 staticmethod 描述符查找开销."""
    getattr(HexDisplayedInteger, "new")


def int_subclass_no_setattr():
    """仅 int 子类实例化（无 setattr）— 验证 int 子类 __new__ 基线."""
    HexDisplayedInteger(INTVALUE)


def bytes_subclass():
    """bytes 子类实例化（对照 int 路径）— 对应 HX2/HD1."""
    HexDisplayedBytes(BYTESVALUE)


def plain_int():
    """普通 int() 构造 — 对照基线."""
    int(INTVALUE)


def plain_bytes():
    """普通 bytes() 构造 — 对照基线."""
    bytes(BYTESVALUE)


def setattr_on_int_subclass():
    """对已构造的 int 子类 setattr — 单独测 setattr 开销.

    需复用实例避免被实例化开销淹没。每次 setattr 不同 key 强制写入。
    """
    obj = HexDisplayedInteger(INTVALUE)
    obj.fmtstr = FMT  # 预热（创建 __dict__）


# ---------------------------------------------------------------------------
# 运行基准
# ---------------------------------------------------------------------------

def bench(func, number=200000, repeat=7):
    gc.collect()
    times = timeit.repeat(func, number=number, repeat=repeat)
    best = min(times) / number * 1e9  # ns/op
    return best


CASES = [
    ("plain_int",                plain_int,                "普通 int() 构造（基线）"),
    ("plain_bytes",              plain_bytes,              "普通 bytes() 构造（基线）"),
    ("int_subclass_no_setattr",  int_subclass_no_setattr,  "int 子类实例化（无 setattr）"),
    ("bytes_subclass",           bytes_subclass,           "HexDisplayedBytes(bytes) [HX2/HD1 路径]"),
    ("new_decomposed",           new_decomposed,           "cls(intvalue)+setattr [方案A Rust 内联]"),
    ("new_full",                 new_full,                 "HexDisplayedInteger.new [当前 Rust call_method1]"),
    ("getattr_only",             getattr_only,             "getattr(cls,'new') [方案A 可省]"),
]


def main():
    print("=" * 78)
    print("Hex 显示类构造开销 profiling")
    print(f"Python {sys.version.split()[0]} | {platform.machine()} | {platform.system()}")
    print(f"number=200000, repeat=7 (取 best ns/op)")
    print("=" * 78)
    print()
    print(f"{'Case':<28} {'ns/op':>9}  说明")
    print("-" * 78)

    results = {}
    for name, func, desc in CASES:
        ns = bench(func)
        results[name] = ns
        print(f"{name:<28} {ns:>8.1f}  {desc}")

    print()
    print("=" * 78)
    print("关键差异分析")
    print("=" * 78)

    def show(label, a, b, extra=""):
        delta = results[a] - results[b]
        ratio = results[a] / results[b] if results[b] else float('inf')
        print(f"{label}")
        print(f"  {a:.<26} {results[a]:>7.1f} ns")
        print(f"  {b:.<26} {results[b]:>7.1f} ns")
        print(f"  delta = {delta:>+7.1f} ns  ({ratio:.2f}x)  {extra}")
        print()
        return delta

    # H1: new_full vs new_decomposed = getattr + 字节码调度（方案A 可省部分）
    d1 = show("H1: new 完整 vs 拆解 = staticmethod 调度开销（方案 A 可省）",
              "new_full", "new_decomposed",
              "← 这部分是 Rust 改 call_method1→call1+setattr 能省的")

    # H2: int 子类 setattr 开销（CPython 固有）
    setattr_cost = results["new_decomposed"] - results["int_subclass_no_setattr"]
    print(f"H2: int 子类 setattr 开销（obj.fmtstr=...）")
    print(f"  new_decomposed - int_subclass_no_setattr = {setattr_cost:>+7.1f} ns")
    print(f"  ← CPython 固有（int 子类 __dict__ 写入），方案 A 无法省")
    print()

    # int 子类实例化开销（CPython 固有）
    inst_cost = results["int_subclass_no_setattr"] - results["plain_int"]
    print(f"int 子类实例化 vs 普通 int（CPython 固有）")
    print(f"  int_subclass_no_setattr - plain_int = {inst_cost:>+7.1f} ns")
    print()

    # H3: bytes 路径 vs int 路径
    d3 = show("H3: bytes 子类 vs int 子类实例化（HX2 vs HX1 差距来源）",
              "int_subclass_no_setattr", "bytes_subclass",
              "← HX1 比 HX2 多的固定开销之一")

    # H4: getattr 单独开销
    print(f"H4: getattr(cls, 'new') 单独开销 = {results['getattr_only']:.1f} ns")
    print(f"  ← call_method1 内部隐含此步；call1 不需要")
    print()

    print("=" * 78)
    print("外推到 Rust 侧（HX1 = 581ns 实测）")
    print("=" * 78)
    print()
    print("Rust call_method1('new',...) 比 Python 直接 new() 多 1 次 FFI 边界开销，")
    print("但 C API 路径相同（PyObject_GetAttr + PyObject_Call）。方案 A 预测节省：")
    print(f"  省掉 staticmethod 调度（H1 delta）≈ {d1:.0f} ns（Python 层比例）")
    print(f"  外推 Rust 侧：HX1 581ns 中约 {d1/max(results['new_full'],1)*100:.0f}% 来自调度开销")
    print(f"  方案 A 预测 HX1 优化后 ≈ 581 * (1 - {d1/max(results['new_full'],1):.2f}) ≈ "
          f"{581*(1-d1/max(results['new_full'],1)):.0f} ns")
    print()
    print("注：以上为量级参考（L-09），实际需 Rust 实验/复测验证。")


if __name__ == "__main__":
    main()
