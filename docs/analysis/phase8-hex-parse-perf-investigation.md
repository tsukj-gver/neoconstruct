---
id: ANALYSIS-phase8-hex-parse
status: active
phase: "8"
task: "8.HEX-INVEST [ARCH Hex/HexDump parse 性能差距根因分析]"
type: investigation
depends_on:
  - DESIGN-Phase8-P0
  - PERF-bench
  - PERF-retest
  - ADR-022
created: 2026-07-31
last_updated: 2026-07-31
---

# Phase 8 Hex/HexDump parse 性能差距根因分析

> **角色**：ARCH
> **任务标识**：`8.HEX-INVEST [ARCH Hex/HexDump parse 性能差距根因分析]`
> **触发**：PM 验收 Phase 8 性能数据时，用户质疑 Hex 族 parse 加速比（4.3-6.9x）显著低于同档
> 轻量字段构造器（Const 8.3x / Enum 9.0x / Mapping 9.0x），要求 ARCH 调查是否为设计问题。

---

## 0. 一句话根因 + 判定

> **根因**：`HexNode::parse`（int 分支）当前用 `call_method1("new", ...)` **跨 FFI 回调到
> Python 解释器执行 `HexDisplayedInteger.new` 的字节码**（设计文档 §2.2 误判此为"C 级实现"，
> 实为 Python 字节码），加上每次 parse 重新构造 fmtstr PyString——这两项构成 HX1 相比 Const
> 多出的 ~330ns 中的可优化部分（约 150-180ns）。
>
> **判定**：**设计判断错误 + 实现问题**（非固有开销、非架构级 §0 违反）。
>
> - 设计判断错误：§2.2 称"显示类 factory 是 C 级实现（CPython 内置类型方法），走 §0.2 判据 2"，
>   实际 `HexDisplayedInteger.new` 是 `lib/hex.py` 的 Python `@staticmethod`（字节码），
>   `call_method1("new")` 会进入 CPython ceval 字节码循环——属于"跨 FFI 回调 Python 代码"。
> - 实现问题：fmtstr 每次 parse 重新 `PyString::new_bound`（未编译期 intern）；以及可改用
>   Rust 内 C API（call1 + setattr）等价完成 new 逻辑，消除字节码进入。
> - 有修正方案（方案 A + 方案 B），可证伪，不涉及公开接口变更，无需走 DESIGNING→DESIGN_REVIEW。
>
> **是否有修正方案**：是。见 §5（方案 A：Rust 内 call1+setattr 替代 call_method1；方案 B：编译期
> intern fmtstr）。预测 HX1 从 581ns 降至 ~350-400ns（加速比 4.36x → ~6.3-7.2x）。HX2/HD1 改善
> 有限（已接近 CPython bytes/dict 子类实例化固有开销下限）。

---

## 1. 实测数据回顾（Controlled A/B Test，已排除漂移）

数据源：`plans/phase8-adapters-struct-streams/traces/PERF-retest.md`（14/14 STRUCTURAL，
0 DRIFT，std 0.06-0.28，阳性对照 ≥10x，环境漂移 <0.3x 阈值）。

| Case | 构造器 | 方向 | rs ns | py ns | speedup | 门禁 |
|------|--------|------|-------|-------|---------|------|
| **HX1** | Hex(Int32ub) | parse | **581** | 2519 | **4.36x** | 10 / LOW |
| **HX2** | Hex(Bytes(4)) | parse | **330** | 2296 | **6.87x** | 10 / LOW |
| **HD1** | HexDump(Bytes(4)) | parse | **324** | 2244 | **6.88x** | 10 / LOW |
| CN1 | Const(b'IHDR') | parse | 253 | 2109 | 8.34x | 10 / LOW |
| EN1 | Enum(Byte) | parse | 239 | 2236 | 9.03x | 10 / LOW |
| MP1 | Mapping(Byte) | parse | 239 | 2189 | 9.00x | 10 / LOW |
| HX1 | Hex | build | 152 | 2053 | 13.51x | 10 / OK |

