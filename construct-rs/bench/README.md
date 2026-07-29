# construct-rs Benchmark 套件

> 性能基准测试集合：construct-rs vs Python construct 2.10.70 加速比测量。

## 目录结构

```
bench/
├── _helpers/              # 公共组件（设计 §2.5-§2.7）
│   ├── runner.py          # BenchRunner（统一子进程计时 + 统计）
│   ├── stats.py           # welch_t / speedup_ratio / ConfidenceInterval
│   └── report.py          # BenchReport + check_gates + format_results_table
├── bench_struct.py        # Struct B1-B7（Phase 1 基线，用 BenchRunner）
├── bench_expr.py          # 表达式 E1-E3（Phase 2）
├── bench_bitstream.py     # BitStream 场景（Phase 3）
├── bench_stages.py        # 分阶段计时（Phase 5 内核开销分解）
├── bench_breakdown.py     # 性能根因验证（full parse / _parse_raw 分解）
└── results/               # bench 输出归档（JSON + Markdown + txt）
```

## 运行方式

所有 bench 脚本需在能访问两个 construct 实现的环境运行：
- **construct-rs**：通过 `construct-rs/python/` 目录（sys.path 注入）
- **Python construct 2.10.70**：通过 site-packages（pip install construct==2.10.70）

推荐用专门 venv（`bench_struct.py` 已内置三级 venv 解析）：

```sh
# bench_struct.py（推荐，用 BenchRunner + 专门 venv）
python bench/bench_struct.py
python bench/bench_struct.py --case B1 --case B4
python bench/bench_struct.py --iterations 10

# 其他 bench（保留原 sys.executable 逻辑，需主 python 装了两个 construct）
python bench/bench_expr.py
python bench/bench_bitstream.py
python bench/bench_stages.py
python bench/bench_breakdown.py
```

## 与 perf-scenarios.csv 的关系

`docs/perf-scenarios.csv` 是 PM 维护的性能数据单一事实源（154 测量点）。bench 脚本
生成的 JSON 报告（`bench/results/*.json`）字段与 CSV 对齐，便于 PM 自动导入。

## 设计依据

- `docs/design/基础设施/测试框架设计.md` §2.5-§2.7 + §5.2
- `experiments/E01_O1_ab_stats.py`（Welch t 检验经验复用源）

## venv 解析（bench_struct.py，与 conftest.py 一致）

`bench_struct.py` 的 venv 解析顺序（REV 新-改进-5）：
1. 环境变量 `CRS_PYTHON` / `PC_PYTHON`（CI 覆盖用）
2. 默认 venv 路径 `crs_venv_new` / `crs_venv_py_new`
3. fallback `sys.executable`（最后手段）

其他 4 个 bench 脚本（bench_expr/bitstream/stages/breakdown）保留原 `sys.executable`
逻辑（已验证），后续可逐步迁移到 BenchRunner + 三级 venv 解析。
