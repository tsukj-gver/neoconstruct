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
from .py3compat import *
from .binary import *
from .bitstream import *
from .hex import *

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
    # py3compat
    "PY", "PYPY", "ONWINDOWS", "INT2BYTE_CACHE",
    "int2byte", "byte2int", "str2bytes", "bytes2str",
    "PY2", "PY3", "stringtypes", "integertypes",
    "unicodestringtype", "bytestringtype", "reprstring",
    "integers2bytes", "bytes2integers", "trimstring",
    # binary
    "integer2bits", "integer2bytes", "bits2integer", "bytes2integer",
    "bytes2bits", "bits2bytes", "swapbytes", "swapbytesinbits",
    "swapbitsinbytes", "hexlify", "unhexlify",
    "BYTES2BITS_CACHE", "BITS2BYTES_CACHE", "SWAPBITSINBYTES_CACHE",
    # bitstream
    "RestreamedBytesIO", "RebufferedBytesIO",
    # hex
    "HexDisplayedInteger", "HexDisplayedBytes", "HexDisplayedDict",
    "HexDumpDisplayedBytes", "HexDumpDisplayedDict",
    "PRINTABLE", "HEXPRINT", "hexdump", "hexundump",
]