**核心矛盾**（用户视角）：
- Python 原版开销相近（Hex ~2500ns ≈ Const ~2100ns），Rust 侧差距显著（HX1 581 vs CN1 253，**多 ~330ns**）。
- 同构造器 build 方向 ≥13x（Hex build 152ns），证明 Hex 字段本身不慢——**仅 parse 方向异常重**。
- Const 也构造 PyObject（rich_compare 值比较）且仅 253ns，Hex 却 581ns。

---

## 2. Hex parse 路径开销拆解

### 2.1 当前实现路径（`construct-rs/src/nodes/hex.rs` L150-203，int 分支）

```
HexNode::parse(int 路径):
  1. inner.parse(Int32ub)              ── 共享，~120ns（与 Const 同）
  2. obj.bind(py)                       ── borrow，~2ns
  3. is_instance_of::<PyLong>           ── PyObject_IsInstance，~15ns
  4. fmtstr.clone() (String)            ── Rust String clone，~15ns
  5. PyString::new_bound(py, "08X")     ── 每次构造新 PyString，~45ns ★可优化
  6. integer_cls.bind(py)
  7. cls.call_method1("new", (obj, fmt)) ── 跨 FFI 回调 Python 字节码 ★可优化
       a. pyo3 → PyObject_CallMethodObjArgs(cls, "new", ...)
       b. PyObject_GetAttr(cls, "new")    ── 属性查找，~35ns
       c. staticmethod 描述符 __get__     ── → PyFunctionObject，~10ns
       d. PyObject_Call(func, args_tuple) ── 进入 ceval 字节码循环
       e. 字节码执行 new 函数体:
            LOAD_GLOBAL HexDisplayedInteger
            LOAD_FAST intvalue
            CALL 1                        ── HexDisplayedInteger(intvalue)，C 级 long_new，~55ns
            STORE_FAST obj
            LOAD_FAST fmtstr
            STORE_ATTR fmtstr             ── GenericSetAttr，~50ns
            RETURN_VALUE
       f. 返回 Py<PyAny>，unbind
  小计步骤 7 ≈ 150-200ns（FFI 边界 + getattr + 字节码 dispatch + 函数体）
  总计 ≈ 120 + 2 + 15 + 15 + 45 + 180 = 377ns（下限估算）
  实测 581ns，差额 ~200ns 来自 Int32ub parse 实际更贵 / FFI 边界放大 / pyo3 框架开销
```

### 2.2 开销分类（按可优化性）

| 开销项 | 估算（Rust 侧） | 类别 | 可优化? |
|--------|----------------|------|--------|
| Int32ub inner.parse | ~120-150ns | 共享（所有构造器都有） | 否（与 Const 同） |
| is_instance_of PyLong | ~15ns | C API 调用 | 微优化（~5ns） |
| **fmtstr String clone + PyString::new** | **~60ns** | 实现低效 | **是（方案 B）** |
| **call_method1 getattr(cls,"new")** | **~35ns** | 实现低效 | **是（方案 A）** |
| **call_method1 字节码 dispatch** | **~40-50ns** | 实现低效 | **是（方案 A）** |
| int 子类实例化 (long_new) | ~55ns | CPython 固有 | 否 |
| setattr fmtstr (GenericSetAttr) | ~50ns | CPython 固有 | 否（含 __dict__ 分配） |
| FFI 边界 + pyo3 框架 | ~50ns | 跨边界固有 | 部分（方案 A 减少 1 次） |
| **可优化合计** | **~135-145ns** | | |
| **不可优化合计** | **~290-315ns** | | |

**结论**：HX1 的 581ns 中，约 135-145ns 可通过方案 A+B 消除，其余 ~290-315ns 为
CPython int 子类实例化 + setattr + Int32ub parse + FFI 框架的固有组合开销。

