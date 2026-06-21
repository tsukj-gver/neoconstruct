//! StructRefNode：嵌套引用节点。
//!
//! 设计依据：`docs/架构设计.md` §C.3.5、§B.7。
//!
//! ## 用途
//!
//! 引用另一个 `StructMixin` 子类的执行树，用于嵌套结构（`Outer.inner: Inner`）。
//! 存储类引用（`Py<PyType>`），运行时通过 `cls._construct_compiled` 动态获取对方的
//! [`CompiledSchema`](crate::schema::CompiledSchema) —— 这保证互相引用（NodeA↔NodeB）可行。
//!
//! ## resolve_schema 机制（关键设计）
//!
//! StructRef 存**类引用**（`Py<PyType>`）而非编译产物引用。原因：
//! - **互相引用场景**（NodeA.peer: NodeB，NodeB.peer: NodeA）：NodeA 编译时 NodeB
//!   可能尚未完成编译。若存编译产物引用，NodeA 编译时无法获取 NodeB 的产物。
//!   存类引用，NodeA 编译成功（只持有 NodeB 的类对象）。
//! - **运行时解析**：首次 `NodeA.parse(data)` 时，NodeB 已完成编译，
//!   [`StructRefNode::resolve_schema`] 通过 `cls._construct_compiled` 获取 NodeB 产物。
//! - **缓存**：`OnceLock` 首次解析后缓存，后续调用零查找开销。
//!
//! ## parse 嵌套实例化（§B.7）
//!
//! Rust 内部递归遍历对方执行树构造子 `PyDict`，随后调用对方类构造实例
//! （Rust→Python 回调，归类为 FFI 设计 §4.4 的用户钩子例外）：
//!
//! ```text
//! let sub_dict = schema.root().parse(...)?;   // 递归遍历对方执行树
//! let instance = cls.call((), Some(&sub_dict))?;  // Rust→Python 回调
//! ```

use crate::context::Context;
use crate::error::ConstructError;
use crate::path::Path;
use crate::schema::CompiledSchema;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyType};
use std::sync::OnceLock;

use super::Construct;

/// 引用另一个 `StructMixin` 子类的执行树，用于嵌套结构。
///
/// 存储类引用（`Py<PyType>`），运行时通过 `cls._construct_compiled` 动态获取对方的
/// [`CompiledSchema`]。首次解析后缓存到 `schema_cache`，后续调用零查找开销。
///
/// # parse 行为（§B.7 嵌套实例化）
///
/// 1. [`StructRefNode::resolve_schema`] 获取对方执行树。
/// 2. 递归遍历对方 `root.parse(...)`，获得子 `PyDict`。
/// 3. 调用对方类构造实例（`cls(**dict)`），返回实例。
///
/// 步骤 3 是 Rust→Python 回调（归类为 FFI 设计 §4.4 的用户钩子例外）。
///
/// # build 行为
///
/// 1. [`StructRefNode::resolve_schema`] 获取对方执行树。
/// 2. 递归遍历对方 `root.build(obj, ...)`，obj 已是对方类实例。
///
/// build 全程无 Python 回调，严格一次 FFI。
///
/// # sizeof
///
/// `[设计质疑]` 当前 [`Construct`] trait 的 sizeof 签名不含 `Python` 参数，
/// 无法在 sizeof 中获取 GIL 来解析引用。StructRefNode 的 sizeof 返回 Err。
/// 若需支持嵌套 sizeof，需在后续子任务修改 trait 签名（给 sizeof 加 py 参数）。
pub struct StructRefNode {
    /// 对方类的 Python 引用（`StructMixin` 子类）。
    cls: Py<PyType>,
    /// 缓存对方 [`CompiledSchema`]（首次 parse/build 时填充）。
    ///
    /// `OnceLock` 提供线程安全的"一次写入"语义，允许多线程并发读取缓存值。
    /// 初始为空，首次 `resolve_schema` 后填充。
    schema_cache: OnceLock<Py<CompiledSchema>>,
}

impl StructRefNode {
    /// 创建一个指向 `cls` 的 `StructRefNode`。
    ///
    /// `cls` 应为 `StructMixin` 子类（运行时通过 `_construct_compiled` 属性访问其
    /// [`CompiledSchema`]）。
    pub fn new(cls: Py<PyType>) -> Self {
        Self {
            cls,
            schema_cache: OnceLock::new(),
        }
    }

