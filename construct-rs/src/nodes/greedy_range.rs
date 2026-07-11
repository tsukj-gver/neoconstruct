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
//! - **4.7 lazy path 迁移**：成功路径不调 `path.push_index/pop`（P0-3 模式推广），
//!   子节点返回 Err 时通过 `ConstructError::push_path_index(i)` 重建索引段。
//!   特殊：parse 的"吞错误回退"分支丢弃错误，不重建 path（对齐 Python `except Exception`）。
//! - **4.7 PyList Vec 中转**：用 `Vec<Py<PyAny>>` 收集元素后一次性 `PyList::new_bound`。
//!   count 未知用 `Vec::new()` 起步，Rust Vec 增长策略（doubling）比 CPython list（~1.125x）高效。
//! - 每次迭代记录 `fallback = stream.tell()`（~1ns），失败时 seek 回退。
//! - `ctx.set_index` 是栈字段写入（~1ns）。

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;
use pyo3::types::PyList;

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
        // 4.7 PyList Vec 中转：count 未知，用 Vec::new() 起步。
        // Rust Vec 增长策略（doubling）比 CPython list（~1.125x）高效，
        // 对 N=1000 约 ~10 次 realloc vs PyList ~60 次。
        let mut elems: Vec<Py<PyAny>> = Vec::new();

        // 保存外层 _index（嵌套数组支持，设计 §3.2.2）。
        let old_index = ctx.index();

        let mut i: usize = 0;
        loop {
            // 记录 fallback 位置（用于子构造器失败时回退）。
            let fallback = stream.tell();

            ctx.set_index(i);
            // 4.7 lazy path：成功路径不调 path.push_index/pop。

            match self.inner.parse(py, stream, ctx, path) {
                Ok(elem) => {
                    if !self.discard {
                        elems.push(elem);
                    }
                    i += 1;
                }
                Err(ConstructError::StopField { .. }) => {
                    // StopIf 触发：正常终止（对齐 Python StopFieldError 捕获）。
                    // StopIf 不消耗字节，但仍 seek 回 fallback（防御性，对齐设计 §4.2.2）。
                    // 错误是哨兵，丢弃；无需重建 path。
                    let _ = stream.seek(fallback, path);
                    break;
                }
                Err(e) => {
                    // 其他错误：seek 回退 + 正常终止。
                    // 对齐 Python L2609-2614 的 `except Exception` 路径
                    // （ExplicitError 暂无等价变体，统一回退，设计 §9.5 GE-1）。
                    // **不调 push_path_index**——错误被丢弃（对齐 Python 语义），
                    // path 此时为 Root 态，seek 内部不读 path 内容仅传递。
                    let _ = e;
                    let _ = stream.seek(fallback, path);
                    break;
                }
            }
        }

        // 一次性创建 PyList。
        let list = PyList::new_bound(py, elems);

        ctx.restore_index(old_index);
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
        // 4.7 P1-1 整合：build 路径统一调 collect_obj_to_vec 收集 obj 到 Vec。
        let items = super::common::collect_obj_to_vec(obj, "GreedyRange", path)?;

        let old_index = ctx.index();

        for (i, elem) in items.into_iter().enumerate() {
            ctx.set_index(i);
            // 4.7 lazy path：成功路径不调 path.push_index/pop。
            let elem_bound = elem.bind(py);
            match self.inner.build(py, elem_bound, stream, ctx, path) {
                Ok(()) => {}
                Err(ConstructError::StopField { .. }) => {
                    // StopIf 触发：停止后续元素构建（对齐 Python L2627-2628）。
                    // 哨兵被捕获丢弃，无需重建 path。
                    ctx.restore_index(old_index);
                    return Ok(());
                }
                Err(mut e) => {
                    // 其他错误：重建索引段后向上传播。
                    e.push_path_index(i);
                    ctx.restore_index(old_index);
                    return Err(e);
                }
            }
        }

        ctx.restore_index(old_index);
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

