---
id: DESIGN-phase6-adapter
status: active
phase: "6"
depends_on: [DESIGN-Architecture, DESIGN-Expr, DESIGN-Array, ADR-001, ADR-004, ADR-006, ADR-012, ADR-015]
supersedes: []
superseded_by: []
last_updated: 2026-07-30
---

# 模块设计：Adapter 核心（Phase 6.3）

> **设计依据**：
> - `AGENTS.md §0`（八条核心原则：一次 FFI / 无中间表示层 / 输入输出无 trait 抽象层 / pyo3 核心 / mashumaro API / enum_dispatch / Result+thiserror / Stream 纯 Rust）
> - `harness/experiences.md §L-01`（中间表示层违反）、`§L-02/L-05`（性能假设与瓶颈覆盖）
> - `plans/phase6-primitives-strings-adapter/总纲.md §PM 决策 2`（Adapter 双层分离，**用户强化约束**）
> - `docs/analysis/分析报告-Phase6启动前置.md §B.4`（路线 3 推荐：内置 + 用户面分离）
> - `construct-rs/src/nodes/mod.rs`（现有 24 个 Node 变体 + Construct trait）
> - `construct-rs/src/stream.rs`（ParseStream::seek/tell/read、BuildStream）
> - `construct-rs/src/nodes/transform.rs`（已有的"递归 `Box<Node>` 包装"模式参照）
> - `construct-rs/src/nodes/tell.rs` / `computed.rs`（已有 RO 节点 + compute_ro_value 模式参照）
> - `construct-rs/src/nodes/struct_node.rs`（FieldMode::Ro 路径 + Rebuild 集成点）
> - `construct-rs/src/compile.rs`（描述符识别 + 递归 build_node_from_descriptor）
> - Python 源码：`construct/construct/core.py`
>   （Subconstruct L787 / Adapter L813 / SymmetricAdapter L837 / Rebuild L2975 /
>   Peek L4486 / Pass L4687 / RawCopy L4761）
>
> **角色**：ARCH
> **状态**：DESIGNING
> **创建时间**：2026-07-30
> **子任务**：`6.3 [Adapter 详细设计]`

---

## 0. 设计概述与 PM 决策 2 落实

### 0.1 范围

| 构造器 | 实现层 | Python 行号 | 本设计 |
|--------|--------|------------|--------|
| `Subconstruct(subcon)` | Rust Node | L787-810 | `SubconstructNode`（纯转发） |
| `RawCopy(subcon)` | Rust Node | L4761-4814 | `RawCopyNode`（捕获 raw bytes） |
| `Peek(subcon)` | Rust Node | L4486-4545 | `PeekNode`（seek 回退） |
| `Rebuild(subcon, func)` | Rust Node | L2975-3028 | `RebuildNode`（build 走 ExprProgram） |
| `Pass` | Rust Node | L4687-4723 | `PassNode`（no-op） |
| `Adapter(subcon)` | **Python 层** | L813-834 | `AdapterDescriptor`（钩子，详见 §4） |
| `SymmetricAdapter(subcon)` | **Python 层** | L837-846 | 复用 Adapter 机制 |

### 0.2 双层分离约束（PM 决策 2 强化）

**约束 1（类型层面分离）**：内置 Adapter 与用户面 Adapter **不共享内部实现**，各自独立的 Rust 类型 / Python 类型。不存在"一个类型内部两条路径"。

**约束 2（用户面 Adapter 在 Python 层）**：通用 `Adapter` / `SymmetricAdapter` 基类**基本在 Python 层实现**。Rust 不背"通用用户回调"包袱。

**双层分工对照**：

| 层 | 实现位置 | FFI 次数（每次 parse/build） | 性能目标 | 用户主动选择 |
|----|---------|----------------------------|---------|------------|
| 内置 Adapter（Subconstruct/RawCopy/Peek/Rebuild） | Rust Node 变体 | **1 次**（仅编译期一次 + 运行时零 FFI） | ≥10x（与现有 Node 同量级） | 用户用具体名（如 `Peek(Int8ub)`） |
| 用户面 Adapter（Adapter/SymmetricAdapter 基类） | Python 类（用户继承） | **≥2 次**（详见 §4.3） | 用户主动接受折衷（不设硬门禁） | 用户继承 `Adapter` 写 `_decode/_encode` |
| Pass | Rust Node 变体 | **1 次** | 不适用（no-op） | Phase 7 If/Switch 默认值 |

### 0.3 与参考实现分析报告（§B.4）的差异

§B.4 的"路线 3（混合）"原本把 Adapter 列为"内置 Node"。**PM 决策 2 强化后**，Adapter 从 Rust Node 改为 Python 层构造器，仅 Subconstruct/RawCopy/Peek/Rebuild 作为 Rust Node。

**ARCH 确认**：PM 决策 2 不违反任何 §0 原则——反而更彻底地落实"Rust 不背用户回调包袱"。本设计在 §4 详细论证 Adapter Python 层化与 §0 的关系。

---

## 1. 内置 Adapter Rust Node 设计（4 个）

### 1.1 SubconstructNode（纯转发包装）

#### 1.1.1 数据结构

```rust
/// 单子构造器包装节点：parse/build/sizeof 全部转发给 inner。
///
/// 对应 Python construct `Subconstruct`（core.py L787）。Subconstruct 在
/// Python 是抽象基类（Adapter/RawCopy/Peek/Rebuild/Tunnel 等的父类），
/// 但 construct-rs 把它实现为**具体节点**，用于：
/// - 用户显式包装（极少用，主要供未来 Pointer/Prefixed 复用）
/// - 作为 §2 其他内置 Adapter 的实现基础（共享"持有 `Box<Node>` + 转发"模式）
///
/// # 三方法行为
///
/// - parse：`inner.parse(...)`，结果原样返回（不解码）
/// - build：`inner.build(obj, ...)`，obj 原样传入（不编码）
/// - sizeof：`inner.sizeof(ctx)`
#[derive(Debug)]
pub struct SubconstructNode {
    inner: Box<Node>,
}

impl SubconstructNode {
    pub fn new(inner: Node) -> Self { Self { inner: Box::new(inner) } }
    pub fn inner(&self) -> &Node { &self.inner }
}
```

#### 1.1.2 Construct impl

```rust
impl Construct for SubconstructNode {
    fn parse<'py>(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
        self.inner.parse(py, stream, ctx, path)  // 直接转发
    }
    fn build(&self, py, obj, stream, ctx, path) -> Result<(), ConstructError> {
        self.inner.build(py, obj, stream, ctx, path)
    }
    fn sizeof(&self, ctx) -> Result<usize, ConstructError> {
        self.inner.sizeof(ctx)
    }
}
```

**`has_expressions` 集成**：`Node::has_expressions()` 在 mod.rs 加分支 `Node::Subconstruct(s) => s.inner().has_expressions()`（与 Bitwise/Bytewise/Transform 同模式）。

**§0 对照**：1 次 FFI（仅 Struct 顶层 parse 入口穿越，内部转发零 FFI）；无中间表示层（PyObject 原样透传）；无 I/O trait 抽象（仅 `Box<Node>`）。

---

### 1.2 PeekNode（预读不消费流）

#### 1.2.1 数据结构

