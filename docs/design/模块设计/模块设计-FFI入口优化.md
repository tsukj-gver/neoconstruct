---
id: DESIGN-phase5-ffi
status: active
phase: "5"
depends_on: [DESIGN-Architecture, ADR-008, ADR-016, ADR-019]
supersedes: []
superseded_by: []
last_updated: 2026-07-29
revision: "v2（5.R 驳回后修正：P0-1 ABI3 spike + P0-2 profiling + P1-6 文档统一）"
---

# 模块设计：FFI 入口优化（Phase 5，A+B+C 组合方案）

> **5.1 v2 修正摘要**（2026-07-29，回应 5.R 驳回的 2 项 P0 + 1 项 P1）：
>
> | 修正项 | 原状态 | 5.1 v2 修正 | 证据 |
> |-------|-------|------------|------|
> | **P1-6** §2.3.2 + §2.6 矛盾 | cls.parse + cls.build 都挂载 | 统一为"仅 parse 去层化"，build 保留 StructMixin.build | §2.3.2 + §2.6 BC-A1 重写 |
> | **P0-1** 候选 B ABI3 | "未验证"一句话 | 4 路径分析 + spike 计划 + B-conservative 详细设计 | §3.2.3.1/.2/.3 + §3.2.5 + `experiments/phase5_abi3_spike/` |
> | **P0-2** 候选 C 收益 | 6-15ns 理论估算 | profiling 实测修正为 0-15ns（下界下调） | §4.3 + §7.1 + `experiments/phase5_candidate_c_profiling.py` |
> | **补充** 候选 A 收益 | 30-50ns 估算 | Controlled A/B Test 实测 ~60ns（高于估算） | §7.1 + `experiments/phase5_candidate_a_ab_test.py` |
> | **B1 中位估算** | 9.88x（临界） | **~10.2x**（因候选 A 实测超预期而上调） | §7.2 |
>
> **新增实验文件**（5.1 v2）：
> - `experiments/phase5_abi3_spike/分析报告-ABI3兼容性.md`（ABI3 4 路径分析）
> - `experiments/phase5_abi3_spike/verify_spike.py`（Python 侧 spike，8/9 PASS）
> - `experiments/phase5_abi3_spike/spike_result.md`（spike 结果）
> - `experiments/phase5_candidate_c_profiling.py` + `_result.md`（候选 C profiling）
> - `experiments/phase5_candidate_a_ab_test.py` + `_ab_result.md`（候选 A 实测）

> **设计依据**：
> - `AGENTS.md §0`（8 条核心原则：一次 FFI / 无中间表示层 / 输入输出侧无抽象 trait / pyo3 核心依赖 / mashumaro API / enum_dispatch 静态分派 / Result+thiserror+path / 纯 Rust Stream）
> - `plans/phase5-struct-ffi/分析报告-FFI入口优化.md`（5.0 分析报告，586 行，含 5 候选 + 推荐方案）
> - `plans/phase5-struct-ffi/总纲.md §PM 决策`（3 项决策：B1 ≥10x 硬目标 / unsafe raw FFI 接受 / 候选 D 后备）
> - `harness/experiences.md §L-01`（中间表示层违反）+ `§L-02`（理论估算替代实证）+ `§L-05`（优化 A 路径忽略 B 路径）+ `§L-09`（跨时段性能对比归因失效）
> - `docs/decisions/ADR-008`（parse 借用实例 `__dict__`）+ `ADR-016`（P0-3 lazy path 错误传播）+ `ADR-019`（错误抛出 fast-path，首次 unsafe raw FFI 验证）
> - 现有源码（精确行号见 §1.2 与各候选节）：
>   - `construct-rs/src/schema.rs`（`_parse_raw` L148-165 / `_build_raw` L180-204）
>   - `construct-rs/src/nodes/struct_node.rs`（StructNode.parse L274-376 / build L378-447）
>   - `construct-rs/src/instance.rs`（`create_class` L72-97，tp_new 22ns 来源）
>   - `construct-rs/src/error.rs`（raw FFI 先例 `try_fast_path_alloc` L740-825）
>   - `construct-rs/src/lib.rs`（pyo3 模块注册 L67-110）
>   - `construct-rs/src/compile.rs`（`compile_schema` L104-241）
>   - `construct-rs/python/construct/_mixin.py`（`StructMixin.parse` classmethod L1051-1070 / 延迟桩 L907-992 / 编译管线 L821-894）
>
> **角色**：ARCH
> **状态**：DESIGNING（待 REV 检视）
> **创建时间**：2026-07-29
> **数据基准**：`docs/perf-scenarios.csv`（B1 parse 3123/386/8.09x，行 2）+ `docs/analysis/phase3-parse-build不对称.md`（bench_asymmetry 实测）

---

## 0. 文档定位与边界

### 0.1 本文档的范围

本文档是 Phase 5 子任务 `5.1 [详细设计]` 的产出，覆盖 5.0 分析报告推荐的 **A+B+C 组合方案**（候选 D 单字段特化作为 5.5 条件触发后备，不在本文档详细设计；候选 E 已在 5.0 报告排除）。

**三个候选各自独立可实施，但组合后预期叠加收益**（详见 §5 集成方案 + §7 性能假设）。

### 0.2 不在本文档范围

- 候选 D（单字段 InlineNode 特化）：高风险高收益后备，5.4 实测后若 StopIf S01/S03 仍 <10x 才启用，届时单独编写设计文档。
- 候选 E（tp_new 替代方案）：5.0 报告 §2.5 已排除（违反 mashumaro API 语义 / §0 第 5 条）。
- 性能实测验证：本文档仅给**可证伪预测**（L-02 教训），实际验证由 5.6 子任务的 Controlled A/B Test 完成（L-09 教训）。

### 0.3 核心约束（PM 决策 + §0 + 教训）

| 约束来源 | 内容 | 本文档对应节 |
|---------|------|-------------|
| PM 决策 1 | B1 硬目标 ≥10x（12x 软目标） | §7 性能假设 |
| PM 决策 2 | 接受 unsafe raw FFI 热路径首次使用（候选 B） | §3 候选 B + §3.3 SAFETY 审查 |
| PM 决策 3 | 候选 D 作为 5.5 条件触发后备 | §0.2 不在范围 |
| §0 全部 8 条 | 一次 FFI / 无中间表示层 / ... | §6 §0 原则对照表 |
| L-01 教训 | 不引入中间表示层 | §6 §0 原则对照表（硬要求） |
| L-02 教训 | 性能假设必须基于实测数据反推 | §7 性能假设 |
| L-05 教训 | 可证伪预测覆盖所有 FFI/拷贝/转换来源 | §7.2 组合预测 + §7.5 来源清单 |
| L-09 教训 | 性能验证用 Controlled A/B Test | §7.3 验证方法 |

---

## 1. 整体方案概览（A+B+C 组合）

### 1.1 三候选攻击面（无重叠，理论可加）

基于 5.0 报告 §1.2.3 的固定开销 281ns 拆解（B1 parse 386ns，固定开销占 72.8%）：

```
[Python 入口]                    [pyo3 wrapper]              [Rust 内部]
cls.parse(data)                  _parse_raw #[pymethods]    StructNode.parse
   │                                │                          │
   ├─ classmethod 解析              ├─ GIL token 校验          ├─ create_class (tp_new 22ns)
   ├─ _construct_compiled 查找      ├─ 参数解析（PyBytes 借用）├─ getattr __dict__ (11.6ns)
   ├─ None 检查                     └─ 返回值包装              ├─ dict.set_item × 3 (13.5ns × 3)
   └─ bound method _parse_raw 分发                             └─ 实例返回
   ▲                                                              ▲
   │                                                              │
候选 A 攻击                                                      候选 C 攻击
（60-90ns，省 30-50ns）                                         （迭代器+分支，省 6-15ns）

                          ▲
                          │
                       候选 B 攻击
                       （30-50ns，省 15-35ns）
```

### 1.2 当前调用链（精确源码行号，B1 parse 场景）

基于 `schema.rs:148-165` + `_mixin.py:1051-1070` + `struct_node.rs:274-376`：

```
[Python 层]                                          ← 来源：_mixin.py:1051-1070
  1. cls.parse(data)                                 ← CPython LOAD_METHOD + CALL（classmethod）
  2. StructMixin.parse 函数体（_mixin.py:1063-1070）：
     ├─ schema = cls._construct_compiled             ← PyType_Lookup（~20-30ns）
     ├─ if schema is None: raise ConstructError(...)  ← 分支判断（~1-2ns）
     └─ return schema._parse_raw(data)                ← bound method 分发（~30-50ns）

[FFI 穿越入 - pyo3 #[pymethods] wrapper]              ← 来源：schema.rs:148-165
  3. pyo3 wrapper（30-50ns，5.0 报告 §1.2.3）：
     ├─ GIL token 校验（py: Python<'py> 参数提取）
     ├─ 参数解析（data: &Bound<'py, PyBytes> 借用校验）
     └─ 返回值包装（PyResult<Bound<'py, PyAny>> → PyObject）

[Rust 内部 - 全部为 C API 调用，非 FFI 穿越]
  4. _parse_raw 函数体（schema.rs:150-165）：
     ├─ data.as_bytes()                               ← ~1ns
     ├─ ParseStream::new(bytes)                       ← ~2ns（切片 + usize）
     ├─ Context::placeholder(py)                      ← ~5ns（无 PyDict 分配）
     ├─ Path::new()                                   ← ~1ns
     └─ self.root.parse(py, &mut stream, &mut ctx, &mut path)

  5. StructNode.parse（struct_node.rs:274-376）：
     ├─ create_class(self.cls.bind(py))               ← tp_new，22ns（bench_asymmetry 组3 实测）
     │                                                  来源：instance.rs:72-97
     ├─ instance.getattr("__dict__")                  ← 10.5ns（组5）
     ├─ downcast::<PyDict>                            ← +1.1ns（组5）
     └─ for field in self.fields（3 次循环，struct_node.rs:343-360）：
         ├─ field.node.parse（FormatField，enum_dispatch 静态分派）
         ├─ match field.mode（分支预测）
         └─ dict_bound.set_item(field.name.py_name(), value)  ← 13.5ns（组5）

[FFI 穿越出 - 返回用户类实例]
  6. 实例 Py<PyAny> 返回 + 引用计数转移              ← ~5ns
```

### 1.3 三候选与现有调用链的对应关系

| 候选 | 攻击的调用链环节 | 当前贡献（5.0 §1.2.3） | **5.1 v2 实测/修正预期节省** | 详细设计节 |
|------|----------------|---------------------|---------------------------|-----------|
| **A（FFI 去层化）** | 步骤 1-2（Python 入口） | 60-90ns | **~60ns**（Controlled A/B 实测，高于估算） | §2 |
| **B（pyo3 raw FFI）** | 步骤 3（pyo3 wrapper） | 30-50ns | **5-35ns**（ABI3 spike 未定：路径 C/D/E） | §3 |
| **C（小字段 fast-path）** | 步骤 5 内部循环 | **0.4-5ns**（profiling 修正，原估 6-15ns） | **0-15ns**（下界下调，LTO=fat 已优化循环） | §4 |
| **A+B+C 叠加** | 步骤 1-3 + 步骤 5 内部 | — | **65-110ns**（A 实测主导 + B/C 叠加） | §5 |

---

## 2. 候选 A 详细设计：FFI 去层化

### 2.1 当前路径（候选 A 攻击点）

来源：`construct-rs/python/construct/_mixin.py:1051-1070`：

```python
class StructMixin:
    @classmethod
    def parse(cls, data):
        schema = cls._construct_compiled       # 属性查找（~20-30ns）
        if schema is None:                     # 分支判断
            raise ConstructError(...)
        return schema._parse_raw(data)         # bound method 分发（~30-50ns）

    def build(self):
        schema = type(self)._construct_compiled
        if schema is None:
            raise ConstructError(...)
        return schema._build_raw(self)
```

每次 `cls.parse(data)` 的 Python 层路径：
1. `classmethod` 描述符协议（LOAD_METHOD + CALL，~10-15ns）
2. `cls._construct_compiled` 类属性查找（PyType_Lookup，~20-30ns）
3. `schema is None` 分支判断（~1-2ns）
4. `schema._parse_raw` bound method 分发（~30-50ns）

**Python 入口合计 60-90ns**（5.0 报告 §1.2.3 反推）。

### 2.2 去层化目标路径

**核心思路**：编译期（`compile_schema` 完成）将 `schema._parse_raw` / `schema._build_raw` 直接 setattr 到用户类（`cls.parse` / `cls.build`），消除运行时的中间属性查找与 bound method 分发。

去层化后的 Python 路径：
1. `cls.parse(data)` → LOAD_ATTR（命中类型 `__dict__`，返回已绑定的 `_parse_raw` 方法对象）
2. CALL（直接调用 bound method，无需再次属性查找）

**消除的开销**：
- classmethod 描述符协议：~10-15ns（保留普通方法调用开销 ~5ns）
- `cls._construct_compiled` 属性查找：~20-30ns
- `schema is None` 分支判断：~1-2ns
- `schema._parse_raw` bound method 分发：~30-50ns

**保留的开销**：
- LOAD_ATTR（~5-10ns，命中类型 dict 缓存）
- CALL（~5-10ns，CPython METHOD CALL 快速路径）

**净节省估算：30-50ns**（5.0 报告 §2 候选 A 预期收益）。

> **5.1 v2 实测修正**：Controlled A/B Test 实测候选 A 净节省 **~60ns**（高于估算上界 50ns）。
> 实测包含全部 Python 入口开销消除（classmethod 描述符 + 属性查找 + None 检查 + bound method 分发）。
> 详见 `experiments/phase5_candidate_a_ab_result.md`。

### 2.3 接口签名

#### 2.3.1 Rust 侧（无变化）

`schema.rs:148-204` 的 `_parse_raw` / `_build_raw` 签名保持不变：

```rust
#[pymethods]
impl CompiledSchema {
    #[pyo3(signature = (data))]
    pub fn _parse_raw<'py>(
        &self,
        py: Python<'py>,
        data: &Bound<'py, PyBytes>,
    ) -> PyResult<Bound<'py, PyAny>> { ... }

    pub fn _build_raw<'py>(
        &self,
        py: Python<'py>,
        obj: &Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyBytes>> { ... }
}
```

