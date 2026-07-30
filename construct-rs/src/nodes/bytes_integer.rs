//! BytesIntegerNode：任意字节长度整数（对应 Python construct `BytesInteger`）。
//!
//! 设计依据：`docs/design/模块设计/模块设计-Primitives收尾.md` §1.5。
//! Python 参考：`construct/construct/core.py:1203-1292`（BytesInteger 类）、
//! `construct/construct/lib/binary.py:38-92`（integer2bytes / bytes2integer 工具）。
//!
//! ## 三档路径设计（D-2 决策 + 6.1-fix u128 扩展）
//!
//! - **u64 fast-path（length ≤ 8）**：Rust 原生 u64/i64 + endian 转换，零 Python 调用
//! - **u128 fast-path（9 ≤ length ≤ 16）**：Rust 原生 u128/i128 + pyo3 `IntoPy`，
//!   零 unsafe（pyo3 内部封装 `_PyLong_FromByteArray`，由 pyo3 维护者审计，
//!   与 `u64::into_py` 内部调 `PyLong_FromLongLong` 同性质）。
//!   设计依据：`docs/design/queries/设计质疑-BytesInteger-num-bigint.md` §3.1。
//! - **slow-path（length > 16）**：调用 Python `int.from_bytes` / `int.to_bytes`
//!   （CPython 公开稳定 API，自 Python 3.2 起，无 unsafe raw FFI）
//!
//! u64/u128 fast-path 覆盖绝大多数实际用法（UUID / GUID / SHA-1 / MD5 / IPv6 数值
//! 表示均为 16 字节），slow-path 仅触发于显式 `BytesInteger(17+)` 极罕见用法
//! （如 RSA 模数通常用 Bytes 而非 BytesInteger）。详见设计 §1.5.1 D-2 决策表
//! 与设计质疑 §3.3 / §4.1。
//!
//! ## Int24 系列
//!
//! Int24ub/ul/sb/sl 是 `BytesIntegerNode { length: 3, ... }` 的 Python 层别名
//! （Int24ub = BytesInteger(3, signed=False, swapped=False)，依此类推）。
//! 3 字节走 fast-path，符号扩展 / endian 由 fast-path 矩阵处理。
//!
//! ## 错误类型（P1 v2 修正）
//!
//! 所有整数相关错误统一用 [`ConstructError::Integer`]（对应 Python `IntegerError`），
//! 与 `core.py:1248-1267` 对齐。v1 误用 `FormatField` 已修正。

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::conversion::IntoPy;
use pyo3::prelude::*;

/// u64 fast-path 上限：length ≤ 8 走 Rust 原生 u64/i64。
const U64_PATH_MAX: usize = 8;

/// u128 fast-path 上限：9 ≤ length ≤ 16 走 Rust 原生 u128/i128 + pyo3 `IntoPy`。
/// 设计依据：`docs/design/queries/设计质疑-BytesInteger-num-bigint.md` §3.1。
const U128_PATH_MAX: usize = 16;

/// 任意字节长度整数节点（对应 Python construct `BytesInteger`）。
///
/// 字段：
/// - `length`：字节长度（编译期已知，必须 > 0）
/// - `signed`：是否有符号（two's complement）
/// - `swapped`：是否小端（true = little endian，false = big endian）
///
/// Int24ub/ul/sb/sl 是 `BytesIntegerNode { length: 3, ... }` 的 Python 层别名。
#[derive(Debug, Clone)]
pub struct BytesIntegerNode {
    /// 字节长度（编译期已知）。必须 > 0（length==0 在 parse/build 返回 IntegerError，
    /// 对齐 Python `core.py:1248-1249`）。
    length: usize,
    /// 是否有符号（two's complement）。
    signed: bool,
    /// 是否小端（true = little endian，false = big endian）。
    swapped: bool,
}

impl BytesIntegerNode {
    /// 创建 `BytesIntegerNode`。
    ///
    /// 不做参数校验（length > 0 等），校验在 parse/build 时进行（返回 IntegerError，
    /// 对齐 Python 在运行时抛 IntegerError 的行为）。
    pub fn new(length: usize, signed: bool, swapped: bool) -> Self {
        Self {
            length,
            signed,
            swapped,
        }
    }

    /// 返回字节长度。
    pub fn length(&self) -> usize {
        self.length
    }

    /// 是否有符号。
    pub fn signed(&self) -> bool {
        self.signed
    }

    /// 是否小端。
    pub fn swapped(&self) -> bool {
        self.swapped
    }
}

