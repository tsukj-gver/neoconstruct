"""Python 异常类定义。

本模块将在子任务 1.2/1.7 中实现，定义与 Python construct 2.10.70 对齐的异常层次：

- ``ConstructError``（基类）
- ``StreamError``、``FormatFieldError``、``FieldLengthError``
- ``SizeofError``、``CompilationError``、``UnresolvedReferenceError`` 等

Rust 内核的 ``ConstructError``（见 docs/架构设计.md §B.8）在 FFI 边界处
通过 ``impl From<ConstructError> for PyErr`` 转换为对应的 Python 异常类。
所有异常携带 ``path`` 属性用于追踪出错位置。
"""
