---
id: QUERY-bytesinteger-bigint
status: active
phase: "6"
type: design-query
raised_by: user
forwarded_by: PM
responder: ARCH
target_design: docs/design/模块设计/模块设计-Primitives收尾.md §1.5
target_impl: construct-rs/src/nodes/bytes_integer.rs
created: 2026-07-30
last_updated: 2026-07-30
placement_note: "PM 原指示写入 plans/phase6-primitives-strings-adapter/traces/，但 AGENTS.md §2 规定 ARCH 不可写 plans/。本文件暂存 ARCH 可写范围 docs/design/queries/，请 PM 决定是否迁移/复制到 traces/。"
---

# 设计质疑回应：BytesInteger >8 字节方案 — num-bigint 评估 + u128 路径发现

## §0 质疑摘要（用户原话要点）

用户对 6.1 设计 D-2 决策（BytesInteger >8 字节走 Python `int.from_bytes` 慢路径）提出三点质疑：

1. **"零 unsafe"与"高性能"为何被当作互斥？** 设计似乎只给出"unsafe raw FFI vs Python 慢路径"二选一。
2. **`num-bigint` crate 为何没评估？** Rust 原生大整数库，bytes→BigInt→PyObject 全程零 unsafe + 可能高性能。
3. **是否还有其他零 unsafe + 高性能方案？** 如 pyo3 安全 API 构造 PyLong。

PM 要求 ARCH 回应四项：(1) 承认或反驳设计遗漏；(2) num-bigint 补充评估；(3) 其他候选；(4) 修正建议。

---

## §1 质疑确认（ARCH 自检）

### 1.1 事实核对：设计 §1.5.1 实际给出**三个**选项（非两个）

用户陈述"ARCH 6.1 设计只给了两个选项"与文档事实**不符**。设计文档 §1.5.1（`模块设计-Primitives收尾.md` line 689-694）的决策对比表明确列出三个方案：

| 方案 | unsafe？ | 中间类型？ | 性能（大整数） | 选用 |
|------|---------|----------|-------------|-----|
| **A. Python `int.from_bytes`** | 否 | 否 | ~5-8x | ✅ 最终选择 |
| B. raw FFI `PyLong_AsByteArray` | 是 | 否 | ~8-12x | ❌ 排除 |
| **C. num-bigint 中间类型** | 否 | 是（声称违反 §0 #2） | ~3-5x | ❌ 排除 |

**因此"只给了两个选项 / num-bigint 没评估"的事实前提不成立**——option C 即 num-bigint，已在表中列出并给出性能量级（~3-5x）。

### 1.2 部分承认：option C（num-bigint）评估**过浅**

尽管 num-bigint 被列为 option C，ARCH **承认该评估过浅**，具体缺陷：

1. **拒绝理由单薄**：仅表格 cell 一句"违反 §0 #2（中间类型）"，无独立小节论证。
2. **性能量级（~3-5x）无依据**：未说明 3-5x 如何得出，未拆解 BigInt→PyLong 转换成本。
3. **与设计自身 §3.1 澄清矛盾**：§3.1（line 1075-1083）已澄清"`half::f16` / `u64` / `bytes` 是 Rust 内部 transient 计算载体，非 §0 #2 所指的'跨 FFI 返回的 Rust enum/dict/trait 抽象层'"。按同一逻辑，**BigInt 作为 parse 内部 transient 载体（不跨 FFI 返回为 Rust 类型）是否真违反 §0 #2，存疑**——拒绝理由需重新论证。
4. **未评估 BigInt→PyLong 转换的实际可行路径**（见 §2.2，这是真正的技术瓶颈，比 §0 #2 更致命）。

**结论**：用户"num-bigint 没评估"的直觉**部分正确**——被列名但未严肃评估。ARCH 承认此设计缺陷，将在 §2 补全评估。

### 1.3 不承认：用户"二选一"的框架

用户将问题框架化为"unsafe raw FFI ↔ Python 慢路径"二选一，并推论"零 unsafe 与高性能被当互斥"。**此框架不成立**：

- 设计实际是三选一（A/B/C），且 option A（Python 慢路径）本身就是"零 unsafe"路径，证明 ARCH **并未**将两者当互斥——只是认为"零 unsafe 路径中，Python from_bytes 比 num-bigint 更优"。
- 但 ARCH **承认**：option A（Python from_bytes）虽零 unsafe，性能确实不佳（实测 2.82x，低于 §4.4 门禁 ≥4x）。用户对**性能结果**的不满是合理的，只是把原因误归到"漏评 num-bigint"，而真正遗漏的是 **§3.1 将揭示的 u128 路径**。