    /// 返回对方类引用。
    pub fn cls(&self) -> &Py<PyType> {
        &self.cls
    }

    /// 运行时动态解析对方的 [`CompiledSchema`]。
    ///
    /// 优先从 `schema_cache` 读取（命中则零开销返回克隆）；缓存未命中时通过
    /// `cls._construct_compiled` 类属性获取并填充缓存。
    ///
    /// # 错误
    ///
    /// - [`ConstructError::UnresolvedReference`]：对方类未编译
    ///   （`_construct_compiled` 为 `None` 或属性不存在或类型错误）。
    fn resolve_schema<'py>(&self, py: Python<'py>) -> Result<Py<CompiledSchema>, ConstructError> {
        // 优先从缓存读取（命中则零查找开销）
        if let Some(schema) = self.schema_cache.get() {
            return Ok(schema.clone_ref(py));
        }

        // 缓存未命中：通过 C API 读取 cls._construct_compiled
        let cls_bound = self.cls.bind(py);
        let cls_name = cls_bound
            .name()
            .map(|n| n.to_string())
            .unwrap_or_else(|_| "<unknown>".to_string());

        let compiled = cls_bound.getattr("_construct_compiled").map_err(|_| {
            ConstructError::UnresolvedReference {
                message: format!(
                    "class '{}' has no '_construct_compiled' attribute \
                     (not a StructMixin subclass?)",
                    cls_name
                ),
            }
        })?;

        // compiled 为 None → 对方仍是延迟桩（前向引用未解析）
        if compiled.is_none() {
            return Err(ConstructError::UnresolvedReference {
                message: format!(
                    "class '{}' is still a lazy stub, forward reference unresolved",
                    cls_name
                ),
            });
        }

        let schema: Py<CompiledSchema> =
            compiled
                .extract()
                .map_err(|_| ConstructError::UnresolvedReference {
                    message: format!(
                        "'_construct_compiled' of class '{}' is not a CompiledSchema",
                        cls_name
                    ),
                })?;

        // 填充缓存（OnceLock::set 失败说明并发已被填充，无碍）
        let _ = self.schema_cache.set(schema.clone_ref(py));
        Ok(schema)
    }
}

