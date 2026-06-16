"""ConstructMixin — dataclass-first binary format base class.

This module provides :class:`ConstructMixin`, a pure-Python mixin that
transforms a standard :mod:`dataclasses.dataclass` into a construct-compatible
binary format with ``parse`` / ``build`` / ``sizeof`` methods.

The mixin delegates to the Rust native extension (Phase 13 compiled path)
for the actual parse/build work, using the direct-to-Python
``CompiledSchemaHolder`` API exposed in Phase 14.1.

Phase 14.1 scope: flat dataclass (no nested dataclass fields). Parse uses
the MVP "scheme A" path (PyDict intermediate → ``cls(**dict)``).

Phase 14.2 scope: nested dataclass support via :meth:`_build_struct_decl`
recursion + :meth:`_dict_to_instance` recursive dict→instance conversion.

Phase 14.3 scope: forward references (``cs_field(None)`` with string /
``Optional[T]`` annotations), Optional field auto-wrapping, circular
reference detection.
"""

import dataclasses
import sys
import types
import typing
from typing import TYPE_CHECKING, Any, ClassVar, Optional

from ._core import Struct, compile_schema
from ._fields import (
    _collect_field_subcons,
    _get_compiling_stack,
    _is_construct_mixin_subclass,
    _resolve_nested_subcon,
)

if TYPE_CHECKING:
    from ._core import CompiledSchemaHolder


# Sentinel value used to distinguish "cache miss" from cached ``None``.
# (``None`` is a valid type-hints result for empty dataclasses.)
_HINTS_UNSET: Any = object()


def _is_union_origin(origin: Any) -> bool:
    """Returns ``True`` if *origin* is a Union origin type.

    Supports both :data:`typing.Union` (the classic ``Union[X, None]`` /
    ``Optional[X]`` form) and :class:`types.UnionType` (the PEP 604
    ``X | None`` syntax, available on Python 3.10+).

    Used by :meth:`ConstructMixin._maybe_wrap_optional`,
    :meth:`ConstructMixin._subcon_from_annotation`, and
    :func:`_coerce_value_to_dataclass` to consistently detect Optional
    types regardless of which Union syntax the user employed.
    """
    if origin is typing.Union:
        return True
    union_type = getattr(types, "UnionType", None)
    return union_type is not None and origin is union_type


class _DualMethod:
    """Descriptor: a method callable both as ``instance.method(*args)`` and
    ``Class.method(obj, *args)``.

    Phase 14.2/14.3: enables dataclass subclasses to be embedded directly
    inside pure-Python construct wrappers (``Array``, ``PrefixedArray``,
    ``Optional``) whose ``build_stream`` may be invoked with a
    :class:`Container` dict (produced by the parent's ``as_value`` Container
    conversion) rather than a real dataclass instance.

    - ``instance.build_stream(stream)`` → ``fn(cls, instance, stream)``
    - ``Class.build_stream(dict_obj, stream)`` → ``fn(cls, dict_obj, stream)``

    The wrapped ``fn`` receives the owning class as its first argument so it
    can dispatch on whether the input is a dict (needs ``_dict_to_instance``
    coercion) or a real instance.
    """

    def __init__(self, fn: Any) -> None:
        self.fn = fn
        self.__doc__ = fn.__doc__
        self.__name__ = getattr(fn, "__name__", "_dual_method")

    def __set_name__(self, owner: type, name: str) -> None:
        self.__name__ = name

    def __get__(self, instance: Any, owner: type) -> Any:
        fn = self.fn
        if instance is None:
            # Class access: returned callable expects (obj, *args, **kw).
            def class_call(obj: Any, *args: Any, **kw: Any) -> Any:
                return fn(owner, obj, *args, **kw)

            class_call.__name__ = self.__name__
            class_call.__doc__ = self.__doc__
            return class_call
        # Instance access: returned callable expects (*args, **kw),
        # with `instance` bound as the obj argument.
        def instance_call(*args: Any, **kw: Any) -> Any:
            return fn(owner, instance, *args, **kw)

        instance_call.__name__ = self.__name__
        instance_call.__doc__ = self.__doc__
        return instance_call


# Default subcons for primitive type annotations (used by forward-reference
# resolution when no explicit subcon is given). These mirror the defaults
# from the design doc §8.3.2.
_DEFAULT_SUBCONS: "dict[type, Any]" = {}


