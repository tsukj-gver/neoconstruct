"""类型描述符的 Python 侧重导出（纯 Python 委托层）。

设计依据：``docs/架构设计.md`` §A.4（类型描述符）、§D.3（Python 包结构）、
``docs/模块设计-BitStream.md`` §8.2（bit 描述符）。

实际的描述符 ``pyclass`` 定义在 Rust 侧（``src/descriptors/``），由
``_construct_rust`` 扩展模块提供。本模块将其重导出，供 ``__init__.py``
和内部模块使用。

三种描述符形态（§A.4）：

1. **预定义单例**（无参数原子描述符）：
   - ``Int8ub``, ``Int8ul``, ``Int8sb``, ``Int8sl``
   - ``Int16ub``, ``Int16ul``, ``Int16sb``, ``Int16sl``
   - ``Int32ub``, ``Int32ul``, ``Int32sb``, ``Int32sl``
   - ``Int64ub``, ``Int64ul``, ``Int64sb``, ``Int64sl``
   - ``GreedyBytes``

2. **可实例化描述符**（带参数）：
   - ``Bytes(length)`` — 读取 ``length`` 字节
   - ``BitsInteger(length, signed=False, swapped=False)`` — bit 级整数
     （Phase 3.1 仅常量 length，表达式 length 编译期拒绝）

3. **StructMixin 子类引用**（嵌套结构）：
   - 直接传 StructMixin 子类作为描述符：``field(Inner)``

4. **纯 Python 描述符（Phase 3.1 bit 域）**：
   - ``BitsIntegerDescriptor`` — bit 级整数（由 Rust type-name 识别）
   - ``Bit()`` / ``Nibble()`` / ``Octet()`` — ``BitsInteger(1/4/8)`` 语法糖
"""

# 从 Rust 扩展导入描述符类与单例。
# 扩展未构建时静默跳过（允许纯 Python 开发模式）。
try:
    from ._construct_rust import (
        # 描述符 pyclass
        FormatFieldDescriptor,
        BytesDescriptor,
        GreedyBytesDescriptor,
        # 16 个整数格式预定义单例
        Int8ub,
        Int8ul,
        Int8sb,
        Int8sl,
        Int16ub,
        Int16ul,
        Int16sb,
        Int16sl,
        Int32ub,
        Int32ul,
        Int32sb,
        Int32sl,
        Int64ub,
        Int64ul,
        Int64sb,
        Int64sl,
        # GreedyBytes 预定义单例
        GreedyBytes,
    )
except ImportError:  # pragma: no cover - 仅在扩展未构建时触发
    pass

# Bytes 作为 BytesDescriptor 的别名导出。
# 用户通过 Bytes(4) 创建 BytesDescriptor 实例（Rust 侧 #[new] 支持）。
try:
    from ._construct_rust import BytesDescriptor as Bytes
except ImportError:  # pragma: no cover
    pass


# ---------------------------------------------------------------------------
# Phase 3.1: BitsInteger / Bit / Nibble / Octet 纯 Python 描述符
#
# 设计依据：``docs/模块设计-BitStream.md`` §8.2。
#
# 这些描述符是纯 Python 类（不需要 Rust pyclass），通过 type name 识别。
# compile_schema 的 build_node_from_descriptor 通过 ``type(desc).__name__``
# 匹配到 "BitsIntegerDescriptor" 字符串，构建对应的 ``Node::BitsInteger``。
# （详见 compile.rs 的 build_bits_integer_node）
#
# 使用方式：必须在 Bitwise/BitStruct 域内使用（Phase 3.1 仅实现节点与编译，
# BitwiseNode/BitStructMixin 由后续子任务实现）。
# ---------------------------------------------------------------------------


class BitsIntegerDescriptor:
    """``BitsInteger(length, signed=False, swapped=False)`` 描述符。

    bit 级整数描述符，对应 Python construct 的 ``BitsInteger``。
    必须在 Bitwise（或 BitStruct）域内使用。

    Phase 3.1：仅支持常量 length（int）。表达式 length（``FieldRef``/``ExprRef``）
    在 Rust 编译管线（``compile_schema``）返回 ``CompilationError``。

    ``_expr_params`` 协议返回 ``{"length": <value>}``（仅当 length 是 FieldRef/ExprRef
    时被 _extract_and_compile_exprs 编译，常量 int 跳过编译）。

    :param length: bit 宽度（正整数，<=64）。
    :param signed: 是否有符号（二补码），默认 False。
    :param swapped: 是否做字节序反序（要求 length 是 8 的倍数），默认 False。
    """

    __slots__ = ("length", "signed", "swapped")

    def __init__(self, length, signed=False, swapped=False):
        """初始化 BitsInteger 描述符。

        :param length: bit 宽度。
        :param signed: 是否有符号。
        :param swapped: 是否做字节序反序。
        """
        self.length = length
        self.signed = signed
        self.swapped = swapped

    @property
    def _expr_params(self):
        """表达式参数协议。

        返回 ``{"length": self.length}``。当 length 是 int 时，
        ``_extract_and_compile_exprs`` 跳过编译（保留在描述符中供 Rust 侧读取）。
        当 length 是 ``_FieldDescriptor``/``_ExprRef`` 时，编译为 ExprOp 列表。
        """
        return {"length": self.length}

    def __repr__(self):
        return "BitsInteger(length={!r}, signed={!r}, swapped={!r})".format(
            self.length, self.signed, self.swapped
        )


