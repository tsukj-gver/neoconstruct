---
id: TRACE-P0-REV
phase: "8"
task: "8.0 [P0 批次详细设计] REV 设计检视"
status: PASS-WITH-FOLLOWUPS
reviewer: REV
reviewed_doc: docs/design/模块设计/模块设计-Phase8-P0.md（2081 行，6 子任务）
last_updated: 2026-07-31
---

# Phase 8 P0 批次设计检视报告

**检视文档**：`docs/design/模块设计/模块设计-Phase8-P0.md`
**PM 决策依据**：D-1~D-7 + D-P0-1~5 全部已确认
**检视范围**：6 子任务（8.1/8.4/8.5/8.8/8.9/8.10）
**检视基准**：AGENTS.md §0 + ADR-004/006/012/014/022 + experiences.md L-01/L-02/L-05/L-14

---

## 0. 检视结论

**结论：通过（PASS-WITH-FOLLOWUPS）**

驳回目标状态：DESIGNING（如驳回）。本次不驳回——3 个完备性建议补充项不构成架构级缺陷，ARCH 在 DEV 实施前修订即可。设计整体严谨，§0 合规判据清晰、零拷贝路径（L-14 工程化）正确、85 边界条件精确覆盖、ADR 关系正确、实施顺序合理。

---

## 1. 五维度检视结果

### 1.1 延续性：✅ 通过

- 与已有架构一致：Construct trait 签名不变（parse/build/sizeof 三方法）；Node enum 扩展在 `nodes/mod.rs` 内部；compile_schema 签名不变（设计 §11.1 明确）。
- 调用方向正确：所有新 Node 持 `Box<Node>`（Subconstruct 包装模式）或独立（Construct 非包装模式），与 SubconstructNode/PeekNode/RawCopyNode/RebuildNode/PassNode（Phase 6.3 沉淀）模式一致。
- 与已实现模块接口风格统一：
  - DefaultNode.value: ExprProgram（与 RebuildNode.func 同模式，src/nodes/rebuild.rs L62 验证）
  - AlignedNode.pattern: u8（与 PaddingNode.pattern 同模式，src/nodes/padding.rs L66 验证）
  - TerminatedNode 单元结构体（与 PassNode 同模式，src/nodes/pass.rs L44 验证）
  - CheckNode.func: ExprProgram + RO 字段（与 ComputedNode.expr 同模式，src/nodes/computed.rs L43 验证）
- CancelParsing 复用 ADR-012 Result 哨兵模式（与 StopField 同脉络）✅
- AdapterCallbackNode（Phase 6.3 已实现）继续作为"用户面 Adapter 嵌入 Struct"的钩子，P0 批次不修改其语义 ✅

### 1.2 性能：✅ 通过

每个子任务都包含完整的性能假设结构（瓶颈识别 + 可证伪预测 + 失败模式 + FFI 来源清单），L-02/L-05 对策落实：

| 子任务 | 瓶颈量化来源 | FFI/拷贝来源清单 | 失败模式可证伪 | 预测覆盖 |
|-------|-------------|-----------------|---------------|---------|
| 8.1 | pyo3 benchmark ~20-30ns rich_compare + Phase 2 ExprProgram ~10ns | ✅ L313-314 | ✅ L317-319 | ✅ Const ≥8x / Default ≥8x / Check ≥10x |
| 8.4 | PyObject_IsInstance ~10ns + 类型子类构造 ~80-150ns | ✅ L600-603 | ✅ L596 | ✅ Hex ≥6x（含风险标注） |
| 8.5 | sha2 SIMD ~2-3µs + PyBytes ~80ns + RawCopy dict ~150ns | ✅ L1029-1035（4 条全列） | ✅ L1038-1040 | ✅ B1 ≥1.3x / A2 ≥1.05x（含 sweet spot 风险） |
| 8.8 | PaddingNode 同量级 | 隐含（ExprProgram + stream 内） | ✅ L1290（常量 vs 表达式 modulus） | ✅ Aligned ≥8x |
| 8.9 | PassNode 同量级 | ✅ L1546（print 是副作用 FFI） | Probe 不设门禁（调试用） | ✅ Terminated ≥10x |
| 8.10 | 错误路径不设门禁 | PyErr 类型识别 ~5ns | N/A（用户主动触发） | N/A |

