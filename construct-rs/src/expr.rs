//! Expression system for referencing context fields and computing derived values.
//!
//! This module provides a declarative expression system that mirrors the Python
//! `construct.expr` module. Expressions can reference context fields via
//! [`Path`] (corresponding to Python `this` / `obj_`), perform arithmetic and
//! logical operations via [`BinExpr`] / [`UniExpr`], invoke built-in functions
//! via [`FuncPath`], and select values conditionally via [`CondExpr`].
//!
//! # Core concepts
//!
//! - [`Evaluate`] — the trait that all expression nodes implement; evaluates
//!   against a [`Context`](crate::core::context::Context) and an optional build
//!   object
//! - [`Path`] — a dotted path that resolves to a [`Value`](crate::value::Value)
//!   inside a context or build object
//! - [`BinExpr`] / [`UniExpr`] — binary and unary expression nodes
//! - [`FuncPath`] — built-in function application (len_, sum_, min_, max_, abs_)
//! - [`CondExpr`] — conditional (ternary) expression
//! - [`ConstExpr`] — a literal value wrapped as an expression
//! - [`IntoExpr`] — convenience trait for converting values/paths into boxed
//!   expression nodes
//!
//! # Convenience constructors
//!
//! ```
//! # use construct::expr::{this_, obj_};
//! // this_() creates a Path rooted at the current context
//! let path = this_().field("header").field("length");
//!
//! // obj_() creates a Path rooted at the build object
//! let obj_path = obj_().field("count");
//! ```
//!
//! # Python correspondence
//!
//! | Python | Rust |
//! |--------|------|
//! | `this.field` | `this_().field("field")` |
//! | `obj_.field` | `obj_().field("field")` |
//! | `this.a + 1` | `this_().field("a").add(ConstExpr::new(Value::Int(1)))` |
//! | `len_(this.items)` | `FuncPath::new("len", this_().field("items"))` |
//! | `this.x if this.x > 0 else 0` | `CondExpr::new(cond, then, else)` |

use crate::core::context::Context;
use crate::core::error::{ConstructError, Result};
use crate::value::Value;

// ===========================================================================
// Evaluate trait
// ===========================================================================

/// The trait implemented by all expression nodes.
///
/// An expression is evaluated against a [`Context`] and an optional `obj`
/// reference. During `parse`, `obj` is `None`; during `build`, `obj` is
/// `Some(&Value)` pointing to the build object.
///
/// Corresponds to the Python `__call__(obj)` protocol on `ExprMixin` subclasses.
pub trait Evaluate: Send + Sync + std::fmt::Debug {
    /// Evaluates this expression and returns the resulting [`Value`].
    ///
    /// # Errors
    ///
    /// Returns [`ConstructError::Expr`] if path resolution fails, a type
    /// mismatch occurs during an operation, or any other evaluation error.
    fn evaluate(&self, ctx: &Context, obj: Option<&Value>) -> Result<Value>;
}

// ===========================================================================
// PathRoot
// ===========================================================================

/// The root of a [`Path`] expression — determines where resolution starts.
///
/// Corresponds to the distinction between Python `this` (current context) and
/// `obj_` (build object).
#[derive(Clone, Debug, PartialEq)]
pub enum PathRoot {
    /// Resolve starting from the current context (`this` in Python).
    This,
    /// Resolve starting from the build object (`obj_` in Python).
    Obj,
}

// ===========================================================================
// Path
// ===========================================================================

/// A dotted path that resolves to a [`Value`] inside a context or build object.
///
/// A `Path` has a [`PathRoot`] (either [`This`](PathRoot::This) for context or
/// [`Obj`](PathRoot::Obj) for the build object) and zero or more segments that
/// navigate into nested [`Value::Container`] fields.
///
/// # Python correspondence
///
/// | Python | Rust |
/// |--------|------|
/// | `this` | `Path { root: PathRoot::This, segments: vec![] }` |
/// | `this.header` | `Path { root: PathRoot::This, segments: vec!["header"] }` |
/// | `this.header.length` | `Path { root: PathRoot::This, segments: vec!["header", "length"] }` |
/// | `obj_` | `Path { root: PathRoot::Obj, segments: vec![] }` |
/// | `obj_.count` | `Path { root: PathRoot::Obj, segments: vec!["count"] }` |
#[derive(Clone, Debug, PartialEq)]
pub struct Path {
    /// The segments to traverse after the root.
    pub segments: Vec<String>,
    /// The root of the path — context or build object.
    pub root: PathRoot,
}

impl Path {
    /// Creates a new `Path` with the given root and no segments.
    ///
    /// # Example
    ///
    /// ```
    /// # use construct::expr::{Path, PathRoot};
    /// let path = Path::new(PathRoot::This);
    /// assert!(path.segments.is_empty());
    /// ```
    #[must_use]
    pub fn new(root: PathRoot) -> Self {
        Path {
            segments: Vec::new(),
            root,
        }
    }

    /// Appends a segment and returns a new `Path`.
    ///
    /// This is the Rust equivalent of Python's `this.field` attribute access.
    ///
    /// # Example
    ///
    /// ```
    /// # use construct::expr::{Path, PathRoot};
    /// let path = Path::new(PathRoot::This).field("header").field("length");
    /// assert_eq!(path.segments, vec!["header", "length"]);
    /// ```
    #[must_use]
    pub fn field(&self, name: impl Into<String>) -> Self {
        let mut new_path = self.clone();
        new_path.segments.push(name.into());
        new_path
    }
}

impl Evaluate for Path {
    fn evaluate(&self, ctx: &Context, obj: Option<&Value>) -> Result<Value> {
        match self.root {
            PathRoot::This => {
                if self.segments.is_empty() {
                    // `this` with no segments: return the full context as a Container.
                    // In Python, `this()` returns `obj`, which during parse is the
                    // context being built up. We return None as a sentinel — callers
                    // that need context-as-value should use explicit field paths.
                    Err(ConstructError::Expr {
                        path: String::new(),
                        message: "bare 'this' without field segments cannot be evaluated"
                            .to_string(),
                    })
                } else {
                    let path_refs: Vec<String> = self.segments.clone();
                    ctx.get_path(&path_refs).cloned()
                }
            }
            PathRoot::Obj => {
                let value = obj.ok_or_else(|| ConstructError::Expr {
                    path: String::new(),
                    message: "obj_ path used outside of build context (obj is None)".to_string(),
                })?;
                if self.segments.is_empty() {
                    Ok(value.clone())
                } else {
                    let dotted = self.segments.join(".");
                    value.get_path(&dotted).cloned()
                }
            }
        }
    }
}

// ===========================================================================
// ConstExpr
// ===========================================================================

/// A constant value expression that always returns the same [`Value`].
///
/// Used to wrap literal values so they can participate in expression trees
/// (e.g. `this.count + 1` where `1` is a `ConstExpr`).
#[derive(Clone, Debug, PartialEq)]
pub struct ConstExpr {
    /// The constant value to return.
    pub value: Value,
}

impl ConstExpr {
    /// Creates a new `ConstExpr` wrapping the given value.
    ///
    /// # Example
    ///
    /// ```
    /// # use construct::expr::ConstExpr;
    /// # use construct::value::Value;
    /// let expr = ConstExpr::new(Value::Int(42));
    /// ```
    #[must_use]
    pub fn new(value: Value) -> Self {
        ConstExpr { value }
    }
}

impl Evaluate for ConstExpr {
    fn evaluate(&self, _ctx: &Context, _obj: Option<&Value>) -> Result<Value> {
        Ok(self.value.clone())
    }
}

// ===========================================================================
// BinOp
// ===========================================================================

/// Binary operators supported by [`BinExpr`].
///
/// Each variant corresponds to a Python `operator` module function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    /// Addition (`+`).
    Add,
    /// Subtraction (`-`).
    Sub,
    /// Multiplication (`*`).
    Mul,
    /// Division (`/`).
    Div,
    /// Modulo (`%`).
    Mod,
    /// Left shift (`<<`).
    Shl,
    /// Right shift (`>>`).
    Shr,
    /// Bitwise AND (`&`).
    BitAnd,
    /// Bitwise OR (`|`).
    BitOr,
    /// Bitwise XOR (`^`).
    BitXor,
    /// Equality (`==`).
    Eq,
    /// Inequality (`!=`).
    Ne,
    /// Less than (`<`).
    Lt,
    /// Greater than (`>`).
    Gt,
    /// Less than or equal (`<=`).
    Le,
    /// Greater than or equal (`>=`).
    Ge,
    /// Logical AND (`and`).
    And,
    /// Logical OR (`or`).
    Or,
}

/// Returns the human-readable symbol for the given operator.
fn bin_op_symbol(op: &BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Shl => "<<",
        BinOp::Shr => ">>",
        BinOp::BitAnd => "&",
        BinOp::BitOr => "|",
        BinOp::BitXor => "^",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Gt => ">",
        BinOp::Le => "<=",
        BinOp::Ge => ">=",
        BinOp::And => "and",
        BinOp::Or => "or",
    }
}

// ===========================================================================
// BinExpr
// ===========================================================================

/// A binary expression that applies an operator to two sub-expressions.
///
/// Corresponds to the Python `BinExpr` class.
///
/// # Example
///
/// ```
/// # use construct::expr::{this_, ConstExpr, BinExpr, BinOp, Evaluate};
/// # use construct::core::context::Context;
/// # use construct::value::Value;
/// let expr = BinExpr::new(
///     BinOp::Add,
///     this_().field("count"),
///     ConstExpr::new(Value::Int(1)),
/// );
/// let mut ctx = Context::new();
/// ctx.insert("count", Value::Int(10));
/// let result = expr.evaluate(&ctx, None).unwrap();
/// assert_eq!(result, Value::Int(11));
/// ```
#[derive(Debug)]
pub struct BinExpr {
    /// The binary operator to apply.
    pub op: BinOp,
    /// The left-hand side expression.
    pub left: Box<dyn Evaluate>,
    /// The right-hand side expression.
    pub right: Box<dyn Evaluate>,
}

impl BinExpr {
    /// Creates a new `BinExpr` with the given operator and sub-expressions.
    ///
    /// # Example
    ///
    /// ```
    /// # use construct::expr::{BinExpr, BinOp, ConstExpr, Evaluate};
    /// # use construct::value::Value;
    /// let expr = BinExpr::new(
    ///     BinOp::Mul,
    ///     ConstExpr::new(Value::Int(3)),
    ///     ConstExpr::new(Value::Int(4)),
    /// );
    /// ```
    pub fn new<L: Evaluate + 'static, R: Evaluate + 'static>(op: BinOp, left: L, right: R) -> Self {
        BinExpr {
            op,
            left: Box::new(left),
            right: Box::new(right),
        }
    }
}

