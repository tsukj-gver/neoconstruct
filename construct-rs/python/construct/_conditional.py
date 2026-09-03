"""Conditional 构造器（If / IfThenElse / Switch / Select / FocusedSeq）。

本模块提供 5 个 Conditional 构造器的 Python 用户面 + Descriptor 类。
Rust 侧通过 type name 识别 Descriptor，构建对应的 Node。

**If macro**：Python macro（不新增 Rust Node），等价 IfThenElse(cond, sub, Pass)。

Switch keyfunc 接受 int 常量或字段名表达式（``_FieldDescriptor`` / ``_ExprRef``，
含 ``x + 1`` 等算术组合），编译为 ExprProgram 运行时零 FFI 求值（复杂 int
表达式按功能保留）。v0.1.1 起 Switch/IfThenElse 可嵌套于包装器内（Prefixed 等），
表达式引用同 Struct 前序字段。

Select / Peek 不吞 ExplicitError，直接传播。
"""

from ._errors import CompilationError
from ._mixin import _FieldDescriptor, _ExprRef

try:
    from ._descriptors import Pass
except ImportError:  # pragma: no cover
    Pass = None


# ---------------------------------------------------------------------------
# IfThenElseDescriptor / IfThenElse
# ---------------------------------------------------------------------------


class IfThenElseDescriptor:
    """``IfThenElse(condfunc, thensubcon, elsesubcon)`` 描述符。

    双分支条件：求值 condfunc，根据结果选择 then/else 子树委托。

    对应 Python construct 的 ``IfThenElse``（core.py L3944）。

    v0.1.1 起可嵌套于包装器内（Prefixed 等），表达式引用同 Struct 前序字段。

    :param condfunc: 条件。``True`` / ``False`` 常量，或 FieldRef/ExprRef 表达式
                     （如 ``x > 0``）。
    :param thensubcon: 条件为真时的子构造器。
    :param elsesubcon: 条件为假时的子构造器（通常为 ``Pass``）。

    ``_expr_params`` 协议（与 ``StopIfDescriptor`` 同模式）：
    - condfunc 是 bool 时返回 ``{}``（跳过编译）。
    - condfunc 是 FieldRef/ExprRef 时返回 ``{"cond": condfunc}``（编译为 ExprOp 列表）。
    """

    __slots__ = ("condfunc", "thensubcon", "elsesubcon")

    def __init__(self, condfunc, thensubcon, elsesubcon):
        """初始化 IfThenElse 描述符。

        :param condfunc: 条件（``True`` / ``False`` / FieldRef/ExprRef 表达式）。
        :param thensubcon: 条件为真时的子构造器。
        :param elsesubcon: 条件为假时的子构造器。
        """
        self.condfunc = condfunc
        self.thensubcon = thensubcon
        self.elsesubcon = elsesubcon

    @property
    def _expr_params(self):
        """表达式参数协议（与 StopIfDescriptor 同模式）。"""
        if isinstance(self.condfunc, bool):
            return {}
        return {"cond": self.condfunc}

    def __repr__(self):
        return "IfThenElse({!r}, {!r}, {!r})".format(
            self.condfunc, self.thensubcon, self.elsesubcon
        )


def IfThenElse(condfunc, thensubcon, elsesubcon):
    """创建一个 IfThenElse 描述符。

    使用方式（``x`` 必须是同 Struct 内已声明的 int 字段）::

        @dataclass
        class P(StructMixin):
            x: int = field(Int8ub)
            v: int = field(IfThenElse(x > 0, Int8ub, Int16ub))

        P.parse(b"\\x01\\xff")        # x=1 > 0, then 分支 → P(x=1, v=0xFF)
        P.parse(b"\\x00\\xff\\x01")   # x=0, else 分支 → P(x=0, v=0xFF01)

    v0.1.1 起可嵌套于包装器内（Prefixed 等），表达式引用同 Struct 前序字段。

    :param condfunc: 条件（``True`` / ``False`` / FieldRef/ExprRef 表达式）。
    :param thensubcon: 条件为真时的子构造器。
    :param elsesubcon: 条件为假时的子构造器。
    :return: ``IfThenElseDescriptor`` 实例。
    """
    return IfThenElseDescriptor(condfunc, thensubcon, elsesubcon)


def If(condfunc, subcon):
    """``If`` macro：等价 ``IfThenElse(condfunc, subcon, Pass)``。

    对应 Python construct 的 ``If``（core.py L3912）。
    Rust 不新增 IfNode——Python 用户面 macro 完成等价转换。

    使用方式（``x`` 必须是同 Struct 内已声明的 int 字段）::

        @dataclass
        class P(StructMixin):
            x: int = field(Int8ub)
            v: int = field(If(x > 0, Byte))

        P.parse(b"\\x01\\xff")  # x=1 > 0, 解析 Byte → P(x=1, v=0xFF)
        P.parse(b"\\x00")       # x=0, Pass（v=None）

    :param condfunc: 条件（``True`` / ``False`` / FieldRef/ExprRef 表达式）。
    :param subcon: 条件为真时的子构造器。
    :return: ``IfThenElseDescriptor`` 实例（else=Pass）。
    """
    if Pass is None:
        raise ImportError(
            "Pass descriptor is required for If macro but not available "
            "(Rust extension not built)"
        )
    return IfThenElseDescriptor(condfunc, subcon, Pass)