---

## §2 num-bigint 方案深度评估（补全设计缺陷）

### 2.1 技术可行性

`num-bigint` 提供 `BigInt::from_signed_bytes_le/be` 与 `to_signed_bytes_le/be`，bytes↔BigInt 双向可行。

**真正瓶颈在 BigInt → PyLong 方向**（parse 路径的输出）。pyo3 0.22 **不原生支持** `num-bigint`：
- pyo3 的 `IntoPy<Py<PyAny>>` 仅实现到 `i128/u128/isize/usize`（`docs/analysis/phase3-parse-build不对称.md §3.1` 实测 i128::into_py 21-57ns）。
- pyo3 **无** `PyLong::from_digits()` / `PyLong::from_bigint()` 等接受任意精度 digit 数组的安全构造 API。
- `num-bigint` 的 `BigInt` 内部 digit 基数（`u32` 或 `u64`，平台相关）≠ CPython `PyLong` 内部 digit 基数（`2^30`，`PYLONG_BITS_IN_DIGIT=30`），无法直接 memcpy。

### 2.2 BigInt → PyLong 转换的三条实际路径（核心分析）

| 路径 | 机制 | unsafe？ | 性能量级 | 评价 |
|------|------|---------|---------|------|
| **(a) string round-trip** | `bigint.to_string()` → Python `int(str)` | 否 | **极慢**（~µs 级：Rust 格式化 + Python 字符串解析） | 不可用 |
| **(b) bytes round-trip** | `bigint.to_signed_bytes_be()` → `int.from_bytes(bytes)` | 否 | **≈ 当前 slow-path + 额外 round-trip**（bytes→BigInt→bytes，净增成本） | 比当前更慢 |
| **(c) pyo3-num-bigint crate** | 第三方 crate，内部用 `pyo3::ffi::_PyLong_FromByteArray` | **是（藏依赖里）** | ~8-12x（同 raw FFI） | unsafe 转移，非真零 unsafe |

**路径 (b) 详解**（最能说明 num-bigint 无益）：
```
当前 slow-path:   bytes ────────────────────► PyLong   （1 次 int.from_bytes）
num-bigint path:  bytes ─► BigInt ─► bytes ─► PyLong   （多一次 Rust 内 bytes↔BigInt 双向转换）
```
num-bigint 路径在 parse 末端仍要调 `int.from_bytes`，且在 Rust 内多一次 `from_signed_bytes` + `to_signed_bytes` 的内存分配与拷贝。**净效果：比当前 slow-path 更慢**。

**路径 (c) 详解**（用户认为"零 unsafe"的关键反驳）：
`pyo3-num-bigint`（社区 crate）的 BigInt→PyLong 实现依赖 `pyo3::ffi::_PyLong_FromByteArray`——**这是 unsafe raw FFI**，只是封装在依赖 crate 内。从 construct-rs 的 `cargo` 视角看不到 `unsafe` 关键字，但**安全责任转移给了第三方 crate 的维护者**，等同于"把 unsafe 藏起来"。这与 ADR-019 建立的规范（"每个 unsafe 块必须有完整 SAFETY 注释 + 前置条件 + 失败模式"）相悖——我们无法在 construct-rs 内为第三方 crate 的 unsafe 块写 SAFETY 注释。

### 2.3 性能预测（修正设计 §1.5.1 的 ~3-5x 估计）

| 子路径 | Rust 端预估 | vs Python（~300-500ns） |
|--------|------------|----------------------|
| num-bigint + 路径 (a) string | ~1000-2000ns | **0.2-0.5x（比 Python 还慢）** |
| num-bigint + 路径 (b) bytes round-trip | ~350-500ns（当前 slow-path + ~80-120ns BigInt 双向转换） | **~1-1.5x（劣于当前 2.82x）** |
| num-bigint + 路径 (c) 第三方 crate | ~80-150ns（同 raw FFI） | ~3-6x（但非真零 unsafe） |

**设计原估 "~3-5x" 实际对应路径 (c)**，即"把 unsafe 藏进依赖"的情形——这恰好证明了用户"零 unsafe 与高性能互斥"在 num-bigint 场景下**实质成立**：要高性能必须走 raw FFI（路径 c 的依赖内部就是 raw FFI），真零 unsafe（路径 a/b）则性能不优。

