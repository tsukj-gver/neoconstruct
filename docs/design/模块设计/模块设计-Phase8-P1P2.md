---
id: DESIGN-Phase8-P1P2
status: active
revision: v2
phase: "8"
depends_on:
  - ANALYSIS-phase8-pre
  - DESIGN-Phase8-P0
  - ADR-022
  - ADR-014
  - ADR-006
  - ADR-004
  - ADR-012
  - DESIGN-phase6-adapter
supersedes: []
superseded_by: []
last_updated: 2026-07-31
---

# 模块设计：Phase 8 P1+P2 批次（剩余构造器：Enum 家族 / 验证 / Union / Sequence / ProcessXor / NamedTuple / Timestamp）

> **角色**：ARCH
> > **任务**：`8.2 / 8.3 / 8.6 / 8.7 / 8.11 / 8.12 [P1+P2 批次合并详细设计]`
> **状态**：DESIGNING
> **创建时间**：2026-07-31
>
> **承接**：本设计延续 `模块设计-Phase8-P0.md`（P0 已 ACCEPTED，9 Node）。PM 决策 D-1~D-7 + D-P0-1~5 全部已确认。本设计覆盖 P1+P2 剩余 12 个构造器。
>
> **修订记录**：
> - **v2（2026-07-31，REV 驳回修正，TRACE-phase8-P1P2-REV）**：
>   - **F-1（驳回级）**：§6.2.3 Timestamp `import arrow` 从 `_macros.py` 顶部移入 `Timestamp` 函数体内（对齐 Python core.py L3478），避免 arrow 缺失时破坏共享模块 `_macros.py`（含 P0 已验收的 `AlignedStruct`，回归风险）。
>   - **C-1**：§4.5 注释 + §4.8 SQ-7 + §7.5 补充 Sequence RO 字段 build 的 parity 差异标注（construct-rs RO 字段不消费 list 位置；Python core.py L2410 对所有 subcons 都 `next(objiter)`）。
>   - **C-2**：§8.1 AD-P1-5 修正理由 + §6.1.6 NT-10 + §7.5 补充 NamedTuple over Struct 的 parity 差异（Python `factory(**obj)` 传所有字段且多余字段报 TypeError；construct-rs 只传 tuplefields，忽略多余字段，更宽松）。
>   - **C-3**：§3.4 build parity 说明 + §3.7 UN-14/UN-15 补充 Union build 的 `context.update(obj)` 与 `flagbuildnone` 处理。
>   - C-4（Mapping TypeError 捕获）/C-5（Timestamp subcon 类型检查）/C-6（Enum bool 边界）转 CODING 阶段由 DEV 实施时补充边界条件，不阻塞设计通过。
> - **v1（2026-07-31，初版）**：12 构造器首批设计。

---

## 0. P1+P2 批次范围与共通设计原则

### 0.1 批次范围（ARCH 自行确定，基于 8.0 分析报告 §3 + PM 已 ACCEPTED 的 P0 重整）

P0 已吸收原分析报告的 8.1 / 8.4 / 8.5 / 8.8 / 8.9 / 8.10（含 Aligned/Hex/HexDump/Checksum/Terminated/Probe + CancelParsing）。剩余 12 构造器按 P1/P2 两批次：

| 批次 | 子任务 | 构造器 | 性质 | 新增 Node 变体 | Python 层 |
|------|--------|--------|------|---------------|-----------|
| **P1** | 8.2 | Enum / FlagsEnum / Mapping | 内置 Rust Node（dict 物化） | 3 | — |
| **P1** | 8.3 | OneOf / NoneOf + Validator | 内置 Rust Node + Python 基类 | 2 | Validator 类（复用 AdapterCallbackNode） |
| **P1** | 8.6 | Union | 内置 Rust Node（多视角 seek） | 1 | — |
| **P1** | 8.7 | Sequence | 内置 Rust Node（list sink） | 1 | — |
| **P1** | 8.12 | ProcessXor / ProcessRotateLeft | 内置 Rust Node（字节变换） | 2 | — |
| **P2** | 8.11 | NamedTuple / Timestamp | Rust Node + Python macro | 1 | Timestamp macro（arrow 依赖） |

**Node enum 净增**：10 个新变体（当前 48 → 58）。
**Python 层新增**：Validator 基类（`_adapters.py`）+ Timestamp macro（`_macros.py`）。

**Node 变体命名约定**：与 P0 一致，`pub enum Node` 中变体名用构造器名（如 `Enum`/`FlagsEnum`/`Mapping`/`OneOf`/`NoneOf`/`Union`/`Sequence`/`NamedTuple`/`ProcessXor`/`ProcessRotateLeft`），struct 类型加 `Node` 后缀（`EnumNode` 等）。

### 0.2 §0 合规判据（继承 P0 §0.2，本批次重点应用）

PM 已接受 ARCH 8.0 分析报告确立的关键判据（P0 §0.2 重申）：

> **"FFI crossing（调用用户定义的 Python 代码）≠ C API 操作（直接操作 CPython 内置类型）"**

本批次该判据的**核心应用场景**是 Enum/FlagsEnum/Mapping/OneOf/NoneOf/NamedTuple——这 6 个构造器持有编译期物化的 `Py<PyDict>` / `Py<PyFrozenSet>` / `Py<PyType>`，运行时通过 C API（`PyDict_GetItem` / `PySet_Contains` / `PyType_Call`）查询或构造。即使内部触发 `__hash__`/`__eq__`，对 int/str/bytes/frozenset 等内置类型这些是 C 级实现，**不计额外 FFI**（§0 #1 明文允许的"Rust 内部通过 CPython C API 直接操作 Python 对象"）。

**边界**：若用户传入自定义子类（如 `class MySet(set)` 重写 `__contains__`），`__contains__` 是用户代码会触发额外 Python 调用——但这是用户主动引入的边缘场景，不破坏"常见场景 1 FFI"的合规论证（与 Computed 用户写复杂表达式 / Hex 用户传自定义 Adapter 同脉络）。

### 0.3 共通模式（L-04 对策：模式复用，不重新决策）

| 模式 | 适用 | 已有先例（参考实现位置） |
|------|------|------------------------|
| **编译期物化 `Py<PyAny>` 容器** | Enum / FlagsEnum / Mapping / OneOf / NoneOf | P0 HexNode（`Py<PyType>` 显示类物化）；SwitchNode FieldRef（`Py<PyAny>` 字段引用） |
| **Subconstruct 包装模式** | ProcessXor / ProcessRotateLeft / NamedTuple | P0 ConstNode/AlignedNode；Phase 6.3 SubconstructNode/PeekNode/RawCopyNode |
| **子流构造 + 字节变换** | ProcessXor / ProcessRotateLeft | TransformNode（Phase 3.3：`ParseStream::new(&transformed)` + `inner.parse(sub_stream)`） |
| **context nesting（new_child）** | Sequence / Union | FocusedSeqNode（Phase 7.1：`Context::new_child(ctx, py)?` + `set_field_at`） |
| **StopField 哨兵捕获** | Sequence | StructNode / GreedyRangeNode（`Err(ConstructError::StopField{..}) => break`） |
| **Python 层 Adapter（ADR-022）** | Validator | AdapterCallbackNode（Phase 6.3） |
| **Python macro（make_dataclass）** | Timestamp | P0 AlignedStruct macro（`_macros.py`） |

### 0.4 共通约束（继承 P0 §0.4）

- **表达式系统不接 lambda/callable**（ADR-006/014）：ProcessXor.padfunc / ProcessRotateLeft.amount/group 凡需"动态值"的位置一律编译为 `ExprProgram`，不接收 Python callable。Union.parsefrom 仅支持 `{None | 常量 index | 常量 name | ExprProgram(求值→index)}`，不支持 context lambda（D-5 已确认）。这是与 Python 原版的已知 parity 差异。
- **Rust 编码红线**：禁止 `unwrap()`/`expect()` 在非测试代码；禁止 `TODO`/`FIXME`；禁止硬编码魔法数字；所有 `pub` 项必须有 `///` 文档注释；parse/build 对称。
- **§0 #2 合规**：禁止在 Rust 侧引入中间数据类型。所有"对象"直接以 `Py<PyAny>` 持有。

### 0.5 实施顺序建议（基于依赖）

```
P1（核心 Adapter + 复合，可并行 5 子任务）：
  8.2 (Enum/FlagsEnum/Mapping)      ← 编译期物化 dict 模式，跑通 §0.2 判据
  8.3 (OneOf/NoneOf/Validator)      ← frozenset 物化 + Python 基类
  8.7 (Sequence)                    ← PyList sink + context nesting（参考 FocusedSeq）
  8.6 (Union)                       ← 多视角 seek（依赖 Sequence 的 context nesting 经验）
  8.12 (ProcessXor/ProcessRotateLeft) ← 子流 + 位运算（参考 Transform）
      ↓
P2（串行，依赖 P1 经验）：
  8.11 (NamedTuple/Timestamp)       ← NamedTuple 依赖 Struct/Sequence 已稳定；Timestamp Python macro
```

---

## 1. 子任务 8.2：Enum / FlagsEnum / Mapping

### 1.1 Python 参考实现摘要

| 构造器 | Python 行号 | 核心语义 |
|--------|-----------|---------|
| `Enum(subcon, *merge, **mapping)` | core.py L1920-2005 | parse：subcon.parse → `decmapping[obj]`（KeyError → 返回 `EnumInteger(obj)`，int 子类，**不报错**）；build：obj is int → 用 obj；否则 `encmapping[obj]`（KeyError → MappingError）。`__getattr__(name)` 返回 label。`EnumIntegerString` 是 str 子类（带 `.intvalue`），用作 decmapping 的值；`EnumInteger` 是 int 子类，用作无映射 fallback |
| `FlagsEnum(subcon, *merge, **flags)` | core.py L2018-2109 | parse：subcon.parse(int) → `Container({_flagsenum=True, name: (obj & value == value) for each flag})`；build：int → 用 obj；str → `split("\|")` 后 OR 每个 name 的 value；dict → OR where value True 且 name 不以 `_` 开头。KeyError → MappingError |
| `Mapping(subcon, mapping)` | core.py L2112-2156 | parse：`decmapping[obj]`（KeyError/TypeError → MappingError，**报错**，与 Enum 不同）；build：`encmapping[obj]`（KeyError/TypeError → MappingError）。key/value 可为任意 hashable 对象 |

**关键差异（Enum vs Mapping）**：Enum 无映射时不报错（返回 EnumInteger）；Mapping 无映射时报 MappingError。Enum 的 key 固定是 int（subcon 返回值），value 是 EnumIntegerString；Mapping 的 key/value 任意。

**EnumInteger / EnumIntegerString 内部类**（core.py L1899-1917）：
- `EnumInteger(int): pass` — int 子类，无额外方法（仅类型标记）
- `EnumIntegerString(str)` — str 子类，`__int__()` 返回 `.intvalue`；`@staticmethod new(intvalue, stringvalue)` 构造

### 1.2 关键设计决策：Enum/FlagsEnum/Mapping 走 Rust Node（§0.2 判据应用）

**ARCH 决策（已确认，详见 ADR-022 §0.2 + 8.0 分析报告 §0.2）**：Enum/FlagsEnum/Mapping 实现为 **Rust Node 变体**，不走 AdapterCallbackNode 路径。

**理由**：
1. 这三者是 construct 核心库内置 Adapter（非用户自定义），与 Hex/HexDump（P0 已落地为 Rust Node）同档
2. parse/build 逻辑是"dict lookup + Python 类型构造"，全程 C API 操作（`PyDict_GetItem` / `PyDict_Contains`），**不算额外 FFI**（§0.2 判据 2）
3. dict 在编译期一次性物化为 `Py<PyDict>`，运行时零 Python 回调
4. 避免 AdapterCallbackNode 的 Rust→Python 回调开销（~200-300ns/字段）

**判据应用对照**：
- `PyDict_GetItem(dict, key)` 对 int/str key：内部走 C 级 `__hash__`/`__eq__`，**不计 FFI**
- `EnumIntegerString.new(v, label)` 调用：`EnumIntegerString` 是 construct-rs Python 端内部类，`new` 是 staticmethod，调用走 C API（类型构造），**不计 FFI**（与 P0 HexNode 调 `HexDisplayedInteger.new` 同脉络）
- `PyDict_SetItem`（FlagsEnum 构造 Container 时）：C API 写入，**不计 FFI**

### 1.3 Rust Node 设计

#### 1.3.1 EnumNode

```rust
/// 枚举映射节点：subcon 整数 ↔ label 字符串（int-convertible）。
///
/// 对应 Python construct `Enum(subcon, *merge, **mapping)`（core.py L1920）。
/// 在 construct-rs 中实现为 Rust Node（非 AdapterCallbackNode，详见设计 §1.2）。
///
/// # 三方法行为
///
/// - parse：inner.parse → `decmapping.get(obj)`（C API PyDict_GetItem）
///   - Some(label) → 返回 label（EnumIntegerString 实例，str 子类）
///   - None → 返回 `EnumInteger(obj)`（int 子类，**不报错**，对齐 Python L1982）
/// - build：obj is int → inner.build(obj)；否则 `encmapping.get(obj)`
///   - Some(v) → inner.build(v)
///   - None → MappingError（"building failed, no mapping for {obj}"）
/// - sizeof：转发 inner.sizeof
///
/// # 编译期物化（§0.2 判据）
///
/// `decmapping` / `encmapping` 在编译期由 Python `EnumDescriptor.__init__` 构造为
/// 普通 Python dict（key/value 是 EnumIntegerString / int），再由 compile.rs 物化为
/// `Py<PyDict>` 引用存入 Node。运行时通过 `PyDict_GetItem` 查询（C API，不计 FFI）。
///
/// # `__getattr__` 语义
///
/// Python `d.one` → label 是描述符层职责（`EnumDescriptor.__getattr__`），Rust Node
/// 不涉及。用户面 `Enum(Byte, one=1).one` 在 Python 层返回 EnumIntegerString。
#[derive(Debug)]
pub struct EnumNode {
    inner: Box<Node>,
    /// 解码映射：int（raw value）→ EnumIntegerString（label）。
    /// 编译期物化。
    decmapping: Py<PyDict>,
    /// 编码映射：EnumIntegerString/str（label）→ int（raw value）。
    /// key 是 str 子类，PyDict_GetItem 用 str 相等匹配。编译期物化。
    encmapping: Py<PyDict>,
    /// EnumInteger 类引用（用于无映射 fallback，构造 int 子类实例）。
    /// 从 `construct._internals` 加载（construct-rs port）。
    enum_integer_cls: Py<PyType>,
}

impl EnumNode {
    pub fn new(
        inner: Node,
        decmapping: Py<PyDict>,
        encmapping: Py<PyDict>,
        enum_integer_cls: Py<PyType>,
    ) -> Self {
        Self { inner: Box::new(inner), decmapping, encmapping, enum_integer_cls }
    }
    pub fn inner(&self) -> &Node { &self.inner }
    pub fn decmapping(&self) -> &Py<PyDict> { &self.decmapping }
    pub fn encmapping(&self) -> &Py<PyDict> { &self.encmapping }
}
```

**`has_expressions` 集成**：`Node::Enum(e) => e.inner().has_expressions()`（Enum 不引入新表达式，与 Subconstruct 同模式）。

#### 1.3.2 FlagsEnumNode

```rust
/// 标志位枚举节点：subcon 整数 → Container（每 flag 一 bool）。
///
/// 对应 Python construct `FlagsEnum(subcon, *merge, **flags)`（core.py L2018）。
///
/// # 三方法行为
///
/// - parse：inner.parse(int) → 遍历 flags 构造 PyDict（每项 `set_item(name, obj & value == value)`）
///   并设置 `_flagsenum=True` 标志（特殊 key）。返回该 dict
/// - build：按 obj 类型分支：
///   - int → inner.build(obj)
///   - str → split("|") 后查 encmapping 累加 OR（key 缺失 → MappingError）
///   - dict → 遍历 items，name 不以 "_" 开头且 value truthy 时累加 OR
/// - sizeof：转发 inner.sizeof
///
/// # 编译期物化
///
/// `flags` 编译期物化为 `Vec<(Py<PyString>, i64)>`（name + value），
/// 减少 PyDict 遍历开销（Rust 端直接遍历 Vec）。`encmapping` 同 EnumNode（str→int）。
#[derive(Debug)]
pub struct FlagsEnumNode {
    inner: Box<Node>,
    /// flags 列表：编译期物化为 Rust Vec（避免运行时 PyDict 遍历）。
    /// (label_name, raw_value)。name 是 Py<PyString>（用于 parse 构造 dict key）。
    flags: Vec<(Py<PyString>, i64)>,
    /// 编码映射：label(str) → value(int)。用于 build 的 str/dict 分支。
    encmapping: Py<PyDict>,
}

impl FlagsEnumNode {
    pub fn new(inner: Node, flags: Vec<(Py<PyString>, i64)>, encmapping: Py<PyDict>) -> Self {
        Self { inner: Box::new(inner), flags, encmapping }
    }
    pub fn inner(&self) -> &Node { &self.inner }
    pub fn flags(&self) -> &[(Py<PyString>, i64)] { &self.flags }
}
```

**`has_expressions` 集成**：`Node::FlagsEnum(f) => f.inner().has_expressions()`。

#### 1.3.3 MappingNode

```rust
/// 通用对象映射节点：subcon 对象 ↔ 任意对象（key/value 可为任意 hashable）。
///
/// 对应 Python construct `Mapping(subcon, mapping)`（core.py L2112）。
/// 与 EnumNode 结构同，但 key/value 任意（非限定 int/str），且**无映射时报错**
/// （与 EnumNode 的"无映射返回 EnumInteger"不同）。
///
/// # 三方法行为
///
/// - parse：inner.parse → `decmapping.get(obj)`
///   - Some(k) → 返回 k
///   - None → MappingError（"parsing failed, no decoding mapping for {obj}"）
/// - build：`encmapping.get(obj)`
///   - Some(v) → inner.build(v)
///   - None → MappingError（"building failed, no encoding mapping for {obj}"）
/// - sizeof：转发 inner.sizeof
///
/// # §0.2 判据边界
///
/// 若 key 是自定义对象（重写 `__hash__`/`__eq__`），查询触发用户 Python 代码——
/// 边缘场景，不破坏常见场景（int/str/bytes key）的 1 FFI 合规论证。
#[derive(Debug)]
pub struct MappingNode {
    inner: Box<Node>,
    /// 解码映射：raw value → mapped object。编译期物化。
    decmapping: Py<PyDict>,
    /// 编码映射：mapped object → raw value。编译期物化。
    encmapping: Py<PyDict>,
}

impl MappingNode {
    pub fn new(inner: Node, decmapping: Py<PyDict>, encmapping: Py<PyDict>) -> Self {
        Self { inner: Box::new(inner), decmapping, encmapping }
    }
    pub fn inner(&self) -> &Node { &self.inner }
    pub fn decmapping(&self) -> &Py<PyDict> { &self.decmapping }
    pub fn encmapping(&self) -> &Py<PyDict> { &self.encmapping }
}
```

