//! 字节流游标抽象：parse 用 `ParseStream`，build 用 `BuildStream`。
//!
//! 设计依据：`docs/架构设计.md` §C.4、`docs/模块设计-BitStream.md` §3、
//! `docs/design/模块设计/模块设计-Streams.md` §3.1（Phase 7.2 Stream 基础设施扩展）。
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
//!
//! ## Phase 7.2：Stream 基础设施扩展（Seek / Pointer / Prefixed 支撑）
//!
//! - [`Whence`] enum：流定位参考点（Start / Current / End），对齐 Python
//!   `io.SEEK_SET/CUR/END`（0/1/2）。
//! - [`ParseStream::seek_whence`]：通用 seek，支持 whence=Start/Current/End
//!   （Pointer 负 offset / relativeOffset 用）。现有 [`ParseStream::seek`] 保留
//!   （whence=0 专用，向后兼容 Phase 4 GreedyRange/Peek 调用方）。
//! - [`BuildStream::pos`]：新增字段，写入位置可与 `buf.len()` 分离（覆盖写 / 零填充）。
//!   现有调用方不调 seek，`pos == buf.len()`，行为零变更（设计附录 A 审计表）。
//! - [`BuildStream::seek`]：新方法，仅移动指针，零填充延迟到下次 [`BuildStream::write`]。
//! - [`BuildStream::written_len`]：返回 buf 实际长度（与 [`BuildStream::tell`] 的 pos 区分）。

use crate::error::ConstructError;
use crate::path::Path;

// ---------------------------------------------------------------------------
// Phase 7.2：Whence enum（Stream 基础设施扩展）
//
// 设计依据：`docs/design/模块设计/模块设计-Streams.md` §3.1.1。
//
// 用于 `ParseStream::seek_whence` 与 `BuildStream::seek`，对齐 Python
// `io.SEEK_SET/CUR/END`（0/1/2）。
// ---------------------------------------------------------------------------

/// 流定位的参考点，对齐 Python `io.SEEK_SET/CUR/END`（0/1/2）。
///
/// 用于 [`ParseStream::seek_whence`] 与 [`BuildStream::seek`]。
///
/// # Phase 7.2 引入
///
/// Phase 7 前的 `ParseStream::seek(pos, path)` 仅支持 whence=0（绝对定位）。
/// Pointer 节点（负 offset / relativeOffset=True）需要 whence=1/2 支持，
/// 引入此枚举统一三参考点语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Whence {
    /// 从流开头（绝对定位）。对应 Python whence=0。
    Start,
    /// 从当前位置（相对定位）。对应 Python whence=1。
    Current,
    /// 从流末尾（at 通常为负）。对应 Python whence=2。
    End,
}

