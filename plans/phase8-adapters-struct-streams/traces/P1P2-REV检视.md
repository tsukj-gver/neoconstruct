---
id: TRACE-phase8-P1P2-REV
status: 通过-DESIGN_REVIEW完成
phase: "8"
task: "8.2/8.3/8.6/8.7/8.11/8.12 [P1+P2 批次 REV 设计检视]"
reviewer: REV
target: DESIGN-Phase8-P1P2
created: 2026-07-31
revised: 2026-07-31（v2 复检）
---

# Phase 8 P1+P2 批次 REV 设计检视报告

**检视文档**：`docs/design/模块设计/模块设计-Phase8-P1P2.md`（2409 行，12 构造器）
**检视范围**：P1（Enum/FlagsEnum/Mapping、OneOf/NoneOf/Validator、Union、Sequence、ProcessXor/ProcessRotateLeft）+ P2（NamedTuple/Timestamp）
**检视依据**：AGENTS.md §0 + reviewer-extension.md + experiences.md L-01~L-14 + ADR-022/014/017/008 + Python 参考实现（core.py）+ 已有 Rust 实现（FocusedSeqNode/TransformNode/AdapterCallbackNode/StructNode）

---

## 逐项检视结论

### 延续性：✅ 通过

- 设计与 P0（已 ACCEPTED）的 §0.2 判据、Node enum 命名约定、Subconstruct 包装模式、Construct 非包装模式一致。
- 新增 10 个 Node 变体（48→58）的 `has_expressions` 集成遵循已有模式（递归子树 / `|| matches!`）。
- Validator 复用 AdapterCallbackNode（ADR-022 L204-205 已预告"Phase 6+ 用户面 Adapter 子类复用"），与 Phase 6.3 已实现接口风格统一。
- Sequence 复用 FocusedSeqNode 的 context nesting 模式（`Context::new_child` + `set_field_at`）—— 已对照 `focused_seq.rs` L163-180 确认模式一致。
- ProcessXor/ProcessRotateLeft 复用 TransformNode 的子流构造模式（`ParseStream::new(&transformed)` + `BuildStream::with_capacity`）—— 已对照 `transform.rs` L193/L208 确认 API 存在。
- **未发现延续性破坏**。

### 性能：✅ 通过（含已识别风险）

- 设计含"性能假设"章节（§1.6/§2.5/§3.6/§4.7/§5.8），每子任务含瓶颈识别 + 可证伪预测。
- 瓶颈识别有量化来源（pyo3 benchmark PyDict_GetItem ~50-80ns / PySet_Contains ~30-50ns / PyList::append ~30ns / seek ~1ns）。
- FFI 来源清单（L-05 对策）覆盖完整：每子任务列出所有 FFI/拷贝/转换来源，且 C API 操作明确标注"不计 FFI"（§0.2 判据 2）。
- 可证伪预测有失败模式应对（§1.6.3 Enum <3x 防御方案 / §8.2 风险点 1 Enum 4-6x 由 PM 决策）。
- **Enum/FlagsEnum 性能下限风险**（检视重点之一）：设计 §8.2 已识别"dict lookup 可能仅 4-6x"。审查确认：Python Adapter._decode 的方法调度开销是主要差距来源（dict lookup 本身 C 级），construct-rs 消除该开销应达 4-6x。设计预测 ≥6x 略乐观但合理，且已明确"<4x 由 PM 决策接受"。**性能假设成立**。
- Sequence/Union/ProcessXor/ProcessRotateLeft 的可证伪预测（≥5-10x）覆盖所有瓶颈来源，无"优化 A 忽略 B"问题（L-05 合规）。

### 整体性：✅ 通过

- 模块边界清晰：每个构造器独立 Node 变体，职责单一。
- 调用方向正确：Enum/Mapping/OneOf 等包装 inner（`Box<Node>`）；Sequence/Union 持有字段列表；ProcessXor 持 inner + pad。无循环依赖。
- 与全局架构一致：Node enum 静态分派（§0 #6）、Construct trait 三方法（parse/build/sizeof）、ConstructError 变体 + path（§0 #7）。
- AD-P1-1~5 自主决策合理（OneOf/NoneOf 分开变体、Sequence 不复用代码、EnumIntegerString 预构造、XorPad::Expr resolve、NamedTuple getattr）。

### 可行性：❌ 问题（1 项驳回级）

