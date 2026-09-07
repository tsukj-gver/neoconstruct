# neoconstruct 测试遗漏面审查报告

> 审查人视角：资深测试工程师，首次接触本库，仅以 README + SKILL + 库本体为输入。
> 方法：先按文档把玩（约 7 轮探针脚本，见本目录 `explore_0*.py`），再对照 `neoconstruct/tests/`（1228 passed / 8 skipped / 36s，全绿）逐面核对覆盖。
> 核心问题：**如果明天一万个真实开发者使用，什么问题最先暴露而现有测试完全不会发现？**
> 环境：Windows，CPython 3.13，neoconstruct 0.1.0（.venv），construct 2.10.70（.venv-pc，仅作行为对照）。

---

## 结论速览（按风险排序）

| # | 遗漏面 | 实测结果 | 严重度 | 建议优先级 |
|---|--------|---------|--------|-----------|
| 1 | 不可信 count 字段 → **进程级 abort**（Array/PrefixedArray 预分配） | 4 个用例复现，rc=0xC0000409，34GB 分配失败即崩 | Critical | P0 |
| 2 | **parse→build 回环对一切可选/条件字段断裂**（If/Switch/Select 命中 None） | 复现；construct 原版同场景通过；`default=None` 也救不回 | Critical | P0 |
| 3 | **StructMixin 继承静默丢基类字段**（build 产出错误字节，不报错） | 复现：3 字节协议 build 出 1 字节；parse 抛裸 AttributeError | High | P0/P1 |
| 4 | build 不接受 bytearray（parse 不接受是已测契约，build 侧是真空白） | 复现 GenericConstructError；同名测试从未传入 bytearray | High | P1 |
| 5 | `dataclasses.field` 遮蔽 / 忘记 `@dataclass` → **静默错误解析** | 复现：parse 返回默认值、0 字节消费、无任何报错 | Medium-High | P1 |
| 6 | GreedyRange 尾部残缺记录**静默丢弃**（rebuild != 输入，无警告） | 复现：5 字节进 4 字节出 | Medium | P2 |
| 7 | 测试架构缺少"崩溃隔离/健壮性"维度（无 fuzz、无并发、无泄漏防护、无 parse-后-rebuild 属性测试） | 架构事实（并发/泄漏今天实测健康，但无守护） | Medium | P1（架构）/ P2（用例） |
| 8 | 杂项 UX：`field(42)` 延迟报错、pyo3 错误消息泄漏、错误消息含 Rust 内部格式串 | 实测确认，均轻 | Low-Medium | P3 |

---

## 1. [Critical] 不可信 count 字段使整个 Python 进程 abort

**场景**：任何解析不可信输入的用户——网络报文、恶意/损坏文件、模糊测试。一个 32-bit 长度字段被翻转或被恶意置为 `0xFFFFFFFF`：

```python
@dataclass
class P1(StructMixin):
    count: int = field(Int32ub)
    items: list = field(Array(count, Int16ub))

P1.parse(b"\xFF\xFF\xFF\xFF")     # 无任何后续数据
# → 进程直接死亡：memory allocation of 34359738360 bytes failed
#   rc = 3221226505，Python 层无法 catch，不是 MemoryError
```

**实测矩阵**（`explore_02b_isolated.py` / `explore_05b_boundary.py`，子进程隔离逐例运行）：

| 输入 | 结果 |
|------|------|
| `Array(count=0xFFFFFFFF)` 无数据 / 带 100B 数据 | **abort**（34359738360 = 0xFFFFFFFF×8，结果 Vec 按count 预分配） |
| `Array(4294967295, Int8ub)` 字面常量（无不可信输入） | **abort**（同根因） |
| `PrefixedArray(Int32ub, Int8ub).parse(b"\xff\xff\xff\xff")` | **abort**（同根因） |
| `Array(count=160,000,000)`（1.28GB 预分配成功） | StreamError，安全 |
| `Bytes(n=0xFFFFFFFF)` / `Prefixed` 巨大长度 | StreamError，安全（不预分配） |
| 负 count（Int32sl） | RangeError，安全 |
| 嵌套 `Array(count, Array(1000, ...))` 巨大 count | StreamError，安全 |

**为什么现有测试发现不了**：全测试套件中 `Array(` 的 count 无一超过千级（grep 验证：最大为数万级语义测试）；没有任何"不可信输入/模糊"维度用例；也没有子进程隔离的健壮性 runner——pytest 进程内一旦 abort 整套测试直接崩掉，但根本没有用例走到这条路。

