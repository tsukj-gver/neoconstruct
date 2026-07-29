"""tests 公共组件包（_ 前缀 → pytest 不收集）。

设计依据：docs/design/基础设施/测试框架设计.md §1.1 + §2.2.2

导出公开 API（设计附录 A 步骤 1）：
    - run_parity_case / assert_parity / assert_fidelity / make_parity_results_fixture
    - PARITY_SCRIPT_TEMPLATE
    - normalize
"""

from .normalize import normalize
from .parity import (
    PARITY_SCRIPT_TEMPLATE,
    assert_fidelity,
    assert_parity,
    make_parity_results_fixture,
    run_parity_case,
)

__all__ = [
    "normalize",
    "run_parity_case",
    "assert_parity",
    "assert_fidelity",
    "make_parity_results_fixture",
    "PARITY_SCRIPT_TEMPLATE",
]
