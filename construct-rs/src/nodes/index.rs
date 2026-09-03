//! IndexNode：取当前数组迭代下标。
//!
//! 设计依据：`docs/模块设计-Array.md` §4.4（v3 决策：直接调 `ctx.index()`）。
//! Python 参考：`construct/construct/core.py` `Index`（L2934-2972）。
//!
//! ## 概述
//!
//! `Index` 是 construct-rs 中**唯一**的用户面下标访问机制。
//! parse 时从 `ctx._index`（栈分配字段）读取当前数组迭代下标，返回 PyLong；
//! 不在数组中时返回 Py_None（对齐 Python `context.get("_index", None)`）。
//!
//! - sizeof = 0（不消耗流）
//! - build 是 no-op（不写字节）
//! - 不走 ExprProgram（v3 决策删除 ExprOp::GetIndex，§3.3）
//!
//! ## 用户访问下标的机制（设计 §3.3 v3 决策）
//!
//! | 用户需求 | construct-rs 写法 | 编译结果 |
//! |---------|------------------|---------|
//! | 取当前下标值（作为字段） | `i: int = rfield(Index())` | IndexNode.parse 读 `ctx.index()` |
//! | 在表达式中引用下标 | 先声明 Index 字段，再用字段名引用 | `[GetInt(idx_of_i), ...]` |
//!
//! v3 决策：不扩展 ExprOp（不引入 GetIndex）。用户在表达式中引用下标时，
//! 先用 Index 字段声明（如 `i: int = rfield(Index())`），再用字段名 `i`
//! 参与表达式（如 `Bytes(i + 1)`），编译为 `[GetInt(0), Const(1), Add]`。
//! 这与 Phase 2「字段名即引用」的设计一致。

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

// ---------------------------------------------------------------------------
// IndexNode
// ---------------------------------------------------------------------------

/// 取当前数组迭代下标的节点。
///
/// 对应 Python construct `Index`（core.py L2934）。
///
/// construct-rs 中 IndexNode 是用户访问数组下标的唯一机制（v3 决策，§3.3）。
/// 直接调 [`Context::index`] 读取，不走 ExprProgram。
///
/// # parse 行为
///
/// 对齐 Python `Index._parse`（core.py L2965-2966）：
/// ```python
/// def _parse(self, stream, context, path):
///     return context.get("_index", None)
/// ```
/// - `ctx.index() == Some(i)` → 返回 `PyLong(i)`
/// - `ctx.index() == None` → 返回 `Py_None`（不在数组迭代中）
///
/// # build 行为
///
/// 对齐 Python `Index._build`（core.py L2968-2969）：build 是 no-op（不写字节），
/// IndexNode 的 sizeof=0。Python 返回 `context._index` 但 build 结果不影响输出。
///
/// # sizeof 行为
///
/// 对齐 Python `Index._sizeof`（core.py L2971-2972）：返回 0（不消耗字节）。
#[derive(Debug, Default, Clone, Copy)]
pub struct IndexNode;

impl IndexNode {
    /// 创建 IndexNode。
    pub fn new() -> Self {
        Self
    }
}

// ---------------------------------------------------------------------------
// Construct impl
// ---------------------------------------------------------------------------

impl super::Construct for IndexNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        _stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        _path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 对齐 Python `context.get("_index", None)`：
        // 在 Array 系列节点内 → Some(i) → PyLong
        // 不在 Array 内 → None → Py_None
        Ok(ctx.index().into_py(py))
    }

    fn build(
        &self,
        _py: Python<'_>,
        _obj: &Bound<'_, PyAny>,
        _stream: &mut BuildStream,
        _ctx: &mut Context<'_>,
        _path: &mut Path,
    ) -> Result<(), ConstructError> {
        // 对齐 Python `Index._build`：no-op（sizeof=0，不写字节）。
        // Python 的 `_build` 返回 `context._index`，但 IndexNode 不参与 build 输出。
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // 对齐 Python `Index._sizeof` 返回 0（IX-3）。
        Ok(0)
    }
}

