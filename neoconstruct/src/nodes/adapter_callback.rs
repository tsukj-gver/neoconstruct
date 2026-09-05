//! AdapterCallbackNode：用户面 Adapter 嵌入 Struct 字段时的最小钩子。
//!
//! Python 参考：`construct/construct/core.py` `Adapter`（L813-834）。
//!
//! ## 角色定位
//!
//! AdapterCallbackNode 是"最小通用钩子"。
//! 仅用于用户面 Adapter / SymmetricAdapter 嵌入 Struct 字段的场景。
//!
//! 用户单独使用 Adapter（如 `HexAdapter(Int8ub).parse(b)`）不经过此节点——
//! 直接走 Python 层 Adapter.parse（adapter.py 实现）。
//!
//! ## FFI 边界
//!
//! 嵌入 Struct 时，subcon 部分在 Rust 内执行（零 FFI），
//! 但 `_decode` / `_encode` 是用户 Python 方法，需 Rust→Python 回调（1 次额外 FFI）。
//! 总计 2 次 FFI 穿越（parse 入口 + _decode 回调）。
//!
//! 用户主动继承 Adapter = 显式接受此性能折衷。
//! 用户面 Adapter 不设硬性能门禁。
//!
//! ## 架构原则合规性
//!
//! AdapterCallbackNode 在执行树内（Node enum 变体），2 次 FFI 是用户主动选择的
//! 后处理开销，不属于"中间表示层"（中间表示层指 Rust 端临时数据类型
//! 再转换，非用户 Python 回调）。

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;
use pyo3::types::PyTuple;

use super::Construct;
use crate::nodes::Node;

/// Adapter 回调节点：嵌入 Struct 字段时，subcon 在 Rust 执行，adapter 的
/// `_decode` / `_encode` 通过 `Py<PyAny>` 引用在 Rust→Python 回调中执行。
///
/// **仅用于 Adapter/SymmetricAdapter 嵌入 Struct 场景**。用户单独使用 Adapter
/// （如 `HexAdapter(Int8ub).parse(b)`）不经过此节点（直接走 Python 层
/// `Adapter.parse`）。
///
/// 此节点是"最小通用钩子"——它不引入"通用用户回调"机制，
/// 仅在 Struct 字段编译期把"Adapter 包装"翻译为"subcon Rust Node + 回调引用"。
///
/// # 三方法行为
///
/// - parse：`subcon.parse(...)` → Rust→Python 回调 `_decode(obj, ctx, path)`
/// - build：Rust→Python 回调 `_encode(obj, ctx, path)` → `subcon.build(result)`
/// - sizeof：`subcon.sizeof(ctx)`
#[derive(Debug)]
pub struct AdapterCallbackNode {
    /// Rust 端执行的 subcon。
    subcon: Box<Node>,
    /// 用户 `_decode` 方法引用（bound method，含 self）。
    decode: Py<PyAny>,
    /// 用户 `_encode` 方法引用（bound method，含 self）。
    encode: Py<PyAny>,
}

impl AdapterCallbackNode {
    /// 创建 `AdapterCallbackNode`，包裹 Rust subcon + 用户 _decode/_encode 方法引用。
    ///
    /// # 参数
    ///
    /// - `subcon`：Rust 端执行的子树（由 AdapterDescriptor.subcon 递归编译得到）。
    /// - `decode`：用户 Adapter 子类的 `_decode` bound method（含 self）。
    /// - `encode`：用户 Adapter 子类的 `_encode` bound method（含 self）。
    pub fn new(subcon: Node, decode: Py<PyAny>, encode: Py<PyAny>) -> Self {
        Self {
            subcon: Box::new(subcon),
            decode,
            encode,
        }
    }

    /// 返回内部 subcon 节点的引用。
    pub fn subcon(&self) -> &Node {
        &self.subcon
    }

    /// 返回 `_decode` 方法引用。
    pub fn decode(&self) -> &Py<PyAny> {
        &self.decode
    }

    /// 返回 `_encode` 方法引用。
    pub fn encode(&self) -> &Py<PyAny> {
        &self.encode
    }
}

