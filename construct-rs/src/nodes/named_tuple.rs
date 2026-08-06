//! NamedTupleNode：NamedTuple 包装节点。
//!
//! 设计依据：`docs/design/模块设计/模块设计-Phase8-P1P2.md` §6.1。
//! Python 参考：`construct/construct/core.py` `NamedTuple`（L3381-3446）。
//!
//! ## 行为概述
//!
//! `NamedTuple(tuplename, tuplefields, subcon)` 把 inner（Struct/Sequence/Array/GreedyRange）
//! 结果转为 `collections.namedtuple` 实例。
//!
//! - parse：inner.parse → 按模式提取字段 → factory(args/kwargs)
//!   - Struct 模式：按 tuplefields 名字 getattr → factory(**kwargs)
//!   - Sequence 模式：list 解包 → factory(*args)
//! - build：namedtuple 实例 → 按模式提取 → inner.build
//!
//! ## C-2：NamedTuple over Struct 的多余字段差异（设计 §6.1.6 NT-10）
//!
//! Python `factory(**obj)`（core.py L3416）传 Container 所有字段，多余字段触发 TypeError。
//! construct-rs 只传 tuplefields 命名的字段（按 tuplefields getattr），**忽略**额外字段
//! （更宽松，与 Python 不对齐，详见 §7.5）。

use crate::context::Context;
use crate::error::ConstructError;
use crate::nodes::Node;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyString, PyType};

use super::Construct;

/// NamedTuple 字段提取模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedTupleMode {
    /// inner 是 Struct（构造器工厂或 StructMixin 子类）→ 按名提取 dict → factory(**kwargs)。
    Struct,
    /// inner 是 Sequence/Array/GreedyRange → 按位置解包 → factory(*args)。
    Sequence,
}

/// NamedTuple 包装节点：把 inner（Struct/Sequence）结果转为 collections.namedtuple 实例。
///
/// 对应 Python construct `NamedTuple(tuplename, tuplefields, subcon)`（core.py L3381）。
/// 详见模块级注释。
#[derive(Debug)]
pub struct NamedTupleNode {
    /// 被包装的子树根（必须是 Struct/Sequence/Array/GreedyRange 之一）。
    inner: Box<Node>,
    /// collections.namedtuple 工厂类（编译期物化）。
    factory: Py<PyType>,
    /// 字段提取模式（编译期从 inner 类型推断）。
    mode: NamedTupleMode,
    /// tuplefields 名字列表（Struct 模式用于 getattr）。
    /// Sequence 模式可为空（按位置解包）。
    field_names: Vec<Py<PyString>>,
}

impl NamedTupleNode {
    /// 创建 `NamedTupleNode`。
    pub fn new(
        inner: Node,
        factory: Py<PyType>,
        mode: NamedTupleMode,
        field_names: Vec<Py<PyString>>,
    ) -> Self {
        Self {
            inner: Box::new(inner),
            factory,
            mode,
            field_names,
        }
    }

    /// 返回内部子树根节点的引用。
    pub fn inner(&self) -> &Node {
        &self.inner
    }

    /// 返回 factory 引用。
    pub fn factory(&self) -> &Py<PyType> {
        &self.factory
    }

    /// 返回字段提取模式。
    pub fn mode(&self) -> NamedTupleMode {
        self.mode
    }

    /// 返回 field_names 切片。
    pub fn field_names(&self) -> &[Py<PyString>] {
        &self.field_names
    }
}