```rust
/// 预读节点：parse 子解析后回退 stream 到入口位置（不消费字节）。
///
/// 对应 Python construct `Peek`（core.py L4486）。Python 行为：
/// - parse：记录 fallback = stream.tell()；try subcon._parsereport；
///   except ConstructError（非 ExplicitError）吞掉返回 None；finally stream.seek(fallback)
/// - build：no-op（`return obj`）
/// - sizeof：0
///
/// # 与 Python 的差异（行为对齐，仅错误传播路径不同）
///
/// Python 用 try/except 吞掉 ConstructError。construct-rs 用 [`ConstructError`]
/// 的 [`ConstructError::StopField`] 哨兵模式 + 显式错误分类判断：
/// - 子解析成功 → 返回值，然后 seek 回 fallback
/// - 子解析 Err 且**非** ExplicitError 等价物 → 返回 Py_None，seek 回 fallback
/// - 子解析 Err 且**是** ExplicitError 等价物 → 向上传播（seek 回 fallback 再 Err）
///
/// ExplicitError 等价物判定：见 §5.1 边界 PE-3。
#[derive(Debug)]
pub struct PeekNode {
    inner: Box<Node>,
}

impl PeekNode {
    pub fn new(inner: Node) -> Self { Self { inner: Box::new(inner) } }
    pub fn inner(&self) -> &Node { &self.inner }
}
```

#### 1.2.2 Construct impl

```rust
impl Construct for PeekNode {
    fn parse<'py>(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
        let fallback = stream.tell();
        let result = self.inner.parse(py, stream, ctx, path);
        // 无论成功/失败，最终都 seek 回 fallback（对齐 Python finally）
        let seek_outcome = stream.seek(fallback, path);
        match result {
            Ok(value) => {
                seek_outcome?;  // seek 自身失败转 Err
                Ok(value)
            }
            Err(e) => {
                // seek 失败优先上报（即使原错误被吞）
                seek_outcome?;
                if is_explicit_error(&e) {
                    return Err(e);  // PE-3: ExplicitError 等价物向上传播
                }
                // 非 explicit：吞掉，返回 None（对齐 Python `except ConstructError: pass`）
                Ok(py.None())
            }
        }
    }
    fn build(&self, _py, _obj, _stream, _ctx, _path) -> Result<(), ConstructError> {
        Ok(())  // build no-op（对齐 Python `return obj`，但 construct-rs build 不返回值）
    }
    fn sizeof(&self, _ctx) -> Result<usize, ConstructError> {
        Ok(0)  // sizeof = 0（对齐 Python）
    }
}
```

**关键点**：
1. `stream.seek(fallback, path)` 在 `ParseStream` 已存在（Phase 4 GreedyRange 用，stream.rs L148）。`seek` 会重置 `bit_pos=0`——Peek 内部若走 bit 域（如 `Peek(Bitwise(...))`），子 parse 退出 BitwiseNode 时已对齐字节，fallback 也是字节对齐位置，seek 安全。
2. `bit_pos != 0` 时调用 `Peek`：Python `stream_tell` 在 bit 流仍返回字节偏移。construct-rs `ParseStream::tell()` 返回字节 `pos`（stream.rs L128 注释明确），与 Python 对齐。但若 inner 内部推进了 bit_pos 未跨字节边界，fallback-tell 与 seek 后 bit_pos 被重置为 0 会丢失部分 bit 状态——**这在 Python 同样不支持**（Python `stream_seek(stream, fallback, 0)` 也是字节级 seek）。视为已知行为对齐。
3. **`is_explicit_error` 判定**（详见 §5.1 PE-3）：当前 `ConstructError` 枚举无 `Explicit` 变体。Python `ExplicitError` 是用户主动抛出的"不可被 Peek/Select 吞掉"的错误。**Phase 6 决策**：暂不引入 `ConstructError::Explicit` 变体（Phase 6 范围内无用户场景需要），`is_explicit_error` 初始实现统一返回 `false`（所有错误都被 Peek 吞掉）。未来若用户场景需要（如 Phase 7 Select），新增 `ConstructError::Explicit` 变体后改一行即可。

---

### 1.3 RawCopyNode（捕获原始字节）

#### 1.3.1 数据结构

```rust
/// 原始字节捕获节点：parse 返回 Container(data,value,offset1,offset2,length)。
///
/// 对应 Python construct `RawCopy`（core.py L4761）。Python 行为：
/// - parse：offset1 = tell(); obj = subcon.parse(); offset2 = tell();
///   seek(offset1); data = read(offset2-offset1);
///   return Container(data=data, value=obj, offset1=offset1, offset2=offset2, length=...)
/// - build：
///   - obj 含 'data' 键：write(data)；返回 Container(obj, data, offset1, offset2, length)
///   - obj 含 'value' 键：offset1=tell(); subcon.build(value); offset2=tell();
///     seek(offset1); data=read(offset2-offset1); 返回 Container(obj, data, value, offset1, ...)
///   - 否则：RawCopyError
/// - sizeof：inner.sizeof(ctx)
///
/// # 返回值类型
///
/// parse 返回 **Python dict**（含 5 键：data/value/offset1/offset2/length）。
/// 使用 `PyDict::new_bound(py)` 直接构造，零中间表示层（dict 本身就是
/// Python 对象，§0 #1 合规——与 StructNode 用实例 __dict__ 同脉络）。
#[derive(Debug)]
pub struct RawCopyNode {
    inner: Box<Node>,
}

impl RawCopyNode {
    pub fn new(inner: Node) -> Self { Self { inner: Box::new(inner) } }
    pub fn inner(&self) -> &Node { &self.inner }
}
```

#### 1.3.2 Construct impl

```rust
impl Construct for RawCopyNode {
    fn parse<'py>(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
        let offset1 = stream.tell();
        let value = self.inner.parse(py, stream, ctx, path)?;
        let offset2 = stream.tell();
        let length = offset2.checked_sub(offset1).ok_or_else(|| ConstructError::Generic {
            message: format!("RawCopy: offset2 < offset1 ({}, {})", offset2, offset1),
            path: path.to_string(),
        })?;
        // seek 回 offset1 重读 raw bytes
        stream.seek(offset1, path)?;
        let data_slice = stream.read(length, path)?;
        // 构造 Python dict（直接 PyDict，无中间 Container 类型）
        let dict = PyDict::new_bound(py);
        dict.set_item("data", PyBytes::new_bound(py, data_slice))?;
        dict.set_item("value", value.bind(py))?;
        dict.set_item("offset1", offset1.into_py(py).bind(py))?;
        dict.set_item("offset2", offset2.into_py(py).bind(py))?;
        dict.set_item("length", length.into_py(py).bind(py))?;
        Ok(dict.into_any().unbind())
    }
    fn build(&self, py, obj, stream, ctx, path) -> Result<(), ConstructError> {
        // 检查 'data' 键优先（对齐 Python）
        let has_data = obj.getattr("__contains__")?.call1(("data",))?.extract::<bool>()?;
        let has_value = obj.getattr("__contains__")?.call1(("value",))?.extract::<bool>()?;
        if has_data {
            let data: &[u8] = obj.get_item("data")?.extract()?;
            stream.write(data);
            Ok(())
        } else if has_value {
            // subcon.build(value)，记录 offset 后回读（但 build 已写字节，无需回读再覆盖）
            // Python 的回读行为是为了"返回 data"——construct-rs build 不返回值，
            // 因此只需 build 即可，省去 seek+read
            let value = obj.get_item("value")?;
            self.inner.build(py, &value, stream, ctx, path)
        } else {
            Err(ConstructError::Generic {
                message: "RawCopy cannot build: both data and value keys are missing".to_string(),
                path: path.to_string(),
            })
        }
    }
    fn sizeof(&self, ctx) -> Result<usize, ConstructError> {
        self.inner.sizeof(ctx)
    }
}
```