**`has_expressions` 集成**：`Node::Mapping(m) => m.inner().has_expressions()`。

### 1.4 Construct impl（关键路径摘要）

```rust
impl Construct for EnumNode {
    fn parse<'py>(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
        let obj = self.inner.parse(py, stream, ctx, path)?;
        let label_opt = self.decmapping.bind(py).get_item(obj.bind(py))?;
        match label_opt {
            Some(label) => Ok(label.unbind()),
            None => {
                // 无映射 fallback：构造 EnumInteger(obj)（int 子类，C API 类型构造）
                let fallback = self.enum_integer_cls.bind(py).call1((obj.bind(py),))?;
                Ok(fallback.unbind())
            }
        }
    }
    fn build(&self, py, obj, stream, ctx, path) -> Result<(), ConstructError> {
        let raw: Py<PyAny> = if obj.is_instance_of::<PyLong>(py) {
            obj.clone().unbind()  // int 直接用
        } else {
            // label → raw value（encmapping 查询）
            match self.encmapping.bind(py).get_item(obj)? {
                Some(v) => v.unbind(),
                None => return Err(ConstructError::Mapping {
                    message: format!("building failed, no mapping for {:?}", obj.repr()?),
                    path: path.to_string(),
                }),
            }
        };
        self.inner.build(py, raw.bind(py), stream, ctx, path)
    }
    fn sizeof(&self, ctx) -> Result<usize, ConstructError> { self.inner.sizeof(ctx) }
}

impl Construct for FlagsEnumNode {
    fn parse<'py>(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
        let obj = self.inner.parse(py, stream, ctx, path)?;
        let int_val: i64 = obj.bind(py).extract()?;  // subcon 必须返回 int
        let result = PyDict::new_bound(py);
        // _flagsenum=True 标志（对齐 Python L2072）
        result.set_item("_flagsenum", true)?;
        for (name, value) in &self.flags {
            let set = (int_val & value) == *value;
            result.set_item(name.bind(py), set)?;
        }
        Ok(result.into_py(py))
    }
    fn build(&self, py, obj, stream, ctx, path) -> Result<(), ConstructError> {
        let int_val: i64 = if obj.is_instance_of::<PyLong>(py) {
            obj.extract()?  // int 直接用
        } else if obj.is_instance_of::<PyString>(py) {
            // str → split("|") 后查 encmapping OR
            let s: String = obj.extract()?;
            let mut acc: i64 = 0;
            for name in s.split('|') {
                let trimmed = name.trim();
                if trimmed.is_empty() { continue; }
                match self.encmapping.bind(py).get_item(trimmed)? {
                    Some(v) => acc |= v.extract::<i64>()?,
                    None => return Err(ConstructError::Mapping {
                        message: format!("building failed, unknown label: {:?}", obj.repr()?),
                        path: path.to_string(),
                    }),
                }
            }
            acc
        } else if obj.is_instance_of::<PyDict>(py) {
            // dict → 遍历 items，name 不以 "_" 开头且 value truthy 时 OR
            let d = obj.downcast::<PyDict>(py)?;
            let mut acc: i64 = 0;
            for (k, v) in d.iter() {
                let name_str: String = k.extract()?;
                if name_str.starts_with('_') { continue; }
                let truthy = v.is_truthy()?;
                if truthy {
                    match self.encmapping.bind(py).get_item(k)? {
                        Some(val) => acc |= val.extract::<i64>()?,
                        None => return Err(ConstructError::Mapping {
                            message: format!("building failed, unknown label: {:?}", k.repr()?),
                            path: path.to_string(),
                        }),
                    }
                }
            }
            acc
        } else {
            return Err(ConstructError::Mapping {
                message: format!("building failed, unknown object: {:?}", obj.repr()?),
                path: path.to_string(),
            });
        };
        let int_obj = int_val.into_py(py);
        self.inner.build(py, int_obj.bind(py), stream, ctx, path)
    }
    fn sizeof(&self, ctx) -> Result<usize, ConstructError> { self.inner.sizeof(ctx) }
}

impl Construct for MappingNode {
    fn parse<'py>(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
        let obj = self.inner.parse(py, stream, ctx, path)?;
        match self.decmapping.bind(py).get_item(obj.bind(py))? {
            Some(k) => Ok(k.unbind()),
            None => Err(ConstructError::Mapping {
                message: format!("parsing failed, no decoding mapping for {:?}", obj.bind(py).repr()?),
                path: path.to_string(),
            }),
        }
    }
    fn build(&self, py, obj, stream, ctx, path) -> Result<(), ConstructError> {
        match self.encmapping.bind(py).get_item(obj)? {
            Some(v) => self.inner.build(py, v.bind(py), stream, ctx, path),
            None => Err(ConstructError::Mapping {
                message: format!("building failed, no encoding mapping for {:?}", obj.repr()?),
                path: path.to_string(),
            }),
        }
    }
    fn sizeof(&self, ctx) -> Result<usize, ConstructError> { self.inner.sizeof(ctx) }
}
```

**注**：`obj.is_instance_of::<PyLong>` 是 pyo3 内部走 `PyObject_IsInstance`（对内置类型 fast-path，~10ns）。FlagsEnum 的 str split 用 Rust `str::split`（零拷贝，比 Python `str.split` 快）。dict 遍历用 `PyDict::iter`（C API）。

### 1.5 §0 原则对照表（L-01 对策）

| §0 原则 | EnumNode | FlagsEnumNode | MappingNode |
|---------|----------|---------------|-------------|
| #1 一次 FFI | ✅ 严格 1 次。inner.parse/build 在 Rust 内；`PyDict_GetItem` / `PyDict::set_item` / `PyType::call1` 全是 C API（§0.2 判据 2）。int/str key 的 `__hash__`/`__eq__` 是 C 级实现 | ✅ 同 Enum；PyDict 构造是 C API | ✅ 同 Enum（自定义 key 边缘场景见 §1.3.3） |
| #2 无中间表示层 | ✅ `Py<PyDict>` 持有 Python dict 本身；EnumIntegerString 是最终用户对象，非 Rust 中间类型 | ✅ PyDict 是 Python 对象本身 | ✅ |
| #3 输入输出无 trait 抽象 | ✅ 仅 `Box<Node>` + enum 分派 | ✅ | ✅ |
| #4 pyo3 核心依赖 | ✅ 全程 pyo3 API（get_item/set_item/call1） | ✅ | ✅ |
| #5 mashumaro API | ✅ Enum 是字段描述符 | ✅ | ✅ |
| #6 enum_dispatch | ✅ 3 个新 Node 加入 enum | ✅ | ✅ |
| #7 Result + path | ✅ ConstructError::Mapping 携带 path | ✅ | ✅ |
| #8 Stream 纯 Rust | ✅ Enum/Mapping 透传 inner.stream；FlagsEnum 不操作 stream | ✅ | ✅ |

### 1.6 性能假设（L-02/L-05 对策）

#### 1.6.1 瓶颈识别（量化数据 + 来源）

| Node | 主要瓶颈 | 量化来源 |
|------|---------|---------|
| Enum | inner.parse + 1 次 `PyDict_GetItem`（int key，~50-80ns）+ 可能 1 次 EnumInteger 构造（~80ns，仅 fallback 路径） | pyo3 benchmark：PyDict_GetItem ~50-80ns（int key hash+eq 全 C 级） |
| FlagsEnum | inner.parse + 1 次 extract i64 + N 次 `PyDict_SetItem`（~40ns × flag 数） | PyDict_SetItem ~40ns/项 |
| Mapping | inner.parse + 1 次 `PyDict_GetItem`（~50-100ns，key 类型相关） | 同 Enum |

#### 1.6.2 可证伪预测（覆盖所有 FFI/拷贝/转换来源）

**嵌入 Struct 字段，与 Python construct 2.10.70 对比**：

- **Enum(Byte, one=1, two=2)**：加速比 ≥6x（vs Python：Adapter._decode 调度 + Python dict `[]` + EnumIntegerString.new Python 调用）。**风险**：若实测 <3x，说明 CPython dict 查找与 Python 路径差距小（dict 操作本身已是 C 级）——但 Python 路径额外有 Adapter 方法调度开销，应能拉开
- **FlagsEnum(Byte, one=1, two=2, four=4, eight=8)**：加速比 ≥4x（4 flag dict 构造 + 4 次 set_item）。**风险**：flag 数多时 set_item 累积开销接近 Python
- **Mapping(Byte, {obj:0})**：加速比 ≥6x（同 Enum，但无 fallback 路径）

**FFI 来源清单**（L-05 对策）：
- StructMixin.parse 入口（1 次，共享）
- inner.parse（Rust 内）
- PyDict_GetItem / set_item / call1（C API，不计 FFI，§0.2 判据 2）
- 无 Rust→Python 回调（严格 1 次 FFI）

#### 1.6.3 失败模式（可证伪）

- 若 Enum 实测 <3x：可能 EnumIntegerString 的 `new` staticmethod 调用是 Python 层（非纯 C）。**防御**：编译期物化时把 decmapping 的值预构造为 EnumIntegerString 实例存入 dict，parse 时直接 `get_item` 返回（不调 `new`）——这正是本设计的做法（§1.3.1 编译期物化）。运行时仅 fallback 路径调 EnumInteger 构造
- 若 FlagsEnum 实测 <3x：PyDict 构造 + 多次 set_item 开销大。**防御**：bench 覆盖不同 flag 数（2/4/8/16），识别 sweet spot

### 1.7 边界条件清单

| 编号 | 场景 | 预期行为 |
|------|------|---------|
| EN-1 | `Enum(Byte, one=1, two=2).parse(b'\x01')` | 返回 EnumIntegerString('one')（str 子类，`int(obj)` → 1） |
| EN-2 | `Enum(Byte, one=1).parse(b'\xff')` | 返回 EnumInteger(255)（int 子类，无映射 fallback） |
| EN-3 | `Enum(Byte, one=1).build('one')` | 输出 b'\x01'（label → encmapping → 1） |
| EN-4 | `Enum(Byte, one=1).build(1)` | 输出 b'\x01'（int 直接用，对齐 Python L1986） |
| EN-5 | `Enum(Byte, one=1).build('unknown')` | ConstructError::Mapping（"building failed, no mapping for 'unknown'"） |
| EN-6 | `Enum(Byte, one=1).build(99)` | 输出 b'\x63'（int 直接用，即使 99 不在 mapping，对齐 Python L1987 `isinstance(obj,int) return obj`） |
| EN-7 | `Enum(Byte, one=1).sizeof()` | 返回 1（转发 inner） |
| EN-8 | `EnumDescriptor.__getattr__('one')` | Python 层返回 EnumIntegerString('one')（描述符职责，Rust 不涉及） |
| EN-9 | Enum 合并 enum.IntEnum（`Enum(Byte, MyIntEnum)`） | 编译期展开为 mapping（Python 描述符层处理，Rust 收到最终 mapping dict） |
| FE-1 | `FlagsEnum(Byte, one=1, two=2, four=4, eight=8).parse(b'\x03')` | 返回 dict{_flagsenum=True, one=True, two=True, four=False, eight=False} |
| FE-2 | `FlagsEnum(Byte, one=1, two=2).build(dict(one=True, two=True))` | 输出 b'\x03'（OR：1|2=3） |
| FE-3 | `FlagsEnum(Byte, one=1, two=2).build('one\|two')` | 输出 b'\x03'（str split 后 OR） |
| FE-4 | `FlagsEnum(Byte, one=1, two=2).build(3)` | 输出 b'\x03'（int 直接用） |
| FE-5 | `FlagsEnum(Byte, one=1).build(dict(one=True, _flagsenum=True))` | 输出 b'\x01'（_flagsenum 以 _ 开头，跳过，对齐 Python L2091） |
| FE-6 | `FlagsEnum(Byte, one=1).build('unknown')` | ConstructError::Mapping（"building failed, unknown label"） |
| FE-7 | `FlagsEnum(Byte, one=1).build(dict(unknown=True))` | ConstructError::Mapping |
| FE-8 | `FlagsEnum(Byte, one=1).build(None)` | ConstructError::Mapping（None 非 int/str/dict） |
| FE-9 | `FlagsEnum(Int16ub, one=1).parse(...)` inner 返回非 int | ConstructError::Generic（extract i64 失败，对齐 Python "Can raise arbitrary exceptions"） |
| MP-1 | `Mapping(Byte, {x:0}).parse(b'\x00')` | 返回 x（任意 hashable 对象） |
| MP-2 | `Mapping(Byte, {x:0}).parse(b'\xff')` | ConstructError::Mapping（无解码映射，**报错**，与 EN-2 不同） |
| MP-3 | `Mapping(Byte, {x:0}).build(x)` | 输出 b'\x00' |
| MP-4 | `Mapping(Byte, {x:0}).build(unknown)` | ConstructError::Mapping（无编码映射） |
| MP-5 | `Mapping(Byte, {1:'a', 2:'b'}).build('a')` | 输出 b'\x01'（encmapping: 'a'→1） |

### 1.8 DEV 实施清单

| 文件 | 内容 | 行数估 |
|------|------|-------|
| `nodes/enum_node.rs`（命名避开 Rust 关键字 `enum`） | EnumNode + impl + 单元测试 | ~320 |
| `nodes/flags_enum.rs` | FlagsEnumNode + impl + 单元测试 | ~350 |
| `nodes/mapping.rs` | MappingNode + impl + 单元测试（可与 enum_node 共享 dict-lookup 辅助） | ~220 |
| `nodes/mod.rs` | 新增 3 个 mod/enum 变体 + has_expressions 分支 | ~25 增量 |
| `compile.rs` | 新增 3 个 build_*_node 分支（mapping dict 物化 + EnumInteger/EnumIntegerString 类加载） | ~100 增量 |
| `error.rs` | 新增 ConstructError::Mapping 变体 + ExceptionClasses.mapping_error + Python 类映射 | ~30 增量 |
| `python/construct/_internals.py`（新建或追加） | EnumInteger / EnumIntegerString port（core.py L1899-1917） | ~30 |
| `_descriptors.py` | EnumDescriptor / FlagsEnumDescriptor / MappingDescriptor + 工厂函数（含 `*merge` enum 展开） | ~120 增量 |
| `_errors.py` | MappingError Python 类（对齐 core.py L102） | ~10 增量 |
| `__init__.py` | 导出 Enum / FlagsEnum / Mapping / MappingError | ~10 增量 |

**Cargo.toml 不需新增依赖**（PyDict C API 已有）。

**Python 端 EnumInteger/EnumIntegerString port 说明**：这两个内部类是纯 Python（无 Rust 等价物需求），直接复制 core.py L1899-1917 到 `construct-rs/python/construct/_internals.py`。编译期 Rust 端通过 `py.import_bound("construct._internals")` 加载 `EnumInteger` 类物化为 `Py<PyType>`。

### 1.9 Parity 测试模板

```python
def test_enum_parse_returns_label():
    # Enum(Byte, one=1, two=2).parse(b'\x01') → 'one'（EnumIntegerString）
    # int(obj) == 1；str(obj) == 'one'
    assert_parity_case("""
    from construct import Enum, Byte
    obj = Enum(Byte, one=1, two=2).parse(b'\\x01')
    """, check_only=["int(obj)==1", "str(obj)=='one'"])

def test_enum_parse_unmapped_returns_enuminteger():
    # Enum(Byte, one=1).parse(b'\xff') → EnumInteger(255)
    # int(obj) == 255
    assert_parity_case("""
    from construct import Enum, Byte
    obj = Enum(Byte, one=1).parse(b'\\xff')
    """, check_only=["int(obj)==255"])

def test_enum_build_from_label():
    # Enum(Byte, one=1).build('one') → b'\x01'
    assert_parity_case("""
    from construct import Enum, Byte
    out = Enum(Byte, one=1).build('one')
    """, expected_py=b'\\x01')

def test_enum_build_from_int_passthrough():
    # Enum(Byte, one=1).build(99) → b'\x63'（int 直接用）
    assert_parity_case("""
    from construct import Enum, Byte
    out = Enum(Byte, one=1).build(99)
    """, expected_py=b'\\x63')

def test_enum_build_unknown_label_raises():
    # Enum(Byte, one=1).build('unknown') → MappingError
    assert_parity_error("""
    from construct import Enum, Byte, MappingError
    try: Enum(Byte, one=1).build('unknown')
    except MappingError: pass
    """)

def test_flagsenum_parse_returns_flag_dict():
    # FlagsEnum(Byte, one=1, two=2).parse(b'\x03') → {one=True, two=True, _flagsenum=True}
    assert_parity_case("""
    from construct import FlagsEnum, Byte
    obj = FlagsEnum(Byte, one=1, two=2).parse(b'\\x03')
    """, check_only=["obj['one']==True", "obj['two']==True", "obj['_flagsenum']==True"])

def test_flagsenum_build_from_dict():
    # FlagsEnum(Byte, one=1, two=2).build(dict(one=True, two=True)) → b'\x03'
    assert_parity_case("""
    from construct import FlagsEnum, Byte
    out = FlagsEnum(Byte, one=1, two=2).build(dict(one=True, two=True))
    """, expected_py=b'\\x03')

def test_flagsenum_build_from_pipe_string():
    # FlagsEnum(Byte, one=1, two=2).build('one|two') → b'\x03'
    assert_parity_case("""
    from construct import FlagsEnum, Byte
    out = FlagsEnum(Byte, one=1, two=2).build('one|two')
    """, expected_py=b'\\x03')

def test_mapping_parse_returns_mapped():
    # x = object; Mapping(Byte, {x:0}).parse(b'\x00') → x
    assert_parity_case("""
    from construct import Mapping, Byte
    x = object
    obj = Mapping(Byte, {x:0}).parse(b'\\x00')
    """, check_only=["obj is x"])

def test_mapping_parse_unmapped_raises():
    # Mapping(Byte, {x:0}).parse(b'\xff') → MappingError（与 Enum 不同）
    assert_parity_error("""
    from construct import Mapping, Byte, MappingError
    try: Mapping(Byte, {object: 0}).parse(b'\\xff')
    except MappingError: pass
    """)
```

**Parity 重点**：
- EN-2 vs MP-2 的行为差异（Enum 无映射返回 EnumInteger；Mapping 无映射报错）
- FE-3 的 str split("|") OR 语义
- FE-5 的 `_` 前缀 key 跳过（`_flagsenum` 标志不被当 flag）

---

## 2. 子任务 8.3：OneOf / NoneOf + Validator

### 2.1 Python 参考实现摘要

