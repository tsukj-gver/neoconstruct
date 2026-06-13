//! Benchmarks for gallery (real-world format) parsers.
//!
//! Run with `cargo bench --bench gallery_formats`.
//!
//! These end-to-end benchmarks exercise the full construct tree on
//! real-world binary formats. See `docs/模块设计-性能优化.md` §3.3.5.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};

use construct::core::Construct;
use construct::gallery::elf;
use construct::gallery::pe32coff;
use construct::gallery::ut_index::UTIndex;
use construct::value::Value;

use indexmap::IndexMap;

// ===========================================================================
// UTIndex
// ===========================================================================

fn bench_ut_index(c: &mut Criterion) {
    let mut group = c.benchmark_group("ut_index");
    let construct: &dyn Construct = &UTIndex::new();

    // Build a vector of UTIndex-encoded values of varying byte lengths.
    let values: Vec<i64> = (0..200).map(|i| (i + 1) as i64).collect();
    let encoded: Vec<Vec<u8>> = values
        .iter()
        .map(|&v| construct.build_bytes(&Value::Int(v)).unwrap())
        .collect();
    // Flatten into a single contiguous buffer for a batch parse benchmark.
    let batch: Vec<u8> = encoded.iter().flatten().copied().collect();

    group.throughput(Throughput::Bytes(batch.len() as u64));
    group.bench_function("parse_single", |b| {
        b.iter(|| {
            let _: Value = construct.parse_bytes(black_box(&encoded[10])).unwrap();
        });
    });
    group.bench_function("build_single", |b| {
        b.iter(|| {
            let _: Vec<u8> = construct
                .build_bytes(black_box(&Value::Int(1234567)))
                .unwrap();
        });
    });
    // Sequential parse of the whole batch simulates parsing a stream of
    // UTIndex integers (e.g. an Unreal Tournament index table).
    group.bench_function("parse_batch_200", |b| {
        b.iter(|| {
            let mut pos = 0;
            let data = black_box(&batch);
            while pos < data.len() {
                let val: Value = construct.parse_bytes(&data[pos..]).unwrap();
                // Advance past the bytes this value consumed by rebuilding.
                let rebuilt = construct.build_bytes(&val).unwrap();
                pos += rebuilt.len();
            }
        });
    });
    group.finish();
}

// ===========================================================================
// ELF identifier header (first 16 bytes of an ELF file)
// ===========================================================================

fn make_elf_identifier_data() -> Vec<u8> {
    // A minimal valid ELF32 LSB identifier (16 bytes).
    let mut data = vec![
        0x7f, b'E', b'L', b'F', // signature
        1,    // ELFCLASS32
        1,    // LSB encoding
        1,    // EV_CURRENT
        0,    // ELFOSABI_NONE
        0,    // abiversion
    ];
    data.extend_from_slice(&[0u8; 7]); // padding
    data
}

fn bench_elf_identifier(c: &mut Criterion) {
    let mut group = c.benchmark_group("elf_identifier");
    let construct: &dyn Construct = &elf::identifier();
    let data = make_elf_identifier_data();
    let parsed = construct.parse_bytes(&data).unwrap();

    group.throughput(Throughput::Bytes(data.len() as u64));
    group.bench_function("parse", |b| {
        b.iter(|| {
            let _: Value = construct.parse_bytes(black_box(&data)).unwrap();
        });
    });
    group.bench_function("build", |b| {
        b.iter(|| {
            let _: Vec<u8> = construct.build_bytes(black_box(&parsed)).unwrap();
        });
    });
    group.finish();
}

// ===========================================================================
// PE/COFF MZ DOS header
// ===========================================================================

fn make_msdos_data() -> Vec<u8> {
    // A minimal MZ DOS header: "MZ" signature + pointer to PE header at 0x3C.
    // The msdosheader() construct writes 2 bytes ("MZ") + a Pointer that
    // seeks to 0x3C and writes a 16-bit LE value there.
    let mut data = vec![0u8; 0x3E];
    data[0] = b'M';
    data[1] = b'Z';
    // lfanew at offset 0x3C (little-endian u16) = 0x0040
    data[0x3C] = 0x40;
    data[0x3D] = 0x00;
    data
}

fn bench_pe_msdosheader(c: &mut Criterion) {
    let mut group = c.benchmark_group("pe_msdosheader");
    let construct: &dyn Construct = &pe32coff::msdosheader();
    let data = make_msdos_data();
    // Build a representative parsed value for the build benchmark.
    let parsed = {
        let mut map = IndexMap::new();
        map.insert("signature".to_string(), Value::Bytes(b"MZ".to_vec()));
        map.insert("lfanew".to_string(), Value::UInt(0x40));
        Value::Container(map)
    };

    group.throughput(Throughput::Bytes(data.len() as u64));
    group.bench_function("parse", |b| {
        b.iter(|| {
            let _: Value = construct.parse_bytes(black_box(&data)).unwrap();
        });
    });
    group.bench_function("build", |b| {
        b.iter(|| {
            let _: Vec<u8> = construct.build_bytes(black_box(&parsed)).unwrap();
        });
    });
    group.finish();
}

// ===========================================================================
// Struct (10 fields) — included here as a composite reference point for the
// gallery benchmarks so users can compare against the core_constructs results.
// ===========================================================================

fn bench_struct_composite(c: &mut Criterion) {
    use construct::constructs::format_field::INT8UB;
    use construct::constructs::struct_::Struct;

    let mut group = c.benchmark_group("struct_composite");
    let mut s = Struct::new();
    for i in 0..10 {
        s = s.field(format!("f{i}"), Box::new(INT8UB));
    }
    let construct: &dyn Construct = &s;
    let data: Vec<u8> = (0..10).collect();
    let value = {
        let mut map = IndexMap::new();
        for i in 0..10u8 {
            map.insert(format!("f{i}"), Value::UInt(i as u64));
        }
        Value::Container(map)
    };

    group.throughput(Throughput::Bytes(10));
    group.bench_function("parse_10fields", |b| {
        b.iter(|| {
            let _: Value = construct.parse_bytes(black_box(&data)).unwrap();
        });
    });
    group.bench_function("build_10fields", |b| {
        b.iter(|| {
            let _: Vec<u8> = construct.build_bytes(black_box(&value)).unwrap();
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_ut_index,
    bench_elf_identifier,
    bench_pe_msdosheader,
    bench_struct_composite,
);
criterion_main!(benches);
