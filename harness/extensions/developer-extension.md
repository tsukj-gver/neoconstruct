---
id: EXT-developer
project: construct-rs
phase: meta
last_updated: 2026-07-27
---

# DEV 项目特定拓展（construct-rs）

> 配合 `.opencode/agents/developer.md`（跨工程 base）使用。

## 工作流状态名

子任务状态机：`PENDING → DESIGNING → DESIGN_REVIEW → CODING → CODE_REVIEW → ACCEPTED → AUDITED`

DEV 在 **CODING** 阶段执行。

## 子任务标识格式

`<phase>.<子任务编号> [任务名]`。

## 项目特定自检项（补充 base 通用自检）

性能（当阶段总纲有 S-PERF 标准时必填）：
- [ ] 按 `.opencode/skills/performance-gate/SKILL.md` 性能门禁要求运行 benchmark（口径、基线、方法）
- [ ] 报告中包含性能数据表（场景 × 方向 × 加速比）
- [ ] 如果子任务尚未端到端可用，说明原因并提供可用的组件级数据

接口对照（项目特定）：
- [ ] 公开 API 与设计文档（`docs/design/模块设计/模块设计-*.md`）签名一致
- [ ] 与已实现模块的接口风格一致

代码质量（Rust 项目特定）：
- [ ] 无 unwrap()/expect() 在非测试代码中
- [ ] 无 panic — 非法输入返回 Err
- [ ] 无硬编码魔法数字
- [ ] 所有 pub 项有 /// 文档注释
- [ ] 错误携带 path 字段

parse/build 对称性（construct-rs 核心约束）：
- [ ] parse 和 build 均已实现
- [ ] build 后 parse 可还原原始数据

## Rust 编码红线（项目特定）

1. **禁止 `unwrap()` / `expect()` 在非测试代码中出现** — 所有可能失败的操作必须返回 `Result`
2. **禁止 panic** — 非法输入必须返回 `Err` 而非 panic
3. **禁止 `TODO` / `FIXME`** — 当轮任务当轮解决
4. **禁止硬编码魔法数字** — 使用常量或枚举
5. **所有 `pub` 项必须有 `///` 文档注释**
6. **错误必须携带 `path` 字段** — 用于追踪出错位置
7. **parse/build 对称性** — 所有构造器必须同时支持 parse 和 build

## 项目特定工作流程

1. 阅读 PM 分派的任务要求
2. 阅读设计文档（`docs/design/模块设计/模块设计-*.md`）与跨阶段决策（`docs/decisions/`）
3. 阅读参考实现（Python 原版源码，位置速查见 `.opencode/agents/architect.md §Python 参考速查`）
4. 编写 Rust 代码
5. 编写单元测试
6. **过程中阶段性 commit checkpoint**（每完成一个功能模块或通过一组测试后提交，防止中断丢失进度）
7. 执行自检清单（base + 本 extension）
8. 在过程记录中更新开发日志
9. 返回开发报告给 PM

## 项目特定注意事项

- 所有 cargo 命令必须使用 bash 工具的 `workdir="construct-rs"` 参数
- 优先阅读 Python 原版实现来理解行为，但编码风格必须是惯用 Rust
- 不要过度设计，只实现当前子任务要求的功能
- 跨阶段决策不可违反 `docs/decisions/`（ADR-001~ADR-NNN）

## 可写文件

- `construct-rs/src/**`（代码和测试）
- `plans/phaseN/过程记录.md`（开发日志，按 `harness/metadata-convention.md` 格式）
- `experiments/**`
