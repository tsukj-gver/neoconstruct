"""construct-rs 内部类（与 Python construct core.py 内部类对齐）。

本模块提供 Enum 系列节点所需的内部类：

- :class:`EnumInteger`：int 子类，Enum 无映射 fallback 的返回类型。
  对应 Python construct core.py L1899。
- :class:`EnumIntegerString`：str 子类，带 ``.intvalue``，Enum decmapping 的值类型。
  对应 Python construct core.py L1904。

这些类在编译期被 Rust 端 ``build_enum_node`` 加载（``py.import_bound("construct._internals")``），
物化为 ``Py<PyType>`` 存入 ``EnumNode``。运行时 ``EnumNode.parse`` 的 fallback 路径
调用 ``EnumInteger(obj)`` 构造实例。

注：``EnumIntegerString`` 实例由 Python 描述符在编译期预构造（
``EnumDescriptor.__init__`` 调用 ``EnumIntegerString.new``），存入 ``decmapping`` dict。
Rust 端 ``EnumNode.parse`` 直接返回 dict 中的预构造实例（不调 ``new``）。
"""

__all__ = [
    "EnumInteger",
    "EnumIntegerString",
]


class EnumInteger(int):
    """Enum 无映射 fallback 的 int 子类。

    对应 Python construct core.py L1899。纯类型标记，无额外方法。

    parse 时若 subcon 返回值不在 decmapping 中，``EnumNode.parse`` 调用
    ``EnumInteger(obj)`` 构造实例返回。``int(EnumInteger(255)) == 255``，
    行为与普通 int 一致，仅类型标记不同（供用户区分"无映射 fallback"与
    "正常 int 字段"）。
    """

    pass


class EnumIntegerString(str):
    """Enum decmapping 的值类型（str 子类，带 ``.intvalue``）。

    对应 Python construct core.py L1904。``str(obj)`` 返回 label，
    ``int(obj)`` 返回 ``.intvalue``（原始 int raw value）。

    编译期由 :class:`construct._descriptors.EnumDescriptor.__init__` 调用
    :meth:`new` 预构造存入 decmapping。运行时 Rust 端 ``EnumNode.parse``
    直接返回 dict 中的实例（不调 ``new``，零开销）。

    使用方式（用户通常不直接构造）::

        >>> s = EnumIntegerString.new(1, 'one')
        >>> str(s)
        'one'
        >>> int(s)
        1
        >>> s.intvalue
        1
    """

    @staticmethod
    def new(intvalue, stringvalue):
        """构造 ``EnumIntegerString`` 实例。

        对应 Python construct core.py L1914 的 staticmethod。

        :param intvalue: 原始 int raw value。
        :param stringvalue: label 字符串。
        :return: ``EnumIntegerString`` 实例（str 子类，含 ``.intvalue``）。
        """
        obj = EnumIntegerString(stringvalue)
        obj.intvalue = intvalue
        return obj

    def __int__(self):
        """返回原始 int raw value（``.intvalue``）。

        若 ``.intvalue`` 未设置（异常情况），返回 0（防御性，对齐 Python
        ``int(str)`` 在非数字字符串上抛错的差异——construct-rs 选择返回 0 而非
        抛错，避免 parse 路径意外失败）。
        """
        return getattr(self, "intvalue", 0)
