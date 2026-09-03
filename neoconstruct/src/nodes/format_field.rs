//! FormatFieldNode：使用 struct 模块格式读写整数与浮点。
//!
//! Python 参考：`construct/construct/core.py` `FormatField`（L1124-1175）。
//!
//! ## 支持范围
//!
//! 16 种整数格式：`u8` / `i8` / `u16` / `i16` / `u32` / `i32` / `u64` / `i64`
//! × 大端 / 小端；另有 6 种 IEEE 754 浮点格式（Float16 / Float32 / Float64 × 双端序）。
//!
//! ## 设计简化
//!
//! 直接在构造时确定 `PythonFormat`（编译时已知），无需延迟解析格式字符串。
//!
//! ## build 值范围校验
//!
//! build 时禁止 `as` 截断：使用 `try_into` 显式校验值范围。例如
//! `Int8ub.build(300)` 返回 `FormatFieldError`（300 超出 u8 范围），
//! `Int8ub.build(-1)` 同样返回 `FormatFieldError`（无符号不接受负数）。

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::conversion::IntoPy;
use pyo3::prelude::*;

// ---------------------------------------------------------------------------
// PythonFormat
// ---------------------------------------------------------------------------

/// 预编译的格式枚举：16 种整数变体（8 种类型 × 2 种字节序）+ 6 种 IEEE 754 浮点变体。
///
/// 每个 `Int*ub`/`Int*sb`/`Int*ul`/`Int*sl`/`Float*l`/`Float*b` 预定义单例对应一个变体。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PythonFormat {
    /// 无符号 1 字节整数，大端（`>B`，对应 `Int8ub`）
    UnsignedInt8Big,
    /// 无符号 1 字节整数，小端（`<B`，对应 `Int8ul`）
    UnsignedInt8Little,
    /// 有符号 1 字节整数，大端（`>b`，对应 `Int8sb`）
    SignedInt8Big,
    /// 有符号 1 字节整数，小端（`<b`，对应 `Int8sl`）
    SignedInt8Little,
    /// 无符号 2 字节整数，大端（`>H`，对应 `Int16ub`）
    UnsignedInt16Big,
    /// 无符号 2 字节整数，小端（`<H`，对应 `Int16ul`）
    UnsignedInt16Little,
    /// 有符号 2 字节整数，大端（`>h`，对应 `Int16sb`）
    SignedInt16Big,
    /// 有符号 2 字节整数，小端（`<h`，对应 `Int16sl`）
    SignedInt16Little,
    /// 无符号 4 字节整数，大端（`>I`，对应 `Int32ub`）
    UnsignedInt32Big,
    /// 无符号 4 字节整数，小端（`<I`，对应 `Int32ul`）
    UnsignedInt32Little,
    /// 有符号 4 字节整数，大端（`>i`，对应 `Int32sb`）
    SignedInt32Big,
    /// 有符号 4 字节整数，小端（`<i`，对应 `Int32sl`）
    SignedInt32Little,
    /// 无符号 8 字节整数，大端（`>Q`，对应 `Int64ub`）
    UnsignedInt64Big,
    /// 无符号 8 字节整数，小端（`<Q`，对应 `Int64ul`）
    UnsignedInt64Little,
    /// 有符号 8 字节整数，大端（`>q`，对应 `Int64sb`）
    SignedInt64Big,
    /// 有符号 8 字节整数，小端（`<q`，对应 `Int64sl`）
    SignedInt64Little,
    // ---- Float 系列 6 个变体（IEEE 754）----
    /// 半精度 IEEE 754，大端（`>e`，对应 `Float16b`）。Float16 用 `half` crate。
    Float16Big,
    /// 半精度 IEEE 754，小端（`<e`，对应 `Float16l`）。
    Float16Little,
    /// 单精度 IEEE 754，大端（`>f`，对应 `Float32b`）。
    Float32Big,
    /// 单精度 IEEE 754，小端（`<f`，对应 `Float32l`）。
    Float32Little,
    /// 双精度 IEEE 754，大端（`>d`，对应 `Float64b`）。
    Float64Big,
    /// 双精度 IEEE 754，小端（`<d`，对应 `Float64l`）。
    Float64Little,
}