**关键点**：
1. **Python 的 RawCopy.build 返回 Container**（带 data 键），用于后续字段引用。construct-rs 的 build 不返回值，因此用户面无法拿到 build 出的 raw bytes——这与 construct-rs 整体设计一致（build 路径无返回值，§0 #1 的"一次 FFI"是指 build 不回流数据到 Python）。**已知行为差异**，记入 §5.3 RC-build-1。
2. **`'in'` 操作符**：Python `obj is None and self.flagbuildnone` / `'data' in obj` 在 construct-rs 用 `__contains__` + `get_item` 模拟。Python 用户面 RawCopy 字段的值通常是 dict，Python dict 自带 `__contains__`。若用户传非 dict 对象，会拿到 `ConstructError::Generic`（行为对齐 Python `TypeError` 隐式路径）。
3. parse 路径用 `PyDict` 直接构造（5 个 set_item），无中间 Container 类型——`Container` 在 construct-rs 不存在（设计 ADR-008 + ADR-011 决策）。

---

### 1.4 RebuildNode（build 时基于已 build 数据重算字段）

#### 1.4.1 数据结构与 RO 模式集成

```rust
/// build 时重算字段节点。
///
/// 对应 Python construct `Rebuild(subcon, func)`（core.py L2975）。Python 行为：
/// - parse：转发 subcon.parse（继承自 Subconstruct）
/// - build：忽略传入 obj，obj = evaluate(func, context)；subcon.build(obj)
/// - sizeof：subcon.sizeof
/// - flagbuildnone = True（用户可不提供 build 值）
///
/// # construct-rs 集成：作为 RO 字段
///
/// Rebuild 的本质是"build 值不来自用户输入，而来自表达式求值"——与 `Computed`
/// 同类（[`crate::nodes::computed::ComputedNode`]）。**Rebuild 必须作为 `FieldMode::Ro`
/// 字段使用**（用户写 `count: int = rfield(Rebuild(Byte, this.items.length))`）。
///
/// 编译期校验：`RebuildDescriptor` 在 `_field_kind` 返回 `"ro"`（与 Computed/Tell 一致），
/// 若用户用 `field(...)`（rw）或 `wfield(...)`（wo）包装，编译期拒绝。
///
/// # 表达式系统
///
/// `func` 必须是 Phase 2 表达式（FieldRef/ExprRef/int 组合），编译为 ExprProgram，
/// 运行时零 FFI 求值。**不接收 Python lambda/callable**（与 RepeatUntil v5 同脉络，
/// 不重蹈 ADR-013 → ADR-014 的覆辙）。
#[derive(Debug)]
pub struct RebuildNode {
    inner: Box<Node>,
    /// build 时求值的表达式（编译为 ExprProgram）。
    func: ExprProgram,
}

impl RebuildNode {
    pub fn new(inner: Node, func: ExprProgram) -> Self {
        Self { inner: Box::new(inner), func }
    }
    pub fn inner(&self) -> &Node { &self.inner }
    pub fn func(&self) -> &ExprProgram { &self.func }
}
```

#### 1.4.2 Construct impl

```rust
impl Construct for RebuildNode {
    fn parse<'py>(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
        // parse 转发 subcon
        self.inner.parse(py, stream, ctx, path)
    }
    fn build(&self, py, obj, stream, ctx, path) -> Result<(), ConstructError> {
        // build：先求值表达式得到 i64，转 PyLong，再交给 subcon.build
        // 注：Rebuild 作为 RO 字段时，obj 由 StructNode.compute_ro_value 提供
        // （compute_ro_value 调本节点的 compute_ro_value 分支，详见 §3.2）
        // 此处 build 的 obj 是 compute_ro_value 的返回值（PyLong）。
        // 但 Python 语义是"忽略 obj，重算"——为对齐 Python，build 内部仍调
        // 表达式求值（即使 obj 已是 compute_ro_value 计算的值）。
        // 双重求值的开销：仅一次 ExprProgram::eval（~10ns），可忽略。
        let value_i64 = crate::expr::eval_expr_int(&self.func, ctx, py)?;
        let value_py = value_i64.into_py(py);
        self.inner.build(py, value_py.bind(py), stream, ctx, path)
    }
    fn sizeof(&self, ctx) -> Result<usize, ConstructError> {
        self.inner.sizeof(ctx)
    }
}
```

#### 1.4.3 compute_ro_value 集成

`Node::compute_ro_value`（mod.rs L290）需新增 `Rebuild` 分支：

```rust
Node::Rebuild(r) => {
    // 求值表达式得到 i64，转 PyLong（与 Computed 同模式）
    let v = crate::expr::eval_expr_int(r.func(), ctx, py)?;
    Ok(v.into_py(py))
}
```

**双重求值的语义澄清**：`compute_ro_value` 计算的值会作为 `RebuildNode.build` 的 `obj` 入参；`build` 内部再次求值表达式。这是设计上的冗余——但 `compute_ro_value` 必须返回一个值供 StructNode 写入 context（供后续表达式引用），而 `build` 内部无法区分"obj 是 compute_ro_value 算的还是用户给的"。

**优化**：`RebuildNode.build` 直接用 `obj`（不再求值），由 `compute_ro_value` 唯一负责求值。但 `Rebuild` 也可能作为 RW 字段使用（用户传值覆盖）——此时 compute_ro_value 不被调用，build 必须自己求值。**当前设计**：build 总是求值（与 Python 语义一致："build value is ignored"）。性能开销可忽略（ExprProgram eval ~10ns vs FormatField build ~30ns）。

---

## 2. Pass Node 设计（no-op）

### 2.1 数据结构

```rust
/// No-op 节点：parse 返回 Py_None，build 不写字节，sizeof=0。
///
/// 对应 Python construct `Pass`（core.py L4687）。Python 是 `@singleton`，
/// construct-rs 用零字段单元结构体（编译期占位，运行时零开销）。
///
/// # Phase 7 依赖
///
/// Pass 是 Phase 7 `If`/`Switch` 默认值的依赖：
/// - `If(cond, then)` ≡ `IfThenElse(cond, then, Pass)`
/// - `Switch(key, cases, default=Pass)`
///
/// 作为 Phase 6.3 的"附带品"实现（< 100 行 Rust，含测试）。
#[derive(Debug, Default)]
pub struct PassNode;

impl PassNode {
    pub fn new() -> Self { Self }
}
```

### 2.2 Construct impl

```rust
impl Construct for PassNode {
    fn parse<'py>(&self, py, _stream, _ctx, _path) -> Result<Py<PyAny>, ConstructError> {
        Ok(py.None())
    }
    fn build(&self, _py, _obj, _stream, _ctx, _path) -> Result<(), ConstructError> {
        Ok(())
    }
    fn sizeof(&self, _ctx) -> Result<usize, ConstructError> {
        Ok(0)
    }
}
```

**`has_expressions` 集成**：Pass 不含表达式，`Node::has_expressions` 默认 `false` 分支已覆盖（mod.rs L260 `_ => false`）。`compute_ro_value` 同理——Pass 不作为 RO 字段（用户无此场景），落入 `_ => Err(Generic)` 兜底分支即可。

