# construct-rs

> 高性能二进制解析/构建库 —— **Rust 内核 + Python `@dataclass` API**。
>
> 性能目标：**≥4x vs Python [construct](https://github.com/construct/construct) 2.10.70**（10x 为理想）。实测 555 个测量点平均 **11.98x**（parse 10.16x / build 13.99x），96.6% 达到 ≥4x 目标。

`construct-rs` 用 Rust 重写了 Python `construct` 库的内核：通过 [pyo3](https://pyo3.rs) 直接操作 CPython C API，parse/build 各只有**一次 FFI 边界穿越**，不在 Rust 侧引入中间表示层。用户面是声明式的 `@dataclass` 语法（mashumaro 风格），适合表达二进制协议、文件格式、网络报文。

```python
from dataclasses import dataclass
from construct import StructMixin, field, Int16ub, Int8ub, Bytes

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
- **字段名直接引用**（不是 `this.xxx`）：`Bytes(n)` 中 `n` 直接指向同 Struct 中已声明的字段；表达式编译为 Rust VM 指令，运行时零 FFI
- **三种字段模式**：`field`（读写）/ `rfield`（只读，自动算）/ `wfield`（只写，padding/reserved）
- **全场景高性能**：从 3 字段小 Struct 到 10000 元素 Array，均显著快于纯 Python 实现

## 安装

需要 Rust 工具链（stable）与 Python ≥ 3.7。

```bash
cd construct-rs
maturin develop --release
```

编译产物安装为 `construct._construct_rust`，用户统一 `import construct`。开发模式（不含 release 优化）可去掉 `--release`，但性能测量必须用 release。

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

## 文档

### 用户面（用 construct-rs 实现协议）

- **用法 SKILL（自包含）**：`.opencode/skills/construct-rs-usage/SKILL.md` —— 读完即可从零实现含条件分支 + bitfield + CRC 的复杂协议，所有示例可直接复制运行
- **最小示例**：本 README §核心示例
- **系统测试参考实现**：`testing/system/` 下 4 个真实协议（Modbus RTU / CAN / IEC104 / IPv4）
- **SKILL 验证实现**：`experiments/skill-validation/sctp_crs.py`（完整 SCTP 协议，53 round-trip + 8 parity 测试）

### 工程面（参与开发 / Agent 协作）

本项目使用基于 [HARNESS.md v1.0](https://github.com/construct/construct) 的 Agent 工程化流程（PM / ARCH / DEV / REV / VET / AUDITOR 六角色协作）。

| 入口 | 用途 |
|------|------|
| `AGENTS.md` | 项目级 System Rules（最高优先级，全员必读） |
| `harness/MEMORY.md` | 长期记忆 L0 索引（项目定位 / 阶段索引 / 教训索引 / 性能快照） |
| `harness/experiences.md` | L1 教训（L-01~L-14 模式化失败，决策前对照） |
| `docs/decisions/` | L2 决策记录（ADR-001~ADR-NNN） |
| `docs/design/` | 设计文档（模块设计 + 基础设施） |
| `plans/phaseN/总纲.md` | 各阶段单一事实源 |
| `.opencode/agents/<role>.md` | 各角色职责（base + extension 分层） |

## 目录结构

```
construct-rs/     # Rust 内核 + Python 包源码（交付物）
  ├── src/        # Rust 业务代码（nodes / descriptors / ...）
  ├── python/     # Python 用户面包（import construct 入口）
  ├── tests/      # Rust 单元测试 + Python 集成测试
  └── bench/      # 性能基准
construct/        # Python construct 原版参考仓库（只读）
docs/             # 设计 / 决策 / 审查 / 规范 / CSV 清单
plans/            # 阶段总纲 + 过程记录
harness/          # LTM（MEMORY / experiences / manifests / extensions）
testing/          # CI 门禁脚本（L1 质量 / L2 功能 / L3 性能 / L4 一致性）
experiments/      # 实验代码（性能调查 / SKILL 验证）
.opencode/        # agents + skills 配置
```

## 质量门禁

每次出口必须通过 4 层门禁：

```bash
cargo build && cargo clippy --all-targets -- -D warnings && cargo fmt --check && cargo test
```

详见 `testing/ci/`（L1 质量 / L2 功能 / L3 性能 / L4 一致性）。

## 项目状态

- **当前进度**：Phase 0-10 已完成，构造器覆盖 75%，4 个真实协议系统测试通过
- **基线版本**：Python `construct==2.10.70`
- **许可**：MIT

## 致谢

- [construct](https://github.com/construct/construct)（Python 原版，参考实现）
- [pydantic-core](https://github.com/pydantic/pydantic-core)（pyo3 + 零中间表示层架构参考）
- [mashumaro](https://github.com/Fatal1ty/mashumaro)（`@dataclass` 声明式 API 参考）
