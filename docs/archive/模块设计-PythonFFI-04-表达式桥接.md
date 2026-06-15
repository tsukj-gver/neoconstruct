# 模块设计：PythonFFI-04 表达式系统 Python 桥接

## 模块位置

- `construct-py/construct_rust/expr.py` — 纯 Python 表达式系统（Path/Path2/FuncPath/UniExpr/BinExpr）
- `construct-py/src/expr_bridge.rs` — PyClosure 及函数类型桥接器（Rust）
- `construct-py/src/py_adapter.rs` — PyConstructAdapter 混合架构桥接器（Rust）
- `construct-py/src/param.rs` — 构造器参数双模式 helper（Rust）

## 职责

将 Python 表达式系统（`this`/`obj_`/`list_`/`len_`/`sum_`/`min_`/`max_`/`abs_` + 运算符重载）和 Python 用户自定义构造器（Adapter/Validator 子类）桥接到 Rust 内核的 `Evaluate` trait、各函数类型别名（`CondFunc`/`KeyFunc`/`ComputeFunc`/`RepeatPredicate`）和 `Construct` trait。

---

## 1. 核心设计决策

### 1.1 纯 Python 表达式系统（与 10.3 Container 策略一致）

**决策**：Python 表达式系统（`expr.py`，256 行）作为**纯 Python 模块**移植到 `construct-py/construct_rust/expr.py`，不从 Rust 端暴露。

**理由**：

1. **运算符重载是 Python 专属协议**：`this.x + 1` 依赖 Python `__add__`/`__radd__` 协议链。PyO3 `#[pyclass]` 虽然可以实现 `__add__` 等 dunder 方法，但 `ExprMixin` 的运算符重载涉及 28 个 dunder 方法（14 个二元 + 14 个反射），且需返回新的 Python 表达式对象（`BinExpr`/`UniExpr`），这些对象本身又需要运算符重载。用 PyO3 实现此链式协议的复杂度极高且收益为零。

2. **表达式对象是 Python callable**：Python 表达式（Path/BinExpr/lambda）通过 `__call__` 协议被 `evaluate(param, context)` 调用。纯 Python 对象天然支持 callable 协议。PyO3 对象的 `__call__` 需要额外实现且每次调用都穿越 FFI 边界。

3. **与 10.3 Container 一致**：Container 和表达式系统都是 Python 专属机制的集合，放入 `construct_rust/` 纯 Python 包中，架构一致。

4. **零行为差异**：逐行移植原版 `expr.py`，所有 28 个运算符、`__call__`/`__repr__`/`__str__`/`__getstate__`/`__setstate__` 方法与原版完全一致。

### 1.2 Rust 函数类型 → Python callable 桥接策略

Rust 内核使用多种闭包类型别名接收表达式参数：

| Rust 类型别名 | 签名 | 使用者 | Python 调用约定 |
|--------------|------|--------|----------------|
| `Box<dyn Evaluate>` | `Fn(&Context, Option<&Value>) -> Result<Value>` | Array count, Bytes length, Seek at, Pointer offset | `expr(context)` — 1 arg |
| `CondFunc` | `Fn(&Context) -> bool` | IfThenElse, StopIf | `expr(context)` → bool |
| `KeyFunc` | `Fn(&Context) -> Result<Value>` | Switch | `expr(context)` → Value |
| `CheckFunc` | `Fn(&Context) -> Result<()>` | Check | `expr(context)` → pass/raise |
| `ComputeFunc` | `Fn(&Context) -> Result<Value>` | Computed, Rebuild | `expr(context)` → Value |
| `RepeatPredicate` | `Fn(&Value, &[Value], &Context) -> bool` | RepeatUntil | `expr(element, list, context)` — 3 args |

**关键发现**：除 `RepeatPredicate` 外，所有函数类型都以 `(context)` 单参数调用 Python 表达式。`RepeatPredicate` 使用 `(element, list, context)` 三参数调用约定（对应 Python `RepeatUntil` 的 `predicate(e, obj, context)` 调用）。

**桥接方案**：设计通用 `PyClosure` 结构体持有 Python callable，通过统一的 `call_single`（1-arg）和 `call_triple`（3-arg）方法，为每种 Rust 函数类型生成对应的桥接实现。

### 1.3 PyConstructAdapter 混合架构

**决策**：设计 `PyConstructAdapter` 实现 Rust `Construct` trait，将任意 Python 构造器对象（用户自定义 Adapter/Validator 子类）包装为 Rust 可调用的 `Box<dyn Construct>`。

**架构概览**：

```
用户 Python 代码:
    class MyAdapter(Adapter):
        def _decode(self, obj, context, path): ...

    d = Struct("field" / MyAdapter(Byte[4]))
                     ^^^^^^^^^^^^^^^^^^^^^^^^
                     纯 Python 对象

Rust Struct 接收 subcons 时:
    检测每个 subcon 类型:
    ├── PyO3 #[pyclass] 对象 → 直接使用 Box<dyn Construct>
    └── 纯 Python 对象 → 包装为 PyConstructAdapter → Box<dyn Construct>

执行时:
    Rust Struct.parse() → PyConstructAdapter.parse()
        → Python::with_gil → python_obj.parse_stream(py_stream, **kw)
        → Python Adapter._parse() → self.subcon._parsereport()
            → 可能回调 Rust subcon 的 PyO3 wrapper
```

---

## 2. 纯 Python 表达式系统

### 2.1 文件位置

`construct-py/construct_rust/expr.py`

### 2.2 设计方案：逐行移植原版 `construct/construct/expr.py`

原版 `expr.py` 共 256 行，无外部依赖（仅用 `operator` 标准库）。逐行移植，不做任何行为修改。

### 2.3 模块结构

```python
import operator
if not hasattr(operator, "div"):
    operator.div = operator.truediv

# 运算符 → 符号映射表（用于 __repr__/__str__）
opnames = { ... }  # 24 个运算符映射，与原版完全一致

class ExprMixin(object):
    """所有表达式类的运算符重载 mixin。"""
    # 28 个 dunder 方法（14 二元 + 14 反射）
    # 3 个一元运算符
    # 6 个比较运算符
    # __contains__（已知 xfail bug，保留原版行为）
    # __getstate__/__setstate__（pickle 支持）

class UniExpr(ExprMixin):
    """一元表达式：op(operand)。"""
    # __init__(op, operand)
    # __repr__/__str__
    # __call__(obj, *args) — 对 operand 求值后应用 op

class BinExpr(ExprMixin):
    """二元表达式：op(lhs, rhs)。"""
    # __init__(op, lhs, rhs)
    # __repr__/__str__
    # __call__(obj, *args) — 分别对 lhs/rhs 求值后应用 op

class Path(ExprMixin):
    """属性/下标路径表达式（this/obj_）。"""
    # __init__(name, field=None, parent=None)
    # __repr__/__str__
    # __call__(obj, *args) — 从 obj 开始逐级索引
    # __getattr__(name) → 返回新 Path（链式访问）
    # __getitem__(name) → 同 __getattr__

class Path2(ExprMixin):
    """列表索引路径表达式（list_）。"""
    # __init__(name, index=None, parent=None)
    # __repr__/__str__
    # __call__(*args) — 从 args[1]（列表）开始索引
    # __getitem__(index) → 返回新 Path2

class FuncPath(ExprMixin):
    """内置函数路径表达式（len_/sum_/min_/max_/abs_）。"""
    # __init__(func, operand=None)
    # __repr__/__str__
    # __call__(operand, *args) — 柯里化或应用

# 全局实例
this = Path("this")
obj_ = Path("obj_")
list_ = Path2("list_")

len_ = FuncPath(len)
sum_ = FuncPath(sum)
min_ = FuncPath(min)
max_ = FuncPath(max)
abs_ = FuncPath(abs)
```

