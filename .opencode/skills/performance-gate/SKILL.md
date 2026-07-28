# Performance Gate

Use this skill when any work involves performance optimization, performance-critical refactoring,
FFI design, or when a Phase defines S-PERF acceptance criteria.

This skill enforces three checkpoints that prevent the class of failure where a refactor is
architecturally correct but does not actually achieve its performance goal.

## When to trigger

- Any Phase with S-PERF criteria in the 总纲
- Any design document that claims performance improvement
- Any work involving FFI / Python C API / pyo3
- Any refactor motivated by a performance problem
- PM acceptance of any Phase containing S-PERF criteria

## Checkpoint 1: Falsifiable Performance Hypothesis (DESIGNING → DESIGN_REVIEW)

**Who**: ARCH must write it, REV must verify it.

Every design document for performance-critical work MUST contain a section:

```markdown
## 性能假设

### 瓶颈识别（必须量化）
- 当前系统的性能瓶颈，附 profiling / benchmark 数据
- 不接受纯理论推算（"我们认为 FFI 是瓶颈" → ❌）
- 必须有数据来源（"Phase 10 cargo bench: 0.199x, profile 显示 70% 在 FFI" → ✅）

### 机制说明（因果链）
- 本设计如何解决该瓶颈
- 必须是具体的因果链，不是泛泛的"更高效"

### 可证伪预测（必须可测量）
- 一个具体的、实现后可验证的声明
- 必须覆盖所有瓶颈来源，不只是被优化的那条路径
- 反例声明：如果预测为假，设计需要怎样修改

### 验证方法
- 实现后如何验证预测（具体工具/命令）
- 验证时机（哪个子任务完成后）
```

### REV 检查项（硬性，不通过则驳回）

```
[ ] "瓶颈识别"有量化数据支撑（非纯理论推算）
[ ] "可证伪预测"覆盖了所有性能瓶颈来源（不仅被优化的路径）
    - 检查方法：列出设计中提到的所有 FFI/拷贝/转换来源，
      确认每个来源都在"可证伪预测"中有对应的预测
[ ] 如果预测为假，有明确的应对方案
```

### 常见陷阱（REV 必须警惕）

1. **优化了 A 路径但忽略了 B 路径**：
   - 设计声称"消除表达式求值的 FFI"（✅ 确实消除了）
   - 但忽略了"PyDict_SetItem 仍是 per-field FFI"（❌ 未分析）
   - REV 必须检查：设计是否列出了**所有** FFI/瓶颈来源？

2. **理论估算替代实证数据**：
   - "预计 5000x 加速" → ❌ 这是理论推算，不是测量
   - "Phase 10 profile 显示 70% 在 FFI 边界" → ✅ 有数据来源

3. **相对基线而非绝对基线**：
   - "比旧路径快 1.2x" → 如果旧路径是 0.2x，1.2x 改善仍是 0.24x
   - REV 必须检查：瓶颈识别的基线是目标系统（Python 原版）还是自己？

## Checkpoint 2: Performance Smoke Test (首个端到端实现后)

**Who**: PM 强制执行，DEV 执行测量。

当性能关键路径的**首个端到端实现**完成后（能从 Python 用户层调用 parse/build），
PM 必须在分派后续阶段之前执行一次性能烟雾测试。

### 烟雾测试要求

1. 选择 ≥2 种格式复杂度（如：3 字段 flat Struct + 100 元素 Array）
2. 对比 **目标基线**（Python 原版 or 现有最佳实现），不是旧路径
3. 用 Python `time.perf_counter` 端到端测量（用户视角）
4. 迭代次数 ≥1000

### 判定标准

```
vs 目标基线 ≥ 0.8x → 🟢 绿灯：继续后续阶段
vs 目标基线 0.5x ~ 0.8x → 🟡 黄灯：记录差距，可继续但需在后续阶段关注
vs 目标基线 < 0.5x → 🔴 红灯：暂停后续阶段，回 ARCH 分析原因
```

