//! Strings 子模块：6 个 String Node + 公共编码层。
//!
//! # 模块组织
//!
//! - [`encoding`]：[`Encoding`] enum（编译期 6 变体）+ decode/encode helper
//!   （utf8/ascii 安全路径 + utf16/32 raw FFI）。
//! - [`c_string`]：[`CStringNode`]。
//! - [`greedy_string`]：[`GreedyStringNode`]。
//! - [`padded_string`]：[`PaddedStringNode`]。
//! - [`pascal_string`]：[`PascalStringNode`]。
//! - [`null_terminated`]：[`NullTerminatedNode`]。
//! - [`null_stripped`]：[`NullStrippedNode`]。
//!
//! # 公共 helper
//!
//! [`rstrip_pad`] 由 [`null_stripped::NullStrippedNode`] 与
//! [`padded_string::PaddedStringNode`] 共享。

pub mod c_string;
pub mod encoding;
pub mod greedy_string;
pub mod null_stripped;
pub mod null_terminated;
pub mod padded_string;
pub mod pascal_string;

pub use c_string::CStringNode;
pub use encoding::Encoding;
pub use greedy_string::GreedyStringNode;
pub use null_stripped::NullStrippedNode;
pub use null_terminated::NullTerminatedNode;
pub use padded_string::PaddedStringNode;
pub use pascal_string::PascalStringNode;

/// 右剥离 pad 字节串。
///
/// 对齐 Python construct `NullStripped._parse` 的 rstrip 算法（core.py L5156-5165）：
/// - `unit == 1`：单字节 pad，从末尾反复剥离匹配字节（等价 `data.rstrip(pad)`）。
/// - `unit > 1`：多字节 pad，先处理尾部不完整单元（长度 % unit），再剥离整 unit。
///
/// # 参数
///
/// - `data`：待剥离的字节切片。
/// - `pad`：pad 字节串（长度 ≥ 1，调用方负责校验）。
///
/// # 返回
///
/// 剥离后的子切片（借用 `data`）。`pad` 为空时原样返回（调用方应在编译期校验）。
///
/// # 算法细节（多字节 pad，对齐 Python core.py:5159-5165）
///
/// ```text
/// tailunit = len(data) % unit
/// end = len(data)
/// if tailunit != 0 and data[-tailunit:] == pad[:tailunit]:
///     end -= tailunit
/// while end - unit >= 0 and data[end-unit:end] == pad:
///     end -= unit
/// return data[:end]
/// ```
pub fn rstrip_pad<'a>(data: &'a [u8], pad: &[u8]) -> &'a [u8] {
    let unit = pad.len();
    if unit == 0 {
        return data;
    }
    if unit == 1 {
        let pb = pad[0];
        let mut end = data.len();
        while end > 0 && data[end - 1] == pb {
            end -= 1;
        }
        &data[..end]
    } else {
        let tailunit = data.len() % unit;
        let mut end = data.len();
        if tailunit != 0 && data[data.len() - tailunit..] == pad[..tailunit] {
            end -= tailunit;
        }
        while end >= unit && &data[end - unit..end] == pad {
            end -= unit;
        }
        &data[..end]
    }
}

#[cfg(test)]
mod tests {
    use super::rstrip_pad;

    #[test]
    fn rstrip_pad_single_byte_strips_trailing_zeros() {
        assert_eq!(rstrip_pad(b"abc\x00\x00", b"\x00"), b"abc");
        assert_eq!(rstrip_pad(b"abc", b"\x00"), b"abc");
        assert_eq!(rstrip_pad(b"\x00\x00", b"\x00"), b"");
        assert_eq!(rstrip_pad(b"", b"\x00"), b"");
    }

    #[test]
    fn rstrip_pad_single_byte_custom_pad() {
        // Python NullStripped pad 可任意字节（设计保留兼容性）
        assert_eq!(rstrip_pad(b"abcXX", b"X"), b"abc");
    }

    #[test]
    fn rstrip_pad_multibyte_full_units() {
        // utf16 pad = b"\x00\x00"，2 字节单元
        assert_eq!(rstrip_pad(b"ab\x00\x00\x00\x00", b"\x00\x00"), b"ab");
        assert_eq!(rstrip_pad(b"ab", b"\x00\x00"), b"ab");
    }

    #[test]
    fn rstrip_pad_multibyte_partial_tail_matching_prefix() {
        // 数据长度 % unit != 0 且尾部匹配 pad 前缀 → 剥离前缀
        // data = b"abcX"（4 字节），pad = b"XY"（unit=2），tailunit=0 不触发
        // 改为：data = b"abX"（3 字节），pad=b"XY"（unit=2），tailunit=1，
        //       data[-1:] = b"X" == pad[:1] = b"X" → end -= 1 → "ab"
        assert_eq!(rstrip_pad(b"abX", b"XY"), b"ab");
    }

    #[test]
    fn rstrip_pad_multibyte_partial_tail_non_matching() {
        // tailunit != 0 但尾部不匹配 pad 前缀 → 不剥离前缀，直接进 unit 循环
        // data = b"abZ"（3 字节），pad=b"XY"，tailunit=1，data[-1:]=b"Z" != b"X" → 不剥离
        assert_eq!(rstrip_pad(b"abZ", b"XY"), b"abZ");
    }

    #[test]
    fn rstrip_pad_empty_pad_returns_data_as_is() {
        // 空 pad 防御性返回原数据（实际调用方应校验 pad 非空）
        assert_eq!(rstrip_pad(b"abc", b""), b"abc");
    }
}
