"""construct-rs Python 异常层次。

设计依据：``docs/架构设计.md`` §B.8。

所有异常携带 ``message`` 与可选的 ``path`` 属性，``str(e)`` 格式与 Python construct
2.10.70 对齐：path 非 None 时为 ``"Error in path {path}\\n{message}"``。

异常层次与 Rust 侧 ``ConstructError`` 枚举变体一一对应：

============================  ==========================================
Python 异常                    Rust ConstructError 变体
============================  ==========================================
:class:`ConstructError`       （基类，对应所有变体）
:class:`StreamError`          ``ConstructError::Stream``
:class:`FormatFieldError`     ``ConstructError::FormatField``
:class:`FieldLengthError`     ``ConstructError::FieldLength``
:class:`SizeofError`          （sizeof 调用失败时）
:class:`CompilationError`     ``ConstructError::Compilation``
:class:`UnresolvedReferenceError`  ``ConstructError::UnresolvedReference``
:class:`GenericConstructError``ConstructError::Generic``
============================  ==========================================

.. note::

    Phase 1 的 Rust ``From<ConstructError> for PyErr`` 实现将所有变体映射为
    ``PyValueError``（携带含 path 的完整消息）。当后续更新 Rust 侧映射为各 Python
    异常子类后，用户可通过 ``except StreamError`` 等精确捕获。当前阶段用户可通过
    ``except ValueError`` 或 ``except Exception`` 捕获 Rust 抛出的错误，错误消息
    中已包含 path 信息。
"""


class ConstructError(Exception):
    """所有 construct-rs 错误的基类。

    :param message: 错误详情。
    :param path: 错误发生的路径（如 ``"root.header.flags"``），编译期错误为 ``None``。
    """

    def __init__(self, message: str = "", path: str = None):
        self.message = message
        self.path = path
        if path is not None:
            full = "Error in path {}\n{}".format(path, message)
        else:
            full = message
        super().__init__(full)

    def __str__(self):
        if self.path is not None:
            return "Error in path {}\n{}".format(self.path, self.message)
        return self.message


class StreamError(ConstructError):
    """流错误：字节不足、流读/写失败等。

    对应 Python construct 的 ``StreamError`` 和 Rust 的
    ``ConstructError::Stream``。
    """


class FormatFieldError(ConstructError):
    """格式字段错误：整数/浮点解析或构建失败。

    对应 Python construct 的 ``FormatFieldError`` 和 Rust 的
    ``ConstructError::FormatField``。
    """


class FieldLengthError(ConstructError):
    """字段长度错误：``Bytes(n)`` 的实际数据长度不匹配等。

    对应 Python construct 的 ``FieldError`` / ``StreamError``（长度不匹配场景）和
    Rust 的 ``ConstructError::FieldLength``。
    """


class SizeofError(ConstructError):
    """大小计算错误：字段大小无法在编译期或当前上下文确定。

    对应 Python construct 的 ``SizeofError``。
    """


class CompilationError(ConstructError):
    """编译错误：schema 编译失败（未知描述符、字段重复等）。

    对应 Rust 的 ``ConstructError::Compilation``。编译期错误不携带 ``path``。
    """


class UnresolvedReferenceError(ConstructError):
    """未解析的类型引用：前向引用未在运行时解析成功。

    对应 Rust 的 ``ConstructError::UnresolvedReference``。
    通常发生在互相引用的结构体中，被引用类尚未完成延迟编译时。
    """


class GenericConstructError(ConstructError):
    """通用错误：其他无法归类的运行时错误。

    对应 Rust 的 ``ConstructError::Generic``。
    """


__all__ = [
    "ConstructError",
    "StreamError",
    "FormatFieldError",
    "FieldLengthError",
    "SizeofError",
    "CompilationError",
    "UnresolvedReferenceError",
    "GenericConstructError",
]
