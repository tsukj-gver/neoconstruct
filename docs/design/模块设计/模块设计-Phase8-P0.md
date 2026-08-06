---
id: DESIGN-Phase8-P0
status: active
phase: "8"
depends_on:
  - ANALYSIS-phase8-pre
  - ADR-022
  - ADR-014
  - ADR-004
  - ADR-012
  - QUERY-rawcopy-architecture
  - DESIGN-phase6-adapter
  - PLAN-phase8
supersedes: []
superseded_by: []
last_updated: 2026-07-31
---

# 模块设计：Phase 8 P0 批次（基础设施 + 高优先级构造器）

> **角色**：ARCH
> **任务**：`8.0 [P0 批次详细设计]`
> **状态**：DESIGNING
> **创建时间**：2026-07-30
>
> **2026-07-31 修订（语义重新设计，两轮 + REV followup 修正）**：
> - **第一轮**：§2（Hex / HexDump）经用户决策推翻原"display 装饰器"语义，改为"hex 字符串
>   编解码器"语义（采用 `Hex(length)` API）。本次修订仅替换 §2 全部内容 + §0.1 表格 Hex 行。
> - **第二轮（本节）**：用户指出 `Hex(length)` 无法包装变长子构造器（GreedyBytes 等），
>   要求改回 `Hex(subcon)` 包装器 API。Hex 的角色是**格式转换器**——包装任意 subcon，
>   把 subcon 输出转 hex 字符串（parse），反向把 hex 字符串还原后交给 subcon（build）。
>   详见 §2.0 第二轮修正说明。
> - **REV followup 修正（2026-07-31，§2.2/§2.3.2/§2.4/§2.6.1/§2.7）**：REV 检视
>   PASS-WITH-FOLLOWUPS 后，ARCH 修订 3 个完备性 gap：
>   - **F-1**：`infer_hex_inner_kind` 对 FormatField 细分——Float 格式归 Unknown（非 Int）
>   - **F-2**：Int kind 提取从 `i64` 扩展到 `i128` + 任意精度 Python fallback（覆盖 u64::MAX / 大整数）
>   - **F-3**：`infer_hex_inner_kind` 递归穿透透明包装器（方案 A，消除 HX-25 矛盾）
>   详见 §2.2 表格注释 + §2.3.2 重写后的 `infer_hex_inner_kind`。
> §1 / §3+ 不受影响。

---

## 0. P0 批次范围与共通设计原则

### 0.1 P0 批次范围（6 个子任务，ARCH 自行确定）

PM 决策 1-7 全部接受（详见 `plans/phase8-adapters-struct-streams/总纲.md §PM 决策`）。本设计文档覆盖 P0 批次——"其他子任务依赖的基础设施 + 高优先级构造器"：

| 子任务 | 构造器 | 性质 | 新增 Node 变体 | 关键基础设施贡献 |
|--------|--------|------|---------------|-----------------|
| **8.1** | Const / Default / Check | 内置 Rust Node | 3（ConstNode / DefaultNode / CheckNode） | 跑通 Subconstruct 包装 + Construct 非包装两类模式 |
| **8.4** | Hex / HexDump | 内置 Rust Node（subcon 包装器，hex 格式转换器） | 1（HexNode 重写；HexDump 视用户决策） | **语义重新设计（两轮）**：Hex 从 display 装饰器改为 subcon 包装器 hex 格式转换器（parse 返回 hex str，build 接收 hex str）。保留 `Hex(subcon)` API 支持 GreedyBytes 等变长子构造器。需 `hex` crate + `HexInnerKind` 编译期分类 |
| **8.5** ⭐ | Checksum + Rust hashfunc + ParseStream::slice | 内置 Rust Node + 基础设施 | 1（ChecksumNode） | ParseStream::slice（零拷贝切片）+ 5 个 Rust crate + HashAlgo enum |
| **8.8** | Aligned + AlignedStruct（宏） | Rust Node + Python 宏 | 1（AlignedNode） | padding 算法（modulus + subcon size + pattern） |
| **8.9** | Terminated / Probe | 内置 Rust Node | 2（TerminatedNode / ProbeNode） | 流位置查询 / context dump |
| **8.10** | CancelParsing | ConstructError 变体 + schema.rs 顶层 catch | 0（Error 变体） | schema.rs parse 入口修改 + Python 层 CancelParsing 异常导出 |

**Node enum 净增**：9 个新变体（当前 39 → 48）。

**P1 批次（不在本设计范围）**：8.2 (Enum/FlagsEnum/Mapping) / 8.3 (OneOf/NoneOf + Validator) / 8.6 (Union) / 8.7 (Sequence) / 8.11 (NamedTuple/Timestamp) / 8.12 (ProcessXor/ProcessRotateLeft)。

**实施顺序建议（基于依赖）**：
```
8.10 (CancelParsing，Error 变体 + parse 入口)  ← 先做，基础设施改动
8.1 (Const/Default/Check)                      ← 跑通模式，参考 RebuildNode
8.9 (Terminated/Probe)                         ← 最简单
8.8 (Aligned + AlignedStruct)                  ← padding 算法
8.4 (Hex/HexDump)                              ← hex 字符串编解码器（语义重新设计）
8.5 (Checksum + Rust hashfunc)                 ← 依赖 ParseStream::slice + 5 crate
```

### 0.2 §0 合规判据（PM 决策框架 §0.2 重申）

PM 已接受 ARCH 8.0 分析报告确立的关键判据：

> **"FFI crossing（调用用户定义的 Python 代码）≠ C API 操作（直接操作 CPython 内置类型）"**

具体判据：
1. **跨 FFI 回调用户 Python 代码**（如 AdapterCallbackNode 调 `_decode`）：算额外 FFI，仅用户主动选择时允许（ADR-022）
2. **Rust 内调 CPython C API**（`PyDict_GetItem` / `PySet_Contains` / `PyType_Call` / `PyLong_FromLong`）：不算额外 FFI，是 §0 #1 明文允许的"Rust 内部通过 CPython C API 直接操作 Python 对象"——即使内部触发 `__hash__`/`__eq__`（对 int/str/bytes/frozenset 等内置类型，这些是 C 级实现）

**P0 批次所有构造器均走判据 2**（编译期物化 `Py<PyDict>`/`Py<PyFrozenSet>`/`Py<PyType>` + 运行时 C API 查询），全程严格 1 次 FFI（仅 parse/build 入口）。

### 0.3 共通模式（L-04 对策：模式复用）

P0 批次构造器复用 Phase 6.3 Adapter 核心已沉淀的两类模式，不重新决策：

| 模式 | 适用 | 已有先例（参考实现位置） |
|------|------|------------------------|
| **Subconstruct 包装模式** | Const / Default / Aligned | `SubconstructNode` / `PeekNode` / `RawCopyNode` / `RebuildNode`（`nodes/subconstruct.rs` 等） |
| **Construct 非包装模式** | Check / Checksum / Terminated / Probe | `PassNode` / `ArrayNode` / `FocusedSeqNode`（`nodes/pass.rs` 等） |
| **Error 变体 + 顶层 catch 模式** | CancelParsing | `StopField` 哨兵（`error.rs`）+ `schema.rs` 入口 |

### 0.4 共通约束（Rust 编码红线 + ADR-014/022）

- **表达式系统不接 lambda/callable**（ADR-006/014）：Default.value / Check.func / Aligned.modulus / Checksum.StreamRange.start/end / Probe.into 凡需"动态值"的位置，一律编译为 `ExprProgram`，不接收 Python callable。这是与 Python 原版"接受 context lambda"的已知 parity 差异（与 RebuildNode RB-5 同硬约束）
- **Rust 编码红线**：禁止 `unwrap()`/`expect()` 在非测试代码；禁止 `TODO`/`FIXME`；禁止硬编码魔法数字；所有 `pub` 项必须有 `///` 文档注释；parse/build 对称
- **§0 #2 合规**：禁止在 Rust 侧引入中间数据类型（如 `enum Value { Int(i64), Bytes(Vec<u8>), ... }`）。所有"对象"直接以 `Py<PyAny>` 持有，需要时通过 pyo3 C API 操作

---

## 1. 子任务 8.1：Const / Default / Check

### 1.1 Python 参考实现摘要

| 构造器 | Python 行号 | 核心语义 |
|--------|-----------|---------|
| `Const(value, subcon=None)` | core.py L2808-2876 | parse：subcon.parse → `obj == value` 不等抛 ConstError；build：obj ∈ {None, value} → subcon.build(value)；sizeof 转发。`flagbuildnone=True`。subcon 缺省时若 value 为 bytes 则 `Bytes(len(value))` |
| `Default(subcon, value)` | core.py L3030-3078 | parse：转发 subcon（继承 Subconstruct）；build：obj is None → `evaluate(value)`，否则用 obj；subcon.build。`flagbuildnone=True` |
| `Check(func)` | core.py L3081-3129 | parse/build：`evaluate(func, context)`，非真抛 CheckError；sizeof=0；parse 返回 None（无字段值）。`flagbuildnone=True` |

### 1.2 Rust Node 设计

#### 1.2.1 ConstNode（Subconstruct 包装模式）

```rust
/// 常量字段节点：parse 校验子解析结果 == value；build 用 value（忽略 obj）。
///
/// 对应 Python construct `Const(value, subcon=None)`（core.py L2808）。
///
/// # 三方法行为
///
/// - parse：inner.parse(obj) → `obj == value`（pyo3 rich compare C API）→ 不等
///   `ConstructError::Const`，等则返回 obj
/// - build：obj is None 或 obj == value → inner.build(value)；否则 Const 错误
/// - sizeof：转发 inner.sizeof
///
/// # value 类型
///
/// `value: Py<PyAny>` 持有任意 Python 对象（int/bytes/str 等）。比较走 pyo3
/// `obj.bind(py).rich_compare(value.bind(py), CompareOp::Eq)`，由 CPython 调度
/// 到对应类型的 `__eq__`（对 int/bytes/str 是 C 级实现，§0.2 判据 2）。
#[derive(Debug)]
pub struct ConstNode {
    inner: Box<Node>,
    value: Py<PyAny>,
}

impl ConstNode {
    pub fn new(inner: Node, value: Py<PyAny>) -> Self {
        Self { inner: Box::new(inner), value }
    }
    pub fn inner(&self) -> &Node { &self.inner }
    pub fn value(&self) -> &Py<PyAny> { &self.value }
}
```

**`has_expressions` 集成**：`Node::Const(c) => c.inner().has_expressions()`（与 Subconstruct 同模式）。

#### 1.2.2 DefaultNode（Subconstruct 包装 + 表达式求值）

```rust
/// 默认值字段节点：build 时 obj 为 None 则用 value 表达式求值。
///
/// 对应 Python construct `Default(subcon, value)`（core.py L3030）。
/// Python 的 value 可为常量或 context lambda；construct-rs 强制编译为 ExprProgram
/// 或编译期常量（与 Rebuild func 同脉络，ADR-014 硬约束）。
///
/// # 三方法行为
///
/// - parse：转发 inner.parse（与 Subconstruct 一致）
/// - build：obj is None → 求值 ExprProgram 得 i64 → 转 PyLong → inner.build(py_long)；
///   否则 inner.build(obj)
/// - sizeof：转发 inner.sizeof
#[derive(Debug)]
pub struct DefaultNode {
    inner: Box<Node>,
    /// build 时默认值表达式。常量值编译期包装为单条 `PushI64` ExprProgram。
    value: ExprProgram,
}

impl DefaultNode {
    pub fn new(inner: Node, value: ExprProgram) -> Self {
        Self { inner: Box::new(inner), value }
    }
    pub fn inner(&self) -> &Node { &self.inner }
    pub fn value(&self) -> &ExprProgram { &self.value }
}
```

**`has_expressions` 集成**：`Node::Default(d) => d.inner().has_expressions() || true`（恒 true，含 value 表达式）。

**常量 value 的编译期包装**：用户传 `Default(Byte, 0)` 时，0 在编译期编译为 `ExprProgram { ops: vec![ExprOp::PushI64(0)] }`（与 Computed 常量模式同，详见 `nodes/computed.rs`）。Python 用户写 `Default(Byte, lambda ctx: ctx.x + 1)` 不支持——parity 已知差异（DF-1）。

#### 1.2.3 CheckNode（Construct 非包装 + RO 字段）

```rust
/// 断言检查节点：parse/build 求值表达式，非真抛 CheckError。
///
/// 对应 Python construct `Check(func)`（core.py L3081）。
///
/// **必须作为 RO 字段使用**（与 Computed 同类，`_field_kind = "ro"`）。
/// parse 返回 Py_None（无字段值）；build 求值表达式但不写字节。
///
/// # 表达式约束
///
/// func 必须是 Phase 2 表达式（FieldRef/ExprRef/int 组合），编译为 ExprProgram。
/// **不接收 Python lambda/callable**（ADR-014 硬约束）。
#[derive(Debug)]
pub struct CheckNode {
    func: ExprProgram,
}

impl CheckNode {
    pub fn new(func: ExprProgram) -> Self { Self { func } }
    pub fn func(&self) -> &ExprProgram { &self.func }
}
```

**`compute_ro_value` 集成**：Check 作为 RO 字段，`Node::compute_ro_value` 新增 `Check` 分支返回 `py.None()`（不参与 context 写入）。

### 1.3 Construct impl（关键路径摘要）

```rust
impl Construct for ConstNode {
    fn parse<'py>(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
        let obj = self.inner.parse(py, stream, ctx, path)?;
        // pyo3 rich compare（C API，对 int/bytes/str 走 C 级 __eq__）
        let is_eq = obj.bind(py).rich_compare(self.value.bind(py), CompareOp::Eq)?
            .is_truthy()?;
        if !is_eq {
            return Err(ConstructError::Const {
                message: format!(
                    "parsing expected {:?} but parsed {:?}",
                    self.value.bind(py).repr()?, obj.bind(py).repr()?
                ),
                path: path.to_string(),
            });
        }
        Ok(obj)
    }
    fn build(&self, py, obj, stream, ctx, path) -> Result<(), ConstructError> {
        // obj is None 或 obj == value → inner.build(value)；否则 Const 错误
        let obj_is_none = obj.is_none();
        let is_eq = if !obj_is_none {
            obj.rich_compare(self.value.bind(py), CompareOp::Eq)?.is_truthy()?
        } else { false };
        if obj_is_none || is_eq {
            self.inner.build(py, self.value.bind(py), stream, ctx, path)
        } else {
            Err(ConstructError::Const {
                message: format!("building expected None or value, got {:?}", obj.repr()?),
                path: path.to_string(),
            })
        }
    }
    fn sizeof(&self, ctx) -> Result<usize, ConstructError> { self.inner.sizeof(ctx) }
}

impl Construct for DefaultNode {
    fn parse<'py>(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
        self.inner.parse(py, stream, ctx, path)  // 转发
    }
    fn build(&self, py, obj, stream, ctx, path) -> Result<(), ConstructError> {
        let build_obj: Py<PyAny> = if obj.is_none() {
            // 求值 ExprProgram 得 i64 → PyLong
            let v = crate::expr::eval_expr_int(&self.value, ctx, py)?;
            v.into_py(py)
        } else {
            // 借用 obj（不克隆，转交 inner）
            obj.clone().into_py(py)  // 注：obj 是 &Bound，需 unbound 引用
        };
        self.inner.build(py, build_obj.bind(py), stream, ctx, path)
    }
    fn sizeof(&self, ctx) -> Result<usize, ConstructError> { self.inner.sizeof(ctx) }
}

impl Construct for CheckNode {
    fn parse<'py>(&self, py, _stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
        let passed = crate::expr::eval_expr_bool(&self.func, ctx, py)?;
        if !passed {
            return Err(ConstructError::Check {
                message: "check failed during parsing".to_string(),
                path: path.to_string(),
            });
        }
        Ok(py.None())
    }
    fn build(&self, py, _obj, _stream, ctx, path) -> Result<(), ConstructError> {
        let passed = crate::expr::eval_expr_bool(&self.func, ctx, py)?;
        if !passed {
            return Err(ConstructError::Check {
                message: "check failed during building".to_string(),
                path: path.to_string(),
            });
        }
        Ok(())
    }
    fn sizeof(&self, _ctx) -> Result<usize, ConstructError> { Ok(0) }
}
```

**`eval_expr_bool` 依赖**：Phase 2 ExprProgram 已有 `eval_expr_int`（i64），需新增 `eval_expr_bool` 包装（i64 非零即 true，对齐 Python `if not passed` 语义）。若无此 API，DEV 可用 `eval_expr_int(v)?; Ok(v != 0)` 内联。

### 1.4 §0 原则对照表（L-01 对策）

| §0 原则 | ConstNode | DefaultNode | CheckNode |
|---------|-----------|-------------|-----------|
| #1 一次 FFI | ✅ 严格 1 次。inner.parse/build 在 Rust 内；`rich_compare` 是 C API（§0.2 判据 2） | ✅ ExprProgram 在 Rust 内求值（~10ns，Phase 2 已验证）；PyLong 构造是 C API | ✅ ExprProgram 求值在 Rust 内 |
| #2 无中间表示层 | ✅ `Py<PyAny>` 持有 Python 对象本身，无 Rust 中间类型 | ✅ i64→PyLong 是 Python 对象构造，非中间类型 | ✅ 无中间数据 |
| #3 输入输出无 trait 抽象 | ✅ 仅 `Box<Node>`（与 Subconstruct 同模式） | ✅ | ✅ |
| #4 pyo3 核心依赖 | ✅ `rich_compare` / `is_truthy` / `repr` 全为 pyo3 API | ✅ ExprProgram + PyLong 都是 pyo3 操作 | ✅ |
| #5 mashumaro API | ✅ Const 是字段描述符（与 Bytes 同层） | ✅ | ✅ |
| #6 enum_dispatch | ✅ 3 个新 Node 加入 enum | ✅ | ✅ |
| #7 Result + path | ✅ ConstructError::Const 携带 path | ✅ | ✅ ConstructError::Check |
| #8 Stream 纯 Rust | ✅ Const/Default 转发 inner.stream 操作；Check 不操作 stream | ✅ | ✅ Check 不消费 stream |

### 1.5 性能假设（L-02/L-05 对策）

#### 1.5.1 瓶颈识别（量化数据 + 来源）

| Node | 主要瓶颈 | 量化来源 |
|------|---------|---------|
| Const | inner.parse + 1 次 `rich_compare`（~30ns） | pyo3 benchmark：PyObject_RichCompare ~20-30ns |
| Default | inner.parse/build + ExprProgram::eval（~10ns） | Phase 2 ExprProgram 已验证 |
| Check | ExprProgram::eval_bool（~10ns） | Phase 2 Computed 已验证（同量级） |

#### 1.5.2 可证伪预测（覆盖所有 FFI/拷贝/转换来源）

**嵌入 Struct 字段，与 Python construct 2.10.70 对比**：

- **Const**：加速比 ≥8x（vs Python：subcon.parse + Python `==` 调度 + Python `repr` 格式化 + Python raise）
- **Default**：加速比 ≥8x（与 Rebuild 同量级，ExprProgram ~10ns vs Python lambda ~270ns）
- **Check**：加速比 ≥10x（与 Computed 同量级，且省去 Python evaluate + raise 路径）

**FFI 来源清单**（L-05 对策）：
- StructMixin.parse 入口（1 次，共享）
- Rust 内 rich_compare / ExprProgram / PyLong 构造（不计 FFI，§0.2 判据 2）
- 无 Rust→Python 回调（严格 1 次 FFI）

#### 1.5.3 失败模式（可证伪）

