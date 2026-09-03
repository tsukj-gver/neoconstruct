//! VarIntNode：LEB128 无符号变长整数（Google Protocol Buffers 编码）。
//!
//! Python 参考：`construct/construct/core.py:1601-1647`（VarInt 类）。
//!
//! ## 编码规则
//!
//! 每字节低 7 位是有效数据，最高位（MSB）=1 表示后续还有字节，MSB=0 表示终止。
//! 小整数（0-127）仅需 1 字节，大整数按 7 位分组扩展。
//!
//! ## 双路径
//!
//! - **fast-path（u64 范围内）**：i64/u64 extract + `varint_encode_u64` 编码，零 Python 调用
//! - **slow-path（> 2^64）**：调用缓存的 Python `_varint_encode` 函数
//!
//! ## 限制
//!
//! - 仅支持非负整数（build 负数返回 `IntegerError`，与 Python 一致）
//! - sizeof 永远返回 `Err(SizeofError)`（变长，无法预知）

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::conversion::IntoPy;
use pyo3::prelude::*;

/// u64 经 LEB128 编码后最多占 10 字节（70 位，ceil(64/7) = 10）。
const U64_VARINT_MAX_BYTES: usize = 10;

/// LEB128 无符号变长整数节点（对应 Python construct `VarInt`）。
///
/// 单例语义（Python `VarInt` 是 `@singleton` class），Rust 侧所有实例等价
/// （`VarIntNode::new()` 任意次返回等价节点）。
#[derive(Debug, Clone, Copy)]
pub struct VarIntNode;

impl VarIntNode {
    /// 创建 `VarIntNode`。
    pub fn new() -> Self {
        Self
    }
}

impl Default for VarIntNode {
    fn default() -> Self {
        Self::new()
    }
}

