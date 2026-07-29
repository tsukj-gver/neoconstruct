---
id: DESIGN-Array-Perf
status: active
phase: "4"
depends_on: [DESIGN-Array, ADR-016, ADR-017, ADR-018]
supersedes: []
superseded_by: []
last_updated: 2026-07-27
---

# 模块设计：Array 系列节点 per-iter 性能优化

## 模块位置

- `construct-rs/src/error.rs`（ConstructError 新增 `push_path_index` 方法）
- `construct-rs/src/path.rs`（Path 类型零分配优化）
- `construct-rs/src/nodes/array.rs`（ArrayNode 迁移到 lazy path + PyList 优化）
- `construct-rs/src/nodes/greedy_range.rs`（同上）
- `construct-rs/src/nodes/prefixed_array.rs`（同上）
- `construct-rs/src/nodes/repeat_until.rs`（同上，仅 Expr 路径 PyList 优化）

## 职责

消除 Array 系列节点（Array / GreedyRange / PrefixedArray / RepeatUntil）parse/build 循环中的 per-iteration 不必要开销，使受影响的 5 个 C 类场景（i01-i04 parse、g01 parse 边缘）在 apples-to-apples 口径下达到 ≥10x 加速比。

---

## §1 问题背景（C 类根因摘要）

### 1.1 根因来源

引用 `experiments/phase4_perf_investigation_report.md` §4.2 的 **严重度 2：C 类（per-iteration 不必要工作）**：

> `ArrayNode::parse/build` 每次迭代调 `path.push_index(i)` + `path.pop()`。这两个操作是错误路径专用（仅 `to_string()` 时用），但被无条件执行。

**代码位置**：`array.rs:163/176`、`greedy_range.rs:126/130/139/147`、`prefixed_array.rs:140/144/151`、`repeat_until.rs:268/271/277/345/348/354/424/428/435/500/504/510`。

**影响**：每元素多消耗 ~5-7ns。对 N=100 的 Index 场景，多消耗 500-700ns，让加速比从 ~10x 跌到 9.17x。

### 1.2 P0-3 先例：StructNode 已完成 lazy path 迁移

**关键发现**：StructNode 已在 Phase 1 P0-3 优化中完成了 lazy path 迁移（`struct_node.rs:27-32` 注释）：

```
//! ## 路径追踪（P0-3 优化：成功路径零成本）
//! 成功路径**不**调用 `path.push_field`/`path.pop`（消除每字段 String 堆分配）。
//! 子节点返回 Err 时，通过 ConstructError::push_path_segment 将当前字段名
//! 插入错误路径，重建完整路径。
```

StructNode 的模式：
1. 成功路径：不调 `path.push_field`/`path.pop`，`path` 参数仅透传给子节点
2. 错误路径：子节点返回 `Err(e)` 时，调 `e.push_path_segment(field_name)` 重建路径

**Array 系列节点是仅存的未迁移节点**。本次设计将 P0-3 模式推广到 Array 系列。

### 1.3 Path::new 堆分配（B 类）

引用 §2.1：

> `Path::new`（1 Vec 堆分配）≈ 20-30ns

每次 parse/build 入口（`schema.rs:161/201`）调一次 `Path::new()`，创建 `vec![PathSegment::Root]`，一次堆分配。对零工作量场景（e01_empty_parse）影响显著。

### 1.4 PyList::append 慢路径

引用 §2.2：

> 已经预分配 capacity，可改用 unsafe `PyList_SET_ITEM` + 手动长度管理，每元素省 2-3ns。

**调查发现**：当前 `PyList::new_bound(py, Vec::with_capacity(count))` 实际创建的是**空 list**（Vec len=0，pyo3 迭代时不保留 capacity hint）。后续每次 `list.append(elem)` 走 CPython `PyList_Append` 慢路径（capacity 检查 + 可能的 realloc）。

---

## §2 path 延迟构建方案

### 2.1 方案决策：方案 B'（StructNode P0-3 模式推广）

**不采用** pydantic-core 的 `Location` 反转 Vec 方案（方案 B），因为 construct-rs 已有成熟的 P0-3 模式（`ConstructError::push_path_segment`），只需推广到 Array 系列。

**不采用** 方案 A（错误时反向重建），因为重建需要节点知道自己在树中的位置，而 P0-3 模式天然利用错误传播链（从叶到根逐层 `push_path_segment`/`push_path_index`），无需额外状态。

**采用** 方案 B'：StructNode P0-3 模式的直接推广。

### 2.2 核心变更

#### 2.2.1 新增 `ConstructError::push_path_index`（error.rs）

与现有 `push_path_segment`（field 名）平行，新增 `push_path_index`（数组索引）：

