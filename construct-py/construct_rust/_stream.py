"""Stream-processing constructs implemented in pure Python.

These constructs depend on Python runtime features (byte-at-a-time IO,
context lambda evaluation, itertools) or have no Rust equivalent.
They inherit from the Subconstruct base class (10.4/10.7) and use the
dual-path mechanism for correct delegation to nested Rust subcons.
"""

import itertools

from ._adapter import Construct, Subconstruct, _filter_context, _is_construct_like


def byte2int(x):
    """Return the integer value of a single-byte bytes object."""
    if isinstance(x, int):
        return x
    return x[0] if len(x) > 0 else 0


def _sub_parse(subcon, stream, context, path):
    """Dual-path parse delegation to a subcon.

    If the subcon is a pure-Python Construct (has ``_parsereport``), use the
    internal API directly so the same context is shared by reference.
    Otherwise (Rust-backed PyO3 construct), use the public ``parse_stream``
    API with non-underscore context fields passed as keyword arguments.
    """
    if hasattr(subcon, "_parsereport"):
        return subcon._parsereport(stream, context, path)
    else:
        contextkw = _filter_context(context)
        return subcon.parse_stream(stream, **contextkw)


def _sub_build(subcon, obj, stream, context, path):
    """Dual-path build delegation to a subcon.

    If the subcon is a pure-Python Construct (has ``_build``), use the
    internal API directly. Otherwise (Rust-backed PyO3 construct), use the
    public ``build_stream`` API with non-underscore context fields passed as
    keyword arguments.
    """
    if hasattr(subcon, "_build"):
        return subcon._build(obj, stream, context, path)
    else:
        contextkw = _filter_context(context)
        subcon.build_stream(obj, stream, **contextkw)
        return obj


class NullTerminated(Subconstruct):
    r"""Restricts parsing to bytes preceding a null byte.

    See construct.core.NullTerminated for full documentation.
    """

    def __init__(self, subcon, term=b"\x00", include=False, consume=True, require=True):
        super().__init__(subcon)
        self.term = term
        self.include = include
        self.consume = consume
        self.require = require

    def _parse(self, stream, context, path):
        term = self.term
        unit = len(term)
        if unit < 1:
            from . import PaddingError

            raise PaddingError(
                "NullTerminated term must be at least 1 byte in path %s" % (path,)
            )
        data = b""
        while True:
            try:
                b = stream.read(unit)
                if len(b) < unit:
                    raise EOFError()
            except Exception:
                if self.require:
                    from . import StreamError

                    raise StreamError(
                        "stream read less than specified amount in path %s" % (path,)
                    )
                break
            if b == term:
                if self.include:
                    data += b
                if not self.consume:
                    stream.seek(-unit, 1)
                break
            data += b
        from io import BytesIO

        return _sub_parse(self.subcon, BytesIO(data), context, path)

    def _build(self, obj, stream, context, path):
        buildret = _sub_build(self.subcon, obj, stream, context, path)
        stream.write(self.term)
        return buildret

    def _sizeof(self, context, path):
        from . import SizeofError

        raise SizeofError("variable size: %s" % path)


class NullStripped(Subconstruct):
    r"""Restricts parsing to bytes except padding left of EOF."""

    def __init__(self, subcon, pad=b"\x00"):
        super().__init__(subcon)
        self.pad = pad

    def _parse(self, stream, context, path):
        pad = self.pad
        unit = len(pad)
        if unit < 1:
            from . import PaddingError

            raise PaddingError(
                "NullStripped pad must be at least 1 byte in path %s" % (path,)
            )
        data = stream.read()
        if unit == 1:
            data = data.rstrip(pad)
        else:
            tailunit = len(data) % unit
            end = len(data)
            if tailunit and data[-tailunit:] == pad[:tailunit]:
                end -= tailunit
            while end - unit >= 0 and data[end - unit : end] == pad:
                end -= unit
            data = data[:end]
        from io import BytesIO

        return _sub_parse(self.subcon, BytesIO(data), context, path)

    def _build(self, obj, stream, context, path):
        return _sub_build(self.subcon, obj, stream, context, path)

    def _sizeof(self, context, path):
        from . import SizeofError

        raise SizeofError("variable size: %s" % path)


class RestreamData(Subconstruct):
    r"""Parses a field on external data (does not build)."""

    def __init__(self, datafunc, subcon):
        if not _is_construct_like(subcon):
            raise TypeError("subcon should be a Construct field")
        super().__init__(subcon)
        self.datafunc = datafunc
        self.flagbuildnone = True

    def _parse(self, stream, context, path):
        from io import BytesIO

        from .expr import evaluate

        data = evaluate(self.datafunc, context)
        if isinstance(data, bytes):
            stream2 = BytesIO(data)
        elif isinstance(data, BytesIO):
            stream2 = data
        elif isinstance(data, Construct) or _is_construct_like(data):
            stream2 = BytesIO(_sub_parse(data, stream, context, path))
        else:
            from . import StreamError

            raise StreamError(
                "RestreamData datafunc returned unsupported type in path %s" % (path,)
            )
        return _sub_parse(self.subcon, stream2, context, path)

    def _build(self, obj, stream, context, path):
        return obj

    def _sizeof(self, context, path):
        return 0


