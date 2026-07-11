//! RepeatUntilNode：终止表达式数组读写（v5 重写）。
//!
//! 设计依据：`docs/模块设计-Array.md` §4.3（v5 完全重写）+ §2.5 决策 A5。
//! Python 参考：`construct/construct/core.py` `RepeatUntil`（L2637-2704）。
//!
//! ## v5 概述
//!
//! `RepeatUntil(terminator, subcon, discard)` 解析元素到 list 直到终止表达式
//! 求值非零（最后元素被包含），或从 list 构建字节序列直到某元素满足终止表达式。
//!
//! ## v5 关键变更（vs v4）
//!
//! - **删除 PyCallable 路径**：不再接收 Python lambda/callable（违反"一次 FFI"
//!   核心原则 + "白名单+兜底"二分设计）
//! - **终止表达式 = Phase 2 ExprProgram**：编译期从用户面表达式编译，运行时
//!   Rust 内部栈式 VM 求值（零 FFI）
//! - **Element 字段借用模式**：用户声明 `e: int = rfield(Element())`，RepeatUntil
//!   在迭代时 `ctx.set_field_at(element_field_idx, elem, ...)` 借用 Element 字段
//!   槽位，终止表达式中的 `GetInt(element_field_idx)` 即取到此值
//! - **不引入 ExprOp::GetElem / _current_elem_ptr**（v4 删除，v5 不恢复）
//!
//! ## parse 流程（对齐 Python L2670-2682）
//!
//! 1. 创建空 Vec（4.7 PyList Vec 中转模式）
//! 2. 保存外层 `ctx._index`（嵌套数组支持，设计 §3.2.2）
//! 3. `loop`：
//!    - 设置 `ctx._index = i`（4.7 lazy path：成功路径不调 path.push_index/pop）
//!    - inner.parse → elem（失败直接上抛，RU-1，不像 GreedyRange 回退）
//!    - 若 `!discard`，push elem 到 Vec
//!    - `ctx.set_field_at(element_field_idx, elem_name, elem, py)` 借用 Element 字段槽位
//!    - 求值终止表达式（GetInt(element_field_idx) 从 expr_values_buf 取到 elem）
//!    - 非零 → 终止（最后元素已包含在 Vec 中，RU-2）；零 → 继续迭代
//! 4. 一次性 `PyList::new_bound` + 恢复 `ctx._index`
//!
//! ## build 流程（对齐 Python L2684-2701）
//!
//! 1. 收集 obj（list/tuple/iterable）到 Vec（调 collect_obj_to_vec）
//! 2. 遍历元素：
//!    - inner.build(e) 写入 stream
//!    - `ctx.set_field_at(element_field_idx, elem_name, e, py)` 借用 Element 字段槽位
//!    - 求值终止表达式：非零 → matched=true，break
//! 3. 若 !matched → `Repeat` 错误（RU-3）
//!
//! ## sizeof
//!
//! 永远返回 `Err`（对齐 Python L2703-2704）。
//!
//! ## 模式采用声明（强制）
//!
//! 本节点（RepeatUntilNode）采用以下跨阶段已验证模式：
//! - ✅ P0-3 lazy path 错误传播（成功路径不维护 Path）
//! - ✅ _index save/restore 配对（调 `ctx.restore_index(old)`）
//! - ✅ Vec 中转 PyList（一次性 `PyList::new_bound`）
//!
//! 已查阅共享层：`nodes/common.rs` 的 `collect_obj_to_vec` / `obj_type_name`。

use crate::context::Context;
use crate::error::ConstructError;
use crate::expr::{eval_expr_int, ExprProgram};
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;
use pyo3::types::PyList;
use pyo3::types::PyString;

// ---------------------------------------------------------------------------
// RepeatUntilNode（v5 数据结构）
// ---------------------------------------------------------------------------

