//! Phase 3 parse/build 不对称根因分析实验。
//!
//! 目标：量化以下元操作的成本，验证"Phase 3 不对称 = StructNode 固有不对称
//! + BitsInteger i128 增量"的分析模型。
//!
//! 测量组：
//! 1. PyLong 创建成本：i64 / i128 / u64 IntoPy 对比（小值 + 大值）
//! 2. PyLong 读取成本：i64 / i128 Extract 对比（小值 + 大值）
//! 3. create_class (tp_new) 成本：Phase 1/2/3 共有的 parse 独有开销
//! 4. PyBytes::new_bound 成本：build 独有开销
//! 5. getattr("__dict__") + downcast 成本：parse 独有开销
//! 6. dict.set_item 成本：parse 独有开销（每字段）
//! 7. getattr(field_name) 成本：build 独有开销（每字段）
//!
//! 运行：`cargo run --release`（在 experiments/bench_asymmetry/ 下）

use construct_rust::context::Context;
use construct_rust::instance::create_class;
use construct_rust::nodes::bits_integer::BitsIntegerNode;
use construct_rust::nodes::format_field::{FormatFieldNode, PythonFormat};
use construct_rust::nodes::struct_node::{FieldName, StructField, StructNode, FieldMode};
use construct_rust::nodes::{Construct, Node};
use construct_rust::path::Path;
use construct_rust::stream::{BuildStream, ParseStream};

use pyo3::conversion::IntoPy;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyType};

/// 每个测试运行的内层迭代次数。
const INNER: usize = 1_000_000;

/// 每个测试外层重复次数（取最小值 + 中位数）。
const OUTER: usize = 7;

/// 单次测量结果。
#[derive(Debug, Clone, Copy)]
struct Sample {
    median_ns: f64,
    min_ns: f64,
}

