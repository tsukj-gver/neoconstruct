---
id: QUERY-phase8-probe-namedtuple-deviation
status: active
phase: "8"
depends_on:
  - DESIGN-Phase8-P0
  - DESIGN-Phase8-P1P2
last_updated: 2026-07-31
---

# 设计质疑回应：Phase 8 P0/P1P2 两项 DEV 实施偏离

> **触发**：DEV 在 8.P0 / 8.P1P2 实施阶段标注 2 个 [设计质疑]，PM 转发 ARCH 必须回应
> （AGENTS.md §1 工作流管道 + architect.md §响应设计质疑）。
>
> **ARCH 判定**：
> - 质疑 1（D-P0-2 Probe.into）：**方案 A 接受 DEV 实施**（FieldName 代替 ExprProgram）
> - 质疑 2（AD-P1-5 NamedTuple build）：**方案 A 接受 DEV 实施**（直接传实例代替构造 dict）
>
> 两项均需同步更新设计文档对应章节（PM 确认后由 ARCH 统一修改）。

---

## 0. 判断维度与 L-14 交叉验证

ARCH 对每个质疑按以下维度判断（architect.md §响应设计质疑 + experiences.md §L-14 对策）：

1. **parity 影响**（对照 Python construct 2.10.70 真实行为）
2. **API 合理性**（与 construct-rs 整体语法/已有模式一致性）
3. **§0 合规**（AGENTS.md §0 核心原则 1-8）
4. **实施成本**（DEV 工作量 + 基础设施扩展）
5. **用户迁移影响**（从 Python construct 迁移的语义差异）
6. **L-14 交叉验证**：识别"设计文档原方案"是否基于单一实现路径推断，是否漏了替代路径

---

## 1. 质疑 1：Probe.into 字段类型（D-P0-2）

### 1.1 DEV 偏离内容摘要

**位置**：`construct-rs/src/nodes/probe.rs` 模块级注释 + `src/nodes/mod.rs` Node::Probe 变体
+ `src/compile.rs` Probe 分支。

**偏离**：

| 维度 | 设计文档 §5.2.2 / §10 D-P0-2 原文 | DEV 实施 |
|------|--------------------------------|---------|
| 字段类型 | `into: Option<ExprProgram>` | `into: Option<FieldName>` |
| 扩展 | 推荐 P0 内扩展 `eval_expr_any` ~30 行（D-P0-2） | 改用 FieldName，避免扩展 expr.rs |
| 表达式组合 | 支持 `Probe(count + 1)`（算术） | ❌ 不支持（仅单字段名引用） |
| 任意类型字段 | 需 `eval_expr_any`（返回 PyObject） | ✅ 支持（`ctx.get_field` 返回 PyObject） |
| ADR-014 一致性 | ✅（ExprProgram 不接 callable） | ✅（FieldName 不接 callable） |

**DEV 偏离理由**（P0-DEV实施.md L107-116）：
1. Probe.into 实际用例是"调试打印字段值"（任意类型），ExprProgram 仅支持 i64 求值
2. 要支持 PyObject 需扩展 ~30 行 `eval_expr_any` API
3. FieldName 路径走 `ctx.get_field()` 直接返回 PyObject（与 Switch FieldRef 同模式），无类型限制
4. Probe 仅"打印"，不参与计算字段长度/计数，ExprProgram 的算术能力对 Probe 无意义

### 1.2 设计文档原文

**§5.2.2 ProbeNode struct（L1473-1487）**：

```rust
#[derive(Debug)]
pub struct ProbeNode {
    /// 可选表达式：求值后 repr 打印。None 表示打印整个 context。
    into: Option<ExprProgram>,
    /// 可选 peek 字节数：None 表示不 peek。
    lookahead: Option<usize>,
}

impl ProbeNode {
    pub fn new(into: Option<ExprProgram>, lookahead: Option<usize>) -> Self { ... }
    pub fn into(&self) -> Option<&ExprProgram> { self.into.as_ref() }
    ...
}
```