**FFI 边界量化分析（项目特定补充）**：
- 8.1/8.8/8.9 严格 1 次 FFI（仅 parse/build 入口，所有 Node 操作 + ExprProgram 求值在 Rust 内）
- 8.4 实际 2 次 FFI（parse 入口 + 调 lib/hex.py 显示类 Python 方法）——见 §3.3 完备性问题 3
- 8.5 B1 严格 1 次 FFI（零拷贝路径，sha2 操作 `&[u8]`）；A2 2 次 FFI（+ Python hashfunc 回调）
- 8.10 CancelParsing 是错误路径，无 FFI 次数假设

**数据结构选择**：BytesSource 双 enum（StreamRange / ContextBytes）+ HashFunc 双 enum（BuiltIn / PythonCallable）——清晰且覆盖 4 路径矩阵，无冗余。

**L-02 教训对照**：所有性能数字有量化来源（pyo3 benchmark / Phase 2 已验证 / sha2 SIMD 基准），无纯理论推算。
**L-05 教训对照**：8.5 §3.7.2 显式列出 4 条 FFI/拷贝来源（checksumfield.parse / expr 求值 / slice 借用 / Rust 内置哈希 + 路径 A 额外 2 条），可证伪预测覆盖所有来源。

### 1.3 整体性：✅ 通过

- 模块边界清晰：9 个新 Node 变体各持独立文件（const_node/default_node/check_node/hex/hex_dump/checksum/aligned/terminated/probe），职责单一。
- 与全局架构图一致：设计 §7 总体 Node enum 扩展汇总、§8 Cargo.toml 新增依赖确认、§9 ConstructError 新增变体汇总——三处全局视图齐全。
- 调用方向正确，无循环依赖：所有新 Node 都是叶子或单向包装 inner（`Box<Node>`），无 A→B→A 回环。
- CancelParsing 路径识别（修改 `From<PyErr> for ConstructError`）影响所有 PyErr→ConstructError 转换路径——但仅加 1 个指针比较分支（~5ns），错误路径不设硬门禁，影响可接受（设计 §6.2.2 方案 A 论证合理）。

### 1.4 可行性：✅ 通过（Rust + PyO3 0.22 技术栈）

逐一验证关键 API 引用：

| 设计引用 | 源码验证 | 状态 |
|---------|---------|------|
| `ParseStream::slice` 新增 | stream.rs 无 slice，有 data()（L335）+ remaining()（L340）+ tell()（L185） | ✅ 新增可行（~10 行） |
| `ParseStream::read/seek/tell/remaining/data` 已有 | stream.rs 全部验证 | ✅ |
| `BuildStream::write/tell/as_bytes` 已有 | stream.rs L562/L592/L677 验证 | ✅ |
| `Node::compute_ro_value` 兜底为 `_ => Err` | nodes/mod.rs L456-464 验证 | ✅ CheckNode 新增分支模式正确 |
| `Node::has_expressions` 各分支 | nodes/mod.rs L344-390 验证 | ✅ 新 Node 集成模式正确 |
| `From<PyErr> for ConstructError` 当前统一转 Generic | error.rs L1004-1011 验证 | ✅ CancelParsing 修改点准确 |
| `EXCEPTIONS: GILOnceCell<ExceptionClasses>` | error.rs L620 验证 | ✅ |
| `ExceptionClasses::is_builtin_class` 指针比较 | error.rs L644-669 验证（当前 16 数组） | ✅ 扩展到 21 模式正确 |
| `select_exception_class` 当前 16 分支 | error.rs L732-774 验证 | ✅ 新增 5 分支模式正确 |
| `expr::eval_expr_int(program, ctx, py) -> i64` | expr.rs L311 验证 | ✅ DefaultNode/AlignedNode/ChecksumNode(StreamRange) 直接复用 |
| `expr::eval_expr_bool/bytes/any` | **当前不存在**（ExprProgram 仅 i64，ADR-006） | ⚠️ 设计已识别（D-P0-1/D-P0-2），Check 可内联 `eval_expr_int(v)? != 0`，Checksum ContextBytes 与 Probe.into 需 DEV 实施时扩展（~30-40 行 Rust） |
| `CompiledSchema::_parse_raw` 入口 | schema.rs L150-165 验证 | ✅ CancelParsing catch 修改点准确（加 5 行 match 分支） |
| `RebuildNode.func: ExprProgram` 先例 | nodes/rebuild.rs L62 验证 | ✅ DefaultNode.value 同模式 |
| `PaddingNode { length, pattern: u8 }` 先例 | nodes/padding.rs L61-66 验证 | ✅ AlignedNode 同模式 |
| `PassNode` 单元结构体先例 | nodes/pass.rs L44 验证 | ✅ TerminatedNode 同模式 |

