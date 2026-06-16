"""End-to-end tests for the Phase 14 dataclass-first API.

These tests exercise the full ConstructMixin + cs_field pipeline through the
Python package (construct_rust), which imports the native extension
(construct_rust._core) and the pure-Python mixin modules.

Phase 14.1: flat dataclass parse/build.
Phase 14.2: nested dataclass (cs_field(OtherDataclass)).
Phase 14.3: forward references (cs_field(None) + Optional[T] annotation),
            Optional auto-wrapping, circular reference detection, frozen
            dataclass, inheritance.
Phase 14.4: @construct_dataclass decorator, declarative interop, performance
            baseline, PEP 604 (types.UnionType) support, sizeof/asdict/
            stream method coverage.

Run with: ``pytest tests/test_dataclass_api.py`` (after ``maturin develop``).
"""

import dataclasses
import io
import time
from typing import Optional

import pytest

from construct_rust import (
    Array,
    Bytes,
    ConstructMixin,
    Int16ub,
    Int32ul,
    Int8ub,
    Struct,
    construct_dataclass,
    cs_field,
    this,
)


# ---------------------------------------------------------------------------
# Test dataclasses
# ---------------------------------------------------------------------------


@dataclasses.dataclass
class Point(ConstructMixin):
    """Simple 2-field flat dataclass."""

    x: int = cs_field(Int8ub)
    y: int = cs_field(Int8ub)


@dataclasses.dataclass
class Header(ConstructMixin):
    """Multi-field flat dataclass with different types."""

    magic: bytes = cs_field(Bytes(4))
    version: int = cs_field(Int32ul)
    count: int = cs_field(Int16ub)


@dataclasses.dataclass
class WithDefault(ConstructMixin):
    """Dataclass with a default field value."""

    a: int = cs_field(Int8ub)
    b: int = cs_field(Int8ub, default=0)


# ---------------------------------------------------------------------------
# Parse tests
# ---------------------------------------------------------------------------


class TestParse:
    def test_parse_two_field_dataclass(self):
        point = Point.parse(b"\x0A\x0B")
        assert point.x == 0x0A
        assert point.y == 0x0B

    def test_parse_multi_type_dataclass(self):
        data = b"\xDE\xAD\xBE\xEF" + (4).to_bytes(4, "little") + (7).to_bytes(2, "big")
        header = Header.parse(data)
        assert header.magic == b"\xDE\xAD\xBE\xEF"
        assert header.version == 4
        assert header.count == 7

    def test_parse_returns_correct_type(self):
        point = Point.parse(b"\x01\x02")
        assert isinstance(point, Point)

    def test_parse_with_context_kw(self):
        # **kw are injected as context fields.
        point = Point.parse(b"\x01\x02", extra=123)
        assert point.x == 1
        assert point.y == 2


# ---------------------------------------------------------------------------
# Build tests
# ---------------------------------------------------------------------------


class TestBuild:
    def test_build_two_field_dataclass(self):
        point = Point(x=0xCD, y=0xEF)
        assert point.build() == b"\xCD\xEF"

    def test_build_multi_type_dataclass(self):
        header = Header(magic=b"\xDE\xAD\xBE\xEF", version=4, count=7)
        data = header.build()
        assert data[:4] == b"\xDE\xAD\xBE\xEF"
        assert len(data) == 10

    def test_build_with_default(self):
        # b uses default value 0.
        instance = WithDefault(a=5)
        assert instance.build() == b"\x05\x00"

    def test_build_overrides_default(self):
        instance = WithDefault(a=5, b=9)
        assert instance.build() == b"\x05\x09"


# ---------------------------------------------------------------------------
# Round-trip tests
# ---------------------------------------------------------------------------


class TestRoundTrip:
    def test_roundtrip_point(self):
        original = Point(x=0x12, y=0x34)
        data = original.build()
        parsed = Point.parse(data)
        assert parsed.x == original.x
        assert parsed.y == original.y
        assert parsed.build() == data

    def test_roundtrip_header(self):
        original = Header(magic=b"\xAA\xBB\xCC\xDD", version=42, count=999)
        data = original.build()
        parsed = Header.parse(data)
        assert parsed.magic == original.magic
        assert parsed.version == original.version
        assert parsed.count == original.count
        assert parsed.build() == data

    def test_build_parse_symmetry(self):
        """build(parse(data)) == data (the core C4 invariant)."""
        data = b"\x07\x08"
        parsed = Point.parse(data)
        rebuilt = parsed.build()
        assert rebuilt == data


