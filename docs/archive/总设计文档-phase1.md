# Construct-RS 总设计文档

## 1. 项目概述

### 1.1 项目背景

Construct 是一个成熟的 Python 库（v2.10.70），用于声明式地定义和解析二进制数据结构。其核心特点是**对称性**：同一套声明式定义既能解析（parse）二进制数据，也能构建（build）二进制数据。

本项目的目标是将 Construct 的核心内核用 Rust 重写，提供：
- 显著更高的解析/构建性能（目标 10x+）
- 内存安全的二进制数据处理
- 零成本抽象的声明式 API
- 可选的 Python 绑定（通过 PyO3）

### 1.2 设计原则

1. **API 忠实性**：公开 API 尽可能与 Python 版本功能对齐，降低迁移成本
2. **Rust 惯用法**：在保持 API 兼容的前提下，充分利用 Rust 的类型系统和所有权模型
3. **零运行时依赖**：核心库无必需外部依赖（与 Python 版本一致）
4. **可选功能**：压缩、加密等功能通过 feature gate 按需启用
5. **对称性**：所有构造器必须同时支持 parse 和 build

## 2. 架构总览

### 2.1 分层架构

```
┌─────────────────────────────────────────────┐
│             Gallery / 应用层                  │  格式定义（ELF, PE, BMP...）
├─────────────────────────────────────────────┤
│             构造器层 (Constructs)             │  ~100 个构造器实现
├─────────────────────────────────────────────┤
│           表达式系统 (Expressions)            │  this/obj_ 路径引用、运算
├─────────────────────────────────────────────┤
│             核心抽象层 (Core)                 │  Construct trait, Value, Context
├─────────────────────────────────────────────┤
│             基础设施层 (Infra)                │  Stream, Error, Binary utils
└─────────────────────────────────────────────┘
```

### 2.2 数据流

```
Parse 方向:
  二进制数据 → Stream → Construct._parse() → Value

Build 方向:
  Value → Construct._build() → Stream → 二进制数据
```

### 2.3 核心抽象关系

```
Construct (trait)
├── parse(stream, ctx) → Result<Value>
├── build(value, stream, ctx) → Result<()>
└── sizeof(ctx) → Result<usize>

Value (enum)
├── None, Bool, Int, UInt, BigInt
├── Float, Bytes, String
└── List, Container

Context (struct)
├── fields: BTreeMap<String, Value>
└── parent: Option<Box<Context>>   // 嵌套上下文

Stream (trait)
├── read(buf) → Result<usize>
├── write(buf) → Result<()>
├── seek(pos) → Result<()>
└── tell() → Result<u64>
```

## 3. 项目结构

```
construct-rs/
├── Cargo.toml                    # 包配置，feature 定义
├── src/
│   ├── lib.rs                    # crate 入口，公共 API 导出
│   ├── core/
│   │   ├── mod.rs                # Construct trait 定义
│   │   ├── context.rs            # Context 容器
│   │   ├── stream.rs             # Stream trait + ByteStream 实现
│   │   └── error.rs              # 错误类型定义
│   ├── value.rs                  # Value 枚举类型
│   ├── containers.rs             # Container（有序字典）, ListContainer
│   ├── binary.rs                 # 整数/位/字节转换工具
│   ├── bitstream.rs              # 位级流包装
│   ├── expr.rs                   # 表达式系统
│   ├── constructs/
│   │   ├── mod.rs                # 构造器注册和导出
│   │   ├── atomic.rs             # Bytes, FormatField, BytesInteger, BitsInteger, VarInt, Flag
│   │   ├── composites.rs         # Struct, Sequence, Union, Select, FocusedSeq
│   │   ├── repetition.rs         # Array, GreedyRange, RepeatUntil
│   │   ├── adapters.rs           # Adapter, SymmetricAdapter, Validator, Enum, Mapping
│   │   ├── control_flow.rs       # If, IfThenElse, Switch, Check, StopIf
│   │   ├── meta.rs               # Computed, Rebuild, Default, Const, Index, Tell, Seek
│   │   ├── tunneling.rs          # Pointer, Peek, Prefixed, Transformed, Restreamed
│   │   ├── lazy.rs               # Lazy, LazyStruct, LazyArray, LazyBound, Rebuffered
│   │   ├── string.rs             # StringEncoded, PaddedString, CString
│   │   ├── crypto.rs             # Compressed, EncryptedSym（feature-gated）
│   │   └── misc.rs               # Pass, Terminator, Error, Numpy, Pickled, Slicing, Indexing
│   └── gallery/                  # 格式解析器示例
│       ├── mod.rs
│       ├── elf.rs
│       └── pe32coff.rs
├── tests/                        # 集成测试
├── benches/                      # 基准测试
└── examples/                     # 示例代码
```

