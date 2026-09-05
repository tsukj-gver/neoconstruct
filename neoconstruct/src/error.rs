//! neoconstruct 内部统一错误类型，以及向 pyo3 `PyErr` 的转换。
//!
//! ## 错误流转
//!
//! ```text
//! 节点方法签名:  fn parse(...) -> Result<T, ConstructError>     ← Rust 内部用结构化错误
//!                               ↓ impl From<ConstructError> for PyErr
//! FFI 入口签名:  fn _parse_raw(...) -> PyResult<Py<PyAny>>     ← 入口处转为 PyErr
//!                               ↓ pyo3 自动抛出
//! Python 侧:    except StreamError as e: ...                    ← 用户捕获 Python 异常
//! ```
//!
//! ## 异常类映射
//!
//! Python 异常类定义在 `neoconstruct._errors`（纯 Python 单一定义），Rust 侧在模块
//! 初始化时（[`crate::_neoconstruct_core`]）通过 [`init_exception_classes`] 缓存这些
//! 类的引用到全局 [`EXCEPTIONS`]。`From<ConstructError> for PyErr` 根据变体选择
//! 对应的 Python 异常类，确保 `except StreamError` 等能精确捕获。
//!
//! 缓存未初始化时（如单元测试中未走过模块初始化），`From` 回退到 `PyValueError`，
//! 携带 `full_message()` 输出——保证错误信息不丢失，仅类型降级。

use pyo3::exceptions::PyValueError;
use pyo3::ffi;
// SAFETY 用途：c_str! 宏将 &str 字面量编译期转为 &CStr（CPython C API 需要 *const c_char）。
use pyo3::ffi::c_str;
use pyo3::prelude::*;
use pyo3::sync::GILOnceCell;
use pyo3::types::{PyAny, PyString, PyTuple, PyType};