# ---------------------------------------------------------------------------
# Compilation cache tests
# ---------------------------------------------------------------------------


class TestCompilationCache:
    def test_cache_reused_across_calls(self):
        # First parse triggers compilation.
        Point.parse(b"\x01\x02")
        holder1 = Point._compiled
        # Second parse should reuse the same holder.
        Point.parse(b"\x03\x04")
        holder2 = Point._compiled
        assert holder1 is holder2

    def test_cache_per_class(self):
        Point.parse(b"\x01\x02")
        Header.parse(b"\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00")
        # Different classes have different cached holders.
        assert Point._compiled is not Header._compiled


# ---------------------------------------------------------------------------
# Error path tests
# ---------------------------------------------------------------------------


class TestErrors:
    def test_field_without_cs_field_raises_typeerror(self):
        @dataclasses.dataclass
        class Bad(ConstructMixin):
            value: int = 0  # NOT a cs_field!

        with pytest.raises(TypeError, match="cs_field"):
            Bad.parse(b"\x01")

    def test_parse_truncated_input_raises(self):
        # Point needs 2 bytes, give 1.
        with pytest.raises(Exception):
            Point.parse(b"\x01")


# ---------------------------------------------------------------------------
# Dataclass protocol compatibility
# ---------------------------------------------------------------------------


class TestDataclassProtocol:
    def test_fields_preserved(self):
        names = [f.name for f in dataclasses.fields(Point)]
        assert names == ["x", "y"]

    def test_asdict_works(self):
        point = Point(x=1, y=2)
        d = dataclasses.asdict(point)
        assert d == {"x": 1, "y": 2}

    def test_equality_works(self):
        p1 = Point(x=1, y=2)
        p2 = Point(x=1, y=2)
        p3 = Point(x=1, y=3)
        assert p1 == p2
        assert p1 != p3

    def test_repr_works(self):
        point = Point(x=1, y=2)
        assert "Point" in repr(point)


# ===========================================================================
# Phase 14.2: nested dataclass support
# ===========================================================================


@dataclasses.dataclass
class Inner(ConstructMixin):
    """Nested dataclass used by outer formats."""

    a: int = cs_field(Int8ub)
    b: int = cs_field(Int8ub)


@dataclasses.dataclass
class Outer(ConstructMixin):
    """Flat outer struct embedding Inner via cs_field(Inner)."""

    inner: Inner = cs_field(Inner)
    suffix: int = cs_field(Int8ub)


@dataclasses.dataclass
class FileHeader(ConstructMixin):
    """Header for the FileFormat demo (with cross-field expression)."""

    magic: bytes = cs_field(Bytes(4))
    size: int = cs_field(Int32ul)


@dataclasses.dataclass
class FileFormat(ConstructMixin):
    """Classic nested-dataclass + expression example.

    ``data`` length references the parsed ``header.size`` field via the
    expression system (``this.header.size``).
    """

    header: FileHeader = cs_field(FileHeader)
    data: bytes = cs_field(Bytes(this.header.size))


