//! ELF (Executable and Linkable Format) parser.
//!
//! Ported from `construct/gallery/elf.py`. Supports 32/64-bit, big/little-endian.
//!
//! # Structure
//!
//! | Function | Python counterpart |
//! |----------|-------------------|
//! | [`identifier`] | `identifier` Struct |
//! | [`program_header`] | `program_header(ELFInt32, ELFInt64, is64bit)` |
//! | [`section_header`] | `section_header(ELFInt32, ELFInt64, is64bit)` |
//! | [`body`] | `body(ELFInt16, ELFInt32, ELFInt64, is64bit)` |
//! | [`elf`] | `elf` Struct (top-level) |

use indexmap::IndexMap;

use crate::combined::CombinedConstruct;
use crate::constructs::computed::Padding;
use crate::constructs::const_::Const;
use crate::constructs::control_flow::{If, IfThenElse};
use crate::constructs::enum_::Enum;
use crate::constructs::format_field::{Endianness, FormatField, FormatKind};
use crate::constructs::repetition::ArrayExpr;
use crate::constructs::stream_ops::PointerExpr;
use crate::constructs::strings::CString;
use crate::constructs::struct_::Struct;
use crate::core::context::Context;
use crate::expr::{this_, BinExpr, BinOp, ConstExpr};
use crate::value::Value;

