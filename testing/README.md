---
id: README-testing
status: active
phase: meta
last_updated: 2026-07-29
---

# testing/ — 正式测试基础设施

> iter10（2026-07-29）引入。承载项目的正式测试基础设施（CI 门禁系统）。
> 与 `experiments/`（一次性实验）物理隔离。

## 目录结构

```
testing/
└── ci/                      # CI 冒烟门禁系统（META-CI 实施，ADR-020 沉淀）
    ├── run_smoke.ps1        # 冒烟总入口（-Level L1,L2,L3,L4）
    ├── run_l1_quality.ps1   # L1 质量门禁（cargo build + clippy + fmt + test）
    ├── run_l2_functional.ps1 # L2 功能门禁（Python parity 测试）
    ├── run_l3_perf.py       # L3 性能门禁（基准 + Controlled A/B Test）
    ├── run_l4_consistency.py # L4 一致性门禁（CSV ↔ 代码 ↔ 文档）
    ├── ab_test/             # Controlled A/B Test 框架（L-09 对策）
    │   ├── ab_bench.py
    │   ├── ab_harness.ps1
    │   └── ab_stats.py
    ├── known_exemptions.json # 门禁豁免清单
    ├── install_hook.ps1     # git pre-commit hook 安装器
    ├── pre-commit.template  # pre-commit hook 模板
    ├── lib_smoke.ps1        # 共享库
    ├── README.md            # CI 系统完整文档
    └── reports/             # CI 输出历史
```

## 使用方式

```powershell
# 完整门禁（L1+L2+L3+L4）
powershell -File testing/ci/run_smoke.ps1 -Level "L1,L2,L3,L4"

# 仅快速门禁（L1+L4）
powershell -File testing/ci/run_smoke.ps1 -Level "L1,L4"

# 安装 pre-commit hook
powershell -File testing/ci/install_hook.ps1
```

## 相关文档

| 类型 | 路径 |
|------|------|
| 设计文档 | `docs/design/基础设施/CI冒烟门禁设计.md` |
| 决策记录 | `docs/decisions/ADR-020-CI冒烟门禁方法学.md` |
| 过程记录 | `plans/meta/CI-冒烟门禁/traces/` |
| 教训 | `harness/experiences.md §L-09`（Controlled A/B Test 工程化） |