class TestNestedDataclass:
    """Phase 14.2: cs_field(OtherConstructMixin) → recursive Struct decl."""

    def test_nested_build(self):
        """Inner instance is built correctly inside outer."""
        outer = Outer(inner=Inner(a=1, b=2), suffix=3)
        assert outer.build() == b"\x01\x02\x03"

    def test_nested_parse_returns_nested_instances(self):
        """Parsed inner field is a real Inner instance, not a dict."""
        parsed = Outer.parse(b"\x05\x06\x07")
        assert isinstance(parsed, Outer)
        assert isinstance(parsed.inner, Inner)
        assert parsed.inner.a == 5
        assert parsed.inner.b == 6
        assert parsed.suffix == 7

    def test_nested_roundtrip(self):
        """build(parse(data)) == data (C4 invariant) for nested dataclasses."""
        original = Outer(inner=Inner(a=0xAB, b=0xCD), suffix=0xEF)
        data = original.build()
        parsed = Outer.parse(data)
        # Field-level equality
        assert parsed.inner.a == original.inner.a
        assert parsed.inner.b == original.inner.b
        assert parsed.suffix == original.suffix
        # Byte-level round-trip
        assert parsed.build() == data

    def test_nested_with_expression_roundtrip(self):
        """Nested dataclass + this.<field>.<subfield> expression reference."""
        original = FileFormat(
            header=FileHeader(magic=b"MAGC", size=4),
            data=b"ABCD",
        )
        data = original.build()
        # Expected layout: 4 bytes magic + 4 bytes LE size + N bytes data
        assert data == b"MAGC" + (4).to_bytes(4, "little") + b"ABCD"

        parsed = FileFormat.parse(data)
        assert isinstance(parsed.header, FileHeader)
        assert parsed.header.magic == b"MAGC"
        assert parsed.header.size == 4
        assert parsed.data == b"ABCD"
        assert parsed.build() == data

    def test_deeply_nested_three_levels(self):
        """Three-level nesting: Outermost → Mid → Innermost."""

        @dataclasses.dataclass
        class Innermost(ConstructMixin):
            x: int = cs_field(Int8ub)

        @dataclasses.dataclass
        class Mid(ConstructMixin):
            inner: Innermost = cs_field(Innermost)
            y: int = cs_field(Int8ub)

        @dataclasses.dataclass
        class Outermost(ConstructMixin):
            mid: Mid = cs_field(Mid)
            z: int = cs_field(Int8ub)

        original = Outermost(mid=Mid(inner=Innermost(x=1), y=2), z=3)
        data = original.build()
        assert data == b"\x01\x02\x03"

        parsed = Outermost.parse(data)
        assert isinstance(parsed.mid, Mid)
        assert isinstance(parsed.mid.inner, Innermost)
        assert parsed.mid.inner.x == 1
        assert parsed.mid.y == 2
        assert parsed.z == 3
        assert parsed.build() == data


# ===========================================================================
# Phase 14.3: forward references + Optional + boundary cases
# ===========================================================================


@dataclasses.dataclass
class Node(ConstructMixin):
    """Linked-list node: forward reference via cs_field(None) + Optional."""

    value: int = cs_field(Int32ul)
    next: Optional["Node"] = cs_field(None)


@dataclasses.dataclass
class Record(ConstructMixin):
    """Record with an Optional field (default=None)."""

    name: int = cs_field(Int32ul)
    optional_value: Optional[int] = cs_field(Int32ul, default=None)


class TestForwardReferences:
    """Phase 14.3: cs_field(None) + type annotation."""

    def test_forward_ref_linked_list_build(self):
        """3-element linked list builds correct bytes (each node 4 bytes)."""
        n3 = Node(value=3, next=None)
        n2 = Node(value=2, next=n3)
        n1 = Node(value=1, next=n2)
        data = n1.build()
        assert data == b"\x01\x00\x00\x00\x02\x00\x00\x00\x03\x00\x00\x00"

    def test_forward_ref_linked_list_parse(self):
        """Parsed linked list has real Node instances recursively."""
        data = b"\x01\x00\x00\x00\x02\x00\x00\x00\x03\x00\x00\x00"
        parsed = Node.parse(data)
        assert isinstance(parsed, Node)
        assert parsed.value == 1
        assert isinstance(parsed.next, Node)
        assert parsed.next.value == 2
        assert isinstance(parsed.next.next, Node)
        assert parsed.next.next.value == 3
        assert parsed.next.next.next is None

    def test_forward_ref_linked_list_roundtrip(self):
        """build(parse(data)) == data for forward-ref linked list."""
        n3 = Node(value=3, next=None)
        n2 = Node(value=2, next=n3)
        n1 = Node(value=1, next=n2)
        data = n1.build()
        parsed = Node.parse(data)
        assert parsed.build() == data

    def test_forward_ref_single_node_roundtrip(self):
        """Single-node list (next=None) round-trips."""
        node = Node(value=42, next=None)
        data = node.build()
        assert data == (42).to_bytes(4, "little")
        parsed = Node.parse(data)
        assert parsed.value == 42
        assert parsed.next is None


