//! construct-rs crate 入口与 pyo3 模块注册。
//!
//! 本 crate 是 Python 包 `construct` 的高性能 Rust 内核。
//! 编译后由 maturin 安装为 `construct._construct_rust` 扩展模块，
//! Python 侧通过 `from ._construct_rust import *` 导入。
//!
//! ## Phase 1
//!
//! - 子任务 1.1：项目骨架，注册模块名与 `version` 函数，验证 FFI 链路。
//! - 子任务 1.2：Rust 基础设施（错误 / Path / Stream / Context），不暴露给 Python。
//! - 子任务 1.3：Node 系统 + 3 个原子节点（FormatField / Bytes / GreedyBytes）。
//! - 子任务 1.4：复合节点（StructNode / StructRefNode）+ CompiledSchema 定义。
//! - 子任务 1.5+1.6：编译入口 `compile_schema`、执行入口 `_parse_raw` / `_build_raw`、
//!   类型描述符 pyclass（Int8ub 等 16 种单例 + BytesDescriptor + GreedyBytesDescriptor）。
//! - 子任务 1.8：错误映射更新——模块初始化时缓存 Python 异常类引用，
//!   `From<ConstructError> for PyErr` 按变体选择对应 Python 异常类。
//!
//! 参考设计：`docs/架构设计.md` §D.1（crate 结构）、§B（FFI 边界）、§A.4（描述符）、
//! §B.8（错误映射）。

pub mod compile;
pub mod context;
pub mod descriptors;
pub mod error;
pub mod instance;
pub mod nodes;
pub mod path;
pub mod schema;
pub mod stream;

use descriptors::{BytesDescriptor, FormatFieldDescriptor, GreedyBytesDescriptor};
use nodes::format_field::PythonFormat;
use pyo3::prelude::*;
use schema::CompiledSchema;

/// 返回 construct-rs Rust 内核的版本号字符串。
///
/// 用于验证 Python↔Rust FFI 链路是否可用：
///
/// ```python
/// from construct._construct_rust import version
/// print(version())  # "0.1.0"
/// ```
#[pyfunction]
fn version() -> &'static str {
    "0.1.0"
}

/// 注册 Python 扩展模块 `construct._construct_rust`。
///
/// 注册项：
/// - `version` 函数 + `__version__` 常量
/// - [`compile::compile_schema`]：编译入口 FFI 函数（§B.2）
/// - [`schema::CompiledSchema`]：编译产物 pyclass（§B.6），含 `_parse_raw` / `_build_raw`
/// - 类型描述符 pyclass（§A.4）：
///   - 16 个 `FormatFieldDescriptor` 单例（`Int8ub` 等）
///   - `BytesDescriptor`（可实例化）
///   - `GreedyBytesDescriptor` 单例（`GreedyBytes`）
///
/// 模块初始化时还会调用 [`error::init_exception_classes`]，从 `construct._errors`
/// 缓存 Python 异常类引用，使 `From<ConstructError> for PyErr` 能按变体映射到
/// `StreamError` 等（§B.8）。
#[pymodule]
fn _construct_rust(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = m.py();

    m.add_function(wrap_pyfunction!(version, m)?)?;
    m.add("__version__", version())?;

    // FFI 入口
    m.add_function(wrap_pyfunction!(compile::compile_schema, m)?)?;

    // pyclass 注册
    m.add_class::<CompiledSchema>()?;
    m.add_class::<FormatFieldDescriptor>()?;
    m.add_class::<BytesDescriptor>()?;
    m.add_class::<GreedyBytesDescriptor>()?;

    // 16 个 FormatFieldDescriptor 预定义单例
    register_format_singletons(py, m)?;

    // GreedyBytes 预定义单例
    m.add("GreedyBytes", Py::new(py, GreedyBytesDescriptor)?)?;

    // 缓存 Python 异常类引用（子任务 1.8）。
    // 失败不致命：未初始化时 From<ConstructError> 回退到 PyValueError。
    // 但记录到 stderr 帮助调试。
    if let Err(e) = error::init_exception_classes(py) {
        // 使用 Python 的 print 输出（确保 GIL 持有），不阻塞模块加载。
        let _ = py
            .import_bound("sys")
            .and_then(|sys| sys.getattr("stderr"))
            .and_then(|stderr| {
                stderr.call_method1(
                    "write",
                    (format!(
                        "construct-rs: 警告——无法初始化 Python 异常类缓存，\
                         错误将回退到 ValueError：{}\n",
                        e
                    ),),
                )
            });
    }

    Ok(())
}

/// 注册 16 个整数格式预定义单例到模块。
///
/// 对应 Python construct 的 `Int8ub`、`Int16sb` 等原子格式常量。
/// 每个单例为 `FormatFieldDescriptor` pyclass 实例（frozen，模块级常量）。
fn register_format_singletons(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    /// 16 种整数格式：`(Python 标识符, 预编译格式)`。
    ///
    /// 使用常量数组避免硬编码散落的 `m.add` 调用，集中管理单例定义。
    const SINGLETONS: &[(&str, PythonFormat)] = &[
        ("Int8ub", PythonFormat::UnsignedInt8Big),
        ("Int8ul", PythonFormat::UnsignedInt8Little),
        ("Int8sb", PythonFormat::SignedInt8Big),
        ("Int8sl", PythonFormat::SignedInt8Little),
        ("Int16ub", PythonFormat::UnsignedInt16Big),
        ("Int16ul", PythonFormat::UnsignedInt16Little),
        ("Int16sb", PythonFormat::SignedInt16Big),
        ("Int16sl", PythonFormat::SignedInt16Little),
        ("Int32ub", PythonFormat::UnsignedInt32Big),
        ("Int32ul", PythonFormat::UnsignedInt32Little),
        ("Int32sb", PythonFormat::SignedInt32Big),
        ("Int32sl", PythonFormat::SignedInt32Little),
        ("Int64ub", PythonFormat::UnsignedInt64Big),
        ("Int64ul", PythonFormat::UnsignedInt64Little),
        ("Int64sb", PythonFormat::SignedInt64Big),
        ("Int64sl", PythonFormat::SignedInt64Little),
    ];

    for (name, format) in SINGLETONS {
        let desc = Py::new(py, FormatFieldDescriptor::new(name, *format))?;
        m.add(*name, desc)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_returns_expected_string() {
        assert_eq!(version(), "0.1.0");
    }

    #[test]
    fn version_is_non_empty() {
        assert!(!version().is_empty());
    }
}