### 2.4 运算符重载语义

#### 二元运算符（14 个正向 + 14 个反射）

| Python 运算符 | `__`method`__` | 反射 `__r`method`__` | 产生的类型 |
|--------------|----------------|---------------------|-----------|
| `a + b` | `__add__` | `__radd__` | `BinExpr(operator.add, ...)` |
| `a - b` | `__sub__` | `__rsub__` | `BinExpr(operator.sub, ...)` |
| `a * b` | `__mul__` | `__rmul__` | `BinExpr(operator.mul, ...)` |
| `a // b` | `__floordiv__` | `__rfloordiv__` | `BinExpr(operator.floordiv, ...)` |
| `a / b` | `__truediv__` | `__rtruediv__` | `BinExpr(operator.div, ...)` |
| `a % b` | `__mod__` | `__rmod__` | `BinExpr(operator.mod, ...)` |
| `a ** b` | `__pow__` | `__rpow__` | `BinExpr(operator.pow, ...)` |
| `a ^ b` | `__xor__` | `__rxor__` | `BinExpr(operator.xor, ...)` |
| `a >> b` | `__rshift__` | `__rrshift__` | `BinExpr(operator.rshift, ...)` |
| `a << b` | `__lshift__` | `__rlshift__` | `BinExpr(operator.lshift, ...)` |
| `a & b` | `__and__` | `__rand__` | `BinExpr(operator.and_, ...)` |
| `a \| b` | `__or__` | `__ror__` | `BinExpr(operator.or_, ...)` |

**反射运算符语义**：当左操作数不支持运算时（如 `5 + this.x` 中 `5` 是 int），Python 调用右操作数的 `__radd__`。此时 BinExpr 的参数顺序反转：`BinExpr(operator.add, other, self)`。

#### 一元运算符（3 个）

| Python 运算符 | 方法 | 产生的类型 |
|--------------|------|-----------|
| `-a` | `__neg__` | `UniExpr(operator.neg, self)` |
| `+a` | `__pos__` | `UniExpr(operator.pos, self)` |
| `~a` | `__invert__` / `__inv__` | `UniExpr(operator.not_, self)` |

**注意**：`~a` 映射到 `operator.not_`（逻辑非），不是按位取反。这是原版行为。

#### 比较运算符（6 个）

| Python 运算符 | 方法 | 产生的类型 |
|--------------|------|-----------|
| `a > b` | `__gt__` | `BinExpr(operator.gt, self, other)` |
| `a >= b` | `__ge__` | `BinExpr(operator.ge, self, other)` |
| `a < b` | `__lt__` | `BinExpr(operator.lt, self, other)` |
| `a <= b` | `__le__` | `BinExpr(operator.le, self, other)` |
| `a == b` | `__eq__` | `BinExpr(operator.eq, self, other)` |
| `a != b` | `__ne__` | `BinExpr(operator.ne, self, other)` |

**重要**：比较运算符返回 `BinExpr` 对象（延迟求值），不返回 `bool`。这使得 `this.x == 0` 创建一个表达式对象而非立即比较。Python 的 `__eq__` 重载会改变对象在 dict 中的哈希行为，但表达式对象不做 hash（原版未实现 `__hash__`，Python 3 中 `__eq__` 定义后 `__hash__` 自动设为 `None`，表达式对象不可哈希）。

#### 包含运算符（`in`）— 已知 xfail

```python
def __contains__(self, other):
    return BinExpr(operator.contains, self, other)
```

`this.data in [1,2,3]` 在 Python 中会**先调用右操作数** `[1,2,3].__contains__(this.data)`，由于 Path 对象不在列表中，直接返回 `False`，不会创建 BinExpr。这是原版已知 bug（`test_this_in_operator` 标记 `@xfail`）。lambda 版本 `lambda ctx: ctx.data in [1,2,3]` 可正常工作（`test_lambda_in_operator`）。

### 2.5 Path 调用语义

`Path.__call__(obj, *args)` 的行为取决于是否有 parent：

| 场景 | parent | 调用 | 结果 |
|------|--------|------|------|
| 裸 `this` | None | `this(context)` | 返回 `context` 本身 |
| `this.x` | Path("this") | `this.x(context)` | `this(context)["x"]` = `context["x"]` |
| `this.x.y` | Path("this","x") | `this.x.y(context)` | `this.x(context)["y"]` |
| 裸 `obj_` | None | `obj_(element, list, ctx)` | 返回 `element`（第一个参数） |
| `obj_` in RepeatUntil | None | `predicate(e, list, ctx)` | 返回 `e` |

**关键**：Path 的 `__call__` 只使用第一个位置参数。多出的参数被 `*args` 捕获并忽略。这意味着：
- `this.x(context)` → `context["x"]` ✓
- `this.x(element, list, context)` → `element["x"]` （语义变化！但 RepeatUntil 中 `this` 较少使用）
- `obj_(element, list, context)` → `element` ✓

### 2.6 Path2（list_）调用语义

`Path2.__call__(*args)` 的行为：

| 场景 | parent | 调用 | 结果 |
|------|--------|------|------|
| 裸 `list_` | None | `list_(e, list, ctx)` | 返回 `args[1]` = `list` |
| `list_[-1]` | Path2("list_") | `list_[-1](e, list, ctx)` | `list_(e, list, ctx)[-1]` = `list[-1]` |

**关键**：Path2 使用 `args[1]`（第二个参数）作为列表源。这是 `list_` 与 `this`/`obj_` 的本质区别。

### 2.7 FuncPath 调用语义

`FuncPath.__call__(operand, *args)` 的行为取决于是否已绑定 operand：

| 场景 | `__operand` | 调用 | 结果 |
|------|-------------|------|------|
| `len_` 裸调用 | None | `len_(this.items)` | 返回新 `FuncPath(len, this.items)`（柯里化） |
| `len_(this.items)` 已绑定 | Some(expr) | `len_(this.items)(context)` | `len(this.items(context))` |

**柯里化机制**：`len_` 本身是 `FuncPath(len, None)`。调用 `len_(this.items)` 时，`this.items` 是 callable → 返回新的 `FuncPath(len, this.items)`（延迟求值）。调用 `len_(5)` 时，`5` 不是 callable → 直接返回 `5`（透传非 callable 参数）。

### 2.8 evaluate() 函数

```python
def evaluate(param, context):
    return param(context) if callable(param) else param
```

此函数在 Python 侧提供，供纯 Python 构造器（如 Adapter 基类）使用。Rust 内核构造器通过各自的函数类型别名（CondFunc/KeyFunc 等）直接求值，不经过此函数。

### 2.9 导出接口

在 `construct_rust/__init__.py`（或 `construct_rust/_core` 的 PyO3 模块注册）中导出：

```python
from .expr import (
    this, obj_, list_,
    len_, sum_, min_, max_, abs_,
    ExprMixin, UniExpr, BinExpr, Path, Path2, FuncPath,
    opnames,
)
```

同时在 `__all__` 中包含以上名称，使 `from construct import *` 能正确导入。

---

## 3. PyClosure 桥接器

### 3.1 文件位置

`construct-py/src/expr_bridge.rs`

### 3.2 核心结构