impl PythonFormat {
    /// 从 Python struct 模块的 endianity 和 format 字符构造 `PythonFormat`。
    ///
    /// # 参数
    ///
    /// - `endianity`：字节序字符，`'>'`（大端）、`'<'`（小端）、`'='`（本机）
    /// - `format`：格式字符，`'B'`/`'b'`/`'H'`/`'h'`/`'I'`/`'i'`/`'Q'`/`'q'`
    ///   （浮点：`'e'` Float16 / `'f'` Float32 / `'d'` Float64）
    ///
    /// # 返回
    ///
    /// 有效组合返回 `Some(PythonFormat)`，无效字符返回 `None`。
    pub fn from_chars(endianity: char, format: char) -> Option<Self> {
        let big_endian = match endianity {
            '>' => true,
            '<' => false,
            '=' => cfg!(target_endian = "big"),
            _ => return None,
        };
        Some(match (format, big_endian) {
            ('B', true) => Self::UnsignedInt8Big,
            ('B', false) => Self::UnsignedInt8Little,
            ('b', true) => Self::SignedInt8Big,
            ('b', false) => Self::SignedInt8Little,
            ('H', true) => Self::UnsignedInt16Big,
            ('H', false) => Self::UnsignedInt16Little,
            ('h', true) => Self::SignedInt16Big,
            ('h', false) => Self::SignedInt16Little,
            ('I', true) => Self::UnsignedInt32Big,
            ('I', false) => Self::UnsignedInt32Little,
            ('i', true) => Self::SignedInt32Big,
            ('i', false) => Self::SignedInt32Little,
            ('Q', true) => Self::UnsignedInt64Big,
            ('Q', false) => Self::UnsignedInt64Little,
            ('q', true) => Self::SignedInt64Big,
            ('q', false) => Self::SignedInt64Little,
            // Float 系列（'e'/'f'/'d' × 2 endian = 6 变体）
            ('e', true) => Self::Float16Big,
            ('e', false) => Self::Float16Little,
            ('f', true) => Self::Float32Big,
            ('f', false) => Self::Float32Little,
            ('d', true) => Self::Float64Big,
            ('d', false) => Self::Float64Little,
            _ => return None,
        })
    }

    /// 返回此格式的字节长度（1 / 2 / 4 / 8）。
    pub fn byte_length(&self) -> usize {
        match self {
            Self::UnsignedInt8Big
            | Self::UnsignedInt8Little
            | Self::SignedInt8Big
            | Self::SignedInt8Little => 1,
            // Float16 与 Int16 同为 2 字节
            Self::UnsignedInt16Big
            | Self::UnsignedInt16Little
            | Self::SignedInt16Big
            | Self::SignedInt16Little
            | Self::Float16Big
            | Self::Float16Little => 2,
            // Float32 与 Int32 同为 4 字节
            Self::UnsignedInt32Big
            | Self::UnsignedInt32Little
            | Self::SignedInt32Big
            | Self::SignedInt32Little
            | Self::Float32Big
            | Self::Float32Little => 4,
            // Float64 与 Int64 同为 8 字节
            Self::UnsignedInt64Big
            | Self::UnsignedInt64Little
            | Self::SignedInt64Big
            | Self::SignedInt64Little
            | Self::Float64Big
            | Self::Float64Little => 8,
        }
    }

    /// 返回 Python struct 模块等价格式字符串（如 `">B"`、`"<q"`）。
    ///
    /// 用于错误消息，与 Python construct `FormatField` 的错误信息格式对齐。
    pub fn fmtstr(&self) -> &'static str {
        match self {
            Self::UnsignedInt8Big => ">B",
            Self::UnsignedInt8Little => "<B",
            Self::SignedInt8Big => ">b",
            Self::SignedInt8Little => "<b",
            Self::UnsignedInt16Big => ">H",
            Self::UnsignedInt16Little => "<H",
            Self::SignedInt16Big => ">h",
            Self::SignedInt16Little => "<h",
            Self::UnsignedInt32Big => ">I",
            Self::UnsignedInt32Little => "<I",
            Self::SignedInt32Big => ">i",
            Self::SignedInt32Little => "<i",
            Self::UnsignedInt64Big => ">Q",
            Self::UnsignedInt64Little => "<Q",
            Self::SignedInt64Big => ">q",
            Self::SignedInt64Little => "<q",
            // Float 系列 fmtstr
            Self::Float16Big => ">e",
            Self::Float16Little => "<e",
            Self::Float32Big => ">f",
            Self::Float32Little => "<f",
            Self::Float64Big => ">d",
            Self::Float64Little => "<d",
        }
    }

    /// 是否为大端字节序。
    pub fn is_big_endian(&self) -> bool {
        matches!(
            self,
            Self::UnsignedInt8Big
                | Self::SignedInt8Big
                | Self::UnsignedInt16Big
                | Self::SignedInt16Big
                | Self::UnsignedInt32Big
                | Self::SignedInt32Big
                | Self::UnsignedInt64Big
                | Self::SignedInt64Big
        )
    }
}

// ---------------------------------------------------------------------------
// FormatFieldNode
// ---------------------------------------------------------------------------

