# Phase 5 候选 B ABI3 兼容性 Spike 分析

> **目的**：回应 5.R 设计检视 P0-1——候选 B（raw FFI 热路径）的 ABI3 兼容性未验证。
> 本文档基于**现有源码实证分析** + **CPython ABI3 规范**，给出候选 B 的可行性结论。
>
> **作者**：ARCH（5.1 v2 设计修正）
> **日期**：2026-07-29
> **方法**：源码审查（`error.rs` / `instance.rs` raw FFI 先例）+ CPython 稳定 ABI 规范对照，**不修改任何 src/ 文件**（ARCH 权限限制）

---

## 1. 背景澄清：ABI3 是本项目的真实约束

### 1.1 项目为何使用 ABI3

来源：`construct-rs/src/instance.rs:24-37` + `plans/phase1-foundation/过程记录.md:3005`。

```rust
//! 本项目通过 `PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1` 编译（详见 conftest.py），
//! 启用 `Py_LIMITED_API`。此模式下：
//!
//! - `ffi::PyTypeObject` 为 opaque（不可直接访问 `tp_new` 字段），必须用
//!   `PyType_GetSlot(type, Py_tp_new)` 间接获取槽位（ABI3 稳定 API）。
```

**根因**：开发环境 Python 3.14 vs pyo3 0.22.6（最高原生支持 3.13），必须设
`PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1` 才能 cargo build。**这是构建环境硬约束，
不仅是 wheel 分发需求**。

### 1.2 设计文档原描述（§3.2.3）的准确性

设计 §3.2.3 / §8.3 BC-B3 / §12.2 第 2 项均正确识别 ABI3 约束。REV P0-1 指出的
"未验证"问题，本质是：**"raw C 函数注册到 pyclass 类型方法表"这一具体路径未验证**，
而非"ABI3 下能否用 raw C API"（后者已被 error.rs 先例证明可行）。

---

## 2. 现有 raw FFI 先例审查（error.rs / instance.rs）

### 2.1 已验证可用的 ABI3 稳定 API（项目内先例）

来源：`construct-rs/src/error.rs:740-825`（`try_fast_path_alloc`）+ 
`construct-rs/src/instance.rs:72-97`（`create_class`）+ 
`construct-rs/src/instance.rs:120-143`（`force_setattr`）。

| CPython C API | 项目内使用位置 | ABI3 稳定性 | 用途 |
|---------------|---------------|-------------|------|
| `PyType_GenericAlloc` | `error.rs:761` | ✅ 稳定 ABI（Python 3.2+） | 分配异常实例 |
| `PyObject_SetAttrString` | `error.rs:774,794,812` | ✅ 稳定 ABI | 设置实例属性（message/path/args） |
| `Py_DecRef` | `error.rs:778,796,814` | ✅ 稳定 ABI | 引用计数递减 |
| `PyErr_Clear` | `error.rs:764,777,795,813` | ✅ 稳定 ABI | 清除挂起异常 |
| `PyErr_SetString` | `error.rs` 多处 | ✅ 稳定 ABI | 设置异常 |
| `Py_None()` | `error.rs:788` | ✅ 稳定 ABI | 获取 None 单例 |
| `PyType_GetSlot(type, Py_tp_new)` | `instance.rs:78` | ✅ 稳定 ABI（Python 3.4+） | 读取 tp_new 函数指针 |
| `PyObject_GenericSetAttr` | `instance.rs:136` | ✅ 稳定 ABI | 绕过自定义 `__setattr__` |
| `c_str!` 宏 | `error.rs` 多处 | ✅ pyo3 0.22 提供 | C 字符串字面量 |

**关键结论**：候选 B 的 5 个 unsafe 块（设计 §3.3 SAFETY-1~5）所需的全部 C API
（`PyArg_ParseTuple` / `PyBytes_AsStringAndSize` / `PyObject_SetAttrString` /
`PyErr_SetString` / `Py_DecRef` / `PyErr_Clear` / `Py_None`）**均为 ABI3 稳定 API**，
且项目已有先例证明可用。**SAFETY-1~5 本身无 ABI3 风险**。

### 2.2 候选 B 的真正 ABI3 风险点：类型方法注册