```rust
/// 在错误路径中插入一个数组索引段。
///
/// 与 push_path_segment 平行，但插入 `[i]` 而非 `.field`。
/// 用于 Array 系列节点在子节点返回 Err 时重建索引路径段。
///
/// # 路径重建规则（INSERT-after-root，与 push_path_segment 一致）
///
/// - `"root"` → `"root[i]"`（叶节点基线）
/// - `"root.x.y"` → `"root[i].x.y"`（Struct 内层已重建，Array 外层插入索引）
/// - `"root[j]"` → `"root[i][j]"`（内层 Array 已重建，外层 Array 插入索引）
/// - `""` → `"root[i]"`（From<PyErr> 路径）
/// - 编译期错误（无 path）→ 不变
pub fn push_path_index(&mut self, i: usize) {
    if self.path().is_none() {
        return;
    }
    let new_path = match self.path() {
        Some(p) if p.is_empty() || p == "root" => format!("root[{}]", i),
        Some(p) => {
            if let Some(suffix) = p.strip_prefix("root") {
                // suffix 为 ""（"root"）或 ".x.y" / "[j]" / ".x[j].y" 等。
                format!("root[{}]{}", i, suffix)
            } else {
                format!("{}[{}]", p, i)
            }
        }
        None => format!("root[{}]", i),
    };
    self.set_path(new_path);
}
```

**与 `push_path_segment` 的关系**：

| 方法 | 插入文本 | 语义 | 调用者 |
|------|---------|------|--------|
| `push_path_segment(name)` | `.{name}` | 字段名 | StructNode |
| `push_path_index(i)`（新增） | `[i]` | 数组索引 | ArrayNode / GreedyRange / PrefixedArray / RepeatUntil |

两者共享 INSERT-after-root 策略（把新段插入到 `"root"` 之后、已有 suffix 之前），保证任意嵌套组合（Struct↔Array）都能正确重建路径。

**正确性验证**（所有嵌套组合）：

| 嵌套结构 | 叶错误 path | 重建步骤 | 最终 path |
|---------|-----------|---------|----------|
| Array[2] Byte | `"root"` | Array.push_index(2) | `"root[2]"` ✓ |
| Struct{items: Array[2] Byte} | `"root"` | Array.push_index(2) → Struct.push_segment("items") | `"root.items[2]"` ✓ |
| Array[1] Struct{x: Byte} | `"root"` | Struct.push_segment("x") → Array.push_index(1) | `"root[1].x"` ✓ |
| Array[1] Array[2] Byte | `"root"` | inner.push_index(2) → outer.push_index(1) | `"root[1][2]"` ✓ |
| Struct{a: Struct{b: Array[2] Byte}} | `"root"` | Array.push_index(2) → inner.push_segment("b") → outer.push_segment("a") | `"root.a.b[2]"` ✓ |

**O1 兼容性**：现有 O1 修复（4.6）移除了 ArrayNode 错误路径的 `push_path_segment("[i]")` 调用，因为 inner.parse 在 path 栈含 Index(i) 时已经把 path 写成 `"root[i]"`。迁移到 lazy path 后，inner.parse 不再看到 Index(i)，叶错误 path 为 `"root"`，ArrayNode 通过 `push_path_index(i)` 重建。最终 path 与 O1 修复前一致（`"root[i]"`），O1 测试（`parse_const_error_path_no_double_index_marker` 等）继续通过。

#### 2.2.2 Array 系列节点迁移模式

以 ArrayNode.parse 为例（其他节点同构）：

**迁移前**（当前代码）：
```rust
for i in 0..count {
    ctx.set_index(i);
    path.push_index(i);                    // ← 每次迭代调用（浪费）
    let elem = match self.inner.parse(py, stream, ctx, path) {
        Ok(v) => v,
        Err(e) => {
            path.pop();                    // ← 错误路径栈平衡
            restore_index(ctx, old_index);
            return Err(e);
        }
    };
    path.pop();                            // ← 每次迭代调用（浪费）
    if !self.discard {
        list.append(elem)...;
    }
}
```

**迁移后**：
```rust
for i in 0..count {
    ctx.set_index(i);
    // path.push_index(i) 已移除——成功路径不维护 path 栈
    let elem = match self.inner.parse(py, stream, ctx, path) {
        Ok(v) => v,
        Err(mut e) => {
            // 错误路径：重建 index 段（与 StructNode P0-3 模式一致）
            e.push_path_index(i);
            restore_index(ctx, old_index);
            return Err(e);
        }
    };
    // path.pop() 已移除
    if !self.discard {
        list.append(elem)...;  // 或 Vec push（见 §4）
    }
}
```

