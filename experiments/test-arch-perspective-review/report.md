# neoconstruct 测试套件视角覆盖审查报告

> 审查者角色：资深测试架构师（首次接触本库，仅以 README + SKILL + 库/测试套件为输入）。
> 方法：先归纳用户视角全集 → 逐视角对照套件组织结构 → 对存疑视角写探针实测（本目录 6 个探针）→ 判定。
> 基线：`pytest tests/ -q` = **1257 passed / 8 skipped / 43.9s**（分布：parity 405、integration 396、unit 284、system 91、errors 89，共 1265 collected）。
> 本报告不做缺陷挖掘，只回答"哪些用户视角有测试、哪些没有、为什么结构上没有"。

---

## 1. 用户视角全集（经验归纳，不预设框架）

### 1.1 用户是谁（Persona）

| # | Persona | 典型场景 |
|---|---------|---------|
| P1 | 协议实现工程师（工业/网络/嵌入式） | 按 spec 写协议类，解析抓包/设备上报，生成下发帧 |
| P2 | construct 迁移用户 | 存量 construct 代码换库，关心行为是否等价 |
| P3 | 应用/服务开发者 | parse/build 嵌入服务：多线程 worker、高频小包、对象进缓存/消息队列 |
| P4 | 数据分析师/逆向工程师 | 大文件（pcap/固件镜像）、百万级记录、结果导出 JSON/入库 |
| P5 | 测试向量生成者 | build 为主：构造实例生成协议合规测试帧 |
| P6 | 新手/评估者 | 从 README/SKILL 复制代码起步，评估是否采用 |

### 1.2 生命周期阶段（Lifecycle）

| # | 阶段 | 用户在做什么 |
|---|------|-------------|
| L1 | 上手 | pip install、跑通首屏示例、import 文档承诺的符号 |
| L2 | 开发迭代 | REPL/notebook 反复编辑协议类、重定义、reload 模块、复用 field() 描述符 |
| L3 | 调试坏数据 | 截断/篡改的输入、读懂错误消息与 path、用 Probe 定位 |
| L4 | 集成进应用 | 并发调用、parse 产物 pickle/asdict/deepcopy/JSON 后续流转 |
| L5 | 生产加固 | 不可信输入（恶意长度字段）、错误恢复后继续服务 |
| L6 | 维护/演进 | schema 变更（加字段/继承旧类）、库升级后行为/性能不回退 |

### 1.3 使用模式（Mode）

| # | 模式 | 说明 |
|---|------|------|
| M1 | 整包一次性 parse(bytes) | 主导模式 |
| M2 | 流式/部分数据 | TCP 分帧重组后 parse；parse 只收 bytes 是既定契约 |
| M3 | 大规模数据 | 大缓冲、百万记录：时间 + 内存包络 |
| M4 | 高频小包 | 服务端每包开销 |
| M5 | 递归/树状协议 | TLV（ASN.1/MKV/ISO7816）、自引用结构 |
| M6 | 构建为主 | 测试向量/帧生成（build 侧语义） |

---

## 2. 现有套件组织 → 视角映射（覆盖判定）

```
tests/
├── unit/        284  表达式编译、编译期拒绝（含继承拒绝/dataclasses.field 遮蔽）、字段类型、RepeatUntil/Tell/Computed
├── integration/ 396  构造器×语义矩阵（value_semantics_matrix 1538 行组合完备）、roundtrip、edge_cases、各语义文件
├── errors/       89  错误分类、path 质量、恢复不变量、不可信 count 加固（子进程隔离敌意矩阵）
├── parity/      405  vs construct 2.10.70 行为等价（primitives/strings/bitstream/array/adapter/conditional）
├── system/       91  真实协议：IPv4 / IEC104 / CAN / Modbus RTU（spec 向量 + 双实现 parity）
├── rust/          -  Rust 侧 bench（非 pytest 资产）
└── _helpers/      -  parity 子进程 runner / normalize
```