REV P0-1 的核心担忧：raw C 函数 `parse_raw_fast(self_, args)` 如何让 Python 侧
`schema._parse_raw_fast(data)` 自动绑定 `self_ = schema`。

**ABI3 限制下的三条候选路径**：

#### 路径 A：PyType_FromSpec 重新定义类型（设计 §3.2.3 提及，❌ 不可行）

- ABI3 稳定 API 有 `PyType_FromSpec`，但它**创建新类型**，不修改现有类型
- CompiledSchema 已由 pyo3 `#[pyclass]` 宏注册，用 `PyType_FromSpec` 会创建**第二个**类型对象
- 与 pyo3 的类型注册冲突 → **设计文档已正确排除此路径**

#### 路径 B：直接修改 pyclass 类型的 tp_methods 槽（❌ ABI3 不可行）

- 非 ABI3 模式下可直接读 `PyTypeObject.tp_methods` 字段并修改
- ABI3 模式下 `PyTypeObject` 为 opaque，无 API 可动态修改已注册类型的 `tp_methods`
- `PyType_GetSlot` 只读，无 `PyType_SetSlot` → **此路径被 ABI3 阻断**

#### 路径 C：PyObject_SetAttrString 挂载 PyCFunction 到类型对象（⚠️ 需仔细分析）

```rust
// 伪代码：
let method_def = ffi::PyMethodDef { ml_name: ..., ml_meth: Some(parse_raw_fast), ... };
let py_cfunc = unsafe { ffi::PyCFunction_New(&method_def, schema_ptr) };
ffi::PyObject_SetAttrString(schema_type_ptr, "_parse_raw_fast", py_cfunc);
```

**ABI3 兼容性**：`PyCFunction_New` ✅ 稳定 ABI；`PyObject_SetAttrString` ✅ 稳定 ABI。

**但存在语义陷阱**（设计 §3.2.3 + REV P0-1 第 2 点未深究的核心问题）：

- `PyCFunction_New` 创建的 PyCFunctionObject 是**自由函数**，不是描述符
- Python 描述符协议（`__get__`）**不会**对 PyCFunction 触发 self 绑定
- 调用 `schema._parse_raw_fast(data)` 时：
  - LOAD_ATTR 命中类属性 `_parse_raw_fast`（PyCFunction 对象）
  - PyCFunction **不是描述符**，直接返回（不包装为 bound method）
  - CALL 时：CPython 调用 `PyCFunction_Call(func, args, kwargs)`
  - 对于 `METH_VARARGS` 签名，CPython 调用 `func(self_module, args)`
  - **`self_module` 是 PyCFunctionObject 的 `m_self` 字段**（创建时由 `PyCFunction_New` 第二参传入）

**关键发现**：如果用 `PyCFunction_NewEx(methoddef, schema_ptr, NULL)` 并把 `schema_ptr`
作为 `m_self`，则调用 `schema._parse_raw_fast(data)` 时，CPython 会把 `schema_ptr`
作为第一个参数传给 C 函数！

**等等——这不对**。让我重新核实 CPython 源码语义：

实际行为（CPython `Objects/methodobject.c` + `call.c`）：
- `instance.method(args)` 查找 `method` → 找到 PyCFunctionObject（在类上）
- CPython `method_call` / `vectorcall` 路径：对于类属性中的 PyCFunction，
  **`instance` 不会自动作为 self 传入**——因为 PyCFunction 的 `m_self` 字段
  在创建时已固定（`PyCFunction_New` 第二参）
- 调用 `instance.method(args)` 实际等价于 `PyCFunction(m_self, args)`，
  其中 `m_self` 是创建时传入的对象

**这意味着**：
- 若 `PyCFunction_New(methoddef, schema_ptr)` 创建时绑定 `m_self = schema_ptr`
- 则 `any_instance._parse_raw_fast(data)` 都会调用 `parse_raw_fast(schema_ptr, data)`
- **self 永远是 schema_ptr，无论通过哪个实例访问**——这正是候选 A + B 想要的语义！

**但有限制**：
- PyCFunction 的 `m_self` 是**创建时固定的**，所有共享同一 PyCFunction 的实例都拿到同一个 self
- CompiledSchema 是 frozen pyclass，每个用户类对应一个 schema 实例
- 因此每个用户类需要**独立的** PyCFunction 对象（m_self 绑定到该类的 schema）
- 这是 per-class 一次性开销（编译期），不是 per-call 开销 ✓

