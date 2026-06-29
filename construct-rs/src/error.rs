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
//! Python 侧:    except StreamError as e: ...                    ← 用户捕获 Python 异常
//! ```
//!
//! ## 异常类映射（子任务 1.8）
//!
//! Python 异常类定义在 `construct._errors`（纯 Python 单一定义），Rust 侧在模块
//! 初始化时（[`crate::_construct_rust`]）通过 [`init_exception_classes`] 缓存这些
//! 类的引用到全局 [`EXCEPTIONS`]。`From<ConstructError> for PyErr` 根据变体选择
//! 对应的 Python 异常类，确保 `except StreamError` 等能精确捕获。
//!
//! 缓存未初始化时（如单元测试中未走过模块初始化），`From` 回退到 `PyValueError`，
//! 携带 `full_message()` 输出——保证错误信息不丢失，仅类型降级。

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::sync::GILOnceCell;
use pyo3::types::PyType;

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

    // --- 表达式求值错误（Phase 2，§4.5）---
    // SF-2 修复：所有 Expr* 变体携带 `path` 字段，支持 `push_path_segment`
    // 重建路径，与 Stream/FormatField/Generic 等变体行为一致。
    /// 表达式求值：字段值类型不匹配（期望 i64，实际为 str/bytes 等）。
    ///
    /// 触发场景：[`crate::expr::ExprOp::GetInt`] 从 context 取到的值无法 extract 为 `i64`。
    #[error("expression field {field:?} has wrong type, expected {expected} at {path}")]
    ExprType {
        /// 出错字段名。
        field: String,
        /// 期望的类型描述（如 `"integer (i64)"`）。
        expected: String,
        /// 错误发生的路径。
        path: String,
    },

    /// 表达式求值：引用的字段不存在于 context 中。
    ///
    /// 触发场景：[`crate::expr::ExprOp::GetInt`] 取值时 `PyDict::get_item` 返回 `None`。
    /// 编译期应通过字段存在性检查避免，运行时触发属于内部错误。
    #[error("expression field {field:?} missing in context at {path}")]
    ExprFieldMissing {
        /// 缺失的字段名。
        field: String,
        /// 错误发生的路径。
        path: String,
    },

    /// 表达式求值：context 为 placeholder（无 PyDict），无法求值。
    ///
    /// 触发场景：在 [`crate::context::Context::placeholder`] 上调用表达式求值。
    /// 理论上不会发生——有表达式的 Struct 使用 [`crate::context::Context::new_root`]，
    /// 而非 placeholder。
    #[error("expression context error: {message} at {path}")]
    ExprContext {
        /// 错误详情。
        message: String,
        /// 错误发生的路径。
        path: String,
    },

    /// 表达式求值：除以零。
    ///
    /// 触发场景：[`crate::expr::ExprOp::FloorDiv`] 或 [`crate::expr::ExprOp::Mod`]
    /// 的除数（栈顶值）为 0。
    #[error("expression error: {message} at {path}")]
    ExprDivByZero {
        /// 错误详情（如 `"expression division by zero"`）。
        message: String,
        /// 错误发生的路径。
        path: String,
    },

    /// 表达式求值：栈下溢（指令序列不合法，编译期应保证不发生）。
    ///
    /// 触发场景：[`crate::expr::eval_expr_int`] 内 `pop2` 或 `pop` 时栈为空。
    /// 仅作为防御性错误，正常路径不应触发。
    #[error("expression stack underflow at {path}")]
    ExprStackUnderflow {
        /// 错误发生的路径。
        path: String,
    },

    /// bit 域对齐错误：`BitwiseNode` 结束时 bit 游标未回到入口值
    /// （消耗的 bit 数非 8 倍数），或 `BitwiseNode::sizeof` 的 inner sizeof 非 8 倍数。
    ///
    /// 对应 Python construct 的 `RestreamedBytesIO.close()` 缓冲清空校验
    /// （`lib/bitstream.py` L50-54）：Bitwise 的 Restreamed 用 decoderunit=1 byte /
    /// encoderunit=8 bits（`core.py` L1068），close() 检查 rbuffer/wbuffer 为空
    /// 等价于"消耗 bit 数为 8 倍数"。
    ///
    /// Python 无独立 `BitFieldError`，复用 `StreamError` 映射（§7.3）。
    #[error("bit field error: {message} at {path}")]
    BitField {
        /// 错误详情。
        message: String,
        /// 错误发生的路径。
        path: String,
    },

    /// bit 级整数错误：`BitsInteger` 节点的运行时错误（非整数输入、超出范围、
    /// length<=0、swapped+%8!=0 等）。
    ///
    /// 对应 Python construct 的 `IntegerError`（`core.py` L49、L1367/L1374/L1378/L1387）。
    ///
    /// Phase 3.3 新增：将原 BitsInteger 节点使用 `FormatField` 变体的实现统一改为
    /// `Integer`，使用户可通过 `except IntegerError` 精确捕获（3.1 VET P1 修复）。
    #[error("integer error: {message} at {path}")]
    Integer {
        /// 错误详情。
        message: String,
        /// 错误发生的路径。
        path: String,
    },

    /// 填充错误：`Padding` 字段的 pattern 非法（bit 域仅接受 `0x00` / `0x01`）等。
    ///
    /// 对应 Python construct 的 `PaddingError`（`core.py` L124、L4146）。
    ///
    /// Phase 3.3 新增：`BitPaddingNode::new` 在编译期对非法 pattern 返回此错误，
    /// 比 Python 在 `bits2bytes` 阶段延迟 `KeyError` 更早暴露（设计 §4.2 P1 修复）。
    /// 字节级 `PaddingNode` 的负长度等错误也归此类。
    #[error("padding error: {message} at {path}")]
    Padding {
        /// 错误详情。
        message: String,
        /// 错误发生的路径。
        path: String,
    },

    // --- Phase 4 Array 系列错误（设计 §3.4）---
    /// Array count 无效（负数或与给定列表长度不符）。
    ///
    /// 对应 Python construct 的 `RangeError`（`core.py` L2528、L2541、L2543）。
    ///
    /// 触发场景：
    /// - Array count 表达式求值为负数
    /// - Array build 时 `len(obj) != count`
    /// - PrefixedArray countfield 解析得到负数
    #[error("range error: {message} at {path}")]
    Range {
        /// 错误详情。
        message: String,
        /// 错误发生的路径。
        path: String,
    },

    /// RepeatUntil build 时无元素满足谓词。
    ///
    /// 对应 Python construct 的 `RepeatError`（`core.py` L2700）。
    #[error("repeat error: {message} at {path}")]
    Repeat {
        /// 错误详情。
        message: String,
        /// 错误发生的路径。
        path: String,
    },

    /// 早停信号（仅 `StopIfNode` 产生，`GreedyRangeNode` / `StructNode` 捕获）。
    ///
    /// 对应 Python construct 的 `StopFieldError`（`core.py` L4106）。
    ///
    /// 作为 `Result` 的哨兵变体而非 panic，保证 `Construct` trait 签名不变
    /// （设计 §2.4 决策 A4）。正常路径由 `GreedyRangeNode` 等捕获，不应到达 FFI 入口。
    #[error("stop field signal at {path}")]
    StopField {
        /// 错误发生的路径。
        path: String,
    },

    /// Index 节点读取 `_index` 时上下文未提供（理论上不发生——Array 都会设置）。
    ///
    /// 对应 Python construct 的 `IndexFieldError`。
    ///
    /// 当前设计 `IndexNode` 在 `_index` 为 `None` 时返回 `Py_None`（对齐 Python
    /// `context.get("_index", None)`），不主动触发此变体。保留供未来严格模式使用
    /// （设计 §3.4.4）。
    #[error("index field error: {message} at {path}")]
    IndexField {
        /// 错误详情。
        message: String,
        /// 错误发生的路径。
        path: String,
    },
}

