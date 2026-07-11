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

# RepeatUntilDescriptor.__init__ 需要的 CompilationError（v5 callable 检查）。
from ._errors import CompilationError

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


# ---------------------------------------------------------------------------
# Phase 3.3: Padding / Bytewise / BitsSwapped / ByteSwapped 描述符
#
# 设计依据：``docs/模块设计-BitStream.md`` §4.2 / §4.4 / §4.5 / §4.6 / §8.2。
#
# - ``PaddingDescriptor``：根据编译期 ``bitwise`` 上下文编译为 ``BitPaddingNode``
#   （bit 域，pattern 严格 0x00/0x01）或 ``PaddingNode``（字节域）。
# - ``BytewiseDescriptor``：在 bit 域内重建字节流（``BytewiseNode``）。
# - ``BitsSwappedDescriptor`` / ``ByteSwappedDescriptor``：字节级 bit/字节反序变换
#   （``TransformNode`` with BitSwap/ByteSwap）。
# ---------------------------------------------------------------------------


class PaddingDescriptor:
    """``Padding(length, pattern=b"\\x00")`` 描述符。

    单位由编译期 ``bitwise`` 上下文决定（设计 §2.4 决策 B4）：

    - 字节域（普通 Struct 内）：``PaddingNode``，length 单位为字节，pattern 任意 0-255。
    - bit 域（Bitwise/BitStruct 内）：``BitPaddingNode``，length 单位为 bit，
      pattern 严格 ``0x00`` 或 ``0x01``（其他值返回 ``PaddingError``）。

    Phase 3.1 / 3.3：仅支持常量 length（int）。表达式 length 在 Rust 编译管线
    返回 ``CompilationError``。

    :param length: 填充长度（bit 或字节，由上下文决定）。
    :param pattern: 填充模式。字节域：1 字节 bytes（取首字节）；bit 域：仅 0x00 或 0x01。
    """

    __slots__ = ("length", "pattern")

    def __init__(self, length, pattern=b"\x00"):
        """初始化 Padding 描述符。

        :param length: 填充长度。
        :param pattern: 填充模式（默认 ``b"\\x00"``）。Python construct 接受 bytes，
                        这里取首字节作为 int pattern。
        """
        self.length = length
        # pattern 在 Python construct 中是 bytes；这里取首字节作为 int 保留。
        if isinstance(pattern, (bytes, bytearray)):
            self.pattern = pattern[0] if len(pattern) > 0 else 0
        else:
            # 已经是 int 或其他形式（直接保留，Rust 侧 extract<u8> 校验）
            self.pattern = pattern

    @property
    def _expr_params(self):
        """表达式参数协议。

        返回 ``{"length": self.length}``。当 length 是 int 时跳过编译；
        是 FieldRef/ExprRef 时编译为 ExprOp 列表。
        """
        if isinstance(self.length, int):
            return {}
        return {"length": self.length}

    def __repr__(self):
        return "Padding(length={!r}, pattern={!r})".format(self.length, self.pattern)


def Padding(length, pattern=b"\x00"):
    """创建一个 Padding 描述符。

    填充指定长度的字节或 bit（由所在上下文决定）。

    使用方式（字节域）::

        @dataclass
        class Header(StructMixin):
            magic: int = field(Int32ub)
            reserved: int = rfield(Padding(4))  # 4 字节填充

    使用方式（bit 域）::

        @dataclass
        class Bits(BitStructMixin):
            flag: int = field(Bit())               # 1 bit
            reserved: int = rfield(Padding(7))      # 7 bit 填充

    :param length: 填充长度（bit 或字节）。
    :param pattern: 填充模式。bit 域仅接受 ``b"\\x00"`` 或 ``b"\\x01"``。
    :return: ``PaddingDescriptor`` 实例。
    """
    return PaddingDescriptor(length, pattern)


class BytewiseDescriptor:
    """``Bytewise(subcon)`` 描述符。

    bit→byte 适配器：在 bit 域（Bitwise/BitStruct 内）为内部 subcon 重建字节流。
    必须在 Bitwise 域内使用。

    ``_expr_params`` 协议返回空 dict：Bytewise 无表达式参数（subcon 由递归处理）。
    """

    __slots__ = ("subcon",)

    def __init__(self, subcon):
        """初始化 Bytewise 描述符。

        :param subcon: 字节级子构造器（通常是 ``Int16ub``、``Bytes(N)``、Struct 等）。
        """
        self.subcon = subcon

    _expr_params = {}

    def __repr__(self):
        return "Bytewise({!r})".format(self.subcon)