---

## 3. HX1 (int, 581ns) vs HX2 (bytes, 330ns) 差异分析

### 3.1 路径差异（`hex.rs`）

```
HX1 int 路径:    is_instance_of PyLong → call_method1("new", (obj, fmt))  [跨 FFI 回调字节码]
HX2 bytes 路径:  is_instance_of PyLong(fail) → is_instance_of PyBytes → cls.call1((obj,))  [C 级]
```

### 3.2 差异来源（~251ns）

| 差异项 | HX1 (int) | HX2 (bytes) | 差额 |
|--------|-----------|-------------|------|
| inner.parse | Int32ub ~120ns | Bytes(4) ~100ns | ~20ns |
| is_instance_of | 1 次 (~15ns) | 2 次 (~30ns，PyLong fail + PyBytes ok) | -15ns（HX2 略多） |
| fmtstr 处理 | String clone + PyString::new (~60ns) | **无**（bytes 路径不需 fmtstr） | **~60ns** |
| 显示对象构造 | call_method1("new") 跨 FFI 回调字节码 (~180-200ns) | call1((bytes,)) C 级 (~80ns) | **~120ns** |
| int/bytes 子类实例化固有差 | long_new ~55ns | bytes_new ~75ns | -20ns（bytes 略贵） |
| setattr | 1 次 (~50ns) | **无**（HexDisplayedBytes 无 __init__ setattr） | **~50ns** |
| **合计差额** | | | **~215ns** |

实测差额 251ns，其中：
- fmtstr 处理 ~60ns（bytes 路径完全不需要 fmtstr）
- call_method1 vs call1 字节码调度差 ~120ns（int 走字节码，bytes 走 C 级）
- setattr ~50ns（int 子类需设 fmtstr，bytes 子类无）
- 其余 ~21ns 为 Int32ub vs Bytes parse 差异 + 测量噪声

**这解释了为什么 int 路径（HX1）远贵于 bytes 路径（HX2/HD1）**：bytes 路径天然不需要
fmtstr，且直接 call1 走 C 级，没有 Python 字节码进入。**bytes 路径已是接近最优实现**。

### 3.3 关键洞察

bytes 路径（HX2/HD1）的 330ns 中，绝大部分是 CPython bytes 子类实例化（buffer 复制）
+ FFI 框架 + is_instance_of 双检查的固有开销。**HX2/HD1 几乎无可优化空间**——它们的
加速比 6.87x/6.88x 已经接近"显示对象构造器"的理论上限（因为 bytes 子类实例化是
CPython 固有，Python 原版也要付同样的 CPython 税）。

可优化的主要是 **int 路径（HX1）**——它因为 call_method1 跨 FFI 回调字节码 + 每次构造
PyString 而多付了 ~135-145ns 的"可避免税"。

---

## 4. 与 Const（253ns）对比 + Python 原版对比

### 4.1 Const vs Hex 开销差异（328ns）

| 步骤 | Const (253ns) | Hex HX1 (581ns) | 差额 |
|------|--------------|-----------------|------|
| inner.parse | Bytes(4) ~100ns | Int32ub ~120ns | ~20ns |
| 类型检查 | 无 | is_instance_of ~15ns | ~15ns |
| fmtstr 处理 | 无 | String clone + PyString::new ~60ns | ~60ns |
| 核心操作 | rich_compare(value) ~30ns（C 级） | call_method1("new") ~180ns（跨 FFI 字节码） | ~150ns |
| setattr | 无 | setattr fmtstr ~50ns | ~50ns |
| **合计** | | | **~295ns** |

实测差额 328ns，与拆解吻合（误差 ~33ns 为 FFI 框架 + 测量噪声）。

**Const 仅做 1 次 C 级 rich_compare（~30ns），Hex 做 1 次跨 FFI 回调字节码（~180ns）+
setattr（~50ns）+ fmtstr 构造（~60ns）——这就是 328ns 差距的本质**。