**严重度**：Critical。这是可被远端确定性触发的进程击杀（DoS）；且边界是"目标机可用内存"，CI 容器 / 边缘设备 / Lambda 上 1.6 亿这种"本机安全"的 count 也会崩。修法方向：预分配前用可用流长度封顶（`Bytes`/`Prefixed` 路径已是如此）。

**验证**：已运行，4 个独立复现（含字面常量与 PrefixedArray 变体）。

---

## 2. [Critical] parse→build 回环对一切可选/条件字段断裂

README 核心承诺："**parse/build 对称：互为逆运算**"。实测对条件字段不成立：

```python
@dataclass
class IFC(StructMixin):
    x: int = field(Int8ub)
    v: object = field(If(x > 0, Int8ub))

r = IFC.parse(b"\x00")     # x=0 → v=None（正确，Pass 不消费字节）
r.build()                  # FieldValueMissingError: root.v —— 解析出来的对象不能 build 回去
```

**实测**（`explore_02b` E1 / `explore_07` AB/AC）：
- `If` 条件为假 → parse 得 `v=None` → rebuild 抛 `FieldValueMissingError`；
- `Switch` key 未命中（`default=Pass`）→ 同样断裂；
- `Select` 兜底 None → 同样断裂；
- **`field(..., default=None)` 显式默认也救不回**（None 被实现归类为"缺值"）；
- construct 2.10.70 对照：同结构 `parse` 得 `v=None` 后 `build(parsed)` **正常**（Pass 分支吞掉 None，输出 `b'\x00'` / `b'\x07'`），两个场景（If 假分支、Switch 未命中）均已实测对照。

**用户画像**：网关/代理/抓包改包工具的标准流水线是 `parse(bytes) → 改一两个字段 → build(bytes) → 转发`。任何含可选字段的协议（TLS 可选扩展、IPv4 可变选项、Modbus 变体帧……），第一帧可选分支未命中就崩。变通只能给每个可选字段塞哑值——parsed 对象里它们是 None，用户得逐字段清洗。

**为什么现有测试发现不了**（这是测试**架构**层面的盲区，不只是用例缺失）：
- parity harness（`tests/_helpers/parity.py`）的执行段是 `parsed = extract(cls.parse(data))` + `built = build_factory().build()`——**build 永远从全新构造的实例出发，从不 build 解析产物**（roundtrip 是 build→parse，不是 parse→build）；
- `test_conditional_parity.py` S2（Switch default=Pass）为让用例过绿，build 侧显式传了哑值 `P(v=0)`，注释自述"rs 侧 build 显式传值（None ≡ 缺值）"——**用例围绕缺陷设计而非暴露缺陷**；
- 单元侧 `test_conditional.py:304` 只断言 parse 返回 None，不 build 回去。

**严重度**：Critical。直接违反 README 头部承诺，影响所有含条件分支的协议（SKILL 第 4/6 章的 Modbus/IEC104 示例全是条件分支，只是测试数据恰好全部命中已知分支）。

---

## 3. [High] StructMixin 继承：静默产出错误字节

**场景**：协议头复用是 Python 用户的天性——基类 Header + 子类 ExtendedHeader。实测：

```python
@dataclass
class BaseHeader(StructMixin):
    magic: int = field(Int16ub)

@dataclass
class ExtendedHeader(BaseHeader):
    version: int = field(Int8ub)

ExtendedHeader(magic=0xCAFE, version=7).build()   # → b'\x07' ！！
# magic 两个字节无声消失，无任何报错/警告 —— 静默数据损坏

ExtendedHeader.parse(b"\x00\x01\x02")
# → AttributeError: 'ExtendedHeader' object has no attribute 'magic'
#   裸 AttributeError，不是 ConstructError，except ConstructError 捕不住

class ExtEmpty(BaseHeader): pass   # 空子类：build() → b''，magic 照丢
```

**为什么现有测试发现不了**：grep 全套件——**零继承用例**。`test_edge_cases.py` 覆盖嵌套/前向引用/frozen/post_init，唯独没有"子类化"。

**严重度**：High。build 侧是最坏的失败类别（静默错误输出而非报错）；若实现上选择不支持继承，也应在 `__init_subclass__` 显式报 CompilationError，而不是产出错字节。至少 parse 侧的错误应该包成 ConstructError。

**验证**：`explore_03` H、`explore_07` AF，三种变体全部复现。

---

## 4. [High] build 不接受 bytearray / memoryview

**场景**：真实数据源大量产出 `bytearray`（socket.recv 放入预分配缓冲、serial.read、`readinto`、np.frombuffer 段）。实测：

