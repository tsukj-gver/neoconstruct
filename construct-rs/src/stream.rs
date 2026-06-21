//! 字节流游标抽象：parse 用 `ParseStream`，build 用 `BuildStream`。
//!
//! 设计依据：`docs/架构设计.md` §C.4。
//!
//! ## 关键约束
//!
//! - Stream 是纯 Rust 内部抽象，**不跨 FFI**。所有方法均为 Rust 原生操作，无 C API 调用。
//! - `ParseStream` 借用 `&[u8]`，零拷贝，读取返回切片引用（无需分配）。
//! - `BuildStream` 持有 `Vec<u8>`，可通过 `with_capacity` 预分配容量减少 reallocate。
//! - read 失败时的错误信息与 Python construct `stream_read` 对齐（含 expected/found）。

use crate::error::ConstructError;
use crate::path::Path;

/// 解析流：包装输入字节切片，维护读取游标。
///
/// 生命周期 `'a` 绑定到底层字节缓冲（通常来自 `PyBytes`）。
#[derive(Debug)]
pub struct ParseStream<'a> {
    /// 底层字节缓冲（只读借用）。
    data: &'a [u8],
    /// 当前读取位置（从 0 开始）。
    pos: usize,
}

impl<'a> ParseStream<'a> {
    /// 创建解析流，初始位置为 0。
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// 读取 `n` 字节。
    ///
    /// 不足时返回 `ConstructError::Stream`，错误信息对齐 Python construct
    /// `stream_read`：
    /// ```text
    /// "stream read less than specified amount, expected {n}, found {remaining}"
    /// ```
    ///
    /// `n` 为 0 时返回空切片（不前进游标也不报错）。
    pub fn read(&mut self, n: usize, path: &Path) -> Result<&'a [u8], ConstructError> {
        if n == 0 {
            return Ok(&self.data[self.pos..self.pos]);
        }
        let remaining = self.data.len().saturating_sub(self.pos);
        if remaining < n {
            return Err(ConstructError::Stream {
                message: format!(
                    "stream read less than specified amount, expected {}, found {}",
                    n, remaining
                ),
                path: path.to_string(),
            });
        }
        let result = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(result)
    }

    /// 读取流中剩余所有字节（`GreedyBytes` 用）。
    ///
    /// 将游标推进到末尾，之后 `is_at_end()` 返回 `true`。
    pub fn read_remaining(&mut self) -> &'a [u8] {
        let result = &self.data[self.pos..];
        self.pos = self.data.len();
        result
    }

    /// 当前读取位置（字节偏移）。
    pub fn tell(&self) -> usize {
        self.pos
    }

    /// 是否已读到流末尾。
    pub fn is_at_end(&self) -> bool {
        self.pos >= self.data.len()
    }

    /// 底层字节切片（含已读与未读部分）。
    ///
    /// 主要用于测试与调试。生产代码不应直接访问此方法。
    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    /// 剩余未读字节数。
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }
}

/// 构建流：输出缓冲区。
///
/// 包装 `Vec<u8>`，提供追加写入与最终消费为 `Vec<u8>` 的能力。
#[derive(Debug, Default)]
pub struct BuildStream {
    /// 输出缓冲。
    buf: Vec<u8>,
}

impl BuildStream {
    /// 创建空的构建流。
    pub fn new() -> Self {
        Self::default()
    }