**变化**：
- 成功路径：减少 2 次 Vec 操作（push + pop）per iteration ≈ 节省 5-7ns/elem
- 错误路径：增加 1 次 `push_path_index` 调用（format! + set_path），约 ~50-100ns（仅在错误时，可接受）

### 2.3 影响范围

| 文件 | 修改点 | 行号（当前） |
|------|--------|-------------|
| `error.rs` | 新增 `push_path_index` 方法 | 在 `push_path_segment` 之后（~line 405） |
| `array.rs` | parse + build 移除 push/pop，错误路径加 push_index | 161-182（parse）、239-253（build） |
| `greedy_range.rs` | parse + build 同上 | 120-153（parse）、196-218（build） |
| `prefixed_array.rs` | parse + build 同上 | 138-153（parse）、209-221（build） |
| `repeat_until.rs` | parse + build（Expr + PyCallable 两路径）同上 | 422-451（parse_expr）、490-531（parse_callable）、262-292（build Expr）、329-377（build Callable） |

**不受影响的节点**：
- StructNode：已迁移（P0-3）
- 叶节点（FormatField / Bytes / Padding 等）：从不 push path，只在错误时读 `path.to_string()`
- BitwiseNode / TellNode / StopIfNode：生产代码不 push path（仅测试代码中有 push_field 调用）

### 2.4 错误消息兼容性

**保证不变**：`ConstructError.path` 字段的格式（`"root.field[N]"`）在迁移后保持完全一致。

**现有 O1 测试覆盖**（迁移后必须继续通过）：
- `array.rs: parse_const_error_path_no_double_index_marker`（嵌套 Array path 格式）
- `array.rs: parse_const_insufficient_returns_stream_error`（`"root[2]"`）
- `greedy_range.rs: build_propagates_inner_error`（`"root[0]"` 或 endswith `[0]`）
- `prefixed_array.rs: build_propagates_inner_error`（同上）
- `prefixed_array.rs: build_count_overflow_propagates_countfield_error`（path 含 `"countfield"`）

### 2.5 风险评估

| 风险项 | 级别 | 缓解措施 |
|--------|------|---------|
| 错误 path 格式回归 | 中 | 现有 O1 测试 + 新增嵌套组合测试（§6） |
| 遗漏某个 Array 节点 | 低 | 4 个节点逐一列举，grep 验证零残留 push_index/pop |
| push_path_index 格式错误 | 低 | 穷举嵌套组合的单元测试 |
| 性能未达预期 | 低 | 4.x-INVEST 已定位到精确代码行，优化方向明确 |

---

## §3 Path 类型优化方案

### 3.1 当前状态

```rust
// path.rs（当前）
pub struct Path {
    segments: Vec<PathSegment>,  // 堆分配
}

impl Path {
    pub fn new() -> Self {
        Self { segments: vec![PathSegment::Root] }  // ← 每次 parse/build 一次堆分配
    }
}
```

**调用点**：`schema.rs:161`（parse 入口）、`schema.rs:201`（build 入口）—— 每次 parse/build 调用一次。

### 3.2 方案决策：方案 A（两态 enum，零分配）

**不采用** 方案 B（SmallVec / ArrayVec），理由：
1. 引入新依赖（smallvec crate），需要论证必要性——但 §2 的 lazy path 迁移后，生产代码不再 push path，SmallVec 的容量永远不会超出 inline 存储，浪费。
2. 引入方案 A 能完全消除堆分配，比 SmallVec 更优。

**不采用** 方案 C（thread_local 缓冲区），理由：
1. thread_local 有 TLS 访问开销（~5-10ns），可能抵消省下的分配开销。
2. thread_local 在多线程 GIL 场景下语义复杂，不必要。

**采用** 方案 A：两态 enum。

### 3.3 迁移后的 Path 使用模式

完成 §2 lazy path 迁移后：
- **成功路径**：Path 创建为 `Root`，从不 push，从不读。零成本。
- **错误路径**：叶节点读 `path.to_string()`（返回 `"root"`），父节点通过 `err.push_path_segment` / `err.push_path_index` 重建路径。Path 对象本身保持 `Root` 态不变。
- **测试代码**：少量测试显式 push_field 验证 path 行为，会触发 `Root → Segments` 转换（不影响生产性能）。

### 3.4 新 Path 类型设计

```rust
/// 错误路径追踪栈。
///
/// 迁移到 lazy path 模式（P0-3）后，生产代码成功路径从不 push。
/// `Root` 变体覆盖 >99% 场景，零堆分配。
#[derive(Debug, Clone)]
pub enum Path {
    /// 仅含根段。零分配。
    ///
    /// 这是 lazy path 模式下的常态：成功路径不 push，叶节点读 to_string() 得 "root"。
    Root,
    /// 扩展路径（push 后转换）。
    ///
    /// 仅在显式调用 push_field/push_index 后产生。生产代码不触发（lazy path 模式），
    /// 保留用于测试代码与未来可能的 eager path 节点。
    Segments(Vec<PathSegment>),
}
```

