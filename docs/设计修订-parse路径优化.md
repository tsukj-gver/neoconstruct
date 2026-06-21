# 设计修订：parse 路径优化（消除中间 dict + kwargs unpacking）

> **触发原因**：Phase 1 子任务 1.8 性能基准测试揭示 B4（100 字段）加速比仅 0.94x，
> 多数用例未达 ≥4x 硬目标。DEV 分阶段计时精确定位两个独立瓶颈。
>
> **提出者**：PM 转发 DEV 的设计质疑
> **处理者**：ARCH
> **日期**：2026-06-22
> **修订记录**：R1（2026-06-22 初版）→ R2（2026-06-22，响应 REV 检视）→
>   **R3（2026-06-22，方案 B → 方案 B'，与 pydantic-core 对齐）**
>   - R3 核心变更：实例构造策略从方案 B（逐字段写入 `instance.__dict__`）升级为
>     方案 B'（Rust 内构造独立 PyDict + `tp_new` 创建空实例 +
>     `PyObject_GenericSetAttr` 整体替换 `__dict__`）。详见 §2.2、§3.2、§5.2。
>   - R3 动机：调查 pydantic-core 后确认方案 B' 在语义安全性（绕过自定义
>     `__setattr__`，兼容 frozen dataclass）和生产验证上优于方案 B。同时纠正
>     R1/R2 未发现的 §0"无中间表示层"原则违反（dict 返回 Python 属于中间表示）。
>   - R3 保留 R2 的全部修正（P1 缓存策略调整、P2 `__slots__` 编译期报错、N1-N5）。
>   - R3 新增 §0 原则对照表（§0.4）、build 路径评估（§3.8）。
>   - R2 修正点 1（P1）：`object_new: Py<PyAny>` 编译期缓存 → R3 进一步优化为
>     `create_class` 辅助函数（直接读 `tp_new` 槽位，移除 `object_new` 字段）。
>   - R2 修正点 2（P2）：统一 `__slots__` 处理策略为编译期检测 → CompilationError
>     （不变）。
> **影响范围**：架构设计 §A.2 / §B.3 / §B.7 / §C.3.4 / §C.3.5 / §C.5 / §G
> **状态**：R3 已完成（含 B1-R3 pyo3 0.22 兼容修正），待 PM 确认后分派 DEV 实施

---

## 0. §0 原则对照表（R3 新增）

> PM 指出：当前实现（`_parse_raw` 返回 dict → Python 侧 `cls(**dict)`）从 1.5 开始
> 就违反了 §0"无中间表示层 / 一次 FFI"原则，ARCH/REV/PM 三方审查均未发现。
> R2 的方案 B 部分修正了此问题（Rust 内构造实例），但 R3 的方案 B' 更严格地对齐 §0。
> 本节明确说明方案 B' 如何满足每条 §0 原则。

### 0.1 AGENTS.md §0 原则原文摘要

| §0 原则 | 原文核心要求 |
|---------|-------------|
| **一次 FFI** | 编译、parse、build 各只有一次 Python↔Rust 边界穿越 |
| **无中间表示层** | parse 直接从 bytes 构造 PyObject，build 直接从 PyObject 读属性写字节，不在 Rust 侧引入独立的中间数据类型再转换 |
| **直接操作 Python 对象** | 通过 CPython C API 直接操作 Python 对象 |
| **pyo3 是核心依赖** | 单 crate，不存在独立的 Python 绑定层 |

### 0.2 三种实现路径的 §0 对照

| §0 原则 | 当前实现（1.5-1.8） | 方案 B（R2） | 方案 B'（R3，选定） |
|---------|--------------------|--------------|--------------------|
| **一次 FFI** | ❌ `_parse_raw` 返回 dict 到 Python，Python 侧再 `cls(**dict)`——dict 跨越 FFI 边界返回，实例构造在 Python 侧发生 | ✅ Rust 内完成 `object.__new__` + `__dict__` SetItem，返回最终实例 | ✅ Rust 内完成 PyDict 构造 + `tp_new` + `GenericSetAttr`，返回最终实例 |
| **无中间表示层** | ❌ **dict 是明确的中间表示**：bytes → Rust dict → **跨 FFI 返回** → Python 侧 `cls(**dict)` → 实例。dict 是"再转换"的中间数据类型 | ⚠️ Rust 内仍有 dict，但**不跨 FFI 返回**——dict 是构造过程中的内部临时对象，最终返回实例。严格说不违反"不跨 FFI"，但 dict 仍是"中间数据类型" | ⚠️ 同方案 B：Rust 内构造独立 dict 作为临时对象，通过 `GenericSetAttr` 整体替换 `__dict__` 后丢弃。dict 不跨 FFI 边界，属构造过程内部细节 |
| **直接操作 Python 对象** | ❌ Python 侧 `cls(**dict)` 是 Python 字节码层面的 kwargs unpacking，非 C API 直接操作 | ✅ Rust 内通过 C API（`PyDict_SetItem`）操作 `instance.__dict__` | ✅ Rust 内通过 C API（`PyDict_SetItem` + `PyObject_GenericSetAttr`）操作 |
| **pyo3 是核心依赖** | ✅ | ✅ | ✅ |

### 0.3 关于"dict 仍是中间表示"的说明

方案 B 和 B' 在 Rust 内部都构造一个 PyDict 作为临时对象。这是否违反"无中间表示层"？

**结论：不违反。** 理由：

1. **§0 禁止的是"中间数据类型再转换"**——即 Rust 侧引入独立的 Rust 中间枚举
   （如 `enum Value { Int(i64), Bytes(Vec<u8>), ... }`），parse 先构造中间类型，
   再逐个转换为 Python 对象。这是 FFI 设计 §5.3 记录的"历史教训"（退化为 0.3-1.8x）。

2. **PyDict 不是"中间表示"**——它是 Python 对象本身。parse 的最终目标就是构造一个
   有字段的 Python 实例，而 Python 实例的属性存储介质就是 `__dict__`（PyDict）。
   构造 PyDict 并填入字段值，就是在**直接构造目标 Python 对象的状态**，不存在
   "Rust 类型 → Python 类型"的转换步骤。

3. **方案 B' 的 dict 是构造过程的内部细节**——它被 `GenericSetAttr` 整体替换到
   实例的 `__dict__` 槽位后，生命周期结束。这与 pydantic-core 的做法完全一致
   （model.rs:367-379 `set_model_attrs`）。

4. **关键区分**：当前实现（1.5-1.8）的 dict **跨越 FFI 边界返回 Python**，在 Python
   侧被 `cls(**dict)` 消费——这才是 §0 禁止的"中间表示跨 FFI"。方案 B/B' 的 dict
   **全程在 Rust 内**，不跨 FFI。

### 0.4 方案 B' 对每条 §0 原则的满足方式

