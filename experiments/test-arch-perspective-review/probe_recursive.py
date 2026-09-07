"""探针 4：递归/自引用协议（TLV 树）与深嵌套栈安全。

用户视角：TLV（type-length-value 嵌套）是二进制协议最常见形态之一
（ASN.1 BER、MKV、ISO 7816 TLV、蓝牙 ATT）。用户会问："我能表达
'value 里还有同类记录' 吗？"现有套件只测 3~5 层静态深度（test_edge_cases）
且 mutual-reference 测试被显式"简化跳过"——递归形态从未被验证过。

运行：.venv/Scripts/python.exe probe_recursive.py
（v2：修复 v1 中 exec 注入导致的 TLV 假失败与 build 实例 bug）
"""

from __future__ import annotations

import sys

sys.path.insert(0, r"D:\Project\Github\neoconstruct\neoconstruct\python")

from dataclasses import dataclass

from neoconstruct import Bytes, Int8ub, StructMixin, field

# --- 4.1 固定深度 TLV（组合，无递归）——基线可用性 ---
@dataclass
class TLV(StructMixin):
    t: int = field(Int8ub)
    l: int = field(Int8ub)
    v: bytes = field(Bytes(l))


print("4.1 fixed TLV:", TLV.parse(b"\x01\x02\xAA\xBB"), "| build:", TLV(t=1, l=2, v=b"\xAA\xBB").build())

# --- 4.2 两层嵌套 TLV（value 内含子 TLV，手动分字节）---
@dataclass
class TLVChild(StructMixin):
    t: int = field(Int8ub)
    l: int = field(Int8ub)
    v: bytes = field(Bytes(l))


@dataclass
class TLVParent(StructMixin):
    t: int = field(Int8ub)
    l: int = field(Int8ub)
    child: TLVChild = field(TLVChild)


p = TLVParent.parse(b"\x01\x03\x05\x01\xAA")
print("4.2 nested TLV (fixed depth 2):", p)

# --- 4.3 真递归：自引用（Switch 到自身类名/字符串）---
try:
    exec("""
from dataclasses import dataclass
from neoconstruct import Bytes, Int8ub, StructMixin, field, Switch

@dataclass
class Node(StructMixin):
    t: int = field(Int8ub)
    child: object = field(Switch(t, {1: 'Node'}))   # 字符串自引用尝试
""")
    print("4.3 self-reference via Switch: OK（意外支持）")
except Exception as e:
    print(f"4.3 self-reference via Switch: {type(e).__name__}: {str(e)[:100]}")

# --- 4.4 前向类引用（B 在 A 之后定义）---
try:
    exec("""
from dataclasses import dataclass
from neoconstruct import Bytes, Int8ub, StructMixin, field, Array

@dataclass
class A2(StructMixin):
    tag: int = field(Int8ub)
    kids: list = field(Array(2, B2))   # B2 后定义
""")
    print("4.4 forward class ref: OK（延迟桩生效）")
except NameError as e:
    print(f"4.4 forward class ref: NameError（README 已知限制）: {str(e)[:60]}")
except Exception as e:
    print(f"4.4 forward class ref: {type(e).__name__}: {str(e)[:100]}")

# --- 4.5 深嵌套栈安全：动态生成 300 层并 parse+build ---
import types


def make_deep(levels: int):
    """在真实模块命名空间中生成 levels 层嵌套类，返回顶层类。"""
    mod = types.ModuleType(f"probe_deep_{levels}")
    sys.modules[mod.__name__] = mod
    src_lines = [
        "from dataclasses import dataclass",
        "from neoconstruct import Int8ub, StructMixin, field",
        "@dataclass",
        "class N0(StructMixin):",
        "    v: int = field(Int8ub)",
    ]
    for i in range(1, levels):
        src_lines += [
            "@dataclass",
            f"class N{i}(StructMixin):",
            "    x: int = field(Int8ub)",
            f"    c: object = field(N{i-1})",
        ]
    exec("\n".join(src_lines), mod.__dict__)
    return mod, mod.__dict__[f"N{levels - 1}"]


try:
    mod, top = make_deep(300)
    data = b"\x01" * 300
    inst = top.parse(data)
    depth = 0
    node = inst
    while hasattr(node, "c") and node.c is not None:
        node = node.c
        depth += 1
    rebuilt = inst.build()
    print(f"4.5 deep nesting 300 levels: parse OK (walked {depth}), build round-trip: {rebuilt == data}")
    sys.modules.pop(mod.__name__, None)
except Exception as e:
    print(f"4.5 deep nesting FAIL: {type(e).__name__}: {str(e)[:160]}")

# --- 4.6 更深：2000 层（Rust 侧递归下降是否会栈溢出/进程崩溃）---
try:
    mod, top = make_deep(2000)
    data = b"\x01" * 2000
    inst = top.parse(data)
    rebuilt = inst.build()
    print(f"4.6 deep nesting 2000 levels: parse+build OK, round-trip: {rebuilt == data}")
    sys.modules.pop(mod.__name__, None)
except Exception as e:
    print(f"4.6 deep nesting 2000 FAIL: {type(e).__name__}: {str(e)[:160]}")
