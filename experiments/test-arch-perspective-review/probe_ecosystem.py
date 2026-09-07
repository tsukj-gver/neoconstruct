"""探针 1：parse 产出对象在 Python 数据生态中的后续生命周期。

用户视角：parse 只是起点——用户会把解析结果 pickle 进缓存、asdict 导出、
deepcopy、放进 set/dict、json 序列化做日志。这些操作是否如 dataclass
用户所预期地工作，不属于任何构造器语义，现有套件（构造器×语义矩阵）
结构上不会触及。

运行：.venv/Scripts/python.exe probe_ecosystem.py
"""

from __future__ import annotations

import copy
import json
import pickle
import sys
from dataclasses import asdict, astuple, replace

sys.path.insert(0, r"D:\Project\Github\neoconstruct\neoconstruct\python")

from dataclasses import dataclass

from neoconstruct import Bytes, Int8ub, Int16ub, StructMixin, field


@dataclass
class Inner(StructMixin):
    a: int = field(Int8ub)
    b: int = field(Int8ub)


@dataclass
class Frame(StructMixin):
    magic: int = field(Int16ub)
    inner: Inner = field(Inner)
    payload: bytes = field(Bytes(3))


DATA = b"\xCA\xFE\x01\x02\xAA\xBB\xCC"
frame = Frame.parse(DATA)

print("== parsed ==", frame)

# --- asdict / astuple / replace ---
try:
    d = asdict(frame)
    print("asdict OK:", d)
except Exception as e:
    print("asdict FAIL:", type(e).__name__, e)

try:
    t = astuple(frame)
    print("astuple OK:", t)
except Exception as e:
    print("astuple FAIL:", type(e).__name__, e)

try:
    f2 = replace(frame, magic=1)
    print("replace OK:", f2, "build:", f2.build().hex())
except Exception as e:
    print("replace FAIL:", type(e).__name__, e)

# --- copy / deepcopy ---
try:
    c = copy.copy(frame)
    print("copy OK:", c == frame)
except Exception as e:
    print("copy FAIL:", type(e).__name__, e)

try:
    dc = copy.deepcopy(frame)
    print("deepcopy OK:", dc == frame, dc is not frame, dc.inner is not frame.inner)
except Exception as e:
    print("deepcopy FAIL:", type(e).__name__, e)

# --- pickle ---
try:
    blob = pickle.dumps(frame, protocol=pickle.HIGHEST_PROTOCOL)
    back = pickle.loads(blob)
    print("pickle OK: roundtrip equal =", back == frame, "| build =", back.build() == frame.build())
except Exception as e:
    print("pickle FAIL:", type(e).__name__, e)

# --- hash / set / dict key（可变 dataclass 默认不可 hash——与纯 dataclass 一致即可）---
try:
    hash(frame)
    print("hash OK")
except TypeError as e:
    print("hash raises TypeError (与普通可变 dataclass 一致):", str(e)[:60])

# --- repr 可读性（REPL/日志用户）---
r = repr(frame)
print("repr:", r[:120])

# --- json（需先 asdict；bytes 不 JSON 可序列化是 Python 常识，这里只看链路）---
try:
    print("json(asdict, default=hex):", json.dumps(asdict(frame), default=lambda o: o.hex())[:100])
except Exception as e:
    print("json FAIL:", type(e).__name__, e)

# --- 相等性语义：同字段值的不同实例 ==；与 construct Container 的混用不可比 ---
print("eq same-value instances:", Frame.parse(DATA) == Frame.parse(DATA))

# --- 类本身的可 pickle 性（类属性挂着 Rust 编译产物）---
try:
    pickle.dumps(Frame)  # 类对象本身
    print("pickle(class) OK")
except Exception as e:
    print("pickle(class) FAIL:", type(e).__name__, str(e)[:80])
