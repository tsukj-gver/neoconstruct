---
id: DESIGN-phase6-strings
status: active
phase: "6"
revision: v2
depends_on: [DESIGN-Architecture, DESIGN-Expr, DESIGN-Array, ADR-001, ADR-007, ADR-016, ADR-017, ADR-019]
supersedes: []
superseded_by: []
last_updated: 2026-07-30
---

> **修订记录**
> - **v1**（2026-07-30）：首次产出。
> - **v2**（2026-07-30）：依据 6.2-REV 检视报告（`plans/phase6-primitives-strings-adapter/traces/6.2-REV检视.md`）局部修订。**整体方向不变**（6 Node 独立 / 编译期 enum / StringError / 模块组织 / 边界条件 / parity 模板）。修订点：
>   - **P1（§0 #2 违反）**：§2.2.2 encode 按编码分支，utf16/32 直接持有 `&Bound<PyString>` 调 raw FFI，不经 `Cow<str>` 中转
>   - **P2a（可行性）**：§2.2.2 decode `byteorder` 修正为 `Le => -1, Be => 1`（CPython `PyUnicode_DecodeUTF16` 语义）
>   - **P2b（parity 决策）**：ARCH 选 **方案 1（显式不支持无后缀 utf16/utf32）**，理由见 §2.1.2 决策记录
>   - P3（性能假设补类比基准）/ P4（StringEncoded breaking change 标注）/ P5（ADR-021 前置）/ P6（PascalString 验证时机）/ P7（Node enum 变体数 24→20）

# 模块设计：Strings 支持（Phase 6.2）

> **设计依据**：
> - `AGENTS.md` §0（核心原则：一次 FFI、无中间表示层、直接操作 Python 对象）、
>   §3（全员红线）、`harness/extensions/architect-extension.md`（模块设计模板）
> - `plans/phase6-primitives-strings-adapter/总纲.md` §PM 决策 1（方案 A 独立 Node）
> - `docs/analysis/分析报告-Phase6启动前置.md` §A.4 / §B.3（Strings 实现路线 + 构造器分析）
> - `construct-rs/src/nodes/mod.rs`（Node enum + Construct trait）
> - `construct-rs/src/nodes/bytes.rs` / `greedy_bytes.rs` / `prefixed_array.rs`（实现参考）
> - `construct-rs/src/stream.rs`（ParseStream / BuildStream）
> - `construct-rs/src/error.rs`（现有错误变体）
> - `construct-rs/tests/_helpers/parity.py`（parity helper API 契约，6.0 已 ACCEPTED）
> - Python 源码：`construct/construct/core.py`
>   （CString L1811、PascalString L1778、PaddedString L1747、GreedyString L1837、
>   StringEncoded L1711、NullTerminated L5050、NullStripped L5123、FixedSized L4986、
>   possiblestringencodings L1695、StringError L54）
>
> **角色**：ARCH
> **状态**：DESIGNING（首次产出，2026-07-30）
> **子任务标识**：`6.2 [Strings 详细设计]`

---

## 0. 设计摘要

### 0.1 7 个构造器一览

| # | 构造器 | Rust Node | 持有字段 | 返回类型 | 跨 Phase 依赖 |
|---|--------|-----------|---------|---------|--------------|
| 1 | `CString(enc)` | **CStringNode** | `encoding` | `str` | 无 |
| 2 | `GreedyString(enc)` | **GreedyStringNode** | `encoding` | `str` | 无 |
| 3 | `PaddedString(length, enc)` | **PaddedStringNode** | `length`, `encoding` | `str` | 无 |
| 4 | `PascalString(lengthfield, enc)` | **PascalStringNode** | `lengthfield: Box<Node>`, `encoding` | `str` | 无（lengthfield 是任意已实现 Node） |
| 5 | `NullTerminated(subcon, term, ...)` | **NullTerminatedNode** | `inner: Box<Node>`, `term`, `include/consume/require` | inner 的返回类型（通常 `bytes`） | 无 |
| 6 | `NullStripped(subcon, pad)` | **NullStrippedNode** | `inner: Box<Node>`, `pad` | inner 的返回类型（通常 `bytes`） | 无 |
| 7 | `StringEncoded(subcon, enc)` | **不实现为 Rust Node** | — | — | Python 侧 macro 工具（§3.7） |

### 0.2 PM 决策 1 落地（方案 A：独立 Node）

PM 决策 1 明确："6 个 String 构造器实现为独立 Node，直接处理 byte↔str 编解码，**不照搬 Python macro 嵌套**（PascalString→Prefixed / PaddedString→FixedSized）"。

本设计严格遵循：
- **6 个 Rust Node**（表 #1-#6）：每个 Node 直接完成"读字节 → 解码为 str"或"读字节 → 转发 inner"，无 Adapter/Subconstruct 嵌套层。
- **StringEncoded 不实现为 Rust Node**（表 #7）：Python 原版中 StringEncoded 是 Adapter 子类，包装其他构造器并做 bytes↔str 转换。在方案 A 下，编解码逻辑已下沉到 CStringNode / GreedyStringNode / PaddedStringNode / PascalStringNode 内部，StringEncoded 失去存在意义。Phase 6.2 在 Python 侧保留 `StringEncoded` 名字仅作为兼容性别名（§3.7），Rust 侧不分配 Node 变体。

### 0.3 Phase 6.2 自洽性确认

**7 个构造器全部可在 Phase 6 自洽实现，无跨 Phase 依赖**：
- 不需要 Phase 7 的 `Prefixed`（PascalString 独立实现 length 读取）
- 不需要未规划的 `FixedSized`（PaddedString 独立实现固定长度读取 + pad）
- `NullTerminated` / `NullStripped` 持有 `inner: Box<Node>`，但 inner 由用户传入（默认 `GreedyBytes`，Phase 1 已实现），不依赖 Phase 6.3 Subconstruct
- `PascalString.lengthfield` 是任意已实现 Node（用户传入 `VarInt` / `Int16ub` 等，依赖 6.1 Primitives 或 Phase 1 FormatField）

**唯一外部依赖**：6.1 Primitives 的 `VarInt`（推荐 lengthfield）。但 PascalString 的 lengthfield 由**用户**传入，不是 6.2 编译期硬依赖——即使用户传入 Phase 1 的 `Byte` / `Int16ub`，PascalString 也能工作。6.2 可与 6.1 并行（与总纲 §子任务依赖图一致）。

### 0.4 关键设计决策摘要

| 决策 | 选项 | ARCH 推荐 | 详见 |
|------|------|----------|------|
| 编码表示 | (A) 编译期 `Encoding` enum / (B) 运行期字符串匹配 | **A** | §2.1 |
| 编解码实现 | (X) CPython raw FFI / (Y) encoding_rs crate / (Z) std + pyo3 | **Z（utf8/ascii）+ X（utf16/32）混合** | §2.2 |
| **无后缀 utf16/utf32 处理**（v2 新增 P2b） | (1) 显式不支持编译期报错 / (2) 新增 BOM 变体 | **1**（显式不支持） | §2.1.3 |
| StringError 错误映射 | (A) 新增 `ConstructError::String` 变体 / (B) 复用 `Generic` | **A** | §3.8 |
| PaddedString.length 类型 | (A) 仅常量 `usize` / (B) `BytesLength` 复用（Const + Expr） | **B** | §3.5 |
| term/pad 字节串表示 | (A) `Vec<u8>` / (B) 从 `Encoding` 推导 | **A**（保留 Python 多字节 term 兼容性） | §3.2 / §3.3 |

---

## 1. 模块位置与文件组织

### 1.1 Rust 源码位置

参照分析报告 §B.3 "公共组件建议"：新增 `nodes/strings/` 子模块目录。

```
construct-rs/src/nodes/
├── mod.rs                      # Node enum 新增 6 变体（CString/GreedyString/PaddedString/
│                               #                       PascalString/NullTerminated/NullStripped）
└── strings/                    # 新增子模块（Phase 6.2）
    ├── mod.rs                  # 子模块入口（pub mod 声明 + 公共 re-export）
    ├── encoding.rs             # Encoding enum + 编解码 helper（§2）
    ├── c_string.rs             # CStringNode（§3.1）
    ├── greedy_string.rs        # GreedyStringNode（§3.4）
    ├── padded_string.rs        # PaddedStringNode（§3.5）
    ├── pascal_string.rs        # PascalStringNode（§3.6）
    ├── null_terminated.rs      # NullTerminatedNode（§3.2）
    └── null_stripped.rs        # NullStrippedNode（§3.3）
```

**子模块化理由**：
- 6 个 Node 文件 + 1 个 encoding 文件，集中管理避免污染 `nodes/` 顶层
- 与 `nodes/array/` 等未来扩展保持平级（Phase 7+ Conditional 可建 `nodes/conditional/`）
- 公共 encoding helper 被所有 String Node 共享，单独文件避免循环依赖

### 1.2 Python 侧改动位置

```
construct-rs/python/construct/
├── _descriptors.py             # 新增 6 个 Descriptor（CStringDescriptor / ...）
├── _construct_rust.rs          # Rust 模块新增 6 个 Node 构造器 pyfunction
└── __init__.py                 # （无需改）CString / PascalString / ... 已在 __init__ 暴露
```

### 1.3 测试位置（参照 6.0 已 ACCEPTED 目录结构）

```
construct-rs/tests/
├── parity/
│   └── test_phase6_strings_parity.py   # 新增（§11 parity 模板）
├── unit/
│   └── test_strings_unit.py            # 新增（Rust 内部 unit 测试，参考 bytes.rs 测试风格）
└── errors/
    └── test_strings_errors.py          # 新增（编码错误 / 长度错误 / EOF 等）

construct-rs/tests/_helpers/
└── encoding_parity.py                  # 占位模块，DEV 实施时填充（6.0 已建空文件）

construct-rs/bench/
└── bench_strings.py                    # 新增（性能基线，§7 性能假设验证）
```

---

## 2. 公共编码层设计（核心）

> **本节是 PM 决策点"StringEncoded 的编码可配置方案（编译期 vs 运行期）"的技术论证**。
> 见 §12 PM 决策点 D-1 / D-2。

所有 String Node 共享一套编码处理逻辑。本节定义公共类型与 helper，§3 各 Node 调用之。

### 2.1 编码表示：编译期 `Encoding` enum（决策 A）

#### 2.1.1 Python 支持的编码清单

Python `possiblestringencodings`（core.py:1695-1700）显式列出 6 类编码别名：

```python
possiblestringencodings = dict(
    ascii=1,
    utf8=1, utf_8=1, u8=1,
    utf16=2, utf_16=2, u16=2, utf_16_be=2, utf_16_le=2,
    utf32=4, utf_32=4, u32=4, utf_32_be=4, utf_32_le=4,
)
```

数字（1/2/4）是**编码单元字节数**（unit_size），用于构造多字节 term / pad。

#### 2.1.2 Rust `Encoding` enum 定义（编译期）

```rust
// nodes/strings/encoding.rs

/// Phase 6.2 Strings 支持的编码（封闭集合，对齐 Python possiblestringencodings）。
///
/// 编码在 Descriptor 编译期（`__init_subclass__`）从用户字符串解析为 enum 变体，
/// 运行时零字符串匹配开销（决策 A）。
///
/// 不在列表内的编码名 → `ConstructError::Compilation`（编译期失败，不进入运行时）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// ASCII（unit=1 字节）。Rust 标准库 + CPython `PyUnicode_DecodeASCII`。
    Ascii,
    /// UTF-8（unit=1 字节）。Rust 标准库 `std::str::from_utf8` + `PyString::new_bound`。
    Utf8,
    /// UTF-16 Little Endian（unit=2 字节）。CPython `PyUnicode_DecodeUTF16(NULL, 0, NULL)`。
    Utf16Le,
    /// UTF-16 Big Endian（unit=2 字节）。CPython `PyUnicode_DecodeUTF16(&byteorder=1)`。
    Utf16Be,
    /// UTF-32 Little Endian（unit=4 字节）。CPython `PyUnicode_DecodeUTF32(NULL, 0, NULL)`。
    Utf32Le,
    /// UTF-32 Big Endian（unit=4 字节）。CPython `PyUnicode_DecodeUTF32(&byteorder=1)`。
    Utf32Be,
}

impl Encoding {
    /// 从用户传入的字符串解析为 `Encoding`（编译期一次）。
    ///
    /// 对齐 Python `possiblestringencodings` 的**有后缀**编码别名集合（normalize：去 `-` 改 `_`、转小写）。
    /// 未识别 → `Err(ConstructError::Compilation)`，由 Descriptor 转为用户面 `StringError`。
    ///
    /// **P2b 决策（v2 修订）**：**不接受无后缀的 `utf16`/`utf_32`/`u16`/`u32` 别名**。
    /// 理由见 §2.1.2 决策记录。用户传无后缀编码 → `Err(Compilation)`，
    /// 错误信息明确引导使用 `utf_16_le`/`utf_16_be`/`utf_32_le`/`utf_32_be`。
    ///
    /// # 参数
    /// - `s`：用户传入的编码名（如 `"utf8"` / `"UTF-16-LE"`）
    pub fn from_user_str(s: &str) -> Result<Self, ConstructError> {
        let normalized: String = s.replace('-', "_").to_ascii_lowercase();
        match normalized.as_str() {
            "ascii" => Ok(Encoding::Ascii),
            "utf8" | "utf_8" | "u8" => Ok(Encoding::Utf8),
            // utf8/utf_8/u8 不含 BOM 语义（CPython `str.encode("utf8")` 不加 BOM），可安全接受
            "utf_16_le" => Ok(Encoding::Utf16Le),
            "utf_16_be" => Ok(Encoding::Utf16Be),
            "utf_32_le" => Ok(Encoding::Utf32Le),
            "utf_32_be" => Ok(Encoding::Utf32Be),
            // 显式拒绝无后缀别名（utf16/utf_16/u16/utf32/utf_32/u32）。
            // 这些在 Python 中带"本机字节序 BOM"语义，construct-rs 不支持。
            "utf16" | "utf_16" | "u16" | "utf32" | "utf_32" | "u32" => {
                Err(ConstructError::Compilation {
                    message: format!(
                        "encoding {:?} is ambiguous (Python adds native-endian BOM, \
                         which is non-portable). construct-rs requires explicit byte order: \
                         use {:?} or {:?} instead.",
                        s,
                        if normalized.starts_with("utf16") || normalized.starts_with("u16") {
                            "utf_16_le"
                        } else {
                            "utf_32_le"
                        },
                        if normalized.starts_with("utf16") || normalized.starts_with("u16") {
                            "utf_16_be"
                        } else {
                            "utf_32_be"
                        },
                    ),
                })
            }
            _ => Err(ConstructError::Compilation {
                message: format!(
                    "encoding {:?} not found among supported encodings \
                     (ascii, utf8/utf_8/u8, utf_16_le, utf_16_be, utf_32_le, utf_32_be). \
                     Note: bare utf16/utf32 are rejected (use explicit _le/_be suffix).",
                    s
                ),
            }),
        }
    }

    /// 编码单元字节数（utf8/ascii=1, utf16=2, utf32=4）。
    /// 用于 term/pad 字节串的长度推导与尾部不完整单元检测。
    pub const fn unit_size(&self) -> usize {
        match self {
            Encoding::Ascii | Encoding::Utf8 => 1,
            Encoding::Utf16Le | Encoding::Utf16Be => 2,
            Encoding::Utf32Le | Encoding::Utf32Be => 4,
        }
    }

    /// 默认 term 字节串（unit_size 个 0x00），用于 CString（无显式 term 参数时）。
    /// 对齐 Python `encodingunit(encoding)` 返回 `bytes(unit_size)`。
    pub fn default_term(&self) -> Vec<u8> {
        vec![0u8; self.unit_size()]
    }
}
```