**实现规模**：`pass.rs` 文件含 doc + impl + 单元测试，预估 < 100 行（与 TellNode 同量级，tell.rs 是 359 行含 200+ 行测试，纯实现部分 < 80 行）。

---

## 3. Node enum 扩展 + compute_ro_value 集成

### 3.1 Node enum 新增 5 个变体

`construct-rs/src/nodes/mod.rs` 的 `Node` enum 当前 24 个变体，新增：

```rust
pub enum Node {
    // ... 现有 24 个 ...
    /// 单子构造器包装节点（对应 Python Subconstruct）。Phase 6.3 新增。
    Subconstruct(SubconstructNode),
    /// 预读不消费流节点（对应 Python Peek）。Phase 6.3 新增。
    Peek(PeekNode),
    /// 原始字节捕获节点（对应 Python RawCopy）。Phase 6.3 新增。
    RawCopy(RawCopyNode),
    /// build 时基于表达式重算字段节点（对应 Python Rebuild）。Phase 6.3 新增。
    /// 必须作为 RO 字段使用（与 Computed 同类）。
    Rebuild(RebuildNode),
    /// No-op 节点（对应 Python Pass）。Phase 6.3 新增（Phase 7 If/Switch 默认值依赖）。
    Pass(PassNode),
}
```

**mod.rs 同步修改**：
- `pub mod pass;` `pub mod peek;` `pub mod raw_copy;` `pub mod rebuild;` `pub mod subconstruct;`（5 个新模块声明）
- `use` 5 个新 Node 类型
- `Node::has_expressions()` 加 5 个分支（Subconstruct/Peek/RawCopy 递归 `inner().has_expressions()`；Rebuild 总是 `true`——含 func 表达式；Pass `false`）

### 3.2 compute_ro_value 集成（mod.rs L290）

`Node::compute_ro_value` 新增 `Rebuild` 分支（其他 4 个新 Node 不作为 RO 字段，落入 `_ => Err` 兜底）：

```rust
pub fn compute_ro_value(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
    match self {
        // ... 现有 Tell/Computed/Index/StopIf/Element 分支 ...
        Node::Rebuild(r) => {
            let v = crate::expr::eval_expr_int(r.func(), ctx, py)?;
            Ok(v.into_py(py))
        }
        _ => Err(ConstructError::Generic { /* ... */ }),
    }
}
```

**Rebuild 作为非 RO 字段时**：若用户错误地用 `field(Rebuild(...))`（RW），编译期 `_field_kind` 校验拒绝（详见 §6.2）。运行时若 Rebuild 字段意外走 RW build 路径，`build` 内部仍调表达式求值，行为正确（与 RO 路径一致），但 context 中无该字段值供后续表达式引用——这是用户面错误，由编译期校验防止。

---

## 4. 用户面 Adapter（Python 层实现）

> **PM 决策 2 约束 2 强化**：通用 `Adapter` / `SymmetricAdapter` 在 Python 层实现。
> Rust 层**不**新增 Adapter Node 变体。Rust 仅提供"最小通用钩子"（§4.4）。

### 4.1 Python 层 Adapter 基类设计

```python
# construct-rs/python/construct/_adapters.py（新建）

class Adapter:
    """通用 Adapter 基类（用户继承 + 实现 _decode/_encode）。

    对应 Python construct core.py:813 的 Adapter。在 construct-rs 中，
    Adapter **完全在 Python 层实现**——用户继承此类并实现 _decode/_encode，
    parse/build 路径在 Python 层调用 subcon 的 Rust parse/build。

    使用方式：

        class HexAdapter(Adapter):
            def _decode(self, obj, context, path):
                return hex(obj)
            def _encode(self, obj, context, path):
                return int(obj, 16)

        adapter = HexAdapter(Int8ub)
        adapter.parse(b"\\x10")  # → "0x10"
        adapter.build("0x10")    # → b"\\x10"

    性能说明：用户主动继承 Adapter = 显式接受 Python 层解码开销。
    详见设计文档 §4.3 FFI 边界分析。
    """

    def __init__(self, subcon):
        if not isinstance(subcon, ...):  # 接受任意 construct-rs 描述符或 Adapter
            raise TypeError("subcon should be a construct field")
        self.subcon = subcon
        self.flagbuildnone = getattr(subcon, "flagbuildnone", False)

    def parse(self, data, **kw):  # 顶层 parse 入口（与 StructMixin.parse 同模式）
        # 调用 subcon.parse（一次 Rust FFI）→ Python obj → _decode（Python）→ 返回
        stream = _make_stream(data)
        ctx = _make_context(kw)
        obj = self.subcon._parse(stream, ctx, "<root>")  # 触发 Rust FFI
        return self._decode(obj, ctx, "<root>")

    def build(self, obj, **kw):  # 顶层 build 入口
        stream = _make_build_stream()
        ctx = _make_context(kw)
        obj2 = self._encode(obj, ctx, "<root>")  # Python 层编码
        self.subcon._build(obj2, stream, ctx, "<root>")  # 触发 Rust FFI
        return stream.getvalue()

    def _decode(self, obj, context, path):
        raise NotImplementedError

    def _encode(self, obj, context, path):
        raise NotImplementedError


class SymmetricAdapter(Adapter):
    """对称 Adapter：_encode = _decode（用户只需实现 _decode）。"""

    def _encode(self, obj, context, path):
        return self._decode(obj, context, path)
```

### 4.2 用户面 Adapter 嵌入 Struct 字段

Adapter 可作为 Struct 字段使用（Python 用户面）：

```python
@dataclass
class Packet(StructMixin):
    value: str = field(HexAdapter(Int8ub))
```

**编译期识别**（§6.3 详述）：`compile_schema` 在 `build_node_from_descriptor` 中识别 `Adapter` 实例（通过 `isinstance(desc, Adapter)` 或 duck typing type name），将其编译为**"Python callable 节点"占位**——Rust 不直接执行 _decode/_encode，而是把 subcon 部分编译为 Rust Node，Adapter 的 _decode/_encode 在 Python 侧在 parse/build 的"Struct 顶层"完成。

### 4.3 FFI 边界分析（关键设计问题）

**用户面 Adapter 的 FFI 次数**：

| 场景 | Rust FFI 次数 | Python 调用 | 总边界穿越 |
|------|-------------|-----------|----------|
| `HexAdapter(Int8ub).parse(b)` 单独使用 | 1（subcon._parse） | 1（_decode） | 2 |
| `HexAdapter(Int8ub)` 嵌入 Struct 第 i 字段 | 1（Struct 顶层 parse，subcon 在 Rust 内执行） | 1（_decode，Python 层后处理） | **2** |
| 普通 `Int8ub` 嵌入 Struct 第 i 字段 | 1（Struct 顶层 parse） | 0 | 1 |

**详细说明（嵌入 Struct 场景）**：

```
用户：Struct { value: HexAdapter(Int8ub) }
parse(b"\\x10")：

1. Python 调 StructMixin.parse → 跨 FFI 进入 Rust（1 次）
2. Rust StructNode.parse 遍历字段：
   - 识别 HexAdapter 字段 → Rust 端无法直接执行（_decode 是 Python 方法）
   - Rust 调 subcon (Int8ub) parse → 得到 PyLong(16)
   - **Rust 调 Python adapter._decode(16, ctx, path)** → 得到 PyString("0x16")
     （这是 Rust→Python 的回调，算 1 次额外 FFI 穿越）
3. Rust 返回 Struct 实例 → 跨 FFI 回 Python（与步骤 1 配对，算同次会话）

总：1 次主 FFI（parse 入口）+ 1 次 Rust→Python 回调（_decode）= **2 次边界穿越**
```