> **说明**：候选 A 不改 Rust 侧签名。仅 Python 侧调整 `cls.parse` / `cls.build` 的挂载方式。
> 候选 B 会改 Rust 侧签名（§3），与候选 A 独立。

#### 2.3.2 Python 侧（核心改动）

**改动位置**：`construct-rs/python/construct/_mixin.py:894`（`_compile_schema_for_class` 末尾，`_apply_dataclass_field_config` 之后）。

**当前代码**（`_mixin.py:893-894`）：
```python
    # 编译成功：注入 dataclass 字段配置（在编译后、@dataclass 执行前）
    _apply_dataclass_field_config(cls, descriptors)
```

**去层化后代码**（候选 A 实施位置）：
```python
    # 编译成功：注入 dataclass 字段配置（在编译后、@dataclass 执行前）
    _apply_dataclass_field_config(cls, descriptors)
    # 候选 A（Phase 5.2）：将 schema._parse_raw / _build_raw 直接挂到 cls.parse / build，
    # 消除 StructMixin.parse classmethod 的中间属性查找 + bound method 分发（~30-50ns）。
    _install_fast_call_bindings(cls)
```

**新增函数**（_mixin.py 末尾，`_remove_lazy_stubs` 之后）：

```python
def _install_fast_call_bindings(cls):
    """将 CompiledSchema._parse_raw 挂到 cls.parse（仅 parse 路径去层化）。

    候选 A（Phase 5.2）核心优化：消除 StructMixin.parse 的中间属性查找
    （cls._construct_compiled）+ bound method 分发（schema._parse_raw）。

    挂载方式：将 schema._parse_raw 作为 cls.parse 的直接值（pyo3 bound method）。
    由于 _parse_raw 是 CompiledSchema 实例方法（self = schema），绑定为 cls.parse
    后调用 cls.parse(data) 等价于 schema._parse_raw(data)——self 自动绑定为 schema。

    **仅挂载 parse 路径**（5.1 v2 修正 P1-6）：
    - build 路径保留 StructMixin.build（继承），不挂载 fast binding。
    - 原因：build 是实例方法（self = instance），schema._build_raw 的 self=schema，
      若 setattr cls.build = schema._build_raw，则 instance.build() 调用时
      Python 描述符协议会把 instance 当作 self 传入，但 _build_raw 期望 self=schema，
      obj=instance——self 冲突（详见 §2.6 BC-A1）。
    - build 11.71x 已远超 10x 硬目标，无需优化（L-05 教训——聚焦瓶颈）。

    兼容性：
    - 与延迟桩机制共存：_install_lazy_stubs 会 setattr cls.parse = _lazy_parse
      （前向引用未解析时优先），编译成功后 _remove_lazy_stubs 删除桩，
      再由 _install_fast_call_bindings 挂载 fast binding。
    - StructMixin.parse 作为 fallback 保留（fast binding 缺失时仍可用）。
    - 子类继承：每个 StructMixin 子类在 __init_subclass__ 中独立编译 + 独立挂载，
      不会污染父类。

    实证依据（5.1 v2 spike，experiments/phase5_abi3_spike/verify_spike.py）：
    - pyo3 bound method 作为类属性的行为已验证：cls.parse = schema._parse_raw 后，
      TestStruct.parse(data) 与 instance.parse(data) 都正确返回新实例，
      self 绑定为 schema（非调用方 instance），字段值正确。
    """
    schema = cls._construct_compiled
    if schema is None:
        # 理论不会发生（_compile_schema_for_class 成功路径才会调用此函数）
        return
    # 仅 parse 路径去层化（build 路径保留 StructMixin.build，见函数 docstring）
    cls.parse = schema._parse_raw       # bound method，self 绑定为 schema
    # cls.build 不挂载 fast binding——保留继承的 StructMixin.build（BC-A1 self 冲突）
```

#### 2.3.3 延迟桩路径同步调整

`_mixin.py:907-992` 的 `_install_lazy_stubs` 与 `_remove_lazy_stubs`：

- **`_install_lazy_stubs`**（编译失败，前向引用）：保持当前行为，setattr `cls.parse = _lazy_parse`（classmethod 桩）。
- **`_remove_lazy_stubs`**（延迟编译成功后）：当前仅 `delattr(cls, "parse")`，使 cls 回退到 `StructMixin.parse`。**候选 A 需调整**：删除桩后立即调用 `_install_fast_call_bindings(cls)`，挂载 fast binding。

改动位置：`_mixin.py:995-1004` 的 `_remove_lazy_stubs`：

```python
def _remove_lazy_stubs(cls):
    """删除延迟桩方法，并挂载 fast call binding（候选 A）。"""
    for method_name in ("parse", "build"):
        try:
            delattr(cls, method_name)
        except AttributeError:
            pass
    # 候选 A：删除桩后挂载 fast binding（替代回退到 StructMixin.parse 的慢路径）
    _install_fast_call_bindings(cls)
```

### 2.4 数据流（parse / build）

#### 2.4.1 parse 数据流（去层化后）

```
[用户调用] MyClass.parse(b'\x01\x02\x03')
       │
       ▼
[Python LOAD_ATTR] 命中 MyClass.parse（= schema._parse_raw bound method）
       │
       ▼
[CALL] schema._parse_raw(b'\x01\x02\x03')    ← 单次 FFI 穿越（候选 A 不改变 FFI 次数）
       │
       ▼
[pyo3 wrapper]（候选 B 攻击点，候选 A 不改）
       │
       ▼
[Rust 内部] _parse_raw 函数体 → StructNode.parse → 实例返回
       │
       ▼
[返回] MyClass 实例
```

#### 2.4.2 build 数据流（不变，保留 StructMixin.build）

```
[用户调用] my_instance.build()
       │
       ▼
[Python LOAD_ATTR] 命中 type(my_instance).build（= 继承的 StructMixin.build）
       │  StructMixin.build 是普通实例方法，self=my_instance（正确）
       │
       ▼
[StructMixin.build 函数体]（_mixin.py:1072-1086）：
   ├─ schema = type(self)._construct_compiled
   └─ return schema._build_raw(self)   ← 单次 FFI 穿越
       │
       ▼
[返回] bytes
```

> **注意（5.1 v2）**：build 路径**不**去层化（保留 StructMixin.build）。
> 详见 §2.6 BC-A1。candidate A 仅优化 parse 路径。

### 2.5 影响的源码文件清单

| 文件 | 函数/位置 | 改动类型 | 改动量 |
|------|---------|---------|--------|
| `construct-rs/python/construct/_mixin.py` | `_compile_schema_for_class`（L893-894 之后） | 新增 1 行调用 `_install_fast_call_bindings(cls)` | +1 行 |
| `construct-rs/python/construct/_mixin.py` | `_remove_lazy_stubs`（L995-1004） | 末尾新增 1 行调用 `_install_fast_call_bindings(cls)` | +1 行 |
| `construct-rs/python/construct/_mixin.py` | 文件末尾（L1129 之后） | 新增 `_install_fast_call_bindings` 函数 | +20-25 行 |
| `construct-rs/python/construct/_mixin.py` | `StructMixin.parse`（L1051-1070） | 保留作为 fallback（fast binding 缺失时使用） | 0 行 |
| `construct-rs/python/construct/_mixin.py` | `StructMixin.build`（L1072-1086） | 保留作为 fallback | 0 行 |
| `construct-rs/src/schema.rs` | `_parse_raw` / `_build_raw` | **无变化**（候选 A 不改 Rust） | 0 行 |
| `construct-rs/src/lib.rs` | 模块注册 | **无变化** | 0 行 |
| `construct-rs/src/compile.rs` | `compile_schema` | **无变化**（候选 A 是纯 Python 侧改动） | 0 行 |

**总改动量估算**：~25-30 行（仅 Python 侧）。

### 2.6 候选 A 专属边界条件

#### BC-A1：build 路径 self 冲突 → 决策仅优化 parse 路径（5.1 v2 简化）

**问题**：`schema._build_raw` 是 `CompiledSchema` 的实例方法，签名为 `_build_raw(&self, py, obj)`。
若直接 setattr `cls.build = schema._build_raw`，则 `my_instance.build()` 调用时，Python
描述符协议会把 `my_instance` 作为函数的第一个参数（即 `self`）传入，但 `_build_raw` 期望
`self=schema`（CompiledSchema 实例），`obj=my_instance`——self 错绑，运行时崩溃。

**parse 路径无此问题**：parse 是 classmethod 语义（`cls.parse(data)`，无 instance self）。
pyo3 bound method `schema._parse_raw` 作为类属性后：
- `TestStruct.parse(data)` → 调用 `schema._parse_raw(data)`，self=schema ✓
- `instance.parse(data)` → 同样调用 `schema._parse_raw(data)`，self=schema（非 instance）✓

**parse 路径已实证验证**（5.1 v2 spike，`experiments/phase5_abi3_spike/verify_spike.py`）：
- spike 3.2：`TestStruct.parse(data)` 返回正确实例，字段 (1,2,3) ✓
- spike 3.3：`instance.parse(data)` 返回**新实例**（非 dummy_instance），字段 (1,2,3) ✓
  **关键**：self 未错绑为 instance——pyo3 bound method 的 m_self 在创建时已固定为 schema。

**最终决策（5.1 v2，统一 §2.3.2 与 §2.6）**：
- **仅 parse 路径去层化**（`cls.parse = schema._parse_raw`）
- **build 路径保留 `StructMixin.build`**（继承，不挂载 fast binding）
- **理由**：build 11.71x 已远超 10x 硬目标，无需优化（L-05 教训——聚焦瓶颈 parse B1 8.09x）。

**已废弃的替代方案**（5.1 v2 删除，避免 DEV 困惑）：
- ~~functools.partial 绑定 schema~~：partial 调用有 ~10-15ns Python 字节码开销，抵消收益
- ~~Python 闭包 `_fast_build(self)`~~：同样有 Python 字节码开销
- ~~pyo3 BuildDescriptor pyclass（实现 `__get__`）~~：增加 Rust 复杂度，build 无需优化
- 若 5.6 实测显示 build 也需优化（极不可能），再单独设计 build 去层化方案。

#### BC-A2：子类继承与 fast binding 污染

**问题**：若用户继承 `MyClass(StructMixin)` 后再子类化 `class SubClass(MyClass): pass`，`SubClass` 在 `__init_subclass__` 时会重新编译 + 重新挂载 fast binding，不污染父类。但若用户手动 `SubClass.parse = something`，会覆盖 fast binding——这是用户主动行为，文档需说明。

**处理**：文档说明"覆盖 `cls.parse` / `cls.build` 会失去 fast binding 优化"，无代码层面保护。

#### BC-A3：延迟桩与 fast binding 的时序

**问题**：前向引用场景下，编译流程为：
1. 首次编译失败（前向引用）→ `_install_lazy_stubs` setattr `cls.parse = _lazy_parse`
2. 首次调用 `cls.parse(data)` → `_lazy_parse` → `_retry_compile` → 编译成功 → `_remove_lazy_stubs` → `_install_fast_call_bindings`

时序正确，无冲突。但需确保 `_install_fast_call_bindings` 在 `_remove_lazy_stubs` 之后调用（§2.3.3 已设计）。

#### BC-A4：`StructMixin.parse` 作为 fallback 的语义

**问题**：fast binding 缺失时（如 `_install_fast_call_bindings` 抛异常），`cls.parse` 回退到 `StructMixin.parse` classmethod。需确保回退路径仍能正常工作。

**处理**：`_install_fast_call_bindings` 内部 try/except，失败时静默（log 警告），保留 `StructMixin.parse` 作为 fallback。性能不达预期但不崩溃。

---

## 3. 候选 B 详细设计：pyo3 raw FFI 热路径

> **PM 决策 2**：接受 unsafe raw FFI 热路径首次使用。VET 必须严格审查每处 unsafe 的 SAFETY 注释。

### 3.1 当前实现（pyo3 `#[pymethods]` wrapper）

来源：`construct-rs/src/schema.rs:148-165`：

```rust
#[pymethods]
impl CompiledSchema {
    #[pyo3(signature = (data))]
    pub fn _parse_raw<'py>(
        &self,
        py: Python<'py>,
        data: &Bound<'py, PyBytes>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let bytes = data.as_bytes();
        let mut stream = ParseStream::new(bytes);
        let mut ctx = Context::placeholder(py);
        let mut path = Path::new();
        let result = self.root.parse(py, &mut stream, &mut ctx, &mut path)?;
        Ok(result.into_bound(py))
    }
}
```

pyo3 `#[pymethods]` 宏在 `_parse_raw` 外层生成 wrapper，包含：
1. **GIL token 校验**：从 `Python::with_gil` 或 assume_gil 持有证明
2. **参数解析**：从 `*mut PyObject` args 元组提取 `data`，类型转换为 `&Bound<'py, PyBytes>`（含类型检查 + 借用校验）
3. **self 提取**：从 `*mut PyObject` 提取 `&self`（CompiledSchema 实例引用）
4. **返回值包装**：将 `PyResult<Bound<'py, PyAny>>` 转换为 `*mut PyObject`（含 incref + 错误转换）

**pyo3 wrapper 合计 30-50ns**（5.0 报告 §1.2.3，基于 Context-Vec §3.1 估算）。

### 3.2 raw FFI 实现方案

**核心思路**：绕过 pyo3 `#[pymethods]` 宏生成的 wrapper，直接用 CPython C API 实现 FFI 入口。参考 ADR-019（错误抛出 fast-path）+ `error.rs:740-825`（`try_fast_path_alloc`）的 raw FFI 模式。

#### 3.2.1 实施方案选择

候选 B 有两个实施层次（DEV 根据实际验证选择）：

**方案 B-conservative（保守，推荐先尝试）**：
- 保留 `#[pymethods]` 入口，但内部用 raw C API 替代 pyo3 高级抽象
- 例如：`data.as_bytes()` 替换为 `unsafe { PyBytes_AsStringAndSize(data.as_ptr(), ...) }`
- 收益有限（~5-10ns，仅省 pyo3 内部簿记），但风险低

**方案 B-aggressive（激进，PM 决策 2 已授权）**：
- 将 `_parse_raw` / `_build_raw` 改为模块级 raw C 函数，用 `PyMethodDef` 直接注册
- 完全绕过 pyo3 wrapper（省 15-35ns）
- 需要 unsafe + ABI3 兼容性验证

