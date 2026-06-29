//! Phase 3.3 Bytewise / BitsSwapped / ByteSwapped / Padding + 端到端 BitStruct 性能 benchmark。
//!
//! 测量目标：
//! 1. BytewiseNode（对齐快路径 / 未对齐慢路径）的 parse/build
//! 2. TransformNode（ByteSwap / BitSwap）的 parse/build
//! 3. BitPaddingNode / PaddingNode 的 parse/build
//! 4. 端到端 BitStruct（Bitwise(Struct{...})）的 parse/build
//!
//! 所有场景对照 Python construct 2.10.70 的等价构造器（详见 bench_phase33_python.py）。
//!
//! 测量方法：
//! - 通过 path 依赖 construct-rs rlib，**调用真实公共 API**
//!   （Construct::parse/build、ParseStream::new、BuildStream::new、Context、Path）
//! - parse 包含 GIL 持有 + PyLong/PyDict/实例构造的真实开销
//! - build 包含 GIL 持有 + getattr + i128 提取 + 范围校验 + 字节写入的真实开销
//!
//! 运行：`cargo run --release`（在 experiments/bench_phase33/ 下）
//! 需要环境变量 `PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1`（pyo3 0.22 不原生支持 Python 3.14）。

use construct_rust::context::Context;
use construct_rust::nodes::bit_padding::BitPaddingNode;
use construct_rust::nodes::bits_integer::BitsIntegerNode;
use construct_rust::nodes::bitwise::BitwiseNode;
use construct_rust::nodes::bytes::BytesNode;
use construct_rust::nodes::bytewise::BytewiseNode;
use construct_rust::nodes::format_field::{FormatFieldNode, PythonFormat};
use construct_rust::nodes::padding::PaddingNode;
use construct_rust::nodes::struct_node::{FieldName, FieldMode, StructField, StructNode};
use construct_rust::nodes::transform::{ByteTransform, TransformNode};
use construct_rust::nodes::{Construct, Node};
use construct_rust::path::Path;
use construct_rust::stream::{BuildStream, ParseStream};

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyModule, PyType};

/// 每个测试内层迭代次数（足够大以稳定 ns 级测量）。
const INNER: usize = 200_000;
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

/// 用 `Instant` 测 `INNER` 次操作的总耗时，重复 `OUTER` 轮，取 min + median。
fn bench<F: FnMut()>(mut f: F) -> Sample {
    use std::time::Instant;
    let mut samples: Vec<f64> = Vec::with_capacity(OUTER);
    for _ in 0..OUTER {
        // 预热：先跑一小轮，避免分支预测器/缓存冷启动影响第一轮
        for _ in 0..(INNER.min(5_000)) {
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
// Python 对象构造辅助
// ---------------------------------------------------------------------------

/// 创建一个 `types.SimpleNamespace` 实例，并设置给定的 (属性名, 值) 对。
///
/// StructNode.build 通过 `obj.getattr(name)` 取每个字段值。SimpleNamespace 是 CPython
/// 原生支持的标准类型，开销接近 dict 但语义更接近用户 @dataclass 实例。
fn make_namespace<'py>(
    py: Python<'py>,
    attrs: &[(&str, Py<PyAny>)],
) -> PyResult<Bound<'py, PyAny>> {
    let types_mod = PyModule::import_bound(py, "types")?;
    let ns_cls = types_mod.getattr("SimpleNamespace")?;
    let instance = ns_cls.call0()?;
    for (name, value) in attrs {
        instance.setattr(*name, value.bind(py))?;
    }
    Ok(instance)
}

// ---------------------------------------------------------------------------
// 节点树构造辅助
// ---------------------------------------------------------------------------

fn u8_node() -> Node {
    Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big))
}

fn u16_node() -> Node {
    Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big))
}

fn u32_node() -> Node {
    Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt32Big))
}

fn bytes_node(n: usize) -> Node {
    Node::Bytes(BytesNode::new_const(n))
}

fn nibble_node() -> Node {
    Node::BitsInteger(BitsIntegerNode::new(4, false, false))
}

fn bits_node(n: usize) -> Node {
    Node::BitsInteger(BitsIntegerNode::new(n, false, false))
}

/// 把 Rust `&[u8]` 转为 Python `bytes` 对象（`PyBytes`）。
fn to_pybytes(py: Python<'_>, data: &'static [u8]) -> Py<PyAny> {
    PyBytes::new_bound(py, data).into_any().unbind()
}