impl ConstructError {
    /// 返回错误的简短消息（不含 path 前缀），若存在。
    ///
    /// 用于构造 Python 异常的 `message` 部分。返回 `Option`：
    /// - `Some(message)`：变体有单一 `message` 字段。
    /// - `None`：结构化变体（`ExprType` / `ExprFieldMissing` / `ExprStackUnderflow`），
    ///   无单一 message 字段，调用方应改用 `Display`（`to_string` / `full_message`）。
    pub fn message(&self) -> Option<&str> {
        match self {
            ConstructError::Stream { message, .. }
            | ConstructError::FormatField { message, .. }
            | ConstructError::FieldLength { message, .. }
            | ConstructError::Compilation { message }
            | ConstructError::UnresolvedReference { message }
            | ConstructError::Generic { message, .. }
            | ConstructError::ExprContext { message, .. }
            | ConstructError::ExprDivByZero { message, .. }
            | ConstructError::BitField { message, .. }
            | ConstructError::Integer { message, .. }
            | ConstructError::Padding { message, .. }
            | ConstructError::Range { message, .. }
            | ConstructError::Repeat { message, .. }
            | ConstructError::IndexField { message, .. } => Some(message),
            // 这些变体没有单一 message 字段，完整错误信息通过 to_string() / full_message() 获取。
            ConstructError::ExprType { .. }
            | ConstructError::ExprFieldMissing { .. }
            | ConstructError::ExprStackUnderflow { .. }
            | ConstructError::StopField { .. } => None,
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
            | ConstructError::Generic { path, .. }
            | ConstructError::ExprType { path, .. }
            | ConstructError::ExprFieldMissing { path, .. }
            | ConstructError::ExprContext { path, .. }
            | ConstructError::ExprDivByZero { path, .. }
            | ConstructError::ExprStackUnderflow { path }
            | ConstructError::BitField { path, .. }
            | ConstructError::Integer { path, .. }
            | ConstructError::Padding { path, .. }
            | ConstructError::Range { path, .. }
            | ConstructError::Repeat { path, .. }
            | ConstructError::StopField { path }
            | ConstructError::IndexField { path, .. } => Some(path),
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
    ///
    /// 对于无单一 `message` 字段的结构化变体（`ExprType` / `ExprFieldMissing` /
    /// `ExprStackUnderflow`），使用 `Display` 实现作为完整消息。
    pub fn full_message(&self) -> String {
        match self {
            // 结构化变体：无单一 message 字段，Display 已包含完整信息（含 path）。
            ConstructError::ExprType { .. }
            | ConstructError::ExprFieldMissing { .. }
            | ConstructError::ExprStackUnderflow { .. }
            | ConstructError::StopField { .. } => self.to_string(),
            // 其他变体：按 path 拼接。此分支的变体均有 message 字段（message() 返回 Some）。
            _ => match self.path() {
                Some(p) => format!("Error in path {}\n{}", p, self.message().unwrap_or("")),
                None => self.message().unwrap_or("").to_string(),
            },
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
            ConstructError::ExprType { .. } => "ExprType",
            ConstructError::ExprFieldMissing { .. } => "ExprFieldMissing",
            ConstructError::ExprContext { .. } => "ExprContext",
            ConstructError::ExprDivByZero { .. } => "ExprDivByZero",
            ConstructError::ExprStackUnderflow { .. } => "ExprStackUnderflow",
            ConstructError::BitField { .. } => "BitField",
            ConstructError::Integer { .. } => "Integer",
            ConstructError::Padding { .. } => "Padding",
            ConstructError::Range { .. } => "Range",
            ConstructError::Repeat { .. } => "Repeat",
            ConstructError::StopField { .. } => "StopField",
            ConstructError::IndexField { .. } => "IndexField",
        }
    }

    /// 在错误路径中插入一个字段段（P0-3 优化：错误路径延迟构建路径）。
    ///
    /// 成功路径不再维护 `Path` 栈（无 `push_field`/`pop`），因此叶节点产生的
    /// 错误路径只含根（`"root"`）。父 `StructNode` 在子节点返回 `Err` 时调用此方法，
    /// 将自己的字段名插入到 `"root"` 之后，重建完整路径。
    ///
    /// # 路径重建规则
    ///
    /// - 错误路径为 `None` / `""` / `"root"`（叶节点基线）→ `"root.{segment}"`。
    /// - 错误路径形如 `"root.x.y"`（内层 StructNode 已重建）→
    ///   `"root.{segment}.x.y"`（segment 插入 root 之后）。
    /// - 编译期错误（无 path）→ 不变。
    ///
    /// 仅在错误路径调用（成功路径零成本），开销可接受。
    pub fn push_path_segment(&mut self, segment: &str) {
        // 编译期错误（Compilation / UnresolvedReference）无 path 字段，
        // set_path 对其是 no-op。提前返回，跳过无用的 format! 路径计算。
        if self.path().is_none() {
            return;
        }
        let new_path = match self.path() {
            Some(p) if p.is_empty() || p == "root" => format!("root.{}", segment),
            Some(p) => {
                if let Some(suffix) = p.strip_prefix("root") {
                    // suffix 为 ""（"root"）或 ".x.y"（"root.x.y"）。
                    format!("root.{}{}", segment, suffix)
                } else {
                    // 非标准根，退化为追加（仅防御性，正常路径不触发）。
                    format!("{}.{}", p, segment)
                }
            }
            None => format!("root.{}", segment),
        };
        self.set_path(new_path);
    }

    /// 设置错误路径（内部辅助，仅供 [`push_path_segment`] 使用）。
    ///
    /// 编译期错误（`Compilation` / `UnresolvedReference`）无 path 字段，此方法对其无操作。
    ///
    /// [`push_path_segment`]: ConstructError::push_path_segment
    fn set_path(&mut self, new_path: String) {
        match self {
            ConstructError::Stream { path, .. }
            | ConstructError::FormatField { path, .. }
            | ConstructError::FieldLength { path, .. }
            | ConstructError::Generic { path, .. }
            | ConstructError::ExprType { path, .. }
            | ConstructError::ExprFieldMissing { path, .. }
            | ConstructError::ExprContext { path, .. }
            | ConstructError::ExprDivByZero { path, .. }
            | ConstructError::ExprStackUnderflow { path }
            | ConstructError::BitField { path, .. }
            | ConstructError::Integer { path, .. }
            | ConstructError::Padding { path, .. }
            | ConstructError::Range { path, .. }
            | ConstructError::Repeat { path, .. }
            | ConstructError::StopField { path }
            | ConstructError::IndexField { path, .. } => *path = new_path,
            ConstructError::Compilation { .. } | ConstructError::UnresolvedReference { .. } => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Python 异常类缓存（子任务 1.8）
// ---------------------------------------------------------------------------

/// 全局缓存的 Python 异常类引用。
///
/// 在模块初始化（[`init_exception_classes`]）时一次性填充，之后
/// [`From<ConstructError> for PyErr`] 通过此缓存查找类。
///
/// 缓存未初始化时（如单元测试中），[`From`] 实现回退到 [`PyValueError`]，
/// 错误消息不丢失（仍包含 path）。
///
/// 字段全部为 `Py<PyType>`（Python 异常类的引用），通过 GIL 安全访问。
struct ExceptionClasses {
    /// 对应 `ConstructError::Stream`。
    stream_error: Py<PyType>,
    /// 对应 `ConstructError::FormatField`。
    format_field_error: Py<PyType>,
    /// 对应 `ConstructError::FieldLength`。
    field_length_error: Py<PyType>,
    /// 对应 `ConstructError::Compilation`。
    compilation_error: Py<PyType>,
    /// 对应 `ConstructError::UnresolvedReference`。
    unresolved_reference_error: Py<PyType>,
    /// 对应 `ConstructError::Generic`。
    generic_construct_error: Py<PyType>,
    /// `ConstructError` 基类，用于 Phase 2 新增的 Expr* 变体
    /// （ExprType / ExprFieldMissing / ExprContext / ExprDivByZero / ExprStackUnderflow）。
    ///
    /// Phase 2 暂无专门的 ExprError Python 类，统一映射到基类。
    /// 后续子任务可增加专门的 ExprError 类并在此缓存。
    construct_error_base: Py<PyType>,
    /// 对应 `ConstructError::Integer`（Phase 3.3）。
    /// Python construct 的 `IntegerError`，用于 BitsInteger 节点的运行时错误。
    integer_error: Py<PyType>,
    /// 对应 `ConstructError::Padding`（Phase 3.3）。
    /// Python construct 的 `PaddingError`，用于 Padding 字段的非法 pattern 等。
    padding_error: Py<PyType>,
    /// 对应 `ConstructError::Range`（Phase 4）。
    /// Python construct 的 `RangeError`，用于 Array count 无效等。
    range_error: Py<PyType>,
    /// 对应 `ConstructError::Repeat`（Phase 4）。
    /// Python construct 的 `RepeatError`，用于 RepeatUntil build 无元素满足谓词。
    repeat_error: Py<PyType>,
    /// 对应 `ConstructError::StopField`（Phase 4）。
    /// Python construct 的 `StopFieldError`，StopIf 早停信号。
    stop_field_error: Py<PyType>,
    /// 对应 `ConstructError::IndexField`（Phase 4）。
    /// Python construct 的 `IndexFieldError`，Index 节点 _index 缺失（保留）。
    index_field_error: Py<PyType>,
}

/// 全局 Python 异常类缓存。
///
/// 使用 [`GILOnceCell`] 保证：
/// - 写入需要 GIL（仅在模块初始化时调用 [`init_exception_classes`]）
/// - 读取需要 GIL（pyo3 的 `Python::with_gil` 在 [`From`] 实现中获取）
/// - 一旦写入，永不变更（异常类是模块级单例）
static EXCEPTIONS: GILOnceCell<ExceptionClasses> = GILOnceCell::new();

/// 在模块初始化时调用，从 `construct._errors` 缓存 Python 异常类引用。
///
/// 必须在 [`crate::_construct_rust`] 模块初始化函数中调用一次。多次调用幂等
/// （后续调用因已初始化而立即返回 `Ok(())`）。
///
/// 调用时机：`construct._errors` 已被 Python 侧 `construct/__init__.py` 导入
/// （在 `_construct_rust` 之前），因此可直接 `import`。
///
/// # 错误
///
/// - `construct._errors` 模块不存在（环境异常，理论上不应发生）。
/// - 模块中缺少预期的异常类（开发期错误）。
pub fn init_exception_classes(py: Python<'_>) -> PyResult<()> {
    if EXCEPTIONS.get(py).is_some() {
        return Ok(());
    }

    let errors_module = py.import_bound("construct._errors")?;
    let get = |name: &str| -> PyResult<Py<PyType>> {
        errors_module
            .getattr(name)
            .and_then(|attr| attr.extract::<Py<PyType>>())
    };

    let classes = ExceptionClasses {
        stream_error: get("StreamError")?,
        format_field_error: get("FormatFieldError")?,
        field_length_error: get("FieldLengthError")?,
        compilation_error: get("CompilationError")?,
        unresolved_reference_error: get("UnresolvedReferenceError")?,
        generic_construct_error: get("GenericConstructError")?,
        construct_error_base: get("ConstructError")?,
        integer_error: get("IntegerError")?,
        padding_error: get("PaddingError")?,
        range_error: get("RangeError")?,
        repeat_error: get("RepeatError")?,
        stop_field_error: get("StopFieldError")?,
        index_field_error: get("IndexFieldError")?,
    };

    // GILOnceCell::set 在已初始化时返回 Err(value)。由于前面已检查，这里应成功；
    // 即使并发竞争（不会发生，因为模块初始化持有 GIL），也安全降级。
    let _ = EXCEPTIONS.set(py, classes);
    Ok(())
}

/// 根据变体选择对应的 Python 异常类引用。
///
/// 不存在对应类时（如未来新增变体），返回 `None`，调用方应回退到
/// [`PyValueError`] 或 [`construct_error`](ExceptionClasses::construct_error) 基类。
fn select_exception_class<'py>(
    err: &ConstructError,
    classes: &'py ExceptionClasses,
    py: Python<'py>,
) -> Bound<'py, PyType> {
    let cls: &Py<PyType> = match err {
        ConstructError::Stream { .. } => &classes.stream_error,
        ConstructError::FormatField { .. } => &classes.format_field_error,
        ConstructError::FieldLength { .. } => &classes.field_length_error,
        ConstructError::Compilation { .. } => &classes.compilation_error,
        ConstructError::UnresolvedReference { .. } => &classes.unresolved_reference_error,
        ConstructError::Generic { .. } => &classes.generic_construct_error,
        // Phase 2 Expr* 变体暂无专门的 Python 异常类，统一映射到基类 ConstructError。
        ConstructError::ExprType { .. }
        | ConstructError::ExprFieldMissing { .. }
        | ConstructError::ExprContext { .. }
        | ConstructError::ExprDivByZero { .. }
        | ConstructError::ExprStackUnderflow { .. } => &classes.construct_error_base,
        // Phase 3.2 BitField：Python construct 无独立 BitFieldError，
        // 设计 §7.3 规定复用 StreamError（与 Python RestreamedBytesIO.close 缓冲
        // 清空校验的 StreamError 对齐）。
        ConstructError::BitField { .. } => &classes.stream_error,
        // Phase 3.3：BitsInteger 运行时错误映射到 Python IntegerError，
        // 与 Python construct 的 IntegerError 对齐（core.py L49/L1367 等）。
        ConstructError::Integer { .. } => &classes.integer_error,
        // Phase 3.3：Padding 错误映射到 Python PaddingError（core.py L124/L4146）。
        ConstructError::Padding { .. } => &classes.padding_error,
        // Phase 4 Array 系列（设计 §3.4.5）。
        ConstructError::Range { .. } => &classes.range_error,
        ConstructError::Repeat { .. } => &classes.repeat_error,
        ConstructError::StopField { .. } => &classes.stop_field_error,
        ConstructError::IndexField { .. } => &classes.index_field_error,
    };
    cls.bind(py).clone()
}

/// 构造一个携带 `message` 与可选 `path` 的 Python 异常实例。
///
/// 对齐 `construct._errors.ConstructError.__init__(message, path=None)` 的签名。
///
/// 返回 `Bound<PyAny>`（pyo3 0.22 API），调用方通过 [`PyErr::from_value_bound`]
/// 转换为 `PyErr`。
fn build_exception_instance<'py>(
    py: Python<'py>,
    cls: &Bound<'py, PyType>,
    message: &str,
    path: Option<&str>,
) -> PyResult<Bound<'py, PyAny>> {
    let _ = py;
    match path {
        Some(p) => cls.call1((message, p)),
        None => cls.call1((message,)),
    }
}

/// 将 `ConstructError` 转换为 pyo3 的 `PyErr`。
///
/// ## 映射规则
///
/// - 若 [`EXCEPTIONS`] 已通过 [`init_exception_classes`] 初始化：按变体映射到
///   对应的 Python 异常类（`Stream` → `StreamError`，依此类推）。
/// - 否则（如单元测试）：回退到 [`PyValueError`]，错误消息为 [`ConstructError::full_message`]。
///
/// 无论映射到哪个 Python 异常类，错误消息的格式都与 `construct._errors.ConstructError`
/// 一致：path 非 None 时为 `"Error in path {path}\n{message}"`，否则为 `{message}`。
///
/// ## GIL 获取
///
/// `From` trait 不持有 GIL，因此内部通过 [`Python::with_gil`] 获取。仅在错误路径
/// 触发（成功路径零成本），开销可接受（~微秒级）。
impl From<ConstructError> for PyErr {
    fn from(err: ConstructError) -> Self {
        // 预先提取 message/path（避免在 with_gil 闭包中持有 err 的引用）。
        let path_owned: Option<String> = err.path().map(|s| s.to_string());
        // message() 返回 Option：None 表示结构化变体（ExprType / ExprFieldMissing /
        // ExprStackUnderflow），此时使用 Display（to_string）作为完整消息（已包含 path），
        // 且不再单独传递 path（避免重复）。
        let (message, effective_path): (String, Option<&str>) = match err.message() {
            Some(msg) => (msg.to_string(), path_owned.as_deref()),
            None => (err.to_string(), None),
        };

        Python::with_gil(|py| match EXCEPTIONS.get(py) {
            Some(classes) => {
                let cls = select_exception_class(&err, classes, py);
                match build_exception_instance(py, &cls, &message, effective_path) {
                    Ok(instance) => PyErr::from_value_bound(instance),
                    // fallback：异常实例构造失败，回退到 PyValueError + full_message。
                    Err(_) => PyValueError::new_err(err.full_message()),
                }
            }
            None => PyValueError::new_err(err.full_message()),
        })
    }
}

/// 将 pyo3 的 `PyErr` 转换为 [`ConstructError`]（P0-4 优化）。
///
/// 使 `dict.set_item(...)?` 等返回 `PyResult` 的 C API 调用能通过 `?` 直接传播，
/// 无需在每个调用点写 `map_err` 闭包。错误上下文（字段名、路径）由上层
/// `StructNode` 通过 [`ConstructError::push_path_segment`] 统一补充。
///
/// 对齐 pydantic-core 的 `ValResult: From<PyErr>`（model_fields.rs:363）。
impl From<PyErr> for ConstructError {
    fn from(e: PyErr) -> Self {
        ConstructError::Generic {
            message: e.to_string(),
            path: String::new(),
        }
    }
}

// 防止未来重构时引入 fmt 误用。

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

