//! BitsIntegerNode：bit 级整数读写。
//!
//! 设计依据：`docs/模块设计-BitStream.md` §4.1（bit 原子整数）、§5.1（swapbytesinbits）、
//! §9.1（边界条件 BI-1~BI-9）。
//! Python 参考：`construct/construct/core.py` `BitsInteger`（L1295-1406）、
//! `Bit` / `Nibble` / `Octet`（L1410-1420，均为 `BitsInteger` 的语法糖）、
//! `construct/construct/lib/binary.py` `bits2integer`（L57）、`integer2bits`（L6）、
//! `swapbytesinbits`（L135）。
//!
//! ## Phase 3.1 范围
//!
//! - `length` 为编译期常量（`usize`）。表达式 length 由编译管线在
//!   `build_node_from_descriptor` 中拒绝（返回 `Compilation` 错误）。
//! - `length` 上限 64（`MAX_BITS_INTEGER`），编译期校验。
//! - parse 调用 `ParseStream::read_bits(length)` 获取 MSB-first `u64`。
//! - build 接收 Python `int`，校验范围后转 `u64` 写入。
//!
//! ## signed 语义
//!
//! 对齐 Python `bits2integer(data, signed)`（binary.py L57）：signed=true 时，
//! 最高位为 1 表示负数（二补码）：`result - (1 << length)`。
//!
//! 边界：length == 64 且 signed=true 时，`1i64 << 64` 会溢出。改用 `i128` 中间值。
//!
//! ## swapped 语义（字节序）
//!
//! 对齐 Python `swapbytesinbits`（binary.py L135）：在 read/write 前后对 bit 串
//! 按 8 位组反序。**仅 length 为 8 倍数时有定义**（否则 Python 抛 ValueError，
//! construct-rs 返回 `FormatField` 错误，BI-6）。

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::conversion::IntoPy;
use pyo3::prelude::*;

/// BitsInteger 的 length 上限（u64 位宽）。
///
/// Python 无此限制（用大整数），construct-rs 限定为 u64 以覆盖所有实际用例。
/// 超出在编译期返回 `Compilation` 错误（§9.1 BI-3）。
pub const MAX_BITS_INTEGER: usize = 64;

// ---------------------------------------------------------------------------
// BitsIntegerNode
// ---------------------------------------------------------------------------

/// bit 级整数节点：读取/写入 `length` 个 bit，返回 Python `int`。
///
/// 对应 Python construct 的 `BitsInteger(length, signed, swapped)`，以及其语法糖
/// `Bit` (= `BitsInteger(1)`)、`Nibble` (= `BitsInteger(4)`)、
/// `Octet` (= `BitsInteger(8)`)。
///
/// 必须在 bit 域（`BitwiseNode` 内）使用。parse 调用 `stream.read_bits(length)`，
/// build 调用 `stream.write_bits(value, length)`。
///
/// # signed 语义
///
/// 对齐 Python `bits2integer(data, signed)`（`lib/binary.py` L57）：
/// signed=true 时，最高位为 1 表示负数（二补码）：`result - (1 << length)`。
/// 边界 length=64 + signed=true 时用 i128 中间值（§9.1 BI-4）。
///
/// # swapped 语义（字节序）
///
/// 对齐 Python `swapbytesinbits`（`lib/binary.py` L135）：在 read/write 前后对 bit 串
/// 按 8 位组反序。**仅 length 为 8 倍数时有定义**（否则返回 `FormatField` 错误，BI-6）。
/// 对应"Byte-Level Little-Endian"（分析报告 §2 四种组合）。
///
/// # sizeof
///
/// 返回 `length`（bit 数）。在 bit 域内由 `StructNode` 累加。
///
/// # 错误（§9.1 BI-1~BI-9）
///
/// - BI-1 length==0：parse/build 返回 `FormatField` 错误
/// - BI-2 length<0：编译期 `Compilation` 错误（在 `compile.rs` 中检测）
/// - BI-3 length>64：编译期 `Compilation` 错误（在 `compile.rs` 中检测）
/// - BI-4 length==64 + signed：用 i128 中间值，正确处理二补码
/// - BI-6 swapped + length%8!=0：parse/build 返回 `FormatField` 错误
/// - BI-7 build 非 int：返回 `FormatField` 错误
/// - BI-8 build 超范围：返回 `FormatField` 错误
/// - BI-9 流中 bit 不足：parse 返回 `Stream` 错误（由 `read_bits` 传播）
#[derive(Debug, Clone, Copy)]
pub struct BitsIntegerNode {
    /// bit 宽度（1..=64）。
    length: usize,
    /// 是否有符号（二补码）。
    signed: bool,
    /// 是否做字节序反序（要求 length % 8 == 0）。
    swapped: bool,
}