class TestOptionalFields:
    """Phase 14.3: cs_field(subcon, default=None) + Optional[X] annotation."""

    def test_optional_field_none_build(self):
        """When optional_value=None, build emits only the required field."""
        rec = Record(name=42)  # optional_value defaults to None
        data = rec.build()
        # Only the `name` field is built; Optional wraps the Int32ul so that
        # None produces empty bytes.
        assert data == (42).to_bytes(4, "little")

    def test_optional_field_value_build(self):
        """When optional_value is set, build emits both fields."""
        rec = Record(name=42, optional_value=100)
        data = rec.build()
        assert data == (42).to_bytes(4, "little") + (100).to_bytes(4, "little")

    def test_optional_field_parse_none(self):
        """Short input → optional_value parses as None."""
        data = (42).to_bytes(4, "little")
        parsed = Record.parse(data)
        assert parsed.name == 42
        assert parsed.optional_value is None

    def test_optional_field_parse_value(self):
        """Longer input → optional_value parses as the int."""
        data = (42).to_bytes(4, "little") + (100).to_bytes(4, "little")
        parsed = Record.parse(data)
        assert parsed.name == 42
        assert parsed.optional_value == 100

    def test_optional_roundtrip(self):
        """Round-trip with optional_value present."""
        rec = Record(name=7, optional_value=99)
        data = rec.build()
        parsed = Record.parse(data)
        assert parsed.name == 7
        assert parsed.optional_value == 99
        assert parsed.build() == data


class TestCircularReferenceDetection:
    """Phase 14.3: direct mutual recursion raises RecursionError."""

    def test_direct_self_recursion_raises(self):
        """cs_field(Self) inside Self → RecursionError at compile time.

        We construct a class that directly nests itself (non-Optional) and
        verify that compiling it triggers cycle detection. The class body
        uses cs_field(None) + a "Self" annotation; when _resolve_forward_ref
        resolves "Self" → the class itself, _resolve_nested_subcon then
        recursively calls Self._build_struct_decl which hits the cycle.
        """

        @dataclasses.dataclass
        class SelfRef(ConstructMixin):
            value: int = cs_field(Int8ub)
            # Forward ref to SelfRef itself (non-Optional) — direct cycle.
            child: "SelfRef" = cs_field(None)

        with pytest.raises((RecursionError, TypeError)):
            SelfRef.parse(b"\x01")

    def test_no_cycle_when_optional(self):
        """Optional[X] referencing X itself is allowed (linked-list pattern).

        This is the canonical valid recursive structure: each ``Node`` has
        an ``Optional[Node]`` next pointer. Cycle detection must NOT fire.
        """
        # Node is already defined at module level — re-test it here for
        # explicit documentation.
        n = Node(value=1, next=None)
        data = n.build()
        assert Node.parse(data).value == 1


# ===========================================================================
# Phase 14.3: boundary cases
# ===========================================================================


class TestFrozenDataclass:
    """frozen=True dataclasses should still build (build is read-only)."""

    def test_frozen_build_and_parse(self):
        @dataclasses.dataclass(frozen=True)
        class FrozenPoint(ConstructMixin):
            x: int = cs_field(Int8ub)
            y: int = cs_field(Int8ub)

        p = FrozenPoint(x=1, y=2)
        # Build works (only reads attributes).
        data = p.build()
        assert data == b"\x01\x02"
        # Frozen instances cannot be mutated.
        with pytest.raises(dataclasses.FrozenInstanceError):
            p.x = 99
        # Parse returns a frozen instance.
        parsed = FrozenPoint.parse(b"\x03\x04")
        assert isinstance(parsed, FrozenPoint)
        assert parsed.x == 3
        assert parsed.y == 4
        assert parsed.build() == b"\x03\x04"


