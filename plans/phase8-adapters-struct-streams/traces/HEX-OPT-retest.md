---
id: TRACE-8.HEX-OPT-RETEST
phase: "8"
task: "8.HEX-OPT [Hex parse Plan A+B Controlled A/B Test 复测]"
status: coding
owners: [DEV]
started: 2026-07-31
completed: 2026-07-31
manifest_refs: []
---

# 8.HEX-OPT-RETEST — Hex parse 性能优化 Controlled A/B Test 复测报告

## 测量环境

| 维度 | 值 |
|------|-----|
| 测量时段 | 2026-07-31 21:12 |
| rs venv | crs_venv_new (Python 3.14.2) |
| py venv | crs_venv_py_new (Python 3.14.2) |
| 构建方式 | maturin develop --release（PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1） |
| number | 5000（timeit 内循环） |
| iterations | 10（每轮采样数，取 median） |
| warmup | 3（预热丢弃） |
| rounds | 5（交替 rs↔py 配对轮数） |

## 对照组设计

| 对照类型 | Case | 预期 | 用途 |
|---------|------|------|------|
| 阳性对照 | UN1 Union parse (初测 11.56x) | ≥10x | 验证测量系统正常 |
| 阴性对照 | CN1 Const build (初测 11.82x) | ≥10x | 验证环境漂移幅度 |

## 对照组结果

### 阳性对照（验证测量系统）

| Case | Mean | Min | Max | Std | 判定 |
|------|------|-----|-----|-----|------|
| UN1 | 11.52x | 11.46x | 11.62x | 0.07 | ✅ ≥10x 稳定 |

### 阴性对照（验证环境漂移）

| Case | Mean | Min | Max | Std | 判定 |
|------|------|-----|-----|-----|------|
| CN1 build | 11.51x | 11.21x | 11.98x | 0.30 | ✅ ≥10x 稳定 |

**结论**：测量系统正常。对照组全部 ≥10x 且 std 极低（0.07-0.30），证明环境噪声可控、
测量系统可靠。Pre-opt PERF-retest.md（std 0.05-0.31）与本次（std 0.07-0.30）在同一
环境漂移量级内，跨时段对比有效。

## 目标 case 复测结果：优化前 vs 优化后

| Case | 构造器 | 方向 | rs_pre | rs_post | py_ns | sp_pre | sp_post | Δ Speedup | rs_改善 | Min | Max | Std |
|------|--------|------|--------|---------|-------|--------|---------|-----------|---------|-----|-----|-----|
| **HX1** | Hex(Int32ub) | parse | 581ns | 388ns | 2508ns | 4.36x | **6.46x** | **+2.10x** | **-193ns** | 6.31x | 6.63x | 0.11 |
| HX2 | Hex(Bytes(4)) | parse | 330ns | 322ns | 2253ns | 6.87x | **6.99x** | +0.12x | -8ns | 6.94x | 7.06x | 0.05 |
| HD1 | HexDump(Bytes(4)) | parse | 324ns | 318ns | 2225ns | 6.88x | **6.99x** | +0.11x | -6ns | 6.86x | 7.21x | 0.14 |

## 分析

### HX1（方案 A+B 主目标）：4.36x → 6.46x（+2.10x）

- **Rust 侧时间从 581ns 降至 388ns**（-193ns，-33%）
- 完全符合 ARCH 预测（~350-400ns，~6.3-7.2x）
- 可证伪条件检查：HX1 优化后 388ns，在预测范围 [350, 400] 内 ✅
- **193ns 改善来源**（与 ARCH §6.3 预测一致）：
  - 方案 A（消除 call_method1 字节码进入）：~100-130ns
    - getattr(cls, "new") + staticmethod 描述符 + ceval dispatch
  - 方案 B（消除每次 PyString::new）：~60ns
    - String clone ~15ns + PyString::new ~45ns
  - 合计 ~160-190ns，实测 193ns（FFI 放大略超预期，在合理范围内）

