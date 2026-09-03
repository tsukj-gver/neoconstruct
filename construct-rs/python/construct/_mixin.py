"""StructMixin 基类、``field()``/``rfield()``/``wfield()`` 声明函数、
表达式类型系统（FieldRef/ExprRef）与 Schema 编译管线（纯 Python 实现）。

核心流程：

1. 用户定义 ``@dataclass class X(StructMixin): ...``
2. Python 创建类对象后立即调用 ``StructMixin.__init_subclass__(X)``
3. ``__init_subclass__`` 从类体收集 ``field()``/``rfield()``/``wfield()`` 声明，
   调用 Rust ``compile_schema``
4. 编译产物存为 ``X._construct_compiled``（类属性，零开销查找）
5. ``_apply_dataclass_field_config`` 将 ``_FieldDescriptor`` 替换为
   ``dataclasses.field()`` 配置（RO→init=False，有 default→kw_only=True，
   v0.1.1 起值提供型 subcon→隐式 default=None + kw_only=True）
6. ``@dataclass`` 装饰器随后执行（生成 ``__init__`` 等）

关键约束：``__init_subclass__`` 在 ``@dataclass`` **之前**执行，因此不能
依赖 ``__dataclass_fields__``（尚不存在），从类体原始属性提取字段信息。
"""

import dataclasses
import operator
import threading
import weakref

from ._errors import CompilationError, ConstructError

# ---------------------------------------------------------------------------
# Rust 扩展导入
# ---------------------------------------------------------------------------

# 延迟导入 Rust 扩展的 compile_schema 函数。
# 若扩展未构建（如纯 Python 开发），_compile_schema 为 None，在实际编译时抛出
# 明确错误而非导入时失败。
try:
    from ._construct_rust import compile_schema as _compile_schema
except ImportError:  # pragma: no cover - 仅在扩展未构建时触发
    _compile_schema = None


# ---------------------------------------------------------------------------
# 哨兵值
# ---------------------------------------------------------------------------

_MISSING = object()
"""哨兵值：表示 field() 未指定 ``default`` 参数。

用于区分 ``field(Int8ub)``（无默认值）与 ``field(Int8ub, default=None)``
（默认值为 None）。采用全局单例 object() 保证 identity 唯一性。
"""


# ---------------------------------------------------------------------------
# 表达式类型系统：_ExprMixin / _FieldDescriptor / _ExprRef
# ---------------------------------------------------------------------------


class _ExprMixin:
    """FieldRef/ExprRef 共享的算术运算符重载。

    ``_FieldDescriptor`` 和 ``_ExprRef`` 都继承此类，使字段引用和表达式节点
    都能通过 Python 运算符构建表达式树。每次二元运算返回新的 ``_ExprRef`` 节点。

    运算符支持矩阵（映射到 ExprOp）：

    ============  ======  ===========================================
    分类           运算符   ExprOp
    ============  ======  ===========================================
    算术           + - *   Add / Sub / Mul
    算术           // %    FloorDiv / Mod
    位运算         &       BitAnd
    位运算         |       BitOr
    位运算         ^       BitXor
    位运算         << >>   Shl / Shr
    一元           - ~     Neg / Not
    比较           == !=   Eq / Ne
    比较           < <=    Lt / Le
    比较           > >=    Gt / Ge
    ============  ======  ===========================================

    **不支持** ``__truediv__``（``/``）：结果可能为浮点，VM 栈为 i64。
    **不支持** ``and``/``or``：Python 短路求值无法重载。
    """

    __slots__ = ()

    # --- 二元算术运算 ---
    def __add__(self, other):
        return _ExprRef(operator.add, self, other)

    def __radd__(self, other):
        return _ExprRef(operator.add, other, self)

    def __sub__(self, other):
        return _ExprRef(operator.sub, self, other)

    def __rsub__(self, other):
        return _ExprRef(operator.sub, other, self)

    def __mul__(self, other):
        return _ExprRef(operator.mul, self, other)

    def __rmul__(self, other):
        return _ExprRef(operator.mul, other, self)

    def __floordiv__(self, other):
        return _ExprRef(operator.floordiv, self, other)

    def __rfloordiv__(self, other):
        return _ExprRef(operator.floordiv, other, self)

    def __mod__(self, other):
        return _ExprRef(operator.mod, self, other)

    def __rmod__(self, other):
        return _ExprRef(operator.mod, other, self)

    # --- 位运算 ---
    def __and__(self, other):
        return _ExprRef(operator.and_, self, other)

    def __or__(self, other):
        return _ExprRef(operator.or_, self, other)

    def __xor__(self, other):
        return _ExprRef(operator.xor, self, other)

    def __lshift__(self, other):
        return _ExprRef(operator.lshift, self, other)

    def __rshift__(self, other):
        return _ExprRef(operator.rshift, self, other)

    # --- 一元运算 ---
    def __neg__(self):
        return _ExprRef(operator.neg, self, None)

    def __pos__(self):
        return self  # 正号无操作

    def __invert__(self):
        return _ExprRef(operator.invert, self, None)

    # --- 比较运算（返回 _ExprRef，编译为 Eq/Lt/Gt 等，VM 返回 0/1） ---
    def __eq__(self, other):
        return _ExprRef(operator.eq, self, other)

    def __ne__(self, other):
        return _ExprRef(operator.ne, self, other)

    def __lt__(self, other):
        return _ExprRef(operator.lt, self, other)

    def __le__(self, other):
        return _ExprRef(operator.le, self, other)

    def __gt__(self, other):
        return _ExprRef(operator.gt, self, other)

    def __ge__(self, other):
        return _ExprRef(operator.ge, self, other)

    def __class_getitem__(cls, item):
        """支持 ``FieldRef[int]`` / ``ExprRef[int]`` 语法（T 仅用于 IDE 提示）。"""
        return cls

    __hash__ = None  # 表达式不可哈希（与 Python construct 一致）


class _FieldDescriptor(_ExprMixin):
    """``field()`` / ``rfield()`` / ``wfield()`` 的返回值。

    同时满足三套协议：

    - **@dataclass 字段协议**：作为字段的默认值。``@dataclass`` 将其视为普通默认值，
      用户构造实例时传入真正的值（如 ``MyMsg(address=1)``），默认值不参与热路径。
    - **Mixin 编译协议**：``__init_subclass__`` 从类体提取 ``_FieldDescriptor`` 实例，
      读取其 ``subcon`` / ``mode`` / ``default`` 属性配置编译。
    - **FieldRef 协议**：可被其他字段的表达式引用（如 ``Bytes(count)`` 中的 ``count``）。
      通过对象 identity（``id()``）唯一标识，编译期建立 ``id→field_index`` 映射。
      该对象支持算术运算重载（``__add__`` 等），返回 ``_ExprRef``。
    """

    __slots__ = ("subcon", "mode", "default", "name", "_context_injections")

    def __init__(self, subcon, *, mode="rw", default=_MISSING, context=None):
        """初始化字段描述符。

        :param subcon: 类型描述符对象（``Int8ub``、``Bytes(n)``、``GreedyBytes``、
                        或另一个 ``StructMixin`` 子类）。
        :param mode: 字段模式，``"rw"``（读写，默认）/ ``"ro"``（只读）/ ``"wo"``（只写）。
        :param default: 可选默认值。``_MISSING`` 表示未指定（默认无默认值）。
        :param context: 可选，跨层引用的 context 注入映射（``context=`` 参数）。
        """
        self.subcon = subcon
        self.mode = mode
        self.default = default
        self.name = None  # __set_name__ 填充（用于错误信息）
        self._context_injections = context

    def __set_name__(self, owner, name):
        """Python 3.6+ 描述符协议：类创建时自动调用，填充字段名。"""
        self.name = name

    def __repr__(self):
        return "_FieldDescriptor(name={!r}, mode={!r}, subcon={!r})".format(
            self.name, self.mode, self.subcon
        )


