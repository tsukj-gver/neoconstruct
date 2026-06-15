"""Pure-Python fallbacks for BytesInteger/BitsInteger with dynamic parameters.

Used when ``swapped`` is a context lambda (Path) or ``length`` is a callable.
The Rust-backed PyO3 versions only accept static ``length: usize`` and
``swapped: bool``, so these fallbacks handle the dynamic case by evaluating
the expression at parse/build time and delegating to the appropriate Rust
primitive.
"""

import io

from ._adapter import Construct, Subconstruct, _filter_context
from .lib.binary import integer2bits, bits2integer, integer2bytes, bytes2integer
from .expr import evaluate


def _swapbytesinbits(data):
    """Swap byte-chunks (of 8 bits) within a bit-string.

    Mirrors ``construct.lib.binary.swapbytesinbits``.
    """
    if len(data) % 8 != 0:
        raise ValueError(
            "little-endianness is only defined if data length %d is multiple of 8"
            % len(data)
        )
    return b"".join(data[i : i + 8] for i in reversed(range(0, len(data), 8)))


class _BytesIntegerExpr(Construct):
    """BytesInteger with dynamic length and/or swapped."""

    def __init__(self, length, signed, swapped):
        super().__init__()
        self.length = length
        self.signed = signed
        self.swapped = swapped

    def _parse(self, stream, context, path):
        from . import IntegerError, StreamError

        length = evaluate(self.length, context)
        if length <= 0:
            raise IntegerError("length %s must be positive in path %s" % (length, path))
        data = stream.read(length)
        if len(data) != length:
            raise StreamError(
                "stream read only %d bytes, expected %d in path %s"
                % (len(data), length, path)
            )
        if evaluate(self.swapped, context):
            data = data[::-1]
        try:
            return bytes2integer(data, self.signed)
        except ValueError as e:
            raise IntegerError(str(e) + " in path " + str(path))

    def _build(self, obj, stream, context, path):
        from . import IntegerError

        if not isinstance(obj, int):
            raise IntegerError("value %r is not an integer in path %s" % (obj, path))
        length = evaluate(self.length, context)
        if length <= 0:
            raise IntegerError("length %s must be positive in path %s" % (length, path))
        try:
            data = integer2bytes(obj, length, self.signed)
        except ValueError as e:
            raise IntegerError(str(e) + " in path " + str(path))
        if evaluate(self.swapped, context):
            data = data[::-1]
        stream.write(data)
        return obj

    def _sizeof(self, context, path):
        from . import SizeofError

        try:
            return evaluate(self.length, context)
        except Exception:
            raise SizeofError("cannot compute sizeof in path %s" % str(path))


class _BitsIntegerExpr(Construct):
    """BitsInteger with dynamic length and/or swapped."""

    def __init__(self, length, signed, swapped):
        super().__init__()
        self.length = length
        self.signed = signed
        self.swapped = swapped

    def _parse(self, stream, context, path):
        from . import IntegerError, StreamError

        length = evaluate(self.length, context)
        if length <= 0:
            raise IntegerError("length %s must be positive in path %s" % (length, path))
        data = stream.read(length)
        if len(data) != length:
            raise StreamError(
                "stream read only %d bits, expected %d in path %s"
                % (len(data), length, path)
            )
        if evaluate(self.swapped, context):
            data = _swapbytesinbits(data)
        try:
            return bits2integer(data, self.signed)
        except ValueError as e:
            raise IntegerError(str(e) + " in path " + str(path))

    def _build(self, obj, stream, context, path):
        from . import IntegerError

        if not isinstance(obj, int):
            raise IntegerError("value %r is not an integer in path %s" % (obj, path))
        length = evaluate(self.length, context)
        if length <= 0:
            raise IntegerError("length %s must be positive in path %s" % (length, path))
        try:
            data = integer2bits(obj, length, self.signed)
        except ValueError as e:
            raise IntegerError(str(e) + " in path " + str(path))
        if evaluate(self.swapped, context):
            data = _swapbytesinbits(data)
        stream.write(bytes(data))
        return obj

    def _sizeof(self, context, path):
        from . import SizeofError

        try:
            return evaluate(self.length, context)
        except Exception:
            raise SizeofError("cannot compute sizeof in path %s" % str(path))


def _bytes_integer_fallback(length, signed, swapped):
    return _BytesIntegerExpr(length, signed, swapped)


def _bits_integer_fallback(length, signed, swapped):
    return _BitsIntegerExpr(length, signed, swapped)