**§5.2.2 表达式约束说明（L1469-1472）**：
> `into` 是 Python context lambda；construct-rs 强制编译为 ExprProgram
> （PM 决策 D-6 已接受，不支持 callable）。`lookahead` 是编译期 usize 常量。

**§10 D-P0-2 决策（L1952-1961）**：
> **问题**：Probe.into 表达式求值结果可能是任意 Python 对象（非 i64），需 `eval_expr_any -> Py<PyAny>`。
> **ARCH 推荐**：**P0 内推荐扩展**。理由：Probe.into 是用户调试常用场景；当前 ExprProgram 仅 i64 严重限制 Probe 可用性；扩展成本低（~30 行）。
> **若 PM 决定 P0 内不扩展**：Probe.into 暂仅支持 i64 求值（用户调试 int 字段可用，其他类型需等 P1+）。

**PM 决策 D-6（总纲 L74）**：Probe into 支持 ExprProgram → 接受（基于 8.0 分析报告时的推荐）。
**PM 后续决策（P0-VET驳回修复.md L20）**：Probe.into 设计质疑（D-P0-2）→ **接受 FieldName 方案**，设计文档 ARCH 后续更新。

### 1.3 Python 参考实现事实（L-14 交叉验证基础）

**`construct/construct/debug.py` Probe 类（L6-95）**：

- `Probe(into=None, lookahead=None)`：`into` 是 context lambda（任意 callable）
- `printout` 中 `subcontext = self.into(context)`（L89）——调用 callable 求值
- **docstring 仅提供 2 个示例**（L15-47）：
  - `Probe(lookahead=32)` —— 无 into
  - `Probe(this.count)` —— **单字段引用**（`this.count` 是 construct 表达式对象，非 lambda）

**关键事实**：Python 原版 docstring 中**没有**算术组合示例（如 `Probe(this.count + 1)`）。
实际用例是"打印某字段值"（单字段引用），而非"打印算术表达式结果"。

**ADR-014 已排除 callable**：`Probe(lambda ctx: complex_expr)` 在 construct-rs 不支持
（与 Rebuild/Check/Default/Checksum.StreamRange 同硬约束）。这是已知 parity 差异，
与 into 字段类型选择无关（FieldName 与 ExprProgram 都不接 callable）。

### 1.4 L-14 交叉验证（硬约束检查）

设计 §5.2.2 选 `ExprProgram` 是基于**单一实现路径**（"ExprProgram 已有 GetInt 指令，
扩展为 GetField 返回 PyObject 即可"，D-P0-2 L1959）推断的，未交叉验证替代路径：

| 实现路径 | 描述 | 任意类型字段 | 算术组合 | 扩展成本 | §0/ADR 合规 |
|---------|------|------------|---------|---------|------------|
| A（设计原方案）| ExprProgram + 扩展 `eval_expr_any` | ✅（需扩展）| ✅ | ~30 行 | ✅ |
| **B（DEV 实施）**| **FieldName + `ctx.get_field`** | **✅（原生）** | ❌ | **0 行** | **✅** |
| C（双轨）| `enum { Field(FieldName), Expr(ExprProgram) }` | ✅ | ✅ | ~40 行（enum + 分派）| ✅ |

**路径 B 是否被 §0 或 ADR 排除？**
- §0 #3（输入输出无 trait 抽象）：FieldName 不是 trait，是具体类型（Phase 1 引入，Switch FieldRef 已用），合规
- ADR-006（表达式 VM 仅 i64）：FieldName 不走 ExprProgram VM，是独立的字段引用机制，不违反
- ADR-014（不接 callable）：FieldName 不接 callable，合规

**结论**：路径 B 未被任何硬约束排除。路径 A 的"算术组合"能力在 Probe 场景**无实际用例支撑**
（Python docstring 无此类示例，Probe 是调试构造器用户不会写复杂表达式）。

### 1.5 决策：方案 A（接受 DEV 实施）

**理由**（按 §0 判断维度）：

