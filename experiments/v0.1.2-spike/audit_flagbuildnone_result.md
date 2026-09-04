# flagbuildnone × ValueKind 对账全表（脚本生成，禁手工）

- 生成脚本：`experiments/v0.1.2-spike/audit_flagbuildnone.py`
- 原版源：`neoconstruct\.venv-pc\Lib\site-packages\construct\core.py`（AST 提取，construct 2.10.70）
- neoconstruct 源：`neoconstruct\python\neoconstruct\__init__.py`（AST 提取导出全集）

## 原版侧提取结果

- `flagbuildnone = True` 叶类：**15** 个（断言通过 = 预期 15）：
  - `Const`（core.py:2840）
  - `Computed`（core.py:2912）
  - `Index`（core.py:2960）
  - `Rebuild`（core.py:3000）
  - `Default`（core.py:3051）
  - `Check`（core.py:3094）
  - `Error`（core.py:3142）
  - `StopIf`（core.py:4093）
  - `Peek`（core.py:4481）
  - `Seek`（core.py:4595）
  - `Tell`（core.py:4639）
  - `Pass`（core.py:4676）
  - `Terminated`（core.py:4716）
  - `RestreamData`（core.py:5182）
  - `Checksum`（core.py:5553）
- 传播类：**7** 个：
  - `Subconstruct`（core.py:795，透传 subcon）
  - `Struct`（core.py:2222，all(子构造器)）
  - `Sequence`（core.py:2379，all(子构造器)）
  - `Select`（core.py:3850，any(子构造器)）
  - `IfThenElse`（core.py:3964，and(then,else)）
  - `Switch`（core.py:4031，all(子构造器)）
  - `LazyStruct`（core.py:5991，all(子构造器)）
- Construct 基类默认：`flagbuildnone = False`（core.py:360）
- 其中 neoconstruct 未实现：['Error', 'RestreamData']（预留分类见下）

## neoconstruct 构造器 × ValueKind 分类全表

导出构造器总数：**100**（异常/Descriptor 类型名/工厂/Mixin 已过滤）