**F-1（驳回级）**：Timestamp 顶部 `import arrow` 破坏包可用性（详见具体问题 F-1）。

其余可行性项通过：
- Rust + PyO3 技术栈可实现（PyDict/PyFrozenSet/PyType/PyList C API + ExprProgram + stream seek/tell/data 全部已有，已对照 `stream.rs` L129/185/205/335/582/612/642 确认）。
- ProcessRotateLeft 位运算经验证正确（PR-1/PR-2 算例手工验证通过，与 Python L5474-5489 四分支逻辑一致）。
- ProcessXor fast-path（pad==0 / 全零 bytes）与 Python L5394/L5397 一致。
- Cargo.toml 不需新增依赖（确认）。

### 完备性：❌ 问题（3 项需补充 + 3 项建议）

**C-1（需补充）**：Sequence RO 字段 build 行为与 Python 不对齐，未标注 parity 差异（详见 C-1）。
**C-2（需补充）**：NamedTuple over Struct 的字段提取与 Python 不对齐，AD-P1-5 理由描述错误（详见 C-2）。
**C-3（需补充）**：Union build 缺少 flagbuildnone 处理 + context.update(obj)（详见 C-3）。

建议项（不阻塞，报告中标注）：
- C-4：Mapping 缺少 TypeError 捕获对齐（Python L2152 `except (KeyError, TypeError)`）。
- C-5：Timestamp 缺少 subcon 类型检查（Python L3480 `isinstance(subcon, Construct)`）。
- C-6：Enum build 的 bool 边界（`isinstance(True, int)` 为 True）未列边界条件。

---

## 结论：❌ 驳回（至 DESIGNING）

**驳回依据**：可行性问题 F-1（Timestamp 顶部 import arrow）破坏 P0 已交付功能 + 不对齐 Python 原版。

**驳回目标状态**：DESIGNING（ARCH 修改设计后重新提交检视）。

**修改要求**（ARCH 必须处理）：
1. **F-1**（必须）：修改 Timestamp 的 `import arrow` 策略为函数体内延迟导入（对齐 Python L3478）。
2. **C-1/C-2/C-3**（必须）：补充 Sequence RO 字段 parity 差异标注、修正 NamedTuple AD-P1-5 描述、补充 Union flagbuildnone 处理。

**可在 CODING 阶段处理**（不阻塞设计通过）：
- C-4/C-5/C-6（建议项，DEV 实施时补充边界条件）。

---

## 具体问题（按严重度排序）

### F-1 [可行性·驳回级] Timestamp 顶部 `import arrow` 破坏包可用性

**文档位置**：§6.2.3 Python 层 Timestamp macro 实现，`_macros.py` 顶部 `import arrow`

**问题描述**：

设计 §6.2.3 在 `construct-rs/python/construct/_macros.py` 顶部添加 `import arrow`。当前 `_macros.py` 已含 P0 交付的 `AlignedStruct` 宏（已对照 `_macros.py` 确认，51 行，无 arrow 依赖）。

顶部 `import arrow` 的后果链：
1. 用户未安装 `arrow`（`pip install arrow` 是可选用户依赖，非核心依赖）
2. `_macros.py` 模块加载时 `import arrow` 抛 `ModuleNotFoundError`
3. **`_macros.py` 整个模块加载失败** → `AlignedStruct`（P0 已交付，与 arrow 毫无关系）随之不可用
4. 即使 `__init__.py` L207-210 用 `try/except ImportError` 包裹，`AlignedStruct` 仍会因 arrow 缺失而丢失——这是**P0 已验收功能的回归**

**与 Python 原版对比**：

Python `construct/core.py` L3478：
```python
def Timestamp(subcon, unit, epoch):
    import arrow  # ← 函数体内延迟导入
```

Python 把 `import arrow` 放在 Timestamp **函数体内**，正是为了避免 arrow 缺失时影响其他构造器。设计 §6.2.3 自己也承认"对齐 Python L3478 import arrow 在 Timestamp 函数体（更宽松：仅 Timestamp 调用时失败）"，却选择顶部 import——**自相矛盾**。

**根因**：设计把"更早失败"当作优点，但忽略了 construct-rs 的 `_macros.py` 是**共享模块**（含 AlignedStruct），顶部 import 会污染同模块的其他宏。

**建议修改方向**：

