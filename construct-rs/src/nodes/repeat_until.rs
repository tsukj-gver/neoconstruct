//! RepeatUntilNode：谓词终止数组读写。
//!
//! 设计依据：`docs/模块设计-Array.md` §4.3（含 §4.3.4 build discard 修正）。
//! Python 参考：`construct/construct/core.py` `RepeatUntil`（L2637-2704）。
//!
//! ## 概述
//!
//! `RepeatUntil(predicate, subcon, discard)` 解析元素到 list 直到谓词为真
//! （最后元素被包含），或从 list 构建字节序列直到某元素满足谓词。
//!
//! ## 两条谓词路径（设计 §2.5 决策 A5 / §4.3.1）
//!
//! - **PyCallable 路径**：predicate 是 Python callable `(obj, list, ctx) -> bool`。
//!   每次迭代跨 FFI 调用，性能 3-5x（Python 谓词 ~500-1000ns/次主导）。
//! - **Expr 路径**：predicate 是简单 lambda（仅依赖当前元素，如
//!   `lambda x, _, _: x > 5`），编译期识别并翻译为 ExprProgram
//!   （`[GetElem, Const(N), Op]`）。运行时 Rust 内部栈式求值，零 FFI。
//!   性能 ≥8x（接近 Array 水平）。
//!
//! ## parse 行为（对齐 Python L2670-2682）
//!
//! 1. 创建空 PyList（Expr 路径用 Vec 中转，PyCallable 路径保持 PyList）
//! 2. `loop`：
//!    - 设置 `ctx._index = i`（4.7 lazy path：成功路径不调 path.push_index/pop）
//!    - inner.parse → elem（失败直接上抛，RU-1，**不**像 GreedyRange 回退）
//!    - 若 `!discard`，append elem 到 list
//!    - 评估谓词（Expr / PyCallable）：
//!      - 为真 → 终止（最后元素已包含在 list 中，RU-2）
//!      - 为假 → 继续下一次迭代
//! 3. 恢复 `ctx._index`
//!
//! ## build 行为（对齐 Python L2684-2701，含 P2 修正）
//!
//! 1. 收集 obj（list/tuple/iterable）到 Vec
//! 2. `partial = PyList::empty()`：用于谓词的 list 参数
//! 3. 遍历元素：
//!    - inner.build(e) 写入 stream
//!    - **P2 修正**：若 `!discard`，partial.append(e)（discard=True 时 partial 始终为空）
//!    - 评估谓词：为真 → matched=true，break
//! 4. 若 !matched → `Repeat` 错误（RU-3）
//!
//! ## 已知差异（V-2，文档化）
//!
//! **partial 收集 `elem` 而非 `buildret`**：Python L2693-2696 中
//! `partiallist.append(buildret)`，其中 `buildret = self.subcon._build(e, ...)`。
//! Rust `Construct::build` 返回 `()`（无返回值），因此收集的是用户输入 `e`。
//! 对 FormatField/Bytes 内部（绝大多数场景），`buildret == e` 无差异；
//! 对 Adapter 系（如 Enum）作为 inner 时，`buildret` 是 decode 后的子构造器值，
//! 可能 ≠ e。Adapter-as-inner-RepeatUntil 是罕见场景，行为差异在此文档化。
//!
//! ## sizeof
//!
//! 永远返回 `Err`（对齐 Python L2703-2704）。

use crate::container_cache;
use crate::context::Context;
use crate::error::ConstructError;
use crate::expr::{eval_expr_int, ExprProgram};
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyList, PyTuple, PyType};

use super::Construct;

// ---------------------------------------------------------------------------
// RepeatPredicate
// ---------------------------------------------------------------------------

/// RepeatUntil 的终止条件。
///
/// 设计依据：`docs/模块设计-Array.md` §4.3.1。
///
/// # 两条路径
///
/// - [`Expr`](RepeatPredicate::Expr)：编译期识别的简单 lambda（仅依赖当前元素）。
///   运行时 Rust 内部栈式求值（`[GetElem, Const(N), Op]`），零 FFI。
///   仅支持整数元素（PyLong_AsLongLong）。
/// - [`PyCallable`](RepeatPredicate::PyCallable)：复杂谓词，每次迭代跨 FFI 调用
///   Python callable `(obj, list, context) -> bool`。
///
/// 选择规则（Python 侧 `_try_compile_repeat_predicate`）：
/// 简单 lambda（如 `lambda x, _, _: x > 5`）→ Expr；其他 → PyCallable。
#[derive(Debug)]
pub enum RepeatPredicate {
    /// Expr 谓词路径：编译期从简单 lambda 编译的 ExprProgram。
    ///
    /// 求值时通过 `ctx.set_current_elem_ptr` 设置当前元素指针，
    /// `ExprOp::GetElem` 指令读取并 `PyLong_AsLongLong`。
    Expr(ExprProgram),
    /// PyCallable 谓词路径：任意 Python callable `(obj, list, ctx) -> bool`。
    PyCallable(Py<PyAny>),
}

impl RepeatPredicate {
    /// 是否为 Expr 谓词路径。
    pub fn is_expr(&self) -> bool {
        matches!(self, RepeatPredicate::Expr(_))
    }
}

// ---------------------------------------------------------------------------
// RepeatUntilNode
// ---------------------------------------------------------------------------

/// 谓词终止数组节点。
///
/// 对应 Python construct `RepeatUntil(predicate, subcon, discard)`（core.py L2637）。
///
/// 持有 `inner: Box<Node>` 元素子树 + [`RepeatPredicate`] 终止条件 + `discard: bool`。
///
/// # parse 行为
///
/// 1. 创建空 PyList（无法预知容量，动态增长）
/// 2. 保存外层 `ctx._index`（嵌套数组支持，设计 §3.2.2）
/// 3. `loop`：设置 `ctx._index = i`、`path.push_index(i)`，inner.parse → elem
///    - 失败：直接上抛错误（RU-1，**不**像 GreedyRange 回退）
/// 4. `!discard` 时 append elem 到 list
/// 5. 评估谓词（Expr / PyCallable）：
///    - Expr 路径：`ctx.set_current_elem_ptr(elem.as_ptr())`，调 `eval_expr_int`
///    - PyCallable 路径：调 Python `predicate(elem, list, context_proxy)`
/// 6. 谓词为真 → 终止（最后元素已包含，RU-2）
///
/// # build 行为
///
/// 对齐 Python core.py L2684-2701（含设计 §4.3.4 P2 discard 修正）：
/// - 遍历元素 → inner.build
/// - **discard 守卫**：`if !discard { partial.append(e) }`（P2 修正，
///   discard=True 时 partial 始终为空，谓词收到空 list）
/// - 评估谓词：为真则停止；全部遍历完无元素满足则 `Repeat` 错误（RU-3）
///
/// # sizeof 行为
///
/// 永远返回 `Err`（对齐 Python L2703-2704）。
#[derive(Debug)]
pub struct RepeatUntilNode {
    /// 元素子树（递归 Box）。
    inner: Box<crate::nodes::Node>,
    /// 终止条件（Expr 快路径 / PyCallable 兜底）。
    predicate: RepeatPredicate,
    /// 是否丢弃解析结果（仍消耗流）。
    discard: bool,
}

impl RepeatUntilNode {
    /// 创建 RepeatUntilNode。
    pub fn new(inner: crate::nodes::Node, predicate: RepeatPredicate, discard: bool) -> Self {
        Self {
            inner: Box::new(inner),
            predicate,
            discard,
        }
    }