class _ExprRef(_ExprMixin):
    """表达式节点，编译期翻译为 ExprOp 指令序列。

    ``_ExprRef`` 形成一棵二叉表达式树：

    - ``lhs`` / ``rhs`` 可以是 ``_FieldDescriptor``、``_ExprRef`` 或常量（int）
    - ``op`` 是 ``operator`` 模块的函数（``operator.add`` 等）
    - 一元运算时 ``rhs=None``

    ``_ExprRef`` 本身也重载算术运算，支持无限嵌套。
    通过对象 identity 与 ``_FieldDescriptor`` 区分。
    """

    __slots__ = ("op", "lhs", "rhs")

    def __init__(self, op, lhs, rhs):
        """初始化表达式节点。

        :param op: ``operator`` 模块的函数（``operator.add`` 等）。
        :param lhs: 左操作数（``_FieldDescriptor`` / ``_ExprRef`` / int）。
        :param rhs: 右操作数（同 lhs），一元运算时为 ``None``。
        """
        self.op = op
        self.lhs = lhs
        self.rhs = rhs

    def __repr__(self):
        return "_ExprRef(op={!r})".format(getattr(self.op, "__name__", self.op))


# ---------------------------------------------------------------------------
# field() / rfield() / wfield() 声明函数
# ---------------------------------------------------------------------------


def field(subcon, *, default=_MISSING, context=None):
    """声明一个 RW 字段（build 要填，parse 能读）。

    用法::

        @dataclass
        class MyMsg(StructMixin):
            address: int = field(Int8ub)
            data: bytes = field(Bytes(4))
            version: int = field(Int8ub, default=1)  # 有 default → kw_only=True

    :param subcon: 类型描述符，决定该字段的二进制格式。必须是框架已知的描述符类型
        （``Int8ub`` 等 16 种单例、``Bytes(n)``、``GreedyBytes``、或另一个
        ``StructMixin`` 子类）。
    :param default: 可选默认值。指定后自动设为 ``kw_only=True``。
    :param context: 可选，跨层引用的 context 注入映射。
    :return: ``_FieldDescriptor(mode="rw")`` 对象，作为类属性默认值。

    字段顺序由 Python 3.7+ ``__annotations__`` 的插入顺序保证（PEP 526）。
    """
    return _FieldDescriptor(subcon, mode="rw", default=default, context=context)


def rfield(subcon, *, context=None):
    """声明一个 RO 字段（build 不用填，parse 能读）。

    RO 字段在 build 时自动计算（不从实例取值）：
    - ``Tell()``：build 时取流位置
    - ``Computed(expr)``：build 时求值表达式
    - ``Const(subcon, value)``：build 时用常量值

    RO 字段的 ``init=False``，不出现在 ``__init__`` 参数中。

    :param subcon: 类型描述符。
    :param context: 可选，跨层引用的 context 注入映射。
    :return: ``_FieldDescriptor(mode="ro")``。
    """
    return _FieldDescriptor(subcon, mode="ro", context=context)


def wfield(subcon, *, default=_MISSING, context=None):
    """声明一个 WO 字段（build 要填，parse 读不到）。

    parse 时正常解析（消费字节），但结果不存入实例属性（丢弃）。
    典型场景：padding、reserved 字段。

    :param subcon: 类型描述符。
    :param default: 可选默认值。指定后自动设为 ``kw_only=True``。
    :param context: 可选，跨层引用的 context 注入映射。
    :return: ``_FieldDescriptor(mode="wo")``。
    """
    return _FieldDescriptor(subcon, mode="wo", default=default, context=context)


# ---------------------------------------------------------------------------
# Tell / Computed 描述符
#
# 这些描述符是纯 Python 类（不需要 Rust pyclass），通过 type name 识别。
# compile_schema 的 build_node_from_descriptor 通过 ``type(desc).__name__``
# 匹配到对应的 Node 变体。
# ---------------------------------------------------------------------------


class TellDescriptor:
    """记录当前流位置的描述符。无参数。

    使用方式：``rfield(Tell())``。

    对应 Python construct 的 ``Tell``。parse 时返回当前流位置，
    build 时位置由 StructNode 的 ``compute_ro_value`` 处理（不入流）。

    ``_expr_params`` 协议返回空 dict：Tell 无表达式参数。
    """

    __slots__ = ()

    # 类级别常量（协议实现：无表达式参数）。
    _expr_params = {}

    def __repr__(self):
        return "Tell()"


def Tell():
    """创建一个 Tell 描述符。

    使用方式::

        @dataclass
        class Packet(StructMixin):
            start: int = rfield(Tell())
            count: int = field(Int8ub)
            end: int = rfield(Tell())

    :return: ``TellDescriptor`` 实例。
    """
    return TellDescriptor()


class ComputedDescriptor:
    """从表达式计算值的描述符。

    使用方式：``rfield(Computed(end - start))``。

    对应 Python construct 的 ``Computed``。parse 时通过表达式 VM 求值，
    build 时由 StructNode 的 ``compute_ro_value`` 调用同样的求值逻辑。

    ``_expr_params`` 协议返回 ``{"func": <expr>}``：编译期将 expr
    （``_FieldDescriptor`` / ``_ExprRef``）翻译为 ExprOp 指令列表，
    存入 expr_programs 的 "func" 键。
    """

    __slots__ = ("expr",)

    def __init__(self, expr):
        """初始化 Computed 描述符。

        :param expr: 计算表达式（``_FieldDescriptor`` / ``_ExprRef``）。
            必须是引用当前 Struct 前序字段的表达式，求值结果为整数。
        """
        self.expr = expr

    @property
    def _expr_params(self):
        return {"func": self.expr}

    def __repr__(self):
        return "Computed({!r})".format(self.expr)


def Computed(expr):
    """创建一个 Computed 描述符。

    使用方式::

        @dataclass
        class Packet(StructMixin):
            start: int = rfield(Tell())
            end: int = rfield(Tell())
            size: int = rfield(Computed(end - start))

    :param expr: 计算表达式（FieldRef/ExprRef）。
    :return: ``ComputedDescriptor`` 实例。
    """
    return ComputedDescriptor(expr)