    /// 初始化 Python 解释器（幂等），供需要 C API 的测试使用。
    fn ensure_python() {
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(pyo3::prepare_freethreaded_python);
    }

    #[test]
    fn from_construct_error_falls_back_to_value_error_when_uninitialized() {
        // 不调用 init_exception_classes，EXCEPTIONS 应未初始化（除非其他测试已污染，
        // GILOnceCell 一旦设置不可重置，因此本测试仅在 EXCEPTIONS 未设置时严格）。
        ensure_python();

        let err = ConstructError::Stream {
            message: "expected 4, found 2".to_string(),
            path: "root.header".to_string(),
        };
        let pyerr: PyErr = err.into();

        Python::with_gil(|py| {
            // 若 EXCEPTIONS 已被其他测试初始化，则不再是 PyValueError。
            // 此测试仅在 EXCEPTIONS 未初始化时严格断言 PyValueError；
            // 否则跳过类型断言（但仍验证消息）。
            if EXCEPTIONS.get(py).is_none() {
                assert!(
                    pyerr.is_instance_of::<PyValueError>(py),
                    "fallback should be PyValueError"
                );
            }
        });

        let s = format!("{}", pyerr);
        assert!(s.contains("Error in path root.header"), "got: {}", s);
        assert!(s.contains("expected 4, found 2"), "got: {}", s);
    }