#### 路径 D：模块级函数 + 候选 A functools.partial 绑定（✅ 完全可行，REV P1-4 提及）

- raw C 函数作为模块级函数注册（`#[pyfn]` 或 `PyModule_AddObject`）
- 候选 A 的 `_install_fast_call_bindings` 用 `functools.partial` 绑定 schema
- ABI3 完全兼容（无类型方法表操作）
- **代价**：functools.partial 调用开销 ~10-15ns，抵消候选 B 部分收益

---

## 3. 可行性矩阵

| 路径 | ABI3 兼容 | 与 pyo3 共存 | 自绑定语义 | 实施复杂度 | 净收益（vs pyo3 wrapper） |
|------|----------|-------------|-----------|-----------|------------------------|
| A. PyType_FromSpec | ✅ | ❌ 冲突 | — | — | **不可行** |
| B. 改 tp_methods | ❌ opaque | — | — | — | **不可行** |
| C. PyCFunction_New + SetAttr 类型 | ✅ | ✅ | ⚠️ 需验证 | 中（~50 行 unsafe） | 15-35ns（需 spike 验证 m_self 语义） |
| D. 模块级 + functools.partial | ✅ | ✅ | ✅（partial 绑定） | 低（~30 行） | 5-20ns（partial 抵消部分） |
| E. B-conservative（#[pymethods] + 内部 raw C API） | ✅ | ✅ | ✅（pyo3 处理） | 极低（~20 行） | 5-10ns |

---

## 4. 路径 C 的关键技术风险（必须 DEV spike 验证）

虽然路径 C 在理论上可行（`PyCFunction_New` + `m_self` 绑定），但以下三点必须在
开发实施前用 minimal spike 验证（20-30 行 Rust + Python 测试）：

### 风险 1：pyclass 类型对象能否 PyObject_SetAttrString

- pyo3 `#[pyclass]` 生成的类型是否允许 setattr 新属性？
- Python 默认 type 对象允许 setattr（通过 `type.__setattr__`）
- 但 pyo3 可能设置 `Py_TPFLAGS_HAVE_GC` / `tp_setattro` 特殊处理
- **Spike 步骤**：模块初始化时对 CompiledSchema 类型 setattr 一个测试属性，
  Python 侧 `CompiledSchema._test_attr` 是否可读

### 风险 2：PyCFunction m_self 绑定后的调用语义

- `PyCFunction_NewEx(methoddef, schema_ptr, NULL)` 创建 PyCFunction
- setattr 到类型对象后，`schema._parse_raw_fast(data)` 的实际调用路径
- CPython 3.14 的 vectorcall 是否对类属性上的 PyCFunction 做内联优化？
- **Spike 步骤**：创建一个 minimal raw C 函数 `test_fast(self_, args) -> self_`，
  setattr 到 CompiledSchema 类型，Python 调用验证 self_ 是 schema_ptr

### 风险 3：PyCFunction 与 pyclass __getattribute__ 的交互

- CompiledSchema 是 `#[pyclass(frozen)]`，pyo3 可能自定义 `tp_getattro`
- 通过实例访问 `_parse_raw_fast` 时，pyo3 的属性查找是否优先于类型 dict？
- **Spike 步骤**：验证 `schema._parse_raw_fast` 与 `type(schema)._parse_raw_fast` 是同一对象

---

## 5. ABI3 Spike 计划（供 DEV 实施第一步执行）

### 5.1 Spike 代码骨架（DEV 实现，~40 行）

**位置**：`construct-rs/src/schema_ffi_spike.rs`（临时 spike 文件，验证后删除）

