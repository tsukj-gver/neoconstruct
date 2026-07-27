# 设计决策记录

> 本文件记录全项目跨阶段的设计约束。任何新功能设计不可违反这些决策。
> 所有角色必须阅读。AGENTS.md §8.5 指向本文件。

## Phase 2：表达式系统

| 决策 | 内容 | 理由 |
|------|------|------|
| **废弃 `this`** | 用户写 `Bytes(count)` 而非 `Bytes(this.count)`。字段名本身就是引用对象，编译期通过 `id()` 绑定到字段索引。禁止在 construct-rs 设计和文档中使用 `this.xxx` 语法 | 简化表达式，编译期绑定 |
| **不提供 `len_`** | 用 `Tell()` + `Computed()` 替代 | 语义更清晰 |
| **嵌套跨层引用用显式 `context=`** | 不靠 `this._._` 魔法 | 显式优于隐式 |
| **三种 field 函数** | `field(subcon)` = RW / `rfield(subcon)` = RO / `wfield(subcon)` = WO。WO 字段编译期禁止被表达式引用 | 数学完备分类 |
| **default 自动 kw_only** | `field(subcon, default=value)` 解决字段顺序冲突 | dataclass 兼容 |
| **表达式 VM 仅 i64** | 栈式 VM，不支持浮点/lambda/字符串表达式 | 覆盖长度/计数场景 |

## Phase 2.5：性能优化

| 决策 | 内容 | 理由 |
|------|------|------|
| **Context Vec 化** | GetInt 走编译期索引数组（不走 PyDict hash 查找） | 消除 hash 查找开销 |
| **parse 借用实例 `__dict__`** | parse 先 tp_new 创建实例，借用实例自带 `__dict__` 直接填充，无独立中间 dict + force_setattr | 消除中间 dict 开销 |

## Phase 3：BitStream

| 决策 | 内容 | 理由 |
|------|------|------|
| **bit 顺序默认 MSB-first** | LSB-first 通过 BitsSwapped 包装实现，不是参数 | 与 Python construct 一致 |
| **剩余位不自动 padding** | 不足 8 的倍数报错，用户必须用 Padding 补齐 | 与 Python construct 一致 |

## Phase 4：Array

