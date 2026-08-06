//! SequenceNode：位置序字段序列节点。
//!
//! 设计依据：`docs/design/模块设计/模块设计-Phase8-P1P2.md` §4。
//! Python 参考：`construct/construct/core.py` `Sequence`（L2329-2487）。
//!
//! ## 行为概述
//!
//! `Sequence(*subcons)` 是位置序字段序列，parse 返回 PyList（按 subcons 顺序），
//! build 接收 list/iterable。与 StructNode 平行但 sink 是 PyList（非实例 __dict__）。
//!
//! - parse：context nesting；空 PyList；遍历 fields：field.parse → append；
//!   命名字段写入 child_ctx；StopField 哨兵捕获 break；返回 PyList
//! - build：context nesting；obj 是 list；遍历 fields：next(iter) 取元素；
//!   命名字段前置写入；RO 字段走 compute_ro_value（不从 list 取，C-1）；
//!   StopField 哨兵捕获 break
//! - sizeof：sum 字段 sizeof（context nesting 仅影响字段引用，sizeof 用父 ctx）
//!
//! ## C-1：RO 字段不从 list 取值（parity 差异，设计 §4.8 SQ-7）
//!
//! Python core.py L2410 对所有 subcons 都 `next(objiter)`，含 RO 字段（Check/Computed
//! 等），故 Python 用户需传 `[1, None, 2]`（list 含 RO 字段占位 None）。
//! construct-rs RO 字段走 compute_ro_value（与 StructNode 同模式），list 不含占位——
//! 用户传 `[1, 2]` 即可。**用户从 Python 迁移需调整 build 输入**。

use crate::context::Context;
use crate::error::ConstructError;
use crate::nodes::struct_node::{FieldMode, FieldName};
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;
use pyo3::types::PyList;

use super::{Construct, Node};

/// Sequence 字段（name 可选 + node + field_kind）。
#[derive(Debug)]
pub struct SequenceField {
    /// 字段名（None 表示匿名字段）。
    pub name: Option<FieldName>,
    /// 子节点。
    pub node: Node,
    /// 字段模式（ro/rw；与 StructNode FieldMode 同脉络，仅 Rw/Ro 使用）。
    /// RO 字段（Check/Computed/Tell/Rebuild 等）在 build 时不从 list 取值，
    /// 走 compute_ro_value（C-1）。
    pub field_kind: FieldMode,
}

impl SequenceField {
    /// 创建命名的 Rw SequenceField。
    pub fn new_named(name: FieldName, node: Node) -> Self {
        Self {
            name: Some(name),
            node,
            field_kind: FieldMode::Rw,
        }
    }

    /// 创建命名的 SequenceField（指定 field_kind）。
    pub fn new_named_with_kind(name: FieldName, node: Node, kind: FieldMode) -> Self {
        Self {
            name: Some(name),
            node,
            field_kind: kind,
        }
    }

    /// 创建匿名的 Rw SequenceField。
    pub fn new_anonymous(node: Node) -> Self {
        Self {
            name: None,
            node,
            field_kind: FieldMode::Rw,
        }
    }
}

/// 位置序字段序列节点：parse 产出 PyList（按 subcons 顺序）。
///
/// 对应 Python construct `Sequence(*subcons)`（core.py L2329）。
/// 详见模块级注释。
#[derive(Debug)]
pub struct SequenceNode {
    /// 有序字段列表。
    fields: Vec<SequenceField>,
    /// 是否含表达式（编译期递归子树计算）。
    has_expressions: bool,
}

impl SequenceNode {
    /// 创建 `SequenceNode`。
    pub fn new(fields: Vec<SequenceField>, has_expressions: bool) -> Self {
        Self {
            fields,
            has_expressions,
        }
    }

    /// 返回字段列表切片。
    pub fn fields(&self) -> &[SequenceField] {
        &self.fields
    }

    /// 返回是否含表达式。
    pub fn has_expressions(&self) -> bool {
        self.has_expressions
    }
}

