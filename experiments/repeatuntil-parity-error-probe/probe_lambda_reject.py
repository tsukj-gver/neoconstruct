"""探针：R1-R3 rs 侧子进程失败根因复现。

直接在 CRS venv 中执行 parity case 的 rs 侧定义代码，捕获完整 traceback。
"""

import traceback


def probe_r1_lambda():
    """R1 原始 case 定义：lambda terminator。"""
    from dataclasses import dataclass
    from neoconstruct import StructMixin, field, RepeatUntil, Int8ub

    @dataclass
    class P(StructMixin):
        items: list = field(RepeatUntil(lambda x, lst, ctx: x > 5, Int8ub))


def probe_r2_lambda():
    """R2 原始 case 定义：依赖 list 内容的 lambda。"""
    from dataclasses import dataclass
    from neoconstruct import StructMixin, field, RepeatUntil, Int8ub

    @dataclass
    class P(StructMixin):
        items: list = field(
            RepeatUntil(lambda x, lst, ctx: len(lst) >= 2 and lst[-2] == lst[-1], Int8ub)
        )


def probe_r3_lambda():
    """R3 原始 case 定义：负数哨兵 lambda。"""
    from dataclasses import dataclass
    from neoconstruct import StructMixin, field, RepeatUntil, Int8sb

    @dataclass
    class P(StructMixin):
        items: list = field(RepeatUntil(lambda x, lst, ctx: x != -1, Int8sb))


if __name__ == "__main__":
    for name in ("probe_r1_lambda", "probe_r2_lambda", "probe_r3_lambda"):
        print(f"===== {name} =====")
        try:
            globals()[name]()
            print("  [OK] 编译通过")
        except Exception:
            lines = traceback.format_exc().splitlines()
            # 打印最后 6 行（异常类型 + 消息）
            for line in lines[-6:]:
                print("  " + line)
        print()
