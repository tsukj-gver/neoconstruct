"""类型描述符的 Python 侧重导出（纯 Python 委托层）。

设计依据：``docs/架构设计.md`` §A.4（类型描述符）、§D.3（Python 包结构）。

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

3. **StructMixin 子类引用**（嵌套结构）：
   - 直接传 StructMixin 子类作为描述符：``field(Inner)``
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
]
