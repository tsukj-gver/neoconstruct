"""Pure-Python adapter/validator base classes for the hybrid architecture.

These classes allow Python users to subclass :class:`Adapter`,
:class:`SymmetricAdapter`, and :class:`Validator` to create custom
constructs that transform or validate values. The subcon wrapped by these
adapters can be either a Rust-backed PyO3 construct or another pure-Python
construct.

Design note (PM decision, Phase 10 sub-task 10.4):

The original Python ``Adapter._parse`` calls ``self.subcon._parsereport()``,
which is an internal API not exposed by PyO3-wrapped Rust constructs. Per
design decision ( 方案 B), the base classes here use a dual-path approach:

* If ``self.subcon`` has ``_parsereport`` (pure-Python construct), the
  internal API is used directly, passing the same context by reference.
* Otherwise (Rust-backed PyO3 construct), the public ``parse_stream`` /
  ``build_stream`` API is used. Non-underscore context fields are passed
  as keyword arguments to preserve field references. Underscore-prefixed
  keys (``_``, ``_params``, ``_parsing``, etc.) are filtered to avoid
  circular references and Python-internal state leaking into the Rust
  context.
"""

import io

from .lib.containers import Container
from . import expr as _expr

evaluate = _expr.evaluate


class Construct(object):
    """Minimal pure-Python Construct base class.

    Provides the external API (``parse``, ``parse_stream``, ``build``,
    ``build_stream``, ``sizeof``) and the subclass API (``_parse``,
    ``_build``, ``_sizeof``, ``_parsereport``). Subclasses override the
    underscore-prefixed methods.
    """

    def __init__(self):
        self.name = None
        self.docs = ""
        self.flagbuildnone = False
        self.parsed = None

    def __repr__(self):
        return "<%s%s%s%s>" % (
            self.__class__.__name__,
            " " + self.name if self.name else "",
            " +nonbuild" if self.flagbuildnone else "",
            " +docs" if self.docs else "",
        )

    def parse(self, data, **contextkw):
        return self.parse_stream(io.BytesIO(data), **contextkw)

    def parse_stream(self, stream, **contextkw):
        context = Container(**contextkw)
        context._parsing = True
        context._building = False
        context._sizing = False
        context._params = context
        return self._parsereport(stream, context, "(parsing)")

    def parse_file(self, filename, **contextkw):
        with open(filename, "rb") as f:
            return self.parse_stream(f, **contextkw)

    def _parsereport(self, stream, context, path):
        obj = self._parse(stream, context, path)
        if self.parsed is not None:
            self.parsed(obj, context)
        return obj

    def _parse(self, stream, context, path):
        raise NotImplementedError

    def build(self, obj, **contextkw):
        stream = io.BytesIO()
        self.build_stream(obj, stream, **contextkw)
        return stream.getvalue()

    def build_stream(self, obj, stream, **contextkw):
        context = Container(**contextkw)
        context._parsing = False
        context._building = True
        context._sizing = False
        context._params = context
        self._build(obj, stream, context, "(building)")

    def build_file(self, obj, filename, **contextkw):
        with open(filename, "w+b") as f:
            self.build_stream(obj, f, **contextkw)

    def _build(self, obj, stream, context, path):
        raise NotImplementedError

    def sizeof(self, **contextkw):
        context = Container(**contextkw)
        context._parsing = False
        context._building = False
        context._sizing = True
        context._params = context
        return self._sizeof(context, "(sizeof)")

    def _sizeof(self, context, path):
        from . import SizeofError

        raise SizeofError(path=path)


def _filter_context(context):
    """Extract non-underscore context fields as a plain dict.

    Underscore-prefixed keys (``_``, ``_params``, ``_parsing``, etc.) are
    internal Python construct state and should not be passed to Rust-backed
    subcons via ``parse_stream(**kw)``.
    """
    return {
        k: v
        for k, v in dict.items(context)
        if not (isinstance(k, str) and k.startswith("_"))
    }


class Subconstruct(Construct):
    """Abstract subconstruct that wraps an inner construct.

    Parsing, building, and sizeof are by default deferred to ``subcon``.
    """

    def __init__(self, subcon):
        if not isinstance(subcon, Construct) and not _is_construct_like(subcon):
            raise TypeError("subcon should be a Construct field")
        super().__init__()
        self.subcon = subcon
        self.flagbuildnone = getattr(subcon, "flagbuildnone", False)

    def _parse(self, stream, context, path):
        if hasattr(self.subcon, "_parsereport"):
            return self.subcon._parsereport(stream, context, path)
        else:
            contextkw = _filter_context(context)
            return self.subcon.parse_stream(stream, **contextkw)

    def _build(self, obj, stream, context, path):
        if hasattr(self.subcon, "_build"):
            return self.subcon._build(obj, stream, context, path)
        else:
            contextkw = _filter_context(context)
            self.subcon.build_stream(obj, stream, **contextkw)
            return obj

    def _sizeof(self, context, path):
        if hasattr(self.subcon, "_sizeof"):
            return self.subcon._sizeof(context, path)
        else:
            contextkw = _filter_context(context)
            return self.subcon.sizeof(**contextkw)


class Adapter(Subconstruct):
    """Abstract adapter class.

    Needs to implement :meth:`_decode` for parsing and :meth:`_encode` for
    building.
    """

    def _parse(self, stream, context, path):
        if hasattr(self.subcon, "_parsereport"):
            obj = self.subcon._parsereport(stream, context, path)
        else:
            contextkw = _filter_context(context)
            obj = self.subcon.parse_stream(stream, **contextkw)
        return self._decode(obj, context, path)

    def _build(self, obj, stream, context, path):
        obj2 = self._encode(obj, context, path)
        if hasattr(self.subcon, "_build"):
            self.subcon._build(obj2, stream, context, path)
        else:
            contextkw = _filter_context(context)
            self.subcon.build_stream(obj2, stream, **contextkw)
        return obj

    def _decode(self, obj, context, path):
        raise NotImplementedError

    def _encode(self, obj, context, path):
        raise NotImplementedError


class SymmetricAdapter(Adapter):
    """Abstract adapter where ``_decode == _encode``.

    Needs to implement :meth:`_decode` only.
    """

    def _encode(self, obj, context, path):
        return self._decode(obj, context, path)


class Validator(SymmetricAdapter):
    """Abstract validator that checks a condition without transforming.

    Needs to implement :meth:`_validate` that returns a bool (or truthy value).
    """

    def _decode(self, obj, context, path):
        if not self._validate(obj, context, path):
            from . import ValidationError

            raise ValidationError("object failed validation: %s" % (obj,), path=path)
        return obj

    def _validate(self, obj, context, path):
        raise NotImplementedError


def _is_construct_like(obj):
    """Check if an object quacks like a Construct (duck typing).

    Used to accept both pure-Python Construct instances and Rust-backed
    PyO3 construct wrappers that expose ``parse_stream`` / ``build_stream``.
    """
    return hasattr(obj, "parse_stream") and hasattr(obj, "build_stream")
