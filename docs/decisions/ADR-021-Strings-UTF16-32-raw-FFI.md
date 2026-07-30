---
id: ADR-021
title: Strings UTF-16/32 编解码 unsafe raw FFI（首次正常路径 unsafe）
status: accepted
phase: "6"
decides: "Strings utf16/utf32 编解码用 CPython raw FFI（PyUnicode_DecodeUTF16/32 + AsUTF16/32String），是项目首次正常路径 unsafe；utf8/ascii 保持安全路径"
supersedes: []
superseded_by: ""
depends_on: [ADR-019]
last_updated: 2026-07-30
---

# ADR-021: Strings UTF-16/32 编解码 unsafe raw FFI（首次正常路径 unsafe）

## Context

Phase 6.2 Strings（7 个构造器）共享一套编解码层（`Encoding::decode` / `Encoding::encode`）。
Python `possiblestringencodings`（core.py:1695）封闭列出 6 类编码（ascii / utf8 / utf16 / utf32，
及其别名）。本 ADR 沉淀"编解码底层实现路线"的跨阶段决策。

### 问题：utf16/utf32 在 Rust 标准库无原生支持

Python `str.encode("utf_16_le")` / `bytes.decode("utf_16_le")` 是 CPython 内建能力。
要在 Rust 侧实现 bytes↔PyUnicode 双向转换且**不引入中间表示层**（§0 #2），有三条候选路线：

| 路线 | UTF-8 / ASCII 实现 | UTF-16 / UTF-32 实现 | unsafe？ | 依赖 | 中间表示？ |
|------|-------------------|---------------------|---------|------|-----------|
| **X（CPython raw FFI）** | `PyUnicode_DecodeUTF8` | `PyUnicode_DecodeUTF16/32` | ✅ 是（正常路径） | 无 | ❌ 无 |
| **Y（encoding_rs crate）** | `encoding_rs::UTF_8.decode()` | `encoding_rs::UTF_16LE.decode()` | ❌ 否 | +1 crate | ⚠️ Rust `String` 中转 |
| **Z（std + pyo3）** | `std::str::from_utf8` + `PyString::new_bound` | （不支持，需 X 或 Y） | ❌ 否 | 无 | ⚠️ Rust `str` 中转 |

utf16/32 路径下，**只有路线 X** 能避免 `PyString → Rust String/Cow<str> → 重建 PyString/PyBytes`
的荒谬往返（违反 §0 #2 精神）。

### ADR-019 先例：错误路径 unsafe raw FFI 已验证可行

Phase 4.x 错误路径（`error.rs::try_fast_path_alloc`）首次使用 `pyo3::ffi` raw CPython C API
绕过 Python `__init__` 字节码，实测 a_err_eof **11.34x**（VET Controlled A/B Test，DLL hash 验证
真重编译）。ADR-019 建立了 unsafe raw FFI 的 SAFETY 注释规范（每个 unsafe 块列出前置条件 +
失败模式 + 回退路径）。

本 ADR 是项目**首次正常路径**（非错误路径）的 unsafe raw FFI——utf16/32 编解码是 Strings 的
正常数据流，不是错误分支。需要把 ADR-019 的规范从错误路径扩展到正常路径。

### utf16/32 的使用频率边界

`possiblestringencodings` 的 6 类编码中，**utf8/ascii（unit=1）覆盖用户 >90% 场景**（ASCII 协议
文本、UTF-8 JSON 等）；utf16/32（unit=2/4）是用户主动指定的特殊编码（SMB / NTLM / JSON UTF-16
等）。用户选择 utf16/utf32 编码 = 已意识到编码开销的折衷。

## Decision

### 核心决策：混合方案（Z + X）

| 编码 | 实现路线 | unsafe？ | 理由 |
|------|---------|---------|------|
| **utf8 / ascii** | **Z（std + pyo3 安全路径）** | ❌ 否 | Rust 标准库 `std::str::from_utf8` 是最优零拷贝字节验证路径，无需 unsafe；encode 方向 `to_cow()` 在 ASCII 主导场景多为 `Borrowed` 零分配 |
| **utf16Le / utf16Be / utf32Le / utf32Be** | **X（CPython raw FFI）** | ✅ 是 | Rust 标准库无原生 UTF-16/32 编解码；raw FFI 是唯一不引入 Rust `String` 中转的方式（§0 #2 合规） |