## 4. 核心设计决策

### 4.1 动态类型替代：Value 枚举

Python 版本依赖动态类型系统。Rust 中使用 `Value` 枚举：

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    None,
    Bool(bool),
    Int(i64),
    UInt(u64),
    BigInt(i128),
    Float(f64),
    Bytes(Vec<u8>),
    String(String),
    List(Vec<Value>),
    Container(IndexMap<String, Value>),  // 有序字典
}
```

**理由**：
- 枚举 + 模式匹配是 Rust 中处理动态类型的标准方式
- 编译期保证类型安全
- 运行时开销可接受

### 4.2 trait 对象 vs 泛型

采用 `Box<dyn Construct>` 作为主要抽象方式：

```rust
pub struct Struct {
    fields: Vec<(String, Box<dyn Construct>)>,
}
```

**理由**：
- 与 Python 版本的动态组合能力一致
- 用户可以在运行时构建任意构造器组合
- 灵活性优先，性能在 Phase 9 优化

### 4.3 错误处理

使用 `thiserror` 定义错误体系，统一 `Result<T, ConstructError>`：

```rust
#[derive(Debug, thiserror::Error)]
pub enum ConstructError {
    #[error("Format field error at {path}: {message}")]
    FormatField { path: String, message: String },
    #[error("Stream error at {path}: {source}")]
    Stream { path: String, #[source] source: std::io::Error },
    // ... ~40 个变体
}
```

### 4.4 上下文传递

```rust
pub struct Context {
    fields: BTreeMap<String, Value>,
    parent: Option<Box<Context>>,
}
```

上下文在 parse/build 过程中向下传递，支持：
- 子字段引用先前解析的值（`this.field_name`）
- 嵌套作用域（进入子构造器时创建子上下文）
- `_` 引用父级上下文

### 4.5 流抽象

基于 `std::io` traits，扩展支持位级操作：

```rust
pub trait Stream: Read + Write + Seek {
    fn read_bits(&mut self, count: usize) -> Result<Vec<u8>>;
    fn write_bits(&mut self, bits: &[u8]) -> Result<()>;
}
```

## 5. 依赖策略

### 5.1 核心依赖（必需）

| crate | 用途 |
|-------|------|
| `thiserror` | 错误类型派生 |

### 5.2 可选依赖（feature-gated）

| crate | feature | 用途 |
|-------|---------|------|
| `byteorder` | `byteorder` | 字节序数值读写 |
| `num-bigint` | `bigint` | 大整数支持（>8字节） |
| `flate2` | `compression` | zlib/gzip 压缩 |
| `lz4` | `compression` | LZ4 压缩 |
| `aes` + `cbc` | `encryption` | AES 对称加密 |
| `aes-gcm` | `encryption` | AES-GCM 认证加密 |
| `criterion` | `bench` | 基准测试 |
| `pyo3` | `python` | Python 绑定 |

## 6. 与 Python 版本的差异

| 方面 | Python 版本 | Rust 版本 |
|------|------------|----------|
| 类型系统 | 动态类型 | `Value` 枚举 + 模式匹配 |
| 错误处理 | 异常 | `Result<T, ConstructError>` |
| 运算符重载 | `__truediv__`, `__add__` 等 | 方法链：`named()`, `add()` |
| 表达式 | `this.field` 全局单例 | `path("field")` 或宏 |
| 惰性解析 | `LazyContainer` 继承 dict | `LazyValue` 包装 `OnceCell` |
| 编译优化 | `compile()` 生成 Python 代码 | 可选 proc-macro 或宏 DSL |
| 容器 | Container(dict) + 属性访问 | `Value::Container(IndexMap)` |

## 7. 开发流程

1. 每个阶段依据 `plans/phaseN/总纲.md` 执行
2. 子任务完成后在 `plans/phaseN/过程记录.md` 中记录
3. 阶段完成后依据总纲中的出口标准验收
4. 设计变更需同步更新 `docs/` 下的设计文档