impl super::Construct for BytesIntegerNode {
    fn parse(
        &self,
        py: Python<'_>,
        stream: &mut ParseStream<'_>,
        _ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        if self.length == 0 {
            // 对齐 Python core.py:1248-1249 `raise IntegerError("length must be positive")`。
            // P1 v2 修正：用 ConstructError::Integer（v1 误用 FormatField）。
            return Err(ConstructError::Integer {
                message: format!("length {} must be positive", self.length),
                path: path.to_string(),
            });
        }
        let data = stream.read(self.length, path)?;

        if self.length <= U64_PATH_MAX {
            // ---- u64 fast-path：Rust 原生 u64/i64 + endian 转换 ----
            // 将 data 补齐到 8 字节（按 endian 决定补齐方向与符号扩展）。
            let mut buf = [0u8; 8];
            if self.swapped {
                // little endian：data 放低位（buf 前段），高位补 0（或符号扩展 0xFF）
                buf[..self.length].copy_from_slice(data);
                if self.signed && (data[self.length - 1] & 0x80 != 0) {
                    // 符号扩展：高位填充 0xFF（two's complement 负数）
                    for b in &mut buf[self.length..] {
                        *b = 0xFF;
                    }
                }
                let val_u64 = u64::from_le_bytes(buf);
                if self.signed {
                    Ok((val_u64 as i64).into_py(py))
                } else {
                    Ok(val_u64.into_py(py))
                }
            } else {
                // big endian：data 放高位（buf 后段），低位补 0（或符号扩展 0xFF）
                buf[8 - self.length..].copy_from_slice(data);
                if self.signed && (data[0] & 0x80 != 0) {
                    for b in &mut buf[..8 - self.length] {
                        *b = 0xFF;
                    }
                }
                let val_u64 = u64::from_be_bytes(buf);
                if self.signed {
                    Ok((val_u64 as i64).into_py(py))
                } else {
                    Ok(val_u64.into_py(py))
                }
            }
        } else if self.length <= U128_PATH_MAX {
            // ---- u128 fast-path：Rust 原生 u128/i128 + pyo3 IntoPy（6.1-fix 新增） ----
            // 逻辑与 u64 fast-path 同构，仅位宽从 8 → 16 字节。
            // pyo3 0.22 原生支持 u128/i128 IntoPy（内部走 _PyLong_FromByteArray 类路径）。
            let mut buf = [0u8; 16];
            if self.swapped {
                // little endian：data 放低位（buf 前段），高位补 0（或符号扩展 0xFF）
                buf[..self.length].copy_from_slice(data);
                if self.signed && (data[self.length - 1] & 0x80 != 0) {
                    for b in &mut buf[self.length..] {
                        *b = 0xFF;
                    }
                }
                let val_u128 = u128::from_le_bytes(buf);
                if self.signed {
                    Ok((val_u128 as i128).into_py(py))
                } else {
                    Ok(val_u128.into_py(py))
                }
            } else {
                // big endian：data 放高位（buf 后段），低位补 0（或符号扩展 0xFF）
                buf[16 - self.length..].copy_from_slice(data);
                if self.signed && (data[0] & 0x80 != 0) {
                    for b in &mut buf[..16 - self.length] {
                        *b = 0xFF;
                    }
                }
                let val_u128 = u128::from_be_bytes(buf);
                if self.signed {
                    Ok((val_u128 as i128).into_py(py))
                } else {
                    Ok(val_u128.into_py(py))
                }
            }
        } else {
            // ---- slow-path：调用 Python int.from_bytes（D-2 决策，无 unsafe） ----
            parse_bigint_from_bytes(py, data, self.signed, self.swapped, path)
        }
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        _ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        if self.length == 0 {
            // 对齐 Python core.py:1262-1263 `if length <= 0: raise IntegerError(...)`。
            // P1 v2 补全（parity 完备）。
            return Err(ConstructError::Integer {
                message: format!("length {} must be positive", self.length),
                path: path.to_string(),
            });
        }

        if self.length <= U64_PATH_MAX {
            // ---- u64 fast-path：extract i64/u64 → 字节序转换 → write ----
            let (val_u64, is_negative_signed) = if self.signed {
                let val: i64 = obj.extract::<i64>().map_err(|_| {
                    make_int_error("value is not an integer or out of i64 range", obj, path)
                })?;
                (val as u64, val < 0)
            } else {
                let val: u64 = obj.extract::<u64>().map_err(|_| {
                    make_int_error("value is not an integer or out of u64 range", obj, path)
                })?;
                (val, false)
            };
            // 范围检查（与 Python integer2bytes 一致：超范围报 IntegerError）
            self.check_range(val_u64, is_negative_signed, obj, path)?;

            let buf = if self.swapped {
                val_u64.to_le_bytes()
            } else {
                val_u64.to_be_bytes()
            };
            // 截取 self.length 字节（按 endian 决定截取位置）
            let bytes_to_write: &[u8] = if self.swapped {
                // little endian：低字节在前（buf 前段）
                &buf[..self.length]
            } else {
                // big endian：高字节在前（buf 后段）
                &buf[8 - self.length..]
            };
            stream.write(bytes_to_write);
            Ok(())
        } else if self.length <= U128_PATH_MAX {
            // ---- u128 fast-path：extract i128/u128 → 字节序转换 → write（6.1-fix 新增） ----
            let (val_u128, is_negative_signed) = if self.signed {
                let val: i128 = obj.extract::<i128>().map_err(|_| {
                    make_int_error("value is not an integer or out of i128 range", obj, path)
                })?;
                (val as u128, val < 0)
            } else {
                let val: u128 = obj.extract::<u128>().map_err(|_| {
                    make_int_error("value is not an integer or out of u128 range", obj, path)
                })?;
                (val, false)
            };
            // 范围检查（与 Python integer2bytes 一致：超范围报 IntegerError）
            self.check_range_u128(val_u128, is_negative_signed, obj, path)?;

            let buf = if self.swapped {
                val_u128.to_le_bytes()
            } else {
                val_u128.to_be_bytes()
            };
            // 截取 self.length 字节（按 endian 决定截取位置）
            let bytes_to_write: &[u8] = if self.swapped {
                // little endian：低字节在前（buf 前段）
                &buf[..self.length]
            } else {
                // big endian：高字节在前（buf 后段）
                &buf[16 - self.length..]
            };
            stream.write(bytes_to_write);
            Ok(())
        } else {
            // ---- slow-path：调用 Python int.to_bytes ----
            build_bigint_to_bytes(
                py,
                obj,
                self.length,
                self.signed,
                self.swapped,
                stream,
                path,
            )
        }
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        Ok(self.length)
    }
}