def _init_default_subcons() -> None:
    """Populates :data:`_DEFAULT_SUBCONS` lazily (avoids import-time cost)."""
    if _DEFAULT_SUBCONS:
        return
    from ._core import (
        Flag,
        Float64l,
        GreedyBytes,
        Int32ul,
        CString,
    )

    _DEFAULT_SUBCONS[int] = Int32ul
    _DEFAULT_SUBCONS[bool] = Flag
    _DEFAULT_SUBCONS[float] = Float64l
    _DEFAULT_SUBCONS[str] = CString("utf8")
    # bytes → GreedyBytes (variable-length; reads to end of stream).
    _DEFAULT_SUBCONS[bytes] = GreedyBytes


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

    Phase 14.2+ also supports nested dataclasses::

        @dataclasses.dataclass
        class FileFormat(ConstructMixin):
            header: Header = cs_field(Header)            # nested dataclass
            data: bytes = cs_field(Bytes(this.header.size))

    and forward references (Phase 14.3)::

        @dataclasses.dataclass
        class Node(ConstructMixin):
            value: int = cs_field(Int32ul)
            next: "Optional[Node]" = cs_field(None)      # forward reference
    """

    # Per-class compiled schema cache. Lazily initialized on first
    # parse/build/sizeof call. ``ClassVar`` prevents ``@dataclass`` from
    # treating this as a field.
    _compiled: "ClassVar[Optional[CompiledSchemaHolder]]" = None

    # Per-class cached type hints (resolved via ``typing.get_type_hints``).
    # Used by both ``_build_struct_decl`` (forward-ref resolution) and
    # ``_dict_to_instance`` (recursive dict→instance conversion).
    # ``_HINTS_UNSET`` sentinel distinguishes "not yet computed" from
    # cached ``None`` / empty dict (S-PERF-1: avoid recomputing per parse).
    _cached_type_hints: "ClassVar[Any]" = _HINTS_UNSET

    # ── Compilation ──────────────────────────────────────────────────────

    @classmethod
    def _get_type_hints(cls) -> "dict[str, Any]":
        """Resolves and caches type hints for this class.

        Uses :func:`typing.get_type_hints` with explicit ``globalns`` from
        the class's defining module (I-COMP-1 fix: handles
        ``from __future__ import annotations`` correctly by resolving string
        annotations against module globals).

        Two-stage fallback:

        1. Try ``typing.get_type_hints(cls, globalns, localns)`` with a
           ``localns`` stub for ``CompiledSchemaHolder`` (which is imported
           only under ``TYPE_CHECKING`` and thus absent at runtime). Filter
           the result down to dataclass-field names.
        2. If stage 1 fails (e.g. another TYPE_CHECKING-only annotation),
           resolve each dataclass field's annotation individually via
           :meth:`_resolve_field_hints_individually`.

        Returns:
            A dict mapping field names to resolved type objects.
        """
        if cls._cached_type_hints is not _HINTS_UNSET:
            return cls._cached_type_hints
        module = sys.modules.get(getattr(cls, "__module__", None))
        globalns = getattr(module, "__dict__", {}) if module else {}
        # Stage 1: full typing.get_type_hints with a localns stub.
        # localns provides placeholders for names referenced only under
        # TYPE_CHECKING (CompiledSchemaHolder) and common typing aliases
        # (ClassVar, Optional, ...) that may be needed to resolve the
        # mixin's own ClassVar annotations (e.g. `_compiled`).
        localns: "dict[str, Any]" = {
            "CompiledSchemaHolder": object,
            "ClassVar": typing.ClassVar,
            "Optional": typing.Optional,
            "Union": typing.Union,
            "List": typing.List,
            "Dict": typing.Dict,
            "Tuple": typing.Tuple,
            "Any": typing.Any,
        }
        field_names = {f.name for f in dataclasses.fields(cls)}
        try:
            full_hints = typing.get_type_hints(cls, globalns=globalns, localns=localns)
            hints = {k: v for k, v in full_hints.items() if k in field_names}
        except Exception:
            # Stage 2: per-field fallback.
            hints = cls._resolve_field_hints_individually(globalns, field_names)
        # Cache on the class itself (per-class, not per-instance).
        cls._cached_type_hints = hints
        return hints

    @classmethod
    def _resolve_field_hints_individually(
        cls,
        globalns: "dict[str, Any]",
        field_names: "set[str]",
    ) -> "dict[str, Any]":
        """Per-field annotation resolution fallback.

        Walks ``cls.__mro__`` to collect inherited ``__annotations__``,
        then for each dataclass field, evaluates string annotations using
        ``globalns`` augmented with common :mod:`typing` aliases.
        Non-string annotations are kept as-is.

        Args:
            globalns: Module globals from ``cls.__module__``.
            field_names: Set of dataclass field names to resolve.

        Returns:
            A dict mapping each field name to its resolved (or raw) type.
        """
        # Build eval globals: module globals + common typing aliases.
        eval_globals: "dict[str, Any]" = dict(globalns) if globalns else {}
        for name in ("Optional", "Union", "List", "Dict", "Tuple", "Any"):
            eval_globals.setdefault(name, getattr(typing, name))
        # Collect annotations from all classes in the MRO (dataclass fields
        # may be inherited).
        annotations: "dict[str, Any]" = {}
        for klass in reversed(cls.__mro__):
            annotations.update(getattr(klass, "__annotations__", {}))
        hints: "dict[str, Any]" = {}
        for fname in field_names:
            raw = annotations.get(fname)
            if raw is None:
                # Use dataclasses field's .type attribute as last resort.
                for f in dataclasses.fields(cls):
                    if f.name == fname:
                        raw = f.type
                        break
            if isinstance(raw, str):
                try:
                    resolved = eval(raw, eval_globals, None)  # noqa: S307
                except Exception:
                    # Leave as string; caller (_resolve_forward_ref /
                    # _coerce_value_to_dataclass) will raise informatively.
                    resolved = raw
            else:
                resolved = raw
            hints[fname] = resolved
        return hints

    @classmethod
    def _build_struct_decl(cls) -> Any:
        """Builds a PyStruct declaration (not yet compiled) from this class.

        Used in two contexts:

        1. :meth:`_compile` calls this to build the top-level struct, then
           compiles it via :func:`compile_schema`.
        2. When this class is nested inside another ConstructMixin
           subclass (``cs_field(OtherDataclass)``), the parent's
           :func:`_resolve_nested_subcon` calls this to build the nested
           PyStruct decl, which is then passed as a value in the parent's
           ``Struct(**kwargs)`` dict.

        This method handles:

        - **Forward references** (``cs_field(None)``): resolves the field's
          type annotation to a subcon via :meth:`_resolve_forward_ref`.
        - **Optional wrapping** (field type is ``Optional[X]`` but subcon
          is a bare construct): wraps the subcon in :class:`Optional`.
        - **Nested dataclasses** (subcon is a ConstructMixin subclass):
          delegates to :func:`_resolve_nested_subcon` for recursive
          PyStruct construction with cycle detection.

        Cycle detection: this class is added to the per-thread compiling
        stack on entry; a re-entrant call for the same class raises
        :class:`RecursionError` (direct mutual recursion ``A → B → A``).

        Returns:
            A ``PyStruct`` instance (Rust-backed Python object). The parent
            ``Struct(**kwargs)`` factory will apply ``apply_rename`` and
            ``extract_subcon`` on the Rust side.

        Raises:
            RecursionError: If a circular nesting chain is detected.
            TypeError: If a forward reference cannot be resolved.
        """
        stack = _get_compiling_stack()
        if cls in stack:
            chain = " -> ".join(c.__name__ for c in stack) + f" -> {cls.__name__}"
            raise RecursionError(
                f"Circular nested dataclass reference detected: {chain}. "
                f"Use cs_field(None) with an Optional[{cls.__name__}] "
                f"annotation for recursive structures (see §8.3)."
            )
        stack.add(cls)
        try:
            raw_subcons = _collect_field_subcons(cls)
            resolved_kw: "dict[str, Any]" = {}
            for name, subcon in raw_subcons.items():
                if subcon is None:
                    # Forward reference — resolve from type annotation.
                    subcon = cls._resolve_forward_ref(name)
                else:
                    # Auto-wrap Optional if annotation demands it.
                    subcon = cls._maybe_wrap_optional(name, subcon)
                resolved_kw[name] = _resolve_nested_subcon(subcon, cls, name)
            return Struct(**resolved_kw)
        finally:
            stack.discard(cls)

    @classmethod
    def _compile(cls) -> "CompiledSchemaHolder":
        """Compiles this dataclass's fields into a ``CompiledSchemaHolder``.

        Calls :meth:`_build_struct_decl` to assemble the PyStruct declaration
        (resolving forward references and nested dataclasses), then compiles
        via :func:`compile_schema`. Result cached in ``cls._compiled``.

        Key design point (B-FEAS-1/B-FEAS-2): subcon→Rust extraction happens
        **inside** ``Struct(**kwargs)`` on the Rust side (via
        ``extract_subcon``). Python never calls ``extract_subcon`` directly.

        Returns:
            The cached :class:`CompiledSchemaHolder` for this class.
        """
        if cls._compiled is not None:
            return cls._compiled
        struct_decl = cls._build_struct_decl()
        holder = compile_schema(struct_decl)
        cls._compiled = holder
        return holder

    # ── Forward-reference resolution (Phase 14.3) ────────────────────────

    @classmethod
    def _resolve_forward_ref(cls, field_name: str) -> Any:
        """Resolves a ``cs_field(None)`` field from its type annotation.

        Supported annotation forms (see design §8.3):

        - ``ConstructMixin`` subclass (bare ``T``): returned as-is. The
          caller (:func:`_resolve_nested_subcon`) handles building the
          nested PyStruct decl.
        - ``Optional[T]`` / ``Union[T, None]``: wraps the resolved ``T``
          subcon in :class:`Optional`. The inner ``T`` may itself be a
          ConstructMixin subclass — in that case it is **kept as a class
          reference** (not pre-compiled) so recursive structures (linked
          lists, trees) work without triggering cycle detection.
        - Primitive types (``int``, ``bytes``, ``str``, ``float``, ``bool``):
          mapped to default subcons (see :data:`_DEFAULT_SUBCONS`).
        - ``List[T]`` where ``T`` is a ConstructMixin subclass: raises
          :class:`TypeError` (length cannot be inferred from annotation;
          use explicit ``cs_field(Array(n, T))``).

        Args:
            field_name: The dataclass field name (with ``cs_field(None)``).

        Returns:
            A resolved subcon (Python object suitable for ``Struct(**kwargs)``).

        Raises:
            TypeError: If the annotation is missing, unresolved, or
                unsupported.
        """
        hints = cls._get_type_hints()
        ftype = hints.get(field_name)
        if ftype is None:
            raise TypeError(
                f"cs_field(None) on '{cls.__name__}.{field_name}' requires a "
                f"type annotation. Add an annotation like ': SomeDataclass' "
                f"or ': Optional[SomeDataclass]', or pass an explicit subcon "
                f"to cs_field()."
            )
        return cls._subcon_from_annotation(ftype, field_name)

    @classmethod
    def _subcon_from_annotation(cls, ftype: Any, field_name: str) -> Any:
        """Builds a subcon from a (possibly Optional-wrapped) type annotation.

        See :meth:`_resolve_forward_ref` for supported forms.
        """
        origin = typing.get_origin(ftype)
        args = typing.get_args(ftype)
        # Optional[T] is Union[T, None] (or Union[T1, T2, ..., None]).
        # Python 3.10+ also supports `T | None` (types.UnionType).
        # _is_union_origin handles both forms consistently (M1 fix).
        if _is_union_origin(origin):
            non_none = [a for a in args if a is not type(None)]  # noqa: E721
            if len(non_none) == len(args):
                # Union without None — not supported, fall through to bare type.
                pass
            elif len(non_none) == 1:
                inner_subcon = cls._bare_subcon_from_type(non_none[0], field_name)
                # Wrap in construct Optional: allows parse/build to skip
                # the field when the value is None. For ConstructMixin
                # inner types, we keep them as class references (lazy)
                # rather than pre-compiling — this is what makes linked
                # lists / trees work without cycle-detection failures.
                from ._optional import Optional as PyOptional

                return PyOptional(inner_subcon)
            else:
                # Multi-type Union (e.g. Union[int, str, None]) — unsupported.
                raise TypeError(
                    f"Field '{cls.__name__}.{field_name}' has multi-type "
                    f"Union annotation {ftype!r}; only Optional[T] "
                    f"(Union[T, None]) is supported. Use cs_field() with "
                    f"an explicit subcon instead."
                )
        # Non-Optional annotation.
        return cls._bare_subcon_from_type(ftype, field_name)

    @classmethod
    def _bare_subcon_from_type(cls, ftype: Any, field_name: str) -> Any:
        """Maps a non-Optional type annotation to a subcon.

        - ConstructMixin subclass → wrapped in :class:`_DataclassSubcon`
          (lazy adapter). The wrapper is necessary so that the class can
          be embedded inside a pure-Python construct (Optional / Array)
          whose ``build_stream`` may be called with a :class:`Container`
          dict (produced by the parent's ``as_value`` conversion) rather
          than a real dataclass instance.
        - :class:`typing.ForwardRef` → resolved against module globals
          (e.g. ``Optional["Node"]`` string annotations).
        - ``int``/``bool``/``float``/``str``/``bytes`` → default subcon.
        - ``List[T]`` → raises (length cannot be inferred).
        - Anything else → raises.
        """
        _init_default_subcons()
        # Unwrap ForwardRef (e.g. from Optional["Node"] string annotations).
        # typing.get_type_hints normally resolves these, but under certain
        # fallback paths they may survive as ForwardRef instances.
        if isinstance(ftype, typing.ForwardRef):
            ftype = cls._resolve_forwardref_value(ftype, field_name)
        if _is_construct_mixin_subclass(ftype):
            # Wrap so Optional/Array/etc. can pass Container dicts in.
            return _DataclassSubcon(ftype)
        if isinstance(ftype, type) and ftype in _DEFAULT_SUBCONS:
            return _DEFAULT_SUBCONS[ftype]
        origin = typing.get_origin(ftype)
        if origin in (list, typing.List):
            raise TypeError(
                f"Cannot infer Array length for '{cls.__name__}.{field_name}' "
                f"annotation {ftype!r}. Provide an explicit subcon: "
                f"cs_field(Array(n, ItemClass)) or "
                f"cs_field(PrefixedArray(Int32ul, ItemClass))."
            )
        raise TypeError(
            f"Cannot infer subcon from annotation {ftype!r} for "
            f"'{cls.__name__}.{field_name}'. Pass an explicit subcon "
            f"to cs_field()."
        )

    @classmethod
    def _resolve_forwardref_value(cls, fref: Any, field_name: str) -> Any:
        """Resolves a :class:`typing.ForwardRef` against module globals.

        Args:
            fref: A :class:`typing.ForwardRef` instance.
            field_name: Field name (for error messages).

        Returns:
            The resolved type object.

        Raises:
            TypeError: If the forward reference cannot be resolved.
        """
        arg = getattr(fref, "__forward_arg__", None) or str(fref)
        module = sys.modules.get(getattr(cls, "__module__", None))
        globalns = getattr(module, "__dict__", {}) if module else {}
        try:
            return eval(arg, globalns, None)  # noqa: S307
        except Exception as e:  # noqa: BLE001
            raise TypeError(
                f"Cannot resolve forward reference {arg!r} for "
                f"'{cls.__name__}.{field_name}': {e}. Ensure the referenced "
                f"class is defined in the same module or imported."
            ) from e

    @classmethod
    def _maybe_wrap_optional(cls, field_name: str, subcon: Any) -> Any:
        """Auto-wraps ``subcon`` in :class:`Optional` if needed.

        Triggered when the field has an explicit subcon (not ``None``) AND
        its type annotation is ``Optional[X]`` AND the subcon does not
        already look like an Optional wrapper. This makes the following
        work as expected::

            @dataclass
            class Record(ConstructMixin):
                name: str = cs_field(CString("utf8"))
                opt: Optional[int] = cs_field(Int32ul, default=None)

        Without this wrapping, ``cs_field(Int32ul, default=None)`` would
        hard-error on ``build`` when ``opt=None`` (the inner Int32ul cannot
        serialize None). With wrapping, ``build`` emits empty bytes and
        ``parse`` skips the field when exhausted.

        Detection of "already Optional": duck-typing on the
        ``flagbuildnone`` attribute (set by :class:`Optional` and by
        construct-core's Optional wrapper).
        """
        hints = cls._get_type_hints()
        ftype = hints.get(field_name)
        if ftype is None:
            return subcon
        origin = typing.get_origin(ftype)
        # M1 fix: check both typing.Union and types.UnionType (PEP 604).
        if not _is_union_origin(origin):
            return subcon
        args = typing.get_args(ftype)
        if type(None) not in args:
            return subcon
        # ftype is Optional[...]. Skip wrapping if subcon already handles None.
        if getattr(subcon, "flagbuildnone", False):
            return subcon
        from ._optional import Optional as PyOptional

        return PyOptional(subcon)

    # ── Parse (classmethods) ─────────────────────────────────────────────

    @classmethod
    def parse(cls, data: bytes, **kw: Any) -> "ConstructMixin":
        """Parse bytes into a dataclass instance.

        Args:
            data: Binary data to parse.
            **kw: Top-level context fields (matching ``**contextkw``).

        Returns:
            An instance of ``cls`` with fields populated from the parsed data.
            Nested dataclass fields are recursively reconstructed (Phase 14.2).
        """
        holder = cls._compile()
        result_dict = holder.parse_bytes_py(data, **kw)
        return cls._dict_to_instance(result_dict)

    @classmethod
    def parse_stream(cls, stream: Any, **kw: Any) -> "ConstructMixin":
        """Parse from a file-like stream object.

        Reads either ``cls.sizeof()`` bytes (when the format has a static
        size — typical for fixed-layout dataclasses) or — as a fallback —
        all remaining bytes (legacy behavior, used when the size is not
        statically known, e.g. variable-length fields).

        The size-aware path is essential when this class is embedded inside
        a parent construct (``Array``, ``PrefixedArray``, ``Optional``) whose
        own parse logic expects each child to consume exactly its declared
        byte count; without it, ``stream.read()`` would gobble all remaining
        bytes and break sibling parses.

        Args:
            stream: A file-like object with ``read()`` / ``tell()`` methods.
            **kw: Top-level context fields.
        """
        data = _read_field_bytes(cls, stream)
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

    # ── Build (dual methods: instance call + class call for nesting) ────

    @_DualMethod
    def build(cls: type, obj: Any, **kw: Any) -> bytes:
        """Build this dataclass instance (or a dict representation) into bytes.

        Dual-method (Phase 14.2+):

        - ``instance.build()``: serializes the instance.
        - ``Class.build(dict_obj)``: coerces the dict to an instance first
          (via :meth:`_dict_to_instance`), then serializes. Used when this
          class is embedded inside another construct (``Array``, ``Optional``
          from a parent ``Struct``) where the build input is a
          :class:`Container` dict.

        Args:
            obj: The instance (when called on an instance) or a dict /
                Container (when called on the class).
            **kw: Top-level context fields.

        Returns:
            The serialized bytes.
        """
        if obj is None:
            return b""
        if isinstance(obj, dict):
            obj = cls._dict_to_instance(obj)
        holder = cls._compile()
        return holder.build_from_py(obj, **kw)

    @_DualMethod
    def build_stream(cls: type, obj: Any, stream: Any, **kw: Any) -> None:
        """Build and write to a file-like stream.

        Dual-method (Phase 14.2+): see :meth:`build` for class-call
        semantics. ``None`` input produces no output (matches
        :class:`Optional` semantics).

        Args:
            obj: The instance (when called on an instance) or a dict /
                Container (when called on the class).
            stream: A file-like object with a ``write()`` method.
            **kw: Top-level context fields.
        """
        if obj is None:
            return
        if isinstance(obj, dict):
            obj = cls._dict_to_instance(obj)
        holder = cls._compile()
        data = holder.build_from_py(obj, **kw)
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

    # ── dict → instance (MVP scheme A, Phase 14.2 nested support) ────────

    @classmethod
    def _dict_to_instance(cls, result_dict: "dict[str, Any]") -> "ConstructMixin":
        """Recursively converts a parsed dict into a dataclass instance.

        Phase 14.2: walks :func:`dataclasses.fields` (whitelist, I-COMP-2)
        and coerces values based on resolved type hints (I-COMP-1):

        - Nested dict whose field type is a ConstructMixin subclass →
          recursively converted via ``ftype._dict_to_instance``.
        - ``list[dict]`` whose field type is ``List[ConstructMixin]`` →
          list of recursively-converted instances (N-6).
        - All other values are passed through unchanged.

        Args:
            result_dict: The dict returned by ``parse_bytes_py``. May contain
                extra keys (anonymous fields, internal cache entries) —
                silently ignored by the ``fields()`` whitelist.

        Returns:
            A ``cls`` instance with fields populated from ``result_dict``.
        """
        hints = cls._get_type_hints()
        kwargs: "dict[str, Any]" = {}
        for f in dataclasses.fields(cls):
            fname = f.name
            if fname not in result_dict:
                # Rely on dataclass default if present; otherwise the
                # subsequent cls(**kwargs) call will raise the canonical
                # "missing required argument" error.
                continue
            val = result_dict[fname]
            ftype = hints.get(fname)
            kwargs[fname] = _coerce_value_to_dataclass(ftype, val)
        return cls(**kwargs)


def _read_field_bytes(cls: type, stream: Any) -> bytes:
    """Reads the bytes this dataclass needs from ``stream``.

    Phase 14.2/14.3 helper used by :meth:`ConstructMixin.parse_stream` when
    the class is embedded inside a parent construct (``Array``, ``Optional``,
    ...). If the class has a static byte size, only that many bytes are read;
    otherwise all remaining bytes are read (legacy behavior).

    Args:
        cls: A :class:`ConstructMixin` subclass.
        stream: A file-like object with ``read()``.

    Returns:
        The bytes read from ``stream``.
    """
    try:
        size = cls.sizeof()
    except Exception:
        size = None
    if size is None:
        return stream.read()
    return stream.read(size)


def _coerce_value_to_dataclass(ftype: Any, val: Any) -> Any:
    """Recursively coerces parsed values to dataclass instances.

    - ``ConstructMixin`` subclass + dict value → recursive
      :meth:`_dict_to_instance`.
    - ``Optional[T]`` / ``Union[T, None]`` + dict value → unwraps ``T`` and
      recurses (linked-list / tree pattern).
    - ``List[T]`` + list value → list of recursive calls.
    - Anything else → returned unchanged.
    """
    if val is None:
        return None
    if _is_construct_mixin_subclass(ftype) and isinstance(val, dict):
        return ftype._dict_to_instance(val)
    # Optional[T] / Union[T, None]: unwrap and recurse on T.
    # M1 fix: also handle types.UnionType (PEP 604 `T | None` syntax).
    origin = typing.get_origin(ftype)
    if _is_union_origin(origin):
        args = typing.get_args(ftype)
        non_none = [a for a in args if a is not type(None)]  # noqa: E721
        if len(non_none) == 1 and type(None) in args:
            return _coerce_value_to_dataclass(non_none[0], val)
    # List[T] where T is a ConstructMixin subclass.
    if origin in (list, typing.List) and isinstance(val, list):
        args = typing.get_args(ftype)
        if args and _is_construct_mixin_subclass(args[0]):
            inner = args[0]
            return [
                inner._dict_to_instance(item) if isinstance(item, dict) else item
                for item in val
            ]
    return val


class _DataclassSubcon:
    """Lazy adapter wrapping a :class:`ConstructMixin` subclass as a subcon.

    Used to embed a ConstructMixin subclass inside a pure-Python construct
    wrapper (``Optional``, ``Array``, ``PrefixedArray``, ...) where the
    build input may be a :class:`Container` dict (produced by the parent's
    ``as_value`` Container conversion) rather than a real dataclass instance.

    Without this wrapper, ``Optional(Node).build_stream(dict_value, stream)``
    would call ``Node.build_stream(dict_value, stream)``, but
    ``ConstructMixin.build_stream`` is an **instance** method that requires
    ``self`` to be a real instance. The adapter detects dict inputs and
    reconstructs the instance via :meth:`ConstructMixin._dict_to_instance`.

    The adapter exposes the duck-typed interface (``parse_stream`` /
    ``build_stream``) required by :func:`extract_subcon`, so it can be
    passed through ``Struct(**kwargs)`` like any other Python construct.
    """

    #: Default ``flagbuildnone`` (construct convention). Wrapped by
    #: :class:`Optional` if needed.
    flagbuildnone: bool = False

    def __init__(self, cls: type) -> None:
        self.cls = cls

    # -- parse API (mirror ConstructMixin classmethods) ---------------------

    def parse(self, data: bytes, **kw: Any) -> Any:
        return self.cls.parse(data, **kw)

    def parse_stream(self, stream: Any, **kw: Any) -> Any:
        return self.cls.parse_stream(stream, **kw)

    def parse_file(self, filename: str, **kw: Any) -> Any:
        return self.cls.parse_file(filename, **kw)

    # -- build API (handle dict OR instance) --------------------------------

    def _coerce(self, obj: Any) -> Any:
        """Converts a dict (Container) into a dataclass instance if needed.

        Build callers (Optional / Array) may pass either:
        - a real dataclass instance (forwarded as-is), or
        - a dict / Container (produced by parent's ``as_value``) — converted
          via :meth:`ConstructMixin._dict_to_instance`.
        """
        if obj is None:
            return None
        if isinstance(obj, self.cls):
            return obj
        if isinstance(obj, dict):
            return self.cls._dict_to_instance(obj)
        # Lists of dicts (Array-of-dataclass case): coerce element-wise.
        if isinstance(obj, list):
            return [
                self.cls._dict_to_instance(item) if isinstance(item, dict) else item
                for item in obj
            ]
        return obj

    def build(self, obj: Any, **kw: Any) -> bytes:
        instance = self._coerce(obj)
        if instance is None:
            return b""
        if isinstance(instance, list):
            # Array-of-dataclass fallback: serialize each item and concatenate.
            # (Normally Array itself handles element iteration; this branch
            # is a defensive fallback for unusual call patterns.)
            return b"".join(item.build(**kw) for item in instance)
        return instance.build(**kw)

    def build_stream(self, obj: Any, stream: Any, **kw: Any) -> None:
        instance = self._coerce(obj)
        if instance is None:
            return
        if isinstance(instance, list):
            for item in instance:
                item.build_stream(stream, **kw)
            return
        instance.build_stream(stream, **kw)

    def build_file(self, obj: Any, filename: str, **kw: Any) -> None:
        instance = self._coerce(obj)
        if instance is None:
            with open(filename, "wb"):
                pass
            return
        instance.build_file(filename, **kw)

    # -- sizeof -------------------------------------------------------------

    def sizeof(self, **kw: Any) -> "Optional[int]":
        return self.cls.sizeof(**kw)

    # -- construct operator support -----------------------------------------

    def __repr__(self) -> str:
        return f"<_DataclassSubcon {self.cls.__name__}>"

    def __rtruediv__(self, name: str) -> Any:
        """Enables ``"name" / subcon`` syntax (Renamed wrapper)."""
        if not isinstance(name, str):
            return NotImplemented
        from ._core import Renamed

        return Renamed(self, name)


# ── @construct_dataclass decorator (Phase 14.4) ────────────────────────────


def construct_dataclass(cls: Any = None, **dataclass_kwargs: Any) -> Any:
    """Decorator that combines ``@dataclasses.dataclass`` with
    :class:`ConstructMixin` injection.

    This is syntactic sugar equivalent to::

        @dataclasses.dataclass(**dataclass_kwargs)
        class Header(ConstructMixin):
            ...

    but lets the user omit the explicit ``ConstructMixin`` base::

        @construct_dataclass
        class Header:
            magic: bytes = cs_field(Bytes(4))
            version: int = cs_field(Int32ul)

    Works both bare (``@construct_dataclass``) and parameterised
    (``@construct_dataclass(frozen=True)``).

    **Implementation** (design §4.3, I-COMP-3 fix): instead of mutating
    ``cls.__bases__`` (which is risky and can conflict with existing bases),
    this function creates a **new subclass** via :func:`type` that inherits
    from :class:`ConstructMixin` and the original class's bases. All class
    body attributes (annotations, field defaults from :func:`cs_field`,
    methods) are preserved by copying ``cls.__dict__`` into the new class's
    namespace. The new class replaces the original in the caller's
    namespace (standard decorator semantics).

    If the class already inherits :class:`ConstructMixin`, the decorator
    simply applies ``@dataclasses.dataclass`` without creating a subclass.

    Args:
        cls: The class to decorate. When ``None``, the decorator was used
            with parentheses (``@construct_dataclass(frozen=True)``); a
            wrapper function is returned instead.
        **dataclass_kwargs: Keyword arguments forwarded to
            :func:`dataclasses.dataclass` (e.g. ``frozen=True``,
            ``slots=True``).

    Returns:
        A dataclass that also inherits :class:`ConstructMixin`, with
        ``parse`` / ``build`` / ``sizeof`` methods available.

    Example::

        @construct_dataclass
        class Header:
            magic: bytes = cs_field(Bytes(4))
            version: int = cs_field(Int32ul)

        header = Header.parse(b'MAGC\\x01\\x00\\x00\\x00')
        assert header.magic == b'MAGC'

        @construct_dataclass(frozen=True)
        class ImmutablePoint:
            x: int = cs_field(Int8ub)
            y: int = cs_field(Int8ub)
    """

    def _decorate(target_cls: Any) -> Any:
        # If ConstructMixin is already in the MRO, just apply @dataclass.
        if ConstructMixin in target_cls.__mro__:
            return dataclasses.dataclass(target_cls, **dataclass_kwargs)

        # Create a new class inheriting from ConstructMixin + original bases.
        # I-COMP-3 fix: use type() instead of mutating __bases__.
        # Copy the full __dict__ so field defaults (cs_field() results) and
        # annotations are present in the new class's own namespace —
        # dataclasses.dataclass reads defaults from cls.__dict__, not via
        # inheritance, so a shallow copy is essential.
        namespace: "dict[str, Any]" = {}
        for key, value in target_cls.__dict__.items():
            # Skip internal descriptors that type() manages automatically.
            if key in ("__dict__", "__weakref__"):
                continue
            namespace[key] = value

        new_cls = type(
            target_cls.__name__,
            (ConstructMixin,) + target_cls.__bases__,
            namespace,
        )
        new_cls.__module__ = target_cls.__module__
        new_cls.__qualname__ = target_cls.__qualname__
        return dataclasses.dataclass(new_cls, **dataclass_kwargs)

    if cls is not None:
        # Bare decorator: @construct_dataclass
        return _decorate(cls)
    # Parameterised: @construct_dataclass(frozen=True)
    return _decorate