| 构造器 | Python 行号 | 核心语义 |
|--------|-----------|---------|
| `OneOf(subcon, valids)` | core.py L6320-6339 | 函数：返回 `ExprValidator(subcon, lambda obj,ctx: obj in valids)`。parse：subcon.parse → 校验 `obj in valids`，False → ValidationError |
| `NoneOf(subcon, invalids)` | core.py L6342-6354 | 函数：返回 `ExprValidator(subcon, lambda obj,ctx: obj not in invalids)`。与 OneOf 取反 |
| `Validator(SymmetricAdapter)` | core.py L849-863 | **抽象基类**，`_decode` 调 `_validate` 返回 bool，False 抛 ValidationError。用户继承实现 `_validate(self, obj, context, path)` |
| `ExprValidator(Validator)` | core.py L6311-6318 | `__init__(subcon, validator)`：`self._validate = lambda obj,ctx,path: validator(obj,ctx)`。接受 lambda |

**OneOf/NoneOf 的双重身份**：在 Python 它们是返回 `ExprValidator` 实例的**函数**（非类）。在 construct-rs，OneOf/NoneOf 是核心库函数（非用户继承基类），其语义固定（"obj in valids"），可编译为 Rust Node + 编译期物化 `Py<PyFrozenSet>`。

### 2.2 OneOf/NoneOf：Rust Node 设计（§0.2 判据应用）

**设计决策**：OneOf/NoneOf 实现为 **Rust Node 变体**（`OneOfNode` / `NoneOfNode`，分开两个变体，语义清晰，与 Enum/Mapping 分开同脉络）。`valids`/`invalids` 编译期物化为 `Py<PyFrozenSet>`（list/set 输入统一转 frozenset，Python docstring L6324 "For performance, valids should be a set or frozenset"）。

**ARCH 选择分开两个 Node 变体而非合并 `MembershipCheckNode`**（分析报告 §2.1.5 备选）：(1) 语义清晰，错误消息明确（OneOf vs NoneOf）；(2) Node enum 习惯一构造器一变体；(3) 共享的 frozenset contains 逻辑通过私有辅助函数复用即可，不需合并类型。

#### 2.2.1 数据结构

```rust
/// 单值校验节点：parse/build 校验 inner 结果 ∈ valids（frozenset）。
///
/// 对应 Python construct `OneOf(subcon, valids)`（core.py L6320）。
/// 在 construct-rs 中实现为 Rust Node（valids 编译期物化为 frozenset）。
///
/// # 三方法行为
///
/// - parse：inner.parse → `valids.contains(obj)`（C API PySet_Contains）
///   - true → 返回 obj
///   - false → ValidationError（"object failed validation: {obj}"）
/// - build：对称——校验 obj ∈ valids 后 inner.build(obj)
/// - sizeof：转发 inner.sizeof
///
/// # §0.2 判据
///
/// `PySet_Contains` 对 int/str/bytes 元素走 C 级 `__hash__`/`__eq__`，不计额外 FFI。
/// frozenset 在编译期物化（list/set 输入统一转 frozenset）。
#[derive(Debug)]
pub struct OneOfNode {
    inner: Box<Node>,
    /// 合法值集合：编译期物化为 frozenset。
    valids: Py<PyFrozenSet>,
}

impl OneOfNode {
    pub fn new(inner: Node, valids: Py<PyFrozenSet>) -> Self {
        Self { inner: Box::new(inner), valids }
    }
    pub fn inner(&self) -> &Node { &self.inner }
    pub fn valids(&self) -> &Py<PyFrozenSet> { &self.valids }
}

/// 排除值校验节点：parse/build 校验 inner 结果 ∉ invalids。
///
/// 对应 Python construct `NoneOf(subcon, invalids)`（core.py L6342）。
/// 与 [`OneOfNode`] 结构同，校验取反。
#[derive(Debug)]
pub struct NoneOfNode {
    inner: Box<Node>,
    /// 非法值集合：编译期物化为 frozenset。
    invalids: Py<PyFrozenSet>,
}

impl NoneOfNode {
    pub fn new(inner: Node, invalids: Py<PyFrozenSet>) -> Self {
        Self { inner: Box::new(inner), invalids }
    }
    pub fn inner(&self) -> &Node { &self.inner }
    pub fn invalids(&self) -> &Py<PyFrozenSet> { &self.invalids }
}
```

**`has_expressions` 集成**：`Node::OneOf(o) => o.inner().has_expressions()`；`Node::NoneOf(n) => n.inner().has_expressions()`（与 Subconstruct 同模式）。

#### 2.2.2 Construct impl

```rust
impl Construct for OneOfNode {
    fn parse<'py>(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
        let obj = self.inner.parse(py, stream, ctx, path)?;
        let contains = self.valids.bind(py).contains(obj.bind(py))?;
        if !contains {
            return Err(ConstructError::Validation {
                message: format!("object failed validation: {:?}", obj.bind(py).repr()?),
                path: path.to_string(),
            });
        }
        Ok(obj)
    }
    fn build(&self, py, obj, stream, ctx, path) -> Result<(), ConstructError> {
        let contains = self.valids.bind(py).contains(obj)?;
        if !contains {
            return Err(ConstructError::Validation {
                message: format!("object failed validation: {:?}", obj.repr()?),
                path: path.to_string(),
            });
        }
        self.inner.build(py, obj, stream, ctx, path)
    }
    fn sizeof(&self, ctx) -> Result<usize, ConstructError> { self.inner.sizeof(ctx) }
}

impl Construct for NoneOfNode {
    // 与 OneOfNode 同结构，contains 取反（contains 为 true 时报错）
    fn parse<'py>(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
        let obj = self.inner.parse(py, stream, ctx, path)?;
        let contains = self.invalids.bind(py).contains(obj.bind(py))?;
        if contains {
            return Err(ConstructError::Validation {
                message: format!("object failed validation: {:?}", obj.bind(py).repr()?),
                path: path.to_string(),
            });
        }
        Ok(obj)
    }
    // build 对称（contains 取反），sizeof 转发
    // ...
}
```

**注**：`PyFrozenSet::contains` 是 pyo3 包装 `PySet_Contains`（C API）。对 int/str/bytes 元素，内部 `__hash__`/`__eq__` 是 C 级实现。

### 2.3 Validator：Python 层基类（ADR-022 应用）

**设计决策**：Validator 实现为 **Python 层用户面基类**（与 Adapter/SymmetricAdapter 同档，ADR-022 决策 2）。用户继承 `Validator` 实现 `_validate(self, obj, context, path)`。嵌入 Struct 时复用 `AdapterCallbackNode`（Phase 6.3 已实现）。

**理由**：
1. Validator 的 `_validate` 是**用户写的 Python 方法**，Rust 无法编译为 Node（ADR-022 §0 论证：用户域后处理）
2. PM 决策 D-4 已确认"Phase 8 不提供 ExprValidator 表达式版等价"——用户要表达式校验用 Check（8.1 已实现），要 Python 逻辑校验继承 Validator
3. 嵌入 Struct 时 2 次 FFI（parse 入口 + `_validate` 回调），用户主动选择已接受折衷（ADR-022）

#### 2.3.1 Python 层 Validator 基类

```python
# construct-rs/python/construct/_adapters.py（新增或追加）

class Validator(SymmetricAdapter):
    """Abstract class that validates a condition on the encoded/decoded object.

    对应 Python construct core.py:849 的 Validator。construct-rs 用户继承此类
    实现 _validate(self, obj, context, path) -> bool。

    嵌入 Struct 时走 AdapterCallbackNode（Phase 6.3），2 次 FFI（parse 入口 +
    _validate 回调）。用户主动选择继承 Validator = 接受性能折衷（ADR-022）。

    若需表达式版校验（无 Python 回调），使用 Check(expr)（Phase 8.1）。
    """

    def _decode(self, obj, context, path):
        if not self._validate(obj, context, path):
            raise ValidationError("object failed validation: %s" % (obj,), path=path)
        return obj

    def _validate(self, obj, context, path):
        raise NotImplementedError
```

**复用 AdapterCallbackNode**：编译期 `compile.rs` 识别 `Validator` 子类实例（与 `Adapter` 同检测逻辑），编译为 `AdapterCallbackNode { subcon, decode: _decode, encode: _encode, adapter_instance }`。`_encode` 由 `SymmetricAdapter` 提供为 `_decode` 别名（core.py L845）。

#### 2.3.2 ExprValidator（Python 层快捷构造，受限版）

PM 决策 D-4 明确"Phase 8 不提供表达式版 Validator 快捷构造"。OneOf/NoneOf 已是 Rust Node（不走 ExprValidator）。**`ExprValidator` 不导出**——用户写 `class MyValidator(Validator)` 是唯一路径。

### 2.4 §0 原则对照表

| §0 原则 | OneOfNode / NoneOfNode | Validator（Python 层） |
|---------|------------------------|----------------------|
| #1 一次 FFI | ✅ 严格 1 次。inner.parse/build 在 Rust 内；`PyFrozenSet::contains` 是 C API（§0.2 判据 2，int/str 元素 `__hash__`/`__eq__` C 级） | ⚠️ **2 次**（parse 入口 + `_validate` 回调）。用户主动继承 Validator = 显式接受折衷（ADR-022 §0 论证，不违反 §0 #1） |
| #2 无中间表示层 | ✅ frozenset 是 Python 对象本身 | ✅ _validate 输入输出是最终用户对象 |
| #3 输入输出无 trait 抽象 | ✅ 仅 `Box<Node>` | ✅ 复用 AdapterCallbackNode |
| #4 pyo3 核心依赖 | ✅ PyFrozenSet::contains 是 pyo3 API | ✅ AdapterCallbackNode 持 `Py<PyAny>` |
| #5 mashumaro API | ✅ OneOf/NoneOf 是字段描述符 | ✅ Validator 是用户继承基类 |
| #6 enum_dispatch | ✅ 2 个新 Node（OneOf/NoneOf） | N/A（Python 层无 Node） |
| #7 Result + path | ✅ ConstructError::Validation 携带 path | ✅ 同（跨 FFI 转 Generic） |
| #8 Stream 纯 Rust | ✅ 透传 inner.stream | ✅ 复用 AdapterCallbackNode |

### 2.5 性能假设

#### 2.5.1 瓶颈识别

| Node | 主要瓶颈 | 量化来源 |
|------|---------|---------|
| OneOf/NoneOf | inner.parse + 1 次 `PyFrozenSet::contains`（int 元素，~30-50ns） | pyo3 benchmark：PySet_Contains ~30-50ns（int hash+eq C 级） |

#### 2.5.2 可证伪预测

**嵌入 Struct 字段，与 Python construct 2.10.70 对比**：

- **OneOf(Byte, [1,2,3])**：加速比 ≥6x（vs Python：ExprValidator._decode 调度 + Python `in` list 查找）。**风险**：若 valids 是 list（Python），`in` 是 O(n) 线性；construct-rs 编译期转 frozenset 是 O(1) hash lookup，应明显拉开
- Validator（Python 层）：不设硬门禁（用户域后处理，2 FFI）

### 2.6 边界条件清单

| 编号 | 场景 | 预期行为 |
|------|------|---------|
| OO-1 | `OneOf(Byte, [1,2,3]).parse(b'\x01')` | 返回 1（∈ valids） |
| OO-2 | `OneOf(Byte, [1,2,3]).parse(b'\xff')` | ConstructError::Validation（"object failed validation: 255"） |
| OO-3 | `OneOf(Byte, [1,2,3]).build(1)` | 输出 b'\x01'（∈ valids） |
| OO-4 | `OneOf(Byte, [1,2,3]).build(99)` | ConstructError::Validation |
| OO-5 | `OneOf(Byte, [])` valids 为空 | 总是 ValidationError（对齐 Python：空集合不含任何元素） |
| OO-6 | valids 含非 hashable 元素（如 list） | 编译期 TypeError（frozenset 构造失败，对齐 Python frozenset([[1]])） |
| OO-7 | valids 是 set 输入（非 list） | 编译期转 frozenset（Python docstring 推荐 set/frozenset） |
| NO-1 | `NoneOf(Byte, [1,2,3]).parse(b'\xff')` | 返回 255（∉ invalids） |
| NO-2 | `NoneOf(Byte, [1,2,3]).parse(b'\x01')` | ConstructError::Validation |
| NO-3 | `NoneOf(Byte, []).parse(b'\xff')` | 返回 255（空 invalids，无元素被排除） |
| VL-1 | `class MyV(Validator): _validate(...)` parse 校验通过 | 返回 obj |
| VL-2 | `class MyV(Validator): _validate(...)` parse 校验失败 | ValidationError（"object failed validation: ..."） |
| VL-3 | Validator 嵌入 Struct（`field(MyV(Int8ub))`） | 编译为 AdapterCallbackNode，2 FFI |
| VL-4 | `_validate` 抛 Python 异常 | 跨 FFI 转 ConstructError::Generic |
| VL-5 | ExprValidator（用户传 lambda） | **不支持**（construct-rs 不导出 ExprValidator，ADR-014 硬约束） |

### 2.7 DEV 实施清单

| 文件 | 内容 | 行数估 |
|------|------|-------|
| `nodes/one_of.rs` | OneOfNode + impl + 单元测试 | ~180 |
| `nodes/none_of.rs` | NoneOfNode + impl + 单元测试（可与 one_of 共享 frozenset 辅助） | ~150 |
| `nodes/mod.rs` | 新增 2 个 mod/enum 变体 + has_expressions 分支 | ~15 增量 |
| `compile.rs` | 新增 2 个 build_*_node 分支（valids/invalids 转 frozenset 物化） | ~40 增量 |
| `error.rs` | 新增 ConstructError::Validation 变体 + ExceptionClasses.validation_error + Python 类映射 | ~30 增量 |
| `python/construct/_adapters.py`（新建或追加） | Validator 基类（port core.py L849-863，依赖 SymmetricAdapter） | ~30 |
| `_descriptors.py` | OneOfDescriptor / NoneOfDescriptor + 工厂函数 | ~30 增量 |
| `_errors.py` | ValidationError Python 类（对齐 core.py L125） | ~10 增量 |
| `__init__.py` | 导出 OneOf / NoneOf / Validator / ValidationError | ~10 增量 |

**Cargo.toml 不需新增依赖**（PyFrozenSet C API 已有）。

### 2.8 Parity 测试模板

```python
def test_oneof_parse_valid():
    # OneOf(Byte, [1,2,3]).parse(b'\x01') → 1
    assert_parity_case("""
    from construct import OneOf, Byte
    obj = OneOf(Byte, [1,2,3]).parse(b'\\x01')
    """, expected_py=1)

def test_oneof_parse_invalid_raises():
    # OneOf(Byte, [1,2,3]).parse(b'\xff') → ValidationError
    assert_parity_error("""
    from construct import OneOf, Byte, ValidationError
    try: OneOf(Byte, [1,2,3]).parse(b'\\xff')
    except ValidationError: pass
    """)

def test_oneof_set_input_compiles_to_frozenset():
    # OneOf(Byte, {1,2,3}) set 输入 → frozenset 物化（编译期）
    assert_parity_case("""
    from construct import OneOf, Byte
    obj = OneOf(Byte, {1,2,3}).parse(b'\\x01')
    """, expected_py=1)

def test_noneof_parse_excluded():
    # NoneOf(Byte, [1,2,3]).parse(b'\xff') → 255
    assert_parity_case("""
    from construct import NoneOf, Byte
    obj = NoneOf(Byte, [1,2,3]).parse(b'\\xff')
    """, expected_py=255)

def test_validator_subclass_in_struct():
    # class MyV(Validator):
    #     def _validate(self, obj, ctx, path): return obj > 0
    # @dataclass class P(StructMixin):
    #     val: int = field(MyV(Int8ub))
    # P.parse(b'\x05') → P(val=5)；P.parse(b'\xff') → ValidationError
    assert_parity_case("""
    from dataclasses import dataclass
    from construct import StructMixin, field, Validator, Int8ub
    class MyV(Validator):
        def _validate(self, obj, ctx, path): return obj > 0
    @dataclass
    class P(StructMixin):
        val: int = field(MyV(Int8ub))
    obj = P.parse(b'\\x05')
    """, check_only=["obj.val==5"])
```

**Parity 重点**：
- OO-5/NO-3 空集合边界（OneOf 空集总报错；NoneOf 空集总通过）
- VL-5 ExprValidator 不支持的 parity 差异（用户文档标注）

---

## 3. 子任务 8.6：Union

### 3.1 Python 参考实现摘要

`Union(parsefrom, *subcons, **subconskw)`（core.py L3641-3800）：

- **parse**：context nesting；`fallback = stream.tell()`；遍历 subcons，每 subcon.parse（命名字段写入 obj 和 context）；记录 `forwards[i] = tell()`（每 subcon parse 后位置）+ `forwards[name] = tell()`（命名字段）；每 subcon 后 `stream.seek(fallback)`（回退到入口）。最后按 parsefrom seek 到 `forwards[parsefrom]`（None 则不 seek，留 fallback）
- **build**：context nesting；`context.update(obj)`；遍历 subcons，找首个 `name in obj`（或 flagbuildnone 时 `obj.get(name)`）的 subcon，build 它，返回 `Container({name: buildret})`；无匹配 → UnionError
- **sizeof**：永远 SizeofError（"Union builds depending on actual object dict, size is unknown"）
- **parsefrom** 类型：None / int（index）/ str（name）/ context lambda

### 3.2 parsefrom ExprProgram 方案（PM 决策 D-5 已接受）

PM 决策 D-5：支持 ExprProgram（int 求值 → index），不支持 context lambda。

```rust
/// parsefrom 的解析结果（编译期从用户输入编译）。
///
/// - None：parse 后留 stream 在 fallback（不 seek）
/// - Index(usize)：parse 后 seek 到 forwards[index]
/// - Name：编译期解析 name→index（与 Index 同），仅用于错误消息区分
/// - Expr(ExprProgram)：运行期求值得 i64 → usize index → seek forwards[index]
///
/// Python 的 `forwards[name]` 在 construct-rs 编译期解析为 index（name→index
/// 映射固定），运行期只需 Vec<usize>（按 index 查 forward）。Name 变体实际
/// 存储 resolved_index，与 Index 同效——但保留枚举变体便于错误消息与文档。
#[derive(Debug)]
pub enum ParseFrom {
    /// 留在 fallback 位置（Python `parsefrom=None`）。
    None,
    /// 常量 index（Python `parsefrom=0`）。
    Index(usize),
    /// 常量 name，编译期已解析为 index（Python `parsefrom="raw"`）。
    Name { name: Py<PyString>, resolved_index: usize },
    /// 表达式，运行期求值得 index（construct-rs 扩展，替代 context lambda）。
    Expr(ExprProgram),
}
```