| 决策 | 内容 | 理由 |
|------|------|------|
| **ListContainer 返回原生 list** | parse 直接返回 Python `list`，不包装为 ListContainer 子类 | 性能优先，`==` 行为一致 |
| **StopField 用 Result 哨兵** | 不用 panic/异常做控制流，用 `ConstructError::StopField` 变体 | Rust 不适合用异常做控制流 |
| **Index 仅作为构造器字段** | `Index` 是构造器（subcon），`rfield(Index())` 解析得到当前下标。**不引入** `ExprOp::GetIndex` / `_IndexRef` 等表达式内"下标引用"机制。用户要在表达式中引用下标，先声明 Index 字段（`i: int = rfield(Index())`），再用字段名引用（`Bytes(i + 1)`，编译为 `[GetInt(idx_of_i), Const(1), Add]`） | (1) 符合 Phase 2 "字段名即引用、废弃 this" 哲学，所有引用统一走 `_FieldDescriptor`；(2) 表达式系统输入类型保持纯粹（仅 `_FieldDescriptor` / `_ExprRef` / 常量），不引入特例；(3) IndexNode + 字段引用已完整覆盖 Python `this._index` 的功能等价物；(4) YAGNI——表达式内引用下标是罕见场景，不值得为它扩展表达式系统 |
| **RepeatUntil PyCallable 谓词 context proxy 用 Container 包装**（v4，V-1 修正）— **v5 作废** | ~~PyCallable 路径传给谓词的 context proxy **必须**是 `construct.lib.containers.Container` 实例~~ | **v5 用户打回作废**：v4 的 PyCallable 路径整体删除（违反"一次 FFI"核心原则 + "白名单+兜底"二分设计 + 隐藏决策路径）。详见决策 4' |
| **决策 4'：RepeatUntil 终止表达式 = Phase 2 表达式**（v5 用户硬约束 #5） | RepeatUntil 用户面 API `RepeatUntil(terminator, subcon, discard)` 中 `terminator` 必须是 Phase 2 表达式（`_FieldDescriptor` / `_ExprRef` / `int` 组合），编译为 ExprProgram，运行时零 FFI 求值。**不接收 Python lambda/callable**。删除 v4 的 `RepeatPredicate` 枚举、`call_repeat_predicate`、`build_context_proxy`、`container_cache.rs`、AST 识别器 `_try_compile_repeat_predicate`。RepeatUntil 用户面参数 `predicate` 改名为 `terminator`，禁用"谓词"术语。无法用 Phase 2 表达式描述的终止逻辑（如 `lambda x,lst,ctx: lst[-2:] == [0, 0]`）用户须改用 Adapter（显式慢路径，不在 RepeatUntil 内部交汇） | (1) 用户硬约束 #1：禁止 PyCallable 路径（违反"一次 FFI"原则）；(2) 用户硬约束 #2：禁止"白名单+兜底"二分设计（隐藏决策路径）；(3) 用户硬约束 #3：禁用"谓词"术语；(4) 用户硬约束 #4：只认 Phase 2 表达式语法（禁止 RepeatUntil 自建表达式机制）；(5) v4 性能门禁 1.5x/3x 无物理推导（Python 3.14 实测 Python 调用仅 270-300ns，v4 估算高估 2-3 倍）；(6) RepeatUntil 终止表达式 ≥10x 数值推导见 `docs/模块设计-Array.md` §8.3 |
| **决策 5：Element 仅作为构造器字段**（v5 新增，2026-07-11 v5 实施质疑回应澄清） | `Element` 是构造器（subcon），`rfield(Element())` 在 RepeatUntil 终止表达式中引用当前元素。**不引入** `ExprOp::GetElem` / `_current_elem_ptr` 等表达式内"元素引用"机制。用户要在 RepeatUntil 终止表达式中引用当前元素，先声明 Element 字段（`e: int = rfield(Element())`），再用字段名引用（`RepeatUntil(e > 5, Int8ub)`，编译为 `[GetInt(idx_of_e), Const(5), Gt]`）。RepeatUntil 在迭代时调 **`ctx.set_expr_value_only(element_field_idx, elem)`** 借用 Element 字段槽位（仅写 `expr_values_buf`，不写 PyDict），使 GetInt 取到当前元素；借用不影响 Packet 实例的 Element 字段属性（始终为 None）。**v5 实施澄清**（DEV 质疑 1 回应）：原决策文字误称用 `set_field_at`，但 `set_field_at` 双写（PyDict + buf），会污染 Packet 实例 `__dict__`。改用 `Context::set_expr_value_only`（context.rs L251-278，仅写 buf 的公开 API）。**Index 字段在 RepeatUntil 终止表达式中的支持**（DEV 质疑 2 回应）：终止表达式可额外引用 Index 字段（如 `(e + i) >= 10`），RepeatUntil 通过 `index_field_indices: Vec<usize>` + `sync_index_fields` 方法在每次迭代把当前 `ctx._index` 同步到 Index 字段 buf 槽位（同样仅写 buf）。**v5 关键变更**：v4 曾实现 `ExprOp::GetElem` + `_current_elem_ptr` borrowed ptr，v5 删除（不安全 + 用户面入口不统一）。 | (1) 与 Phase 4 决策 3（Index 仅作为构造器字段）完全平行：用户面形式一致（`rfield(<构造器字段>())` + 字段名引用），表达式系统输入类型保持纯粹（仅 `_FieldDescriptor` / `_ExprRef` / `int`）；(2) 不引入新 ExprOp 指令（GetInt 已足够）；(3) ElementNode 与 IndexNode 唯一实现差异：值生命周期（Element 由 RepeatUntil 主动 set_expr_value_only 借用 buf 槽位，Index 由 IndexNode.parse 一次性写入 dict+buf，被 RepeatUntil 引用时再由 sync_index_fields 同步 buf），这是实现细节不影响用户面一致性；(4) 消除 v4 `_current_elem_ptr` borrowed ptr 的 SAFETY 风险（改用 set_expr_value_only，PyObject 由 Vec 持有，生命周期覆盖终止表达式求值）；(5) 详见 `docs/模块设计-Array.md` §2.5.3 一致性论证 / §4.3.5 API 说明 / §4.7.1 Element 字段语义 |

## 跨阶段：已验证模式规范

