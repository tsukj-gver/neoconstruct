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
]