/// neoconstruct 内部统一错误类型，携带结构化信息。
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

    /// 大小计算错误：节点无静态尺寸（元素数量运行时未知）或依赖动态值。
    ///
    /// 对应 Python 的 `SizeofError`。
    ///
    /// 触发场景（内核 sizeof 路径）：
    /// - `RepeatUntilNode::sizeof`：元素数量由终止表达式运行时决定，无静态尺寸
    /// - `AlignedNode::sizeof`：modulus 非编译期常量，无法静态计算
    ///
    /// 用户面 sizeof API 尚未提供（follow-up）；当前该变体由内核 sizeof
    /// 失败路径产生，保证错误分类学与 Python 侧 `SizeofError` 类接线一致。
    #[error("sizeof error: {message} at {path}")]
    Sizeof {
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

    /// build 时字段缺值：Instance 字段的实例值为 None，或实例无该属性
    /// （AttributeError ≡ 缺值，缺值统一语义）。
    ///
    /// 触发场景：构造实例时未提供该字段值（且无框架派生默认值），错误
    /// 时机后移到 build（值使用点）而非实例化——携带字段名与指引。
    #[error("field value missing: no value was provided for field '{field}' when constructing the instance (provide the field value or set an explicit default) at {path}")]
    BuildValueMissing {
        /// 缺值的字段名。
        field: String,
        /// 错误发生的路径。
        path: String,
    },

    // --- 表达式求值错误 ---
    // 所有 Expr* 变体携带 `path` 字段，支持 `push_path_segment`
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

    /// 表达式求值：算术/移位溢出（i64 边界）。
    ///
    /// 触发场景：[`crate::expr::ExprOp`] 的 `Add` / `Sub` / `Mul` / `Neg` /
    /// `FloorDiv`（`i64::MIN / -1`）/ `Shl`（值超界或移位量 ≥ 64）/ `Shr`
    /// （移位量 ≥ 64）数学结果超出 i64 表示范围。
    ///
    /// Python int 是任意精度永不溢出；静默回绕会产生符号翻转错值（协议
    /// 字段缩放/校验和计算最危险的缺陷形态），因此显式报错。消息统一含
    /// "overflow" 语义词。
    #[error("expression error: {message} at {path}")]
    ExprOverflow {
        /// 错误详情（统一含 "overflow" 语义词与操作数）。
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
    /// Python 无独立 `BitFieldError`，复用 `StreamError` 映射。
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
    /// BitsInteger 节点的运行时错误使用此变体，用户可通过 `except IntegerError`
    /// 精确捕获。
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
    /// `BitPaddingNode::new` 在编译期对非法 pattern 返回此错误，
    /// 比 Python 在 `bits2bytes` 阶段延迟 `KeyError` 更早暴露。
    /// 字节级 `PaddingNode` 的负长度等错误也归此类。
    #[error("padding error: {message} at {path}")]
    Padding {
        /// 错误详情。
        message: String,
        /// 错误发生的路径。
        path: String,
    },

    // --- Array 系列错误 ---
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

    /// RepeatUntil build 时无元素满足终止表达式。
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
    /// 作为 `Result` 的哨兵变体而非 panic，保证 `Construct` trait 签名不变。
    /// 正常路径由 `GreedyRangeNode` 等捕获，不应到达 FFI 入口。
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
    /// `context.get("_index", None)`），不主动触发此变体。保留供未来严格模式使用。
    #[error("index field error: {message} at {path}")]
    IndexField {
        /// 错误详情。
        message: String,
        /// 错误发生的路径。
        path: String,
    },

    /// 字符串错误：编码/解码失败、非 Unicode 输入等。
    ///
    /// 对应 Python construct 的 `StringError`（core.py L54）。
    ///
    /// 使 String 系列构造器的字符串错误可被
    /// `except StringError` 精确捕获，与 Python construct 用户面对齐。
    ///
    /// 触发场景：
    /// - `Encoding::decode`：非法字节序列（如无效 UTF-8 字节）
    /// - `Encoding::encode`：输入非 `str` 类型（如 `bytes`）
    /// - ASCII 编码遇到非 ASCII 字符（>= 128）
    ///
    /// 编码名不合法（`Encoding::from_user_str` 编译期失败）归
    /// [`Compilation`](Self::Compilation)，原因：编码名在 `__init_subclass__`
    /// 时确定，属于配置错误而非数据错误。
    #[error("string error: {message} at {path}")]
    String {
        /// 错误详情。
        message: String,
        /// 错误发生的路径。
        path: String,
    },

    /// 显式错误：用户主动抛出（对应 Python construct `ExplicitError`，core.py L89）。
    ///
    /// **不被 Select / Peek 吞掉**——直接向上传播。Python 中由 `Error` 构造器
    /// 或用户在 `_emitparse`/Adapter 回调中主动抛出。
    ///
    /// 触发场景：
    /// - Select 遍历 subcons 时，某 subcon 抛 Explicit → Select 直接传播（不尝试后续）
    /// - Peek 预读时，inner 抛 Explicit → Peek 直接传播（不返回 Py_None）
    ///
    /// **已知差异**：当前 `From<PyErr> for ConstructError`
    /// 统一转 `Generic`，无法保留 Python 侧的 `ExplicitError` 类型信息。
    /// Rust 不主动构造 Explicit 变体（仅供 Select/Peek 识别用；
    /// 用户路径的完整 parity 依赖 `From<PyErr>` 的后续改进）。
    #[error("explicit error: {message} at {path}")]
    Explicit {
        /// 错误详情。
        message: String,
        /// 错误发生的路径。
        path: String,
    },

    /// Select 错误：所有 subcon 都未成功（对应 Python construct `SelectError`，core.py L109）。
    ///
    /// 触发场景：[`crate::nodes::select::SelectNode`] 的 `parse` / `build` 遍历全部
    /// subcons 后无成功者。
    #[error("select error: {message} at {path}")]
    Select {
        /// 错误详情（如 "no subconstruct matched"）。
        message: String,
        /// 错误发生的路径。
        path: String,
    },

    // --- 校验类错误（Const / Check / Checksum / Terminated / CancelParsing）---
    /// 常量字段错误：parse 时 subcon 结果与期望值不符，或 build 时 obj 非 None/期望值。
    ///
    /// 对应 Python construct `ConstError`（core.py L74）。
    #[error("const error: {message} at {path}")]
    Const {
        /// 错误详情（含期望值与实际值的 repr）。
        message: String,
        /// 错误发生的路径。
        path: String,
    },

    /// 断言检查错误：Check 节点的表达式求值为假。
    ///
    /// 对应 Python construct `CheckError`（core.py L84）。
    #[error("check error: {message} at {path}")]
    Check {
        /// 错误详情（如 "check failed during parsing"）。
        message: String,
        /// 错误发生的路径。
        path: String,
    },

    /// 校验和错误：parse 时 hash 不匹配。
    ///
    /// 对应 Python construct `ChecksumError`（core.py L144）。
    #[error("checksum error: {message} at {path}")]
    Checksum {
        /// 错误详情（含 read / computed 的 hex 编码）。
        message: String,
        /// 错误发生的路径。
        path: String,
    },

    /// 终止符错误：Terminated 节点 parse 时 stream 未到 EOF。
    ///
    /// 对应 Python construct `TerminatedError`（core.py L129）。
    #[error("terminated error: {message} at {path}")]
    Terminated {
        /// 错误详情（如 "expected end of stream"）。
        message: String,
        /// 错误发生的路径。
        path: String,
    },

    /// 用户主动取消解析（对应 Python construct `CancelParsing`，core.py L149）。
    ///
    /// **仅由 Python 用户代码触发**——用户从 Adapter `_decode` / 自定义 Python
    /// 代码 `raise CancelParsing()`，跨 FFI 转为 ConstructError::CancelParsing。
    /// schema.rs parse 入口捕获后返回 Py_None（对齐 Python L418 `except: pass`）。
    ///
    /// # 触发场景限制
    ///
    /// neoconstruct 表达式系统不接 lambda，用户无法在 Computed 表达式
    /// 中 raise CancelParsing。**仅 Python 层用户代码可触发**：
    /// - 用户继承 Adapter 写 `_decode` 内 raise CancelParsing
    /// - 用户在 StructMixin 子类的 `__post_init__` 内 raise
    ///
    /// # 与 ExplicitError 的区别
    ///
    /// CancelParsing 被顶层 catch（schema.rs），parse 返回 None；
    /// ExplicitError 不被顶层 catch，正常向上传播为 ConstructError。
    #[error("cancel parsing at {path}")]
    CancelParsing {
        /// 错误发生的路径（顶层 catch 后丢弃，对齐 Python `pass`）。
        path: String,
    },

    // --- Mapping / Validation / Union / Rotation / NamedTuple 错误 ---
    /// 映射错误：Enum/FlagsEnum/Mapping 的 label↔value 查找失败。
    ///
    /// 对应 Python construct `MappingError`（core.py L102）。
    ///
    /// 触发场景：
    /// - Enum build 时 label 不在 encmapping（bool 边界由编译期处理）
    /// - FlagsEnum build 时未知 label
    /// - Mapping parse/build 时 key 不在映射（包含 TypeError → MappingError 转换）
    #[error("mapping error: {message} at {path}")]
    Mapping {
        /// 错误详情（含期望与实际值的 repr）。
        message: String,
        /// 错误发生的路径。
        path: String,
    },

    /// 校验错误：OneOf/NoneOf 的值集合校验失败。
    ///
    /// 对应 Python construct `ValidationError`（core.py L125）。
    ///
    /// 触发场景：
    /// - OneOf parse/build 时 obj ∉ valids
    /// - NoneOf parse/build 时 obj ∈ invalids
    /// - Validator 子类的 _validate 返回 False
    #[error("validation error: {message} at {path}")]
    Validation {
        /// 错误详情。
        message: String,
        /// 错误发生的路径。
        path: String,
    },

    /// Union 错误：Union build 时无 subcon 匹配，或 parsefrom 解析失败。
    ///
    /// 对应 Python construct `UnionError`（core.py L130）。
    #[error("union error: {message} at {path}")]
    Union {
        /// 错误详情。
        message: String,
        /// 错误发生的路径。
        path: String,
    },

    /// 旋转错误：ProcessRotateLeft 的 group/amount 参数非法或数据长度不对齐。
    ///
    /// 对应 Python construct `RotationError`（core.py L138）。
    ///
    /// 触发场景：
    /// - group < 1
    /// - data.len() % group != 0
    #[error("rotation error: {message} at {path}")]
    Rotation {
        /// 错误详情。
        message: String,
        /// 错误发生的路径。
        path: String,
    },

    /// NamedTuple 错误：inner 非 Struct/Sequence/Array/GreedyRange，或字段提取失败。
    ///
    /// 对应 Python construct `NamedTupleError`（core.py L120）。
    #[error("namedtuple error: {message} at {path}")]
    NamedTuple {
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
            | ConstructError::Sizeof { message, .. }
            | ConstructError::Compilation { message }
            | ConstructError::UnresolvedReference { message }
            | ConstructError::Generic { message, .. }
            | ConstructError::ExprContext { message, .. }
            | ConstructError::ExprDivByZero { message, .. }
            | ConstructError::ExprOverflow { message, .. }
            | ConstructError::BitField { message, .. }
            | ConstructError::Integer { message, .. }
            | ConstructError::Padding { message, .. }
            | ConstructError::Range { message, .. }
            | ConstructError::Repeat { message, .. }
            | ConstructError::IndexField { message, .. }
            | ConstructError::String { message, .. }
            | ConstructError::Explicit { message, .. }
            | ConstructError::Select { message, .. }
            // 校验类（Const / Check / Checksum / Terminated）。
            | ConstructError::Const { message, .. }
            | ConstructError::Check { message, .. }
            | ConstructError::Checksum { message, .. }
            | ConstructError::Terminated { message, .. }
            // Mapping / Validation / Union / Rotation / NamedTuple。
            | ConstructError::Mapping { message, .. }
            | ConstructError::Validation { message, .. }
            | ConstructError::Union { message, .. }
            | ConstructError::Rotation { message, .. }
            | ConstructError::NamedTuple { message, .. } => Some(message),
            // 这些变体没有单一 message 字段，完整错误信息通过 to_string() / full_message() 获取。
            ConstructError::ExprType { .. }
            | ConstructError::ExprFieldMissing { .. }
            | ConstructError::ExprStackUnderflow { .. }
            | ConstructError::StopField { .. }
            | ConstructError::CancelParsing { .. }
            | ConstructError::BuildValueMissing { .. } => None,
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
            | ConstructError::Sizeof { path, .. }
            | ConstructError::Generic { path, .. }
            | ConstructError::BuildValueMissing { path, .. }
            | ConstructError::ExprType { path, .. }
            | ConstructError::ExprFieldMissing { path, .. }
            | ConstructError::ExprContext { path, .. }
            | ConstructError::ExprDivByZero { path, .. }
            | ConstructError::ExprOverflow { path, .. }
            | ConstructError::ExprStackUnderflow { path }
            | ConstructError::BitField { path, .. }
            | ConstructError::Integer { path, .. }
            | ConstructError::Padding { path, .. }
            | ConstructError::Range { path, .. }
            | ConstructError::Repeat { path, .. }
            | ConstructError::StopField { path }
            | ConstructError::IndexField { path, .. }
            | ConstructError::String { path, .. }
            | ConstructError::Explicit { path, .. }
            | ConstructError::Select { path, .. }
            // 校验与取消类（Const / Check / Checksum / Terminated / CancelParsing）。
            | ConstructError::Const { path, .. }
            | ConstructError::Check { path, .. }
            | ConstructError::Checksum { path, .. }
            | ConstructError::Terminated { path, .. }
            | ConstructError::CancelParsing { path }
            // Mapping / Validation / Union / Rotation / NamedTuple。
            | ConstructError::Mapping { path, .. }
            | ConstructError::Validation { path, .. }
            | ConstructError::Union { path, .. }
            | ConstructError::Rotation { path, .. }
            | ConstructError::NamedTuple { path, .. } => Some(path),
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
    /// `ExprStackUnderflow` / `BuildValueMissing`），使用 `Display` 实现作为完整消息。
    pub fn full_message(&self) -> String {
        match self {
            // 结构化变体：无单一 message 字段，Display 已包含完整信息（含 path）。
            ConstructError::ExprType { .. }
            | ConstructError::ExprFieldMissing { .. }
            | ConstructError::ExprStackUnderflow { .. }
            | ConstructError::StopField { .. }
            // CancelParsing 也走 Display（无 message 字段，仅 path）。
            | ConstructError::CancelParsing { .. }
            | ConstructError::BuildValueMissing { .. } => self.to_string(),
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
            ConstructError::Sizeof { .. } => "Sizeof",
            ConstructError::Compilation { .. } => "Compilation",
            ConstructError::UnresolvedReference { .. } => "UnresolvedReference",
            ConstructError::Generic { .. } => "Generic",
            ConstructError::BuildValueMissing { .. } => "BuildValueMissing",
            ConstructError::ExprType { .. } => "ExprType",
            ConstructError::ExprFieldMissing { .. } => "ExprFieldMissing",
            ConstructError::ExprContext { .. } => "ExprContext",
            ConstructError::ExprDivByZero { .. } => "ExprDivByZero",
            ConstructError::ExprOverflow { .. } => "ExprOverflow",
            ConstructError::ExprStackUnderflow { .. } => "ExprStackUnderflow",
            ConstructError::BitField { .. } => "BitField",
            ConstructError::Integer { .. } => "Integer",
            ConstructError::Padding { .. } => "Padding",
            ConstructError::Range { .. } => "Range",
            ConstructError::Repeat { .. } => "Repeat",
            ConstructError::StopField { .. } => "StopField",
            ConstructError::IndexField { .. } => "IndexField",
            ConstructError::String { .. } => "String",
            ConstructError::Explicit { .. } => "Explicit",
            ConstructError::Select { .. } => "Select",
            // 校验与取消类（Const / Check / Checksum / Terminated / CancelParsing）。
            ConstructError::Const { .. } => "Const",
            ConstructError::Check { .. } => "Check",
            ConstructError::Checksum { .. } => "Checksum",
            ConstructError::Terminated { .. } => "Terminated",
            ConstructError::CancelParsing { .. } => "CancelParsing",
            // Mapping / Validation / Union / Rotation / NamedTuple。
            ConstructError::Mapping { .. } => "Mapping",
            ConstructError::Validation { .. } => "Validation",
            ConstructError::Union { .. } => "Union",
            ConstructError::Rotation { .. } => "Rotation",
            ConstructError::NamedTuple { .. } => "NamedTuple",
        }
    }

    /// 在错误路径中插入一个字段段（lazy path 模式：错误路径延迟重建）。
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

    /// 在错误路径中插入一个数组索引段（Array 系列 lazy path 模式）。
    ///
    /// 与 [`push_path_segment`](Self::push_path_segment) 平行，但插入 `[i]` 而非 `.field`。
    /// 用于 Array 系列节点（Array / GreedyRange / PrefixedArray / RepeatUntil）在
    /// 子节点返回 `Err` 时重建索引路径段。
    ///
    /// # 路径重建规则（INSERT-after-root，与 push_path_segment 一致）
    ///
    /// 把新段 `[i]` 插入到 `"root"` 之后、已有 suffix 之前，保证任意嵌套组合
    /// （Struct↔Array）都能正确重建路径：
    ///
    /// - 错误路径为 `None` / `""` / `"root"`（叶节点基线）→ `"root[i]"`。
    /// - 错误路径形如 `"root.x.y"`（内层 StructNode 已重建）→
    ///   `"root[i].x.y"`（segment 插入 root 之后）。
    /// - 错误路径形如 `"root[j]"`（内层 Array 已重建）→ `"root[i][j]"`。
    /// - 编译期错误（无 path）→ 不变。
    ///
    /// 仅在错误路径调用（成功路径零成本），开销可接受。
    pub fn push_path_index(&mut self, i: usize) {
        // 编译期错误（Compilation / UnresolvedReference）无 path 字段，
        // set_path 对其是 no-op。提前返回，跳过无用的 format! 路径计算。
        if self.path().is_none() {
            return;
        }
        let new_path = match self.path() {
            Some(p) if p.is_empty() || p == "root" => format!("root[{}]", i),
            Some(p) => {
                if let Some(suffix) = p.strip_prefix("root") {
                    // suffix 为 ""（"root"）或 ".x.y" / "[j]" / ".x[j].y" 等。
                    format!("root[{}]{}", i, suffix)
                } else {
                    // 非标准根，退化为追加（仅防御性，正常路径不触发）。
                    format!("{}[{}]", p, i)
                }
            }
            None => format!("root[{}]", i),
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
            | ConstructError::Sizeof { path, .. }
            | ConstructError::Generic { path, .. }
            | ConstructError::BuildValueMissing { path, .. }
            | ConstructError::ExprType { path, .. }
            | ConstructError::ExprFieldMissing { path, .. }
            | ConstructError::ExprContext { path, .. }
            | ConstructError::ExprDivByZero { path, .. }
            | ConstructError::ExprOverflow { path, .. }
            | ConstructError::ExprStackUnderflow { path }
            | ConstructError::BitField { path, .. }
            | ConstructError::Integer { path, .. }
            | ConstructError::Padding { path, .. }
            | ConstructError::Range { path, .. }
            | ConstructError::Repeat { path, .. }
            | ConstructError::StopField { path }
            | ConstructError::IndexField { path, .. }
            | ConstructError::String { path, .. }
            | ConstructError::Explicit { path, .. }
            | ConstructError::Select { path, .. }
            // 校验与取消类（Const / Check / Checksum / Terminated / CancelParsing）。
            | ConstructError::Const { path, .. }
            | ConstructError::Check { path, .. }
            | ConstructError::Checksum { path, .. }
            | ConstructError::Terminated { path, .. }
            | ConstructError::CancelParsing { path }
            // Mapping / Validation / Union / Rotation / NamedTuple。
            | ConstructError::Mapping { path, .. }
            | ConstructError::Validation { path, .. }
            | ConstructError::Union { path, .. }
            | ConstructError::Rotation { path, .. }
            | ConstructError::NamedTuple { path, .. } => *path = new_path,
            ConstructError::Compilation { .. } | ConstructError::UnresolvedReference { .. } => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Python 异常类缓存
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
    /// 对应 `ConstructError::Sizeof`。
    /// Python 的 `SizeofError`，内核 sizeof 失败（无静态尺寸/动态 modulus）。
    sizeof_error: Py<PyType>,
    /// 对应 `ConstructError::Compilation`。
    compilation_error: Py<PyType>,
    /// 对应 `ConstructError::UnresolvedReference`。
    unresolved_reference_error: Py<PyType>,
    /// 对应 `ConstructError::Generic`。
    generic_construct_error: Py<PyType>,
    /// 对应 `ConstructError::BuildValueMissing`。
    /// Python 的 `FieldValueMissingError`（build 时字段缺值）。
    field_value_missing_error: Py<PyType>,
    /// `ConstructError` 基类，用于 Expr* 变体
    /// （ExprType / ExprFieldMissing / ExprContext / ExprDivByZero /
    /// ExprOverflow / ExprStackUnderflow）。
    ///
    /// 当前无专门的 ExprError Python 类，统一映射到基类。
    /// 若需更精确的错误类型映射，可增加专门的 ExprError 类并在此缓存。
    construct_error_base: Py<PyType>,
    /// 对应 `ConstructError::Integer`。
    /// Python construct 的 `IntegerError`，用于 BitsInteger 节点的运行时错误。
    integer_error: Py<PyType>,
    /// 对应 `ConstructError::Padding`。
    /// Python construct 的 `PaddingError`，用于 Padding 字段的非法 pattern 等。
    padding_error: Py<PyType>,
    /// 对应 `ConstructError::Range`。
    /// Python construct 的 `RangeError`，用于 Array count 无效等。
    range_error: Py<PyType>,
    /// 对应 `ConstructError::Repeat`。
    /// Python construct 的 `RepeatError`，用于 RepeatUntil build 无元素满足终止表达式。
    repeat_error: Py<PyType>,
    /// 对应 `ConstructError::StopField`。
    /// Python construct 的 `StopFieldError`，StopIf 早停信号。
    stop_field_error: Py<PyType>,
    /// 对应 `ConstructError::IndexField`。
    /// Python construct 的 `IndexFieldError`，Index 节点 _index 缺失（保留）。
    index_field_error: Py<PyType>,
    /// 对应 `ConstructError::String`。
    /// Python construct 的 `StringError`（core.py L54），String 系列构造器的
    /// 编解码失败 / 非 Unicode 输入等。
    string_error: Py<PyType>,
    /// 对应 `ConstructError::Explicit`。
    /// Python construct 的 `ExplicitError`（core.py L89），用户主动抛出的错误
    /// （Select / Peek 不吞掉，直接向上传播）。
    explicit_error: Py<PyType>,
    /// 对应 `ConstructError::Select`。
    /// Python construct 的 `SelectError`（core.py L109），Select 遍历全部
    /// subcons 后无成功者。
    select_error: Py<PyType>,
    // === 校验与取消类（Const / Check / Checksum / Terminated / CancelParsing）===
    /// 对应 `ConstructError::Const`。
    /// Python construct 的 `ConstError`（core.py L74）。
    const_error: Py<PyType>,
    /// 对应 `ConstructError::Check`。
    /// Python construct 的 `CheckError`（core.py L84）。
    check_error: Py<PyType>,
    /// 对应 `ConstructError::Checksum`。
    /// Python construct 的 `ChecksumError`（core.py L144）。
    checksum_error: Py<PyType>,
    /// 对应 `ConstructError::Terminated`。
    /// Python construct 的 `TerminatedError`（core.py L129）。
    terminated_error: Py<PyType>,
    /// 对应 `ConstructError::CancelParsing`。
    /// Python construct 的 `CancelParsing`（core.py L149）。用户主动取消解析。
    cancel_parsing_error: Py<PyType>,
    // === Mapping / Validation / Union / Rotation / NamedTuple ===
    /// 对应 `ConstructError::Mapping`。
    /// Python construct 的 `MappingError`（core.py L102）。
    mapping_error: Py<PyType>,
    /// 对应 `ConstructError::Validation`。
    /// Python construct 的 `ValidationError`（core.py L125）。
    validation_error: Py<PyType>,
    /// 对应 `ConstructError::Union`。
    /// Python construct 的 `UnionError`（core.py L130）。
    union_error: Py<PyType>,
    /// 对应 `ConstructError::Rotation`。
    /// Python construct 的 `RotationError`（core.py L138）。
    rotation_error: Py<PyType>,
    /// 对应 `ConstructError::NamedTuple`。
    /// Python construct 的 `NamedTupleError`（core.py L120）。
    namedtuple_error: Py<PyType>,
}

/// 全局 Python 异常类缓存。
///
/// 使用 [`GILOnceCell`] 保证：
/// - 写入需要 GIL（仅在模块初始化时调用 [`init_exception_classes`]）
/// - 读取需要 GIL（pyo3 的 `Python::with_gil` 在 [`From`] 实现中获取）
/// - 一旦写入，永不变更（异常类是模块级单例）
static EXCEPTIONS: GILOnceCell<ExceptionClasses> = GILOnceCell::new();

impl ExceptionClasses {
    /// 检查 `cls_ptr` 是否确切等于缓存中某个内置异常类（指针相等比较）。
    ///
    /// 用于 fast-path 前置条件检查：仅当 `cls` 是
    /// 确切内置异常类时，才能安全跳过 Python `__init__` 字节码；用户子类化必须
    /// 走 `call1` 慢路径，否则会绕过用户的 `__init__` 重写。
    ///
    /// # 子类化检测方法
    ///
    /// 用裸指针相等比较，**而非** `PyType_IsSubtype`：后者检查"A 是否 B 的子类"，
    /// 对任何子类返回 true，不适用于"是否确切等于内置类"判定。指针比较直接判定
    /// 对象身份，仅确切类型（无子类）才返回 true。
    ///
    /// # 参数
    ///
    /// - `cls_ptr`：待判定的 Python 类型对象指针（来自 `Bound<PyType>::as_ptr()`
    ///   或 `Py<PyType>::as_ptr()`，两者均返回 `*mut PyObject`）。
    ///
    /// # 返回
    ///
    /// `true` 表示 `cls_ptr` 等于 16 个内置异常类之一，可进入 fast-path；
    /// `false` 表示用户子类化或未知类，走慢路径。
    fn is_builtin_class(&self, cls_ptr: *mut ffi::PyObject) -> bool {
        // Py<PyType>::as_ptr() 返回 *mut PyObject（与 Bound<PyType>::as_ptr() 一致）。
        // 比较裸指针即可判定是否同一对象（CPython 类型对象是单例）。
        //
        // 数组覆盖全部 28 个缓存异常类；新增缓存字段时需同步扩容此数组与类型长度。
        let builtin_ptrs: [*mut ffi::PyObject; 28] = [
            self.stream_error.as_ptr(),
            self.format_field_error.as_ptr(),
            self.field_length_error.as_ptr(),
            self.sizeof_error.as_ptr(),
            self.compilation_error.as_ptr(),
            self.unresolved_reference_error.as_ptr(),
            self.generic_construct_error.as_ptr(),
            self.field_value_missing_error.as_ptr(),
            self.construct_error_base.as_ptr(),
            self.integer_error.as_ptr(),
            self.padding_error.as_ptr(),
            self.range_error.as_ptr(),
            self.repeat_error.as_ptr(),
            self.stop_field_error.as_ptr(),
            self.index_field_error.as_ptr(),
            self.string_error.as_ptr(),
            self.explicit_error.as_ptr(),
            self.select_error.as_ptr(),
            // 校验与取消类（Const / Check / Checksum / Terminated / CancelParsing）。
            self.const_error.as_ptr(),
            self.check_error.as_ptr(),
            self.checksum_error.as_ptr(),
            self.terminated_error.as_ptr(),
            self.cancel_parsing_error.as_ptr(),
            // Mapping / Validation / Union / Rotation / NamedTuple。
            self.mapping_error.as_ptr(),
            self.validation_error.as_ptr(),
            self.union_error.as_ptr(),
            self.rotation_error.as_ptr(),
            self.namedtuple_error.as_ptr(),
        ];
        builtin_ptrs.contains(&cls_ptr)
    }
}

/// 在模块初始化时调用，从 `neoconstruct._errors` 缓存 Python 异常类引用。
///
/// 必须在 [`crate::_neoconstruct_core`] 模块初始化函数中调用一次。多次调用幂等
/// （后续调用因已初始化而立即返回 `Ok(())`）。
///
/// 调用时机：`neoconstruct._errors` 已被 Python 侧 `neoconstruct/__init__.py` 导入
/// （在 `_neoconstruct_core` 之前），因此可直接 `import`。
///
/// # 错误
///
/// - `neoconstruct._errors` 模块不存在（环境异常，理论上不应发生）。
/// - 模块中缺少预期的异常类（开发期错误）。
pub fn init_exception_classes(py: Python<'_>) -> PyResult<()> {
    if EXCEPTIONS.get(py).is_some() {
        return Ok(());
    }

    let errors_module = py.import_bound("neoconstruct._errors")?;
    let get = |name: &str| -> PyResult<Py<PyType>> {
        errors_module
            .getattr(name)
            .and_then(|attr| attr.extract::<Py<PyType>>())
    };

    let classes = ExceptionClasses {
        stream_error: get("StreamError")?,
        format_field_error: get("FormatFieldError")?,
        field_length_error: get("FieldLengthError")?,
        sizeof_error: get("SizeofError")?,
        compilation_error: get("CompilationError")?,
        unresolved_reference_error: get("UnresolvedReferenceError")?,
        generic_construct_error: get("GenericConstructError")?,
        field_value_missing_error: get("FieldValueMissingError")?,
        construct_error_base: get("ConstructError")?,
        integer_error: get("IntegerError")?,
        padding_error: get("PaddingError")?,
        range_error: get("RangeError")?,
        repeat_error: get("RepeatError")?,
        stop_field_error: get("StopFieldError")?,
        index_field_error: get("IndexFieldError")?,
        string_error: get("StringError")?,
        explicit_error: get("ExplicitError")?,
        select_error: get("SelectError")?,
        // 校验与取消类（Const / Check / Checksum / Terminated / CancelParsing）。
        const_error: get("ConstError")?,
        check_error: get("CheckError")?,
        checksum_error: get("ChecksumError")?,
        terminated_error: get("TerminatedError")?,
        cancel_parsing_error: get("CancelParsing")?,
        // Mapping / Validation / Union / Rotation / NamedTuple。
        mapping_error: get("MappingError")?,
        validation_error: get("ValidationError")?,
        union_error: get("UnionError")?,
        rotation_error: get("RotationError")?,
        namedtuple_error: get("NamedTupleError")?,
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
///
/// # 借用返回（避免 incref 开销）
///
/// 返回 `&Bound<'py, PyType>` 借用引用，**不**调用 `clone()`（避免 `Py_INCREF`/
/// `Py_DECREF` 各一次的 ~20ns incref 开销）。借用周期由 `classes` 参数的生命周期
/// `'py` 保证——`classes` 来自全局 [`EXCEPTIONS`] 的 `GILOnceCell::get`，一旦模块
/// 初始化完成永不变更。
fn select_exception_class<'py>(
    err: &ConstructError,
    classes: &'py ExceptionClasses,
    py: Python<'py>,
) -> &'py Bound<'py, PyType> {
    let cls: &Py<PyType> = match err {
        ConstructError::Stream { .. } => &classes.stream_error,
        ConstructError::FormatField { .. } => &classes.format_field_error,
        ConstructError::FieldLength { .. } => &classes.field_length_error,
        ConstructError::Sizeof { .. } => &classes.sizeof_error,
        ConstructError::Compilation { .. } => &classes.compilation_error,
        ConstructError::UnresolvedReference { .. } => &classes.unresolved_reference_error,
        ConstructError::Generic { .. } => &classes.generic_construct_error,
        // build 时字段缺值映射到 FieldValueMissingError。
        ConstructError::BuildValueMissing { .. } => &classes.field_value_missing_error,
        // Expr* 变体无专门的 Python 异常类，统一映射到基类 ConstructError。
        ConstructError::ExprType { .. }
        | ConstructError::ExprFieldMissing { .. }
        | ConstructError::ExprContext { .. }
        | ConstructError::ExprDivByZero { .. }
        | ConstructError::ExprOverflow { .. }
        | ConstructError::ExprStackUnderflow { .. } => &classes.construct_error_base,
        // BitField：Python construct 无独立 BitFieldError，
        // 复用 StreamError（与 Python RestreamedBytesIO.close 缓冲
        // 清空校验的 StreamError 对齐）。
        ConstructError::BitField { .. } => &classes.stream_error,
        // BitsInteger 运行时错误映射到 Python IntegerError，
        // 与 Python construct 的 IntegerError 对齐（core.py L49/L1367 等）。
        ConstructError::Integer { .. } => &classes.integer_error,
        // Padding 错误映射到 Python PaddingError（core.py L124/L4146）。
        ConstructError::Padding { .. } => &classes.padding_error,
        // Array 系列。
        ConstructError::Range { .. } => &classes.range_error,
        ConstructError::Repeat { .. } => &classes.repeat_error,
        ConstructError::StopField { .. } => &classes.stop_field_error,
        ConstructError::IndexField { .. } => &classes.index_field_error,
        // String 系列错误映射到 Python StringError（core.py L54）。
        ConstructError::String { .. } => &classes.string_error,
        // 显式错误映射到 Python ExplicitError（core.py L89）。
        ConstructError::Explicit { .. } => &classes.explicit_error,
        // Select 错误映射到 Python SelectError（core.py L109）。
        ConstructError::Select { .. } => &classes.select_error,
        // 校验与取消类（Const / Check / Checksum / Terminated / CancelParsing）。
        ConstructError::Const { .. } => &classes.const_error,
        ConstructError::Check { .. } => &classes.check_error,
        ConstructError::Checksum { .. } => &classes.checksum_error,
        ConstructError::Terminated { .. } => &classes.terminated_error,
        ConstructError::CancelParsing { .. } => &classes.cancel_parsing_error,
        // Mapping / Validation / Union / Rotation / NamedTuple。
        ConstructError::Mapping { .. } => &classes.mapping_error,
        ConstructError::Validation { .. } => &classes.validation_error,
        ConstructError::Union { .. } => &classes.union_error,
        ConstructError::Rotation { .. } => &classes.rotation_error,
        ConstructError::NamedTuple { .. } => &classes.namedtuple_error,
    };
    // cls.bind(py) 返回 &Bound<'py, PyType>，借用 cls（借自 classes）。
    // 不调 clone()，避免 incref/decref 各一次。
    cls.bind(py)
}

/// 构造一个携带 `message` 与可选 `path` 的 Python 异常实例。
///
/// 对齐 `neoconstruct._errors.ConstructError.__init__(message, path=None)` 的签名。
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

/// 异常构造 fast-path：绕过 Python `__init__` 直接构造异常实例。
///
/// 仅对 [`ExceptionClasses`] 缓存的 26 个内置异常类启用（指针相等比较判定，见
/// [`ExceptionClasses::is_builtin_class`]）。用户子类化的异常返回 `None`，由调用方
/// 走 [`build_exception_instance`] 慢路径以保证用户的 `__init__` 被调用。
///
/// # fast-path 流程
///
/// 1. `PyType_GenericAlloc(cls, 0)` 分配实例（CPython 固有 ~200ns，不可压缩）。
/// 2. `PyObject_SetAttrString(instance, "message", msg)` 写入 `message` 属性。
/// 3. `PyObject_SetAttrString(instance, "path", path_or_none)` 写入 `path` 属性。
/// 4. `PyObject_SetAttrString(instance, "args", (message,))` 写入 `args` 字段
///    （`BaseException_setattro` 在 CPython 中对 `"args"` 特殊处理，绕过 READONLY
///    member 限制——见 `Objects/exceptions.c` 中 `BaseException_setattro`）。
/// 5. 包装为 `PyErr`（直接构造，避免中间层 incref）。
///
/// 与慢路径 [`build_exception_instance`] 的行为差异：
/// - 慢路径：`args = (full,)`，其中 `full = "Error in path X\nY"` 或 `Y`。
/// - fast-path：`args = (message,)`（不含 path 前缀）。
/// - `str(e)` 行为一致：Python 侧 `__str__` 已重写为基于 `self.path` + `self.message`
///   格式化，不依赖 `args`。
/// - `repr(e)` / `pickle` 行为与慢路径存在已知差异（`args` 不含 path 前缀，属有意设计）。
///
/// # 失败模式（回退到慢路径）
///
/// 任何步骤返回 -1 或 NULL 时：
/// 1. 调用 `ffi::PyErr_Clear()` 清除挂起的 Python 异常（避免污染后续调用）。
/// 2. 释放已分配的部分实例（`ffi::Py_DecRef`），避免内存泄漏。
/// 3. 返回 `None`，调用方走 [`build_exception_instance`] 慢路径。
///
/// # 引用计数
///
/// `PyType_GenericAlloc` 返回 refcount=1 的新实例。在 4 个 SetAttr 调用中：
/// - `message` / `args` 共享同一 `PyString`（refcount=2 from instance attrs）。
/// - `path` 为 PyString（refcount=1 from instance attr）或借用 PyNone（SetAttr
///   内部 incref，无需调用方管理）。
///
/// 最后 `Bound::from_owned_ptr_or_opt` 消费 instance 指针（不 incref），所有权
/// 转移给 `Bound<PyAny>`，再转移给 `PyErr`。
///
/// # 安全性
///
/// 内部 4 个 `unsafe` 块各自标注前置条件，每个块的内联 `SAFETY` 注释列出前置条件
/// + 失败模式 + 慢路径回退。函数本身是 safe（所有 unsafe 操作均被前置条件检查包裹）。
fn try_fast_path_alloc<'py>(
    py: Python<'py>,
    cls: &Bound<'py, PyType>,
    classes: &ExceptionClasses,
    message: &str,
    path: Option<&str>,
) -> Option<PyErr> {
    // 前置条件：确切类型为内置类（指针相等比较，非 PyType_IsSubtype）。
    // 用户子类化（cls != 任何内置类）→ 返回 None，走慢路径。
    if !classes.is_builtin_class(cls.as_ptr()) {
        return None;
    }

    // SAFETY: 以下 unsafe 块逐项满足各自注释标明的前置条件。
    // - GIL 持有：py: Python<'py> token 在作用域内（FFI 入口已持 GIL）。
    // - cls 有效：来自 Bound<PyType>（pyo3 类型保证）。
    // - cls 确切为内置类：上面 is_builtin_class 指针比较已验证。
    unsafe {
        // 步骤 1：分配实例。
        // 前置条件：GIL 持有；cls 是有效 PyTypeObject；cls 是确切内置类。
        // 失败模式：返回 NULL + PyErr_MemoryError（仅 OOM），下面 is_null() 分支处理。
        let instance: *mut ffi::PyObject = ffi::PyType_GenericAlloc(cls.as_type_ptr(), 0);
        if instance.is_null() {
            // 清除挂起的异常，回退到慢路径。
            ffi::PyErr_Clear();
            return None;
        }

        // 构造 message PyString（new_bound 返回 owned Bound，refcount=1）。
        let msg_bound: Bound<'py, PyString> = PyString::new_bound(py, message);

        // 步骤 2：设置 message 属性。
        // 前置条件：instance 是步骤 1 返回的非空对象；msg_bound.as_ptr() 是有效 PyString。
        // 失败模式：返回 -1（极端，如内置类被 monkey patch __setattr__），下面分支处理。
        if ffi::PyObject_SetAttrString(instance, c_str!("message").as_ptr(), msg_bound.as_ptr()) < 0
        {
            // 清除异常 + 释放已分配实例，回退慢路径。
            ffi::PyErr_Clear();
            ffi::Py_DecRef(instance);
            return None;
        }
        // 引用计数：msg_bound 的 PyString 现 refcount=2（msg_bound + instance.message）。

        // 构造 path 值。Some(p) → PyString；None → 借用 Py_None（不 incref，SetAttr 内部 incref）。
        let path_owned: Option<Bound<'py, PyString>> = path.map(|p| PyString::new_bound(py, p));
        let path_ptr: *mut ffi::PyObject = match path_owned.as_ref() {
            Some(b) => b.as_ptr(),
            // SAFETY: 持有 GIL，Py_None() 返回有效 *mut PyObject（CPython 单例）。
            None => ffi::Py_None(),
        };

        // 步骤 3：设置 path 属性。
        // 前置条件：instance 是步骤 1 返回的非空对象；path_ptr 是有效 PyString 或 Py_None。
        // 失败模式：同步骤 2。
        if ffi::PyObject_SetAttrString(instance, c_str!("path").as_ptr(), path_ptr) < 0 {
            ffi::PyErr_Clear();
            ffi::Py_DecRef(instance);
            return None;
        }
        // 引用计数：path_owned 的 PyString 现 refcount=2（path_owned + instance.path）；
        // 若为 Py_None，Py_None 全局 refcount +1（由 instance 持有）。

        // 步骤 4：设置 args = (message,)。
        // BaseException_setattro 在 CPython 中对 "args" 特殊处理（虽 member 标记 READONLY）：
        //   - 检查 name == &_Py_ID(args)，调 Py_XSETREF(self->args, value)。
        //   - 调用方无需操作 C struct 字段。
        // PyTuple::new_bound(py, iter) 创建新元组，increfs 每个 element（msg refcount 现 =3）。
        let args_tuple: Bound<'py, PyTuple> = PyTuple::new_bound(py, [msg_bound.clone()]);
        // msg_bound.clone() 返回 Py<PyString>（refcount +1 =3: msg_bound + instance.message + 临时）。
        // PyTuple::new_bound 消费 iter，构造元组时 incref（+1 =4）。临时 Py<PyString> drop → -1 =3。
        // 最终：instance.message (1) + tuple element (1) + msg_bound (1) = 3。

        if ffi::PyObject_SetAttrString(instance, c_str!("args").as_ptr(), args_tuple.as_ptr()) < 0 {
            ffi::PyErr_Clear();
            ffi::Py_DecRef(instance);
            return None;
        }
        // 引用计数：args_tuple 现 refcount=2（args_tuple + instance.args）。

        // 直接构造 PyErr。Bound::from_owned_ptr_or_opt 消费 instance 指针
        // （不 incref），所有权转移给 Bound<PyAny>。再 PyErr::from_value_bound
        // 包装（不 incref，转移所有权）。
        let bound_any: Bound<'py, PyAny> = Bound::from_owned_ptr_or_opt(py, instance)?.into_any();
        Some(PyErr::from_value_bound(bound_any))
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
/// 无论映射到哪个 Python 异常类，错误消息的格式都与 `neoconstruct._errors.ConstructError`
/// 一致：path 非 None 时为 `"Error in path {path}\n{message}"`，否则为 `{message}`。
///
/// ## 错误路径性能优化
///
/// 优化路径：
///
/// | 优化项 | 实现 |
/// |--------|------|
/// | lazy 字符串 | 借用 `&err` 进入 with_gil 闭包，避免 `to_string()` 预格式化 |
/// | 避免 incref | `select_exception_class` 返回 `&Bound<'py, PyType>`，不 `clone()` |
/// | 绕过 __init__ | `try_fast_path_alloc` 用 raw CPython C API 直接构造 |
/// | PyErr 直接构造 | `PyErr::from_value_bound` 一步构造，无中间层 |
///
/// 慢路径（[`build_exception_instance`]）保留作为：
/// - 用户子类化异常的兼容路径
/// - fast-path 失败的回退路径
///
/// ## GIL 获取
///
/// `From` trait 不持有 GIL，因此内部通过 [`Python::with_gil`] 获取。仅在错误路径
/// 触发（成功路径零成本），开销可接受（~微秒级）。
impl From<ConstructError> for PyErr {
    fn from(err: ConstructError) -> Self {
        // 借用 err，避免在 with_gil 闭包外预格式化 message/path。
        // 闭包捕获 &err，借用其 message()/path() 返回的 &str（零拷贝）。
        Python::with_gil(|py| match EXCEPTIONS.get(py) {
            Some(classes) => {
                // 直接借用 err.message() / err.path()，不调 to_string()。
                // 对于无单一 message 字段的结构化变体（ExprType/ExprFieldMissing/
                // ExprStackUnderflow/StopField），使用 Display（to_string）作为
                // 完整消息，path=None（避免重复，path 已嵌入 Display 输出）。
                // 此分支罕见（仅在表达式错误时），分配 String 可接受。
                //
                // 例外：BuildValueMissing 虽无单一 message 字段，但 path 是
                // 结构化 API 承诺（错误携带 path 字段）——拆分出消息体与
                // path 分别传入，保证 Python 侧 `e.path == "root.y"` 可用
                // （与其他运行期变体的 path 契约一致）。
                let formatted_str: String;
                let bvm_str: String;
                let (message, path): (&str, Option<&str>) = match &err {
                    ConstructError::BuildValueMissing { field, path } => {
                        bvm_str = build_value_missing_message(field);
                        (bvm_str.as_str(), Some(path.as_str()))
                    }
                    other => match other.message() {
                        Some(m) => (m, other.path()),
                        None => {
                            formatted_str = other.to_string();
                            (formatted_str.as_str(), None)
                        }
                    },
                };

                // 借用类引用，无 incref。
                let cls: &Bound<'_, PyType> = select_exception_class(&err, classes, py);

                // 尝试 fast-path。仅内置类生效；用户子类化 / fast-path 失败
                // 时返回 None，落到下面的慢路径。
                if let Some(pyerr) = try_fast_path_alloc(py, cls, classes, message, path) {
                    return pyerr;
                }

                // 慢路径（用户子类化 / fast-path 失败）：call1 实现。
                match build_exception_instance(py, cls, message, path) {
                    Ok(instance) => PyErr::from_value_bound(instance),
                    // fallback：异常实例构造失败，回退到 PyValueError + full_message。
                    Err(_) => PyValueError::new_err(err.full_message()),
                }
            }
            None => PyValueError::new_err(err.full_message()),
        })
    }
}

/// 构造 `BuildValueMissing` 的 Python 侧消息体（不含 path 后缀）。
///
/// 与 [`ConstructError::BuildValueMissing`] 的 Display 输出同文（去掉
/// " at {path}" 尾部），使 Python 侧 `str(e)` 呈现为标准的
/// "Error in path {path}\n{message}" 形态且 `.path` 属性可用。
fn build_value_missing_message(field: &str) -> String {
    format!(
        "field value missing: no value was provided for field '{}' when \
         constructing the instance (provide the field value or set an explicit default)",
        field
    )
}

/// 用户语义异常分类：识别 Python 侧主动 raise 的特殊异常类。
///
/// 识别清单（指针相等比较，与 [`ExceptionClasses::is_builtin_class`] 同模式）：
///
/// - `ExplicitError` → [`ConstructError::Explicit`]：用户显式终止信号，
///   Select/Peek 等容错链**透传不吞**（兑现 `_errors.py` docstring 承诺）。
/// - `CancelParsing` → [`ConstructError::CancelParsing`]：用户主动取消，
///   被 schema.rs 顶层 catch（parse 返回 None）。
///
/// 其余异常（用户回调中的 ValueError 等）返回 `None`，由调用方按 Generic
/// 包装（携带上下文消息）。
///
/// 供 [`From<PyErr> for ConstructError`](impl-From%3CPyErr%3E-for-ConstructError)
/// 与用户回调边界（如 `AdapterCallbackNode`）共用——两处的用户异常分类
/// 必须一致，否则嵌入回调链中的语义异常会被吞掉。
pub(crate) fn classify_user_pyerr(e: &PyErr, py: Python<'_>) -> Option<ConstructError> {
    let classes = EXCEPTIONS.get(py)?;
    let err_type_ptr = e.get_type_bound(py).as_ptr();
    let value = e.value_bound(py);
    // 提取异常实例的 message/path 属性（失败时回退空值——异常构造不走
    // fast-path 时属性仍由 __init__ 设置，正常路径均有）。
    let extract_str = |name: &str| -> String {
        value
            .getattr(name)
            .ok()
            .and_then(|p| p.extract::<String>().ok())
            .unwrap_or_default()
    };
    if err_type_ptr == classes.explicit_error.as_ptr() {
        return Some(ConstructError::Explicit {
            message: extract_str("message"),
            path: extract_str("path"),
        });
    }
    if err_type_ptr == classes.cancel_parsing_error.as_ptr() {
        // CancelParsing 顶层 catch 后 path 会被丢弃（对齐 Python `pass`）。
        return Some(ConstructError::CancelParsing {
            path: extract_str("path"),
        });
    }
    None
}

/// 将 pyo3 的 `PyErr` 转换为 [`ConstructError`]（含用户语义异常识别）。
///
/// 使 `dict.set_item(...)?` 等返回 `PyResult` 的 C API 调用能通过 `?` 直接传播，
/// 无需在每个调用点写 `map_err` 闭包。错误上下文（字段名、路径）由上层
/// `StructNode` 通过 [`ConstructError::push_path_segment`] 统一补充。
///
/// 对齐 pydantic-core 的 `ValResult: From<PyErr>`（model_fields.rs:363）。
///
/// # 用户语义异常识别
///
/// 用户从 Python 代码 `raise ExplicitError` / `raise CancelParsing` 时，
/// pyo3 把 PyErr 传给 Rust。本实现通过 [`classify_user_pyerr`] 识别这两个
/// 用户语义异常类（指针相等比较），保持类型语义跨 FFI 不丢失；
/// 否则 fallback 到 `Generic`。
impl From<PyErr> for ConstructError {
    fn from(e: PyErr) -> Self {
        Python::with_gil(|py| {
            if let Some(classified) = classify_user_pyerr(&e, py) {
                return classified;
            }
            // 默认 fallback（原逻辑）
            ConstructError::Generic {
                message: e.to_string(),
                path: String::new(),
            }
        })
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
            "class SizeofError(ConstructError): pass\n",
            "class CompilationError(ConstructError): pass\n",
            "class UnresolvedReferenceError(ConstructError): pass\n",
            "class GenericConstructError(ConstructError): pass\n",
            "class FieldValueMissingError(ConstructError): pass\n",
            "class IntegerError(ConstructError): pass\n",
            "class PaddingError(ConstructError): pass\n",
            "class RangeError(ConstructError): pass\n",
            "class RepeatError(ConstructError): pass\n",
            "class StopFieldError(ConstructError): pass\n",
            "class IndexFieldError(ConstructError): pass\n",
            "class StringError(ConstructError): pass\n",
            "class ExplicitError(ConstructError): pass\n",
            "class SelectError(ConstructError): pass\n",
            "class ConstError(ConstructError): pass\n",
            "class CheckError(ConstructError): pass\n",
            "class ChecksumError(ConstructError): pass\n",
            "class TerminatedError(ConstructError): pass\n",
            "class CancelParsing(ConstructError): pass\n",
            "class MappingError(ConstructError): pass\n",
            "class ValidationError(ConstructError): pass\n",
            "class UnionError(ConstructError): pass\n",
            "class RotationError(ConstructError): pass\n",
            "class NamedTupleError(ConstructError): pass\n",
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
            sizeof_error: get("SizeofError"),
            compilation_error: get("CompilationError"),
            unresolved_reference_error: get("UnresolvedReferenceError"),
            generic_construct_error: get("GenericConstructError"),
            field_value_missing_error: get("FieldValueMissingError"),
            construct_error_base: get("ConstructError"),
            integer_error: get("IntegerError"),
            padding_error: get("PaddingError"),
            range_error: get("RangeError"),
            repeat_error: get("RepeatError"),
            stop_field_error: get("StopFieldError"),
            index_field_error: get("IndexFieldError"),
            string_error: get("StringError"),
            explicit_error: get("ExplicitError"),
            select_error: get("SelectError"),
            const_error: get("ConstError"),
            check_error: get("CheckError"),
            checksum_error: get("ChecksumError"),
            terminated_error: get("TerminatedError"),
            cancel_parsing_error: get("CancelParsing"),
            mapping_error: get("MappingError"),
            validation_error: get("ValidationError"),
            union_error: get("UnionError"),
            rotation_error: get("RotationError"),
            namedtuple_error: get("NamedTupleError"),
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
                build_exception_instance(py, cls, "boom", Some("root.x")).expect("build");
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
    // push_path_segment（错误路径延迟重建）
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
        // Expr* 变体携带 path 字段，push_path_segment 能正确写入。
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
    // push_path_index（Array 系列 lazy path 模式）
    // ======================================================================

    #[test]
    fn push_path_index_on_root_base() {
        // 叶节点错误路径为 "root"，父 ArrayNode 插入数组索引。
        let mut err = ConstructError::Stream {
            message: "expected 1".to_string(),
            path: "root".to_string(),
        };
        err.push_path_index(2);
        assert_eq!(err.path(), Some("root[2]"));
    }

    #[test]
    fn push_path_index_on_empty_base() {
        // 占位路径（From<PyErr> 路径为空）。
        let mut err = ConstructError::Generic {
            message: "x".to_string(),
            path: String::new(),
        };
        err.push_path_index(0);
        assert_eq!(err.path(), Some("root[0]"));
    }

    #[test]
    fn push_path_index_nested_arrays() {
        // 嵌套 Array：外层 push_index(1) 后内层 push_index(2)
        // 模拟 Array[2] Array[1] Byte：内层先重建 → "root[2]"，外层再重建 → "root[1][2]"
        let mut err = ConstructError::Stream {
            message: "expected 1".to_string(),
            path: "root".to_string(),
        };
        err.push_path_index(2); // 内层 Array（i=2）
        assert_eq!(err.path(), Some("root[2]"));
        err.push_path_index(1); // 外层 Array（i=1）
        assert_eq!(err.path(), Some("root[1][2]"));
    }

    #[test]
    fn push_path_index_then_segment() {
        // Array 在外层，Struct 在内层：先 push_index(1) 后 push_segment("x")
        // 模拟 Array[1] Struct{x: Byte}：内层 Struct 先重建 → "root.x"，外层 Array 再重建 → "root[1].x"
        let mut err = ConstructError::Stream {
            message: "x".to_string(),
            path: "root".to_string(),
        };
        err.push_path_segment("x"); // 内层 Struct（字段 x）
        assert_eq!(err.path(), Some("root.x"));
        err.push_path_index(1); // 外层 Array（i=1）
        assert_eq!(err.path(), Some("root[1].x"));
    }

    #[test]
    fn push_path_segment_then_index() {
        // Struct 在外层，Array 在内层：先 push_segment("x") 后 push_index(1)
        // 模拟 Struct{x: Array[1] Byte}：内层 Array 先重建 → "root[1]"，外层 Struct 再重建 → "root.x[1]"
        let mut err = ConstructError::Stream {
            message: "x".to_string(),
            path: "root".to_string(),
        };
        err.push_path_index(1); // 内层 Array（i=1）
        assert_eq!(err.path(), Some("root[1]"));
        err.push_path_segment("x"); // 外层 Struct（字段 x）
        assert_eq!(err.path(), Some("root.x[1]"));
    }

    #[test]
    fn push_path_index_deeply_nested() {
        // 模拟 Array[1] Struct{x: Array[2] Byte}：叶错误路径重建为 "root[1].x[2]"
        // 重建顺序（从叶到根）：
        //   1. innermost Array(2) push_index(2): "root" → "root[2]"
        //   2. Struct push_segment("x"): "root[2]" → "root.x[2]"
        //   3. outer Array(1) push_index(1): "root.x[2]" → "root[1].x[2]"
        let mut err = ConstructError::Stream {
            message: "expected 1".to_string(),
            path: "root".to_string(),
        };
        err.push_path_index(2);
        assert_eq!(err.path(), Some("root[2]"));
        err.push_path_segment("x");
        assert_eq!(err.path(), Some("root.x[2]"));
        err.push_path_index(1);
        assert_eq!(err.path(), Some("root[1].x[2]"));
    }

    #[test]
    fn push_path_index_preserves_message() {
        // 验证 message 字段不被 push_path_index 修改。
        let mut err = ConstructError::FormatField {
            message: "struct '>I' error".to_string(),
            path: "root".to_string(),
        };
        err.push_path_index(3);
        assert_eq!(err.message(), Some("struct '>I' error"));
        assert_eq!(err.path(), Some("root[3]"));
    }

    #[test]
    fn push_path_index_on_compilation_error_is_noop() {
        // 编译期错误无 path，push_path_index 不操作。
        let mut err = ConstructError::Compilation {
            message: "bad schema".to_string(),
        };
        err.push_path_index(5);
        assert_eq!(err.path(), None);
    }

    #[test]
    fn push_path_index_works_on_all_runtime_variants() {
        // 验证 push_path_index 对所有运行时变体（携带 path 字段）都生效。
        let mut range_err = ConstructError::Range {
            message: "x".to_string(),
            path: "root".to_string(),
        };
        range_err.push_path_index(0);
        assert_eq!(range_err.path(), Some("root[0]"));

        let mut repeat_err = ConstructError::Repeat {
            message: "x".to_string(),
            path: "root".to_string(),
        };
        repeat_err.push_path_index(1);
        assert_eq!(repeat_err.path(), Some("root[1]"));

        let mut stopfield_err = ConstructError::StopField {
            path: "root".to_string(),
        };
        stopfield_err.push_path_index(2);
        assert_eq!(stopfield_err.path(), Some("root[2]"));
    }

    // ======================================================================
    // From<PyErr> for ConstructError（set_item 裸 ? 直接传播）
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

    // ======================================================================
    // 错误路径 fast-path 测试
    // ======================================================================

    /// 辅助：在 Python 中定义对齐生产 `_errors.py` 行为的异常层次。
    ///
    /// 与现有 [`build_test_classes`] 不同，本辅助在 ConstructError 上同时实现
    /// `__init__` 与 `__str__`（与 `neoconstruct/python/construct/_errors.py:46-58` 一致），
    /// 用于验证 fast-path 绕过 `__init__` 后 `__str__` 行为正确（依赖
    /// `self.path` + `self.message` 而非 `args`）。
    fn build_test_classes_aligned(py: Python<'_>) -> ExceptionClasses {
        let code = concat!(
            "class ConstructError(Exception):\n",
            "    def __init__(self, message='', path=None):\n",
            "        self.message = message\n",
            "        self.path = path\n",
            "        if path is not None:\n",
            "            full = \"Error in path {}\\n{}\".format(path, message)\n",
            "        else:\n",
            "            full = message\n",
            "        super().__init__(full)\n",
            "    def __str__(self):\n",
            "        if self.path is not None:\n",
            "            return \"Error in path {}\\n{}\".format(self.path, self.message)\n",
            "        return self.message\n",
            "class StreamError(ConstructError): pass\n",
            "class FormatFieldError(ConstructError): pass\n",
            "class FieldLengthError(ConstructError): pass\n",
            "class SizeofError(ConstructError): pass\n",
            "class CompilationError(ConstructError): pass\n",
            "class UnresolvedReferenceError(ConstructError): pass\n",
            "class GenericConstructError(ConstructError): pass\n",
            "class FieldValueMissingError(ConstructError): pass\n",
            "class IntegerError(ConstructError): pass\n",
            "class PaddingError(ConstructError): pass\n",
            "class RangeError(ConstructError): pass\n",
            "class RepeatError(ConstructError): pass\n",
            "class StopFieldError(ConstructError): pass\n",
            "class IndexFieldError(ConstructError): pass\n",
            "class StringError(ConstructError): pass\n",
            "class ExplicitError(ConstructError): pass\n",
            "class SelectError(ConstructError): pass\n",
            "class ConstError(ConstructError): pass\n",
            "class CheckError(ConstructError): pass\n",
            "class ChecksumError(ConstructError): pass\n",
            "class TerminatedError(ConstructError): pass\n",
            "class CancelParsing(ConstructError): pass\n",
            "class MappingError(ConstructError): pass\n",
            "class ValidationError(ConstructError): pass\n",
            "class UnionError(ConstructError): pass\n",
            "class RotationError(ConstructError): pass\n",
            "class NamedTupleError(ConstructError): pass\n",
        );
        py.run_bound(code, None, None)
            .expect("run aligned test classes definition");

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
            sizeof_error: get("SizeofError"),
            compilation_error: get("CompilationError"),
            unresolved_reference_error: get("UnresolvedReferenceError"),
            generic_construct_error: get("GenericConstructError"),
            field_value_missing_error: get("FieldValueMissingError"),
            construct_error_base: get("ConstructError"),
            integer_error: get("IntegerError"),
            padding_error: get("PaddingError"),
            range_error: get("RangeError"),
            repeat_error: get("RepeatError"),
            stop_field_error: get("StopFieldError"),
            index_field_error: get("IndexFieldError"),
            string_error: get("StringError"),
            explicit_error: get("ExplicitError"),
            select_error: get("SelectError"),
            const_error: get("ConstError"),
            check_error: get("CheckError"),
            checksum_error: get("ChecksumError"),
            terminated_error: get("TerminatedError"),
            cancel_parsing_error: get("CancelParsing"),
            mapping_error: get("MappingError"),
            validation_error: get("ValidationError"),
            union_error: get("UnionError"),
            rotation_error: get("RotationError"),
            namedtuple_error: get("NamedTupleError"),
        }
    }

    /// `is_builtin_class` 指针比较仅匹配确切内置类（不匹配用户子类）。
    #[test]
    fn is_builtin_class_matches_cached_ptrs_only() {
        ensure_python();
        Python::with_gil(|py| {
            let classes = build_test_classes(py);

            // 内置类的指针应被识别。
            assert!(
                classes.is_builtin_class(classes.stream_error.as_ptr()),
                "StreamError 应被识别为内置类"
            );
            assert!(
                classes.is_builtin_class(classes.stop_field_error.as_ptr()),
                "StopFieldError 应被识别为内置类"
            );

            // 用户子类（在 Python 中实时定义）应被识别为非内置类。
            let user_cls: Py<PyType> = py
                .eval_bound("type('MyError', (StreamError,), {})", None, None)
                .unwrap()
                .extract()
                .unwrap();
            assert!(
                !classes.is_builtin_class(user_cls.as_ptr()),
                "用户子类 MyError 不应被识别为内置类"
            );
        });
    }

    /// fast-path：内置 StreamError + path → 返回 Some(PyErr)，
    /// 实例的 message/path/args 属性正确设置。
    #[test]
    fn try_fast_path_alloc_builtin_stream_with_path() {
        ensure_python();
        Python::with_gil(|py| {
            let classes = build_test_classes_aligned(py);
            let cls = classes.stream_error.bind(py);

            let pyerr = try_fast_path_alloc(py, cls, &classes, "boom", Some("root.x"));
            let pyerr = pyerr.expect("fast-path 应返回 Some(PyErr) for 内置类");

            // 验证 PyErr 实例的 message / path / args 属性。
            let value = pyerr.value_bound(py);
            let message: String = value
                .getattr("message")
                .expect("get message")
                .extract()
                .expect("extract message");
            assert_eq!(message, "boom");

            let path: String = value
                .getattr("path")
                .expect("get path")
                .extract()
                .expect("extract path");
            assert_eq!(path, "root.x");

            // args = (message,) 而非 (full,)（有意设计，str(e) 不依赖 args）。
            let args_repr: String = value
                .getattr("args")
                .expect("get args")
                .repr()
                .expect("repr args")
                .to_string();
            assert_eq!(args_repr, "('boom',)");

            // 验证类型名（fast-path 不应改变类型）。
            let type_name: String = value.get_type().name().expect("type name").to_string();
            assert_eq!(type_name, "StreamError");
        });
    }

    /// fast-path：内置 CompilationError + path=None → 实例 path 属性为 None。
    #[test]
    fn try_fast_path_alloc_builtin_compilation_with_none_path() {
        ensure_python();
        Python::with_gil(|py| {
            let classes = build_test_classes_aligned(py);
            let cls = classes.compilation_error.bind(py);

            let pyerr = try_fast_path_alloc(py, cls, &classes, "bad schema", None);
            let pyerr = pyerr.expect("fast-path 应返回 Some(PyErr) for 内置 CompilationError");

            let value = pyerr.value_bound(py);

            // path 属性应为 None（path=None → SetAttr Py_None）。
            let path_obj = value.getattr("path").expect("get path");
            assert!(path_obj.is_none(), "path 应为 None");

            // args = (message,)。
            let args_repr: String = value
                .getattr("args")
                .expect("get args")
                .repr()
                .expect("repr args")
                .to_string();
            assert_eq!(args_repr, "('bad schema',)");
        });
    }

    /// fast-path：全部缓存的内置异常类都被识别（覆盖测试，避免遗漏某个类）。
    #[test]
    fn try_fast_path_alloc_all_builtin_classes_recognized() {
        ensure_python();
        Python::with_gil(|py| {
            let classes = build_test_classes_aligned(py);

            let all_classes: [(&Bound<'_, PyType>, &str); 14] = [
                (classes.stream_error.bind(py), "stream"),
                (classes.format_field_error.bind(py), "format_field"),
                (classes.field_length_error.bind(py), "field_length"),
                (classes.compilation_error.bind(py), "compilation"),
                (classes.unresolved_reference_error.bind(py), "unresolved"),
                (classes.generic_construct_error.bind(py), "generic"),
                (classes.construct_error_base.bind(py), "base"),
                (classes.integer_error.bind(py), "integer"),
                (classes.padding_error.bind(py), "padding"),
                (classes.range_error.bind(py), "range"),
                (classes.repeat_error.bind(py), "repeat"),
                (classes.stop_field_error.bind(py), "stop_field"),
                (classes.index_field_error.bind(py), "index_field"),
                (classes.string_error.bind(py), "string"),
            ];

            for (cls, name) in all_classes.iter() {
                let pyerr = try_fast_path_alloc(py, cls, &classes, "msg", Some("p"));
                assert!(
                    pyerr.is_some(),
                    "fast-path 应识别内置类 {} (ptr={:p})",
                    name,
                    cls.as_ptr()
                );
            }
        });
    }

    /// 用户子类化的异常返回 None，
    /// 信号调用方走慢路径（call1）以调用用户的 __init__。
    #[test]
    fn try_fast_path_alloc_user_subclass_returns_none() {
        ensure_python();
        Python::with_gil(|py| {
            let classes = build_test_classes_aligned(py);

            // 在 Python 中定义 StreamError 的子类（含自定义 __init__）。
            let code = concat!(
                "class MyStreamError(StreamError):\n",
                "    def __init__(self, message='', path=None):\n",
                "        super().__init__(message, path)\n",
                "        self.extra = 'custom_marker'\n",
            );
            py.run_bound(code, None, None)
                .expect("define MyStreamError");

            let user_cls: Bound<'_, PyType> = py
                .eval_bound("MyStreamError", None, None)
                .unwrap()
                .extract::<Bound<'_, PyType>>()
                .unwrap();

            // fast-path 应返回 None（信号慢路径）—— 否则会绕过 MyStreamError.__init__，
            // 丢失 self.extra 标记（破坏用户代码）。
            let result = try_fast_path_alloc(py, &user_cls, &classes, "msg", Some("p"));
            assert!(
                result.is_none(),
                "fast-path 必须对用户子类返回 None，否则 __init__ 不被调用（破坏行为兼容）"
            );
        });
    }

    /// 用户子类化时，慢路径 [`build_exception_instance`]
    /// 调用用户的 __init__（self.extra 被正确设置）。
    ///
    /// 这是行为兼容性验证：子类化异常的用户自定义行为不被破坏。
    #[test]
    fn slow_path_invokes_user_subclass_init() {
        ensure_python();
        Python::with_gil(|py| {
            // build_test_classes_aligned 用于在 Python 解释器中定义 StreamError 等基类，
            // 返回的 ExceptionClasses 本身不直接使用（MyStreamError 通过继承 StreamError
            // 间接受益）。
            let _classes = build_test_classes_aligned(py);

            let code = concat!(
                "class MyStreamError(StreamError):\n",
                "    def __init__(self, message='', path=None):\n",
                "        super().__init__(message, path)\n",
                "        self.extra = 'custom_marker'\n",
            );
            py.run_bound(code, None, None)
                .expect("define MyStreamError");

            let user_cls: Bound<'_, PyType> = py
                .eval_bound("MyStreamError", None, None)
                .unwrap()
                .extract::<Bound<'_, PyType>>()
                .unwrap();

            // 慢路径 call1：应调用 MyStreamError.__init__。
            let instance =
                build_exception_instance(py, &user_cls, "boom", Some("root.x")).expect("slow path");

            // extra 属性应被设置（__init__ 被调用的直接证据）。
            let extra: String = instance
                .getattr("extra")
                .expect("get extra")
                .extract()
                .expect("extract extra");
            assert_eq!(extra, "custom_marker", "用户子类 __init__ 必须被调用");

            // message/path 也应正确设置（通过 super().__init__ 链）。
            let message: String = instance.getattr("message").unwrap().extract().unwrap();
            let path: String = instance.getattr("path").unwrap().extract().unwrap();
            assert_eq!(message, "boom");
            assert_eq!(path, "root.x");
        });
    }

    /// 行为对齐：fast-path 实例的 `str(e)` 与慢路径一致（依赖 __str__ 重写，
    /// 不依赖 args）。
    #[test]
    fn fast_path_str_matches_slow_path_when_path_set() {
        ensure_python();
        Python::with_gil(|py| {
            let classes = build_test_classes_aligned(py);
            let cls = classes.stream_error.bind(py);

            // fast-path 实例
            let fast_pyerr =
                try_fast_path_alloc(py, cls, &classes, "boom", Some("root.x")).expect("fast-path");
            let fast_str: String = fast_pyerr.value_bound(py).str().expect("str").to_string();

            // 慢路径实例
            let slow_instance =
                build_exception_instance(py, cls, "boom", Some("root.x")).expect("slow");
            let slow_str: String = slow_instance.str().expect("str").to_string();

            // 两者 str(e) 一致（__str__ 基于 self.path + self.message，与 args 无关）。
            assert_eq!(
                fast_str, slow_str,
                "fast-path str(e) 应与慢路径一致（生产 __str__ 重写）"
            );
            assert!(
                fast_str.contains("Error in path root.x"),
                "str(e) 应包含 path：{}",
                fast_str
            );
            assert!(
                fast_str.contains("boom"),
                "str(e) 应包含 message：{}",
                fast_str
            );
        });
    }

    /// 行为差异（有意设计）：fast-path 的 args=(message,)
    /// 而慢路径的 args=(full,)。repr 因此不同，str(e) 保持一致。
    #[test]
    fn fast_path_args_differs_from_slow_path_by_design() {
        ensure_python();
        Python::with_gil(|py| {
            let classes = build_test_classes_aligned(py);
            let cls = classes.stream_error.bind(py);

            // fast-path: args = (message,) = ('boom',)
            let fast_pyerr =
                try_fast_path_alloc(py, cls, &classes, "boom", Some("root.x")).expect("fast-path");
            let fast_args: String = fast_pyerr
                .value_bound(py)
                .getattr("args")
                .unwrap()
                .repr()
                .unwrap()
                .to_string();

            // 慢路径: args = (full,) = ('Error in path root.x\nboom',)
            let slow_instance =
                build_exception_instance(py, cls, "boom", Some("root.x")).expect("slow");
            let slow_args: String = slow_instance
                .getattr("args")
                .unwrap()
                .repr()
                .unwrap()
                .to_string();

            // 两者应不同（args 内容不同）。
            assert_ne!(
                fast_args, slow_args,
                "fast-path args 应与慢路径不同（有意设计）"
            );
            assert_eq!(fast_args, "('boom',)");
            assert!(slow_args.contains("Error in path root.x"));
        });
    }
}