def Bytewise(subcon):
    """创建一个 Bytewise 描述符。

    在 bit 域内重建字节流，使内部 subcon 在字节级操作。对应 Python construct 的 ``Bytewise``。

    使用方式（在 BitStruct 中嵌入整字节字段）::

        @dataclass
        class Header(BitStructMixin):
            flag: int = field(Nibble())           # 4 bit
            value: int = field(Bytewise(Int32ub)) # 4 字节（对齐快路径）
            tail: int = field(Nibble())           # 4 bit

    :param subcon: 字节级子构造器。
    :return: ``BytewiseDescriptor`` 实例。
    """
    return BytewiseDescriptor(subcon)


class BitsSwappedDescriptor:
    """``BitsSwapped(subcon)`` 描述符。

    字节内 bit 序翻转：每字节的 8 个 bit 反序（0xF0 → 0x0F）。
    对应 Python construct 的 ``BitsSwapped``（基于 ``swapbitsinbytes`` 变换）。

    ``_expr_params`` 协议返回空 dict。
    """

    __slots__ = ("subcon",)

    def __init__(self, subcon):
        """初始化 BitsSwapped 描述符。

        :param subcon: 被包裹的子构造器（必须定长，Phase 3.1 已知限制）。
        """
        self.subcon = subcon

    _expr_params = {}

    def __repr__(self):
        return "BitsSwapped({!r})".format(self.subcon)


def BitsSwapped(subcon):
    """创建一个 BitsSwapped 描述符。

    对每字节的 bit 序进行翻转。对应 Python construct 的 ``BitsSwapped``。

    使用方式::

        d = BitsSwapped(Bitwise(Bytes(8)))
        d.parse(b"\\x01")  # bit 反序后解析

    注意：Phase 3 的 ``TransformNode`` 要求定长 subcon（设计 §13.5 已知限制）。

    :param subcon: 被包裹的子构造器。
    :return: ``BitsSwappedDescriptor`` 实例。
    """
    return BitsSwappedDescriptor(subcon)


class ByteSwappedDescriptor:
    """``ByteSwapped(subcon)`` 描述符。

    整体字节序翻转：bytes 反序（[0x01, 0x02] → [0x02, 0x01]）。
    对应 Python construct 的 ``ByteSwapped``（基于 ``swapbytes`` 变换）。

    ``_expr_params`` 协议返回空 dict。
    """

    __slots__ = ("subcon",)

    def __init__(self, subcon):
        """初始化 ByteSwapped 描述符。

        :param subcon: 被包裹的子构造器（必须定长）。
        """
        self.subcon = subcon

    _expr_params = {}

    def __repr__(self):
        return "ByteSwapped({!r})".format(self.subcon)


def ByteSwapped(subcon):
    """创建一个 ByteSwapped 描述符。

    对字节序进行整体翻转。对应 Python construct 的 ``ByteSwapped``。

    使用方式::

        Int24ul <--> ByteSwapped(Int24ub) <--> BytesInteger(3, swapped=True)

    注意：Phase 3 的 ``TransformNode`` 要求定长 subcon。

    :param subcon: 被包裹的子构造器。
    :return: ``ByteSwappedDescriptor`` 实例。
    """
    return ByteSwappedDescriptor(subcon)


# ---------------------------------------------------------------------------
# Phase 4: Array 描述符
#
# 设计依据：``docs/模块设计-Array.md`` §6.2.2 / §6.3。
#
# ``Array(count, subcon, discard=False)`` 是固定次数数组描述符，对应 Python
# construct 的 ``Array``。construct-rs 通过 type name "ArrayDescriptor" 识别，
# 构建 ``Node::Array(ArrayNode)``。
#
# count 支持：
# - int 常量 → ``CountSource::Const``
# - FieldRef/ExprRef → ``CountSource::Expr``（从 expr_programs 取 "count" 键）
#
# 限制（§6.2.2 P3.1）：inner subcon 暂不支持含表达式的子描述符
# （如 ``Array(N, Bytes(this.m))``）。需将 inner 表达式扁平化到字段层级。
# ---------------------------------------------------------------------------


