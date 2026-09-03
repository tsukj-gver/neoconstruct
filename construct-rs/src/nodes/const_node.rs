//! ConstNode：常量字段节点。
//!
//! Python 参考：`construct/construct/core.py` `Const`（L2808-2876）。
//!
//! ## 行为概述
//!
//! Const 是 Subconstruct 包装：parse 校验子解析结果 == value；
//! build 用 value（忽略 obj，除非 obj 已经等于 value）。
//!
//! ## 与 Subconstruct 模式的关系
//!
//! ConstNode 是 Subconstruct 的"带校验"变体：parse 转发 inner.parse 后做相等比较；
//! build 接受 None 或等于 value 的 obj（对齐 Python `flagbuildnone=True`）。

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::basic::CompareOp;
use pyo3::prelude::*;

use super::Construct;
use crate::nodes::Node;

/// 常量字段节点：parse 校验子解析结果 == value；build 用 value（忽略 obj）。
///
/// 对应 Python construct `Const(value, subcon=None)`（core.py L2808）。
///
/// # 三方法行为
///
/// - parse：inner.parse(obj) → `obj == value`（pyo3 rich compare C API）→ 不等
///   `ConstructError::Const`，等则返回 obj
/// - build：obj is None 或 obj == value → inner.build(value)；否则 Const 错误
/// - sizeof：转发 inner.sizeof
///
/// # value 类型
///
/// `value: Py<PyAny>` 持有任意 Python 对象（int/bytes/str 等）。比较走 pyo3
/// `obj.bind(py).rich_compare(value.bind(py), CompareOp::Eq)`，由 CPython 调度
/// 到对应类型的 `__eq__`（对 int/bytes/str 是 C 级实现）。
#[derive(Debug)]
pub struct ConstNode {
    /// 被包装的子树根。
    inner: Box<Node>,
    /// 期望值（任意 Python 对象）。
    value: Py<PyAny>,
}

impl ConstNode {
    /// 创建 `ConstNode`，包裹给定的子树根节点与期望值。
    ///
    /// # 参数
    ///
    /// - `inner`：被包装的子树（用于 parse/build/sizeof）。
    /// - `value`：期望的常量值（任意 Python 对象，通常为 int/bytes/str）。
    pub fn new(inner: Node, value: Py<PyAny>) -> Self {
        Self {
            inner: Box::new(inner),
            value,
        }
    }

    /// 返回内部子树根节点的引用。
    pub fn inner(&self) -> &Node {
        &self.inner
    }

    /// 返回期望值的引用。
    pub fn value(&self) -> &Py<PyAny> {
        &self.value
    }
}

