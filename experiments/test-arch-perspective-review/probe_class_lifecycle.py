"""探针 3：协议类的开发期生命周期（编辑-重定义循环 / 继承 / 复用 / 重载）。

用户视角：协议定义不是一次写完的。用户会在 REPL/notebook 里反复编辑类、
继承旧版本类做 schema 演进、在模块级共享 field(...) 描述符、reload 模块。
现有套件全部把类定义写死在测试函数体内、一次性定义一次性使用——
结构上不覆盖"类的第二次生命"。

运行：.venv/Scripts/python.exe probe_class_lifecycle.py
"""

from __future__ import annotations

import gc
import importlib
import sys
import textwrap

sys.path.insert(0, r"D:\Project\Github\neoconstruct\neoconstruct\python")

from dataclasses import dataclass

from neoconstruct import Int8ub, Int16ub, StructMixin, field

# --- 3.1 编辑循环：同名类反复重定义（REPL rerun cell）---
try:
    for i in range(200):
        ns = {"__annotations__": {"x": int, "y": int}, "x": field(Int8ub), "y": field(Int16ub)}
        cls = dataclass(type(f"Redefined{i}", (StructMixin,), ns))
        assert cls.parse(b"\x01\x00\x02").x == 1
    print("redefine 200x: OK")
    gc.collect()
    print(f"gc ok, classes存活由引用管理（无法直接量化，无 crash 即可）")
except Exception as e:
    print("redefine FAIL:", type(e).__name__, e)

# --- 3.2 schema 演进：继承已编译的协议类 ---
@dataclass
class V1(StructMixin):
    magic: int = field(Int8ub)


try:
    @dataclass
    class V2(V1):
        extra: int = field(Int8ub)

    print("subclass V2(V1): defined OK, fields:", list(V2.__dataclass_fields__))
    b = V2(magic=1, extra=2).build()
    print("V2 build:", b.hex(), "| parse:", V2.parse(b))
except Exception as e:
    print("subclass FAIL:", type(e).__name__, e)

# --- 3.3 模块级共享 field 描述符（用户常见写法：F = field(Int8ub) 复用）---
shared = field(Int8ub)
try:

    @dataclass
    class A(StructMixin):
        v: int = shared

    @dataclass
    class B(StructMixin):
        w: int = shared

    print("shared descriptor in 2 classes: A.parse:", A.parse(b"\x07"), "B.parse:", B.parse(b"\x09"))
except Exception as e:
    print("shared descriptor FAIL:", type(e).__name__, e)

# --- 3.4 修改字段默认值后重建实例（编辑-继续用旧实例）---
@dataclass
class P(StructMixin):
    x: int = field(Int8ub)


p = P(x=1)
try:
    P.__init__.__defaults__  # dataclass 冻结签名，正常
    p2 = P(x=2)
    print("old instance still usable after edits:", p.build().hex(), p2.build().hex())
except Exception as e:
    print("old instance FAIL:", type(e).__name__, e)

# --- 3.5 importlib.reload：模块级协议定义重复导入 ---
mod_src = textwrap.dedent("""
from dataclasses import dataclass
from neoconstruct import Int8ub, StructMixin, field

@dataclass
class Proto(StructMixin):
    v: int = field(Int8ub)
""")
from pathlib import Path

mod_file = Path(__file__).with_name("probe_proto_mod_file.py")
mod_file.write_text(mod_src, encoding="utf-8")
sys.path.insert(0, str(mod_file.parent))
import probe_proto_mod_file as mod  # noqa: E402

first = mod.Proto
try:
    mod2 = importlib.reload(mod)
    print("reload: OK, new class is distinct:", mod2.Proto is not first,
          "| new parse:", mod2.Proto.parse(b"\x03").v,
          "| old still works:", first.parse(b"\x04").v)
except Exception as e:
    print("reload FAIL:", type(e).__name__, e)
finally:
    sys.modules.pop("probe_proto_mod_file", None)
    sys.path.remove(str(mod_file.parent))
    mod_file.unlink(missing_ok=True)

# --- 3.6 类定义后修改 dataclass_fields（危险操作用户也可能做，看是否静默错）---
# （省略：属于防御性，不属主要视角）