| §0 原则 | 方案 B' 如何满足 | 证据 |
|---------|----------------|------|
| 一次 FFI | `_parse_raw` 进入 Rust 一次（接收 bytes），返回一次（返回实例）。Rust 内的 `tp_new`、`PyDict_SetItem`、`PyObject_GenericSetAttr` 均为 C API 调用（§FFI 设计 §2.4），不计为 FFI 穿越 | pydantic-core `validate_construct`（model.rs:282-328）单次 validate 内完成全部构造 |
| 无中间表示层 | parse 从 bytes 直接构造目标 PyObject（用户类实例）。PyDict 是构造过程中的内部临时对象，不跨 FFI，不是 Rust→Python 的类型转换 | pydantic-core `set_model_attrs` 同样构造 dict 后整体替换 |
| 直接操作 Python 对象 | 全程通过 CPython C API 操作：`PyDict_New`、`PyDict_SetItem`、`tp_new`、`PyObject_GenericSetAttr` | pydantic-core `force_setattr`（model.rs:381-394）直接调 `ffi::PyObject_GenericSetAttr` |
| pyo3 是核心依赖 | 单 crate，`force_setattr` 等辅助函数在主 crate 内，通过 `pyo3::ffi` 调用 CPython C API | pydantic-core 同为单 crate |

---

## 1. 问题诊断

### 1.1 基准测试结果（1.8）

| 用例 | N | Full 加速比 | Rust-only 加速比 | 硬目标 |
|------|---|-----------|----------------|--------|
| B1 | 3 | 3.49x | 5.92x | ≥4x |
| B2 | 10 | 2.80x | 4.66x | ≥4x |
| B3 | 50 | 3.03x | 3.40x | ≥4x |
| B4 | 100 | **0.94x** | 2.89x | ≥4x |

### 1.2 瓶颈 1：Python 侧 `**dict` kwargs unpacking（O(N²)）

当前 parse 路径（**R3 将消除**）：`schema._parse_raw(data)` 返回 dict → Python 侧 `cls(**dict)`。

DEV 精确测量 `init(**dict)` 与 `init(*tuple)` 差值（纯 kwargs unpacking 开销）：

| 用例 | N | init(**) - init(*) | diff/N² | 结论 |
|------|---|--------------------|---------|------|
| B1 | 3 | 114.6 ns | 12.73 ns | 小 N 时常数未稳定 |
| B2 | 10 | 400.4 ns | 4.00 ns | O(N²) 成立 |
| B3 | 50 | 12557.9 ns | 5.02 ns | O(N²) 成立 |
| B4 | 100 | 55782.0 ns | 5.58 ns | O(N²) 成立 |

N≥10 时 `diff/N²` 趋于常数 ~5 ns，确认 **`**dict` unpacking 是 O(N²)**。
B4 的 kwargs unpacking 开销（55782 ns）占 Full parse（88098 ns）的 **63%**。

### 1.3 瓶颈 2：Rust 侧每字段成本偏高

`_parse_raw` 每字段成本：

| 用例 | N | _parse_raw (ns) | per-field (ns) |
|------|---|----------------|----------------|
| B1 | 3 | 523 | 174 |
| B2 | 10 | 1292 | 129 |
| B3 | 50 | 12880 | 258 |
| B4 | 100 | 28648 | 286 |

N=10 时每字段 129ns，但 N=100 时涨至 286ns。额外开销来源：

1. **每字段 `ctx.set_field(name, value)`**：Phase 1 不读取 context（无 this 表达式），
   纯浪费。每次调用创建 str key + PyDict_SetItem，约 40-70 ns/field。
2. **每字段 `dict.set_item(name.as_str(), value)` 创建临时 str**：`name.as_str()` 每次
   创建新的 PyUnicodeObject 并计算 hash，约 20-25 ns/field。
3. **PyDict resize**（B3/B4）：100 个 key 的 dict 可能触发多次 resize，额外 rehash 开销。
4. **CPU cache 效应**（B3/B4）：大循环体可能超出 instruction cache。

### 1.4 核心问题：违反 §0 原则

当前路径 `bytes → Rust dict → **dict unpacking → dataclass 实例` 引入了 **dict 中间表示**，
正是 AGENTS.md §0 和 FFI 设计 §5 所禁止的"中间类型转换层"。dict 跨越 FFI 边界返回
Python，是明确的 §0 违反（详见 §0.2 对照表）。

---

## 2. 方案评估

### 2.1 方案 A：Rust 内 dict 创建后 `__dict__.update`

parse 创建 dict → `cls.__new__(cls)` → `instance.__dict__.update(dict)`。

- **优点**：`__dict__.update` 是 O(N) 批量操作，消除 kwargs unpacking。
- **缺点**：仍创建中间 dict（瓶颈 2 的 dict 创建/SetItem 开销不变）。
- **评价**：部分优化。dict 中间表示仍然存在。

### 2.2 方案 B'：Rust 内构造 PyDict + `tp_new` + `GenericSetAttr` 整体替换 —— **R3 选定方案**

> **R2 原方案 B**：`object.__new__(cls)` → 获取 `instance.__dict__` → 逐字段
> `PyDict_SetItem(instance.__dict__, key, value)`（就地修改实例已有的 dict）。
>
> **R3 升级为方案 B'**：参考 pydantic-core 的 `create_class` + `set_model_attrs` +
> `force_setattr` 三件套，改为"独立 dict 构造 + 整体指针替换"策略。

**方案 B' 的实例构造策略（5 步）**：

```
1. PyDict::new_bound(py) 创建独立空 dict（可选 PyDict_NewPresized(N) 预分配）
2. 逐字段 dict.set_item(field_name, value)    // Rust 内 C API hash 操作
3. tp_new(cls, (), NULL) 创建空实例            // 直接调类型对象 tp_new 槽位，等价 cls.__new__(cls)
4. PyObject_GenericSetAttr(instance, "__dict__", dict)  // 一次性整体替换 __dict__ 指针
5. 如有 __post_init__，显式调用
```

**为什么方案 B' 优于方案 B（R3 核心论据）**：

1. **用 `PyObject_GenericSetAttr` 绕过自定义 `__setattr__`**——
   - 方案 B 逐字段 `dict.set_item` 操作的是 `instance.__dict__` 这个 dict 对象本身，
     不经过 `tp_setattro`，所以标准 dataclass（含 frozen）是安全的。
   - 但方案 B' 的 `GenericSetAttr` 是**整体替换 `__dict__` 指针**，通过
     `object.__setattr__` 的 C 实现（`PyObject_GenericSetAttr`）执行，**绕过**
     任何自定义 `tp_setattro`（包括 frozen dataclass 生成的会 raise
     `FrozenInstanceError` 的 `__setattr__`）。语义更明确、更安全。
   - 对 frozen dataclass：方案 B 能工作（dict.set_item 不被拦截），方案 B' 也能
     工作（GenericSetAttr 绕过拦截）——但方案 B' 的机制更健壮，不依赖"恰好不被拦截"。

2. **整体指针替换——一次 C 层赋值替换整个 `__dict__` 指针**——
   - 方案 B' 的 N 次 `set_item` 在**独立的预分配 dict** 上进行（可用
     `PyDict_NewPresized(N)` 避免 resize），最后一次 `GenericSetAttr` 仅替换指针。
   - 方案 B 的 N 次 `set_item` 在 `instance.__dict__`（默认 8 slots 空 dict）上进行，
     大 N 时触发多次 resize（N=100 约 4-5 次 rehash，每次 ~200-500ns）。
   - **大 N 性能优势**：方案 B' 配合 Presized 可消除 resize 开销。