unsafe 范围**严格限定**到 4 个变体的双向编解码（decode + encode 共 8 个 unsafe 调用点），
utf8/ascii（用户最常用 >90% 场景）保持完全安全路径。

### decode 方向：`PyUnicode_DecodeUTF16/32`（bytes → PyUnicode）

```rust
unsafe {
    ffi::PyUnicode_DecodeUTF16(
        bytes.as_ptr(),          // GIL 持有 + bytes 生命周期覆盖 unsafe 块
        bytes.len() as isize,    // CPython 按 unit_size 分组，尾部不完整单元拒绝并设 PyErr
        null(),                  // errors=NULL 用默认 "strict"
        &byteorder as *const i32 // 栈上临时 i32，CPython 调用后不再使用
    )
}
// UTF-32 用 PyUnicode_DecodeUTF32，签名一致
```

**byteorder 语义**（P2a 修订，v2 设计 §2.2.2）：

| 用户传入 | Rust `Encoding` 变体 | CPython `byteorder` 值 | 含义 |
|---------|---------------------|----------------------|------|
| `utf_16_le` / `utf_32_le` | `Utf16Le` / `Utf32Le` | **-1** | 强制 Little Endian，**不扫描 BOM** |
| `utf_16_be` / `utf_32_be` | `Utf16Be` / `Utf32Be` | **+1** | 强制 Big Endian，**不扫描 BOM** |

**严禁使用 `byteorder = 0`**（自动检测 BOM）：若数据流开头恰好是 `\xff\xfe`（LE BOM 字节序列），
会被误判为 BOM 消费掉，破坏 parity（Python `bytes.decode("utf_16_le")` 不消费 BOM）。用户传
`utf_16_le`/`utf_16_be` 期望"纯 LE/BE 无 BOM 语义"，必须用 ±1。

### encode 方向：`PyUnicode_AsUTF16/32String`（PyUnicode → PyBytes）

```rust
let ptr = unsafe {
    ffi::PyUnicode_AsUTF16String(obj.as_ptr())  // obj 是 downcast 后的 &Bound<PyString>
};
// UTF-32 用 PyUnicode_AsUTF32String
```

**本机序 byte-swap 处理**：CPython `AsUTF16String`/`AsUTF32String` 输出**本机字节序**字节串（无 BOM）。
返回 PyBytes 后需按目标字节序处理：

- 目标 == 本机序（如 `Utf16Le` 在 LE 平台）→ 直接取 `as_bytes()`
- 目标 != 本机序（如 `Utf16Be` 在 LE 平台）→ 按 unit_size（2 或 4）分组 byte-swap

本机序检测用编译期 `cfg!(target_endian = "little")`，无运行时开销。

**encode 直接持有 `&Bound<PyString>` 调 raw FFI，不经 `Cow<str>` 中转**（P1 修订，v2 设计 §2.2.2）——
否则产生 `PyString → Cow<str> → 重建 PyString → PyBytes` 的往返，违反 §0 #2。

### SAFETY 前置条件列表（按 ADR-019 模板）

| # | unsafe API | 安全前置条件 | 失败模式 | 回退 |
|---|-----------|-------------|---------|------|
| D1 | `PyUnicode_DecodeUTF16/32` | GIL 持有（`py: Python<'py>` token 在作用域）+ `bytes.as_ptr()` 有效（来自 `&[u8]`，生命周期覆盖整个 unsafe 块）+ `bytes.len()` 是字节数 + `&byteorder` 是栈上临时指针（CPython 调用后不再使用） | NULL + PyErr（非法字节序列 / 尾部不完整单元） | `PyErr::fetch(py)` → `ConstructError::String` |
| E1 | `PyUnicode_AsUTF16/32String` | GIL 持有 + `obj.as_ptr()` 是有效 PyUnicode 指针（来自 downcast 后的 `&Bound<PyString>`） | NULL + PyErr（罕见，obj 非 str 已在 downcast 前过滤） | `PyErr::fetch(py)` → `ConstructError::String` |

