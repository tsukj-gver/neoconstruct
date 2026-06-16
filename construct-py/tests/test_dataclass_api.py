"""End-to-end tests for the Phase 14 dataclass-first API.

These tests exercise the full ConstructMixin + cs_field pipeline through the
Python package (construct_rust), which imports the native extension
(construct_rust._core) and the pure-Python mixin modules.

Phase 14.1: flat dataclass parse/build.
Phase 14.2: nested dataclass (cs_field(OtherDataclass)).
Phase 14.3: forward references (cs_field(None) + Optional[T] annotation),
            Optional auto-wrapping, circular reference detection, frozen
            dataclass, inheritance.

Run with: ``pytest tests/test_dataclass_api.py`` (after ``maturin develop``).
"""

import dataclasses
from typing import Optional

import pytest

from construct_rust import (
    Bytes,
    ConstructMixin,
    Int16ub,
    Int32ul,
    Int8ub,
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

