# v0.1.2-1 spike：字段值语义框架原型与证据

任务：v0.1.2-1 [字段值语义问题·框架方案探索与原型验证]（ARCH；v2 修订 =
v0.1.2-1r，REV 驳回后补证据）。
设计文档：`docs/design/基础设施/v0.1.2-字段值语义框架.md`（v2）。

## 内容

| 文件 | 用途 |
|------|------|
| `anchor_construct.py` / `anchor_construct2.py` | construct 2.10.70 锚点实测（.venv-pc，31+ 用例：flagbuildnone 矩阵 / parse 存储 / build 缺省 / 嵌套表达式 ctx） |
| `anchor3.py` | **v2** P1/P4/P5 补充锚点（15 条）：Peek/Seek/Checksum 哑值语义、If/Renamed 传播、B2 数据负例（StreamError） |
| `audit_flagbuildnone.py` → `audit_flagbuildnone_result.md` | **v2** AST 全量对账脚本：原版 flagbuildnone（15 叶类+7 传播类+基类 False）× neoconstruct 导出构造器 100 个 × ValueKind 分类（PASS） |
| `current_neoconstruct.py` | 当前 neoconstruct 同矩阵对照（B1-B4 形态固化，含 rfield(Const) build 失败证据；B1/B1e/B1f/B3 已被止血修复转绿） |
| `valuekind-spike/` | 推荐方案核心机制原型（pyo3 0.22，与主 crate 同版本）：v0 现状形态 vs v1 ValueKind 形态 A/B |
| `bench_ab.py` | 机制验证（ctx 有效值回写 / None 补值 / 错误时机）+ 同会话交替 A/B 微基准 |

## 复现

```sh
# 锚点（.venv-pc）
neoconstruct/.venv-pc/Scripts/python.exe anchor_construct.py
neoconstruct/.venv-pc/Scripts/python.exe anchor_construct2.py
# 现状对照（dev venv）
neoconstruct/.venv/Scripts/python.exe current_neoconstruct.py
# spike 构建 + A/B（已在 dev venv 安装 valuekind_spike）
cd valuekind-spike && VIRTUAL_ENV=../../.venv python -m maturin develop --release
neoconstruct/.venv/Scripts/python.exe bench_ab.py
# 主基线（DEV 实施后对照）
cd neoconstruct/bench && ../.venv/Scripts/python.exe bench_struct.py --iterations 5
```

## 关键结论

- 机制：v1 ctx 写**有效值**（修复 None 流入表达式 = B1e 根因），None 实例值
  → 明确 BuildValueMissing（对齐原版 KeyError 时机后移）。
- 性能（2 会话 × 3 轮 × 9 批 × 20000 次，6 字段负载）：Δ 中位 +2.52% / +0.84%，
  多轮 ≤0——噪声级（L-09 判据 <30%）。
- **实现警示**：resolve 返回 owned `Py<PyAny>` 的形态曾实测 +6.7%~+9.6%（不可用；
  数据已测、代码未保留——lib.rs 仅存就地 Bound 形态 + 禁令注释，REV P6 确认）；
  必须就地 Bound 替换（设计文档 §7 明令）。