| 构造器 | ValueKind | 原版对应 | 原版判定 | 一致性 |
|--------|-----------|----------|----------|--------|
| Adapter | Instance | Adapter 族 | P | 传播（§5.1c）✓ |
| Aligned | Instance | Subconstruct 透传族 | P | 传播（§5.1c）✓ |
| AlignedStruct | Instance | Struct 家族 | P/F | 必填 ✓ |
| Array | Instance | Array 家族 | F | 必填 ✓ |
| Bit | Instance | BytesInteger | F | 必填 ✓ |
| BitsInteger | Instance | BytesInteger | F | 必填 ✓ |
| BitsSwapped | Instance | Subconstruct 透传族 | P | 传播（§5.1c）✓ |
| Bitwise | Instance | Subconstruct 透传族 | P | 传播（§5.1c）✓ |
| Byte | Instance | FormatField | F | 必填 ✓ |
| ByteSwapped | Instance | Subconstruct 透传族 | P | 传播（§5.1c）✓ |
| Bytes | Instance | Bytes | F | 必填 ✓ |
| BytesInteger | Instance | BytesInteger | F | 必填 ✓ |
| Bytewise | Instance | Subconstruct 透传族 | P | 传播（§5.1c）✓ |
| CString | Instance | String 家族 | F | 必填 ✓ |
| Check | Control | Check | T | 哑值可省 ✓ |
| Checksum | Control | Checksum | T | 哑值可省 ✓ |
| Computed | Computed | Computed | T | 哑值可省 ✓ |
| Const | Const | Const | T | 哑值可省 ✓ |
| Default | Default | Default | T | 哑值可省 ✓ |
| Double | Instance | FormatField | F | 必填 ✓ |
| Element | Control | Element | X | neoconstruct 扩展 |
| Enum | Instance | Adapter 族 | P | 传播（§5.1c）✓ |
| FlagsEnum | Instance | Adapter 族 | P | 传播（§5.1c）✓ |
| Float16b | Instance | FormatField | F | 必填 ✓ |
| Float16l | Instance | FormatField | F | 必填 ✓ |
| Float32b | Instance | FormatField | F | 必填 ✓ |
| Float32l | Instance | FormatField | F | 必填 ✓ |
| Float64b | Instance | FormatField | F | 必填 ✓ |
| Float64l | Instance | FormatField | F | 必填 ✓ |
| FocusedSeq | Instance | Struct 家族 | P/F | 必填 ✓ |
| GreedyBytes | Instance | Bytes | F | 必填 ✓ |
| GreedyRange | Instance | Array 家族 | F | 必填 ✓ |
| GreedyString | Instance | String 家族 | F | 必填 ✓ |
| Half | Instance | FormatField | F | 必填 ✓ |
| Hex | Instance | Adapter 族 | P | 传播（§5.1c）✓ |
| HexDump | Instance | Adapter 族 | P | 传播（§5.1c）✓ |
| If | Instance | 条件/选择 | P | 传播（§5.1c）✓ |
| IfThenElse | Instance | 条件/选择 | P | 传播（§5.1c）✓ |
| Index | Index | Index | T | 哑值可省 ✓ |
| Int | Instance | FormatField | F | 必填 ✓ |
| Int16sb | Instance | FormatField | F | 必填 ✓ |
| Int16sl | Instance | FormatField | F | 必填 ✓ |
| Int16ub | Instance | FormatField | F | 必填 ✓ |
| Int16ul | Instance | FormatField | F | 必填 ✓ |
| Int24sb | Instance | BytesInteger | F | 必填 ✓ |
| Int24sl | Instance | BytesInteger | F | 必填 ✓ |
| Int24ub | Instance | BytesInteger | F | 必填 ✓ |
| Int24ul | Instance | BytesInteger | F | 必填 ✓ |
| Int32sb | Instance | FormatField | F | 必填 ✓ |
| Int32sl | Instance | FormatField | F | 必填 ✓ |
| Int32ub | Instance | FormatField | F | 必填 ✓ |
| Int32ul | Instance | FormatField | F | 必填 ✓ |
| Int64sb | Instance | FormatField | F | 必填 ✓ |
| Int64sl | Instance | FormatField | F | 必填 ✓ |
| Int64ub | Instance | FormatField | F | 必填 ✓ |
| Int64ul | Instance | FormatField | F | 必填 ✓ |
| Int8sb | Instance | FormatField | F | 必填 ✓ |
| Int8sl | Instance | FormatField | F | 必填 ✓ |
| Int8ub | Instance | FormatField | F | 必填 ✓ |
| Int8ul | Instance | FormatField | F | 必填 ✓ |
| Long | Instance | FormatField | F | 必填 ✓ |
| Mapping | Instance | Adapter 族 | P | 传播（§5.1c）✓ |
| NamedTuple | Instance | Adapter 族 | P | 传播（§5.1c）✓ |
| Nibble | Instance | BytesInteger | F | 必填 ✓ |
| NoneOf | Instance | Adapter 族 | P | 传播（§5.1c）✓ |
| NullStripped | Instance | String 家族 | F | 必填 ✓ |
| NullTerminated | Instance | String 家族 | F | 必填 ✓ |
| Octet | Instance | BytesInteger | F | 必填 ✓ |
| OneOf | Instance | Adapter 族 | P | 传播（§5.1c）✓ |
| PaddedString | Instance | String 家族 | F | 必填 ✓ |
| Padding | Void | Padded/Pass | T | 哑值可省 ✓ |
| PascalString | Instance | String 家族 | F | 必填 ✓ |
| Pass | Void | Padded/Pass | T | 哑值可省 ✓ |
| Peek | Control | Peek | T | 哑值可省 ✓ |
| Pointer | Instance | Subconstruct 透传族 | P | 传播（§5.1c）✓ |
| Prefixed | Instance | Subconstruct 透传族 | P | 传播（§5.1c）✓ |
| PrefixedArray | Instance | Array 家族 | F | 必填 ✓ |
| Probe | Instance | Subconstruct 透传族 | P | 传播（§5.1c）✓ |
| ProcessRotateLeft | Instance | Adapter 族 | P | 传播（§5.1c）✓ |
| ProcessXor | Instance | Adapter 族 | P | 传播（§5.1c）✓ |
| RawCopy | Instance | Adapter 族 | P | 传播（§5.1c）✓ |
| Rebuild | Rebuild | Rebuild | T | 哑值可省 ✓ |
| Renamed | Instance | Subconstruct 透传族 | P | 传播（§5.1c）✓ |
| RepeatUntil | Instance | Array 家族 | F | 必填 ✓ |
| Seek | Control | Seek | T | 哑值可省 ✓ |
| Select | Instance | 条件/选择 | P | 传播（§5.1c）✓ |
| Sequence | Instance | Struct 家族 | P/F | 必填 ✓ |
| Short | Instance | FormatField | F | 必填 ✓ |
| Single | Instance | FormatField | F | 必填 ✓ |
| StopIf | Control | StopIf | T | 哑值可省 ✓ |
| Subconstruct | Instance | Subconstruct 透传族 | P | 传播（§5.1c）✓ |
| Switch | Instance | 条件/选择 | P | 传播（§5.1c）✓ |
| SymmetricAdapter | Instance | Adapter 族 | P | 传播（§5.1c）✓ |
| Tell | Tell | Tell | T | 哑值可省 ✓ |
| Terminated | Control | Terminated | T | 哑值可省 ✓ |
| Timestamp | Instance | Adapter 族 | P | 传播（§5.1c）✓ |
| Union | Instance | Struct 家族 | P/F | 必填 ✓ |
| Validator | Instance | Adapter 族 | P | 传播（§5.1c）✓ |
| VarInt | Instance | BytesInteger | F | 必填 ✓ |
| ZigZag | Instance | BytesInteger | F | 必填 ✓ |

## 未实现的原版 flagbuildnone=True 构造器（预留分类）

| 原版构造器 | 预留 ValueKind | 依据 |
|-----------|---------------|------|
| Error | Control | parse/build 无条件 ExplicitError（core.py:3142）|
| RestreamData | Control | build no-op return obj（core.py:5182）|

## 分类分布统计

- Instance: 85
- Control: 7
- Void: 2
- Const: 1
- Computed: 1
- Index: 1
- Rebuild: 1
- Default: 1
- Tell: 1
- 原版判定分布：T=14 / F=52 / P=29 / X=1

## 结论

**PASS**：原版 flagbuildnone 语义（15 叶类 + 7 传播类 + 基类 False）与 ValueKind v2 分类逐一对账一致；导出构造器 100 个全覆盖。