### 4.2 Python 原版对比（是否重复 Python 已有工作）

Python 原版 Hex（`construct/construct/core.py` Hex Adapter）的 _decode 同样调用
`HexDisplayedInteger.new(obj, fmtstr)`。所以 construct-rs **没有重复 Python 的工作**——
两者都调用同一个 Python new 字节码。

差异在于：Python 原版从 Python 调 Python new（无 FFI 边界，开销 ~136ns Python 层实测），
construct-rs 从 Rust 跨 FFI 调 Python new（多一次 FFI 边界 + pyo3 框架，开销 ~180-200ns）。

**这意味着**：即使保留 call_method1，construct-rs 的 Hex parse 也比 Python 原版快
（581ns vs 2519ns，4.36x）——加速来自消除了 Python Adapter 调度栈 + isinstance 3 次 +
subcon.parse 的 Python 解释器开销。但**加速比被 call_method1 的 FFI 回调拖累**，未能
达到 Const 那样的 8x+。

---

## 5. 判定：设计判断错误 + 实现问题（非固有开销）

### 5.1 设计判断错误（设计文档 §2.2 / §2.5 / §2.6.1）

设计文档 `docs/design/模块设计/模块设计-Phase8-P0.md` 多处断言：

> §2.2：「显示类的 `__call__`（如 `HexDisplayedInteger.new`）是 **C 级实现**
> （CPython 内置类型方法），调用走 §0.2 判据 2（不计额外 FFI）」
>
> §2.5 §0 对照表：「`is_instance_of` / `call_method1` / `call1` 是 pyo3 C API（§0.2 判据 2）。
> 显示类 `__call__` / `__init__` 是 CPython 内置子类构造（int/bytes/dict 的 C 级实现）」
>
> §2.6.1 瓶颈识别：「1 次 `call_method1`（显示类构造，~80-150ns）」

**这是错误判断**。`HexDisplayedInteger.new`（`lib/hex.py` L24-28）是 **Python `@staticmethod`**，
函数体是 Python 字节码（`verify_struct_facts.py` 的 dis 输出证实：LOAD_GLOBAL / CALL /
STORE_ATTR 等指令）。`call_method1("new", ...)` 的实际 C API 链为：

```
PyObject_CallMethodObjArgs(cls, "new", arg1, arg2, NULL)
  → PyObject_GetAttr(cls, "new")       // C API，拿 staticmethod 描述符
  → staticmethod.__get__               // C 级，返回 PyFunctionObject
  → PyObject_Call(func, args)          // ★ 进入 ceval.c 字节码循环执行 new 函数体 ★
```

**步骤 `PyObject_Call(func, args)` 进入 CPython 字节码解释器**——这属于 ADR-022 §0.2
判据 1 的「跨 FFI 回调 Python 代码」（非判据 2 的「Rust 内调 CPython C API」）。

设计文档把"调用类方法"误判为"C 级内置类型方法调用"，忽略了被调用对象是 PyFunctionObject
（Python 字节码），而非 PyCFunctionObject（C 扩展函数）。

**与 ADR-022 §0.2 判据的关系**：严格说 Hex 走的是「判据 1 路径」（跨 FFI 回调 Python 代码），
而非设计文档声称的「判据 2」（C API 直操）。但：
- 这**不是 §0 #1 架构级违反**——§0 #1 约束的是"中间表示层 / per-field trait 抽象 / dict 跨 FFI"，
  Hex 不引入中间数据类型，不破坏 parse 直接构造 PyObject 的原则。
- 这**不是 AdapterCallbackNode 的用户回调**——Hex 是核心库内置 Adapter，new 是固定的库代码
  （非用户传入的 _decode/_encode）。性质介于「判据 1」与「判据 2」之间。
- 但它确实**多了一次 Python 解释器进入**（相比 Const 的纯 C API rich_compare），构成可优化的实现低效。