# ---------------------------------------------------------------------------
# 字段信息收集
# ---------------------------------------------------------------------------


def _collect_field_descriptors(cls):
    """从类体收集字段信息。

    遍历 ``cls.__annotations__``（Python 3.7+ 保序），提取其中 ``_FieldDescriptor``
    类型的默认值。不依赖 ``__dataclass_fields__``（此刻尚未生成）。

    :param cls: 刚创建的 StructMixin 子类。
    :return: ``[(field_name, descriptor), ...]`` 有序列表，顺序与字段声明顺序一致。
             每项的 descriptor 是完整的 ``_FieldDescriptor`` 对象（含 mode、default 等）。
    """
    annotations = getattr(cls, "__annotations__", {})
    fields = []
    for name in annotations:
        # 仅检查 cls 自身的 __dict__（不含继承的属性）
        default = cls.__dict__.get(name)
        if isinstance(default, _FieldDescriptor):
            fields.append((name, default))
    return fields


# ---------------------------------------------------------------------------
# 表达式编译
# ---------------------------------------------------------------------------

# operator 模块函数 → ExprOp 指令名称的映射。
#
# 编译期将 _ExprRef.op（operator.add 等）翻译为对应的 ExprOp 名称字符串，
# Rust 侧根据字符串构建 ExprOp 枚举变体。
#
# 不在此映射中的 operator 函数（如 operator.truediv）→ 编译期 CompilationError。
_OP_TO_EXPROP = {
    operator.add: "add",
    operator.sub: "sub",
    operator.mul: "mul",
    operator.floordiv: "floordiv",
    operator.mod: "mod",
    operator.and_: "bitand",
    operator.or_: "bitor",
    operator.xor: "bitxor",
    operator.lshift: "shl",
    operator.rshift: "shr",
    operator.neg: "neg",
    operator.invert: "not",
    operator.eq: "eq",
    operator.ne: "ne",
    operator.lt: "lt",
    operator.le: "le",
    operator.gt: "gt",
    operator.ge: "ge",
}


def _compile_expr_tree(node, field_index_map, referencing_field_name):
    """将 FieldRef/ExprRef 树翻译为 ExprOp 指令列表（后序遍历）。

    递归遍历表达式树，按后序（left → right → op）发射指令：

    - ``_FieldDescriptor`` → ``("getint", field_index)``
    - ``int`` → ``("const", value)``
    - ``_ExprRef`` 二元运算 → 递归 lhs、递归 rhs，然后 ``("op_name",)``
    - ``_ExprRef`` 一元运算（neg/invert）→ 递归 lhs，然后 ``("op_name",)``

    :param node: 表达式树根节点（``_FieldDescriptor`` / ``_ExprRef`` / ``int``）。
    :param field_index_map: ``{id(descriptor): field_index}``，编译期建立的映射。
    :param referencing_field_name: 引用方字段名（用于错误信息）。
    :return: ExprOp 指令元组列表，如 ``[("getint", 0), ("getint", 1), ("add",)]``。
    :raises CompilationError: 字段引用未找到 / 不支持的节点类型 / 浮点常量。
    """
    ops = []
    _emit_expr_ops(node, field_index_map, referencing_field_name, ops)
    return ops


def _emit_expr_ops(node, field_index_map, ref_name, ops):
    """递归遍历表达式树，后序发射 ExprOp 指令。

    纯整数 VM 栈：只接受 ``_FieldDescriptor`` / ``_ExprRef`` / ``int`` 节点。
    ``float`` 与其他类型直接 CompilationError。
    """
    if isinstance(node, _FieldDescriptor):
        # 字段引用：查 field_index_map（通过对象 identity）
        idx = field_index_map.get(id(node))
        if idx is None:
            raise CompilationError(
                "字段引用未找到：字段 '{}' 引用了未知字段 '{}'。"
                "请检查拼写或确保被引用的字段在当前 Struct 中声明。".format(
                    ref_name, node.name or "<unnamed>"
                )
            )
        ops.append(("getint", idx))

    elif isinstance(node, _ExprRef):
        # 表达式节点：递归 lhs、rhs，然后发射操作符
        if node.op in (operator.neg, operator.invert):
            # 一元运算：只有 lhs
            _emit_expr_ops(node.lhs, field_index_map, ref_name, ops)
            ops.append((_OP_TO_EXPROP[node.op],))
        else:
            # 二元运算：lhs、rhs、op
            _emit_expr_ops(node.lhs, field_index_map, ref_name, ops)
            _emit_expr_ops(node.rhs, field_index_map, ref_name, ops)
            op_name = _OP_TO_EXPROP.get(node.op)
            if op_name is None:
                raise CompilationError(
                    "不支持的表达式运算符：{}（在字段 '{}' 中）。"
                    "VM 栈为 i64，不支持浮点除法或未实现的运算。".format(node.op, ref_name)
                )
            ops.append((op_name,))

    elif isinstance(node, bool):
        # bool 是 int 的子类，但显式拒绝以避免歧义（表达式应使用整数）
        raise CompilationError(
            "不支持的表达式节点类型 bool：{!r}（在字段 '{}' 中）。"
            "请使用整数 0/1 替代。".format(node, ref_name)
        )

    elif isinstance(node, int):
        # 整数常量（排除 bool，已在上分支处理）
        ops.append(("const", node))

    elif isinstance(node, float):
        raise CompilationError(
            "浮点常量不支持（VM 栈为 i64）。"
            "请使用整数表达式（在字段 '{}' 中）".format(ref_name)
        )

    else:
        raise CompilationError(
            "不支持的表达式节点类型 {}：{!r}（在字段 '{}' 中）".format(
                type(node).__name__, node, ref_name
            )
        )


def _check_forward_reference(expr_ops, current_field_index, ref_name):
    """检查表达式中是否引用了后序字段（前向引用）。

    parse 方向只能引用已解析的前序字段（声明顺序在当前字段之前）。
    引用后序字段（``getint idx >= current_field_index``）是逻辑错误。

    :param expr_ops: 编译后的 ExprOp 指令列表。
    :param current_field_index: 当前字段在 descriptors 列表中的索引。
    :param ref_name: 当前字段名（用于错误信息）。
    :raises CompilationError: 引用了后序字段。
    """
    for op in expr_ops:
        if op[0] == "getint" and op[1] >= current_field_index:
            raise CompilationError(
                "字段 '{}' 的表达式引用了后序字段（索引 {} >= {}）。"
                "表达式只能引用在当前字段之前声明的字段。".format(
                    ref_name, op[1], current_field_index
                )
            )


