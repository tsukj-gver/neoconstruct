//! FocusedSeqNode：聚焦字段序列节点（context nesting + 返回单聚焦字段值）。
//!
//! Python 参考：`construct/construct/core.py` `FocusedSeq`（L3176-3267）。
//!
//! ## 行为概述
//!
//! `FocusedSeq(parsebuildfrom, subcons)` 是有序字段序列，parse 返回**单聚焦字段**
//! 的值（不是用户类实例），build 接收单值（仅聚焦字段从 obj 取值，其余传 None）。
//!
//! - parse：`Context::new_child` 创建嵌套 ctx（`_` 指向外层）→ 字段遍历 →
//!   返回 focus 字段的 parseret
//! - build：`Context::new_child` → 预置 focus 字段值 → 字段遍历（focus 传 obj，
//!   其余传 None）
//! - sizeof：`Context::new_child` 不可用（无 py token），sum 字段 sizeof
//!
//! ## 设计取舍：独立 Node（不复用 StructNode）
//!
//! FocusedSeq 独立实现的成本：context nesting 逻辑（`Context::new_child` +
//! 字段遍历 + set_field_at）~100 行，远小于复用 StructNode 引入的耦合复杂度。

use crate::context::Context;
use crate::error::ConstructError;
use crate::nodes::struct_node::FieldName;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

use super::{Construct, Node};

// ---------------------------------------------------------------------------
// FocusedSeqField
// ---------------------------------------------------------------------------

/// FocusedSeq 字段（与 StructField 平行但独立，避免 mode 字段——
/// FocusedSeq 字段无 RW/RO/WO 区分）。
#[derive(Debug)]
pub struct FocusedSeqField {
    /// 字段名（None 表示匿名字段，对应 Python `Renamed` 无名字段或直接 subcon）。
    name: Option<FieldName>,
    /// 子节点。
    node: Node,
}

impl FocusedSeqField {
    /// 创建一个命名的 `FocusedSeqField`。
    pub fn new_named(name: FieldName, node: Node) -> Self {
        Self {
            name: Some(name),
            node,
        }
    }

    /// 创建一个匿名的 `FocusedSeqField`。
    pub fn new_anonymous(node: Node) -> Self {
        Self { name: None, node }
    }

    /// 返回字段名引用（None 表示匿名字段）。
    pub fn name(&self) -> Option<&FieldName> {
        self.name.as_ref()
    }

    /// 返回子节点引用。
    pub fn node(&self) -> &Node {
        &self.node
    }
}

// ---------------------------------------------------------------------------
// FocusedSeqNode
// ---------------------------------------------------------------------------

/// 聚焦字段序列节点（对应 Python construct `FocusedSeq`，core.py L3176）。
///
/// # 三方法行为
///
/// ## parse（对齐 Python L3236-3246）
///
/// 1. [`Context::new_child`] 创建嵌套 context（`_` 指向外层）
/// 2. （若 has_expressions）`init_expr_values(n_fields)`
/// 3. 遍历 fields：
///    - `field.node.parse(...)` → 求值
///    - 若 `field.name` 非 None：`ctx.set_field_at(idx, name, value)` 写入 child ctx
///    - 若 `idx == focus_idx`：记录 `finalret = value`
/// 4. return `finalret`（聚焦字段的值）
///
/// ## build（对齐 Python L3248-3259）
///
/// 1. [`Context::new_child`] 创建嵌套 context
/// 2. （若 has_expressions）`init_expr_values(n_fields)`
/// 3. **预置** focus 字段值：`ctx.set_field_at(focus_idx, focus_name, obj)`
/// 4. 遍历 fields：
///    - 若 `idx == focus_idx`：调 `field.node.build(obj, ...)`
///    - 否则：调 `field.node.build(None, ...)`（非聚焦字段传 None）
/// 5. return `Ok(())`
///
/// ## sizeof（对齐 Python L3261-3267）
///
/// 直接在父 ctx 上 sum 字段 sizeof（sizeof 接口无 py token，无法 new_child）。
/// 字段大小依赖嵌套 ctx 时返回 Err（对齐 Python SizeofError）。
///
/// # context nesting
///
/// FocusedSeq 主动 new_child（与 StructRefNode 委托 StructNode 的隐式 nesting 不同）。
/// child ctx 的 `_` 指向外层 ctx（Python L3237：`Container(_ = context, ...)`）。
///
/// # has_expressions
///
/// 递归检查 fields 子树（与 Bitwise 同模式）。
#[derive(Debug)]
pub struct FocusedSeqNode {
    /// 有序字段列表。
    fields: Vec<FocusedSeqField>,
    /// 聚焦字段在 fields 中的索引（编译期从 parsebuildfrom 解析）。
    /// 编译期保证：`fields[focus_idx].name == Some(parsebuildfrom)`。
    focus_idx: usize,
    /// 该 FocusedSeq 是否含表达式（编译期计算，递归子树）。
    has_expressions: bool,
}

