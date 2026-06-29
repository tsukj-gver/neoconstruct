//! GreedyRangeNode：读到流结束的数组读写。
//!
//! 设计依据：`docs/模块设计-Array.md` §4.2。
//! Python 参考：`construct/construct/core.py` `GreedyRange`（L2570-2634）。
//!
//! ## 概述
//!
//! `GreedyRange(subcon, discard=False)` 解析零或多个 `subcon` 元素到 Python list，
//! 直到流末尾或子构造器解析失败为止。
//!
//! - **parse 终止条件**：
//!   - 子构造器返回 `StopField`（StopIf 触发）：正常终止，seek 回 fallback。
//!   - 子构造器返回其他错误：seek 回 fallback，正常终止（吞掉错误，对齐 Python
//!     `except Exception` 路径，详见设计 §9.5 GE-1/GE-2）。
//!   - 流读到 EOF：子构造器返回 Stream 错误，按上一条规则回退终止。
//! - `discard=True` 时仍消耗流但不收集结果（返回空 list）。
//! - 嵌套数组（GreedyRange 内 GreedyRange）：内层 `_index` 覆盖外层，
//!   循环结束后恢复。
//! - `sizeof` 永远返回 `Err`（元素数量运行时未知）。
//!
//! ## 性能要点
//!
//! - 与 ArrayNode 不同，无法预知元素数量，PyList 动态增长（无预分配）。
//! - 每次迭代记录 `fallback = stream.tell()`（~1ns），失败时 seek 回退。
//! - `ctx.set_index` 是栈字段写入（~1ns）。
//! - `path.push_index/pop` 每次迭代调用（与 ArrayNode 一致）。

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;
use pyo3::types::{PyList, PyTuple};

// ---------------------------------------------------------------------------
// GreedyRangeNode
// ---------------------------------------------------------------------------

/// 读到流结束的数组节点。
///
/// 对应 Python construct `GreedyRange(subcon, discard)`（core.py L2570）。
///
/// # parse 行为
///
/// 1. 创建空 PyList（无法预知容量，动态增长）
/// 2. 保存外层 `ctx._index`（嵌套数组支持，设计 §3.2.2）
/// 3. `loop`：
///    - 记录 `fallback = stream.tell()`
///    - 设置 `ctx._index = i`、`path.push_index(i)`
///    - inner.parse：
///      - Ok(elem)：若 !discard，append 到 list；i += 1；继续
///      - Err(StopField)：seek 回 fallback，break（设计 §4.2.2）
///      - Err(其他)：seek 回 fallback，break（对齐 Python `except Exception`，
///        设计 §9.5 GE-1/GE-2）
/// 4. 恢复 `ctx._index` 到入口值
///
/// # build 行为
///
/// 1. 遍历 obj（list/tuple/任意 iterable）
/// 2. 对每个元素：设置 `ctx._index = i`，inner.build
///    - StopField：停止后续元素（对齐 Python L2627-2628）
///    - 其他错误：向上传播
/// 3. 恢复 `ctx._index`
///
/// # sizeof 行为
///
/// 永远返回 `Err`（对齐 Python L2630-2631）。
#[derive(Debug)]
pub struct GreedyRangeNode {
    /// 元素子树（递归 Box）。
    inner: Box<crate::nodes::Node>,
    /// 是否丢弃解析结果（仍消耗流）。
    discard: bool,
}

impl GreedyRangeNode {
    /// 创建 GreedyRangeNode。
    pub fn new(inner: crate::nodes::Node, discard: bool) -> Self {
        Self {
            inner: Box::new(inner),
            discard,
        }
    }

    /// 返回 inner 子树引用。
    pub fn inner(&self) -> &crate::nodes::Node {
        &self.inner
    }

    /// 是否丢弃解析结果。
    pub fn discard(&self) -> bool {
        self.discard
    }

    /// has_expressions 判断（设计 §6.1.1）。
    /// GreedyRange 无 count 表达式，仅递归检查 inner。
    pub fn has_expressions(&self) -> bool {
        self.inner.has_expressions()
    }
}

// ---------------------------------------------------------------------------
// Construct impl
// ---------------------------------------------------------------------------