impl BitsIntegerNode {
    /// 创建 `BitsIntegerNode`。
    ///
    /// 注意：本构造函数不做参数校验（length 范围、swapped 与 length 关系等）。
    /// 校验在编译管线（`build_node_from_descriptor`）与 parse/build 时进行：
    /// - length<=0 / >64 / swapped+%8!=0 在 parse/build 时返回 `FormatField` 错误
    /// - length<0 / >64 在编译期返回 `Compilation` 错误（更早暴露）
    ///
    /// # 参数
    ///
    /// - `length`：bit 宽度。运行时校验 `> 0` 且 `<= MAX_BITS_INTEGER`（64）。
    /// - `signed`：是否为有符号整数（二补码）。
    /// - `swapped`：是否做字节序反序（要求 `length % 8 == 0`）。
    pub fn new(length: usize, signed: bool, swapped: bool) -> Self {
        Self {
            length,
            signed,
            swapped,
        }
    }

    /// 返回 bit 宽度。
    pub fn length(&self) -> usize {
        self.length
    }

    /// 是否为有符号整数。
    pub fn signed(&self) -> bool {
        self.signed
    }

    /// 是否做字节序反序。
    pub fn swapped(&self) -> bool {
        self.swapped
    }
}

// ---------------------------------------------------------------------------
// Construct impl
// ---------------------------------------------------------------------------

impl super::Construct for BitsIntegerNode {
    fn parse(
        &self,
        py: Python<'_>,
        stream: &mut ParseStream<'_>,
        _ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // BI-1: length must be positive
        if self.length == 0 {
            return Err(ConstructError::FormatField {
                message: format!("BitsInteger length {} must be positive", self.length),
                path: path.to_string(),
            });
        }
        // BI-3 defensive: length <= 64 (编译期已校验，运行时兜底)
        if self.length > MAX_BITS_INTEGER {
            return Err(ConstructError::FormatField {
                message: format!(
                    "BitsInteger length {} exceeds 64-bit limit (max {})",
                    self.length, MAX_BITS_INTEGER
                ),
                path: path.to_string(),
            });
        }
        // BI-6: swapped requires length % 8 == 0
        if self.swapped && !self.length.is_multiple_of(8) {
            return Err(ConstructError::FormatField {
                message: format!(
                    "BitsInteger swapped (little-endian) requires length {} to be a multiple of 8",
                    self.length
                ),
                path: path.to_string(),
            });
        }

        // 读取 length 个 bit（MSB-first u64）
        let raw = stream.read_bits(self.length, path)?;

        // 应用 swapped（字节序反序）
        let raw = if self.swapped {
            swapbytesinbits_u64(raw, self.length)
        } else {
            raw
        };

        // 应用 signed（二补码）
        // 使用 i128 中间值以正确处理 length=64 + signed 的边界（BI-4）
        let value: i128 = if self.signed && (raw >> (self.length - 1)) & 1 == 1 {
            // 最高位为 1 → 负数：raw - 2^length
            (raw as i128) - (1i128 << self.length)
        } else {
            raw as i128
        };

        // 转 PyLong（pyo3 支持 i128 → Python 任意精度 int）
        Ok(value.into_py(py))
    }

