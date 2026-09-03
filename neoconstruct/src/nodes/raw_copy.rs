//! RawCopyNode：捕获原始字节的节点。
//!
//! Python 参考：`construct/construct/core.py` `RawCopy`（L4761-4814）。
//!
//! ## 行为概述
//!
//! RawCopy 在 parse 时记录入口 offset1，执行 inner.parse，记录出口 offset2，
//! 然后 seek 回 offset1 重读 length = offset2 - offset1 字节作为 raw bytes。
//! 返回 Python dict（含 5 键：data/value/offset1/offset2/length）。
//!
//! build 支持两种输入：
//! - 含 `'data'` 键：直接 write(data)
//! - 含 `'value'` 键：inner.build(value)
//! - 否则：RawCopyError（Generic）
//!
//! ## 已知差异
//!
//! Python RawCopy.build 返回 Container(data=...)（供后续字段引用）。
//! neoconstruct 的 build 不返回值（"一次 FFI"原则的体现），用户面无法拿到
//! build 出的 raw bytes。用户需要 raw bytes 应走 parse 路径。

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};

use super::Construct;
use crate::nodes::Node;

/// 原始字节捕获节点：parse 返回 dict(data,value,offset1,offset2,length)。
///
/// 对应 Python construct `RawCopy`（core.py L4761）。
///
/// # parse 行为
///
/// 1. `offset1 = stream.tell()`
/// 2. `value = inner.parse(...)`
/// 3. `offset2 = stream.tell()`
/// 4. `length = offset2 - offset1`
/// 5. seek 回 offset1，重读 length 字节得到 raw bytes
/// 6. 构造并返回 Python dict（5 键）
///
/// # build 行为
///
/// - 含 `'data'` 键：直接 write(data)（不调 inner.build）
/// - 含 `'value'` 键：inner.build(value)
/// - 否则：`ConstructError::Generic`
///
/// # 一次 FFI 合规性
///
/// parse 用 `PyDict` 直接构造（dict 本身就是 Python 对象——
/// 与 StructNode 用实例 `__dict__` 同脉络）。
#[derive(Debug)]
pub struct RawCopyNode {
    /// 被捕获原始字节的子树根。
    inner: Box<Node>,
}

impl RawCopyNode {
    /// 创建 `RawCopyNode`，包裹给定的子树根节点。
    pub fn new(inner: Node) -> Self {
        Self {
            inner: Box::new(inner),
        }
    }

    /// 返回内部子树根节点的引用。
    pub fn inner(&self) -> &Node {
        &self.inner
    }
}

