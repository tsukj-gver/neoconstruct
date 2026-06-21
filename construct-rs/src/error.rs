//! construct-rs 内部统一错误类型，以及向 pyo3 `PyErr` 的转换。
//!
//! 设计依据：`docs/架构设计.md` §B.8。
//!
//! ## 错误流转
//!
//! ```text
//! 节点方法签名:  fn parse(...) -> Result<T, ConstructError>     ← Rust 内部用结构化错误
//!                               ↓ impl From<ConstructError> for PyErr
//! FFI 入口签名:  fn _parse_raw(...) -> PyResult<PyDict>          ← 入口处转为 PyErr
//!                               ↓ pyo3 自动抛出
//! Python 侧:    except ConstructError as e: ...                  ← 用户捕获 Python 异常
//! ```
//!
//! ## Phase 1 简化
//!
//! 当前阶段所有 `ConstructError` 变体都映射为 `pyo3::exceptions::PyValueError`。
//! 子任务 1.7 会定义 Python 异常类层次（`ConstructError` 基类 + 各子类），
//! 届时此 `From` 实现会被替换为按变体映射到对应的 Python 异常类。

use pyo3::exceptions::PyValueError;
use pyo3::PyErr;

/// construct-rs 内部统一错误类型，携带结构化信息。
///
/// 每个变体的字段：
/// - `message`：人类可读的错误描述。
/// - `path`：错误发生的路径（如 `"root.header.flags"`），用于追踪嵌套结构中的出错字段。
///
/// `Compilation` 与 `UnresolvedReference` 不携带 `path`，因为它们发生在编译阶段，
/// 此时执行树尚未进入运行时路径追踪。
#[derive(Debug, thiserror::Error)]
pub enum ConstructError {
    /// 流错误：字节不足、流读/写失败等。
    ///
    /// 对应 Python construct 的 `StreamError`。
    #[error("stream error: {message} at {path}")]
    Stream {
        /// 错误详情（如 "stream read less than specified amount, expected 4, found 2"）。
        message: String,
        /// 错误发生的路径。
        path: String,
    },

    /// 格式字段错误：整数/浮点解析或构建失败。
    ///
    /// 对应 Python construct 的 `FormatFieldError`。
    #[error("format field error: {message} at {path}")]
    FormatField {
        /// 错误详情。
        message: String,
        /// 错误发生的路径。
        path: String,
    },

    /// 字段长度错误：`Bytes(n)` 的实际数据长度不匹配等。
    ///
    /// 对应 Python construct 中 `stream_write` 触发的长度不匹配错误。
    #[error("field length error: {message} at {path}")]
    FieldLength {
        /// 错误详情。
        message: String,
        /// 错误发生的路径。
        path: String,
    },

    /// 编译错误：schema 编译失败（未知描述符、字段重复等）。
    ///
    /// 对应 Python construct 在类创建阶段触发的错误。
    #[error("compilation error: {message}")]
    Compilation {
        /// 错误详情。
        message: String,
    },

    /// 未解析的类型引用：前向引用未在运行时解析成功。
    ///
    /// 对应 Python construct 中 `_construct_compiled` 为 `None` 的场景。
    #[error("unresolved type reference: {message}")]
    UnresolvedReference {
        /// 错误详情。
        message: String,
    },

    /// 通用错误：其他无法归类的错误。
    #[error("{message} at {path}")]
    Generic {
        /// 错误详情。
        message: String,
        /// 错误发生的路径。
        path: String,
    },
}

impl ConstructError {
    /// 返回错误的简短消息（不含 path 前缀）。
    ///
    /// 用于构造 Python 异常的 `message` 部分。
    pub fn message(&self) -> &str {
        match self {
            ConstructError::Stream { message, .. }
            | ConstructError::FormatField { message, .. }
            | ConstructError::FieldLength { message, .. }
            | ConstructError::Compilation { message }
            | ConstructError::UnresolvedReference { message }
            | ConstructError::Generic { message, .. } => message,
        }
    }

