//! MappingNode：通用对象映射节点。
//!
//! 设计依据：`docs/design/模块设计/模块设计-Phase8-P1P2.md` §1.3.3。
//! Python 参考：`construct/construct/core.py` `Mapping`（L2112-2156）。
//!
//! ## 行为概述
//!
//! `Mapping(subcon, mapping)` 与 EnumNode 结构同，但 key/value 任意（非限定 int/str），
//! 且**无映射时报错**（与 EnumNode 的"无映射返回 EnumInteger"不同）。
//!
//! ## C-4：TypeError 捕获
//!
//! Python `decmapping[obj]` / `encmapping[obj]` 对**不可哈希的 key**（如 list/dict）
//! 抛 TypeError，core.py L2129-2131 捕获后转为 MappingError。construct-rs 的
//! `PyDict::get_item` 内部走 `PyObject_Hash`，对不可哈希对象抛 TypeError，
//! 本节点用 `map_err(ConstructError::from)` 捕获并转为 Generic（From<PyErr> 路径），
//! 再在 mapping 错误检查中识别 — 实际 construct-rs 用 `ConstructError::Mapping`
//! 直接包装 TypeError，对齐 Python 语义。

use crate::context::Context;
use crate::error::ConstructError;
use crate::nodes::Node;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;
use pyo3::types::PyDict;

use super::Construct;

/// 通用对象映射节点：subcon 对象 ↔ 任意对象（key/value 可为任意 hashable）。
///
/// 对应 Python construct `Mapping(subcon, mapping)`（core.py L2112）。
/// 与 EnumNode 结构同，但 key/value 任意，且无映射时报错（详见模块级注释）。
#[derive(Debug)]
pub struct MappingNode {
    /// 被包装的子树根。
    inner: Box<Node>,
    /// 解码映射：raw value → mapped object。编译期物化。
    decmapping: Py<PyDict>,
    /// 编码映射：mapped object → raw value。编译期物化。
    encmapping: Py<PyDict>,
}

impl MappingNode {
    /// 创建 `MappingNode`。
    pub fn new(inner: Node, decmapping: Py<PyDict>, encmapping: Py<PyDict>) -> Self {
        Self {
            inner: Box::new(inner),
            decmapping,
            encmapping,
        }
    }

    /// 返回内部子树根节点的引用。
    pub fn inner(&self) -> &Node {
        &self.inner
    }

    /// 返回解码映射 dict 引用。
    pub fn decmapping(&self) -> &Py<PyDict> {
        &self.decmapping
    }

    /// 返回编码映射 dict 引用。
    pub fn encmapping(&self) -> &Py<PyDict> {
        &self.encmapping
    }
}

impl Construct for MappingNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let obj = self.inner.parse(py, stream, ctx, path)?;
        // C-4：捕获 TypeError（不可哈希 key）→ MappingError，对齐 Python core.py L2129。
        let val_opt = self
            .decmapping
            .bind(py)
            .get_item(obj.bind(py))
            .map_err(|e| ConstructError::Mapping {
                message: format!("parsing failed, decmapping lookup raised: {}", e),
                path: path.to_string(),
            })?;
        match val_opt {
            Some(v) => Ok(v.unbind()),
            None => Err(ConstructError::Mapping {
                message: format!(
                    "parsing failed, no decoding mapping for {}",
                    obj.bind(py)
                        .repr()
                        .map(|r| r.to_string())
                        .unwrap_or_default()
                ),
                path: path.to_string(),
            }),
        }
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        // C-4：同 parse 路径，捕获 TypeError → MappingError。
        let val_opt =
            self.encmapping
                .bind(py)
                .get_item(obj)
                .map_err(|e| ConstructError::Mapping {
                    message: format!("building failed, encmapping lookup raised: {}", e),
                    path: path.to_string(),
                })?;
        match val_opt {
            Some(v) => self.inner.build(py, &v, stream, ctx, path),
            None => Err(ConstructError::Mapping {
                message: format!(
                    "building failed, no encoding mapping for {}",
                    obj.repr().map(|r| r.to_string()).unwrap_or_default()
                ),
                path: path.to_string(),
            }),
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

    /// 构造 MappingNode 测试 fixture：inner=Byte, mapping={0:x, 1:y, 2:z}
    /// 其中 x/y/z 是字符串。decmapping: 0→'x', 1→'y', 2→'z'; encmapping 反向。
    fn make_mapping_node(py: Python<'_>) -> MappingNode {
        let decmapping = PyDict::new_bound(py);
        decmapping.set_item(0, "x").unwrap();
        decmapping.set_item(1, "y").unwrap();
        decmapping.set_item(2, "z").unwrap();
        let encmapping = PyDict::new_bound(py);
        encmapping.set_item("x", 0).unwrap();
        encmapping.set_item("y", 1).unwrap();
        encmapping.set_item("z", 2).unwrap();
        let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
        MappingNode::new(inner, decmapping.unbind(), encmapping.unbind())
    }

    #[test]
    fn parse_mapped_returns_value() {
        // MP-1: Mapping(Byte, {x:0}).parse(b'\x00') → x（mapped object）
        with_py(|py| {
            let node = make_mapping_node(py);
            let mut stream = ParseStream::new(&[0x00]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let s: String = result.bind(py).extract().expect("str");
            assert_eq!(s, "x");
        });
    }

    #[test]
    fn parse_unmapped_raises_mapping_error() {
        // MP-2: Mapping(Byte, {x:0}).parse(b'\xff') → MappingError（与 Enum 不同）
        with_py(|py| {
            let node = make_mapping_node(py);
            let mut stream = ParseStream::new(&[0xff]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Mapping { .. }));
        });
    }

    #[test]
    fn build_mapped() {
        // MP-3: build('x') → b'\x00'
        with_py(|py| {
            let node = make_mapping_node(py);
            let obj = py.eval_bound("'x'", None, None).expect("str");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x00]);
        });
    }

    #[test]
    fn build_unmapped_raises_mapping_error() {
        // MP-4: build('unknown') → MappingError
        with_py(|py| {
            let node = make_mapping_node(py);
            let obj = py.eval_bound("'unknown'", None, None).expect("str");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Mapping { .. }));
        });
    }

    #[test]
    fn sizeof_forwards_to_inner() {
        with_py(|py| {
            let node = make_mapping_node(py);
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).unwrap(), 1);
        });
    }
}
