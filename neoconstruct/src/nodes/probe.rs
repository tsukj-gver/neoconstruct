//! ProbeNode：调试探针节点。
//!
//! Python 参考：`construct/construct/debug.py` `Probe`（L6-95）。
//!
//! ## 行为概述
//!
//! Probe 在 parse/build/sizeof 时调用 `printout`：输出分隔线 + path +
//! 可选 stream peek + 可选 context dump，然后返回 Py_None（parse）/ no-op（build）。
//!
//! ## into 字段类型：FieldName 代替 ExprProgram
//!
//! `into` 用 `Option<FieldName>` 而非 `Option<ExprProgram>`：ExprProgram 仅支持 i64
//! 求值，而 Probe.into 的实际用例是"调试打印某字段的值"（任意类型），与 Switch
//! FieldRef 同模式（`ctx.get_field(name)` → PyObject），FieldName 更直接地满足
//! 用例且无需扩展 expr.rs。
//!
//! Parity 影响：Probe(lambda ctx: complex_expr) 不支持（所有"动态值"位置不接
//! callable）。Probe(some_field) 完全支持，且支持任意类型字段。

use crate::context::Context;
use crate::error::ConstructError;
use crate::nodes::struct_node::FieldName;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

use super::Construct;

/// 调试探针节点：parse/build 时打印 path + 可选 stream peek + 可选 context dump。
///
/// 对应 Python construct `Probe(into=None, lookahead=None)`（debug.py L6）。
///
/// # 三方法行为
///
/// - parse/build：调用内部 `printout` 函数（输出到 Python `print`，与 Python
///   原版保持缓冲一致），返回 Py_None（parse）/ no-op（build）
/// - sizeof：调用 printout 后返回 0
///
/// # 字段说明
///
/// - `into`：可选字段名（任意类型字段引用），求值后 repr 打印。None 表示打印整个 context。
///   （用 `FieldName` 而非 `ExprProgram`，详见模块级注释）
/// - `lookahead`：可选 peek 字节数。None 表示不 peek。
#[derive(Debug)]
pub struct ProbeNode {
    /// 可选字段名：求值后 repr 打印。None 表示打印整个 context。
    into: Option<FieldName>,
    /// 可选 peek 字节数：None 表示不 peek。
    lookahead: Option<usize>,
}

impl ProbeNode {
    /// 创建 `ProbeNode`。
    ///
    /// # 参数
    ///
    /// - `into`：可选字段名（任意类型字段引用），None 表示打印整个 context。
    /// - `lookahead`：可选 peek 字节数，None 表示不 peek。
    pub fn new(into: Option<FieldName>, lookahead: Option<usize>) -> Self {
        Self { into, lookahead }
    }

    /// 返回 into 字段名的引用（None 表示打印整个 context）。
    pub fn into_field(&self) -> Option<&FieldName> {
        self.into.as_ref()
    }

    /// 返回 lookahead 字节数。
    pub fn lookahead(&self) -> Option<usize> {
        self.lookahead
    }

    /// 内部 printout 函数：输出分隔线 + path + 可选 stream peek + 可选 context dump。
    ///
    /// 与 Python 原版行为对齐（debug.py L36-95）：
    /// 1. 分隔线
    /// 2. "Probe, path is X"
    /// 3. 可选 "Stream peek: (hexlified) ..."
    /// 4. 可选 context dump（into 有时打印字段值，无时打印整个 context）
    /// 5. 分隔线
    ///
    /// `stream` 为 None 时不输出 peek 行（build/sizeof 路径）。
    fn printout(
        &self,
        py: Python<'_>,
        stream: Option<&mut ParseStream<'_>>,
        ctx: &Context<'_>,
        path: &Path,
    ) -> Result<(), ConstructError> {
        let sep = "===============================================================";
        let _ = py
            .import_bound("builtins")
            .and_then(|b| b.getattr("print"))
            .and_then(|print| print.call1((sep,)));

        let path_str = format!("Probe, path is {}", path);
        let _ = py
            .import_bound("builtins")
            .and_then(|b| b.getattr("print"))
            .and_then(|print| print.call1((path_str,)));

        // 可选 stream peek（lookahead 字节数）
        if let Some(n) = self.lookahead {
            if let Some(s) = stream {
                let fallback = s.tell();
                let remaining = s.remaining();
                let peek_str = if n == 0 || remaining == 0 {
                    "Stream peek: EOF reached".to_string()
                } else {
                    let read_len = n.min(remaining);
                    let chunk = match s.read(read_len, path) {
                        Ok(c) => c,
                        Err(_) => &[][..],
                    };
                    // hexlify 简化：直接 format bytes
                    let hex: String = chunk.iter().map(|b| format!("{:02x}", b)).collect();
                    // seek 回 fallback
                    let _ = s.seek(fallback, path);
                    format!("Stream peek: {}", hex)
                };
                let _ = py
                    .import_bound("builtins")
                    .and_then(|b| b.getattr("print"))
                    .and_then(|print| print.call1((peek_str,)));
            }
        }

        // 可选 context dump
        let ctx_str: String = if let Some(name) = &self.into {
            // 求值字段名 → repr
            match ctx.get_field(name.rust_name()) {
                Ok(Some(v)) => {
                    let r = v.repr().map(|r| r.to_string_lossy().into_owned());
                    r.unwrap_or_else(|_| "<unrepr>".to_string())
                }
                _ => format!("<field {} missing>", name.rust_name()),
            }
        } else {
            // 打印整个 context
            match ctx.fields() {
                Some(d) => {
                    let r = d.repr().map(|r| r.to_string_lossy().into_owned());
                    r.unwrap_or_else(|_| "{}".to_string())
                }
                None => "<no context>".to_string(),
            }
        };
        let _ = py
            .import_bound("builtins")
            .and_then(|b| b.getattr("print"))
            .and_then(|print| print.call1((ctx_str,)));

        let _ = py
            .import_bound("builtins")
            .and_then(|b| b.getattr("print"))
            .and_then(|print| print.call1((sep,)));

        Ok(())
    }
}