**所有权转移**：
- decode 非 NULL 返回 → `Bound::from_owned_ptr` 消费指针（owned，无 incref）
- encode 非 NULL 返回 → `Bound::<PyBytes>::from_owned_ptr` 消费指针 → `as_bytes()` → `to_vec()` / 本机序 swap 后 `to_vec()`

**引用计数追踪**：encode 路径 `obj`（`&Bound<PyString>`，借用引用，refcount 不变）→ AsUTF16String 产新
PyBytes（owned）→ 转 `Vec<u8>` 后 PyBytes 析构。无引用泄漏。

### BC 回退路径（对齐 ADR-019 三层降级精神）

1. unsafe API 返回 NULL → `PyErr::fetch(py)` 取回 Python 异常 → 转 `ConstructError::String { message, path }`
2. `ConstructError::String` 经 `From<ConstructError> for PyErr`（ADR-019 fast-path / 慢路径）抛出
   Python 侧 `StringError`（v2 设计 §3.8 新增变体 + `ExceptionClasses` 注册）

**注意**：与 ADR-019 不同，utf16/32 raw FFI 是正常数据路径，**无"fast-path 失败回退慢路径"的双路径**
设计——decode/encode 只有 raw FFI 一条路径，NULL 即转 StringError。ADR-019 的 BC6 三层降级
（fast-path → 慢路径 → ValueError 兜底）不直接适用；本 ADR 的回退是"raw FFI NULL → ConstructError::String"。

### 错误变体：`ConstructError::String`（v2 设计 §3.8，本 ADR 依赖）

新增 `ConstructError::String { message, path }` 变体（对齐 Python `StringError` core.py:54）。
同步修改 4 处（v2 设计 §3.8.3）：
- `error.rs` enum 新增 `String` 变体 + `message()` / `path()` / `kind_str()` 分支
- `ExceptionClasses` 新增 `string_error: Py<PyType>`
- `init_exception_classes` 新增 `get("StringError")?`
- `select_exception_class` + `is_builtin_class`（`[13]` → `[14]`，让 StringError 走 ADR-019 fast-path）

## 适用范围

仅 Phase 6.2 Strings 的 `Encoding::decode_utf16_32_raw` / `Encoding::decode_utf16_32_raw`（utf16/32
双向共 8 个 unsafe 调用点）。utf8/ascii 路径（>90% 用户场景）保持完全安全。

**不触发 §0 八条原则任何一条**（v2 设计 §6 §0 对照表逐条核对）：
- #1 一次 FFI：raw FFI 在 Rust 内部（GIL 已持有），不跨 Python↔Rust 边界穿越
- #2 无中间表示层：decode 直接产 PyUnicode；encode 直接消费 `&Bound<PyString>` 产 PyBytes。不经 Rust `String`/`Cow<str>` 中转（本 ADR 核心合规论证）
- #3 无 I/O trait 抽象：Encoding 是数据 enum，非 trait
- #4 pyo3 核心：raw FFI 是 pyo3 公开稳定 API（`pyo3::ffi`）

## 禁止行为

- ❌ utf8/ascii 路径用 unsafe raw FFI（std 安全路径已最优）
- ❌ utf16/32 decode 用 `byteorder = 0`（自动检测 BOM 会破坏 parity，必须用 ±1）
- ❌ utf16/32 encode 经 `Cow<str>` 中转（违反 §0 #2，P1 修订禁止）
- ❌ 任何 unsafe 块无 SAFETY 注释（必须按 ADR-019 模板列出前置条件 + 失败模式 + 回退）
- ❌ raw FFI 返回 NULL 不调 `PyErr::fetch` 取回异常（避免 Python 错误状态污染）
- ❌ 接受无后缀 `utf16`/`utf32`/`u16`/`u32` 编码（编译期 Err 引导用 `_le`/`_be` 后缀，见 Consequences）