class ArrayDescriptor:
    """``Array(count, subcon, discard=False)`` 描述符。

    固定次数数组：解析 ``count`` 个 ``subcon`` 元素到 Python list，
    或从 list 构建 ``count`` 个元素的字节序列。

    对应 Python construct 的 ``Array``。

    ``_expr_params`` 协议：当 ``count`` 是 int 时返回 ``{}``（跳过编译）；
    当 count 是 FieldRef/ExprRef 时返回 ``{"count": self.count}``（编译为 ExprOp 列表）。

    :param count: 元素数量（int 常量或 FieldRef/ExprRef 表达式）。
    :param subcon: 元素子构造器（描述符）。
    :param discard: 若为 True，解析时仍消耗流但不收集结果（返回空 list）。
    """

    __slots__ = ("count", "subcon", "discard")

    def __init__(self, count, subcon, discard=False):
        """初始化 Array 描述符。

        :param count: 元素数量。
        :param subcon: 元素子构造器。
        :param discard: 是否丢弃解析结果。
        """
        self.count = count
        self.subcon = subcon
        self.discard = discard

    @property
    def _expr_params(self):
        """表达式参数协议（与 BytesDescriptor._expr_params 同模式）。

        返回 ``{"count": self.count}``。当 count 是 int 时，
        ``_extract_and_compile_exprs`` 跳过编译；当 count 是 FieldRef/ExprRef 时
        编译为 ExprOp 列表。
        """
        if isinstance(self.count, int):
            return {}
        return {"count": self.count}

    def __repr__(self):
        return "Array(count={!r}, subcon={!r}, discard={!r})".format(
            self.count, self.subcon, self.discard
        )


def Array(count, subcon, discard=False):
    """创建一个 Array 描述符。

    固定次数数组。对应 Python construct 的 ``Array``。

    使用方式::

        @dataclass
        class Header(StructMixin):
            count: int = field(Int8ub)
            items: list = field(Array(this.count, Byte))

    运算符重载：``Byte[5]`` 等价于 ``Array(5, Byte)``（Python construct 推荐语法）。

    限制（Phase 4）：inner subcon 不支持含表达式的子描述符（如
    ``Array(N, Bytes(this.m))``）。若需 inner 表达式，请将 inner 扁平化为
    独立字段。

    :param count: 元素数量（int 或 FieldRef/ExprRef 表达式）。
    :param subcon: 元素子构造器。
    :param discard: 若为 True，解析返回空 list 但仍消耗流。
    :return: ``ArrayDescriptor`` 实例。
    """
    return ArrayDescriptor(count, subcon, discard)


# ---------------------------------------------------------------------------
# Phase 4 子任务 4.2: GreedyRange 描述符
#
# 设计依据：``docs/模块设计-Array.md`` §4.2 / §6.3。
#
# ``GreedyRange(subcon, discard=False)`` 是读到流结束的数组描述符，对应 Python
# construct 的 ``GreedyRange``。construct-rs 通过 type name "GreedyRangeDescriptor"
# 识别，构建 ``Node::GreedyRange(GreedyRangeNode)``。
#
# parse 终止条件：
# - 子构造器返回错误（EOF、字节不足等）：seek 回 fallback + 正常终止
# - StopField（StopIf 触发）：seek 回 fallback + 正常终止
# ---------------------------------------------------------------------------


class GreedyRangeDescriptor:
    """``GreedyRange(subcon, discard=False)`` 描述符。

    读到流结束的数组：解析零或多个 ``subcon`` 元素到 Python list，
    直到流末尾或子构造器解析失败。对应 Python construct 的 ``GreedyRange``。

    ``_expr_params`` 协议返回空 dict：GreedyRange 无表达式参数（无 count，
    subcon 由递归处理）。

    :param subcon: 元素子构造器（描述符）。
    :param discard: 若为 True，解析时仍消耗流但不收集结果（返回空 list）。
    """

    __slots__ = ("subcon", "discard")

    def __init__(self, subcon, discard=False):
        """初始化 GreedyRange 描述符。

        :param subcon: 元素子构造器。
        :param discard: 是否丢弃解析结果。
        """
        self.subcon = subcon
        self.discard = discard

    # 类级别常量：GreedyRange 无表达式参数。
    _expr_params = {}

    def __repr__(self):
        return "GreedyRange(subcon={!r}, discard={!r})".format(self.subcon, self.discard)