def BitsInteger(length, signed=False, swapped=False):
    """创建一个 BitsInteger 描述符。

    使用方式（在 Bitwise 域内）::

        # BitStruct 字段（Phase 3.2 BitStructMixin 实现后）
        @dataclass
        class Header(BitStructMixin):
            flag: int = field(Bit())
            value: int = field(BitsInteger(10))
            reserved: int = field(BitsInteger(8, swapped=True))

    :param length: bit 宽度（正整数，<=64）。
    :param signed: 是否有符号（二补码），默认 False。
    :param swapped: 是否做字节序反序（要求 length 是 8 倍数），默认 False。
    :return: ``BitsIntegerDescriptor`` 实例。
    """
    return BitsIntegerDescriptor(length, signed, swapped)


def Bit():
    """1-bit 整数描述符（等价于 ``BitsInteger(1)``）。

    对应 Python construct 的 ``Bit``。必须在 Bitwise/BitStruct 域内使用。

    :return: ``BitsIntegerDescriptor(1, False, False)``。
    """
    return BitsIntegerDescriptor(1, False, False)


def Nibble():
    """4-bit 整数描述符（等价于 ``BitsInteger(4)``）。

    对应 Python construct 的 ``Nibble``。必须在 Bitwise/BitStruct 域内使用。

    :return: ``BitsIntegerDescriptor(4, False, False)``。
    """
    return BitsIntegerDescriptor(4, False, False)


def Octet():
    """8-bit 整数描述符（等价于 ``BitsInteger(8)``）。

    对应 Python construct 的 ``Octet``。必须在 Bitwise/BitStruct 域内使用。

    :return: ``BitsIntegerDescriptor(8, False, False)``。
    """
    return BitsIntegerDescriptor(8, False, False)


# ---------------------------------------------------------------------------
# Phase 3.2: Bitwise 描述符
#
# 设计依据：``docs/模块设计-BitStream.md`` §8.2、§11。
#
# ``Bitwise(subcon)`` 是核心包装器，将字节流转为 bit 流。construct-rs 通过
# type name "BitwiseDescriptor" 识别，递归编译内部 subcon（bitwise=true 上下文），
# 包装为 ``Node::Bitwise(BitwiseNode)``。
#
# 使用方式：通常用户使用 ``BitStructMixin`` 而非显式 ``Bitwise``。
# 显式 ``Bitwise(subcon)`` 用于：在普通 Struct 中嵌入 bit 域字段。
# ---------------------------------------------------------------------------


class BitwiseDescriptor:
    """``Bitwise(subcon)`` 描述符。

    bit 域包装器：将字节流转为 bit 流，内部 subcon 在 bit 域内编译与执行。
    对应 Python construct 的 ``Bitwise``。

    ``_expr_params`` 协议返回空 dict：Bitwise 无表达式参数（subcon 由递归处理）。
    """

    __slots__ = ("subcon",)

    def __init__(self, subcon):
        """初始化 Bitwise 描述符。

        :param subcon: 被包裹的子构造器（通常是 ``Struct`` 的描述符、``BitsInteger``、
                        或其他 bit 友好的描述符）。
        """
        self.subcon = subcon

    # 类级别常量：Bitwise 无表达式参数。
    _expr_params = {}

    def __repr__(self):
        return "Bitwise({!r})".format(self.subcon)


def Bitwise(subcon):
    """创建一个 Bitwise 描述符。

    bit 域包装器：将字节流转为 bit 流。对应 Python construct 的 ``Bitwise``。

    使用方式（在普通 Struct 中嵌入 bit 域）::

        @dataclass
        class Header(StructMixin):
            magic: int = field(Int32ub)
            bits: int = field(Bitwise(BitsInteger(8)))  # 1 字节 = 8 bit

    注意：``BitStruct`` 用户通常使用 ``BitStructMixin`` 而非显式 ``Bitwise``。
    ``BitStruct(...)`` 等价于 ``Bitwise(Struct(...))``。

    :param subcon: 被包裹的子构造器。
    :return: ``BitwiseDescriptor`` 实例。
    """
    return BitwiseDescriptor(subcon)


__all__ = [
    "FormatFieldDescriptor",
    "BytesDescriptor",
    "GreedyBytesDescriptor",
    "Bytes",
    "Int8ub",
    "Int8ul",
    "Int8sb",
    "Int8sl",
    "Int16ub",
    "Int16ul",
    "Int16sb",
    "Int16sl",
    "Int32ub",
    "Int32ul",
    "Int32sb",
    "Int32sl",
    "Int64ub",
    "Int64ul",
    "Int64sb",
    "Int64sl",
    "GreedyBytes",
    # Phase 3.1 bit 域描述符
    "BitsIntegerDescriptor",
    "BitsInteger",
    "Bit",
    "Nibble",
    "Octet",
    # Phase 3.2 Bitwise
    "BitwiseDescriptor",
    "Bitwise",
]