def _check_wo_reference(expr_ops, descriptors, ref_name):
    """检查表达式中是否引用了 WO 字段。

    WO 字段（padding/reserved）不写入 context，表达式运行时无法取到其值。
    编译期检测到 ``getint`` 引用 WO 字段时立即抛 ``CompilationError``。

    :param expr_ops: 编译后的 ExprOp 指令列表。
    :param descriptors: ``[(name, _FieldDescriptor), ...]`` 完整字段列表。
    :param ref_name: 引用方字段名（用于错误信息）。
    :raises CompilationError: 引用了 WO 字段。
    """
    wo_indices = {
        idx for idx, (_, desc) in enumerate(descriptors) if desc.mode == "wo"
    }
    for op in expr_ops:
        if op[0] == "getint" and op[1] in wo_indices:
            wo_name = descriptors[op[1]][0]
            raise CompilationError(
                "字段 '{}' 的表达式引用了 WO 字段 '{}'。"
                "WO 字段（padding/reserved）不写入 context，"
                "表达式无法引用。如需引用，请将该字段改为 field() 或 rfield()。".format(
                    ref_name, wo_name
                )
            )


# ---------------------------------------------------------------------------
# 嵌套表达式递归收集（扁平键 + 冲突检测 + 槽位名协议）
# ---------------------------------------------------------------------------

# 递归遍历的子描述符槽位名集合。仅这些属性名中的值会被视为
# 子描述符递归；其余属性（如 Union.parsefrom、Checksum.start/end）是表达式
# 参数，由 ``_expr_params`` 协议消费，不进槽位集合。
_EXPR_SLOT_NAMES = (
    "subcon",
    "subcons",
    "cases",
    "default",
    "thensubcon",
    "elsesubcon",
    "lengthfield",
    "countfield",
    "checksumfield",
)

# 槽位值守卫：这些类型的值不可能是子描述符（常量叶子），直接跳过。
_EXPR_SLOT_LEAF_TYPES = (bool, int, float, str, bytes)


def _compile_expr_param_value(param_value, field_index_map, field_name):
    """编译单个 ``_expr_params`` 参数值为 ExprOp 指令列表。

    :param param_value: 参数值（FieldRef/ExprRef、预编译 ops list 或 int 常量）。
    :param field_index_map: ``{id(descriptor): field_index}``。
    :param field_name: 字段名（用于错误信息）。
    :return: ExprOp 指令元组列表；不可编译的常量值（bytes/str 等，留在描述符
             中供 Rust 侧直接读取）返回 ``None``。
    """
    if isinstance(param_value, (_FieldDescriptor, _ExprRef)):
        return _compile_expr_tree(param_value, field_index_map, field_name)
    if isinstance(param_value, list):
        # 预编译 ops（RepeatUntilDescriptor 通过 set_compiled_expr_params
        # 注入已编译的 ExprOp 元组列表）。直接透传，无需再编译。
        # 注意：元素必须是元组（与 _compile_expr_tree 输出格式一致）。
        return param_value
    if isinstance(param_value, int) and not isinstance(param_value, bool):
        # int 常量编译为单条 Const ExprOp。
        # DefaultDescriptor.value / CheckDescriptor.func 需要 ExprProgram
        # （int 常量也包装为单条 Const）。
        # BytesDescriptor.length 的常量路径由 Rust 侧直接从描述符读取，
        # 此处的 ExprProgram 冗余但无害（Rust 优先 extract::<usize>）。
        return [("const", param_value)]
    # 其他常量值（bytes/str）不编译（留在描述符中供 Rust 侧读取）
    return None


def _merge_expr_param(out, param_name, ops, field_name):
    """同名表达式参数合并（扁平键 + 冲突检测）。

    - 首次出现 → 写入。
    - 同名同 ops → 去重（diamond / 同表达式的多个消费者共用一份程序）。
    - 同名不同 ops → ``CompilationError``（诚实限制：扁平键无法区分同字段内
      多个同类消费者，如 ``Union(None, Switch(a, ...), Switch(b, ...))``）。

    :param out: 目标 ``{param_name: [expr_ops]}`` dict（原位合并）。
    :param param_name: 参数名（"key"/"cond"/"func"/"length"/...）。
    :param ops: 已编译的 ExprOp 指令列表。
    :param field_name: 字段名（用于错误信息）。
    :raises CompilationError: 同名不同 ops。
    """
    if param_name not in out:
        out[param_name] = ops
    elif out[param_name] != ops:
        raise CompilationError(
            "字段 '{}' 内有多个同名表达式参数 '{}'（如两个 Switch 的 'key'），"
            "construct-rs 当前无法区分。请拆分为多个字段，"
            "或提升其中一个到顶层字段。".format(field_name, param_name)
        )
    # 同 ops → 去重（不重复写入）


def _collect_exprs_from_value(value, field_index_map, field_name, out,
                              repeat_untils, seen, ru_cls):
    """槽位值分派：叶子守卫 → 容器逐项展开 → 子节点递归。

    值守卫：``None``/bool/int/float/str/bytes → 跳过；list/tuple → 逐项；
    dict → ``.values()`` 逐项；其余 → 视为子节点递归。
    """
    if value is None or isinstance(value, _EXPR_SLOT_LEAF_TYPES):
        return
    if isinstance(value, (list, tuple)):
        for item in value:
            _collect_exprs_from_value(
                item, field_index_map, field_name, out, repeat_untils, seen, ru_cls
            )
        return
    if isinstance(value, dict):
        for item in value.values():
            _collect_exprs_from_value(
                item, field_index_map, field_name, out, repeat_untils, seen, ru_cls
            )
        return
    _collect_exprs_from_node(
        value, field_index_map, field_name, out, repeat_untils, seen, ru_cls
    )


def _collect_exprs_from_node(node, field_index_map, field_name, out,
                             repeat_untils, seen, ru_cls):
    """单个描述符节点：收集自身 ``_expr_params`` 并递归槽位子节点。

    节点守卫：

    - ``_FieldDescriptor`` / ``_ExprRef``：表达式节点不是子描述符（其表达式
      参数属于宿主字段自身，且 ``_FieldDescriptor`` 的 ``subcon``/``default``
      槽位不属于子描述符图）。
    - 类对象（嵌套 ``StructMixin`` 子类）：内部表达式在该类自身编译期处理，
      不遍历（与原版嵌套 ctx 隔离语义一致）。
    - ``id(node)`` 已见集合防环，**按字段**新建（模块级单例如 Int8ub 跨字段
      共享，不可全局去重）。

    RepeatUntil 特殊路径：terminator 从原始 ``.terminator``
    属性读取——``set_compiled_expr_params`` 只替换 ``_expr_params``，不触碰
    ``.terminator``，保证延迟重编译路径幂等；任意深度的 RU 都记入
    ``repeat_untils`` 供 ``_finalize_repeat_until`` 注入。
    """
    if isinstance(node, (_FieldDescriptor, _ExprRef)) or isinstance(node, type):
        return
    node_id = id(node)
    if node_id in seen:
        return
    seen.add(node_id)

    if isinstance(node, ru_cls):
        ops = _compile_expr_param_value(node.terminator, field_index_map, field_name)
        if ops is not None:
            _merge_expr_param(out, "terminator", ops, field_name)
        repeat_untils.append((node, ops))
    else:
        # 通过 _expr_params 协议统一检测（覆盖所有描述符类型）
        # BytesDescriptor: {"length": <value>}
        # ComputedDescriptor: {"func": <expr>}
        # SwitchDescriptor: {"key": <expr>}
        # IfThenElseDescriptor: {"cond": <expr>}
        # 无表达式参数的描述符（Const/Tell/FormatField 等）：空 dict 或无属性
        expr_params = getattr(node, "_expr_params", None)
        if expr_params:
            for param_name, param_value in expr_params.items():
                ops = _compile_expr_param_value(
                    param_value, field_index_map, field_name
                )
                if ops is not None:
                    _merge_expr_param(out, param_name, ops, field_name)

    # 递归槽位子节点（包装器 .subcon / Switch .cases / IfThenElse
    # .thensubcon/.elsesubcon / Prefixed .lengthfield 等）
    for slot in _EXPR_SLOT_NAMES:
        _collect_exprs_from_value(
            getattr(node, slot, None),
            field_index_map, field_name, out, repeat_untils, seen, ru_cls,
        )


