"""construct-rs: 高性能二进制解析/构建库（Python 包层）。

本包是 ``construct-rs`` 的用户可见入口，提供声明式二进制结构定义 API。

用户 API 总览（与 ``examples/example.py`` 对齐）::

    from dataclasses import dataclass
    from construct import StructMixin, field, Int8ub, GreedyBytes

    @dataclass
    class MyMsg(StructMixin):
        address: int = field(Int8ub)
        data: bytes = field(GreedyBytes)

    # build：从实例构造字节
    msg = MyMsg(address=1, data=b'\\x00\\x01')
    built = msg.build()

    # parse：从字节构造实例
    parsed = MyMsg.parse(built)

设计依据：``docs/架构设计.md`` §A（Python 接口设计）、§D.3（Python 包结构）。
"""

from ._errors import (
    CompilationError,
    ConstructError,
    FieldLengthError,
    FormatFieldError,
    GenericConstructError,
    SizeofError,
    StreamError,
    UnresolvedReferenceError,
)
from ._mixin import StructMixin, field, rfield, wfield, Tell, Computed

# ---------------------------------------------------------------------------
# Rust 扩展导入
# ---------------------------------------------------------------------------

# Rust 内核扩展模块。maturin 编译安装后可用。
# 以下导入故意宽松处理：扩展未构建时（如仅做纯 Python 侧开发）仍允许导入本包，
# 仅在实际调用内核功能（__init_subclass__ 中的 compile_schema）时才会失败。
try:
    from . import _construct_rust as _native  # noqa: F401  保留供调试与内省
except ImportError:  # pragma: no cover - 仅在扩展未构建时触发
    _native = None

# 导入类型描述符单例与类，供用户通过 ``from construct import Int8ub`` 使用。
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
    )
except ImportError:  # pragma: no cover - 仅在扩展未构建时触发
    pass

# CompiledSchema 编译产物类型（用户通常不直接使用，但导出供类型注解与调试）。
try:
    from ._construct_rust import CompiledSchema
except ImportError:  # pragma: no cover
    pass

__version__ = "0.1.0"

__all__ = [
    # 核心 API
    "StructMixin",
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
]
