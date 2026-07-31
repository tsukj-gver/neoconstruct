---
id: TRACE-phase8-P0-VET
status: pass
phase: "8"
task: "8.P0 [VET 代码审查 P0 批次]"
last_updated: 2026-07-31
---

# Phase 8 P0 批次 VET 代码审查报告

> **角色**：VET
> **任务**：8.P0（9 Node + 5 Error 变体 + ParseStream::slice + HashAlgo）
> **依据**：AGENTS.md §0/§1/§3 + vetter.md + vetter-extension.md + 设计文档 `模块设计-Phase8-P0.md`
> **结论**：**驳回**（驳回目标状态：CODING）

## 审查文件清单

### Rust 新增（已全部审查）
- `construct-rs/src/nodes/const_node.rs`（ConstNode + impl + 测试，383 行）
- `construct-rs/src/nodes/default_node.rs`（DefaultNode + impl + 测试，295 行）
- `construct-rs/src/nodes/check_node.rs`（CheckNode + impl + 测试，284 行）
- `construct-rs/src/nodes/aligned.rs`（AlignedNode + impl + 测试，416 行）
- `construct-rs/src/nodes/terminated.rs`（TerminatedNode + impl + 测试，241 行）
- `construct-rs/src/nodes/probe.rs`（ProbeNode + impl + 测试，369 行）
- `construct-rs/src/nodes/hex.rs`（HexNode + HexDisplayClasses + load + 测试，366 行）
- `construct-rs/src/nodes/hex_dump.rs`（HexDumpNode + impl + 测试，206 行）
- `construct-rs/src/nodes/checksum.rs`（ChecksumNode + HashFunc + BuiltinHash + BytesSource + compute_builtin_hash + 测试，788 行）
- `construct-rs/src/stream.rs`（新增 `ParseStream::slice` 零拷贝切片 API，L339-357）
- `construct-rs/src/expr.rs`（新增 `eval_expr_bool`，L454-461）

### Rust 修改（重点审查）
- `construct-rs/Cargo.toml`（追加 5 个 hashfunc crate）
- `construct-rs/src/error.rs`（5 个新 Error 变体 L328-405 + ExceptionClasses 16→21 L729-793 + From<PyErr> CancelParsing 识别 L1150-1174）
- `construct-rs/src/schema.rs`（_parse_raw 顶层 catch CancelParsing L163-174）
- `construct-rs/src/nodes/mod.rs`（9 个 mod + Node enum 变体 + has_expressions + compute_ro_value）
- `construct-rs/src/compile.rs`（9 个 build_*_node 辅助函数 L3125-3664）
- `construct-rs/src/lib.rs`（re-export eval_expr_bool）

### Python 新增（接口对照审查）
- `python/construct/lib/__init__.py`、`lib/hex.py`、`_macros.py`、`_hashalgo.py`

## 审查清单（按 vetter-extension + base）

### 逻辑正确性：✅ / ❌（混合）

- **CancelParsing 顶层 catch（schema.rs L163-174）**：✅ 正确捕获 `ConstructError::CancelParsing { .. }` 返回 None，对齐 Python `except CancelParsing: pass`（core.py L416-419）。其他错误 `Err(e) => Err(e.into())` 正常转 PyErr 向上传播，不影响正常 parse 路径。build 路径（L190-214）不 catch（对齐 Python `build_stream` 无 try/except，CP-5）。
- **From<PyErr> CancelParsing 识别（error.rs L1150-1174）**：✅ 通过指针相等（`err_type_ptr == classes.cancel_parsing_error.as_ptr()`）识别，与 `is_builtin_class` 同模式。提取 path 失败时返回 default（顶层 catch 后 path 被丢弃，对齐 Python `pass`）。
- **eval_expr_bool（expr.rs L454-461）**：✅ 包装 `eval_expr_int` + `Ok(v != 0)`，对齐 Python `if not passed` 语义。错误传播 ExprContext / ExprFieldMissing / ExprDivByZero 等。
- **ConstNode 错误处理（const_node.rs L82-138）**：✅ rich_compare 使用 CompareOp::Eq（C API）；parse 不等时 Const Error 含 expected/parsed repr；build obj is None / obj == value 时用 value（对齐 Python `flagbuildnone=True`）。
- **CheckNode 错误处理（check_node.rs L62-87）**：✅ parse/build 调 eval_expr_bool；非真抛 CheckError，错误信息区分 "parsing" / "building"；sizeof 恒为 0（CK-4）。
- **Aligned padding 算法（aligned.rs L94-167）**：✅ `(-(consumed as i64)).rem_euclid(modulus_i64)` 正确对齐 Python `-(position2-position1) % modulus`；`checked_sub` 防御性处理 pos2 < pos1。
- **Aligned sizeof（aligned.rs L156-166）**：❌ **违反设计 §4.3 / 边界 AL-3/4/5 + D-P0-3**（详见 §1 关键发现）。
- **Checksum hash 比较（checksum.rs L278-296）**：✅ CS-12 长度不等先报错（与设计文档一致）；内容不等时 hex 编码 read/computed（对齐 Python binascii.hexlify）。
- **BuiltinHash 算法（checksum.rs L125-159）**：✅ SHA-256/MD5/CRC32/Adler32 已知向量测试覆盖（NIST FIPS 180-2 + zlib 标准测试向量）；CRC32/Adler32 转 4 字节 big-endian（对齐 Python `to_bytes(4, 'big')`）。
- **Hex/HexDump 类型分派（hex.rs L149-203 / hex_dump.rs L54-87）**：✅ `is_instance_of::<PyLong/PyBytes/PyDict>` 正确分派；HX-4/HD-3 未知类型透传对齐 Python `return obj`。

