---
id: RESEARCH-rawcopy
status: active
phase: cross
depends_on: []
last_updated: 2026-07-30
---

# 调研：RawCopy 使用场景与改造可行性

> **触发**：用户认为 Python construct 原版 RawCopy 设计"不聪明"——parse 时把 inner 结果包进额外的 Container（5 键：data/value/offset1/offset2/length），破坏数据结构扁平性。用户想改掉它，需先摸清使用场景。
>
> **调研范围**：Python construct 2.10.70 全量源码（core.py + tests/ + deprecated_gallery/ + docs/）。
>
> **结论先行**（详见 §4）：实际使用场景中**没有任何一处引用 offset1/offset2/length 这 3 个键**。RawCopy 的真实价值只有 `data` 键（Checksum / Hex 场景）和 `value` 键（后续字段引用场景）。offset/length 三键是"顺带提供但无人使用"的死代码。

---

## 1. Python construct 原版 RawCopy 的设计意图

### 1.1 类定义与 docstring（core.py:4761-4786）

```python
class RawCopy(Subconstruct):
    r"""
    Used to obtain byte representation of a field (aside of object value).

    Returns a dict containing both parsed subcon value, the raw bytes that were
    consumed by subcon, starting and ending offset in the stream, and amount in
    bytes. Builds either from raw bytes representation or a value used by subcon.
    Size is same as subcon.

    Object is a dictionary with either "data" or "value" keys, or both.

    When building, if both the "value" and "data" keys are present, then the
    "data" key is used and the "value" key is ignored. ...
    """
```

**设计目的**（docstring 原文）：同时获取字段的「解析对象值」和「原始字节表示」，附带 stream 位置元信息。

### 1.2 5 键 Container 各自的设计价值

| 键 | 来源 | 设计意图 | 实际使用频率（见 §2） |
|----|------|---------|---------------------|
| `data` | `stream_seek(offset1) + stream_read(length)` 重读 | 原始字节（用于哈希、显示、保留） | **高**（Checksum + Hex 场景全部依赖） |
| `value` | `inner._parsereport(...)` | inner 解析结果（结构化对象） | **中**（issue_289 / issue_358 用 `.value` 访问 inner） |
| `offset1` | `stream_tell()` 入口 | 字段起始 stream 位置 | **零**（无任何用户代码引用） |
| `offset2` | `stream_tell()` 出口 | 字段结束 stream 位置 | **零**（无任何用户代码引用） |
| `length` | `offset2 - offset1` | 字段字节长度 | **零**（无任何用户代码引用） |

### 1.3 为什么返回 5 键 Container 而非直接返回 inner 值

Python construct 作者的设计取舍（从 core.py:4665 Tell docstring 推断）：

> Tell is useful for adjusting relative offsets to absolute positions, or to
> measure sizes of Constructs. ... However, its recommended to use RawCopy
> instead of manually extracting two positions and computing difference.

**推断**：作者把 RawCopy 定位为「Tell + 字节捕获 + inner 解析」三合一便利构造器，offset/length 是 Tell 的替代品。但**实际使用中无人使用 offset/length**——需要 stream 位置的用户直接用 Tell（更直观），需要字节长度的用户用 `.data` 后 `len()`。

### 1.4 实现机制（core.py:4788-4814）

**parse**：
```python
def _parse(self, stream, context, path):
    offset1 = stream_tell(stream, path)
    obj = self.subcon._parsereport(stream, context, path)
    offset2 = stream_tell(stream, path)
    stream_seek(stream, offset1, 0, path)          # ← seek 回入口
    data = stream_read(stream, offset2-offset1, path)  # ← 重读（拷贝）
    return Container(data=data, value=obj, offset1=offset1, offset2=offset2, length=(offset2-offset1))
```

