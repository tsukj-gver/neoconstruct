//! UnionNode：联合体节点（多视角 parse）。
//!
//! Python 参考：`construct/construct/core.py` `Union`（L3641-3800）。
//!
//! ## 行为概述
//!
//! `Union(parsefrom, *subcons)` 多视角 parse——每 subcon 独立 parse 后回退到 fallback，
//! 按 parsefrom 选择 forward 位置。
//!
//! - parse：context nesting；fallback = stream.tell()；遍历 subcons，每 subcon.parse，
//!   命名字段写入 obj（PyDict）+ child_ctx；记录 forwards[i] = tell()；seek(fallback)。
//!   按 parsefrom seek 到 forwards[target]（None 则不 seek）
//! - build：context nesting；遍历 subcons，找首个 name in obj 的 → build 它，返回
//! - sizeof：永远 Err（"Union builds depending on actual object dict, size is unknown"）
//!
//! ## 已知 parity 限制
//!
//! 1. `context.update(obj)`：Python L3732 把整个 obj dict 合并进 context，
//!    neoconstruct child_ctx 仅 set_field_at 被选中的单个 subcon 字段。
//!    跨 subcon 引用的 build 用例受此限制。
//! 2. `flagbuildnone`：Python L3734-3735 对 flagbuildnone=True 的 subcon 用
//!    `obj.get(name, None)`（允许 obj 缺键），neoconstruct 跳过缺键 subcon。

use crate::context::Context;
use crate::error::ConstructError;
use crate::expr::{eval_expr_int, ExprProgram};
use crate::nodes::struct_node::FieldName;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString};

use super::{Construct, Node};

/// parsefrom 的解析结果（编译期从用户输入编译）。
///
#[derive(Debug)]
pub enum ParseFrom {
    /// 留在 fallback 位置（Python `parsefrom=None`）。
    None,
    /// 常量 index（Python `parsefrom=0`）。
    Index(usize),
    /// 常量 name，编译期已解析为 index（Python `parsefrom="raw"`）。
    Name {
        /// 原始 name（用于错误消息）。
        name: Py<PyString>,
        /// 解析后的 subcon index。
        resolved_index: usize,
    },
    /// 表达式，运行期求值得 index（neoconstruct 扩展，替代 context lambda）。
    Expr(ExprProgram),
}

/// Union 子构造器（name 可选 + node）。
#[derive(Debug)]
pub struct UnionSubcon {
    /// 字段名（None 表示匿名 subcon）。
    pub name: Option<FieldName>,
    /// 子节点。
    pub node: Node,
}

impl UnionSubcon {
    /// 创建命名的 UnionSubcon。
    pub fn new_named(name: FieldName, node: Node) -> Self {
        Self {
            name: Some(name),
            node,
        }
    }

    /// 创建匿名的 UnionSubcon。
    pub fn new_anonymous(node: Node) -> Self {
        Self { name: None, node }
    }
}

/// 联合体节点：多视角 parse（每 subcon 独立 parse 后回退），按 parsefrom 选择 forward。
///
/// 对应 Python construct `Union(parsefrom, *subcons)`（core.py L3641）。
/// 详见模块级注释。
#[derive(Debug)]
pub struct UnionNode {
    /// 有序 subcons 列表。
    subcons: Vec<UnionSubcon>,
    /// parsefrom 策略（编译期编译）。
    parsefrom: ParseFrom,
    /// 是否含表达式（编译期递归子树计算）。
    has_expressions: bool,
}

impl UnionNode {
    /// 创建 `UnionNode`。
    pub fn new(subcons: Vec<UnionSubcon>, parsefrom: ParseFrom, has_expressions: bool) -> Self {
        Self {
            subcons,
            parsefrom,
            has_expressions,
        }
    }

    /// 返回 subcons 切片。
    pub fn subcons(&self) -> &[UnionSubcon] {
        &self.subcons
    }

    /// 返回 parsefrom 引用。
    pub fn parsefrom(&self) -> &ParseFrom {
        &self.parsefrom
    }