- 若 Const 实测 <4x：可能 `rich_compare` 调度到 Python 用户类型的 `__eq__`（自定义类），破坏 §0.2 判据 2 前提。**防御**：dev 文档标注"对 int/bytes/str 等内置类型 fast-path，自定义类型走 Python `__eq__` 是用户引入的边缘场景"
- 若 Default 实测 <4x：可能 ExprProgram 求值失败路径开销过大。**防御**：测试覆盖 RB-3（表达式求值失败错误传播）

### 1.6 边界条件清单

| 编号 | 场景 | 预期行为 |
|------|------|---------|
| CN-1 | `Const(b"IHDR")` parse b"IHDR" | 返回 b'IHDR'（subcon 缺省 Bytes(4)，编译期推断） |
| CN-2 | `Const(b"IHDR")` parse b"JPEG" | ConstructError::Const（"parsing expected b'IHDR' but parsed b'JPEG'"） |
| CN-3 | `Const(b"IHDR").build(None)` | 输出 b'IHDR'（flagbuildnone=True） |
| CN-4 | `Const(255, Int32ul).build(255)` | 输出 b'\xff\x00\x00\x00'（obj == value，build value） |
| CN-5 | `Const(255, Int32ul).build(256)` | ConstructError::Const（"building expected None or 255 but got 256"） |
| CN-6 | `Const(255, Int32ul).build(None)` | 输出 b'\xff\x00\x00\x00' |
| CN-7 | `Const(b"IHDR")` 编译期 value 是 bytes 但 subcon 显式给出 | 用显式 subcon（用户指定优先） |
| CN-8 | `Const(b"IHDR")` 编译期 value 非 bytes 且 subcon 缺省 | 编译期 StringError（对齐 Python L2838） |
| DF-1 | `Default(Byte, 0).build(None)` | 输出 b'\x00'（求值常量 ExprProgram → 0） |
| DF-2 | `Default(Byte, 0).build(5)` | 输出 b'\x05'（obj 非 None，用 obj） |
| DF-3 | `Default(Byte, some_expr).build(None)` | 求值 some_expr → i64 → inner.build(i64) |
| DF-4 | `Default(Byte, lambda ctx: ...)` | **编译期拒绝**（parity 差异：ADR-014 硬约束，与 Rebuild RB-5 同） |
| DF-5 | `Default(Byte, some_expr)` 表达式求值失败 | ExprFieldMissing 错误向上传播（与 Rebuild RB-3 同） |
| CK-1 | `Check(some_expr)` parse 表达式为真 | 返回 None（Py_None） |
| CK-2 | `Check(some_expr)` parse 表达式为假（0） | ConstructError::Check（"check failed during parsing"） |
| CK-3 | `Check(some_expr)` build 表达式为假 | ConstructError::Check（"check failed during building"） |
| CK-4 | `Check(some_expr).sizeof()` | 返回 0 |
| CK-5 | `Check(lambda ctx: ...)` | **编译期拒绝**（DF-4 同脉络） |
| CK-6 | 用户用 `field(Check(...))`（RW） | **编译期拒绝**（`_field_kind = "ro"`，与 Computed 同） |
| CK-7 | Check 嵌入 Sequence（无字段名） | 行为同嵌入 Struct（求值但不写 list） |

### 1.7 DEV 实施清单

| 文件 | 内容 | 行数估 |
|------|------|-------|
| `nodes/const_node.rs`（命名避开 Rust 关键字 `const`） | ConstNode + impl + 单元测试 | ~250 |
| `nodes/default_node.rs` | DefaultNode + impl + 单元测试 | ~200 |
| `nodes/check_node.rs` | CheckNode + impl + 单元测试 | ~180 |
| `nodes/mod.rs` | 新增 3 个 mod/enum 变体 + has_expressions + compute_ro_value 分支 | ~40 增量 |
| `compile.rs` | 新增 3 个 build_*_node 分支 | ~80 增量 |
| `error.rs` | 新增 ConstructError::Const + ConstructError::Check 变体 + ExceptionClasses 字段 + Python 类映射 | ~60 增量 |
| `_descriptors.py` | ConstDescriptor / DefaultDescriptor / CheckDescriptor + 工厂函数 | ~80 增量 |
| `_errors.py`（或现有错误定义文件） | ConstError / CheckError Python 类（对齐 core.py L74/L84） | ~20 增量 |
| `__init__.py` | 导出 Const / Default / Check / ConstError / CheckError | ~15 增量 |

**Cargo.toml 不需新增依赖**（rich_compare / ExprProgram 已有）。

### 1.8 Parity 测试模板（与 Python construct 2.10.70 对照）

```python
# tests/parity/test_phase8_p0_const_default_check.py（骨架）

def test_const_bytes_parse_ok():
    # Const(b"IHDR").parse(b"IHDR") → b"IHDR"
    assert_parity_case("""
    from construct import Const
    parsed = Const(b"IHDR").parse(b"IHDR")
    """, expected_py=b"IHDR")

def test_const_bytes_parse_fail():
    # Const(b"IHDR").parse(b"JPEG") → ConstError
    assert_parity_error("""
    from construct import Const, ConstError
    try:
        Const(b"IHDR").parse(b"JPEG")
    except ConstError:
        pass
    """)

def test_const_int_build_none():
    # Const(255, Int32ul).build(None) → b'\xff\x00\x00\x00'
    ...

def test_default_build_none_uses_value():
    # Default(Byte, 0).build(None) → b'\x00'
    ...

def test_default_with_expression_in_struct():
    # @dataclass class P(StructMixin):
    #     count: int = rfield(Byte)
    #     padded: int = rfield(Default(Byte, count))
    # P.build(P(count=5)) → b'\x05\x05'（padded 用 count=5）
    # P.build(P(count=5, padded=9)) → b'\x05\x09'（padded 用 obj）
    ...

def test_check_in_struct_pass():
    # @dataclass class P(StructMixin):
    #     width: int = rfield(Byte)
    #     height: int = rfield(Byte)
    #     _check: None = rfield(Check(width * height > 0))
    # P.parse(b'\x02\x03') → P(width=2, height=3)
    ...

def test_check_in_struct_fail():
    # Check(width == 0) 表达式不满足 → CheckError
    ...
```

**Parity 重点**：
- CN-7/CN-8 编译期 subcon 推断（bytes → Bytes(len)）
- DF-4/CK-5 lambda 不支持的 parity 差异（用户文档标注，parity 测试跳过）

---

## 2. 子任务 8.4：Hex / HexDump

### 2.0 语义重新设计说明（2026-07-31，两轮用户决策）

> **本节记录两轮语义修正。最终采用 `Hex(subcon)` 包装器 API（第二轮）。**

#### 第一轮（display 装饰器 → hex 字符串编解码器）

原设计遵循 Python construct 原版的"display 装饰器"语义（parse 返回 int/bytes/dict **子类**，
子类仅覆盖 `__str__` 显示 hex 格式，值不变）。实测该语义导致性能瓶颈（HX1 parse 仅 4.36x），
根因是 CPython display 子类实例化 + setattr 是不可消除的固有税（详见
`docs/analysis/phase8-hex-parse-perf-investigation.md`）。

第一轮用户决策：消除"值和显示分离"，Hex 改为"hex 字符串编解码器"，**首轮采用 `Hex(length)`
API**（像 `Bytes(N)` 一样接收字节数）。

#### 第二轮（Hex(length) → Hex(subcon) 包装器，本节最终方案）

**第二轮用户决策（2026-07-31）**：`Hex(length)` 只能读固定字节数，无法包装 `GreedyBytes` 等
变长子构造器。用户要求改回 `Hex(subcon)` 包装器 API：

> "还是支持 subcon 的 api 比较好，否则无法处理一些变长数据，比如 greedybytes 之类的，
> 相当于 Hex 变成了一个格式转换器，只是从输出 bytes 或对象，变成了输出 hex。"

**最终语义核心**：Hex 是 **subcon 包装器 hex 格式转换器**——
- **parse**：`inner.parse(stream)` 得到结果（bytes/int/...）→ 按类型 hex 编码 → `PyString`
- **build**：接收 hex 字符串 → hex 解码 → 按类型还原 → `inner.build(restored, stream)`
- **subcon 输出类型**由编译期 `HexInnerKind` 推断（Bytes/Int/Unknown 三类，详见 §2.3）

**前两轮设计失效的结论**（不再适用，记录于此供审计追溯）：
- display 装饰器路径（HexDisplayClasses / fmtstr / call_method1 / setattr）全部废弃
- 第一轮 `Hex(length)` 的 `BytesLength` / 无 inner Node / 直接 `stream.read` 路径全部废弃
- `construct.lib.hex` 的 5 个显示类在新语义下不再被 Rust parse 调用

### 2.1 Python 参考实现摘要（原版语义，仅作 parity 对照基准）

| 构造器 | Python 行号 | 原版语义（construct-rs **不沿用 display 部分**） |
|--------|-----------|---------|
| `Hex(subcon)` | core.py L3523-3580 | Adapter。`_decode(obj)` 按 `isinstance(obj, int/bytes/dict)` 分派到显示类 factory；`_encode(obj)` 直接 `return obj`（build 透传）。parse 返回 int/bytes/dict 子类（仅 `__str__` 显示 hex） |
| `HexDump(subcon)` | core.py L3583-3635 | Adapter，display 装饰器。parse 返回 bytes/dict 子类（`__str__` 输出 xxd 风格 hexdump） |

> **parity 关键**：construct-rs 新 Hex **保留 `Hex(subcon)` API 与原版一致**（包装任意子构造器），
> 但 parse/build 的值语义变更（原版返回 display 子类，construct-rs 返回/接收 hex 字符串）。
> 原版 `_decode` 的 int/bytes/dict 三分支在新设计中映射为 `HexInnerKind`（Bytes/Int/Unknown）。
> 这是**值语义级 breaking change**（用户明确认可），但**API 形态保持兼容**。

### 2.2 关键设计决策：Hex 为 subcon 包装器 hex 格式转换器

**最终语义定义**：

| 维度 | construct-rs（subcon 包装器） | 原版（Python construct，仅对照） |
|------|-------------------------------|-------------------------------|
| API | `Hex(subcon)` —— 包装子构造器（**与原版 API 一致**） | `Hex(subcon)` —— 包装子构造器 |
| parse 返回 | `str`（hex 编码字符串），按 `HexInnerKind` 分派编码方式 | int/bytes/dict 子类（display 对象） |
| build 接收 | `str`（hex 字符串）→ 解码还原 → 交给 `inner.build` | 原始 int/bytes/dict 值（`_encode` 透传） |
| sizeof | 转发 `inner.sizeof`（与 Subconstruct 同） | 转发 `subcon.sizeof` |
| 内部实现 | `inner.parse` → 按类型 `hex::encode`/`format!("{:x}")` → `PyString` | subcon.parse → Python 显示类 factory |

**理由**（用户需求 + 性能 + 语义清晰）：
1. **支持变长子构造器**（用户核心需求）：`Hex(GreedyBytes)` / `Hex(Prefixed(...))` 等场景
   在 `Hex(length)` 下无法表达，subcon 包装器模式天然支持
2. **消除 display 子类固有税**（第一轮已确立）：parse 不再构造 Python display 子类
3. **inner.parse/build 全在 Rust 内部**（§0 #1 合规）：subcon 包装器模式与 Subconstruct/Const/Peek
   同构，inner 调用是 Rust 内部函数调用，不构成额外 FFI
4. **round-trip 对称**：parse → hex str，build(hex str) → 原始字节（经 inner 还原）

**subcon 输出类型处理（核心设计决策）**：

Hex(subcon) 的 subcon 可能输出不同 Python 类型，hex 编码方式随之不同。ARCH 决策：**编译期
根据 inner Node 变体推断 `HexInnerKind`**（运行期无需 trial-and-error，避免歧义）：

| HexInnerKind | 覆盖的 inner Node 变体 | parse 编码 | build 解码 |
|--------------|----------------------|-----------|-----------|
| **Bytes** | `Bytes` / `GreedyBytes` + **透明包装器递归命中上述叶节点**（见 §2.3.2） | `hex::encode(&bytes)` → 小写连写（`"0000001f"`） | `hex::decode(s)` → `PyBytes` → `inner.build` |
| **Int** | `FormatField`（**仅 int 格式**，F-1）/ `VarInt` / `ZigZag` / `BytesInteger` / `BitsInteger` / `Tell` / `Computed` / `Index` + 透明包装器递归 | `format!("{:x}", v)`（v 为 i128，F-2）→ 无前导零小写（`"1f"`） | `i128::from_str_radix(s, 16)`（F-2）→ `PyLong` → `inner.build` |
| **Unknown** | `FormatField`（**Float 格式**，F-1）/ `Struct` / `RawCopy` / `Array` / `Adapter` / `Strings` / 不一致多分支 / 非透明包装器等 | **透传**（obj 原样返回，fail-soft） | **透传**（obj 原样传 inner.build） |

> **F-1/F-2/F-3 修订说明（2026-07-31 REV followup）**：
> - **F-1**：`FormatField` 不再 blanket 归 Int。`PythonFormat` 的 6 个 Float 变体
>   （Float16/32/64 × Big/Little）parse 返回 PyFloat，归为 **Unknown**（透传），
>   对齐 Python 原版 `_decode` 的 `return obj` fallback。仅 16 个 int 格式归 Int。
> - **F-2**：Int kind 的提取从 `i64` 扩展到 **`i128`**（覆盖 i64 全域 + u64::MAX +
>   VarInt/BytesInteger ≤ 16 字节）。超过 i128 的任意精度（BytesInteger > 16 字节，
>   极罕见）走 Python `int.__format__` / `int(s, 16)` fallback（§0.2 判据 2，C API）。
> - **F-3**：`infer_hex_inner_kind` 不再是扁平 match——对**透明包装器**
>   （parse 输出 == inner 输出的包装器）递归穿透，详见 §2.3.2。

**Unknown 分支的设计理由**：
- 对齐 Python 原版 `Hex._decode` 的 `return obj` fallback（core.py L3565）
- 对齐 Python 原版 `Hex._encode` 的 `return obj`（core.py L3568，build 透传）
- 用户若需要对 Struct/RawCopy 输出做 hex，应在 Python 层后处理（`binascii.hexlify`），
  而非依赖 Hex 包装器——Struct/dict 的 hex 编码语义不明确
- **fail-soft 优于 fail-hard**：Unknown 类型 round-trip 对称（parse 透传 X，build 透传 X 还原）

**字符串格式规范**：
- **Bytes kind**：纯连写小写（`"0000001f"`），等价 `binascii.hexlify`；build 大小写兼容
- **Int kind**：无前导零小写（`"1f"`），等价 Python `format(v, 'x')`；build 接受可选 `"0x"` 前缀
  （对齐 Python `int(s, 16)` 宽容性，Rust `from_str_radix` 不接受前缀故手动 strip）
- **负数**（Int kind）：`format!("{:x}", -31)` → `"-1f"`；build strip `"-0x"` 后保留负号

**HexDump 处理**：见 §2.10（ARCH 推荐方案 C wont_implement，与新设计兼容）。

### 2.3 Rust Node 设计

#### 2.3.1 HexNode（subcon 包装器 hex 格式转换器）

```rust
/// Hex 格式转换器节点：包装任意 subcon，parse 输出 hex 字符串，build 从 hex 字符串还原。
///
/// **语义（2026-07-31 第二轮修正，最终方案）**：保留 `Hex(subcon)` 包装器 API
/// （与 Python 原版一致），支持 GreedyBytes 等变长子构造器。Hex 的角色是"格式转换器"
/// ——把 subcon 输出转 hex 字符串（parse），反向把 hex 字符串还原后交给 subcon（build）。
///
/// 对应 Python construct `Hex(subcon)`（core.py L3523）。值语义为 breaking change
/// （原版返回 display 子类，construct-rs 返回 hex 字符串），详见设计 §2.0。
///
/// # subcon 输出类型处理（HexInnerKind）
///
/// 编译期根据 inner Node 变体推断输出类型（运行期无需 trial-and-error）：
/// - **Bytes**（Bytes/GreedyBytes）：parse `hex::encode(&bytes)`，build `hex::decode` → bytes
/// - **Int**（FormatField int 格式/VarInt/ZigZag/BytesInteger/BitsInteger/Tell/Computed/Index）：
///   parse `format!("{:x}", v)`（i128 提取），build `i128::from_str_radix(s, 16)` → int
/// - **Unknown**（FormatField Float 格式/Struct/Sequence/RawCopy/dict/list/Strings/非透明包装器等）：
///   parse/build 透传
///
/// **透明包装器穿透**（F-3）：Subconstruct/Peek/Const/Default/Rebuild/Aligned/Prefixed/
/// Pointer/Transform/Bitwise/Bytewise/ProcessXor/ProcessRotateLeft/NullTerminated/
/// NullStripped/OneOf/NoneOf 等包装器的 parse 输出 == inner 输出，`infer_hex_inner_kind`
/// 递归穿透它们（IfThenElse/Switch/Select 检查所有分支一致）。详见 §2.3.2。
///
/// # 三方法行为
///
/// - parse：`inner.parse(stream)` → 按 `inner_kind` 分派 hex 编码 → `PyString`（或透传）
/// - build：按 `inner_kind` 分派 hex 解码 → `inner.build(restored, stream)`（或透传）
/// - sizeof：转发 `inner.sizeof(ctx)`（与 Subconstruct 同）
///
/// # 与其他包装器的同构性
///
/// HexNode 与 SubconstructNode / ConstNode / PeekNode 同属 `inner: Box<Node>` 包装器模式，
/// 唯一差异是 parse/build 对 inner 结果做 hex 编解码转换（而非纯转发）。
#[derive(Debug)]
pub struct HexNode {
    /// 被包装的子树根。
    inner: Box<Node>,
    /// 编译期推断的 inner 输出类型（决定 parse/build 的 hex 编解码方式）。
    inner_kind: HexInnerKind,
}

/// inner Node 的输出类型分类（编译期推断，决定 hex 编解码方式）。
///
/// 这是"哪些 Node 变体输出 bytes/int"的**单一事实源**（L-04 对策）。
/// 新增 Node 变体若输出 bytes/int，需同步更新 `infer_hex_inner_kind`（compile.rs）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HexInnerKind {
    /// inner.parse 输出 PyBytes（Bytes/GreedyBytes 等）。
    /// parse: `hex::encode`；build: `hex::decode` → PyBytes。
    Bytes,
    /// inner.parse 输出 PyLong（FormatField int 格式/VarInt 等）。
    /// parse: `extract::<i128>` + `format!("{:x}")`（任意精度走 __format__ fallback）；
    /// build: `i128::from_str_radix(s, 16)`（任意精度走 int(s,16) fallback）→ PyLong。
    Int,
    /// inner.parse 输出其他类型（Struct/RawCopy/Array/Strings/包装器等）。
    /// parse/build 透传（fail-soft，对齐 Python 原版 _decode/_encode fallback）。
    Unknown,
}

impl HexNode {
    /// 创建 `HexNode`，包裹给定的子树根节点 + 编译期推断的输出类型。
    pub fn new(inner: Node, inner_kind: HexInnerKind) -> Self {
        Self { inner: Box::new(inner), inner_kind }
    }
    /// 返回内部子树根节点的引用。
    pub fn inner(&self) -> &Node { &self.inner }
    /// 返回编译期推断的 inner 输出类型分类。
    pub fn inner_kind(&self) -> HexInnerKind { self.inner_kind }
}
```

**关键设计点**：
1. **subcon 包装器模式**——`inner: Box<Node>`，与 Subconstruct/Const/Peek 同构（L-04 模式复用）
2. **编译期 HexInnerKind 分类**——避免运行期 trial-and-error，parse/build 走确定的 fast-path
3. **`hex` crate**——新增依赖（`hex = "0.4"`，纯 Rust 编解码库，§0 #4 合规，与 `half`/`sha2` 同先例）
4. **删除全部显示类相关结构**——`HexDisplayClasses` / `HexDumpDisplayClasses` /
   `load_hex_display_classes` / `load_hexdump_display_classes` / `fmtstr` 字段全部废弃

