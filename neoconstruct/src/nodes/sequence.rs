//! SequenceNode：位置序字段序列节点。
//!
//! ## 行为概述
//!
//! `Sequence(*subcons)` 是位置序字段序列，parse 返回 PyList（按 subcons 顺序），
//! build 接收 list/iterable。与 StructNode 平行但 sink 是 PyList（非实例 __dict__）。
//!
//! - parse：context nesting；空 PyList；遍历 fields：field.parse → append；
//!   命名字段写入 child_ctx；StopField 哨兵捕获 break；返回 PyList
//! - build：context nesting；obj 是 list；遍历 fields 按 [`ValueKind`] resolve——
//!   Instance 从 list 位置取值（`iter.next()`，None 视为缺值），Conditional
//!   消费 list 元素但 None 透传（分支自治），其余分类产各自的有效值
//!   （Const 补常量/Default 求值/Tell 流位置/Control 哑值 None
//!   等，不消费 list 元素）；命名字段写 child_ctx；StopField 哨兵捕获 break
//! - sizeof：sum 字段 sizeof（context nesting 仅影响字段引用，sizeof 用父 ctx）
//!
//! ## RO 字段不从 list 取值（与 StructNode 的 Instance 取值入口差异）
//!
//! 值来源分类（[`ValueKind`]）与 StructNode 完全一致，仅 Instance 分支的
//! 取值入口不同：StructNode 从实例 getattr，SequenceNode 从 list 位置访问。
//! Conditional 分支同样消费 list 位置（parse 侧无条件 append 全部字段值，
//! build 侧按位置对称取回；None 透传，元素缺位 ≡ None）。值提供型分类
//! （Const/Default/Tell/Control 等）的字段不要求 list 提供元素——list 仅需
//! 覆盖 Instance 与 Conditional 字段（宽松方向：多余元素忽略）。

use crate::context::Context;
use crate::error::ConstructError;
use crate::nodes::struct_node::{FieldMode, FieldName, ValueKind};
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;
use pyo3::types::PyList;

use super::{Construct, Node};

/// Sequence 字段（name 可选 + node + field_kind + 值来源分类）。
#[derive(Debug)]
pub struct SequenceField {
    /// 字段名（None 表示匿名字段）。
    pub name: Option<FieldName>,
    /// 子节点。
    pub node: Node,
    /// 字段模式（ro/rw；parse 存储语义，与 StructNode FieldMode 同脉络）。
    pub field_kind: FieldMode,
    /// build 值来源分类（与 StructNode 一致；Instance 从 list 位置取值）。
    pub value_kind: ValueKind,
}

impl SequenceField {
    /// 创建命名的 Rw SequenceField（值来源分类按节点推导，无表达式派生）。
    pub fn new_named(py: Python<'_>, name: FieldName, node: Node) -> Self {
        Self {
            name: Some(name),
            value_kind: ValueKind::classify(py, &node, false),
            node,
            field_kind: FieldMode::Rw,
        }
    }

    /// 创建命名的 SequenceField（指定 field_kind，值来源分类按节点推导）。
    pub fn new_named_with_kind(
        py: Python<'_>,
        name: FieldName,
        node: Node,
        kind: FieldMode,
    ) -> Self {
        Self {
            name: Some(name),
            value_kind: ValueKind::classify(py, &node, false),
            node,
            field_kind: kind,
        }
    }

