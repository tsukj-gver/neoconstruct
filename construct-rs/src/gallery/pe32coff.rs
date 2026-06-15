//! PE/COFF (Windows Portable Executable) format parser.
//!
//! Ported from `construct/gallery/pe32coff.py`. Supports PE32 and PE32+ formats.
//!
//! # Structure
//!
//! | Function | Python counterpart |
//! |----------|-------------------|
//! | [`msdosheader`] | `msdosheader` Struct |
//! | [`coffheader`] | `coffheader` Struct |
//! | [`datadirectory`] | `datadirectory` Struct |
//! | [`optionalheader`] | `optionalheader` Struct |
//! | [`section`] | `section` Struct |
//! | [`pe32file`] | `pe32file` Struct (top-level) |

use indexmap::IndexMap;

use crate::combined::CombinedConstruct;
use crate::constructs::bytes::BytesExpr;
use crate::constructs::computed::{Computed, TimestampAdapter, TimestampEpoch, TimestampUnit};
use crate::constructs::const_::Const;
use crate::constructs::control_flow::{If, IfThenElse};
use crate::constructs::enum_::{Enum, FlagsEnum};
use crate::constructs::format_field::{INT16UL, INT32UL, INT64UL, INT8UL};
use crate::constructs::meta::SeekExpr;
use crate::constructs::repetition::ArrayExpr;
use crate::constructs::stream_ops::{Pointer, PointerExpr};
use crate::constructs::strings::PaddedString;
use crate::constructs::struct_::Struct;
use crate::core::context::Context;
use crate::core::error::{ConstructError, Result};
use crate::expr::{this_, CondExpr, ConstExpr};
use crate::value::Value;

