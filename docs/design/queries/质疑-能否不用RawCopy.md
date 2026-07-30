---
id: QUERY-rawcopy-architecture
status: active
phase: cross
depends_on:
  - RESEARCH-rawcopy
  - DESIGN-phase6-adapter
  - ADR-007
  - ADR-008
last_updated: 2026-07-30
---

# 设计质疑响应：能否不用 RawCopy？

> **触发**：用户质疑 Python construct 用"组合 RawCopy + Checksum/Hex"解决"获取原始字节"问题，每段需要校验/显示的字节都额外承担一次拷贝 + 一层 dict 包装。如果 Checksum/Hex 自己具备"从流中直接捕获字节范围"的能力，就根本不需要 RawCopy 这个中间层。
>
> **质疑方核心论点**：RawCopy 是 Python 作者的"不聪明"设计，construct-rs 应当跳出原版框架，让消费者自己捕获字节。
>
> **ARCH 立场**：质疑部分成立。详见 §1-§4 分析与 §5 最终建议。
>
> ---
>
> **【2026-07-30 补充段】**：用户追问"为什么要在 Python 层面拷贝？标准哈希函数可以 Rust 实现"。
> ARCH 承认之前的回应存在盲点——把"hashfunc 是 Python callable → 跨 FFI 必须拷贝"当成硬约束，漏了 **Rust 内置 hashfunc** 这条零拷贝路径。
>
> **§7-§10 补充评估**修正了原 §1.4 / §4.2 / §4.4 / §5.6 的结论。核心修正：
> - FFI 入口（PyBytes → &[u8]）本身零拷贝（`schema.rs:155-156` 证据）
> - Rust 内置 hashfunc（sha2/md-5/crc32fast crate）+ StreamRange 模式 = **真正零拷贝**
> - 用户质疑完全成立，详见 §7。

---

## 0. 前置事实（参考实现硬约束）

在分析方案前，必须先固定以下三个**参考实现事实**（这些是后续所有讨论的地基）：

### 0.1 Python 原版 Checksum 的 bytesfunc 是"用户提供的 context lambda"，不是"自己捕获字节"

**源码**（`core.py:5582-5592`）：
```python
def _parse(self, stream, context, path):
    hash1 = self.checksumfield._parsereport(stream, context, path)
    hash2 = self.hashfunc(self.bytesfunc(context))   # ← bytesfunc(context)
    if hash1 != hash2:
        raise ChecksumError(...)
    return hash1
```

**关键事实**：
- Checksum **不读 stream**（除了 checksumfield 自身）
- Checksum **不知道也不关心** bytes 从哪来——只调用用户提供的 `bytesfunc(context)` 拿 bytes
- RawCopy 出现在 Checksum 用法里，**纯粹是因为用户需要 RawCopy 把 raw bytes 写进 context.data**
- **Python 作者没有把"字节捕获"职责放在 Checksum 身上**——这是设计取舍而非疏漏

### 0.2 Python 原版 Hex 的 _decode 接收 inner.parse 结果，不是自己读 stream

**源码**（`core.py:3558-3565`）：
```python
def _decode(self, obj, context, path):
    if isinstance(obj, int):   return HexDisplayedInteger.new(obj, ...)
    if isinstance(obj, bytes): return HexDisplayedBytes(obj)
    if isinstance(obj, dict):  return HexDisplayedDict(obj)   # ← RawCopy 走这里
    return obj
```

**关键事实**：
- Hex 是 Adapter，`_decode(obj)` 的 obj 就是 inner.parse 的结果
- Hex **本身不读 stream**——RawCopy + Hex 组合中，是 RawCopy 把 inner.parse 结果包装成 dict（含 data 键），Hex 拿到 dict 后在 `__str__` 时取 data 做 hexlify
- **`Hex(Int32ub)` 在 Python 原版直接工作**（`core.py:3533-3538`）——返回 HexDisplayedInteger，print 显示 `0x00000102`。不依赖 RawCopy
- **`Hex(Bytes(4))` / `Hex(GreedyBytes)` 也直接工作**——返回 HexDisplayedBytes，print 显示 `unhexlify('00000102')`。也不依赖 RawCopy

### 0.3 Python 原版 RawCopy 本身就是拷贝（不是 construct-rs 实现选择）

**源码**（`core.py:4788-4794`）：
```python
def _parse(self, stream, context, path):
    offset1 = stream_tell(stream, path)
    obj = self.subcon._parsereport(stream, context, path)
    offset2 = stream_tell(stream, path)
    stream_seek(stream, offset1, 0, path)
    data = stream_read(stream, offset2-offset1, path)   # ← 拷贝（CPython bytes 不可变）
    return Container(data=data, value=obj, offset1=..., offset2=..., length=...)
```

**关键事实**（已沉淀于 `设计质疑-三性能优化评估.md §2.2`）：
- Python `stream_read` 返回新 `bytes` 对象（CPython `bytes` 不可变，无零拷贝 view）
- construct-rs 的 `raw_copy.rs:100-111` 与原版**完全等价**
- 拷贝是 CPython 内存模型硬约束，**不是 construct-rs 的实现缺陷**

### 0.4 construct-rs 当前架构能力（用于评估方案可行性）

| 已有能力 | 位置 | 与本质疑的关系 |
|---------|------|--------------|
| `ParseStream::tell() / seek(pos, path) / read(n, path) / seek_whence(at, whence, path)` | `stream.rs:185-325` | Checksum/Hex 自捕获字节的基础——已有，零新增 |
| `ParseStream::data() -> &'a [u8]`（整个底层缓冲） | `stream.rs:335` | 可新增 `slice(start, end)` 零拷贝 API（§4 评估） |
| `Context` 持有 PyDict + parent + _index + expr_values_buf | `context.rs:46-97` | 不持有 raw bytes 范围（§4 评估是否扩展） |
| `RawCopyNode`（已实现，严格 parity 5 键 dict） | `nodes/raw_copy.rs` | parse 性能 3.47-6.12x（RC5 大 N <4x，已接受） |
| Adapter 用户面 Python 层化（ADR-022 + PM 决策 2） | `模块设计-Adapter核心.md §4` | Hex/HexDump 作为 Adapter 子类的实现位置选择 |