# ---------------------------------------------------------------------------
# SwitchDescriptor / Switch
# ---------------------------------------------------------------------------


class SwitchDescriptor:
    """``Switch(keyfunc, cases, default=Pass)`` 描述符。

    多分支条件：求值 keyfunc，根据结果在 cases dict 中匹配。

    对应 Python construct 的 ``Switch``（core.py L4002）。

    keyfunc 接受的形态：
    - int/bool 常量 → ``SwitchKey::ConstInt``
    - 字段名 int 表达式（``n`` 直接引用，或 ``x + 1`` 等算术组合）→
      ``SwitchKey::IntExpr``（零 FFI，复杂 int 表达式同样编译为 IntExpr，
      按功能保留）
    - Python callable/lambda → 构造期拒绝

    v0.1.1 起可嵌套于包装器内（Prefixed/PrefixedArray/Bitwise/Hex/Union/
    Select 等），表达式引用同 Struct 前序字段。

    :param keyfunc: key 函数。int/bool 常量或字段名表达式（``_FieldDescriptor``
                    / ``_ExprRef``）。
    :param cases: dict，``{key: subcon}``。key 是 int（常量匹配）。
    :param default: 默认 subcon。``None`` 时设为 ``Pass``（对齐 Python L4032）。

    ``_expr_params`` 协议：
    - keyfunc 是 int/bool 时返回 ``{}``（跳过编译）。
    - keyfunc 是 FieldRef/ExprRef 时返回 ``{"key": keyfunc}``（编译为 ExprOp 列表）。
    """

    __slots__ = ("keyfunc", "cases", "default")

    def __init__(self, keyfunc, cases, default=None):
        """初始化 Switch 描述符。

        :param keyfunc: key 函数（int/bool 或 FieldRef/ExprRef）。
        :param cases: dict，{key: subcon}。
        :param default: 默认 subcon（None 时设为 Pass）。
        """
        # callable 检查（与 RepeatUntil 同模式）：拒绝 Python lambda。
        if callable(keyfunc) and not isinstance(keyfunc, (_FieldDescriptor, _ExprRef)):
            raise CompilationError(
                "Switch keyfunc must be int/bool constant or a field expression "
                "(_FieldDescriptor / _ExprRef), not a Python callable. "
                "Example: Switch(n, {1: Byte, 2: Short}) where 'n' is a field "
                "in the same Struct; arithmetic like (x + 1) is also supported."
            )
        self.keyfunc = keyfunc
        self.cases = dict(cases) if cases else {}
        self.default = default if default is not None else Pass

    @property
    def _expr_params(self):
        """表达式参数协议。"""
        if isinstance(self.keyfunc, bool):
            return {}
        if isinstance(self.keyfunc, int):
            return {}
        if isinstance(self.keyfunc, (_FieldDescriptor, _ExprRef)):
            return {"key": self.keyfunc}
        # 其他类型 → 编译期拒绝（Rust 侧报错更具体）
        return {}

    def __repr__(self):
        return "Switch({!r}, {!r}, default={!r})".format(
            self.keyfunc, self.cases, self.default
        )


def Switch(keyfunc, cases, default=None):
    """创建一个 Switch 描述符。

    使用方式（int key 常量；``n`` 必须是同 Struct 内已声明的 int 字段）::

        @dataclass
        class P(StructMixin):
            n: int = field(Int8ub)
            v: int = field(Switch(n, {1: Int8ub, 2: Int16ub}))

        P.parse(b"\\x01\\x05")        # n=1 → Int8ub → P(n=1, v=5)
        P.parse(b"\\x02\\x00\\x05")   # n=2 → Int16ub → P(n=2, v=5)

    使用方式（默认值；未命中 cases 时走 default）::

        @dataclass
        class P(StructMixin):
            n: int = field(Int8ub)
            v: int = field(Switch(n, {}, default=Byte))

        P.parse(b"\\xff\\x01")  # n=255 未命中, default=Byte → P(n=255, v=1)

    v0.1.1 起可嵌套于包装器内（Prefixed 等），表达式引用同 Struct 前序字段::

        @dataclass
        class P(StructMixin):
            typ: int = field(Int8ub)
            br: Any = field(Prefixed(Int8ub, Switch(typ, {1: Int8ub, 2: Bytes(2)})))

    :param keyfunc: key 函数（int/bool 常量或 FieldRef/ExprRef 表达式）。
    :param cases: dict，{key: subcon}。
    :param default: 默认 subcon（None 时设为 Pass）。
    :return: ``SwitchDescriptor`` 实例。
    """
    return SwitchDescriptor(keyfunc, cases, default)


# ---------------------------------------------------------------------------
# SelectDescriptor / Select
# ---------------------------------------------------------------------------


