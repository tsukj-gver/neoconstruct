"""neoconstruct: 高性能二进制解析/构建库（Python 包层）。

本包是 ``neoconstruct`` 的用户可见入口，提供声明式二进制结构定义 API。

用户 API 总览（与 ``examples/example.py`` 对齐）::

    from dataclasses import dataclass
    from neoconstruct import StructMixin, field, Int8ub, GreedyBytes

    @dataclass
    class MyMsg(StructMixin):
        address: int = field(Int8ub)
        data: bytes = field(GreedyBytes)

    # build：从实例构造字节
    msg = MyMsg(address=1, data=b'\\x00\\x01')
    built = msg.build()

    # parse：从字节构造实例
    parsed = MyMsg.parse(built)
"""

from ._errors import (
    CancelParsing,
    CheckError,
    ChecksumError,
    CompilationError,
    ConstError,
    ConstructError,
    ExplicitError,
    FieldLengthError,
    FormatFieldError,
    GenericConstructError,
    IndexFieldError,
    IntegerError,
    MappingError,
    NamedTupleError,
    PaddingError,
    RangeError,
    RepeatError,
    RotationError,
    SelectError,
    SizeofError,
    StopFieldError,
    StreamError,
    StringError,
    StringEncoded,
    TerminatedError,
    TimestampError,
    UnionError,
    UnresolvedReferenceError,
    ValidationError,
)
from ._mixin import StructMixin, BitStructMixin, field, rfield, wfield, Tell, Computed

# ---------------------------------------------------------------------------
# Rust 扩展导入
# ---------------------------------------------------------------------------

# Rust 内核扩展模块。maturin 编译安装后可用。
# 以下导入故意宽松处理：扩展未构建时（如仅做纯 Python 侧开发）仍允许导入本包，
# 仅在实际调用内核功能（__init_subclass__ 中的 compile_schema）时才会失败。
try:
    from . import _neoconstruct_core as _native  # noqa: F401  保留供调试与内省
except ImportError:  # pragma: no cover - 仅在扩展未构建时触发
    _native = None

# 导入类型描述符单例与类，供用户通过 ``from neoconstruct import Int8ub`` 使用。
# 扩展未构建时静默跳过。
try:
    from ._descriptors import (
        Bytes,
        BytesDescriptor,
        FormatFieldDescriptor,
        GreedyBytes,
        GreedyBytesDescriptor,
        Int8sb,
        Int8sl,
        Int8ub,
        Int8ul,
        Int16sb,
        Int16sl,
        Int16ub,
        Int16ul,
        Int32sb,
        Int32sl,
        Int32ub,
        Int32ul,
        Int64sb,
        Int64sl,
        Int64ub,
        Int64ul,
        # bit 域描述符
        Bit,
        BitsInteger,
        BitsIntegerDescriptor,
        Nibble,
        Octet,
        # Bitwise
        Bitwise,
        BitwiseDescriptor,
        # Padding / Bytewise / BitsSwapped / ByteSwapped
        Padding,
        PaddingDescriptor,
        Bytewise,
        BytewiseDescriptor,
        BitsSwapped,
        BitsSwappedDescriptor,
        ByteSwapped,
        ByteSwappedDescriptor,
        # Array / GreedyRange / PrefixedArray
        Array,
        ArrayDescriptor,
        GreedyRange,
        GreedyRangeDescriptor,
        PrefixedArray,
        PrefixedArrayDescriptor,
        # Index / StopIf / RepeatUntil
        Index,
        IndexDescriptor,
        StopIf,
        StopIfDescriptor,
        RepeatUntil,
        RepeatUntilDescriptor,
        # Element
        Element,
        ElementDescriptor,
        # Primitives 补充与别名
        BytesInteger,
        BytesIntegerDescriptor,
        Float16b,
        Float16l,
        Float32b,
        Float32l,
        Float64b,
        Float64l,
        Int24ub,
        Int24ul,
        Int24sb,
        Int24sl,
        VarInt,
        VarIntDescriptor,
        ZigZag,
        ZigZagDescriptor,
        Byte,
        Short,
        Int,
        Long,
        Half,
        Single,
        Double,
        # Strings
        CString,
        CStringDescriptor,
        GreedyString,
        GreedyStringDescriptor,
        PaddedString,
        PaddedStringDescriptor,
        PascalString,
        PascalStringDescriptor,
        NullTerminated,
        NullTerminatedDescriptor,
        NullStripped,
        NullStrippedDescriptor,
        # 内置 Adapter
        Subconstruct,
        SubconstructDescriptor,
        Peek,
        PeekDescriptor,
        RawCopy,
        RawCopyDescriptor,
        Rebuild,
        RebuildDescriptor,
        Pass,
        PassDescriptor,
        # Streams (Seek / Pointer / Prefixed)
        Seek,
        SeekDescriptor,
        Pointer,
        PointerDescriptor,
        Prefixed,
        PrefixedDescriptor,
        # Const / Default / Check
        Const,
        ConstDescriptor,
        Default,
        DefaultDescriptor,
        Check,
        CheckDescriptor,
        # Terminated / Probe
        Terminated,
        TerminatedDescriptor,
        Probe,
        ProbeDescriptor,
        # Aligned
        Aligned,
        AlignedDescriptor,
        # Hex / HexDump
        Hex,
        HexDescriptor,
        HexDump,
        HexDumpDescriptor,
        # Checksum
        Checksum,
        ChecksumDescriptor,
        # Enum / FlagsEnum / Mapping / OneOf / NoneOf / Union / Sequence
        # / ProcessXor / ProcessRotateLeft / NamedTuple
        Enum,
        EnumDescriptor,
        FlagsEnum,
        FlagsEnumDescriptor,
        Mapping,
        MappingDescriptor,
        OneOf,
        OneOfDescriptor,
        NoneOf,
        NoneOfDescriptor,
        Union,
        UnionDescriptor,
        Sequence,
        SequenceDescriptor,
        ProcessXor,
        ProcessXorDescriptor,
        ProcessRotateLeft,
        ProcessRotateLeftDescriptor,
        NamedTuple,
        NamedTupleDescriptor,
    )
