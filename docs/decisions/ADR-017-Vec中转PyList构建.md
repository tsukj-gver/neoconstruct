---
id: ADR-017
status: accepted
phase: cross
decides: "Vec 中转 PyList 构建模式"
supersedes: []
superseded_by: []
depends_on: []
last_updated: 2026-07-27
---

# ADR-017: Vec 中转 PyList 构建

## Context

返回 list 的 Node（如 ArrayNode.parse）需要把多个 PyObject 收集成 Python list。

直接 `PyList::new_bound(py, Vec::with_capacity(count))` + `append` 会丢失 capacity hint，触发 realloc 慢路径。

## Decision

解析多个元素时：
1. 用 `Vec<Py<PyAny>>::with_capacity(count)` 收集（编译期或运行期已知 count）
2. 最后一次性 `PyList::new_bound(py, elems)`（pyo3 内部用 `PyList_New` + `PyList_SET_ITEM`）

## 适用范围

所有返回 list 的 Node（parse 路径）。

## 禁止行为

- ❌ `PyList::new_bound(py, Vec::with_capacity(count))` + `append`（capacity hint 丢失，走 realloc 慢路径）

## Consequences

- 正面：单次 PyList 构造，无 realloc
- 正面：pyo3 内部 `PyList_SET_ITEM` 直接 steal ref，无 refcount 抖动

## Relations

- 首次验证：Phase 4.7（Array 系列 4 节点）
- 引用证据：`docs/reviews/架构审查-重复代码与抽象质量.md` §1.2
