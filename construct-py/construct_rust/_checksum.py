"""Pure-Python Checksum implementation for the Python bridge.

The Rust Checksum is designed for bytes-based checksum fields (Bytes(N)),
but Python Checksum supports arbitrary types (Byte, Int, etc.) where
bytesfunc returns any value and hashfunc processes it.

This Python implementation preserves the full flexibility.
"""

import io

from ._adapter import Construct, _filter_context
from .lib.containers import Container
from .expr import evaluate


class _Checksum(Construct):
    """Python-side Checksum that preserves the bytesfunc → hashfunc pipeline."""

    def __init__(self, checksumfield, hashfunc, bytesfunc):
        super().__init__()
        self.checksumfield = checksumfield
        self.hashfunc = hashfunc
        self.bytesfunc = bytesfunc
        self.flagbuildnone = True

    def _parse(self, stream, context, path):
        from . import ChecksumError

        if hasattr(self.checksumfield, "_parsereport"):
            hash1 = self.checksumfield._parsereport(stream, context, path)
        else:
            kw = _filter_context(context)
            hash1 = self.checksumfield.parse_stream(stream, **kw)
        bytesval = evaluate(self.bytesfunc, context)
        hash2 = self.hashfunc(bytesval)
        if hash1 != hash2:
            import binascii
            hex1 = binascii.hexlify(hash1) if isinstance(hash1, (bytes, bytearray)) else hash1
            hex2 = binascii.hexlify(hash2) if isinstance(hash2, (bytes, bytearray)) else hash2
            raise ChecksumError(
                "wrong checksum, read %r, computed %r in path %s" % (hex1, hex2, path)
            )
        return hash1

    def _build(self, obj, stream, context, path):
        bytesval = evaluate(self.bytesfunc, context)
        hash2 = self.hashfunc(bytesval)
        if hasattr(self.checksumfield, "_build"):
            self.checksumfield._build(hash2, stream, context, path)
        else:
            kw = _filter_context(context)
            self.checksumfield.build_stream(hash2, stream, **kw)

    def _sizeof(self, context, path):
        from . import SizeofError

        try:
            kw = _filter_context(context)
            return self.checksumfield.sizeof(**kw)
        except Exception:
            raise SizeofError("cannot compute sizeof in path %s" % str(path))