```rust
use pyo3::prelude::*;
use pyo3::types::PyTuple;
use construct::expr::{Evaluate, ConstExpr};
use construct::core::context::Context;
use construct::core::error::{ConstructError, Result};
use construct::value::Value;

/// 包装任意 Python callable，使其可在 Rust 内核中被求值。
///
/// Python 表达式（Path/BinExpr/FuncPath/lambda）均为 callable 对象。
/// PyClosure 持有其引用，在 evaluate 时获取 GIL、转换 Context → Container、
/// 调用 Python callable、转换返回值 → Value。
pub struct PyClosure {
    /// Python callable 对象（Path, BinExpr, FuncPath, lambda 等）
    callable: PyObject,
}

impl PyClosure {
    /// 从任意 Python 对象创建 PyClosure。
    pub fn new(callable: PyObject) -> Self {
        PyClosure { callable }
    }

    /// 单参数调用：`callable(context)` — 标准求值约定。
    ///
    /// 用于 Box<dyn Evaluate>, CondFunc, KeyFunc, CheckFunc, ComputeFunc。
    fn call_single(&self, py: Python<'_>, py_ctx: &PyAny) -> PyResult<PyObject> {
        self.callable.call1(py, (py_ctx,))
    }

    /// 三参数调用：`callable(element, list, context)` — RepeatUntil 约定。
    ///
    /// 用于 RepeatPredicate。
    fn call_triple(
        &self,
        py: Python<'_>,
        py_element: &PyAny,
        py_list: &PyAny,
        py_ctx: &PyAny,
    ) -> PyResult<PyObject> {
        self.callable
            .call1(py, (py_element, py_list, py_ctx))
    }
}
```

### 3.3 Evaluate trait 实现

```rust
impl Evaluate for PyClosure {
    fn evaluate(&self, ctx: &Context, _obj: Option<&Value>) -> Result<Value> {
        Python::with_gil(|py| {
            // 1. Context → Python Container（使用 10.2 的 context_to_py_container）
            let py_ctx = crate::conversions::context_to_py_container(py, ctx)
                .map_err(|e| ConstructError::Expr {
                    path: String::new(),
                    message: format!("Context conversion failed: {}", e),
                })?;
            let py_ctx_ref = py_ctx.bind(py);

            // 2. 调用 Python callable（单参数约定）
            let result = self.call_single(py, py_ctx_ref).map_err(|e| {
                ConstructError::Expr {
                    path: String::new(),
                    message: format!("Python expression error: {}", e),
                }
            })?;

            // 3. 返回值 → Value（使用 10.2 的 py_to_value）
            crate::conversions::py_to_value(py, result.bind(py)).map_err(|e| {
                ConstructError::Expr {
                    path: String::new(),
                    message: format!("Expression result conversion failed: {}", e),
                }
            })
        })
    }
}
```

**`_obj` 参数被忽略的原因**：Rust 内核中 `Box<dyn Evaluate>` 的调用者（Array/Bytes/Seek/Pointer）始终传 `None` 作为 `obj`（参见 `repetition.rs:231`、`bytes.rs:198` 等）。Python 表达式通过单参数 `(context)` 调用，不需要 `obj`。

### 3.4 GIL 安全性

- `Python::with_gil` 在已持有 GIL 时安全重入（PyO3 保证）
- Rust 内核解析过程中不会释放 GIL（Phase 10 优先正确性，GIL 阻塞风险在总纲已记录）
- 表达式求值频率：每次遇到表达式参数的构造器（如 Array count）时调用一次，通常不在热循环内（Array 的 count 在循环外求值一次）

---

## 4. 函数类型桥接器

### 4.1 设计方案

Rust 内核的 5 种函数类型别名（`CondFunc`/`KeyFunc`/`CheckFunc`/`ComputeFunc`/`RepeatPredicate`）需要各自的 Python callable 桥接。设计统一的 `PyClosure` + 类型转换适配器模式。

### 4.2 CondFunc 桥接（IfThenElse / StopIf）

```rust
/// 将 PyClosure 适配为 CondFunc（Fn(&Context) -> bool）。
pub fn py_to_cond_func(callable: PyObject) -> Box<dyn Fn(&Context) -> bool> {
    Box::new(move |ctx| {
        Python::with_gil(|py| {
            let py_ctx = match crate::conversions::context_to_py_container(py, ctx) {
                Ok(c) => c,
                Err(_) => return false,
            };
            match callable.call1(py, (py_ctx.bind(py),)) {
                Ok(result) => result.is_truthy(py).unwrap_or(false),
                Err(_) => false,
            }
        })
    })
}
```

**Python `in` 运算符在 CondFunc 中的行为**：

- `If(this.x > 0, ...)` — `this.x > 0` 是 BinExpr，callable → 求值后返回 Python bool
- `If(lambda ctx: ctx.x in [1,2,3], ...)` — lambda 返回 Python bool
- `If(this.x in [1,2,3], ...)` — xfail（`in` 不创建表达式，直接返回 False）

### 4.3 KeyFunc 桥接（Switch）

```rust
/// 将 PyClosure 适配为 KeyFunc（Fn(&Context) -> Result<Value>）。
pub fn py_to_key_func(callable: PyObject) -> Box<dyn Fn(&Context) -> Result<Value>> {
    Box::new(move |ctx| {
        Python::with_gil(|py| {
            let py_ctx = crate::conversions::context_to_py_container(py, ctx)
                .map_err(|e| ConstructError::Expr {
                    path: String::new(),
                    message: format!("Context conversion: {}", e),
                })?;
            let result = callable.call1(py, (py_ctx.bind(py),)).map_err(|e| {
                ConstructError::Expr {
                    path: String::new(),
                    message: format!("Switch keyfunc error: {}", e),
                }
            })?;
            crate::conversions::py_to_value(py, result.bind(py)).map_err(|e| {
                ConstructError::Expr {
                    path: String::new(),
                    message: format!("Key conversion: {}", e),
                }
            })
        })
    })
}
```

**Switch key 匹配**：Rust Switch 使用 `Value::PartialEq` 进行线性搜索（`control_flow.rs:261`）。Python 端 `this.type` 返回 int → Value::Int/UInt → 与 cases 中的 Value 比较。

### 4.4 CheckFunc 桥接（Check）

```rust
/// 将 PyClosure 适配为 CheckFunc（Fn(&Context) -> Result<()>）。
pub fn py_to_check_func(callable: PyObject) -> Box<dyn Fn(&Context) -> Result<()>> {
    Box::new(move |ctx| {
        Python::with_gil(|py| {
            let py_ctx = crate::conversions::context_to_py_container(py, ctx)
                .map_err(|e| ConstructError::Expr {
                    path: String::new(),
                    message: format!("Context conversion: {}", e),
                })?;
            let result = callable.call1(py, (py_ctx.bind(py),)).map_err(|e| {
                ConstructError::Expr {
                    path: String::new(),
                    message: format!("Check func error: {}", e),
                }
            })?;
            // Python truthy → Ok(()), falsy → Err(CheckError)
            if result.is_truthy(py).unwrap_or(false) {
                Ok(())
            } else {
                Err(ConstructError::Check {
                    path: String::new(),
                    message: "Check failed".to_string(),
                })
            }
        })
    })
}
```

**Check 语义**：Python `Check(this.x == 0)` 中 `this.x == 0` 创建 BinExpr，求值后返回 Python bool。`True` → pass，`False` → `ConstructError::Check`。

### 4.5 ComputeFunc 桥接（Computed / Rebuild）

```rust
/// 将 PyClosure 适配为 ComputeFunc（Fn(&Context) -> Result<Value>）。
pub fn py_to_compute_func(callable: PyObject) -> Box<dyn Fn(&Context) -> Result<Value>> {
    // 与 KeyFunc 桥接逻辑相同，复用代码
    py_to_key_func(callable)
}
```

**Computed 语义**：Python `Computed(this.x * 2)` 中 `this.x * 2` 是 BinExpr，求值后返回计算值。

### 4.6 RepeatPredicate 桥接（RepeatUntil）