    fn build(
        &self,
        _py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        _ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        // BI-1: length must be positive
        if self.length == 0 {
            return Err(ConstructError::FormatField {
                message: format!("BitsInteger length {} must be positive", self.length),
                path: path.to_string(),
            });
        }
        if self.length > MAX_BITS_INTEGER {
            return Err(ConstructError::FormatField {
                message: format!(
                    "BitsInteger length {} exceeds 64-bit limit (max {})",
                    self.length, MAX_BITS_INTEGER
                ),
                path: path.to_string(),
            });
        }
        // BI-6: swapped requires length % 8 == 0
        if self.swapped && !self.length.is_multiple_of(8) {
            return Err(ConstructError::FormatField {
                message: format!(
                    "BitsInteger swapped (little-endian) requires length {} to be a multiple of 8",
                    self.length
                ),
                path: path.to_string(),
            });
        }

        // BI-7: obj must be a Python int
        // 使用 PyAny::is_instance_of::<PyLong>() 检测（注意：bool 是 int 的子类，
        // Python construct 的 isinstance(obj, int) 对 True/False 返回 True，
        // 行为等价）。
        if !obj.is_instance_of::<pyo3::types::PyLong>() {
            let type_name = obj
                .get_type()
                .name()
                .map(|n| n.to_string())
                .unwrap_or_else(|_| "<unknown>".to_string());
            return Err(ConstructError::FormatField {
                message: format!("BitsInteger value of type {} is not an integer", type_name),
                path: path.to_string(),
            });
        }

        // 提取 i128（覆盖 i64 范围 + 大正数报错路径）
        let value: i128 = match obj.extract::<i128>() {
            Ok(v) => v,
            Err(_) => {
                // extract 失败：可能超出 i128 范围（极大正/负数）
                let repr = obj
                    .repr()
                    .ok()
                    .and_then(|r| r.to_str().ok().map(String::from))
                    .unwrap_or_else(|| "<unknown>".to_string());
                return Err(ConstructError::FormatField {
                    message: format!(
                        "BitsInteger value {} out of representable range (i128)",
                        repr
                    ),
                    path: path.to_string(),
                });
            }
        };

        // BI-8: 范围校验（对齐 integer2fits L24）
        let (min, max) = if self.signed {
            // signed: -(2^(length-1)) .. 2^(length-1) - 1
            let m = 1i128 << (self.length - 1);
            (-m, m - 1)
        } else {
            // unsigned: 0 .. 2^length - 1
            (0i128, (1i128 << self.length) - 1)
        };
        if value < min || value > max {
            let repr = obj
                .repr()
                .ok()
                .and_then(|r| r.to_str().ok().map(String::from))
                .unwrap_or_else(|| "<unknown>".to_string());
            return Err(ConstructError::FormatField {
                message: format!(
                    "BitsInteger value {} out of range (min={}, max={}, length={}, signed={})",
                    repr, min, max, self.length, self.signed
                ),
                path: path.to_string(),
            });
        }

        // 转为 u64 raw（负数取二补码）
        let raw: u64 = if value < 0 {
            // value + 2^length 得到二补码表示
            // 使用 i128 避免溢出，结果在 u64 范围内（length <= 64）
            (value + (1i128 << self.length)) as u64
        } else {
            value as u64
        };

        // 应用 swapped
        let raw = if self.swapped {
            swapbytesinbits_u64(raw, self.length)
        } else {
            raw
        };

        // 写入 length 个 bit
        stream.write_bits(raw, self.length);
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        Ok(self.length)
    }
}

// ---------------------------------------------------------------------------
// 辅助函数（§5.1 swapbytesinbits）
// ---------------------------------------------------------------------------

