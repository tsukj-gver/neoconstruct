//! PyO3 wrappers for Gallery format parsers (ELF, PE32/COFF, UTIndex).
//!
//! The Rust gallery factory functions return `Box<dyn Construct>`. This
//! module provides a generic [`PyGalleryParser`] wrapper that holds the
//! boxed construct, plus three `#[pyfunction]` factories.

use construct::core::Construct;

use pyo3::prelude::*;

use crate::construct_macros::PyConstructWrapper;

// ===========================================================================
// GalleryKind — tracks which factory produced this parser
// ===========================================================================

/// Identifies which gallery factory created this parser.
///
/// Used by [`PyGalleryParser::make_owned`] to reconstruct an owned
/// `Box<dyn Construct>` by re-calling the appropriate factory.
#[derive(Clone, Copy)]
enum GalleryKind {
    /// ELF parser.
    Elf,
    /// PE32/COFF parser.
    Pe32File,
    /// Unreal Tournament Index parser.
    UTIndex,
}

// ===========================================================================
// PyGalleryParser
// ===========================================================================

/// Generic PyO3 wrapper for gallery format parsers.
///
/// Holds a `Box<dyn Construct>` produced by a Rust gallery factory.
/// Corresponds to Python `gallery.elf.elf`, `gallery.pe32coff.pe32file`,
/// and `gallery.ut_index.UTIndex`.
#[pyclass(name = "GalleryParser", unsendable)]
pub struct PyGalleryParser {
    /// The boxed Rust construct.
    pub(crate) inner: Box<dyn Construct>,
    /// Which factory produced this parser (for `make_owned`).
    kind: GalleryKind,
}

impl PyConstructWrapper for PyGalleryParser {
    fn as_construct(&self) -> &dyn Construct {
        self.inner.as_ref()
    }
}

impl PyGalleryParser {
    /// Reconstructs an owned `Box<dyn Construct>` by re-calling the
    /// appropriate gallery factory.
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Ok(match self.kind {
            GalleryKind::Elf => construct::gallery::elf::elf(),
            GalleryKind::Pe32File => construct::gallery::pe32coff::pe32file(),
            GalleryKind::UTIndex => Box::new(construct::gallery::ut_index::UTIndex::new()),
        })
    }
}

// ===========================================================================
// Factory functions
// ===========================================================================

/// Factory: ELF parser.
///
/// Returns a [`PyGalleryParser`] wrapping `construct::gallery::elf::elf()`.
#[pyfunction]
#[pyo3(name = "elf")]
pub fn py_elf() -> PyGalleryParser {
    PyGalleryParser {
        inner: construct::gallery::elf::elf(),
        kind: GalleryKind::Elf,
    }
}

/// Factory: PE32/COFF parser.
///
/// Returns a [`PyGalleryParser`] wrapping `construct::gallery::pe32coff::pe32file()`.
#[pyfunction]
#[pyo3(name = "pe32file")]
pub fn py_pe32file() -> PyGalleryParser {
    PyGalleryParser {
        inner: construct::gallery::pe32coff::pe32file(),
        kind: GalleryKind::Pe32File,
    }
}

/// Factory: Unreal Tournament Index parser.
///
/// Returns a [`PyGalleryParser`] wrapping `construct::gallery::ut_index::UTIndex`.
#[pyfunction]
#[pyo3(name = "UTIndex")]
pub fn py_ut_index() -> PyGalleryParser {
    PyGalleryParser {
        inner: Box::new(construct::gallery::ut_index::UTIndex::new()),
        kind: GalleryKind::UTIndex,
    }
}

crate::impl_api_methods!(PyGalleryParser);
crate::impl_construct_operators!(PyGalleryParser);

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use construct::core::context::Context;
    use construct::core::stream::{ByteStream, CombinedStream};

    #[test]
    fn elf_factory_returns_parser() {
        let parser = py_elf();
        // Invalid signature should cause a parse error.
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"NOT-ELF"));
        let mut ctx = Context::new();
        assert!(parser.inner.parse(&mut stream, &mut ctx).is_err());
    }

    #[test]
    fn pe32file_factory_returns_parser() {
        let parser = py_pe32file();
        // Invalid MZ signature should cause a parse error.
        let mut stream = CombinedStream::ByteStream(ByteStream::new_read(b"XXXX"));
        let mut ctx = Context::new();
        assert!(parser.inner.parse(&mut stream, &mut ctx).is_err());
    }

    #[test]
    fn ut_index_factory_returns_parser() {
        let parser = py_ut_index();
        // UTIndex sizeof should fail (variable length).
        let ctx = Context::new();
        assert!(parser.inner.sizeof(&ctx).is_err());
    }

    #[test]
    fn ut_index_parse_basic() {
        let parser = py_ut_index();
        let c: &dyn Construct = parser.inner.as_ref();
        // Value 1 → single byte 0x01
        let result = c.parse_bytes(&[0x01]).unwrap();
        assert_eq!(result, construct::value::Value::Int(1));
    }

    #[test]
    fn gallery_make_owned_produces_independent() {
        let parser = py_ut_index();
        let owned = parser.make_owned().unwrap();
        let c: &dyn Construct = owned.as_ref();
        let result = c.parse_bytes(&[0x01]).unwrap();
        assert_eq!(result, construct::value::Value::Int(1));
    }
}