### 红灯处理

1. PM 暂停当前 Phase 的后续子任务分派
2. PM 分派 ARCH 做"性能差距分析"（profiling + 瓶颈定位）
3. ARCH 产出修正方案（可能是设计变更或实现优化）
4. 修正方案走 DESIGNING → DESIGN_REVIEW 流程
5. 修正通过后才恢复后续阶段

### 烟雾测试脚本模板

```python
import time

# 目标基线（Python 原版或现有最佳）
# ... setup ...

iterations = 1000

# Parse benchmark
start = time.perf_counter()
for _ in range(iterations):
    baseline_fmt.parse(test_data)
baseline_time = time.perf_counter() - start

start = time.perf_counter()
for _ in range(iterations):
    new_path.parse(test_data)
new_time = time.perf_counter() - start

ratio = new_time / baseline_time  # < 1.0 = faster than baseline
print(f"ratio: {ratio:.2f}x ({'faster' if ratio < 1 else 'slower'} than baseline)")
```

## Checkpoint 3: Acceptance Evidence Verification (验收阶段)

**Who**: PM 执行。

### 硬规则（不可覆盖）

```
IF 验收标准定义为"≥X metric"
AND DEV 报告的证据类型不包含"实际测量数据"
THEN PM MUST 驳回，要求补测
AND PM MUST NOT 标注"通过"
AND PM MUST NOT 打 tag
```

### 证据类型要求

| 标准类型 | 要求的证据 | 不可接受的证据 |
|---------|-----------|-------------|
| S-PERF "≥1.0x" | 对比数据表（多格式 × 多方向） | "bench 编译通过" / "1 个 test passed" |
| S-FUNC "功能覆盖" | 测试结果汇总（pass/fail 计数） | "代码已实现" |
| S-QUAL "零 warning" | clippy/fmt 命令输出 | "我觉得没问题" |
| S-ARCH "D1-D7 实现" | 逐项源码位置确认 | "设计文档里写了" |

### 验收流程

1. PM 读取总纲的验收标准
2. 对每条标准，检查 DEV/VET 报告中的证据类型
3. 证据类型匹配 → 检查数值是否达标
4. 证据类型不匹配 → 驳回
5. 全部达标 → 才可执行 git tag

## 历史教训

Phase 15 验收失败案例（2026-06-19）：

- **标准**：S-PERF-2 "Python 路径 ≥1.0x"
- **报告**："bench 编译通过，无对比基准"
- **PM 行为**：接受并打 tag
- **实际性能**：parse 0.26x / build 0.08x vs Python 原版
- **根因**：PyDictSink 仍 per-field 调 PyDict_SetItem，FFI 调用次数未减少
- **教训**：
  1. 设计的"可证伪预测"只分析了表达式 FFI，未分析数据操作 FFI
  2. REV 接受了理论推算（"5000x 加速"）而非要求实证数据
  3. 首个实现后无烟雾测试检查点
  4. PM 接受了"没测对比"作为"对比达标"

Phase 4 E01 O1 回归根因调查案例（2026-07-28，对应 `experiences.md §L-09`）：

- **质疑**：用户提出 E01 从 4.7 PM 验收 10.56x 变为 4.6 VET 复测 9.51x（-1.05x），可能是 O1 引入
- **VET 之前 H1 排查**："O1 只改 stop_if.rs，E01 不调用 StopIf，cargo 未重编译"——论断事实错误（实测 cargo 重编译产生不同 DLL）
- **ARCH Controlled A/B Test**：6 次交替测量证实 E01 O1 off vs on Δ=+0.023x（远 <0.3x 波动阈值）；阴性对照 i01 同会话出现 0.29x 摆动证明环境漂移足以解释观察差距
- **根因**：17 天跨度（07-11→07-28）测量环境漂移；E01 真实稳态 9.4-9.7x
- **教训**：跨时段性能对比的"消除法归因"在边界场景（Rust 侧 <300ns）失效，必须用 Controlled A/B Test

