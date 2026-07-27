---
id: ADR-015
status: accepted
phase: "4"
decides: "Element 仅作为构造器字段（v5）"
supersedes: []
superseded_by: []
depends_on: [ADR-014]
last_updated: 2026-07-27
---

# ADR-015: Element 仅作为构造器字段

## Context

v4 曾实现 `ExprOp::GetElem` + `_current_elem_ptr` borrowed ptr 机制，让 RepeatUntil 终止表达式直接引用当前元素。问题：
1. `_current_elem_ptr` 涉及 borrowed reference 的生命周期安全风险
2. 用户面入口与 Index（ADR 平行：Index 作为字段）不一致
3. 表达式系统输入类型扩展（多了"元素引用"特例）

## Decision

`Element` 是构造器（subcon），`rfield(Element())` 在 RepeatUntil 终止表达式中引用当前元素。

**不引入** `ExprOp::GetElem` / `_current_elem_ptr` 等表达式内"元素引用"机制。

### 用户面形式

```python
@dataclass
class Packet(StructMixin):
    e: int = rfield(Element())
    # 终止表达式引用 e
    # RepeatUntil(e > 5, Int8ub)
```

字段名引用编译为 `[GetInt(idx_of_e), Const(5), Gt]`。

### 实现机制

RepeatUntil 在迭代时调 **`Context::set_expr_value_only(element_field_idx, elem)`** 借用 Element 字段槽位（仅写 `expr_values_buf`，不写 PyDict），使 GetInt 取到当前元素。

借用不影响 Packet 实例的 Element 字段属性（始终为 None）。

### v5 实施澄清（DEV 质疑 1 回应）

原决策文字误称用 `set_field_at`，但 `set_field_at` 双写（PyDict + buf），会污染 Packet 实例 `__dict__`。改用 `Context::set_expr_value_only`（`context.rs` L251-278，仅写 buf 的公开 API）。

### Index 字段在 RepeatUntil 终止表达式中的支持（DEV 质疑 2 回应）

终止表达式可额外引用 Index 字段（如 `(e + i) >= 10`）。RepeatUntil 通过 `index_field_indices: Vec<usize>` + `sync_index_fields` 方法在每次迭代把当前 `ctx._index` 同步到 Index 字段 buf 槽位（同样仅写 buf）。

## Consequences

- 正面：与 Index（ADR 平行）完全平行——用户面形式一致（`rfield(<构造器字段>())` + 字段名引用）
- 正面：表达式系统输入类型保持纯粹（仅 `_FieldDescriptor` / `_ExprRef` / `int`）
- 正面：不引入新 ExprOp 指令（GetInt 已足够）
- 正面：消除 v4 `_current_elem_ptr` borrowed ptr 的 SAFETY 风险（改用 set_expr_value_only，PyObject 由 Vec 持有，生命周期覆盖终止表达式求值）
- 中性：ElementNode 与 IndexNode 唯一实现差异是值生命周期（实现细节不影响用户面一致性）

## Relations

- 与 Index 决策平行（Index 仅作为构造器字段）
- 引用证据：`docs/design/模块设计-Array.md` §2.5.3（一致性论证）/ §4.3.5（API 说明）/ §4.7.1（Element 字段语义）
- 关联 ADR：ADR-014（RepeatUntil 终止表达式）