**编译期 name→index 解析**：`compile.rs` 在识别 `UnionDescriptor` 时，遍历命名的 subcons 建立 `{name: index}` 映射，parsefrom 是 str 时查表得 `resolved_index`（找不到 → CompilationError，对齐 Python L3774 KeyError）。

### 3.3 Rust Node 设计

```rust
/// 联合体节点：多视角 parse（每 subcon 独立 parse 后回退），按 parsefrom 选择 forward。
///
/// 对应 Python construct `Union(parsefrom, *subcons)`（core.py L3641）。
///
/// # parse 流程（设计 §3.4）
///
/// 1. context nesting（new_child，与 FocusedSeq 同模式）
/// 2. fallback = stream.tell()
/// 3. 遍历 subcons：每 subcon.parse；命名字段写入 obj（PyDict）+ child_ctx；
///    记录 forwards[i] = tell()；seek(fallback)
/// 4. 按 parsefrom 解析得 target_index（None 时不 seek）
/// 5. seek(forwards[target_index])
/// 6. 返回 obj（PyDict）
///
/// # build 流程
///
/// context nesting；遍历 subcons 找首个 name 在 obj（PyDict）中的 → build 它，返回。
/// 无匹配 → UnionError。
///
/// # sizeof
///
/// 永远 Err（对齐 Python L3751）。
///
/// # 表达式约束（PM 决策 D-5）
///
/// parsefrom 仅支持 None/常量 index/常量 name/ExprProgram(→index)，不支持 context lambda。
#[derive(Debug)]
pub struct UnionNode {
    /// 有序 subcons 列表（name 可选）。
    subcons: Vec<UnionSubcon>,
    /// parsefrom 策略（编译期编译）。
    parsefrom: ParseFrom,
    /// 是否含表达式（编译期递归子树计算）。
    has_expressions: bool,
}

/// Union 子构造器（name 可选 + node）。
#[derive(Debug)]
pub struct UnionSubcon {
    pub name: Option<Py<PyString>>,
    pub node: Node,
}

impl UnionNode {
    pub fn new(subcons: Vec<UnionSubcon>, parsefrom: ParseFrom, has_expressions: bool) -> Self {
        Self { subcons, parsefrom, has_expressions }
    }
    pub fn subcons(&self) -> &[UnionSubcon] { &self.subcons }
    pub fn parsefrom(&self) -> &ParseFrom { &self.parsefrom }
    pub fn has_expressions(&self) -> bool { self.has_expressions }
}
```

**`has_expressions` 集成**：`Node::Union(u) => u.has_expressions()`（编译期预算，与 FocusedSeq 同模式）。

### 3.4 Construct impl（关键路径摘要）

```rust
impl Construct for UnionNode {
    fn parse<'py>(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
        // 1. context nesting
        let mut child_ctx = Context::new_child(ctx, py)?;
        if self.has_expressions {
            child_ctx.init_expr_values(self.subcons.len());
        }

        // 2. 结果 dict（Container 等价：Python 返回 Container，construct-rs 返回 PyDict）
        let obj = PyDict::new_bound(py);

        // 3. fallback + 遍历
        let fallback = stream.tell();
        let mut forwards = Vec::with_capacity(self.subcons.len());
        for (idx, sc) in self.subcons.iter().enumerate() {
            let subobj = sc.node.parse(py, stream, &mut child_ctx, path)?;
            if let Some(name) = &sc.name {
                obj.set_item(name.bind(py), subobj.bind(py))?;
                child_ctx.set_field_at(idx, name.py_name(), subobj.bind(py), py)?;
            }
            forwards.push(stream.tell());
            stream.seek(fallback, path)?;  // 回退到入口
        }

        // 4. 按 parsefrom seek
        let target_index: Option<usize> = match &self.parsefrom {
            ParseFrom::None => None,
            ParseFrom::Index(i) => Some(*i),
            ParseFrom::Name { resolved_index, .. } => Some(*resolved_index),
            ParseFrom::Expr(prog) => {
                let v = crate::expr::eval_expr_int(prog, &child_ctx, py)?;
                Some(v as usize)
            }
        };
        if let Some(idx) = target_index {
            // 越界检查（对齐 Python forwards[parsefrom] KeyError/IndexError）
            let forward = forwards.get(idx).ok_or_else(|| ConstructError::Union {
                message: format!("parsefrom index {} out of range ({} subcons)", idx, self.subcons.len()),
                path: path.to_string(),
            })?;
            stream.seek(*forward, path)?;
        }
        Ok(obj.into_py(py))
    }
    fn build(&self, py, obj, stream, ctx, path) -> Result<(), ConstructError> {
        let mut child_ctx = Context::new_child(ctx, py)?;
        if self.has_expressions {
            child_ctx.init_expr_values(self.subcons.len());
        }
        // obj 是 PyDict（用户传 dict({name: value})）
        let obj_dict = obj.downcast::<PyDict>(py).map_err(|_| ConstructError::Union {
            message: format!("Union build expects a dict, got {:?}", obj.repr()?),
            path: path.to_string(),
        })?;
        // 遍历找首个 name in obj 的 subcon
        for (idx, sc) in self.subcons.iter().enumerate() {
            let take = match &sc.name {
                Some(name) => obj_dict.get_item(name.bind(py))?.is_some(),
                None => false,  // 匿名 subcon 不参与 build 选择
            };
            if take {
                let name = sc.name.as_ref().unwrap();
                let value = obj_dict.get_item(name.bind(py))?.unwrap();
                child_ctx.set_field_at(idx, name.py_name(), &value, py)?;
                sc.node.build(py, &value, stream, &mut child_ctx, path)?;
                return Ok(());
            }
        }
        Err(ConstructError::Union {
            message: format!("cannot build, none of subcons were found in the dictionary {:?}", obj.repr()?),
            path: path.to_string(),
        })
    }
    fn sizeof(&self, _ctx) -> Result<usize, ConstructError> {
        Err(ConstructError::Union {
            message: "Union builds depending on actual object dict, size is unknown".to_string(),
            path: String::new(),
        })
    }
}
```

**Union build 的 parity 说明（C-3 修正）**：

Python `Union._build`（core.py L3729-3749）有两点 construct-rs 设计需明确标注的差异：

1. **`context.update(obj)`**（Python L3732）：Python 把整个 obj dict 合并进 context，使被选中的 subcon 之外的字段也可通过 context 被引用（若该 subcon 的表达式引用其他字段）。construct-rs 设计的 `child_ctx` 只 `set_field_at` 被选中的单个 subcon 字段——**已知限制**：build 仅构建一个 subcon，若该 subcon 的表达式引用 obj 中其他命名字段，construct-rs child_ctx 未含这些字段（Python context.update(obj) 含）。多数 Union subcon 不互引，影响有限；若 DEV 实施时发现常见用例受影响，可补充 `child_ctx` 遍历 obj 全字段 `set_field_at`（实现等价的 `context.update(obj)`）。
2. **`flagbuildnone` 分支**（Python L3734-3735）：Python 对 `flagbuildnone=True` 的 subcon（如被 `Const`/`Default` 包装的 Union 命名成员）用 `obj.get(sc.name, None)`（允许 obj 缺键，返回 None 构建）。construct-rs 当前设计仅查 `obj_dict.get_item(name)` 是否 `Some`——命名的 `flagbuildnone` subcon 在 obj 缺键时会被跳过（而非传 None 构建）。**parity 差异**：含 `flagbuildnone` subcon 的 Union build 行为可能不一致（见 UN-14）。多数 Union subcon 非 `flagbuildnone`，常见用例不受影响；若 DEV 实施时需要，可在 UnionSubcon 增加 `flag_build_none: bool` 字段并在 build 分支处理。

以上两点是 construct-rs 的**已知 parity 限制**，标注在 §7.5 parity 差异清单。不阻塞设计通过（常见用例不受影响），DEV 实施时按实际用例评估是否补齐。

**ParseStream::seek/tell 已有**（Phase 4/7 验证）：`stream.tell() -> usize` / `stream.seek(pos, path) -> Result<(), ConstructError>`。Union 无需新 stream API。

**Container vs PyDict 差异**：Python Union.parse 返回 `Container`（construct 的 dict 子类，`__repr__` 特殊）。construct-rs 返回普通 PyDict。**已知 repr 差异**（dict vs Container 格式），与 Probe 同处理——parity 测试不断言 repr 字符串，仅断言含相同键值。

### 3.5 §0 原则对照表

| §0 原则 | UnionNode |
|---------|-----------|
| #1 一次 FFI | ✅ 严格 1 次。所有 subcon.parse 在 Rust 内；seek/tell 纯 Rust；context nesting 纯 Rust；ExprProgram 求值在 Rust 内；PyDict::set_item 是 C API（§0.2 判据 2） |
| #2 无中间表示层 | ✅ PyDict 是 Python 对象本身（obj 结果）；forwards 是 `Vec<usize>`（流位置，非 Python 对象中间态） |
| #3 输入输出无 trait 抽象 | ✅ 仅 `Vec<UnionSubcon>` + `Box<Node>`（与 FocusedSeq 同模式） |
| #4 pyo3 核心依赖 | ✅ 全程 pyo3 API + ExprProgram |
| #5 mashumaro API | ✅ Union 可作为字段描述符（嵌入 Struct 时 inner 是 Union Node） |
| #6 enum_dispatch | ✅ 1 个新 Node 加入 enum |
| #7 Result + path | ✅ ConstructError::Union 携带 path |
| #8 Stream 纯 Rust | ✅ seek/tell 纯 Rust |

### 3.6 性能假设

#### 3.6.1 瓶颈识别

| Node | 主要瓶颈 | 量化来源 |
|------|---------|---------|
| Union | N 次 subcon.parse + N 次 tell + N 次 seek + M 次 PyDict::set_item（M = 命名字段数） | Phase 4/7 seek/tell ~1ns 各；PyDict::set_item ~40ns |

#### 3.6.2 可证伪预测

**嵌入 Struct 字段，与 Python construct 2.10.70 对比**：

- **Union(0, raw=Bytes(8), ints=Int32ub[2])**（2 subcon，1 命名 each）：加速比 ≥5x（vs Python：2 次 subcon.parse + 2 次 stream_tell + 2 次 stream_seek + Container 构造 + Python dict 操作）。**风险**：subcon 数多时 seek 开销累积；但 seek 是纯 Rust ~1ns，应保持优势

**FFI 来源清单**（L-05 对策）：
- StructMixin.parse 入口（1 次，共享）
- N 次 subcon.parse（Rust 内，递归）
- seek/tell（Rust 内）
- ExprProgram 求值（Rust 内，仅 Expr 变体）
- PyDict::set_item（C API，不计 FFI）

### 3.7 边界条件清单

| 编号 | 场景 | 预期行为 |
|------|------|---------|
| UN-1 | `Union(None, "raw"/Bytes(8), "ints"/Int32ub[2]).parse(b"12345678")` | 返回 dict{raw=b'12345678', ints=[...]}; stream 留在 fallback（位置 0） |
| UN-2 | `Union(0, "raw"/Bytes(8), "ints"/Int32ub[2]).parse(b"12345678")` | 返回 dict{...}; stream seek 到 forwards[0]=8（Bytes(8) 后） |
| UN-3 | `Union("ints", "raw"/Bytes(8), "ints"/Int32ub[2]).parse(b"12345678")` | 返回 dict{...}; stream seek 到 forwards[1]=8 |
| UN-4 | `Union(my_expr, ...)` parsefrom 是表达式 | 求值得 index → seek forwards[index] |
| UN-5 | `Union(my_expr, ...)` 表达式求值 index 越界 | ConstructError::Union（"parsefrom index out of range"） |
| UN-6 | `Union("unknown", ...)` parsefrom name 不存在 | **编译期拒绝**（CompilationError，name→index 解析失败） |
| UN-7 | `Union(0, ...).build(dict(raw=b'abc'))` | build 首个 name in obj 的 subcon（raw）→ 输出 b'abc' |
| UN-8 | `Union(0, ...).build(dict(unknown=1))` | ConstructError::Union（"none of subcons were found"） |
| UN-9 | `Union(0, ...).build(None)` | ConstructError::Union（None 非 dict） |
| UN-10 | `Union(0, ...).sizeof()` | Err（SizeofError 等价，对齐 Python L3751） |
| UN-11 | Union 含匿名 subcon（`Bytes(8)` 无 name） | parse 时该 subcon 结果不写入 obj（丢弃）；build 时匿名 subcon 不参与选择 |
| UN-12 | Union subcon parse 失败（如 Bytes(8) 但流不足） | 错误向上传播（无特殊捕获，对齐 Python） |
| UN-13 | parsefrom 是 context lambda（`lambda ctx: ...`） | **编译期拒绝**（ADR-014，与 DF-4 同硬约束，parity 差异） |
| UN-14 | Union 含 `flagbuildnone` subcon（如 `Const`/`Default` 包装的命名成员）且 obj 缺该键 | **parity 差异（C-3）**：Python（core.py L3734-3735）用 `obj.get(name, None)` 传 None 构建；construct-rs 当前设计跳过该 subcon（缺键视为不匹配），可能不构建或行为不一致。多数 Union subcon 非 flagbuildnone，常见用例不受影响（见 §3.4 build parity 说明） |
| UN-15 | Union subcon 表达式引用 obj 中其他命名字段（跨 subcon 引用） | **parity 限制（C-3）**：Python `context.update(obj)`（L3732）使所有命名字段可被引用；construct-rs child_ctx 仅含被选中 subcon 字段。跨 subcon 引用的 build 用例受此限制（见 §3.4 build parity 说明） |

### 3.8 DEV 实施清单

| 文件 | 内容 | 行数估 |
|------|------|-------|
| `nodes/union.rs` | UnionNode + UnionSubcon + ParseFrom + impl + 单元测试 | ~450 |
| `nodes/mod.rs` | 新增 mod/enum 变体 + has_expressions 分支 | ~15 增量 |
| `compile.rs` | 新增 build_union_node 分支（parsefrom 编译 + name→index 解析 + subcons 递归） | ~90 增量 |
| `error.rs` | 新增 ConstructError::Union 变体 + ExceptionClasses.union_error + Python 类映射 | ~30 增量 |
| `_descriptors.py` | UnionDescriptor + 工厂函数 | ~70 增量 |
| `_errors.py` | UnionError Python 类（对齐 core.py L130） | ~10 增量 |
| `__init__.py` | 导出 Union / UnionError | ~10 增量 |

**Cargo.toml 不需新增依赖**（seek/tell/ExprProgram 已有）。

### 3.9 Parity 测试模板

```python
def test_union_parsefrom_none_keeps_fallback():
    # Union(None, raw=Bytes(8), ints=Int32ub[2]).parse(b"12345678")
    # → dict(raw=b'12345678', ints=[825373492, 892745528])
    # stream.tell() == 0（留 fallback）
    assert_parity_case("""
    from construct import Union, Bytes, Int32ub
    from construct.lib.containers import ListContainer
    d = Union(None, raw=Bytes(8), ints=Int32ub[2])
    obj = d.parse(b'12345678')
    """, check_only=["obj['raw']==b'12345678'", "list(obj['ints'])==[825373492, 892745528]"])

def test_union_parsefrom_index_seeks_forward():
    # Union(0, raw=Bytes(8), ...).parse(b"12345678") → stream 前进到 8
    # 注：stream 状态需通过后续解析验证
    assert_parity_case("""
    from construct import Union, Bytes, Int32ub, Struct, Tell
    d = Struct('u' / Union(0, raw=Bytes(8)), 'after' / Tell)
    obj = d.parse(b'12345678')
    """, check_only=["obj.after==8"])

def test_union_parsefrom_name():
    # Union("ints", raw=Bytes(8), ints=Int32ub[2]).parse(...)
    assert_parity_case("""
    from construct import Union, Bytes, Int32ub
    obj = Union('ints', raw=Bytes(8), ints=Int32ub[2]).parse(b'12345678')
    """, check_only=["list(obj['ints'])==[825373492, 892745528]"])

def test_union_build_first_matching():
    # Union(0, raw=Bytes(8), ...).build(dict(chars=...)) → build chars subcon
    assert_parity_case("""
    from construct import Union, Byte
    d = Union(0, chars=Byte[8])
    out = d.build(dict(chars=range(8)))
    """, expected_py=b'\\x00\\x01\\x02\\x03\\x04\\x05\\x06\\x07')

def test_union_sizeof_raises():
    # Union(0, ...).sizeof() → SizeofError/UnionError
    assert_parity_error("""
    from construct import Union, Bytes, SizeofError
    try: Union(0, raw=Bytes(8)).sizeof()
    except Exception: pass
    """)
```

**Parity 重点**：
- UN-1/UN-2 parsefrom=None vs Index 的 stream 位置差异
- UN-13 context lambda 不支持的 parity 差异（用户文档标注，仅支持表达式）

---

## 4. 子任务 8.7：Sequence

### 4.1 Python 参考实现摘要

`Sequence(*subcons, **subconskw)`（core.py L2329-2487）：

- **parse**：context nesting（`Container(_ = context, ..., _subcons, _io, _index)`）；`obj = ListContainer()`；遍历 subcons，每 subcon.parse → append 到 obj；命名字段额外写入 context；`except StopFieldError: break`；返回 obj（list）
- **build**：context nesting；`objiter = iter(obj)`；`retlist = ListContainer()`；遍历 subcons，`subobj = next(objiter)`；命名字段写入 context 前置 + build 后更新；`except StopFieldError: break`；返回 retlist
- **sizeof**：`sum(sc.sizeof for sc in subcons)`（context nesting 后）
- 字段可命名（写入 context）或匿名（仅 append）；`>>` 操作符是语法糖（construct-rs 不重载）

### 4.2 关键设计决策：Sequence 不复用 StructNode 代码，共享模式

**ARCH 决策（已确认，8.0 分析报告 §2.2.1 + §7.5）**：Sequence 实现为**独立 SequenceNode**，不复用 StructNode 代码。

**理由**：
1. **输出 sink 不同**：StructNode 把字段值写入实例 `__dict__`（PyDict，ADR-008）；SequenceNode 把字段值 append 到 `PyList`。两类 sink 的构造/写入 API 完全不同（`PyDict_SetItem` vs `PyList_Append`），强行共享代码会引入"sink 抽象层"——违反 §0 #3（输入输出无 trait 抽象）
2. **返回对象类型不同**：StructNode 返回用户实例（dataclass 实例）；SequenceNode 返回 PyList。实例构造（`tp_new` + `__dict__` 初始化）与 PyList 构造（`PyList_New`）路径完全不同
3. **共享的是模式，非代码**：context nesting（new_child）、`has_expressions` 递归、`StopField` 哨兵处理、`compute_ro_value`（命名 RO 字段）、`set_field_at` 命名字段写入——这些**模式**与 StructNode 同脉络，但 SequenceNode 是独立实现（参考 FocusedSeqNode 的做法：FocusedSeq 也是独立 Node，不委托 StructNode）