**本文档设计基于方案 B-aggressive**（PM 决策 2 明确"接受 unsafe raw FFI 热路径首次使用"）。

#### 3.2.2 raw C 函数接口签名（DEV 实现时遵循）

新增模块级 raw C 函数（位置：`schema.rs` 末尾或新建 `schema_ffi.rs`）：

```rust
use pyo3::ffi::{self, c_str};
use std::os::raw::{c_char, c_int};

/// raw FFI parse 入口（候选 B，Phase 5.3）。
///
/// 替代 CompiledSchema::_parse_raw 的 pyo3 #[pymethods] wrapper。
/// 直接用 CPython C API 解析参数 + 调用 Rust 内部逻辑。
///
/// # Python 可见签名
///
/// `_parse_raw_fast(schema, data) -> Any`
///
/// # 参数
///
/// - `self_`：CompiledSchema 实例（self 指针，由 Python 侧调用时自动传入）
/// - `args`：Python args 元组，含 1 个元素（data: bytes）
///
/// # 返回
///
/// 成功返回用户类实例（new reference），失败返回 NULL + 设置 PyErr。
///
/// # SAFETY（VET 审查用，见 §3.3 SAFETY-1）
///
/// [SAFETY 注释由 DEV 实现时填写，本设计给出框架]
pub unsafe extern "C" fn parse_raw_fast(
    self_: *mut ffi::PyObject,
    args: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    // ... DEV 实现时填写
}

/// raw FFI build 入口（候选 B，Phase 5.3）。
pub unsafe extern "C" fn build_raw_fast(
    self_: *mut ffi::PyObject,
    args: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    // ... DEV 实现时填写
}
```

#### 3.2.3 注册到 Python 模块

在 `lib.rs:67-110` 的 `_construct_rust` 模块初始化中，用 `PyMethodDef` 注册 raw C 函数：

```rust
#[pymodule]
fn _construct_rust(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // ... 现有注册逻辑（lib.rs:71-107）保持不变 ...

    // 候选 B（Phase 5.3）：注册 raw FFI 入口
    // 用 PyMethodDef 直接注册，绕过 pyo3 #[pyfunction] wrapper
    let parse_method = ffi::PyMethodDef {
        ml_name: c_str!("_parse_raw_fast").as_ptr(),
        ml_meth: ffi::PyMethodDef__bindgen_ty_1 {
            _unnamed_at_1: Some(schema::parse_raw_fast),
        },
        ml_flags: ffi::METH_VARARGS as c_int,
        ml_doc: c_str!("raw FFI parse entry (Phase 5.3 candidate B)").as_ptr(),
    };
    // ... 注册 parse_method + build_method 到模块 ...
    // 注意：raw C 函数需挂到 CompiledSchema 类型的方法表，而非模块级
    //      （parse_raw_fast 的 self_ 是 CompiledSchema 实例）
}
```

##### 3.2.3.1 ABI3 兼容性 4 路径分析（5.1 v2 补充，P0-1 回应）

> **完整分析**：`experiments/phase5_abi3_spike/分析报告-ABI3兼容性.md`
> **ABI3 是项目硬约束**（来源：`instance.rs:24-37` + `plans/phase1-foundation/过程记录.md:3005`），
> 因开发环境 Python 3.14 vs pyo3 0.22.6（最高支持 3.13），必须设 `PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1`。

raw C 函数注册到 Python 的 4 条路径（ABI3 兼容性矩阵）：

| 路径 | ABI3 兼容 | 与 pyo3 共存 | 自绑定语义 | 实施复杂度 | 净收益 |
|------|----------|-------------|-----------|-----------|-------|
| **A. PyType_FromSpec** | ✅ | ❌ 冲突 | — | — | **不可行** |
| **B. 改 tp_methods 槽** | ❌ opaque | — | — | — | **不可行** |
| **C. PyCFunction_New + SetAttr 类型** | ✅ | ✅ | ⚠️ 需 spike | 中（~50 行 unsafe） | 15-35ns |
| **D. 模块级函数 + functools.partial** | ✅ | ✅ | ✅（partial 绑定） | 低（~30 行） | 5-20ns |
| **E. B-conservative**（#[pymethods] + 内部 raw C API） | ✅ | ✅ | ✅（pyo3 处理） | 极低（~20 行） | 5-10ns |

**路径 A 排除**：`PyType_FromSpec` 创建**新类型**，不修改现有类型。CompiledSchema 已由
pyo3 `#[pyclass]` 宏注册，用 `PyType_FromSpec` 会创建第二个类型对象，冲突。

**路径 B 排除**：ABI3 模式下 `PyTypeObject` 为 opaque，无 API 可动态修改已注册类型的
`tp_methods`。`PyType_GetSlot` 只读，无 `PyType_SetSlot`。

**路径 C（B-aggressive 首选）**：用 `PyCFunction_NewEx(methoddef, schema_ptr, NULL)` 创建
PyCFunction（`m_self` 绑定为 schema_ptr），再 `PyObject_SetAttrString` 挂载到类型对象。
- **ABI3 风险 1 已排除**（Python spike 1.1）：CompiledSchema frozen pyclass 允许 setattr ✓
- **ABI3 风险 2/3 待 Rust spike 验证**：PyCFunction m_self 绑定后的调用语义 + pyclass getattr 拦截

**路径 D（路径 C 失败的回退）**：raw C 函数作为模块级函数 + 候选 A 的 functools.partial 绑定。
完全 ABI3 兼容，但 functools.partial 调用开销 ~10-15ns 抵消候选 B 部分收益。

**路径 E（B-conservative，最保守回退）**：保留 `#[pymethods]` 入口，内部用 `PyBytes_AsStringAndSize`
等 raw C API 替代 pyo3 高级抽象。ABI3 零风险，收益最低（5-10ns）。

##### 3.2.3.2 已验证的 ABI3 稳定 API（项目内先例）

来源：`error.rs:740-825`（`try_fast_path_alloc`）+ `instance.rs:72-143`（`create_class` / `force_setattr`）。

| CPython C API | 项目内先例位置 | 候选 B 用途 |
|---------------|---------------|-----------|
| `PyArg_ParseTuple` | —（候选 B 首次使用） | SAFETY-1 参数解析 |
| `PyBytes_AsStringAndSize` | —（候选 B 首次使用） | SAFETY-3 字节读取 |
| `PyObject_SetAttrString` | `error.rs:774,794,812` | 路径 C 类型 setattr |
| `PyCFunction_NewEx` | —（候选 B 首次使用） | 路径 C 创建 PyCFunction |
| `PyType_GenericAlloc` | `error.rs:761` | 参考（候选 B 不直接用） |
| `PyType_GetSlot` | `instance.rs:78` | 参考（路径 B 排除依据） |
| `Py_DecRef` / `PyErr_Clear` / `PyErr_SetString` / `Py_None` | `error.rs` 多处 | SAFETY-1~5 错误处理 |

**结论**：候选 B SAFETY-1~5 所需的全部 C API 均为 ABI3 稳定 API，且大部分有项目内先例。
**ABI3 风险集中在"类型方法注册"（路径 C/D），而非 unsafe C API 本身**。

##### 3.2.3.3 ABI3 Spike 计划（DEV 实施第一步，强制）

DEV 实施 5.3（候选 B）的第一步必须是 ABI3 spike，验证路径 C 的风险 2/3：

**Spike 代码骨架**（`experiments/phase5_abi3_spike/分析报告-ABI3兼容性.md §5.1`，
~40 行 Rust + Python 验证脚本）：

1. 写 minimal raw C 函数 `_spike_identity(self_, args) -> self_`（仅 incref + 返回 self_）
2. `PyCFunction_NewEx(methoddef, schema_ptr, NULL)` 创建 PyCFunction，m_self=schema_ptr
3. `PyObject_SetAttrString(CompiledSchema_type, "_spike_identity", pyCFunction)`
4. Python 侧验证：`schema._spike_identity()` 返回 schema_ptr（非调用方 instance）

**Spike 结果判定**：

| Spike 结果 | 候选 B 决策 | B1 收益预估 |
|-----------|-----------|-----------|
| 风险 2/3 全部 PASS | 路径 C（B-aggressive）实施 | 15-35ns |
| 风险 2 FAIL（m_self 未绑定） | 回退路径 D 或 B-conservative | 5-20ns / 5-10ns |
| 风险 3 FAIL（pyclass getattr 拦截） | 回退路径 D | 5-20ns |

> **实施难点**（5.1 v2 更新）：原设计的"PyType_FromSpec 重新定义类型"路径已被排除（§3.2.3.1
> 路径 A）。当前推荐路径 C（PyCFunction_New + SetAttr），需 DEV spike 验证风险 2/3。
> 若 spike 失败，回退到路径 D（模块级 + functools.partial）或 B-conservative（§3.2.5）。

#### 3.2.5 B-conservative 详细设计（路径 C spike 失败的回退，5.1 v2 新增）

若 §3.2.3.3 spike 验证路径 C 不可行，候选 B 降级为 B-conservative（路径 E）。

**核心思路**：保留 `#[pymethods]` 入口签名（不改 FFI 注册方式），仅函数体内部用
raw C API 替代 pyo3 高级抽象，省去 pyo3 内部的借用校验 + 参数解析簿记。

**接口签名（无变化）**：`schema.rs:148-165` 的 `_parse_raw` 签名保持不变。

**内部实现改动**（~20 行）：

```rust
#[pyo3(signature = (data))]
pub fn _parse_raw<'py>(
    &self,
    py: Python<'py>,
    data: &Bound<'py, PyBytes>,
) -> PyResult<Bound<'py, PyAny>> {
    // B-conservative：用 raw C API 替代 pyo3 高级抽象
    // 1. PyBytes_AsStringAndSize 替代 data.as_bytes()（省 pyo3 借用校验 ~2-4ns）
    let mut buf: *const u8 = std::ptr::null();
    let mut size: ffi::Py_ssize_t = 0;
    // SAFETY: data 是 Bound<PyBytes>，as_ptr() 返回有效 PyObject 指针。
    // PyBytes_AsStringAndSize 是 ABI3 稳定 API（自 Python 2.5 起）。
    let ok = unsafe {
        ffi::PyBytes_AsStringAndSize(
            data.as_ptr(),
            &mut buf as *mut *const u8 as *mut *mut c_char,
            &mut size,
        )
    };
    if ok == -1 {
        return Err(PyErr::fetch(py));
    }
    // SAFETY: buf 在 data 存活期间有效，size 非负（PyBytes 不可能负长度）。
    let bytes: &[u8] = unsafe { std::slice::from_raw_parts(buf, size as usize) };

    // 2. 其余逻辑不变（仍用 pyo3 的 py: Python<'py> token）
    let mut stream = ParseStream::new(bytes);
    let mut ctx = Context::placeholder(py);
    let mut path = Path::new();
    let result = self.root.parse(py, &mut stream, &mut ctx, &mut path)?;
    Ok(result.into_bound(py))
}
```

**B-conservative 收益重新评估**：

| 优化点 | pyo3 wrapper 原开销 | B-conservative 后 | 净节省 |
|--------|-------------------|------------------|-------|
| PyBytes 借用校验（pyo3 内部） | ~3-5ns | ~1ns（直接 C API） | 2-4ns |
| pyo3 参数解析簿记 | ~5-8ns | ~2ns | 3-6ns |
| pyo3 返回值包装 | ~5-10ns | 不变（仍走 PyResult） | 0ns |
| **合计** | | | **5-10ns** |

**B-conservative 对 B1 加速比影响**：
- B1 rs_ns 基线 386ns，B-conservative 节省 5-10ns → 376-381ns
- B1 加速比 3123/376 = 8.31x ~ 3123/381 = 8.20x（单独不达标，需 A+C 叠加）

**与候选 A 的集成**：B-conservative 不改 FFI 入口签名，候选 A 的
`cls.parse = schema._parse_raw` 挂载方式**无需调整**。

### 3.3 unsafe 块位置清单 + SAFETY 审查清单（VET 审查用）

候选 B 的 raw FFI 入口预计包含以下 unsafe 操作。每处必须有 SAFETY 注释，VET 在 CODE_REVIEW 阶段逐项审查。

#### SAFETY-1：参数解析（PyArg_ParseTuple）

**位置**：`parse_raw_fast` / `build_raw_fast` 入口处。

```rust
// SAFETY: 持有 GIL（由 pyo3 模块调用约定保证——CPython 调用 C 函数时必持 GIL）。
// args 是有效的 *mut PyObject（CPython 调用约定保证——由 args tuple 传入）。
// PyArg_ParseTuple 是 CPython 稳定 ABI（自 Python 3.0 起）。
// 格式字符串 "O" 表示接受任意 PyObject，不做类型检查（类型检查由后续 if let 完成）。
let mut data_obj: *mut ffi::PyObject = std::ptr::null_mut();
let parsed = unsafe {
    ffi::PyArg_ParseTuple(args, c_str!("O").as_ptr(), &mut data_obj)
};
if parsed == 0 {
    // PyArg_ParseTuple 失败时已设置 PyErr，直接返回 NULL
    return std::ptr::null_mut();
}
```

**VET 审查项**：
- [ ] GIL 持有证明（CPython 调用约定 + pyo3 模块注册）
- [ ] args 非空且为 tuple（CPython 调用约定保证）
- [ ] 格式字符串正确（"O" 接受任意对象，后续手工类型检查）
- [ ] data_obj 在 PyArg_ParseTuple 成功后非空

#### SAFETY-2：self 指针解引用（CompiledSchema 实例提取）

**位置**：`parse_raw_fast` 入口处，从 `self_` 提取 `&CompiledSchema`。

```rust
// SAFETY: self_ 来自 Python 侧调用 parse_raw_fast 时的 self 参数，
// 是 CompiledSchema pyclass 实例指针（由 pyo3 #[pyclass] 注册保证布局）。
// pyo3 的 PyClass 实例布局：PyObject header + CompiledSchema 数据。
// PyType_IsSubtype(self_.ob_type, &CompiledSchema_Type) 应为 true。
// 这里用 from_owned_ptr_or_err / from_borrowed_ptr 转换为 Bound<CompiledSchema>。
let schema: Bound<CompiledSchema> = unsafe {
    // 方式 1：用 pyo3 的 from_borrowed_ptr（安全转换，含类型检查）
    match Bound::from_borrowed_ptr_or_opt(py, self_)
        .and_then(|obj| obj.downcast_into::<CompiledSchema>().ok())
    {
        Some(s) => s,
        None => {
            // 类型不匹配，设置 TypeError
            ffi::PyErr_SetString(
                ffi::PyExc_TypeError,
                c_str!("expected CompiledSchema instance").as_ptr(),
            );
            return std::ptr::null_mut();
        }
    }
};
```