## Consequences

### 正面

- **§0 #2 合规**：utf16/32 双向编解码无 Rust `String` 中转，是 Rust 标准库无 UTF-16/32 原生支持下的
  唯一不引入中间表示层的方式
- **性能**：类比 ADR-019 a_err_eof 11.34x（同 unsafe CPython C API 路径），utf16/32 预测 ≥4-5x
  （接近总门禁，需 VET Controlled A/B Test 验证，v2 设计 §7 风险点）
- **utf8/ascii 安全路径保底**：>90% 用户场景零 unsafe，性能预期 ≥10x（类比 Phase 5 B1 11.64x）
- **不引入新 crate**：避免 encoding_rs（路线 Y）的 ~200KB 编译产物 + Rust String 二次拷贝开销
- **错误精度**：新增 `ConstructError::String` 变体，用户面 `except StringError` 精确捕获（对齐 Python）

### 负面

- **unsafe 范围扩大到正常路径**：ADR-019 是错误路径（罕见），本 ADR 是正常数据路径（utf16/32 用户
  主动选编码时必经）。SAFETY 审查负担增加（VET 重点核查 8 个 unsafe 调用点）
- **utf16/32 性能可能接近门限**：raw FFI 虽无 Rust String 中转，但 PyUnicode_DecodeUTF16/32 本身
  有 CPython 内部分配开销。实测若 <4x，需评估是否回退 encoding_rs（路线 Y，§备选方案 1）
- **本机序 byte-swap 增加复杂度**：encode 路径需按 `cfg!(target_endian)` 分支 + unit_size 分组 swap，
  测试矩阵翻倍（LE 平台 + BE 平台 × 4 编码变体）

### 中性

- **breaking change P2b**（v2 设计 §2.1.3）：无后缀 `utf16`/`utf32`/`u16`/`u32` 编码编译期 Err
  引导用 `utf_16_le`/`utf_16_be`/`utf_32_le`/`utf_32_be`。理由：Python `str.encode("utf16")` 加
  本机序 BOM（跨平台不可移植），construct-rs 作为协议解析库应强制显式字节序。迁移成本仅改一个
  字符串字面量，错误信息携带引导文本

## Alternatives Considered

### 替代方案 1：encoding_rs crate（路线 Y，全安全）

**描述**：utf16/32 用 `encoding_rs::UTF_16LE.decode()`（Rust 安全 API），产 Rust `String` 再用
`PyString::new_bound` 转 PyUnicode。

**为何不采用**：
1. **§0 #2 违反**：引入 `PyString → Rust String → 重建 PyString` 的中间表示层（encode 方向更严重：
   `PyString → Cow<str> → encoding_rs → Vec<u8> → PyBytes`）。本 ADR 核心目标是 §0 #2 合规
2. **新 crate 依赖**：+~200KB 编译产物 + ~50ns 解码开销（Rust String → PyString 二次拷贝）
3. **性能下降**：utf16/32 从预测 ≥4-5x 降到 ≥3x（Rust String 中转 + 二次拷贝），可能不达 ≥4x 总门禁

**保留为降级方案**（v2 设计 §2.2.4）：若 PM 拒绝 unsafe 或 VET 实测 utf16/32 raw FFI <4x，
回退 encoding_rs（牺牲性能 + 引入 crate）。ARCH 不推荐。

### 替代方案 2：全部用 CPython raw FFI（路线 X，含 utf8/ascii）

**描述**：utf8/ascii 也用 `PyUnicode_DecodeUTF8` raw FFI。

**为何不采用**：Rust 标准库 `std::str::from_utf8` 已是零拷贝字节验证的最优路径（返回 `&str` 借用
原始字节，无分配）。`PyUnicode_DecodeUTF8` 虽功能等价但引入不必要的 unsafe。本 ADR 把 unsafe
范围**严格限定**到 utf16/32 4 个变体，utf8/ascii 保持安全。

### 替代方案 3：encoding_rs decode + raw FFI encode（不对称混合）

**描述**：decode 用 encoding_rs（安全），encode 用 raw FFI（直接消费 PyString）。