    /// 创建匿名的 Rw SequenceField（值来源分类按节点推导）。
    pub fn new_anonymous(py: Python<'_>, node: Node) -> Self {
        Self {
            name: None,
            value_kind: ValueKind::classify(py, &node, false),
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

        // 2. PyList（ListContainer 等价：neoconstruct 返回普通 list）
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

        // obj 是 list（Python Sequence.build 接受任意 iterable，neoconstruct 要求 list）。
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
            // resolve：Instance 从 list 位置取值（与 StructNode 的 getattr
            // 入口不同），其余分类产各自有效值（不消费 list 元素）。
            let build_obj: Bound<'_, PyAny> = match &field.value_kind {
                ValueKind::Instance { .. } => match iter.next() {
                    Some(item) => {
                        if item.is_none() {
                            // list 元素 None ≡ 缺值（与 StructNode 实例属性
                            // None 同语义，错误时机＝build 值使用点）。
                            let mut err = ConstructError::BuildValueMissing {
                                field: field
                                    .name
                                    .as_ref()
                                    .map(|n| n.rust_name().to_string())
                                    .unwrap_or_else(|| format!("sequence[{}]", idx)),
                                path: path.to_string(),
                            };
                            if let Some(n) = &field.name {
                                err.push_path_segment(n.rust_name());
                            }
                            return Err(err);
                        }
                        item
                    }
                    None => {
                        return Err(ConstructError::Generic {
                            message: "Sequence build: list shorter than fields".to_string(),
                            path: path.to_string(),
                        })
                    }
                },
                ValueKind::Const(c) => c.bind(py).to_owned().into_any(),
                ValueKind::Default(p) | ValueKind::Rebuild(p) | ValueKind::Computed(p) => {
                    let n = crate::expr::eval_expr_int(p, &child_ctx, py)?;
                    n.into_py(py).into_bound(py)
                }
                ValueKind::Tell => stream.tell().into_py(py).into_bound(py),
                ValueKind::Conditional => match iter.next() {
                    // 条件根（IfThenElse/Switch/Select）：list 元素透传（含 None——
                    // parse 侧 Pass 分支产 None 元素，round-trip 位置对称）；
                    // 元素缺位 ≡ None（None ≡ 缺席，来源无关）。分支自治：
                    // Pass 分支不写字节；实值分支收到 None 由分支节点自然报错。
                    Some(item) => item,
                    None => py.None().into_bound(py),
                },
                ValueKind::Void | ValueKind::Control => py.None().into_bound(py),
                ValueKind::Index => child_ctx.index().into_py(py).into_bound(py),
            };
            // 命名字段前置写入 child_ctx（与 StructNode build 同模式）。
            if let Some(name) = &field.name {
                child_ctx.set_field_at(idx, name.py_name(), &build_obj, py)?;
            }
            // build
            match field
                .node
                .build(py, &build_obj, stream, &mut child_ctx, path)
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
            SequenceField::new_anonymous(py, fmt_node(PythonFormat::UnsignedInt8Big)),
            SequenceField::new_anonymous(py, fmt_node(PythonFormat::UnsignedInt8Big)),
        ];
        SequenceNode::new(fields, false)
    }

    #[test]
    fn parse_returns_list() {
        // Sequence(Byte, Byte).parse(b'\x01\x02') → [1, 2]
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
        // Sequence(Byte, Byte).build([1, 2]) → b'\x01\x02'
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
        // Sequence(Byte, Byte).build([1]) → 错误
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
        // Sequence(Byte, Byte).build([1, 2, 3]) → b'\x01\x02'（多余忽略）
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
        // build(None) → 错误
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
        // Sequence(Byte, Byte).sizeof() → 2
        with_py(|py| {
            let node = make_simple_sequence(py);
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).unwrap(), 2);
        });
    }

    #[test]
    fn build_with_named_field_writes_context() {
        // Sequence("count"/Byte, Byte).build([3, 99])
        // 这里第二个字段用 Bytes(count) 表达式会触发 has_expressions=true，
        // 但简化测试仅验证命名字段不破坏 build 流程。
        with_py(|py| {
            let fields = vec![
                SequenceField::new_named(
                    py,
                    FieldName::new(py, "count"),
                    fmt_node(PythonFormat::UnsignedInt8Big),
                ),
                SequenceField::new_anonymous(py, fmt_node(PythonFormat::UnsignedInt8Big)),
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

    // ======================================================================
    // Conditional 元素（条件根：list 元素 None 透传，缺位 ≡ None）
    // ======================================================================

    /// 构造 Switch(99, {} 无命中, default=Pass) 根节点（条件根 → Conditional）。
    fn switch_pass_node() -> Node {
        use crate::nodes::pass::PassNode;
        use crate::nodes::switch::{SwitchKey, SwitchNode};
        Node::Switch(SwitchNode::new(
            SwitchKey::ConstInt(99),
            Vec::new(),
            Node::Pass(PassNode::new()),
        ))
    }

    /// 构造 If(x>0, Byte) 根节点（命名 x 为 field 0，else=Pass）。
    fn if_gt_zero_node() -> Node {
        use crate::expr::{ExprOp, ExprProgram};
        use crate::nodes::if_then_else::IfThenElseNode;
        use crate::nodes::pass::PassNode;
        use crate::nodes::stop_if::StopIfCondition;
        Node::IfThenElse(IfThenElseNode::new(
            StopIfCondition::Expr(ExprProgram::new(vec![
                ExprOp::GetInt(0),
                ExprOp::Const(0),
                ExprOp::Gt,
            ])),
            fmt_node(PythonFormat::UnsignedInt8Big),
            Node::Pass(PassNode::new()),
        ))
    }

    #[test]
    fn conditional_element_none_passthrough_pass_branch() {
        // Sequence(x/Byte, Switch→Pass).build([0, None])：None 元素透传，
        // Pass 分支不写字节 → b'\x00'。
        with_py(|py| {
            let fields = vec![
                SequenceField::new_named(
                    py,
                    FieldName::new(py, "x"),
                    fmt_node(PythonFormat::UnsignedInt8Big),
                ),
                SequenceField::new_anonymous(py, switch_pass_node()),
            ];
            let node = SequenceNode::new(fields, false);
            let obj = py.eval_bound("[0, None]", None, None).expect("list");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build should succeed (None passthrough)");
            assert_eq!(stream.as_bytes(), &[0x00]);
        });
    }

    #[test]
    fn conditional_element_missing_treated_as_none() {
        // list 短缺（元素缺位 ≡ None，来源无关）：[0] 同 [0, None] 语义。
        with_py(|py| {
            let fields = vec![
                SequenceField::new_named(
                    py,
                    FieldName::new(py, "x"),
                    fmt_node(PythonFormat::UnsignedInt8Big),
                ),
                SequenceField::new_anonymous(py, switch_pass_node()),
            ];
            let node = SequenceNode::new(fields, false);
            let obj = py.eval_bound("[0]", None, None).expect("list");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build should succeed (missing element ≡ None)");
            assert_eq!(stream.as_bytes(), &[0x00]);
        });
    }

    #[test]
    fn conditional_element_none_real_branch_natural_error() {
        // If(x>0, Byte)：x=0 → Pass；list [0, None] → Pass 不消费值 → OK；
        // 对照：x=1（cond 真）且元素 None → Byte 分支自然报错。
        with_py(|py| {
            let fields = vec![
                SequenceField::new_named(
                    py,
                    FieldName::new(py, "x"),
                    fmt_node(PythonFormat::UnsignedInt8Big),
                ),
                SequenceField::new_anonymous(py, if_gt_zero_node()),
            ];
            let node = SequenceNode::new(fields, true);

            // cond 真 + None → 分支节点自然报错
            let obj = py.eval_bound("[1, None]", None, None).expect("list");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("real branch with None should fail");
            assert!(
                matches!(err, ConstructError::FormatField { .. }),
                "expected FormatFieldError from branch node, got {:?}",
                err
            );

            // cond 假 + None → Pass 不写字节
            let obj2 = py.eval_bound("[0, None]", None, None).expect("list");
            let mut stream2 = BuildStream::new();
            let mut ctx2 = Context::new_root(py).expect("ctx");
            node.build(py, &obj2, &mut stream2, &mut ctx2, &mut path)
                .expect("Pass branch with None should succeed");
            assert_eq!(stream2.as_bytes(), &[0x00]);
        });
    }

    #[test]
    fn conditional_element_round_trip() {
        // parse 产 [x, c]（Pass 分支产 None 元素）→ build 回环对称。
        with_py(|py| {
            let fields = vec![
                SequenceField::new_named(
                    py,
                    FieldName::new(py, "x"),
                    fmt_node(PythonFormat::UnsignedInt8Big),
                ),
                SequenceField::new_anonymous(py, if_gt_zero_node()),
            ];
            let node = SequenceNode::new(fields, true);

            // cond 假：parse(b'\x00') → [0, None] → build 回 b'\x00'
            let mut stream = ParseStream::new(&[0x00]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let parsed = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let mut bstream = BuildStream::new();
            node.build(py, parsed.bind(py), &mut bstream, &mut ctx, &mut path)
                .expect("build from parse product");
            assert_eq!(bstream.as_bytes(), &[0x00]);

            // cond 真：parse(b'\x01\x7f') → [1, 0x7f] → build 回原字节
            let mut stream2 = ParseStream::new(&[0x01, 0x7F]);
            let mut ctx2 = Context::new_root(py).expect("ctx");
            let parsed2 = node
                .parse(py, &mut stream2, &mut ctx2, &mut path)
                .expect("parse");
            let mut bstream2 = BuildStream::new();
            node.build(py, parsed2.bind(py), &mut bstream2, &mut ctx2, &mut path)
                .expect("build from parse product");
            assert_eq!(bstream2.as_bytes(), &[0x01, 0x7F]);
        });
    }
}