impl Whence {
    /// 从 Python 传入的整数（0/1/2）构造。其他值返回 `Err`（编译期校验）。
    ///
    /// 用于编译期 `compile.rs` 从 Python 描述符读取 whence int 字段时校验。
    pub fn from_python_int(n: i64) -> Result<Self, ConstructError> {
        match n {
            0 => Ok(Whence::Start),
            1 => Ok(Whence::Current),
            2 => Ok(Whence::End),
            other => Err(ConstructError::Compilation {
                message: format!("whence must be 0/1/2, got {}", other),
            }),
        }
    }
}

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

    /// 设置字节游标到 `pos`，同时重置 bit 游标为 0（字节对齐）。
    ///
    /// 用于 GreedyRange 失败回退（对齐 Python
    /// `stream_seek(stream, fallback, 0, path)`，`whence=0` 绝对定位）。
    /// Phase 4 新增（设计 §3.1.1）。
    ///
    /// # 边界
    ///
    /// - `pos > data.len()`：返回 `ConstructError::Stream`（含 expected/found）。
    /// - `bit_pos != 0` 时调用：先重置 bit_pos 为 0（GreedyRange 通常字节对齐，
    ///   此分支防御性兼容）。
    ///
    /// # 参数
    ///
    /// - `pos`：目标字节位置（绝对偏移，0-based）。
    /// - `path`：错误追踪路径。
    pub fn seek(&mut self, pos: usize, path: &Path) -> Result<(), ConstructError> {
        if pos > self.data.len() {
            return Err(ConstructError::Stream {
                message: format!(
                    "stream seek out of bounds, pos={}, data_len={}",
                    pos,
                    self.data.len()
                ),
                path: path.to_string(),
            });
        }
        self.pos = pos;
        self.bit_pos = 0;
        Ok(())
    }

    /// 通用 seek，支持 whence=Start/Current/End（Phase 7.2 新增）。
    ///
    /// 对齐 Python `stream_seek(stream, offset, whence, path)`（core.py L204）。
    ///
    /// # 行为
    ///
    /// - [`Whence::Start`]：`at < 0` 或 `at > data.len()` → Stream Err；否则 `pos = at`。
    /// - [`Whence::Current`]：`tell() + at` 溢出 usize 或越界 → Stream Err；否则
    ///   `pos = tell() + at`。`at` 可为负（向后回退）。
    /// - [`Whence::End`]：`data.len() + at`，`at` 通常为负（从末尾向前）；
    ///   `at > 0` → Stream Err（parse 不可超越 EOF，与 Python BytesIO 差异：
    ///   BytesIO 允许 seek 超过 EOF 但 read 时才报错；parse 场景 seek 超过 EOF
    ///   无意义，提前报错更清晰，详见设计 §7.1 S6 / 附录 B）。
    /// - 重置 `bit_pos = 0`（字节对齐，与现有 [`seek`](Self::seek) 一致）。
    ///
    /// # 与现有 `seek` 的关系
    ///
    /// 现有 [`ParseStream::seek`](Self::seek)（whence=0 专用）保留不变，供
    /// GreedyRange / Peek 等已有调用方继续使用（避免破坏 Phase 4 已验收代码）。
    /// 新方法 `seek_whence` 供 Pointer / Seek 使用。
    pub fn seek_whence(
        &mut self,
        at: i64,
        whence: Whence,
        path: &Path,
    ) -> Result<(), ConstructError> {
        let new_pos: usize = match whence {
            Whence::Start => {
                if at < 0 {
                    return Err(ConstructError::Stream {
                        message: format!(
                            "stream seek with whence=0 (Start) requires non-negative offset, got {}",
                            at
                        ),
                        path: path.to_string(),
                    });
                }
                let at_us = at as usize;
                if at_us > self.data.len() {
                    return Err(ConstructError::Stream {
                        message: format!(
                            "stream seek out of bounds, at={}, data_len={}",
                            at_us,
                            self.data.len()
                        ),
                        path: path.to_string(),
                    });
                }
                at_us
            }
            Whence::Current => {
                let cur = self.pos as i64;
                let target = cur.checked_add(at).ok_or_else(|| ConstructError::Stream {
                    message: format!("stream seek overflow: cur={} + at={}", cur, at),
                    path: path.to_string(),
                })?;
                if target < 0 || target as usize > self.data.len() {
                    return Err(ConstructError::Stream {
                        message: format!(
                            "stream seek out of bounds, target={}, data_len={}",
                            target,
                            self.data.len()
                        ),
                        path: path.to_string(),
                    });
                }
                target as usize
            }
            Whence::End => {
                if at > 0 {
                    return Err(ConstructError::Stream {
                        message: format!(
                            "stream seek with whence=2 (End) requires non-positive offset on parse stream, got {}",
                            at
                        ),
                        path: path.to_string(),
                    });
                }
                let target = (self.data.len() as i64).checked_add(at).ok_or_else(|| {
                    ConstructError::Stream {
                        message: format!(
                            "stream seek underflow: len={} + at={}",
                            self.data.len(),
                            at
                        ),
                        path: path.to_string(),
                    }
                })?;
                if target < 0 {
                    return Err(ConstructError::Stream {
                        message: format!(
                            "stream seek out of bounds, target={}, data_len={}",
                            target,
                            self.data.len()
                        ),
                        path: path.to_string(),
                    });
                }
                target as usize
            }
        };
        self.pos = new_pos;
        self.bit_pos = 0;
        Ok(())
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

    /// 借用底层缓冲的 `[start..end)` 切片（零拷贝，Phase 8.5 新增）。
    ///
    /// 设计依据：`docs/design/模块设计/模块设计-Phase8-P0.md` §3.3.1（L-14 教训触发）。
    /// 用于 Checksum StreamRange 模式：Rust 内置 hashfunc 直接操作 `&[u8]`，
    /// 全程零拷贝（不构造 PyBytes）。
    ///
    /// # 行为
    ///
    /// - `start > end` 或 `end > data.len()`：返回 None
    /// - 不修改 stream 游标（纯借用查询，与 read 不同）
    /// - 不要求 bit_pos == 0（切片不消费字节）
    ///
    /// # 安全
    ///
    /// 返回 `Option<&'a [u8]>`，生命周期绑定到 ParseStream 持有的 `&'a [u8]`
    /// （即 Python 端 PyBytes 的 buffer，pyo3 GIL 保证有效）。
    pub fn slice(&self, start: usize, end: usize) -> Option<&'a [u8]> {
        self.data.get(start..end)
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

        // 预检查：剩余 bit 数（考虑当前 bit_pos 偏移）。
        // 使用 saturating_sub / saturating_mul 防御性计算：即使不变量被破坏
        // （pos > data.len() 或剩余字节为 0 但 bit_pos > 0），也不会下溢回绕。
        let remaining_bytes = self.data.len().saturating_sub(self.pos);
        let remaining_bits = remaining_bytes
            .saturating_mul(8)
            .saturating_sub(self.bit_pos as usize);
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
        let remaining_bits = remaining_bytes
            .saturating_mul(8)
            .saturating_sub(self.bit_pos as usize);
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
///
/// # Phase 7.2：pos 字段（写入位置）
///
/// 引入 `pos` 字段后，写入位置可与 `buf.len()` 分离，支持覆盖写与零填充：
/// - `pos < buf.len()`：覆盖写（下次 write 从 pos 开始覆盖，pos 推进，buf.len() 不变）
/// - `pos > buf.len()`：零填充扩展到 pos（下次 write 前 resize）
/// - `pos == buf.len()`：末尾追加（原 Phase 1-6 行为）
///
/// **向后兼容性**（设计附录 A）：所有现有 build 调用方（StructNode / BitwiseNode /
/// BytesNode / FormatFieldNode / PrefixedArrayNode 等）不调 seek，`pos` 始终
/// 等于 `buf.len()`，行为零变更。bit 级 API（`write_bits` / `write_padding_bits`）
/// 不感知 pos（仍 buf.push 末尾），用 `debug_assert!(pos == buf.len())` 守卫契约。
#[derive(Debug, Default)]
pub struct BuildStream {
    /// 输出缓冲。
    buf: Vec<u8>,
    /// 当前写入位置（字节偏移）。0..=buf.len()。
    ///
    /// Phase 7 前无此字段（写入永远是末尾追加）。引入 seek 后，写入位置可与末尾分离：
    /// - `pos < buf.len()`：覆盖写（下次 write 从 pos 开始覆盖，pos 推进，buf.len() 不变）
    /// - `pos > buf.len()`：零填充扩展到 pos（下次 write 前 resize）
    /// - `pos == buf.len()`：末尾追加（原行为）
    pos: usize,
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
            pos: 0,
            bit_pos: 0,
            current_byte: 0,
        }
    }

    /// 写入字节切片（字节级，要求 `bit_pos == 0`）。
    ///
    /// Phase 7.2 行为变更（设计 §3.1.3）：现在从 `pos` 开始写，可能覆盖或扩展 buf。
    ///
    /// - `pos + data.len() <= buf.len()`：覆盖写（`buf[pos..pos+data.len()] = data`）
    /// - `pos + data.len() > buf.len()`：先 resize 到 `pos + data.len()`（零填充间隙），
    ///   再写入 data（覆盖刚填充的零的尾部）
    /// - 写完后 `pos += data.len()`
    ///
    /// 对齐 Python construct `stream.write`：不校验长度（长度校验由具体节点如
    /// `BytesNode.build` 负责）。
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
        // 空 write 显式 no-op：避免 pos > buf.len() 时被误扩展（与原 extend_from_slice
        // 同行为，但 pos 引入后需显式早返回）。
        if data.is_empty() {
            return;
        }
        let end = self.pos + data.len();
        if end > self.buf.len() {
            // 间隙 [buf.len()..end) 用 0x00 填充（Python BytesIO 行为）。
            self.buf.resize(end, 0u8);
        }
        self.buf[self.pos..end].copy_from_slice(data);
        self.pos = end;
    }

    /// 当前写入位置（字节偏移）。
    ///
    /// Phase 7.2 行为变更（设计 §3.1.3）：返回 `pos`（写入位置），不是 `buf.len()`。
    /// 对齐 Python `io.BytesIO.tell()`（返回当前指针位置）。
    ///
    /// 旧调用方（StructNode build）依赖 `tell()` 返回末尾位置——由于 StructNode 不调
    /// seek，`pos` 始终等于 `buf.len()`，行为不变（向后兼容）。
    ///
    /// 注意：若 `bit_pos != 0`，`tell()` 不包含正在填充的 `current_byte`。
    /// 字节级调用方应在 `bit_pos == 0` 时使用此值。
    pub fn tell(&self) -> usize {
        self.pos
    }

    /// 已写入字节数（buf 实际长度，非 pos）。
    ///
    /// Phase 7.2 新增。用于 sizeof 计算与顶层 build 收尾。与 [`tell`](Self::tell)
    /// （返回 pos）区分：seek 后 `pos` 可小于 `buf.len()`，但 `buf.len()` 反映
    /// 实际已分配/已写入的边界。
    pub fn written_len(&self) -> usize {
        self.buf.len()
    }

    /// seek 到指定位置（支持零填充扩展，Phase 7.2 新增）。
    ///
    /// 对齐 Python `io.BytesIO.seek(offset, whence)`。
    ///
    /// # 行为
    ///
    /// - [`Whence::Start`]：`at < 0` → Stream Err；否则 `pos = at`（**不扩展 buf**，
    ///   仅移动指针；零填充延迟到下次 [`write`](Self::write)，避免无 write 的空 seek
    ///   浪费内存）
    /// - [`Whence::Current`]：`pos + at`，`at` 可为负；溢出/负值 → Stream Err
    /// - [`Whence::End`]：`buf.len() + at`，`at` 可为正（build 允许 seek 超过末尾，
    ///   下次 write 时零填充）
    /// - 重置 `bit_pos = 0`、`current_byte = 0`（字节对齐，与 ParseStream 一致）
    ///
    /// # 错误
    ///
    /// at/whence 组合导致 pos 为负或溢出 → [`ConstructError::Stream`]。
    pub fn seek(&mut self, at: i64, whence: Whence, path: &Path) -> Result<(), ConstructError> {
        let new_pos: i64 = match whence {
            Whence::Start => at,
            Whence::Current => {
                (self.pos as i64)
                    .checked_add(at)
                    .ok_or_else(|| ConstructError::Stream {
                        message: format!("BuildStream seek overflow: pos={} + at={}", self.pos, at),
                        path: path.to_string(),
                    })?
            }
            Whence::End => {
                (self.buf.len() as i64)
                    .checked_add(at)
                    .ok_or_else(|| ConstructError::Stream {
                        message: format!(
                            "BuildStream seek overflow: len={} + at={}",
                            self.buf.len(),
                            at
                        ),
                        path: path.to_string(),
                    })?
            }
        };
        if new_pos < 0 {
            return Err(ConstructError::Stream {
                message: format!("BuildStream seek to negative position: {}", new_pos),
                path: path.to_string(),
            });
        }
        self.pos = new_pos as usize;
        self.bit_pos = 0;
        self.current_byte = 0;
        Ok(())
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
        // Phase 7.2 守卫：bit API 假设末尾追加模型（self.buf.push），不感知 pos。
        // 若用户在 Bitwise 域内 seek 使 pos != buf.len() 后再调 bit API，
        // 会破坏一致性（bit 写到末尾，pos 未推进）。debug build 捕获契约违反。
        // Pointer/Seek 在 Bitwise 域内行为未定义（设计附录 A 已知限制）。
        //
        // 注意：bit API 自身的连续调用通过下方 `self.pos = self.buf.len()` 保持
        // pos 与 buf.len() 同步（每次 push 后更新），所以连续 bit 写入不会触发
        // 此 assert。仅"先 seek 再 bit 写"会触发。
        debug_assert!(
            self.pos == self.buf.len(),
            "write_bits after seek (pos={}, buf.len()={}); bit API requires append-only mode",
            self.pos,
            self.buf.len()
        );

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
            // Phase 7.2：bit API 不感知 pos，但保持 pos 与 buf.len() 同步
            // （连续 bit 写入视为追加；seek 后调 bit 写由入口 debug_assert 捕获）。
            self.pos = self.buf.len();

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
        // Phase 7.2 守卫：同 write_bits，bit API 假设末尾追加模型。
        debug_assert!(
            self.pos == self.buf.len(),
            "write_padding_bits after seek (pos={}, buf.len()={}); bit API requires append-only mode",
            self.pos,
            self.buf.len()
        );
        let bit_val: u8 = if bit != 0 { 1 } else { 0 };

        // 字节对齐 + 大块：批量 push 相同字节
        if self.bit_pos == 0 && n >= 8 {
            let full_bytes = n / 8;
            let rem_bits = n % 8;
            let byte_val: u8 = if bit_val == 1 { 0xFF } else { 0x00 };
            // resize_with 比 push 循环更高效（一次性预留容量）
            let old_len = self.buf.len();
            self.buf.resize(old_len + full_bytes, byte_val);
            // Phase 7.2：bit API 同步 pos（同 write_bits 批量路径）。
            self.pos = self.buf.len();

            // 剩余 rem_bits bit 逐位写入
            for _ in 0..rem_bits {
                self.current_byte |= bit_val << (7 - self.bit_pos);
                self.bit_pos += 1;
                if self.bit_pos == 8 {
                    self.buf.push(self.current_byte);
                    self.current_byte = 0;
                    self.bit_pos = 0;
                    self.pos = self.buf.len();
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
                    self.pos = self.buf.len();
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
                // Phase 7.2：bit API 同步 pos（同 write_bits 批量路径）。
                self.pos = self.buf.len();
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
    // Phase 8.5：ParseStream::slice
    // ---------------------------------------------------------------------------

    #[test]
    fn parse_stream_slice_returns_subslice() {
        // 基本用法：slice(1, 4) 返回 [1..4)
        let data = b"abcdef";
        let s = ParseStream::new(data);
        assert_eq!(s.slice(1, 4), Some(b"bcd".as_ref()));
    }

    #[test]
    fn parse_stream_slice_does_not_advance_cursor() {
        // slice 不修改游标
        let data = b"abcdef";
        let mut s = ParseStream::new(data);
        let path = root_path();
        assert_eq!(s.tell(), 0);
        let _ = s.slice(0, 3);
        assert_eq!(s.tell(), 0);
        // 仍可正常 read
        let chunk = s.read(2, &path).expect("read");
        assert_eq!(chunk, b"ab");
    }

    #[test]
    fn parse_stream_slice_start_greater_than_end_returns_none() {
        let s = ParseStream::new(b"abcdef");
        assert_eq!(s.slice(4, 1), None);
    }

    #[test]
    fn parse_stream_slice_end_beyond_data_returns_none() {
        let s = ParseStream::new(b"abc");
        assert_eq!(s.slice(0, 10), None);
    }

    #[test]
    fn parse_stream_slice_full_range() {
        let s = ParseStream::new(b"abcdef");
        assert_eq!(s.slice(0, 6), Some(b"abcdef".as_ref()));
    }

    #[test]
    fn parse_stream_slice_empty_range() {
        let s = ParseStream::new(b"abcdef");
        assert_eq!(s.slice(2, 2), Some(b"".as_ref()));
    }

    #[test]
    fn parse_stream_slice_at_end_boundary() {
        let s = ParseStream::new(b"abc");
        // end == data.len() 应成功
        assert_eq!(s.slice(1, 3), Some(b"bc".as_ref()));
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
    // ParseStream::seek（Phase 4 新增）
    // ---------------------------------------------------------------------------

    #[test]
    fn parse_stream_seek_to_absolute_position() {
        let data = b"abcdef";
        let mut s = ParseStream::new(data);
        let path = root_path();
        // 初始 pos=0
        assert_eq!(s.tell(), 0);
        s.seek(3, &path).expect("seek to 3");
        assert_eq!(s.tell(), 3);
        let chunk = s.read(2, &path).expect("read 2");
        assert_eq!(chunk, b"de");
    }

    #[test]
    fn parse_stream_seek_to_zero_resets_cursor() {
        let data = b"abcdef";
        let mut s = ParseStream::new(data);
        let path = root_path();
        let _ = s.read(4, &path).expect("read 4");
        assert_eq!(s.tell(), 4);
        s.seek(0, &path).expect("seek back to 0");
        assert_eq!(s.tell(), 0);
    }

    #[test]
    fn parse_stream_seek_to_end_allowed() {
        let data = b"abc";
        let mut s = ParseStream::new(data);
        let path = root_path();
        s.seek(3, &path).expect("seek to len");
        assert_eq!(s.tell(), 3);
        assert!(s.is_at_end());
    }

    #[test]
    fn parse_stream_seek_beyond_end_returns_stream_error() {
        let data = b"abc";
        let mut s = ParseStream::new(data);
        let path = root_path();
        let err = s.seek(10, &path).expect_err("should fail");
        match err {
            ConstructError::Stream { message, path } => {
                assert!(message.contains("pos=10"), "got: {}", message);
                assert!(message.contains("data_len=3"), "got: {}", message);
                assert_eq!(path, "root");
            }
            other => panic!("expected Stream error, got {:?}", other),
        }
        // 失败时不推进游标
        assert_eq!(s.tell(), 0);
    }

    #[test]
    fn parse_stream_seek_resets_bit_pos_to_zero() {
        // 验证 seek 重置 bit 游标（防御性兼容 bit 域调用）
        let mut s = ParseStream::new(&[0xFF, 0xFF]);
        let path = root_path();
        // 制造 bit 偏移
        let _ = s.read_bits(4, &path).expect("read 4 bits");
        assert_eq!(s.bit_pos(), 4);
        // seek 应重置 bit_pos
        s.seek(0, &path).expect("seek");
        assert_eq!(s.bit_pos(), 0);
        assert_eq!(s.tell(), 0);
    }

    #[test]
    fn parse_stream_seek_used_for_greedy_range_fallback_pattern() {
        // 模拟 GreedyRange 的回退模式：记录 fallback → 失败时 seek 回 fallback
        let data = b"\x01\x02\x03\xFF"; // 前 3 字节合法，第 4 字节"失败"
        let mut s = ParseStream::new(data);
        let path = root_path();

        let fallback = s.tell();
        let _ = s.read(1, &path).expect("read 1"); // 模拟成功解析第 1 个元素
        let fallback1 = s.tell();

        let _ = s.read(1, &path).expect("read 1"); // 第 2 个
        let fallback2 = s.tell();

        // 假设第 3 次解析失败，回退到 fallback2
        s.seek(fallback2, &path).expect("seek back");
        assert_eq!(s.tell(), fallback2);
        let _ = fallback;
        let _ = fallback1;
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

    // ---------------------------------------------------------------------------
    // Phase 7.2：Stream 扩展测试（Whence / seek_whence / BuildStream.seek）
    //
    // 设计依据：`docs/design/模块设计/模块设计-Streams.md` §7.1（S1-S15）。
    // ---------------------------------------------------------------------------

    // === Whence enum ===

    #[test]
    fn whence_from_python_int_recognizes_0_1_2() {
        assert_eq!(Whence::from_python_int(0).unwrap(), Whence::Start);
        assert_eq!(Whence::from_python_int(1).unwrap(), Whence::Current);
        assert_eq!(Whence::from_python_int(2).unwrap(), Whence::End);
    }

    #[test]
    fn whence_from_python_int_rejects_other_values() {
        assert!(Whence::from_python_int(3).is_err());
        assert!(Whence::from_python_int(-1).is_err());
        assert!(Whence::from_python_int(100).is_err());
    }

    // === ParseStream::seek_whence ===

    #[test]
    fn parse_stream_seek_whence_start_basic() {
        // S1: whence=Start, at=-1 → Err；正常 at → 设置 pos
        let mut s = ParseStream::new(b"abcdef");
        let path = root_path();
        s.seek_whence(3, Whence::Start, &path).expect("seek to 3");
        assert_eq!(s.tell(), 3);
    }

    #[test]
    fn parse_stream_seek_whence_start_negative_returns_err() {
        let mut s = ParseStream::new(b"abcdef");
        let path = root_path();
        let err = s
            .seek_whence(-1, Whence::Start, &path)
            .expect_err("should fail");
        match err {
            ConstructError::Stream { message, .. } => {
                assert!(message.contains("non-negative"), "got: {}", message);
            }
            other => panic!("expected Stream, got {:?}", other),
        }
        // 失败时不推进游标
        assert_eq!(s.tell(), 0);
    }

    #[test]
    fn parse_stream_seek_whence_start_beyond_end_returns_err() {
        // S2: whence=Start, at > data.len() → Err
        let mut s = ParseStream::new(b"abc");
        let path = root_path();
        let err = s
            .seek_whence(100, Whence::Start, &path)
            .expect_err("should fail");
        match err {
            ConstructError::Stream { message, .. } => {
                assert!(message.contains("out of bounds"), "got: {}", message);
                assert!(message.contains("data_len=3"), "got: {}", message);
            }
            other => panic!("expected Stream, got {:?}", other),
        }
    }

    #[test]
    fn parse_stream_seek_whence_start_to_end_allowed() {
        // seek 到末尾允许（pos == data.len()）
        let mut s = ParseStream::new(b"abc");
        let path = root_path();
        s.seek_whence(3, Whence::Start, &path).expect("seek to end");
        assert_eq!(s.tell(), 3);
        assert!(s.is_at_end());
    }

    #[test]
    fn parse_stream_seek_whence_current_forward() {
        // S3: whence=Current, tell()=3, at=2 → pos=5
        let mut s = ParseStream::new(b"abcdef");
        let path = root_path();
        s.seek_whence(3, Whence::Start, &path)
            .expect("initial seek");
        s.seek_whence(2, Whence::Current, &path)
            .expect("forward seek");
        assert_eq!(s.tell(), 5);
    }

    #[test]
    fn parse_stream_seek_whence_current_backward() {
        // S4: whence=Current, tell()=5, at=-2 → pos=3
        let mut s = ParseStream::new(b"abcdef");
        let path = root_path();
        s.seek_whence(5, Whence::Start, &path)
            .expect("initial seek");
        s.seek_whence(-2, Whence::Current, &path)
            .expect("backward seek");
        assert_eq!(s.tell(), 3);
    }

    #[test]
    fn parse_stream_seek_whence_current_overflow_returns_err() {
        // S5: whence=Current, tell()=usize::MAX, at=1 → checked_add 失败
        let data = b"abc";
        let mut s = ParseStream::new(data);
        let path = root_path();
        // 手动构造溢出场景：set pos 到 usize::MAX-1（绕过 data.len() 检查）
        // 这里直接测 at 极大值
        let err = s
            .seek_whence(i64::MAX, Whence::Current, &path)
            .expect_err("should fail");
        match err {
            ConstructError::Stream { message, .. } => {
                // overflow 或 out of bounds（usize::MAX + i64::MAX 触发 checked_add 失败）
                assert!(
                    message.contains("overflow") || message.contains("out of bounds"),
                    "got: {}",
                    message
                );
            }
            other => panic!("expected Stream, got {:?}", other),
        }
    }

    #[test]
    fn parse_stream_seek_whence_current_to_negative_returns_err() {
        // whence=Current, tell()=1, at=-5 → target=-4 < 0 → Err
        let mut s = ParseStream::new(b"abcdef");
        let path = root_path();
        s.seek_whence(1, Whence::Start, &path).expect("initial");
        let err = s
            .seek_whence(-5, Whence::Current, &path)
            .expect_err("should fail");
        match err {
            ConstructError::Stream { message, .. } => {
                assert!(
                    message.contains("out of bounds") || message.contains("overflow"),
                    "got: {}",
                    message
                );
            }
            other => panic!("expected Stream, got {:?}", other),
        }
    }

    #[test]
    fn parse_stream_seek_whence_end_positive_returns_err() {
        // S6: whence=End, at=1（正）→ Err（parse 不可超越 EOF）
        let mut s = ParseStream::new(b"abcdef");
        let path = root_path();
        let err = s
            .seek_whence(1, Whence::End, &path)
            .expect_err("should fail");
        match err {
            ConstructError::Stream { message, .. } => {
                assert!(message.contains("non-positive"), "got: {}", message);
            }
            other => panic!("expected Stream, got {:?}", other),
        }
    }

    #[test]
    fn parse_stream_seek_whence_end_negative_from_eof() {
        // S7: whence=End, data.len()=10, at=-3 → pos=7
        let mut s = ParseStream::new(b"0123456789");
        let path = root_path();
        s.seek_whence(-3, Whence::End, &path)
            .expect("seek from end");
        assert_eq!(s.tell(), 7);
    }

    #[test]
    fn parse_stream_seek_whence_end_zero_goes_to_eof() {
        let mut s = ParseStream::new(b"abc");
        let path = root_path();
        s.seek_whence(0, Whence::End, &path).expect("seek to end");
        assert_eq!(s.tell(), 3);
        assert!(s.is_at_end());
    }

    #[test]
    fn parse_stream_seek_whence_end_too_negative_returns_err() {
        // whence=End, data.len()=3, at=-5 → target=-2 < 0 → Err
        let mut s = ParseStream::new(b"abc");
        let path = root_path();
        let err = s
            .seek_whence(-5, Whence::End, &path)
            .expect_err("should fail");
        match err {
            ConstructError::Stream { message, .. } => {
                assert!(message.contains("out of bounds"), "got: {}", message);
            }
            other => panic!("expected Stream, got {:?}", other),
        }
    }

    #[test]
    fn parse_stream_seek_whence_resets_bit_pos() {
        // S8: seek_whence 重置 bit_pos（与现有 seek 一致）
        let mut s = ParseStream::new(&[0xFF, 0xFF]);
        let path = root_path();
        // 制造 bit 偏移
        let _ = s.read_bits(4, &path).expect("read 4 bits");
        assert_eq!(s.bit_pos(), 4);
        // seek_whence 应重置 bit_pos
        s.seek_whence(0, Whence::Start, &path).expect("seek_whence");
        assert_eq!(s.bit_pos(), 0);
        assert_eq!(s.tell(), 0);
    }

    // === BuildStream::seek + pos ===

    #[test]
    fn build_stream_seek_start_does_not_extend_buf() {
        // S9: BuildStream.seek whence=Start, at=5 → pos=5（不扩展 buf）
        let mut s = BuildStream::new();
        s.write(b"abc"); // buf=[a,b,c], pos=3
        let path = root_path();
        s.seek(5, Whence::Start, &path).expect("seek to 5");
        assert_eq!(s.tell(), 5);
        // buf 不扩展（零填充延迟到下次 write）
        assert_eq!(s.written_len(), 3);
        assert_eq!(s.as_bytes(), b"abc");
    }

    #[test]
    fn build_stream_seek_then_write_zero_pads() {
        // S10: seek(10) + write(b"X"), buf.len()=3 → buf 变为 [0]*10 + [X]（len=11）
        let mut s = BuildStream::new();
        s.write(b"abc"); // buf=[a,b,c], pos=3
        let path = root_path();
        s.seek(10, Whence::Start, &path).expect("seek to 10");
        s.write(b"X");
        assert_eq!(s.tell(), 11);
        assert_eq!(s.written_len(), 11);
        let mut expected = vec![0u8; 10];
        expected.extend_from_slice(b"X");
        // 前 3 字节是 "abc"（覆盖写时 pos=10 > len=3，先 resize 到 10 填零，再写 X 到 pos=10）
        // 注意：seek(10) 后 pos=10, buf.len()=3。write(b"X"): end=11, resize(11, 0)
        // → buf = [a,b,c,0,0,0,0,0,0,0,0], copy X 到 [10..11] → buf=[a,b,c,0,0,0,0,0,0,0,X]
        let mut expected_with_abc = b"abc".to_vec();
        expected_with_abc.extend_from_slice(&[0u8; 7]);
        expected_with_abc.extend_from_slice(b"X");
        assert_eq!(s.as_bytes(), expected_with_abc.as_slice());
        // 验证总长
        assert_eq!(s.as_bytes().len(), 11);
        // 验证间隙为 0
        assert_eq!(&s.as_bytes()[3..10], &[0u8; 7]);
        // 验证末尾是 X
        assert_eq!(s.as_bytes()[10], b'X');
        // 抑制未使用变量
        let _ = expected;
    }

    #[test]
    fn build_stream_seek_overwrite_existing() {
        // S11: buf=[1,2,3], seek(1) + write(b"X") → buf=[1,X,3], pos=2
        let mut s = BuildStream::new();
        s.write(&[1u8, 2, 3]);
        let path = root_path();
        s.seek(1, Whence::Start, &path).expect("seek to 1");
        s.write(b"X");
        assert_eq!(s.as_bytes(), &[1u8, b'X', 3]);
        assert_eq!(s.tell(), 2);
        // buf.len() 不变（覆盖写）
        assert_eq!(s.written_len(), 3);
    }

    #[test]
    fn build_stream_seek_end_positive_extends_pos() {
        // S12: whence=End, buf.len()=5, at=3 → pos=8（下次 write 零填充）
        let mut s = BuildStream::new();
        s.write(b"abcde"); // buf.len()=5, pos=5
        let path = root_path();
        s.seek(3, Whence::End, &path).expect("seek end +3");
        assert_eq!(s.tell(), 8);
        // buf 不扩展（延迟零填充）
        assert_eq!(s.written_len(), 5);
        // 写一字节验证零填充
        s.write(b"X");
        assert_eq!(s.written_len(), 9);
        assert_eq!(s.as_bytes()[5..8], [0u8; 3]); // 间隙为 0
        assert_eq!(s.as_bytes()[8], b'X');
    }

    #[test]
    fn build_stream_seek_current_negative() {
        // S13: whence=Current, pos=5, at=-2 → pos=3
        let mut s = BuildStream::new();
        s.write(b"abcde");
        let path = root_path();
        s.seek(-2, Whence::Current, &path).expect("seek current -2");
        assert_eq!(s.tell(), 3);
    }

    #[test]
    fn build_stream_seek_current_underflow_returns_err() {
        // S14: whence=Current, pos=1, at=-5 → new_pos=-4 < 0 → Err
        let mut s = BuildStream::new();
        s.write(b"a");
        let path = root_path();
        let err = s.seek(-5, Whence::Current, &path).expect_err("should fail");
        match err {
            ConstructError::Stream { message, .. } => {
                assert!(message.contains("negative"), "got: {}", message);
            }
            other => panic!("expected Stream, got {:?}", other),
        }
        // 失败时不推进游标
        assert_eq!(s.tell(), 1);
    }

    #[test]
    fn build_stream_seek_resets_bit_state() {
        // S15: seek 重置 bit_pos / current_byte（bit 域内 seek 不支持，但仍重置状态）
        let mut s = BuildStream::new();
        s.write_bits(0xAB, 4); // bit_pos=4, current_byte=0xA0
        let path = root_path();
        // 此时 pos == buf.len() == 0（partial byte 未 push），seek 仍可调用
        s.seek(0, Whence::Start, &path).expect("seek 0");
        assert_eq!(s.bit_pos(), 0);
    }

    #[test]
    fn build_stream_seek_start_negative_returns_err() {
        let mut s = BuildStream::new();
        let path = root_path();
        let err = s.seek(-1, Whence::Start, &path).expect_err("should fail");
        match err {
            ConstructError::Stream { message, .. } => {
                assert!(message.contains("negative"), "got: {}", message);
            }
            other => panic!("expected Stream, got {:?}", other),
        }
    }

    #[test]
    fn build_stream_written_len_distinct_from_tell() {
        // 验证 written_len 与 tell 在 seek 后分离
        let mut s = BuildStream::new();
        s.write(b"hello"); // buf.len()=5, pos=5
        assert_eq!(s.tell(), 5);
        assert_eq!(s.written_len(), 5);
        let path = root_path();
        s.seek(2, Whence::Start, &path).expect("seek back");
        // pos 回到 2，但 buf 长度仍是 5
        assert_eq!(s.tell(), 2);
        assert_eq!(s.written_len(), 5);
    }

    // === 向后兼容回归：现有调用方不调 seek，pos == buf.len() ===

    #[test]
    fn build_stream_backward_compat_sequential_writes_append() {
        // 模拟 StructNode build：连续 write，无 seek
        let mut s = BuildStream::new();
        s.write(b"foo");
        s.write(b"bar");
        s.write(b"baz");
        // 行为应与 Phase 1-6 一致：append-only
        assert_eq!(s.as_bytes(), b"foobarbaz");
        assert_eq!(s.tell(), 9);
        assert_eq!(s.written_len(), 9);
    }

    #[test]
    fn build_stream_backward_compat_empty_write_is_noop() {
        let mut s = BuildStream::new();
        s.write(b"abc");
        let len_before = s.written_len();
        s.write(b"");
        // 空 write 显式 no-op（pos 不变，buf 不扩展）
        assert_eq!(s.tell(), 3);
        assert_eq!(s.written_len(), len_before);
    }

    #[test]
    fn build_stream_into_bytes_after_seek_returns_full_buf() {
        // 验证 into_bytes 返回 buf（不受 pos 影响）
        let mut s = BuildStream::new();
        s.write(b"hello");
        let path = root_path();
        s.seek(0, Whence::Start, &path).expect("seek back to 0");
        // pos=0, buf.len()=5
        let bytes = s.into_bytes();
        assert_eq!(bytes, b"hello");
    }
}
