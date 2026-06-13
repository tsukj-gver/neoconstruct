"""``construct_rust.lib`` -- utility sub-package mirroring ``construct.lib``.

This is a skeleton re-export hub. The actual sub-modules are populated by later
Phase 10 sub-tasks:

* ``containers``  -- :class:`Container` / :class:`ListContainer` (10.3)
* ``binary``      -- integer/bits/bytes helpers (10.9, pure Python)
* ``bitstream``   -- :class:`RestreamedBytesIO` / :class:`RebufferedBytesIO` (10.9)
* ``hex``         -- hex dump utilities (10.9)
* ``py3compat``   -- Python 2/3 compatibility shims (10.9)

Each sub-task will uncomment (or replace) the corresponding import line below
once the module exists. Keeping them commented ensures ``import construct_rust.lib``
succeeds during 10.1 even though the sub-modules are not yet created.
"""

from .containers import (
    Container,
    ListContainer,
    globalPrintFullStrings,
    globalPrintFalseFlags,
    globalPrintPrivateEntries,
    setGlobalPrintFullStrings,
    setGlobalPrintFalseFlags,
    setGlobalPrintPrivateEntries,
    recursion_lock,
    value_to_string,
)
# from .binary import *        # 10.9
# from .bitstream import *     # 10.9
# from .hex import *           # 10.9
# from .py3compat import *     # 10.9

__all__ = [
    "Container",
    "ListContainer",
    "globalPrintFullStrings",
    "globalPrintFalseFlags",
    "globalPrintPrivateEntries",
    "setGlobalPrintFullStrings",
    "setGlobalPrintFalseFlags",
    "setGlobalPrintPrivateEntries",
    "recursion_lock",
    "value_to_string",
]  # populated as sub-modules are added