1. **parity 影响**：
   - Python 实际用例（`Probe(this.count)` 单字段引用）→ construct-rs `Probe(count)` 完全覆盖（FieldName）
   - 任意类型字段（int/bytes/str/dict）→ FieldName 原生支持（`ctx.get_field` 返回 PyObject），**比 ExprProgram i64 限制更优**
   - `Probe(lambda ctx: ...)` → 两者都不支持（ADR-014 一致）
   - `Probe(count + 1)`（算术组合）→ FieldName 不支持，但**无 Python 实际用例**，且 Probe 是调试构造器（用户可改用 Check 或在 Python 侧预计算）

2. **API 合理性**：
   - FieldName 与 Switch FieldRef（`模块设计-Conditional.md` SwitchKey::FieldRef）同模式，construct-rs 内已有先例
   - 与 Phase 2 废弃 `this` 后的语法一致（字段名直接引用，非 `this.xxx`）
   - ExprProgram 仅 i64，对"打印任意类型字段"反而受限

3. **§0 合规**：FieldName 走 `ctx.get_field`（C API，§0.2 判据 2），不违反任何 §0 原则

4. **实施成本**：零扩展（FieldName 已有）；路径 A 需 ~30 行 `eval_expr_any` 扩展
   - 注：`eval_expr_any` 在 P1 已实施（ProcessXor XorPad::Expr 路径用，P1P2-DEV实施 L23），
     但 Probe 在 P0 先做，彼时该基础设施尚未存在——DEV 选择 FieldName 避免了 P0 内的前置依赖

5. **用户迁移影响**：
   - `Probe(this.count)` → `Probe(count)`（与 Phase 2 整体 `this` 废弃迁移一致）
   - `Probe(lookahead=32)` → 不变
   - 无额外迁移负担

**不选方案 C（双轨）的理由**：Probe 是调试构造器（设计 §5.4 明确"不设硬性能门禁"），
双轨增加 Node 字段复杂度（enum + 编译分派）收益低。若未来有强需求（用户反馈需要算术组合），
可后续扩展为双轨——但当前无证据支撑。

### 1.6 需更新设计文档（PM 确认后由 ARCH 统一修改）

| 章节 | 更新内容 |
|------|---------|
| **§5.2.2 ProbeNode struct** | `into: Option<ExprProgram>` → `into: Option<FieldName>`；`new`/`into()` 返回类型同步；表达式约束说明改为"into 是 Option<FieldName>（单字段引用），不支持 callable（ADR-014），不支持算术组合" |
| **§5.2.2 printout 实现要点 L1493** | "into 是 ExprProgram 时求值后 print" → "into 是 FieldName 时 `ctx.get_field` 取 PyObject 后 print" |
| **§5.2.3 has_expressions L1539** | `Node::Probe(p) => p.into().is_some()` 保持（FieldName 非表达式，但 Probe 含 into 时仍需走非 fast-path？需 DEV 确认——FieldName 不依赖 ctx 表达式求值，has_expressions 应为 false。**需 ARCH 复核**：Probe 的 has_expressions 语义） |
| **§5.5 边界条件 PB-3** | `Probe(some_expr)` → `Probe(some_field)`（单字段引用，任意类型） |
| **§5.5 PB-5** | `Probe(lambda ctx: ...)` 编译期拒绝（不变，ADR-014） |
| **§5.6 DEV 实施清单 L1594** | 删除 `expr.rs` 行"若 `eval_expr_any` 不存在，新增（用于 Probe.into）~30 增量（可选）"——不再需要 |
| **§5.7 Parity 测试模板** | Probe 表达式相关用例改为字段引用（`Probe(count)` 而非 `Probe(this.count)`） |
| **§10 D-P0-2 决策** | 更新结论为："PM 决策（P0-VET驳回修复阶段确认）：接受 DEV FieldName 方案。原 ExprProgram + eval_expr_any 推荐撤销——FieldName 覆盖实际用例（单字段引用，任意类型），无算术组合需求。" |
| **§11.2 ADR-006 引用 L2006** | "Probe.into 走 ExprProgram" → "Probe.into 走 FieldName（单字段引用），不走 ExprProgram VM" |