将 `import arrow` 移入 `Timestamp` 函数体内（对齐 Python L3478）：
```python
# _macros.py（修正后）
def Timestamp(subcon, unit, epoch):
    import arrow  # 延迟导入，仅 Timestamp 调用时需要
    if not isinstance(unit, (int, float, str)):
        raise TimestampError(...)
    # ...
```

这样 arrow 缺失时仅 `Timestamp()` 调用失败，`AlignedStruct` 等其他宏不受影响。同时 `MsdosTimestampAdapter`/`EpochTimestampAdapter` 的 `_decode`/`_encode` 内部使用 arrow 时也需确保在调用时才触发（类定义时不执行方法体，所以类定义本身不需 import）。

---

### C-1 [完备性·需补充] Sequence RO 字段 build 行为与 Python 不对齐

**文档位置**：§4.4 SequenceField.field_kind + §4.5 build 逻辑（`if field.field_kind == FieldKind::Ro { compute_ro_value } else { iter.next() }`）+ SQ-6/SQ-7

**问题描述**：

设计引入 `FieldKind::Ro`，Sequence build 时 RO 字段（Check/Computed/Tell 等）**不从 list 取值**，走 `compute_ro_value`。

Python `construct/core.py` Sequence._build（L2410-2423）对所有 subcons 都 `next(objiter)`：
```python
for i, sc in enumerate(self.subcons):
    subobj = next(objiter)  # ← 对所有 subcon，包括 Check
    if sc.name:
        context[sc.name] = subobj
    buildret = sc._build(subobj, stream, context, path)
```

**API 不兼容**：
- Python：`Sequence("a"/Byte, Check(...), "b"/Byte).build([1, None, 2])` — list 含 Check 占位（3 元素）
- construct-rs 设计：Check 是 RO 不消费 list → 用户应传 `[1, 2]`（2 元素）

若 Python 用户迁移代码传 `[1, None, 2]`，construct-rs 会：Check 跳过 iter.next()，`b` 字段消费 list[1]=None → Byte.build(None) 失败或行为异常。

**评估**：这是 construct-rs 的合理设计选择（与 Struct 的 RO 字段概念一致），但**必须明确标注为 parity 差异**。当前设计 SQ-6/SQ-7 描述了行为，但未说明"这与 Python 原版不同"，也未列入 §7.5"已知 parity 差异"清单。

**建议修改方向**：

1. 在 §4.8 边界条件 SQ-6/SQ-7 补注"（construct-rs 设计：RO 字段不消费 list 位置，与 Python 原版行为不同）"
2. 在 §7.5 已知 parity 差异清单新增："Sequence 含 RO 字段（Check/Computed 等）时，build 的 list 不含 RO 字段占位（与 Python 传占位 None 不同）"

---

### C-2 [完备性·需补充] NamedTuple over Struct 字段提取与 Python 不对齐，AD-P1-5 理由错误

**文档位置**：§6.1.4 NamedTupleNode Struct 模式 parse/build + AD-P1-5 决策理由

**问题描述**：

设计 §6.1.4 Struct 模式 parse：按 `field_names`（tuplefields 解析）逐个 `getattr`，再 `factory(**kwargs)`。

Python `construct/core.py` NamedTuple._decode（L3413-3416）：
```python
def _decode(self, obj, context, path):
    if isinstance(self.subcon, Struct):
        del obj["_io"]
        return self.factory(**obj)  # ← 传 Container 所有字段（除 _io）
```

Python 传 Container 的**所有**字段（除 _io）。若 Container 有 tuplefields 之外的字段 → `factory(**obj)` 报 TypeError（多余参数）。

**AD-P1-5 理由错误**：设计 AD-P1-5 声称"按名字提取对齐 Python `factory(**obj)` 语义且避免多余字段"。实际上：
- Python `factory(**obj)` 传**所有**字段，多余字段会**报错**（严格）
- construct-rs 只传 field_names，**忽略**额外字段（宽松）
- 两者**不对齐**，construct-rs 更宽松

construct-rs 的做法在工程上更合理（Struct 实例 __dict__ 可能含 RO 字段，按 tuplefields 提取避免干扰），但**不是对齐 Python**。

**建议修改方向**：