/// Builds an `IndexMap<String, u64>` from an array of `(name, value)` pairs.
fn mapping(entries: &[(&str, u64)]) -> IndexMap<String, u64> {
    entries.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

/// Checks if a context field is an enum string matching `expected`.
fn check_enum_field(ctx: &Context, field: &str, expected: &str) -> bool {
    ctx.get(field)
        .and_then(|v| v.as_string().ok().map(|s| s.to_string()))
        .map(|s| s == expected)
        .unwrap_or(false)
}

// ===========================================================================
// MZ DOS header
// ===========================================================================

/// MZ DOS header.
///
/// Contains the MZ signature and a pointer (`lfanew`) to the PE header at
/// fixed offset 0x3C.
///
/// Corresponds to Python `msdosheader`.
pub fn msdosheader() -> Struct {
    Struct::new()
        .field(
            "signature",
            Box::new(Const::new_bytes(b"MZ".to_vec()).into()),
        )
        .field(
            "lfanew",
            Box::new(Pointer::new(0x3C, Box::new(INT16UL.into())).into()),
        )
}

// ===========================================================================
// COFF header
// ===========================================================================

/// COFF header.
///
/// Contains the PE signature, machine type, section count, timestamp, symbol
/// table info, optional header size, and characteristics flags.
///
/// Corresponds to Python `coffheader`.
pub fn coffheader() -> Struct {
    Struct::new()
        .field(
            "signature",
            Box::new(Const::new_bytes(b"PE\x00\x00".to_vec()).into()),
        )
        .field(
            "machine",
            Box::new(
                Enum::new(
                    Box::new(INT16UL.into()),
                    mapping(&[
                        ("UNKNOWN", 0x0),
                        ("AM33", 0x1d3),
                        ("AMD64", 0x8664),
                        ("ARM", 0x1c0),
                        ("ARM64", 0xaa64),
                        ("ARMNT", 0x1c4),
                        ("EBC", 0xebc),
                        ("I386", 0x14c),
                        ("IA64", 0x200),
                        ("M32R", 0x9041),
                        ("MIPS16", 0x266),
                        ("MIPSFPU", 0x366),
                        ("MIPSFPU16", 0x466),
                        ("POWERPC", 0x1f0),
                        ("POWERPCFP", 0x1f1),
                        ("R4000", 0x166),
                        ("RISCV32", 0x5032),
                        ("RISCV64", 0x5064),
                        ("RISCV128", 0x5128),
                        ("SH3", 0x1a2),
                        ("SH3DSP", 0x1a3),
                        ("SH4", 0x1a6),
                        ("SH5", 0x1a8),
                        ("THUMB", 0x1c2),
                        ("WCEMIPSV2", 0x169),
                    ]),
                )
                .into(),
            ),
        )
        .field("sections_count", Box::new(INT16UL.into()))
        .field(
            "created",
            Box::new(
                TimestampAdapter::new(
                    Box::new(INT32UL.into()),
                    TimestampUnit::Seconds,
                    TimestampEpoch::Unix,
                )
                .into(),
            ),
        )
        .field("symbol_pointer", Box::new(INT32UL.into()))
        .field("symbol_count", Box::new(INT32UL.into()))
        .field("optionalheader_size", Box::new(INT16UL.into()))
        .field(
            "characteristics",
            Box::new(
                FlagsEnum::new(
                    Box::new(INT16UL.into()),
                    mapping(&[
                        ("RELOCS_STRIPPED", 0x0001),
                        ("EXECUTABLE_IMAGE", 0x0002),
                        ("LINE_NUMS_STRIPPED", 0x0004),
                        ("LOCAL_SYMS_STRIPPED", 0x0008),
                        ("AGGRESSIVE_WS_TRIM", 0x0010),
                        ("LARGE_ADDRESS_AWARE", 0x0020),
                        ("RESERVED", 0x0040),
                        ("BYTES_REVERSED_LO", 0x0080),
                        ("MACHINE_32BIT", 0x0100),
                        ("DEBUG_STRIPPED", 0x0200),
                        ("REMOVABLE_RUN_FROM_SWAP", 0x0400),
                        ("SYSTEM", 0x1000),
                        ("DLL", 0x2000),
                        ("UNIPROCESSOR_ONLY", 0x4000),
                        ("BIG_ENDIAN_MACHINE", 0x8000),
                    ]),
                )
                .into(),
            ),
        )
}

// ===========================================================================
// Data directory
// ===========================================================================

/// Static lookup table for data directory entry names.
const ENTRIES_NAMES: [(u64, &str); 16] = [
    (0, "export_table"),
    (1, "import_table"),
    (2, "resource_table"),
    (3, "exception_table"),
    (4, "certificate_table"),
    (5, "base_relocation_table"),
    (6, "debug"),
    (7, "architecture"),
    (8, "global_ptr"),
    (9, "tls_table"),
    (10, "load_config_table"),
    (11, "bound_import"),
    (12, "import_address_table"),
    (13, "delay_import_descriptor"),
    (14, "clr_runtime_header"),
    (15, "reserved"),
];

/// Computes the name for a data directory entry from the repetition index.
fn compute_datadirectory_name(ctx: &Context) -> Result<Value> {
    let index = ctx
        .get("_index")
        .and_then(|v| v.to_u64().ok())
        .ok_or_else(|| ConstructError::FieldMissing {
            path: String::new(),
            field: "_index".to_string(),
        })?;
    let name = ENTRIES_NAMES
        .iter()
        .find(|(i, _)| *i == index)
        .map(|(_, n)| *n)
        .unwrap_or("unknown");
    Ok(Value::String(name.to_string()))
}

/// Data directory entry.
///
/// The `name` field is computed from the repetition index using
/// [`ENTRIES_NAMES`].
///
/// Corresponds to Python `datadirectory`.
pub fn datadirectory() -> Struct {
    Struct::new()
        .field(
            "name",
            Box::new(Computed::new(Box::new(compute_datadirectory_name)).into()),
        )
        .field("virtualaddress", Box::new(INT32UL.into()))
        .field("size", Box::new(INT32UL.into()))
}

// ===========================================================================
// Optional header
// ===========================================================================

/// Returns a construct that selects between PE32 (32-bit) and PE32+ (64-bit)
/// based on the `signature` field in the current context.
fn plusfield() -> Box<CombinedConstruct> {
    Box::new(
        IfThenElse::new(
            Box::new(|ctx: &Context| check_enum_field(ctx, "signature", "PE32plus")),
            Box::new(INT64UL.into()),
            Box::new(INT32UL.into()),
        )
        .into(),
    )
}

/// PE optional header.
///
/// Contains linker version, code/data sizes, entry point, image base, alignment,
/// versions, subsystem, stack/heap sizes, and data directories.
///
/// Corresponds to Python `optionalheader`.
pub fn optionalheader() -> Struct {
    Struct::new()
        .field(
            "signature",
            Box::new(
                Enum::new(
                    Box::new(INT16UL.into()),
                    mapping(&[("PE32", 0x10b), ("PE32plus", 0x20b), ("ROMIMAGE", 0x107)]),
                )
                .into(),
            ),
        )
        .field(
            "linker_version",
            Box::new(
                ArrayExpr::new(
                    Box::new(ConstExpr::new(Value::UInt(2)).into()),
                    Box::new(INT8UL.into()),
                )
                .into(),
            ),
        )
        .field("size_code", Box::new(INT32UL.into()))
        .field("size_initialized_data", Box::new(INT32UL.into()))
        .field("size_uninitialized_data", Box::new(INT32UL.into()))
        .field("entrypoint", Box::new(INT32UL.into()))
        .field("base_code", Box::new(INT32UL.into()))
        .field(
            "base_data",
            Box::new(
                If(
                    Box::new(|ctx: &Context| check_enum_field(ctx, "signature", "PE32")),
                    Box::new(INT32UL.into()),
                )
                .into(),
            ),
        )
        .field("image_base", plusfield())
        .field("section_alignment", Box::new(INT32UL.into()))
        .field("file_alignment", Box::new(INT32UL.into()))
        .field(
            "os_version",
            Box::new(
                ArrayExpr::new(
                    Box::new(ConstExpr::new(Value::UInt(2)).into()),
                    Box::new(INT16UL.into()),
                )
                .into(),
            ),
        )
        .field(
            "image_version",
            Box::new(
                ArrayExpr::new(
                    Box::new(ConstExpr::new(Value::UInt(2)).into()),
                    Box::new(INT16UL.into()),
                )
                .into(),
            ),
        )
        .field(
            "subsystem_version",
            Box::new(
                ArrayExpr::new(
                    Box::new(ConstExpr::new(Value::UInt(2)).into()),
                    Box::new(INT16UL.into()),
                )
                .into(),
            ),
        )
        .field("win32versionvalue", Box::new(INT32UL.into()))
        .field("image_size", Box::new(INT32UL.into()))
        .field("headers_size", Box::new(INT32UL.into()))
        .field("checksum", Box::new(INT32UL.into()))
        .field(
            "subsystem",
            Box::new(
                Enum::new(
                    Box::new(INT16UL.into()),
                    mapping(&[
                        ("UNKNOWN", 0),
                        ("NATIVE", 1),
                        ("WINDOWS_GUI", 2),
                        ("WINDOWS_CUI", 3),
                        ("OS2_CUI", 5),
                        ("POSIX_CUI", 7),
                        ("WINDOWS_NATIVE", 8),
                        ("WINDOWS_CE_GUI", 9),
                        ("EFI_APPLICATION", 10),
                        ("EFI_BOOT_SERVICE_DRIVER", 11),
                        ("EFI_RUNTIME_DRIVER", 12),
                        ("EFI_ROM", 13),
                        ("XBOX", 14),
                        ("WINDOWS_BOOT_APPLICATION", 16),
                    ]),
                )
                .into(),
            ),
        )
        .field(
            "dll_characteristics",
            Box::new(
                FlagsEnum::new(
                    Box::new(INT16UL.into()),
                    mapping(&[
                        ("HIGH_ENTROPY_VA", 0x0020),
                        ("DYNAMIC_BASE", 0x0040),
                        ("FORCE_INTEGRITY", 0x0080),
                        ("NX_COMPAT", 0x0100),
                        ("NO_ISOLATION", 0x0200),
                        ("NO_SEH", 0x0400),
                        ("NO_BIND", 0x0800),
                        ("APPCONTAINER", 0x1000),
                        ("WDM_DRIVER", 0x2000),
                        ("GUARD_CF", 0x4000),
                        ("TERMINAL_SERVER_AWARE", 0x8000),
                    ]),
                )
                .into(),
            ),
        )
        .field("stack_reserve", plusfield())
        .field("stack_commit", plusfield())
        .field("heap_reserve", plusfield())
        .field("heap_commit", plusfield())
        .field("loader_flags", Box::new(INT32UL.into()))
        .field("datadirectories_count", Box::new(INT32UL.into()))
        .field(
            "datadirectories",
            Box::new(
                ArrayExpr::new(
                    Box::new(this_().field("datadirectories_count").into()),
                    Box::new(datadirectory().into()),
                )
                .into(),
            ),
        )
}

// ===========================================================================
// Section header
// ===========================================================================

/// Section header table entry.
///
/// Contains the section name, virtual/raw sizes and addresses, relocation and
/// line number info, characteristics flags, and pointer-referenced raw data,
/// relocations, and line numbers.
///
/// Corresponds to Python `section`.
pub fn section() -> Struct {
    Struct::new()
        .field("name", Box::new(PaddedString::utf8(8).into()))
        .field("virtual_size", Box::new(INT32UL.into()))
        .field("virtual_address", Box::new(INT32UL.into()))
        .field("rawdata_size", Box::new(INT32UL.into()))
        .field("rawdata_pointer", Box::new(INT32UL.into()))
        .field("relocations_pointer", Box::new(INT32UL.into()))
        .field("linenumbers_pointer", Box::new(INT32UL.into()))
        .field("relocations_count", Box::new(INT16UL.into()))
        .field("linenumbers_count", Box::new(INT16UL.into()))
        .field(
            "characteristics",
            Box::new(
                FlagsEnum::new(
                    Box::new(INT32UL.into()),
                    mapping(&[
                        ("TYPE_REG", 0x0000_0000),
                        ("TYPE_DSECT", 0x0000_0001),
                        ("TYPE_NOLOAD", 0x0000_0002),
                        ("TYPE_GROUP", 0x0000_0004),
                        ("TYPE_NO_PAD", 0x0000_0008),
                        ("TYPE_COPY", 0x0000_0010),
                        ("CNT_CODE", 0x0000_0020),
                        ("CNT_INITIALIZED_DATA", 0x0000_0040),
                        ("CNT_UNINITIALIZED_DATA", 0x0000_0080),
                        ("LNK_OTHER", 0x0000_0100),
                        ("LNK_INFO", 0x0000_0200),
                        ("TYPE_OVER", 0x0000_0400),
                        ("LNK_REMOVE", 0x0000_0800),
                        ("LNK_COMDAT", 0x0000_1000),
                        ("MEM_FARDATA", 0x0000_8000),
                        ("MEM_PURGEABLE", 0x0002_0000),
                        ("MEM_16BIT", 0x0002_0000),
                        ("MEM_LOCKED", 0x0004_0000),
                        ("MEM_PRELOAD", 0x0008_0000),
                        ("ALIGN_1BYTES", 0x0010_0000),
                        ("ALIGN_2BYTES", 0x0020_0000),
                        ("ALIGN_4BYTES", 0x0030_0000),
                        ("ALIGN_8BYTES", 0x0040_0000),
                        ("ALIGN_16BYTES", 0x0050_0000),
                        ("ALIGN_32BYTES", 0x0060_0000),
                        ("ALIGN_64BYTES", 0x0070_0000),
                        ("ALIGN_128BYTES", 0x0080_0000),
                        ("ALIGN_256BYTES", 0x0090_0000),
                        ("ALIGN_512BYTES", 0x00A0_0000),
                        ("ALIGN_1024BYTES", 0x00B0_0000),
                        ("ALIGN_2048BYTES", 0x00C0_0000),
                        ("ALIGN_4096BYTES", 0x00D0_0000),
                        ("ALIGN_8192BYTES", 0x00E0_0000),
                        ("LNK_NRELOC_OVFL", 0x0100_0000),
                        ("MEM_DISCARDABLE", 0x0200_0000),
                        ("MEM_NOT_CACHED", 0x0400_0000),
                        ("MEM_NOT_PAGED", 0x0800_0000),
                        ("MEM_SHARED", 0x1000_0000),
                        ("MEM_EXECUTE", 0x2000_0000),
                        ("MEM_READ", 0x4000_0000),
                        ("MEM_WRITE", 0x8000_0000),
                    ]),
                )
                .into(),
            ),
        )
        .field(
            "rawdata",
            Box::new(
                PointerExpr::new(
                    Box::new(this_().field("rawdata_pointer").into()),
                    Box::new(
                        BytesExpr::new(Box::new(
                            CondExpr::new(
                                this_()
                                    .field("rawdata_pointer")
                                    .gt(ConstExpr::new(Value::UInt(0))),
                                this_().field("rawdata_size"),
                                ConstExpr::new(Value::UInt(0)),
                            )
                            .into(),
                        ))
                        .into(),
                    ),
                )
                .into(),
            ),
        )
        .field(
            "relocations",
            Box::new(
                PointerExpr::new(
                    Box::new(this_().field("relocations_pointer").into()),
                    Box::new(
                        ArrayExpr::new(
                            Box::new(this_().field("relocations_count").into()),
                            Box::new(
                                Struct::new()
                                    .field("virtualaddress", Box::new(INT32UL.into()))
                                    .field("symboltable_index", Box::new(INT32UL.into()))
                                    .field("type", Box::new(INT16UL.into()))
                                    .into(),
                            ),
                        )
                        .into(),
                    ),
                )
                .into(),
            ),
        )
        .field(
            "linenumbers",
            Box::new(
                PointerExpr::new(
                    Box::new(this_().field("linenumbers_pointer").into()),
                    Box::new(
                        ArrayExpr::new(
                            Box::new(this_().field("linenumbers_count").into()),
                            Box::new(
                                Struct::new()
                                    .field("_type", Box::new(INT32UL.into()))
                                    .field("_linenumber", Box::new(INT16UL.into()))
                                    .into(),
                            ),
                        )
                        .into(),
                    ),
                )
                .into(),
            ),
        )
}

// ===========================================================================
// Top-level PE32 file
// ===========================================================================

/// Computes `sections_count` from `coffheader.sections_count`.
fn compute_sections_count(ctx: &Context) -> Result<Value> {
    let string_path: Vec<String> = vec!["coffheader".to_string(), "sections_count".to_string()];
    ctx.get_path(&string_path)
        .cloned()
        .map_err(|e| e.with_path_prefix("sections_count"))
}

/// Complete PE32 file construct.
///
/// Parses the MZ DOS header, seeks to the PE header, parses the COFF header,
/// optional header (if present), and section table.
///
/// Corresponds to Python `pe32file`.
pub fn pe32file() -> Box<CombinedConstruct> {
    Box::new(
        Struct::new()
            .field("msdosheader", Box::new(msdosheader().into()))
            .anonymous(Box::new(
                SeekExpr::new(Box::new(
                    this_().field("msdosheader").field("lfanew").into(),
                ))
                .into(),
            ))
            .field("coffheader", Box::new(coffheader().into()))
            .field(
                "optionalheader",
                Box::new(
                    If(
                        Box::new(|ctx: &Context| {
                            ctx.get_path(&[
                                "coffheader".to_string(),
                                "optionalheader_size".to_string(),
                            ])
                            .ok()
                            .and_then(|v| v.to_u64().ok())
                            .map(|v| v > 0)
                            .unwrap_or(false)
                        }),
                        Box::new(optionalheader().into()),
                    )
                    .into(),
                ),
            )
            .field(
                "sections_count",
                Box::new(Computed::new(Box::new(compute_sections_count)).into()),
            )
            .field(
                "sections",
                Box::new(
                    ArrayExpr::new(
                        Box::new(this_().field("sections_count").into()),
                        Box::new(section().into()),
                    )
                    .into(),
                ),
            )
            .into(),
    )
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msdosheader_compiles() {
        let _ = msdosheader();
    }

    #[test]
    fn coffheader_compiles() {
        let _ = coffheader();
    }

    #[test]
    fn datadirectory_compiles() {
        let _ = datadirectory();
    }

    #[test]
    fn optionalheader_compiles() {
        let _ = optionalheader();
    }

    #[test]
    fn section_compiles() {
        let _ = section();
    }

    #[test]
    fn pe32file_compiles() {
        let _ = pe32file();
    }

    #[test]
    fn datadirectory_name_lookup() {
        let mut ctx = Context::new();
        ctx.insert("_index", Value::UInt(0));
        let result = compute_datadirectory_name(&ctx).unwrap();
        assert_eq!(result, Value::String("export_table".to_string()));

        ctx.insert("_index", Value::UInt(5));
        let result = compute_datadirectory_name(&ctx).unwrap();
        assert_eq!(result, Value::String("base_relocation_table".to_string()));
    }

    #[test]
    fn datadirectory_name_unmapped_returns_unknown() {
        let mut ctx = Context::new();
        ctx.insert("_index", Value::UInt(99));
        let result = compute_datadirectory_name(&ctx).unwrap();
        assert_eq!(result, Value::String("unknown".to_string()));
    }

    #[test]
    fn datadirectory_name_missing_index_returns_error() {
        let ctx = Context::new();
        let result = compute_datadirectory_name(&ctx);
        assert!(result.is_err());
    }

    #[test]
    fn entries_names_table_has_16_entries() {
        assert_eq!(ENTRIES_NAMES.len(), 16);
    }

    #[test]
    fn check_enum_field_works() {
        let mut ctx = Context::new();
        ctx.insert("sig", Value::String("PE32plus".to_string()));
        assert!(check_enum_field(&ctx, "sig", "PE32plus"));
        assert!(!check_enum_field(&ctx, "sig", "PE32"));
        assert!(!check_enum_field(&ctx, "missing", "PE32"));
    }
}
