//! PrefixedArrayNode：前缀长度数组读写。
//!
//! Python 参考：`construct/construct/core.py` `PrefixedArray`（L4934-4983）。
//!
//! ## 概述
//!
//! `PrefixedArray(countfield, subcon)` 解析 `countfield`（如 Byte / Int16ub）得到元素
//! 数量，再解析 `count` 个 `subcon` 元素到 Python list。build 方向：取 list 长度，
//! 先 build countfield（写入长度），再遍历 list build 每个元素。
//!
//! - `countfield` 是任意能产生整数的 Node（FormatField / Bytes(1) + Adapter 等）。
//!   实际场景常见的是 `VarInt`、`Byte`、`Int16ub` 等。
//! - parse 时 countfield 返回的对象必须可 `extract::<i64>()`，否则返回 `Range` 错误。
//!   负数 count 返回 `Range` 错误。
//! - 嵌套数组（PrefixedArray 内 PrefixedArray）：内层 `_index` 覆盖外层，循环结束
//!   后恢复（与 ArrayNode 同语义）。
//! - `sizeof` 永远返回 `Err`（元素数量运行时未知，保守策略）。
//!
//! ## 与 ArrayNode 的差异
//!
//! - PrefixedArray 的 count 来自流（countfield.parse），而非编译期或表达式。
//! - build 时 PrefixedArray 不校验长度（count 超出 countfield 表示范围由
//!   countfield 节点自行报错），ArrayNode build 时严格校验 `len(obj) == count`。
//!
//! ## 性能要点
//!
//! - **lazy path**：成功路径不调 `path.push_index/pop`，
//!   子节点返回 Err 时通过 `ConstructError::push_path_index(i)` 重建索引段。
//!   countfield 错误仍用 `push_path_segment("countfield")`（字段名，非索引）。
//! - **PyList Vec 中转**：用 `Vec<Py<PyAny>>` 收集元素后一次性 `PyList::new_bound`。
//! - parse 比 ArrayNode 多一次 countfield.parse（~15-30ns）。

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;
use pyo3::types::PyList;

// ---------------------------------------------------------------------------
// PrefixedArrayNode
// ---------------------------------------------------------------------------

/// 前缀长度数组节点。
///
/// 对应 Python construct `PrefixedArray(countfield, subcon)`（core.py L4934）。
///
/// # parse 行为
///
/// 1. `countfield.parse(stream)` → 得到 Python 整数对象
/// 2. extract i64；非整数返回 `Range` 错误，负数返回 `Range` 错误
/// 3. 创建 PyList（预分配容量）
/// 4. `for i in 0..count`：设置 `ctx._index = i`，inner.parse，append 到 list
/// 5. 循环结束恢复 `ctx._index`
///
/// # build 行为
///
/// 1. 校验 obj 是 list/tuple/iterable，收集到 Vec
/// 2. 先 build countfield：`countfield.build(len, ...)`（超出表示范围由
///    countfield 自行报错）
/// 3. 遍历元素：设置 `ctx._index = i`，inner.build
///
/// # sizeof 行为
///
/// 永远返回 `Err`（保守策略）：元素数量运行时未知。
#[derive(Debug)]
pub struct PrefixedArrayNode {
    /// 计数字段（如 Byte、Int16ub、VarInt）。
    countfield: Box<crate::nodes::Node>,
    /// 元素子树。
    inner: Box<crate::nodes::Node>,
}

impl PrefixedArrayNode {
    /// 创建 PrefixedArrayNode。
    pub fn new(countfield: crate::nodes::Node, inner: crate::nodes::Node) -> Self {
        Self {
            countfield: Box::new(countfield),
            inner: Box::new(inner),
        }
    }

    /// 返回 countfield 子树引用。
    pub fn countfield(&self) -> &crate::nodes::Node {
        &self.countfield
    }

    /// 返回 inner 子树引用。
    pub fn inner(&self) -> &crate::nodes::Node {
        &self.inner
    }

    /// has_expressions 判断。
    /// countfield 与 inner 任一含表达式即返回 true。
    pub fn has_expressions(&self) -> bool {
        self.countfield.has_expressions() || self.inner.has_expressions()
    }
}