    /// 返回错误的路径（若存在）。
    ///
    /// 编译期错误（`Compilation`、`UnresolvedReference`）没有路径，返回 `None`。
    pub fn path(&self) -> Option<&str> {
        match self {
            ConstructError::Stream { path, .. }
            | ConstructError::FormatField { path, .. }
            | ConstructError::FieldLength { path, .. }
            | ConstructError::Generic { path, .. } => Some(path),
            ConstructError::Compilation { .. } | ConstructError::UnresolvedReference { .. } => None,
        }
    }

    /// 生成对齐 Python construct 的完整错误消息。
    ///
    /// Python construct 的 `ConstructError.__init__` 在 path 非 None 时构造：
    /// ```text
    /// "Error in path {path}\n{message}"
    /// ```
    /// 若无 path（编译期错误），仅返回 `{message}`。
    pub fn full_message(&self) -> String {
        match self.path() {
            Some(p) => format!("Error in path {}\n{}", p, self.message()),
            None => self.message().to_string(),
        }
    }

    /// 错误类别的简短标识，用于调试与日志。
    pub fn kind_str(&self) -> &'static str {
        match self {
            ConstructError::Stream { .. } => "Stream",
            ConstructError::FormatField { .. } => "FormatField",
            ConstructError::FieldLength { .. } => "FieldLength",
            ConstructError::Compilation { .. } => "Compilation",
            ConstructError::UnresolvedReference { .. } => "UnresolvedReference",
            ConstructError::Generic { .. } => "Generic",
        }
    }
}