impl Construct for RawCopyNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let offset1 = stream.tell();
        let value = self.inner.parse(py, stream, ctx, path)?;
        let offset2 = stream.tell();

        // 防御性：offset2 应 >= offset1（inner 不应反向回退 stream）。
        // checked_sub 防 underflow，错误时返回结构化错误而非 panic。
        let length = offset2.checked_sub(offset1).ok_or_else(|| {
            ConstructError::Generic {
                message: format!(
                    "RawCopy: stream position regressed during inner parse (offset1={}, offset2={})",
                    offset1, offset2
                ),
                path: path.to_string(),
            }
        })?;

        // seek 回 offset1 重读 raw bytes（对齐 Python RawCopy._parse L4786-4787）
        stream.seek(offset1, path)?;
        let data_slice = stream.read(length, path)?;

        // 构造 Python dict（直接 PyDict，无中间 Container 类型）
        let dict = PyDict::new_bound(py);
        dict.set_item("data", PyBytes::new_bound(py, data_slice))?;
        dict.set_item("value", value.bind(py))?;
        dict.set_item("offset1", (offset1 as i64).into_py(py).bind(py))?;
        dict.set_item("offset2", (offset2 as i64).into_py(py).bind(py))?;
        dict.set_item("length", (length as i64).into_py(py).bind(py))?;
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
        // Python 'data' in obj / 'value' in obj → 用 contains 检查 dict 键
        // （RawCopy 字段值通常是 dict）。
        let has_data = obj.contains("data").map_err(|e| ConstructError::Generic {
            message: format!("RawCopy build: failed to check 'data' key: {}", e),
            path: path.to_string(),
        })?;
        let has_value = obj.contains("value").map_err(|e| ConstructError::Generic {
            message: format!("RawCopy build: failed to check 'value' key: {}", e),
            path: path.to_string(),
        })?;

        if has_data {
            // 含 data 键：直接 write(data)，不调 inner.build
            let data_obj = obj.get_item("data").map_err(|e| ConstructError::Generic {
                message: format!("RawCopy build: failed to get 'data' item: {}", e),
                path: path.to_string(),
            })?;
            let data_bytes: &[u8] = data_obj.extract().map_err(|_| ConstructError::Generic {
                message: "RawCopy build: 'data' value must be bytes-like".to_string(),
                path: path.to_string(),
            })?;
            stream.write(data_bytes);
            Ok(())
        } else if has_value {
            // 含 value 键：inner.build(value)
            // 注：neoconstruct build 不返回值，无法回流 raw bytes
            let value = obj.get_item("value").map_err(|e| ConstructError::Generic {
                message: format!("RawCopy build: failed to get 'value' item: {}", e),
                path: path.to_string(),
            })?;
            self.inner.build(py, &value, stream, ctx, path)
        } else {
            // 无 data 也无 value → RawCopyError（Generic）
            Err(ConstructError::Generic {
                message: "RawCopy cannot build: both 'data' and 'value' keys are missing"
                    .to_string(),
                path: path.to_string(),
            })
        }
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
    use crate::nodes::format_field::{FormatFieldNode, PythonFormat};
    use pyo3::types::PyDict;

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

    // ======================================================================
    // 构造器
    // ======================================================================

    #[test]
    fn new_stores_inner() {
        let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
        let node = RawCopyNode::new(inner);
        assert!(matches!(node.inner(), Node::FormatField(_)));
    }

    // ======================================================================
    // parse
    // ======================================================================

    #[test]
    fn parse_int8ub_returns_full_dict() {
        // RawCopy(Int8ub) parse 1 字节 → dict{data=b"\xff", value=255,
        //        offset1=0, offset2=1, length=1}
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let node = RawCopyNode::new(inner);

            let mut stream = ParseStream::new(&[0xFF]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let dict = result.bind(py).downcast::<PyDict>().expect("is dict");
            assert_eq!(dict.len(), 5);

            let data_binding = dict.get_item("data").unwrap().expect("data key");
            let data: &[u8] = data_binding.extract().unwrap();
            assert_eq!(data, &[0xFF]);

            let value: i64 = dict
                .get_item("value")
                .unwrap()
                .expect("value key")
                .extract()
                .unwrap();
            assert_eq!(value, 255);

            let offset1: i64 = dict
                .get_item("offset1")
                .unwrap()
                .expect("offset1 key")
                .extract()
                .unwrap();
            assert_eq!(offset1, 0);

            let offset2: i64 = dict
                .get_item("offset2")
                .unwrap()
                .expect("offset2 key")
                .extract()
                .unwrap();
            assert_eq!(offset2, 1);

            let length: i64 = dict
                .get_item("length")
                .unwrap()
                .expect("length key")
                .extract()
                .unwrap();
            assert_eq!(length, 1);
        });
    }

    #[test]
    fn parse_int32ub_captures_four_bytes() {
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt32Big));
            let node = RawCopyNode::new(inner);

            let mut stream = ParseStream::new(&[0x01, 0x02, 0x03, 0x04]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let dict = result.bind(py).downcast::<PyDict>().expect("is dict");

            let data_binding = dict.get_item("data").unwrap().expect("data key");
            let data: &[u8] = data_binding.extract().unwrap();
            assert_eq!(data, &[0x01, 0x02, 0x03, 0x04]);

            let value: i64 = dict
                .get_item("value")
                .unwrap()
                .expect("value key")
                .extract()
                .unwrap();
            assert_eq!(value, 0x01020304);

            let length: i64 = dict
                .get_item("length")
                .unwrap()
                .expect("length key")
                .extract()
                .unwrap();
            assert_eq!(length, 4);
        });
    }

    #[test]
    fn parse_advances_stream_to_end_of_inner() {
        // parse 完成后 stream.tell() 应为 offset2（消费了 length 字节）
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
            let node = RawCopyNode::new(inner);

            let mut stream = ParseStream::new(&[0xAA, 0xBB, 0xCC]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let _ = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // 内部：seek 回 offset1 重读 length=2，最终 pos = offset1 + length = 2
            assert_eq!(stream.tell(), 2);
            assert_eq!(stream.remaining(), 1);
        });
    }

    // ======================================================================
    // build
    // ======================================================================

    #[test]
    fn build_from_data_writes_directly() {
        // RawCopy.build({"data": b"\xff"}) → 直接 write，不调 inner.build
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let node = RawCopyNode::new(inner);

            let dict = PyDict::new_bound(py);
            dict.set_item("data", PyBytes::new_bound(py, b"\xff"))
                .unwrap();
            let obj = dict.into_any();
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0xFF]);
        });
    }

    #[test]
    fn build_from_value_calls_inner_build() {
        // RawCopy.build({"value": 255}) → inner.build(255) → 写出 b"\xff"
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let node = RawCopyNode::new(inner);

            let dict = PyDict::new_bound(py);
            dict.set_item("value", 255).unwrap();
            let obj = dict.into_any();
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0xFF]);
        });
    }

    #[test]
    fn build_data_takes_precedence_over_value() {
        // 同时存在 data 与 value 时，data 优先（对齐 Python RawCopy._build L4797）
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let node = RawCopyNode::new(inner);

            let dict = PyDict::new_bound(py);
            dict.set_item("data", PyBytes::new_bound(py, b"\xAA"))
                .unwrap();
            dict.set_item("value", 0xBB).unwrap();
            let obj = dict.into_any();
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            // data 优先 → 写 0xAA
            assert_eq!(stream.as_bytes(), &[0xAA]);
        });
    }

    #[test]
    fn build_missing_both_keys_returns_error() {
        // 无 data 无 value → Generic 错误
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let node = RawCopyNode::new(inner);

            let dict = PyDict::new_bound(py);
            dict.set_item("unknown", 1).unwrap();
            let obj = dict.into_any();
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Generic { message, .. } => {
                    assert!(
                        message.contains("data") && message.contains("value"),
                        "got: {}",
                        message
                    );
                }
                other => panic!("expected Generic, got {:?}", other),
            }
        });
    }

    #[test]
    fn build_empty_dict_returns_error() {
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let node = RawCopyNode::new(inner);

            let dict = PyDict::new_bound(py);
            let obj = dict.into_any();
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Generic { .. }));
        });
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_returns_inner_size() {
        with_py(|py| {
            let ctx = Context::placeholder(py);
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
            let node = RawCopyNode::new(inner);
            assert_eq!(node.sizeof(&ctx).unwrap(), 2);
        });
    }

    // ======================================================================
    // parse ↔ build 往返
    // ======================================================================

    #[test]
    fn round_trip_through_data_key() {
        // parse → 拿到 dict["data"] → 再 build({"data": ...}) → 还原字节
        with_py(|py| {
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let node = RawCopyNode::new(inner);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            // parse
            let mut pstream = ParseStream::new(&[0x42]);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let parsed = result.bind(py);
            let data_obj = parsed.get_item("data").expect("get_item data");

            // build with data
            let dict = PyDict::new_bound(py);
            dict.set_item("data", &data_obj).unwrap();
            let obj = dict.into_any();
            let mut bstream = BuildStream::new();
            node.build(py, &obj, &mut bstream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(bstream.as_bytes(), &[0x42]);
        });
    }

    // ======================================================================
    // Debug
    // ======================================================================

    #[test]
    fn debug_format_includes_raw_copy_node() {
        let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
        let node = RawCopyNode::new(inner);
        let s = format!("{:?}", node);
        assert!(s.contains("RawCopyNode"), "got: {}", s);
    }
}