**与 FocusedSeqNode 的关系**：FocusedSeq（Phase 7.1）是"返回单聚焦字段的 Sequence"——它已经独立实现了 context nesting + new_child 模式。SequenceNode 是 FocusedSeq 的"全字段返回"泛化（返回所有字段而非单 focus）。**SequenceNode 复用 FocusedSeqNode 的 context nesting 模式**（`Context::new_child` + `set_field_at`），但 sink 是 PyList 而非单值。

**L-04 对策落实**：模式（context nesting + StopField 捕获）已在 FocusedSeq/Struct/GreedyRange 沉淀，SequenceNode 复用模式不重新决策。代码层面独立实现是 §0 #3（无 trait 抽象）的硬要求。

### 4.3 用户面 API 设计（非 dataclass 语法）

**关键差异**：construct-rs 用户面是 dataclass 语法（命名序），但 Sequence 是**位置序**（Python `Sequence(Byte, Float32b)` 按 subcons 顺序）。因此 Sequence 不作为 StructMixin 子类使用，而是**构造器工厂**：

```python
# construct-rs 用户面（与 Python construct 2.10.70 一致的位置序）
from construct import Sequence, Byte, Float32b
d = Sequence(Byte, Float32b)
d.parse(b'\x00?\x9dp\xa4')  # → [0, 1.23]
d.build([0, 1.23])          # → b'\x00?\x9dp\xa4'
```

**嵌套用法**：Sequence 可作为 Struct 字段（`field(Sequence(Byte, Byte))`）或 Union subcon 或 NamedTuple inner。

### 4.4 Rust Node 设计

```rust
/// 位置序字段序列节点：parse 产出 PyList（按 subcons 顺序）。
///
/// 对应 Python construct `Sequence(*subcons)`（core.py L2329）。
///
/// # 与 StructNode 的关系（设计 §4.2）
///
/// 不复用 StructNode 代码——输出 sink 不同（PyList vs 实例 __dict__）。
/// 共享 context nesting / StopField 捕获 / has_expressions 递归模式
///（参考 FocusedSeqNode 的独立实现）。
///
/// # parse 流程
///
/// 1. context nesting（new_child）
/// 2. 创建空 PyList（ListContainer 等价：construct-rs 返回普通 list）
/// 3. 遍历 fields：field.node.parse → PyList::append；命名字段写入 child_ctx
/// 4. StopField 哨兵：捕获后 break（对齐 Python `except StopFieldError`）
/// 5. 返回 PyList
///
/// # build 流程
///
/// 1. context nesting
/// 2. obj 是 list/iterable：取 iter
/// 3. 遍历 fields：next(iter) 取元素；命名字段写入 child_ctx 前置；
///    field.node.build(元素)；RO 字段走 compute_ro_value
/// 4. StopField 哨兵：捕获后 break
#[derive(Debug)]
pub struct SequenceNode {
    /// 有序字段列表（name 可选）。
    fields: Vec<SequenceField>,
    /// 是否含表达式（编译期递归子树计算）。
    has_expressions: bool,
}

/// Sequence 字段（name 可选 + node + field_kind）。
#[derive(Debug)]
pub struct SequenceField {
    pub name: Option<Py<PyString>>,
    pub node: Node,
    /// 字段模式（ro/rw，与 StructNode FieldMode 同脉络）。
    /// Check/Computed/Tell 等 RO 字段在 build 时不从 list 取值，走 compute_ro_value。
    pub field_kind: FieldKind,
}

impl SequenceNode {
    pub fn new(fields: Vec<SequenceField>, has_expressions: bool) -> Self {
        Self { fields, has_expressions }
    }
    pub fn fields(&self) -> &[SequenceField] { &self.fields }
    pub fn has_expressions(&self) -> bool { self.has_expressions }
}
```

**`has_expressions` 集成**：`Node::Sequence(s) => s.has_expressions()`（编译期预算，与 FocusedSeq 同模式）。

**FieldKind**：复用 StructNode 已有的 `FieldKind` 枚举（ro/rw）。RO 字段（Check/Computed/Tell/Rebuild/StopIf/Index/Element）在 build 时不从 list 取元素，走 `compute_ro_value`——这与 StructNode 的 RO 路径一致（设计 §4.2 共享模式）。

### 4.5 Construct impl（关键路径摘要）

```rust
impl Construct for SequenceNode {
    fn parse<'py>(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
        // 1. context nesting
        let mut child_ctx = Context::new_child(ctx, py)?;
        if self.has_expressions {
            child_ctx.init_expr_values(self.fields.len());
        }

        // 2. PyList（ListContainer 等价）
        let list = PyList::empty_bound(py);

        // 3. 遍历 fields
        for (idx, field) in self.fields.iter().enumerate() {
            match field.node.parse(py, stream, &mut child_ctx, path) {
                Ok(val) => {
                    list.append(val)?;
                    if let Some(name) = &field.name {
                        child_ctx.set_field_at(idx, name.py_name(), list.get_item(list.len()-1)?, py)?;
                    }
                }
                Err(ConstructError::StopField { .. }) => break,  // StopIf 触发，正常终止
                Err(e) => return Err(e),
            }
        }
        Ok(list.into_py(py))
    }
    fn build(&self, py, obj, stream, ctx, path) -> Result<(), ConstructError> {
        let mut child_ctx = Context::new_child(ctx, py)?;
        if self.has_expressions {
            child_ctx.init_expr_values(self.fields.len());
        }

        // obj 是 list/iterable。提取为迭代器（pyo3 PyList::iter 或通用 iter）
        // 注：Python Sequence.build 接受任意 iterable，construct-rs 要求 list（pyo3 extract）
        let obj_list = obj.downcast::<PyList>(py).map_err(|_| ConstructError::Generic {
            message: format!("Sequence build expects a list, got {:?}", obj.repr()?),
            path: path.to_string(),
        })?;
        let obj_iter = obj_list.iter();
        let mut iter = obj_iter;

        for (idx, field) in self.fields.iter().enumerate() {
            // RO 字段不从 list 取值（parity 差异：Python core.py L2410 对所有 subcons 都 next(objiter)，含 RO 字段；construct-rs RO 字段走 compute_ro_value，list 不含占位。见 §7.5 parity 差异清单）
            let build_obj_result: Result<Py<PyAny>, ConstructError> = if field.field_kind == FieldKind::Ro {
                // compute_ro_value（与 StructNode 同模式）
                Ok(field.node.compute_ro_value(py, stream, &child_ctx, path)?)
            } else {
                match iter.next() {
                    Some(item) => Ok(item.unbind()),
                    None => return Err(ConstructError::Generic {
                        message: "Sequence build: list shorter than fields".to_string(),
                        path: path.to_string(),
                    }),
                }
            };
            let build_obj = build_obj_result?;
            // 命名字段前置写入 context（与 StructNode build 同模式）
            if let Some(name) = &field.name {
                child_ctx.set_field_at(idx, name.py_name(), build_obj.bind(py), py)?;
            }
            // build
            match field.node.build(py, build_obj.bind(py), stream, &mut child_ctx, path) {
                Ok(()) => {},
                Err(ConstructError::StopField { .. }) => break,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
    fn sizeof(&self, ctx) -> Result<usize, ConstructError> {
        // sizeof 无 py token，无法 new_child。直接 sum（与 FocusedSeq sizeof 同处理）。
        // context nesting 仅影响字段间引用，不影响静态 sizeof（对齐 Python L2429）。
        let mut total = 0usize;
        for field in &self.fields {
            total += field.node.sizeof(ctx)?;
        }
        Ok(total)
    }
}
```

**StopField 捕获**：与 StructNode/GreedyRange 同模式（`Err(ConstructError::StopField{..}) => break`）。lazy path 错误传播（P0-3 模式）也适用——成功路径不 push_path_segment，子节点 Err 时由上层补 path 段。

**`compute_ro_value` 复用**：SequenceField 的 RO 字段（如嵌入 Check）走 `Node::compute_ro_value`（mod.rs 已有的公共 API），与 StructNode 完全一致——这是"共享模式非代码"的体现。

### 4.6 §0 原则对照表

| §0 原则 | SequenceNode |
|---------|--------------|
| #1 一次 FFI | ✅ 严格 1 次。所有 field.parse/build 在 Rust 内；context nesting 纯 Rust；`PyList::append` 是 C API（§0.2 判据 2）；compute_ro_value 在 Rust 内 |
| #2 无中间表示层 | ✅ PyList 是 Python 对象本身（obj 结果）；无 Rust 中间类型 |
| #3 输入输出无 trait 抽象 | ✅ 独立 Node，无 sink trait 抽象（与 StructNode 分离是 §0 #3 的硬要求，设计 §4.2） |
| #4 pyo3 核心依赖 | ✅ PyList/PyDict/context 全 pyo3 API |
| #5 mashumaro API | ✅ Sequence 是构造器工厂（位置序），可作为字段描述符嵌入 |
| #6 enum_dispatch | ✅ 1 个新 Node 加入 enum |
| #7 Result + path | ✅ 错误带 path（StopField + lazy path 传播） |
| #8 Stream 纯 Rust | ✅ 透传 field.stream 操作 |

### 4.7 性能假设

#### 4.7.1 瓶颈识别

| Node | 主要瓶颈 | 量化来源 |
|------|---------|---------|
| Sequence | N 次 field.parse + N 次 `PyList::append`（~30ns 各）+ context nesting（new_child ~50ns 一次性） | ADR-017（Vec 中转 PyList）：PyList::append ~30ns；FocusedSeq 已验证 new_child ~50ns |

#### 4.7.2 可证伪预测

**与 Python construct 2.10.70 对比**：

- **Sequence(Byte, Float32b)**（2 字段）：加速比 ≥8x（vs Python：ListContainer append + Container nesting + Python 循环开销）
- **Sequence(10 个 Int8ub)**：加速比 ≥8x（field 数多时 PyList::append 累积，但仍远优于 Python 循环）

**FFI 来源清单**（L-05 对策）：
- StructMixin.parse 入口（1 次，共享，若 Sequence 嵌入 Struct）
- 或 Sequence.parse 入口（1 次，若单独使用）
- N 次 field.parse（Rust 内，递归）
- PyList::append（C API，不计 FFI）
- context nesting（Rust 内）

### 4.8 边界条件清单

| 编号 | 场景 | 预期行为 |
|------|------|---------|
| SQ-1 | `Sequence(Byte, Float32b).parse(b'\x00?\x9dp\xa4')` | 返回 [0, 1.23]（PyList） |
| SQ-2 | `Sequence(Byte, Float32b).build([0, 1.23])` | 输出 b'\x00?\x9dp\xa4' |
| SQ-3 | `Sequence("count"/Byte, "data"/Bytes(count)).parse(b'\x03ABC')` | 返回 [3, b'ABC']（命名字段写入 context，data 引用 count） |
| SQ-4 | `Sequence("count"/Byte, "data"/Bytes(count)).build([3, b'ABC'])` | 输出 b'\x03ABC' |
| SQ-5 | Sequence 含 StopIf 字段 | StopIf 触发时正常终止（捕获 StopField，对齐 Python L2399） |
| SQ-6 | Sequence 含 Check（RO 字段）parse | Check 求值表达式，通过时 append Py_None 到 list |
| SQ-7 | Sequence 含 Check（RO 字段）build | Check 不从 list 取值（走 compute_ro_value 返回 None 占位）。**parity 差异**：Python `Sequence._build`（core.py L2410-2423）对所有 subcons 都 `next(objiter)`（含 Check），故 Python 用户需传 `[1, None, 2]`（list 含 RO 字段占位）；construct-rs RO 字段不消费 list 位置，用户传 `[1, 2]` 即可。用户从 Python 迁移时需调整 build 输入（见 §7.5） |
| SQ-8 | `Sequence(Byte, Byte).build([1])`（list 比字段短） | ConstructError::Generic（"Sequence build: list shorter than fields"） |
| SQ-9 | `Sequence(Byte, Byte).build([1,2,3])`（list 比字段长） | 仅用前 2 个元素（多余忽略，对齐 Python `next(objiter)` 不消费多余） |
| SQ-10 | `Sequence(Byte, Byte).build(None)` | ConstructError::Generic（None 非 list） |
| SQ-11 | `Sequence(Byte, Byte).sizeof()` | 返回 2（sum） |
| SQ-12 | `Sequence("a"/Byte, Bytes(2)).parse(b'\x01\x02\x03')` | 返回 [1, b'\x02\x03']（匿名 Bytes 不写 context） |
| SQ-13 | Sequence 嵌入 Struct（`field(Sequence(Byte, Byte))`） | StructNode 遍历到 Sequence 字段 → Sequence.parse 返回 PyList |
| SQ-14 | Sequence field.parse 失败（流不足） | 错误向上传播（无特殊捕获，对齐 Python） |
| SQ-15 | `>>` 操作符（`Byte >> Byte`） | **不支持**（construct-rs 不重载操作符，parity 差异，用户用 Sequence(...) 显式构造） |

### 4.9 DEV 实施清单

| 文件 | 内容 | 行数估 |
|------|------|-------|
| `nodes/sequence.rs` | SequenceNode + SequenceField + impl + 单元测试 | ~500 |
| `nodes/mod.rs` | 新增 mod/enum 变体 + has_expressions 分支 | ~15 增量 |
| `compile.rs` | 新增 build_sequence_node 分支（fields 递归 + FieldKind 识别 + has_expressions 计算） | ~80 增量 |
| `_descriptors.py` | SequenceDescriptor + 工厂函数（支持 *subcons 位置序） | ~80 增量 |
| `__init__.py` | 导出 Sequence | ~5 增量 |

**Cargo.toml 不需新增依赖**（PyList/context/new_child 已有）。

**FieldKind 复用**：SequenceField.field_kind 复用 StructNode 已有的 `FieldKind` 枚举（compile.rs 识别 RO 描述符时标记）。DEV 实施时确认 FieldKind 是否 pub 可见，否则提取到 common.rs。

### 4.10 Parity 测试模板

```python
def test_sequence_parse_returns_list():
    # Sequence(Byte, Float32b).parse(b'\x00?\x9dp\xa4') → [0, 1.23]
    assert_parity_case("""
    from construct import Sequence, Byte, Float32b
    obj = Sequence(Byte, Float32b).parse(b'\\x00?\\x9dp\\xa4')
    """, check_only=["list(obj)==[0, 1.2300000190734863]"])

def test_sequence_build_from_list():
    # Sequence(Byte, Float32b).build([0, 1.23]) → b'\x00?\x9dp\xa4'
    assert_parity_case("""
    from construct import Sequence, Byte, Float32b
    out = Sequence(Byte, Float32b).build([0, 1.23])
    """, expected_py=b'\\x00?\\x9dp\\xa4')

def test_sequence_named_field_in_context():
    # Sequence("count"/Byte, "data"/Bytes(count)).parse(b'\x03ABC') → [3, b'ABC']
    assert_parity_case("""
    from construct import Sequence, Byte, Bytes
    obj = Sequence('count'/Byte, 'data'/Bytes(lambda this: this.count)).parse(b'\\x03ABC')
    """, check_only=["list(obj)==[3, b'ABC']"])

def test_sequence_with_stopif():
    # Sequence(Byte, StopIf(this[0]==1), Byte) → StopIf 触发时仅返回 [1]
    assert_parity_case("""
    from construct import Sequence, Byte, StopIf
    obj = Sequence(Byte, StopIf(lambda this: this[0]==1), Byte).parse(b'\\x01\\x02')
    """, check_only=["list(obj)==[1]"])

def test_sequence_build_short_list_raises():
    # Sequence(Byte, Byte).build([1]) → 错误（list 比字段短）
    assert_parity_error("""
    from construct import Sequence, Byte
    try: Sequence(Byte, Byte).build([1])
    except Exception: pass
    """)

def test_sequence_sizeof():
    # Sequence(Byte, Byte).sizeof() → 2
    assert_parity_case("""
    from construct import Sequence, Byte
    s = Sequence(Byte, Byte).sizeof()
    """, expected_py=2)
```

**Parity 重点**：
- SQ-3 命名字段 context 写入（data 引用 count）
- SQ-5/SQ-6 StopIf/Check 嵌入 Sequence 的 RO 字段处理
- SQ-15 `>>` 操作符不支持（用户文档标注）

---

## 5. 子任务 8.12：ProcessXor / ProcessRotateLeft

### 5.1 Python 参考实现摘要

| 构造器 | Python 行号 | 核心语义 |
|--------|-----------|---------|
| `ProcessXor(padfunc, subcon)` | core.py L5357-5421 | Subconstruct。parse：evaluate pad → 若 bytes len==1 转 int → `stream_read_entire`（读至 EOF）→ **fast-path**：pad==0 或 pad 是全零 bytes（len<=64）则不变换；否则 XOR（int：每字节 `b^pad`；bytes：`zip(cycle(pad))`）→ 构造 `BytesIOWithOffsets(data, stream, offset)` 子流 → subcon.parse。build 对称（build 到 stream2，XOR，写主流）。sizeof 转发 |
| `ProcessRotateLeft(amount, group, subcon)` | core.py L5424-5529 | Subconstruct。parse：evaluate amount/group；`group < 1` → RotationError；`amount %= group*8`；read entire stream；`len(data) % group != 0` → RotationError；**4 分支**：(1) amount==0 pass；(2) group==1 查 `precomputed_single_rotations[amount]` 表；(3) amount%8==0 字节序重排（indices）；(4) 通用 bit rotate（indices_pairs）；构造 `io.BytesIO(data)` 子流 → subcon.parse。build 对称（`amount = -amount % (group*8)` 取负后同样 4 分支）。sizeof 转发 |

**ProcessXor 关键 fast-path**（Python L5394/L5397）：
- pad 为 int 且 `pad == 0`：不变换（直接用原 data）
- pad 为 bytes 且 `len(pad) <= 64 and pad == bytes(len(pad))`（全零）：不变换

**ProcessRotateLeft `precomputed_single_rotations`**（Python L5454，类级缓存）：
`{amount: [(i << amount) & 0xff | (i >> (8-amount)) for i in range(256)] for amount in range(1,8)}`——7 个表，每表 256 个单字节旋转值。construct-rs 用 `const` 表 + 索引（参考 TransformNode 的 `BIT_REVERSE_TABLE`，Phase 3.3）。

### 5.2 Rust Node 设计：复用 TransformNode 子流模式

**设计决策**：ProcessXor/ProcessRotateLeft 实现为 **Rust Node 变体**，复用 TransformNode 的"读字节 → Rust 内变换 → 子流构造 → inner.parse"模式（Phase 3.3 已验证）。