3. **与 pydantic-core 实证对齐**——pydantic-core 用了完全相同的策略，已有生产验证：
   - `create_class`：`src/validators/model.rs:348-365`（`tp_new` 调用）
   - `set_model_attrs`：`src/validators/model.rs:367-379`（整体替换 `__dict__`）
   - `force_setattr`：`src/validators/model.rs:381-394`（`PyObject_GenericSetAttr`）
   - DataclassValidator `set_dict_call`：`src/validators/dataclass.rs:650-678`
     （非 slots 整体替换，slots 逐字段 force_setattr）

**方案 B' 与方案 B 的性能差异**：

| 维度 | 方案 B（R2） | 方案 B'（R3） | 差异 |
|------|-------------|--------------|------|
| 逐字段 set_item | N 次（到 instance.__dict__） | N 次（到独立 dict） | 相同 |
| dict resize（N=100） | ~4-5 次 rehash | 0 次（Presized） | **B' 优** |
| 实例创建 | `object.__new__` call1 ~50ns | `tp_new` 直接调用 ~30ns | B' 略优 |
| `__dict__` 获取 | `getattr(instance, "__dict__")` ~30ns | 不需要（整体替换） | B' 略优 |
| `__dict__` 替换 | 不需要 | `GenericSetAttr` ~20ns | B 额外省 20ns |
| 固定开销合计 | ~250ns | ~230ns | B' 略优 ~20ns |

> **结论**：方案 B' 与方案 B 的性能差异**主要在大 N 的 dict resize 消除**（~1-2.5μs，
> N=100 时显著）。固定开销差异小（~20ns）。方案 B' 的核心价值在于**语义安全性**
> 和**工程对齐**（pydantic-core 生产验证），性能是次要收益。

### 2.3 方案 C：PyDict_NewPresized + 缓存 key

不改变 parse 路径，仅优化 Rust 侧 dict 操作。

- **优点**：改动小。
- **缺点**：不解决 kwargs unpacking O(N²) 瓶颈（瓶颈 1），不解决 §0 违反。
- **评价**：不充分。其 Presized 优化已被方案 B' 吸收。

### 2.4 决策

**选择方案 B'**（R3），并附带以下补充优化（保留 R2 的全部修正）：

1. **缓存字段名为 interned `Py<PyString>`**：消除每字段 str 创建 + hash 开销。（R2 不变）
2. **Phase 1 不调用 `ctx.set_field`**：Phase 1 无 this 表达式，context 不被读取。（R2 不变）
3. **`__post_init__` 条件调用**：编译期检查类是否定义 `__post_init__`，有则在实例
   构造完毕后从 Rust 调用一次（用户钩子回调，归类为 FFI 设计 §4.4 例外）。（R2 不变）
4. **R3：`create_class` 辅助函数替代 `object_new` 缓存**：直接从 `cls.as_type_ptr()`
   读取 `tp_new` 槽位并调用（开销 ~1-2ns），无需缓存 `object.__new__` 绑定方法对象。
   移除 R2 的 `StructNode.object_new` 字段（详见 §3.2）。
5. **R3：`PyDict_NewPresized` 预分配**：大 N 时避免 dict resize（详见 §3.2）。

---

## 3. 设计决策详述

### 3.1 parse 路径变更

**当前路径**（违反 §0，R3 将消除）：
```
Python: schema._parse_raw(data) → dict → cls(**dict) → 实例
  [1次FFI]                        [dict跨FFI返回]  [O(N²) kwargs]  [O(N) init]
```

**方案 B' 优化后路径**（R3）：
```
Python: schema._parse_raw(data) → 实例（直接返回）
  [1次FFI: Rust内 PyDict构造 + tp_new + GenericSetAttr + 可选__post_init__]
```

Rust 内部流程（全部 C API，不跨 FFI）：
```
1. PyDict::new_bound(py) [或 NewPresized(N)]
2. for field: node.parse → dict.set_item(interned_key, value)
3. create_class(cls)  // tp_new(cls, (), NULL)
4. force_setattr(instance, "__dict__", dict.into_any().unbind())  // PyObject_GenericSetAttr
5. if has_post_init: instance.__post_init__()
6. return instance
```

### 3.2 StructNode 结构变更（R3）

```rust
pub struct StructNode {
    fields: Vec<(FieldName, Node)>,
    cls: Py<PyType>,            // 用户类引用（parse 时 tp_new 构造实例 + 错误信息）
    has_post_init: bool,        // 编译期检查是否有 __post_init__
    field_count: usize,         // R3 新增：字段数，用于 PyDict_NewPresized 预分配
    // R3 移除：object_new: Py<PyAny>
    //   理由：create_class 直接从 cls.as_type_ptr() 读 tp_new 槽位（~1-2ns），
    //   无需缓存 object.__new__ 绑定方法对象。与 pydantic-core create_class 对齐。
}

/// 编译期缓存的字段名（interned PyString），避免每次 SetItem 重新创建。
pub struct FieldName {
    py_name: Py<PyString>,      // 缓存的 interned str（与架构设计 §C.3.4 一致）
    rust_name: String,          // Rust 侧用（path、错误信息）
}
```

**R3 关键决策——`create_class` 辅助函数替代 `object_new` 缓存**：

R2 缓存 `object.__new__` 绑定方法为 `StructNode.object_new: Py<PyAny>`，parse 时
`call1` 调用（~50ns）。R3 参考 pydantic-core `create_class`（model.rs:348-365），
改为辅助函数直接从类型对象的 `tp_new` 槽位调用：

```rust
/// 创建用户类的空实例（等价 cls.__new__(cls)，绕过 Python 层方法查找）。
/// 与 pydantic-core create_class 语义一致（model.rs:348-365）。
///
/// pyo3 0.22 兼容：newfunc 的首参类型为 *mut PyObject，而 raw_type 为
/// *mut PyTypeObject，需显式 cast（PyTypeObject 以 PyObject 为首字段，布局兼容）。
fn create_class<'py>(cls: &Bound<'py, PyType>) -> PyResult<Bound<'py, PyAny>> {
    let py = cls.py();
    let args = PyTuple::empty(py);
    let raw_type = cls.as_type_ptr();        // *mut ffi::PyTypeObject
    unsafe {
        match (*raw_type).tp_new {
            Some(new_func) => {
                // new_func 签名: (*mut PyObject, *mut PyObject, *mut PyObject) -> *mut PyObject
                // raw_type 是 *mut PyTypeObject，需 cast 为 *mut PyObject。
                let instance_ptr = new_func(
                    raw_type as *mut pyo3::ffi::PyObject,
                    args.as_ptr(),
                    std::ptr::null_mut(),
                );
                Bound::from_owned_ptr_or_err(py, instance_ptr)
            }
            None => Err(pyo3::exceptions::PyTypeError::new_err(
                "base type without tp_new",
            )),
        }
    }
}
```