---

## 1. Checksum 能否不依赖 RawCopy？

### 1.1 方案矩阵与可行性评估

| 方案 | 核心思路 | 技术可行性 | 性能 | parity 影响 | §0 合规 |
|------|---------|----------|------|-----------|---------|
| **A. Checksum 自记录 offset range** | Checksum 接收 start/end 表达式，自己 seek+read | ✅ 高 | ✅ 优于 RawCopy+Checksum | 扩展（不破坏） | ✅ |
| **B. ParseStream 层 capture_range** | `stream.slice(start, end) -> &'a [u8]` 零拷贝 | ✅ 高（Rust 内部） | ⚠️ 跨 FFI 仍拷贝 | 无 | ✅ |
| **C. Context 层自动旁路记录** | StructNode 自动记录每字段 raw bytes 范围 | ⚠️ 中（破坏 §0 #2） | ❌ 所有 Struct 加开销 | 无 | ❌ |
| **D. Checksum 接受 offset 参数（A 的具体化）** | `Checksum(field, hash, start, end)`（construct-rs 字段名直接引用，非 `this.xxx`） | ✅ 高 | ✅ 最优 | 扩展（不破坏） | ✅ |

### 1.2 方案 A/D 详细设计（ARCH 推荐）

**用户面 API**（construct-rs 扩展，保留原版 bytesfunc 模式作默认）：

```python
# 【Python construct 原版写法】（保留，parity）—— 用户用 RawCopy 把 bytes 塞进 context
d1 = Struct(
    "fields" / RawCopy(Struct(...)),
    "checksum" / Checksum(Bytes(64), hashfunc, this.fields.data),
)

# 【construct-rs 扩展写法】—— 用户用 Tell 标记范围，Checksum 自己读 stream。
# construct-rs 用户面为 dataclass 语法，字段名直接引用（非 this.xxx）。
@dataclass
class D2(StructMixin):
    start: int = rfield(Tell())
    fields: ... = field(...)                      # 被校验的数据（无 RawCopy 包装）
    end: int = rfield(Tell())
    checksum: bytes = rfield(Checksum(Bytes(64), hashfunc, start, end))
```

**Rust 端 ChecksumNode 设计**（待 Checksum 子任务启动时正式设计，此处仅示形）：

```rust
enum BytesSource {
    /// 原版模式：从 context 求值 bytesfunc 表达式得到 bytes 对象
    ContextBytes(ExprProgram),
    /// 扩展模式：从 stream 范围 [start, end) 直接读 bytes
    StreamRange { start: ExprProgram, end: ExprProgram },
}

struct ChecksumNode {
    checksumfield: Box<Node>,
    hashfunc: PyObject,          // Python callable，不可避（hashlib.sha512 等）
    bytes_source: BytesSource,
}
```

**parse 流程**（StreamRange 模式）：
1. `hash1 = checksumfield.parse(stream, ctx, path)` —— 读 checksum 字段
2. `start = eval(start_expr, ctx)` / `end = eval(end_expr, ctx)` —— 求值边界
3. `stream.seek(start, path)?` —— 回退到起始
4. `data = stream.read(end - start, path)?` —— 读字节（CPython 拷贝约束：跨 FFI 给 hashfunc 时仍需 PyBytes）
5. `hash2 = hashfunc(PyBytes::new(data))` —— Python 回调（一次 FFI）
6. 比较 hash1 / hash2，不等 raise ChecksumError

### 1.3 性能对比（StreamRange vs RawCopy+Checksum）

**原版组合的开销**（RawCopy parse 在 Struct 第 i 字段）：
| 步骤 | 开销 | 性质 |
|------|------|------|
| `tell()` | ~1ns | 必需 |
| `inner.parse(Struct(...))` | ~220ns + N×set_item | 内层 Struct 固有 |
| `tell()` | ~1ns | 必需 |
| `seek(offset1)` | ~2ns | 必需 |
| `read(length)` + PyBytes 拷贝 | ~30-50ns | **拷贝**（CPython 硬约束） |
| 5 键 PyDict 构造 | ~150ns（5×set_item） | **dict 包装开销** |
| 后续 Checksum.parse 中 `bytesfunc(context)` 求值 | ~30ns（context.fields.data 访问） | 必需 |

**StreamRange 模式的开销**：
| 步骤 | 开销 | 性质 |
|------|------|------|
| Tell(start) 字段 | ~10ns（含 set_item 写 context） | 替代品 |
| `inner.parse(Struct(...))` | ~220ns + N×set_item | 内层 Struct 固有（同上） |
| Tell(end) 字段 | ~10ns | 替代品 |
| Checksum.parse 中 checksumfield.parse | 同原版 | 必需 |
| `start/end` 表达式求值 | ~12ns（2×GetInt） | 替代 bytesfunc |
| `seek(start)` + `read(end-start)` + PyBytes 拷贝 | ~32-52ns | **拷贝仍存在**（CPython 硬约束） |
| hashfunc 调用 | 同原版 | 必需 |

**节省**：
- **5 键 PyDict 构造**（~150ns）—— 这是 RawCopy 的核心额外开销
- **value 解析的 set_item**（在 Checksum 场景下不需要 value，但 RawCopy 仍构造它）

**无法节省**：
- **PyBytes 拷贝**——CPython bytes 不可变硬约束（§0.3）。只要 hashfunc 是 Python callable，跨 FFI 就必须构造 PyBytes
- **inner.parse 开销**——Struct 本身的 parse 成本不变

**预期净收益**：~150-200ns/字段，加速比从（无独立 Checksum bench，但 RawCopy 是主开销）推断可显著提升。**但拷贝仍存在**——只是从"RawCopy 拷一次 + Checksum 读 dict"变成"Checksum 直接拷一次"。

### 1.4 关键限制：hashfunc 是 Python callable → 拷贝不可避免

