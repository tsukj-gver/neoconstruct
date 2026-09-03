# construct-rs

> 高性能二进制解析/构建库 —— **Rust 内核 + Python `@dataclass` API**。
>
> 性能目标：**≥4x vs Python [construct](https://github.com/construct/construct) 2.10.70**（10x 为理想）。实测 555 个测量点平均 **11.98x**（parse 10.16x / build 13.99x），96.6% 达到 ≥4x 目标。

`construct-rs` 用 Rust 重写了 Python `construct` 库的内核：通过 [pyo3](https://pyo3.rs) 直接操作 CPython C API，parse/build 各只有**一次 FFI 边界穿越**，不在 Rust 侧引入中间表示层。用户面是声明式的 `@dataclass` 语法，适合表达二进制协议、文件格式、网络报文。

## 安装

```bash
pip install construct-rs
```

从源码构建（需要 Rust 工具链 stable 与 [maturin](https://github.com/PyO3/maturin)）：

```bash
cd construct-rs
maturin build --release
pip install dist/*.whl
```

## 核心示例

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
- **字段名直接引用**：`Bytes(n)` 中 `n` 直接指向同 Struct 中已声明的字段；表达式编译为 Rust VM 指令，运行时零 FFI
- **三种字段模式**：`field`（读写）/ `rfield`（只读，自动算）/ `wfield`（只写，padding/reserved）
- **表达式消费者可嵌套**（v0.1.1 起）：`Switch` / `If` / `Computed` / `Bytes(len)` / `Array(count)` 等可嵌套于 `Prefixed` / `PrefixedArray` / `Bitwise` / `Hex` / `Select` 等包装器内
- **值提供型构造器自动默认值**（v0.1.1 起）：`Const` / `Default` / `Rebuild` / `Computed` / `Padding` 作为 `field()`/`wfield()` 时自动获得隐式 `default=None`（实例化不再强制实参）
- **全场景高性能**：从 3 字段小 Struct 到 10000 元素 Array，均显著快于纯 Python 实现

## 性能

基于 555 个测量点（每点为绝对基线 `construct==2.10.70` 的 Controlled A/B Test 实测）：

| 指标 | 数值 |
|------|------|
| 总体平均加速比 | **11.98x** |
| parse 平均 | 10.16x |
| build 平均 | 13.99x |
| 达到 ≥4x 目标 | 536 / 555 = **96.6%** |
| 达到 ≥10x 理想 | 354 / 555 = 63.8% |

> 未达 4x 的 19 个测量点全部是已知边界场景（PyCallable 回调、BytesInteger(16) 慢路径、显示对象构造固有税等）。

## 功能覆盖（101 / 134，75%）

- **Primitives**：`Int8ub`~`Int64sl` / `Float16b`~`Float64l` / `Byte`/`Short`/`Int`/`Long` 别名 / `Bytes` / `GreedyBytes` / `BytesInteger` / `VarInt` / `ZigZag`
- **Strings**：`CString` / `GreedyString` / `PaddedString` / `PascalString` / `NullTerminated` / `NullStripped`
- **Bit / Bitfield**：`Bit` / `Nibble` / `Octet` / `BitsInteger` / `Bitwise` / `Bytewise` / `BitsSwapped` / `ByteSwapped`
- **Adapter**：`Const` / `Default` / `Check` / `Rebuild` / `Computed` / `Peek` / `RawCopy` / `Hex` / `HexDump` / `Enum` / `FlagsEnum` / `Mapping` / `OneOf` / `NoneOf` / `Union` / `Checksum` / `Tell` / `Terminated` / `Probe` / `Pass`
- **Conditional**：`If` / `IfThenElse` / `Switch` / `Select` / `FocusedSeq` / `StopIf`
- **Streams**：`Seek` / `Pointer` / `Prefixed`
- **Array**：`Array` / `GreedyRange` / `PrefixedArray` / `RepeatUntil` / `Index` / `Element`

## 已知限制

- **跨层引用**：表达式只能引用同 Struct 内的兄弟字段，嵌套 `StructMixin` 子类类体无法引用外层字段名（报 `NameError`）。变通：字段提升到同一层、外层 `Computed(...)` 预计算、或 parse 后在 Python 层运算。
- `Union` 内的表达式引用外层字段暂不可用（Union 解析使用隔离 context，属已知边界）。
- `Select` 的 `default=Pass` 分支命中时提前返回 `None`（与原版行为一致）。
- 值提供型构造器隐式 `default=None` 在 mypy/pyright strict 下与 `int` 注解会有告警，可显式传 `default=` 或注解写 `int | None`。
- **环境要求**：CPython 3.8+（无 abi3，每 Python 版本独立 wheel）。

## License

MIT