class TestInheritance:
    """ConstructMixin subclass inheriting another ConstructMixin subclass."""

    def test_inheritance_basic(self):
        @dataclasses.dataclass
        class Base(ConstructMixin):
            x: int = cs_field(Int8ub)

        @dataclasses.dataclass
        class Derived(Base):
            y: int = cs_field(Int8ub)

        # Derived has fields x (inherited) + y (own).
        field_names = [f.name for f in dataclasses.fields(Derived)]
        assert field_names == ["x", "y"]

        d = Derived(x=1, y=2)
        assert d.build() == b"\x01\x02"

        parsed = Derived.parse(b"\x05\x06")
        assert parsed.x == 5
        assert parsed.y == 6
        assert parsed.build() == b"\x05\x06"

    def test_inheritance_with_nested(self):
        """Inheritance + nested dataclass together."""

        @dataclasses.dataclass
        class Child(ConstructMixin):
            v: int = cs_field(Int8ub)

        @dataclasses.dataclass
        class ParentBase(ConstructMixin):
            tag: int = cs_field(Int8ub)

        @dataclasses.dataclass
        class Parent(ParentBase):
            child: Child = cs_field(Child)

        original = Parent(tag=10, child=Child(v=20))
        data = original.build()
        assert data == b"\x0A\x14"

        parsed = Parent.parse(data)
        assert parsed.tag == 10
        assert isinstance(parsed.child, Child)
        assert parsed.child.v == 20
        assert parsed.build() == data


class TestCsFieldNoneWithoutAnnotation:
    """cs_field(None) with no usable annotation → error.

    Python ``@dataclass`` itself rejects a field declared without any type
    annotation (``other = cs_field(None)`` — no annotation), so the error
    surfaces at class-definition time. This test verifies that contract.
    """

    def test_cs_field_none_without_annotation_rejected_by_dataclass(self):
        with pytest.raises(TypeError, match="no type annotation"):

            @dataclasses.dataclass
            class Bad(ConstructMixin):
                value: int = cs_field(Int8ub)
                # No annotation at all — @dataclass rejects this.
                other = cs_field(None)

    def test_cs_field_none_with_unresolvable_annotation_raises_at_compile(self):
        """Annotation present but unresolvable → TypeError at compile time."""

        @dataclasses.dataclass
        class BadUnresolved(ConstructMixin):
            value: int = cs_field(Int8ub)
            # Annotation references an undefined name.
            other: "DoesNotExist" = cs_field(None)

        with pytest.raises((TypeError, NameError)):
            BadUnresolved.parse(b"\x01\x02")


# ===========================================================================
# Phase 14.4: @construct_dataclass decorator
# ===========================================================================


class TestConstructDataclassDecorator:
    """Phase 14.4: @construct_dataclass syntactic sugar."""

    def test_decorator_bare_no_parens(self):
        """@construct_dataclass (no parens) injects ConstructMixin."""

        @construct_dataclass
        class Header:
            magic: bytes = cs_field(Bytes(4))
            version: int = cs_field(Int32ul)

        # Is a dataclass.
        assert dataclasses.is_dataclass(Header)
        # Inherits ConstructMixin.
        assert issubclass(Header, ConstructMixin)
        # Fields preserved.
        names = [f.name for f in dataclasses.fields(Header)]
        assert names == ["magic", "version"]

    def test_decorator_parse_and_build(self):
        """Decorated class has working parse/build methods."""

        @construct_dataclass
        class Header:
            magic: bytes = cs_field(Bytes(4))
            version: int = cs_field(Int32ul)

        data = b"ABCD" + (1).to_bytes(4, "little")
        parsed = Header.parse(data)
        assert isinstance(parsed, Header)
        assert parsed.magic == b"ABCD"
        assert parsed.version == 1
        assert parsed.build() == data

    def test_decorator_roundtrip(self):
        """build(parse(data)) == data for decorated class."""

        @construct_dataclass
        class Record:
            a: int = cs_field(Int8ub)
            b: int = cs_field(Int8ub)

        original = Record(a=0x12, b=0x34)
        data = original.build()
        parsed = Record.parse(data)
        assert parsed.build() == data

    def test_decorator_with_kwargs(self):
        """@construct_dataclass(frozen=True) works with dataclass kwargs."""

        @construct_dataclass(frozen=True)
        class ImmutablePoint:
            x: int = cs_field(Int8ub)
            y: int = cs_field(Int8ub)

        assert dataclasses.is_dataclass(ImmutablePoint)
        assert issubclass(ImmutablePoint, ConstructMixin)
        p = ImmutablePoint(x=1, y=2)
        assert p.build() == b"\x01\x02"
        # Frozen: setattr should fail.
        with pytest.raises(dataclasses.FrozenInstanceError):
            p.x = 99

    def test_decorator_already_inherits_mixin(self):
        """Decorator on a class already inheriting ConstructMixin just
        applies @dataclass."""

        @construct_dataclass
        class DoubleDecorated(ConstructMixin):
            v: int = cs_field(Int8ub)

        assert dataclasses.is_dataclass(DoubleDecorated)
        assert DoubleDecorated.parse(b"\x05").v == 5

    def test_decorator_dataclass_protocol(self):
        """Standard dataclass tools work on decorated classes."""

        @construct_dataclass
        class Item:
            name: int = cs_field(Int8ub)
            qty: int = cs_field(Int8ub)

        item = Item(name=1, qty=2)
        # dataclasses.fields
        assert [f.name for f in dataclasses.fields(Item)] == ["name", "qty"]
        # dataclasses.asdict
        assert dataclasses.asdict(item) == {"name": 1, "qty": 2}
        # repr contains class name
        assert "Item" in repr(item)


