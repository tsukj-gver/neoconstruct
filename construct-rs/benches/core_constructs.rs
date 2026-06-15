//! Benchmarks for the core (atomic + composite) constructs.
//!
//! Run with `cargo bench --bench core_constructs`.
//!
//! Covers the parse and build directions of the most commonly used
//! constructs. See `docs/模块设计-性能优化.md` §3.3 for the design matrix.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use construct::constructs::bytes::Bytes;
use construct::constructs::enum_::Enum;
use construct::constructs::format_field::{INT32UB, INT8UB};
use construct::constructs::repetition::Array;
use construct::constructs::struct_::Struct;
use construct::constructs::varint::VarInt;
use construct::core::Construct;
use construct::value::Value;

use indexmap::IndexMap;

// ===========================================================================
// Atomic constructs
// ===========================================================================

fn bench_bytes(c: &mut Criterion) {
    let mut group = c.benchmark_group("bytes");
    let length: usize = 100;
    let data = vec![0x42u8; length];
    let value = Value::Bytes(data.clone());
    let construct: &dyn Construct = &Bytes::new(length);

    group.throughput(Throughput::Bytes(length as u64));
    group.bench_function("parse_100", |b| {
        b.iter(|| {
            let _: Value = construct.parse_bytes(black_box(&data)).unwrap();
        });
    });
    group.bench_function("build_100", |b| {
        b.iter(|| {
            let _: Vec<u8> = construct.build_bytes(black_box(&value)).unwrap();
        });
    });
    group.finish();
}

fn bench_format_field(c: &mut Criterion) {
    let mut group = c.benchmark_group("format_field");
    let data: [u8; 4] = [0x00, 0x00, 0x00, 0x2A];
    let value = Value::UInt(42);
    let construct: &dyn Construct = &INT32UB;

    group.throughput(Throughput::Bytes(4));
    group.bench_function("int32ub_parse", |b| {
        b.iter(|| {
            let _: Value = construct.parse_bytes(black_box(&data)).unwrap();
        });
    });
    group.bench_function("int32ub_build", |b| {
        b.iter(|| {
            let _: Vec<u8> = construct.build_bytes(black_box(&value)).unwrap();
        });
    });
    group.finish();
}

fn bench_varint(c: &mut Criterion) {
    let mut group = c.benchmark_group("varint");
    let construct: &dyn Construct = &VarInt::new();
    // 300 encodes to two LEB128 bytes (0xAC 0x02).
    let data: Vec<u8> = vec![0xAC, 0x02];
    let value = Value::UInt(300);

    group.throughput(Throughput::Bytes(2));
    group.bench_function("parse", |b| {
        b.iter(|| {
            let _: Value = construct.parse_bytes(black_box(&data)).unwrap();
        });
    });
    group.bench_function("build", |b| {
        b.iter(|| {
            let _: Vec<u8> = construct.build_bytes(black_box(&value)).unwrap();
        });
    });
    group.finish();
}

// ===========================================================================
// Composite constructs
// ===========================================================================

/// Builds a 10-field Struct of `INT8UB` for benchmarking.
fn make_struct_10fields() -> Struct {
    let mut s = Struct::new();
    for i in 0..10 {
        s = s.field(format!("f{i}"), Box::new(INT8UB.into()));
    }
    s
}

fn bench_struct(c: &mut Criterion) {
    let mut group = c.benchmark_group("struct");
    let construct: &dyn Construct = &make_struct_10fields();
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

fn bench_array(c: &mut Criterion) {
    let mut group = c.benchmark_group("array");
    let count = 100;
    let construct: &dyn Construct = &Array::new(count, Box::new(INT8UB.into()));
    let data: Vec<u8> = (0..count as u8).collect();
    let value = Value::List((0..count).map(|i| Value::UInt(i as u64)).collect());

    group.throughput(Throughput::Bytes(count as u64));
    group.bench_function("parse_100", |b| {
        b.iter(|| {
            let _: Value = construct.parse_bytes(black_box(&data)).unwrap();
        });
    });
    group.bench_function("build_100", |b| {
        b.iter(|| {
            let _: Vec<u8> = construct.build_bytes(black_box(&value)).unwrap();
        });
    });
    group.finish();
}

fn bench_enum(c: &mut Criterion) {
    let mut group = c.benchmark_group("enum");
    let mut mapping: IndexMap<String, u64> = IndexMap::new();
    mapping.insert("a".to_string(), 0);
    mapping.insert("b".to_string(), 1);
    mapping.insert("c".to_string(), 2);
    let construct: &dyn Construct = &Enum::new(Box::new(INT8UB.into()), mapping);
    let data: [u8; 1] = [0x01];
    let value = Value::String("b".to_string());

    group.throughput(Throughput::Bytes(1));
    group.bench_function("parse", |b| {
        b.iter(|| {
            let _: Value = construct.parse_bytes(black_box(&data)).unwrap();
        });
    });
    group.bench_function("build", |b| {
        b.iter(|| {
            let _: Vec<u8> = construct.build_bytes(black_box(&value)).unwrap();
        });
    });
    group.finish();
}

// ===========================================================================
// Scaled Bytes benchmark (allocations)
// ===========================================================================

fn bench_bytes_scaled(c: &mut Criterion) {
    let mut group = c.benchmark_group("bytes_scaled");
    for &size in &[64usize, 4096, 65536] {
        let data = vec![0x42u8; size];
        let value = Value::Bytes(data.clone());
        let construct: &dyn Construct = &Bytes::new(size);
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::new("parse", size), &size, |b, _| {
            b.iter(|| {
                let _: Value = construct.parse_bytes(black_box(&data)).unwrap();
            });
        });
        group.bench_with_input(BenchmarkId::new("build", size), &size, |b, _| {
            b.iter(|| {
                let _: Vec<u8> = construct.build_bytes(black_box(&value)).unwrap();
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_bytes,
    bench_format_field,
    bench_varint,
    bench_struct,
    bench_array,
    bench_enum,
    bench_bytes_scaled,
);
criterion_main!(benches);