### 行为一致性（与 Python construct 2.10.70 对照）：✅ / ❌（混合）

- **Const parse/build**：✅ CN-1/2/3/4/5/6 全部测试覆盖，行为对齐。
- **Default parse/build**：✅ DF-1/2/3/5 测试覆盖；DF-4 lambda 不支持是 ADR-014 已知 parity 差异。
- **Check parse/build**：✅ CK-1/2/3/4 测试覆盖；CK-5/6 lambda / RW 编译期拒绝。
- **Aligned**：❌ **AL-3/4/5 sizeof 完全未实现**（详见 §1 关键发现）；AL-1/2/9/10/14 测试覆盖。
- **Terminated**：✅ TM-1/2/3/4/5/7 测试覆盖；与 Python 实现等价性论证（`stream.remaining() > 0` 替代 `stream.read(1)`）合理——抛错路径下 stream 状态不重要。
- **Probe**：⚠️ PB-1/2/3/4/7 测试覆盖；但 into 字段类型偏离设计文档（详见 §3 关键发现）。
- **Hex/HexDump**：✅ HX-1/2/6 + HD-1/3 测试覆盖。
- **Checksum**：✅ CS-1/2/3 + A2 callable 路径 + B1 build 测试覆盖。

### 错误处理：✅

- 外部调用（PyErr / pyo3 C API）错误正确传播（`?` 操作符）。
- 越界检查：ParseStream::slice 返回 None（stream.rs L355-357）；Checksum L190/L324 显式 out-of-bounds Err。
- 整数运算：Aligned `checked_sub`（L105-113）；rem_euclid 防御性。Checksum `eval_expr_int as usize` 负数回绕保护——slice 会 out-of-bounds 返回 None（语义正确，但错误信息可能令人困惑，低优先级）。
- 完整 match/if-else 覆盖：Hex/HexDump 的 int/bytes/dict/else 四分支完整；BuiltinHash 6 算法 match 完整。
- 错误信息含 path/message/repr：5 个新 Error 变体全部携带 path（error.rs L434-499 / L510-537 / L644-648）。

### 资源安全：✅（带性能改进建议）

- 无 `unsafe`：全部用 pyo3 安全 API（rich_compare / is_instance_of / call_method1 / extract）。
- 无 panic / unwrap / expect / TODO / FIXME（非测试代码）。
- 无内存泄漏风险：Py<PyAny> / Py<PyType> 持有引用计数管理。
- **Checksum 路径 B1 加了 `.to_vec()`**（checksum.rs L194）——非设计文档"严格零拷贝"语义，但**性能已达标（1.84x）**，详见 §2 关键发现。
- **ProbeNode printout 重复 import**（probe.rs L96-99 / L102-105 / L126-129 / L153-156 / L158-161）：每次 printout 调用 5 次 `py.import_bound("builtins").getattr("print").call1(...)`。建议缓存 print callable（性能改进建议，不阻塞 VET 验收——Probe 是调试构造器，不设硬性能门禁）。

### API 一致性：✅

- snake_case 命名风格统一（`fn parse` / `fn build` / `fn sizeof` / `fn new` / `pub fn inner`）。
- 与设计文档 §7 Node enum 变体名一致（Const / Default / Check / Hex / HexDump / Checksum / Aligned / Terminated / Probe 共 9 个）。
- 5 个 Error 变体与设计 §9 一致（Const / Check / Checksum / Terminated / CancelParsing）。
- Cargo.toml 新增 5 crate（sha2/sha1/md-5/crc32fast/adler）与设计 §3.4 + §8.1 一致；md-5 crate name 转 lib name `md5`（checksum.rs L130 `use md5::Md5` 正确）。

### 边界条件：❌（AL-3/4/5 违反）

- **AL-3/4/5（sizeof 常量 modulus 应返回 inner.sizeof + pad）**：❌ 违反（详见 §1）。
- **CS-11（变长 checksumfield sizeof 返回 Err）**：✅ ChecksumNode::sizeof 直接转发 checksumfield.sizeof（L436-438），变长时自然返回 Err。
- 其他 P0 节点边界条件（CN/DF/CK/HX/HD/TM/PB/CS）测试覆盖完整。
- AL-6/7/8/11/12 编译期 PaddingError 校验（compile.rs L3380-3392 / L3447-3456）：✅ modulus < 2 + pattern len != 1 均编译期拒绝。
- CS-10（HashAlgo 未知 enum name）编译期拒绝（compile.rs L3620-3635）：✅ match 6 已知 name + `_` 分支返回 CompilationError。
- CS-13/14（hashfunc 既非 HashAlgo 也非 callable）编译期拒绝（compile.rs L3637-3645）：✅。

