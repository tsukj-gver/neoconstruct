# neoconstruct

> 高性能二进制解析/构建库 —— **Rust 内核 + Python `@dataclass` API**。
>
> 性能目标：**≥4x vs Python [construct](https://github.com/construct/construct) 2.10.70**（10x 为理想）。实测 555 个测量点平均 **11.98x**（parse 10.16x / build 13.99x），96.6% 达到 ≥4x 目标。

neoconstruct 用 Rust 重写了 Python `construct` 库的内核：通过 [pyo3](https://pyo3.rs) 直接操作 CPython C API，parse/build 各只有**一次 FFI 边界穿越**，不在 Rust 侧引入中间表示层。用户面是声明式的 `@dataclass` 语法（mashumaro 风格），适合表达二进制协议、文件格式、网络报文。

```python
from dataclasses import dataclass
from neoconstruct import StructMixin, field, Int16ub, Int8ub, Bytes

@dataclass
class Header(StructMixin):
    magic:   int   = field(Int16ub)       # 2 字节大端无符号整数
    version: int   = field(Int8ub)        # 1 字节
    payload: bytes = field(Bytes(4))      # 固定 4 字节

# build：对象 -> 字节
h = Header(magic=0xCAFE, version=1, payload=b"ABCD")
assert h.build() == b"\xCA\xFE\x01ABCD"

# parse：字节 -> 对象（互为逆运算）
assert Header.parse(b"\xCA\xFE\x01ABCD") == h
```

## 核心特性

- **声明式 API**：`@dataclass class X(StructMixin)` + `field(subcon)`，编译在 `__init_subclass__` 自动触发
- **parse/build 对称**：parse 把字节变对象，build 把对象变字节
- **字段名直接引用**：`Bytes(n)` 中 `n` 直接指向同 Struct 中已声明的字段；表达式编译为 Rust VM 指令，运行时零 FFI
- **三种字段模式**：`field`（读写）/ `rfield`（只读，自动算）/ `wfield`（只写，padding/reserved）
- **全场景高性能**：从 3 字段小 Struct 到 10000 元素 Array，均显著快于纯 Python 实现

## 安装

需要 Rust 工具链（stable）与 Python ≥ 3.10。

```bash
cd neoconstruct
maturin build --release
pip install dist/*.whl
```

编译产物安装为 `neoconstruct._neoconstruct_core`，用户统一 `import neoconstruct`。

## 性能

基于 `docs/perf-scenarios.csv` 共 **555 个测量点**（每点为绝对基线 `construct==2.10.70` 的 Controlled A/B Test 实测，详见 `.opencode/skills/performance-gate/SKILL.md`）的统计：

| 指标 | 数值 |
|------|------|
| 总体平均加速比 | **11.98x** |
| parse 平均 | 10.16x |
| build 平均 | 13.99x |
| 达到 ≥4x 目标 | 536 / 555 = **96.6%** |
| 达到 ≥10x 理想 | 354 / 555 = 63.8% |

按构造器平均（节选，完整数据见 CSV）：

| 构造器 | 平均 | 测量点数 |
|--------|------|---------|
| GreedyRange / PrefixedArray / Index / Array | 17.5-20.1x | 76 |
| VarInt / Sequence / Union / Int24ub | 14.8-15.2x | 22 |
| Check / FocusedSeq / Subconstruct / ProcessXor | 13.6-14.1x | 18 |
| Enum / PaddedString / NamedTuple / Mapping / Peek | 12.0-12.2x | 44 |
| Switch / Float32/64 / CString / Pointer / Prefixed | 10.2-10.6x | 70 |
| Float16 / FormatField / Bytes / GreedyString | 9.4-10.0x | 53 |
| FlagsEnum / RepeatUntil / Checksum / RawCopy | 6.5-7.6x | 57 |

> **未达 4x 的 19 个测量点**全部是已知边界场景（与各阶段验收记录一致）：PyCallable 模式 `RepeatUntil`（每元素回调跨 FFI）、`BytesInteger(16)` 慢路径、`RawCopy` 多字段、`Checksum` 显示对象固有税、Phase 1 早期深嵌套测量点。详见 `docs/perf-scenarios.csv` 的 `notes` 列与 `harness/experiences.md`。

## 功能覆盖

134 个构造器中已实现 **101 个（75%）**，覆盖：

- **Primitives**：`Int8ub`~`Int64sl` / `Float16b`~`Float64l` / `Byte`/`Short`/`Int`/`Long` 别名 / `Bytes` / `GreedyBytes` / `BytesInteger` / `VarInt` / `ZigZag`
- **Strings**：`CString` / `GreedyString` / `PaddedString` / `PascalString` / `NullTerminated` / `NullStripped`
- **Bit / Bitfield**：`Bit` / `Nibble` / `Octet` / `BitsInteger` / `Bitwise` / `Bytewise` / `BitsSwapped` / `ByteSwapped`
- **Adapter**：`Const` / `Default` / `Check` / `Rebuild` / `Computed` / `Peek` / `RawCopy` / `Hex` / `HexDump` / `Enum` / `FlagsEnum` / `Mapping` / `OneOf` / `NoneOf` / `Union` / `Checksum` / `Tell` / `Terminated` / `Probe` / `Pass`
- **Conditional**：`If` / `IfThenElse` / `Switch` / `Select` / `FocusedSeq` / `StopIf`
- **Streams**：`Seek` / `Pointer` / `Prefixed`
- **Array**：`Array` / `GreedyRange` / `PrefixedArray` / `RepeatUntil` / `Index` / `Element`

> 完整状态见 `docs/constructors-inventory.csv`。

## 已知限制

- **跨层引用**：嵌套 `StructMixin` 子类的类体在 Python 类体作用域求值，无法引用外层字段名（报 `NameError`）——neoconstruct 不支持跨层字段引用，与原版 construct 的嵌套 `this` 语义同样隔离。变通：把所需字段提升到同一层，或在外层用 `Computed(...)` 预计算，或 parse 后在 Python 层做属性运算。注意：`field(..., context=...)` 参数当前未实现（保留位），请勿依赖。
- v0.1.1 起 `Switch` / `If` / `IfThenElse` / `Computed` / `Bytes(len)` / `Array(count)` 等表达式消费者**可嵌套于包装器内**（`Prefixed` / `PrefixedArray` / `Bitwise` / `Hex` / `Select` 等），表达式引用同 Struct 前序字段。例外：`Union` 内的表达式引用外层字段暂不可用（Union 解析使用隔离 context，属已知边界）。
- `Select` 的 `default=Pass` 分支命中时提前返回 `None`（与原版行为一致，非缺陷）。
- 值提供型构造器（`Const` / `Default` / `Rebuild` / `Computed` / `Padding`）作为 `field()`/`wfield()` 时，v0.1.1 起自动获得隐式 `default=None` + `kw_only=True`（实例化不再强制实参；build 由节点层补值）。mypy/pyright strict 下与 `int` 注解会有告警，可显式传 `default=` 或注解写 `int | None`。

## 文档

### 用户面（用 neoconstruct 实现协议）

- **用法 SKILL（自包含）**：`.opencode/skills/neoconstruct-usage/SKILL.md` —— 读完即可从零实现含条件分支 + bitfield + CRC 的复杂协议，所有示例可直接复制运行
- **最小示例**：本 README §核心示例
- **系统测试参考实现**：`testing/system/` 下 4 个真实协议（Modbus RTU / CAN / IEC104 / IPv4）
- **SKILL 验证实现**：`experiments/skill-validation/sctp_crs.py`（完整 SCTP 协议，53 round-trip + 8 parity 测试）

### 工程面（参与开发 / Agent 协作）

本项目使用基于 [HARNESS.md v1.0](https://github.com/construct/construct) 的 Agent 工程化流程（PM / ARCH / DEV / REV / VET / AUDITOR 六角色协作）。

| 入口 | 用途 |
|------|------|
| `AGENTS.md` | 项目级 System Rules（最高优先级，全员必读） |
| `harness/MEMORY.md` | 长期记忆 L0 索引（项目定位 / 阶段索引 / 教训索引 / 性能快照） |
| `harness/experiences.md` | L1 教训（L-01~L-15 模式化失败，决策前对照） |
| `docs/decisions/` | L2 决策记录（ADR-001~ADR-NNN） |
| `docs/design/` | 设计文档（模块设计 + 基础设施） |
| `plans/phaseN/总纲.md` | 各阶段单一事实源 |
| `.opencode/agents/<role>.md` | 各角色职责（base + extension 分层） |

## 目录结构

```
neoconstruct/     # Rust 内核 + Python 包源码（交付物）
  ├── src/        # Rust 业务代码（nodes / descriptors / ...）
  ├── python/     # Python 用户面包（import neoconstruct 入口）
  ├── tests/      # Rust 单元测试 + Python 集成测试
  └── bench/      # 性能基准
docs/             # 设计 / 决策 / 审查 / 规范 / CSV 清单
plans/            # 阶段总纲 + 过程记录
harness/          # LTM（MEMORY / experiences / manifests / extensions）
testing/          # CI 门禁脚本（L1 质量 / L2 功能 / L3 性能 / L4 一致性）
experiments/      # 实验代码（性能调查 / SKILL 验证）
.opencode/        # agents + skills 配置
```

> parity 测试的原版参考实现是 `PC_PYTHON` 指向的 venv 中 pip 安装的 `construct==2.10.70`（工作区内无原版仓库 checkout）。

## 质量门禁

每次出口必须通过 4 层门禁：

```bash
cargo build && cargo clippy --all-targets -- -D warnings && cargo fmt --check && cargo test
```

详见 `testing/ci/`（L1 质量 / L2 功能 / L3 性能 / L4 一致性）。

### 测试环境变量

parity / system 测试需要两个 Python 解释器，通过环境变量指定：

| 变量 | 指向 | 用途 |
|------|------|------|
| `PC_PYTHON` | 安装了 `construct==2.10.70` 的 venv 的 python.exe | parity 测试的原版参考实现（推荐 `neoconstruct/.venv-pc`） |
| `CRS_PYTHON` | 安装了 neoconstruct 扩展的 venv 的 python.exe | system 测试的 neoconstruct 实现（推荐 `neoconstruct/.venv`） |

未设置时回退 `sys.executable`（仅当该解释器已装对应包时可用）。换机运行测试前必须设置，否则 system parity 会因找不到原版 construct 而失败。

## 项目状态

- **当前进度**：Phase 0-10 已完成，构造器覆盖 75%，4 个真实协议系统测试通过
- **基线版本**：Python `construct==2.10.70`
- **许可**：MIT

## 致谢

- [construct](https://github.com/construct/construct)（Python 原版，参考实现）
- [pydantic-core](https://github.com/pydantic/pydantic-core)（pyo3 + 零中间表示层架构参考）
- [mashumaro](https://github.com/Fatal1ty/mashumaro)（`@dataclass` 声明式 API 参考）
