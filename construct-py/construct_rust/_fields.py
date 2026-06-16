"""cs_field — dataclass field descriptor for construct subcons.

This module provides :func:`cs_field`, the field declaration helper that
attaches a construct subcon to a :mod:`dataclasses` field via ``metadata``.
At compile time (see :mod:`construct_rust._mixins`), the subcon is extracted
from metadata and passed to the Rust ``Struct(**kwargs)`` factory.

Phase 14.1 scope: flat dataclass support.
Phase 14.2 scope: nested dataclass support via :func:`_resolve_nested_subcon`.
Phase 14.3 scope: forward references (``cs_field(None)``) — handled by the
caller (:meth:`ConstructMixin._build_struct_decl`).
"""

import dataclasses
import threading
from typing import Any, TYPE_CHECKING

if TYPE_CHECKING:
    from ._mixins import ConstructMixin

# Metadata key under which the construct subcon is stored.
_CS_SUBCON_KEY = "cs_subcon"

# Thread-local compiling stack used to detect circular references between
# nested dataclasses (``A → B → A``). See §8.2.1 of the design doc.
# Initialized lazily per-thread on first access via :func:`_get_compiling_stack`.
_compiling_stack: "threading.local" = threading.local()


def cs_field(subcon: Any, **field_kwargs: Any) -> Any:
    """Construct-aware dataclass field declaration.

    Wraps :func:`dataclasses.field`, attaching the construct subcon to the
    field's ``metadata``. The subcon is extracted at compile time by
    :meth:`ConstructMixin._compile <construct_rust._mixins.ConstructMixin._compile>`.

    Args:
        subcon: A construct subcon (e.g. ``Int32ul``, ``Bytes(4)``), another
            ``ConstructMixin`` subclass for nesting (Phase 14.2), or ``None``
            for a forward-reference field declared with a type annotation
            (Phase 14.3, e.g. ``next: "Optional[Node]" = cs_field(None)``).
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
        ``cs_field`` does **not** set a default value for the field unless
        ``default`` / ``default_factory`` is passed in ``field_kwargs``.
        ``cs_field(None)`` defers subcon resolution to the type annotation
        (see Phase 14.3 forward-reference handling).
    """
    metadata = dict(field_kwargs.pop("metadata", {}))
    metadata[_CS_SUBCON_KEY] = subcon
    return dataclasses.field(metadata=metadata, **field_kwargs)


def _get_field_subcon(field: "dataclasses.Field[Any]") -> Any:
    """Extracts the construct subcon from a dataclass field's metadata.

    Returns ``None`` if the field was declared with ``cs_field(None)`` (a
    forward-reference placeholder). The caller
    (:meth:`ConstructMixin._build_struct_decl`) resolves ``None`` to a real
    subcon using the field's type annotation.

    Args:
        field: A :class:`dataclasses.Field` instance.

    Returns:
        The construct subcon stored by :func:`cs_field`, or ``None`` for
        forward-reference fields.

    Raises:
        TypeError: If the field has no ``cs_field`` metadata at all (i.e. it
            was declared without ``cs_field()``). ``cs_field(None)`` is
            valid and distinct from "no cs_field".
    """
    if _CS_SUBCON_KEY not in field.metadata:
        raise TypeError(
            f"ConstructMixin field '{field.name}' must be declared with "
            f"cs_field(). Did you forget cs_field()?"
        )
    return field.metadata[_CS_SUBCON_KEY]


def _collect_field_subcons(cls: type) -> "dict[str, Any]":
    """Collects ``{field_name: raw_subcon}`` pairs from a ConstructMixin subclass.

    Returns the **raw** metadata subcon for each field — which may be ``None``
    for ``cs_field(None)`` forward-reference fields. Forward-reference and
    nested-dataclass resolution is performed by the caller
    (:meth:`ConstructMixin._build_struct_decl`), keeping this function a
    pure metadata reader (design N-2: unifies §3.2 / §8.3.2 signatures).

    Args:
        cls: A ``ConstructMixin`` subclass (must be a dataclass).

    Returns:
        A dict mapping field names to raw subcons (possibly ``None``).

    Raises:
        TypeError: If any field lacks ``cs_field`` metadata.
    """
    subcons_kw: "dict[str, Any]" = {}
    for f in dataclasses.fields(cls):
        subcons_kw[f.name] = _get_field_subcon(f)
    return subcons_kw


def _get_compiling_stack() -> "set[type]":
    """Returns the per-thread compiling stack, creating it if needed.

    Used by :meth:`ConstructMixin._build_struct_decl` for circular-reference
    detection between nested dataclasses.
    """
    stack = getattr(_compiling_stack, "classes", None)
    if stack is None:
        stack = set()
        _compiling_stack.classes = stack
    return stack


def _is_construct_mixin_subclass(obj: Any) -> bool:
    """Returns True if ``obj`` is a ConstructMixin subclass (not an instance).

    Imports :class:`ConstructMixin` lazily to avoid a circular import
    (``_mixins`` imports from ``_fields``).
    """
    if not isinstance(obj, type):
        return False
    from ._mixins import ConstructMixin

    return issubclass(obj, ConstructMixin)


def _resolve_nested_subcon(
    subcon: Any,
    owner_cls: type,
    field_name: str,
) -> Any:
    """Resolves a cs_field subcon for nesting inside a parent ``Struct(**kwargs)``.

    Returns a Python object suitable as a value in the kwargs dict.

    Three cases:

    (a) ``subcon`` is ``None`` (``cs_field(None)`` forward reference):
        raises :class:`TypeError`. The caller
        (:meth:`ConstructMixin._build_struct_decl`) must resolve
        forward references **before** invoking this function.
    (b) ``subcon`` is a ConstructMixin subclass (nested dataclass, declared
        directly via ``cs_field(OtherDataclass)``): recursively builds a
        PyStruct via ``subcon._build_struct_decl()``. Cycle detection uses
        the per-thread compiling stack — direct (non-Optional) mutual
        recursion ``A → B → A`` raises :class:`RecursionError`.
    (c) ``subcon`` is a regular PyO3 construct wrapper (Int32ul, Bytes(4),
        a pure-Python construct like :class:`Optional`, ...): returned
        as-is. ``py_struct`` calls ``extract_subcon`` on the Rust side.

    Args:
        subcon: The raw subcon from ``cs_field`` metadata.
        owner_cls: The ConstructMixin subclass owning the field (for error
            messages).
        field_name: The dataclass field name (for error messages).

    Returns:
        The resolved subcon (case b: a PyStruct; case c: the input as-is).

    Raises:
        TypeError: If ``subcon`` is ``None`` (caller bug — forward refs must
            be resolved first).
        RecursionError: If a circular nesting chain is detected (case b).
    """
    if subcon is None:
        raise TypeError(
            f"cs_field(None) on '{owner_cls.__name__}.{field_name}' cannot "
            f"be resolved as a nested subcon. Forward-reference fields must "
            f"have a type annotation (e.g. Optional[Node]) resolvable via "
            f"_build_struct_decl."
        )
    if _is_construct_mixin_subclass(subcon):
        # Case (b): nested dataclass — recursively build its PyStruct decl.
        # Cycle detection lives inside ``_build_struct_decl`` (it adds/removes
        # ``cls`` around the full field-resolution loop, so nested calls
        # observe the parent on the stack). We deliberately do NOT touch the
        # stack here — that would double-add and cause false positives.
        return subcon._build_struct_decl()
    # Case (c): regular construct wrapper — pass through.
    return subcon
