//! Context 操作 micro-benchmark（Phase 2.5 性能退化排查）。
//!
//! 测量 Context 的关键操作在 Rust 级别的纳秒成本，用于隔离 Vec 化优化的
//! 开销来源。
//!
//! ## 运行方法
//!
//! ```sh
//! cargo test --release --test bench_context_ops -- --nocapture --ignored
//! ```
//!
//! ## 测量项
//!
//! | 操作 | 测量内容 |
//! |------|---------|
//! | `init_expr_values(n)` | 内联缓冲区初始化成本 |
//! | `set_field_at` | dict 写入 + buf 写入成本 |
//! | `get_int_at`（修复后） | Vec 索引 + PyLong_AsLongLong |
//! | `get_int_at_eager_ok_or`（模拟旧代码） | ok_or + .to_string() 的堆分配开销 |
//! | `take_fields` | Option::take 成本 |

#![allow(clippy::needless_range_loop)]

use construct_rust::context::Context;
use construct_rust::error::ConstructError;
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::PyString;
use std::hint::black_box;
use std::time::Instant;

/// 每次测量的迭代次数。
const ITERS: usize = 2_000_000;

/// 预热迭代次数。
const WARMUP: usize = 50_000;

/// 初始化 Python 解释器（幂等）。
fn ensure_python() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(pyo3::prepare_freethreaded_python);
}

/// 测量并打印单次操作的平均纳秒成本。
fn bench_ns<F: FnMut()>(name: &str, mut f: F) -> f64 {
    // 预热
    for _ in 0..WARMUP {
        f();
    }
    // 测量
    let start = Instant::now();
    for _ in 0..ITERS {
        f();
    }
    let elapsed = start.elapsed();
    let ns = elapsed.as_nanos() as f64 / ITERS as f64;
    println!("  {:<42} {:>8.1} ns/op", name, ns);
    ns
}

// ======================================================================
// 模拟旧代码的 eager ok_or 路径（用于对比）
// ======================================================================

/// 模拟旧 `get_int_at` 中的 `ok_or` + `.to_string()` 行为：
/// 即使在成功路径上也会构造错误值（含 String 堆分配）。
///
/// 返回的 dummy error 被丢弃（black_box 防止优化器消除）。
#[inline(never)]
fn eager_error_construction() {
    let _ = black_box(ConstructError::ExprContext {
        message: "get_int_at: expr_values not initialized (placeholder context)".to_string(),
        path: String::new(),
    });
}

/// 模拟修复后的惰性路径：成功路径上不构造任何错误。
#[inline(never)]
fn lazy_error_check(len: usize) -> bool {
    len != 0
}

// ======================================================================
// Benchmark tests
// ======================================================================

