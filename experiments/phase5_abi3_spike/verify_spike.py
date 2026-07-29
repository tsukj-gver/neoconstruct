"""Phase 5 ABI3 + pyo3 bound method 行为 spike（Python 侧轻量验证）。

ARCH 权限限制：不可修改 src/ 文件。本脚本只做 Python 侧可验证的子集，
为候选 A（pyo3 bound method 类属性行为）+ 候选 B（pyclass 类型 setattr 可行性）
提供实证依据。

可验证项：
1. pyclass frozen 类型对象能否 setattr 新属性（候选 B 路径 C 风险 1）
2. pyo3 bound method（schema._parse_raw）作为类属性时的调用语义（候选 A 核心假设，REV P1-3）
3. StructMixin.parse classmethod 替换为 bound method 后的调用正确性
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "construct-rs" / "python"))

from construct import StructMixin, field, Int8ub  # noqa: E402
from construct._construct_rust import CompiledSchema  # noqa: E402
from dataclasses import dataclass  # noqa: E402

results = []


def check(name: str, cond: bool, detail: str = ""):
    status = "PASS" if cond else "FAIL"
    results.append((name, status, detail))
    print(f"  [{status}] {name}: {detail}")


print("=" * 72)
print("Spike 1: pyclass frozen 类型对象能否 setattr 新属性（路径 C 风险 1）")
print("=" * 72)

# 1.1 验证 CompiledSchema 类型可 setattr（pyclass frozen 不一定阻止类型对象 setattr）
try:
    sentinel = object()
    CompiledSchema._spike_test_attr = sentinel
    retrieved = CompiledSchema._spike_test_attr
    check(
        "1.1 CompiledSchema 类型 setattr 新属性",
        retrieved is sentinel,
        f"sentinel 存活：retrieved is sentinel = {retrieved is sentinel}",
    )
    # 清理
    del CompiledSchema._spike_test_attr
except Exception as e:
    check("1.1 CompiledSchema 类型 setattr 新属性", False, f"异常：{type(e).__name__}: {e}")

# 1.2 验证 CompiledSchema 类型 setattr 一个 PyCFunction（模拟路径 C）
# 这里用 Python 函数代替（CPython 内置函数对象与 PyCFunction 行为接近）
def fake_raw_c_func(self_, data):
    """模拟 raw C 函数签名 (self_, data) -> result。"""
    return (self_, data)


try:
    CompiledSchema._spike_pycfunc_like = fake_raw_c_func
    check(
        "1.2 类型 setattr Python 函数",
        True,
        "setattr 成功（Python 函数作为类型属性）",
    )
except Exception as e:
    check("1.2 类型 setattr Python 函数", False, f"异常：{type(e).__name__}: {e}")

# 清理
try:
    del CompiledSchema._spike_pycfunc_like
except AttributeError:
    pass


print()
print("=" * 72)
print("Spike 2: pyo3 bound method 作为类属性的调用语义（候选 A 核心，REV P1-3）")
print("=" * 72)

# 构造一个真实的 StructMixin 子类 + schema
@dataclass
class TestStruct(StructMixin):
    x: int = field(Int8ub)
    y: int = field(Int8ub)
    z: int = field(Int8ub)


schema = TestStruct._construct_compiled

# 2.1 验证 schema._parse_raw 是 bound method（self 已绑定为 schema）
parse_attr = schema._parse_raw
check(
    "2.1 schema._parse_raw 类型",
    "method" in str(type(parse_attr)).lower() or "builtin_function_or_method" in str(type(parse_attr)),
    f"type(schema._parse_raw) = {type(parse_attr).__name__}",
)

# 2.2 验证 schema._parse_raw(b'...') 正常工作（bound method 自带 self=schema）
test_data = b"\x01\x02\x03"
try:
    instance_via_bound = schema._parse_raw(test_data)
    check(
        "2.2 schema._parse_raw(data) 直接调用",
        isinstance(instance_via_bound, TestStruct),
        f"返回 TestStruct 实例：{type(instance_via_bound).__name__}",
    )
    check(
        "2.2.1 字段值正确",
        (instance_via_bound.x, instance_via_bound.y, instance_via_bound.z) == (1, 2, 3),
        f"(x,y,z) = ({instance_via_bound.x},{instance_via_bound.y},{instance_via_bound.z})",
    )
except Exception as e:
    check("2.2 schema._parse_raw(data) 直接调用", False, f"异常：{type(e).__name__}: {e}")


print()
print("=" * 72)
print("Spike 3: 候选 A 核心假设——cls.parse = schema._parse_raw 的调用正确性")
print("=" * 72)

# 3.1 备份原 parse，挂载 fast binding
original_parse = TestStruct.parse  # classmethod
TestStruct.parse = schema._parse_raw  # 候选 A：直接挂载 bound method

try:
    # 3.2 通过类调用：TestStruct.parse(data)
    instance_class_call = TestStruct.parse(test_data)
    check(
        "3.2 TestStruct.parse(data) 类调用",
        isinstance(instance_class_call, TestStruct)
        and (instance_class_call.x, instance_class_call.y, instance_class_call.z) == (1, 2, 3),
        f"返回 TestStruct 实例，字段正确",
    )

    # 3.3 通过实例调用：instance.parse(data) —— 关键验证（self 是否错绑）
    # 注意：这是 REV P1-3 的核心担忧——pyo3 bound method 作为类属性时，
    #       instance.parse(data) 会不会把 instance 当作 self 重复绑定？
    dummy_instance = TestStruct(x=99, y=99, z=99)
    try:
        instance_method_call = dummy_instance.parse(test_data)
        check(
            "3.3 instance.parse(data) 实例调用（self 绑定验证）",
            isinstance(instance_method_call, TestStruct)
            and (instance_method_call.x, instance_method_call.y, instance_method_call.z) == (1, 2, 3),
            f"返回新实例，字段=(1,2,3)，self 未错绑为 dummy_instance",
        )
    except Exception as e:
        check(
            "3.3 instance.parse(data) 实例调用",
            False,
            f"异常（self 可能错绑）：{type(e).__name__}: {e}",
        )

    # 3.4 验证 self 是 schema 而非 instance——通过比较 id
    # schema._parse_raw 创建的实例 __class__ 是 TestStruct，但创建过程用的是 schema，
    # 不是 instance。这点已由 3.3 的字段正确性间接证明。

except Exception as e:
    check("3 候选 A fast binding", False, f"异常：{type(e).__name__}: {e}")
finally:
    # 恢复
    TestStruct.parse = original_parse


print()
print("=" * 72)
print("Spike 4: parse 是 classmethod，替换为普通函数后实例访问的行为")
print("=" * 72)
# 这是候选 A build 路径 BC-A1 self 冲突的对比验证——parse 不应有此问题

# 4.1 验证原始 StructMixin.parse 是 classmethod
check(
    "4.1 StructMixin.parse 是 classmethod",
    isinstance(StructMixin.parse, classmethod) or isinstance(
        type(StructMixin).__dict__.get("parse", StructMixin.parse), classmethod
    ),
    f"type(StructMixin.parse) = {type(StructMixin.parse).__name__}",
)

# 4.2 验证 schema._parse_raw 不是 classmethod（是 instance method / builtin）
check(
    "4.2 schema._parse_raw 不是 classmethod",
    not isinstance(schema._parse_raw, classmethod),
    f"type = {type(schema._parse_raw).__name__}",
)

# 总结
print()
print("=" * 72)
print("SPIKE 结果汇总")
print("=" * 72)
total = len(results)
passed = sum(1 for _, s, _ in results if s == "PASS")
failed = sum(1 for _, s, _ in results if s == "FAIL")
print(f"  {passed}/{total} PASS, {failed}/{total} FAIL")
print()

if failed == 0:
    print("结论：")
    print("  - 候选 A 核心假设（pyo3 bound method 作为类属性）验证通过")
    print("  - pyclass 类型 setattr 新属性可行（路径 C 风险 1 初步排除）")
    print("  - 剩余路径 C 风险（m_self 绑定语义 + pyclass getattr 拦截）需 Rust 侧 spike")
else:
    print("⚠️  有 FAIL 项，需 ARCH 进一步分析")

# 写入结果文件
out = Path(__file__).parent / "spike_result.md"
lines = [
    "# Phase 5 ABI3 + pyo3 bound method Python 侧 Spike 结果",
    "",
    f"测量时间：{__import__('datetime').datetime.now().isoformat(timespec='seconds')}",
    f"Python：{sys.version.split()[0]}",
    "",
    f"## 汇总：{passed}/{total} PASS, {failed}/{total} FAIL",
    "",
    "## 详细结果",
    "",
    "| # | 检查项 | 状态 | 详情 |",
    "|---|--------|------|------|",
]
for i, (name, status, detail) in enumerate(results, 1):
    lines.append(f"| {i} | {name} | {status} | {detail} |")
lines += [
    "",
    "## 覆盖范围",
    "",
    "- 候选 A 核心假设（pyo3 bound method 作为类属性，self 自动绑定为 schema）：✅ 验证",
    "- 候选 B 路径 C 风险 1（pyclass 类型 setattr）：✅ 初步验证（Python 函数）",
    "- 候选 B 路径 C 风险 2（PyCFunction m_self 绑定语义）：❌ 需 Rust 侧 spike",
    "- 候选 B 路径 C 风险 3（pyclass getattr 拦截）：❌ 需 Rust 侧 spike",
]
out.write_text("\n".join(lines), encoding="utf-8")
print(f"\n结果已写入：{out}")