**`has_expressions` 集成**：`Node::Hex(h) => h.inner().has_expressions()`
（与 Subconstruct/Const 同模式：递归检查 inner 是否含表达式）。

#### 2.3.2 build_hex_node 编译逻辑（compile.rs）

新 `build_hex_node` 恢复 subcon 包装器模式：递归编译 subcon → 推断 HexInnerKind。

```rust
fn build_hex_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
) -> Result<HexNode, ConstructError> {
    // 1. 递归编译 subcon（与 Subconstruct/Rebuild/Const 同模式）。
    let subcon_desc = desc
        .getattr("subcon")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "HexDescriptor missing 'subcon' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let inner_node = build_node_from_descriptor(
        py, &subcon_desc, field_index, expr_programs, field_names, false,
    )?;

    // 2. 编译期推断 inner 输出类型（单一事实源，L-04 对策）。
    let inner_kind = infer_hex_inner_kind(&inner_node);

    Ok(HexNode::new(inner_node, inner_kind))
}

/// 根据 inner Node 变体推断其 parse 输出的 Python 类型分类。
///
/// **单一事实源**：新增 Node 变体若输出 bytes/int，必须在此 match 分支补全。
/// 未覆盖的变体归为 Unknown（parse/build 透传，fail-soft）。
///
/// **F-1 修订**：FormatField 细分——int 格式归 Int，6 个 Float 格式归 Unknown。
/// **F-3 修订**：透明包装器递归穿透——包装器 parse 输出 == inner 输出时，
/// 递归检查 inner 的输出类型（含多分支一致性检查）。
fn infer_hex_inner_kind(inner: &Node) -> HexInnerKind {
    use Node::*;
    match inner {
        // ---- 叶节点：输出 PyBytes ----
        Bytes(_) | GreedyBytes(_) => HexInnerKind::Bytes,

        // ---- 叶节点：输出 PyLong（int）----
        // F-1：FormatField 细分——仅 int 格式归 Int，Float 格式归 Unknown
        FormatField(ff) => {
            if ff.format().is_float() {
                HexInnerKind::Unknown   // Float16/32/64 → PyFloat → 透传
            } else {
                HexInnerKind::Int
            }
        }
        VarInt(_) | ZigZag(_) | BytesInteger(_) | BitsInteger(_)
        | Tell(_) | Computed(_) | Index(_) => HexInnerKind::Int,

        // ---- 透明单子包装器：parse 输出 == inner().parse 输出 → 递归（F-3）----
        // 注：各包装器是不同 Rust 类型，须分开 match arm（无法用 `|` 合并不同类型绑定）。
        // 已确认 accessor：这些 Node 均有 `pub fn inner(&self) -> &Node`。
        Subconstruct(s) => infer_hex_inner_kind(s.inner()),
        Peek(s) => infer_hex_inner_kind(s.inner()),
        Const(s) => infer_hex_inner_kind(s.inner()),
        Default(s) => infer_hex_inner_kind(s.inner()),
        Rebuild(s) => infer_hex_inner_kind(s.inner()),
        Aligned(s) => infer_hex_inner_kind(s.inner()),
        Transform(s) => infer_hex_inner_kind(s.inner()),
        Bitwise(s) => infer_hex_inner_kind(s.inner()),
        Bytewise(s) => infer_hex_inner_kind(s.inner()),
        ProcessXor(s) => infer_hex_inner_kind(s.inner()),
        ProcessRotateLeft(s) => infer_hex_inner_kind(s.inner()),
        NullTerminated(s) => infer_hex_inner_kind(s.inner()),
        NullStripped(s) => infer_hex_inner_kind(s.inner()),
        OneOf(s) => infer_hex_inner_kind(s.inner()),
        NoneOf(s) => infer_hex_inner_kind(s.inner()),

        // ---- 透明双字段包装器：parse 输出 == subcon().parse 输出 → 递归 subcon（F-3）----
        // Prefixed：lengthfield 仅消费定长，不进入输出；Pointer：offset 仅定位。
        // 已确认 accessor：`pub fn subcon(&self) -> &Node`（两者不同类型，分开 arm）
        Prefixed(p) => infer_hex_inner_kind(p.subcon()),
        Pointer(p) => infer_hex_inner_kind(p.subcon()),

        // ---- 多分支条件包装器：所有分支输出类型一致时归该类，否则 Unknown（F-3）----
        IfThenElse(ite) => {
            let t = infer_hex_inner_kind(ite.then_sub());
            let e = infer_hex_inner_kind(ite.else_sub());
            if t == e { t } else { HexInnerKind::Unknown }
        }
        Switch(sw) => {
            // 遍历所有 case + default，全一致则归该类
            let mut acc: Option<HexInnerKind> = None;
            for case in sw.cases() {
                let k = infer_hex_inner_kind(case.subcon());
                acc = Some(match acc { None => k, Some(prev) => if prev == k { prev } else { return HexInnerKind::Unknown } });
            }
            let d = infer_hex_inner_kind(sw.default());
            match acc { None => d, Some(prev) => if prev == d { prev } else { HexInnerKind::Unknown } }
        }
        Select(sel) => {
            // 遍历所有 subcons，全一致则归该类
            let mut acc: Option<HexInnerKind> = None;
            for sub in sel.subcons() {
                let k = infer_hex_inner_kind(sub);
                acc = Some(match acc { None => k, Some(prev) => if prev == k { prev } else { return HexInnerKind::Unknown } });
            }
            acc.unwrap_or(HexInnerKind::Unknown)
        }

        // ---- 非透明 / 复合输出 → Unknown 透传 ----
        // RawCopy（dict）/ Struct（dict）/ Sequence（list）/ Array/GreedyRange/PrefixedArray/
        // RepeatUntil（list）/ Union（dict/multi）/ NamedTuple（tuple）/ Enum/FlagsEnum/Mapping
        // （映射值类型不定）/ AdapterCallback（用户 _decode 输出未知）/ Hex/HexDump（str）/
        // Strings（str）/ Padding/BitPadding/Pass/Check/Terminated/Probe/Seek/StopIf/Element
        // （None/元数据）/ FocusedSeq（context nesting，聚焦字段类型不定）/ Checksum（特殊）
        _ => HexInnerKind::Unknown,
    }
}
```

**descriptor `_expr_params` 协议**（Python 侧）：HexDescriptor 不持有自身表达式（length 已废弃），
表达式来源全部来自 inner subcon 的递归编译。HexDescriptor 的 `_expr_params` 返回 inner 的
表达式参数（与 SubconstructDescriptor / PeekDescriptor 同模式，由 `build_node_from_descriptor`
递归处理）。

**关于 HexInnerKind 推断的维护性**（L-04 对策 + F-1/F-3 修订）：
- `infer_hex_inner_kind` 是"Node 变体 → 输出类型"映射的单一事实源
- **F-1 依赖**：`FormatField(ff).format().is_float()` 需要 `PythonFormat` 新增 `is_float()`
  方法（`format_field.rs`，匹配 6 个 Float 变体，与已有 `is_big_endian()` 同模式，~6 行）。
  若 DEV 不新增该方法，可内联 `matches!(ff.format(), PythonFormat::Float16Big | ...)` 替代
- **F-3 维护契约**：新增包装器 Node 时，若其 `parse 输出 == inner.parse 输出`
  （透明包装器），DEV/VET 必须在 `infer_hex_inner_kind` 的递归分支补全（否则该包装器
  被 Hex 包装时归 Unknown 透传，功能退化但不崩溃）。与 `has_expressions()` 递归维护同模式
- Unknown 分支是 fail-soft（不崩溃），但用户会观察到"Hex 包装后 parse 不是字符串"——
  这是引导用户使用受支持 subcon 类型的设计意图，非 bug

### 2.4 Construct impl（subcon 包装器 hex 编解码路径）

```rust
impl Construct for HexNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 1. inner.parse 得到原始对象（Rust 内部调用，不跨 FFI）
        let obj = self.inner.parse(py, stream, ctx, path)?;
        let bound = obj.bind(py);
        // 2. 按 inner_kind 分派 hex 编码
        match self.inner_kind {
            HexInnerKind::Bytes => {
                // 借用 PyBytes buffer（不拷贝）
                let bytes: &[u8] = bound.extract().map_err(|e| ConstructError::Generic {
                    message: format!(
                        "Hex parse (Bytes kind): inner returned non-bytes value: {}",
                        e
                    ),
                    path: path.to_string(),
                })?;
                let hex_str = hex::encode(bytes);  // 纯 Rust，SIMD 优化
                Ok(PyString::new_bound(py, &hex_str).into_any().unbind())
            }
            HexInnerKind::Int => {
                // F-2：i128 fast-path 覆盖 i64 全域 + u64::MAX + VarInt/BytesInteger ≤ 16 字节。
                // 超 i128 的任意精度（BytesInteger > 16 字节，极罕见）走 Python __format__ fallback。
                if let Ok(v) = bound.extract::<i128>() {
                    // 无前导零小写，对齐 Python format(v, 'x')；负数自动加 "-"
                    // u64::MAX (正 i128) → "ffffffffffffffff"；i64::MIN → "-8000000000000000"
                    let hex_str = format!("{:x}", v);
                    Ok(PyString::new_bound(py, &hex_str).into_any().unbind())
                } else {
                    // 任意精度 fallback（> 128 bits）：调 int.__format__(self, "x")（C API，
                    // §0.2 判据 2，C 级 builtin，无用户字节码）。仅 BytesInteger > 16 字节触发。
                    let hex_obj = bound
                        .call_method1("__format__", (PyString::new_bound(py, "x"),))
                        .map_err(|e| ConstructError::Generic {
                            message: format!("Hex parse (Int kind): __format__ failed: {}", e),
                            path: path.to_string(),
                        })?;
                    Ok(hex_obj.unbind())
                }
            }
            HexInnerKind::Unknown => {
                // 透传（fail-soft，对齐 Python 原版 _decode else 分支 return obj）
                Ok(obj)
            }
        }
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        match self.inner_kind {
            HexInnerKind::Bytes => {
                // obj 应为 hex 字符串
                let s: String = obj.extract().map_err(|_| ConstructError::Generic {
                    message: format!(
                        "Hex build (Bytes kind) expected hex string, got type {}",
                        obj.get_type()
                            .name()
                            .map(|n| n.to_string())
                            .unwrap_or_else(|_| "<unknown>".to_string())
                    ),
                    path: path.to_string(),
                })?;
                // Rust 内 hex::decode（纯 Rust，大小写兼容）
                let bytes: Vec<u8> = hex::decode(&s).map_err(|e| ConstructError::Generic {
                    message: format!("Hex build (Bytes kind): failed to decode hex string '{}': {}", s, e),
                    path: path.to_string(),
                })?;
                let bytes_obj = PyBytes::new_bound(py, &bytes);
                self.inner.build(py, &bytes_obj, stream, ctx, path)
            }
            HexInnerKind::Int => {
                let s: String = obj.extract().map_err(|_| ConstructError::Generic {
                    message: format!(
                        "Hex build (Int kind) expected hex string, got type {}",
                        obj.get_type()
                            .name()
                            .map(|n| n.to_string())
                            .unwrap_or_else(|_| "<unknown>".to_string())
                    ),
                    path: path.to_string(),
                })?;
                // 剥可选 "0x"/"-0x" 前缀（对齐 Python int(s, 16) 宽容性）
                let negative = s.starts_with('-');
                let stripped = s
                    .trim_start_matches('-')
                    .trim_start_matches("0x");
                let radix_str = if negative { format!("-{}", stripped) } else { stripped.to_string() };
                // F-2：i128 fast-path 覆盖 u64::MAX 等；超 i128 的 hex 字符串走 Python int(s,16) fallback
                if let Ok(v) = i128::from_str_radix(&radix_str, 16) {
                    let int_obj = v.into_py(py);
                    self.inner.build(py, int_obj.bind(py), stream, ctx, path)
                } else {
                    // 任意精度 fallback：调 builtins.int(s, 16)（C API，§0.2 判据 2）
                    let builtins = py.import_bound("builtins").map_err(|e| ConstructError::Generic {
                        message: format!("Hex build (Int kind): import builtins failed: {}", e),
                        path: path.to_string(),
                    })?;
                    let int_obj = builtins
                        .call_method1("int", (&radix_str, 16))
                        .map_err(|e| ConstructError::Generic {
                            message: format!("Hex build (Int kind): int('{}', 16) failed: {}", s, e),
                            path: path.to_string(),
                        })?;
                    // int_obj 是 Bound<PyAny>（PyLong），转交 inner.build
                    self.inner.build(py, &int_obj, stream, ctx, path)
                }
            }
            HexInnerKind::Unknown => {
                // 透传（obj 原样传给 inner.build，对齐 Python 原版 _encode return obj）
                self.inner.build(py, obj, stream, ctx, path)
            }
        }
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // 转发 inner.sizeof（与 Subconstruct/Const/Peek 同模式）
        self.inner.sizeof(ctx)
    }
}
```

**关键实现说明**：
1. **inner.parse/build 是 Rust 内部函数调用**（§0 #1 合规）——subcon 包装器模式与 Subconstruct 同构，
   hex 编解码在 inner 结果之上叠加，不构成额外 FFI
2. **Bytes kind 的 `extract::<&[u8]>` 借用不拷贝**——pyo3 直接引用 PyBytes 内部 buffer
3. **Int kind 的 `extract::<i128>` + fallback**（F-2）——`i128` 覆盖 i64/u64 全域 +
   VarInt/BytesInteger ≤ 16 字节；超 i128 的任意精度走 Python `int.__format__`（C API，
   仅 BytesInteger > 16 字节触发，极罕见）。负数 `format!("{:x}", -31i128)` → `"-1f"`
4. **Unknown kind 透传**——不尝试 hex 编解码，obj 原样传递（round-trip 对称）
5. **build Int kind 的前缀 strip**——手动处理 `0x`/`-0x` 前缀（Rust `from_str_radix` 不接受前缀），
   保持与 Python `int(s, 16)` 一致的宽容性；任意精度走 Python `int(s, 16)` fallback
6. **F-3 透明包装器递归是编译期操作**——`infer_hex_inner_kind` 在 `build_hex_node`（编译期）
   执行，递归穿透不影响运行期 parse/build 性能（0 额外运行时开销）

### 2.5 §0 原则对照表（L-01 对策）

| §0 原则 | HexNode（subcon 包装器 hex 格式转换器） |
|---------|----------------------|
| #1 一次 FFI | ✅ **严格 1 次**。`inner.parse`/`inner.build` 是 Rust 内部函数调用（与 Subconstruct 同构，subcon 包装器模式不构成额外 FFI）；`hex::encode`/`hex::decode` 全在 Rust 内（纯 Rust crate）；`extract`/`PyString::new_bound`/`PyBytes::new_bound`/`into_py` 是 C API（§0.2 判据 2）。**F-2 fallback**（`int.__format__`/`int(s,16)`，仅任意精度 > 128bit 触发）是 C 级 builtin 方法调用（§0.2 判据 2，无用户字节码）。**无 Python 字节码进入、无 Rust→Python 回调** |
| #2 无中间表示层 | ✅ inner.parse 返回的 `Py<PyAny>` 直接处理（按 HexInnerKind 分派），不引入 Rust 中间数据类型。`hex::encode` 产出 Rust `String`（原生类型，非跨 FFI 中间层），随即构造 `PyString`（Python 对象本身） |
| #3 输入输出无 trait 抽象 | ✅ 仅 `Box<Node>`（与 Subconstruct/Const 同模式）+ `HexInnerKind` enum（编译期分类，非运行期 trait dispatch） |
| #4 pyo3 核心依赖 | ✅ `hex = "0.4"` 是纯 Rust 编解码库（无 Python 跨界），与 `half`/`sha2` 同先例。pyo3 仍是 FFI 唯一桥梁 |
| #5 mashumaro API | ✅ `Hex(subcon)` 是字段描述符（与 Subconstruct/Peek 同层，包装任意 subcon） |
| #6 enum_dispatch | ✅ HexNode 重写（变体数不变，HexDump 视用户决策） |
| #7 Result + path | ✅ ConstructError::Generic（hex decode/parse 失败、类型不匹配）携带 path |
| #8 Stream 纯 Rust | ✅ inner.parse/build 操作 stream（Rust 内部），Hex 不直接操作 stream（与 Subconstruct 同） |

### 2.6 性能假设（L-02/L-05 对策）

#### 2.6.1 瓶颈识别（量化数据 + 来源）

数据源：`phase8-hex-parse-perf-investigation.md`（HX1 当前 581ns）+ BytesNode/FormatField 性能基准。

subcon 包装器模式比第一轮 `Hex(length)` 多一层 `inner.parse` 调用（Rust 内部，~60-80ns）。

**Bytes kind（用户核心场景：Hex(GreedyBytes)）**：

| 操作 | 估算（Rust 侧） | 类别 | 来源 |
|------|----------------|------|------|
| `inner.parse`（GreedyBytes） | ~60-80ns | Rust 内部调用 | GreedyBytes 基准 |
| `extract::<&[u8]>()` | ~10ns | C API（借用，不拷贝） | pyo3 基准 |
| `hex::encode(&[u8])` | ~5-10ns | 纯 Rust，SIMD | hex crate |
| `PyString::new_bound` | ~45-50ns | C API（§0.2 判据 2） | pyo3 基准 |
| **parse 合计（Bytes kind）** | **~120-150ns** | | |

**Int kind（兼容场景：Hex(Int32ub) / Hex(Int64ub)，F-2 i128 路径）**：

| 操作 | 估算（Rust 侧） | 类别 | 来源 |
|------|----------------|------|------|
| `inner.parse`（FormatField Int32ub） | ~60-80ns | Rust 内部调用 | FormatField 基准 |
| `extract::<i128>()`（F-2，替代 i64） | ~12-15ns | C API | pyo3 基准（i128 比i64 略慢，仍在 ns 级） |
| `format!("{:x}", v)`（i128） | ~20ns | Rust 原生格式化 | Rust std |
| `PyString::new_bound` | ~45-50ns | C API | pyo3 基准 |
| **parse 合计（Int kind，fast-path）** | **~140-165ns** | | |
| （fallback：`__format__` C API） | ~80-120ns | 仅 BytesInteger > 16B 触发 | 极罕见，不进 hot path |

| build 操作 | 估算 | 类别 | 来源 |
|------|------|------|------|
| `obj.extract::<String>()` | ~30ns | C API | pyo3 基准 |
| `hex::decode` / `from_str_radix` | ~10-15ns | 纯 Rust | hex crate / std |
| `PyBytes::new_bound` / `into_py` | ~40-50ns | C API | pyo3 基准 |
| `inner.build` | ~60-80ns | Rust 内部调用 | 各 Node 基准 |
| **build 合计** | **~140-175ns** | | |

#### 2.6.2 可证伪预测（覆盖所有 FFI/拷贝来源）

**嵌入 Struct 字段，与 Python construct 2.10.70（Hex(Int32ub) display 装饰器）对比**：

