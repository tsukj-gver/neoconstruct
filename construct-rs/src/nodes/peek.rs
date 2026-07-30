//! PeekNode：预读不消费流的节点。
//!
//! 设计依据：`docs/design/模块设计/模块设计-Adapter核心.md` §1.2。
//! Python 参考：`construct/construct/core.py` `Peek`（L4486-4545）。
//!
//! ## 行为概述
//!
//! Peek 在 parse 时执行 inner.parse，结束后无论成功/失败都 seek 回入口位置
//! （不消费字节）。Python 用 try/except/finally 实现此语义；construct-rs 用
//! `ParseStream::seek` + Result 匹配（§0 #8 Stream 抽象纯 Rust 内部）。
//!
//! ## 已知差异（设计 §5.3 PE-3）
//!
//! Python `Peek` 在 inner.parse 抛 `ExplicitError` 时**不吞掉**（向上传播）。
//! construct-rs 当前 `ConstructError` 枚举无 `Explicit` 变体（PM 决策 6.3-D2：
//! Phase 6 不引入）。`is_explicit_error` 占位实现统一返回 `false`——所有
//! 错误都被 Peek 吞掉（返回 Py_None）。Phase 7 Select 实现时若需引入，再补。

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

use super::Construct;
use crate::nodes::Node;

/// 预读节点：parse 子解析后回退 stream 到入口位置（不消费字节）。
///
/// 对应 Python construct `Peek`（core.py L4486）。
///
/// # 三方法行为
///
/// - parse：记录 fallback = stream.tell()；调用 inner.parse；
///   无论成功/失败都 seek 回 fallback。成功 → 返回 inner 结果；
///   失败且非 explicit → 返回 Py_None（吞掉错误）；失败且 explicit → 向上传播
/// - build：no-op（对齐 Python `return obj`，但 construct-rs build 不返回值）
/// - sizeof：返回 0
///
/// # ExplicitError 处理（设计 §5.3 PE-3）
///
/// construct-rs 当前无 `ConstructError::Explicit` 变体（PM 决策 6.3-D2）。
/// [`is_explicit_error`] 占位返回 `false`，所有错误都被吞掉。
#[derive(Debug)]
pub struct PeekNode {
    /// 被预读的子树根。
    inner: Box<Node>,
}

impl PeekNode {
    /// 创建 `PeekNode`，包裹给定的子树根节点。
    pub fn new(inner: Node) -> Self {
        Self {
            inner: Box::new(inner),
        }
    }

    /// 返回内部子树根节点的引用。
    pub fn inner(&self) -> &Node {
        &self.inner
    }
}

/// 判断错误是否为"ExplicitError 等价物"（不被 Peek 吞掉）。
///
/// **PM 决策 6.3-D2（Phase 6 不引入 ExplicitError 变体）**：当前 `ConstructError`
/// 枚举无 `Explicit` 变体。本函数占位返回 `false`——所有错误都被 Peek 吞掉。
///
/// 未来若引入 `ConstructError::Explicit` 变体（如 Phase 7 Select 需要），
/// 改为 `matches!(e, ConstructError::Explicit { .. })` 一行即可。
fn is_explicit_error(_e: &ConstructError) -> bool {
    false
}

