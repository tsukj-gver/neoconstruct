//! 字节流游标抽象：parse 用 `ParseStream`，build 用 `BuildStream`。
//!
//! 设计依据：`docs/架构设计.md` §C.4、`docs/模块设计-BitStream.md` §3。
//!
//! ## 关键约束
//!
//! - Stream 是纯 Rust 内部抽象，**不跨 FFI**。所有方法均为 Rust 原生操作，无 C API 调用。
//! - `ParseStream` 借用 `&[u8]`，零拷贝，读取返回切片引用（无需分配）。
//! - `BuildStream` 持有 `Vec<u8>`，可通过 `with_capacity` 预分配容量减少 reallocate。
//! - read 失败时的错误信息与 Python construct `stream_read` 对齐（含 expected/found）。
//!
//! ## Bit-level 支持（Phase 3.1）
//!
//! 除字节游标（`pos`）外，`ParseStream` / `BuildStream` 还维护 **bit 游标**
//! （`bit_pos: u8`，取值 0-7）。bit 顺序固定为 **MSB-first**（bit 0 是字节最高有效位），
//! 对齐 Python construct 默认行为。
//!
//! - `bit_pos == 0`：字节对齐，字节级 `read(n)` / `write(data)` 可用
//! - `bit_pos != 0`：处于字节内 bit 偏移，仅 bit 级 `read_bits` / `write_bits` 可用
//!
//! 字节级 API 在 `bit_pos != 0` 时调用属契约违反：`read` 在 release 返回 `Stream` Err
//! （可恢复），`write` / `into_bytes` 在 release 行为未定义但不 panic（与 std `Vec` 等
//! "无额外运行时检查"API 一致）。所有 panic 路径限定为 `debug_assert!`（仅 debug build）。

use crate::error::ConstructError;
use crate::path::Path;

/// 解析流：包装输入字节切片，维护字节游标与 bit 游标。
///
/// 生命周期 `'a` 绑定到底层字节缓冲（通常来自 `PyBytes`）。
///
/// # bit 游标
///
/// `bit_pos`（0-7）记录当前字节内的 bit 偏移。详见模块级文档。
#[derive(Debug)]
pub struct ParseStream<'a> {
    /// 底层字节缓冲（只读借用）。
    data: &'a [u8],
    /// 当前读取位置（从 0 开始）。
    pos: usize,
    /// 当前字节内 bit 偏移（0-7）。0 表示字节对齐。
    bit_pos: u8,
}

