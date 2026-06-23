//! BytesNode：固定/表达式长度字节读写。
//!
//! 设计依据：`docs/架构设计.md` §C.3.2、`docs/模块设计-表达式系统.md` §4.7。
//! Python 参考：`construct/construct/core.py` `Bytes`（L924-980）。
//!
//! ## Phase 2 范围
//!
//! `length` 可以是编译期常量（[`BytesLength::Const`]）或表达式程序
//! （[`BytesLength::Expr`]）。表达式长度在 parse 时通过 [`crate::expr::eval_expr_int`]
//! 求值得到（如 `Bytes(count)` / `Bytes(count + 1)`）。
//!
//! ## REV 约束（Phase 1 遗留）
//!
//! build 时**仅接受 `bytes` 输入**。int/bytearray 转换推迟到后续阶段。

use crate::context::Context;
use crate::error::ConstructError;
use crate::expr::{eval_expr_int, ExprProgram};
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;
use pyo3::types::PyBytes;

// ---------------------------------------------------------------------------
// BytesLength
// ---------------------------------------------------------------------------

/// Bytes 的长度来源。
///
/// 设计依据：`docs/模块设计-表达式系统.md` §4.7。
#[derive(Debug, Clone)]
pub enum BytesLength {
    /// 固定长度（Phase 1 兼容）。
    Const(usize),
    /// 表达式长度（Phase 2 新增）。parse 时求值为 `usize`。
    Expr(ExprProgram),
}

impl BytesLength {
    /// 如果是常量长度，返回 `Some(&n)`，否则 `None`。
    pub fn as_const(&self) -> Option<&usize> {
        match self {
            BytesLength::Const(n) => Some(n),
            BytesLength::Expr(_) => None,
        }
    }

    /// 是否为常量长度。
    pub fn is_const(&self) -> bool {
        matches!(self, BytesLength::Const(_))
    }

    /// 是否为表达式长度。
    pub fn is_expr(&self) -> bool {
        matches!(self, BytesLength::Expr(_))
    }
}

// ---------------------------------------------------------------------------
// BytesNode
// ---------------------------------------------------------------------------

/// 字节字段节点：读取/写入 `length` 字节的原始数据。
///
/// `length` 可为编译期常量（[`BytesLength::Const`]）或表达式程序
/// （[`BytesLength::Expr`]），后者在 parse 时通过 [`eval_expr_int`] 求值。
///
/// 对应 Python construct 的 `Bytes(length)`。
///
/// # parse 行为
///
/// - [`BytesLength::Const(n)`]：从流中读取 `n` 字节。
/// - [`BytesLength::Expr(prog)`]：求值表达式 → `i64` → 负数返回 `FieldLength` 错误，
///   否则 `as usize` 后读取。求值使用 `ctx.field_names()` 作为字段名表。
///
/// 读取后创建 `PyBytes` 返回。不足时返回 `Stream` 错误。
///
/// # build 行为
///
/// 校验输入对象为 `bytes` 类型，并校验数据长度是否匹配声明的 `length`
/// （SF-3 修复：对齐 Python construct `stream_write` 的长度校验行为）。
/// 长度不匹配时返回 `FieldLength` 错误。表达式长度通过 `ctx.field_names()`
/// 求值表达式获取期望长度。
///
/// # sizeof
///
/// - [`BytesLength::Const(n)`] → `Ok(n)`
/// - [`BytesLength::Expr`] → `Err`（大小依赖运行时 context，编译期不可知）
#[derive(Debug, Clone)]
pub struct BytesNode {
    /// 长度来源：常量或表达式。
    length: BytesLength,
}

impl BytesNode {
    /// Phase 1 兼容构造器：固定长度。
    pub fn new_const(length: usize) -> Self {
        Self {
            length: BytesLength::Const(length),
        }
    }

    /// Phase 2 新增：表达式长度。
    ///
    /// parse 时通过 [`eval_expr_int`] 求值 `program` 得到字节数。
    pub fn new_expr(program: ExprProgram) -> Self {
        Self {
            length: BytesLength::Expr(program),
        }
    }

    /// 返回长度来源的引用。
    pub fn length(&self) -> &BytesLength {
        &self.length
    }
}

// ---------------------------------------------------------------------------
// Construct impl
// ---------------------------------------------------------------------------

