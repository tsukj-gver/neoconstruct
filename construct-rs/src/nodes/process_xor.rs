//! ProcessXorNode：XOR 字节变换节点。
//!
//! Python 参考：`construct/construct/core.py` `ProcessXor`（L5357-5421）。
//!
//! ## 行为概述
//!
//! `ProcessXor(padfunc, subcon)`：parse 读至 EOF → XOR pad → 子流 → inner.parse；
//! build 对称（XOR 是对合运算）。
//!
//! ## pad 类型（XorPad）
//!
//! - `Int(u8)`：单字节 int pad（pad==0 时 fast-path，不变换）
//! - `Bytes(Vec<u8>)`：多字节 bytes pad（全零 fast-path）
//! - `Expr(ExprProgram)`：表达式 pad，运行期 eval 得 int/bytes 再变换
//!
//! ## fast-path（对齐 Python L5394/L5397）
//!
//! - pad int == 0：不变换
//! - pad bytes 全零（len <= 64）：不变换

use crate::context::Context;
use crate::error::ConstructError;
use crate::expr::{eval_expr_any, ExprProgram};
use crate::nodes::Node;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

use super::Construct;

/// XOR pad 类型（编译期物化或运行期求值）。
#[derive(Debug)]
pub enum XorPad {
    /// 单字节 int pad（pad==0 时 fast-path，不变换）。
    Int(u8),
    /// 多字节 bytes pad（编译期物化 Py<PyBytes> → Vec<u8>，全零 fast-path）。
    Bytes(Vec<u8>),
    /// 表达式 pad（运行期 eval 得 int 或 bytes）。
    Expr(ExprProgram),
}

/// XOR 字节变换节点：parse 读至 EOF → XOR pad → 子流 → inner.parse。
///
/// 对应 Python construct `ProcessXor(padfunc, subcon)`（core.py L5357）。
/// 详见模块级注释。
#[derive(Debug)]
pub struct ProcessXorNode {
    /// 被包装的子树根。
    inner: Box<Node>,
    /// XOR pad（编译期物化或运行期求值）。
    pad: XorPad,
}

impl ProcessXorNode {
    /// 创建 `ProcessXorNode`。
    pub fn new(inner: Node, pad: XorPad) -> Self {
        Self {
            inner: Box::new(inner),
            pad,
        }
    }

    /// 返回内部子树根节点的引用。
    pub fn inner(&self) -> &Node {
        &self.inner
    }

    /// 返回 pad 引用。
    pub fn pad(&self) -> &XorPad {
        &self.pad
    }
}

impl Construct for ProcessXorNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let offset = stream.tell();
        let raw = stream.data();
        let data = &raw[offset..];
        let transformed = apply_xor(&self.pad, data, ctx, py, path)?;
        let mut sub_stream = ParseStream::new(&transformed);
        self.inner.parse(py, &mut sub_stream, ctx, path)
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        // 1. inner build 到子流
        let capacity = self.inner.sizeof(ctx).unwrap_or(0);
        let mut sub_stream = BuildStream::with_capacity(capacity);
        self.inner.build(py, obj, &mut sub_stream, ctx, path)?;
        let data = sub_stream.into_bytes();
        // 2. apply XOR（XOR 对合，parse/build 同变换）
        let transformed = apply_xor(&self.pad, &data, ctx, py, path)?;
        // 3. 写主流
        stream.write(&transformed);
        Ok(())
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        self.inner.sizeof(ctx)
    }
}

/// 应用 XOR 变换（parse + build 共用，XOR 是对合运算）。
fn apply_xor(
    pad: &XorPad,
    data: &[u8],
    ctx: &Context<'_>,
    py: Python<'_>,
    path: &mut Path,
) -> Result<Vec<u8>, ConstructError> {
    match pad {
        XorPad::Int(0) => {
            // fast-path：pad==0 不变换（对齐 Python L5394）。
            Ok(data.to_vec())
        }
        XorPad::Int(p) => {
            // int：每字节 b ^ pad（Rust 内联，SIMD 友好）。
            Ok(data.iter().map(|&b| b ^ p).collect())
        }
        XorPad::Bytes(pad_bytes) => {
            if pad_bytes.len() <= 64 && pad_bytes.iter().all(|&b| b == 0) {
                // fast-path：全零 bytes 不变换（对齐 Python L5397）。
                Ok(data.to_vec())
            } else {
                // bytes：zip(cycle(pad))，每字节 b ^ pad[i % len]
                let plen = pad_bytes.len();
                Ok(data
                    .iter()
                    .enumerate()
                    .map(|(i, &b)| b ^ pad_bytes[i % plen])
                    .collect())
            }
        }
        XorPad::Expr(prog) => {
            // 运行期 eval 得 int 或 bytes，递归 apply。
            let pad_val = eval_expr_any(prog, ctx, py)?;
            let resolved = resolve_xor_pad(pad_val.bind(py), path)?;
            apply_xor(&resolved, data, ctx, py, path)
        }
    }
}