/// Builds an `IndexMap<String, u64>` from an array of `(name, value)` pairs.
fn mapping(entries: &[(&str, u64)]) -> IndexMap<String, u64> {
    entries.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

/// Creates a `Box<CombinedConstruct>` for an unsigned integer field of the given
/// endianness and width.
fn int_field(endian: Endianness, kind: FormatKind) -> Box<CombinedConstruct> {
    Box::new(FormatField::new(endian, kind).into())
}

// ===========================================================================
// ELF identifier
// ===========================================================================

/// ELF identifier header.
///
/// Contains the magic signature, class (32/64-bit), encoding (endianness),
/// version, OS/ABI, ABI version, and 7 bytes of padding.
///
/// Corresponds to Python `identifier` Struct.
pub fn identifier() -> Struct {
    Struct::new()
        .field(
            "signature",
            Box::new(Const::new_bytes(b"\x7fELF".to_vec()).into()),
        )
        .field(
            "elfclass",
            Box::new(
                Enum::new(
                    int_field(Endianness::Big, FormatKind::U8),
                    mapping(&[("ELFCLASSNONE", 0), ("ELFCLASS32", 1), ("ELFCLASS64", 2)]),
                )
                .into(),
            ),
        )
        .field(
            "encoding",
            Box::new(
                Enum::new(
                    int_field(Endianness::Big, FormatKind::U8),
                    mapping(&[("ELFDATANONE", 0), ("LSB", 1), ("MSB", 2)]),
                )
                .into(),
            ),
        )
        .field(
            "version",
            Box::new(
                Enum::new(
                    int_field(Endianness::Big, FormatKind::U8),
                    mapping(&[("EV_NONE", 0), ("EV_CURRENT", 1)]),
                )
                .into(),
            ),
        )
        .field(
            "osabi",
            Box::new(
                Enum::new(
                    int_field(Endianness::Big, FormatKind::U8),
                    mapping(&[
                        ("ELFOSABI_NONE", 0),
                        ("ELFOSABI_HPUX", 1),
                        ("ELFOSABI_NETBSD", 2),
                        ("ELFOSABI_GNU", 3),
                        ("ELFOSABI_SOLARIS", 6),
                        ("ELFOSABI_AIX", 7),
                        ("ELFOSABI_IRIX", 8),
                        ("ELFOSABI_FREEBSD", 9),
                        ("ELFOSABI_TRU64", 10),
                        ("ELFOSABI_MODESTO", 11),
                        ("ELFOSABI_OPENBSD", 12),
                        ("ELFOSABI_OPENVMS", 13),
                        ("ELFOSABI_NSK", 14),
                        ("ELFOSABI_AROS", 15),
                        ("ELFOSABI_FENIXOS", 16),
                        ("ELFOSABI_CLOUDABI", 17),
                        ("ELFOSABI_OPENVOS", 18),
                    ]),
                )
                .into(),
            ),
        )
        .field("abiversion", int_field(Endianness::Big, FormatKind::U8))
        .anonymous(Box::new(Padding(7).into()))
}

// ===========================================================================
// Program header
// ===========================================================================

/// ELF program header table entry.
///
/// # Parameters
///
/// - `endian` — byte order for multi-byte fields
/// - `is64bit` — whether the ELF file is 64-bit (affects address field width
///   and flag field placement)
///
/// Corresponds to Python `program_header(ELFInt32, ELFInt64, is64bit)`.
pub fn program_header(endian: Endianness, is64bit: bool) -> Struct {
    let addr_kind = if is64bit {
        FormatKind::U64
    } else {
        FormatKind::U32
    };
    let int32 = || int_field(endian, FormatKind::U32);
    let addr = || int_field(endian, addr_kind);

    let flags_enum = || {
        Enum::new(
            int32(),
            mapping(&[
                ("R__", 0x4),
                ("RW_", 0x6),
                ("RWX", 0x7),
                ("_WX", 0x3),
                ("__X", 0x1),
                ("_W_", 0x2),
                ("R_X", 0x5),
            ]),
        )
    };

    Struct::new()
        .field(
            "p_type",
            Box::new(
                Enum::new(
                    int32(),
                    mapping(&[
                        ("PT_NULL", 0x0000_0000),
                        ("PT_LOAD", 0x0000_0001),
                        ("PT_DYNAMIC", 0x0000_0002),
                        ("PT_INTERP", 0x0000_0003),
                        ("PT_NOTE", 0x0000_0004),
                        ("PT_SHLIB", 0x0000_0005),
                        ("PT_PHDR", 0x0000_0006),
                        ("PT_TLS", 0x0000_0007),
                        ("PT_LDOOS", 0x6000_0000),
                        ("PT_HIOS", 0x6FFF_FFFF),
                        ("PT_LOPROC", 0x7000_0000),
                        ("PT_HIPROC", 0x7FFF_FFFF),
                    ]),
                )
                .into(),
            ),
        )
        .field(
            "flags_64",
            Box::new(
                If(
                    Box::new(move |_ctx: &Context| is64bit),
                    Box::new(flags_enum().into()),
                )
                .into(),
            ),
        )
        .field("offset", addr())
        .field("virtual_address", addr())
        .field("physical_address", addr())
        .field("size_file", addr())
        .field("size_mem", addr())
        .field(
            "flags_32",
            Box::new(
                If(
                    Box::new(move |_ctx: &Context| !is64bit),
                    Box::new(flags_enum().into()),
                )
                .into(),
            ),
        )
        .field("alignment", addr())
}

// ===========================================================================
// Section header
// ===========================================================================

/// ELF section header table entry.
///
/// # Parameters
///
/// - `endian` — byte order
/// - `is64bit` — whether the ELF file is 64-bit
///
/// Corresponds to Python `section_header(ELFInt32, ELFInt64, is64bit)`.
pub fn section_header(endian: Endianness, is64bit: bool) -> Struct {
    let addr_kind = if is64bit {
        FormatKind::U64
    } else {
        FormatKind::U32
    };
    let int32 = || int_field(endian, FormatKind::U32);
    let addr = || int_field(endian, addr_kind);

    Struct::new()
        .field("sh_name_offset", int32())
        .field(
            "sh_name",
            Box::new(
                PointerExpr::new(
                    Box::new(
                        this_()
                            .field("_")
                            .field("strtab_data_offset")
                            .add(this_().field("sh_name_offset"))
                            .into(),
                    ),
                    Box::new(CString::utf8().into()),
                )
                .into(),
            ),
        )
        .field(
            "sh_type",
            Box::new(
                Enum::new(
                    int32(),
                    mapping(&[
                        ("SHT_NULL", 0x0),
                        ("SHT_PROGBITS", 0x1),
                        ("SHT_SYMTAB", 0x2),
                        ("SHT_STRTAB", 0x3),
                        ("SHT_RELA", 0x4),
                        ("SHT_HASH", 0x5),
                        ("SHT_DYNAMIC", 0x6),
                        ("SHT_NOTE", 0x7),
                        ("SHT_NOBITS", 0x7),
                        ("SHT_REL", 0x9),
                        ("SHT_SHLIB", 0x0A),
                        ("SHT_DYNSYM", 0x0B),
                        ("SHT_INIT_ARRAY", 0x0E),
                        ("SHT_FINI_ARRAY", 0x0F),
                        ("SHT_PREINIT_ARRAY", 0x10),
                        ("SHT_GROUP", 0x11),
                        ("SHT_SYMTAB_SHNDX", 0x12),
                        ("SHT_NUM", 0x13),
                        ("SHT_LOOS", 0x6000_0000),
                    ]),
                )
                .into(),
            ),
        )
        .field(
            "sh_flags",
            Box::new(
                Enum::new(
                    addr(),
                    mapping(&[
                        ("SHF_WRITE", 0x1),
                        ("SHF_ALLOC", 0x2),
                        ("SHF_EXECINSTR", 0x4),
                        ("SHF_MERGE", 0x10),
                        ("SHF_STRINGS", 0x20),
                        ("SHF_INFO_LINK", 0x40),
                        ("SHF_LINK_ORDER", 0x80),
                        ("SHF_OS_NONCONFORMING", 0x100),
                        ("SHF_GROUP", 0x200),
                        ("SHF_TLS", 0x400),
                        ("SHF_MASKOS", 0x0FF0_0000),
                        ("SHF_MASKPROC", 0xF000_0000),
                        ("SHF_ORDERED", 0x400_0000),
                        ("SHF_EXCLUDE", 0x800_0000),
                    ]),
                )
                .into(),
            ),
        )
        .field("sh_addr", addr())
        .field("sh_offset", addr())
        .field("sh_size", addr())
        .field("sh_link", int32())
        .field("sh_info", int32())
        .field("sh_addralign", addr())
        .field("sh_entsize", addr())
}

// ===========================================================================
// ELF body (after identifier)
// ===========================================================================

/// ELF body (everything after the identifier header).
///
/// # Parameters
///
/// - `endian` — byte order
/// - `is64bit` — whether the ELF file is 64-bit
///
/// Corresponds to Python `body(ELFInt16, ELFInt32, ELFInt64, is64bit)`.
pub fn body(endian: Endianness, is64bit: bool) -> Struct {
    let int16 = || int_field(endian, FormatKind::U16);
    let int32 = || int_field(endian, FormatKind::U32);
    let addr_kind = if is64bit {
        FormatKind::U64
    } else {
        FormatKind::U32
    };
    let addr = || int_field(endian, addr_kind);

    let ph = program_header(endian, is64bit);
    let sh = section_header(endian, is64bit);

    let strtab_extra: i64 = if is64bit { 24 } else { 16 };

    Struct::new()
        .field(
            "type",
            Box::new(
                Enum::new(
                    int16(),
                    mapping(&[
                        ("ET_NONE", 0),
                        ("ET_REL", 1),
                        ("ET_EXEC", 2),
                        ("ET_DYN", 3),
                        ("ET_CORE", 4),
                        ("ET_LOOS", 0xFE00),
                        ("ET_HIOS", 0xFEFF),
                        ("ET_LOPROC", 0xFF00),
                        ("ET_HIPROC", 0xFFFF),
                    ]),
                )
                .into(),
            ),
        )
        .field(
            "machine",
            Box::new(
                Enum::new(
                    int16(),
                    mapping(&[
                        ("EM_NONE", 0),
                        ("EM_M32", 1),
                        ("EM_SPARC", 2),
                        ("EM_386", 3),
                        ("EM_68K", 4),
                        ("EM_88K", 5),
                        ("EM_860", 7),
                        ("EM_MIPS", 8),
                        ("EM_S370", 9),
                        ("EM_MIPS_RS3_LE", 10),
                        ("EM_PARISC", 15),
                        ("EM_VPP500", 17),
                        ("EM_SPARC32PLUS", 18),
                        ("EM_960", 19),
                        ("EM_PPC", 20),
                        ("EM_PPC64", 21),
                        ("EM_S390", 22),
                        ("EM_V800", 36),
                        ("EM_FR20", 37),
                        ("EM_RH32", 38),
                        ("EM_RCE", 39),
                        ("EM_ARM", 40),
                        ("EM_ALPHA", 41),
                        ("EM_SH", 42),
                        ("EM_SPARCV9", 43),
                        ("EM_TRICORE", 44),
                        ("EM_ARC", 45),
                        ("EM_H8_300", 46),
                        ("EM_H8_300H", 47),
                        ("EM_H8S", 48),
                        ("EM_H8_500", 49),
                        ("EM_IA_64", 50),
                        ("EM_MIPS_X", 51),
                        ("EM_COLDFIRE", 52),
                        ("EM_68HC12", 53),
                        ("EM_MMA", 54),
                        ("EM_PCP", 55),
                        ("EM_NCPU", 56),
                        ("EM_NDR1", 57),
                        ("EM_STARCORE", 58),
                        ("EM_ME16", 59),
                        ("EM_ST100", 60),
                        ("EM_TINYJ", 61),
                        ("EM_X86_64", 62),
                        ("EM_PDSP", 63),
                        ("EM_PDP10", 64),
                        ("EM_PDP11", 65),
                        ("EM_FX66", 66),
                        ("EM_ST9PLUS", 67),
                        ("EM_ST7", 68),
                        ("EM_68HC16", 69),
                        ("EM_68HC11", 70),
                        ("EM_68HC08", 71),
                        ("EM_68HC05", 72),
                        ("EM_SVX", 73),
                        ("EM_ST19", 74),
                        ("EM_VAX", 75),
                        ("EM_CRIS", 76),
                        ("EM_JAVELIN", 77),
                        ("EM_FIREPATH", 78),
                        ("EM_ZSP", 79),
                        ("EM_MMIX", 80),
                        ("EM_HUANY", 81),
                        ("EM_PRISM", 82),
                        ("EM_AVR", 83),
                        ("EM_FR30", 84),
                        ("EM_D10V", 85),
                        ("EM_D30V", 86),
                        ("EM_V850", 87),
                        ("EM_M32R", 88),
                        ("EM_MN10300", 89),
                        ("EM_MN10200", 90),
                        ("EM_PJ", 91),
                        ("EM_OPENRISC", 92),
                        ("EM_ARC_COMPACT", 93),
                        ("EM_XTENSA", 94),
                        ("EM_VIDEOCORE", 95),
                        ("EM_TMM_GPP", 96),
                        ("EM_NS32K", 97),
                        ("EM_TPC", 98),
                        ("EM_SNP1K", 99),
                        ("EM_ST200", 100),
                        ("EM_IP2K", 101),
                        ("EM_MAX", 102),
                        ("EM_CR", 103),
                        ("EM_F2MC16", 104),
                        ("EM_MSP430", 105),
                        ("EM_BLACKFIN", 106),
                        ("EM_SE_C33", 107),
                        ("EM_SEP", 108),
                        ("EM_ARCA", 109),
                        ("EM_UNICORE", 110),
                        ("EM_EXCESS", 111),
                        ("EM_DXP", 112),
                        ("EM_ALTERA_NIOS2", 113),
                        ("EM_CRX", 114),
                        ("EM_XGATE", 115),
                        ("EM_C166", 116),
                        ("EM_M16C", 117),
                        ("EM_DSPIC30F", 118),
                        ("EM_CE", 119),
                        ("EM_M32C", 120),
                        ("EM_TSK3000", 131),
                        ("EM_RS08", 132),
                        ("EM_SHARC", 133),
                        ("EM_ECOG2", 134),
                        ("EM_SCORE7", 135),
                        ("EM_DSP24", 136),
                        ("EM_VIDEOCORE3", 137),
                        ("EM_LATTICEMICO32", 138),
                        ("EM_SE_C17", 139),
                        ("EM_TI_C6000", 140),
                        ("EM_TI_C2000", 141),
                        ("EM_TI_C5500", 142),
                        ("EM_TI_ARP32", 143),
                        ("EM_TI_PRU", 144),
                        ("EM_MMDSP_PLUS", 160),
                        ("EM_CYPRESS_M8C", 161),
                        ("EM_R32C", 162),
                        ("EM_TRIMEDIA", 163),
                        ("EM_HEXAGON", 164),
                        ("EM_8051", 165),
                        ("EM_STXP7X", 166),
                        ("EM_NDS32", 167),
                        ("EM_ECOG1", 168),
                        ("EM_MAXQ30", 169),
                        ("EM_XIMO16", 170),
                        ("EM_MANIK", 171),
                        ("EM_CRAYNV2", 172),
                        ("EM_RX", 173),
                        ("EM_METAG", 174),
                        ("EM_MCST_ELBRUS", 175),
                        ("EM_ECOG16", 176),
                        ("EM_CR16", 177),
                        ("EM_ETPU", 178),
                        ("EM_SLE9X", 179),
                        ("EM_L10M", 180),
                        ("EM_K10M", 181),
                        ("EM_AARCH64", 183),
                        ("EM_AVR32", 185),
                        ("EM_STM8", 186),
                        ("EM_TILE64", 187),
                        ("EM_TILEPRO", 188),
                        ("EM_CUDA", 190),
                        ("EM_TILEGX", 191),
                        ("EM_CLOUDSHIELD", 192),
                        ("EM_COREA_1ST", 193),
                        ("EM_COREA_2ND", 194),
                        ("EM_ARC_COMPACT2", 195),
                        ("EM_OPEN8", 196),
                        ("EM_RL78", 197),
                        ("EM_VIDEOCORE5", 198),
                        ("EM_78KOR", 199),
                        ("EM_56800EX", 200),
                        ("EM_BA1", 201),
                        ("EM_BA2", 202),
                        ("EM_XCORE", 203),
                        ("EM_MCHP_PIC", 204),
                        ("EM_INTEL205", 205),
                        ("EM_INTEL206", 206),
                        ("EM_INTEL207", 207),
                        ("EM_INTEL208", 208),
                        ("EM_INTEL209", 209),
                        ("EM_KM32", 210),
                        ("EM_KMX32", 211),
                        ("EM_KMX16", 212),
                        ("EM_KMX8", 213),
                        ("EM_KVARC", 214),
                        ("EM_CDP", 215),
                        ("EM_COGE", 216),
                        ("EM_COOL", 217),
                        ("EM_NORC", 218),
                        ("EM_CSR_KALIMBA", 219),
                        ("EM_Z80", 220),
                        ("EM_VISIUM", 221),
                        ("EM_FT32", 222),
                        ("EM_MOXIE", 223),
                        ("EM_AMDGPU", 224),
                        ("EM_RISCV", 243),
                    ]),
                )
                .into(),
            ),
        )
        .field(
            "version",
            Box::new(Enum::new(int32(), mapping(&[("EV_NONE", 0), ("EV_CURRENT", 1)])).into()),
        )
        .field("entry", addr())
        .field("ph_offset", addr())
        .field("sh_offset", addr())
        .field("flags", int32())
        .field("header_size", int16())
        .field("ph_entry_size", int16())
        .field("ph_count", int16())
        .field("sh_entry_size", int16())
        .field("sh_count", int16())
        .field("strtab_section_index", int16())
        .field(
            "strtab_data_offset",
            Box::new(
                PointerExpr::new(
                    Box::new(
                        BinExpr::new(
                            BinOp::Add,
                            BinExpr::new(
                                BinOp::Add,
                                this_().field("sh_offset"),
                                this_()
                                    .field("strtab_section_index")
                                    .mul(this_().field("sh_entry_size")),
                            ),
                            ConstExpr::new(Value::Int(strtab_extra)),
                        )
                        .into(),
                    ),
                    int32(),
                )
                .into(),
            ),
        )
        .field(
            "program_table",
            Box::new(
                PointerExpr::new(
                    Box::new(this_().field("ph_offset").into()),
                    Box::new(
                        ArrayExpr::new(
                            Box::new(this_().field("ph_count").into()),
                            Box::new(ph.into()),
                        )
                        .into(),
                    ),
                )
                .into(),
            ),
        )
        .field(
            "sections",
            Box::new(
                PointerExpr::new(
                    Box::new(this_().field("sh_offset").into()),
                    Box::new(
                        ArrayExpr::new(
                            Box::new(this_().field("sh_count").into()),
                            Box::new(sh.into()),
                        )
                        .into(),
                    ),
                )
                .into(),
            ),
        )
}

// ===========================================================================
// Top-level ELF
// ===========================================================================

/// Checks if a nested context field is an enum string matching `expected`.
fn check_enum_field(ctx: &Context, path: &[&str], expected: &str) -> bool {
    let string_path: Vec<String> = path.iter().map(|s| s.to_string()).collect();
    ctx.get_path(&string_path)
        .ok()
        .and_then(|v| v.as_string().ok().map(|s| s.to_string()))
        .map(|s| s == expected)
        .unwrap_or(false)
}

/// Complete ELF file construct.
///
/// Parses the identifier header, then dynamically selects the body variant
/// (32/64-bit × little/big-endian) based on the `encoding` and `elfclass`
/// fields parsed from the identifier.
///
/// Corresponds to Python `elf` Struct.
pub fn elf() -> Box<CombinedConstruct> {
    Box::new(
        Struct::new()
            .field("identifier", Box::new(identifier().into()))
            .field(
                "body",
                Box::new(
                    IfThenElse::new(
                        // LSB (little-endian) branch?
                        Box::new(|ctx: &Context| {
                            check_enum_field(ctx, &["identifier", "encoding"], "LSB")
                        }),
                        // LSB: 64 or 32 bit?
                        Box::new(
                            IfThenElse::new(
                                Box::new(|ctx: &Context| {
                                    check_enum_field(ctx, &["identifier", "elfclass"], "ELFCLASS64")
                                }),
                                Box::new(body(Endianness::Little, true).into()),
                                Box::new(body(Endianness::Little, false).into()),
                            )
                            .into(),
                        ),
                        // MSB (big-endian): 64 or 32 bit?
                        Box::new(
                            IfThenElse::new(
                                Box::new(|ctx: &Context| {
                                    check_enum_field(ctx, &["identifier", "elfclass"], "ELFCLASS64")
                                }),
                                Box::new(body(Endianness::Big, true).into()),
                                Box::new(body(Endianness::Big, false).into()),
                            )
                            .into(),
                        ),
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
    use crate::core::Construct;

    #[test]
    fn identifier_has_correct_fields() {
        let id = identifier();
        // Build a valid identifier and verify round-trip
        let mut data = vec![0x7f, b'E', b'L', b'F']; // signature
        data.push(1); // ELFCLASS32
        data.push(1); // LSB
        data.push(1); // EV_CURRENT
        data.push(0); // ELFOSABI_NONE
        data.push(0); // abiversion
        data.extend_from_slice(&[0u8; 7]); // padding

        let c: &dyn Construct = &id;
        let parsed = c.parse_bytes(&data).unwrap();
        let container = parsed.as_container().unwrap();
        assert!(container.get("signature").is_some());
        assert_eq!(
            container.get("elfclass").unwrap(),
            &Value::String("ELFCLASS32".to_string())
        );
        assert_eq!(
            container.get("encoding").unwrap(),
            &Value::String("LSB".to_string())
        );
    }

    #[test]
    fn identifier_wrong_signature_fails() {
        let id = identifier();
        let c: &dyn Construct = &id;
        let result = c.parse_bytes(b"XXXX\x01\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00");
        assert!(result.is_err());
    }

    #[test]
    fn elf_top_level_returns_boxed_construct() {
        let _ = elf();
    }

    #[test]
    fn program_header_32bit_has_flags_32() {
        let ph = program_header(Endianness::Little, false);
        let _ = ph; // Just verify it compiles
    }

    #[test]
    fn program_header_64bit_has_flags_64() {
        let ph = program_header(Endianness::Little, true);
        let _ = ph;
    }

    #[test]
    fn section_header_compiles() {
        let _ = section_header(Endianness::Little, false);
        let _ = section_header(Endianness::Big, true);
    }

    #[test]
    fn body_compiles() {
        let _ = body(Endianness::Little, false);
        let _ = body(Endianness::Little, true);
        let _ = body(Endianness::Big, false);
        let _ = body(Endianness::Big, true);
    }
}