except ImportError:  # pragma: no cover - 仅在扩展未构建时触发
    pass

# AlignedStruct / Timestamp 宏（不依赖 Rust 扩展）。
try:
    from ._macros import AlignedStruct, Timestamp
except ImportError:  # pragma: no cover
    pass

# HashAlgo Python enum（不依赖 Rust 扩展）。
try:
    from ._hashalgo import HashAlgo
except ImportError:  # pragma: no cover
    pass

# 用户面 Adapter 基类（Python 层，不依赖 Rust 扩展）。
try:
    from ._adapters import Adapter, AdapterDescriptor, SymmetricAdapter, Validator
except ImportError:  # pragma: no cover
    pass

# Conditional 构造器（If / IfThenElse / Switch / Select / FocusedSeq）。
# Python 用户面 + Descriptor 类（不依赖 Rust pyclass，通过 type name 识别）。
try:
    from ._conditional import (
        If,
        IfThenElse,
        IfThenElseDescriptor,
        Switch,
        SwitchDescriptor,
        Select,
        SelectDescriptor,
        FocusedSeq,
        FocusedSeqDescriptor,
        Renamed,
    )
except ImportError:  # pragma: no cover
    pass

# CompiledSchema 编译产物类型（用户通常不直接使用，但导出供类型注解与调试）。
try:
    from ._neoconstruct_core import CompiledSchema
except ImportError:  # pragma: no cover
    pass

__version__ = "0.1.0"