**API 变化**：

| 方法 | 当前实现 | 新实现 |
|------|---------|--------|
| `new()` | `vec![Root]`（堆分配） | `Path::Root`（零分配） |
| `push_field(name)` | `segments.push(Field(name))` | `Root` → `Segments(vec![Root, Field(name)])`；`Segments` → push |
| `push_index(i)` | `segments.push(Index(i))` | 同上 |
| `pop()` | `segments.pop()` | `Segments` → pop；若变空则回到 `Root`（或 Empty） |
| `to_string()` | 遍历 segments | `Root` → `"root"`；`Segments` → 遍历 |
| `len()` | `segments.len()` | `Root` → 1；`Segments` → len |
| `is_empty()` | `segments.is_empty()` | `Root` → false；`Segments` → is_empty |

**性能影响**：
- `Path::new()`：从 ~20-30ns（Vec 分配）→ ~0ns（enum 构造）
- `path.to_string()`（错误路径）：`Root` → 从遍历 1 元素变直接返回 `"root"` 字面量，省一次 match 分支

### 3.5 影响范围

- `path.rs`：类型重写（API 签名不变，实现变更）
- 所有调用 `Path::new()` / `path.to_string()` / `path.push_*` / `path.pop()` 的代码：**无需修改**（API 兼容）
- 所有 `&mut Path` 参数：**无需修改**

### 3.6 风险评估

| 风险项 | 级别 | 缓解措施 |
|--------|------|---------|
| enum 变体匹配遗漏 | 低 | 编译器 exhaustiveness 检查 |
| pop 回到 Root 的边界条件 | 低 | 新增边界测试（pop 到空、pop Root 不 panic） |
| Display 格式回归 | 低 | 现有 path.rs 测试全部保留 |

---

## §4 PyList 构建路径评估

### 4.1 当前实现分析

ArrayNode.parse 当前实现：
```rust
let list = PyList::new_bound(py, Vec::<Py<PyAny>>::with_capacity(count));
for i in 0..count {
    // ...
    list.append(elem).map_err(ConstructError::from)?;
}
```

**问题**：`PyList::new_bound(py, Vec::with_capacity(count))` 传入的是一个**空 Vec**（len=0, capacity=count）。pyo3 0.22 的 `PyList::new_bound` 通过迭代创建 PyList——空迭代器产生空 list（`len=0`, PyList 内部 `allocated=0`）。Vec 的 capacity hint **不被保留**。

后续每次 `list.append(elem)` 调 CPython `PyList_Append`：
1. 检查 `allocated > ob_size`（首次 false → 触发 `list_resize`）
2. `list_resize` 按增长策略（~1.125x）realloc ob_item 数组
3. 写入元素 + `ob_size += 1`

对 N=1000，约触发 ~log₁.₁₂₅(1000) ≈ 60 次 realloc。

### 4.2 方案决策：Rust Vec 中转 + 一次性 PyList 创建

**采用** 方案：用 `Vec<Py<PyAny>>` 收集元素，循环结束后一次性转为 PyList。

```rust
// ArrayNode.parse 优化后
let mut elems: Vec<Py<PyAny>> = Vec::with_capacity(count);
for i in 0..count {
    ctx.set_index(i);
    let elem = self.inner.parse(py, stream, ctx, path)?;
    if !self.discard {
        elems.push(elem);  // Rust Vec push ~1-2ns（无 FFI）
    } else {
        drop(elem);
    }
}
let list = PyList::new_bound(py, elems);  // 一次性创建，内部 PyList_SET_ITEM
```

**pyo3 `PyList::new_bound(py, vec)` 内部行为**（非空 Vec）：
1. `PyList_New(vec.len())` —— 一次性分配 `len` 个 slot（一次 malloc）
2. 对每个元素调 `PyList_SET_ITEM`（unsafe 直接写入，无 capacity 检查）
3. 消费 Vec（避免双重 free）

这比手动 `PyList_SET_ITEM` + 手动管理 `ob_size` 更安全（pyo3 处理 unsafe），且性能等价。

### 4.3 各节点适用性

| 节点 | parse 方向 | 适用 Vec 中转？ | 理由 |
|------|-----------|----------------|------|
| ArrayNode | ✓ | **是** | 内部不传 list 给 inner，count 已知 |
| GreedyRangeNode | ✓ | **是** | 内部不传 list 给 inner，count 未知但 Vec 动态增长高效 |
| PrefixedArrayNode | ✓ | **是** | 同 ArrayNode |
| RepeatUntilNode Expr path | ✓ | **是** | Expr 谓词不依赖 list（用 ctx._current_elem_ptr） |
| RepeatUntilNode PyCallable path | ✗ | **否** | Python 谓词接收 `(obj, list, ctx)`，需实时 PyList |