#### 2.1.3 P2b 决策记录：显式不支持无后缀 `utf16`/`utf32`（v2 修订）

REV P2b 指出：Python `CString("utf16").build("h")` 产出 `b'\xff\xfeh\x00\x00\x00'`（含本机序 BOM），而 v1 设计把 `utf16`/`utf_16`/`u16` 静默映射到 `Utf16Le`（无 BOM）→ **parity 不一致**。

ARCH 在 REV 给出的两种方案中选择**方案 1（显式不支持无后缀编码）**：

| 项 | 方案 1（ARCH 选 ✅） | 方案 2（备选） |
|----|---------------------|---------------|
| **行为** | `from_user_str("utf16")` → `Err(Compilation)`，编译期报错引导用户用 `utf_16_le`/`utf_16_be` | Encoding enum 增加 `Utf16Bom`/`Utf32Bom` 变体；decode 用 `byteorder=0` 自动检测 BOM；encode 用 `PyUnicode_AsUTF16String` + 本机序 BOM |
| **优点** | (1) 6 变体 enum 保持不变；(2) 编译期早失败，错误信息明确；(3) 不引入 BOM 处理复杂度 | 完全兼容 Python parity |
| **缺点** | **breaking change**：Python `CString("utf16")` 可用，construct-rs 报错 | (1) enum 增 2 变体；(2) encode 需按本机序决定 BOM 字节序（跨平台不可移植）；(3) BOM 解码路径与无 BOM 路径分叉，测试矩阵翻倍 |

**ARCH 选方案 1 的理由**：

1. **Python 行为本身不可移植**：CPython `str.encode("utf16")` 加**本机字节序** BOM（x86/ARM 写 `b'\xff\xfe'` LE BOM，Sparc 写 `b'\xfe\xff'` BE BOM）。同一份 Python 代码在不同平台产出的二进制数据不同——这是 Python 字符串编码的著名陷阱。construct-rs 是协议解析库，应**强制用户显式指定字节序**，避免产出不可移植的二进制。

2. **用户群体主流习惯**：实际协议规范（如 JSON UTF-16 / SMB / NTLM）几乎都用显式 `utf_16_le`。`utf16` 无后缀主要出现在 Python 文档示例中，生产代码罕见。

3. **错误时机友好**：在 `__init_subclass__`（编译期）报错，错误信息明确引导用户改用 `utf_16_le`/`utf_16_be`，迁移成本仅改一个字符串字面量。

4. **parity 测试影响可控**：6.2 parity case 矩阵（§11.2）原本就用 `utf_16_le`（如 `CS-utf16-1`），无需调整。

**用户面影响**：
- **breaking change**：从 Python construct 迁移的用户，如代码用 `CString("utf16")`/`PaddedString(10, "utf16")`/`PascalString(VarInt, "utf16")`/`GreedyString("utf16")`，需改为 `utf_16_le` 或 `utf_16_be`。
- DEV 实施时在 Python 侧 docstring 明确标注此差异（§3.7.3 + §10 P3 同步标注）。
- 错误信息携带引导文本（见 `from_user_str` 错误分支）。

**与 §0 兼容性**：方案 1 不引入新 trait / 不增加跨 FFI 边界 / 不增加中间表示层——完全 §0 合规。

**备选方案 2 保留为后续选项**：若 VET 实测阶段或用户反馈证明 `utf16` 无后缀编码是高频需求，可在 Phase 6.3+ 扩展（增加 `Utf16Bom`/`Utf32Bom` 变体）。届时 encode 用 `PyUnicode_AsUTF16String`（输出本机序无 BOM）+ 手动写本机序 BOM；decode 用 `byteorder=0`（自动检测 BOM）。本设计不为方案 2 预留接口（避免 YAGNI）。

#### 2.1.4 决策 A 的理由（编译期 enum vs 运行期字符串匹配）

| 维度 | 决策 A（编译期 enum） ✅ | 决策 B（运行期字符串匹配） ❌ |
|------|------------------------|----------------------------|
| **运行时开销** | 0（编译期解析完毕，运行时直接 match） | 每次 parse/build 字符串哈希 + 匹配（~20-50ns） |
| **类型安全** | 编译期穷尽性检查（match 缺分支 → 编译错误） | 运行期才暴露未支持的编码 |
| **错误时机** | 编译期（`__init_subclass__`）报 Compilation 错误 | 运行期首次 parse 才报错 |
| **代码体积** | 6 个 match 分支（每分支 5-15 行） | 单一函数 + 字符串表查找 |
| **与 §0 #1（一次 FFI）兼容性** | ✅ | ✅ |

**ARCH 推荐 A**。理由：编码名是用户在类定义时**一次性**传入的静态信息（`PaddedString(10, "utf8")`），不属于运行时数据，编译期解析最自然。Python `possiblestringencodings` 也是封闭集合（6 类），用 enum 表达恰好。

### 2.2 编解码实现：混合方案（Z + X）

> **PM 决策点 D-2**：编解码底层实现方案（详见 §12）

#### 2.2.1 三条候选路线

| 路线 | UTF-8 / ASCII 实现 | UTF-16 / UTF-32 实现 | unsafe？ | 依赖 | 中间表示？ |
|------|-------------------|---------------------|---------|------|-----------|
| **X（CPython raw FFI）** | `PyUnicode_DecodeUTF8` | `PyUnicode_DecodeUTF16/32` | ✅ 是（热路径） | 无 | ❌ 无 |
| **Y（encoding_rs crate）** | `encoding_rs::UTF_8.decode()` | `encoding_rs::UTF_16LE.decode()` | ❌ 否 | +1 crate | ⚠️ Rust `String` 中转 |
| **Z（std + pyo3）** | `std::str::from_utf8` + `PyString::new_bound` | （不支持，需 X 或 Y） | ❌ 否 | 无 | ⚠️ Rust `str` 中转 |

#### 2.2.2 ARCH 推荐：混合方案（Z + X）

```rust
// nodes/strings/encoding.rs

impl Encoding {
    /// bytes → PyUnicode（parse 方向）。
    ///
    /// 直接产 PyString，不经过 Rust String 中转（除 utf8/ascii 的 std 路径）。
    ///
    /// # 实现路径（决策 Z+X 混合）
    /// - Utf8 / Ascii：std::str::from_utf8 → PyString::new_bound（安全路径 Z）
    /// - Utf16Le/Be / Utf32Le/Be：CPython `PyUnicode_DecodeUTF16/32` raw FFI（X）
    ///
    /// # 错误
    /// 解码失败（非法字节序列）→ `ConstructError::String`（§3.8 新增变体），
    /// 对齐 Python `StringError("cannot use encoding ... to decode ...")`。
    pub fn decode<'py>(
        &self,
        py: Python<'py>,
        bytes: &[u8],
        path: &mut Path,
    ) -> Result<Bound<'py, PyString>, ConstructError> {
        match self {
            Encoding::Utf8 => match std::str::from_utf8(bytes) {
                Ok(s) => Ok(PyString::new_bound(py, s)),
                Err(_) => Err(ConstructError::String {
                    message: format!(
                        "cannot use encoding 'utf8' to decode {} bytes (invalid UTF-8 sequence)",
                        bytes.len()
                    ),
                    path: path.to_string(),
                }),
            },
            Encoding::Ascii => {
                // ASCII = UTF-8 子集；额外校验所有字节 < 128 对齐 Python ascii 编码严格性
                if bytes.iter().all(|&b| b < 128) {
                    Ok(PyString::new_bound(py, std::str::from_utf8(bytes).unwrap_or("")))
                } else {
                    Err(ConstructError::String {
                        message: format!(
                            "cannot use encoding 'ascii' to decode (byte >= 128 found)"
                        ),
                        path: path.to_string(),
                    })
                }
            }
            Encoding::Utf16Le | Encoding::Utf16Be | Encoding::Utf32Le | Encoding::Utf32Be => {
                // 路线 X：CPython raw FFI
                self.decode_utf16_32_raw(py, bytes, path)
            }
        }
    }

    /// PyUnicode → owned bytes（build 方向）。
    ///
    /// 返回 `Vec<u8>` 而非 `&[u8]`，因 PyUnicode 内部表示可能需转换分配。
    /// 调用方（各 String Node 的 build）随后 `stream.write(&vec)`。
    ///
    /// **P1 修订（v2）**：encode 按编码分支处理，避免 utf16/32 路径引入 Rust `Cow<str>` 中转层
    /// （违反 §0 #2"build 直接从 PyObject 读"）。
    /// - utf8/ascii（安全路径 Z）：`to_cow()` 转 Rust `Cow<str>`。此路径下 ASCII 主导，
    ///   PyString 内部表示多为 ASCII compact（`to_cow` 返回 `Borrowed`，零分配）；即使非 ASCII，
    ///   utf8 编码本身要求 UTF-8 字节序列，转 Rust str 不可避免。
    /// - utf16/32（raw FFI 路径 X）：**直接持有原始 `&Bound<PyString>`**，调 CPython
    ///   `PyUnicode_AsUTF16String`/`PyUnicode_AsUTF32String`（输入是 PyObject 指针，不是 Rust str）。
    ///   不经 `Cow<str>`——否则产生 `PyString → Cow<str> → 重建 PyString → PyBytes` 的荒谬往返。
    ///
    /// # 输入校验
    /// 非 `str` 类型 → `ConstructError::String`（对齐 Python `StringError("expected unicode string")`）。
    pub fn encode<'py>(
        &self,
        py: Python<'py>,
        obj: &Bound<'py, PyAny>,
        path: &mut Path,
    ) -> Result<Vec<u8>, ConstructError> {
        match self {
            Encoding::Utf8 | Encoding::Ascii => {
                // 安全路径 Z：to_cow 在 utf8/ascii 场景可接受
                // （ASCII 主导，多为 Borrowed；非 ASCII 时 UTF-8 转换不可避免）
                let s = obj
                    .downcast::<PyString>()
                    .map_err(|_| ConstructError::String {
                        message: format!(
                            "string encoding failed, expected unicode string, got {}",
                            obj.get_type()
                                .name()
                                .map(|n| n.to_string())
                                .unwrap_or_else(|_| "<unknown>".to_string())
                        ),
                        path: path.to_string(),
                    })?
                    .to_cow()?;
                match self {
                    Encoding::Utf8 => Ok(s.into_owned().into_bytes()),
                    Encoding::Ascii => {
                        // 严格 ASCII：所有字符 < 128（对齐 Python ascii 编码严格性）
                        if s.bytes().all(|b| b < 128) {
                            Ok(s.into_owned().into_bytes())
                        } else {
                            Err(ConstructError::String {
                                message: format!(
                                    "cannot use encoding 'ascii' to encode (non-ASCII char)"
                                ),
                                path: path.to_string(),
                            })
                        }
                    }
                    _ => unreachable!("Utf8/Ascii branch only"),
                }
            }
            Encoding::Utf16Le | Encoding::Utf16Be | Encoding::Utf32Le | Encoding::Utf32Be => {
                // raw FFI 路径 X：直接 downcast，不调用 to_cow（P1 修订）
                // 仅做类型校验，PyObject 所有权不转移，传 &Bound<PyString> 给 raw FFI
                let py_str = obj.downcast::<PyString>().map_err(|_| {
                    ConstructError::String {
                        message: format!(
                            "string encoding failed, expected unicode string, got {}",
                            obj.get_type()
                                .name()
                                .map(|n| n.to_string())
                                .unwrap_or_else(|_| "<unknown>".to_string())
                        ),
                        path: path.to_string(),
                    }
                })?;
                // 直接传原始 PyString 给 raw FFI（不经 Cow<str>，§0 #2 合规）
                self.encode_utf16_32_raw(py, py_str, path)
            }
        }
    }

    /// UTF-16/32 解码（路线 X：CPython raw FFI）。
    ///
    /// 使用 `PyUnicode_DecodeUTF16` / `PyUnicode_DecodeUTF32`，通过 byteorder 参数
    /// 选择 LE/BE。
    ///
    /// **P2a 修订（v2）**：byteorder 取值修正——
    /// - LE → **byteorder = -1**（强制 LE，**不扫描 BOM**）
    /// - BE → **byteorder = +1**（强制 BE，**不扫描 BOM**）
    ///
    /// CPython `PyUnicode_DecodeUTF16(s, size, errors, &byteorder)` byteorder 语义：
    /// | 值 | 含义 |
    /// |----|------|
    /// | `0` | 自动检测 BOM（开头有 BOM 消费并按 BOM 序；无 BOM 按本机序） |
    /// | `-1` | 强制 Little Endian，**不检测 BOM** |
    /// | `+1` | 强制 Big Endian，**不检测 BOM** |
    ///
    /// 用户传 `utf_16_le`/`utf_16_be` 期望"纯 LE/BE 无 BOM 语义"，必须用 ±1 而非 0。
    /// 若用 0（v1 错误值），数据流开头恰好是 `\xff\xfe`（LE BOM 字节序列）会被误判为 BOM
    /// 消费掉，破坏 parity（Python `bytes.decode("utf_16_le")` 不消费 BOM）。
    ///
    /// # SAFETY（按 ADR-019 unsafe CPython C API 安全前置条件列表）
    /// - GIL 持有：`py: Python<'py>` token 在作用域内
    /// - `bytes.as_ptr()` 有效：来自 `&[u8]` 切片，生命周期覆盖整个 unsafe 块
    /// - `bytes.len()` 是字节数（CPython 内部按 unit_size 分组：UTF-16 按 2 字节、UTF-32 按 4 字节；
    ///   尾部不完整单元 CPython 会拒绝并设置 PyErr）
    /// - `&byteorder` 指针仅在本函数栈上读写，CPython 调用结束后不再使用（CPython 内部可能
    ///   修改 byteorder 值，但本函数不依赖调用后的值）
    /// - 返回值非 NULL 时所有权转移给 `Bound::from_owned_ptr`；NULL 时 PyErr 已设置，
    ///   由 `PyErr::fetch` 取回并转换为 `ConstructError::String`
    #[allow(clippy::unnecessary_wraps)]
    fn decode_utf16_32_raw<'py>(
        &self,
        py: Python<'py>,
        bytes: &[u8],
        path: &mut Path,
    ) -> Result<Bound<'py, PyString>, ConstructError> {
        // 实现细节由 DEV 填充。关键点（v2 修订后）：
        // 1. byteorder: i32 = match self {
        //        Utf16Le | Utf32Le => -1,   // 强制 LE，不扫描 BOM
        //        Utf16Be | Utf32Be => 1,    // 强制 BE，不扫描 BOM
        //    }
        // 2. let ptr = unsafe { ffi::PyUnicode_DecodeUTF16(
        //        bytes.as_ptr(), bytes.len() as isize, null(), &byteorder as *const i32) };
        //    （UTF-32 用 PyUnicode_DecodeUTF32，签名一致）
        // 3. NULL → PyErr::fetch(py) → ConstructError::String
        // 4. 非 NULL → Bound::from_owned_ptr → downcast to PyString
        // SAFETY 注释逐项列出前置条件（GIL / 指针有效 / 长度一致 / byteorder 语义 / 所有权转移）
        todo!("DEV 实现：参考 ADR-019 unsafe 块 SAFETY 注释模板 + ADR-021（待 PM 决策）")
    }

    /// UTF-16/32 编码（路线 X：CPython raw FFI）。
    ///
    /// **P1 修订（v2）**：签名从 `s: &Cow<'py, str>` 改为 `obj: &Bound<'py, PyString>`，
    /// 直接持有原始 PyString PyObject 调 raw FFI，不经 Rust `Cow<str>` 中转（§0 #2 合规）。
    ///
    /// # 实现要点
    /// - 使用 `PyUnicode_AsUTF16String(obj.as_ptr())` / `PyUnicode_AsUTF32String(obj.as_ptr())`
    ///   输入是 PyObject 指针（不是 Rust str），输出是本机字节序的 PyBytes（无 BOM）
    /// - **本机序处理**：CPython `AsUTF16String`/`AsUTF32String` 输出**本机字节序**字节串，
    ///   需在返回 PyBytes 上按目标字节序做 byte-swap：
    ///   - 目标 == 本机序（如 `Utf16Le` 在 LE 平台）：直接取 as_bytes()
    ///   - 目标 != 本机序（如 `Utf16Be` 在 LE 平台）：按 unit_size（2 或 4）分组 swap
    /// - 本机序检测：编译期 cfg(target_endian = "little")（无运行时开销）
    ///
    /// # SAFETY
    /// - GIL 持有：`py: Python<'py>` token
    /// - `obj.as_ptr()` 是有效的 PyUnicode 对象指针（来自 downcast 后的 Bound<PyString>）
    /// - 返回 PyBytes 非 NULL 时所有权转移；NULL 时 PyErr::fetch 取回转 ConstructError::String
    fn encode_utf16_32_raw<'py>(
        &self,
        py: Python<'py>,
        obj: &Bound<'py, PyString>,
        path: &mut Path,
    ) -> Result<Vec<u8>, ConstructError> {
        // 实现要点（v2 修订）：
        // 1. let ptr = unsafe { ffi::PyUnicode_AsUTF16String(obj.as_ptr()) };
        //    （UTF-32 用 PyUnicode_AsUTF32String）
        // 2. NULL → PyErr::fetch → ConstructError::String
        // 3. 非 NULL → Bound::<PyBytes>::from_owned_ptr → as_bytes() → 按 endian 处理：
        //    if cfg!(target_endian = "little") && matches!(self, Utf16Be | Utf32Be) {
        //        // 本机 LE 但目标 BE：按 unit swap
        //        byteswap_unit(bytes, unit_size)
        //    } else if cfg!(target_endian = "big") && matches!(self, Utf16Le | Utf32Le) {
        //        byteswap_unit(bytes, unit_size)
        //    } else {
        //        bytes.to_vec()  // 本机序与目标序一致
        //    }
        // SAFETY 注释：GIL / obj.as_ptr() 是有效 PyUnicode / 所有权转移 / byteorder 处理
        let _ = (py, obj, path);
        todo!("DEV 实现：按上述要点 + ADR-019 SAFETY 模板")
    }
}
```

#### 2.2.3 混合方案的理由

1. **§0 #2（无中间表示层）精神**：v2 修订后，utf8/ascii 走 std + pyo3（`std::str::from_utf8` 零拷贝字节验证 + `PyString::new_bound` 直接构造；encode 方向 `to_cow()` 在 ASCII 主导场景多为 `Borrowed` 零分配），utf16/32 在 decode 和 encode **两个方向**都用 CPython raw FFI（decode 直接产 PyUnicode；encode 直接消费 `&Bound<PyString>` 产 PyBytes），不经 Rust `Cow<str>` 中转。这是 utf16/32 在 Rust 标准库无原生支持下的唯一不引入中间表示层的方式。

2. **避免新 crate 依赖**：encoding_rs（路线 Y）虽安全，但：
   - 增加 ~200KB 编译产物 + ~50ns 解码开销（Rust String → PyString 二次拷贝）
   - 项目目前依赖极简（pyo3 + thiserror + enum_dispatch + half），引入 encoding_rs 违背"无中间表示层"精神

3. **unsafe 范围限定**：仅 utf16/32 4 个变体走 raw FFI；utf8/ascii（用户最常用，>90% 场景）保持安全路径。utf16/32 的 unsafe 块按 ADR-019 模板（每个 unsafe 块附 SAFETY 注释 + 失败模式 + BC6 回退）。

4. **ADR-019 先例**：Phase 4.x 错误路径已验证 unsafe raw FFI 在热路径可行（a_err_eof 11.34x）。本设计是**首次正常路径**（非错误路径）的 unsafe raw FFI，建议沉淀 ADR-021（详见 §12 D-2 决策记录需求）。

#### 2.2.4 备选方案（如 PM 拒绝 unsafe）

若 PM 决策不接受 utf16/32 的 unsafe raw FFI，**降级方案**：
- 引入 `encoding_rs` crate（路线 Y）
- utf16/32 性能预期从 ≥5x 下调到 ≥3x（Rust String 中转 + 二次拷贝开销）
- 此时 utf16/32 场景可能不达 ≥4x 总门禁，但 utf8/ascii（>90% 用户场景）仍 ≥10x

ARCH 不推荐降级（牺牲性能 + 引入 crate），但列入备选供 PM 决策。

---

## 3. 7 个构造器详细设计

> 每个 Node 的伪代码用 Rust 签名 + 关键逻辑说明。完整 impl 由 DEV 实现。
> 所有 Node 实现 `super::Construct` trait 的 `parse` / `build` / `sizeof` 三方法。
>
> **共同点**（避免每 Node 重复说明）：
> - 所有 Node 是 `#[derive(Debug, Clone)]` 结构体（除 PascalString/NullTerminated/NullStripped 持 `Box<Node>` 不 `Copy`）
> - 所有 Node 加入 `Node` enum（§4.1）+ `has_expressions()` 扩展（§4.2）
> - 所有 String Node（返回 `str`）调 `Encoding::decode/encode`（§2.2）
> - 错误路径走 `ConstructError::push_path_segment` 重建（与现有 Node 一致）