impl Evaluate for BinExpr {
    fn evaluate(&self, ctx: &Context, obj: Option<&Value>) -> Result<Value> {
        let lhs = self.left.evaluate(ctx, obj)?;
        let rhs = self.right.evaluate(ctx, obj)?;

        match self.op {
            BinOp::Add => apply_arithmetic(
                &lhs,
                &rhs,
                i64::wrapping_add,
                u64::wrapping_add,
                i128::wrapping_add,
                |a, b| a + b,
            ),
            BinOp::Sub => apply_arithmetic(
                &lhs,
                &rhs,
                i64::wrapping_sub,
                u64::wrapping_sub,
                i128::wrapping_sub,
                |a, b| a - b,
            ),
            BinOp::Mul => apply_arithmetic(
                &lhs,
                &rhs,
                i64::wrapping_mul,
                u64::wrapping_mul,
                i128::wrapping_mul,
                |a, b| a * b,
            ),
            BinOp::Div => {
                if is_zero(&rhs) {
                    return Err(ConstructError::Expr {
                        path: String::new(),
                        message: "division by zero".to_string(),
                    });
                }
                apply_arithmetic(
                    &lhs,
                    &rhs,
                    i64::wrapping_div,
                    u64::wrapping_div,
                    i128::wrapping_div,
                    |a, b| a / b,
                )
            }
            BinOp::Mod => {
                if is_zero(&rhs) {
                    return Err(ConstructError::Expr {
                        path: String::new(),
                        message: "modulo by zero".to_string(),
                    });
                }
                apply_arithmetic(
                    &lhs,
                    &rhs,
                    i64::wrapping_rem,
                    u64::wrapping_rem,
                    i128::wrapping_rem,
                    |a, b| a % b,
                )
            }
            BinOp::Shl => apply_shift(&lhs, &rhs, |a, b| a.wrapping_shl(b as u32)),
            BinOp::Shr => apply_shift(&lhs, &rhs, |a, b| a.wrapping_shr(b as u32)),
            BinOp::BitAnd => apply_bitwise(&lhs, &rhs, |a, b| a & b),
            BinOp::BitOr => apply_bitwise(&lhs, &rhs, |a, b| a | b),
            BinOp::BitXor => apply_bitwise(&lhs, &rhs, |a, b| a ^ b),
            BinOp::Eq => Ok(Value::Bool(lhs == rhs)),
            BinOp::Ne => Ok(Value::Bool(lhs != rhs)),
            BinOp::Lt => apply_cmp(&lhs, &rhs, |a, b| a < b, |a, b| a < b),
            BinOp::Gt => apply_cmp(&lhs, &rhs, |a, b| a > b, |a, b| a > b),
            BinOp::Le => apply_cmp(&lhs, &rhs, |a, b| a <= b, |a, b| a <= b),
            BinOp::Ge => apply_cmp(&lhs, &rhs, |a, b| a >= b, |a, b| a >= b),
            BinOp::And => apply_logical(&lhs, &rhs, |a, b| a && b),
            BinOp::Or => apply_logical(&lhs, &rhs, |a, b| a || b),
        }
        .map_err(|e| {
            if matches!(e, ConstructError::Expr { .. }) {
                e
            } else {
                ConstructError::Expr {
                    path: String::new(),
                    message: format!(
                        "binary operation '{}' failed: {}",
                        bin_op_symbol(&self.op),
                        e
                    ),
                }
            }
        })
    }
}

/// Checks if a value is numeric zero (for division/modulo checks).
fn is_zero(v: &Value) -> bool {
    match v {
        Value::Int(i) => *i == 0,
        Value::UInt(u) => *u == 0,
        Value::BigInt(i) => *i == 0,
        Value::Float(f) => *f == 0.0,
        _ => false,
    }
}

/// Applies an arithmetic operation to two integer or float values.
///
/// Uses `wrapping_*` semantics for integer operations to avoid panicking on
/// overflow in debug mode.
fn apply_arithmetic(
    lhs: &Value,
    rhs: &Value,
    int_op: impl Fn(i64, i64) -> i64,
    uint_op: impl Fn(u64, u64) -> u64,
    bigint_op: impl Fn(i128, i128) -> i128,
    float_op: impl Fn(f64, f64) -> f64,
) -> Result<Value> {
    // Try integer arithmetic first
    match (lhs, rhs) {
        // Both Int → Int (wrapping)
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(int_op(*a, *b))),
        // Both UInt → UInt (wrapping)
        (Value::UInt(a), Value::UInt(b)) => Ok(Value::UInt(uint_op(*a, *b))),
        // Mixed Int/UInt → promote to i64 (wrapping)
        (Value::Int(a), Value::UInt(b)) => Ok(Value::Int(int_op(*a, *b as i64))),
        (Value::UInt(a), Value::Int(b)) => Ok(Value::Int(int_op(*a as i64, *b))),
        // BigInt involved — use i128 wrapping arithmetic
        (Value::BigInt(a), Value::BigInt(b)) => Ok(Value::BigInt(bigint_op(*a, *b))),
        (Value::BigInt(a), Value::Int(b)) => Ok(Value::BigInt(bigint_op(*a, *b as i128))),
        (Value::BigInt(a), Value::UInt(b)) => Ok(Value::BigInt(bigint_op(*a, *b as i128))),
        (Value::Int(a), Value::BigInt(b)) => Ok(Value::BigInt(bigint_op(*a as i128, *b))),
        (Value::UInt(a), Value::BigInt(b)) => Ok(Value::BigInt(bigint_op(*a as i128, *b))),
        // Float involved → float arithmetic
        _ => {
            let a = lhs.to_f64().map_err(|e| ConstructError::Expr {
                path: String::new(),
                message: format!("left operand is not numeric: {e}"),
            })?;
            let b = rhs.to_f64().map_err(|e| ConstructError::Expr {
                path: String::new(),
                message: format!("right operand is not numeric: {e}"),
            })?;
            Ok(Value::Float(float_op(a, b)))
        }
    }
}

/// Applies a bitwise operation to two integer values.
fn apply_bitwise(lhs: &Value, rhs: &Value, op: impl Fn(i64, i64) -> i64) -> Result<Value> {
    match (lhs, rhs) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(op(*a, *b))),
        (Value::UInt(a), Value::UInt(b)) => {
            let result = op(*a as i64, *b as i64);
            if result >= 0 {
                Ok(Value::UInt(result as u64))
            } else {
                Ok(Value::Int(result))
            }
        }
        (Value::Int(a), Value::UInt(b)) => Ok(Value::Int(op(*a, *b as i64))),
        (Value::UInt(a), Value::Int(b)) => Ok(Value::Int(op(*a as i64, *b))),
        (Value::BigInt(a), Value::BigInt(b)) => {
            let result = op(*a as i64, *b as i64) as i128;
            Ok(Value::BigInt(result))
        }
        (Value::BigInt(a), Value::Int(b)) => {
            let result = op(*a as i64, *b) as i128;
            Ok(Value::BigInt(result))
        }
        (Value::BigInt(a), Value::UInt(b)) => {
            let result = op(*a as i64, *b as i64) as i128;
            Ok(Value::BigInt(result))
        }
        (Value::Int(a), Value::BigInt(b)) => {
            let result = op(*a, *b as i64) as i128;
            Ok(Value::BigInt(result))
        }
        (Value::UInt(a), Value::BigInt(b)) => {
            let result = op(*a as i64, *b as i64) as i128;
            Ok(Value::BigInt(result))
        }
        _ => Err(ConstructError::Expr {
            path: String::new(),
            message: format!(
                "bitwise operation requires integer operands, got {} and {}",
                lhs.type_name(),
                rhs.type_name()
            ),
        }),
    }
}

/// Applies a shift operation to two integer values.
fn apply_shift(lhs: &Value, rhs: &Value, op: impl Fn(i64, i64) -> i64) -> Result<Value> {
    let shift_amount = rhs.to_u64().map_err(|e| ConstructError::Expr {
        path: String::new(),
        message: format!("shift amount must be a non-negative integer: {e}"),
    })?;

    // Limit shift amount to 63 to avoid undefined behavior
    if shift_amount > 63 {
        return Err(ConstructError::Expr {
            path: String::new(),
            message: format!("shift amount {shift_amount} exceeds maximum of 63"),
        });
    }

    match lhs {
        Value::Int(a) => Ok(Value::Int(op(*a, shift_amount as i64))),
        Value::UInt(a) => {
            let result = op(*a as i64, shift_amount as i64);
            if result >= 0 {
                Ok(Value::UInt(result as u64))
            } else {
                Ok(Value::Int(result))
            }
        }
        Value::BigInt(a) => Ok(Value::BigInt(op(*a as i64, shift_amount as i64) as i128)),
        _ => Err(ConstructError::Expr {
            path: String::new(),
            message: format!(
                "shift operation requires integer left operand, got {}",
                lhs.type_name()
            ),
        }),
    }
}

/// Applies a comparison operation to two values.
///
/// Integer comparisons use the `int_op` closure. When one or both operands
/// are floats (and not both integers), the values are converted to `f64` and
/// compared with the `float_op` closure. This avoids the truncation bug that
/// would occur if float values were cast to `i64` before comparing.
fn apply_cmp(
    lhs: &Value,
    rhs: &Value,
    int_op: impl Fn(i64, i64) -> bool,
    float_op: impl Fn(f64, f64) -> bool,
) -> Result<Value> {
    // Try integer comparison
    match (lhs, rhs) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(int_op(*a, *b))),
        (Value::UInt(a), Value::UInt(b)) => Ok(Value::Bool(int_op(*a as i64, *b as i64))),
        (Value::Int(a), Value::UInt(b)) => Ok(Value::Bool(int_op(*a, *b as i64))),
        (Value::UInt(a), Value::Int(b)) => Ok(Value::Bool(int_op(*a as i64, *b))),
        (Value::BigInt(a), Value::BigInt(b)) => Ok(Value::Bool(int_op(*a as i64, *b as i64))),
        (Value::BigInt(a), Value::Int(b)) => Ok(Value::Bool(int_op(*a as i64, *b))),
        (Value::BigInt(a), Value::UInt(b)) => Ok(Value::Bool(int_op(*a as i64, *b as i64))),
        (Value::Int(a), Value::BigInt(b)) => Ok(Value::Bool(int_op(*a, *b as i64))),
        (Value::UInt(a), Value::BigInt(b)) => Ok(Value::Bool(int_op(*a as i64, *b as i64))),
        _ => {
            // Fall back to float comparison — use float_op directly on f64 values
            let a = lhs.to_f64().map_err(|e| ConstructError::Expr {
                path: String::new(),
                message: format!("comparison requires numeric operands: {e}"),
            })?;
            let b = rhs.to_f64().map_err(|e| ConstructError::Expr {
                path: String::new(),
                message: format!("comparison requires numeric operands: {e}"),
            })?;
            Ok(Value::Bool(float_op(a, b)))
        }
    }
}

