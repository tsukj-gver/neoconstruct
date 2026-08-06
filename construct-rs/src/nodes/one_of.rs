//! OneOfNode / NoneOfNode：值集合校验节点。
//!
//! 设计依据：`docs/design/模块设计/模块设计-Phase8-P1P2.md` §2.2。
//! Python 参考：`construct/construct/core.py` `OneOf`（L6320）/ `NoneOf`（L6342）。
//!
//! ## 行为概述
//!
//! - OneOf：parse/build 校验 inner 结果 ∈ valids（frozenset）；不在则 ValidationError。
//! - NoneOf：与 OneOf 取反——校验 inner 结果 ∉ invalids；在则 ValidationError。
//!
//! ## 编译期物化（§0.2 判据）
//!
//! `valids`/`invalids` 编译期物化为 `Py<PyFrozenSet>`。运行时通过
//! `PyFrozenSet::contains`（C API `PySet_Contains`）查询，对 int/str/bytes 元素
//! 走 C 级 `__hash__`/`__eq__`，**不计额外 FFI**（§0.2 判据 2）。

use crate::context::Context;
use crate::error::ConstructError;
use crate::nodes::Node;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;
use pyo3::types::PyFrozenSet;

use super::Construct;

/// 单值校验节点：parse/build 校验 inner 结果 ∈ valids。
///
/// 对应 Python construct `OneOf(subcon, valids)`（core.py L6320）。
/// 详见模块级注释。
#[derive(Debug)]
pub struct OneOfNode {
    /// 被包装的子树根。
    inner: Box<Node>,
    /// 合法值集合：编译期物化为 frozenset。
    valids: Py<PyFrozenSet>,
}

impl OneOfNode {
    /// 创建 `OneOfNode`。
    pub fn new(inner: Node, valids: Py<PyFrozenSet>) -> Self {
        Self {
            inner: Box::new(inner),
            valids,
        }
    }

    /// 返回内部子树根节点的引用。
    pub fn inner(&self) -> &Node {
        &self.inner
    }

    /// 返回合法值集合引用。
    pub fn valids(&self) -> &Py<PyFrozenSet> {
        &self.valids
    }
}

