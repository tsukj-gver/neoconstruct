//! BitPaddingNode：bit 级填充节点。
//!
//! Python 参考：`construct/construct/core.py` `Padding`（L4136，bit 域内通过
//! `Padded._build` 写 `self.pattern * pad`，L4239）；
//! `construct/construct/lib/binary.py` `BITS2BYTES_CACHE`（L108）。
//!
//! ## 职责
//!
//! BitPaddingNode 在 bit 域内跳过或写入 `length` 个 bit。parse 返回 Python `None`，
//! build 接受任意对象（忽略），用 `pattern_bit` 填充。
//!
//! ## pattern 严格校验
//!
//! Python `bits2bytes` 通过 `BITS2BYTES_CACHE` 查表，cache 键仅含 `0x00`/`0x01` 序列。
//! 因此 bit 域 Padding 的 pattern 字节只能是 `0x00`（填 0 bit）或 `0x01`（填 1 bit），
//! 其他值在 Python 端触发 `KeyError`。construct-rs 在 `BitPaddingNode::new` 编译期
//! 严格校验 `pattern ∈ {0x00, 0x01}`，否则返回 `ConstructError::Padding`
//! （fail-fast，比 Python 延迟 KeyError 更早暴露）。

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

use super::Construct;

// ---------------------------------------------------------------------------
// BitPaddingNode
// ---------------------------------------------------------------------------

/// bit 级填充节点：跳过/写入 `length` 个 bit。
///
/// 详见模块级文档。
///
/// # 错误
///
/// - `length == 0` → parse 不推进游标返回 None；build 不写入
/// - 流中 bit 数不足 → parse 返回 `Stream` 错误
/// - build 时 obj 为任意值（含 None） → 忽略 obj，写入 pattern bit
///
/// # pattern 合法值
///
/// 仅接受 `0x00` 或 `0x01`（对齐 Python `bits2bytes` 查表键的约束）。
/// 其他值在 `new` 时返回 `ConstructError::Padding`。
#[derive(Debug, Clone, Copy)]
pub struct BitPaddingNode {
    /// bit 宽度。
    length: usize,
    /// 填充 bit（0 或 1）。
    pattern_bit: u8,
}

impl BitPaddingNode {
    /// 创建 bit 级填充节点。
    ///
    /// `pattern` 必须是 `0x00` 或 `0x01`（对齐 Python bit 域 Padding 的合法值），
    /// 其他值返回 `ConstructError::Padding`（对应 Python bits2bytes 的 KeyError，
    /// 但在编译期/构造期提前暴露）。
    ///
    /// `pattern_bit` 由 `pattern & 1` 导出（对合法值等价于 pattern 本身）。
    ///
    /// # 参数
    ///
    /// - `length`：填充的 bit 数（非负）。
    /// - `pattern`：填充字节值，必须是 `0x00` 或 `0x01`。
    pub fn new(length: usize, pattern: u8) -> Result<Self, ConstructError> {
        // 严格校验 pattern 合法性。
        // Python 的 BITS2BYTES_CACHE 键仅含 0x00/0x01，其他值会 KeyError。
        if pattern != 0x00 && pattern != 0x01 {
            return Err(ConstructError::Padding {
                message: format!(
                    "bit-domain padding pattern must be 0x00 or 0x01, got 0x{:02x}",
                    pattern
                ),
                // 编译期错误，path 由上层 with_field_context 附加字段名上下文。
                path: String::new(),
            });
        }
        Ok(Self {
            length,
            pattern_bit: pattern & 1,
        })
    }

    /// 返回填充 bit 数。
    pub fn length(&self) -> usize {
        self.length
    }

    /// 返回填充 bit 值（0 或 1）。
    pub fn pattern_bit(&self) -> u8 {
        self.pattern_bit
    }
}