### 3.1 CStringNode（CString）

#### 3.1.1 Python 原版（参考）

```python
# core.py:1811-1834
def CString(encoding):
    return StringEncoded(NullTerminated(GreedyBytes, term=encodingunit(encoding)), encoding)
```

CString = "读到 term 字节（默认 encoding 的 unit_size 个 0x00）→ 剥离 term → 解码"。
Python 实现是 macro 嵌套（NullTerminated + StringEncoded）。

#### 3.1.2 Rust 独立 Node

```rust
// nodes/strings/c_string.rs

/// C 风格 null 终止字符串节点。
///
/// 对应 Python construct `CString(encoding)`（core.py:1811）。
///
/// # parse 行为
///
/// 1. 从流中逐 unit_size 字节扫描，直到遇到 `term` 字节串
/// 2. 消费 term（不剥离 = 默认 include=False；用户传 include=True 时保留）
/// 3. 剩余字节（不含 term）调 `Encoding::decode` → PyString
/// 4. EOF 前未遇到 term：
///    - `require=True`（默认）→ `ConstructError::Stream`（"stream read less than specified"）
///    - `require=False` → 读到 EOF 的所有字节解码（无 term）
///
/// # build 行为
///
/// 1. 校验输入是 str → `Encoding::encode` → 字节序列
/// 2. 写字节序列 + 写 term（encoding.default_term()）
///
/// # sizeof
///
/// 永远 `Err`（字符串长度 + term 长度运行时未知）。
#[derive(Debug, Clone)]
pub struct CStringNode {
    /// 编码（编译期从用户字符串解析为 enum）。
    encoding: Encoding,
    /// 终止符字节串（默认 = encoding.default_term()，即 unit_size 个 0x00）。
    /// 用户可通过 `CString(encoding, term=b"...")` 显式传入（Python 兼容）。
    term: Vec<u8>,
    /// 是否将 term 包含在解码数据中（Python `NullTerminated(include=...)`，默认 false）。
    include: bool,
    /// 是否在 EOF 时报错（Python `NullTerminated(require=...)`，默认 true）。
    require: bool,
}

impl CStringNode {
    /// 默认构造：`CString(encoding)`，term = encoding 单元的全零字节串。
    pub fn new(encoding: Encoding) -> Self {
        Self {
            encoding,
            term: encoding.default_term(),
            include: false,
            require: true,
        }
    }

    /// 全参数构造（Descriptor 编译期调用，对应 Python `NullTerminated(..., include, consume, require)`）。
    pub fn with_options(encoding: Encoding, term: Vec<u8>, include: bool, require: bool) -> Self {
        Self { encoding, term, include, require }
    }

    pub fn encoding(&self) -> Encoding { self.encoding }
    pub fn term(&self) -> &[u8] { &self.term }
}
```

#### 3.1.3 parse 伪代码

```rust
fn parse<'py>(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
    let unit = self.term.len();  // term 长度 = unit_size
    if unit == 0 {
        return Err(ConstructError::Padding {
            message: "CString term must be at least 1 byte".to_string(),
            path: path.to_string(),
        });
    }
    let start = stream.tell();
    let mut accumulated: Vec<u8> = Vec::new();
    loop {
        match stream.read(unit, path) {
            Ok(chunk) => {
                if chunk == self.term.as_slice() {
                    if self.include {
                        accumulated.extend_from_slice(chunk);
                    }
                    break;  // 找到 term
                }
                accumulated.extend_from_slice(chunk);
            }
            Err(ConstructError::Stream { .. }) if !self.require => {
                // EOF 且 require=False：用已累积数据
                break;
            }
            Err(e) => return Err(e),  // 其他错误（含 require=True 时 EOF）
        }
    }
    // 直接 decode，无 String 中转（utf8/ascii）或 raw FFI（utf16/32）
    let py_str = self.encoding.decode(py, &accumulated, path)?;
    Ok(py_str.into_any().unbind())
}
```

#### 3.1.4 build 伪代码

```rust
fn build(&self, py, obj, stream, ctx, path) -> Result<(), ConstructError> {
    let data = self.encoding.encode(py, obj, path)?;  // 非 str 报 StringError
    stream.write(&data);
    stream.write(&self.term);
    Ok(())
}
```

#### 3.1.5 边界条件

| 场景 | Python 行为 | Rust 行为 |
|------|------------|----------|
| 空字符串 build `""` | `b'\x00'`（utf8） | 同左（encode("") → b"" + term） |
| 流首字节即 term | 解码空字节 → `""` | 同左 |
| EOF 前无 term + require=True | StreamError | ConstructError::Stream |
| EOF 前无 term + require=False | 解码已读字节 | 同左 |
| term 多字节（utf16：`b'\x00\x00'`） | 按 2 字节扫描 | 同左 |
| 数据长度非 unit_size 倍数 | "strange things" (Issue 1046) | 最后不完整单元保留在 accumulated，decode 时报 StringError |

---

### 3.2 NullTerminatedNode（NullTerminated）

#### 3.2.1 Python 原版

```python
# core.py:5050-5115
class NullTerminated(Subconstruct):
    def __init__(self, subcon, term=b"\x00", include=False, consume=True, require=True): ...
    def _parse(self, stream, context, path):
        # 逐 unit 字节读，遇 term 停；按 include/consume 控制 term 处理
        # 然后 subcon._parsereport(BytesIOWithOffsets(data, ...))
    def _build(self, obj, stream, context, path):
        buildret = self.subcon._build(obj, stream, context, path)
        stream_write(stream, self.term, len(self.term), path)
        return buildret
```

**关键**：NullTerminated 持有 `subcon`（任意 Construct），返回 subcon 的解析结果（通常是 `bytes`，因默认 subcon=GreedyBytes）。**不是 String Node**，但归入 Phase 6.2 是因为它是 String 系列的内层组件。

#### 3.2.2 Rust 独立 Node

```rust
// nodes/strings/null_terminated.rs

/// null 终止包装器节点（持有任意 inner subcon）。
///
/// 对应 Python construct `NullTerminated(subcon, term, include, consume, require)`（core.py:5050）。
///
/// # parse 行为
///
/// 1. 逐 unit 字节扫描直到 term（同 CString，但累积的 bytes 用于 inner.parse）
/// 2. 在累积的 bytes 上构造子 ParseStream，调 inner.parse
/// 3. 返回 inner 的结果（通常是 PyBytes，inner=GreedyBytes 时）
///
/// # build 行为
///
/// 1. 调 inner.build(obj) → 字节写入主 stream
/// 2. 写 term 字节串
///
/// # sizeof
///
/// 永远 `Err`（inner 长度 + term 长度未知）。
#[derive(Debug, Clone)]
pub struct NullTerminatedNode {
    /// 内层子构造器（默认 GreedyBytes，但用户可传 Byte / Bytes(n) / Struct 等）。
    inner: Box<crate::nodes::Node>,
    /// 终止符字节串（默认 `b"\x00"`）。
    term: Vec<u8>,
    /// 是否将 term 包含在累积数据中（默认 false）。
    include: bool,
    /// 是否消费 term（true=消费；false=seek 回退 unit 字节，默认 true）。
    consume: bool,
    /// EOF 时是否报错（默认 true）。
    require: bool,
}

impl NullTerminatedNode {
    pub fn new(inner: crate::nodes::Node) -> Self {
        Self {
            inner: Box::new(inner),
            term: vec![0u8],
            include: false,
            consume: true,
            require: true,
        }
    }
    pub fn with_options(inner, term, include, consume, require) -> Self { ... }
    pub fn inner(&self) -> &crate::nodes::Node { &self.inner }
    pub fn has_expressions(&self) -> bool { self.inner.has_expressions() }
}
```

#### 3.2.3 parse 伪代码（与 CString 共享扫描逻辑）

```rust
fn parse<'py>(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
    let unit = self.term.len();
    if unit == 0 { return Err(... PaddingError ...); }
    let mut accumulated: Vec<u8> = Vec::new();
    loop {
        match stream.read(unit, path) {
            Ok(chunk) => {
                if chunk == self.term.as_slice() {
                    if self.include { accumulated.extend_from_slice(chunk); }
                    if !self.consume {
                        stream.seek(stream.tell() - unit, path)?;
                    }
                    break;
                }
                accumulated.extend_from_slice(chunk);
            }
            Err(Stream) if !self.require => break,
            Err(e) => return Err(e),
        }
    }
    // 用 accumulated 构造子流，调 inner.parse
    let mut sub_stream = ParseStream::new(&accumulated);
    self.inner.parse(py, &mut sub_stream, ctx, path)
}
```

#### 3.2.4 build 伪代码

```rust
fn build(&self, py, obj, stream, ctx, path) -> Result<(), ConstructError> {
    self.inner.build(py, obj, stream, ctx, path)?;  // inner 写自己的字节
    stream.write(&self.term);  // 追加 term
    Ok(())
}
```

#### 3.2.5 边界条件

| 场景 | Python 行为 | Rust 行为 |
|------|------------|----------|
| inner 非 GreedyBytes（如 Byte） | 读到 term 后用累积字节流 parse Byte（取首字节） | 同左 |
| include=True | 累积数据含 term | 同左 |
| consume=False | term 不被消费（stream 回退） | `stream.seek(tell - unit)` |
| require=False + EOF | 解析已累积数据 | 同左 |
| inner build 失败 | 错误传播 | 错误传播（带 path） |

---

### 3.3 NullStrippedNode（NullStripped）

#### 3.3.1 Python 原版

```python
# core.py:5123-5178
class NullStripped(Subconstruct):
    def __init__(self, subcon, pad=b"\x00"): ...
    def _parse(self, stream, context, path):
        # 读全部剩余字节 → rstrip(pad) → subcon._parsereport(BytesIOWithOffsets(data, ...))
        data = stream_read_entire(stream, path)
        unit = len(pad)
        if unit == 1:
            data = data.rstrip(pad)
        else:
            # 多字节 pad：处理尾部不完整单元 + 整 unit 剥离
            ...
    def _build(self, obj, stream, context, path):
        return self.subcon._build(obj, stream, context, path)  # 直接转发，不补 pad
```

