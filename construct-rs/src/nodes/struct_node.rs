//! StructNode：字段序列根节点。
//!
//! 设计依据：`docs/架构设计.md` §C.3.4。
//! Python 参考：`construct/construct/core.py` `Struct._parse` / `_build`（L2162-2268）。
//!
//! ## 行为概述
//!
//! StructNode 是 StructMixin 子类执行树的根节点：按顺序解析/构建一组命名字段。
//!
//! - **parse**：创建 `PyDict`，逐字段递归子节点解析并 `set_item`，最后返回 dict。
//!   Python 侧用 `cls(**dict)` 构造实例（见 `docs/架构设计.md` §A.2、§B.7）。
//! - **build**：逐字段从 Python 对象 `getattr` 取值，递归子节点构建，写入 stream。
//! - **sizeof**：累加所有字段 sizeof；任一字段返回 Err 则整体返回 Err。
//!
//! ## 与 Python construct Struct 的对齐
//!
//! Python construct 的 `Struct._parse` 创建 `Container`（dict 子类），遍历 `subcons`，
//! 每个有名 subcon 调用 `_parsereport`，结果存入 `obj[sc.name]` 和 `context[sc.name]`。
//! 我们的实现语义一致：创建 `PyDict`，每个字段递归 parse 后 `dict.set_item` 和
//! `ctx.set_field`。
//!
//! ## 关于 Context 的嵌套
//!
//! 设计文档 §C.5 提到 Struct 节点 parse 时 `new_child` 创建嵌套 context。但子任务 1.4
//! 的 Phase 1 范围不支持 this 表达式，context 仅用于存储（不读取），不创建嵌套层不影响
//! 正确性。本实现按子任务要求直接使用传入的 ctx 调用 `set_field`。

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;
use pyo3::types::PyDict;

use super::{Construct, Node};

/// StructMixin 子类的根节点：按顺序解析/构建一组命名字段。
///
/// 对应 Python construct 的 `Struct`。
///
/// # parse 行为
///
/// 1. 创建 `PyDict`（C API `PyDict_New`）。
/// 2. 对每个字段 `(name, node)`：
///    - `path.push_field(name)` —— 进入字段路径，错误时 path 含字段名
///    - `node.parse(...)` —— 递归解析子节点
///    - `dict.set_item(name, value)` —— C API `PyDict_SetItem`
///    - `ctx.set_field(name, value)` —— 存入上下文（供 Phase 2 this 引用）
///    - `path.pop()` —— 离开字段路径
/// 3. 返回 `PyDict`（Python 侧 `cls(**dict)` 构造实例）。
///
/// parse **不要求消费全部输入**：多余字节被忽略（对齐 Python construct Struct 语义）。
///
/// # build 行为
///
/// 对每个字段 `(name, node)`：
/// - `obj.getattr(name)` —— 从 Python 对象读取属性（C API `PyObject_GetAttr`）
/// - `path.push_field(name)`
/// - `ctx.set_field(name, value)`
/// - `node.build(value, ...)` —— 递归构建子节点
/// - `path.pop()`
///
/// # sizeof
///
/// 累加所有字段 sizeof；任一字段返回 Err（如 `GreedyBytes`）则整体返回 Err。
pub struct StructNode {
    /// 有序字段列表：`(字段名, 子节点)`。
    fields: Vec<(String, Node)>,
}

impl StructNode {
    /// 创建一个 `StructNode`，包含给定的有序字段列表。
    ///
    /// 字段顺序即声明顺序，决定 parse/build 的字段处理顺序。
    pub fn new(fields: Vec<(String, Node)>) -> Self {
        Self { fields }
    }

    /// 返回字段数量。
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// 是否为空结构体（无字段）。
    ///
    /// 空结构体合法：parse(b'') → {}，build({}) → b''（见 §A.6）。
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// 返回字段列表的只读切片。
    pub fn fields(&self) -> &[(String, Node)] {
        &self.fields
    }
}