- **Hex(Int32ub) parse（Int kind）**：加速比 **≥15x**（预测 rs ~150ns vs py 2519ns）。
  - **对比当前实现**：原 HX1 parse 581ns（4.36x）→ 新设计 ~150ns（~17x）。**提升来源**：
    (1) 消除 is_instance_of ~15ns（改编译期 HexInnerKind）；(2) 消除 call_method1 + 字节码
    dispatch ~150ns；(3) 消除 int 子类实例化 + setattr ~105ns；(4) 消除 fmtstr 构造 ~60ns。
    保留 inner.parse ~70ns（必要开销），新增 hex 编码 + PyString ~70ns。
  - **与第一轮 Hex(length) 对比**：第一轮 ~100ns（~25x）更快（无 inner.parse），但第二轮
    保留 subcon 包装器能力（用户需求），多 ~50ns 仍远超 10x 门禁
  - **可证伪条件**：若实测 >250ns（加速比 <10x），说明 inner.parse 或 extract 开销被低估——
    需进一步调查
- **Hex(GreedyBytes) parse（Bytes kind，用户核心场景）**：加速比 **≥15x**（预测 rs ~130ns
  vs py ~2500ns 量级）。GreedyBytes 内部已优化，hex::encode 仅 ~10ns 额外
- **build**：加速比 **≥12x**（预测 rs ~150ns vs py ~2053ns 量级）

**FFI/拷贝来源清单**（L-05 对策，新设计后剩余）：
- StructMixin.parse/build 入口（1 次 FFI，共享，不可消除）
- inner.parse/inner.build（Rust 内部函数调用，0 FFI）
- hex::encode/decode / `format!` / `i128::from_str_radix`（Rust 内，0 FFI）
- extract(i128) / PyString::new_bound / PyBytes::new_bound / into_py（C API，§0.2 判据 2，不计额外 FFI）
- **F-2 fallback**（`int.__format__` / `int(s,16)`，仅任意精度 > 128bit 触发）：C 级 builtin 方法调用（§0.2 判据 2），不进 hot path
- **无 Rust→Python 回调**（严格 1 次 FFI）

#### 2.6.3 与 Subconstruct/Bytes 的对照（验证合理性）

新 HexNode 的 parse 路径 = Subconstruct.parse（inner.parse 转发）+ hex 编码层。Subconstruct
parse 实测加速比 ~10x+（Phase 6.3 基准），HexNode 预期同量级或略低（hex 编码 + PyString 构造
额外 ~70ns），仍远超 10x 门禁。Bytes kind 的 hex::encode 与 BytesNode 的 PyBytes 构造同量级。

**量级参考声明**（L-09）：以上 ns 估算为量级参考，绝对值需 DEV 实施后 Controlled A/B Test
复测验证。

### 2.7 边界条件清单

| 编号 | 场景 | 预期行为 |
|------|------|---------|
| HX-1 | `Hex(Bytes(4)).parse(b'\x00\x00\x01\x02')`（Bytes kind） | 返回 `"00000102"`（小写 hex 字符串） |
| HX-2 | `Hex(Bytes(4)).build("0000001f")`（Bytes kind） | 输出 `b'\x00\x00\x00\x1f'` |
| HX-3 | `Hex(Bytes(4)).build("0000001F")` | 输出 `b'\x00\x00\x00\x1f'`（大小写兼容，hex::decode 接受大写） |
| HX-4 | `Hex(Bytes(4)).build("0000001")` | ConstructError::Generic（hex 字符串长度必须偶数，7 chars 非法） |
| HX-5 | `Hex(Bytes(4)).build("0000001g")` | ConstructError::Generic（'g' 非合法 hex 字符） |
| HX-6 | `Hex(Bytes(4)).build("00")` | inner.build(bytes(1 byte)) → inner 自身校验长度（Bytes(4) 期望 4 字节，报 FieldLength）。Hex 不做长度校验（转交 inner） |
| HX-7 | `Hex(Bytes(4)).build(258)` | ConstructError::Generic（期望 str，收到 int） |
| HX-8 | `Hex(Bytes(4)).parse(b'\x00\x00\x01')` | ConstructError::Stream（inner.parse Bytes(4) 仅 3 字节，不足 4） |
| HX-9 | `Hex(Bytes(4)).sizeof()` | 返回 4（转发 inner.sizeof） |
| HX-10 | `Hex(Bytes(0)).parse(b'')` | 返回 `""`（空 hex 字符串） |
| HX-11 | `Hex(Bytes(0)).build("")` | 输出 `b''`（空字节） |
| HX-12 | round-trip（Bytes kind）：`Hex(Bytes(4)).build(Hex(Bytes(4)).parse(data)) == data` | 恒成立（parse 输出小写，build 大小写兼容） |
| HX-13 | `Hex(GreedyBytes).parse(b'\xde\xad\xbe\xef')`（用户核心场景） | 返回 `"deadbeef"`（变长 bytes → hex） |
| HX-14 | `Hex(GreedyBytes).build("deadbeef")` | 输出 `b'\xde\xad\xbe\xef'`（GreedyBytes build 接收任意长度 bytes） |
| HX-15 | `Hex(Int32ub).parse(b'\x00\x00\x01\x02')`（Int kind） | 返回 `"102"`（无前导零小写，format!("{:x}", 258)） |
| HX-16 | `Hex(Int32ub).build("102")`（Int kind） | 输出 `b'\x00\x00\x01\x02'`（from_str_radix → 258 → inner.build） |
| HX-17 | `Hex(Int32ub).build("0x102")` | 输出 `b'\x00\x00\x01\x02'`（build 接受 "0x" 前缀，strip 后解析） |
| HX-18 | `Hex(Int32sb).build("-1f")`（负数） | 输出对应负数字节（format 时 `"-1f"`，strip 后保留负号 → -31） |
| HX-19 | round-trip（Int kind）：`Hex(Int32ub).build(Hex(Int32ub).parse(data)) == data` | 恒成立（无前导零，from_str_radix 解析后 inner.build 写 4 字节） |
| HX-20 | `Hex(Int32ub).build("1g")` | ConstructError::Generic（'g' 非合法 hex 字符） |
| HX-21 | `Hex(Struct(...)).parse(...)`（Unknown kind） | **透传**：返回 inner.parse 结果（dict/dataclass），不做 hex 编码（fail-soft） |
| HX-22 | `Hex(Struct(...)).build(obj)`（Unknown kind） | **透传**：obj 原样传 inner.build（fail-soft） |
| HX-23 | `Hex(RawCopy(Int32ub)).parse(...)`（Unknown kind） | **透传**：返回 RawCopy dict（含 data/value/offset1/offset2/length） |
| HX-24 | `Hex(Bytes(4)).build(None)` | ConstructError::Generic（期望 str，收到 None） |
| HX-25 | `Hex(Prefixed(Byte, GreedyBytes)).parse(...)`（变长前缀 bytes，Bytes kind） | 正确：Prefixed inner parse 得 bytes → hex 编码 |
| HX-26 | `Hex(Float32b).parse(b'\x42\x28')`（F-1：Float 格式 → Unknown） | **透传**：返回原始 PyFloat（不做 hex 编码，对齐 Python 原版 `_decode` fallback） |
| HX-27 | `Hex(Int64ub).parse(b'\xff\xff\xff\xff\xff\xff\xff\xff')`（F-2：u64::MAX） | 返回 `"ffffffffffffffff"`（i128 fast-path，16 个 f，无前导零） |
| HX-28 | `Hex(Int64ub).build("ffffffffffffffff")`（F-2：u64::MAX build） | 输出 `b'\xff' * 8`（i128::from_str_radix → u64 PyLong → inner.build） |
| HX-29 | `Hex(BytesInteger(20)).parse(<20 bytes>)`（F-2：任意精度 > i128） | 返回 hex 字符串（Python `__format__` fallback，无前导零）；合法数据不报错 |
| HX-30 | `Hex(Subconstruct(Bytes(4))).parse(...)`（F-3：透明包装器递归） | 返回 hex 字符串（Subconstruct 透明 → 递归命中 Bytes → Bytes kind） |
| HX-31 | `Hex(IfThenElse(cond, Bytes(4), Bytes(8))).parse(...)`（F-3：多分支一致） | 返回 hex 字符串（两分支均 Bytes → Bytes kind） |
| HX-32 | `Hex(IfThenElse(cond, Bytes(4), Int32ub)).parse(...)`（F-3：多分支不一致） | **透传**（分支类型不一致 → Unknown，fail-soft） |
| HX-33 | `Hex(Const(255, Int32ul)).parse(...)`（F-3：Const 透明递归） | 返回 hex 字符串（Const 透传 inner Int32ul → Int kind，format 常量值） |

> **边界条件说明**：
> - HX-6 关键差异：subcon 包装器模式不做 Hex 自身的长度校验（第一轮 `Hex(length)` 会校验），
>   长度校验转交 inner（Bytes(4) 自身会校验）。这符合"Hex 是格式转换器"的定位
> - HX-21/22/23 Unknown 透传是 fail-soft 设计：用户若需 hex 字符串，应使用 Bytes/GreedyBytes/
>   Int 类 subcon，而非 Struct/RawCopy。文档需明确标注受支持的 subcon 类型
> - **F-1（HX-26）**：Float 格式（Float16/32/64）parse 返回 PyFloat，归 Unknown 透传——
>   hex 编码浮点数无语义，透传是对齐 Python 原版 `return obj` 的 graceful fail
> - **F-2（HX-27~29）**：i128 fast-path 覆盖所有 FormatField + u64::MAX + VarInt/BytesInteger
>   ≤ 16 字节；任意精度 fallback 确保"合法数据不报错"（Int64ub 是常用格式，必须正确）
> - **F-3（HX-25/30~33）**：透明包装器递归使 `Hex(Prefixed(...))` / `Hex(Subconstruct(...))`
>   等组合正确输出 hex（符合用户"格式转换器"心智模型）；多分支不一致时 fail-soft 透传

### 2.8 DEV 实施清单

| 文件 | 内容 | 行数估 |
|------|------|-------|
| `nodes/hex.rs` | **完全重写**：HexNode（subcon 包装器）+ `HexInnerKind` enum + `Construct` impl（F-2 i128 + fallback 路径）+ 单元测试。删除 HexDisplayClasses / load_hex_display_classes / fmtstr / call_method1/setattr 全部旧逻辑 | ~280 |
| `nodes/hex_dump.rs` | **视用户决策**（§2.10）：方案 C 删除此文件；方案 A 重写为 hexdump 字符串编解码器 | 0 或 ~250 |
| `nodes/mod.rs` | 更新 `Hex(HexNode)` 注释（subcon 包装器语义）；HexDump 变体视决策保留/移除；has_expressions 分支改为 `h.inner().has_expressions()`（递归 inner） | ~10 增量 |
| `nodes/format_field.rs` | **F-1 依赖**：新增 `PythonFormat::is_float()` 方法（匹配 6 个 Float 变体，~6 行，与 `is_big_endian()` 同模式） | ~6 增量 |
| `compile.rs` | **重写** `build_hex_node` + `infer_hex_inner_kind`（F-1 FormatField 细分 + F-3 透明包装器递归 + 多分支一致性，~50 行）；`build_hexdump_node` 视决策删除/重写；删除 display_classes 物化逻辑 | ~50 增量 |
| `Cargo.toml` | 新增 `hex = "0.4"` | 1 行 |
| `python/construct/lib/hex.py` | **视用户决策**：方案 C 删除显示类（仅保留 `hexdump`/`hexundump` 函数供用户可选调用）；方案 A 保留/精简 | — |
| `python/construct/_descriptors.py` | **恢复** HexDescriptor（接收 `subcon`，与 SubconstructDescriptor 同模式）+ `Hex()` 工厂（签名恢复 `Hex(subcon)`）；HexDumpDescriptor 视决策删除/重写 | ~20 增量 |
| `python/construct/__init__.py` | 更新导出（Hex 文档化 subcon 包装器语义 + 受支持 subcon 类型） | ~5 增量 |
| `bench/bench_phase8.py` | 更新 HX1/HX2 case（保持 `Hex(Int32ub)` + 新增 `Hex(GreedyBytes)` case；期望值 str）；HD1 视决策删除 | ~15 增量 |
| `bench/bench_phase8_ab_retest.py` | 同上 | ~15 增量 |
| `bench/bench_hex_opt_ab_retest.py` | **删除**（方案 A+B 优化测试不再适用） | -106 |
| `bench/hex_parity_check.py` | **重写**（验证新语义：Bytes/Int kind parse 返回 str，build 接收 str；Unknown 透传） | ~100 |
| `docs/perf-scenarios.csv` | 更新 HX1/HX2/HD1 行 + 新增 Hex(GreedyBytes) 场景（PM 职责，ARCH 提供数据） | — |

**净代码变化**：删除 ~400 行旧代码（display 类 + 调用逻辑 + 优化测试），新增 ~250 行新代码。
总体代码量减少，复杂度大幅降低。相比第一轮 `Hex(length)` 方案，第二轮 subcon 包装器多
`HexInnerKind` enum + `infer_hex_inner_kind` 函数（~30 行），但恢复了 subcon 包装能力。

### 2.9 Parity 测试模板（subcon 包装器新语义）

```python
# 新语义 parity：construct-rs Hex(subcon)
# 值语义 breaking change（parse 返回 str），API 形态保持 Hex(subcon)

# === Bytes kind（用户核心场景）===

def test_hex_bytes_parse_returns_lowercase_string():
    # Hex(Bytes(4)).parse → hex 字符串
    @dataclass
    class P(StructMixin):
        v: str = field(Hex(Bytes(4)))
    result = P.parse(b'\x00\x00\x01\x02')
    assert result.v == "00000102"
    assert isinstance(result.v, str)  # 不是 bytes 子类

def test_hex_greedybytes_parse():  # 用户核心场景（变长）
    @dataclass
    class P(StructMixin):
        v: str = field(Hex(GreedyBytes))
    result = P.parse(b'\xde\xad\xbe\xef')
    assert result.v == "deadbeef"

def test_hex_bytes_build_from_string():
    @dataclass
    class P(StructMixin):
        v: str = field(Hex(Bytes(4)))
    assert P(v="0000001f").build() == b'\x00\x00\x00\x1f'

def test_hex_bytes_build_accepts_uppercase():
    @dataclass
    class P(StructMixin):
        v: str = field(Hex(Bytes(4)))
    assert P(v="0000001F").build() == b'\x00\x00\x00\x1f'

def test_hex_bytes_round_trip():
    @dataclass
    class P(StructMixin):
        v: str = field(Hex(Bytes(4)))
    data = b'\x00\x00\x01\x02'
    parsed = P.parse(data)
    rebuilt = parsed.build()
    assert rebuilt == data

def test_hex_bytes_build_odd_length_rejected():
    @dataclass
    class P(StructMixin):
        v: str = field(Hex(Bytes(4)))
    with pytest.raises(ConstructError):
        P(v="0000001").build()  # 7 hex chars（奇数）

# === Int kind（兼容场景）===

def test_hex_int_parse_no_leading_zeros():
    # Hex(Int32ub).parse → format!("{:x}", v)，无前导零
    @dataclass
    class P(StructMixin):
        v: str = field(Hex(Int32ub))
    result = P.parse(b'\x00\x00\x01\x02')  # 258
    assert result.v == "102"  # 非 "00000102"

def test_hex_int_build():
    @dataclass
    class P(StructMixin):
        v: str = field(Hex(Int32ub))
    assert P(v="102").build() == b'\x00\x00\x01\x02'

def test_hex_int_build_accepts_0x_prefix():
    @dataclass
    class P(StructMixin):
        v: str = field(Hex(Int32ub))
    assert P(v="0x102").build() == b'\x00\x00\x01\x02'

def test_hex_int_round_trip():
    @dataclass
    class P(StructMixin):
        v: str = field(Hex(Int32ub))
    data = b'\x00\x00\x01\x02'
    assert P.build(P.parse(data)) == data

# === Unknown kind（fail-soft 透传）===

def test_hex_struct_passthrough():
    # Hex(Struct(...)) inner_kind=Unknown → parse 透传
    @dataclass
    class Inner(StructMixin):
        a: int = rfield(Byte)
    @dataclass
    class P(StructMixin):
        v: Inner = field(Hex(Inner))
    result = P.parse(b'\x42')
    # Unknown 透传：返回 Inner 实例（不是 hex 字符串）
    assert isinstance(result.v, Inner)
    assert result.v.a == 0x42

# === F-1/F-2/F-3 修订场景（REV followup）===

def test_hex_float_passthrough():  # F-1：Float 格式 → Unknown 透传
    @dataclass
    class P(StructMixin):
        v: float = field(Hex(Float32b))
    result = P.parse(b'\x42\x28\x00\x00')  # 65.0
    assert isinstance(result.v, float)      # 不是 str，透传原始 float
    assert result.v == 65.0

def test_hex_int64ub_max():  # F-2：u64::MAX 不报错
    @dataclass
    class P(StructMixin):
        v: str = field(Hex(Int64ub))
    result = P.parse(b'\xff\xff\xff\xff\xff\xff\xff\xff')
    assert result.v == "ffffffffffffffff"   # 16 个 f，无前导零
    # round-trip
    assert P(v="ffffffffffffffff").build() == b'\xff' * 8

def test_hex_prefixed_greedybytes():  # F-3：透明包装器递归（用户核心变长场景）
    @dataclass
    class P(StructMixin):
        v: str = field(Hex(Prefixed(Byte, GreedyBytes)))
    result = P.parse(b'\x04\xde\xad\xbe\xef')
    assert result.v == "deadbeef"           # Prefixed 透传 → GreedyBytes → Bytes kind

def test_hex_subconstruct_passthrough_to_bytes():  # F-3：Subconstruct 透明
    @dataclass
    class P(StructMixin):
        v: str = field(Hex(Subconstruct(Bytes(4))))
    result = P.parse(b'\x00\x00\x01\x02')
    assert result.v == "00000102"

def test_hex_mixed_branches_passthrough():  # F-3：多分支不一致 → Unknown
    @dataclass
    class P(StructMixin):
        flag: int = rfield(Byte)
        v: object = field(Hex(IfThenElse(flag > 0, Bytes(4), Int32ub)))
    result = P.parse(b'\x01\x00\x00\x01\x02')
    # 分支类型不一致 → Unknown → 透传（返回 bytes 或 int，非 hex str）
    assert not isinstance(result.v, str)
```

**Parity 重点**：
- HX-1/2/3/12 Bytes kind：parse 返回 str（小写连写），build 接收 str
- HX-13/14 GreedyBytes（用户核心场景）：变长 bytes → hex 字符串
- HX-15/16/19 Int kind：parse 返回无前导零 hex（`"102"`），build 接受纯 hex + "0x" 前缀
- HX-21/22/23 Unknown kind：透传（fail-soft），round-trip 对称
- **F-1（HX-26）**：Float 格式透传（Hex(Float32b) parse 返回 float，非 str）
- **F-2（HX-27/28）**：u64::MAX（`Hex(Int64ub)` parse/build round-trip，合法数据不报错）
- **F-3（HX-25/30/31/32）**：透明包装器递归（Hex(Prefixed/Subconstruct/IfThenElse) 正确分派；
  多分支不一致时 fail-soft 透传）
- 不再测试 `Hex(Int32ub)` int 显示子类、`Hex(RawCopy(...))` dict 显示子类（原版独有 display 语义）

### 2.10 HexDump 处理方案（需用户决策）

> ARCH 推荐见 `docs/design/queries/决策-HexDump处理方案.md`。本节摘要三个方案。

| 方案 | 描述 | ARCH 评估 |
|------|------|----------|
| **A** | 同样改为"返回 hexdump 格式字符串"，build 逆向解析 | parse 可行（Rust 内构造多行格式），但 build 逆向解析 hexdump 格式（剥地址/ASCII 冗余）复杂、易错、工程量大 |
| B | 与 Hex 合并 | 不推荐——hexdump 多行带 ASCII 格式与纯 hex 字符串本质不同，合并丢失可读性优势 |
| **C** ⭐ | 标 wont_implement（hexdump 是纯显示功能，build 逆向不值得实现） | **推荐**：用户可用 `construct.lib.hex.hexdump()` 函数对 parse 出的 bytes 做后处理（纯显示，不进入执行树）。详见决策文档 |

