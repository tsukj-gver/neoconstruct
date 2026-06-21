"""construct-rs: 高性能二进制解析/构建库（Python 包层）。

本包是 ``construct-rs`` 的用户可见入口。公共 API（StructMixin、field、
Int8ub 等类型描述符单例）将由后续子任务逐步填充。

设计依据：``docs/架构设计.md`` §A（Python 接口设计）、§D.3（Python 包结构）。
"""

# Rust 内核扩展模块。maturin 编译安装后可用。
# 以下导入故意宽松处理：扩展未构建时（如仅做纯 Python 侧开发）仍允许导入本包，
# 仅在实际调用内核功能时才会失败。后续子任务填充内核入口后，此处改为强依赖。
try:
    from . import _construct_rust as _native  # noqa: F401  保留供后续子任务使用
except ImportError:  # pragma: no cover - 仅在扩展未构建时触发
    _native = None

__version__ = "0.1.0"