impl Construct for SequenceNode {
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
            child_ctx.init_expr_values(self.fields.len());
        }

        // 2. PyList（ListContainer 等价：construct-rs 返回普通 list）
        let list = PyList::empty_bound(py);

        // 3. 遍历 fields
        for (idx, field) in self.fields.iter().enumerate() {
            match field.node.parse(py, stream, &mut child_ctx, path) {
                Ok(val) => {
                    list.append(val)?;
                    if let Some(name) = &field.name {
                        // 命名字段：把 list 中刚 append 的元素借用写入 child_ctx。
                        let last_idx = list.len() - 1;
                        let borrowed =
                            list.get_item(last_idx)
                                .map_err(|e| ConstructError::Generic {
                                    message: format!("Sequence parse: list get item failed: {}", e),
                                    path: path.to_string(),
                                })?;
                        child_ctx.set_field_at(idx, name.py_name(), &borrowed, py)?;
                    }
                }
                Err(ConstructError::StopField { .. }) => break, // StopIf 触发，正常终止
                Err(e) => return Err(e),
            }
        }
        Ok(list.into_py(py))
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
            child_ctx.init_expr_values(self.fields.len());
        }

        // obj 是 list（Python Sequence.build 接受任意 iterable，construct-rs 要求 list）。
        let obj_list = obj
            .downcast::<PyList>()
            .map_err(|_| ConstructError::Generic {
                message: format!(
                    "Sequence build expects a list, got {}",
                    obj.get_type()
                        .name()
                        .map(|s| s.to_string())
                        .unwrap_or_default()
                ),
                path: path.to_string(),
            })?;

        let mut iter = obj_list.iter().peekable();
        for (idx, field) in self.fields.iter().enumerate() {
            // C-1：RO 字段不从 list 取值（走 compute_ro_value），list 不含占位。
            let build_obj: Py<PyAny> = if field.field_kind == FieldMode::Ro {
                field.node.compute_ro_value(py, stream, &child_ctx, path)?
            } else {
                match iter.next() {
                    Some(item) => item.unbind(),
                    None => {
                        return Err(ConstructError::Generic {
                            message: "Sequence build: list shorter than fields".to_string(),
                            path: path.to_string(),
                        })
                    }
                }
            };
            // 命名字段前置写入 child_ctx（与 StructNode build 同模式）。
            if let Some(name) = &field.name {
                child_ctx.set_field_at(idx, name.py_name(), build_obj.bind(py), py)?;
            }
            // build
            match field
                .node
                .build(py, build_obj.bind(py), stream, &mut child_ctx, path)
            {
                Ok(()) => {}
                Err(ConstructError::StopField { .. }) => break,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // sizeof 无 py token，无法 new_child。直接 sum（与 FocusedSeq sizeof 同处理）。
        let mut total = 0usize;
        for field in &self.fields {
            total = total.checked_add(field.node.sizeof(ctx)?).ok_or_else(|| {
                ConstructError::Generic {
                    message: "Sequence sizeof overflow".to_string(),
                    path: String::new(),
                }
            })?;
        }
        Ok(total)
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

    fn fmt_node(fmt: PythonFormat) -> Node {
        Node::FormatField(FormatFieldNode::new(fmt))
    }

    /// 构造 Sequence(Byte, Byte) 测试 fixture。
    fn make_simple_sequence(py: Python<'_>) -> SequenceNode {
        let fields = vec![
            SequenceField::new_anonymous(fmt_node(PythonFormat::UnsignedInt8Big)),
            SequenceField::new_anonymous(fmt_node(PythonFormat::UnsignedInt8Big)),
        ];
        SequenceNode::new(fields, false)
    }

    #[test]
    fn parse_returns_list() {
        // SQ-1: Sequence(Byte, Byte).parse(b'\x01\x02') → [1, 2]
        with_py(|py| {
            let node = make_simple_sequence(py);
            let mut stream = ParseStream::new(&[0x01, 0x02]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let r = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = r.bind(py).downcast::<PyList>().expect("list");
            assert_eq!(list.len(), 2);
            assert_eq!(list.get_item(0).unwrap().extract::<i64>().unwrap(), 1);
            assert_eq!(list.get_item(1).unwrap().extract::<i64>().unwrap(), 2);
        });
    }

    #[test]
    fn build_from_list() {
        // SQ-2: Sequence(Byte, Byte).build([1, 2]) → b'\x01\x02'
        with_py(|py| {
            let node = make_simple_sequence(py);
            let obj = py.eval_bound("[1, 2]", None, None).expect("list");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x01, 0x02]);
        });
    }

    #[test]
    fn build_short_list_raises() {
        // SQ-8: Sequence(Byte, Byte).build([1]) → 错误
        with_py(|py| {
            let node = make_simple_sequence(py);
            let obj = py.eval_bound("[1]", None, None).expect("list");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Generic { .. }));
        });
    }

    #[test]
    fn build_long_list_uses_prefix() {
        // SQ-9: Sequence(Byte, Byte).build([1, 2, 3]) → b'\x01\x02'（多余忽略）
        with_py(|py| {
            let node = make_simple_sequence(py);
            let obj = py.eval_bound("[1, 2, 3]", None, None).expect("list");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x01, 0x02]);
        });
    }

    #[test]
    fn build_non_list_raises() {
        // SQ-10: build(None) → 错误
        with_py(|py| {
            let node = make_simple_sequence(py);
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Generic { .. }));
        });
    }

    #[test]
    fn sizeof_returns_sum() {
        // SQ-11: Sequence(Byte, Byte).sizeof() → 2
        with_py(|py| {
            let node = make_simple_sequence(py);
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).unwrap(), 2);
        });
    }

    #[test]
    fn build_with_named_field_writes_context() {
        // SQ-3: Sequence("count"/Byte, Byte).build([3, 99])
        // 这里第二个字段用 Bytes(count) 表达式会触发 has_expressions=true，
        // 但简化测试仅验证命名字段不破坏 build 流程。
        with_py(|py| {
            let fields = vec![
                SequenceField::new_named(
                    FieldName::new(py, "count"),
                    fmt_node(PythonFormat::UnsignedInt8Big),
                ),
                SequenceField::new_anonymous(fmt_node(PythonFormat::UnsignedInt8Big)),
            ];
            let node = SequenceNode::new(fields, false);
            let obj = py.eval_bound("[3, 99]", None, None).expect("list");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[3, 99]);
        });
    }

    #[test]
    fn debug_format_includes_sequence_node() {
        with_py(|py| {
            let node = make_simple_sequence(py);
            let s = format!("{:?}", node);
            assert!(s.contains("SequenceNode"), "got: {}", s);
        });
    }
}