**VET 审查项**：
- [ ] self_ 非空（CPython 调用约定保证）
- [ ] self_ 类型确实是 CompiledSchema（downcast 失败时正确设置 TypeError）
- [ ] from_borrowed_ptr 不获取所有权（避免 double free）

#### SAFETY-3：PyBytes 内容读取（PyBytes_AsStringAndSize）

**位置**：从 `data_obj` 提取字节切片。

```rust
// SAFETY: data_obj 是 PyBytes 类型（已在前面类型检查通过）。
// PyBytes_AsStringAndSize 是 CPython 稳定 ABI。
// 返回的 buffer 指针指向 PyBytes 内部缓冲，生命周期与 data_obj 绑定。
// size 为非负数（PyBytes 长度不可能为负）。
let mut buf: *const u8 = std::ptr::null();
let mut size: ffi::Py_ssize_t = 0;
let ok = unsafe {
    ffi::PyBytes_AsStringAndSize(data_obj, &mut buf as *mut *const u8 as *mut *mut c_char, &mut size)
};
if ok == -1 {
    // 失败时 PyErr 已设置
    return std::ptr::null_mut();
}
let bytes: &[u8] = unsafe {
    std::slice::from_raw_parts(buf, size as usize)
};
```

**VET 审查项**：
- [ ] data_obj 已通过类型检查（PyBytes_Check 或 downcast::<PyBytes>）
- [ ] buf 在 data_obj 存活期间有效（不逃逸出函数作用域）
- [ ] size 非负（PyBytes 不可能负长度）
- [ ] bytes 切片在 parse 完成前不逃逸

#### SAFETY-4：错误转换（ConstructError → PyErr）

**位置**：Rust 内部 `Result<T, ConstructError>` 错误传播。

候选 B 不能直接用 `?` 传播错误（`?` 会触发 `From<ConstructError> for PyErr`，但 raw C 函数返回 `*mut PyObject` 而非 `PyResult`）。需手工调用 error.rs 的 `From` 实现：

```rust
// 候选 B：错误转换沿用现有 error.rs:857-896 的 From<ConstructError> for PyErr。
// 该 impl 内部用 Python::with_gil 获取 GIL（raw C 函数已持 GIL，with_gil 是 no-op）。
// 转换后用 PyErr::_restore(py) 设置 Python 异常，返回 NULL。
match result {
    Ok(instance) => instance_obj,  // 返回 new reference
    Err(e) => {
        let pyerr: PyErr = e.into();  // 触发 error.rs:857 的 From impl
        pyerr.restore(py);            // 设置 Python 异常
        std::ptr::null_mut()
    }
}
```

**VET 审查项**：
- [ ] 错误转换语义等价（与 pyo3 `#[pymethods]` 的 `?` 行为一致）
- [ ] PyErr::restore 正确设置异常（不重复设置）
- [ ] 返回 NULL 时 PyErr 已设置（CPython 调用约定）

#### SAFETY-5：返回值构造（new reference）

**位置**：将 parse 结果（用户类实例）转为 `*mut PyObject` 返回。

```rust
// SAFETY: instance 是 Py<PyAny>（owned reference，refcount=1）。
// into_ptr() 消费 Py，返回 *mut PyObject（转移所有权给 Python）。
// Python 侧调用者负责 DECREF（CPython 引用计数约定）。
let instance: Py<PyAny> = result.unwrap();  // 已通过 match 检查
instance.into_ptr()  // 返回 new reference
```

**VET 审查项**：
- [ ] 返回的指针是 new reference（refcount=1，Python 侧负责释放）
- [ ] 不重复 incref（into_ptr 转移所有权，不 incref）

### 3.4 参考 Phase 4.x error.rs raw FFI 先例

候选 B 的 raw FFI 模式参考 ADR-019（错误抛出 fast-path）+ `error.rs:740-825`（`try_fast_path_alloc`）。

#### 3.4.1 先例位置

- **ADR-019**：`docs/decisions/ADR-019-错误抛出fast-path.md`（首次 unsafe raw FFI 在错误路径验证，a_err_eof 11.34x）
- **error.rs L740-825**：`try_fast_path_alloc` 函数，含 4 个 unsafe 块（U1-U4）
- **error.rs L757-824**：完整的 unsafe 块，含 SAFETY 注释模板

#### 3.4.2 可复用模式

| 模式 | error.rs 位置 | 候选 B 复用点 |
|------|--------------|--------------|
| GIL 持有证明 | error.rs:754-756（`py: Python<'py> token 在作用域内`） | SAFETY-1/2/3 |
| 指针非空检查 | error.rs:762-766（`if instance.is_null()` + BC6 回退） | SAFETY-2（self_ 解引用） |
| 错误回退（BC6） | error.rs:763-766（`PyErr_Clear` + 回退慢路径） | SAFETY-1/3（PyArg_ParseTuple 失败） |
| 引用计数追踪 | error.rs:781-817（详细的 incref/decref 注释） | SAFETY-5（返回值构造） |
| c_str! 宏使用 | error.rs:774（`c_str!("message").as_ptr()`） | SAFETY-1（格式字符串） |

#### 3.4.3 关键差异（候选 B vs error.rs 先例）

| 维度 | error.rs O3 fast-path | 候选 B parse_raw_fast |
|------|----------------------|----------------------|
| 触发频率 | 错误路径（罕见） | **热路径**（每次 parse） |
| unsafe 块数量 | 4 个（U1-U4） | 5 个（SAFETY-1~5） |
| 失败回退 | BC6：回退到 `call1` 慢路径 | **无回退**（必须成功，否则 parse 崩溃） |
| 性能要求 | 错误路径不严格要求 | 热路径，每次节省 15-35ns |
| ABI3 兼容性 | 已验证（PyType_GenericAlloc + SetAttrString） | **需验证**（PyMethodDef + 类型方法表） |

**关键风险**：error.rs 的 O3 fast-path 失败时可回退到 `call1` 慢路径（BC6），但候选 B 是热路径入口，无回退路径——若 raw FFI 失败，parse 直接崩溃。**VET 必须确认所有失败模式都被 SAFETY 注释覆盖**。

### 3.5 回退方案（unsoundness 应急）

若候选 B 实施后发现 unsoundness（如 miri 报错 / fuzzing 崩溃 / 内存泄漏），回退方案：

1. **立即回退**：删除 `parse_raw_fast` / `build_raw_fast` 注册，恢复 pyo3 `#[pymethods]` 入口（`schema.rs:148-204` 不变，候选 B 不破坏现有代码）。
2. **性能影响**：回退后 B1 加速比回到候选 A+C 的水平（~9.5-10x），可能不达 ≥10x 硬目标。
3. **触发候选 D**：若回退后 B1 <10x，启用 5.5 单字段特化（候选 D）。

**设计保证**：候选 B 的代码以**新增**为主（新函数 + 新注册），不修改现有 `_parse_raw` / `_build_raw`。回退时仅需注释掉注册代码，现有 pyo3 入口自动恢复工作。

---

## 4. 候选 C 详细设计：小字段 fast-path

### 4.1 触发条件

**编译期检测**：在 `compile.rs:223-229` 创建 `StructNode` 时，新增标志位 `is_small_struct: bool`，判断条件：

```rust
// 触发条件（三个 AND 条件）：
// 1. fields.len() <= 3（小字段，覆盖 B1 3 字段 + StopIf 单字段场景）
// 2. !has_expressions（无表达式字段——表达式路径需要 ctx 注入，fast-path 不支持）
// 3. !has_post_init（无 __post_init__——fast-path 不调用，需走通用路径）
let is_small_struct = fields.len() <= 3
    && !has_expressions
    && !has_post_init;
```

**为什么是 ≤3 字段**：
- B1（3 字段）是 FFI 稀释最严重场景（固定开销占 72.8%，5.0 报告 §1.3）
- B2（10 字段）固定开销占 44.7%，已接近达标（9.66x），无需 fast-path
- 单字段场景（StopIf 容器 Struct）也受益
- 阈值过大（如 ≤10）会增加代码膨胀，收益递减

**为什么排除 has_expressions / has_post_init**：
- has_expressions=true 时，StructNode.parse 需 `ctx.inject_fields(dict)` + `ctx.set_field_at`（struct_node.rs:309-338），fast-path 内联化复杂度高、易出错
- has_post_init=true 时，需在 dict 填充后调用 `instance.__post_init__()`（struct_node.rs:365-372），fast-path 仍需此步骤，节省有限
- **保守策略**：仅对最简单的无表达式、无 post_init 小 Struct 启用 fast-path，降低正确性风险

### 4.2 StructNode 数据结构变更

来源：`construct-rs/src/nodes/struct_node.rs:170-215`（当前 StructNode 定义）。

**新增字段**（StructNode struct）：

```rust
#[derive(Debug)]
pub struct StructNode {
    // ... 现有字段（fields / cls / has_post_init / has_expressions / dict_attr_name）保持不变 ...

    /// 候选 C（Phase 5.4）：小字段 fast-path 标志。
    ///
    /// 编译期计算（compile.rs 中 StructNode::new 调用时传入）。
    /// true → parse/build 入口走专用内联路径（手动展开 1-3 字段）。
    /// false → 走当前通用循环路径。
    ///
    /// 触发条件：fields.len() <= 3 && !has_expressions && !has_post_init。
    is_small_struct: bool,
}
```

**StructNode::new 签名扩展**：

当前签名（struct_node.rs:201-215）：
```rust
pub fn new(
    py: Python<'_>,
    fields: Vec<StructField>,
    cls: Py<PyType>,
    has_post_init: bool,
    has_expressions: bool,
) -> Self
```

候选 C 后签名（新增 `is_small_struct` 参数）：
```rust
pub fn new(
    py: Python<'_>,
    fields: Vec<StructField>,
    cls: Py<PyType>,
    has_post_init: bool,
    has_expressions: bool,
    is_small_struct: bool,  // 候选 C 新增
) -> Self
```

**或**：不扩展 `new` 签名，在 `new` 内部根据 `fields.len() / has_expressions / has_post_init` 自动计算 `is_small_struct`。**推荐此方案**（调用方无需感知 fast-path 标志，逻辑内聚）：

```rust
pub fn new(
    py: Python<'_>,
    fields: Vec<StructField>,
    cls: Py<PyType>,
    has_post_init: bool,
    has_expressions: bool,
) -> Self {
    // 候选 C：自动计算 is_small_struct（无需调用方传入）
    let is_small_struct = fields.len() <= 3
        && !has_expressions
        && !has_post_init;
    Self {
        fields,
        cls,
        has_post_init,
        has_expressions,
        dict_attr_name: intern_pystring(py, "__dict__"),
        is_small_struct,
    }
}
```

> **设计决策**：采用"new 内部自动计算"方案。**理由**：
> 1. 调用方（compile.rs:223-229）无需修改，候选 C 是 StructNode 内部优化
> 2. 触发条件集中管理，避免分散在多处
> 3. `new_for_test`（struct_node.rs:227-243）自动继承，测试用例无需改

### 4.3 专用 parse 路径（fast-path）

来源：`struct_node.rs:274-376`（当前 parse 实现）。

**入口分支**（parse 方法开头）：

```rust
impl Construct for StructNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 候选 C（Phase 5.4）：小字段 fast-path 分支
        if self.is_small_struct {
            return self.parse_small(py, stream, ctx, path);
        }
        // 通用路径（当前实现，struct_node.rs:281-375）
        self.parse_general(py, stream, ctx, path)
    }
}
```

**fast-path 实现**（`parse_small` 方法，候选 C 新增）：

```rust
impl StructNode {
    /// 候选 C：小字段 fast-path（≤3 字段 + 无表达式 + 无 post_init）。
    ///
    /// 与 parse_general 的差异：
    /// 1. 无 Vec 迭代器（直接索引 fields[0] / fields[1] / fields[2]）
    /// 2. match field.mode 分支保留（语义等价，编译器可优化为跳转表）
    /// 3. 字段处理逻辑内联（编译器可跨字段优化）
    ///
    /// 语义等价保证：fast-path 与 parse_general 的输出必须 100% 一致
    /// （Round-trip 测试覆盖，见 §8 边界条件 BC-C2）。
    ///
    /// **5.1 v2 profiling 修正**（experiments/phase5_candidate_c_profiling.py）：
    /// 实测 per-field 边际成本 28.64 ns（1-3 字段平均），其中已知固定项
    /// （Int8ub parse ~10-15ns + PyDict_SetItem 13.5ns）合计 23.5-28.5ns，
    /// 循环结构开销仅 0.14-5.14 ns/field（LTO=fat 下编译器已大幅优化循环）。
    /// 3 字段总可优化空间：0.43-15.43ns（中位 7.93ns）。
    /// **原估算 6-15ns 高估下界**，修正为 0-15ns（详见 §7.1 候选 C 修正）。
    #[inline]
    fn parse_small<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 步骤 1-2：create_class + getattr __dict__（与通用路径相同）
        let instance = create_class(self.cls.bind(py)).map_err(|e| ConstructError::Generic {
            message: format!("failed to create instance via tp_new: {}", e),
            path: path.to_string(),
        })?;
        let dict_bound = instance
            .getattr(self.dict_attr_name.bind(py))
            .map_err(|e| ConstructError::Generic {
                message: format!(
                    "failed to get __dict__ from instance: {}. 该类可能使用了 __slots__。",
                    e
                ),
                path: path.to_string(),
            })?
            .downcast_into::<PyDict>()
            .map_err(|_| ConstructError::Generic {
                message: "instance __dict__ is not a dict (possible __slots__ class)".into(),
                path: path.to_string(),
            })?;

        // 步骤 3：字段处理（推荐实现：循环 + #[inline]，由编译器优化）
        //
        // 5.1 v2 推荐实现方式（回应 REV P1-5）：
        // **采用"循环 + #[inline]"方式**，而非 macro / 显式展开。理由：
        // 1. profiling 证明编译器在 LTO=fat 下已将 `for field in &self.fields`
        //    优化为指针递增（无边界检查），macro/显式展开的边际收益极小
        // 2. 循环方式代码与通用路径几乎一致，语义等价性易保证（BC-C2）
        // 3. macro/显式展开需为 1/2/3 字段分别写代码，复杂度高，收益不确定
        //
        // 关键：每个字段的处理逻辑必须与 struct_node.rs:343-360 的通用路径
        // 完全等价（包括 StopField 捕获 + push_path_segment 错误路径重建）。
        for field in &self.fields {
            let value = match field.node.parse(py, stream, ctx, path) {
                Ok(v) => v,
                Err(ConstructError::StopField { .. }) => break,
                Err(mut e) => {
                    e.push_path_segment(field.name.rust_name());
                    return Err(e);
                }
            };
            match field.mode {
                FieldMode::Rw | FieldMode::Ro => {
                    dict_bound.set_item(field.name.py_name().bind(py), value.bind(py))?;
                }
                FieldMode::Wo => { drop(value); }
            }
        }

        // 步骤 4：fast-path 不调用 __post_init__（is_small_struct 已排除 has_post_init=true）

        // 步骤 5：返回实例
        Ok(instance.unbind())
    }
}
```