class SelectDescriptor:
    """``Select(*subcons)`` 描述符。

    多分支尝试：遍历 subcons，首个成功者胜出。ExplicitError 不被吞掉，直接传播。

    对应 Python construct 的 ``Select``（core.py L3830）。

    :param subcons: 候选 subcon 列表。

    ``_expr_params`` 协议返回空 dict（Select 无表达式参数）。
    """

    __slots__ = ("subcons",)

    def __init__(self, subcons):
        """初始化 Select 描述符。

        :param subcons: 候选 subcon 列表（Python list）。
        """
        self.subcons = list(subcons)

    def __repr__(self):
        return "Select({})".format(", ".join(repr(s) for s in self.subcons))


def Select(*subcons):
    """创建一个 Select 描述符。

    使用方式::

        d = Select(Int32ub, CString("utf8"))
        d.parse(b"\\x00\\x00\\x00\\x01")  # Int32ub 成功 → 1
        d.parse(b"hello\\x00")            # CString 成功 → "hello"

    全部失败时抛 ``SelectError``::

        try:
            Select(Int32ub, CString("utf8")).parse(b"")
        except SelectError:
            pass

    :param subcons: 候选 subcon 列表（可变位置参数）。
    :return: ``SelectDescriptor`` 实例。
    """
    return SelectDescriptor(list(subcons))


# ---------------------------------------------------------------------------
# FocusedSeqDescriptor / FocusedSeq + Renamed
# ---------------------------------------------------------------------------


class Renamed:
    """命名包装器：为匿名 subcon 添加字段名（用于 FocusedSeq / Sequence）。

    对应 Python construct 的 ``Renamed``（core.py L2014），但 construct-rs 中
    仅用于 FocusedSeq / Sequence 的字段命名（不作为通用 Adapter）。

    通过 ``Renamed(name, subcon)`` 直接创建（Sequence 也可用关键字参数
    ``Sequence(name=subcon)``，内部等价包装为 Renamed）。

    :param name: 字段名（str）。
    :param subcon: 被包装的子构造器。
    """

    __slots__ = ("name", "subcon")

    def __init__(self, name, subcon):
        """初始化 Renamed 包装器。

        :param name: 字段名（str）。
        :param subcon: 被包装的子构造器。
        """
        self.name = name
        self.subcon = subcon

    def __repr__(self):
        return "{!r} / {!r}".format(self.name, self.subcon)


class FocusedSeqDescriptor:
    """``FocusedSeq(parsebuildfrom, *subcons)`` 描述符。

    聚焦字段序列：parse 返回聚焦字段的值（不是用户类实例），
    build 接收单值（仅聚焦字段从 obj 取值，其余传 None）。

    对应 Python construct 的 ``FocusedSeq``（core.py L3176）。

    :param parsebuildfrom: 聚焦字段名（str）。必须匹配 subcons 中某个命名字段
                            （通过 ``Renamed`` 创建）。
    :param subcons: 子构造器列表。命名字段用 ``Renamed("name", subcon)`` 创建，
                    匿名字段直接传 subcon。

    ``_expr_params`` 协议返回空 dict（FocusedSeq 本体无表达式参数；
    子构造器各自编译）。
    """

    __slots__ = ("parsebuildfrom", "subcons")

    def __init__(self, parsebuildfrom, subcons):
        """初始化 FocusedSeq 描述符。

        :param parsebuildfrom: 聚焦字段名（str）。
        :param subcons: 子构造器列表（含 Renamed 包装或匿名 subcon）。
        """
        if not isinstance(parsebuildfrom, str):
            raise CompilationError(
                "FocusedSeq parsebuildfrom must be a string (e.g. 'num'). "
                "Lambda context functions are not supported in construct-rs."
            )
        self.parsebuildfrom = parsebuildfrom
        self.subcons = list(subcons)

    def __repr__(self):
        return "FocusedSeq({!r}, {})".format(
            self.parsebuildfrom, ", ".join(repr(s) for s in self.subcons)
        )


def FocusedSeq(parsebuildfrom, *subcons):
    """创建一个 FocusedSeq 描述符。

    使用方式::

        d = FocusedSeq("num",
            Const(b"SIG"),          # 匿名字段
            Renamed("num", Byte),   # 命名字段（focus）
            Terminated,             # 匿名字段
        )
        d.parse(b"SIG\\xff")  # 返回 255（num 字段值）
        d.build(255)          # 返回 b"SIG\\xff"

    :param parsebuildfrom: 聚焦字段名（str）。必须匹配 subcons 中某个命名字段。
    :param subcons: 子构造器列表（``Renamed("name", subcon)`` 或匿名 subcon）。
    :return: ``FocusedSeqDescriptor`` 实例。
    """
    return FocusedSeqDescriptor(parsebuildfrom, list(subcons))


__all__ = [
    "IfThenElseDescriptor",
    "IfThenElse",
    "If",
    "SwitchDescriptor",
    "Switch",
    "SelectDescriptor",
    "Select",
    "FocusedSeqDescriptor",
    "FocusedSeq",
    "Renamed",
]
