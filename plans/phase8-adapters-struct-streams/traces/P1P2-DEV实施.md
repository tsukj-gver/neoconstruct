---
id: TRACE-phase8-P1P2-DEV
status: in-progress
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

## 实施进度

### 基础设施（共通前置）
- [ ] ConstructError 6 个新变体（Mapping/Validation/Union/Rotation/NamedTuple；StringError 已存在复用）
- [ ] Python 异常类（MappingError/ValidationError/UnionError/RotationError/NamedTupleError/TimestampError）
- [ ] `eval_expr_any` 辅助函数（ProcessXor XorPad::Expr 路径用）
- [ ] ExceptionClasses 扩展（缓存新异常类）

### P1（9 Node + Validator Python 类）

#### 8.2 Enum / FlagsEnum / Mapping（3 Node）
- [ ] EnumNode（含 EnumInteger fallback）
- [ ] FlagsEnumNode（含 _flagsenum dict 构造 + str/dict 多态 build）
- [ ] MappingNode（C-4：TypeError 捕获 → MappingError）
- [ ] compile.rs 分支 + EnumInteger/EnumIntegerString Python 内部类
- [ ] 单元测试

#### 8.3 OneOf / NoneOf / Validator（2 Node + Python 类）
- [ ] OneOfNode（PyFrozenSet 编译期物化）
- [ ] NoneOfNode（同上取反）
- [ ] Validator Python 基类（复用 AdapterCallbackNode，C-6 bool 边界处理）
- [ ] compile.rs 分支 + Python ValidationError 类

#### 8.6 Union（1 Node）
- [ ] UnionNode + ParseFrom enum + UnionSubcon
- [ ] compile.rs 分支（含 name→index 解析 + ExprProgram 编译）
- [ ] 单元测试（C-3：context.update + flagbuildnone 已知 parity 限制标注）

#### 8.7 Sequence（1 Node）
- [ ] SequenceNode + SequenceField（复用 FieldMode::Ro）
- [ ] compile.rs 分支
- [ ] C-1：RO 字段不从 list 取值（compute_ro_value 路径）

#### 8.12 ProcessXor / ProcessRotateLeft（2 Node）
- [ ] ProcessXorNode（XorPad enum + fast-path pad==0/全零）
- [ ] ProcessRotateLeftNode（ROTATION_TABLES const + 4 分支位运算）
- [ ] compile.rs 分支（pad/amount/group 编译期物化或 ExprProgram）

### P2（1 Node + Timestamp macro）

#### 8.11 NamedTuple / Timestamp
- [ ] NamedTupleNode + NamedTupleMode
- [ ] Timestamp macro（F-1：import arrow 在函数体内；C-5：subcon 类型检查）
- [ ] compile.rs 分支（inner 类型校验 + factory 物化）
- [ ] C-2：NamedTuple over Struct 忽略多余字段

## 自检记录

每子任务完成后记录 cargo build/clippy/fmt/test 结果。

## 偏离设计 / 设计质疑

（实施过程中如有发现，记录于此）