**为何不采用**：decode 路径仍违反 §0 #2（Rust String 中转），且引入双库（encoding_rs + raw FFI）
维护负担。本 ADR 选择对称的 Z+X 混合（utf8/ascii 全安全，utf16/32 全 raw FFI），逻辑清晰。

## Relations

- **关联 ADR**：`ADR-019 错误抛出 fast-path`（unsafe raw FFI 先例）
  - ADR-021 扩展 ADR-019 的 unsafe raw FFI 规范从**错误路径**到**正常路径**
  - 两者形成 unsafe raw FFI 的两个维度：

| 维度 | ADR-019（错误路径） | ADR-021（正常路径） |
|------|--------------------|--------------------|
| 触发场景 | `ConstructError → PyErr` 转换（错误抛出） | Strings utf16/32 编解码（正常数据流） |
| unsafe API | `PyType_GenericAlloc` + `PyObject_SetAttrString` × 3（绕过 `__init__`） | `PyUnicode_DecodeUTF16/32` + `AsUTF16/32String` |
| 回退机制 | BC6 三层降级（fast-path → 慢路径 → ValueError） | 单层（raw FFI NULL → ConstructError::String） |
| 首次验证 | Phase 4.x（a_err_eof 11.34x，2026-07-28 ACCEPTED） | Phase 6.2（待 VET 验证） |

- **关联教训**：
  - `harness/experiences.md §L-01`（中间表示层违反）——本 ADR 核心对策：raw FFI 是 utf16/32 避免引入
    Rust String 中转层的唯一方式（§0 #2 合规）
  - `harness/experiences.md §L-02`（理论估算替代实证数据）——utf16/32 性能 ≥4-5x 是**类比 ADR-019
    的预测**（非实测），Phase 6.2 VET 必须用 Controlled A/B Test（L-09 对策）实测验证
- **设计文档**：`docs/design/模块设计/模块设计-Strings.md`（§2.2 编解码实现 + §2.2.2 P1/P2a 修订 +
  §2.2.3 混合方案理由 + §3.8 错误变体 + §6 §0 对照表 + §7 性能假设 + §12 D-2 决策记录）
- **PM 决策**：`plans/phase6-primitives-strings-adapter/总纲.md §PM 决策 6.2-D2`（接受混合方案，
  建议 ADR-021 沉淀 unsafe 先例）
- **VET 验证**：`harness/experiences.md §L-09`（跨时段性能对比消除法归因失效）——Phase 6.2 VET
  utf16/32 性能验证必须用 Controlled A/B Test（DLL hash 验证 + 同会话 3 轮交替均值），不可依赖
  跨时段单次对比
- **实现位置**：`construct-rs/src/nodes/strings/encoding.rs`（`decode_utf16_32_raw` + `encode_utf16_32_raw`，
  DEV 实施 R4，v2 设计 §10.1）
- **SAFETY 审查点**：`plans/phase6-primitives-strings-adapter/traces/6.2-VET.md`（VET 重点核查
  8 个 unsafe 调用点的 SAFETY 注释 + 引用计数链路 + byteorder ±1 + 本机序 swap）
- **首次验证**：Phase 6.2（Strings，2026-07-30 设计 ACCEPTED，DEV/VET 待 6.6 集成验证）
- **未来复用**：Phase 7+ 任何需要 bytes↔PyUnicode UTF-16/32 转换的场景（如 Tunnel / 自定义编码
  Adapter）可直接引用本 ADR；同时为"正常路径 unsafe raw FFI"建立可复用 SAFETY 模板

### ADR 编号说明

本 ADR 占用编号 **021**（非 022）。Phase 6.2 Strings 设计（v2 §10.1 R3.5）与 Phase 6.3 Adapter
设计（§12）曾都预留 ADR-021。PM 决策采用**方案 A**：6.2 Strings（先验收）用 ADR-021，
6.3 Adapter 用 ADR-022。本 ADR 即 Strings 的 unsafe raw FFI 决策；用户面 Adapter 决策见
`ADR-022`。
