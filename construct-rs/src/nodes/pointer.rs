//! PointerNode：绝对偏移读写节点（Phase 7.2 §3.3）。
//!
//! Python 参考：`construct/construct/core.py` `Pointer`（L4384-4443）。
//!
//! ## 行为概述
//!
//! Pointer 在指定偏移处读写 subcon，结束后 seek 回原位置（不占主流位置）。
//! 复用 [`crate::nodes::peek::PeekNode`] 已验证的 save/restore Tell 模式
//! （fallback = stream.tell() → seek(offset) → 处理 subcon → seek(fallback)）。
//!
//! ## offset 参数（int 或表达式）
//!
//! - 常量（Python int）→ [`PointerOffset::Const`]
//! - 表达式（FieldRef/ExprRef）→ [`PointerOffset::Expr`]，运行时调
//!   [`crate::expr::eval_expr_int`] 求值
//!
//! ## whence 计算（对齐 Python `_pointer_seek`，core.py L4421-4424）
//!
//! - `relative == true` → [`Whence::Current`]
//! - `relative == false`：
//!   - `offset >= 0` → [`Whence::Start`]
//!   - `offset < 0` → [`Whence::End`]（从 EOF 向前）
//!
//! ## 错误路径强制 seek 回（对齐 Python `finally`）
//!
//! subcon.parse/build 失败时，必须先 seek 回 fallback 再返回 Err（否则主流位置错乱，
//! 破坏后续字段）。Rust 用 `match` + 显式 seek（无 finally），与 PeekNode 模式一致。
//!
//! ## 已知限制（设计文档化）
//!
//! - **`stream` 参数（换流）不支持**：Python 允许 `Pointer(offset, subcon, stream=context_lambda)`
//!   换流；construct-rs 单一流模型不支持。编译期拒绝非 None stream。

use crate::context::Context;
use crate::error::ConstructError;
use crate::expr::{eval_expr_int, ExprProgram};
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream, Whence};
use pyo3::prelude::*;

use super::Construct;
use crate::nodes::Node;

/// 绝对偏移读写节点：seek 到 offset 处理 subcon，再 seek 回原位置。
///
/// 对应 Python construct `Pointer(offset, subcon, stream=None, relativeOffset=False)`
/// （core.py L4384）。
///
/// # 三方法行为
///
/// - parse：求 offset → 记 `fallback = stream.tell()` → seek(offset, whence) →
///   subcon.parse → **无论 Ok/Err 都 seek(fallback, Start)** → 返回 subcon 结果
/// - build：对称（seek → subcon.build → seek 回 fallback）
/// - sizeof：返回 0（Pointer 不占主流位置）
///
/// # whence 计算（对齐 Python `_pointer_seek`，core.py L4421-4424）
///
/// - `relative == true` → [`Whence::Current`]
/// - `relative == false`：
///   - `offset >= 0` → [`Whence::Start`]
///   - `offset < 0` → [`Whence::End`]（从 EOF 向前）
///
/// # 已知限制（文档化）
///
/// - **`stream` 参数（换流）不支持**：Python 允许 `Pointer(offset, subcon, stream=context_lambda)`
///   换流。construct-rs 单一流模型不支持。换流极少用（主流用法是默认流）。
/// - **错误路径强制 seek 回 fallback**：subcon.parse/build 失败时，必须先 seek 回 fallback
///   再返回 Err（对齐 Python `finally: stream_seek(fallback, 0)`）。Rust 用 `match` + 显式
///   seek（无 finally），与 PeekNode 模式一致。
#[derive(Debug)]
pub struct PointerNode {
    /// 偏移目标：编译期常量或运行时表达式。
    offset: PointerOffset,
    /// relativeOffset 编译期 bool（Python 默认 False）。
    relative: bool,
    /// 被偏移处理的子树。
    subcon: Box<Node>,
}

