//! AlignedNode：字节对齐包装节点。
//!
//! Python 参考：`construct/construct/core.py` `Aligned`（L4261-4331）。
//!
//! ## 行为概述
//!
//! Aligned 是 Subconstruct 包装：inner 解析/构建后，填充字节到 modulus 的整数倍。
//! parse 消费填充字节（不验证 pattern）；build 写入填充字节。
//!
//! ## modulus 算法
//!
//! Python 用 `pad = -(position2 - position1) % modulus`，Python 的 `%` 是模运算
//! （结果非负）。Rust 的 `%` 是 remainder（可为负），需用 `rem_euclid` 对齐 Python 语义。
//! `(-(x)).rem_euclid(m)` 等价于 Python `(-x) % m`。

use crate::context::Context;
use crate::error::ConstructError;
use crate::expr::{eval_expr_int, ExprOp, ExprProgram};
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

use super::Construct;
use crate::nodes::Node;

/// 对齐包装节点：inner 解析/构建后，填充字节到 modulus 的整数倍。
///
/// 对应 Python construct `Aligned(modulus, subcon, pattern=b"\\x00")`
/// （core.py L4261）。与 PaddingNode 同模式。
///
/// # padding 算法
///
/// - parse：inner.parse 后，`pad = -(tell_after - tell_before) % modulus`，
///   stream.read(pad) 消费填充字节（不验证 pattern，对齐 Python L4303）
/// - build：inner.build 后，`pad = -(tell_after - tell_before) % modulus`，
///   stream.write(pattern * pad)
/// - sizeof：inner.sizeof + (-inner.sizeof % modulus)
///
/// # modulus 约束
///
/// modulus 必须 >= 2（Python L4297/L4308 校验）。编译期常量在 build_node 时
/// 校验；运行期 ExprProgram 求值后校验（<2 抛 PaddingError）。
#[derive(Debug)]
pub struct AlignedNode {
    /// 被包装的子树根。
    inner: Box<Node>,
    /// 对齐模数表达式（常量或字段引用）。
    modulus: ExprProgram,
    /// 填充字节模式（默认 0x00）。Python 限定 len==1，编译期已提取为单字节。
    pattern: u8,
}

impl AlignedNode {
    /// 创建 `AlignedNode`。
    ///
    /// # 参数
    ///
    /// - `inner`：被包装的子树。
    /// - `modulus`：对齐模数表达式（常量值编译期包装为 `Const` ExprProgram）。
    /// - `pattern`：填充字节模式（0-255，默认 0x00）。
    pub fn new(inner: Node, modulus: ExprProgram, pattern: u8) -> Self {
        Self {
            inner: Box::new(inner),
            modulus,
            pattern,
        }
    }

    /// 返回内部子树根节点的引用。
    pub fn inner(&self) -> &Node {
        &self.inner
    }

    /// 返回对齐模数表达式的引用。
    pub fn modulus(&self) -> &ExprProgram {
        &self.modulus
    }

    /// 返回填充字节模式。
    pub fn pattern(&self) -> u8 {
        self.pattern
    }

    /// 返回 modulus 是否为编译期常量（单条 [`ExprOp::Const`] 指令）。
    ///
    /// 编译期常量 modulus 不需要 ctx 字段求值（Const 指令不入 ctx），
    /// 因此 sizeof 可静态计算，Struct static_size 预分配可恢复。
    fn modulus_is_const(&self) -> bool {
        matches!(self.modulus.ops(), [ExprOp::Const(_)])
    }

    /// 判断此节点（含子树）是否含运行期表达式字段。
    ///
    /// - modulus 是运行期表达式（非单条 Const）→ `true`
    /// - inner 子树含表达式 → `true`
    /// - modulus 是编译期常量且 inner 无表达式 → `false`
    ///
    /// 决定 Struct build 入口 context 模式（new_root vs placeholder）与
    /// static_size 缓存（schema.rs 在 has_expressions=false 时计算预分配容量）。
    pub fn has_expressions(&self) -> bool {
        !self.modulus_is_const() || self.inner.has_expressions()
    }
}