**ARCH 推荐 C**，理由：(1) hexdump 的核心价值是"人类可读调试显示"，在数据编解码场景无实际用途；
(2) build 逆向解析 hexdump 格式价值低且易错；(3) 删除 HexDump 可减少 ~250 行代码 + 1 个 Node 变体。
若用户坚持保留，方案 A 作为备选（parse 仅，build 暂不支持或简化）。

---

## 3. 子任务 8.5：Checksum + Rust hashfunc + ParseStream::slice ⭐ 重点

> **L-14 教训触发**：本设计是 `质疑-能否不用RawCopy.md §7` 修正结论的工程化落地。
> ARCH 承认原 §1.4 把"hashfunc 是 Python callable → 跨 FFI 必须拷贝"当硬约束存在盲点，
> 漏了 Rust 内置 hashfunc 零拷贝路径。本节正式确认双轨方案。

### 3.1 Python 参考实现摘要

`Checksum(checksumfield, hashfunc, bytesfunc)`（core.py L5532-5600）：

- **parse**：`hash1 = checksumfield.parse(stream)`；`hash2 = hashfunc(bytesfunc(context))`；不等抛 ChecksumError；返回 hash1
- **build**：`hash2 = hashfunc(bytesfunc(context))`；`checksumfield.build(hash2)`
- **sizeof**：转发 `checksumfield.sizeof`

**Python 关键事实**：
- `bytesfunc(context)` 是用户提供的 context lambda（通常 `this.fields.data`，配合 RawCopy 把 raw bytes 塞进 context）
- `hashfunc(bytes)` 是 Python callable（通常 `lambda d: hashlib.sha512(d).digest()`）
- **Checksum 不读 stream**（除 checksumfield 自身）——它依赖 RawCopy 把字节塞进 context

### 3.2 双轨方案（PM 决策 D-1 已接受）

#### 3.2.1 路径 A：Python callable + ContextBytes（parity 兼容路径）

对齐 Python 原版用法，保留现有用户代码：

```python
# Python construct 原版写法（保留兼容）
import hashlib
d = Struct(
    "fields"   / RawCopy(Struct(...)),
    "checksum" / Checksum(Bytes(64), lambda d: hashlib.sha512(d).digest(), this.fields.data),
)
```

construct-rs 实现约束：表达式系统不接 lambda（ADR-014），但 `bytesfunc` 必须接 context 表达式（编译为 ExprProgram）；`hashfunc` 是 Python callable（运行期 FFI 回调）。

#### 3.2.2 路径 B：Rust 内置 hashfunc + StreamRange（零拷贝扩展路径）⭐

construct-rs 扩展，零拷贝最优解：

```python
# construct-rs 扩展写法（推荐用于高性能场景）
from dataclasses import dataclass
from construct import StructMixin, rfield, field, Bytes, Tell, Checksum, HashAlgo

@dataclass
class Packet(StructMixin):
    start: int = rfield(Tell())                       # 标记起始
    fields: ... = field(...)                          # 被校验数据（无 RawCopy 包装）
    end: int = rfield(Tell())                         # 标记结束
    checksum: bytes = rfield(Checksum(Bytes(32), HashAlgo.SHA256, start, end))
    # 字段名直接引用 start/end（非 this.start/this.end，L-13 教训）
```

**HashAlgo Python enum**（construct-rs 扩展）：
```python
class HashAlgo(Enum):
    MD5     = auto()
    SHA1    = auto()
    SHA256  = auto()
    SHA512  = auto()
    CRC32   = auto()    # zlib.crc32 等价
    ADLER32 = auto()    # zlib.adler32 等价
```

### 3.3 基础设施 #1：ParseStream::slice（新 API）

#### 3.3.1 设计

```rust
impl<'a> ParseStream<'a> {
    /// 借用底层缓冲的 `[start..end)` 切片（零拷贝）。
    ///
    /// Phase 8.5 新增（设计 §3.3.1，L-14 教训触发）。
    /// 用于 Checksum StreamRange 模式：Rust 内置 hashfunc 直接操作 `&[u8]`，
    /// 全程零拷贝（不构造 PyBytes）。
    ///
    /// # 行为
    ///
    /// - `start > end` 或 `end > data.len()`：返回 None
    /// - 不修改 stream 游标（纯借用查询，与 read 不同）
    /// - 不要求 bit_pos == 0（切片不消费字节）
    ///
    /// # 安全
    ///
    /// 返回 `Option<&'a [u8]>`，生命周期绑定到 ParseStream 持有的 `&'a [u8]`
    /// （即 Python 端 PyBytes 的 buffer，pyo3 GIL 保证有效）。
    pub fn slice(&self, start: usize, end: usize) -> Option<&'a [u8]> {
        self.data.get(start..end)
    }
}
```

**实现规模**：~10 行（含边界校验 + 文档注释）。

**与已有 `data()` 方法关系**：stream.rs L335 已有 `pub fn data(&self) -> &'a [u8]`（返回整个缓冲）。`slice` 是 `data().get(start..end)` 的封装，提供更清晰的边界校验语义。

#### 3.3.2 §0 合规

| §0 原则 | 合规性 |
|---------|--------|
| #1 一次 FFI | ✅ slice 是纯 Rust 内部操作 |
| #2 无中间表示层 | ✅ `&[u8]` 是借用引用，非中间数据类型 |
| #3 输入输出无 trait 抽象 | ✅ |
| #8 Stream 纯 Rust | ✅ |

### 3.4 基础设施 #2：Rust hashfunc crate（Cargo.toml 新增）

| crate | 版本 | 用途 | BuiltinHash 变体 |
|-------|------|------|-----------------|
| `sha2` | 0.10 | SHA-224/256/384/512 | `Sha256` / `Sha512` |
| `sha1` | 0.10 | SHA-1 | `Sha1` |
| `md-5` | 0.10 | MD5 | `Md5` |
| `crc32fast` | 1.4 | CRC32（zlib.crc32 等价） | `Crc32` |
| `adler` | 1.0 | Adler32（zlib.adler32 等价） | `Adler32` |

**Cargo.toml 追加**（在 `[dependencies]` 段，与 `half = "2.4"` 同位置）：
```toml
# Phase 8.5：Checksum Rust 内置 hashfunc（L-14 教训触发，零拷贝路径）。
# 纯 Rust 计算库（不跨 FFI），与 §0 #4（pyo3 核心依赖）不冲突——`half` 已有先例。
sha2 = "0.10"
sha1 = "0.10"
md-5 = "0.10"
crc32fast = "1.4"
adler = "1.0"
```

**§0 #4 合规**：5 个 crate 均为纯 Rust 计算库（无 Python 跨界），与 Phase 6.1 的 `half = "2.4"`（Float16）同性质。pyo3 仍是 FFI 唯一桥梁。

### 3.5 ChecksumNode Rust 设计

#### 3.5.1 数据结构

```rust
/// 校验和节点：parse 校验 hash，build 计算 hash。
///
/// 对应 Python construct `Checksum(checksumfield, hashfunc, bytesfunc)`
/// （core.py L5532）。construct-rs 扩展支持两种 hashfunc 类型与两种
/// bytes_source 类型，组合出 4 条路径（详见 §3.5.4 路径矩阵）。
///
/// # 双轨方案（PM 决策 D-1 接受）
///
/// - **路径 A**（parity 兼容）：Python callable hashfunc + ContextBytes
///   bytes_source——对齐 Python 原版，1 次拷贝（CPython 硬约束）
/// - **路径 B**（零拷贝扩展）：Rust 内置 hashfunc + StreamRange bytes_source
///   ——construct-rs 扩展，全程零拷贝（L-14 教训触发）
#[derive(Debug)]
pub struct ChecksumNode {
    /// 校验字段节点（通常 Bytes(32)/Bytes(64)）。
    checksumfield: Box<Node>,
    /// 哈希函数：Rust 内置（零拷贝）或 Python callable（兼容）。
    hashfunc: HashFunc,
    /// 被哈希字节来源：context 表达式求值（原版）或 stream 切片（扩展）。
    bytes_source: BytesSource,
}

/// 哈希函数类型（双轨）。
#[derive(Debug)]
pub enum HashFunc {
    /// Rust 内置哈希（零拷贝，操作 `&[u8]`）。
    BuiltIn(BuiltinHash),
    /// Python callable（兼容模式，跨 FFI）。
    /// 持用户传的 hashfunc（如 `lambda d: hashlib.sha512(d).digest()`）。
    PythonCallable(Py<PyAny>),
}

/// 内置哈希算法（编译期从 HashAlgo enum 编译）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinHash {
    Md5,
    Sha1,
    Sha256,
    Sha512,
    Crc32,
    Adler32,
}

/// 被哈希字节来源（双轨）。
#[derive(Debug)]
pub enum BytesSource {
    /// 原版模式：从 context 求值 bytesfunc 表达式得 bytes 对象。
    /// bytesfunc 编译为 ExprProgram，运行期求值得到字段索引，从 ctx 取 bytes。
    ContextBytes(ExprProgram),
    /// 扩展模式：从 stream 范围 [start, end) 直接切片。
    /// start/end 编译为 ExprProgram，运行期求值得到字节偏移。
    StreamRange { start: ExprProgram, end: ExprProgram },
}
```

#### 3.5.2 Construct impl（路径 B 零拷贝流程）

```rust
impl Construct for ChecksumNode {
    fn parse<'py>(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
        // 1. 读 checksum 字段（返回 PyBytes，必要返回值）
        let hash1 = self.checksumfield.parse(py, stream, ctx, path)?;
        let hash1_bound = hash1.bind(py);
        let hash1_bytes: &[u8] = hash1_bound.extract()?;  // 借用 PyBytes buffer，不拷贝

        // 2. 取被哈希字节
        let (computed_digest, digest_len): (Vec<u8>, usize) = match &self.hashfunc {
            HashFunc::BuiltIn(builtin) => {
                // 路径 B：Rust 内置 hashfunc
                let data_slice: &[u8] = match &self.bytes_source {
                    BytesSource::StreamRange { start, end } => {
                        // 零拷贝：直接切片借用
                        let s = crate::expr::eval_expr_int(start, ctx, py)? as usize;
                        let e = crate::expr::eval_expr_int(end, ctx, py)? as usize;
                        stream.slice(s, e).ok_or_else(|| ConstructError::Checksum {
                            message: format!("stream slice out of bounds: [{}, {})", s, e),
                            path: path.to_string(),
                        })?
                    }
                    BytesSource::ContextBytes(expr) => {
                        // 路径 B + ContextBytes：求值表达式得 bytes（仍需 PyBytes 借用）
                        let bytes_obj = crate::expr::eval_expr_bytes(expr, ctx, py)?;
                        bytes_obj.bind(py).extract::<&[u8]>()?
                    }
                };
                let digest = compute_builtin_hash(builtin, data_slice)?;
                (digest, digest.len())
            }
            HashFunc::PythonCallable(callable) => {
                // 路径 A：Python callable hashfunc（1 次拷贝，CPython 硬约束）
                let bytes_py: Py<PyBytes> = match &self.bytes_source {
                    BytesSource::StreamRange { start, end } => {
                        let s = crate::expr::eval_expr_int(start, ctx, py)? as usize;
                        let e = crate::expr::eval_expr_int(end, ctx, py)? as usize;
                        let slice = stream.slice(s, e).ok_or_else(|| ConstructError::Checksum {
                            message: format!("stream slice out of bounds"), path: path.to_string()
                        })?;
                        // **拷贝点**：构造新 PyBytes（CPython 硬约束，无法避免）
                        PyBytes::new_bound(py, slice).into()
                    }
                    BytesSource::ContextBytes(expr) => {
                        // bytesfunc(context) 已是 PyBytes（RawCopy 路径），直接借用
                        crate::expr::eval_expr_bytes(expr, ctx, py)?
                    }
                };
                // FFI 回调 Python hashfunc
                let result = callable.call1(py, (bytes_py,))?;
                let result_bytes: &[u8] = result.bind(py).extract()?;
                (result_bytes.to_vec(), result_bytes.len())  // 注：to_vec 仅用于跨所有权边界
            }
        };

        // 3. 比较 hash1 / computed_digest
        if hash1_bytes.len() != digest_len || hash1_bytes != computed_digest.as_slice() {
            return Err(ConstructError::Checksum {
                message: format!(
                    "wrong checksum, read {}, computed {}",
                    hex::encode(hash1_bytes),  // 注：hex 编码可选，对齐 Python binascii.hexlify
                    hex::encode(&computed_digest)
                ),
                path: path.to_string(),
            });
        }
        Ok(hash1)  // 返回 hash1（PyBytes）
    }
    fn build(&self, py, _obj, _stream, ctx, path) -> Result<(), ConstructError> {
        // build：计算 hash2 → checksumfield.build(hash2)
        // 注：build 时 checksumfield 不读主流（Python 行为：build writes hash2 to stream）
        // ...（路径 A/B 对称，详细见 DEV 实现参考 parse 结构）
        unimplemented!("DEV 实现时按 parse 对称实现")
    }
    fn sizeof(&self, ctx) -> Result<usize, ConstructError> {
        self.checksumfield.sizeof(ctx)
    }
}

/// Rust 内置哈希计算（dispatch 到对应 crate）。
fn compute_builtin_hash(algo: BuiltinHash, data: &[u8]) -> Result<Vec<u8>, ConstructError> {
    use sha2::Digest;
    Ok(match algo {
        BuiltinHash::Md5 => {
            let mut h = md5::Md5::new();
            h.update(data);
            h.finalize().to_vec()
        }
        BuiltinHash::Sha1 => {
            let mut h = sha1::Sha1::new();
            h.update(data);
            h.finalize().to_vec()
        }
        BuiltinHash::Sha256 => {
            let mut h = sha2::Sha256::new();
            h.update(data);
            h.finalize().to_vec()
        }
        BuiltinHash::Sha512 => {
            let mut h = sha2::Sha512::new();
            h.update(data);
            h.finalize().to_vec()
        }
        BuiltinHash::Crc32 => {
            // crc32fast 返回 u32，转 4 字节 big-endian（对齐 hashlib 摘要语义）
            let crc = crc32fast::hash(data);
            crc.to_be_bytes().to_vec()
        }
        BuiltinHash::Adler32 => {
            let adler = adler::adler32_slice(data);
            adler.to_be_bytes().to_vec()
        }
    })
}
```

**注**：`md5` crate 名为 `md-5`，在 Rust 中 `use md5::...` 实际是 `use md_5::...`（Cargo 自动转换）。`crate::expr::eval_expr_bytes` 是 Phase 2 表达式系统可能需扩展的 API（当前 ExprProgram 主要支持 i64；bytes 求值需新增返回 `Py<PyBytes>` 的 API，DEV 实施时确认是否已存在；若不存在，路径 B + ContextBytes 走"表达式取字段索引 → ctx.get_field_bytes"中间层）。

#### 3.5.3 路径矩阵（4 条组合路径）

| 路径 | hashfunc | bytes_source | 拷贝次数 | 适用场景 |
|------|---------|-------------|---------|---------|
| **B1（最优）** | Rust 内置 | StreamRange | **0 次** | construct-rs 新代码（推荐） |
| B2 | Rust 内置 | ContextBytes | 0 次（借用 PyBytes） | 改造现有 RawCopy 用法但保留 dict 结构 |
| A1 | Python callable | StreamRange | 1 次（构造 PyBytes 给 callable） | 用户坚持用 hashlib/zlib |
| A2（Python 原版等价） | Python callable | ContextBytes | 0 次额外（RawCopy 已构造 PyBytes） | 完全 parity 兼容 |

**CRC32/Adler32 摘要长度说明**：Python `zlib.crc32(d)` 返回 int（非 bytes），用户写 `lambda d: zlib.crc32(d).to_bytes(4, 'big')`。construct-rs 的 BuiltinHash::Crc32 直接返回 4 字节 big-endian（对齐 `to_bytes(4, 'big')` 习惯）。DEV 实现时 parity 测试需对齐 Python 用户写法（用户可能用 little-endian，建议文档标注推荐 big-endian）。

### 3.6 §0 原则对照表

| §0 原则 | ChecksumNode（路径 B1 零拷贝） | ChecksumNode（路径 A Python callable） |
|---------|------------------------------|--------------------------------------|
| #1 一次 FFI | ✅ 严格 1 次。hashfunc 全在 Rust 内（sha2/crc32fast crate）；slice 借用；expr 求值在 Rust 内 | ⚠️ **2 次**（parse 入口 + Python hashfunc 回调）。**不违反** §0 #1——同 AdapterCallbackNode 论证（用户主动选 callable，详见 ADR-022 §0.1） |
| #2 无中间表示层 | ✅ `&[u8]` 是借用引用，非中间类型 | ✅ PyBytes 是 Python 对象本身 |
| #3 输入输出无 trait 抽象 | ✅ 仅 `Box<Node>` + enum 分派 | ✅ |
| #4 pyo3 核心依赖 | ✅ sha2/crc32 是纯 Rust 内部计算库（与 `half` 同先例，§0.2 判据 2） | ✅ Python callable 持 `Py<PyAny>` |
| #5 mashumaro API | ✅ Checksum 是字段描述符 | ✅ |
| #6 enum_dispatch | ✅ 1 个新 Node 加入 enum | ✅ |
| #7 Result + path | ✅ ConstructError::Checksum 携带 path | ✅ |
| #8 Stream 纯 Rust | ✅ slice 是纯 Rust | ✅ |

### 3.7 性能假设（L-02/L-05 对策）

#### 3.7.1 瓶颈识别（量化数据 + 来源）

数据源：`质疑-能否不用RawCopy.md §7.4`（基于 sha2 crate SIMD + OpenSSL 性能基准）：

| 路径 | 哈希计算 | PyBytes 构造 | FFI 跨界 | RawCopy dict 构造 | 总计估算（1KB 数据） |
|------|---------|-------------|---------|------------------|------------------|
| Python 原版（RawCopy + callable） | ~3µs（OpenSSL C） | ~80ns（N=1024B）+ ~80ns（digest 32B） | 1 次回调（~200ns） | ~150ns（5 键 dict） | ~3.5µs |
| A2（StreamRange + callable，无 RawCopy） | ~3µs | ~80ns（N=1024B） | 1 次回调 | 0 | ~3.3µs |
| **B1（StreamRange + Rust sha256）** | ~2-3µs（sha2 crate SIMD） | **0** | **0** | 0 | **~2-3µs** |

#### 3.7.2 可证伪预测（覆盖所有 FFI/拷贝来源）

**嵌入 Struct 字段，与 Python construct 2.10.70（RawCopy + Python callable）对比**：

- **路径 B1（推荐扩展）**：加速比 ≥1.3x（vs Python RawCopy+callable）。**节省来源**：(1) 1 次 N 字节 PyBytes 拷贝；(2) 1 次 FFI 回调；(3) RawCopy 5 键 dict 构造。**风险**：若实测 <1.1x，说明 sha2 crate 与 OpenSSL 性能差距小（哈希本身已是 C 级性能），节省的边际开销被 pyo3 入口税抵消
- **路径 A2（parity 兼容）**：加速比 ≥1.05x（仅消除 RawCopy dict 开销，FFI 与 PyBytes 拷贝仍存在）

**FFI/拷贝来源清单**（L-05 对策）：
- StructMixin.parse 入口（1 次，共享）
- checksumfield.parse（Rust 内，Bytes(N) 读字节）
- expr 求值（Rust 内，~10ns × 2）
- slice 借用（Rust 内，0 拷贝）
- Rust 内置哈希（Rust 内，0 FFI）
- **路径 A 额外**：Python callable 回调（1 次 FFI）+ PyBytes 构造（1 次 N 字节拷贝）