**关键**：NullStripped 是"读全部 + 右剥离 pad + inner parse"。build 仅转发 inner（不补 pad）——补 pad 由外层 FixedSized 负责（PaddedString 中）。NullStripped 独立使用时 build 不 pad。

#### 3.3.2 Rust 独立 Node

```rust
// nodes/strings/null_stripped.rs

/// null 剥离包装器节点（持有任意 inner subcon）。
///
/// 对应 Python construct `NullStripped(subcon, pad)`（core.py:5123）。
///
/// # parse 行为
///
/// 1. 读流中剩余所有字节（`stream.read_remaining()`）
/// 2. 右剥离 pad 字节串：
///    - unit=1（默认）：`data.rstrip(pad_byte)` 反复剥离末尾匹配字节
///    - unit>1：尾部不完整单元匹配 pad 前缀则剥离 + 整 unit 剥离（对齐 Python 算法）
/// 3. 用剥离后字节构造子流，调 inner.parse
///
/// # build 行为
///
/// 直接调 `inner.build(obj)`，不追加 pad（与 Python NullStripped._build 一致；
/// pad 补全由外层 PaddedStringNode 内联实现，§3.5）。
///
/// # sizeof
///
/// 永远 `Err`（数据 + pad 长度未知）。
#[derive(Debug, Clone)]
pub struct NullStrippedNode {
    /// 内层子构造器。
    inner: Box<crate::nodes::Node>,
    /// pad 字节串（默认 `b"\x00"`）。
    pad: Vec<u8>,
}

impl NullStrippedNode {
    pub fn new(inner: crate::nodes::Node) -> Self {
        Self { inner: Box::new(inner), pad: vec![0u8] }
    }
    pub fn with_pad(inner, pad: Vec<u8>) -> Self { ... }
    pub fn inner(&self) -> &crate::nodes::Node { &self.inner }
    pub fn has_expressions(&self) -> bool { self.inner.has_expressions() }
}
```

#### 3.3.3 parse 伪代码（右剥离算法）

```rust
fn parse<'py>(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
    let data = stream.read_remaining();
    let unit = self.pad.len();
    if unit == 0 {
        return Err(ConstructError::Padding {
            message: "NullStripped pad must be at least 1 byte".to_string(),
            path: path.to_string(),
        });
    }
    let stripped = if unit == 1 {
        // 单字节 pad：rstrip 等价
        let pb = self.pad[0];
        let mut end = data.len();
        while end > 0 && data[end - 1] == pb { end -= 1; }
        &data[..end]
    } else {
        // 多字节 pad：对齐 Python core.py:5159-5165 算法
        let tailunit = data.len() % unit;
        let mut end = data.len();
        if tailunit != 0 && data[data.len() - tailunit..] == self.pad[..tailunit] {
            end -= tailunit;
        }
        while end >= unit && data[end - unit..end] == self.pad.as_slice() {
            end -= unit;
        }
        &data[..end]
    };
    let mut sub_stream = ParseStream::new(stripped);
    self.inner.parse(py, &mut sub_stream, ctx, path)
}
```

#### 3.3.4 build 伪代码

```rust
fn build(&self, py, obj, stream, ctx, path) -> Result<(), ConstructError> {
    // 直接转发 inner（不补 pad——pad 由外层 PaddedStringNode 内联处理）
    self.inner.build(py, obj, stream, ctx, path)
}
```

#### 3.3.5 边界条件

| 场景 | Python 行为 | Rust 行为 |
|------|------------|----------|
| 数据全为 pad | rstrip 后空 → inner.parse(empty) | 同左 |
| 数据尾部不完整 unit（多字节 pad） | 匹配 pad 前缀则剥离 | 对齐 Python core.py:5160-5162 |
| build 不补 pad | 同左 | 同左（外层 PaddedString 补） |
| pad 空字节串 | PaddingError | ConstructError::Padding |

---

### 3.4 GreedyStringNode（GreedyString）

#### 3.4.1 Python 原版

```python
# core.py:1837-1858
def GreedyString(encoding):
    return StringEncoded(GreedyBytes, encoding)
```

GreedyString = "读到 EOF + decode"。最简单的 String Node。

#### 3.4.2 Rust 独立 Node

```rust
// nodes/strings/greedy_string.rs

/// 贪婪字符串节点：读到流结束并解码。
///
/// 对应 Python construct `GreedyString(encoding)`（core.py:1837）。
///
/// # parse 行为
///
/// `stream.read_remaining()` → `Encoding::decode` → PyString。
///
/// # build 行为
///
/// 校验 str → `Encoding::encode` → 写入 stream（不校验长度）。
///
/// # sizeof
///
/// 永远 `Err`（大小未知）。
#[derive(Debug, Clone, Copy)]
pub struct GreedyStringNode {
    encoding: Encoding,
}

impl GreedyStringNode {
    pub fn new(encoding: Encoding) -> Self { Self { encoding } }
    pub fn encoding(&self) -> Encoding { self.encoding }
}
```

#### 3.4.3 实现伪代码（极简）

```rust
fn parse<'py>(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
    let data = stream.read_remaining();
    let py_str = self.encoding.decode(py, data, path)?;
    Ok(py_str.into_any().unbind())
}

fn build(&self, py, obj, stream, ctx, path) -> Result<(), ConstructError> {
    let data = self.encoding.encode(py, obj, path)?;  // 非 str 报 StringError
    stream.write(&data);
    Ok(())
}

fn sizeof(&self, _ctx) -> Result<usize, ConstructError> {
    Err(ConstructError::Generic {
        message: "GreedyString size is undefined".to_string(),
        path: String::new(),
    })
}
```

#### 3.4.4 边界条件

| 场景 | Python 行为 | Rust 行为 |
|------|------------|----------|
| 空流 parse | `""` | 同左（decode(b"") → ""） |
| 非法 UTF-8 字节 | StringError | ConstructError::String |
| build 非 str（如 bytes） | StringError("expected unicode") | ConstructError::String |

---

### 3.5 PaddedStringNode（PaddedString）

> **PM 决策 1 关键落地**：Python 用 `StringEncoded(FixedSized(length, NullStripped(GreedyBytes, pad=...)), encoding)` 三层嵌套。
> 本设计独立实现：PaddedStringNode 内联"固定长度读取 + rstrip pad + decode"，不依赖 FixedSized / NullStripped。

#### 3.5.1 Python 原版

```python
# core.py:1747-1775
def PaddedString(length, encoding):
    return StringEncoded(
        FixedSized(length, NullStripped(GreedyBytes, pad=encodingunit(encoding))),
        encoding,
    )
```

`length` 可以是 `int` 或 `context lambda`（Python `_parse` 调 `evaluate(self.length, context)`）。

#### 3.5.2 Rust 独立 Node

```rust
// nodes/strings/padded_string.rs

use crate::nodes::bytes::BytesLength;  // 复用 Phase 2 已有的 BytesLength（Const + Expr）

/// 固定长度填充字符串节点（独立实现，不依赖 FixedSized / NullStripped）。
///
/// 对应 Python construct `PaddedString(length, encoding)`（core.py:1747）。
///
/// # parse 行为
///
/// 1. 求值 length（常量 / 表达式）
/// 2. 读 length 字节
/// 3. 右剥离 pad（pad = encoding 单元的全零字节串，对齐 Python `encodingunit`）
/// 4. `Encoding::decode` → PyString
///
/// # build 行为
///
/// 1. 校验 str → `Encoding::encode` → 字节序列
/// 2. 若 encoded.len() > length：PaddingError（对齐 Python FixedSized "negative padding"）
/// 3. 写 encoded + 写 (length - encoded.len()) 个 pad 字节
///
/// # sizeof
///
/// - `BytesLength::Const(n)` → `Ok(n)`
/// - `BytesLength::Expr` → `Err`（表达式长度 sizeof 编译期未知，对齐 BytesNode 行为）
#[derive(Debug, Clone)]
pub struct PaddedStringNode {
    /// 总长度（含数据 + pad），可为常量或表达式。
    length: BytesLength,
    /// 编码（编译期从用户字符串解析）。
    encoding: Encoding,
    /// pad 字节串（默认 = encoding.default_term()，即 unit_size 个 0x00）。
    /// 对齐 Python `encodingunit(encoding)` 返回的 bytes。
    pad: Vec<u8>,
}

impl PaddedStringNode {
    pub fn new(length: BytesLength, encoding: Encoding) -> Self {
        Self {
            length,
            pad: encoding.default_term(),  // pad 默认 = encoding unit 全零
            encoding,
        }
    }

    /// 用户显式传 pad 时（Python 兼容，但 PaddedString 原版不暴露 pad 参数）。
    pub fn with_pad(length, encoding, pad: Vec<u8>) -> Self { ... }

    pub fn length(&self) -> &BytesLength { &self.length }
    pub fn encoding(&self) -> Encoding { self.encoding }
    pub fn pad(&self) -> &[u8] { &self.pad }

    /// has_expressions：Expr 长度时返回 true（StructNode 编译期检查用）。
    pub fn has_expressions(&self) -> bool {
        matches!(self.length, BytesLength::Expr(_))
    }
}
```

#### 3.5.3 parse 伪代码

```rust
fn parse<'py>(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
    // 1. 求值 length（复用 BytesNode 的求值逻辑）
    let length = match &self.length {
        BytesLength::Const(n) => *n,
        BytesLength::Expr(prog) => {
            let n = crate::expr::eval_expr_int(prog, ctx, py).map_err(|e| ...)?;
            if n < 0 { return Err(... FieldLength negative ...); }
            n as usize
        }
    };
    // 2. 读 length 字节
    let data = stream.read(length, path)?;
    // 3. 右剥离 pad（复用 NullStripped 的剥离算法，但内联以避免 Subconstruct 层）
    let stripped = rstrip_pad(data, &self.pad);
    // 4. decode
    let py_str = self.encoding.decode(py, stripped, path)?;
    Ok(py_str.into_any().unbind())
}

// 辅助：右剥离（NullStripped 的算法提取为公共 helper）
fn rstrip_pad<'a>(data: &'a [u8], pad: &[u8]) -> &'a [u8] {
    let unit = pad.len();
    if unit == 0 { return data; }
    if unit == 1 {
        let pb = pad[0];
        let mut end = data.len();
        while end > 0 && data[end - 1] == pb { end -= 1; }
        &data[..end]
    } else {
        let tailunit = data.len() % unit;
        let mut end = data.len();
        if tailunit != 0 && data[data.len() - tailunit..] == pad[..tailunit] {
            end -= tailunit;
        }
        while end >= unit && data[end - unit..end] == pad {
            end -= unit;
        }
        &data[..end]
    }
}
```

#### 3.5.4 build 伪代码

```rust
fn build(&self, py, obj, stream, ctx, path) -> Result<(), ConstructError> {
    let length = match &self.length {
        BytesLength::Const(n) => *n,
        BytesLength::Expr(prog) => {
            let n = eval_expr_int(prog, ctx, py).map_err(|e| ...)?;
            if n < 0 { return Err(...); }
            n as usize
        }
    };
    let encoded = self.encoding.encode(py, obj, path)?;  // 非 str 报 StringError
    if encoded.len() > length {
        // 对齐 Python FixedSized._build "subcon build %d bytes but was allowed only %d"
        return Err(ConstructError::Padding {
            message: format!(
                "PaddedString build {} bytes but was allowed only {}",
                encoded.len(), length
            ),
            path: path.to_string(),
        });
    }
    stream.write(&encoded);
    // 补 pad 到 length
    let pad_len = length - encoded.len();
    if pad_len > 0 {
        let mut padding = Vec::with_capacity(pad_len);
        // pad 重复填充（对齐 FixedSized._build: `stream_write(stream, bytes(pad), pad, path)`）
        let unit = self.pad.len();
        for i in 0..pad_len {
            padding.push(self.pad[i % unit]);
        }
        stream.write(&padding);
    }
    Ok(())
}
```

#### 3.5.5 sizeof 伪代码

```rust
fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
    match &self.length {
        BytesLength::Const(n) => Ok(*n),
        BytesLength::Expr(_) => Err(ConstructError::Generic {
            message: "PaddedString with expression length has no static size".to_string(),
            path: String::new(),
        }),
    }
}
```

#### 3.5.6 边界条件

| 场景 | Python 行为 | Rust 行为 |
|------|------------|----------|
| length=0 | parse 读 0 字节 → "" ；build 仅传空 str OK | 同左 |
| encoded.len() == length | 不补 pad | 同左 |
| encoded.len() > length | PaddingError（negative padding） | ConstructError::Padding |
| encoded.len() < length | 右补 pad 到 length | 同左（按 unit 循环填充） |
| length 为表达式求值 | 运行时求值 | `eval_expr_int` |
| length 为负数表达式 | PaddingError | ConstructError::FieldLength |
| pad 多字节（utf16） | rstrip 按 unit 剥离 | `rstrip_pad` 复用 |
| 数据全 pad | rstrip 后空 → "" | 同左 |
| UTF-8 多字节字符被 pad 切半 | 不会发生（Python PaddedString length 是字节数，编码后 padding 前长度已定） | 同左 |

#### 3.5.7 决策记录：length 类型用 BytesLength（决策 B）

**为何不用单独 `usize`**：Python `length` 参数接受 `int` 或 `context lambda`。Phase 2 已实现 `BytesLength`（Const + Expr），完全覆盖 Python 语义。复用避免：
- 重复定义"常量 or 表达式"的枚举
- Descriptor 编译期两条路径（编译 int → Const，编译 AST → Expr）
- StructNode `has_expressions` 检查逻辑分叉

`BytesLength` 是 Phase 2 已 ACCEPTED 的公共类型，PaddedString 复用零成本。

---

### 3.6 PascalStringNode（PascalString）

> **PM 决策 1 关键落地**：Python 用 `StringEncoded(Prefixed(lengthfield, GreedyBytes), encoding)` 嵌套。
> 本设计独立实现：PascalStringNode 内联"lengthfield 解析 + 读 N 字节 + decode"，不依赖 Prefixed（Phase 7 Streams）。

#### 3.6.1 Python 原版

```python
# core.py:1778-1808
def PascalString(lengthfield, encoding):
    return StringEncoded(Prefixed(lengthfield, GreedyBytes), encoding)
```

`lengthfield` 是任意 Construct（VarInt / Int16ub / Byte 等），用于解析"后续数据字节数"。
数据长度是**字节数**（非字符数），Python docstring 明确："Stored length is in bytes, not characters"。

#### 3.6.2 Rust 独立 Node

```rust
// nodes/strings/pascal_string.rs

/// 长度前缀字符串节点（独立实现，不依赖 Prefixed）。
///
/// 对应 Python construct `PascalString(lengthfield, encoding)`（core.py:1778）。
///
/// # parse 行为
///
/// 1. `lengthfield.parse(stream)` → 得到 Python 整数对象
/// 2. extract i64；非整数 → `Range` 错误；负数 → `Range` 错误（对齐 PrefixedArray PA-1/PA-2）
/// 3. 读 N 字节
/// 4. `Encoding::decode` → PyString
///
/// # build 行为
///
/// 1. 校验 str → `Encoding::encode` → 字节序列
/// 2. 取 encoded.len() → build lengthfield（写入长度）
/// 3. 写 encoded 字节
/// （对齐 Python：build 时 lengthfield 自动从数据长度计算，不需用户传长度）
///
/// # sizeof
///
/// 永远 `Err`（lengthfield 大小 + 数据长度，后者未知）。
#[derive(Debug, Clone)]
pub struct PascalStringNode {
    /// 长度字段（任意能产生整数的 Node：VarInt / Int16ub / Byte 等）。
    lengthfield: Box<crate::nodes::Node>,
    /// 编码。
    encoding: Encoding,
}

impl PascalStringNode {
    pub fn new(lengthfield: crate::nodes::Node, encoding: Encoding) -> Self {
        Self { lengthfield: Box::new(lengthfield), encoding }
    }
    pub fn lengthfield(&self) -> &crate::nodes::Node { &self.lengthfield }
    pub fn encoding(&self) -> Encoding { self.encoding }
    pub fn has_expressions(&self) -> bool { self.lengthfield.has_expressions() }
}
```