**GreedyRange 特殊处理**：count 未知，用 `Vec::new()` 起步。Rust Vec 的增长策略（doubling）比 CPython list（~1.125x）更高效——对 N=1000，Vec 仅 ~10 次 realloc vs PyList ~60 次。

**RepeatUntil PyCallable path**：保持当前 PyList 实现。该路径每次迭代调 Python 谓词（~500-1000ns/iter），PyList::append 的 ~5ns 开销占比 < 1%，不值得优化。

### 4.4 build 方向

build 方向不需要构建 PyList（从已有 Python list 读取），无需优化。

### 4.5 预期收益

| 节点 | per-elem 节省 | N=100 总节省 | N=1000 总节省 |
|------|-------------|-------------|--------------|
| ArrayNode | ~3-5ns | 300-500ns | 3000-5000ns |
| GreedyRangeNode | ~3-5ns | 同上 | 同上 |
| PrefixedArrayNode | ~3-5ns | 同上 | 同上 |
| RepeatUntil Expr | ~3-5ns | 同上 | 同上 |

### 4.6 风险评估

| 风险项 | 级别 | 缓解措施 |
|--------|------|---------|
| 中途错误时元素引用计数 | 低 | `Vec<Py<PyAny>>` drop 时自动 decref 所有元素，与 PyList drop 行为等价 |
| discard 路径行为变化 | 无 | discard 时不 push 到 Vec，与不 append 到 list 等价 |
| GreedyRange 回退后 Vec 状态 | 低 | GreedyRange 在回退时 break 循环，Vec 仅含成功解析的元素，行为与 list 一致 |

---

## §5 DEV 实施清单

### 优先级排序

| 优先级 | 实施项 | 预期收益 | 风险 |
|--------|--------|---------|------|
| P0 | Item 1: `push_path_index` 方法 | 前置依赖（Item 2-5 需要） | 低 |
| P0 | Item 2: ArrayNode lazy path | 5-7ns/elem × N | 中 |
| P0 | Item 3: GreedyRangeNode lazy path | 5-7ns/elem × N | 中 |
| P0 | Item 4: PrefixedArrayNode lazy path | 5-7ns/elem × N | 中 |
| P0 | Item 5: RepeatUntilNode lazy path | 5-7ns/elem × N | 中 |
| P1 | Item 6: Path::new 零分配 | 20-30ns/call | 低-中 |
| P1 | Item 7: PyList Vec 中转（Array/Greedy/Prefixed） | 3-5ns/elem × N | 中 |
| P1 | Item 8: RepeatUntil Expr path PyList Vec 中转 | 3-5ns/elem × N | 低 |

### Item 1: ConstructError::push_path_index 方法

**文件**：`construct-rs/src/error.rs`
**位置**：在 `push_path_segment` 方法之后（约 line 405）
**修改要点**：
- 新增 `pub fn push_path_index(&mut self, i: usize)` 方法
- 实现 INSERT-after-root 策略（见 §2.2.1 的完整代码）
**风险等级**：低（纯新增 API，不改现有行为）
**测试影响**：
- 新增单元测试：push_path_index 单独使用、与 push_path_segment 混合使用、嵌套 Array、Struct↔Array 组合
- 现有测试不受影响
**预期加速比提升**：无直接收益（是 Item 2-5 的前置依赖）

### Item 2: ArrayNode 迁移到 lazy path

**文件**：`construct-rs/src/nodes/array.rs`
**位置**：
- parse：line 161-182（循环体）
- build：line 239-253（循环体）
**修改要点**：
- 移除 `path.push_index(i)` 和所有 `path.pop()` 调用
- 错误分支改为 `Err(mut e) => { e.push_path_index(i); restore_index(ctx, old_index); return Err(e); }`
- 更新模块文档注释（移除"path.push_index/pop 每次迭代调用"的说明）
**风险等级**：中（涉及错误路径格式）
**测试影响**：
- 现有 O1 测试必须继续通过：`parse_const_insufficient_returns_stream_error`（path = `"root[2]"`）、`parse_const_error_path_no_double_index_marker`（嵌套 path = `"root[1][1]"`）
- `build_length_mismatch_returns_range_error`：path 来自 `path.to_string()`（入口时 = `"root"`），不受影响
**预期加速比提升**：5-7ns/elem。对 i02 (N=100) 约提升 0.8-1.2x 加速比

### Item 3: GreedyRangeNode 迁移到 lazy path