/// Pointer 的偏移目标，对应 Python `offset` 参数（int 或 context lambda）。
#[derive(Debug)]
pub enum PointerOffset {
    /// 编译期常量（如 `Pointer(8, Bytes(1))`）。
    Const(i64),
    /// 表达式（如 `Pointer(this.off, Bytes(1))`），编译期从 FieldRef/ExprRef 翻译为 ExprProgram。
    Expr(ExprProgram),
}

impl PointerNode {
    /// 创建 `PointerNode`。
    pub fn new(offset: PointerOffset, relative: bool, subcon: Node) -> Self {
        Self {
            offset,
            relative,
            subcon: Box::new(subcon),
        }
    }

    /// 返回 offset 的引用。
    pub fn offset(&self) -> &PointerOffset {
        &self.offset
    }

    /// 返回 relative 标志。
    pub fn relative(&self) -> bool {
        self.relative
    }

    /// 返回 subcon 子树引用。
    pub fn subcon(&self) -> &Node {
        &self.subcon
    }

    /// has_expressions：Expr offset 或 subcon 含表达式。
    pub fn has_expressions(&self) -> bool {
        matches!(self.offset, PointerOffset::Expr(_)) || self.subcon.has_expressions()
    }

    /// 求值 offset（Const 直接返回，Expr 调 eval_expr_int）。
    ///
    /// Expr 路径失败时 push_path_segment("offset")，对齐 PrefixedArray 的 countfield 错误处理
    /// （ADR-016 lazy path：错误自带 path，由 push_path_segment 补充字段名段）。
    fn eval_offset(&self, ctx: &Context<'_>, py: Python<'_>) -> Result<i64, ConstructError> {
        match &self.offset {
            PointerOffset::Const(n) => Ok(*n),
            PointerOffset::Expr(prog) => eval_expr_int(prog, ctx, py).map_err(|mut e| {
                e.push_path_segment("offset");
                e
            }),
        }
    }

    /// 计算 whence（对齐 Python `_pointer_seek` L4421-4424）。
    ///
    /// - `relative == true` → [`Whence::Current`]
    /// - `relative == false`：
    ///   - `offset_val < 0` → [`Whence::End`]
    ///   - 否则 → [`Whence::Start`]
    fn compute_whence(&self, offset_val: i64) -> Whence {
        if self.relative {
            Whence::Current
        } else if offset_val < 0 {
            Whence::End
        } else {
            Whence::Start
        }
    }
}

