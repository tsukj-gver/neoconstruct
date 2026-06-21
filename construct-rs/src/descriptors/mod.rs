//! 类型描述符 pyclass：Python 可见的二进制格式声明对象。
//!
//! 设计依据：`docs/架构设计.md` §A.4。
//!
//! ## 三种描述符形态
//!
//! | 描述符 | Python 侧 | Rust 侧 | 用途 |
//! |--------|-----------|---------|------|
//! | [`FormatFieldDescriptor`] | 预定义单例（`Int8ub` 等 16 种） | 预编译的 [`PythonFormat`] | 整数格式字段 |
//! | [`BytesDescriptor`] | 可实例化（`Bytes(n)`） | 固定 `length` | 固定长度原始字节 |
//! | [`GreedyBytesDescriptor`] | 预定义单例（`GreedyBytes`） | 单元结构 | 剩余全部字节 |
//!
//! ## 与编译器（compile_schema）的协作
//!
//! 用户通过 `field(Int8ub)` 或 `field(Bytes(4))` 将描述符传入字段声明。
//! `__init_subclass__` 收集描述符列表后调用 [`crate::compile::compile_schema`]，
//! 后者通过 pyo3 `extract::<Py<FormatFieldDescriptor>>()` 等类型安全地识别描述符，
//! 读取内部参数构建对应的执行树节点（[`crate::nodes::Node`]）。
//!
//! ## 不可变性
//!
//! 所有描述符均为 `#[pyclass(frozen)]`，Python 侧无法修改其属性。
//! 这保证编译期确定的格式参数在运行时不被篡改。

use crate::nodes::format_field::PythonFormat;
use pyo3::prelude::*;

// ---------------------------------------------------------------------------
// FormatFieldDescriptor
// ---------------------------------------------------------------------------

/// 整数格式描述符。对应 Python 侧的 `Int8ub`、`Int16sb` 等预定义单例。
///
/// 每个 `Int*ub` / `Int*sb` / `Int*ul` / `Int*sl` 预定义单例在模块加载时
/// 创建为一个 `FormatFieldDescriptor` 实例，作为模块级常量。
///
/// 用户通过 `field(Int8ub)` 传入，编译器（`compile_schema`）通过 pyo3 `extract`
/// 识别此类型，读取内部的 `format` 字段构建对应的 `FormatFieldNode`。
///
/// # Phase 1
///
/// 支持 16 种整数格式（8 种类型 × 2 种字节序）。浮点与布尔推迟到 Phase 2。
#[pyclass(frozen, name = "FormatFieldDescriptor", module = "construct")]
#[derive(Debug, Clone, Copy)]
pub struct FormatFieldDescriptor {
    /// Python 可见属性：格式名（如 `"Int8ub"`）。
    #[pyo3(get)]
    pub name: &'static str,
    /// Rust 内部：预编译的整数格式枚举（不暴露给 Python）。
    pub format: PythonFormat,
    /// Rust 内部：字节长度（等于 `format.byte_length()`，缓存避免重复计算）。
    pub length: usize,
}

impl FormatFieldDescriptor {
    /// 创建一个格式字段描述符（Rust 内部使用，用于构建模块级单例）。
    ///
    /// Python 用户不应直接构造此类——应使用预定义的 `Int8ub` 等模块级常量。
    ///
    /// # 参数
    ///
    /// - `name`：格式名（与 Python 侧的标识符一致，如 `"Int8ub"`）
    /// - `format`：预编译的整数格式
    pub fn new(name: &'static str, format: PythonFormat) -> Self {
        Self {
            name,
            length: format.byte_length(),
            format,
        }
    }
}

#[pymethods]
impl FormatFieldDescriptor {
    /// 返回格式的 Python 可读表示。
    fn __repr__(&self) -> String {
        format!("FormatFieldDescriptor({:?})", self.name)
    }
}

// ---------------------------------------------------------------------------
// BytesDescriptor
// ---------------------------------------------------------------------------

/// 可实例化字节描述符：对应 Python 侧的 `Bytes(n)`。
///
/// 用户在 Python 中通过 `Bytes(4)` 创建实例，传入 `field(Bytes(4))`。
/// 编译器读取 `length` 字段构建 [`crate::nodes::bytes::BytesNode`]。
///
/// # Phase 1
///
/// `length` 仅支持编译期常量正整数。
/// `Bytes(this.length)` 等上下文 lambda 推迟到 Phase 2（需表达式系统）。
#[pyclass(frozen, name = "BytesDescriptor", module = "construct")]
#[derive(Debug, Clone, Copy)]
pub struct BytesDescriptor {
    /// Python 可见属性：固定字节长度。
    #[pyo3(get)]
    pub length: usize,
}

#[pymethods]
impl BytesDescriptor {
    /// 创建一个读取/写入 `length` 字节的 `Bytes` 描述符。
    ///
    /// Python 用法：`Bytes(4)`
    #[new]
    fn new(length: usize) -> Self {
        Self { length }
    }

    /// 返回描述符的 Python 可读表示。
    fn __repr__(&self) -> String {
        format!("BytesDescriptor(length={})", self.length)
    }
}

// ---------------------------------------------------------------------------
// GreedyBytesDescriptor
// ---------------------------------------------------------------------------

/// 剩余字节描述符：对应 Python 侧的 `GreedyBytes` 预定义单例。
///
/// 在流中读取/写入所有剩余字节。编译器识别此类型后构建
/// [`crate::nodes::greedy_bytes::GreedyBytesNode`]。
#[pyclass(frozen, name = "GreedyBytesDescriptor", module = "construct")]
#[derive(Debug, Clone, Copy, Default)]
pub struct GreedyBytesDescriptor;