    #[test]
    fn from_compilation_error_to_pyerr_has_no_path_prefix() {
        ensure_python();

        let err = ConstructError::Compilation {
            message: "unknown descriptor 'Foo'".to_string(),
        };
        let pyerr: PyErr = err.into();
        let s = format!("{}", pyerr);
        assert!(!s.contains("Error in path"), "got: {}", s);
        assert!(s.contains("unknown descriptor 'Foo'"), "got: {}", s);
    }

    /// 辅助：在测试中手动构建 ExceptionClasses（不依赖 construct 包安装）。
    ///
    /// 在 Python 中定义最小异常层次，提取类引用。使用 `run_bound` 执行多语句
    /// 类定义（`eval_bound` 仅支持表达式），随后通过 `eval_bound` 提取单个类。
    fn build_test_classes(py: Python<'_>) -> ExceptionClasses {
        let code = concat!(
            "class ConstructError(Exception):\n",
            "    def __init__(self, message='', path=None):\n",
            "        self.message = message\n",
            "        self.path = path\n",
            "        super().__init__(message)\n",
            "class StreamError(ConstructError): pass\n",
            "class FormatFieldError(ConstructError): pass\n",
            "class FieldLengthError(ConstructError): pass\n",
            "class CompilationError(ConstructError): pass\n",
            "class UnresolvedReferenceError(ConstructError): pass\n",
            "class GenericConstructError(ConstructError): pass\n",
            "class IntegerError(ConstructError): pass\n",
            "class PaddingError(ConstructError): pass\n",
            "class RangeError(ConstructError): pass\n",
            "class RepeatError(ConstructError): pass\n",
            "class StopFieldError(ConstructError): pass\n",
            "class IndexFieldError(ConstructError): pass\n",
        );
        py.run_bound(code, None, None)
            .expect("run test classes definition");

        let get = |name: &str| -> Py<PyType> {
            py.eval_bound(name, None, None)
                .unwrap_or_else(|e| panic!("get {}: {}", name, e))
                .extract::<Py<PyType>>()
                .expect("extract PyType")
        };

        ExceptionClasses {
            stream_error: get("StreamError"),
            format_field_error: get("FormatFieldError"),
            field_length_error: get("FieldLengthError"),
            compilation_error: get("CompilationError"),
            unresolved_reference_error: get("UnresolvedReferenceError"),
            generic_construct_error: get("GenericConstructError"),
            construct_error_base: get("ConstructError"),
            integer_error: get("IntegerError"),
            padding_error: get("PaddingError"),
            range_error: get("RangeError"),
            repeat_error: get("RepeatError"),
            stop_field_error: get("StopFieldError"),
            index_field_error: get("IndexFieldError"),
        }
    }

