//! 表达式系统 VM 核心：指令集、程序结构与栈式求值器。
//!
//! 设计依据：`docs/模块设计-表达式系统.md` §4.1-§4.3。
//!
//! ## 概述
//!
//! 表达式（如 `Bytes(count + 1)` 中的 `count + 1`）在 `__init_subclass__`
//! 编译期被翻译为扁平的 [`ExprOp`] 指令序列（后序遍历），运行时由
//! [`eval_expr_int`] 在 Rust 内部栈式执行——除 [`ExprOp::GetInt`] 必须从
//! `Context` 持有的 `PyDict` 取值外，全程零 FFI。
//!
//! ## 栈语义
//!
//! VM 栈统一为 `i64`，覆盖长度、计数、偏移量等整数场景。指令序列的栈效果：
//!
//! | 指令类别 | 栈效果 |
//! |---------|--------|
//! | `GetInt` / `Const` | push 1（栈深 +1） |
//! | 二元运算（`Add` 等） | pop 2, push 1（栈深 -1） |
//! | 一元运算（`Neg` / `Not`） | pop 1, push 1（栈深 ±0） |
//!
//! 编译期通过 [`compute_max_stack`] 预计算最大栈深度，运行时 [`ExprProgram::max_stack`]
//! 用于 `Vec::with_capacity` 预分配，避免扩容。
//!
//! ## 整数溢出
//!
//! 所有算术/位运算使用 `wrapping_*` 语义（如 [`i64::wrapping_add`]），对齐
//! Python 无溢出异常的行为。编译期不做溢出检查（设计 §11.1）。

use crate::context::Context;
use crate::error::ConstructError;
use pyo3::prelude::*;
use pyo3::types::PyString;

/// 表达式 VM 指令。编译期由 FieldRef/ExprRef 树后序遍历产生。
///
/// 运行时由 [`eval_expr_int`] 在 Rust 内部栈式执行，零 FFI（GetInt 除外，
/// 需从 `Context` 持有的 `PyDict` 取值）。
///
/// VM 栈统一为 `i64`，覆盖长度、计数、偏移量等整数场景。非整数引用
/// （如 Switch 的字符串 key）走单独路径（直接从 `Context` 取 Python 对象）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExprOp {
    /// 从 context 取整数字段值，压入栈顶。
    ///
    /// 参数是字段在当前 StructNode 的 interned 字段名列表中的索引（编译期确定）。
    /// 执行：`ctx.get_int_by_name(names[idx], py)` → `stack.push(i64)`。
    GetInt(usize),

    /// 压入编译期常量整数。
    Const(i64),

    // --- 算术运算（二元，弹出栈顶两个 a/b，压入结果）---
    /// 栈顶弹出 b、a，压入 `a + b`（[`i64::wrapping_add`]，对齐 Python 无溢出异常语义）。
    Add,
    /// 栈顶弹出 b、a，压入 `a - b`（[`i64::wrapping_sub`]）。
    Sub,
    /// 栈顶弹出 b、a，压入 `a * b`（[`i64::wrapping_mul`]）。
    Mul,
    /// 栈顶弹出 b、a，压入 `a // b`（向负无穷取整，对齐 Python `//` 语义；
    /// `b == 0` 返回 [`ConstructError::ExprDivByZero`]）。
    FloorDiv,
    /// 栈顶弹出 b、a，压入 `a % b`（结果符号与除数一致，对齐 Python `%` 语义；
    /// `b == 0` 返回 [`ConstructError::ExprDivByZero`]）。
    Mod,

    // --- 位运算（二元）---
    /// 栈顶弹出 b、a，压入 `a & b`。
    BitAnd,
    /// 栈顶弹出 b、a，压入 `a | b`。
    BitOr,
    /// 栈顶弹出 b、a，压入 `a ^ b`。
    BitXor,
    /// 栈顶弹出 b、a，压入 `a << b`（[`i64::wrapping_shl`]，b 被 cast 为 u32）。
    Shl,
    /// 栈顶弹出 b、a，压入 `a >> b`（[`i64::wrapping_shr`]，b 被 cast 为 u32）。
    Shr,

    // --- 一元运算 ---
    /// 栈顶弹出 a，压入 `-a`（[`i64::wrapping_neg`]）。
    Neg,
    /// 栈顶弹出 a，压入 `!a`（按位取反）。
    Not,

    // --- 比较运算（二元，弹出 a/b，压入 0 或 1）---
    /// 栈顶弹出 b、a，压入 `(a == b) as i64`。
    Eq,
    /// 栈顶弹出 b、a，压入 `(a != b) as i64`。
    Ne,
    /// 栈顶弹出 b、a，压入 `(a < b) as i64`。
    Lt,
    /// 栈顶弹出 b、a，压入 `(a <= b) as i64`。
    Le,
    /// 栈顶弹出 b、a，压入 `(a > b) as i64`。
    Gt,
    /// 栈顶弹出 b、a，压入 `(a >= b) as i64`。
    Ge,
}

/// 编译后的表达式程序：指令序列 + 求值所需的最大栈深度。
///
/// 存储在 Node 中（如 BytesNode、ComputedNode、SwitchNode），parse/build 时
/// 传给 [`eval_expr_int`] 求值。
///
/// `max_stack` 在 [`ExprProgram::new`] 中通过 [`compute_max_stack`] 自动计算，
/// 用于运行时 `Vec::with_capacity` 预分配，避免扩容开销。
#[derive(Debug, Clone)]
pub struct ExprProgram {
    /// 指令序列（后序遍历产生）。
    ops: Vec<ExprOp>,
    /// 最大栈深度（编译期计算，用于 `Vec::with_capacity` 预分配）。
    max_stack: usize,
}