impl Construct for AlignedNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let modulus_i64 = eval_expr_int(&self.modulus, ctx, py)?;
        if modulus_i64 < 2 {
            return Err(ConstructError::Padding {
                message: format!("expected modulo 2 or greater, got {}", modulus_i64),
                path: path.to_string(),
            });
        }
        let pos1 = stream.tell();
        let obj = self.inner.parse(py, stream, ctx, path)?;
        let pos2 = stream.tell();
        // checked_sub 防御性，pos2 应 >= pos1
        let consumed = pos2
            .checked_sub(pos1)
            .ok_or_else(|| ConstructError::Generic {
                message: format!(
                    "Aligned: stream position regressed during inner parse (pos1={}, pos2={})",
                    pos1, pos2
                ),
                path: path.to_string(),
            })?;
        // pad = -(consumed) % modulus（Python 语义 → rem_euclid）
        let pad = (-(consumed as i64)).rem_euclid(modulus_i64) as usize;
        // 消费填充字节（不验证 pattern，对齐 Python L4303）
        let _ = stream.read(pad, path)?;
        Ok(obj)
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        let modulus_i64 = eval_expr_int(&self.modulus, ctx, py)?;
        if modulus_i64 < 2 {
            return Err(ConstructError::Padding {
                message: format!("expected modulo 2 or greater, got {}", modulus_i64),
                path: path.to_string(),
            });
        }
        let pos1 = stream.tell();
        self.inner.build(py, obj, stream, ctx, path)?;
        let pos2 = stream.tell();
        let consumed = pos2
            .checked_sub(pos1)
            .ok_or_else(|| ConstructError::Generic {
                message: format!(
                    "Aligned: stream position regressed during inner build (pos1={}, pos2={})",
                    pos1, pos2
                ),
                path: path.to_string(),
            })?;
        let pad = (-(consumed as i64)).rem_euclid(modulus_i64) as usize;
        if pad > 0 {
            let pad_bytes = vec![self.pattern; pad];
            stream.write(&pad_bytes);
        }
        Ok(())
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // 检测 modulus 是否编译期常量（单条 ExprOp::Const 指令）。
        // - 编译期常量且 >= 2：计算 inner.sizeof + pad。
        // - 非常量或非法常量（< 2）：返回 Err（对齐 Python SizeofError）。
        //
        // 编译期常量 modulus 通过 ExprProgram::Const 包装，sizeof 签名无 py 参数，
        // 但 Const 指令求值不依赖 ctx（见 eval_expr_int ExprOp::Const 分支），
        // 因此可直接读取常量值而无需 eval。
        let modulus = match self.modulus.ops() {
            [ExprOp::Const(n)] if *n >= 2 => *n as usize,
            _ => {
                return Err(ConstructError::Generic {
                    message:
                        "Aligned sizeof requires constant modulus (dynamic modulus triggers SizeofError)"
                            .to_string(),
                    path: String::new(),
                });
            }
        };
        let inner_len = self.inner.sizeof(ctx)?;
        // pad = -(inner_len) % modulus（Python 语义 → rem_euclid）
        let pad = (-(inner_len as i64)).rem_euclid(modulus as i64) as usize;
        Ok(inner_len + pad)
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
    use crate::nodes::format_field::{FormatFieldNode, PythonFormat};
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

    /// 构造一个含若干整数字段的 `Context`，用于表达式求值测试。
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

    /// 常量 modulus 4。
    fn modulus_four() -> ExprProgram {
        ExprProgram::new(vec![ExprOp::Const(4)])
    }

    // ======================================================================
    // 构造器
    // ======================================================================

    #[test]
    fn new_stores_inner_modulus_pattern() {
        let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
        let modulus = modulus_four();
        let node = AlignedNode::new(inner, modulus, 0xFF);
        assert!(matches!(node.inner(), Node::FormatField(_)));
        assert_eq!(node.pattern(), 0xFF);
    }

    // ======================================================================
    // has_expressions — 编译期常量 modulus 不计入表达式
    // ======================================================================

    #[test]
    fn has_expressions_const_modulus_no_expr_inner_returns_false() {
        // modulus=Const(4) + inner 无表达式 → false
        // 允许 Struct static_size 预分配恢复（schema.rs L86-90）。
        let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
        let node = AlignedNode::new(inner, modulus_four(), 0x00);
        assert!(
            !node.has_expressions(),
            "const modulus + no-expr inner should not count as expression"
        );
    }

    #[test]
    fn has_expressions_runtime_modulus_returns_true() {
        // modulus 是运行期表达式（GetInt）→ true
        let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
        let modulus = ExprProgram::new(vec![ExprOp::GetInt(0)]);
        let node = AlignedNode::new(inner, modulus, 0x00);
        assert!(
            node.has_expressions(),
            "runtime modulus expression should count as expression"
        );
    }

    #[test]
    fn has_expressions_const_modulus_with_expr_inner_returns_true() {
        // modulus=Const(4) 但 inner 含表达式（Rebuild）→ true（递归检查子树）
        let inner = Node::Rebuild(crate::nodes::rebuild::RebuildNode::new(
            Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big)),
            ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(1), ExprOp::Add]),
        ));
        let node = AlignedNode::new(inner, modulus_four(), 0x00);
        assert!(
            node.has_expressions(),
            "const modulus but expr inner should count as expression"
        );
    }

    // ======================================================================
    // parse
    // ======================================================================

    #[test]
    fn parse_consumes_padding() {
        // Aligned(4, Int16ub).parse(b'\x00\x01\x00\x00') → 1
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
            let node = AlignedNode::new(inner, modulus_four(), 0x00);
            let mut stream = ParseStream::new(&[0x00, 0x01, 0x00, 0x00]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().unwrap();
            assert_eq!(v, 1);
            assert_eq!(
                stream.tell(),
                4,
                "should consume 4 bytes (2 data + 2 padding)"
            );
        });
    }

    #[test]
    fn parse_greedy_bytes_then_padding() {
        // Aligned(4, GreedyBytes).parse(b'\x01\x02\x03\x04')
        //   inner 读 3 字节，consumed=3, pad=1, 读 1 字节填充
        with_py(|py| {
            let inner = Node::GreedyBytes(crate::nodes::greedy_bytes::GreedyBytesNode::new());
            // GreedyBytes 会读到 EOF，所以这里用 4 字节测试
            let node = AlignedNode::new(inner, modulus_four(), 0x00);
            let mut stream = ParseStream::new(&[0x01, 0x02, 0x03, 0x04]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let _ = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // GreedyBytes 读 4 字节，consumed=4, pad=0, 不再读
            assert_eq!(stream.tell(), 4);
        });
    }

    #[test]
    fn parse_modulus_lt_2_raises_padding_error() {
        // Aligned(1, Int16ub) parse → 运行期 PaddingError
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
            let modulus = ExprProgram::new(vec![ExprOp::Const(1)]);
            let node = AlignedNode::new(inner, modulus, 0x00);
            let mut stream = ParseStream::new(&[0x00, 0x01]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Padding { .. }));
        });
    }

    #[test]
    fn parse_with_expression_modulus() {
        // 等价：Aligned(some_field, ...) where some_field=4
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            // modulus: GetInt(0)（字段 0 = "mod" = 4）
            let modulus = ExprProgram::new(vec![ExprOp::GetInt(0)]);
            let node = AlignedNode::new(inner, modulus, 0x00);
            let mut stream = ParseStream::new(&[0xFF, 0x00, 0x00, 0x00]);
            let mut ctx = make_context(py, &[("mod", 4)]);
            let mut path = Path::new();
            let _ = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // consumed=1, pad=3, 总消费 4
            assert_eq!(stream.tell(), 4);
        });
    }

    // ======================================================================
    // build
    // ======================================================================

    #[test]
    fn build_writes_padding() {
        // Aligned(4, Int16ub).build(1) → b'\x00\x01\x00\x00'
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
            let node = AlignedNode::new(inner, modulus_four(), 0x00);
            let obj = py.eval_bound("1", None, None).expect("1");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x00, 0x01, 0x00, 0x00]);
        });
    }

    #[test]
    fn build_with_pattern() {
        // Aligned(4, Int16ub, pattern=b'\xff').build(1) → b'\x00\x01\xff\xff'
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
            let node = AlignedNode::new(inner, modulus_four(), 0xFF);
            let obj = py.eval_bound("1", None, None).expect("1");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x00, 0x01, 0xFF, 0xFF]);
        });
    }

    #[test]
    fn build_no_padding_when_aligned() {
        // Aligned(4, Bytes(4)).build(b'xxxx') → b'xxxx'（无 padding）
        with_py(|py| {
            let inner = Node::Bytes(BytesNode::new_const(4));
            let node = AlignedNode::new(inner, modulus_four(), 0x00);
            let obj = py.eval_bound("b'xxxx'", None, None).expect("bytes");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), b"xxxx");
        });
    }

    // ======================================================================
    // sizeof — 编译期常量成功，运行期表达式 Err
    // ======================================================================

    #[test]
    fn sizeof_const_modulus_returns_inner_plus_pad() {
        // Aligned(4, Int16ub).sizeof() → 4 (inner_len=2, pad=2)
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
            let node = AlignedNode::new(inner, modulus_four(), 0x00);
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).expect("const modulus 4 + Int16ub"), 4);
        });
    }

    #[test]
    fn sizeof_const_modulus_pad_one() {
        // Aligned(4, Bytes(3)).sizeof() → 4 (inner_len=3, pad=1)
        with_py(|py| {
            let inner = Node::Bytes(BytesNode::new_const(3));
            let node = AlignedNode::new(inner, modulus_four(), 0x00);
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).expect("const modulus 4 + Bytes(3)"), 4);
        });
    }

    #[test]
    fn sizeof_const_modulus_no_pad_when_aligned() {
        // Aligned(4, Bytes(4)).sizeof() → 4 (inner_len=4, pad=0)
        with_py(|py| {
            let inner = Node::Bytes(BytesNode::new_const(4));
            let node = AlignedNode::new(inner, modulus_four(), 0x00);
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).expect("const modulus 4 + Bytes(4)"), 4);
        });
    }

    #[test]
    fn sizeof_const_modulus_other_values() {
        // 补充覆盖：modulus 8 + Int8ub (inner_len=1, pad=7) → 8
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let modulus = ExprProgram::new(vec![ExprOp::Const(8)]);
            let node = AlignedNode::new(inner, modulus, 0x00);
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).expect("const modulus 8 + Int8ub"), 8);
        });
    }

    #[test]
    fn sizeof_runtime_modulus_returns_err() {
        // modulus 是运行期表达式（GetInt）→ sizeof 返回 Err（对齐 Python SizeofError）
        // 区分编译期常量（应成功，见上）vs 运行期表达式（应 Err）。
        let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
        let modulus = ExprProgram::new(vec![ExprOp::GetInt(0)]);
        let node = AlignedNode::new(inner, modulus, 0x00);
        with_py(|py| {
            let ctx = Context::placeholder(py);
            let err = node
                .sizeof(&ctx)
                .expect_err("runtime modulus should fail sizeof");
            assert!(matches!(err, ConstructError::Generic { .. }));
        });
    }

    #[test]
    fn sizeof_const_modulus_below_two_returns_err() {
        // 防御性：Const(1) 是非法 modulus（< 2），sizeof 返回 Err。
        // 注：编译期 build_node 会拒绝 Const(<2)，此处覆盖防御性运行期路径。
        let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
        let modulus = ExprProgram::new(vec![ExprOp::Const(1)]);
        let node = AlignedNode::new(inner, modulus, 0x00);
        with_py(|py| {
            let ctx = Context::placeholder(py);
            let err = node
                .sizeof(&ctx)
                .expect_err("const modulus < 2 should fail");
            assert!(matches!(err, ConstructError::Generic { .. }));
        });
    }

    // ======================================================================
    // parse ↔ build 往返
    // ======================================================================

    #[test]
    fn round_trip_aligned_int() {
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let node = AlignedNode::new(inner, modulus_four(), 0x00);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            // build
            let obj = py.eval_bound("0xAB", None, None).expect("obj");
            let mut bstream = BuildStream::new();
            node.build(py, &obj, &mut bstream, &mut ctx, &mut path)
                .expect("build");
            let bytes = bstream.into_bytes();
            assert_eq!(bytes, &[0xAB, 0x00, 0x00, 0x00]);

            // parse back
            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().unwrap();
            assert_eq!(v, 0xAB);
        });
    }

    // ======================================================================
    // Debug
    // ======================================================================

    #[test]
    fn debug_format_includes_aligned_node() {
        let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
        let node = AlignedNode::new(inner, modulus_four(), 0x00);
        let s = format!("{:?}", node);
        assert!(s.contains("AlignedNode"), "got: {}", s);
    }
}