    #[test]
    fn select_exception_class_returns_stream_error_for_stream_variant() {
        ensure_python();
        Python::with_gil(|py| {
            let classes = build_test_classes(py);
            let err = ConstructError::Stream {
                message: "x".to_string(),
                path: "p".to_string(),
            };
            let cls = select_exception_class(&err, &classes, py);
            assert!(cls.is(&classes.stream_error));
        });
    }

    #[test]
    fn select_exception_class_returns_compilation_for_compilation_variant() {
        ensure_python();
        Python::with_gil(|py| {
            let classes = build_test_classes(py);
            let err = ConstructError::Compilation {
                message: "x".to_string(),
            };
            let cls = select_exception_class(&err, &classes, py);
            assert!(cls.is(&classes.compilation_error));
        });
    }

    #[test]
    fn build_exception_instance_passes_message_and_path() {
        ensure_python();
        Python::with_gil(|py| {
            let classes = build_test_classes(py);
            let cls = classes.stream_error.bind(py).clone();
            let instance =
                build_exception_instance(py, &cls, "msg", Some("root.x")).expect("build instance");

            // 验证属性被设置（与 _errors.py 行为一致）。
            let message_attr: String = instance
                .getattr("message")
                .expect("get message")
                .extract()
                .expect("extract message");
            let path_attr: String = instance
                .getattr("path")
                .expect("get path")
                .extract()
                .expect("extract path");
            assert_eq!(message_attr, "msg");
            assert_eq!(path_attr, "root.x");
        });
    }

