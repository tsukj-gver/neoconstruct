"""Enum integer/string helper classes and Enum/FlagsEnum wrappers.

Mirrors construct.core.EnumIntegerString, EnumInteger, Enum, and FlagsEnum.
The wrappers delegate to the Rust-backed _core.Enum but post-process parse
results to produce EnumIntegerString/EnumInteger objects.
"""

import enum as _enum_module

from ._core import Enum as _RustEnum
from .lib.containers import Container
from ._core import FlagsEnum as _RustFlagsEnum


class EnumInteger(int):
    """Used internally when an unmapped enum value is parsed."""
    pass


class BitwisableString(str):
    """Used internally by FlagsEnum to support ``d.one|d.two`` syntax."""

    def __or__(self, other):
        return BitwisableString("{}|{}".format(self, other))


class EnumIntegerString(str):
    """Used internally when a mapped enum value is parsed.

    Behaves as a string (the label name) but ``int()`` returns the
    original integer value.
    """

    def __repr__(self):
        return "EnumIntegerString.new(%s, %s)" % (
            self.intvalue,
            str.__repr__(self),
        )

    def __int__(self):
        return self.intvalue

    @staticmethod
    def new(intvalue, stringvalue):
        ret = EnumIntegerString(stringvalue)
        ret.intvalue = intvalue
        return ret


class Enum:
    """Translates unicode label names to subcon values, and vice versa.

    Wraps the Rust-backed Enum construct, post-processing parse results
    to return EnumIntegerString (for mapped values) or EnumInteger (for
    unmapped values).
    """

    def __init__(self, subcon, *merge, **mapping):
        # Merge enum.IntEnum / enum.IntFlag members
        for e in merge:
            for entry in e:
                mapping[entry.name] = entry.value
        self._mapping = dict(mapping)
        self._inv_mapping = {v: k for k, v in mapping.items()}
        self._rust = _RustEnum(subcon, **mapping)

    def __getattr__(self, name):
        if name.startswith("_"):
            raise AttributeError(name)
        if name in self._mapping:
            return EnumIntegerString.new(self._mapping[name], name)
        raise AttributeError(
            "'Enum' object has no attribute '%s'" % name)

    def parse(self, data, **kw):
        result = self._rust.parse(data, **kw)
        return self._postprocess(result)

    def parse_stream(self, stream, **kw):
        result = self._rust.parse_stream(stream, **kw)
        return self._postprocess(result)

    def parse_file(self, filename, **kw):
        result = self._rust.parse_file(filename, **kw)
        return self._postprocess(result)

    def build(self, obj, **kw):
        return self._rust.build(obj, **kw)

    def build_stream(self, obj, stream, **kw):
        return self._rust.build_stream(obj, stream, **kw)

    def build_file(self, obj, filename, **kw):
        return self._rust.build_file(obj, filename, **kw)

    def sizeof(self, **kw):
        return self._rust.sizeof(**kw)

    def compile(self, *args, **kw):
        """No-op compile for API compatibility with Python construct.

        The Rust kernel is already compiled, so this returns ``self``.
        """
        return self

    def _postprocess(self, result):
        if isinstance(result, str) and not isinstance(result, EnumIntegerString):
            if result in self._mapping:
                return EnumIntegerString.new(self._mapping[result], result)
        elif isinstance(result, int) and not isinstance(result, bool):
            if not isinstance(result, EnumInteger):
                return EnumInteger(result)
        return result

    # Operator support (for "x" / Enum(...) syntax)
    def __rtruediv__(self, name):
        from ._core import Renamed

        return Renamed(self, name)

    def __mul__(self, other):
        return self._rust * other

    def __rmul__(self, other):
        return other * self._rust

    def __add__(self, other):
        return self._rust + other

    def __rshift__(self, other):
        return self._rust >> other

    def __getitem__(self, count):
        return self._rust[count]


class FlagsEnum:
    """Translates integer flags to/from a Container of bool labels.

    Wraps the Rust-backed FlagsEnum construct.
    """

    def __init__(self, subcon, *merge, **flags):
        for e in merge:
            for entry in e:
                flags[entry.name] = entry.value
        self._flags = dict(flags)
        self._subcon = subcon
        self._rust = _RustFlagsEnum(subcon, **flags)

    def __getattr__(self, name):
        if name.startswith("_"):
            raise AttributeError(name)
        flags = self.__dict__.get("_flags", {})
        if name in flags:
            return BitwisableString(name)
        raise AttributeError(
            "'FlagsEnum' object has no attribute '%s'" % name)

    def parse(self, data, **kw):
        return self._rust.parse(data, **kw)

    def parse_stream(self, stream, **kw):
        return self._rust.parse_stream(stream, **kw)

    def parse_file(self, filename, **kw):
        return self._rust.parse_file(filename, **kw)

    def build(self, obj, **kw):
        from . import MappingError
        if isinstance(obj, int):
            return self._subcon.build(obj, **kw)
        if hasattr(obj, "intvalue"):
            return self._subcon.build(int(obj), **kw)
        if isinstance(obj, str):
            # BitwisableString or label string: convert to int via flags
            total = 0
            for part in obj.split("|"):
                if part in self._flags:
                    total |= self._flags[part]
                else:
                    raise MappingError("unknown mapping: %r" % (part,))
            return self._subcon.build(total, **kw)
        if isinstance(obj, (dict, Container)):
            return self._rust.build(obj, **kw)
        raise MappingError("unknown mapping: %r" % (obj,))

    def build_stream(self, obj, stream, **kw):
        if isinstance(obj, int):
            return self._subcon.build_stream(obj, stream, **kw)
        return self._rust.build_stream(obj, stream, **kw)

    def build_file(self, obj, filename, **kw):
        if isinstance(obj, int):
            return self._subcon.build_file(obj, filename, **kw)
        return self._rust.build_file(obj, filename, **kw)

    def sizeof(self, **kw):
        return self._rust.sizeof(**kw)

    def compile(self, *args, **kw):
        """No-op compile for API compatibility with Python construct."""
        return self

    def __rtruediv__(self, name):
        from ._core import Renamed

        return Renamed(self, name)

    def __mul__(self, other):
        return self._rust * other

    def __rmul__(self, other):
        return other * self._rust

    def __add__(self, other):
        return self._rust + other

    def __rshift__(self, other):
        return self._rust >> other

    def __getitem__(self, count):
        return self._rust[count]
