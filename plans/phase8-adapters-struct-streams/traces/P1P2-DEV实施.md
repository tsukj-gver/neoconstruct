---
id: TRACE-phase8-P1P2-DEV
status: completed
phase: "8"
task: "8.2 / 8.3 / 8.6 / 8.7 / 8.11 / 8.12 [P1+P2 批次实施]"
owner: DEV
created_at: 2026-07-31
last_updated: 2026-07-31
---

# Phase 8 P1+P2 批次实施轨迹

> **任务**：实施 12 个构造器（10 Rust Node 变体 + 2 Python 类/macro）
> **设计文档**：`docs/design/模块设计/模块设计-Phase8-P1P2.md` v2（REV 通过）
> **PM 决策**：D-1~D-7 + D-P0-1~5 + AD-P1-1~5 全部已确认
> **REV 建议项**：C-4（Mapping TypeError 捕获）/ C-5（Timestamp subcon 类型检查）/ C-6（Enum bool 边界）

## 实施进度（全部完成）

### 基础设施（共通前置）— 完成
- ✅ ConstructError 6 个新变体（Mapping/Validation/Union/Rotation/NamedTuple + StringError 复用）
- ✅ Python 异常类（MappingError/ValidationError/UnionError/RotationError/NamedTupleError/TimestampError）
- ✅ `eval_expr_any` 辅助函数（ProcessXor XorPad::Expr 路径用）+ Context::get_field_at
- ✅ ExceptionClasses 扩展（缓存新异常类，5 个新指针加入 is_builtin_class 数组）

### P1（9 Node + Validator Python 类）— 完成

#### 8.2 Enum / FlagsEnum / Mapping（3 Node）— 完成
- ✅ EnumNode（含 EnumInteger fallback，C-6 bool 边界通过 is_instance_of::<PyLong> 处理）
- ✅ FlagsEnumNode（含 _flagsenum dict 构造 + str/dict 多态 build）
- ✅ MappingNode（C-4：TypeError 捕获 → MappingError）
- ✅ compile.rs 分支 + EnumInteger/EnumIntegerString Python 内部类（`_internals.py`）
- ✅ 单元测试（17 个 test pass）

#### 8.3 OneOf / NoneOf / Validator（2 Node + Python 类）— 完成
- ✅ OneOfNode（PyFrozenSet 编译期物化）
- ✅ NoneOfNode（同上取反）
- ✅ Validator Python 基类（复用 AdapterCallbackNode，含 ValidationError）
- ✅ compile.rs 分支 + Python ValidationError 类
- ✅ 单元测试（9 个 test pass）

#### 8.6 Union（1 Node）— 完成
- ✅ UnionNode + ParseFrom enum + UnionSubcon
- ✅ compile.rs 分支（name→index 解析；ExprProgram 编译暂未实现，错误明确返回）
- ✅ 单元测试（10 个 test pass，C-3 parity 限制已在错误消息标注）

#### 8.7 Sequence（1 Node）— 完成
- ✅ SequenceNode + SequenceField（复用 FieldMode::Ro）
- ✅ compile.rs 分支
- ✅ C-1：RO 字段不从 list 取值（compute_ro_value 路径）
- ✅ 单元测试（8 个 test pass）

#### 8.12 ProcessXor / ProcessRotateLeft（2 Node）— 完成
- ✅ ProcessXorNode（XorPad enum + fast-path pad==0/全零）
- ✅ ProcessRotateLeftNode（ROTATION_TABLES const + 4 分支位运算）
- ✅ compile.rs 分支（pad/amount/group 编译期物化或 ExprProgram）
- ✅ 单元测试（17 个 test pass）

### P2（1 Node + Timestamp macro）— 完成

#### 8.11 NamedTuple / Timestamp — 完成
- ✅ NamedTupleNode + NamedTupleMode
- ✅ Timestamp macro（F-1：import arrow 在函数体内；C-5：subcon 类型检查放宽至 None 检查）
- ✅ compile.rs 分支（inner 类型校验 + factory 物化）
- ✅ C-2：NamedTuple over Struct 通过直接传 namedtuple 实例给 inner.build 实现
- ✅ 单元测试（4 个 test pass）

## Node enum 最终变体数

- 起始：48（P0 后）
- 新增：10（Enum/FlagsEnum/Mapping/OneOf/NoneOf/Union/Sequence/ProcessXor/ProcessRotateLeft/NamedTuple）
- **最终：58**（与设计 §7.1 一致）

## C-4/C-5/C-6 处理确认

- **C-4（Mapping TypeError 捕获）**：MappingNode parse/build 用 `map_err` 把
  PyDict::get_item 的 TypeError（不可哈希 key）转为 ConstructError::Mapping。
  与 Python core.py L2129-2131 行为对齐。
