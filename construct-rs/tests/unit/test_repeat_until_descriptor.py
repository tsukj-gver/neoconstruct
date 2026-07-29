"""Phase 4 子任务 4.5 v5 RepeatUntil 描述符单元测试（纯 Python，无 pytest）。

v5 重写后测试覆盖：
- callable 拒绝（用户硬约束 #1）
- terminator 表达式编译（Phase 2 表达式系统）
- _expr_params 协议（terminator + element_field_idx）
- set_compiled_expr_params 方法

注意：本测试仅检查 Python 侧描述符的编译行为（v5），
不涉及 Rust parse/build 路径（由 cargo test + experiments 覆盖）。

运行方式：``python tests/test_repeat_until_descriptor.py``
"""

import sys
import traceback

# 顶部 import：让 pytest 收集时可用（原 standalone 模式 import 在 main() 里，
# pytest 直接调 test 函数会 NameError）。main() 里的 global import 保留兼容。
from construct._descriptors import RepeatUntilDescriptor
from construct import Int8ub, CompilationError


# ============================================================
# v5 callable 拒绝（用户硬约束 #1）
# ============================================================

def test_callable_lambda_rejected():
    """Python lambda 必须被 RepeatUntilDescriptor.__init__ 拒绝。"""
    try:
        RepeatUntilDescriptor(lambda x, l, c: x > 5, Int8ub)
        raise AssertionError("callable terminator should be rejected")
    except CompilationError as e:
        assert "callable" in str(e).lower() or "Phase 2 expression" in str(e), (
            f"error message should mention callable: {e}"
        )


def test_callable_function_rejected():
    """Python 函数也必须被拒绝。"""

    def predicate(x, l, c):
        return x > 5

    try:
        RepeatUntilDescriptor(predicate, Int8ub)
        raise AssertionError("callable function should be rejected")
    except CompilationError:
        pass  # expected


def test_callable_class_with_call_rejected():
    """实现 __call__ 的类实例也必须被拒绝。"""

    class CallableClass:
        def __call__(self, x, l, c):
            return x > 5

    try:
        RepeatUntilDescriptor(CallableClass(), Int8ub)
        raise AssertionError("callable instance should be rejected")
    except CompilationError:
        pass  # expected


# ============================================================
# v5 terminator 表达式存储
# ============================================================

def test_terminator_stored_correctly():
    """terminator（Phase 2 表达式）应被正确存储为属性。"""
    from construct import rfield, Element
    # terminator 是 _FieldDescriptor 引用（e > 5）
    # 但 Element() 在 class 上下文才有 _FieldDescriptor。这里直接用 int 模拟。
    desc = RepeatUntilDescriptor(5, Int8ub)  # 5 是 int（非 callable）
    assert desc.terminator == 5
    assert desc.subcon is Int8ub
    assert desc.discard is False


def test_discard_flag_stored():
    desc = RepeatUntilDescriptor(5, Int8ub, discard=True)
    assert desc.discard is True


def test_repr_uses_terminator_not_predicate():
    """__repr__ 应使用 terminator 而非 predicate（禁用谓词术语）。"""
    desc = RepeatUntilDescriptor(5, Int8ub)
    repr_str = repr(desc)
    assert "terminator" in repr_str, f"repr should use 'terminator': {repr_str}"
    assert "predicate" not in repr_str, (
        f"repr should not use 'predicate' (禁用谓词术语): {repr_str}"
    )


# ============================================================
# v5 _expr_params 协议
# ============================================================

def test_expr_params_initial_has_terminator():
    """_expr_params 初始应含 terminator 键（值为原始表达式）。"""
    desc = RepeatUntilDescriptor(5, Int8ub)
    assert "terminator" in desc._expr_params, (
        f"_expr_params should have 'terminator': {desc._expr_params}"
    )
    assert desc._expr_params["terminator"] == 5


def test_element_field_idx_initial_none():
    """_element_field_idx 初始为 None（编译期由 _compile_expressions 填充）。"""
    desc = RepeatUntilDescriptor(5, Int8ub)
    assert desc._element_field_idx is None