### §0 合规：✅

- **#1 一次 FFI**：所有 P0 节点 parse/build 入口仅 1 次 FFI（共享 StructMixin.parse / build 入口）。Rust 内 C API 操作（rich_compare / is_instance_of / call_method1 / call1 / GetItem / extract）属于 §0.2 判据 2，不计额外 FFI。Checksum 路径 A Python callable 回调 1 次额外 FFI，但用户主动选择（ADR-022 §0.1 论证）。
- **#2 无中间表示层**：所有节点 parse 直接构造/借用 PyObject，无 Rust 中间数据类型。Checksum `data_owned: Vec<u8>` 是字节缓冲（非对象表示），不违反。
- **#3 输入输出无 trait 抽象**：仅 `Box<Node>` enum_dispatch。
- **#4 pyo3 核心依赖**：5 个 hashfunc crate 是纯 Rust 内部计算库（不跨 FFI），与 `half = "2.4"`（Phase 6.1 Float16）同性质，pyo3 仍是 FFI 唯一桥梁。
- **#5 mashumaro API**：9 个新构造器是字段描述符。
- **#6 enum_dispatch**：9 个新 Node 加入 Node enum。
- **#7 Result + path**：5 个新 Error 变体携带 path。
- **#8 Stream 纯 Rust**：ParseStream::slice / read / tell / seek / remaining 全 Rust 内。

---

## §1 关键发现 1：Aligned sizeof 违反设计文档与 D-P0-3 决策（**驳回**）

### 问题位置

`construct-rs/src/nodes/aligned.rs` L156-166：

```rust
fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
    // D-P0-3：modulus 是表达式时返回 Err（对齐 Python SizeofError）。
    // 编译期常量 modulus 也通过 ExprProgram 包装（Const 指令），
    // 但 sizeof 签名无 py 参数，无法 eval。
    // 对齐 Python 行为：modulus 表达式 + sizeof 触发 SizeofError。
    Err(ConstructError::Generic {
        message: "Aligned sizeof requires runtime modulus evaluation (not supported in static sizeof)"
            .to_string(),
        path: String::new(),
    })
}
```

### 违反内容

**设计文档 §4.3.2 边界条件清单 AL-3/4/5（L1298-1300）**：

| 编号 | 场景 | 设计预期行为 | 实际行为 |
|------|------|------------|---------|
| AL-3 | `Aligned(4, Int16ub).sizeof()` | **返回 4**（inner_len=2, pad=2） | ❌ 返回 Err |
| AL-4 | `Aligned(4, Bytes(3)).sizeof()` | **返回 4**（inner_len=3, pad=1） | ❌ 返回 Err |
| AL-5 | `Aligned(4, Bytes(4)).sizeof()` | **返回 4**（inner_len=4, pad=0） | ❌ 返回 Err |

**设计文档 §10 D-P0-3 决策（L1965-1970）**：
> ARCH 推荐：对齐 Python 行为——modulus 表达式 + sizeof 触发 SizeofError。理由：...
> - **DEV 实施时 Aligned::sizeof 检测 modulus 是否编译期常量；非常量直接返回 Err**

即 D-P0-3 明文要求 "**编译期常量时 sizeof 成功**，**仅非常量才返回 Err**"，而实际实施 **统一返回 Err**。

### 影响分析

1. **parity 差异**：Python `len(Aligned(4, Int16ub))` 返回 4；construct-rs 抛 SizeofError。用户从 Python 迁移时遇到非预期失败。
2. **性能影响（更重要）**：Struct 静态布局预分配（schema.rs L195-202 `self.static_size`）依赖各字段 sizeof 成功。`Aligned` 字段 sizeof 失败导致整个 Struct static_size 为 None，BuildStream::new() 退化为无预分配模式（每次 build 走 realloc 扩容路径）。
3. **测试本身错位**：`sizeof_returns_err_runtime_modulus` 测试（aligned.rs L364-373）用 `modulus_four()`（即 `ExprOp::Const(4)` 常量）却断言"应返回 Err"——这是 DEV 把实施偏离当作预期固化到测试中。

### 检测 modulus 是否编译期常量的可行方案

设计文档 §4.3.2 备选方案已指出："modulus 若是编译期常量，sizeof 不需 eval（直接用常量值）"。`ExprOp::Const(i64)` 是单条编译期常量指令（expr.rs L50），可通过 `self.modulus.ops() == &[ExprOp::Const(n)]` 检测。

建议实现（参考）：
```rust
fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
    // D-P0-3：检测 modulus 是否编译期常量（单条 ExprOp::Const 指令）。
    let modulus = match self.modulus.ops() {
        [ExprOp::Const(n)] if *n >= 2 => *n as usize,
        _ => return Err(ConstructError::Generic {
            message: "Aligned sizeof requires constant modulus (dynamic modulus triggers SizeofError)"
                .to_string(),
            path: String::new(),
        }),
    };
    let inner_len = self.inner.sizeof(ctx)?;
    let pad = (-(inner_len as i64)).rem_euclid(modulus as i64) as usize;
    Ok(inner_len + pad)
}
```