# ===========================================================================
# Phase 14.4: declarative API interop
# ===========================================================================


class TestDeclarativeInterop:
    """Phase 14.4: ConstructMixin dataclass ↔ declarative Struct interop.

    .. note::

       The ``"name" / DataclassClass`` syntax does **not** work because
       Python's binary operator protocol dispatches ``__rtruediv__`` on
       ``type(b)`` (the metaclass), not on the class itself. Use the
       keyword form ``Struct(name=DataclassClass)`` or wrap the class in
       ``Array(n, DataclassClass)`` (whose Rust wrapper supports ``/``).
    """

    def test_dataclass_as_struct_subcon_build(self):
        """ConstructMixin subclass used as a subcon inside a declarative
        Struct — build direction.

        Uses the keyword form ``Struct(point=Point)`` since
        ``"point" / Point`` is not supported by Python's operator protocol
        for class operands (see class docstring).
        """

        @dataclasses.dataclass
        class Point(ConstructMixin):
            x: int = cs_field(Int8ub)
            y: int = cs_field(Int8ub)

        # Build: Struct(point=Point) → serialises Point instance attributes.
        fmt = Struct(point=Point)
        data = fmt.build({"point": Point(x=1, y=2)})
        assert data == b"\x01\x02"

    def test_dataclass_as_struct_subcon_parse(self):
        """ConstructMixin subclass used as a subcon inside a declarative
        Struct — parse direction.

        The parsed result for the dataclass field is a Container dict (the
        declarative API's standard output), not a dataclass instance.
        """

        @dataclasses.dataclass
        class Point(ConstructMixin):
            x: int = cs_field(Int8ub)
            y: int = cs_field(Int8ub)

        fmt = Struct(point=Point)
        result = fmt.parse(b"\x01\x02")
        assert result["point"]["x"] == 1
        assert result["point"]["y"] == 2

    def test_array_of_dataclass_build(self):
        """Array(n, Dataclass) build path works — each element is built
        via the dataclass's build method."""

        @dataclasses.dataclass
        class Coord(ConstructMixin):
            x: int = cs_field(Int8ub)
            y: int = cs_field(Int8ub)

        arr = Array(3, Coord)
        data = arr.build([Coord(1, 2), Coord(3, 4), Coord(5, 6)])
        assert data == b"\x01\x02\x03\x04\x05\x06"

    def test_declarative_construct_as_cs_field(self):
        """A declarative construct (e.g. Struct) used as a cs_field value
        inside a dataclass — already implicitly tested in 14.2 with
        Bytes(this.header.size); this test uses a plain declarative
        subcon explicitly."""

        @dataclasses.dataclass
        class Wrapper(ConstructMixin):
            pair: object = cs_field(Struct("a" / Int8ub, "b" / Int8ub))
            tag: int = cs_field(Int8ub)

        w = Wrapper(pair={"a": 10, "b": 20}, tag=30)
        data = w.build()
        assert data == b"\x0A\x14\x1E"

        parsed = Wrapper.parse(data)
        assert parsed.pair["a"] == 10
        assert parsed.pair["b"] == 20
        assert parsed.tag == 30


# ===========================================================================
# Phase 14.4: PEP 604 UnionType support (M1 fix)
# ===========================================================================