**注意 §5.2.3 has_expressions 复核项**：原设计 `Node::Probe(p) => p.into().is_some()`
是基于 ExprProgram 是表达式。改用 FieldName 后，FieldName 不是"运行期表达式"
（它只是字段名查找，不涉及算术/比较运算）。需 ARCH 复核：Probe 含 into 时
has_expressions 应返回 true 还是 false？这影响 schema.rs static_size 预分配
（若 Probe 字段被标记为含表达式，Struct static_size 会退化为 None）。
**初步判断**：FieldName 查找不依赖运行期求值（与 Switch FieldRef 同性质），
has_expressions 应为 false。但需 ARCH 实际审查 probe.rs 实施确认。

---

## 2. 质疑 2：NamedTuple over Struct 的 build 实现路径（AD-P1-5）

### 2.1 DEV 偏离内容摘要

**位置**：`construct-rs/src/nodes/named_tuple.rs` build 方法。

**偏离**：

| 维度 | 设计文档 §6.1.4 原文 | DEV 实施 |
|------|--------------------|---------|
| build_obj 构造 | namedtuple 实例 → 按 field_names getattr → 构造 PyDict → `inner.build(dict)` | 直接传 namedtuple 实例 → `inner.build(instance)` |
| 中间对象 | PyDict（kwargs） | 无（透传实例） |
| StructNode.build 取值方式假设 | 假设 inner.build 接受 dict（按 key 取值） | 实际 StructNode.build 用 `obj.getattr(field_name)` 取值 |

**DEV 偏离理由**（P1P2-DEV实施.md L107-115）：
1. StructNode.build 内部对 obj 调 getattr（期望实例），传 dict 会失败（dict 没有 getattr 语义）
2. namedtuple 支持 getattr（如 `nt.x`），故直接传实例即可
3. 与 Python 不对齐：Python `factory(**obj)`（core.py L3416）传 Container 所有字段，
   多余字段会触发 TypeError（严格）；construct-rs 让 StructNode 自己 getattr
   tuplefields 命名的字段（忽略多余字段，**更宽松**）
4. 这是 AD-P1-5 修正的延续——v1 设计说"对齐 factory(**obj)"描述错误，v2 已修正但
   build 实现路径仍含矛盾。本实施按 AD-P1-5 §8.1 修正理由实现，与设计 §6.1.6 NT-10
   + §7.5 描述的"宽松忽略"行为一致

### 2.2 设计文档原文

**§6.1.4 Construct impl build 代码（L2081-2100）**：

```rust
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
            let seq = pyo3::types::PyList::sequence_to_list(obj)?;
            seq.into_py(py)
        }
    };
    self.inner.build(py, build_obj.bind(py), stream, ctx, path)
}
```

**§6.1.2 construct-rs Struct 返回实例（非 Container）的处理（L1988）**：
> Python NamedTuple._decode 对 Struct 分支做 `del obj["_io"]; factory(**obj)`（obj 是 Container dict）。
> construct-rs 的 StructNode.parse 返回用户实例（dataclass 实例，字段在 `__dict__`），**不含 `_io`**。
> 因此 NamedTuple 对 Struct 分支需：按 tuplefields 名字逐个 `getattr(instance, name)` 提取，再 `factory(**kwargs)`。

**§8.1 AD-P1-5 决策（L2409，v2 已修正）**：
> NamedTuple over Struct 的字段提取 → **按 tuplefields 名字 getattr**（非 `**__dict__`）。
> construct-rs Struct 返回实例（非 Container），实例 `__dict__` 可能含 RO/Computed 字段。
> **parity 差异（C-2 修正）**：Python `factory(**obj)`（core.py L3416）传 Container **所有**字段
> （除 `_io`），多余字段会触发 `TypeError`（严格）；construct-rs 只传 tuplefields 命名的字段，
> **忽略**额外字段（宽松）。两者**不对齐**，construct-rs 更宽松。这是 construct-rs 的合理工程选择
> （实例 `__dict__` 含 RO/Computed 字段时按 tuplefields 提取避免干扰），但**非对齐 Python**
> （v1 理由"对齐 factory(**obj) 语义"描述错误，已修正）。见 §6.1.6 NT-10 + §7.5

### 2.3 Python 参考实现事实（L-14 交叉验证基础）

