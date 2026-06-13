//! Gallery — real-world format parsers ported from Python `construct.gallery`.
//!
//! These modules parse well-known binary formats (ELF, PE/COFF, Unreal
//! Tournament index) and serve as end-to-end validation of the construct-rs
//! library.
//!
//! # Modules
//!
//! | Module | Format | Python source |
//! |--------|--------|---------------|
//! | [`ut_index`] | Unreal Tournament 1999 Index (variable-length integer) | `gallery/ut_index.py` |
//! | [`elf`] | ELF (Executable and Linkable Format) | `gallery/elf.py` |
//! | [`pe32coff`] | PE/COFF (Windows portable executable) | `gallery/pe32coff.py` |

pub mod elf;
pub mod pe32coff;
pub mod ut_index;