/// 用 `Instant` 测 `INNER` 次操作的总耗时，重复 `OUTER` 轮，返回 median + min。
fn bench<F: FnMut()>(mut f: F) -> Sample {
    use std::time::Instant;
    let mut samples: Vec<f64> = Vec::with_capacity(OUTER);
    for _ in 0..OUTER {
        // 预热：先跑一小轮避免分支预测器冷启动。
        for _ in 0..(INNER.min(20_000)) {
            f();
        }
        let start = Instant::now();
        for _ in 0..INNER {
            f();
        }
        let elapsed = start.elapsed();
        samples.push(elapsed.as_nanos() as f64 / INNER as f64);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let min_ns = samples[0];
    let median_ns = samples[samples.len() / 2];
    Sample { median_ns, min_ns }
}

fn print_header(title: &str) {
    println!();
    println!("=== {} ===", title);
    println!(
        "{:<40}  {:>10}  {:>10}",
        "operation", "median(ns)", "min(ns)"
    );
    println!("{}", "-".repeat(64));
}

fn print_row(label: &str, s: Sample) {
    println!(
        "{:<40}  {:>10.2}  {:>10.2}",
        label, s.median_ns, s.min_ns
    );
}

// ---------------------------------------------------------------------------
// 组 1：PyLong 创建成本（IntoPy）
// ---------------------------------------------------------------------------

fn bench_pylong_create(py: Python<'_>) {
    print_header("组1: PyLong 创建成本 (IntoPy, 值在 i64 范围内)");

    // 小正值（0..256 范围，CPython 小整数缓存可能命中）
    let s_i64_small: Sample = bench(|| {
        let v: Py<PyAny> = 42i64.into_py(py);
        std::hint::black_box(v);
    });
    print_row("i64::into_py(42) [小正]", s_i64_small);

    let s_i128_small: Sample = bench(|| {
        let v: Py<PyAny> = 42i128.into_py(py);
        std::hint::black_box(v);
    });
    print_row("i128::into_py(42) [小正]", s_i128_small);

    let s_u64_small: Sample = bench(|| {
        let v: Py<PyAny> = 42u64.into_py(py);
        std::hint::black_box(v);
    });
    print_row("u64::into_py(42) [小正]", s_u64_small);

    // 大正值（i64 范围内但不命中缓存）
    let s_i64_big: Sample = bench(|| {
        let v: Py<PyAny> = 0x1234_5678i64.into_py(py);
        std::hint::black_box(v);
    });
    print_row("i64::into_py(0x12345678) [大正]", s_i64_big);

    let s_i128_big: Sample = bench(|| {
        let v: Py<PyAny> = 0x1234_5678i128.into_py(py);
        std::hint::black_box(v);
    });
    print_row("i128::into_py(0x12345678) [大正]", s_i128_big);

    // 负数（i64 范围内）
    let s_i64_neg: Sample = bench(|| {
        let v: Py<PyAny> = (-1i64).into_py(py);
        std::hint::black_box(v);
    });
    print_row("i64::into_py(-1) [负]", s_i64_neg);

    let s_i128_neg: Sample = bench(|| {
        let v: Py<PyAny> = (-1i128).into_py(py);
        std::hint::black_box(v);
    });
    print_row("i128::into_py(-1) [负]", s_i128_neg);

    let s_i128_neg_big: Sample = bench(|| {
        let v: Py<PyAny> = (-0x1234_5678i128).into_py(py);
        std::hint::black_box(v);
    });
    print_row("i128::into_py(-0x12345678) [大负]", s_i128_neg_big);

    // u64 超出 i64 范围（u64::MAX）
    let s_u64_max: Sample = bench(|| {
        let v: Py<PyAny> = u64::MAX.into_py(py);
        std::hint::black_box(v);
    });
    print_row("u64::into_py(u64::MAX)", s_u64_max);
}

// ---------------------------------------------------------------------------
// 组 2：PyLong 读取成本（Extract）
// ---------------------------------------------------------------------------

fn bench_pylong_extract(py: Python<'_>) {
    print_header("组2: PyLong 读取成本 (Extract)");

    let v_small: Py<PyAny> = 42i64.into_py(py);
    let vb_small = v_small.bind(py);
    let s_i64_small: Sample = bench(|| {
        let n: i64 = vb_small.extract().unwrap();
        std::hint::black_box(n);
    });
    print_row("extract::<i64>(42)", s_i64_small);

    let s_i128_small: Sample = bench(|| {
        let n: i128 = vb_small.extract().unwrap();
        std::hint::black_box(n);
    });
    print_row("extract::<i128>(42)", s_i128_small);

    let v_big: Py<PyAny> = 0x1234_5678i64.into_py(py);
    let vb_big = v_big.bind(py);
    let s_i64_big: Sample = bench(|| {
        let n: i64 = vb_big.extract().unwrap();
        std::hint::black_box(n);
    });
    print_row("extract::<i64>(0x12345678)", s_i64_big);

    let s_i128_big: Sample = bench(|| {
        let n: i128 = vb_big.extract().unwrap();
        std::hint::black_box(n);
    });
    print_row("extract::<i128>(0x12345678)", s_i128_big);

    let v_neg: Py<PyAny> = (-1i64).into_py(py);
    let vb_neg = v_neg.bind(py);
    let s_i128_neg: Sample = bench(|| {
        let n: i128 = vb_neg.extract().unwrap();
        std::hint::black_box(n);
    });
    print_row("extract::<i128>(-1)", s_i128_neg);

    // is_instance_of::<PyLong>() 类型检查
    let v_any: Py<PyAny> = 42i64.into_py(py);
    let vb_any = v_any.bind(py);
    let s_isinst: Sample = bench(|| {
        let b: bool = vb_any.is_instance_of::<pyo3::types::PyLong>();
        std::hint::black_box(b);
    });
    print_row("is_instance_of::<PyLong>", s_isinst);
}

fn main() {
    pyo3::prepare_freethreaded_python();

    Python::with_gil(|py| {
        println!("construct-rs Phase 3 parse/build 不对称根因分析");
        println!("============================================================");
        println!(
            "INNER = {} iter/round, OUTER = {} rounds, 取 median + min, 单位 ns/op",
            INNER, OUTER
        );

        bench_pylong_create(py);
        bench_pylong_extract(py);
        bench_struct_overhead(py);
        bench_pyybytes_getattr(py);
        bench_bitsinteger_compare(py);
        bench_summary();
    });
}

// 组 3-6 的实现见 main_part2.rs（通过 include! 合并）
include!("main_part2.rs");
