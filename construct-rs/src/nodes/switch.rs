//! SwitchNode：多分支条件节点（PM 决策 1：A+B 混合方案）。
//!
//! 设计依据：`docs/design/模块设计/模块设计-Conditional.md` §3。
//! Python 参考：`construct/construct/core.py` `Switch`（L4002-4058）。
//!
//! ## 行为概述
//!
//! `Switch(keyfunc, cases, default=Pass)` 求值 keyfunc，根据结果在 cases dict 中
//! 匹配，命中则委托对应 subcon 的 parse/build；未命中走 default。
//!
//! ## 路线 A+B 混合（PM 决策 1）
//!
//! - 路线 A（[`SwitchKey::ConstInt`] / [`SwitchKey::IntExpr`]）：i64 key，
//!   Rust 内 `==` 匹配（零 FFI）；keyfunc 支持 int/bool 常量与字段名表达式
//!   （含算术，如 `n + 1`）
//! - 路线 B（[`SwitchKey::FieldRef`]）：PyObject key，`PyObject_RichCompare` 匹配
//!   （每比较 1 FFI，N=cases 数）；保留变体，当前编译路径仅产出路线 A
//! - keyfunc 不接收 Python callable/lambda（ADR-014 硬约束）
//!
//! ## sizeof 实现（设计 §3.5 注解）
//!
//! `sizeof` 拆分 `match_sub_int`（无需 py token）和 `match_sub_py`（需 py）。
//! `sizeof` 仅调 `match_sub_int`，避免无谓获取 GIL（设计推荐方案）。

use crate::context::Context;
use crate::error::ConstructError;
use crate::expr::{eval_expr_int, ExprProgram};
use crate::nodes::struct_node::FieldName;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::ffi;
use pyo3::prelude::*;

use super::{Construct, Node};

// ---------------------------------------------------------------------------
// SwitchKey
// ---------------------------------------------------------------------------

/// Switch 的 keyfunc 来源（PM 决策 1：A+B 混合，设计 §3.3）。
///
/// 编译期从用户面 keyfunc 分类：
/// - `ConstInt(k)`：常量 int key（罕见，主要用于调试）
/// - `IntExpr(prog)`：字段名 int 表达式（`n` 是 int 字段，含算术如 `n + 1`）→ 路线 A（零 FFI）
/// - `FieldRef(name)`：单字段引用（`tag` 是 str/bytes 字段）→ 路线 B（PyObject __eq__）；
///   保留变体，当前编译路径仅产出 ConstInt / IntExpr
///
/// keyfunc 不接收 Python callable/lambda（ADR-014 硬约束）。
///
/// [设计质疑]：设计文档 §3.3 标注 `#[derive(Debug, Clone)]`，但 `FieldName`
/// 未实现 `Clone`。删除 `Clone` derive，与 `IfThenElseNode` 等持有非 Clone
/// 字段的类型保持一致。
#[derive(Debug)]
pub enum SwitchKey {
    /// 常量 int key（编译期已知）。
    ConstInt(i64),
    /// 单字段 int 表达式（路线 A）。
    /// 运行时求值 ExprProgram 得 i64 → Rust 内 i64 == 匹配 cases（零 FFI）。
    IntExpr(ExprProgram),
    /// 单字段引用（路线 B）。
    /// 运行时从 ctx 读 PyObject → cases 用 PyObject `__eq__` 匹配（每比较 1 FFI）。
    FieldRef(FieldName),
}

// ---------------------------------------------------------------------------
// SwitchCase
// ---------------------------------------------------------------------------

/// 单个 case 项：Python key + 预编译的 subcon（设计 §3.3）。
///
/// 编译期从 Python dict `{key: subcon}` 展开。每个 case 同时缓存：
/// - `key_py`：Python 侧 key（int/str/bytes），用于 PyObject `__eq__`
/// - `key_int`：若 key 是 int，额外存 i64 用于零 FFI 快速匹配（路线 A 快速路径）
#[derive(Debug)]
pub struct SwitchCase {
    /// Python 侧 key（原始对象，用于 PyObject `__eq__`）。
    key_py: Py<PyAny>,
    /// 若 key 是 int，缓存其 i64 值（路线 A 快速匹配用）；否则 None。
    key_int: Option<i64>,
    /// 该 key 对应的子树。
    subcon: Box<Node>,
}