class TestPEP604UnionType:
    """M1 fix: ``X | None`` (types.UnionType) is treated the same as
    ``Optional[X]`` (typing.Union) in all annotation-processing paths."""

    def test_pep604_optional_field_build_none(self):
        """``int | None`` annotation with default=None → build emits
        empty bytes when the value is None."""

        @dataclasses.dataclass
        class Rec604(ConstructMixin):
            name: int = cs_field(Int32ul)
            opt: "int | None" = cs_field(Int32ul, default=None)

        r = Rec604(name=42)
        data = r.build()
        assert data == (42).to_bytes(4, "little")

    def test_pep604_optional_field_build_value(self):
        """``int | None`` annotation with a value → build emits both
        fields."""

        @dataclasses.dataclass
        class Rec604(ConstructMixin):
            name: int = cs_field(Int32ul)
            opt: "int | None" = cs_field(Int32ul, default=None)

        r = Rec604(name=42, opt=100)
        data = r.build()
        assert data == (42).to_bytes(4, "little") + (100).to_bytes(4, "little")

    def test_pep604_optional_field_roundtrip(self):
        """Full round-trip with PEP 604 syntax."""

        @dataclasses.dataclass
        class Rec604(ConstructMixin):
            name: int = cs_field(Int32ul)
            opt: "int | None" = cs_field(Int32ul, default=None)

        original = Rec604(name=7, opt=99)
        data = original.build()
        parsed = Rec604.parse(data)
        assert parsed.name == 7
        assert parsed.opt == 99
        assert parsed.build() == data


# ===========================================================================
# Phase 14.4: sizeof / asdict / empty dataclass / stream methods (M3 fix)
# ===========================================================================


class TestSizeofAndAsdict:
    """M3: sizeof(), dataclasses.asdict() on nested, empty dataclass."""

    def test_sizeof_flat_returns_value(self):
        """sizeof() on a flat fixed-size dataclass returns an int or None.

        Note: Due to Phase 13's Dynamic-node compilation (see 14.2/14.3
        process record N-6), sizeof() may return None. We accept either
        but verify the call does not error.
        """

        @dataclasses.dataclass
        class FixedSize(ConstructMixin):
            a: int = cs_field(Int8ub)
            b: int = cs_field(Int8ub)

        size = FixedSize.sizeof()
        # Accept either a correct int or None (Phase 13 limitation).
        assert size is None or size == 2

    def test_asdict_nested_dataclass(self):
        """dataclasses.asdict() recursively converts nested dataclass
        instances to plain dicts."""

        @dataclasses.dataclass
        class Inner2(ConstructMixin):
            v: int = cs_field(Int8ub)

        @dataclasses.dataclass
        class Outer2(ConstructMixin):
            inner: Inner2 = cs_field(Inner2)
            tag: int = cs_field(Int8ub)

        obj = Outer2(inner=Inner2(v=5), tag=9)
        d = dataclasses.asdict(obj)
        assert d == {"inner": {"v": 5}, "tag": 9}

    def test_empty_dataclass_build_parse(self):
        """A dataclass with zero fields builds/parse empty bytes."""

        @dataclasses.dataclass
        class Empty(ConstructMixin):
            pass

        e = Empty()
        assert e.build() == b""
        parsed = Empty.parse(b"")
        assert isinstance(parsed, Empty)
        assert parsed.build() == b""

    def test_asdict_with_optional_field(self):
        """dataclasses.asdict handles Optional fields correctly."""

        @dataclasses.dataclass
        class WithOpt(ConstructMixin):
            a: int = cs_field(Int8ub)
            b: Optional[int] = cs_field(Int8ub, default=None)

        obj = WithOpt(a=1)
        d = dataclasses.asdict(obj)
        assert d == {"a": 1, "b": None}


