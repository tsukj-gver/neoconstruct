//! HexDumpNode：HexDump 显示包装节点。
//!
//! Python 参考：`construct/construct/core.py` `HexDump`（L3583-3635）。
//!
//! ## 行为概述
//!
//! HexDump 与 HexNode 同模式，仅显示类不同（仅 bytes/dict 两种）。
//! int 类型不包装（透传 PyLong）。

use crate::context::Context;
use crate::error::ConstructError;
use crate::nodes::hex::HexDumpDisplayClasses;
use crate::nodes::Node;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};

use super::Construct;

/// HexDump 显示包装节点：与 HexNode 同模式，仅显示类不同。
///
/// 对应 Python construct `HexDump(subcon)`（core.py L3583）。
///
/// # 三方法行为
///
/// - parse：inner.parse → 按 obj 类型分派（bytes/dict 走显示类；其他透传）
/// - build：透传 inner.build
/// - sizeof：转发 inner.sizeof
#[derive(Debug)]
pub struct HexDumpNode {
    /// 被包装的子树根。
    inner: Box<Node>,
    /// 编译期加载的 Python 显示类引用（bytes/dict 两类）。
    display_classes: HexDumpDisplayClasses,
}

impl HexDumpNode {
    /// 创建 `HexDumpNode`。
    pub fn new(inner: Node, display_classes: HexDumpDisplayClasses) -> Self {
        Self {
            inner: Box::new(inner),
            display_classes,
        }
    }

    /// 返回内部子树根节点的引用。
    pub fn inner(&self) -> &Node {
        &self.inner
    }
}

impl Construct for HexDumpNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let obj = self.inner.parse(py, stream, ctx, path)?;
        let bound = obj.bind(py);
        // HexDump 仅包装 bytes/dict，其他类型透传（int 透传）。
        if bound.is_instance_of::<PyBytes>() {
            let cls_bound = self.display_classes.bytes_cls.bind(py);
            let new_obj = cls_bound
                .call1((bound,))
                .map_err(|e| ConstructError::Generic {
                    message: format!("HexDumpDisplayedBytes() failed: {}", e),
                    path: path.to_string(),
                })?;
            Ok(new_obj.unbind())
        } else if bound.is_instance_of::<PyDict>() {
            let cls_bound = self.display_classes.dict.bind(py);
            let new_obj = cls_bound
                .call1((bound,))
                .map_err(|e| ConstructError::Generic {
                    message: format!("HexDumpDisplayedDict() failed: {}", e),
                    path: path.to_string(),
                })?;
            Ok(new_obj.unbind())
        } else {
            // 未知类型或 int 透传
            Ok(obj)
        }
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        self.inner.build(py, obj, stream, ctx, path)
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        self.inner.sizeof(ctx)
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::bytes::BytesNode;
    use crate::nodes::format_field::{FormatFieldNode, PythonFormat};

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

    /// 加载真实 HexDump 显示类（如果 neoconstruct.lib.hex 可用）。
    fn try_load_classes(py: Python<'_>) -> Option<HexDumpDisplayClasses> {
        crate::nodes::hex::load_hexdump_display_classes(py).ok()
    }

    #[test]
    fn parse_bytes_returns_hexdump_displayed_bytes() {
        // HexDump(Bytes(4)).parse(b'\x00\x00\x01\x02') → HexDumpDisplayedBytes
        with_py(|py| {
            let classes = match try_load_classes(py) {
                Some(c) => c,
                None => {
                    eprintln!("skipping: neoconstruct.lib.hex not available");
                    return;
                }
            };
            let inner = Node::Bytes(BytesNode::new_const(4));
            let node = HexDumpNode::new(inner, classes);
            let mut stream = ParseStream::new(&[0x00, 0x00, 0x01, 0x02]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bound = result.bind(py);
            assert!(bound.is_instance_of::<PyBytes>());
        });
    }

    #[test]
    fn parse_int_passes_through() {
        // HexDump(Int32ub).parse(...) → 透传 PyLong（int 不包装）
        with_py(|py| {
            let classes = match try_load_classes(py) {
                Some(c) => c,
                None => return,
            };
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt32Big));
            let node = HexDumpNode::new(inner, classes);
            let mut stream = ParseStream::new(&[0x00, 0x00, 0x01, 0x02]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bound = result.bind(py);
            assert!(bound.is_instance_of::<pyo3::types::PyLong>());
        });
    }

    #[test]
    fn sizeof_returns_inner_size() {
        with_py(|py| {
            let classes = match try_load_classes(py) {
                Some(c) => c,
                None => return,
            };
            let inner = Node::Bytes(BytesNode::new_const(4));
            let node = HexDumpNode::new(inner, classes);
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).unwrap(), 4);
        });
    }

    #[test]
    fn debug_format_includes_hexdump_node() {
        with_py(|py| {
            let classes = match try_load_classes(py) {
                Some(c) => c,
                None => return,
            };
            let inner = Node::Bytes(BytesNode::new_const(4));
            let node = HexDumpNode::new(inner, classes);
            let s = format!("{:?}", node);
            assert!(s.contains("HexDumpNode"), "got: {}", s);
        });
    }
}
