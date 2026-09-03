//! SeekNode：流定位节点（Phase 7.2 §3.2）。
//!
//! Python 参考：`construct/construct/core.py` `Seek`（L4594-4640）。
//!
//! ## 行为概述
//!
//! Seek 是纯流定位构造器：parse/build 执行 stream.seek 后返回（不读/写实际数据）。
//! Python construct 用 `flagbuildnone=True` 使 build 接受 None 输入；construct-rs
//! build 签名接收任意 `&Bound<PyAny>`，Seek 忽略 obj 内容（仅求 at/whence 定位），
//! 自然兼容 None。
//!
//! ## at 参数（int 或表达式）
//!
//! - 常量（Python int）→ [`SeekOffset::Const`]
//! - 表达式（FieldRef/ExprRef）→ [`SeekOffset::Expr`]，运行时调
//!   [`crate::expr::eval_expr_int`] 求值（零 FFI）
//!
//! ## whence 参数（int 0/1/2）
//!
//! 编译期从 Python 描述符读取，翻译为 [`crate::stream::Whence`] enum。
//! 默认 0（Start）。

use crate::context::Context;
use crate::error::ConstructError;
use crate::expr::{eval_expr_int, ExprProgram};
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream, Whence};
use pyo3::prelude::*;

use super::Construct;

/// 流定位节点：parse/build 执行 stream.seek，返回新位置。
///
/// 对应 Python construct `Seek(at, whence=0)`（core.py L4594）。
///
/// # 三方法行为
///
/// - parse：求 at（常量或 ExprProgram）→ `stream.seek_whence(at, whence, path)` →
///   返回新位置 `PyLong(stream.tell())`（对齐 Python `_parse` 返回 `stream_seek` 返回值）
/// - build：求 at → `stream.seek(at, whence, path)` → 返回 `Ok(())`
///   （construct-rs build 签名不返回值；Python `_build` 返回 stream_seek 返回值被
///   Sequence 忽略）
/// - sizeof：永远 `Err`（对齐 Python `SizeofError`）
///
/// # flagbuildnone 兼容
///
/// Python `Seek.__init__` 设 `flagbuildnone=True`（build 接受 None 输入）。
/// construct-rs build 签名接收 `&Bound<PyAny>`，Seek build 忽略 obj 内容
/// （仅求 at/whence 定位），自然兼容 None。
#[derive(Debug)]
pub struct SeekNode {
    /// 定位目标：编译期常量或运行时表达式。
    at: SeekOffset,
    /// 定位参考点（编译期常量，Python whence 通常是 int 字面量）。
    whence: Whence,
}

/// Seek 的定位目标，对应 Python `at` 参数（int 或 context lambda）。
#[derive(Debug)]
pub enum SeekOffset {
    /// 编译期常量（如 `Seek(5)`）。
    Const(i64),
    /// 表达式（如 `Seek(offset)`），编译期从 FieldRef/ExprRef 翻译为 ExprProgram。
    Expr(ExprProgram),
}

impl SeekNode {
    /// 创建 `SeekNode`。
    pub fn new(at: SeekOffset, whence: Whence) -> Self {
        Self { at, whence }
    }

    /// 返回定位目标的引用。
    pub fn at(&self) -> &SeekOffset {
        &self.at
    }

    /// 返回 whence（Copy）。
    pub fn whence(&self) -> Whence {
        self.whence
    }

    /// has_expressions：仅 Expr 变体为 true（与 ArrayNode/PointerNode 同模式）。
    pub fn has_expressions(&self) -> bool {
        matches!(self.at, SeekOffset::Expr(_))
    }

    /// 求值 at（Const 直接返回，Expr 调 eval_expr_int）。
    ///
    /// Expr 路径失败时 push_path_segment("at")，对齐 PrefixedArray 的 countfield 错误处理
    /// （ADR-016 lazy path：错误自带 path，由 push_path_segment 补充字段名段）。
    fn eval_at(&self, ctx: &Context<'_>, py: Python<'_>) -> Result<i64, ConstructError> {
        match &self.at {
            SeekOffset::Const(n) => Ok(*n),
            SeekOffset::Expr(prog) => eval_expr_int(prog, ctx, py).map_err(|mut e| {
                e.push_path_segment("at");
                e
            }),
        }
    }
}

