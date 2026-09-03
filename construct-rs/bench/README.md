# construct-rs Benchmark 套件

> 性能基准测试集合：construct-rs vs Python construct 2.10.70 加速比测量。

## 目录结构

```
bench/
├── _helpers/                      # 公共组件
│   ├── runner.py                  # BenchRunner（统一子进程计时 + 统计）
│   ├── stats.py                   # welch_t / speedup_ratio / ConfidenceInterval
│   └── report.py                  # BenchReport + check_gates + format_results_table
├── bench_struct.py                # Struct B1-B7（用 BenchRunner）
├── bench_expr.py                  # 表达式 E1-E3
├── bench_bitstream.py             # BitStream 场景
├── bench_stages.py                # 分阶段计时（内核开销分解）
├── bench_breakdown.py             # 性能根因验证（full parse / _parse_raw 分解）
├── bench_primitives_strings_adapter.py  # Primitives / Strings / Adapter 集成基准
├── bench_conditional_streams.py   # Conditional / Streams 集成基准
├── bench_adapters_struct_streams.py     # Adapters / Struct / Streams 集成基准（P0 + P1P2）
├── bench_adapters_retest.py       # Controlled A/B Test 复测（<10x case）
├── bench_hex_opt_ab_retest.py     # Hex 显示优化 A/B 复测
├── bench_opt_shared_retest.py     # 共享节点优化后复测
├── hex_parity_check.py            # Hex/HexDump 显示对象 parity 快检
└── results/                       # bench 输出归档（JSON + Markdown + txt）
```

## 运行方式

所有 bench 脚本需在能访问两个 construct 实现的环境运行：
- **construct-rs**：通过 `construct-rs/python/` 目录（sys.path 注入）
- **Python construct 2.10.70**：通过 site-packages（pip install construct==2.10.70）

推荐用专门 venv（多数脚本已内置三级 venv 解析，见下节）：

```sh
# bench_struct.py（用 BenchRunner + 专门 venv）
python bench/bench_struct.py
python bench/bench_struct.py --case B1 --case B4
python bench/bench_struct.py --iterations 10

# 集成基准（BenchRunner + 三级 venv 解析）
python bench/bench_primitives_strings_adapter.py
python bench/bench_conditional_streams.py
python bench/bench_adapters_struct_streams.py

# 其余 4 个脚本（bench_expr / bench_bitstream / bench_stages / bench_breakdown
# 保留 sys.executable 逻辑，需主 python 装了两个 construct）
python bench/bench_expr.py
python bench/bench_bitstream.py
python bench/bench_stages.py
python bench/bench_breakdown.py
```

## 与 perf-scenarios.csv 的关系

`docs/perf-scenarios.csv` 是性能数据单一事实源（154 测量点）。bench 脚本
生成的 JSON 报告（`bench/results/*.json`）字段与 CSV 对齐，便于自动导入。

## venv 解析（与 conftest.py 一致）

`bench_struct.py` / 集成基准脚本 / 复测脚本的 venv 解析顺序：
1. 环境变量 `CRS_PYTHON` / `PC_PYTHON`（CI 覆盖用）
2. 项目内 venv：`construct-rs/.venv`（扩展）/ `construct-rs/.venv-pc`（原版参考）
3. fallback `sys.executable`（最后手段）

bench_expr / bench_bitstream / bench_stages / bench_breakdown 4 个脚本保留
`sys.executable` 逻辑（要求主 python 同时可导入两个 construct）。
