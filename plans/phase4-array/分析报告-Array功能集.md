---
id: REQUIREMENT-Array
status: accepted
phase: "4"
task: "Phase 4 Array 功能集需求分析"
last_updated: 2026-07-27
---

# Array 功能集分析报告（Python construct 2.10.70）

> 调查时间：2026-06-29
> 来源：`construct/construct/core.py`、`construct/construct/lib/containers.py`、`construct/construct/__init__.py`

## 1. Array 相关构造器

| 构造器 | 类型 | 行号 | 功能 |
|--------|------|------|------|
| Array(count, subcon, discard) | class | 2493 | 固定次数，count 支持 int 或 context lambda |
| GreedyRange(subcon, discard) | class | 2570 | 读到流结束或解析失败 |
| RepeatUntil(predicate, subcon, discard) | class | 2637 | 谓词为真时终止 |
| PrefixedArray(countfield, subcon) | function | 4934 | FocusedSeq + Rebuild + Array 的组合宏 |
| LazyArray(count, subcon) | class | 6118 | 懒求值版本 |
| Index | class | 2934 | 从 context 取 _index，sizeof=0 |
| StopIf(condfunc) | class | 4079 | 用 StopFieldError 异常做早停信号 |

注：Range 不存在于 2.10.70（2.11+ 才引入）。

## 2. 各构造器的 parse/build/sizeof 流程

### Array
- parse：evaluate(count) → for i in range(count): context._index=i; subcon._parsereport → append
- build：evaluate(count) → 校验 len(obj)==count → for each: subcon._build
- sizeof：count × subcon.sizeof()（静态可计算）
- discard=True：仍消耗流但不收集结果

### GreedyRange
- parse：无限循环，每次记录 fallback=stream.tell()，子构造器失败时 seek 回 fallback
- 三种异常处理：StopFieldError→正常终止；ExplicitError→上抛；其他→seek回退+正常返回
- build：遍历所有元素，StopFieldError 早停
- sizeof：永远 SizeofError

### RepeatUntil
- parse：无限循环，先 append 再判断谓词（最后触发的元素被包含）
- 谓词签名：(obj, list, context) -> bool
- 不吞异常（失败直接上抛，不像 GreedyRange 回退）
- build：遍历到谓词满足，无元素满足则 RepeatError
- sizeof：永远 SizeofError

### PrefixedArray
- 不是独立类，是 FocusedSeq + Rebuild + Array 的组合宏
- parse：读 countfield → Array(count, subcon).parse
- build：Rebuild(countfield, len_(items)) → Array(count, subcon).build

### Index
- parse/build：从 context 取 _index，sizeof=0
- 完全依赖外层数组写 context._index

### StopIf
- parse/build：条件为真时 raise StopFieldError
- 仅 Struct/Sequence/GreedyRange 捕获此异常

## 3. ListContainer

`class ListContainer(list)` — 纯 list 子类，仅添加 repr/str 美化和 search/search_all。
`==` 行为与普通 list 完全一致。construct-rs 可直接返回原生 list。

## 4. 与其他构造器的交互

- Array 内放 Struct：正常，Struct 子 context 可通过 context 链访问外层 _index
- Array 内放 Array：正常，多维数组
- Array count 为表达式：通过 evaluate() 求值
- GreedyRange seek 回退：失败时回退到最后成功位置，否则外层会从错误位置继续

## 5. 公开 API

Array, GreedyRange, RepeatUntil, PrefixedArray, LazyArray, LazyListContainer, ListContainer, Index, StopIf, len_

异常：RangeError, RepeatError, StopFieldError, IndexFieldError

## 6. construct-rs 当前状态

| 需要 | 状态 |
|------|------|
| Node::Array 变体 | ❌ 不存在 |
| 编译管线识别 Array 描述符 | ❌ |
| ParseStream.seek(pos) | ❌ 缺失（GreedyRange 硬依赖） |
| ExprOp 读 _index | ❌ 缺失 |
| Context 写 _index | ⚠️ 可走 PyDict 路径但无 buf 加速 |
| Path.push_index(i) | ✅ 已存在 |
| RangeError/RepeatError/StopField 错误变体 | ❌ |

## 7. 关键设计要点

1. **ParseStream.seek**：GreedyRange 硬依赖。需处理 bit_pos（GreedyRange 通常字节对齐）
2. **_index 表达式支持**：当前 ExprOp 只有 GetInt(编译期字段索引)。需要新增读 _index 的能力
3. **StopFieldError 的 Rust 化**：Python 用异常做控制流，Rust 应改为 Result 哨兵变体
4. **RepeatUntil 谓词跨 FFI**：每次迭代调 Python callable，性能风险。可编译为 ExprProgram 的常见模式走零 FFI 路径
5. **PrefixedArray 可组合获得**：若同时实现 Array + Rebuild + FocusedSeq，PrefixedArray 是宏
6. **ListContainer**：直接返回原生 list