impl super::Construct for GreedyRangeNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 无法预知容量，PyList 动态增长。
        let list = PyList::new_bound(py, Vec::<Py<PyAny>>::new());

        // 保存外层 _index（嵌套数组支持，设计 §3.2.2）。
        let old_index = ctx.index();

        let mut i: usize = 0;
        loop {
            // 记录 fallback 位置（用于子构造器失败时回退）。
            let fallback = stream.tell();

            ctx.set_index(i);
            path.push_index(i);

            match self.inner.parse(py, stream, ctx, path) {
                Ok(elem) => {
                    path.pop();
                    if !self.discard {
                        list.append(elem).map_err(ConstructError::from)?;
                    }
                    i += 1;
                }
                Err(ConstructError::StopField { .. }) => {
                    // StopIf 触发：正常终止（对齐 Python StopFieldError 捕获）。
                    // StopIf 不消耗字节，但仍 seek 回 fallback（防御性，对齐设计 §4.2.2）。
                    path.pop();
                    let _ = stream.seek(fallback, path);
                    break;
                }
                Err(e) => {
                    // 其他错误：seek 回退 + 正常终止。
                    // 对齐 Python L2609-2614 的 `except Exception` 路径
                    // （ExplicitError 暂无等价变体，统一回退，设计 §9.5 GE-1）。
                    path.pop();
                    let _ = e; // 错误丢弃（对齐 Python 语义）
                    let _ = stream.seek(fallback, path);
                    break;
                }
            }
        }

        restore_index(ctx, old_index);
        Ok(list.into_any().unbind())
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        // obj 必须是可迭代对象（list/tuple/任意 iterable）。
        // 优先快路径（list/tuple），回落到通用 iterable。
        let items: Vec<Py<PyAny>> = if let Ok(list) = obj.downcast::<PyList>() {
            list.iter().map(|b| b.unbind()).collect()
        } else if let Ok(tuple) = obj.downcast::<PyTuple>() {
            tuple.iter().map(|b| b.unbind()).collect()
        } else {
            // 兜底：尝试 iter() 收集。性能差，仅在用户传入非 list/tuple 时触发。
            let iter = obj.iter().map_err(|e| ConstructError::Generic {
                message: format!(
                    "GreedyRange build expects list/tuple, got {} (iter error: {})",
                    obj.get_type()
                        .name()
                        .map(|n| n.to_string())
                        .unwrap_or_else(|_| "<unknown>".to_string()),
                    e
                ),
                path: path.to_string(),
            })?;
            let collected: PyResult<Vec<Py<PyAny>>> =
                iter.map(|b| b.map(|bound| bound.unbind())).collect();
            collected.map_err(|e| ConstructError::Generic {
                message: format!("GreedyRange build iterable error: {}", e),
                path: path.to_string(),
            })?
        };

        let old_index = ctx.index();

        for (i, elem) in items.into_iter().enumerate() {
            ctx.set_index(i);
            path.push_index(i);
            let elem_bound = elem.bind(py);
            match self.inner.build(py, elem_bound, stream, ctx, path) {
                Ok(()) => {
                    path.pop();
                }
                Err(ConstructError::StopField { .. }) => {
                    // StopIf 触发：停止后续元素构建（对齐 Python L2627-2628）。
                    path.pop();
                    restore_index(ctx, old_index);
                    return Ok(());
                }
                Err(mut e) => {
                    path.pop();
                    e.push_path_segment(&format!("[{}]", i));
                    restore_index(ctx, old_index);
                    return Err(e);
                }
            }
        }

        restore_index(ctx, old_index);
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // 对齐 Python `GreedyRange._sizeof` L2630-2631：永远 SizeofError。
        // 设计 §4.2.4：保守用 Generic（错误消息明确）。
        Err(ConstructError::Generic {
            message: "GreedyRange size is undefined".to_string(),
            path: String::new(),
        })
    }
}