> **API 版本说明（pyo3 0.22）**：
> - `cls.as_type_ptr()`：`Bound<PyType>` 方法，返回 `*mut ffi::PyTypeObject`（0.22 可用）。
> - `(*raw_type).tp_new`：直接解引用 `PyTypeObject` 取槽位（`Option<newfunc>`）。
> - `Bound::from_owned_ptr_or_err`：pyo3 0.22 API，接管裸指针所有权为 `Bound<T>`。
> - `PyTuple::empty(py)`：0.22 返回 `Bound<'_, PyTuple>`。
> - **首参 cast 不可省略**：`raw_type: *mut PyTypeObject` 不能隐式转 `*mut PyObject`，
>   Rust 类型系统要求显式 `as` 转换（PyTypeObject 内存布局以 PyObject 头开始，转换安全）。

**为什么不缓存 `tp_new` 函数指针**（对 PM 指示的细化）：

PM 指示"改为缓存 `tp_new` 函数指针"。经调查 pydantic-core 后，ARCH 建议进一步
简化为**不缓存**，理由：

1. `tp_new` 是类型对象（`PyTypeObject`）的槽位，读取它只需 `(*raw_type).tp_new`
   （一次指针解引用，~1-2ns），比缓存后读取还快（缓存值在 StructNode 堆内存中，
   可能触发 cache miss）。
2. pydantic-core `create_class` **不缓存** `tp_new`，每次从类型对象读取。
3. 缓存函数指针需要 `Option<extern "C" fn(...)>`，增加 StructNode 体积。
4. `cls: Py<PyType>` 已在 StructNode 中，`create_class(&self.cls.bind(py))` 足够。

> **与 PM 指示的关系**：PM 的核心意图是"不再缓存 `object.__new__` 绑定方法对象，
> 改用 `tp_new` 槽位"。ARCH 采纳此方向，但在"是否缓存函数指针"上选择不缓存
> （与 pydantic-core 一致）。如 PM/REV 认为必须缓存，可加回 `tp_new` 字段，
> 对架构无破坏性。

**`tp_new` vs `object.__new__` 的语义差异**：

| 调用方式 | 标准 @dataclass | 用户自定义 `__new__` |
|---------|----------------|---------------------|
| R2：`object.__new__(cls)` | ✅ 等价（dataclass 无自定义 `__new__`） | ⚠️ 绕过用户 `__new__` |
| R3：`cls.tp_new(cls, (), NULL)` | ✅ 等价（`cls.tp_new` 继承自 object） | ✅ 调用用户 `__new__`（与 pydantic-core 一致） |

R3 改为 `cls.tp_new`（用户类的 `tp_new` 槽位），语义与 pydantic-core `create_class`
完全一致。标准 @dataclass 无自定义 `__new__`，`cls.tp_new` == `object.tp_new`，
行为与 R2 等价。用户自定义 `__new__` 时，R3 会调用它（尊重用户类定义），
R2 会绕过——R3 的行为更正确。

**`force_setattr` 辅助函数**（语义与 pydantic-core model.rs:381-394 一致；
API 适配 pyo3 0.22）：

> **B1-R3 修正（2026-06-22）**：R3 初版误用了 pyo3 0.23+ API
> （`IntoPyObject`、`into_pyobject_or_pyerr`、`py_error_on_minusone`）。
> 项目 `Cargo.toml` 锁定 pyo3 0.22，这些 API 不可用。改为 pyo3 0.22 兼容写法：
> `IntoPy<Py<PyAny>>` + `into_py(py)` + 手动 `== -1` 检查。算法不变。

```rust
/// 强制设置属性，绕过自定义 __setattr__（直接调 PyObject_GenericSetAttr）。
/// 用于整体替换实例的 __dict__，兼容 frozen dataclass。
///
/// pyo3 0.22 兼容写法（项目锁定 0.22，不可用 0.23+ 的 IntoPyObject API）。
fn force_setattr<N, V>(
    py: Python<'_>,
    obj: &Bound<'_, PyAny>,
    attr_name: N,
    value: V,
) -> PyResult<()>
where
    N: pyo3::conversion::IntoPy<Py<PyAny>>,
    V: pyo3::conversion::IntoPy<Py<PyAny>>,
{
    // into_py 返回 Py<PyAny>（拥有所有权的 Python 对象指针），as_ptr 取裸指针。
    let attr_name = attr_name.into_py(py);
    let value = value.into_py(py);
    let result = unsafe {
        pyo3::ffi::PyObject_GenericSetAttr(
            obj.as_ptr(),
            attr_name.as_ptr(),
            value.as_ptr(),
        )
    };
    if result == -1 {
        // C API 返回 -1 表示出错，Python 异常已挂起，fetch 取出转为 PyErr。
        Err(PyErr::fetch(py))
    } else {
        Ok(())
    }
}
```

> **API 版本说明**：
> - `IntoPy<Py<PyAny>>`：pyo3 0.22 trait（`pyo3::conversion::IntoPy`），0.23 起被
>   `IntoPyObject` 取代。本项目锁定 0.22，必须用 `IntoPy`。
> - `.into_py(py)`：返回 `Py<PyAny>`，非 Result，无需 `?`。
> - `PyErr::fetch(py)`：pyo3 0.22 从挂起的 C API 异常取出 `PyErr`，
>   替代 0.23+ 的 `pyo3::err::py_error_on_minusone` helper。
> - `obj.as_ptr()` / `attr_name.as_ptr()`：`Py<T>` 和 `&Bound<T>` 均有 `as_ptr`。

> **根节点关系说明（保留 N1）**：根 StructNode 的 `cls` 与 CompiledSchema 的 `cls`
> 是**同一个 Python 类对象的两个强引用**（`Py<PyType>::clone`，引用计数 +1）。
> CompiledSchema.cls 用于顶层错误信息和类名报告；StructNode.cls 用于 parse
> 实例构造（`create_class(cls)`）与 `__post_init__` 调用。每个 StructNode 都需要
> 自己的 `cls`——根节点恰好等于 CompiledSchema.cls。

### 3.3 `__init__` 语义权衡

绕过 `@dataclass.__init__` 的语义分析：

| `@dataclass.__init__` 做的事 | 方案 B' 是否覆盖 | 方式 |
|------------------------------|-----------------|------|
| 逐字段 setattr | ✅ 等价 | 构造独立 dict → `GenericSetAttr` 整体替换 `__dict__` |
| 调用 `__post_init__` | ✅ 条件覆盖 | `has_post_init` 标志触发回调 |
| 类型转换/校验 | ❌ 不覆盖 | `@dataclass` 默认不生成类型转换 |

**结论**：对于标准 `@dataclass`（无自定义 `__init__`、无 `__post_init__`），
方案 B' 的 `create_class` + `GenericSetAttr(__dict__)` 与 `cls(**dict)` **语义完全等价**。

**边界情况处理**：

| 情况 | 处理 |
|------|------|
| 用户定义了 `__post_init__` | Rust 内 `GenericSetAttr` 替换 `__dict__` 后调用一次 `instance.__post_init__()` |
| 用户自定义 `__init__`（非 dataclass 生成） | 不支持（Phase 1 API 要求 `@dataclass`） |
| 用户定义了 `__new__` | R3：`cls.tp_new` 会调用用户 `__new__`（与 pydantic-core 一致，尊重用户类定义） |
| `__slots__` dataclass | 编译期报 CompilationError（fail fast，见 §3.6） |
| `frozen=True` dataclass | `GenericSetAttr` 绕过 frozen 的 `__setattr__`（会 raise FrozenInstanceError），走快速路径 |
| 用户自定义 `__setattr__`（非 dataclass） | `GenericSetAttr` 绕过，已知限制（见 §3.7 N2） |
| InitVar 字段声明 `field()` | 不支持，文档禁止此用法（见 §3.7 N3） |