**`construct/construct/core.py` NamedTuple 类（L3381-3446）**：

```python
def _decode(self, obj, context, path):
    if isinstance(self.subcon, Struct):
        del obj["_io"]
        return self.factory(**obj)          # L3416: 传 Container 所有字段
    if isinstance(self.subcon, (Sequence, Array, GreedyRange)):
        return self.factory(*obj)
    ...

def _encode(self, obj, context, path):
    if isinstance(self.subcon, Struct):
        return Container({sc.name: getattr(obj, sc.name) for sc in self.subcon.subcons if sc.name})  # L3423
    if isinstance(self.subcon, (Sequence, Array, GreedyRange)):
        return list(obj)
    ...
```

**关键事实**：
1. Python `_decode`（L3416）：`factory(**obj)` 传 Container **所有字段**（Python Struct.parse 返回 Container dict）
2. Python `_encode`（L3423）：`getattr(obj, sc.name)` 按 subcons 名字提取 → 返回 **Container**（dict 子类）
3. Python inner `_build` 接收的是 Container（dict 子类），按 dict key 取值（`obj[name]`）

**construct-rs 的关键差异**：
- StructNode.parse 返回**用户实例**（dataclass 实例，非 Container/dict）
- StructNode.build 用 `obj.getattr(field_name)` 取值（struct_node.rs L393-396）——**不是 dict key 查找**

### 2.4 L-14 交叉验证（硬约束检查）—— 设计文档 §6.1.4 build 代码存在 bug

设计 §6.1.4 build 代码构造 PyDict 后传给 `inner.build(dict)`。但 StructNode.build 的实际实现
（struct_node.rs L378-396）是：

```rust
fn build(...) {
    for field in &self.fields {
        match field.mode {
            FieldMode::Rw | FieldMode::Wo => {
                // RW/WO：从实例 getattr 取值，递归 build
                let value = obj.getattr(field.name.py_name().bind(py)).map_err(...)?;  // L396
                ...
            }
        }
    }
}
```

**PyDict 的 getattr 行为**：dict 实例**不支持**用 key 作为属性访问。
`{"x": 1}.getattr("x")` 会抛 `AttributeError`（dict 没有名为 "x" 的属性）。
dict 的属性访问是 `dict.keys` / `dict.items` 等 dict 类型自身的方法，而非用户存的 key。

**结论**：设计文档 §6.1.4 的 build 代码（构造 dict → inner.build(dict)）**会导致 StructNode.build 失败**
——`obj.getattr("x")` 对 dict 抛 AttributeError。**这是设计文档的错误**，不是 DEV 的偏离。

交叉验证所有实现路径：

| 实现路径 | 描述 | StructNode.build 兼容性 | 正确性 |
|---------|------|------------------------|-------|
| **A（设计原方案）** | 构造 PyDict → `inner.build(dict)` | ❌ dict 不支持 `getattr(field_name)` | **错误** |
| **B（DEV 实施）** | 直接传 namedtuple 实例 → `inner.build(instance)` | ✅ namedtuple 支持 `getattr(field_name)` | **正确** |
| C（替代） | 构造 SimpleNamespace → `inner.build(ns)` | ✅ namespace 支持 getattr | 多余开销（构造临时对象） |

**路径 B 是唯一无冗余的正确路径**。路径 A 失败（dict 无 getattr 语义），
路径 C 多余（构造临时对象仅为绕过 dict 限制）。DEV 实施（路径 B）正确。

**parity 差异已记录**：DEV 实施的"宽松忽略多余字段"行为（StructNode 按 tuplefields
getattr，忽略实例 `__dict__` 中其他字段）已在 §6.1.6 NT-10 + §7.5 + §8.1 AD-P1-5 记录
（v2 修正）。这与 Python `factory(**obj)` 的"严格报 TypeError"行为不一致，是
construct-rs 的合理工程选择（避免 RO/Computed 字段干扰 namedtuple 构造）。

### 2.5 决策：方案 A（接受 DEV 实施）

**理由**（按 §0 判断维度）：