/// 整数格式字段节点：使用 Rust 原生整数类型读写，替代 Python `struct` 模块。
///
/// 对应 Python construct 的 `FormatField`。每个 `Int*ub`/`Int*sb`/`Int*ul`/`Int*sl`
/// 预定义单例在编译时转换为一个 `FormatFieldNode`。
///
/// # parse 行为
///
/// 1. 从流中读取 N 字节（N = 字节长度）。
/// 2. 用 `from_be_bytes` / `from_le_bytes` 转换为 Rust 整数。
/// 3. 通过 C API 创建 Python `int` 对象，返回 `Py<PyAny>`。
///
/// # build 行为
///
/// 1. 从 Python 对象 `extract` 数值。
/// 2. 使用 `try_into` 显式校验值范围（禁止 `as` 截断）。
/// 3. 按字节序 `to_be_bytes` / `to_le_bytes` 写入流。
///
/// 值超出范围 → `FormatFieldError`（对应 Python construct 的 `struct.error`）。
#[derive(Debug, Clone, Copy)]
pub struct FormatFieldNode {
    /// 预编译的整数格式。
    format: PythonFormat,
    /// 字节长度（编译期已知，等于 `format.byte_length()`）。
    length: usize,
}

impl FormatFieldNode {
    /// 从 `PythonFormat` 构造 `FormatFieldNode`。
    pub fn new(format: PythonFormat) -> Self {
        Self {
            length: format.byte_length(),
            format,
        }
    }

    /// 从 Python struct 模块的 endianity 和 format 字符构造节点。
    ///
    /// 返回 `None` 表示格式字符无效。
    pub fn from_chars(endianity: char, format: char) -> Option<Self> {
        PythonFormat::from_chars(endianity, format).map(Self::new)
    }

    /// 返回字节长度。
    pub fn length(&self) -> usize {
        self.length
    }

    /// 返回格式枚举。
    pub fn format(&self) -> PythonFormat {
        self.format
    }
}

// ---------------------------------------------------------------------------
// Construct impl
// ---------------------------------------------------------------------------

impl super::Construct for FormatFieldNode {
    fn parse(
        &self,
        py: Python<'_>,
        stream: &mut ParseStream<'_>,
        _ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        match self.format {
            // 8-bit
            PythonFormat::UnsignedInt8Big => {
                let arr = read_array::<1>(stream, path)?;
                Ok(u8::from_be_bytes(arr).into_py(py))
            }
            PythonFormat::UnsignedInt8Little => {
                let arr = read_array::<1>(stream, path)?;
                Ok(u8::from_le_bytes(arr).into_py(py))
            }
            PythonFormat::SignedInt8Big => {
                let arr = read_array::<1>(stream, path)?;
                Ok(i8::from_be_bytes(arr).into_py(py))
            }
            PythonFormat::SignedInt8Little => {
                let arr = read_array::<1>(stream, path)?;
                Ok(i8::from_le_bytes(arr).into_py(py))
            }
            // 16-bit
            PythonFormat::UnsignedInt16Big => {
                let arr = read_array::<2>(stream, path)?;
                Ok(u16::from_be_bytes(arr).into_py(py))
            }
            PythonFormat::UnsignedInt16Little => {
                let arr = read_array::<2>(stream, path)?;
                Ok(u16::from_le_bytes(arr).into_py(py))
            }
            PythonFormat::SignedInt16Big => {
                let arr = read_array::<2>(stream, path)?;
                Ok(i16::from_be_bytes(arr).into_py(py))
            }
            PythonFormat::SignedInt16Little => {
                let arr = read_array::<2>(stream, path)?;
                Ok(i16::from_le_bytes(arr).into_py(py))
            }
            // 32-bit
            PythonFormat::UnsignedInt32Big => {
                let arr = read_array::<4>(stream, path)?;
                Ok(u32::from_be_bytes(arr).into_py(py))
            }
            PythonFormat::UnsignedInt32Little => {
                let arr = read_array::<4>(stream, path)?;
                Ok(u32::from_le_bytes(arr).into_py(py))
            }
            PythonFormat::SignedInt32Big => {
                let arr = read_array::<4>(stream, path)?;
                Ok(i32::from_be_bytes(arr).into_py(py))
            }
            PythonFormat::SignedInt32Little => {
                let arr = read_array::<4>(stream, path)?;
                Ok(i32::from_le_bytes(arr).into_py(py))
            }
            // 64-bit
            PythonFormat::UnsignedInt64Big => {
                let arr = read_array::<8>(stream, path)?;
                Ok(u64::from_be_bytes(arr).into_py(py))
            }
            PythonFormat::UnsignedInt64Little => {
                let arr = read_array::<8>(stream, path)?;
                Ok(u64::from_le_bytes(arr).into_py(py))
            }
            PythonFormat::SignedInt64Big => {
                let arr = read_array::<8>(stream, path)?;
                Ok(i64::from_be_bytes(arr).into_py(py))
            }
            PythonFormat::SignedInt64Little => {
                let arr = read_array::<8>(stream, path)?;
                Ok(i64::from_le_bytes(arr).into_py(py))
            }
            // ---- Float 系列 parse 分支 ----
            // Float16 → PyFloat：Python 无 f16 类型，half::f16::to_f64 转 f64 后创建 PyFloat，
            // 与 CPython struct 模块 'e' 格式行为一致。
            PythonFormat::Float16Big => {
                let arr = read_array::<2>(stream, path)?;
                let f16_val = half::f16::from_be_bytes(arr);
                Ok(f16_val.to_f64().into_py(py))
            }
            PythonFormat::Float16Little => {
                let arr = read_array::<2>(stream, path)?;
                let f16_val = half::f16::from_le_bytes(arr);
                Ok(f16_val.to_f64().into_py(py))
            }
            PythonFormat::Float32Big => {
                let arr = read_array::<4>(stream, path)?;
                Ok(f32::from_be_bytes(arr).into_py(py))
            }
            PythonFormat::Float32Little => {
                let arr = read_array::<4>(stream, path)?;
                Ok(f32::from_le_bytes(arr).into_py(py))
            }
            PythonFormat::Float64Big => {
                let arr = read_array::<8>(stream, path)?;
                Ok(f64::from_be_bytes(arr).into_py(py))
            }
            PythonFormat::Float64Little => {
                let arr = read_array::<8>(stream, path)?;
                Ok(f64::from_le_bytes(arr).into_py(py))
            }
        }
    }