impl Construct for BitPaddingNode {
    fn parse(
        &self,
        py: Python<'_>,
        stream: &mut ParseStream<'_>,
        _ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 跳过 length 个 bit（stream.skip_bits 处理 length==0 与流不足）。
        stream.skip_bits(self.length, path)?;
        // 返回 Python None（Padding 在 parse 方向丢弃数据）。
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
        // 忽略 obj（Padding 接受任意值），写入 length 个 pattern_bit。
        // write_padding_bits 内部处理 length==0（no-op）。
        stream.write_padding_bits(self.length, self.pattern_bit);
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // 返回 bit 数（在 bit 域内由 StructNode 累加）。
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

    /// 初始化 Python 解释器（幂等）。
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
    fn new_pattern_0x00_succeeds() {
        let node = BitPaddingNode::new(4, 0x00).expect("0x00 is valid");
        assert_eq!(node.length(), 4);
        assert_eq!(node.pattern_bit(), 0);
    }

    #[test]
    fn new_pattern_0x01_succeeds() {
        let node = BitPaddingNode::new(4, 0x01).expect("0x01 is valid");
        assert_eq!(node.length(), 4);
        assert_eq!(node.pattern_bit(), 1);
    }

    #[test]
    fn new_pattern_0x80_returns_padding_error() {
        let err = BitPaddingNode::new(4, 0x80).expect_err("0x80 invalid");
        match err {
            ConstructError::Padding { message, path } => {
                assert!(message.contains("0x80"), "got: {}", message);
                assert!(message.contains("0x00") && message.contains("0x01"));
                assert!(path.is_empty());
            }
            other => panic!("expected Padding, got {:?}", other),
        }
    }

    #[test]
    fn new_pattern_0xff_returns_padding_error() {
        let err = BitPaddingNode::new(4, 0xFF).expect_err("0xFF invalid");
        assert!(matches!(err, ConstructError::Padding { .. }));
    }

    #[test]
    fn new_pattern_0x02_returns_padding_error() {
        // 0x02 也不是合法 bit 域 pattern（仅 0x00 和 0x01）
        let err = BitPaddingNode::new(4, 0x02).expect_err("0x02 invalid");
        assert!(matches!(err, ConstructError::Padding { .. }));
    }

    #[test]
    fn new_zero_length_succeeds() {
        let node = BitPaddingNode::new(0, 0x00).expect("zero length valid");
        assert_eq!(node.length(), 0);
    }

    // ======================================================================
    // parse
    // ======================================================================

    #[test]
    fn parse_aligned_byte_skips_8_bits() {
        with_py(|py| {
            let node = BitPaddingNode::new(8, 0x00).expect("valid");
            let mut stream = ParseStream::new(&[0xAB, 0xCD]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // 返回 None
            assert!(result.is(&py.None()));
            // 跳过 8 bit = 1 字节
            assert_eq!(stream.tell(), 1);
            assert_eq!(stream.bit_pos(), 0);
        });
    }

    #[test]
    fn parse_unaligned_skips_n_bits() {
        with_py(|py| {
            let node = BitPaddingNode::new(4, 0x00).expect("valid");
            let mut stream = ParseStream::new(&[0xFF]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let _ = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert_eq!(stream.bit_pos(), 4);
            assert_eq!(stream.tell(), 0);
        });
    }

    #[test]
    fn parse_zero_length_is_noop() {
        // length=0 不推进游标
        with_py(|py| {
            let node = BitPaddingNode::new(0, 0x00).expect("zero length");
            let mut stream = ParseStream::new(&[0xFF]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let _ = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert_eq!(stream.tell(), 0);
            assert_eq!(stream.bit_pos(), 0);
        });
    }

    #[test]
    fn parse_insufficient_bits_returns_stream_error() {
        // 流中只有 8 bit，请求 12 bit
        with_py(|py| {
            let node = BitPaddingNode::new(12, 0x00).expect("valid");
            let mut stream = ParseStream::new(&[0xFF]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Stream { message, .. } => {
                    assert!(message.contains("expected 12 bits"), "got: {}", message);
                    assert!(message.contains("found 8"), "got: {}", message);
                }
                other => panic!("expected Stream, got {:?}", other),
            }
            // 失败时不推进游标
            assert_eq!(stream.tell(), 0);
            assert_eq!(stream.bit_pos(), 0);
        });
    }

    // ======================================================================
    // build
    // ======================================================================

    #[test]
    fn build_pattern_zero_writes_zero_bits() {
        with_py(|py| {
            let node = BitPaddingNode::new(8, 0x00).expect("valid");
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x00]);
        });
    }

    #[test]
    fn build_pattern_one_writes_one_bits() {
        with_py(|py| {
            let node = BitPaddingNode::new(8, 0x01).expect("valid");
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0xFF]);
        });
    }

    #[test]
    fn build_ignores_obj_value() {
        // obj 为任意值（非 None）也被忽略
        with_py(|py| {
            let node = BitPaddingNode::new(4, 0x01).expect("valid");
            // 传入字符串对象（不是 None）
            let obj = py.eval_bound("'hello'", None, None).expect("string");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            // 4 个 1 bit → current_byte 高 4 位为 1 = 0xF0
            assert_eq!(stream.bit_pos(), 4);
            assert_eq!(stream.as_bytes(), &[]);
        });
    }

    #[test]
    fn build_zero_length_is_noop() {
        with_py(|py| {
            let node = BitPaddingNode::new(0, 0x01).expect("valid");
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.tell(), 0);
            assert_eq!(stream.bit_pos(), 0);
        });
    }

    #[test]
    fn build_unaligned_extends_correctly() {
        // 先写 4 个 0 bit，再用 4 个 1 bit 填充 → 字节 0x0F
        with_py(|py| {
            let mut stream = BuildStream::new();
            stream.write_bits(0, 4);
            let node = BitPaddingNode::new(4, 0x01).expect("valid");
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.bit_pos(), 0);
            assert_eq!(stream.as_bytes(), &[0x0F]);
        });
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_returns_length_in_bits() {
        with_py(|py| {
            let ctx = Context::placeholder(py);
            let node = BitPaddingNode::new(4, 0x00).expect("valid");
            assert_eq!(node.sizeof(&ctx).unwrap(), 4);
            let node = BitPaddingNode::new(16, 0x01).expect("valid");
            assert_eq!(node.sizeof(&ctx).unwrap(), 16);
            let node = BitPaddingNode::new(0, 0x00).expect("valid");
            assert_eq!(node.sizeof(&ctx).unwrap(), 0);
        });
    }

    // ======================================================================
    // parse ↔ build 往返
    // ======================================================================

    #[test]
    fn round_trip_pattern_zero_byte() {
        with_py(|py| {
            let node = BitPaddingNode::new(8, 0x00).expect("valid");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            // build
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();
            assert_eq!(bytes, vec![0x00]);

            // parse back
            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            assert!(result.is(&py.None()));
            assert_eq!(pstream.tell(), 1);
            assert_eq!(pstream.bit_pos(), 0);
        });
    }
}
