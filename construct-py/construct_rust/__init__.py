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

# ---------------------------------------------------------------------------
# metadata (mirrors construct/__init__.py)
# ---------------------------------------------------------------------------
__version__ = version_string