| 视角 | 对应套件 | 判定 |
|------|---------|------|
| P1 协议实现 | system/（4 协议，spec 向量 + parity 双保险） | **覆盖良好** |
| P2 construct 迁移 | parity/（405 例，最大份额） | **覆盖良好** |
| P3 应用集成（并发/对象流转） | 无 | **缺失**（探针 1/2 证实能力正常但零守卫） |
| P4 规模化（大文件/内存） | 无 | **缺失**（探针 5 实测 3MB→163MB 无人关注） |
| P5 测试向量生成（build 侧） | value_semantics_matrix + 各语义文件的 build 分支 | **覆盖良好** |
| P6 上手（文档契约） | test_example.py（Modbus 例子，非 README 首屏） | **部分**（探针 6 证实当前文档契约成立但无守卫） |
| L1 上手 | 同 P6 | 部分 |
| L2 开发迭代 | 无（所有类定义在函数体内一次性使用） | **缺失**（探针 3 证实能力正常） |
| L3 调试坏数据 | errors/（path 精确到 `root.items[2].b`） | **覆盖良好** |
| L4 应用集成 | 无 | 缺失 |
| L5 生产加固 | errors/test_untrusted_count_hardening（子进程防 abort 矩阵） | **覆盖良好** |
| L6 维护/演进 | schema 继承：设计拒绝 + 完整测试（test_compile_rejections）；性能回归：**无 pytest 守卫** | 部分 |
| M1 整包 parse | 全套件主导模式 | 覆盖充分 |
| M2 流式边界 | parse 仅收 bytes 的 TypeError 已锁定；增量组帧模式无锚点 | 部分（契约已锁，模式无锚） |
| M3 大规模 | 无 | 缺失 |
| M4 高频小包 | 无（0.6us/包，探针 5） | 缺失 |
| M5 递归协议 | 无（test_edge_cases.py:131 mutual-reference 被显式"简化跳过"） | **缺失且能力未定**（探针 4） |
| M6 build 为主 | 覆盖良好 | 覆盖良好 |

**结构性观察**：套件按"测试层级 + 构造器语义"双轴组织，断言终点几乎全部停在 **parse/build 的返回值与抛错**。parse 产出对象之后的生命、类在第一次定义之后的生命、进程在单线程之外的生命、库在文档承诺之下的表面——这四类"之后"均无落脚目录。

---

## 3. 探针实录（全部可复现，见本目录 probe_*.py）

| 探针 | 结果 |
|------|------|
| probe_ecosystem.py | asdict/astuple/replace/copy/deepcopy/pickle/repr/eq **全部正常**；`pickle(类)` 也正常；hash 抛 TypeError（与普通可变 dataclass 一致） |
| probe_threads.py | 8 线程 × 1000 次 parse/build 共享类 + 4 线程并发动态定义类：**全部正确、无 crash** |
| probe_class_lifecycle.py | 同名类重定义 200 次 OK；继承→设计拒绝（已有测试锁定）；**同一 field() 描述符复用于 2 个类 OK**；旧实例在类重定义后仍可 build；importlib.reload 后新旧类共存均可用 |
| probe_recursive.py | 固定深度 TLV OK；**自引用 `Switch(t,{1:'Node'})` → `CompilationError: 未知的字段描述符类型: 'Node'`（无引导的错误消息）**；前向类引用 NameError（README 已知限制）；**300/2000 层深嵌套 parse+build 均正常**（无栈溢出） |
| probe_scale.py | 1M 记录（3MB）parse 0.31s / build 0.14s；**内存放大 54x（3MB 输入 → 163MB peak）**；小包 0.6us/parse |
| probe_doc_contract.py | SKILL 附录 82/82 + 表格补充 21/21 符号可导入；**README 首屏示例逐字运行通过** |
| 收集验证 | `test_checksum_bench.py` **pytest 收集 0 个测试**（纯 main() 打印脚本，无阈值断言） |

---

## 4. 缺失视角清单（按用户价值排序）