/// 将 `ConstructError` 转换为 pyo3 的 `PyErr`。
///
/// ## Phase 1 简化
///
/// 当前所有变体统一映射为 `PyValueError`。子任务 1.7 会替换为完整的 Python 异常层次：
/// - `Stream` → `StreamError`
/// - `FormatField` → `FormatFieldError`
/// - 其他依此类推。
///
/// 无论映射到哪个 Python 异常类，错误消息的格式都是 `full_message()` 的输出，
/// 即 `"Error in path {path}\n{message}"`（与 Python construct 一致）。
impl From<ConstructError> for PyErr {
    fn from(err: ConstructError) -> Self {
        PyValueError::new_err(err.full_message())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_display_includes_message_and_path() {
        let err = ConstructError::Stream {
            message: "stream read less than specified amount, expected 4, found 2".to_string(),
            path: "root.header".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "stream error: stream read less than specified amount, expected 4, found 2 \
             at root.header"
        );
    }

    #[test]
    fn format_field_display_includes_message_and_path() {
        let err = ConstructError::FormatField {
            message: "struct '>I' error during parsing".to_string(),
            path: "root.value".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "format field error: struct '>I' error during parsing at root.value"
        );
    }

    #[test]
    fn field_length_display_includes_message_and_path() {
        let err = ConstructError::FieldLength {
            message: "bytes object of wrong length, expected 4, found 2".to_string(),
            path: "root.payload".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "field length error: bytes object of wrong length, expected 4, found 2 at root.payload"
        );
    }

    #[test]
    fn compilation_display_includes_message_only() {
        let err = ConstructError::Compilation {
            message: "unknown descriptor type at field 'foo'".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "compilation error: unknown descriptor type at field 'foo'"
        );
    }

    #[test]
    fn unresolved_reference_display_includes_message_only() {
        let err = ConstructError::UnresolvedReference {
            message: "class 'NodeB' is still a lazy stub".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "unresolved type reference: class 'NodeB' is still a lazy stub"
        );
    }

    #[test]
    fn generic_display_includes_message_and_path() {
        let err = ConstructError::Generic {
            message: "unexpected value type".to_string(),
            path: "root".to_string(),
        };
        assert_eq!(err.to_string(), "unexpected value type at root");
    }

    #[test]
    fn full_message_with_path_aligns_python_construct() {
        let err = ConstructError::Stream {
            message: "expected 4, found 2".to_string(),
            path: "root.header.flags".to_string(),
        };
        // Python construct: "Error in path {}\n".format(path) + message
        assert_eq!(
            err.full_message(),
            "Error in path root.header.flags\nexpected 4, found 2"
        );
    }

    #[test]
    fn full_message_without_path_returns_message_only() {
        let err = ConstructError::Compilation {
            message: "schema is malformed".to_string(),
        };
        assert_eq!(err.full_message(), "schema is malformed");
    }

    #[test]
    fn path_returns_some_for_runtime_errors() {
        let stream_err = ConstructError::Stream {
            message: "x".to_string(),
            path: "root".to_string(),
        };
        assert_eq!(stream_err.path(), Some("root"));

        let generic_err = ConstructError::Generic {
            message: "x".to_string(),
            path: "root.a".to_string(),
        };
        assert_eq!(generic_err.path(), Some("root.a"));
    }

    #[test]
    fn path_returns_none_for_compile_time_errors() {
        let compilation_err = ConstructError::Compilation {
            message: "x".to_string(),
        };
        assert_eq!(compilation_err.path(), None);

        let unresolved_err = ConstructError::UnresolvedReference {
            message: "x".to_string(),
        };
        assert_eq!(unresolved_err.path(), None);
    }

    #[test]
    fn kind_str_returns_correct_identifier() {
        assert_eq!(
            ConstructError::Stream {
                message: "x".to_string(),
                path: "p".to_string()
            }
            .kind_str(),
            "Stream"
        );
        assert_eq!(
            ConstructError::FormatField {
                message: "x".to_string(),
                path: "p".to_string()
            }
            .kind_str(),
            "FormatField"
        );
        assert_eq!(
            ConstructError::FieldLength {
                message: "x".to_string(),
                path: "p".to_string()
            }
            .kind_str(),
            "FieldLength"
        );
        assert_eq!(
            ConstructError::Compilation {
                message: "x".to_string()
            }
            .kind_str(),
            "Compilation"
        );
        assert_eq!(
            ConstructError::UnresolvedReference {
                message: "x".to_string()
            }
            .kind_str(),
            "UnresolvedReference"
        );
        assert_eq!(
            ConstructError::Generic {
                message: "x".to_string(),
                path: "p".to_string()
            }
            .kind_str(),
            "Generic"
        );
    }

    #[test]
    fn from_construct_error_to_pyerr_yields_value_error_with_full_message() {
        use pyo3::prelude::*;
        use std::sync::Once;

        // 测试需要 Python 解释器（PyValueError 实例化涉及 C API）。
        static INIT: Once = Once::new();
        INIT.call_once(pyo3::prepare_freethreaded_python);

        let err = ConstructError::Stream {
            message: "expected 4, found 2".to_string(),
            path: "root.header".to_string(),
        };
        let pyerr: PyErr = err.into();
        Python::with_gil(|py| {
            // Phase 1: 所有变体映射为 PyValueError。
            assert!(pyerr.is_instance_of::<PyValueError>(py));
        });
        // PyErr 的 to_string 应包含完整消息。
        let s = format!("{}", pyerr);
        assert!(s.contains("Error in path root.header"), "got: {}", s);
        assert!(s.contains("expected 4, found 2"), "got: {}", s);
    }

    #[test]
    fn from_compilation_error_to_pyerr_has_no_path_prefix() {
        use pyo3::prelude::*;
        use std::sync::Once;

        static INIT: Once = Once::new();
        INIT.call_once(pyo3::prepare_freethreaded_python);

        let err = ConstructError::Compilation {
            message: "unknown descriptor 'Foo'".to_string(),
        };
        let pyerr: PyErr = err.into();
        let s = format!("{}", pyerr);
        assert!(!s.contains("Error in path"), "got: {}", s);
        assert!(s.contains("unknown descriptor 'Foo'"), "got: {}", s);
    }
}