#### 3.6.3 parse 伪代码

```rust
fn parse<'py>(&self, py, stream, ctx, path) -> Result<Py<PyAny>, ConstructError> {
    // 1. 解析 lengthfield 得到 Python 整数（复用 PrefixedArray 的模式）
    let length_obj = self.lengthfield.parse(py, stream, ctx, path)?;
    let length_i64: i64 = length_obj.bind(py).extract().map_err(|_| {
        ConstructError::Range {
            message: format!(
                "PascalString lengthfield returned non-integer value of type {}",
                length_obj.bind(py).get_type().name().unwrap_or_default()
            ),
            path: path.to_string(),
        }
    })?;
    if length_i64 < 0 {
        return Err(ConstructError::Range {
            message: format!("PascalString length is negative: {}", length_i64),
            path: path.to_string(),
        });
    }
    let length = length_i64 as usize;
    // 2. 读 length 字节
    let data = stream.read(length, path)?;
    // 3. decode
    let py_str = self.encoding.decode(py, data, path)?;
    Ok(py_str.into_any().unbind())
}
```

#### 3.6.4 build 伪代码

```rust
fn build(&self, py, obj, stream, ctx, path) -> Result<(), ConstructError> {
    // 1. encode 数据
    let encoded = self.encoding.encode(py, obj, path)?;  // 非 str 报 StringError
    // 2. build lengthfield（传入 encoded.len() 作为 Python int）
    //    lengthfield 自行检查长度范围（如 Byte 超过 255 报 Range，对齐 PrefixedArray PA-5）
    let len_obj = (encoded.len() as i64).into_py(py);
    self.lengthfield.build(py, len_obj.bind(py), stream, ctx, path)?;
    // 3. 写数据
    stream.write(&encoded);
    Ok(())
}
```

#### 3.6.5 边界条件

| 场景 | Python 行为 | Rust 行为 |
|------|------------|----------|
| length=0 | 读 0 字节 → "" | 同左 |
| lengthfield 解析失败 | 错误传播 | 错误传播（带 path "lengthfield"） |
| lengthfield 返回非整数 | （未明确，通常 int() 转换错误） | `ConstructError::Range`（对齐 PrefixedArray PA-2） |
| length 为负数 | （Python 不可能，因 lengthfield 是无符号通常） | `ConstructError::Range`（PA-1 对齐） |
| encoded.len() 超出 lengthfield 表示范围 | lengthfield.build 报错 | lengthfield.build 报错（PA-5 模式） |
| 流字节不足 | StreamError | `ConstructError::Stream` |
| 非法 UTF-8 字节 | StringError | `ConstructError::String` |

#### 3.6.6 决策记录：lengthfield 用 Box<Node>（不引入 LengthfieldTrait）

**为何不抽 trait**：`lengthfield` 可以是任意产生整数的 Node（FormatField / VarInt / Bytes(1) + Adapter 等）。引入 `LengthfieldTrait` 会导致：
- 每个候选 Node 都 impl 该 trait（侵入式）
- 用户面无法传"任意 Node"（必须 trait object 或 enum 列举）

持有 `Box<Node>` 的方案（与 PrefixedArray.countfield 一致）：
- 用户可传任意 Node（VarInt / Int16ub / Byte / ...）
- 调通用 `Construct::parse` / `Construct::build`
- 运行时 `extract::<i64>` 校验类型（非整数报 Range，对齐 PA-2）
- 符合 §0 #3（输入输出侧无 trait 抽象层）：Node enum 是封闭的静态分派，非 trait object

**§0 合规**：`Box<Node>` 是 enum_dispatch 的标准模式（BitwiseNode / TransformNode / ArrayNode 等已用），无动态分派开销。

---

### 3.7 StringEncoded（Python 侧 macro 工具，不实现为 Rust Node）

#### 3.7.1 为何不实现为 Rust Node

Python `StringEncoded(subcon, encoding)` 是 Adapter 子类（core.py:1711-1744），它：
- 包装任意 subcon（产生 bytes 的 Construct）
- `_decode(obj)` = `obj.decode(encoding)`（bytes → str）
- `_encode(obj)` = `obj.encode(encoding)`（str → bytes）

在 PM 决策 1 方案 A 下，**编解码逻辑已下沉到各 String Node 内部**：
- CStringNode / GreedyStringNode / PaddedStringNode / PascalStringNode 都直接调 `Encoding::decode/encode`
- 不存在"用户需要包装任意 bytes-producing Construct 再 decode"的场景

如果用户确实需要"自定义 bytes 来源 + decode"，可走 Phase 6.3 用户面 Adapter 路径（Python 层继承 Adapter，写 `_decode`/`_encode`）。

#### 3.7.2 Python 侧兼容性别名（保留 API 表面）

```python
# construct-rs/python/construct/_descriptors.py（或 _compat.py）

def StringEncoded(subcon, encoding):
    """兼容性别名（不推荐使用）。
    
    Phase 6.2 起，编解码逻辑已下沉到 CString / GreedyString / PaddedString /
    PascalString 内部。此函数仅为向后兼容保留，行为是：
    1. 校验 encoding 合法
    2. 抛 StringError 提示用户改用具体的 String Node
    """
    raise StringError(
        "StringEncoded is deprecated in construct-rs; use CString / GreedyString / "
        "PaddedString / PascalString directly. For custom bytes->str adapters, "
        "inherit from Adapter (Phase 6.3)."
    )
```

**或者**（更温和的方案）：保留 StringEncoded 作为通用 Adapter 注册的入口，等 Phase 6.3 用户面 Adapter 实现后整合。具体由 PM 决策（见 §12 D-3）。

#### 3.7.3 用户面迁移指引（DEV 文档化责任）

> **⚠️ BREAKING CHANGES（v2 修订，DEV 必须在 Python 侧 docstring 突出标注）**
>
> 从 Python construct 迁移到 construct-rs 的用户，String 相关 API 有两处破坏性变更：
>
> 1. **`StringEncoded(subcon, encoding)` 一调用即抛 `StringError`**（§3.7.2）：
>    Python 原版 `StringEncoded` 是可用的 Adapter 子类；construct-rs 中编解码已下沉到
>    CString/GreedyString/PaddedString/PascalString 内部，StringEncoded 失去存在意义，
>    保留名字仅为兼容性别名（调用即抛错指引迁移）。**迁移指引**：
>    - `StringEncoded(Bytes(4), "utf8")` → 改用 `PaddedString(4, "utf8")`
>    - `StringEncoded(GreedyBytes, "utf8")` → 改用 `GreedyString("utf8")`
>    - 自定义 bytes 来源 + decode → 走 Phase 6.3 用户面 Adapter（继承 `Adapter` 写 `_decode`/`_encode`）
>
> 2. **无后缀 `utf16`/`utf32`/`u16`/`u32` 编码被拒绝**（§2.1.3 P2b 决策）：
>    Python `CString("utf16")` 可用（产出本机序 BOM）；construct-rs 编译期报 `StringError`
>    引导用户用 `utf_16_le`/`utf_16_be`/`utf_32_le`/`utf_32_be`。理由：本机序 BOM 在跨平台
>    下不可移植。**迁移指引**：把字符串字面量从 `"utf16"` 改为 `"utf_16_le"`（或 `_be`）。

DEV 在实施时需在 Python 侧 docstring 注明：
- `construct.StringEncoded` 在 construct-rs 中**不作为常用 API**（Python 原版也标注 "Used internally"），且**一调用即抛 `StringError`**
- `construct.CString` / `GreedyString` / `PaddedString` / `PascalString` 的 `encoding` 参数**不接受无后缀 `utf16`/`utf32`**（必须用 `_le`/`_be` 后缀）
- 用户应直接用 `CString` / `PascalString` 等具体 String 类
- 自定义编解码逻辑用 Adapter 继承（Phase 6.3）

---

### 3.8 错误处理：新增 `ConstructError::String` 变体（决策 A）

#### 3.8.1 Python StringError

```python
# construct/construct/core.py:54
class StringError(ConstructError):
    """Raised when string operations fail (encoding/decoding, unsupported encoding, etc.)."""
```

Python `construct._errors` 已定义 StringError 类（与 StreamError / FieldLengthError 等平级）。

#### 3.8.2 Rust 侧新增变体

```rust
// construct-rs/src/error.rs（修改：在 ConstructError enum 新增变体）

#[derive(Debug, thiserror::Error)]
pub enum ConstructError {
    // ... 现有变体 ...

    /// 字符串错误：编码/解码失败、不支持的编码、非 Unicode 输入等。
    ///
    /// 对应 Python construct 的 `StringError`（core.py:54）。
    ///
    /// 触发场景（Phase 6.2）：
    /// - `Encoding::decode`：非法字节序列（如无效 UTF-8）
    /// - `Encoding::encode`：输入非 str 类型
    /// - `Encoding::from_user_str`：未支持的编码名（编译期，归 Compilation）
    #[error("string error: {message} at {path}")]
    String {
        /// 错误详情。
        message: String,
        /// 错误发生的路径。
        path: String,
    },
}
```

#### 3.8.3 错误映射链路改造

新增 `String` 变体需同步修改 3 处（DEV 实施清单 §10）：

| 文件 | 修改 |
|------|------|
| `error.rs` | 新增 `String { message, path }` 变体 + `message()` / `path()` / `kind_str()` match 分支 |
| `error.rs` `ExceptionClasses` | 新增 `string_error: Py<PyType>` 字段 |
| `error.rs` `init_exception_classes` | 新增 `string_error: get("StringError")?` |
| `error.rs` `select_exception_class` | 新增 `ConstructError::String { .. } => &classes.string_error` 分支 |
| `error.rs` `is_builtin_class` | `builtin_ptrs` 数组加 `self.string_error.as_ptr()`（14 个内置类） |

**注意**：`ExceptionClasses::is_builtin_class` 内的 `[*mut ffi::PyObject; 13]` 改为 `[..; 14]`，否则 O3 fast-path 不覆盖 StringError（用户 `except StringError` 仍能捕获，但走慢路径）。

#### 3.8.4 决策 A 的理由（新增变体 vs 复用 Generic）

| 维度 | 决策 A（新增 String 变体） ✅ | 决策 B（复用 Generic） ❌ |
|------|----------------------------|------------------------|
| **用户面** | `except StringError` 精确捕获（对齐 Python） | 仅能 `except GenericConstructError`（语义模糊） |
| **错误精度** | 字符串错误可独立统计 / 过滤 | 与其他 Generic 混杂 |
| **fast-path 适用** | 可走 O3 fast-path（需加 builtin_ptrs） | 走 Generic fast-path |
| **代码改动量** | 5 处（§3.8.3） | 0 |

**ARCH 推荐 A**。Python construct 用户面 `except StringError` 是常用模式（处理编码失败），Rust 侧应精确映射。改动量小（5 处，~30 行），收益大（用户面兼容 + 错误精度）。

#### 3.8.5 编译期 StringError（编码名不合法）

`Encoding::from_user_str` 编译期失败归 `ConstructError::Compilation`（不是 `String`），原因：
- 编码名在 `__init_subclass__` 时确定（编译期）
- Python `encodingunit(encoding)` 也是在类定义时 `raise StringError`，但实质是配置错误（用户代码错误，非数据错误）
- 编译期错误不携带 path（与现有 Compilation 一致）

DEV 实施时，`Encoding::from_user_str` 返回 `Err(ConstructError::Compilation)`，由 Descriptor 转 `StringError` 抛给用户（保持用户面异常类型）。

---

## 4. 接口签名汇总

### 4.1 Node enum 新增 6 变体

`construct-rs/src/nodes/mod.rs` 修改：

```rust
#[derive(Debug)]
#[enum_dispatch(Construct)]
pub enum Node {
    // ... 现有 20 变体（v2 修订：P7，原 v1 写 24 是错的）...
    //   4 原子：FormatField / Bytes / GreedyBytes / BitsInteger
    //   5 复合：Struct / StructRef / Bitwise / Bytewise / Transform
    //   2 RO  ：Tell / Computed
    //   2 填充：BitPadding / Padding
    //   7 数组：Array / GreedyRange / PrefixedArray / RepeatUntil / Index / StopIf / Element
    // 6.1（Primitives）若先行：可能新增若干变体，届时本设计 §4.1 与实际差 N，
    //      DEV 实施时以当前 `construct-rs/src/nodes/mod.rs` 实测为准

    // === Phase 6.2 Strings（6 个新变体） ===
    /// C 风格 null 终止字符串（Phase 6.2）。
    CString(crate::nodes::strings::CStringNode),
    /// 贪婪字符串：读到 EOF + decode（Phase 6.2）。
    GreedyString(crate::nodes::strings::GreedyStringNode),
    /// 固定长度填充字符串（Phase 6.2）。
    PaddedString(crate::nodes::strings::PaddedStringNode),
    /// 长度前缀字符串（Phase 6.2）。
    PascalString(crate::nodes::strings::PascalStringNode),
    /// null 终止包装器（持有任意 inner，Phase 6.2）。
    NullTerminated(crate::nodes::strings::NullTerminatedNode),
    /// null 剥离包装器（持有任意 inner，Phase 6.2）。
    NullStripped(crate::nodes::strings::NullStrippedNode),
}
```

模块声明（mod.rs 顶部）：

```rust
pub mod strings;  // 新增子模块（nodes/strings/mod.rs）
```

`construct-rs/src/nodes/strings/mod.rs`：

```rust
//! Strings 子模块（Phase 6.2）：6 个 String Node + 公共编码层。
pub mod encoding;
pub mod c_string;
pub mod greedy_string;
pub mod padded_string;
pub mod pascal_string;
pub mod null_terminated;
pub mod null_stripped;

pub use c_string::CStringNode;
pub use greedy_string::GreedyStringNode;
pub use padded_string::PaddedStringNode;
pub use pascal_string::PascalStringNode;
pub use null_terminated::NullTerminatedNode;
pub use null_stripped::NullStrippedNode;
pub use encoding::Encoding;
```

### 4.2 has_expressions 扩展

`Node::has_expressions()` 需新增 6 分支。**只有持有 `Box<Node>` 子树的 Node 需要递归检查**：

```rust
// nodes/mod.rs：Node::has_expressions
pub fn has_expressions(&self) -> bool {
    match self {
        // ... 现有分支 ...
        // Phase 6.2 Strings
        Node::PaddedString(p) => p.has_expressions(),     // BytesLength::Expr 时 true
        Node::PascalString(p) => p.has_expressions(),     // lengthfield 递归
        Node::NullTerminated(n) => n.has_expressions(),   // inner 递归
        Node::NullStripped(n) => n.has_expressions(),     // inner 递归
        // 以下 3 个不含表达式（encoding 是编译期 enum，无 ExprProgram）
        Node::CString(_) | Node::GreedyString(_) => false,
        _ => false,
    }
}
```

### 4.3 compute_ro_value 无需扩展

String Node 都不是 RO 节点（用户必须提供字符串值），无需在 `Node::compute_ro_value` 加分支。若用户误把 String Node 标 `rfield`，走现有 `_` 分支报 Generic 错误（"not a valid RO node"）。

### 4.4 公共 API（Python 侧 Descriptor）

DEV 在 `construct-rs/python/construct/_descriptors.py` 新增 6 个 Descriptor：