class ProcessXor(Subconstruct):
    r"""XOR-transforms bytes between the underlying stream and subcon."""

    def __init__(self, padfunc, subcon):
        super().__init__(subcon)
        self.padfunc = padfunc

    def _parse(self, stream, context, path):
        from io import BytesIO

        from .expr import evaluate

        pad = evaluate(self.padfunc, context)
        if not isinstance(pad, (int, bytes)):
            from . import StringError

            raise StringError("ProcessXor needs integer or bytes pad in path " + str(path))
        if isinstance(pad, bytes) and len(pad) == 1:
            pad = byte2int(pad)
        data = stream.read()
        if isinstance(pad, int):
            if pad != 0:
                data = bytes(b ^ pad for b in data)
        elif isinstance(pad, bytes):
            if not (len(pad) <= 64 and pad == bytes(len(pad))):
                data = bytes(
                    b ^ p for b, p in zip(data, itertools.cycle(pad))
                )
        # Known limitation: Python original uses BytesIOWithOffsets to
        # preserve absolute offset information for the subcon. The pure-Python
        # shim uses plain BytesIO which resets offsets to 0. This does not
        # affect test_core.py scenarios but may impact constructs relying on
        # absolute stream offsets (Pointer, etc.) nested inside ProcessXor.
        return _sub_parse(self.subcon, BytesIO(data), context, path)

    def _build(self, obj, stream, context, path):
        from io import BytesIO

        from .expr import evaluate

        pad = evaluate(self.padfunc, context)
        if not isinstance(pad, (int, bytes)):
            from . import StringError

            raise StringError("ProcessXor needs integer or bytes pad in path " + str(path))
        if isinstance(pad, bytes) and len(pad) == 1:
            pad = byte2int(pad)
        stream2 = BytesIO()
        buildret = _sub_build(self.subcon, obj, stream2, context, path)
        data = stream2.getvalue()
        if isinstance(pad, int):
            if pad != 0:
                data = bytes(b ^ pad for b in data)
        elif isinstance(pad, bytes):
            if not (len(pad) <= 64 and pad == bytes(len(pad))):
                data = bytes(
                    b ^ p for b, p in zip(data, itertools.cycle(pad))
                )
        stream.write(data)
        return buildret

    def _sizeof(self, context, path):
        if hasattr(self.subcon, '_sizeof'):
            return self.subcon._sizeof(context, path)
        return self.subcon.sizeof()


class ProcessRotateLeft(Subconstruct):
    r"""Rotates (shifts) bytes left by amount in bits, within group-sized chunks."""

    _precomputed_single_rotations = {
        amount: [(i << amount) & 0xFF | (i >> (8 - amount)) for i in range(256)]
        for amount in range(1, 8)
    }

    def __init__(self, amount, group, subcon):
        super().__init__(subcon)
        self.amount = amount
        self.group = group

    def _rotate(self, data, amount, group, path):
        if group < 1:
            from . import RotationError

            raise RotationError(
                "group size must be at least 1 to be valid in path %s" % (path,)
            )
        amount = amount % (group * 8)
        amount_bytes = amount // 8
        if len(data) % group != 0:
            from . import RotationError

            raise RotationError(
                "data length must be a multiple of group size in path %s" % (path,)
            )
        if amount == 0:
            return data
        elif group == 1:
            translate = self._precomputed_single_rotations[amount]
            return bytes(translate[a] for a in data)
        elif amount % 8 == 0:
            indices = [(i + amount_bytes) % group for i in range(group)]
            return bytes(
                data[i + k]
                for i in range(0, len(data), group)
                for k in indices
            )
        else:
            amount1 = amount % 8
            amount2 = 8 - amount1
            indices_pairs = [
                ((i + amount_bytes) % group, (i + 1 + amount_bytes) % group)
                for i in range(group)
            ]
            return bytes(
                (data[i + k1] << amount1) & 0xFF | (data[i + k2] >> amount2)
                for i in range(0, len(data), group)
                for k1, k2 in indices_pairs
            )

    def _parse(self, stream, context, path):
        from io import BytesIO

        from .expr import evaluate

        amount = evaluate(self.amount, context)
        group = evaluate(self.group, context)
        data = stream.read()
        data = self._rotate(data, amount, group, path)
        # Known limitation: same as ProcessXor — plain BytesIO instead of
        # BytesIOWithOffsets (see ProcessXor._parse comment above).
        return _sub_parse(self.subcon, BytesIO(data), context, path)

    def _build(self, obj, stream, context, path):
        from io import BytesIO

        from .expr import evaluate

        amount = evaluate(self.amount, context)
        group = evaluate(self.group, context)
        stream2 = BytesIO()
        buildret = _sub_build(self.subcon, obj, stream2, context, path)
        data = stream2.getvalue()
        data = self._rotate(data, -amount % (group * 8), group, path)
        stream.write(data)
        return buildret

    def _sizeof(self, context, path):
        if hasattr(self.subcon, '_sizeof'):
            return self.subcon._sizeof(context, path)
        return self.subcon.sizeof()


class OffsettedEnd(Subconstruct):
    r"""Parses all bytes till EOF plus a negative endoffset is reached."""

    def __init__(self, endoffset, subcon):
        super().__init__(subcon)
        self.endoffset = endoffset

    def _parse(self, stream, context, path):
        from io import BytesIO

        from .expr import evaluate

        endoffset = evaluate(self.endoffset, context)
        curpos = stream.tell()
        stream.seek(0, 2)
        endpos = stream.tell()
        stream.seek(curpos, 0)
        length = endpos + endoffset - curpos
        data = stream.read(length)
        return _sub_parse(self.subcon, BytesIO(data), context, path)

    def _build(self, obj, stream, context, path):
        return _sub_build(self.subcon, obj, stream, context, path)

    def _sizeof(self, context, path):
        from . import SizeofError

        raise SizeofError("variable size: %s" % path)