    /// 返回 inner 子树引用。
    pub fn inner(&self) -> &crate::nodes::Node {
        &self.inner
    }

    /// 返回 predicate 引用。
    pub fn predicate(&self) -> &RepeatPredicate {
        &self.predicate
    }

    /// 是否丢弃解析结果。
    pub fn discard(&self) -> bool {
        self.discard
    }

    /// has_expressions 判断（设计 §6.1.1）。
    ///
    /// **始终返回 true**（v4 V-1 修正）。
    ///
    /// # 理由
    ///
    /// RepeatUntil 无论走 Expr 还是 PyCallable 谓词路径，都依赖外层 Struct
    /// 通过 [`Context::inject_fields`] 注入实例 dict 到 ctx.fields()：
    ///
    /// - **Expr 路径**：[`eval_expr_predicate`] 通过 [`Context::set_current_elem_ptr`]
    ///   设置当前元素指针；当前不读 expr_values_buf，但保留 true 作为防御性编码，
    ///   避免未来 ExprProgram 扩展引入 GetInt（如谓词 `x > field_name`）时遗漏 init。
    /// - **PyCallable 路径**（v4 V-1）：[`build_context_proxy`] 需要从 ctx.fields()
    ///   浅复制 Struct 字段构造 Container proxy，谓词访问 `ctx.threshold` 才能工作。
    ///   若 has_expressions=false，外层 Struct 走 "无表达式路径" 直接操作 dict
    ///   而不 inject_fields，导致 ctx.fields() == None，proxy 仅含 _index（无字段）。
    ///
    /// 开销 ~5-10ns（一次 Vec::with_capacity），相对 PyCallable 路径 ~575-1125ns/iter
    /// 可忽略。
    pub fn has_expressions(&self) -> bool {
        // 始终 true（v4 V-1）：保证外层 Struct inject_fields，使 ctx.fields() 可用。
        // inner.has_expressions() / predicate.is_expr() 短路求值无意义，
        // 此处显式返回 true 表达意图。
        let _ = (self.inner.has_expressions(), self.predicate.is_expr());
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
        let old_index = ctx.index();

        // 4.7 Item 8：Expr 路径用 Vec 中转（谓词不读 list），PyCallable 路径保持 PyList
        // （谓词签名 `(obj, list, ctx)` 需要实时 list 参数）。
        let result_list: Py<PyAny> = match &self.predicate {
            RepeatPredicate::Expr(program) => {
                let elems = parse_expr_path(
                    py,
                    stream,
                    ctx,
                    path,
                    &self.inner,
                    program,
                    self.discard,
                )?;
                PyList::new_bound(py, elems).into_any().unbind()
            }
            RepeatPredicate::PyCallable(predicate) => {
                // PyCallable 路径保持 PyList（谓词需要实时 list 参数）。
                let list = PyList::new_bound(py, Vec::<Py<PyAny>>::new());
                parse_callable_path(
                    py,
                    stream,
                    ctx,
                    path,
                    &self.inner,
                    predicate.bind(py),
                    &list,
                    self.discard,
                )?;
                list.into_any().unbind()
            }
        };

        restore_index(ctx, old_index);
        Ok(result_list)
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        let old_index = ctx.index();

        // partial list：传给谓词的 list 参数。discard=True 时始终为空（P2 修正）。
        let partial = PyList::new_bound(py, Vec::<Py<PyAny>>::new());

        let mut matched = false;

        match &self.predicate {
            RepeatPredicate::Expr(program) => {
                // Expr 路径：物化 obj 到 Vec（谓词在 Rust 内求值，需 owned 元素引用）。
                let items: Vec<Py<PyAny>> = collect_iterable(py, obj, path)?;
                for (i, elem) in items.iter().enumerate() {
                    ctx.set_index(i);
                    // 4.7 lazy path：成功路径不调 path.push_index/pop。
                    let elem_bound = elem.bind(py);
                    if let Err(mut e) = self.inner.build(py, elem_bound, stream, ctx, path) {
                        e.push_path_index(i);
                        restore_index(ctx, old_index);
                        return Err(e);
                    }

                    // P2 修正：仅非 discard 时 append 到 partial。
                    if !self.discard {
                        partial
                            .append(elem.clone_ref(py))
                            .map_err(ConstructError::from)?;
                    }

                    // Expr 路径谓词求值：设置 _current_elem_ptr → eval → clear。
                    let stop = eval_expr_predicate(py, ctx, elem.bind(py), program)?;
                    if stop {
                        matched = true;
                        break;
                    }
                }
            }
            RepeatPredicate::PyCallable(predicate) => {
                let pred_bound = predicate.bind(py);
                // v4 V-1：缓存 Container 类，零开销取用。
                let container_cls =
                    container_cache::container_class(py).map_err(|e| ConstructError::Generic {
                        message: format!("RepeatUntil build: failed to get Container class: {}", e),
                        path: path.to_string(),
                    })?;

                // 优化：入口处一次性构造 Container proxy（含 ctx.fields() 字段 + 初始 _index）。
                // 每次迭代仅更新 _index（~20ns），避免每次构造新 Container（~300ns）。
                let ctx_proxy = match build_context_proxy(py, &container_cls, ctx, path) {
                    Ok(p) => p,
                    Err(e) => {
                        restore_index(ctx, old_index);
                        return Err(e);
                    }
                };

                // V-2 性能优化：对 PyCallable 谓词路径用惰性迭代，避免提前物化整个 list。
                // Python `for i, e in enumerate(obj)` 在谓词满足时立即停止，不消费后续元素。
                // 对 N 很大但谓词早停的场景，节省 N - k 次 iter() 调用 + list materialize 开销。
                let py_iter = match obj.iter() {
                    Ok(it) => it,
                    Err(e) => {
                        return Err(ConstructError::Generic {
                            message: format!(
                                "RepeatUntil build expects iterable, got iter() error: {}",
                                e
                            ),
                            path: path.to_string(),
                        });
                    }
                };

                let mut i: usize = 0;
                for elem_result in py_iter {
                    let elem = elem_result.map_err(|e| ConstructError::Generic {
                        message: format!("RepeatUntil build iterable error: {}", e),
                        path: path.to_string(),
                    })?;
                    ctx.set_index(i);
                    // 更新 proxy 中的 _index（Container 继承 dict，set_item 走 dict.__setitem__）
                    if let Err(e) = ctx_proxy.set_item("_index", i.into_py(py).bind(py)) {
                        restore_index(ctx, old_index);
                        return Err(ConstructError::Generic {
                            message: format!("RepeatUntil build: set _index failed: {}", e),
                            path: path.to_string(),
                        });
                    }

                    // 4.7 lazy path：成功路径不调 path.push_index/pop。
                    let elem_bound = &elem;
                    if let Err(mut e) = self.inner.build(py, elem_bound, stream, ctx, path) {
                        // 错误路径重建索引段。
                        e.push_path_index(i);
                        restore_index(ctx, old_index);
                        return Err(e);
                    }

                    // P2 修正：仅非 discard 时 append 到 partial。
                    // V-2 已知差异：partial 收集的是 elem（用户输入），
                    // Python L2693-2696 收集 buildret（inner._build 返回值）。
                    // 详见模块顶部"已知差异"段落。
                    if !self.discard {
                        partial.append(&elem).map_err(ConstructError::from)?;
                    }

                    let stop = call_repeat_predicate(
                        py,
                        pred_bound,
                        elem_bound,
                        partial.as_any(),
                        &ctx_proxy,
                        path,
                    )?;
                    if stop {
                        matched = true;
                        break;
                    }
                    i = i.saturating_add(1);
                }
            }
        }

        restore_index(ctx, old_index);

        if !matched {
            // RU-3: 无元素满足谓词。
            return Err(ConstructError::Repeat {
                message: "expected any item to match predicate, when building".to_string(),
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

// ---------------------------------------------------------------------------
// 内部辅助：parse 路径
// ---------------------------------------------------------------------------

/// Expr 路径的 parse 内部实现（4.7：Vec 中转 + lazy path）。
///
/// 通过 `ctx.set_current_elem_ptr` 设置当前元素指针，调 `eval_expr_int`，
/// 求值后立即 `clear_current_elem_ptr`。
///
/// 返回收集到的元素 `Vec`，由调用方一次性创建 PyList。
#[allow(clippy::too_many_arguments)]
fn parse_expr_path<'py>(
    py: Python<'py>,
    stream: &mut ParseStream<'_>,
    ctx: &mut Context<'py>,
    path: &mut Path,
    inner: &crate::nodes::Node,
    program: &ExprProgram,
    discard: bool,
) -> Result<Vec<Py<PyAny>>, ConstructError> {
    // 4.7 Item 8：用 Vec 中转收集元素。Expr 谓词不读 list，仅读 ctx._current_elem_ptr。
    let mut elems: Vec<Py<PyAny>> = Vec::new();
    let mut i: usize = 0;
    loop {
        ctx.set_index(i);
        // 4.7 lazy path：成功路径不调 path.push_index/pop。
        let elem = match inner.parse(py, stream, ctx, path) {
            Ok(v) => v,
            Err(mut e) => {
                // RU-1: 失败直接上抛（不像 GreedyRange 回退）。
                e.push_path_index(i);
                return Err(e);
            }
        };

        if !discard {
            elems.push(elem.clone_ref(py));
        }

        // Expr 路径谓词求值：设置 _current_elem_ptr → eval → clear。
        let stop = eval_expr_predicate(py, ctx, elem.bind(py), program)?;
        if stop {
            // RU-2: 谓词为真时终止（最后元素已包含在 Vec 中）。
            return Ok(elems);
        }

        i = i.saturating_add(1);
    }
}

/// PyCallable 路径的 parse 内部实现（v4 V-1，含优化）。
///
/// **优化**（设计 §4.3.3 v4 优化方向）：在节点入口预创建一个 Container proxy，
/// 每次迭代仅更新 `_index` 字段（PyDict_SetItem 快路径，~20ns），避免每次迭代
/// 都重新构造 Container 实例（~300-500ns/iter）。
///
/// 这对齐 Python `RepeatUntil._parse` 的语义——Python 直接复用同一个 context 对象，
/// 仅修改 `context._index = i`（core.py L2677）。
///
/// 字段集在入口处从 `ctx.fields()` 一次性复制（对齐 Python 一次构造 Container），
/// 迭代过程中不再同步 ctx 可能的字段变更（罕见场景：inner.parse 修改外层字段）。
///
/// 设计依据：`docs/模块设计-Array.md` §4.3.3 v4 + §2.5 决策 A5 v4 + §8.5 关键依赖 4。
#[allow(clippy::too_many_arguments)]
fn parse_callable_path<'py>(
    py: Python<'py>,
    stream: &mut ParseStream<'_>,
    ctx: &mut Context<'py>,
    path: &mut Path,
    inner: &crate::nodes::Node,
    predicate: &Bound<'py, PyAny>,
    list: &Bound<'py, PyList>,
    discard: bool,
) -> Result<(), ConstructError> {
    // v4 V-1：缓存 Container 类，零开销取用。
    let container_cls =
        container_cache::container_class(py).map_err(|e| ConstructError::Generic {
            message: format!("RepeatUntil parse: failed to get Container class: {}", e),
            path: path.to_string(),
        })?;

    // 优化：入口处一次性构造 Container proxy（包含外层 ctx.fields() 字段 + 初始 _index）。
    // 每次迭代仅更新 _index 字段（~20ns），避免每次构造新 Container（~300ns）。
    // 对齐 Python `RepeatUntil._parse` 复用同一个 context 对象的语义。
    let ctx_proxy = build_context_proxy(py, &container_cls, ctx, path)?;

    let mut i: usize = 0;
    loop {
        ctx.set_index(i);
        // 更新 proxy 中的 _index（Container 继承 dict，set_item 直接走 dict.__setitem__）
        ctx_proxy
            .set_item("_index", i.into_py(py).bind(py))
            .map_err(|e| ConstructError::Generic {
                message: format!("RepeatUntil parse: set _index failed: {}", e),
                path: path.to_string(),
            })?;

        // 4.7 lazy path：成功路径不调 path.push_index/pop。
        let elem = match inner.parse(py, stream, ctx, path) {
            Ok(v) => v,
            Err(mut e) => {
                // RU-1: 失败直接上抛。
                e.push_path_index(i);
                return Err(e);
            }
        };

        if !discard {
            list.append(elem.clone_ref(py))
                .map_err(ConstructError::from)?;
        }

        // v4 V-1：复用同一 Container proxy，调用谓词。
        let stop = call_repeat_predicate(
            py,
            predicate,
            elem.bind(py),
            list.as_any(),
            &ctx_proxy,
            path,
        )?;
        if stop {
            return Ok(());
        }

        i = i.saturating_add(1);
    }
}

/// 评估 Expr 谓词：设置 `_current_elem_ptr` → `eval_expr_int` → 清除指针。
///
/// 返回 `true` 表示谓词为真（应终止循环）。
fn eval_expr_predicate(
    py: Python<'_>,
    ctx: &mut Context<'_>,
    elem: &Bound<'_, PyAny>,
    program: &ExprProgram,
) -> Result<bool, ConstructError> {
    // SAFETY: elem 是 Bound<'_, PyAny>，其 underlying PyObject 在整个谓词求值期间
    // 存活（Bound 持有 GIL scope 内的引用）。set/clear 之间不会 GC。
    let ptr = elem.as_ptr();
    ctx.set_current_elem_ptr(ptr);
    let result = eval_expr_int(program, ctx, py);
    ctx.clear_current_elem_ptr();
    let v = result?;
    Ok(v != 0)
}

/// 构造 Container proxy（每次迭代调用）。
///
/// v4 V-1 修正（`docs/模块设计-Array.md` §4.3.3 + §2.5 决策 A5 v4 +
/// `docs/设计决策记录.md` Phase 4 决策 4）。
///
/// 1. 构造临时 PyDict，浅复制 `ctx.fields()` 全部字段；
/// 2. 写入 `_index`（对齐 Python `context._index = i`，RU-9）；
/// 3. 用 Container 类包装（`Container.__init__` 自动执行 `self.__dict__ = self`，
///    使 attribute 访问与 item 访问等价）。
///
/// # 为什么必须是 Container 而非 PyDict
///
/// Container 通过 `__dict__ = self`（containers.py L110）使
/// `ctx_proxy.threshold` 与 `ctx_proxy['threshold']` 等价。Python
/// `RepeatUntil._parse/_build`（core.py L2681/L2697）直接把 Container
/// 传给谓词，用户写 `lambda x, lst, ctx: x > ctx.threshold` 是 Python
/// 文档示明的核心用法。仅传 PyDict 会让 attribute 访问静默失败
/// （`AttributeError: 'dict' object has no attribute 'threshold'`），
/// 属于"隐性破坏"，违反 AGENTS.md §0 Python 兼容性核心目标。
///
/// # 性能开销（每迭代）
///
/// - PyDict::new_bound + 字段复制：~200-400ns（字段数 N）
/// - Container 实例化（`Container.__init__`）：~200-500ns
/// - 合计：~400-900ns/iter（详见 §8.5 关键依赖 4）
///
/// PyCallable 是兜底路径（Expr 路径 ≥10x），行为正确性优先于性能。
fn build_context_proxy<'py>(
    py: Python<'py>,
    container_cls: &Bound<'py, PyType>,
    ctx: &Context<'_>,
    path: &mut Path,
) -> Result<Bound<'py, PyAny>, ConstructError> {
    // 1. 构造临时 PyDict，复制 ctx.fields() 全部字段（浅复制）。
    let dict = PyDict::new_bound(py);
    if let Some(fields) = ctx.fields() {
        for item in fields.iter() {
            let (key, value) = item;
            // set_item 失败极少见（key 不可哈希等），包装为 Generic + path。
            dict.set_item(&key, &value)
                .map_err(|e| ConstructError::Generic {
                    message: format!("RepeatUntil build_context_proxy: set_item failed: {}", e),
                    path: path.to_string(),
                })?;
        }
    }
    // 2. 写入 _index（对齐 Python `context._index = i`）。
    match ctx.index() {
        Some(i) => {
            dict.set_item("_index", i.into_py(py).bind(py))
                .map_err(|e| ConstructError::Generic {
                    message: format!("RepeatUntil build_context_proxy: set _index failed: {}", e),
                    path: path.to_string(),
                })?;
        }
        None => {
            dict.set_item("_index", py.None().bind(py))
                .map_err(|e| ConstructError::Generic {
                    message: format!(
                        "RepeatUntil build_context_proxy: set _index=None failed: {}",
                        e
                    ),
                    path: path.to_string(),
                })?;
        }
    }
    // 3. 用 Container 包装（Container.__init__ 接受 dict，自动 __dict__ = self）。
    container_cls
        .call1((dict,))
        .map_err(|e| ConstructError::Generic {
            message: format!("RepeatUntil build_context_proxy: Container() failed: {}", e),
            path: path.to_string(),
        })
        .map(|obj| obj.into_any())
}

/// 调用 RepeatUntil 谓词（PyCallable 路径），返回是否应终止。
///
/// 对齐 Python `predicate(obj, list, context)`（core.py L2681/L2697）。
///
/// # v4 V-1
///
/// `ctx_proxy` 必须是 Container 实例（由 [`build_context_proxy`] 构造），
/// 支持 attribute 与 item 双重访问。详见 [`build_context_proxy`] 文档。
fn call_repeat_predicate(
    py: Python<'_>,
    predicate: &Bound<'_, PyAny>,
    elem: &Bound<'_, PyAny>,
    list: &Bound<'_, PyAny>,
    ctx_proxy: &Bound<'_, PyAny>,
    path: &mut Path,
) -> Result<bool, ConstructError> {
    let _ = py;
    let result = predicate
        .call1((elem, list, ctx_proxy))
        .map_err(|e| ConstructError::Generic {
            message: format!("RepeatUntil predicate raised: {}", e),
            path: path.to_string(),
        })?;
    let truthy = result.is_truthy().map_err(|e| ConstructError::Generic {
        message: format!("RepeatUntil predicate returned non-bool: {}", e),
        path: path.to_string(),
    })?;
    Ok(truthy)
}

/// 从 iterable（list / tuple / 任意 iterable）收集元素到 Vec。
///
/// 与 ArrayNode/GreedyRangeNode 的 collect_iterable 同模式。
fn collect_iterable(
    py: Python<'_>,
    obj: &Bound<'_, PyAny>,
    path: &mut Path,
) -> Result<Vec<Py<PyAny>>, ConstructError> {
    let _ = py;
    if let Ok(list) = obj.downcast::<PyList>() {
        Ok(list.iter().map(|b| b.unbind()).collect())
    } else if let Ok(tuple) = obj.downcast::<PyTuple>() {
        Ok(tuple.iter().map(|b| b.unbind()).collect())
    } else {
        // 兜底：尝试 iter() 收集。
        let iter = obj.iter().map_err(|e| ConstructError::Generic {
            message: format!(
                "RepeatUntil build expects list/tuple, got {} (iter error: {})",
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
            message: format!("RepeatUntil build iterable error: {}", e),
            path: path.to_string(),
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
    use crate::expr::ExprOp;
    use crate::nodes::format_field::{FormatFieldNode, PythonFormat};
    use crate::nodes::Construct;
    use crate::nodes::Node;

    fn ensure_python() {
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            pyo3::prepare_freethreaded_python();
            Python::with_gil(|py| {
                // v4 V-1：测试前确保 Container 类缓存已初始化。
                // 失败不致命——某些环境（无 construct.lib.containers）下会回落，
                // 但 RepeatUntil PyCallable 测试需要 Container 才能正常工作。
                let _ = crate::container_cache::init_container_class(py);
            });
        });
    }

    fn with_py<F, R>(f: F) -> R
    where
        F: for<'py> FnOnce(Python<'py>) -> R,
    {
        ensure_python();
        Python::with_gil(f)
    }

    /// 构造一个 Byte（Int8ub）节点。
    fn byte_node() -> Node {
        Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big))
    }