impl IndexNode {
    /// has_expressions 判断（设计 §6.1.1）。
    ///
    /// IndexNode 仅读 `ctx._index`（v3：不走 ExprProgram），不引用 Struct 字段。
    /// 返回 `false`——不需要触发 StructNode 的 init_expr_values 路径。
    pub fn has_expressions(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::Construct;
    use pyo3::types::PyList;

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
    // 构造器与访问器
    // ======================================================================

    #[test]
    fn new_creates_index_node() {
        let _node = IndexNode::new();
    }

    #[test]
    fn default_creates_index_node() {
        let _node = IndexNode;
    }

    #[test]
    fn has_expressions_returns_false() {
        let node = IndexNode::new();
        assert!(!node.has_expressions());
    }

    // ======================================================================
    // IX-1：在 Array 内返回当前下标
    // ======================================================================

    #[test]
    fn parse_returns_index_when_set() {
        // IX-1: ctx.index() = Some(i) → 返回 PyLong(i)
        with_py(|py| {
            let node = IndexNode::new();
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            ctx.set_index(42);
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().expect("extract i64");
            assert_eq!(v, 42);
        });
    }

    #[test]
    fn parse_returns_zero_index() {
        // IX-1 边界：index = 0 也是合法值（不与 None 混淆）
        with_py(|py| {
            let node = IndexNode::new();
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            ctx.set_index(0);
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // 必须是 PyLong(0)，不是 None
            assert!(
                !result.bind(py).is_none(),
                "0 should not be confused with None"
            );
            let v: i64 = result.bind(py).extract().expect("extract i64");
            assert_eq!(v, 0);
        });
    }

    #[test]
    fn parse_returns_large_index() {
        // IX-1 边界：大下标（PyLong）
        with_py(|py| {
            let node = IndexNode::new();
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            ctx.set_index(1_000_000);
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().expect("extract i64");
            assert_eq!(v, 1_000_000);
        });
    }

    // ======================================================================
    // IX-2：不在 Array 内返回 Py_None
    // ======================================================================

    #[test]
    fn parse_returns_none_when_not_in_array() {
        // IX-2: ctx.index() = None → 返回 Py_None（对齐 Python `context.get("_index", None)`）
        with_py(|py| {
            let node = IndexNode::new();
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            // 默认 _index = None（new_root 不设置）
            assert!(ctx.index().is_none());

            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert!(result.bind(py).is_none(), "should return Py_None");
        });
    }

    #[test]
    fn parse_returns_none_after_clear_index() {
        // IX-2: clear_index 后返回 None
        with_py(|py| {
            let node = IndexNode::new();
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            ctx.set_index(5);
            ctx.clear_index();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert!(result.bind(py).is_none());
        });
    }

    #[test]
    fn parse_returns_none_on_placeholder_context() {
        // IX-2: placeholder context 也没有 _index
        with_py(|py| {
            let node = IndexNode::new();
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
    // IX-3：sizeof 返回 0
    // ======================================================================

    #[test]
    fn sizeof_returns_zero() {
        // IX-3: sizeof = 0
        with_py(|py| {
            let ctx = Context::new_root(py).expect("ctx");
            let node = IndexNode::new();
            assert_eq!(node.sizeof(&ctx).expect("sizeof"), 0);
        });
    }

    #[test]
    fn sizeof_returns_zero_on_placeholder() {
        with_py(|py| {
            let ctx = Context::placeholder(py);
            let node = IndexNode::new();
            assert_eq!(node.sizeof(&ctx).expect("sizeof"), 0);
        });
    }

    // ======================================================================
    // IX-4：build 是 no-op
    // ======================================================================

    #[test]
    fn build_is_noop() {
        // IX-4: build 不写字节，不消费 obj
        with_py(|py| {
            let node = IndexNode::new();
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert!(
                stream.as_bytes().is_empty(),
                "IndexNode build should not write bytes"
            );
        });
    }

    #[test]
    fn build_ignores_obj_value() {
        // IX-4: build 忽略 obj 内容（对齐 Python `Index._build`：返回 _index 但不写字节）
        with_py(|py| {
            let node = IndexNode::new();
            // 即使 obj 是任意值，build 也不写字节
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
        // IX-4 边界：build 时即使 ctx._index 为 None 也不报错
        with_py(|py| {
            let node = IndexNode::new();
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
        // IndexNode 不消耗字节（sizeof=0）
        with_py(|py| {
            let node = IndexNode::new();
            let mut stream = ParseStream::new(&[0x01, 0x02, 0x03]);
            let mut ctx = Context::new_root(py).expect("ctx");
            ctx.set_index(0);
            let mut path = Path::new();

            let _ = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert_eq!(stream.tell(), 0, "IndexNode should not advance stream");
        });
    }

    // ======================================================================
    // round-trip（parse/build 对称性）
    // ======================================================================

    #[test]
    fn round_trip_index_in_array_context() {
        // 模拟 Array 内 Index 字段：parse 读下标，build 不写字节
        with_py(|py| {
            let node = IndexNode::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            // parse: 模拟 Array 迭代设置 _index
            let mut stream = ParseStream::new(b"");
            ctx.set_index(7);
            let parsed = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = parsed.bind(py).extract().expect("i64");
            assert_eq!(v, 7);

            // build: no-op，stream 仍为空
            let obj = parsed.bind(py);
            let mut bstream = BuildStream::new();
            node.build(py, obj, &mut bstream, &mut ctx, &mut path)
                .expect("build");
            assert!(bstream.as_bytes().is_empty());
        });
    }

    // ======================================================================
    // 集成：Array 内 Index 字段（模拟场景）
    // ======================================================================

    #[test]
    fn parse_in_simulated_array_loop_returns_indices() {
        // 模拟 Array(3, Index)：3 次迭代，每次返回当前下标
        with_py(|py| {
            let node = IndexNode::new();
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            let results = PyList::new_bound(py, Vec::<Py<PyAny>>::new());
            let old_index = ctx.index();
            for i in 0..3usize {
                ctx.set_index(i);
                let v = node
                    .parse(py, &mut stream, &mut ctx, &mut path)
                    .expect("parse");
                results.append(v).expect("append");
            }
            match old_index {
                Some(idx) => ctx.set_index(idx),
                None => ctx.clear_index(),
            }

            // 验证返回 [0, 1, 2]
            assert_eq!(results.len(), 3);
            for i in 0..3 {
                let v: i64 = results
                    .get_item(i)
                    .expect("get")
                    .extract()
                    .expect("extract");
                assert_eq!(v, i as i64);
            }
        });
    }

    #[test]
    fn parse_in_nested_array_returns_inner_index() {
        // 模拟 Array(N, Array(M, Index))：内层 _index 覆盖外层
        // 内层每次迭代返回内层下标 j，外层迭代结束后恢复
        with_py(|py| {
            let node = IndexNode::new();
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            // 外层 Array(2, ...)
            let outer_old = ctx.index();
            for i in 0..2usize {
                ctx.set_index(i);
                // 内层 Array(3, Index)
                let inner_old = ctx.index();
                for j in 0..3usize {
                    ctx.set_index(j);
                    let v = node
                        .parse(py, &mut stream, &mut ctx, &mut path)
                        .expect("parse");
                    let n: i64 = v.bind(py).extract().expect("extract");
                    // 内层 Index 读到的是内层下标 j（覆盖外层 i）
                    assert_eq!(n, j as i64, "inner should see j={} (i={})", j, i);
                }
                match inner_old {
                    Some(idx) => ctx.set_index(idx),
                    None => ctx.clear_index(),
                }
                // 内层结束后，外层下标恢复
                assert_eq!(ctx.index(), Some(i), "outer should be restored to i={}", i);
            }
            match outer_old {
                Some(idx) => ctx.set_index(idx),
                None => ctx.clear_index(),
            }
        });
    }

    #[test]
    fn parse_in_child_context_inherits_parent_index() {
        // 模拟 Array 内 Struct 字段：子 context 通过 new_child 继承父的 _index
        // 对应设计 §3.2.2 选项 A
        with_py(|py| {
            let node = IndexNode::new();
            let mut stream = ParseStream::new(b"");
            let mut parent = Context::new_root(py).expect("parent ctx");
            let mut path = Path::new();

            parent.set_index(5);
            let mut child = Context::new_child(&parent, py).expect("child ctx");
            // 子 context 应继承父的 _index
            assert_eq!(child.index(), Some(5));

            // IndexNode 在子 context 中读到继承的 _index
            let result = node
                .parse(py, &mut stream, &mut child, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().expect("extract i64");
            assert_eq!(v, 5);
        });
    }

    #[test]
    fn parse_in_child_placeholder_inherits_parent_index() {
        // 同上，但用 new_child_placeholder（StructRef 路径）
        with_py(|py| {
            let node = IndexNode::new();
            let mut stream = ParseStream::new(b"");
            let mut parent = Context::new_root(py).expect("parent ctx");
            let mut path = Path::new();

            parent.set_index(9);
            let mut child = Context::new_child_placeholder(&parent);
            assert_eq!(child.index(), Some(9));

            let result = node
                .parse(py, &mut stream, &mut child, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().expect("extract i64");
            assert_eq!(v, 9);
        });
    }
}
