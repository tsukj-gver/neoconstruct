//! PaddingNode：字节级填充节点。
//!
//! Python 参考：`construct/construct/core.py` `Padding(length, pattern)`（L4136），
//! 字节域行为通过 `Padded(length, Pass, pattern)` 实现（L4175）。
//!
//! ## 职责
//!
//! PaddingNode 在字节域跳过或写入 `length` 个字节。parse 返回 Python `None`，
//! build 写入 `pattern * length` 字节。
//!
//! ## 与 BitPaddingNode 的关系
//!
//! - 字节域（普通 Struct 内）：使用 [`PaddingNode`]（本节点）。
//! - bit 域（Bitwise/BitStruct 内）：使用 [`super::bit_padding::BitPaddingNode`]。
//!
//! 单位由编译期 `bitwise` 上下文决定：编译管线在
//! `PaddingDescriptor` 分支根据 `bitwise` 标志选择具体节点。

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

use super::Construct;

// ---------------------------------------------------------------------------
// PaddingNode
// ---------------------------------------------------------------------------

/// 字节级填充节点：跳过/写入 `length` 个字节。
///
/// 对应 Python construct 的 `Padding(length)` 在字节域的行为
/// （等价于 `Padded(length, Pass, pattern)`，subcon 固定为 Pass）。
///
/// # parse 行为
///
/// 从流中读取 `length` 字节并丢弃，返回 Python `None`。
///
/// # build 行为
///
/// 写入 `pattern * length` 字节。接受任意对象（忽略 obj）。
///
/// # sizeof
///
/// 返回 `length`（字节数）。
///
/// # 错误
///
/// - length 表达式求值为负 → 返回 `Padding` 错误（编译期已拒绝，运行时不会触发）
/// - 流中字节不足 → parse 返回 `Stream` 错误
/// - length == 0 → 不读写，返回 None
///
/// # 支持范围
///
/// 仅支持常量 length（编译期校验）。表达式 length 由编译管线在
/// `build_node_from_descriptor` 中拒绝（返回 `Compilation` 错误）。
#[derive(Debug, Clone, Copy)]
pub struct PaddingNode {
    /// 字节长度。
    length: usize,
    /// 填充字节模式（0-255）。
    pattern: u8,
}

impl PaddingNode {
    /// 创建一个常量长度的字节级填充节点。
    ///
    /// # 参数
    ///
    /// - `length`：填充字节数（非负）。
    /// - `pattern`：填充字节模式（0-255）。Python 默认 `b"\\x00"` → 0。
    pub fn new_const(length: usize, pattern: u8) -> Self {
        Self { length, pattern }
    }

    /// 返回填充字节数。
    pub fn length(&self) -> usize {
        self.length
    }

    /// 返回填充字节模式。
    pub fn pattern(&self) -> u8 {
        self.pattern
    }
}

impl Construct for PaddingNode {
    fn parse(
        &self,
        py: Python<'_>,
        stream: &mut ParseStream<'_>,
        _ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 读取 length 字节并丢弃。
        // read(0) 返回空切片（不推进游标）。
        let _ = stream.read(self.length, path)?;
        Ok(py.None())
    }

    fn build(
        &self,
        _py: Python<'_>,
        _obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        _ctx: &mut Context<'_>,
        _path: &mut Path,
    ) -> Result<(), ConstructError> {
        // 忽略 obj，写入 length 个 pattern 字节。
        if self.length == 0 {
            return Ok(());
        }
        // 预分配 Vec 并填充 pattern（避免 push 循环）。
        let buf = vec![self.pattern; self.length];
        stream.write(&buf);
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        Ok(self.length)
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
    fn new_const_stores_params() {
        let node = PaddingNode::new_const(4, 0xAB);
        assert_eq!(node.length(), 4);
        assert_eq!(node.pattern(), 0xAB);
    }

    #[test]
    fn new_const_zero_length() {
        let node = PaddingNode::new_const(0, 0x00);
        assert_eq!(node.length(), 0);
    }

    #[test]
    fn new_const_default_pattern() {
        let node = PaddingNode::new_const(4, 0x00);
        assert_eq!(node.pattern(), 0);
    }

    // ======================================================================
    // parse
    // ======================================================================

    #[test]
    fn parse_skips_length_bytes() {
        with_py(|py| {
            let node = PaddingNode::new_const(3, 0x00);
            let mut stream = ParseStream::new(b"\x01\x02\x03\x04");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // 返回 None
            assert!(result.is(&py.None()));
            // 跳过 3 字节
            assert_eq!(stream.tell(), 3);
            // 剩余 1 字节
            let rest = stream.read(1, &path).expect("read rest");
            assert_eq!(rest, &[0x04]);
        });
    }

    #[test]
    fn parse_zero_length_is_noop() {
        // length=0 不读写
        with_py(|py| {
            let node = PaddingNode::new_const(0, 0x00);
            let mut stream = ParseStream::new(b"abc");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let _ = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert_eq!(stream.tell(), 0);
        });
    }

    #[test]
    fn parse_insufficient_bytes_returns_stream_error() {
        // 流中只有 2 字节，请求 4 字节
        with_py(|py| {
            let node = PaddingNode::new_const(4, 0x00);
            let mut stream = ParseStream::new(b"\x01\x02");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Stream { message, .. } => {
                    assert!(message.contains("expected 4"), "got: {}", message);
                    assert!(message.contains("found 2"), "got: {}", message);
                }
                other => panic!("expected Stream, got {:?}", other),
            }
            assert_eq!(stream.tell(), 0);
        });
    }

    // ======================================================================
    // build
    // ======================================================================

    #[test]
    fn build_writes_pattern_repeated() {
        with_py(|py| {
            let node = PaddingNode::new_const(4, 0xAB);
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0xAB, 0xAB, 0xAB, 0xAB]);
        });
    }

    #[test]
    fn build_default_pattern_zero() {
        with_py(|py| {
            let node = PaddingNode::new_const(4, 0x00);
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x00, 0x00, 0x00, 0x00]);
        });
    }

    #[test]
    fn build_zero_length_is_noop() {
        with_py(|py| {
            let node = PaddingNode::new_const(0, 0xAB);
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.tell(), 0);
        });
    }

    #[test]
    fn build_ignores_obj() {
        // obj 可以是任意值（非 None）
        with_py(|py| {
            let node = PaddingNode::new_const(2, 0xFF);
            let obj = py.eval_bound("'hello'", None, None).expect("string");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0xFF, 0xFF]);
        });
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_returns_length_in_bytes() {
        with_py(|py| {
            let ctx = Context::placeholder(py);
            assert_eq!(PaddingNode::new_const(0, 0x00).sizeof(&ctx).unwrap(), 0);
            assert_eq!(PaddingNode::new_const(1, 0x00).sizeof(&ctx).unwrap(), 1);
            assert_eq!(PaddingNode::new_const(100, 0xAB).sizeof(&ctx).unwrap(), 100);
        });
    }

    // ======================================================================
    // parse ↔ build 往返
    // ======================================================================

    #[test]
    fn round_trip_padding_default_pattern() {
        with_py(|py| {
            let node = PaddingNode::new_const(4, 0x00);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();

            let mut pstream = ParseStream::new(&bytes);
            let _ = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            assert!(pstream.is_at_end());
        });
    }
}