**无已知 PyO3 0.22 API 障碍**：
- rich_compare / is_instance_of / call_method1 / call1 都是 pyo3 0.22 stable API
- 显示类工厂调用（call_method1("new", ...) / call1((bound,))）模式正确
- 编译期类加载（py.import_bound + getattr + extract<Py<PyType>>）模式与 init_exception_classes 同（error.rs L689-694）

**不依赖未实现模块**：所有依赖（ExprProgram/Stream/ConstructError/Schema）均 Phase 1-7 已实现。8.5 的 `eval_expr_bytes` 与 8.9 的 `eval_expr_any` 是 P0 内部扩展，不算外部模块依赖。

### 1.5 完备性：⚠️ 通过（3 个建议补充项）

**85 边界条件精确覆盖**（设计文档 §1.6/§2.7/§3.8/§4.6/§5.5/§6.5 实测）：
- 8.1: CN-1~8 (8) + DF-1~5 (5) + CK-1~7 (7) = **20**
- 8.4: HX-1~8 (8) + HD-1~4 (4) = **12**
- 8.5: CS-1~14 = **14**
- 8.8: AL-1~16 = **16**
- 8.9: TM-1~7 (7) + PB-1~8 (8) = **15**
- 8.10: CP-1~8 = **8**
- 总计：**85**（精确匹配）

**Python 参考实现行为覆盖**：
- Const/Default/Check/Hex/HexDump/Checksum/Aligned/AlignedStruct/Terminated/Probe/CancelParsing 全部对照 core.py/debug.py 行号（设计附录 A 速查表完整）
- flagbuildnone=True 语义全部对齐（Const/Default/Check/Terminated/Checksum/CancelParsing）
- 编译期校验场景（CN-7/CN-8/DF-4/CK-5/AL-6/AL-7/AL-8/AL-11/AL-12/CS-10/CS-13/CS-14/PB-5）全部覆盖
- 与 ADR-014 硬约束（不接 lambda）的 parity 差异全部文档化（DF-4/CK-5/CS-13/PB-5）

**3 个建议补充项**（详见 §3）：
1. ChecksumNode build 路径 StreamRange 语义未定义（§3.5.2 build 是 unimplemented!）
2. ChecksumNode checksumfield 返回非 PyBytes 类型场景未覆盖（CS-11 仅覆盖变长 sizeof）
3. Hex/HexDump §0 对照表 #1 措辞不准确（"显示类 __call__/__init__ 是 C 级实现"）

---

## 2. 关键发现（PM 决策框架与 L-14 工程化验证）

### 2.1 §0 合规判据严谨性（L-01 对策）：✅ 严谨

**PM 决策框架 §0.2 重申**："FFI crossing（调用用户 Python 代码）≠ C API 操作（直接操作内置类型）"

**REV 验证**：判据本身与 ADR-022 §0 合规论证（4 点理由）一致——§0 #1 原文"一次 Python↔Rust 边界穿越"针对的是**编译期生成的执行树**，禁止"中间表示层"；CPython C API（PyDict_GetItem/PyLong_FromLong/rich_compare）在 Rust 内部调用是"边界内操作"，不是"边界穿越"。

**关键边界场景验证**：
- Enum/Mapping 等持有 Py<PyDict> 运行时 C API 查询 → ✅ 合规（PyDict_GetItem 是 C API，§0.2 判据 2；即使触发 __hash__/__eq__，对 int/str/bytes/frozenset 等内置类型是 C 级实现，不算用户 FFI）
- ConstNode rich_compare 调度到用户自定义类型 __eq__ → ⚠️ 边缘场景，设计 §1.5.3 已识别并标注"对 int/bytes/str 等内置类型 fast-path，自定义类型走 Python __eq__ 是用户引入的边缘场景"——判据前提条件（内置类型）已在失败模式中明确
- Hex/HexDump 调 lib/hex.py 显示类 → ⚠️ 见 §3.3（措辞问题，不影响合规性）