1. 修正 AD-P1-5 理由为："construct-rs 设计选择：按 tuplefields 名字提取（比 Python `factory(**obj)` 更宽松，忽略非 tuplefields 字段）。parity 差异：Python 传所有字段，多余字段报 TypeError；construct-rs 忽略多余字段"
2. 在 §6.1.6 边界条件补充 NT-10："NamedTuple over Struct 且实例含 tuplefields 之外字段：construct-rs 忽略（Python 会 TypeError）"
3. 在 §7.5 已知 parity 差异清单新增此差异

---

### C-3 [完备性·需补充] Union build 缺少 flagbuildnone 处理

**文档位置**：§3.4 UnionNode build（L1001-1029）

**问题描述**：

Python `construct/core.py` Union._build（L3729-3749）：
```python
context.update(obj)  # ← 把 obj dict 合并进 context
for sc in self.subcons:
    if sc.flagbuildnone:              # ← flagbuildnone 分支
        subobj = obj.get(sc.name, None)
    elif sc.name in obj:
        subobj = obj[sc.name]
    else:
        continue
```

设计 §3.4 build 缺少：
1. **`context.update(obj)`**：Python 把整个 obj dict 合并进 context（subcons 之间可通过 context 引用其他字段）。设计只 `set_field_at` 被选中的 subcon 字段。
2. **`flagbuildnone` 分支**：Python 对 `flagbuildnone=True` 的 subcon 用 `obj.get(sc.name, None)`（允许缺键）。设计未处理 flagbuildnone，命名的 flagbuildnone subcon 在 obj 缺键时会被跳过（与 Python `obj.get` 返回 None 不同）。

**影响评估**：
- `context.update(obj)` 缺失：build 只构建一个 subcon，其他字段即使进 context 也不会被该 subcon 用到（除非该 subcon 的表达式引用其他字段）。影响有限但 parity 不完整。
- `flagbuildnone` 缺失：含 flagbuildnone subcon（如 `Const`/`Default` 包装的 Union 成员）的 build 行为可能不一致。UN-11 仅覆盖匿名 subcon，未覆盖 flagbuildnone。

**建议修改方向**：

1. §3.4 build 补充 `context.update(obj)` 等价逻辑（或在文档注明 construct-rs 不合并 obj 到 context 的限制）
2. §3.4 build 补充 flagbuildnone 分支，或至少在 §3.7 边界条件补充 UN-14："Union 含 flagbuildnone subcon 的 build 行为"
3. 评估是否影响常见用例（多数 Union subcon 非 flagbuildnone，影响可能小）

---

### 建议项（C-4/C-5/C-6，不阻塞，DEV 实施时补充）

**C-4 [建议] Mapping TypeError 捕获差异**（§1.4 MappingNode）
- Python Mapping `_decode`/`_encode` 捕获 `(KeyError, TypeError)`（L2152，unhashable key 触发 TypeError）。
- 设计用 pyo3 `get_item?` 传播，unhashable key 时 pyo3 返回 Err，可能转为 ConstructError::Generic 而非 Mapping。
- 建议：DEV 实施时在 MappingNode 的 get_item 失败处捕获并转为 ConstructError::Mapping，或补充边界条件 MP-6 注明差异。

**C-5 [建议] Timestamp 缺少 subcon 类型检查**（§6.2.3）
- Python L3480 检查 `isinstance(subcon, Construct)`，非 Construct 抛 TimestampError。
- 设计 §6.2.3 仅检查 unit/epoch 类型，未检查 subcon。
- 建议：补充 subcon 类型检查（Python 层，TS-4 边界条件扩展）。

**C-6 [建议] Enum build 的 bool 边界未列**（§1.7）
- Python `isinstance(True, int)` 为 True，`Enum(Byte, one=1).build(True)` 返回 True（=1）。
- 设计 `obj.is_instance_of::<PyLong>` 对 PyBool 返回 True（bool 是 int 子类），行为一致。
- 建议：补充边界条件 EN-10 注明 bool 作为 int 的行为。

---

## 通过项确认（检视重点回应）

### 1. §0 合规（L-01）判据严谨性 — ✅ 通过

**检视重点**："FFI crossing ≠ C API 操作"判据在 Enum/Mapping 应用是否严谨？持有 Py<PyDict> 运行时 C API 查询是否引入中间表示层？

**审查结论**：判据严谨，不违反 L-01。