```rust
/// 将 PyClosure 适配为 RepeatPredicate（Fn(&Value, &[Value], &Context) -> bool）。
///
/// 使用三参数调用约定：callable(element, list, context)。
pub fn py_to_repeat_predicate(callable: PyObject) -> RepeatPredicate {
    Box::new(move |element, list, ctx| {
        Python::with_gil(|py| {
            // 1. 转换三个参数
            let py_element = match crate::conversions::value_to_py(py, element) {
                Ok(v) => v,
                Err(_) => return false,
            };
            let py_list = match crate::conversions::value_to_py(
                py,
                &Value::List(list.to_vec()),
            ) {
                Ok(v) => v,
                Err(_) => return false,
            };
            let py_ctx = match crate::conversions::context_to_py_container(py, ctx) {
                Ok(c) => c,
                Err(_) => return false,
            };

            // 2. 三参数调用
            let result = match callable.call1(
                py,
                (py_element.bind(py), py_list.bind(py), py_ctx.bind(py)),
            ) {
                Ok(r) => r,
                Err(_) => return false,
            };

            // 3. 结果 → bool
            result.is_truthy(py).unwrap_or(false)
        })
    })
}
```

**三参数调用约定对应**：

| Python 表达式 | `__call__` 签名 | 第一个 arg | 第二个 arg | 行为 |
|--------------|----------------|-----------|-----------|------|
| `obj_ == 255` | `BinExpr.__call__(e, list, ctx)` | `e` (element) | 忽略 | `e == 255` |
| `list_[-1] == 255` | `BinExpr.__call__(e, list, ctx)` | `e` (element) | `list` | `list[-1] == 255` |
| `lambda x,l,c: x > 7` | lambda `(x, l, c)` | `x` (element) | `l` (list) | `x > 7` |

**注意**：`list_[-1] == 255` 中，`list_[-1]` 是 Path2，其 `__call__(e, list, ctx)` 返回 `list[-1]`。然后 `BinExpr.__call__(e, list, ctx)` 对 lhs（`list_[-1]`）调用 `(list_[-1])(e, list, ctx)` → `list[-1]`，对 rhs（`255`）返回 `255`，最后 `operator.eq(list[-1], 255)`。✓

---

## 5. 构造器参数双模式

### 5.1 文件位置

`construct-py/src/param.rs`

### 5.2 设计方案

当 Python 用户调用 `Bytes(this.length)` 或 `Array(5, Byte)` 时，PyO3 wrapper 需要区分参数是 plain value 还是 callable expression，并分别处理。

### 5.3 参数转换 helper

```rust
use construct::expr::{Evaluate, ConstExpr, IntoExpr};
use construct::value::Value;

/// 将 Python 参数转换为 Rust 表达式（Box<dyn Evaluate>）。
///
/// - callable（Path/BinExpr/lambda）→ PyClosure → Box<dyn Evaluate>
/// - plain value（int/bytes/float）→ ConstExpr → Box<dyn Evaluate>
pub fn py_param_to_evaluate(
    py: Python<'_>,
    param: &PyAny,
) -> PyResult<Box<dyn Evaluate>> {
    if param.is_callable() {
        // 表达式：Path, BinExpr, FuncPath, lambda, etc.
        let closure = PyClosure::new(param.into());
        Ok(Box::new(closure))
    } else {
        // 常量值：int, bytes, float, etc.
        let value = crate::conversions::py_to_value(py, param)?;
        Ok(Box::new(ConstExpr::new(value)))
    }
}

/// 将 Python 参数转换为 CondFunc。
///
/// - callable → py_to_cond_func
/// - non-callable truthy → 始终 True；non-callable falsy → 始终 False
pub fn py_param_to_cond_func(
    py: Python<'_>,
    param: &PyAny,
) -> PyResult<Box<dyn Fn(&Context) -> bool>> {
    if param.is_callable() {
        Ok(py_to_cond_func(param.into()))
    } else {
        // 非 callable：Python truthiness 决定常量条件
        let truthy = param.is_truthy()?;
        Ok(Box::new(move |_| truthy))
    }
}

/// 将 Python 参数转换为 ComputeFunc。
///
/// - callable → py_to_compute_func
/// - non-callable → 常量值
pub fn py_param_to_compute_func(
    py: Python<'_>,
    param: &PyAny,
) -> PyResult<Box<dyn Fn(&Context) -> Result<Value>>> {
    if param.is_callable() {
        Ok(py_to_compute_func(param.into()))
    } else {
        let value = crate::conversions::py_to_value(py, param)?;
        Ok(Box::new(move |_| Ok(value.clone())))
    }
}
```

### 5.4 使用示例

```rust
// Bytes wrapper
#[pyfunction]
pub fn Bytes(length: &PyAny) -> PyResult<PyBytes> {
    let py = length.py();
    let length_expr = py_param_to_evaluate(py, length)?;
    Ok(PyBytes {
        inner: construct::constructs::bytes::Bytes::new(length_expr),
    })
}

// Array wrapper
#[pyfunction]
pub fn Array(count: &PyAny, subcon: &PyAny) -> PyResult<PyArray> {
    let py = count.py();
    let count_expr = py_param_to_evaluate(py, count)?;
    let inner_subcon = extract_subcon(subcon)?; // PyO3 or PyConstructAdapter
    Ok(PyArray {
        inner: construct::constructs::repetition::Array::new(
            count_expr,
            inner_subcon,
        ),
    })
}

// IfThenElse wrapper
#[pyfunction]
pub fn IfThenElse(condfunc: &PyAny, then_sc: &PyAny, else_sc: &PyAny) -> PyResult<PyIfThenElse> {
    let py = condfunc.py();
    let cond = py_param_to_cond_func(py, condfunc)?;
    let then = extract_subcon(then_sc)?;
    let els = extract_subcon(else_sc)?;
    Ok(PyIfThenElse {
        inner: construct::constructs::control_flow::IfThenElse::new(cond, then, els),
    })
}
```

### 5.5 callable 检测的可靠性

PyO3 的 `PyAny::is_callable()` 检查 Python 对象是否有 `__call__` 属性：

| Python 对象 | `is_callable()` | 处理 |
|------------|-----------------|------|
| `Path` (this.x) | `True` | → PyClosure |
| `BinExpr` (this.x > 0) | `True` | → PyClosure |
| `FuncPath` (len_) | `True` | → PyClosure |
| `lambda ctx: ...` | `True` | → PyClosure |
| `int` (5) | `False` | → ConstExpr |
| `bytes` (b"data") | `False` | → ConstExpr |
| `float` (3.14) | `False` | → ConstExpr |
| `bool` (True/False) | `False` | → ConstExpr |

**边界情况**：Python `bool` 是 `int` 子类，`is_callable()` 返回 `False`。Python 函数/方法对象返回 `True`。这与 Python `callable()` 内建函数行为一致。

---

## 6. PyConstructAdapter 混合架构桥接器

### 6.1 文件位置

`construct-py/src/py_adapter.rs`

### 6.2 应用场景

当 Python 用户自定义构造器（Adapter/Validator 子类）嵌套在 Rust Struct/Sequence 中时：

```python
class IpAddressAdapter(Adapter):
    def _decode(self, obj, context, path):
        return ".".join(str(x) for x in obj)
    def _encode(self, obj, context, path):
        return [int(x) for x in obj.split(".")]

# IpAddressAdapter 是纯 Python 对象，嵌套在 Rust Struct 中
d = Struct("ip" / IpAddressAdapter(Byte[4]))
```

Rust Struct 的 subcons 列表中，`IpAddressAdapter(Byte[4])` 是纯 Python 对象，不是 PyO3 `#[pyclass]`。Rust Struct 需要能调用它的 `parse`/`build`/`sizeof` 方法。

### 6.3 核心结构