/// 构造一个 `StructNode`：所有字段均为 RW，has_post_init=false，has_expressions=false。
///
/// 这是 `construct-rs` 内 `StructNode::new_for_test` 的外部等价物（后者仅 `#[cfg(test)]`）。
/// 创建一个最小 Python `object` 子类作为 `cls`，并通过 `FieldName::new` 把每个字段名
/// intern 一次。
fn make_struct_node(py: Python<'_>, fields: Vec<(&str, Node)>) -> StructNode {
    let struct_fields = fields
        .into_iter()
        .map(|(name, node)| StructField {
            name: FieldName::new(py, name),
            node,
            mode: FieldMode::Rw,
        })
        .collect();
    // 最小 mock 类：`type('Mock', (object,), {})`，与 construct-rs 单元测试一致
    let cls = py
        .eval_bound("type('MockStruct', (), {})", None, None)
        .expect("create mock class")
        .extract::<Py<PyType>>()
        .expect("extract Py<PyType>");
    StructNode::new(py, struct_fields, cls, false, false)
}

// ---------------------------------------------------------------------------
// Scenario 抽象
// ---------------------------------------------------------------------------

/// 单个 benchmark 场景。
struct Scenario {
    /// 场景名（与 Python 端对齐）。
    name: &'static str,
    /// 待测的根 Node。
    node: Node,
    /// parse 时的输入字节。
    parse_data: Vec<u8>,
    /// build 时的输入对象（Some 表示有 StructNode 需要构造 namespace；None 表示标量）。
    /// 标量场景下，build 直接对 node.build 传 PyLong / PyBytes。
    build_obj: BuildObj,
}