### 2.4 §0 #2 重新评估（设计拒绝理由本身存疑）

按设计 §3.1 自身澄清（line 1077）："'中间表示层'指**跨 FFI 返回的 Rust enum / dict / per-field trait 抽象层**"。BigInt 在 num-bigint 路径中是 parse 内部的 **transient 计算载体**（bytes→BigInt→立即转 PyLong，不跨 FFI 返回为 Rust 类型），与已澄清的 `half::f16`（"仅作 Rust 内部位运算载体"）性质相同。

**因此"违反 §0 #2"作为拒绝 num-bigint 的理由本身是站不住脚的**——这是设计的另一个缺陷。但 §2.2-2.3 表明：num-bigint 的真正问题是**无高效安全的 BigInt→PyLong 转换**，拒绝结论（不用 num-bigint）仍然正确，只是理由应从"违反 §0 #2"改写为"无高效安全 BigInt→PyLong 路径"。

### 2.5 成本

- 新增依赖：`num-bigint`（~30KB 编译产物）+ 可能的 `pyo3-num-bigint`（若走路径 c）
- Cargo.toml + Cargo.lock 改动
- 实现复杂度：~40-60 行（vs 当前 slow-path ~40 行），无净收益

### 2.6 num-bigint 结论

**拒绝成立，但理由需修正**。num-bigint 在 BytesInteger >8 字节场景**不优于当前 Python slow-path**：
- 真零 unsafe 路径（a/b）性能劣于或等同于当前 slow-path；
- 高性能路径（c）实为 unsafe raw FFI 藏进依赖，违反 ADR-019 的 unsafe 可见性规范。

**用户关于 num-bigint 的核心主张（"零 unsafe + 可能高性能"）经评估不成立**——num-bigint 在此场景下无法同时满足零 unsafe 与高性能。

---

## §3 其他零 unsafe + 高性能候选（ARCH 主动发现）

本节是本次质疑回应的**核心新增贡献**。ARCH 在补评 num-bigint 过程中发现一个**设计真正遗漏的零 unsafe + 高性能方案**。

### 3.1 u128/i128 pyo3 safe 路径（9-16 字节）—— 真正的遗漏方案 ⭐

**关键证据**（`docs/analysis/phase3-parse-build不对称.md §3.1`，已实测）：

| 操作 | ns/op | 说明 |
|------|-------|------|
| `i64::into_py(42)` | 1.70 | 小整数缓存 |
| `i128::into_py(42)` | **21.63** | pyo3 走 `_PyLong_FromByteArray` 类似路径 |
| `i128::into_py(-1)` | **57.18** | 负数任意精度路径 |

**pyo3 0.22 原生支持 `u128/i128::into_py`**，内部封装 `_PyLong_FromByteArray`-类 C API（与设计 option B 的 raw FFI 是**同一个 C 函数**），但从 construct-rs 代码视角**零 unsafe**（unsafe 封装在 pyo3 内部，由 pyo3 维护者审计，等同 `i64::into_py` 内部调 `PyLong_FromLongLong` 的性质）。

**u128/i128 精确覆盖 9-16 字节范围**：
- `u128` 最大值 = `2^128 - 1` = 恰好 16 字节无符号上界
- `i128` 覆盖 16 字节有符号（two's complement）
- 9-15 字节通过符号扩展/零填充补齐到 16 字节后走 i128/u128（与当前 8 字节 fast-path 补齐到 8 字节同模式）

**为什么这是"零 unsafe + 高性能"**：
- **零 unsafe**（construct-rs 视角）：仅调 `u128::from_be_bytes` + `.into_py(py)`，无 `unsafe` 块
- **高性能**：parse 末端 ~21-57ns（实测），vs 当前 slow-path ~280-380ns（call_method ~80ns + Python from_bytes 执行 ~200-300ns）
- **预测加速比**：16 字节场景从当前 **2.82x → ~8-12x**（Python BytesInteger(16) ≈ 300-500ns，Rust u128 路径 ~30-60ns）