- 该判据在 P0 §0.2 已确立并经 PM 接受（本批次"继承 P0 §0.2"）。判据符合 §0 #1 原文"Rust 内部通过 CPython C API 直接操作 Python 对象"。
- **持有 Py<PyDict> 不引入中间表示层**：`Py<PyDict>` 是 Python dict 对象本身的引用（编译期物化），不是 Rust 端的临时数据类型。运行时 `PyDict_GetItem` 是 CPython C API 操作，直接读写 Python dict 的内部状态——这正是 §0 #1 允许的。
- L-01 禁止的是"Rust 端临时数据类型再转换"（如 `enum Value { Int(i64), ... }`）。Enum/Mapping 的 dict 是 Python 对象本身，decmapping 的值（EnumIntegerString）是最终用户对象，非 Rust 中间类型。
- 对 int/str/bytes/frozenset key 的 `__hash__`/`__eq__` 是 C 级实现，不计额外 FFI——与 P0 HexNode 持 `Py<PyType>` 调 `HexDisplayedInteger.new` 同脉络。
- 边缘场景（用户传自定义子类重写 `__contains__`/`__hash__`）已在 §0.2 + §1.3.3 + §2.2.1 论证为"用户主动引入，不破坏常见场景合规"——合理。

### 2. Sequence 不复用 StructNode（§0 #3）决策 — ✅ 通过

**检视重点**：Sequence 不复用 StructNode 代码是否正确？

**审查结论**：决策正确，符合 §0 #3 硬要求。

- StructNode sink = 实例 `__dict__`（PyDict，`PyDict_SetItem`）；SequenceNode sink = PyList（`PyList_Append`）。两类 sink 的写入 API 完全不同。
- 若强行共享代码需引入"sink trait 抽象层"（如 `trait FieldSink { fn write(...); }`）——这正是 §0 #3 禁止的"输入输出侧 trait 抽象层"（L-01 事件 1/2 的失败模式）。
- 已对照 `focused_seq.rs` 确认：FocusedSeq（Phase 7.1）同样是独立 Node，不委托 StructNode。Sequence 复用 FocusedSeq 的**模式**（context nesting + set_field_at）而非**代码**，符合 L-04 对策（模式复用）。
- AD-P1-2 决策理由充分。

### 3. Union parsefrom ExprProgram 方案 — ✅ 通过

**检视重点**：Union parsefrom ExprProgram 方案是否合理？

**审查结论**：方案合理。

- `ParseFrom` 枚举四变体（None/Index/Name{resolved_index}/Expr）覆盖 Python 的 None/int/str/lambda 四种 parsefrom。
- 编译期 name→index 解析（§3.2）正确：Python `forwards[name]` 在 construct-rs 编译期解析为 index（name→index 映射固定），运行期只需 `Vec<usize>`。Name 变体存 `resolved_index` 与 Index 同效，保留枚举变体便于错误消息——合理。
- Expr 变体用 ExprProgram 替代 context lambda（D-5 已确认，ADR-014 硬约束）——与 Union UN-13 "context lambda 编译期拒绝"一致。
- 越界检查（§3.4 L992-996 `forwards.get(idx).ok_or_else(UnionError)`）对齐 Python `forwards[parsefrom]` KeyError/IndexError。

### 4. ProcessXor/ProcessRotateLeft 位运算正确性 — ✅ 通过

**检视重点**：位运算是否正确？

**审查结论**：经验证正确。

- **ProcessXor fast-path**（§5.4）：pad==0 不变换 / bytes 全零（len≤64）不变换——与 Python L5394/L5397 一致。双重否定 `if not (len<=64 and all_zero)` 转肯定 `if len<=64 && all_zero` 等价。
- **ProcessRotateLeft 四分支**（§5.6）：分支顺序（amount==0 → group==1 查表 → amount%8==0 字节重排 → 通用）与 Python L5474-5489 一致。
- **PR-1 算例验证**（group=1, amount=4, b'\x0f\xf0'）：
  - ROTATION_TABLES[4][0x0f] = (0x0f<<4)|(0x0f>>4) = 0xf0
  - ROTATION_TABLES[4][0xf0] = (0xf0<<4)&0xff|(0xf0>>4) = 0x0f
  - 结果 [0xf0, 0x0f] → 0xf00f ✓ 与设计 PR-1 预期一致
- **PR-2 算例验证**（group=2, amount=4, b'\x0f\xf0'）：
  - indices_pairs = [(0,1),(1,0)]（amount_bytes=0）
  - (0x0f<<4)&0xff|(0xf0>>4)=0xff, (0xf0<<4)&0xff|(0x0f>>4)=0x00
  - 结果 [0xff, 0x00] → 0xff00 ✓ 与设计 PR-2 预期一致