def GreedyRange(subcon, discard=False):
    """创建一个 GreedyRange 描述符。

    读到流结束的数组。对应 Python construct 的 ``GreedyRange``。

    使用方式::

        @dataclass
        class Packet(StructMixin):
            magic: int = field(Int8ub)
            payload: list = field(GreedyRange(Int8ub))  # 读到 EOF

    parse 行为：
    - 子构造器解析失败（如 EOF、字节不足）：seek 回最后一次成功位置，正常终止
    - StopIf 触发（StopField 哨兵）：正常终止

    限制（Phase 4）：
    - sizeof 永远返回 ``SizeofError``（元素数量运行时未知）
    - 嵌套 ``GreedyRange(GreedyRange(...))`` 在 EOF 时会无限循环（与 Python
      construct 行为一致），属用户误用。请改用 ``GreedyRange(Array(N, ...))``
      或 ``Array(N, GreedyRange(...))``。

    :param subcon: 元素子构造器。
    :param discard: 若为 True，解析返回空 list 但仍消耗流。
    :return: ``GreedyRangeDescriptor`` 实例。
    """
    return GreedyRangeDescriptor(subcon, discard)


# ---------------------------------------------------------------------------
# Phase 4 子任务 4.3: PrefixedArray 描述符
#
# 设计依据：``docs/模块设计-Array.md`` §4.6 / §6.3。
#
# ``PrefixedArray(countfield, subcon)`` 是前缀长度数组描述符，对应 Python
# construct 的 ``PrefixedArray``。construct-rs 通过 type name "PrefixedArrayDescriptor"
# 识别，构建 ``Node::PrefixedArray(PrefixedArrayNode)``。
#
# 与 Python 原版的关键差异：
# - construct-rs 不依赖 FocusedSeq/Rebuild（未实现），而是独立 Node
#   （设计决策 A6，§5.1 选项 A）。
# - ``len_`` 辅助函数不实现（§6.3.3）。
# ---------------------------------------------------------------------------


class PrefixedArrayDescriptor:
    """``PrefixedArray(countfield, subcon)`` 描述符。

    前缀长度数组：先解析/构建 ``countfield`` 得到元素数量 ``count``，再循环 ``count``
    次 ``subcon`` 的解析/构建。对应 Python construct 的 ``PrefixedArray``。

    construct-rs 用独立 Node 实现（不依赖 FocusedSeq/Rebuild）。

    ``_expr_params`` 协议返回空 dict：PrefixedArray 自身无表达式参数
    （countfield 与 subcon 由递归处理，P3.1 限制下两者均不支持含表达式的子描述符）。

    :param countfield: 计数字段（描述符），常见 ``Int8ub`` / ``Int16ub`` / ``VarInt``。
                       build 时取 list 长度作为 countfield 的输入值。
    :param subcon: 元素子构造器（描述符）。
    """

    __slots__ = ("countfield", "subcon")

    def __init__(self, countfield, subcon):
        """初始化 PrefixedArray 描述符。

        :param countfield: 计数字段（必须能产生/接收整数）。
        :param subcon: 元素子构造器。
        """
        self.countfield = countfield
        self.subcon = subcon

    # 类级别常量：PrefixedArray 无表达式参数。
    _expr_params = {}

    def __repr__(self):
        return "PrefixedArray(countfield={!r}, subcon={!r})".format(
            self.countfield, self.subcon
        )


def PrefixedArray(countfield, subcon):
    """创建一个 PrefixedArray 描述符。

    前缀长度数组。对应 Python construct 的 ``PrefixedArray``。

    使用方式::

        @dataclass
        class Packet(StructMixin):
            items: list = field(PrefixedArray(Int8ub, Int32ub))
            # 等价于 Python：PrefixedArray(Byte, Int32ub)

    parse 行为：
    - 先解析 ``countfield`` 得到 count（必须是非负整数，否则 RangeError）
    - 循环 count 次解析 ``subcon``，返回 Python 原生 list（非 ListContainer）

    build 行为：
    - 取 list 长度，先 build ``countfield`` 写入长度
    - 遍历 list build ``subcon``

    限制（Phase 4）：
    - sizeof 永远返回 ``SizeofError``（元素数量运行时未知，§4.6.4）
    - countfield 与 subcon 均不支持含表达式的子描述符（与 ``Array`` / ``Bitwise`` 同限制）
    - count 超出 countfield 表示范围时由 countfield 节点自行报错（PA-5）

    :param countfield: 计数字段（描述符）。
    :param subcon: 元素子构造器。
    :return: ``PrefixedArrayDescriptor`` 实例。
    """
    return PrefixedArrayDescriptor(countfield, subcon)