impl Construct for StructRefNode {
    fn parse(
        &self,
        py: Python<'_>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let schema = self.resolve_schema(py)?;
        let schema_bound = schema.bind(py);
        let root = schema_bound.get().root();

        // 递归遍历对方执行树，获得子 PyDict
        let sub_dict = root.parse(py, stream, ctx, path)?;

        // 调用对方类构造实例（Rust→Python 回调，归类为用户钩子）
        // cls(**sub_dict)
        let dict_bound =
            sub_dict
                .bind(py)
                .downcast::<PyDict>()
                .map_err(|_| ConstructError::Generic {
                    message: "StructRef parse: inner parse did not return a dict".to_string(),
                    path: path.to_string(),
                })?;
        let instance =
            self.cls
                .bind(py)
                .call((), Some(dict_bound))
                .map_err(|e| ConstructError::Generic {
                    message: format!("failed to instantiate nested class: {}", e),
                    path: path.to_string(),
                })?;
        Ok(instance.unbind())
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        let schema = self.resolve_schema(py)?;
        let schema_bound = schema.bind(py);
        let root = schema_bound.get().root();

        // obj 已是对方类实例，直接传给对方执行树
        root.build(py, obj, stream, ctx, path)
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // [设计质疑] sizeof 签名无 py 参数，无法解析引用获取对方 schema。
        // 对应 Python construct 的 SizeofError（嵌套引用大小未知）。
        Err(ConstructError::Generic {
            message: "StructRefNode size cannot be determined without resolving the reference"
                .to_string(),
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
    use crate::nodes::format_field::{FormatFieldNode, PythonFormat};
    use crate::nodes::struct_node::StructNode;
    use crate::nodes::{Construct, Node};
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

    /// 定义一个简单的 Python 类，用于测试嵌套引用。
    ///
    /// 返回类的 `Py<PyType>`。类有 `__init__(self, x=0, y=0)` 和 `__eq__`。
    fn define_test_class(py: Python<'_>, name: &str, fields: &[&str]) -> Py<PyType> {
        let fields_init = fields
            .iter()
            .map(|f| format!("{}=0", f))
            .collect::<Vec<_>>()
            .join(", ");
        let fields_assign = fields
            .iter()
            .map(|f| format!("self.{} = {}", f, f))
            .collect::<Vec<_>>()
            .join("\n        ");
        let fields_eq = fields
            .iter()
            .map(|f| format!("getattr(other, '{}', None) == self.{}", f, f))
            .collect::<Vec<_>>()
            .join(" and ");

        let code = format!(
            r#"
class {name}:
    def __init__(self, {fields_init}):
        {fields_assign}
    def __repr__(self):
        return '{name}(' + ', '.join(f'{{k}}={{v}}' for k, v in vars(self).items()) + ')'
    def __eq__(self, other):
        if not isinstance(other, {name}):
            return False
        return {fields_eq}
"#,
            name = name,
            fields_init = fields_init,
            fields_assign = fields_assign,
            fields_eq = fields_eq,
        );

        let globals = PyDict::new_bound(py);
        py.run_bound(&code, Some(&globals), None)
            .unwrap_or_else(|e| panic!("failed to define class: {}", e));
        globals
            .get_item(name)
            .expect("get_item ok")
            .unwrap_or_else(|| panic!("class {} not found after definition", name))
            .extract::<Py<PyType>>()
            .expect("extract Py<PyType>")
    }

    /// 构造一个单字段（Int8ub）的 Inner CompiledSchema 并安装到类上。
    fn install_schema_for_class(
        py: Python<'_>,
        cls: &Py<PyType>,
        fields: Vec<(String, Node)>,
    ) -> Py<CompiledSchema> {
        let root = Node::Struct(StructNode::new(fields));
        let schema = CompiledSchema::new(root, cls.clone_ref(py));
        let schema_py = Py::new(py, schema).expect("Py::new schema");
        cls.bind(py)
            .setattr("_construct_compiled", schema_py.clone_ref(py))
            .expect("setattr _construct_compiled");
        schema_py
    }

    /// 构造一个 Int8ub 节点。
    fn u8_node() -> Node {
        Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big))
    }

    // ======================================================================
    // 基本 parse/build
    // ======================================================================

    #[test]
    fn parse_returns_instance_not_dict() {
        // 关键验证：parse 返回对方类的实例（通过 Rust→Python 回调构造），
        // 而非裸 dict。
        with_py(|py| {
            let inner_cls = define_test_class(py, "Inner", &["x"]);
            install_schema_for_class(py, &inner_cls, vec![("x".to_string(), u8_node())]);

            let struct_ref = StructRefNode::new(inner_cls.clone_ref(py));
            let mut stream = ParseStream::new(&[0x05]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            let result = struct_ref
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");

            // 验证是 Inner 实例
            let result_bound = result.bind(py);
            assert!(
                result_bound
                    .is_instance(inner_cls.bind(py))
                    .expect("is_instance"),
                "result should be an Inner instance"
            );
            // 验证字段值
            let x: i64 = result_bound
                .getattr("x")
                .expect("getattr x")
                .extract()
                .expect("extract x");
            assert_eq!(x, 5);
        });
    }

    #[test]
    fn build_writes_from_instance_attributes() {
        with_py(|py| {
            let inner_cls = define_test_class(py, "Inner", &["x"]);
            install_schema_for_class(py, &inner_cls, vec![("x".to_string(), u8_node())]);

            let struct_ref = StructRefNode::new(inner_cls.clone_ref(py));

            // 构造 Inner 实例（x=200）
            let instance = inner_cls.bind(py).call((), None).expect("call default");
            instance.setattr("x", 200).expect("setattr x");

            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            struct_ref
                .build(py, &instance, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[200]);
        });
    }

    #[test]
    fn round_trip_preserves_instance() {
        with_py(|py| {
            let inner_cls = define_test_class(py, "Inner", &["x"]);
            install_schema_for_class(py, &inner_cls, vec![("x".to_string(), u8_node())]);

            let struct_ref = StructRefNode::new(inner_cls.clone_ref(py));
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            // build 实例 x=42
            let instance = inner_cls.bind(py).call((), None).expect("call");
            instance.setattr("x", 42).expect("set x");
            let mut stream = BuildStream::new();
            struct_ref
                .build(py, &instance, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();
            assert_eq!(bytes, &[42]);

            // parse 回来
            let mut pstream = ParseStream::new(&bytes);
            let result = struct_ref
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            // 验证实例相等
            let result_eq = instance.eq(result.bind(py)).expect("eq");
            assert!(result_eq, "parsed instance should equal original");
        });
    }

    // ======================================================================
    // 多字段嵌套引用
    // ======================================================================

    #[test]
    fn multi_field_nested_parse_and_build() {
        with_py(|py| {
            let point_cls = define_test_class(py, "Point", &["x", "y"]);
            install_schema_for_class(
                py,
                &point_cls,
                vec![("x".to_string(), u8_node()), ("y".to_string(), u8_node())],
            );

            let struct_ref = StructRefNode::new(point_cls.clone_ref(py));

            // parse b'\x01\x02'
            let mut stream = ParseStream::new(&[0x01, 0x02]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = struct_ref
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let result_bound = result.bind(py);
            assert!(
                result_bound
                    .is_instance(point_cls.bind(py))
                    .expect("is_instance"),
                "should be Point"
            );
            let x: i64 = result_bound.getattr("x").unwrap().extract().unwrap();
            let y: i64 = result_bound.getattr("y").unwrap().extract().unwrap();
            assert_eq!(x, 1);
            assert_eq!(y, 2);

            // build
            let instance = point_cls.bind(py).call((), None).expect("call");
            instance.setattr("x", 1).unwrap();
            instance.setattr("y", 2).unwrap();
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            struct_ref
                .build(py, &instance, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[1, 2]);
        });
    }

    // ======================================================================
    // 嵌套结构：外层 Struct + 内层 StructRef
    // ======================================================================

    #[test]
    fn outer_struct_with_inner_structref() {
        with_py(|py| {
            // Inner: { x: Int8ub }
            let inner_cls = define_test_class(py, "Inner", &["x"]);
            install_schema_for_class(py, &inner_cls, vec![("x".to_string(), u8_node())]);

            // Outer: { tag: Int8ub, inner: Inner }
            let outer_cls = define_test_class(py, "Outer", &["tag", "inner"]);
            let outer_struct = Node::Struct(StructNode::new(vec![
                ("tag".to_string(), u8_node()),
                (
                    "inner".to_string(),
                    Node::StructRef(StructRefNode::new(inner_cls.clone_ref(py))),
                ),
            ]));
            let outer_schema = CompiledSchema::new(outer_struct, outer_cls.clone_ref(py));
            let outer_schema_py = Py::new(py, outer_schema).expect("schema");
            outer_cls
                .bind(py)
                .setattr("_construct_compiled", outer_schema_py)
                .expect("setattr");

            // parse b'\xAA\x05'
            let outer_root = outer_cls.bind(py).getattr("_construct_compiled").unwrap();
            let outer_schema_ref: Py<CompiledSchema> = outer_root.extract().unwrap();
            let root_node = outer_schema_ref.bind(py).get().root();

            let mut stream = ParseStream::new(&[0xAA, 0x05]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = root_node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let dict = result.bind(py).downcast::<PyDict>().expect("dict");
            let tag: i64 = dict.get_item("tag").unwrap().unwrap().extract().unwrap();
            assert_eq!(tag, 0xAA);
            // inner 应该是 Inner 实例（StructRef 回调构造）
            let inner_obj = dict.get_item("inner").unwrap().unwrap();
            assert!(
                inner_obj
                    .is_instance(inner_cls.bind(py))
                    .expect("is_instance"),
                "inner should be Inner instance"
            );
            let x: i64 = inner_obj.getattr("x").unwrap().extract().unwrap();
            assert_eq!(x, 5);
        });
    }

    // ======================================================================
    // 未解析引用 → UnresolvedReference
    // ======================================================================

    #[test]
    fn unresolved_when_attribute_missing() {
        // 类无 _construct_compiled 属性
        with_py(|py| {
            let cls = py
                .eval_bound("type('NoCompiled', (), {})", None, None)
                .expect("create type")
                .extract::<Py<PyType>>()
                .expect("extract");
            let struct_ref = StructRefNode::new(cls);

            let mut stream = ParseStream::new(&[0x01]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = struct_ref
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(
                matches!(err, ConstructError::UnresolvedReference { .. }),
                "expected UnresolvedReference, got {:?}",
                err
            );
        });
    }

    #[test]
    fn unresolved_when_attribute_is_none() {
        // _construct_compiled 存在但为 None（延迟桩未解析）
        with_py(|py| {
            let cls = py
                .eval_bound("type('Stub', (), {})", None, None)
                .expect("create type")
                .extract::<Py<PyType>>()
                .expect("extract");
            cls.bind(py)
                .setattr("_construct_compiled", py.None())
                .unwrap();

            let struct_ref = StructRefNode::new(cls);
            let mut stream = ParseStream::new(&[0x01]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = struct_ref
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::UnresolvedReference { message } => {
                    assert!(
                        message.contains("lazy stub") || message.contains("unresolved"),
                        "message should mention lazy stub: {}",
                        message
                    );
                }
                other => panic!("expected UnresolvedReference, got {:?}", other),
            }
        });
    }

    #[test]
    fn unresolved_when_attribute_is_wrong_type() {
        // _construct_compiled 存在但不是 CompiledSchema
        with_py(|py| {
            let cls = py
                .eval_bound("type('Wrong', (), {})", None, None)
                .expect("create type")
                .extract::<Py<PyType>>()
                .expect("extract");
            // 设置为字符串而非 CompiledSchema
            cls.bind(py)
                .setattr("_construct_compiled", "not a schema")
                .unwrap();

            let struct_ref = StructRefNode::new(cls);
            let mut stream = ParseStream::new(&[0x01]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = struct_ref
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(
                matches!(err, ConstructError::UnresolvedReference { .. }),
                "expected UnresolvedReference, got {:?}",
                err
            );
        });
    }

    // ======================================================================
    // 缓存机制
    // ======================================================================

    #[test]
    fn schema_cache_avoids_repeated_lookup() {
        // 首次 resolve_schema 后缓存命中，删除 _construct_compiled 属性
        // 后再次 parse 仍应成功（用缓存）。
        with_py(|py| {
            let inner_cls = define_test_class(py, "Cached", &["x"]);
            install_schema_for_class(py, &inner_cls, vec![("x".to_string(), u8_node())]);

            let struct_ref = StructRefNode::new(inner_cls.clone_ref(py));

            // 第一次 parse（填充缓存）
            let mut stream = ParseStream::new(&[0x01]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let r1 = struct_ref
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("first parse");
            let x1: i64 = r1.bind(py).getattr("x").unwrap().extract().unwrap();
            assert_eq!(x1, 1);

            // 删除 _construct_compiled 属性
            inner_cls
                .bind(py)
                .delattr("_construct_compiled")
                .expect("delattr");

            // 第二次 parse（应从缓存读取，不依赖属性）
            let mut stream2 = ParseStream::new(&[0x02]);
            let mut ctx2 = Context::new_root(py).expect("ctx");
            let mut path2 = Path::new();
            let r2 = struct_ref
                .parse(py, &mut stream2, &mut ctx2, &mut path2)
                .expect("second parse (cached)");
            let x2: i64 = r2.bind(py).getattr("x").unwrap().extract().unwrap();
            assert_eq!(x2, 2);
        });
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_returns_err_because_cannot_resolve_without_gil() {
        with_py(|py| {
            let cls = define_test_class(py, "Sized", &["x"]);
            install_schema_for_class(py, &cls, vec![("x".to_string(), u8_node())]);
            let struct_ref = StructRefNode::new(cls);
            let ctx = Context::new_root(py).expect("ctx");
            let err = struct_ref.sizeof(&ctx).expect_err("should fail");
            assert!(matches!(err, ConstructError::Generic { .. }));
        });
    }

    // ======================================================================
    // 访问器
    // ======================================================================

    #[test]
    fn cls_returns_stored_class_reference() {
        with_py(|py| {
            let cls = py
                .eval_bound("type('Foo', (), {})", None, None)
                .expect("type")
                .extract::<Py<PyType>>()
                .expect("extract");
            let node = StructRefNode::new(cls.clone_ref(py));
            assert!(node.cls().is(&cls));
        });
    }
}