/// 解析 Python 对象为 XorPad（Expr 路径用）。
fn resolve_xor_pad(obj: &Bound<'_, PyAny>, path: &mut Path) -> Result<XorPad, ConstructError> {
    if let Ok(v) = obj.extract::<i64>() {
        Ok(XorPad::Int(v as u8))
    } else if let Ok(b) = obj.extract::<&[u8]>() {
        if b.len() == 1 {
            Ok(XorPad::Int(b[0]))
        } else {
            Ok(XorPad::Bytes(b.to_vec()))
        }
    } else {
        Err(ConstructError::String {
            message: format!(
                "ProcessXor needs integer or bytes pad, got {}",
                obj.get_type()
                    .name()
                    .map(|s| s.to_string())
                    .unwrap_or_default()
            ),
            path: path.to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{ExprOp, ExprProgram};
    use crate::nodes::format_field::{FormatFieldNode, PythonFormat};
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

    fn make_processxor_int(pad: u8) -> ProcessXorNode {
        let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
        ProcessXorNode::new(inner, XorPad::Int(pad))
    }

    fn make_processxor_bytes(pad: Vec<u8>) -> ProcessXorNode {
        let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
        ProcessXorNode::new(inner, XorPad::Bytes(pad))
    }

    #[test]
    fn parse_int_pad() {
        // PX-1: ProcessXor(0xf0, Int16ub).parse(b'\x00\xff') → XOR 后 b'\xf0\x0f' → 0xf00f
        with_py(|py| {
            let node = make_processxor_int(0xf0);
            let mut stream = ParseStream::new(&[0x00, 0xff]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let n: i64 = v.bind(py).extract().unwrap();
            assert_eq!(n, 0xf00f);
        });
    }

    #[test]
    fn parse_zero_pad_fastpath() {
        // PX-2: ProcessXor(0, Int16ub).parse(b'\x00\xff') → fast-path → 0x00ff
        with_py(|py| {
            let node = make_processxor_int(0);
            let mut stream = ParseStream::new(&[0x00, 0xff]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let n: i64 = v.bind(py).extract().unwrap();
            assert_eq!(n, 0x00ff);
        });
    }

    #[test]
    fn parse_bytes_pad() {
        // PX-3: ProcessXor(b'\xf0\xf1', Int16ub).parse(b'\x00\xff') → 0xf00e
        with_py(|py| {
            let node = make_processxor_bytes(vec![0xf0, 0xf1]);
            let mut stream = ParseStream::new(&[0x00, 0xff]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let n: i64 = v.bind(py).extract().unwrap();
            assert_eq!(n, 0xf00e);
        });
    }

    #[test]
    fn parse_all_zero_bytes_fastpath() {
        // PX-5: ProcessXor(b'\x00\x00', Int16ub).parse(b'\x00\xff') → fast-path → 0x00ff
        with_py(|py| {
            let node = make_processxor_bytes(vec![0x00, 0x00]);
            let mut stream = ParseStream::new(&[0x00, 0xff]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let n: i64 = v.bind(py).extract().unwrap();
            assert_eq!(n, 0x00ff);
        });
    }

    #[test]
    fn build_inverse() {
        // PX-8: ProcessXor(0xf0, Int16ub).build(0xf00f) → b'\x00\xff'（XOR 对合）
        with_py(|py| {
            let node = make_processxor_int(0xf0);
            let obj = py.eval_bound("0xf00f", None, None).expect("int");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x00, 0xff]);
        });
    }

    #[test]
    fn sizeof_forwards_to_inner() {
        // PX-9
        with_py(|py| {
            let node = make_processxor_int(0xf0);
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).unwrap(), 2);
        });
    }

    #[test]
    fn expr_pad_resolves_int() {
        // PX-6: 表达式 pad（单 GetInt）解析为 int 后 apply
        with_py(|py| {
            use pyo3::types::PyString;
            let mut ctx = Context::new_root(py).expect("ctx");
            ctx.init_expr_values(1);
            let key = PyString::new_bound(py, "pad").unbind();
            let val = 0xf0i64.into_py(py);
            ctx.set_field_at(0, &key, val.bind(py), py).unwrap();
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0)]);
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
            let node = ProcessXorNode::new(inner, XorPad::Expr(prog));
            let mut stream = ParseStream::new(&[0x00, 0xff]);
            let mut path = Path::new();
            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let n: i64 = v.bind(py).extract().unwrap();
            assert_eq!(n, 0xf00f);
        });
    }

    #[test]
    fn debug_format_includes_processxor_node() {
        with_py(|_py| {
            let node = make_processxor_int(0xf0);
            let s = format!("{:?}", node);
            assert!(s.contains("ProcessXorNode"), "got: {}", s);
        });
    }
}