def test_set_compiled_expr_params_injects_ops_and_idx():
    """set_compiled_expr_params 应注入 ops 和 element_field_idx。"""
    desc = RepeatUntilDescriptor(5, Int8ub)
    ops = [("getint", 0), ("const", 5), ("gt",)]
    desc.set_compiled_expr_params(ops, 0)
    assert desc._element_field_idx == 0
    assert desc._expr_params["terminator"] == ops
    assert desc._expr_params["element_field_idx"] == 0
    # index_field_indices 默认为空列表
    assert desc._expr_params["index_field_indices"] == []


def test_set_compiled_expr_params_with_index_fields():
    """set_compiled_expr_params 应注入 index_field_indices。"""
    desc = RepeatUntilDescriptor(5, Int8ub)
    ops = [("getint", 0), ("getint", 1), ("add",)]
    desc.set_compiled_expr_params(ops, 0, [1])
    assert desc._element_field_idx == 0
    assert desc._expr_params["index_field_indices"] == [1]


# ============================================================
# 端到端：完整 StructMixin 子类（含 Element 字段）
# ============================================================

def test_end_to_end_struct_with_element():
    """端到端：完整的 v5 RepeatUntil + Element StructMixin 子类。"""
    from dataclasses import dataclass
    from construct import (
        StructMixin,
        field,
        rfield,
        RepeatUntil,
        Element,
    )

    @dataclass
    class Packet(StructMixin):
        e: int = rfield(Element())
        payload: list = field(RepeatUntil(e > 5, Int8ub))

    pkt = Packet.parse(b"\x01\x02\x06\xaa")
    assert pkt.payload == [1, 2, 6], f"got {pkt.payload}"
    assert pkt.e is None, f"Element field must be None, got {pkt.e}"

    # build round-trip
    built = Packet.build(Packet(payload=[1, 2, 6]))
    assert built == b"\x01\x02\x06", f"got {built!r}"
    pkt2 = Packet.parse(built)
    assert pkt2.payload == [1, 2, 6], f"got {pkt2.payload}"


def test_end_to_end_compound_terminator():
    """复合终止表达式：(e + offset) & mask == sentinel"""
    from dataclasses import dataclass
    from construct import (
        StructMixin,
        field,
        rfield,
        RepeatUntil,
        Element,
    )

    @dataclass
    class P(StructMixin):
        offset: int = field(Int8ub)
        mask: int = field(Int8ub)
        sentinel: int = field(Int8ub)
        e: int = rfield(Element())
        payload: list = field(RepeatUntil(((e + offset) & mask) == sentinel, Int8ub))

    # offset=1, mask=0x07, sentinel=0x00
    # e=0: (0+1)&7=1 != 0
    # e=1: (1+1)&7=2 != 0
    # ...
    # e=7: (7+1)&7=0 == 0 → 终止
    pkt = P.parse(b"\x01\x07\x00\x00\x01\x02\x03\x07\xff")
    assert pkt.payload == [0, 1, 2, 3, 7], f"got {pkt.payload}"


# ============================================================
# 主入口：纯 Python 测试 runner
# ============================================================

def main():
    global RepeatUntilDescriptor, Int8ub, CompilationError
    from construct._descriptors import (
        RepeatUntilDescriptor as _RUD,
    )
    from construct import Int8ub as _I8
    from construct import CompilationError as _CE
    RepeatUntilDescriptor = _RUD
    Int8ub = _I8
    CompilationError = _CE

    # 收集所有 test_ 函数
    test_funcs = [
        (name, globals()[name])
        for name in sorted(globals())
        if name.startswith("test_") and callable(globals()[name])
    ]

    passed = 0
    failed = 0
    for name, fn in test_funcs:
        try:
            fn()
            print(f"  [PASS] {name}")
            passed += 1
        except Exception:
            print(f"  [FAIL] {name}")
            traceback.print_exc()
            failed += 1

    print()
    print(f"Total: {passed + failed}, Passed: {passed}, Failed: {failed}")
    return 0 if failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
