//! TellNode：记录当前流位置（不消费字节）。
//!
//! 设计依据：`docs/模块设计-表达式系统.md` §4.6.1。
//! Python 参考：`construct/construct/core.py` `Tell`（L1751-1770）。
//!
//! ## 行为概述
//!
//! Tell 是 RO（只读）节点：parse 返回当前 `ParseStream` 的读取位置，
//! build 不写字节（位置记录由 [`crate::nodes::struct_node`] 的
//! `compute_ro_value` 在 RO 字段处理时完成）。sizeof 恒为 0。
//!
//! ## 典型用法
//!
//! ```python
//! @dataclass
//! class Packet(StructMixin):
//!     start: int = rfield(Tell())
//!     count: int = field(Int8ub)
//!     end: int = rfield(Tell())
//!     size: int = rfield(Computed(end - start))
//! ```

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

use super::Construct;

/// 记录当前流位置。对应 Python construct 的 `Tell`。
///
/// parse 返回当前 `ParseStream` 的读取位置（`usize` → `PyLong`）。
/// sizeof 返回 0（不占字节）。
///
/// build 方向是 no-op：位置记录由 [`crate::nodes::struct_node`] 的
/// `compute_ro_value` 在 RO 字段处理时完成（设计 §5.3-§5.4）。
#[derive(Debug)]
pub struct TellNode;

impl TellNode {
    /// 创建一个 `TellNode`。
    pub fn new() -> Self {
        Self
    }
}

impl Default for TellNode {
    fn default() -> Self {
        Self::new()
    }
}

impl Construct for TellNode {
    fn parse(
        &self,
        py: Python<'_>,
        stream: &mut ParseStream<'_>,
        _ctx: &mut Context<'_>,
        _path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let pos = stream.tell();
        Ok(pos.into_py(py)) // usize → PyLong
    }

    fn build(
        &self,
        _py: Python<'_>,
        _obj: &Bound<'_, PyAny>,
        _stream: &mut BuildStream,
        _ctx: &mut Context<'_>,
        _path: &mut Path,
    ) -> Result<(), ConstructError> {
        // Tell build 是 no-op。位置记录由 StructNode 的 compute_ro_value 处理。
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
    use pyo3::types::PyString;

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
    // parse
    // ======================================================================

    #[test]
    fn parse_returns_zero_at_start_of_stream() {
        with_py(|py| {
            let node = TellNode::new();
            let mut stream = ParseStream::new(b"abc");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let n: i64 = result.bind(py).extract().expect("extract i64");
            assert_eq!(n, 0);
        });
    }

    #[test]
    fn parse_returns_nonzero_after_read() {
        with_py(|py| {
            let node = TellNode::new();
            // 先消费 2 字节
            let mut stream = ParseStream::new(b"abcdef");
            let _ = stream.read(2, &Path::new()).expect("read 2");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let n: i64 = result.bind(py).extract().expect("extract i64");
            assert_eq!(n, 2);
        });
    }

    #[test]
    fn parse_multiple_calls_track_position() {
        // 多次调用 Tell 应返回递增的位置（流位置未被 Tell 改变）。
        with_py(|py| {
            let node = TellNode::new();
            let mut stream = ParseStream::new(b"xyz");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            // 位置 0
            let r1 = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse 1");
            assert_eq!(r1.bind(py).extract::<i64>().unwrap(), 0);

            // 消费 1 字节
            let _ = stream.read(1, &Path::new()).expect("read 1");

            // 位置 1
            let r2 = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse 2");
            assert_eq!(r2.bind(py).extract::<i64>().unwrap(), 1);

            // 消费 2 字节
            let _ = stream.read(2, &Path::new()).expect("read 2");

            // 位置 3
            let r3 = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse 3");
            assert_eq!(r3.bind(py).extract::<i64>().unwrap(), 3);
        });
    }

    #[test]
    fn parse_does_not_consume_bytes() {
        // Tell 不应改变流位置。
        with_py(|py| {
            let node = TellNode::new();
            let mut stream = ParseStream::new(b"abc");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let _ = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert_eq!(stream.tell(), 0, "Tell should not advance stream");
        });
    }

    #[test]
    fn parse_returns_pylong() {
        // parse 返回的对象应为 Python int。
        with_py(|py| {
            let node = TellNode::new();
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // int 类型的 Python type name 应为 "int"
            let type_name = result
                .bind(py)
                .get_type()
                .name()
                .expect("type name")
                .to_string();
            assert_eq!(type_name, "int");
        });
    }

    // ======================================================================
    // build (no-op)
    // ======================================================================

    #[test]
    fn build_is_noop_returns_ok() {
        with_py(|py| {
            let node = TellNode::new();
            let obj = py.eval_bound("0", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert!(stream.as_bytes().is_empty(), "Tell build should not write");
        });
    }

    #[test]
    fn build_does_not_advance_stream() {
        with_py(|py| {
            let node = TellNode::new();
            let obj = py.eval_bound("42", None, None).expect("obj");
            let mut stream = BuildStream::new();
            // 预先写入一些字节
            stream.write(b"abc");
            let pos_before = stream.tell();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.tell(), pos_before, "Tell build should not advance");
        });
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_returns_zero() {
        with_py(|py| {
            let node = TellNode::new();
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).expect("sizeof"), 0);
        });
    }

    #[test]
    fn sizeof_zero_on_root_context() {
        with_py(|py| {
            let node = TellNode::new();
            let ctx = Context::new_root(py).expect("root");
            assert_eq!(node.sizeof(&ctx).expect("sizeof"), 0);
        });
    }

    // ======================================================================
    // 构造器与 Default
    // ======================================================================

    #[test]
    fn new_returns_default_instance() {
        let node = TellNode::new();
        let _ = format!("{:?}", node); // Debug 可用
    }

    #[test]
    fn default_equals_new() {
        let _new = TellNode::new();
        let _default = TellNode::default();
        // 两者都是 unit-like struct，无需等值比较
    }

    // ======================================================================
    // 类型无关属性
    // ======================================================================

    #[test]
    fn tell_node_is_debug() {
        let node = TellNode::new();
        let s = format!("{:?}", node);
        assert!(s.contains("TellNode"), "got: {}", s);
    }

    #[test]
    fn parse_ignores_context_state() {
        // Tell 不读取 context 内容，即使 context 已有字段也能正常工作。
        with_py(|py| {
            let node = TellNode::new();
            let mut ctx = Context::new_root(py).expect("root");
            let val = 42i64.into_py(py);
            ctx.set_field("count", val.bind(py)).expect("set count");
            let mut stream = ParseStream::new(b"ab");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let n: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(n, 0);
        });
    }

    #[test]
    fn parse_works_with_empty_stream() {
        // 空 stream 上调用 Tell 应返回 0。
        with_py(|py| {
            let node = TellNode::new();
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let n: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(n, 0);
        });
    }

    #[test]
    fn parse_does_not_depend_on_path() {
        // Path 内容不影响 parse 结果。
        with_py(|py| {
            let node = TellNode::new();
            let mut stream = ParseStream::new(b"abc");
            let _ = stream.read(1, &Path::new()).expect("read 1");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            path.push_field("outer");
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let n: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(n, 1);
        });
    }

    // 让 _ 绑定避免 unused 警告
    #[test]
    fn debug_repr_format() {
        let node = TellNode::new();
        assert_eq!(format!("{:?}", node), "TellNode");
    }

    #[allow(dead_code)]
    fn _ensure_pystring_used(_s: &PyString) {}
}