#### 3.7.3 失败模式（可证伪）

- 若路径 B1 实测 ≤1.0x：可能 sha2 crate 性能与 OpenSSL 差距大于预期，或 pyo3 入口税（~200ns）摊薄了节省。**对策**：bench 报告需展示 hash_size × data_size 矩阵，识别 sweet spot（小数据 vs 大数据）
- 若路径 A2 实测 <1.0x（比 Python 还慢）：说明 Rust→Python 回调开销过大。**对策**：重新评估 ExprProgram 求值路径，或引导用户转路径 B

### 3.8 边界条件清单

| 编号 | 场景 | 预期行为 |
|------|------|---------|
| CS-1 | 路径 B1，hash 匹配 | 返回 hash1（PyBytes） |
| CS-2 | 路径 B1，hash 不匹配 | ConstructError::Checksum（含 read/computed hex 编码） |
| CS-3 | 路径 B1，start/end 越界 | ConstructError::Checksum（"stream slice out of bounds"） |
| CS-4 | 路径 B1，start > end | 同 CS-3（slice 返回 None） |
| CS-5 | 路径 A1（callable + StreamRange），hash 匹配 | 返回 hash1 |
| CS-6 | 路径 A2（callable + ContextBytes），bytesfunc 求值失败 | ExprFieldMissing 错误向上传播 |
| CS-7 | Python callable 抛异常 | ConstructError::Generic（含原始 traceback） |
| CS-8 | `Checksum(Bytes(64), HashAlgo.SHA512, start, end).build(...)` | 计算 SHA-512 → checksumfield.build(digest) |
| CS-9 | `Checksum(Bytes(4), HashAlgo.CRC32, start, end).build(...)` | 计算 CRC32 → 4 字节 big-endian → checksumfield.build |
| CS-10 | HashAlgo 未知值（用户传非法 enum） | 编译期拒绝（CompilationError） |
| CS-11 | checksumfield 是变长（非 Bytes(N)） | sizeof 返回 Err；build 时仍可工作 |
| CS-12 | hash1 长度与 computed_digest 长度不同（如 Bytes(4) vs SHA512 64 字节） | ConstructError::Checksum（长度不等先报错） |
| CS-13 | bytesfunc 是 lambda（非 Phase 2 表达式） | **编译期拒绝**（ADR-014，与 DF-4 同脉络） |
| CS-14 | hashfunc 既非 HashAlgo 也非 callable | **编译期拒绝**（CompilationError） |

### 3.9 DEV 实施清单

| 文件 | 内容 | 行数估 |
|------|------|-------|
| `nodes/checksum.rs` | ChecksumNode + HashFunc + BuiltinHash + BytesSource + impl + compute_builtin_hash + 单元测试 | ~600 |
| `stream.rs` | 新增 `slice` 方法（§3.3.1） | ~20 增量 |
| `nodes/mod.rs` | 新增 mod/enum 变体 + has_expressions 分支 | ~15 增量 |
| `compile.rs` | 新增 build_checksum_node 分支（识别 HashAlgo enum → BuiltinHash / callable → PythonCallable） | ~120 增量 |
| `error.rs` | 新增 ConstructError::Checksum 变体 + ExceptionClasses.checksum_error + Python 类映射 | ~30 增量 |
| `expr.rs` | 若 `eval_expr_bytes` 不存在，新增（用于 ContextBytes 路径） | ~40 增量（可选） |
| `_descriptors.py` | ChecksumDescriptor（双轨识别） | ~60 增量 |
| `_hashalgo.py`（新建） | HashAlgo Python enum + 6 个值 | ~30 |
| `_errors.py` | ChecksumError Python 类（对齐 core.py L144） | ~10 增量 |
| `__init__.py` | 导出 Checksum / HashAlgo / ChecksumError | ~15 增量 |
| **Cargo.toml** | **新增 5 个 crate**（§3.4） | ~6 增量 |

### 3.10 Parity 测试模板

```python
def test_checksum_builtin_sha256_matches():
    # 路径 B1：Rust sha256 vs Python hashlib.sha256 摘要相同
    # @dataclass class P(StructMixin):
    #     start: int = rfield(Tell())
    #     data: bytes = field(Bytes(16))
    #     end: int = rfield(Tell())
    #     checksum: bytes = rfield(Checksum(Bytes(32), HashAlgo.SHA256, start, end))
    #
    # rs 端 parse：data=16字节，checksum=sha256(data)
    # py 端对照：用 hashlib.sha256 计算期望值，硬编码到测试
    ...

def test_checksum_python_callable_compat():
    # 路径 A2：Python callable + ContextBytes
    # d = Struct("fields" / RawCopy(Bytes(16)),
    #            "checksum" / Checksum(Bytes(32), lambda d: hashlib.sha256(d).digest(), this.fields.data))
    # rs 与 py 行为一致（注：rs 端 bytesfunc 是字段名直接引用 fields.data，不是 this.fields.data）
    ...

def test_checksum_mismatch_raises():
    # 解析时 checksum 不匹配 → ChecksumError
    ...

def test_checksum_crc32_big_endian():
    # CRC32 → 4 字节 big-endian 摘要
    # 验证 rs 与 py 的 zlib.crc32(data).to_bytes(4, 'big') 一致
    ...

def test_checksum_build_computes_hash():
    # build: 计算 hash → checksumfield.build(hash)
    ...
```

**Parity 重点**：
- B1 vs A2 性能对比 bench（hash_size × data_size 矩阵，识别 sweet spot）
- CRC32/Adler32 字节序对齐（big-endian，对齐 Python `to_bytes(4, 'big')` 习惯）
- CS-12 长度不等的优先报错（vs 内容不等的报错，对齐 Python 顺序）

---

## 4. 子任务 8.8：Aligned + AlignedStruct

### 4.1 Python 参考实现摘要

#### 4.1.1 Aligned（core.py L4261-4331）

`Aligned(modulus, subcon, pattern=b"\x00")` Subconstruct：

- **parse**：`modulus = evaluate(modulus, context)`；若 <2 抛 PaddingError；`position1 = tell()`；`obj = subcon.parse()`；`position2 = tell()`；`pad = -(position2 - position1) % modulus`；`stream.read(pad)`（消费填充字节，丢弃）；返回 obj
- **build**：对称——`position1=tell()`；`subcon.build(obj)`；`position2=tell()`；`pad = -(position2 - position1) % modulus`；`stream.write(pattern * pad)`；返回 buildret
- **sizeof**：`modulus = evaluate()`；`subconlen = subcon.sizeof()`；返回 `subconlen + (-subconlen % modulus)`。SizeofError 时透传

#### 4.1.2 AlignedStruct（core.py L4334-4351）—— 宏

```python
def AlignedStruct(modulus, *subcons, **subconskw):
    subcons = list(subcons) + list(k/v for k,v in subconskw.items())
    return Struct(*[sc.name / Aligned(modulus, sc) for sc in subcons])
```

**本质**：AlignedStruct 是 Python 函数（非类），展开为 `Struct` 每字段用 `Aligned(modulus, sc)` 包装。Python 语义中"每字段对齐到 modulus"。

### 4.2 实现层决策（PM 决策 D-2 已接受）

**ARCH 推荐（已接受）**：Aligned 实现为 **Rust Node**（与 PaddingNode 同脉络，Phase 3.3 验证模式），AlignedStruct 实现为 **Python 层宏**（trivial 展开，无独立 Node）。

**理由**：
1. Aligned 是 Subconstruct（持有 inner），与 PaddingNode 同模式
2. AlignedStruct 是纯宏展开（每个字段套 Aligned），无独立语义——若实现独立 AlignedStructNode 会与 Aligned+Struct 重复代码
3. 单独实现 AlignedStruct 而无 Aligned 需在 AlignedStructNode 内联 padding，违反 DRY

### 4.3 AlignedNode Rust 设计

#### 4.3.1 数据结构

```rust
/// 对齐包装节点：inner 解析/构建后，填充字节到 modulus 的整数倍。
///
/// 对应 Python construct `Aligned(modulus, subcon, pattern=b"\\x00")`
/// （core.py L4261）。与 PaddingNode 同模式（Phase 3.3 验证）。
///
/// # padding 算法
///
/// - parse：inner.parse 后，`pad = -(tell_after - tell_before) % modulus`，
///   stream.read(pad) 消费填充字节（不验证 pattern，对齐 Python L4303）
/// - build：inner.build 后，`pad = -(tell_after - tell_before) % modulus`，
///   stream.write(pattern * pad)
/// - sizeof：inner.sizeof + (-inner.sizeof % modulus)
///
/// # modulus 约束
///
/// modulus 必须 >= 2（Python L4297/L4308 校验）。编译期常量在 build_node 时
/// 校验；运行期 ExprProgram 求值后校验（<2 抛 PaddingError）。
#[derive(Debug)]
pub struct AlignedNode {
    inner: Box<Node>,
    /// 对齐模数：常量或表达式（context lambda 不支持，编译为 ExprProgram）。
    modulus: ExprProgram,
    /// 填充字节模式（默认 b"\\x00"）。Python 限定 len==1。
    pattern: u8,
}

impl AlignedNode {
    pub fn new(inner: Node, modulus: ExprProgram, pattern: u8) -> Self {
        Self { inner: Box::new(inner), modulus, pattern }
    }
    pub fn inner(&self) -> &Node { &self.inner }
    pub fn modulus(&self) -> &ExprProgram { &self.modulus }
    pub fn pattern(&self) -> u8 { self.pattern }
}
```

**pattern 编译期校验**：Python 限定 `isinstance(pattern, bytes) and len(pattern) == 1`（L4289）。construct-rs 编译期提取 `pattern: Py<PyBytes>` → 校验长度 1 → 提取单字节为 `u8` 存入 Node。非法 pattern（多字节/非 bytes）→ CompilationError（PaddingError）。

**modulus 编译期 vs 运行期**：
- 编译期常量（用户传 `Aligned(4, ...)`）：编译为 `ExprProgram { ops: vec![PushI64(4)] }`，运行期 `eval_expr_int` ~5ns
- 表达式（用户传 `Aligned(some_field, ...)`）：编译为对应 ExprProgram

#### 4.3.2 Construct impl（关键路径摘要）

```rust
impl Construct for AlignedNode {
    fn parse<'py>(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
        let modulus = crate::expr::eval_expr_int(&self.modulus, ctx, py)?;
        if modulus < 2 {
            return Err(ConstructError::Padding {
                message: format!("expected modulo 2 or greater, got {}", modulus),
                path: path.to_string(),
            });
        }
        let pos1 = stream.tell();
        let obj = self.inner.parse(py, stream, ctx, path)?;
        let pos2 = stream.tell();
        let consumed = pos2.checked_sub(pos1).ok_or_else(|| ConstructError::Generic {
            message: format!("Aligned: pos2 {} < pos1 {}", pos2, pos1),
            path: path.to_string(),
        })?;
        let pad = (-(consumed as i64) % modulus).rem_euclid(modulus) as usize;
        // 消费填充字节（不验证 pattern，对齐 Python L4303 stream_read）
        // 注：stream.read 会校验 bit_pos==0，否则 StreamError
        let _ = stream.read(pad, path)?;
        Ok(obj)
    }
    fn build(&self, py, obj, stream, ctx, path) -> Result<(), ConstructError> {
        let modulus = crate::expr::eval_expr_int(&self.modulus, ctx, py)?;
        if modulus < 2 {
            return Err(ConstructError::Padding {
                message: format!("expected modulo 2 or greater, got {}", modulus),
                path: path.to_string(),
            });
        }
        let pos1 = stream.tell();
        self.inner.build(py, obj, stream, ctx, path)?;
        let pos2 = stream.tell();
        let consumed = pos2.checked_sub(pos1).ok_or_else(|| ConstructError::Generic {
            message: format!("Aligned: pos2 < pos1"), path: path.to_string()
        })?;
        let pad = (-(consumed as i64) % modulus).rem_euclid(modulus) as usize;
        // 写填充字节
        let pad_bytes = vec![self.pattern; pad];
        stream.write(&pad_bytes);
        Ok(())
    }
    fn sizeof(&self, ctx) -> Result<usize, ConstructError> {
        let modulus = crate::expr::eval_expr_int(&self.modulus, ctx, Python::with_gil(|py| py))?;
        if modulus < 2 {
            return Err(ConstructError::Padding {
                message: format!("expected modulo 2 or greater"), path: String::new()
            });
        }
        let inner_len = self.inner.sizeof(ctx)?;
        let pad = (-(inner_len as i64) % modulus).rem_euclid(modulus) as usize;
        Ok(inner_len + pad)
    }
}
```

**pad 算法注释**：Python 用 `pad = -(position2 - position1) % modulus`，Python 的 `%` 是模运算（结果非负）。Rust 的 `%` 是 remainder（可为负），需用 `rem_euclid` 对齐 Python 语义。`(-(x)).rem_euclid(m)` 等价于 Python `(-x) % m`。

**sizeof 内 Python GIL**：当前 sizeof 签名 `fn sizeof(&self, ctx: &Context) -> Result<usize, ConstructError>` 无 `py` 参数。若 ExprProgram::eval 需要 GIL，sizeof 路径需特殊处理——DEV 实施时确认 ExprProgram 是否有 "no GIL" eval 路径，或与 PM 协调修改 sizeof 签名（影响所有 Node，需 ADR）。

**备选方案**（不需修改 sizeof 签名）：modulus 若是编译期常量，sizeof 不需 eval（直接用常量值）；若是表达式，sizeof 返回 Err（对齐 Python SizeofError）。Python 实际行为：modulus 表达式 + sizeof 触发 `evaluate(self.modulus, context)`，context 缺字段时抛 SizeofError。construct-rs 对齐此行为即可。

**`has_expressions` 集成**：`Node::Aligned(a) => a.inner().has_expressions() || true`（恒 true，含 modulus 表达式）。

### 4.4 §0 原则对照表

| §0 原则 | AlignedNode |
|---------|-------------|
| #1 一次 FFI | ✅ 严格 1 次。inner.parse/build 在 Rust 内；ExprProgram::eval 在 Rust 内（~10ns）；stream.read/write 是 Rust 内 |
| #2 无中间表示层 | ✅ pattern 是 u8（基础类型），非中间 Python 对象；填充字节直接 stream.write |
| #3 输入输出无 trait 抽象 | ✅ 仅 `Box<Node>` |
| #4 pyo3 核心依赖 | ✅ ExprProgram + stream 都是已有抽象 |
| #5 mashumaro API | ✅ Aligned 是字段描述符 |
| #6 enum_dispatch | ✅ 1 个新 Node 加入 enum |
| #7 Result + path | ✅ PaddingError 携带 path |
| #8 Stream 纯 Rust | ✅ stream.read/write/tell 都是 Rust 内 |

### 4.5 性能假设

#### 4.5.1 瓶颈识别

| Node | 主要瓶颈 | 量化来源 |
|------|---------|---------|
| Aligned | inner.parse/build + 2 次 tell（~1ns 各）+ 1 次 ExprProgram::eval（~10ns）+ 1 次 read/write pad 字节（~5ns） | Phase 4/6 已验证；PaddingNode 同量级 |

#### 4.5.2 可证伪预测

- **Aligned(Int16ub)（modulus=4）嵌入 Struct**：加速比 ≥8x（vs Python：evaluate + stream_tell × 2 + stream_read + Python mod 算术）

**风险**：modulus 表达式路径（非常量）多 1 次 eval_expr_int 开销（~10ns）。**对策**：bench 覆盖常量 + 表达式两种 modulus。

### 4.6 边界条件清单

| 编号 | 场景 | 预期行为 |
|------|------|---------|
| AL-1 | `Aligned(4, Int16ub).parse(b'\x00\x01\x00\x00')` | 返回 1；stream 消费 4 字节（2 数据 + 2 填充） |
| AL-2 | `Aligned(4, Int16ub).build(1)` | 输出 b'\x00\x01\x00\x00'（2 数据 + 2 填充） |
| AL-3 | `Aligned(4, Int16ub).sizeof()` | 返回 4（inner_len=2, pad=2） |
| AL-4 | `Aligned(4, Bytes(3)).sizeof()` | 返回 4（inner_len=3, pad=1） |
| AL-5 | `Aligned(4, Bytes(4)).sizeof()` | 返回 4（inner_len=4, pad=0） |
| AL-6 | `Aligned(1, Int16ub)` | **编译期 PaddingError**（modulus < 2） |
| AL-7 | `Aligned(0, Int16ub)` | **编译期 PaddingError**（modulus < 2） |
| AL-8 | `Aligned(-1, Int16ub)` | **编译期 PaddingError**（modulus < 2） |
| AL-9 | `Aligned(some_field, Int16ub)` parse，some_field 求值得 1 | 运行期 PaddingError（对齐 Python L4297） |
| AL-10 | `Aligned(4, Int16ub, pattern=b'\xff').build(1)` | 输出 b'\x00\x01\xff\xff'（2 数据 + 2 填充 0xff） |
| AL-11 | `Aligned(4, Int16ub, pattern=b'')` | **编译期 PaddingError**（pattern len != 1） |
| AL-12 | `Aligned(4, Int16ub, pattern=b'\x00\x00')` | **编译期 PaddingError**（pattern len != 1） |
| AL-13 | `Aligned(4, Bytes(variable)).sizeof()` where variable 求值失败 | SizeofError（透传 inner SizeofError） |
| AL-14 | `Aligned(4, GreedyBytes).parse(b'\x01\x02\x03')` | inner 读 3 字节，consumed=3, pad=1, 读 1 字节填充 → 总消费 4 字节 |
| AL-15 | `Aligned(4, GreedyBytes).parse(b'\x01\x02\x03\x04\x05')` | inner 读全部 5 字节，consumed=5, pad=3，stream.read(3) 失败（不足）→ StreamError |
| AL-16 | `AlignedStruct(4, "a"/Int8ub, "b"/Int16ub).build(dict(a=0xFF,b=0xFFFF))` | 输出 b'\xff\x00\x00\x00\xff\xff\x00\x00'（每字段对齐 4 字节，与 Python docstring 示例一致） |

### 4.7 AlignedStruct Python 宏设计

#### 4.7.1 实现（_macros.py 新建）

```python
# construct-rs/python/construct/_macros.py（新建）

from ._descriptors import field, Aligned, StructMixin

def AlignedStruct(modulus, **subconskw):
    """对齐结构宏：每字段用 Aligned(modulus, ...) 包装。

    对应 Python construct core.py:4334 的 AlignedStruct 函数（宏展开）。
    在 construct-rs 中，用户用 dataclass 语法（而非位置 subcons）。

    使用方式：
        >>> from dataclasses import dataclass
        >>> from construct import StructMixin, AlignedStruct, Int8ub, Int16ub
        >>> AlignedPacket = AlignedStruct(4, a=Int8ub, b=Int16ub)
        >>> # 等价于：
        >>> # @dataclass
        >>> # class AlignedPacket(StructMixin):
        >>> #     a: int = field(Aligned(4, Int8ub))
        >>> #     b: int = field(Aligned(4, Int16ub))

    性能说明：宏展开为 dataclass + Aligned 节点（每字段独立 padding），
    与 Python 原版 Struct 语义一致。无独立 AlignedStructNode 变体。

    :param modulus: 整数或表达式（不支持 context lambda，ADR-014）
    :param subconskw: 字段名=描述符
    """
    from dataclasses import dataclass, make_dataclass
    from typing import Any
    fields = [(name, Any, field(Aligned(modulus, desc))) for name, desc in subconskw.items()]
    return make_dataclass("AlignedStruct", fields, bases=(StructMixin,))
```

#### 4.7.2 设计权衡

