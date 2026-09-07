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

use std::borrow::Cow;

use crate::error::ConstructError;
use crate::path::Path;
use pyo3::buffer::PyBuffer;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyByteArray, PyBytes, PyList, PyTuple};

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

// ---------------------------------------------------------------------------
// 不可信 count 预分配封顶
// ---------------------------------------------------------------------------

/// 元素静态尺寸未知（或为 0）时的结果 Vec 初始容量。
///
/// 零尺寸/动态尺寸元素无法用流剩余长度推定元素数上限，预分配退化为
/// 小常量起步，由 `push` 按摊还策略增长（永不按不可信 count 巨额预分配）。
const DYNAMIC_ELEMENT_INITIAL_CAPACITY: usize = 16;

/// 按 count 与流剩余长度计算结果 Vec 的安全预分配容量。
///
/// 防御不可信 count（网络报文/损坏文件中的 32-bit 长度字段被置为
/// `0xFFFFFFFF`）触发的巨型预分配 abort（Rust 分配失败直接终止进程，
/// Python 层无法 catch）。
///
/// 规则：
/// - 元素静态尺寸 `size >= 1` 字节：`cap = min(count, remaining / size)`。
///   顺序流中从当前位置起最多解析 `remaining / size` 个该尺寸元素，
///   预分配不会超出实际可解析数量；count 超出部分由逐元素 parse 的
///   自然 `Stream` 错误拒绝。
/// - 元素尺寸未知（sizeof 失败，如 GreedyBytes）或为 0（如 Pass）：
///   退化为 `min(count, DYNAMIC_ELEMENT_INITIAL_CAPACITY)`。
///   封顶值是保守下界（bit 域元素的 sizeof 单位是 bit，混用时同样只会
///   低估不会高估），实际超出容量时由 push 摊还增长补齐。
///
/// 分配量由此以输入规模为上界（O(remaining)），不再以不可信 count 为上界。
pub fn capped_result_capacity(
    count: usize,
    min_elem_size: Option<usize>,
    stream_remaining: usize,
) -> usize {
    match min_elem_size {
        Some(size) if size > 0 => count.min(stream_remaining / size),
        _ => count.min(DYNAMIC_ELEMENT_INITIAL_CAPACITY),
    }
}

// ---------------------------------------------------------------------------
// bytes-like 值提取（bytes / bytearray / memoryview）
// ---------------------------------------------------------------------------

/// 提取 bytes-like 对象的字节内容为 [`Cow`] 切片。
///
/// 接受三种形态（语义等同 `bytes`）：
///
/// - `bytes`：`PyBytes::as_bytes` 零拷贝借用；
/// - `bytearray`：经 `to_vec` 取一次拷贝（pyo3 未提供安全的借用切片 API，
///   而 build 侧随后写入 `BuildStream` 本就需要一次拷贝，无额外净开销）；
/// - `memoryview` 等 buffer 协议对象：经 `PyBuffer::to_vec` 取一次拷贝
///   （非 u8 格式 / 非连续缓冲由 PyBuffer 拒绝，落入 Err）。
///
/// 其他类型返回 `Err(类型名)`，由调用方包装为构造错误。
pub fn extract_bytes_like<'a, 'py>(
    py: Python<'py>,
    obj: &'a Bound<'py, PyAny>,
) -> Result<Cow<'a, [u8]>, String> {
    if let Ok(py_bytes) = obj.downcast::<PyBytes>() {
        return Ok(Cow::Borrowed(py_bytes.as_bytes()));
    }
    if let Ok(py_bytearray) = obj.downcast::<PyByteArray>() {
        return Ok(Cow::Owned(py_bytearray.to_vec()));
    }
    if let Ok(buffer) = PyBuffer::<u8>::get_bound(obj) {
        return buffer
            .to_vec(py)
            .map(Cow::Owned)
            .map_err(|_| obj_type_name(obj));
    }
    Err(obj_type_name(obj))
}

// `PyAny` import 保留用于未来 doc-link 与潜在扩展（如 type check 辅助）。
#[allow(dead_code)]
fn _ensure_pyany_in_scope(_obj: &Bound<'_, PyAny>) {}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn ensure_python() {
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(pyo3::prepare_freethreaded_python);
    }

    fn with_py<F, R>(f: F) -> R
    where
        F: for<'py> FnOnce(pyo3::Python<'py>) -> R,
    {
        ensure_python();
        pyo3::Python::with_gil(f)
    }

    // ======================================================================
    // capped_result_capacity
    // ======================================================================

    #[test]
    fn capacity_huge_count_capped_by_stream_remaining() {
        // 不可信 count（0xFFFFFFFF）+ 静态尺寸元素 → 封顶为 remaining/size，
        // 不再按 count 巨额预分配。
        let cap = capped_result_capacity(0xFFFF_FFFF, Some(1), 4);
        assert_eq!(cap, 4);
    }

    #[test]
    fn capacity_two_byte_element_halves_cap() {
        let cap = capped_result_capacity(1000, Some(2), 7);
        assert_eq!(cap, 3);
    }

    #[test]
    fn capacity_small_count_kept_as_is() {
        // 合法小 count 不受封顶影响（min 语义）。
        assert_eq!(capped_result_capacity(3, Some(1), 100), 3);
        assert_eq!(capped_result_capacity(0, Some(1), 100), 0);
    }

    #[test]
    fn capacity_dynamic_element_falls_back_to_const() {
        // 尺寸未知（None）/零尺寸（Some(0)）→ 小常量起步。
        assert_eq!(capped_result_capacity(0xFFFF_FFFF, None, 100), 16);
        assert_eq!(capped_result_capacity(0xFFFF_FFFF, Some(0), 100), 16);
        assert_eq!(capped_result_capacity(8, None, 100), 8);
    }

    #[test]
    fn capacity_empty_stream_capped_to_zero() {
        assert_eq!(capped_result_capacity(0xFFFF_FFFF, Some(1), 0), 0);
    }

    // ======================================================================
    // extract_bytes_like
    // ======================================================================

    #[test]
    fn extract_bytes_returns_borrowed_slice() {
        with_py(|py| {
            let obj = py.eval_bound("b'ab'", None, None).expect("eval");
            match extract_bytes_like(py, &obj).expect("extract") {
                Cow::Borrowed(s) => assert_eq!(s, b"ab"),
                Cow::Owned(_) => panic!("bytes 应走零拷贝借用"),
            }
        });
    }

    #[test]
    fn extract_bytearray_returns_same_content() {
        with_py(|py| {
            let obj = py
                .eval_bound("bytearray(b'xyz')", None, None)
                .expect("eval");
            let data = extract_bytes_like(py, &obj).expect("extract");
            assert_eq!(data.as_ref(), b"xyz");
        });
    }

    #[test]
    fn extract_memoryview_returns_same_content() {
        with_py(|py| {
            let obj = py
                .eval_bound("memoryview(b'pqrs')", None, None)
                .expect("eval");
            let data = extract_bytes_like(py, &obj).expect("extract");
            assert_eq!(data.as_ref(), b"pqrs");
        });
    }

    #[test]
    fn extract_rejects_non_bytes_like() {
        with_py(|py| {
            for expr in ["42", "'str'", "None", "[1, 2]"] {
                let obj = py.eval_bound(expr, None, None).expect("eval");
                let err = extract_bytes_like(py, &obj).expect_err("should reject");
                assert!(!err.is_empty(), "错误应携带类型名");
            }
        });
    }
}