impl SwitchCase {
    /// 创建一个 `SwitchCase`，自动从 Python key 提取 i64 缓存（若可）。
    ///
    /// # 参数
    ///
    /// - `key_py`：Python 侧 key（int / str / bytes 等）
    /// - `subcon`：该 key 对应的子节点
    pub fn new(py: Python<'_>, key_py: Py<PyAny>, subcon: Node) -> Self {
        // 尝试 extract i64（int key 才有快速路径）；失败则 key_int = None。
        // Python bool 也是 int（True == 1, False == 0），extract::<i64>() 成功。
        let key_int = key_py.bind(py).extract::<i64>().ok();
        Self {
            key_py,
            key_int,
            subcon: Box::new(subcon),
        }
    }

    /// 返回 Python 侧 key 引用。
    pub fn key_py(&self) -> &Py<PyAny> {
        &self.key_py
    }

    /// 返回 i64 快速匹配值（若 key 是 int）。
    pub fn key_int(&self) -> Option<i64> {
        self.key_int
    }

    /// 返回子节点引用。
    pub fn subcon(&self) -> &Node {
        &self.subcon
    }
}

// ---------------------------------------------------------------------------
// SwitchNode
// ---------------------------------------------------------------------------

/// 多分支条件节点（对应 Python construct `Switch`，core.py L4002）。
///
/// # 三方法行为
///
/// 对齐 Python `Switch._parse` / `_build` / `_sizeof`（L4041-4058）：
/// - parse：求 key → 匹配 cases → 委托 subcon.parse（未命中走 default）
/// - build：对称
/// - sizeof：仅 [`SwitchKey::ConstInt`] 返回确定路径；其他返回 Err（无法在 sizeof 求值）
///
/// # 路线 A+B 混合匹配（PM 决策 1）
///
/// 求出 key 后的匹配顺序（核心优化，设计 §3.4）：
/// 1. **路线 A 快速路径**：若 key 是 IntExpr 求值结果 i64，遍历 cases 的 `key_int`，
///    Rust 内 i64 == 匹配（零 FFI）
/// 2. **路线 B 慢路径**：快速路径未命中，或 key 是 FieldRef(PyObject)，遍历 cases
///    用 `PyObject_RichCompare` 求 `__eq__`（每比较 1 FFI）
/// 3. **default 兜底**：全部未命中，委托 default subcon
///
/// # 已知限制（与 Python 不完全 parity，设计 §12-D2）
///
/// - `default=Error` 不支持：Python 的 `Error` 构造器 Phase 7 不实现
/// - keyfunc 不接收 Python callable/lambda（ADR-014 硬约束）
#[derive(Debug)]
pub struct SwitchNode {
    /// keyfunc 编译期分类。
    key: SwitchKey,
    /// 编译期从 Python dict 展开的 cases 列表。
    cases: Vec<SwitchCase>,
    /// 默认 subcon（编译期保证非空——Python `default=None` 时由 Python 侧替换为 Pass）。
    default: Box<Node>,
}

impl SwitchNode {
    /// 创建 `SwitchNode`，包含给定的 key、cases 与 default subcon。
    pub fn new(key: SwitchKey, cases: Vec<SwitchCase>, default: Node) -> Self {
        Self {
            key,
            cases,
            default: Box::new(default),
        }
    }

    /// 返回 keyfunc 分类引用。
    pub fn key(&self) -> &SwitchKey {
        &self.key
    }

    /// 返回 cases 切片。
    pub fn cases(&self) -> &[SwitchCase] {
        &self.cases
    }

    /// 返回 default subcon 引用。
    pub fn default(&self) -> &Node {
        &self.default
    }

    /// has_expressions：IntExpr 与 FieldRef 都引用 Struct 字段，返回 true。
    /// ConstInt 不引用字段，返回 false（设计 §3.3）。
    pub fn has_expressions(&self) -> bool {
        !matches!(self.key, SwitchKey::ConstInt(_))
    }