#[test]
#[ignore = "benchmark — run with --ignored"]
fn bench_all_context_ops() {
    ensure_python();
    Python::with_gil(|py| {
        println!();
        println!("============================================================");
        println!("Context 操作 micro-benchmark ({} iters each)", ITERS);
        println!("============================================================");
        println!();

        // ------------------------------------------------------------------
        // 1. init_expr_values(n)
        // ------------------------------------------------------------------
        println!("[1] init_expr_values — 内联缓冲区初始化");
        {
            let mut ctx = Context::new_root(py).expect("root");
            bench_ns("init_expr_values(2)", || {
                ctx.init_expr_values(2);
            });
            bench_ns("init_expr_values(4)", || {
                ctx.init_expr_values(4);
            });
            bench_ns("init_expr_values(6)", || {
                ctx.init_expr_values(6);
            });
        }
        println!();

        // ------------------------------------------------------------------
        // 2. set_field_at — dict 写入 + buf 写入
        // ------------------------------------------------------------------
        println!("[2] set_field_at — PyDict.SetItem + buf[idx] 写入");
        {
            let mut ctx = Context::new_root(py).expect("root");
            ctx.init_expr_values(2);
            let key = PyString::new_bound(py, "count").unbind();
            let value = 42i64.into_py(py);
            let value_ref = value.bind(py);
            bench_ns("set_field_at(0, 'count', 42) [len=2]", || {
                ctx.set_field_at(0, &key, value_ref, py).expect("set");
            });
        }
        println!();

        // ------------------------------------------------------------------
        // 3. get_int_at — 修复后的路径
        // ------------------------------------------------------------------
        println!("[3] get_int_at — 成功路径（修复后）");
        {
            let mut ctx = Context::new_root(py).expect("root");
            ctx.init_expr_values(2);
            let key = PyString::new_bound(py, "count").unbind();
            let value = 42i64.into_py(py);
            ctx.set_field_at(0, &key, value.bind(py), py).expect("set");
            bench_ns("get_int_at(0) [buf[idx] + AsLongLong]", || {
                let v = ctx.get_int_at(0, py).expect("get");
                black_box(v);
            });
        }
        println!();

        // ------------------------------------------------------------------
        // 4. 对比：eager ok_or .to_string() vs 惰性检查
        // ------------------------------------------------------------------
        println!("[4] 错误构造开销对比（退化根因）");
        {
            bench_ns("eager: .to_string() + String::new() [旧路径]", || {
                eager_error_construction();
            });
            bench_ns("lazy: len != 0 check [新路径]", || {
                let r = lazy_error_check(2);
                black_box(r);
            });
        }
        println!();

        // ------------------------------------------------------------------
        // 5. take_fields
        // ------------------------------------------------------------------
        println!("[5] take_fields — Option::take");
        {
            let mut ctx = Context::new_root(py).expect("root");
            bench_ns("take_fields + re-init", || {
                // 每次需要重建 ctx，因为 take 后 fields=None
                // 这里测量 take 本身的成本（不含 PyDict 创建）
                let _ = black_box(ctx.take_fields());
                // 重建以保持循环不变量
                ctx = Context::new_root(py).expect("root");
                ctx.init_expr_values(0);
            });
        }
        println!();

        // ------------------------------------------------------------------
        // 6. 完整 parse 模拟（init + N×set + N×get_int）
        // ------------------------------------------------------------------
        println!("[6] 完整表达式 Struct 模拟（2 字段, 1 GetInt — 类似 E1）");
        {
            let mut ctx = Context::new_root(py).expect("root");
            let keys: Vec<_> = ["count", "data"]
                .iter()
                .map(|n| PyString::new_bound(py, n).unbind())
                .collect();
            let values: Vec<_> = [42i64, 0].iter().map(|v| (*v).into_py(py)).collect();

            bench_ns("sim: init(2) + 2×set + 1×get_int_at", || {
                ctx.init_expr_values(2);
                ctx.set_field_at(0, &keys[0], values[0].bind(py), py)
                    .expect("set0");
                ctx.set_field_at(1, &keys[1], values[1].bind(py), py)
                    .expect("set1");
                let v = ctx.get_int_at(0, py).expect("get");
                black_box(v);
            });
        }
        println!();

        // ------------------------------------------------------------------
        // 7. 对比：PyDict_GetItem + extract（模拟旧 get_int_by_name 路径）
        // ------------------------------------------------------------------
        println!("[7] 对比：PyDict hash 查找路径（旧 get_int_by_name 等效）");
        {
            let ctx = Context::new_root(py).expect("root");
            let key = PyString::new_bound(py, "count").unbind();
            let value = 42i64.into_py(py);
            ctx.set_field("count", value.bind(py)).expect("set");

            bench_ns("PyDict_GetItem + PyLong_AsLongLong [旧路径]", || {
                let fields = ctx.fields().expect("fields");
                let ptr = unsafe { ffi::PyDict_GetItem(fields.as_ptr(), key.bind(py).as_ptr()) };
                let v = unsafe { ffi::PyLong_AsLongLong(ptr) };
                black_box(v);
            });
        }
        println!();
        println!("============================================================");
        println!("Micro-benchmark 完成");
        println!("============================================================");
    });
}
