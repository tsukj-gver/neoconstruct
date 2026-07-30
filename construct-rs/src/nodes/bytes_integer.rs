//! BytesIntegerNode：任意字节长度整数（对应 Python construct `BytesInteger`）。
//!
//! 设计依据：`docs/design/模块设计/模块设计-Primitives收尾.md` §1.5。
//! Python 参考：`construct/construct/core.py:1203-1292`（BytesInteger 类）、
//! `construct/construct/lib/binary.py:38-92`（integer2bytes / bytes2integer 工具）。
//!
//! ## 双路径设计（D-2 决策）
//!
//! - **fast-path（length ≤ 8）**：Rust 原生 u64/i64 + endian 转换，零 Python 调用
//! - **slow-path（length > 8）**：调用 Python `int.from_bytes` / `int.to_bytes`
//!   （CPython 公开稳定 API，自 Python 3.2 起，无 unsafe raw FFI）
//!
//! fast-path 覆盖主场景（协议通常用 Int24=3 字节），slow-path 仅触发于显式
//! `BytesInteger(16+)` 罕见用法（如大整数 / 哈希）。详见设计 §1.5.1 D-2 决策表。
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

/// fast-path 上限：length ≤ 8 走 Rust 原生 u64/i64；> 8 走 Python 慢路径。
const FAST_PATH_MAX_LEN: usize = 8;

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

        if self.length <= FAST_PATH_MAX_LEN {
            // ---- fast-path：Rust 原生 u64/i64 + endian 转换 ----
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

        if self.length <= FAST_PATH_MAX_LEN {
            // ---- fast-path：extract i64/u64 → 字节序转换 → write ----
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
    // parse — slow-path（> 8 字节）
    // ======================================================================

    #[test]
    fn parse_bigint_16bytes_zero_slow_path() {
        with_py(|py| {
            // BytesInteger(16).parse(b'\x00' * 16) == 0（slow-path）
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
    // build — slow-path（> 8 字节）
    // ======================================================================

    #[test]
    fn build_bigint_16bytes_slow_path() {
        with_py(|py| {
            // BytesInteger(16).build(2**64) → 16 字节（slow-path）
            // 2^64 在 16 字节 big endian 中：bit 64 → byte index 7（16-8-1=7）
            // 实测：(2**64).to_bytes(16, 'big') == b'\x00'*7 + b'\x01' + b'\x00'*8
            let node = BytesIntegerNode::new(16, false, false);
            let obj = py.eval_bound("2**64", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let mut expected = vec![0u8; 16];
            expected[7] = 0x01;
            assert_eq!(stream.as_bytes(), &expected[..]);
        });
    }

    #[test]
    fn build_bigint_overflow_returns_integer_error() {
        with_py(|py| {
            // BytesInteger(16).build(2**128) → IntegerError（Python to_bytes OverflowError）
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