impl Construct for SeekNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let at = self.eval_at(ctx, py)?;
        stream.seek_whence(at, self.whence, path)?;
        // 返回新位置（对齐 Python _parse 返回 stream_seek 返回值）
        Ok(stream.tell().into_py(py))
    }

    fn build(
        &self,
        py: Python<'_>,
        _obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        // Seek 忽略 obj（flagbuildnone 兼容：build 接受 None 或任意值）
        let at = self.eval_at(ctx, py)?;
        stream.seek(at, self.whence, path)
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        Err(ConstructError::Generic {
            message: "Seek only moves the stream, size is not meaningful".to_string(),
            path: String::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::ExprOp;
    use crate::nodes::bytes::BytesNode;
    use crate::nodes::Construct;
    use pyo3::types::{PyList, PyString};

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

    /// 构造一个含若干整数字段的 Context，用于表达式求值测试。
    fn make_context<'py>(py: Python<'py>, entries: &[(&str, i64)]) -> Context<'py> {
        let mut ctx = Context::new_root(py).expect("new_root");
        ctx.init_expr_values(entries.len());
        for (idx, (name, value)) in entries.iter().enumerate() {
            let key = PyString::new_bound(py, name).unbind();
            let val = (*value).into_py(py);
            ctx.set_field_at(idx, &key, val.bind(py), py)
                .expect("set_field_at");
        }
        ctx
    }

    // ======================================================================
    // 构造器 & 访问器
    // ======================================================================

    #[test]
    fn new_stores_const_at_and_whence() {
        let node = SeekNode::new(SeekOffset::Const(5), Whence::Start);
        assert!(matches!(node.at(), SeekOffset::Const(5)));
        assert_eq!(node.whence(), Whence::Start);
    }

    #[test]
    fn new_stores_expr_at() {
        let prog = ExprProgram::new(vec![ExprOp::Const(3)]);
        let node = SeekNode::new(SeekOffset::Expr(prog), Whence::Current);
        assert!(matches!(node.at(), SeekOffset::Expr(_)));
        assert_eq!(node.whence(), Whence::Current);
    }

    #[test]
    fn has_expressions_const_returns_false() {
        let node = SeekNode::new(SeekOffset::Const(5), Whence::Start);
        assert!(!node.has_expressions());
    }

    #[test]
    fn has_expressions_expr_returns_true() {
        let prog = ExprProgram::new(vec![ExprOp::Const(3)]);
        let node = SeekNode::new(SeekOffset::Expr(prog), Whence::Start);
        assert!(node.has_expressions());
    }

    // ======================================================================
    // parse — SK1: 常量 at，whence=Start
    // ======================================================================

    #[test]
    fn parse_sk1_const_at_whence_start() {
        with_py(|py| {
            let node = SeekNode::new(SeekOffset::Const(5), Whence::Start);
            let mut stream = ParseStream::new(b"01234x");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // pos 应为 5
            assert_eq!(stream.tell(), 5);
            // 返回值应为 PyLong(5)
            let n: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(n, 5);
        });
    }

    // ======================================================================
    // parse — SK2: 表达式 at
    // ======================================================================

    #[test]
    fn parse_sk2_expr_at() {
        with_py(|py| {
            // Seek(off), off=3
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0)]);
            let node = SeekNode::new(SeekOffset::Expr(prog), Whence::Start);
            let mut stream = ParseStream::new(b"0123456789");
            let mut ctx = make_context(py, &[("off", 3)]);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert_eq!(stream.tell(), 3);
            let n: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(n, 3);
        });
    }

    // ======================================================================
    // parse — SK3: whence=Current
    // ======================================================================

    #[test]
    fn parse_sk3_whence_current() {
        with_py(|py| {
            // 先读 3 字节让 tell=3，再 Seek(2, whence=1) → pos=5
            let node = SeekNode::new(SeekOffset::Const(2), Whence::Current);
            let mut stream = ParseStream::new(b"abcde");
            let _ = stream.read(3, &Path::new()).expect("read 3");
            assert_eq!(stream.tell(), 3);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert_eq!(stream.tell(), 5);
            let n: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(n, 5);
        });
    }

    // ======================================================================
    // parse — SK4: whence=End（从末尾向前）
    // ======================================================================

    #[test]
    fn parse_sk4_whence_end() {
        with_py(|py| {
            // data.len()=10, Seek(-2, whence=2) → pos=8
            let node = SeekNode::new(SeekOffset::Const(-2), Whence::End);
            let mut stream = ParseStream::new(b"0123456789");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert_eq!(stream.tell(), 8);
            let n: i64 = result.bind(py).extract().expect("extract");
            assert_eq!(n, 8);
        });
    }

    // ======================================================================
    // build — SK5: 接受 None（flagbuildnone 兼容）
    // ======================================================================

    #[test]
    fn build_sk5_accepts_none() {
        with_py(|py| {
            let node = SeekNode::new(SeekOffset::Const(5), Whence::Start);
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            stream.write(b"0123456789");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.tell(), 5);
            // buf 不变（覆盖写或追加未发生）
            assert_eq!(stream.written_len(), 10);
        });
    }

    #[test]
    fn build_seek_zero_pads_on_next_write() {
        // build Seek(8) 后写一字节 → 零填充
        with_py(|py| {
            let node = SeekNode::new(SeekOffset::Const(8), Whence::Start);
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.tell(), 8);
            assert_eq!(stream.written_len(), 0);
            // 接着写一字节
            stream.write(b"X");
            assert_eq!(stream.written_len(), 9);
            assert_eq!(&stream.as_bytes()[..8], &[0u8; 8]);
            assert_eq!(stream.as_bytes()[8], b'X');
        });
    }

    #[test]
    fn build_accepts_arbitrary_obj() {
        // build 接受非 None 对象（如 int），Seek 仍忽略 obj 内容
        with_py(|py| {
            let node = SeekNode::new(SeekOffset::Const(2), Whence::Start);
            let obj = py.eval_bound("42", None, None).expect("int");
            let mut stream = BuildStream::new();
            stream.write(b"abcde");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.tell(), 2);
        });
    }

    // ======================================================================
    // sizeof — SK6: 永远 Err
    // ======================================================================

    #[test]
    fn sizeof_sk6_always_returns_err() {
        with_py(|py| {
            let node = SeekNode::new(SeekOffset::Const(5), Whence::Start);
            let ctx = Context::placeholder(py);
            let err = node.sizeof(&ctx).expect_err("should fail");
            match err {
                ConstructError::Generic { message, .. } => {
                    assert!(message.contains("Seek"), "got: {}", message);
                }
                other => panic!("expected Generic, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // SK7: 表达式 at 求值失败时错误路径含 "at"
    // ======================================================================

    #[test]
    fn parse_sk7_expr_at_error_path_contains_at() {
        with_py(|py| {
            // 引用 idx 0 但 ctx 未初始化 expr_values → ExprContext 错误
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0)]);
            let node = SeekNode::new(SeekOffset::Expr(prog), Whence::Start);
            let mut stream = ParseStream::new(b"abc");
            // placeholder ctx 未 init_expr_values
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            let p = err.path().unwrap_or("");
            assert!(p.contains("at"), "expected 'at' in path, got: {}", p);
        });
    }

    // ======================================================================
    // 综合场景：Seek + Bytes 在 Sequence 内的行为
    // ======================================================================

    #[test]
    fn parse_seek_then_read_returns_correct_byte() {
        // 模拟 Sequence(Seek(5), Bytes(1))：seek 到 5 后读 1 字节
        with_py(|py| {
            let seek_node = SeekNode::new(SeekOffset::Const(5), Whence::Start);
            let bytes_node = BytesNode::new_const(1);
            let mut stream = ParseStream::new(b"0123456789");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let _ = seek_node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("seek");
            let chunk = bytes_node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("bytes");
            // 读 pos=5 处的字节 = '5'
            let bytes: &[u8] = chunk.bind(py).extract().expect("extract bytes");
            assert_eq!(bytes, b"5");
            assert_eq!(stream.tell(), 6);
        });
    }

    #[test]
    fn parse_seek_beyond_eof_returns_stream_error() {
        with_py(|py| {
            let node = SeekNode::new(SeekOffset::Const(100), Whence::Start);
            let mut stream = ParseStream::new(b"abc");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Stream { message, .. } => {
                    assert!(message.contains("out of bounds"), "got: {}", message);
                }
                other => panic!("expected Stream, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // Debug
    // ======================================================================

    #[test]
    fn debug_format_includes_seek_node() {
        let node = SeekNode::new(SeekOffset::Const(5), Whence::Start);
        let s = format!("{:?}", node);
        assert!(s.contains("SeekNode"), "got: {}", s);
    }

    // 抑制未使用警告
    #[allow(dead_code)]
    fn _ensure_pylist_used(_l: &PyList) {}
}
