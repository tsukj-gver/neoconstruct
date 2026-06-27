//! Phase 3.1 bit-level 性能 benchmark。
//!
//! 测量目标：
//! 1. 底层 `ParseStream::read_bits` / `BuildStream::write_bits`（裸 bit 操作成本）
//! 2. `BitsIntegerNode.parse` / `build`（含 GIL + PyLong 构造/提取的完整路径）
//!
//! 覆盖 length = 1, 4, 8, 16, 32, 64 bit × signed/unsigned。
//!
//! 输出：每个操作的 ns/call（中位数 + 最小值），供与 Python construct 2.10.70 对比。
//!
//! 运行：`cargo run --release`（在 experiments/bench_bits/ 下）

use construct_rust::context::Context;
use construct_rust::nodes::bits_integer::BitsIntegerNode;
use construct_rust::nodes::Construct;
use construct_rust::path::Path;
use construct_rust::stream::{BuildStream, ParseStream};

use pyo3::prelude::*;

/// 待测的 bit 宽度集合（覆盖 1/4/8/16/32/64，对应 Bit/Nibble/Octet/short/int/long）。
const LENGTHS: &[usize] = &[1, 4, 8, 16, 32, 64];

/// 每个测试运行的内层迭代次数（足够大以稳定 ns 级测量）。
const INNER: usize = 1_000_000;

/// 每个测试外层重复次数（取最小值，减少噪声）。
const OUTER: usize = 5;

/// 单次测量结果。
#[derive(Debug, Clone, Copy)]
struct Sample {
    /// 中位数 ns/op。
    median_ns: f64,
    /// 最小 ns/op。
    min_ns: f64,
}