# ---------------------------------------------------------------------------
# Phase 4 子任务 4.4: Index / StopIf 描述符
#
# 设计依据：``docs/模块设计-Array.md`` §4.4 / §4.5 / §6.3。
#
# - ``Index()``：取当前数组迭代下标（对应 Python construct 的 ``Index``）。
#   construct-rs 中 IndexNode 是用户访问数组下标的唯一机制（v3 决策 §3.3），
#   直接调 ``ctx.index()`` 读取，不走 ExprProgram。
#
# - ``StopIf(condfunc)``：早停信号（对应 Python construct 的 ``StopIf``）。
#   条件为 true 时抛 StopFieldError 哨兵，被外层 Struct / GreedyRange 捕获，
#   停止后续字段/迭代。
#   condfunc 支持：
#   - Python bool 常量（True/False）→ StopIfCondition::Always / Never
#   - FieldRef/ExprRef 表达式 → StopIfCondition::Expr
# ---------------------------------------------------------------------------


class IndexDescriptor:
    """``Index()`` 描述符。

    取当前数组迭代下标。对应 Python construct 的 ``Index``。

    必须在 ``Array`` / ``GreedyRange`` / ``RepeatUntil`` / ``PrefixedArray``
    内使用。在数组外使用时 parse 返回 ``None``（对齐 Python
    ``context.get("_index", None)``）。

    ``_expr_params`` 协议返回空 dict：Index 无表达式参数（v3 决策，§3.3）。
    """

    __slots__ = ()

    # 类级别常量：Index 无表达式参数。
    _expr_params = {}

    def __repr__(self):
        return "Index()"


def Index():
    """创建一个 Index 描述符。

    取当前数组迭代下标。对应 Python construct 的 ``Index``。

    使用方式::

        @dataclass
        class Item(StructMixin):
            i: int = rfield(Index())      # 当前下标
            v: int = field(Int8ub)

        @dataclass
        class Packet(StructMixin):
            items: list = field(Array(3, Item))

        Packet.parse(b"\\x01\\x02\\x03")
        # items = [Item(i=0, v=1), Item(i=1, v=2), Item(i=2, v=3)]

    在数组外使用时，``Index()`` parse 返回 ``None``（IX-2）：

    ::

        @dataclass
        class Top(StructMixin):
            idx: int = rfield(Index())    # 不在数组内，parse 得到 None

    在表达式中引用下标（v3 决策，§3.3）：先用 Index 字段声明，再用字段名引用::

        @dataclass
        class Item(StructMixin):
            i: int = rfield(Index())
            v: bytes = field(Bytes(i + 1))   # 引用字段名 i，编译为 [GetInt(0), Const(1), Add]

    :return: ``IndexDescriptor`` 实例。
    """
    return IndexDescriptor()


# ---------------------------------------------------------------------------
# Phase 4 子任务 4.5 v5: Element 描述符（v5 新增）
#
# 设计依据：``docs/模块设计-Array.md`` §4.7 / §6.3.1。
#
# ``Element()`` 是 RepeatUntil 终止表达式中"当前元素"的引用入口。
# 与 Index 字段平行（Index 是 Array 内"当前下标"引用入口）。
# 必须在 ``rfield()`` 中使用（RO 模式），声明在 RepeatUntil 字段之前。
# ---------------------------------------------------------------------------


class ElementDescriptor:
    """``Element()`` 描述符（v5 新增）。

    RepeatUntil 终止表达式中"当前元素"的引用入口。无参数。

    必须在 ``rfield()`` 中使用（RO 模式），且必须声明在 RepeatUntil 字段之前。
    Element 字段在 Packet 实例中始终为 None（不持有真实数据）——其值由
    RepeatUntilNode 在迭代时通过 set_field_at 借用设置。

    与 Index 字段平行（设计决策记录 Phase 4 决策 3 + 决策 5）：用户面形式一致
    （``rfield(<构造器字段>())`` + 字段名引用），表达式系统输入类型保持纯粹
    （仅 ``_FieldDescriptor`` / ``_ExprRef`` / ``int``，不引入新 ExprOp 指令）。

    ``_expr_params`` 协议返回空 dict：Element 无表达式参数。

    设计依据：``docs/模块设计-Array.md`` §4.7。
    """

    __slots__ = ()

    # 类级别常量：Element 无表达式参数。
    _expr_params = {}

    def __repr__(self):
        return "Element()"