impl Construct for NamedTupleNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let obj = self.inner.parse(py, stream, ctx, path)?;
        let factory_bound = self.factory.bind(py);
        let named = match self.mode {
            NamedTupleMode::Struct => {
                // Struct 返回实例：按 field_names getattr → kwargs dict → factory(**kwargs)
                // C-2：只取 tuplefields 名字（忽略额外字段，更宽松）。
                let kwargs = PyDict::new_bound(py);
                for name in &self.field_names {
                    let val = obj.bind(py).getattr(name.bind(py)).map_err(|e| {
                        ConstructError::NamedTuple {
                            message: format!(
                                "NamedTuple Struct getattr failed for {}: {}",
                                name, e
                            ),
                            path: path.to_string(),
                        }
                    })?;
                    kwargs.set_item(name.bind(py), val)?;
                }
                factory_bound
                    .call((), Some(&kwargs))
                    .map_err(|e| ConstructError::NamedTuple {
                        message: format!("namedtuple factory call failed: {}", e),
                        path: path.to_string(),
                    })?
            }
            NamedTupleMode::Sequence => {
                // Sequence/Array/GreedyRange 返回 list：factory(*list)
                let list =
                    obj.bind(py)
                        .downcast::<PyList>()
                        .map_err(|_| ConstructError::NamedTuple {
                            message: format!(
                                "NamedTuple Sequence mode expects list, got {}",
                                obj.bind(py)
                                    .get_type()
                                    .name()
                                    .map(|s| s.to_string())
                                    .unwrap_or_default()
                            ),
                            path: path.to_string(),
                        })?;
                // 构造 args tuple（PyType::call 接受位置参数）。
                let args_tuple = pyo3::types::PyTuple::new_bound(py, list.iter());
                factory_bound
                    .call(args_tuple, None)
                    .map_err(|e| ConstructError::NamedTuple {
                        message: format!("namedtuple factory call failed: {}", e),
                        path: path.to_string(),
                    })?
            }
        };
        Ok(named.unbind())
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        let build_obj: Py<PyAny> = match self.mode {
            NamedTupleMode::Struct => {
                // [设计质疑 AD-P1-5]：设计 §6.1.4 说"按名字 getattr → 构造 dict → inner.build(dict)"，
                // 但 StructNode.build 内部对 obj 调 getattr（期望实例），传 dict 会失败。
                // 实际实现：直接传 namedtuple 实例给 inner.build（namedtuple 支持 getattr，
                // StructNode.build 可正常工作）。这与 Python `factory(**obj)` 不对齐——
                // Python 把 Container 所有字段（含 _io）传给 factory（多字段 TypeError），
                // construct-rs 让 StructNode 自己 getattr tuplefields 命名的字段
                // （忽略实例 __dict__ 中其他字段，C-2 修正的"宽松"行为）。
                obj.clone().unbind()
            }
            NamedTupleMode::Sequence => {
                // namedtuple 实例 → list(instance) → inner.build(list)
                // namedtuple 是 tuple 子类，list(nt) 转 list
                let iter_bound = obj.iter().map_err(|e| ConstructError::NamedTuple {
                    message: format!("NamedTuple build: obj not iterable: {}", e),
                    path: path.to_string(),
                })?;
                let mut collected: Vec<Py<PyAny>> = Vec::new();
                for item in iter_bound {
                    collected.push(item?.unbind());
                }
                let seq = PyList::new_bound(py, &collected);
                seq.into_py(py)
            }
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
    use crate::nodes::bytes::BytesNode;
    use crate::nodes::format_field::{FormatFieldNode, PythonFormat};
    use crate::nodes::struct_node::FieldMode;
    use crate::nodes::struct_node::{FieldName, StructField, StructNode};
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

    /// 加载 collections.namedtuple 工厂。
    fn make_factory(py: Python<'_>, name: &str, fields: &[&str]) -> Py<PyType> {
        let nt_module = py.import_bound("collections").expect("collections module");
        let namedtuple_fn = nt_module.getattr("namedtuple").expect("namedtuple attr");
        let name_bound = pyo3::types::PyString::new_bound(py, name).into_any();
        let fields_bound = pyo3::types::PyList::new_bound(py, fields.iter().copied()).into_any();
        let args_tuple = pyo3::types::PyTuple::new_bound(py, [name_bound, fields_bound]);
        namedtuple_fn
            .call1(args_tuple)
            .expect("namedtuple call")
            .extract::<Py<PyType>>()
            .expect("PyType")
    }

    #[test]
    fn parse_struct_mode_returns_namedtuple() {
        // NT-2: NamedTuple("coord", "x y", Struct{x:Int8ub, y:Int8ub}).parse(b'\x01\x02')
        //      → coord(x=1, y=2)
        with_py(|py| {
            let struct_node = StructNode::new(
                py,
                vec![
                    StructField {
                        name: FieldName::new(py, "x"),
                        node: Node::FormatField(FormatFieldNode::new(
                            PythonFormat::UnsignedInt8Big,
                        )),
                        mode: FieldMode::Rw,
                    },
                    StructField {
                        name: FieldName::new(py, "y"),
                        node: Node::FormatField(FormatFieldNode::new(
                            PythonFormat::UnsignedInt8Big,
                        )),
                        mode: FieldMode::Rw,
                    },
                ],
                py.eval_bound("type('MockStruct', (), {})", None, None)
                    .unwrap()
                    .extract::<Py<PyType>>()
                    .unwrap(),
                false,
                false,
            );
            let factory = make_factory(py, "coord", &["x", "y"]);
            let field_names = vec![
                PyString::new_bound(py, "x").unbind(),
                PyString::new_bound(py, "y").unbind(),
            ];
            let node = NamedTupleNode::new(
                Node::Struct(struct_node),
                factory,
                NamedTupleMode::Struct,
                field_names,
            );
            let mut stream = ParseStream::new(&[0x01, 0x02]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let r = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let x: i64 = r.bind(py).getattr("x").unwrap().extract().unwrap();
            let y: i64 = r.bind(py).getattr("y").unwrap().extract().unwrap();
            assert_eq!(x, 1);
            assert_eq!(y, 2);
        });
    }

    #[test]
    fn build_struct_mode() {
        // NT-7: NamedTuple("coord", "x y", Struct).build(coord(x=1, y=2)) → b'\x01\x02'
        with_py(|py| {
            let struct_node = StructNode::new(
                py,
                vec![
                    StructField {
                        name: FieldName::new(py, "x"),
                        node: Node::FormatField(FormatFieldNode::new(
                            PythonFormat::UnsignedInt8Big,
                        )),
                        mode: FieldMode::Rw,
                    },
                    StructField {
                        name: FieldName::new(py, "y"),
                        node: Node::FormatField(FormatFieldNode::new(
                            PythonFormat::UnsignedInt8Big,
                        )),
                        mode: FieldMode::Rw,
                    },
                ],
                py.eval_bound("type('MockStruct', (), {})", None, None)
                    .unwrap()
                    .extract::<Py<PyType>>()
                    .unwrap(),
                false,
                false,
            );
            let factory = make_factory(py, "coord", &["x", "y"]);
            let field_names = vec![
                PyString::new_bound(py, "x").unbind(),
                PyString::new_bound(py, "y").unbind(),
            ];
            let node = NamedTupleNode::new(
                Node::Struct(struct_node),
                factory.clone_ref(py),
                NamedTupleMode::Struct,
                field_names,
            );
            // 构造 coord(x=1, y=2) 实例（位置参数）
            let args = pyo3::types::PyTuple::new_bound(py, [1i64, 2i64]);
            let obj = factory.bind(py).call1(args).expect("make coord");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x01, 0x02]);
        });
    }

    #[test]
    fn sizeof_forwards_to_inner() {
        // NT-8
        with_py(|py| {
            let inner = Node::Bytes(BytesNode::new_const(2));
            let factory = make_factory(py, "c", &["a", "b"]);
            let node = NamedTupleNode::new(inner, factory, NamedTupleMode::Sequence, vec![]);
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).unwrap(), 2);
        });
    }

    #[test]
    fn debug_format_includes_namedtuple_node() {
        with_py(|py| {
            let inner = Node::Bytes(BytesNode::new_const(2));
            let factory = make_factory(py, "c", &["a", "b"]);
            let node = NamedTupleNode::new(inner, factory, NamedTupleMode::Sequence, vec![]);
            let s = format!("{:?}", node);
            assert!(s.contains("NamedTupleNode"), "got: {}", s);
        });
    }
}