impl Construct for ConstNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let obj = self.inner.parse(py, stream, ctx, path)?;
        // pyo3 rich_compare（C API，对 int/bytes/str 走 C 级 __eq__）。
        let is_eq = obj
            .bind(py)
            .rich_compare(self.value.bind(py), CompareOp::Eq)?
            .is_truthy()?;
        if !is_eq {
            let value_repr = self
                .value
                .bind(py)
                .repr()
                .map(|r| r.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "<unrepr>".to_string());
            let obj_repr = obj
                .bind(py)
                .repr()
                .map(|r| r.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "<unrepr>".to_string());
            return Err(ConstructError::Const {
                message: format!("parsing expected {} but parsed {}", value_repr, obj_repr),
                path: path.to_string(),
            });
        }
        Ok(obj)
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        // CN-3/CN-4/CN-5/CN-6：obj is None 或 obj == value → inner.build(value)；
        // 否则 Const 错误。flagbuildnone=True 对齐 Python。
        let obj_is_none = obj.is_none();
        let is_eq = if !obj_is_none {
            obj.rich_compare(self.value.bind(py), CompareOp::Eq)?
                .is_truthy()?
        } else {
            false
        };
        if obj_is_none || is_eq {
            self.inner.build(py, self.value.bind(py), stream, ctx, path)
        } else {
            let obj_repr = obj
                .repr()
                .map(|r| r.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "<unrepr>".to_string());
            Err(ConstructError::Const {
                message: format!("building expected None or value, but got {}", obj_repr),
                path: path.to_string(),
            })
        }
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        self.inner.sizeof(ctx)
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::bytes::BytesNode;
    use crate::nodes::format_field::{FormatFieldNode, PythonFormat};
    use pyo3::types::PyBytes;

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

    // ======================================================================
    // 构造器
    // ======================================================================

    #[test]
    fn new_stores_inner_and_value() {
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let value: Py<PyAny> = 42i64.into_py(py);
            let node = ConstNode::new(inner, value.clone_ref(py));
            assert!(matches!(node.inner(), Node::FormatField(_)));
            let stored: i64 = node.value().bind(py).extract().unwrap();
            assert_eq!(stored, 42);
        });
    }

    // ======================================================================
    // parse — CN-1 / CN-2
    // ======================================================================

    #[test]
    fn parse_bytes_match_returns_obj() {
        // CN-1: Const(Bytes(4), b"IHDR").parse(b"IHDR") → b"IHDR"
        with_py(|py| {
            let inner = Node::Bytes(BytesNode::new_const(4));
            let value = PyBytes::new_bound(py, b"IHDR").into_any().unbind();
            let node = ConstNode::new(inner, value);
            let mut stream = ParseStream::new(b"IHDR");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().unwrap();
            assert_eq!(bytes, b"IHDR");
            assert_eq!(stream.tell(), 4);
        });
    }

    #[test]
    fn parse_bytes_mismatch_raises_const_error() {
        // CN-2: Const(Bytes(4), b"IHDR").parse(b"JPEG") → ConstError
        with_py(|py| {
            let inner = Node::Bytes(BytesNode::new_const(4));
            let value = PyBytes::new_bound(py, b"IHDR").into_any().unbind();
            let node = ConstNode::new(inner, value);
            let mut stream = ParseStream::new(b"JPEG");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Const { message, path: p } => {
                    assert!(message.contains("expected"), "got: {}", message);
                    assert!(message.contains("parsed"), "got: {}", message);
                    assert_eq!(p, "root");
                }
                other => panic!("expected Const error, got {:?}", other),
            }
        });
    }

    #[test]
    fn parse_int_match_returns_obj() {
        // CN-4 等价：Const(255, Int32ub).parse(b'\x00\x00\x00\xff') → 255
        // Int32ub 是 big-endian，255 = 0x000000ff
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt32Big));
            let value: Py<PyAny> = 255i64.into_py(py);
            let node = ConstNode::new(inner, value);
            let mut stream = ParseStream::new(&[0x00, 0x00, 0x00, 0xff]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().unwrap();
            assert_eq!(v, 255);
        });
    }

    // ======================================================================
    // build — CN-3 / CN-5 / CN-6
    // ======================================================================

    #[test]
    fn build_none_uses_value() {
        // CN-3/CN-6: Const(255, Int32ub).build(None) → b'\x00\x00\x00\xff'（big-endian 255）
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt32Big));
            let value: Py<PyAny> = 255i64.into_py(py);
            let node = ConstNode::new(inner, value);
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x00, 0x00, 0x00, 0xff]);
        });
    }

    #[test]
    fn build_equal_obj_uses_value() {
        // CN-4: Const(255, Int32ub).build(255) → b'\x00\x00\x00\xff'（big-endian 255）
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt32Big));
            let value: Py<PyAny> = 255i64.into_py(py);
            let node = ConstNode::new(inner, value);
            let obj = py.eval_bound("255", None, None).expect("255");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x00, 0x00, 0x00, 0xff]);
        });
    }

    #[test]
    fn build_mismatched_obj_raises_const_error() {
        // CN-5: Const(255, Int32ub).build(256) → ConstError
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt32Big));
            let value: Py<PyAny> = 255i64.into_py(py);
            let node = ConstNode::new(inner, value);
            let obj = py.eval_bound("256", None, None).expect("256");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Const { .. }));
        });
    }

    #[test]
    fn build_bytes_none_writes_value() {
        with_py(|py| {
            let inner = Node::Bytes(BytesNode::new_const(4));
            let value = PyBytes::new_bound(py, b"IHDR").into_any().unbind();
            let node = ConstNode::new(inner, value);
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), b"IHDR");
        });
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_returns_inner_size() {
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt32Big));
            let value: Py<PyAny> = 255i64.into_py(py);
            let node = ConstNode::new(inner, value);
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).unwrap(), 4);
        });
    }

    // ======================================================================
    // parse ↔ build 往返
    // ======================================================================

    #[test]
    fn round_trip_const_int() {
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let value: Py<PyAny> = 42i64.into_py(py);
            let node = ConstNode::new(inner, value);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            // build
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut bstream = BuildStream::new();
            node.build(py, &obj, &mut bstream, &mut ctx, &mut path)
                .expect("build");
            let bytes = bstream.into_bytes();

            // parse back
            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().unwrap();
            assert_eq!(v, 42);
        });
    }

    // ======================================================================
    // Debug
    // ======================================================================

    #[test]
    fn debug_format_includes_const_node() {
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let value: Py<PyAny> = 42i64.into_py(py);
            let node = ConstNode::new(inner, value);
            let s = format!("{:?}", node);
            assert!(s.contains("ConstNode"), "got: {}", s);
        });
    }
}