def Element():
    """创建一个 Element 描述符（v5 新增）。

    RepeatUntil 终止表达式中"当前元素"的引用入口。

    使用方式（在 RepeatUntil 终止表达式中引用当前元素）::

        @dataclass
        class Packet(StructMixin):
            e: int = rfield(Element())                # Element 字段
            payload: list = field(RepeatUntil(e > 5, Int8ub))

        Packet.parse(b"\\x01\\x02\\x06\\xAA")
        # Packet(e=None, payload=[1, 2, 6])    # 最后元素 6 满足 e > 5

    Element 字段在 Packet 实例中始终为 None（值由 RepeatUntilNode 借用设置）。

    限制（Phase 4.5 v5）：

    - 必须为 RO 模式（``rfield(Element())``），编译期校验
    - 必须声明在 RepeatUntil 字段之前（前序字段引用约束）
    - 同 Struct 内建议 ≤1 个 Element 字段（多个无意义）

    :return: ``ElementDescriptor`` 实例。
    """
    return ElementDescriptor()


class StopIfDescriptor:
    """``StopIf(condfunc)`` 描述符。

    早停信号：检查条件，条件为真时返回 ``StopFieldError`` 哨兵，
    被外层 ``Struct`` / ``GreedyRange`` 捕获，停止后续字段/迭代。

    对应 Python construct 的 ``StopIf``。

    ``_expr_params`` 协议：当 ``condfunc`` 是 bool 常量时返回 ``{}``（跳过编译）；
    当 condfunc 是 FieldRef/ExprRef 时返回 ``{"cond": self.condfunc}``（编译为 ExprOp 列表）。

    :param condfunc: 条件（``True`` / ``False`` / FieldRef / ExprRef 表达式）。
    """

    __slots__ = ("condfunc",)

    def __init__(self, condfunc):
        """初始化 StopIf 描述符。

        :param condfunc: 条件。``True`` 永远停止（调试用），``False`` 永远不停止（调试用），
                         或 FieldRef/ExprRef 表达式（如 ``x == 0``）。
        """
        self.condfunc = condfunc

    @property
    def _expr_params(self):
        """表达式参数协议（与 ComputedDescriptor._expr_params 同模式）。

        返回 ``{"cond": self.condfunc}``。当 condfunc 是 bool 时，
        ``_extract_and_compile_exprs`` 跳过编译；当 condfunc 是 FieldRef/ExprRef 时
        编译为 ExprOp 列表。
        """
        if isinstance(self.condfunc, bool):
            return {}
        return {"cond": self.condfunc}

    def __repr__(self):
        return "StopIf({!r})".format(self.condfunc)


def StopIf(condfunc):
    """创建一个 StopIf 描述符。

    早停信号。对应 Python construct 的 ``StopIf``。

    使用方式（在 Struct 中）::

        @dataclass
        class Packet(StructMixin):
            x: int = field(Int8ub)
            stop = rfield(StopIf(x == 0))    # x 为 0 时停止
            y: int = field(Int8ub)           # 仅在 x != 0 时解析

        Packet.parse(b"\\x00")              # x=0, stop 触发，y 不解析
        # Packet(x=0, y=None)
        Packet.parse(b"\\x05\\x99")         # x=5, 不停, y=0x99
        # Packet(x=5, y=0x99)

    使用方式（在 GreedyRange 中，GR-4）::

        @dataclass
        class Item(StructMixin):
            x: int = field(Int8ub)
            stop = rfield(StopIf(x == 0xFF))   # 0xFF 为终止符

        items = GreedyRange(Item)
        items.parse(b"\\x01\\x02\\xFF")
        # [Item(x=1), Item(x=2), Item(x=0xFF)]

    注：``StopIf`` 在 ``Array`` 内不捕获（设计 SI-3，Array 是固定次数，
    StopIf 在 Array 内是用户误用，错误向上传播）。

    :param condfunc: 条件（``True`` / ``False`` / 字段名引用表达式）。
    :return: ``StopIfDescriptor`` 实例。
    """
    return StopIfDescriptor(condfunc)


