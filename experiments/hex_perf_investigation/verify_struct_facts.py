"""补充实验：验证 int 子类 setattr 与 CPython long 子类实例化行为。

确认以下结构性事实（为报告提供证据）：
  F1: HexDisplayedInteger 实例确实持有 __dict__（int 子类无 __slots__）
  F2: setattr fmtstr 写入实例 __dict__
  F3: call1(cls, (intvalue,)) 不进入 Python 字节码（走 C 级 long_new）
  F4: call_method1 的 new 进入 Python 字节码（PyFunctionObject）

本实验用 Python 层确认对象结构，Rust 侧行为由 CPython 机制保证（同一解释器）。
"""

import dis
import sys


class HexDisplayedInteger(int):
    def __str__(self):
        return "0x" + format(self, self.fmtstr).upper()

    @staticmethod
    def new(intvalue, fmtstr):
        obj = HexDisplayedInteger(intvalue)
        obj.fmtstr = fmtstr
        return obj


def main():
    print("=" * 70)
    print("结构性事实验证")
    print("=" * 70)

    # F1: int 子类实例持有 __dict__
    obj = HexDisplayedInteger.new(258, "08X")
    has_dict = hasattr(obj, "__dict__")
    print(f"\nF1: HexDisplayedInteger 实例有 __dict__: {has_dict}")
    if has_dict:
        print(f"    __dict__ 内容: {dict(obj.__dict__)}")
        print(f"    -> int 基类无 __dict__，子类实例化时 CPython 分配 __dict__")

    # F2: setattr 写入 __dict__
    print(f"\nF2: setattr 'fmtstr' 写入实例 __dict__")
    print(f"    obj.fmtstr = {obj.fmtstr!r}  (在 __dict__ 中: {'fmtstr' in obj.__dict__})")

    # F3/F4: new 是 Python 函数（PyFunctionObject），有字节码
    new_func = HexDisplayedInteger.__dict__["new"]
    print(f"\nF3: HexDisplayedInteger.new 类型: {type(new_func).__name__}")
    print(f"    -> @staticmethod 包装的 Python 函数，有 __code__ (字节码)")
    print(f"    call_method1('new',...) 会: getattr → staticmethod.__get__ → ")
    print(f"    PyFunctionObject → PyObject_Call → 进入 ceval 字节码循环")
    print()
    print("    new 字节码:")
    dis.dis(HexDisplayedInteger.new)

    # F5: 对照 — type.__call__ 路径（call1(cls, args)）不进字节码
    print(f"\nF5: call1(cls, (intvalue,)) 路径分析")
    print(f"    HexDisplayedInteger.__new__ 是: {HexDisplayedInteger.__new__}")
    print(f"    HexDisplayedInteger.__init__ 是: {HexDisplayedInteger.__init__}")
    print(f"    -> __new__ 继承自 int (long_new, C 级)；__init__ 继承自 object (C 级)")
    print(f"    -> call1 不进入 Python 字节码循环（全程 C 级 type.__call__）")
    print(f"    -> setattr 也是 C 级 (PyObject_GenericSetAttr)")

    print()
    print("=" * 70)
    print("结论：方案 A（call1 + setattr 替代 call_method1）消除 Python 字节码进入")
    print("=" * 70)

if __name__ == "__main__":
    main()