impl FocusedSeqNode {
    /// 创建 `FocusedSeqNode`，包含给定的字段列表、focus 索引与 has_expressions 标志。
    ///
    /// # 参数
    ///
    /// - `fields`：有序字段列表（focus_idx 索引的字段名必须匹配 parsebuildfrom）
    /// - `focus_idx`：聚焦字段索引（编译期保证有效）
    /// - `has_expressions`：是否含表达式（递归子树）
    pub fn new(fields: Vec<FocusedSeqField>, focus_idx: usize, has_expressions: bool) -> Self {
        Self {
            fields,
            focus_idx,
            has_expressions,
        }
    }

    /// 返回字段列表切片。
    pub fn fields(&self) -> &[FocusedSeqField] {
        &self.fields
    }

    /// 返回聚焦字段索引。
    pub fn focus_idx(&self) -> usize {
        self.focus_idx
    }

    /// 返回是否含表达式。
    pub fn has_expressions(&self) -> bool {
        self.has_expressions
    }
}

impl Construct for FocusedSeqNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 1. context nesting（Python L3237: Container(_ = context, ...)）
        let mut child_ctx = Context::new_child(ctx, py)?;
        if self.has_expressions {
            child_ctx.init_expr_values(self.fields.len());
        }

        // 2. 遍历 fields
        let mut finalret: Option<Py<PyAny>> = None;
        for (idx, field) in self.fields.iter().enumerate() {
            let parseret = field.node.parse(py, stream, &mut child_ctx, path)?;
            if let Some(name) = &field.name {
                // 命名字段写入 child ctx（与 StructNode 同模式）
                child_ctx.set_field_at(idx, name.py_name(), parseret.bind(py), py)?;
            }
            if idx == self.focus_idx {
                finalret = Some(parseret);
            }
        }

        // 3. 返回 focus 字段值（编译期保证 focus_idx 有效，finalret 必为 Some）。
        finalret.ok_or_else(|| ConstructError::Generic {
            message: "FocusedSeq focus field not executed (compiler invariant violated)"
                .to_string(),
            path: path.to_string(),
        })
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

        // 预置 focus 字段值（Python L3252: context[parsebuildfrom] = obj）。
        let focus_field = &self.fields[self.focus_idx];
        if let Some(name) = &focus_field.name {
            child_ctx.set_field_at(self.focus_idx, name.py_name(), obj, py)?;
        }

        for (idx, field) in self.fields.iter().enumerate() {
            // focus 字段传 obj，其余传 None（对齐 Python L3254）。
            // 注意：py.None().bind(py) 是临时借用，需用 let 绑定延长生命周期。
            let none_obj = py.None();
            let none_bound = none_obj.bind(py);
            let build_obj = if idx == self.focus_idx {
                obj
            } else {
                none_bound
            };
            field
                .node
                .build(py, build_obj, stream, &mut child_ctx, path)?;
        }
        Ok(())
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // sizeof 接口无 py token，无法 new_child（需创建 PyDict）。
        // 替代：直接在父 ctx 上 sum sizeof（context nesting 仅影响字段间引用，
        // 不影响静态 sizeof——Python L3265 也是 sum(sc._sizeof for sc in subcons)）。
        // 字段大小依赖嵌套 ctx 时通过 ExprProgram 求值失败抛 Err。
        let mut total: usize = 0;
        for field in &self.fields {
            total = total.checked_add(field.node.sizeof(ctx)?).ok_or_else(|| {
                ConstructError::Generic {
                    message: "FocusedSeq sizeof overflow".to_string(),
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
    use crate::nodes::bytes::BytesNode;
    use crate::nodes::format_field::{FormatFieldNode, PythonFormat};
    use crate::nodes::padding::PaddingNode;
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

    /// 构造测试场景：FocusedSeq("num", Const(b"SIG"), "num"/Byte, Terminated)。
    /// 由于 Const/Terminated 在 neoconstruct 中可能未直接提供，这里用近似替代：
    /// - 用 Bytes(3)（"SIG"）作为匿名字段
    /// - "num"/Byte 作为 focus 字段
    /// - Padding(0) 作为末尾字段（近似 Terminated 的 no-op）
    fn fs1_node(py: Python<'_>) -> FocusedSeqNode {
        let fields = vec![
            // 匿名 Const(b"SIG")：用 Bytes(3) 近似（不写入 ctx）
            FocusedSeqField::new_anonymous(Node::Bytes(BytesNode::new_const(3))),
            // "num" / Byte：focus 字段
            FocusedSeqField::new_named(
                crate::nodes::struct_node::FieldName::new(py, "num"),
                fmt_node(PythonFormat::UnsignedInt8Big),
            ),
            // 匿名 Terminated：用 Padding(0) 近似
            FocusedSeqField::new_anonymous(Node::Padding(PaddingNode::new_const(0, 0))),
        ];
        // focus 索引 = 1（"num" 字段）
        FocusedSeqNode::new(fields, 1, false)
    }

    // ======================================================================
    // 构造器
    // ======================================================================

    #[test]
    fn new_stores_fields_and_focus_idx() {
        with_py(|py| {
            let node = fs1_node(py);
            assert_eq!(node.fields().len(), 3);
            assert_eq!(node.focus_idx(), 1);
        });
    }

    #[test]
    fn focused_seq_field_named_vs_anonymous() {
        with_py(|py| {
            let named = FocusedSeqField::new_named(
                crate::nodes::struct_node::FieldName::new(py, "x"),
                fmt_node(PythonFormat::UnsignedInt8Big),
            );
            assert!(named.name().is_some());

            let anon = FocusedSeqField::new_anonymous(fmt_node(PythonFormat::UnsignedInt8Big));
            assert!(anon.name().is_none());
        });
    }

    // ======================================================================
    // parse 基础场景
    // ======================================================================

    #[test]
    fn parse_returns_focus_field_value() {
        // FocusedSeq("num", Const(b"SIG"), "num"/Byte, Terminated)
        //       parse(b"SIG\xff") → 255（num 字段值）
        with_py(|py| {
            let node = fs1_node(py);
            let mut stream = ParseStream::new(b"SIG\xff");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().expect("i64");
            assert_eq!(v, 0xFF, "FocusedSeq returns focus field value");
        });
    }

    #[test]
    fn parse_focus_field_at_different_index() {
        // focus 在第一个字段时
        with_py(|py| {
            let fields = vec![FocusedSeqField::new_named(
                crate::nodes::struct_node::FieldName::new(py, "first"),
                fmt_node(PythonFormat::UnsignedInt8Big),
            )];
            let node = FocusedSeqNode::new(fields, 0, false);
            let mut stream = ParseStream::new(&[0x42]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let r = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = r.bind(py).extract().expect("i64");
            assert_eq!(v, 0x42);
        });
    }

    // ======================================================================
    // build 对称性
    // ======================================================================

    #[test]
    fn build_writes_all_fields_with_obj_at_focus() {
        // build: FocusedSeq.build(255) → b"SIG\xff"
        // 注：非 focus 字段传 None，Bytes(3).build(None) 会失败（None 不是 bytes）
        // 这里改用 Padding(3) 代替 Bytes(3)（Padding.build(None) 忽略 obj）
        with_py(|py| {
            let fields = vec![
                // 匿名 Padding(3)（取代 Const(b"SIG")，因 Bytes.build(None) 失败）
                FocusedSeqField::new_anonymous(Node::Padding(PaddingNode::new_const(3, 0))),
                // "num" / Byte：focus 字段
                FocusedSeqField::new_named(
                    crate::nodes::struct_node::FieldName::new(py, "num"),
                    fmt_node(PythonFormat::UnsignedInt8Big),
                ),
                // 匿名 Padding(0)
                FocusedSeqField::new_anonymous(Node::Padding(PaddingNode::new_const(0, 0))),
            ];
            let node = FocusedSeqNode::new(fields, 1, false);
            let obj = py.eval_bound("0xFF", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            // 3 字节 padding (0x00*3) + 1 字节 0xFF + 0 字节 padding
            assert_eq!(stream.as_bytes(), &[0x00, 0x00, 0x00, 0xFF]);
        });
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_returns_sum_of_field_sizes() {
        with_py(|py| {
            let node = fs1_node(py);
            let ctx = Context::new_root(py).expect("ctx");
            // Bytes(3) + Int8ub(1) + Padding(0) = 4
            assert_eq!(node.sizeof(&ctx).expect("sizeof"), 4);
        });
    }

    #[test]
    fn sizeof_with_padding_field() {
        with_py(|py| {
            let fields = vec![
                FocusedSeqField::new_named(
                    crate::nodes::struct_node::FieldName::new(py, "a"),
                    fmt_node(PythonFormat::UnsignedInt16Big),
                ),
                FocusedSeqField::new_anonymous(Node::Padding(PaddingNode::new_const(4, 0))),
            ];
            let node = FocusedSeqNode::new(fields, 0, false);
            let ctx = Context::new_root(py).expect("ctx");
            // Int16ub(2) + Padding(4) = 6
            assert_eq!(node.sizeof(&ctx).expect("sizeof"), 6);
        });
    }

    // ======================================================================
    // context nesting
    // ======================================================================

    #[test]
    fn parse_context_nesting_does_not_leak_to_parent() {
        // FocusedSeq 的 child ctx 字段不应泄漏到外层 parent ctx。
        with_py(|py| {
            let node = fs1_node(py);
            let mut stream = ParseStream::new(b"SIG\xff");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let _ = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // parent ctx 不应有 "num" 字段（FocusedSeq 在 child ctx 中写入）
            let parent_num = ctx.get_field("num").expect("get");
            assert!(
                parent_num.is_none(),
                "parent ctx should not contain FocusedSeq child fields"
            );
        });
    }

    // ======================================================================
    // Debug
    // ======================================================================

    #[test]
    fn debug_format_includes_focused_seq_node() {
        with_py(|py| {
            let node = fs1_node(py);
            let s = format!("{:?}", node);
            assert!(s.contains("FocusedSeqNode"), "got: {}", s);
        });
    }
}