### 4.4 §0 合规性论证（PM 决策点的核心）

**§0 #1（一次 FFI）合规性**：

- **内置 Adapter（Subconstruct/RawCopy/Peek/Rebuild/Pass）**：✅ 严格 1 次 FFI。Rust Node 内部完成所有工作，无 Rust→Python 回调。
- **用户面 Adapter（Adapter/SymmetricAdapter）**：⚠️ **2 次 FFI**（parse 入口 + _decode 回调）。

**用户面 Adapter 是否违反 §0 #1？**

**ARCH 论证：不违反，理由如下**：

1. **§0 #1 的精确语义**（AGENTS.md 原文）："编译 / parse / build 各只有一次 Python↔Rust 边界穿越，Rust 内部通过 CPython C API 直接操作 Python 对象"。这条原则针对的是**编译期生成的执行树**——执行树一旦编译完成，运行时不应每次字段都跨越 FFI。
2. **用户面 Adapter 不在执行树内**：Adapter 的 _decode/_encode 是**用户 Python 代码**，无法被 Rust 编译为 Node。它的本质就是"用户在 Python 层包了一层"。Rust 不能"穿透"到用户的 Python 类里去执行用户写的代码——这不是 §0 #1 要禁止的"中间表示层"，而是用户主动添加的"用户域后处理"。
3. **PM 决策 2 已明确**："用户主动选 Adapter = 显式接受性能折衷"（与 Phase 4 RepeatUntil v5 删 PyCallable 兜底同一脉络）。用户面 Adapter 是用户的**显式选择**，不是构造器的"内部决策路径"。
4. **类比**：Python 用户也可以自己写 `parsed = MyStruct.parse(data); parsed.value = hex(parsed.value)`——这是 100% Python 后处理。Adapter 只是把这个后处理"包装"成了构造器形式，**不改变 FFI 次数本质**（仍是 Rust parse 1 次 + Python 后处理 N 次）。

**§0 #2（无中间表示层）合规性**：

- **内置 Adapter**：✅ 无中间表示层。Subconstruct 透传 PyObject；RawCopy 用 PyDict（本身就是 Python 对象）；Peek 用 ParseStream::seek（纯 Rust）；Rebuild 用 ExprProgram（已验证）。
- **用户面 Adapter**：⚠️ _decode/_encode 的输入输出是 Python 对象——但这些 Python 对象**就是最终用户看到的对象**，不是"中间表示"（中间表示指 Rust 端的临时数据类型再转换）。用户面 Adapter 的"中间态"在用户域，不在 Rust 域。

**§0 #3（输入输出无 trait 抽象层）合规性**：

- 内置 Adapter 仅持有 `Box<Node>`（与 Bitwise/Bytewise/Transform 同模式，已在 Phase 3 验证），**不引入新 trait**。
- 用户面 Adapter 在 Python 层，与 trait 无关。

### 4.5 Rust 层"最小通用钩子"（PM 决策 2 允许的范围）

PM 决策 2 约束 2 原文："最多提供最小通用钩子（如 StructMixin 的某个机制让 Adapter 包装的子构造器仍走 Rust parse/build）"。

**ARCH 设计：最小钩子 = AdapterCallbackNode（仅在 Adapter 嵌入 Struct 时使用）**

```rust
/// Adapter 回调节点：嵌入 Struct 字段时，subcon 在 Rust 执行，adapter 的
/// _decode/_encode 通过 Py<PyAny> 引用在 Rust→Python 回调中执行。
///
/// **仅用于 Adapter/SymmetricAdapter 嵌入 Struct 场景**。用户单独使用 Adapter
/// （如 HexAdapter(Int8ub).parse(b)）不经过此节点（直接走 Python 层 Adapter.parse）。
///
/// 此节点是 PM 决策 2 允许的"最小通用钩子"——它不引入"通用用户回调"机制，
/// 仅在 Struct 字段编译期把"Adapter 包装"翻译为"subcon Rust Node + 回调引用"。
#[derive(Debug)]
pub struct AdapterCallbackNode {
    subcon: Box<Node>,           // Rust 端执行的 subcon
    decode: Py<PyAny>,           // 用户 _decode 方法引用
    encode: Py<PyAny>,           // 用户 _encode 方法引用
    adapter_instance: Py<PyAny>, // self 引用（_decode/_encode 是 bound method）
}
```

**为什么这是"最小"钩子，不是"通用用户回调包袱"**：

1. **不接收任意 callable**：仅识别 `Adapter` 子类实例（编译期 type check），用户不能传 lambda 当字段（与 RepeatUntil v5 同硬约束）。
2. **不引入新表达式机制**：_decode/_encode 是用户 Python 方法，Rust 不解析其内部逻辑，只调 `decode.call(py, (obj, ctx, path))`。
3. **不破坏 §0**：用户主动继承 Adapter = 已接受 2 次 FFI（§4.4 论证）。

> **替代方案（不推荐，列出供 PM 决策）**：完全禁止 Adapter 嵌入 Struct，强制用户在 Struct 外层包装（`adapter.parse(struct.build(data))` 模式）。优点：Rust 完全不背 Adapter 包袱，§0 #1 严格 1 次 FFI。缺点：用户面 API 严重破坏（Python construct 习惯 `field(HexAdapter(Int8ub))`），且实际 FFI 次数未减少（用户必然在 Python 层做转换）。

**ARCH 推荐**：采用 AdapterCallbackNode 最小钩子方案。理由：(1) 用户面 API 兼容 Python 习惯；(2) 性能损失用户已显式接受；(3) 实现简单（< 200 行 Rust）；(4) 未来 Phase 7+ 用户面 Adapter 子类（Hex/HexDump/Enum/Validator/Mapping）均复用此机制。

> **PM 决策点（详见 §8 D-1）**：是否采用 AdapterCallbackNode 最小钩子方案，还是完全禁止 Adapter 嵌入 Struct。

---

## 5. 边界条件清单

### 5.1 内置 Adapter 边界