__all__ = [
    # 核心 API
    "StructMixin",
    "BitStructMixin",
    "field",
    "rfield",
    "wfield",
    # RO 节点描述符
    "Tell",
    "Computed",
    # 类型描述符
    "Bytes",
    "GreedyBytes",
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
    "FormatFieldDescriptor",
    "BytesDescriptor",
    "GreedyBytesDescriptor",
    # bit 域描述符
    "Bit",
    "Nibble",
    "Octet",
    "BitsInteger",
    "BitsIntegerDescriptor",
    # Bitwise
    "Bitwise",
    "BitwiseDescriptor",
    # Padding / Bytewise / BitsSwapped / ByteSwapped
    "Padding",
    "PaddingDescriptor",
    "Bytewise",
    "BytewiseDescriptor",
    "BitsSwapped",
    "BitsSwappedDescriptor",
    "ByteSwapped",
    "ByteSwappedDescriptor",
    # Array / GreedyRange / PrefixedArray
    "Array",
    "ArrayDescriptor",
    "GreedyRange",
    "GreedyRangeDescriptor",
    "PrefixedArray",
    "PrefixedArrayDescriptor",
    # Index / StopIf / RepeatUntil
    "Index",
    "IndexDescriptor",
    "StopIf",
    "StopIfDescriptor",
    "RepeatUntil",
    "RepeatUntilDescriptor",
    # Element
    "Element",
    "ElementDescriptor",
    # Primitives 补充与别名
    "BytesInteger",
    "BytesIntegerDescriptor",
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
    "VarInt",
    "VarIntDescriptor",
    "ZigZag",
    "ZigZagDescriptor",
    "Byte",
    "Short",
    "Int",
    "Long",
    "Half",
    "Single",
    "Double",
    # Strings
    "CString",
    "CStringDescriptor",
    "GreedyString",
    "GreedyStringDescriptor",
    "PaddedString",
    "PaddedStringDescriptor",
    "PascalString",
    "PascalStringDescriptor",
    "NullTerminated",
    "NullTerminatedDescriptor",
    "NullStripped",
    "NullStrippedDescriptor",
    # 内置 Adapter
    "Subconstruct",
    "SubconstructDescriptor",
    "Peek",
    "PeekDescriptor",
    "RawCopy",
    "RawCopyDescriptor",
    "Rebuild",
    "RebuildDescriptor",
    "Pass",
    "PassDescriptor",
    # Streams
    "Seek",
    "SeekDescriptor",
    "Pointer",
    "PointerDescriptor",
    "Prefixed",
    "PrefixedDescriptor",
    # Const / Default / Check
    "Const",
    "ConstDescriptor",
    "Default",
    "DefaultDescriptor",
    "Check",
    "CheckDescriptor",
    # Terminated / Probe
    "Terminated",
    "TerminatedDescriptor",
    "Probe",
    "ProbeDescriptor",
    # Aligned / AlignedStruct
    "Aligned",
    "AlignedDescriptor",
    "AlignedStruct",
    "Timestamp",
    # Hex / HexDump
    "Hex",
    "HexDescriptor",
    "HexDump",
    "HexDumpDescriptor",
    # Checksum / HashAlgo
    "Checksum",
    "ChecksumDescriptor",
    "HashAlgo",
    # Enum / FlagsEnum / Mapping / OneOf / NoneOf / Union / Sequence
    # / ProcessXor / ProcessRotateLeft / NamedTuple
    "Enum",
    "EnumDescriptor",
    "FlagsEnum",
    "FlagsEnumDescriptor",
    "Mapping",
    "MappingDescriptor",
    "OneOf",
    "OneOfDescriptor",
    "NoneOf",
    "NoneOfDescriptor",
    "Union",
    "UnionDescriptor",
    "Sequence",
    "SequenceDescriptor",
    "ProcessXor",
    "ProcessXorDescriptor",
    "ProcessRotateLeft",
    "ProcessRotateLeftDescriptor",
    "NamedTuple",
    "NamedTupleDescriptor",
    # 用户面 Adapter 基类
    "Adapter",
    "AdapterDescriptor",
    "SymmetricAdapter",
    "Validator",
    # Conditional 构造器
    "If",
    "IfThenElse",
    "IfThenElseDescriptor",
    "Switch",
    "SwitchDescriptor",
    "Select",
    "SelectDescriptor",
    "FocusedSeq",
    "FocusedSeqDescriptor",
    "Renamed",
    # 编译产物
    "CompiledSchema",
    # 异常
    "ConstructError",
    "StreamError",
    "FormatFieldError",
    "FieldLengthError",
    "SizeofError",
    "CompilationError",
    "UnresolvedReferenceError",
    "GenericConstructError",
    "IntegerError",
    "PaddingError",
    "RangeError",
    "RepeatError",
    "StopFieldError",
    "IndexFieldError",
    "StringError",
    "ExplicitError",
    "SelectError",
    "StringEncoded",
    "ConstError",
    "CheckError",
    "ChecksumError",
    "TerminatedError",
    "CancelParsing",
    "MappingError",
    "ValidationError",
    "UnionError",
    "RotationError",
    "NamedTupleError",
    "TimestampError",
]
