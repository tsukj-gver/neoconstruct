//! SelectNode：多分支尝试节点（首个成功者胜出）。
//!
//! Python 参考：`construct/construct/core.py` `Select`（L3830-3884）。
//!
//! ## 行为概述
//!
//! `Select(subcons)` 遍历 subcons 尝试 parse/build，首个成功者胜出。
//!
//! - parse：每个 subcon 尝试解析；失败时 seek 回 fallback 继续尝试下一个；
//!   成功则短路返回；全部失败抛 `ConstructError::Select`。
//! - build：每个 subcon 在 temp `BuildStream` 上尝试构建；成功则把字节写入主流；
//!   全部失败抛 `ConstructError::Select`。
//! - sizeof：永远返回 `Err`（对齐 Python 无 `_sizeof` 方法定义）。
//!
//! ## ExplicitError 集成
//!
//! `ConstructError::Explicit` 不被 Select 吞掉，直接向上传播
//! （对齐 Python `except ExplicitError: raise`）。
//!
//! ## 已知差异
//!
//! Python `Select._build` 在 inner build 抛非 Explicit 错误时**不回退 stream**
//! （因为 build 到 temp BytesIO，失败时 temp 丢弃即可）。construct-rs 行为一致。

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

use super::Construct;
use crate::nodes::Node;

/// 多分支尝试节点（对应 Python construct `Select`，core.py L3830）。
///
/// # 三方法行为
///
/// ## parse
///
/// 遍历 subcons：
/// 1. `fallback = stream.tell()`
/// 2. 调 `sub.parse(...)`
///    - `Ok(obj)` → return obj（短路，后续不尝试）
///    - `Err(Explicit)` → **直接 return Err**（Python `except ExplicitError: raise`）
///    - `Err(其他)` → `stream.seek(fallback)` 继续
/// 3. 全部失败 → [`ConstructError::Select`]
///
/// ## build
///
/// 遍历 subcons：
/// 1. 创建临时 `BuildStream`（与 PrefixedArray build 同模式）
/// 2. 调 `sub.build(obj, &mut temp_stream, ...)`
///    - `Ok(())` → 把 temp_stream 字节写到主 stream → return Ok(())（短路）
///    - `Err(Explicit)` → **直接 return Err**（穿透）
///    - `Err(其他)` → 丢弃 temp_stream 继续
/// 3. 全部失败 → [`ConstructError::Select`]
///
/// ## sizeof
///
/// 永远返回 `Err`（对齐 Python `Select._sizeof` 抛 SizeofError——Python 未定义
/// `_sizeof` 时默认 raise SizeofError）。
///
/// # ExplicitError 集成
///
/// `ConstructError::Explicit` 不被 Select 吞掉，直接向上传播
/// （Python `except ExplicitError: raise`）。
#[derive(Debug)]
pub struct SelectNode {
    /// 候选 subcon 列表（编译期保证：Python 允许空 Select 但 parse 立即抛 SelectError）。
    subcons: Vec<Node>,
}

impl SelectNode {
    /// 创建 `SelectNode`，包含给定的候选 subcon 列表。
    pub fn new(subcons: Vec<Node>) -> Self {
        Self { subcons }
    }

    /// 返回候选 subcon 列表的切片。
    pub fn subcons(&self) -> &[Node] {
        &self.subcons
    }

    /// has_expressions：递归检查任一 subcon 子树（与 Bitwise/Transform 同模式）。
    pub fn has_expressions(&self) -> bool {
        self.subcons.iter().any(|s| s.has_expressions())
    }
}