```rust
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};
use construct::core::{Construct, Context, Stream};
use construct::core::error::{ConstructError, Result};
use construct::value::Value;

/// 将任意 Python 构造器对象包装为 Rust Construct trait 实现。
///
/// 通过 GIL 回调 Python 的 parse_stream/build_stream/sizeof 方法。
/// 用于混合架构中纯 Python 构造器（用户自定义 Adapter/Validator）。
pub struct PyConstructAdapter {
    /// Python 构造器对象（必须有 parse_stream/build_stream/sizeof 方法）
    py_obj: PyObject,
}

impl PyConstructAdapter {
    pub fn new(py_obj: PyObject) -> Self {
        PyConstructAdapter { py_obj }
    }
}
```

### 6.4 Construct trait 实现

```rust
impl Construct for PyConstructAdapter {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        Python::with_gil(|py| {
            // 1. 读取剩余数据（简单方案：read all remaining bytes）
            //    注意：此方案不支持 seek 回退，对于大多数 Adapter 场景足够
            let remaining = stream.remaining_bytes()
                .map_err(|e| ConstructError::Stream {
                    path: String::new(),
                    message: format!("Failed to read stream: {}", e),
                })?;
            
            // 2. 创建 Python BytesIO 包装数据
            let io_module = py.import("io").map_err(|e| ConstructError::Generic {
                path: String::new(),
                message: format!("Failed to import io: {}", e),
            })?;
            let py_stream = io_module
                .getattr("BytesIO")
                .map_err(|e| ConstructError::Generic {
                    path: String::new(),
                    message: format!("Failed to get BytesIO: {}", e),
                })?
                .call1((PyBytes::new(py, &remaining),))
                .map_err(|e| ConstructError::Generic {
                    path: String::new(),
                    message: format!("Failed to create BytesIO: {}", e),
                })?;

            // 3. Context → Python Container + contextkw
            let py_ctx = crate::conversions::context_to_py_container(py, ctx)?;
            
            // 4. 调用 Python parse_stream(stream, **contextkw)
            let result = self.py_obj
                .call_method(py, "parse_stream", (py_stream,), Some(py_ctx.bind(py).into()))
                .map_err(|e| ConstructError::Generic {
                    path: String::new(),
                    message: format!("Python construct parse error: {}", e),
                })?;

            // 5. 同步流位置：Python BytesIO 的 tell() → Rust Stream seek
            let consumed = py_stream.call_method0(py, "tell")
                .map_err(|e| ConstructError::Generic {
                    path: String::new(),
                    message: format!("Failed to get stream position: {}", e),
                })?
                .extract::<usize>(py)
                .map_err(|e| ConstructError::Generic {
                    path: String::new(),
                    message: format!("Invalid stream position: {}", e),
                })?;
            stream.seek(consumed as u64)
                .map_err(|e| ConstructError::Stream {
                    path: String::new(),
                    message: format!("Failed to seek: {}", e),
                })?;

            // 6. 返回值 → Value
            crate::conversions::py_to_value(py, result.bind(py))
        })
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        Python::with_gil(|py| {
            // 1. Value → Python 对象
            let py_data = crate::conversions::value_to_py(py, data)?;
            
            // 2. 创建 Python BytesIO（空，用于写入）
            let io_module = py.import("io")?;
            let py_stream = io_module.getattr("BytesIO")?.call0()?;
            
            // 3. Context → Python Container
            let py_ctx = crate::conversions::context_to_py_container(py, ctx)?;
            
            // 4. 调用 Python build_stream(obj, stream, **contextkw)
            self.py_obj.call_method(
                py,
                "build_stream",
                (py_data, py_stream),
                Some(py_ctx.bind(py).into()),
            )?;
            
            // 5. 读取 BytesIO 内容，写入 Rust stream
            py_stream.call_method0(py, "seek")?; // seek to beginning
            let built = py_stream.call_method0(py, "getvalue")?;
            let bytes: &[u8] = built.downcast::<PyBytes>(py)?.as_bytes();
            stream.write_bytes(bytes)?;
            
            Ok(())
        })
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        Python::with_gil(|py| {
            let py_ctx = crate::conversions::context_to_py_container(py, ctx)?;
            let result = self.py_obj
                .call_method(py, "sizeof", (), Some(py_ctx.bind(py).into()));
            
            match result {
                Ok(v) => {
                    let size: i64 = v.extract(py).map_err(|e| ConstructError::Generic {
                        path: String::new(),
                        message: format!("sizeof returned non-int: {}", e),
                    })?;
                    if size >= 0 {
                        Ok(size as usize)
                    } else {
                        Err(ConstructError::Generic {
                            path: String::new(),
                            message: format!("sizeof returned negative: {}", size),
                        })
                    }
                }
                Err(_) => {
                    // Python sizeof 抛出 SizeofError → 映射为 Rust Sizeof
                    Err(ConstructError::Sizeof {
                        path: String::new(),
                        reason: "Python construct sizeof failed".to_string(),
                    })
                }
            }
        })
    }

    fn flagbuildnone(&self) -> bool {
        Python::with_gil(|py| {
            // 检查 Python 对象是否有 flagbuildnone 属性
            match self.py_obj.getattr(py, "flagbuildnone") {
                Ok(v) => v.extract::<bool>(py).unwrap_or(false),
                Err(_) => false,
            }
        })
    }
}
```

### 6.5 流桥接方案分析

| 方案 | 描述 | 优点 | 缺点 |
|------|------|------|------|
| **A: Read-all + BytesIO**（推荐） | 读取 Rust Stream 剩余字节 → Python BytesIO → 调用后按 tell() 同步位置 | 实现简单；Python 侧完全兼容 | 大文件内存占用；不支持流式写入 |
| B: Rust → Python stream wrapper | 创建 Python file-like 对象包装 Rust Stream，每次 read/write 穿越 FFI | 内存友好；真流式 | 每次读写穿越 FFI 边界；实现复杂；GIL 重入风险 |
| C: 混合 | 小数据用 A，大数据用 B | 兼顾性能与内存 | 实现复杂度高 |

**推荐方案 A**，理由：
1. Adapter/Validator 场景中数据量通常不大（IP 地址 4 字节、UUID 16 字节等）
2. 实现简单，风险低
3. Phase 10 优先正确性，性能优化为后续工作
4. 与 Python 原版 BytesIO 行为一致（Python 原版的 parse_stream 也从 stream 读取到内存）

### 6.6 混合分发机制

当 Python 用户创建 `Struct(*subcons)` 时，PyO3 wrapper 需要检查每个 subcon 的类型：

```rust
/// 从 Python 对象提取 Rust Construct（混合分发）。
///
/// - PyO3 #[pyclass] 对象 → 直接获取内部 Box<dyn Construct>
/// - 纯 Python 对象 → 包装为 PyConstructAdapter
pub fn extract_subcon(py: Python<'_>, obj: &PyAny) -> PyResult<Box<dyn Construct>> {
    // 尝试 1：是否为已知的 PyO3 包装类型
    if let Ok(wrapper) = obj.extract::<PyRef<PyBytes>>() {
        return Ok(wrapper.inner.clone_box());
    }
    if let Ok(wrapper) = obj.extract::<PyRef<PyStruct>>() {
        return Ok(wrapper.inner.clone_box());
    }
    // ... 其他 PyO3 类型检查 ...

    // 尝试 2：纯 Python 对象 → PyConstructAdapter
    // 检查是否有 parse_stream/build_stream 方法（duck typing）
    if obj.hasattr("parse_stream")? && obj.hasattr("build_stream")? {
        let adapter = PyConstructAdapter::new(obj.into());
        return Ok(Box::new(adapter));
    }

    Err(pyo3::exceptions::PyTypeError::new_err(
        format!("Expected a Construct instance, got {}", obj.getattr("__class__")?)
    ))
}
```