**与 TransformNode 的差异**：
- TransformNode 要求定长 subcon（`inner.sizeof()` 读字节）；ProcessXor/ProcessRotateLeft 读**至 EOF**（`stream_read_entire` 等价，即 `stream.data()[stream.tell()..]`）
- TransformNode 变换固定（ByteSwap/BitSwap）；ProcessXor/ProcessRotateLeft 变换动态（pad/amount/group 可为表达式）

### 5.3 ProcessXorNode 设计

```rust
/// XOR 字节变换节点：parse 读至 EOF → XOR pad → 子流 → inner.parse。
///
/// 对应 Python construct `ProcessXor(padfunc, subcon)`（core.py L5357）。
///
/// # pad 编译期 vs 运行期（§0.4 表达式约束）
///
/// padfunc 可为：
/// - **编译期常量**（int 或 bytes）：物化为 [`XorPad`]，运行期零 eval
/// - **表达式**（FieldRef 等）：编译为 ExprProgram，运行期 eval 得 int/bytes 再变换
///
/// **不接收 Python callable**（ADR-014 硬约束，parity 差异 PX-4）。
///
/// # 三方法行为
///
/// - parse：读 `stream.data()[tell()..]`（至 EOF）→ apply_xor → 子流 → inner.parse
/// - build：inner build 到子流 → apply_xor → 写主流
/// - sizeof：转发 inner.sizeof
///
/// # fast-path（对齐 Python L5394/L5397）
///
/// - pad int == 0：不变换（直接用原 data）
/// - pad bytes 全零（len<=64）：不变换
#[derive(Debug)]
pub struct ProcessXorNode {
    inner: Box<Node>,
    pad: XorPad,
}

/// XOR pad 类型（编译期物化或运行期求值）。
#[derive(Debug)]
pub enum XorPad {
    /// 单字节 int pad（pad==0 时 fast-path，不变换）。
    Int(u8),
    /// 多字节 bytes pad（编译期物化 Py<PyBytes> → Vec<u8>，全零 fast-path）。
    Bytes(Vec<u8>),
    /// 表达式 pad（运行期 eval 得 int 或 bytes）。
    Expr(ExprProgram),
}

impl ProcessXorNode {
    pub fn new(inner: Node, pad: XorPad) -> Self {
        Self { inner: Box::new(inner), pad }
    }
    pub fn inner(&self) -> &Node { &self.inner }
    pub fn pad(&self) -> &XorPad { &self.pad }
}
```

**`has_expressions` 集成**：`Node::ProcessXor(p) => p.inner().has_expressions() || matches!(p.pad(), XorPad::Expr(_))`（Expr pad 时为 true，与 Default 含 value 表达式同模式）。

**pad 编译期物化**：
- 用户传 `ProcessXor(0xf0, Int16ub)`：编译期物化为 `XorPad::Int(0xf0)`
- 用户传 `ProcessXor(b'\xf0\xf1', Int16ub)`：bytes len==1 时转 int（对齐 Python L5389）；否则物化为 `XorPad::Bytes(vec![0xf0, 0xf1])`
- 用户传 `ProcessXor(my_field, ...)`：编译为 `XorPad::Expr(expr_program)`

### 5.4 ProcessXorNode Construct impl

```rust
impl Construct for ProcessXorNode {
    fn parse<'py>(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
        // 1. 解析 pad（编译期物化或运行期 eval）
        let offset = stream.tell();
        let raw = stream.data();  // 整个缓冲
        let data = &raw[offset..];  // 至 EOF

        // 2. apply XOR（含 fast-path）
        let transformed = apply_xor(&self.pad, data, ctx, py)?;

        // 3. 子流构造 + inner.parse
        let mut sub_stream = ParseStream::new(&transformed);
        self.inner.parse(py, &mut sub_stream, ctx, path)
    }
    fn build(&self, py, obj, stream, ctx, path) -> Result<(), ConstructError> {
        // 1. inner build 到子流
        let capacity = self.inner.sizeof(ctx).unwrap_or(0);
        let mut sub_stream = BuildStream::with_capacity(capacity);
        self.inner.build(py, obj, &mut sub_stream, ctx, path)?;
        let data = sub_stream.into_bytes();

        // 2. apply XOR（对称）
        let transformed = apply_xor_build(&self.pad, &data, ctx, py)?;

        // 3. 写主流
        stream.write(&transformed);
        Ok(())
    }
    fn sizeof(&self, ctx) -> Result<usize, ConstructError> { self.inner.sizeof(ctx) }
}

/// 应用 XOR 变换（parse 路径）。
fn apply_xor(
    pad: &XorPad, data: &[u8], ctx: &Context, py: Python,
) -> Result<Vec<u8>, ConstructError> {
    match pad {
        XorPad::Int(0) => {
            // fast-path：pad==0 不变换（对齐 Python L5394）
            Ok(data.to_vec())
        }
        XorPad::Int(p) => {
            // int：每字节 b ^ pad（Rust 内联，SIMD 友好）
            Ok(data.iter().map(|&b| b ^ p).collect())
        }
        XorPad::Bytes(pad_bytes) => {
            if pad_bytes.len() <= 64 && pad_bytes.iter().all(|&b| b == 0) {
                // fast-path：全零 bytes 不变换（对齐 Python L5397）
                Ok(data.to_vec())
            } else {
                // bytes：zip(cycle(pad))，每字节 b ^ pad[i % len]
                let plen = pad_bytes.len();
                Ok(data.iter().enumerate()
                    .map(|(i, &b)| b ^ pad_bytes[i % plen])
                    .collect())
            }
        }
        XorPad::Expr(prog) => {
            // 运行期 eval 得 int 或 bytes，递归 apply
            let pad_val = crate::expr::eval_expr_any(prog, ctx, py)?;
            let resolved = resolve_xor_pad(&pad_val, py)?;
            apply_xor(&resolved, data, ctx, py)
        }
    }
}

/// 解析 Python 对象为 XorPad（Expr 路径用）。
fn resolve_xor_pad(obj: &Bound<PyAny>, py: Python) -> Result<XorPad, ConstructError> {
    if let Ok(v) = obj.extract::<i64>() {
        Ok(XorPad::Int(v as u8))
    } else if let Ok(b) = obj.extract::<&[u8]>() {
        if b.len() == 1 { Ok(XorPad::Int(b[0])) }
        else { Ok(XorPad::Bytes(b.to_vec())) }
    } else {
        Err(ConstructError::String {
            message: format!("ProcessXor needs integer or bytes pad, got {:?}", obj.repr()?),
            path: String::new(),
        })
    }
}
```

**`apply_xor_build`**：与 `apply_xor` 同（XOR 是对合运算，`b ^ p ^ p == b`，build 与 parse 用同一变换）。注：ProcessXor 的 build 不取负（与 ProcessRotateLeft 不同），直接复用 `apply_xor`。

### 5.5 ProcessRotateLeftNode 设计

```rust
/// 位旋转左移节点：parse 读至 EOF → 按 amount/group 位旋转 → 子流 → inner.parse。
///
/// 对应 Python construct `ProcessRotateLeft(amount, group, subcon)`（core.py L5424）。
///
/// # amount/group 表达式约束（§0.4）
///
/// amount/group 可为编译期常量或 ExprProgram，**不接收 callable**（ADR-014）。
///
/// # parse 流程
///
/// 1. eval amount/group；`group < 1` → RotationError
/// 2. `amount %= group * 8`
/// 3. 读至 EOF；`len % group != 0` → RotationError
/// 4. 4 分支变换（设计 §5.6）
/// 5. 子流构造 + inner.parse
///
/// # build 流程
///
/// 与 parse 对称，但 `amount = (-amount) % (group*8)`（取负，对齐 Python L5499）。
#[derive(Debug)]
pub struct ProcessRotateLeftNode {
    inner: Box<Node>,
    amount: ExprProgram,
    group: ExprProgram,
}

impl ProcessRotateLeftNode {
    pub fn new(inner: Node, amount: ExprProgram, group: ExprProgram) -> Self {
        Self { inner: Box::new(inner), amount, group }
    }
    pub fn inner(&self) -> &Node { &self.inner }
    pub fn amount(&self) -> &ExprProgram { &self.amount }
    pub fn group(&self) -> &ExprProgram { &self.group }
}
```

**`has_expressions` 集成**：`Node::ProcessRotateLeft(p) => true`（恒 true，含 amount/group 表达式，与 Default/Rebuild 同模式）。

**amount/group 常量编译期包装**：用户传 `ProcessRotateLeft(4, 1, Int16ub)`（常量）时，4 和 1 编译期包装为 `ExprProgram { ops: vec![PushI64(4)] }` / `PushI64(1)`（与 Computed/Default 常量模式同）。

### 5.6 ProcessRotateLeft 4 分支位运算（Rust 实现）

```rust
/// 预计算单字节旋转表（对应 Python L5454 precomputed_single_rotations）。
/// 7 个表（amount 1..7），每表 256 值。const 表，零运行时初始化。
/// 参考实现：transform.rs BIT_REVERSE_TABLE（Phase 3.3）。
const ROTATION_TABLES: [[u8; 256]; 8] = {
    let mut tables = [[0u8; 256]; 8];
    let mut amount = 1;
    while amount < 8 {
        let mut i = 0;
        while i < 256 {
            let v = i as u8;
            tables[amount][i] = (v << amount) | (v >> (8 - amount));
            i += 1;
        }
        amount += 1;
    }
    tables
};

/// 应用旋转（parse 路径，amount 为正）。
fn rotate_left(data: &[u8], amount: usize, group: usize) -> Result<Vec<u8>, ConstructError> {
    if group < 1 {
        return Err(ConstructError::Rotation {
            message: "group size must be at least 1 to be valid".to_string(),
            path: String::new(),
        });
    }
    if data.len() % group != 0 {
        return Err(ConstructError::Rotation {
            message: "data length must be a multiple of group size".to_string(),
            path: String::new(),
        });
    }
    let amount = amount % (group * 8);
    let amount_bytes = amount / 8;

    if amount == 0 {
        // 分支 1：不旋转
        return Ok(data.to_vec());
    }
    if group == 1 {
        // 分支 2：查表（amount 1..7，对齐 Python L5478）
        let table = &ROTATION_TABLES[amount];  // amount < 8（group==1 时 amount%8）
        return Ok(data.iter().map(|&b| table[b as usize]).collect());
    }
    if amount % 8 == 0 {
        // 分支 3：字节序重排（纯字节移位，对齐 Python L5482）
        let indices: Vec<usize> = (0..group).map(|i| (i + amount_bytes) % group).collect();
        let mut result = Vec::with_capacity(data.len());
        for chunk_start in (0..data.len()).step_by(group) {
            for &k in &indices {
                result.push(data[chunk_start + k]);
            }
        }
        return Ok(result);
    }
    // 分支 4：通用 bit rotate（对齐 Python L5488）
    let amount1 = amount % 8;
    let amount2 = 8 - amount1;
    let indices_pairs: Vec<(usize, usize)> = (0..group)
        .map(|i| ((i + amount_bytes) % group, (i + 1 + amount_bytes) % group))
        .collect();
    let mut result = Vec::with_capacity(data.len());
    for chunk_start in (0..data.len()).step_by(group) {
        for &(k1, k2) in &indices_pairs {
            let rotated = ((data[chunk_start + k1] << amount1) & 0xff)
                | (data[chunk_start + k2] >> amount2);
            result.push(rotated);
        }
    }
    Ok(result)
}
```

**Construct impl**（parse/build 对称，build 取负 amount）：

```rust
impl Construct for ProcessRotateLeftNode {
    fn parse<'py>(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
        let amount = crate::expr::eval_expr_int(&self.amount, ctx, py)?;
        let group = crate::expr::eval_expr_int(&self.group, ctx, py)?;
        let offset = stream.tell();
        let data = &stream.data()[offset..];
        let transformed = rotate_left(data, amount as usize, group as usize)
            .map_err(|e| e.with_path(path))?;
        let mut sub_stream = ParseStream::new(&transformed);
        self.inner.parse(py, &mut sub_stream, ctx, path)
    }
    fn build(&self, py, obj, stream, ctx, path) -> Result<(), ConstructError> {
        let amount = crate::expr::eval_expr_int(&self.amount, ctx, py)?;
        let group = crate::expr::eval_expr_int(&self.group, ctx, py)?;
        // build 取负（对齐 Python L5499：amount = -amount % (group*8)）
        let neg_amount = (-(amount)).rem_euclid(group * 8) as usize;
        let capacity = self.inner.sizeof(ctx).unwrap_or(0);
        let mut sub_stream = BuildStream::with_capacity(capacity);
        self.inner.build(py, obj, &mut sub_stream, ctx, path)?;
        let data = sub_stream.into_bytes();
        let transformed = rotate_left(&data, neg_amount, group as usize)
            .map_err(|e| e.with_path(path))?;
        stream.write(&transformed);
        Ok(())
    }
    fn sizeof(&self, ctx) -> Result<usize, ConstructError> { self.inner.sizeof(ctx) }
}
```

**`rem_euclid` 注意**：Python `(-amount) % (group*8)` 是模运算（非负）；Rust `%` 是 remainder（可为负），用 `rem_euclid` 对齐（与 P0 AlignedNode pad 算法同处理）。

### 5.7 §0 原则对照表

| §0 原则 | ProcessXorNode | ProcessRotateLeftNode |
|---------|----------------|----------------------|
| #1 一次 FFI | ✅ 严格 1 次。读字节 Rust 内；XOR 在 Rust 内（`iter().map`）；子流构造 Rust 内；inner.parse Rust 内；ExprProgram 求值 Rust 内 | ✅ 同；4 分支位运算全 Rust 内；ROTATION_TABLES const 表 |
| #2 无中间表示层 | ✅ `Vec<u8>` 是变换缓冲（非 Python 对象中间态），与 TransformNode 同处理 | ✅ 同 |
| #3 输入输出无 trait 抽象 | ✅ 仅 `Box<Node>`（与 TransformNode 同模式） | ✅ |
| #4 pyo3 核心依赖 | ✅ ExprProgram + stream | ✅ |
| #5 mashumaro API | ✅ ProcessXor/ProcessRotateLeft 是字段描述符 | ✅ |
| #6 enum_dispatch | ✅ 2 个新 Node | ✅ |
| #7 Result + path | ✅ ConstructError::String/Rotation 携带 path | ✅ |
| #8 Stream 纯 Rust | ✅ data()/slice 借用；子流构造纯 Rust | ✅ |

### 5.8 性能假设

#### 5.8.1 瓶颈识别

| Node | 主要瓶颈 | 量化来源 |
|------|---------|---------|
| ProcessXor | N 字节 XOR（Rust `iter().map`，~0.3ns/字节，SIMD 友好）+ 子流构造 + inner.parse | Rust XOR benchmark：~0.3ns/字节（vs Python ~30ns/字节，10x） |
| ProcessRotateLeft | 分支 2 查表（~1ns/字节）/ 分支 3 字节重排（~1ns/字节）/ 分支 4 通用（~2ns/字节）+ 子流 + inner.parse | 与 TransformNode BIT_REVERSE_TABLE 同量级（已验证） |

#### 5.8.2 可证伪预测

**与 Python construct 2.10.70 对比**：

- **ProcessXor(0xf0, Int16ub)**：加速比 ≥10x（Rust XOR 是 Python `bytes((b^pad) for b in data)` 的 ~10x；Rust SIMD + 零 Python 循环开销）
- **ProcessRotateLeft(4, 1, Int16ub)**：加速比 ≥10x（查表 + Rust 循环 vs Python `bytes(translate[a] for a in data)`）

**FFI 来源清单**（L-05 对策）：
- 入口（1 次）
- 读字节（Rust 内，slice 借用）
- XOR/rotate（Rust 内）
- 子流构造（Rust 内）
- inner.parse（Rust 内，递归）

### 5.9 边界条件清单

| 编号 | 场景 | 预期行为 |
|------|------|---------|
| PX-1 | `ProcessXor(0xf0, Int16ub).parse(b'\x00\xff')` | XOR 后 b'\xf0\x0f' → Int16ub → 0xf00f |
| PX-2 | `ProcessXor(0, Int16ub).parse(b'\x00\xff')` | fast-path pad==0 不变换 → 0x00ff |
| PX-3 | `ProcessXor(b'\xf0\xf1', Int16ub).parse(b'\x00\xff')` | bytes pad：b'\x00'^0xf0=0xf0, b'\xff'^0xf1=0x0e → 0xf00e |
| PX-4 | `ProcessXor(b'\xf0', Int16ub)` bytes len==1 | 编译期转 int 0xf0（对齐 Python L5389） |
| PX-5 | `ProcessXor(b'\x00\x00', Int16ub).parse(b'\x00\xff')` | fast-path 全零 bytes 不变换 → 0x00ff |
| PX-6 | `ProcessXor(my_field, Int16ub)` pad 是表达式 | 运行期 eval → resolve_xor_pad → apply |
| PX-7 | `ProcessXor(lambda ctx: ..., Int16ub)` pad 是 callable | **编译期拒绝**（ADR-014，parity 差异） |
| PX-8 | `ProcessXor(0xf0, Int16ub).build(0xf00f)` | build 0xf00f → b'\x00\xff' → XOR → b'\xf0\x0f' |
| PX-9 | `ProcessXor(0xf0, Int16ub).sizeof()` | 返回 2（转发 inner） |
| PX-10 | pad eval 得非 int/bytes | ConstructError::String（"ProcessXor needs integer or bytes pad"） |
| PR-1 | `ProcessRotateLeft(4, 1, Int16ub).parse(b'\x0f\xf0')` | group=1 查表：0x0f→0xf0, 0xf0→0x0f → b'\xf0\x0f' → 0xf00f |
| PR-2 | `ProcessRotateLeft(4, 2, Int16ub).parse(b'\x0f\xf0')` | group=2 amount%8==4≠0 走分支 4 → 0xff00 |
| PR-3 | `ProcessRotateLeft(0, 1, Int16ub).parse(b'\x0f\xf0')` | amount==0 分支 1 不变换 → 0x0ff0 |
| PR-4 | `ProcessRotateLeft(8, 1, Int16ub)` amount%8==0 group==1 | amount%8==0 但 group==1：实际 amount% (1*8)=0 → 走分支 1（amount==0） |
| PR-5 | `ProcessRotateLeft(4, 0, Int16ub)` group<1 | ConstructError::Rotation（"group size must be at least 1"） |
| PR-6 | `ProcessRotateLeft(4, 3, Bytes(2))` data len=2 % group=3 != 0 | ConstructError::Rotation（"data length must be a multiple of group size"） |
| PR-7 | `ProcessRotateLeft(4, 1, Int16ub).build(0xf00f)` | build 取负 amount=-4%(1*8)=4 → 旋转 → b'\x0f\xf0' |
| PR-8 | `ProcessRotateLeft(amount_expr, 1, Int16ub)` amount 是表达式 | 运行期 eval → rotate |
| PR-9 | `ProcessRotateLeft(16, 2, Int16ub)` amount=16, group=2 | amount%16=0 → 分支 1 不变 |
| PR-10 | `ProcessRotateLeft(lambda..., 1, ...)` amount/group 是 callable | **编译期拒绝**（ADR-014） |