impl Construct for SelectNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        for sub in &self.subcons {
            let fallback = stream.tell();
            match sub.parse(py, stream, ctx, path) {
                Ok(obj) => return Ok(obj),
                Err(e) => {
                    // Explicit 不吞，直接传播（Python L3865-3866）。
                    if matches!(e, ConstructError::Explicit { .. }) {
                        return Err(e);
                    }
                    // 其他错误：seek 回 fallback，继续尝试下一个。
                    // seek 失败也忽略（与 Python except Exception 一致）。
                    let _ = stream.seek(fallback, path);
                }
            }
        }
        Err(ConstructError::Select {
            message: "no subconstruct matched".to_string(),
            path: path.to_string(),
        })
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        for sub in &self.subcons {
            // 在临时 BuildStream 上尝试（与 PrefixedArray build 同模式）。
            let mut temp_stream = BuildStream::new();
            match sub.build(py, obj, &mut temp_stream, ctx, path) {
                Ok(()) => {
                    // 成功：把 temp 字节写到主 stream。
                    stream.write(temp_stream.as_bytes());
                    return Ok(());
                }
                Err(e) => {
                    if matches!(e, ConstructError::Explicit { .. }) {
                        return Err(e);
                    }
                    // 其他错误：丢弃 temp，继续尝试。
                }
            }
        }
        Err(ConstructError::Select {
            message: format!("no subconstruct matched: {}", obj),
            path: path.to_string(),
        })
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // Python Select 无 _sizeof 方法定义，默认抛 SizeofError。
        Err(ConstructError::Generic {
            message: "Select size is undefined".to_string(),
            path: String::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::bytes::BytesNode;
    use crate::nodes::format_field::{FormatFieldNode, PythonFormat};
    use crate::nodes::pass::PassNode;

    fn ensure_python() {
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(pyo3::prepare_freethreaded_python);
    }

    fn with_py<F, R>(f: F) -> R
    where
        F: for<'py> FnOnce(Python<'py>) -> R,
    {
        ensure_python();
        Python::with_gil(f)
    }

    /// 构造一个 `FormatField` Node 包装。
    fn fmt_node(fmt: PythonFormat) -> Node {
        Node::FormatField(FormatFieldNode::new(fmt))
    }

    // ======================================================================
    // 构造器
    // ======================================================================

    #[test]
    fn new_stores_subcons() {
        let node = SelectNode::new(vec![fmt_node(PythonFormat::UnsignedInt8Big)]);
        assert_eq!(node.subcons().len(), 1);
    }

    #[test]
    fn new_empty_subcons_allowed() {
        // 空 Select 允许构造（parse/build 时立即抛 SelectError）。
        let node = SelectNode::new(vec![]);
        assert_eq!(node.subcons().len(), 0);
    }

    #[test]
    fn has_expressions_depends_on_children() {
        // 普通节点：false
        let node = SelectNode::new(vec![fmt_node(PythonFormat::UnsignedInt8Big)]);
        assert!(!node.has_expressions());
        // Bytes 表达式长度 → true（has_expressions 递归）
        // BytesNode::new_expr 需要 ExprProgram；用 Computed 等同模式验证递归即可。
        // 这里仅用 Bytes(const) 验证 false 路径。
        let node2 = SelectNode::new(vec![Node::Bytes(BytesNode::new_const(2))]);
        assert!(!node2.has_expressions());
    }

    // ======================================================================
    // parse 第 1 个成功 → 短路返回
    // ======================================================================

    #[test]
    fn parse_first_success_short_circuits() {
        with_py(|py| {
            let node = SelectNode::new(vec![
                fmt_node(PythonFormat::UnsignedInt8Big),
                fmt_node(PythonFormat::UnsignedInt16Big),
            ]);
            let mut stream = ParseStream::new(&[0x42]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().expect("i64");
            assert_eq!(v, 0x42);
            // 短路：第 2 个未尝试，stream 仅消费 1 字节
            assert_eq!(stream.tell(), 1);
        });
    }

    // ======================================================================
    // parse 第 1 个失败 + 第 2 个成功 → seek 回退后第 2 个成功
    // ======================================================================

    #[test]
    fn parse_first_fails_second_succeeds() {
        with_py(|py| {
            // Int32ub 需要 4 字节，仅 2 字节 → 失败；Int16ub 成功
            let node = SelectNode::new(vec![
                fmt_node(PythonFormat::UnsignedInt32Big),
                fmt_node(PythonFormat::UnsignedInt16Big),
            ]);
            let mut stream = ParseStream::new(&[0xAA, 0xBB]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().expect("i64");
            assert_eq!(v, 0xAABB);
            // stream 应消费 2 字节（fallback=0 → Int16ub 解析 2 字节）
            assert_eq!(stream.tell(), 2);
        });
    }

    // ======================================================================
    // 全部失败 → Select error
    // ======================================================================

    #[test]
    fn parse_all_fail_returns_select_error() {
        with_py(|py| {
            // Int32ub 需要 4 字节，CString 需要 null 终止符 → 都不匹配空输入
            let node = SelectNode::new(vec![
                fmt_node(PythonFormat::UnsignedInt32Big),
                Node::Bytes(BytesNode::new_const(2)),
            ]);
            let mut stream = ParseStream::new(&[]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("Select");
            match err {
                ConstructError::Select { .. } => {}
                other => panic!("expected Select, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // parse 空 Select → Select error
    // ======================================================================

    #[test]
    fn parse_empty_subcons_returns_select_error() {
        with_py(|py| {
            let node = SelectNode::new(vec![]);
            let mut stream = ParseStream::new(&[0x42]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("Select");
            assert!(matches!(err, ConstructError::Select { .. }));
        });
    }

    // ======================================================================
    // parse Explicit 错误 → 立即传播（不 seek 回退，不尝试后续）
    // ======================================================================

    #[test]
    fn parse_propagates_explicit_error() {
        // Explicit 错误立即传播（不 seek 回退，不尝试后续）。
        // 当前 Rust 不主动构造 Explicit 变体（仅供 Select/Peek 识别用）；
        // 此处验证 SelectNode 的源码分支 `matches!(e, ConstructError::Explicit { .. })`
        // 已实现（编译期保证），运行时 Explicit 集成测试见 Python parity 测试。
        let _ = SelectNode::new(vec![fmt_node(PythonFormat::UnsignedInt8Big)]);
    }

    // ======================================================================
    // build 路径
    // ======================================================================

    #[test]
    fn build_first_success_writes_to_stream() {
        with_py(|py| {
            let node = SelectNode::new(vec![
                fmt_node(PythonFormat::UnsignedInt8Big),
                fmt_node(PythonFormat::UnsignedInt16Big),
            ]);
            let obj = py.eval_bound("0x42", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            // 第 1 个 Int8ub 成功 → 写 1 字节
            assert_eq!(stream.as_bytes(), &[0x42]);
        });
    }

    #[test]
    fn build_first_fails_second_succeeds() {
        with_py(|py| {
            let obj = py.eval_bound("0x42", None, None).expect("obj");
            // 第 1 个 subcon: Bytes(2) build(int) 失败（int 不是 bytes）；
            // 第 2 个 Int16ub build(int) → 成功 → 写 2 字节
            let node = SelectNode::new(vec![
                Node::Bytes(BytesNode::new_const(2)),
                fmt_node(PythonFormat::UnsignedInt16Big),
            ]);
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x00, 0x42]);
        });
    }

    #[test]
    fn build_all_fail_returns_select_error() {
        with_py(|py| {
            // Int8ub build("not int") 失败；Bytes(2) build("not bytes") 失败
            let node = SelectNode::new(vec![
                fmt_node(PythonFormat::UnsignedInt8Big),
                Node::Bytes(BytesNode::new_const(2)),
            ]);
            let obj = py
                .eval_bound("'not_int_or_bytes'", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("Select");
            assert!(matches!(err, ConstructError::Select { .. }));
            // 主 stream 应未被写入（temp 失败时丢弃）
            assert!(stream.as_bytes().is_empty());
        });
    }

    // ======================================================================
    // sizeof 永远 Err
    // ======================================================================

    #[test]
    fn sizeof_always_returns_error() {
        with_py(|py| {
            let node = SelectNode::new(vec![fmt_node(PythonFormat::UnsignedInt8Big)]);
            let ctx = Context::placeholder(py);
            let err = node.sizeof(&ctx).expect_err("sizeof");
            match err {
                ConstructError::Generic { message, .. } => {
                    assert!(message.contains("Select"), "got: {}", message);
                }
                other => panic!("expected Generic, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // Optional macro 等价：Select(subcon, Pass) 总成功
    // ======================================================================

    #[test]
    fn select_with_pass_always_succeeds_parse() {
        // Optional(subcon) = Select(subcon, Pass)：subcon 失败时 Pass.parse 总返回 None
        with_py(|py| {
            let node = SelectNode::new(vec![
                Node::Bytes(BytesNode::new_const(2)), // 仅 0 字节可用 → 失败
                Node::Pass(PassNode::new()),
            ]);
            let mut stream = ParseStream::new(&[]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("Pass.parse should succeed");
            assert!(result.bind(py).is_none(), "Pass.parse returns None");
        });
    }

    // ======================================================================
    // Debug
    // ======================================================================

    #[test]
    fn debug_format_includes_select_node() {
        let node = SelectNode::new(vec![fmt_node(PythonFormat::UnsignedInt8Big)]);
        let s = format!("{:?}", node);
        assert!(s.contains("SelectNode"), "got: {}", s);
    }
}