**文件**：`construct-rs/src/nodes/greedy_range.rs`
**位置**：
- parse：line 120-153（loop 体，含 3 个分支的 path.pop）
- build：line 196-218（for 循环体，含 3 个分支的 path.pop）
**修改要点**：
- parse 的 3 个错误分支（Ok/StopField/Err）中的 `path.pop()` 和 `path.push_index(i)` 移除
- parse 的 Err 分支（非 StopField，吞错误并 break）：**不需要** push_path_index（错误被丢弃，对齐 Python `except Exception` 语义）
- build 的错误分支加 `e.push_path_index(i)`
**风险等级**：中
**特殊注意**：GreedyRange parse 的"吞错误回退"分支（line 143-151）**不调** push_path_index——错误被丢弃，不向上传播，无需重建 path。仅保留 `stream.seek(fallback, path)` 中的 path 参数（此时 path 仍为 `Root`，seek 内部不读 path 的内容仅传递）。
**测试影响**：
- `build_propagates_inner_error`：验证 path = `"root[0]"` 或 endswith `[0]`
**预期加速比提升**：5-7ns/elem。对 g01 parse (N=10) 边际改善（小 N），对大 N 显著

### Item 4: PrefixedArrayNode 迁移到 lazy path

**文件**：`construct-rs/src/nodes/prefixed_array.rs`
**位置**：
- parse：line 138-153（循环体）
- build：line 209-221（循环体）
**修改要点**：同 Item 2
**额外注意**：build 中 countfield 错误的 `e.push_path_segment("countfield")`（line 202）保持不变——这是字段名重建，不是 index 重建。
**风险等级**：中
**测试影响**：
- `build_propagates_inner_error`：path = `"root[0]"`
- `build_count_overflow_propagates_countfield_error`：path 含 `"countfield"`
**预期加速比提升**：5-7ns/elem

### Item 5: RepeatUntilNode 迁移到 lazy path

**文件**：`construct-rs/src/nodes/repeat_until.rs`
**位置**：
- parse_expr_path：line 422-451
- parse_callable_path：line 490-531
- build Expr path：line 262-292
- build PyCallable path：line 329-377
**修改要点**：
- 4 个循环体逐一迁移（与 Item 2 同构）
- parse 的 `Err(e)` 分支加 `e.push_path_index(i)` 后返回
- build 的 `Err(e)` 分支加 `e.push_path_index(i)` 后返回
**风险等级**：中（4 处修改，两套路径）
**测试影响**：
- `parse_callable_inner_failure_propagates_error`：验证 Stream 错误
- `build_callable_no_match_returns_repeat_error`：Repeat 错误的 path
**预期加速比提升**：5-7ns/elem（Expr path 主要受益；PyCallable path 被 Python 谓词开销主导，改善微小）

### Item 6: Path::new 零分配优化

**文件**：`construct-rs/src/path.rs`
**位置**：全文件重写类型定义（API 不变）
**修改要点**：
- `struct Path { segments: Vec<PathSegment> }` → `enum Path { Root, Segments(Vec<PathSegment>) }`
- `new()` → `Path::Root`
- `push_field` / `push_index` → Root 转 Segments 再 push
- `pop()` → Segments pop；若空则按语义处理
- `to_string()` / `len()` / `is_empty()` → match 两变体
- `Display` impl → match 两变体
**风险等级**：低-中（类型变更但 API 不变）
**测试影响**：
- 现有 path.rs 全部单元测试必须通过
- 新增边界测试：`pop` on Root（不 panic）、Root → push → pop → 回到 Root 态
**预期加速比提升**：20-30ns/call（每次 parse/build 入口省一次 Vec 分配）。对零工作量场景（e01）最显著但对达 10x 无决定性影响

### Item 7: PyList Vec 中转（Array/Greedy/Prefixed parse）

**文件**：
- `construct-rs/src/nodes/array.rs`（parse，与 Item 2 同次修改）
- `construct-rs/src/nodes/greedy_range.rs`（parse，与 Item 3 同次修改）
- `construct-rs/src/nodes/prefixed_array.rs`（parse，与 Item 4 同次修改）
**修改要点**：
- 将 `let list = PyList::new_bound(py, ...)` 改为 `let mut elems: Vec<Py<PyAny>> = Vec::with_capacity(count)`（GreedyRange 用 `Vec::new()`）
- 循环内 `list.append(elem)` 改为 `elems.push(elem)`
- 循环结束后 `let list = PyList::new_bound(py, elems);`
**风险等级**：中
**建议**：与 Item 2/3/4 合并到同一 DEV 任务（同文件、同函数，拆开反而增加冲突）
**测试影响**：
- parse 测试验证返回值仍为 PyList 且元素正确
- discard 路径测试验证返回空 list
**预期加速比提升**：3-5ns/elem。与 Item 2 叠加后 i02 (N=100) 预计 9.17x → 11-13x