    #[test]
    fn build_exception_instance_accepts_no_path() {
        ensure_python();
        Python::with_gil(|py| {
            let classes = build_test_classes(py);
            let cls = classes.compilation_error.bind(py).clone();
            let instance =
                build_exception_instance(py, &cls, "compilation failed", None).expect("build");

            let path_attr = instance.getattr("path").expect("get path");
            assert!(path_attr.is_none(), "path should default to None");
        });
    }

    #[test]
    fn pyerr_from_construct_error_uses_test_classes_when_set() {
        ensure_python();
        Python::with_gil(|py| {
            // 直接测试 select + build 路径（不污染全局 EXCEPTIONS）。
            let classes = build_test_classes(py);
            let err = ConstructError::Stream {
                message: "boom".to_string(),
                path: "root.x".to_string(),
            };
            let cls = select_exception_class(&err, &classes, py);
            let instance =
                build_exception_instance(py, &cls, "boom", Some("root.x")).expect("build");
            let pyerr = PyErr::from_value_bound(instance);

            assert!(
                pyerr.is_instance_bound(py, classes.stream_error.bind(py)),
                "should be StreamError"
            );
            assert!(
                !pyerr.is_instance_bound(py, classes.format_field_error.bind(py)),
                "should not be FormatFieldError"
            );
        });
    }

