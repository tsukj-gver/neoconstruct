"""construct-rs 内置的 ``construct.lib`` 包。

本包提供 Container / ListContainer 等 Python 端数据结构，供 Rust 内核
通过 CPython API 构造与读取。原版 construct 的 ``construct.lib`` 还包含
``binary`` / ``bitstream`` / ``py3compat`` 等工具模块，construct-rs 仅按需
提供 ``containers``，其他工具未来按需添加。
"""
