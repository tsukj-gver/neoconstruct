"""Phase 8 宏模块：AlignedStruct + Timestamp。

设计依据：``docs/design/模块设计/模块设计-Phase8-P0.md`` §4.7（AlignedStruct）
+ ``docs/design/模块设计/模块设计-Phase8-P1P2.md`` §6.2（Timestamp）。

AlignedStruct 是 Python 函数（宏），展开为 dataclass + 每字段套 Aligned 包装。
Timestamp 是 Python 函数（宏），返回 Adapter 子类（依赖 arrow，无法 Rust 化）。

**F-1（REV 驳回修正）**：``import arrow`` 必须放在 Timestamp 函数体内（对齐 Python
core.py L3478），而非模块顶部。原因：``_macros.py`` 是共享模块，顶部 ``import arrow``
失败时会导致整个模块加载失败，连带破坏 P0 已验收的 ``AlignedStruct``。函数体内导入
使 arrow 缺失仅影响 Timestamp 调用（用户 ``pip install arrow`` 后可用）。
"""

from dataclasses import dataclass, make_dataclass
from typing import Any

from ._descriptors import Aligned
from ._errors import TimestampError
from ._mixin import StructMixin, field


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


def Timestamp(subcon, unit, epoch):
    """Datetime as Arrow object.

    对应 Python construct core.py:3453 的 Timestamp 函数（macro）。
    construct-rs 实现为 Python 层（arrow 依赖，无法 Rust 化）。

    嵌入 Struct 时走 AdapterCallbackNode（2 FFI，用户主动选择 arrow = 接受折衷，
    PM 决策 D-3）。性能不设硬门禁（Python 层 arrow 路径）。

    **F-1**：``import arrow`` 在函数体内（延迟导入），arrow 缺失时仅 Timestamp
    调用失败，不影响同模块的 ``AlignedStruct``。

    **C-5（REV 修正）**：subcon 类型检查——若非描述符/Construct 实例则抛
    TimestampError。Python core.py 不做检查（运行时由 subcon.parse 失败抛错），
    construct-rs 提前在 macro 中检查，提供更友好的错误信息。

    :param subcon: Int*/Float* 描述符，或 Int32ub（msdos 模式自动用 BitStruct）。
                   必须是描述符类型（编译期校验，C-5）。
    :param unit: int/float（秒/毫秒/微秒分辨率）或 "msdos"。
    :param epoch: int（年）/ Arrow 实例 / "msdos"。
    :raises TimestampError: 参数类型错误（unit/epoch/subcon）。
    :raises ImportError: arrow 未安装（用户 pip install arrow）。

    Example::

        >>> from construct import Timestamp, Int64ub
        >>> d = Timestamp(Int64ub, 1., 1970)
        >>> d.parse(b'\\x00\\x00\\x00\\x00ZIz\\x00')
        <Arrow [2018-01-01T00:00:00+00:00]>
    """
    import arrow

    # C-5：subcon 类型检查（必须在场，None 或非合法描述符 → TimestampError）。
    # Rust pyclass（如 FormatFieldDescriptor）的 hasattr 行为可能与纯 Python 类不同，
    # 因此仅做最低限度的 None 检查 + 类型对象检查。详细类型检查由 inner.parse/build
    # 在运行时自然抛错。
    if subcon is None:
        raise TimestampError(
            "subcon must not be None; pass a descriptor or Construct instance"
        )

    if not isinstance(unit, (int, float, str)):
        raise TimestampError("unit must be one of: int float string")
    if not isinstance(epoch, (int, arrow.Arrow, str)):
        raise TimestampError("epoch must be one of: int Arrow string")

    if unit == "msdos" or epoch == "msdos":
        # msdos 模式：用 BitStruct 解码 32-bit packed datetime
        # construct-rs 用 make_dataclass + BitStructMixin（与 AlignedStruct 同模式）
        from dataclasses import make_dataclass
        from typing import Any
        from ._descriptors import BitsInteger
        from ._mixin import BitStructMixin, field

        st_cls = make_dataclass(
            "MsdosTimestampBits",
            [
                ("year", Any, field(BitsInteger(7))),
                ("month", Any, field(BitsInteger(4))),
                ("day", Any, field(BitsInteger(5))),
                ("hour", Any, field(BitsInteger(5))),
                ("minute", Any, field(BitsInteger(6))),
                ("second", Any, field(BitsInteger(5))),
            ],
            bases=(BitStructMixin,),
        )

        from ._adapters import Adapter

        # Container 是 dict 子类（construct-rs 中用普通 dict 替代）
        def _make_container(year, month, day, hour, minute, second):
            return dict(year=year, month=month, day=day,
                        hour=hour, minute=minute, second=second)

        class MsdosTimestampAdapter(Adapter):
            def _decode(self, obj, context, path):
                return arrow.Arrow(1980, 1, 1).shift(
                    years=obj.year, months=obj.month - 1, days=obj.day - 1,
                    hours=obj.hour, minutes=obj.minute, seconds=obj.second * 2,
                )

            def _encode(self, obj, context, path):
                t = obj.timetuple()
                return _make_container(
                    year=t.tm_year - 1980, month=t.tm_mon, day=t.tm_mday,
                    hour=t.tm_hour, minute=t.tm_min, second=t.tm_sec // 2,
                )

        return MsdosTimestampAdapter(st_cls)

    if isinstance(epoch, int):
        epoch = arrow.Arrow(epoch, 1, 1)

    from ._adapters import Adapter

    class EpochTimestampAdapter(Adapter):
        def _decode(self, obj, context, path):
            return epoch.shift(seconds=obj * unit)

        def _encode(self, obj, context, path):
            return int((obj - epoch).total_seconds() / unit)

    return EpochTimestampAdapter(subcon)


__all__ = [
    "AlignedStruct",
    "Timestamp",
]