> **DEV 实现自由度（5.1 v2 收窄）**：fast-path 采用"循环 + `#[inline]`"方式（上文）。
> 若 DEV 实测后发现 macro / 显式展开确实有 >3ns 额外收益（profiling 证明），可切换实现方式，
> 但必须满足：
> 1. 语义与通用路径 100% 等价（BC-C2）
> 2. StopField 捕获 + 错误路径重建（push_path_segment）行为一致
> 3. 实施后 Controlled A/B Test 验证实际收益 ≥1ns（否则 fast-path 价值存疑）

### 4.4 专用 build 路径（fast-path）

来源：`struct_node.rs:378-447`（当前 build 实现）。

**入口分支**（build 方法开头）：

```rust
fn build(
    &self,
    py: Python<'_>,
    obj: &Bound<'_, PyAny>,
    stream: &mut BuildStream,
    ctx: &mut Context<'_>,
    path: &mut Path,
) -> Result<(), ConstructError> {
    // 候选 C：小字段 fast-path 分支
    if self.is_small_struct {
        return self.build_small(py, obj, stream, ctx, path);
    }
    // 通用路径（当前实现）
    self.build_general(py, obj, stream, ctx, path)
}
```

**fast-path 实现**（`build_small` 方法）：

与 `parse_small` 类似，手动展开 1-3 字段的 build 处理。关键差异：
- has_expressions=false（fast-path 触发条件），所以无需 `ctx.init_expr_values` / `ctx.set_field_at`
- 仅 RW/WO 字段从 `obj.getattr` 取值 + `field.node.build`
- RO 字段调用 `compute_ro_value`（但 has_expressions=false 时 RO 节点通常是 Tell/Computed，无表达式依赖）

> **注意**：build_small 仍需处理 RO 字段（如 Tell/Computed），但无需写 context（has_expressions=false）。语义等价性由 BC-C2 Round-trip 测试保证。

### 4.5 不影响大字段场景的保证（L-05 教训）

**L-05 教训**：优化 A 路径不能让 B 路径回退。候选 C 优化小字段（≤3），必须保证大字段（B2/B3/B4）不回退。

**保证机制**：

1. **入口分支隔离**：`is_small_struct` 标志在编译期计算，运行期仅一次 `if` 判断（~1ns）。大字段场景 `is_small_struct=false`，直接走 `parse_general` / `build_general`（当前通用路径），代码路径完全不变。

2. **通用路径代码保留**：候选 C 不删除 `parse_general` / `build_general`（即当前的 parse/build 实现），仅在其外层加 `if is_small_struct` 分支。大字段场景的执行流与 Phase 4 完全一致。

3. **可证伪预测（阴性对照）**：B2（10 字段）/ B3（50 字段）/ B4（100 字段）的 rs_ns 在候选 C 实施前后应基本不变（<5ns 波动，5.0 报告 §2 候选 C 阳性对照）。

**回归风险**：
- **低风险**：候选 C 是纯 Rust 内部优化，不改 FFI 入口、不改数据流、不改 Node enum。
- **唯一风险**：fast-path 与通用路径的语义不等价（如遗漏 StopField 捕获）。由 BC-C2 Round-trip 测试覆盖。

### 4.6 候选 C 专属边界条件

#### BC-C1：is_small_struct 标志的边界值

| fields.len() | has_expressions | has_post_init | is_small_struct |
|--------------|-----------------|---------------|-----------------|
| 0 | false | false | **true**（空 Struct，fast-path 是 no-op） |
| 1 | false | false | **true**（单字段，StopIf 容器场景） |
| 2 | false | false | **true** |
| 3 | false | false | **true**（B1 场景） |
| 3 | true | false | false（有表达式，走通用路径） |
| 3 | false | true | false（有 post_init，走通用路径） |
| 4 | false | false | false（超阈值，走通用路径） |

**DEV 实现时验证**：上表所有组合的 Round-trip 测试通过。

#### BC-C2：fast-path 与通用路径的语义等价（Round-trip 测试）

**要求**：对所有 is_small_struct=true 的 Struct，fast-path 输出必须与通用路径 100% 一致。

**测试方法**：
1. 构造 ≤3 字段 + 无表达式 + 无 post_init 的 Struct
2. 用 fast-path parse 一段字节 → 得到实例 A
3. 临时禁用 fast-path（如 `is_small_struct=false`）parse 同一段字节 → 得到实例 B
4. 断言 A 与 B 的 `__dict__` 完全相等（key 顺序 + value 类型 + value 内容）
5. 同样验证 build 方向

**覆盖场景**：
- 0 字段（空 Struct）
- 1 字段（单字段）
- 2 字段
- 3 字段（B1 场景）
- 含 WO 字段（padding/reserved）
- 含 RO 字段（Tell，无表达式）
- StopField 捕获（StopIf 在 Struct 字段中触发）
- 错误路径（stream 字节不足 → push_path_segment 重建路径）

#### BC-C3：StopField 捕获行为一致

**问题**：通用路径在 `for field in &self.fields` 循环中捕获 `StopField`（struct_node.rs:321, 347），break 出循环。fast-path 手动展开字段时，也必须在每个字段处理后检查 StopField，及时 break。

**DEV 实现注意**：fast-path 的字段展开必须保留 `Err(ConstructError::StopField { .. }) => break` 分支，不能简化为 `?` 传播。

---

## 5. 集成方案（A+B+C 实施顺序 + 接口一致性 + 回归风险）

### 5.1 实施顺序与依赖关系

基于 5.0 报告 §3.2 推荐排序 + 总纲 §子任务分解：

```
5.R (设计检视) ─┬─→ 5.2 (候选 A: FFI 去层化, 独立) ─┐
                ├─→ 5.3 (候选 B: raw FFI, 独立)    ─┤
                └─→ 5.4 (候选 C: fast-path, 独立)  ─┤
                                                    ↓
                                            5.6 (集成测试 + 性能验证)
                                                    ↓
                                            [决策点 A: B1 ≥10x?]
                                                    ├─ 是 → 收尾
                                                    └─ 否 → 5.5 (候选 D 后备)
```

**实施顺序建议**（基于风险 / 收益 / 依赖）：

| 顺序 | 候选 | 理由 |
|------|------|------|
| 1 | **C（fast-path）** | 低风险打底。即使 A/B 后续不达预期，C 仍可独立交付（B1 至少 8.2-8.4x）。局部改动，不影响 FFI 入口。 |
| 2 | **A（FFI 去层化）** | 高收益主战场（30-50ns）。纯 Python 侧改动，无 unsafe 风险。与 C 独立（攻击不同环节）。 |
| 3 | **B（raw FFI）** | 叠加冲击 ≥10x（15-35ns）。unsafe 风险最高，最后实施（若 A+C 已达标可跳过）。 |

**并行可能性**：
- 5.2 / 5.3 / 5.4 攻击不同环节（Python 入口 / pyo3 wrapper / Rust 内部），**理论可并行**
- 但 5.2（候选 A）改 `_mixin.py` + 5.3（候选 B）改 `schema.rs` / `lib.rs` + 5.4（候选 C）改 `struct_node.rs` / `compile.rs`，**无源码冲突**
- **建议顺序实施**（C → A → B），便于逐步验证收益 + 定位回归

### 5.2 接口一致性（三候选交互点）

三候选攻击不同环节，但存在以下交互点需保持一致：

#### 交互点 1：FFI 入口签名

| 候选 | 改 FFI 入口？ | 影响 |
|------|-------------|------|
| A | 否（Python 侧改挂载方式） | `_parse_raw` / `_build_raw` Rust 签名不变 |
| B | **是**（新增 raw C 函数） | 新增 `parse_raw_fast` / `build_raw_fast`，与现有 `_parse_raw` / `_build_raw` 并存 |
| C | 否（Rust 内部 StructNode 优化） | FFI 入口完全不变 |

**一致性保证**：
- 候选 A 挂载 `schema._parse_raw`（pyo3 方法）到 `cls.parse`
- 候选 B 实施后，候选 A 可改为挂载 `parse_raw_fast`（raw C 函数）到 `cls.parse`
- 候选 C 对 FFI 入口透明（仅 StructNode 内部分支）

**DEV 协调**：候选 A 与候选 B 若同期实施，需统一挂载目标（pyo3 方法 vs raw C 函数）。

#### 交互点 2：CompiledSchema 实例引用

- 候选 A：`cls.parse = schema._parse_raw`（schema 是 CompiledSchema 实例）
- 候选 B：`parse_raw_fast(self_, args)` 中 self_ 是 CompiledSchema 实例指针
- 候选 C：不涉及 CompiledSchema（StructNode 内部优化）

**一致性**：三候选都依赖 `CompiledSchema` 实例的正确绑定，无冲突。

#### 交互点 3：错误处理路径

- 候选 A：错误由 `_parse_raw` 内部 `?` 触发 `From<ConstructError> for PyErr`（error.rs:857）
- 候选 B：错误由 raw C 函数手工调用 `From<ConstructError> for PyErr` + `PyErr::restore`（SAFETY-4）
- 候选 C：错误由 StructNode.parse/build 内部 `?` 传播，路径不变

**一致性**：三候选的错误转换语义等价（都通过 error.rs:857 的 From impl），用户侧看到的 Python 异常类型 + 消息一致。

### 5.3 回归风险分析（L-05 教训执行）

> **L-05 教训**：可证伪预测必须覆盖**所有**瓶颈来源，不仅被优化的路径。

#### 5.3.1 三候选攻击的环节清单（全覆盖）

| 调用链环节 | 攻击候选 | 当前贡献 | 优化后预期 |
|-----------|---------|---------|-----------|
| Python 入口（classmethod + 属性查找 + bound method） | **A** | 60-90ns | 30-40ns |
| pyo3 wrapper（GIL + 参数解析 + 返回包装） | **B** | 30-50ns | 10-20ns |
| StructNode 固有（tp_new + getattr_dict + dict 初始化） | —（不可优化） | 34-40ns | 34-40ns |
| StructNode 字段循环（迭代器 + 分支预测） | **C** | **0.4-5ns**（5.1 v2 profiling：3 字段实测，LTO=fat 下编译器已大幅优化） | 0-3ns |
| 实例返回 + 引用计数 | —（不可优化） | 5ns | 5ns |
| 未解释残差（cache miss / 分支预测 / pyo3 内部） | —（需 profiling） | 60-95ns | 60-95ns |

**L-05 检查**：本设计覆盖了**所有可优化的 FFI/拷贝/转换来源**（A/B/C），未优化的来源（StructNode 固有 / 实例返回 / 残差）已在表中标注，无遗漏。

#### 5.3.2 已 ≥10x 场景的回归风险

| 场景 | 当前加速比 | 候选影响 | 回归风险 |
|------|-----------|---------|---------|
| B2（10 字段）parse | 9.66x | A+B 减固定开销（~50ns），C 不触发（>3 字段） | **低**（9.66x → ~10.5x，达标） |
| B3（50 字段）parse | 11.54x ✅ | A+B 减固定开销，字段开销主导 | **极低**（固定开销占比 13.9%，优化后 ~11.8x） |
| B4（100 字段）parse | 12.06x ✅ | 同 B3 | **极低**（固定开销占比 7.7%，优化后 ~12.2x） |
| B1-B4 build | 11.71-17.4x ✅ | 候选 A build 路径保守保留（BC-A1），C 不触发 build fast-path | **零**（build 路径不变） |
| Phase 4 Array/GreedyRange/etc. | ≥10x ✅ | A 减固定开销（每次 parse/build 受益），C 不触发（非 StructNode） | **极低** |
| Phase 4 StopIf S01/S03（单字段） | 9.96x | A+B+C 全部受益（单字段是 fast-path 触发场景） | **负**（加速比提升） |

**结论**：三候选对已 ≥10x 场景的回归风险**极低**。理论上所有场景都会因为固定开销减少而**轻微提升**或**持平**，不会回退。

#### 5.3.3 阴性对照设计（Controlled A/B Test 必备）

5.6 集成测试时，必须设计阴性对照证明环境漂移幅度（L-09 教训）：

- **阳性对照**：B1（3 字段）parse，预期加速比显著提升（8.09x → ~10x）
- **阴性对照 1**：B4（100 字段）parse，预期加速比基本不变（12.06x → ~12.2x，<0.3x 波动）
- **阴性对照 2**：B7（空 Struct）parse，预期固定开销减少带来轻微提升（与 B1 同方向但幅度小）

---

## 6. §0 原则对照表（L-01 教训硬要求）

> **L-01 教训对策**：涉及 parse/build 数据流的设计**必须**包含 §0 原则对照表。REV 检视时未提供对照表直接驳回。

对照 `AGENTS.md §0` 全部 8 条核心原则：

