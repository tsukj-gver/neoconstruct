//! ZigZagNode：有符号变长整数（Google Protocol Buffers ZigZag 编码）。
//!
//! 设计依据：`docs/design/模块设计/模块设计-Primitives收尾.md` §1.4.2。
//! Python 参考：`construct/construct/core.py:1651-1686`（ZigZag 类）。
//!
//! ## 编码规则
//!
//! ZigZag 将有符号整数映射为无符号整数后再用 LEB128 编码：
//! - 0 → 0, -1 → 1, 1 → 2, -2 → 3, 2 → 4, ...
//! - 公式：`zz(n) = (n << 1) ^ (n >> 63)`（Rust 算术右移）
//! - 反向：`n = (zz >> 1) ^ -(zz & 1)`
//!
//! ## 设计选择
//!
//! 不通过组合 VarIntNode 实现（避免 Box<Node> 间接），而是直接复用 VarIntNode
//! 的 parse/build 方法（设计 §1.4.2）。ZigZag 仅多一次 XOR + shift，组合 Node
//! 会引入额外 dispatch 开销。

use crate::context::Context;
use crate::error::ConstructError;
use crate::nodes::varint::VarIntNode;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::conversion::IntoPy;
use pyo3::prelude::*;

/// 有符号变长整数节点（对应 Python construct `ZigZag`）。
///
/// 单例语义（Python `ZigZag` 是 `@singleton` class）。
#[derive(Debug, Clone, Copy)]
pub struct ZigZagNode;

impl ZigZagNode {
    /// 创建 `ZigZagNode`。
    pub fn new() -> Self {
        Self
    }
}

impl Default for ZigZagNode {
    fn default() -> Self {
        Self::new()
    }
}

impl super::Construct for ZigZagNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 复用 VarIntNode 解码（得到无符号 zz 值），再做 ZigZag 反变换
        let zz_node = VarIntNode::new();
        let zz_py = zz_node.parse(py, stream, ctx, path)?;
        let zz: u64 = zz_py
            .bind(py)
            .extract()
            .map_err(|_| ConstructError::Integer {
                message: "ZigZag: VarInt decode returned non-u64".into(),
                path: path.to_string(),
            })?;
        // ZigZag 反变换：n = (zz >> 1) ^ -(zz & 1)
        // -(zz & 1) as i64：zz&1 为 0 或 1，取负后为 0 或 -1（all-ones），
        // XOR 等价于"zz 为奇数时翻转所有位"。
        let signed: i64 = ((zz >> 1) as i64) ^ -((zz & 1) as i64);
        Ok(signed.into_py(py))
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        let val: i64 = obj.extract::<i64>().map_err(|_| {
            // Python ZigZag 仅检查 isinstance(obj, int)（core.py:1679），不检查范围
            // （Python int 无限精度，ZigZag 调 VarInt 处理任意大小）。Rust 限 i64
            // （§1.4.2 i64 范围决策）：非 int 或超 i64 都归 IntegerError，与 Python
            // IntegerError 对齐（P1 v2 修正：v1 误用 FormatField 已改）。
            ConstructError::Integer {
                message: format!("value {} is not an integer or out of i64 range", obj),
                path: path.to_string(),
            }
        })?;
        // ZigZag 正变换：zz = (val << 1) ^ (val >> 63)（Rust 算术右移）
        // val >> 63：i64 算术右移，负数为 -1（all-ones），正数为 0。
        // XOR 等价于"负数时翻转所有位"。结果 as u64 提供无符号位模式给 VarInt 编码。
        let zz: u64 = ((val << 1) ^ (val >> 63)) as u64;
        // 复用 VarIntNode build（fast-path + slow-path）
        let zz_obj = zz.into_py(py);
        let zz_node = VarIntNode::new();
        zz_node.build(py, zz_obj.bind(py), stream, ctx, path)
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // 变长字段。与 VarIntNode 同（[设计质疑] 见 varint.rs 注释）。
        Err(ConstructError::Generic {
            message: "ZigZag has variable size".into(),
            path: String::new(),
        })
    }
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
    fn parse_zero() {
        with_py(|py| {
            // ZigZag.parse(b'\x00') == 0（zz(0) = 0）
            let node = ZigZagNode::new();
            let mut stream = ParseStream::new(&[0x00]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert_eq!(result.bind(py).extract::<i64>().unwrap(), 0);
        });
    }

    #[test]
    fn parse_minus_one() {
        with_py(|py| {
            // ZigZag.parse(b'\x01') == -1（zz(-1) = 1）
            let node = ZigZagNode::new();
            let mut stream = ParseStream::new(&[0x01]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert_eq!(result.bind(py).extract::<i64>().unwrap(), -1);
        });
    }

    #[test]
    fn parse_one() {
        with_py(|py| {
            // ZigZag.parse(b'\x02') == 1（zz(1) = 2）
            let node = ZigZagNode::new();
            let mut stream = ParseStream::new(&[0x02]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert_eq!(result.bind(py).extract::<i64>().unwrap(), 1);
        });
    }

    // ---- build ----

    #[test]
    fn build_minus_three() {
        with_py(|py| {
            // ZigZag.build(-3) == b'\x05'（zz(-3) = 5）
            let node = ZigZagNode::new();
            let obj = py.eval_bound("-3", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x05]);
        });
    }

    #[test]
    fn build_three() {
        with_py(|py| {
            // ZigZag.build(3) == b'\x06'（zz(3) = 6）
            let node = ZigZagNode::new();
            let obj = py.eval_bound("3", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x06]);
        });
    }

    #[test]
    fn build_zero() {
        with_py(|py| {
            let node = ZigZagNode::new();
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
    fn build_non_integer_returns_integer_error() {
        with_py(|py| {
            // ZigZag.build("not int") → IntegerError
            let node = ZigZagNode::new();
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

    // ---- sizeof ----

    #[test]
    fn sizeof_returns_err() {
        with_py(|py| {
            let ctx = Context::new_root(py).expect("ctx");
            let err = ZigZagNode::new().sizeof(&ctx).expect_err("should err");
            assert!(matches!(err, ConstructError::Generic { .. }));
        });
    }

    // ---- 往返一致性 ----

    #[test]
    fn round_trip_various_signed_values() {
        with_py(|py| {
            let node = ZigZagNode::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            for v in [0i64, -1, 1, -2, 2, -3, 3, 100, -100, 10000, -10000] {
                let obj = py.eval_bound(&v.to_string(), None, None).expect("eval");
                let mut stream = BuildStream::new();
                node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                    .expect("build");
                let bytes = stream.into_bytes();

                let mut pstream = ParseStream::new(&bytes);
                let result = node
                    .parse(py, &mut pstream, &mut ctx, &mut path)
                    .expect("parse");
                assert_eq!(result.bind(py).extract::<i64>().unwrap(), v, "v={}", v);
            }
        });
    }
}