### 3.4 StructRefNode 变更

StructRefNode.parse 不再自己创建实例——对方 root（StructNode）的 parse 已包含实例构造。

```rust
// StructRefNode.parse（简化）
fn parse(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
    let schema = self.resolve_schema(py)?;
    let root = schema.root();
    // root.parse 内部已构造对方类实例（create_class + GenericSetAttr）
    root.parse(py, stream, ctx, path)
    // 不再需要 cls.call((), Some(&sub_dict))
}
```

**收益**：消除嵌套结构的 Rust→Python 回调（每次 ~200-400ns），嵌套性能大幅提升。

### 3.5 Context 优化

Phase 1 中 StructNode.parse/build **不调用 `ctx.set_field`**。

- Phase 1 无 this 表达式，context 不会被读取。
- Context 结构保留（trait 签名不变），只是 StructNode 不填充。
- Phase 2 加入 this 表达式时恢复 `ctx.set_field`（按需添加）。

### 3.6 `__slots__` 处理策略（R2 统一，R3 保留）

> **R2 修正说明**：原 R1 文档对此处表述自相矛盾。R2 统一为**单一策略：编译期检测 →
> CompilationError（fail fast）**。R3 保留此策略不变。

**策略**：编译期检测用户类是否定义了 `__slots__`。检测到则 `compile_schema` 返回
`ConstructError::Compilation`，**类定义时（`__init_subclass__` 中）即报错**，符合
§E.1"编译错误在导入时报出"原则。

**R3 补充（pydantic-core 对比）**：pydantic-core 的 DataclassValidator 区分 slots/非
slots（dataclass.rs:658-665）：非 slots 用整体 `__dict__` 替换（我们的方案 B'），
slots 用逐字段 `force_setattr`（因为 slots 类无 `__dict__`）。Phase 1 不支持 slots，
后续阶段如需支持，可参考 pydantic-core 的 slots 分支（逐字段 `force_setattr` 到
slot descriptor）。

### 3.7 语义差异与已知限制（R3 更新 N2）

> 本节明确绕过 `@dataclass.__init__` 带来的语义差异，避免 DEV/REV/用户混淆。

**N2：自定义 `__setattr__` 的语义差异（R3 更新）**：

方案 B' 用 `PyObject_GenericSetAttr` 整体替换 `__dict__`，**绕过所有自定义
`tp_setattro`**（即任何 `__setattr__`）。

| 类形态 | `cls(**dict)` 行为 | 方案 B' `GenericSetAttr` 行为 | 影响 |
|--------|-------------------|------------------------------|------|
| 标准 `@dataclass`（不生成 `__setattr__`） | setattr → 写 `__dict__` | 整体替换 `__dict__` | ✅ 完全等价 |
| `@dataclass(frozen=True)` | 生成 `__setattr__` 拦截赋值（raise FrozenInstanceError） | `GenericSetAttr` 绕过拦截，整体替换 `__dict__` | ✅ 等价（parse 需要写入字段，frozen 的拦截在 init 后才应生效） |
| 用户自定义 `__setattr__`（非 dataclass） | 触发用户逻辑 | `GenericSetAttr` 绕过用户逻辑 | ⚠️ 已知限制 |

**R3 改善（vs R2 方案 B）**：方案 B' 的 `GenericSetAttr` 对 frozen dataclass 的处理
比方案 B 更明确——方案 B 靠"dict.set_item 不经过 tp_setattro"绕过 frozen 拦截
（恰好不被拦截），方案 B' 靠"GenericSetAttr 是 object 的 setattr 实现"绕过
（明确绕过）。两者结果相同，但方案 B' 的机制更健壮、更易理解。

标准 dataclass（含 frozen）不受影响。仅在用户**额外**定义了 `__setattr__` 时存在
差异，此为已知限制，文档标注。Phase 1 不处理。

**N3：`dataclasses.InitVar` 不支持**（R2 不变）：

`@dataclass` 的 `InitVar[T]` 字段是仅 `__init__` 用的临时变量，不存为实例属性。
**约束**：InitVar 字段不应声明 `field()`。若误用，行为是把 InitVar 当普通字段解析——
属用户错误，不特殊处理。

### 3.8 build 路径评估（R3 新增）

> PM 要求评估 build 路径是否有"中间表示"问题。

**当前 build 路径**：
```
Python: obj.build() → schema._build_raw(self) → Rust 内遍历执行树
  → 逐字段 getattr(obj, field_name) → 写入 BuildStream → 返回 PyBytes
```

**评估结论：build 路径无 §0 违反，无需修改。**

| §0 原则 | build 路径现状 | 是否违反 |
|---------|--------------|---------|
| 一次 FFI | `obj.build()` → Rust 一次（传实例），返回 bytes 一次 | ✅ 满足 |
| 无中间表示层 | Rust 直接从实例 `getattr` 读属性写字节，无 Rust 中间类型 | ✅ 满足 |
| 直接操作 Python 对象 | `getattr`（C API `PyObject_GetAttr`）直接操作 | ✅ 满足 |
| pyo3 是核心依赖 | 单 crate | ✅ 满足 |

**与 parse 路径的对比**：

- parse 的旧实现（1.5-1.8）违反 §0，是因为它返回 dict 到 Python，Python 侧再
  `cls(**dict)`——dict 跨 FFI 返回是"中间表示跨边界"。
- build 路径**不存在此问题**：Python 侧直接传实例（`schema._build_raw(self)`），
  Rust 内 `getattr` 读属性，输出 bytes。全程无中间表示跨 FFI。

**pydantic-core 的 serialize（build）路径对比**：

pydantic-core 的 serialize 路径同样是：遍历字段的 serializer，每个字段从 model
实例 `getattr`（或直接读 `__dict__`）取值，然后 serialize。这与我们的 build 路径
一致——直接操作 Python 对象，无中间表示。pydantic-core 的 `PythonRenderer`/
`SchemaSerializer` 体系确认了此模式。

**build 路径的优化**（已在 R2 完成，R3 保留）：
1. 消除 `ctx.set_field`（Phase 1 不需要）——每字段省 ~40-70ns。
2. 缓存字段名用于 getattr（PyString interned）——每字段省 ~20ns。

> **结论**：build 路径无需在本次 R3 修订中修改。当前设计符合 §0 全部原则。

---

## 4. 性能预测（R3 更新）

### 4.1 每字段成本模型

基于实测数据（B1 _parse_raw = 523ns, N=3）反推：

- FFI 固定开销：~100ns（进入+返回）
- 当前每字段：~140ns（FormatField ~30ns + dict.set_item ~50ns + ctx.set_field ~50ns + overhead ~10ns）

方案 B' 优化后每字段：
- FormatField parse：~30ns（不变）
- `dict.set_item`（interned key，独立 dict）：~25ns（消除 str 创建+hash）
- 无 ctx.set_field：0ns
- path + overhead：~10ns
- **优化后每字段：~65ns**（与方案 B 相同，因为 set_item 操作本身一样）