impl ExprProgram {
    /// 从 `ExprOp` 列表构建，自动计算 `max_stack`。
    ///
    /// # 示例
    ///
    /// `count + 1` 编译为：
    /// ```rust,ignore
    /// ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(1), ExprOp::Add])
    /// ```
    pub fn new(ops: Vec<ExprOp>) -> Self {
        let max_stack = compute_max_stack(&ops);
        Self { ops, max_stack }
    }

    /// 空表达式程序（常用于无表达式的兼容路径）。
    ///
    /// [`eval_expr_int`] 在空程序上返回错误（无值可求）。
    pub fn empty() -> Self {
        Self {
            ops: Vec::new(),
            max_stack: 0,
        }
    }

    /// 返回是否为空程序（无表达式）。
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// 返回指令序列的切片引用。
    pub fn ops(&self) -> &[ExprOp] {
        &self.ops
    }

    /// 返回预计算的最大栈深度。
    ///
    /// 由 [`compute_max_stack`] 在构造时计算，用于 `Vec::with_capacity` 预分配。
    pub fn max_stack(&self) -> usize {
        self.max_stack
    }
}

/// 编译期计算表达式所需的最大栈深度。
///
/// 模拟执行指令序列，追踪栈高度峰值。规则：
/// - [`ExprOp::GetInt`] / [`ExprOp::Const`]：栈深 +1
/// - 二元运算（[`ExprOp::Add`] 等 16 个）：栈深 -1（`saturating_sub`，防御性）
/// - 一元运算（[`ExprOp::Neg`] / [`ExprOp::Not`]）：栈深 ±0
///
/// 返回追踪过程中的栈深度最大值。
fn compute_max_stack(ops: &[ExprOp]) -> usize {
    let mut depth: usize = 0;
    let mut max: usize = 0;
    for op in ops {
        match op {
            ExprOp::GetInt(_) | ExprOp::Const(_) => {
                depth += 1;
                if depth > max {
                    max = depth;
                }
            }
            // 二元运算：弹 2 压 1，净减 1
            ExprOp::Add
            | ExprOp::Sub
            | ExprOp::Mul
            | ExprOp::FloorDiv
            | ExprOp::Mod
            | ExprOp::BitAnd
            | ExprOp::BitOr
            | ExprOp::BitXor
            | ExprOp::Shl
            | ExprOp::Shr
            | ExprOp::Eq
            | ExprOp::Ne
            | ExprOp::Lt
            | ExprOp::Le
            | ExprOp::Gt
            | ExprOp::Ge => {
                depth = depth.saturating_sub(1);
            }
            // 一元运算：弹 1 压 1，净不变
            ExprOp::Neg | ExprOp::Not => {}
        }
    }
    max
}

/// 在 Rust 内部栈式求值表达式，零 FFI（[`ExprOp::GetInt`] 除外，需从 PyDict 取值）。
///
/// # 参数
///
/// - `program`：编译后的表达式程序（[`ExprProgram`]）
/// - `names`：interned 字段名列表（与 StructNode 的字段顺序对齐），
///   [`ExprOp::GetInt`] 的索引基于此列表取值
/// - `ctx`：当前上下文（持有 `PyDict`），通过
///   [`Context::get_int_by_name`] 取字段值
/// - `py`：GIL token
///
/// # 返回
///
/// 求值结果（`i64`）。求值结束后栈应恰好剩一个值（编译期保证）。
///
/// # 错误
///
/// - [`ConstructError::ExprType`]：`GetInt` 取到的值无法 extract 为 `i64`
/// - [`ConstructError::ExprFieldMissing`]：`GetInt` 引用的字段不存在于 context
/// - [`ConstructError::ExprContext`]：`ctx` 为 placeholder（无 PyDict），无法求值
/// - [`ConstructError::ExprDivByZero`]：`FloorDiv` / `Mod` 除数为 0
/// - [`ConstructError::ExprStackUnderflow`]：栈下溢（指令序列不合法，编译期保证不会发生）
/// - [`ConstructError::Generic`]：传入空程序，或 `GetInt` 索引越界（编译期保证不发生）
pub fn eval_expr_int(
    program: &ExprProgram,
    names: &[Py<PyString>],
    ctx: &Context<'_>,
    py: Python<'_>,
) -> Result<i64, ConstructError> {
    if program.is_empty() {
        return Err(ConstructError::Generic {
            message: "eval_expr_int: empty expression program".to_string(),
            path: String::new(),
        });
    }

    let mut stack: Vec<i64> = Vec::with_capacity(program.max_stack());

    for op in program.ops() {
        match op {
            ExprOp::GetInt(idx) => {
                let name = names.get(*idx).ok_or_else(|| ConstructError::Generic {
                    message: format!(
                        "expression GetInt index {} out of bounds (names has {} entries)",
                        idx,
                        names.len()
                    ),
                    path: String::new(),
                })?;
                let val = ctx.get_int_by_name(name.bind(py), py)?;
                stack.push(val);
            }
            ExprOp::Const(v) => {
                stack.push(*v);
            }
            // 二元算术
            ExprOp::Add => binop(&mut stack, i64::wrapping_add)?,
            ExprOp::Sub => binop(&mut stack, i64::wrapping_sub)?,
            ExprOp::Mul => binop(&mut stack, i64::wrapping_mul)?,
            ExprOp::FloorDiv => {
                let (a, b) = pop2(&mut stack)?;
                if b == 0 {
                    return Err(ConstructError::ExprDivByZero {
                        message: "expression division by zero".to_string(),
                    });
                }
                // Python `//` 语义：向负无穷取整（floor division）。
                // div_euclid 不匹配（欧几里得余数恒非负，Python 余数符号同除数）。
                // 用 wrapping_* 实现真 floor division，永不 panic。
                stack.push(floor_div(a, b));
            }
            ExprOp::Mod => {
                let (a, b) = pop2(&mut stack)?;
                if b == 0 {
                    return Err(ConstructError::ExprDivByZero {
                        message: "expression modulo by zero".to_string(),
                    });
                }
                // Python `%` 语义：结果符号与除数一致（floor modulo）。
                stack.push(floor_rem(a, b));
            }
            // 位运算
            ExprOp::BitAnd => binop(&mut stack, |a, b| a & b)?,
            ExprOp::BitOr => binop(&mut stack, |a, b| a | b)?,
            ExprOp::BitXor => binop(&mut stack, |a, b| a ^ b)?,
            ExprOp::Shl => binop(&mut stack, |a, b| a.wrapping_shl(b as u32))?,
            ExprOp::Shr => binop(&mut stack, |a, b| a.wrapping_shr(b as u32))?,
            // 一元
            ExprOp::Neg => {
                let a = stack.pop().ok_or(ConstructError::ExprStackUnderflow)?;
                stack.push(a.wrapping_neg());
            }
            ExprOp::Not => {
                let a = stack.pop().ok_or(ConstructError::ExprStackUnderflow)?;
                stack.push(!a);
            }
            // 比较
            ExprOp::Eq => cmp(&mut stack, |a, b| a == b)?,
            ExprOp::Ne => cmp(&mut stack, |a, b| a != b)?,
            ExprOp::Lt => cmp(&mut stack, |a, b| a < b)?,
            ExprOp::Le => cmp(&mut stack, |a, b| a <= b)?,
            ExprOp::Gt => cmp(&mut stack, |a, b| a > b)?,
            ExprOp::Ge => cmp(&mut stack, |a, b| a >= b)?,
        }
    }

    stack.pop().ok_or(ConstructError::ExprStackUnderflow)
}