```python
class CStringDescriptor(_Descriptor):
    def __init__(self, encoding, *, term=None, include=False, consume=True, require=True):
        self.encoding = encoding
        self.term = term
        # ...
    def _compile(self):
        encoding = _cr.Encoding_from_user_str(self.encoding)  # pyfunction 暴露
        return _cr.make_cstring_node(encoding, term, include, consume, require)

# 类似：GreedyStringDescriptor / PaddedStringDescriptor / PascalStringDescriptor /
#       NullTerminatedDescriptor / NullStrippedDescriptor
```

用户面（`construct-rs/python/construct/__init__.py` 已暴露 `CString` 等名字）：

```python
def CString(encoding):
    return _Field(CStringDescriptor(encoding))

def PaddedString(length, encoding):
    return _Field(PaddedStringDescriptor(length, encoding))

def PascalString(lengthfield, encoding):
    return _Field(PascalStringDescriptor(lengthfield, encoding))

# NullTerminated / NullStripped / GreedyString 同模式
```

---

## 5. 与 Python 版本对应

### 5.1 构造器级映射

| Python 类/函数 | Python 行号 | Rust Node | 行为对齐度 |
|---------------|------------|-----------|----------|
| `CString(encoding)` | core.py:1811 | `CStringNode { encoding, term, include, require }` | ✅ 完全（含 include/consume/require）；⚠️ `encoding` 不接受无后缀 `utf16`/`utf32`（§2.1.3 P2b） |
| `GreedyString(encoding)` | core.py:1837 | `GreedyStringNode { encoding }` | ✅ 完全；⚠️ 同上 |
| `PaddedString(length, encoding)` | core.py:1747 | `PaddedStringNode { length: BytesLength, encoding, pad }` | ✅ 完全（length 支持 int / context lambda）；⚠️ 同上 |
| `PascalString(lengthfield, encoding)` | core.py:1778 | `PascalStringNode { lengthfield: Box<Node>, encoding }` | ✅ 完全；⚠️ 同上 |
| `NullTerminated(subcon, term, include, consume, require)` | core.py:5050 | `NullTerminatedNode { inner, term, include, consume, require }` | ✅ 完全（4 个开关全支持） |
| `NullStripped(subcon, pad)` | core.py:5123 | `NullStrippedNode { inner, pad }` | ✅ 完全 |
| `StringEncoded(subcon, encoding)` | core.py:1711 | （不实现为 Node，Python 侧兼容别名） | ⚠️ **breaking change**：API 保留但一调用即抛 `StringError`（§3.7.2/§3.7.3） |
| `possiblestringencodings` | core.py:1695 | `Encoding::from_user_str` 接受**有后缀**子集 | ⚠️ API 表面差异：拒绝无后缀 `utf16`/`utf_16`/`u16`/`utf32`/`utf_32`/`u32`（§2.1.3 P2b 决策） |
| `encodingunit(encoding)` | core.py:1703 | `Encoding::default_term()` / `Encoding::unit_size()` | ✅ 完全 |
| `StringError` | core.py:54 | `ConstructError::String` | ✅ 完全（新增变体，§3.8） |

### 5.2 方法级行为对齐（核心方法覆盖检查）

按 architect-extension 要求，逐一核对参考实现（Python core.py + String 类）的所有公开方法：

| Python 方法 | Rust 对应 | 状态 |
|------------|----------|------|
| `CString.parse(data)` | `CStringNode::parse` | ✅ §3.1.3 |
| `CString.build(str)` | `CStringNode::build` | ✅ §3.1.4 |
| `CString.sizeof()` | `CStringNode::sizeof` → Err | ✅ §3.1.2 注释 |
| `GreedyString.parse/build/sizeof` | `GreedyStringNode::*` | ✅ §3.4 |
| `PaddedString.parse/build/sizeof` | `PaddedStringNode::*` | ✅ §3.5（sizeof 对 Const 返回 length） |
| `PascalString.parse/build/sizeof` | `PascalStringNode::*` | ✅ §3.6 |
| `NullTerminated.parse/build/sizeof` | `NullTerminatedNode::*` | ✅ §3.2 |
| `NullStripped.parse/build/sizeof` | `NullStrippedNode::*` | ✅ §3.3 |
| `StringEncoded._decode(obj, context, path)` | `Encoding::decode` | ✅ §2.2（直接调，非 Adapter 层） |
| `StringEncoded._encode(obj, context, path)` | `Encoding::encode` | ✅ §2.2 |
| `StringEncoded.__init__(subcon, encoding)` | Python 侧兼容别名 | ✅ §3.7 |
| `NullTerminated.__init__(subcon, term, include, consume, require)` | `CStringNode::with_options` + `NullTerminatedNode::with_options` | ✅ 4 个开关 |
| `NullStripped.__init__(subcon, pad)` | `NullStrippedNode::with_pad` | ✅ |

**覆盖完整性**：所有 Python 公开方法（`__init__` / `parse` / `build` / `sizeof` / `_decode` / `_encode`）均有 Rust 对应，无遗漏。

---

## 6. §0 原则对照表（L-01 对策硬要求）

逐条说明本设计如何满足 `AGENTS.md §0` 八条核心原则：

| §0 原则 | 本设计满足方式 | 证据 |
|---------|--------------|------|
| **#1 一次 FFI** | parse/build 各一次 Python↔Rust 边界穿越。6 个 Node 内部直接操作 stream + PyObject，不跨 FFI | §3 各 Node 伪代码：parse 入参 stream/ctx/path（Rust 内部），返回 PyObject（一次 FFI 出） |
| **#2 无中间表示层** | **parse**：直接从 bytes 构造 PyString（utf8/ascii 走 `std::str::from_utf8` + `PyString::new_bound`；utf16/32 走 `PyUnicode_DecodeUTF16/32` 直接产 PyUnicode）。**build**：utf8/ascii 走 `to_cow()` 转 Rust `Cow<str>`（ASCII 主导场景多为 `Borrowed` 零分配）；**utf16/32 直接 downcast 后持有 `&Bound<PyString>` 调 `AsUTF16/32String` raw FFI**，不经 `Cow<str>` 中转（v2 修订 P1） | §2.2.2 encode/decode 双向：utf8/ascii 安全路径；utf16/32 raw FFI 直接操作 PyObject。对照 `greedy_bytes.rs:79-87` build 零拷贝模式 |
| **#3 输入输出侧无 trait 抽象层** | 6 个 Node 直接实现 `Construct` trait，加入 `Node` enum 静态分派。无新 trait 抽象（Encoding 是数据类型，非 I/O trait） | §4.1 Node enum 新增 6 变体；§2.1 Encoding 是 enum 不是 trait |
| **#4 pyo3 是核心依赖** | 所有 Python 对象操作通过 pyo3（`PyString` / `PyBytes` / `Python<'py>` token）。utf16/32 用 `pyo3::ffi` raw CPython C API | §2.2 `use pyo3::prelude::*` + `pyo3::ffi` |
| **#5 mashumaro 式 API** | 用户面仍用 `@dataclass class X(StructMixin)` + `field: str = CString("utf8")`。String Node 不破坏 StructMixin 用户面 | §4.4 用户面示例 |
| **#6 enum_dispatch 静态分派** | 6 个 Node 加入 `Node` enum，通过 `#[enum_dispatch(Construct)]` 静态分派（与现有 20 变体一致，v2 修订 P7） | §4.1 |
| **#7 Result<T, ConstructError>** | 所有 Node parse/build 返回 `Result<Py<PyAny>, ConstructError>` / `Result<(), ConstructError>`。新增 `String` 变体携带 path | §3.8 |
| **#8 Stream 抽象纯 Rust 内部** | ParseStream / BuildStream 是纯 Rust 抽象，6 个 Node 操作 stream 不跨 FFI。NullTerminated/NullStripped 用 `ParseStream::new(&accumulated)` 构造子流（纯 Rust） | §3.2.3 / §3.3.3 |

**结论**：本设计完全满足 §0 八条原则。无中间表示层（原则 #2）的潜在风险点是 utf16/32 raw FFI，已在 §2.2.3 论证（CPython raw FFI 是唯一不引入 Rust String 中转的方式，且符合 §0 精神）。

---

## 7. 性能假设（L-02/L-05 对策）

> 按 L-02（理论估算替代实证数据）+ L-05（优化 A 路径忽略 B 路径）对策，
> 本节标注**量级估算**，所有数字需在 Phase 6.2 VET 阶段实测验证。
> 性能门禁：**≥4x vs Python construct 2.10.70**（项目总目标，10x 为理想）。

### 7.1 瓶颈识别（Python 原版 slowdown 来源）

> **类比基准（v2 修订，P3）**：项目已有 ≥10x 实测数据可作为 Strings 性能预测的类比锚点：
> - Phase 5 B1-B4 Struct parse **11.64-13.93x**（`harness/MEMORY.md` 性能快照）
> - Phase 3 BitStruct **12x**
> - Phase 4 全场景 ≥10x
> - Phase 4.x a_err_eof 错误路径 unsafe raw FFI **11.34x**（ADR-019 先例，验证热路径 unsafe 可行）
>
> CString/PaddedString/GreedyString 内部结构（单层扫描 + decode）比 Struct（字段序列 + 表达式求值）
> 简单，预测 ≥10x 合理；utf16/32 raw FFI 类比 a_err_eof（同 unsafe CPython C API 路径）。

| 来源 | Python 实现 | Rust 优化 |
|------|------------|----------|
| **三层 Adapter 嵌套** | CString = StringEncoded(NullTerminated(GreedyBytes, ...), encoding) | 单层 CStringNode，内联扫描 + decode |
| **BytesIOWithOffsets 包装** | NullTerminated._parse 构造 substream = BytesIOWithOffsets(data, ...) | `ParseStream::new(&accumulated)` 零成本 |
| **每字节 read 1 次** | NullTerminated._parse: `while True: stream_read(stream, unit, path)` | 单次 `stream.read(unit, path)` 切片 |
| **bytes += 拼接** | `data += b`（每次 O(n) 拷贝） | `Vec::extend_from_slice`（amortized O(1)） |
| **decode 调用开销** | `obj.decode(self.encoding)` 走 Python method dispatch | `Encoding::decode` match 分派（编译期） |
| **Adapter._decode 包装** | StringEncoded._decode 调用 subcon 后再 decode | 单次 decode（编解码下沉到 Node） |

### 7.2 可证伪预测（按场景）

> 以下为**量级估算**（L-02 对策）。实测需在 `bench/bench_strings.py` 跑 Controlled A/B Test。
> **类比来源**列（v2 修订，P3）：引用项目已有实测数据支撑预测合理性。

| 场景 | 编码 | 预测加速比 | 瓶颈分析 | 风险 | 类比来源 |
|------|------|----------|---------|------|---------|
| CString parse（短字符串 ~10 字节） | utf8 | **12-15x** | Python 三层 Adapter 嵌套 + BytesIO 包装；Rust 单层扫描 + std::str::from_utf8 | 低（utf8 标准库优化） | Phase 5 B1 Struct 11.64x（结构更复杂的 Rust Node） |
| CString parse（长字符串 ~1KB） | utf8 | **8-10x** | 数据拷贝主导，嵌套开销被摊薄 | 低 | Phase 5 B2-B4 Struct 11-14x |
| GreedyString parse | utf8 | **15-20x** | 无 term 扫描，纯 read_remaining + decode；Python GreedyBytes + Adapter 双层 | 低 | Phase 1 GreedyBytes 实测（更简单结构） |
| PaddedString parse | utf8 | **10-12x** | + rstrip pad（单次线性扫描）+ decode | 低 | Phase 5 B3 Struct 12x |
| PaddedString build | utf8 | **8-10x** | + pad 填充（Vec::with_capacity 预分配） | 低 | Phase 5 B4 Struct build 12x |
| PascalString parse（VarInt 前缀） | utf8 | **10-12x** | + lengthfield.parse（VarInt ~10x）+ decode | 中（依赖 6.1 VarInt 性能） | Phase 4 PrefixedArray（同 lengthfield 模式） |
| NullTerminated parse（inner=GreedyBytes） | — | **12-15x** | 同 CString 但返回 bytes（无 decode） | 低 | Phase 5 B1 Struct 11.64x |
| NullStripped parse（inner=GreedyBytes） | — | **10-12x** | read_remaining + rstrip + 子流 inner parse | 低 | Phase 4 同类 |
| CString parse | utf16-le | **5-8x** | raw FFI decode（PyUnicode_DecodeUTF16, byteorder=-1）；CPython 内部 native | 中（首次热路径 unsafe） | **ADR-019 a_err_eof 11.34x**（同 unsafe CPython C API 热路径，但 decode 比 error path 重） |
| CString parse | utf32-be | **5-7x** | 同上，utf32 单元 4 字节 | 中 | 同上 |
| CString parse | ascii | **12-15x** | 同 utf8 路径（ascii 是 utf8 子集） | 低 | Phase 5 B1 Struct 11.64x |

### 7.3 关键预测（≥4x 门禁达标性）

| 编码族 | 预测达标性 | 备注 |
|--------|----------|------|
| utf8 / ascii（>90% 用户场景） | ✅ ≥10x | 标准库优化，门禁轻松达标 |
| utf16-le / utf16-be | ⚠️ 5-8x | raw FFI 必要，门禁达标但有风险（实测可能 4-5x） |
| utf32-le / utf32-be | ⚠️ 5-7x | utf32 用户极少，门禁达标但样本小 |

**L-05 对策（优化 A 路径忽略 B 路径）**：
- A 路径（utf8/ascii）：优化重点，预测 ≥10x（>90% 用户场景）
- B 路径（utf16/32）：raw FFI 优化，预测 ≥5x（<10% 用户场景）
- **不能因 A 路径 ≥10x 就忽略 B 路径**——若 utf16/32 实测 <4x，需在 PM 决策时标注"utf16/32 场景豁免"或调整 raw FFI 实现

### 7.4 性能验证计划

| 阶段 | 工作 | 责任 |
|------|------|------|
| DEV 实施完成 | 跑 `bench/bench_strings.py`（utf8/ascii 主要场景） | DEV |
| VET 阶段 | Controlled A/B Test（utf8 + utf16 + utf32 三编码族） | VET |
| Phase 6 验收 | 全部场景填 `docs/perf-scenarios.csv`，≥4x 达标 | PM（依据 VET 数据） |

> **PascalString 性能验证时机（v2 修订，P6）**：PascalString parse 预测 10-12x 依赖 lengthfield
> 候选（VarInt / Int16ub）至少一个**已实现且有实测数据**。当前 6.1 VarInt 同处 DESIGN_REVIEW
> 状态。VET 验证 PascalString 性能时需等：(a) 6.1 VarInt 实现并通过 VET 实测，或 (b) 用 Phase 1
> 已实测的 FormatField（如 Int16ub）作 lengthfield 候选——两者任一满足即可进入 PascalString 性能验证。

---

## 8. 边界条件清单（汇总）

> 各 Node 的逐项边界条件见 §3.1.5 / §3.2.5 / §3.3.5 / §3.4.4 / §3.5.6 / §3.6.5。
> 本节按**类别**汇总，便于 DEV 实施时检查覆盖度。

### 8.1 空输入处理

| 场景 | 涉及 Node | 行为 |
|------|----------|------|
| 流为空（0 字节） | 所有 String Node | CString/NullTerminated + require=True → Stream error；其他 → ""（decode empty bytes） |
| 空字符串 build (`""`) | CString/PaddedString/PascalString/GreedyString | encode("") → b""，按 Node 规则处理（CString 加 term；PaddedString 补 pad 到 length；PascalString 写 lengthfield=0 + 0 字节） |
| lengthfield 解析得到 0 | PascalString | 读 0 字节 → decode(b"") → "" |
| PaddedString length=0 | PaddedString | parse 读 0 字节 → ""；build 仅接受空 str（encoded.len() ≤ 0） |

