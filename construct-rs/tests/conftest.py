"""pytest 全局配置：确保 construct-rs 包优先于 site-packages 中的同名 Python 包。

背景：基准测试需要安装 Python 原版 ``construct==2.10.70`` 作为绝对基线（AGENTS.md
性能门禁 §2）。该包与 construct-rs 同名，安装在 site-packages 下。pytest 默认
将 site-packages 置于 sys.path 前部，会导致 ``import construct`` 误命中 Python
原版（无 Rust 扩展）。

本文件在测试收集前加载（早于任何 ``import construct``），通过显式 sys.path 操纵
确保 construct-rs 的 ``python/`` 目录优先。
"""

import os
import sys

# construct-rs 的 Python 包源码根目录：tests/ 的上一级 + python/
_CRS_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "python"))

# 把 construct-rs 的 python/ 目录插入 sys.path 最前部。
# 即使 pyproject.toml 的 pythonpath 配置已起作用，这里也做幂等保护。
if _CRS_ROOT not in sys.path or sys.path[0] != _CRS_ROOT:
    sys.path.insert(0, _CRS_ROOT)

# 清除任何已缓存的 construct 模块（以防之前在其他 conftest 阶段被预导入）。
# 仅在尚未导入 construct 时无操作。
for _key in list(sys.modules):
    if _key == "construct" or _key.startswith("construct."):
        del sys.modules[_key]
