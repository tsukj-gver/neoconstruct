"""ConstructMixin — dataclass-first binary format base class.

This module provides :class:`ConstructMixin`, a pure-Python mixin that
transforms a standard :mod:`dataclasses.dataclass` into a construct-compatible
binary format with ``parse`` / ``build`` / ``sizeof`` methods.

The mixin delegates to the Rust native extension (Phase 13 compiled path)
for the actual parse/build work, using the direct-to-Python
``CompiledSchemaHolder`` API exposed in Phase 14.1.

Phase 14.1 scope: flat dataclass (no nested dataclass fields). Parse uses
the MVP "scheme A" path (PyDict intermediate → ``cls(**dict)``).
"""

from typing import Any, ClassVar, Optional, TYPE_CHECKING

from ._core import Struct, compile_schema
from ._fields import _collect_field_subcons

if TYPE_CHECKING:
    from ._core import CompiledSchemaHolder


class ConstructMixin:
    """Mixin base for dataclass-first binary formats.

    Subclass with ``@dataclasses.dataclass`` and ``cs_field``-decorated fields::

        import dataclasses
        from construct_rust import ConstructMixin, cs_field, Int32ul, Bytes

        @dataclasses.dataclass
        class Header(ConstructMixin):
            magic: bytes = cs_field(Bytes(4))
            version: int = cs_field(Int32ul)

    Then use the class-level ``parse`` / instance-level ``build`` methods::

        header = Header.parse(b'\\x00\\x01\\x02\\x03\\x04\\x00\\x00\\x00')
        assert header.magic == b'\\x00\\x01\\x02\\x03'
        assert header.version == 4

        data = header.build()
        assert data == b'\\x00\\x01\\x02\\x03\\x04\\x00\\x00\\x00'
    """

    # Per-class compiled schema cache. Lazily initialized on first
    # parse/build/sizeof call. ``ClassVar`` prevents ``@dataclass`` from
    # treating this as a field.
    _compiled: "ClassVar[Optional[CompiledSchemaHolder]]" = None

    # ── Compilation ──────────────────────────────────────────────────────

    @classmethod
    def _compile(cls) -> "CompiledSchemaHolder":
        """Compiles this dataclass's fields into a ``CompiledSchemaHolder``.

        Scans :func:`dataclasses.fields`, collects ``cs_field`` subcons into a
        keyword dict, passes it to Rust ``Struct(**subcons_kw)`` (which
        internally calls ``extract_subcon`` on each value), then compiles via
        the ``compile_schema`` pyfunction. Result cached in ``cls._compiled``.

        Key design point (B-FEAS-1/B-FEAS-2): the subcon→Rust extraction
        happens **inside** ``Struct(**kwargs)`` on the Rust side. Python never
        calls ``extract_subcon`` (it returns a non-exposable Rust type).

        Returns:
            The cached :class:`CompiledSchemaHolder` for this class.
        """
        if cls._compiled is not None:
            return cls._compiled
        subcons_kw = _collect_field_subcons(cls)
        struct_decl = Struct(**subcons_kw)
        holder = compile_schema(struct_decl)
        cls._compiled = holder
        return holder

    # ── Parse (classmethods) ─────────────────────────────────────────────

    @classmethod
    def parse(cls, data: bytes, **kw: Any) -> "ConstructMixin":
        """Parse bytes into a dataclass instance.

        Args:
            data: Binary data to parse.
            **kw: Top-level context fields (matching ``**contextkw``).

        Returns:
            An instance of ``cls`` with fields populated from the parsed data.
        """
        holder = cls._compile()
        result_dict = holder.parse_bytes_py(data, **kw)
        return cls._dict_to_instance(result_dict)

    @classmethod
    def parse_stream(cls, stream: Any, **kw: Any) -> "ConstructMixin":
        """Parse from a file-like stream object.

        Reads all bytes from the stream, then delegates to :meth:`parse`.

        Args:
            stream: A file-like object with a ``read()`` method.
            **kw: Top-level context fields.
        """
        data = stream.read()
        return cls.parse(data, **kw)

    @classmethod
    def parse_file(cls, filename: str, **kw: Any) -> "ConstructMixin":
        """Parse from a file path.

        Args:
            filename: Path to a binary file.
            **kw: Top-level context fields.
        """
        with open(filename, "rb") as f:
            return cls.parse(f.read(), **kw)

    # ── Build (instance methods) ─────────────────────────────────────────

    def build(self, **kw: Any) -> bytes:
        """Build this dataclass instance into bytes.

        The instance is passed directly to the Rust ``build_from_py`` method,
        which reads attributes lazily via ``PyInput`` (no dict conversion).

        Args:
            **kw: Top-level context fields.

        Returns:
            The serialized bytes.
        """
        holder = type(self)._compile()
        return holder.build_from_py(self, **kw)

    def build_stream(self, stream: Any, **kw: Any) -> None:
        """Build and write to a file-like stream.

        Args:
            stream: A file-like object with a ``write()`` method.
            **kw: Top-level context fields.
        """
        data = self.build(**kw)
        stream.write(data)

    def build_file(self, filename: str, **kw: Any) -> None:
        """Build and write to a file path.

        Args:
            filename: Path to write the serialized bytes to.
            **kw: Top-level context fields.
        """
        with open(filename, "wb") as f:
            self.build_stream(f, **kw)

    # ── Sizeof ───────────────────────────────────────────────────────────

    @classmethod
    def sizeof(cls, **kw: Any) -> "Optional[int]":
        """Compute the static byte size of this format.

        Args:
            **kw: Accepted for API consistency; currently ignored (the
                compiled schema's static size is computed at compile time).

        Returns:
            The static byte size, or ``None`` if the size is not statically
            known (e.g. variable-length fields are present).
        """
        holder = cls._compile()
        return holder.sizeof(**kw)

    # ── dict → instance (MVP scheme A) ───────────────────────────────────

    @classmethod
    def _dict_to_instance(cls, result_dict: "dict[str, Any]") -> "ConstructMixin":
        """Converts a parsed dict into a dataclass instance (MVP scheme A).

        Filters keys based on :func:`dataclasses.fields` whitelist (I-COMP-2):
        only fields declared with ``cs_field`` are passed to ``cls(**kwargs)``.
        Extra keys from anonymous struct fields or internal cache entries are
        silently ignored.

        Phase 14.1: flat fields only. Phase 14.2 will add recursive nested
        dict → nested dataclass conversion.

        Args:
            result_dict: The dict returned by ``parse_bytes_py``.

        Returns:
            A ``cls`` instance with fields populated from ``result_dict``.
        """
        import dataclasses

        field_names = {f.name for f in dataclasses.fields(cls)}
        kwargs = {k: v for k, v in result_dict.items() if k in field_names}
        return cls(**kwargs)