    /// 求值 keyfunc，返回 (i64 快速匹配值, PyObject 慢匹配值)（设计 §3.4）。
    ///
    /// - [`SwitchKey::ConstInt`] → `(Some(k), None)` —— 仅走快速路径
    /// - [`SwitchKey::IntExpr`] → `(Some(求值结果), None)` —— 仅走快速路径
    /// - [`SwitchKey::FieldRef`] → `(None, Some(PyObject))` —— 仅走慢路径
    ///
    /// IntExpr 求值失败（字段缺失）错误向上传播（与 Computed 同路径）。
    /// FieldRef 字段缺失返回 [`ConstructError::ExprFieldMissing`]。
    fn eval_key(
        &self,
        ctx: &Context<'_>,
        py: Python<'_>,
    ) -> Result<(Option<i64>, Option<Py<PyAny>>), ConstructError> {
        match &self.key {
            SwitchKey::ConstInt(k) => Ok((Some(*k), None)),
            SwitchKey::IntExpr(prog) => {
                // 复用 ExprProgram 通用求值（GetInt 单 op 是 try_eval_simple_cmp
                // 未覆盖的模式；直接走 eval_expr_int）。
                let v = eval_expr_int(prog, ctx, py)?;
                Ok((Some(v), None))
            }
            SwitchKey::FieldRef(name) => {
                // 从 ctx.fields() 读 PyObject（key = interned field name）。
                let val = ctx
                    .get_field(name.rust_name())
                    .map_err(|e| ConstructError::Generic {
                        message: format!(
                            "Switch FieldRef failed to read field '{}': {}",
                            name.rust_name(),
                            e
                        ),
                        path: String::new(),
                    })?;
                let val = val.ok_or_else(|| ConstructError::ExprFieldMissing {
                    field: name.rust_name().to_string(),
                    path: String::new(),
                })?;
                Ok((None, Some(val.unbind())))
            }
        }
    }

    /// 路线 A 快速路径：i64 匹配 cases，返回命中 subcon 引用（设计 §3.5 拆分）。
    ///
    /// 不需要 py token（纯 Rust i64 == 比较），可在 sizeof 接口调用。
    fn match_sub_int(&self, key_int: i64) -> &Node {
        for case in &self.cases {
            if let Some(ck) = case.key_int {
                if ck == key_int {
                    return &case.subcon;
                }
            }
        }
        &self.default
    }

    /// 路线 B 慢路径：PyObject `__eq__` 匹配 cases（设计 §3.5 拆分）。
    ///
    /// 需要 py token（调 `PyObject_RichCompare`）。
    fn match_sub_py<'a>(&'a self, key_py: &Py<PyAny>, py: Python<'_>) -> &'a Node {
        for case in &self.cases {
            if py_eq(key_py, &case.key_py, py) {
                return &case.subcon;
            }
        }
        &self.default
    }
}

// ---------------------------------------------------------------------------
// py_eq：PyObject __eq__ 容错比较
// ---------------------------------------------------------------------------

/// PyObject `__eq__` 比较（CPython `PyObject_RichCompare` 包装，设计 §3.4）。
///
/// 返回 `true` 当且仅当 `a == b`（Py_EQ）且结果非异常。
/// 异常（如自定义 `__eq__` 抛错）按 Python 语义视为不匹配（返回 false），
/// 与 Python `dict.get` 的 `__hash__ + __eq__` 一致。
fn py_eq(a: &Py<PyAny>, b: &Py<PyAny>, _py: Python<'_>) -> bool {
    // SAFETY: 持有 GIL（py token），a/b 是有效 PyObject。
    let result = unsafe { ffi::PyObject_RichCompare(a.as_ptr(), b.as_ptr(), ffi::Py_EQ) };
    if result.is_null() {
        // 比较抛异常：清除异常，按不匹配处理（Python dict.get 语义）。
        unsafe {
            ffi::PyErr_Clear();
        }
        return false;
    }
    // SAFETY: result 非 null，是 PyObject_RichCompare 返回的新引用（owned）。
    let is_true = unsafe { ffi::PyObject_IsTrue(result) } == 1;
    unsafe {
        ffi::Py_DecRef(result);
    }
    is_true
}

