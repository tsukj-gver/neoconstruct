# 实验验证：若 Python 侧递归收集嵌套表达式，运行时是否直接可用（不改库代码，仅 monkeypatch）
import construct._mixin as M
from construct._descriptors import PrefixedDescriptor
from construct._mixin import _FieldDescriptor, _ExprRef

orig = M._extract_and_compile_exprs

def patched(subcon, field_index_map, field_name):
    result = orig(subcon, field_index_map, field_name)
    # 递归进 Prefixed.subcon（一层），合并其表达式参数
    if isinstance(subcon, PrefixedDescriptor):
        inner = subcon.subcon
        inner_params = getattr(inner, "_expr_params", None)
        if inner_params:
            for pname, pval in inner_params.items():
                if isinstance(pval, (_FieldDescriptor, _ExprRef)):
                    ops = M._compile_expr_tree(pval, field_index_map, field_name)
                    # 防止覆盖外层同名参数：嵌套参数加前缀模拟（此处仅验证）
                    result[pname] = ops
    return result

M._extract_and_compile_exprs = patched

from dataclasses import dataclass
from typing import Any
from construct import StructMixin, field, Int8ub, Prefixed, Switch, Bytes, Computed

@dataclass
class P(StructMixin):
    typ: int = field(Int8ub)
    br: Any = field(Prefixed(Int8ub, Switch(typ, {1: Int8ub, 2: Bytes(2)})))

print("compile: OK")
print("parse typ=1:", P.parse(b"\x01\x01\x7f"))
print("parse typ=2:", P.parse(b"\x02\x02AB"))
print("build typ=1:", P(typ=1, br=127).build())
print("build typ=2:", P(typ=2, br=b"AB").build())

# Computed 嵌套（"func" 参数）
@dataclass
class P2(StructMixin):
    typ: int = field(Int8ub)
    br: Any = field(Prefixed(Int8ub, Computed(typ)))
print("Prefixed x Computed parse:", P2.parse(b"\x01\x01"))

# If 嵌套（"cond" 参数）
from construct import If
@dataclass
class P3(StructMixin):
    typ: int = field(Int8ub)
    br: Any = field(Prefixed(Int8ub, If(typ == 1, Int8ub)))
print("Prefixed x If parse:", P3.parse(b"\x01\x01\x7f"))
print("Prefixed x If parse(false):", P3.parse(b"\x00\x00"))

# Bytes 长度嵌套（"length" 参数）
@dataclass
class P4(StructMixin):
    typ: int = field(Int8ub)
    br: Any = field(Prefixed(Int8ub, Bytes(typ)))
print("Prefixed x Bytes(typ) parse:", P4.parse(b"\x02\x02AB"))
print("Prefixed x Bytes(typ) build:", P4(typ=2, br=b"XY").build())

# PrefixedArray 嵌套 Switch
from construct import PrefixedArray
M._extract_and_compile_exprs = orig

# PrefixedArray 也 patch（其 subcon 属性名也是 subcon）
from construct._descriptors import PrefixedArrayDescriptor
def patched2(subcon, field_index_map, field_name):
    result = orig(subcon, field_index_map, field_name)
    if isinstance(subcon, (PrefixedDescriptor, PrefixedArrayDescriptor)):
        inner = subcon.subcon
        inner_params = getattr(inner, "_expr_params", None)
        if inner_params:
            for pname, pval in inner_params.items():
                if isinstance(pval, (_FieldDescriptor, _ExprRef)):
                    ops = M._compile_expr_tree(pval, field_index_map, field_name)
                    result[pname] = ops
    return result
M._extract_and_compile_exprs = patched2

@dataclass
class P5(StructMixin):
    typ: int = field(Int8ub)
    br: Any = field(PrefixedArray(Int8ub, Switch(typ, {1: Int8ub, 2: Bytes(2)})))
print("PrefixedArray x Switch parse:", P5.parse(b"\x01\x02\x05\x06"))