/// 弹出栈顶两个元素 `(a, b)`。
///
/// 弹出顺序：先弹出的是 rhs（`b`），后弹出的是 lhs（`a`）。
/// 栈为空时返回 [`ConstructError::ExprStackUnderflow`]。
fn pop2(stack: &mut Vec<i64>) -> Result<(i64, i64), ConstructError> {
    let b = stack.pop().ok_or(ConstructError::ExprStackUnderflow)?;
    let a = stack.pop().ok_or(ConstructError::ExprStackUnderflow)?;
    Ok((a, b))
}

/// Python 风格的向下取整除法（`//`），对齐 Python `int.__floordiv__`。
///
/// 使用 `wrapping_*` 算术，永不 panic（即使 `i64::MIN / -1` 也不触发硬件异常）。
/// 向负无穷取整，区别于 Rust `/` 的向零取整。
///
/// 示例（与 Python 一致）：`floor_div(-7, 2) = -4`，`floor_div(7, -2) = -4`。
fn floor_div(a: i64, b: i64) -> i64 {
    debug_assert!(b != 0, "division by zero must be checked by caller");
    let q = a.wrapping_div(b);
    let r = a.wrapping_rem(b);
    if r != 0 && ((r < 0) != (b < 0)) {
        q.wrapping_sub(1)
    } else {
        q
    }
}

/// Python 风格的取模（`%`），对齐 Python `int.__mod__`。
///
/// 使用 `wrapping_*` 算术，永不 panic。结果符号与除数一致，
/// 区别于 Rust `%` 的结果符号与被除数一致。
///
/// 示例（与 Python 一致）：`floor_rem(-7, 2) = 1`，`floor_rem(7, -2) = -1`。
fn floor_rem(a: i64, b: i64) -> i64 {
    debug_assert!(b != 0, "modulo by zero must be checked by caller");
    let r = a.wrapping_rem(b);
    if r != 0 && ((r < 0) != (b < 0)) {
        r.wrapping_add(b)
    } else {
        r
    }
}

/// 二元运算辅助：弹出 `(a, b)`，压入 `f(a, b)`。
///
/// 泛型 `F` 接收 `(i64, i64)` 返回 `i64`。栈下溢由 [`pop2`] 处理。
fn binop<F>(stack: &mut Vec<i64>, f: F) -> Result<(), ConstructError>
where
    F: FnOnce(i64, i64) -> i64,
{
    let (a, b) = pop2(stack)?;
    stack.push(f(a, b));
    Ok(())
}

