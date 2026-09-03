//! ElementNode：RepeatUntil 终止表达式中"当前元素"引用入口。
//!
//! Python 参考无（construct-rs 新增——Python construct 用 lambda 参数 `x`
//! 引用当前元素，construct-rs 用 Element 字段 + 字段名引用机制）。
//!
//! ## 概述
//!
//! `Element` 是 construct-rs 中 RepeatUntil 终止表达式引用"当前元素"的机制。
//! 与 Index（引用"当前下标"）平行——两者都遵循"字段名即引用、所有引用统一走
//! `_FieldDescriptor` + GetInt"哲学。
//!
//! - parse 返回 `Py_None`（Element 字段不持有真实数据——其值由 RepeatUntilNode
//!   在迭代时通过 `ctx.set_expr_value_raw/set_expr_value_py(element_field_idx, elem)` 借用设置）
//! - build 是 no-op（sizeof=0，不写字节）
//! - sizeof 返回 0
//! - has_expressions 返回 false（ElementNode 自身不引用 Struct 字段，仅作为引用入口）
//!
//! ## 用户访问 RepeatUntil 当前元素的机制
//!
//! | 用户需求 | construct-rs 写法 | 编译结果 |
//! |---------|------------------|---------|
//! | 在 RepeatUntil 终止表达式中引用当前元素 | `e: int = rfield(Element()); RepeatUntil(e > 5, Int8ub)` | `[GetInt(idx_of_e), Const(5), Gt]` |
//!
//! 设计取向（与 Element 一致性）：不引入 `ExprOp::GetElem` /
//! `_current_elem_ptr` 等表达式内"元素引用"机制。终止表达式统一通过 Element
//! 字段 + `GetInt(idx_of_e)` 取值（与其他字段引用完全同模式）。
//!
//! ## 与 IndexNode 的关系
//!
//! ElementNode 与 IndexNode 同构——唯一的实现差异是值生命周期：
//! - IndexNode：值由 IndexNode.parse 在 Struct 进入时一次性写入 ctx（单次 parse 周期）
//! - ElementNode：值由 RepeatUntilNode 在每次迭代时主动 set_expr_value_raw/
//!   set_expr_value_py 覆盖（RepeatUntil 单次迭代周期，借用模式）
//!
//! 这是实现细节，不影响用户面一致性。

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

// ---------------------------------------------------------------------------
// ElementNode
// ---------------------------------------------------------------------------

/// RepeatUntil 当前元素引用入口节点。
///
/// 对应 construct-rs 用户面 `Element()`。Python construct 无对应物——Python 用
/// lambda 参数 `x` 引用当前元素，construct-rs 用 Element 字段 + 字段名引用机制。
///
/// # parse 行为
///
/// 始终返回 `Py_None`——ElementNode.parse 在用户面不可见。Element 字段的值
/// 由 [`crate::nodes::repeat_until::RepeatUntilNode`] 在迭代时通过
/// `ctx.set_expr_value_raw/set_expr_value_py(element_field_idx, elem)` 借用设置，
/// 终止表达式通过 `GetInt(element_field_idx)` 取值。
///
/// # build 行为
///
/// no-op（sizeof=0，不写字节）。
///
/// # sizeof 行为
///
/// 返回 0（不消耗字节）。
#[derive(Debug, Default, Clone, Copy)]
pub struct ElementNode;

impl ElementNode {
    /// 创建 ElementNode。
    pub fn new() -> Self {
        Self
    }

    /// has_expressions 判断。
    ///
    /// 返回 `false`——ElementNode 自身不引用 Struct 字段（仅作为引用入口）。
    /// RepeatUntilNode 的 has_expressions 始终返回 true，覆盖 Element 字段的
    /// init_expr_values 需求。
    pub fn has_expressions(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Construct impl
// ---------------------------------------------------------------------------

impl super::Construct for ElementNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        _stream: &mut ParseStream<'_>,
        _ctx: &mut Context<'py>,
        _path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 始终返回 Py_None。Element 字段的值由 RepeatUntilNode 借用设置，
        // 不在 StructNode.parse 处理 Element 字段时取值。
        Ok(py.None())
    }