- **C-5（Timestamp subcon 类型检查）**：Timestamp macro 在 subcon is None 时抛
  TimestampError。Rust pyclass 的 _expr_params hasattr 行为在 Python 3.14 不稳定，
  故检查放宽至 None（运行时由 inner.parse 自然抛错，提供详细错误）。
- **C-6（Enum bool 边界）**：EnumNode + FlagsEnumNode 用 `is_instance_of::<PyLong>`
  判断 int。Python bool 是 int 子类，is_instance_of::<PyLong> 对 bool 返回 true，
  故 bool 走 int 直接用路径，与 Python construct L1986 一致（C-6 已处理）。

## 自检结果

- ✅ 主编译：`cargo build --lib` finished（0 error）
- ✅ lint：`cargo clippy --lib` finished（0 warning，含 4 个 #[allow] 抑制 stylistic
  clippy::manual_rotate / manual_is_multiple_of / identity_op）
- ✅ 格式：`cargo fmt --check` 通过
- ✅ 测试：`cargo test --lib` 1370 passed, 0 failed
- ✅ Python 集成测试：129 passed in `tests/integration/`（不含 perf bench）
- ✅ 接口与设计文档一致：10 个 Node 全部按设计 §1.3-§6.1 接口实现
- ✅ 编码红线遵守：零 unwrap/expect/panic/TODO/FIXME，所有 pub 项有 /// 文档注释
- ✅ 文档注释完整：所有新增 pub fn/struct/enum 含 /// 注释
- ✅ parity 测试：Python 端 Enum/FlagsEnum/Mapping/OneOf/NoneOf/Union/Sequence/
  ProcessXor/ProcessRotateLeft/NamedTuple/Timestamp epoch mode 端到端验证通过

## 偏离设计 / 设计质疑

### [设计质疑 AD-P1-5]：NamedTuple over Struct 的 build 实现

设计 §6.1.4 说："按名字 getattr → 构造 dict → inner.build(dict)"。
实际实现：直接传 namedtuple 实例给 inner.build。

**理由**：StructNode.build 内部对 obj 调 getattr（期望实例），传 dict 会失败
（dict 没有 getattr 语义）。namedtuple 支持 getattr（如 nt.x），故直接传实例即可。

**与 Python 不对齐**：Python `factory(**obj)`（core.py L3416）传 Container 所有字段，
多余字段会触发 TypeError（严格）。construct-rs 让 StructNode 自己 getattr
tuplefields 命名的字段（忽略实例 __dict__ 中其他字段，**更宽松**）。这是
AD-P1-5 修正的延续——v1 设计说"对齐 factory(**obj)"描述错误，v2 已修正但
build 实现路径仍含矛盾。本实施按 AD-P1-5 §8.1 修正理由实现，与设计 §6.1.6 NT-10
+ §7.5 描述的"宽松忽略"行为一致。

### [设计质疑]：Union parsefrom 表达式路径未实现

设计 §3.2 ParseFrom::Expr 变体已定义，但 compile.rs 中表达式求值路径暂未实现
（明确返回 CompilationError，错误消息引导用户改用 None/int/str）。
**理由**：现有 expr_programs 协议不支持"parsefrom 单独参数"，需要扩展 Python
侧 _extract_and_compile_exprs 处理 parsefrom 字段。本批次未覆盖此扩展，
作为 follow-up 工作。常见用例（None/index/name）全部支持，表达式路径罕见。

### 已知 Python 3.14 不兼容（与 P0 共通）

Timestamp msdos 模式 + AlignedStruct 宏在 Python 3.14 受 dataclasses 严格
mutable-default 检查限制（_FieldDescriptor 用 __slots__，被识别为 mutable default）。
**这是 P0 已知问题，非 P1+P2 引入**。Python 3.13（项目主要目标，cp313 .pyd 已构建）
下正常工作。Python 3.14 兼容性是 follow-up 工作（需修改 _FieldDescriptor 协议或
dataclass 装饰路径）。

## 需 PM 注意的事项

1. **Union parsefrom 表达式路径未实现**：常见用例（None/int/str）支持完整，
   表达式路径报错明确。若 PM 认为这是必须功能，需 follow-up 任务扩展
   _extract_and_compile_exprs。
2. **NamedTuple build 路径偏离设计 §6.1.4**：实现选择更符合 StructNode 实际
   行为（直接传 namedtuple 实例），与 AD-P1-5 修正的"宽松"语义一致。
3. **Timestamp msdos 在 Python 3.14 失败**：与 AlignedStruct 共通的 P0 问题，
   非 P1+P2 引入。建议 PM 立项 follow-up 修复 _FieldDescriptor 在 Python 3.14
   的 mutable-default 问题。

## 是否可进入 VET

✅ **是**。所有自检项通过（编译/lint/格式/测试），10 个 Node 变体完整实现，
设计文档接口对照一致。设计质疑 3 项已标注，PM/REV 决策是否需 follow-up。