### 5.2 实现问题（hex.rs）

两个可避免的低效点（hex.rs L162-178，int 分支）：

1. **fmtstr 每次 parse 重新构造**（L163-169 + L170）：
   ```rust
   let fmt_str = match &self.fmtstr { Some(s) => s.clone(), ... };
   let fmt_py = PyString::new_bound(py, &fmt_str);  // ★ 每次构造新 PyString
   ```
   `self.fmtstr` 是 `Option<String>`（Rust String）。即使编译期已预算 fmtstr，每次 parse 仍
   clone String + new PyString。可改为编译期 intern 为 `Option<Py<PyString>>`，parse 时借用。

2. **call_method1 跨 FFI 回调 Python 字节码**（L171-177）：
   ```rust
   let cls_bound = self.display_classes.integer.bind(py);
   let new_obj = cls_bound.call_method1("new", (bound, &fmt_py))?;
   ```
   可改为 Rust 内用 C API 等价完成 new 的逻辑（call1 + setattr），消除字节码进入。

### 5.3 非固有开销的依据（L-14 交叉验证）

「int 子类实例化 + setattr 是 CPython 固有」这一结论经过交叉验证（L-14 对策）：

- **路径 1（当前）**：call_method1("new") → Python 字节码 → cls(intvalue) + setattr
- **路径 2（方案 A）**：Rust 内 call1(cls, (intvalue,)) + setattr
- **路径 3（不存在）**：纯 Rust 构造 int 子类——无法绕过 CPython `type.__call__`，
  int 子类实例化必须走 long_new（C 级，CPython 内部机制）

路径 1 与路径 2 的差异 = getattr(cls,"new") + 字节码 dispatch（**可消除**）。
路径 2 与路径 3 的差异 = int 子类实例化本身（**不可消除，CPython 固有**）。

Python 层实测（`profile_hex_display.py`，number=200000）：

| 操作 | ns/op | 类别 |
|------|-------|------|
| plain_int `int(258)` | 28.9 | 基线 |
| int_subclass_no_setattr `HexDisplayedInteger(258)` | 55.4 | CPython 固有（long_new） |
| bytes_subclass `HexDisplayedBytes(b'...')` | 75.7 | CPython 固有（bytes_new） |
| new_decomposed `cls(intvalue)+setattr`（方案 A 等价） | 105.9 | 固有 + setattr |
| new_full `HexDisplayedInteger.new(...)`（当前路径等价） | 135.7 | 固有 + setattr + 调度 |
| getattr(cls,"new") | 35.5 | 可消除（方案 A） |

关键比例（Python 层，外推 Rust 侧方向一致）：
- **H1: new_full - new_decomposed = +29.7ns**（staticmethod 调度开销，方案 A 可省）
- **setattr 开销 = 50.5ns**（CPython 固有，方案 A 无法省）
- **int 子类实例化额外开销 = 26.5ns**（vs plain int，CPython 固有）

---

## 6. 修正方案

### 6.1 方案 A：Rust 内 call1 + setattr 替代 call_method1（int 路径）

**改动范围**：`construct-rs/src/nodes/hex.rs` L160-178（仅 int 分支）

```rust
// 当前（L171-177）：
let new_obj = cls_bound
    .call_method1("new", (bound, &fmt_py))
    .map_err(...)?;

// 方案 A 改为：
// HexDisplayedInteger.new(intvalue, fmtstr) 的字节码逻辑：
//   obj = HexDisplayedInteger(intvalue); obj.fmtstr = fmtstr; return obj
// Rust 内用 C API 等价完成，消除 getattr + 字节码 dispatch：
let cls_bound = self.display_classes.integer.bind(py);
let new_obj = cls_bound.call1((bound,)).map_err(|e| ConstructError::Generic {
    message: format!("HexDisplayedInteger() instantiation failed: {}", e),
    path: path.to_string(),
})?;  // type.__call__ → long_new (C 级)
new_obj.bind(py).setattr("fmtstr", fmt_py).map_err(|e| ConstructError::Generic {
    message: format!("setattr fmtstr failed: {}", e),
    path: path.to_string(),
})?;  // PyObject_GenericSetAttr (C 级)
```