### Item 8: RepeatUntil Expr path PyList Vec 中转

**文件**：`construct-rs/src/nodes/repeat_until.rs`
**位置**：`parse_expr_path` 函数（line 411-451）
**修改要点**：
- 仅 Expr path 的 `list` 改为 Vec 中转
- PyCallable path 保持 PyList（谓词需要实时 list 参数）
**风险等级**：低（PyCallable path 不变）
**建议**：与 Item 5 合并
**预期加速比提升**：3-5ns/elem（Expr path）

---

## §6 测试覆盖要求

### 6.1 push_path_index 单元测试（Item 1 新增）

在 `error.rs` 测试模块新增：

| 测试名 | 场景 | 期望 path |
|--------|------|----------|
| `push_path_index_on_root_base` | `"root"` + push_index(2) | `"root[2]"` |
| `push_path_index_on_empty_base` | `""` + push_index(0) | `"root[0]"` |
| `push_path_index_nested_arrays` | `"root"` → push_index(1) → push_index(2) | `"root[2][1]"` |
| `push_path_index_then_segment` | `"root"` → push_index(1) → push_segment("x") | `"root.x[1]"` |
| `push_path_segment_then_index` | `"root"` → push_segment("x") → push_index(1) | `"root[1].x"` |
| `push_path_index_deeply_nested` | 模拟 Array[1] Struct{x: Array[2] Byte} | `"root[1].x[2]"` |
| `push_path_index_preserves_message` | 验证 message 字段不被修改 | — |
| `push_path_index_on_compilation_error_noop` | Compilation 无 path | None |

### 6.2 lazy path 迁移回归测试（Item 2-5）

**现有 O1 测试必须全部通过**（不新增，仅验证不回归）：

| 文件 | 测试名 | 验证的 path 格式 |
|------|--------|-----------------|
| array.rs | `parse_const_insufficient_returns_stream_error` | `"root[2]"` |
| array.rs | `parse_const_error_path_no_double_index_marker` | `"root[1][1]"` |
| greedy_range.rs | `build_propagates_inner_error` | endswith `[0]`，不含 `.[` |
| prefixed_array.rs | `build_propagates_inner_error` | endswith `[0]` |
| prefixed_array.rs | `build_count_overflow_propagates_countfield_error` | 含 `"countfield"` |

**新增嵌套组合测试**（在各自节点的测试模块中）：

| 测试名 | 场景 | 期望 path |
|--------|------|----------|
| `array_parse_struct_inside_array_error_path` | Array(3, Struct{x: Byte})，Array[1].x EOF | `"root[1].x"` |
| `array_build_struct_inside_array_error_path` | 同上 build 方向 | `"root[1].x"` |
| `greedy_range_parse_nested_in_array_error_path` | Array(2, GreedyRange(Byte)) 内层 EOF | 验证含 `[` |

### 6.3 Path 零分配测试（Item 6）

| 测试名 | 验证点 |
|--------|--------|
| `new_returns_root_variant` | `Path::new()` 不分配（可通过 size_of 或行为推断） |
| `root_to_string_returns_literal` | `Path::Root.to_string()` == `"root"` |
| `push_on_root_transitions_to_segments` | push 后变 Segments 变体 |
| `pop_to_empty_transitions_back` | Segments pop 到空后回到 Root（或 Empty） |
| `all_existing_path_tests_pass` | 现有 18 个 path 测试全部通过 |

### 6.4 PyList Vec 中转测试（Item 7-8）

**现有 round-trip 测试覆盖正确性**：
- `round_trip_const_array_preserves_data`
- `round_trip_greedy_range_preserves_data`
- `round_trip_prefixed_array_preserves_data`

**新增**：
| 测试名 | 验证点 |
|--------|--------|
| `parse_returns_native_list_type` | 返回值 `isinstance(result, list)` 为 True |
| `parse_discard_returns_empty_list_with_vec` | discard=True 仍返回空 list |

### 6.5 性能验证（PM 执行）

完成全部 8 个 Item 后，PM 执行：
1. `maturin develop --release`
2. 运行 `experiments/phase4_perf_investigation.py all`（17 场景 + B1 同次对照）
3. 验证 i02/i03/i04 parse 加速比 ≥ 10x
4. 验证 g01 parse 加速比 ≥ 10x（apples-to-apples）
5. 验证无任何场景加速比下降

---

## §7 流程建议（trivial vs 完整流程）

### 7.1 分类依据

AGENTS.md §1 简化规则：trivial 性质的子任务可跳过 DESIGNING 和 DESIGN_REVIEW，仅走 CODING → CODE_REVIEW。