def _extract_and_compile_exprs(subcon, field_index_map, field_name):
    """从 subcon **递归**提取含表达式的参数，编译为 ExprProgram。

    递归遍历嵌套描述符（包装器内任意深度的表达式消费者，如 ``Prefixed(Int8ub,
    Switch(typ, ...))``），收集结果**扁平合并**到同一字段的
    ``{param_name: [expr_ops]}``——Rust 侧消费点按
    ``expr_programs[field_index][param_name]`` 取程序，递归构建时沿用同一
    field_index 与同一 dict，扁平合并与消费协议天然兼容（Rust 数据面零改动）。

    合并语义见 ``_merge_expr_param``：同名同 ops 去重；同名不同 ops 报
    ``CompilationError``（诚实限制）。

    递归范围（槽位名协议 ``_EXPR_SLOT_NAMES``）：``subcon`` / ``subcons`` /
    ``cases`` / ``default`` / ``thensubcon`` / ``elsesubcon`` / ``lengthfield`` /
    ``countfield`` / ``checksumfield``。嵌套 StructMixin 子类（类对象）不
    遍历（其内部表达式在该类自身编译期处理）。

    :param subcon: 字段的类型描述符（顶层或含包装器）。
    :param field_index_map: ``{id(descriptor): field_index}``。
    :param field_name: 字段名（用于错误信息）。
    :return: ``(result, repeat_untils)``：result 为 ``{param_name: [expr_ops]}``
             或空 dict（无表达式参数）；repeat_untils 为遍历中遇到的
             ``(RepeatUntilDescriptor, terminator_ops)`` 列表（任意深度，供
             ``_finalize_repeat_until`` 逐个注入编译产物）。
    :raises CompilationError: 表达式编译失败（同名参数冲突、未知字段引用、
                              不支持的节点类型等；前向引用/WO 引用由调用方检查）。
    """
    # 延迟导入 RepeatUntilDescriptor（避免循环导入），
    # 单次导入后作为参数传递给递归收集器。
    from ._descriptors import RepeatUntilDescriptor

    result = {}
    repeat_untils = []
    seen = set()
    _collect_exprs_from_node(
        subcon, field_index_map, field_name, result, repeat_untils,
        seen, RepeatUntilDescriptor,
    )
    return result, repeat_untils


def _compile_expressions(descriptors, field_index_map):
    """遍历所有字段的 subcon，编译其中的表达式。

    对每个字段：
    1. 从 subcon **递归**提取表达式参数（``_extract_and_compile_exprs``，
       含包装器内任意深度的表达式消费者，扁平合并到同一字段）
    2. 对合并后的每个表达式执行编译期验证（前向引用、WO 引用）——判据对
       嵌套 ops 语义恰好正确（嵌套消费者在宿主字段 parse/build 期间求值，
       ctx 仅含 0..idx-1 前序字段）
    3. 收集到 ``{field_index: {param_name: [expr_ops]}}`` 结构
    4. RepeatUntilDescriptor 特殊处理（v0.1.1 起含任意深度的嵌套 RU）：
       对遍历中遇到的每个 RU 调 ``_finalize_repeat_until``，从 terminator
       ops 提取 element_field_idx 并调 set_compiled_expr_params

    :param descriptors: ``[(name, _FieldDescriptor), ...]`` 有序列表。
    :param field_index_map: ``{id(descriptor): field_index}``。
    :return: ``{field_index: {param_name: [expr_ops]}}`` 嵌套字典，空 dict 表示无表达式。
    :raises CompilationError: 任何表达式编译或验证失败。
    """
    expr_programs = {}
    for idx, (name, desc) in enumerate(descriptors):
        subcon = desc.subcon
        field_exprs, nested_repeat_untils = _extract_and_compile_exprs(
            subcon, field_index_map, name
        )
        if field_exprs:
            # 对每个表达式的 expr_ops 执行编译期验证
            for param_name, ops in field_exprs.items():
                _check_forward_reference(ops, idx, name)
                _check_wo_reference(ops, descriptors, name)
            expr_programs[idx] = field_exprs

        # RepeatUntil 特殊处理（v0.1.1 起含包装器内任意深度的嵌套 RU）：
        # terminator 表达式需提取 element_field_idx。descriptors/idx/name
        # 沿用宿主字段。
        for ru_desc, terminator_ops in nested_repeat_untils:
            _finalize_repeat_until(
                ru_desc, terminator_ops, idx, name, descriptors, field_exprs
            )

    return expr_programs


