---
id: README-experiments
status: active
phase: meta
last_updated: 2026-07-29
---

# experiments/ — 一次性实验

> iter10（2026-07-29）澄清目录语义：本目录**仅用于一次性实验**（验证方案可行性 / 历史 benchmark / 调查快照）。
> 正式测试基础设施在 `testing/ci/`；过程记录在 `plans/`；harness 资产在 `harness/`。

## 当前内容

| 子目录/文件 | 类型 | 说明 |
|------------|------|------|
| `bench_bits/` | 历史 Rust 实验 | BitStream benchmark 实验（Phase 3 期间），含 target/ 编译产物 |
| `bench_phase33/` | 历史 Rust 实验 | Phase 3.3 Rust benchmark 实验，含 target/ |
| `phase4_investigation_data/` | 调查数据 | Phase 4 性能调查测量数据 |
| `_snapshot_4x/` | DLL 快照 | 4.x 错误路径 Controlled A/B Test 的 DLL 对照快照 |
| `phase1_smoke.py` | 一次性脚本 | Phase 1 冒烟验证 |
| `phase2_smoke.py` | 一次性脚本 | Phase 2 冒烟验证 |
| `phase4_4x_t6_verify.py` | 一次性脚本 | Phase 4.x T6 验证 |
| `phase4_repeat_until_examples.py` | 一次性脚本 | RepeatUntil 示例验证 |
| `phase4_bench_summary.py` | 一次性脚本 | Phase 4 benchmark 汇总 |

## 使用原则

- ✅ **放这里**：验证某方案是否可行的实验脚本 / 一次性调查数据 / 历史 benchmark
- ❌ **不放这里**：正式测试用例（→ `construct-rs/tests/`）/ CI 基础设施（→ `testing/`）/ 持续维护的 benchmark（→ `construct-rs/benches/`）

## 清理建议

- `bench_bits/target/` 和 `bench_phase33/target/` 是 Rust 编译产物，可加入 .gitignore 或定期清理
- 历史脚本（phase*_smoke.py）保留作参考，不再活跃维护