/// 用 `Instant` 测 `INNER` 次操作的总耗时，重复 `OUTER` 轮取最小值，
/// 中位数作为稳健估计。返回 ns/op。
fn bench<F: FnMut()>(mut f: F) -> Sample {
    use std::time::Instant;
    let mut samples: Vec<f64> = Vec::with_capacity(OUTER);
    for _ in 0..OUTER {
        // 预热：先跑一小轮，避免分支预测器/缓存冷启动影响第一轮
        for _ in 0..(INNER.min(10_000)) {
            f();
        }
        let start = Instant::now();
        for _ in 0..INNER {
            f();
        }
        let elapsed = start.elapsed();
        let ns_per_op = elapsed.as_nanos() as f64 / INNER as f64;
        samples.push(ns_per_op);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let min_ns = samples[0];
    let median_ns = samples[samples.len() / 2];
    Sample { median_ns, min_ns }
}

// ---------------------------------------------------------------------------
// 底层 read_bits / write_bits
// ---------------------------------------------------------------------------

/// 准备长度为 `n_bytes` 的随机数据（固定模式，避免与全 0 / 全 1 产生特殊分支）。
fn make_data(n_bytes: usize) -> Vec<u8> {
    // 0xA5 = 0b1010_0101，0x3C = 0b0011_1100，交替打破对称性
    let mut v = Vec::with_capacity(n_bytes);
    for i in 0..n_bytes {
        v.push(if i % 2 == 0 { 0xA5 } else { 0x3C });
    }
    v
}

fn bench_read_bits(py: Python<'_>, length: usize) -> Sample {
    // 至少 8 字节，覆盖 64-bit 情况
    let n_bytes = (length + 7) / 8;
    let data = make_data(n_bytes.max(1));
    let mut path = Path::new();
    bench(|| {
        // 每次新建 stream 模拟实际 parse 场景中流的状态
        let mut stream = ParseStream::new(&data);
        let _ = stream.read_bits(length, &path);
        // 防止编译器优化掉 stream
        std::hint::black_box(&mut path);
        std::hint::black_box(stream.bit_pos());
        let _ = py; // 不实际使用 GIL，但保持签名一致
    })
}

fn bench_write_bits(_py: Python<'_>, length: usize) -> Sample {
    let value: u64 = if length < 64 {
        (1u64 << length) - 1
    } else {
        u64::MAX
    };
    bench(|| {
        let mut stream = BuildStream::new();
        stream.write_bits(value, length);
        std::hint::black_box(stream.bit_pos());
    })
}

/// 与 `bench_write_bits` 相同，但用 `with_capacity` 预分配缓冲。
///
/// 这反映生产中的真实场景：`BitwiseNode.build` 入口预分配 stream，
/// 内部多个 `BitsInteger` 字段复用，单次 `write_bits` 不应背负 Vec 分配开销。
fn bench_write_bits_prealloc(_py: Python<'_>, length: usize) -> Sample {
    let value: u64 = if length < 64 {
        (1u64 << length) - 1
    } else {
        u64::MAX
    };
    bench(|| {
        // with_capacity(16) 容纳 64-bit + 余量；避免 reallocate
        let mut stream = BuildStream::with_capacity(16);
        stream.write_bits(value, length);
        std::hint::black_box(stream.bit_pos());
    })
}

// ---------------------------------------------------------------------------
// BitsIntegerNode.parse / build（完整路径，含 PyLong 构造/提取）
// ---------------------------------------------------------------------------

fn bench_bits_integer_parse(py: Python<'_>, length: usize, signed: bool) -> Sample {
    let n_bytes = (length + 7) / 8;
    let data = make_data(n_bytes.max(1));
    let node = BitsIntegerNode::new(length, signed, false);
    let mut ctx = Context::placeholder(py);
    let mut path = Path::new();
    bench(|| {
        let mut stream = ParseStream::new(&data);
        let v = node.parse(py, &mut stream, &mut ctx, &mut path).expect("parse");
        std::hint::black_box(v);
    })
}

fn bench_bits_integer_build(py: Python<'_>, length: usize, signed: bool) -> Sample {
    // 选择范围内的代表性值（无符号取中点，有符号取 -1 触发二补码路径）
    let value_obj: Py<PyAny> = if signed {
        let max_positive = if length < 64 {
            (1i128 << (length - 1)) - 1
        } else {
            (1i128 << 63) - 1
        };
        // 取负数（覆盖二补码 build 路径）
        let v: i128 = -(max_positive / 2).max(1);
        v.into_py(py)
    } else {
        let max: u64 = if length < 64 {
            (1u64 << length) - 1
        } else {
            u64::MAX
        };
        (max / 2).into_py(py)
    };
    let obj_bound = value_obj.bind(py);
    let node = BitsIntegerNode::new(length, signed, false);
    let mut ctx = Context::placeholder(py);
    let mut path = Path::new();
    bench(|| {
        let mut stream = BuildStream::new();
        node.build(py, obj_bound, &mut stream, &mut ctx, &mut path)
            .expect("build");
        std::hint::black_box(stream.bit_pos());
    })
}

/// 与 `bench_bits_integer_build` 相同，但 stream 用 `with_capacity` 预分配。
///
/// 反映 `BitwiseNode.build` 入口预分配 stream 的真实场景。
fn bench_bits_integer_build_prealloc(py: Python<'_>, length: usize, signed: bool) -> Sample {
    let value_obj: Py<PyAny> = if signed {
        let max_positive = if length < 64 {
            (1i128 << (length - 1)) - 1
        } else {
            (1i128 << 63) - 1
        };
        let v: i128 = -(max_positive / 2).max(1);
        v.into_py(py)
    } else {
        let max: u64 = if length < 64 {
            (1u64 << length) - 1
        } else {
            u64::MAX
        };
        (max / 2).into_py(py)
    };
    let obj_bound = value_obj.bind(py);
    let node = BitsIntegerNode::new(length, signed, false);
    let mut ctx = Context::placeholder(py);
    let mut path = Path::new();
    bench(|| {
        let mut stream = BuildStream::with_capacity(16);
        node.build(py, obj_bound, &mut stream, &mut ctx, &mut path)
            .expect("build");
        std::hint::black_box(stream.bit_pos());
    })
}

// ---------------------------------------------------------------------------
// 输出
// ---------------------------------------------------------------------------

fn print_header(title: &str) {
    println!();
    println!("=== {} ===", title);
    println!(
        "{:>6}  {:>8}  {:>8}  {:>8}  {:>8}",
        "length", "median", "min", "median", "min"
    );
    println!(
        "{:>6}  {:>8}  {:>8}  {:>8}  {:>8}",
        "(bit)", "(ns/op)", "(ns/op)", "(clock)", "(clock)"
    );
}

fn print_row(label: &str, length: usize, s: Sample) {
    println!(
        "{:>6}  {:>8.2}  {:>8.2}  {:>8.0}  {:>8.0}",
        format!("{}{}", label, length),
        s.median_ns,
        s.min_ns,
        // clock cycles @ ~3.0 GHz（仅作辅助直觉参考，CPU 频率因机型而异）
        s.median_ns * 3.0,
        s.min_ns * 3.0
    );
}

fn main() {
    pyo3::prepare_freethreaded_python();

    Python::with_gil(|py| {
        println!("construct-rs Phase 3.1 bit-level benchmark");
        println!("------------------------------------------");
        println!("INNER = {} iterations/round, OUTER = {} rounds", INNER, OUTER);
        println!("(取 median 与 min，单位 ns/op)");

        // 1. 底层 read_bits
        print_header("底层 ParseStream::read_bits (unsigned, MSB-first)");
        for &len in LENGTHS {
            let s = bench_read_bits(py, len);
            print_row("read_bits_u", len, s);
        }

        // 2. 底层 write_bits
        print_header("底层 BuildStream::write_bits (unsigned, MSB-first, 每次新建 stream)");
        for &len in LENGTHS {
            let s = bench_write_bits(py, len);
            print_row("write_bits_u", len, s);
        }

        // 2b. 底层 write_bits（预分配，分离 Vec 分配开销）
        print_header("底层 BuildStream::write_bits (unsigned, with_capacity 预分配)");
        for &len in LENGTHS {
            let s = bench_write_bits_prealloc(py, len);
            print_row("write_bits_pre_u", len, s);
        }

        // 3. BitsIntegerNode.parse — unsigned
        print_header("BitsIntegerNode.parse (unsigned, 完整路径含 PyLong 构造)");
        for &len in LENGTHS {
            let s = bench_bits_integer_parse(py, len, false);
            print_row("parse_u", len, s);
        }

        // 4. BitsIntegerNode.parse — signed
        print_header("BitsIntegerNode.parse (signed, 完整路径含二补码 + PyLong)");
        for &len in LENGTHS {
            let s = bench_bits_integer_parse(py, len, true);
            print_row("parse_s", len, s);
        }

        // 5. BitsIntegerNode.build — unsigned
        print_header("BitsIntegerNode.build (unsigned, 完整路径含 i128 提取 + 范围校验, 每次新建 stream)");
        for &len in LENGTHS {
            let s = bench_bits_integer_build(py, len, false);
            print_row("build_u", len, s);
        }

        // 5b. BitsIntegerNode.build — unsigned（预分配）
        print_header("BitsIntegerNode.build (unsigned, with_capacity 预分配)");
        for &len in LENGTHS {
            let s = bench_bits_integer_build_prealloc(py, len, false);
            print_row("build_pre_u", len, s);
        }

        // 6. BitsIntegerNode.build — signed
        print_header("BitsIntegerNode.build (signed, 完整路径含二补码, 每次新建 stream)");
        for &len in LENGTHS {
            let s = bench_bits_integer_build(py, len, true);
            print_row("build_s", len, s);
        }

        // 6b. BitsIntegerNode.build — signed（预分配）
        print_header("BitsIntegerNode.build (signed, with_capacity 预分配)");
        for &len in LENGTHS {
            let s = bench_bits_integer_build_prealloc(py, len, true);
            print_row("build_pre_s", len, s);
        }
    });
}