> 本章节记录在某个阶段验证、后续阶段**必须遵循**的通用实现模式。
>
> **强制规范**（非可选）：
> - ARCH 在设计新 Node 时必须查阅本章节，并在设计文档中明确"本节点采用模式 X"
> - REV 检视清单包含"模式一致性"维度（详见 `docs/架构审查-重复代码与抽象质量.md` §6.2）
> - DEV 实现前必须在过程记录中声明"已查阅本章节 + 已查阅 `nodes/common.rs` 共享层"
> - PM 在 CODING 阶段入口检查 DEV 声明，在 ACCEPTED 时检查模式采用情况
>
> 历史教训：Phase 4 Array 系列最初未采用 Phase 1 已验证的 P0-3 lazy path 模式（设计文档明知该模式存在，但选择"首版保留、后续优化"），导致 4.7 性能塌方后补迁移。本章节旨在防止类似重复决策。

| 模式 | 首次验证 | 内容 | 适用范围 | 禁止行为 |
|------|---------|------|---------|---------|
| **P0-3 lazy path 错误传播** | Phase 1 (StructNode)，Phase 4.7 推广到 Array 系列 | 成功路径**不**维护 Path 栈（零 String 分配，零 Vec 操作）。子节点返回 `Err` 时，父节点通过 `ConstructError::push_path_segment(name)` 或 `push_path_index(i)` 重建路径段。Path 类型为 enum（`Root`/`Segments`），生产代码成功路径走 `Root` 分支（零分配） | 所有含递归子节点的节点（Struct / Array / GreedyRange / PrefixedArray / RepeatUntil / Bitwise / Bytewise / Transform / StructRef） | ❌ 成功路径调用 `path.push_field` / `path.push_index` / `path.pop`<br>❌ "首版保留 push/pop，后续优化"的重新决策 |
| **Vec 中转 PyList 构建** | Phase 4.7 (Array 系列 4 节点) | 解析多个元素时用 `Vec<Py<PyAny>>::with_capacity(count)` 收集，最后一次性 `PyList::new_bound(py, elems)`（pyo3 内部用 `PyList_New` + `PyList_SET_ITEM`） | 所有返回 list 的 Node（parse 路径） | ❌ `PyList::new_bound(py, Vec::with_capacity(count))` + `append`（capacity hint 丢失，走 realloc 慢路径） |
| **_index save/restore 配对** | Phase 4 (ArrayNode)，Phase 4.5 v5 整合为 `Context::restore_index` 方法 | 数组迭代前 `let old = ctx.index()`，迭代内 `ctx.set_index(i)`，**所有退出路径**（成功 return / 错误 return / break）必须 `ctx.restore_index(old)`。嵌套数组通过覆盖+恢复支持 | 所有使用 `ctx.set_index` 的 Node（Array 系列） | ❌ 只有 `set_index` 无 `restore_index`<br>❌ 错误路径漏掉 restore |

### 模式采用声明（强制）

新 Node 设计文档必须包含"模式采用声明"段，例如：

```markdown
### 模式采用声明

本节点（XxxNode）采用以下跨阶段已验证模式：
- ✅ P0-3 lazy path 错误传播（成功路径不维护 Path）
- ✅ _index save/restore 配对（如适用）
- N/A Vec 中转 PyList（本节点不返回 list）

已查阅共享层：`nodes/common.rs` 的 `collect_obj_to_vec` / `obj_type_name`。
```

### 整改进度（来自 PA-REVIEW 审查）

| 整改项 | 优先级 | 状态 | 时机 |
|--------|--------|------|------|
| `restore_index` 提升为 Context 方法（P0-1） | P0 | 待 4.5 v5 实施时同步 | 4.5 v5 |
| `collect_obj_to_vec` 提取为公共 helper（P1-1） | P1 | 待 4.5 v5 实施时同步 | 4.5 v5 |
| 新增 `nodes/common.rs` 共享工具层 | P1 | 待 4.5 v5 实施时同步 | 4.5 v5 |
| 测试 helper 提取到 `tests/common/mod.rs`（P2-1） | P2 | 待办 | Phase 4 收尾 |
| `compile.rs` `desc_getattr` helper（P3-2） | P3 | 待办 | 后续阶段 |

详见 `docs/架构审查-重复代码与抽象质量.md` §4 整改清单。