### GAP-1 parse 产物的 Python 生态后续生命周期（P3/P4 × L4）★★★
- **场景**：用户 parse 后 pickle 进缓存/消息队列、`asdict` 导 JSON 日志、deepcopy、`dataclasses.replace` 派生新帧。这是 dataclass API 的核心卖点之一，也是日常最高频的"parse 之后"。
- **结构性缺失原因**：套件按构造器语义矩阵组织，断言终点在返回值本身；`asdict` 在 system/ 里只作为 parity 归一化的**工具**出现，从未作为**被测对象**。
- **风险**：未来任何改动（自定义 `__eq__`、`__slots__`、惰性字段、Rust 侧持有状态的对象值）都可能无声破坏 pickle/copy 链路。
- **建议组织**：`tests/ecosystem/test_object_model.py`——parse 产物的 dataclass 协议契约（asdict 形状、pickle round-trip 后 build 等价、deepcopy 独立性、replace 派生 build、eq 语义、repr 含字段名）。

### GAP-2 并发共享协议类（P3 × L4）★★★
- **场景**：服务进程多线程/asyncio 任务共用一个协议类（编译产物是类级共享状态），并发动态注册协议类。
- **结构性缺失原因**：全部测试单线程单类；Rust 内核共享状态的安全性没有任何回归网。探针证实当前正常——正是补测试最便宜的时机。
- **建议组织**：`tests/integration/test_thread_safety.py`（N 线程 parse/build 结果一致性 + 并发类定义）。

### GAP-3 规模化使用包络（P4 × M3/M4）★★
- **场景**：解析大文件/百万记录。探针实测 3MB 输入 → 163MB peak（54x 放大，"parse 成 Python 对象"设计的固有代价）。用户解析 500MB pcap 前需要知道这个包络——目前无处得知、无测试锚定。
- **结构性缺失原因**：套件断言单位是"单次调用值正确"；内存/时间不在任何 pytest 断言中。
- **建议组织**：`tests/scale/test_scale_envelope.py`：冒烟级锚点（1M 记录 parse 时限 + 内存放大系数上限的软门槛），并在 README 增补"内存形态"说明。

### GAP-4 性能承诺的回归守卫（L6）★★
- **场景**：README 头条承诺 ≥4x / 实测 11.98x。库升级后用户预期性能不回退。
- **结构性缺失原因**：性能验证被视为开发期活动：`tests/integration/test_checksum_bench.py` 无 test_ 函数（收集 0），555 测量点基建在 tests/ 之外，tests/rust 是 Rust 内部 bench。pytest 套件对**头条承诺**零守卫。
- **建议组织**：`tests/perf/` 带 `@pytest.mark.perf` 的冒烟集（少量代表性场景 + 宽松阈值，与外部精密 bench 分层），避免 bench 脚本混在测试目录里制造"已覆盖"假象。

### GAP-5 递归/自引用协议（M5，P1/P2 高频需求）★★
- **场景**：TLV 树（ASN.1 BER、MKV、ISO 7816）是二进制解析的常见形态；construct 老用户会找 `Embedded`/自引用等价物。
- **结构性缺失原因**：`test_edge_cases.py:131` mutual-reference 测试被显式"简化跳过"（承认缺口未跟进）；自引用撞墙时错误消息是 `未知的字段描述符类型: 'Node'`——对比继承拒绝的优质引导（理由 + 组合替代），递归需求得不到同等对待。固定深度组合（探针 4.2）可用但文档只字未提。
- **建议组织**：二选一并测试锁定：a) 设计拒绝路线——`test_compile_rejections.py` 增补自引用场景 + 改善错误引导（像继承那样说明理由与变通）；b) 支持路线——延迟引用 capability 测试。无论哪条，SKILL §5 易错点应增补"递归协议怎么办"。

