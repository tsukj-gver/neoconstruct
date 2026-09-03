//! EnumNode：枚举映射节点。
//!
//! Python 参考：`construct/construct/core.py` `Enum`（L1920-2005）。
//!
//! ## 行为概述
//!
//! `Enum(subcon, *merge, **mapping)` 把 subcon 返回的整数映射为 label 字符串
//! （int-convertible）。在 neoconstruct 中实现为 Rust Node（非 AdapterCallbackNode）。
//!
//! - parse：inner.parse → decmapping 查 label；命中返回 label（EnumIntegerString），
//!   未命中返回 EnumInteger(obj)（int 子类，**不报错**，对齐 Python L1982）
//! - build：obj 是 int → 直接用；否则查 encmapping；命中 inner.build(raw)，未命中 MappingError
//! - sizeof：转发 inner.sizeof
//!
//! ## 编译期物化
//!
//! `decmapping` / `encmapping` 在编译期由 Python 描述符构造为普通 Python dict，
//! 再由 compile.rs 物化为 `Py<PyDict>` 引用存入 Node。运行时通过 `PyDict_GetItem`
//! 查询（C API，不计额外 FFI）。
//!
//! ## EnumInteger / EnumIntegerString
//!
//! Python 内部类（`neoconstruct._internals`）：`EnumInteger(int)` 是 int 子类（无映射 fallback），
//! `EnumIntegerString(str)` 是 str 子类（带 `.intvalue`）。编译期从 Python 加载
//! `EnumInteger` 类物化为 `Py<PyType>`，运行时通过 `cls.call1((obj,))` 构造。

use crate::context::Context;
use crate::error::ConstructError;
use crate::nodes::Node;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyLong, PyType};

use super::Construct;

/// 枚举映射节点：subcon 整数 ↔ label 字符串。
///
/// 对应 Python construct `Enum(subcon, *merge, **mapping)`（core.py L1920）。
/// 详见模块级注释。
#[derive(Debug)]
pub struct EnumNode {
    /// 被包装的子树根（通常是 Int* 整数节点）。
    inner: Box<Node>,
    /// 解码映射：int（raw value）→ EnumIntegerString（label）。编译期物化。
    decmapping: Py<PyDict>,
    /// 编码映射：EnumIntegerString/str（label）→ int（raw value）。编译期物化。
    encmapping: Py<PyDict>,
    /// `EnumInteger` 类引用（用于无映射 fallback，构造 int 子类实例）。
    /// 从 `neoconstruct._internals` 加载（neoconstruct port）。
    enum_integer_cls: Py<PyType>,
}

impl EnumNode {
    /// 创建 `EnumNode`。
    pub fn new(
        inner: Node,
        decmapping: Py<PyDict>,
        encmapping: Py<PyDict>,
        enum_integer_cls: Py<PyType>,
    ) -> Self {
        Self {
            inner: Box::new(inner),
            decmapping,
            encmapping,
            enum_integer_cls,
        }
    }

    /// 返回内部子树根节点的引用。
    pub fn inner(&self) -> &Node {
        &self.inner
    }

    /// 返回解码映射 dict 引用。
    pub fn decmapping(&self) -> &Py<PyDict> {
        &self.decmapping
    }

    /// 返回编码映射 dict 引用。
    pub fn encmapping(&self) -> &Py<PyDict> {
        &self.encmapping
    }
}