### 8.2 最大/最小值处理

| 场景 | 涉及 Node | 行为 |
|------|----------|------|
| PascalString length = i64::MAX | PascalString | stream.read 会失败（Stream error，bytes 不足） |
| PaddedString length = usize::MAX | PaddedString | 同上 |
| UTF-8 字符串超长（>1GB） | 所有 String Node | Vec 自动扩容；理论无上限（实际受内存限制） |
| lengthfield 返回负数 | PascalString | Range error（PA-1 对齐） |

### 8.3 溢出处理

| 场景 | 涉及 Node | 行为 |
|------|----------|------|
| PascalString lengthfield 超出 usize 表示（i64 → usize） | PascalString | 64 位平台无溢出；32 位平台 i64 → usize as usize 截断（理论问题，实际协议不会出现） |
| PaddedString encoded.len() > length | PaddedString | Padding error（§3.5.4） |
| PascalString encoded.len() 超出 lengthfield 表示范围 | PascalString | lengthfield.build 报错（如 Byte 超过 255） |

### 8.4 默认值行为

| 参数 | 默认值 | 来源 |
|------|--------|------|
| `CString.term` | encoding 单元的 0x00 字节串 | Python `encodingunit(encoding)` |
| `CString.include` | false | Python `NullTerminated(include=False)` |
| `CString.require` | true | Python `NullTerminated(require=True)` |
| `NullTerminated.term` | `b"\x00"` | Python `NullTerminated.__init__` |
| `NullTerminated.consume` | true | Python `NullTerminated.__init__` |
| `NullStripped.pad` | `b"\x00"` | Python `NullStripped.__init__` |
| `PaddedString.pad` | encoding 单元的 0x00 字节串 | Python `encodingunit(encoding)` |

### 8.5 错误条件

| 错误类型 | 触发条件 | Python 异常 | Rust 变体 |
|---------|---------|------------|----------|
| 编码名不合法 | `Encoding::from_user_str` 未识别 | StringError | Compilation（编译期） |
| 解码失败（非法字节） | `Encoding::decode` 验证失败 | StringError | String（§3.8） |
| 编码失败（非 str 输入） | `Encoding::encode` downcast 失败 | StringError("expected unicode") | String |
| 流字节不足 | stream.read(n) 失败 | StreamError | Stream |
| term 找不到（require=True） | CString/NullTerminated EOF 无 term | StreamError | Stream |
| term/pad 空字节串 | unit_size == 0 | PaddingError | Padding |
| length 负数（PaddedString/PascalString） | 表达式求值 < 0 | PaddingError/RangeError | FieldLength / Range |
| encoded 超出 length（PaddedString build） | encoded.len() > length | PaddingError | Padding |

---

## 9. 与其他模块的交互

### 9.1 依赖（本模块需要）

| 依赖项 | 来源 | 用途 | Phase |
|--------|------|------|-------|
| `crate::nodes::Node` enum + `Construct` trait | nodes/mod.rs | 6 个 String Node 加入 enum | Phase 1 |
| `crate::nodes::bytes::BytesLength` | nodes/bytes.rs | PaddedString.length 类型（Const + Expr） | Phase 2 |
| `crate::stream::{ParseStream, BuildStream}` | stream.rs | 字节读写 | Phase 1 |
| `crate::expr::eval_expr_int` | expr.rs | PaddedString 表达式长度求值 | Phase 2 |
| `crate::context::Context` | context.rs | 表达式求值的 context 访问 | Phase 1 |
| `crate::error::ConstructError` | error.rs | 错误返回（新增 String 变体） | Phase 1（修改） |
| `pyo3::ffi`（CPython C API） | pyo3 | utf16/32 raw FFI 解码 | Phase 1（既有） |
| **VarIntNode**（推荐 lengthfield） | nodes/varint.rs（6.1 新增） | PascalString 推荐用法 | Phase 6.1（**软依赖**，6.2 可独立） |

**软依赖说明**：PascalString 的 lengthfield 是用户传入的任意 Node。推荐用 6.1 的 VarInt（protobuf-like 协议常用），但 PascalString 本身不硬依赖 VarInt——用户可传 Phase 1 的 `Byte` / `Int16ub` 等。6.2 可与 6.1 并行（与总纲 §子任务依赖图一致）。

### 9.2 被依赖（未来模块需要本模块）

| 被依赖项 | 用途 | 计划 Phase |
|---------|------|-----------|
| CString / PascalString / PaddedString | 协议格式常用（用户直接用） | Phase 6.2 即交付 |
| NullTerminated / NullStripped | 用户自定义 Adapter 内层 | Phase 6.3+ |
| `Encoding` enum + decode/encode helper | 未来 BitPackedString / 自定义 String Adapter | Phase 7+（如有） |
| `ConstructError::String` 变体 | 未来需要字符串错误的地方 | Phase 6.2 即引入 |

### 9.3 跨阶段决策检查

按 architect-extension §跨阶段决策检查，已对照：
- **ADR-001 ~ ADR-020**：无决策被本设计违反。特别对照：
  - ADR-016（P0-3 lazy path 错误传播）：6 个 String Node 错误路径走 `push_path_segment`，成功路径零成本（与现有 Node 一致）——✅
  - ADR-017（Vec 中转 PyList 构建）：String Node 不构造 PyList（返回单个 PyString），不适用——N/A
  - ADR-019（错误抛出 fast-path）：本设计是**首次正常路径** unsafe raw FFI（utf16/32），建议沉淀 **ADR-021**（§12 D-2 决策记录需求）——⚠️ 待 PM 决策

- **L-01 ~ L-12 教训**：
  - L-01（中间表示层）：本设计 utf8/ascii 不引入 Rust String 中转；utf16/32 用 raw FFI 避免 String 中转——✅
  - L-02（理论估算替代实证）：§7 全部预测标注"量级估算"+ VET 验证计划——✅
  - L-05（优化 A 忽略 B）：§7.3 明确 utf8/ascii 与 utf16/32 双路径分析——✅
  - L-09（跨时段性能对比消除法）：VET 用 Controlled A/B Test——✅
  - L-12（PM 角色越界）：本设计由 ARCH 完成，PM 仅决策——✅

---

## 10. DEV 实施清单

> 本节为 DEV 实施时的逐步指引（非代码本身）。每步需通过 `cargo build` + `cargo clippy`（零 warning）+ `cargo fmt --check` + `cargo test`。

### 10.1 Rust 侧实施顺序（建议）

| 步骤 | 文件 | 工作内容 | 验证 |
|------|------|---------|------|
| **R1** | `src/error.rs` | 新增 `ConstructError::String { message, path }` 变体 + 改造 `ExceptionClasses`（14 个内置类）+ `select_exception_class` + `is_builtin_class` | `cargo test error::tests` 全 PASS |
| **R2** | `src/nodes/strings/mod.rs` + `encoding.rs` | 建立 strings 子模块；实现 `Encoding` enum + `from_user_str`（v2 含 utf16/utf32 拒绝分支）+ `unit_size` + `default_term` | unit test 覆盖有后缀别名（utf_16_le/be 等）+ 拒绝无后缀（utf16/utf_16/u16/utf32/utf_32/u32） |
| **R3** | `encoding.rs`（decode/encode utf8/ascii） | 实现 utf8/ascii 的 `decode`/`encode`（std + pyo3 路径） | unit test：合法/非法字节序列 |
| **R3.5**（v2 新增，P5） | `docs/decisions/ADR-021-strings-utf16-32-raw-ffi.md`（**编号待 PM 确认**，见下方注） | **ARCH 沉淀 ADR**（首次正常路径 unsafe raw FFI 决策 + SAFETY 规范），作为 R4 前置。内容：引用 ADR-019（错误路径先例）+ 列出本设计 utf16/32 双向 raw FFI 的 SAFETY 前置条件 + BC6 回退 + byteorder=-1/+1 语义 + 本机序 byte-swap 处理 | PM 审核并入 `docs/decisions/README.md` 索引 |

> **⚠️ ADR 编号冲突（待 PM 决策）**：`docs/decisions/` 当前实际占用 ADR-001~ADR-020，ADR-021 是下一个可用编号。但 `docs/design/模块设计/模块设计-Adapter核心.md` §12（Phase 6.3 Adapter，同处 DESIGNING）已预留 ADR-021 给"用户面 Adapter Python 层化 + 最小钩子"。两个设计都想用 ADR-021。
>
> ARCH 不擅自决定编号归属，提请 PM 决策：
> - **方案 A**：本设计（6.2 Strings）用 **ADR-021**（先到先得，6.2 在 6.3 前验收），6.3 Adapter 改用 ADR-022。
> - **方案 B**：本设计直接用 **ADR-022**，6.3 Adapter 保留 ADR-021。
>
> ARCH 推荐方案 A（按验收顺序，避免后到者占用前编号）。PM 决策后 DEV 在 R3.5 实施时使用最终编号。
| **R4** | `encoding.rs`（utf16/32 raw FFI） | 实现 `decode_utf16_32_raw`（byteorder=-1/+1，v2 P2a）+ `encode_utf16_32_raw`（接收 `&Bound<PyString>`，v2 P1；含本机序 byte-swap）；unsafe 块附 SAFETY 注释，按 ADR-019 + **ADR-021** 模板 | unit test + unsafe 审查点（REV 重点）+ 本机序/目标序交叉测试 |
| **R5** | `greedy_string.rs` | 实现 `GreedyStringNode`（最简单，先做） | unit test：空流 / 非法 UTF-8 / 非 str build |
| **R6** | `c_string.rs` | 实现 `CStringNode`（扫描 + decode） | unit test：include/consume/require 4 开关组合 |
| **R7** | `null_terminated.rs` | 实现 `NullTerminatedNode`（持有 inner） | unit test：inner=GreedyBytes / inner=Byte |
| **R8** | `null_stripped.rs` | 实现 `NullStrippedNode`（rstrip 算法） | unit test：单字节 pad / 多字节 pad |
| **R9** | `padded_string.rs` | 实现 `PaddedStringNode`（BytesLength 复用 + rstrip_pad helper） | unit test：Const + Expr length；encoded > length PaddingError |
| **R10** | `pascal_string.rs` | 实现 `PascalStringNode`（lengthfield: Box<Node>） | unit test：lengthfield=FormatField；负数 Range；非整数 Range |
| **R11** | `nodes/mod.rs` | Node enum 新增 6 变体（v2：基线 20 + 6 = 26，6.1 若先行则再增） + `has_expressions` 扩展 + `pub mod strings` | `cargo build` 编译通过 |
| **R12** | 公共 helper `rstrip_pad` | 提取到 strings/mod.rs 或 encoding.rs（NullStripped + PaddedString 共享） | unit test 复用 |

### 10.2 Python 侧实施顺序

| 步骤 | 文件 | 工作内容 |
|------|------|---------|
| **P1** | `python/construct/_descriptors.py` | 新增 6 个 Descriptor（CStringDescriptor 等）+ `_compile` 调 Rust pyfunction。`Encoding_from_user_str` 在编译期被调，无后缀编码此处即抛 `StringError`（v2 P2b） |
| **P2** | `src/lib.rs`（pyfunction 注册） | 暴露 `make_cstring_node` / `make_greedy_string_node` / ... 6 个 pyfunction + `Encoding_from_user_str` |
| **P3** | `python/construct/__init__.py` | （无需改）CString/PascalString/... 已暴露；只需保证 Descriptor 路径正确 |
| **P4** | `python/construct/_compat.py`（新建或改 _descriptors.py） | StringEncoded 兼容别名（§3.7.2，一调用即抛 `StringError`） |
| **P5**（v2 新增，P4） | 全部 String 类的 docstring | 突出标注 breaking changes：(a) StringEncoded 一调用即抛错；(b) encoding 不接受无后缀 utf16/utf32（§3.7.3） |

### 10.3 测试实施顺序

| 步骤 | 文件 | 工作内容 |
|------|------|---------|
| **T1** | `tests/unit/test_strings_unit.py` | Rust 内部 unit 测试（参考 bytes.rs 风格） |
| **T2** | `tests/_helpers/encoding_parity.py` | 填充 6.0 占位模块：跨编码比对 helper |
| **T3** | `tests/parity/test_phase6_strings_parity.py` | parity 测试（§11 模板） |
| **T4** | `tests/errors/test_strings_errors.py` | 错误路径（编码失败 / 长度错误 / EOF） |
| **T5** | `bench/bench_strings.py` | 性能基线（utf8/ascii + utf16/utf32 三编码族） |

### 10.4 实施风险点（REV 重点审查）

| 风险 | 位置 | 缓解 |
|------|------|------|
| **utf16/32 unsafe raw FFI（双向）** | encoding.rs R4 | 按 ADR-019 + **ADR-021（R3.5 沉淀）** SAFETY 注释模板；BC6 回退路径；**v2 P1+P2a 重点**：encode 不经 Cow<str>、decode byteorder=-1/+1（不是 0）、encode 需本机序 byte-swap |
| **`is_builtin_class` 数组扩容** | error.rs R1 | `[13]` → `[14]`，遗漏会导致 StringError 不走 fast-path |
| **多字节 pad rstrip 算法** | null_stripped.rs R8 + padded_string.rs R9 | 对齐 Python core.py:5159-5165（尾部不完整单元 + 整 unit 剥离）；提取公共 helper |
| **PaddedString build 的 pad 循环填充** | padded_string.rs R9 | `for i in 0..pad_len { padding.push(self.pad[i % unit]) }`，多字节 pad 时正确循环 |
| **PascalString lengthfield 错误 path** | pascal_string.rs R10 | `lengthfield.parse` 失败时 push_path_segment("lengthfield")（对齐 PrefixedArray） |
| **CString/NullTerminated 扫描的 EOF 处理** | c_string.rs R6 / null_terminated.rs R7 | require=True 时 EOF → Stream；require=False 时用已累积数据 |
| **v2 P2b：无后缀编码编译期拒绝**（新增） | encoding.rs R2 + Descriptor P1 | unit test 覆盖 `from_user_str("utf16")`/`("u32")` 等返回 Err；错误信息含引导文本 |
| **v2 P1：encode 本机序 byte-swap 正确性**（新增） | encoding.rs R4 | cfg(target_endian) + 目标序交叉测试（LE 平台测 Utf16Be / BE 平台测 Utf16Le） |

---

## 11. parity 测试模板

> 基于 6.0 已 ACCEPTED 的 `_helpers/parity.py` API 契约。
> DEV 实施时按本模板填充 `tests/parity/test_phase6_strings_parity.py`。

### 11.1 case 定义结构

按 `_helpers/parity.py` 模块 docstring 约定，case_definitions 字符串必须定义：
- `_make_case_rs(case_id)` → `(cls, parse_data, build_factory, extract)`
- `_make_case_py(case_id)` → `(fmt, parse_data, build_input)`

### 11.2 Phase 6.2 case 矩阵（最小集，DEV 可扩展）

> **v2 修订**：补 P2b 编译期拒绝 case + utf16-le 解码 byteorder=-1 正确性 case。
> 注意：v1 的 `CS-utf16-1` 已用 `utf_16_le`（无后缀），符合 P2b 决策。