/// 恢复 ctx._index 到节点入口时的值。
///
/// `Some(idx)` → set_index(idx)；`None` → clear_index。
/// 设计 §3.2.2 嵌套数组语义。
#[inline]
fn restore_index(ctx: &mut Context<'_>, old: Option<usize>) {
    match old {
        Some(idx) => ctx.set_index(idx),
        None => ctx.clear_index(),
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

    /// 构造一个 Byte（Int8ub）节点，用于 GreedyRange 内部。
    fn byte_node() -> crate::nodes::Node {
        crate::nodes::Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big))
    }

    /// 构造一个 Int16ub 节点（2 字节），用于测试"部分元素失败回退"。
    fn int16ub_node() -> crate::nodes::Node {
        crate::nodes::Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big))
    }

    /// 在 ctx 中设置字段值（无表达式场景，仅占位）。
    fn setup_ctx<'py>(py: Python<'py>) -> Context<'py> {
        let mut ctx = Context::new_root(py).expect("ctx");
        ctx.init_expr_values(0);
        // 写一个占位字段，确保 expr_values_buf 至少有内容（GreedyRange 本身不需要）。
        let key = PyString::new_bound(py, "_placeholder").unbind();
        let val = 0i64.into_py(py);
        ctx.set_field_at(0, &key, val.bind(py), py)
            .expect("set_field_at");
        ctx
    }

    // ======================================================================
    // 构造器
    // ======================================================================

    #[test]
    fn new_creates_greedy_range_with_discard_false() {
        let node = GreedyRangeNode::new(byte_node(), false);
        assert!(!node.discard());
    }

    #[test]
    fn new_discard_true() {
        let node = GreedyRangeNode::new(byte_node(), true);
        assert!(node.discard());
    }

    #[test]
    fn has_expressions_inner_no_expr_returns_false() {
        let node = GreedyRangeNode::new(byte_node(), false);
        assert!(!node.has_expressions());
    }

    // ======================================================================
    // parse：基本场景
    // ======================================================================

    #[test]
    fn parse_empty_stream_returns_empty_list() {
        // GR-1: 空流（立即 EOF）→ 返回空 list
        with_py(|py| {
            let node = GreedyRangeNode::new(byte_node(), false);
            let mut stream = ParseStream::new(&[]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("is list");
            assert_eq!(list.len(), 0);
            assert_eq!(stream.tell(), 0);
            // 嵌套数组：循环结束后 _index 恢复 None
            assert!(ctx.index().is_none());
        });
    }

    #[test]
    fn parse_reads_all_elements_until_eof() {
        // 基本解析：读到 EOF
        with_py(|py| {
            let node = GreedyRangeNode::new(byte_node(), false);
            let mut stream = ParseStream::new(&[0x01, 0x02, 0x03, 0x04, 0x05]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("is list");
            assert_eq!(list.len(), 5);
            let values: Vec<i64> = list.iter().map(|b| b.extract().unwrap()).collect();
            assert_eq!(values, vec![1, 2, 3, 4, 5]);
            assert_eq!(stream.tell(), 5);
            assert!(stream.is_at_end());
        });
    }

    #[test]
    fn parse_single_element() {
        with_py(|py| {
            let node = GreedyRangeNode::new(byte_node(), false);
            let mut stream = ParseStream::new(&[0xAB]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("is list");
            assert_eq!(list.len(), 1);
            let v: i64 = list.get_item(0).unwrap().extract().unwrap();
            assert_eq!(v, 0xAB);
        });
    }

    // ======================================================================
    // parse：子构造器失败时回退（GR-2, GR-3, GR-5）
    // ======================================================================

    #[test]
    fn parse_first_element_fallback_returns_empty_list() {
        // GR-2: 第一个元素解析失败 → 回退到 pos=0，返回空 list
        // 用 Int16ub（2 字节）作为 inner，流只有 1 字节 → 第一次解析就失败
        with_py(|py| {
            let node = GreedyRangeNode::new(int16ub_node(), false);
            let mut stream = ParseStream::new(&[0xFF]); // 仅 1 字节，Int16ub 需要 2
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("is list");
            assert_eq!(list.len(), 0);
            // 回退到 pos=0
            assert_eq!(stream.tell(), 0);
        });
    }

    #[test]
    fn parse_partial_element_fallback_returns_previous_elements() {
        // GR-3/GR-5: 第 N 个元素解析失败 → 回退，返回前 N-1 个完整元素
        // 用 Int16ub：前 4 字节 = 2 个完整 Int16ub，第 5 字节不足
        with_py(|py| {
            let node = GreedyRangeNode::new(int16ub_node(), false);
            // 0x0102, 0x0304 是完整 Int16ub，0x05 是残留字节
            let mut stream = ParseStream::new(&[0x01, 0x02, 0x03, 0x04, 0x05]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("is list");
            assert_eq!(list.len(), 2);
            let v0: i64 = list.get_item(0).unwrap().extract().unwrap();
            let v1: i64 = list.get_item(1).unwrap().extract().unwrap();
            assert_eq!(v0, 0x0102);
            assert_eq!(v1, 0x0304);
            // 回退到第 3 个元素的起始位置（pos=4）
            assert_eq!(stream.tell(), 4);
        });
    }

    #[test]
    fn parse_does_not_set_index_after_completion() {
        // 循环结束后 _index 应被恢复为入口值
        with_py(|py| {
            let node = GreedyRangeNode::new(byte_node(), false);
            let mut stream = ParseStream::new(&[0x01, 0x02, 0x03]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let _ = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert!(ctx.index().is_none());
        });
    }

    // ======================================================================
    // parse：discard 行为（GR-6）
    // ======================================================================

    #[test]
    fn parse_discard_returns_empty_but_consumes_stream() {
        // GR-6: discard=True 仍消耗流，返回空 list
        with_py(|py| {
            let node = GreedyRangeNode::new(byte_node(), true);
            let mut stream = ParseStream::new(&[0x01, 0x02, 0x03, 0x04]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("is list");
            assert_eq!(list.len(), 0);
            assert_eq!(stream.tell(), 4);
        });
    }

    // ======================================================================
    // parse：嵌套 GreedyRange
    // ======================================================================
    // 注：`GreedyRange(GreedyRange(Byte))` 在 EOF 时会无限循环——内层 GreedyRange
    // 在 EOF 返回空 list（成功），外层看不到错误，无限 append 空 list。
    // 这与 Python construct 行为完全一致（Python 也会无限循环），属用户误用。
    // 正确的嵌套用法：`GreedyRange(Array(N, Byte))` 或 `Array(N, GreedyRange(Byte))`。

    #[test]
    fn parse_nested_greedy_range_inside_array_restores_outer_index() {
        // Array(GreedyRange(Byte))：每个外层元素是变长 list（终止于 EOF 或第 1 个元素失败）
        // 数据：4 字节，外层 Array(2)，每个内层 GreedyRange 读到 EOF（最后 1 个也是 EOF）
        //   - Array[0] = GreedyRange 读 pos 0..4 → [1,2,3,4]（4 字节全读完）
        //   - Array[1] = GreedyRange 读 pos 4..4（EOF） → []
        //   注意：GreedyRange 在 EOF 时返回空 list 是合法行为（与 Python 一致）。
        // 这里测试 Array 包 GreedyRange，外层 Array 控制循环次数（避免无限循环）。
        with_py(|py| {
            use crate::nodes::array::{ArrayNode, CountSource};
            let inner = GreedyRangeNode::new(byte_node(), false);
            let outer_node = crate::nodes::Node::Array(ArrayNode::new(
                crate::nodes::Node::GreedyRange(inner),
                CountSource::Const(2),
                false,
            ));
            let mut stream = ParseStream::new(&[0x01, 0x02, 0x03, 0x04]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = outer_node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let outer_list = result.bind(py).downcast::<PyList>().expect("outer list");
            assert_eq!(outer_list.len(), 2);
            // 第一个 inner：读所有 4 字节
            let inner0 = outer_list.get_item(0).unwrap();
            let inner0_list = inner0.downcast::<PyList>().expect("inner0 list");
            assert_eq!(inner0_list.len(), 4);
            let v0: i64 = inner0_list.get_item(0).unwrap().extract().unwrap();
            let v3: i64 = inner0_list.get_item(3).unwrap().extract().unwrap();
            assert_eq!(v0, 1);
            assert_eq!(v3, 4);
            // 第二个 inner：从 EOF 开始，空 list
            let inner1 = outer_list.get_item(1).unwrap();
            let inner1_list = inner1.downcast::<PyList>().expect("inner1 list");
            assert_eq!(inner1_list.len(), 0);
            // 外层 _index 恢复 None（ArrayNode 负责恢复）
            assert!(ctx.index().is_none());
        });
    }

    #[test]
    fn parse_nested_greedy_range_with_greedy_array_inside_array() {
        // GreedyRange 内嵌 Array：每个外层元素是固定长度 list
        // 用 6 字节数据：外层每次调 inner Array(2, Byte)，应得到 3 个 inner list
        with_py(|py| {
            use crate::nodes::array::{ArrayNode, CountSource};
            let inner = ArrayNode::new(byte_node(), CountSource::Const(2), false);
            let outer_node = crate::nodes::Node::GreedyRange(GreedyRangeNode::new(
                crate::nodes::Node::Array(inner),
                false,
            ));
            let mut stream = ParseStream::new(&[0x01, 0x02, 0x03, 0x04, 0x05, 0x06]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = outer_node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let outer_list = result.bind(py).downcast::<PyList>().expect("outer list");
            assert_eq!(outer_list.len(), 3);
            // 第一个 inner
            let inner0 = outer_list.get_item(0).unwrap();
            let inner0_list = inner0.downcast::<PyList>().expect("inner0 list");
            assert_eq!(inner0_list.len(), 2);
            let v: i64 = inner0_list.get_item(0).unwrap().extract().unwrap();
            assert_eq!(v, 1);
            // 第三个 inner
            let inner2 = outer_list.get_item(2).unwrap();
            let inner2_list = inner2.downcast::<PyList>().expect("inner2 list");
            let v: i64 = inner2_list.get_item(1).unwrap().extract().unwrap();
            assert_eq!(v, 6);
        });
    }

    // ======================================================================
    // parse：StopField 捕获（GR-4，需 StopIfNode）
    // ======================================================================
    // 注：完整的 StopIf 集成测试在 4.4 子任务（StopIfNode 实现）后补充。
    // 当前 GreedyRangeNode 已实现 StopField 捕获分支，但触发 StopField 需要 StopIfNode。
    // 这里通过手动构造错误变体验证（用 Computed 节点间接验证错误传播路径不可行，
    // 因 Computed 不会产生 StopField）。
    // → 集成测试留待 4.4。

    // ======================================================================
    // build
    // ======================================================================

    #[test]
    fn build_writes_list_elements_until_end() {
        with_py(|py| {
            let node = GreedyRangeNode::new(byte_node(), false);
            let obj = py.eval_bound("[10, 20, 30]", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[10, 20, 30]);
        });
    }

    #[test]
    fn build_writes_tuple_elements() {
        with_py(|py| {
            let node = GreedyRangeNode::new(byte_node(), false);
            let obj = py.eval_bound("(1, 2, 3)", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[1, 2, 3]);
        });
    }

    #[test]
    fn build_empty_list_writes_nothing() {
        // GR-7: build 时空列表 → 不写字节
        with_py(|py| {
            let node = GreedyRangeNode::new(byte_node(), false);
            let obj = py.eval_bound("[]", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert!(stream.as_bytes().is_empty());
        });
    }

    #[test]
    fn build_propagates_inner_error() {
        // GR-8: build 时某元素失败 → 错误向上传播（不回退，build 无 seek）
        // 用 Int16ub 作为 inner，传入 [1, 2, 3]，第一个就因类型不符失败
        with_py(|py| {
            let node = GreedyRangeNode::new(int16ub_node(), false);
            // Int16ub.build 接收 int，但 256 超出 u8 范围不会失败；用 bytes 制造类型错
            let obj = py.eval_bound("[b'bad']", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            // 错误应携带 [0] 路径段（GreedyRange build 错误路径附加）
            assert!(err.path().unwrap_or("").contains("[0]"));
        });
    }

    #[test]
    fn build_iterable_object_works() {
        // 兜底路径：传入 generator/iterable（非 list/tuple）
        with_py(|py| {
            let node = GreedyRangeNode::new(byte_node(), false);
            let obj = py
                .eval_bound("(x for x in [5, 6, 7])", None, None)
                .expect("eval generator");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[5, 6, 7]);
        });
    }

    #[test]
    fn build_restores_index_after_completion() {
        with_py(|py| {
            let node = GreedyRangeNode::new(byte_node(), false);
            let obj = py.eval_bound("[1, 2, 3]", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert!(ctx.index().is_none());
        });
    }

    // ======================================================================
    // parse ↔ build 往返
    // ======================================================================

    #[test]
    fn round_trip_greedy_range_preserves_data() {
        with_py(|py| {
            let node = GreedyRangeNode::new(byte_node(), false);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            // build
            let obj = py
                .eval_bound("[100, 110, 120, 130, 140]", None, None)
                .expect("eval");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();

            // parse 回来
            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("list");
            assert_eq!(list.len(), 5);
            let values: Vec<i64> = list.iter().map(|b| b.extract().unwrap()).collect();
            assert_eq!(values, vec![100, 110, 120, 130, 140]);
        });
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_always_returns_error() {
        // GR-9: sizeof 永远 Err
        with_py(|py| {
            let ctx = setup_ctx(py);
            let node = GreedyRangeNode::new(byte_node(), false);
            let err = node.sizeof(&ctx).expect_err("should fail");
            match err {
                ConstructError::Generic { message, .. } => {
                    assert!(message.contains("GreedyRange"), "got: {}", message);
                }
                other => panic!("expected Generic, got {:?}", other),
            }
        });
    }
}
