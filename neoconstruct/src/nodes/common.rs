//! Array 系列节点共享工具层。
//!
//! ## 概述
//!
//! Array 系列 4 节点（ArrayNode / GreedyRangeNode / PrefixedArrayNode /
//! RepeatUntilNode）的 build 路径都需要把用户传入的 obj（list/tuple/任意 iterable）
//! 收集到 `Vec<Py<PyAny>>`。该逻辑此前在 4 个文件中重复（差异仅错误消息中的
//! 类型名），提取为公共 helper。
//!
//! ## 采用方
//!
//! 本模块提供的 helper 已被以下节点采用：
//! - ArrayNode（array.rs）
//! - GreedyRangeNode（greedy_range.rs）
//! - PrefixedArrayNode（prefixed_array.rs）
//! - RepeatUntilNode（repeat_until.rs）

use crate::error::ConstructError;
use crate::path::Path;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyList, PyTuple};

/// 从 list / tuple / 任意 iterable 收集元素到 `Vec<Py<PyAny>>`。
///
/// # 快路径
///
/// - `PyList` / `PyTuple`：直接 `iter().map(|b| b.unbind()).collect()`（零额外开销）。
/// - 其他类型：尝试 `obj.iter()` 兜底（性能差，仅在用户传非 list/tuple 时触发）。
///
/// # 参数
///
/// - `obj`：待收集的 Python 对象（用户传入的 list/tuple/generator 等）。
/// - `node_name`：调用方节点名（如 `"Array"` / `"GreedyRange"` / ...），
///   用于错误消息中定位。
/// - `path`：错误追踪路径栈。
///
/// # 错误
///
/// - [`ConstructError::Generic`]：`obj` 不是 iterable 或迭代中抛异常，
///   message 含 `node_name` + 实际类型名 + 原始错误。
pub fn collect_obj_to_vec(
    obj: &Bound<'_, PyAny>,
    node_name: &str,
    path: &mut Path,
) -> Result<Vec<Py<PyAny>>, ConstructError> {
    if let Ok(list) = obj.downcast::<PyList>() {
        Ok(list.iter().map(|b| b.unbind()).collect())
    } else if let Ok(tuple) = obj.downcast::<PyTuple>() {
        Ok(tuple.iter().map(|b| b.unbind()).collect())
    } else {
        let type_name = obj_type_name(obj);
        let iter = obj.iter().map_err(|e| ConstructError::Generic {
            message: format!(
                "{} build expects list/tuple, got {} (iter error: {})",
                node_name, type_name, e
            ),
            path: path.to_string(),
        })?;
        iter.map(|b| b.map(|bound| bound.unbind()))
            .collect::<PyResult<Vec<_>>>()
            .map_err(|e| ConstructError::Generic {
                message: format!("{} build iterable error: {}", node_name, e),
                path: path.to_string(),
            })
    }
}

/// 取 Python 对象的类型名（用于错误消息）。
///
/// 失败时返回 `"<unknown>"`（防御性，不 panic）。
#[inline]
pub fn obj_type_name(obj: &Bound<'_, PyAny>) -> String {
    obj.get_type()
        .name()
        .map(|n| n.to_string())
        .unwrap_or_else(|_| "<unknown>".to_string())
}

// `PyAny` import 保留用于未来 doc-link 与潜在扩展（如 type check 辅助）。
#[allow(dead_code)]
fn _ensure_pyany_in_scope(_obj: &Bound<'_, PyAny>) {}