    fn build(
        &self,
        _py: Python<'_>,
        _obj: &Bound<'_, PyAny>,
        _stream: &mut BuildStream,
        _ctx: &mut Context<'_>,
        _path: &mut Path,
    ) -> Result<(), ConstructError> {
        // no-op（sizeof=0，不写字节）。
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // 对齐 IndexNode：返回 0（不消耗字节）。
        Ok(0)
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
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

    // ======================================================================
    // 构造器
    // ======================================================================

    #[test]
    fn new_creates_element_node() {
        let _node = ElementNode::new();
    }

    #[test]
    fn default_creates_element_node() {
        let _node = ElementNode;
    }

    #[test]
    fn has_expressions_returns_false() {
        let node = ElementNode::new();
        assert!(!node.has_expressions());
    }

    // ======================================================================
    // parse 在 RepeatUntil 之外（独立 Struct）返回 Py_None
    // ======================================================================

    #[test]
    fn parse_returns_none_in_standalone_context() {
        // ElementNode.parse 在 RepeatUntil 之外返回 Py_None。
        // Element 字段不持有真实数据——其值由 RepeatUntil 借用设置。
        with_py(|py| {
            let node = ElementNode::new();
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert!(result.bind(py).is_none(), "should return Py_None");
        });
    }

    #[test]
    fn parse_returns_none_on_placeholder_context() {
        with_py(|py| {
            let node = ElementNode::new();
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert!(result.bind(py).is_none());
        });
    }

    // ======================================================================
    // sizeof 返回 0
    // ======================================================================

    #[test]
    fn sizeof_returns_zero() {
        with_py(|py| {
            let ctx = Context::new_root(py).expect("ctx");
            let node = ElementNode::new();
            assert_eq!(node.sizeof(&ctx).expect("sizeof"), 0);
        });
    }

    #[test]
    fn sizeof_returns_zero_on_placeholder() {
        with_py(|py| {
            let ctx = Context::placeholder(py);
            let node = ElementNode::new();
            assert_eq!(node.sizeof(&ctx).expect("sizeof"), 0);
        });
    }

    // ======================================================================
    // build 是 no-op
    // ======================================================================

    #[test]
    fn build_is_noop() {
        // build 不写字节，不消费 obj。
        with_py(|py| {
            let node = ElementNode::new();
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert!(
                stream.as_bytes().is_empty(),
                "ElementNode build should not write bytes"
            );
        });
    }

    #[test]
    fn build_ignores_obj_value() {
        // build 忽略 obj 内容。
        with_py(|py| {
            let node = ElementNode::new();
            let obj = py.eval_bound("12345", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert!(stream.as_bytes().is_empty());
        });
    }

    #[test]
    fn build_does_not_require_index_in_context() {
        // 边界：build 时即使 ctx._index 为 None 也不报错。
        with_py(|py| {
            let node = ElementNode::new();
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build should succeed without _index");
        });
    }

    // ======================================================================
    // 不消耗字节验证
    // ======================================================================

    #[test]
    fn parse_does_not_consume_stream() {
        // ElementNode 不消耗字节（sizeof=0）。
        with_py(|py| {
            let node = ElementNode::new();
            let mut stream = ParseStream::new(&[0x01, 0x02, 0x03]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            let _ = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert_eq!(stream.tell(), 0, "ElementNode should not advance stream");
        });
    }

    // ======================================================================
    // round-trip（parse/build 对称性）
    // ======================================================================

    #[test]
    fn round_trip_element_returns_none_and_writes_nothing() {
        // ElementNode round-trip：parse 返回 None，build 不写字节。
        with_py(|py| {
            let node = ElementNode::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            // parse: 返回 None
            let mut stream = ParseStream::new(b"");
            let parsed = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert!(parsed.bind(py).is_none());

            // build: no-op，stream 仍为空
            let obj = parsed.bind(py);
            let mut bstream = BuildStream::new();
            node.build(py, obj, &mut bstream, &mut ctx, &mut path)
                .expect("build");
            assert!(bstream.as_bytes().is_empty());
        });
    }
}