**build**：
```python
def _build(self, obj, stream, context, path):
    if 'data' in obj:           # data 优先
        data = obj['data']
        stream_write(stream, data, len(data), path)
        return Container(obj, data=data, offset1=..., offset2=..., length=...)
    if 'value' in obj:          # value 次选
        value = obj['value']
        buildret = self.subcon._build(value, stream, context, path)
        ...
        return Container(obj, data=data, value=value, ...)
    raise RawCopyError('both data and value keys are missing', path=path)
```

**关键事实**：Python 原版**本身就是拷贝**（`stream_read` 返回新 bytes）。construct-rs 的实现（`raw_copy.rs:100-111`）与原版**完全等价**——不存在"原版引用、Rust 拷贝"的差异（详见已归档质疑文档 `设计质疑-三性能优化评估.md §2.2`）。

---

## 2. 实际使用场景（从源码 / 测试 / 示例全量提取）

grep `construct/` 全量 49 处 RawCopy 引用，去重后归为 5 个场景（嵌套 RawCopy 是编译鲁棒性测试，无业务语义，单列）。

### 2.1 场景 A：Checksum 配合（最重要，6 处）

**位置**：
- `core.py:5549` Checksum docstring 示例 1
- `core.py:5562` Checksum docstring 示例 2（Pointer 回写）
- `docs/tunneling.rst:203` / `docs/tunneling.rst:218`（同上两个示例）
- `tests/test_core.py:1291` `test_checksum`
- `tests/test_core.py:1762` `test_from_issue_71`
- `tests/test_core.py:1846` `test_from_issue_324`（Prefixed + RawCopy 变种）

**典型代码**（`test_core.py:1291`）：
```python
d = Struct(
    "fields" / RawCopy(Struct("a" / Byte, "b" / Byte)),
    "checksum" / Checksum(Bytes(64), lambda data: hashlib.sha512(data).digest(), this.fields.data),
)
```

**配合机制**：
- RawCopy 包裹需要校验的字段，parse 后 `this.fields.data` 拿到原始字节
- Checksum 的 `bytesfunc` lambda 接收原始字节 → `hashfunc`（如 sha512）→ 与下一个字段读到的 checksum 比对

**这个场景真正需要的是**：**`.data` 键**（原始字节）。
- **不需要** value（Checksum 不关心解析值）
- **不需要** offset1/offset2/length（Checksum 不关心位置）

### 2.2 场景 B：Hex / HexDump 配合（显示用，4 处）

**位置**：
- `core.py:3547` Hex docstring 示例 `Hex(RawCopy(Int32ub))`
- `core.py:3602` HexDump docstring 示例 `HexDump(RawCopy(Int32ub))`
- `tests/test_core.py:742` `test_hex`
- `tests/test_core.py:762` `test_hexdump`
- `tests/test_compiler.py:99/101` 编译覆盖

**配合机制**（core.py:3558-3565 Hex._decode）：
```python
def _decode(self, obj, context, path):
    if isinstance(obj, int):     return HexDisplayedInteger.new(...)
    if isinstance(obj, bytes):   return HexDisplayedBytes(obj)
    if isinstance(obj, dict):    return HexDisplayedDict(obj)   # ← RawCopy 走这里
    return obj
```

Hex 的 `_decode` 接收 RawCopy 的 5 键 dict，`HexDisplayedDict` 在 `str()` 时从 `data` 键取 bytes 做 hexlify 显示。

**这个场景真正需要的是**：**`.data` 键**（原始字节，用于 hex 显示）。
- **不需要** value/offset1/offset2/length（仅 data 用于显示）

### 2.3 场景 C：实际协议（EMF，1 处）

**位置**：`deprecated_gallery/emf.py:109`
```python
generic_record = Struct(
    "record_type" / record_type,
    "record_size" / Int32ul,
    "params" / RawCopy(Array((this.record_size - 8) // 4, Int32ul)),
)
```

**分析**：EMF generic_record 的 params 字段用 RawCopy 包裹变长 Int32ul 数组。
- **意图**：同时保留原始字节（`.data`）和解析后的数组（`.value`）。
- **实际引用**：该文件后续代码未显式引用 `.data` 或 `.value`（deprecated gallery 是展示用途，无下游消费）。推测作者保留 RawCopy 是为「将来可能做 checksum / 调试 dump」预留。