    fn build(
        &self,
        _py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        _ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        let fmtstr = self.format.fmtstr();
        match self.format {
            // 8/16/32-bit: 先 extract 为 i64，再 try_into 显式收窄
            PythonFormat::UnsignedInt8Big => {
                let val: u8 = build_small_int::<u8>(obj, fmtstr, path)?;
                stream.write(&val.to_be_bytes());
            }
            PythonFormat::UnsignedInt8Little => {
                let val: u8 = build_small_int::<u8>(obj, fmtstr, path)?;
                stream.write(&val.to_le_bytes());
            }
            PythonFormat::SignedInt8Big => {
                let val: i8 = build_small_int::<i8>(obj, fmtstr, path)?;
                stream.write(&val.to_be_bytes());
            }
            PythonFormat::SignedInt8Little => {
                let val: i8 = build_small_int::<i8>(obj, fmtstr, path)?;
                stream.write(&val.to_le_bytes());
            }
            PythonFormat::UnsignedInt16Big => {
                let val: u16 = build_small_int::<u16>(obj, fmtstr, path)?;
                stream.write(&val.to_be_bytes());
            }
            PythonFormat::UnsignedInt16Little => {
                let val: u16 = build_small_int::<u16>(obj, fmtstr, path)?;
                stream.write(&val.to_le_bytes());
            }
            PythonFormat::SignedInt16Big => {
                let val: i16 = build_small_int::<i16>(obj, fmtstr, path)?;
                stream.write(&val.to_be_bytes());
            }
            PythonFormat::SignedInt16Little => {
                let val: i16 = build_small_int::<i16>(obj, fmtstr, path)?;
                stream.write(&val.to_le_bytes());
            }
            PythonFormat::UnsignedInt32Big => {
                let val: u32 = build_small_int::<u32>(obj, fmtstr, path)?;
                stream.write(&val.to_be_bytes());
            }
            PythonFormat::UnsignedInt32Little => {
                let val: u32 = build_small_int::<u32>(obj, fmtstr, path)?;
                stream.write(&val.to_le_bytes());
            }
            PythonFormat::SignedInt32Big => {
                let val: i32 = build_small_int::<i32>(obj, fmtstr, path)?;
                stream.write(&val.to_be_bytes());
            }
            PythonFormat::SignedInt32Little => {
                let val: i32 = build_small_int::<i32>(obj, fmtstr, path)?;
                stream.write(&val.to_le_bytes());
            }
            // 64-bit: 直接 extract 为目标类型（pyo3 自带范围检查）
            PythonFormat::UnsignedInt64Big => {
                let val: u64 = obj
                    .extract::<u64>()
                    .map_err(|_| make_build_error(fmtstr, obj, path))?;
                stream.write(&val.to_be_bytes());
            }
            PythonFormat::UnsignedInt64Little => {
                let val: u64 = obj
                    .extract::<u64>()
                    .map_err(|_| make_build_error(fmtstr, obj, path))?;
                stream.write(&val.to_le_bytes());
            }
            PythonFormat::SignedInt64Big => {
                let val: i64 = obj
                    .extract::<i64>()
                    .map_err(|_| make_build_error(fmtstr, obj, path))?;
                stream.write(&val.to_be_bytes());
            }
            PythonFormat::SignedInt64Little => {
                let val: i64 = obj
                    .extract::<i64>()
                    .map_err(|_| make_build_error(fmtstr, obj, path))?;
                stream.write(&val.to_le_bytes());
            }
            // ---- Float 系列 build 分支 ----
            //
            // 设计要点：
            // - int→float 兼容：所有 6 个 Float build 分支加 i64/u64 fallback。
            //   Python `struct.pack('>f', 42)` 接受 int，mashumaro `v: float` 注解运行时不强制。
            //   Rust 顺序：extract f32/f64（Python float）→ i64 as f32/f64 → u64 as f32/f64。
            // - Float16 范围检查：超 f16 max（65504）的 finite float手动返回 FormatFieldError，
            //   对齐 `struct.pack('>e', 70000)` OverflowError→construct FormatFieldError。
            //   NaN/Inf 放行（由 from_f64 处理）。
            // - Float32 范围：extract::<f32>() 对超范围值触发 pyo3 OverflowError。
            //   int 输入走 i64/u64 fallback（i64/u64::MAX < f32::MAX，as f32 不溢出）。
            // - Float64 无范围检查：f64 精度足以容纳所有 i64/u64。
            PythonFormat::Float16Big => {
                let val: f64 = extract_float_with_int_fallback(obj, fmtstr, path)?;
                let f16_val = if val.is_finite() && val.abs() > F16_MAX_ABS {
                    return Err(make_build_error(fmtstr, obj, path));
                } else {
                    half::f16::from_f64(val)
                };
                stream.write(&f16_val.to_be_bytes());
            }
            PythonFormat::Float16Little => {
                let val: f64 = extract_float_with_int_fallback(obj, fmtstr, path)?;
                let f16_val = if val.is_finite() && val.abs() > F16_MAX_ABS {
                    return Err(make_build_error(fmtstr, obj, path));
                } else {
                    half::f16::from_f64(val)
                };
                stream.write(&f16_val.to_le_bytes());
            }
            PythonFormat::Float32Big => {
                let val: f32 = extract_f32_with_int_fallback(obj, fmtstr, path)?;
                stream.write(&val.to_be_bytes());
            }
            PythonFormat::Float32Little => {
                let val: f32 = extract_f32_with_int_fallback(obj, fmtstr, path)?;
                stream.write(&val.to_le_bytes());
            }
            PythonFormat::Float64Big => {
                let val: f64 = extract_float_with_int_fallback(obj, fmtstr, path)?;
                stream.write(&val.to_be_bytes());
            }
            PythonFormat::Float64Little => {
                let val: f64 = extract_float_with_int_fallback(obj, fmtstr, path)?;
                stream.write(&val.to_le_bytes());
            }
        }
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        Ok(self.length)
    }
}