def _finalize_repeat_until(desc, terminator_ops, field_index, field_name,
                           descriptors, field_exprs):
    """完成 RepeatUntilDescriptor 的编译期参数注入。

    v0.1.1 起：对遍历中遇到的**每个** RepeatUntilDescriptor（含包装器内
    任意深度的嵌套 RU）调用；``descriptors/idx/name`` 参数沿用宿主字段。

    从 terminator ops 中提取所有 getint 索引：
    - 第一个 getint 索引作为 element_field_idx（Element 字段）
    - 其余 getint 索引作为 index_field_indices（Index 字段，每次迭代同步下标）
    调 ``desc.set_compiled_expr_params(ops, element_field_idx, index_field_indices)``，
    并把编译产物写入 field_exprs（供 Rust 侧 build_repeat_until_node 读取）。

    :param desc: RepeatUntilDescriptor 实例（任意深度）。
    :param terminator_ops: 已编译的 terminator ExprOp 列表（接收
                           已编译 ops 而非从 field_exprs 取，避免依赖插入顺序；
                           None 时报缺失错误）。
    :param field_index: 宿主字段在 descriptors 列表中的索引。
    :param field_name: 宿主字段名（用于错误信息）。
    :param descriptors: ``[(name, _FieldDescriptor), ...]`` 宿主完整字段列表。
    :param field_exprs: 宿主字段的表达式 dict（写入编译产物键）。
    :raises CompilationError: terminator 不引用任何 Element 字段。
    """
    ops = terminator_ops
    if ops is None:
        raise CompilationError(
            "RepeatUntil field '{}' (index {}) missing 'terminator' expression. "
            "terminator must be a field expression (e.g. e > 5).".format(
                field_name, field_index
            )
        )

    # 提取所有 getint 索引（按出现顺序，去重）。
    all_getint_indices = []
    seen = set()
    for op in ops:
        if op[0] == "getint":
            idx = op[1]
            if idx not in seen:
                seen.add(idx)
                all_getint_indices.append(idx)

    if not all_getint_indices:
        raise CompilationError(
            "RepeatUntil terminator in field '{}' (index {}) does not reference any "
            "field. terminator must reference an Element field declared before "
            "RepeatUntil (e.g. e > 5 where e = rfield(Element())).".format(
                field_name, field_index
            )
        )

    # 第一个 getint 索引作为 element_field_idx（Element 字段）。
    element_field_idx = all_getint_indices[0]

    # 校验 element_field_idx 引用的字段确实是 Element 字段。
    if element_field_idx >= len(descriptors):
        raise CompilationError(
            "RepeatUntil element_field_idx {} out of range (field_count {}) in "
            "field '{}' (index {}).".format(
                element_field_idx, len(descriptors), field_name, field_index
            )
        )
    elem_name, elem_desc = descriptors[element_field_idx]
    elem_subcon = elem_desc.subcon
    from ._descriptors import ElementDescriptor, IndexDescriptor

    if not isinstance(elem_subcon, ElementDescriptor):
        raise CompilationError(
            "RepeatUntil terminator in field '{}' (index {}) references field '{}' "
            "(index {}) which is not an Element field. The first field reference in "
            "terminator must be an Element field (rfield(Element())) — this is the "
            "field that gets updated with the current element each iteration.".format(
                field_name, field_index, elem_name, element_field_idx
            )
        )

    # 其余 getint 索引分类：
    # - Index 字段（rfield(Index())）：每次迭代同步为当前下标
    # - 其他字段（普通 RW/RO 字段）：保留 StructNode.parse 写入的值（稳定值）
    #   终止表达式可读，但不随迭代变化。
    index_field_indices = []
    for idx in all_getint_indices[1:]:
        if idx >= len(descriptors):
            # 越界：保留 element_field_idx 校验已在上方完成，其他字段越界
            # 也应报错。但为容错（普通字段引用），仅跳过。
            continue
        _ref_name, ref_desc = descriptors[idx]
        ref_subcon = ref_desc.subcon
        if isinstance(ref_subcon, IndexDescriptor):
            index_field_indices.append(idx)
        # 其他类型字段（Element/普通字段）：保留原值，不加入 index_field_indices。

    # 注入编译产物到描述符（供 Rust 侧 build_repeat_until_node 读取）。
    desc.set_compiled_expr_params(ops, element_field_idx, index_field_indices)
    # 同步更新 field_exprs：替换 terminator 值为已编译 ops + 新增编译产物键。
    field_exprs["terminator"] = ops
    field_exprs["element_field_idx"] = element_field_idx
    field_exprs["index_field_indices"] = index_field_indices


def _expr_programs_to_list(expr_programs, field_count):
    """将 ``{field_index: {param_name: [ops]}}`` 转换为 ``Vec<Option<dict>>`` 格式。

    Rust 侧 ``compile_schema`` 接收 ``expr_programs: Option<Vec<Option<PyObject>>>``，
    长度与字段数一致。每个元素为 ``None``（该字段无表达式）或 Python dict
    （``{param_name: [expr_ops]}``）。

    :param expr_programs: ``{field_index: {param_name: [ops]}}`` 嵌套字典。
    :param field_count: 字段总数。
    :return: 长度为 ``field_count`` 的列表，每项为 ``None`` 或 dict。
    """
    result = []
    for i in range(field_count):
        if i in expr_programs:
            result.append(expr_programs[i])
        else:
            result.append(None)
    return result


# ---------------------------------------------------------------------------
# dataclass 字段配置注入
# ---------------------------------------------------------------------------


def _apply_dataclass_field_config(cls, descriptors):
    """将有 default 的 ``_FieldDescriptor`` 转换为 ``dataclasses.field()`` 配置。

    在 ``__init_subclass__`` 末尾调用（编译完成后，``@dataclass`` 执行前）。
    将类属性上的 ``_FieldDescriptor`` 替换为 ``dataclasses.field()`` 返回值，
    使 ``@dataclass`` 能正确配置 ``init`` / ``default`` / ``kw_only``。

    处理规则：
    - ``mode="ro"`` → ``dataclasses.field(init=False, default=None)``
      （RO 字段不在 ``__init__`` 中，parse 时由 force_setattr 覆盖）
    - ``mode="rw"`` 或 ``"wo"`` 且有显式 ``default`` →
      ``dataclasses.field(kw_only=True, default=desc.default)``
    - ``mode="rw"`` 或 ``"wo"`` 无显式 ``default``，但 subcon 是值提供型
      构造器（Const/Default/Rebuild/Computed/Padding，v0.1.1 起）→
      ``dataclasses.field(kw_only=True, default=None)``——节点层 build 已
      支持 None 补值，实例化不再强制实参
    - 其余 ``mode="rw"`` 或 ``"wo"`` → ``dataclasses.field()``
      （必填 positional 参数，无默认值）

    隐式 default 设计：

    - **显式 default 优先**（上述分支顺序保证）。
    - **kw_only=True 是必需**而非可选：隐式 default 字段在前、必填字段在后
      时，``@dataclass`` 否则报 "non-default argument follows default
      argument"。
    - 统一 ``default=None`` 语义 = "让节点层自动生成值"。行为差异由节点层
      既有语义决定：Const/Default 传非 None 值会被校验（错值报
      ConstError）；Padding/Rebuild/Computed 传值被忽略。
    - **类型注解告警接受 + 文档说明**（``v: int = field(Const(5, Int8ub))``
      隐式 default=None 与 int 注解在 mypy/pyright strict 下告警，运行时无
      影响；可显式传 default 或注解写 ``int | None``），不为静态检查器改机制。

    注意：``_FieldDescriptor`` 带有 ``__eq__`` 运算符重载（表达式系统），
    导致 ``__hash__ = None``。Python 3.x 的
    ``@dataclass`` 将 ``__hash__ is None`` 的对象视为 mutable default 并拒绝。
    因此所有 ``_FieldDescriptor`` 必须替换为 ``dataclasses.field()``。
    对于无默认值的 RW/WO 字段，使用无参 ``dataclasses.field()`` 使其成为
    必填 positional 参数——行为上等价（用户始终提供值）。

    :param cls: StructMixin 子类。
    :param descriptors: ``[(field_name, _FieldDescriptor), ...]`` 有序列表。
    """
    # 延迟导入值提供型描述符（_mixin 顶部不导入 _descriptors，
    # 避免循环导入）。
    from ._descriptors import (
        ConstDescriptor,
        DefaultDescriptor,
        PaddingDescriptor,
        RebuildDescriptor,
    )

    # 值提供型构造器：build 时节点层可自动获得值的描述符。
    # ComputedDescriptor 定义于本模块。
    value_providing = (
        ConstDescriptor,
        DefaultDescriptor,
        RebuildDescriptor,
        PaddingDescriptor,
        ComputedDescriptor,
    )

    for name, desc in descriptors:
        if desc.mode == "ro":
            # RO 字段：init=False，用 None 占位（parse 时由 force_setattr 覆盖）
            setattr(cls, name, dataclasses.field(init=False, default=None))
        elif desc.default is not _MISSING:
            # RW/WO 字段且有显式 default：kw_only=True + default（显式优先）
            setattr(
                cls,
                name,
                dataclasses.field(kw_only=True, default=desc.default),
            )
        elif isinstance(desc.subcon, value_providing):
            # 值提供型 subcon 且无显式 default → 隐式
            # default=None + kw_only=True。build 由节点层补值（Const/Default
            # 用常量/表达式值，Rebuild/Computed 求值，Padding 忽略传入值）。
            setattr(cls, name, dataclasses.field(kw_only=True, default=None))
        else:
            # RW/WO 无 default → dataclasses.field() 无默认值（必填 positional）
            setattr(cls, name, dataclasses.field())