| §0 原则 | 候选 A（FFI 去层化） | 候选 B（raw FFI） | 候选 C（fast-path） | 违反风险 |
|---------|---------------------|------------------|--------------------|---------|
| **1. 一次 FFI**（编译/parse/build 各只有一次 Python↔Rust 边界穿越） | ✅ 仅优化 Python 侧到 FFI 入口的路径，FFI 次数不变（仍是 1 次 `_parse_raw` 调用） | ✅ 仅改变 FFI 入口实现方式（raw C 函数 vs pyo3 wrapper），FFI 次数不变 | ✅ 纯 Rust 内部优化，FFI 次数不变 | 零 |
| **2. 无中间表示层**（parse 直接构造 PyObject，build 直接读 PyObject） | ✅ 不引入 Rust 中间类型（仅改 Python 侧挂载方式） | ✅ 不引入 Rust 中间类型（raw C 函数直接操作 PyObject 指针） | ✅ 不引入 Rust 中间类型（StructNode 内部分支） | 零 |
| **3. 输入/输出侧无抽象 trait** | ✅ 无 trait 抽象 | ✅ 无 trait 抽象（raw C 函数非 trait） | ✅ 无 trait 抽象 | 零 |
| **4. pyo3 是核心依赖** | ✅ 仍用 pyo3（仅调整 Python 侧挂载，Rust 侧 pyo3 不变） | ⚠️ **边缘**（raw FFI 绕过 `#[pymethods]` 宏，但 GIL token 仍由 pyo3 提供；符合 §0 第 4 条精神——pyo3 仍是核心依赖，仅热路径绕过宏生成的 wrapper） | ✅ pyo3 不变 | 候选 B 边缘（符合精神，但形式上绕过宏） |
| **5. mashumaro 式 API**（`@dataclass class X(StructMixin)`，编译在 `__init_subclass__`） | ✅ 用户 API 不变（`cls.parse(data)` 签名一致） | ✅ 用户 API 不变 | ✅ 用户 API 不变 | 零 |
| **6. 构造器分派**（enum_dispatch 静态分派） | ✅ Node enum 不变 | ✅ Node enum 不变 | ✅ Node enum 不变（仅 StructNode 内部分支） | 零 |
| **7. 错误处理**（Result + thiserror + path） | ✅ ConstructError 不变（错误仍由 `_parse_raw` 内部 `?` 触发 error.rs:857 From impl） | ⚠️ raw C 函数手工调用 `From<ConstructError> for PyErr`（SAFETY-4），**语义等价**但形式不同（无 `?` 语法糖） | ✅ 错误处理不变（StructNode 内部 `?` 传播） | 候选 B 边缘（语义等价，需 VET 确认） |
| **8. Stream 抽象**（纯 Rust 内部，不跨 FFI） | ✅ Stream 不变（ParseStream/BuildStream 内部使用） | ✅ Stream 不变 | ✅ Stream 不变 | 零 |

### §0 违反风险总结

- **候选 A / C**：**零 §0 风险**（纯优化，不触及任何原则）
- **候选 B**：**两处边缘**（§0 第 4 / 第 7 条），但符合原则精神：
  - 第 4 条：pyo3 仍是核心依赖（GIL token 由 pyo3 提供），仅热路径绕过 `#[pymethods]` 宏。类比 ADR-019 已在错误路径用 raw C API（error.rs:740-825），项目已接受此模式。
  - 第 7 条：错误转换语义等价（SAFETY-4 沿用 error.rs:857 的 From impl），仅形式上无 `?` 语法糖。

**结论**：A+B+C 三候选**不违反**任何 §0 核心原则。候选 B 的两处边缘符合原则精神，且 PM 决策 2 已明确接受 unsafe raw FFI 热路径首次使用。

---

## 7. 性能假设（L-02 / L-05 / L-09 教训）

> **L-02 教训**：性能假设必须基于实测数据反推，非理论估算。
> **L-05 教训**：可证伪预测必须覆盖所有 FFI/拷贝/转换来源。
> **L-09 教训**：性能验证用 Controlled A/B Test，ns 级估算标"量级参考"。

### 7.1 单候选收益预测（基于 5.0 报告 §2 实测反推 + 5.1 v2 profiling 修正）

> **5.1 v2 修正（P0-2 回应）**：候选 A/C 已用 Controlled A/B Test + micro-benchmark 实测验证，
> 候选 B 待 ABI3 spike。详见 `experiments/phase5_candidate_a_ab_result.md` +
> `experiments/phase5_candidate_c_profiling_result.md`。

| 候选 | 攻击环节 | 原估算（5.0 报告） | **5.1 v2 实测/修正** | B1 rs_ns 变化 | B1 加速比变化 |
|------|---------|-------------------|---------------------|--------------|--------------|
| A（FFI 去层化） | Python 入口 | 30-50ns | **~60ns**（Controlled A/B 实测，**高于估算**） | 386 → 326 | 8.09x → **~9.6x** |
| B（pyo3 raw FFI） | pyo3 wrapper | 15-35ns | **5-35ns**（ABI3 spike 未定：路径 C=15-35ns / 路径 D=5-20ns / B-conservative=5-10ns） | 386 → 351-381 | 8.09x → **8.2-8.9x** |
| C（fast-path） | 字段循环 | 6-15ns | **0-15ns**（profiling 反推：循环结构开销 0.14-5.14ns/field，3 字段 0.4-15.4ns） | 386 → 371-386 | 8.09x → **8.1-8.4x** |

**单候选结论（5.1 v2）**：
- **候选 A 是绝对主力**（实测 ~60ns，单独可能使 B1 接近或达到 ≥10x）
- 候选 B/C 单独贡献有限，作为叠加优化

### 7.2 组合后收益预测（A+B+C 叠加，5.1 v2 修正）

基于 5.0 报告 §3.1 的叠加模型 + 5.1 v2 实测修正：

| 组合 | 理论叠加节省 | **5.1 v2 修正**（实际叠加） | B1 rs_ns | B1 加速比 |
|------|------------|---------------------------|---------|-----------|
| A 单独（实测） | — | **~60ns** | ~326 | **~9.6x** |
| A + B（路径 C） | 75-95ns | 60+15~35 = 75-95ns | 291-311 | **10.0-10.7x** |
| A + B（路径 D） | 65-80ns | 60+5~20 = 65-80ns | 306-321 | **9.7-10.2x** |
| A + B（B-conservative） | 65-70ns | 60+5~10 = 65-70ns | 316-321 | **9.7-9.9x** |
| A + C | 60-75ns | 60+0~15 = 60-75ns | 311-326 | **9.6-10.1x** |
| **A + B + C（路径 C，最佳）** | 75-110ns | 60+15~35+0~15 = 75-110ns | **276-311** | **10.0-11.3x** |
| **A + B + C（路径 D，中位）** | 65-95ns | 60+5~20+0~15 = 65-95ns | **291-321** | **9.7-10.7x** |
| **A + B + C（B-conservative，保守）** | 65-85ns | 60+5~10+0~15 = 65-85ns | **301-321** | **9.7-10.4x** |

**组合后中位估算（5.1 v2 修正）**：
- **候选 A 已实测 ~60ns**（高置信度，Controlled A/B Test）
- 候选 B+C 中位叠加：~15-25ns（B 路径 D 中位 12ns + C 中位 8ns）
- B1 rs_ns 中位：386 - 60 - 20 = ~306ns
- B1 加速比中位：3123 / 306 = **~10.2x**（5.0 报告原中位 9.88x，**因候选 A 实测超预期而上调**）

**临界性声明（5.1 v2 修正）**：
- **B1 ≥10x 在中位估算下趋于达标**（~10.2x，较原 9.88x 乐观）
- **关键变量是候选 A**（已实测 ~60ns，置信度高）
- 候选 B/C 即使收益落在区间下界（B-conservative + C=0ns），A+B-conservative 仍达 ~9.7x（接近 10x）
- **若候选 A 实测在全项目（非仅 micro-benchmark）稳定 ~60ns，B1 ≥10x 概率显著提升**
- 仍需 5.6 Controlled A/B Test 全场景验证（L-09 教训），不可用 micro-benchmark 替代集成测试

### 7.3 验证方法（Controlled A/B Test，L-09 教训）

5.6 集成测试必须采用 Controlled A/B Test，**禁止跨时段单次对比**。

#### 7.3.1 测试协议

```
1. 同会话（同一 Python 进程，同一 venv，同一时段）
2. 交替测量 A/B（A=优化前 git commit，B=优化后 git commit）：
   - A → B → A → B → A → B（至少 3 轮）
   - 每轮每场景采样 ≥5 次
3. 阳性对照：B1（3 字段）parse，预期显著提升
4. 阴性对照 1：B4（100 字段）parse，预期 <0.3x 波动
5. 阴性对照 2：B7（空 Struct）parse，预期轻微提升（固定开销减少）
6. 统计判据：
   - 差异 >0.5x → 显著（回归或提升）
   - 差异 0.3-0.5x → 加测
   - 差异 <0.3x → 波动（环境漂移，L-09 教训）
```

#### 7.3.2 DLL hash 验证（L-09 教训）

每次 A/B 切换后，验证 cargo 重编译产生不同 DLL（hash 不同），避免"消除法归因"陷阱（5.0 报告引用 `docs/analysis/E01-O1-regression-investigation.md`）。

### 7.4 波动阈值（L-09 教训）

- **回归判定**：加速比下降 ≥0.5x → 回归（需排查原因）
- **波动判定**：加速比变化 <0.3x → 环境漂移（不归因于代码改动）
- **中间区域**（0.3-0.5x）：加测 ≥3 轮，取统计区间

### 7.5 FFI/拷贝/转换来源清单（L-05 教训硬要求）

> REV 检视时，本清单是必查项——确认每个来源都在 §7.1/§7.2 预测中有对应声明。

| 来源 | 类别 | 当前贡献 | 候选 | 优化后预期 | 预测位置 |
|------|------|---------|------|-----------|---------|
| classmethod 描述符协议 | Python 入口 | 10-15ns | A | ~5ns（普通方法调用） | §7.1 候选 A |
| `cls._construct_compiled` 属性查找 | Python 入口 | 20-30ns | A | 0ns（消除） | §7.1 候选 A |
| `schema is None` 分支 | Python 入口 | 1-2ns | A | 0ns（消除） | §7.1 候选 A |
| `schema._parse_raw` bound method 分发 | Python 入口 | 30-50ns | A | 0ns（直接挂载） | §7.1 候选 A |
| pyo3 GIL token 校验 | pyo3 wrapper | 5-10ns | B | 0-5ns（raw FFI 仍需 GIL，但无校验） | §7.1 候选 B |
| pyo3 参数解析（PyBytes 借用） | pyo3 wrapper | 10-20ns | B | 5-10ns（PyArg_ParseTuple + 手工类型检查） | §7.1 候选 B |
| pyo3 返回值包装 | pyo3 wrapper | 10-15ns | B | 0-5ns（直接返回 *mut PyObject） | §7.1 候选 B |
| StructNode tp_new | Rust 内部 | 22ns | —（不可优化） | 22ns | §5.3.1 |
| StructNode getattr_dict | Rust 内部 | 11.6ns | —（不可优化） | 11.6ns | §5.3.1 |
| StructNode dict.set_item × 3 | Rust 内部 | 40.5ns（3 × 13.5） | —（不可优化，PyDict_SetItem 固有） | 40.5ns | §5.3.1 |
| Vec 迭代器（3 字段） | Rust 内部 | **0.4-1.5ns**（5.1 v2 profiling：LTO=fat 下编译器已优化，近乎零边界检查） | C | 0-0.5ns（手动展开） | §7.1 候选 C |
| match field.mode 分支预测 | Rust 内部 | **0-3.6ns**（5.1 v2 profiling：3 变体编译为短分支链，命中率极高） | C | 0-1ns（编译器优化跳转表） | §7.1 候选 C |
| 实例返回 + 引用计数 | FFI 出口 | 5ns | —（不可优化） | 5ns | §5.3.1 |
| 未解释残差 | 多种 | 60-95ns | —（需 profiling） | 60-95ns | §5.3.1 |

**L-05 检查结论**：本设计覆盖了**所有** FFI/拷贝/转换来源，每个来源都在预测中有对应声明。无遗漏。

---

## 8. 边界条件清单

> 汇总三候选的所有边界条件（候选专属边界条件已分别在 §2.6 / §4.6 详述，本节为全局视图）。

### 8.1 输入边界条件

| 场景 | 当前行为 | 候选 A/B/C 影响 | 处理方式 |
|------|---------|---------------|---------|
| **空 Struct（0 字段）** | parse(b'') → 空实例；build() → b'' | A：fast binding 正常挂载；B：raw FFI 入口正常；C：is_small_struct=true，fast-path 是 no-op（0 字段循环） | Round-trip 测试覆盖（struct_node.rs:510-537 已有） |
| **单字段 Struct** | 正常 parse/build | A+B+C 全部受益；C 的 fast-path 触发（fields.len()==1 ≤3） | Round-trip 测试覆盖（struct_node.rs:580-647） |
| **B1 场景（3 字段）** | 8.09x | A+B+C 叠加主战场，预期 9.2-10.6x | 5.6 Controlled A/B Test 阳性对照 |
| **大字段 Struct（10/50/100）** | 9.66-12.06x | A+B 减固定开销（轻微提升）；C 不触发（>3 字段走通用路径） | 5.6 阴性对照，确保不回退 |
| **嵌套 Struct（Struct 字段是另一 StructMixin）** | 递归 parse | A：内层 Struct 的 fast binding 独立挂载；B：内层走 StructRefNode → resolve_schema；C：内层 StructNode 独立判断 is_small_struct | Round-trip 测试覆盖（struct_node.rs:775-880） |
| **frozen dataclass** | R4 借用 __dict__ 绕过 FrozenInstanceError | A+B+C 不改 __dict__ 操作路径，frozen 兼容性保留 | Round-trip 测试覆盖（struct_node.rs:1187-1233） |
| **slots dataclass** | 编译期报错（compile.rs:170-183） + parse 兜底报错（struct_node.rs:296-306） | A+B+C 不改 slots 检测路径 | 现有测试覆盖 |

### 8.2 错误路径边界条件