// ---------------------------------------------------------------------------
// Construct impl
// ---------------------------------------------------------------------------

impl super::Construct for PrefixedArrayNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 1. 解析 countfield 得到 count（Python 整数对象）。
        let count_obj = self.countfield.parse(py, stream, ctx, path)?;

        // 2. extract 为 i64。非整数返回 Range 错误。
        let count_i64: i64 = count_obj
            .bind(py)
            .extract()
            .map_err(|_| ConstructError::Range {
                message: "PrefixedArray countfield did not produce an integer".to_string(),
                path: path.to_string(),
            })?;

        // 3. 负数 count 返回 Range 错误。
        if count_i64 < 0 {
            return Err(ConstructError::Range {
                message: format!("invalid PrefixedArray count {}", count_i64),
                path: path.to_string(),
            });
        }
        let count = count_i64 as usize;

        // 4. 内联 Array 逻辑（避免构造临时 ArrayNode 实例）。
        // PyList Vec 中转：用 Vec 收集元素后一次性创建 PyList。
        // count 来自流中数据（典型不可信输入），预分配容量按流剩余长度
        // 封顶，防止巨额分配 abort；超出部分由逐元素 parse 自然报 Stream 错误。
        let capacity = super::common::capped_result_capacity(
            count,
            self.inner.sizeof(ctx).ok(),
            stream.remaining(),
        );
        let mut elems: Vec<Py<PyAny>> = Vec::with_capacity(capacity);

        // 保存外层 _index（嵌套数组支持）。
        let old_index = ctx.index();

        for i in 0..count {
            ctx.set_index(i);
            // lazy path：成功路径不调 path.push_index/pop。
            let elem = match self.inner.parse(py, stream, ctx, path) {
                Ok(v) => v,
                Err(mut e) => {
                    e.push_path_index(i);
                    ctx.restore_index(old_index);
                    return Err(e);
                }
            };
            elems.push(elem);
        }

        // 一次性创建 PyList。
        let list = PyList::new_bound(py, elems);

        ctx.restore_index(old_index);
        Ok(list.into_any().unbind())
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        // build 路径统一调 collect_obj_to_vec 收集 obj 到 Vec。
        let items = super::common::collect_obj_to_vec(obj, "PrefixedArray", path)?;

        let count = items.len();

        // 2. 先构建 countfield（写入长度）。
        //    count 超出 countfield 表示范围时，由 countfield 节点自行报错
        //    （如 FormatFieldNode 抛 FormatField/Stream 错误），PrefixedArrayNode
        //    不做额外校验（对齐 Python core.py）。
        let count_py = count.into_py(py);
        let count_bound = count_py.bind(py);
        if let Err(mut e) = self.countfield.build(py, count_bound, stream, ctx, path) {
            e.push_path_segment("countfield");
            return Err(e);
        }

        // 3. 遍历构建元素。
        let old_index = ctx.index();

        for (i, elem) in items.into_iter().enumerate() {
            ctx.set_index(i);
            // lazy path：成功路径不调 path.push_index/pop。
            let elem_bound = elem.bind(py);
            if let Err(mut e) = self.inner.build(py, elem_bound, stream, ctx, path) {
                e.push_path_index(i);
                ctx.restore_index(old_index);
                return Err(e);
            }
        }

        ctx.restore_index(old_index);
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // 保守返回 Err。
        // 元素数量运行时未知，无法静态计算。对齐 Python PrefixedArray._actualsize
        // 需流上下文（无静态 _sizeof）。
        Err(ConstructError::Generic {
            message: "PrefixedArray size depends on stream data".to_string(),
            path: String::new(),
        })
    }
}