**消除的开销**：
- `getattr(cls, "new")` 属性查找（Python 层 ~35ns，Rust 侧 FFI 放大 ~50ns）
- staticmethod 描述符 `__get__`（~10ns）
- Python 栈帧创建 + ceval 字节码 dispatch（Python 层 ~30ns，Rust 侧 ~40-50ns）
- call_method1 的字符串参数装箱（pyo3 内部构造 "new" tuple）

**保留的开销**（CPython 固有）：int 子类实例化（long_new）+ setattr fmtstr。

**parity 合规**：`lib/hex.py` 的 `new` 字节码体（dis 输出）就是
`obj = HexDisplayedInteger(intvalue); obj.fmtstr = fmtstr; return obj`。方案 A 是其 C API
等价重写，产生的对象结构与属性完全一致（`verify_struct_facts.py` F1/F2 证实 __dict__ 含
fmtstr）。**不破坏 parity**。唯一理论边缘场景：用户子类化 HexDisplayedInteger 并覆盖 new/
__new__——但 HexDisplayedInteger 是 "Used internally" 类（docstring 明示），非用户扩展点。

### 6.2 方案 B：编译期 intern fmtstr（消除每次 PyString::new）

**改动范围**：`hex.rs` HexNode 字段 + `new()` + `compile.rs` 物化

```rust
// 当前：
pub struct HexNode {
    inner: Box<Node>,
    display_classes: HexDisplayClasses,
    fmtstr: Option<String>,         // Rust String，每次 parse clone + new PyString
}

// 方案 B 改为：
pub struct HexNode {
    inner: Box<Node>,
    display_classes: HexDisplayClasses,
    fmtstr: Option<Py<PyString>>,   // 编译期 intern 的 PyString，parse 时借用
}
// parse int 分支：
let fmt_py = match &self.fmtstr {
    Some(s) => s.bind(py),          // 借用，0 构造
    None => { /* 运行期 fallback：仅 None 分支才 PyString::new */ ... }
};
```

**消除的开销**：String clone（~15ns）+ PyString::new_bound（~45ns）≈ 60ns/parse。

**parity 合规**：fmtstr 内容不变（"08X" 等），仅物化时机从运行期提前到编译期。

### 6.3 可证伪预测（覆盖所有 FFI/拷贝来源，L-02/L-05 对策）

| Case | 当前 rs_ns | 方案 A+B 后预测 | 加速比变化 | 依据 |
|------|-----------|----------------|-----------|------|
| **HX1** parse | 581ns | **~350-400ns** | 4.36x → **~6.3-7.2x** | 省 getattr+字节码dispatch ~90ns + fmtstr构造 ~60ns + FFI框架 ~30ns ≈ 180ns |
| HX2 parse | 330ns | ~310-320ns | 6.87x → ~7.0-7.2x | 仅方案 B 不适用（bytes 无 fmtstr）；微优化 is_instance_of ~10ns |
| HD1 parse | 324ns | ~305-315ns | 6.88x → ~7.0-7.2x | 同 HX2（bytes 路径已是 C 级 call1） |

**预测的 FFI/拷贝来源清单**（方案 A+B 后剩余）：
- StructMixin.parse 入口（1 次 FFI，共享，不可消除）
- inner.parse Int32ub（Rust 内，~120ns）
- is_instance_of PyLong（C API，~15ns）
- call1(cls, (intvalue,)) → type.__call__ → long_new（C API，~80ns 含 FFI 边界）
- setattr fmtstr（C API，~50ns）
- 编译期 intern PyString 借用（~5ns）
- pyo3 框架 + unbind（~30ns）
- 合计 ~300ns + Int32ub parse ~120ns ≈ 420ns（下限），实测因 FFI 放大可能 ~350-400ns