impl Construct for EnumNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let obj = self.inner.parse(py, stream, ctx, path)?;
        let obj_bound = obj.bind(py);
        let label_opt = self
            .decmapping
            .bind(py)
            .get_item(obj_bound)
            .map_err(ConstructError::from)?;
        match label_opt {
            Some(label) => Ok(label.unbind()),
            None => {
                // 无映射 fallback：构造 EnumInteger(obj)（int 子类，C API 类型构造）。
                let fallback = self
                    .enum_integer_cls
                    .bind(py)
                    .call1((obj_bound,))
                    .map_err(|e| ConstructError::Generic {
                        message: format!("EnumInteger() construction failed: {}", e),
                        path: path.to_string(),
                    })?;
                Ok(fallback.unbind())
            }
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
        // 边界：Python bool 是 int 子类（True == 1, False == 0）。
        // is_instance_of::<PyLong> 对 bool 也返回 true，因此 bool 会走 int 直接用路径，
        // 与 Python construct L1986 `isinstance(obj, int): return obj` 一致。
        let raw: Py<PyAny> = if obj.is_instance_of::<PyLong>() {
            obj.clone().unbind()
        } else {
            // label → raw value（encmapping 查询）。
            match self
                .encmapping
                .bind(py)
                .get_item(obj)
                .map_err(ConstructError::from)?
            {
                Some(v) => v.unbind(),
                None => {
                    return Err(ConstructError::Mapping {
                        message: format!(
                            "building failed, no mapping for {}",
                            obj.repr().map(|r| r.to_string()).unwrap_or_default()
                        ),
                        path: path.to_string(),
                    })
                }
            }
        };
        self.inner.build(py, raw.bind(py), stream, ctx, path)
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
    use crate::nodes::Construct;

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

    /// 构造 EnumNode 测试 fixture：inner=Int8ub，mapping={1:"one", 2:"two"}。
    /// 注：decmapping/encmapping 是手工构造的 Python dict；EnumInteger 类用 Python type() 创建。
    fn make_enum_node(py: Python<'_>) -> Option<EnumNode> {
        // 加载 EnumInteger 类（从 neoconstruct._internals，若可用）。
        let enum_integer_cls = py
            .import_bound("neoconstruct._internals")
            .and_then(|m| m.getattr("EnumInteger"))
            .and_then(|a| a.extract::<Py<PyType>>())
            .ok()?;
        let decmapping = PyDict::new_bound(py);
        decmapping.set_item(1, "one").unwrap();
        decmapping.set_item(2, "two").unwrap();
        let encmapping = PyDict::new_bound(py);
        encmapping.set_item("one", 1).unwrap();
        encmapping.set_item("two", 2).unwrap();
        let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
        Some(EnumNode::new(
            inner,
            decmapping.unbind(),
            encmapping.unbind(),
            enum_integer_cls,
        ))
    }

    #[test]
    fn parse_mapped_returns_label() {
        // EN-1: Enum(Byte, one=1, two=2).parse(b'\x01') → 'one'
        with_py(|py| {
            let node = match make_enum_node(py) {
                Some(n) => n,
                None => {
                    eprintln!("skipping: neoconstruct._internals.EnumInteger not available");
                    return;
                }
            };
            let mut stream = ParseStream::new(&[0x01]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let s: String = result.bind(py).extract().expect("str");
            assert_eq!(s, "one");
        });
    }

    #[test]
    fn parse_unmapped_returns_enuminteger() {
        // EN-2: Enum(Byte, one=1).parse(b'\xff') → EnumInteger(255)
        with_py(|py| {
            let node = match make_enum_node(py) {
                Some(n) => n,
                None => return,
            };
            let mut stream = ParseStream::new(&[0xff]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let v: i64 = result.bind(py).extract().expect("i64");
            assert_eq!(v, 255);
        });
    }

    #[test]
    fn build_label_uses_encmapping() {
        // EN-3: Enum(Byte, one=1).build('one') → b'\x01'
        with_py(|py| {
            let node = match make_enum_node(py) {
                Some(n) => n,
                None => return,
            };
            let obj = py.eval_bound("'one'", None, None).expect("label");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x01]);
        });
    }

    #[test]
    fn build_int_passthrough() {
        // EN-4/EN-6: Enum(Byte, one=1).build(99) → b'\x63'（int 直接用，即使 99 不在 mapping）
        with_py(|py| {
            let node = match make_enum_node(py) {
                Some(n) => n,
                None => return,
            };
            let obj = py.eval_bound("99", None, None).expect("int");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x63]);
        });
    }

    #[test]
    fn build_unknown_label_raises_mapping_error() {
        // EN-5: Enum(Byte, one=1).build('unknown') → MappingError
        with_py(|py| {
            let node = match make_enum_node(py) {
                Some(n) => n,
                None => return,
            };
            let obj = py.eval_bound("'unknown'", None, None).expect("label");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Mapping { .. }));
        });
    }

    #[test]
    fn sizeof_forwards_to_inner() {
        // EN-7: Enum(Byte, one=1).sizeof() → 1
        with_py(|py| {
            let node = match make_enum_node(py) {
                Some(n) => n,
                None => return,
            };
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).unwrap(), 1);
        });
    }

    #[test]
    fn debug_format_includes_enum_node() {
        with_py(|py| {
            let node = match make_enum_node(py) {
                Some(n) => n,
                None => return,
            };
            let s = format!("{:?}", node);
            assert!(s.contains("EnumNode"), "got: {}", s);
        });
    }

    // 确保未使用 BytesNode import 警告（_x 表示"使用"该 import）。
    #[test]
    fn _ensure_bytes_node_import_used() {
        let _ = Node::Bytes(BytesNode::new_const(0));
    }
}
