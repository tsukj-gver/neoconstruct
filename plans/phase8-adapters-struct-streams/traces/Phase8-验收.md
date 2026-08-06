---
id: TRACE-phase8-accept
phase: "8"
task: "Phase 8 验收（部分通过）"
status: accepted-partial
last_updated: 2026-08-07
---

# Phase 8 验收（部分通过）

## 验收决策（2026-08-07）

PM 验收。用户决策：剩余 6 项加速比 <10x 的 case 接受为已知边界（"已不是关键的常用构造器"），Phase 8 **部分通过**，tag `phase-8-complete`。紧急推进 Phase 9（系统测试）/ Phase 10（SKILL）。

不再做 8.OPT-CASE-A/B——用户判断剩余 case 已不是关键常用构造器。

## 实现范围

22 构造器（P0 13 + P1P2 12）全部实现 + VET 通过。构造器清单（CSV）待同步 status。

## 性能现状（Python 3.13 非 abi3 + 8.OPT-SHARED 后）

**42/48 PASS（87.5%）**。6 项已知 LOW（下表）。基线对照：8.ENV（41/48）→ OPT-SHARED（CN1 LOW→OK，42/48）。

## 6 项已知 LOW（接受为已知边界）

| Case | 构造器 | 方向 | 加速比 | 根因（L-14 交叉验证 + Python 层实测）| 决策 |
|------|--------|------|------:|------|------|
| HX1 | Hex(Int) | parse | 7.77x | int 子类实例化 long_new ~55ns + setattr ~50ns（CPython 固有，Python 层实测）| 接受 |
| HX2 | Hex(Bytes) | parse | 8.67x | bytes 子类实例化（CPython 固有，buffer 复制）| 接受 |
| HD1 | HexDump(Bytes) | parse | 8.33x | 同 HX2（bytes 子类实例化）| 接受 |
| NT1 | NamedTuple | parse | 8.12x | namedtuple factory 实例化（collections.namedtuple `__new__`，CPython 固有）| 接受 |
| FE1 | FlagsEnum | parse | 9.59x | Container 4 字段构造（接近 10x，min/max 跨边界）| 接受 |
| FE1 | FlagsEnum | build | 5.92x | dict→int 转换本质开销（dict 遍历 + 位 OR）+ Python baseline 轻（~2471ns）| 接受 |

**根因充分性**：所有 6 项均经 PERF-REASSESS §1 profiling（Python 原版 87-98% 框架税，核心操作 ≤12.3%）+ L-14 交叉验证（每项 ≥2 替代路径）+ Python 层实测（long_new/setattr/namedtuple factory 的 ns 级开销）。属 CPython 子类实例化/factory 固有税，Python 原版也付此税（无法绕过 `type.__call__`）。

## 管道历程

P0（13 构造器）→ P1P2（12 构造器）→ HEX-OPT（HX1 4.36→6.46x）→ PERF-bench（48 测量点）→ PERF-retest（14/14 STRUCTURAL）→ BUGFIX（DF1/TM1/DF2）→ PERF-REASSESS（L-14 第 2 次，推翻"6-7x 合理目标"）→ 8.ENV（放弃 abi3，切 Python 3.13，PASS 33→41）→ 8.OPT-SHARED（ADR-023，L-14 第 3 次 managed dict，共享税 -47ns，CN1 LOW→OK）→ VET 通过。

## Evaluate 摘录（AHE §演化循环，Phase 级）

- phase: 8
- 关键产出: 22 构造器实现 + 共享税优化框架（ADR-023）+ Python 3.13 非 abi3 基线
- failures 对齐 L-XX: L-14 第 2/3 次触发（Hex 归因 + managed dict UB）；L-03 对策应用（VET 独立复测性能，PM 不采信 DEV 自测）
- outcome: 部分完成（42/48 PASS，6 项已知边界 LOW 接受）

## follow-up（不阻塞，记录待办）

1. **CSV 同步**：22 构造器 not_implemented → implemented（PM 主维护，分派执行中）
2. **设计质疑**：Union parsefrom 表达式路径未实现（常见用例全支持）
3. **设计质疑**：Timestamp / AlignedStruct Python 3.14 不兼容（P0 遗留；项目目标已改为 3.13，3.14 支持降级）
4. **VET NB2-1**：O2-A SAFETY 注释精度（PyBytes_Check vs CheckExact，等价）
5. **R1**：bench_stages.py / bench_breakdown.py abi3 环境变量残留清理
6. **MEMORY.md 更新**：阶段索引 Phase 8 → ✅ + Phase 9/10 立项
