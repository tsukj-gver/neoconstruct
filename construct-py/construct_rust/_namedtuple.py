"""NamedTuple construct — maps parsed values to ``collections.namedtuple`` instances.

This is a pure-Python implementation because it depends on the Python standard
library's ``collections.namedtuple`` factory, which dynamically generates Python
classes. This cannot be reasonably replicated in Rust.

The ``NamedTuple`` class inherits from the pure-Python :class:`Adapter` base
class (in ``_adapter.py``), allowing it to wrap either Rust-backed PyO3
constructs or other pure-Python constructs via the dual-path mechanism.
"""

import collections

from ._adapter import Adapter
from .lib.containers import Container


def _is_composite(obj):
    """Check if obj is a Struct/Sequence/Array/GreedyRange (Rust or Python).

    Uses duck typing (PM decision §11.5): checks the Python type name rather
    than ``isinstance``, since Rust-backed PyO3 wrappers do not inherit from
    the pure-Python Construct base class.
    """
    class_name = type(obj).__name__
    return class_name in ("Struct", "Sequence", "Array", "GreedyRange")


class NamedTuple(Adapter):
    """Both arrays, structs, and sequences can be mapped to a namedtuple.

    :param tuplename: name of the namedtuple class
    :param tuplefields: string of space-separated field names, or list of strings
    :param subcon: Construct instance (Struct, Sequence, Array, GreedyRange)
    """

    def __init__(self, tuplename, tuplefields, subcon):
        if not _is_composite(subcon):
            from . import NamedTupleError

            raise NamedTupleError(
                "subcon is neither Struct Sequence Array GreedyRange")
        super().__init__(subcon)
        self.tuplename = tuplename
        self.tuplefields = tuplefields
        self.factory = collections.namedtuple(tuplename, tuplefields)

    def _decode(self, obj, context, path):
        if isinstance(obj, (list, tuple)):
            return self.factory(*obj)
        if isinstance(obj, dict):
            obj = {k: v for k, v in dict.items(obj)
                   if not (isinstance(k, str) and k.startswith("_"))}
            return self.factory(**obj)
        from . import NamedTupleError

        raise NamedTupleError(
            "subcon is neither Struct Sequence Array GreedyRange",
            path=path)

    def _encode(self, obj, context, path):
        if isinstance(obj, tuple) and hasattr(obj, "_fields"):
            # namedtuple instance
            fields = [f for f in obj._fields]
            values = {f: getattr(obj, f) for f in fields}
            # Check if subcon is a Struct-like (dict-returning) construct
            if type(self.subcon).__name__ == "Struct":
                return Container(values)
            return list(obj)
        if isinstance(obj, (list, dict)):
            return obj
        from . import NamedTupleError

        raise NamedTupleError("cannot encode in path " + str(path))