impl BytesIntegerNode {
    /// fast-path build 范围检查（对齐 Python `lib/binary.py:integer2bytes`）。
    ///
    /// 校验 val_u64 / val_i64 是否能装入 `length` 字节的 unsigned / signed 表示。
    /// 超范围返回 IntegerError（对齐 Python core.py:1259-1260）。
    fn check_range(
        &self,
        val_u64: u64,
        is_negative_signed: bool,
        obj: &Bound<'_, PyAny>,
        path: &Path,
    ) -> Result<(), ConstructError> {
        if self.signed {
            // 有符号范围：[-2^(8n-1), 2^(8n-1) - 1]
            // n ≤ 8 故 8n ≤ 64，shift 不溢出（n=8 时 8n-1=63，1<<63 = i64::MIN abs）
            let bits = (self.length as u32) * 8;
            // 最大正值 = 2^(bits-1) - 1，最小负值 = -2^(bits-1)
            // 用 i128 中间值避免溢出（bits=64 时 1<<64 溢出 i64）
            let max_pos: i128 = (1i128 << (bits - 1)) - 1;
            let min_neg: i128 = -(1i128 << (bits - 1));
            // 将 val_u64 解释为 i64（two's complement）再扩展为 i128
            let val_i128: i128 = val_u64 as i64 as i128;
            // is_negative_signed 标志来自原 i64；val_i128 已经包含正确符号
            let _ = is_negative_signed; // 保留参数语义清晰，实际通过 val_i128 符号位判定
            if val_i128 < min_neg || val_i128 > max_pos {
                return Err(make_int_error(
                    format!(
                        "value {} out of range for signed BytesInteger({})",
                        obj.repr()
                            .ok()
                            .and_then(|r| r.to_str().ok().map(String::from))
                            .unwrap_or_else(|| "<unknown>".to_string()),
                        self.length
                    ),
                    obj,
                    path,
                ));
            }
        } else {
            // 无符号范围：[0, 2^(8n) - 1]
            let bits = (self.length as u32) * 8;
            // 1u128 << bits 在 bits=64 时为 1<<64，超过 u64，需 u128 中间值
            let max_unsigned: u128 = if bits >= 64 {
                u64::MAX as u128
            } else {
                (1u128 << bits) - 1
            };
            // val_u64 已是 u64；bits < 64 时需额外上界检查
            if bits < 64 && (val_u64 as u128) > max_unsigned {
                return Err(make_int_error(
                    format!(
                        "value {} out of range for unsigned BytesInteger({})",
                        obj.repr()
                            .ok()
                            .and_then(|r| r.to_str().ok().map(String::from))
                            .unwrap_or_else(|| "<unknown>".to_string()),
                        self.length
                    ),
                    obj,
                    path,
                ));
            }
            // bits == 64 时 u64 全范围有效，无需额外检查
        }
        Ok(())
    }