| 编号 | 场景 | 预期行为 |
|------|------|---------|
| SC-1 | `Subconstruct(FormatField)` parse/build/sizeof | 全部转发，结果与直接用 FormatField 一致 |
| SC-2 | `Subconstruct(Struct)` 嵌套 | inner.has_expressions 正确递归（决定 context 模式） |
| PE-1 | `Peek(Int8ub)` parse 1 字节 | 返回 PyLong，stream.tell() 不变（seek 回 fallback） |
| PE-2 | `Peek(Int8ub)` 流不足 | inner.parse 返回 Stream Err → Peek 吞掉返回 None，stream 已 seek 回 fallback |
| PE-3 | `Peek(Int8ub)` ExplicitError | 当前实现：所有 Err 都被吞（construct-rs 暂无 Explicit 变体）。**已知行为差异**，未来若引入 ExplicitError 变体需补 is_explicit_error 判定 |
| PE-4 | `Peek(...).build(obj)` | no-op（不写字节，不消费 stream） |
| PE-5 | `Peek(...).sizeof()` | 返回 0 |
| RC-1 | `RawCopy(Int8ub)` parse 1 字节 | 返回 dict{data=b"\\x01", value=1, offset1=0, offset2=1, length=1} |
| RC-2 | `RawCopy(GreedyBytes)` parse 到 EOF | length = stream.len()，data 是全部剩余字节 |
| RC-3 | `RawCopy(Int8ub).build({"data": b"\\xff"})` | 直接 write(b"\\xff")，不调 inner.build |
| RC-4 | `RawCopy(Int8ub).build({"value": 255})` | inner.build(255)，写出 b"\\xff" |
| RC-5 | `RawCopy(Int8ub).build({"unknown": 1})` | ConstructError::Generic（"both data and value keys are missing"） |
| RC-6 | `RawCopy(Int8ub).build({})` | 同 RC-5 |
| RC-build-1 | `RawCopy(Int8ub).build({"value": 255})` 返回值 | construct-rs build 不返回值，用户无法拿到 build 出的 raw bytes。**已知差异**（Python 返回 Container(data=...)）。记入 §5.3 |
| RB-1 | `Rebuild(Byte, this.items.length).parse(b"\\x03")` | 返回 PyLong(3)（转发 inner.parse） |
| RB-2 | `Rebuild(Byte, this.items.length)` 作为 RO 字段 build | 求值 items.length → 写入 context → inner.build(value) |
| RB-3 | `Rebuild(Byte, this.items.length)` 表达式求值失败 | ExprFieldMissing 错误向上传播 |
| RB-4 | 用户用 `field(Rebuild(...))`（RW 模式） | **编译期拒绝**（_field_kind 返回 "ro"） |
| RB-5 | Rebuild 表达式含 lambda/callable | **编译期拒绝**（与 RepeatUntil v5 同硬约束，不接收 callable） |
| PA-1 | `Pass.parse(b"")` | 返回 None |
| PA-2 | `Pass.parse(b"\\x01\\x02")` | 返回 None，stream 不消费字节 |
| PA-3 | `Pass.build(None)` | no-op，输出 b"" |
| PA-4 | `Pass.build(42)` | no-op（忽略 obj），输出 b"" |
| PA-5 | `Pass.sizeof()` | 返回 0 |

### 5.2 AdapterCallbackNode 边界（若 PM 采用 §4.5 方案）

| 编号 | 场景 | 预期行为 |
|------|------|---------|
| AC-1 | `field(HexAdapter(Int8ub))` parse | subcon (Int8ub) Rust parse → 回调 _decode(16) → "0x10" |
| AC-2 | `field(HexAdapter(Int8ub))` build "0x10" | 回调 _encode("0x10") → 16 → subcon.build(16) |
| AC-3 | _decode 抛 Python 异常 | 跨 FFI 转为 ConstructError::Generic（含原始 traceback） |
| AC-4 | Adapter 嵌套 Adapter（`HexAdapter(HexAdapter(Int8ub))`） | 内层 Adapter 编译为 AdapterCallbackNode，外层 AdapterCallbackNode.subcon = AdapterCallbackNode。递归回调 2 次（每次 _decode） |

### 5.3 已知行为差异表（与 Python construct 2.10.70 对比）

| 编号 | 差异 | 原因 | 处理 |
|------|------|------|------|
| PE-3 | Peek 不区分 ExplicitError | construct-rs 当前无 ExplicitError 变体 | Phase 6 不引入；Phase 7 若 Select 需要，再补 |
| RC-build-1 | RawCopy.build 不返回带 data 的 Container | construct-rs build 路径无返回值（§0 #1） | 文档化；用户需 raw bytes 应走 parse 路径 |
| AC-FFI | 用户面 Adapter 嵌入 Struct 有 2 次 FFI | PM 决策 2：用户主动选 Adapter | 文档化性能折衷，不设硬门禁 |

---

## 6. 与其他模块的交互

### 6.1 依赖（前置）

- **Phase 1 FormatField / Bytes / GreedyBytes**：作为 Subconstruct/Peek/RawCopy 的 inner 测试用例
- **Phase 2 ExprProgram**：Rebuild 的 func 走 ExprProgram 求值
- **Phase 2 FieldMode (ADR-004)**：Rebuild 必须为 RO 字段
- **Phase 3 Bitwise/Bytewise/Transform**：复用 `Box<Node>` 递归模式（已有 3 个先例）
- **Phase 4 Array**：AdapterCallbackNode 若包装 Array 字段，subcon 递归到 Array Node
- **stream.rs ParseStream::seek/tell/read**：Peek 和 RawCopy 已有 API 可用（Phase 4 新增 seek）

### 6.2 被依赖

- **Phase 7 If/Switch**：默认值依赖 `Pass`
- **Phase 7 Pointer/Prefixed**：实现上类似 Subconstruct（持有 inner + 转发 + 加 seek/子流逻辑）——本设计的 SubconstructNode 是它们的"模式参照"
- **Phase 6+ Adapter 子类（Hex/HexDump/Enum/Validator/Mapping/FlagsEnum）**：用户面 Adapter 基类的直接子类，复用 §4 机制

### 6.3 编译管线集成（compile.rs）

`build_node_from_descriptor` 新增 6 个分支（5 个内置 + 1 个 AdapterCallback）：

```rust
// 在现有 match type_name 分支中新增：
"SubconstructDescriptor" => {
    let inner_desc = desc.getattr("subcon")?;
    let inner_node = build_node_from_descriptor(py, &inner_desc, field_index, expr_programs, field_names, bitwise)?;
    return Ok(Node::Subconstruct(SubconstructNode::new(inner_node)));
}
"PeekDescriptor" => { /* 同上，Node::Peek */ }
"RawCopyDescriptor" => { /* 同上，Node::RawCopy */ }
"RebuildDescriptor" => {
    let inner_desc = desc.getattr("subcon")?;
    let inner_node = build_node_from_descriptor(...)?;
    // 从 expr_programs[field_index]["func"] 取 ExprOp 列表（与 Computed 同模式）
    let func = compile_expr_param(py, expr_programs, field_index, "func")?;
    return Ok(Node::Rebuild(RebuildNode::new(inner_node, func)));
}
"PassDescriptor" => return Ok(Node::Pass(PassNode::new())),
// AdapterCallbackNode（若 PM 采用 §4.5 方案）：
"AdapterDescriptor" => {
    let inner_desc = desc.getattr("subcon")?;
    let inner_node = build_node_from_descriptor(...)?;
    let decode = desc.getattr("_decode")?;  // bound method
    let encode = desc.getattr("_encode")?;
    return Ok(Node::AdapterCallback(AdapterCallbackNode::new(inner_node, decode, encode, desc.clone())));
}
```

### 6.4 Python 侧描述符（_descriptors.py / 新建 _adapters.py）

```python
# _descriptors.py 新增（纯 Python 描述符，通过 type name 识别）：

class SubconstructDescriptor:
    def __init__(self, subcon): self.subcon = subcon

class PeekDescriptor:
    def __init__(self, subcon): self.subcon = subcon

class RawCopyDescriptor:
    def __init__(self, subcon): self.subcon = subcon

class RebuildDescriptor:
    """func 必须是 FieldRef/ExprRef/int 组合（v5 同脉络，不接收 callable）。"""
    _expr_params = {"func": None}  # 占位，实际 func 在 __init__ 设置
    def __init__(self, subcon, func):
        self.subcon = subcon
        self.func = func
        self._expr_params = {"func": func}
    @property
    def _field_kind(self):  # ADR-004 集成
        return "ro"

class PassDescriptor:
    pass

# 工厂函数（用户面 API）：
def Subconstruct(subcon): return SubconstructDescriptor(subcon)
def Peek(subcon): return PeekDescriptor(subcon)
def RawCopy(subcon): return RawCopyDescriptor(subcon)
def Rebuild(subcon, func): return RebuildDescriptor(subcon, func)
Pass = PassDescriptor()  # singleton（与 GreedyBytes 同模式）

# _adapters.py 新建（用户面 Adapter 基类）：
class Adapter: ...  # §4.1
class SymmetricAdapter(Adapter): ...  # §4.1
```