// ---------------------------------------------------------------------------
// Construct impl
// ---------------------------------------------------------------------------

impl Construct for SwitchNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let (key_int, key_py) = self.eval_key(ctx, py)?;
        let sub = match (key_int, key_py.as_ref()) {
            (Some(k), _) => self.match_sub_int(k),
            (None, Some(kp)) => self.match_sub_py(kp, py),
            // eval_key 的返回保证二者恰一为 Some，不会到达此分支
            (None, None) => &self.default,
        };
        sub.parse(py, stream, ctx, path)
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        let (key_int, key_py) = self.eval_key(ctx, py)?;
        let sub = match (key_int, key_py.as_ref()) {
            (Some(k), _) => self.match_sub_int(k),
            (None, Some(kp)) => self.match_sub_py(kp, py),
            (None, None) => &self.default,
        };
        sub.build(py, obj, stream, ctx, path)
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // 对齐 Python L4051-4058：try evaluate(keyfunc) → match → sizeof；
        // except (KeyError, AttributeError) → SizeofError。
        //
        // sizeof 接口无 py token，无法求值表达式。处理（设计 §3.5）：
        // - ConstInt(k) → match_sub_int → sizeof（确定路径）
        // - IntExpr / FieldRef → 返回 Generic（SizeofError 等价）
        match &self.key {
            SwitchKey::ConstInt(k) => {
                let sub = self.match_sub_int(*k);
                sub.sizeof(ctx)
            }
            SwitchKey::IntExpr(_) | SwitchKey::FieldRef(_) => Err(ConstructError::Generic {
                message: "Switch size is undefined when keyfunc is not a constant".to_string(),
                path: String::new(),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{ExprOp, ExprProgram};
    use crate::nodes::format_field::{FormatFieldNode, PythonFormat};
    use crate::nodes::pass::PassNode;
    use crate::nodes::struct_node::FieldName;
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

    /// 构造 IntExpr keyfunc：GetInt(idx)（单字段引用，求值结果即字段值）。
    fn int_expr_key(idx: usize) -> ExprProgram {
        ExprProgram::new(vec![ExprOp::GetInt(idx)])
    }

    /// 构造一个 SwitchCase（Python int key + FormatField subcon）。
    fn int_case(py: Python<'_>, key: i64, subcon: Node) -> SwitchCase {
        SwitchCase::new(py, key.into_py(py), subcon)
    }

    /// 构造一个 SwitchCase（Python str key + FormatField subcon）。
    fn str_case(py: Python<'_>, key: &str, subcon: Node) -> SwitchCase {
        SwitchCase::new(py, key.into_py(py), subcon)
    }

    // ======================================================================
    // 构造器
    // ======================================================================

    #[test]
    fn new_stores_key_cases_default() {
        with_py(|py| {
            let cases = vec![int_case(py, 1, fmt_node(PythonFormat::UnsignedInt8Big))];
            let node = SwitchNode::new(SwitchKey::ConstInt(1), cases, Node::Pass(PassNode::new()));
            assert!(matches!(node.key(), SwitchKey::ConstInt(_)));
            assert_eq!(node.cases().len(), 1);
            assert!(matches!(node.default(), Node::Pass(_)));
        });
    }

    #[test]
    fn has_expressions_depends_on_key_kind() {
        with_py(|py| {
            // ConstInt → false
            let node = SwitchNode::new(SwitchKey::ConstInt(1), vec![], Node::Pass(PassNode::new()));
            assert!(!node.has_expressions());

            // IntExpr → true
            let node = SwitchNode::new(
                SwitchKey::IntExpr(int_expr_key(0)),
                vec![],
                Node::Pass(PassNode::new()),
            );
            assert!(node.has_expressions());

            // FieldRef → true
            let fname = FieldName::new(py, "tag");
            let node = SwitchNode::new(
                SwitchKey::FieldRef(fname),
                vec![],
                Node::Pass(PassNode::new()),
            );
            assert!(node.has_expressions());
        });
    }

    #[test]
    fn switch_case_extracts_int_key_cache() {
        with_py(|py| {
            let case = int_case(py, 42, fmt_node(PythonFormat::UnsignedInt8Big));
            assert_eq!(case.key_int(), Some(42));
        });
    }

    #[test]
    fn switch_case_str_key_has_no_int_cache() {
        with_py(|py| {
            let case = str_case(py, "foo", fmt_node(PythonFormat::UnsignedInt8Big));
            assert_eq!(case.key_int(), None);
        });
    }

    // ======================================================================
    // SW-1: cases = {} + default=Pass → 任意 key 走 default
    // ======================================================================

    #[test]
    fn parse_empty_cases_with_pass_default_returns_none() {
        with_py(|py| {
            let node =
                SwitchNode::new(SwitchKey::ConstInt(99), vec![], Node::Pass(PassNode::new()));
            let mut stream = ParseStream::new(&[]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let r = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert!(r.bind(py).is_none(), "Pass default returns None");
        });
    }

    // ======================================================================
    // SW-2/SW-3: 路线 A IntExpr 快速匹配 / 未命中走 default
    // ======================================================================

    #[test]
    fn parse_int_expr_key_matches_case() {
        // SW-2: keyfunc=n, n=2, cases={1:A, 2:B} → 匹配 B (Int16ub)
        with_py(|py| {
            use pyo3::types::PyString;
            let cases = vec![
                int_case(py, 1, fmt_node(PythonFormat::UnsignedInt8Big)),
                int_case(py, 2, fmt_node(PythonFormat::UnsignedInt16Big)),
            ];
            let node = SwitchNode::new(
                SwitchKey::IntExpr(int_expr_key(0)),
                cases,
                Node::Pass(PassNode::new()),
            );
            let mut stream = ParseStream::new(&[0xAA, 0xBB]);
            let mut ctx = Context::new_root(py).expect("ctx");
            ctx.init_expr_values(1);
            let key = PyString::new_bound(py, "n").unbind();
            let val = 2i64.into_py(py);
            ctx.set_field_at(0, &key, val.bind(py), py).expect("set");
            let mut path = Path::new();
            let r = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = r.bind(py).extract().expect("i64");
            assert_eq!(v, 0xAABB);
            assert_eq!(stream.tell(), 2);
        });
    }

    #[test]
    fn parse_int_expr_key_no_match_falls_to_default() {
        // SW-3: keyfunc=n, n=99, cases={1:A, 2:B} → default (Pass)
        with_py(|py| {
            use pyo3::types::PyString;
            let cases = vec![
                int_case(py, 1, fmt_node(PythonFormat::UnsignedInt8Big)),
                int_case(py, 2, fmt_node(PythonFormat::UnsignedInt16Big)),
            ];
            let node = SwitchNode::new(
                SwitchKey::IntExpr(int_expr_key(0)),
                cases,
                Node::Pass(PassNode::new()),
            );
            let mut stream = ParseStream::new(&[]);
            let mut ctx = Context::new_root(py).expect("ctx");
            ctx.init_expr_values(1);
            let key = PyString::new_bound(py, "n").unbind();
            let val = 99i64.into_py(py);
            ctx.set_field_at(0, &key, val.bind(py), py).expect("set");
            let mut path = Path::new();
            let r = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert!(r.bind(py).is_none(), "default Pass returns None");
        });
    }

    // ======================================================================
    // SW-4: IntExpr 字段缺失 → ExprFieldMissing
    // ======================================================================

    #[test]
    fn parse_int_expr_missing_field_returns_error() {
        with_py(|py| {
            let node = SwitchNode::new(
                SwitchKey::IntExpr(int_expr_key(0)),
                vec![],
                Node::Pass(PassNode::new()),
            );
            let mut stream = ParseStream::new(&[]);
            let mut ctx = Context::new_root(py).expect("ctx");
            ctx.init_expr_values(1);
            // 不设置字段 → ExprFieldMissing
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("ExprFieldMissing");
            assert!(matches!(err, ConstructError::ExprFieldMissing { .. }));
        });
    }

    // ======================================================================
    // SW-5/SW-6: 路线 B FieldRef 慢路径匹配
    // ======================================================================

    #[test]
    fn parse_field_ref_str_key_matches_case() {
        // SW-5: keyfunc=tag, tag="foo", cases={"foo":A, "bar":B} → 匹配 A
        with_py(|py| {
            use pyo3::types::PyString;
            let cases = vec![
                str_case(py, "foo", fmt_node(PythonFormat::UnsignedInt8Big)),
                str_case(py, "bar", fmt_node(PythonFormat::UnsignedInt16Big)),
            ];
            let node = SwitchNode::new(
                SwitchKey::FieldRef(FieldName::new(py, "tag")),
                cases,
                Node::Pass(PassNode::new()),
            );
            let mut stream = ParseStream::new(&[0x42]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let tag_val = PyString::new_bound(py, "foo");
            ctx.set_field("tag", &tag_val).expect("set tag");
            let mut path = Path::new();
            let r = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = r.bind(py).extract().expect("i64");
            assert_eq!(v, 0x42);
        });
    }

    #[test]
    fn parse_field_ref_no_match_falls_to_default() {
        // SW-6: keyfunc=tag, tag="x", cases={"foo":A} → default
        with_py(|py| {
            use pyo3::types::PyString;
            let cases = vec![str_case(py, "foo", fmt_node(PythonFormat::UnsignedInt8Big))];
            let node = SwitchNode::new(
                SwitchKey::FieldRef(FieldName::new(py, "tag")),
                cases,
                Node::Pass(PassNode::new()),
            );
            let mut stream = ParseStream::new(&[]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let tag_val = PyString::new_bound(py, "x");
            ctx.set_field("tag", &tag_val).expect("set tag");
            let mut path = Path::new();
            let r = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert!(r.bind(py).is_none(), "default Pass returns None");
        });
    }

    // ======================================================================
    // ConstInt keyfunc
    // ======================================================================

    #[test]
    fn parse_const_int_key_matches_case() {
        with_py(|py| {
            let cases = vec![
                int_case(py, 1, fmt_node(PythonFormat::UnsignedInt8Big)),
                int_case(py, 2, fmt_node(PythonFormat::UnsignedInt16Big)),
            ];
            let node = SwitchNode::new(SwitchKey::ConstInt(2), cases, Node::Pass(PassNode::new()));
            let mut stream = ParseStream::new(&[0xAA, 0xBB]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let r = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = r.bind(py).extract().expect("i64");
            assert_eq!(v, 0xAABB);
        });
    }

    // ======================================================================
    // build 对称性
    // ======================================================================

    #[test]
    fn build_int_expr_key_matches_case() {
        with_py(|py| {
            use pyo3::types::PyString;
            let cases = vec![
                int_case(py, 1, fmt_node(PythonFormat::UnsignedInt8Big)),
                int_case(py, 2, fmt_node(PythonFormat::UnsignedInt16Big)),
            ];
            let node = SwitchNode::new(
                SwitchKey::IntExpr(int_expr_key(0)),
                cases,
                Node::Pass(PassNode::new()),
            );
            let obj = py.eval_bound("0x42", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            ctx.init_expr_values(1);
            let key = PyString::new_bound(py, "n").unbind();
            let val = 2i64.into_py(py);
            ctx.set_field_at(0, &key, val.bind(py), py).expect("set");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x00, 0x42]);
        });
    }

    #[test]
    fn build_field_ref_str_key_matches_case() {
        with_py(|py| {
            use pyo3::types::PyString;
            let cases = vec![
                str_case(py, "foo", fmt_node(PythonFormat::UnsignedInt8Big)),
                str_case(py, "bar", fmt_node(PythonFormat::UnsignedInt16Big)),
            ];
            let node = SwitchNode::new(
                SwitchKey::FieldRef(FieldName::new(py, "tag")),
                cases,
                Node::Pass(PassNode::new()),
            );
            let obj = py.eval_bound("0x42", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let tag_val = PyString::new_bound(py, "foo");
            ctx.set_field("tag", &tag_val).expect("set tag");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x42]);
        });
    }

    // ======================================================================
    // SW-9/SW-10: sizeof
    // ======================================================================

    #[test]
    fn sizeof_const_int_returns_matched_subcon_size() {
        // SW-10: ConstInt(2) 路径 sizeof 调用 → match cases → 委托 sizeof
        with_py(|py| {
            let cases = vec![
                int_case(py, 1, fmt_node(PythonFormat::UnsignedInt8Big)),
                int_case(py, 2, fmt_node(PythonFormat::UnsignedInt32Big)),
            ];
            let node = SwitchNode::new(SwitchKey::ConstInt(2), cases, Node::Pass(PassNode::new()));
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).expect("sizeof"), 4);
        });
    }

    #[test]
    fn sizeof_int_expr_returns_error() {
        // SW-9: IntExpr 路径 sizeof 调用 → Generic（无法求值）
        with_py(|py| {
            let node = SwitchNode::new(
                SwitchKey::IntExpr(int_expr_key(0)),
                vec![],
                Node::Pass(PassNode::new()),
            );
            let ctx = Context::placeholder(py);
            let err = node.sizeof(&ctx).expect_err("sizeof");
            match err {
                ConstructError::Generic { message, .. } => {
                    assert!(message.contains("Switch"), "got: {}", message);
                }
                other => panic!("expected Generic, got {:?}", other),
            }
        });
    }

    #[test]
    fn sizeof_field_ref_returns_error() {
        with_py(|py| {
            let node = SwitchNode::new(
                SwitchKey::FieldRef(FieldName::new(py, "tag")),
                vec![],
                Node::Pass(PassNode::new()),
            );
            let ctx = Context::placeholder(py);
            assert!(node.sizeof(&ctx).is_err());
        });
    }

    // ======================================================================
    // SW-12: 慢路径 __eq__ 抛异常 → py_eq 捕获 → 视为不匹配
    // ======================================================================

    #[test]
    fn parse_field_ref_eq_raises_treated_as_no_match() {
        // SW-12: cases 中含一个 __eq__ 抛异常的 key（用自定义类）
        // py_eq 捕获 PyErr → 返回 false → 继续 / default
        with_py(|py| {
            // 构造一个 BadEq 类，__eq__ 抛 RuntimeError
            let bad_class = py
                .eval_bound(
                    "type('BadEq', (), {'__eq__': lambda self, o: (_ for _ in ()).throw(RuntimeError('boom'))})",
                    None,
                    None,
                ).expect("create BadEq class");
            let bad_obj = bad_class.call0().expect("instantiate BadEq");
            // case key 是 BadEq 实例（extract::<i64>() 失败 → key_int = None）
            let case = SwitchCase::new(
                py,
                bad_obj.clone().unbind(),
                fmt_node(PythonFormat::UnsignedInt8Big),
            );
            let node = SwitchNode::new(
                SwitchKey::FieldRef(FieldName::new(py, "tag")),
                vec![case],
                Node::Pass(PassNode::new()),
            );
            let mut stream = ParseStream::new(&[]);
            let mut ctx = Context::new_root(py).expect("ctx");
            // tag 字段值是任意 str（与 BadEq 比较 → 抛异常 → py_eq 返回 false）
            use pyo3::types::PyString;
            let tag_val = PyString::new_bound(py, "anything");
            ctx.set_field("tag", &tag_val).expect("set tag");
            let mut path = Path::new();
            let r = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // __eq__ 抛异常 → 视为不匹配 → 走 default (Pass)
            assert!(
                r.bind(py).is_none(),
                "BadEq __eq__ raises → no match → default"
            );
        });
    }

    // ======================================================================
    // Debug
    // ======================================================================

    #[test]
    fn debug_format_includes_switch_node() {
        with_py(|_py| {
            let node = SwitchNode::new(SwitchKey::ConstInt(1), vec![], Node::Pass(PassNode::new()));
            let s = format!("{:?}", node);
            assert!(s.contains("SwitchNode"), "got: {}", s);
        });
    }
}