/// 比较运算辅助：弹出 `(a, b)`，压入 `f(a, b) as i64`（0 或 1）。
///
/// 泛型 `F` 接收 `(i64, i64)` 返回 `bool`，结果被 cast 为 `i64`（true→1, false→0）。
fn cmp<F>(stack: &mut Vec<i64>, f: F) -> Result<(), ConstructError>
where
    F: FnOnce(i64, i64) -> bool,
{
    let (a, b) = pop2(stack)?;
    stack.push(if f(a, b) { 1 } else { 0 });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::types::PyString;

    // ======================================================================
    // Python 测试辅助
    // ======================================================================

    /// 在测试运行前手动初始化 Python 解释器（幂等）。
    ///
    /// 参见 `context::tests::ensure_python_initialized` 的说明。
    fn ensure_python_initialized() {
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(pyo3::prepare_freethreaded_python);
    }

    /// 在已初始化的 Python 解释器上执行闭包。
    fn with_python<F, R>(f: F) -> R
    where
        F: for<'py> FnOnce(Python<'py>) -> R,
    {
        ensure_python_initialized();
        Python::with_gil(f)
    }

    /// 构造一个含若干整数字段的 `Context`，用于表达式求值测试。
    ///
    /// `entries` 为 `(字段名, i64 值)` 列表。返回的 `Context` 为 `new_root`，
    /// 持有填充好的 `PyDict`。
    fn make_context<'py>(py: Python<'py>, entries: &[(&str, i64)]) -> Context<'py> {
        let ctx = Context::new_root(py).expect("new_root");
        for (name, value) in entries {
            let val = (*value).into_py(py);
            ctx.set_field(name, val.bind(py)).expect("set_field");
        }
        ctx
    }

    /// 构造 interned PyString 列表（模拟 StructNode 的字段名表）。
    fn make_names<'py>(py: Python<'py>, names: &[&str]) -> Vec<Py<PyString>> {
        names
            .iter()
            .map(|n| PyString::new_bound(py, n).into())
            .collect()
    }

    // ======================================================================
    // ExprProgram / compute_max_stack
    // ======================================================================

    #[test]
    fn empty_program_is_empty() {
        let prog = ExprProgram::empty();
        assert!(prog.is_empty());
        assert_eq!(prog.max_stack(), 0);
        assert_eq!(prog.ops().len(), 0);
    }

    #[test]
    fn new_with_single_const_has_max_stack_1() {
        let prog = ExprProgram::new(vec![ExprOp::Const(42)]);
        assert!(!prog.is_empty());
        assert_eq!(prog.max_stack(), 1);
        assert_eq!(prog.ops().len(), 1);
    }

    #[test]
    fn new_with_single_getint_has_max_stack_1() {
        let prog = ExprProgram::new(vec![ExprOp::GetInt(0)]);
        assert_eq!(prog.max_stack(), 1);
    }

    #[test]
    fn new_with_binary_op_has_max_stack_2() {
        // [GetInt(0), GetInt(1), Add] → 深度 1, 2, 1，峰值 2
        let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::GetInt(1), ExprOp::Add]);
        assert_eq!(prog.max_stack(), 2);
    }

    #[test]
    fn new_with_chained_addition() {
        // count + flag + 1 → [GetInt(0), GetInt(1), Add, Const(1), Add]
        // 深度：1, 2, 1, 2, 1，峰值 2
        let prog = ExprProgram::new(vec![
            ExprOp::GetInt(0),
            ExprOp::GetInt(1),
            ExprOp::Add,
            ExprOp::Const(1),
            ExprOp::Add,
        ]);
        assert_eq!(prog.max_stack(), 2);
    }

    #[test]
    fn new_with_nested_expression() {
        // (a + b) * (c + d) → [G(0), G(1), Add, G(2), G(3), Add, Mul]
        // 深度：1, 2, 1, 2, 3, 2, 1，峰值 3
        let prog = ExprProgram::new(vec![
            ExprOp::GetInt(0),
            ExprOp::GetInt(1),
            ExprOp::Add,
            ExprOp::GetInt(2),
            ExprOp::GetInt(3),
            ExprOp::Add,
            ExprOp::Mul,
        ]);
        assert_eq!(prog.max_stack(), 3);
    }

    #[test]
    fn new_with_unary_ops_preserves_depth() {
        // [G(0), Neg] → 深度 1, 1，峰值 1
        let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Neg]);
        assert_eq!(prog.max_stack(), 1);

        // [G(0), Not, Neg, Not] → 深度 1, 1, 1, 1，峰值 1
        let prog2 = ExprProgram::new(vec![
            ExprOp::GetInt(0),
            ExprOp::Not,
            ExprOp::Neg,
            ExprOp::Not,
        ]);
        assert_eq!(prog2.max_stack(), 1);
    }

    #[test]
    fn new_with_comparison_ops() {
        // [G(0), Const(0), Gt] → 深度 1, 2, 1，峰值 2
        let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(0), ExprOp::Gt]);
        assert_eq!(prog.max_stack(), 2);
    }

    #[test]
    fn compute_max_stack_empty_returns_zero() {
        assert_eq!(compute_max_stack(&[]), 0);
    }

    #[test]
    fn compute_max_stack_all_pushes() {
        let ops = vec![ExprOp::Const(1), ExprOp::Const(2), ExprOp::Const(3)];
        assert_eq!(compute_max_stack(&ops), 3);
    }

    #[test]
    fn ops_returns_slice_reference() {
        let prog = ExprProgram::new(vec![ExprOp::Const(1), ExprOp::Const(2)]);
        assert_eq!(prog.ops(), &[ExprOp::Const(1), ExprOp::Const(2)]);
    }

    #[test]
    fn clone_preserves_ops_and_max_stack() {
        let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(1), ExprOp::Add]);
        let cloned = prog.clone();
        assert_eq!(cloned.ops(), prog.ops());
        assert_eq!(cloned.max_stack(), prog.max_stack());
    }

    // ======================================================================
    // eval_expr_int：常量与字段引用
    // ======================================================================

    #[test]
    fn eval_const_returns_value() {
        with_python(|py| {
            let ctx = Context::placeholder(py);
            let names: Vec<Py<PyString>> = vec![];
            let prog = ExprProgram::new(vec![ExprOp::Const(42)]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("const");
            assert_eq!(result, 42);
        });
    }

    #[test]
    fn eval_const_negative() {
        with_python(|py| {
            let ctx = Context::placeholder(py);
            let names: Vec<Py<PyString>> = vec![];
            let prog = ExprProgram::new(vec![ExprOp::Const(-100)]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("neg const");
            assert_eq!(result, -100);
        });
    }

    #[test]
    fn eval_getint_returns_field_value() {
        with_python(|py| {
            let ctx = make_context(py, &[("count", 7)]);
            let names = make_names(py, &["count"]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0)]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("getint");
            assert_eq!(result, 7);
        });
    }

    #[test]
    fn eval_getint_multiple_fields() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 10), ("b", 20), ("c", 30)]);
            let names = make_names(py, &["a", "b", "c"]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(2)]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("getint idx 2");
            assert_eq!(result, 30);
        });
    }

    // ======================================================================
    // eval_expr_int：算术运算
    // ======================================================================

    #[test]
    fn eval_add_two_fields() {
        with_python(|py| {
            let ctx = make_context(py, &[("count", 3), ("flag", 5)]);
            let names = make_names(py, &["count", "flag"]);
            // count + flag
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::GetInt(1), ExprOp::Add]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("add");
            assert_eq!(result, 8);
        });
    }

    #[test]
    fn eval_count_times_two() {
        with_python(|py| {
            let ctx = make_context(py, &[("count", 6)]);
            let names = make_names(py, &["count"]);
            // count * 2
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(2), ExprOp::Mul]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("mul");
            assert_eq!(result, 12);
        });
    }

    #[test]
    fn eval_chained_addition_count_plus_flag_plus_one() {
        with_python(|py| {
            let ctx = make_context(py, &[("count", 3), ("flag", 5)]);
            let names = make_names(py, &["count", "flag"]);
            // count + flag + 1 → [G(0), G(1), Add, Const(1), Add]
            let prog = ExprProgram::new(vec![
                ExprOp::GetInt(0),
                ExprOp::GetInt(1),
                ExprOp::Add,
                ExprOp::Const(1),
                ExprOp::Add,
            ]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("chained add");
            assert_eq!(result, 9);
        });
    }

    #[test]
    fn eval_sub() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 10), ("b", 3)]);
            let names = make_names(py, &["a", "b"]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::GetInt(1), ExprOp::Sub]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("sub");
            assert_eq!(result, 7);
        });
    }

    #[test]
    fn eval_floor_div() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 17), ("b", 5)]);
            let names = make_names(py, &["a", "b"]);
            let prog =
                ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::GetInt(1), ExprOp::FloorDiv]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("floordiv");
            assert_eq!(result, 3);
        });
    }

    #[test]
    fn eval_mod() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 17), ("b", 5)]);
            let names = make_names(py, &["a", "b"]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::GetInt(1), ExprOp::Mod]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("mod");
            assert_eq!(result, 2);
        });
    }

    #[test]
    fn eval_floor_div_negative_dividend() {
        // Python: -7 // 2 = -4（向负无穷取整，不是 truncating 的 -3）
        with_python(|py| {
            let ctx = make_context(py, &[("a", -7), ("b", 2)]);
            let names = make_names(py, &["a", "b"]);
            let prog =
                ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::GetInt(1), ExprOp::FloorDiv]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("floordiv neg dividend");
            assert_eq!(result, -4);
        });
    }

    #[test]
    fn eval_mod_negative_dividend() {
        // Python: -7 % 2 = 1（结果符号与除数一致）
        with_python(|py| {
            let ctx = make_context(py, &[("a", -7), ("b", 2)]);
            let names = make_names(py, &["a", "b"]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::GetInt(1), ExprOp::Mod]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("mod neg dividend");
            assert_eq!(result, 1);
        });
    }

    #[test]
    fn eval_floor_div_negative_divisor() {
        // Python: 7 // -2 = -4（向负无穷取整，不是 truncating 的 -3）
        with_python(|py| {
            let ctx = make_context(py, &[("a", 7), ("b", -2)]);
            let names = make_names(py, &["a", "b"]);
            let prog =
                ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::GetInt(1), ExprOp::FloorDiv]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("floordiv neg divisor");
            assert_eq!(result, -4);
        });
    }

    #[test]
    fn eval_floor_div_both_negative() {
        // Python: -7 // -1 = 7（负负得正，无取整差异）
        with_python(|py| {
            let ctx = make_context(py, &[("a", -7), ("b", -1)]);
            let names = make_names(py, &["a", "b"]);
            let prog =
                ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::GetInt(1), ExprOp::FloorDiv]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("floordiv both neg");
            assert_eq!(result, 7);
        });
    }

    #[test]
    fn eval_mod_negative_divisor() {
        // Python: 7 % -2 = -1（结果符号与除数一致）
        with_python(|py| {
            let ctx = make_context(py, &[("a", 7), ("b", -2)]);
            let names = make_names(py, &["a", "b"]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::GetInt(1), ExprOp::Mod]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("mod neg divisor");
            assert_eq!(result, -1);
        });
    }

    #[test]
    fn eval_floor_div_mod_i64_min_div_minus_one() {
        // i64::MIN / -1 和 i64::MIN % -1：数学结果分别为 9223372036854775808 和 0。
        // Python 不会报错（任意精度），i64 VM 中 FloorDiv wrapping，Mod 为 0。
        // 确保此边界场景不 panic。
        with_python(|py| {
            let ctx = Context::placeholder(py);
            let names: Vec<Py<PyString>> = vec![];
            // i64::MIN / -1
            let prog_div = ExprProgram::new(vec![
                ExprOp::Const(i64::MIN),
                ExprOp::Const(-1),
                ExprOp::FloorDiv,
            ]);
            let result_div = eval_expr_int(&prog_div, &names, &ctx, py).expect("min div -1");
            assert_eq!(result_div, i64::MIN); // wrapping: 9223372036854775808 mod 2^64

            // i64::MIN % -1
            let prog_mod = ExprProgram::new(vec![
                ExprOp::Const(i64::MIN),
                ExprOp::Const(-1),
                ExprOp::Mod,
            ]);
            let result_mod = eval_expr_int(&prog_mod, &names, &ctx, py).expect("min mod -1");
            assert_eq!(result_mod, 0);
        });
    }

    #[test]
    fn eval_nested_arithmetic() {
        // (a + b) * c → [G(0), G(1), Add, G(2), Mul]
        with_python(|py| {
            let ctx = make_context(py, &[("a", 2), ("b", 3), ("c", 4)]);
            let names = make_names(py, &["a", "b", "c"]);
            let prog = ExprProgram::new(vec![
                ExprOp::GetInt(0),
                ExprOp::GetInt(1),
                ExprOp::Add,
                ExprOp::GetInt(2),
                ExprOp::Mul,
            ]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("nested");
            assert_eq!(result, 20); // (2+3)*4 = 20
        });
    }

    // ======================================================================
    // eval_expr_int：一元运算
    // ======================================================================

    #[test]
    fn eval_neg() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 42)]);
            let names = make_names(py, &["a"]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Neg]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("neg");
            assert_eq!(result, -42);
        });
    }

    #[test]
    fn eval_neg_negative_to_positive() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", -42)]);
            let names = make_names(py, &["a"]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Neg]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("neg");
            assert_eq!(result, 42);
        });
    }

    #[test]
    fn eval_not_bitwise() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 0)]);
            let names = make_names(py, &["a"]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Not]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("not 0");
            assert_eq!(result, -1); // !0 = -1 in i64
        });
    }

    #[test]
    fn eval_not_all_ones() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", -1)]);
            let names = make_names(py, &["a"]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Not]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("not -1");
            assert_eq!(result, 0); // !(-1) = 0
        });
    }

    // ======================================================================
    // eval_expr_int：比较运算
    // ======================================================================

    #[test]
    fn eval_gt_true() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 5)]);
            let names = make_names(py, &["a"]);
            // a > 0
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(0), ExprOp::Gt]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("gt");
            assert_eq!(result, 1);
        });
    }

    #[test]
    fn eval_gt_false() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 0)]);
            let names = make_names(py, &["a"]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(1), ExprOp::Gt]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("gt false");
            assert_eq!(result, 0);
        });
    }

    #[test]
    fn eval_eq_true() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 5)]);
            let names = make_names(py, &["a"]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(5), ExprOp::Eq]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("eq");
            assert_eq!(result, 1);
        });
    }

    #[test]
    fn eval_ne_true() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 5)]);
            let names = make_names(py, &["a"]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(6), ExprOp::Ne]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("ne");
            assert_eq!(result, 1);
        });
    }

    #[test]
    fn eval_lt_le_ge() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 3)]);
            let names = make_names(py, &["a"]);

            // a < 5 → 1
            let prog_lt = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(5), ExprOp::Lt]);
            assert_eq!(eval_expr_int(&prog_lt, &names, &ctx, py).expect("lt"), 1);

            // a <= 3 → 1
            let prog_le = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(3), ExprOp::Le]);
            assert_eq!(eval_expr_int(&prog_le, &names, &ctx, py).expect("le"), 1);

            // a >= 5 → 0
            let prog_ge = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(5), ExprOp::Ge]);
            assert_eq!(eval_expr_int(&prog_ge, &names, &ctx, py).expect("ge"), 0);
        });
    }

    // ======================================================================
    // eval_expr_int：位运算
    // ======================================================================

    #[test]
    fn eval_bitand() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 0xAB)]);
            let names = make_names(py, &["a"]);
            // a & 0xFF → 0xAB
            let prog =
                ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(0xFF), ExprOp::BitAnd]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("bitand");
            assert_eq!(result, 0xAB);
        });
    }

    #[test]
    fn eval_bitor() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 0xF0)]);
            let names = make_names(py, &["a"]);
            // a | 0x0F → 0xFF
            let prog =
                ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(0x0F), ExprOp::BitOr]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("bitor");
            assert_eq!(result, 0xFF);
        });
    }

    #[test]
    fn eval_bitxor() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 0xFF)]);
            let names = make_names(py, &["a"]);
            // a ^ 0x0F → 0xF0
            let prog =
                ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(0x0F), ExprOp::BitXor]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("bitxor");
            assert_eq!(result, 0xF0);
        });
    }

    #[test]
    fn eval_shl() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 1)]);
            let names = make_names(py, &["a"]);
            // a << 4 → 16
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(4), ExprOp::Shl]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("shl");
            assert_eq!(result, 16);
        });
    }

    #[test]
    fn eval_shr() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 256)]);
            let names = make_names(py, &["a"]);
            // a >> 4 → 16
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(4), ExprOp::Shr]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("shr");
            assert_eq!(result, 16);
        });
    }

    // ======================================================================
    // eval_expr_int：错误路径
    // ======================================================================

    #[test]
    fn eval_empty_program_returns_error() {
        with_python(|py| {
            let ctx = Context::placeholder(py);
            let names: Vec<Py<PyString>> = vec![];
            let prog = ExprProgram::empty();
            let result = eval_expr_int(&prog, &names, &ctx, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::Generic { .. }) => {}
                other => panic!("expected Generic, got {:?}", other),
            }
        });
    }

    #[test]
    fn eval_floor_div_by_zero_returns_error() {
        with_python(|py| {
            let ctx = Context::placeholder(py);
            let names: Vec<Py<PyString>> = vec![];
            // 10 / 0
            let prog =
                ExprProgram::new(vec![ExprOp::Const(10), ExprOp::Const(0), ExprOp::FloorDiv]);
            let result = eval_expr_int(&prog, &names, &ctx, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprDivByZero { .. }) => {}
                other => panic!("expected ExprDivByZero, got {:?}", other),
            }
        });
    }

    #[test]
    fn eval_mod_by_zero_returns_error() {
        with_python(|py| {
            let ctx = Context::placeholder(py);
            let names: Vec<Py<PyString>> = vec![];
            // 10 % 0
            let prog = ExprProgram::new(vec![ExprOp::Const(10), ExprOp::Const(0), ExprOp::Mod]);
            let result = eval_expr_int(&prog, &names, &ctx, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprDivByZero { .. }) => {}
                other => panic!("expected ExprDivByZero, got {:?}", other),
            }
        });
    }

    #[test]
    fn eval_stack_underflow_on_binary_op_with_empty_stack() {
        with_python(|py| {
            let ctx = Context::placeholder(py);
            let names: Vec<Py<PyString>> = vec![];
            // [Add] with empty stack → stack underflow
            let prog = ExprProgram::new(vec![ExprOp::Add]);
            let result = eval_expr_int(&prog, &names, &ctx, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprStackUnderflow) => {}
                other => panic!("expected ExprStackUnderflow, got {:?}", other),
            }
        });
    }

    #[test]
    fn eval_stack_underflow_on_unary_op_with_empty_stack() {
        with_python(|py| {
            let ctx = Context::placeholder(py);
            let names: Vec<Py<PyString>> = vec![];
            // [Neg] with empty stack → stack underflow
            let prog = ExprProgram::new(vec![ExprOp::Neg]);
            let result = eval_expr_int(&prog, &names, &ctx, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprStackUnderflow) => {}
                other => panic!("expected ExprStackUnderflow, got {:?}", other),
            }
        });
    }

    #[test]
    fn eval_getint_on_placeholder_context_returns_expr_context_error() {
        with_python(|py| {
            let ctx = Context::placeholder(py);
            let names = make_names(py, &["count"]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0)]);
            let result = eval_expr_int(&prog, &names, &ctx, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprContext { .. }) => {}
                other => panic!("expected ExprContext, got {:?}", other),
            }
        });
    }

    #[test]
    fn eval_getint_missing_field_returns_expr_field_missing_error() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 1)]);
            let names = make_names(py, &["a", "nonexistent"]);
            // 引用 names[1]="nonexistent"，context 中不存在
            let prog = ExprProgram::new(vec![ExprOp::GetInt(1)]);
            let result = eval_expr_int(&prog, &names, &ctx, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprFieldMissing { field }) => {
                    assert_eq!(field, "nonexistent");
                }
                other => panic!("expected ExprFieldMissing, got {:?}", other),
            }
        });
    }

    #[test]
    fn eval_getint_wrong_type_returns_expr_type_error() {
        with_python(|py| {
            let ctx = Context::new_root(py).expect("root");
            // 存入 str 值而非 int
            let str_val = PyString::new_bound(py, "hello");
            ctx.set_field("name", &str_val).expect("set str");
            let names = make_names(py, &["name"]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0)]);
            let result = eval_expr_int(&prog, &names, &ctx, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprType { field, expected }) => {
                    assert_eq!(field, "name");
                    assert!(expected.contains("integer"));
                }
                other => panic!("expected ExprType, got {:?}", other),
            }
        });
    }

    #[test]
    fn eval_getint_index_out_of_bounds_returns_generic_error() {
        // names 只有 1 个条目，GetInt(5) 越界 → 应返回 Generic 错误而非 panic
        with_python(|py| {
            let ctx = make_context(py, &[("count", 1)]);
            let names = make_names(py, &["count"]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(5)]);
            let result = eval_expr_int(&prog, &names, &ctx, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::Generic { message, .. }) => {
                    assert!(
                        message.contains("5"),
                        "error message should contain idx 5, got: {message}"
                    );
                    assert!(
                        message.contains("1"),
                        "error message should contain names len 1, got: {message}"
                    );
                }
                other => panic!("expected Generic, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // eval_expr_int：复杂表达式（模拟真实场景）
    // ======================================================================

    #[test]
    fn eval_bytes_length_expression() {
        // 模拟 Bytes(count) → 仅引用 count
        with_python(|py| {
            let ctx = make_context(py, &[("count", 42)]);
            let names = make_names(py, &["count"]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0)]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("bytes len");
            assert_eq!(result, 42);
        });
    }

    #[test]
    fn eval_bytes_count_plus_flag_expression() {
        // 模拟 Bytes(count + flag)
        with_python(|py| {
            let ctx = make_context(py, &[("count", 10), ("flag", 2)]);
            let names = make_names(py, &["count", "flag"]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::GetInt(1), ExprOp::Add]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("bytes len");
            assert_eq!(result, 12);
        });
    }

    #[test]
    fn eval_computed_end_minus_start() {
        // 模拟 Computed(end - start)
        with_python(|py| {
            let ctx = make_context(py, &[("start", 0), ("end", 4)]);
            let names = make_names(py, &["start", "end"]);
            let prog = ExprProgram::new(vec![
                ExprOp::GetInt(1), // end
                ExprOp::GetInt(0), // start
                ExprOp::Sub,
            ]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("computed");
            assert_eq!(result, 4);
        });
    }

    #[test]
    fn eval_switch_cond_expression() {
        // 模拟 Switch 条件: type_flag == 1
        with_python(|py| {
            let ctx = make_context(py, &[("type_flag", 1)]);
            let names = make_names(py, &["type_flag"]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(1), ExprOp::Eq]);
            let result = eval_expr_int(&prog, &names, &ctx, py).expect("switch cond");
            assert_eq!(result, 1);
        });
    }

    // ======================================================================
    // Context::get_int_by_name
    // ======================================================================

    #[test]
    fn context_get_int_by_name_returns_value() {
        with_python(|py| {
            let ctx = make_context(py, &[("count", 42)]);
            let name = PyString::new_bound(py, "count");
            let result = ctx.get_int_by_name(&name, py).expect("get int");
            assert_eq!(result, 42);
        });
    }

    #[test]
    fn context_get_int_by_name_missing_returns_error() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 1)]);
            let name = PyString::new_bound(py, "nonexistent");
            let result = ctx.get_int_by_name(&name, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprFieldMissing { field }) => {
                    assert_eq!(field, "nonexistent");
                }
                other => panic!("expected ExprFieldMissing, got {:?}", other),
            }
        });
    }

    #[test]
    fn context_get_int_by_name_wrong_type_returns_error() {
        with_python(|py| {
            let ctx = Context::new_root(py).expect("root");
            let str_val = PyString::new_bound(py, "hello");
            ctx.set_field("name", &str_val).expect("set str");
            let name = PyString::new_bound(py, "name");
            let result = ctx.get_int_by_name(&name, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprType { field, expected }) => {
                    assert_eq!(field, "name");
                    assert!(expected.contains("integer"));
                }
                other => panic!("expected ExprType, got {:?}", other),
            }
        });
    }

    #[test]
    fn context_get_int_by_name_on_placeholder_returns_error() {
        with_python(|py| {
            let ctx = Context::placeholder(py);
            let name = PyString::new_bound(py, "count");
            let result = ctx.get_int_by_name(&name, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprContext { message }) => {
                    assert!(message.contains("placeholder"));
                }
                other => panic!("expected ExprContext, got {:?}", other),
            }
        });
    }

    #[test]
    fn context_get_int_by_name_negative_value() {
        with_python(|py| {
            let ctx = make_context(py, &[("offset", -100)]);
            let name = PyString::new_bound(py, "offset");
            let result = ctx.get_int_by_name(&name, py).expect("get negative");
            assert_eq!(result, -100);
        });
    }

    #[test]
    fn context_get_int_by_name_i64_max() {
        with_python(|py| {
            let ctx = make_context(py, &[("max", i64::MAX)]);
            let name = PyString::new_bound(py, "max");
            let result = ctx.get_int_by_name(&name, py).expect("get max");
            assert_eq!(result, i64::MAX);
        });
    }

    // ======================================================================
    // Context::get_obj_by_name
    // ======================================================================

    #[test]
    fn context_get_obj_by_name_returns_value() {
        with_python(|py| {
            let ctx = make_context(py, &[("count", 42)]);
            let name = PyString::new_bound(py, "count");
            let result = ctx.get_obj_by_name(&name).expect("get obj");
            assert!(result.is_some());
            let obj = result.expect("value");
            let n: i64 = obj.extract().expect("extract i64");
            assert_eq!(n, 42);
        });
    }

    #[test]
    fn context_get_obj_by_name_missing_returns_none() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 1)]);
            let name = PyString::new_bound(py, "nonexistent");
            let result = ctx.get_obj_by_name(&name).expect("get obj");
            assert!(result.is_none());
        });
    }

    #[test]
    fn context_get_obj_by_name_on_placeholder_returns_error() {
        with_python(|py| {
            let ctx = Context::placeholder(py);
            let name = PyString::new_bound(py, "count");
            let result = ctx.get_obj_by_name(&name);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprContext { message }) => {
                    assert!(message.contains("placeholder"));
                }
                other => panic!("expected ExprContext, got {:?}", other),
            }
        });
    }

    #[test]
    fn context_get_obj_by_name_returns_non_integer() {
        // get_obj_by_name 不做类型转换，可以取 str 值
        with_python(|py| {
            let ctx = Context::new_root(py).expect("root");
            let str_val = PyString::new_bound(py, "hello");
            ctx.set_field("name", &str_val).expect("set str");
            let name = PyString::new_bound(py, "name");
            let result = ctx.get_obj_by_name(&name).expect("get obj");
            assert!(result.is_some());
            let obj = result.expect("value");
            let s: String = obj.extract().expect("extract str");
            assert_eq!(s, "hello");
        });
    }

    // ======================================================================
    // ExprOp 派生 trait
    // ======================================================================

    #[test]
    fn exprop_is_copy() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<ExprOp>();
    }

    #[test]
    fn exprop_equality() {
        assert_eq!(ExprOp::Const(42), ExprOp::Const(42));
        assert_ne!(ExprOp::Const(42), ExprOp::Const(43));
        assert_eq!(ExprOp::Add, ExprOp::Add);
        assert_ne!(ExprOp::Add, ExprOp::Sub);
        assert_eq!(ExprOp::GetInt(0), ExprOp::GetInt(0));
        assert_ne!(ExprOp::GetInt(0), ExprOp::GetInt(1));
    }

    #[test]
    fn exprop_debug_format() {
        let op = ExprOp::Const(42);
        assert_eq!(format!("{:?}", op), "Const(42)");

        let op = ExprOp::GetInt(3);
        assert_eq!(format!("{:?}", op), "GetInt(3)");

        let op = ExprOp::Add;
        assert_eq!(format!("{:?}", op), "Add");
    }

    // ======================================================================
    // Context 辅助：确保 set_field/get_field 与新方法兼容
    // ======================================================================

    #[test]
    fn make_context_helper_works() {
        // 验证测试辅助函数 make_context 正确填充 PyDict
        with_python(|py| {
            let ctx = make_context(py, &[("a", 1), ("b", 2), ("c", 3)]);
            assert_eq!(ctx.fields().expect("fields").len(), 3);
            let name_a = PyString::new_bound(py, "a");
            assert_eq!(ctx.get_int_by_name(&name_a, py).expect("a"), 1);
            let name_c = PyString::new_bound(py, "c");
            assert_eq!(ctx.get_int_by_name(&name_c, py).expect("c"), 3);
        });
    }
}