### 5.10 DEV 实施清单

| 文件 | 内容 | 行数估 |
|------|------|-------|
| `nodes/process_xor.rs` | ProcessXorNode + XorPad + apply_xor + resolve_xor_pad + impl + 单元测试 | ~350 |
| `nodes/process_rotate_left.rs` | ProcessRotateLeftNode + ROTATION_TABLES const + rotate_left 4 分支 + impl + 单元测试 | ~420 |
| `nodes/mod.rs` | 新增 2 个 mod/enum 变体 + has_expressions 分支 | ~15 增量 |
| `compile.rs` | 新增 2 个 build_*_node 分支（pad/amount/group 编译期物化或 ExprProgram） | ~70 增量 |
| `error.rs` | 新增 ConstructError::String + ConstructError::Rotation 变体 + ExceptionClasses + Python 类映射 | ~40 增量 |
| `_descriptors.py` | ProcessXorDescriptor / ProcessRotateLeftDescriptor + 工厂函数 | ~50 增量 |
| `_errors.py` | StringError / RotationError Python 类（对齐 core.py L138/L140） | ~15 增量 |
| `__init__.py` | 导出 ProcessXor / ProcessRotateLeft / StringError / RotationError | ~10 增量 |

**Cargo.toml 不需新增依赖**（位运算 Rust 原生；ROTATION_TABLES const）。

**子流构造 API 确认**：`ParseStream::new(&[u8])` + `BuildStream::with_capacity(usize)` + `stream.data() -> &[u8]` + `stream.tell() -> usize` 全部已有（TransformNode 验证）。DEV 实施时确认 `stream.data()` 返回完整缓冲（stream.rs L335）。

### 5.11 Parity 测试模板

```python
def test_processxor_int_pad():
    # ProcessXor(0xf0, Int16ub).parse(b'\x00\xff') → 0xf00f
    assert_parity_case("""
    from construct import ProcessXor, Int16ub
    obj = ProcessXor(0xf0, Int16ub).parse(b'\\x00\\xff')
    """, expected_py=0xf00f)

def test_processxor_zero_pad_fastpath():
    # ProcessXor(0, Int16ub).parse(b'\x00\xff') → 0x00ff（不变换）
    assert_parity_case("""
    from construct import ProcessXor, Int16ub
    obj = ProcessXor(0, Int16ub).parse(b'\\x00\\xff')
    """, expected_py=0x00ff)

def test_processxor_bytes_pad():
    # ProcessXor(b'\xf0\xf1', Int16ub).parse(b'\x00\xff') → 0xf00e
    assert_parity_case("""
    from construct import ProcessXor, Int16ub
    obj = ProcessXor(b'\\xf0\\xf1', Int16ub).parse(b'\\x00\\xff')
    """, expected_py=0xf00e)

def test_processxor_build_inverse():
    # ProcessXor(0xf0, Int16ub).build(0xf00f) → b'\x00\xff'（XOR 对合）
    assert_parity_case("""
    from construct import ProcessXor, Int16ub
    out = ProcessXor(0xf0, Int16ub).build(0xf00f)
    """, expected_py=b'\\x00\\xff')

def test_processrotateleft_group1():
    # ProcessRotateLeft(4, 1, Int16ub).parse(b'\x0f\xf0') → 0xf00f
    assert_parity_case("""
    from construct import ProcessRotateLeft, Int16ub
    obj = ProcessRotateLeft(4, 1, Int16ub).parse(b'\\x0f\\xf0')
    """, expected_py=0xf00f)

def test_processrotateleft_group2():
    # ProcessRotateLeft(4, 2, Int16ub).parse(b'\x0f\xf0') → 0xff00
    assert_parity_case("""
    from construct import ProcessRotateLeft, Int16ub
    obj = ProcessRotateLeft(4, 2, Int16ub).parse(b'\\x0f\\xf0')
    """, expected_py=0xff00)

def test_processrotateleft_zero_amount():
    # ProcessRotateLeft(0, 1, Int16ub).parse(b'\x0f\xf0') → 0x0ff0
    assert_parity_case("""
    from construct import ProcessRotateLeft, Int16ub
    obj = ProcessRotateLeft(0, 1, Int16ub).parse(b'\\x0f\\xf0')
    """, expected_py=0x0ff0)

def test_processrotateleft_group_lt_1_raises():
    # ProcessRotateLeft(4, 0, Int16ub) → RotationError
    assert_parity_error("""
    from construct import ProcessRotateLeft, Int16ub, RotationError
    try: ProcessRotateLeft(4, 0, Int16ub).parse(b'\\x0f\\xf0')
    except RotationError: pass
    """)

def test_processrotateleft_build_negates_amount():
    # ProcessRotateLeft(4, 1, Int16ub).build(0xf00f) → b'\x0f\xf0'（build 取负）
    assert_parity_case("""
    from construct import ProcessRotateLeft, Int16ub
    out = ProcessRotateLeft(4, 1, Int16ub).build(0xf00f)
    """, expected_py=b'\\x0f\\xf0')
```

**Parity 重点**：
- PX-2/PX-5 fast-path（pad==0 / 全零 bytes 不变换）
- PR-1/PR-2 group=1 查表 vs group=2 通用分支
- PR-7 build 取负 amount 的对称性

---

## 6. 子任务 8.11：NamedTuple / Timestamp（P2 批次）

### 6.1 NamedTuple

#### 6.1.1 Python 参考实现摘要

`NamedTuple(tuplename, tuplefields, subcon)`（core.py L3381-3446）Adapter：
- subcon 必须是 Struct/Sequence/Array/GreedyRange（编译期校验）
- `factory = collections.namedtuple(tuplename, tuplefields)`
- `_decode`：Struct → `del obj["_io"]; factory(**obj)`；Sequence/Array/GreedyRange → `factory(*obj)`
- `_encode`：Struct → `Container({name: getattr(obj, name) for named subcon})`；Sequence/... → `list(obj)`
- tuplefields 可为 str（空格分隔）或 list

#### 6.1.2 关键设计决策：NamedTuple 走 Rust Node（§0.2 判据应用）

**ARCH 决策（已确认，8.0 分析报告 §2.5.1）**：NamedTuple 实现为 **Rust Node 变体**（非 AdapterCallbackNode）。

**理由**：
1. `collections.namedtuple` factory 是 **C 级元组子类构造**（CPython 内置），`factory.__call__(...)` 走 C API，不算额外 FFI（§0.2 判据 2）
2. NamedTuple 是核心库 Adapter（非用户自定义），与 Hex/Enum 同档
3. 避免 AdapterCallbackNode 的 `_decode` 回调开销

**construct-rs Struct 返回实例（非 Container）的处理**：Python NamedTuple._decode 对 Struct 分支做 `del obj["_io"]; factory(**obj)`（obj 是 Container dict）。construct-rs 的 StructNode.parse 返回用户实例（dataclass 实例，字段在 `__dict__`），**不含 `_io`**。因此 NamedTuple 对 Struct 分支需：按 tuplefields 名字逐个 `getattr(instance, name)` 提取，再 `factory(**kwargs)`。

#### 6.1.3 Rust Node 设计

```rust
/// NamedTuple 包装节点：把 inner（Struct/Sequence）结果转为 collections.namedtuple 实例。
///
/// 对应 Python construct `NamedTuple(tuplename, tuplefields, subcon)`（core.py L3381）。
/// 在 construct-rs 中实现为 Rust Node（namedtuple factory 是 C 级构造，§0.2 判据 2）。
///
/// # 三方法行为
///
/// - parse：inner.parse → 按模式提取字段 → factory(args/kwargs)
///   - Struct 模式：按 tuplefields 名字 getattr → factory(**kwargs)
///   - Sequence 模式：list 解包 → factory(*args)
/// - build：namedtuple 实例 → 按模式提取 → inner.build
///   - Struct 模式：按名字 getattr → 构造 dict → inner.build(dict)
///   - Sequence 模式：list(instance) → inner.build(list)
/// - sizeof：转发 inner.sizeof
///
/// # factory 物化
///
/// `collections.namedtuple(tuplename, tuplefields)` 在编译期（Python 描述符 __init__）
/// 调用一次，物化为 `Py<PyType>` 存入 Node。运行时 `factory.call(py, args)` 是 C API。
#[derive(Debug)]
pub struct NamedTupleNode {
    inner: Box<Node>,
    /// collections.namedtuple 工厂类（编译期物化）。
    factory: Py<PyType>,
    /// 字段提取模式（编译期从 inner 类型推断）。
    mode: NamedTupleMode,
    /// tuplefields 名字列表（Struct 模式用于 getattr）。
    /// Sequence 模式可为空（按位置解包）。
    field_names: Vec<Py<PyString>>,
}

/// NamedTuple 字段提取模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedTupleMode {
    /// inner 是 Struct（构造器工厂或 StructMixin 子类）→ 按名提取 dict → factory(**kwargs)。
    Struct,
    /// inner 是 Sequence/Array/GreedyRange → 按位置解包 → factory(*args)。
    Sequence,
}

impl NamedTupleNode {
    pub fn new(
        inner: Node,
        factory: Py<PyType>,
        mode: NamedTupleMode,
        field_names: Vec<Py<PyString>>,
    ) -> Self {
        Self { inner: Box::new(inner), factory, mode, field_names }
    }
    pub fn inner(&self) -> &Node { &self.inner }
    pub fn factory(&self) -> &Py<PyType> { &self.factory }
    pub fn mode(&self) -> NamedTupleMode { self.mode }
}
```

**`has_expressions` 集成**：`Node::NamedTuple(n) => n.inner().has_expressions()`（NamedTuple 不引入新表达式）。

**编译期 inner 类型校验**：`compile.rs` 识别 NamedTupleDescriptor 时，检查 inner 描述符是否为 Struct/Sequence/Array/GreedyRange（对齐 Python L3406）。Array/GreedyRange 归入 Sequence 模式（按位置解包）。

#### 6.1.4 Construct impl（关键路径摘要）

```rust
impl Construct for NamedTupleNode {
    fn parse<'py>(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
        let obj = self.inner.parse(py, stream, ctx, path)?;
        let factory_bound = self.factory.bind(py);
        let named = match self.mode {
            NamedTupleMode::Struct => {
                // Struct 返回实例：按 field_names getattr → kwargs dict → factory(**kwargs)
                let kwargs = PyDict::new_bound(py);
                for name in &self.field_names {
                    let val = obj.bind(py).getattr(name.bind(py))?;
                    kwargs.set_item(name.bind(py), val)?;
                }
                factory_bound.call((), Some(&kwargs))?
            }
            NamedTupleMode::Sequence => {
                // Sequence/Array/GreedyRange 返回 list：factory(*list)
                let list = obj.bind(py).downcast::<PyList>()
                    .map_err(|_| ConstructError::NamedTuple {
                        message: format!("NamedTuple Sequence mode expects list, got {:?}", obj.bind(py).repr()?),
                        path: path.to_string(),
                    })?;
                factory_bound.call(list, None)?
            }
        };
        Ok(named.unbind())
    }
    fn build(&self, py, obj, stream, ctx, path) -> Result<(), ConstructError> {
        let build_obj: Py<PyAny> = match self.mode {
            NamedTupleMode::Struct => {
                // namedtuple 实例 → 按名字 getattr → dict → inner.build(dict)
                let kwargs = PyDict::new_bound(py);
                for name in &self.field_names {
                    let val = obj.getattr(name.bind(py))?;
                    kwargs.set_item(name.bind(py), val)?;
                }
                kwargs.into_py(py)
            }
            NamedTupleMode::Sequence => {
                // namedtuple 实例 → list(instance) → inner.build(list)
                // namedtuple 是 tuple 子类，list(nt) 转 list
                let seq = pyo3::types::PyList::sequence_to_list(obj)?;
                seq.into_py(py)
            }
        };
        self.inner.build(py, build_obj.bind(py), stream, ctx, path)
    }
    fn sizeof(&self, ctx) -> Result<usize, ConstructError> { self.inner.sizeof(ctx) }
}
```

**`factory.call((), Some(&kwargs))`**：pyo3 调 `PyType.__call__`（C API），对标准 namedtuple 是 C 级元组子类构造。`getattr(instance, name)` 是 C API（实例 `__dict__` 查找）。

#### 6.1.5 §0 原则对照表

| §0 原则 | NamedTupleNode |
|---------|----------------|
| #1 一次 FFI | ✅ 严格 1 次。inner.parse/build Rust 内；`PyType::call` / `getattr` / `PyDict::set_item` 全 C API（§0.2 判据 2，namedtuple 是 CPython 内置） |
| #2 无中间表示层 | ✅ PyDict（kwargs）是 Python 对象本身；namedtuple 实例是最终用户对象 |
| #3 输入输出无 trait 抽象 | ✅ 仅 `Box<Node>` |
| #4 pyo3 核心依赖 | ✅ PyType/getattr/PyDict 全 pyo3 API |
| #5 mashumaro API | ✅ NamedTuple 是字段描述符 |
| #6 enum_dispatch | ✅ 1 个新 Node |
| #7 Result + path | ✅ ConstructError::NamedTuple 携带 path |
| #8 Stream 纯 Rust | ✅ 透传 inner.stream |

#### 6.1.6 边界条件清单

| 编号 | 场景 | 预期行为 |
|------|------|---------|
| NT-1 | `NamedTuple("coord", "x y z", Byte[3]).parse(b'123')` | 返回 coord(x=49, y=50, z=51)（Array→Sequence 模式） |
| NT-2 | `NamedTuple("coord", "x y z", Struct).parse(...)` Struct 模式 | 返回 coord(x=..., y=..., z=...)（按名 getattr） |
| NT-3 | `NamedTuple("coord", ["x","y","z"], Byte[3])` tuplefields 是 list | 编译期统一处理（list 与空格分隔 str 等价） |
| NT-4 | `NamedTuple("c", "x y", GreedyRange(Byte)).parse(b'\x01\x02')` | GreedyRange → Sequence 模式 → coord(x=1, y=2) |
| NT-5 | inner 非 Struct/Sequence/Array/GreedyRange | **编译期拒绝**（NamedTupleError，对齐 Python L3407） |
| NT-6 | NamedTuple over Struct，实例缺某 field_name | getattr 失败 → ConstructError::Generic（AttributeError 转换） |
| NT-7 | `NamedTuple("c", "x y", Byte[3]).build(coord(x=1,y=2))` | list(coord)=[1,2] → inner.build([1,2]) |
| NT-8 | `NamedTuple("c", "x y", Byte[3]).sizeof()` | 返回 3（转发 inner Array sizeof） |
| NT-9 | NamedTuple 嵌入 Struct（`field(NamedTuple(...))`） | StructNode 遍历到 NamedTuple 字段 → NamedTuple.parse 返回 namedtuple 实例 |
| NT-10 | NamedTuple over Struct，实例含 tuplefields 之外字段（如 RO/Computed） | construct-rs **忽略**额外字段（按 tuplefields 名字 getattr）。**parity 差异（C-2）**：Python `factory(**obj)`（core.py L3416）传 Container 所有字段，多余字段会触发 `TypeError: __init__() got an unexpected keyword argument`（严格）；construct-rs 宽松忽略。用户从 Python 迁移含额外字段的 Struct 不会报错（行为差异，见 §7.5） |

#### 6.1.7 DEV 实施清单（NamedTuple 部分）

| 文件 | 内容 | 行数估 |
|------|------|-------|
| `nodes/named_tuple.rs` | NamedTupleNode + NamedTupleMode + impl + 单元测试 | ~300 |
| `nodes/mod.rs` | 新增 mod/enum 变体 + has_expressions 分支 | ~12 增量 |
| `compile.rs` | 新增 build_named_tuple_node 分支（inner 类型校验 + factory 物化 + field_names 解析） | ~70 增量 |
| `error.rs` | 新增 ConstructError::NamedTuple 变体 + ExceptionClasses + Python 类映射 | ~25 增量 |
| `_descriptors.py` | NamedTupleDescriptor + 工厂函数（调 collections.namedtuple） | ~50 增量 |
| `_errors.py` | NamedTupleError Python 类（对齐 core.py L120） | ~10 增量 |
| `__init__.py` | 导出 NamedTuple / NamedTupleError | ~8 增量 |

### 6.2 Timestamp（Python 层 macro，arrow 依赖）

#### 6.2.1 Python 参考实现摘要

`Timestamp(subcon, unit, epoch)`（core.py L3449-3520）是**函数**（非类），返回 `TimestampAdapter`（Adapter 子类）实例：
- 依赖 `arrow` 第三方包（`import arrow` 在函数体顶部，失败 → ImportError）
- 两模式：
  - **msdos**（unit=="msdos" 或 epoch=="msdos"）：用 BitStruct（7+4+5+5+6+5 bits）+ 自定义 `_decode`/`_encode`（`arrow.Arrow(1980,1,1).shift(...)` / 反向）。2 秒分辨率，1980 epoch
  - **epoch**（默认）：`epoch` 为 int（年）转 `arrow.Arrow(epoch,1,1)`；`_decode`: `epoch.shift(seconds=obj*unit)`；`_encode`: `int((obj-epoch).total_seconds()/unit)`

#### 6.2.2 设计决策：Timestamp 走 Python 层 macro（PM 决策 D-3 已接受）

**ARCH 决策（已确认，PM 决策 D-3）**：Timestamp 实现为 **Python 层 macro 函数 + Adapter 子类**，不实现为 Rust Node。

**理由**：
1. `arrow` 是第三方 Python 包（非 Rust），Arrow datetime 算术（`.shift()`/`.total_seconds()`）无法 Rust 化
2. `_decode`/`_encode` 涉及 Arrow 对象构造与运算，是 100% Python 代码
3. msdos 模式内部用 BitStruct（已有 Rust Node）+ Python decode 逻辑
4. **性能不设硬门禁**（Python 层 arrow 路径，与 Validator 同档）

**§0 合规**：⚠️ 嵌入 Struct 时 2 FFI（parse 入口 + `_decode` 回调）。PM 决策 D-3 接受（用户域后处理，arrow 是用户依赖）。

#### 6.2.3 Python 层 Timestamp macro 实现

