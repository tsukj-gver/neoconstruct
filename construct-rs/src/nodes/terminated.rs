//! TerminatedNode：EOF 断言节点。
//!
//! 设计依据：`docs/design/模块设计/模块设计-Phase8-P0.md` §5.2.1。
//! Python 参考：`construct/construct/core.py` `Terminated`（L4727-4755）。
//!
//! ## 行为概述
//!
//! Terminated 是单元节点：parse 时若 stream 未到 EOF 抛 TerminatedError。
//! build 是 no-op；sizeof 返回 Err（SizeofError 等价）。
//!
//! ## 与 Python 实现的差异
//!
//! Python 用 `stream.read(1)` 判断 EOF——若 read 成功（返回 b"x"）则抛错，
//! EOF（返回 b""）则通过。construct-rs 直接用 `stream.remaining() > 0` 判断，
//! **不消费字节**——但语义等价（抛错路径下 stream 状态不重要）。

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

use super::Construct;

/// EOF 断言节点：parse 时若 stream 未到 EOF 抛 TerminatedError。
///
/// 对应 Python construct `Terminated`（core.py L4727）。Python 用
/// `stream.read(1)` 判断（EOF 时返回 b""）；construct-rs 直接用
/// `stream.remaining() > 0` 判断（更直接，不消费字节）。
///
/// # 三方法行为
///
/// - parse：`stream.remaining() > 0` → TerminatedError；否则返回 Py_None
/// - build：no-op（对齐 Python `return obj`）
/// - sizeof：返回 Err（SizeofError 等价）
#[derive(Debug, Default)]
pub struct TerminatedNode;

impl TerminatedNode {
    /// 创建 `TerminatedNode`（零字段单元结构体）。
    pub fn new() -> Self {
        Self
    }
}

impl Construct for TerminatedNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        _ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        if stream.remaining() > 0 {
            return Err(ConstructError::Terminated {
                message: "expected end of stream".to_string(),
                path: path.to_string(),
            });
        }
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
        // no-op（对齐 Python `return obj`）
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        Err(ConstructError::Generic {
            message: "size of Terminated is undefined".to_string(),
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
    use crate::nodes::Construct;

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
        let node = TerminatedNode::new();
        let _ = format!("{:?}", node);
    }

    #[test]
    fn default_equals_new() {
        let _n1 = TerminatedNode::new();
        fn assert_default<T: Default>() {}
        assert_default::<TerminatedNode>();
    }

    // ======================================================================
    // parse — TM-1 / TM-2
    // ======================================================================

    #[test]
    fn parse_at_eof_returns_none() {
        // TM-1: Terminated.parse(b"") → None
        with_py(|py| {
            let node = TerminatedNode::new();
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert!(result.is(&py.None()));
        });
    }

    #[test]
    fn parse_not_at_eof_raises_terminated_error() {
        // TM-2: Terminated.parse(b"remaining") → TerminatedError
        with_py(|py| {
            let node = TerminatedNode::new();
            let mut stream = ParseStream::new(b"remaining");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Terminated { message, path: p } => {
                    assert!(
                        message.contains("expected end of stream"),
                        "got: {}",
                        message
                    );
                    assert_eq!(p, "root");
                }
                other => panic!("expected Terminated error, got {:?}", other),
            }
        });
    }

    #[test]
    fn parse_does_not_consume_bytes() {
        // 即使 EOF 检查通过（无字节可消费），调用前若 pos < len 也不应推进
        with_py(|py| {
            let node = TerminatedNode::new();
            let mut stream = ParseStream::new(b"abc");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let _ = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail (stream has bytes)");
            assert_eq!(stream.tell(), 0, "Terminated should not advance stream");
        });
    }

    // ======================================================================
    // build — TM-3 / TM-4
    // ======================================================================

    #[test]
    fn build_none_is_noop() {
        // TM-3: Terminated.build(None) → no-op
        with_py(|py| {
            let node = TerminatedNode::new();
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert!(stream.as_bytes().is_empty());
        });
    }

    #[test]
    fn build_arbitrary_obj_is_noop() {
        // TM-4: Terminated.build(42) → no-op（忽略 obj）
        with_py(|py| {
            let node = TerminatedNode::new();
            let obj = py.eval_bound("42", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert!(stream.as_bytes().is_empty());
        });
    }

    // ======================================================================
    // sizeof — TM-5
    // ======================================================================

    #[test]
    fn sizeof_returns_err() {
        // TM-5: Terminated.sizeof() → Err（SizeofError 等价）
        with_py(|py| {
            let node = TerminatedNode::new();
            let ctx = Context::placeholder(py);
            let err = node.sizeof(&ctx).expect_err("should fail");
            assert!(matches!(err, ConstructError::Generic { .. }));
        });
    }

    // ======================================================================
    // Debug
    // ======================================================================

    #[test]
    fn debug_format_includes_terminated_node() {
        let node = TerminatedNode::new();
        let s = format!("{:?}", node);
        assert!(s.contains("TerminatedNode"), "got: {}", s);
    }
}