impl super::Construct for BytesNode {
    fn parse(
        &self,
        py: Python<'_>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let length = match &self.length {
            BytesLength::Const(n) => *n,
            BytesLength::Expr(prog) => {
                let names = ctx.field_names().ok_or_else(|| ConstructError::Generic {
                    message: "Bytes expression length requires field_names in context".to_string(),
                    path: path.to_string(),
                })?;
                let n = eval_expr_int(prog, names, ctx, py).map_err(|e| {
                    // Expr* 错误不携带 path，转换为 Generic 以暴露出错位置。
                    ConstructError::Generic {
                        message: format!(
                            "Bytes length expression evaluation failed: {}",
                            e.full_message()
                        ),
                        path: path.to_string(),
                    }
                })?;
                if n < 0 {
                    return Err(ConstructError::FieldLength {
                        message: format!("Bytes length expression evaluated to negative: {}", n),
                        path: path.to_string(),
                    });
                }
                n as usize
            }
        };
        let data = stream.read(length, path)?;
        // PyBytes::new_bound 拷贝 data 到 Python 堆。
        Ok(PyBytes::new_bound(py, data).into_any().unbind())
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        // REV 约束：仅接受 bytes（int/bytearray 转换推迟到后续阶段）
        let py_bytes = obj
            .downcast::<PyBytes>()
            .map_err(|_| ConstructError::Generic {
                message: format!(
                    "expected bytes object for Bytes field, received non-bytes value of type {}",
                    obj.get_type()
                        .name()
                        .map(|n| n.to_string())
                        .unwrap_or_else(|_| "<unknown>".to_string()),
                ),
                path: path.to_string(),
            })?;
        let data = py_bytes.as_bytes();

        // SF-3 修复：校验长度（对齐 Python construct 的 stream_write 行为）。
        let expected = match &self.length {
            BytesLength::Const(n) => *n,
            BytesLength::Expr(prog) => {
                let names = ctx.field_names().ok_or_else(|| ConstructError::Generic {
                    message: "Bytes expression build requires context with field_names".to_string(),
                    path: path.to_string(),
                })?;
                let n =
                    eval_expr_int(prog, names, ctx, py).map_err(|e| ConstructError::Generic {
                        message: format!(
                            "Bytes length expression evaluation failed during build: {}",
                            e.full_message()
                        ),
                        path: path.to_string(),
                    })?;
                if n < 0 {
                    return Err(ConstructError::FieldLength {
                        message: format!("Bytes length expression evaluated to negative: {}", n),
                        path: path.to_string(),
                    });
                }
                n as usize
            }
        };
        if data.len() != expected {
            return Err(ConstructError::FieldLength {
                message: format!(
                    "Bytes build length mismatch: expected {}, got {}",
                    expected,
                    data.len()
                ),
                path: path.to_string(),
            });
        }
        stream.write(data);
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        match &self.length {
            BytesLength::Const(n) => Ok(*n),
            BytesLength::Expr(_) => {
                // 表达式长度的 Bytes 在编译期无法求值大小（需要 context + GIL）。
                // 对齐 GreedyBytes 的 sizeof 行为：用 Generic 错误表示大小未知。
                Err(ConstructError::Generic {
                    message: "Bytes with expression length has no static size".to_string(),
                    path: String::new(),
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{ExprOp, ExprProgram};
    use crate::nodes::Construct;
    use pyo3::types::PyString;

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

    /// 构造 interned PyString 列表（模拟 StructNode 的字段名表）。
    fn make_names<'py>(py: Python<'py>, names: &[&str]) -> Vec<Py<PyString>> {
        names
            .iter()
            .map(|n| PyString::new_bound(py, n).into())
            .collect()
    }

    /// 在 ctx 中设置字段值 + field_names，模拟 StructNode parse 环境。
    fn setup_ctx_for_expr<'py>(
        py: Python<'py>,
        field_values: &[(&str, i64)],
        names: &[&str],
    ) -> Context<'py> {
        let mut ctx = Context::new_root(py).expect("ctx");
        for (name, value) in field_values {
            let val = (*value).into_py(py);
            ctx.set_field(name, val.bind(py)).expect("set_field");
        }
        ctx.set_field_names(make_names(py, names));
        ctx
    }

    // ======================================================================
    // BytesNode 构造器 & BytesLength
    // ======================================================================

    #[test]
    fn new_const_creates_const_length() {
        let node = BytesNode::new_const(4);
        assert!(matches!(node.length(), BytesLength::Const(4)));
    }

    #[test]
    fn new_const_zero() {
        let node = BytesNode::new_const(0);
        assert!(matches!(node.length(), BytesLength::Const(0)));
    }

    #[test]
    fn new_expr_creates_expr_length() {
        let prog = ExprProgram::new(vec![ExprOp::GetInt(0)]);
        let node = BytesNode::new_expr(prog);
        assert!(matches!(node.length(), BytesLength::Expr(_)));
    }

    #[test]
    fn clone_preserves_length_variant() {
        let const_node = BytesNode::new_const(7);
        let cloned = const_node.clone();
        assert!(matches!(cloned.length(), BytesLength::Const(7)));

        let prog = ExprProgram::new(vec![ExprOp::Const(5)]);
        let expr_node = BytesNode::new_expr(prog);
        let cloned = expr_node.clone();
        assert!(matches!(cloned.length(), BytesLength::Expr(_)));
    }

    // ======================================================================
    // parse：Const 路径（Phase 1 兼容）
    // ======================================================================

    #[test]
    fn parse_const_reads_exact_length() {
        with_py(|py| {
            let node = BytesNode::new_const(4);
            let mut stream = ParseStream::new(b"hello world");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().expect("extract bytes");
            assert_eq!(bytes, b"hell");
            assert_eq!(stream.tell(), 4);
        });
    }

    #[test]
    fn parse_const_zero_length_returns_empty_bytes() {
        with_py(|py| {
            let node = BytesNode::new_const(0);
            let mut stream = ParseStream::new(b"abc");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert!(bytes.is_empty());
            assert_eq!(stream.tell(), 0);
        });
    }

    #[test]
    fn parse_const_insufficient_bytes_returns_stream_error() {
        with_py(|py| {
            let node = BytesNode::new_const(5);
            let mut stream = ParseStream::new(b"abc");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Stream { message, .. } => {
                    assert!(message.contains("expected 5"), "got: {}", message);
                    assert!(message.contains("found 3"), "got: {}", message);
                }
                other => panic!("expected Stream error, got {:?}", other),
            }
        });
    }