    /// 返回是否含表达式。
    pub fn has_expressions(&self) -> bool {
        self.has_expressions
    }
}

impl Construct for UnionNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 1. context nesting（与 FocusedSeq 同模式）
        let mut child_ctx = Context::new_child(ctx, py)?;
        if self.has_expressions {
            child_ctx.init_expr_values(self.subcons.len());
        }

        // 2. 结果 dict（Python 返回 Container，neoconstruct 返回普通 PyDict）
        let obj = PyDict::new_bound(py);

        // 3. fallback + 遍历
        let fallback = stream.tell();
        let mut forwards = Vec::with_capacity(self.subcons.len());
        for (idx, sc) in self.subcons.iter().enumerate() {
            let subobj = sc.node.parse(py, stream, &mut child_ctx, path)?;
            if let Some(name) = &sc.name {
                obj.set_item(name.py_name(), subobj.bind(py))?;
                child_ctx.set_field_at(idx, name.py_name(), subobj.bind(py), py)?;
            }
            forwards.push(stream.tell());
            stream.seek(fallback, path)?;
        }

        // 4. 按 parsefrom seek
        let target_index: Option<usize> = match &self.parsefrom {
            ParseFrom::None => None,
            ParseFrom::Index(i) => Some(*i),
            ParseFrom::Name { resolved_index, .. } => Some(*resolved_index),
            ParseFrom::Expr(prog) => {
                let v = eval_expr_int(prog, &child_ctx, py)?;
                Some(v as usize)
            }
        };
        if let Some(idx) = target_index {
            let forward = forwards.get(idx).ok_or_else(|| ConstructError::Union {
                message: format!(
                    "parsefrom index {} out of range ({} subcons)",
                    idx,
                    self.subcons.len()
                ),
                path: path.to_string(),
            })?;
            stream.seek(*forward, path)?;
        }
        Ok(obj.into_py(py))
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        let mut child_ctx = Context::new_child(ctx, py)?;
        if self.has_expressions {
            child_ctx.init_expr_values(self.subcons.len());
        }
        // obj 是 PyDict（用户传 dict({name: value})）
        let obj_dict = obj
            .downcast::<PyDict>()
            .map_err(|_| ConstructError::Union {
                message: format!(
                    "Union build expects a dict, got {}",
                    obj.get_type()
                        .name()
                        .map(|s| s.to_string())
                        .unwrap_or_default()
                ),
                path: path.to_string(),
            })?;
        // 遍历找首个 name in obj 的 subcon
        for (idx, sc) in self.subcons.iter().enumerate() {
            let take = match &sc.name {
                Some(name) => obj_dict.get_item(name.py_name())?.is_some(),
                None => false, // 匿名 subcon 不参与 build 选择
            };
            if take {
                let name = sc.name.as_ref().expect("name present when take=true");
                let value = obj_dict
                    .get_item(name.py_name())?
                    .expect("value present when take=true");
                child_ctx.set_field_at(idx, name.py_name(), &value, py)?;
                sc.node.build(py, &value, stream, &mut child_ctx, path)?;
                return Ok(());
            }
        }
        Err(ConstructError::Union {
            message: format!(
                "cannot build, none of subcons were found in the dictionary {}",
                obj.repr().map(|r| r.to_string()).unwrap_or_default()
            ),
            path: path.to_string(),
        })
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        Err(ConstructError::Union {
            message: "Union builds depending on actual object dict, size is unknown".to_string(),
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
    use crate::nodes::bytes::BytesNode;
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

    fn fmt_node(fmt: PythonFormat) -> Node {
        Node::FormatField(FormatFieldNode::new(fmt))
    }

    /// 构造 Union(None, "raw"/Bytes(2), "ints"/Byte[2]) 测试 fixture。
    fn make_union_node(py: Python<'_>, parsefrom: ParseFrom) -> UnionNode {
        let raw_field = UnionSubcon::new_named(
            FieldName::new(py, "raw"),
            Node::Bytes(BytesNode::new_const(2)),
        );
        // Byte[2] 简化为 SequenceNode 等价的简化形态：用 Bytes(2) 暂替
        // （实际生产中由 compile.rs 编译 Array(2, Byte) 为 Node::Array）
        let ints_field = UnionSubcon::new_named(
            FieldName::new(py, "ints"),
            Node::Bytes(BytesNode::new_const(2)),
        );
        UnionNode::new(vec![raw_field, ints_field], parsefrom, false)
    }

    #[test]
    fn parse_parsefrom_none_keeps_fallback() {
        // Union(None, raw=Bytes(2), ints=Bytes(2)).parse(b"AB")
        //      → dict(raw=b"AB", ints=b"AB")；stream 留在 0（fallback）
        with_py(|py| {
            let node = make_union_node(py, ParseFrom::None);
            let mut stream = ParseStream::new(b"AB");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let r = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let d = r.bind(py).downcast::<PyDict>().expect("dict");
            assert_eq!(
                d.get_item("raw")
                    .unwrap()
                    .unwrap()
                    .extract::<&[u8]>()
                    .unwrap(),
                b"AB"
            );
            assert_eq!(stream.tell(), 0, "parsefrom=None leaves stream at fallback");
        });
    }

    #[test]
    fn parse_parsefrom_index_seeks_forward() {
        // Union(0, ...).parse(b"AB") → stream seek 到 forwards[0]=2
        with_py(|py| {
            let node = make_union_node(py, ParseFrom::Index(0));
            let mut stream = ParseStream::new(b"AB");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert_eq!(stream.tell(), 2, "parsefrom=0 seeks to forwards[0]=2");
        });
    }

    #[test]
    fn parse_parsefrom_name() {
        // Union("ints", ...).parse(b"AB") → stream seek 到 forwards[1]=2
        with_py(|py| {
            let parsefrom = ParseFrom::Name {
                name: PyString::new_bound(py, "ints").unbind(),
                resolved_index: 1,
            };
            let node = make_union_node(py, parsefrom);
            let mut stream = ParseStream::new(b"AB");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert_eq!(stream.tell(), 2, "parsefrom='ints' seeks to forwards[1]=2");
        });
    }

    #[test]
    fn parse_parsefrom_index_out_of_range_raises() {
        // Union(99, ...).parse → Union error
        with_py(|py| {
            let node = make_union_node(py, ParseFrom::Index(99));
            let mut stream = ParseStream::new(b"AB");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Union { .. }));
        });
    }

    #[test]
    fn build_first_matching() {
        // build(dict(raw=b"AB")) → build raw subcon → b"AB"
        with_py(|py| {
            let node = make_union_node(py, ParseFrom::None);
            let obj = py.eval_bound("dict(raw=b'AB')", None, None).expect("dict");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), b"AB");
        });
    }

    #[test]
    fn build_unknown_name_raises() {
        // build(dict(unknown=1)) → Union error
        with_py(|py| {
            let node = make_union_node(py, ParseFrom::None);
            let obj = py.eval_bound("dict(unknown=1)", None, None).expect("dict");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Union { .. }));
        });
    }

    #[test]
    fn build_non_dict_raises() {
        // build(None) → Union error
        with_py(|py| {
            let node = make_union_node(py, ParseFrom::None);
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Union { .. }));
        });
    }

    #[test]
    fn sizeof_always_raises() {
        // sizeof → Err（SizeofError 等价）
        with_py(|py| {
            let node = make_union_node(py, ParseFrom::None);
            let ctx = Context::placeholder(py);
            let err = node.sizeof(&ctx).expect_err("should fail");
            assert!(matches!(err, ConstructError::Union { .. }));
        });
    }

    #[test]
    fn debug_format_includes_union_node() {
        with_py(|py| {
            let node = make_union_node(py, ParseFrom::None);
            let s = format!("{:?}", node);
            assert!(s.contains("UnionNode"), "got: {}", s);
        });
    }

    // 避免 unused 警告：fmt_node 在 make_union_node 中未直接用，但保留供未来扩展。
    #[test]
    fn _ensure_fmt_node_used() {
        with_py(|_py| {
            let _ = fmt_node(PythonFormat::UnsignedInt8Big);
        });
    }
}