#[pymethods]
impl GreedyBytesDescriptor {
    /// 创建一个 `GreedyBytes` 描述符实例。
    ///
    /// Python 用户通常直接使用模块级常量 `GreedyBytes`，但也可通过
    /// `GreedyBytesDescriptor()` 构造等价实例。
    #[new]
    fn new() -> Self {
        Self
    }

    /// 返回描述符的 Python 可读表示。
    fn __repr__(&self) -> &'static str {
        "GreedyBytesDescriptor()"
    }
}

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
        F: for<'py> FnOnce(Python<'py>) -> R,
    {
        ensure_python();
        Python::with_gil(f)
    }

    // ======================================================================
    // FormatFieldDescriptor
    // ======================================================================

    #[test]
    fn format_descriptor_new_stores_name_and_format() {
        let desc = FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big);
        assert_eq!(desc.name, "Int8ub");
        assert_eq!(desc.format, PythonFormat::UnsignedInt8Big);
        assert_eq!(desc.length, 1);
    }

    #[test]
    fn format_descriptor_length_matches_byte_length() {
        let cases = [
            ("Int8ub", PythonFormat::UnsignedInt8Big, 1usize),
            ("Int16ub", PythonFormat::UnsignedInt16Big, 2),
            ("Int32ub", PythonFormat::UnsignedInt32Big, 4),
            ("Int64ub", PythonFormat::UnsignedInt64Big, 8),
        ];
        for (name, fmt, expected_len) in cases {
            let desc = FormatFieldDescriptor::new(name, fmt);
            assert_eq!(desc.length, expected_len, "length mismatch for {}", name);
            assert_eq!(desc.length, fmt.byte_length());
        }
    }

    #[test]
    fn format_descriptor_all_16_singletons_have_correct_format() {
        let singletons: &[(&'static str, PythonFormat)] = &[
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
        assert_eq!(
            singletons.len(),
            16,
            "expected 16 integer format singletons"
        );
        for (name, fmt) in singletons {
            let desc = FormatFieldDescriptor::new(name, *fmt);
            assert_eq!(desc.name, *name);
            assert_eq!(desc.format, *fmt);
        }
    }

    #[test]
    fn format_descriptor_is_frozen_pyclass_on_python_heap() {
        // 验证 FormatFieldDescriptor 能通过 Py::new 创建到 Python 堆，
        // 且 .name 属性可从 Python 侧读取。
        with_py(|py| {
            let desc = FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big);
            let desc_py = Py::new(py, desc).expect("Py::new");
            let name = desc_py
                .bind(py)
                .getattr("name")
                .expect("getattr name")
                .extract::<String>()
                .expect("extract name");
            assert_eq!(name, "Int8ub");
        });
    }

    #[test]
    fn format_descriptor_repr_is_readable() {
        let desc = FormatFieldDescriptor::new("Int16ub", PythonFormat::UnsignedInt16Big);
        assert_eq!(desc.__repr__(), r#"FormatFieldDescriptor("Int16ub")"#);
    }

    // ======================================================================
    // BytesDescriptor
    // ======================================================================

    #[test]
    fn bytes_descriptor_new_stores_length() {
        let desc = BytesDescriptor { length: 4 };
        assert_eq!(desc.length, 4);
    }

    #[test]
    fn bytes_descriptor_zero_length() {
        let desc = BytesDescriptor { length: 0 };
        assert_eq!(desc.length, 0);
    }

    #[test]
    fn bytes_descriptor_large_length() {
        let desc = BytesDescriptor { length: 65535 };
        assert_eq!(desc.length, 65535);
    }

    #[test]
    fn bytes_descriptor_length_accessible_from_python() {
        with_py(|py| {
            let desc = BytesDescriptor { length: 7 };
            let desc_py = Py::new(py, desc).expect("Py::new");
            let length: usize = desc_py
                .bind(py)
                .getattr("length")
                .expect("getattr")
                .extract()
                .expect("extract");
            assert_eq!(length, 7);
        });
    }

    #[test]
    fn bytes_descriptor_repr_includes_length() {
        let desc = BytesDescriptor { length: 4 };
        assert_eq!(desc.__repr__(), "BytesDescriptor(length=4)");
    }

    #[test]
    fn bytes_descriptor_instantiable_from_python() {
        // 验证 BytesDescriptor 可通过 Python 调用 BytesDescriptor(4) 构造
        with_py(|py| {
            // 先获取类型对象
            let desc = BytesDescriptor { length: 4 };
            let desc_py = Py::new(py, desc).expect("Py::new");
            let ty = desc_py.bind(py).get_type();
            // 用类型对象调用构造（args 需为元组）
            let instance = ty.call((1,), None).expect("call BytesDescriptor(1)");
            let length: usize = instance.getattr("length").unwrap().extract().unwrap();
            assert_eq!(length, 1);
        });
    }

    // ======================================================================
    // GreedyBytesDescriptor
    // ======================================================================

    #[test]
    fn greedy_bytes_descriptor_new_is_unit() {
        let _desc = GreedyBytesDescriptor;
        let _also = GreedyBytesDescriptor;
    }

    #[test]
    fn greedy_bytes_descriptor_repr() {
        let desc = GreedyBytesDescriptor;
        // repr 通过 #[pymethods] 定义，通过 Py 对象访问验证
        with_py(|py| {
            let desc_py = Py::new(py, desc).expect("Py::new");
            let r = desc_py.bind(py).repr().expect("repr").to_string();
            assert_eq!(r, "GreedyBytesDescriptor()");
        });
    }

    #[test]
    fn greedy_bytes_descriptor_on_python_heap() {
        with_py(|py| {
            let desc = GreedyBytesDescriptor::new();
            let desc_py = Py::new(py, desc).expect("Py::new");
            assert!(desc_py.bind(py).repr().is_ok());
        });
    }
}