**这个场景真正需要的是**：**`.data` + `.value`**（两者都可能用到，但 offset/length 仍未被引用）。

### 2.4 场景 D：基础用法 + issue 修复（测试，4 处）

**位置**：
- `tests/test_core.py:1054` `test_rawcopy`（基础功能）
- `tests/test_core.py:1062` `test_rawcopy_issue_289`
- `tests/test_core.py:1074` `test_rawcopy_issue_358`
- `tests/test_core.py:1079` `test_rawcopy_issue_888`

**典型代码**（`test_rawcopy_issue_289`，用 `.value` 访问 inner）：
```python
d = Struct(
    "raw" / RawCopy(Struct("x"/Byte, "len"/Byte)),
    "array" / Byte[this.raw.value.len],   # ← 用 .value.len 引用 inner 字段
)
```

**典型代码**（`test_rawcopy_issue_358`，用 `.value`）：
```python
d = Struct("a"/RawCopy(Byte), "check"/Check(this.a.value == 255))
```

**这个场景真正需要的是**：**`.value` 键**（访问 inner 解析结果，供后续字段引用）。
- issue_289 / issue_358 用 `.value.<inner_field>`
- **不需要** data / offset / length

### 2.5 场景 E：嵌套 RawCopy（编译鲁棒性测试，1 处）

**位置**：`tests/test_compiler.py:134`
```python
"rawcopy2" / RawCopy(RawCopy(RawCopy(Byte))),
```

**分析**：这是 `test_compiler.py` 的编译覆盖测试（确保所有构造器组合能编译）。**无任何业务场景需要嵌套 RawCopy**——嵌套只会产生 `dict(value=dict(value=dict(value=..., data=..., ...), data=..., ...), ...)` 这种无意义的嵌套 dict。

**这个场景真正需要的是**：**仅测试用途**（验证编译器不崩），与 RawCopy 设计本身无关。

---

## 3. 场景分类汇总 + 改造可行性

### 3.1 场景 × 真实键需求矩阵

| 场景 | 频率 | data | value | offset1 | offset2 | length | 真正需要的形态 |
|------|------|------|-------|---------|---------|--------|--------------|
| **A. Checksum** | 6 处（最重要） | ✅ | ❌ | ❌ | ❌ | ❌ | 仅原始字节 bytes |
| **B. Hex/HexDump** | 4 处 | ✅ | ❌ | ❌ | ❌ | ❌ | 仅原始字节 bytes |
| **C. EMF 协议** | 1 处 | ✅ | ✅ | ❌ | ❌ | ❌ | dict{data, value} |
| **D. issue_289/358** | 2 处 | ❌ | ✅ | ❌ | ❌ | ❌ | 仅 inner 值（透传） |
| **E. 嵌套** | 1 处（测试） | — | — | — | — | — | 无业务需求 |

**核心发现**：
1. **`offset1` / `offset2` / `length` 三键在全量源码中零引用**——是"顺带提供但无人使用"的死代码。
2. **没有任何场景同时需要全部 5 键**。
3. **Checksum 场景**（RawCopy 最主要消费方）只需要 `data`，完全不需要 dict 包装。
4. **value 场景**（issue_289/358）只需要 inner 透传，等同于 `Subconstruct`。

### 3.2 各场景能否用更扁平的方式实现