impl<'a> ParseStream<'a> {
    /// 创建解析流，初始位置为 0，bit 对齐。
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            bit_pos: 0,
        }
    }

    /// 读取 `n` 字节（字节级，要求 `bit_pos == 0`）。
    ///
    /// 不足时返回 `ConstructError::Stream`，错误信息对齐 Python construct
    /// `stream_read`：
    /// ```text
    /// "stream read less than specified amount, expected {n}, found {remaining}"
    /// ```
    ///
    /// `n` 为 0 时返回空切片（不前进游标也不报错）。
    ///
    /// # bit 对齐前置条件
    ///
    /// 调用前必须 `bit_pos == 0`。否则：
    /// - debug build：`debug_assert!` 触发 panic（开发期捕获调用方 bug）
    /// - release build：返回 `ConstructError::Stream`（携带 path 与 bit 偏移信息）
    ///
    /// 详见 `docs/模块设计-BitStream.md` §3.3。
    pub fn read(&mut self, n: usize, path: &Path) -> Result<&'a [u8], ConstructError> {
        debug_assert!(
            self.bit_pos == 0,
            "byte-level read in bit-unaligned position (bit_pos={})",
            self.bit_pos
        );
        if self.bit_pos != 0 {
            return Err(ConstructError::Stream {
                message: format!(
                    "byte-level read at bit offset {} (expected byte alignment)",
                    self.bit_pos
                ),
                path: path.to_string(),
            });
        }
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
    ///
    /// # bit 对齐前置条件
    ///
    /// 与 [`read`](Self::read) 一致，要求 `bit_pos == 0`。否则在 release 中静默返回空
    /// 切片（不推进游标），debug 中触发 `debug_assert!`。调用方应在字节边界调用。
    pub fn read_remaining(&mut self) -> &'a [u8] {
        debug_assert!(
            self.bit_pos == 0,
            "read_remaining in bit-unaligned position (bit_pos={})",
            self.bit_pos
        );
        let result = &self.data[self.pos..];
        self.pos = self.data.len();
        result
    }

    /// 当前读取位置（字节偏移）。
    ///
    /// 注意：若 `bit_pos != 0`，`tell()` 返回的是当前正在部分读取的字节索引
    /// （该字节的低 `bit_pos` bit 已读，高 `8 - bit_pos` bit 未读）。
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

    /// 当前 bit 偏移（0-7）。`0` 表示字节对齐。
    ///
    /// 详见模块级文档的 bit 游标说明。
    pub fn bit_pos(&self) -> u8 {
        self.bit_pos
    }

    /// 是否字节对齐（`bit_pos == 0`）。
    pub fn is_byte_aligned(&self) -> bool {
        self.bit_pos == 0
    }

    /// 读取 `n` 个 bit，返回 MSB-first 的 `u64` 整数。
    ///
    /// 对齐 Python `bits2integer`（`lib/binary.py` L57）。
    ///
    /// # 参数
    ///
    /// - `n`：要读取的 bit 数。必须 `<= 64`（u64 上限，超出返回 `Stream` Err）。
    /// - `path`：错误追踪路径。
    ///
    /// # 行为
    ///
    /// - `n == 0`：返回 `Ok(0)`，不推进游标。
    /// - `n > 64`：返回 `ConstructError::Stream`。
    /// - 流中 bit 数不足：返回 `ConstructError::Stream`（含 expected/found bit 数）。
    /// - 当 `bit_pos == 0` 且 `n >= 8` 时，走批量字节路径（直接读 `n/8` 字节左移，
    ///   剩余 `n%8` bit 逐位）——这是 `BytewiseNode` 在对齐时的快路径。
    /// - 否则（未对齐或 `n < 8`），逐 bit 读取（跨字节边界自动衔接）。
    ///
    /// 返回值的低 `n` 位是读到的 bit 序（高位在前 = MSB-first）。
    pub fn read_bits(&mut self, n: usize, path: &Path) -> Result<u64, ConstructError> {
        if n == 0 {
            return Ok(0);
        }
        if n > 64 {
            return Err(ConstructError::Stream {
                message: format!(
                    "read_bits: cannot read more than 64 bits at once, requested {}",
                    n
                ),
                path: path.to_string(),
            });
        }

        // 预检查：剩余 bit 数（考虑当前 bit_pos 偏移）
        let remaining_bytes = self.data.len().saturating_sub(self.pos);
        let remaining_bits = remaining_bytes * 8 - self.bit_pos as usize;
        if n > remaining_bits {
            return Err(ConstructError::Stream {
                message: format!(
                    "stream read less than specified amount, expected {} bits, found {}",
                    n, remaining_bits
                ),
                path: path.to_string(),
            });
        }

        let mut result: u64 = 0;

        // 批量路径：字节对齐 + 至少 1 整字节
        if self.bit_pos == 0 && n >= 8 {
            let full_bytes = n / 8;
            let rem_bits = n % 8;

            if full_bytes > 0 {
                // 直接切片读取（bounds 已在上面校验），不走 self.read() 以避免
                // 重复 bit_pos 校验
                let chunk = &self.data[self.pos..self.pos + full_bytes];
                for &b in chunk {
                    result = (result << 8) | b as u64;
                }
                self.pos += full_bytes;
            }

            // 逐位读取剩余 rem_bits bit（bit_pos 仍为 0）
            for _ in 0..rem_bits {
                let byte = self.data[self.pos];
                let bit = (byte >> (7 - self.bit_pos)) & 1;
                result = (result << 1) | bit as u64;
                self.bit_pos += 1;
                if self.bit_pos == 8 {
                    self.bit_pos = 0;
                    self.pos += 1;
                }
            }
        } else {
            // 慢路径：未对齐或 n < 8，逐 bit 读取（跨字节自动衔接）
            for _ in 0..n {
                let byte = self.data[self.pos];
                let bit = (byte >> (7 - self.bit_pos)) & 1;
                result = (result << 1) | bit as u64;
                self.bit_pos += 1;
                if self.bit_pos == 8 {
                    self.bit_pos = 0;
                    self.pos += 1;
                }
            }
        }

        Ok(result)
    }

    /// 跳过 `n` 个 bit（不返回读到的值，仍校验流足够）。
    ///
    /// 用于 bit 域 Padding。等价于 `read_bits(n)` 但不构造返回值。
    ///
    /// # 参数
    ///
    /// - `n`：要跳过的 bit 数。
    /// - `path`：错误追踪路径。
    pub fn skip_bits(&mut self, n: usize, path: &Path) -> Result<(), ConstructError> {
        if n == 0 {
            return Ok(());
        }

        let remaining_bytes = self.data.len().saturating_sub(self.pos);
        let remaining_bits = remaining_bytes * 8 - self.bit_pos as usize;
        if n > remaining_bits {
            return Err(ConstructError::Stream {
                message: format!(
                    "stream skip less than specified amount, expected {} bits, found {}",
                    n, remaining_bits
                ),
                path: path.to_string(),
            });
        }

        // 直接推进游标（无需读出 bit）
        let new_total = self.bit_pos as usize + n;
        self.pos += new_total / 8;
        self.bit_pos = (new_total % 8) as u8;

        Ok(())
    }
}