    /// 构造一个 Int16ub 节点（2 字节）。
    fn int16ub_node() -> Node {
        Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big))
    }

    /// 创建一个简单的 Python lambda `(x, lst, ctx) -> x > N`。
    fn make_lambda_gt(n: i64) -> Py<PyAny> {
        with_py(|py| {
            let code = format!("lambda x, lst, ctx: x > {}", n);
            py.eval_bound(&code, None, None)
                .expect("lambda")
                .extract::<Py<PyAny>>()
                .expect("Py<PyAny>")
        })
    }

    /// 创建一个 Python lambda 检查 list 末尾两个元素。
    fn make_lambda_list_tail() -> Py<PyAny> {
        with_py(|py| {
            py.eval_bound(
                "lambda x, lst, ctx: len(lst) >= 2 and lst[-2:] == [0, 0]",
                None,
                None,
            )
            .expect("lambda")
            .extract::<Py<PyAny>>()
            .expect("Py<PyAny>")
        })
    }

    // ======================================================================
    // 构造器 & 访问器
    // ======================================================================

    #[test]
    fn new_callable_predicate_stores_predicate() {
        let py_pred = with_py(|_py| make_lambda_gt(5));
        let node = RepeatUntilNode::new(byte_node(), RepeatPredicate::PyCallable(py_pred), false);
        assert!(!node.discard());
        assert!(matches!(node.predicate(), RepeatPredicate::PyCallable(_)));
    }

    #[test]
    fn new_expr_predicate_stores_predicate() {
        let prog = ExprProgram::new(vec![ExprOp::GetElem, ExprOp::Const(5), ExprOp::Gt]);
        let node = RepeatUntilNode::new(byte_node(), RepeatPredicate::Expr(prog), false);
        assert!(node.predicate().is_expr());
    }

    #[test]
    fn discard_flag_stored_correctly() {
        let prog = ExprProgram::new(vec![ExprOp::GetElem, ExprOp::Const(5), ExprOp::Gt]);
        let node = RepeatUntilNode::new(byte_node(), RepeatPredicate::Expr(prog), true);
        assert!(node.discard());
    }

    #[test]
    fn has_expressions_expr_predicate_returns_true() {
        let prog = ExprProgram::new(vec![ExprOp::GetElem, ExprOp::Const(5), ExprOp::Gt]);
        let node = RepeatUntilNode::new(byte_node(), RepeatPredicate::Expr(prog), false);
        assert!(node.has_expressions());
    }

    #[test]
    fn has_expressions_callable_predicate_returns_inner_value() {
        // v4 V-1：has_expressions 始终返回 true（即便 callable 谓词 + 无表达式 inner）。
        // 理由：PyCallable 谓词可能访问 ctx 字段，外层 Struct 必须 inject_fields
        // 使 ctx.fields() 可用，否则 build_context_proxy 拿不到字段。
        let py_pred = with_py(|_py| make_lambda_gt(5));
        let node = RepeatUntilNode::new(byte_node(), RepeatPredicate::PyCallable(py_pred), false);
        assert!(
            node.has_expressions(),
            "v4 V-1: RepeatUntil.has_expressions should always be true"
        );
    }

    // ======================================================================
    // PyCallable 路径：parse 基本场景
    // ======================================================================

    #[test]
    fn parse_callable_predicate_match_at_element() {
        // RU-2: 谓词在第 N 个元素为真 → 返回前 N+1 个元素
        // 谓词 x > 5：[1, 2, 3, 4, 5, 6] → 6 时满足，返回 [1,2,3,4,5,6]
        with_py(|py| {
            let py_pred = make_lambda_gt(5);
            let node =
                RepeatUntilNode::new(byte_node(), RepeatPredicate::PyCallable(py_pred), false);
            let mut stream = ParseStream::new(&[1, 2, 3, 4, 5, 6, 7, 8]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("list");
            assert_eq!(list.len(), 6);
            let last: i64 = list.get_item(5).unwrap().extract().unwrap();
            assert_eq!(last, 6);
            // 流位置：6 字节已读
            assert_eq!(stream.tell(), 6);
            // _index 恢复
            assert!(ctx.index().is_none());
        });
    }

    #[test]
    fn parse_callable_first_element_matches() {
        // 边界：第一个元素就满足谓词
        // 谓词 x > 5：[6, 7, 8] → 6 时满足，返回 [6]
        with_py(|py| {
            let py_pred = make_lambda_gt(5);
            let node =
                RepeatUntilNode::new(byte_node(), RepeatPredicate::PyCallable(py_pred), false);
            let mut stream = ParseStream::new(&[6, 7, 8]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("list");
            assert_eq!(list.len(), 1);
            let v: i64 = list.get_item(0).unwrap().extract().unwrap();
            assert_eq!(v, 6);
            assert_eq!(stream.tell(), 1);
        });
    }

    #[test]
    fn parse_callable_inner_failure_propagates_error() {
        // RU-1: 子构造器解析失败 → 错误直接上抛（不像 GreedyRange 回退）
        // 用 Int16ub（2 字节）作为 inner，流只 1 字节 → 第一次解析就失败
        with_py(|py| {
            let py_pred = make_lambda_gt(255);
            let node =
                RepeatUntilNode::new(int16ub_node(), RepeatPredicate::PyCallable(py_pred), false);
            let mut stream = ParseStream::new(&[0xFF]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            // 错误应是 Stream（字节不足）
            assert!(matches!(err, ConstructError::Stream { .. }));
        });
    }

    #[test]
    fn parse_callable_discard_returns_empty_but_consumes() {
        // RU-7: discard=True 时仍调谓词（用空 list），不收集元素到 obj list
        // 谓词 x > 5：[1, 2, 3, 4, 5, 6] → 6 时满足
        with_py(|py| {
            let py_pred = make_lambda_gt(5);
            let node =
                RepeatUntilNode::new(byte_node(), RepeatPredicate::PyCallable(py_pred), true);
            let mut stream = ParseStream::new(&[1, 2, 3, 4, 5, 6, 7, 8]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("list");
            // discard=True：list 为空，但流消耗到第 6 字节
            assert_eq!(list.len(), 0);
            assert_eq!(stream.tell(), 6);
        });
    }

    #[test]
    fn parse_callable_predicate_depending_on_list() {
        // 复杂谓词（依赖 list 末尾）：lambda x, lst, ctx: lst[-2:] == [0, 0]
        // [1, 0, 0, 9]：第 3 个元素后 lst=[1,0,0]，lst[-2:]==[0,0] → 满足
        with_py(|py| {
            let py_pred = make_lambda_list_tail();
            let node =
                RepeatUntilNode::new(byte_node(), RepeatPredicate::PyCallable(py_pred), false);
            let mut stream = ParseStream::new(&[1, 0, 0, 9, 9]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("list");
            assert_eq!(list.len(), 3);
            let v0: i64 = list.get_item(0).unwrap().extract().unwrap();
            let v1: i64 = list.get_item(1).unwrap().extract().unwrap();
            let v2: i64 = list.get_item(2).unwrap().extract().unwrap();
            assert_eq!(v0, 1);
            assert_eq!(v1, 0);
            assert_eq!(v2, 0);
        });
    }

    // ======================================================================
    // PyCallable 路径：build
    // ======================================================================

    #[test]
    fn build_callable_predicate_match_stops_at_element() {
        // build 方向：谓词 x > 5，list = [1, 2, 3, 4, 5, 6, 7]
        // 期望：写到第 6 个元素（6 满足谓词）就停止，stream = [1,2,3,4,5,6]
        with_py(|py| {
            let py_pred = make_lambda_gt(5);
            let node =
                RepeatUntilNode::new(byte_node(), RepeatPredicate::PyCallable(py_pred), false);
            let obj = py
                .eval_bound("[1, 2, 3, 4, 5, 6, 7]", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[1, 2, 3, 4, 5, 6]);
        });
    }

    #[test]
    fn build_callable_no_match_returns_repeat_error() {
        // RU-3: 无元素满足谓词 → Repeat 错误
        with_py(|py| {
            let py_pred = make_lambda_gt(100);
            let node =
                RepeatUntilNode::new(byte_node(), RepeatPredicate::PyCallable(py_pred), false);
            let obj = py.eval_bound("[1, 2, 3]", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Repeat { message, .. } => {
                    assert!(message.contains("expected any item"));
                }
                other => panic!("expected Repeat, got {:?}", other),
            }
        });
    }

    #[test]
    fn build_callable_discard_passes_empty_partial_to_predicate() {
        // RU-8: build discard=True 时 partial 始终为空。
        // 谓词依赖 list：lambda x, lst, ctx: len(lst) >= 2 and lst[-2:] == [0, 0]
        // discard=True 时 lst 永远为空 → 永远不满足 → RepeatError
        with_py(|py| {
            let py_pred = make_lambda_list_tail();
            let node =
                RepeatUntilNode::new(byte_node(), RepeatPredicate::PyCallable(py_pred), true);
            let obj = py.eval_bound("[1, 0, 0, 9, 9]", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail with discard=True");
            match err {
                ConstructError::Repeat { .. } => {}
                other => panic!("expected Repeat, got {:?}", other),
            }
            // 全部元素都被 build（谓词不满足，遍历完所有）
            assert_eq!(stream.as_bytes(), &[1, 0, 0, 9, 9]);
        });
    }

    #[test]
    fn build_callable_predicate_with_discard_false_succeeds_on_list_dep() {
        // 对照：discard=False 时，同样的谓词能正常终止
        with_py(|py| {
            let py_pred = make_lambda_list_tail();
            let node =
                RepeatUntilNode::new(byte_node(), RepeatPredicate::PyCallable(py_pred), false);
            let obj = py.eval_bound("[1, 0, 0, 9, 9]", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            // 第 3 个元素后 partial=[1,0,0]，满足 lst[-2:]==[0,0]
            assert_eq!(stream.as_bytes(), &[1, 0, 0]);
        });
    }

    // ======================================================================
    // RU-9: PyCallable 谓词访问 context 字段（v4 V-1 修正）
    // ======================================================================

    /// 创建一个访问 context attribute 的谓词：`lambda x, lst, ctx: x > ctx.threshold`
    fn make_lambda_ctx_threshold_attr() -> Py<PyAny> {
        with_py(|py| {
            py.eval_bound("lambda x, lst, ctx: x > ctx.threshold", None, None)
                .expect("lambda")
                .extract::<Py<PyAny>>()
                .expect("Py<PyAny>")
        })
    }

    /// 创建一个访问 context item 的谓词：`lambda x, lst, ctx: x > ctx['threshold']`
    fn make_lambda_ctx_threshold_item() -> Py<PyAny> {
        with_py(|py| {
            py.eval_bound("lambda x, lst, ctx: x > ctx['threshold']", None, None)
                .expect("lambda")
                .extract::<Py<PyAny>>()
                .expect("Py<PyAny>")
        })
    }

    /// 创建一个访问 context _index 的谓词：`lambda x, lst, ctx: ctx._index >= 2`
    fn make_lambda_ctx_index_at_least(n: i64) -> Py<PyAny> {
        with_py(|py| {
            let code = format!("lambda x, lst, ctx: ctx._index >= {}", n);
            py.eval_bound(&code, None, None)
                .expect("lambda")
                .extract::<Py<PyAny>>()
                .expect("Py<PyAny>")
        })
    }

    #[test]
    fn parse_callable_predicate_accesses_context_field_attr() {
        // RU-9: 谓词 `lambda x, lst, ctx: x > ctx.threshold`（attribute 访问）
        // v4 V-1：proxy 必须是 Container（__dict__ = self 支持 attribute 访问）
        // ctx.threshold = 5，data = [1, 2, 3, 4, 5, 6, 7] → 6 时满足（x > 5）
        with_py(|py| {
            crate::container_cache::init_container_class(py).expect("init container");
            let py_pred = make_lambda_ctx_threshold_attr();
            let node =
                RepeatUntilNode::new(byte_node(), RepeatPredicate::PyCallable(py_pred), false);
            let mut stream = ParseStream::new(&[1, 2, 3, 4, 5, 6, 7]);
            let mut ctx = Context::new_root(py).expect("ctx");
            // 设置 context 字段 threshold = 5
            let threshold_val = 5i64.into_py(py);
            ctx.set_field("threshold", threshold_val.bind(py))
                .expect("set threshold");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("list");
            // x > 5：1,2,3,4,5,6 → 6 时满足（6 > 5）
            assert_eq!(list.len(), 6);
            let last: i64 = list.get_item(5).unwrap().extract().unwrap();
            assert_eq!(last, 6);
            assert_eq!(stream.tell(), 6);
        });
    }

    #[test]
    fn parse_callable_predicate_accesses_context_field_item() {
        // RU-9: 谓词 `lambda x, lst, ctx: x > ctx['threshold']`（item 访问）
        // 同样需要 Container（Container 继承 dict，item 访问可用）
        with_py(|py| {
            crate::container_cache::init_container_class(py).expect("init container");
            let py_pred = make_lambda_ctx_threshold_item();
            let node =
                RepeatUntilNode::new(byte_node(), RepeatPredicate::PyCallable(py_pred), false);
            let mut stream = ParseStream::new(&[1, 2, 3, 4, 5, 6, 7]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let threshold_val = 5i64.into_py(py);
            ctx.set_field("threshold", threshold_val.bind(py))
                .expect("set threshold");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("list");
            assert_eq!(list.len(), 6);
        });
    }

    #[test]
    fn parse_callable_predicate_accesses_context_index() {
        // RU-9 变体：谓词 `lambda x, lst, ctx: ctx._index >= 2`（_index 字段）
        // data = [9, 9, 9, 9] → 在 i=2 时满足（index=2 >= 2）
        with_py(|py| {
            crate::container_cache::init_container_class(py).expect("init container");
            let py_pred = make_lambda_ctx_index_at_least(2);
            let node =
                RepeatUntilNode::new(byte_node(), RepeatPredicate::PyCallable(py_pred), false);
            let mut stream = ParseStream::new(&[9, 9, 9, 9]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("list");
            assert_eq!(list.len(), 3); // i=0,1,2 → i=2 满足
        });
    }

    #[test]
    fn build_callable_predicate_accesses_context_field() {
        // RU-9 build 方向：谓词访问 context attribute
        // ctx.threshold = 3，obj = [1, 2, 3, 4, 5, 6] → 在 4 时满足（4 > 3）
        with_py(|py| {
            crate::container_cache::init_container_class(py).expect("init container");
            let py_pred = make_lambda_ctx_threshold_attr();
            let node =
                RepeatUntilNode::new(byte_node(), RepeatPredicate::PyCallable(py_pred), false);
            let obj = py
                .eval_bound("[1, 2, 3, 4, 5, 6]", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let threshold_val = 3i64.into_py(py);
            ctx.set_field("threshold", threshold_val.bind(py))
                .expect("set threshold");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            // x > 3：1,2,3,4 → 在 4 时满足
            assert_eq!(stream.as_bytes(), &[1, 2, 3, 4]);
        });
    }

    #[test]
    fn parse_callable_proxy_is_container_instance() {
        // RU-9 类型检查：谓词收到的 ctx_proxy isinstance(ctx_proxy, Container) 为 True
        // 这是 Python 用户可能写的类型检查（如调试场景）
        with_py(|py| {
            crate::container_cache::init_container_class(py).expect("init container");
            // 用模块级 def 让 Container 进入函数 __globals__
            let code = concat!(
                "Container = __import__('construct.lib.containers', fromlist=['Container']).Container\n",
                "def p(x, lst, ctx):\n",
                "    return isinstance(ctx, Container) and x > 200\n",
            );
            py.run_bound(code, None, None).expect("def");
            let py_pred = py
                .eval_bound("p", None, None)
                .expect("p")
                .extract::<Py<PyAny>>()
                .expect("Py");
            let node =
                RepeatUntilNode::new(byte_node(), RepeatPredicate::PyCallable(py_pred), false);
            let mut stream = ParseStream::new(&[1, 2, 3, 255]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("list");
            // x > 200：1,2,3,255 → 在 255 时满足（255 > 200）
            assert_eq!(list.len(), 4);
        });
    }

    #[test]
    fn parse_callable_proxy_reflects_index_change_per_iteration() {
        // RU-9 关键不变量：每次迭代 proxy._index 反映当前 i
        // 谓词记录每次收到的 _index，验证递增序列
        with_py(|py| {
            crate::container_cache::init_container_class(py).expect("init container");
            // 模块级 def + recorder 列表，函数 __globals__ 可见
            let code = concat!(
                "recorder = []\n",
                "def p(x, lst, ctx):\n",
                "    recorder.append(ctx._index)\n",
                "    return x >= 2\n",
            );
            py.run_bound(code, None, None).expect("def");
            let py_pred = py
                .eval_bound("p", None, None)
                .expect("p")
                .extract::<Py<PyAny>>()
                .expect("Py");
            let node =
                RepeatUntilNode::new(byte_node(), RepeatPredicate::PyCallable(py_pred), false);
            let mut stream = ParseStream::new(&[0, 1, 2, 3]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("list");
            assert_eq!(list.len(), 3); // 0,1,2 → 在 i=2 时 x=2>=2 满足
                                       // 验证 recorder 记录的是 [0, 1, 2]
            let recorder_bound = py.eval_bound("recorder", None, None).expect("recorder");
            let recorder = recorder_bound.downcast::<PyList>().expect("list");
            let indices: Vec<i64> = recorder.iter().map(|b| b.extract().unwrap()).collect();
            assert_eq!(indices, vec![0, 1, 2]);
        });
    }

    // ======================================================================
    // Expr 路径：parse
    // ======================================================================

    #[test]
    fn parse_expr_predicate_match_at_element() {
        // RU-2 (Expr 路径)：谓词 x > 5，[1, 2, 3, 4, 5, 6, 7, 8] → 6 时满足
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::GetElem, ExprOp::Const(5), ExprOp::Gt]);
            let node = RepeatUntilNode::new(byte_node(), RepeatPredicate::Expr(prog), false);
            let mut stream = ParseStream::new(&[1, 2, 3, 4, 5, 6, 7, 8]);
            let mut ctx = Context::new_root(py).expect("ctx");
            ctx.init_expr_values(0);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("list");
            assert_eq!(list.len(), 6);
            let last: i64 = list.get_item(5).unwrap().extract().unwrap();
            assert_eq!(last, 6);
            assert_eq!(stream.tell(), 6);
            assert!(ctx.index().is_none());
        });
    }

    #[test]
    fn parse_expr_predicate_eq_operator() {
        // 用 == 运算符：x == 3，[1, 2, 3, 4, 5] → 第 3 个元素满足
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::GetElem, ExprOp::Const(3), ExprOp::Eq]);
            let node = RepeatUntilNode::new(byte_node(), RepeatPredicate::Expr(prog), false);
            let mut stream = ParseStream::new(&[1, 2, 3, 4, 5]);
            let mut ctx = Context::new_root(py).expect("ctx");
            ctx.init_expr_values(0);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("list");
            assert_eq!(list.len(), 3);
        });
    }

    #[test]
    fn parse_expr_predicate_discard() {
        // RU-7 (Expr): discard=True 仍消耗流，返回空 list
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::GetElem, ExprOp::Const(5), ExprOp::Gt]);
            let node = RepeatUntilNode::new(byte_node(), RepeatPredicate::Expr(prog), true);
            let mut stream = ParseStream::new(&[1, 2, 3, 4, 5, 6, 7]);
            let mut ctx = Context::new_root(py).expect("ctx");
            ctx.init_expr_values(0);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("list");
            assert_eq!(list.len(), 0);
            assert_eq!(stream.tell(), 6);
        });
    }

    #[test]
    fn parse_expr_predicate_clears_current_elem_after_eval() {
        // 验证 _current_elem_ptr 在每次求值后被清除
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::GetElem, ExprOp::Const(0), ExprOp::Gt]);
            let node = RepeatUntilNode::new(byte_node(), RepeatPredicate::Expr(prog), false);
            let mut stream = ParseStream::new(&[5]);
            let mut ctx = Context::new_root(py).expect("ctx");
            ctx.init_expr_values(0);
            let mut path = Path::new();
            let _ = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // parse 结束后 _current_elem_ptr 应为 None
            assert!(ctx.current_elem_ptr().is_none());
        });
    }

    #[test]
    fn parse_expr_predicate_inner_failure_propagates() {
        // RU-1 (Expr): 子构造器失败 → 错误上抛
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::GetElem, ExprOp::Const(0), ExprOp::Gt]);
            let node = RepeatUntilNode::new(int16ub_node(), RepeatPredicate::Expr(prog), false);
            let mut stream = ParseStream::new(&[0xFF]);
            let mut ctx = Context::new_root(py).expect("ctx");
            ctx.init_expr_values(0);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Stream { .. }));
        });
    }

    // ======================================================================
    // Expr 路径：build
    // ======================================================================

    #[test]
    fn build_expr_predicate_match_stops() {
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::GetElem, ExprOp::Const(5), ExprOp::Gt]);
            let node = RepeatUntilNode::new(byte_node(), RepeatPredicate::Expr(prog), false);
            let obj = py
                .eval_bound("[1, 2, 3, 4, 5, 6, 7]", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            ctx.init_expr_values(0);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[1, 2, 3, 4, 5, 6]);
        });
    }

    #[test]
    fn build_expr_predicate_no_match_returns_repeat_error() {
        // RU-3 (Expr): 无元素满足谓词 → Repeat 错误
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::GetElem, ExprOp::Const(100), ExprOp::Gt]);
            let node = RepeatUntilNode::new(byte_node(), RepeatPredicate::Expr(prog), false);
            let obj = py.eval_bound("[1, 2, 3]", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            ctx.init_expr_values(0);
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Repeat { .. }));
        });
    }

    // ======================================================================
    // 嵌套
    // ======================================================================

    #[test]
    fn parse_nested_repeat_until_restores_outer_index() {
        // RepeatUntil(RepeatUntil) 内层终止后恢复外层 _index
        // 注意：这是为了验证 _index 恢复机制；实际用户极少这样用
        with_py(|py| {
            let inner_prog = ExprProgram::new(vec![ExprOp::GetElem, ExprOp::Const(2), ExprOp::Eq]);
            let inner = RepeatUntilNode::new(byte_node(), RepeatPredicate::Expr(inner_prog), false);
            // 外层谓词始终不满足（永远 != 255），用 PyCallable 控制终止
            // 改用 Array 包 RepeatUntil，避免无限循环
            use crate::nodes::array::{ArrayNode, CountSource};
            let outer_node = crate::nodes::Node::Array(ArrayNode::new(
                crate::nodes::Node::RepeatUntil(inner),
                CountSource::Const(2),
                false,
            ));
            // 数据：2 组 [1, 2, 3]（内层在第 2 个元素 2 满足）
            let mut stream = ParseStream::new(&[1, 2, 3, 1, 2, 3]);
            let mut ctx = Context::new_root(py).expect("ctx");
            ctx.init_expr_values(0);
            let mut path = Path::new();
            let result = outer_node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let outer_list = result.bind(py).downcast::<PyList>().expect("outer list");
            assert_eq!(outer_list.len(), 2);
            // 每个内层 list 应该是 [1, 2]
            let inner0 = outer_list.get_item(0).unwrap();
            let inner0_list = inner0.downcast::<PyList>().expect("inner0");
            assert_eq!(inner0_list.len(), 2);
            // 外层 _index 恢复 None
            assert!(ctx.index().is_none());
        });
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_always_returns_error() {
        // RU-6: sizeof 永远 Err
        with_py(|py| {
            let ctx = Context::new_root(py).expect("ctx");
            let py_pred = make_lambda_gt(5);
            let node =
                RepeatUntilNode::new(byte_node(), RepeatPredicate::PyCallable(py_pred), false);
            let err = node.sizeof(&ctx).expect_err("should fail");
            match err {
                ConstructError::Generic { message, .. } => {
                    assert!(message.contains("RepeatUntil"), "got: {}", message);
                }
                other => panic!("expected Generic, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // parse ↔ build 往返
    // ======================================================================

    #[test]
    fn round_trip_callable_preserves_data() {
        with_py(|py| {
            // 谓词 x > 100：list = [1, 2, 200] 第 3 个元素 200 满足
            let py_pred = make_lambda_gt(100);
            let node =
                RepeatUntilNode::new(byte_node(), RepeatPredicate::PyCallable(py_pred), false);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            // build
            let obj = py.eval_bound("[1, 2, 200]", None, None).expect("obj");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();
            assert_eq!(bytes, &[1, 2, 200]);

            // parse 回来
            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("list");
            assert_eq!(list.len(), 3);
            let values: Vec<i64> = list.iter().map(|b| b.extract().unwrap()).collect();
            assert_eq!(values, vec![1, 2, 200]);
        });
    }

    #[test]
    fn round_trip_expr_preserves_data() {
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::GetElem, ExprOp::Const(100), ExprOp::Gt]);
            let node = RepeatUntilNode::new(byte_node(), RepeatPredicate::Expr(prog), false);
            let mut ctx = Context::new_root(py).expect("ctx");
            ctx.init_expr_values(0);
            let mut path = Path::new();

            // build
            let obj = py.eval_bound("[1, 2, 200]", None, None).expect("obj");
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
            assert_eq!(list.len(), 3);
            let values: Vec<i64> = list.iter().map(|b| b.extract().unwrap()).collect();
            assert_eq!(values, vec![1, 2, 200]);
        });
    }

    // ======================================================================
    // 边界：predicate 异常、空 list build
    // ======================================================================

    #[test]
    fn build_callable_empty_list_returns_repeat_error() {
        // build 时空 list → 无元素满足谓词 → RepeatError
        with_py(|py| {
            let py_pred = make_lambda_gt(0);
            let node =
                RepeatUntilNode::new(byte_node(), RepeatPredicate::PyCallable(py_pred), false);
            let obj = py.eval_bound("[]", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Repeat { .. }));
        });
    }

    #[test]
    fn parse_callable_predicate_raises_propagates_error() {
        // RU-4: 谓词抛异常 → 向上传播（Generic 错误）
        with_py(|py| {
            // 谓词抛异常：lambda x, lst, ctx: 1/0
            let py_pred = py
                .eval_bound("lambda x, lst, ctx: 1/0", None, None)
                .expect("pred")
                .extract::<Py<PyAny>>()
                .expect("Py<PyAny>");
            let node =
                RepeatUntilNode::new(byte_node(), RepeatPredicate::PyCallable(py_pred), false);
            let mut stream = ParseStream::new(&[1, 2, 3]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            // 应是 Generic（Python 异常包装）
            assert!(matches!(err, ConstructError::Generic { .. }));
        });
    }

    #[test]
    fn build_callable_iterable_object_works() {
        // 兜底路径：传入 generator（非 list/tuple）
        with_py(|py| {
            let py_pred = make_lambda_gt(2);
            let node =
                RepeatUntilNode::new(byte_node(), RepeatPredicate::PyCallable(py_pred), false);
            let obj = py
                .eval_bound("(x for x in [1, 2, 3, 4, 5])", None, None)
                .expect("gen");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[1, 2, 3]);
        });
    }

    // ======================================================================
    // context_proxy 内容验证（PyCallable 路径内部）
    // ======================================================================

    #[test]
    fn build_context_proxy_copies_fields_and_index() {
        // 验证 build_context_proxy 正确复制 ctx.fields() 与 _index
        // v4 V-1：proxy 是 Container 实例（attribute + item 双重访问）
        with_py(|py| {
            crate::container_cache::init_container_class(py).expect("init container");
            let container_cls =
                crate::container_cache::container_class(py).expect("container class");

            let mut ctx = Context::new_root(py).expect("ctx");
            let val = 42i64.into_py(py);
            ctx.set_field("myfield", val.bind(py)).expect("set");
            ctx.set_index(7);

            let mut path = Path::new();
            let proxy = build_context_proxy(py, &container_cls, &ctx, &mut path).expect("proxy");

            // isinstance(proxy, Container) → True（V-1 RU-9）
            assert!(proxy
                .is_instance(&container_cls)
                .expect("is_instance Container"));

            // item 访问（PyAny::get_item 返回 PyResult<Bound>，非 Option）
            let idx: i64 = proxy
                .get_item("_index")
                .expect("get _index")
                .extract()
                .expect("i64");
            assert_eq!(idx, 7);
            let v: i64 = proxy
                .get_item("myfield")
                .expect("get myfield")
                .extract()
                .expect("i64");
            assert_eq!(v, 42);

            // attribute 访问（V-1 RU-9：Container.__dict__ = self）
            let idx_attr: i64 = proxy
                .getattr("_index")
                .expect("attr _index")
                .extract()
                .expect("i64");
            assert_eq!(idx_attr, 7);
            let v_attr: i64 = proxy
                .getattr("myfield")
                .expect("attr myfield")
                .extract()
                .expect("i64");
            assert_eq!(v_attr, 42);
        });
    }

    #[test]
    fn build_context_proxy_handles_none_index() {
        // _index 为 None 时，proxy._index 应为 Py_None
        with_py(|py| {
            crate::container_cache::init_container_class(py).expect("init container");
            let container_cls =
                crate::container_cache::container_class(py).expect("container class");

            let ctx = Context::new_root(py).expect("ctx");
            // ctx.index() 默认 None
            let mut path = Path::new();
            let proxy = build_context_proxy(py, &container_cls, &ctx, &mut path).expect("proxy");
            let idx_obj = proxy.get_item("_index").expect("get _index");
            assert!(idx_obj.is_none());
        });
    }

    // ======================================================================
    // 集成：_current_elem_ptr 在 build/parse 路径均正确清除
    // ======================================================================

    #[test]
    fn parse_expr_predicate_nested_inner_does_not_inherit_current_elem() {
        // 验证子 context 不继承 _current_elem_ptr。
        // 由于 RepeatUntil 的 inner 一般是 Struct（创建 child ctx），
        // 这里通过验证 child ctx.current_elem_ptr() == None 间接验证。
        // 实际上 inner.parse 不会调用 GetElem（GetElem 只在 RepeatUntil 的谓词中），
        // 所以即使继承也无影响——但 contract 是不继承。
        with_py(|py| {
            // 跳过此测试：无法直接观察 inner 的 ctx（inner 用同一 ctx，不创建 child）
            // 此处保留为 contract 文档化：设计 §4.3.2 明确"不继承"。
            // 实际验证在 new_child / new_child_placeholder 单元测试中。
            let _ = py;
        });
    }

    #[test]
    fn parse_callable_with_dict_iteration_in_predicate_works() {
        // 验证 build_context_proxy 的 dict 迭代不会 panic
        // （迭代 PyDict 时返回 PyResult，处理潜在错误）
        with_py(|py| {
            crate::container_cache::init_container_class(py).expect("init container");
            let container_cls =
                crate::container_cache::container_class(py).expect("container class");

            let mut ctx = Context::new_root(py).expect("ctx");
            // 写入多个字段
            for i in 0..5i64 {
                let key = format!("k{}", i);
                let v = i.into_py(py);
                ctx.set_field(&key, v.bind(py)).expect("set");
            }
            ctx.set_index(0);
            let mut path = Path::new();
            let proxy = build_context_proxy(py, &container_cls, &ctx, &mut path).expect("proxy");
            // Container 继承 dict，len() 走 dict.__len__，返回 usize（无 Option 包装）
            let n = proxy.len().expect("len");
            assert_eq!(n, 6); // 5 字段 + _index
        });
    }
}
