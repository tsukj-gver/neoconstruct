# Phase 5 ABI3 + pyo3 bound method Python 侧 Spike 结果

测量时间：2026-07-29T20:06:17
Python：3.14.2

## 汇总：8/9 PASS, 1/9 FAIL

## 详细结果

| # | 检查项 | 状态 | 详情 |
|---|--------|------|------|
| 1 | 1.1 CompiledSchema 类型 setattr 新属性 | PASS | sentinel 存活：retrieved is sentinel = True |
| 2 | 1.2 类型 setattr Python 函数 | PASS | setattr 成功（Python 函数作为类型属性） |
| 3 | 2.1 schema._parse_raw 类型 | PASS | type(schema._parse_raw) = builtin_function_or_method |
| 4 | 2.2 schema._parse_raw(data) 直接调用 | PASS | 返回 TestStruct 实例：TestStruct |
| 5 | 2.2.1 字段值正确 | PASS | (x,y,z) = (1,2,3) |
| 6 | 3.2 TestStruct.parse(data) 类调用 | PASS | 返回 TestStruct 实例，字段正确 |
| 7 | 3.3 instance.parse(data) 实例调用（self 绑定验证） | PASS | 返回新实例，字段=(1,2,3)，self 未错绑为 dummy_instance |
| 8 | 4.1 StructMixin.parse 是 classmethod | FAIL | type(StructMixin.parse) = method |
| 9 | 4.2 schema._parse_raw 不是 classmethod | PASS | type = builtin_function_or_method |

## 覆盖范围

- 候选 A 核心假设（pyo3 bound method 作为类属性，self 自动绑定为 schema）：✅ 验证
- 候选 B 路径 C 风险 1（pyclass 类型 setattr）：✅ 初步验证（Python 函数）
- 候选 B 路径 C 风险 2（PyCFunction m_self 绑定语义）：❌ 需 Rust 侧 spike
- 候选 B 路径 C 风险 3（pyclass getattr 拦截）：❌ 需 Rust 侧 spike