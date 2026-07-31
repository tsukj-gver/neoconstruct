---
id: TRACE-phase8-P0-VET-REWORK
status: complete
phase: "8"
task: "8.P0 [DEV 修复 VET 驳回项 - Aligned sizeof]"
last_updated: 2026-07-31
---

# Phase 8 P0 VET 驳回修复记录

> **角色**：DEV
> **任务**：修复 VET 驳回项（Aligned sizeof 违反设计 D-P0-3）
> **依据**：`P0-VET审查.md` §1 关键发现 1（驳回）+ PM 决策（接受 Probe FieldName + 接受 Checksum .to_vec()）
> **返工范围**：仅 `aligned.rs` sizeof + 连带 `mod.rs` has_expressions + `schema.rs` static_size 验证

## PM 决策对齐（2 项设计层面，无需 DEV 修复）

| 项 | PM 决策 | DEV 行动 |
|----|---------|---------|
| Probe.into 设计质疑（D-P0-2） | 接受 FieldName 方案 | 无（设计文档 ARCH 后续更新） |
| Checksum .to_vec() | 接受当前实施（1.84x 已达标） | 无（.to_vec() 消除作为后续优化） |

## 必须修复（VET 驳回项）

### Aligned sizeof 违反设计 D-P0-3

**问题**：`aligned.rs` sizeof 统一返回 Err，即使 modulus 是编译期常量（`ExprOp::Const(4)`）也失败。违反设计 §4.3.2 边界 AL-3/4/5 + §10 D-P0-3 决策。

**修复内容**：

#### 1. `aligned.rs` sizeof 实现（L156-180）

按 D-P0-3 实现 modulus 编译期常量检测：

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

- 编译期常量 modulus（`[ExprOp::Const(n)]` 且 `n >= 2`）：计算 `inner.sizeof + pad`（对齐 Python AL-3/4/5）
- 非常量 modulus 或非法常量（`< 2`）：返回 Err（对齐 Python SizeofError）
- `ExprOp` 导入提升到模块顶部（原仅 `#[cfg(test)]` 内导入）

#### 2. `aligned.rs` 新增 `has_expressions` 方法（连带修复）

**关键发现**：仅修复 sizeof 不足以恢复 Struct static_size 预分配。`mod.rs` L433 原 `Node::Aligned(_) => true` 让 Aligned 字段总标记为含表达式，导致 `schema.rs` L86-90 `static_size` 永远为 None（即使 sizeof 修复后能成功计算）。

按 PointerNode 模式（`pointer.rs` L115-117）实现：

```rust
fn modulus_is_const(&self) -> bool {
    matches!(self.modulus.ops(), [ExprOp::Const(_)])
}

pub fn has_expressions(&self) -> bool {
    !self.modulus_is_const() || self.inner.has_expressions()
}
```

`mod.rs` L433 改为委托：`Node::Aligned(a) => a.has_expressions()`。

**修复链**：
1. `compile.rs` L253 `fields.iter().any(|f| f.node.has_expressions())` 对 Aligned(常量 modulus) 字段返回 false
2. `StructNode.has_expressions` 编译期存储为 false（若其他字段也无表达式）
3. `schema.rs` L84 `root.has_expressions()=false` → L86 计算 static_size
4. `Aligned.sizeof` 现在成功 → `static_size=Some(n)`
5. `_build_raw` 用 `BuildStream::with_capacity(n)` 预分配（恢复性能优化）

#### 3. `aligned.rs` 测试更新

删除错位测试 `sizeof_returns_err_runtime_modulus`（用 Const(4) 却断言 Err），新增 9 个测试：

| 测试 | 覆盖 |
|------|------|
| `sizeof_const_modulus_returns_inner_plus_pad` | AL-3（Int16ub → 4） |
| `sizeof_const_modulus_pad_one` | AL-4（Bytes(3) → 4） |
| `sizeof_const_modulus_no_pad_when_aligned` | AL-5（Bytes(4) → 4） |
| `sizeof_const_modulus_other_values` | 补充（modulus 8 + Int8ub → 8） |
| `sizeof_runtime_modulus_returns_err` | D-P0-3 非常量（GetInt）→ Err |
| `sizeof_const_modulus_below_two_returns_err` | 防御性（Const(1) → Err） |
| `has_expressions_const_modulus_no_expr_inner_returns_false` | D-P0-3 委托基础 |
| `has_expressions_runtime_modulus_returns_true` | GetInt → true |
| `has_expressions_const_modulus_with_expr_inner_returns_true` | Const + Rebuild inner → true（递归） |

#### 4. `schema.rs` static_size 端到端验证（L317-388）

新增 2 个测试验证 static_size 预分配恢复：

| 测试 | 预期 |
|------|------|
| `static_size_restored_when_aligned_has_const_modulus` | `Some(4)`（Int16ub=2 + pad=2） |
| `static_size_none_when_aligned_has_runtime_modulus` | `None`（GetInt modulus → SizeofError） |

## 自检结果

| 项 | 结果 | 数据 |
|----|------|------|
| cargo build --lib | ✅ | 零 error |
| cargo clippy --lib | ✅ | 零 warning |
| cargo fmt --check | ✅ | 格式正确 |
| cargo test --lib | ✅ | **1302 passed, 0 failed**（原 1297 + 新增 11：aligned 9 + schema 2） |
| cargo build --release | ✅ | maturin develop --release 成功（cp314） |
| pytest 回归 | ✅ | **764 passed, 20 skipped**（与 DEV 实施 baseline 一致，零回归） |

## 修改文件清单

| 文件 | 修改类型 | 行数变化 |
|------|---------|---------|
| `construct-rs/src/nodes/aligned.rs` | sizeof 修复 + has_expressions 新增 + ExprOp 导入 + 9 测试 | +108 |
| `construct-rs/src/nodes/mod.rs` | L433 Node::Aligned 委托 has_expressions | +4/-2 |
| `construct-rs/src/schema.rs` | 2 个 static_size 端到端测试 | +72 |

## VET 返工审查范围对照

按 `P0-VET审查.md` §"返工审查范围（DEV 修复后）"：

| 返工项 | 状态 |
|--------|------|
| aligned.rs sizeof 修复 | ✅ 完成 |
| 测试断言更新（sizeof_returns_err_runtime_modulus） | ✅ 完成（删除并替换为 6 个 sizeof 测试） |
| AL-3/4/5 测试用例新增 | ✅ 完成（3 个 + 1 补充） |
| Struct static_size 预分配路径恢复验证（schema.rs L195-202） | ✅ 完成（2 个端到端测试） |

## 可重交 VET 复审

仅 `aligned.rs` + `mod.rs` L433 + `schema.rs` 测试模块变更。建议 VET 复审重点：
1. `aligned.rs` sizeof D-P0-3 实现正确性（编译期常量检测边界）
2. `aligned.rs` has_expressions 委托逻辑（与 PointerNode 同模式）
3. `schema.rs` static_size 端到端验证覆盖完整