| 场景 | 扁平替代方案 | parity 影响 |
|------|------------|-----------|
| A. Checksum | `RawBytes(subcon)` 直接返回 bytes（无 dict 包装） | **需 Checksum 配合**：bytesfunc 从 `this.fields.data` 改为 `this.fields`（直接是 bytes）。Checksum 未实现（见 §4.2），可一并设计 |
| B. Hex/HexDump | Hex._decode 已支持 bytes 输入分支（`HexDisplayedBytes`）。`Hex(RawBytes(Int32ub))` 直接走 bytes 分支，无需 dict 分支 | **无 parity 影响**（Hex 已支持 bytes） |
| C. EMF | 保留 RawCopy（需要 data+value 两键） | 无 |
| D. issue_289/358 | 用 `Subconstruct(subcon)`（透传 inner 值，无 dict 包装） | **代码改写**：`this.raw.value.len` → `this.raw.len`（少一层 .value） |
| E. 嵌套 | 无需替代（测试用途） | 无 |

### 3.3 改造对 parity（Python construct 行为一致性）的影响

- **`offset1/offset2/length` 移除**：parity **理论破坏**（用户代码若用这 3 键会 KeyError），但**实际零引用**（全量 grep 无命中）。破坏风险极低。
- **`data` 扁平化为 bytes**：parity **破坏**（Checksum / Hex 的 bytesfunc / `_decode` 分支需配合改）。需 Checksum 同步设计。
- **`value` 扁平化为透传**：parity **破坏**（issue_289/358 模式的 `.value.xxx` 引用需改写为 `.xxx`）。

---

## 4. 改造方向建议（≤3 个选项）

### 选项 1：精简 RawCopy 为 2 键（data + value）—— **不推荐**

**方案**：移除 `offset1/offset2/length`，RawCopy 只返回 `dict(data=..., value=...)`。

**影响的场景**：
- A/B/C/D 全部（dict 形态变化）
- E 无影响

**parity 影响**：破坏（3 键缺失，理论不兼容）

**收益**：
- parse 节省 3 次 `PyDict::set_item`（~90ns），RawCopy parse 5.77x → ~6.5x（**仍不达标 10x**）
- 详见已归档质疑文档 `设计质疑-三性能优化评估.md §2.3`：性能收益不足以达标

**ARCH 评估**：❌ **不推荐**。
- 收益微小（性能仍不达标，理由见归档质疑 §2.4：真正瓶颈是 inner.parse + seek/read 双重开销，不是 dict 键数）
- 破坏 parity 但只解决"死代码"问题，未解决"扁平性"核心诉求（仍是 dict 包装）
- 不解决用户真正不满（5 键 Container 的"不聪明"主要是指 dict 包装本身，不是键数）

### 选项 2：新增扁平替代构造器 + 保留 RawCopy —— **ARCH 推荐**

**方案**：不改 RawCopy（保留 parity），新增两个扁平构造器：

| 新构造器 | 行为 | 替代场景 |
|---------|------|---------|
| `RawBytes(subcon)` | parse 直接返回 bytes（inner 解析后丢弃 value，仅保留 data） | A. Checksum / B. Hex |
| （无需新增） | 用现有 `Subconstruct(subcon)` 透传 inner 值 | D. issue_289/358 |

**影响的场景**：
- A. Checksum：`"fields" / RawBytes(Struct(...))` + bytesfunc 改为 `this.fields`（直接 bytes）
- B. Hex：`Hex(RawBytes(Int32ub))` 直接走 Hex 的 bytes 分支
- C. EMF：保留 `RawCopy`（需要 data+value 两键，无替代）
- D. issue_289/358：改用 `Subconstruct`，`.value.len` → `.len`
- E. 嵌套：保留 RawCopy（测试用途）

**parity 影响**：**无破坏**（RawCopy 保留原行为，新构造器是 construct-rs 扩展）

**收益**：
- Checksum / Hex 场景 parse 省去 5 键 dict 构造 + value 解析保留开销，预计提速 1.5-2x
- 数据结构扁平化（用户拿到 bytes 而非 dict）
- 解决用户核心不满（dict 包装）

**代价**：
- 新增 1 个 Node（`RawBytesNode`，复用 RawCopyNode 的 seek/read 逻辑，但不构造 dict）
- Checksum 设计需同步调整（bytesfunc 接收 bytes 而非 `this.x.data`）
- 用户面 API 扩展（需文档说明 RawBytes 是 construct-rs 增强，Python construct 无此构造器）