impl super::Construct for VarIntNode {
    fn parse(
        &self,
        py: Python<'_>,
        stream: &mut ParseStream<'_>,
        _ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // LEB128 解码循环（对齐 Python core.py:1608-1618 顺序）
        let mut result: u64 = 0;
        let mut shift: u32 = 0;
        loop {
            let byte_chunk = stream.read(1, path)?;
            let b = byte_chunk[0];
            result |= ((b & 0x7F) as u64) << shift;
            if b & 0x80 == 0 {
                break;
            }
            shift += 7;
            // 防御：u64 最多 10 字节（70 位），超过则溢出（对齐 Python 无限精度差异）
            if shift >= 64 {
                return Err(ConstructError::Integer {
                    message: "VarInt overflow: exceeds 64 bits".into(),
                    path: path.to_string(),
                });
            }
        }
        Ok(result.into_py(py))
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        _ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        // Python VarInt._build 顺序（core.py:1632-1636）：
        //   1. not isinstance(obj, int) → IntegerError "value {obj} is not an integer"
        //   2. obj < 0                  → IntegerError "VarInt cannot build from negative number {obj}"
        //   3. LEB128 编码（任意大小，无上限）
        // Rust 分三段处理（顺序对齐 Python 语义）：
        //   a. extract i64 成功 → 负数检查 → fast-path（v as u64，覆盖 i64 非负范围）
        //   b. extract i64 失败但 extract u64 成功 → fast-path（i64::MAX < v <= u64::MAX）
        //   c. 两者皆失败 → 若 obj 是 Python int 走大整数 slow-path，否则 IntegerError
        if let Ok(v) = obj.extract::<i64>() {
            if v < 0 {
                return Err(ConstructError::Integer {
                    message: format!("VarInt cannot build from negative number {}", v),
                    path: path.to_string(),
                });
            }
            let mut buf = [0u8; U64_VARINT_MAX_BYTES];
            let len = varint_encode_u64(v as u64, &mut buf);
            stream.write(&buf[..len]);
            return Ok(());
        }
        if let Ok(v) = obj.extract::<u64>() {
            let mut buf = [0u8; U64_VARINT_MAX_BYTES];
            let len = varint_encode_u64(v, &mut buf);
            stream.write(&buf[..len]);
            return Ok(());
        }
        if obj.is_instance_of::<pyo3::types::PyLong>() {
            // Python int 但 extract u64/i64 失败 → 大整数（> 2^64）走 slow-path
            return build_varint_bigint(py, obj, stream, path);
        }
        // 非 Python int（str/float/list 等）→ 与 Python "value {obj} is not an integer" 对齐
        Err(ConstructError::Integer {
            message: format!("value {} is not an integer", obj),
            path: path.to_string(),
        })
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // 变长字段，无法预知大小，返回 Err(SizeofError)。
        // error.rs 无 Sizeof 变体，项目惯例（GreedyRange / PrefixedArray）
        // 用 ConstructError::Generic。Python 端 SizeofError 是 ConstructError 子类，
        // 此处 Generic 经 select_exception_class 映射到 GenericConstructError，
        // Python 用户面 `except ConstructError` 仍可捕获（SizeofError 同为子类）。
        Err(ConstructError::Generic {
            message: "VarInt has variable size".into(),
            path: String::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// 辅助函数
// ---------------------------------------------------------------------------

/// LEB128 fast-path helper：编码 u64 到给定 10 字节缓冲区，返回写入字节数。
///
/// VarIntNode 与 ZigZagNode（经 ZigZag 正变换后）共用此 helper，
/// 避免两处重复字节编解码逻辑。
pub(crate) fn varint_encode_u64(mut x: u64, buf: &mut [u8; U64_VARINT_MAX_BYTES]) -> usize {
    let mut len = 0;
    while x > 0x7F {
        buf[len] = 0x80 | (x as u8 & 0x7F);
        x >>= 7;
        len += 1;
    }
    buf[len] = x as u8;
    len + 1
}

/// 缓存编译后的 VarInt 大整数编码函数（Python 层 `_varint_encode`）。
///
/// 首次调用编译并缓存函数引用，后续 slow-path 仅 1 次 `call1` 跨 FFI
/// （与 BytesInteger slow-path 同模式，与 error.rs 的 `EXCEPTIONS` 异常类缓存同模式）。
static VARINT_ENCODER: pyo3::sync::GILOnceCell<Py<PyAny>> = pyo3::sync::GILOnceCell::new();

/// 获取（必要时编译并缓存）VarInt 大整数编码函数。
///
/// 幂等：模块初始化后后续调用直接返回缓存的 `Py<PyAny>` 引用。
fn get_varint_encoder(py: Python<'_>) -> PyResult<Py<PyAny>> {
    let cached = VARINT_ENCODER.get_or_try_init(py, || {
        // _varint_encode 与 Python VarInt._build（core.py:1637-1643）逐行等价：
        // 每 7 位 + MSB 续位的 LEB128 编码。
        let code = r#"
def _varint_encode(x):
    B = bytearray()
    while x > 0b01111111:
        B.append(0b10000000 | (x & 0b01111111))
        x >>= 7
    B.append(x)
    return bytes(B)
"#;
        let locals = pyo3::types::PyDict::new_bound(py);
        py.run_bound(code, None, Some(&locals))?;
        let encoder = locals.get_item("_varint_encode")?.ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err("_varint_encode not found after run")
        })?;
        encoder.extract::<Py<PyAny>>()
    })?;
    // pyo3 0.22：Py<T> 用 clone_ref(py) 而非 clone()（引用计数操作需 GIL 边界明确）。
    Ok(cached.clone_ref(py))
}

/// VarInt 大整数（> 2^64）slow-path：调用缓存的 Python `_varint_encode` 函数。
///
/// 罕见路径走 Python callable，无 unsafe，无中间类型。
///
/// **不能与 BytesInteger slow-path（`call_method("to_bytes")`）统一**：
/// VarInt 是 LEB128 编码（每 7 位 + MSB 续位），`int.to_bytes(length, 'big')` 是
/// 直接转 N 字节大端，两者编码逻辑完全不同。
fn build_varint_bigint(
    py: Python<'_>,
    obj: &Bound<'_, PyAny>,
    stream: &mut BuildStream,
    path: &Path,
) -> Result<(), ConstructError> {
    let encoder = get_varint_encoder(py).map_err(|e| ConstructError::Integer {
        message: format!("VarInt slow path init failed: {}", e),
        path: path.to_string(),
    })?;
    let bytes_obj: Py<PyAny> = encoder
        .call1(py, (obj,))
        .map_err(|e| ConstructError::Integer {
            message: format!("VarInt slow path encode failed: {}", e),
            path: path.to_string(),
        })?;
    let bytes_ref = bytes_obj
        .bind(py)
        .downcast::<pyo3::types::PyBytes>()
        .map_err(|_| ConstructError::Integer {
            message: "VarInt slow path: expected bytes".into(),
            path: path.to_string(),
        })?;
    stream.write(bytes_ref.as_bytes());
    Ok(())
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

    // ---- parse ----

    #[test]
    fn parse_single_byte() {
        with_py(|py| {
            // VarInt.parse(b'\x01') == 1
            let node = VarIntNode::new();
            let mut stream = ParseStream::new(&[0x01]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert_eq!(result.bind(py).extract::<i64>().unwrap(), 1);
        });
    }

    #[test]
    fn parse_multi_byte_128() {
        with_py(|py| {
            // VarInt.parse(b'\x80\x01') == 128（0x80 续位 + 0x01）
            let node = VarIntNode::new();
            let mut stream = ParseStream::new(&[0x80, 0x01]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert_eq!(result.bind(py).extract::<i64>().unwrap(), 128);
        });
    }

    #[test]
    fn parse_u32_max_5bytes() {
        with_py(|py| {
            // VarInt.parse(b'\xFF\xFF\xFF\xFF\x0F') == 4294967295（u32 最大，5 字节 LEB128）
            // 0x7F | (0x7F<<7) | (0x7F<<14) | (0x7F<<21) | (0x0F<<28) = 4294967295
            let node = VarIntNode::new();
            let mut stream = ParseStream::new(&[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert_eq!(result.bind(py).extract::<i64>().unwrap(), 4294967295);
        });
    }

    #[test]
    fn parse_zero() {
        with_py(|py| {
            let node = VarIntNode::new();
            let mut stream = ParseStream::new(&[0x00]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert_eq!(result.bind(py).extract::<i64>().unwrap(), 0);
        });
    }

    // ---- build ----

    #[test]
    fn build_zero() {
        with_py(|py| {
            // VarInt.build(0) == b'\x00'
            let node = VarIntNode::new();
            let obj = py.eval_bound("0", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x00]);
        });
    }

    #[test]
    fn build_128_multi_byte() {
        with_py(|py| {
            // VarInt.build(128) == b'\x80\x01'
            let node = VarIntNode::new();
            let obj = py.eval_bound("128", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x80, 0x01]);
        });
    }

    #[test]
    fn build_negative_returns_integer_error() {
        with_py(|py| {
            // VarInt.build(-1) → IntegerError（错误信息含 -1）
            let node = VarIntNode::new();
            let obj = py.eval_bound("-1", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Integer { message, .. } => {
                    assert!(message.contains("-1"), "got: {}", message);
                    assert!(message.contains("negative"), "got: {}", message);
                }
                other => panic!("expected Integer error, got {:?}", other),
            }
        });
    }

    #[test]
    fn build_non_integer_returns_integer_error() {
        with_py(|py| {
            // VarInt.build("not int") → IntegerError
            let node = VarIntNode::new();
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
    fn build_bigint_slow_path_2_to_100() {
        with_py(|py| {
            // VarInt.build(2**100) → 15 字节（slow-path）
            let node = VarIntNode::new();
            let obj = py.eval_bound("2**100", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            // 2**100 经 LEB128 编码：100 / 7 = 14.28，向上取整 = 15 字节
            assert_eq!(stream.as_bytes().len(), 15);
        });
    }

    // ---- sizeof ----

    #[test]
    fn sizeof_returns_err() {
        with_py(|py| {
            let ctx = Context::new_root(py).expect("ctx");
            let err = VarIntNode::new().sizeof(&ctx).expect_err("should err");
            // 项目惯例：变长字段 sizeof 返回 Generic（error.rs 无 Sizeof 变体）
            assert!(matches!(err, ConstructError::Generic { .. }));
        });
    }

    // ---- 往返一致性 ----

    #[test]
    fn round_trip_various_values() {
        with_py(|py| {
            let node = VarIntNode::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            for v in [0i64, 1, 127, 128, 16383, 16384, 268435455] {
                let obj = py.eval_bound(&v.to_string(), None, None).expect("eval");
                let mut stream = BuildStream::new();
                node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                    .expect("build");
                let bytes = stream.into_bytes();

                let mut pstream = ParseStream::new(&bytes);
                let result = node
                    .parse(py, &mut pstream, &mut ctx, &mut path)
                    .expect("parse");
                assert_eq!(result.bind(py).extract::<i64>().unwrap(), v);
            }
        });
    }
}
