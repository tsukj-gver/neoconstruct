"""包元数据一致性：Python 侧 ``__version__`` 与 Rust 内核同源。

Rust 内核的 ``version()`` / ``__version__`` 取自 Cargo.toml 的
``[package] version``（编译期 ``env!("CARGO_PKG_VERSION")``）；Python 侧
直接引用内核值，消除双处硬编码漂移。
"""

import neoconstruct
from neoconstruct import _neoconstruct_core


def test_python_version_matches_rust_core():
    """``neoconstruct.__version__`` 与扩展模块 ``__version__`` 一致。"""
    assert neoconstruct.__version__ == _neoconstruct_core.__version__


def test_core_version_function_matches_module_constant():
    """``version()`` 函数与模块常量一致（FFI 链路 sanity）。"""
    assert _neoconstruct_core.version() == _neoconstruct_core.__version__


def test_version_is_semver_like():
    """版本号非占位值且形如 X.Y.Z。"""
    assert neoconstruct.__version__ not in ("", "0.0.0")
    parts = neoconstruct.__version__.split(".")
    assert len(parts) >= 2
    assert all(p.isdigit() for p in parts[:2]), neoconstruct.__version__