### 处理建议

**驳回**：要求 DEV 修复 AlignedNode::sizeof，使其：
1. modulus 是编译期常量（`ops() == [ExprOp::Const(n)]`）时按 AL-3/4/5 返回正确值。
2. modulus 是表达式时返回 Err（D-P0-3 已知 parity 差异，不阻塞）。
3. 同步修复 `sizeof_returns_err_runtime_modulus` 测试断言——当 modulus_four() 是 Const 时应断言成功。

---

## §2 关键发现 2：Checksum 路径 B1 实施偏离"严格零拷贝"描述（**性能达标，不阻塞**）

### 问题位置

`construct-rs/src/nodes/checksum.rs` L184-219（parse BuiltIn 路径）：

```rust
BytesSource::StreamRange { start, end } => {
    let s = eval_expr_int(start, ctx, py)? as usize;
    let e = eval_expr_int(end, ctx, py)? as usize;
    stream
        .slice(s, e)
        .ok_or_else(|| ConstructError::Checksum { ... })?
        .to_vec()    // <-- 拷贝点：N 字节
}
BytesSource::ContextBytes { field_name, .. } => {
    ...
    borrow.to_vec()  // <-- 拷贝点：N 字节
}
```

### 偏离内容

**设计文档 §3.5.2 路径 B1 实施参考代码**：
```rust
let data_slice: &[u8] = match &self.bytes_source {
    BytesSource::StreamRange { start, end } => {
        // 零拷贝：直接切片借用
        stream.slice(s, e).ok_or(...)?  // <-- 设计示例：直接返回 &[u8]
    }
    BytesSource::ContextBytes(expr) => { ... }
};
let digest = compute_builtin_hash(builtin, data_slice)?;  // <-- 直接传 slice
```

**设计文档 §3.5.4 路径矩阵**：
| 路径 | hashfunc | bytes_source | 拷贝次数 |
|------|---------|-------------|---------|
| B1（最优） | Rust 内置 | StreamRange | **0 次** |
| B2 | Rust 内置 | ContextBytes | **0 次**（借用 PyBytes） |

实际实施：B1 与 B2 均加了 `.to_vec()`，发生 1 次 N 字节拷贝。

### DEV 偏离原因分析

外层 match 把 `BuiltIn(builtin)` 与 `StreamRange` / `ContextBytes` 组合 4 分支统一为 `data_owned: Vec<u8>` 类型——`ContextBytes` 路径需从 PyObject 借用 bytes 后 `.to_vec()` 拿 owned 跨所有权边界，为统一类型签名，`StreamRange` 也被迫 `.to_vec()`。

### 影响评估

- **L-14 教训核心**：承认 "Rust 内置 hashfunc 零拷贝路径"存在（否定了"必须拷贝"硬约束）——✅ 已实现（路径 B 存在且 sha2 操作 `&[u8]`）。
- **设计文档"严格零拷贝"描述**：与实施有偏离，1 次额外拷贝。
- **性能**：实测 B1 vs A2 = 1.84x，超设计 §3.7.2 预测 ≥1.3x 目标。**性能未受影响**。
- **§0 合规**：不违反（Vec<u8> 是 Rust 内字节缓冲，非 Python 中间表示层）。

### 处理建议

**性能改进建议，不阻塞 VET 验收**。建议 DEV 在 P1 或后续优化轮重构 ChecksumNode::parse：

```rust
match &self.hashfunc {
    HashFunc::BuiltIn(builtin) => {
        // BuiltIn 路径真正零拷贝：直接传 &[u8] 给 compute_builtin_hash
        let computed = match &self.bytes_source {
            BytesSource::StreamRange { start, end } => {
                let s = eval_expr_int(start, ctx, py)? as usize;
                let e = eval_expr_int(end, ctx, py)? as usize;
                let slice = stream.slice(s, e).ok_or(...)?;
                compute_builtin_hash(*builtin, slice)
            }
            BytesSource::ContextBytes { field_name, .. } => {
                let bytes_obj = ctx.get_field(field_name)?.ok_or(...)?;
                let borrow: &[u8] = bytes_obj.extract()?;
                compute_builtin_hash(*builtin, borrow)
            }
        };
        computed
    }
    HashFunc::PythonCallable(callable) => {
        // Callable 路径需构造 PyBytes（CPython 硬约束），保持 to_vec()
        ...
    }
}
```

---

## §3 关键发现 3：Probe.into 设计质疑（**PM/ARCH 决策，不阻塞 VET 验收**）

### 偏离内容

| 维度 | 设计文档 §5.2.2 原文 | DEV 实施 |
|------|--------------------|---------|
| 字段类型 | `into: Option<ExprProgram>` | `into: Option<FieldName>` |
| 扩展 | 推荐 P0 内扩展 `eval_expr_any` ~30 行（D-P0-2） | 改用 FieldName，避免扩展 expr.rs |
| 表达式组合 | 支持 `Probe(count + 1)` | ❌ 不支持（仅字段名引用） |
| 任意类型字段 | 需 `eval_expr_any` | ✅ 支持（ctx.get_field 返回 PyObject） |
| ADR-014 一致性 | ✅ | ✅（FieldName 不接 callable） |