1. **parity 影响**：
   - Python `_encode` 用 `getattr(obj, sc.name)` 提取字段 → Container（core.py L3423）
   - construct-rs DEV 实施：直接传 namedtuple 实例，由 StructNode.build 内部 `getattr(field_name)` 提取
   - 两者**字段提取逻辑等价**（都是 getattr by name），差异仅在"是否预构造中间 dict"
   - **已知 parity 差异**（NT-10）：construct-rs 忽略多余字段（宽松），Python 报 TypeError（严格）——已记录，非本次偏离引入

2. **API 合理性**：
   - namedtuple 是 tuple 子类，支持 `getattr(nt, "x")`，与 StructNode.build 的 getattr 取值模式天然兼容
   - 直接传实例避免了"构造 dict → dict 又不支持 getattr"的矛盾
   - 与 Sequence 模式对称：Sequence 模式 DEV 也直接传 list（list 支持位置索引），不构造中间对象

3. **§0 合规**：
   - #1 一次 FFI：namedtuple 实例是 Python 对象，透传给 inner.build 不引入额外 FFI
   - #2 无中间表示层：DEV 方案**优于**设计原方案——设计原方案构造 PyDict（中间对象）反而引入中间表示，DEV 直接透传实例更符合 §0 #2
   - #3/#4/#5/#6/#7/#8：均合规

4. **实施成本**：DEV 方案代码更简洁（省去构造 PyDict 循环 + into_py），无额外成本

5. **用户迁移影响**：
   - 用户从 Python 迁移 NamedTuple over Struct：build 行为差异（多余字段忽略 vs TypeError）已在 NT-10 记录
   - 常见用例（tuplefields 与 Struct 字段一一对应）无差异

**关键判断**：设计文档 §6.1.4 build 代码是**错误实现**（dict 不支持 getattr），
DEV 实施修正了这个错误。这不是"DEV 偏离设计"，而是"DEV 发现设计 bug 并修正"。
按 architect.md §响应设计质疑 回应方式 2（修改设计文档）：确认质疑成立，修改设计文档。

### 2.6 需更新设计文档（PM 确认后由 ARCH 统一修改）

| 章节 | 更新内容 |
|------|---------|
| **§6.1.4 Construct impl build 代码 L2081-2100** | Struct 模式分支改为：`NamedTupleMode::Struct => { /* 直接传 namedtuple 实例，StructNode.build 内部按 field_names getattr 取值（namedtuple 支持 getattr，dict 不支持） */ build_obj = obj.clone() }`；删除构造 PyDict 的循环。Sequence 模式不变（list 透传） |
| **§6.1.4 代码注释 L2083-2084** | "namedtuple 实例 → 按名字 getattr → dict → inner.build(dict)" → "namedtuple 实例直接透传 → inner.build(instance)；StructNode.build 内部 obj.getattr(field_name) 取值（namedtuple 原生支持 getattr，dict 不支持，故不能构造中间 dict）" |
| **§6.1.4 代码后说明 L2105** | 补充："`getattr(instance, name)` 是 C API（实例 `__dict__` 查找）。namedtuple 实例的 `__dict__` 含 tuplefields 命名字段，StructNode.build 按 field.name getattr 可正确取值。注：设计 v1 写'构造 dict → inner.build(dict)'错误——PyDict 不支持字段属性访问，会抛 AttributeError。v2 修正为直接透传实例。" |
| **§6.1.2 L1988 段落** | 补充 build 路径说明：parse 路径仍需构造 kwargs dict（因为 factory(**kwargs) 是 C 级 namedtuple 构造，需 dict）；build 路径**不需要**构造 dict（直接透传实例给 StructNode.build，后者用 getattr 取值） |
| **§6.1.5 §0 对照表 #2 L2112** | "PyDict（kwargs）是 Python 对象本身；namedtuple 实例是最终用户对象" → 补充："build 路径直接透传 namedtuple 实例（无中间 dict），parse 路径 kwargs dict 是 factory.__call__ 的必要输入（C 级构造）" |
| **§8.1 AD-P1-5 L2409** | 补充 build 路径修正说明："v2 仅修正了字段提取语义（按 tuplefields 名字），未修正 build 实现路径。本回应确认 build 路径也需修正为直接透传实例（设计 §6.1.4 原代码构造 dict 会导致 StructNode.build AttributeError）。parity 差异（忽略多余字段）保持 NT-10 描述。" |