    #[test]
    fn parse_const_full_length_consumes_stream() {
        with_py(|py| {
            let node = BytesNode::new_const(5);
            let mut stream = ParseStream::new(b"hello");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(bytes, b"hello");
            assert!(stream.is_at_end());
        });
    }

    // ======================================================================
    // parse：Expr 路径（Phase 2 新增）
    // ======================================================================

    #[test]
    fn parse_expr_simple_field_reference() {
        // Bytes(count) → count=4 → 读 4 字节
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0)]);
            let node = BytesNode::new_expr(prog);
            let mut stream = ParseStream::new(b"ABCDE");
            let mut ctx = setup_ctx_for_expr(py, &[("count", 4)], &["count"]);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(bytes, b"ABCD");
            assert_eq!(stream.tell(), 4);
        });
    }

    #[test]
    fn parse_expr_arithmetic_addition() {
        // Bytes(count + 1) → count=3 → length=4
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(1), ExprOp::Add]);
            let node = BytesNode::new_expr(prog);
            let mut stream = ParseStream::new(b"XYZAB");
            let mut ctx = setup_ctx_for_expr(py, &[("count", 3)], &["count"]);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(bytes, b"XYZA");
        });
    }

    #[test]
    fn parse_expr_multiplication() {
        // Bytes(count * 2) → count=3 → length=6
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(2), ExprOp::Mul]);
            let node = BytesNode::new_expr(prog);
            let mut stream = ParseStream::new(b"ABCDEF123");
            let mut ctx = setup_ctx_for_expr(py, &[("count", 3)], &["count"]);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(bytes, b"ABCDEF");
        });
    }

    #[test]
    fn parse_expr_length_zero() {
        // 表达式求值为 0 → 读取 0 字节
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::Const(0)]);
            let node = BytesNode::new_expr(prog);
            let mut stream = ParseStream::new(b"abc");
            let mut ctx = setup_ctx_for_expr(py, &[], &["unused"]);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert!(bytes.is_empty());
        });
    }

    #[test]
    fn parse_expr_negative_length_returns_field_length_error() {
        // 表达式求值为 -1 → FieldLength 错误
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::Const(-1)]);
            let node = BytesNode::new_expr(prog);
            let mut stream = ParseStream::new(b"abc");
            let mut ctx = setup_ctx_for_expr(py, &[], &["unused"]);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::FieldLength { message, .. } => {
                    assert!(message.contains("negative"), "got: {}", message);
                }
                other => panic!("expected FieldLength, got {:?}", other),
            }
        });
    }

    #[test]
    fn parse_expr_without_field_names_returns_error() {
        // ctx.field_names() 为 None → Generic 错误
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0)]);
            let node = BytesNode::new_expr(prog);
            let mut stream = ParseStream::new(b"abc");
            let mut ctx = Context::new_root(py).expect("ctx"); // 未设置 field_names
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Generic { .. }));
        });
    }

    #[test]
    fn parse_expr_insufficient_bytes_returns_stream_error() {
        // 表达式求值为 5，但流只有 3 字节 → Stream 错误
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::Const(5)]);
            let node = BytesNode::new_expr(prog);
            let mut stream = ParseStream::new(b"abc");
            let mut ctx = setup_ctx_for_expr(py, &[], &["unused"]);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Stream { .. }));
        });
    }

    // ======================================================================
    // build（Const + Expr：校验长度，SF-3 修复）
    // ======================================================================

    #[test]
    fn build_writes_matching_bytes_const() {
        with_py(|py| {
            let node = BytesNode::new_const(4);
            let obj = py.eval_bound("b'beef'", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), b"beef");
        });
    }

    #[test]
    fn build_zero_length_accepts_empty_bytes() {
        with_py(|py| {
            let node = BytesNode::new_const(0);
            let obj = py.eval_bound("b''", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert!(stream.as_bytes().is_empty());
        });
    }

    #[test]
    fn build_validates_length_mismatch_too_short() {
        // SF-3 修复：build 校验长度。传 2 字节但声明 4 字节 → FieldLength 错误。
        with_py(|py| {
            let node = BytesNode::new_const(4);
            let obj = py.eval_bound("b'ab'", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail (length mismatch)");
            match err {
                ConstructError::FieldLength { message, .. } => {
                    assert!(message.contains("expected 4"), "got: {}", message);
                    assert!(message.contains("got 2"), "got: {}", message);
                }
                other => panic!("expected FieldLength, got {:?}", other),
            }
        });
    }

    #[test]
    fn build_validates_length_mismatch_too_long() {
        // SF-3 修复：声明 2 字节，传 5 字节 → FieldLength 错误。
        with_py(|py| {
            let node = BytesNode::new_const(2);
            let obj = py.eval_bound("b'hello'", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail (length mismatch)");
            assert!(matches!(err, ConstructError::FieldLength { .. }));
        });
    }

    #[test]
    fn build_expr_validates_length() {
        // SF-3 修复：Expr 路径的 build 也校验长度。
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::Const(3)]);
            let node = BytesNode::new_expr(prog);
            let obj = py.eval_bound("b'XYZ'", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = setup_ctx_for_expr(py, &[], &["unused"]);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build (matching length)");
            assert_eq!(stream.as_bytes(), b"XYZ");
        });
    }

    #[test]
    fn build_expr_rejects_length_mismatch() {
        // SF-3 修复：Expr 路径 build 长度不匹配 → FieldLength 错误。
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::Const(10)]);
            let node = BytesNode::new_expr(prog);
            let obj = py.eval_bound("b'XYZ'", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = setup_ctx_for_expr(py, &[], &["unused"]);
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail (length mismatch)");
            assert!(matches!(err, ConstructError::FieldLength { .. }));
        });
    }

    #[test]
    fn build_rejects_int_input() {
        // REV 约束：int 转换推迟
        with_py(|py| {
            let node = BytesNode::new_const(4);
            let obj = py.eval_bound("0", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Generic { .. }));
        });
    }

    #[test]
    fn build_rejects_str_input() {
        with_py(|py| {
            let node = BytesNode::new_const(5);
            let obj = py.eval_bound("'hello'", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Generic { .. }));
        });
    }

    #[test]
    fn build_rejects_bytearray_input() {
        // REV 约束：bytearray 转换推迟
        with_py(|py| {
            let node = BytesNode::new_const(5);
            let obj = py
                .eval_bound("bytearray(b'hello')", None, None)
                .expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Generic { .. }));
        });
    }

    // ======================================================================
    // parse ↔ build 往返
    // ======================================================================

    #[test]
    fn round_trip_const_bytes_preserves_data() {
        with_py(|py| {
            let node = BytesNode::new_const(6);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            let obj = py.eval_bound("b'ABCDEF'", None, None).expect("eval");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();

            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let parsed: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(parsed, b"ABCDEF");
        });
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_const_returns_length() {
        with_py(|py| {
            let ctx = Context::new_root(py).expect("ctx");
            assert_eq!(BytesNode::new_const(0).sizeof(&ctx).unwrap(), 0);
            assert_eq!(BytesNode::new_const(1).sizeof(&ctx).unwrap(), 1);
            assert_eq!(BytesNode::new_const(100).sizeof(&ctx).unwrap(), 100);
        });
    }

    #[test]
    fn sizeof_expr_returns_error() {
        // 表达式长度的 Bytes，sizeof 应返回 Err（大小编译期未知）
        with_py(|py| {
            let ctx = Context::new_root(py).expect("ctx");
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0)]);
            let node = BytesNode::new_expr(prog);
            let err = node.sizeof(&ctx).expect_err("should fail");
            assert!(matches!(err, ConstructError::Generic { .. }));
        });
    }
}