```rust
use pyo3::ffi;
use pyo3::prelude::*;
use std::os::raw::{c_char, c_int};

/// Spike：minimal raw C 函数，返回 self_ 指针（验证 m_self 绑定）。
pub unsafe extern "C" fn _spike_identity(
    self_: *mut ffi::PyObject,
    _args: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    // 直接 incref self_ 并返回（验证 self_ 是 schema_ptr）
    unsafe { ffi::Py_INCREF(self_) };
    self_
}

/// Spike：注册到 CompiledSchema 类型 + 测试。
pub fn register_spike(py: Python<'_>, schema_type_ptr: *mut ffi::PyObject) -> PyResult<()> {
    static SPIKE_METHOD: ffi::PyMethodDef = ffi::PyMethodDef {
        ml_name: b"_spike_identity\0".as_ptr() as *const c_char,
        ml_meth: ffi::PyMethodDef__bindgen_ty_1 {
            _unnamed_at_1: Some(_spike_identity),
        },
        ml_flags: ffi::METH_VARARGS as c_int,
        ml_doc: b"spike\0".as_ptr() as *const c_char,
    };

    // 用 PyCFunction_NewEx 绑定 m_self = 一个固定的 sentinel（这里用 Py_None 测试）
    // 真实场景：m_self 应绑定具体的 CompiledSchema 实例
    let sentinel = unsafe { ffi::Py_None() };
    let method = unsafe { ffi::PyCFunction_NewEx(&SPIKE_METHOD, sentinel, std::ptr::null()) };
    if method.is_null() {
        return Err(pyo3::exceptions::PyRuntimeError::new_err("PyCFunction_NewEx failed"));
    }

    // setattr 到类型对象
    let rc = unsafe {
        ffi::PyObject_SetAttrString(
            schema_type_ptr,
            b"_spike_identity\0".as_ptr() as *const c_char,
            method,
        )
    };
    unsafe { ffi::Py_DecRef(method) };
    if rc == -1 {
        return Err(pyo3::exceptions::PyRuntimeError::new_err("SetAttrString failed"));
    }
    Ok(())
}
```

**Python 侧验证**（`experiments/phase5_abi3_spike/verify_spike.py`）：

```python
from construct._construct_rust import CompiledSchema

# 1. 验证 setattr 成功
assert hasattr(CompiledSchema, "_spike_identity"), "SetAttr to type failed"

# 2. 验证 m_self 绑定（返回 Py_None sentinel）
result = CompiledSchema._spike_identity()
assert result is None, f"Expected None (sentinel), got {result!r}"

# 3. 验证通过实例访问也返回 sentinel（非 instance）
schema = ...  # 任一已编译的 schema
result = schema._spike_identity()
assert result is None, "m_self binding failed through instance"

print("SPIKE PASS: PyCFunction m_self 绑定在 ABI3 下工作正常")
```

### 5.2 Spike 结果判定

| Spike 结果 | 候选 B 决策 | B1 收益预估 |
|-----------|-----------|-----------|
| 三项全部 PASS | 路径 C 可行，B-aggressive 实施 | 15-35ns |
| 风险 1 FAIL（type 不允许 setattr） | 路径 C 不可行，改路径 D 或 B-conservative | 5-20ns（D）/ 5-10ns（B-conservative） |
| 风险 2 FAIL（m_self 未绑定） | 路径 C 不可行，改路径 D 或 B-conservative | 同上 |
| 风险 3 FAIL（pyclass 拦截 getattr） | 路径 C 不可行，改路径 D | 5-20ns |

---

## 6. B-conservative 详细设计（回退方案，P0-1 要求）

若路径 C spike 失败，候选 B 降级为 B-conservative。

### 6.1 接口签名（无变化）

`CompiledSchema::_parse_raw` 保持现有 `#[pymethods]` 签名（`schema.rs:148-165`）。

### 6.2 内部实现改动（~20 行）

```rust
#[pyo3(signature = (data))]
pub fn _parse_raw<'py>(
    &self,
    py: Python<'py>,
    data: &Bound<'py, PyBytes>,
) -> PyResult<Bound<'py, PyAny>> {
    // B-conservative：用 raw C API 替代 pyo3 高级抽象
    // 1. PyBytes_AsStringAndSize 替代 data.as_bytes()（省 pyo3 借用校验 ~3-5ns）
    // 2. 其余逻辑不变（仍用 pyo3 的 py: Python<'py> token）
    let data_ptr = data.as_ptr();
    let mut buf: *const u8 = std::ptr::null();
    let mut size: ffi::Py_ssize_t = 0;
    // SAFETY: data 是 Bound<PyBytes>，as_ptr() 返回有效 PyObject 指针。
    // PyBytes_AsStringAndSize 是 ABI3 稳定 API。
    let ok = unsafe {
        ffi::PyBytes_AsStringAndSize(
            data_ptr,
            &mut buf as *mut *const u8 as *mut *mut c_char,
            &mut size,
        )
    };
    if ok == -1 {
        return Err(PyErr::fetch(py));
    }
    let bytes = unsafe { std::slice::from_raw_parts(buf, size as usize) };

    let mut stream = ParseStream::new(bytes);
    let mut ctx = Context::placeholder(py);
    let mut path = Path::new();
    let result = self.root.parse(py, &mut stream, &mut ctx, &mut path)?;
    Ok(result.into_bound(py))
}
```

