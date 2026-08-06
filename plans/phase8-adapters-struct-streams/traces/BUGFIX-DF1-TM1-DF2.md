---
id: TRACE-8.BUGFIX
phase: "8"
task: "8.PERF-RETEST [bug 修复]"
status: coding
owners: [DEV]
started: 2026-07-31
completed: 2026-07-31
manifest_refs: []
---

# 8.PERF-RETEST bug 修复 — DF1 / TM1 / DF2

## 背景

Phase 8 PERF bench 过程发现 3 个功能缺陷（PERF-bench.md [设计质疑]）。
PM 在 8.PERF-RETEST 任务中要求修复后才能验收。

## 修复清单

| Bug | 描述 | 根因 | 修复方式 |
|-----|------|------|----------|
| DF1 | `Default(Byte, 0)` 常量值编译失败 | `_extract_and_compile_exprs` 对 int 常量跳过编译，但 Rust DefaultNode 需要 ExprProgram | Python 侧：int 常量编译为 `[("const", N)]` |
| TM1 | `rfield(Terminated)` build 失败 | `compute_ro_value` 不认 Terminated 为合法 RO 节点 | Rust 侧：添加 `Node::Terminated` → `Ok(py.None())` |
| DF2 | `rfield(Default(...))` build 失败 | 同 TM1，`compute_ro_value` 不认 Default 为合法 RO 节点 | Rust 侧：添加 `Node::Default` → 求值 value 表达式 |

## DF1 修复详情

### 根因

`_extract_and_compile_exprs`（`_mixin.py` L609-618）对 `_expr_params` 返回的 int 常量
跳过编译（`# 常量值（int/bytes/str）不编译`）。但 `DefaultDescriptor._expr_params`
返回 `{"value": self.value}`，当 value 是 int 常量（如 `Default(Byte, 0)` 的 `0`）时，
不会生成 ExprProgram。

Rust 侧 `build_default_node`（`compile.rs` L3304-3316）从 `expr_programs[field_index]["value"]`
取 ExprOp 列表。如果 Python 侧未编译，`expr_programs[field_index]` 为空 → CompilationError。

### 修复

在 `_extract_and_compile_exprs` 中添加 int 常量编译分支：

```python
elif isinstance(param_value, int) and not isinstance(param_value, bool):
    # DF1 修复：int 常量编译为单条 Const ExprOp
    result[param_name] = [("const", param_value)]
```

设计依据：`DefaultDescriptor` docstring（L2289-2290）明确写道
"int 常量也包装为单条 Const，与 Rebuild func 同模式"。

### 影响范围分析

| 描述符 | _expr_params 参数 | int 常量行为 | Rust 读取方式 | 修复后影响 |
|--------|-------------------|-------------|-------------|-----------|
| DefaultDescriptor | `{"value": int/Expr}` | 修复前：跳过 → 编译失败 | `expr_programs[idx]["value"]` | ✅ 正确编译 |
| CheckDescriptor | `{"func": int/Expr}` | 修复前：跳过 → 编译失败 | `expr_programs[idx]["func"]` | ✅ 正确编译（附带修复） |
| BytesDescriptor | `{"length": int/Expr}` | 修复前：跳过 → Rust 从描述符读取 | `desc.length` extract usize（优先）| 冗余但无害 |
| ComputedDescriptor | `{"func": Expr}` | 不出现 int | `expr_programs[idx]["func"]` | 无影响 |
| RebuildDescriptor | `{"func": Expr}` | 不出现 int | `expr_programs[idx]["func"]` | 无影响 |

## TM1 修复详情

### 根因

`compute_ro_value`（`mod.rs` L544-595）的 match 不含 `Node::Terminated` 分支，
fallback 到 `_ => Err(...)` 报 "is not a valid RO node"。

### 修复

在 `compute_ro_value` 添加：

```rust
Node::Terminated(_) => Ok(py.None()),
```

Terminated::build 是 no-op（不写字节，不从 obj 取值），与 StopIf/Check 同模式。
`compute_ro_value` 返回 Py_None 作为占位值，随后 StructNode 调 TerminatedNode.build
执行 no-op。

## DF2 修复详情

### 根因

同 TM1，`compute_ro_value` 不含 `Node::Default` 分支。

### 修复

在 `compute_ro_value` 添加：

```rust
Node::Default(d) => {
    let v = crate::expr::eval_expr_int(d.value(), ctx, py)?;
    Ok(v.into_py(py))
}
```

Default 作为 RO 字段时，`compute_ro_value` 求值 value 表达式（与 Computed/Rebuild 同模式）。
随后 StructNode 将结果传给 DefaultNode.build，因 obj 非 None，DefaultNode 直接转发 inner.build。

## 文件变更

| 文件 | 类型 | 说明 |
|------|------|------|
| `construct-rs/python/construct/_mixin.py` | 修改 | DF1：`_extract_and_compile_exprs` 添加 int→Const 编译 |
| `construct-rs/src/nodes/mod.rs` | 修改 | TM1/DF2：`compute_ro_value` 添加 Terminated/Default 分支 |
| `construct-rs/src/nodes/struct_node.rs` | 修改 | 新增 2 个 Rust 单元测试 |
| `construct-rs/tests/unit/test_expr_compile.py` | 修改 | 更新/新增 Python 单元测试 |
| `construct-rs/bench/bench_phase8.py` | 修改 | 移除 workaround，使用正确模式 |

## 新增测试

### Rust 单元测试（struct_node.rs）

- `ro_terminated_field_build_is_noop`：Terminated 作为 RO 字段 build 成功，不写字节
- `ro_default_field_build_uses_constant_value`：Default(Byte, 0) 作为 RO 字段 build 求值 Const(0)

### Python 单元测试（test_expr_compile.py）

- `test_descriptor_with_int_constant_param_compiles_as_const`：int 4 → `[("const", 4)]`
- `test_descriptor_with_int_constant_zero`：int 0 → `[("const", 0)]`
- `test_descriptor_with_negative_int_constant`：int -1 → `[("const", -1)]`
- `test_descriptor_with_non_int_constant_returns_empty`：bytes 不编译
- 更新 `test_descriptor_with_mixed_params`：int 常量也编译

### 端到端验证

```
DF1: Default(Byte, 0)
  parse(b'\x05') → val=5 ✅
  build(val=None) → b'\x00' ✅
  build(val=9) → b'\x09' ✅

TM1: rfield(Terminated)
  parse(b'\x42') → a=0x42 ✅
  build(a=0x42) → b'\x42' ✅

DF2: rfield(Default(Byte, 0))
  parse(b'\x07\x00') → a=7, b=0 ✅
  build(a=7) → b'\x07\x00' ✅
```

## 自检结果

| 检查项 | 结果 |
|--------|------|
| cargo build --release | ✅ 零 error |
| cargo clippy --release | ✅ 零 warning |
| cargo fmt --check | ✅ 格式正确 |
| cargo test --release | ✅ 1373 passed, 0 failed |
| pytest tests/ | ✅ 767 passed, 20 skipped (pre-existing skips) |
| maturin develop --release | ✅ 安装成功 |
| 端到端验证（DF1/TM1/DF2） | ✅ 全部通过 |
| parse/build 对称性 | ✅ 所有修复均验证 parse+build |
| 无 unwrap()/expect() 在非测试代码 | ✅ |
| 无 TODO/FIXME | ✅ |
| 所有 pub 项有 /// 文档注释 | ✅ |
| 错误携带 path 字段 | ✅ |