/// 终止表达式数组节点（v5 重写）。
///
/// 对应 Python construct `RepeatUntil(terminator, subcon, discard)`。
///
/// construct-rs 用户面 API（v5）：
/// ```python
/// RepeatUntil(terminator, subcon, discard=False)
/// ```
/// `terminator` 是 Phase 2 表达式（引用 Element 字段），编译期翻译为 ExprProgram。
///
/// # 工作原理
///
/// 每次迭代：
/// 1. inner.parse → elem（失败直接上抛，RU-1）
/// 2. `ctx.set_field_at(element_field_idx, elem_name, elem, py)` 把当前元素借用为
///    Element 字段槽位
/// 3. 求值终止表达式（GetInt(element_field_idx) 从 expr_values_buf 取到 elem）
/// 4. 表达式非零 → 终止（最后元素包含在内，RU-2）；零 → 继续迭代
///
/// 终止表达式求值全程在 Rust 内部栈式 VM 执行，零 FFI。
///
/// # parse 行为
///
/// 对齐 Python `RepeatUntil._parse`（core.py L2670-2682），但终止条件用 Phase 2
/// 表达式（零 FFI）。详见模块级文档。
///
/// # build 行为
///
/// 对齐 Python `RepeatUntil._build`（core.py L2684-2701）。详见模块级文档。
///
/// # sizeof 行为
///
/// 永远返回 `Err`（对齐 Python L2703-2704）。
#[derive(Debug)]
pub struct RepeatUntilNode {
    /// 元素子树（递归 Box）。
    inner: Box<crate::nodes::Node>,
    /// 终止表达式（Phase 2 ExprProgram，编译期从用户面表达式编译）。
    /// 求值结果非零即终止。
    terminator: ExprProgram,
    /// Element 字段在当前 Struct 的 `expr_values_buf` 中的索引（编译期确定）。
    /// RepeatUntil 每次迭代调 `ctx.set_field_at(element_field_idx, ...)` 把当前元素
    /// 写入该槽位，终止表达式中的 GetInt(element_field_idx) 即取到此值。
    element_field_idx: usize,
    /// Element 字段名（interned PyString，set_field_at 的 key 参数）。
    element_field_name: Py<PyString>,
    /// 是否丢弃解析结果（仍消耗流）。
    discard: bool,
}

impl RepeatUntilNode {
    /// 创建 RepeatUntilNode。
    ///
    /// # 参数
    ///
    /// - `inner`：元素子树（递归 Box 由调用方 `Box::new`）。
    /// - `terminator`：终止表达式（Phase 2 ExprProgram）。
    /// - `element_field_idx`：Element 字段在 Struct 中的索引。
    /// - `element_field_name`：Element 字段名（interned PyString）。
    /// - `discard`：是否丢弃解析结果。
    pub fn new(
        inner: crate::nodes::Node,
        terminator: ExprProgram,
        element_field_idx: usize,
        element_field_name: Py<PyString>,
        discard: bool,
    ) -> Self {
        Self {
            inner: Box::new(inner),
            terminator,
            element_field_idx,
            element_field_name,
            discard,
        }
    }

    /// 返回 inner 子树引用。
    pub fn inner(&self) -> &crate::nodes::Node {
        &self.inner
    }

    /// 返回终止表达式引用。
    pub fn terminator(&self) -> &ExprProgram {
        &self.terminator
    }

    /// 返回 Element 字段索引。
    pub fn element_field_idx(&self) -> usize {
        self.element_field_idx
    }

    /// 返回 Element 字段名（interned PyString）引用。
    pub fn element_field_name(&self) -> &Py<PyString> {
        &self.element_field_name
    }

    /// 是否丢弃解析结果。
    pub fn discard(&self) -> bool {
        self.discard
    }