**用户质疑"额外承担一份复制开销"的部分真相**：

| 路径 | 拷贝次数 | 说明 |
|------|---------|------|
| RawCopy + Checksum（原版） | **1 次**（RawCopy 的 stream_read） | Checksum 的 bytesfunc 从 context.data 直接拿已构造好的 PyBytes，**无二次拷贝** |
| StreamRange Checksum（方案 D） | **1 次**（Checksum 内部 stream_read） | 同样 1 次 |

**结论**：**两条路径的拷贝次数相同（1 次）**。RawCopy "额外承担一份拷贝"是**直觉错觉**——它确实构造了一个 PyBytes，但这个 PyBytes 就是 Checksum 哈希的输入，Checksum 不会再拷贝。

**RawCopy 真正的额外开销是 5 键 dict 包装（~150ns）+ value 解析**，不是拷贝本身。方案 D 的收益正是省去这部分。

### 1.5 方案 C 为什么不可行（§0 #2 违反）

**方案 C 设计**：Context 增加 `field_ranges: [(usize, usize); N]`，StructNode.parse 在每个字段前后自动调 tell() 记录范围。

**违反 §0 #2（无中间表示层）**：
- field_ranges 是 Rust 侧维护的"元数据中间表示"
- 大多数 Struct 不需要这些范围——为少数场景给所有 Struct 加固定开销
- 与 ADR-007（Context Vec 化的初衷：表达式求值）的设计脉络冲突

**性能损失**：
- 每个 Struct parse 增加 2×N 次 tell()（~2ns × 字段数）
- N=10 的 Struct 增加 ~40ns 固定开销，且 90% 的 Struct 不会用到

**ARCH 结论**：方案 C **驳回**。

---

## 2. Hex/HexDump 能否不依赖 RawCopy？

### 2.1 关键发现：Python 原版 Hex 已经不强制依赖 RawCopy

| Python 用法 | parse 返回类型 | print 输出 | 是否需要 RawCopy |
|------------|--------------|-----------|----------------|
| `Hex(Int32ub)` | `HexDisplayedInteger`（int 子类） | `0x00000102` | **否**（core.py:3533-3538 原版示例） |
| `Hex(Bytes(4))` | `HexDisplayedBytes`（bytes 子类） | `unhexlify('00000102')` | **否**（core.py:3540-3545 原版示例） |
| `Hex(GreedyBytes)` | `HexDisplayedBytes` | 同上 | **否** |
| `Hex(RawCopy(Int32ub))` | `HexDisplayedDict`（dict 子类） | `unhexlify('00000102')` | 是（用户主动选 dict 显示模式） |

**核心事实**：**RawCopy + Hex 是用户主动选择的"dict 显示模式"**，不是 Hex 的设计缺陷。用户想要"原始字节 hex 显示"完全可以用 `Hex(Bytes(N))` 替代。

### 2.2 construct-rs 实现策略（Hex 作为 Rust Node）

**Phase 6.3 决策回顾**（`模块设计-Adapter核心.md §0.2`）：
- 通用 `Adapter` / `SymmetricAdapter` 基类在 Python 层（用户继承，走 AdapterCallbackNode 回调）
- 内置 Adapter 子类（Hex/HexDump/Enum/Validator 等）**可以选择**实现为 Rust Node 或 Python 层

**ARCH 推荐：Hex/HexDump 实现为 Rust Node**（类似 Subconstruct/RawCopy/Peek），不走 AdapterCallbackNode 回调路径。理由：
1. Hex/HexDump 是 construct 核心库内置（非用户自定义），与 Subconstruct 同档
2. parse 逻辑简单（根据 obj 类型分支包装），Rust 内联判断 + Python 显示类构造
3. 避免 AdapterCallbackNode 的 Python 回调开销（每次 parse 1 次 FFI）

**Rust 端 HexNode 设计**（示形，待 Hex 子任务正式设计）：

```rust
struct HexNode {
    inner: Box<Node>,
}

impl Construct for HexNode {
    fn parse(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
        let obj = self.inner.parse(py, stream, ctx, path)?;
        // 根据 obj 类型分支（Rust 内联判断）
        let cls = if obj.is_instance_of::<PyLong>(py) {
            // int → HexDisplayedInteger.new(obj, fmtstr)
            // fmtstr 由 inner.sizeof(ctx) 计算（"0{}X".format(2*size)）
            self.hex_displayed_integer_cls
        } else if obj.is_instance_of::<PyBytes>(py) {
            self.hex_displayed_bytes_cls
        } else if obj.is_instance_of::<PyDict>(py) {
            self.hex_displayed_dict_cls
        } else {
            return Ok(obj);  // 未知类型，原样返回（对齐 Python return obj）
        };
        // 调用 Python 类构造（1 次 FFI，不可避免——显示类是 Python 类）
        Ok(call_python_cls(cls, &[obj])?)
    }
}
```

### 2.3 性能分析

**Hex(RawCopy(Int32ub)) 的 parse 开销**（原版组合）：
| 步骤 | 开销 |
|------|------|
| RawCopy.parse（含 5 键 dict 构造 + 拷贝） | ~430ns |
| Hex._decode(dict) → HexDisplayedDict(obj) | ~50ns（Python 回调 + 类构造） |
| 总计 | ~480ns |

**Hex(Int32ub) 的 parse 开销**（推荐用法）：
| 步骤 | 开销 |
|------|------|
| Int32ub.parse | ~220ns |
| Hex 包装（int → HexDisplayedInteger） | ~50ns（Python 类构造，1 次 FFI） |
| 总计 | ~270ns |

**节省**：~210ns（避免 RawCopy 包装）。但 **Hex(RawCopy(...)) 与 Hex(Int32ub) 是不同显示语义**——前者显示原始字节 hex dump，后者显示 int hex。用户根据需求选择。

### 2.4 parity 影响

**无破坏**：
- `Hex(Int32ub)` / `Hex(Bytes(N))` / `Hex(RawCopy(...))` 三种用法都支持，行为与 Python 原版一致
- 用户代码无需改写

