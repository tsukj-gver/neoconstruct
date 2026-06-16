"""End-to-end tests for the Phase 14.1 dataclass-first API.

These tests exercise the full ConstructMixin + cs_field pipeline through the
Python package (construct_rust), which imports the native extension
(construct_rust._core) and the pure-Python mixin modules.

Run with: ``pytest tests/test_dataclass_api.py`` (after ``maturin develop``).
"""

import dataclasses

import pytest

from construct_rust import (
    Bytes,
    ConstructMixin,
    Int16ub,
    Int32ul,
    Int8ub,
    cs_field,
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