**ARCH 评估**：✅ **推荐**。
- 不破坏 parity（RawCopy 保留）
- 解决用户核心诉求（扁平化）
- Checksum 尚未实现（inventory.csv:91 `not_implemented`），可一并设计，无返工成本
- 实现简单（RawBytesNode 复用 RawCopyNode 90% 代码，差异仅在返回值构造）

### 选项 3：RawCopy 参数化（可选键）—— **中等推荐**

**方案**：扩展 RawCopy 接受 `keys` 参数：
```python
RawCopy(subcon, keys=("data", "value"))  # 默认 5 键（parity）
RawCopy(subcon, keys=("data",))          # 只返回 data 键
RawCopy(subcon, keys=("value",))         # 只返回 value 键
```

**影响的场景**：
- A. Checksum：`RawCopy(Struct(...), keys=("data",))` + bytesfunc 仍用 `this.fields.data`
- B. Hex：`Hex(RawCopy(Int32ub, keys=("data",)))`
- C/D/E：保留默认 keys=None（5 键 parity）

**parity 影响**：**无破坏**（默认行为不变，keys 是 construct-rs 扩展参数）

**收益**：
- 用户按需选择，灵活
- 仍保留 dict 形态（半扁平）

**代价**：
- API 复杂度增加（用户需理解 keys 参数）
- **仍是 dict 包装**（只是键数减少），未完全解决用户"扁平化"诉求
- Checksum 的 bytesfunc 仍需写 `this.fields.data`（多一层访问）

**ARCH 评估**：⚪ **中等推荐**。
- 比 选项 1 好（不破坏 parity）
- 比 选项 2 差（仍是 dict，未完全扁平化；API 更复杂）

---

## 5. PM 决策点

### 决策点 1：用户核心诉求是「扁平化」还是「精简键数」？

- **若核心诉求是扁平化**（拿到 bytes 而非 dict）→ **选项 2**（RawBytes）
- **若核心诉求是精简键数**（接受 dict 但去掉死键）→ 选项 1 或 3
- **ARCH 建议**：从用户原话「破坏了数据结构的扁平性」判断，**核心诉求是扁平化**，选项 2 最匹配。

### 决策点 2：是否接受 construct-rs 扩展构造器（破坏与 Python construct 的 API 一致性）？

- 选项 2 / 3 都引入 Python construct 没有的 API（RawBytes / keys 参数）
- 项目定位是「Python 包，用户是 Python 开发者」（AGENTS.md §0），扩展 API 需文档明确
- **ARCH 建议**：可接受。construct-rs 已有多处增强（如 mashumaro 式 dataclass API 本身就是扩展），RawBytes 与现有设计脉络一致。

### 决策点 3：改造时机（立即 / 等 Checksum 设计时一并）

- Checksum 尚未实现（inventory.csv:91 `not_implemented`，Phase 7+）
- **若选选项 2**：建议**等到 Checksum 子任务启动时一并设计**（避免 RawBytes 先落地后 Checksum 设计时返工 bytesfunc 接口）
- **若选选项 1/3**：可立即改造 RawCopy（不依赖 Checksum）

### 决策点 4：是否保留 RawCopy 用于「需要 data+value 两键」的场景（如 EMF）？

- 选项 2 / 3 都保留 RawCopy
- 选项 1 改造 RawCopy 本身（但仍保留 data+value 两键，EMF 场景仍可用）
- **ARCH 建议**：保留 RawCopy（C 场景需要 data+value 两键，无扁平替代）

---

## 6. 附录：调研证据索引

### 6.1 Python construct 源码位置