### 2.5 Hex 不需要"自捕获字节"能力

**用户思路**："Hex 直接包装 inner subcon，parse 后从 stream 旁路获取原始字节"

**ARCH 评估**：**不需要**。理由：
1. **Hex(Int32ub) 已经足够**——int 的 hex 显示就是 `0x01020304`，不需要原始字节
2. **Hex(Bytes(N)) 也已足够**——bytes 的 hex 显示就是 `unhexlify('...')`，bytes 本身就是原始字节
3. **只有 Hex(RawCopy(...)) 走 dict 分支**——这是用户主动选择"我想看 dict 结构 + 字节 hex"的特殊需求，可用 Hex(Bytes(N)) 替代

**结论**：Hex 不需要新增"自捕获字节"能力。RawCopy + Hex 的组合可以完全用 Hex(inner) 或 Hex(Bytes(N)) 替代。

---

## 3. 是否建议 RawCopy 标 deprecated？

### 3.1 已有调研回顾（`调研-RawCopy使用场景.md`）

| 场景 | 频率 | 真正需要的键 | 替代方案 |
|------|------|-----------|---------|
| A. Checksum | 6 处 | data | Tell + Checksum(StreamRange)（本质疑 §1 方案 D） |
| B. Hex/HexDump | 4 处 | data | Hex(inner) 或 Hex(Bytes(N))（本质疑 §2） |
| C. EMF 协议 | 1 处 | data + value | **无替代**（需要同时保留 raw bytes 和解析值） |
| D. issue_289/358 | 2 处 | value | Subconstruct(inner)（透传 inner 值） |
| E. 嵌套 | 1 处（测试） | — | 无业务需求 |

### 3.2 ARCH 建议：不标 deprecated，但文档引导

**不建议 deprecated 的理由**：

1. **parity 硬约束**：construct-rs 项目定位是 Python 包（AGENTS.md §0），与 Python construct API 一致是核心价值。RawCopy 是 Python construct 公开 API，deprecated 会破坏用户迁移路径
2. **EMF 场景独占**（data + value 两键）：deprecated 后无替代，除非引入两个新构造器（RawBytes + RawValue），API 膨胀更糟
3. **issue_289/358 的 .value 引用是真实用户场景**：这些是 Python construct 官方接受的 bug 修复测试，说明 RawCopy.value 有真实用户基础
4. **deprecated 信号混淆**：迁移用户看到 deprecated 会困惑"是不是不能用"，反而增加支持成本

**建议的折中方案**：

在 RawCopy 文档（docstring + construct-rs 文档站）中增加 "Recommended Alternatives" 章节：

```python
class RawCopy(Subconstruct):
    """
    ...（原 docstring 保留）...

    Recommended Alternatives (construct-rs enhancement):
    ----------------------------------------------------
    For checksum scenarios, prefer Tell + Checksum(offset-based):
        # instead of (Python construct 原版写法):
        #   Struct("fields" / RawCopy(...), "checksum" / Checksum(..., this.fields.data))
        # use (construct-rs dataclass 写法, 字段名直接引用):
        @dataclass
        class P(StructMixin):
            start: int = rfield(Tell())
            fields: ... = field(...)    # no RawCopy wrapper
            end: int = rfield(Tell())
            checksum: bytes = rfield(Checksum(..., start, end))

    For hex display scenarios, prefer Hex(inner) directly:
        # instead of:
        #   Hex(RawCopy(Int32ub))
        # use (if you want int hex display):
        Hex(Int32ub)
        # or (if you want raw bytes hex display):
        Hex(Bytes(N))

    RawCopy remains the recommended choice when you genuinely need both
    raw bytes AND parsed value (e.g., EMF generic_record pattern).
    """
```

### 3.3 引导方案的收益

- **不破坏 parity**：RawCopy 行为完全不变
- **降低 RawCopy 使用频率**：用户在 Checksum/Hex 场景主动选择更高效的替代方案
- **提升整体性能**：Checksum 场景省去 ~150ns dict 构造；Hex 场景省去 ~210ns RawCopy 包装
- **保留 EMF 等真实场景**：data+value 两键需求仍可用

---

## 4. 是否引入"内置原始字节捕获"机制？

### 4.1 三个候选层面评估

| 层面 | 设计 | 可行性 | §0 合规 | 推荐 |
|------|------|-------|---------|------|
| **A. ParseStream 新增 slice(start, end)** | `stream.slice(s, e) -> &'a [u8]` 零拷贝 | ✅ 高 | ✅ 纯 Rust 内部 | ⚪ 可选（收益有限） |
| **B. Context 自动记录字段范围** | StructNode 自动 tell+record | ⚠️ 中 | ❌ 违反 §0 #2 | ❌ 驳回 |
| **C. 编译期标记需要捕获的字段** | 类似 has_expressions 标志 | ✅ 高 | ✅ | ⚪ 可选（复杂度高） |

### 4.2 方案 A 评估（ParseStream::slice）

**设计**：
```rust
impl ParseStream<'_> {
    /// 返回底层缓冲的 [start..end) 切片（零拷贝，Rust 内部使用）。
    /// 越界返回 None。
    pub fn slice(&self, start: usize, end: usize) -> Option<&[u8]> {
        self.data.get(start..end)
    }
}
```

**收益有限的原因**：
- **Rust 内部零拷贝**：如果 hashfunc 是 Rust 实现（如内置 sha256 crate），可直接 slice 后做哈希，零拷贝
- **跨 FFI 仍拷贝**：如果 hashfunc 是 Python callable（hashlib.sha512 等，最常见的 Checksum 用法），仍需 PyBytes::new_bound 拷贝
- **实际 Checksum 场景几乎都是 Python hashfunc**——因为 Python 用户用 hashlib/zlib/crc32 等标准库

**ARCH 评估**：方案 A 是"锦上添花"，收益取决于 hashfunc 是否 Rust 化。**不在本质疑范围内强制推进**，可作为未来优化（如 construct-rs 内置常见 hashfunc 的 Rust 实现）的前置基础设施。