**判据严谨性结论**：判据本身严谨，且**前提条件（内置类型）需在每个 Node 的失败模式中明确**——设计已落实（ConstNode §1.5.3、HexNode 隐含在性能预测中）。

### 2.2 Checksum 零拷贝路径（L-14 教训工程化）：✅ 正确

**L-14 教训**：ARCH 原 §1.4 把"hashfunc 是 Python callable → 跨 FFI 必须拷贝"当硬约束，漏了 Rust 内置 hashfunc 零拷贝路径。

**P0 设计 §3 工程化验证**：
- 路径 B1（StreamRange + Rust sha256）零拷贝链路验证：
  - `ParseStream::slice(s, e)` 借用 `&'a [u8]`（零拷贝，生命周期绑定到 PyBytes buffer）✅
  - `sha2::Sha256::update(data_slice)` 操作 `&[u8]`（零拷贝）✅
  - `h.finalize().to_vec()` 是 hash 输出固定大小拷贝（32/64 字节，sha2 crate API 决定，不可避免但可忽略）✅
  - `hash1_bytes != computed_digest.as_slice()` slice 比较（零拷贝）✅
- 路径矩阵（B1/B2/A1/A2）4 条路径清晰，每条标注拷贝次数与适用场景 ✅
- 5 crate（sha2/sha1/md-5/crc32fast/adler）全部纯 Rust 计算库（不跨 FFI），与 half = "2.4"（Phase 6.1）同性质 ✅
- §0 #4 合规：pyo3 仍是 FFI 唯一桥梁，5 crate 是 Rust 内部计算库 ✅

**L-14 对策落实**：设计文档 §3 开头明确标注"L-14 教训触发"，§3.4 注释"L-14 教训工程化"——交叉验证记录完整。

### 2.3 Hex/HexDump Rust Node vs Adapter（PM 决策 2 双层分离）：✅ 落实

**PM 决策 2 双层分离**（ADR-022 §0.2）：内置 Adapter（Hex/HexDump）= Rust Node；用户面 Adapter（用户继承）= Python 层 + AdapterCallbackNode 钩子。

**P0 设计 §2.2 落实验证**：
- Hex/HexDump 实现为 HexNode / HexDumpNode（Rust Node 变体，加入 Node enum）✅
- 不走 AdapterCallbackNode 路径 ✅
- 与 ADR-022 §0.2（"Hex/HexDump/Enum/FlagsEnum/Mapping 可走 Rust Node"）一致 ✅
- 与 Subconstruct/RawCopy/Peek/Rebuild（Phase 6.3 内置 Adapter Rust Node）同档 ✅

### 2.4 5 crate 新依赖必要性：✅ 必要且合规

每个 crate 对应 Python hashlib/zlib 等价物，无冗余：

| crate | Python 等价 | 用途 | 必要性 |
|-------|-----------|------|--------|
| sha2 | hashlib.sha256/sha512 | SHA-256/512（最常用） | ✅ |
| sha1 | hashlib.sha1 | SHA-1（旧协议兼容） | ✅ |
| md-5 | hashlib.md5 | MD5（旧协议兼容） | ✅ |
| crc32fast | zlib.crc32 | CRC32（常用校验） | ✅ |
| adler | zlib.adler32 | Adler32（zlib 内部） | ✅ |

无可替代方案（自实现哈希会引入正确性风险 + 性能反而下降）。所有 crate 是 Rust 生态主流哈希库（RustCrypto/ebarnburg 系列），维护活跃。

### 2.5 实施顺序依赖（8.10 → 8.1 → 8.9 → 8.8 → 8.4 → 8.5）：✅ 合理