### VET 评估

**DEV 设计质疑合理性分析**：
1. **API 简化合理性**：Probe.into 实际用例是"调试打印某字段值"（任意类型），FieldName 路径 `ctx.get_field(name)` 直接返回 PyObject，对任意类型字段（int/bytes/str/dict）均可用，无类型限制。
2. **expr.rs 扩展省略**：FieldName 替代 ExprProgram 后无需扩展 `eval_expr_any`，减少 ~30 行 Rust 代码。代价是失去算术表达式组合能力（如 `Probe(count + 1)`）。
3. **Parity 影响**：`Probe(lambda ctx: complex_expr)` 不支持（与 ADR-014 一致）；`Probe(some_field)` 完全支持任意类型字段。
4. **ADR-014 硬约束**：FieldName 与 Switch FieldRef 同模式（同 `nodes/struct_node.rs::FieldName` 类型），符合"所有动态值位置不接 callable"。

**与 D-P0-2 决策关系**：D-P0-2 是 "推荐扩展" 而非"必须扩展"——PM 决策可接受 DEV 替代方案（FieldName）作为另一种实施路径。

### 处理建议

**超出 VET 职责**——这是设计变更（D-P0-2 决策），按 AGENTS.md §1 工作流：**PM 转发 ARCH 必须回应**。

VET 仅指出：
1. ✅ DEV 已按规范标注 `[设计质疑]`（probe.rs 模块级注释 + DEV 实施 trace L100-117）。
2. ✅ 实施 API 完整、测试覆盖（PB-1/2/3/4/7）、错误处理（ctx.get_field 失败 fallback `<field X missing>`）。
3. ✅ 未引入 §0 违反。
4. ⚠️ **设计文档 §5.2.2 与实施偏离**——PM/ARCH 需决策是否：
   - 方案 A：接受 DEV 方案（更新设计文档 §5.2.2 + D-P0-2 决策记录）
   - 方案 B：驳回 DEV 改回 ExprProgram 路径（扩展 `eval_expr_any` ~30 行）
   - 方案 C：双轨支持（FieldName + ExprProgram）

VET 不替 ARCH 决策。代码质量层面 ProbeNode 通过；设计层面由 PM/ARCH 处理。

---

## §4 Checksum 零拷贝确认（PM 任务清单要求）

**问题**：ParseStream::slice 借用 + Rust sha2 操作 `&[u8]` 是否真正零拷贝？

**确认结果**：

1. **ParseStream::slice（stream.rs L355-357）**：✅ **真正零拷贝**。返回 `Option<&'a [u8]>`，生命周期绑定到 ParseStream 持有的 `&'a [u8]`（即 Python 端 PyBytes 的 buffer，pyo3 GIL 保证有效）。`self.data.get(start..end)` 是 Rust slice 借用操作，不拷贝字节、不构造 PyBytes。

2. **Rust sha2 crate 操作 &[u8]**：✅ **真正零拷贝**。`sha2::Sha256::new().update(&[u8]).finalize()` 内部直接遍历字节切片，无 FFI 跨界。

3. **路径 B1 全链路**：⚠️ **设计目标零拷贝，实施加了 1 次 `.to_vec()`**（详见 §2）。原因是 outer match 统一 `data_owned: Vec<u8>` 类型以兼容 ContextBytes 路径。

4. **L-14 教训核心达成**：✅ 否定了 "CPython bytes 不可变 → 跨 FFI 必须拷贝" 这个伪硬约束——通过 Rust 内置 hashfunc 路径绕开 callable 回调。L-14 教训的核心要求（"必须枚举至少 2 条实现路径交叉验证"）已落实。

5. **性能达标**：B1 vs A2 = **1.84x**（超过设计 §3.7.2 预测 ≥1.3x 目标），符合 §0 性能门禁。

**结论**：Checksum 零拷贝路径**核心架构正确**（slice 借用 + sha2 操作 &[u8] 真正零拷贝），但**实施细节**（路径 B1 的 `.to_vec()`）与设计文档"严格零拷贝"描述略有偏离。**不影响 L-14 教训达成**，**不影响性能达标**。属"实施可优化"非"设计错误"。

---

## §5 Probe.into 设计质疑评估（PM 任务清单要求）

详见 §3 关键发现 3。

**结论**：DEV 设计质疑合理且符合工程实施层面优化原则。代码质量层面通过；设计层面变更（D-P0-2 决策）由 PM 转发 ARCH 决策。VET 不替 ARCH 决策。

---

## 总体审查清单结论

| 维度 | 结论 |
|------|------|
| 逻辑正确性 | ❌（Aligned sizeof 违反设计 AL-3/4/5 + D-P0-3） |
| 行为一致性 | ❌（Aligned sizeof 与 Python 行为不一致） |
| 错误处理 | ✅ |
| 资源安全 | ✅（带性能改进建议） |
| API 一致性 | ✅ |
| 边界条件 | ❌（AL-3/4/5 违反） |
| §0 合规 | ✅ |