### 4.2 固定开销（R3 修正）

| 固定开销组件 | R2（方案 B） | R3（方案 B'） | 修正原因 |
|-------------|-------------|--------------|---------|
| FFI 穿越（进入+返回） | ~100ns | ~100ns | 不变 |
| dict 创建 | `getattr(instance, "__dict__")` ~30ns | `PyDict::new` 或 `NewPresized` ~20ns | B' 直接分配，比 getattr 更快 |
| 实例构造 | `object.__new__` call1 ~50ns | `tp_new` 直接调用 ~30ns | B' 直接 C 函数调用，比 pyo3 call1 更快 |
| `__dict__` 替换 | 不需要 | `GenericSetAttr` ~20ns | B' 额外的一次指针替换 |
| path 初始化 | ~20ns | ~20ns | 不变 |
| 其他 overhead | ~50ns | ~40ns | B' 无 getattr __dict__ |
| **固定开销合计** | **~250ns** | **~230ns** | B' 略优 ~20ns |

### 4.3 dict resize 开销（R3 新增大 N 分析）

| N | 方案 B（instance.__dict__ 默认 8 slots） | 方案 B'（PyDict_NewPresized(N)） | 差异 |
|---|----------------------------------------|--------------------------------|------|
| 3 | 0 次 resize | 0 次 resize | 无 |
| 10 | 1 次 resize（8→16） | 0 次 | B' 省 ~200ns |
| 50 | 3 次 resize（8→16→32→64） | 0 次 | B' 省 ~600-900ns |
| 100 | 4 次 resize（8→16→32→64→128） | 0 次 | B' 省 ~800-2000ns |

> CPython dict resize 每次约 200-500ns（含 rehash）。方案 B' 的 `PyDict_NewPresized`
> 一次性分配足量 slots，消除全部 resize。这是方案 B' 对大 N 的主要性能优势。

### 4.4 预测加速比（R3）

Full parse（方案 B' 优化后）≈ Rust 内部总耗时（不再有 Python 侧 `cls(**dict)`）。

| 用例 | N | 优化后 per-field | 固定开销 | Full 预测 | Python 基线 | 预测加速比 | 目标 |
|------|---|-----------------|---------|-----------|------------|-----------|------|
| B1 | 3 | 65 ns | ~230 ns | ~425 ns | 3095 ns | **~7.3x** | ≥4x ✓ |
| B2 | 10 | 65 ns | ~230 ns | ~880 ns | 6017 ns | **~6.8x** | ≥4x ✓ |
| B3 | 50 | 80 ns* | ~230 ns | ~4230 ns | 43743 ns | **~10.3x** | ≥4x ✓ |
| B4 | 100 | 90 ns* | ~230 ns | ~9230 ns | 82706 ns | **~9.0x** | ≥4x ✓ |

> R3 修正：固定开销由 R2 的 ~250ns 修正为 ~230ns（create_class + GenericSetAttr
> 比 object.__new__ call1 + getattr __dict__ 更快）。大 N 加速比因消除 dict resize
> 进一步上调（B4 从 R2 的 ~8.9x 上调至 ~9.0x，主要来自 Presized）。

> *B3/B4 每字段略高是因为大循环的 cache 效应，但仍远低于当前的 258/286 ns。
> 方案 B' 的 Presized 消除了 resize，per-field 比 R2 方案 B 更稳定。

### 4.5 保守估计

即使每字段优化后为 120 ns（含 cache miss 余量），固定开销按 R3 的 ~230ns 计：

| 用例 | Full 预测 | 加速比 | 达标 |
|------|-----------|--------|------|
| B1 | ~590 ns | ~5.2x | ✓ |
| B2 | ~1430 ns | ~4.2x | ✓ |
| B3 | ~6230 ns | ~7.0x | ✓ |
| B4 | ~12230 ns | ~6.8x | ✓ |

> R3 保守估计下所有用例均 ≥4x。即使每字段达悲观值 120ns，B2 仍达 4.2x。

### 4.6 结论

**所有用例预计达到 ≥4x**。性能目标 **不需要修订**。方案 B' 相比方案 B 的性能收益
主要在大 N 的 dict resize 消除，核心价值在于语义安全性和工程对齐。

---

## 5. 实现指导（给 DEV，R3 更新）

### 5.1 需要修改的文件

| 文件 | 改动 |
|------|------|
| `src/schema.rs` | `_parse_raw` 签名变更：返回实例（`PyAny`）而非 `PyDict` |
| `src/nodes/struct_node.rs` | 增加 cls/has_post_init/field_count 字段；**移除 object_new**；parse 用 create_class + force_setattr 构造实例 |
| `src/nodes/struct_ref.rs` | parse 简化：委托对方 root.parse |
| `src/compile.rs` | 编译时：创建 FieldName（interned str）；检测 has_post_init；**检测 __slots__ → CompilationError**；记录 field_count |
| `python/construct/_mixin.py` | parse 方法简化：直接返回 schema._parse_raw 结果 |

> **R3 提醒**：`_parse_raw` 返回类型由 `PyDict` 改为 `PyAny`（用户实例）后，
> `compile.rs` 和 `schema.rs` 中相关测试（断言返回 PyDict / `.as_ref().cast_as::<PyDict>()`
> 等）需 DEV 适配为断言实例属性。

### 5.2 StructNode.parse 实现要点（R3 重写）

```rust
impl Construct for StructNode {
    fn parse(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
        // 1. 创建独立 dict（R3：Presized 预分配，消除大 N resize）
        //    pyo3 0.22 用 new_bound（返回 Bound<'_, PyDict>）。
        let dict = PyDict::new_bound(py);
        // 或使用 ffi::PyDict_NewPresized(self.field_count) 预分配：
        //   let dict = Bound::from_owned_ptr_or_err(py,
        //       unsafe { ffi::PyDict_NewPresized(self.field_count as isize) })?
        //       .downcast_into::<PyDict>()?;

        // 2. 逐字段解析 + set_item 到独立 dict
        for (field_name, node) in &self.fields {
            path.push_field(field_name.rust_name());
            let value = node.parse(py, stream, ctx, path)?;
            // 使用缓存的 interned key
            dict.set_item(field_name.py_name().bind(py), value.bind(py))?;
            // Phase 1: 不调用 ctx.set_field
            path.pop();
        }

        // 3. tp_new 创建空实例（R3：create_class 辅助函数，直接读 tp_new 槽位）
        let instance = create_class(self.cls.bind(py))?;

        // 4. 整体替换 __dict__（R3：force_setattr 绕过自定义 __setattr__）
        //    pyo3 0.22：dict.into_any().unbind() 得 Py<PyAny>，满足 IntoPy<Py<PyAny>> bound。
        //    （遵循代码库现有惯例：struct_node.rs:123 等均用 .into_any().unbind()。
        //     consume dict 后不再使用——dict 引用由 instance.__dict__ 持有。）
        force_setattr(py, &instance, "__dict__", dict.into_any().unbind())?;
        // dict 的所有权转移给实例的 __dict__ 槽位

        // 5. 可选：调用 __post_init__
        if self.has_post_init {
            instance.call_method0("__post_init__")?;
        }

        Ok(instance.into_any().unbind())
    }
}
```