`__init__.py` 导出新增：`Subconstruct`、`Peek`、`RawCopy`、`Rebuild`、`Pass`、`Adapter`、`SymmetricAdapter`。

---

## 7. §0 原则对照表（L-01 对策，硬要求）

| §0 原则 | 内置 Adapter（Subconstruct/RawCopy/Peek/Rebuild/Pass） | 用户面 Adapter（Adapter/SymmetricAdapter） |
|---------|-----------------------------------------------------|------------------------------------------|
| #1 一次 FFI | ✅ 严格 1 次。Rust Node 内部完成所有工作，无 Rust→Python 回调 | ⚠️ 2 次（parse 入口 + _decode 回调）。**不违反 §0 #1**——详见 §4.4 论证（用户主动选择 = 显式接受折衷；不在执行树内部的"中间表示层"语义内） |
| #2 无中间表示层 | ✅ Subconstruct 透传 PyObject；RawCopy 用 PyDict（本身就是 Python 对象）；Peek 用纯 Rust seek；Rebuild 用 ExprProgram；Pass 无数据 | ✅ _decode/_encode 操作的就是用户域 Python 对象，无 Rust 端中间类型 |
| #3 输入输出无 trait 抽象层 | ✅ 仅 `Box<Node>`（与 Bitwise/Bytewise/Transform 同模式） | ✅ AdapterCallbackNode 持有 `Box<Node> + Py<PyAny>`，不引入新 trait |
| #4 pyo3 核心依赖 | ✅ 全部用 pyo3 直接操作 PyObject（PyDict/PyBytes/PyLong/Py<PyAny>） | ✅ AdapterCallbackNode 用 pyo3 `Py<PyAny>` 引用 _decode/_encode |
| #5 mashumaro 式 API | ✅ 不破坏 StructMixin 用户面（Adapter 是字段描述符，与 Bytes/FormatField 同层） | ✅ 用户继承 Adapter 写 _decode/_encode（Python 原生模式） |
| #6 enum_dispatch 静态分派 | ✅ 5 个新 Node 加入 enum，match 静态分派 | ✅ AdapterCallbackNode 加入 enum（若 PM 采用 §4.5） |
| #7 Result<T, ConstructError> | ✅ 所有 parse/build/sizeof 返回 Result，错误带 path 字段 | ✅ _decode/_encode 异常跨 FFI 转为 ConstructError::Generic（带 path） |
| #8 Stream 抽象纯 Rust 内部 | ✅ Peek 用 ParseStream::seek；RawCopy 用 tell+read+seek。无 Stream 跨 FFI | ✅ AdapterCallbackNode 不直接操作 Stream（subcon 在 Rust 内操作） |

---

## 8. PM 决策点

### D-1：AdapterCallbackNode 最小钩子方案（推荐采用）

**问题**：用户面 Adapter（Adapter/SymmetricAdapter）嵌入 Struct 字段时，Rust 是否提供"最小通用钩子"（AdapterCallbackNode）让 subcon 走 Rust parse/build + 回调用户 _decode/_encode？

**选项 A（ARCH 推荐）**：采用 AdapterCallbackNode。用户面 API 兼容 Python 习惯（`field(HexAdapter(Int8ub))`）；性能损失用户已显式接受（PM 决策 2）；实现 < 200 行 Rust。

**选项 B**：完全禁止 Adapter 嵌入 Struct。用户必须在 Struct 外层包装。Rust 完全不背 Adapter 包袱；但用户面 API 严重破坏，且实际 FFI 次数未减少。

**ARCH 推荐 A**。理由：(1) 用户面兼容；(2) §0 #1 合规（§4.4 论证用户主动选 = 不违反）；(3) 未来 Hex/Enum/Validator 等 Adapter 子类均复用此机制。

### D-2：是否引入 ExplicitError 变体（Peek/Select 用）

**问题**：Peek 当前吞掉所有错误（PE-3）。Python 区分 ExplicitError（用户主动抛，不被吞）。Phase 6 是否引入 `ConstructError::Explicit` 变体？

**ARCH 推荐**：Phase 6 **不引入**（无用户场景需要）。Phase 7 Select 实现时再评估。当前 `is_explicit_error` 占位返回 false。

### D-3：RawCopy build 路径返回值缺失（RC-build-1）

**问题**：Python RawCopy.build 返回 Container(data=...)，construct-rs build 不返回值。用户面差异。

**ARCH 推荐**：文档化（§5.3 RC-build-1）。用户需要 raw bytes 应走 parse 路径。不破坏 construct-rs 整体设计（build 无返回值是 §0 #1 的体现）。

---

## 9. 性能假设（L-02/L-05 对策）

### 9.1 瓶颈识别（量化数据 + 来源）

| Node | 主要瓶颈 | 量化数据来源 |
|------|---------|------------|
| Subconstruct | 零（纯转发） | 与直接用 inner 同性能 |
| Peek | 1 次 stream.seek（常数 ~5ns） + inner.parse | seek 已在 Phase 4 GreedyRange 验证，开销可忽略 |
| RawCopy | inner.parse + 1 次 seek + 1 次 read(length) + 5 次 PyDict.set_item | PyDict::set_item ~30ns（pyo3 benchmark），5 次 ~150ns |
| Rebuild | inner.parse/build + ExprProgram::eval（~10ns） | ExprProgram eval 开销已 in Phase 2 验证 |
| Pass | 零（仅返回 Py_None） | Py_None 是 CPython singleton，零开销 |
| AdapterCallback（用户面） | subcon.parse + Rust→Python 回调 _decode（~200-300ns） | Python 函数调用 ~270-300ns（Phase 4 v5 实测） |

### 9.2 可证伪预测（覆盖所有 FFI/拷贝/转换来源）

**内置 Adapter（嵌入 Struct 字段，与 Python construct 对比）**：

- **Subconstruct**：加速比 = inner 单独的加速比（无额外开销）。预测 Int8ub 嵌入 ~10x。
- **Peek**：加速比 ≥8x（多 1 次 seek，但 seek 是 Rust 内部 ~5ns，远小于 Python stream_tell + stream_seek 各 ~500ns 的节省）。
- **RawCopy**：加速比 ≥5x（PyDict 构造 5 set_item ~150ns，但 Python Container 构造更慢）。**风险**：小字段场景（1 字节 Int8ub），FFI 摊薄主导，加速比可能降到 3-5x。
- **Rebuild**：加速比 ≥8x（与 Computed 同量级，ExprProgram eval ~10ns vs Python lambda ~270ns）。
- **Pass**：不适用（no-op 无对比意义）。

**用户面 Adapter**：

- **不设硬门禁**（PM 决策 2：用户主动接受折衷）。
- **预测**：加速比 1.5-3x（subcon Rust 部分 ~10x，但 _decode Python 回调拉低整体）。
- **可证伪**：若实测 < 1x（比 Python 还慢），说明 Rust→Python 回调开销过大，需重新评估 AdapterCallbackNode 设计。