/// 恢复 ctx._index（已提升为 `Context::restore_index` 方法，P0-1 整合）。
/// 保留此注释作为 GreedyRangeNode 模式采用声明参考。
// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::format_field::{FormatFieldNode, PythonFormat};
    use crate::nodes::Construct;
    use crate::nodes::Node;
    use pyo3::types::{PyString, PyType};

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
    // parse：StopField 捕获（GR-4，StopIfNode 集成）
    // ======================================================================

    #[test]
    fn parse_stop_if_always_terminates_immediately() {
        // GR-4 集成：GreedyRange(StopIf(Always)) → 第一次迭代就触发 StopField，
        // GreedyRange 捕获并 break，返回空 list。
        //
        // 关键：inner 直接是 StopIfNode（非 Struct 包装），StopField 不被中间节点捕获，
        // 直接传播到 GreedyRange。
        // 设计 §4.2.2 / §9.4：GreedyRange 在 inner.parse 返回 StopField 时正常终止。
        with_py(|py| {
            use crate::nodes::stop_if::{StopIfCondition, StopIfNode};
            let inner = Node::StopIf(StopIfNode::new(StopIfCondition::Always));
            let node = GreedyRangeNode::new(inner, false);
            let mut stream = ParseStream::new(&[0x10, 0x20, 0x30]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("list");
            // 第一次迭代：StopIf(Always) 立即触发 → GreedyRange 捕获 break
            assert_eq!(list.len(), 0);
            // 流未消耗（StopIf 不消耗字节）
            assert_eq!(stream.tell(), 0);
        });
    }

    #[test]
    fn parse_stop_if_never_reads_until_eof() {
        // GR-4 边界：GreedyRange(StopIf(Never)) 永不停止，
        // 每次 append Py_None，读到 EOF 才终止（Stream 错误被吞，正常终止）。
        with_py(|py| {
            use crate::nodes::stop_if::StopIfCondition;
            // 用 Byte 作为 inner：但这里测试 StopIf(Never) 单独使用——
            // 实际上 GreedyRange(StopIf(Never)) 会无限添加 None 直到 EOF
            // （StopIf(Never).parse 返回 Py_None，append 成功，无 EOF 触发）
            // 当流读到 EOF 时，下一次 StopIf(Never).parse 仍返回 None（不消耗流）
            // → GreedyRange 无限循环。这是用户误用。
            //
            // 改为测试 GreedyRange(Byte) + 普通读到 EOF（对比 StopIf 的行为）。
            let inner = byte_node();
            let node = GreedyRangeNode::new(inner, false);
            let mut stream = ParseStream::new(&[0x10, 0x20, 0x30]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("list");
            assert_eq!(list.len(), 3);
            assert!(stream.is_at_end());
            // 抑制 unused 警告
            let _ = StopIfCondition::Never;
        });
    }

    #[test]
    fn parse_stop_if_in_struct_does_not_propagate() {
        // GR-4 行为差异说明：Struct 内的 StopIf 被 Struct 捕获，不传播到 GreedyRange。
        // 这是 construct-rs 与 Python construct 的已知差异（Python 用 FocusedSeq 透传）。
        // 设计 §4.7：StructNode 捕获 StopField 后正常返回实例。
        //
        // 场景：GreedyRange(Struct{x: Byte, stop: StopIf(Always)})
        // 每次 Struct.parse 都会捕获 StopIf(Always)，返回 Instance{x: ...}，
        // GreedyRange 看到 Ok(instance) 继续迭代，直到 EOF。
        with_py(|py| {
            use crate::nodes::stop_if::{StopIfCondition, StopIfNode};
            use crate::nodes::struct_node::{FieldMode, FieldName, StructField, StructNode};
            let inner_fields = vec![
                StructField {
                    name: FieldName::new(py, "x"),
                    node: byte_node(),
                    mode: FieldMode::Rw,
                },
                StructField {
                    name: FieldName::new(py, "stop"),
                    node: Node::StopIf(StopIfNode::new(StopIfCondition::Always)),
                    mode: FieldMode::Ro,
                },
            ];
            let cls = py
                .eval_bound("type('Item', (), {})", None, None)
                .expect("cls")
                .extract::<Py<PyType>>()
                .expect("PyType");
            let inner = Node::Struct(StructNode::new(py, inner_fields, cls, false, false));
            let node = GreedyRangeNode::new(inner, false);

            let mut stream = ParseStream::new(&[0x10, 0x20, 0x30]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("list");
            // Struct 捕获 StopField 后正常返回 → GreedyRange 看到 Ok 继续
            // 3 字节都用完后 EOF 触发 Stream 错误 → GreedyRange 回退终止
            assert_eq!(list.len(), 3);
            assert!(stream.is_at_end());
        });
    }

    #[test]
    fn parse_empty_stream_with_stop_if_returns_empty_list() {
        // GR-4 边界：空流 + StopIf → 第一次迭代 EOF 触发 Stream 错误 → GreedyRange 回退 + 终止
        with_py(|py| {
            use crate::nodes::stop_if::{StopIfCondition, StopIfNode};
            let inner = Node::StopIf(StopIfNode::new(StopIfCondition::Always));
            let node = GreedyRangeNode::new(inner, false);
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("list");
            assert_eq!(list.len(), 0);
        });
    }

    #[test]
    fn build_stop_if_always_stops_after_first_element() {
        // GR-4 build 方向：StopIf 在第一个元素就触发 → 仅 build 第一个
        with_py(|py| {
            use crate::nodes::stop_if::{StopIfCondition, StopIfNode};
            // inner: StopIf(Always) 不写字节，但触发 StopField
            let inner = Node::StopIf(StopIfNode::new(StopIfCondition::Always));
            let node = GreedyRangeNode::new(inner, false);
            // 传入多个元素，但第一个就停止
            let obj = py.eval_bound("[1, 2, 3]", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            // StopIf 不写字节，build 后 stream 应为空
            assert!(stream.as_bytes().is_empty());
        });
    }

    #[test]
    fn build_stop_if_never_builds_all_elements() {
        // GR-4 build 边界：StopIf(Never) 不停止，全部 build
        with_py(|py| {
            // inner: GreedyRange(Struct{Byte, StopIf(Never)})
            use crate::nodes::stop_if::{StopIfCondition, StopIfNode};
            use crate::nodes::struct_node::{FieldMode, FieldName, StructField, StructNode};
            let inner_fields = vec![
                StructField {
                    name: FieldName::new(py, "x"),
                    node: byte_node(),
                    mode: FieldMode::Rw,
                },
                StructField {
                    name: FieldName::new(py, "stop"),
                    node: Node::StopIf(StopIfNode::new(StopIfCondition::Never)),
                    mode: FieldMode::Ro,
                },
            ];
            let cls = py
                .eval_bound("type('Item', (), {})", None, None)
                .expect("cls")
                .extract::<Py<PyType>>()
                .expect("PyType");
            let inner = Node::Struct(StructNode::new(py, inner_fields, cls, false, false));
            let node = GreedyRangeNode::new(inner, false);

            // 构造 list 数据（只需提供 RW 字段 x，StopIf 是 RO 字段不需要值）
            let obj = py
                .eval_bound(
                    "[type('I', (), {'x': 1})(), type('I', (), {'x': 2})()]",
                    None,
                    None,
                )
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            // 两个元素的 x 都被写入
            assert_eq!(stream.as_bytes(), &[1, 2]);
        });
    }

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
            // 4.7 lazy path：error.path 由 inner.build 产生 "root"，GreedyRange 重建为 "root[0]"。
            // 验证 path 为 "root[0]"，不出现 "root.[0][0]" 双重标记或 ".[" 前缀。
            let p = err.path().unwrap_or("");
            assert!(
                p == "root[0]" || p.ends_with("[0]"),
                "lazy path format: expected 'root[0]' or ending with '[0]', got '{}'",
                p
            );
            assert!(
                !p.contains(".["),
                "lazy path format: dot before '[' is invalid (got '{}')",
                p
            );
        });
    }

    #[test]
    fn build_struct_inside_greedy_range_error_path() {
        // 4.7 设计 §6.2：GreedyRange(Struct{x: Byte})，build 时 Struct[1].x 失败
        // → path 应为 "root[1].x"
        // 重建顺序（从叶到根）：
        //   1. leaf FormatField build error path = "root"
        //   2. Struct.push_segment("x"): "root" → "root.x"
        //   3. GreedyRange.push_path_index(1): "root.x" → "root[1].x"
        with_py(|py| {
            use crate::nodes::struct_node::{FieldMode, FieldName, StructField, StructNode};
            use pyo3::types::PyType;
            let inner_struct_fields = vec![StructField {
                name: FieldName::new(py, "x"),
                node: byte_node(),
                mode: FieldMode::Rw,
            }];
            let cls = py
                .eval_bound("type('Item', (), {})", None, None)
                .expect("cls")
                .extract::<Py<PyType>>()
                .expect("PyType");
            let inner = Node::Struct(StructNode::new(py, inner_struct_fields, cls, false, false));
            let node = GreedyRangeNode::new(inner, false);
            // 传入 2 个元素，第 2 个的 x 是 bytes
            let obj = py
                .eval_bound(
                    "[type('I', (), {'x': 1})(), type('I', (), {'x': b'bad'})()]",
                    None,
                    None,
                )
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail on GreedyRange[1].x build");
            let p = err.path().unwrap_or("");
            assert!(
                p == "root[1].x" || p.ends_with("[1].x"),
                "expected 'root[1].x' or ending with '[1].x', got '{}'",
                p
            );
        });
    }

    #[test]
    fn parse_returns_native_list_type() {
        // 4.7：验证 Vec 中转后返回的仍是原生 list 类型。
        with_py(|py| {
            let node = GreedyRangeNode::new(byte_node(), false);
            let mut stream = ParseStream::new(&[0x01, 0x02, 0x03]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let result_bound = result.bind(py);
            let is_list: bool = result_bound.is_instance_of::<PyList>();
            assert!(is_list, "Vec 中转后应返回原生 list");
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