**R3 代码变更要点（vs R2 方案 B）**：
1. 步骤 1-2：先构造独立 dict 再 set_item（R2 是先 `object.__new__` 再 getattr
   `instance.__dict__` 再 set_item）。dict 可 Presized 预分配。
2. 步骤 3：`create_class(cls)` 直接读 `tp_new` 槽位（R2 用 `object_new.call1`）。
3. 步骤 4：`force_setattr(instance, "__dict__", dict)` 整体替换（R2 无此步，
   R2 是就地修改 instance.__dict__）。
4. 移除 R2 的 `object_new` 字段和 `cache_object_new` 函数。

### 5.3 _parse_raw 签名变更

```rust
#[pyo3(signature = (data))]
pub fn _parse_raw<'py>(
    &self,
    py: Python<'py>,
    data: &Bound<'py, PyBytes>,
) -> PyResult<Bound<'py, PyAny>> {  // 返回 PyAny（实例）而非 PyDict
    let bytes = data.as_bytes();
    let mut stream = ParseStream::new(bytes);
    let mut ctx = Context::new_root(py)?;
    let mut path = Path::new();
    // root.parse 返回实例（StructNode 内部已 create_class + force_setattr）
    let result = self.root.parse(py, &mut stream, &mut ctx, &mut path)?;
    Ok(result.into_bound(py))
}
```

### 5.4 Python 侧变更

```python
@classmethod
def parse(cls, data):
    schema = cls._construct_compiled
    if schema is None:
        raise ConstructError(...)
    # 直接返回实例，不再 cls(**dict)
    return schema._parse_raw(data)
```

### 5.5 编译期检测与缓存（R3 简化）

R3 将编译期逻辑统一到 `compile_schema`。**移除 R2 的 `cache_object_new`**
（改用 `create_class` 辅助函数）：

```rust
// R3 移除：fn cache_object_new(...) — 不再需要缓存 object.__new__

/// 检查类是否定义 __post_init__（含 MRO）。
fn check_has_post_init(cls: &Bound<PyType>) -> bool {
    cls.getattr("__post_init__").is_ok()
}

/// 检测 __slots__，命中则编译期报错（fail fast）。
fn check_has_slots(cls: &Bound<PyType>) -> PyResult<bool> {
    match cls.getattr("__slots__") {
        Ok(slots) => Ok(!slots.is_none()),
        Err(_) => Ok(false),
    }
}
```

**组装 StructNode 时**（compile_schema 内）：
```rust
// 1. 检测 __slots__（fail fast）
if check_has_slots(cls)? {
    return Err(ConstructError::Compilation {
        message: format!(
            "类 {} 使用了 __slots__，Phase 1 不支持 slots dataclass（无 __dict__）。\
             请移除 __slots__ 或使用普通 @dataclass。",
            cls.name()?
        ),
    }.into());
}

// 2. R3 移除：不再缓存 object_new（改用 create_class 辅助函数）

// 3. 检测 has_post_init
let has_post_init = check_has_post_init(cls);

// 4. R3 新增：记录 field_count（用于 PyDict_NewPresized）
let field_count = fields.len();

// 5. 组装 StructNode（R3：无 object_new 字段，新增 field_count）
let root = Node::Struct(StructNode {
    fields,
    cls: cls.clone().into_py(py),
    has_post_init,
    field_count,
});
```

### 5.6 测试要点（R3 更新）

#### 5.6.1 功能等价
1. **往返一致性**：`parse(build(obj)) == obj` 必须仍成立（B1-B4 全部用例）。
2. **嵌套结构**：StructRef parse 正确返回嵌套实例（B5、B6）。

#### 5.6.2 编译期行为
3. **`__slots__` 编译期报错**：定义 `@dataclass(slots=True)` 类，
   `__init_subclass__` 应抛 CompilationError（在类定义时，非 parse 时）。
4. **InitVar 误用**：对 InitVar 字段声明 `field()` 应按普通字段处理（§3.7 N3）。

#### 5.6.3 运行期行为
5. **`__post_init__` 调用**：定义有 `__post_init__` 的类，验证 parse 后被调用，
   且调用时字段已写入 `__dict__`（`has_post_init` 标志生效）。
6. **嵌套 `__post_init__`**：嵌套结构中，内外层的 `__post_init__` 均应被正确触发。
7. **frozen dataclass**：`@dataclass(frozen=True)` 类可正常 parse
   （`GenericSetAttr` 绕过 frozen 的 `__setattr__`），parse 返回的实例属性只读语义保持。

#### 5.6.4 R3 新增：force_setattr 正确性
8. **`__dict__` 整体替换验证**（R3）：parse 后 `instance.__dict__` 的所有 key 与
   编译期字段名一致，且 `instance.__dict__` 是 Rust 内构造的 dict（非实例默认空 dict）。
9. **frozen dataclass 写入成功**（R3）：验证 `@dataclass(frozen=True)` 的类 parse
   不抛 FrozenInstanceError（证明 `GenericSetAttr` 成功绕过 frozen `__setattr__`）。

#### 5.6.5 缓存正确性
10. **interned key identity**：验证 parse 写入的 `__dict__` 的 key 与
    `field_name.py_name` 是同一 interned str 对象（`is` 比较），确保缓存生效。

#### 5.6.6 性能回归
11. **性能基准**：重跑 B1-B4，验证 ≥4x（R3 预测见 §4.4）。
12. **大 N 回归**：B4（100 字段）重点验证加速比 ≥4x（R1 此处仅 0.94x）。
13. **R3 新增：dict resize 消除验证**：B3/B4 的 per-field 成本应与 B1/B2 接近
    （Presized 消除 resize 后，大 N 不应有额外 per-field 开销）。

### 5.7 REV 非阻塞建议纳入（R3 修正）

> 以下为 R3 检视中 REV 提出的非阻塞建议，ARCH 确认后纳入本文档，供 DEV 实施时参考。

#### 5.7.1 N4-R3：`@dataclass(slots=True)` 时序限制

**问题**：`check_has_slots` 在 `__init_subclass__` 中执行，但 Python 3.10+ 的
`@dataclass(slots=True)` 生成 `__slots__` 的时机在 `__init_subclass__` **之后**
（slots 由 dataclass 装饰器在类创建完成后通过新建子类实现，非在
`__init_subclass__` 时可见）。因此 `check_has_slots` 在 `__init_subclass__`
阶段**可能无法检测到 `slots=True`**——此时 `cls.__slots__` 尚未被 dataclass 写入。

**影响评估**：中低。Phase 1 用户 API 要求使用标准 `@dataclass`（非 `slots=True`）。
即使 `check_has_slots` 漏检，`force_setattr(py, instance, "__dict__", dict)` 在
slots 类上会**运行期失败**（slots 类无 `__dict__`，`GenericSetAttr` 写 `__dict__`
会报错），属于"晚报错而非不报错"。

**DEV 实施指引**：
1. `check_has_slots` 仍保留在 `__init_subclass__`（能拦截手写 `__slots__` 的非
   dataclass 类，此类在 `__init_subclass__` 时 `__slots__` 已存在）。