    // ======================================================================
    // push_path_segment（P0-3：错误路径延迟重建）
    // ======================================================================

    #[test]
    fn push_path_segment_on_root_base() {
        // 叶节点错误路径为 "root"，父 StructNode 插入字段名。
        let mut err = ConstructError::Stream {
            message: "expected 4".to_string(),
            path: "root".to_string(),
        };
        err.push_path_segment("b");
        assert_eq!(err.path(), Some("root.b"));
    }

    #[test]
    fn push_path_segment_on_empty_base() {
        // 占位路径（set_item 的 From<PyErr> 路径为空）。
        let mut err = ConstructError::Generic {
            message: "x".to_string(),
            path: String::new(),
        };
        err.push_path_segment("b");
        assert_eq!(err.path(), Some("root.b"));
    }

    #[test]
    fn push_path_segment_nested_rebuilds_correct_order() {
        // 内层 StructNode 已重建 "root.c"，外层插入 "inner" → "root.inner.c"。
        let mut err = ConstructError::Stream {
            message: "expected 4".to_string(),
            path: "root.c".to_string(),
        };
        err.push_path_segment("inner");
        assert_eq!(err.path(), Some("root.inner.c"));
    }

    #[test]
    fn push_path_segment_deeply_nested() {
        // 模拟三层嵌套：root → outer → inner → leaf
        let mut err = ConstructError::Stream {
            message: "x".to_string(),
            path: "root".to_string(),
        };
        err.push_path_segment("leaf"); // 最内层 StructNode
        assert_eq!(err.path(), Some("root.leaf"));
        err.push_path_segment("inner"); // 中间 StructNode
        assert_eq!(err.path(), Some("root.inner.leaf"));
        err.push_path_segment("outer"); // 最外层 StructNode
        assert_eq!(err.path(), Some("root.outer.inner.leaf"));
    }