/// build 场景的对象种类。
enum BuildObj {
    /// 直接传一个 Python 标量（PyLong / PyBytes）。
    Scalar(Py<PyAny>),
    /// 通过 SimpleNamespace 携带命名字段（StructNode 场景）。
    Namespace(Vec<(&'static str, Py<PyAny>)>),
}

impl Scenario {
    /// 运行此场景的 parse + build benchmark，返回 (parse_sample, build_sample)。
    fn run(&self, py: Python<'_>) -> (Sample, Sample) {
        let mut ctx = Context::placeholder(py);
        let parse_sample = {
            let node_ref: &Node = &self.node;
            let data: Vec<u8> = self.parse_data.clone();
            bench(move || {
                let mut stream = ParseStream::new(&data);
                let mut path = Path::new();
                let v = node_ref
                    .parse(py, &mut stream, &mut ctx, &mut path)
                    .expect("parse");
                std::hint::black_box(v);
            })
        };

        // 先把 build obj 准备好（避免在 bench 内重复构造 Python 对象）。
        let build_sample = match &self.build_obj {
            BuildObj::Scalar(scalar) => {
                let obj_bound = scalar.bind(py);
                let node_ref: &Node = &self.node;
                bench(move || {
                    let mut stream = BuildStream::new();
                    let mut ctx = Context::placeholder(py);
                    let mut path = Path::new();
                    node_ref
                        .build(py, obj_bound, &mut stream, &mut ctx, &mut path)
                        .expect("build");
                    std::hint::black_box(stream.bit_pos());
                })
            }
            BuildObj::Namespace(attrs) => {
                let obj_bound = make_namespace(py, attrs).expect("namespace");
                let node_ref: &Node = &self.node;
                bench(move || {
                    let mut stream = BuildStream::new();
                    let mut ctx = Context::placeholder(py);
                    let mut path = Path::new();
                    node_ref
                        .build(py, &obj_bound, &mut stream, &mut ctx, &mut path)
                        .expect("build");
                    std::hint::black_box(stream.bit_pos());
                })
            }
        };

        (parse_sample, build_sample)
    }
}

// ---------------------------------------------------------------------------
// 输出
// ---------------------------------------------------------------------------

fn print_results(header: &str, rows: &[(&str, Sample, Sample)]) {
    println!();
    println!("=== {} ===", header);
    println!(
        "{:<48}  {:>9}  {:>9}  {:>9}  {:>9}",
        "scenario", "parse_med", "parse_min", "build_med", "build_min"
    );
    println!(
        "{:<48}  {:>9}  {:>9}  {:>9}  {:>9}",
        "", "(ns/op)", "(ns/op)", "(ns/op)", "(ns/op)"
    );
    for (name, p, b) in rows {
        println!(
            "{:<48}  {:>9.1}  {:>9.1}  {:>9.1}  {:>9.1}",
            name, p.median_ns, p.min_ns, b.median_ns, b.min_ns
        );
    }
}

fn main() {
    pyo3::prepare_freethreaded_python();

    Python::with_gil(|py| {
        println!("construct-rs Phase 3.3 benchmark");
        println!("--------------------------------");
        println!(
            "INNER = {} iters/round, OUTER = {} rounds (取 median + min)",
            INNER, OUTER
        );

        // ====================================================================
        // 1. Bytewise 对齐快路径：Bitwise(Bytewise(Bytes(4)))
        //    Bitwise 入口 bit_pos==0，Bytewise 直接委托 Bytes(4) 读 4 字节
        // ====================================================================
        let bytewise_aligned = Scenario {
            name: "Bitwise(Bytewise(Bytes(4)))",
            node: Node::Bitwise(BitwiseNode::new(Node::Bytewise(BytewiseNode::new(
                bytes_node(4),
            )))),
            parse_data: vec![0xA5, 0x3C, 0x96, 0xC3],
            build_obj: BuildObj::Scalar(to_pybytes(py, b"\xA5\x3C\x96\xC3")),
        };

        // ====================================================================
        // 2. Bytewise 未对齐慢路径：Bitwise(Struct{Nibble, Bytewise(Bytes(2)), Nibble})
        //    Nibble 消耗 4 bit，Bytewise 在 bit_pos=4 走慢路径提取 2 字节
        // ====================================================================
        let bytewise_unaligned = Scenario {
            name: "BitStruct(Nibble, Bytewise(Int16ub), Nibble)",
            node: Node::Bitwise(BitwiseNode::new(Node::Struct(make_struct_node(
                py,
                vec![
                    ("a", nibble_node()),
                    ("b", Node::Bytewise(BytewiseNode::new(u16_node()))),
                    ("c", nibble_node()),
                ],
            )))),
            parse_data: vec![0xA1, 0x23, 0x4B],
            build_obj: BuildObj::Namespace(vec![
                ("a", 0xAu64.into_py(py)),
                ("b", 0x1234u64.into_py(py)),
                ("c", 0xBu64.into_py(py)),
            ]),
        };

        // ====================================================================
        // 3. BitsSwapped: Transform(Bytes(4), BitSwap)
        // ====================================================================
        let bits_swapped = Scenario {
            name: "BitsSwapped(Bytes(4))",
            node: Node::Transform(TransformNode::new(bytes_node(4), ByteTransform::BitSwap)),
            parse_data: vec![0xF0, 0x0F, 0xAA, 0x55],
            build_obj: BuildObj::Scalar(to_pybytes(py, b"\x0F\xF0\x55\xAA")),
        };

        // ====================================================================
        // 4. ByteSwapped: Transform(Int32ub, ByteSwap)
        // ====================================================================
        let byte_swapped = Scenario {
            name: "ByteSwapped(Int32ub)",
            node: Node::Transform(TransformNode::new(u32_node(), ByteTransform::ByteSwap)),
            parse_data: vec![0x78, 0x56, 0x34, 0x12],
            build_obj: BuildObj::Scalar(0x12345678u32.into_py(py)),
        };

        // ====================================================================
        // 5. BitPadding 单独：Bitwise(Struct{Nibble, BitPadding(4)})
        // ====================================================================
        let bit_padding = Scenario {
            name: "BitStruct(Nibble, BitPadding(4))",
            node: Node::Bitwise(BitwiseNode::new(Node::Struct(make_struct_node(
                py,
                vec![
                    ("a", nibble_node()),
                    (
                        "pad",
                        Node::BitPadding(BitPaddingNode::new(4, 0x00).expect("bit padding")),
                    ),
                ],
            )))),
            parse_data: vec![0xA5],
            build_obj: BuildObj::Namespace(vec![("a", 0xAu64.into_py(py)), ("pad", py.None())]),
        };

        // ====================================================================
        // 6. Padding 字节级：Struct{Bytes(1), Padding(4), Bytes(2)}
        // ====================================================================
        let byte_padding = Scenario {
            name: "Struct(Bytes(1), Padding(4), Bytes(2))",
            node: Node::Struct(make_struct_node(
                py,
                vec![
                    ("tag", bytes_node(1)),
                    ("pad", Node::Padding(PaddingNode::new_const(4, 0x00))),
                    ("data", bytes_node(2)),
                ],
            )),
            parse_data: vec![0xAA, 0x00, 0x00, 0x00, 0x00, 0xBB, 0xCC],
            build_obj: BuildObj::Namespace(vec![
                ("tag", to_pybytes(py, b"\xAA")),
                ("pad", py.None()),
                ("data", to_pybytes(py, b"\xBB\xCC")),
            ]),
        };

        // ====================================================================
        // 7. 端到端 BitStruct(3 字段 Nibble/BitsInteger(10)/BitPadding(2))
        //    ARCH 在 Python 3.13 实测：6288 ns/op parse
        // ====================================================================
        let bitstruct_3field = Scenario {
            name: "BitStruct(Nibble, BitsInteger(10), BitPadding(2))",
            node: Node::Bitwise(BitwiseNode::new(Node::Struct(make_struct_node(
                py,
                vec![
                    ("a", nibble_node()),
                    ("b", bits_node(10)),
                    (
                        "c",
                        Node::BitPadding(BitPaddingNode::new(2, 0x00).expect("bit padding")),
                    ),
                ],
            )))),
            parse_data: vec![0xBE, 0xEF],
            build_obj: BuildObj::Namespace(vec![
                ("a", 0xBu64.into_py(py)),
                ("b", 0x1F7u64.into_py(py)),
                ("c", py.None()),
            ]),
        };

        // ====================================================================
        // 8. 对照：单一 Bitwise(BitsInteger(8))（最小包装开销）
        // ====================================================================
        let bitwise_int8 = Scenario {
            name: "Bitwise(BitsInteger(8))",
            node: Node::Bitwise(BitwiseNode::new(bits_node(8))),
            parse_data: vec![0xA5],
            build_obj: BuildObj::Scalar(0xA5u64.into_py(py)),
        };

        // ====================================================================
        // 9. 对照：单一 Bitwise(BitsInteger(16))
        // ====================================================================
        let bitwise_int16 = Scenario {
            name: "Bitwise(BitsInteger(16))",
            node: Node::Bitwise(BitwiseNode::new(bits_node(16))),
            parse_data: vec![0xA5, 0x3C],
            build_obj: BuildObj::Scalar(0xA53Cu64.into_py(py)),
        };

        // ====================================================================
        // 10. Bytewise(Int8ub) 字节内单字节（验证字节节点经 Bytewise 的零开销）
        // ====================================================================
        let bytewise_single = Scenario {
            name: "Bitwise(Bytewise(Int8ub))",
            node: Node::Bitwise(BitwiseNode::new(Node::Bytewise(BytewiseNode::new(u8_node())))),
            parse_data: vec![0x42],
            build_obj: BuildObj::Scalar(0x42u8.into_py(py)),
        };

        // ====================================================================
        // 跑全部场景
        // ====================================================================
        let scenarios: Vec<Scenario> = vec![
            bitwise_int8,
            bitwise_int16,
            bytewise_single,
            bytewise_aligned,
            bytewise_unaligned,
            bits_swapped,
            byte_swapped,
            bit_padding,
            byte_padding,
            bitstruct_3field,
        ];

        let mut rows: Vec<(&str, Sample, Sample)> = Vec::with_capacity(scenarios.len());
        for s in &scenarios {
            let (p, b) = s.run(py);
            rows.push((s.name, p, b));
        }

        // 按类别打印
        print_results("全部场景", &rows);

        // 单独再列一个 "仅 Bytewise/Transform/Padding" 视图（相同数据，仅 subset）
        let subset: Vec<(&str, Sample, Sample)> = rows
            .iter()
            .filter(|(n, _, _)| {
                n.contains("Bytewise")
                    || n.contains("BitsSwapped")
                    || n.contains("ByteSwapped")
                    || n.contains("BitPadding")
                    || n.contains("Padding")
            })
            .map(|(n, p, b)| (*n, *p, *b))
            .collect();
        print_results("Phase 3.3 新增节点（Bytewise / Transform / Padding）", &subset);

        let bitstruct_subset: Vec<(&str, Sample, Sample)> = rows
            .iter()
            .filter(|(n, _, _)| n.contains("BitStruct") || n.contains("Bitwise"))
            .map(|(n, p, b)| (*n, *p, *b))
            .collect();
        print_results("端到端 BitStruct / Bitwise 包装", &bitstruct_subset);
    });
}
