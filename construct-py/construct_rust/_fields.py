"""cs_field — dataclass field descriptor for construct subcons.

This module provides :func:`cs_field`, the field declaration helper that
attaches a construct subcon to a :mod:`dataclasses` field via ``metadata``.
At compile time (see :mod:`construct_rust._mixins`), the subcon is extracted
from metadata and passed to the Rust ``Struct(**kwargs)`` factory.

Phase 14.1 scope: flat dataclass support only. Nested dataclass handling
(``_resolve_nested_subcon``) is deferred to Phase 14.2.
"""

import dataclasses
from typing import Any

# Metadata key under which the construct subcon is stored.
_CS_SUBCON_KEY = "cs_subcon"


def cs_field(subcon: Any, **field_kwargs: Any) -> Any:
    """Construct-aware dataclass field declaration.

    Wraps :func:`dataclasses.field`, attaching the construct subcon to the
    field's ``metadata``. The subcon is extracted at compile time by
    :meth:`ConstructMixin._compile <construct_rust._mixins.ConstructMixin._compile>`.

    Args:
        subcon: A construct subcon (e.g. ``Int32ul``, ``Bytes(4)``, or another
            ``ConstructMixin`` subclass for nesting — the latter requires
            Phase 14.2).
        **field_kwargs: Passed through to :func:`dataclasses.field` (e.g.
            ``default=``, ``default_factory=``, ``repr=False``).

    Returns:
        The return value of ``dataclasses.field(...)``, suitable as a
        dataclass field default.

    Example::

        @dataclass
        class Header(ConstructMixin):
            magic: bytes = cs_field(Bytes(4))
            version: int = cs_field(Int32ul, default=0)

    Note:
        ``cs_field`` does **not** set a default value for the field. The field
        is required unless ``default`` or ``default_factory`` is passed in
        ``field_kwargs``. This matches dataclass semantics: parse always
        produces all fields; build requires all non-default fields.
    """
    metadata = dict(field_kwargs.pop("metadata", {}))
    metadata[_CS_SUBCON_KEY] = subcon
    return dataclasses.field(metadata=metadata, **field_kwargs)


def _get_field_subcon(field: "dataclasses.Field[Any]") -> Any:
    """Extracts the construct subcon from a dataclass field's metadata.

    Called by :func:`_collect_field_subcons` during compilation.

    Args:
        field: A :class:`dataclasses.Field` instance.

    Returns:
        The construct subcon stored by :func:`cs_field`.

    Raises:
        TypeError: If the field has no ``cs_field`` metadata (i.e. it was
            declared without ``cs_field()``).
    """
    subcon = field.metadata.get(_CS_SUBCON_KEY)
    if subcon is None:
        raise TypeError(
            f"ConstructMixin field '{field.name}' must be declared with "
            f"cs_field(). Did you forget cs_field()?"
        )
    return subcon


def _collect_field_subcons(cls: type) -> "dict[str, Any]":
    """Collects ``{field_name: subcon}`` pairs from a ConstructMixin subclass.

    This is the single entry point used by
    :meth:`ConstructMixin._compile <construct_rust._mixins.ConstructMixin._compile>`.
    It scans :func:`dataclasses.fields`, extracts each ``cs_field`` subcon,
    and returns a dict suitable for ``Struct(**subcons_kw)``.

    Phase 14.1: returns subcons as-is (no nested dataclass resolution).
    Phase 14.2 will add ``_resolve_nested_subcon`` here.

    Args:
        cls: A ``ConstructMixin`` subclass (must be a dataclass).

    Returns:
        A dict mapping field names to construct subcons.

    Raises:
        TypeError: If any field lacks ``cs_field`` metadata.
    """
    subcons_kw: "dict[str, Any]" = {}
    for f in dataclasses.fields(cls):
        subcon = _get_field_subcon(f)
        subcons_kw[f.name] = subcon
    return subcons_kw
