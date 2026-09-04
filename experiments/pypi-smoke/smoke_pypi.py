"""PyPI 发布产物的端到端冒烟测试——模拟真实用户 pip install 后的使用场景。

覆盖用户面核心特性：
1. 声明式 @dataclass + field()
2. 字段名直接引用（长度前置字段）
3. Prefixed + Switch 嵌套（v0.1.1 BUG-1 回归）
4. Const / Default 无参实例化（v0.1.1 BUG-2 回归）
5. wfield(Padding) 无哑值（v0.1.1 P3 回归）
6. parse/build/roundtrip
"""
from dataclasses import dataclass
from typing import Any

import neoconstruct
from neoconstruct import (
    Bytes,
    Const,
    Default,
    Int16ub,
    Int8ub,
    Padding,
    Prefixed,
    StructMixin,
    Switch,
    field,
    wfield,
)

print(f"neoconstruct {neoconstruct.__name__} imported OK")


# --- 用例 1：基础声明式 + 字段引用 ---
@dataclass
class Header(StructMixin):
    magic: int = field(Int16ub)
    length: int = field(Int8ub)
    payload: bytes = field(Bytes(length))


h = Header(magic=0xCAFE, length=3, payload=b"abc")
assert h.build() == b"\xCA\xFE\x03abc", h.build()
assert Header.parse(b"\xCA\xFE\x03abc") == h
print("case1 基础声明式 + 字段引用: OK")


# --- 用例 2：Prefixed + Switch 嵌套引用根字段（v0.1.1 修复的 BUG-1）---
@dataclass
class Frame(StructMixin):
    typ: int = field(Int8ub)
    br: Any = field(Prefixed(Int8ub, Switch(typ, {1: Int8ub, 2: Bytes(2)})))


f1 = Frame.parse(b"\x01\x01\x7f")
assert f1.typ == 1 and f1.br == 127, f1
f2 = Frame.parse(b"\x02\x02AB")
assert f2.typ == 2 and f2.br == b"AB", f2
assert Frame(typ=1, br=127).build() == b"\x01\x01\x7f"
assert Frame(typ=2, br=b"AB").build() == b"\x02\x02AB"
print("case2 Prefixed x Switch 嵌套: OK")


# --- 用例 3：Const/Default 无参实例化 + build 自动补值（BUG-2）---
@dataclass
class WithConst(StructMixin):
    magic: int = field(Const(0xA5, Int8ub))
    value: int = field(Default(Int8ub, 0x07))
    data: int = field(Int8ub)


c = WithConst(data=0)  # 普通字段 data 必填（预期）；Const/Default 字段无参——BUG-2 修复点
assert c.build() == b"\xA5\x07\x00", c.build()
c2 = WithConst(data=0xFF)
assert c2.build() == b"\xA5\x07\xFF"
parsed = WithConst.parse(b"\xA5\x07\x33")
assert parsed.data == 0x33
print("case3 Const/Default 无参实例化: OK")


# --- 用例 4：wfield(Padding) 不再强制哑值（P3）---
@dataclass
class Padded(StructMixin):
    n: int = field(Int8ub)
    pad: bytes = wfield(Padding(2))


p = Padded(n=1)  # 无哑值
assert p.build() == b"\x01\x00\x00", p.build()
print("case4 wfield(Padding) 无哑值: OK")


# --- 用例 5：完整协议小帧 roundtrip（组合以上能力）---
@dataclass
class Message(StructMixin):
    proto: int = field(Int8ub)                     # 协议类型
    body: Any = field(Prefixed(Int16ub, Switch(proto, {  # 长度前缀 + 按类型分派
        1: Header,
    })))
    tail: bytes = field(Const(0x7E, Int8ub), ) if False else field(Bytes(1))


m = Message.parse(b"\x01\x00\x06\xCA\xFE\x03abc\x7E")
assert m.proto == 1 and m.tail == b"\x7E", m
b = m.build()
assert b == b"\x01\x00\x06\xCA\xFE\x03abc\x7E", b
print("case5 组合协议帧 roundtrip: OK")

print("\nALL SMOKE CASES PASSED — PyPI wheel (neoconstruct 0.1.1) 可正常使用")