- **build 取负**（§5.6 L1776）：`(-amount).rem_euclid(group*8)` 对齐 Python `-amount % (group*8)`（Python % 是模运算，Rust % 是 remainder，rem_euclid 正确）——与 P0 AlignedNode pad 同处理。
- **ROTATION_TABLES 索引安全**：tables[0] 全零（未用，amount==0 被分支 1 拦截），tables[1..7] 有效。const 表零运行时初始化——与 TransformNode BIT_REVERSE_TABLE 同模式。

### 5. 边界条件覆盖度 — ⚠️ 基本充分（~60 条），有 parity 差异遗漏

- 12 构造器共 ~60 条边界条件，覆盖主要场景（正常/错误/空/越界/类型分支）。
- **遗漏**：Sequence RO 字段 parity 差异（C-1）、NamedTuple 多余字段（C-2）、Union flagbuildnone（C-3）—— 均为 parity 差异未标注，需补充。
- 已知 parity 差异清单（§7.5）覆盖 lambda 禁止 / `>>` 操作符 / ExprValidator / arrow 依赖 / repr 格式，但漏了 C-1/C-2。

### 6. parity 模板 — ✅ 通过

- 每子任务含 parity 测试模板（§1.9/§2.8/§3.9/§4.10/§5.11/§6.1.7/§6.2.7）。
- assert_parity_case / assert_parity_error 模板规范，check_only / expected_py 用法正确。
- EN-2 vs MP-2 的行为差异（Enum 无映射返回 EnumInteger；Mapping 无映射报错）在 parity 模板重点标注——准确对齐 Python L1982 vs L2152。
- FE-3 str split("|") OR 语义、FE-5 `_` 前缀跳过、PR-7 build 取负对称性等关键 parity 点均有覆盖。

---

## 教训对照

- **L-01（中间表示层）**：§0 对照表逐条核对，Py<PyDict>/Py<PyFrozenSet> 持有的是 Python 对象本身，非 Rust 中间类型。✅ 合规。
- **L-02（理论估算）**：瓶颈识别有量化来源（pyo3 benchmark 数据）。✅ 合规。
- **L-04（跨阶段模式）**：§0.3 共通模式表引用 FocusedSeq/Transform/AdapterCallback 先例，未重新决策。✅ 合规。
- **L-05（优化 A 忽略 B）**：FFI 来源清单覆盖完整。✅ 合规。
- **L-13（docstring 语法）**：parity 模板用当前项目语法（无 `this.xxx` 残留）。✅ 合规。
- **L-14（硬约束交叉验证）**：ProcessXor/ProcessRotateLeft 的"读至 EOF"非硬约束（stream.data() 已有），位运算多路径验证。✅ 合规。

---

## 修改后通过条件

ARCH 重新提交设计时，需在修订记录注明以下修改：

1. **F-1（必须）**：§6.2.3 Timestamp `import arrow` 改为函数体内延迟导入。
2. **C-1（必须）**：§4.8 + §7.5 补充 Sequence RO 字段 parity 差异标注。
3. **C-2（必须）**：§8.1 AD-P1-5 修正理由 + §6.1.6 + §7.5 补充 NamedTuple parity 差异。
4. **C-3（必须）**：§3.4 补充 flagbuildnone 处理或注明限制 + §3.7 补充边界条件。

C-4/C-5/C-6 可在 CODING 阶段由 DEV 处理，不阻塞设计通过。

---

> **检视完成时间**：2026-07-31
> **结论**：❌ 驳回（至 DESIGNING）
> **下一步**：PM 转发 ARCH 修改 F-1/C-1/C-2/C-3 后重新提交 DESIGN_REVIEW

---

# v2 复检段（ARCH 修正后复检）

**检视文档**：`docs/design/模块设计/模块设计-Phase8-P1P2.md`（revision: v2，2438 行）
**复检范围**：仅 v1 驳回项 F-1（可行性·驳回级）/ C-1 / C-2 / C-3（完备性·需补充）
**复检依据**：v1 修改要求 + Python 参考实现（core.py L2410/L3416/L3478/L3729-3749）+ 设计文档 v2 修订记录

---

## 逐项复检结论

### F-1 [可行性·驳回级] Timestamp 顶部 `import arrow` → ✅ 已解决

