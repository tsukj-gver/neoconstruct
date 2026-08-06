---
id: QUERY-hexdump-disposition
status: pending-user-decision
phase: "8"
task: "8.4 [Hex/HexDump 语义重新设计]"
type: decision-request
depends_on:
  - DESIGN-Phase8-P0
created: 2026-07-31
last_updated: 2026-07-31
---

# 决策请求：HexDump 处理方案

> **角色**：ARCH
> **背景**：用户决策将 Hex 语义从"display 装饰器"改为"hex 字符串编解码器"（详见
> `模块设计-Phase8-P0.md §2.0`）。HexDump 原是 Hex 的姊妹构造器（xxd 风格字节流 dump），
> 需要决定在新语义下的处理方式。PM 请转达用户决策。

---

## 0. 前提：HexDump 与 Hex 的本质差异

| 维度 | Hex（新语义，已定） | HexDump（待决策） |
|------|-------------------|------------------|
| 输出 | 纯 hex 字符串 `"deadbeef"` | xxd 风格多行格式（地址 + hex + ASCII） |
| 用途 | 数据编解码（round-trip 对称） | 人类可读调试显示（阅读二进制内容） |
| build 逆向 | 简单（`hex::decode` 一行） | 复杂（剥地址偏移、ASCII 列、多行结构） |

**核心问题**：Hex 的语义变更动机是"消除值/显示分离的性能开销 + round-trip 对称"。但
HexDump 的核心价值是"人类可读的多行格式化显示"——它本质上是**纯显示功能**，而非数据编解码。
将 HexDump 改为"返回 hexdump 格式字符串"在语义上成立，但 build 逆向解析的价值存疑。

---

## 方案 A：改为"返回 hexdump 格式字符串"（parse + build 双向）

### 描述
- `HexDump(length)` 接收字节数（同新 Hex）
- parse：读 N 字节 → Rust 内构造 xxd 风格多行字符串 → 返回 PyString
- build：接收 xxd 风格字符串 → Rust 内逆向解析 → 写字节

### 优点
- 与 Hex 语义一致（都是字符串编解码器）
- parse 性能好（Rust 内构造字符串，无 Python 字节码）

### 缺点
- **build 逆向解析复杂**：需从多行格式中剥离地址偏移（`0000   `）、hex 列、ASCII 列，
  还需处理不同 linesize、边界行（最后一行不足 16 字节）。原版 `hexundump` 用 Python
  字符串 split/slice 实现，移植到 Rust 需状态机或正则，工程量 ~150 行，易出边界 bug
- **round-trip 不对称**：parse 输出的 ASCII 列在 build 时需被忽略（冗余信息），用户易困惑
- parse 构造多行格式字符串比纯 hex encode 贵（需逐字节格式化地址/空格/ASCII），但仍远快于 Python

### 性能预估
- parse：~200-300ns（多行格式化，N=16）—— 预计 ≥8x（可达 10x）
- build：~150ns（逆向解析）—— 预计 ≥10x

---

## 方案 B：与 Hex 合并

### 描述
删除 HexDump，用户统一用 Hex。

### 评估
**不推荐**。理由：
- HexDump 的 xxd 多行带 ASCII 格式与 Hex 的纯 hex 字符串本质不同
- 合并会丢失 hexdump 的可读性优势（调试时多行带 ASCII 远比纯 hex 字符串易读）
- 用户此前质疑"两者区别"是在旧语义下（都是 display 装饰器）；新语义下两者用途已分化

---

## 方案 C：标 wont_implement（ARCH 推荐） ⭐

### 描述
- 删除 HexDump 构造器（Node 变体、描述符、测试、bench case）
- 保留 `construct.lib.hex.hexdump(data, linesize)` 函数（纯 Python，供用户可选调用）
- 用户需要 hexdump 显示时，对 parse 出的 bytes 做后处理：

```python
from construct import Bytes
from construct.lib.hex import hexdump

@dataclass
class P(StructMixin):
    payload: bytes = field(Bytes(16))

result = P.parse(data)
print(hexdump(result.payload, 16))  # 纯显示，不进入执行树
```

### 优点
1. **消除 build 逆向解析的工程负担**（~150 行易错代码 + 边界测试）
2. **语义清晰**：hexdump 是纯显示功能，不应进入数据编解码执行树
3. **代码精简**：删除 ~250 行（HexDumpNode + impl + 描述符 + 测试 + bench）
4. **不损失能力**：用户仍可通过 `construct.lib.hex.hexdump()` 函数获得可读 hex dump
5. **与 Hex 新语义解耦**：Hex 是数据编解码器（进执行树），hexdump 是显示工具（用户域后处理）

### 缺点
- 不再支持 `HexDump(N)` 作为 Struct 字段（无法 `field(HexDump(16))`）
- 原版用 `HexDump(subcon).parse(...)` 一行得到格式化字符串的用户需改为两步（parse + hexdump 调用）

### 性能影响
- 无（hexdump 函数是用户域后处理，不进入 Rust 执行树，不影响 parse/build 基准）

---

## ARCH 推荐与理由

**推荐方案 C（wont_implement）**。

**核心理由**：
1. HexDump 在新语义下的定位尴尬——它既不是"数据编解码器"（round-trip 不对称），又因 Hex
   已覆盖纯 hex 字符串需求而失去"hex 显示"的独占价值
2. hexdump 的真正用途是**调试时的人类可读显示**，这是用户域后处理（`print(hexdump(data))`），
   不应作为执行树中的字段类型（强行进执行树反而引入 build 逆向解析的复杂度）
3. 工程经济性：方案 A 的 build 逆向解析是高成本低价值工作（~150 行易错代码，用户极少用
   hexdump 字符串做 build 输入）

**若用户坚持保留 HexDump 构造器**：ARCH 建议方案 A 的简化版——**仅实现 parse**（返回 hexdump
格式字符串），build 暂不支持（抛 NotImplementedError 或标记为 display-only）。这样保留
"parse 出可读字符串"的能力，又不承担 build 逆向解析的工程负担。

---

## 需要用户决策的问题

1. **HexDump 是否保留？**
   - 选 C（推荐）→ 删除 HexDump，保留 `hexdump()` 函数供用户后处理
   - 选 A → 实现为 hexdump 字符串编解码器（parse + build 双向，build 工程量较大）
   - 选 A 简化版 → 仅 parse（返回字符串），build 不支持

2. **若选 A/A 简化版，linesize 是否固定为 16？**
   - 原版默认 linesize=16。construct-rs 可固定 16（简化），或接收参数 `HexDump(length, linesize=16)`

---

## 对 PM 的话

本决策不影响 Hex 的重新设计（§2.0-§2.9 已完成，Hex 按"hex 字符串编解码器"实现）。
HexDump 的处理可在 Hex 实现完成后另行决策，不阻塞 Hex 的开发。建议将本决策与 Hex 验收
合并询问用户。