- `Bytes` 字段 build 传 `bytearray`/`memoryview` → `GenericConstructError: expected bytes object ... received non-bytes value of type bytearray`；
- `parse(bytearray)` → `TypeError: argument 'data': 'bytearray' object cannot be converted to 'PyBytes'`（**注意**：这是已测契约，`test_error_recovery_invariants.py:76` 明确断言抛 TypeError——诚实说明：parse 侧行为有测试、是有意为之。但错误消息是 pyo3 内部消息泄漏，且与 construct 2.10.70 的 bytes-like 兼容面不一致，迁移用户会踩）。

**为什么现有测试发现不了（build 侧）**：`test_roundtrip.py:192` 的测试**名字就叫** `test_bytes_build_accepts_bytearray`，但函数体只测了 `bytes`——注释自述"bytearray 若支持应通过，若不支持应给出明确错误。此处验证实际行为"，然后什么都没验证。**名不符实的测试比没有测试更危险**（给人已覆盖的错觉）。

**严重度**：High（发生频率）/ Low（后果——错误信息尚可）。修复容易（`PyBuffer` 协议或 isinstance 检查转 bytes）。

---

## 5. [Medium-High] 描述符误用静默产生错误解析结果

**场景 A（field 遮蔽）**：用户肌肉记忆 `from dataclasses import dataclass, field`，再 `from neoconstruct import StructMixin, ...`，然后 `a: int = field(default=5)` 用了 dataclasses 的 field：

```python
@dataclass
class Shadowed(StructMixin):
    a: int = dc_field(default=5)     # dataclasses.field！

Shadowed.parse(b"\x00")   # → Shadowed(a=5)：0 字节被消费，输入被完全忽略，无任何报错
```

**场景 B（忘记 @dataclass）**：`NoDC.parse(b"\x05")` 返回无 repr、无 eq 的裸对象（属性倒是设了）；`NoDC(a=9)` 构造时才 TypeError——两个方向的行为不一致，用户在第一小时内必然困惑。

**为什么现有测试发现不了**：全套件无"API 误用"负向用例维度；`test_edge_cases.py` 对 `dc_field` 的用法是受支持的混合字段模式，没测"误当协议字段用"的路径。

**严重度**：Medium-High。静默错误数据（场景 A）比崩溃更难排查；一分钟内可被一万用户中的数百人踩中。

---

## 6. [Medium] GreedyRange 静默丢弃尾部残缺记录

`GreedyRange(Item).parse(b"\x01\x02\x03\x04\x05")`（Item 是 2 字节）→ 2 个元素，悬空的 `0x05` **无声消失**；`rebuild` 输出 4 字节 ≠ 输入 5 字节，无任何警告。损坏的尾部数据被吞掉而不是报错。

- 与 construct 原版语义一致（诚实说明：这不算行为回归，属继承自基线的语义选择）；
- 但结合 #2（可选字段 rebuild 断裂），"静默吞 + 断裂"叠加后用户没有任何手段发现输入异常；
- 测试侧：`GreedyRange` 用例全部用整倍数输入，无残尾用例。

**严重度**：Medium（数据完整性风险，长期静默）。

---

## 7. [Medium] 测试架构维度的结构性空白

这一条不是单个 bug，而是"下一万个用户来临时，哪些风险类别没有任何防线"：

| 维度 | 现状 | 今天实测健康？ |
|------|------|--------------|
| 崩溃隔离 runner | 无（subprocess 只用于跑 parity 双胞胎，从不隔离跑 neoconstruct 自身的健壮性用例） | N/A（#1 已实际炸了） |
| parse(bytes).build() == bytes 属性测试 | 无——parity harness 的 build 永远从新实例出发 | N/A（#2 已实际断裂） |
| 继承 | 零用例 | N/A（#3 已实际损坏） |
| 并发（多线程 parse/build/类编译） | 零用例 | **健康**（8 线程×2000 轮 parse/build 无错；4 线程并发定义类无竞态） |
| 内存泄漏防护 | 零用例 | **健康**（10 万次 parse tracemalloc 净增 0.00MB） |
| 类反复重定义（Jupyter churn） | 零用例 | **健康**（3000 次类定义无异常；泄漏未装 psutil 粗测未见增长） |
| 深嵌套递归 | 5 层用例 | **健康**（实测 256 层 OK） |
| 模糊/随机字节语料 | 无 | N/A |
| 大规模输入（10MB+） | "large" 止步 1024 字节 | 10M 元素 parse 0.27s，健康 |

**为什么重要**：并发/泄漏/重定义今天健康，但 Rust 侧任何一次重构都可能无声破坏它们——没有守护测试就没有回归信号。建议至少给"健康且易碎"的三项（并发、泄漏、深嵌套）补最小守护用例。

---

## 8. [Low-Medium] 杂项