### 4.3 方案 C 评估（编译期标记）

**设计**：StructDescriptor 增加 `captures_raw: bool` 标志，编译期识别需要字节捕获的字段（RawCopy / Checksum(StreamRange) / Hex(raw=True) 等），仅在这些字段的 Struct 上启用 range 记录。

**问题**：
- 复杂度高（需改 compile.rs + StructNode + Context）
- 收益场景狭窄（仅 StreamRange Checksum 受益，且仍需 PyBytes 拷贝）
- 与方案 D（Checksum 直接求值 start/end 表达式）相比，方案 D 更简单（用户显式传 Tell 字段，不需要编译期分析）

**ARCH 评估**：方案 C **过度工程化**。方案 D（Checksum 接受 offset 参数）已经覆盖需求，不需要编译期标记机制。

### 4.4 综合结论：不引入统一机制

**ARCH 不推荐引入"统一的内置原始字节捕获机制"**。理由：

1. **场景碎片化**：
   - Checksum 用 offset 范围（§1 方案 D）
   - Hex 用 inner.parse 结果类型分支（§2）
   - RawCopy 用 5 键 dict（parity）
   - 统一机制反而僵化，无法适应不同消费者需求

2. **§0 #2 风险**：在 Context 或 Struct 层引入"自动捕获"机制（方案 B/C），本质是引入了元数据中间层。即使技术上可行，也与 construct-rs "无中间表示层"的核心原则冲突

3. **零拷贝只在 Rust 内有效**：跨 FFI（hashfunc Python callable）仍需 PyBytes 拷贝。统一机制的零拷贝收益被 FFI 边界吃掉

4. **现有组合已足够**：`ParseStream::seek + read` 组合（RawCopy 就是这么做的）+ `Tell` 字段标记 offset，已经能覆盖所有场景。新增机制是重复造轮子

**而是分场景设计**：
- Checksum：扩展接受 offset-based 参数（§1 方案 D）
- Hex/HexDump：实现为 Rust Node，保持 Python 原版行为（§2）
- RawCopy：保留现有实现，文档引导替代方案（§3）

---

## 5. PM 决策点

### 5.1 决策点 1：是否接受"RawCopy 拷贝是 CPython 硬约束，非实现缺陷"？

**ARCH 论据**（§0.3 + §1.4）：
- Python 原版 RawCopy 本身就是拷贝（stream_read 返回新 bytes）
- construct-rs 与原版完全等价
- **"RawCopy 额外承担一份拷贝"是直觉错觉**——RawCopy+Checksum 组合与 StreamRange Checksum 的拷贝次数相同（都是 1 次）

**ARCH 建议**：**接受**。这是参考实现事实，无可争论。

### 5.2 决策点 2：是否启动 Checksum 扩展设计（StreamRange 模式）？

**ARCH 论据**（§1.2-§1.4）：
- 技术可行，§0 合规
- 性能收益：省去 RawCopy 的 5 键 dict 构造（~150ns）+ value 解析
- parity 无破坏（默认仍是 bytesfunc 模式）
- **Checksum 尚未实现**（inventory.csv:91 `not_implemented`），可一并设计，无返工成本

**ARCH 建议**：**启动**。在 Checksum 子任务（Phase 7+ 或 Phase 8）设计时，明确支持两种 bytes_source 模式（ContextBytes + StreamRange），文档引导用户优先用 StreamRange。

### 5.3 决策点 3：Hex/HexDump 实现层（Rust Node vs Python 层 Adapter）？

**ARCH 论据**（§2.2）：
- Hex/HexDump 是 construct 核心库内置（非用户自定义 Adapter）
- 实现为 Rust Node 避免 AdapterCallbackNode 的 Python 回调开销
- parse 逻辑简单（类型分支 + Python 显示类构造）

**ARCH 建议**：**实现为 Rust Node**。在 Hex/HexDump 子任务（Phase 7+）设计时，明确不走 AdapterCallbackNode 路径，作为内置 Adapter Node（与 Subconstruct/RawCopy/Peek 同档）。

### 5.4 决策点 4：RawCopy 是否 deprecated？

**ARCH 论据**（§3）：不建议 deprecated，但文档引导。

**ARCH 建议**：**保留 RawCopy**，在 docstring 加 "Recommended Alternatives" 章节。引导 Checksum 场景用 Tell+Checksum(StreamRange)，Hex 场景用 Hex(inner)。

### 5.5 决策点 5：是否引入"内置原始字节捕获"统一机制？

**ARCH 论据**（§4）：不引入。场景碎片化 + §0 #2 风险 + 零拷贝收益被 FFI 吃掉。

**ARCH 建议**：**不引入**。分场景设计（Checksum StreamRange + Hex Rust Node）已覆盖需求。

### 5.6 决策点 6：是否在 ParseStream 增加 slice(start, end) 辅助方法？

**ARCH 论据**（§4.2）：锦上添花，收益取决于 hashfunc Rust 化（未来工作）。

**ARCH 建议**：**可选**。如 Phase 8+ 计划实现 Rust 内置 hashfunc（sha256/crc32 等），则 slice 方法作为前置基础设施一并引入；否则不引入（避免无用 API）。

---

## 6. 总结：对用户质疑的逐条回应

| 用户论点 | ARCH 回应 |
|---------|----------|
| "Checksum、Hex 真的就必须要 RawCopy 吗？" | **Checksum 不必须**（§1 方案 D 可替代）；**Hex 本来就不必须**（§2.1 Python 原版已支持 Hex(inner) 直接用法） |
| "为什么要额外承担一份复制开销？" | **拷贝本身不是 RawCopy 的额外开销**——RawCopy+Checksum 与 StreamRange Checksum 拷贝次数相同（1 次，CPython 硬约束）。RawCopy 真正的额外开销是 5 键 dict 包装（~150ns）+ value 解析 |
| "如果 Checksum/Hex 自己具备'从流中直接捕获字节范围'的能力，就根本不需要 RawCopy" | **部分成立**。Checksum 应当具备 StreamRange 能力（§1 方案 D）；Hex 不需要新能力（§2.3 现有 Hex(inner) 已足够）。RawCopy 仍保留用于 EMF 等 data+value 两键场景 |
| "construct-rs 是否应该引入'内置原始字节捕获'机制（如 ParseStream 层 / Context 层 / 编译期标记）？" | **不引入统一机制**（§4）。分场景设计更灵活、§0 合规、避免过度工程化 |

