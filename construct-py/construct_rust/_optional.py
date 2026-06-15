"""Pure-Python Optional construct.

Wraps a subconstruct and returns None if parsing fails (e.g. insufficient
bytes). Mirrors construct.core.Optional.
"""

import io


class Optional:
    """Conditional construct that returns None on parse failure.

    Parse tries the subcon; on any ConstructError, seeks back and returns None.
    Build produces empty bytes for None, otherwise delegates to subcon.
    Sizeof always raises SizeofError (variable consumption).
    """

    def __init__(self, subcon):
        self.subcon = subcon
        self.flagbuildnone = True

    def __repr__(self):
        return "<Optional %r>" % (self.subcon,)

    def __rtruediv__(self, name):
        if not isinstance(name, str):
            return NotImplemented
        from ._core import Renamed
        return Renamed(self, name)

    def parse(self, data, **kw):
        return self.parse_stream(io.BytesIO(data), **kw)

    def parse_stream(self, stream, **kw):
        pos = stream.tell()
        try:
            return self.subcon.parse_stream(stream, **kw)
        except Exception:
            stream.seek(pos)
            return None

    def parse_file(self, filename, **kw):
        with open(filename, "rb") as f:
            return self.parse_stream(f, **kw)

    def build(self, obj, **kw):
        if obj is None:
            return b""
        return self.subcon.build(obj, **kw)

    def build_stream(self, obj, stream, **kw):
        if obj is None:
            return
        return self.subcon.build_stream(obj, stream, **kw)

    def build_file(self, obj, filename, **kw):
        data = self.build(obj, **kw)
        with open(filename, "wb") as f:
            f.write(data)

    def sizeof(self, **kw):
        from ._core import SizeofError
        raise SizeofError("Optional has variable size")

    # Operator support
    def __mul__(self, other):
        return self.subcon * other

    def __rmul__(self, other):
        return other * self.subcon

    def __add__(self, other):
        return self.subcon + other

    def __rshift__(self, other):
        return self.subcon >> other

    def __getitem__(self, count):
        return self.subcon[count]
