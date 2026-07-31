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
    /// 参数是字段在当前 StructNode 的 `expr_values_buf` 中的索引（编译期确定）。
    /// 执行：`ctx.get_int_at(idx, py)` → `stack.push(i64)`。
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

    /// 终止表达式快速求值（4.5 v5.1 性能优化）。
    ///
    /// 为 RepeatUntilNode 热路径提供常见终止表达式模式的内联求值，避免
    /// [`eval_expr_int`] 的函数调用 + stack_buf 分配 + match dispatch 开销
    /// （N=10 小规模场景每迭代 ~10ns）。
    ///
    /// # 覆盖模式
    ///
    /// - **3-op 单字段比较**：`[GetInt(idx), Const(k), <cmp>]`
    ///   覆盖场景：`e > 5` / `e < 0` / `e == 0xFF` 等
    /// - **3-op 双字段比较**：`[GetInt(idx_a), GetInt(idx_b), <cmp>]`
    ///   覆盖场景：`e > threshold` / `e == expected` 等
    ///
    /// 其他模式（`(e & 0xFF) == 0` 5-op / `-e > -5` 4-op）走 [`eval_expr_int`] 通用路径。
    ///
    /// # 未覆盖形式（走慢路径，合理的性能权衡）
    ///
    /// 仅匹配 `[GetInt, Const/GetInt, cmp]` 这一固定指令顺序。翻转比较形式如
    /// `5 < e` / `threshold > e`（编译为 `[Const, GetInt, cmp]`）不命中本方法，
    /// 会回退到 [`eval_expr_int`] 通用路径。这是有意的：用户写 `e > 5` 是绝大多数
    /// 场景，翻转形式罕见，为它额外加分支反而拖慢热路径。
    ///
    /// # 返回
    ///
    /// - `Some(Ok(v))`：命中快速路径，v 为求值结果（0 或 1）
    /// - `Some(Err(e))`：命中快速路径但 GetInt 失败（如 ExprFieldMissing）
    /// - `None`：未命中快速路径模式，调用方应走 [`eval_expr_int`]
    #[inline]
    pub fn try_eval_simple_cmp(
        &self,
        ctx: &Context<'_>,
        py: Python<'_>,
    ) -> Option<Result<i64, ConstructError>> {
        let ops: &[ExprOp] = self.ops.as_slice();
        if ops.len() != 3 {
            return None;
        }
        // 单字段常量比较
        if let (ExprOp::GetInt(idx), ExprOp::Const(k), cmp) = (ops[0], ops[1], ops[2]) {
            let v = match ctx.get_int_at(idx, py) {
                Ok(v) => v,
                Err(e) => return Some(Err(e)),
            };
            let result = match cmp {
                ExprOp::Gt => v > k,
                ExprOp::Ge => v >= k,
                ExprOp::Lt => v < k,
                ExprOp::Le => v <= k,
                ExprOp::Eq => v == k,
                ExprOp::Ne => v != k,
                _ => return None,
            };
            return Some(Ok(result as i64));
        }
        // 双字段比较
        if let (ExprOp::GetInt(idx_a), ExprOp::GetInt(idx_b), cmp) = (ops[0], ops[1], ops[2]) {
            let a = match ctx.get_int_at(idx_a, py) {
                Ok(v) => v,
                Err(e) => return Some(Err(e)),
            };
            let b = match ctx.get_int_at(idx_b, py) {
                Ok(v) => v,
                Err(e) => return Some(Err(e)),
            };
            let result = match cmp {
                ExprOp::Gt => a > b,
                ExprOp::Ge => a >= b,
                ExprOp::Lt => a < b,
                ExprOp::Le => a <= b,
                ExprOp::Eq => a == b,
                ExprOp::Ne => a != b,
                _ => return None,
            };
            return Some(Ok(result as i64));
        }
        None
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

/// 表达式 VM 栈槽位数（栈分配，避免每次求值的堆分配）。
///
/// 典型表达式的最大栈深度为 2-4（如 `a + b` 峰值 2，`(a+b)*(c+d)` 峰值 3）。
/// 32 个槽位覆盖任何合理的构造表达式。若 `max_stack` 超过此值，
/// [`eval_expr_int`] 返回错误（防御性，理论上编译期不会产生如此深的表达式）。
const VM_STACK_SLOTS: usize = 32;

/// 在 Rust 内部栈式求值表达式，零 FFI（[`ExprOp::GetInt`] 除外，需从 PyDict 取值）。
///
/// VM 栈使用栈分配的固定数组（[`VM_STACK_SLOTS`] 个 `i64` 槽位），避免每次求值
/// 的堆分配（`Vec::with_capacity`）。对于典型表达式（峰值栈深 2-4），这消除
/// 了 ~20-50ns/parse 的分配开销。
///
/// # 参数
///
/// - `program`：编译后的表达式程序（[`ExprProgram`]）
/// - `ctx`：当前上下文（持有 `PyDict` 和 `expr_values_buf`），通过
///   [`Context::get_int_at`] 按索引取字段值
/// - `py`：GIL token
///
/// # 返回
///
/// 求值结果（`i64`）。求值结束后栈应恰好剩一个值（编译期保证）。
///
/// # 错误
///
/// - [`ConstructError::ExprType`]：`GetInt` 取到的值无法转换为 `i64`
/// - [`ConstructError::ExprFieldMissing`]：`GetInt` 引用的槽位为 null（未设置）
/// - [`ConstructError::ExprContext`]：`ctx.expr_values_buf` 未初始化（placeholder），无法求值
/// - [`ConstructError::ExprDivByZero`]：`FloorDiv` / `Mod` 除数为 0
/// - [`ConstructError::ExprStackUnderflow`]：栈下溢（指令序列不合法，编译期保证不会发生）
/// - [`ConstructError::Generic`]：传入空程序，或 `max_stack` 超过 [`VM_STACK_SLOTS`]（编译期保证不发生）
pub fn eval_expr_int(
    program: &ExprProgram,
    ctx: &Context<'_>,
    py: Python<'_>,
) -> Result<i64, ConstructError> {
    if program.is_empty() {
        return Err(ConstructError::Generic {
            message: "eval_expr_int: empty expression program".to_string(),
            path: String::new(),
        });
    }

    // 防御性检查：max_stack 超过栈缓冲区大小则返回错误。
    // 理论上不会触发（构造表达式栈深极小），但避免 release 模式越界写入（UB）。
    if program.max_stack() > VM_STACK_SLOTS {
        return Err(ConstructError::Generic {
            message: format!(
                "expression max_stack {} exceeds VM_STACK_SLOTS {}",
                program.max_stack(),
                VM_STACK_SLOTS
            ),
            path: String::new(),
        });
    }

    // 栈分配的 VM 栈：32 个 i64 槽位，零堆分配。
    let mut stack_buf = [0i64; VM_STACK_SLOTS];
    let mut stack_len = 0usize;

    for op in program.ops() {
        match op {
            ExprOp::GetInt(idx) => {
                let val = ctx.get_int_at(*idx, py)?;
                stack_buf[stack_len] = val;
                stack_len += 1;
            }
            ExprOp::Const(v) => {
                stack_buf[stack_len] = *v;
                stack_len += 1;
            }
            // 二元算术
            // 注意：i64 wrapping 语义——溢出时静默回绕（如 i64::MAX + 1 = i64::MIN）。
            // Python int 是任意精度，永不溢出。表达式 VM 用于字段长度计算（典型值 < 2^32），
            // 实际场景不触及 i64 边界。如需精确对齐 Python，需引入 num_bigint（堆分配，
            // 与性能目标冲突）。
            ExprOp::Add => binop_fixed(&mut stack_buf, &mut stack_len, i64::wrapping_add)?,
            ExprOp::Sub => binop_fixed(&mut stack_buf, &mut stack_len, i64::wrapping_sub)?,
            ExprOp::Mul => binop_fixed(&mut stack_buf, &mut stack_len, i64::wrapping_mul)?,
            ExprOp::FloorDiv => {
                let (a, b) = pop2_fixed(&stack_buf, &mut stack_len)?;
                if b == 0 {
                    return Err(ConstructError::ExprDivByZero {
                        message: "expression division by zero".to_string(),
                        path: String::new(),
                    });
                }
                // Python `//` 语义：向负无穷取整（floor division）。
                // div_euclid 不匹配（欧几里得余数恒非负，Python 余数符号同除数）。
                // 用 wrapping_* 实现真 floor division，永不 panic。
                push_fixed(&mut stack_buf, &mut stack_len, floor_div(a, b));
            }
            ExprOp::Mod => {
                let (a, b) = pop2_fixed(&stack_buf, &mut stack_len)?;
                if b == 0 {
                    return Err(ConstructError::ExprDivByZero {
                        message: "expression modulo by zero".to_string(),
                        path: String::new(),
                    });
                }
                // Python `%` 语义：结果符号与除数一致（floor modulo）。
                push_fixed(&mut stack_buf, &mut stack_len, floor_rem(a, b));
            }
            // 位运算
            ExprOp::BitAnd => binop_fixed(&mut stack_buf, &mut stack_len, |a, b| a & b)?,
            ExprOp::BitOr => binop_fixed(&mut stack_buf, &mut stack_len, |a, b| a | b)?,
            ExprOp::BitXor => binop_fixed(&mut stack_buf, &mut stack_len, |a, b| a ^ b)?,
            // SF-1 修复：Python `a << b` 当 b < 0 时抛 ValueError，此处对齐行为。
            ExprOp::Shl => {
                let (a, b) = pop2_fixed(&stack_buf, &mut stack_len)?;
                if b < 0 {
                    return Err(ConstructError::Generic {
                        message: format!("negative shift count: {}", b),
                        path: String::new(),
                    });
                }
                push_fixed(&mut stack_buf, &mut stack_len, a.wrapping_shl(b as u32));
            }
            ExprOp::Shr => {
                let (a, b) = pop2_fixed(&stack_buf, &mut stack_len)?;
                if b < 0 {
                    return Err(ConstructError::Generic {
                        message: format!("negative shift count: {}", b),
                        path: String::new(),
                    });
                }
                push_fixed(&mut stack_buf, &mut stack_len, a.wrapping_shr(b as u32));
            }
            // 一元
            ExprOp::Neg => {
                if stack_len == 0 {
                    return Err(ConstructError::ExprStackUnderflow {
                        path: String::new(),
                    });
                }
                let i = stack_len - 1;
                stack_buf[i] = stack_buf[i].wrapping_neg();
            }
            ExprOp::Not => {
                if stack_len == 0 {
                    return Err(ConstructError::ExprStackUnderflow {
                        path: String::new(),
                    });
                }
                let i = stack_len - 1;
                stack_buf[i] = !stack_buf[i];
            }
            // 比较
            ExprOp::Eq => cmp_fixed(&mut stack_buf, &mut stack_len, |a, b| a == b)?,
            ExprOp::Ne => cmp_fixed(&mut stack_buf, &mut stack_len, |a, b| a != b)?,
            ExprOp::Lt => cmp_fixed(&mut stack_buf, &mut stack_len, |a, b| a < b)?,
            ExprOp::Le => cmp_fixed(&mut stack_buf, &mut stack_len, |a, b| a <= b)?,
            ExprOp::Gt => cmp_fixed(&mut stack_buf, &mut stack_len, |a, b| a > b)?,
            ExprOp::Ge => cmp_fixed(&mut stack_buf, &mut stack_len, |a, b| a >= b)?,
        }
    }

    if stack_len == 0 {
        return Err(ConstructError::ExprStackUnderflow {
            path: String::new(),
        });
    }
    stack_len -= 1;
    Ok(stack_buf[stack_len])
}

/// 求值表达式为布尔值（Phase 8.1 Check 节点用）。
///
/// 与 [`eval_expr_int`] 同栈式求值，仅将最终 i64 转为 bool
/// （非零即真，对齐 Python `if not passed` 语义）。
///
/// # 错误
///
/// 同 [`eval_expr_int`]（向上传播 ExprContext / ExprFieldMissing / ExprDivByZero 等）。
pub fn eval_expr_bool(
    program: &ExprProgram,
    ctx: &Context<'_>,
    py: Python<'_>,
) -> Result<bool, ConstructError> {
    let v = eval_expr_int(program, ctx, py)?;
    Ok(v != 0)
}

/// 求值表达式为任意 Python 对象（Phase 8.12 ProcessXor XorPad::Expr 路径用）。
///
/// 与 [`eval_expr_int`] 同栈式求值，但最终值通过 `PyObject_*` API 取出原始 PyObject
/// 而非 i64。用于需要返回 bytes / str / 任意 Python 对象的场景（如 ProcessXor 的
/// padfunc 表达式求值可能返回 int 或 bytes）。
///
/// # 实现说明
///
/// 内部仍走 i64 栈式 VM，但 `GetInt` 时直接从 ctx 取 PyObject 引用（同时维持
/// i64 缓存以支持后续算术）。最终 `pop` 时返回 PyObject（若栈顶是 GetInt 取出的
/// 原始字段，则返回该字段 PyObject；若栈顶是 Const 计算结果，则返回 i64 PyLong）。
///
/// # 限制
///
/// - 不支持纯 Const 程序返回 bytes/str（Const 是 i64，仅能返回 PyLong）
/// - 仅支持"单字段引用"或"i64 算术"两种模式（与 Switch FieldRef 同脉络）
///
/// # 错误
///
/// 同 [`eval_expr_int`]。
pub fn eval_expr_any(
    program: &ExprProgram,
    ctx: &Context<'_>,
    py: Python<'_>,
) -> Result<Py<PyAny>, ConstructError> {
    if program.is_empty() {
        return Err(ConstructError::Generic {
            message: "eval_expr_any: empty expression program".to_string(),
            path: String::new(),
        });
    }

    if program.max_stack() > VM_STACK_SLOTS {
        return Err(ConstructError::Generic {
            message: format!(
                "expression max_stack {} exceeds VM_STACK_SLOTS {}",
                program.max_stack(),
                VM_STACK_SLOTS
            ),
            path: String::new(),
        });
    }

    // 简化策略：先求 i64 结果，若程序是"单 GetInt"模式则返回原始 PyObject
    // （覆盖 ProcessXor padfunc = this.pad_field 的常见用例）。
    // 其他模式 fallback 到 i64 → PyLong。
    let ops = program.ops();
    if ops.len() == 1 {
        if let ExprOp::GetInt(idx) = ops[0] {
            // 单字段引用：返回原始 PyObject（可能是 bytes / str / int / 任意类型）。
            let val = ctx.get_field_at(idx, py)?;
            return Ok(val.unbind());
        }
    }

    // 通用 fallback：i64 → PyLong（覆盖 Const + 算术）。
    let v = eval_expr_int(program, ctx, py)?;
    Ok(v.into_py(py))
}

/// 压入一个值到固定大小 VM 栈顶。
///
/// 调用方需保证 `len < VM_STACK_SLOTS`（由 [`eval_expr_int`] 入口的
/// `max_stack` 检查间接保证）。
#[inline]
fn push_fixed(stack: &mut [i64; VM_STACK_SLOTS], len: &mut usize, val: i64) {
    stack[*len] = val;
    *len += 1;
}

/// 从固定大小 VM 栈弹出栈顶两个元素 `(a, b)`。
///
/// 弹出顺序：栈顶是 rhs（`b`），其下是 lhs（`a`）。
/// `len < 2` 时返回 [`ConstructError::ExprStackUnderflow`]（原子检查，不部分消费）。
#[inline]
fn pop2_fixed(
    stack: &[i64; VM_STACK_SLOTS],
    len: &mut usize,
) -> Result<(i64, i64), ConstructError> {
    if *len < 2 {
        return Err(ConstructError::ExprStackUnderflow {
            path: String::new(),
        });
    }
    *len -= 2;
    let a = stack[*len];
    let b = stack[*len + 1];
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

/// 二元运算辅助（固定数组版）：弹出 `(a, b)`，压入 `f(a, b)`。
///
/// 泛型 `F` 接收 `(i64, i64)` 返回 `i64`。栈下溢由 [`pop2_fixed`] 处理。
#[inline]
fn binop_fixed<F>(
    stack: &mut [i64; VM_STACK_SLOTS],
    len: &mut usize,
    f: F,
) -> Result<(), ConstructError>
where
    F: FnOnce(i64, i64) -> i64,
{
    let (a, b) = pop2_fixed(stack, len)?;
    stack[*len] = f(a, b);
    *len += 1;
    Ok(())
}

/// 比较运算辅助（固定数组版）：弹出 `(a, b)`，压入 `f(a, b) as i64`（0 或 1）。
///
/// 泛型 `F` 接收 `(i64, i64)` 返回 `bool`，结果被 cast 为 `i64`（true→1, false→0）。
#[inline]
fn cmp_fixed<F>(
    stack: &mut [i64; VM_STACK_SLOTS],
    len: &mut usize,
    f: F,
) -> Result<(), ConstructError>
where
    F: FnOnce(i64, i64) -> bool,
{
    let (a, b) = pop2_fixed(stack, len)?;
    stack[*len] = if f(a, b) { 1 } else { 0 };
    *len += 1;
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
    /// 使用新 Vec 化 API：`init_expr_values` + `set_field_at`。
    /// 返回的 `Context` 为 `new_root`，持有填充好的 `PyDict` 和 `expr_values`。
    fn make_context<'py>(py: Python<'py>, entries: &[(&str, i64)]) -> Context<'py> {
        let mut ctx = Context::new_root(py).expect("new_root");
        ctx.init_expr_values(entries.len());
        for (idx, (name, value)) in entries.iter().enumerate() {
            let key = PyString::new_bound(py, name).unbind();
            let val = (*value).into_py(py);
            ctx.set_field_at(idx, &key, val.bind(py), py)
                .expect("set_field_at");
        }
        ctx
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
            let prog = ExprProgram::new(vec![ExprOp::Const(42)]);
            let result = eval_expr_int(&prog, &ctx, py).expect("const");
            assert_eq!(result, 42);
        });
    }

    #[test]
    fn eval_const_negative() {
        with_python(|py| {
            let ctx = Context::placeholder(py);
            let prog = ExprProgram::new(vec![ExprOp::Const(-100)]);
            let result = eval_expr_int(&prog, &ctx, py).expect("neg const");
            assert_eq!(result, -100);
        });
    }

    #[test]
    fn eval_getint_returns_field_value() {
        with_python(|py| {
            let ctx = make_context(py, &[("count", 7)]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0)]);
            let result = eval_expr_int(&prog, &ctx, py).expect("getint");
            assert_eq!(result, 7);
        });
    }

    #[test]
    fn eval_getint_multiple_fields() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 10), ("b", 20), ("c", 30)]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(2)]);
            let result = eval_expr_int(&prog, &ctx, py).expect("getint idx 2");
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
            // count + flag
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::GetInt(1), ExprOp::Add]);
            let result = eval_expr_int(&prog, &ctx, py).expect("add");
            assert_eq!(result, 8);
        });
    }

    #[test]
    fn eval_count_times_two() {
        with_python(|py| {
            let ctx = make_context(py, &[("count", 6)]);
            // count * 2
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(2), ExprOp::Mul]);
            let result = eval_expr_int(&prog, &ctx, py).expect("mul");
            assert_eq!(result, 12);
        });
    }

    #[test]
    fn eval_chained_addition_count_plus_flag_plus_one() {
        with_python(|py| {
            let ctx = make_context(py, &[("count", 3), ("flag", 5)]);
            // count + flag + 1 → [G(0), G(1), Add, Const(1), Add]
            let prog = ExprProgram::new(vec![
                ExprOp::GetInt(0),
                ExprOp::GetInt(1),
                ExprOp::Add,
                ExprOp::Const(1),
                ExprOp::Add,
            ]);
            let result = eval_expr_int(&prog, &ctx, py).expect("chained add");
            assert_eq!(result, 9);
        });
    }

    #[test]
    fn eval_sub() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 10), ("b", 3)]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::GetInt(1), ExprOp::Sub]);
            let result = eval_expr_int(&prog, &ctx, py).expect("sub");
            assert_eq!(result, 7);
        });
    }

    #[test]
    fn eval_floor_div() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 17), ("b", 5)]);
            let prog =
                ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::GetInt(1), ExprOp::FloorDiv]);
            let result = eval_expr_int(&prog, &ctx, py).expect("floordiv");
            assert_eq!(result, 3);
        });
    }

    #[test]
    fn eval_mod() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 17), ("b", 5)]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::GetInt(1), ExprOp::Mod]);
            let result = eval_expr_int(&prog, &ctx, py).expect("mod");
            assert_eq!(result, 2);
        });
    }

    #[test]
    fn eval_floor_div_negative_dividend() {
        // Python: -7 // 2 = -4（向负无穷取整，不是 truncating 的 -3）
        with_python(|py| {
            let ctx = make_context(py, &[("a", -7), ("b", 2)]);
            let prog =
                ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::GetInt(1), ExprOp::FloorDiv]);
            let result = eval_expr_int(&prog, &ctx, py).expect("floordiv neg dividend");
            assert_eq!(result, -4);
        });
    }

    #[test]
    fn eval_mod_negative_dividend() {
        // Python: -7 % 2 = 1（结果符号与除数一致）
        with_python(|py| {
            let ctx = make_context(py, &[("a", -7), ("b", 2)]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::GetInt(1), ExprOp::Mod]);
            let result = eval_expr_int(&prog, &ctx, py).expect("mod neg dividend");
            assert_eq!(result, 1);
        });
    }

    #[test]
    fn eval_floor_div_negative_divisor() {
        // Python: 7 // -2 = -4（向负无穷取整，不是 truncating 的 -3）
        with_python(|py| {
            let ctx = make_context(py, &[("a", 7), ("b", -2)]);
            let prog =
                ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::GetInt(1), ExprOp::FloorDiv]);
            let result = eval_expr_int(&prog, &ctx, py).expect("floordiv neg divisor");
            assert_eq!(result, -4);
        });
    }

    #[test]
    fn eval_floor_div_both_negative() {
        // Python: -7 // -1 = 7（负负得正，无取整差异）
        with_python(|py| {
            let ctx = make_context(py, &[("a", -7), ("b", -1)]);
            let prog =
                ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::GetInt(1), ExprOp::FloorDiv]);
            let result = eval_expr_int(&prog, &ctx, py).expect("floordiv both neg");
            assert_eq!(result, 7);
        });
    }

    #[test]
    fn eval_mod_negative_divisor() {
        // Python: 7 % -2 = -1（结果符号与除数一致）
        with_python(|py| {
            let ctx = make_context(py, &[("a", 7), ("b", -2)]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::GetInt(1), ExprOp::Mod]);
            let result = eval_expr_int(&prog, &ctx, py).expect("mod neg divisor");
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
            // i64::MIN / -1
            let prog_div = ExprProgram::new(vec![
                ExprOp::Const(i64::MIN),
                ExprOp::Const(-1),
                ExprOp::FloorDiv,
            ]);
            let result_div = eval_expr_int(&prog_div, &ctx, py).expect("min div -1");
            assert_eq!(result_div, i64::MIN); // wrapping: 9223372036854775808 mod 2^64

            // i64::MIN % -1
            let prog_mod = ExprProgram::new(vec![
                ExprOp::Const(i64::MIN),
                ExprOp::Const(-1),
                ExprOp::Mod,
            ]);
            let result_mod = eval_expr_int(&prog_mod, &ctx, py).expect("min mod -1");
            assert_eq!(result_mod, 0);
        });
    }

    #[test]
    fn eval_nested_arithmetic() {
        // (a + b) * c → [G(0), G(1), Add, G(2), Mul]
        with_python(|py| {
            let ctx = make_context(py, &[("a", 2), ("b", 3), ("c", 4)]);
            let prog = ExprProgram::new(vec![
                ExprOp::GetInt(0),
                ExprOp::GetInt(1),
                ExprOp::Add,
                ExprOp::GetInt(2),
                ExprOp::Mul,
            ]);
            let result = eval_expr_int(&prog, &ctx, py).expect("nested");
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
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Neg]);
            let result = eval_expr_int(&prog, &ctx, py).expect("neg");
            assert_eq!(result, -42);
        });
    }

    #[test]
    fn eval_neg_negative_to_positive() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", -42)]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Neg]);
            let result = eval_expr_int(&prog, &ctx, py).expect("neg");
            assert_eq!(result, 42);
        });
    }

    #[test]
    fn eval_not_bitwise() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 0)]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Not]);
            let result = eval_expr_int(&prog, &ctx, py).expect("not 0");
            assert_eq!(result, -1); // !0 = -1 in i64
        });
    }

    #[test]
    fn eval_not_all_ones() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", -1)]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Not]);
            let result = eval_expr_int(&prog, &ctx, py).expect("not -1");
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
            // a > 0
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(0), ExprOp::Gt]);
            let result = eval_expr_int(&prog, &ctx, py).expect("gt");
            assert_eq!(result, 1);
        });
    }

    #[test]
    fn eval_gt_false() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 0)]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(1), ExprOp::Gt]);
            let result = eval_expr_int(&prog, &ctx, py).expect("gt false");
            assert_eq!(result, 0);
        });
    }

    #[test]
    fn eval_eq_true() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 5)]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(5), ExprOp::Eq]);
            let result = eval_expr_int(&prog, &ctx, py).expect("eq");
            assert_eq!(result, 1);
        });
    }

    #[test]
    fn eval_ne_true() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 5)]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(6), ExprOp::Ne]);
            let result = eval_expr_int(&prog, &ctx, py).expect("ne");
            assert_eq!(result, 1);
        });
    }

    #[test]
    fn eval_lt_le_ge() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 3)]);

            // a < 5 → 1
            let prog_lt = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(5), ExprOp::Lt]);
            assert_eq!(eval_expr_int(&prog_lt, &ctx, py).expect("lt"), 1);

            // a <= 3 → 1
            let prog_le = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(3), ExprOp::Le]);
            assert_eq!(eval_expr_int(&prog_le, &ctx, py).expect("le"), 1);

            // a >= 5 → 0
            let prog_ge = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(5), ExprOp::Ge]);
            assert_eq!(eval_expr_int(&prog_ge, &ctx, py).expect("ge"), 0);
        });
    }

    // ======================================================================
    // eval_expr_int：位运算
    // ======================================================================

    #[test]
    fn eval_bitand() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 0xAB)]);
            // a & 0xFF → 0xAB
            let prog =
                ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(0xFF), ExprOp::BitAnd]);
            let result = eval_expr_int(&prog, &ctx, py).expect("bitand");
            assert_eq!(result, 0xAB);
        });
    }

    #[test]
    fn eval_bitor() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 0xF0)]);
            // a | 0x0F → 0xFF
            let prog =
                ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(0x0F), ExprOp::BitOr]);
            let result = eval_expr_int(&prog, &ctx, py).expect("bitor");
            assert_eq!(result, 0xFF);
        });
    }

    #[test]
    fn eval_bitxor() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 0xFF)]);
            // a ^ 0x0F → 0xF0
            let prog =
                ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(0x0F), ExprOp::BitXor]);
            let result = eval_expr_int(&prog, &ctx, py).expect("bitxor");
            assert_eq!(result, 0xF0);
        });
    }

    #[test]
    fn eval_shl() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 1)]);
            // a << 4 → 16
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(4), ExprOp::Shl]);
            let result = eval_expr_int(&prog, &ctx, py).expect("shl");
            assert_eq!(result, 16);
        });
    }

    #[test]
    fn eval_shr() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 256)]);
            // a >> 4 → 16
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(4), ExprOp::Shr]);
            let result = eval_expr_int(&prog, &ctx, py).expect("shr");
            assert_eq!(result, 16);
        });
    }

    #[test]
    fn eval_shl_negative_shift_count_returns_error() {
        // SF-1 修复：Python `a << b` 当 b < 0 时抛 ValueError，此处对齐行为。
        with_python(|py| {
            let ctx = Context::placeholder(py);
            // 1 << -1 → error
            let prog = ExprProgram::new(vec![ExprOp::Const(1), ExprOp::Const(-1), ExprOp::Shl]);
            let result = eval_expr_int(&prog, &ctx, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::Generic { message, .. }) => {
                    assert!(message.contains("negative shift count"), "got: {}", message);
                }
                other => panic!("expected Generic error, got {:?}", other),
            }
        });
    }

    #[test]
    fn eval_shr_negative_shift_count_returns_error() {
        // SF-1 修复：Python `a >> b` 当 b < 0 时抛 ValueError，此处对齐行为。
        with_python(|py| {
            let ctx = Context::placeholder(py);
            // 256 >> -1 → error
            let prog = ExprProgram::new(vec![ExprOp::Const(256), ExprOp::Const(-1), ExprOp::Shr]);
            let result = eval_expr_int(&prog, &ctx, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::Generic { message, .. }) => {
                    assert!(message.contains("negative shift count"), "got: {}", message);
                }
                other => panic!("expected Generic error, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // eval_expr_int：错误路径
    // ======================================================================

    #[test]
    fn eval_empty_program_returns_error() {
        with_python(|py| {
            let ctx = Context::placeholder(py);
            let prog = ExprProgram::empty();
            let result = eval_expr_int(&prog, &ctx, py);
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
            // 10 / 0
            let prog =
                ExprProgram::new(vec![ExprOp::Const(10), ExprOp::Const(0), ExprOp::FloorDiv]);
            let result = eval_expr_int(&prog, &ctx, py);
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
            // 10 % 0
            let prog = ExprProgram::new(vec![ExprOp::Const(10), ExprOp::Const(0), ExprOp::Mod]);
            let result = eval_expr_int(&prog, &ctx, py);
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
            // [Add] with empty stack → stack underflow
            let prog = ExprProgram::new(vec![ExprOp::Add]);
            let result = eval_expr_int(&prog, &ctx, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprStackUnderflow { .. }) => {}
                other => panic!("expected ExprStackUnderflow, got {:?}", other),
            }
        });
    }

    #[test]
    fn eval_stack_underflow_on_unary_op_with_empty_stack() {
        with_python(|py| {
            let ctx = Context::placeholder(py);
            // [Neg] with empty stack → stack underflow
            let prog = ExprProgram::new(vec![ExprOp::Neg]);
            let result = eval_expr_int(&prog, &ctx, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprStackUnderflow { .. }) => {}
                other => panic!("expected ExprStackUnderflow, got {:?}", other),
            }
        });
    }

    #[test]
    fn eval_getint_on_placeholder_context_returns_expr_context_error() {
        with_python(|py| {
            let ctx = Context::placeholder(py);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0)]);
            let result = eval_expr_int(&prog, &ctx, py);
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
            // init_expr_values(2)，但只 set_field_at(0, "a")。idx 1 为 null 槽位。
            let mut ctx = Context::new_root(py).expect("new_root");
            ctx.init_expr_values(2);
            let key = PyString::new_bound(py, "a").unbind();
            let val = 1i64.into_py(py);
            ctx.set_field_at(0, &key, val.bind(py), py).expect("set a");
            // idx 1 保持 null

            let prog = ExprProgram::new(vec![ExprOp::GetInt(1)]);
            let result = eval_expr_int(&prog, &ctx, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprFieldMissing { field, .. }) => {
                    assert!(
                        field.contains("1"),
                        "field should contain index 1: {}",
                        field
                    );
                }
                other => panic!("expected ExprFieldMissing, got {:?}", other),
            }
        });
    }

    #[test]
    fn eval_getint_wrong_type_returns_expr_type_error() {
        with_python(|py| {
            let mut ctx = Context::new_root(py).expect("root");
            ctx.init_expr_values(1);
            // 存入 str 值而非 int
            let str_val = PyString::new_bound(py, "hello").into_any();
            let key = PyString::new_bound(py, "name").unbind();
            ctx.set_field_at(0, &key, &str_val, py).expect("set str");
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0)]);
            let result = eval_expr_int(&prog, &ctx, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprType {
                    field, expected, ..
                }) => {
                    assert!(
                        field.contains("0"),
                        "field should contain index 0: {}",
                        field
                    );
                    assert!(expected.contains("integer"));
                }
                other => panic!("expected ExprType, got {:?}", other),
            }
        });
    }

    #[test]
    fn eval_getint_index_out_of_bounds_returns_field_missing_error() {
        // expr_values 有 1 个槽位，GetInt(5) 越界 → ExprFieldMissing（null 槽位）
        with_python(|py| {
            let ctx = make_context(py, &[("count", 1)]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(5)]);
            let result = eval_expr_int(&prog, &ctx, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprFieldMissing { field, .. }) => {
                    assert!(field.contains("5"), "field should contain idx 5: {}", field);
                }
                other => panic!("expected ExprFieldMissing, got {:?}", other),
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
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0)]);
            let result = eval_expr_int(&prog, &ctx, py).expect("bytes len");
            assert_eq!(result, 42);
        });
    }

    #[test]
    fn eval_bytes_count_plus_flag_expression() {
        // 模拟 Bytes(count + flag)
        with_python(|py| {
            let ctx = make_context(py, &[("count", 10), ("flag", 2)]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::GetInt(1), ExprOp::Add]);
            let result = eval_expr_int(&prog, &ctx, py).expect("bytes len");
            assert_eq!(result, 12);
        });
    }

    #[test]
    fn eval_computed_end_minus_start() {
        // 模拟 Computed(end - start)
        with_python(|py| {
            let ctx = make_context(py, &[("start", 0), ("end", 4)]);
            let prog = ExprProgram::new(vec![
                ExprOp::GetInt(1), // end
                ExprOp::GetInt(0), // start
                ExprOp::Sub,
            ]);
            let result = eval_expr_int(&prog, &ctx, py).expect("computed");
            assert_eq!(result, 4);
        });
    }

    #[test]
    fn eval_switch_cond_expression() {
        // 模拟 Switch 条件: type_flag == 1
        with_python(|py| {
            let ctx = make_context(py, &[("type_flag", 1)]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(1), ExprOp::Eq]);
            let result = eval_expr_int(&prog, &ctx, py).expect("switch cond");
            assert_eq!(result, 1);
        });
    }

    // ======================================================================
    // Context::get_int_at（Vec 化 API 集成测试）
    // ======================================================================

    #[test]
    fn get_int_at_returns_value() {
        with_python(|py| {
            let ctx = make_context(py, &[("count", 42)]);
            assert_eq!(ctx.get_int_at(0, py).expect("get int"), 42);
        });
    }

    #[test]
    fn get_int_at_multiple_fields() {
        with_python(|py| {
            let ctx = make_context(py, &[("a", 10), ("b", 20), ("c", 30)]);
            assert_eq!(ctx.get_int_at(0, py).expect("a"), 10);
            assert_eq!(ctx.get_int_at(1, py).expect("b"), 20);
            assert_eq!(ctx.get_int_at(2, py).expect("c"), 30);
        });
    }

    #[test]
    fn get_int_at_null_slot_returns_field_missing_error() {
        // init_expr_values(2) 但只设置 idx 0 → idx 1 为 null
        with_python(|py| {
            let mut ctx = Context::new_root(py).expect("new_root");
            ctx.init_expr_values(2);
            let key = PyString::new_bound(py, "a").unbind();
            let val = 1i64.into_py(py);
            ctx.set_field_at(0, &key, val.bind(py), py).expect("set a");

            let result = ctx.get_int_at(1, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprFieldMissing { field, .. }) => {
                    assert!(field.contains("1"), "field: {}", field);
                }
                other => panic!("expected ExprFieldMissing, got {:?}", other),
            }
        });
    }

    #[test]
    fn get_int_at_on_placeholder_returns_expr_context_error() {
        with_python(|py| {
            let ctx = Context::placeholder(py);
            let result = ctx.get_int_at(0, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprContext { message, .. }) => {
                    assert!(message.contains("not initialized"), "message: {}", message);
                }
                other => panic!("expected ExprContext, got {:?}", other),
            }
        });
    }

    #[test]
    fn get_int_at_wrong_type_returns_expr_type_error() {
        with_python(|py| {
            let mut ctx = Context::new_root(py).expect("root");
            ctx.init_expr_values(1);
            let str_val = PyString::new_bound(py, "hello").into_any();
            let key = PyString::new_bound(py, "name").unbind();
            ctx.set_field_at(0, &key, &str_val, py).expect("set str");
            let result = ctx.get_int_at(0, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprType { expected, .. }) => {
                    assert!(expected.contains("integer"));
                }
                other => panic!("expected ExprType, got {:?}", other),
            }
        });
    }

    #[test]
    fn get_int_at_negative_value() {
        with_python(|py| {
            let ctx = make_context(py, &[("offset", -100)]);
            assert_eq!(ctx.get_int_at(0, py).expect("get negative"), -100);
        });
    }

    #[test]
    fn get_int_at_i64_max() {
        with_python(|py| {
            let ctx = make_context(py, &[("max", i64::MAX)]);
            assert_eq!(ctx.get_int_at(0, py).expect("get max"), i64::MAX);
        });
    }

    #[test]
    fn get_int_at_minus_one_distinguished_from_error() {
        // PyLong_AsLongLong(-1) 返回 -1 但 PyErr_Occurred 为 null → 正常返回 -1
        with_python(|py| {
            let ctx = make_context(py, &[("val", -1)]);
            assert_eq!(ctx.get_int_at(0, py).expect("get -1"), -1);
        });
    }

    #[test]
    fn get_int_at_without_init_returns_expr_context_error() {
        // new_root 未调用 init_expr_values → expr_values = None
        with_python(|py| {
            let ctx = Context::new_root(py).expect("root");
            let result = ctx.get_int_at(0, py);
            assert!(result.is_err());
            match result {
                Err(ConstructError::ExprContext { message, .. }) => {
                    assert!(message.contains("not initialized"));
                }
                other => panic!("expected ExprContext, got {:?}", other),
            }
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
    // 测试辅助函数验证
    // ======================================================================

    #[test]
    fn make_context_helper_works() {
        // 验证测试辅助函数 make_context 正确填充 PyDict 和 expr_values
        with_python(|py| {
            let ctx = make_context(py, &[("a", 1), ("b", 2), ("c", 3)]);
            assert_eq!(ctx.fields().expect("fields").len(), 3);
            assert_eq!(ctx.get_int_at(0, py).expect("a"), 1);
            assert_eq!(ctx.get_int_at(1, py).expect("b"), 2);
            assert_eq!(ctx.get_int_at(2, py).expect("c"), 3);
        });
    }
}