## 驳回决策

**结论：驳回**（驳回目标状态：CODING）

### 必须修复（阻塞 VET 验收）

1. **[aligned.rs L156-166] AlignedNode::sizeof 违反设计 AL-3/4/5 + D-P0-3 决策**
   - 现状：sizeof 统一返回 Err，即使 modulus 是编译期常量（`ExprOp::Const(n)`）也失败。
   - 设计要求：编译期常量 modulus 时应返回 `inner.sizeof + pad`；仅非常量 modulus 才返回 Err。
   - 影响：parity 差异（Python `len(Aligned(4, Int16ub))` 返回 4，construct-rs 抛 SizeofError）+ Struct static_size 预分配失效（性能影响）。
   - 修复建议：参见 §1 检测 modulus 是否编译期常量的可行方案。
   - 同步修复：`sizeof_returns_err_runtime_modulus` 测试（aligned.rs L364-373）—— 当 modulus 是 Const 时应断言 sizeof 成功而非失败。建议加 2-3 个测试用例覆盖 AL-3/4/5。

### 设计层面待处理（不阻塞 VET，PM 转发 ARCH）

2. **[probe.rs L52 + compile.rs L3289-3339 + nodes/mod.rs] Probe.into 设计质疑（D-P0-2 决策）**
   - DEV 改用 FieldName 代替 ExprProgram，超出 VET 职责。
   - PM 按 AGENTS.md §1 工作流转发 ARCH 必须回应（方案 A/B/C）。
   - 若 ARCH 决策方案 B（驳回改回 ExprProgram），DEV 同步扩展 `eval_expr_any` ~30 行。

### 性能改进建议（不阻塞 VET，DEV 自行评估优先级）

3. **[checksum.rs L184-219] ChecksumNode::parse 路径 B1/B2 加了 `.to_vec()`**
   - 偏离设计 §3.5.4 路径矩阵"0 次拷贝"描述。
   - 性能达标（1.84x），不影响 L-14 教训达成。
   - 建议在 P1 或后续优化轮重构 match 分支使 BuiltIn 路径真正零拷贝。
   - **同步更新**：设计文档 §3.5.2 / §3.5.4 与实施偏离——若 ARCH 决策保留 `.to_vec()`，需更新设计文档路径矩阵拷贝次数为"1 次"。

4. **[probe.rs L96-161] ProbeNode printout 重复 import**
   - 每次 printout 调用 5 次 `py.import_bound("builtins").getattr("print").call1(...)`。
   - 建议在 ProbeNode 构造时缓存 print callable，或 printout 入口缓存一次。
   - Probe 是调试构造器不设硬性能门禁，低优先级。

### 验收通过的子任务

| 子任务 | 结论 | 备注 |
|--------|------|------|
| 8.10 CancelParsing | ✅ 通过 | 顶层 catch + From<PyErr> 识别完整正确 |
| 8.1 Const/Default/Check | ✅ 通过 | 错误处理 + parity 完整 |
| 8.9 Terminated | ✅ 通过 | 行为等价 Python（remaining 替代 read(1)） |
| 8.9 Probe（代码层面） | ✅ 通过 | 测试覆盖完整（设计层面偏离 PM/ARCH 决策） |
| 8.4 Hex/HexDump | ✅ 通过 | 走 Rust Node 非 AdapterCallbackNode，§0.2 判据 2 合规 |
| 8.5 Checksum | ✅ 通过（带改进建议） | L-14 教训达成，零拷贝架构正确，性能达标 1.84x |
| 8.8 Aligned | ❌ **驳回** | sizeof 违反 AL-3/4/5 + D-P0-3，需 DEV 修复 |

## 返工审查范围（DEV 修复后）

- 必须修复项 1（Aligned sizeof）：仅审查 aligned.rs L156-166 sizeof 修复 + 测试断言更新 + AL-3/4/5 测试用例新增；同步验证 Struct static_size 预分配路径恢复（schema.rs L195-202）。
- 若 ARCH 决策 Probe.into 方案 B：补充审查 expr.rs 新增 `eval_expr_any` ~30 行 + probe.rs 改回 ExprProgram + compile.rs L3289-3339。
- 性能改进项 3/4：DEV 自行评估优先级，不阻塞 VET 复审。

---

## §6 复审段（VET v2，2026-07-31）— Aligned sizeof 修复

> **触发**：VET v1 驳回项（§1 必须修复 1：AlignedNode::sizeof 违反 AL-3/4/5 + D-P0-3）DEV 已修复，本段为驳回返工审查。
> **范围**：按 VET base §驳回返工审查「仅审查被驳回相关部分 + 确认未引入新问题」。审查对象：
> 1. `aligned.rs` sizeof 编译期常量检测（L176-199）
> 2. `aligned.rs` has_expressions 委托（L89-103）
> 3. `schema.rs` static_size 端到端测试（L317-393）
>
> 修复未涉及接口变更或核心逻辑重写（AlignedNode 字段签名不变），故按聚焦审查执行，未重跑完整 P0 清单。