- **为什么不用 dataclass 装饰器形式**：用户已可手动写 `@dataclass class P(StructMixin): a: int = field(Aligned(4, Int8ub))`。宏提供的是便利的快捷方式
- **为什么支持 kwargs 不支持 *subcons**：construct-rs 用户面是 dataclass 语法（命名序），不支持 Python construct 的位置序。若用户传位置参数，宏报 TypeError
- **AlignedStruct 与 Struct 性能对比**：等同（每字段都是 Aligned+inner，无额外开销）

### 4.8 DEV 实施清单

| 文件 | 内容 | 行数估 |
|------|------|-------|
| `nodes/aligned.rs` | AlignedNode + impl + pad 算法 + 单元测试 | ~300 |
| `nodes/mod.rs` | 新增 mod/enum 变体 + has_expressions 分支 | ~15 增量 |
| `compile.rs` | 新增 build_aligned_node 分支（modulus + pattern 编译期校验） | ~50 增量 |
| `_descriptors.py` | AlignedDescriptor + Aligned 工厂函数 | ~30 增量 |
| `_macros.py`（新建） | AlignedStruct 宏（make_dataclass + Aligned 包装） | ~60 |
| `__init__.py` | 导出 Aligned / AlignedStruct | ~10 增量 |

**Cargo.toml 不需新增依赖**（ExprProgram + stream 已有）。

### 4.9 Parity 测试模板

```python
def test_aligned_parse_consumes_padding():
    # Aligned(4, Int16ub).parse(b'\x00\x01\x00\x00') → 1
    # stream 全消费（4 字节）
    assert_parity_case("""
    from construct import Aligned, Int16ub
    parsed = Aligned(4, Int16ub).parse(b'\\x00\\x01\\x00\\x00')
    """, expected_py=1)

def test_aligned_build_writes_padding():
    # Aligned(4, Int16ub).build(1) → b'\x00\x01\x00\x00'
    ...

def test_aligned_struct_macro():
    # AlignedStruct(4, a=Int8ub, b=Int16ub).build(dict(a=0xFF, b=0xFFFF))
    # → b'\xff\x00\x00\x00\xff\xff\x00\x00'（Python docstring 示例）
    ...

def test_aligned_modulus_lt_2_padding_error():
    # Aligned(1, Int16ub) → 编译期 PaddingError
    ...

def test_aligned_pattern_multibyte_error():
    # Aligned(4, Int16ub, pattern=b'\x00\x00') → 编译期 PaddingError
    ...

def test_aligned_with_expression_modulus():
    # @dataclass class P(StructMixin):
    #     width: int = rfield(Byte)
    #     data: int = rfield(Aligned(width, Byte))  # modulus 是字段引用
    ...

def test_aligned_greedy_bytes_insufficient_padding():
    # AL-15：inner 读 5 字节后 pad=3，stream 不足 → StreamError
    ...
```

---

## 5. 子任务 8.9：Terminated / Probe

### 5.1 Python 参考实现摘要

#### 5.1.1 Terminated（core.py L4727-4755）

`Terminated`（singleton，flagbuildnone=True）：

- **parse**：`if stream.read(1): raise TerminatedError("expected end of stream")`。注意 Python 实现是 read(1)，若 EOF（stream.read 返回 b""）则不抛错
- **build**：return obj（no-op）
- **sizeof**：raise SizeofError

#### 5.1.2 Probe（debug.py L6-95）

`Probe(into=None, lookahead=None)`（flagbuildnone=True）：

- **parse/build/sizeof** 都调 `printout(stream, context, path)`。printout 输出：分隔线 + path + into repr + （lookahead 时）stream peek（tell + read(lookahead) + seek 回）+ （context is not None 时）context dump（into 有时调 into(context)，无时 print(context)）+ 分隔线
- **into**：None 或 context lambda（求值后 print）
- **lookahead**：None 或 int（peek 字节数）

### 5.2 Rust Node 设计

#### 5.2.1 TerminatedNode（与 PassNode 同量级）

```rust
/// EOF 断言节点：parse 时若 stream 未到 EOF 抛 TerminatedError。
///
/// 对应 Python construct `Terminated`（core.py L4727）。Python 用
/// `stream.read(1)` 判断（EOF 时返回 b""）；construct-rs 直接用
/// `stream.remaining() > 0` 判断（更直接，不消费字节）。
///
/// # 三方法行为
///
/// - parse：`stream.remaining() > 0` → TerminatedError；否则返回 Py_None
/// - build：no-op
/// - sizeof：返回 Err（SizeofError 等价）
#[derive(Debug, Default)]
pub struct TerminatedNode;

impl TerminatedNode {
    pub fn new() -> Self { Self }
}
```

**关键决策**：Python 用 `stream.read(1)` 判断 EOF——若 read 成功（返回 b"x"）则抛错，EOF（返回 b""）则通过。construct-rs 用 `stream.remaining() > 0` 直接判断，**不消费字节**——但语义等价（Python 的 read(1) 在抛错时也已"消费"了 1 字节，但抛错路径下 stream 状态不重要）。

#### 5.2.2 ProbeNode

```rust
/// 调试探针节点：parse/build 时打印 path + 可选 stream peek + 可选 context dump。
///
/// 对应 Python construct `Probe(into=None, lookahead=None)`（debug.py L6）。
///
/// # 三方法行为
///
/// - parse/build：调用内部 `printout` 函数（输出到 Python `print`，与 Python
///   原版保持缓冲一致），返回 Py_None（parse）/ no-op（build）
/// - sizeof：调用 printout 后返回 0
///
/// # 表达式约束
///
/// `into` 是 Python context lambda；construct-rs 强制编译为 ExprProgram
/// （PM 决策 D-6 已接受，不支持 callable）。`lookahead` 是编译期 usize 常量。
#[derive(Debug)]
pub struct ProbeNode {
    /// 可选表达式：求值后 repr 打印。None 表示打印整个 context。
    into: Option<ExprProgram>,
    /// 可选 peek 字节数：None 表示不 peek。
    lookahead: Option<usize>,
}

impl ProbeNode {
    pub fn new(into: Option<ExprProgram>, lookahead: Option<usize>) -> Self {
        Self { into, lookahead }
    }
    pub fn into(&self) -> Option<&ExprProgram> { self.into.as_ref() }
    pub fn lookahead(&self) -> Option<usize> { self.lookahead }
}
```

**`printout` 实现要点**（与 Python 原版行为对齐）：
1. 调 Python `builtins.print`（保持 stdout 缓冲一致，对齐 Python 原版）
2. 输出顺序：分隔线 → "Probe, path is X, into is Y" → 可选 "Stream peek: (hexlified) ..." → 可选 context dump → 分隔线
3. into 是 ExprProgram 时求值后 print（`print(value)`）；None 时 print 整个 context（`print(ctx_dict)`）

**输出对齐说明**：Python `print(context)` 输出 `Container(a=1, b=2)`（Container `__repr__`）。construct-rs context 是 PyDict，`print(dict)` 输出 `{'a': 1, 'b': 2}`。**已知 repr 差异**（dict vs Container 格式），但 `print` 的语义对齐（都是输出 context 内容）。Parity 测试不断言 repr 字符串完全一致，仅断言含相同键值。

#### 5.2.3 Construct impl（TerminatedNode 摘要）

```rust
impl Construct for TerminatedNode {
    fn parse<'py>(&self, py, stream, _ctx, path) -> Result<Py<PyAny>, ConstructError> {
        if stream.remaining() > 0 {
            return Err(ConstructError::Terminated {
                message: "expected end of stream".to_string(),
                path: path.to_string(),
            });
        }
        Ok(py.None())
    }
    fn build(&self, _py, _obj, _stream, _ctx, _path) -> Result<(), ConstructError> {
        Ok(())  // no-op（对齐 Python `return obj`）
    }
    fn sizeof(&self, _ctx) -> Result<usize, ConstructError> {
        Err(ConstructError::Generic {
            message: "size of Terminated is undefined".to_string(),
            path: String::new(),
        })
    }
}

impl Construct for ProbeNode {
    fn parse<'py>(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
        self.printout(py, Some(stream), ctx, path)?;
        Ok(py.None())
    }
    fn build(&self, py, _obj, _stream, ctx, path) -> Result<(), ConstructError> {
        self.printout(py, None, ctx, path)?;
        Ok(())
    }
    fn sizeof(&self, py, ctx, path) -> Result<usize, ConstructError> {
        self.printout(py, None, ctx, path)?;
        Ok(0)
    }
}
```

**`has_expressions` 集成**：
- `Node::Terminated(_) => false`
- `Node::Probe(p) => p.into().is_some()`（含表达式时为 true）

**ParseStream::remaining**：stream.rs 已有（pass.rs 测试 L166 `stream.remaining()` 引用），无需新增 API。

### 5.3 §0 原则对照表

| §0 原则 | TerminatedNode | ProbeNode |
|---------|----------------|-----------|
| #1 一次 FFI | ✅ stream.remaining 是 Rust 内 | ✅ printout 调 Python print 是副作用（非数据流 FFI）；ctx.as_py_dict 是借用 |
| #2 无中间表示层 | ✅ 无中间数据 | ✅ 字符串构造仅用于打印输出 |
| #3 输入输出无 trait 抽象 | ✅ 单元结构体 | ✅ |
| #4 pyo3 核心依赖 | ✅ | ✅ print / dict 都是 pyo3 API |
| #5 mashumaro API | ✅ | ✅ |
| #6 enum_dispatch | ✅ 2 个新 Node | ✅ |
| #7 Result + path | ✅ TerminatedError 携带 path | ✅ |
| #8 Stream 纯 Rust | ✅ remaining 不消费字节 | ✅ stream.peek 内部用 read+seek |

**ProbeNode #1 注意**：printout 调用 Python `print` 是跨 FFI（每次 print 1 次 FFI），但这是**调试副作用的固有开销**（不是数据流 FFI）。Python 原版同样调 print。Probe 主要用于调试，性能不设硬门禁。

### 5.4 性能假设

| Node | 性能目标 |
|------|---------|
| Terminated | ≥10x（与 PassNode 同量级，纯 Rust 内 stream.remaining 判断） |
| Probe | 不设硬门禁（调试构造器，print 是固有开销，与 Python 原版同档） |

### 5.5 边界条件清单

| 编号 | 场景 | 预期行为 |
|------|------|---------|
| TM-1 | `Terminated.parse(b"")` | 返回 None（EOF） |
| TM-2 | `Terminated.parse(b"remaining")` | TerminatedError（"expected end of stream"） |
| TM-3 | `Terminated.build(None)` | no-op |
| TM-4 | `Terminated.build(42)` | no-op（忽略 obj） |
| TM-5 | `Terminated.sizeof()` | Err（SizeofError 等价） |
| TM-6 | Terminated 嵌入 Struct 末尾，stream 有剩余 | TerminatedError |
| TM-7 | Terminated 嵌入 Struct 末尾，stream 已 EOF | 通过，返回 None |
| PB-1 | `Probe()` parse/build | 输出分隔线 + path + context dump |
| PB-2 | `Probe(lookahead=32)` parse | 多输出一行 "Stream peek: (hexlified) ..." |
| PB-3 | `Probe(some_expr)` parse | 多输出一行（表达式求值结果 repr） |
| PB-4 | `Probe()` parse stream 已 EOF，lookahead=32 | 输出 "Stream peek: EOF reached" |
| PB-5 | `Probe(lambda ctx: ...)` | **编译期拒绝**（DF-4 同脉络，PM 决策 D-6） |
| PB-6 | `Probe(lookahead=0)` | lookahead==0 时按 Python 行为：read(0) 返回空，"Stream peek: EOF reached"（边界） |
| PB-7 | `Probe()` sizeof | 输出 printout 后返回 0 |
| PB-8 | Probe 嵌入 Sequence（无字段名） | 行为同嵌入 Struct（输出 path + context） |

### 5.6 DEV 实施清单

| 文件 | 内容 | 行数估 |
|------|------|-------|
| `nodes/terminated.rs` | TerminatedNode + impl + 单元测试 | ~120 |
| `nodes/probe.rs` | ProbeNode + impl + printout + 单元测试 | ~250 |
| `nodes/mod.rs` | 新增 2 个 mod/enum 变体 + has_expressions 分支 | ~15 增量 |
| `compile.rs` | 新增 2 个 build_*_node 分支 | ~30 增量 |
| `error.rs` | 新增 ConstructError::Terminated 变体 + ExceptionClasses.terminated_error + Python 类映射 | ~30 增量 |
| `expr.rs` | 若 `eval_expr_any` 不存在，新增（用于 Probe.into） | ~30 增量（可选） |
| `_descriptors.py` | TerminatedDescriptor（singleton）/ ProbeDescriptor + 工厂函数 | ~40 增量 |
| `_errors.py` | TerminatedError Python 类（对齐 core.py L129） | ~10 增量 |
| `__init__.py` | 导出 Terminated / Probe / TerminatedError | ~10 增量 |

**Cargo.toml 不需新增依赖**。

### 5.7 Parity 测试模板

```python
def test_terminated_parse_at_eof_returns_none():
    # Terminated.parse(b"") → None
    assert_parity_case("""
    from construct import Terminated
    parsed = Terminated.parse(b'')
    """, expected_py=None)

def test_terminated_parse_not_at_eof_raises():
    # Terminated.parse(b"remaining") → TerminatedError
    ...

def test_probe_basic_outputs_to_stdout(capsys):
    # Probe() parse → 输出包含 "Probe, path is" + 分隔线
    # 测试捕获 stdout，断言含关键子串
    ...

def test_probe_with_lookahead(capsys):
    # Probe(lookahead=4).parse(b'\x00\x01\x02\x03') → 输出含 "Stream peek"
    ...
```

---

## 6. 子任务 8.10：CancelParsing

### 6.1 Python 参考实现摘要

#### 6.1.1 CancelParsing 异常（core.py L149-153）

```python
class CancelParsing(ConstructError):
    """
    This exception can only be raise explicitly by the user, and it causes
    the parsing class to stop what it is doing (interrupts parsing or building).
    """
    pass
```

#### 6.1.2 顶层 catch（core.py L416-419）

```python
def parse_stream(self, stream, **contextkw):
    context = Container(**contextkw)
    context._parsing = True
    # ... context setup ...
    try:
        return self._parsereport(stream, context, "(parsing)")
    except CancelParsing:
        pass   # parse 提前终止，返回 None
```

**与 ExplicitError 的关系（详见 8.0 分析报告 §2.4.1）**：
- CancelParsing：**顶层 catch**（parse_stream 入口），中止整个 parse，返回 None。用户主动抛
- ExplicitError（Phase 7 已实现）：**字段级穿透**，不被 Peek/Select 吞掉，但正常向上传播到 parse 入口（转 ConstructError）

### 6.2 Rust 设计：Error 变体 + schema.rs 顶层 catch

#### 6.2.1 ConstructError::CancelParsing 变体

```rust
// error.rs 新增变体

/// 用户主动取消解析（对应 Python construct `CancelParsing`，core.py L149）。
///
/// **仅由 Python 用户代码触发**——用户从 Adapter `_decode` / 自定义 Python
/// 代码 `raise CancelParsing()`，跨 FFI 转为 ConstructError::CancelParsing。
/// schema.rs parse 入口捕获后返回 Py_None（对齐 Python L418 `except: pass`）。
///
/// # 触发场景限制（PM 决策 D-7）
///
/// construct-rs 表达式系统不接 lambda（ADR-006），用户无法在 Computed 表达式
/// 中 raise CancelParsing。**仅 Python 层用户代码可触发**：
/// - 用户继承 Adapter 写 `_decode` 内 raise CancelParsing
/// - 用户在 StructMixin 子类的 `__post_init__` 内 raise
///
/// # 与 ExplicitError 的区别
///
/// CancelParsing 被顶层 catch（schema.rs），parse 返回 None；
/// ExplicitError 不被顶层 catch，正常向上传播为 ConstructError。
#[error("cancel parsing at {path}")]
CancelParsing {
    /// 错误发生的路径。
    path: String,
},
```

**ExceptionClasses 新增字段**：`cancel_parsing_error: Py<PyType>`（从 `construct._errors.CancelParsing` 加载）。

**注意**：CancelParsing 是 Python 的 `ConstructError` 子类（用户 `except CancelParsing` 可捕获）。Rust 端 `From<PyErr> for ConstructError` 当前统一转 `Generic`（error.rs L1004）。**CancelParsing 不能走 Generic 路径**——需要特殊的 PyErr 类型识别。

#### 6.2.2 From<PyErr> 路径识别（关键设计）

**问题**：用户在 Python 层 `raise CancelParsing()`，跨 FFI 后 pyo3 把 PyErr 传给 Rust。Rust 端如何识别这是 CancelParsing 而非其他异常？

**方案 A（ARCH 推荐）**：在 `From<PyErr> for ConstructError` 实现中加类型检查分支：

```rust
impl From<PyErr> for ConstructError {
    fn from(e: PyErr) -> Self {
        Python::with_gil(|py| {
            // 检查是否是 CancelParsing 类型（指针相等，与 ExceptionClasses::is_builtin_class 同模式）
            if let Some(classes) = EXCEPTIONS.get(py) {
                let err_type = e.ptype(py);
                let err_type_ptr = err_type.as_ptr();
                if err_type_ptr == classes.cancel_parsing_error.as_ptr() {
                    // 提取 path（若异常实例携带）
                    let path = e.instance(py).getattr("path").ok()
                        .and_then(|p| p.extract::<String>().ok())
                        .unwrap_or_default();
                    return ConstructError::CancelParsing { path };
                }
            }
            // 默认 fallback（原逻辑）
            ConstructError::Generic {
                message: e.to_string(),
                path: String::new(),
            }
        })
    }
}
```

**方案 B（不推荐）**：扩展 PyErr 类型到 ConstructError 的多分支匹配——增加复杂性，且仅 CancelParsing 需要。

**ARCH 选方案 A**：仅加 1 个分支检查，最小侵入。代价：每次 PyErr→ConstructError 转换多 ~5ns（指针比较 + 可能的 EXCEPTIONS.get 查询），但错误路径不设硬门禁，开销可接受。

#### 6.2.3 schema.rs parse 入口 catch

```rust
// schema.rs _parse_raw 修改

#[pyo3(signature = (data))]
pub fn _parse_raw<'py>(
    &self,
    py: Python<'py>,
    data: &Bound<'py, PyBytes>,
) -> PyResult<Bound<'py, PyAny>> {
    let bytes = data.as_bytes();
    let mut stream = ParseStream::new(bytes);
    let mut ctx = Context::placeholder(py);
    let mut path = Path::new();
    // 关键修改：捕获 CancelParsing（对齐 Python core.py L416-419）
    match self.root.parse(py, &mut stream, &mut ctx, &mut path) {
        Ok(result) => Ok(result.into_bound(py)),
        Err(ConstructError::CancelParsing { .. }) => {
            // 用户主动取消，返回 None（对齐 Python `except CancelParsing: pass`）
            Ok(py.None().into_bound(py))
        }
        Err(e) => Err(e.into()),  // 其他错误正常转 PyErr
    }
}
```

**注意事项**：
- CancelParsing 不在 build 路径捕获（Python `build_stream` 无 try/except）
- CancelParsing 是字段级错误传播到根，路径重建（`push_path_segment`）正常工作
- CancelParsing 不应被 Peek/Select 吞掉（与 ExplicitError 同语义）——但当前 Peek 吞所有非 Explicit 错误，**CancelParsing 会被 Peek 吞掉**。**已知差异**（CP-3），需在 §6.5 边界明确（与 Python 行为对齐——Python Peek 也吞 ConstructError 子类）

### 6.3 §0 原则对照表