### 9.3 FFI/拷贝来源清单（L-05 对策：列出所有来源）

**内置 Adapter 单次 parse 的 FFI 来源**：
1. StructMixin.parse 入口（1 次，共享）
2. Rust 内部 ParseStream 操作（零 FFI）
3. Py_None / PyLong / PyDict 构造（Rust 内通过 pyo3 C API，不计 FFI 穿越）

**用户面 Adapter 单次 parse 的 FFI 来源**：
1. StructMixin.parse 入口（1 次，共享）
2. Rust → Python _decode 回调（1 次额外）
3. _decode 内部 Python 操作（用户域，不计）

总计 2 次 FFI 穿越（vs 普通 Struct 字段 1 次）。已在 §4.3 详述。

---

## 10. DEV 实施清单

### 10.1 Rust 端（construct-rs/src/nodes/）

| 文件 | 内容 | 行数估计 |
|------|------|---------|
| `nodes/subconstruct.rs` | SubstructNode + impl + 单元测试 | ~150 |
| `nodes/peek.rs` | PeekNode + impl + is_explicit_error（占位）+ 单元测试 | ~250 |
| `nodes/raw_copy.rs` | RawCopyNode + impl + 单元测试 | ~300 |
| `nodes/rebuild.rs` | RebuildNode + impl + 单元测试 | ~250 |
| `nodes/pass.rs` | PassNode + impl + 单元测试 | ~100 |
| `nodes/adapter_callback.rs`（若 D-1 采用 A） | AdapterCallbackNode + impl + 单元测试 | ~250 |
| `nodes/mod.rs` | 新增 5-6 个 mod/use/enum 变体 + has_expressions + compute_ro_value | ~50 增量 |
| `compile.rs` | 新增 5-6 个 build_*_node 分支 | ~150 增量 |

### 10.2 Python 端（construct-rs/python/construct/）

| 文件 | 内容 | 行数估计 |
|------|------|---------|
| `_descriptors.py` | 5 个 Descriptor 类 + 工厂函数 | ~150 增量 |
| `_adapters.py`（新建） | Adapter + SymmetricAdapter 基类 | ~150 |
| `__init__.py` | 导出 Subconstruct/Peek/RawCopy/Rebuild/Pass/Adapter/SymmetricAdapter | ~30 增量 |

### 10.3 测试（construct-rs/tests/parity/）

| 文件 | 内容 |
|------|------|
| `test_phase6_adapter_parity.py` | 5 个内置 Adapter + Pass 的 parity 测试（vs Python construct 2.10.70） |
| `test_phase6_adapter_errors.py` | RC-5/RC-6/RB-3/RB-4/RB-5 错误路径 |

### 10.4 实施顺序建议（DEV）

1. **Pass**（最简单，< 100 行，先跑通编译管线 + Node enum 集成）
2. **Subconstruct**（纯转发，验证 Box<Node> + has_expressions 模式）
3. **Peek**（验证 seek 集成 + 错误吞掉语义）
4. **RawCopy**（验证 PyDict 构造 + build 双键路径）
5. **Rebuild**（验证 ExprProgram 集成 + RO 字段模式）
6. **AdapterCallback**（若 D-1 采用 A，验证 Rust→Python 回调）

---

## 11. Parity 测试模板（与 Python construct 2.10.70 对照）

```python
# tests/parity/test_phase6_adapter_parity.py（骨架）

import pytest
from _helpers.parity import run_parity_case, assert_parity

# ---------------- Pass ----------------
def test_pass_parse_returns_none():
    case = """
    from construct import Pass
    parsed = Pass.parse(b"")
    """
    results = run_parity_case(...)
    assert_parity(results, "Pass.parse returns None")
    # rs 与 py 都应返回 None

def test_pass_build_noop():
    case = """
    from construct import Pass
    built = Pass.build(None)
    """
    # rs 与 py 都应返回 b""

# ---------------- Subconstruct ----------------
def test_subconstruct_int8ub_parse():
    case = """
    from construct import Subconstruct, Int8ub  # py
    # rs 用 construct_rs.Subconstruct
    parsed = Subconstruct(Int8ub).parse(b"\\x10")
    """
    # rs 与 py 都应返回 16

# ---------------- Peek ----------------
def test_peek_does_not_consume_stream():
    case = """
    from construct import Peek, Int8ub, Bytes
    # 单独用 Peek 难以验证 stream 位置，需嵌入 Struct
    from construct import Struct
    s = Struct("a" / Peek(Int8ub), "b" / Bytes(2))
    parsed = s.parse(b"\\x01\\x02\\x03")
    # py: parsed = Container(a=1, b=b"\\x01\\x02")
    # construct-rs 用 @dataclass + field(Peek(Int8ub)) + field(Bytes(2))
    """
    # rs 与 py 都应：a=1, b=b"\\x01\\x02"

def test_peek_returns_none_on_failure():
    # Peek(Int32ub) parse 1 字节 → inner 失败 → Peek 返回 None
    ...

# ---------------- RawCopy ----------------
def test_rawcopy_parse_returns_dict():
    # RawCopy(Int8ub).parse(b"\\xff") → Container(data=b"\\xff", value=255, ...)
    ...

def test_rawcopy_build_from_data():
    # RawCopy(Int8ub).build({"data": b"\\xff"}) → b"\\xff"
    ...

def test_rawcopy_build_from_value():
    # RawCopy(Int8ub).build({"value": 255}) → b"\\xff"
    ...

# ---------------- Rebuild ----------------
def test_rebuild_in_struct():
    # Struct("count" / Rebuild(Byte, len_(this.items)), "items" / Byte[3])
    # build({"items": [1,2,3]}) → b"\\x03\\x01\\x02\\x03"
    ...

# ---------------- AdapterCallback（用户面）----------------
def test_user_adapter_in_struct():
    # class HexAdapter(Adapter): ...
    # @dataclass class P(StructMixin): value: str = field(HexAdapter(Int8ub))
    # parse(b"\\x10") → P(value="0x10")
    # build(P(value="0x10")) → b"\\x10"
    ...
```

**Parity 重点关注**：
- RC-build-1（build 不返回 Container.data）—— **rs 与 py 行为差异，parity 测试应跳过 build 返回值断言**
- PE-3（ExplicitError）—— Phase 6 不区分，parity 测试不包含 ExplicitError 场景
- AC-FFI（用户面 Adapter 嵌入 Struct 2 次 FFI）—— 性能 parity 不设硬门禁，仅功能 parity

---

## 12. 与总设计文档的关系

本设计**不修改** `docs/design/基础设施/架构设计.md`：
- Construct trait 签名不变（parse/build/sizeof 三方法）
- Node enum 扩展在 nodes/mod.rs 内部，不影响架构层
- compile_schema 签名不变（仅 build_node_from_descriptor 内部加分支）

**可能修改的 ADR**：
- 若 PM 采用 D-1 选项 A（AdapterCallbackNode），考虑新增 ADR-021《用户面 Adapter Python 层化 + 最小钩子》记录此决策（PM 决策 2 的工程化落地）
- 不修改现有 ADR-001~ADR-020

---

> **设计文档完成时间**：2026-07-30
> **下一步**：PM 决策 D-1/D-2/D-3 → REV 设计检视 → DEV 实施