**类型检查顺序**：
1. 先检查 PyO3 `#[pyclass]` 类型（Rust 内建构造器）— 快速路径，直接获取 `Box<dyn Construct>`
2. 再检查 duck typing（有 `parse_stream` + `build_stream` 方法）— 慢速路径，包装为 `PyConstructAdapter`

**性能影响**：
- Rust 内建构造器：零开销（直接使用 Rust 实现）
- 用户自定义 Python 构造器：每次 parse/build 穿越 FFI 边界一次 + Context 转换 + GIL 获取
- 嵌套层级影响：每层 Python Adapter 增加一次 FFI 往返

### 6.7 纯 Python Adapter/Validator 基类

这些基类作为纯 Python 类提供在 `construct-py/construct_rust/adapters.py` 中：

```python
class Adapter(Subconstruct):
    """Abstract adapter class for user subclassing."""
    
    def _decode(self, obj, context, path):
        raise NotImplementedError
    
    def _encode(self, obj, context, path):
        raise NotImplementedError
    
    def _parse(self, stream, context, path):
        obj = self.subcon._parsereport(stream, context, path)
        return self._decode(obj, context, path, self.flagbuildnone)
    
    def _build(self, obj, stream, context, path):
        obj = self._encode(obj, context, path, self.flagbuildnone)
        return self.subcon._build(obj, stream, context, path)

class SymmetricAdapter(Adapter):
    """Adapter where _decode == _encode."""
    
    def _decode(self, obj, context, path):
        raise NotImplementedError
    
    def _encode(self, obj, context, path):
        return self._decode(obj, context, path)

class Validator(SymmetricAdapter):
    """Adapter that only validates without transforming."""
    
    def _validate(self, obj, context, path):
        raise NotImplementedError
    
    def _decode(self, obj, context, path):
        if not self._validate(obj, context, path):
            raise ValidationError("object failed validation", path=path)
        return obj
```

**关键**：这些基类的 `_parse`/`_build` 方法调用 `self.subcon._parsereport` / `self.subcon._build`，其中 `self.subcon` 可能是 Rust PyO3 包装的构造器。这形成 Rust → Python → Rust 的混合调用链。

**`_parsereport` 方法**：Python 原版 Construct 类有 `_parsereport` 方法（调用 `_parse` + `parsed` hook）。PyO3 包装的 Rust 构造器需要暴露此方法（或 `parse_stream`），使纯 Python Adapter 能调用。

---

## 7. Python API 映射表

### 7.1 纯 Python 表达式系统

| Python 原版 (`expr.py`) | 本模块实现 | 说明 |
|------------------------|----------|------|
| `opnames` dict | 同 | 24 个运算符 → 符号映射 |
| `class ExprMixin` | 同 | 28 个二元 + 3 个一元 + 6 个比较 + `__contains__` + pickle |
| `class UniExpr(ExprMixin)` | 同 | `__init__`/`__repr__`/`__str__`/`__call__` |
| `class BinExpr(ExprMixin)` | 同 | `__init__`/`__repr__`/`__str__`/`__call__` |
| `class Path(ExprMixin)` | 同 | `__init__`/`__repr__`/`__str__`/`__call__`/`__getattr__`/`__getitem__`/`__getfield__` |
| `class Path2(ExprMixin)` | 同 | `__init__`/`__repr__`/`__str__`/`__call__`/`__getitem__` |
| `class FuncPath(ExprMixin)` | 同 | `__init__`/`__repr__`/`__str__`/`__call__` |
| `this = Path("this")` | 同 | 全局实例 |
| `obj_ = Path("obj_")` | 同 | 全局实例 |
| `list_ = Path2("list_")` | 同 | 全局实例 |
| `len_ = FuncPath(len)` | 同 | 全局实例 |
| `sum_ = FuncPath(sum)` | 同 | 全局实例 |
| `min_ = FuncPath(min)` | 同 | 全局实例 |
| `max_ = FuncPath(max)` | 同 | 全局实例 |
| `abs_ = FuncPath(abs)` | 同 | 全局实例 |

### 7.2 Rust 桥接层

| Rust 类型/函数 | 本模块实现 | 说明 |
|---------------|----------|------|
| `struct PyClosure` | `expr_bridge.rs` | Python callable → Evaluate trait |
| `impl Evaluate for PyClosure` | 同 | `call_single` → `expr(context)` |
| `fn py_to_cond_func` | 同 | Python callable → CondFunc |
| `fn py_to_key_func` | 同 | Python callable → KeyFunc |
| `fn py_to_check_func` | 同 | Python callable → CheckFunc |
| `fn py_to_compute_func` | 同 | Python callable → ComputeFunc |
| `fn py_to_repeat_predicate` | 同 | Python callable → RepeatPredicate（3-arg） |
| `fn py_param_to_evaluate` | `param.rs` | 构造器参数 → Box<dyn Evaluate> |
| `fn py_param_to_cond_func` | 同 | 构造器参数 → CondFunc |
| `fn py_param_to_compute_func` | 同 | 构造器参数 → ComputeFunc |
| `struct PyConstructAdapter` | `py_adapter.rs` | Python Construct → Rust Construct |
| `impl Construct for PyConstructAdapter` | 同 | parse/build/sizeof/flagbuildnone |
| `fn extract_subcon` | 同 | 混合分发（PyO3 vs 纯 Python） |
| `class Adapter` | `adapters.py` | 纯 Python 基类 |
| `class SymmetricAdapter` | 同 | 纯 Python 基类 |
| `class Validator` | 同 | 纯 Python 基类 |
| `class Subconstruct` | 同 | 纯 Python 基类 |
| `def evaluate(param, context)` | `expr.py` 或 `core.py` | `param(context) if callable(param) else param` |

---

## 8. 边界条件清单

### 8.1 表达式求值边界

| # | 场景 | 输入 | 预期行为 | 测试 |
|---|------|------|---------|------|
| 1 | Path 链式访问 | `this.a.b.c(context)` | `context["a"]["b"]["c"]` | `test_this` |
| 2 | Path 下标访问 | `this["num"](context)` | `context["num"]`（等价于 `this.num`） | `test_this_getitem` |
| 3 | Path 缺失字段 | `this.missing(context)` | `KeyError`（Python 侧）/ `FieldMissing`（Rust 侧） | `test_this` |
| 4 | 嵌套 `_` 父级引用 | `this._.length(context)` | `context["_"]["length"]` | `test_this` |
| 5 | BinExpr 复合运算 | `~((path.foo * 2 + 3 << 2) % 11)` | 正确的运算符优先级 | `test_path` |
| 6 | FuncPath 柯里化 | `len_(this.items)` | 返回新 FuncPath（延迟求值） | `test_functions` |
| 7 | FuncPath 非 callable 参数 | `len_(5)` | 直接返回 5（透传） | `test_functions` |
| 8 | obj_ 单参数调用 | `obj_(context)` | 返回 context 本身（标准 evaluate 路径） | — |
| 9 | obj_ 三参数调用 | `obj_(element, list, context)` | 返回 element（RepeatUntil 路径） | `test_obj` |
| 10 | list_ 三参数调用 | `list_(element, list, context)` | 返回 list | `test_list` (xfail) |
| 11 | list_[-1] 三参数调用 | `list_[-1](element, list, context)` | 返回 `list[-1]` | `test_list` (xfail) |
| 12 | `in` 运算符 | `this.data in [1,2,3]` | 直接返回 False（xfail bug） | `test_this_in_operator` (xfail) |
| 13 | lambda `in` 运算符 | `lambda ctx: ctx.data in [1,2,3]` | 正确的 Python `in` 检查 | `test_lambda_in_operator` |
| 14 | 移位运算 | `this.a << 1`, `this.a >> 1` | 正确的左移/右移 | `test_this_shift_operator` |

### 8.2 PyClosure 桥接边界