### 6.3 B-conservative 收益重新评估

| 优化点 | pyo3 wrapper 原开销 | B-conservative 后 | 净节省 |
|--------|-------------------|------------------|-------|
| PyBytes 借用校验（pyo3 内部） | ~3-5ns | ~1ns（直接 C API） | 2-4ns |
| pyo3 参数解析簿记 | ~5-8ns | ~2ns | 3-6ns |
| pyo3 返回值包装 | ~5-10ns | 不变（仍走 PyResult） | 0ns |
| **合计** | | | **5-10ns** |

**B-conservative 比 B-aggressive 收益低 ~10-25ns**，但 ABI3 风险为零。

### 6.4 B-conservative 对 B1 加速比影响

- B1 rs_ns 基线：386ns
- B-conservative 节省：5-10ns
- B1 rs_ns 优化后：376-381ns
- B1 加速比：3123/376 = 8.31x ~ 3123/381 = 8.20x
- **单独 B-conservative 不达标**，需 A + C 叠加

---

## 7. 结论与建议

### 7.1 ABI3 兼容性结论

- **SAFETY-1~5 所需的全部 C API 在 ABI3 下可用**（项目已有先例：error.rs / instance.rs）
- **类型方法注册路径 C（PyCFunction_New + SetAttr）理论可行但需 spike 验证 3 个风险点**
- **路径 D（模块级 + functools.partial）完全可行但收益降低**
- **B-conservative 完全可行，零 ABI3 风险，收益 5-10ns**

### 7.2 给 PM / DEV 的建议

1. **5.3 子任务（候选 B）实施顺序调整**：
   - **第一步必须是 spike**（§5.1 代码骨架，~40 行 + Python 验证脚本）
   - Spike PASS → 路径 C（B-aggressive，15-35ns）
   - Spike FAIL → 路径 D（模块级 + partial，5-20ns）或 B-conservative（5-10ns）

2. **设计文档修正方向**：
   - §3.2.3 补充本 spike 分析的 4 条路径 + 可行性矩阵
   - §3.2.4 新增 spike 计划（§5.1）
   - §3.2.5 新增 B-conservative 详细设计（§6）
   - §7.1/§7.2 候选 B 收益预估改为"取决于 spike 结果"：
     - 上界（路径 C 成功）：15-35ns
     - 中位（路径 D）：5-20ns
     - 下界（B-conservative）：5-10ns

3. **B1 加速比修正**（基于候选 B 不确定 + 候选 C 收益下调）：
   - 详见设计文档 §7.2 修正后估算

---

## 8. 引用证据

| 来源 | 内容 | 用途 |
|------|------|------|
| `construct-rs/src/error.rs:740-825` | `try_fast_path_alloc` raw FFI 先例 | §2.1 ABI3 稳定 API 已验证 |
| `construct-rs/src/instance.rs:24-37,72-97` | ABI3 兼容性注释 + `PyType_GetSlot` 用法 | §1.1 + §2.1 |
| `construct-rs/Cargo.toml:23` | `pyo3 = "0.22"` 未启用 abi3 feature | §1.1 ABI3 由环境变量触发 |
| `plans/phase1-foundation/过程记录.md:3005` | ABI3 限制适配记录 | §1.1 |
| CPython `Objects/methodobject.c` + `Objects/call.c` | PyCFunction m_self 语义 | §2.2 路径 C 分析 |
| CPython 稳定 ABI 文档（docs.python.org/3/c-api/stable.html） | 各 API 的 ABI3 稳定性 | §2.1 + §3 |
