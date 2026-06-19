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
