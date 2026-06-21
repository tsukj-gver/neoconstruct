"""类型描述符委托层（纯 Python）。

本模块将在子任务 1.6/1.7 中实现，包含：

- 预定义单例描述符（``Int8ub``、``Int16ub``、``GreedyBytes`` 等）的 Python 侧导入与
  重导出，供用户通过 ``field(Int8ub)`` 声明字段。
- 可实例化描述符（如 ``Bytes(length)``）的 Python 包装（若需要）。

实际的描述符 pyclass 定义在 Rust 侧（``src/descriptors/``），由 ``_native`` 扩展提供。
设计依据：docs/架构设计.md §A.4（类型描述符）、§D.3。
"""