**v1 要求**：将 `import arrow` 从 `_macros.py` 顶部移入 `Timestamp` 函数体内（对齐 Python core.py L3478）。

**v2 验证**：

1. **顶部无 `import arrow`**（L2176-2177）：`_macros.py` 追加段顶部仅有
   ```python
   from ._adapters import Adapter
   from . import BitStruct, BitsInteger, Container
   ```
   确认无 `import arrow`——P0 已验收的 `AlignedStruct` 不受 arrow 缺失影响，**回归风险消除**。

2. **`import arrow` 在函数体内**（L2203）：`def Timestamp(subcon, unit, epoch):` 函数体第一行即为 `import arrow`，对齐 Python core.py L3478。arrow 缺失时仅 `Timestamp(...)` 调用抛 `ModuleNotFoundError`。

3. **注释 + 说明完整**（L2173-2174 / L2248-2250）：设计明确标注"v1 设计曾错误地认为'顶部 import 更早失败是优点'，但忽略了共享模块的污染问题（自相矛盾，已修正）"——根因认知到位。

**结论**：✅ F-1 完全解决。arrow 缺失的影响范围已隔离到 `Timestamp` 函数，共享模块 `_macros.py` 的其他宏（`AlignedStruct`）不受影响。

---

### C-1 [完备性] Sequence RO 字段 build parity 差异标注 → ✅ 已解决

**v1 要求**：§4.8 SQ-6/SQ-7 补注 + §7.5 parity 差异清单新增 Sequence RO 字段差异。

**v2 验证**（三处标注齐全）：

1. **§4.5 build 逻辑行内注释**（L1325）：
   ```rust
   // RO 字段不从 list 取值（parity 差异：Python core.py L2410 对所有 subcons 都 next(objiter)，含 RO 字段；
   // construct-rs RO 字段走 compute_ro_value，list 不含占位。见 §7.5 parity 差异清单）
   ```
   实现代码旁直接标注，DEV 实施时不会遗漏。

2. **§4.8 SQ-7 边界条件**（L1413）：完整说明
   - Python（core.py L2410-2423）对所有 subcons 都 `next(objiter)`，用户需传 `[1, None, 2]`（list 含 RO 字段占位）；
   - construct-rs RO 字段不消费 list 位置，用户传 `[1, 2]` 即可；
   - 给出"用户从 Python 迁移时需调整 build 输入"指引。

3. **§7.5 测试策略 parity 差异清单**（L2391）："Sequence 含 RO 字段（Check/Computed/Tell 等）的 build（C-1，§4.8 SQ-7）"条目，含 Python 行号引用 + 差异方向。

**结论**：✅ C-1 完全解决。差异方向（construct-rs RO 字段不消费 list / Python 消费并需占位）描述准确，三处标注互相引用（代码注释 → §7.5 → §4.8）。

---

### C-2 [完备性] NamedTuple AD-P1-5 理由修正 + parity 标注 → ✅ 已解决

**v1 要求**：修正 AD-P1-5 理由（v1 错误声称"对齐 Python factory(**obj)"，实际两者不对齐）+ §6.1.6 NT-10 + §7.5 补充差异。

**v2 验证**（三处标注齐全）：

1. **§8.1 AD-P1-5 理由修正**（L2409）：
   - 明确标注"**parity 差异（C-2 修正）**"；
   - Python（core.py L3416）`factory(**obj)` 传 Container **所有**字段，多余字段触发 `TypeError`（严格）；
   - construct-rs 只传 tuplefields 命名字段，**忽略**额外字段（宽松）；
   - 承认"v1 理由'对齐 factory(**obj) 语义'描述错误，已修正"——自省到位，无掩盖。

2. **§6.1.6 NT-10 边界条件**（L2133）：完整说明 construct-rs 忽略额外字段 vs Python `TypeError: __init__() got an unexpected keyword argument`，含 core.py 行号。

3. **§7.5 测试策略 parity 差异清单**（L2392）："NamedTuple over Struct 的多余字段处理（C-2，§6.1.6 NT-10）"条目，明确 construct-rs 更宽松。

**结论**：✅ C-2 完全解决。差异方向（construct-rs 宽松忽略 / Python 严格报 TypeError）描述准确，v1 错误理由已显式修正。

---

### C-3 [完备性] Union build context.update + flagbuildnone 标注 → ✅ 已解决