# ---------------------------------------------------------------------------
# Schema 编译
# ---------------------------------------------------------------------------


def _compile_schema_for_class(cls):
    """为 StructMixin 子类编译 schema。

    在 ``__init_subclass__`` 中调用。收集字段 → 编译表达式 → 调用 Rust
    ``compile_schema`` → 注入 dataclass 配置 → 存储编译产物。前向引用未解析时
    安装延迟桩，其他错误 fail fast。

    流程：
    1. 收集 descriptors（已有）
    2. 构建 field_index_map: ``{id(desc): idx}``
    3. 调用 ``_compile_expressions`` 获取 expr_programs
    4. 提取 modes 列表
    5. 传入 compile_schema（扩展后的签名，含 modes + expr_programs）
    6. 读取 ``cls._construct_bitwise`` 标志（由 ``BitStructMixin`` 设置）。
       为 True 时传入 ``bitwise=True``，根 StructNode 被 Rust 侧包入 BitwiseNode。

    :param cls: 刚创建的 StructMixin 子类。
    """
    descriptors = _collect_field_descriptors(cls)
    field_names = [name for name, _ in descriptors]
    subcons = [desc.subcon for _, desc in descriptors]
    modes = [desc.mode for _, desc in descriptors]

    # 构建 field_index_map：{id(descriptor): field_index}。
    # 表达式中的 FieldRef 通过 id() 查找对应字段索引。
    field_index_map = {id(desc): idx for idx, (_, desc) in enumerate(descriptors)}

    # 编译表达式：遍历每个字段的 subcon，提取 _expr_params 中的表达式参数，
    # 翻译为 ExprOp 指令列表。同时执行编译期验证（前向引用、WO 引用）。
    # 返回 {field_index: {param_name: [expr_ops]}}。
    expr_programs = _compile_expressions(descriptors, field_index_map)

    # 转换为 Rust 侧期望的 Vec<Option<dict>> 格式。
    expr_programs_list = _expr_programs_to_list(expr_programs, len(descriptors))

    # 读取 BitStructMixin 设置的 bitwise 标志。
    # 普通 StructMixin 子类不设置此标志，getattr 默认 False。
    bitwise = getattr(cls, "_construct_bitwise", False)

    # 缓存 descriptors 供延迟桩重试编译使用。
    cls._cached_descriptors = descriptors

    if _compile_schema is None:
        raise ConstructError(
            "construct-rs Rust 扩展不可用，无法编译 schema。"
            "请确保已通过 maturin develop 安装。"
        )

    try:
        cls._construct_compiled = _compile_schema(
            cls,
            field_names,
            subcons,
            modes=modes,
            expr_programs=expr_programs_list,
            bitwise=bitwise,
        )
    except Exception as e:
        # 检查是否为前向引用未解析（UnresolvedReference）。
        # Rust 侧该错误消息含 "unresolved" 或 "未解析"。
        err_str = str(e)
        if "unresolved" in err_str.lower() or "未解析" in err_str:
            # 在安装延迟桩前注入 dataclass 字段配置。
            # _FieldDescriptor 有 __hash__=None（表达式系统），
            # 若不替换为 dataclasses.field()，@dataclass 会拒绝。
            _apply_dataclass_field_config(cls, descriptors)
            _install_lazy_stubs(cls)
        else:
            raise

    # 编译成功：注入 dataclass 字段配置（在编译后、@dataclass 执行前）
    _apply_dataclass_field_config(cls, descriptors)


# ---------------------------------------------------------------------------
# 延迟桩机制
# ---------------------------------------------------------------------------

# 每个延迟桩类对应的编译锁，确保多线程下编译只发生一次。
# 使用 WeakKeyDictionary 避免类被删除后锁对象仍被引用（内存泄漏）。
# WeakKeyDictionary 在 cls 被 GC 回收时自动移除对应条目。
_lazy_compile_locks = weakref.WeakKeyDictionary()


def _install_lazy_stubs(cls):
    """安装延迟编译桩。

    当编译因前向引用失败时，将 ``parse``/``build`` 替换为桩方法。首次调用时
    在锁保护下重试编译，成功后删除桩（回退到 ``StructMixin`` 继承的方法）。

    使用 ``cls._cached_descriptors``（由
    ``_compile_schema_for_class`` 缓存），而非重新收集。原因：
    1. ``_apply_dataclass_field_config`` 已将类属性上的 ``_FieldDescriptor``
       替换为 ``dataclasses.field()`` 返回值，``_collect_field_descriptors``
       此刻无法再识别它们（isinstance 检查失败）。
    2. 重新收集会得到空列表，导致延迟桩阶段编译出空 schema，丢失所有字段。

    线程安全：``threading.Lock`` + 双重检查确保编译只发生一次。

    :param cls: 需要安装延迟桩的 StructMixin 子类。
    """
    cls._construct_compiled = None
    lock = threading.Lock()
    _lazy_compile_locks[cls] = lock

    def _retry_compile():
        """在锁保护下重试编译。已编译则直接返回。"""
        if cls._construct_compiled is not None:
            return
        with lock:
            if cls._construct_compiled is not None:
                return
            # 使用缓存的 descriptors，而非重新收集。
            # _apply_dataclass_field_config 已替换类属性，重新收集会失败。
            descriptors = getattr(cls, "_cached_descriptors", None)
            if descriptors is None:
                # 兜底：理论上不会发生（_compile_schema_for_class 总会缓存）。
                # 若确实缺失，尝试重新收集（可能在 _apply_dataclass_field_config
                # 之前被调用）。
                descriptors = _collect_field_descriptors(cls)
            field_names = [name for name, _ in descriptors]
            subcons = [desc.subcon for _, desc in descriptors]
            modes = [desc.mode for _, desc in descriptors]

            # 构建 field_index_map 并编译表达式（与首次编译路径一致）。
            field_index_map = {
                id(desc): idx for idx, (_, desc) in enumerate(descriptors)
            }
            expr_programs = _compile_expressions(descriptors, field_index_map)
            expr_programs_list = _expr_programs_to_list(
                expr_programs, len(descriptors)
            )

            schema = _compile_schema(
                cls,
                field_names,
                subcons,
                modes=modes,
                expr_programs=expr_programs_list,
                bitwise=getattr(cls, "_construct_bitwise", False),
            )
            # 编译成功：注入 dataclass 配置，存储产物，删除桩
            _apply_dataclass_field_config(cls, descriptors)
            cls._construct_compiled = schema
            _remove_lazy_stubs(cls)

    @classmethod
    def _lazy_parse(cls_, data):
        """延迟 parse 桩：首次调用时重试编译，然后执行 parse。"""
        _retry_compile()
        schema = cls_._construct_compiled
        if schema is None:
            raise ConstructError(
                "{} 尚未完成编译（延迟编译失败）".format(cls_.__name__)
            )
        # Rust 内部直接构造实例（与 StructMixin.parse 一致）
        return schema._parse_raw(data)

    def _lazy_build(self):
        """延迟 build 桩：首次调用时重试编译，然后执行 build。"""
        _retry_compile()
        schema = type(self)._construct_compiled
        if schema is None:
            raise ConstructError(
                "{} 尚未完成编译（延迟编译失败）".format(type(self).__name__)
            )
        return schema._build_raw(self)

    cls.parse = _lazy_parse
    cls.build = _lazy_build