**ARCH 整体立场（原 §1-§6）**：质疑**部分成立**。用户的直觉"RawCopy 不应是必须的"是正确的——但根因不是"拷贝开销"（这是 CPython 硬约束），而是"Python 作者把字节捕获职责放在 RawCopy 而非消费者"的设计取舍。construct-rs 可以在保持 parity 的前提下，通过扩展 Checksum（StreamRange 模式）和正确实现 Hex（Rust Node）来减少 RawCopy 的使用频率，同时保留 RawCopy 用于真正需要 data+value 的场景。

> **【2026-07-30 §7 修正】**：上述"拷贝是 CPython 硬约束"的表述**不准确**。该约束仅针对"Python callable hashfunc 跨 FFI"路径；若 hashfunc 在 Rust 内实现（sha2/md-5/crc32fast crate），全程可零拷贝。用户追问完全成立，详见 §7。

---

## 7. 补充评估：Rust 内置 hashfunc 路径（2026-07-30）

> 触发：用户追问"为什么要在 Python 层面拷贝？标准哈希函数可以 Rust 实现，整个流程不需要拷贝到 Python 层面。"
>
> ARCH 承认原 §1.4 / §4.2 / §4.4 / §5.6 的论证存在盲点：把"hashfunc 是 Python callable → 跨 FFI 必须拷贝"当成硬约束，未评估 Rust 内置 hashfunc 这条零拷贝路径。

### 7.1 关键事实修正：FFI 入口本身就是零拷贝

原 §0.3 "CPython bytes 不可变硬约束" 与 §1.4 "拷贝不可避免" 的表述**不准确**。

**证据**（`construct-rs/src/schema.rs:155-156`）：
```rust
let bytes = data.as_bytes();              // PyBytes::as_bytes() 借用 CPython 内部 buffer
let mut stream = ParseStream::new(bytes); // ParseStream<'a> 持有 &'a [u8]
```

**机制澄清**：
- CPython `bytes` 对象内部即连续 `ob_val: [u8]` 数组
- `PyBytes::as_bytes()` 返回对该数组的借用引用 `&[u8]`，**不发生内存拷贝**
- `ParseStream<'a>` 持有此借用，生命周期绑定到 Python `bytes` 对象（pyo3 GIL 保证有效）
- `ParseStream::read(n)` 返回 `&'a [u8]`（`stream.rs:129, 157`），仍是借用，**全程零拷贝**

**CPython bytes 不可变约束的真实边界**：
- ✅ 约束针对"**创建新 PyBytes 对象**"（必须分配新内存 + memcpy）
- ❌ 约束**不针对**"**借用现有 PyBytes 的内部 buffer**"（pyo3 `as_bytes()` 直接借用）
- 原 §0.3 的 RawCopy 拷贝来自 `stream_read` 构造**新** PyBytes（Python 原版语义，不可避），不是来自 FFI 入口本身

### 7.2 Rust 内置 hashfunc 方案

**用户面 API（双轨）**：
```python
from construct import Checksum, Bytes, Tell, StructMixin, HashAlgo
from dataclasses import dataclass

# 路径 1：Rust 内置（零拷贝，高性能）—— construct-rs 扩展（dataclass 写法，字段名直接引用）
@dataclass
class D1(StructMixin):
    start: int = rfield(Tell())
    fields: ... = field(...)
    end: int = rfield(Tell())
    checksum: bytes = rfield(Checksum(Bytes(32), HashAlgo.SHA256, start, end))

# 路径 2：Python callable（兼容模式）—— 【Python construct 原版写法，对齐 core.py:5584】
import hashlib
d2 = Struct(
    "fields"   / RawCopy(Struct(...)),
    "checksum" / Checksum(Bytes(32), lambda d: hashlib.sha256(d).digest(), this.fields.data),
)
```

**HashAlgo enum（construct-rs 扩展）**：
```python
class HashAlgo(Enum):
    MD5     = auto()
    SHA1    = auto()
    SHA256  = auto()
    SHA512  = auto()
    CRC32   = auto()    # zlib.crc32 等价
    ADLER32 = auto()    # zlib.adler32 等价
```

**Rust 端 ChecksumNode 设计（修正原 §1.2）**：
```rust
enum HashFunc {
    /// Rust 内置哈希（零拷贝，操作 &[u8]）
    BuiltIn(BuiltinHash),
    /// Python callable（兼容模式，跨 FFI）
    PythonCallable(PyObject),
}

enum BuiltinHash {
    Md5, Sha1, Sha256, Sha512,   // 依赖 sha2 + md-5 crate
    Crc32, Adler32,               // 依赖 crc32fast + adler crate
}

enum BytesSource {
    /// 原版模式：从 context 求值 bytesfunc（需 RawCopy 把 bytes 塞进 context）
    ContextBytes(ExprProgram),
    /// 扩展模式：从 stream 范围 [start, end) 直接切片
    StreamRange { start: ExprProgram, end: ExprProgram },
}

struct ChecksumNode {
    checksumfield: Box<Node>,
    hashfunc: HashFunc,
    bytes_source: BytesSource,
}
```

**parse 流程（Rust 内置 + StreamRange，零拷贝路径）**：
1. `hash1 = checksumfield.parse(stream, ctx, path)` → PyBytes（如 32 字节，这是必要返回值，非额外开销）
2. `start = eval(start_expr, ctx)` / `end = eval(end_expr, ctx)` → usize
3. `data_slice: &[u8] = stream.data().get(start..end)` → **借用切片，零拷贝**（直接对底层 PyBytes buffer 切片）
4. `digest = match hashfunc { BuiltIn(Sha256) => Sha256::digest(data_slice), ... }` → **Rust 内计算，操作 &[u8]，零拷贝**
5. `if hash1.as_bytes() != digest.as_slice() { raise ChecksumError }` → Rust 内字节比较
6. 返回 hash1（已是 PyBytes）