impl Construct for PeekNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let fallback = stream.tell();
        let result = self.inner.parse(py, stream, ctx, path);
        // 无论成功/失败，最终都 seek 回 fallback（对齐 Python finally）
        let seek_outcome = stream.seek(fallback, path);
        match result {
            Ok(value) => {
                // seek 自身失败转 Err（优先于已成功的 parse 结果）
                seek_outcome?;
                Ok(value)
            }
            Err(e) => {
                // seek 失败优先上报（即使原错误被吞）
                seek_outcome?;
                if is_explicit_error(&e) {
                    return Err(e);
                }
                // 非 explicit：吞掉，返回 None（对齐 Python `except ConstructError: pass`）
                Ok(py.None())
            }
        }
    }

    fn build(
        &self,
        _py: Python<'_>,
        _obj: &Bound<'_, PyAny>,
        _stream: &mut BuildStream,
        _ctx: &mut Context<'_>,
        _path: &mut Path,
    ) -> Result<(), ConstructError> {
        // build no-op（对齐 Python `return obj`，但 construct-rs build 不返回值）
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
    use crate::nodes::bytes::BytesNode;
    use crate::nodes::format_field::{FormatFieldNode, PythonFormat};

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
    fn new_stores_inner() {
        let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
        let node = PeekNode::new(inner);
        assert!(matches!(node.inner(), Node::FormatField(_)));
    }

    // ======================================================================
    // parse — 成功路径
    // ======================================================================

    #[test]
    fn parse_returns_inner_value_without_consuming_stream() {
        // PE-1: Peek(Int8ub) parse 1 字节 → 返回值，stream.tell() 不变
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let node = PeekNode::new(inner);

            let mut stream = ParseStream::new(&[0x42, 0x99]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().unwrap();
            assert_eq!(v, 0x42);
            // stream.tell() 应仍为 0（Peek 不消费字节）
            assert_eq!(stream.tell(), 0);
        });
    }

    #[test]
    fn parse_does_not_advance_stream_after_success() {
        with_py(|py| {
            let inner = Node::Bytes(BytesNode::new_const(3));
            let node = PeekNode::new(inner);

            let mut stream = ParseStream::new(&[0x01, 0x02, 0x03, 0x04]);
            // 先消费 1 字节使 tell != 0
            let _ = stream.read(1, &Path::new()).expect("read 1");
            assert_eq!(stream.tell(), 1);

            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let _ = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // Peek 应 seek 回 fallback=1，不推进
            assert_eq!(stream.tell(), 1);
        });
    }

    // ======================================================================
    // parse — 失败路径（吞掉错误）
    // ======================================================================

    #[test]
    fn parse_returns_none_when_inner_fails() {
        // PE-2: Peek(Int32ub) parse 1 字节 → inner 失败 → Peek 吞掉返回 None，
        // stream 已 seek 回 fallback
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt32Big));
            let node = PeekNode::new(inner);

            let mut stream = ParseStream::new(&[0x01]); // 仅 1 字节，Int32ub 需要 4 字节
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("Peek should swallow inner error");
            assert!(result.bind(py).is_none());
            assert_eq!(stream.tell(), 0); // seek 回 fallback
        });
    }

    #[test]
    fn parse_preserves_stream_position_on_failure() {
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
            let node = PeekNode::new(inner);

            let mut stream = ParseStream::new(&[0xAA, 0xBB]);
            // 先消费 1 字节
            let _ = stream.read(1, &Path::new()).expect("read 1");
            assert_eq!(stream.tell(), 1);

            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            // 残留 1 字节，Int16ub 需要 2 → 失败
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("Peek should swallow");
            assert!(result.bind(py).is_none());
            // seek 回 fallback=1（入口位置）
            assert_eq!(stream.tell(), 1);
        });
    }

    // ======================================================================
    // build (no-op)
    // ======================================================================

    #[test]
    fn build_is_noop_returns_ok() {
        // PE-4: Peek.build no-op
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let node = PeekNode::new(inner);

            let obj = py.eval_bound("42", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert!(stream.as_bytes().is_empty(), "Peek build should not write");
        });
    }

    #[test]
    fn build_does_not_advance_stream() {
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let node = PeekNode::new(inner);

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
        // PE-5: Peek.sizeof() = 0
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let node = PeekNode::new(inner);
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).expect("sizeof"), 0);
        });
    }

    #[test]
    fn sizeof_zero_regardless_of_inner_size() {
        with_py(|py| {
            // inner 是 Int32ub（4 字节），但 Peek.sizeof 仍为 0
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt32Big));
            let node = PeekNode::new(inner);
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).expect("sizeof"), 0);
        });
    }

    // ======================================================================
    // is_explicit_error 占位
    // ======================================================================

    #[test]
    fn is_explicit_error_always_false_in_phase6() {
        // PM 决策 6.3-D2：Phase 6 不引入 Explicit 变体
        let err = ConstructError::Stream {
            message: "test".to_string(),
            path: "root".to_string(),
        };
        assert!(!is_explicit_error(&err));

        let err = ConstructError::Generic {
            message: "test".to_string(),
            path: "root".to_string(),
        };
        assert!(!is_explicit_error(&err));
    }

    // ======================================================================
    // Debug
    // ======================================================================

    #[test]
    fn debug_format_includes_peek_node() {
        let inner = Node::Bytes(BytesNode::new_const(1));
        let node = PeekNode::new(inner);
        let s = format!("{:?}", node);
        assert!(s.contains("PeekNode"), "got: {}", s);
    }
}