### HX2/HD1（bytes 路径，预期改善有限）

- HX2：330→322ns（-8ns），6.87x→6.99x（+0.12x）
- HD1：324→318ns（-6ns），6.88x→6.99x（+0.11x）
- **改善极小（~6-8ns），完全符合 ARCH 预测**（§6.3：HX2/HD1 改善有限，bytes 路径
  已是 C 级 call1，方案 A 不适用）
- 微小改善可能来自 LTO/codegen 变化（crate 整体重编译）或测量噪声（~6-8ns 在
  std=0.05-0.14 的波动范围内）

### 合理目标加速比确认

| Case | 优化后 | ARCH §6.4 合理目标（6-7x） | 判定 |
|------|--------|--------------------------|------|
| HX1 | 6.46x | 6-7x 区间 | ✅ 达到合理目标 |
| HX2 | 6.99x | 6-7x 区间 | ✅ 达到合理目标 |
| HD1 | 6.99x | 6-7x 区间 | ✅ 达到合理目标 |

**结论**：Hex 族 parse 全部从实现低效区间（HX1 4.36x）提升到合理目标区间（6.46-6.99x）。
不达 10x 的原因是 CPython 显示对象构造固有税（int/bytes 子类实例化 + setattr fmtstr），
约 ~275-300ns 不可优化部分（ARCH §6.4）。此为结构性边界，非实现问题。

## parity 变化

**无 parity 变化**。优化前后 display 对象的 `__str__`/`__repr__`/`fmtstr` 属性/类型/build
行为完全一致（详见 HEX-OPT-DEV.md parity 确认段落）。

## 详细数据（5 轮采样）

### HX1 Hex(Int32ub) parse

| Round | rs ns | py ns | Speedup |
|-------|-------|-------|---------|
| 1 | 390 | 2461 | 6.31x |
| 2 | 390 | 2533 | 6.49x |
| 3 | 388 | 2492 | 6.42x |
| 4 | 390 | 2520 | 6.46x |
| 5 | 384 | 2545 | 6.63x |
| **mean** | **388** | **2510** | **6.46x** |

### HX2 Hex(Bytes(4)) parse

| Round | rs ns | py ns | Speedup |
|-------|-------|-------|---------|
| 1 | 321 | 2250 | 7.01x |
| 2 | 324 | 2248 | 6.94x |
| 3 | 321 | 2269 | 7.06x |
| 4 | 322 | 2249 | 6.98x |
| 5 | 324 | 2248 | 6.95x |
| **mean** | **322** | **2253** | **6.99x** |

### HD1 HexDump(Bytes(4)) parse

| Round | rs ns | py ns | Speedup |
|-------|-------|-------|---------|
| 1 | 320 | 2239 | 6.99x |
| 2 | 320 | 2198 | 6.87x |
| 3 | 309 | 2227 | 7.21x |
| 4 | 322 | 2210 | 6.86x |
| 5 | 321 | 2251 | 7.01x |
| **mean** | **318** | **2225** | **6.99x** |

## 测量脚本

- `bench/bench_hex_opt_ab_retest.py`（本次复测脚本）
- `bench/results/bench_hex_opt_ab_retest.json`（原始数据）

## 总结

| 维度 | 结论 |
|------|------|
| HX1 改善 | 4.36x → 6.46x（+2.10x），193ns 省下，符合预测 |
| HX2 改善 | 6.87x → 6.99x（+0.12x），改善极小（bytes 路径无可优化，符合预测） |
| HD1 改善 | 6.88x → 6.99x（+0.11x），改善极小（同 HX2） |
| parity 变化 | 无（display 对象行为完全一致） |
| 对照组 | 阳性 UN1 11.52x + 阴性 CN1 build 11.51x，均稳定 ≥10x |
| 合理目标 | Hex 族 parse 全部在 6-7x 合理目标区间（显示对象构造固有税） |
| ARCH 预测验证 | HX1 388ns 在 [350, 400] 预测范围内 ✅ |