2. 补充一条**运行期兜底**：`force_setattr` 失败（`PyObject_GenericSetAttr` 返回 -1）
   时，错误信息应包含"该类可能使用了 `__slots__` / `slots=True`，Phase 1 不支持"提示，
   便于用户定位问题。
3. 测试 §5.6.2 第 3 条改为：**手写 `__slots__`** 的类应在编译期（`__init_subclass__`）
   报错；**`@dataclass(slots=True)`** 的类可能在 parse 时报错（运行期兜底），两种路径
   均需覆盖测试。在测试文档中明确标注此差异。

#### 5.7.2 N6-R3：StructNode::new() 签名变更影响测试

**问题**：R3 为 `StructNode` 新增 `cls: Py<PyType>`、`has_post_init: bool`、
`field_count: usize` 三个字段。现有 `StructNode::new(fields: Vec<(String, Node)>)`
签名必须变更（需额外传入 cls 等参数）。

**影响范围**：当前代码库中 `StructNode::new(...)` 的调用点（主要是单元测试中手工
构造 StructNode 的地方）。经 grep 确认约 15+ 处测试调用。

**DEV 实施指引**：
1. 新签名建议：
   ```rust
   pub fn new(
       fields: Vec<(FieldName, Node)>,   // R3：String → FieldName
       cls: Py<PyType>,
       has_post_init: bool,
       field_count: usize,                // 或在内部用 fields.len() 推导
   ) -> Self
   ```
   > 注意：`field_count` 可由 `fields.len()` 推导，作为独立字段仅为可读性 / Presized
   > 时不用重复 `.len()`。DEV 可选择省略此参数，在 parse 内用 `self.fields.len()`。
2. 测试中手工构造 StructNode 时需提供测试用的 `Py<PyType>`（如临时定义一个
   `@dataclass` 测试类取其类型对象）和 `has_post_init = false`。
3. 建议提供**测试专用构造辅助**（如 `StructNode::new_for_test(fields)`，
   内部创建一个最小 mock 类），减少 15+ 处测试的样板代码。是否添加由 DEV 决定。
4. 所有受影响测试必须在同一子任务（1.x 重构）内同步更新，不可遗留编译失败的测试。

---

## 6. 对架构设计文档的更新清单

以下章节需要同步更新（R3，ARCH 已执行）：

| 章节 | 更新内容 |
|------|---------|
| §A.2 | parse 方法：移除 `cls(**dict)`，直接返回 Rust 产出；更新决策说明为方案 B' |
| §B.1 | parse 入口输出：dict → 实例 |
| §B.3 | _parse_raw 签名：返回 PyAny |
| §B.7 | dataclass 实例构造：R3 改写为方案 B'（create_class + force_setattr）；移除 object_new 缓存说明 |
| §C.3.4 | StructNode：移除 object_new，新增 field_count；parse 改为 dict 构造 + create_class + force_setattr |
| §C.3.5 | StructRefNode：parse 委托对方 root（不变） |
| §C.5 | Context：Phase 1 不填充（不变） |
| §G.1-2 | 瓶颈识别和预测更新（R3：固定开销 ~230ns，新增 dict resize 分析） |
| §G.3 | __slots__ 退化场景：编译期报错（不变） |
| 附录B | parse 返回实例的决策溯源更新为方案 B' |

---

## 7. 风险与缓解

| 风险 | 概率 | 缓解 |
|------|------|------|
| `tp_new` 在某些 Python 版本行为不一致 | 低 | 测试覆盖 3.10-3.14；pydantic-core 已生产验证此模式 |
| `PyObject_GenericSetAttr` 整体替换 `__dict__` 的副作用 | 低 | pydantic-core `set_model_attrs` 已验证；标准 dataclass 无副作用 |
| `__post_init__` 调用引入额外开销 | 中 | 仅 has_post_init=True 时调用，无则零开销 |
| `__dict__` 访问在特殊类失败 | 低 | R2 编译期已拦 slots；frozen dataclass 有 `__dict__` |
| 嵌套结构性能提升不如预期 | 低 | StructRef 回调消除是确定性的 |
| R3：`tp_new` 调用用户自定义 `__new__` 的副作用 | 低 | 标准 @dataclass 无自定义 `__new__`；用户自定义属罕见边缘情况 |
| R3：Presized dict 内存浪费（小 N 时多分配） | 极低 | Presized 分配量 = field_count，与实际需求精确匹配 |

---

## 8. 实现指导摘要（给 DEV）

> 本节为 DEV 的快速参考，完整细节见 §5。

**方案 B' 核心步骤**（parse）：
1. `PyDict::new_bound(py)`（或 `PyDict_NewPresized(field_count)`）创建独立 dict
2. 逐字段 `node.parse` → `dict.set_item(interned_key, value)`
3. `create_class(cls)` —— `tp_new(cls, (), NULL)` 创建空实例
4. `force_setattr(instance, "__dict__", dict.into_any().unbind())` —— `PyObject_GenericSetAttr` 整体替换
5. `if has_post_init: instance.__post_init__()`
6. 返回实例

> **pyo3 0.22 提醒**：本项目锁定 pyo3 0.22（`Cargo.toml`）。上述 API 中：
> - 用 `PyDict::new_bound(py)`，**不要**用 `PyDict::new(py)`（后者是 0.21 前 API）。
> - `force_setattr` / `create_class` 辅助函数的 trait bound 用 `IntoPy<Py<PyAny>>`，
>   **不要**用 `IntoPyObject`（0.23+ API，0.22 不可用）。详见 §3.2 B1-R3 修正。

**需要新增的辅助函数**（放 `nodes/mod.rs` 或 `nodes/struct_node.rs`）：
- `create_class(cls: &Bound<PyType>) -> PyResult<Bound<PyAny>>`：参考 pydantic-core
  model.rs:348-365
- `force_setattr(py, obj, name, value) -> PyResult<()>`：参考 pydantic-core
  model.rs:381-394

**需要修改的 StructNode 字段**：
- 移除：`object_new: Py<PyAny>`（R2）
- 新增：`field_count: usize`（R3，用于 Presized）
- 保留：`fields`, `cls`, `has_post_init`

**build 路径**：**不修改**（已符合 §0，详见 §3.8）。

---

*本设计修订（**R3，2026-06-22**）经 PM 确认后，由 ARCH 更新 `docs/架构设计.md`
相关章节，再由 DEV 实施。R3 将实例构造策略从方案 B 升级为方案 B'（与 pydantic-core
对齐），移除 `object_new` 缓存（改用 `create_class` 辅助函数），新增 §0 原则对照表
和 build 路径评估。R3 保留 R2 的全部修正（P2 `__slots__` 编译期报错、N1-N5）。

**R3 修正（B1-R3，同日）**：修正 `force_setattr` 误用 pyo3 0.23+ API 的问题
（`IntoPyObject` / `into_pyobject_or_pyerr` / `py_error_on_minusone` →
`IntoPy<Py<PyAny>>` / `into_py` / 手动 `== -1` 检查），修正 `create_class`
首参 cast 缺失，修正 `PyDict::new` → `new_bound`。纳入 REV 非阻塞建议 N4-R3
（`slots=True` 时序限制 + 运行期兜底）、N6-R3（`StructNode::new` 签名变更影响测试）。*
