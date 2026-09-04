---
id: EXT-vetter
project: neoconstruct
phase: meta
last_updated: 2026-07-27
---

# VET 项目特定拓展（neoconstruct）

> 配合 `.opencode/agents/vetter.md`（跨工程 base）使用。
> 工作流状态机与子任务标识格式见 `AGENTS.md §1`。
> VET 在 **CODE_REVIEW** 阶段执行。**驳回目标**：`CODING`（如涉及设计变更，PM 协调先回退至 `DESIGNING`）。

## 行为一致性项目特定

基准：设计文档（`docs/design/`）与语义行为基线（已验收测试锁定的行为）。**禁止以 Python construct 行为作为审查依据**（benchmark 性能对比除外，见 AGENTS.md §0 第 9 条）。

对照方法：
- [ ] parse：相同二进制 → 解析结果符合设计规格
- [ ] build：相同输入值 → 字节输出符合设计规格
- [ ] round-trip：parse→build 幂等（设计声明对称的构造器）
- [ ] 值语义：对象字段值断言与语义规格一致（不只是字节）

## 错误处理项目特定

补充 base §错误处理：
- [ ] 流操作（read/write/seek）错误正确传播
- [ ] `Value` 类型转换错误处理（项目特定的动态类型枚举）
- [ ] 整数运算溢出检查（Rust `as` 转换需特别审）
- [ ] 错误携带 `path` 字段（项目硬要求，详见 `developer-extension.md §Rust 编码红线`）

## 资源安全项目特定（Rust）

补充 base §资源安全：
- [ ] 无 `unsafe` 或有安全论证
- [ ] 无不必要 `clone()`（性能考量）

## API 一致性项目特定（Rust）

补充 base §API 一致性：
- [ ] snake_case 命名风格统一
- [ ] 与设计文档（`docs/design/模块设计/模块设计-*.md`）签名一致

## 逻辑正确性项目特定

补充 base §逻辑正确性：
- [ ] Context 读写时序正确（项目特定的上下文对象）

## 项目特定注意事项

- Python 行为对比是核心——不能跳过
- Rust 编码红线（禁止 `unwrap`/`expect`/panic 等）见 `developer-extension.md §Rust 编码红线`
- 性能数据由 PM 验证（`pm-performance-validation` skill），你只审代码

## 可写文件

- `plans/phaseN/过程记录.md`（审查结果，按 `harness/metadata-convention.md` 格式）
- `experiments/**`