/// Applies a logical (boolean) operation to two values.
fn apply_logical(lhs: &Value, rhs: &Value, op: impl Fn(bool, bool) -> bool) -> Result<Value> {
    let a = value_to_bool(lhs)?;
    let b = value_to_bool(rhs)?;
    Ok(Value::Bool(op(a, b)))
}

/// Converts a `Value` to a boolean for logical operations.
///
/// Follows Python truthiness rules:
/// - `None` → false
/// - `Bool(b)` → b
/// - Numeric zero → false, nonzero → true
/// - Empty string/bytes/list/container → false, nonempty → true
fn value_to_bool(v: &Value) -> Result<bool> {
    match v {
        Value::None => Ok(false),
        Value::Bool(b) => Ok(*b),
        Value::Int(i) => Ok(*i != 0),
        Value::UInt(u) => Ok(*u != 0),
        Value::BigInt(i) => Ok(*i != 0),
        Value::Float(f) => Ok(*f != 0.0),
        Value::Bytes(b) => Ok(!b.is_empty()),
        Value::String(s) => Ok(!s.is_empty()),
        Value::List(l) => Ok(!l.is_empty()),
        Value::Container(c) => Ok(!c.is_empty()),
    }
}

// ===========================================================================
// UniOp
// ===========================================================================

/// Unary operators supported by [`UniExpr`].
///
/// Each variant corresponds to a Python `operator` module function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UniOp {
    /// Negation (`-`).
    Neg,
    /// Logical NOT (`not`).
    Not,
    /// Bitwise NOT (`~`).
    BitNot,
}

// ===========================================================================
// UniExpr
// ===========================================================================

/// A unary expression that applies an operator to a single sub-expression.
///
/// Corresponds to the Python `UniExpr` class.
///
/// # Example
///
/// ```
/// # use construct::expr::{UniExpr, UniOp, ConstExpr, Evaluate};
/// # use construct::core::context::Context;
/// # use construct::value::Value;
/// let expr = UniExpr::new(UniOp::Neg, ConstExpr::new(Value::Int(42)));
/// let result = expr.evaluate(&Context::new(), None).unwrap();
/// assert_eq!(result, Value::Int(-42));
/// ```
#[derive(Debug)]
pub struct UniExpr {
    /// The unary operator to apply.
    pub op: UniOp,
    /// The inner expression.
    pub inner: Box<dyn Evaluate>,
}

impl UniExpr {
    /// Creates a new `UniExpr` with the given operator and inner expression.
    pub fn new<E: Evaluate + 'static>(op: UniOp, inner: E) -> Self {
        UniExpr {
            op,
            inner: Box::new(inner),
        }
    }
}

impl Evaluate for UniExpr {
    fn evaluate(&self, ctx: &Context, obj: Option<&Value>) -> Result<Value> {
        let val = self.inner.evaluate(ctx, obj)?;

        match self.op {
            UniOp::Neg => match val {
                Value::Int(i) => Ok(Value::Int(-i)),
                Value::UInt(u) => Ok(Value::Int(-(u as i64))),
                Value::BigInt(i) => Ok(Value::BigInt(-i)),
                Value::Float(f) => Ok(Value::Float(-f)),
                _ => Err(ConstructError::Expr {
                    path: String::new(),
                    message: format!("negation requires a numeric value, got {}", val.type_name()),
                }),
            },
            UniOp::Not => {
                let b = value_to_bool(&val)?;
                Ok(Value::Bool(!b))
            }
            UniOp::BitNot => match val {
                Value::Int(i) => Ok(Value::Int(!i)),
                Value::UInt(u) => Ok(Value::UInt(!u)),
                Value::BigInt(i) => Ok(Value::BigInt(!i)),
                _ => Err(ConstructError::Expr {
                    path: String::new(),
                    message: format!(
                        "bitwise NOT requires an integer value, got {}",
                        val.type_name()
                    ),
                }),
            },
        }
    }
}

// ===========================================================================
// FuncPath
// ===========================================================================

/// A built-in function expression that applies a named function to a
/// sub-expression.
///
/// Corresponds to the Python `FuncPath` class and the global instances
/// `len_`, `sum_`, `min_`, `max_`, `abs_`.
///
/// # Supported functions
///
/// | Name | Behaviour |
/// |------|-----------|
/// | `len` | Returns the length of a `List`, `Bytes`, `String`, or `Container` |
/// | `sum` | Returns the sum of all elements in a `List` of integers |
/// | `min` | Returns the minimum element in a `List` of integers |
/// | `max` | Returns the maximum element in a `List` of integers |
/// | `abs` | Returns the absolute value of an integer or float |
///
/// # Example
///
/// ```
/// # use construct::expr::{FuncPath, this_, Evaluate};
/// # use construct::core::context::Context;
/// # use construct::value::Value;
/// let expr = FuncPath::new("len", this_().field("items"));
/// ```
#[derive(Debug)]
pub struct FuncPath {
    /// The function name (e.g. `"len"`, `"sum"`, `"min"`, `"max"`, `"abs"`).
    pub name: String,
    /// The inner expression whose result is passed to the function.
    pub inner: Box<dyn Evaluate>,
}

impl FuncPath {
    /// Creates a new `FuncPath` with the given function name and inner expression.
    ///
    /// # Errors
    ///
    /// Evaluation will return an error if the function name is not recognized
    /// or the inner value is not compatible with the function.
    pub fn new<E: Evaluate + 'static>(name: impl Into<String>, inner: E) -> Self {
        FuncPath {
            name: name.into(),
            inner: Box::new(inner),
        }
    }
}

impl Evaluate for FuncPath {
    fn evaluate(&self, ctx: &Context, obj: Option<&Value>) -> Result<Value> {
        let val = self.inner.evaluate(ctx, obj)?;

        match self.name.as_str() {
            "len" => match &val {
                Value::List(l) => Ok(Value::UInt(l.len() as u64)),
                Value::Bytes(b) => Ok(Value::UInt(b.len() as u64)),
                Value::String(s) => Ok(Value::UInt(s.len() as u64)),
                Value::Container(c) => Ok(Value::UInt(c.len() as u64)),
                _ => Err(ConstructError::Expr {
                    path: String::new(),
                    message: format!(
                        "len_() requires a List, Bytes, String, or Container, got {}",
                        val.type_name()
                    ),
                }),
            },
            "sum" => {
                let list = val.as_list().map_err(|e| ConstructError::Expr {
                    path: String::new(),
                    message: format!("sum_() requires a List: {e}"),
                })?;
                let mut total: i64 = 0;
                for item in list {
                    total = total
                        .checked_add(item.to_i64().map_err(|e| ConstructError::Expr {
                            path: String::new(),
                            message: format!("sum_() element is not an integer: {e}"),
                        })?)
                        .ok_or_else(|| ConstructError::Expr {
                            path: String::new(),
                            message: "sum_() overflow".to_string(),
                        })?;
                }
                Ok(Value::Int(total))
            }
            "min" => {
                let list = val.as_list().map_err(|e| ConstructError::Expr {
                    path: String::new(),
                    message: format!("min_() requires a List: {e}"),
                })?;
                if list.is_empty() {
                    return Err(ConstructError::Expr {
                        path: String::new(),
                        message: "min_() requires a non-empty List".to_string(),
                    });
                }
                let mut min_val = list[0].to_i64().map_err(|e| ConstructError::Expr {
                    path: String::new(),
                    message: format!("min_() element is not an integer: {e}"),
                })?;
                for item in &list[1..] {
                    let v = item.to_i64().map_err(|e| ConstructError::Expr {
                        path: String::new(),
                        message: format!("min_() element is not an integer: {e}"),
                    })?;
                    if v < min_val {
                        min_val = v;
                    }
                }
                Ok(Value::Int(min_val))
            }
            "max" => {
                let list = val.as_list().map_err(|e| ConstructError::Expr {
                    path: String::new(),
                    message: format!("max_() requires a List: {e}"),
                })?;
                if list.is_empty() {
                    return Err(ConstructError::Expr {
                        path: String::new(),
                        message: "max_() requires a non-empty List".to_string(),
                    });
                }
                let mut max_val = list[0].to_i64().map_err(|e| ConstructError::Expr {
                    path: String::new(),
                    message: format!("max_() element is not an integer: {e}"),
                })?;
                for item in &list[1..] {
                    let v = item.to_i64().map_err(|e| ConstructError::Expr {
                        path: String::new(),
                        message: format!("max_() element is not an integer: {e}"),
                    })?;
                    if v > max_val {
                        max_val = v;
                    }
                }
                Ok(Value::Int(max_val))
            }
            "abs" => match &val {
                Value::Int(i) => Ok(Value::Int(i.abs())),
                Value::UInt(u) => Ok(Value::UInt(*u)),
                Value::BigInt(i) => Ok(Value::BigInt(i.abs())),
                Value::Float(f) => Ok(Value::Float(f.abs())),
                _ => Err(ConstructError::Expr {
                    path: String::new(),
                    message: format!("abs_() requires a numeric value, got {}", val.type_name()),
                }),
            },
            _ => Err(ConstructError::Expr {
                path: String::new(),
                message: format!("unknown function: {}", self.name),
            }),
        }
    }
}

// ===========================================================================
// CondExpr
// ===========================================================================

/// A conditional (ternary) expression that selects between two sub-expressions.
///
/// Corresponds to Python's `a if cond else b` pattern.
///
/// # Example
///
/// ```
/// # use construct::expr::{CondExpr, ConstExpr, BinExpr, BinOp, Evaluate};
/// # use construct::core::context::Context;
/// # use construct::value::Value;
/// let expr = CondExpr::new(
///     BinExpr::new(BinOp::Gt, ConstExpr::new(Value::Int(5)), ConstExpr::new(Value::Int(3))),
///     ConstExpr::new(Value::String("yes".to_string())),
///     ConstExpr::new(Value::String("no".to_string())),
/// );
/// let result = expr.evaluate(&Context::new(), None).unwrap();
/// assert_eq!(result, Value::String("yes".to_string()));
/// ```
#[derive(Debug)]
pub struct CondExpr {
    /// The condition expression.
    pub cond: Box<dyn Evaluate>,
    /// The expression to evaluate if the condition is truthy.
    pub then_val: Box<dyn Evaluate>,
    /// The expression to evaluate if the condition is falsy.
    pub else_val: Box<dyn Evaluate>,
}

impl CondExpr {
    /// Creates a new `CondExpr` with the given condition and branches.
    pub fn new<C: Evaluate + 'static, T: Evaluate + 'static, E: Evaluate + 'static>(
        cond: C,
        then_val: T,
        else_val: E,
    ) -> Self {
        CondExpr {
            cond: Box::new(cond),
            then_val: Box::new(then_val),
            else_val: Box::new(else_val),
        }
    }
}

impl Evaluate for CondExpr {
    fn evaluate(&self, ctx: &Context, obj: Option<&Value>) -> Result<Value> {
        let cond_val = self.cond.evaluate(ctx, obj)?;
        if value_to_bool(&cond_val)? {
            self.then_val.evaluate(ctx, obj)
        } else {
            self.else_val.evaluate(ctx, obj)
        }
    }
}