| 场景 | 当前行为 | 候选 A/B/C 影响 | 处理方式 |
|------|---------|---------------|---------|
| **字节不足（stream error）** | push_path_segment 重建路径（struct_node.rs:323-325, 348-350） | C：fast-path 必须保留 push_path_segment（BC-C3）；A+B：错误转换路径不变 | Round-trip 错误测试（struct_node.rs:886-953） |
| **StopField 捕获** | break 出字段循环（struct_node.rs:321, 347） | C：fast-path 必须保留 StopField 捕获（BC-C3） | 现有测试 + BC-C2 Round-trip |
| **WO 字段（padding/reserved）** | drop(value)，不写入 dict（struct_node.rs:357-359） | C：fast-path 必须保留 WO 分支 | BC-C2 Round-trip |
| **RO 字段（Tell/Computed，无表达式）** | compute_ro_value 计算（struct_node.rs:422-443） | C：fast-path 的 build_small 仍需处理 RO 字段（但 has_expressions=false 时简化） | BC-C2 Round-trip |
| **`_construct_compiled` 为 None（延迟桩）** | StructMixin.parse raise ConstructError（_mixin.py:1064-1068） | A：fast binding 缺失时回退到 StructMixin.parse（BC-A4）；延迟桩路径保留 | 延迟桩测试（_mixin.py:907-992） |
| **前向引用未解析** | _install_lazy_stubs 挂桩 → 首次调用重试编译（_mixin.py:907-967） | A：_remove_lazy_stubs 后调用 _install_fast_call_bindings（BC-A3） | 延迟桩 Round-trip 测试 |
| **raw FFI 参数类型错误（候选 B）** | pyo3 wrapper 自动 TypeError | B：raw C 函数手工 TypeError（SAFETY-1/2） | BC-B1（见下） |

### 8.3 候选 B 专属边界条件

#### BC-B1：raw FFI 参数类型校验

**问题**：pyo3 `#[pymethods]` wrapper 自动校验参数类型（如 `data: &Bound<PyBytes>` 不是 bytes 时自动 TypeError）。raw C 函数需手工校验。

**处理**（SAFETY-1 之后）：
```rust
// PyArg_ParseTuple 用 "O" 接受任意对象，后续手工类型检查
if unsafe { ffi::PyBytes_Check(data_obj) } == 0 {
    ffi::PyErr_SetString(
        ffi::PyExc_TypeError,
        c_str!("expected bytes").as_ptr(),
    );
    return std::ptr::null_mut();
}
```

**VET 审查**：所有 pyo3 自动校验的类型，raw C 函数必须手工等价校验。

#### BC-B2：raw FFI 失败时的 PyErr 清理

**问题**：raw C 函数中多个 C API 调用可能失败，每次失败后 PyErr 已设置。若后续代码继续执行（如清理资源），可能污染异常状态。

**处理**（参考 error.rs:763-766 BC6 模式）：
- 每个 C API 调用后立即检查返回值
- 失败时 `ffi::PyErr_Clear()`（若需清理资源）+ 释放已分配内存 + 返回 NULL
- 不在 PyErr 已设置后继续调用可能失败的 C API

#### BC-B3：ABI3 兼容性回退（5.1 v2 更新）

**问题**：候选 B 的 raw C 函数注册到类型方法表，ABI3（Py_LIMITED_API）下需选择可行路径。

**5.1 v2 修正**：完整路径分析见 §3.2.3.1（4 路径可行性矩阵）。
- 路径 A（PyType_FromSpec）：❌ 排除（与 pyo3 冲突）
- 路径 B（改 tp_methods）：❌ 排除（ABI3 opaque）
- **路径 C（PyCFunction_New + SetAttr）**：⚠️ 理论可行，需 spike 验证风险 2/3
- **路径 D（模块级 + functools.partial）**：✅ 完全可行，收益 5-20ns
- **路径 E（B-conservative）**：✅ 完全可行，收益 5-10ns（§3.2.5 详细设计）

**处理（5.1 v2）**：
- DEV 实施第一步是 ABI3 spike（§3.2.3.3），Python 侧已验证风险 1（类型 setattr 可行）
- Spike 结果决定路径 C/D/E：
  - PASS → 路径 C（15-35ns）
  - FAIL → 路径 D（5-20ns）或 B-conservative（5-10ns）
- **不允许**为了候选 B 而禁用 ABI3（`PYO3_USE_ABI3_FORWARD_COMPATIBILITY=0`），
  这会破坏项目的 Python 版本兼容性（开发环境 Python 3.14 vs pyo3 0.22.6 硬约束）

---

## 9. 与现有架构的集成

### 9.1 Node enum 影响

**结论**：A+B+C 三候选**均不修改 Node enum**。

| 候选 | 改 Node enum？ | 理由 |
|------|--------------|------|
| A | 否 | Python 侧改动，不触及 Rust 执行树 |
| B | 否 | 仅改 FFI 入口实现方式，不改执行树 |
| C | 否 | 仅 StructNode 内部加 `is_small_struct` 标志 + 分支，不改 Node 变体 |

**对比候选 D**：候选 D（5.5 后备）会新增 `Node::InlineSingleField` 变体，影响 enum_dispatch + 所有 match 臂 + 嵌套场景——这是候选 D 高风险的根源，也是 PM 决策 3 将其作为后备的理由。

### 9.2 compile.rs 影响

**结论**：A+B+C 三候选**均不实质修改 compile.rs**。

| 候选 | 改 compile.rs？ | 理由 |
|------|----------------|------|
| A | 否 | Python 侧 `_install_fast_call_bindings` 在 `_compile_schema_for_class`（_mixin.py）末尾调用，不触及 Rust compile_schema |
| B | 否 | 候选 B 改 schema.rs / lib.rs，不改 compile.rs |
| C | **可能微调** | 若采用"StructNode::new 内部自动计算 is_small_struct"方案（§4.2 推荐），compile.rs:223-229 的 `StructNode::new` 调用**无需修改**；若采用"调用方传入"方案，compile.rs 需多传一个参数 |

**推荐**：采用"new 内部自动计算"方案，compile.rs 零改动。

### 9.3 Python 侧 _mixin.py 影响

**结论**：仅候选 A 改 _mixin.py。

| 候选 | 改 _mixin.py？ | 改动位置 |
|------|---------------|---------|
| A | **是** | `_compile_schema_for_class`（L893-894 后 +1 行）+ `_remove_lazy_stubs`（L995-1004 末尾 +1 行）+ 新增 `_install_fast_call_bindings` 函数（+20-25 行） |
| B | 否 | 候选 B 是 Rust 侧改动 |
| C | 否 | 候选 C 是 Rust 侧改动 |

**_mixin.py 改动总计**：~25-30 行（仅候选 A）。

### 9.4 schema.rs 影响

| 候选 | 改 schema.rs？ | 改动位置 |
|------|---------------|---------|
| A | 否 | 候选 A 不改 Rust |
| B | **是**（新增） | 新增 `parse_raw_fast` / `build_raw_fast` raw C 函数（+80-120 行，含 SAFETY 注释） |
| C | 否 | 候选 C 不改 FFI 入口 |

### 9.5 struct_node.rs 影响

| 候选 | 改 struct_node.rs？ | 改动位置 |
|------|-------------------|---------|
| A | 否 | 候选 A 不改 Rust 内部 |
| B | 否 | 候选 B 不改 StructNode |
| C | **是** | StructNode 新增 `is_small_struct` 字段（+1 字段）+ parse/build 入口分支（+2 行 if）+ 新增 `parse_small` / `build_small` 方法（+60-80 行） |

### 9.6 lib.rs 影响

| 候选 | 改 lib.rs？ | 改动位置 |
|------|------------|---------|
| A | 否 | 候选 A 不改模块注册 |
| B | **可能** | 若 raw C 函数作为模块级函数注册（非类型方法），需在 `_construct_rust`（L67-110）新增注册代码（+10-15 行） |
| C | 否 | 候选 C 不改模块注册 |

### 9.7 总改动量汇总

| 文件 | 候选 A | 候选 B | 候选 C | 合计 |
|------|--------|--------|--------|------|
| `_mixin.py` | +25-30 行 | 0 | 0 | +25-30 行 |
| `schema.rs` | 0 | +80-120 行 | 0 | +80-120 行 |
| `struct_node.rs` | 0 | 0 | +65-85 行 | +65-85 行 |
| `compile.rs` | 0 | 0 | 0（推荐方案） | 0 |
| `lib.rs` | 0 | +10-15 行（可能） | 0 | +10-15 行 |
| **合计** | **+25-30 行** | **+90-135 行** | **+65-85 行** | **+180-250 行** |

**对比 5.0 报告估算**：5.0 报告 §2 各候选估算（A ~80-120 / B ~100-150 / C ~60-80 行），本文档估算与报告一致（A 偏低因纯 Python 改动，B 偏高因含 SAFETY 注释）。

---

## 10. DEV 实施注意点

> 本节是 ARCH 给 DEV 的实施指引，非强制规范（DEV 可根据实际情况调整），但偏离时需说明理由。

### 10.1 候选 A 实施注意点

1. **仅 parse 路径去层化（5.1 v2 统一 P1-6）**：`_install_fast_call_bindings` 仅挂载
   `cls.parse = schema._parse_raw`，**不挂载** `cls.build`（保留继承的 StructMixin.build）。
   理由：build 是实例方法，setattr 会触发 self 冲突（BC-A1）；且 build 11.71x 已达标（L-05）。
   parse 路径已实证验证（`experiments/phase5_abi3_spike/verify_spike.py` spike 3.2/3.3）。

2. **延迟桩时序（BC-A3）**：`_remove_lazy_stubs` 末尾必须调用 `_install_fast_call_bindings`，
   否则延迟编译成功的类会回退到慢路径（StructMixin.parse classmethod）。

3. **fallback 语义（BC-A4）**：`_install_fast_call_bindings` 内部 try/except，失败时静默
   （log 警告到 stderr），保留 `StructMixin.parse` 作为 fallback。性能不达预期但不崩溃。

4. **实测收益参考（5.1 v2 Controlled A/B Test）**：候选 A 实测节省 ~60ns（高于原估算 30-50ns）。
   DEV 实施后应立即做 Controlled A/B Test 验证全场景收益稳定（micro-benchmark 数据仅供设计参考）。

5. **测试要求**：
   - 验证 `cls.parse is schema._parse_raw`（fast binding 挂载成功）
   - 验证 `MyClass.parse(data)` 与 `StructMixin.parse(MyClass, data)` 输出一致
   - 验证 `instance.parse(data)` 返回新实例（self 未错绑，spike 3.3 已验证）
   - 验证延迟桩路径（前向引用）正常工作

### 10.2 候选 B 实施注意点

1. **ABI3 spike 是第一步（强制，5.1 v2）**：实施候选 B 的第一步必须是 ABI3 spike
   （§3.2.3.3），验证路径 C 的风险 2（PyCFunction m_self 绑定语义）+ 风险 3（pyclass getattr 拦截）。
   Python 侧已验证风险 1（CompiledSchema 类型允许 setattr，`experiments/phase5_abi3_spike/verify_spike.py` spike 1.1）。
   - Spike PASS → 路径 C（B-aggressive，15-35ns）
   - Spike FAIL → 路径 D（模块级 + functools.partial，5-20ns）或 B-conservative（§3.2.5，5-10ns）
   - **不允许**为了候选 B 而禁用 ABI3（`PYO3_USE_ABI3_FORWARD_COMPATIBILITY=0`）

2. **SAFETY 注释完整性**：每处 unsafe 块必须有 SAFETY 注释（§3.3 SAFETY-1~5 模板）。
   VET 在 CODE_REVIEW 阶段逐项审查，缺注释直接驳回。

3. **miri 验证**：实施后用 `cargo +nightly miri test` 验证 raw FFI 的内存安全。
   miri 报错必须修复或回退。

4. **错误转换语义等价（SAFETY-4）**：raw C 函数的错误转换必须与 pyo3 `#[pymethods]` 的
   `?` 行为一致（都通过 error.rs:857 的 From impl）。测试：触发各种 ConstructError，
   验证 Python 侧捕获的异常类型 + 消息一致。

5. **性能验证**：实施后立即做 Controlled A/B Test（L-09 教训），验证 B1 parse rs_ns 下降。
   - 路径 C 期望下降 ≥15ns
   - 路径 D 期望下降 ≥5ns
   - B-conservative 期望下降 ≥5ns
   若下降 <5ns，候选 B 收益被高估，考虑放弃。

6. **测试要求**：
   - 所有现有 parse/build 测试通过（不回归）
   - 新增 raw FFI 专项测试：参数类型错误（BC-B1）/ GIL 持有 / 错误转换 / 内存安全（miri）
   - ABI3 spike 结果记录到过程记录

### 10.3 候选 C 实施注意点

1. **is_small_struct 自动计算（§4.2）**：推荐在 `StructNode::new` 内部根据
   `fields.len() / has_expressions / has_post_init` 自动计算，不扩展 `new` 签名。compile.rs 零改动。

2. **推荐"循环 + #[inline]"实现（5.1 v2，回应 REV P1-5）**：fast-path 字段处理采用
   `for field in &self.fields` 循环 + `#[inline]` 标注（§4.3 步骤 3 已给出完整实现）。
   理由：profiling 证明 LTO=fat 下编译器已将循环优化为指针递增，macro/显式展开边际收益极小
   （实测循环结构开销 0.14-5.14ns/field）。
   若 DEV 实测发现 macro/显式展开有 >3ns 额外收益（profiling 证明），可切换。

3. **语义等价（BC-C2）**：fast-path 与通用路径必须 100% 语义等价。
   Round-trip 测试覆盖所有 is_small_struct=true 的组合（§4.6 BC-C1 表格）。

4. **StopField 捕获（BC-C3）**：fast-path 的字段展开必须保留
   `Err(ConstructError::StopField { .. }) => break` 分支，不能简化为 `?` 传播。

5. **不优化 build 路径（保守）**：与候选 A 一致，build fast-path 暂不实施。
   build 11.71x 已达标，parse 是瓶颈。

6. **收益预期管理（5.1 v2）**：候选 C 实测修正收益为 **0-15ns**（原估算 6-15ns）。
   DEV 实施后 Controlled A/B Test 若显示收益 <1ns，**候选 C 可选放弃**（实施成本低但收益小，
   不值得增加代码复杂度）。决策由 PM 在 5.6 集成测试后基于实测数据定。

7. **测试要求**：
   - Round-trip 测试：fast-path vs 通用路径输出一致（BC-C2）
   - 边界值测试：is_small_struct 标志的所有组合（BC-C1）
   - StopField 捕获测试（BC-C3）
   - 性能测试：B1 parse rs_ns 下降 ≥1ns（5.1 v2 修正下界，原 ≥3ns）

