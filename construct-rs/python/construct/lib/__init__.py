"""construct.lib 子包（construct-rs port）。

设计依据：``docs/design/模块设计/模块设计-Phase8-P0.md`` §2.8。

本子包是 Python construct ``construct/lib/`` 的复刻。当前仅包含
``hex.py``（Hex/HexDump 显示类，Phase 8.4 port）。

Rust 端（compile.rs / hex.rs）通过 ``py.import_bound("construct.lib.hex")``
加载显示类。
"""

from . import hex  # noqa: F401  re-export 子模块供 Rust 端 import 链工作