### §6.1 aligned.rs sizeof（L176-199）— ✅ 通过

**实施**：

```rust
fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
    let modulus = match self.modulus.ops() {
        [ExprOp::Const(n)] if *n >= 2 => *n as usize,
        _ => return Err(ConstructError::Generic { ... }),
    };
    let inner_len = self.inner.sizeof(ctx)?;
    let pad = (-(inner_len as i64)).rem_euclid(modulus as i64) as usize;
    Ok(inner_len + pad)
}
```

**审查结论**：

- **编译期常量检测**：`[ExprOp::Const(n)] if *n >= 2` 精确匹配单条 Const 指令且 modulus 合法（≥2）。`ExprOp::Const(i64)` 是编译期常量指令（expr.rs L50，编译期包装，运行时求值不依赖 ctx），符合 D-P0-3「编译期常量时 sizeof 成功」。
- **非常量 → Err**：`_` 分支统一返回 Err（含 GetInt 运行期表达式、多指令表达式、Const(<2) 非法常量），符合 D-P0-3「仅非常量才返回 Err」。
- **padding 算法**：`(-(inner_len as i64)).rem_euclid(modulus as i64)` 对齐 Python `(-subconlen) % modulus`（rem_euclid 保证非负，对齐 Python 模运算语义）。
- **Python parity 确认**：Python `Aligned._sizeof`（core.py L4317-4325）常量 modulus + subcon 有 size → 返回 `subconlen + (-subconlen % modulus)`；lambda 引用缺失字段 → KeyError → SizeofError。construct-rs 行为完全对齐（常量成功 / 运行期 Err 对齐 SizeofError）。
- **类型安全**：`*n as usize`（n 为 i64 且 n≥2 保证正数，64 位平台无截断）；`inner_len as i64` / `modulus as i64` 在实际场景（modulus 小正整数、inner_len 有限）无溢出风险。

**AL-3/4/5 验证（测试覆盖）**：

| 编号 | 场景 | inner_len / pad | 预期 | 实测测试 |
|------|------|----------------|------|---------|
| AL-3 | `Aligned(4, Int16ub).sizeof()` | 2 / 2 | 4 | ✅ `sizeof_const_modulus_returns_inner_plus_pad`（aligned.rs L439） |
| AL-4 | `Aligned(4, Bytes(3)).sizeof()` | 3 / 1 | 4 | ✅ `sizeof_const_modulus_pad_one`（aligned.rs L453） |
| AL-5 | `Aligned(4, Bytes(4)).sizeof()` | 4 / 0 | 4 | ✅ `sizeof_const_modulus_no_pad_when_aligned`（aligned.rs L467） |
| 补充 | `Aligned(8, Int8ub).sizeof()` | 1 / 7 | 8 | ✅ `sizeof_const_modulus_other_values`（aligned.rs L481） |
| D-P0-3 | 运行期 modulus（GetInt）→ Err | — | Err | ✅ `sizeof_runtime_modulus_returns_err`（aligned.rs L493） |
| 防御 | Const(1) < 2 → Err | — | Err | ✅ `sizeof_const_modulus_below_two_returns_err`（aligned.rs L509） |

VET v1 §1 指出的「测试本身错位」（`sizeof_returns_err_runtime_modulus` 用 Const 却断言 Err）已修复——测试拆分为 `sizeof_const_modulus_*`（断言成功）与 `sizeof_runtime_modulus_returns_err`（用 GetInt 断言 Err），语义清晰对齐。

### §6.2 aligned.rs has_expressions（L89-103）— ✅ 通过

**实施**：

```rust
fn modulus_is_const(&self) -> bool {
    matches!(self.modulus.ops(), [ExprOp::Const(_)])
}
pub fn has_expressions(&self) -> bool {
    !self.modulus_is_const() || self.inner.has_expressions()
}
```

**审查结论**：

- **委托逻辑正确**：modulus 是常量 → 不计入表达式；递归检查 inner 子树（与 Bitwise/Transform/Subconstruct 同模式）。
- **Node enum 分派**：`nodes/mod.rs` L435 `Node::Aligned(a) => a.has_expressions()` 已正确接入（v1 审查范围，未回归）。
- **分支覆盖完整**（3 个单元测试，aligned.rs L264-299）：
  - Const(4) + 无 expr inner → false ✅
  - 运行期 modulus（GetInt）→ true ✅
  - Const(4) + 含 expr inner（Rebuild）→ true ✅
- **性能影响链恢复**：has_expressions=false → schema.rs L86-90 走 `root.sizeof().ok()` → static_size=Some → `_build_raw` 走 `BuildStream::with_capacity`（L199-202）预分配。VET v1 §1 影响分析第 2 点（Struct static_size 预分配失效）已修复。