### 10.4 集成实施注意点（5.6 子任务）

1. **实施顺序**：C → A → B（§5.1 推荐顺序）。每步实施后立即 Controlled A/B Test 验证收益，不要三候选一次性合并。

2. **决策点 A**（5.6 完成后）：
   - B1 ≥10x → 收尾，跳过 5.5（候选 D）
   - B1 9.5-10x → 评估是否接受（用户决策），或启用 5.5
   - B1 <9.5x → 回到设计：候选收益被高估，需重新 profiling 残差 60-95ns（5.0 报告 §1.2.3）

3. **回归验证**：5.6 必须覆盖所有 Phase 1-4 场景（B1-B7 + Phase 4 Array/StopIf/etc.），确保无回归。重点阴性对照：B3/B4（已 ≥10x）+ Phase 4 StopIf 不触发场景。

4. ** Controlled A/B Test 强制**（L-09 教训）：禁止跨时段单次对比。同会话交替测量 A/B 至少 3 轮，含阳性对照 + 阴性对照 + DLL hash 验证。

---

## 11. 引用证据清单

### 11.1 性能数据点（CSV 行号）

| 数据点 | CSV 行 | 用途 |
|--------|--------|------|
| B1 parse 3123/386/8.09x | 行 2 | B1 现状基线（本文档核心场景） |
| B2 parse 6068/628/9.66x | 行 4 | 字段级贡献反推 + 阴性对照 |
| B3 parse 23314/2020/11.54x | 行 6 | 已达标阴性对照 |
| B4 parse 44097/3656/12.06x | 行 8 | 已达标阴性对照 |
| B1 build 11.71x | 行 3 | build 路径不需优化的依据 |
| B7（空 Struct）331ns | 行 14 | 固定开销参照 + 阴性对照 2 |
| StopIf S03 2995/301/9.96x | 行 106 | 候选 D 受益场景（5.5 后备触发条件） |
| a_err_eof 11.34x | 行 153 | 候选 B 阳性对照（raw FFI 已在错误路径验证） |

### 11.2 源码位置

| 文件 | 行号 | 内容 | 本文档引用 |
|------|------|------|-----------|
| `construct-rs/src/schema.rs` | 148-165 | `_parse_raw` 当前实现（pyo3 #[pymethods]） | §1.2 步骤 3 / §3.1 |
| `construct-rs/src/schema.rs` | 180-204 | `_build_raw` 当前实现 | §3.1 |
| `construct-rs/src/nodes/struct_node.rs` | 274-376 | StructNode.parse 完整流程 | §1.2 步骤 5 / §4.3 |
| `construct-rs/src/nodes/struct_node.rs` | 340-362 | 无表达式路径（候选 C 攻击点） | §4.3 |
| `construct-rs/src/nodes/struct_node.rs` | 378-447 | StructNode.build 完整流程 | §4.4 |
| `construct-rs/src/nodes/struct_node.rs` | 170-215 | StructNode struct 定义 + new | §4.2 |
| `construct-rs/src/instance.rs` | 72-97 | `create_class`（tp_new 22ns 来源） | §1.2 步骤 5 |
| `construct-rs/src/instance.rs` | 120-143 | `force_setattr`（R4 历史） | §6 §0 第 2 条对照 |
| `construct-rs/src/error.rs` | 740-825 | `try_fast_path_alloc`（raw FFI 先例，ADR-019） | §3.4 |
| `construct-rs/src/error.rs` | 857-896 | `From<ConstructError> for PyErr` | §3.3 SAFETY-4 / §6 §0 第 7 条 |
| `construct-rs/src/lib.rs` | 67-110 | `_construct_rust` 模块注册 | §3.2.3 / §9.6 |
| `construct-rs/src/compile.rs` | 104-241 | `compile_schema` 编译入口 | §9.2 |
| `construct-rs/src/compile.rs` | 223-229 | StructNode::new 调用 | §4.2 / §9.2 |
| `construct-rs/python/construct/_mixin.py` | 1051-1070 | `StructMixin.parse` classmethod（候选 A 攻击点） | §2.1 |
| `construct-rs/python/construct/_mixin.py` | 907-992 | 延迟桩机制（候选 A 需同步处理） | §2.3.3 / §8.2 |
| `construct-rs/python/construct/_mixin.py` | 995-1004 | `_remove_lazy_stubs`（候选 A 改动点） | §2.3.3 |
| `construct-rs/python/construct/_mixin.py` | 821-894 | `_compile_schema_for_class`（候选 A 改动点） | §2.3.2 |

### 11.3 历史分析报告与 ADR

| 文档 | 关键数据 | 本文档引用 |
|------|---------|-----------|
| `docs/analysis/phase3-parse-build不对称.md` §3.3 | tp_new = 22ns（实测） | §1.2 步骤 5 / §5.3.1 |
| `docs/analysis/phase3-parse-build不对称.md` §3.5 | getattr_dict+downcast = 11.6ns / dict.set_item = 13.5ns | §1.2 步骤 5 / §7.5 |
| `docs/analysis/phase3-parse-build不对称.md` §4.2 | Python 入口差 C = 41-49ns | §2.1 |
| `docs/analysis/E01-O1-regression-investigation.md` | L-09 教训原始证据 + Controlled A/B Test 方法学 | §7.3 |
| `docs/decisions/ADR-008` | parse 借用实例 `__dict__`（R4 优化） | §6 §0 第 2 条 |
| `docs/decisions/ADR-016` | P0-3 lazy path 错误传播模式 | §4.3（fast-path 保留 push_path_segment） |
| `docs/decisions/ADR-019` | 错误抛出 fast-path（首次 unsafe raw FFI 验证） | §3.4（候选 B 先例） |
| `harness/experiences.md §L-01` | 中间表示层违反 | §6 §0 对照表（硬要求） |
| `harness/experiences.md §L-02` | 理论估算替代实证数据 | §7 性能假设 |
| `harness/experiences.md §L-05` | 优化 A 路径忽略 B 路径 | §5.3 / §7.5 |
| `harness/experiences.md §L-09` | 跨时段性能对比归因失效 | §7.3 / §7.4 |

### 11.4 5.1 v2 新增实验证据（P0-1 / P0-2 / P1-3 回应）

| 文档 | 关键数据 | 本文档引用 |
|------|---------|-----------|
| `experiments/phase5_abi3_spike/分析报告-ABI3兼容性.md` | ABI3 4 路径可行性矩阵 + spike 计划 + B-conservative 详细设计 | §3.2.3.1 / §3.2.3.2 / §3.2.3.3 / §3.2.5 |
| `experiments/phase5_abi3_spike/verify_spike.py` + `spike_result.md` | Python 侧 spike 8/9 PASS：pyclass 类型 setattr 可行（风险 1 排除）+ pyo3 bound method 类属性行为验证（候选 A 核心假设，回应 REV P1-3） | §2.3.2 / §2.6 BC-A1 / §3.2.3.1 |
| `experiments/phase5_candidate_c_profiling.py` + `_result.md` | per-field 边际成本 28.64ns（1-3 字段平均），循环结构开销 0.14-5.14ns/field，候选 C 3 字段收益 0.4-15.4ns（中位 7.93ns） | §4.3 / §7.1 / §7.5 / §5.3.1 |
| `experiments/phase5_candidate_a_ab_test.py` + `_ab_result.md` | Controlled A/B Test：候选 A 实测节省 ~60ns（高于原估算 30-50ns），3 轮 A/B 交替稳定 | §7.1 / §7.2 / §10.1 / §12.2 |

---

## 12. 设计完成状态

### 12.1 任务要求覆盖检查

| 任务要求（PM 分派） | 本文档对应节 | 状态 |
|-------------------|------------|------|
| 1. 候选 A 详细设计（当前/去层化调用链 + 接口签名 + 数据流 + 影响源码清单） | §2 | ✅ |
| 2. 候选 B 详细设计（unsafe 块清单 + SAFETY 审查 + Phase 4.x 先例 + 回退方案） | §3 | ✅ |
| 3. 候选 C 详细设计（触发条件 + 专用路径 + 不影响大字段保证） | §4 | ✅ |
| 4. 集成方案（实施顺序 + 接口一致性 + 回归风险 L-05） | §5 | ✅ |
| 5. 性能假设（L-02 可证伪 + L-09 验证方法 + 波动阈值） | §7 | ✅ |
| 6. 边界条件清单 | §8 | ✅ |
| 7. 与现有架构集成（Node enum / compile.rs / _mixin.py） | §9 | ✅ |
| frontmatter（id/status/phase/last_updated） | 文档头 | ✅ |
| §0 原则对照表（L-01 教训硬要求） | §6 | ✅ |

### 12.2 需要PM注意的事项（5.1 v2 更新）

1. **B1 ≥10x 达标概率上调**（5.1 v2 修正）：
   - 原估算中位 9.88x（临界）→ 5.1 v2 修正中位 **~10.2x**（趋于达标）
   - **关键变量是候选 A**：Controlled A/B Test 实测节省 ~60ns（高于原估算 30-50ns）
   - 若候选 A 在 5.6 全场景验证中稳定 ~60ns，B1 ≥10x 概率显著提升
   - 候选 B/C 即使落在区间下界，A + B-conservative 仍达 ~9.7x（接近 10x）
   - 仍需 5.6 Controlled A/B Test 全场景验证（micro-benchmark ≠ 集成测试，L-09 教训）

2. **候选 B ABI3 兼容性（5.1 v2 部分解决）**：
   - **SAFETY-1~5 所需 C API 全部 ABI3 稳定**（error.rs / instance.rs 已有先例）
   - **类型方法注册有 4 路径**（§3.2.3.1）：路径 A/B 排除，路径 C 理论可行待 spike，
     路径 D / B-conservative 完全可行
   - **5.3 子任务 DEV 第一步必须是 spike**（§3.2.3.3），验证路径 C 风险 2/3
   - Spike 失败 → 回退路径 D（5-20ns）或 B-conservative（5-10ns，§3.2.5 详细设计）
   - **不再有"未知技术障碍"**——所有路径都有明确可行性与回退方案

3. **候选 A build 路径保守保留（5.1 v2 统一）**：
   - §2.3.2 + §2.6 已统一：**仅 parse 路径去层化**，build 保留 StructMixin.build
   - parse 路径已 Python spike 实证验证（`experiments/phase5_abi3_spike/verify_spike.py`）：
     `cls.parse = schema._parse_raw` 后类调用与实例调用都正确，self 绑定为 schema
   - build 路径无需优化（11.71x 已达标，L-05 教训）

4. **候选 C 收益下调（5.1 v2 profiling 修正）**：
   - 原估算 6-15ns → 实测修正 **0-15ns**（下界下调，LTO=fat 下编译器已优化循环）
   - 候选 C 价值降低，但实施成本低（纯 Rust 内部，无 unsafe），仍建议实施作为叠加优化
   - **不再将候选 C 视为达标关键**——候选 A 是绝对主力

5. **5.6 必须用 Controlled A/B Test**（L-09 教训）：禁止跨时段单次对比。
   同会话交替测量 + DLL hash 验证 + 阴性对照。5.1 v2 的 micro-benchmark 仅作设计验证，
   不替代集成测试。

6. **L-09 跨时段数据差异注意**：5.1 v2 micro-benchmark 测得当前 B1 parse 端到端 ~265ns，
   而 perf-scenarios.csv 行 2 记录 rs_ns=386ns（Phase 1 测量）。差异源于：
   - 测量环境（Python 3.14 vs Phase 1 时期版本、不同 venv、CPU 状态）
   - 测量口径（micro-benchmark min(repeat) vs Phase 1 集成测试口径）
   - **不影响候选 A 相对收益结论**（同会话 Controlled A/B Test），但**影响绝对加速比预测**
   - 5.6 集成测试需用当前测量环境重新建立 B1 基线，不可直接引用 Phase 1 的 386ns

### 12.3 设计完成声明

- [x] §0 文档定位与边界
- [x] §1 整体方案概览（A+B+C 组合，5.1 v2 实测修正）
- [x] §2 候选 A 详细设计（FFI 去层化，5.1 v2 P1-6 统一仅 parse 路径）
- [x] §3 候选 B 详细设计（pyo3 raw FFI 热路径，5.1 v2 P0-1 补充 ABI3 4 路径 + spike + B-conservative）
- [x] §4 候选 C 详细设计（小字段 fast-path，5.1 v2 P0-2 profiling 修正收益 + 推荐循环实现）
- [x] §5 集成方案（实施顺序 + 接口一致性 + 回归风险 L-05）
- [x] §6 §0 原则对照表（L-01 教训硬要求）
- [x] §7 性能假设（L-02/L-05/L-09 教训，5.1 v2 候选 A 实测 + 候选 C profiling 修正）
- [x] §8 边界条件清单（5.1 v2 BC-B3 更新）
- [x] §9 与现有架构的集成
- [x] §10 DEV 实施注意点（5.1 v2 三候选注意点全部更新）
- [x] §11 引用证据清单（5.1 v2 新增 §11.4 实验证据）
- [x] §12 设计完成状态（5.1 v2 §12.2 PM 注意事项更新）

**设计文档状态**：DESIGNING v2 完成（5.R 驳回后修正），等待 REV 重新检视（5.R v2 子任务）。

**5.1 v2 修正覆盖的驳回项**：
- ✅ P0-1（ABI3 兼容性）：§3.2.3.1 4 路径分析 + §3.2.3.3 spike 计划 + §3.2.5 B-conservative 详细设计
- ✅ P0-2（候选 C profiling）：§4.3 实测修正 + §7.1 收益下调 + experiments/phase5_candidate_c_profiling.py
- ✅ P1-6（文档矛盾）：§2.3.2 + §2.6 BC-A1 统一为仅 parse 去层化
- ✅ P1-3（pyo3 bound method 假设）：experiments/phase5_abi3_spike/verify_spike.py 实证验证（8/9 PASS）
- ✅ P1-5（候选 C fast-path 实现）：§4.3 步骤 3 推荐"循环 + #[inline]"完整实现
- ✅ P2-1（候选 A 属性查找估算）：experiments/phase5_candidate_a_ab_test.py 实测 ~60ns（含全部 Python 入口开销）