- `field(42)` / `field("x")` 不在创建时报错，延迟到类编译才报（编译期消息尚可）；
- `parse(None/str/int)` 的 TypeError 是 pyo3 内部消息（`argument 'data': ... cannot be converted to 'PyBytes'`），应包装；
- 整数 build 越界错误消息含 Rust 内部格式串 `struct '>H' error during building`——可读但泄露实现；
- `Timestamp(Int32ub, "unix", 0)` 拒绝并给出正确指引（用 `1.0`），SKILL 表格里 `unit` 参数描述过于含糊，用户第一写法必错——但错误消息质量好，属文档级小问题。

---

## 诚实说明：验证后**不是**问题的面

以下按职业怀疑逐一验证过，实际健康或有覆盖，不应列入风险：

1. **截断输入**：所有粒度（0/1/2/3 字节）均抛带精确嵌套 path 的 `StreamError`（`root.items[1].y` 级别），`errors/` 目录有系统性覆盖——**覆盖良好**。
2. **表达式 VM 危险运算**：`a//0`、`a%0` → 干净 `ConstructError`；`2<<62`、`8<<62` → `ConstructError: integer overflow ... does not fit in i64`。Rust 无 panic 穿透，且 `test_expression_arithmetic.py` 有边界用例——**覆盖良好**（这恰是最容易 panic 的地方，做对了）。
3. **并发**：多线程 parse/build/类编译实测无错——**健康（但无守护用例）**。
4. **内存**：10 万次 parse 无泄漏；实例/类均可 deepcopy、pickle——**健康**。
5. `dataclasses.replace` / `asdict` / `astuple` / frozen / `__post_init__`：全部工作且有测试——**覆盖良好**。
6. **VarInt 恶意超长编码**（10+ 字节 continuation）：`IntegerError: VarInt overflow: exceeds 64 bits`——干净。
7. **IntEnum/bool 传给整数字段 build**：接受。
8. **KeyboardInterrupt 延迟**：10M 元素单次 parse 0.27s，信号在调用结束时即刻处理，无长时屏蔽。
9. `parse(bytearray)` 拒绝：是有测试的**有意契约**（虽然错误消息质量值得改进，且与 construct 迁移预期有摩擦）。

---

## 建议行动优先级

**P0（发版前必须）**
1. 修 #1：Array/PrefixedArray 预分配前按流剩余长度封顶（参照 Bytes 路径行为），并补"不可信 count"子进程隔离测试矩阵（0xFFFFFFFF / 负数 / 内存受限模拟）。
2. 修 #2：让 `None` 值经 Pass 分支 build 为"不写字节"（对齐 README 互逆承诺），并在测试架构中加入 **parse→build(parsed) 回环属性测试**（这条不改 harness，#2 这类问题永远测不出来）。
3. #3 二选一：支持继承（合并基类 annotations）或在 `__init_subclass__` 显式拒绝；当前"静默错字节"不可接受。

**P1（一周内）**
4. build 侧接受 bytes-like（bytearray/memoryview），把 `test_bytes_build_accepts_bytearray` 改成真的传入 bytearray（名实相符）。
5. `dataclasses.field` 误用与忘 `@dataclass` 的编译期检测（`__init_subclass__` 里识别 dataclasses.Field 实例 / 缺 `__dataclass_fields__` 即报 CompilationError）。
6. 补并发、泄漏、类重定义三件最小守护测试（今日健康、明日易碎）。

**P2/P3**
7. GreedyRange 残尾行为：至少文档明示；可考虑可选 strict 模式。
8. 错误消息去 pyo3 内部串、包装 TypeError。
9. 清理名不符实的测试名（`test_bytes_build_accepts_bytearray`、`test_mutual_reference_via_forward_ref` 的 docstring 承诺"互相引用"但注释自认省略）。

---

## 附：本目录文件

| 文件 | 内容 |
|------|------|
| `explore_01_basics.py` | README 上手 + 首小时行为（类型误用/pickle/deepcopy/IntEnum/bytearray） |
| `explore_02_hostile.py` | 敌意输入首轮（发现进程 abort） |
| `explore_02b_isolated.py` | 子进程逐例隔离复现（#1/#2/#6 初证） |
| `explore_03_ecosystem.py` | 生态集成：忘 @dataclass/field 遮蔽/继承/空类/unicode 字段名 |
| `explore_04_threads_mem.py` | 并发/内存/深嵌套/类重定义 |
| `explore_05_limits.py` + `explore_05b_boundary.py` | abort 边界矩阵 + GIL/信号延迟 + VarInt/Timestamp |
| `explore_06_workflows.py` | replace/asdict/自引用/字面量大 count/build 不匹配 |
| `explore_07_final.py` | None 回环全家族/错误继承链/继承变体/GreedyBytes 饥饿 |
