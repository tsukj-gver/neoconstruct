"""construct_rust -- Rust-backed port of the Python ``construct`` library.

This pure-Python package wraps the native extension (:mod:`construct_rust._core`)
and re-exports its symbols alongside version metadata and (in later sub-tasks)
pure-Python base classes for the hybrid architecture.
"""

from ._core import *          # native extension symbols
from .version import *        # version, version_string, release_date
from . import lib             # make construct_rust.lib accessible (mirrors construct)

# 10.4 expression system: pure-Python expression objects (this, obj_, etc.)
from .expr import (
    this, obj_, list_,
    len_, sum_, min_, max_, abs_,
    ExprMixin, UniExpr, BinExpr, Path, Path2, FuncPath,
    opnames, evaluate,
)

# 10.4 hybrid architecture: pure-Python adapter base classes
from ._adapter import (
    Construct,
    Subconstruct,
    Adapter,
    SymmetricAdapter,
    Validator,
)

# 10.8 pure-Python stream-processing constructs
from ._stream import (
    NullTerminated,
    NullStripped,
    RestreamData,
    ProcessXor,
    ProcessRotateLeft,
    OffsettedEnd,
)

# 10.9 pure-Python string constructs
from ._string import (
    StringEncoded,
    PascalString,
    GreedyString,
)

# 10.9 debugging tools (stubs)
from .debug import (
    Probe,
    Debugger,
)

# 10.7 pure-Python constructs: NamedTuple (depends on collections.namedtuple)
from ._namedtuple import NamedTuple

# Enum helper classes and wrappers (pure Python, mirrors construct.core)
from ._enum import EnumIntegerString, EnumInteger, Enum, FlagsEnum

# Optional wrapper (pure Python, mirrors construct.core.Optional)
from ._optional import Optional

# 14.1 dataclass-first API: ConstructMixin + cs_field
from ._fields import cs_field
from ._mixins import ConstructMixin, construct_dataclass

# 10.7 Timestamp stub: requires 'arrow' library (not installed in Phase 10).
# Import does not fail; calling Timestamp/TimestampAdapter raises ImportError.
try:
    import arrow  # noqa: F401
    from ._timestamp import Timestamp, TimestampAdapter, TimestampError
except ImportError:
    class TimestampError(Exception):
        """Raised when Timestamp encounters an error (stub when arrow is missing)."""

        pass

    def Timestamp(*args, **kwargs):
        """Stub: Timestamp requires the 'arrow' library."""
        raise ImportError(
            "Timestamp requires the 'arrow' library. "
            "Install it with: pip install arrow"
        )

    TimestampAdapter = Timestamp  # alias

# ---------------------------------------------------------------------------
# metadata (mirrors construct/__init__.py)
# ---------------------------------------------------------------------------
__version__ = version_string