impl Construct for AdapterCallbackNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // parse：subcon.parse → 回调 _decode(obj, ctx, path)
        let inner_value = self.subcon.parse(py, stream, ctx, path)?;

        // 构造 _decode 调用参数：(obj, context, path)
        // path 用字符串表示（对齐 Python Adapter._decode(obj, context, path) 签名）。
        // context 用 ctx.fields()（若存在）或 Py_None（占位 ctx / 无表达式 Struct）。
        // 大多数 Adapter._decode 仅用 obj，context 占位即可。
        let path_str = path.to_string();
        let path_py = pyo3::types::PyString::new_bound(py, &path_str);
        let ctx_view: Py<PyAny> = match ctx.fields() {
            Some(dict) => dict.clone().into_any().unbind(),
            None => py.None(),
        };
        let args = PyTuple::new_bound(py, [inner_value.bind(py), ctx_view.bind(py), &path_py]);

        // Rust→Python 回调：_decode 是 bound method，含 self（adapter 实例）。
        // 用户语义异常（ExplicitError / CancelParsing）先分类保真（透传/顶层
        // 捕获），其余异常包装为 Generic（携带回调名与 path 上下文）。
        let result = self.decode.call_bound(py, args, None).map_err(|e| {
            crate::error::classify_user_pyerr(&e, py).unwrap_or(ConstructError::Generic {
                message: format!(
                    "Adapter _decode callback failed: {} (path: {})",
                    e, path_str
                ),
                path: path_str,
            })
        })?;
        Ok(result)
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        // build：回调 _encode(obj, ctx, path) → subcon.build(result)
        let path_str = path.to_string();
        let path_py = pyo3::types::PyString::new_bound(py, &path_str);
        let ctx_view: Py<PyAny> = match ctx.fields() {
            Some(dict) => dict.clone().into_any().unbind(),
            None => py.None(),
        };
        let args = PyTuple::new_bound(py, [obj, ctx_view.bind(py), &path_py]);

        let encoded = self.encode.call_bound(py, args, None).map_err(|e| {
            // 用户语义异常先分类保真（与 _decode 回调同判据），其余包装 Generic。
            crate::error::classify_user_pyerr(&e, py).unwrap_or(ConstructError::Generic {
                message: format!(
                    "Adapter _encode callback failed: {} (path: {})",
                    e, path_str
                ),
                path: path_str,
            })
        })?;

        // 把 _encode 结果交给 subcon.build（encoded 是 owned Py<PyAny>，需 bind）
        self.subcon.build(py, encoded.bind(py), stream, ctx, path)
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        self.subcon.sizeof(ctx)
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
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

    /// 定义一个简单的 Python Adapter 类（hex 编码/解码），
    /// 返回 (cls, decode_bound, encode_bound) 三元组。
    fn make_hex_adapter<'py>(py: Python<'py>) -> (Py<PyAny>, Bound<'py, PyAny>, Bound<'py, PyAny>) {
        let code = r#"
class HexAdapter:
    def _decode(self, obj, context, path):
        return hex(obj)
    def _encode(self, obj, context, path):
        return int(obj, 16)
"#;
        let globals = pyo3::types::PyDict::new_bound(py);
        py.run_bound(code, Some(&globals), None).expect("def");
        let adapter_cls = globals.get_item("HexAdapter").unwrap().unwrap();
        let instance = adapter_cls.call((), None).expect("instantiate");
        let decode = instance.getattr("_decode").expect("getattr _decode");
        let encode = instance.getattr("_encode").expect("getattr _encode");
        (instance.into(), decode, encode)
    }

    // ======================================================================
    // 构造器
    // ======================================================================

    #[test]
    fn new_stores_subcon_and_callbacks() {
        with_py(|py| {
            let subcon = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let (_, decode, encode) = make_hex_adapter(py);
            let decode_ptr = decode.as_ptr();
            let encode_ptr = encode.as_ptr();
            let node = AdapterCallbackNode::new(subcon, decode.into(), encode.into());
            assert!(matches!(node.subcon(), Node::FormatField(_)));
            // 引用非空（指针比较：存入引用应等于原始 bound method 的指针）
            assert_eq!(node.decode().as_ptr(), decode_ptr);
            assert_eq!(node.encode().as_ptr(), encode_ptr);
        });
    }

    // ======================================================================
    // parse — subcon Rust parse → 回调 _decode
    // ======================================================================

    #[test]
    fn parse_calls_decode_after_inner_parse() {
        // field(HexAdapter(Int8ub)) parse b"\x10" → Int8ub.parse → 16
        //        → _decode(16) → "0x10"
        with_py(|py| {
            let subcon = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let (_, decode, encode) = make_hex_adapter(py);
            let node = AdapterCallbackNode::new(subcon, decode.into(), encode.into());

            let mut stream = ParseStream::new(&[0x10]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let s: String = result.bind(py).extract().unwrap();
            assert_eq!(s, "0x10");
        });
    }

    // ======================================================================
    // build — 回调 _encode → subcon.build
    // ======================================================================

    #[test]
    fn build_calls_encode_before_inner_build() {
        // field(HexAdapter(Int8ub)) build "0xff" → _encode("0xff") → 255
        //        → Int8ub.build(255) → b"\xff"
        with_py(|py| {
            let subcon = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let (_, decode, encode) = make_hex_adapter(py);
            let node = AdapterCallbackNode::new(subcon, decode.into(), encode.into());

            let obj = py.eval_bound("'0xff'", None, None).expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0xFF]);
        });
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_returns_subcon_size() {
        with_py(|py| {
            let ctx = Context::placeholder(py);
            let subcon = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big));
            let (_, decode, encode) = make_hex_adapter(py);
            let node = AdapterCallbackNode::new(subcon, decode.into(), encode.into());
            assert_eq!(node.sizeof(&ctx).unwrap(), 2);
        });
    }

    // ======================================================================
    // parse ↔ build 往返
    // ======================================================================

    #[test]
    fn round_trip_parse_build_consistency() {
        with_py(|py| {
            let subcon = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let (_, decode, encode) = make_hex_adapter(py);
            let node = AdapterCallbackNode::new(subcon, decode.into(), encode.into());
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();

            // build "0xab" → b"\xab"
            let obj = py.eval_bound("'0xab'", None, None).expect("obj");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();
            assert_eq!(bytes, vec![0xAB]);

            // parse back → "0xab"
            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let s: String = result.bind(py).extract().unwrap();
            assert_eq!(s, "0xab");
        });
    }

    // ======================================================================
    // 错误路径 — _decode/_encode 抛 Python 异常 → ConstructError::Generic
    // ======================================================================

    #[test]
    fn parse_decode_raises_python_exception_returns_generic_error() {
        // _decode 抛 Python 异常 → 跨 FFI 转 ConstructError::Generic
        with_py(|py| {
            let code = r#"
class FailingAdapter:
    def _decode(self, obj, context, path):
        raise ValueError("decode failed")
    def _encode(self, obj, context, path):
        return obj
"#;
            let globals = pyo3::types::PyDict::new_bound(py);
            py.run_bound(code, Some(&globals), None).expect("def");
            let adapter = globals
                .get_item("FailingAdapter")
                .unwrap()
                .unwrap()
                .call((), None)
                .expect("instantiate");
            let decode = adapter.getattr("_decode").unwrap();
            let encode = adapter.getattr("_encode").unwrap();

            let subcon = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let node = AdapterCallbackNode::new(subcon, decode.into(), encode.into());

            let mut stream = ParseStream::new(&[0x10]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Generic { message, .. } => {
                    assert!(
                        message.contains("_decode") || message.contains("decode failed"),
                        "got: {}",
                        message
                    );
                }
                other => panic!("expected Generic, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // Debug
    // ======================================================================

    #[test]
    fn debug_format_includes_adapter_callback_node() {
        with_py(|py| {
            let subcon = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
            let (_, decode, encode) = make_hex_adapter(py);
            let node = AdapterCallbackNode::new(subcon, decode.into(), encode.into());
            let s = format!("{:?}", node);
            assert!(s.contains("AdapterCallbackNode"), "got: {}", s);
        });
    }
}