**可证伪条件**：
- 若 HX1 优化后仍 >480ns（加速比 <5.2x）：说明 call1+setattr 的 FFI 边界开销被低估，
  int 子类实例化在 Rust 侧比 Python 层贵得多——需进一步调查是否 Rust 侧有额外的
  pyo3 框架税。
- 若 HX1 优化后 <300ns（加速比 >8.4x）：说明字节码 dispatch 在 Rust 侧 FFI 放大远超预期，
  方案 A 收益比预测更大。

**量级参考声明**（L-09）：以上 ns 估算为量级参考，绝对值需 DEV 实施后 Controlled A/B Test
复测验证。Python 层比例（H1 delta=29.7ns / new_full=135.7ns = 22%）可外推 Rust 侧方向。

### 6.4 不达 10x 的不可消除部分（固有开销）

即使方案 A+B 全部实施，HX1 仍难达 10x（2519/10 = 252ns 目标）。剩余 ~350ns 中：
- Int32ub inner.parse ~120ns（与 Const 共享，Const 总共 253ns 也含此）
- int 子类实例化 long_new ~55ns（CPython 固有，Python 层实测）
- setattr fmtstr ~50ns（CPython 固有，int 子类 __dict__ 分配）
- call1 + setattr 的 FFI 边界 ~50ns（跨边界固有，无法消除）

这 ~275ns 是「显示对象包装」相比「无包装字段」的**结构性固有税**——任何需要构造 Python
显示对象（int/bytes/dict 子类）的 parse 路径都要付。Python 原版 Hex 也要付同样的 CPython
税（2519ns 中约 200ns 是 new 调用），但 Python 原版还叠加了 Adapter 调度栈 + isinstance×3 +
Python 解释器开销（~2300ns），所以 construct-rs 仍能保持 6-7x 加速。

**结论**：Hex 族 parse 的合理目标加速比是 **6-7x**（显示对象包装的固有税），而非 10x。
当前 HX1 的 4.36x 因实现低效（call_method1 + fmtstr 构造）低于合理目标；方案 A+B 可
将其提升到合理目标区间。HX2/HD1 的 6.87x/6.88x 已在合理目标区间（bytes 路径无可优化）。

---

## 7. §0 合规确认

### 7.1 当前实现（含 call_method1）是否违反 §0？

| §0 原则 | 当前实现 | 判定 |
|---------|---------|------|
| #1 一次 FFI | parse 入口 1 次 + call_method1 回调 Python 字节码 1 次 = **2 次边界穿越**（但非"额外 FFI"架构违反） | ⚠️ 实现低效，非架构违反 |
| #2 无中间表示层 | ✅ Py<PyAny> 直持，无 Rust 中间类型 | 合规 |
| #3 无 trait 抽象 | ✅ 仅 Box<Node> | 合规 |
| #4 pyo3 核心依赖 | ✅ 全程 pyo3 API | 合规 |

**判定**：当前 call_method1 是「实现层面的低效」（多一次解释器进入），**不是 §0 #1 的
架构级违反**（不引入中间表示层 / 不 per-field 跨 FFI / 不 dict 跨 FFI）。设计文档 §2.2 对
此的「判据 2」归类是错误的（实为判据 1 路径），但结论「不算架构级 §0 违反」成立。

### 7.2 方案 A+B 是否改变 §0 合规性？

方案 A 把 call_method1（含字节码进入）改为 call1 + setattr（纯 C API，不进字节码循环）。
**改善** §0 #1 合规性——从「2 次边界穿越」回到「1 次 FFI + Rust 内 C API」（判据 2）。
方案 B 纯 Rust 内 intern，不影响 §0。

---

## 8. 流程判定

### 8.1 是否需走 DESIGNING → DESIGN_REVIEW？