**为什么设计漏了它**：设计 §1.5.2 的 fast-path 阈值 `FAST_PATH_MAX_LEN = 8`（`bytes_integer.rs:35`）直接套用 u64/i64 的 8 字节上限，**未考虑 u128/i128 可覆盖到 16 字节**。phase3 分析（2026-07 早于 6.1 设计）已有 i128::into_py 实测数据，但 6.1 设计未回溯引用。

### 3.2 pyo3 digit-array 构造 API（用户问及）

**不存在**。pyo3 0.22 的 PyLong 构造仅支持：
- 固定位宽整数 `IntoPy`：`i8..i128` / `u8..u128` / `isize` / `usize`
- 字符串：需经 Python `int(str)`（慢）

无 `PyLong::from_digits(&[u32])` / `PyLong::from_limb_slice` 等接受任意精度 limb 数组的公开安全 API。**因此 >16 字节无法用 pyo3 安全 API 高效构造**。

### 3.3 >16 字节方案

| 字节数 | 零 unsafe 高性能方案 | 评估 |
|--------|-------------------|------|
| 9-16 | **u128/i128 pyo3 safe 路径**（§3.1） | ✅ 存在，应实施 |
| 17-32 | 无（pyo3 无 u256；num-bigint 无高效安全转换） | ❌ 必须走 Python slow-path 或 raw FFI |
| >32 | 同上 | ❌ |

**>16 字节是真正罕见场景**（RSA 2048 模数 = 256 字节、SHA-256 = 32 字节但通常用 Bytes 而非 BytesInteger）。16 字节（UUID / GUID / SHA-1 / MD5 / IPv6 数值表示）是"大整数"中最常见的尺寸——**修复 9-16 字节即可覆盖绝大多数 >8 字节实际用法**。

---

## §4 性能重新预测（u128 路径引入后）

### 4.1 修正后的三档 fast/slow 路径

| 字节长度 | 路径 | Rust 端预估 | 预测加速比 | 实测对照 |
|---------|------|------------|---------|---------|
| 1-8 | u64/i64 fast-path（现有） | ~30-50ns | ~10-15x | 已验证（6.6 VET 9.32x median） |
| **9-16** | **u128/i128 pyo3 safe（新增）** | **~40-80ns** | **~8-12x** | 待 VET 复测 |
| >16 | Python `int.from_bytes` slow-path（现有） | ~280-400ns | ~2-4x | 罕见，可接受 |

### 4.2 §4.4 性能门禁修正建议

| 场景 | 原门禁 | 修正后门禁 | 理由 |
|------|-------|---------|-----|
| BYTESINT-fast（≤8） | （未单列） | ≥10x | u64/i64 路径 |
| **BYTESINT-mid（9-16）** | （原归入 slow ≥4x） | **≥8x** | u128 路径，修复 2.82x |
| BYTESINT-slow（>16） | ≥4x | ≥2x（软目标） | 真罕见场景，Python slow-path |

---

## §5 修正建议

### 5.1 修改设计文档 §1.5（扩展 fast-path 到 ≤16 字节）

**改动点**：
1. §1.5.2 `BytesIntegerNode` 数据结构：新增注释说明三档路径
2. §1.5.3 parse：`FAST_PATH_MAX_LEN` 改为分档常量 `U64_PATH_MAX=8` / `U128_PATH_MAX=16`，9-16 字节走 u128/i128 补齐 + `into_py` 分支
3. §1.5.4 build：9-16 字节走 `extract::<i128>` / `extract::<u128>` + 字节序转换 + 范围检查（i128/u128 范围检查用 i128 中间值，与现有 `check_range` 同模式扩展）
4. §1.5.1 决策表：option C（num-bigint）补全 §2.2-2.4 的评估理由（拒绝理由从"违反 §0 #2"改为"无高效安全 BigInt→PyLong 路径"）
5. §4.2/§4.4 性能预测与门禁按 §4 修正
6. §3 §0 原则对照表：BytesInteger 列补 u128 路径说明（`u128/i128` 同 `u64/i64`，是 Rust 原生整数，pyo3 直接转换）

### 5.2 num-bigint 保持拒绝

ARCH 决定（非 PM 决策点）。理由见 §2.6。设计文档需补全评估论证（§5.1 改动点 4）。

### 5.3 >16 字节保持 Python slow-path

ARCH 决定（非 PM 决策点）。>16 字节真罕见，Python slow-path 零 unsafe、实现简洁，性能 ~2-4x 可接受。若未来出现 >16 字节热路径需求，再评估 raw FFI（option B，需 PM 决策接受 unsafe + ADR-019 式 SAFETY 论证）。