**注意**：parse 路径（L2056-2080）的 Struct 模式**仍需构造 kwargs dict**——因为
`factory.call((), Some(&kwargs))` 是 C 级 namedtuple 构造（`factory(**kwargs)`），
需要 dict 作为关键字参数容器。这部分设计文档正确，**不改**。仅 build 路径需修正。

---

## 3. 总结：需 ARCH 后续更新的设计文档章节清单

PM 确认本回应决策后，ARCH 将统一修改以下设计文档章节（本次不直接改，先给决策）：

### 3.1 模块设计-Phase8-P0.md（质疑 1：Probe.into）

| 章节 | 修改类型 |
|------|---------|
| §5.2.2 ProbeNode struct + 表达式约束说明 | 字段类型 ExprProgram → FieldName；注释同步 |
| §5.2.2 printout 实现要点 | ExprProgram 求值 → FieldName get_field |
| §5.2.3 has_expressions（**需 ARCH 复核**） | FieldName 非运行期表达式，has_expressions 语义需确认 |
| §5.5 PB-3 / PB-5 边界条件 | some_expr → some_field；PB-5 不变 |
| §5.6 DEV 实施清单 | 删除 expr.rs eval_expr_any 行 |
| §5.7 Parity 测试模板 | Probe 字段引用用例更新 |
| §10 D-P0-2 决策 | 结论改为"接受 FieldName 方案" |
| §11.2 ADR-006 引用 | Probe.into 走 FieldName 非 ExprProgram |

### 3.2 模块设计-Phase8-P1P2.md（质疑 2：NamedTuple build）

| 章节 | 修改类型 |
|------|---------|
| §6.1.4 Construct impl build 代码 | Struct 模式：构造 dict → 直接透传实例 |
| §6.1.4 代码注释 + 后说明 | 标注 v1 错误（dict 不支持 getattr）+ v2 修正 |
| §6.1.2 段落 | 补充 build 路径不需构造 dict 的说明 |
| §6.1.5 §0 对照表 #2 | 区分 parse（需 kwargs dict）vs build（透传实例） |
| §8.1 AD-P1-5 | 补充 build 路径修正说明 |

### 3.3 待 ARCH 复核项（更新前需确认）

1. **§5.2.3 has_expressions 语义**（质疑 1）：FieldName 改用后，Probe 含 into 时
   has_expressions 应返回 true 还是 false？需 ARCH 实际审查 probe.rs 实施 +
   schema.rs static_size 影响。**初步判断 false**（FieldName 不依赖运行期表达式求值），
   但需确认实施一致性。

### 3.4 不需更新的章节

- §6.1.4 parse 路径（L2056-2080）：仍需构造 kwargs dict（factory(**kwargs) 需要），正确
- §6.1.6 NT-10 / §7.5：parity 差异已记录（v2 修正），本次偏离不改变其描述
- Sequence 模式 build（list 透传）：设计文档原方案正确，DEV 实施一致

---

## 4. ARCH 决策一句话摘要

- **质疑 1（D-P0-2 Probe.into）**：**方案 A 接受 DEV FieldName 实施**——FieldName 覆盖
  Probe 实际用例（单字段引用，任意类型），ExprProgram 算术组合无 Python 实际用例支撑，
  L-14 交叉验证确认路径 B 未被 §0/ADR 排除；需更新设计 §5.2.2 + D-P0-2 等 8 处章节。

- **质疑 2（AD-P1-5 NamedTuple build）**：**方案 A 接受 DEV 直接透传实例实施**——设计
  §6.1.4 原代码构造 dict 会导致 StructNode.build AttributeError（dict 不支持字段属性访问），
  这是设计文档 bug，DEV 实施修正了它；parity 差异（忽略多余字段）已记录在 NT-10/§7.5；
  需更新设计 §6.1.4 build 代码 + AD-P1-5 等 5 处章节。