| # | 场景 | 输入 | 预期行为 |
|---|------|------|---------|
| 15 | Python int 返回值 | `this.count(ctx)` → Python int 42 | `Value::Int(42)` 或 `Value::UInt(42)`（按范围） |
| 16 | Python bool 返回值 | `this.x > 0(ctx)` → Python True | `Value::Bool(true)` → CondFunc 返回 true |
| 17 | Python bytes 返回值 | `this.data(ctx)` → Python b"..." | `Value::Bytes(...)` |
| 18 | Python Container 返回值 | `Computed(func)(ctx)` → Container | `Value::Container(...)` |
| 19 | Python None 返回值 | `Computed(lambda ctx: None)(ctx)` | `Value::None` |
| 20 | Python 表达式抛异常 | `this.missing(ctx)` → KeyError | `ConstructError::Expr { message: "KeyError: ..." }` |
| 21 | Python lambda 抛异常 | `lambda ctx: 1/0(ctx)` → ZeroDivisionError | `ConstructError::Expr { message: "ZeroDivisionError: ..." }` |
| 22 | 大整数返回值 | `this.big(ctx)` → Python int > i128 | `ConstructError::Expr`（OverflowError） |

### 8.3 构造器参数双模式边界

| # | 场景 | 输入 | 预期行为 |
|---|------|------|---------|
| 23 | int 参数 | `Bytes(5)` | `ConstExpr(Value::UInt(5))` → 固定长度 5 |
| 24 | 表达式参数 | `Bytes(this.length)` | `PyClosure(Path(...))` → 动态长度 |
| 25 | lambda 参数 | `If(lambda ctx: ctx.x > 0, ...)` | `py_to_cond_func(lambda)` |
| 26 | Path 参数 | `If(this.x > 0, ...)` | `py_to_cond_func(BinExpr(...))` |
| 27 | bool 参数（非 callable） | `If(True, ...)` | 常量 CondFunc 始终返回 true |
| 28 | int 参数（非 callable） | `Check(0)` | 常量 CheckFunc 始终失败 |
| 29 | 嵌套表达式 | `len_(this.items) == 2` | `BinExpr(eq, FuncPath(len, Path), ConstExpr(2))` |

### 8.4 PyConstructAdapter 边界

| # | 场景 | 输入 | 预期行为 |
|---|------|------|---------|
| 30 | Adapter parse | `MyAdapter(Byte[4]).parse(b"...")` | Python `_decode` 被调用，结果转换 |
| 31 | Adapter build | `MyAdapter(Byte[4]).build("1.2.3.4")` | Python `_encode` → list → Rust subcon build |
| 32 | Adapter sizeof | `MyAdapter(Byte[4]).sizeof()` | 委托 subcon sizeof |
| 33 | Validator 失败 | `Validator(Byte).parse(b"\xff")` | Python `_validate` 返回 False → ValidationError |
| 34 | Python 异常传播 | Adapter `_decode` 抛 ValueError | 映射为 ConstructError(Generic) |
| 35 | CancelParsing | Python 回调抛 CancelParsing | parse 路径优雅退出（10.7 完整实现） |
| 36 | 流同步 | PyConstructAdapter parse 后 | Rust Stream seek 到 Python BytesIO tell() 位置 |
| 37 | 空 BytesIO | PyConstructAdapter build | 创建空 BytesIO，Python 写入后 getvalue |

### 8.5 混合分发边界

| # | 场景 | 输入 | 预期行为 |
|---|------|------|---------|
| 38 | PyO3 构造器 subcon | `Struct("x" / Byte)` | `Byte` 是 PyO3 `#[pyclass]` → 直接使用 |
| 39 | 纯 Python Adapter subcon | `Struct("x" / MyAdapter(Byte))` | `MyAdapter(...)` 是纯 Python → PyConstructAdapter |
| 40 | 非 Construct 对象 | `Struct("x" / 42)` | `TypeError`（42 无 parse_stream 方法） |
| 41 | 混合嵌套 | Rust Struct → Python Adapter → Rust Bytes | 3 层调用链：Rust→Python→Rust |

---

## 9. 与其他模块的交互

### 9.1 依赖（本模块需要）

| 模块 | 使用方式 |
|------|---------|
| **10.2 Value 映射** | `context_to_py_container`（Context → Container）、`value_to_py`（Value → PyObject）、`py_to_value`（PyObject → Value）是所有桥接器的基础 |
| **10.3 Container** | Container/ListContainer 纯 Python 类是 `context_to_py_container` 的输出类型，也是表达式 `this.x` 中 `["x"]` 下标访问的目标 |
| **construct-rs expr.rs** | `Evaluate` trait、`ConstExpr`、`IntoExpr` — PyClosure 实现 `Evaluate`，常量参数用 `ConstExpr` |
| **construct-rs constructs** | 各函数类型别名（`CondFunc`/`KeyFunc`/`CheckFunc`/`ComputeFunc`/`RepeatPredicate`）— 桥接器的目标类型 |
| **PyO3 0.22** | `PyObject`、`Python::with_gil`、`PyAny::is_callable`、`call1`/`call_method` |

### 9.2 被依赖（下游模块需要本模块）

| 模块 | 使用方式 |
|------|---------|
| **10.5 原子构造器** | `Bytes(length)`/`Seek(at)`/`Pointer(offset, ...)` 接受表达式参数 → `py_param_to_evaluate` |
| **10.6 复合构造器** | `Array(count, ...)` 接受表达式参数 → `py_param_to_evaluate`；`Struct`/`Sequence` 接受 Python subcons → `extract_subcon`（混合分发） |
| **10.7 适配器** | `If(condfunc, ...)`/`Switch(keyfunc, ...)`/`Check(func)`/`Computed(func)`/`RepeatUntil(predicate, ...)` → 各函数类型桥接器；`Adapter`/`Validator` 基类；`PyConstructAdapter` 混合架构 |
| **10.8 流操作** | `Prefixed(lengthfield, ...)` 中 lengthfield 可能是表达式；`RestreamData(datafunc, ...)` |
| **10.10 测试** | `test_expr.py`（9 用例，2 xfail）直接测试本模块的表达式系统 |

### 9.3 接口契约（对下游的承诺）

1. **纯 Python 表达式系统**：与原版 `expr.py` 行为完全一致（逐行移植），所有运算符重载、`__call__`、`__repr__`/`__str__` 行为相同
2. **PyClosure 透明性**：Python callable 通过 PyClosure 桥接后，Rust 内核感知不到差异——`Evaluate::evaluate(ctx, None)` 的行为与原生 Rust 表达式一致
3. **Context 转换**：每次表达式求值时，Rust Context → Python Container 转换保持字段名和嵌套结构（`_` parent 链正确传递）
4. **错误传播**：Python 表达式抛出的任何异常都被捕获并映射为 `ConstructError::Expr`，不导致 Rust panic
5. **PyConstructAdapter 兼容性**：任何实现了 `parse_stream`/`build_stream`/`sizeof` 的 Python 对象都可通过 PyConstructAdapter 嵌套在 Rust 构造器中使用
6. **RepeatUntil 三参数约定**：`py_to_repeat_predicate` 正确传递 `(element, list, context)` 三个参数给 Python callable

---

## 10. 需 PM 确认的事项

### 10.1 evaluate() 函数放置位置

`evaluate(param, context)` 函数在 Python 原版中定义在 `core.py` 中（line 314）。本模块有两种放置方案：

- **方案 A**：放入 `construct_rust/expr.py`（与表达式系统一起）
- **方案 B**：放入 `construct_rust/core.py`（与原版一致）

**建议方案 A**，因为 `evaluate()` 只被纯 Python 代码（Adapter 基类等）使用，Rust 内核有自己的求值路径。放入 `expr.py` 减少模块间依赖。

