---
id: TRACE-8.HEX-OPT-DEV
phase: "8"
task: "8.HEX-OPT [DEV 实施 Hex/HexDump parse 性能优化（方案 A+B）]"
status: coding
owners: [DEV]
started: 2026-07-31
completed: 2026-07-31
manifest_refs: []
---

# 8.HEX-OPT — Hex/HexDump parse 性能优化（方案 A+B）开发日志

## 任务概述

ARCH 完成 Hex 族 parse 性能差距根因分析（`docs/analysis/phase8-hex-parse-perf-investigation.md`），
判定为设计判断错误（`HexDisplayedInteger.new` 是 Python 字节码非 C 级）+ 实现问题
（fmtstr 每次 PyString::new）。提出两个修正方案 A+B。

DEV 实施方案 A+B，修正 hex.rs int 分支实现，保持 parity 不变。

## 方案 A+B 实施细节

### 方案 A：Rust 内 call1+setattr 替代 call_method1（int 路径）

**改动文件**：`construct-rs/src/nodes/hex.rs` parse int 分支

**改动前**（跨 FFI 回调 Python 字节码）：
```rust
let cls_bound = self.display_classes.integer.bind(py);
let new_obj = cls_bound
    .call_method1("new", (bound, &fmt_py))   // → ceval 字节码循环
    .map_err(|e| ConstructError::Generic { ... })?;
```

`call_method1("new", ...)` 的 C API 链：
1. `PyObject_GetAttr(cls, "new")` → 拿 staticmethod 描述符
2. staticmethod `__get__` → 返回 PyFunctionObject
3. `PyObject_Call(func, args)` → **进入 ceval 字节码循环**执行 `new` 函数体

**改动后**（纯 C API，不进字节码）：
```rust
let cls_bound = self.display_classes.integer.bind(py);
let new_obj = cls_bound
    .call1((bound,))                           // → type.__call__ → long_new (C 级)
    .map_err(|e| ConstructError::Generic { ... })?;
new_obj
    .setattr("fmtstr", interned.bind(py))      // → PyObject_GenericSetAttr (C 级)
    .map_err(|e| ConstructError::Generic { ... })?;
```

**消除的开销**：getattr(cls, "new") 属性查找 + staticmethod 描述符 __get__ + ceval 字节码
dispatch + call_method1 字符串参数装箱。

**parity 论证**：`lib/hex.py` 的 `new` 字节码体等价于 `obj = cls(intvalue); obj.fmtstr =
fmtstr; return obj`。方案 A 是其 C API 等价重写（call1 走 type.__call__ → long_new，
setattr 走 PyObject_GenericSetAttr），产生的对象结构完全一致。测试 `parse_int_returns_hex_displayed_integer`
新增 `fmtstr` 属性验证断言通过，Python 侧 parity check 全部通过。

### 方案 B：编译期 intern fmtstr（消除每次 PyString::new）

**改动文件**：
- `construct-rs/src/nodes/hex.rs` struct 字段 + constructor + getter
- `construct-rs/src/compile.rs` build_hex_node fmtstr 物化

**改动前**：
```rust
pub struct HexNode {
    fmtstr: Option<String>,  // Rust String，每次 parse clone + new PyString
}
// parse: String clone → PyString::new_bound (每次 ~60ns)
```

**改动后**：
```rust
pub struct HexNode {
    fmtstr: Option<Py<PyString>>,  // 编译期 intern，parse 时借用
}
// compile.rs: PyString::new_bound(py, &fmt_str).into_py(py) (一次)
// parse int 分支 Some: interned.bind(py) 直接借用（0 构造）
```

**消除的开销**：每次 parse 的 String clone（~15ns）+ PyString::new_bound（~45ns）≈ 60ns。

**parity 论证**：fmtstr 内容不变（"08X" 等），仅物化时机从运行期提前到编译期。PyString
内容完全一致，setattr 行为不变。

### None fallback 路径保留

当 inner sizeof 不可静态计算时（fmtstr = None），parse 时运行期计算 fmtstr 并构造
新 PyString。此路径行为与改动前一致，仅 fmtstr 类型从 String 变为 Py<PyString>。

## parity 确认

### Rust 单元测试验证

`hex.rs::tests::parse_int_returns_hex_displayed_integer` 测试增强：
```rust
// parity 验证：fmtstr 属性已设置（方案 A setattr）
let fmtstr_val: String = bound.getattr("fmtstr").unwrap().extract().unwrap();
assert_eq!(fmtstr_val, "08X");
```

全 1372 单元测试通过（0 failed）。

### Python parity check（hex_parity_check.py）

| Case | type | value | str | repr | fmtstr | build round-trip |
|------|------|-------|-----|------|--------|------------------|
| HX1 Hex(Int32ub) | HexDisplayedInteger | 258 | 0x00000102 | 258 | 08X | ✅ |
| HX2 Hex(Bytes(4)) | HexDisplayedBytes | b'\x00\x00\x01\x02' | unhexlify('00000102') | b'\x00...' | N/A | ✅ |
| HD1 HexDump(Bytes(4)) | HexDumpDisplayedBytes | b'\x00\x00\x01\x02' | hexundump(...) | b'\x00...' | N/A | ✅ |

fmtstr size 变体验证：
- Hex(Int8ub): fmtstr=02X ✅
- Hex(Int16ub): fmtstr=04X ✅
- Hex(Int32ub): fmtstr=08X ✅
- Hex(Int64ub): fmtstr=016X ✅

**结论**：优化前后 display 对象的 `__str__`/`__repr__`/属性/类型/build 行为完全一致。

## 自检结果

| 检查项 | 结果 |
|--------|------|
| cargo build (PYO3_PYTHON=3.13) | ✅ 零 error |
| cargo clippy | ✅ 零 warning |
| cargo fmt --check | ✅ 格式正确 |
| cargo test --lib | ✅ 1372 passed, 0 failed |
| Python parity check | ✅ 全部通过 |
| maturin develop --release | ✅ 成功 |
| 无 unwrap/panic/TODO（非测试代码） | ✅ |
| 所有 pub 项有 /// 文档注释 | ✅ |
| 错误携带 path 字段 | ✅ |
| parse/build 对称性 | ✅（round-trip 验证通过） |

## 改动文件清单

1. `construct-rs/src/nodes/hex.rs`（struct 字段 + constructor + getter + parse int 分支 +
   模块文档注释 + HexDisplayClasses 文档注释 + 测试更新 + intern_fmtstr 测试辅助）
2. `construct-rs/src/compile.rs`（build_hex_node fmtstr 物化为 Py<PyString> + 注释更新）

## 偏离设计文档的地方

无偏离。方案 A+B 不改变公开接口（HexNode 是内部 Node 变体，Hex/HexDump Python 描述符
接口不变），不改语义/parity，是 ARCH 分析报告 §6 定义的 bug 级实现优化。