impl Construct for StructNode {
    fn parse(
        &self,
        py: Python<'_>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let dict = PyDict::new_bound(py);
        for (name, node) in &self.fields {
            path.push_field(name);
            let value = node.parse(py, stream, ctx, path)?;
            let value_bound = value.bind(py);
            dict.set_item(name, value_bound)
                .map_err(|e| ConstructError::Generic {
                    message: format!("failed to set dict item for field '{}': {}", name, e),
                    path: path.to_string(),
                })?;
            ctx.set_field(name, value_bound)
                .map_err(|e| ConstructError::Generic {
                    message: format!("failed to set context field '{}': {}", name, e),
                    path: path.to_string(),
                })?;
            path.pop();
        }
        Ok(dict.into_any().unbind())
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        for (name, node) in &self.fields {
            // 从 Python 对象读取属性（getattr 在 push_field 之前，
            // 对齐设计文档 §C.3.4。错误消息含字段名以辅助定位）。
            let value = obj
                .getattr(name.as_str())
                .map_err(|e| ConstructError::Generic {
                    message: format!(
                        "object has no attribute '{}' (required for build): {}",
                        name, e
                    ),
                    path: path.to_string(),
                })?;
            path.push_field(name);
            ctx.set_field(name, &value)
                .map_err(|e| ConstructError::Generic {
                    message: format!("failed to set context field '{}': {}", name, e),
                    path: path.to_string(),
                })?;
            node.build(py, &value, stream, ctx, path)?;
            path.pop();
        }
        Ok(())
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        let mut total = 0usize;
        for (_, node) in &self.fields {
            total =
                total
                    .checked_add(node.sizeof(ctx)?)
                    .ok_or_else(|| ConstructError::Generic {
                        message: "struct size overflowed usize".to_string(),
                        path: String::new(),
                    })?;
        }
        Ok(total)
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

    /// 构造 Int8ub 节点的便捷函数。
    fn u8_node() -> Node {
        Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big))
    }

    /// 构造 Int16ub 节点的便捷函数。
    fn u16_node() -> Node {
        Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big))
    }

    // ======================================================================
    // 空结构体
    // ======================================================================

    #[test]
    fn empty_struct_parse_empty_bytes_returns_empty_dict() {
        with_py(|py| {
            let node = StructNode::new(Vec::new());
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let dict = result.bind(py).downcast::<PyDict>().expect("is dict");
            assert_eq!(dict.len(), 0);
            assert_eq!(stream.tell(), 0);
        });
    }

    #[test]
    fn empty_struct_parse_ignores_extra_bytes() {
        // 对齐 Python construct Struct：不要求消费全部输入。
        with_py(|py| {
            let node = StructNode::new(Vec::new());
            let mut stream = ParseStream::new(b"abc");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let dict = result.bind(py).downcast::<PyDict>().expect("is dict");
            assert_eq!(dict.len(), 0);
            // 多余字节未被消费
            assert_eq!(stream.tell(), 0);
            assert!(!stream.is_at_end());
        });
    }

    #[test]
    fn empty_struct_build_empty_dict_returns_empty_bytes() {
        with_py(|py| {
            let node = StructNode::new(Vec::new());
            let obj = py.eval_bound("{}", None, None).expect("empty dict");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert!(stream.as_bytes().is_empty());
        });
    }

    // ======================================================================
    // 单字段结构体
    // ======================================================================

    #[test]
    fn single_field_parse_returns_dict_with_value() {
        with_py(|py| {
            let node = StructNode::new(vec![("x".to_string(), u8_node())]);
            let mut stream = ParseStream::new(&[0x42]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let dict = result.bind(py).downcast::<PyDict>().expect("is dict");
            assert_eq!(dict.len(), 1);
            let x: i64 = dict
                .get_item("x")
                .expect("get_item ok")
                .expect("x exists")
                .extract()
                .expect("extract i64");
            assert_eq!(x, 0x42);
        });
    }

    #[test]
    fn single_field_build_writes_bytes() {
        with_py(|py| {
            let node = StructNode::new(vec![("x".to_string(), u8_node())]);
            // 用简单 Python 对象 {x: 42}
            let obj = py
                .eval_bound("type('O', (), {'x': 42})()", None, None)
                .expect("create obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[42]);
        });
    }

    #[test]
    fn single_field_round_trip_preserves_value() {
        with_py(|py| {
            let node = StructNode::new(vec![("x".to_string(), u8_node())]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            // build
            let obj = py
                .eval_bound("type('O', (), {'x': 200})()", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();

            // parse
            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let dict = result.bind(py).downcast::<PyDict>().expect("dict");
            let x: i64 = dict
                .get_item("x")
                .expect("ok")
                .expect("exist")
                .extract()
                .expect("extract");
            assert_eq!(x, 200);
        });
    }

    // ======================================================================
    // 多字段结构体
    // ======================================================================

    #[test]
    fn multi_field_parse_returns_all_values_in_order() {
        // Int8ub + Int16ub + Bytes(2)
        with_py(|py| {
            let node = StructNode::new(vec![
                ("a".to_string(), u8_node()),
                ("b".to_string(), u16_node()),
                ("c".to_string(), Node::Bytes(BytesNode::new(2))),
            ]);
            // a=0x01, b=0x0203, c=0x0405
            let mut stream = ParseStream::new(&[0x01, 0x02, 0x03, 0x04, 0x05]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let dict = result.bind(py).downcast::<PyDict>().expect("dict");
            assert_eq!(dict.len(), 3);
            let a: i64 = dict.get_item("a").unwrap().unwrap().extract().unwrap();
            let b: i64 = dict.get_item("b").unwrap().unwrap().extract().unwrap();
            let c_binding = dict.get_item("c").unwrap().unwrap();
            let c: &[u8] = c_binding.extract().unwrap();
            assert_eq!(a, 1);
            assert_eq!(b, 0x0203);
            assert_eq!(c, &[0x04, 0x05]);
            assert!(stream.is_at_end());
        });
    }

    #[test]
    fn multi_field_build_writes_all_fields_in_order() {
        with_py(|py| {
            let node = StructNode::new(vec![
                ("a".to_string(), u8_node()),
                ("b".to_string(), u16_node()),
                ("c".to_string(), Node::Bytes(BytesNode::new(2))),
            ]);
            let obj = py
                .eval_bound(
                    "type('O', (), {'a': 1, 'b': 0x0203, 'c': b'\\x04\\x05'})()",
                    None,
                    None,
                )
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x01, 0x02, 0x03, 0x04, 0x05]);
        });
    }

    #[test]
    fn multi_field_parse_does_not_consume_extra_bytes() {
        with_py(|py| {
            let node = StructNode::new(vec![
                ("a".to_string(), u8_node()),
                ("b".to_string(), u8_node()),
            ]);
            // 只消费 2 字节，多余 3 字节忽略
            let mut stream = ParseStream::new(&[0x10, 0x20, 0x30, 0x40, 0x50]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let dict = result.bind(py).downcast::<PyDict>().expect("dict");
            let a: i64 = dict.get_item("a").unwrap().unwrap().extract().unwrap();
            let b: i64 = dict.get_item("b").unwrap().unwrap().extract().unwrap();
            assert_eq!(a, 0x10);
            assert_eq!(b, 0x20);
            assert_eq!(stream.tell(), 2);
            assert!(!stream.is_at_end());
        });
    }

    // ======================================================================
    // build 缺字段 → 错误
    // ======================================================================

    #[test]
    fn build_missing_field_returns_generic_error_with_field_name() {
        with_py(|py| {
            let node = StructNode::new(vec![
                ("a".to_string(), u8_node()),
                ("b".to_string(), u8_node()),
            ]);
            // 对象只有 a，缺 b
            let obj = py
                .eval_bound("type('O', (), {'a': 1})()", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Generic { message, .. } => {
                    assert!(
                        message.contains("b"),
                        "error should mention field 'b': {}",
                        message
                    );
                }
                other => panic!("expected Generic error, got {:?}", other),
            }
            // a 已写入，b 未写入
            assert_eq!(stream.as_bytes(), &[1]);
        });
    }

    // ======================================================================
    // 嵌套 StructNode（StructNode 内嵌 StructNode）
    // ======================================================================

    #[test]
    fn nested_struct_parse_returns_nested_dict() {
        with_py(|py| {
            // 外层 { len: Int8ub, inner: Struct { value: Int8ub } }
            let inner = Node::Struct(StructNode::new(vec![("value".to_string(), u8_node())]));
            let outer = StructNode::new(vec![
                ("len".to_string(), u8_node()),
                ("inner".to_string(), inner),
            ]);
            let mut stream = ParseStream::new(&[0x03, 0x42]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = outer
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let dict = result.bind(py).downcast::<PyDict>().expect("dict");
            let len: i64 = dict.get_item("len").unwrap().unwrap().extract().unwrap();
            assert_eq!(len, 3);
            let inner_dict = dict.get_item("inner").unwrap().unwrap();
            let inner_dict = inner_dict.downcast::<PyDict>().expect("inner dict");
            let value: i64 = inner_dict
                .get_item("value")
                .unwrap()
                .unwrap()
                .extract()
                .unwrap();
            assert_eq!(value, 0x42);
        });
    }

    #[test]
    fn nested_struct_build_writes_nested_dict() {
        with_py(|py| {
            let inner = Node::Struct(StructNode::new(vec![("value".to_string(), u8_node())]));
            let outer = StructNode::new(vec![
                ("len".to_string(), u8_node()),
                ("inner".to_string(), inner),
            ]);
            // 构造嵌套 dict 对象
            let obj = py
                .eval_bound(
                    "type('O', (), {'len': 3, 'inner': type('I',(),{'value':0x42})()})()",
                    None,
                    None,
                )
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            outer
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x03, 0x42]);
        });
    }

    #[test]
    fn nested_struct_round_trip() {
        with_py(|py| {
            let inner = Node::Struct(StructNode::new(vec![("value".to_string(), u8_node())]));
            let outer = StructNode::new(vec![
                ("a".to_string(), u8_node()),
                ("inner".to_string(), inner),
                ("b".to_string(), u8_node()),
            ]);

            // build
            let obj = py
                .eval_bound(
                    "type('O', (), {'a': 0xAA, 'inner': type('I',(),{'value':0xBB})(), 'b': 0xCC})()",
                    None,
                    None,
                )
                .expect("obj");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let mut stream = BuildStream::new();
            outer
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();
            assert_eq!(bytes, &[0xAA, 0xBB, 0xCC]);

            // parse 回来
            let mut pstream = ParseStream::new(&bytes);
            let result = outer
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let dict = result.bind(py).downcast::<PyDict>().expect("dict");
            let a: i64 = dict.get_item("a").unwrap().unwrap().extract().unwrap();
            assert_eq!(a, 0xAA);
            let inner_dict = dict.get_item("inner").unwrap().unwrap();
            let inner_dict = inner_dict.downcast::<PyDict>().expect("inner dict");
            let value: i64 = inner_dict
                .get_item("value")
                .unwrap()
                .unwrap()
                .extract()
                .unwrap();
            assert_eq!(value, 0xBB);
            let b: i64 = dict.get_item("b").unwrap().unwrap().extract().unwrap();
            assert_eq!(b, 0xCC);
        });
    }

    // ======================================================================
    // 错误 path 追踪
    // ======================================================================

    #[test]
    fn error_path_tracks_field_on_stream_error() {
        with_py(|py| {
            // { a: Int8ub, b: Int32ub }
            // b 读取时字节不足
            let node = StructNode::new(vec![
                ("a".to_string(), u8_node()),
                (
                    "b".to_string(),
                    Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt32Big)),
                ),
            ]);
            let mut stream = ParseStream::new(&[0x01]); // 只够 a，b 需 4 字节
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Stream { path, message } => {
                    assert!(path.contains(".b"), "path should contain '.b': {}", path);
                    assert!(message.contains("expected 4"), "got: {}", message);
                }
                other => panic!("expected Stream error, got {:?}", other),
            }
        });
    }

    #[test]
    fn error_path_tracks_nested_field() {
        with_py(|py| {
            // { a: Int8ub, inner: Struct { c: Int32ub } }
            let inner = Node::Struct(StructNode::new(vec![(
                "c".to_string(),
                Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt32Big)),
            )]));
            let outer = StructNode::new(vec![
                ("a".to_string(), u8_node()),
                ("inner".to_string(), inner),
            ]);
            // 只够 a（1 字节），inner.c 需要 4 字节
            let mut stream = ParseStream::new(&[0x01]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = outer
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Stream { path, message } => {
                    // path 应为 "root.inner.c"
                    assert!(
                        path.contains("inner"),
                        "path should contain 'inner': {}",
                        path
                    );
                    assert!(path.contains("c"), "path should contain 'c': {}", path);
                    assert!(message.contains("expected 4"), "got: {}", message);
                }
                other => panic!("expected Stream error, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_empty_struct_is_zero() {
        with_py(|py| {
            let ctx = Context::new_root(py).expect("ctx");
            let node = StructNode::new(Vec::new());
            assert_eq!(node.sizeof(&ctx).unwrap(), 0);
        });
    }

    #[test]
    fn sizeof_sums_all_fields() {
        with_py(|py| {
            let ctx = Context::new_root(py).expect("ctx");
            let node = StructNode::new(vec![
                ("a".to_string(), u8_node()),                      // 1
                ("b".to_string(), u16_node()),                     // 2
                ("c".to_string(), Node::Bytes(BytesNode::new(4))), // 4
            ]);
            assert_eq!(node.sizeof(&ctx).unwrap(), 7);
        });
    }

    #[test]
    fn sizeof_with_greedy_bytes_returns_err() {
        with_py(|py| {
            use crate::nodes::greedy_bytes::GreedyBytesNode;
            let ctx = Context::new_root(py).expect("ctx");
            let node = StructNode::new(vec![
                ("a".to_string(), u8_node()),
                ("b".to_string(), Node::GreedyBytes(GreedyBytesNode::new())),
            ]);
            assert!(node.sizeof(&ctx).is_err());
        });
    }

    // ======================================================================
    // StructNode 访问器
    // ======================================================================

    #[test]
    fn len_and_is_empty() {
        let empty = StructNode::new(Vec::new());
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);

        let nonempty = StructNode::new(vec![("x".to_string(), u8_node())]);
        assert!(!nonempty.is_empty());
        assert_eq!(nonempty.len(), 1);
    }

    #[test]
    fn fields_returns_slice() {
        let node = StructNode::new(vec![
            ("a".to_string(), u8_node()),
            ("b".to_string(), u16_node()),
        ]);
        let fields = node.fields();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].0, "a");
        assert_eq!(fields[1].0, "b");
    }
}