**全程零拷贝**：FFI 入口零拷贝（§7.1）+ StreamRange 借用切片零拷贝 + Rust 内哈希零拷贝。

### 7.3 修正的拷贝次数对照表（修正原 §1.4）

| 路径 | 被哈希 N 字节的拷贝次数 | 说明 |
|------|----------------------|------|
| RawCopy + Checksum(Python callable) | **1 次** | RawCopy.read 构造 N 字节 PyBytes（Python 原版语义） |
| StreamRange + Checksum(Python callable) | **1 次** | Checksum 内 read N 字节 PyBytes，hashfunc 跨 FFI |
| **StreamRange + Checksum(Rust 内置 hashfunc)** | **0 次** | 全程借用 `&[u8]`，Rust 内计算，无 FFI 跨界 |

**修正结论**：用户质疑成立——**Rust 内置 hashfunc 路径确实是零拷贝**。原 §1.4 "拷贝次数相同（1 次）"仅在 Python callable 路径下成立，未考虑 Rust 内置 hashfunc 路径。

### 7.4 性能预测（L-02 对策：待 Phase 8+ 实测验证）

**Rust 内置 sha256 vs Python hashlib.sha256**（对 1KB 数据，估算）：

| 路径 | 哈希计算 | PyBytes 构造 | FFI 跨界 | 总计估算 |
|------|---------|-------------|---------|---------|
| RawCopy + Python callable | ~3µs（OpenSSL C） | ~80ns（N=1024B）+ ~80ns（digest 32B）| 1 次回调（~200ns）| ~3.4µs |
| StreamRange + Python callable | ~3µs | ~80ns（仅 digest 32B）| 1 次回调 | ~3.3µs |
| **StreamRange + Rust 内置** | ~2-3µs（sha2 crate SIMD） | **0** | **0** | ~2-3µs |

**可证伪预测**：
- vs RawCopy + Python callable：节省 ~30-40%（消除 1 次 N 字节 PyBytes 拷贝 + 1 次 FFI 回调 + RawCopy 5 键 dict 构造）
- vs StreamRange + Python callable：节省 ~10-30%（消除 1 次 FFI 回调 + PyBytes 构造）
- **Rust 内置路径 parse 提速 ≥1.3x vs Python callable 路径**（Phase 8+ Checksum bench 实测验证）

注：哈希算法本身（OpenSSL vs sha2 crate）差距不大，主要收益来自消除 FFI 跨界与 PyBytes 构造。

### 7.5 parity 影响：纯增量扩展，不破坏

- 接收 `HashAlgo.SHA256` → Rust 内置路径（扩展）
- 接收 callable → Python callable 路径（原版 `core.py:5584` 行为）
- 现有用户代码无需改写
- 新代码可选 HashAlgo 提升性能

**与 §0 合规性**：
- §0 #1（一次 FFI）：Rust 内置路径哈希完全在 Rust 内完成，不跨 FFI
- §0 #2（无中间表示层）：无 Rust→Python 中间数据结构
- §0 #4（pyo3 是核心依赖）：sha2/crc32 是 Rust 内部计算库，不影响 pyo3 核心地位（类比 `half` crate，Cargo.toml 已有先例）

### 7.6 新增依赖评估

| crate | 用途 | 成熟度 | 许可证 |
|-------|------|-------|--------|
| `sha2` | SHA-224/256/384/512 | RustCrypto 维护，广泛使用 | MIT/Apache-2.0 |
| `md-5` | MD5 | RustCrypto 维护 | MIT/Apache-2.0 |
| `crc32fast` | CRC32 | 成熟，广泛使用 | MIT/Apache-2.0 OR Zlib |
| `adler` | Adler32 | RustCrypto 维护 | MIT/Apache-2.0 |

均为纯 Rust 计算库，不跨 FFI，与 §0 #4（pyo3 核心依赖）不冲突——Cargo.toml 已有 `half = "2.4"` 先例（Phase 6.1 Float16）。

---

## 8. 对原结论的修正清单

| 原章节 | 原结论 | 修正后结论 |
|-------|-------|----------|
| §0.3 | "CPython bytes 不可变 → 拷贝是硬约束" | 约束仅针对"创建新 PyBytes"，不针对"借用现有 buffer"。FFI 入口零拷贝（§7.1） |
| §1.4 | "拷贝次数相同（1 次）" | **仅 Python callable 路径成立**。Rust 内置 hashfunc 路径是 **0 次拷贝**（§7.3） |
| §4.2 | "slice 收益有限，取决于 hashfunc Rust 化" | **强化为推荐**：Rust 内置 hashfunc 是明确收益路径，slice 是前置基础设施（§7.2） |
| §4.4 | "零拷贝收益被 FFI 吃掉" | **修正**：此结论针对 Python callable；Rust 内置 hashfunc 路径无 FFI 跨界，零拷贝收益完整保留 |
| §5.6 | "slice 可选，如 Phase 8+ 计划则引入" | **升级为推荐**：与 Rust 内置 hashfunc 同步落地 |
| §6 总结表 | "拷贝本身不是 RawCopy 的额外开销" | 修正：RawCopy 的额外开销 = 1 次 N 字节拷贝 + 5 键 dict；Rust 内置 hashfunc 路径可消除前者 |

---

## 9. RawCopy 最终建议（不变，强化引导）

**仍然不建议 deprecated**（理由同 §3.2，不重复：parity / EMF 真实场景 / 迁移成本）。

**强化文档引导**：RawCopy 在 Checksum 场景的推荐替代品从原 §3.2 的"StreamRange + Python callable"升级为"**StreamRange + Rust 内置 HashAlgo**"——后者是真正的零拷贝最优解，前者仅消除 5 键 dict 包装。

---

## 10. PM 决策点（补充）

