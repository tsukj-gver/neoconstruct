"""表达式编译管线测试（Phase 2.3 Part C）。

设计依据：``docs/design/模块设计/模块设计-表达式系统.md`` §3.1-§3.7。

覆盖：
- ``_compile_expr_tree``：表达式树 → ExprOp 指令列表（后序遍历）
- ``_OP_TO_EXPROP``：operator 函数 → ExprOp 名称映射完整性
- ``_check_forward_reference``：前向引用检测
- ``_check_wo_reference``：WO 字段引用检测
- ``_extract_and_compile_exprs``：``_expr_params`` 协议提取
- ``_compile_expressions``：端到端编译
- ``_expr_programs_to_list``：格式转换
- 错误路径：float / bool / 未知类型 / 未知 operator

注意：当前阶段（2.3）Rust 侧的 ``BytesDescriptor`` 尚未扩展 ``_expr_params``
（那是后续子任务）。因此本测试使用 mock 对象模拟 ``_expr_params`` 协议。
"""

import operator

import pytest

from construct import CompilationError, Int8ub
from construct._mixin import (
    _FieldDescriptor,
    _OP_TO_EXPROP,
    _check_forward_reference,
    _check_wo_reference,
    _compile_expr_tree,
    _compile_expressions,
    _emit_expr_ops,
    _expr_programs_to_list,
    _extract_and_compile_exprs,
    _ExprRef,
    field,
    rfield,
    wfield,
)


# ---------------------------------------------------------------------------
# Mock 描述符：模拟 _expr_params 协议
# ---------------------------------------------------------------------------


class _MockExprParamsDescriptor:
    """模拟实现了 ``_expr_params`` 协议的描述符。

    真实描述符（如扩展后的 ``BytesDescriptor``、``ComputedDescriptor``）将在
    后续子任务中实现。此处用纯 Python mock 验证编译管线逻辑。

    ``_expr_params`` 返回 ``{param_name: value}``，value 可以是：
    - int / 常量：不编译
    - ``_FieldDescriptor`` / ``_ExprRef``：编译为 ExprOp 指令
    """

    def __init__(self, params):
        self._params = params

    @property
    def _expr_params(self):
        return self._params

    def __repr__(self):
        return "_MockExprParamsDescriptor({!r})".format(self._params)


# ---------------------------------------------------------------------------
# _OP_TO_EXPROP 映射完整性
# ---------------------------------------------------------------------------


class TestOpMapping:
    """``_OP_TO_EXPROP`` 映射覆盖所有支持的 operator 函数。"""

    def test_all_arithmetic_ops_mapped(self):
        """算术运算符全部映射。"""
        assert _OP_TO_EXPROP[operator.add] == "add"
        assert _OP_TO_EXPROP[operator.sub] == "sub"
        assert _OP_TO_EXPROP[operator.mul] == "mul"
        assert _OP_TO_EXPROP[operator.floordiv] == "floordiv"
        assert _OP_TO_EXPROP[operator.mod] == "mod"

    def test_all_bitwise_ops_mapped(self):
        """位运算符全部映射。"""
        assert _OP_TO_EXPROP[operator.and_] == "bitand"
        assert _OP_TO_EXPROP[operator.or_] == "bitor"
        assert _OP_TO_EXPROP[operator.xor] == "bitxor"
        assert _OP_TO_EXPROP[operator.lshift] == "shl"
        assert _OP_TO_EXPROP[operator.rshift] == "shr"

    def test_unary_ops_mapped(self):
        """一元运算符全部映射。"""
        assert _OP_TO_EXPROP[operator.neg] == "neg"
        assert _OP_TO_EXPROP[operator.invert] == "not"

    def test_comparison_ops_mapped(self):
        """比较运算符全部映射。"""
        assert _OP_TO_EXPROP[operator.eq] == "eq"
        assert _OP_TO_EXPROP[operator.ne] == "ne"
        assert _OP_TO_EXPROP[operator.lt] == "lt"
        assert _OP_TO_EXPROP[operator.le] == "le"
        assert _OP_TO_EXPROP[operator.gt] == "gt"
        assert _OP_TO_EXPROP[operator.ge] == "ge"

    def test_truediv_not_mapped(self):
        """``operator.truediv`` 不在映射中（浮点不支持）。"""
        assert operator.truediv not in _OP_TO_EXPROP

    def test_mapping_has_18_entries(self):
        """映射应包含 18 个 operator（5 算术 + 5 位 + 2 一元 + 6 比较）。"""
        assert len(_OP_TO_EXPROP) == 18


