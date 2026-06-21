"""StructMixin 基类与 field() 声明函数（纯 Python 实现）。

本模块将在子任务 1.7 中实现，包含：

- ``StructMixin``：所有用户二进制结构类的基类，负责协调 ``__init_subclass__``
  编译流程与 ``parse``/``build`` 方法（见 docs/架构设计.md §A.2）。
- ``field(subcon)``：声明一个二进制字段，返回同时满足 @dataclass 字段协议
  与 Mixin 编译协议的描述符对象（见 docs/架构设计.md §A.3）。
- 延迟桩机制：前向引用未解析时安装的占位 parse/build 方法（见 §E.6）。
"""