### 10.1 决策点 7（新增）：是否启动 Rust 内置 hashfunc 工作？

**ARCH 论据**（§7）：
- 用户追问成立：Rust 内置 hashfunc + StreamRange = 真正零拷贝（§7.1-§7.3）
- 性能预期：vs Python callable 节省 30-40%（待实测，§7.4）
- parity：纯增量扩展，不破坏现有 API（§7.5）
- 依赖：新增 sha2 / md-5 / crc32fast / adler crate（均成熟、纯 Rust、广泛使用，§7.6）

**ARCH 建议**：**启动**。在 Checksum 子任务设计时一并规划 `HashAlgo` enum + Rust 内置哈希路径。新增 crate 依赖需 PM 评估（§0 #4 仅约束 pyo3 核心地位，不约束 Rust 内部计算库；`half` crate 已有先例）。

### 10.2 决策点 8（新增/升级）：是否引入 ParseStream::slice(start, end)？

**ARCH 论据**（§7.2 + §8）：从原 §5.6 的"可选"**升级为推荐**。

Rust 内置 hashfunc 路径需要切片访问 `stream.data().get(start..end)`。暴露专用 `slice()` 方法比让 ChecksumNode 自己调 `data()` 更清晰（封装边界校验、文档意图）。

**ARCH 建议**：**引入**。与 Checksum 子任务同步落地。

### 10.3 修正决策点 5（原 §5.5）

原 §5.5 "不引入统一机制"的结论**仍然成立**——不引入 Context / 编译期层面的统一捕获机制（§0 #2 合规）。

但分场景设计中，Checksum 场景的最优解从"StreamRange + Python callable"**升级为"StreamRange + Rust 内置 HashAlgo"**（零拷贝最优解）。

---

## 附录 A：参考文件索引

| 文件 | 用途 |
|------|------|
| `construct/construct/core.py:5532-5600` | Checksum class（bytesfunc 模式证据） |
| `construct/construct/core.py:3523-3635` | Hex / HexDump class（_decode 三分支证据） |
| `construct/construct/core.py:4761-4814` | RawCopy class（5 键 dict + seek+read 证据） |
| `construct/construct/lib/hex.py:1-43` | HexDisplayedInteger/Bytes/Dict（Hex 包装逻辑证据） |
| `construct-rs/src/stream.rs:85-485` | ParseStream（已有 seek/tell/read 能力，§7.1 零拷贝借用证据） |
| `construct-rs/src/schema.rs:149-165` | parse FFI 入口（PyBytes::as_bytes 借用，FFI 入口零拷贝证据 §7.1） |
| `construct-rs/src/context.rs:46-97` | Context（不支持旁路记录 raw bytes 范围） |
| `construct-rs/src/nodes/raw_copy.rs:76-112` | RawCopyNode（当前实现，严格 parity） |
| `construct-rs/Cargo.toml` | 当前依赖（pyo3/thiserror/enum_dispatch/half，无 sha2/crc，§7.6 新增依赖评估） |
| `docs/design/queries/调研-RawCopy使用场景.md` | 已有调研（5 场景分类 + 选项 2 RawBytes 推荐） |
| `docs/design/queries/设计质疑-三性能优化评估.md §2` | 已归档质疑（CPython bytes 不可变硬约束） |
| `docs/design/模块设计/模块设计-Adapter核心.md §0.2` | Adapter 双层分离（PM 决策 2 强化） |
| `docs/decisions/ADR-007.md` | Context Vec 化（与本质疑方案 C 冲突） |
| `docs/decisions/ADR-008.md` | parse 借用实例 dict（RawCopy PyDict 合规性基础） |
| `docs/decisions/ADR-022.md` | 用户面 Adapter Python 层化（Hex 实现层选择依据） |

---

## 11. 笔误确认：本文档中的 `this.xxx` 示例（2026-07-30 补充）

> **触发**：用户质疑本文档 §1.2 / §3.2 / §7.2 的 API 示例使用了 `this.start` /
> `this.end` / `this.fields.data` 语法，与 Phase 2"废弃 `this`"决策冲突。

**ARCH 判定**：**全部是文档笔误**——撰写示例时随手用了 Python construct 原版语法，
未转换为 construct-rs 实际语法。**实现层面零违反**。

**完整确认**（含 construct-rs 正确语法示例、全量源码 grep 证据、受影响文件清单、
修复建议）见独立文档：

**[`确认-表达式系统this语法.md`](./确认-表达式系统this语法.md)**

**简表**：

| 检查项 | 结果 |
|--------|------|
| Rust `src/` 中 `"this"` 字符串字面量 | **0 个** |
| Rust `src/` 中 `\bthis\b` 词匹配 | 34 个，**全部是注释/docstring** |
| Python `python/` 中 `\bthis\b` 词匹配 | 19 个，**全部是 docstring/错误信息** |
| `__init__.py` 导出 `this` | **未导出**（`from construct import this` → ImportError） |
| `ExprOp` enum | 无 `This` 变体，`GetInt(usize)` 索引式 |
| 实现层面违反 | **零** |

**正确的 construct-rs 语法**（替换本文档所有 `this.xxx`）：

```python
@dataclass
class Packet(StructMixin):
    start: int = rfield(Tell())
    fields: bytes = field(Bytes(64))
    end: int = rfield(Tell())
    checksum: bytes = rfield(Checksum(Bytes(32), HashAlgo.SHA256, start, end))
    #                                                           ↑↑↑↑↑ ↑↑↑
    #                                          字段名直接引用，不是 this.start / this.end
```

**修复状态**：本文档 §1.1 / §1.2 / §3.2 / §7.2 的 `this.xxx` 笔误**已由 ARCH 修复**
（2026-07-30）——construct-rs 扩展/推荐模式示例改为 dataclass 写法 + 字段名直接引用；
Python construct 原版模式示例保留 `this.xxx` 并显式标注"Python construct 原版写法"。
`_descriptors.py` / `_conditional.py` 的同类 docstring 笔误待 DEV 修复（不在 ARCH 可写范围）。
详见确认文档 §4。
