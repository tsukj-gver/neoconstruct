"""String constructs implemented in pure Python.

StringEncoded is an Adapter ported from construct.core.StringEncoded.
PascalString and GreedyString are macro functions that compose
StringEncoded with Prefixed/GreedyBytes (both Rust-backed PyO3 wrappers).

PaddedString callable-length fallback (_padded_string_fallback) is used
by the Rust py_padded_string factory when length is a context lambda.
"""

from io import BytesIO

from ._adapter import Adapter, Subconstruct
from ._core import StringError, FixedSized, GreedyBytes, Prefixed
from ._stream import NullStripped, _sub_parse, _sub_build


class StringEncoded(Adapter):
    """Used internally. Ported from construct.core.StringEncoded."""

    def __init__(self, subcon, encoding):
        super().__init__(subcon)
        if not encoding:
            raise StringError("String* classes require explicit encoding")
        self.encoding = encoding

    def _decode(self, obj, context, path):
        try:
            return obj.decode(self.encoding)
        except Exception:
            raise StringError(
                "cannot use encoding %r to decode %r" % (self.encoding, obj)
            )

    def _encode(self, obj, context, path):
        if not isinstance(obj, str):
            raise StringError("string encoding failed, expected unicode string")
        if obj == u"":
            return b""
        try:
            return obj.encode(self.encoding)
        except Exception:
            raise StringError(
                "cannot use encoding %r to encode %r" % (self.encoding, obj)
            )


def PascalString(lengthfield, encoding):
    """Length-prefixed string. Ported from construct.core.PascalString.

    Composes Prefixed(lengthfield, GreedyBytes) with StringEncoded for
    bytes<->str conversion.
    """
    return StringEncoded(Prefixed(lengthfield, GreedyBytes), encoding)


def GreedyString(encoding):
    """String that reads entire stream until EOF. Ported from construct.core.GreedyString."""
    return StringEncoded(GreedyBytes, encoding)


def _encoding_unit_size(encoding):
    """Returns the null-terminator unit size in bytes for the given encoding."""
    enc = encoding.replace("-", "_").lower()
    unit_sizes = {
        "ascii": 1,
        "utf8": 1, "utf_8": 1, "u8": 1,
        "utf16": 2, "utf_16": 2, "u16": 2,
        "utf_16_be": 2, "utf_16_le": 2,
        "utf32": 4, "utf_32": 4, "u32": 4,
        "utf_32_be": 4, "utf_32_le": 4,
    }
    if enc not in unit_sizes:
        raise StringError(
            "encoding %r not found among supported encodings" % encoding
        )
    return unit_sizes[enc]


class _CallableFixedSized(Subconstruct):
    """Internal: FixedSized that supports callable (context lambda) length.

    Used by _padded_string_fallback when PaddedString receives a callable
    length argument (PM decision: 方案 A).
    """

    def __init__(self, length, subcon):
        super().__init__(subcon)
        self.length = length

    def _parse(self, stream, context, path):
        from .expr import evaluate

        length = evaluate(self.length, context)
        data = stream.read(length)
        return _sub_parse(self.subcon, BytesIO(data), context, path)

    def _build(self, obj, stream, context, path):
        from .expr import evaluate

        length = evaluate(self.length, context)
        stream2 = BytesIO()
        ret = _sub_build(self.subcon, obj, stream2, context, path)
        data = stream2.getvalue()
        if len(data) > length:
            from . import PaddingError

            raise PaddingError(
                "encoded string is %d bytes but PaddedString length is %d"
                % (len(data), length)
            )
        data += b"\x00" * (length - len(data))
        stream.write(data)
        return ret

    def _sizeof(self, context, path):
        from .expr import evaluate

        try:
            return evaluate(self.length, context)
        except Exception:
            from . import SizeofError

            raise SizeofError("variable size: %s" % path)


def _padded_string_fallback(length, encoding):
    """Fallback for PaddedString when length is a callable (context lambda).

    Creates the equivalent pure-Python macro:
    StringEncoded(FixedSized(length, NullStripped(GreedyBytes, pad=unit)), encoding)
    """
    unit = bytes(_encoding_unit_size(encoding))
    return StringEncoded(
        _CallableFixedSized(length, NullStripped(GreedyBytes, pad=unit)),
        encoding,
    )