## Checkpoint 4: Cross-Time Performance Comparison (跨时段性能对比 / 回归判定)

**Who**: 任何怀疑性能回归的角色（VET / PM / 用户）触发，ARCH 或 VET 执行 Controlled A/B Test。

**规范来源**：`experiences.md §L-09`（跨时段性能对比消除法归因失效）

### 触发场景

- 跨时段性能数据对比（如本 phase VET 复测 vs 上 phase PM 验收基线）
- 边界场景（Rust 侧 <300ns / 加速比 9-11x 边缘）
- 任何"X 改动 vs Y 基线"的回归判定

### 测量规范（必须遵守）

1. **跨时段对比必须标注测量环境**：
   - Python 版本 / venv 重建状态 / 测量时段 / CPU 型号 / OS 版本 / 环境变量
   - 跨时段数据必须在报告里附"测量环境差异"段落

2. **边界场景多次采样**：
   - Rust 侧 <300ns 的场景必须 ≥5 次采样
   - 报告统计区间（min/max/mean/stddev），不可仅用单点值

3. **消除法归因的"未改"假设必须实测验证**：
   - "X 没改 → X 不是原因"的论证需要实测验证（DLL hash / 汇编对比 / git stash 验证）
   - 不可凭直觉判定"crate 整体未受影响"——crate inlining / codegen units / LTO 决策可能传播影响

### Controlled A/B Test（怀疑回归时必做）

当出现跨时段性能差距 ≥0.5x 时，**必须**做 Controlled A/B Test，不可依赖消除法归因或跨时段单次测量对比：

```bash
# 在当前 HEAD 上做对照实验
# A. O1 off（或质疑改动的 off 状态）
git stash push -- <质疑改动文件>
cargo build --release && maturin develop --release
# 跑 benchmark 5 次（min(repeat=5) × number），记录每次结果

# B. O1 on（或质疑改动的 on 状态）
git stash pop
cargo build --release && maturin develop --release
# 跑 benchmark 5 次

# C. 交替测量消除时间漂移
# 重复 A→B→A→B→A→B 至少 3 轮（共 6 次测量，每种条件 3 次）
```

### 对照组要求（必做）

- **阳性对照**：与质疑改动相关、预期应有效应的场景（验证 toggle 真实性）
- **阴性对照**：与质疑改动完全无关的场景（验证环境漂移幅度）

> 没有对照组的 A/B Test 不构成因果证据。

### 结论判据

| 差异 | 判定 | 处理 |
|------|------|------|
| >0.5x | O1 引入回归 | 进 Step 2 排查 crate inlining 机制 + 设计修复 |
| <0.3x | 测量波动 | 不需修复；记录统计学证据 |
| 0.3-0.5x | 中间情况 | 加测 5 轮，看趋势 |

### REV / VET 检查项（硬性，不通过则驳回）

```
[ ] 跨时段性能对比报告含"测量环境差异"段落
[ ] 边界场景（Rust 侧 <300ns）有 ≥5 次采样 + 统计区间
[ ] 怀疑回归时执行了 Controlled A/B Test（含交替测量 + 阳性对照 + 阴性对照）
[ ] 消除法归因的"未改"假设有实测验证（不可仅凭直觉）
```

### pm-performance-validation skill 整合

PM 在 `pm-performance-validation` skill 中触发本 Checkpoint 4 的"内部数据关系"维度时，必须检查：
- 跨时段数据是否标注环境差异
- 边界场景是否有统计区间
- 若仅有跨时段单次对比 → 触发本 Checkpoint 4，要求重做 Controlled A/B Test

### 工具支持（参考 `experiments/E01_O1_ab_*`）

Phase 4 E01 调查产物可作为 Controlled A/B Test 的参考实现：
- `experiments/E01_O1_ab_bench.py`：测量脚本
- `experiments/E01_O1_ab_harness.ps1`：交替测量 harness
- `experiments/E01_O1_ab_stats.py`：统计分析（含 welch_t 检验）
