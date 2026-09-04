"""v0.1.2-2 B2 组合矩阵探针。

矩阵维度：消费者（Bytes/Array(count)/Switch(key)/If(cond)/Computed）
         × 包装器（Prefixed/PrefixedArray/Bitwise/Hex/Select）
         × 表达式形态（字段引用 x / 算术 x+1 / 算术 x*2）

x=1。断言策略：
1. build 字节精确断言（prefix x 字节 + 包装器开销 + inner 数据字节）
2. parse(built) 后再 build == built（roundtrip，暴露静默错读）
3. parsed.x == 1

用法：python matrix_b2.py [--quiet]
退出码：全绿 0，有红 1。
"""
import sys
import traceback

from dataclasses import dataclass
from typing import Any

from neoconstruct import (
    Array, Bitwise, Byte, Bytes, Computed, GreedyBytes, Hex, If, Prefixed,
    PrefixedArray, Select, StructMixin, Switch, field,
)

# ---------------------------------------------------------------------------
# 矩阵定义
# ---------------------------------------------------------------------------

# 表达式形态：名称 → (python 表达式源码, 求值结果 e)
FORMS = {
    "ref_x": ("x", 1),
    "add_x1": ("x + 1", 2),
    "mul_x2": ("x * 2", 2),
}

# 消费者：名称 → (inner subcon 源码模板, build 值, inner 数据字节)
# e = 表达式值；key=1 → Byte 分支，key=2 → Bytes(2) 分支
def consumer_spec(consumer, form_src, e):
    """返回 (inner_subcon_src, build_value, inner_bytes)。"""
    if consumer == "Bytes":
        if e == 1:
            return f"Bytes({form_src})", b"Z", b"Z"
        return f"Bytes({form_src})", b"ab", b"ab"
    if consumer == "Array":
        if e == 1:
            return f"Array({form_src}, Byte)", [0x35], b"\x35"
        return f"Array({form_src}, Byte)", [1, 2], b"\x01\x02"
    if consumer == "Switch":
        if e == 1:
            return (f"Switch({form_src}, {{1: Byte, 2: Bytes(2)}})", 0x7F, b"\x7f")
        return (f"Switch({form_src}, {{1: Byte, 2: Bytes(2)}})", b"AB", b"AB")
    if consumer == "If":
        if e == 0:
            return f"If({form_src}, Byte)", None, b""
        # cond 非零 → Byte 分支
        return f"If({form_src}, Byte)", 0x7F, b"\x7f"
    if consumer == "Computed":
        # 不消费字节；值 = 表达式求值（RW 字段 build 忽略传入值）
        return f"Computed({form_src})", e, b""
    raise ValueError(consumer)


# 包装器：名称 → (包装后 subcon 源码模板, build 值变换, 前缀字节工厂)
# prefix_bytes(inner_bytes) → 包装层引入的字节（在 inner 数据之前）
WRAPPERS = {
    "Prefixed": (
        lambda inner_src: f"Prefixed(Int8ub, {inner_src})",
        lambda v: v,
        lambda ib: bytes([len(ib)]),
    ),
    "PrefixedArray": (
        lambda inner_src: f"PrefixedArray(Int8ub, {inner_src})",
        lambda v: [v],
        lambda ib: b"\x01",
    ),
    "Bitwise": (
        lambda inner_src: f"Bitwise({inner_src})",
        lambda v: v,
        lambda ib: b"",
    ),
    "Hex": (
        lambda inner_src: f"Hex({inner_src})",
        lambda v: v,
        lambda ib: b"",
    ),
    "Select": (
        lambda inner_src: f"Select({inner_src}, GreedyBytes)",
        lambda v: v,
        lambda ib: b"",
    ),
}

CONSUMERS = ["Bytes", "Array", "Switch", "If", "Computed"]

CASE_TMPL = """
@dataclass
class {name}(StructMixin):
    x: int = field(Int8ub)
    c: Any = field({subcon_src})
"""


def build_case(wrapper, consumer, form, form_src=None, e=None):
    """生成并验证一个矩阵单元。返回 (状态, 信息)。"""
    if form_src is None:
        form_src, e = FORMS[form]
    inner_src, inner_val, inner_bytes = consumer_spec(consumer, form_src, e)
    wrap_src, val_xform, prefix_of = WRAPPERS[wrapper]
    subcon_src = wrap_src(inner_src)

    name = "P_{}_{}_{}".format(wrapper, consumer, form)
    ns = dict(
        dataclass=dataclass, StructMixin=StructMixin, field=field, Any=Any,
        Int8ub=Byte, Byte=Byte, Bytes=Bytes, Array=Array, Switch=Switch,
        If=If, Computed=Computed, Prefixed=Prefixed, PrefixedArray=PrefixedArray,
        Bitwise=Bitwise, Hex=Hex, Select=Select, GreedyBytes=GreedyBytes,
    )
    # Int8ub 用单例本名（Byte 是别名，等价）
    from neoconstruct import Int8ub as _Int8ub
    ns["Int8ub"] = _Int8ub
    try:
        exec(CASE_TMPL.format(name=name, subcon_src=subcon_src), ns)
    except Exception as ex:
        return "FAIL", "compile: {}: {}".format(type(ex).__name__, str(ex)[:160])

    cls = ns[name]
    c_val = val_xform(inner_val)
    expected = b"\x01" + prefix_of(inner_bytes) + inner_bytes

    try:
        inst = cls(x=1, c=c_val)
        built = inst.build()
    except Exception as ex:
        return "FAIL", "build: {}: {}".format(type(ex).__name__, str(ex)[:160])
    if built != expected:
        return "FAIL", "build bytes {} != expected {}".format(built, expected)

    try:
        parsed = cls.parse(built)
    except Exception as ex:
        return "FAIL", "parse: {}: {}".format(type(ex).__name__, str(ex)[:160])
    if parsed.x != 1:
        return "FAIL", "parse x={} != 1".format(parsed.x)
    try:
        built2 = parsed.build()
    except Exception as ex:
        return "FAIL", "rebuild: {}: {}".format(type(ex).__name__, str(ex)[:160])
    if built2 != built:
        return "FAIL", "roundtrip {} != {}".format(built2, built)
    return "PASS", "c={!r}".format(parsed.c)


def main():
    rows = []
    fails = 0
    for wrapper in WRAPPERS:
        for consumer in CONSUMERS:
            for form in FORMS:
                status, info = build_case(wrapper, consumer, form)
                rows.append((wrapper, consumer, form, status, info))
                if status != "PASS":
                    fails += 1
    # 附加：If false 分支（cond 求值 0 → Pass，inner 0 字节）
    for wrapper in WRAPPERS:
        status, info = build_case(wrapper, "If", "sub_x1_false",
                                  form_src="x - 1", e=0)
        rows.append((wrapper, "If", "sub_x1_false", status, info))
        if status != "PASS":
            fails += 1

    quiet = "--quiet" in sys.argv
    if not quiet:
        w = max(len(r[0]) for r in rows)
        c = max(len(r[1]) for r in rows)
        f = max(len(r[2]) for r in rows)
        for wrapper, consumer, form, status, info in rows:
            mark = "✓" if status == "PASS" else "✗"
            print("{:{}} {:{}} {:{}} {} {}".format(
                wrapper, w, consumer, c, form, f, mark, info))
    total = len(rows)
    print("\n{} / {} PASS, {} FAIL".format(total - fails, total, fails))
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