### 5.4 §0 原则对照（u128 路径合规性）

| §0 原则 | u128 路径合规性 |
|---------|---------------|
| #1 一次 FFI | ✅ parse 末尾单次 `into_py`，build 开头单次 `extract` |
| #2 无中间表示层 | ✅ `u128/i128` 是 Rust 原生整数，pyo3 直接 ↔ PyLong（与 `u64/i64` 同性质，§3.1 已澄清） |
| #3 输入输出侧无抽象 trait | ✅ 无新 trait |
| #4 pyo3 核心依赖 | ✅ 用 pyo3 `IntoPy` / `extract`，无 raw FFI |
| #5-8 | ✅ 同现有 fast-path |

---

## §6 PM 决策点

### D-Q1：是否接受扩展 fast-path 到 ≤16 字节（u128 路径重实施）？

- **ARCH 推荐**：**是**。修复 16 字节场景 2.82x → 预测 8-12x，零 unsafe，无新依赖，改动量小（~60-80 行，扩展现有 `parse`/`build`/`check_range`）。
- **影响范围**：`bytes_integer.rs` 重实施 + 重跑 bench（6.6 VET 复测 BYTESINT-mid 场景）+ 设计文档 §1.5/§4 修订
- **风险**：低。u128/i128 IntoPy 已被 phase3 分析实测验证可用；逻辑与现有 8 字节 fast-path 同构（仅位宽翻倍）
- **决策性质**：PM 决策（涉及已 ACCEPTED 子任务 6.1 的设计修订 + DEV 重实施）

### D-Q2：num-bigint 保持拒绝（ARCH 决定，非 PM 决策点）

见 §2.6。设计文档仅需补全评估论证。

### D-Q3：6.6 ACCEPTED 是否需回退？

- **ARCH 建议**：**无需整体回退**。6.6 验收的 24 个构造器中仅 BytesInteger(16) 单点受影响（2.82x），其余 23 个构造器性能数据不受 u128 路径影响。
- **处理方式**：将 D-Q1 作为 **6.1 的增量优化子任务**（如 `6.1-fix [BytesInteger u128 fast-path 扩展]`），走 DESIGNING → CODING → CODE_REVIEW → VET 复测 BYTESINT-mid 场景。原 6.6 ACCEPTED 保持，新增子任务验收后更新 inventory.csv 性能列。
- **PM 决策点**：是否开 6.1-fix 子任务？或并入 Phase 7+ 性能优化批次？

---

## §7 ARCH 总结回答（对应 PM 要求的四项）

| PM 要求 | ARCH 回答 |
|--------|---------|
| **(1) 是否承认设计遗漏** | **部分承认**。用户"只给两个选项 / num-bigint 没评估"的事实前提**不成立**（设计 §1.5.1 实为三选项，num-bigint 是 option C）。但承认 option C 评估**过浅**（拒绝理由单薄、与 §3.1 自身澄清矛盾、未分析 BigInt→PyLong 转换瓶颈）。同时承认设计**遗漏了 u128/i128 路径**（§3.1，真正的零 unsafe + 高性能方案）。 |
| **(2) num-bigint 性能预测** | **不优于当前**。真零 unsafe 路径（string/bytes round-trip）性能 ~0.2-1.5x（劣于当前 2.82x）；高性能路径（pyo3-num-bigint 依赖）实为 unsafe 藏依赖。详见 §2.3。 |
| **(3) 是否建议修改设计 + 重新实施** | **是，但改的是 u128 路径，非 num-bigint**。建议扩展 fast-path 到 ≤16 字节（§5.1），num-bigint 保持拒绝（§5.2）。>16 字节保持 Python slow-path（§5.3）。 |
| **(4) PM 决策点** | **D-Q1**（是否接受 u128 路径重实施，ARCH 推荐"是"）+ **D-Q3**（6.1-fix 子任务开否 / 并入 Phase 7+）。D-Q2 非 PM 决策点。 |

**一句话回应**：用户的直觉（"应有零 unsafe + 高性能方案"）**正确**，但答案不是 num-bigint（经评估确实不优），而是 **u128/i128 pyo3 safe 路径**（9-16 字节，phase3 已实测可用）——这是 6.1 设计真正遗漏的选项，修复后 16 字节场景预计从 2.82x 提升到 8-12x。