// ===========================================================================
// ListPath (Python Path2 / list_)
// ===========================================================================

/// A list-indexing path expression, corresponding to Python's `Path2` / `list_`.
///
/// `ListPath` resolves a list value from a parent expression and indexes into
/// it. Negative indices count from the end (Python-style), e.g. `-1` refers to
/// the last element.
///
/// The primary use case is `list_[-1]` in constructs like `RepeatUntil`, which
/// needs to inspect the last element parsed so far.
///
/// # Python correspondence
///
/// | Python | Rust |
/// |--------|------|
/// | `list_` | `list_()` (bare, no index) |
/// | `list_[-1]` | `list_().index(-1)` |
/// | `list_[0]` | `list_().index(0)` |
///
/// # Example
///
/// ```
/// # use construct::expr::{ListPath, ConstExpr, Evaluate};
/// # use construct::core::context::Context;
/// # use construct::value::Value;
/// // list_[-1] on a list [10, 20, 30] → 30
/// let list_val = Value::List(vec![Value::Int(10), Value::Int(20), Value::Int(30)]);
/// let expr = ListPath::new_indexed(-1, ConstExpr::new(list_val));
/// let result = expr.evaluate(&Context::new(), None).unwrap();
/// assert_eq!(result, Value::Int(30));
/// ```
#[derive(Debug)]
pub struct ListPath {
    /// The index into the list. Negative values count from the end.
    pub index: i64,
    /// The inner expression that resolves to a [`Value::List`].
    /// If `None`, the list is expected to be found in the context under the
    /// special key `list_` (for bare `list_()` without an inner expression).
    pub inner: Option<Box<dyn Evaluate>>,
}

impl ListPath {
    /// Creates a new `ListPath` that indexes into the list produced by `inner`.
    ///
    /// Negative indices count from the end: `-1` is the last element.
    #[must_use]
    pub fn new_indexed<E: Evaluate + 'static>(index: i64, inner: E) -> Self {
        ListPath {
            index,
            inner: Some(Box::new(inner)),
        }
    }

    /// Creates a bare `ListPath` without an inner expression.
    ///
    /// When evaluated, this looks up the `"list_"` key in the context.
    /// This is typically chained with [`index`](ListPath::index) to create
    /// expressions like `list_[-1]`.
    #[must_use]
    pub fn new_bare() -> Self {
        ListPath {
            index: 0,
            inner: None,
        }
    }

    /// Returns a new `ListPath` with the given index.
    ///
    /// Only valid when called on a bare [`ListPath`] (created by [`list_()`]).
    /// The returned `ListPath` will look up the `"list_"` key in the context
    /// and index into the resulting list.
    ///
    /// This is the Rust equivalent of Python's `list_[-1]`.
    #[must_use]
    pub fn index(&self, idx: i64) -> Self {
        ListPath {
            index: idx,
            inner: None,
        }
    }
}

impl Evaluate for ListPath {
    fn evaluate(&self, ctx: &Context, obj: Option<&Value>) -> Result<Value> {
        let list_val = if let Some(ref inner) = self.inner {
            inner.evaluate(ctx, obj)?
        } else {
            // Bare list_(): look up "list_" in context
            ctx.get("list_")
                .cloned()
                .ok_or_else(|| ConstructError::Expr {
                    path: String::new(),
                    message: "list_ path used but no 'list_' key found in context".to_string(),
                })?
        };

        let list = list_val.as_list().map_err(|e| ConstructError::Expr {
            path: String::new(),
            message: format!("list_ requires a List value: {e}"),
        })?;

        let len = list.len() as i64;
        let resolved_index = if self.index < 0 {
            len + self.index
        } else {
            self.index
        };

        if resolved_index < 0 || resolved_index >= len {
            return Err(ConstructError::Expr {
                path: String::new(),
                message: format!(
                    "list_ index {} out of range for list of length {}",
                    self.index,
                    list.len()
                ),
            });
        }

        Ok(list[resolved_index as usize].clone())
    }
}

/// Creates a bare [`ListPath`] corresponding to Python's `list_`.
///
/// Use [`ListPath::index`] to specify the index:
///
/// ```
/// # use construct::expr::{list_, Evaluate};
/// # use construct::core::context::Context;
/// # use construct::value::Value;
/// let mut ctx = Context::new();
/// ctx.insert("list_", Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]));
/// let expr = list_().index(-1);
/// let result = expr.evaluate(&ctx, None).unwrap();
/// assert_eq!(result, Value::Int(3));
/// ```
#[must_use]
pub fn list_() -> ListPath {
    ListPath::new_bare()
}

// ===========================================================================
// IntoExpr
// ===========================================================================

/// A trait for converting a value into a boxed expression node.
///
/// This allows APIs to accept either a literal value, a [`Path`], or an
/// existing `Box<dyn Evaluate>` interchangeably.
///
/// # Example
///
/// ```
/// # use construct::expr::{IntoExpr, this_, Path, PathRoot, ConstExpr, Evaluate};
/// # use construct::core::context::Context;
/// # use construct::value::Value;
/// let mut ctx = Context::new();
/// ctx.insert("x", Value::Int(42));
///
/// // usize converts to a ConstExpr
/// let expr = 10usize.into_expr();
/// assert_eq!(expr.evaluate(&ctx, None).unwrap(), Value::UInt(10));
///
/// // Path converts directly
/// let path: Path = this_().field("x");
/// let expr = path.into_expr();
/// assert_eq!(expr.evaluate(&ctx, None).unwrap(), Value::Int(42));
/// ```
pub trait IntoExpr: 'static {
    /// Converts `self` into a boxed expression node.
    fn into_expr(self) -> Box<dyn Evaluate>;
}

impl IntoExpr for usize {
    fn into_expr(self) -> Box<dyn Evaluate> {
        Box::new(ConstExpr::new(Value::UInt(self as u64)))
    }
}

impl IntoExpr for i64 {
    fn into_expr(self) -> Box<dyn Evaluate> {
        Box::new(ConstExpr::new(Value::Int(self)))
    }
}

impl IntoExpr for u64 {
    fn into_expr(self) -> Box<dyn Evaluate> {
        Box::new(ConstExpr::new(Value::UInt(self)))
    }
}

impl IntoExpr for Path {
    fn into_expr(self) -> Box<dyn Evaluate> {
        Box::new(self)
    }
}

impl IntoExpr for ListPath {
    fn into_expr(self) -> Box<dyn Evaluate> {
        Box::new(self)
    }
}

impl IntoExpr for Box<dyn Evaluate> {
    fn into_expr(self) -> Box<dyn Evaluate> {
        self
    }
}

// ===========================================================================
// Convenience constructors
// ===========================================================================

/// Creates a new [`Path`] rooted at the current context (`this`).
///
/// # Example
///
/// ```
/// # use construct::expr::{this_, PathRoot, Evaluate};
/// # use construct::core::context::Context;
/// # use construct::value::Value;
/// let mut ctx = Context::new();
/// ctx.insert("count", Value::Int(5));
/// let path = this_().field("count");
/// let result = path.evaluate(&ctx, None).unwrap();
/// assert_eq!(result, Value::Int(5));
/// ```
#[must_use]
pub fn this_() -> Path {
    Path::new(PathRoot::This)
}

/// Creates a new [`Path`] rooted at the build object (`obj_`).
///
/// # Example
///
/// ```
/// # use construct::expr::{obj_, PathRoot, Evaluate};
/// # use construct::core::context::Context;
/// # use construct::value::Value;
/// let obj = Value::Int(99);
/// let path = obj_();
/// let result = path.evaluate(&Context::new(), Some(&obj)).unwrap();
/// assert_eq!(result, Value::Int(99));
/// ```
#[must_use]
pub fn obj_() -> Path {
    Path::new(PathRoot::Obj)
}

/// Creates a [`ConstExpr`] from a [`Value`].
///
/// Convenience function for wrapping literal values.
///
/// # Example
///
/// ```
/// # use construct::expr::{const_val, Evaluate};
/// # use construct::core::context::Context;
/// # use construct::value::Value;
/// let expr = const_val(Value::Int(42));
/// let result = expr.evaluate(&Context::new(), None).unwrap();
/// assert_eq!(result, Value::Int(42));
/// ```
#[must_use]
pub fn const_val(value: Value) -> ConstExpr {
    ConstExpr::new(value)
}

// ===========================================================================
// Operator methods on Path (builder-style)
// ===========================================================================