# ---------------------------------------------------------------------------
# Phase 4 子任务 4.5: RepeatUntil 描述符
#
# 设计依据：``docs/模块设计-Array.md`` §4.3 / §6.3 / §10.1。
#
# ``RepeatUntil(predicate, subcon, discard=False)`` 是终止表达式数组描述符，对应
# Python construct 的 ``RepeatUntil``。construct-rs 通过 type name "RepeatUntilDescriptor"
# 识别，构建 ``Node::RepeatUntil(RepeatUntilNode)``。
#
# 两条终止表达式路径（设计 §4.3.1 / §2.5）：
# - **Expr 路径**（性能 ≥8x）：简单 lambda（如 ``lambda x,_,_: x > 5``）编译期
#   通过 AST 识别并翻译为 ExprProgram ``[GetElem, Const(N), Op]``，运行时零 FFI。
#   仅支持整数元素 + 6 种比较运算（>, >=, ==, !=, <, <=）。
# - **PyCallable 路径**（性能 ≥3x）：复杂 lambda 回落到 Python callable，
#   每次迭代跨 FFI 调用。
# ---------------------------------------------------------------------------


# ---------------------------------------------------------------------------
# Phase 4 子任务 4.5 v5: RepeatUntil 描述符（v5 完全重写）
#
# 设计依据：``docs/模块设计-Array.md`` §4.3 / §6.3.1 / §13（v5）。
#
# v5 用户硬约束：
# - 删除 AST 识别器（``_AST_OP_TO_EXPROP`` / ``_try_compile_repeat_predicate``）
# - 不再接收 Python lambda/callable（用户硬约束 #1）
# - 第一参数从 ``predicate`` 改名为 ``terminator``（用户硬约束 #3，统一使用"终止表达式"表述）
# - terminator 必须是 Phase 2 表达式（``_FieldDescriptor`` / ``_ExprRef`` / ``int`` 组合）
#   编译为 ExprProgram，运行时零 FFI 求值（用户硬约束 #4）
# ---------------------------------------------------------------------------


class RepeatUntilDescriptor:
    """``RepeatUntil(terminator, subcon, discard=False)`` 描述符（v5 重写）。

    终止表达式数组：解析元素到 list 直到终止表达式求值非零（最后元素包含在内），
    或从 list 构建字节序列直到某元素满足终止表达式。对应 Python construct 的 ``RepeatUntil``。

    :param terminator: 终止表达式（Phase 2 表达式：``_FieldDescriptor`` / ``_ExprRef`` /
                       ``int`` 组合）。必须引用同 Struct 中已声明的 Element 字段
                       （如 ``e > 5``，其中 ``e`` 是 ``rfield(Element())``）。
                       求值结果非零即终止。不接收 Python lambda / callable。
    :param subcon: 元素子构造器（描述符）。
    :param discard: 若为 True，parse 返回空 list 但仍消耗流。

    ``_expr_params`` 协议（v5）：

    - 返回 ``{"terminator": <expr>, "element_field_idx": <int>}``（编译期由
      ``set_compiled_expr_params`` 注入）。
    - ``terminator`` 是终止表达式（由 ``_compile_expr_tree`` 编译为 ExprOp 列表）。
    - ``element_field_idx`` 是终止表达式引用的 Element 字段在 Struct 中的索引
      （由 ``_compile_expr_tree`` 在编译时从表达式中提取的 GetInt 索引推导）。

    与 Python construct 的差异：

    - **不接收 Python lambda / callable**：用户硬约束 #1（避免隐藏决策路径）。
      Phase 2 表达式 VM 无法描述的终止逻辑（如 list 切片），用户须改用 Adapter
      （显式慢路径）。详见 §13.9 能力边界。
    - **discard 语义简化**：v5 中 discard 仅影响 parse 方向 list 收集；
      build 方向 discard 不影响终止表达式求值（终止表达式不接收 list 参数）。
    """

    __slots__ = ("terminator", "subcon", "discard", "_expr_params", "_element_field_idx")

    def __init__(self, terminator, subcon, discard=False):
        """初始化 RepeatUntil 描述符（v5 重写）。

        :param terminator: 终止表达式（Phase 2 表达式）。
        :param subcon: 元素子构造器。
        :param discard: 是否丢弃解析结果。
        """
        # 用户硬约束 #1：terminator 不能是 callable（v5 删除 PyCallable 路径）。
        # callable 包括 lambda / 函数 / 实现了 __call__ 的类实例。
        if callable(terminator):
            raise CompilationError(
                "RepeatUntil terminator must be a Phase 2 expression "
                "(_FieldDescriptor / _ExprRef / int), not a Python callable. "
                "Example: RepeatUntil(e > 5, Int8ub) where 'e' is rfield(Element()). "
                "For complex termination logic depending on list/context, "
                "use Adapter (explicit slow path)."
            )
        self.terminator = terminator
        self.subcon = subcon
        self.discard = discard
        # _element_field_idx 由 _compile_expressions 在编译期填充（延迟设置）。
        self._element_field_idx = None
        # _expr_params 初始为 {"terminator": <expr>}，set_compiled_expr_params
        # 会替换为含 element_field_idx 的版本。
        self._expr_params = {"terminator": terminator}

    def set_compiled_expr_params(self, ops, element_field_idx, index_field_indices=None):
        """编译期由 _compile_expressions 调用，注入已编译的 ops 和 Element/Index 字段索引。

        :param ops: 编译后的 ExprOp 元组列表（如 ``[("getint", 0), ("const", 5), ("gt",)]``）。
        :param element_field_idx: Element 字段在 Struct 中的索引。
        :param index_field_indices: Index 字段索引列表（终止表达式中引用的 Index 字段，
                                     不含 element_field_idx）。每次迭代同步为当前下标。
        """
        self._element_field_idx = element_field_idx
        if index_field_indices is None:
            index_field_indices = []
        self._expr_params = {
            "terminator": ops,
            "element_field_idx": element_field_idx,
            "index_field_indices": index_field_indices,
        }

    def __repr__(self):
        return "RepeatUntil(terminator={!r}, subcon={!r}, discard={!r})".format(
            self.terminator, self.subcon, self.discard
        )