// ---------------------------------------------------------------------------
// 辅助函数
// ---------------------------------------------------------------------------

/// 从流中读取恰好 N 字节，转为固定大小数组。
///
/// `stream.read(N)` 已保证返回 N 字节或 Err，所以 `try_into` 正常情况下不会失败。
/// 失败分支仅用于防御性编程（满足禁止 unwrap 的红线）。
fn read_array<const N: usize>(
    stream: &mut ParseStream<'_>,
    path: &Path,
) -> Result<[u8; N], ConstructError> {
    let data = stream.read(N, path)?;
    data.try_into().map_err(|_| ConstructError::Stream {
        message: format!(
            "internal error: stream.read returned {} bytes, expected {}",
            data.len(),
            N
        ),
        path: path.to_string(),
    })
}

/// 构造 build 错误：值类型不匹配或范围超出。
///
/// 错误消息格式与 Python construct `FormatField._build` 对齐：
/// `"struct '>B' error during building, given value 300"`
fn make_build_error(fmtstr: &str, obj: &Bound<'_, PyAny>, path: &Path) -> ConstructError {
    let repr = obj
        .repr()
        .ok()
        .and_then(|r| r.to_str().ok().map(String::from))
        .unwrap_or_else(|| "<unknown>".to_string());
    ConstructError::FormatField {
        message: format!(
            "struct '{}' error during building, given value {}",
            fmtstr, repr
        ),
        path: path.to_string(),
    }
}

/// 对 8/16/32 位整数：extract 为 i64，再 `try_into` 显式收窄到目标类型。
///
/// 禁止 `as u8` 截断：`try_into` 在值超出范围时返回 `Err`，
/// 转为 `FormatFieldError`。
fn build_small_int<T>(
    obj: &Bound<'_, PyAny>,
    fmtstr: &str,
    path: &Path,
) -> Result<T, ConstructError>
where
    T: TryFrom<i64>,
{
    let val: i64 = obj
        .extract::<i64>()
        .map_err(|_| make_build_error(fmtstr, obj, path))?;
    val.try_into()
        .map_err(|_| make_build_error(fmtstr, obj, path))
}

// ---- Float build 辅助函数 ----

/// IEEE 754 half precision 的最大有限正值（65504.0）。
///
/// 用于 Float16 build 的范围检查（对齐 Python `struct.pack('>e', 70000)` OverflowError）。
/// 大于此值的 finite float 手动返回 FormatFieldError。
const F16_MAX_ABS: f64 = 65504.0;