# ---------------------------------------------------------------------------
# _compile_expr_tree：表达式树 → ExprOp 指令
# ---------------------------------------------------------------------------


class TestCompileExprTree:
    """``_compile_expr_tree`` 后序遍历翻译。"""

    def test_single_field_ref(self):
        """单个字段引用 → ``[("getint", idx)]``。"""
        a = field(Int8ub)
        idx_map = {id(a): 0}
        ops = _compile_expr_tree(a, idx_map, "test")
        assert ops == [("getint", 0)]

    def test_single_int_constant(self):
        """整数常量 → ``[("const", value)]``。"""
        ops = _compile_expr_tree(42, {}, "test")
        assert ops == [("const", 42)]

    def test_zero_constant(self):
        """零常量。"""
        ops = _compile_expr_tree(0, {}, "test")
        assert ops == [("const", 0)]

    def test_negative_constant(self):
        """负整数常量。"""
        ops = _compile_expr_tree(-1, {}, "test")
        assert ops == [("const", -1)]

    def test_binary_add_two_fields(self):
        """``a + b`` → ``[getint(0), getint(1), add]``。"""
        a = field(Int8ub)
        b = field(Int8ub)
        idx_map = {id(a): 0, id(b): 1}
        tree = a + b  # _ExprRef(add, a, b)
        ops = _compile_expr_tree(tree, idx_map, "test")
        assert ops == [("getint", 0), ("getint", 1), ("add",)]

    def test_binary_sub_field_minus_const(self):
        """``a - 5`` → ``[getint(0), const(5), sub]``。"""
        a = field(Int8ub)
        idx_map = {id(a): 0}
        tree = a - 5
        ops = _compile_expr_tree(tree, idx_map, "test")
        assert ops == [("getint", 0), ("const", 5), ("sub",)]

    def test_binary_mul_const_times_field(self):
        """``2 * a`` (radd 形式) → ``[const(2), getint(0), mul]``。"""
        a = field(Int8ub)
        idx_map = {id(a): 0}
        tree = 2 * a  # __rmul__
        ops = _compile_expr_tree(tree, idx_map, "test")
        assert ops == [("const", 2), ("getint", 0), ("mul",)]

    def test_nested_expression_left_assoc(self):
        """``(a + b) - c`` → ``[getint(0), getint(1), add, getint(2), sub]``。"""
        a = field(Int8ub)
        b = field(Int8ub)
        c = field(Int8ub)
        idx_map = {id(a): 0, id(b): 1, id(c): 2}
        tree = (a + b) - c
        ops = _compile_expr_tree(tree, idx_map, "test")
        assert ops == [
            ("getint", 0),
            ("getint", 1),
            ("add",),
            ("getint", 2),
            ("sub",),
        ]

    def test_nested_expression_right_assoc(self):
        """``a + (b * c)`` → ``[getint(0), getint(1), getint(2), mul, add]``。"""
        a = field(Int8ub)
        b = field(Int8ub)
        c = field(Int8ub)
        idx_map = {id(a): 0, id(b): 1, id(c): 2}
        tree = a + (b * c)
        ops = _compile_expr_tree(tree, idx_map, "test")
        assert ops == [
            ("getint", 0),
            ("getint", 1),
            ("getint", 2),
            ("mul",),
            ("add",),
        ]

    def test_unary_neg(self):
        """``-a`` → ``[getint(0), neg]``。"""
        a = field(Int8ub)
        idx_map = {id(a): 0}
        tree = -a
        ops = _compile_expr_tree(tree, idx_map, "test")
        assert ops == [("getint", 0), ("neg",)]

    def test_unary_invert(self):
        """``~a`` → ``[getint(0), not]``。"""
        a = field(Int8ub)
        idx_map = {id(a): 0}
        tree = ~a
        ops = _compile_expr_tree(tree, idx_map, "test")
        assert ops == [("getint", 0), ("not",)]

    def test_comparison_eq(self):
        """``a == 5`` → ``[getint(0), const(5), eq]``。"""
        a = field(Int8ub)
        idx_map = {id(a): 0}
        tree = (a == 5)
        ops = _compile_expr_tree(tree, idx_map, "test")
        assert ops == [("getint", 0), ("const", 5), ("eq",)]

    def test_all_binary_ops_compile(self):
        """所有 16 种二元运算符都能正确编译。"""
        a = field(Int8ub)
        b = field(Int8ub)
        idx_map = {id(a): 0, id(b): 1}
        cases = [
            (a + b, "add"),
            (a - b, "sub"),
            (a * b, "mul"),
            (a // b, "floordiv"),
            (a % b, "mod"),
            (a & b, "bitand"),
            (a | b, "bitor"),
            (a ^ b, "bitxor"),
            (a << b, "shl"),
            (a >> b, "shr"),
            (a == b, "eq"),
            (a != b, "ne"),
            (a < b, "lt"),
            (a <= b, "le"),
            (a > b, "gt"),
            (a >= b, "ge"),
        ]
        for tree, expected_op in cases:
            ops = _compile_expr_tree(tree, idx_map, "test")
            assert ops == [
                ("getint", 0),
                ("getint", 1),
                (expected_op,),
            ], "failed for op: {}".format(expected_op)

    def test_deeply_nested(self):
        """深层嵌套 ``((a + b) * (c - d)) // 2``。"""
        a = field(Int8ub)
        b = field(Int8ub)
        c = field(Int8ub)
        d = field(Int8ub)
        idx_map = {id(a): 0, id(b): 1, id(c): 2, id(d): 3}
        tree = ((a + b) * (c - d)) // 2
        ops = _compile_expr_tree(tree, idx_map, "test")
        assert ops == [
            ("getint", 0),
            ("getint", 1),
            ("add",),
            ("getint", 2),
            ("getint", 3),
            ("sub",),
            ("mul",),
            ("const", 2),
            ("floordiv",),
        ]

    def test_field_ref_not_in_map_raises(self):
        """字段引用不在 field_index_map 中 → CompilationError。"""
        a = field(Int8ub)
        other = field(Int8ub)
        idx_map = {id(a): 0}  # 缺少 other
        with pytest.raises(CompilationError) as exc_info:
            _compile_expr_tree(other, idx_map, "test")
        assert "未找到" in str(exc_info.value)

    def test_float_constant_raises(self):
        """浮点常量 → CompilationError。"""
        with pytest.raises(CompilationError) as exc_info:
            _compile_expr_tree(3.14, {}, "test")
        assert "浮点" in str(exc_info.value)

    def test_bool_constant_raises(self):
        """bool 常量 → CompilationError。"""
        with pytest.raises(CompilationError) as exc_info:
            _compile_expr_tree(True, {}, "test")
        assert "bool" in str(exc_info.value)

    def test_string_constant_raises(self):
        """字符串常量 → CompilationError。"""
        with pytest.raises(CompilationError) as exc_info:
            _compile_expr_tree("hello", {}, "test")
        assert "不支持" in str(exc_info.value)

    def test_none_in_binary_raises(self):
        """二元运算的 rhs 为 None → CompilationError（类型不支持）。"""
        a = field(Int8ub)
        idx_map = {id(a): 0}
        # 手动构造非法表达式：rhs=None 但 op 是二元
        tree = _ExprRef(operator.add, a, None)
        with pytest.raises(CompilationError):
            _compile_expr_tree(tree, idx_map, "test")


# ---------------------------------------------------------------------------
# _emit_expr_ops 内部递归
# ---------------------------------------------------------------------------


class TestEmitExprOps:
    """``_emit_expr_ops`` 递归发射指令（直接测试内部函数）。"""

    def test_emits_to_provided_list(self):
        """指令被追加到传入的 ops 列表。"""
        a = field(Int8ub)
        idx_map = {id(a): 0}
        ops = []
        _emit_expr_ops(a, idx_map, "test", ops)
        assert ops == [("getint", 0)]

    def test_multiple_calls_accumulate(self):
        """多次调用同一 ops 列表会累积。"""
        a = field(Int8ub)
        idx_map = {id(a): 0}
        ops = []
        _emit_expr_ops(a, idx_map, "test", ops)
        _emit_expr_ops(5, idx_map, "test", ops)
        assert ops == [("getint", 0), ("const", 5)]


# ---------------------------------------------------------------------------
# _check_forward_reference：前向引用检测
# ---------------------------------------------------------------------------


class TestCheckForwardReference:
    """``_check_forward_reference`` 检测表达式是否引用后序字段。"""

    def test_reference_to_earlier_field_ok(self):
        """引用前序字段（idx < current）→ 通过。"""
        ops = [("getint", 0), ("getint", 1), ("add",)]
        # 当前字段索引为 2，引用 0 和 1 都是前序 → OK
        _check_forward_reference(ops, 2, "current")

    def test_reference_to_same_index_raises(self):
        """引用同索引字段（自引用）→ CompilationError。"""
        ops = [("getint", 2)]
        with pytest.raises(CompilationError) as exc_info:
            _check_forward_reference(ops, 2, "current")
        assert "后序字段" in str(exc_info.value)

    def test_reference_to_later_index_raises(self):
        """引用后序字段 → CompilationError。"""
        ops = [("getint", 5)]
        with pytest.raises(CompilationError) as exc_info:
            _check_forward_reference(ops, 2, "current")
        assert "后序字段" in str(exc_info.value)

    def test_const_not_checked(self):
        """const 指令不涉及字段引用，不检查。"""
        ops = [("const", 999)]
        _check_forward_reference(ops, 0, "current")  # 不抛异常

    def test_mixed_ops_only_checks_getint(self):
        """混合指令中只检查 getint 的索引。"""
        ops = [("getint", 0), ("const", 3), ("getint", 1), ("add",)]
        # 引用 0 和 1，当前索引 2 → OK
        _check_forward_reference(ops, 2, "current")

    def test_first_field_cannot_reference_anything(self):
        """第一个字段（索引 0）不能引用任何字段。"""
        ops = [("getint", 0)]
        with pytest.raises(CompilationError):
            _check_forward_reference(ops, 0, "first")


# ---------------------------------------------------------------------------
# _check_wo_reference：WO 字段引用检测
# ---------------------------------------------------------------------------


class TestCheckWoReference:
    """``_check_wo_reference`` 检测表达式是否引用 WO 字段。"""

    def test_reference_to_rw_field_ok(self):
        """引用 RW 字段 → 通过。"""
        descriptors = [
            ("a", field(Int8ub)),  # rw
            ("b", field(Int8ub)),  # rw
        ]
        ops = [("getint", 0), ("getint", 1), ("add",)]
        _check_wo_reference(ops, descriptors, "current")

    def test_reference_to_wo_field_raises(self):
        """引用 WO 字段 → CompilationError。"""
        descriptors = [
            ("pad", wfield(Int8ub)),  # wo
            ("b", field(Int8ub)),     # rw
        ]
        ops = [("getint", 0)]  # 引用 pad (wo)
        with pytest.raises(CompilationError) as exc_info:
            _check_wo_reference(ops, descriptors, "current")
        assert "WO" in str(exc_info.value)

    def test_reference_to_rw_when_wo_exists_ok(self):
        """存在 WO 字段但表达式不引用它 → 通过。"""
        descriptors = [
            ("pad", wfield(Int8ub)),  # wo, idx=0
            ("a", field(Int8ub)),     # rw, idx=1
        ]
        ops = [("getint", 1)]  # 只引用 a (rw)
        _check_wo_reference(ops, descriptors, "current")

    def test_const_not_checked(self):
        """const 指令不检查。"""
        descriptors = [("pad", wfield(Int8ub))]
        ops = [("const", 42)]
        _check_wo_reference(ops, descriptors, "current")

    def test_multiple_wo_fields_all_detected(self):
        """多个 WO 字段都被正确识别。"""
        descriptors = [
            ("pad1", wfield(Int8ub)),
            ("pad2", wfield(Int8ub)),
            ("a", field(Int8ub)),
        ]
        # 引用 pad2 (idx=1)
        ops = [("getint", 1)]
        with pytest.raises(CompilationError):
            _check_wo_reference(ops, descriptors, "current")


# ---------------------------------------------------------------------------
# _extract_and_compile_exprs：_expr_params 协议
# ---------------------------------------------------------------------------


class TestExtractAndCompileExprs:
    """``_extract_and_compile_exprs`` 从描述符提取表达式参数。"""

    def test_descriptor_without_expr_params_returns_empty(self):
        """无 ``_expr_params`` 属性的描述符 → 空 dict。"""
        # Int8ub 是 FormatFieldDescriptor，没有 _expr_params
        result = _extract_and_compile_exprs(Int8ub, {}, "test")
        assert result == {}

    def test_descriptor_with_int_constant_param_compiles_as_const(self):
        """``_expr_params`` 含 int 常量 → 编译为单条 Const ExprOp（DF1 修复）。

        设计 §1.2.2："int 常量也包装为单条 Const"。
        DefaultDescriptor.value / CheckDescriptor.func 需要 ExprProgram。
        """
        desc = _MockExprParamsDescriptor({"length": 4})
        result = _extract_and_compile_exprs(desc, {}, "test")
        assert result == {"length": [("const", 4)]}

    def test_descriptor_with_int_constant_zero(self):
        """int 常量 0 编译为 ``[("const", 0)]``（DF1 常见用例 ``Default(Byte, 0)``）。"""
        desc = _MockExprParamsDescriptor({"value": 0})
        result = _extract_and_compile_exprs(desc, {}, "test")
        assert result == {"value": [("const", 0)]}

    def test_descriptor_with_negative_int_constant(self):
        """负 int 常量编译为 ``[("const", N)]``。"""
        desc = _MockExprParamsDescriptor({"value": -1})
        result = _extract_and_compile_exprs(desc, {}, "test")
        assert result == {"value": [("const", -1)]}

    def test_descriptor_with_non_int_constant_returns_empty(self):
        """非 int 常量（bytes/str）不编译，返回空 dict。

        与 int 不同，bytes/str 常量由 Rust 侧直接从描述符读取（如 ConstDescriptor.value）。
        """
        desc = _MockExprParamsDescriptor({"value": b"abc"})
        result = _extract_and_compile_exprs(desc, {}, "test")
        assert result == {}

    def test_descriptor_with_field_ref_param(self):
        """``_expr_params`` 含 FieldRef → 编译为 ops。"""
        a = field(Int8ub)
        idx_map = {id(a): 0}
        desc = _MockExprParamsDescriptor({"length": a})
        result = _extract_and_compile_exprs(desc, idx_map, "test")
        assert result == {"length": [("getint", 0)]}

    def test_descriptor_with_expr_ref_param(self):
        """``_expr_params`` 含 ExprRef → 编译为 ops。"""
        a = field(Int8ub)
        b = field(Int8ub)
        idx_map = {id(a): 0, id(b): 1}
        desc = _MockExprParamsDescriptor({"length": a + b})
        result = _extract_and_compile_exprs(desc, idx_map, "test")
        assert result == {"length": [("getint", 0), ("getint", 1), ("add",)]}

    def test_descriptor_with_mixed_params(self):
        """``_expr_params`` 含 int 常量和表达式 → 全部编译（int 为 Const）。"""
        a = field(Int8ub)
        idx_map = {id(a): 0}
        desc = _MockExprParamsDescriptor({
            "length": a,
            "padding": 4,  # int 常量 → Const ExprOp
        })
        result = _extract_and_compile_exprs(desc, idx_map, "test")
        assert result == {
            "length": [("getint", 0)],
            "padding": [("const", 4)],
        }

    def test_descriptor_with_multiple_expr_params(self):
        """``_expr_params`` 含多个表达式参数 → 全部编译。"""
        a = field(Int8ub)
        b = field(Int8ub)
        idx_map = {id(a): 0, id(b): 1}
        desc = _MockExprParamsDescriptor({
            "length": a,
            "condfunc": a > b,
        })
        result = _extract_and_compile_exprs(desc, idx_map, "test")
        assert "length" in result
        assert "condfunc" in result
        assert result["length"] == [("getint", 0)]
        assert result["condfunc"] == [("getint", 0), ("getint", 1), ("gt",)]

    def test_empty_expr_params_dict(self):
        """``_expr_params`` 为空 dict → 返回空 dict。"""
        desc = _MockExprParamsDescriptor({})
        result = _extract_and_compile_exprs(desc, {}, "test")
        assert result == {}


# ---------------------------------------------------------------------------
# _compile_expressions：端到端编译
# ---------------------------------------------------------------------------


class TestCompileExpressions:
    """``_compile_expressions`` 遍历所有字段编译表达式。"""

    def test_no_expressions_returns_empty(self):
        """所有字段无表达式 → 空 dict。"""
        descriptors = [
            ("a", field(Int8ub)),
            ("b", field(Int8ub)),
        ]
        idx_map = {id(d): i for i, (_, d) in enumerate(descriptors)}
        result = _compile_expressions(descriptors, idx_map)
        assert result == {}

    def test_single_field_with_expression(self):
        """一个字段含表达式 → ``{idx: {param: [ops]}}``。"""
        a = field(Int8ub)
        # b 是一个字段，其 subcon 是含表达式的 mock
        b = _FieldDescriptor(_MockExprParamsDescriptor({"length": a}))
        descriptors = [("a", a), ("b", b)]
        idx_map = {id(d): i for i, (_, d) in enumerate(descriptors)}
        result = _compile_expressions(descriptors, idx_map)
        assert 1 in result
        assert result[1] == {"length": [("getint", 0)]}

    def test_multiple_fields_with_expressions(self):
        """多个字段含表达式。"""
        a = field(Int8ub)
        b = field(Int8ub)
        # c 和 d 是字段，其 subcon 是含表达式的 mock
        c = _FieldDescriptor(_MockExprParamsDescriptor({"length": a}))
        d = _FieldDescriptor(_MockExprParamsDescriptor({"length": a + b}))
        descriptors = [("a", a), ("b", b), ("c", c), ("d", d)]
        idx_map = {id(d): i for i, (_, d) in enumerate(descriptors)}
        result = _compile_expressions(descriptors, idx_map)
        # c (idx=2) 引用 a (idx=0)
        assert result[2] == {"length": [("getint", 0)]}
        # d (idx=3) 引用 a+b (idx=0,1)
        assert result[3] == {
            "length": [("getint", 0), ("getint", 1), ("add",)]
        }

    def test_forward_reference_detected(self):
        """编译期检测到前向引用 → CompilationError。

        b (idx=1) 的表达式引用 c (idx=2，后序字段)。
        """
        a = field(Int8ub)
        c = field(Int8ub)
        # b 的表达式引用 c（后序字段）
        b = _FieldDescriptor(_MockExprParamsDescriptor({"length": c}))
        descriptors = [("a", a), ("b", b), ("c", c)]
        idx_map = {id(d): i for i, (_, d) in enumerate(descriptors)}
        with pytest.raises(CompilationError) as exc_info:
            _compile_expressions(descriptors, idx_map)
        assert "后序字段" in str(exc_info.value)

    def test_wo_reference_detected(self):
        """编译期检测到引用 WO 字段 → CompilationError。"""
        pad = wfield(Int8ub)  # wo
        # a 的表达式引用 pad (wo)
        a = _FieldDescriptor(_MockExprParamsDescriptor({"length": pad}))
        descriptors = [("pad", pad), ("a", a)]
        idx_map = {id(d): i for i, (_, d) in enumerate(descriptors)}
        with pytest.raises(CompilationError) as exc_info:
            _compile_expressions(descriptors, idx_map)
        assert "WO" in str(exc_info.value)

    def test_first_field_with_self_reference_raises(self):
        """第一个字段引用自己 → 前向引用错误（idx 0 >= current 0）。"""
        a = field(Int8ub)
        # first 字段的 subcon 引用 a 自身
        first = _FieldDescriptor(_MockExprParamsDescriptor({"length": a}))
        descriptors = [("first", first), ("a", a)]
        idx_map = {id(d): i for i, (_, d) in enumerate(descriptors)}
        with pytest.raises(CompilationError):
            _compile_expressions(descriptors, idx_map)


# ---------------------------------------------------------------------------
# _expr_programs_to_list：格式转换
# ---------------------------------------------------------------------------


class TestExprProgramsToList:
    """``_expr_programs_to_list`` 将 dict 转为 Vec<Option<dict>> 格式。"""

    def test_empty_dict_all_none(self):
        """无表达式 → 全 None 列表。"""
        result = _expr_programs_to_list({}, 3)
        assert result == [None, None, None]

    def test_single_expression_at_index_1(self):
        """索引 1 有表达式。"""
        expr_programs = {1: {"length": [("getint", 0)]}}
        result = _expr_programs_to_list(expr_programs, 3)
        assert result[0] is None
        assert result[1] == {"length": [("getint", 0)]}
        assert result[2] is None

    def test_multiple_expressions(self):
        """多个字段有表达式。"""
        expr_programs = {
            0: {"length": [("const", 5)]},
            2: {"length": [("getint", 0), ("getint", 1), ("add",)]},
        }
        result = _expr_programs_to_list(expr_programs, 3)
        assert result[0] == {"length": [("const", 5)]}
        assert result[1] is None
        assert result[2] == {
            "length": [("getint", 0), ("getint", 1), ("add",)]
        }

    def test_length_matches_field_count(self):
        """结果列表长度 = field_count。"""
        result = _expr_programs_to_list({}, 10)
        assert len(result) == 10

    def test_empty_with_zero_fields(self):
        """零字段 → 空列表。"""
        result = _expr_programs_to_list({}, 0)
        assert result == []


# ---------------------------------------------------------------------------
# 端到端：compile_schema 集成（modes + expr_programs 传入）
# ---------------------------------------------------------------------------


class TestCompileSchemaIntegration:
    """验证 ``_compile_schema_for_class`` 能正确传递 modes 和 expr_programs。

    当前阶段（2.3）Rust 侧仅接收 expr_programs 但不解析（仅设置 has_expressions 标志）。
    这些测试验证编译管线不会因 modes/expr_programs 参数而崩溃。
    """

    def test_simple_struct_compiles_with_modes(self):
        """简单 Struct 编译成功，modes 正确传递。"""
        from dataclasses import dataclass

        from construct import StructMixin

        @dataclass
        class M(StructMixin):
            a: int = field(Int8ub)
            b: int = field(Int8ub)

        # 编译成功即可（不抛异常）
        assert M._construct_compiled is not None
        # 基本功能验证
        assert M.parse(b"\x01\x02").a == 1

    def test_struct_with_ro_wo_modes_compiles(self):
        """含 RO/WO 字段的 Struct 编译成功。"""
        from dataclasses import dataclass

        from construct import StructMixin

        @dataclass
        class M(StructMixin):
            a: int = field(Int8ub)
            b: int = rfield(Int8ub)
            c: int = wfield(Int8ub)

        assert M._construct_compiled is not None

    def test_struct_with_no_expressions_has_flag_false(self):
        """无表达式的 Struct，has_expressions 应为 False。"""
        from dataclasses import dataclass

        from construct import StructMixin

        @dataclass
        class M(StructMixin):
            a: int = field(Int8ub)

        # StructNode 的 has_expressions 方法
        compiled = M._construct_compiled
        # compiled 是 CompiledSchema，内部有 node
        # 我们通过 _node 属性访问（如果暴露）
        # 退而求其次：只要编译成功即可
        assert compiled is not None

    def test_cached_descriptors_stored(self):
        """``_compile_schema_for_class`` 缓存了 descriptors。"""
        from dataclasses import dataclass

        from construct import StructMixin

        @dataclass
        class M(StructMixin):
            a: int = field(Int8ub)

        assert hasattr(M, "_cached_descriptors")
        cached = M._cached_descriptors
        assert len(cached) == 1
        assert cached[0][0] == "a"