**v1 要求**：§3.4 build 补充 `context.update(obj)` 等价逻辑或注明限制 + §3.7 补充 flagbuildnone 边界条件。

**v2 验证**（四处标注齐全）：

1. **§3.4 build 后 parity 说明**（L1049-1056）：分两点逐一说明
   - **`context.update(obj)`**（Python L3732）：Python 合并 obj 全字段进 context，construct-rs child_ctx 仅含被选中 subcon 字段 → 跨 subcon 引用受限；给出 DEV 补齐方案（child_ctx 遍历 obj 全字段 set_field_at）。
   - **`flagbuildnone`**（Python L3734-3735）：Python 用 `obj.get(name, None)` 允许缺键传 None；construct-rs 缺键时跳过该 subcon；给出 DEV 补齐方案（UnionSubcon 增 flag_build_none 字段）。

2. **§3.7 UN-14 边界条件**（L1113）：flagbuildnone subcon 缺键场景，标注 parity 差异 + core.py 行号。

3. **§3.7 UN-15 边界条件**（L1114）：跨 subcon 引用场景（context.update 差异），标注 parity 限制 + core.py 行号。

4. **§7.5 测试策略 parity 差异清单**（L2393）："Union build 的 context 合并与 flagbuildnone（C-3，§3.4 + UN-14/UN-15）"条目。

**结论**：✅ C-3 完全解决。两点差异（context 合并 + flagbuildnone）均独立标注，含 core.py 行号、影响评估、DEV 补齐方案。ARCH 选择"注明限制 + 给补齐路径"而非"立即补齐实现"——合理（多数用例不受影响，避免过度设计）。

---

## 复检总结

| 驳回项 | 类别 | v2 状态 | 标注位置 | 质量评估 |
|--------|------|--------|---------|---------|
| F-1 | 可行性·驳回级 | ✅ 解决 | §6.2.3 L2172-2250 | 顶部无 arrow / 函数体内 import / 根因修正说明完整 |
| C-1 | 完备性·需补充 | ✅ 解决 | L1325 + L1413 + L2391 | 三处互相引用，差异方向准确 |
| C-2 | 完备性·需补充 | ✅ 解决 | L2409 + L2133 + L2392 | v1 错误理由显式修正，差异方向准确 |
| C-3 | 完备性·需补充 | ✅ 解决 | L1049-1056 + L1113-1114 + L2393 | 四处标注，两点独立说明 + DEV 补齐方案 |

**标注质量通用评估**：
- 所有 parity 标注均引用 Python core.py 行号（L2410/L3416/L3478/L3732/L3734-3735）——可追溯。
- 均明确说明差异方向（哪个更严格 / 哪个更宽松）——用户迁移可判断。
- C-1/C-2 给出用户迁移指引，C-3 给出 DEV 补齐路径——可操作。
- 代码注释（§4.5）、边界条件（§4.8/§6.1.6/§3.7）、parity 清单（§7.5）三/四处互相引用——无孤岛标注。

**建议项 C-4/C-5/C-6**：v2 修订记录已明确转 CODING 阶段由 DEV 实施时补充边界条件，不阻塞设计通过——处理方式合理，符合 v1 报告"可在 CODING 阶段处理"的判定。

---

## v2 复检结论：✅ 通过

**结论**：v1 全部驳回项（F-1 驳回级 + C-1/C-2/C-3 需补充）均已完整解决，标注质量达标。设计无新增延续性 / 性能 / 整体性 / 可行性 / 完备性问题。

**下一步**：PM 推进至 CODING 阶段，分派 DEV 按 §0.5 实施顺序（Enum/FlagsEnum/Mapping → OneOf/NoneOf/Validator → Union → Sequence → ProcessXor/ProcessRotateLeft → NamedTuple/Timestamp）实施。

**遗留（DEV 实施时处理，不阻塞）**：
- C-4（Mapping TypeError 捕获）/ C-5（Timestamp subcon 类型检查）/ C-6（Enum bool 边界）：DEV 实施时补充对应边界条件。
- C-3 补齐路径：若 bench/parity 发现 flagbuildnone 或跨 subcon 引用常见用例受影响，DEV 按 §3.4 L1049-1056 给出的方案补齐。

---

> **v2 复检完成时间**：2026-07-31
> **结论**：✅ 通过（DESIGN_REVIEW 完成）
> **下一步**：PM → CODING