| 顺序 | 子任务 | 理由 | 验证 |
|-----|-------|------|------|
| 1 | 8.10 CancelParsing | 基础设施改动（error.rs + schema.rs 入口） | ✅ 其他子任务的 PyErr 处理依赖此基础设施 |
| 2 | 8.1 Const/Default/Check | 跑通两类模式（Subconstruct 包装 + Construct 非包装） | ✅ 后续 Node 复用模式 |
| 3 | 8.9 Terminated/Probe | 最简单（单元结构体 + 调试输出） | ✅ 无前置依赖 |
| 4 | 8.8 Aligned + AlignedStruct | padding 算法（modulus + pattern） | ✅ 复用 8.1 的 ExprProgram 模式 |
| 5 | 8.4 Hex/HexDump | 依赖 lib/hex.py port + 显示类加载 | ✅ 独立模块，无前置 Node 依赖 |
| 6 | 8.5 Checksum | 依赖 ParseStream::slice + 5 crate + 最复杂 | ✅ 最后做，基础设施齐备 |

---

## 3. 完备性建议补充项（3 项，要求 ARCH 在 DEV 实施前修订）

### 3.1 [§3.5.2] ChecksumNode build 路径 StreamRange 语义未定义

**问题**：设计 §3.5.2 的 ChecksumNode build 方法是 `unimplemented!("DEV 实现时按 parse 对称实现")`（L940）。但 parse 与 build 的 StreamRange 语义**不对称**：
- parse 时：start/end 是前序 Tell 字段值，从 ParseStream 借用切片（只读缓冲）
- build 时：start/end 同样是前序 Tell 字段值（Tell 作为 RO 字段，compute_ro_value 返回 `BuildStream::tell()` 写入位置），但 BuildStream 是写入缓冲——需从 `BuildStream::as_bytes().get(start..end)` 切片（已写入部分）

**Python 参考**（core.py L5594-5597）：build 时 `hash2 = self.hashfunc(self.bytesfunc(context))`——bytesfunc 从 context 取（RawCopy 塞进去的 data 字段），不"读 stream"。construct-rs 路径 B1 用 StreamRange 替代 RawCopy，build 时需明确"从 BuildStream 已写入部分切片"。

**风险**：DEV 实施时若按 parse 对称实现（误以为 BuildStream 也能 slice 借用 `&'a [u8]`），会卡在 BuildStream 无 slice API（as_bytes 返回 `&[u8]` 但生命周期与 BuildStream 实例绑定，不是 `'a`）。

**建议修订方向**：在 §3.5.2 build 注释中补充：
> build 时 StreamRange 语义：start/end 是前序 Tell 字段值（compute_ro_value 写入 context），从 `BuildStream::as_bytes().get(start..end)` 切片（已写入部分）。Checksum 字段必须出现在 start/end 字段之后（时序保证）。

**严重度**：中（不构成架构级缺陷，但 DEV 实施时需 ARCH 确认）。

### 3.2 [§3.8 CS-11] ChecksumNode checksumfield 返回非 PyBytes 类型场景未覆盖

**问题**：CS-11 仅覆盖"checksumfield 是变长（非 Bytes(N)）→ sizeof 返回 Err"。但未覆盖"checksumfield.parse 返回非 PyBytes 类型"场景。

**Python 参考行为**（core.py L5583-5592）：`hash1 = self.checksumfield._parsereport(...)`——hash1 类型由 checksumfield 决定（通常 Bytes(N) 返回 bytes，但也可能是 GreedyBytes/String 等）。`hash1 != hash2` 比较时若类型不匹配，Python 静默返回 False（触发 ChecksumError）。

**construct-rs 实现**（设计 §3.5.2 L874）：`hash1_bytes: &[u8] = hash1_bound.extract()?`——若 hash1 不是 PyBytes（如 CString 返回 str），extract 失败 → ConstructError::Generic（PyErr 转换）。这与 Python 行为不一致（Python 不报错而是触发 ChecksumError）。

**建议修订方向**：在 §3.8 补充边界条件：
> | CS-15 | checksumfield.parse 返回非 PyBytes（如 CString） | ConstructError::Generic（extract 失败）—— 与 Python 行为差异（Python 触发 ChecksumError），文档标注 parity 差异 |

**严重度**：低（边缘场景，用户极少用非 Bytes checksumfield；但应文档化差异）。

### 3.3 [§2.5] Hex/HexDump §0 对照表 #1 措辞不准确

**问题**：设计 §2.5 §0 原则对照表 #1 措辞：
> "显示类 `__call__` / `__init__` 是 CPython 内置子类构造（int/bytes/dict 的 C 级实现）"