**非阻塞观察（代码整洁性，不影响验收）**：`modulus_is_const` 用 `[ExprOp::Const(_)]`（匹配任意 Const 含 Const(1)），而 sizeof guard 要求 `Const(n) if *n >= 2`。两者对「合法常量」定义略有差异。此差异**无实际影响**：
- compile.rs 编译期拒绝 Const(<2)（v1 已确认 L3380-3392），生产路径不会出现非法 Const。
- 即便手动构造（仅测试），has_expressions=false → sizeof 尝试 → Err → static_size=None（安全降级，无预分配但不崩溃）。
- 建议（可选，后续优化轮）：将 `modulus_is_const` 收紧为 `[ExprOp::Const(n)] if *n >= 2` 以保持两处判定一致。**不阻塞 VET 验收**。

### §6.3 schema.rs static_size 端到端测试（L317-393）— ✅ 通过

**实施**：新增 2 个端到端测试，验证 sizeof 修复对 Struct static_size 预分配链路的恢复。

**审查结论**：

| 测试 | 场景 | 断言 | 结论 |
|------|------|------|------|
| `static_size_restored_when_aligned_has_const_modulus`（L325） | Struct(Aligned(4, Int16ub)) | `static_size == Some(4)` | ✅ 预分配恢复 |
| `static_size_none_when_aligned_has_runtime_modulus`（L363） | Struct(Aligned(GetInt(0), Int16ub)) | `static_size == None` | ✅ 运行期 modulus 保持 None |

- **端到端覆盖**：测试从 `CompiledSchema::new` → `root.has_expressions()` → `root.sizeof(placeholder)` → `static_size` 缓存全链路，验证 D-P0-3 修复的实际效果（预分配容量恢复）。
- **CompiledSchema::new 逻辑（L86-90）未变**：`if !has_expressions { root.sizeof().ok() } else { None }` 逻辑正确衔接修复后的 AlignedNode::sizeof。
- **对照测试设计合理**：常量 modulus（成功）+ 运行期 modulus（Err→None）双测试形成 A/B 对照，精确刻画 D-P0-3 决策边界。

**测试设计说明**：两测试用 `StructNode::new_for_test`（struct_node.rs L228）强制 StructNode.has_expressions=false，模拟编译期正确识别 Aligned 常量 modulus 不含表达式。真实编译路径下 StructNode 的 has_expressions 聚合会通过 `Node::Aligned` 分派调 AlignedNode::has_expressions()（已在 §6.2 单元测试验证）。聚合链路与 schema 缓存链路分片测试，覆盖完整，无盲区。

### §6.4 质量门禁（运行验证）

| 门禁 | 结果 |
|------|------|
| `cargo build`（PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1，环境 Python 3.14） | ✅ Finished |
| `cargo clippy --lib` | ✅ 零 warning 零 error |
| `cargo test --lib aligned` | ✅ aligned 17 测试 + schema 2 测试全 PASS（52 passed / 0 failed） |

> 注：Python 3.14 超出 PyO3 0.22.6 默认支持（3.13），通过 `PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1` 抑制。属环境配置，非代码问题，不影响审查结论。

### §6.5 复审清单小结

| 维度 | 结论 | 依据 |
|------|------|------|
| 逻辑正确性 | ✅ | sizeof 常量检测 + padding 算法 + Python parity（§6.1） |
| 行为一致性 | ✅ | AL-3/4/5 与 Python `_sizeof` 对齐，运行期 modulus 对齐 SizeofError（§6.1） |
| 错误处理 | ✅ | 非法常量 / 运行期表达式统一 Err，路径清晰 |
| 资源安全 | ✅ | 无 unsafe / unwrap / panic；static_size 预分配恢复（性能正向） |
| API 一致性 | ✅ | 命名风格 + Node enum 分派不变（无接口变更） |
| 边界条件 | ✅ | AL-3/4/5 + 补充 modulus 8 + 运行期 + Const(<2) 全覆盖 |
| §0 合规 | ✅ | 修复未引入 §0 违反（sizeof 纯 Rust 内部计算） |
| 新问题引入检查 | ✅ | 修复聚焦 sizeof + has_expressions，未触碰 parse/build 路径；clippy 零 warning 确认无回归 |

### §6.6 复审结论

**结论：通过**（驳回项已修复且未引入新问题）

- ✅ **Aligned sizeof 已解决**：D-P0-3「编译期常量 modulus 成功 / 非常量 Err」正确实施，AL-3/4/5 全部覆盖，Python parity 确认。
- ✅ **has_expressions 委托正确**：常量 modulus 不计入表达式，inner 递归检查，static_size 预分配链路恢复。
- ✅ **static_size 端到端测试**：常量成功（Some）+ 运行期 None 双对照，覆盖完整。
- ✅ **质量门禁**：build / clippy / test 全绿。

**非阻塞建议（可选，不阻塞本子任务验收）**：
1. [aligned.rs L89-91] `modulus_is_const` 可收紧为 `Const(n) if *n >= 2` 与 sizeof guard 保持一致（防御性整洁，生产路径无影响）。

**8.P0 子任务状态**：8.8 Aligned 由 ❌ 驳回转为 ✅ 通过。8.P0 批次全部子任务通过（详见 §验收通过的子任务 表，8.8 行更新）。