def _remove_lazy_stubs(cls):
    """删除延迟桩方法，使 cls 回退到 StructMixin 继承的 parse/build。

    编译成功后调用，后续 parse/build 调用直接走 StructMixin 的方法（零桩开销）。
    """
    for method_name in ("parse", "build"):
        try:
            delattr(cls, method_name)
        except AttributeError:
            pass


# ---------------------------------------------------------------------------
# StructMixin 基类
# ---------------------------------------------------------------------------


class StructMixin:
    """所有用户二进制结构类的基类。

    子类需配合 ``@dataclass`` 装饰器使用。类创建时自动触发 Schema 编译，
    编译产物存于类属性 ``_construct_compiled``。

    用法::

        from dataclasses import dataclass
        from construct import StructMixin, field, rfield, Int8ub, GreedyBytes

        @dataclass
        class MyMsg(StructMixin):
            address: int = field(Int8ub)
            data: bytes = field(GreedyBytes)

        msg = MyMsg(address=1, data=b'\\x00\\x01')
        built = msg.build()
        parsed = MyMsg.parse(built)

    编译流程在 ``__init_subclass__`` 中完成，用户无感。``@dataclass``
    在 ``__init_subclass__`` 之后执行，两者通过 ``field()`` 返回值
    协调（同时满足 dataclass 默认值协议与 Mixin 编译协议）。
    """

    # 编译产物缓存槽（由 __init_subclass__ 设置）。None 表示尚未编译
    # （可能是延迟桩阶段，或空基类）。
    _construct_compiled = None

    def __init_subclass__(cls, **kwargs):
        """子类创建时自动编译 schema。

        此方法在 ``@dataclass`` 装饰器之前执行（Python 类创建顺序）。
        编译流程：收集字段 → 调用 Rust compile_schema → 注入 dataclass 配置 →
        存储产物。
        """
        super().__init_subclass__(**kwargs)
        _compile_schema_for_class(cls)

    @classmethod
    def parse(cls, data):
        """从字节解析为本类的实例。

        恰好一次 FFI 穿越：调用 ``CompiledSchema._parse_raw`` 直接返回本类实例
        （Rust 内部通过 create_class + force_setattr 构造实例，
        无 Python 侧 ``cls(**dict)`` kwargs unpacking 开销）。

        :param data: 待解析的字节数据。
        :return: 本类的实例，字段值从 ``data`` 中解析得到。
        :raises ConstructError: 编译未完成或解析失败（字节不足、格式错误等）。
        """
        schema = cls._construct_compiled
        if schema is None:
            raise ConstructError(
                "{} 尚未完成编译（可能存在未解析的前向引用），"
                "请确保所有被引用的类型已定义后再调用 parse".format(cls.__name__)
            )
        # Rust 内部直接构造实例，不再 cls(**raw_fields)
        return schema._parse_raw(data)

    def build(self):
        """从本实例构造字节。

        恰好一次 FFI 穿越：Rust 内部遍历执行树，通过 C API 读取本实例属性，
        直接写入输出缓冲区。

        :return: 构建的字节数据。
        :raises ConstructError: 编译未完成或构建失败（属性缺失、类型错误等）。
        """
        schema = type(self)._construct_compiled
        if schema is None:
            raise ConstructError(
                "{} 尚未完成编译".format(type(self).__name__)
            )
        return schema._build_raw(self)


class BitStructMixin(StructMixin):
    """BitStruct 基类。子类用 ``@dataclass`` 装饰，字段使用 bit 级描述符
    （``Nibble``、``BitsInteger``、``Bit`` 等）。

    等价于 Python construct 的 ``BitStruct``，即 ``Bitwise(Struct(...))`` 的语法糖。
    编译时（``__init_subclass__``）整体被 ``BitwiseNode`` 包裹，字段树在 bit 域
    编译（Padding → ``BitPaddingNode``）。

    使用方式::

        from dataclasses import dataclass
        from construct import BitStructMixin, field, Bit, Nibble, BitsInteger

        @dataclass
        class Header(BitStructMixin):
            flag: int = field(Bit())            # 1 bit
            value: int = field(BitsInteger(10))  # 10 bits
            reserved: int = field(BitsInteger(5))  # 5 bits (凑齐 16 bit = 2 字节)

        Header.parse(b'\\xbe\\xef')  # 2 字节 = 16 bits

    实现机制：

    1. ``BitStructMixin.__init_subclass__`` 在 ``StructMixin.__init_subclass__``
       之前设置 ``cls._construct_bitwise = True``。
    2. ``StructMixin.__init_subclass__`` 调用 ``_compile_schema_for_class``，
       读取 ``_construct_bitwise`` 标志，传入 ``compile_schema(bitwise=True)``。
    3. Rust 侧 ``compile_schema`` 见 ``bitwise=true``，将根 ``StructNode`` 包入
       ``BitwiseNode``（等价于 ``Bitwise(Struct(...))``）。

    注意：``BitStructMixin`` 字段总尺寸必须是 8 的倍数（bit 数），否则
    ``BitwiseNode`` 在 parse/build 时返回 ``BitFieldError``。
    """

    def __init_subclass__(cls, **kwargs):
        """子类创建时先标记 bitwise，再委托 ``StructMixin`` 完成编译。"""
        cls._construct_bitwise = True
        super().__init_subclass__(**kwargs)


__all__ = ["StructMixin", "BitStructMixin", "field", "rfield", "wfield", "Tell", "Computed"]