    /// 创建具有指定初始容量的构建流，避免后续 reallocate。
    ///
    /// 当构建的总字节数可预估时（如 `StructNode._sizeof` 能算出）应使用此构造函数。
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buf: Vec::with_capacity(capacity),
        }
    }

    /// 追加写入字节切片。
    ///
    /// 对齐 Python construct `stream.write`：直接追加，不校验长度
    /// （长度校验由具体节点如 `BytesNode.build` 负责）。
    pub fn write(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// 当前已写入字节数。
    pub fn tell(&self) -> usize {
        self.buf.len()
    }

    /// 消费此流，返回内部的字节缓冲。
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    /// 借用已写入字节切片（用于测试与中间检查）。
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root_path() -> Path {
        Path::new()
    }

    // ---------------------------------------------------------------------------
    // ParseStream
    // ---------------------------------------------------------------------------

    #[test]
    fn parse_stream_new_initializes_pos_zero() {
        let data = b"hello";
        let s = ParseStream::new(data);
        assert_eq!(s.tell(), 0);
        assert_eq!(s.data(), data);
        assert_eq!(s.remaining(), 5);
        assert!(!s.is_at_end());
    }

    #[test]
    fn parse_stream_read_returns_slice_and_advances() {
        let data = b"hello";
        let mut s = ParseStream::new(data);
        let path = root_path();
        let chunk = s.read(3, &path).expect("read 3 bytes");
        assert_eq!(chunk, b"hel");
        assert_eq!(s.tell(), 3);
        assert_eq!(s.remaining(), 2);
    }

    #[test]
    fn parse_stream_read_zero_bytes_returns_empty() {
        let data = b"hello";
        let mut s = ParseStream::new(data);
        let path = root_path();
        let chunk = s.read(0, &path).expect("read 0 bytes should succeed");
        assert!(chunk.is_empty());
        assert_eq!(s.tell(), 0);
    }

    #[test]
    fn parse_stream_read_exact_remaining() {
        let data = b"abc";
        let mut s = ParseStream::new(data);
        let path = root_path();
        let chunk = s.read(3, &path).expect("read all");
        assert_eq!(chunk, b"abc");
        assert_eq!(s.tell(), 3);
        assert!(s.is_at_end());
    }

    #[test]
    fn parse_stream_read_insufficient_returns_stream_error() {
        let data = b"ab";
        let mut s = ParseStream::new(data);
        let path = root_path();
        let err = s.read(5, &path).expect_err("should fail");
        match err {
            ConstructError::Stream { message, path } => {
                assert!(message.contains("expected 5"), "got: {}", message);
                assert!(message.contains("found 2"), "got: {}", message);
                assert_eq!(path, "root");
            }
            other => panic!("expected Stream error, got {:?}", other),
        }
        // 失败时不前进游标。
        assert_eq!(s.tell(), 0);
    }

    #[test]
    fn parse_stream_read_after_partial_read_fails_correctly() {
        let data = b"abcde";
        let mut s = ParseStream::new(data);
        let path = root_path();
        let _ = s.read(2, &path).expect("first 2");
        assert_eq!(s.tell(), 2);
        // 试图读 4 字节，但只剩 3 字节。
        let err = s.read(4, &path).expect_err("should fail");
        match err {
            ConstructError::Stream { message, .. } => {
                assert!(message.contains("expected 4"), "got: {}", message);
                assert!(message.contains("found 3"), "got: {}", message);
            }
            _ => panic!("expected Stream error"),
        }
        assert_eq!(s.tell(), 2);
    }

    #[test]
    fn parse_stream_read_remaining_consumes_all() {
        let data = b"hello world";
        let mut s = ParseStream::new(data);
        let path = root_path();
        let _ = s.read(6, &path).expect("first 6"); // 读 "hello "
        let rest = s.read_remaining();
        assert_eq!(rest, b"world");
        assert!(s.is_at_end());
        assert_eq!(s.tell(), 11);
        assert_eq!(s.remaining(), 0);
    }

    #[test]
    fn parse_stream_read_remaining_empty_when_at_end() {
        let data = b"hi";
        let mut s = ParseStream::new(data);
        let path = root_path();
        let _ = s.read(2, &path).expect("all");
        let rest = s.read_remaining();
        assert!(rest.is_empty());
    }

    #[test]
    fn parse_stream_read_remaining_at_start_returns_all() {
        let data = b"hello";
        let mut s = ParseStream::new(data);
        let rest = s.read_remaining();
        assert_eq!(rest, data);
        assert!(s.is_at_end());
    }

    #[test]
    fn parse_stream_is_at_end_after_full_read() {
        let data = b"abc";
        let mut s = ParseStream::new(data);
        let path = root_path();
        assert!(!s.is_at_end());
        let _ = s.read(3, &path).expect("all");
        assert!(s.is_at_end());
    }

    #[test]
    fn parse_stream_empty_input_is_immediately_at_end() {
        let data = b"";
        let s = ParseStream::new(data);
        assert!(s.is_at_end());
        assert_eq!(s.remaining(), 0);
    }

    #[test]
    fn parse_stream_remaining_decreases_with_reads() {
        let data = b"abcdef";
        let mut s = ParseStream::new(data);
        let path = root_path();
        assert_eq!(s.remaining(), 6);
        let _ = s.read(2, &path).expect("first");
        assert_eq!(s.remaining(), 4);
        let _ = s.read(2, &path).expect("second");
        assert_eq!(s.remaining(), 2);
    }

    // ---------------------------------------------------------------------------
    // BuildStream
    // ---------------------------------------------------------------------------

    #[test]
    fn build_stream_new_starts_empty() {
        let s = BuildStream::new();
        assert_eq!(s.tell(), 0);
        assert!(s.as_bytes().is_empty());
    }

    #[test]
    fn build_stream_write_appends_bytes() {
        let mut s = BuildStream::new();
        s.write(b"hello");
        assert_eq!(s.as_bytes(), b"hello");
        assert_eq!(s.tell(), 5);
    }

    #[test]
    fn build_stream_multiple_writes_concatenate() {
        let mut s = BuildStream::new();
        s.write(b"foo");
        s.write(b"bar");
        s.write(b"baz");
        assert_eq!(s.as_bytes(), b"foobarbaz");
        assert_eq!(s.tell(), 9);
    }

    #[test]
    fn build_stream_write_empty_is_noop() {
        let mut s = BuildStream::new();
        s.write(b"");
        assert_eq!(s.tell(), 0);
        assert!(s.as_bytes().is_empty());
    }

    #[test]
    fn build_stream_into_bytes_consumes() {
        let mut s = BuildStream::new();
        s.write(b"data");
        let bytes = s.into_bytes();
        assert_eq!(bytes, b"data");
    }

    #[test]
    fn build_stream_with_capacity_preserves_content() {
        let mut s = BuildStream::with_capacity(64);
        s.write(b"hello");
        assert_eq!(s.as_bytes(), b"hello");
        // 容量预分配，但仍能动态扩展。
        assert!(s.tell() <= 64 || s.as_bytes().len() == 5);
    }

    #[test]
    fn build_stream_with_capacity_zero() {
        let mut s = BuildStream::with_capacity(0);
        s.write(b"abc");
        assert_eq!(s.as_bytes(), b"abc");
    }

    #[test]
    fn build_stream_default_equals_new() {
        let s1 = BuildStream::default();
        let s2 = BuildStream::new();
        assert_eq!(s1.tell(), s2.tell());
        assert_eq!(s1.as_bytes(), s2.as_bytes());
    }

    #[test]
    fn build_stream_write_large_data() {
        let mut s = BuildStream::new();
        let data = vec![0xABu8; 1024];
        s.write(&data);
        assert_eq!(s.tell(), 1024);
        assert_eq!(s.as_bytes(), data.as_slice());
    }

    // ---------------------------------------------------------------------------
    // ParseStream + BuildStream 互操作（语义验证）
    // ---------------------------------------------------------------------------

    #[test]
    fn round_trip_write_then_read_preserves_bytes() {
        let original = b"\x01\x02\x03\x04\x05";
        // 写入
        let mut writer = BuildStream::new();
        writer.write(original);
        let bytes = writer.into_bytes();
        // 读取
        let mut reader = ParseStream::new(&bytes);
        let path = root_path();
        let chunk = reader.read(original.len(), &path).expect("read back");
        assert_eq!(chunk, original);
        assert!(reader.is_at_end());
    }
}