```python
# construct-rs/python/construct/_macros.py（追加，与 AlignedStruct 同文件）
# 注（F-1 修正）：import arrow 延迟到 Timestamp 函数体内（对齐 Python core.py L3478），
#   避免 arrow 缺失时整个 _macros.py 加载失败（会破坏共享的 AlignedStruct，P0 已验收）。

from ._adapters import Adapter
from . import BitStruct, BitsInteger, Container


def Timestamp(subcon, unit, epoch):
    """Datetime as Arrow object.

    对应 Python construct core.py:3453 的 Timestamp 函数（macro）。
    construct-rs 实现为 Python 层（arrow 依赖，无法 Rust 化）。

    嵌入 Struct 时走 AdapterCallbackNode（2 FFI，用户主动选择 arrow = 接受折衷）。
    性能不设硬门禁（Python 层 arrow 路径）。

    :param subcon: Int*/Float* 或 Int32ub（msdos）
    :param unit: int/float（秒/毫秒/微秒分辨率）或 "msdos"
    :param epoch: int（年）/ Arrow 实例 / "msdos"

    :raises ImportError: arrow 未安装（用户 pip install arrow）
    :raises TimestampError: 参数类型错误

    Example::

        >>> from construct import Timestamp, Int64ub
        >>> d = Timestamp(Int64ub, 1., 1970)
        >>> d.parse(b'\\x00\\x00\\x00\\x00ZIz\\x00')
        <Arrow [2018-01-01T00:00:00+00:00]>
    """
    import arrow
    if not isinstance(unit, (int, float, str)):
        raise TimestampError("unit must be one of: int float string")
    if not isinstance(epoch, (int, arrow.Arrow, str)):
        raise TimestampError("epoch must be one of: int Arrow string")

    if unit == "msdos" or epoch == "msdos":
        st = BitStruct(
            "year" / BitsInteger(7),
            "month" / BitsInteger(4),
            "day" / BitsInteger(5),
            "hour" / BitsInteger(5),
            "minute" / BitsInteger(6),
            "second" / BitsInteger(5),
        )

        class MsdosTimestampAdapter(Adapter):
            def _decode(self, obj, context, path):
                return arrow.Arrow(1980, 1, 1).shift(
                    years=obj.year, months=obj.month - 1, days=obj.day - 1,
                    hours=obj.hour, minutes=obj.minute, seconds=obj.second * 2,
                )

            def _encode(self, obj, context, path):
                t = obj.timetuple()
                return Container(
                    year=t.tm_year - 1980, month=t.tm_mon, day=t.tm_mday,
                    hour=t.tm_hour, minute=t.tm_min, second=t.tm_sec // 2,
                )

        return MsdosTimestampAdapter(st)

    if isinstance(epoch, int):
        epoch = arrow.Arrow(epoch, 1, 1)

    class EpochTimestampAdapter(Adapter):
        def _decode(self, obj, context, path):
            return epoch.shift(seconds=obj * unit)

        def _encode(self, obj, context, path):
            return int((obj - epoch).total_seconds() / unit)

    return EpochTimestampAdapter(subcon)
```

**`import arrow` 失败处理（F-1 修正）**：`arrow` 是用户依赖（construct-rs 不打包）。`import arrow` 放在 `Timestamp` **函数体内**（对齐 Python core.py L3478），arrow 缺失时**仅 `Timestamp(...)` 调用失败**（抛 `ModuleNotFoundError`），不影响 `_macros.py` 同模块的其他宏——特别是 P0 已验收的 `AlignedStruct`。

**为什么不用顶部 import**：`_macros.py` 是共享模块，顶部 `import arrow` 失败会导致**整个模块加载失败**，连带破坏与本任务无关的 `AlignedStruct`（P0 已验收功能的回归）。这是 REV F-1 驳回的根因。Python 原版选择函数体内延迟导入（L3478），正是为了隔离 arrow 缺失的影响范围——construct-rs 对齐此设计。v1 设计曾错误地认为"顶部 import 更早失败是优点"，但忽略了共享模块的污染问题（自相矛盾，已修正）。

**BitStruct 已有 Rust Node**：msdos 模式的 `BitStruct(...)` 在 construct-rs 是 Rust Node（Phase 3）。`MsdosTimestampAdapter(st)` 嵌入 Struct 时，st 部分在 Rust 内 parse，`_decode` 部分是 Python 回调（AdapterCallbackNode）。

#### 6.2.4 §0 原则对照表

| §0 原则 | Timestamp（Python 层） |
|---------|----------------------|
| #1 一次 FFI | ⚠️ **2 次**（parse 入口 + `_decode` 回调）。用户主动选 arrow = 接受折衷（ADR-022 §0 论证，PM 决策 D-3 接受） |
| #2 无中间表示层 | ✅ Arrow 对象是最终用户对象 |
| #3 输入输出无 trait 抽象 | ✅ 复用 AdapterCallbackNode |
| #4 pyo3 核心依赖 | ✅ AdapterCallbackNode 持 Py<PyAny> |
| #5 mashumaro API | ✅ Timestamp 是字段描述符工厂 |
| #6 enum_dispatch | N/A（Python 层无 Node） |
| #7 Result + path | ✅ 跨 FFI 转 Generic |
| #8 Stream 纯 Rust | ✅ BitStruct 部分纯 Rust；arrow 算术不操作 stream |

#### 6.2.5 边界条件清单

| 编号 | 场景 | 预期行为 |
|------|------|---------|
| TS-1 | `Timestamp(Int64ub, 1., 1970).parse(b'\\x00\\x00\\x00\\x00ZIz\\x00')` | 返回 Arrow(2018-01-01 00:00:00+00:00) |
| TS-2 | `Timestamp(Int32ub, "msdos", "msdos").parse(b'H9\\x8c"')` | 返回 Arrow(2016-01-25 17:33:04+00:00)（2 秒分辨率） |
| TS-3 | `arrow` 未安装 | ImportError（用户 pip install arrow） |
| TS-4 | `Timestamp(Int8ub, unit, epoch)` unit 非 int/float/str | TimestampError（编译期） |
| TS-5 | Timestamp 嵌入 Struct（`field(Timestamp(...))`） | 编译为 AdapterCallbackNode，2 FFI |
| TS-6 | msdos build：Arrow → Container(year=..., second=sec//2) | 2 秒分辨率（sec//2 取整，对齐 Python L3501） |

#### 6.2.6 DEV 实施清单（Timestamp 部分）

| 文件 | 内容 | 行数估 |
|------|------|-------|
| `python/construct/_macros.py`（追加） | Timestamp macro 函数 + MsdosTimestampAdapter + EpochTimestampAdapter | ~70 |
| `python/construct/_adapters.py` | Adapter 基类（若 P1 Validator 已建则复用；否则同步建） | （P1 已建） |
| `python/construct/requirements.txt`（或 setup.py extras） | `arrow` 依赖声明（用户侧） | ~2 |
| `_descriptors.py` | Timestamp 工厂函数（调 macro） | ~10 增量 |
| `_errors.py` | TimestampError Python 类（对齐 core.py L128） | ~10 增量 |
| `__init__.py` | 导出 Timestamp / TimestampError | ~8 增量 |

**Cargo.toml 不需新增依赖**（arrow 是 Python 用户依赖，非 Rust）。

#### 6.2.7 Parity 测试模板

```python
def test_timestamp_epoch_parse():
    # Timestamp(Int64ub, 1., 1970).parse(b'\x00\x00\x00\x00ZIz\x00') → Arrow(2018-01-01)
    # 注：需 pip install arrow（测试前置）
    assert_parity_case("""
    from construct import Timestamp, Int64ub
    import arrow
    obj = Timestamp(Int64ub, 1., 1970).parse(b'\\x00\\x00\\x00\\x00ZIz\\x00')
    """, check_only=["obj.year==2018", "obj.month==1", "obj.day==1"])

def test_timestamp_msdos_parse():
    # Timestamp(Int32ub, "msdos", "msdos").parse(b'H9\\x8c"') → Arrow(2016-01-25 17:33:04)
    assert_parity_case("""
    from construct import Timestamp, Int32ub
    obj = Timestamp(Int32ub, 'msdos', 'msdos').parse(b'H9\\x8c\"')
    """, check_only=["obj.year==2016", "obj.month==1", "obj.day==25", "obj.hour==17"])

def test_timestamp_build_epoch():
    # Timestamp(Int64ub, 1., 1970).build(Arrow(2018-01-01)) → b'\x00\x00\x00\x00ZIz\x00'
    assert_parity_case("""
    from construct import Timestamp, Int64ub
    import arrow
    out = Timestamp(Int64ub, 1., 1970).build(arrow.Arrow(2018,1,1))
    """, expected_py=b'\\x00\\x00\\x00\\x00ZIz\\x00')
```

**Parity 重点**：
- TS-1/TS-2 epoch vs msdos 两模式
- TS-6 msdos 2 秒分辨率（second//2 取整）

---

## 7. 跨子任务实现汇总

### 7.1 Node enum 变体增量

| 批次 | 子任务 | 新增 Node 变体 | 当前 enum 总数 |
|------|--------|---------------|---------------|
| — | P0（已 ACCEPTED） | Const/Default/Check/Aligned/Hex/HexDump/Checksum/Terminated/Probe（9） | 48 |
| **P1** | 8.2 | Enum / FlagsEnum / Mapping（3） | 51 |
| **P1** | 8.3 | OneOf / NoneOf（2，Validator 是 Python 层无 Node） | 53 |
| **P1** | 8.6 | Union（1） | 54 |
| **P1** | 8.7 | Sequence（1） | 55 |
| **P1** | 8.12 | ProcessXor / ProcessRotateLeft（2） | 57 |
| **P2** | 8.11 | NamedTuple（1，Timestamp 是 Python 层无 Node） | **58** |

**P1+P2 净增 10 个 Node 变体（48 → 58）。**

### 7.2 ConstructError 变体增量

| 变体 | 子任务 | Python 类映射 |
|------|--------|-------------|
| `Mapping` | 8.2 | MappingError |
| `Validation` | 8.3 | ValidationError |
| `Union` | 8.6 | UnionError |
| `String` | 8.12 | StringError |
| `Rotation` | 8.12 | RotationError |
| `NamedTuple` | 8.11 | NamedTupleError |

（TimestampError 走 Python 层，不进 Rust ConstructError。）

### 7.3 依赖与基础设施

**Cargo.toml**：本批次**不需新增任何 Rust crate**（PyDict/PyFrozenSet/PyType C API + ExprProgram + stream seek/tell + 子流构造全部已有）。与 P0 的 Checksum（新增 5 crate）形成对比——P1+P2 是"纯模式应用"批次。

**Python 依赖**：
- `arrow`（Timestamp 用户依赖，construct-rs 不打包，用户 `pip install arrow`）
- 无新增 Rust→Python 桥接

**新 API**：无（stream.seek/tell/data、Context::new_child/set_field_at、compute_ro_value 全部已有）。

### 7.4 实施顺序与并行性

```
P1（5 子任务可并行，无相互依赖）：
  8.2 (Enum/FlagsEnum/Mapping)      ← dict 物化模式
  8.3 (OneOf/NoneOf/Validator)      ← frozenset 物化 + Python 基类
  8.6 (Union)                       ← seek 多视角
  8.7 (Sequence)                    ← PyList sink
  8.12 (ProcessXor/ProcessRotateLeft) ← 位运算
      ↓
P2（串行，依赖 P1 的 Validator/Adapter 基类）：
  8.11 (NamedTuple/Timestamp)       ← NamedTuple 依赖 Struct/Sequence 稳定；
                                        Timestamp 依赖 _adapters.py Adapter 基类（P1 8.3 建）
```

**关键依赖链**：8.11 Timestamp 的 `_macros.py` 依赖 `_adapters.py` 的 `Adapter` 基类——若 8.3 已建（Validator 复用 Adapter/SymmetricAdapter），8.11 直接复用。建议 8.3 先于 8.11。

### 7.5 测试策略

- **parity**：每子任务含 parity 测试模板（§1.9 / §2.8 / §3.9 / §4.10 / §5.11 / §6.1.7 / §6.2.7）
- **bench**：核心场景（Enum 命中/未命中、Sequence 多字段、Union 多 subcon、ProcessXor 大数据）进 `docs/perf-scenarios.csv`
- **已知 parity 差异**（用户文档标注）：
  - Default/Check/Union.parsefrom/Probe/Padding padfunc/ProcessXor pad 不接 lambda（ADR-014）
  - Sequence 不支持 `>>` 操作符
  - Validator 不导出 ExprValidator
  - Timestamp 需 pip install arrow；`import arrow` 延迟到 Timestamp 函数体内（F-1，对齐 core.py L3478），arrow 缺失仅 Timestamp 调用失败
  - Container/HexDisplayed repr 格式 vs construct-rs dict/对象 repr（Probe/Union 已知差异）
  - **Sequence 含 RO 字段（Check/Computed/Tell 等）的 build**（C-1，§4.8 SQ-7）：construct-rs RO 字段不从 list 取值（走 compute_ro_value），list 不含 RO 字段占位；Python（core.py L2410）对所有 subcons 都 `next(objiter)`，list 需含 RO 字段占位（如 None）。用户迁移需调整 build 输入。
  - **NamedTuple over Struct 的多余字段处理**（C-2，§6.1.6 NT-10）：construct-rs 只传 tuplefields 命名的字段（忽略实例 `__dict__` 中的 RO/Computed 等额外字段）；Python（core.py L3416）`factory(**obj)` 传 Container 所有字段，多余字段报 TypeError。construct-rs 更宽松。
  - **Union build 的 context 合并与 flagbuildnone**（C-3，§3.4 + UN-14/UN-15）：construct-rs child_ctx 仅含被选中 subcon 字段（Python `context.update(obj)` 合并所有字段，core.py L3732）；construct-rs 不处理 flagbuildnone subcon 的缺键（Python 用 `obj.get(name, None)`，core.py L3734-3735）。跨 subcon 引用与 flagbuildnone subcon 的 build 用例有行为差异。

---

## 8. PM 决策点（P1+P2 新增）

8.0 分析报告 D-1~D-7 全部已确认。本批次**无新增强制决策点**——以下为 ARCH 自主决策（PM 知悉，若有异议可标记）：

### 8.1 ARCH 自主决策（PM 知悉）

| ID | 决策 | ARCH 选择 | 理由 |
|----|------|----------|------|
| AD-P1-1 | OneOf/NoneOf 分开两 Node 变体 vs 合并 MembershipCheckNode | **分开**（OneOfNode / NoneOfNode） | 语义清晰，错误消息明确，Node enum 习惯一构造器一变体 |
| AD-P1-2 | Sequence 是否复用 StructNode 代码 | **不复用代码，共享模式** | 输出 sink 不同（PyList vs 实例 __dict__），强行共享引入 sink trait 抽象违反 §0 #3；参考 FocusedSeq 独立实现 |
| AD-P1-3 | EnumNode decmapping 值是否预构造 EnumIntegerString | **编译期预构造**（存入 dict） | parse 时直接 get_item 返回，运行时不调 `EnumIntegerString.new`（仅 fallback 路径调 EnumInteger 构造） |
| AD-P1-4 | ProcessXor pad 为表达式时运行期 resolve | **XorPad::Expr 走 eval_expr_any + resolve_xor_pad** | 表达式求值得 int/bytes 后递归 apply_xor（与 Switch keyfunc 混合求值同脉络） |
| AD-P1-5 | NamedTuple over Struct 的字段提取 | **按 tuplefields 名字 getattr**（非 `**__dict__`） | construct-rs Struct 返回实例（非 Container），实例 `__dict__` 可能含 RO/Computed 字段。**parity 差异（C-2 修正）**：Python `factory(**obj)`（core.py L3416）传 Container **所有**字段（除 `_io`），多余字段会触发 `TypeError`（严格）；construct-rs 只传 tuplefields 命名的字段，**忽略**额外字段（宽松）。两者**不对齐**，construct-rs 更宽松。这是 construct-rs 的合理工程选择（实例 `__dict__` 含 RO/Computed 字段时按 tuplefields 提取避免干扰），但**非对齐 Python**（v1 理由"对齐 factory(**obj) 语义"描述错误，已修正）。见 §6.1.6 NT-10 + §7.5 |

### 8.2 需 PM 留意的风险点

1. **Enum/FlagsEnum 性能下限**：若 Python construct 的 dict lookup 本身已是 C 级（dict 是 CPython 优化重点），Enum 加速比可能仅 4-6x（非 10x）。**对策**：bench 覆盖命中/未命中/multi-key 场景；若 <4x 由 PM 决策接受（与 Phase 6 的 6 个 <10x 同档处理）
2. **Sequence RO 字段处理**：SequenceField.field_kind 复用 StructNode 的 FieldKind——DEV 实施时确认 FieldKind 是否 pub 可见（若否，提取到 common.rs，微小重构）。**不阻塞设计**
3. **Timestamp arrow 依赖**：CI 环境需 `pip install arrow`（否则 Timestamp parity 测试跳过）。**对策**：PM 在 testing/ci 配置中加 arrow 到 requirements

---

## 附录 A：参考文件索引

| 文件 | 用途 |
|------|------|
| `construct/construct/core.py` | 12 构造器 Python 源码（Enum L1920 / FlagsEnum L2018 / Mapping L2112 / Validator L849 / ExprValidator L6311 / OneOf L6320 / NoneOf L6342 / Union L3641 / Sequence L2329 / NamedTuple L3381 / Timestamp L3449 / ProcessXor L5357 / ProcessRotateLeft L5424 / EnumInteger L1899 / EnumIntegerString L1904） |
| `docs/design/模块设计/模块设计-Phase8-P0.md` | P0 设计（9 Node 模式参照：ConstNode Subconstruct 包装 / HexNode Py<PyType> 物化 / ChecksumNode / AlignedNode / ProbeNode） |
| `docs/design/模块设计/模块设计-Adapter核心.md` | AdapterCallbackNode + Validator 复用基础（Phase 6.3） |
| `docs/design/模块设计/模块设计-Array.md` | SequenceNode 的 list sink + StopField 捕获 + lazy path 模式参照 |
| `construct-rs/src/nodes/focused_seq.rs` | SequenceNode 的 context nesting（new_child + set_field_at）直接参照 |
| `construct-rs/src/nodes/transform.rs` | ProcessXor/ProcessRotateLeft 的子流构造 + const 查表模式参照 |
| `construct-rs/src/nodes/struct_node.rs` | SequenceField.field_kind + compute_ro_value RO 路径参照 |
| `construct-rs/src/nodes/mod.rs` | Node enum（48 变体）+ Construct trait + compute_ro_value 公共 API |
| `docs/decisions/ADR-022` | Validator/Timestamp Python 层化（用户面 Adapter） |
| `docs/decisions/ADR-014` | 表达式不接 callable（Union.parsefrom / ProcessXor padfunc 同硬约束） |
| `docs/decisions/ADR-017` | Vec 中转 PyList（Sequence PyList 构建模式） |

---

> **报告完成时间**：2026-07-31
> **下一步**：PM DESIGN_REVIEW 分派 REV → REV 检视 §0 对照表 + parity 模板 + 边界条件完整性 → CODING（DEV 按 §0.5 顺序实施）