**实际**（construct/lib/hex.py L5-42 验证）：
- `HexDisplayedInteger.new` 是 **Python 实现的 staticmethod**（hex.py L10-14），不是 C 级
- `HexDisplayedBytes` / `HexDisplayedDict` 用默认 `__init__`（CPython 内置 bytes/dict 构造），但 `__str__` 是 Python 实现
- Rust 端 `call_method1(cls, "new", (bound, fmt))` 实际跨 FFI 调用 Python 代码（lib/hex.py 的 new 方法体）

**含义**：Hex/HexDump 实际是 **2 次 FFI**（parse 入口 + 调 lib/hex.py 显示类工厂），不是设计 §2.5 暗示的"严格 1 次"。

**这不算违反 §0**——因为 lib/hex.py 是 construct-rs 自己的核心库代码（非用户代码），属于"内置后处理"（与 AdapterCallbackNode 调用户代码不同档，与 ADR-022 §0.2 内置 Adapter Rust Node 的论证不冲突——只是显示类工厂本身是 Python 实现的）。

**建议修订方向**：将 §2.5 §0 对照表 #1 措辞修订为：
> "显示类 `new` / `__init__` 是 construct-rs 核心库 Python 实现（lib/hex.py，非用户代码）。调用是 1 次额外 FFI 给内置库代码（与 AdapterCallbackNode 调用户代码不同档）。性能预测 Hex ≥6x 仍成立（Python 原版路径同样要调这些 Python 类，节省来自 inner.parse 与 isinstance Rust 化）。"

**严重度**：低（不影响合规性，但 §0 对照表措辞应准确）。

---

## 4. 小问题（文档质量，不驳回，DEV 实施时修订）

### 4.1 Node enum 当前变体数：实际 42（非 39）

设计 §0.1/§7/§11 多处表述"当前 39 → 48"。实际 `nodes/mod.rs` 验证当前 Node enum 有 **42 个变体**（FormatField/Bytes/GreedyBytes/Struct/StructRef/Tell/Computed/BitsInteger/Bitwise/Bytewise/Transform/BitPadding/Padding/Array/GreedyRange/PrefixedArray/RepeatUntil/Index/StopIf/Element/Subconstruct/Peek/RawCopy/Rebuild/Pass/AdapterCallback/VarInt/ZigZag/BytesInteger/CString/GreedyString/PaddedString/PascalString/NullTerminated/NullStripped/IfThenElse/Switch/Select/FocusedSeq/Seek/Pointer/Prefixed）。

P0 后应为 **42 → 51**（非 39 → 48）。建议 ARCH 全文检索修订。

### 4.2 DefaultNode obj 处理代码与注释矛盾（§1.3 L246-248）

```rust
} else {
    // 借用 obj（不克隆，转交 inner）
    obj.clone().into_py(py)  // 注：obj 是 &Bound，需 unbound 引用
};
```

注释说"不克隆"，代码 `obj.clone()` 实际克隆（incref）。建议修订为实现：直接传 obj 给 inner.build（inner.build 接收 `&Bound<PyAny>`，无需 clone）。

### 4.3 AlignedNode sizeof 代码示例与 D-P0-3 推荐方案矛盾（§4.3.2 L1244）

```rust
fn sizeof(&self, ctx) -> Result<usize, ConstructError> {
    let modulus = crate::expr::eval_expr_int(&self.modulus, ctx, Python::with_gil(|py| py))?;
```

`Python::with_gil(|py| py)` 在已持 GIL 的上下文中是反模式（嵌套获取）。设计 D-P0-3 已识别此问题并推荐"对齐 Python 行为——modulus 表达式 + sizeof 触发 SizeofError"。建议删除 L1244 代码示例或修订为 D-P0-3 推荐方案（检测 modulus 是否编译期常量；非常量直接返回 Err）。

### 4.4 ProbeNode build 时 lookahead peek 语义未定义（§5.2.2）

Python Probe `_build` 也调 printout(stream, ...)，lookahead 时执行 `stream.read(lookahead) + seek(fallback)`。construct-rs BuildStream 无 read API（只有 write）。设计 ProbeNode build 路径的 lookahead 语义未定义。

**严重度**：极低（用户极少在 build 时用 Probe lookahead；可归为"DEV 实施时确认：build 时 lookahead 输出 'Stream peek: build mode, skipped'"或类似降级行为）。