def RepeatUntil(terminator, subcon, discard=False):
    """创建一个 RepeatUntil 描述符（v5 重写）。

    终止表达式数组。对应 Python construct 的 ``RepeatUntil``。

    使用方式（终止表达式 = Phase 2 表达式，引用 Element 字段）::

        @dataclass
        class Packet(StructMixin):
            e: int = rfield(Element())                # 当前元素引用入口
            payload: list = field(RepeatUntil(e > 5, Int8ub))

        Packet.parse(b"\\x01\\x02\\x06\\xAA")
        # Packet(e=None, payload=[1, 2, 6])    # 最后元素 6 满足 e > 5

    限制（Phase 4.5 v5）：

    - **不接收 Python lambda / callable**（用户硬约束 #1）。如传入 callable，
      ``RepeatUntilDescriptor.__init__`` 立即抛 ``CompilationError``。
    - 终止表达式必须引用 Element 字段（编译期校验，否则 ``terminator must reference
      an Element field`` 错误）。
    - sizeof 永远返回 ``SizeofError``
    - inner subcon 不支持含表达式的子描述符（与 ``Array`` / ``Bitwise`` 同限制）

    Phase 2 表达式 VM 无法描述的终止逻辑（如 list 切片、字符串比较），
    用户须改用 Adapter（显式慢路径）。详见 §13.9 能力边界。

    :param terminator: 终止表达式（Phase 2 表达式，引用 Element 字段）。
    :param subcon: 元素子构造器。
    :param discard: 若为 True，parse 返回空 list 但仍消耗流。
    :return: ``RepeatUntilDescriptor`` 实例。
    """
    return RepeatUntilDescriptor(terminator, subcon, discard)


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
    # Phase 3.3 Padding / Bytewise / BitsSwapped / ByteSwapped
    "PaddingDescriptor",
    "Padding",
    "BytewiseDescriptor",
    "Bytewise",
    "BitsSwappedDescriptor",
    "BitsSwapped",
    "ByteSwappedDescriptor",
    "ByteSwapped",
    # Phase 4 Array / GreedyRange / PrefixedArray
    "ArrayDescriptor",
    "Array",
    "GreedyRangeDescriptor",
    "GreedyRange",
    "PrefixedArrayDescriptor",
    "PrefixedArray",
    # Phase 4 Index / StopIf / RepeatUntil
    "IndexDescriptor",
    "Index",
    "StopIfDescriptor",
    "StopIf",
    "RepeatUntilDescriptor",
    "RepeatUntil",
    # Phase 4.5 v5 Element
    "ElementDescriptor",
    "Element",
]
