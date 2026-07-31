"""Phase 8.8 AlignedStruct 宏。

设计依据：``docs/design/模块设计/模块设计-Phase8-P0.md`` §4.7。

AlignedStruct 是 Python 函数（宏），展开为 dataclass + 每字段套 Aligned 包装。
对应 Python construct core.py:4334 的 ``AlignedStruct`` 函数。

construct-rs 用户面是 dataclass 语法（命名序），不支持 Python construct 的位置序
（D-P0-4 决策：仅支持 ``**subconskw``）。
"""

from dataclasses import dataclass, make_dataclass
from typing import Any

from ._descriptors import Aligned, StructMixin
from ._mixin import field


def AlignedStruct(modulus, **subconskw):
    """对齐结构宏：每字段用 ``Aligned(modulus, ...)`` 包装。

    对应 Python construct core.py:4334 的 ``AlignedStruct`` 函数（宏展开）。
    在 construct-rs 中，用户用 dataclass 语法（而非位置 subcons）。

    使用方式::

        >>> from construct import AlignedStruct, Int8ub, Int16ub
        >>> AlignedPacket = AlignedStruct(4, a=Int8ub, b=Int16ub)
        >>> # 等价于：
        >>> # @dataclass
        >>> # class AlignedPacket(StructMixin):
        >>> #     a: int = field(Aligned(4, Int8ub))
        >>> #     b: int = field(Aligned(4, Int16ub))

    性能说明：宏展开为 dataclass + Aligned 节点（每字段独立 padding），
    与 Python 原版 Struct 语义一致。无独立 AlignedStructNode 变体。

    :param modulus: 整数或表达式（不支持 context lambda，ADR-014）。
    :param subconskw: 字段名=描述符（关键字参数，D-P0-4 不支持位置参数）。
    :return: 动态生成的 dataclass 类（继承 StructMixin）。
    """
    fields = [
        (name, Any, field(Aligned(modulus, desc)))
        for name, desc in subconskw.items()
    ]
    return make_dataclass("AlignedStruct", fields, bases=(StructMixin,))


__all__ = [
    "AlignedStruct",
]