| case_id | 描述 | 编码 | 类型 |
|---------|------|------|------|
| `CS-utf8-1` | CString utf8 短字符串 | utf8 | CString |
| `CS-utf8-2` | CString utf8 多字节字符（中文/俄文） | utf8 | CString |
| `CS-utf16-1` | CString utf16-le | utf_16_le | CString |
| `CS-utf16-be-1` | CString utf16-be（v2 新增，验证 byte-swap） | utf_16_be | CString |
| `CS-utf16-le-fffe-data` | CString utf16-le 解码含 `\xff\xfe` 字节流（v2 新增，验证 byteorder=-1 不误判 BOM） | utf_16_le | CString |
| `CS-utf32-1` | CString utf32-le（v2 新增） | utf_32_le | CString |
| `CS-ascii-1` | CString ascii | ascii | CString |
| `CS-eof-require-true` | CString EOF + require=True（应 Stream error） | utf8 | CString |
| `CS-reject-utf16-bare`（v2 新增） | `CString("utf16")` 编译期拒绝（应 StringError，含引导文本） | utf16（无后缀） | CString |
| `CS-reject-utf32-bare`（v2 新增） | `CString("utf32")` 编译期拒绝 | utf32（无后缀） | CString |
| `GS-utf8-1` | GreedyString utf8 | utf8 | GreedyString |
| `GS-utf8-empty` | GreedyString 空流 | utf8 | GreedyString |
| `PS-utf8-1` | PaddedString utf8 length=10 | utf8 | PaddedString |
| `PS-utf8-pad` | PaddedString encoded < length（右补 pad） | utf8 | PaddedString |
| `PS-utf8-overflow` | PaddedString encoded > length（PaddingError） | utf8 | PaddedString |
| `PS-expr-length` | PaddedString length=表达式 | utf8 | PaddedString |
| `PASC-utf8-varint` | PascalString + VarInt lengthfield | utf8 | PascalString |
| `PASC-utf8-int16ub` | PascalString + Int16ub lengthfield | utf8 | PascalString |
| `NT-bytes-default` | NullTerminated(GreedyBytes) 默认 term | — | NullTerminated |
| `NT-include-true` | NullTerminated include=True | — | NullTerminated |
| `NT-consume-false` | NullTerminated consume=False | — | NullTerminated |
| `NS-bytes-default` | NullStripped(GreedyBytes) 默认 pad | — | NullStripped |
| `NS-utf16-pad` | NullStripped 多字节 pad（utf16） | — | NullStripped |
| `ERR-non-str-build` | build 传非 str（StringError） | utf8 | CString |
| `ERR-invalid-utf8` | parse 非法 UTF-8（StringError） | utf8 | GreedyString |
| `ERR-stringencoded-call`（v2 新增） | `StringEncoded(Bytes(4), "utf8")` 一调用即抛 StringError | utf8 | （兼容别名） |

### 11.3 case 定义示例（CS-utf8-1）

```python
# tests/parity/test_phase6_strings_parity.py

CASE_DEFINITIONS = """
def _make_case_rs(case_id):
    from construct import CString
    if case_id == "CS-utf8-1":
        cls = CString("utf8")
        parse_data = b'hello\\x00'
        build_factory = lambda: cls("hello")
        extract = lambda obj: obj  # CString 返回 str，直接比对
        return cls, parse_data, build_factory, extract
    # ... 其他 case

def _make_case_py(case_id):
    from construct import CString
    if case_id == "CS-utf8-1":
        fmt = CString("utf8")
        parse_data = b'hello\\x00'
        build_input = u"hello"
        return fmt, parse_data, build_input
    # ... 其他 case
"""

ALL_CASES = [
    ("CS-utf8-1", "CString utf8 short"),
    ("CS-utf8-2", "CString utf8 multibyte"),
    # ... 完整列表
]

@pytest.fixture(scope="module")
def parity_results(venv_pair):
    return make_parity_results_fixture(ALL_CASES, CASE_DEFINITIONS)(venv_pair)

@pytest.mark.parametrize("case_id,desc", ALL_CASES)
def test_strings_parity(case_id, desc, parity_results):
    rs = parity_results[case_id]["rs"]
    py = parity_results[case_id]["py"]
    assert_parity(rs, py, case_id, desc=desc)
```

### 11.4 错误路径 case 处理

错误 case（如 `ERR-non-str-build` / `ERR-invalid-utf8`）需在 case_definitions 中用 try/except 包装，捕获异常类型字符串作为比对对象（normalize 后是 `{"__exception__": "StringError", "message": "..."}`）。具体 helper 由 DEV 在 `encoding_parity.py` 中实现。

### 11.5 性能验证（独立于 parity）

`bench/bench_strings.py` 用 6.0 已 ACCEPTED 的 `bench/_helpers/runner.py` 跑 Controlled A/B Test：
- 三编码族（utf8 / utf16-le / utf32-be）× 6 个 Node = 18 个测量点
- 填入 `docs/perf-scenarios.csv`（PM 主维护）
- ≥4x 门禁达标性见 §7.3

---

## 12. PM 决策点汇总

> 本设计共 4 个 PM 决策点。ARCH 推荐 + 影响范围 + 备选方案如下。

### D-1：编码表示 = 编译期 `Encoding` enum（决策 A）

| 项 | 内容 |
|----|------|
| **决策** | 编码用编译期 `Encoding` enum（6 变体）表示，运行时零字符串匹配 |
| **ARCH 推荐** | ✅ 决策 A |
| **理由** | 编码名是用户类定义时的静态信息；Python `possiblestringencodings` 是封闭集合（6 类）；编译期解析最自然 |
| **备选** | 决策 B（运行期字符串匹配）——不推荐，每次 parse 多 20-50ns 字符串哈希 |
| **影响范围** | §2.1 Encoding enum 定义；所有 6 个 String Node 持有 `encoding: Encoding` 字段 |
| **优先级** | **P0**（设计基础，不接受 B 则需重新设计 §2） |

### D-2：编解码实现 = 混合方案（utf8/ascii 安全路径 + utf16/32 raw FFI）

| 项 | 内容 |
|----|------|
| **决策** | utf8/ascii 用 `std::str::from_utf8` + `PyString::new_bound`（安全）；utf16/32 用 CPython `PyUnicode_DecodeUTF16/32` raw FFI（unsafe） |
| **ARCH 推荐** | ✅ 混合方案（Z + X） |
| **理由** | utf16/32 在 Rust 标准库无原生支持；raw FFI 是唯一不引入 Rust String 中转的方式（§0 #2 合规）；unsafe 范围限定（仅 4 个变体）；ADR-019 已验证热路径 unsafe 可行 |
| **备选 1** | 全部用 encoding_rs crate（路线 Y）——引入新依赖 + Rust String 中转（违反 §0 #2 精神）；utf16/32 性能从 ≥5x 降到 ≥3x |
| **备选 2** | 全部用 CPython raw FFI（路线 X）——utf8/ascii 也 unsafe，但 std 路径已是最优，不必 unsafe |
| **影响范围** | §2.2 decode/encode 实现；ADR-021 沉淀需求（首次正常路径 unsafe raw FFI） |
| **风险** | utf16/32 实测可能 4-5x（接近门禁），需 VET Controlled A/B Test 验证 |
| **优先级** | **P0**（影响 §0 合规性 + 性能门禁达标性） |

**ADR-021 沉淀建议**（如 PM 接受 D-2）：
- 标题：`ADR-021 Strings UTF-16/32 编解码 unsafe raw FFI`
- 内容：记录首次正常路径（非错误路径）unsafe raw FFI 的决策；引用 ADR-019（错误路径先例）；列出 SAFETY 前置条件 + BC6 回退
- 由 ARCH 在 PM 决策后写入 `docs/decisions/`

### D-3：StringEncoded 处理方式

| 项 | 内容 |
|----|------|
| **决策** | StringEncoded 不实现为 Rust Node；Python 侧保留兼容别名 |
| **ARCH 推荐** | ✅ 抛 StringError 提示用户改用具体 String Node |
| **备选** | 保留 StringEncoded 作为通用 Adapter 入口（待 Phase 6.3 用户面 Adapter 实现后整合） |
| **理由** | PM 决策 1 方案 A 下，编解码已下沉到各 String Node；StringEncoded 失去存在意义；Python 原版也标注 "Used internally" |
| **影响范围** | §3.7；Python 侧 `_compat.py` 或 `_descriptors.py` 新增兼容别名 |
| **优先级** | **P1**（不影响 6.2 启动，可在 DEV 实施时决定具体形态） |

### D-4：StringError 错误变体 = 新增 `ConstructError::String`

| 项 | 内容 |
|----|------|
| **决策** | 新增 `ConstructError::String { message, path }` 变体 + 注册 StringError 类 |
| **ARCH 推荐** | ✅ 决策 A（新增变体） |
| **备选** | 决策 B（复用 Generic）——用户面失去 `except StringError` 精确捕获 |
| **理由** | Python 用户面 `except StringError` 是常用模式；改动量小（5 处，~30 行）；收益大（用户面兼容 + 错误精度） |
| **影响范围** | §3.8；error.rs 5 处修改；`is_builtin_class` 数组 `[13]` → `[14]` |
| **优先级** | **P0**（影响用户面异常兼容性） |

### 12.1 额外提示（不需 PM 决策但需知会）

| 项 | 内容 |
|----|------|
| **inventory.csv 更新** | 7 个 Strings 行已存在（PM 之前维护），notes 标 "Phase 5+" 过时。建议 PM 在 ACCEPTED 时更新为 "Phase 6.2，设计见 docs/design/模块设计/模块设计-Strings.md"（ARCH 按 extension 规定不修改已有行） |
| **6.0 测试框架兼容** | `_helpers/encoding_parity.py` 是 6.0 建立的占位模块（10 行），本设计 §11 parity 模板依赖其填充。DEV 实施时填充该模块 |
| **VarInt 软依赖** | PascalString 推荐 lengthfield = VarInt（6.1）。6.2 可与 6.1 并行；若 6.1 未完成，6.2 测试用 Byte / Int16ub 作 lengthfield |
| **Node enum 扩容**（v2 修订，P7） | 现有 **20** 变体（v1 误写 24，6.2-REV 实测 `construct-rs/src/nodes/mod.rs` 为 20）+ 本设计 6 = **26** 变体（若 6.1 先行验收则再增）。enum_dispatch 静态分派无性能影响（match 编译期展开） |
| **ADR-021 沉淀**（v2 新增，P5） | R3.5 由 ARCH 在 R4 前沉淀 `docs/decisions/ADR-021-strings-utf16-32-raw-ffi.md`，扩展 ADR-019（错误路径）到正常路径。PM 需在转 DEV 前确认 ADR-021 已入索引 |

---

## 13. 自检清单（ARCH 交付前）

按 architect-extension §模块设计文档模板逐项核对：

| 检查项 | 状态 | 证据 |
|--------|------|------|
| frontmatter（id / status / phase / revision / depends_on / last_updated） | ✅ | 文件顶部（v2 加 revision: v2） |
| 模块位置（文件路径） | ✅ | §1.1 |
| 职责（一句话说明） | ✅ | §0.1 |
| 详细设计（Rust 接口签名） | ✅ | §2-§3（6 Node + Encoding） |
| 与 Python 版本对应 | ✅ | §5（含方法级覆盖检查 + v2 标注 P2b 差异） |
| §0 原则对照表 | ✅ | §6（8 条逐项；v2 #2 行修正 encode 方向措辞） |
| 性能假设（L-02/L-05 对策） | ✅ | §7（瓶颈识别 + v2 类比基准 + 可证伪预测 + 双路径分析） |
| 边界条件清单 | ✅ | §8（5 类汇总） + §3 各 Node 逐项 |
| 与其他模块的交互 | ✅ | §9（依赖 + 被依赖 + 跨阶段决策检查） |
| DEV 实施清单 | ✅ | §10（R/P/T 三轨 + v2 R3.5 ADR-021 前置 + P5 docstring + 风险点扩充） |
| parity 测试模板 | ✅ | §11（case 矩阵 + v2 新增 P2b 拒绝 case + byteorder 验证 case） |
| PM 决策点汇总 | ✅ | §12（4 个决策 + v2 ADR-021 沉淀提示 + 变体数修正） |
| 参考实现映射完整性 | ✅ | §5.2（所有公开方法核对） |
| 跨阶段决策不违反 | ✅ | §9.3（ADR-001~020 + L-01~L-12；ADR-021 待 R3.5 沉淀） |
| **v2 修订记录** | ✅ | frontmatter 修订记录段；§2.1.3 P2b 决策；§2.2.2 P1+P2a；§2.2.3 #2 修正；§3.7.3 breaking change；§5.1 对齐表差异；§6 #2 行；§7.1/§7.2 类比基准；§7.4 PascalString 时机；§10 R3.5/R4/P5/风险点；§11.2 新增 case；§12.1 变体数 + ADR-021 |

---

## 14. v2 修订溯源（ARCH 自检）

依据 `plans/phase6-primitives-strings-adapter/traces/6.2-REV检视.md` 七个问题，逐项落实：

| REV 问题 | 严重度 | 修订位置 | 修订方式 |
|---------|-------|---------|---------|
| **P1** encode 路径引入 Rust String 中转层（§0 #2 违反） | 高（驳回） | §2.2.2 `encode` 函数 + `encode_utf16_32_raw` 签名 + §2.2.3 #1 + §6 #2 行 | encode 按编码分支：utf16/32 直接持有 `&Bound<PyString>` 调 raw FFI，不经 `Cow<str>` |
| **P2a** utf16/32 decode byteorder 值错误（Le=>0 应为 -1） | 高（驳回） | §2.2.2 `decode_utf16_32_raw` 注释 + SAFETY 段 | byteorder 改为 `Le => -1, Be => 1`（CPython 语义：0=BOM 检测，±1=强制） |
| **P2b** `from_user_str` 把 utf16/u16 静默映射 Utf16Le | 高（驳回） | §2.1.2 `from_user_str` + §2.1.3 决策记录 + §5.1 对齐表 + §3.7.3 + §11.2 case | ARCH 选**方案 1（显式不支持无后缀编码）**，编译期 Err 引导用 `utf_16_le`/`utf_16_be` |
| **P3** 性能假设缺量化数据（L-02） | 中（合并修订） | §7.1 类比基准段 + §7.2 类比来源列 | 引用 Phase 5 B1 11.64x / Phase 3 BitStruct 12x / ADR-019 a_err_eof 11.34x 作类比锚点 |
| **P4** StringEncoded 抛错方案破坏性未突出 | 低（标注） | §3.7.3 + §10.2 P5 | §3.7.3 增加 BREAKING CHANGES 段；P5 新增 docstring 标注步骤 |
| **P5** ADR-021 沉淀时机未明确 | 低（标注） | §10.1 R3.5（新增） + §12.1 ADR-021 行 | R3.5 由 ARCH 在 R4 前沉淀 ADR-021，PM 转 DEV 前确认入索引 |
| **P6** PascalString 性能依赖未实测的 6.1 VarInt | 中（标注） | §7.4 PascalString 验证时机段（新增） | 明确等 VarInt 实测或用 Phase 1 FormatField 作候选 |
| **P7** Node enum 变体数 24 错误（实际 20） | 低（标注） | §4.1 + §12.1 | 24 → 20（4 原子+5 复合+2 RO+2 填充+7 数组） |

**未变部分**（REV 驳回范围说明确认）：6 Node 独立实现方案 / Encoding enum 编译期思路 / StringError 新增变体 / 模块文件组织 / 边界条件清单 / parity 测试模板结构 / DEV 实施顺序 R1-R3/R5-R12。

---

> **设计完成时间**：v1 2026-07-30 / **v2 修订** 2026-07-30
> **下一步**：PM 决策 4 项（D-1/D-2/D-3/D-4）+ 确认 ADR-021 沉淀 → 转 REV v2 复检（重点 §2.2.2 encode 路径 + utf16/32 SAFETY 论证） → 通过后 DEV 实施
> **预计 DEV 工作量**：3-4 天（R1-R12 Rust + R3.5 ADR-021 ARCH 协作 + P1-P5 Python + T1-T5 测试）