| §0 原则 | CancelParsing |
|---------|---------------|
| #1 一次 FFI | ✅ CancelParsing 传播是 Rust 内；顶层 catch 在 schema.rs 入口；用户 raise 是 1 次额外 FFI（用户主动触发，不算构造器开销） |
| #2 无中间表示层 | ✅ 无中间数据 |
| #3 输入输出无 trait 抽象 | ✅ |
| #4 pyo3 核心依赖 | ✅ PyErr 类型识别走 pyo3 C API |
| #5 mashumaro API | ✅ |
| #6 enum_dispatch | ✅（Error 变体，非 Node 变体） |
| #7 Result + path | ✅ CancelParsing 携带 path |
| #8 Stream 纯 Rust | ✅ |

### 6.4 性能假设

CancelParsing 是错误路径（用户主动中断），性能不设硬门禁。开销仅来自：
- PyErr 类型识别（指针比较，~5ns）
- path 提取（PyObject_GetAttr，~20ns）

### 6.5 边界条件清单

| 编号 | 场景 | 预期行为 |
|------|------|---------|
| CP-1 | 用户在 Adapter `_decode` 内 `raise CancelParsing()` | parse 入口捕获，返回 None |
| CP-2 | 用户在 `__post_init__` 内 `raise CancelParsing()` | 同 CP-1（StructNode 内部跨 FFI 传播） |
| CP-3 | CancelParsing 在 Peek 内层抛出 | **被 Peek 吞掉返回 None**（已知行为，Peek 不区分 CancelParsing）。Python 同样行为：Peek 用 `except ConstructError`，CancelParsing 是 ConstructError 子类，也被吞 |
| CP-4 | CancelParsing 在 Select 内层抛出 | **被 Select 当作 subcon 失败继续尝试下一个**（Select 当前吞所有非 Explicit 错误）。Python 同样行为 |
| CP-5 | CancelParsing 在 build 路径抛出 | 正常向上传播为 ConstructError（build 入口不 catch） |
| CP-6 | CancelParsing 携带 path | 顶层 catch 后丢弃 path（对齐 Python `pass`，不输出） |
| CP-7 | 用户表达式内 `raise CancelParsing` | 不支持（ADR-014，表达式不接 lambda） |
| CP-8 | Python 用户 `except CancelParsing` 是否能捕获 | **不能**（rs 端捕获后返回 None，Python 侧无异常抛出）。与 Python 原版行为对齐（Python 也是 except 后 pass） |

### 6.6 DEV 实施清单

| 文件 | 内容 | 行数估 |
|------|------|-------|
| `error.rs` | 新增 ConstructError::CancelParsing 变体 + ExceptionClasses.cancel_parsing_error + From<PyErr> 分支识别 + select_exception_class 分支 + message/path/kind_str 分支 | ~80 增量 |
| `schema.rs` | _parse_raw 入口加 CancelParsing catch 分支（~5 行） | ~10 增量 |
| `lib.rs`（模块初始化） | init_exception_classes 加载 CancelParsing 类（~3 行） | ~5 增量 |
| `_errors.py` | CancelParsing Python 类（对齐 core.py L149） | ~10 增量 |
| `__init__.py` | 导出 CancelParsing | ~5 增量 |

**Cargo.toml 不需新增依赖**。

### 6.7 Parity 测试模板

```python
def test_cancel_parsing_in_user_adapter():
    # 用户继承 Adapter，_decode 内 raise CancelParsing
    # class MyAdapter(Adapter):
    #     def _decode(self, obj, ctx, path):
    #         raise CancelParsing()
    # @dataclass class P(StructMixin):
    #     value: int = field(MyAdapter(Int8ub))
    # P.parse(b'\x01') → None（顶层 catch）
    assert_parity_case("""
    from construct import Adapter, CancelParsing

    class MyAdapter(Adapter):
        def _decode(self, obj, ctx, path):
            raise CancelParsing()

    # rs 与 py 都返回 None
    """)

def test_cancel_parsing_in_post_init():
    # @dataclass class P(StructMixin):
    #     value: int = rfield(Int8ub)
    #     def __post_init__(self):
    #         raise CancelParsing()
    # P.parse(b'\x01') → None
    ...

def test_cancel_parsing_propagates_path():
    # CancelParsing 在嵌套 Struct 字段抛出，path 应重建
    # （顶层 catch 后丢弃，但内部错误日志可观察）
    ...

def test_cancel_parsing_swallowed_by_peek():
    # CP-3：Peek 内层抛 CancelParsing → Peek 吞掉返回 None
    # Python 原版同样行为（Peek 用 except ConstructError）
    ...
```

**Parity 重点**：
- CP-1/CP-2 用户主动触发的 2 个路径
- CP-3/CP-4 Peek/Select 吞掉的已知行为（与 Python 行为对齐）

---

## 7. 总体 Node enum 扩展汇总

P0 批次新增 **9 个 Node 变体**（不含 CancelParsing，它是 Error 变体）：

```rust
pub enum Node {
    // ... 现有 39 个变体 ...

    // === Phase 8 P0 批次（9 个新变体） ===
    /// 8.1：常量字段节点（对应 Python Const）。
    Const(ConstNode),
    /// 8.1：默认值字段节点（对应 Python Default）。
    Default(DefaultNode),
    /// 8.1：断言检查节点（对应 Python Check）。必须作为 RO 字段使用。
    Check(CheckNode),
    /// 8.4：Hex 显示包装节点（对应 Python Hex，Rust Node 非 AdapterCallback）。
    Hex(HexNode),
    /// 8.4：HexDump 显示包装节点（对应 Python HexDump）。
    HexDump(HexDumpNode),
    /// 8.5：校验和节点（对应 Python Checksum，双轨 hashfunc + StreamRange）。
    Checksum(ChecksumNode),
    /// 8.8：对齐包装节点（对应 Python Aligned）。
    Aligned(AlignedNode),
    /// 8.9：EOF 断言节点（对应 Python Terminated）。
    Terminated(TerminatedNode),
    /// 8.9：调试探针节点（对应 Python Probe）。
    Probe(ProbeNode),
}
```

**mod.rs 同步修改清单**：
- `pub mod const_node;` `pub mod default_node;` `pub mod check_node;` `pub mod hex;` `pub mod hex_dump;` `pub mod checksum;` `pub mod aligned;` `pub mod terminated;` `pub mod probe;`（9 个新模块声明，注：`const_node.rs` 避开 Rust 关键字 `const`）
- `use` 9 个新 Node 类型
- `Node::has_expressions()` 加 9 个分支（详见各节 §0 对照表）
- `Node::compute_ro_value` 新增 `Check` 分支（返回 Py_None）；其他 8 个新 Node 不作为 RO 字段（落入 `_ => Err` 兜底）

**Node enum 净增**：39 → 48（Phase 8 P0 后）；P1+ 批次预计再增 9 变体（Enum/FlagsEnum/Mapping/OneOf/NoneOf/Union/Sequence/NamedTuple/ProcessXor/ProcessRotateLeft），最终 57 变体（与 8.0 分析报告一致）。

## 8. Cargo.toml 新增依赖确认

### 8.1 仅 8.5 Checksum 需新增（5 个 crate）

```toml
# 在 [dependencies] 段追加（与 half = "2.4" 同位置）：

# Phase 8.5：Checksum Rust 内置 hashfunc（L-14 教训触发，零拷贝路径）。
# 纯 Rust 计算库（不跨 FFI），与 §0 #4（pyo3 核心依赖）不冲突——`half` 已有先例。
sha2 = "0.10"      # SHA-256/512
sha1 = "0.10"      # SHA-1
md-5 = "0.10"      # MD5（crate 名 md-5，Rust 中 use md_5）
crc32fast = "1.4"  # CRC32（zlib.crc32 等价）
adler = "1.0"      # Adler32（zlib.adler32 等价）
```

### 8.2 P0 其他子任务依赖清单（全部已有，无需新增）

| 子任务 | 依赖 | 状态 |
|--------|------|------|
| 8.1 Const/Default/Check | ExprProgram / pyo3 rich_compare / ConstructError | ✅ 已有 |
| 8.4 Hex/HexDump | pyo3 is_instance_of / call_method1 | ✅ 已有 |
| 8.5 ParseStream::slice | stream.rs 现有 data() | ✅ 已有（封装 ~10 行） |
| 8.8 Aligned/AlignedStruct | ExprProgram / stream.read/write | ✅ 已有 |
| 8.9 Terminated/Probe | stream.remaining / Python print | ✅ 已有 |
| 8.10 CancelParsing | ConstructError / schema.rs / PyErr | ✅ 已有 |

### 8.3 §0 #4 合规总结

- pyo3 仍是 FFI 唯一桥梁（5 个 hashfunc crate 均纯 Rust 内部计算，不跨 FFI）
- `half = "2.4"`（Phase 6.1）已有先例——纯 Rust 计算库不冲突 §0 #4
- L-14 教训落实：交叉验证"必须拷贝"硬约束，发现 Rust 内置 hashfunc 零拷贝路径

## 9. ConstructError 新增变体汇总

P0 批次新增 **5 个 Error 变体**：

| 变体 | 子任务 | Python 异常类 | 备注 |
|------|--------|--------------|------|
| `Const { message, path }` | 8.1 | `ConstError`（core.py L74） | 编译期 + 运行期触发 |
| `Check { message, path }` | 8.1 | `CheckError`（core.py L84） | 运行期触发 |
| `Checksum { message, path }` | 8.5 | `ChecksumError`（core.py L144） | 运行期触发 |
| `Terminated { message, path }` | 8.9 | `TerminatedError`（core.py L129） | 运行期触发 |
| `CancelParsing { path }` | 8.10 | `CancelParsing`（core.py L149） | 用户主动触发，schema.rs 顶层 catch |

**ExceptionClasses 字段同步扩展**：从 16 → 21（新增 const_error / check_error / checksum_error / terminated_error / cancel_parsing_error）。

**is_builtin_class 数组扩展**：16 → 21（与 ExceptionClasses 字段同步）。

## 10. 总体 PM 决策点（P0 批次内 ARCH 推荐）

### D-P0-1：ChecksumNode 是否需扩展 `eval_expr_bytes` API

**问题**：Checksum 路径 A2（Python callable + ContextBytes）需从 context 求值 bytesfunc 表达式得 `Py<PyBytes>`。当前 ExprProgram 主要支持 `eval_expr_int`（i64）。是否新增 `eval_expr_bytes`？

**ARCH 推荐**：**P0 范围内可暂缓**。理由：
- 路径 B1（推荐）用 StreamRange，不需要 eval_expr_bytes
- 路径 A2 是兼容路径，DEV 实施时若评估 ExprProgram 已能表达"取字段值"（GetInt + 字段索引），可走"取字段 PyObject → 提取 bytes"中间层
- 若必须新增，作为 8.5 子任务的内部基础设施（不算外部 API 变更）

**若 PM 决定 P0 内必须支持 A2 全 parity**：DEV 实施时同步扩展 `eval_expr_bytes`（~40 行 Rust），与本设计文档不冲突。

### D-P0-2：ProbeNode 是否需扩展 `eval_expr_any` API

**问题**：Probe.into 表达式求值结果可能是任意 Python 对象（非 i64），需 `eval_expr_any -> Py<PyAny>`。

**ARCH 推荐**：**P0 内推荐扩展**。理由：
- Probe.into 是用户调试常用场景（"查看某字段值"）
- 当前 ExprProgram 仅 i64 严重限制 Probe 可用性
- 扩展成本低（~30 行，ExprProgram 已有 GetInt 指令，扩展为 GetField 返回 PyObject 即可）

**若 PM 决定 P0 内不扩展**：Probe.into 暂仅支持 i64 求值（用户调试 int 字段可用，其他类型需等 P1+）。

### D-P0-3：Aligned sizeof 路径 modulus 表达式的 GIL 问题

**问题**：当前 `sizeof(&self, ctx) -> Result<usize, ConstructError>` 签名无 `py: Python` 参数。若 modulus 是表达式，eval 需 GIL。

**ARCH 推荐**：**对齐 Python 行为——modulus 表达式 + sizeof 触发 SizeofError**。理由：
- Python 同样行为（modulus 不可求值时 SizeofError）
- 避免修改 sizeof 签名（影响所有 Node，需 ADR）
- DEV 实施时 Aligned::sizeof 检测 modulus 是否编译期常量；非常量直接返回 Err

**若 PM 决定需精确支持**：需新增 ADR 修改 Construct::sizeof 签名（加 py 参数），影响面广，**ARCH 不推荐 P0 内推进**。

### D-P0-4：AlignedStruct 宏是否支持位置参数（`*subcons`）

**问题**：Python `AlignedStruct(modulus, *subcons, **subconskw)` 同时支持位置 + 关键字。construct-rs dataclass 语法是命名序，宏设计（§4.7.1）仅支持 `**subconskw`。

**ARCH 推荐**：**仅支持 `**subconskw`**。理由：
- construct-rs 用户面是 dataclass 语法（命名序），不支持位置序
- 用户传位置参数宏报 TypeError（清晰错误）
- parity 已知差异（用户从 Python 迁移需调整写法）

### D-P0-5：8.5 子任务规模较大（~700 行 Rust + 基础设施），是否拆分

**问题**：8.5 单子任务含 ChecksumNode（~600 行）+ ParseStream::slice（~20 行）+ Cargo.toml 5 crate + Python HashAlgo enum + 4 路径测试。DEV 工作量较大。

**ARCH 推荐**：**不拆分**。理由：
- 5 crate + slice 是 Checksum 的必要前置基础设施，单独拆出无意义
- 4 路径共用 ChecksumNode 数据结构，拆分会重复
- DEV 可分批实施（先路径 B1 零拷贝，再加 A2 兼容），但设计文档统一

**若 PM 决定拆分**：8.5a（基础设施：slice + Cargo.toml + HashAlgo）+ 8.5b（ChecksumNode）。ARCH 可接受。

## 11. 与总设计文档 / 已有 ADR 的关系

### 11.1 不修改 `docs/design/基础设施/架构设计.md`

- Construct trait 签名不变（parse/build/sizeof 三方法）
- Node enum 扩展在 nodes/mod.rs 内部
- compile_schema 签名不变

### 11.2 不修改现有 ADR-001 ~ ADR-022

所有 P0 批次设计遵循已有 ADR：
- ADR-004（三种 field 函数 RW/RO/WO）：CheckNode 是 RO 字段
- ADR-006（表达式 VM 仅 i64）：所有"动态值"位置（Default.value / Check.func / Aligned.modulus / Checksum.StreamRange.start/end / Probe.into）走 ExprProgram
- ADR-012（StopField 用 Result 哨兵）：CancelParsing 复用 Result 模式（Error 变体 + 顶层 catch）
- ADR-014（RepeatUntil 终止表达式 v5 删 PyCallable）：所有内置 Adapter Node 不接收 callable（用户面 Adapter 才接 callable，走 AdapterCallbackNode）
- ADR-022（用户面 Adapter Python 层化）：Hex/HexDump 走 Rust Node 是 ADR-022 §0.2 论证的工程化落地

### 11.3 建议沉淀的 ADR

**P0 批次完成后，建议沉淀以下决策到 ADR**：

| 建议 ADR | 内容 | 触发子任务 |
|---------|------|-----------|
| ADR-023 | Checksum 双轨方案（Rust 内置 hashfunc + StreamRange 零拷贝） | 8.5（L-14 教训工程化） |
| ADR-024 | Hex/HexDump Rust Node 设计（非 AdapterCallbackNode） | 8.4（ADR-022 §0.2 落地） |
| ADR-025 | CancelParsing Error 变体 + 顶层 catch 模式 | 8.10 |

PM 在 ACCEPTED 阶段决定是否沉淀。ARCH 推荐：8.5 必沉淀（L-14 教训重要里程碑），8.4/8.10 可选（与 ADR-022 同脉络，可合并引用）。

## 12. 附录

### 附录 A：Python 源码行号速查（P0 批次）

| 构造器 | Python 文件 | 行号 |
|--------|------------|------|
| Const | core.py | L2808-2876 |
| Default | core.py | L3030-3078 |
| Check | core.py | L3081-3129 |
| Hex | core.py | L3523-3580 |
| HexDump | core.py | L3583-3635 |
| Checksum | core.py | L5532-5600 |
| Aligned | core.py | L4261-4331 |
| AlignedStruct | core.py | L4334-4351 |
| Terminated | core.py | L4727-4755 |
| Probe | debug.py | L6-95 |
| CancelParsing 异常 | core.py | L149-153 |
| CancelParsing 顶层 catch | core.py | L416-419 |
| ConstError | core.py | L74 |
| CheckError | core.py | L84 |
| TerminatedError | core.py | L129 |
| ChecksumError | core.py | L144 |
| HexDisplayedInteger/Bytes/Dict | lib/hex.py | L5-28 |
| HexDumpDisplayedBytes/Dict | lib/hex.py | L30-42 |

### 附录 B：参考文件索引

| 文件 | 用途 |
|------|------|
| `docs/analysis/分析报告-Phase8启动前置.md` | ARCH 8.0 分析报告（P0/P1/P2 拆分 + 7 PM 决策） |
| `docs/design/queries/质疑-能否不用RawCopy.md §7-§10` | Checksum Rust hashfunc 零拷贝方案（L-14 教训触发，§3 基线） |
| `docs/design/模块设计/模块设计-Adapter核心.md` | Phase 6.3 双层分离模式（SubconstructNode/AdapterCallbackNode） |
| `docs/decisions/ADR-022-用户面Adapter-Python层化.md` | Hex/HexDump Rust Node 论证（§0.2 框架） |
| `docs/decisions/ADR-014-*` | 不接 callable 硬约束（Default/Check/Aligned/Checksum 同脉络） |
| `harness/experiences.md §L-14` | 设计硬约束认知需交叉验证（Checksum 零拷贝触发） |
| `harness/experiences.md §L-01` | 中间表示层违反（§0 对照表对策） |
| `harness/experiences.md §L-02/L-05` | 性能假设需覆盖所有 FFI/拷贝来源 |
| `construct-rs/src/stream.rs` | ParseStream::tell/seek/read/remaining/data（P0 复用） |
| `construct-rs/src/error.rs` | ConstructError 枚举 + ExceptionClasses（P0 扩展） |
| `construct-rs/src/schema.rs:130-205` | parse/build FFI 入口（CancelParsing catch 修改点） |
| `construct-rs/src/nodes/pass.rs` | PassNode（TerminatedNode 模式参照） |
| `construct-rs/src/nodes/rebuild.rs` | RebuildNode（DefaultNode 模式参照） |
| `construct-rs/src/nodes/padding.rs` | PaddingNode（AlignedNode 模式参照） |
| `construct-rs/Cargo.toml` | 当前依赖（pyo3/thiserror/enum_dispatch/half）+ P0 新增 5 crate |
| `construct-rs/python/construct/lib/hex.py` | 5 个 Hex 显示类（8.4 port 源） |
| `construct/construct/lib/hex.py` | Python 原版 Hex 显示类（94 行） |
| `construct/construct/core.py` | 22 构造器 Python 源码 |
| `construct/construct/debug.py` | Probe 源码（160 行） |

---

> **设计文档完成时间**：2026-07-30
> **角色**：ARCH
> **状态**：等待 PM 确认 P0 范围 + D-P0-1 ~ D-P0-5 决策 → REV 设计检视 → DEV 实施
> **下一步**：
> - PM 确认 P0 批次范围（6 个子任务）+ D-P0-1 ~ D-P0-5 决策
> - REV 设计检视（§0 对照表 / 性能假设 / 边界条件 / parity 模板）
> - 按 §0.1 实施顺序分派 DEV（8.10 → 8.1 → 8.9 → 8.8 → 8.4 → 8.5）

