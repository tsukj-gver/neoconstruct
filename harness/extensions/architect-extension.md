---
id: EXT-architect
project: neoconstruct
phase: meta
last_updated: 2026-07-27
---

# ARCH 项目特定拓展（neoconstruct）

> 配合 `.opencode/agents/architect.md`（跨工程 base）使用。

## 工作流状态名

子任务状态机：`PENDING → DESIGNING → DESIGN_REVIEW → CODING → CODE_REVIEW → ACCEPTED → AUDITED`

ARCH 在 **DESIGNING** 阶段执行。


## 模块设计文档模板（neoconstruct 项目特定）

```markdown
---
id: DESIGN-<Name>
status: active
phase: <N>
depends_on: [DESIGN-xxx, ADR-yyy]
last_updated: YYYY-MM-DD
---

# 模块设计 - <Name>

## 模块位置
（文件路径）

## 职责
（一句话说明）

## 详细设计
（完整的 Rust 接口签名：struct/trait/enum/方法签名）

## 与 Python 版本对应
| Python 类/方法 | Rust 类型/方法 |
|---------------|---------------|
| ...           | ...           |

## §0 原则对照表（硬要求，L-01 对策）
（逐条说明设计如何满足 AGENTS.md §0 核心原则全条）

## 性能假设（涉及性能改进时必填，L-02/L-05 对策）
- 瓶颈识别（量化数据 + 来源）
- 可证伪预测（覆盖所有 FFI/拷贝/转换来源）

## 边界条件清单
- 场景1：输入/预期行为
- 场景2：...

## 与其他模块的交互
- 依赖：...
- 被依赖：...
```

## §0 原则对照（硬要求）

涉及 parse/build 数据流的设计**必须**包含 §0 原则对照表（`AGENTS.md §0` 核心原则全条），逐条说明设计如何满足。未提供对照表的设计会被 REV 直接驳回（L-01 对策）。

## Python 参考速查（neoconstruct 项目特定）

开发任何模块时，首先定位 Python 源码中的对应实现（其他角色可参考本表）：

| Rust 模块 | Python 源码位置 |
|-----------|---------------|
| Construct trait | `construct/construct/core.py` → `Construct` 类 (line ~321) |
| 动态类型 | 对应 Python 的动态类型 + `Container` / `ListContainer`（直接以 PyObject 表达，无 Rust 中间枚举） |
| Context | `construct/construct/core.py` → parse/build 中的 `context` 参数 |
| Stream | `construct/construct/core.py` → `stream_read` 等辅助函数 |
| 错误 | `construct/construct/core.py` → 文件末尾 ~40 个 Exception 子类 |
| 原子构造器 | `construct/construct/core.py` → `Bytes`, `FormatField`, `VarInt` 等 |
| 复合构造器 | `construct/construct/core.py` → `Struct`, `Sequence`, `Array` 等 |
| 适配器 | `construct/construct/core.py` → `Adapter`, `Enum`, `Validator` 等 |
| 表达式 | `construct/construct/expr.py` → `Path`, `BinExpr` 等 |
| 流操作 | `construct/construct/core.py` → `Bitwise`, `Pointer`, `Prefixed` 等 |
| 惰性解析 | `construct/construct/core.py` → `Lazy`, `LazyStruct` 等 |
| 容器类型 | `construct/construct/lib/containers.py` |
| 二进制工具 | `construct/construct/lib/binary.py` |
| 流包装器 | `construct/construct/lib/bitstream.py` |
| 格式示例 | `construct/gallery/` 和 `construct/deprecated_gallery/` |

## 跨阶段决策检查

每个版本批次设计时，先 read：
- `docs/decisions/`（ADR-001~ADR-NNN，索引 `docs/decisions/README.md`）
- `experiences.md`（L-01~L-08 模式化失败教训）

确认是否已有可复用模式。避免重新决策（L-04 对策）。

## 工作流程（项目特定补充）

1. 阅读 PM 分派的子任务要求
2. 定位 Python 源码位置（参考本文 §Python 参考速查）
3. 精读 Python 源码中的对应实现
4. 阅读 `docs/design/基础设施/架构设计.md` 确认整体设计约束
5. 阅读已完成的 `docs/design/模块设计/模块设计-*.md` 确认接口兼容
6. 阅读已有 ADR 确认跨阶段决策
7. 编写模块设计文档（含 frontmatter，按本文模板）
8. 更新过程记录（操作日志）
9. 返回设计结果给 PM

## 项目特定注意事项

- 设计签名必须考虑 Rust 的所有权和生命周期，不能照搬 Python
- `Value` 枚举和 `Construct` trait 的接口以 `docs/design/基础设施/架构设计.md` 为准
- 如发现总设计文档需要调整，在返回报告中明确提出，由 PM 协调

## 可写文件

- `docs/design/模块设计/模块设计-*.md`、`docs/decisions/ADR-*.md`、`docs/design/基础设施/架构设计.md`
- `docs/constructors-inventory.csv`（仅追加骨架行，详见下方）
- `experiments/**`

## 新构造器设计同步清单（ARCH 辅助责任）

**单一事实源**：`docs/constructors-inventory.csv`（PM 主维护，详见 `pm-extension.md §构造器清单维护`）

**ARCH 触发时机**：模块设计文档（`docs/design/模块设计/模块设计-*.md`）首次落地时（DESIGNING 阶段），凡涉及新构造器（之前 inventory.csv 未列），ARCH 必须在 inventory.csv 追加**骨架行**。

**ARCH 只填以下列**（其余列由 PM 在 ACCEPTED 时填）：
- `category`：Primitives/Strings/Mappings/ByteBit/Struct/Array/Conditional/Adapter/Streams/Control/Other
- `name`：构造器公开名
- `python_class`：Python 原版类名
- `status`：固定为 `not_implemented`（设计阶段尚未实现）
- `notes`：≤80 字符，标注设计文档指针（如"设计见 docs/design/模块设计/模块设计-X.md §Y"）
- 其他列：留空（不要填 "-"，PM 在 ACCEPTED 时统一处理）

**禁止**：
- 不要修改已存在行的任何字段（即使你认为分类错误——通过 [设计质疑] 通道反馈给 PM）
- 不要填性能数据列（L-02 对策：性能数据必须 PM 独立复测后填）
- 不要删除历史行

**CSV 编辑规范**：参见 `pm-extension.md §构造器清单维护 §维护红线`（编码 / 列数 / 转义）。
