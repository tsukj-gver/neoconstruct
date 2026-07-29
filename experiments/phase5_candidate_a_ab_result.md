# Phase 5 候选 A Controlled A/B Test 结果

测量时间：2026-07-29T20:00:45
Python：3.14.2
采样：NUMBER=200000, REPEAT=7, A/B 交替 ROUNDS=3

## 场景

- B1：3 个 Int8ub 字段 Struct（无表达式，无 post_init）
- A 状态：StructMixin.parse classmethod 路径（当前实现）
- B 状态：`cls.parse = schema._parse_raw` fast binding（候选 A）

## 统计汇总

- A 状态 min: 265.56 ns/op, median: 265.85 ns/op
- B 状态 min: 204.81 ns/op, median: 206.56 ns/op
- 候选 A 实际节省: min-min +60.75 ns, med-med +59.29 ns

## 对比设计 §7.1

- 设计估算：30-50 ns
- 实测：60.7 - 59.3 ns

## 逐轮原始数据

| 轮次 | A min ns | B min ns | Δ (B-A) |
|-----:|---------:|---------:|--------:|
| 1 | 265.85 | 205.65 | -60.20 |
| 2 | 266.31 | 206.68 | -59.63 |
| 3 | 265.67 | 206.70 | -58.97 |