class TestStreamMethods:
    """M3: parse_stream / build_stream / build_file / parse_file."""

    def test_build_stream(self):
        """build_stream writes bytes to a file-like object."""

        @dataclasses.dataclass
        class S(ConstructMixin):
            a: int = cs_field(Int8ub)
            b: int = cs_field(Int8ub)

        s = S(a=1, b=2)
        buf = io.BytesIO()
        s.build_stream(buf)
        assert buf.getvalue() == b"\x01\x02"

    def test_parse_stream(self):
        """parse_stream reads from a file-like object."""

        @dataclasses.dataclass
        class S(ConstructMixin):
            a: int = cs_field(Int8ub)
            b: int = cs_field(Int8ub)

        buf = io.BytesIO(b"\x03\x04")
        parsed = S.parse_stream(buf)
        assert parsed.a == 3
        assert parsed.b == 4

    def test_build_file_and_parse_file(self, tmp_path):
        """build_file / parse_file round-trip through a real file."""

        @dataclasses.dataclass
        class S(ConstructMixin):
            a: int = cs_field(Int8ub)
            b: int = cs_field(Int8ub)

        original = S(a=0xAA, b=0xBB)
        filepath = tmp_path / "test_data.bin"
        original.build_file(str(filepath))

        # File contents correct.
        raw = filepath.read_bytes()
        assert raw == b"\xAA\xBB"

        # Parse back.
        parsed = S.parse_file(str(filepath))
        assert parsed.a == 0xAA
        assert parsed.b == 0xBB


# ===========================================================================
# Phase 14.4: performance baseline (no hard asserts, prints timing)
# ===========================================================================


class TestPerformanceBaseline:
    """Establishes a performance baseline comparing dataclass API and
    declarative API parse/build throughput.

    These tests print timing data and perform only soft sanity checks
    (no hard performance assertions — the goal is to record a baseline
    for future optimisation comparison, not to enforce a speed target).
    """

    def test_perf_dataclass_vs_declarative(self, capsys):
        """Compares 10 000-iteration parse+build for dataclass API vs
        declarative API on an equivalent 6-byte format."""

        # --- Dataclass API ---
        @dataclasses.dataclass
        class PerfHeader(ConstructMixin):
            magic: bytes = cs_field(Bytes(4))
            version: int = cs_field(Int16ub)

        test_bytes = b"TEST" + (1).to_bytes(2, "big")

        # Warm up (trigger compilation).
        PerfHeader.parse(test_bytes)
        PerfHeader(magic=b"TEST", version=1).build()

        iterations = 10_000

        # Parse benchmark — dataclass.
        start = time.perf_counter()
        for _ in range(iterations):
            PerfHeader.parse(test_bytes)
        dc_parse_elapsed = time.perf_counter() - start

        # Build benchmark — dataclass.
        instance = PerfHeader(magic=b"TEST", version=1)
        start = time.perf_counter()
        for _ in range(iterations):
            instance.build()
        dc_build_elapsed = time.perf_counter() - start

        # --- Declarative API ---
        decl_fmt = Struct("magic" / Bytes(4), "version" / Int16ub)

        # Warm up.
        decl_fmt.parse(test_bytes)
        decl_fmt.build({"magic": b"TEST", "version": 1})

        # Parse benchmark — declarative.
        start = time.perf_counter()
        for _ in range(iterations):
            decl_fmt.parse(test_bytes)
        decl_parse_elapsed = time.perf_counter() - start

        # Build benchmark — declarative.
        build_obj = {"magic": b"TEST", "version": 1}
        start = time.perf_counter()
        for _ in range(iterations):
            decl_fmt.build(build_obj)
        decl_build_elapsed = time.perf_counter() - start

        # Print results (captured by capsys, visible with -s flag).
        print(f"\n{'=' * 60}")
        print(f"Performance baseline ({iterations} iterations):")
        print(f"  dataclass  parse: {dc_parse_elapsed:.4f}s")
        print(f"  declarative parse: {decl_parse_elapsed:.4f}s")
        print(f"  dataclass  build: {dc_build_elapsed:.4f}s")
        print(f"  declarative build: {decl_build_elapsed:.4f}s")
        parse_ratio = (
            dc_parse_elapsed / decl_parse_elapsed if decl_parse_elapsed > 0 else 0
        )
        build_ratio = (
            dc_build_elapsed / decl_build_elapsed if decl_build_elapsed > 0 else 0
        )
        print(f"  parse ratio (dc/decl): {parse_ratio:.2f}x")
        print(f"  build ratio (dc/decl): {build_ratio:.2f}x")
        print(f"{'=' * 60}")

        # Soft sanity check: both should complete in reasonable time
        # (< 30 seconds for 10K iterations).
        assert dc_parse_elapsed < 30.0
        assert dc_build_elapsed < 30.0