| 文件 | 行号 | 内容 |
|------|------|------|
| `construct/core.py` | 4761-4814 | RawCopy class 定义 + _parse + _build |
| `construct/core.py` | 134-137 | RawCopyError |
| `construct/core.py` | 4665 | Tell docstring（推荐用 RawCopy 替代手动 Tell） |
| `construct/core.py` | 5532-5600 | Checksum class（docstring 提到「Usually used with RawCopy」） |
| `construct/core.py` | 5549-5572 | Checksum docstring 两个 RawCopy 示例 |
| `construct/core.py` | 3523-3580 | Hex class（_decode 支持 dict 分支） |
| `construct/core.py` | 3583-3620 | HexDump class（_decode 支持 dict 分支） |
| `construct/core.py` | 3547 / 3602 | Hex / HexDump docstring 的 RawCopy 示例 |
| `construct/docs/tunneling.rst` | 1-19 | RawCopy 文档说明 |
| `construct/docs/transition28.rst` | 212 | RawCopy 引入记录 |

### 6.2 使用位置全量（去重后 13 处业务 + 测试）

| # | 文件 | 行号 | 场景分类 |
|---|------|------|---------|
| 1 | `tests/test_core.py` | 742 | B. Hex |
| 2 | `tests/test_core.py` | 762 | B. HexDump |
| 3 | `tests/test_core.py` | 1054 | D. 基础测试 |
| 4 | `tests/test_core.py` | 1062 | D. issue_289（.value） |
| 5 | `tests/test_core.py` | 1074 | D. issue_358（.value） |
| 6 | `tests/test_core.py` | 1079 | D. issue_888 |
| 7 | `tests/test_core.py` | 1291 | A. Checksum |
| 8 | `tests/test_core.py` | 1762 | A. issue_71（Checksum + .data） |
| 9 | `tests/test_core.py` | 1848 | A. issue_324（Prefixed+RawCopy+Checksum） |
| 10 | `tests/test_compiler.py` | 99 | B. Hex 编译覆盖 |
| 11 | `tests/test_compiler.py` | 101 | B. HexDump 编译覆盖 |
| 12 | `tests/test_compiler.py` | 133 | D. RawCopy 编译覆盖 |
| 13 | `tests/test_compiler.py` | 134 | E. 嵌套 RawCopy（编译鲁棒性） |
| 14 | `deprecated_gallery/emf.py` | 109 | C. EMF 协议 |
| 15 | `tests/test_benchmarks.py` | 807-821 | 基准测试（4 个 bench） |

### 6.3 construct-rs 当前状态

- RawCopy **已实现**（Phase 6.3，`construct-rs/src/nodes/raw_copy.rs`），严格 parity（返回 5 键 dict）
- 性能（`docs/perf-scenarios.csv` RC1-RC5）：
  - parse：3.47-6.12x（场景 RC5 大 N 仅 3.47x，不达标 ≥4x；其余 4.98-6.12x 达标 ≥4x 但不达标 10x）
  - build：5.19-9.63x（达标 ≥4x，部分接近 10x）
- 已归档质疑：`docs/design/queries/设计质疑-三性能优化评估.md §2`（结论：接受 RawCopy parse 5.77x，瓶颈是 inner.parse + seek/read 双重开销，非 dict 键数）

### 6.4 offset1/offset2/length 零引用的证据

grep `construct/` 全量：
- `offset1`：仅在 `core.py:4789/4801/4807/4811`（RawCopy._parse/_build 内部）+ `core.py:3550/3605`（Hex/HexDump docstring 展示 RawCopy 输出示例）出现
- `offset2`：同上
- `length`：同上 + `tests/test_core.py:1055/1293/1301/1846`（测试断言 RawCopy 返回的 length 键值正确）+ `test_core.py:1775`（`len_(this.payload.data)` —— 用 `len_(.data)` 而非 `.length`）

**结论**：无任何业务/测试代码**消费** offset1/offset2/length 三键的值进行逻辑判断或后续字段引用。测试中的 length 断言仅是「验证 RawCopy 行为正确」，非「使用 length 做事」。issue_71 用 `len_(this.payload.data)` 而非 `this.payload.length`——证明即使是作者自己，也更倾向用 `.data` + `len_()` 而非 `.length`。