impl Construct for OneOfNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let obj = self.inner.parse(py, stream, ctx, path)?;
        let obj_bound = obj.bind(py);
        // C-6：bool 是 int 子类，valids 含 1 时 True 也匹配（Python 语义）。
        let contains = self
            .valids
            .bind(py)
            .contains(obj_bound)
            .map_err(ConstructError::from)?;
        if !contains {
            return Err(ConstructError::Validation {
                message: format!(
                    "object failed validation: {}",
                    obj_bound.repr().map(|r| r.to_string()).unwrap_or_default()
                ),
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
        let contains = self
            .valids
            .bind(py)
            .contains(obj)
            .map_err(ConstructError::from)?;
        if !contains {
            return Err(ConstructError::Validation {
                message: format!(
                    "object failed validation: {}",
                    obj.repr().map(|r| r.to_string()).unwrap_or_default()
                ),
                path: path.to_string(),
            });
        }
        self.inner.build(py, obj, stream, ctx, path)
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        self.inner.sizeof(ctx)
    }
}

/// 排除值校验节点：parse/build 校验 inner 结果 ∉ invalids。
///
/// 对应 Python construct `NoneOf(subcon, invalids)`（core.py L6342）。
/// 与 [`OneOfNode`] 结构同，校验取反。
#[derive(Debug)]
pub struct NoneOfNode {
    /// 被包装的子树根。
    inner: Box<Node>,
    /// 非法值集合：编译期物化为 frozenset。
    invalids: Py<PyFrozenSet>,
}

impl NoneOfNode {
    /// 创建 `NoneOfNode`。
    pub fn new(inner: Node, invalids: Py<PyFrozenSet>) -> Self {
        Self {
            inner: Box::new(inner),
            invalids,
        }
    }

    /// 返回内部子树根节点的引用。
    pub fn inner(&self) -> &Node {
        &self.inner
    }

    /// 返回非法值集合引用。
    pub fn invalids(&self) -> &Py<PyFrozenSet> {
        &self.invalids
    }
}

impl Construct for NoneOfNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let obj = self.inner.parse(py, stream, ctx, path)?;
        let obj_bound = obj.bind(py);
        let contains = self
            .invalids
            .bind(py)
            .contains(obj_bound)
            .map_err(ConstructError::from)?;
        if contains {
            return Err(ConstructError::Validation {
                message: format!(
                    "object failed validation: {}",
                    obj_bound.repr().map(|r| r.to_string()).unwrap_or_default()
                ),
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
        let contains = self
            .invalids
            .bind(py)
            .contains(obj)
            .map_err(ConstructError::from)?;
        if contains {
            return Err(ConstructError::Validation {
                message: format!(
                    "object failed validation: {}",
                    obj.repr().map(|r| r.to_string()).unwrap_or_default()
                ),
                path: path.to_string(),
            });
        }
        self.inner.build(py, obj, stream, ctx, path)
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

    fn make_frozenset(py: Python<'_>, vals: &[i64]) -> Py<PyFrozenSet> {
        PyFrozenSet::new_bound(py, vals).unwrap().unbind()
    }

    fn make_oneof_node(py: Python<'_>) -> OneOfNode {
        let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
        let valids = make_frozenset(py, &[1, 2, 3]);
        OneOfNode::new(inner, valids)
    }

    fn make_noneof_node(py: Python<'_>) -> NoneOfNode {
        let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
        let invalids = make_frozenset(py, &[1, 2, 3]);
        NoneOfNode::new(inner, invalids)
    }

    #[test]
    fn oneof_parse_valid() {
        // OO-1: OneOf(Byte, [1,2,3]).parse(b'\x01') → 1
        with_py(|py| {
            let node = make_oneof_node(py);
            let mut stream = ParseStream::new(&[0x01]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let n: i64 = v.bind(py).extract().unwrap();
            assert_eq!(n, 1);
        });
    }

    #[test]
    fn oneof_parse_invalid_raises() {
        // OO-2: OneOf(Byte, [1,2,3]).parse(b'\xff') → ValidationError
        with_py(|py| {
            let node = make_oneof_node(py);
            let mut stream = ParseStream::new(&[0xff]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Validation { .. }));
        });
    }

    #[test]
    fn oneof_build_valid() {
        // OO-3: OneOf(Byte, [1,2,3]).build(1) → b'\x01'
        with_py(|py| {
            let node = make_oneof_node(py);
            let obj = py.eval_bound("1", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x01]);
        });
    }

    #[test]
    fn oneof_build_invalid_raises() {
        // OO-4: OneOf(Byte, [1,2,3]).build(99) → ValidationError
        with_py(|py| {
            let node = make_oneof_node(py);
            let obj = py.eval_bound("99", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Validation { .. }));
        });
    }

    #[test]
    fn oneof_empty_valids_always_fails() {
        // OO-5: 空集合不含任何元素 → 总是 ValidationError
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let empty = make_frozenset(py, &[]);
            let node = OneOfNode::new(inner, empty);
            let mut stream = ParseStream::new(&[0x01]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Validation { .. }));
        });
    }

    #[test]
    fn noneof_parse_excluded() {
        // NO-1: NoneOf(Byte, [1,2,3]).parse(b'\xff') → 255
        with_py(|py| {
            let node = make_noneof_node(py);
            let mut stream = ParseStream::new(&[0xff]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let n: i64 = v.bind(py).extract().unwrap();
            assert_eq!(n, 255);
        });
    }

    #[test]
    fn noneof_parse_included_raises() {
        // NO-2: NoneOf(Byte, [1,2,3]).parse(b'\x01') → ValidationError
        with_py(|py| {
            let node = make_noneof_node(py);
            let mut stream = ParseStream::new(&[0x01]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Validation { .. }));
        });
    }

    #[test]
    fn noneof_empty_always_passes() {
        // NO-3: 空 invalids，无元素被排除
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let empty = make_frozenset(py, &[]);
            let node = NoneOfNode::new(inner, empty);
            let mut stream = ParseStream::new(&[0xff]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let v = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let n: i64 = v.bind(py).extract().unwrap();
            assert_eq!(n, 255);
        });
    }

    #[test]
    fn sizeof_forwards_to_inner() {
        with_py(|py| {
            let node = make_oneof_node(py);
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).unwrap(), 1);
        });
    }
}