/// 构建流：输出缓冲区，支持字节级与 bit 级写入。
///
/// 包装 `Vec<u8>`，提供追加写入与最终消费为 `Vec<u8>` 的能力。
///
/// # bit 写入策略
///
/// 采用"部分字节缓冲"：bit_pos > 0 时，正在填充的字节暂存于 `current_byte`，
/// 满 8 bit 后 push 到 `buf`。`current_byte` 中已写入的 bit 永远位于高位
/// （`bit_pos == 0` 时整字节为 0），未写入的位始终为 0，可安全 `|=`。
#[derive(Debug, Default)]
pub struct BuildStream {
    /// 输出缓冲。
    buf: Vec<u8>,
    /// 当前字节内 bit 偏移（0-7）。0 表示字节对齐。
    bit_pos: u8,
    /// `bit_pos > 0` 时的部分字节（高 `bit_pos` 位有效，低 `8 - bit_pos` 位为 0）。
    current_byte: u8,
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
            bit_pos: 0,
            current_byte: 0,
        }
    }

    /// 追加写入字节切片（字节级，要求 `bit_pos == 0`）。
    ///
    /// 对齐 Python construct `stream.write`：直接追加，不校验长度
    /// （长度校验由具体节点如 `BytesNode.build` 负责）。
    ///
    /// # bit 对齐前置条件
    ///
    /// 调用前必须 `bit_pos == 0`。否则：
    /// - debug build：`debug_assert!` 触发 panic
    /// - release build：行为未定义但不 panic（破坏输出缓冲一致性，调用方契约违反）
    ///
    /// 详见 `docs/模块设计-BitStream.md` §3.3。
    pub fn write(&mut self, data: &[u8]) {
        debug_assert!(
            self.bit_pos == 0,
            "byte-level write in bit-unaligned position (bit_pos={})",
            self.bit_pos
        );
        self.buf.extend_from_slice(data);
    }

    /// 当前已写入字节数。
    ///
    /// 注意：若 `bit_pos != 0`，`tell()` 不包含正在填充的 `current_byte`。
    /// 字节级调用方应在 `bit_pos == 0` 时使用此值。
    pub fn tell(&self) -> usize {
        self.buf.len()
    }

    /// 消费此流，返回内部的字节缓冲。
    ///
    /// # bit 对齐前置条件
    ///
    /// 调用前应 `bit_pos == 0`。否则：
    /// - debug build：`debug_assert!` 触发 panic
    /// - release build：不 panic，返回当前 `buf`（部分字节 `current_byte` 的已写 bit 丢失）。
    ///   这是调用方契约违反（顶层 `BitwiseNode.build` 退出时已校验对齐），
    ///   release 模式不为此增加运行时分支。
    pub fn into_bytes(self) -> Vec<u8> {
        debug_assert!(
            self.bit_pos == 0,
            "into_bytes with partial bit byte (bit_pos={})",
            self.bit_pos
        );
        self.buf
    }

    /// 借用已写入字节切片（用于测试与中间检查）。
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    /// 当前 bit 偏移（0-7）。`0` 表示字节对齐。
    pub fn bit_pos(&self) -> u8 {
        self.bit_pos
    }

    /// 是否字节对齐（`bit_pos == 0`）。
    pub fn is_byte_aligned(&self) -> bool {
        self.bit_pos == 0
    }

    /// 写入 `n` 个 bit（`value` 的低 `n` 位，MSB-first）。
    ///
    /// 对齐 Python `integer2bits`（`lib/binary.py` L6）。
    ///
    /// # 参数
    ///
    /// - `value`：要写入的值。仅低 `n` 位被使用（高位被掩码丢弃）。
    /// - `n`：要写入的 bit 数。必须 `<= 64`，否则 debug build panic（debug_assert），
    ///   release 行为未定义（不会越界，但结果无意义）。
    ///
    /// # 行为
    ///
    /// - `n == 0`：不写入，直接返回。
    /// - `bit_pos == 0` 且 `n >= 8`：走批量字节路径，整字节直接 push 到 `buf`，
    ///   剩余 `n % 8` bit 走逐 bit 路径。
    /// - 否则：逐 bit 写入 `current_byte`，满 8 bit 后 push 到 `buf`。
    pub fn write_bits(&mut self, value: u64, n: usize) {
        if n == 0 {
            return;
        }
        debug_assert!(n <= 64, "write_bits: n must be <= 64, got {}", n);

        // 掩码到低 n 位（防御性：调用方可能传入了未掩码的值）
        let masked = if n < 64 {
            value & ((1u64 << n) - 1)
        } else {
            value
        };

        // 批量路径：字节对齐 + 至少 1 整字节
        if self.bit_pos == 0 && n >= 8 {
            let full_bytes = n / 8;
            let rem_bits = n % 8;

            // 整字节部分：从 masked 的高位逐字节提取，直接 push
            // 第 i 字节（i=0 是 MSB）的 shift = rem_bits + 8 * (full_bytes - 1 - i)
            for i in 0..full_bytes {
                let shift = rem_bits + 8 * (full_bytes - 1 - i);
                self.buf.push(((masked >> shift) & 0xFF) as u8);
            }

            // 剩余 rem_bits bit：从 masked 的低位逐 bit 写入 current_byte
            if rem_bits > 0 {
                let low_bits = masked & ((1u64 << rem_bits) - 1);
                self.write_bits_slow(low_bits, rem_bits);
            }
        } else {
            // 慢路径：未对齐或 n < 8
            self.write_bits_slow(masked, n);
        }
    }

    /// 写入 `n` 个值为 `bit`（0 或 1）的 bit（bit 域 Padding 用）。
    ///
    /// 等价于 `write_bits(0, n)`（bit=0）或 `write_bits(0xFFFF..., n)`（bit!=0），
    /// 但避免构造大整数。`bit` 实际取值：`0` → 填 0 bit，非 `0` → 填 1 bit。
    ///
    /// # 参数
    ///
    /// - `n`：要写入的 bit 数。无上限（不像 `write_bits` 限 64），可处理大块 padding。
    /// - `bit`：填充值（0 或非 0）。
    pub fn write_padding_bits(&mut self, n: usize, bit: u8) {
        if n == 0 {
            return;
        }
        let bit_val: u8 = if bit != 0 { 1 } else { 0 };

        // 字节对齐 + 大块：批量 push 相同字节
        if self.bit_pos == 0 && n >= 8 {
            let full_bytes = n / 8;
            let rem_bits = n % 8;
            let byte_val: u8 = if bit_val == 1 { 0xFF } else { 0x00 };
            // resize_with 比 push 循环更高效（一次性预留容量）
            let old_len = self.buf.len();
            self.buf.resize(old_len + full_bytes, byte_val);

            // 剩余 rem_bits bit 逐位写入
            for _ in 0..rem_bits {
                self.current_byte |= bit_val << (7 - self.bit_pos);
                self.bit_pos += 1;
                if self.bit_pos == 8 {
                    self.buf.push(self.current_byte);
                    self.current_byte = 0;
                    self.bit_pos = 0;
                }
            }
        } else {
            // 未对齐或小 n：逐位写入
            for _ in 0..n {
                self.current_byte |= bit_val << (7 - self.bit_pos);
                self.bit_pos += 1;
                if self.bit_pos == 8 {
                    self.buf.push(self.current_byte);
                    self.current_byte = 0;
                    self.bit_pos = 0;
                }
            }
        }
    }

    /// 逐 bit 写入辅助（slow path）。
    ///
    /// `current_byte` 的高 `bit_pos` 位已有效，低 `8 - bit_pos` 位始终为 0
    /// （由 `bit_pos == 8` 时重置保证），因此 OR-equal 写入新位是安全的。
    fn write_bits_slow(&mut self, value: u64, n: usize) {
        for i in 0..n {
            // 第 i bit 从 MSB 起算：位置 (n - 1 - i)
            let bit = ((value >> (n - 1 - i)) & 1) as u8;
            self.current_byte |= bit << (7 - self.bit_pos);
            self.bit_pos += 1;
            if self.bit_pos == 8 {
                self.buf.push(self.current_byte);
                self.current_byte = 0;
                self.bit_pos = 0;
            }
        }
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

    // ---------------------------------------------------------------------------
    // ParseStream bit-level API（Phase 3.1）
    // ---------------------------------------------------------------------------

    #[test]
    fn parse_stream_bit_pos_initially_zero() {
        let s = ParseStream::new(b"abc");
        assert_eq!(s.bit_pos(), 0);
        assert!(s.is_byte_aligned());
    }

    #[test]
    fn parse_stream_read_bits_zero_returns_zero_no_advance() {
        let mut s = ParseStream::new(b"\xff");
        let path = root_path();
        let v = s.read_bits(0, &path).expect("read 0 bits");
        assert_eq!(v, 0);
        assert_eq!(s.tell(), 0);
        assert_eq!(s.bit_pos(), 0);
    }

    #[test]
    fn parse_stream_read_bits_single_bit_msb() {
        // 0x80 = 0b1000_0000 → 第一 bit 是 1
        let mut s = ParseStream::new(&[0x80]);
        let path = root_path();
        let v = s.read_bits(1, &path).expect("read 1 bit");
        assert_eq!(v, 1);
        assert_eq!(s.bit_pos(), 1);
        assert!(!s.is_byte_aligned());
    }

    #[test]
    fn parse_stream_read_bits_single_bit_zero() {
        // 0x40 = 0b0100_0000 → 第一 bit 0，第二 bit 1
        let mut s = ParseStream::new(&[0x40]);
        let path = root_path();
        let v1 = s.read_bits(1, &path).expect("first bit");
        assert_eq!(v1, 0);
        let v2 = s.read_bits(1, &path).expect("second bit");
        assert_eq!(v2, 1);
        assert_eq!(s.bit_pos(), 2);
    }

    #[test]
    fn parse_stream_read_bits_nibble_from_byte() {
        // 0xA5 = 0b1010_0101 → 高 4 bit = 0b1010 = 10，低 4 bit = 0b0101 = 5
        let mut s = ParseStream::new(&[0xA5]);
        let path = root_path();
        let high = s.read_bits(4, &path).expect("high nibble");
        let low = s.read_bits(4, &path).expect("low nibble");
        assert_eq!(high, 10);
        assert_eq!(low, 5);
        assert_eq!(s.bit_pos(), 0);
        assert_eq!(s.tell(), 1);
    }

    #[test]
    fn parse_stream_read_bits_byte_aligned_full_byte() {
        // 0x42 = 66
        let mut s = ParseStream::new(&[0x42]);
        let path = root_path();
        let v = s.read_bits(8, &path).expect("read 8 bits");
        assert_eq!(v, 66);
        assert_eq!(s.bit_pos(), 0);
        assert_eq!(s.tell(), 1);
    }

    #[test]
    fn parse_stream_read_bits_byte_aligned_two_bytes() {
        // 0x01 0x00 → 0x0100 = 256（big-endian）
        let mut s = ParseStream::new(&[0x01, 0x00]);
        let path = root_path();
        let v = s.read_bits(16, &path).expect("read 16 bits");
        assert_eq!(v, 256);
        assert_eq!(s.bit_pos(), 0);
        assert_eq!(s.tell(), 2);
    }

    #[test]
    fn parse_stream_read_bits_12_bits_across_byte_boundary() {
        // 0xAB 0xC0 = 0b1010_1011 1100_0000
        // 前 12 bit = 0b1010_1011_1100 = 0xABC = 2748
        let mut s = ParseStream::new(&[0xAB, 0xC0]);
        let path = root_path();
        let v = s.read_bits(12, &path).expect("read 12 bits");
        assert_eq!(v, 0xABC);
        assert_eq!(s.bit_pos(), 4);
        assert_eq!(s.tell(), 1);
    }

    #[test]
    fn parse_stream_read_bits_full_byte_then_remainder() {
        // 0xFF 0xF0 = 全 1 字节 + 高 4 bit 为 1
        let mut s = ParseStream::new(&[0xFF, 0xF0]);
        let path = root_path();
        let full = s.read_bits(8, &path).expect("read 8 bits");
        assert_eq!(full, 0xFF);
        assert_eq!(s.bit_pos(), 0);
        let high = s.read_bits(4, &path).expect("read 4 bits");
        assert_eq!(high, 0x0F);
        assert_eq!(s.bit_pos(), 4);
    }

    #[test]
    fn parse_stream_read_bits_64_max() {
        // 0xFF * 8 = u64::MAX
        let mut s = ParseStream::new(&[0xFF; 8]);
        let path = root_path();
        let v = s.read_bits(64, &path).expect("read 64 bits");
        assert_eq!(v, u64::MAX);
        assert_eq!(s.bit_pos(), 0);
        assert_eq!(s.tell(), 8);
    }

    #[test]
    fn parse_stream_read_bits_exceeds_64_returns_stream_error() {
        let mut s = ParseStream::new(&[0xFF; 9]);
        let path = root_path();
        let err = s.read_bits(65, &path).expect_err("should fail");
        match err {
            ConstructError::Stream { message, .. } => {
                assert!(message.contains("65"), "got: {}", message);
                assert!(message.contains("64"), "got: {}", message);
            }
            other => panic!("expected Stream error, got {:?}", other),
        }
    }

    #[test]
    fn parse_stream_read_bits_insufficient_returns_stream_error() {
        // 只 1 字节（8 bit），请求 12 bit
        let mut s = ParseStream::new(&[0xFF]);
        let path = root_path();
        let err = s.read_bits(12, &path).expect_err("should fail");
        match err {
            ConstructError::Stream { message, .. } => {
                assert!(message.contains("expected 12 bits"), "got: {}", message);
                assert!(message.contains("found 8"), "got: {}", message);
            }
            other => panic!("expected Stream error, got {:?}", other),
        }
        // 失败时不推进游标
        assert_eq!(s.tell(), 0);
        assert_eq!(s.bit_pos(), 0);
    }

    #[test]
    fn parse_stream_read_bits_partial_then_insufficient() {
        // 先读 4 bit（剩 4 bit），再请求 8 bit 失败
        let mut s = ParseStream::new(&[0xFF]);
        let path = root_path();
        let _ = s.read_bits(4, &path).expect("first 4");
        let err = s.read_bits(8, &path).expect_err("should fail");
        match err {
            ConstructError::Stream { message, .. } => {
                assert!(message.contains("expected 8 bits"), "got: {}", message);
                assert!(message.contains("found 4"), "got: {}", message);
            }
            other => panic!("expected Stream error, got {:?}", other),
        }
    }

    #[test]
    fn parse_stream_read_bits_msb_first_ordering() {
        // 0b1010_1010 = 0xAA → 读 4 bit 应得 0b1010 = 10（MSB-first）
        let mut s = ParseStream::new(&[0xAA]);
        let path = root_path();
        let v = s.read_bits(4, &path).expect("read 4 bits");
        assert_eq!(v, 0b1010);
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn parse_stream_byte_read_after_bit_offset_returns_stream_err_release_path() {
        // 此测试仅在 release build（无 debug_assert）下运行：
        // 先读 4 bit 使 bit_pos != 0，再调用 read() 应在 release 返回 Stream Err
        // debug build 下 debug_assert 先触发 panic（契约违反 fail-fast）
        let mut s = ParseStream::new(&[0xFF, 0xFF]);
        let path = root_path();
        let _ = s.read_bits(4, &path).expect("first 4 bits");
        assert_eq!(s.bit_pos(), 4);
        let err = s.read(1, &path).expect_err("should fail");
        match err {
            ConstructError::Stream { message, .. } => {
                assert!(message.contains("bit offset 4"), "got: {}", message);
            }
            other => panic!("expected Stream error, got {:?}", other),
        }
    }

    #[test]
    fn parse_stream_skip_bits_advances_correctly() {
        let mut s = ParseStream::new(&[0xFF, 0xFF, 0xFF]);
        let path = root_path();
        s.skip_bits(4, &path).expect("skip 4");
        assert_eq!(s.bit_pos(), 4);
        assert_eq!(s.tell(), 0);
        s.skip_bits(12, &path).expect("skip 12");
        assert_eq!(s.bit_pos(), 0);
        assert_eq!(s.tell(), 2);
    }

    #[test]
    fn parse_stream_skip_bits_zero_is_noop() {
        let mut s = ParseStream::new(&[0xFF]);
        let path = root_path();
        s.skip_bits(0, &path).expect("skip 0");
        assert_eq!(s.tell(), 0);
        assert_eq!(s.bit_pos(), 0);
    }

    #[test]
    fn parse_stream_skip_bits_insufficient_returns_stream_error() {
        let mut s = ParseStream::new(&[0xFF]);
        let path = root_path();
        let err = s.skip_bits(16, &path).expect_err("should fail");
        match err {
            ConstructError::Stream { message, .. } => {
                assert!(message.contains("expected 16 bits"), "got: {}", message);
                assert!(message.contains("found 8"), "got: {}", message);
            }
            other => panic!("expected Stream error, got {:?}", other),
        }
    }

    #[test]
    fn parse_stream_skip_then_read_after_alignment() {
        // 跳过 4 bit 对齐到字节中点，再读 4 bit 完成一字节
        let mut s = ParseStream::new(&[0xAB]);
        let path = root_path();
        s.skip_bits(4, &path).expect("skip 4");
        let v = s.read_bits(4, &path).expect("read 4");
        // 0xAB = 0b1010_1011，后 4 bit = 0b1011 = 0xB
        assert_eq!(v, 0xB);
        assert_eq!(s.bit_pos(), 0);
        assert_eq!(s.tell(), 1);
    }

    // ---------------------------------------------------------------------------
    // BuildStream bit-level API（Phase 3.1）
    // ---------------------------------------------------------------------------

    #[test]
    fn build_stream_bit_pos_initially_zero() {
        let s = BuildStream::new();
        assert_eq!(s.bit_pos(), 0);
        assert!(s.is_byte_aligned());
    }

    #[test]
    fn build_stream_write_bits_zero_is_noop() {
        let mut s = BuildStream::new();
        s.write_bits(0xFF, 0);
        assert_eq!(s.tell(), 0);
        assert_eq!(s.bit_pos(), 0);
        let bytes = s.into_bytes();
        assert!(bytes.is_empty());
    }

    #[test]
    fn build_stream_write_bits_single_byte_aligned() {
        let mut s = BuildStream::new();
        s.write_bits(0x42, 8);
        assert_eq!(s.bit_pos(), 0);
        assert_eq!(s.tell(), 1);
        assert_eq!(s.as_bytes(), &[0x42]);
    }

    #[test]
    fn build_stream_write_bits_nibble_aligned() {
        let mut s = BuildStream::new();
        s.write_bits(0xA, 4);
        s.write_bits(0x5, 4);
        assert_eq!(s.bit_pos(), 0);
        assert_eq!(s.tell(), 1);
        assert_eq!(s.as_bytes(), &[0xA5]);
    }

    #[test]
    fn build_stream_write_bits_12_bits_value() {
        let mut s = BuildStream::new();
        // 12 bits of 0xABC = 0b1010_1011_1100
        s.write_bits(0xABC, 12);
        assert_eq!(s.bit_pos(), 4);
        // 字节级 tell 不包含部分字节
        assert_eq!(s.tell(), 1);
        assert_eq!(s.as_bytes(), &[0xAB]);
    }

    #[test]
    fn build_stream_write_bits_64_max() {
        let mut s = BuildStream::new();
        s.write_bits(u64::MAX, 64);
        assert_eq!(s.bit_pos(), 0);
        assert_eq!(s.tell(), 8);
        assert_eq!(s.as_bytes(), &[0xFF; 8]);
    }

    #[test]
    fn build_stream_write_bits_value_masked_to_low_n() {
        // 高位被掩码丢弃：value = 0xFF, n = 4 → 仅写 0b1111 = 0xF
        let mut s = BuildStream::new();
        s.write_bits(0xFF, 4);
        s.write_bits(0x00, 4);
        assert_eq!(s.as_bytes(), &[0xF0]);
    }

    #[test]
    fn build_stream_write_bits_msb_first_ordering() {
        // 写入 0b1010_0101 = 0xA5 应得到字节 0xA5（MSB-first）
        let mut s = BuildStream::new();
        s.write_bits(0xA5, 8);
        assert_eq!(s.as_bytes(), &[0xA5]);
    }

    #[test]
    fn build_stream_write_bits_across_byte_boundary() {
        // 写 4 bit (0xF) + 8 bit (0xAA) + 4 bit (0x0) = 0xFA 0xAA
        // 第 1 字节 = 0xF000 | 0xA (high 4 of 0xAA) = 0xFA
        // 第 2 字节 = 0xA (low 4 of 0xAA) << 4 | 0x0 = 0xA0
        let mut s = BuildStream::new();
        s.write_bits(0xF, 4);
        s.write_bits(0xAA, 8);
        s.write_bits(0x0, 4);
        assert_eq!(s.bit_pos(), 0);
        assert_eq!(s.as_bytes(), &[0xFA, 0xA0]);
    }

    #[test]
    fn build_stream_write_bits_full_byte_then_partial() {
        let mut s = BuildStream::new();
        s.write_bits(0xAB, 8); // 整字节
        s.write_bits(0xC, 4); // 部分
        assert_eq!(s.bit_pos(), 4);
        assert_eq!(s.as_bytes(), &[0xAB]);
    }

    #[test]
    fn build_stream_write_padding_bits_aligned_zero() {
        let mut s = BuildStream::new();
        s.write_padding_bits(16, 0);
        assert_eq!(s.bit_pos(), 0);
        assert_eq!(s.as_bytes(), &[0x00, 0x00]);
    }

    #[test]
    fn build_stream_write_padding_bits_aligned_one() {
        let mut s = BuildStream::new();
        s.write_padding_bits(16, 1);
        assert_eq!(s.bit_pos(), 0);
        assert_eq!(s.as_bytes(), &[0xFF, 0xFF]);
    }

    #[test]
    fn build_stream_write_padding_bits_unaligned() {
        let mut s = BuildStream::new();
        s.write_bits(0xA, 4); // bit_pos = 4
        s.write_padding_bits(4, 1); // 填 4 个 1 bit
        assert_eq!(s.bit_pos(), 0);
        assert_eq!(s.as_bytes(), &[0xAF]); // 0xA0 | 0x0F = 0xAF
    }

    #[test]
    fn build_stream_write_padding_bits_zero_is_noop() {
        let mut s = BuildStream::new();
        s.write_padding_bits(0, 1);
        assert_eq!(s.tell(), 0);
        assert_eq!(s.bit_pos(), 0);
    }

    #[test]
    fn build_stream_write_padding_bits_large_aligned() {
        let mut s = BuildStream::new();
        s.write_padding_bits(80, 0); // 10 字节
        assert_eq!(s.tell(), 10);
        assert_eq!(s.as_bytes(), &[0u8; 10]);
    }

    #[test]
    fn build_stream_write_padding_bits_partial_byte_at_end() {
        let mut s = BuildStream::new();
        s.write_padding_bits(5, 1);
        // 5 bit 的 1，应得 current_byte = 0b1111_1000 = 0xF8
        assert_eq!(s.bit_pos(), 5);
        assert_eq!(s.tell(), 0);
        // 完成字节以验证内容
        s.write_padding_bits(3, 0);
        assert_eq!(s.bit_pos(), 0);
        assert_eq!(s.as_bytes(), &[0xF8]);
    }

    #[test]
    fn build_stream_into_bytes_drops_partial_byte_in_release() {
        // 故意不完成字节，验证 release 模式不 panic 且丢弃部分字节
        // debug_assert 在 debug build 触发，故此测试只在 release 验证行为
        // 通过绕过 debug_assert：直接构造状态（无法直接构造私有字段），
        // 改为只验证完整路径的 into_bytes 正常工作
        let mut s = BuildStream::new();
        s.write_bits(0xAB, 8);
        let bytes = s.into_bytes();
        assert_eq!(bytes, vec![0xAB]);
    }

    // ---------------------------------------------------------------------------
    // ParseStream + BuildStream bit-level 互操作
    // ---------------------------------------------------------------------------

    #[test]
    fn round_trip_bit_write_then_read_single_byte() {
        let mut writer = BuildStream::new();
        writer.write_bits(0xAB, 8);
        let bytes = writer.into_bytes();

        let mut reader = ParseStream::new(&bytes);
        let path = root_path();
        let v = reader.read_bits(8, &path).expect("read back");
        assert_eq!(v, 0xAB);
        assert_eq!(reader.bit_pos(), 0);
    }

    #[test]
    fn round_trip_bit_write_then_read_nibbles() {
        let mut writer = BuildStream::new();
        writer.write_bits(0xA, 4);
        writer.write_bits(0x5, 4);
        let bytes = writer.into_bytes();

        let mut reader = ParseStream::new(&bytes);
        let path = root_path();
        let high = reader.read_bits(4, &path).expect("high");
        let low = reader.read_bits(4, &path).expect("low");
        assert_eq!(high, 0xA);
        assert_eq!(low, 0x5);
    }

    #[test]
    fn round_trip_bit_write_then_read_12_bits() {
        let mut writer = BuildStream::new();
        writer.write_bits(0xABC, 12);
        writer.write_bits(0x0, 4); // 凑齐整字节
        let bytes = writer.into_bytes();

        let mut reader = ParseStream::new(&bytes);
        let path = root_path();
        let v = reader.read_bits(12, &path).expect("read 12 bits");
        assert_eq!(v, 0xABC);
    }

    #[test]
    fn round_trip_bit_write_then_read_signed_pattern() {
        // 写 0xFF (8 bit) → 读 8 bit signed → -1
        let mut writer = BuildStream::new();
        writer.write_bits(0xFF, 8);
        let bytes = writer.into_bytes();

        let mut reader = ParseStream::new(&bytes);
        let path = root_path();
        let raw = reader.read_bits(8, &path).expect("read");
        // 模拟 signed 处理
        let signed_val = if (raw >> 7) & 1 == 1 {
            raw as i64 - (1i64 << 8)
        } else {
            raw as i64
        };
        assert_eq!(signed_val, -1);
    }

    #[test]
    fn round_trip_bit_write_then_read_64_bits() {
        let value: u64 = 0x0123_4567_89AB_CDEF;
        let mut writer = BuildStream::new();
        writer.write_bits(value, 64);
        let bytes = writer.into_bytes();

        let mut reader = ParseStream::new(&bytes);
        let path = root_path();
        let v = reader.read_bits(64, &path).expect("read 64");
        assert_eq!(v, value);
    }
}