/// 从 Python 对象提取 f64，带 int fallback（Float16/Float64 build 用）。
///
/// Python `struct.pack` 接受 int（自动转 float），mashumaro `v: float`
/// 注解运行时不强制。Rust 顺序：
/// 1. `extract::<f64>()`（Python float）
/// 2. `i64 as f64`（Python int，f64 精度足以容纳所有 i64）
/// 3. `u64 as f64`（Python int，覆盖 > i64::MAX 范围）
///
/// 全部失败 → FormatFieldError（对齐 Python struct.error）。
fn extract_float_with_int_fallback(
    obj: &Bound<'_, PyAny>,
    fmtstr: &str,
    path: &Path,
) -> Result<f64, ConstructError> {
    obj.extract::<f64>()
        .or_else(|_| obj.extract::<i64>().map(|i| i as f64))
        .or_else(|_| obj.extract::<u64>().map(|u| u as f64))
        .map_err(|_| make_build_error(fmtstr, obj, path))
}

/// 从 Python 对象提取 f32，带 int fallback（Float32 build 用）。
///
/// 与 [`extract_float_with_int_fallback`] 平行，但首选项是 `f32`。
/// Python float 超出 f32 范围时（如 `1e40`）`extract::<f32>` 触发 pyo3
/// OverflowError → FormatFieldError（对齐 `struct.pack('>f', 1e40)` OverflowError）。
fn extract_f32_with_int_fallback(
    obj: &Bound<'_, PyAny>,
    fmtstr: &str,
    path: &Path,
) -> Result<f32, ConstructError> {
    obj.extract::<f32>()
        .or_else(|_| obj.extract::<i64>().map(|i| i as f32))
        .or_else(|_| obj.extract::<u64>().map(|u| u as f32))
        .map_err(|_| make_build_error(fmtstr, obj, path))
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
    // PythonFormat
    // ======================================================================

    #[test]
    fn format_from_chars_valid_combinations() {
        assert_eq!(
            PythonFormat::from_chars('>', 'B'),
            Some(PythonFormat::UnsignedInt8Big)
        );
        assert_eq!(
            PythonFormat::from_chars('<', 'b'),
            Some(PythonFormat::SignedInt8Little)
        );
        assert_eq!(
            PythonFormat::from_chars('>', 'Q'),
            Some(PythonFormat::UnsignedInt64Big)
        );
        assert_eq!(
            PythonFormat::from_chars('<', 'q'),
            Some(PythonFormat::SignedInt64Little)
        );
    }

    #[test]
    fn format_from_chars_invalid_returns_none() {
        assert_eq!(PythonFormat::from_chars('>', 'x'), None);
        assert_eq!(PythonFormat::from_chars('!', 'B'), None);
        // 'e'/'f'/'d'（Float16/32/64）合法；其他无效字符应返回 None。
        assert_eq!(PythonFormat::from_chars('>', 'F'), None);
        assert_eq!(PythonFormat::from_chars('>', 'E'), None);
        assert_eq!(PythonFormat::from_chars('>', 'D'), None);
    }

    #[test]
    fn format_byte_length_correct() {
        assert_eq!(PythonFormat::UnsignedInt8Big.byte_length(), 1);
        assert_eq!(PythonFormat::SignedInt8Little.byte_length(), 1);
        assert_eq!(PythonFormat::UnsignedInt16Big.byte_length(), 2);
        assert_eq!(PythonFormat::SignedInt16Little.byte_length(), 2);
        assert_eq!(PythonFormat::UnsignedInt32Big.byte_length(), 4);
        assert_eq!(PythonFormat::SignedInt32Little.byte_length(), 4);
        assert_eq!(PythonFormat::UnsignedInt64Big.byte_length(), 8);
        assert_eq!(PythonFormat::SignedInt64Little.byte_length(), 8);
    }

    #[test]
    fn format_fmtstr_matches_python_struct() {
        assert_eq!(PythonFormat::UnsignedInt8Big.fmtstr(), ">B");
        assert_eq!(PythonFormat::UnsignedInt8Little.fmtstr(), "<B");
        assert_eq!(PythonFormat::SignedInt8Big.fmtstr(), ">b");
        assert_eq!(PythonFormat::SignedInt8Little.fmtstr(), "<b");
        assert_eq!(PythonFormat::UnsignedInt16Big.fmtstr(), ">H");
        assert_eq!(PythonFormat::UnsignedInt16Little.fmtstr(), "<H");
        assert_eq!(PythonFormat::SignedInt16Big.fmtstr(), ">h");
        assert_eq!(PythonFormat::SignedInt16Little.fmtstr(), "<h");
        assert_eq!(PythonFormat::UnsignedInt32Big.fmtstr(), ">I");
        assert_eq!(PythonFormat::UnsignedInt32Little.fmtstr(), "<I");
        assert_eq!(PythonFormat::SignedInt32Big.fmtstr(), ">i");
        assert_eq!(PythonFormat::SignedInt32Little.fmtstr(), "<i");
        assert_eq!(PythonFormat::UnsignedInt64Big.fmtstr(), ">Q");
        assert_eq!(PythonFormat::UnsignedInt64Little.fmtstr(), "<Q");
        assert_eq!(PythonFormat::SignedInt64Big.fmtstr(), ">q");
        assert_eq!(PythonFormat::SignedInt64Little.fmtstr(), "<q");
    }

    #[test]
    fn format_is_big_endian() {
        assert!(PythonFormat::UnsignedInt8Big.is_big_endian());
        assert!(PythonFormat::SignedInt64Big.is_big_endian());
        assert!(!PythonFormat::UnsignedInt8Little.is_big_endian());
        assert!(!PythonFormat::SignedInt64Little.is_big_endian());
    }

    #[test]
    fn node_from_chars() {
        let node = FormatFieldNode::from_chars('>', 'B');
        assert!(node.is_some());
        assert_eq!(node.unwrap().length(), 1);

        let node = FormatFieldNode::from_chars('<', 'q');
        assert!(node.is_some());
        assert_eq!(node.unwrap().length(), 8);

        assert!(FormatFieldNode::from_chars('!', 'B').is_none());
    }

    // ======================================================================
    // parse — 各格式
    // ======================================================================

    #[test]
    fn parse_unsigned_int8_big() {
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::UnsignedInt8Big);
            let mut stream = ParseStream::new(&[0x42]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let val: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(val, 0x42);
        });
    }

    #[test]
    fn parse_signed_int8_big() {
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::SignedInt8Big);
            let mut stream = ParseStream::new(&[0xFF]);
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
    fn parse_unsigned_int16_big_endian() {
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::UnsignedInt16Big);
            let mut stream = ParseStream::new(&[0x01, 0x00]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let val: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(val, 256);
        });
    }

    #[test]
    fn parse_unsigned_int16_little_endian() {
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::UnsignedInt16Little);
            let mut stream = ParseStream::new(&[0x00, 0x01]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let val: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(val, 256);
        });
    }

    #[test]
    fn parse_signed_int16_big_negative() {
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::SignedInt16Big);
            let mut stream = ParseStream::new(&[0xFF, 0xFF]);
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
    fn parse_unsigned_int32_big() {
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::UnsignedInt32Big);
            let mut stream = ParseStream::new(&[0x00, 0x00, 0x01, 0x00]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let val: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(val, 256);
        });
    }

    #[test]
    fn parse_unsigned_int32_little() {
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::UnsignedInt32Little);
            let mut stream = ParseStream::new(&[0x00, 0x01, 0x00, 0x00]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let val: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(val, 256);
        });
    }

    #[test]
    fn parse_unsigned_int64_big() {
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::UnsignedInt64Big);
            let mut stream = ParseStream::new(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let val: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(val, 256);
        });
    }

    #[test]
    fn parse_unsigned_int64_little() {
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::UnsignedInt64Little);
            let mut stream = ParseStream::new(&[0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let val: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(val, 256);
        });
    }

    #[test]
    fn parse_signed_int64_big_negative() {
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::SignedInt64Big);
            let mut stream = ParseStream::new(&[0xFF; 8]);
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
    fn parse_max_unsigned_values() {
        with_py(|py| {
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            let node = FormatFieldNode::new(PythonFormat::UnsignedInt8Big);
            let r = node
                .parse(py, &mut ParseStream::new(&[0xFF]), &mut ctx, &mut path)
                .expect("u8");
            assert_eq!(r.bind(py).extract::<i64>().unwrap(), 255);

            let node = FormatFieldNode::new(PythonFormat::UnsignedInt16Big);
            let r = node
                .parse(
                    py,
                    &mut ParseStream::new(&[0xFF, 0xFF]),
                    &mut ctx,
                    &mut path,
                )
                .expect("u16");
            assert_eq!(r.bind(py).extract::<i64>().unwrap(), 65535);

            let node = FormatFieldNode::new(PythonFormat::UnsignedInt32Big);
            let r = node
                .parse(
                    py,
                    &mut ParseStream::new(&[0xFF, 0xFF, 0xFF, 0xFF]),
                    &mut ctx,
                    &mut path,
                )
                .expect("u32");
            assert_eq!(r.bind(py).extract::<i64>().unwrap(), 4294967295);
        });
    }

    #[test]
    fn parse_insufficient_bytes_returns_stream_error() {
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::UnsignedInt32Big);
            let mut stream = ParseStream::new(&[0x01, 0x02]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Stream { message, .. } => {
                    assert!(message.contains("expected 4"), "got: {}", message);
                    assert!(message.contains("found 2"), "got: {}", message);
                }
                other => panic!("expected Stream error, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // build — 各格式
    // ======================================================================

    #[test]
    fn build_unsigned_int8_big() {
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::UnsignedInt8Big);
            let obj = py.eval_bound("42", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[42]);
        });
    }

    #[test]
    fn build_unsigned_int16_big() {
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::UnsignedInt16Big);
            let obj = py.eval_bound("256", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x01, 0x00]);
        });
    }

    #[test]
    fn build_unsigned_int16_little() {
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::UnsignedInt16Little);
            let obj = py.eval_bound("256", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x00, 0x01]);
        });
    }

    #[test]
    fn build_signed_int8_big() {
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::SignedInt8Big);
            let obj = py.eval_bound("-1", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0xFF]);
        });
    }

    #[test]
    fn build_unsigned_int32_big() {
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::UnsignedInt32Big);
            let obj = py.eval_bound("16909060", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x01, 0x02, 0x03, 0x04]);
        });
    }

    #[test]
    fn build_signed_int32_little_negative() {
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::SignedInt32Little);
            let obj = py.eval_bound("-1", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0xFF, 0xFF, 0xFF, 0xFF]);
        });
    }

    #[test]
    fn build_unsigned_int64_big() {
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::UnsignedInt64Big);
            let obj = py.eval_bound("256", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(
                stream.as_bytes(),
                &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00]
            );
        });
    }

    #[test]
    fn build_signed_int64_little_negative() {
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::SignedInt64Little);
            let obj = py.eval_bound("-1", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0xFF; 8]);
        });
    }

    // ======================================================================
    // build — 值范围校验（禁止 as 截断）
    // ======================================================================

    #[test]
    fn build_u8_value_too_large_returns_format_field_error() {
        // Int8ub.build(300) → FormatFieldError（300 > 255）
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::UnsignedInt8Big);
            let obj = py.eval_bound("300", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::FormatField { message, path } => {
                    assert!(message.contains("'>B'"), "got: {}", message);
                    assert!(message.contains("300"), "got: {}", message);
                    assert_eq!(path, "root");
                }
                other => panic!("expected FormatField error, got {:?}", other),
            }
        });
    }

    #[test]
    fn build_u8_negative_returns_format_field_error() {
        // Int8ub.build(-1) → FormatFieldError（无符号不接受负数）
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::UnsignedInt8Big);
            let obj = py.eval_bound("-1", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::FormatField { .. }));
        });
    }

    #[test]
    fn build_i8_value_too_large_returns_error() {
        // Int8sb.build(200) → FormatFieldError（200 > 127）
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::SignedInt8Big);
            let obj = py.eval_bound("200", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::FormatField { .. }));
        });
    }

    #[test]
    fn build_i8_value_too_small_returns_error() {
        // Int8sb.build(-200) → FormatFieldError（-200 < -128）
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::SignedInt8Big);
            let obj = py.eval_bound("-200", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::FormatField { .. }));
        });
    }

    #[test]
    fn build_u16_value_too_large_returns_error() {
        // Int16ub.build(70000) → FormatFieldError（70000 > 65535）
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::UnsignedInt16Big);
            let obj = py.eval_bound("70000", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::FormatField { .. }));
        });
    }

    #[test]
    fn build_non_integer_returns_format_field_error() {
        // Int8ub.build("hello") → FormatFieldError
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::UnsignedInt8Big);
            let obj = py.eval_bound("'hello'", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::FormatField { .. }));
        });
    }

    #[test]
    fn build_u64_max_value_succeeds() {
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::UnsignedInt64Big);
            let obj = py
                .eval_bound("18446744073709551615", None, None)
                .expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0xFF; 8]);
        });
    }

    #[test]
    fn build_u64_too_large_returns_error() {
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::UnsignedInt64Big);
            let obj = py.eval_bound("2**64", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
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
    fn round_trip_unsigned_int8_big() {
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::UnsignedInt8Big);
            let mut ctx = Context::new_root(py).expect("ctx");
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
            assert_eq!(result.bind(py).extract::<i64>().unwrap(), 200);
        });
    }

    #[test]
    fn round_trip_signed_int32_little() {
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::SignedInt32Little);
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
    fn round_trip_unsigned_int64_big_large_value() {
        with_py(|py| {
            let node = FormatFieldNode::new(PythonFormat::UnsignedInt64Big);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            let obj = py
                .eval_bound("12345678901234567890", None, None)
                .expect("eval");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();

            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let result_str = format!("{}", result.bind(py).str().expect("str"));
            assert_eq!(result_str, "12345678901234567890");
        });
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_returns_byte_length() {
        with_py(|py| {
            let ctx = Context::new_root(py).expect("ctx");
            assert_eq!(
                FormatFieldNode::new(PythonFormat::UnsignedInt8Big)
                    .sizeof(&ctx)
                    .unwrap(),
                1
            );
            assert_eq!(
                FormatFieldNode::new(PythonFormat::SignedInt16Little)
                    .sizeof(&ctx)
                    .unwrap(),
                2
            );
            assert_eq!(
                FormatFieldNode::new(PythonFormat::UnsignedInt32Big)
                    .sizeof(&ctx)
                    .unwrap(),
                4
            );
            assert_eq!(
                FormatFieldNode::new(PythonFormat::SignedInt64Little)
                    .sizeof(&ctx)
                    .unwrap(),
                8
            );
        });
    }
}
