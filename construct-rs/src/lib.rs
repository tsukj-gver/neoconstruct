//! construct-rs crate 入口与 pyo3 模块注册。
//!
//! 本 crate 是 Python 包 `construct` 的高性能 Rust 内核。
//! 编译后由 maturin 安装为 `construct._construct_rust` 扩展模块，
//! Python 侧通过 `from ._construct_rust import *` 导入。
//!
//! Phase 1：
//! - 子任务 1.1：项目骨架，注册模块名与 `version` 函数，验证 FFI 链路。
//! - 子任务 1.2：Rust 基础设施（错误 / Path / Stream / Context），不暴露给 Python。
//! - 子任务 1.3：Node 系统 + 3 个原子节点（FormatField / Bytes / GreedyBytes）。
//! - 子任务 1.4：复合节点（StructNode / StructRefNode）+ CompiledSchema 定义。
//!
//! 后续子任务将逐步填充编译入口（compile_schema）、parse/build 入口、
//! 类型描述符（Int8ub 等）与 Python 异常层次。
//!
//! 参考设计：`docs/架构设计.md` §D.1（crate 结构）、§B（FFI 边界）。

pub mod context;
pub mod error;
pub mod nodes;
pub mod path;
pub mod schema;
pub mod stream;

use pyo3::prelude::*;

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
/// 当前仅注册 `version` 函数。后续子任务将在此注册：
/// - `compile_schema`：编译入口（§B.2）
/// - `CompiledSchema`：编译产物 pyclass（§B.6）
/// - 类型描述符单例（Int8ub 等，§A.4）
#[pymodule]
fn _construct_rust(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(version, m)?)?;
    m.add("__version__", version())?;
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