**本文档即为设计阶段产出**（已由 ARCH 完成）。在 REV 批准本设计后，所有 Item 的设计阶段即告完成，后续仅需 CODING → CODE_REVIEW。

### 7.2 推荐分派方案

| 分派批次 | 包含 Item | DEV 任务数 | 流程 | 理由 |
|---------|----------|-----------|------|------|
| 批次 1 | Item 1 | 1 | CODING → CODE_REVIEW | 前置依赖，必须先完成 |
| 批次 2 | Item 2+7 (array)、Item 3+7 (greedy)、Item 4+7 (prefixed) | 3 | CODING → CODE_REVIEW | 同文件 lazy path + PyList 合并，避免冲突 |
| 批次 3 | Item 5+8 (repeat_until) | 1 | CODING → CODE_REVIEW | 同文件合并 |
| 批次 4 | Item 6 (path.rs) | 1 | CODING → CODE_REVIEW | 独立文件，可并行 |

**批次 1 必须先于批次 2/3**（Item 2-5 依赖 `push_path_index`）。
**批次 2/3/4 可并行**（不同文件）。

### 7.3 不需要额外 DESIGNING 的理由

| Item | 是否需要新 DESIGNING | 理由 |
|------|---------------------|------|
| 1 (push_path_index) | 否 | 本文档 §2.2.1 提供完整代码 |
| 2-5 (lazy path) | 否 | 本文档 §2.2.2 提供迁移前后代码对比 + StructNode P0-3 先例 |
| 6 (Path enum) | 否 | 本文档 §3.4 提供完整类型设计 |
| 7-8 (PyList Vec) | 否 | 本文档 §4.2 提供完整代码模式 |

### 7.4 CODE_REVIEW 重点

VET 审查时应重点关注：
1. **错误 path 格式**：所有 O1 测试 + §6.2 新增嵌套组合测试通过
2. **引用计数安全**：Vec<Py<PyAny>> drop 时正确 decref（Item 7-8）
3. **discard 路径**：discard=True 时 Vec 不收集元素（Item 7-8）
4. **Path enum 边界**：pop on Root 不 panic（Item 6）
5. **无残留 push_index/pop**：grep 验证生产代码中 `path.push_index` / `path.pop()` 调用为零（Item 2-5 完成后）

### 7.5 预期总加速比提升

基于 4.x-INVEST 数据（apples-to-apples 口径），完成全部 8 个 Item 后：

| 场景 | 当前 | 预期（P0+P1 后） | 达 10x？ |
|------|------|-----------------|---------|
| i02 parse (N=100) | 9.17x | **11-13x** | ✓ |
| i03 parse (N=1000) | 9.46x | **11-13x** | ✓ |
| i04 parse (N=4096) | 9.86x | **11-13x** | ✓ |
| i01 parse (N=10) | 6.76x | ~7.5-8x | 否（结构性） |
| i01 build (N=10) | 9.95x | **~11x** | ✓ |
| g01 parse (N=10) | 9.09x | **~10-10.5x** | ✓ 边缘 |
| a01 parse (N=10) | 7.93x | ~8.5-9x | 否（结构性） |
| e01 parse (N=0) | 6.22x | ~6.5-7x | 否（结构性） |

**预计新增达标场景**：i02、i03、i04、i01 build、g01 parse 共 5 个场景跨过 10x 门槛。

**不达标场景**（结构性硬伤，超出本设计范围）：
- i01/a01 parse (N=10)：Python per-elem 已接近框架开销下界，小 N 下 Rust 固定开销占比过高
- e01 parse (N=0)：零工作量 + FFI 入口下界，需 Rust < 203ns 才达 10x（当前 ~300ns 优化后仍不够）

---

## 附录：与 Python 版本对应

| Python construct 方法 | construct-rs 对应 | 本次优化影响 |
|----------------------|------------------|------------|
| `Array._parse` (L2511-2550) | `ArrayNode::parse` | path lazy + PyList Vec |
| `Array._build` (L2552-2567) | `ArrayNode::build` | path lazy |
| `GreedyRange._parse` (L2584-2620) | `GreedyRangeNode::parse` | path lazy + PyList Vec |
| `GreedyRange._build` (L2622-2634) | `GreedyRangeNode::build` | path lazy |
| `PrefixedArray._parse` (L4950-4968) | `PrefixedArrayNode::parse` | path lazy + PyList Vec |
| `PrefixedArray._build` (L4970-4983) | `PrefixedArrayNode::build` | path lazy |
| `RepeatUntil._parse` (L2670-2682) | `RepeatUntilNode::parse` | path lazy + PyList Vec (Expr only) |
| `RepeatUntil._build` (L2684-2701) | `RepeatUntilNode::build` | path lazy |

**设计文档结束。**