    /// u128 fast-path build 范围检查（6.1-fix 新增，对齐 Python `lib/binary.py:integer2bytes`）。
    ///
    /// 校验 val_u128 / val_i128 是否能装入 `length`（9-16）字节的 unsigned / signed 表示。
    /// 超范围返回 IntegerError（对齐 Python core.py:1259-1260）。
    ///
    /// # 边界处理
    ///
    /// - `length` 范围由调用方保证为 9..=16（`U64_PATH_MAX < length <= U128_PATH_MAX`）。
    /// - `bits = length * 8` 范围为 72..=128。
    /// - signed `bits == 128`（即 length=16）时直接判 i128 全范围有效
    ///   （不能用 `1i128 << 127`：它会 wrap 成 i128::MIN，`half - 1` 再溢出 panic）。
    /// - unsigned `bits == 128` 时同理判为 u128 全范围有效。
    /// - bits ∈ [72, 120]（length ∈ [9, 15]）时 `1 << (bits-1)` / `1 << bits` 远小于类型上界，安全。
    fn check_range_u128(
        &self,
        val_u128: u128,
        is_negative_signed: bool,
        obj: &Bound<'_, PyAny>,
        path: &Path,
    ) -> Result<(), ConstructError> {
        if self.signed {
            // 有符号范围：[-2^(bits-1), 2^(bits-1) - 1]，bits ∈ [72, 128]
            let bits = (self.length as u32) * 8;
            // 将 val_u128 位模式解释为 i128（two's complement）
            let val_i128: i128 = val_u128 as i128;
            // is_negative_signed 标志来自原 i128；val_i128 已含正确符号
            let _ = is_negative_signed;
            if bits >= 128 {
                // length == 16：i128 全范围有效，无需检查。
                // （注意：1i128 << 127 会得到 i128::MIN，half-1 再溢出，故必须显式判断。）
            } else {
                // bits ∈ [72, 120]，half = 2^(bits-1) ∈ [2^71, 2^119]，half-1 不溢出 i128。
                let half: i128 = 1i128 << (bits - 1);
                let max_pos: i128 = half - 1;
                let min_neg: i128 = -half;
                if val_i128 < min_neg || val_i128 > max_pos {
                    return Err(out_of_range_int_error(
                        obj,
                        self.length,
                        /* signed */ true,
                        path,
                    ));
                }
            }
        } else {
            // 无符号范围：[0, 2^bits - 1]，bits ∈ [72, 128]
            let bits = (self.length as u32) * 8;
            if bits >= 128 {
                // length == 16：u128 全范围有效，无需检查。
            } else {
                // bits ∈ [72, 120]，1u128 << bits = 2^bits ∈ [2^72, 2^120]，远小于 u128::MAX。
                let max_unsigned: u128 = (1u128 << bits) - 1;
                if val_u128 > max_unsigned {
                    return Err(out_of_range_int_error(
                        obj,
                        self.length,
                        /* signed */ false,
                        path,
                    ));
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// slow-path 辅助函数
// ---------------------------------------------------------------------------

/// Slow-path：调用 Python `int.from_bytes(data, byteorder, signed=signed)`。
///
/// 对齐 Python `lib/binary.py:bytes2integer`（L82） + `core.py:1255-1256`。
/// 返回 Python 原生 int（无 Rust 中间类型，符合 §0 #2）。
fn parse_bigint_from_bytes(
    py: Python<'_>,
    data: &[u8],
    signed: bool,
    swapped: bool,
    path: &Path,
) -> Result<Py<PyAny>, ConstructError> {
    let byteorder = if swapped { "little" } else { "big" };
    let py_bytes = pyo3::types::PyBytes::new_bound(py, data);
    // pyo3 0.22：通过 builtins 访问 int 类型对象
    // （Python::get_type 在 0.22 不存在；import_bound("builtins").getattr("int") 最稳定）。
    let int_type = py
        .import_bound("builtins")
        .map_err(|e| ConstructError::Integer {
            message: format!("failed to import builtins: {}", e),
            path: path.to_string(),
        })?
        .getattr("int")
        .map_err(|e| ConstructError::Integer {
            message: format!("builtins has no 'int': {}", e),
            path: path.to_string(),
        })?;
    // Python 3.11+：int.from_bytes(bytes, byteorder, *, signed=False)
    // signed 是 keyword-only 参数，必须通过 kwargs 传递（位置传 3 个参数会 TypeError）。
    let kwargs = pyo3::types::PyDict::new_bound(py);
    kwargs
        .set_item("signed", signed)
        .map_err(|e| ConstructError::Integer {
            message: format!("failed to set signed kwarg: {}", e),
            path: path.to_string(),
        })?;
    let result = int_type
        .call_method("from_bytes", (py_bytes, byteorder), Some(&kwargs))
        .map_err(|e| ConstructError::Integer {
            // 对齐 Python core.py:1256 `raise IntegerError(str(e))`。
            message: format!("int.from_bytes failed: {}", e),
            path: path.to_string(),
        })?;
    Ok(result.into_py(py))
}

/// Slow-path：调用 Python `obj.to_bytes(length, byteorder, signed=signed)`。
///
/// 对齐 Python `lib/binary.py:integer2bytes`（L41） + `core.py:1266-1267`。
fn build_bigint_to_bytes(
    py: Python<'_>,
    obj: &Bound<'_, PyAny>,
    length: usize,
    signed: bool,
    swapped: bool,
    stream: &mut BuildStream,
    path: &Path,
) -> Result<(), ConstructError> {
    let byteorder = if swapped { "little" } else { "big" };
    // Python 3.11+：int.to_bytes(length, byteorder, *, signed=False)
    // signed 是 keyword-only 参数，必须通过 kwargs 传递。
    let kwargs = pyo3::types::PyDict::new_bound(py);
    kwargs
        .set_item("signed", signed)
        .map_err(|e| ConstructError::Integer {
            message: format!("failed to set signed kwarg: {}", e),
            path: path.to_string(),
        })?;
    let result = obj
        .call_method("to_bytes", (length, byteorder), Some(&kwargs))
        .map_err(|e| ConstructError::Integer {
            // 对齐 Python core.py:1267 `raise IntegerError(str(e))`。
            // Python to_bytes 在超范围 / 负数 + unsigned 时触发 OverflowError → IntegerError
            message: format!("int.to_bytes failed: {}", e),
            path: path.to_string(),
        })?;
    let bytes_ref =
        result
            .downcast::<pyo3::types::PyBytes>()
            .map_err(|_| ConstructError::Integer {
                message: "int.to_bytes did not return bytes".into(),
                path: path.to_string(),
            })?;
    stream.write(bytes_ref.as_bytes());
    Ok(())
}

/// 构造 `ConstructError::Integer`（与 Python 错误信息对齐）。
///
/// 接受 `&str` 或 `String`（用 `Into<String>`），便于 fast-path build 多处调用。
fn make_int_error<M: Into<String>>(msg: M, obj: &Bound<'_, PyAny>, path: &Path) -> ConstructError {
    // 附带对象 repr 便于调试（obj 在错误路径才格式化，零成功路径开销）
    let _ = obj; // 标记参数已使用（msg 已含 repr 时可省略 obj）
    ConstructError::Integer {
        message: msg.into(),
        path: path.to_string(),
    }
}

/// 构造 "out of range" IntegerError（对齐 Python core.py:1259-1260 `raise IntegerError(str(e))`）。
///
/// `signed` 控制错误信息措辞，`length` 为字节长度。
/// 用于 `check_range` / `check_range_u128` 超范围分支，减少 repr 格式化重复。
fn out_of_range_int_error(
    obj: &Bound<'_, PyAny>,
    length: usize,
    signed: bool,
    path: &Path,
) -> ConstructError {
    let kind = if signed { "signed" } else { "unsigned" };
    let repr = obj
        .repr()
        .ok()
        .and_then(|r| r.to_str().ok().map(String::from))
        .unwrap_or_else(|| "<unknown>".to_string());
    ConstructError::Integer {
        message: format!(
            "value {} out of range for {} BytesInteger({})",
            repr, kind, length
        ),
        path: path.to_string(),
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

    /// 在 GIL 上下文中执行闭包。
    fn with_py<F, R>(f: F) -> R
    where
        F: for<'py> FnOnce(Python<'py>) -> R,
    {
        ensure_python();
        Python::with_gil(f)
    }

    // ======================================================================
    // parse — fast-path
    // ======================================================================

    #[test]
    fn parse_unsigned_4bytes_big_endian() {
        with_py(|py| {
            let node = BytesIntegerNode::new(4, false, false);
            let mut stream = ParseStream::new(&[0x00, 0x00, 0x00, 0x13]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let val: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(val, 19);
        });
    }

    #[test]
    fn parse_signed_4bytes_negative_big_endian() {
        with_py(|py| {
            // BytesInteger(4, signed=True).parse(b'\xFF\xFF\xFF\xFF') == -1
            let node = BytesIntegerNode::new(4, true, false);
            let mut stream = ParseStream::new(&[0xFF; 4]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let val: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(val, -1);
        });
    }

    #[test]
    fn parse_swapped_4bytes_little_endian() {
        with_py(|py| {
            // BytesInteger(4, swapped=True).parse(b'\x01\x00\x00\x00') == 1
            let node = BytesIntegerNode::new(4, false, true);
            let mut stream = ParseStream::new(&[0x01, 0x00, 0x00, 0x00]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let val: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(val, 1);
        });
    }

    // ======================================================================
    // parse — Int24 矩阵（fast-path 3 字节边界，关键 parity）
    // ======================================================================

    #[test]
    fn parse_int24ub_basic() {
        with_py(|py| {
            // Int24ub.parse(b'\x00\x00\x01') == 1
            let node = BytesIntegerNode::new(3, false, false);
            let mut stream = ParseStream::new(&[0x00, 0x00, 0x01]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let val: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(val, 1);
        });
    }

    #[test]
    fn parse_int24ub_max_unsigned() {
        with_py(|py| {
            // Int24ub.parse(b'\xFF\xFF\xFF') == 16777215
            let node = BytesIntegerNode::new(3, false, false);
            let mut stream = ParseStream::new(&[0xFF; 3]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let val: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(val, 16777215);
        });
    }

    #[test]
    fn parse_int24sb_minus_one_sign_extension() {
        with_py(|py| {
            // Int24sb.parse(b'\xFF\xFF\xFF') == -1（符号扩展）
            let node = BytesIntegerNode::new(3, true, false);
            let mut stream = ParseStream::new(&[0xFF; 3]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let val: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(val, -1);
        });
    }

    #[test]
    fn parse_int24sb_min_signed() {
        with_py(|py| {
            // Int24sb.parse(b'\x80\x00\x00') == -8388608（最小负数）
            let node = BytesIntegerNode::new(3, true, false);
            let mut stream = ParseStream::new(&[0x80, 0x00, 0x00]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let val: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(val, -8388608);
        });
    }

    #[test]
    fn parse_int24ul_little_endian() {
        with_py(|py| {
            // Int24ul.parse(b'\x01\x00\x00') == 1
            let node = BytesIntegerNode::new(3, false, true);
            let mut stream = ParseStream::new(&[0x01, 0x00, 0x00]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let val: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(val, 1);
        });
    }

    // ======================================================================
    // parse — u128 fast-path（9-16 字节，6.1-fix 新增）
    // ======================================================================

    #[test]
    fn parse_u128_16bytes_zero_fast_path() {
        with_py(|py| {
            // BytesInteger(16).parse(b'\x00' * 16) == 0（u128 fast-path）
            let node = BytesIntegerNode::new(16, false, false);
            let mut stream = ParseStream::new(&[0x00; 16]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let val: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(val, 0);
        });
    }

    #[test]
    fn parse_u128_16bytes_unsigned_big_endian() {
        with_py(|py| {
            // BytesInteger(16).parse(b'\x00'*15 + b'\x2A') == 42（big endian）
            let node = BytesIntegerNode::new(16, false, false);
            let mut data = [0u8; 16];
            data[15] = 0x2A;
            let mut stream = ParseStream::new(&data);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // 42 在 i64 范围内，extract i64 验证
            let val: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(val, 42);
        });
    }

    #[test]
    fn parse_u128_16bytes_unsigned_little_endian() {
        with_py(|py| {
            // BytesInteger(16, swapped=True).parse(b'\x2A' + b'\x00'*15) == 42（little endian）
            let node = BytesIntegerNode::new(16, false, true);
            let mut data = [0u8; 16];
            data[0] = 0x2A;
            let mut stream = ParseStream::new(&data);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let val: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(val, 42);
        });
    }

    #[test]
    fn parse_u128_16bytes_signed_minus_one() {
        with_py(|py| {
            // BytesInteger(16, signed=True).parse(b'\xFF'*16) == -1（符号扩展）
            let node = BytesIntegerNode::new(16, true, false);
            let mut stream = ParseStream::new(&[0xFF; 16]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let val: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(val, -1);
        });
    }

    #[test]
    fn parse_u128_16bytes_unsigned_max_u128() {
        with_py(|py| {
            // BytesInteger(16).parse(b'\xFF'*16) == 2**128 - 1（u128 上界）
            // 结果超出 i64，需用 Python 比较表达式验证
            let node = BytesIntegerNode::new(16, false, false);
            let mut stream = ParseStream::new(&[0xFF; 16]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // 直接与 Python 构造的期望值比较（PyAny ==）
            let expected = py.eval_bound("2**128 - 1", None, None).expect("eval");
            assert!(result.bind(py).eq(&expected).expect("eq comparison"));
        });
    }

    #[test]
    fn parse_u128_9bytes_signed_min_negative() {
        with_py(|py| {
            // BytesInteger(9, signed=True).parse(b'\x80' + b'\x00'*8) == -2**71
            // 9 字节 signed 最小值，符号扩展后经 from_be_bytes 得 -2**71
            let node = BytesIntegerNode::new(9, true, false);
            let mut data = [0u8; 9];
            data[0] = 0x80;
            let mut stream = ParseStream::new(&data);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // -2**71 超出 i64 范围（i64::MIN = -2**63），用 Python 比较
            let expected = py.eval_bound("-(2**71)", None, None).expect("eval");
            assert!(result.bind(py).eq(&expected).expect("eq comparison"));
        });
    }

    #[test]
    fn parse_u128_9bytes_unsigned_little_endian() {
        with_py(|py| {
            // BytesInteger(9, swapped=True).parse(b'\x01' + b'\x00'*8) == 1
            let node = BytesIntegerNode::new(9, false, true);
            let mut data = [0u8; 9];
            data[0] = 0x01;
            let mut stream = ParseStream::new(&data);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let val: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(val, 1);
        });
    }

    // ======================================================================
    // parse — slow-path（> 16 字节，6.1-fix 边界调整）
    // ======================================================================

    #[test]
    fn parse_bigint_17bytes_zero_slow_path() {
        with_py(|py| {
            // BytesInteger(17).parse(b'\x00' * 17) == 0（>16 字节走 Python slow-path）
            let node = BytesIntegerNode::new(17, false, false);
            let mut stream = ParseStream::new(&[0x00; 17]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let val: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(val, 0);
        });
    }

    // ======================================================================
    // parse — 错误路径
    // ======================================================================

    #[test]
    fn parse_length_zero_returns_integer_error() {
        with_py(|py| {
            let node = BytesIntegerNode::new(0, false, false);
            let mut stream = ParseStream::new(&[]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Integer { .. }));
        });
    }

    // ======================================================================
    // build — fast-path
    // ======================================================================

    #[test]
    fn build_unsigned_4bytes_basic() {
        with_py(|py| {
            let node = BytesIntegerNode::new(4, false, false);
            let obj = py.eval_bound("19", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x00, 0x00, 0x00, 0x13]);
        });
    }

    #[test]
    fn build_signed_4bytes_negative() {
        with_py(|py| {
            let node = BytesIntegerNode::new(4, true, false);
            let obj = py.eval_bound("-1", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0xFF; 4]);
        });
    }

    #[test]
    fn build_int24ub_max_value() {
        with_py(|py| {
            let node = BytesIntegerNode::new(3, false, false);
            let obj = py.eval_bound("16777215", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0xFF, 0xFF, 0xFF]);
        });
    }

    #[test]
    fn build_int24ub_overflow_returns_integer_error() {
        with_py(|py| {
            // Int24ub.build(16777216) → IntegerError（超 3 字节无符号范围）
            let node = BytesIntegerNode::new(3, false, false);
            let obj = py.eval_bound("16777216", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Integer { .. }));
        });
    }

    #[test]
    fn build_unsigned_negative_returns_integer_error() {
        with_py(|py| {
            // Int24ub.build(-1) → IntegerError（无符号不接受负数）
            let node = BytesIntegerNode::new(3, false, false);
            let obj = py.eval_bound("-1", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Integer { .. }));
        });
    }

    #[test]
    fn build_non_integer_returns_integer_error() {
        with_py(|py| {
            // BytesInteger(4).build("not int") → IntegerError
            let node = BytesIntegerNode::new(4, false, false);
            let obj = py.eval_bound("'not int'", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Integer { .. }));
        });
    }

    #[test]
    fn build_length_zero_returns_integer_error() {
        with_py(|py| {
            let node = BytesIntegerNode::new(0, false, false);
            let obj = py.eval_bound("1", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Integer { .. }));
        });
    }

    // ======================================================================
    // build — u128 fast-path（9-16 字节，6.1-fix 新增）
    // ======================================================================

    #[test]
    fn build_u128_16bytes_2pow64_fast_path() {
        with_py(|py| {
            // BytesInteger(16).build(2**64) → 16 字节（u128 fast-path）
            // 2^64 在 16 字节 big endian 中：bit 64 → byte index 7（16-8-1=7）
            // 实测：(2**64).to_bytes(16, 'big') == b'\x00'*7 + b'\x01' + b'\x00'*8
            let node = BytesIntegerNode::new(16, false, false);
            let obj = py.eval_bound("2**64", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let mut expected = [0u8; 16];
            expected[7] = 0x01;
            assert_eq!(stream.as_bytes(), &expected[..]);
        });
    }

    #[test]
    fn build_u128_16bytes_max_unsigned() {
        with_py(|py| {
            // BytesInteger(16).build(2**128 - 1) → b'\xFF'*16（u128 上界）
            let node = BytesIntegerNode::new(16, false, false);
            let obj = py.eval_bound("2**128 - 1", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0xFF; 16]);
        });
    }

    #[test]
    fn build_u128_16bytes_signed_negative() {
        with_py(|py| {
            // BytesInteger(16, signed=True).build(-1) → b'\xFF'*16（two's complement）
            let node = BytesIntegerNode::new(16, true, false);
            let obj = py.eval_bound("-1", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0xFF; 16]);
        });
    }

    #[test]
    fn build_u128_16bytes_little_endian() {
        with_py(|py| {
            // BytesInteger(16, swapped=True).build(42) → b'\x2A' + b'\x00'*15
            let node = BytesIntegerNode::new(16, false, true);
            let obj = py.eval_bound("42", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let mut expected = [0u8; 16];
            expected[0] = 0x2A;
            assert_eq!(stream.as_bytes(), &expected[..]);
        });
    }

    #[test]
    fn build_u128_9bytes_signed_min() {
        with_py(|py| {
            // BytesInteger(9, signed=True).build(-2**71) → b'\x80' + b'\x00'*8
            // -2**71 是 9 字节 signed 最小值，72 位 two's complement = 2^71 = 0x80_00...00
            let node = BytesIntegerNode::new(9, true, false);
            let obj = py.eval_bound("-(2**71)", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let mut expected = [0u8; 9];
            expected[0] = 0x80;
            assert_eq!(stream.as_bytes(), &expected[..]);
        });
    }

    #[test]
    fn build_u128_9bytes_unsigned_overflow_returns_integer_error() {
        with_py(|py| {
            // BytesInteger(9).build(2**72) → IntegerError（超 9 字节无符号上界 2**72 - 1）
            // 走 check_range_u128，checked_shl(72) 得 max=2**72-1，2**72 超限报错
            let node = BytesIntegerNode::new(9, false, false);
            let obj = py.eval_bound("2**72", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Integer { .. }));
        });
    }

    #[test]
    fn build_u128_9bytes_signed_overflow_returns_integer_error() {
        with_py(|py| {
            // BytesInteger(9, signed=True).build(2**71) → IntegerError
            // 9 字节 signed 上界 = 2**71 - 1，2**71 超限报错
            let node = BytesIntegerNode::new(9, true, false);
            let obj = py.eval_bound("2**71", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Integer { .. }));
        });
    }

    #[test]
    fn build_u128_16bytes_overflow_returns_integer_error() {
        with_py(|py| {
            // BytesInteger(16).build(2**128) → IntegerError
            // 2**128 超出 u128 范围，pyo3 extract::<u128> 失败 → IntegerError
            let node = BytesIntegerNode::new(16, false, false);
            let obj = py.eval_bound("2**128", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Integer { .. }));
        });
    }

    // ======================================================================
    // build — slow-path（> 16 字节，6.1-fix 边界调整）
    // ======================================================================

    #[test]
    fn build_bigint_17bytes_slow_path() {
        with_py(|py| {
            // BytesInteger(17).build(2**128) → 17 字节（>16 字节走 Python slow-path）
            // 2^128 在 17 字节 big endian 中：bit 128 → byte index 0
            // 实测：(2**128).to_bytes(17, 'big') == b'\x01' + b'\x00'*16
            let node = BytesIntegerNode::new(17, false, false);
            let obj = py.eval_bound("2**128", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let mut expected = [0u8; 17];
            expected[0] = 0x01;
            assert_eq!(stream.as_bytes(), &expected[..]);
        });
    }

    // ======================================================================
    // 往返一致性
    // ======================================================================

    #[test]
    fn round_trip_int24ub() {
        with_py(|py| {
            let node = BytesIntegerNode::new(3, false, false);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            let obj = py.eval_bound("1234567", None, None).expect("eval");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();

            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            assert_eq!(result.bind(py).extract::<i64>().unwrap(), 1234567);
        });
    }

    #[test]
    fn round_trip_int24sb_negative() {
        with_py(|py| {
            let node = BytesIntegerNode::new(3, true, false);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            let obj = py.eval_bound("-123456", None, None).expect("eval");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();

            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            assert_eq!(result.bind(py).extract::<i64>().unwrap(), -123456);
        });
    }

    #[test]
    fn round_trip_u128_16bytes_large_value() {
        // 16 字节往返一致性（u128 fast-path，超 i64 大数用 Python 比较）
        with_py(|py| {
            let node = BytesIntegerNode::new(16, false, false);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            // 2**100 + 12345（超 i64 但远小于 u128 上界）
            let obj = py.eval_bound("2**100 + 12345", None, None).expect("eval");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();

            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            assert!(result.bind(py).eq(&obj).expect("eq comparison"));
        });
    }

    #[test]
    fn round_trip_u128_16bytes_signed_negative() {
        // 16 字节有符号往返（符号扩展对称性）
        with_py(|py| {
            let node = BytesIntegerNode::new(16, true, false);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            // -(2**70 + 999)（超 i64 负数，用 Python 比较）
            let obj = py.eval_bound("-(2**70 + 999)", None, None).expect("eval");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();

            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            assert!(result.bind(py).eq(&obj).expect("eq comparison"));
        });
    }

    #[test]
    fn round_trip_u128_16bytes_little_endian() {
        with_py(|py| {
            let node = BytesIntegerNode::new(16, false, true);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            let obj = py.eval_bound("2**100 + 777", None, None).expect("eval");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();

            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            assert!(result.bind(py).eq(&obj).expect("eq comparison"));
        });
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_returns_length() {
        with_py(|py| {
            let ctx = Context::new_root(py).expect("ctx");
            assert_eq!(
                BytesIntegerNode::new(3, false, false).sizeof(&ctx).unwrap(),
                3
            );
            assert_eq!(
                BytesIntegerNode::new(16, true, true).sizeof(&ctx).unwrap(),
                16
            );
        });
    }
}