// 恢复 ctx._index 由 `Context::restore_index` 方法承担。
//
// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::format_field::{FormatFieldNode, PythonFormat};
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

    /// 在 ctx 中设置字段值（无表达式场景，仅占位）。
    fn setup_ctx<'py>(py: Python<'py>) -> Context<'py> {
        let mut ctx = Context::new_root(py).expect("ctx");
        ctx.init_expr_values(0);
        // 写一个占位字段，确保 expr_values_buf 至少有内容。
        let key = PyString::new_bound(py, "_placeholder").unbind();
        let val = 0i64.into_py(py);
        ctx.set_field_at(0, &key, val.bind(py), py)
            .expect("set_field_at");
        ctx
    }

    /// 构造一个 Byte（Int8ub）节点，用作 inner。
    fn byte_node() -> crate::nodes::Node {
        crate::nodes::Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big))
    }

    /// 构造一个 Int16ub 节点（2 字节）。
    fn int16ub_node() -> crate::nodes::Node {
        crate::nodes::Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big))
    }

    /// 构造一个 PrefixedArray(Byte, Byte) 节点（最常见配置）。
    fn prefixed_byte_byte() -> PrefixedArrayNode {
        PrefixedArrayNode::new(byte_node(), byte_node())
    }

    // ======================================================================
    // 构造器 & 访问器
    // ======================================================================

    #[test]
    fn new_creates_prefixed_array_with_countfield_and_inner() {
        let node = PrefixedArrayNode::new(byte_node(), byte_node());
        assert!(matches!(
            node.countfield(),
            crate::nodes::Node::FormatField(_)
        ));
        assert!(matches!(node.inner(), crate::nodes::Node::FormatField(_)));
    }

    #[test]
    fn new_accepts_different_countfield_and_inner() {
        // PrefixedArray(Int16ub, Int16ub) —— 2 字节前缀 + 2 字节元素
        let node = PrefixedArrayNode::new(int16ub_node(), int16ub_node());
        assert!(matches!(
            node.countfield(),
            crate::nodes::Node::FormatField(_)
        ));
    }

    #[test]
    fn has_expressions_no_expr_returns_false() {
        let node = PrefixedArrayNode::new(byte_node(), byte_node());
        assert!(!node.has_expressions());
    }

    // ======================================================================
    // parse：基本场景
    // ======================================================================

    #[test]
    fn parse_byte_count_reads_n_elements() {
        // countfield=Byte，inner=Byte
        // 数据：[count=3, e0=10, e1=20, e2=30]
        with_py(|py| {
            let node = prefixed_byte_byte();
            let mut stream = ParseStream::new(&[0x03, 0x0A, 0x14, 0x1E]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("is list");
            assert_eq!(list.len(), 3);
            let v0: i64 = list.get_item(0).unwrap().extract().unwrap();
            let v1: i64 = list.get_item(1).unwrap().extract().unwrap();
            let v2: i64 = list.get_item(2).unwrap().extract().unwrap();
            assert_eq!(v0, 10);
            assert_eq!(v1, 20);
            assert_eq!(v2, 30);
            // 流已读完
            assert_eq!(stream.tell(), 4);
        });
    }

    #[test]
    fn parse_count_zero_returns_empty_list_but_consumes_countfield() {
        // count = 0 → 返回空 list（countfield 已消耗）
        with_py(|py| {
            let node = prefixed_byte_byte();
            let mut stream = ParseStream::new(&[0x00, 0xFF, 0xFF]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("is list");
            assert_eq!(list.len(), 0);
            // countfield 已消耗 1 字节
            assert_eq!(stream.tell(), 1);
        });
    }

    #[test]
    fn parse_int16ub_countfield_reads_2_byte_prefix() {
        // PrefixedArray(Int16ub, Byte)，count = 0x0002
        // 数据：[0x00, 0x02, 0xAB, 0xCD]
        with_py(|py| {
            let node = PrefixedArrayNode::new(int16ub_node(), byte_node());
            let mut stream = ParseStream::new(&[0x00, 0x02, 0xAB, 0xCD]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("is list");
            assert_eq!(list.len(), 2);
            let v0: i64 = list.get_item(0).unwrap().extract().unwrap();
            let v1: i64 = list.get_item(1).unwrap().extract().unwrap();
            assert_eq!(v0, 0xAB);
            assert_eq!(v1, 0xCD);
            assert_eq!(stream.tell(), 4);
        });
    }

    #[test]
    fn parse_count_insufficient_returns_stream_error() {
        // count > 实际可解析元素数 → inner.parse 失败时错误向上传播
        with_py(|py| {
            let node = prefixed_byte_byte();
            // count=5，但只提供 2 个字节
            let mut stream = ParseStream::new(&[0x05, 0x01, 0x02]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Stream { message, .. } => {
                    assert!(message.contains("expected 1"), "got: {}", message);
                }
                other => panic!("expected Stream, got {:?}", other),
            }
        });
    }

    #[test]
    fn parse_countfield_eof_returns_stream_error() {
        // countfield 自身读不到字节（流空）
        with_py(|py| {
            let node = prefixed_byte_byte();
            let mut stream = ParseStream::new(&[]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            // countfield.parse 失败应向上传播（Stream 错误）
            match err {
                ConstructError::Stream { message, .. } => {
                    assert!(message.contains("expected 1"), "got: {}", message);
                }
                other => panic!("expected Stream, got {:?}", other),
            }
        });
    }

    #[test]
    fn parse_count_not_integer_returns_range_error() {
        // countfield 解析结果不是整数（如 countfield 是 Bytes(2)）
        // 用 Bytes(2) 作为 countfield，会返回 bytes 而非 int → extract i64 失败
        with_py(|py| {
            let countfield =
                crate::nodes::Node::Bytes(crate::nodes::bytes::BytesNode::new_const(2));
            let node = PrefixedArrayNode::new(countfield, byte_node());
            let mut stream = ParseStream::new(&[0xAA, 0xBB, 0x01, 0x02]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Range { message, .. } => {
                    assert!(
                        message.contains("did not produce an integer"),
                        "got: {}",
                        message
                    );
                }
                other => panic!("expected Range, got {:?}", other),
            }
        });
    }

    #[test]
    fn parse_countfield_signed_negative_returns_range_error() {
        // countfield 是 Int8sb（signed），解析得到负数 → Range 错误
        // 注意：FormatFieldDescriptor 当前仅有 unsigned 8-bit，用 Int8sb 单例测试
        with_py(|py| {
            let countfield =
                crate::nodes::Node::FormatField(FormatFieldNode::new(PythonFormat::SignedInt8Big));
            let node = PrefixedArrayNode::new(countfield, byte_node());
            let mut stream = ParseStream::new(&[0xFF]); // Int8sb(0xFF) = -1
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Range { message, .. } => {
                    assert!(message.contains("-1"), "got: {}", message);
                }
                other => panic!("expected Range, got {:?}", other),
            }
        });
    }

    #[test]
    fn parse_huge_stream_count_returns_stream_error_not_abort() {
        // 流内 count=0xFFFFFFFF（典型恶意/损坏长度字段）：预分配按流剩余长度
        // 封顶，解析在流耗尽处返回可 catch 的 Stream 错误（而非分配失败 abort）。
        with_py(|py| {
            let countfield = crate::nodes::Node::FormatField(FormatFieldNode::new(
                PythonFormat::UnsignedInt32Big,
            ));
            let node = PrefixedArrayNode::new(countfield, byte_node());
            let mut stream = ParseStream::new(&[0xFF, 0xFF, 0xFF, 0xFF, 0x01, 0x02]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Stream { .. }));
        });
    }

    #[test]
    fn parse_restores_index_after_completion() {
        // 循环结束后 _index 应被恢复为入口值（None）
        with_py(|py| {
            let node = prefixed_byte_byte();
            let mut stream = ParseStream::new(&[0x03, 0x01, 0x02, 0x03]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let _ = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert!(ctx.index().is_none());
        });
    }

    // ======================================================================
    // parse：嵌套 PrefixedArray
    // ======================================================================

    #[test]
    fn parse_nested_prefixed_array_restores_outer_index() {
        // PrefixedArray(Byte, PrefixedArray(Byte, Byte))
        // 数据布局：
        //   外层 count=2
        //     内层 0: count=1, e=0x10
        //     内层 1: count=2, e=0x20, e=0x30
        with_py(|py| {
            let inner = PrefixedArrayNode::new(byte_node(), byte_node());
            let outer_node = crate::nodes::Node::PrefixedArray(PrefixedArrayNode::new(
                byte_node(),
                crate::nodes::Node::PrefixedArray(inner),
            ));
            let mut stream = ParseStream::new(&[
                0x02, // outer count
                0x01, 0x10, // inner 0: count=1, [0x10]
                0x02, 0x20, 0x30, // inner 1: count=2, [0x20, 0x30]
            ]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = outer_node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let outer_list = result.bind(py).downcast::<PyList>().expect("outer list");
            assert_eq!(outer_list.len(), 2);
            // 内层 0
            let inner0 = outer_list.get_item(0).unwrap();
            let inner0_list = inner0.downcast::<PyList>().expect("inner0 list");
            assert_eq!(inner0_list.len(), 1);
            let v: i64 = inner0_list.get_item(0).unwrap().extract().unwrap();
            assert_eq!(v, 0x10);
            // 内层 1
            let inner1 = outer_list.get_item(1).unwrap();
            let inner1_list = inner1.downcast::<PyList>().expect("inner1 list");
            assert_eq!(inner1_list.len(), 2);
            let v0: i64 = inner1_list.get_item(0).unwrap().extract().unwrap();
            let v1: i64 = inner1_list.get_item(1).unwrap().extract().unwrap();
            assert_eq!(v0, 0x20);
            assert_eq!(v1, 0x30);
            // 外层 _index 恢复 None
            assert!(ctx.index().is_none());
        });
    }

    // ======================================================================
    // build
    // ======================================================================

    #[test]
    fn build_writes_countfield_then_elements() {
        with_py(|py| {
            let node = prefixed_byte_byte();
            let obj = py.eval_bound("[10, 20, 30]", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            // 第一个字节是 count（3），后续是元素
            assert_eq!(stream.as_bytes(), &[3, 10, 20, 30]);
        });
    }

    #[test]
    fn build_writes_int16ub_countfield() {
        // PrefixedArray(Int16ub, Byte)：count 用 2 字节大端
        with_py(|py| {
            let node = PrefixedArrayNode::new(int16ub_node(), byte_node());
            let obj = py.eval_bound("[0xAB, 0xCD]", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            // count=2 as big-endian u16 = 0x00 0x02，后跟 0xAB 0xCD
            assert_eq!(stream.as_bytes(), &[0x00, 0x02, 0xAB, 0xCD]);
        });
    }

    #[test]
    fn build_empty_list_writes_zero_count() {
        with_py(|py| {
            let node = prefixed_byte_byte();
            let obj = py.eval_bound("[]", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            // countfield=0，无元素
            assert_eq!(stream.as_bytes(), &[0]);
        });
    }

    #[test]
    fn build_writes_tuple_elements() {
        with_py(|py| {
            let node = prefixed_byte_byte();
            let obj = py.eval_bound("(1, 2, 3)", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[3, 1, 2, 3]);
        });
    }

    #[test]
    fn build_iterable_object_works() {
        // 兜底路径：传入 generator（非 list/tuple）
        with_py(|py| {
            let node = prefixed_byte_byte();
            let obj = py
                .eval_bound("(x for x in [5, 6, 7])", None, None)
                .expect("eval generator");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[3, 5, 6, 7]);
        });
    }

    #[test]
    fn build_propagates_inner_error() {
        // inner.build 失败 → 错误向上传播
        // 用 Int16ub 作为 inner，传入 bytes 制造类型错
        with_py(|py| {
            let node = PrefixedArrayNode::new(byte_node(), int16ub_node());
            let obj = py.eval_bound("[b'bad']", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            // lazy path：error.path 由 inner.build 产生 "root"，PrefixedArray 重建为 "root[0]"。
            let p = err.path().unwrap_or("");
            assert!(
                p == "root[0]" || p.ends_with("[0]"),
                "lazy path format: expected 'root[0]' or ending with '[0]', got '{}'",
                p
            );
            assert!(
                !p.contains(".["),
                "lazy path format: dot before '[' is invalid (got '{}')",
                p
            );
        });
    }

    #[test]
    fn build_count_overflow_propagates_countfield_error() {
        // count 超出 countfield 表示范围 → 由 countfield 自行报错
        // 用 Byte 作 countfield，list 含 256 个元素 → Byte 无法表示 256
        with_py(|py| {
            let node = prefixed_byte_byte();
            let obj = py
                .eval_bound("list(range(256))", None, None)
                .expect("eval 256-list");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            // FormatFieldNode 应抛 FormatField 错误（256 超出 u8 范围）
            // 路径含 "countfield" 段
            let path_str = err.path().unwrap_or("");
            assert!(
                path_str.contains("countfield"),
                "expected countfield in path, got: {}",
                path_str
            );
        });
    }

    #[test]
    fn build_restores_index_after_completion() {
        with_py(|py| {
            let node = prefixed_byte_byte();
            let obj = py.eval_bound("[1, 2, 3]", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert!(ctx.index().is_none());
        });
    }

    // ======================================================================
    // lazy path 嵌套组合测试
    // ======================================================================

    #[test]
    fn parse_struct_inside_prefixed_array_error_path() {
        // PrefixedArray(Byte, Struct{x: Byte})，
        // Array[1].x EOF → path = "root[1].x"
        // 重建顺序（从叶到根）：
        //   1. leaf FormatField error path = "root"
        //   2. Struct.push_segment("x"): "root" → "root.x"
        //   3. PrefixedArray.push_path_index(1): "root.x" → "root[1].x"
        with_py(|py| {
            use crate::nodes::struct_node::{
                FieldMode, FieldName, StructField, StructNode, ValueKind,
            };
            use pyo3::types::PyType;
            let inner_struct_fields = vec![StructField {
                name: FieldName::new(py, "x"),
                node: byte_node(),
                mode: FieldMode::Rw,
                kind: ValueKind::classify(py, &byte_node(), false),
            }];
            let cls = py
                .eval_bound("type('Item', (), {})", None, None)
                .expect("cls")
                .extract::<Py<PyType>>()
                .expect("PyType");
            let inner = crate::nodes::Node::Struct(StructNode::new(
                py,
                inner_struct_fields,
                cls,
                false,
                false,
            ));
            let node = PrefixedArrayNode::new(byte_node(), inner);
            // count=3，仅提供 1 字节 inner 数据 → Array[0].x 成功，Array[1].x EOF
            // 流布局：[count=3, e0.x=0x10]
            let mut stream = ParseStream::new(&[0x03, 0x10]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail on PrefixedArray[1].x EOF");
            match err {
                ConstructError::Stream { path: p, .. } => {
                    assert!(p == "root[1].x", "expected 'root[1].x', got '{}'", p);
                }
                other => panic!("expected Stream, got {:?}", other),
            }
        });
    }

    #[test]
    fn parse_returns_native_list_type() {
        // 验证 Vec 中转后返回的仍是原生 list 类型。
        with_py(|py| {
            let node = prefixed_byte_byte();
            let mut stream = ParseStream::new(&[0x03, 0x01, 0x02, 0x03]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let result_bound = result.bind(py);
            let is_list: bool = result_bound.is_instance_of::<PyList>();
            assert!(is_list, "Vec 中转后应返回原生 list");
        });
    }

    // ======================================================================
    // parse ↔ build 往返
    // ======================================================================

    #[test]
    fn round_trip_prefixed_array_preserves_data() {
        with_py(|py| {
            let node = prefixed_byte_byte();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            // build
            let obj = py
                .eval_bound("[100, 110, 120, 130, 140]", None, None)
                .expect("eval");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();

            // parse 回来
            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("list");
            assert_eq!(list.len(), 5);
            let values: Vec<i64> = list.iter().map(|b| b.extract().unwrap()).collect();
            assert_eq!(values, vec![100, 110, 120, 130, 140]);
        });
    }

    #[test]
    fn round_trip_int16ub_countfield_preserves_data() {
        with_py(|py| {
            let node = PrefixedArrayNode::new(int16ub_node(), byte_node());
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            let obj = py
                .eval_bound("[10, 20, 30, 40, 50, 60, 70, 80]", None, None)
                .expect("eval");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();

            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("list");
            assert_eq!(list.len(), 8);
            let values: Vec<i64> = list.iter().map(|b| b.extract().unwrap()).collect();
            assert_eq!(values, vec![10, 20, 30, 40, 50, 60, 70, 80]);
        });
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_always_returns_error() {
        // sizeof 永远 Err
        with_py(|py| {
            let ctx = setup_ctx(py);
            let node = prefixed_byte_byte();
            let err = node.sizeof(&ctx).expect_err("should fail");
            match err {
                ConstructError::Generic { message, .. } => {
                    assert!(message.contains("PrefixedArray"), "got: {}", message);
                }
                other => panic!("expected Generic, got {:?}", other),
            }
        });
    }
}