// Clippy warns that these method names (add, sub, mul, etc.) shadow standard
// library trait methods (Add, Sub, Mul, etc.). This is intentional — the task
// design explicitly calls for builder-style methods rather than trait overloads.
#[allow(clippy::should_implement_trait)]
impl Path {
    /// Returns a [`BinExpr`] that adds this path and `rhs`.
    pub fn add<E: Evaluate + 'static>(self, rhs: E) -> BinExpr {
        BinExpr::new(BinOp::Add, self, rhs)
    }

    /// Returns a [`BinExpr`] that subtracts `rhs` from this path.
    pub fn sub<E: Evaluate + 'static>(self, rhs: E) -> BinExpr {
        BinExpr::new(BinOp::Sub, self, rhs)
    }

    /// Returns a [`BinExpr`] that multiplies this path by `rhs`.
    pub fn mul<E: Evaluate + 'static>(self, rhs: E) -> BinExpr {
        BinExpr::new(BinOp::Mul, self, rhs)
    }

    /// Returns a [`BinExpr`] that divides this path by `rhs`.
    pub fn div<E: Evaluate + 'static>(self, rhs: E) -> BinExpr {
        BinExpr::new(BinOp::Div, self, rhs)
    }

    /// Returns a [`BinExpr`] that computes `this % rhs`.
    pub fn rem<E: Evaluate + 'static>(self, rhs: E) -> BinExpr {
        BinExpr::new(BinOp::Mod, self, rhs)
    }

    /// Returns a [`BinExpr`] that computes `this << rhs`.
    pub fn shl<E: Evaluate + 'static>(self, rhs: E) -> BinExpr {
        BinExpr::new(BinOp::Shl, self, rhs)
    }

    /// Returns a [`BinExpr`] that computes `this >> rhs`.
    pub fn shr<E: Evaluate + 'static>(self, rhs: E) -> BinExpr {
        BinExpr::new(BinOp::Shr, self, rhs)
    }

    /// Returns a [`BinExpr`] that computes `this & rhs`.
    pub fn bitand<E: Evaluate + 'static>(self, rhs: E) -> BinExpr {
        BinExpr::new(BinOp::BitAnd, self, rhs)
    }

    /// Returns a [`BinExpr`] that computes `this | rhs`.
    pub fn bitor<E: Evaluate + 'static>(self, rhs: E) -> BinExpr {
        BinExpr::new(BinOp::BitOr, self, rhs)
    }

    /// Returns a [`BinExpr`] that computes `this ^ rhs`.
    pub fn bitxor<E: Evaluate + 'static>(self, rhs: E) -> BinExpr {
        BinExpr::new(BinOp::BitXor, self, rhs)
    }

    /// Returns a [`BinExpr`] that computes `this == rhs`.
    pub fn eq_expr<E: Evaluate + 'static>(self, rhs: E) -> BinExpr {
        BinExpr::new(BinOp::Eq, self, rhs)
    }

    /// Returns a [`BinExpr`] that computes `this != rhs`.
    pub fn ne_expr<E: Evaluate + 'static>(self, rhs: E) -> BinExpr {
        BinExpr::new(BinOp::Ne, self, rhs)
    }

    /// Returns a [`BinExpr`] that computes `this < rhs`.
    pub fn lt<E: Evaluate + 'static>(self, rhs: E) -> BinExpr {
        BinExpr::new(BinOp::Lt, self, rhs)
    }

    /// Returns a [`BinExpr`] that computes `this > rhs`.
    pub fn gt<E: Evaluate + 'static>(self, rhs: E) -> BinExpr {
        BinExpr::new(BinOp::Gt, self, rhs)
    }

    /// Returns a [`BinExpr`] that computes `this <= rhs`.
    pub fn le<E: Evaluate + 'static>(self, rhs: E) -> BinExpr {
        BinExpr::new(BinOp::Le, self, rhs)
    }

    /// Returns a [`BinExpr`] that computes `this >= rhs`.
    pub fn ge<E: Evaluate + 'static>(self, rhs: E) -> BinExpr {
        BinExpr::new(BinOp::Ge, self, rhs)
    }

    /// Returns a [`UniExpr`] that negates this path.
    pub fn neg(self) -> UniExpr {
        UniExpr::new(UniOp::Neg, self)
    }

    /// Returns a [`UniExpr`] that logically negates this path.
    pub fn not(self) -> UniExpr {
        UniExpr::new(UniOp::Not, self)
    }

    /// Returns a [`UniExpr`] that bitwise negates this path.
    pub fn bitnot(self) -> UniExpr {
        UniExpr::new(UniOp::BitNot, self)
    }

    /// Returns a [`FuncPath`] that applies `len_()` to this path.
    pub fn len_func(self) -> FuncPath {
        FuncPath::new("len", self)
    }

    /// Returns a [`FuncPath`] that applies `sum_()` to this path.
    pub fn sum_func(self) -> FuncPath {
        FuncPath::new("sum", self)
    }

    /// Returns a [`FuncPath`] that applies `min_()` to this path.
    pub fn min_func(self) -> FuncPath {
        FuncPath::new("min", self)
    }

    /// Returns a [`FuncPath`] that applies `max_()` to this path.
    pub fn max_func(self) -> FuncPath {
        FuncPath::new("max", self)
    }

    /// Returns a [`FuncPath`] that applies `abs_()` to this path.
    pub fn abs_func(self) -> FuncPath {
        FuncPath::new("abs", self)
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    // ======================================================================
    // Path tests
    // ======================================================================

    #[test]
    fn path_new_creates_empty_segments() {
        let path = Path::new(PathRoot::This);
        assert!(path.segments.is_empty());
        assert_eq!(path.root, PathRoot::This);
    }

    #[test]
    fn path_field_appends_segment() {
        let path = Path::new(PathRoot::This).field("header").field("length");
        assert_eq!(path.segments, vec!["header", "length"]);
    }

    #[test]
    fn path_field_does_not_mutate_original() {
        let original = Path::new(PathRoot::This);
        let extended = original.field("x");
        assert!(original.segments.is_empty());
        assert_eq!(extended.segments, vec!["x"]);
    }

    #[test]
    fn path_this_resolves_single_field() {
        let mut ctx = Context::new();
        ctx.insert("count", Value::Int(42));
        let path = this_().field("count");
        let result = path.evaluate(&ctx, None).unwrap();
        assert_eq!(result, Value::Int(42));
    }

    #[test]
    fn path_this_resolves_nested_fields() {
        // ctx: { header: Container { length: 10 } }
        let mut inner = IndexMap::new();
        inner.insert("length".to_string(), Value::Int(10));
        let mut outer = IndexMap::new();
        outer.insert("header".to_string(), Value::Container(inner));
        let mut ctx = Context::new();
        ctx.insert("header", Value::Container(outer));

        let path = this_().field("header").field("header").field("length");
        let result = path.evaluate(&ctx, None).unwrap();
        assert_eq!(result, Value::Int(10));
    }

    #[test]
    fn path_this_missing_field_returns_error() {
        let ctx = Context::new();
        let path = this_().field("nonexistent");
        let err = path.evaluate(&ctx, None).unwrap_err();
        assert!(matches!(err, ConstructError::FieldMissing { .. }));
    }

    #[test]
    fn path_this_bare_returns_error() {
        let ctx = Context::new();
        let path = this_();
        let err = path.evaluate(&ctx, None).unwrap_err();
        assert!(matches!(err, ConstructError::Expr { .. }));
    }

    #[test]
    fn path_obj_resolves_from_build_object() {
        let ctx = Context::new();
        let obj = Value::Int(99);
        let path = obj_();
        let result = path.evaluate(&ctx, Some(&obj)).unwrap();
        assert_eq!(result, Value::Int(99));
    }

    #[test]
    fn path_obj_with_segments() {
        let ctx = Context::new();
        let mut inner = IndexMap::new();
        inner.insert("name".to_string(), Value::String("hello".to_string()));
        let obj = Value::Container(inner);
        let path = obj_().field("name");
        let result = path.evaluate(&ctx, Some(&obj)).unwrap();
        assert_eq!(result, Value::String("hello".to_string()));
    }

    #[test]
    fn path_obj_without_build_object_returns_error() {
        let ctx = Context::new();
        let path = obj_().field("x");
        let err = path.evaluate(&ctx, None).unwrap_err();
        assert!(matches!(err, ConstructError::Expr { .. }));
    }

    #[test]
    fn path_this_underscore_navigates_parent() {
        let mut parent = Context::new();
        parent.insert("field", Value::Int(42));
        let child = parent.subcontext();

        let path = Path {
            root: PathRoot::This,
            segments: vec!["_".to_string(), "field".to_string()],
        };
        let result = path.evaluate(&child, None).unwrap();
        assert_eq!(result, Value::Int(42));
    }

    // ======================================================================
    // ConstExpr tests
    // ======================================================================

    #[test]
    fn const_expr_returns_value() {
        let expr = ConstExpr::new(Value::Int(42));
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(42));
    }

    #[test]
    fn const_expr_with_float() {
        let expr = ConstExpr::new(Value::Float(2.5));
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Float(2.5));
    }

    #[test]
    fn const_expr_with_string() {
        let expr = ConstExpr::new(Value::String("hello".to_string()));
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::String("hello".to_string()));
    }

    // ======================================================================
    // BinExpr arithmetic tests
    // ======================================================================

    #[test]
    fn bin_expr_add_ints() {
        let expr = BinExpr::new(
            BinOp::Add,
            ConstExpr::new(Value::Int(3)),
            ConstExpr::new(Value::Int(4)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(7));
    }

    #[test]
    fn bin_expr_add_uints() {
        let expr = BinExpr::new(
            BinOp::Add,
            ConstExpr::new(Value::UInt(3)),
            ConstExpr::new(Value::UInt(4)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::UInt(7));
    }

    #[test]
    fn bin_expr_add_mixed_int_uint() {
        let expr = BinExpr::new(
            BinOp::Add,
            ConstExpr::new(Value::Int(3)),
            ConstExpr::new(Value::UInt(4)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(7));
    }

    #[test]
    fn bin_expr_add_floats() {
        let expr = BinExpr::new(
            BinOp::Add,
            ConstExpr::new(Value::Float(1.5)),
            ConstExpr::new(Value::Float(2.5)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Float(4.0));
    }

    #[test]
    fn bin_expr_add_int_float() {
        let expr = BinExpr::new(
            BinOp::Add,
            ConstExpr::new(Value::Int(3)),
            ConstExpr::new(Value::Float(2.5)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Float(5.5));
    }

    #[test]
    fn bin_expr_sub() {
        let expr = BinExpr::new(
            BinOp::Sub,
            ConstExpr::new(Value::Int(10)),
            ConstExpr::new(Value::Int(3)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(7));
    }

    #[test]
    fn bin_expr_mul() {
        let expr = BinExpr::new(
            BinOp::Mul,
            ConstExpr::new(Value::Int(6)),
            ConstExpr::new(Value::Int(7)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(42));
    }

    #[test]
    fn bin_expr_div() {
        let expr = BinExpr::new(
            BinOp::Div,
            ConstExpr::new(Value::Int(10)),
            ConstExpr::new(Value::Int(3)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(3)); // integer division
    }

    #[test]
    fn bin_expr_div_by_zero_returns_error() {
        let expr = BinExpr::new(
            BinOp::Div,
            ConstExpr::new(Value::Int(10)),
            ConstExpr::new(Value::Int(0)),
        );
        let err = expr.evaluate(&Context::new(), None).unwrap_err();
        assert!(matches!(err, ConstructError::Expr { .. }));
        assert!(err.to_string().contains("division by zero"));
    }

    #[test]
    fn bin_expr_mod() {
        let expr = BinExpr::new(
            BinOp::Mod,
            ConstExpr::new(Value::Int(10)),
            ConstExpr::new(Value::Int(3)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(1));
    }

    #[test]
    fn bin_expr_mod_by_zero_returns_error() {
        let expr = BinExpr::new(
            BinOp::Mod,
            ConstExpr::new(Value::Int(10)),
            ConstExpr::new(Value::Int(0)),
        );
        let err = expr.evaluate(&Context::new(), None).unwrap_err();
        assert!(matches!(err, ConstructError::Expr { .. }));
        assert!(err.to_string().contains("modulo by zero"));
    }

    // ======================================================================
    // BinExpr bitwise tests
    // ======================================================================

    #[test]
    fn bin_expr_shl() {
        let expr = BinExpr::new(
            BinOp::Shl,
            ConstExpr::new(Value::Int(1)),
            ConstExpr::new(Value::UInt(4)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(16));
    }

    #[test]
    fn bin_expr_shr() {
        let expr = BinExpr::new(
            BinOp::Shr,
            ConstExpr::new(Value::Int(16)),
            ConstExpr::new(Value::UInt(4)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(1));
    }

    #[test]
    fn bin_expr_shl_exceeds_max_returns_error() {
        let expr = BinExpr::new(
            BinOp::Shl,
            ConstExpr::new(Value::Int(1)),
            ConstExpr::new(Value::UInt(100)),
        );
        let err = expr.evaluate(&Context::new(), None).unwrap_err();
        assert!(matches!(err, ConstructError::Expr { .. }));
        assert!(err.to_string().contains("exceeds maximum"));
    }

    #[test]
    fn bin_expr_bitand() {
        let expr = BinExpr::new(
            BinOp::BitAnd,
            ConstExpr::new(Value::Int(0xFF)),
            ConstExpr::new(Value::Int(0x0F)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(0x0F));
    }

    #[test]
    fn bin_expr_bitor() {
        let expr = BinExpr::new(
            BinOp::BitOr,
            ConstExpr::new(Value::Int(0xF0)),
            ConstExpr::new(Value::Int(0x0F)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(0xFF));
    }

    #[test]
    fn bin_expr_bitxor() {
        let expr = BinExpr::new(
            BinOp::BitXor,
            ConstExpr::new(Value::Int(0xFF)),
            ConstExpr::new(Value::Int(0x0F)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(0xF0));
    }

    #[test]
    fn bin_expr_bitwise_rejects_non_integer() {
        let expr = BinExpr::new(
            BinOp::BitAnd,
            ConstExpr::new(Value::Float(1.0)),
            ConstExpr::new(Value::Int(1)),
        );
        let err = expr.evaluate(&Context::new(), None).unwrap_err();
        assert!(matches!(err, ConstructError::Expr { .. }));
        assert!(err.to_string().contains("integer"));
    }

    // ======================================================================
    // BinExpr comparison tests
    // ======================================================================

    #[test]
    fn bin_expr_eq() {
        let expr = BinExpr::new(
            BinOp::Eq,
            ConstExpr::new(Value::Int(5)),
            ConstExpr::new(Value::Int(5)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn bin_expr_ne() {
        let expr = BinExpr::new(
            BinOp::Ne,
            ConstExpr::new(Value::Int(5)),
            ConstExpr::new(Value::Int(3)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn bin_expr_lt() {
        let expr = BinExpr::new(
            BinOp::Lt,
            ConstExpr::new(Value::Int(3)),
            ConstExpr::new(Value::Int(5)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn bin_expr_gt() {
        let expr = BinExpr::new(
            BinOp::Gt,
            ConstExpr::new(Value::Int(5)),
            ConstExpr::new(Value::Int(3)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn bin_expr_le() {
        let expr = BinExpr::new(
            BinOp::Le,
            ConstExpr::new(Value::Int(5)),
            ConstExpr::new(Value::Int(5)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn bin_expr_ge() {
        let expr = BinExpr::new(
            BinOp::Ge,
            ConstExpr::new(Value::Int(5)),
            ConstExpr::new(Value::Int(3)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn bin_expr_eq_different_types() {
        // Int != UInt even with same numeric value
        let expr = BinExpr::new(
            BinOp::Eq,
            ConstExpr::new(Value::Int(5)),
            ConstExpr::new(Value::UInt(5)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    // ======================================================================
    // BinExpr logical tests
    // ======================================================================

    #[test]
    fn bin_expr_and() {
        let expr = BinExpr::new(
            BinOp::And,
            ConstExpr::new(Value::Bool(true)),
            ConstExpr::new(Value::Bool(true)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn bin_expr_and_false() {
        let expr = BinExpr::new(
            BinOp::And,
            ConstExpr::new(Value::Bool(true)),
            ConstExpr::new(Value::Bool(false)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn bin_expr_or() {
        let expr = BinExpr::new(
            BinOp::Or,
            ConstExpr::new(Value::Bool(false)),
            ConstExpr::new(Value::Bool(true)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn bin_expr_or_both_false() {
        let expr = BinExpr::new(
            BinOp::Or,
            ConstExpr::new(Value::Bool(false)),
            ConstExpr::new(Value::Bool(false)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn bin_expr_and_with_truthy_int() {
        // Non-zero int is truthy
        let expr = BinExpr::new(
            BinOp::And,
            ConstExpr::new(Value::Int(1)),
            ConstExpr::new(Value::Int(2)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    // ======================================================================
    // UniExpr tests
    // ======================================================================

    #[test]
    fn uni_expr_neg_int() {
        let expr = UniExpr::new(UniOp::Neg, ConstExpr::new(Value::Int(42)));
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(-42));
    }

    #[test]
    fn uni_expr_neg_uint() {
        let expr = UniExpr::new(UniOp::Neg, ConstExpr::new(Value::UInt(42)));
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(-42));
    }

    #[test]
    fn uni_expr_neg_float() {
        let expr = UniExpr::new(UniOp::Neg, ConstExpr::new(Value::Float(2.5)));
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Float(-2.5));
    }

    #[test]
    fn uni_expr_neg_bigint() {
        let expr = UniExpr::new(UniOp::Neg, ConstExpr::new(Value::BigInt(100)));
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::BigInt(-100));
    }

    #[test]
    fn uni_expr_not_bool() {
        let expr = UniExpr::new(UniOp::Not, ConstExpr::new(Value::Bool(true)));
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn uni_expr_not_truthy_int() {
        let expr = UniExpr::new(UniOp::Not, ConstExpr::new(Value::Int(42)));
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn uni_expr_not_falsy_zero() {
        let expr = UniExpr::new(UniOp::Not, ConstExpr::new(Value::Int(0)));
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn uni_expr_bitnot_int() {
        let expr = UniExpr::new(UniOp::BitNot, ConstExpr::new(Value::Int(0)));
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(-1));
    }

    #[test]
    fn uni_expr_bitnot_uint() {
        let expr = UniExpr::new(UniOp::BitNot, ConstExpr::new(Value::UInt(0)));
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::UInt(u64::MAX));
    }

    #[test]
    fn uni_expr_neg_rejects_non_numeric() {
        let expr = UniExpr::new(UniOp::Neg, ConstExpr::new(Value::String("hi".to_string())));
        let err = expr.evaluate(&Context::new(), None).unwrap_err();
        assert!(matches!(err, ConstructError::Expr { .. }));
        assert!(err.to_string().contains("numeric"));
    }

    #[test]
    fn uni_expr_bitnot_rejects_non_integer() {
        let expr = UniExpr::new(UniOp::BitNot, ConstExpr::new(Value::Float(1.0)));
        let err = expr.evaluate(&Context::new(), None).unwrap_err();
        assert!(matches!(err, ConstructError::Expr { .. }));
        assert!(err.to_string().contains("integer"));
    }

    // ======================================================================
    // FuncPath tests
    // ======================================================================

    #[test]
    fn func_path_len_list() {
        let expr = FuncPath::new(
            "len",
            ConstExpr::new(Value::List(vec![
                Value::Int(1),
                Value::Int(2),
                Value::Int(3),
            ])),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::UInt(3));
    }

    #[test]
    fn func_path_len_bytes() {
        let expr = FuncPath::new("len", ConstExpr::new(Value::Bytes(vec![1, 2, 3, 4])));
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::UInt(4));
    }

    #[test]
    fn func_path_len_string() {
        let expr = FuncPath::new("len", ConstExpr::new(Value::String("hello".to_string())));
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::UInt(5));
    }

    #[test]
    fn func_path_len_container() {
        let mut map = IndexMap::new();
        map.insert("a".to_string(), Value::Int(1));
        map.insert("b".to_string(), Value::Int(2));
        let expr = FuncPath::new("len", ConstExpr::new(Value::Container(map)));
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::UInt(2));
    }

    #[test]
    fn func_path_len_rejects_non_collection() {
        let expr = FuncPath::new("len", ConstExpr::new(Value::Int(42)));
        let err = expr.evaluate(&Context::new(), None).unwrap_err();
        assert!(matches!(err, ConstructError::Expr { .. }));
        assert!(err.to_string().contains("len_()"));
    }

    #[test]
    fn func_path_sum() {
        let expr = FuncPath::new(
            "sum",
            ConstExpr::new(Value::List(vec![
                Value::Int(1),
                Value::Int(2),
                Value::Int(3),
            ])),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(6));
    }

    #[test]
    fn func_path_sum_empty_list() {
        let expr = FuncPath::new("sum", ConstExpr::new(Value::List(vec![])));
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(0));
    }

    #[test]
    fn func_path_sum_rejects_non_list() {
        let expr = FuncPath::new("sum", ConstExpr::new(Value::Int(42)));
        let err = expr.evaluate(&Context::new(), None).unwrap_err();
        assert!(matches!(err, ConstructError::Expr { .. }));
        assert!(err.to_string().contains("sum_()"));
    }

    #[test]
    fn func_path_min() {
        let expr = FuncPath::new(
            "min",
            ConstExpr::new(Value::List(vec![
                Value::Int(3),
                Value::Int(1),
                Value::Int(2),
            ])),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(1));
    }

    #[test]
    fn func_path_max() {
        let expr = FuncPath::new(
            "max",
            ConstExpr::new(Value::List(vec![
                Value::Int(3),
                Value::Int(1),
                Value::Int(2),
            ])),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(3));
    }

    #[test]
    fn func_path_min_empty_returns_error() {
        let expr = FuncPath::new("min", ConstExpr::new(Value::List(vec![])));
        let err = expr.evaluate(&Context::new(), None).unwrap_err();
        assert!(matches!(err, ConstructError::Expr { .. }));
        assert!(err.to_string().contains("non-empty"));
    }

    #[test]
    fn func_path_max_empty_returns_error() {
        let expr = FuncPath::new("max", ConstExpr::new(Value::List(vec![])));
        let err = expr.evaluate(&Context::new(), None).unwrap_err();
        assert!(matches!(err, ConstructError::Expr { .. }));
        assert!(err.to_string().contains("non-empty"));
    }

    #[test]
    fn func_path_abs_int() {
        let expr = FuncPath::new("abs", ConstExpr::new(Value::Int(-42)));
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(42));
    }

    #[test]
    fn func_path_abs_uint() {
        let expr = FuncPath::new("abs", ConstExpr::new(Value::UInt(42)));
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::UInt(42));
    }

    #[test]
    fn func_path_abs_float() {
        let expr = FuncPath::new("abs", ConstExpr::new(Value::Float(-2.5)));
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Float(2.5));
    }

    #[test]
    fn func_path_abs_bigint() {
        let expr = FuncPath::new("abs", ConstExpr::new(Value::BigInt(-100)));
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::BigInt(100));
    }

    #[test]
    fn func_path_abs_rejects_non_numeric() {
        let expr = FuncPath::new("abs", ConstExpr::new(Value::String("hi".to_string())));
        let err = expr.evaluate(&Context::new(), None).unwrap_err();
        assert!(matches!(err, ConstructError::Expr { .. }));
        assert!(err.to_string().contains("abs_()"));
    }

    #[test]
    fn func_path_unknown_function_returns_error() {
        let expr = FuncPath::new("unknown", ConstExpr::new(Value::Int(42)));
        let err = expr.evaluate(&Context::new(), None).unwrap_err();
        assert!(matches!(err, ConstructError::Expr { .. }));
        assert!(err.to_string().contains("unknown function"));
    }

    // ======================================================================
    // CondExpr tests
    // ======================================================================

    #[test]
    fn cond_expr_true_branch() {
        let expr = CondExpr::new(
            ConstExpr::new(Value::Bool(true)),
            ConstExpr::new(Value::Int(1)),
            ConstExpr::new(Value::Int(2)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(1));
    }

    #[test]
    fn cond_expr_false_branch() {
        let expr = CondExpr::new(
            ConstExpr::new(Value::Bool(false)),
            ConstExpr::new(Value::Int(1)),
            ConstExpr::new(Value::Int(2)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(2));
    }

    #[test]
    fn cond_expr_with_comparison() {
        let expr = CondExpr::new(
            BinExpr::new(
                BinOp::Gt,
                ConstExpr::new(Value::Int(5)),
                ConstExpr::new(Value::Int(3)),
            ),
            ConstExpr::new(Value::String("yes".to_string())),
            ConstExpr::new(Value::String("no".to_string())),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::String("yes".to_string()));
    }

    #[test]
    fn cond_expr_truthy_int() {
        let expr = CondExpr::new(
            ConstExpr::new(Value::Int(42)),
            ConstExpr::new(Value::String("truthy".to_string())),
            ConstExpr::new(Value::String("falsy".to_string())),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::String("truthy".to_string()));
    }

    #[test]
    fn cond_expr_falsy_zero() {
        let expr = CondExpr::new(
            ConstExpr::new(Value::Int(0)),
            ConstExpr::new(Value::String("truthy".to_string())),
            ConstExpr::new(Value::String("falsy".to_string())),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::String("falsy".to_string()));
    }

    // ======================================================================
    // IntoExpr tests
    // ======================================================================

    #[test]
    fn into_expr_usize() {
        let expr: Box<dyn Evaluate> = 10usize.into_expr();
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::UInt(10));
    }

    #[test]
    fn into_expr_i64() {
        let expr: Box<dyn Evaluate> = (-42i64).into_expr();
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(-42));
    }

    #[test]
    fn into_expr_u64() {
        let expr: Box<dyn Evaluate> = 100u64.into_expr();
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::UInt(100));
    }

    #[test]
    fn into_expr_path() {
        let mut ctx = Context::new();
        ctx.insert("x", Value::Int(7));
        let path = this_().field("x");
        let expr: Box<dyn Evaluate> = path.into_expr();
        let result = expr.evaluate(&ctx, None).unwrap();
        assert_eq!(result, Value::Int(7));
    }

    #[test]
    fn into_expr_boxed() {
        let boxed: Box<dyn Evaluate> = Box::new(ConstExpr::new(Value::Int(99)));
        let expr: Box<dyn Evaluate> = boxed.into_expr();
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(99));
    }

    // ======================================================================
    // Convenience constructor tests
    // ======================================================================

    #[test]
    fn this_creates_this_rooted_path() {
        let path = this_();
        assert_eq!(path.root, PathRoot::This);
        assert!(path.segments.is_empty());
    }

    #[test]
    fn obj_creates_obj_rooted_path() {
        let path = obj_();
        assert_eq!(path.root, PathRoot::Obj);
        assert!(path.segments.is_empty());
    }

    #[test]
    fn const_val_creates_const_expr() {
        let expr = const_val(Value::Int(42));
        assert_eq!(expr.value, Value::Int(42));
    }

    // ======================================================================
    // Path operator method tests
    // ======================================================================

    #[test]
    fn path_add_with_const() {
        let mut ctx = Context::new();
        ctx.insert("x", Value::Int(10));
        let expr = this_().field("x").add(ConstExpr::new(Value::Int(5)));
        let result = expr.evaluate(&ctx, None).unwrap();
        assert_eq!(result, Value::Int(15));
    }

    #[test]
    fn path_sub_with_const() {
        let mut ctx = Context::new();
        ctx.insert("x", Value::Int(10));
        let expr = this_().field("x").sub(ConstExpr::new(Value::Int(3)));
        let result = expr.evaluate(&ctx, None).unwrap();
        assert_eq!(result, Value::Int(7));
    }

    #[test]
    fn path_mul_with_const() {
        let mut ctx = Context::new();
        ctx.insert("x", Value::Int(6));
        let expr = this_().field("x").mul(ConstExpr::new(Value::Int(7)));
        let result = expr.evaluate(&ctx, None).unwrap();
        assert_eq!(result, Value::Int(42));
    }

    #[test]
    fn path_neg() {
        let mut ctx = Context::new();
        ctx.insert("x", Value::Int(42));
        let expr = this_().field("x").neg();
        let result = expr.evaluate(&ctx, None).unwrap();
        assert_eq!(result, Value::Int(-42));
    }

    #[test]
    fn path_not() {
        let mut ctx = Context::new();
        ctx.insert("x", Value::Bool(true));
        let expr = this_().field("x").not();
        let result = expr.evaluate(&ctx, None).unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn path_bitnot() {
        let mut ctx = Context::new();
        ctx.insert("x", Value::Int(0));
        let expr = this_().field("x").bitnot();
        let result = expr.evaluate(&ctx, None).unwrap();
        assert_eq!(result, Value::Int(-1));
    }

    #[test]
    fn path_len_func() {
        let mut ctx = Context::new();
        ctx.insert("items", Value::List(vec![Value::Int(1), Value::Int(2)]));
        let expr = this_().field("items").len_func();
        let result = expr.evaluate(&ctx, None).unwrap();
        assert_eq!(result, Value::UInt(2));
    }

    #[test]
    fn path_sum_func() {
        let mut ctx = Context::new();
        ctx.insert("items", Value::List(vec![Value::Int(10), Value::Int(20)]));
        let expr = this_().field("items").sum_func();
        let result = expr.evaluate(&ctx, None).unwrap();
        assert_eq!(result, Value::Int(30));
    }

    #[test]
    fn path_abs_func() {
        let mut ctx = Context::new();
        ctx.insert("x", Value::Int(-42));
        let expr = this_().field("x").abs_func();
        let result = expr.evaluate(&ctx, None).unwrap();
        assert_eq!(result, Value::Int(42));
    }

    #[test]
    fn path_eq_expr() {
        let mut ctx = Context::new();
        ctx.insert("x", Value::Int(5));
        let expr = this_().field("x").eq_expr(ConstExpr::new(Value::Int(5)));
        let result = expr.evaluate(&ctx, None).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn path_lt_expr() {
        let mut ctx = Context::new();
        ctx.insert("x", Value::Int(3));
        let expr = this_().field("x").lt(ConstExpr::new(Value::Int(5)));
        let result = expr.evaluate(&ctx, None).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    // ======================================================================
    // Complex expression tests
    // ======================================================================

    #[test]
    fn nested_arithmetic_expression() {
        // (this.a + this.b) * this.c
        let mut ctx = Context::new();
        ctx.insert("a", Value::Int(2));
        ctx.insert("b", Value::Int(3));
        ctx.insert("c", Value::Int(4));

        let expr = BinExpr::new(
            BinOp::Mul,
            BinExpr::new(BinOp::Add, this_().field("a"), this_().field("b")),
            this_().field("c"),
        );
        let result = expr.evaluate(&ctx, None).unwrap();
        assert_eq!(result, Value::Int(20)); // (2+3)*4 = 20
    }

    #[test]
    fn expression_with_obj_and_ctx() {
        // obj_.count + this.offset
        let mut ctx = Context::new();
        ctx.insert("offset", Value::Int(10));
        let obj = Value::Container(IndexMap::from([("count".to_string(), Value::Int(5))]));

        let expr = BinExpr::new(BinOp::Add, obj_().field("count"), this_().field("offset"));
        let result = expr.evaluate(&ctx, Some(&obj)).unwrap();
        assert_eq!(result, Value::Int(15));
    }

    #[test]
    fn complex_conditional_expression() {
        // if this.x > 10 then this.x * 2 else this.x + 10
        let mut ctx = Context::new();
        ctx.insert("x", Value::Int(5));

        let expr = CondExpr::new(
            BinExpr::new(
                BinOp::Gt,
                this_().field("x"),
                ConstExpr::new(Value::Int(10)),
            ),
            BinExpr::new(
                BinOp::Mul,
                this_().field("x"),
                ConstExpr::new(Value::Int(2)),
            ),
            BinExpr::new(
                BinOp::Add,
                this_().field("x"),
                ConstExpr::new(Value::Int(10)),
            ),
        );
        let result = expr.evaluate(&ctx, None).unwrap();
        assert_eq!(result, Value::Int(15)); // 5 <= 10, so 5 + 10 = 15
    }

    #[test]
    fn func_path_on_path_expression() {
        // len_(this.items) + 1
        let mut ctx = Context::new();
        ctx.insert(
            "items",
            Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
        );

        let expr = BinExpr::new(
            BinOp::Add,
            FuncPath::new("len", this_().field("items")),
            ConstExpr::new(Value::Int(1)),
        );
        let result = expr.evaluate(&ctx, None).unwrap();
        assert_eq!(result, Value::Int(4)); // 3 + 1 = 4
    }

    // ======================================================================
    // value_to_bool tests
    // ======================================================================

    #[test]
    fn value_to_bool_variants() {
        assert!(!value_to_bool(&Value::None).unwrap());
        assert!(value_to_bool(&Value::Bool(true)).unwrap());
        assert!(!value_to_bool(&Value::Bool(false)).unwrap());
        assert!(!value_to_bool(&Value::Int(0)).unwrap());
        assert!(value_to_bool(&Value::Int(1)).unwrap());
        assert!(!value_to_bool(&Value::UInt(0)).unwrap());
        assert!(value_to_bool(&Value::UInt(1)).unwrap());
        assert!(!value_to_bool(&Value::BigInt(0)).unwrap());
        assert!(value_to_bool(&Value::BigInt(1)).unwrap());
        assert!(!value_to_bool(&Value::Float(0.0)).unwrap());
        assert!(value_to_bool(&Value::Float(1.0)).unwrap());
        assert!(!value_to_bool(&Value::Bytes(vec![])).unwrap());
        assert!(value_to_bool(&Value::Bytes(vec![1])).unwrap());
        assert!(!value_to_bool(&Value::String(String::new())).unwrap());
        assert!(value_to_bool(&Value::String("a".to_string())).unwrap());
        assert!(!value_to_bool(&Value::List(vec![])).unwrap());
        assert!(value_to_bool(&Value::List(vec![Value::Int(1)])).unwrap());
        assert!(!value_to_bool(&Value::Container(IndexMap::new())).unwrap());
        let mut map = IndexMap::new();
        map.insert("a".to_string(), Value::Int(1));
        assert!(value_to_bool(&Value::Container(map)).unwrap());
    }

    // ======================================================================
    // Edge case tests
    // ======================================================================

    #[test]
    fn bin_expr_non_numeric_arithmetic_returns_error() {
        let expr = BinExpr::new(
            BinOp::Add,
            ConstExpr::new(Value::String("hi".to_string())),
            ConstExpr::new(Value::Int(1)),
        );
        let err = expr.evaluate(&Context::new(), None).unwrap_err();
        assert!(matches!(err, ConstructError::Expr { .. }));
    }

    #[test]
    fn bin_expr_div_float() {
        let expr = BinExpr::new(
            BinOp::Div,
            ConstExpr::new(Value::Float(7.0)),
            ConstExpr::new(Value::Float(2.0)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Float(3.5));
    }

    #[test]
    fn bin_expr_bigint_operations() {
        let expr = BinExpr::new(
            BinOp::Add,
            ConstExpr::new(Value::BigInt(1_000_000_000_000_i128)),
            ConstExpr::new(Value::BigInt(2_000_000_000_000_i128)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::BigInt(3_000_000_000_000_i128));
    }

    #[test]
    fn path_root_debug_and_clone() {
        let root = PathRoot::This;
        let cloned = root.clone();
        assert_eq!(root, cloned);
        let _ = format!("{root:?}");
    }

    #[test]
    fn path_debug_and_clone() {
        let path = this_().field("a").field("b");
        let cloned = path.clone();
        assert_eq!(path, cloned);
        let _ = format!("{path:?}");
    }

    #[test]
    fn const_expr_debug_and_clone() {
        let expr = ConstExpr::new(Value::Int(42));
        let cloned = expr.clone();
        assert_eq!(expr, cloned);
        let _ = format!("{expr:?}");
    }

    #[test]
    fn bin_op_debug_and_copy() {
        let op = BinOp::Add;
        let copied = op;
        assert_eq!(op, copied);
        let _ = format!("{op:?}");
    }

    #[test]
    fn uni_op_debug_and_copy() {
        let op = UniOp::Neg;
        let copied = op;
        assert_eq!(op, copied);
        let _ = format!("{op:?}");
    }

    // ======================================================================
    // Float comparison tests (bug fix verification)
    // ======================================================================

    #[test]
    fn bin_expr_float_lt_correct() {
        // Float(3.0) < Float(3.5) should be true
        let expr = BinExpr::new(
            BinOp::Lt,
            ConstExpr::new(Value::Float(3.0)),
            ConstExpr::new(Value::Float(3.5)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn bin_expr_float_gt_correct() {
        // Float(0.5) > Float(0.3) should be true
        let expr = BinExpr::new(
            BinOp::Gt,
            ConstExpr::new(Value::Float(0.5)),
            ConstExpr::new(Value::Float(0.3)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn bin_expr_float_eq_correct() {
        // Float(3.0) == Float(3.0) should be true
        let expr = BinExpr::new(
            BinOp::Eq,
            ConstExpr::new(Value::Float(3.0)),
            ConstExpr::new(Value::Float(3.0)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn bin_expr_float_ne_correct() {
        // Float(3.0) != Float(3.5) should be true
        let expr = BinExpr::new(
            BinOp::Ne,
            ConstExpr::new(Value::Float(3.0)),
            ConstExpr::new(Value::Float(3.5)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn bin_expr_float_le_correct() {
        // Float(3.0) <= Float(3.5) should be true
        let expr = BinExpr::new(
            BinOp::Le,
            ConstExpr::new(Value::Float(3.0)),
            ConstExpr::new(Value::Float(3.5)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn bin_expr_float_ge_correct() {
        // Float(3.5) >= Float(3.0) should be true
        let expr = BinExpr::new(
            BinOp::Ge,
            ConstExpr::new(Value::Float(3.5)),
            ConstExpr::new(Value::Float(3.0)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn bin_expr_mixed_int_float_comparison() {
        // Int(3) < Float(3.5) should be true
        let expr = BinExpr::new(
            BinOp::Lt,
            ConstExpr::new(Value::Int(3)),
            ConstExpr::new(Value::Float(3.5)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn bin_expr_float_comparison_not_truncated() {
        // Float(0.9) > Float(0.1) should be true
        // Previously this would be truncated: 0.9 as i64 = 0, 0.1 as i64 = 0
        // giving false instead of true
        let expr = BinExpr::new(
            BinOp::Gt,
            ConstExpr::new(Value::Float(0.9)),
            ConstExpr::new(Value::Float(0.1)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    // ======================================================================
    // Arithmetic overflow tests (wrapping semantics verification)
    // ======================================================================

    #[test]
    fn bin_expr_add_overflow_wraps() {
        // i64::MAX + 1 should wrap (not panic)
        let expr = BinExpr::new(
            BinOp::Add,
            ConstExpr::new(Value::Int(i64::MAX)),
            ConstExpr::new(Value::Int(1)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(i64::MIN)); // wrapping
    }

    #[test]
    fn bin_expr_sub_overflow_wraps() {
        // i64::MIN - 1 should wrap (not panic)
        let expr = BinExpr::new(
            BinOp::Sub,
            ConstExpr::new(Value::Int(i64::MIN)),
            ConstExpr::new(Value::Int(1)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(i64::MAX)); // wrapping
    }

    #[test]
    fn bin_expr_mul_overflow_wraps() {
        // i64::MAX * 2 should wrap (not panic)
        let expr = BinExpr::new(
            BinOp::Mul,
            ConstExpr::new(Value::Int(i64::MAX)),
            ConstExpr::new(Value::Int(2)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        // wrapping_mul: i64::MAX * 2 = -2
        assert_eq!(result, Value::Int(-2));
    }

    #[test]
    fn bin_expr_uint_add_overflow_wraps() {
        let expr = BinExpr::new(
            BinOp::Add,
            ConstExpr::new(Value::UInt(u64::MAX)),
            ConstExpr::new(Value::UInt(1)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::UInt(0)); // wrapping
    }

    #[test]
    fn bin_expr_uint_mul_overflow_wraps() {
        let expr = BinExpr::new(
            BinOp::Mul,
            ConstExpr::new(Value::UInt(u64::MAX)),
            ConstExpr::new(Value::UInt(2)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        // wrapping_mul: u64::MAX * 2 = u64::MAX - 1
        assert_eq!(result, Value::UInt(u64::MAX - 1));
    }

    #[test]
    fn bin_expr_bigint_add_overflow_wraps() {
        // BigInt wrapping at i128 boundary
        let expr = BinExpr::new(
            BinOp::Add,
            ConstExpr::new(Value::BigInt(i128::MAX)),
            ConstExpr::new(Value::BigInt(1)),
        );
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::BigInt(i128::MIN)); // wrapping
    }

    // ======================================================================
    // ListPath (list_) tests
    // ======================================================================

    #[test]
    fn list_path_bare_with_positive_index() {
        let mut ctx = Context::new();
        ctx.insert(
            "list_",
            Value::List(vec![Value::Int(10), Value::Int(20), Value::Int(30)]),
        );
        let expr = list_().index(0);
        let result = expr.evaluate(&ctx, None).unwrap();
        assert_eq!(result, Value::Int(10));
    }

    #[test]
    fn list_path_bare_with_negative_index() {
        // list_[-1] → last element
        let mut ctx = Context::new();
        ctx.insert(
            "list_",
            Value::List(vec![Value::Int(10), Value::Int(20), Value::Int(30)]),
        );
        let expr = list_().index(-1);
        let result = expr.evaluate(&ctx, None).unwrap();
        assert_eq!(result, Value::Int(30));
    }

    #[test]
    fn list_path_bare_with_negative_two() {
        // list_[-2] → second to last
        let mut ctx = Context::new();
        ctx.insert(
            "list_",
            Value::List(vec![Value::Int(10), Value::Int(20), Value::Int(30)]),
        );
        let expr = list_().index(-2);
        let result = expr.evaluate(&ctx, None).unwrap();
        assert_eq!(result, Value::Int(20));
    }

    #[test]
    fn list_path_indexed_with_inner() {
        // ListPath::new_indexed(-1, ConstExpr(list)) → last element
        let list_val = Value::List(vec![Value::Int(10), Value::Int(20), Value::Int(30)]);
        let expr = ListPath::new_indexed(-1, ConstExpr::new(list_val));
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(30));
    }

    #[test]
    fn list_path_indexed_first_element() {
        let list_val = Value::List(vec![Value::Int(42), Value::Int(99)]);
        let expr = ListPath::new_indexed(0, ConstExpr::new(list_val));
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(42));
    }

    #[test]
    fn list_path_out_of_bounds_returns_error() {
        let mut ctx = Context::new();
        ctx.insert("list_", Value::List(vec![Value::Int(1)]));
        let expr = list_().index(5);
        let err = expr.evaluate(&ctx, None).unwrap_err();
        assert!(matches!(err, ConstructError::Expr { .. }));
        assert!(err.to_string().contains("out of range"));
    }

    #[test]
    fn list_path_negative_out_of_bounds_returns_error() {
        let mut ctx = Context::new();
        ctx.insert("list_", Value::List(vec![Value::Int(1)]));
        let expr = list_().index(-5);
        let err = expr.evaluate(&ctx, None).unwrap_err();
        assert!(matches!(err, ConstructError::Expr { .. }));
        assert!(err.to_string().contains("out of range"));
    }

    #[test]
    fn list_path_empty_list_returns_error() {
        let mut ctx = Context::new();
        ctx.insert("list_", Value::List(vec![]));
        let expr = list_().index(0);
        let err = expr.evaluate(&ctx, None).unwrap_err();
        assert!(matches!(err, ConstructError::Expr { .. }));
        assert!(err.to_string().contains("out of range"));
    }

    #[test]
    fn list_path_non_list_returns_error() {
        let mut ctx = Context::new();
        ctx.insert("list_", Value::Int(42));
        let expr = list_().index(0);
        let err = expr.evaluate(&ctx, None).unwrap_err();
        assert!(matches!(err, ConstructError::Expr { .. }));
        assert!(err.to_string().contains("List"));
    }

    #[test]
    fn list_path_no_context_key_returns_error() {
        let ctx = Context::new();
        let expr = list_().index(0);
        let err = expr.evaluate(&ctx, None).unwrap_err();
        assert!(matches!(err, ConstructError::Expr { .. }));
        assert!(err.to_string().contains("list_"));
    }

    #[test]
    fn list_path_in_comparison_expression() {
        // list_[-1] == 255 — the RepeatUntil pattern
        let mut ctx = Context::new();
        ctx.insert(
            "list_",
            Value::List(vec![Value::Int(10), Value::Int(20), Value::Int(255)]),
        );
        let expr = BinExpr::new(
            BinOp::Eq,
            list_().index(-1),
            ConstExpr::new(Value::Int(255)),
        );
        let result = expr.evaluate(&ctx, None).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn list_path_into_expr() {
        let list_val = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        let expr: Box<dyn Evaluate> =
            ListPath::new_indexed(-1, ConstExpr::new(list_val)).into_expr();
        let result = expr.evaluate(&Context::new(), None).unwrap();
        assert_eq!(result, Value::Int(3));
    }
}