/// 对 length-bit 的值做 swapbytesinbits（按 8 位组反序）。
///
/// 对应 Python `swapbytesinbits`（`lib/binary.py` L135）。length 必须是 8 倍数。
///
/// # 语义
///
/// `raw` 是 `read_bits` 返回的 MSB-first 整数（高位在前）。swapbytesinbits
/// 改变的是"字节序"（8 位组的顺序），不改变字节内 bit 顺序。等价于把
/// big-endian 字节序转成 little-endian 字节序。
///
/// 例：raw=0xAB_CD（length=16），swapbytesinbits → 0xCD_AB。
///
/// # 参数
///
/// - `raw`：MSB-first 整数值。
/// - `length`：bit 宽度，必须 `<= 64` 且 `% 8 == 0`。
fn swapbytesinbits_u64(raw: u64, length: usize) -> u64 {
    debug_assert!(length.is_multiple_of(8) && length <= 64);
    let n_bytes = length / 8;
    let mut result = 0u64;
    for i in 0..n_bytes {
        // 取 raw 的第 i 字节（从 MSB 起）
        let shift = length - 8 * (i + 1);
        let byte = ((raw >> shift) & 0xFF) as u8;
        // 放到 result 的反序位置（从 LSB 起）
        result |= (byte as u64) << (8 * i);
    }
    result
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

    /// 在 GIL 上下文中执行闭包。
    fn with_py<F, R>(f: F) -> R
    where
        F: for<'py> FnOnce(Python<'py>) -> R,
    {
        ensure_python();
        Python::with_gil(f)
    }

    // ======================================================================
    // swapbytesinbits_u64
    // ======================================================================

    #[test]
    fn swapbytesinbits_16_bits_reverses_byte_order() {
        // raw = 0xAB_CD → swap → 0xCD_AB
        assert_eq!(swapbytesinbits_u64(0xAB_CD, 16), 0xCD_AB);
    }

    #[test]
    fn swapbytesinbits_24_bits_reverses_byte_order() {
        // raw = 0x12_34_56 → swap → 0x56_34_12
        assert_eq!(swapbytesinbits_u64(0x12_34_56, 24), 0x56_34_12);
    }

    #[test]
    fn swapbytesinbits_8_bits_no_change() {
        assert_eq!(swapbytesinbits_u64(0xAB, 8), 0xAB);
    }

    #[test]
    fn swapbytesinbits_32_bits_reverses_byte_order() {
        // raw = 0x01_02_03_04 → swap → 0x04_03_02_01
        assert_eq!(swapbytesinbits_u64(0x01_02_03_04, 32), 0x04_03_02_01);
    }

    #[test]
    fn swapbytesinbits_64_bits_reverses_byte_order() {
        let raw: u64 = 0x01_23_45_67_89_AB_CD_EF;
        let expected: u64 = 0xEF_CD_AB_89_67_45_23_01;
        assert_eq!(swapbytesinbits_u64(raw, 64), expected);
    }

    // ======================================================================
    // 构造器与访问器
    // ======================================================================

    #[test]
    fn new_stores_params() {
        let n = BitsIntegerNode::new(8, true, false);
        assert_eq!(n.length(), 8);
        assert!(n.signed());
        assert!(!n.swapped());
    }

    #[test]
    fn new_default_signed_swapped_false() {
        let n = BitsIntegerNode::new(4, false, false);
        assert_eq!(n.length(), 4);
        assert!(!n.signed());
        assert!(!n.swapped());
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_returns_length_in_bits() {
        with_py(|py| {
            let ctx = Context::placeholder(py);
            assert_eq!(
                BitsIntegerNode::new(1, false, false).sizeof(&ctx).unwrap(),
                1
            );
            assert_eq!(
                BitsIntegerNode::new(4, false, false).sizeof(&ctx).unwrap(),
                4
            );
            assert_eq!(
                BitsIntegerNode::new(8, false, false).sizeof(&ctx).unwrap(),
                8
            );
            assert_eq!(
                BitsIntegerNode::new(64, false, false).sizeof(&ctx).unwrap(),
                64
            );
        });
    }

    // ======================================================================
    // parse — 各 length（无符号）
    // ======================================================================

    #[test]
    fn parse_bit_single_one() {
        // 0x80 第一 bit = 1
        with_py(|py| {
            let node = BitsIntegerNode::new(1, false, false);
            let mut stream = ParseStream::new(&[0x80]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let i: i64 = v.bind(py).extract().unwrap();
            assert_eq!(i, 1);
            assert_eq!(stream.bit_pos(), 1);
        });
    }

    #[test]
    fn parse_bit_single_zero() {
        // 0x00 第一 bit = 0
        with_py(|py| {
            let node = BitsIntegerNode::new(1, false, false);
            let mut stream = ParseStream::new(&[0x00]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let i: i64 = v.bind(py).extract().unwrap();
            assert_eq!(i, 0);
        });
    }

    #[test]
    fn parse_nibble_high_nibble() {
        // 0xA5 高 4 bit = 0xA = 10
        with_py(|py| {
            let node = BitsIntegerNode::new(4, false, false);
            let mut stream = ParseStream::new(&[0xA5]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let i: i64 = v.bind(py).extract().unwrap();
            assert_eq!(i, 10);
        });
    }

    #[test]
    fn parse_octet_full_byte() {
        // 0x42 = 66
        with_py(|py| {
            let node = BitsIntegerNode::new(8, false, false);
            let mut stream = ParseStream::new(&[0x42]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let i: i64 = v.bind(py).extract().unwrap();
            assert_eq!(i, 66);
        });
    }

    #[test]
    fn parse_12_bits_value() {
        // 0xAB 0xC0 = 0b1010_1011_1100_0000，前 12 bit = 0xABC = 2748
        with_py(|py| {
            let node = BitsIntegerNode::new(12, false, false);
            let mut stream = ParseStream::new(&[0xAB, 0xC0]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let i: i64 = v.bind(py).extract().unwrap();
            assert_eq!(i, 0xABC);
            assert_eq!(stream.bit_pos(), 4);
        });
    }

    #[test]
    fn parse_64_bits_max_unsigned() {
        with_py(|py| {
            let node = BitsIntegerNode::new(64, false, false);
            let mut stream = ParseStream::new(&[0xFF; 8]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // u64::MAX 在 Python 中通过 str 比对（i64 不够装）
            let s = format!("{}", v.bind(py).str().unwrap());
            assert_eq!(s, "18446744073709551615");
        });
    }

    // ======================================================================
    // parse — signed（BI-4, BI-5）
    // ======================================================================

    #[test]
    fn parse_signed_negative_8_bits() {
        // 0xFF = -1 (signed 8-bit)
        with_py(|py| {
            let node = BitsIntegerNode::new(8, true, false);
            let mut stream = ParseStream::new(&[0xFF]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let i: i64 = v.bind(py).extract().unwrap();
            assert_eq!(i, -1);
        });
    }

    #[test]
    fn parse_signed_positive_8_bits() {
        // 0x7F = 127 (signed 8-bit max)
        with_py(|py| {
            let node = BitsIntegerNode::new(8, true, false);
            let mut stream = ParseStream::new(&[0x7F]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let i: i64 = v.bind(py).extract().unwrap();
            assert_eq!(i, 127);
        });
    }

    #[test]
    fn parse_signed_negative_4_bits() {
        // 0xF0 高 4 bit = 0b1111 = -1 (signed 4-bit)
        with_py(|py| {
            let node = BitsIntegerNode::new(4, true, false);
            let mut stream = ParseStream::new(&[0xF0]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let i: i64 = v.bind(py).extract().unwrap();
            assert_eq!(i, -1);
        });
    }

    #[test]
    fn parse_signed_4_bits_min() {
        // 0x80 高 4 bit = 0b1000 = -8 (signed 4-bit min)
        with_py(|py| {
            let node = BitsIntegerNode::new(4, true, false);
            let mut stream = ParseStream::new(&[0x80]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let i: i64 = v.bind(py).extract().unwrap();
            assert_eq!(i, -8);
        });
    }

    #[test]
    fn parse_signed_64_bits_min() {
        // BI-4: length=64 + signed，最小值 = -2^63 = -9223372036854775808
        // 0x80 0x00 ... = 0b1000_0000_...0 (64 bit) = 2^63 as unsigned
        with_py(|py| {
            let node = BitsIntegerNode::new(64, true, false);
            let mut data = [0u8; 8];
            data[0] = 0x80;
            let mut stream = ParseStream::new(&data);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let s = format!("{}", v.bind(py).str().unwrap());
            assert_eq!(s, "-9223372036854775808");
        });
    }

    #[test]
    fn parse_signed_64_bits_max() {
        // BI-4: length=64 + signed，最大值 = 2^63 - 1 = 9223372036854775807
        // 0x7F 0xFF ... = 0b0111_1111_...1 (64 bit) = 2^63-1 as unsigned
        with_py(|py| {
            let node = BitsIntegerNode::new(64, true, false);
            let mut data = [0xFFu8; 8];
            data[0] = 0x7F;
            let mut stream = ParseStream::new(&data);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let s = format!("{}", v.bind(py).str().unwrap());
            assert_eq!(s, "9223372036854775807");
        });
    }

    // ======================================================================
    // parse — swapped（BI-6）
    // ======================================================================

    #[test]
    fn parse_swapped_16_bits_little_endian() {
        // swapped = LE。raw 读为 big-endian = 0x01_00 = 256，
        // swapbytesinbits → 0x00_01 = 1
        with_py(|py| {
            let node = BitsIntegerNode::new(16, false, true);
            let mut stream = ParseStream::new(&[0x01, 0x00]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let i: i64 = v.bind(py).extract().unwrap();
            assert_eq!(i, 1);
        });
    }

    #[test]
    fn parse_swapped_8_bits_no_change() {
        // 8 bit swapped = 自反（单字节）
        with_py(|py| {
            let node = BitsIntegerNode::new(8, false, true);
            let mut stream = ParseStream::new(&[0xAB]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let i: i64 = v.bind(py).extract().unwrap();
            assert_eq!(i, 0xAB);
        });
    }

    #[test]
    fn parse_swapped_with_non_multiple_of_8_returns_error() {
        // BI-6: swapped + length=12 (非 8 倍数) → FormatField error
        with_py(|py| {
            let node = BitsIntegerNode::new(12, false, true);
            let mut stream = ParseStream::new(&[0xAB, 0xC0]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::FormatField { message, path } => {
                    assert!(message.contains("multiple of 8"), "got: {}", message);
                    assert_eq!(path, "root");
                }
                other => panic!("expected FormatField, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // parse — 错误（BI-1 length=0, BI-9 insufficient）
    // ======================================================================

    #[test]
    fn parse_length_zero_returns_format_field_error() {
        // BI-1
        with_py(|py| {
            let node = BitsIntegerNode::new(0, false, false);
            let mut stream = ParseStream::new(&[0xFF]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::FormatField { message, .. } => {
                    assert!(message.contains("must be positive"), "got: {}", message);
                }
                other => panic!("expected FormatField, got {:?}", other),
            }
        });
    }

    #[test]
    fn parse_insufficient_bits_returns_stream_error() {
        // BI-9: 流中 8 bit，请求 12 bit
        with_py(|py| {
            let node = BitsIntegerNode::new(12, false, false);
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
        });
    }

    // ======================================================================
    // build — 各 length
    // ======================================================================

    #[test]
    fn build_bit_one() {
        with_py(|py| {
            let node = BitsIntegerNode::new(1, false, false);
            let obj = py.eval_bound("1", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.bit_pos(), 1);
            assert_eq!(stream.tell(), 0);
        });
    }

    #[test]
    fn build_nibble_value() {
        with_py(|py| {
            let node = BitsIntegerNode::new(4, false, false);
            let obj = py.eval_bound("10", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.bit_pos(), 4);
            assert_eq!(stream.tell(), 0);
        });
    }

    #[test]
    fn build_octet_value() {
        with_py(|py| {
            let node = BitsIntegerNode::new(8, false, false);
            let obj = py.eval_bound("0x42", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.bit_pos(), 0);
            assert_eq!(stream.as_bytes(), &[0x42]);
        });
    }

    #[test]
    fn build_64_bits_max_unsigned() {
        with_py(|py| {
            let node = BitsIntegerNode::new(64, false, false);
            let obj = py
                .eval_bound("18446744073709551615", None, None)
                .expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0xFF; 8]);
        });
    }

    // ======================================================================
    // build — signed
    // ======================================================================

    #[test]
    fn build_signed_negative_8_bits() {
        with_py(|py| {
            let node = BitsIntegerNode::new(8, true, false);
            let obj = py.eval_bound("-1", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0xFF]);
        });
    }

    #[test]
    fn build_signed_min_8_bits() {
        // -128 = 0x80 (signed 8-bit min)
        with_py(|py| {
            let node = BitsIntegerNode::new(8, true, false);
            let obj = py.eval_bound("-128", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x80]);
        });
    }

    #[test]
    fn build_signed_64_bits_min() {
        // BI-4: -2^63 build OK
        with_py(|py| {
            let node = BitsIntegerNode::new(64, true, false);
            let obj = py
                .eval_bound("-9223372036854775808", None, None)
                .expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            // -2^63 二补码 = 0x80 0x00 ... 0x00
            let mut expected = [0u8; 8];
            expected[0] = 0x80;
            assert_eq!(stream.as_bytes(), &expected);
        });
    }

    // ======================================================================
    // build — swapped
    // ======================================================================

    #[test]
    fn build_swapped_16_bits_value() {
        // value = 1, length = 16, swapped = true
        // raw = 0x0001，swapbytesinbits → 0x0100
        // 写入 16 bit (byte-aligned) = [0x01, 0x00]
        with_py(|py| {
            let node = BitsIntegerNode::new(16, false, true);
            let obj = py.eval_bound("1", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x01, 0x00]);
        });
    }

    // ======================================================================
    // build — 错误（BI-1, BI-6, BI-7, BI-8）
    // ======================================================================

    #[test]
    fn build_length_zero_returns_format_field_error() {
        // BI-1
        with_py(|py| {
            let node = BitsIntegerNode::new(0, false, false);
            let obj = py.eval_bound("1", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::FormatField { .. }));
        });
    }

    #[test]
    fn build_swapped_non_multiple_of_8_returns_error() {
        // BI-6
        with_py(|py| {
            let node = BitsIntegerNode::new(12, false, true);
            let obj = py.eval_bound("1", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::FormatField { message, .. } => {
                    assert!(message.contains("multiple of 8"), "got: {}", message);
                }
                other => panic!("expected FormatField, got {:?}", other),
            }
        });
    }

    #[test]
    fn build_non_integer_returns_format_field_error() {
        // BI-7
        with_py(|py| {
            let node = BitsIntegerNode::new(8, false, false);
            let obj = py.eval_bound("'hello'", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::FormatField { message, .. } => {
                    assert!(message.contains("not an integer"), "got: {}", message);
                }
                other => panic!("expected FormatField, got {:?}", other),
            }
        });
    }

    #[test]
    fn build_out_of_range_unsigned_returns_error() {
        // BI-8: 8-bit unsigned, value = 256 (> 255)
        with_py(|py| {
            let node = BitsIntegerNode::new(8, false, false);
            let obj = py.eval_bound("256", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::FormatField { message, .. } => {
                    assert!(message.contains("out of range"), "got: {}", message);
                }
                other => panic!("expected FormatField, got {:?}", other),
            }
        });
    }

    #[test]
    fn build_out_of_range_signed_returns_error() {
        // BI-8: 8-bit signed, value = 128 (> 127)
        with_py(|py| {
            let node = BitsIntegerNode::new(8, true, false);
            let obj = py.eval_bound("128", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::FormatField { .. }));
        });
    }

    #[test]
    fn build_negative_for_unsigned_returns_error() {
        // BI-8: 8-bit unsigned, value = -1 (< 0)
        with_py(|py| {
            let node = BitsIntegerNode::new(8, false, false);
            let obj = py.eval_bound("-1", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::FormatField { .. }));
        });
    }

    // ======================================================================
    // parse ↔ build 往返一致性
    // ======================================================================

    #[test]
    fn round_trip_octet_value() {
        with_py(|py| {
            let node = BitsIntegerNode::new(8, false, false);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            let obj = py.eval_bound("200", None, None).expect("eval");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();

            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let i: i64 = result.bind(py).extract().unwrap();
            assert_eq!(i, 200);
        });
    }

    #[test]
    fn round_trip_signed_16_bits_negative() {
        with_py(|py| {
            let node = BitsIntegerNode::new(16, true, false);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            let obj = py.eval_bound("-12345", None, None).expect("eval");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();

            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let i: i64 = result.bind(py).extract().unwrap();
            assert_eq!(i, -12345);
        });
    }

    #[test]
    fn round_trip_signed_64_bits_extreme() {
        with_py(|py| {
            let node = BitsIntegerNode::new(64, true, false);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            let obj = py
                .eval_bound("-9223372036854775808", None, None)
                .expect("eval"); // i64::MIN
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();

            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let s = format!("{}", result.bind(py).str().unwrap());
            assert_eq!(s, "-9223372036854775808");
        });
    }

    #[test]
    fn round_trip_unsigned_64_bits_max() {
        with_py(|py| {
            let node = BitsIntegerNode::new(64, false, false);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            let obj = py
                .eval_bound("18446744073709551615", None, None)
                .expect("eval"); // u64::MAX
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();

            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let s = format!("{}", result.bind(py).str().unwrap());
            assert_eq!(s, "18446744073709551615");
        });
    }

    #[test]
    fn round_trip_swapped_16_bits() {
        with_py(|py| {
            let node = BitsIntegerNode::new(16, false, true);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            let obj = py.eval_bound("0x1234", None, None).expect("eval");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();

            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let i: i64 = result.bind(py).extract().unwrap();
            assert_eq!(i, 0x1234);
        });
    }
}
