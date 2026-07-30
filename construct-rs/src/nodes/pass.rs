//! PassNode：no-op 节点（不消费字节、不写字节）。
//!
//! 设计依据：`docs/design/模块设计/模块设计-Adapter核心.md` §2。
//! Python 参考：`construct/construct/core.py` `Pass`（L4687-4714）。
//!
//! ## 行为概述
//!
//! Pass 是单元节点：parse 返回 `Py_None`（不消费字节）；
//! build 不写字节；sizeof 恒为 0。
//!
//! ## Phase 7 依赖
//!
//! Pass 是 Phase 7 `If`/`Switch` 默认值的依赖：
//! - `If(cond, then)` ≡ `IfThenElse(cond, then, Pass)`
//! - `Switch(key, cases, default=Pass)`
//!
//! 作为 Phase 6.3 的"附带品"实现（< 100 行 Rust，含测试）。

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

use super::Construct;

/// No-op 节点：parse 返回 `Py_None`，build 不写字节，sizeof=0。
///
/// 对应 Python construct `Pass`（core.py L4687）。Python 是 `@singleton`，
/// construct-rs 用零字段单元结构体（编译期占位，运行时零开销）。
///
/// # 三方法行为
///
/// - parse：返回 `Py_None`（不消费字节）
/// - build：no-op（不写字节）
/// - sizeof：返回 0
///
/// # Phase 7 依赖
///
/// Pass 是 Phase 7 `If`/`Switch` 默认值的依赖：
/// - `If(cond, then)` ≡ `IfThenElse(cond, then, Pass)`
/// - `Switch(key, cases, default=Pass)`
#[derive(Debug, Default)]
pub struct PassNode;

impl PassNode {
    /// 创建一个 `PassNode`（零字段单元结构体）。
    pub fn new() -> Self {
        Self
    }
}

impl Construct for PassNode {
    fn parse(
        &self,
        py: Python<'_>,
        _stream: &mut ParseStream<'_>,
        _ctx: &mut Context<'_>,
        _path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        Ok(py.None())
    }

    fn build(
        &self,
        _py: Python<'_>,
        _obj: &Bound<'_, PyAny>,
        _stream: &mut BuildStream,
        _ctx: &mut Context<'_>,
        _path: &mut Path,
    ) -> Result<(), ConstructError> {
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        Ok(0)
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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

    // ======================================================================
    // 构造器
    // ======================================================================

    #[test]
    fn new_returns_default_instance() {
        let node = PassNode::new();
        let _ = format!("{:?}", node);
    }

    #[test]
    fn default_equals_new() {
        // PassNode 是单元结构体，new() 和直接构造等价。
        // 注：clippy::default_constructed_unit_structs 提示对单元结构体用
        // Default::default() 多余，此处仅验证 new 与 Default trait 可用。
        let _n1 = PassNode::new();
        // 验证 PassNode 实现 Default（编译期检查，不实际调用 default()）
        fn assert_default<T: Default>() {}
        assert_default::<PassNode>();
    }

    // ======================================================================
    // parse
    // ======================================================================

    #[test]
    fn parse_empty_stream_returns_none() {
        with_py(|py| {
            let node = PassNode::new();
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert!(result.bind(py).is_none(), "Pass.parse should return None");
        });
    }

    #[test]
    fn parse_non_empty_stream_returns_none() {
        with_py(|py| {
            let node = PassNode::new();
            let mut stream = ParseStream::new(b"\x01\x02\x03");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert!(result.bind(py).is_none());
        });
    }

    #[test]
    fn parse_does_not_consume_bytes() {
        with_py(|py| {
            let node = PassNode::new();
            let mut stream = ParseStream::new(b"abc");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let _ = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert_eq!(stream.tell(), 0, "Pass should not advance stream");
            assert_eq!(stream.remaining(), 3);
        });
    }

    // ======================================================================
    // build
    // ======================================================================

    #[test]
    fn build_none_is_noop() {
        with_py(|py| {
            let node = PassNode::new();
            let obj = py.eval_bound("None", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert!(stream.as_bytes().is_empty(), "Pass build should not write");
        });
    }

    #[test]
    fn build_arbitrary_obj_is_noop() {
        // build 接受任意 obj（对齐 Python，PA-4：忽略 obj）
        with_py(|py| {
            let node = PassNode::new();
            let obj = py.eval_bound("42", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert!(stream.as_bytes().is_empty());
        });
    }

    #[test]
    fn build_does_not_advance_stream() {
        with_py(|py| {
            let node = PassNode::new();
            let obj = py.eval_bound("None", None, None).expect("obj");
            let mut stream = BuildStream::new();
            stream.write(b"abc");
            let pos_before = stream.tell();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.tell(), pos_before);
        });
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_returns_zero() {
        with_py(|py| {
            let node = PassNode::new();
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).expect("sizeof"), 0);
        });
    }

    #[test]
    fn sizeof_zero_on_root_context() {
        with_py(|py| {
            let node = PassNode::new();
            let ctx = Context::new_root(py).expect("root");
            assert_eq!(node.sizeof(&ctx).expect("sizeof"), 0);
        });
    }

    // ======================================================================
    // Debug
    // ======================================================================

    #[test]
    fn debug_format_includes_pass_node() {
        let node = PassNode::new();
        let s = format!("{:?}", node);
        assert!(s.contains("PassNode"), "got: {}", s);
    }
}