    /// has_expressions 判断（设计 §6.1.1）。
    ///
    /// **始终返回 true**（v5）：终止表达式始终引用 Element 字段，需要外层 Struct
    /// 通过 `init_expr_values` 初始化 `expr_values_buf`，使 `set_field_at` 可用。
    ///
    /// 此外，inner 子树可能含表达式（虽 Phase 4 暂不支持 inner 表达式，但
    /// 防御性返回 true 避免未来扩展时遗漏 init）。
    pub fn has_expressions(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Construct impl
// ---------------------------------------------------------------------------

impl super::Construct for RepeatUntilNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 4.7 PyList Vec 中转：count 未知（终止条件运行时求值），用 Vec::new() 起步。
        // Rust Vec 增长策略（doubling）比 CPython list（~1.125x）高效。
        let mut elems: Vec<Py<PyAny>> = Vec::new();

        // 保存外层 _index（嵌套数组支持，设计 §3.2.2）。
        let old_index = ctx.index();

        let mut i: usize = 0;
        loop {
            ctx.set_index(i);
            // 4.7 lazy path：成功路径不调 path.push_index/pop。

            let elem = match self.inner.parse(py, stream, ctx, path) {
                Ok(v) => v,
                Err(mut e) => {
                    // RU-1: 失败直接上抛（不像 GreedyRange 回退）。
                    e.push_path_index(i);
                    ctx.restore_index(old_index);
                    return Err(e);
                }
            };

            if !self.discard {
                elems.push(elem.clone_ref(py));
            }

            // 把当前元素借用为 Element 字段槽位（仅写 expr_values_buf，不写 PyDict）。
            // 终止表达式中的 GetInt(element_field_idx) 会从此槽位取值。
            // 不写 PyDict 保证 StructNode.parse 完成后 Element 字段在实例 __dict__
            // 中仍是 None（语义正确：Element 字段不持有真实数据）。
            ctx.set_expr_value_only(self.element_field_idx, elem.bind(py));

            // 求值终止表达式（零 FFI，Rust 内部栈式 VM）。
            // 表达式非零即终止（最后元素已包含在 Vec 中，RU-2）。
            let stop_v = eval_expr_int(&self.terminator, ctx, py)?;
            if stop_v != 0 {
                // RU-2: 终止表达式为真时终止。
                break;
            }

            i = i.saturating_add(1);
        }

        // 一次性创建 PyList（pyo3 内部用 PyList_New + PyList_SET_ITEM）。
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
        // 注：与 v4 不同，v5 必须物化到 Vec——终止表达式求值需要 owned elem
        // 引用写入 expr_values_buf。可优化为惰性迭代（性能不达标再优化）。
        let items = super::common::collect_obj_to_vec(obj, "RepeatUntil", path)?;

        // 保存外层 _index（嵌套数组支持）。
        let old_index = ctx.index();

        let mut matched = false;
        for (i, elem) in items.iter().enumerate() {
            ctx.set_index(i);
            // 4.7 lazy path：成功路径不调 path.push_index/pop。
            let elem_bound = elem.bind(py);
            if let Err(mut e) = self.inner.build(py, elem_bound, stream, ctx, path) {
                e.push_path_index(i);
                ctx.restore_index(old_index);
                return Err(e);
            }

            // 把当前元素借用为 Element 字段槽位（与 parse 同模式，仅写 expr_values_buf）。
            ctx.set_expr_value_only(self.element_field_idx, elem_bound);

            // 求值终止表达式。
            let stop_v = eval_expr_int(&self.terminator, ctx, py)?;
            if stop_v != 0 {
                matched = true;
                break;
            }
        }

        ctx.restore_index(old_index);

        if !matched {
            // RU-3: 无元素满足终止表达式。
            return Err(ConstructError::Repeat {
                message: "expected any item to match terminator, when building".to_string(),
                path: path.to_string(),
            });
        }
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // 对齐 Python `RepeatUntil._sizeof` L2703-2704：永远 SizeofError。
        Err(ConstructError::Generic {
            message: "RepeatUntil size is undefined".to_string(),
            path: String::new(),
        })
    }
}