---

## 5. 跨阶段决策（ADR）对照

| ADR | 设计遵循情况 |
|-----|-------------|
| ADR-001 废弃 this | ✅ 所有示例用字段名直接引用（Default.width / Checksum start/end），无 this.xxx |
| ADR-004 三种 field 函数 | ✅ CheckNode 明确为 RO 字段（_field_kind = "ro"，与 Computed 同） |
| ADR-006 表达式 VM 仅 i64 | ✅ 所有"动态值"位置编译为 ExprProgram；eval_expr_bool/bytes/any 扩展在设计 D-P0-1/D-P0-2 中明确 |
| ADR-008 parse 借用实例 dict | ✅ 不修改（设计 §11.1） |
| ADR-012 StopField Result 哨兵 | ✅ CancelParsing 复用 Result 模式（Error 变体 + schema.rs 顶层 catch） |
| ADR-014 不接 callable | ✅ Default/Check/Aligned/Checksum.StreamRange/Probe.into 全部编译为 ExprProgram；DF-4/CK-5/CS-13/PB-5 编译期拒绝 lambda |
| ADR-022 用户面 Adapter Python 层化 | ✅ Hex/HexDump Rust Node 是 ADR-022 §0.2 论证落地；AdapterCallbackNode 不修改 |

**建议沉淀的新 ADR**（设计 §11.3）：
- ADR-023：Checksum 双轨方案（L-14 工程化）—— **REV 推荐 8.5 ACCEPTED 时必沉淀**（L-14 重要里程碑）
- ADR-024：Hex/HexDump Rust Node 设计—— 可选（与 ADR-022 同脉络，可合并引用）
- ADR-025：CancelParsing Error 变体 + 顶层 catch—— 可选（与 ADR-012 同脉络）

---

## 6. 教训对照（experiences.md）

| 教训 | 对照结果 |
|------|---------|
| L-01 中间表示层违反 | ✅ 6 子任务全部含 §0 原则对照表（§1.4/§2.5/§3.6/§4.4/§5.3/§6.3），L-01 对策落实 |
| L-02 理论估算替代实证数据 | ✅ 性能假设章节全部含量化来源（pyo3 benchmark / Phase 2 已验证 / sha2 SIMD 基准） |
| L-05 优化 A 路径忽略 B 路径 | ✅ 8.5 §3.7.2 显式列 4 条 FFI/拷贝来源 + 路径 A 额外 2 条；其他子任务 FFI 来源清单齐全 |
| L-13 docstring 未跟随语法演进 | ✅ 所有示例用字段名直接引用（无 this）；DEV 实施时需注意 docstring 同步（建议 VET 检查） |
| L-14 设计硬约束认知需交叉验证 | ✅ Checksum 双轨方案是 L-14 教训工程化的标志案例；设计 §3 开头明确标注"L-14 教训触发" |

---

## 7. 给 PM 的反馈

1. **结论：通过（PASS-WITH-FOLLOWUPS）**。建议 PM 接受设计，但要求 ARCH 在 DEV 实施前完成 §3 的 3 个建议补充项修订。
2. **建议补充项优先级**：
   - §3.1（Checksum build StreamRange 语义）：**高**——DEV 实施 8.5 前必须明确
   - §3.2（checksumfield 非 PyBytes）：**中**——8.5 DEV 实施时补充边界条件
   - §3.3（Hex/HexDump §0 措辞）：**低**——文档质量，可在 VET 阶段同步
3. **小问题（§4）**：DEV 实施时自行修订，不需 ARCH 重新提交。
4. **ADR 沉淀建议**：8.5 ACCEPTED 时必沉淀 ADR-023（L-14 里程碑）；8.4/8.10 可选。
5. **实施顺序**：按设计 §0.1 建议（8.10 → 8.1 → 8.9 → 8.8 → 8.4 → 8.5）分派 DEV，REV 验证合理。
6. **无需修订 ADR-001~ADR-022**（设计 §11.2 明确不修改现有 ADR）。

---

> **检视完成时间**：2026-07-31
> **检视员**：REV
> **下一步**：PM 确认 → ARCH 修订 3 个建议补充项 → 按 §0.1 实施顺序分派 DEV（8.10 → 8.1 → 8.9 → 8.8 → 8.4 → 8.5）