impl Construct for PointerNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let offset_val = self.eval_offset(ctx, py)?;
        let whence = self.compute_whence(offset_val);
        let fallback = stream.tell();
        // seek 到目标（失败直接返回，未改 fallback）
        stream.seek_whence(offset_val, whence, path)?;
        // subcon.parse —— 无论 Ok/Err 都 seek 回 fallback（对齐 Python finally）
        let result = self.subcon.parse(py, stream, ctx, path);
        // seek 回 fallback（whence=0 用现有 seek，fallback 是绝对位置且已校验过 bounds）
        let seek_back = stream.seek(fallback, path);
        // seek 失败优先（破坏主流位置，比 subcon 错误更严重）
        seek_back?;
        result
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        let offset_val = self.eval_offset(ctx, py)?;
        let whence = self.compute_whence(offset_val);
        let fallback = stream.tell();
        stream.seek(offset_val, whence, path)?;
        let result = self.subcon.build(py, obj, stream, ctx, path);
        // seek 回 fallback（用新 seek 方法，fallback 是 usize 转 i64）
        let seek_back = stream.seek(fallback as i64, Whence::Start, path);
        seek_back?;
        result
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // 对齐 Python _sizeof return 0（Pointer 不占主流位置）
        Ok(0)
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
    use crate::nodes::format_field::FormatFieldNode;
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

    /// 构造一个含若干整数字段的 Context。
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

    /// 单字节 Bytes(1) 节点（最常见 Pointer subcon）。
    fn bytes_1() -> Node {
        Node::Bytes(BytesNode::new_const(1))
    }

    // ======================================================================
    // 构造器 & 访问器
    // ======================================================================

    #[test]
    fn new_stores_offset_relative_subcon() {
        let node = PointerNode::new(PointerOffset::Const(8), false, bytes_1());
        assert!(matches!(node.offset(), PointerOffset::Const(8)));
        assert!(!node.relative());
        assert!(matches!(node.subcon(), Node::Bytes(_)));
    }

    #[test]
    fn has_expressions_const_no_expr_returns_false() {
        let node = PointerNode::new(PointerOffset::Const(8), false, bytes_1());
        assert!(!node.has_expressions());
    }

    #[test]
    fn has_expressions_expr_offset_returns_true() {
        let prog = ExprProgram::new(vec![ExprOp::Const(8)]);
        let node = PointerNode::new(PointerOffset::Expr(prog), false, bytes_1());
        assert!(node.has_expressions());
    }

    #[test]
    fn compute_whence_relative_returns_current() {
        let node = PointerNode::new(PointerOffset::Const(2), true, bytes_1());
        assert_eq!(node.compute_whence(2), Whence::Current);
        // relative 时 offset_val 不影响 whence
        assert_eq!(node.compute_whence(-2), Whence::Current);
    }

    #[test]
    fn compute_whence_absolute_positive_returns_start() {
        let node = PointerNode::new(PointerOffset::Const(8), false, bytes_1());
        assert_eq!(node.compute_whence(8), Whence::Start);
        assert_eq!(node.compute_whence(0), Whence::Start);
    }

    #[test]
    fn compute_whence_absolute_negative_returns_end() {
        let node = PointerNode::new(PointerOffset::Const(-2), false, bytes_1());
        assert_eq!(node.compute_whence(-2), Whence::End);
    }

    // ======================================================================
    // parse — P1: 正 offset 绝对定位
    // ======================================================================

    #[test]
    fn parse_p1_positive_offset_absolute() {
        with_py(|py| {
            let node = PointerNode::new(PointerOffset::Const(8), false, bytes_1());
            let mut stream = ParseStream::new(b"abcdefghijkl"); // 12 字节
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // seek 到 8，读 1 字节 = 'i'，seek 回 fallback=0
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(bytes, b"i");
            assert_eq!(stream.tell(), 0); // seek 回 fallback
        });
    }

    // ======================================================================
    // parse — P2: 负 offset 从 EOF
    // ======================================================================

    #[test]
    fn parse_p2_negative_offset_from_eof() {
        with_py(|py| {
            let node = PointerNode::new(PointerOffset::Const(-2), false, bytes_1());
            let mut stream = ParseStream::new(b"abcdefgh"); // 8 字节
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // whence=End, at=-2 → pos=6, 读 'g'，seek 回 0
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(bytes, b"g");
            assert_eq!(stream.tell(), 0);
        });
    }

    // ======================================================================
    // parse — P3: relativeOffset=True
    // ======================================================================

    #[test]
    fn parse_p3_relative_offset() {
        with_py(|py| {
            let node = PointerNode::new(PointerOffset::Const(2), true, bytes_1());
            let mut stream = ParseStream::new(b"abcdefg");
            // 先消费 3 字节让 tell=3
            let _ = stream.read(3, &Path::new()).expect("read 3");
            assert_eq!(stream.tell(), 3);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // whence=Current, at=2, tell=3 → pos=5, 读 'f', seek 回 3
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(bytes, b"f");
            assert_eq!(stream.tell(), 3); // seek 回 fallback=3
        });
    }

    // ======================================================================
    // build — P4: build 零填充
    // ======================================================================

    #[test]
    fn build_p4_zero_padding() {
        with_py(|py| {
            let node = PointerNode::new(PointerOffset::Const(8), false, bytes_1());
            let obj = py.eval_bound("b'Z'", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();
            // 8 字节 0 + 'Z'
            let mut expected = vec![0u8; 8];
            expected.push(b'Z');
            assert_eq!(bytes, expected);
        });
    }

    // ======================================================================
    // sizeof — P5: 返回 0
    // ======================================================================

    #[test]
    fn sizeof_p5_returns_zero() {
        with_py(|py| {
            let node = PointerNode::new(PointerOffset::Const(8), false, bytes_1());
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).expect("sizeof"), 0);
        });
    }

    // ======================================================================
    // parse — P6: subcon.parse 失败仍 seek 回
    // ======================================================================

    #[test]
    fn parse_p6_subcon_failure_still_seeks_back() {
        with_py(|py| {
            // Pointer(8, Bytes(10)) on 5 字节流：seek 到 8 失败（whence=Start 超 EOF）
            // 直接抛错，未改 fallback
            let inner = Node::Bytes(BytesNode::new_const(10));
            let node = PointerNode::new(PointerOffset::Const(2), false, inner);
            let mut stream = ParseStream::new(b"abcde"); // 5 字节
                                                         // 先消费 1 字节让 fallback=1
            let _ = stream.read(1, &Path::new()).expect("read 1");
            let fallback = stream.tell();
            assert_eq!(fallback, 1);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            // 错误应该是 Stream（Bytes(10) 在子流 3 字节上 EOF）
            assert!(matches!(err, ConstructError::Stream { .. }));
            // stream.tell() 应在 fallback 或 seek-back 后的位置（subcon 失败但 seek-back 执行）
            // 设计：subcon.parse 失败时，仍 seek 回 fallback
            assert_eq!(stream.tell(), fallback);
        });
    }

    // ======================================================================
    // parse — P7: offset 表达式
    // ======================================================================

    #[test]
    fn parse_p7_offset_expression() {
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0)]); // off 字段
            let node = PointerNode::new(PointerOffset::Expr(prog), false, bytes_1());
            let mut stream = ParseStream::new(b"0123456789");
            let mut ctx = make_context(py, &[("off", 5)]);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(bytes, b"5");
            assert_eq!(stream.tell(), 0);
        });
    }

    // ======================================================================
    // 综合场景：Pointer 内 Bytes(N) 不止 1 字节
    // ======================================================================

    #[test]
    fn parse_multi_byte_subcon() {
        with_py(|py| {
            let inner = Node::Bytes(BytesNode::new_const(3));
            let node = PointerNode::new(PointerOffset::Const(2), false, inner);
            let mut stream = ParseStream::new(b"0123456789");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bytes: &[u8] = result.bind(py).extract().expect("extract");
            assert_eq!(bytes, b"234");
            assert_eq!(stream.tell(), 0);
        });
    }

    #[test]
    fn build_pointer_overwrites_existing_byte() {
        // 已有数据：Pointer(2, Bytes(1)) 在已有 5 字节流上覆盖位置 2
        with_py(|py| {
            let node = PointerNode::new(PointerOffset::Const(2), false, bytes_1());
            let obj = py.eval_bound("b'X'", None, None).expect("obj");
            let mut stream = BuildStream::new();
            stream.write(b"abcde"); // buf=[a,b,c,d,e], pos=5
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            // Pointer 覆盖位置 2：buf=[a,b,X,d,e]，pos 回到 5
            let bytes = stream.into_bytes();
            assert_eq!(bytes, b"abXde");
        });
    }

    #[test]
    fn parse_offset_expression_error_path() {
        // Expr offset 求值失败 → path 含 "offset"
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0)]); // 引用 idx 0
            let node = PointerNode::new(PointerOffset::Expr(prog), false, bytes_1());
            let mut stream = ParseStream::new(b"abc");
            let mut ctx = Context::placeholder(py); // 未 init_expr_values
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            let p = err.path().unwrap_or("");
            assert!(
                p.contains("offset"),
                "expected 'offset' in path, got: {}",
                p
            );
        });
    }

    // ======================================================================
    // Debug
    // ======================================================================

    #[test]
    fn debug_format_includes_pointer_node() {
        let node = PointerNode::new(PointerOffset::Const(8), false, bytes_1());
        let s = format!("{:?}", node);
        assert!(s.contains("PointerNode"), "got: {}", s);
    }

    // 抑制未使用警告
    #[allow(dead_code)]
    fn _ensure_format_field_used(_f: &FormatFieldNode) {}
}