    #[test]
    fn push_path_segment_preserves_message() {
        let mut err = ConstructError::FormatField {
            message: "struct '>I' error".to_string(),
            path: "root".to_string(),
        };
        err.push_path_segment("value");
        assert_eq!(err.message(), Some("struct '>I' error"));
        assert_eq!(err.path(), Some("root.value"));
    }

    #[test]
    fn push_path_segment_on_compilation_error_is_noop() {
        // 编译期错误无 path，push_path_segment 仍写入 root.segment（不丢失信息）。
        let mut err = ConstructError::Compilation {
            message: "bad schema".to_string(),
        };
        err.push_path_segment("field");
        // 编译期错误 path() 返回 None，set_path 对其无操作。
        assert_eq!(err.path(), None);
    }

    #[test]
    fn push_path_segment_on_expr_errors_sets_path() {
        // SF-2 修复：Expr* 变体现在携带 path 字段，push_path_segment 能正确写入。
        let mut err = ConstructError::ExprFieldMissing {
            field: "count".to_string(),
            path: String::new(),
        };
        err.push_path_segment("data");
        assert_eq!(err.path(), Some("root.data"));

        let mut err2 = ConstructError::ExprDivByZero {
            message: "division by zero".to_string(),
            path: String::new(),
        };
        err2.push_path_segment("ratio");
        assert_eq!(err2.path(), Some("root.ratio"));

        let mut err3 = ConstructError::ExprStackUnderflow {
            path: String::new(),
        };
        err3.push_path_segment("computed");
        assert_eq!(err3.path(), Some("root.computed"));
    }

    // ======================================================================
    // From<PyErr> for ConstructError（P0-4：set_item 裸 ?）
    // ======================================================================

    #[test]
    fn from_pyerr_produces_generic_with_empty_path() {
        ensure_python();
        Python::with_gil(|_py| {
            let pyerr = PyValueError::new_err("something went wrong");
            let err: ConstructError = pyerr.into();
            match err {
                ConstructError::Generic { message, path } => {
                    assert!(message.contains("something went wrong"));
                    assert!(path.is_empty(), "From<PyErr> path should start empty");
                }
                other => panic!("expected Generic, got {:?}", other),
            }
        });
    }

    #[test]
    fn from_pyerr_then_push_path_segment_full_chain() {
        // 模拟 set_item 失败：From<PyErr> → path=""，然后 StructNode push_path_segment。
        ensure_python();
        Python::with_gil(|_py| {
            let pyerr = PyValueError::new_err("dict error");
            let mut err: ConstructError = pyerr.into();
            assert_eq!(err.path(), Some(""));
            err.push_path_segment("myfield");
            assert_eq!(err.path(), Some("root.myfield"));
        });
    }
}