**不需要**。理由：
1. 方案 A+B **不改变公开接口**（HexNode 字段 fmtstr 类型变更，但 HexNode 是内部 Node 变体，
   非 pub API；Hex/HexDump Python 描述符接口不变）。
2. 方案 A+B **不改变语义/parity**（new 字节码体的 C API 等价重写，产出对象结构一致）。
3. 方案 A+B 是 **bug 级实现优化**（修正设计文档 §2.2 的错误判据归因 + 消除已识别的低效点），
   属于 VET→CODING 回退或 PM 直接派 DEV 优化的范畴。
4. 建议路径：**PM 派 DEV 执行方案 A+B**（CODING 阶段），VET 复测验证预测（CODE_REVIEW）。
   本分析报告作为优化依据归档。

### 8.2 设计文档需修正（ARCH 责任，本次一并处理）

设计文档 `模块设计-Phase8-P0.md` §2.2 / §2.5 / §2.6.1 的「C 级实现 / 判据 2」表述需修正为：
「`HexDisplayedInteger.new` 是 Python `@staticmethod`（字节码），call_method1 走判据 1 路径
（跨 FFI 回调 Python 代码）。方案 A 改为 Rust 内 call1+setattr 后回到判据 2。」
**注**：Phase 8 已 ACCEPTED，设计文档修正不回退状态，仅作为 erratum 记录（由 PM 决定是否
正式修订文档）。本分析报告 §5.1 已记录该 erratum。

---

## 9. 证据索引

| 证据 | 文件 | 用途 |
|------|------|------|
| Python 层开销 profiling | `experiments/hex_perf_investigation/profile_hex_display.py` | 验证各步骤 ns/op + staticmethod 调度开销 |
| 结构性事实验证 | `experiments/hex_perf_investigation/verify_struct_facts.py` | 确认 new 是字节码 / call1 走 C 级 / __dict__ 结构 |
| Rust 实现 | `construct-rs/src/nodes/hex.rs` L150-203 | int/bytes/dict 三分支 parse 路径 |
| 显示类 Python | `construct-rs/python/construct/lib/hex.py` L18-28 | HexDisplayedInteger.new 字节码体 |
| Python 原版 | `construct/construct/lib/hex.py` L5-14（只读参考） | 确认 port 一致 |
| 性能数据 | `plans/phase8-adapters-struct-streams/traces/PERF-retest.md` | HX1/HX2/HD1 Controlled A/B Test |
| 设计文档 | `docs/design/模块设计/模块设计-Phase8-P0.md` §2 | 原设计（含 §2.2 erratum） |

---

## 10. 总结

| 维度 | 结论 |
|------|------|
| 根因 | call_method1("new") 跨 FFI 回调 Python 字节码（设计误判为 C 级）+ fmtstr 每次 PyString::new |
| 判定 | **设计判断错误 + 实现问题**（非固有开销，非架构级 §0 违反） |
| HX1 vs HX2 差距 | int 路径多付 fmtstr 构造 + 字节码调度 + setattr（~215ns），bytes 路径天然无这些 |
| vs Const 差距 | Const 仅 C 级 rich_compare ~30ns；Hex 多付 ~295ns 显示对象构造税 |
| 修正方案 | 方案 A（call1+setattr）+ 方案 B（编译期 intern fmtstr） |
| 预测收益 | HX1 581→~350-400ns（4.36x→~6.3-7.2x）；HX2/HD1 改善有限（已在合理区间） |
| §0 合规 | 当前非架构违反（实现低效）；方案 A+B 改善合规性 |
| 流程 | 不需 DESIGNING（无接口变更）；PM 派 DEV CODING + VET 复测 |
| 固有开销下限 | int 子类实例化 + setattr ≈ 105ns（Python 层），Rust 侧 ~150ns——CPython 固有 |
| 合理加速比目标 | 6-7x（显示对象包装固有税），HX2/HD1 已达，HX1 方案 A+B 后可达 |