impl Construct for ProbeNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 借用 stream 给 printout，printout 内部 seek 回 fallback 保证不消费字节
        self.printout(py, Some(stream), ctx, path)?;
        Ok(py.None())
    }

    fn build(
        &self,
        py: Python<'_>,
        _obj: &Bound<'_, PyAny>,
        _stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        self.printout(py, None, ctx, path)?;
        Ok(())
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // sizeof 路径无 path / py 参数，直接调用 Python print（with_gil 获取）。
        // 注：与 Python 原版行为一致——sizeof 也调 printout。
        Python::with_gil(|py| {
            let dummy_path = Path::new();
            self.printout(py, None, ctx, &dummy_path)?;
            Ok::<(), ConstructError>(())
        })?;
        Ok(0)
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

    /// 构造一个含若干整数字段的 `Context`，用于表达式求值测试。
    fn make_context<'py>(py: Python<'py>, entries: &[(&str, i64)]) -> Context<'py> {
        let ctx = Context::new_root(py).expect("new_root");
        for (name, value) in entries {
            let val = (*value).into_py(py);
            ctx.set_field(name, val.bind(py)).expect("set_field");
        }
        ctx
    }

    // ======================================================================
    // 构造器
    // ======================================================================

    #[test]
    fn new_stores_into_and_lookahead() {
        with_py(|py| {
            let fname = FieldName::new(py, "x");
            let node = ProbeNode::new(Some(fname), Some(4));
            assert!(node.into_field().is_some());
            assert_eq!(node.lookahead(), Some(4));
        });
    }

    #[test]
    fn new_with_none_into_and_lookahead() {
        let node = ProbeNode::new(None, None);
        assert!(node.into_field().is_none());
        assert!(node.lookahead().is_none());
    }

    // ======================================================================
    // parse — PB-1 / PB-2 / PB-3 / PB-4
    // ======================================================================

    #[test]
    fn parse_basic_outputs_to_stdout() {
        // PB-1: Probe() parse → 输出含 "Probe, path is" + 分隔线
        // 测试不验证 stdout（cargo test 不易捕获 Python stdout），仅验证不报错。
        with_py(|py| {
            let node = ProbeNode::new(None, None);
            let mut stream = ParseStream::new(b"abc");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert!(result.is(&py.None()));
            // Probe 不消费字节
            assert_eq!(stream.tell(), 0);
        });
    }

    #[test]
    fn parse_with_lookahead_does_not_consume() {
        // PB-2: Probe(lookahead=4) peek 后 stream.tell() 不变
        with_py(|py| {
            let node = ProbeNode::new(None, Some(4));
            let mut stream = ParseStream::new(&[0x00, 0x01, 0x02, 0x03]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let _ = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // peek 后 seek 回 fallback
            assert_eq!(stream.tell(), 0);
        });
    }

    #[test]
    fn parse_with_into_field_outputs_field_value() {
        // PB-3: Probe(x) where x=42 → 输出含 "42"
        with_py(|py| {
            let fname = FieldName::new(py, "x");
            let node = ProbeNode::new(Some(fname), None);
            let mut stream = ParseStream::new(b"");
            let mut ctx = make_context(py, &[("x", 42)]);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert!(result.is(&py.None()));
        });
    }

    #[test]
    fn parse_lookahead_eof_outputs_eof_message() {
        // PB-4: Probe(lookahead=32) parse stream 已 EOF → 输出 "Stream peek: EOF reached"
        with_py(|py| {
            let node = ProbeNode::new(None, Some(32));
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let _ = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert_eq!(stream.tell(), 0);
        });
    }

    // ======================================================================
    // build
    // ======================================================================

    #[test]
    fn build_outputs_to_stdout_no_op() {
        with_py(|py| {
            let node = ProbeNode::new(None, None);
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert!(stream.as_bytes().is_empty());
        });
    }

    // ======================================================================
    // sizeof — PB-7
    // ======================================================================

    #[test]
    fn sizeof_outputs_and_returns_zero() {
        // PB-7: Probe() sizeof → 输出 printout 后返回 0
        with_py(|py| {
            let node = ProbeNode::new(None, None);
            let ctx = Context::placeholder(py);
            let size = node.sizeof(&ctx).expect("sizeof");
            assert_eq!(size, 0);
        });
    }

    // ======================================================================
    // Debug
    // ======================================================================

    #[test]
    fn debug_format_includes_probe_node() {
        let node = ProbeNode::new(None, None);
        let s = format!("{:?}", node);
        assert!(s.contains("ProbeNode"), "got: {}", s);
    }
}