### GAP-6 开发迭代循环（L2）★
- **场景**：REPL/notebook 用户反复编辑类、reload 模块、模块级共享 `field()` 描述符、旧实例继续 build。探针全部证实正常。
- **结构性缺失原因**：所有 dataclass 定义在测试函数体内一次性定义一次性使用——"类的第二次生命"结构上不可见。
- **建议组织**：`tests/integration/test_dev_loop.py`：重定义 N 次、reload 新旧类共存、共享描述符跨类复用、旧实例可用性。

### GAP-7 文档契约（L1 × P6）★
- **场景**：新手逐字复制 README 首屏示例、按 SKILL 附录 import。探针证实当前 100% 成立——但这是"当前恰好对"，无守卫；`test_example.py` 测的是另一个 Modbus 例子。
- **建议组织**：`tests/docs/test_readme_example.py`（提取 README 代码块 exec）+ `test_public_surface.py`（SKILL 附录符号表 hasattr 断言，符号清单变更时强制同步文档）。

### GAP-8 流式/增量数据的用户锚点（M2）☆
- **场景**：TCP 流式分帧 + 重组后 parse。parse 仅收 bytes 是已锁定契约（TypeError 有测试），缺的是"如何组合 Prefixed/GreedyRange 做分帧"的模式锚点。偏文档性质，优先级最低。

---

## 5. 现有结构的改进建议

1. **增设按用户视角命名的顶层目录**：`ecosystem/`（或并入 integration 但文件名视角化：`test_object_model.py` / `test_thread_safety.py` / `test_dev_loop.py`）。现有 unit/integration/errors/parity/system 五层服务于"构造器语义完整性"，新视角塞不进去是结构性的。
2. **bench 文件移出 tests/ 或改造为带阈值的 perf-smoke**：`test_checksum_bench.py` 位于测试目录但收集 0 个测试，制造覆盖假象；命名以 `test_` 开头却无断言，违背最小惊讶。
3. **parity 单独 mark**：405 例（32%）依赖 `.venv-pc` 双子进程环境，是套件最慢、环境最脆弱的部分；加 `@pytest.mark.parity` 支持分层运行（价值不减，故障隔离更好）。
4. **`test_example.py` 名实对齐**：改名 `test_modbus_example.py`，另建真正锚定 README 首屏的示例测试。
5. **errors/ 目录保持现状**：它已经是套件中视角化最好的目录（错误 UX / 敌意输入 / 恢复不变量分文件），可作为新目录组织的范本。
6. **互为镜像的两处"显式跳过"清理**：`test_edge_cases.py:131`（mutual reference 简化跳过）与 parity 的 8 个 skip——前者应升级为 GAP-5 的正式用例，后者已注明理由可保留。

---

## 6. 覆盖良好的视角（诚实声明）

- **构造器语义组合完备性**：value_semantics_matrix（1538 行，构造器 × 字段模式 × 值时机 × 嵌套位置 × 数据负例）是同类库中罕见的系统性矩阵。
- **construct 迁移等价性**：parity 405 例覆盖 6 大构造器族，含 normalize 容差设计（NaN/subnormal）。
- **真实协议**：4 个协议均有 spec 权威向量（Modbus CRC=0x7687 等）+ 双实现对照。
- **错误 UX**：path 质量精确到 `root.items[2].b`，恢复不变量（无残留状态/输入不可变/实例独立）齐备。
- **不可信输入加固**：子进程隔离的敌意矩阵（防 0xFFFFFFFF count 预分配 abort）是生产级安全网。
- **编译期误用拒绝**：继承拒绝（含理由与引导）与 dataclasses.field 遮蔽检测，把静默错误变显式。
- **dataclass 形态边界**：frozen / `__post_init__` / 混合 dc_field / 空类。

## 7. 复现

```
cd neoconstruct
$env:PC_PYTHON="$PWD\.venv-pc\Scripts\python.exe"; $env:CRS_PYTHON="$PWD\.venv\Scripts\python.exe"
& .venv\Scripts\python.exe -m pytest tests/ -q          # 1257 passed / 8 skipped
& .venv\Scripts\python.exe ..\experiments\test-arch-perspective-review\probe_*.py
```
