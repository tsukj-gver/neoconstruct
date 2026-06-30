"""Phase 4 子任务 4.5 V-3/V-4 修正：RepeatUntil 描述符单元测试（纯 Python，无 pytest）。

测试覆盖：
- V-3：非 callable 谓词（``True`` / ``False`` / 常量值）兼容
- V-4：AST 识别负整数常量（``lambda x, _, _: x > -1``、``x != -1``）

注意：本测试仅检查 Python 侧描述符的编译行为（``_try_compile_repeat_predicate``），
不涉及 Rust parse/build 路径（由 cargo test 覆盖）。

运行方式：``python tests/test_repeat_until_descriptor.py``
"""

import sys
import traceback


# ============================================================
# V-4：AST 识别负整数常量
# ============================================================

def test_negative_int_const_right_side():
    # ``x > -1`` → [GetElem, Const(-1), Gt]
    ops = _try_compile_repeat_predicate(lambda x, l, c: x > -1)
    assert ops == [("getelem",), ("const", -1), ("gt",)], f"got {ops}"


def test_negative_int_const_neq():
    # ``x != -1`` → [GetElem, Const(-1), Ne]  —— 常见哨兵终止模式
    ops = _try_compile_repeat_predicate(lambda x, l, c: x != -1)
    assert ops == [("getelem",), ("const", -1), ("ne",)], f"got {ops}"


def test_negative_int_const_left_side():
    # ``-1 < x`` → 翻转 → x > -1 → [GetElem, Const(-1), Gt]
    ops = _try_compile_repeat_predicate(lambda x, l, c: -1 < x)
    assert ops == [("getelem",), ("const", -1), ("gt",)], f"got {ops}"


def test_positive_unary_plus_const():
    # ``x > +5`` → UAdd(Constant(5)) → 5
    ops = _try_compile_repeat_predicate(lambda x, l, c: x > +5)
    assert ops == [("getelem",), ("const", 5), ("gt",)], f"got {ops}"


def test_positive_int_const_still_works():
    # 回归：正整数仍正常识别
    ops = _try_compile_repeat_predicate(lambda x, l, c: x > 5)
    assert ops == [("getelem",), ("const", 5), ("gt",)], f"got {ops}"


def test_double_negative_recognized():
    # ``x > --5``（双重负号）→ UnaryOp(USub, UnaryOp(USub, Constant(5))) → 5
    ops = _try_compile_repeat_predicate(lambda x, l, c: x > --5)
    assert ops == [("getelem",), ("const", 5), ("gt",)], f"got {ops}"


def test_bool_not_recognized_as_int_const():
    # 回归：bool 值不识别为整数常量（``True`` 字面量是 ast.Constant(value=True)）
    ops = _try_compile_repeat_predicate(lambda x, l, c: x > True)
    assert ops is None, f"expected None, got {ops}"


def test_large_negative_int():
    # ``x > -1000000`` 大负数
    ops = _try_compile_repeat_predicate(lambda x, l, c: x > -1000000)
    assert ops == [("getelem",), ("const", -1000000), ("gt",)], f"got {ops}"


# ============================================================
# V-3：非 callable 谓词兼容
# ============================================================

def test_true_predicate_wrapped_as_callable():
    desc = RepeatUntilDescriptor(True, Int8ub)
    assert callable(desc.predicate), "predicate should be callable"


def test_false_predicate_wrapped_as_callable():
    desc = RepeatUntilDescriptor(False, Int8ub)
    assert callable(desc.predicate), "predicate should be callable"


def test_arbitrary_const_predicate_wrapped():
    # 任意常量值（如字符串）也包装
    desc = RepeatUntilDescriptor("sentinel", Int8ub)
    assert callable(desc.predicate), "predicate should be callable"


def test_wrapped_true_predicate_returns_true():
    desc = RepeatUntilDescriptor(True, Int8ub)
    assert desc.predicate(0, [], object()) is True


def test_wrapped_false_predicate_returns_false():
    desc = RepeatUntilDescriptor(False, Int8ub)
    assert desc.predicate(0, [], object()) is False


def test_wrapped_const_predicate_uses_default_arg_captures_value():
    # 验证使用默认参数捕获（避免闭包晚期绑定）
    desc = RepeatUntilDescriptor(42, Int8ub)
    assert desc.predicate(0, [], object()) == 42
    assert desc.predicate(1, [0], object()) == 42


def test_wrapped_predicate_goes_pycallable_path():
    # 非 callable 谓词包装后不识别为 Expr 路径，_expr_params 应为空 dict
    desc = RepeatUntilDescriptor(True, Int8ub)
    assert desc._expr_params == {}, f"got {desc._expr_params}"


def test_pycallable_lambda_still_works():
    # 回归：复杂 lambda 仍走 PyCallable 路径
    desc = RepeatUntilDescriptor(
        lambda x, lst, ctx: len(lst) >= 2 and lst[-2:] == [0, 0],
        Int8ub,
    )
    assert desc._expr_params == {}, f"got {desc._expr_params}"
    assert callable(desc.predicate)


def test_wrapping_does_not_break_expr_path_for_lambda():
    # 回归：简单 lambda 仍走 Expr 路径
    desc = RepeatUntilDescriptor(lambda x, l, c: x > 5, Int8ub)
    assert desc._expr_params == {"predicate": [("getelem",), ("const", 5), ("gt",)]}, \
        f"got {desc._expr_params}"


# ============================================================
# 主入口：纯 Python 测试 runner
# ============================================================

def main():
    # 延迟 import 使 assert 错误消息更精确
    global _try_compile_repeat_predicate, RepeatUntilDescriptor, RepeatUntil, Int8ub
    from construct._descriptors import (
        RepeatUntilDescriptor as _RUD,
        RepeatUntil as _RU,
        _try_compile_repeat_predicate as _tcp,
    )
    from construct import Int8ub as _I8
    _try_compile_repeat_predicate = _tcp
    RepeatUntilDescriptor = _RUD
    RepeatUntil = _RU
    Int8ub = _I8

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
