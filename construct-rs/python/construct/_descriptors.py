"""类型描述符的 Python 侧重导出（纯 Python 委托层）。

设计依据：``docs/design/基础设施/架构设计.md`` §A.4（类型描述符）、§D.3（Python 包结构）、
``docs/design/模块设计/模块设计-BitStream.md`` §8.2（bit 描述符）。

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


# BytesInteger 作为 BytesIntegerDescriptor 的别名导出（Phase 6.1）。
# 用户通过 BytesInteger(3, signed=True) 创建 BytesIntegerDescriptor 实例。
try:
    from ._construct_rust import (
        BytesIntegerDescriptor,
        # 同时暴露 BytesInteger 别名（Python construct 兼容名）。
    )
    BytesInteger = BytesIntegerDescriptor
    # Phase 6.1 Float 单例（FormatFieldDescriptor pyclass 实例）
    from ._construct_rust import (
        Float16b,
        Float16l,
        Float32b,
        Float32l,
        Float64b,
        Float64l,
        # Phase 6.1 Int24 单例（BytesIntegerDescriptor pyclass 实例）
        Int24ub,
        Int24ul,
        Int24sb,
        Int24sl,
    )
except ImportError:  # pragma: no cover - 仅在扩展未构建时触发
    pass


# ---------------------------------------------------------------------------
# Phase 3.1: BitsInteger / Bit / Nibble / Octet 纯 Python 描述符
#
# 设计依据：``docs/design/模块设计/模块设计-BitStream.md`` §8.2。
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
# 设计依据：``docs/design/模块设计/模块设计-BitStream.md`` §8.2、§11。
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
# 设计依据：``docs/design/模块设计/模块设计-BitStream.md`` §4.2 / §4.4 / §4.5 / §4.6 / §8.2。
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
# 设计依据：``docs/design/模块设计/模块设计-Array.md`` §6.2.2 / §6.3。
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
# （如 ``Array(N, Bytes(m))`` 中 ``m`` 是字段引用）。需将 inner 表达式
# 扁平化到字段层级（``m`` 必须是同 Struct 内已声明的 int 字段）。
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

    使用方式（count 为字段引用，``count`` 必须是同 Struct 内已声明的 int 字段）::

        @dataclass
        class Header(StructMixin):
            count: int = field(Int8ub)
            items: list = field(Array(count, Byte))

    运算符重载：``Byte[5]`` 等价于 ``Array(5, Byte)``（Python construct 推荐语法）。

    限制（Phase 4）：inner subcon 不支持含表达式的子描述符（如
    ``Array(N, Bytes(m))``）。若需 inner 表达式，请将 inner 扁平化为
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
# 设计依据：``docs/design/模块设计/模块设计-Array.md`` §4.2 / §6.3。
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
# 设计依据：``docs/design/模块设计/模块设计-Array.md`` §4.6 / §6.3。
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
# 设计依据：``docs/design/模块设计/模块设计-Array.md`` §4.4 / §4.5 / §6.3。
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
# 设计依据：``docs/design/模块设计/模块设计-Array.md`` §4.7 / §6.3.1。
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

    设计依据：``docs/design/模块设计/模块设计-Array.md`` §4.7。
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
# 设计依据：``docs/design/模块设计/模块设计-Array.md`` §4.3 / §6.3 / §10.1。
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
# 设计依据：``docs/design/模块设计/模块设计-Array.md`` §4.3 / §6.3.1 / §13（v5）。
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

    - **不接收 Python lambda / callable**（用户硬约束 #1）。如传入 callable,
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


# ---------------------------------------------------------------------------
# Phase 6.1: VarInt / ZigZag 描述符
#
# 设计依据：``docs/design/模块设计/模块设计-Primitives收尾.md`` §1.4。
#
# 这些描述符是纯 Python 类（不需要 Rust pyclass），通过 type name 识别。
# compile.rs 的 build_node_from_descriptor 通过 ``type(desc).__name__``
# 匹配到 "VarIntDescriptor" / "ZigZagDescriptor" 字符串，构建对应的 Node 变体。
#
# VarInt: LEB128 无符号变长整数（Google Protocol Buffers 编码）。
# ZigZag: 有符号变长整数（VarInt + ZigZag 数值变换）。
# ---------------------------------------------------------------------------


class VarIntDescriptor:
    """``VarInt()`` 描述符。

    LEB128 无符号变长整数。对应 Python construct 的 ``VarInt``
    （core.py L1601-L1647）。

    编码规则：每字节低 7 位是有效数据，最高位 MSB=1 表示后续还有字节，
    MSB=0 表示终止。小整数（0-127）仅需 1 字节。

    限制：

    - 仅支持非负整数（build 负数返回 ``IntegerError``，与 Python 一致）
    - sizeof 永远返回 ``SizeofError``（变长，无法预知）

    ``_expr_params`` 协议返回空 dict：VarInt 无表达式参数。
    """

    __slots__ = ()

    # 类级别常量：VarInt 无表达式参数。
    _expr_params = {}

    def __repr__(self):
        return "VarInt()"


# VarInt singleton（无参数，全局共享一个实例）。
VarInt = VarIntDescriptor()


class ZigZagDescriptor:
    """``ZigZag()`` 描述符。

    有符号变长整数。对应 Python construct 的 ``ZigZag``
    （core.py L1651-L1686）。

    ZigZag 将有符号整数映射为无符号整数后再用 LEB128 编码：
    0 → 0, -1 → 1, 1 → 2, -2 → 3, 2 → 4, ...

    sizeof 永远返回 ``SizeofError``（变长）。

    ``_expr_params`` 协议返回空 dict。
    """

    __slots__ = ()

    # 类级别常量：ZigZag 无表达式参数。
    _expr_params = {}

    def __repr__(self):
        return "ZigZag()"


# ZigZag singleton（无参数，全局共享一个实例）。
ZigZag = ZigZagDescriptor()


# ---------------------------------------------------------------------------
# Phase 6.1: 整数别名（Byte / Short / Int / Long）
#
# 设计依据：``docs/design/模块设计/模块设计-Primitives收尾.md`` §1.2.1。
#
# 这 4 个是 Python 层别名到现有 FormatField 单例：
#   Byte  = Int8ub   (FormatField(">", "B"))
#   Short = Int16ub  (FormatField(">", "H"))
#   Int   = Int32ub  (FormatField(">", "L") → PythonFormat::UnsignedInt32Big)
#   Long  = Int64ub  (FormatField(">", "Q"))
# 零 Rust 改动，仅在 Python wrapper 模块加 alias。
# ---------------------------------------------------------------------------
try:
    Byte = Int8ub
    Short = Int16ub
    Int = Int32ub
    Long = Int64ub
    # Python construct 兼容别名（Half/Single/Double，可选）
    Half = Float16b
    Single = Float32b
    Double = Float64b
except NameError:  # pragma: no cover - 仅在扩展未构建时触发
    pass


# ---------------------------------------------------------------------------
# Phase 6.2: Strings 描述符（6 个）
#
# 设计依据：``docs/design/模块设计/模块设计-Strings.md`` v2 §4.4 / §10.2。
#
# 这些描述符是纯 Python 类（不需要 Rust pyclass），通过 type name 识别。
# compile.rs 的 build_node_from_descriptor 通过 ``type(desc).__name__``
# 匹配到 "CStringDescriptor" / "GreedyStringDescriptor" / ... 字符串，
# 构建对应的 Node 变体（详见 compile.rs Phase 6.2 分支）。
#
# **编码字符串（encoding）**：用户传入字符串（如 ``"utf8"`` / ``"utf_16_le"``），
# Rust 编译期调 ``Encoding::from_user_str`` 解析为 ``Encoding`` enum。
# 不接受无后缀编码（``utf16``/``utf32`` 等），错误信息引导用户用显式 _le/_be。
# （PM 决策 6.2-D1 + P2b + P8，详见 Strings 设计 §2.1.3）
#
# **length 字段（PaddedString）**：可为 int 或 FieldRef/ExprRef 表达式，
# 与 BytesDescriptor 同模式（``_expr_params`` 协议）。
#
# 与 Python construct 的差异（设计 §3.7.3 + Strings §10 P5）：
#
# - **StringEncoded 不实现为 Node**：兼容别名一调用即抛 StringError（_errors.py）
# - **无后缀编码拒绝**：``utf16``/``utf_16``/``u16``/``utf32``/``utf_32``/``u32``
#   在编译期被拒绝，引导用户改用 ``utf_16_le`` / ``utf_16_be`` 等
# ---------------------------------------------------------------------------


class CStringDescriptor:
    """``CString(encoding, term=None, include=False, consume=True, require=True)`` 描述符。

    C 风格 null 终止字符串。对应 Python construct 的 ``CString``（core.py L1811）。

    Python 原版是 ``StringEncoded(NullTerminated(GreedyBytes, term=...), encoding)``
    macro 嵌套；construct-rs 独立实现（PM 决策 1 方案 A）。

    编码字符串在 Rust 编译期解析（``Encoding::from_user_str``）。

    ``_expr_params`` 协议返回空 dict：CString 无表达式参数。

    :param encoding: 编码字符串（如 ``"utf8"`` / ``"utf_16_le"``）。
    :param term: 终止符字节串。默认 ``None`` 表示用编码单元的全零字节串
                  （utf8/ascii = ``b"\\x00"``，utf16 = ``b"\\x00\\x00"``，utf32 = 4 个 0）。
    :param include: 是否将 term 包含在解码数据中。默认 False。
    :param consume: 是否消费 term（True=消费；False=seek 回退 unit 字节）。默认 True。
                     **注意**：当前 CStringNode 实现不暴露 consume（与 Python ``CString``
                     实际行为一致，详见 Strings 设计 §3.1）；此参数仅为兼容性保留。
    :param require: 是否在 EOF 时报错。默认 True。
    """

    __slots__ = ("encoding", "term", "include", "consume", "require")

    def __init__(self, encoding, term=None, include=False, consume=True, require=True):
        """初始化 CString 描述符。

        :param encoding: 编码字符串。
        :param term: 终止符字节串（None = 默认）。
        :param include: 是否将 term 包含在解码数据中。
        :param consume: 是否消费 term（保留参数，当前实现忽略）。
        :param require: 是否在 EOF 时报错。
        """
        self.encoding = encoding
        self.term = term
        self.include = include
        self.consume = consume
        self.require = require

    # 类级别常量：CString 无表达式参数。
    _expr_params = {}

    def __repr__(self):
        return "CString(encoding={!r}, term={!r}, include={!r}, require={!r})".format(
            self.encoding, self.term, self.include, self.require
        )


def CString(encoding, **kwargs):
    """创建一个 CString 描述符。

    C 风格 null 终止字符串。对应 Python construct 的 ``CString``。

    使用方式::

        @dataclass
        class P(StructMixin):
            name: str = field(CString("utf8"))

        P.parse(b"hello\\x00")  # → P(name="hello")

    支持的编码（PM 决策 6.2-D1）：``ascii`` / ``utf8`` / ``utf_8`` / ``u8`` /
    ``utf_16_le`` / ``utf_16_be`` / ``utf_32_le`` / ``utf_32_be``。

    **BREAKING CHANGE**：不接受无后缀编码（``utf16`` / ``utf32`` 等），
    会编译期报错引导用户改用显式 ``_le`` / ``_be`` 后缀（Strings 设计 §2.1.3 P2b）。

    :param encoding: 编码字符串。
    :param term: 终止符字节串（None = 编码单元全零字节串）。
    :param include: 是否将 term 包含在解码数据中（默认 False）。
    :param consume: 是否消费 term（保留参数，当前实现忽略）。
    :param require: 是否在 EOF 时报错（默认 True）。
    :return: ``CStringDescriptor`` 实例。
    """
    return CStringDescriptor(encoding, **kwargs)


class GreedyStringDescriptor:
    """``GreedyString(encoding)`` 描述符。

    读到流结束并解码。对应 Python construct 的 ``GreedyString``（core.py L1837）。

    Python 原版是 ``StringEncoded(GreedyBytes, encoding)`` macro 嵌套；
    construct-rs 独立实现（PM 决策 1 方案 A）。

    ``_expr_params`` 协议返回空 dict：GreedyString 无表达式参数。

    :param encoding: 编码字符串。
    """

    __slots__ = ("encoding",)

    def __init__(self, encoding):
        """初始化 GreedyString 描述符。

        :param encoding: 编码字符串。
        """
        self.encoding = encoding

    # 类级别常量：GreedyString 无表达式参数。
    _expr_params = {}

    def __repr__(self):
        return "GreedyString(encoding={!r})".format(self.encoding)


def GreedyString(encoding):
    """创建一个 GreedyString 描述符。

    读到流结束并解码。对应 Python construct 的 ``GreedyString``。

    使用方式::

        @dataclass
        class P(StructMixin):
            tail: str = field(GreedyString("utf8"))

        P.parse(b"hello world")  # → P(tail="hello world")

    **BREAKING CHANGE**：不接受无后缀编码（同 CString）。

    :param encoding: 编码字符串。
    :return: ``GreedyStringDescriptor`` 实例。
    """
    return GreedyStringDescriptor(encoding)


class PaddedStringDescriptor:
    """``PaddedString(length, encoding)`` 描述符。

    固定长度填充字符串。对应 Python construct 的 ``PaddedString``（core.py L1747）。

    Python 原版是 ``StringEncoded(FixedSized(length, NullStripped(...)), encoding)``
    三层 macro 嵌套；construct-rs 独立实现（PM 决策 1 方案 A）。

    length 支持：
    - int 常量 → BytesLength::Const
    - FieldRef/ExprRef → BytesLength::Expr（从 expr_programs 取 "length" 键）

    ``_expr_params`` 协议：当 ``length`` 是 int 时返回 ``{}``（跳过编译）；
    当 length 是 FieldRef/ExprRef 时返回 ``{"length": self.length}``（编译为 ExprOp 列表）。

    :param length: 总长度（含数据 + pad），int 或表达式。
    :param encoding: 编码字符串。
    """

    __slots__ = ("length", "encoding")

    def __init__(self, length, encoding):
        """初始化 PaddedString 描述符。

        :param length: 总长度（int 或表达式）。
        :param encoding: 编码字符串。
        """
        self.length = length
        self.encoding = encoding

    @property
    def _expr_params(self):
        """表达式参数协议（与 BytesDescriptor._expr_params 同模式）。

        返回 ``{"length": self.length}``。当 length 是 int 时跳过编译；
        当 length 是 FieldRef/ExprRef 时编译为 ExprOp 列表。
        """
        if isinstance(self.length, int):
            return {}
        return {"length": self.length}

    def __repr__(self):
        return "PaddedString(length={!r}, encoding={!r})".format(self.length, self.encoding)


def PaddedString(length, encoding):
    """创建一个 PaddedString 描述符。

    固定长度填充字符串。对应 Python construct 的 ``PaddedString``。

    使用方式（常量长度）::

        @dataclass
        class P(StructMixin):
            name: str = field(PaddedString(10, "utf8"))

        P.parse(b"hello\\x00\\x00\\x00\\x00\\x00")  # → P(name="hello")

    使用方式（表达式长度，``n`` 必须是同 Struct 内已声明的 int 字段）::

        @dataclass
        class P(StructMixin):
            n: int = field(Int8ub)
            name: str = field(PaddedString(n, "utf8"))

    parse 行为：读 length 字节 → 右剥离 pad（编码单元全零字节串）→ decode。
    build 行为：encode → 若 encoded > length 报 PaddingError，否则补 pad 到 length。

    **BREAKING CHANGE**：不接受无后缀编码（同 CString）。

    :param length: 总长度（int 或表达式）。
    :param encoding: 编码字符串。
    :return: ``PaddedStringDescriptor`` 实例。
    """
    return PaddedStringDescriptor(length, encoding)


class PascalStringDescriptor:
    """``PascalString(lengthfield, encoding)`` 描述符。

    长度前缀字符串。对应 Python construct 的 ``PascalString``（core.py L1778）。

    Python 原版是 ``StringEncoded(Prefixed(lengthfield, GreedyBytes), encoding)``
    macro 嵌套；construct-rs 独立实现（PM 决策 1 方案 A），不依赖 Prefixed（Phase 7）。

    ``_expr_params`` 协议返回空 dict：PascalString 自身无表达式参数
    （lengthfield 由递归处理，与 PrefixedArrayDescriptor 同限制：lengthfield
    不支持含表达式的子描述符）。

    :param lengthfield: 长度字段（描述符），常见 ``VarInt`` / ``Int16ub`` / ``Byte``。
    :param encoding: 编码字符串。
    """

    __slots__ = ("lengthfield", "encoding")

    def __init__(self, lengthfield, encoding):
        """初始化 PascalString 描述符。

        :param lengthfield: 长度字段（必须能产生/接收整数）。
        :param encoding: 编码字符串。
        """
        self.lengthfield = lengthfield
        self.encoding = encoding

    # 类级别常量：PascalString 无表达式参数（lengthfield 由递归处理）。
    _expr_params = {}

    def __repr__(self):
        return "PascalString(lengthfield={!r}, encoding={!r})".format(
            self.lengthfield, self.encoding
        )


def PascalString(lengthfield, encoding):
    """创建一个 PascalString 描述符。

    长度前缀字符串。对应 Python construct 的 ``PascalString``。

    使用方式::

        @dataclass
        class P(StructMixin):
            name: str = field(PascalString(Byte, "utf8"))

        P.parse(b"\\x05hello")  # → P(name="hello")
        P(name="hi").build()    # → b"\\x02hi"

    parse 行为：解析 lengthfield → 读 N 字节 → decode。
    build 行为：encode → 取 encoded.len() build lengthfield → 写 encoded。

    **BREAKING CHANGE**：不接受无后缀编码（同 CString）。

    :param lengthfield: 长度字段（描述符）。
    :param encoding: 编码字符串。
    :return: ``PascalStringDescriptor`` 实例。
    """
    return PascalStringDescriptor(lengthfield, encoding)


class NullTerminatedDescriptor:
    """``NullTerminated(subcon, term=b"\\x00", include=False, consume=True, require=True)`` 描述符。

    null 终止包装器（持有任意 inner subcon）。对应 Python construct 的
    ``NullTerminated``（core.py L5050）。

    与 ``CStringDescriptor`` 共享扫描算法，但 inner 不限于 GreedyBytes，
    可以是 Byte / Bytes(n) / Struct 等。

    ``_expr_params`` 协议返回空 dict（inner subcon 由递归处理，与
    BitwiseDescriptor / PrefixedArrayDescriptor 同限制）。

    :param subcon: 内层子构造器（默认 ``GreedyBytes``，但可传其他）。
    :param term: 终止符字节串。默认 ``b"\\x00"``。
    :param include: 是否将 term 包含在累积数据中（默认 False）。
    :param consume: 是否消费 term（True=消费；False=seek 回退 unit 字节，默认 True）。
    :param require: 是否在 EOF 时报错（默认 True）。
    """

    __slots__ = ("subcon", "term", "include", "consume", "require")

    def __init__(self, subcon, term=b"\x00", include=False, consume=True, require=True):
        """初始化 NullTerminated 描述符。

        :param subcon: 内层子构造器。
        :param term: 终止符字节串。
        :param include: 是否将 term 包含在累积数据中。
        :param consume: 是否消费 term。
        :param require: 是否在 EOF 时报错。
        """
        self.subcon = subcon
        self.term = term
        self.include = include
        self.consume = consume
        self.require = require

    # 类级别常量：NullTerminated 自身无表达式参数。
    _expr_params = {}

    def __repr__(self):
        return "NullTerminated(subcon={!r}, term={!r}, include={!r}, consume={!r}, require={!r})".format(
            self.subcon, self.term, self.include, self.consume, self.require
        )


def NullTerminated(subcon, term=b"\x00", include=False, consume=True, require=True):
    """创建一个 NullTerminated 描述符。

    null 终止包装器。对应 Python construct 的 ``NullTerminated``。

    使用方式（默认 term + GreedyBytes inner）::

        @dataclass
        class P(StructMixin):
            data: bytes = field(NullTerminated(GreedyBytes))

        P.parse(b"hello\\x00")  # → P(data=b"hello")

    使用方式（自定义 inner）::

        @dataclass
        class P(StructMixin):
            value: int = field(NullTerminated(Int32ub, term=b"\\xff\\xff"))

    与 ``CString`` 的区别：``NullTerminated`` 返回 inner 的解析结果
    （通常是 bytes，inner=GreedyBytes 时）；``CString`` 直接返回 str。

    :param subcon: 内层子构造器。
    :param term: 终止符字节串。
    :param include: 是否将 term 包含在累积数据中（默认 False）。
    :param consume: 是否消费 term（默认 True）。
    :param require: 是否在 EOF 时报错（默认 True）。
    :return: ``NullTerminatedDescriptor`` 实例。
    """
    return NullTerminatedDescriptor(subcon, term, include, consume, require)


class NullStrippedDescriptor:
    """``NullStripped(subcon, pad=b"\\x00")`` 描述符。

    null 剥离包装器（持有任意 inner subcon）。对应 Python construct 的
    ``NullStripped``（core.py L5123）。

    parse 行为：读流中剩余所有字节 → 右剥离 pad → 用剥离后字节构造子流调 inner.parse。
    build 行为：直接调 inner.build（不补 pad；pad 补全由外层 PaddedStringNode 内联实现）。

    ``_expr_params`` 协议返回空 dict（inner subcon 由递归处理）。

    :param subcon: 内层子构造器。
    :param pad: pad 字节串。默认 ``b"\\x00"``。
    """

    __slots__ = ("subcon", "pad")

    def __init__(self, subcon, pad=b"\x00"):
        """初始化 NullStripped 描述符。

        :param subcon: 内层子构造器。
        :param pad: pad 字节串。
        """
        self.subcon = subcon
        self.pad = pad

    # 类级别常量：NullStripped 自身无表达式参数。
    _expr_params = {}

    def __repr__(self):
        return "NullStripped(subcon={!r}, pad={!r})".format(self.subcon, self.pad)


def NullStripped(subcon, pad=b"\x00"):
    """创建一个 NullStripped 描述符。

    null 剥离包装器。对应 Python construct 的 ``NullStripped``。

    使用方式::

        @dataclass
        class P(StructMixin):
            data: bytes = field(NullStripped(GreedyBytes))

        # 假设流为 b"hello\\x00\\x00"
        P.parse(b"hello\\x00\\x00")  # → P(data=b"hello")

    多字节 pad（utf16）::

        NullStripped(GreedyBytes, pad=b"\\x00\\x00")

    :param subcon: 内层子构造器。
    :param pad: pad 字节串。
    :return: ``NullStrippedDescriptor`` 实例。
    """
    return NullStrippedDescriptor(subcon, pad)


# ---------------------------------------------------------------------------
# Phase 6.3: 内置 Adapter 描述符（Subconstruct / Peek / RawCopy / Rebuild / Pass）
#
# 设计依据：``docs/design/模块设计/模块设计-Adapter核心.md`` §6.4。
#
# 这些描述符是纯 Python 类（不需要 Rust pyclass），通过 type name 识别。
# compile_schema 的 build_node_from_descriptor 通过 ``type(desc).__name__``
# 匹配到对应的 Node 变体（详见 compile.rs Phase 6.3 分支）。
#
# 双层分工（PM 决策 2）：
# - **内置 Adapter**（本节 5 个）：Rust Node 变体，性能 ≥10x（用户主动选具体名）
# - **用户面 Adapter**（_adapters.py）：Python 类，用户继承写 _decode/_encode
#   性能不设硬门禁（用户主动接受 Python 层解码开销）
# ---------------------------------------------------------------------------


class SubconstructDescriptor:
    """``Subconstruct(subcon)`` 描述符。

    单子构造器包装：parse/build/sizeof 全部转发给 subcon。对应 Python
    construct 的 ``Subconstruct``（core.py L787）。

    Python 中 Subconstruct 是抽象基类（Adapter/RawCopy/Peek/Rebuild 等的父类），
    construct-rs 把它实现为**具体节点**，主要供未来 Pointer/Prefixed 复用，
    以及作为其他内置 Adapter 的实现基础。

    ``_expr_params`` 协议返回空 dict：Subconstruct 自身无表达式参数
    （inner subcon 的表达式由递归处理）。
    """

    __slots__ = ("subcon",)

    _expr_params = {}

    def __init__(self, subcon):
        """初始化 Subconstruct 描述符。

        :param subcon: 被包装的子构造器。
        """
        self.subcon = subcon

    def __repr__(self):
        return "Subconstruct({!r})".format(self.subcon)


def Subconstruct(subcon):
    """创建一个 Subconstruct 描述符（纯转发包装）。

    使用方式::

        @dataclass
        class P(StructMixin):
            x: int = field(Subconstruct(Int8ub))

    :param subcon: 被包装的子构造器。
    :return: ``SubconstructDescriptor`` 实例。
    """
    return SubconstructDescriptor(subcon)


class PeekDescriptor:
    """``Peek(subcon)`` 描述符。

    预读节点：parse 子解析后回退 stream 到入口位置（不消费字节）。
    对应 Python construct 的 ``Peek``（core.py L4486）。

    - parse：返回 subcon.parse 结果（不消费字节，seek 回入口位置）；
      子解析失败时吞掉错误返回 None
    - build：no-op
    - sizeof：0

    ``_expr_params`` 协议返回空 dict。
    """

    __slots__ = ("subcon",)

    _expr_params = {}

    def __init__(self, subcon):
        """初始化 Peek 描述符。

        :param subcon: 被预读的子构造器。
        """
        self.subcon = subcon

    def __repr__(self):
        return "Peek({!r})".format(self.subcon)


def Peek(subcon):
    """创建一个 Peek 描述符。

    使用方式::

        @dataclass
        class P(StructMixin):
            a: int = field(Peek(Int8ub))      # 不消费字节
            b: int = field(Int8ub)            # 读同一字节

        P.parse(b"\\x10")  # → P(a=16, b=16)

    :param subcon: 被预读的子构造器。
    :return: ``PeekDescriptor`` 实例。
    """
    return PeekDescriptor(subcon)


class RawCopyDescriptor:
    """``RawCopy(subcon)`` 描述符。

    原始字节捕获节点：parse 返回 ``dict(data, value, offset1, offset2, length)``。
    对应 Python construct 的 ``RawCopy``（core.py L4761）。

    - parse：记录 offset1 → subcon.parse → offset2 → seek 回 offset1 重读 raw bytes
    - build：含 ``'data'`` 键直接 write；含 ``'value'`` 键调 subcon.build；否则错误
    - sizeof：subcon.sizeof

    ``_expr_params`` 协议返回空 dict。

    已知差异（设计 §5.3 RC-build-1）：construct-rs 的 build 不返回值，
    用户无法拿到 build 出的 raw bytes。需要 raw bytes 应走 parse 路径。
    """

    __slots__ = ("subcon",)

    _expr_params = {}

    def __init__(self, subcon):
        """初始化 RawCopy 描述符。

        :param subcon: 被捕获原始字节的子构造器。
        """
        self.subcon = subcon

    def __repr__(self):
        return "RawCopy({!r})".format(self.subcon)


def RawCopy(subcon):
    """创建一个 RawCopy 描述符。

    使用方式::

        @dataclass
        class P(StructMixin):
            x: dict = field(RawCopy(Int8ub))

        parsed = P.parse(b"\\xff")
        # parsed.x == {"data": b"\\xff", "value": 255, "offset1": 0,
        #              "offset2": 1, "length": 1}

    :param subcon: 被捕获原始字节的子构造器。
    :return: ``RawCopyDescriptor`` 实例。
    """
    return RawCopyDescriptor(subcon)


class RebuildDescriptor:
    """``Rebuild(subcon, func)`` 描述符。

    build 时基于表达式重算字段。对应 Python construct 的 ``Rebuild``（core.py L2975）。

    - parse：转发 subcon.parse
    - build：忽略传入 obj，求值 func 表达式 → subcon.build(value)
    - sizeof：subcon.sizeof

    必须作为 RO 字段使用（``rfield(Rebuild(...))``）。若用 ``field(...)`` (rw)
    或 ``wfield(...)`` (wo)，编译期拒绝。

    ``_expr_params`` 协议返回 ``{"func": <expr>}``：编译期将 expr 翻译为 ExprOp
    指令列表，存入 expr_programs 的 "func" 键（与 ComputedDescriptor 同模式）。

    限制（设计 §5.3 RB-callable）：func 必须是 Phase 2 表达式
    （FieldRef/ExprRef/int 组合），**不接收 Python callable/lambda**
    （与 ADR-014 RepeatUntil v5 同脉络）。

    ``_field_kind`` 属性返回 "ro"，由 _mixin._apply_dataclass_field_config 识别，
    决定 dataclass 字段配置（init=False，parse 时由 force_setattr 覆盖）。
    """

    __slots__ = ("subcon", "func")

    def __init__(self, subcon, func):
        """初始化 Rebuild 描述符。

        :param subcon: 被包装的子构造器（用于 parse/sizeof，以及 build 的最终写入）。
        :param func: build 时求值的表达式（FieldRef/ExprRef/int 组合，**不接受 callable**）。
        """
        self.subcon = subcon
        self.func = func

    @property
    def _expr_params(self):
        """表达式参数协议。

        返回 ``{"func": self.func}``。当 func 是 FieldRef/ExprRef 时编译为 ExprOp；
        int 常量也走表达式路径（ExprProgram 包装 Const）。
        """
        return {"func": self.func}

    @property
    def _field_kind(self):
        """字段种类协议（ADR-004）。

        返回 "ro"：Rebuild 必须作为 RO 字段使用（值不来自用户输入）。
        与 Computed/Tell 同类。
        """
        return "ro"

    def __repr__(self):
        return "Rebuild({!r}, {!r})".format(self.subcon, self.func)


def Rebuild(subcon, func):
    """创建一个 Rebuild 描述符。

    使用方式（必须作为 RO 字段，用 ``rfield`` 包装；func 是 Phase 2 表达式）::

        @dataclass
        class P(StructMixin):
            x: int = field(Int8ub)
            y: int = rfield(Rebuild(Int8ub, x + 1))   # build 时 y = x + 1

        P.parse(b"\\x03\\x04")  # → P(x=3, y=4)        # parse 转发 subcon
        P(x=5).build()          # → b"\\x05\\x06"      # build 时 y 由 x+1 计算

    Python construct 原版写法（``this`` 语法，construct-rs 不支持）::

        # 原版：Rebuild(Byte, this.x + 1)  ← this.x 引用 context
        # construct-rs：Rebuild(Int8ub, x + 1)  ← 直接用字段名 x

    限制：

    - **不接收 Python callable / lambda**（同 RepeatUntil v5）。如传入 callable，
      编译期无法转换为 ExprOp，会报 CompilationError。
    - 必须作为 RO 字段使用（``rfield(Rebuild(...))``）。

    :param subcon: 被包装的子构造器。
    :param func: build 时求值的表达式（FieldRef/ExprRef/int 组合）。
    :return: ``RebuildDescriptor`` 实例。
    """
    return RebuildDescriptor(subcon, func)


class PassDescriptor:
    """``Pass`` 描述符（singleton）。

    No-op 节点：parse 返回 None；build 不写字节；sizeof=0。
    对应 Python construct 的 ``Pass``（core.py L4687）。

    主要用于 Phase 7 ``If``/``Switch`` 的默认值（如 ``If(cond, then)`` 等价于
    ``IfThenElse(cond, then, Pass)``）。

    ``_expr_params`` 协议返回空 dict。
    """

    __slots__ = ()

    _expr_params = {}

    def __repr__(self):
        return "Pass()"


# Pass singleton（与 GreedyBytes 单例同模式）。
# Pass 无参数，全局共享一个实例即可。
Pass = PassDescriptor()


# ---------------------------------------------------------------------------
# Phase 7.2: Streams 描述符（Seek / Pointer / Prefixed）
#
# 设计依据：``docs/design/模块设计/模块设计-Streams.md`` §3.6。
#
# 这些描述符是纯 Python 类（不需要 Rust pyclass），通过 type name 识别。
# compile.rs 的 build_node_from_descriptor 通过 ``type(desc).__name__``
# 匹配到 "SeekDescriptor" / "PointerDescriptor" / "PrefixedDescriptor" 字符串，
# 构建对应的 Node 变体（详见 compile.rs Phase 7.2 分支）。
#
# - SeekDescriptor：at（int 或 FieldRef/ExprRef）+ whence（int 0/1/2）
# - PointerDescriptor：offset（int 或表达式）+ subcon + relativeOffset（bool）+ stream（None）
# - PrefixedDescriptor：lengthfield + subcon + includelength（bool）
# ---------------------------------------------------------------------------


class SeekDescriptor:
    """``Seek(at, whence=0)`` 描述符（Phase 7.2）。

    流定位节点：parse/build 执行 ``stream.seek(at, whence)``。
    对应 Python construct 的 ``Seek``（core.py L4594）。

    parse 返回新位置（PyLong）；build 接受任意输入（``flagbuildnone=True`` 兼容）；
    sizeof 永远返回 ``SizeofError``。

    ``_expr_params`` 协议：当 ``at`` 是 int 时返回 ``{}``（跳过编译）；
    当 at 是 FieldRef/ExprRef 时返回 ``{"at": self.at}``（编译为 ExprOp 列表）。

    :param at: 定位偏移（int 常量或 FieldRef/ExprRef 表达式）。可为负（whence=2 时）。
    :param whence: 定位参考点（0=Start / 1=Current / 2=End，默认 0）。
    """

    __slots__ = ("at", "whence")

    def __init__(self, at, whence=0):
        """初始化 Seek 描述符。

        :param at: 定位偏移。
        :param whence: 0/1/2（默认 0）。
        """
        self.at = at
        self.whence = whence

    @property
    def _expr_params(self):
        """表达式参数协议（与 BytesDescriptor._expr_params 同模式）。

        返回 ``{"at": self.at}``。当 at 是 int 时跳过编译；
        当 at 是 FieldRef/ExprRef 时编译为 ExprOp 列表。
        """
        if isinstance(self.at, int) and not isinstance(self.at, bool):
            return {}
        return {"at": self.at}

    def __repr__(self):
        return "Seek(at={!r}, whence={!r})".format(self.at, self.whence)


def Seek(at, whence=0):
    """创建一个 Seek 描述符。

    流定位节点。对应 Python construct 的 ``Seek``。

    使用方式（在 Sequence 内）::

        from construct import Sequence, Seek, Bytes
        d = Sequence(Bytes(10), Seek(5), Bytes(1))
        # parse 跳到 pos=5，再读 1 字节

    使用方式（whence=2，从 EOF 向前）::

        d = Sequence(Seek(-2, 2), Bytes(1))
        # 从末尾退 2 字节再读

    使用方式（表达式 at，``offset`` 必须是同 Struct 内已声明的 int 字段）::

        @dataclass
        class P(StructMixin):
            offset: int = field(Int8ub)
            pos: int = field(Seek(offset))
            tail: bytes = field(Bytes(1))

    Python construct 原版写法（``this`` 语法，construct-rs 不支持）::

        # 原版：d = Sequence(Seek(this.offset), Bytes(1))  ← this.offset 引用 context
        # construct-rs：在 StructMixin 内用 ``field(Seek(offset))`` 引用字段名

    :param at: 定位偏移（int 或 FieldRef/ExprRef 表达式）。
    :param whence: 0=Start / 1=Current / 2=End（默认 0）。
    :return: ``SeekDescriptor`` 实例。
    """
    return SeekDescriptor(at, whence)


class PointerDescriptor:
    """``Pointer(offset, subcon, stream=None, relativeOffset=False)`` 描述符（Phase 7.2）。

    绝对偏移读写节点：seek 到 ``offset`` 处理 ``subcon``，再 seek 回原位置
    （不占主流位置）。对应 Python construct 的 ``Pointer``（core.py L4384）。

    whence 自动计算（对齐 Python ``_pointer_seek``，core.py L4421-4424）：

    - ``relativeOffset=True`` → whence=Current（相对定位）
    - ``relativeOffset=False`` + ``offset >= 0`` → whence=Start（绝对定位）
    - ``relativeOffset=False`` + ``offset < 0`` → whence=End（从 EOF 向前）

    ``_expr_params`` 协议：当 ``offset`` 是 int 时返回 ``{}``；当 offset 是
    FieldRef/ExprRef 时返回 ``{"offset": self.offset}``（编译为 ExprOp 列表）。

    :param offset: 偏移（int 常量或 FieldRef/ExprRef 表达式，可为负）。
    :param subcon: 被偏移处理的子构造器（描述符）。
    :param stream: 换流参数。**不支持**（非 None 编译期拒绝，已知限制）。
    :param relativeOffset: 若 True，offset 解释为相对当前位置；否则绝对（默认 False）。
    """

    __slots__ = ("offset", "subcon", "stream", "relativeOffset")

    def __init__(self, offset, subcon, stream=None, relativeOffset=False):
        """初始化 Pointer 描述符。

        :param offset: 偏移。
        :param subcon: 子构造器。
        :param stream: 换流（不支持，必须 None）。
        :param relativeOffset: 是否相对当前位置。
        """
        self.offset = offset
        self.subcon = subcon
        self.stream = stream
        self.relativeOffset = relativeOffset

    @property
    def _expr_params(self):
        """表达式参数协议（与 BytesDescriptor._expr_params 同模式）。

        返回 ``{"offset": self.offset}``。当 offset 是 int 时跳过编译；
        当 offset 是 FieldRef/ExprRef 时编译为 ExprOp 列表。
        """
        if isinstance(self.offset, int) and not isinstance(self.offset, bool):
            return {}
        return {"offset": self.offset}

    def __repr__(self):
        return "Pointer(offset={!r}, subcon={!r}, relativeOffset={!r})".format(
            self.offset, self.subcon, self.relativeOffset
        )


def Pointer(offset, subcon, stream=None, relativeOffset=False):
    """创建一个 Pointer 描述符。

    绝对偏移读写节点。对应 Python construct 的 ``Pointer``。

    使用方式（正 offset 绝对定位）::

        from construct import Pointer, Bytes
        d = Pointer(8, Bytes(1))
        d.parse(b"abcdefghijkl")  # → b"i"（位置 8 的字节）
        d.build(b"Z")             # → b'\\x00'*8 + b'Z'（零填充到位置 8）

    使用方式（负 offset 从 EOF）::

        d = Pointer(-2, Bytes(1))
        d.parse(b"abcdefgh")      # → b"g"（倒数第 2 字节）

    使用方式（relativeOffset=True）::

        from construct import Sequence
        ds = Sequence(Bytes(3), Pointer(2, Bytes(1), relativeOffset=True))
        # Bytes(3) 后 tell=3，Pointer 相对 +2 → pos=5

    使用方式（Struct 内 Pointer 不占主流位置）::

        from construct import Struct
        d = Struct("ptr"/Pointer(5, Bytes(1)), "direct"/Bytes(1))
        result = d.parse(b"01234Xy")
        # result.ptr == b"X"（位置 5）
        # result.direct == b"0"（Pointer 已 seek 回 0）

    限制：

    - **stream 参数不支持**（换流）。非 None 编译期拒绝。
    - **在 Bitwise 域内行为未定义**（bit API 不感知 pos）。

    :param offset: 偏移（int 或 FieldRef/ExprRef 表达式）。
    :param subcon: 子构造器。
    :param stream: 换流（不支持，必须 None）。
    :param relativeOffset: 是否相对当前位置（默认 False）。
    :return: ``PointerDescriptor`` 实例。
    """
    return PointerDescriptor(offset, subcon, stream, relativeOffset)


class PrefixedDescriptor:
    """``Prefixed(lengthfield, subcon, includelength=False)`` 描述符（Phase 7.2）。

    长度前缀子流节点：``lengthfield`` 给出字节数，``subcon`` 在该子流上处理。
    对应 Python construct 的 ``Prefixed``（core.py L4862）。

    parse 行为：lengthfield.parse → 得 length → ``stream.read(length)`` 取子切片 →
    ``subcon.parse(子流)``。
    build 行为：创建 temp BuildStream → subcon.build → ``lengthfield.build(len)`` →
    写 data 到主流。

    ``_expr_params`` 协议返回空 dict：Prefixed 自身无表达式参数
    （lengthfield 与 subcon 由递归处理，与 PrefixedArrayDescriptor 同限制：
    两者均不支持含表达式的子描述符）。

    与 ``PrefixedArray`` 的区别：PrefixedArray 的 lengthfield 是**元素计数**，
    parse 循环 N 次 inner.parse；Prefixed 的 lengthfield 是**字节计数**，
    parse 读 N 字节作为子流。

    :param lengthfield: 长度字段（描述符），常见 ``VarInt`` / ``Int16ub`` / ``Byte``。
    :param subcon: 子构造器（在子流上 parse/build）。
    :param includelength: 若 True，length 含 lengthfield 自身大小（默认 False）。
    """

    __slots__ = ("lengthfield", "subcon", "includelength")

    def __init__(self, lengthfield, subcon, includelength=False):
        """初始化 Prefixed 描述符。

        :param lengthfield: 长度字段。
        :param subcon: 子构造器。
        :param includelength: 是否包含 lengthfield 自身大小。
        """
        self.lengthfield = lengthfield
        self.subcon = subcon
        self.includelength = includelength

    # 类级别常量：Prefixed 无表达式参数（lengthfield/subcon 由递归处理）。
    _expr_params = {}

    def __repr__(self):
        return "Prefixed(lengthfield={!r}, subcon={!r}, includelength={!r})".format(
            self.lengthfield, self.subcon, self.includelength
        )


def Prefixed(lengthfield, subcon, includelength=False):
    """创建一个 Prefixed 描述符。

    长度前缀子流节点。对应 Python construct 的 ``Prefixed``。

    使用方式（基本）::

        from construct import Prefixed, Byte, Bytes
        d = Prefixed(Byte, Bytes(3))
        d.parse(b"\\x03abc")    # → b"abc"
        d.build(b"abc")         # → b"\\x03abc"

    使用方式（includelength=True）::

        from construct import Prefixed, Int16ub, GreedyBytes
        d = Prefixed(Int16ub, GreedyBytes, includelength=True)
        d.parse(b"\\x00\\x05abc")   # lengthfield=5，减 Int16ub sizeof=2 → 子流 3 字节

    使用方式（VarInt 前缀）::

        from construct import Prefixed, VarInt, GreedyRange, Int32ul
        d = Prefixed(VarInt, GreedyRange(Int32ul))

    与 ``PrefixedArray`` 的区别：

    - ``PrefixedArray(Byte, Item)``：Byte 是**元素计数**，循环 N 次 Item.parse
    - ``Prefixed(Byte, Item)``：Byte 是**字节计数**，读 N 字节作为子流

    限制：

    - sizeof = lengthfield.sizeof + subcon.sizeof（subcon 必须 static size）
    - lengthfield 与 subcon 均不支持含表达式的子描述符（与 ``Array`` 同限制）

    :param lengthfield: 长度字段（描述符）。
    :param subcon: 子构造器。
    :param includelength: 是否包含 lengthfield 自身大小（默认 False）。
    :return: ``PrefixedDescriptor`` 实例。
    """
    return PrefixedDescriptor(lengthfield, subcon, includelength)


# ---------------------------------------------------------------------------
# Phase 8 P0: Const / Default / Check 描述符
#
# 设计依据：``docs/design/模块设计/模块设计-Phase8-P0.md`` §1。
#
# 这些描述符是纯 Python 类（不需要 Rust pyclass），通过 type name 识别。
# compile.rs 的 build_node_from_descriptor 通过 ``type(desc).__name__``
# 匹配到 "ConstDescriptor" / "DefaultDescriptor" / "CheckDescriptor" 字符串，
# 构建对应的 Node 变体（详见 compile.rs Phase 8 P0 分支）。
#
# - ConstDescriptor：value（任意 Python 对象，常为 int/bytes/str）+ subcon
#   subcon 缺省时若 value 是 bytes，自动推断为 Bytes(len(value))（CN-7）。
# - DefaultDescriptor：value（FieldRef/ExprRef/int）+ subcon
#   value 编译为 ExprProgram（与 Rebuild func 同模式）。
# - CheckDescriptor：func（FieldRef/ExprRef/int，编译为 ExprProgram）
#   必须作为 RO 字段（_field_kind = "ro"）。
# ---------------------------------------------------------------------------


class ConstDescriptor:
    """``Const(value, subcon=None)`` 描述符。

    常量字段：parse 校验子解析结果 == value；build 用 value（忽略 obj）。

    对应 Python construct 的 ``Const``（core.py L2808）。Python 中
    ``Const(b"IHDR")`` 会自动用 ``Bytes(4)`` 作为 subcon（针对 bytes value）。

    - parse：inner.parse → ``obj == value`` 检查 → 不等抛 ``ConstError``
    - build：obj is None 或 obj == value → inner.build(value)；否则 ``ConstError``
    - sizeof：转发 inner.sizeof

    ``_expr_params`` 协议返回空 dict：Const 自身无表达式参数
    （value 是常量对象，subcon 由递归处理）。

    :param value: 期望值（int / bytes / str 等任意 Python 对象）。
    :param subcon: 子构造器（描述符）。None 时根据 value 类型推断。
    """

    __slots__ = ("value", "subcon")

    _expr_params = {}

    def __init__(self, value, subcon=None):
        """初始化 Const 描述符。

        :param value: 期望值。
        :param subcon: 子构造器。None 时根据 value 推断（bytes → Bytes(len)）。
        """
        if subcon is None:
            # CN-7: value 是 bytes 但 subcon 缺省 → Bytes(len(value))
            if isinstance(value, (bytes, bytearray)):
                subcon = Bytes(len(value))
            else:
                # CN-8: value 非 bytes 且 subcon 缺省 → 编译期错误
                raise CompilationError(
                    "Const requires explicit subcon when value is not bytes, "
                    "e.g. Const(255, Int32ub)"
                )
        self.value = value
        self.subcon = subcon

    def __repr__(self):
        return "Const({!r}, subcon={!r})".format(self.value, self.subcon)


def Const(value, subcon=None):
    """创建一个 Const 描述符。

    常量字段。对应 Python construct 的 ``Const``（core.py L2808）。

    使用方式（bytes value，subcon 自动推断）::

        from construct import Const

        # 等价于 Const(b"IHDR", Bytes(4))
        c = Const(b"IHDR")
        c.parse(b"IHDR")   # → b"IHDR"
        c.parse(b"JPEG")   # → ConstError

    使用方式（int value，显式 subcon）::

        from construct import Const, Int32ub

        c = Const(255, Int32ub)
        c.build(None)      # → b'\\xff\\x00\\x00\\x00'
        c.build(255)       # → b'\\xff\\x00\\x00\\x00'
        c.build(256)       # → ConstError

    使用方式（Struct 内 magic header）::

        @dataclass
        class PNG(StructMixin):
            magic: bytes = field(Const(b"\\x89PNG\\r\\n\\x1a\\n"))

    :param value: 期望值（任意 Python 对象）。
    :param subcon: 子构造器（None 时根据 value 类型推断）。
    :return: ``ConstDescriptor`` 实例。
    """
    return ConstDescriptor(value, subcon=subcon)


class DefaultDescriptor:
    """``Default(subcon, value)`` 描述符。

    默认值字段：build 时 obj 为 None 则用 value 表达式求值；parse 转发 inner。

    对应 Python construct 的 ``Default``（core.py L3030）。

    - parse：转发 inner.parse
    - build：obj is None → 求值 value ExprProgram → inner.build；否则 inner.build(obj)
    - sizeof：转发 inner.sizeof

    ``_expr_params`` 协议返回 ``{"value": self.value}``：编译期将 value 翻译为
    ExprOp 列表（int 常量也包装为单条 Const，与 Rebuild func 同模式）。

    限制（设计 §1 DF-4）：value 必须是 Phase 2 表达式
    （FieldRef/ExprRef/int 组合），**不接收 Python callable/lambda**
    （与 Rebuild 同脉络，ADR-014 硬约束）。

    :param subcon: 子构造器（描述符）。
    :param value: 默认值（int 常量或 FieldRef/ExprRef 表达式，**不接受 callable**）。
    """

    __slots__ = ("subcon", "value")

    def __init__(self, subcon, value):
        """初始化 Default 描述符。

        :param subcon: 子构造器。
        :param value: 默认值（int 或 FieldRef/ExprRef）。
        """
        self.subcon = subcon
        self.value = value

    @property
    def _expr_params(self):
        """表达式参数协议（与 RebuildDescriptor._expr_params 同模式）。

        返回 ``{"value": self.value}``。int 常量也走表达式路径（包装为 Const）。
        """
        return {"value": self.value}

    def __repr__(self):
        return "Default({!r}, {!r})".format(self.subcon, self.value)


def Default(subcon, value):
    """创建一个 Default 描述符。

    默认值字段。对应 Python construct 的 ``Default``（core.py L3030）。

    使用方式（常量默认值）::

        from construct import Default, Byte

        d = Default(Byte, 0)
        d.build(None)    # → b'\\x00'（用默认值 0）
        d.build(5)       # → b'\\x05'（用 obj）

    使用方式（字段引用表达式）::

        @dataclass
        class P(StructMixin):
            count: int = field(Byte)
            padded: int = field(Default(Byte, count))   # padded 默认 = count

        P(count=5).build()         # → b'\\x05\\x05'（padded 用 count）
        P(count=5, padded=9).build()  # → b'\\x05\\x09'（padded 用 obj）

    Python construct 原版写法（``this`` 语法，construct-rs 不支持）::

        # 原版：Default(Byte, this.count)
        # construct-rs：Default(Byte, count)

    限制：

    - **不接收 Python callable / lambda**（同 Rebuild）。如传入 callable，
      编译期无法转换为 ExprOp，会报 CompilationError。

    :param subcon: 子构造器。
    :param value: 默认值（int 或 FieldRef/ExprRef 表达式）。
    :return: ``DefaultDescriptor`` 实例。
    """
    return DefaultDescriptor(subcon, value)


class CheckDescriptor:
    """``Check(func)`` 描述符。

    断言检查：parse/build 求值表达式，非真抛 ``CheckError``。sizeof 恒为 0。

    对应 Python construct 的 ``Check``（core.py L3081）。

    **必须作为 RO 字段使用**（``rfield(Check(...))``，与 Computed 同类）。

    ``_expr_params`` 协议返回 ``{"func": self.func}``：编译期将 func 翻译为 ExprOp 列表
    （与 ComputedDescriptor / RebuildDescriptor 同模式）。

    ``_field_kind`` 属性返回 "ro"，由 _mixin._apply_dataclass_field_config 识别，
    决定 dataclass 字段配置（init=False）。

    限制（设计 §1 CK-5）：func 必须是 Phase 2 表达式
    （FieldRef/ExprRef/int 组合），**不接收 Python callable/lambda**
    （与 Rebuild 同脉络，ADR-014 硬约束）。

    :param func: 断言表达式（int 或 FieldRef/ExprRef 组合，**不接受 callable**）。
    """

    __slots__ = ("func",)

    def __init__(self, func):
        """初始化 Check 描述符。

        :param func: 断言表达式（int 或 FieldRef/ExprRef）。
        """
        self.func = func

    @property
    def _expr_params(self):
        """表达式参数协议（与 ComputedDescriptor._expr_params 同模式）。"""
        return {"func": self.func}

    @property
    def _field_kind(self):
        """字段种类协议（ADR-004）。

        返回 "ro"：Check 必须作为 RO 字段使用（值不来自用户输入）。
        与 Computed / Tell / Rebuild 同类。
        """
        return "ro"

    def __repr__(self):
        return "Check({!r})".format(self.func)


def Check(func):
    """创建一个 Check 描述符。

    断言检查。对应 Python construct 的 ``Check``（core.py L3081）。

    使用方式（必须作为 RO 字段，用 ``rfield`` 包装；func 是 Phase 2 表达式）::

        @dataclass
        class P(StructMixin):
            width: int = field(Byte)
            height: int = field(Byte)
            _check: None = rfield(Check(width * height > 0))

        P.parse(b"\\x02\\x03")   # → P(width=2, height=3)
        # Check(width == 0) 表达式不满足 → CheckError

    Python construct 原版写法（``this`` 语法，construct-rs 不支持）::

        # 原版：Check(this.width * this.height > 0)
        # construct-rs：Check(width * height > 0)

    限制：

    - **不接收 Python callable / lambda**（同 Computed）。如传入 callable，
      编译期无法转换为 ExprOp，会报 CompilationError。
    - 必须作为 RO 字段使用（``rfield(Check(...))``）。

    :param func: 断言表达式（int 或 FieldRef/ExprRef 组合）。
    :return: ``CheckDescriptor`` 实例。
    """
    return CheckDescriptor(func)


# ---------------------------------------------------------------------------
# Phase 8 P0: Terminated / Probe 描述符
#
# 设计依据：``docs/design/模块设计/模块设计-Phase8-P0.md`` §5。
#
# - TerminatedDescriptor：singleton，无参数。parse 时校验 stream EOF。
# - ProbeDescriptor：into（None 或字段名 str）+ lookahead（None 或 int）。
#   [设计质疑] into 用 FieldName 而非 ExprProgram（详见 probe.rs 模块级注释）。
# ---------------------------------------------------------------------------


class TerminatedDescriptor:
    """``Terminated`` 描述符（singleton）。

    EOF 断言：parse 时若 stream 未到 EOF 抛 ``TerminatedError``。
    对应 Python construct 的 ``Terminated``（core.py L4727）。

    主要用于 Struct 末尾校验已消费所有字节（防止解析漏字节）。

    ``_expr_params`` 协议返回空 dict。
    """

    __slots__ = ()

    _expr_params = {}

    def __repr__(self):
        return "Terminated()"


# Terminated singleton（与 GreedyBytes / Pass 同模式）。
Terminated = TerminatedDescriptor()


class ProbeDescriptor:
    """``Probe(into=None, lookahead=None)`` 描述符。

    调试探针：parse/build/sizeof 时打印 path + 可选 stream peek + 可选 context dump。
    对应 Python construct 的 ``Probe``（debug.py L6）。

    ``_expr_params`` 协议返回空 dict：Probe 无表达式参数
    （into 是字段名引用而非 FieldRef/ExprRef 表达式，[设计质疑] 详见 probe.rs）。

    :param into: 可选字段名（任意类型字段），None 表示打印整个 context。
    :param lookahead: 可选 peek 字节数（int），None 表示不 peek。
    """

    __slots__ = ("into", "lookahead")

    _expr_params = {}

    def __init__(self, into=None, lookahead=None):
        """初始化 Probe 描述符。

        :param into: 可选字段名（str）。
        :param lookahead: 可选 peek 字节数（int）。
        """
        self.into = into
        self.lookahead = lookahead

    def __repr__(self):
        return "Probe(into={!r}, lookahead={!r})".format(self.into, self.lookahead)


def Probe(into=None, lookahead=None):
    """创建一个 Probe 描述符。

    调试探针。对应 Python construct 的 ``Probe``（debug.py L6）。

    使用方式（无参数，打印整个 context）::

        @dataclass
        class P(StructMixin):
            a: int = field(Byte)
            _probe: None = rfield(Probe())     # parse 时打印 {a: ...}
            b: int = field(Byte)

    使用方式（指定字段名，打印该字段值）::

        Probe(into="a")    # parse 时打印 P.a 的值

    使用方式（peek stream）::

        Probe(lookahead=4)    # parse 时额外 hex 打印下 4 字节

    [设计质疑] 与 Python construct 的差异：

    - Python ``Probe(lambda ctx: expr)`` 支持任意 callable。construct-rs 仅支持
      字段名（``Probe(into="field_name")``），不支持 callable（与 ADR-014 一致）。
    - 用户需要打印复杂表达式结果时，应先用 ``Computed(expr)`` 字段，再 ``Probe(into=...)``。

    :param into: 可选字段名（str），None 表示打印整个 context。
    :param lookahead: 可选 peek 字节数（int），None 表示不 peek。
    :return: ``ProbeDescriptor`` 实例。
    """
    return ProbeDescriptor(into=into, lookahead=lookahead)


# ---------------------------------------------------------------------------
# Phase 8 P0: Aligned 描述符
#
# 设计依据：``docs/design/模块设计/模块设计-Phase8-P0.md`` §4。
#
# AlignedDescriptor：modulus（int 或 FieldRef/ExprRef）+ subcon + pattern（1-byte bytes）。
# modulus 表达式路径走 ExprProgram（与 SeekDescriptor at 同模式）。
# pattern 编译期校验 len == 1（AL-11/AL-12）。
# ---------------------------------------------------------------------------


class AlignedDescriptor:
    """``Aligned(modulus, subcon, pattern=b"\\x00")`` 描述符。

    对齐包装节点：inner 解析/构建后，填充字节到 modulus 的整数倍。
    对应 Python construct 的 ``Aligned``（core.py L4261）。

    - parse：inner.parse 后，``pad = -(tell_after - tell_before) % modulus``，
    - build：inner.build 后写 ``pattern * pad`` 字节
    - sizeof：modulus 表达式时返回 Err（D-P0-3，对齐 Python SizeofError）

    ``_expr_params`` 协议：当 ``modulus`` 是 int 时返回 ``{}``（跳过编译）；
    当 modulus 是 FieldRef/ExprRef 时返回 ``{"modulus": self.modulus}``。

    :param modulus: 对齐模数（int >= 2 或 FieldRef/ExprRef 表达式）。
    :param subcon: 子构造器。
    :param pattern: 填充字节模式（1-byte bytes，默认 ``b"\\x00"``）。
    """

    __slots__ = ("modulus", "subcon", "pattern")

    def __init__(self, modulus, subcon, pattern=b"\x00"):
        """初始化 Aligned 描述符。

        :param modulus: 对齐模数。
        :param subcon: 子构造器。
        :param pattern: 1-byte bytes（默认 b"\\x00"）。
        """
        self.modulus = modulus
        self.subcon = subcon
        self.pattern = pattern

    @property
    def _expr_params(self):
        """表达式参数协议（与 BytesDescriptor._expr_params 同模式）。

        返回 ``{"modulus": self.modulus}``。当 modulus 是 int 时跳过编译；
        当 modulus 是 FieldRef/ExprRef 时编译为 ExprOp 列表。
        """
        if isinstance(self.modulus, int) and not isinstance(self.modulus, bool):
            return {}
        return {"modulus": self.modulus}

    def __repr__(self):
        return "Aligned(modulus={!r}, subcon={!r}, pattern={!r})".format(
            self.modulus, self.subcon, self.pattern
        )


def Aligned(modulus, subcon, pattern=b"\x00"):
    """创建一个 Aligned 描述符。

    对齐包装节点。对应 Python construct 的 ``Aligned``（core.py L4261）。

    使用方式（基本）::

        from construct import Aligned, Int16ub

        d = Aligned(4, Int16ub)
        d.parse(b"\\x00\\x01\\x00\\x00")   # → 1（消费 4 字节，2 数据 + 2 填充）
        d.build(1)                          # → b"\\x00\\x01\\x00\\x00"

    使用方式（pattern）::

        Aligned(4, Int16ub, pattern=b"\\xff").build(1)  # → b"\\x00\\x01\\xff\\xff"

    使用方式（字段引用 modulus）::

        @dataclass
        class P(StructMixin):
            width: int = field(Byte)
            data: int = field(Aligned(width, Byte))   # modulus = width 字段值

    Python construct 原版写法（``this`` 语法，construct-rs 不支持）::

        # 原版：Aligned(this.width, Byte)
        # construct-rs：Aligned(width, Byte)

    限制：

    - **modulus 必须 >= 2**（AL-6/AL-7/AL-8）。编译期或运行期 PaddingError。
    - **pattern 必须是 1-byte bytes**（AL-11/AL-12）。编译期 PaddingError。
    - **sizeof 不支持 modulus 表达式**（D-P0-3，对齐 Python SizeofError）。

    :param modulus: 对齐模数（int >= 2 或 FieldRef/ExprRef 表达式）。
    :param subcon: 子构造器。
    :param pattern: 填充字节模式（1-byte bytes，默认 ``b"\\x00"``）。
    :return: ``AlignedDescriptor`` 实例。
    """
    return AlignedDescriptor(modulus, subcon, pattern=pattern)


# ---------------------------------------------------------------------------
# Phase 8 P0: Hex / HexDump 描述符
#
# 设计依据：``docs/design/模块设计/模块设计-Phase8-P0.md`` §2。
#
# Hex/HexDump 是 construct 核心库内置 Adapter（非用户自定义），在 construct-rs 中
# 实现为 Rust Node（非 AdapterCallbackNode，§0.2 判据 2，详见设计 §2.2 + ADR-022）。
# ---------------------------------------------------------------------------


class HexDescriptor:
    """``Hex(subcon)`` 描述符。

    Hex 显示包装节点：根据 inner.parse 结果类型分派到对应 Python 显示类。

    对应 Python construct 的 ``Hex``（core.py L3523）。

    - parse：
        - int → ``HexDisplayedInteger.new(obj, fmtstr)``
        - bytes → ``HexDisplayedBytes(obj)``
        - dict → ``HexDisplayedDict(obj)``
        - else → 透传
    - build：透传 inner.build
    - sizeof：转发 inner.sizeof

    ``_expr_params`` 协议返回空 dict（Hex 自身无表达式参数；subcon 由递归处理）。

    :param subcon: 子构造器（描述符）。
    """

    __slots__ = ("subcon",)

    _expr_params = {}

    def __init__(self, subcon):
        """初始化 Hex 描述符。

        :param subcon: 子构造器。
        """
        self.subcon = subcon

    def __repr__(self):
        return "Hex({!r})".format(self.subcon)


def Hex(subcon):
    """创建一个 Hex 描述符。

    Hex 显示包装节点。对应 Python construct 的 ``Hex``（core.py L3523）。

    使用方式（int 字段）::

        from construct import Hex, Int32ub

        d = Hex(Int32ub)
        result = d.parse(b"\\x00\\x00\\x01\\x02")   # → HexDisplayedInteger(258)
        print(result)                                # 输出 "0x00000102"
        int(result)                                  # → 258（int 子类）

    使用方式（bytes 字段）::

        from construct import Hex, Bytes

        d = Hex(Bytes(4))
        result = d.parse(b"\\x00\\x00\\x01\\x02")
        print(result)   # 输出 "unhexlify('00000102')"

    使用方式（Struct 内）::

        @dataclass
        class P(StructMixin):
            magic: int = field(Hex(Int32ub))

    :param subcon: 子构造器。
    :return: ``HexDescriptor`` 实例。
    """
    return HexDescriptor(subcon)


class HexDumpDescriptor:
    """``HexDump(subcon)`` 描述符。

    HexDump 显示包装节点：与 ``Hex`` 同模式，仅显示类不同（仅 bytes/dict 两种）。
    int 类型不包装（透传 PyLong）。

    对应 Python construct 的 ``HexDump``（core.py L3583）。

    - parse：
        - bytes → ``HexDumpDisplayedBytes(obj)``
        - dict → ``HexDumpDisplayedDict(obj)``
        - else → 透传
    - build：透传 inner.build
    - sizeof：转发 inner.sizeof

    ``_expr_params`` 协议返回空 dict。

    :param subcon: 子构造器。
    """

    __slots__ = ("subcon",)

    _expr_params = {}

    def __init__(self, subcon):
        """初始化 HexDump 描述符。

        :param subcon: 子构造器。
        """
        self.subcon = subcon

    def __repr__(self):
        return "HexDump({!r})".format(self.subcon)


def HexDump(subcon):
    """创建一个 HexDump 描述符。

    HexDump 显示包装节点。对应 Python construct 的 ``HexDump``（core.py L3583）。

    使用方式（bytes 字段）::

        from construct import HexDump, Bytes

        d = HexDump(Bytes(16))
        result = d.parse(b"\\x00" * 16)
        print(result)   # 输出 hexdump 格式（hexundump('''...''')）

    使用方式（Struct 内）::

        @dataclass
        class P(StructMixin):
            payload: bytes = field(HexDump(Bytes(16)))

    :param subcon: 子构造器。
    :return: ``HexDumpDescriptor`` 实例。
    """
    return HexDumpDescriptor(subcon)


# ---------------------------------------------------------------------------
# Phase 8 P0: Checksum 描述符
#
# 设计依据：``docs/design/模块设计/模块设计-Phase8-P0.md`` §3。
#
# ChecksumDescriptor 双轨方案（PM 决策 D-1 接受）：
#
# - hashfunc：``HashAlgo`` enum（B1/B2 零拷贝）或 Python callable（A1/A2 兼容）
# - bytes_source：start/end 表达式（StreamRange）或 bytesfunc_name 字段引用（ContextBytes）
#
# compile.rs 通过 ``type(hashfunc).__name__ == "HashAlgo"`` 识别 enum 类型，
# 按 ``.name`` 取算法名编译为 ``BuiltinHash`` 变体。
# ---------------------------------------------------------------------------


class ChecksumDescriptor:
    """``Checksum(checksumfield, hashfunc, bytesfunc=None, start=None, end=None)`` 描述符。

    校验和节点：parse 校验 hash，build 计算 hash。
    对应 Python construct 的 ``Checksum``（core.py L5532）。

    construct-rs 扩展双轨方案（设计 §3.5）：

    - **路径 B1**（推荐零拷贝）：``HashAlgo`` enum + ``start/end`` 表达式
    - **路径 A2**（Python 原版兼容）：Python callable + ``bytesfunc`` 字段引用

    parse 行为：

    1. ``checksumfield.parse(stream)`` 得到 ``hash1``
    2. 根据 bytes_source 取数据：
       - StreamRange：``stream.slice(start, end)`` 零拷贝切片
       - ContextBytes：``context[bytesfunc]`` 取 bytes 字段
    3. 根据 hashfunc 计算：
       - BuiltinHash：Rust crate 计算（零拷贝）
       - Python callable：FFI 回调计算
    4. ``hash1 == hash2``？等则返回 ``hash1``，不等抛 ``ChecksumError``

    build 行为：计算 ``hash2`` → ``checksumfield.build(hash2)``。

    ``_expr_params`` 协议：StreamRange 模式返回 ``{"start": ..., "end": ...}``；
    ContextBytes 模式返回 ``{}``（bytesfunc 是字段名引用，不需要表达式编译）。

    :param checksumfield: 校验字段（描述符，通常 ``Bytes(32)`` / ``Bytes(64)``）。
    :param hashfunc: ``HashAlgo`` enum（推荐）或 Python callable（兼容）。
    :param bytesfunc: ContextBytes 模式的字段名（str）。
    :param start: StreamRange 模式的起始偏移（int 或 FieldRef/ExprRef）。
    :param end: StreamRange 模式的结束偏移（int 或 FieldRef/ExprRef）。
    """

    __slots__ = ("checksumfield", "hashfunc", "bytesfunc", "start", "end")

    def __init__(self, checksumfield, hashfunc, bytesfunc=None, start=None, end=None):
        """初始化 Checksum 描述符。

        :param checksumfield: 校验字段描述符。
        :param hashfunc: HashAlgo enum 或 callable。
        :param bytesfunc: ContextBytes 字段名（str）。
        :param start: StreamRange 起始偏移。
        :param end: StreamRange 结束偏移。
        """
        self.checksumfield = checksumfield
        self.hashfunc = hashfunc
        self.bytesfunc = bytesfunc
        self.start = start
        self.end = end

        # 校验：bytesfunc 与 start/end 必须二选一
        if bytesfunc is None and (start is None or end is None):
            raise CompilationError(
                "Checksum requires either bytesfunc (ContextBytes mode) "
                "or both start and end (StreamRange mode)"
            )
        if bytesfunc is not None and (start is not None or end is not None):
            raise CompilationError(
                "Checksum bytesfunc and start/end are mutually exclusive"
            )

    @property
    def _expr_params(self):
        """表达式参数协议。

        StreamRange 模式：返回 ``{"start": ..., "end": ...}`` 编译为 ExprOp 列表。
        ContextBytes 模式：返回 ``{}``（bytesfunc 是字段名引用，不编译）。
        """
        if self.start is not None and self.end is not None:
            return {"start": self.start, "end": self.end}
        return {}

    @property
    def bytesfunc_name(self):
        """ContextBytes 模式：返回字段名（供 compile.rs 读取）。"""
        return self.bytesfunc

    @property
    def start_expr(self):
        """StreamRange 模式：返回 start 表达式（供 compile.rs hasattr 检测）。"""
        return self.start

    @property
    def end_expr(self):
        """StreamRange 模式：返回 end 表达式（供 compile.rs hasattr 检测）。"""
        return self.end

    def __repr__(self):
        return "Checksum(checksumfield={!r}, hashfunc={!r})".format(
            self.checksumfield, self.hashfunc
        )


def Checksum(checksumfield, hashfunc, bytesfunc=None, start=None, end=None):
    """创建一个 Checksum 描述符。

    校验和节点。对应 Python construct 的 ``Checksum``（core.py L5532）。

    使用方式（路径 B1：HashAlgo + StreamRange 零拷贝推荐）::

        from construct import (
            StructMixin, Bytes, Tell, Checksum, HashAlgo, rfield, field
        )
        from dataclasses import dataclass

        @dataclass
        class Packet(StructMixin):
            start: int = rfield(Tell())                       # 标记起始
            data: bytes = field(Bytes(16))                    # 被校验数据
            end: int = rfield(Tell())                         # 标记结束
            checksum: bytes = rfield(Checksum(Bytes(32), HashAlgo.SHA256, start, end))

    使用方式（路径 A2：Python callable + ContextBytes 兼容）::

        import hashlib
        from construct import RawCopy

        @dataclass
        class Packet(StructMixin):
            fields: dict = field(RawCopy(Bytes(16)))
            checksum: bytes = field(Checksum(
                Bytes(32),
                lambda d: hashlib.sha256(d).digest(),
                bytesfunc="fields",   # 引用 RawCopy dict 的 "data" 字段
            ))

    Python construct 原版写法（``this`` 语法，construct-rs 不支持）::

        # 原版：Checksum(Bytes(32), lambda d: hashlib.sha256(d).digest(), this.fields.data)
        # construct-rs：bytesfunc="fields"（字段名直接引用）

    限制（设计 §3.8）：

    - **bytesfunc 不支持 lambda**（ADR-014，与 Rebuild 同脉络）。
    - **HashAlgo 未知值编译期拒绝**（CS-10）。
    - **CRC32/Adler32 返回 big-endian 4 字节**（对齐 ``to_bytes(4, 'big')`` 习惯）。

    :param checksumfield: 校验字段描述符。
    :param hashfunc: ``HashAlgo`` enum（推荐）或 Python callable（兼容）。
    :param bytesfunc: ContextBytes 模式的字段名（str）。
    :param start: StreamRange 模式的起始偏移（int 或 FieldRef/ExprRef）。
    :param end: StreamRange 模式的结束偏移（int 或 FieldRef/ExprRef）。
    :return: ``ChecksumDescriptor`` 实例。
    """
    return ChecksumDescriptor(
        checksumfield, hashfunc, bytesfunc=bytesfunc, start=start, end=end
    )


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
    # Phase 6.1 Primitives
    "BytesIntegerDescriptor",
    "BytesInteger",
    "Float16b",
    "Float16l",
    "Float32b",
    "Float32l",
    "Float64b",
    "Float64l",
    "Int24ub",
    "Int24ul",
    "Int24sb",
    "Int24sl",
    "VarIntDescriptor",
    "VarInt",
    "ZigZagDescriptor",
    "ZigZag",
    "Byte",
    "Short",
    "Int",
    "Long",
    "Half",
    "Single",
    "Double",
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
    # Phase 6.2 Strings
    "CStringDescriptor",
    "CString",
    "GreedyStringDescriptor",
    "GreedyString",
    "PaddedStringDescriptor",
    "PaddedString",
    "PascalStringDescriptor",
    "PascalString",
    "NullTerminatedDescriptor",
    "NullTerminated",
    "NullStrippedDescriptor",
    "NullStripped",
    # Phase 6.3 内置 Adapter
    "SubconstructDescriptor",
    "Subconstruct",
    "PeekDescriptor",
    "Peek",
    "RawCopyDescriptor",
    "RawCopy",
    "RebuildDescriptor",
    "Rebuild",
    "PassDescriptor",
    "Pass",
    # Phase 7.2 Streams
    "SeekDescriptor",
    "Seek",
    "PointerDescriptor",
    "Pointer",
    "PrefixedDescriptor",
    "Prefixed",
    # Phase 8 P0: Const / Default / Check
    "ConstDescriptor",
    "Const",
    "DefaultDescriptor",
    "Default",
    "CheckDescriptor",
    "Check",
    # Phase 8 P0: Terminated / Probe
    "TerminatedDescriptor",
    "Terminated",
    "ProbeDescriptor",
    "Probe",
    # Phase 8 P0: Aligned
    "AlignedDescriptor",
    "Aligned",
    # Phase 8 P0: Hex / HexDump
    "HexDescriptor",
    "Hex",
    "HexDumpDescriptor",
    "HexDump",
    # Phase 8 P0: Checksum
    "ChecksumDescriptor",
    "Checksum",
]
