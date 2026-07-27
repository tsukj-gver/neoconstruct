---
id: REQUIREMENT-Bitwise
status: accepted
phase: "3"
task: "Phase 3 Bitwise 功能集需求分析"
last_updated: 2026-07-27
---

# Bitwise 功能集分析报告（Python construct 2.10.70）

> 调查时间：2026-06-27
> 来源：`construct/construct/core.py`、`construct/construct/lib/bitstream.py`、`construct/construct/lib/binary.py`、`construct/construct/__init__.py`

## 1. Bitwise 核心机制

### 1.1 Bitwise 是工厂函数

`Bitwise(subcon)` 定义在 `core.py:1030`，返回 `Transformed`（定长路径）或 `Restreamed`（变长路径）实例。根据 subcon 是否可定长二选一：

```python
def Bitwise(subcon):
    try:
        size = subcon.sizeof()
        macro = Transformed(subcon, bytes2bits, size//8, bits2bytes, size//8)
    except SizeofError:
        macro = Restreamed(subcon, bytes2bits, 1, bits2bytes, 8, lambda n: n//8)
    return macro
```

### 1.2 parse 流程

- **定长路径**（Transformed._parse, core.py:5277）：
  1. 一次性 stream_read(stream, size//8) 读全部字节
  2. bytes2bits(data) 整体解码为 bit 串（长度放大 8 倍）
  3. io.BytesIO(data) 包装，喂给 subcon

- **变长路径**（Restreamed._parse, core.py:5338）：
  1. RestreamedBytesIO(stream, bytes2bits, 1, bits2bytes, 8) 包一层流
  2. subcon 在这层流上读取——每次请求 N 字节，底层按 decoderunit=1 读字节、bytes2bits 解码为 8 字节 bit 串、追加到缓冲
  3. close() 校验无残留

### 1.3 build 流程

- **定长路径**：创建临时 BytesIO → subcon._build → bits2bytes 整体编码回字节 → 写入主流
- **变长路径**：RestreamedBytesIO 包装 → subcon 写入时每累积 8 个 bit 字节用 bits2bytes 编码写入底层 → close() 校验

### 1.4 bit 顺序：MSB-first

默认 MSB to LSB（bit-level big-endian）。LSB-first 通过外层 `BitsSwapped` 包装实现。

### 1.5 剩余位：不自动 padding

没有自动补齐。不足 8 的倍数的剩余 bit 触发错误：
- bits2bytes：`if len(data) % 8 != 0: raise ValueError`
- RestreamedBytesIO.close()：rbuffer/wbuffer 有残留则抛 ValueError

用户必须在 BitStruct 末尾用 Padding(n) 补齐。

### 1.6 中间表示：bit 串 = 字节串（8 倍膨胀）

Python construct 把每个 bit 用一整个字节表示（`\x00` 或 `\x01`）。这让 bit 流复用所有 bytes 接口，代价是 8 倍内存膨胀和读写次数。

## 2. 所有 Bitwise 相关构造器

| 名称 | 类型 | 行号 | 签名 | 功能 | 是否独立 |
|------|------|------|------|------|----------|
| Bitwise | 函数 | 1030 | `Bitwise(subcon)` | 字节流 ↔ bit 流包装器 | 核心 |
| BitStruct | 函数 | 4354 | `BitStruct(*subcons, **subconskw)` | Bitwise(Struct(...)) 语法糖 | 语法糖 |
| Bytewise | 函数 | 1081 | `Bytewise(subcon)` | Bitwise 内部 bit→byte 逆操作 | 独立 |
| BitsSwapped | 函数 | 4836 | `BitsSwapped(subcon)` | 字节内 bit 序翻转 MSB↔LSB | 独立 |
| ByteSwapped | 函数 | 4817 | `ByteSwapped(subcon)` | 字节序翻转 | 独立 |
| BitsInteger | 类 | 1295 | `BitsInteger(length, signed=False, swapped=False)` | 读 length 个 bit 转 int | 独立原子 |
| Bit | 函数 | 1410 | `Bit()` | BitsInteger(1) | 语法糖 |
| Nibble | 函数 | 1414 | `Nibble()` | BitsInteger(4) | 语法糖 |
| Octet | 函数 | 1418 | `Octet()` | BitsInteger(8) | 语法糖 |

### 对偶关系（construct 核心对称性）

```
BytesInteger(n)            <-->  Bitwise(BitsInteger(8*n))
BitsInteger(8*n)           <-->  Bytewise(BytesInteger(n))
```

四种 bit/byte × big/little 组合（core.py:1344-1355）：
```
BitsInteger(16)                              # Byte-BE, Bit-BE
BitsInteger(16, swapped=True)                # Byte-LE, Bit-BE
ByteSwapped(BitsInteger(16))                 # Byte-LE, Bit-LE
ByteSwapped(BitsInteger(16, swapped=True))   # Byte-BE, Bit-LE
```

## 3. 内部 bit 流管理（bitstream.py, 147 行）

### RestreamedBytesIO（line 6-71）

构造：`RestreamedBytesIO(substream, decoder, decoderunit, encoder, encoderunit)`

- rbuffer / wbuffer：两个 bytes 缓冲（读/写分离）
- read(count)：while 缓冲不足 → 从 substream 按 decoderunit 读 → decoder 解码 → 追加缓冲 → 切片返回
- write(data)：追加 wbuffer → while 够 encoderunit → encoder 编码 → 写入 substream
- close()：校验缓冲已清空
- seekable() 返回 False

Bitwise 用它时参数为 (stream, bytes2bits, 1, bits2bytes, 8)。

### binary.py 转换工具（169 行）

| 函数 | 作用 |
|------|------|
| integer2bits(number, width, signed) | int → bit 串（MSB-first） |
| bits2integer(data, signed) | bit 串 → int（MSB-first） |
| bytes2bits(data) | byte 串 → bit 串（8 倍展开，查表） |
| bits2bytes(data) | bit 串 → byte 串（要求 len 是 8 倍数） |
| swapbytes(data) | 字节串整体反序 |
| swapbytesinbits(data) | bit 串按 8 位组反序组顺序 |
| swapbitsinbytes(data) | 每字节内反转 bit 序 |

### 易混淆点

- swapbytesinbits（用于 BitsInteger(swapped=True)）：操作 bit 串，按 8 位组反序
- swapbitsinbytes（用于 BitsSwapped）：操作 byte 串，字节内 bit 反序

## 4. 公开 API（__init__.py）

Bit / BitsInteger / BitsSwapped / BitStruct / Bitwise / BytesInteger / ByteSwapped / Bytewise / Nibble / Octet / BitwisableString

## 5. 继承关系

Bitwise 返回 Transformed 或 Restreamed，两者都继承 Subconstruct。Subconstruct 默认委托 parse/build/sizeof 给 subcon，但 Transformed/Restreamed 覆盖了这三个方法。

## 6. sizeof 语义

- Bitwise 的 sizeof = subcon.sizeof() / 8（subcon 报 bit 数，外层报字节数）
- Bytewise 的 sizeof = subcon.sizeof() * 8（subcon 报字节数，外层报 bit 数）
- subcon 的 bit 数必须是 8 的倍数，否则 build/parse 时暴露

## 7. 注意事项

- Bytewise 必须在 Bitwise 内使用（单独使用无意义）
- BitwisableString 是 FlagsEnum 的 Python 语法糖，与 bit 流解析无关
- Padding 需要同步实现（BitStruct 末尾补齐到字节边界）
