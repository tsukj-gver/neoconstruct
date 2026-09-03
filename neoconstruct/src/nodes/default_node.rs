//! DefaultNode：默认值字段节点。
//!
//! Python 参考：`construct/construct/core.py` `Default`（L3030-3078）。
//!
//! ## 行为概述
//!
//! Default 是 Subconstruct 的"build 默认值"变体：
//! - parse 转发 inner.parse（与 Subconstruct 一致）
//! - build：obj is None → 求值 ExprProgram 得 i64 → 转 PyLong → inner.build；
//!   否则 inner.build(obj)
//!
//! ## 表达式约束
//!
//! `value` 必须是字段表达式（FieldRef/ExprRef/int 组合），编译为 ExprProgram。
//! **不接收 Python lambda/callable**（与 RebuildNode 同约束）。
//! 常量值编译期包装为单条 `Const` ExprProgram。

use crate::context::Context;
use crate::error::ConstructError;
use crate::expr::{eval_expr_int, ExprProgram};
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

use super::Construct;
use crate::nodes::Node;

/// 默认值字段节点：build 时 obj 为 None 则用 value 表达式求值。
///
/// 对应 Python construct `Default(subcon, value)`（core.py L3030）。
/// Python 的 value 可为常量或 context lambda；neoconstruct 强制编译为 ExprProgram
/// 或编译期常量（与 Rebuild func 同脉络）。
///
/// # 三方法行为
///
/// - parse：转发 inner.parse（与 Subconstruct 一致）
/// - build：obj is None → 求值 ExprProgram 得 i64 → 转 PyLong → inner.build(py_long)；
///   否则 inner.build(obj)
/// - sizeof：转发 inner.sizeof
///
/// # 常量 value 的编译期包装
///
/// 用户传 `Default(Byte, 0)` 时，0 在编译期编译为
/// `ExprProgram { ops: vec![ExprOp::Const(0)] }`（与 Computed 常量模式同，
/// 详见 `nodes/computed.rs`）。Python 用户写 `Default(Byte, lambda ctx: ...)`
/// 不支持——parity 已知差异。
#[derive(Debug)]
pub struct DefaultNode {
    /// 被包装的子树根。
    inner: Box<Node>,
    /// build 时默认值表达式。常量值编译期包装为单条 `Const` ExprProgram。
    value: ExprProgram,
}

impl DefaultNode {
    /// 创建 `DefaultNode`，包裹给定的子树根节点与默认值表达式。
    ///
    /// # 参数
    ///
    /// - `inner`：被包装的子树（用于 parse/build/sizeof）。
    /// - `value`：build 时默认值表达式（编译期常量也包装为 ExprProgram）。
    pub fn new(inner: Node, value: ExprProgram) -> Self {
        Self {
            inner: Box::new(inner),
            value,
        }
    }

    /// 返回内部子树根节点的引用。
    pub fn inner(&self) -> &Node {
        &self.inner
    }

    /// 返回默认值表达式的引用。
    pub fn value(&self) -> &ExprProgram {
        &self.value
    }
}

impl Construct for DefaultNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // DF parse 转发 inner（与 Subconstruct 一致）
        self.inner.parse(py, stream, ctx, path)
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        let build_obj: Py<PyAny> = if obj.is_none() {
            // 求值 ExprProgram 得 i64 → PyLong
            let v = eval_expr_int(&self.value, ctx, py)?;
            v.into_py(py)
        } else {
            // 借用 obj（不克隆，转交 inner）
            obj.clone().unbind()
        };
        self.inner.build(py, build_obj.bind(py), stream, ctx, path)
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
    use crate::expr::ExprOp;
    use crate::nodes::format_field::{FormatFieldNode, PythonFormat};
    use pyo3::types::PyString;

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

    /// 构造一个含若干整数字段的 `Context`，用于表达式求值测试。
    fn make_context<'py>(py: Python<'py>, entries: &[(&str, i64)]) -> Context<'py> {
        let mut ctx = Context::new_root(py).expect("new_root");
        ctx.init_expr_values(entries.len());
        for (idx, (name, value)) in entries.iter().enumerate() {
            let key = PyString::new_bound(py, name).unbind();
            let val = (*value).into_py(py);
            ctx.set_field_at(idx, &key, val.bind(py), py)
                .expect("set_field_at");
        }
        ctx
    }

    // ======================================================================
    // 构造器
    // ======================================================================

    #[test]
    fn new_stores_inner_and_value() {
        let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
        let value = ExprProgram::new(vec![ExprOp::Const(0)]);
        let node = DefaultNode::new(inner, value);
        assert!(matches!(node.inner(), Node::FormatField(_)));
        assert_eq!(node.value().ops(), &[ExprOp::Const(0)]);
    }

    // ======================================================================
    // parse — 转发 inner
    // ======================================================================

    #[test]
    fn parse_forwards_to_inner() {
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let value = ExprProgram::new(vec![ExprOp::Const(0)]);
            let node = DefaultNode::new(inner, value);
            let mut stream = ParseStream::new(&[0x42]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().unwrap();
            assert_eq!(v, 0x42);
        });
    }

    // ======================================================================
    // build — obj 为 None 用默认值；否则转发 inner
    // ======================================================================

    #[test]
    fn build_none_uses_constant_value() {
        // Default(Byte, 0).build(None) → b'\x00'
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let value = ExprProgram::new(vec![ExprOp::Const(0)]);
            let node = DefaultNode::new(inner, value);
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x00]);
        });
    }

    #[test]
    fn build_non_none_uses_obj() {
        // Default(Byte, 0).build(5) → b'\x05'
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let value = ExprProgram::new(vec![ExprOp::Const(0)]);
            let node = DefaultNode::new(inner, value);
            let obj = py.eval_bound("5", None, None).expect("5");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x05]);
        });
    }

    #[test]
    fn build_none_uses_expression_value() {
        // Default(Byte, x + 1).build(None) where x=4 → b'\x05'
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            // value: GetInt(0) + Const(1)
            let value = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(1), ExprOp::Add]);
            let node = DefaultNode::new(inner, value);
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            let mut ctx = make_context(py, &[("x", 4)]);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x05]);
        });
    }

    #[test]
    fn build_none_expression_error_propagates() {
        // Default 表达式求值失败 → 错误向上传播
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let value = ExprProgram::new(vec![ExprOp::GetInt(0)]);
            let node = DefaultNode::new(inner, value);
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            // placeholder ctx 未 init_expr_values → ExprContext 错误
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(
                matches!(err, ConstructError::ExprContext { .. }),
                "got: {:?}",
                err
            );
        });
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_returns_inner_size() {
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
            let value = ExprProgram::new(vec![ExprOp::Const(0)]);
            let node = DefaultNode::new(inner, value);
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).unwrap(), 2);
        });
    }

    // ======================================================================
    // Debug
    // ======================================================================

    #[test]
    fn debug_format_includes_default_node() {
        let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
        let value = ExprProgram::new(vec![ExprOp::Const(0)]);
        let node = DefaultNode::new(inner, value);
        let s = format!("{:?}", node);
        assert!(s.contains("DefaultNode"), "got: {}", s);
    }
}