### 10.2 PyConstructAdapter 流桥接方案

§6.5 推荐方案 A（Read-all + BytesIO），但此方案对于大文件或流式构造器（如 `Compressed`、`Restreamed`）可能不兼容。需确认：

1. **Phase 10 是否支持用户自定义 Adapter 嵌套在流式构造器中？** 如果不支持，方案 A 足够。如果支持，需考虑方案 B。
2. **对应的测试用例（`test_ipaddress_adapter_issue_95`、`test_from_issue_244`）是否涉及流式构造？** 经检查，这两个测试使用 `Adapter(Byte[4])` 和简单的字段映射，不涉及流式构造。方案 A 可覆盖。

**建议**：Phase 10 采用方案 A（Read-all + BytesIO），流式场景标记为已知限制。

### 10.3 纯 Python Adapter 基类的 `_parsereport` 调用链

纯 Python `Adapter._parse` 调用 `self.subcon._parsereport(stream, context, path)`。但 PyO3 包装的 Rust 构造器暴露的是 `parse_stream` 方法，不是 `_parsereport`。

**两种解决方案**：

- **方案 A**：PyO3 wrapper 暴露 `_parsereport` 方法（内部委托 `parse_stream`），使纯 Python Adapter 的调用链不断裂
- **方案 B**：修改纯 Python Adapter 基类，调用 `self.subcon.parse_stream(stream, **contextkw)` 而非 `_parsereport`

**建议方案 B**，因为 `parse_stream` 是公共 API（7 个方法之一），`_parsereport` 是内部 API。修改基类使其使用公共 API 更合理。需确认此修改不引入行为差异（`_parsereport` 包含 `parsed` hook 调用，方案 B 需要处理此差异）。

### 10.4 Clone 支持

Rust 内核的某些构造器可能需要 Clone subcon（如 `Array` 在某些路径中）。`PyConstructAdapter` 持有 `PyObject`，`PyObject` 是 `Clone` 的（增加引用计数）。但 `Construct` trait 不要求 `Clone`。需确认 Rust 内核是否有构造器要求 `Box<dyn Construct>: Clone`。

经检查 Rust core 的 Construct trait 定义，`Construct` 不 extends `Clone`。Array/GreedyRange/RepeatUntil 存储的是 `Box<dyn Construct>` 不需要 Clone。因此 PyConstructAdapter 不需要实现 Clone。

### 10.5 Send + Sync 要求

`Evaluate` trait 要求 `Send + Sync`（`expr.rs:59`）。`PyObject` 在 PyO3 0.22 中是 `Send + Sync`（内部引用计数线程安全）。但 `PyConstructAdapter` 需要实现 `Construct` trait，检查 Construct 是否要求 `Send + Sync`。

经检查，`Construct` trait 定义为 `pub trait Construct: Send + Sync`（如有此约束）。`PyObject` 满足 `Send + Sync`。但需要注意：PyConstructAdapter 在多线程中使用时（如果 Rust 内核使用多线程），需要确保 GIL 安全。Phase 10 单线程场景下无问题。

---

## 11. test_expr.py 测试分析

### 11.1 测试用例预期

| 测试用例 | 预期 | 说明 |
|---------|------|------|
| `test_path` | **pass** | Path repr/str + 复合运算符求值。纯 Python 表达式系统 + Context→Container 转换 |
| `test_this` | **pass** | this.x repr/str + Struct 嵌套表达式。Rust Struct + PyClosure 桥接 |
| `test_this_getitem` | **pass** | `this["num"]` 下标访问 + Check。纯 Python Path.__getitem__ |
| `test_functions` | **pass** | len_/sum_/min_/max_/abs_ + Check。FuncPath + PyClosure |
| `test_obj` | **pass** | obj_ repr/str + RepeatUntil(obj_ == 255, Byte)。RepeatPredicate 三参数桥接 |
| `test_list` | **xfail** | 原版标记 `@xfail(reason="faulty implementation")`。list_ 部分功能在原版即失败 |
| `test_this_in_operator` | **xfail** | 原版标记 `@xfail(reason="this expression does not support in operator")` |
| `test_lambda_in_operator` | **pass** | lambda 版 `in` 运算符。纯 Python lambda 求值 |
| `test_this_shift_operator` | **pass** | `this.a << 1` / `this.a >> 1`。ExprMixin.__lshift__/__rshift__ |

**预期结果**：7 pass + 2 xfail = 9 用例全部符合预期。

### 11.2 关键测试路径分析

#### test_this（最重要的集成测试）

```python
this_example = Struct(
    "length" / Int8ub,
    "value" / Bytes(this.length),           # ← PyClosure 桥接
    "nested" / Struct(
        "b1" / Int8ub,
        "b2" / Int8ub,
        "b3" / Computed(this.b1 * this.b2 + this._.length),  # ← 嵌套 + 父级引用
    ),
    "condition" / IfThenElse(
        this.nested.b1 > 50,                 # ← CondFunc 桥接
        "c1" / Int32ub,
        "c2" / Int8ub,
    ),
)
common(this_example, b"\x05helloABXXXX", 
       Container(length=5, value=b'hello', nested=Container(b1=65, b2=66, b3=4295), condition=1482184792))
```

**执行路径**：
1. `Struct.parse(b"\x05helloABXXXX")` → Rust Struct 遍历 subcons
2. `"length" / Int8ub` → Rust FormatField parse → `ctx["length"] = 5`
3. `"value" / Bytes(this.length)` → Rust Bytes evaluate `PyClosure(ctx, None)`
   → Python: `this.length(Container(length=5, ...))` → 5
   → read 5 bytes → `ctx["value"] = b"hello"`
4. `"nested" / Struct(...)` → 嵌套 Rust Struct，parent context 通过 `ctx["_"]` 传递
5. `"b3" / Computed(this.b1 * this.b2 + this._.length)` → Rust Computed evaluate `py_to_compute_func`
   → Python: `(this.b1 * this.b2 + this._.length)(Container(b1=65, b2=66, _=Container(length=5, ...)))`
   → `65 * 66 + 5 = 4295`
6. `"condition" / IfThenElse(this.nested.b1 > 50, ...)` → Rust IfThenElse evaluate `py_to_cond_func`
   → Python: `(this.nested.b1 > 50)(Container(nested=Container(b1=65, ...)))` → True
   → parse Int32ub

**关键依赖**：
- 10.2 `context_to_py_container`：Context → Container 转换，包括 `_` parent 链
- 10.3 Container：属性访问能力（`container.length`、`container.nested.b1`）
- 本模块 PyClosure：`this.length` 等 Path 表达式的求值

#### test_obj（RepeatUntil 三参数桥接）

```python
example = Struct(
    "items" / RepeatUntil(obj_ == 255, Byte),
)
common(example, b"\x03\x07\xff", Container(items=[3,7,255]))
```

**执行路径**：
1. `Struct.parse(b"\x03\x07\xff")` → Rust Struct 遍历 subcons
2. `"items" / RepeatUntil(obj_ == 255, Byte)` → Rust RepeatUntil
3. RepeatUntil._parse 循环：
   - parse byte → `e = 3` → predicate(3, [3], ctx) → Python: `(obj_ == 255)(3, [3], Container(...))` → `3 == 255` → False
   - parse byte → `e = 7` → predicate(7, [3,7], ctx) → `7 == 255` → False
   - parse byte → `e = 255` → predicate(255, [3,7,255], ctx) → `255 == 255` → True → 停止
4. 返回 `ListContainer([3, 7, 255])`

**关键依赖**：
- `py_to_repeat_predicate`：三参数调用约定 `(element, list, context)`
- Python BinExpr `__call__(obj, *args)`：只使用第一个参数 `obj` = element
